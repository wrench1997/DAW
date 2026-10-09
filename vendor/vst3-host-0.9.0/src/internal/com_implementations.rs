//! Internal COM interface implementations for VST3

use super::native_edit_transport::{native_edit_channel, NativeEditReceiver, NativeEditSender};
use crate::midi::{PluginEvent, PluginEventData, MAX_EVENT_PAYLOAD_BYTES, MAX_EVENT_TEXT_UNITS};
use crate::plugin::StateContext;
use std::collections::{HashMap, HashSet};
use std::ffi::CStr;
use std::os::raw::c_char;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, ThreadId};
use vst3::{Class, ComPtr, ComRef, ComWrapper, Interface, Steinberg::Vst::*, Steinberg::*};

// Host Application implementation.
//
// Many plugins (u-he, Waves, ...) query the context passed to `IComponent::initialize`
// for `IHostApplication` and dereference it. Passing a null context makes them crash.
// Providing a real host-application object that at least answers `getName` lets them
// initialize. `createInstance` below also vends the host-created objects they ask for
// (IMessage/IAttributeList), used to pass data between a plugin's component and controller.
struct ProgressState {
    notifications: Vec<crate::plugin::HostNotification>,
    active: HashSet<u64>,
    next_id: u64,
}

impl Default for ProgressState {
    fn default() -> Self {
        Self {
            notifications: Vec::with_capacity(MAX_HOST_NOTIFICATIONS),
            active: HashSet::with_capacity(MAX_HOST_NOTIFICATIONS),
            next_id: 1,
        }
    }
}

pub struct HostApplication {
    progress: Mutex<ProgressState>,
    data_exchange: Arc<super::data_exchange::DataExchangeState>,
    #[cfg(target_os = "linux")]
    run_loop: Arc<Mutex<RunLoopRegistry>>,
}

impl Default for HostApplication {
    fn default() -> Self {
        Self {
            progress: Mutex::new(ProgressState::default()),
            data_exchange: super::data_exchange::DataExchangeState::new(),
            #[cfg(target_os = "linux")]
            run_loop: Arc::new(Mutex::new(RunLoopRegistry::new())),
        }
    }
}

impl HostApplication {
    #[cfg(target_os = "linux")]
    pub fn service_run_loop(&self) {
        service_linux_run_loop(&self.run_loop);
    }
    #[cfg(target_os = "linux")]
    pub fn run_loop_cleanup(&self) -> RunLoopCleanup {
        RunLoopCleanup {
            registry: self.run_loop.clone(),
            armed: true,
        }
    }
    #[cfg(target_os = "linux")]
    pub fn clear_run_loop(&self) {
        let retired = self.run_loop.lock().ok().map(|mut reg| {
            reg.closed = true;
            (
                std::mem::take(&mut reg.handlers),
                std::mem::take(&mut reg.timers),
            )
        });
        drop(retired);
    }

    pub fn take_progress_notifications(&self) -> Vec<crate::plugin::HostNotification> {
        let mut state = self
            .progress
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        state.notifications.drain(..).collect()
    }

    pub fn configure_data_exchange(
        &self,
        processor: *mut IAudioProcessor,
        receiver: Option<ComPtr<IDataExchangeReceiver>>,
    ) {
        self.data_exchange.configure(processor, receiver);
    }

    pub fn set_data_exchange_active(&self, active: bool) {
        self.data_exchange.set_active(active);
    }

    pub fn enter_data_exchange_process(&self) {
        self.data_exchange.enter_process();
    }

    pub fn leave_data_exchange_process(&self) {
        self.data_exchange.leave_process();
    }

    pub fn flush_data_exchange(&self) {
        self.data_exchange.flush();
    }

    pub fn take_data_exchange_blocks(&self) -> Vec<crate::plugin::DataExchangeBlock> {
        self.data_exchange.take_blocks()
    }

    pub fn shutdown_data_exchange(&self) {
        self.data_exchange.shutdown();
    }
}

#[cfg(not(target_os = "linux"))]
impl Class for HostApplication {
    // The standard SDK host context implements both IHostApplication and
    // IPlugInterfaceSupport; plugins query the context for either.
    type Interfaces = (
        IHostApplication,
        IPlugInterfaceSupport,
        IProgress,
        IDataExchangeHandler,
    );
}

#[cfg(target_os = "linux")]
impl Class for HostApplication {
    type Interfaces = (
        IHostApplication,
        IPlugInterfaceSupport,
        IProgress,
        IDataExchangeHandler,
        vst3::Steinberg::Linux::IRunLoop,
    );
}

impl IPlugInterfaceSupportTrait for HostApplication {
    unsafe fn isPlugInterfaceSupported(&self, iid: *const TUID) -> tresult {
        if iid.is_null() {
            return kInvalidArgument;
        }
        // This interface describes plug-in-side interfaces the host knows how to consume.
        // Host callbacks such as IComponentHandler are deliberately absent: advertising
        // those reverses the direction of the contract and makes plug-ins enable features
        // whose corresponding plug-in interface the host may never query.
        let bytes = std::slice::from_raw_parts(iid as *const u8, 16);
        let supported = [
            &IConnectionPoint::IID,
            &IMidiMapping::IID,
            &IUnitInfo::IID,
            &IProgramListData::IID,
            &IUnitData::IID,
            &IEditControllerHostEditing::IID,
            &IMidiLearn::IID,
            &IAutomationState::IID,
            &INoteExpressionController::IID,
            &IPlugViewContentScaleSupport::IID,
            &IProcessContextRequirements::IID,
            &IPrefetchableSupport::IID,
            &IRemapParamID::IID,
            &IDataExchangeReceiver::IID,
        ];
        if supported.iter().any(|supported| bytes == &supported[..]) {
            kResultTrue
        } else {
            kResultFalse
        }
    }
}

impl IDataExchangeHandlerTrait for HostApplication {
    unsafe fn openQueue(
        &self,
        processor: *mut IAudioProcessor,
        block_size: u32,
        num_blocks: u32,
        alignment: u32,
        user_context_id: u32,
        out_id: *mut u32,
    ) -> tresult {
        self.data_exchange.open_queue(
            processor,
            block_size,
            num_blocks,
            alignment,
            user_context_id,
            out_id,
        )
    }

    unsafe fn closeQueue(&self, queue_id: u32) -> tresult {
        self.data_exchange.close_queue(queue_id)
    }

    unsafe fn lockBlock(
        &self,
        queue_id: u32,
        block: *mut vst3::Steinberg::Vst::DataExchangeBlock,
    ) -> tresult {
        self.data_exchange.lock_block(queue_id, block)
    }

    unsafe fn freeBlock(&self, queue_id: u32, block_id: u32, send_to_controller: TBool) -> tresult {
        self.data_exchange
            .free_block(queue_id, block_id, send_to_controller != 0)
    }
}

impl IHostApplicationTrait for HostApplication {
    unsafe fn getName(&self, name: *mut String128) -> tresult {
        if name.is_null() {
            return kResultFalse;
        }
        let dst = &mut *name;
        let mut i = 0;
        for ch in "vst3-host".encode_utf16() {
            if i + 1 >= dst.len() {
                break;
            }
            dst[i] = ch;
            i += 1;
        }
        dst[i] = 0;
        kResultOk
    }

    unsafe fn createInstance(
        &self,
        cid: *mut TUID,
        iid: *mut TUID,
        obj: *mut *mut std::ffi::c_void,
    ) -> tresult {
        // Vend the host-created objects plugins ask for (the SDK's HostApplication does
        // this): IMessage and IAttributeList, used to pass data between a plugin's
        // component and controller halves. Anything else fails cleanly.
        if obj.is_null() || cid.is_null() || iid.is_null() {
            return kInvalidArgument;
        }
        *obj = ptr::null_mut();

        // IMessage and IAttributeList use their interface UID as their host-created class UID.
        // Honour both inputs: returning an IMessage pointer for an unrelated requested IID is
        // a COM type confusion bug even when the class id itself is valid.
        let cid_bytes = std::slice::from_raw_parts(cid as *const u8, 16);
        let iid_bytes = std::slice::from_raw_parts(iid as *const u8, 16);
        let matches =
            |expected: &[u8; 16]| cid_bytes == &expected[..] && iid_bytes == &expected[..];

        if matches(&IMessage::IID) {
            if let Some(p) = create_host_message().to_com_ptr::<IMessage>() {
                *obj = p.into_raw() as *mut std::ffi::c_void;
                return kResultTrue;
            }
        } else if matches(&IAttributeList::IID) {
            if let Some(p) = create_host_attribute_list().to_com_ptr::<IAttributeList>() {
                *obj = p.into_raw() as *mut std::ffi::c_void;
                return kResultTrue;
            }
        }
        kNoInterface
    }
}

impl IProgressTrait for HostApplication {
    unsafe fn start(
        &self,
        r#type: IProgress_::ProgressType,
        optional_description: *const tchar,
        out_id: *mut IProgress_::ID,
    ) -> tresult {
        if out_id.is_null() {
            return kInvalidArgument;
        }
        let description = if optional_description.is_null() {
            None
        } else {
            const MAX_PROGRESS_DESCRIPTION_UNITS: usize = 1024;
            let mut units = Vec::with_capacity(64);
            for index in 0..MAX_PROGRESS_DESCRIPTION_UNITS {
                let unit = *optional_description.add(index);
                if unit == 0 {
                    break;
                }
                units.push(unit);
            }
            if units.len() == MAX_PROGRESS_DESCRIPTION_UNITS {
                return kInvalidArgument;
            }
            Some(String::from_utf16_lossy(&units))
        };
        let kind = match r#type {
            IProgress_::ProgressType_::AsyncStateRestoration => {
                crate::plugin::ProgressKind::AsyncStateRestoration
            }
            IProgress_::ProgressType_::UIBackgroundTask => {
                crate::plugin::ProgressKind::UiBackgroundTask
            }
            other => crate::plugin::ProgressKind::Other(other),
        };

        let mut state = self
            .progress
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if state.notifications.len() >= MAX_HOST_NOTIFICATIONS
            || state.active.len() >= MAX_HOST_NOTIFICATIONS
        {
            return kResultFalse;
        }
        let id = state.next_id;
        state.next_id = state.next_id.checked_add(1).unwrap_or(1);
        state.active.insert(id);
        state
            .notifications
            .push(crate::plugin::HostNotification::ProgressStarted {
                id,
                kind,
                description,
            });
        *out_id = id;
        kResultOk
    }

    unsafe fn update(&self, id: IProgress_::ID, norm_value: ParamValue) -> tresult {
        let Some(value) = crate::plugin::ProgressValue::new(norm_value) else {
            return kInvalidArgument;
        };
        let mut state = self
            .progress
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if !state.active.contains(&id) || state.notifications.len() >= MAX_HOST_NOTIFICATIONS {
            return kResultFalse;
        }
        state
            .notifications
            .push(crate::plugin::HostNotification::ProgressUpdated { id, value });
        kResultOk
    }

    unsafe fn finish(&self, id: IProgress_::ID) -> tresult {
        let mut state = self
            .progress
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if !state.active.contains(&id) || state.notifications.len() >= MAX_HOST_NOTIFICATIONS {
            return kResultFalse;
        }
        state.active.remove(&id);
        state
            .notifications
            .push(crate::plugin::HostNotification::ProgressFinished { id });
        kResultOk
    }
}

/// Create a host-application context to pass to `IComponent::initialize`.
pub fn create_host_application() -> ComWrapper<HostApplication> {
    ComWrapper::new(HostApplication::default())
}

/// Log the first off-thread drop, then every `DROP_LOG_INTERVAL`-th one. A plugin that
/// notifies from its processor thread does so per block, so logging unconditionally would
/// emit thousands of lines a second.
const DROP_LOG_INTERVAL: u64 = 256;

/// Host-side connection point which prevents processor-thread messages from invoking the
/// controller directly.
///
/// # Which thread is the "UI thread"
///
/// The gate is `ConnectionPair::connect`'s calling thread — in practice the thread that
/// loaded the plugin, because the pair is built during load. That matches this library's
/// documented threading model: `load_plugin`, `open_editor` and every controller call belong
/// on the host's GUI thread (see `docs/explanation/threading.md`). Load on a worker thread
/// and the gate follows that worker, not your GUI thread.
///
/// # Dropped messages
///
/// `notify` from any other thread is refused with `kResultFalse` and the message is
/// **dropped**, matching the SDK reference host's `ConnectionProxy`, which also has nothing
/// to hand a message to off the UI thread. Plugins that push meter/waveform updates from
/// `process()` therefore lose those updates; their editors typically fall back to polling.
/// The drop is counted ([`ConnectionPair::dropped_message_count`]) and logged rate-limited so
/// it is diagnosable instead of silent. Queueing the message onto the UI thread would need an
/// owned message copy plus a pump the host is not required to run, and is not implemented.
pub struct ConnectionProxy {
    destination: Mutex<Option<ComPtr<IConnectionPoint>>>,
    control_thread: ThreadId,
    /// Messages refused because `notify` came from a thread other than `control_thread`.
    dropped: AtomicU64,
    /// Which direction this proxy carries, for the drop log ("component→controller").
    direction: &'static str,
}

impl ConnectionProxy {
    fn new(
        destination: ComPtr<IConnectionPoint>,
        control_thread: ThreadId,
        direction: &'static str,
    ) -> Self {
        Self {
            destination: Mutex::new(Some(destination)),
            control_thread,
            dropped: AtomicU64::new(0),
            direction,
        }
    }

    fn clear(&self) {
        self.destination
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .take();
    }

    /// Count an off-thread `notify` and log the first one plus every
    /// [`DROP_LOG_INTERVAL`]-th one after it.
    fn record_off_thread_drop(&self) {
        let count = self.dropped.fetch_add(1, Ordering::Relaxed) + 1;
        if count == 1 || count % DROP_LOG_INTERVAL == 0 {
            log::warn!(
                "ConnectionProxy ({}): dropped an off-thread IConnectionPoint::notify \
                 ({count} so far). VST3 requires component↔controller messages on the UI \
                 thread; the plugin sent this one from another thread (usually its processor \
                 thread), so meter/waveform-style updates will not reach its editor.",
                self.direction
            );
        }
    }

    /// How many `notify` calls this proxy has refused for arriving off the UI thread.
    fn dropped_message_count(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

impl Class for ConnectionProxy {
    type Interfaces = (IConnectionPoint,);
}

impl IConnectionPointTrait for ConnectionProxy {
    unsafe fn connect(&self, _other: *mut IConnectionPoint) -> tresult {
        // The endpoints are fixed by `ConnectionPair`; accepting an arbitrary replacement
        // would let a plugin bypass the thread gate.
        kResultFalse
    }

    unsafe fn disconnect(&self, _other: *mut IConnectionPoint) -> tresult {
        kResultFalse
    }

    unsafe fn notify(&self, message: *mut IMessage) -> tresult {
        if message.is_null() {
            return kResultFalse;
        }
        if thread::current().id() != self.control_thread {
            self.record_off_thread_drop();
            return kResultFalse;
        }

        // Clone under the lock, then release it before calling plugin code. `notify` is allowed
        // to re-enter the host (including disconnect/teardown).
        let destination = self
            .destination
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone();
        match destination {
            Some(destination) => destination.notify(message),
            None => kResultFalse,
        }
    }
}

/// The two proxy connections between a separate component and edit controller.
///
/// Both directions are gated to the thread that called [`Self::connect`] — see
/// [`ConnectionProxy`] for what that means and what is dropped.
pub struct ConnectionPair {
    component: ComPtr<IConnectionPoint>,
    controller: ComPtr<IConnectionPoint>,
    component_to_controller: ComWrapper<ConnectionProxy>,
    controller_to_component: ComWrapper<ConnectionProxy>,
    component_connected: AtomicBool,
    controller_connected: AtomicBool,
}

impl ConnectionPair {
    /// Connect both directions, rolling the first direction back if the second fails.
    pub unsafe fn connect(
        component: ComPtr<IConnectionPoint>,
        controller: ComPtr<IConnectionPoint>,
    ) -> Option<Self> {
        let control_thread = thread::current().id();
        let component_to_controller = ComWrapper::new(ConnectionProxy::new(
            controller.clone(),
            control_thread,
            "component→controller",
        ));
        let controller_to_component = ComWrapper::new(ConnectionProxy::new(
            component.clone(),
            control_thread,
            "controller→component",
        ));
        let component_proxy = component_to_controller.to_com_ptr::<IConnectionPoint>()?;
        let controller_proxy = controller_to_component.to_com_ptr::<IConnectionPoint>()?;

        let component_result = component.connect(component_proxy.as_ptr());
        if component_result != kResultOk && component_result != kResultTrue {
            log::warn!("component refused host connection proxy: {component_result:#x}");
            return None;
        }

        let controller_result = controller.connect(controller_proxy.as_ptr());
        if controller_result != kResultOk && controller_result != kResultTrue {
            component.disconnect(component_proxy.as_ptr());
            log::warn!(
                "controller refused host connection proxy; rolled component connection back: \
                 {controller_result:#x}"
            );
            return None;
        }

        Some(Self {
            component,
            controller,
            component_to_controller,
            controller_to_component,
            component_connected: AtomicBool::new(true),
            controller_connected: AtomicBool::new(true),
        })
    }

    /// Total `notify` calls both directions have refused because they arrived off the UI
    /// thread (the thread [`Self::connect`] ran on).
    ///
    /// Non-zero means the plugin is trying to push component↔controller messages from another
    /// thread and those messages are being dropped, exactly as the SDK reference host drops
    /// them. Useful for answering "why is this plugin's meter frozen?" without a debugger.
    pub fn dropped_message_count(&self) -> u64 {
        self.component_to_controller
            .dropped_message_count()
            .saturating_add(self.controller_to_component.dropped_message_count())
    }

    /// Disconnect both plugin endpoints. Safe to call more than once.
    pub unsafe fn disconnect(&self) {
        if self.component_connected.swap(false, Ordering::AcqRel) {
            if let Some(proxy) = self
                .component_to_controller
                .as_com_ref::<IConnectionPoint>()
            {
                self.component.disconnect(proxy.as_ptr());
            }
        }
        if self.controller_connected.swap(false, Ordering::AcqRel) {
            if let Some(proxy) = self
                .controller_to_component
                .as_com_ref::<IConnectionPoint>()
            {
                self.controller.disconnect(proxy.as_ptr());
            }
        }
        self.component_to_controller.clear();
        self.controller_to_component.clear();
    }
}

impl Drop for ConnectionPair {
    fn drop(&mut self) {
        // A closing summary, so the drops are visible even to a host that never polls the
        // counter. The per-drop log is rate-limited and easy to miss in a long session.
        let dropped = self.dropped_message_count();
        if dropped > 0 {
            log::warn!(
                "ConnectionPair: {dropped} component↔controller message(s) were dropped for \
                 arriving off the UI thread over this plugin's lifetime"
            );
        }
        unsafe {
            self.disconnect();
        }
    }
}

// A host-side IAttributeList: a typed key/value bag plugins use (via the host's
// createInstance) to pass data between their component and controller halves.
#[derive(Debug, Clone, PartialEq)]
enum AttrValue {
    Int(i64),
    Float(f64),
    /// UTF-16 (TChar) string, not null-terminated.
    Str(Vec<u16>),
    Bin(Vec<u8>),
}

/// Host implementation of `IAttributeList`.
#[derive(Default)]
pub struct HostAttributeList {
    attrs: Mutex<HashMap<String, AttrValue>>,
}

impl HostAttributeList {
    pub fn new() -> Self {
        Self::default()
    }

    // Safe inner API (also the unit-test surface).
    fn put(&self, key: String, value: AttrValue) {
        if let Ok(mut m) = self.attrs.lock() {
            m.insert(key, value);
        }
    }
    fn get_value(&self, key: &str) -> Option<AttrValue> {
        self.attrs.lock().ok().and_then(|m| m.get(key).cloned())
    }
}

/// Decode an `AttrID` (a C string) into an owned key.
unsafe fn attr_key(id: *const std::os::raw::c_char) -> String {
    if id.is_null() {
        return String::new();
    }
    CStr::from_ptr(id).to_string_lossy().into_owned()
}

impl Class for HostAttributeList {
    type Interfaces = (IAttributeList,);
}

impl IAttributeListTrait for HostAttributeList {
    unsafe fn setInt(&self, id: *const std::os::raw::c_char, value: i64) -> tresult {
        self.put(attr_key(id), AttrValue::Int(value));
        kResultOk
    }
    unsafe fn getInt(&self, id: *const std::os::raw::c_char, value: *mut i64) -> tresult {
        match self.get_value(&attr_key(id)) {
            Some(AttrValue::Int(v)) if !value.is_null() => {
                *value = v;
                kResultOk
            }
            _ => kResultFalse,
        }
    }
    unsafe fn setFloat(&self, id: *const std::os::raw::c_char, value: f64) -> tresult {
        self.put(attr_key(id), AttrValue::Float(value));
        kResultOk
    }
    unsafe fn getFloat(&self, id: *const std::os::raw::c_char, value: *mut f64) -> tresult {
        match self.get_value(&attr_key(id)) {
            Some(AttrValue::Float(v)) if !value.is_null() => {
                *value = v;
                kResultOk
            }
            _ => kResultFalse,
        }
    }
    unsafe fn setString(&self, id: *const std::os::raw::c_char, string: *const u16) -> tresult {
        if string.is_null() {
            return kResultFalse;
        }
        let mut buf = Vec::new();
        let mut p = string;
        while *p != 0 {
            buf.push(*p);
            p = p.add(1);
        }
        self.put(attr_key(id), AttrValue::Str(buf));
        kResultOk
    }
    unsafe fn getString(
        &self,
        id: *const std::os::raw::c_char,
        string: *mut u16,
        size_in_bytes: u32,
    ) -> tresult {
        match self.get_value(&attr_key(id)) {
            Some(AttrValue::Str(v)) if !string.is_null() => {
                // Copy up to capacity-1 chars, then null-terminate.
                let cap_chars = (size_in_bytes as usize / 2).saturating_sub(1);
                let n = v.len().min(cap_chars);
                for (i, &ch) in v.iter().take(n).enumerate() {
                    *string.add(i) = ch;
                }
                *string.add(n) = 0;
                kResultOk
            }
            _ => kResultFalse,
        }
    }
    unsafe fn setBinary(
        &self,
        id: *const std::os::raw::c_char,
        data: *const std::ffi::c_void,
        size_in_bytes: u32,
    ) -> tresult {
        if data.is_null() {
            return kResultFalse;
        }
        let bytes = std::slice::from_raw_parts(data as *const u8, size_in_bytes as usize).to_vec();
        self.put(attr_key(id), AttrValue::Bin(bytes));
        kResultOk
    }
    unsafe fn getBinary(
        &self,
        id: *const std::os::raw::c_char,
        data: *mut *const std::ffi::c_void,
        size_in_bytes: *mut u32,
    ) -> tresult {
        // Note: returns a pointer into the stored buffer; valid until the entry is
        // replaced. VST3 plugins read it synchronously during init, which is safe here.
        if data.is_null() || size_in_bytes.is_null() {
            return kResultFalse;
        }
        if let Ok(m) = self.attrs.lock() {
            if let Some(AttrValue::Bin(v)) = m.get(&attr_key(id)) {
                *data = v.as_ptr() as *const std::ffi::c_void;
                *size_in_bytes = v.len() as u32;
                return kResultOk;
            }
        }
        kResultFalse
    }
}

/// Create a host attribute list.
pub fn create_host_attribute_list() -> ComWrapper<HostAttributeList> {
    ComWrapper::new(HostAttributeList::new())
}

/// Host implementation of `IMessage` (an id + an attribute list), used for
/// component<->controller communication that plugins allocate via the host.
pub struct HostMessage {
    id: Mutex<Option<std::ffi::CString>>,
    attributes: ComWrapper<HostAttributeList>,
}

impl Default for HostMessage {
    fn default() -> Self {
        Self {
            id: Mutex::new(None),
            attributes: create_host_attribute_list(),
        }
    }
}

impl HostMessage {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Class for HostMessage {
    type Interfaces = (IMessage,);
}

impl IMessageTrait for HostMessage {
    unsafe fn getMessageID(&self) -> FIDString {
        // Pointer to the stored id (valid until replaced); null if unset.
        if let Ok(g) = self.id.lock() {
            if let Some(ref s) = *g {
                return s.as_ptr();
            }
        }
        ptr::null()
    }
    unsafe fn setMessageID(&self, id: FIDString) {
        if id.is_null() {
            return;
        }
        let owned = CStr::from_ptr(id).to_owned();
        if let Ok(mut g) = self.id.lock() {
            *g = Some(owned);
        }
    }
    unsafe fn getAttributes(&self) -> *mut IAttributeList {
        // Borrowed pointer to the message's own attribute list (kept alive by `self`).
        self.attributes
            .to_com_ptr::<IAttributeList>()
            .map(|p| p.as_ptr())
            .unwrap_or(ptr::null_mut())
    }
}

/// Create a host message.
pub fn create_host_message() -> ComWrapper<HostMessage> {
    ComWrapper::new(HostMessage::new())
}

// Host-side in-memory `IBStream`. Plugins serialize their state into a stream the host
// provides (`IComponent::getState`) and restore from one the host fills
// (`IComponent::setState`). This backs both with a growable byte buffer plus a cursor.
struct MemBuf {
    data: Vec<u8>,
    pos: usize,
}

/// Cap on how large a plugin can grow a host-provided state stream (and how far it may seek
/// into one). The cursor and the write length both come from the plugin, and the buffer grows
/// to `cursor + length`: without a bound, a wild seek turns the following write into a
/// multi-gigabyte `Vec::resize` — a capacity-overflow panic inside an `extern "system"` vtable
/// thunk, which aborts the process rather than unwinding. 64 MiB is far above any real plugin
/// state (the largest sample-library presets are a few MiB). Mirrors the `MAX_*` caps on every
/// other host-side buffer; over-cap operations fail with a result code instead of allocating.
pub const MAX_STREAM_BYTES: usize = 64 * 1024 * 1024;
const MAX_STREAM_FILENAME_UNITS: usize = 127;

/// The purpose of a host-provided stream, published under the `StateType` key of
/// `IStreamAttributes::getAttributes` so a plugin can tell a project load from a preset load.
///
/// Every variant maps to a string the SDK defines in `Steinberg::Vst::StateType`
/// (`vstpresetkeys.h`) — the `state_type_values_match_the_sdk_constants` test pins each one
/// against `vst3`'s generated constants so a typo cannot ship. `kDefault` is deliberately
/// absent: it means "restored from a preset *marked as default*, or the host wants to store a
/// default state of the plug-in", which is not something this host ever asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamStateType {
    /// `StateType::kProject` — state saved with, or restored from, a host project.
    Project,
    /// `StateType::kTrackPreset` — state saved to, or restored from, a standalone preset
    /// (a `.vstpreset` file).
    TrackPreset,
}

impl StreamStateType {
    fn attribute_value(self) -> &'static str {
        match self {
            Self::Project => "Project",
            Self::TrackPreset => "TrackPreset",
        }
    }
}

impl From<&StateContext> for StreamStateType {
    fn from(context: &StateContext) -> Self {
        match context {
            StateContext::Project => Self::Project,
            StateContext::Preset { .. } => Self::TrackPreset,
        }
    }
}

/// What the host publishes about a stream it hands a plugin: the `IStreamAttributes` entries
/// and the name `IStreamAttributes::getFileName` reports.
///
/// A struct rather than a row of `Option<&str>` parameters — the two string fields mean very
/// different things (a bare file name versus a full path) and would otherwise be trivially
/// swappable at a call site.
#[derive(Debug, Clone, Copy, Default)]
struct StreamMetadata<'a> {
    /// `PresetAttributes::kStateType`. `None` leaves the attribute off entirely, which the
    /// SDK's `Helpers::isProjectState` reports as "the host does not implement this".
    state_type: Option<StreamStateType>,
    /// What `IStreamAttributes::getFileName` returns — "filename (without file extension)".
    file_name: Option<&'a str>,
    /// `PresetAttributes::kFilePathStringType` — "full file path string (if available) where
    /// the preset comes from".
    file_path: Option<&'a str>,
}

impl<'a> StreamMetadata<'a> {
    /// Metadata that says only what kind of state the stream carries.
    fn new(state_type: StreamStateType) -> Self {
        Self {
            state_type: Some(state_type),
            file_name: None,
            file_path: None,
        }
    }

    /// The metadata a `setState` stream should carry for `context`, including the source
    /// file's path and stem when the context names one.
    fn for_state_context(context: &'a StateContext) -> Self {
        let path = context.file_path();
        Self {
            state_type: Some(StreamStateType::from(context)),
            file_name: path.and_then(|p| p.file_stem()).and_then(|s| s.to_str()),
            file_path: path.and_then(|p| p.to_str()),
        }
    }
}

#[cfg(test)]
mod stream_state_type_tests {
    use super::*;

    /// Read one of `vst3`'s `CString` constants (a NUL-terminated `*const c_char`).
    fn sdk_constant(value: *const std::os::raw::c_char) -> &'static str {
        // SAFETY: the argument is a `'static` NUL-terminated literal generated by `vst3`.
        unsafe { CStr::from_ptr(value) }
            .to_str()
            .expect("SDK state-type constants are ASCII")
    }

    #[test]
    fn state_type_values_match_the_sdk_constants() {
        use vst3::Steinberg::Vst::StateType;

        assert_eq!(
            StreamStateType::Project.attribute_value(),
            sdk_constant(StateType::kProject),
        );
        assert_eq!(
            StreamStateType::TrackPreset.attribute_value(),
            sdk_constant(StateType::kTrackPreset),
        );
        // The one defined value this host does not produce; asserted so that adding it later
        // starts from the SDK spelling rather than a guess.
        assert_eq!(sdk_constant(StateType::kDefault), "Default");
    }

    #[test]
    fn the_attribute_is_published_under_the_sdk_key() {
        assert_eq!(sdk_constant(PresetAttributes::kStateType), "StateType");
        assert_eq!(
            sdk_constant(PresetAttributes::kFilePathStringType),
            "FilePathString"
        );
    }

    /// A project restore and a preset load must not look alike to the plugin: `kProject` is
    /// "restored from a project loading", and everything else is what the SDK's
    /// `Helpers::isProjectState` reports as "coming from a preset".
    #[test]
    fn a_state_context_picks_the_matching_state_type() {
        assert_eq!(
            StreamStateType::from(&StateContext::Project),
            StreamStateType::Project
        );
        assert_eq!(
            StreamStateType::from(&StateContext::preset()),
            StreamStateType::TrackPreset
        );
        assert_eq!(
            StreamStateType::from(&StateContext::preset_from_path("/tmp/Lead.vstpreset")),
            StreamStateType::TrackPreset
        );
    }
}

/// Host implementation of `IBStream` over an in-memory buffer.
pub struct MemoryStream {
    inner: Mutex<MemBuf>,
    file_name: Option<Vec<u16>>,
    attributes: ComWrapper<HostAttributeList>,
}

impl MemoryStream {
    #[cfg(test)]
    fn new(data: Vec<u8>) -> Self {
        Self::with_metadata(data, StreamMetadata::default())
    }

    fn with_metadata(data: Vec<u8>, metadata: StreamMetadata<'_>) -> Self {
        let attributes = create_host_attribute_list();
        if let Some(state_type) = metadata.state_type {
            attributes.put(
                "StateType".to_string(),
                AttrValue::Str(state_type.attribute_value().encode_utf16().collect()),
            );
        }
        if let Some(file_path) = metadata.file_path {
            attributes.put(
                "FilePathString".to_string(),
                AttrValue::Str(file_path.encode_utf16().collect()),
            );
        }
        let file_name = metadata.file_name.map(|name| {
            name.encode_utf16()
                .take(MAX_STREAM_FILENAME_UNITS)
                .collect()
        });
        Self {
            inner: Mutex::new(MemBuf { data, pos: 0 }),
            file_name,
            attributes,
        }
    }

    /// A copy of everything written to the stream (used after `getState`).
    pub fn to_vec(&self) -> Vec<u8> {
        self.inner
            .lock()
            .map(|b| b.data.clone())
            .unwrap_or_default()
    }

    // Safe inner ops — also the unit-test surface for the read/write/seek logic.

    /// Write `src` at the cursor, zero-filling any gap a prior seek left past the end, and
    /// return the number of bytes written. `None` when the write would grow the buffer past
    /// [`MAX_STREAM_BYTES`] (or overflow `usize`), leaving the stream untouched.
    fn write_at_cursor(&self, src: &[u8]) -> Option<usize> {
        let mut b = self.inner.lock().ok()?;
        let end = b.pos.checked_add(src.len())?;
        if end > MAX_STREAM_BYTES {
            return None;
        }
        if end > b.data.len() {
            b.data.resize(end, 0);
        }
        let pos = b.pos;
        b.data[pos..end].copy_from_slice(src);
        b.pos = end;
        Some(src.len())
    }

    fn read_at_cursor(&self, n: usize) -> Vec<u8> {
        if let Ok(mut b) = self.inner.lock() {
            let start = b.pos.min(b.data.len());
            let end = (start + n).min(b.data.len());
            let out = b.data[start..end].to_vec();
            b.pos = end;
            out
        } else {
            Vec::new()
        }
    }

    /// Move the cursor and return its new absolute position. A position before the start is
    /// clamped to 0 (a lenient plugin seeking past the beginning still lands on a valid
    /// stream); one past [`MAX_STREAM_BYTES`] is rejected with `None`, because the cursor is
    /// what the next write grows the buffer to.
    fn seek_to(&self, pos: i64, mode: u32) -> Option<i64> {
        let mut b = self.inner.lock().ok()?;
        let base = match mode {
            SEEK_CUR => b.pos as i64,
            SEEK_END => b.data.len() as i64,
            _ => 0, // SEEK_SET
        };
        let new = base.checked_add(pos)?.max(0);
        if new > MAX_STREAM_BYTES as i64 {
            return None;
        }
        b.pos = new as usize;
        Some(new)
    }

    fn position(&self) -> i64 {
        self.inner.lock().map(|b| b.pos as i64).unwrap_or(0)
    }
}

// IBStream seek modes — fixed by the VST3 ABI. kIBSeekSet (0) is the `_` arm in seek_to.
const SEEK_CUR: u32 = 1; // kIBSeekCur
const SEEK_END: u32 = 2; // kIBSeekEnd

impl Class for MemoryStream {
    type Interfaces = (IBStream, IStreamAttributes);
}

impl IBStreamTrait for MemoryStream {
    unsafe fn read(
        &self,
        buffer: *mut std::ffi::c_void,
        num_bytes: i32,
        num_bytes_read: *mut i32,
    ) -> tresult {
        if buffer.is_null() || num_bytes < 0 {
            return kResultFalse;
        }
        let bytes = self.read_at_cursor(num_bytes as usize);
        ptr::copy_nonoverlapping(bytes.as_ptr(), buffer as *mut u8, bytes.len());
        if !num_bytes_read.is_null() {
            *num_bytes_read = bytes.len() as i32;
        }
        kResultOk
    }

    unsafe fn write(
        &self,
        buffer: *mut std::ffi::c_void,
        num_bytes: i32,
        num_bytes_written: *mut i32,
    ) -> tresult {
        if buffer.is_null() || num_bytes < 0 {
            return kResultFalse;
        }
        let src = std::slice::from_raw_parts(buffer as *const u8, num_bytes as usize);
        // A refused write reports zero bytes written and kOutOfMemory: the plugin's own error
        // path is the only correct answer here, since panicking (or aborting on a capacity
        // overflow) would unwind out of a C++ call.
        let Some(written) = self.write_at_cursor(src) else {
            if !num_bytes_written.is_null() {
                *num_bytes_written = 0;
            }
            return kOutOfMemory;
        };
        if !num_bytes_written.is_null() {
            *num_bytes_written = written as i32;
        }
        kResultOk
    }

    unsafe fn seek(&self, pos: i64, mode: i32, result: *mut i64) -> tresult {
        let Some(new) = self.seek_to(pos, mode as u32) else {
            return kInvalidArgument;
        };
        if !result.is_null() {
            *result = new;
        }
        kResultOk
    }

    unsafe fn tell(&self, pos: *mut i64) -> tresult {
        if pos.is_null() {
            return kResultFalse;
        }
        *pos = self.position();
        kResultOk
    }
}

impl IStreamAttributesTrait for MemoryStream {
    unsafe fn getFileName(&self, name: *mut String128) -> tresult {
        if name.is_null() {
            return kInvalidArgument;
        }
        let Some(file_name) = self.file_name.as_ref() else {
            (*name).fill(0);
            return kResultFalse;
        };
        (*name).fill(0);
        let count = file_name.len().min((*name).len().saturating_sub(1));
        (&mut *name)[..count].copy_from_slice(&file_name[..count]);
        kResultOk
    }

    unsafe fn getAttributes(&self) -> *mut IAttributeList {
        self.attributes
            .as_com_ref::<IAttributeList>()
            .map(|attributes| attributes.as_ptr())
            .unwrap_or(ptr::null_mut())
    }
}

/// Create an empty attributed stream for a plugin data/state write.
pub fn create_memory_stream_with_metadata(
    file_name: Option<&str>,
    state_type: StreamStateType,
) -> ComWrapper<MemoryStream> {
    ComWrapper::new(MemoryStream::with_metadata(
        Vec::new(),
        StreamMetadata {
            file_name,
            ..StreamMetadata::new(state_type)
        },
    ))
}

/// Create an attributed stream seeded for a plugin data/state read.
pub fn create_memory_stream_from_with_metadata(
    data: Vec<u8>,
    file_name: Option<&str>,
    state_type: StreamStateType,
) -> ComWrapper<MemoryStream> {
    ComWrapper::new(MemoryStream::with_metadata(
        data,
        StreamMetadata {
            file_name,
            ..StreamMetadata::new(state_type)
        },
    ))
}

/// Create the stream a `setState` call reads from, carrying the attributes that tell the
/// plugin where the bytes came from: the `StateType`, and — when the context names a source
/// file — that file's path and stem.
pub fn create_state_restore_stream(
    data: Vec<u8>,
    context: &StateContext,
) -> ComWrapper<MemoryStream> {
    ComWrapper::new(MemoryStream::with_metadata(
        data,
        StreamMetadata::for_state_context(context),
    ))
}

// --- Linux IRunLoop ------------------------------------------------------
// VSTGUI-based editors (and most non-JUCE plugin UIs) strictly require the
// host frame to also implement `Steinberg::Linux::IRunLoop`: the view
// registers file-descriptor event handlers (its X11 connection) and
// periodic timers with the host, and paints/responds ONLY when the host
// services them. Without this the editor attaches but stays black. The host
// must call `Plugin::service_run_loop()` on its UI thread regularly (every
// frame) while an editor is open.

/// What a plugin's editor registered with the host's run loop, shared
/// between the frame (registration, called by the plugin during attach) and
/// the plugin impl (servicing, driven by the host each UI frame).
#[cfg(target_os = "linux")]
pub struct RunLoopRegistry {
    pub(super) closed: bool,
    next_registration: u64,
    pub handlers: Vec<(
        vst3::ComPtr<vst3::Steinberg::Linux::IEventHandler>,
        vst3::Steinberg::Linux::FileDescriptor,
        u64,
    )>,
    pub timers: Vec<RunLoopTimer>,
}

#[cfg(target_os = "linux")]
pub struct RunLoopTimer {
    pub registration: u64,
    pub handler: vst3::ComPtr<vst3::Steinberg::Linux::ITimerHandler>,
    pub interval_ms: u64,
    pub due: std::time::Instant,
}

#[cfg(target_os = "linux")]
impl RunLoopRegistry {
    pub fn new() -> Self {
        Self {
            handlers: Vec::new(),
            timers: Vec::new(),
            closed: false,
            next_registration: 1,
        }
    }
}

/// Loading failure must release plugin callbacks before the module is unmapped. This guard
/// is declared after the module/factory and disarmed only after PluginImpl owns teardown.
#[cfg(target_os = "linux")]
pub struct RunLoopCleanup {
    registry: Arc<Mutex<RunLoopRegistry>>,
    armed: bool,
}
#[cfg(target_os = "linux")]
impl RunLoopCleanup {
    pub fn disarm(&mut self) {
        self.armed = false;
    }
}
#[cfg(target_os = "linux")]
impl Drop for RunLoopCleanup {
    fn drop(&mut self) {
        if self.armed {
            let retired = self.registry.lock().ok().map(|mut reg| {
                reg.closed = true;
                (
                    std::mem::take(&mut reg.handlers),
                    std::mem::take(&mut reg.timers),
                )
            });
            drop(retired);
        }
    }
}

// The registry holds COM pointers into plugin code. They are only ever
// touched from the host's UI thread (registration inside `open_editor`,
// servicing inside `service_run_loop`, both UI-thread calls); the Send
// bound is inherited from `PluginInternal: Send` storage, the same
// pragmatics as the ComPtrs PluginImpl already holds.
#[cfg(target_os = "linux")]
unsafe impl Send for RunLoopRegistry {}

// Host implementation of `IPlugFrame` (all platforms) plus
// `Linux::IRunLoop` (Linux only). A plugin editor calls `resizeView` to ask
// the host to resize the window hosting its view (recorded; the host polls
// take_editor_resize_request), and on Linux registers its event
// handlers/timers via the IRunLoop half (serviced via
// `Plugin::service_run_loop`).
pub struct HostPlugFrame {
    requested: Arc<Mutex<Option<(i32, i32)>>>,
    #[cfg(target_os = "linux")]
    run_loop: Arc<Mutex<RunLoopRegistry>>,
}

impl HostPlugFrame {
    #[cfg(target_os = "linux")]
    pub fn new(
        requested: Arc<Mutex<Option<(i32, i32)>>>,
        run_loop: Arc<Mutex<RunLoopRegistry>>,
    ) -> Self {
        Self {
            requested,
            run_loop,
        }
    }

    #[cfg(not(target_os = "linux"))]
    pub fn new(requested: Arc<Mutex<Option<(i32, i32)>>>) -> Self {
        Self { requested }
    }
}

#[cfg(target_os = "linux")]
impl Class for HostPlugFrame {
    type Interfaces = (IPlugFrame, vst3::Steinberg::Linux::IRunLoop);
}

#[cfg(not(target_os = "linux"))]
impl Class for HostPlugFrame {
    type Interfaces = (IPlugFrame,);
}

// Registries are bounded and invoked only by the owning helper's main thread.
#[cfg(target_os = "linux")]
const MAX_RUN_LOOP_REGISTRATIONS: usize = 1024;

#[cfg(target_os = "linux")]
macro_rules! impl_linux_run_loop {
    ($host:ty) => {
        impl vst3::Steinberg::Linux::IRunLoopTrait for $host {
            unsafe fn registerEventHandler(
                &self,
                handler: *mut vst3::Steinberg::Linux::IEventHandler,
                fd: vst3::Steinberg::Linux::FileDescriptor,
            ) -> tresult {
                let Some(handler) = vst3::ComRef::from_raw(handler) else {
                    return kInvalidArgument;
                };
                if fd < 0 {
                    return kInvalidArgument;
                }
                match self.run_loop.lock() {
                    Ok(mut reg) => {
                        if reg.closed {
                            return kResultFalse;
                        }
                        if reg
                            .handlers
                            .iter()
                            .any(|(h, old_fd, _)| h.as_ptr() == handler.as_ptr() && *old_fd == fd)
                        {
                            return kResultFalse;
                        }
                        if reg.handlers.len() >= MAX_RUN_LOOP_REGISTRATIONS {
                            return kOutOfMemory;
                        }
                        let registration = reg.next_registration;
                        let Some(next) = registration.checked_add(1) else {
                            return kInternalError;
                        };
                        reg.next_registration = next;
                        reg.handlers.push((handler.to_com_ptr(), fd, registration));
                        kResultOk
                    }
                    Err(_) => kInternalError,
                }
            }
            unsafe fn unregisterEventHandler(
                &self,
                handler: *mut vst3::Steinberg::Linux::IEventHandler,
            ) -> tresult {
                if handler.is_null() {
                    return kInvalidArgument;
                }
                let retired = match self.run_loop.lock() {
                    Ok(mut reg) => {
                        let (removed, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut reg.handlers)
                            .into_iter()
                            .partition(|(h, _, _)| h.as_ptr() == handler);
                        reg.handlers = kept;
                        removed
                    }
                    Err(_) => return kInternalError,
                };
                // Releasing plugin COM references can itself re-enter the registry.
                drop(retired);
                kResultOk
            }
            unsafe fn registerTimer(
                &self,
                handler: *mut vst3::Steinberg::Linux::ITimerHandler,
                milliseconds: vst3::Steinberg::Linux::TimerInterval,
            ) -> tresult {
                let Some(handler) = vst3::ComRef::from_raw(handler) else {
                    return kInvalidArgument;
                };
                let interval_ms = milliseconds.max(1);
                let Some(due) = std::time::Instant::now()
                    .checked_add(std::time::Duration::from_millis(interval_ms))
                else {
                    return kInvalidArgument;
                };
                match self.run_loop.lock() {
                    Ok(mut reg) => {
                        if reg.closed {
                            return kResultFalse;
                        }
                        if reg
                            .timers
                            .iter()
                            .any(|timer| timer.handler.as_ptr() == handler.as_ptr())
                        {
                            return kResultFalse;
                        }
                        if reg.timers.len() >= MAX_RUN_LOOP_REGISTRATIONS {
                            return kOutOfMemory;
                        }
                        let registration = reg.next_registration;
                        let Some(next) = registration.checked_add(1) else {
                            return kInternalError;
                        };
                        reg.next_registration = next;
                        reg.timers.push(RunLoopTimer {
                            handler: handler.to_com_ptr(),
                            interval_ms,
                            due,
                            registration,
                        });
                        kResultOk
                    }
                    Err(_) => kInternalError,
                }
            }
            unsafe fn unregisterTimer(
                &self,
                handler: *mut vst3::Steinberg::Linux::ITimerHandler,
            ) -> tresult {
                if handler.is_null() {
                    return kInvalidArgument;
                }
                let retired = match self.run_loop.lock() {
                    Ok(mut reg) => {
                        let (removed, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut reg.timers)
                            .into_iter()
                            .partition(|t| t.handler.as_ptr() == handler);
                        reg.timers = kept;
                        removed
                    }
                    Err(_) => return kInternalError,
                };
                drop(retired);
                kResultOk
            }
        }
    };
}
#[cfg(target_os = "linux")]
impl_linux_run_loop!(HostPlugFrame);
#[cfg(target_os = "linux")]
impl_linux_run_loop!(HostApplication);

/// Snapshot under the registry lock, call with it released, and recheck membership before
/// every callback. A callback may unregister itself or another due handler reentrantly.
#[cfg(target_os = "linux")]
pub(crate) fn service_linux_run_loop(registry: &Arc<Mutex<RunLoopRegistry>>) {
    use vst3::Steinberg::Linux::{IEventHandlerTrait, ITimerHandlerTrait};
    let now = std::time::Instant::now();
    let mut due = Vec::new();
    if let Ok(mut reg) = registry.lock() {
        for timer in &mut reg.timers {
            if now >= timer.due {
                timer.due = now
                    .checked_add(std::time::Duration::from_millis(timer.interval_ms))
                    .unwrap_or(now);
                due.push((timer.handler.clone(), timer.registration));
            }
        }
    }
    for (handler, registration) in due {
        let registered = registry.lock().is_ok_and(|reg| {
            reg.timers
                .iter()
                .any(|t| t.handler.as_ptr() == handler.as_ptr() && t.registration == registration)
        });
        if registered {
            unsafe { handler.onTimer() };
        }
    }
    let handlers = match registry.lock() {
        Ok(reg) => reg.handlers.clone(),
        Err(_) => return,
    };
    if handlers.is_empty() {
        return;
    }
    let mut fds: Vec<libc::pollfd> = handlers
        .iter()
        .map(|(_, fd, _)| libc::pollfd {
            fd: *fd,
            events: libc::POLLIN,
            revents: 0,
        })
        .collect();
    if unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, 0) } > 0 {
        for (pfd, (handler, fd, registration)) in fds.iter().zip(&handlers) {
            if pfd.revents & (libc::POLLIN | libc::POLLERR | libc::POLLHUP) != 0 {
                let registered = registry.lock().is_ok_and(|reg| {
                    reg.handlers.iter().any(|(h, registered_fd, current)| {
                        h.as_ptr() == handler.as_ptr()
                            && registered_fd == fd
                            && current == registration
                    })
                });
                if registered {
                    unsafe { handler.onFDIsSet(*fd) };
                }
            }
        }
    }
}

impl IPlugFrameTrait for HostPlugFrame {
    unsafe fn resizeView(&self, view: *mut IPlugView, new_size: *mut ViewRect) -> tresult {
        let Some(view) = ComRef::<IPlugView>::from_raw(view) else {
            return kInvalidArgument;
        };
        if new_size.is_null() {
            return kInvalidArgument;
        }
        let r = &*new_size;
        let Some(width) = r.right.checked_sub(r.left).filter(|width| *width > 0) else {
            return kInvalidArgument;
        };
        let Some(height) = r.bottom.checked_sub(r.top).filter(|height| *height > 0) else {
            return kInvalidArgument;
        };

        match self.requested.lock() {
            Ok(mut slot) => *slot = Some((width, height)),
            Err(_) => return kInternalError,
        }
        // Drop the slot lock before invoking plugin code. `onSize` is required in this exact
        // callstack and may re-enter `resizeView`; retaining the mutex here would deadlock and
        // overwriting the slot after the callback would lose the nested request.
        view.onSize(new_size)
    }
}

/// Create a host plug-frame backed by a shared resize-request slot (and, on
/// Linux, a run-loop registry - see `RunLoopRegistry`).
#[cfg(target_os = "linux")]
pub fn create_host_plug_frame(
    requested: Arc<Mutex<Option<(i32, i32)>>>,
    run_loop: Arc<Mutex<RunLoopRegistry>>,
) -> ComWrapper<HostPlugFrame> {
    ComWrapper::new(HostPlugFrame::new(requested, run_loop))
}

/// Create a host plug-frame backed by a shared resize-request slot.
#[cfg(not(target_os = "linux"))]
pub fn create_host_plug_frame(
    requested: Arc<Mutex<Option<(i32, i32)>>>,
) -> ComWrapper<HostPlugFrame> {
    ComWrapper::new(HostPlugFrame::new(requested))
}

/// Cap on buffered editor feedback — the parameter changes and gesture events a plugin's editor
/// reports through `IComponentHandler`. Both are drained only by an optional host poll
/// (`Plugin::get_parameter_changes` / `Plugin::take_parameter_edits`), so a host that never polls
/// would otherwise grow them for the plugin's whole lifetime: dragging a knob emits one
/// `performEdit` per UI frame. Mirrors `MAX_OUTPUT_MIDI` on the outgoing side — pre-reserved so
/// steady-state pushes never reallocate, and pushes past the cap are dropped.
pub const MAX_EDITOR_FEEDBACK: usize = 4096;

/// Cap on the queued host-notification stream — the `IComponentHandler2` / `IUnitHandler` /
/// `IProgress` requests a plugin raises, drained by `Plugin::take_host_notifications`.
///
/// Unlike the editor-feedback caps, reaching this one is reported to the plugin: the handler
/// returns `kResultFalse` from `setDirty` / `requestOpenEditor` / `startGroupEdit` /
/// `finishGroupEdit` / `notifyUnitSelection` / `notifyProgramListChange` /
/// `notifyUnitByBusChange` / `IProgress::start` rather than silently discarding the request,
/// so a plugin that checks its result code learns the host refused it. Hosts must therefore
/// drain `take_host_notifications` regularly (once per UI frame is plenty) or the queue fills
/// and the plugin starts seeing refusals.
pub const MAX_HOST_NOTIFICATIONS: usize = 1024;
const MAX_CONTEXT_MENU_ITEMS: usize = 256;

struct PendingContextMenuItem {
    tag: i32,
    flags: i32,
    target: Option<ComPtr<IContextMenuTarget>>,
}

struct ContextMenuRegistry {
    next_menu_id: AtomicU64,
    pending: Mutex<HashMap<u64, Vec<PendingContextMenuItem>>>,
    owner_thread: ThreadId,
}

impl ContextMenuRegistry {
    fn new() -> Self {
        Self {
            next_menu_id: AtomicU64::new(1),
            pending: Mutex::new(HashMap::with_capacity(MAX_HOST_NOTIFICATIONS)),
            owner_thread: thread::current().id(),
        }
    }

    fn execute(&self, menu_id: u64, item_id: u32) -> crate::Result<()> {
        if thread::current().id() != self.owner_thread {
            return Err(crate::Error::Other(
                "context-menu targets must be invoked on the plugin control thread".to_string(),
            ));
        }
        let (tag, target) = {
            let mut menus = self
                .pending
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            let items = menus.get(&menu_id).ok_or_else(|| {
                crate::Error::Other("context menu is no longer pending".to_string())
            })?;
            let item = items.get(item_id as usize).ok_or_else(|| {
                crate::Error::Other("context-menu item id is out of range".to_string())
            })?;
            if item.flags & IContextMenuItem_::Flags_::kIsDisabled as i32 != 0
                || item.flags & IContextMenuItem_::Flags_::kIsSeparator as i32 != 0
            {
                return Err(crate::Error::Other(
                    "context-menu item is not executable".to_string(),
                ));
            }
            if item.target.is_none() {
                return Err(crate::Error::Other(
                    "context-menu item has no executable target".to_string(),
                ));
            }
            let tag = item.tag;
            let target = item.target.clone();
            menus.remove(&menu_id);
            (tag, target)
        };
        let result = unsafe {
            target
                .as_ref()
                .ok_or_else(|| crate::Error::Other("context-menu target disappeared".to_string()))?
                .executeMenuItem(tag)
        };
        if result == kResultOk || result == kResultTrue {
            Ok(())
        } else {
            Err(crate::Error::Other(format!(
                "plugin rejected context-menu command: {result:#x}"
            )))
        }
    }

    fn dismiss(&self, menu_id: u64) -> crate::Result<()> {
        if thread::current().id() != self.owner_thread {
            return Err(crate::Error::Other(
                "context menus must be dismissed on the plugin control thread".to_string(),
            ));
        }
        if self
            .pending
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remove(&menu_id)
            .is_some()
        {
            Ok(())
        } else {
            Err(crate::Error::Other(
                "context menu is no longer pending".to_string(),
            ))
        }
    }
}

struct StoredContextMenuItem {
    item: IContextMenuItem,
    target: Option<ComPtr<IContextMenuTarget>>,
}

struct HostContextMenu {
    parameter_id: Option<u32>,
    items: Mutex<Vec<StoredContextMenuItem>>,
    notifications: Arc<Mutex<Vec<crate::plugin::HostNotification>>>,
    registry: Arc<ContextMenuRegistry>,
}

impl Class for HostContextMenu {
    type Interfaces = (IContextMenu,);
}

impl IContextMenuTrait for HostContextMenu {
    unsafe fn getItemCount(&self) -> i32 {
        self.items
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .len()
            .min(i32::MAX as usize) as i32
    }

    unsafe fn getItem(
        &self,
        index: i32,
        item: *mut IContextMenuItem,
        target: *mut *mut IContextMenuTarget,
    ) -> tresult {
        if index < 0 || item.is_null() || target.is_null() {
            return kInvalidArgument;
        }
        *target = ptr::null_mut();
        let items = self
            .items
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let Some(stored) = items.get(index as usize) else {
            return kInvalidArgument;
        };
        *item = stored.item;
        if let Some(stored_target) = stored.target.as_ref() {
            let owned_target = stored_target.clone();
            *target = owned_target.as_ptr();
            std::mem::forget(owned_target);
        }
        kResultOk
    }

    unsafe fn addItem(
        &self,
        item: *const IContextMenuItem,
        target: *mut IContextMenuTarget,
    ) -> tresult {
        if item.is_null() {
            return kInvalidArgument;
        }
        let target =
            ComRef::<IContextMenuTarget>::from_raw(target).map(|target| target.to_com_ptr());
        let mut items = self
            .items
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if items.len() >= MAX_CONTEXT_MENU_ITEMS {
            return kResultFalse;
        }
        items.push(StoredContextMenuItem {
            item: *item,
            target,
        });
        kResultOk
    }

    unsafe fn removeItem(
        &self,
        item: *const IContextMenuItem,
        target: *mut IContextMenuTarget,
    ) -> tresult {
        if item.is_null() {
            return kInvalidArgument;
        }
        let requested = &*item;
        let mut items = self
            .items
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let Some(index) = items.iter().position(|stored| {
            stored.item.name == requested.name
                && stored.item.tag == requested.tag
                && stored.item.flags == requested.flags
                && stored
                    .target
                    .as_ref()
                    .map_or(ptr::null_mut(), ComPtr::as_ptr)
                    == target
        }) else {
            return kResultFalse;
        };
        items.remove(index);
        kResultOk
    }

    unsafe fn popup(&self, x: i32, y: i32) -> tresult {
        let items = self
            .items
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let public_items = items
            .iter()
            .enumerate()
            .map(|(index, stored)| crate::plugin::ContextMenuItem {
                item_id: index as u32,
                name: crate::internal::utils::vst_string_to_string(&stored.item.name),
                tag: stored.item.tag,
                flags: stored.item.flags,
            })
            .collect::<Vec<_>>();
        let pending_items = items
            .iter()
            .map(|stored| PendingContextMenuItem {
                tag: stored.item.tag,
                flags: stored.item.flags,
                target: stored.target.clone(),
            })
            .collect::<Vec<_>>();
        drop(items);

        let mut notifications = self
            .notifications
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if notifications.len() >= MAX_HOST_NOTIFICATIONS {
            return kResultFalse;
        }
        let mut pending = self
            .registry
            .pending
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if pending.len() >= MAX_HOST_NOTIFICATIONS {
            return kResultFalse;
        }
        let menu_id = self.registry.next_menu_id.fetch_add(1, Ordering::Relaxed);
        pending.insert(menu_id, pending_items);
        notifications.push(crate::plugin::HostNotification::ContextMenuRequested {
            menu_id,
            parameter_id: self.parameter_id,
            x,
            y,
            items: public_items,
        });
        kResultOk
    }
}

// Component Handler implementation
//
// # Independent delivery and UI streams
//
// Native processor input has its own acknowledged transport. The two UI/control logs below
// are ordered only within themselves, with no cross-ordering between them:
//
// - `edits` — the per-parameter gesture log (`beginEdit`/`performEdit`/`endEdit`), drained by
//   `take_parameter_edits`.
// - `notifications` — the control-plane request log (`IComponentHandler2`, `IUnitHandler`,
//   `IComponentHandler3` context menus, `IProgress`), drained by `take_host_notifications`.
//
// `startGroupEdit` / `finishGroupEdit` therefore arrive as `HostNotification::GroupEditStarted`
// / `GroupEditFinished` in the *notification* stream while the parameter edits they are meant
// to bracket arrive in the *edit* stream. Nothing records where the bracket fell relative to a
// specific `ParameterEdit`, so a host cannot currently tell which edits belonged to a group —
// only that a group was opened and closed at some point between two drains. Treat the brackets
// as an "a multi-parameter change is in flight" hint (e.g. coalesce undo), not as a delimiter.
// Interleaving them into one ordered stream would change the public shape of both accessors
// and is not implemented.
pub struct ComponentHandler {
    // UI-only value feedback. Polling this never consumes processor input.
    pub parameter_changes: Arc<Mutex<Vec<(u32, f64)>>>,
    pub(super) native_edits: NativeEditSender,
    // Ordered log of begin/change/end gestures the editor reports, preserving their order so
    // the host can reconstruct each gesture (drained via `take_parameter_edits`). This is the
    // richer superset of `parameter_changes` (which keeps display values only).
    // Ordered against itself only — see the type comment about the group-edit brackets.
    edits: Arc<Mutex<Vec<crate::plugin::ParameterEdit>>>,
    // Union of every `restartComponent` flag the plugin has raised since the host last drained
    // it. A bitmask rather than a log: the flags are idempotent requests ("my latency changed",
    // "re-read my parameters"), so accumulating them is both complete and inherently bounded —
    // a plugin that spams restartComponent while nothing polls costs one word, not a queue.
    restart_flags: AtomicI32,
    // Ordered IComponentHandler2 / IUnitHandler / IProgress / context-menu requests. These are
    // control-plane work items, never executed from inside the plugin callback. Ordered against
    // themselves only — not against `edits`. Capped at MAX_HOST_NOTIFICATIONS, and a push past
    // the cap is *refused* (kResultFalse to the plugin) rather than dropped, so a host that
    // never drains makes the plugin's own requests start failing.
    notifications: Arc<Mutex<Vec<crate::plugin::HostNotification>>>,
    context_menus: Arc<ContextMenuRegistry>,
}

impl ComponentHandler {
    pub fn new(parameter_changes: Arc<Mutex<Vec<(u32, f64)>>>) -> (Self, NativeEditReceiver) {
        let (native_edits, receiver) = native_edit_channel(MAX_EDITOR_FEEDBACK);
        (
            ComponentHandler {
                native_edits,
                parameter_changes,
                edits: Arc::new(Mutex::new(Vec::with_capacity(MAX_EDITOR_FEEDBACK))),
                restart_flags: AtomicI32::new(0),
                notifications: Arc::new(Mutex::new(Vec::with_capacity(MAX_HOST_NOTIFICATIONS))),
                context_menus: Arc::new(ContextMenuRegistry::new()),
            },
            receiver,
        )
    }

    fn mark_native_dirty(&self) {
        let _ = self.native_edits.mark_dirty();
    }

    pub fn native_dirty_revision(&self) -> crate::Result<u64> {
        self.native_edits.dirty_revision().map_err(|error| {
            crate::Error::Other(format!("native edit revision unavailable: {error:?}"))
        })
    }

    /// Capture is permitted only after checked admission and successful Process acknowledged
    /// every submitted native value. Display drains never acknowledge processor delivery.
    pub fn native_state_capture_revision(&self) -> crate::Result<u64> {
        self.native_edits.capture_revision().map_err(|error| {
            crate::Error::Other(format!("native edit capture fence is not ready: {error:?}"))
        })
    }

    /// Take the accumulated `restartComponent` flags, clearing them.
    pub fn take_restart_flags(&self) -> crate::plugin::RestartFlags {
        crate::plugin::RestartFlags::from_bits(self.restart_flags.swap(0, Ordering::AcqRel))
    }

    /// Drain the ordered parameter-edit gesture log accumulated since the last call.
    ///
    /// Ordered relative to other parameter edits only. The `startGroupEdit`/`finishGroupEdit`
    /// brackets live in the separate [`Self::take_host_notifications`] stream with no recorded
    /// interleaving, so the edits that a group covered cannot be identified — see the type
    /// comment on [`ComponentHandler`].
    ///
    /// Capped at [`MAX_EDITOR_FEEDBACK`]; gestures past the cap are dropped (the plugin is not
    /// told, because `IComponentHandler` has no result code a plugin acts on here).
    pub fn take_parameter_edits(&self) -> Vec<crate::plugin::ParameterEdit> {
        // A COM FFI callback could be mid-push when a previous one panicked; recover the lock
        // rather than propagating a poison panic across the boundary.
        let mut edits = self.edits.lock().unwrap_or_else(|p| p.into_inner());
        // Drain in place rather than `mem::take`: taking the `Vec` would leave a zero-capacity
        // buffer behind, so the next editor gesture would reallocate on the COM callback path.
        edits.drain(..).collect()
    }

    /// Drain ordered host requests raised through `IComponentHandler2`, `IUnitHandler`,
    /// `IComponentHandler3` and `IProgress`.
    ///
    /// Ordered relative to other notifications only — never against
    /// [`Self::take_parameter_edits`]. In particular `GroupEditStarted`/`GroupEditFinished`
    /// cannot be correlated with the parameter edits they bracket; see the type comment on
    /// [`ComponentHandler`].
    ///
    /// **Drain this regularly.** The queue is capped at [`MAX_HOST_NOTIFICATIONS`] and a push
    /// past the cap returns `kResultFalse` to the plugin, so a host that never drains starts
    /// making the plugin's own `setDirty` / `requestOpenEditor` / group-edit /
    /// progress-reporting calls fail.
    pub fn take_host_notifications(&self) -> Vec<crate::plugin::HostNotification> {
        let mut notifications = self
            .notifications
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        notifications.drain(..).collect()
    }

    pub fn execute_context_menu_item(&self, menu_id: u64, item_id: u32) -> crate::Result<()> {
        self.context_menus.execute(menu_id, item_id)
    }

    pub fn dismiss_context_menu(&self, menu_id: u64) -> crate::Result<()> {
        self.context_menus.dismiss(menu_id)
    }

    // Append a gesture event, recovering a poisoned lock (these run on the COM FFI callback
    // path, where a panic would unwind across the C++ boundary — UB).
    fn push_edit(&self, edit: crate::plugin::ParameterEdit) {
        let mut edits = self.edits.lock().unwrap_or_else(|p| p.into_inner());
        if edits.len() < MAX_EDITOR_FEEDBACK {
            edits.push(edit);
        }
    }

    fn push_notification(&self, notification: crate::plugin::HostNotification) -> bool {
        let mut notifications = self
            .notifications
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if notifications.len() >= MAX_HOST_NOTIFICATIONS {
            return false;
        }
        notifications.push(notification);
        true
    }
}

impl Class for ComponentHandler {
    type Interfaces = (
        IComponentHandler,
        IComponentHandler2,
        IComponentHandler3,
        IUnitHandler,
        IUnitHandler2,
    );
}

impl IComponentHandlerTrait for ComponentHandler {
    unsafe fn beginEdit(&self, id: u32) -> i32 {
        log::debug!("Host: Begin edit for parameter {}", id);
        self.push_edit(crate::plugin::ParameterEdit {
            id,
            kind: crate::plugin::ParameterEditKind::BeginGesture,
            value: None,
        });
        kResultOk
    }

    unsafe fn performEdit(&self, id: u32, value_normalized: f64) -> i32 {
        // Publish independently of both UI locks. Rejection records durable dirty/loss
        // evidence; an empty display queue can never make capture appear current.
        let _ = self.native_edits.send(id, value_normalized);
        log::debug!(
            "Host: Perform edit for parameter {} = {}",
            id,
            value_normalized
        );
        // Independent bounded display feedback; overflow here is not DSP delivery loss.
        match self.parameter_changes.lock() {
            Ok(mut changes) if changes.len() < MAX_EDITOR_FEEDBACK => {
                changes.push((id, value_normalized));
            }
            _ => {}
        }
        // ...and as an ordered gesture event for the richer `take_parameter_edits` drain.
        self.push_edit(crate::plugin::ParameterEdit {
            id,
            kind: crate::plugin::ParameterEditKind::ValueChange,
            value: Some(value_normalized),
        });
        kResultOk
    }

    unsafe fn endEdit(&self, id: u32) -> i32 {
        log::debug!("Host: End edit for parameter {}", id);
        self.push_edit(crate::plugin::ParameterEdit {
            id,
            kind: crate::plugin::ParameterEditKind::EndGesture,
            value: None,
        });
        kResultOk
    }

    unsafe fn restartComponent(&self, flags: i32) -> i32 {
        if flags & (RestartFlags_::kParamValuesChanged | RestartFlags_::kReloadComponent) != 0 {
            self.mark_native_dirty();
        }
        log::debug!("Host: Restart component requested with flags: {flags:#x}");
        // Recorded for the host to poll (`Plugin::take_restart_flags`), not acted on here: the
        // host decides what a restart means for it. See `RestartFlags` for which flags this
        // library handles on the host's behalf (none, currently) and which need host action.
        self.restart_flags.fetch_or(flags, Ordering::AcqRel);
        kResultOk
    }
}

impl IComponentHandler3Trait for ComponentHandler {
    unsafe fn createContextMenu(
        &self,
        plug_view: *mut IPlugView,
        parameter_id: *const u32,
    ) -> *mut IContextMenu {
        if plug_view.is_null() {
            return ptr::null_mut();
        }
        let menu = ComWrapper::new(HostContextMenu {
            parameter_id: parameter_id.as_ref().copied(),
            items: Mutex::new(Vec::with_capacity(16)),
            notifications: self.notifications.clone(),
            registry: self.context_menus.clone(),
        });
        let Some(menu) = menu.to_com_ptr::<IContextMenu>() else {
            return ptr::null_mut();
        };
        let raw = menu.as_ptr();
        std::mem::forget(menu);
        raw
    }
}

impl IComponentHandler2Trait for ComponentHandler {
    unsafe fn setDirty(&self, state: u8) -> i32 {
        if state != 0 {
            self.mark_native_dirty();
        }
        log::debug!("Host: Plugin marked state as dirty (state: {})", state);
        if self.push_notification(crate::plugin::HostNotification::DirtyChanged(state != 0)) {
            kResultOk
        } else {
            kResultFalse
        }
    }

    unsafe fn requestOpenEditor(&self, name: *const std::os::raw::c_char) -> i32 {
        log::debug!("Host: Plugin requested editor open");
        let name = if name.is_null() {
            None
        } else {
            Some(CStr::from_ptr(name).to_string_lossy().into_owned())
        };
        if self.push_notification(crate::plugin::HostNotification::OpenEditorRequested { name }) {
            kResultOk
        } else {
            kResultFalse
        }
    }

    // The group brackets land in the notification stream while the edits they bracket land in
    // the gesture stream; nothing records the interleaving. See the `ComponentHandler` type
    // comment for what a host can and cannot conclude from them.
    unsafe fn startGroupEdit(&self) -> i32 {
        log::debug!("Host: Plugin started group edit");
        if self.push_notification(crate::plugin::HostNotification::GroupEditStarted) {
            kResultOk
        } else {
            kResultFalse
        }
    }

    unsafe fn finishGroupEdit(&self) -> i32 {
        log::debug!("Host: Plugin finished group edit");
        if self.push_notification(crate::plugin::HostNotification::GroupEditFinished) {
            kResultOk
        } else {
            kResultFalse
        }
    }
}

impl IUnitHandlerTrait for ComponentHandler {
    unsafe fn notifyUnitSelection(&self, unit_id: i32) -> tresult {
        if self.push_notification(crate::plugin::HostNotification::UnitSelectionChanged { unit_id })
        {
            kResultOk
        } else {
            kResultFalse
        }
    }

    unsafe fn notifyProgramListChange(&self, list_id: i32, program_index: i32) -> tresult {
        if self.push_notification(crate::plugin::HostNotification::ProgramListChanged {
            list_id,
            program_index: (program_index >= 0).then_some(program_index),
        }) {
            kResultOk
        } else {
            kResultFalse
        }
    }
}

impl IUnitHandler2Trait for ComponentHandler {
    unsafe fn notifyUnitByBusChange(&self) -> tresult {
        if self.push_notification(crate::plugin::HostNotification::UnitByBusChanged) {
            kResultOk
        } else {
            kResultFalse
        }
    }
}

// Event List implementation
/// Cap on queued events. The input list's only drain is `process()`, which returns early while
/// the plugin isn't processing, so a host that sends MIDI to a stopped plugin would otherwise
/// grow this forever (and then dump every stale event into the first block once it starts). Far
/// above any single block's working set; same pre-reserve/drop-when-full policy as
/// `MAX_OUTPUT_MIDI`.
pub const MAX_QUEUED_EVENTS: usize = 4096;
const MAX_QUEUED_EVENT_PAYLOAD_BYTES: usize = 8 * 1024 * 1024;

/// Fixed admission outcomes; rejecting an event never changes the queued events or budget.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EventAdmissionError {
    QueueFull,
    PayloadBudget,
    InvalidEvent,
    Poisoned,
}

pub struct HostEventList {
    pub events: Mutex<Vec<PluginEvent>>,
    payload_bytes: AtomicUsize,
    lost: AtomicBool,
}

impl HostEventList {
    pub fn new() -> Self {
        Self {
            events: Mutex::new(Vec::with_capacity(MAX_QUEUED_EVENTS)),
            payload_bytes: AtomicUsize::new(0),
            lost: AtomicBool::new(false),
        }
    }

    /// Acknowledge any rejected/uncaptured event since the last loss-aware drain.
    pub fn take_loss(&self) -> bool {
        self.lost.swap(false, Ordering::AcqRel)
    }

    pub fn clear(&self) {
        match self.events.lock() {
            Ok(mut events) => {
                events.clear();
                self.payload_bytes.store(0, Ordering::Relaxed);
                log::trace!("HostEventList: Cleared all events");
            }
            Err(_) => {
                self.lost.store(true, Ordering::Release);
                log::error!("HostEventList: Failed to lock events for clear");
            }
        }
    }

    /// Move queued events into reusable optional slots for allocation-free chunk routing.
    pub fn take_into_slots(&self, out: &mut Vec<Option<PluginEvent>>) {
        out.clear();
        if let Ok(mut events) = self.events.lock() {
            out.extend(events.drain(..).map(Some));
            self.payload_bytes.store(0, Ordering::Relaxed);
        }
    }

    /// Replace the queued events with `events`, reusing the existing allocation. Capped like
    /// every other path into the list; the excess is dropped with a warning.
    pub fn reset_with(&self, events: impl IntoIterator<Item = PluginEvent>) {
        let Ok(mut queued) = self.events.lock() else {
            self.lost.store(true, Ordering::Release);
            log::error!("HostEventList: Failed to lock events for reset_with");
            return;
        };
        queued.clear();
        let mut payload_bytes = 0usize;
        for event in events {
            if queued.len() >= MAX_QUEUED_EVENTS {
                self.lost.store(true, Ordering::Release);
                log::warn!("HostEventList: dropping event, queue full at {MAX_QUEUED_EVENTS}");
                break;
            }
            let next_payload_bytes = payload_bytes.saturating_add(event.payload_bytes());
            if next_payload_bytes > MAX_QUEUED_EVENT_PAYLOAD_BYTES {
                self.lost.store(true, Ordering::Release);
                log::warn!(
                    "HostEventList: dropping event, payload budget exceeds \
                     {MAX_QUEUED_EVENT_PAYLOAD_BYTES} bytes"
                );
                continue;
            }
            payload_bytes = next_payload_bytes;
            queued.push(event);
        }
        self.payload_bytes.store(payload_bytes, Ordering::Relaxed);
    }

    /// True if the list currently holds no events.
    pub fn is_empty(&self) -> bool {
        self.events
            .lock()
            .map(|events| events.is_empty())
            .unwrap_or_else(|_| {
                self.lost.store(true, Ordering::Release);
                true
            })
    }

    /// Move each queued event into `f`, leaving the list empty while retaining its backing
    /// allocation. Pointer-backed payloads therefore cross into the output queue without a
    /// second allocation or byte copy on the audio thread.
    pub fn drain_each(&self, mut f: impl FnMut(PluginEvent)) {
        if let Ok(mut events) = self.events.lock() {
            for event in events.drain(..) {
                f(event);
            }
            self.payload_bytes.store(0, Ordering::Relaxed);
        } else {
            self.lost.store(true, Ordering::Release);
        }
    }

    fn reject_admission(&self, error: EventAdmissionError) -> EventAdmissionError {
        self.lost.store(true, Ordering::Release);
        error
    }

    // The caller holds the events guard until it publishes the event and this new total.
    fn admitted_payload_total(&self, additional: usize) -> Result<usize, EventAdmissionError> {
        self.payload_bytes
            .load(Ordering::Relaxed)
            .checked_add(additional)
            .filter(|total| *total <= MAX_QUEUED_EVENT_PAYLOAD_BYTES)
            .ok_or(EventAdmissionError::PayloadBudget)
    }

    /// Admit an already-owned event without changing the queue or budget on rejection.
    /// Dropping a rejected event may free its existing payload allocation.
    pub(crate) fn try_add_event(&self, event: PluginEvent) -> Result<(), EventAdmissionError> {
        let mut events = self
            .events
            .lock()
            .map_err(|_| self.reject_admission(EventAdmissionError::Poisoned))?;
        if events.len() >= MAX_QUEUED_EVENTS {
            return Err(self.reject_admission(EventAdmissionError::QueueFull));
        }
        let payload_bytes =
            owned_event_payload_bytes(&event).map_err(|error| self.reject_admission(error))?;
        let next_payload_bytes = self
            .admitted_payload_total(payload_bytes)
            .map_err(|error| self.reject_admission(error))?;
        events.push(event);
        self.payload_bytes
            .store(next_payload_bytes, Ordering::Relaxed);
        Ok(())
    }

    /// Check metadata and both budgets before deep-copying a raw SDK event.
    /// The same guard covers preflight, copying, and publication, so rejection leaves
    /// both the queued events and their payload-byte total unchanged.
    ///
    /// # Safety
    /// Unless the header queue is full, the union member selected by the event type
    /// must be initialized so its metadata can be inspected.
    /// A pointer-backed payload must remain readable for its declared nonzero length
    /// while this call copies it. No payload pointer is dereferenced if the header
    /// queue is full, the payload metadata is invalid, or the aggregate budget is
    /// exceeded; those cases do not require a readable payload. Metadata checks alone
    /// cannot establish that an otherwise admissible pointer is readable.
    pub(crate) unsafe fn try_add_raw_event(
        &self,
        event: &Event,
    ) -> Result<(), EventAdmissionError> {
        let mut events = self
            .events
            .lock()
            .map_err(|_| self.reject_admission(EventAdmissionError::Poisoned))?;
        if events.len() >= MAX_QUEUED_EVENTS {
            return Err(self.reject_admission(EventAdmissionError::QueueFull));
        }
        let payload_bytes = unsafe { raw_event_payload_bytes(event) }
            .map_err(|error| self.reject_admission(error))?;
        let next_payload_bytes = self
            .admitted_payload_total(payload_bytes)
            .map_err(|error| self.reject_admission(error))?;
        let owned = unsafe { raw_event_to_plugin_event(event) }
            .map_err(|()| self.reject_admission(EventAdmissionError::InvalidEvent))?;
        events.push(owned);
        self.payload_bytes
            .store(next_payload_bytes, Ordering::Relaxed);
        Ok(())
    }
}

impl Default for HostEventList {
    fn default() -> Self {
        Self::new()
    }
}

impl Class for HostEventList {
    type Interfaces = (IEventList,);
}

impl IEventListTrait for HostEventList {
    unsafe fn getEventCount(&self) -> i32 {
        match self.events.lock() {
            Ok(events) => events.len() as i32,
            Err(_) => {
                log::error!("HostEventList: Failed to lock events for getEventCount");
                0
            }
        }
    }

    unsafe fn getEvent(&self, index: i32, event: *mut Event) -> i32 {
        if event.is_null() {
            log::warn!("HostEventList: getEvent called with null event pointer");
            return kResultFalse;
        }

        if index < 0 {
            log::warn!(
                "HostEventList: getEvent called with negative index: {}",
                index
            );
            return kResultFalse;
        }

        match self.events.lock() {
            Ok(events) => {
                if let Some(e) = events.get(index as usize) {
                    match plugin_event_to_raw(e) {
                        Ok(raw) => {
                            *event = raw;
                            kResultOk
                        }
                        Err(()) => kResultFalse,
                    }
                } else {
                    log::warn!(
                        "HostEventList: getEvent index {} out of bounds (count: {})",
                        index,
                        events.len()
                    );
                    kResultFalse
                }
            }
            Err(_) => {
                log::error!("HostEventList: Failed to lock events for getEvent");
                kResultFalse
            }
        }
    }

    unsafe fn addEvent(&self, event: *mut Event) -> i32 {
        if event.is_null() || !event.is_aligned() {
            self.reject_admission(EventAdmissionError::InvalidEvent);
            return kResultFalse;
        }

        match unsafe { self.try_add_raw_event(&*event) } {
            Ok(()) => kResultOk,
            Err(_) => kResultFalse,
        }
    }
}

#[allow(non_upper_case_globals, clippy::unnecessary_cast)]
fn plugin_event_to_raw(event: &PluginEvent) -> std::result::Result<Event, ()> {
    use Event_::EventTypes_::*;

    let mut raw: Event = unsafe { std::mem::zeroed() };
    raw.busIndex = event.bus_index;
    raw.sampleOffset = event.sample_offset;
    raw.ppqPosition = event.ppq_position;
    raw.flags = event.flags;
    match &event.data {
        PluginEventData::NoteOn {
            channel,
            pitch,
            tuning,
            velocity,
            length,
            note_id,
        } => {
            raw.r#type = kNoteOnEvent as u16;
            raw.__field0.noteOn = NoteOnEvent {
                channel: *channel,
                pitch: *pitch,
                tuning: *tuning,
                velocity: *velocity,
                length: *length,
                noteId: *note_id,
            };
        }
        PluginEventData::NoteOff {
            channel,
            pitch,
            velocity,
            note_id,
            tuning,
        } => {
            raw.r#type = kNoteOffEvent as u16;
            raw.__field0.noteOff = NoteOffEvent {
                channel: *channel,
                pitch: *pitch,
                velocity: *velocity,
                noteId: *note_id,
                tuning: *tuning,
            };
        }
        PluginEventData::Data { data_type, bytes } => {
            let size = u32::try_from(bytes.len()).map_err(|_| ())?;
            raw.r#type = kDataEvent as u16;
            raw.__field0.data = DataEvent {
                size,
                r#type: *data_type,
                bytes: bytes.as_ptr(),
            };
        }
        PluginEventData::PolyPressure {
            channel,
            pitch,
            pressure,
            note_id,
        } => {
            raw.r#type = kPolyPressureEvent as u16;
            raw.__field0.polyPressure = PolyPressureEvent {
                channel: *channel,
                pitch: *pitch,
                pressure: *pressure,
                noteId: *note_id,
            };
        }
        PluginEventData::NoteExpressionValue {
            type_id,
            note_id,
            value,
        } => {
            raw.r#type = kNoteExpressionValueEvent as u16;
            raw.__field0.noteExpressionValue = NoteExpressionValueEvent {
                typeId: *type_id,
                noteId: *note_id,
                value: *value,
            };
        }
        PluginEventData::NoteExpressionText {
            type_id,
            note_id,
            text,
        } => {
            raw.r#type = kNoteExpressionTextEvent as u16;
            raw.__field0.noteExpressionText = NoteExpressionTextEvent {
                typeId: *type_id,
                noteId: *note_id,
                textLen: u32::try_from(text.len()).map_err(|_| ())?,
                text: text.as_ptr(),
            };
        }
        PluginEventData::NoteExpressionIntValue {
            type_id,
            note_id,
            value,
        } => {
            raw.r#type = kNoteExpressionIntValueEvent as u16;
            raw.__field0.noteExpressionIntValue = NoteExpressionIntValueEvent {
                typeId: *type_id,
                noteId: *note_id,
                value: *value,
            };
        }
        PluginEventData::Chord {
            root,
            bass_note,
            mask,
            text,
        } => {
            raw.r#type = kChordEvent as u16;
            raw.__field0.chord = ChordEvent {
                root: *root,
                bassNote: *bass_note,
                mask: *mask,
                textLen: u16::try_from(text.len()).map_err(|_| ())?,
                text: text.as_ptr(),
            };
        }
        PluginEventData::Scale { root, mask, text } => {
            raw.r#type = kScaleEvent as u16;
            raw.__field0.scale = ScaleEvent {
                root: *root,
                mask: *mask,
                textLen: u16::try_from(text.len()).map_err(|_| ())?,
                text: text.as_ptr(),
            };
        }
        PluginEventData::LegacyMidiCcOut {
            control_number,
            channel,
            value,
            value2,
        } => {
            raw.r#type = kLegacyMIDICCOutEvent as u16;
            // VST3's `int8` is `c_char`, which is signed on macOS/x86 and unsigned on ARM
            // Linux, so these fields must be cast rather than assigned.
            raw.__field0.midiCCOut = LegacyMIDICCOutEvent {
                controlNumber: *control_number,
                channel: *channel as c_char,
                value: *value as c_char,
                value2: *value2 as c_char,
            };
        }
    }
    Ok(raw)
}

fn checked_payload_bytes<T>(len: usize, max_len: usize) -> Result<usize, EventAdmissionError> {
    if len > max_len {
        return Err(EventAdmissionError::InvalidEvent);
    }
    len.checked_mul(std::mem::size_of::<T>())
        .ok_or(EventAdmissionError::InvalidEvent)
}

fn owned_event_payload_bytes(event: &PluginEvent) -> Result<usize, EventAdmissionError> {
    match &event.data {
        PluginEventData::Data { bytes, .. } => {
            checked_payload_bytes::<u8>(bytes.len(), MAX_EVENT_PAYLOAD_BYTES)
        }
        PluginEventData::NoteExpressionText { text, .. }
        | PluginEventData::Chord { text, .. }
        | PluginEventData::Scale { text, .. } => {
            checked_payload_bytes::<u16>(text.len(), MAX_EVENT_TEXT_UNITS)
        }
        PluginEventData::NoteOn { .. }
        | PluginEventData::NoteOff { .. }
        | PluginEventData::PolyPressure { .. }
        | PluginEventData::NoteExpressionValue { .. }
        | PluginEventData::NoteExpressionIntValue { .. }
        | PluginEventData::LegacyMidiCcOut { .. } => Ok(0),
    }
}

// Inspect only the declared metadata. Do not construct a slice or read any payload
// until both the individual bounds and the list's aggregate budget have passed.
fn checked_raw_payload_bytes<T>(
    pointer: *const T,
    len: usize,
    max_len: usize,
) -> Result<usize, EventAdmissionError> {
    let bytes = checked_payload_bytes::<T>(len, max_len)?;
    if len != 0 && (pointer.is_null() || !pointer.is_aligned()) {
        return Err(EventAdmissionError::InvalidEvent);
    }
    Ok(bytes)
}

// The union member selected by the event type must be initialized. Payload
// pointer validity is deliberately not required for this metadata-only preflight.
#[allow(non_upper_case_globals, clippy::unnecessary_cast)]
unsafe fn raw_event_payload_bytes(raw: &Event) -> Result<usize, EventAdmissionError> {
    use Event_::EventTypes_::*;

    // All fields read here are plain SDK scalars/pointers. The payload is not read.
    unsafe {
        match raw.r#type as u32 {
            t if t == kDataEvent as u32 => {
                let value = raw.__field0.data;
                checked_raw_payload_bytes(
                    value.bytes,
                    usize::try_from(value.size).map_err(|_| EventAdmissionError::InvalidEvent)?,
                    MAX_EVENT_PAYLOAD_BYTES,
                )
            }
            t if t == kNoteExpressionTextEvent as u32 => {
                let value = raw.__field0.noteExpressionText;
                checked_raw_payload_bytes(
                    value.text,
                    usize::try_from(value.textLen)
                        .map_err(|_| EventAdmissionError::InvalidEvent)?,
                    MAX_EVENT_TEXT_UNITS,
                )
            }
            t if t == kChordEvent as u32 => {
                let value = raw.__field0.chord;
                checked_raw_payload_bytes(
                    value.text,
                    usize::from(value.textLen),
                    MAX_EVENT_TEXT_UNITS,
                )
            }
            t if t == kScaleEvent as u32 => {
                let value = raw.__field0.scale;
                checked_raw_payload_bytes(
                    value.text,
                    usize::from(value.textLen),
                    MAX_EVENT_TEXT_UNITS,
                )
            }
            t if t == kNoteOnEvent as u32
                || t == kNoteOffEvent as u32
                || t == kPolyPressureEvent as u32
                || t == kNoteExpressionValueEvent as u32
                || t == kNoteExpressionIntValueEvent as u32
                || t == kLegacyMIDICCOutEvent as u32 =>
            {
                Ok(0)
            }
            _ => Err(EventAdmissionError::InvalidEvent),
        }
    }
}

#[allow(non_upper_case_globals, clippy::unnecessary_cast)]
unsafe fn raw_event_to_plugin_event(raw: &Event) -> std::result::Result<PluginEvent, ()> {
    use Event_::EventTypes_::*;

    unsafe fn copy_bytes(ptr: *const u8, len: usize) -> std::result::Result<Vec<u8>, ()> {
        checked_raw_payload_bytes(ptr, len, MAX_EVENT_PAYLOAD_BYTES).map_err(|_| ())?;
        Ok(if len == 0 {
            Vec::new()
        } else {
            unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec()
        })
    }

    unsafe fn copy_text(ptr: *const u16, len: usize) -> std::result::Result<Vec<u16>, ()> {
        checked_raw_payload_bytes(ptr, len, MAX_EVENT_TEXT_UNITS).map_err(|_| ())?;
        Ok(if len == 0 {
            Vec::new()
        } else {
            unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec()
        })
    }

    let data = unsafe {
        match raw.r#type as u32 {
            t if t == kNoteOnEvent as u32 => {
                let value = raw.__field0.noteOn;
                PluginEventData::NoteOn {
                    channel: value.channel,
                    pitch: value.pitch,
                    tuning: value.tuning,
                    velocity: value.velocity,
                    length: value.length,
                    note_id: value.noteId,
                }
            }
            t if t == kNoteOffEvent as u32 => {
                let value = raw.__field0.noteOff;
                PluginEventData::NoteOff {
                    channel: value.channel,
                    pitch: value.pitch,
                    velocity: value.velocity,
                    note_id: value.noteId,
                    tuning: value.tuning,
                }
            }
            t if t == kDataEvent as u32 => {
                let value = raw.__field0.data;
                PluginEventData::Data {
                    data_type: value.r#type,
                    bytes: copy_bytes(value.bytes, usize::try_from(value.size).map_err(|_| ())?)?,
                }
            }
            t if t == kPolyPressureEvent as u32 => {
                let value = raw.__field0.polyPressure;
                PluginEventData::PolyPressure {
                    channel: value.channel,
                    pitch: value.pitch,
                    pressure: value.pressure,
                    note_id: value.noteId,
                }
            }
            t if t == kNoteExpressionValueEvent as u32 => {
                let value = raw.__field0.noteExpressionValue;
                PluginEventData::NoteExpressionValue {
                    type_id: value.typeId,
                    note_id: value.noteId,
                    value: value.value,
                }
            }
            t if t == kNoteExpressionTextEvent as u32 => {
                let value = raw.__field0.noteExpressionText;
                PluginEventData::NoteExpressionText {
                    type_id: value.typeId,
                    note_id: value.noteId,
                    text: copy_text(value.text, usize::try_from(value.textLen).map_err(|_| ())?)?,
                }
            }
            t if t == kNoteExpressionIntValueEvent as u32 => {
                let value = raw.__field0.noteExpressionIntValue;
                PluginEventData::NoteExpressionIntValue {
                    type_id: value.typeId,
                    note_id: value.noteId,
                    value: value.value,
                }
            }
            t if t == kChordEvent as u32 => {
                let value = raw.__field0.chord;
                PluginEventData::Chord {
                    root: value.root,
                    bass_note: value.bassNote,
                    mask: value.mask,
                    text: copy_text(value.text, usize::from(value.textLen))?,
                }
            }
            t if t == kScaleEvent as u32 => {
                let value = raw.__field0.scale;
                PluginEventData::Scale {
                    root: value.root,
                    mask: value.mask,
                    text: copy_text(value.text, usize::from(value.textLen))?,
                }
            }
            t if t == kLegacyMIDICCOutEvent as u32 => {
                let value = raw.__field0.midiCCOut;
                PluginEventData::LegacyMidiCcOut {
                    control_number: value.controlNumber,
                    channel: value.channel as i8,
                    value: value.value as u8,
                    value2: value.value2 as u8,
                }
            }
            _ => return Err(()),
        }
    };
    Ok(PluginEvent {
        bus_index: raw.busIndex,
        sample_offset: raw.sampleOffset,
        ppq_position: raw.ppqPosition,
        flags: raw.flags,
        data,
    })
}

pub fn create_event_list() -> ComWrapper<HostEventList> {
    ComWrapper::new(HostEventList::new())
}

// Parameter Changes implementation
/// Explicit, per-container budgets. Points are shared by all queues, never multiplied
/// by the queue count. Construction is control-side work, before the first block.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParameterQueueLimits {
    pub max_queues: usize,
    pub max_points: usize,
}

impl ParameterQueueLimits {
    pub const INPUT: Self = Self {
        max_queues: 8192,
        max_points: 8192,
    };
    pub const OUTPUT: Self = Self {
        max_queues: 4096,
        max_points: 4096,
    };
}

/// Small, nonallocating failures; presentation and logging belong outside callbacks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParameterStorageError {
    QueueCapacity,
    PointCapacity,
    InvalidArgument,
    InactiveQueue,
    Poisoned,
    InvalidState,
}

const NO_PARAMETER_NODE: usize = usize::MAX;

#[derive(Clone, Copy)]
struct ParameterPointNode {
    offset: i32,
    value: f64,
    next: usize,
}

#[derive(Clone, Copy)]
struct ParameterQueueSlot {
    id: u32,
    head: usize,
    tail: usize,
    count: usize,
    cache_start: usize,
}

impl ParameterQueueSlot {
    fn empty(id: u32) -> Self {
        Self {
            id,
            head: NO_PARAMETER_NODE,
            tail: NO_PARAMETER_NODE,
            count: 0,
            cache_start: NO_PARAMETER_NODE,
        }
    }
}

/// Only plain metadata/points live here. In particular, this arena must not own any
/// queue wrappers: a legally AddRef-retained queue owns the arena without a cycle.
struct ParameterArena {
    queues: Box<[ParameterQueueSlot]>,
    points: Box<[ParameterPointNode]>,
    // One shared ordinal index, not one max-points array per queue.
    ordinal_index: Box<[usize]>,
    // Ordered registered slots with at least one accepted point. Empty queues stay
    // registered/public, but never participate in ordinal-offset maintenance.
    populated_slots: Box<[usize]>,
    used_populated: usize,
    cache_dirty: bool,
    used_queues: usize,
    used_points: usize,
    failure: Option<ParameterStorageError>,
}

impl ParameterArena {
    fn fail(&mut self, error: ParameterStorageError) -> ParameterStorageError {
        if self.failure.is_none() {
            self.failure = Some(error);
        }
        error
    }

    fn find_queue(&self, id: u32) -> Option<usize> {
        self.queues[..self.used_queues]
            .iter()
            .position(|queue| queue.id == id)
    }

    // Call only after checking queue capacity and, for host admission, point capacity.
    fn activate_queue(&mut self, id: u32) -> usize {
        let slot = self.used_queues;
        self.queues[slot] = ParameterQueueSlot::empty(id);
        self.used_queues += 1;
        // Empty queues have no ordinal slice; their cache_start is never read.
        // Keep a dirty cache dirty without disturbing an already valid index.
        slot
    }

    fn validate_counts(&mut self) -> Result<(), ParameterStorageError> {
        if self.used_queues > self.queues.len()
            || self.used_points > self.points.len()
            || self.used_points > self.ordinal_index.len()
            || self.used_populated > self.populated_slots.len()
            || self.used_populated > self.used_queues
            || self.used_populated > self.used_points
        {
            return Err(self.fail(ParameterStorageError::InvalidState));
        }
        Ok(())
    }

    fn cache_range(
        &mut self,
        queue: ParameterQueueSlot,
    ) -> Result<(usize, usize), ParameterStorageError> {
        let Some(end) = queue.cache_start.checked_add(queue.count) else {
            return Err(self.fail(ParameterStorageError::InvalidState));
        };
        if end > self.used_points || end > self.ordinal_index.len() {
            return Err(self.fail(ParameterStorageError::InvalidState));
        }
        Ok((queue.cache_start, end))
    }

    fn checked_node(&mut self, node: usize) -> Result<ParameterPointNode, ParameterStorageError> {
        if node < self.used_points {
            if let Some(point) = self.points.get(node) {
                return Ok(*point);
            }
        }
        Err(self.fail(ParameterStorageError::InvalidState))
    }

    fn cached_node(&mut self, position: usize) -> Result<usize, ParameterStorageError> {
        if position < self.used_points {
            if let Some(&node) = self.ordinal_index.get(position) {
                if node < self.used_points && node < self.points.len() {
                    return Ok(node);
                }
            }
        }
        Err(self.fail(ParameterStorageError::InvalidState))
    }

    fn populated_slot(&mut self, position: usize) -> Result<usize, ParameterStorageError> {
        if position < self.used_populated {
            if let Some(&slot) = self.populated_slots.get(position) {
                if slot < self.used_queues {
                    if let Some(queue) = self.queues.get(slot) {
                        if queue.count != 0 {
                            return Ok(slot);
                        }
                    }
                }
            }
        }
        Err(self.fail(ParameterStorageError::InvalidState))
    }

    fn populated_position(&mut self, slot: usize) -> Result<(usize, bool), ParameterStorageError> {
        let mut low = 0;
        let mut high = self.used_populated;
        while low < high {
            let middle = low + (high - low) / 2;
            let candidate = self.populated_slot(middle)?;
            if candidate < slot {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        if low != 0 && self.populated_slot(low - 1)? >= slot {
            return Err(self.fail(ParameterStorageError::InvalidState));
        }
        let found = if low < self.used_populated {
            let next = self.populated_slot(low)?;
            if next < slot {
                return Err(self.fail(ParameterStorageError::InvalidState));
            }
            next == slot
        } else {
            false
        };
        Ok((low, found))
    }

    fn insert_point(
        &mut self,
        slot: usize,
        offset: i32,
        value: f64,
    ) -> Result<usize, ParameterStorageError> {
        self.validate_counts()?;
        if slot >= self.used_queues {
            return Err(self.fail(ParameterStorageError::InactiveQueue));
        }
        if self.used_points == self.points.len() {
            return Err(self.fail(ParameterStorageError::PointCapacity));
        }
        if self.used_points == self.ordinal_index.len() {
            return Err(self.fail(ParameterStorageError::InvalidState));
        }
        let mut queue = self.queues[slot];
        if queue.count > self.used_points {
            return Err(self.fail(ParameterStorageError::InvalidState));
        }
        let (populated_position, populated) = self.populated_position(slot)?;
        if populated != (queue.count != 0) {
            return Err(self.fail(ParameterStorageError::InvalidState));
        }
        if !populated && self.used_populated == self.populated_slots.len() {
            return Err(self.fail(ParameterStorageError::InvalidState));
        }
        if !self.cache_dirty {
            if populated {
                self.cache_range(queue)?;
            } else {
                // An empty queue's previous offset is deliberately unused. Derive
                // its first slice from the next populated queue or the arena end.
                queue.cache_start = if populated_position < self.used_populated {
                    let next = self.populated_slot(populated_position)?;
                    self.cache_range(self.queues[next])?.0
                } else {
                    self.used_points
                };
            }
        }
        let tail = if queue.count == 0 {
            if queue.head != NO_PARAMETER_NODE || queue.tail != NO_PARAMETER_NODE {
                return Err(self.fail(ParameterStorageError::InvalidState));
            }
            None
        } else {
            self.checked_node(queue.head)?;
            Some(self.checked_node(queue.tail)?)
        };
        let (previous, next, index) = if let Some(tail) = tail {
            if tail.next != NO_PARAMETER_NODE {
                return Err(self.fail(ParameterStorageError::InvalidState));
            }
            if tail.offset <= offset {
                // Dense, already ordered automation has constant-time rank lookup.
                (queue.tail, NO_PARAMETER_NODE, queue.count)
            } else if !self.cache_dirty {
                // Upper bound on the valid ordinal slice: equal-offset arrivals stay
                // before this new point. Search compares offsets only, never values.
                let mut low = 0;
                let mut high = queue.count;
                while low < high {
                    let middle = low + (high - low) / 2;
                    let node = self.cached_node(queue.cache_start + middle)?;
                    if self.checked_node(node)?.offset <= offset {
                        low = middle + 1;
                    } else {
                        high = middle;
                    }
                }
                let previous = if low == 0 {
                    NO_PARAMETER_NODE
                } else {
                    self.cached_node(queue.cache_start + low - 1)?
                };
                // The tail fast path excludes low == count when the index is sound.
                if low == queue.count {
                    return Err(self.fail(ParameterStorageError::InvalidState));
                }
                let next = self.cached_node(queue.cache_start + low)?;
                (previous, next, low)
            } else {
                let mut previous = NO_PARAMETER_NODE;
                let mut next = queue.head;
                let mut index = 0;
                while next != NO_PARAMETER_NODE {
                    if index == queue.count {
                        return Err(self.fail(ParameterStorageError::InvalidState));
                    }
                    let point = self.checked_node(next)?;
                    if point.offset > offset {
                        break;
                    }
                    previous = next;
                    next = point.next;
                    index += 1;
                }
                (previous, next, index)
            }
        } else {
            (NO_PARAMETER_NODE, NO_PARAMETER_NODE, 0)
        };
        if previous != NO_PARAMETER_NODE {
            self.checked_node(previous)?;
        }
        if next != NO_PARAMETER_NODE {
            self.checked_node(next)?;
        }

        let node = self.used_points;
        // Validate all index movement and later metadata before changing either
        // linked storage or the ordinal index. No full index scan is needed.
        let position = if !self.cache_dirty {
            let Some(position) = queue.cache_start.checked_add(index) else {
                return Err(self.fail(ParameterStorageError::InvalidState));
            };
            let valid_move = position
                .checked_add(1)
                .and_then(|destination| {
                    node.checked_sub(position).and_then(|length| {
                        destination
                            .checked_add(length)
                            .map(|end| end <= self.ordinal_index.len())
                    })
                })
                .unwrap_or(false);
            if !valid_move || self.ordinal_index.get(position..node).is_none() {
                return Err(self.fail(ParameterStorageError::InvalidState));
            }
            Some(position)
        } else {
            None
        };

        let later_start = populated_position + usize::from(populated);
        let mut previous_slot = slot;
        let mut expected_start = if !self.cache_dirty {
            queue.cache_start + queue.count // The checked target range bounds this sum.
        } else {
            0
        };
        for later in later_start..self.used_populated {
            let later_slot = self.populated_slot(later)?;
            if later_slot <= previous_slot {
                return Err(self.fail(ParameterStorageError::InvalidState));
            }
            previous_slot = later_slot;
            if !self.cache_dirty {
                let later_queue = self.queues[later_slot];
                let (start, end) = self.cache_range(later_queue)?;
                if start != expected_start || start.checked_add(1).is_none() {
                    return Err(self.fail(ParameterStorageError::InvalidState));
                }
                expected_start = end;
            }
        }
        if !self.cache_dirty && expected_start != self.used_points {
            return Err(self.fail(ParameterStorageError::InvalidState));
        }
        if !populated {
            let valid_move = populated_position
                .checked_add(1)
                .and_then(|destination| {
                    self.used_populated
                        .checked_sub(populated_position)
                        .and_then(|length| {
                            destination
                                .checked_add(length)
                                .map(|end| end <= self.populated_slots.len())
                        })
                })
                .unwrap_or(false);
            if !valid_move
                || self
                    .populated_slots
                    .get(populated_position..self.used_populated)
                    .is_none()
            {
                return Err(self.fail(ParameterStorageError::InvalidState));
            }
        }
        let cache_start = queue.cache_start;

        self.points[node] = ParameterPointNode {
            offset,
            value,
            next,
        };
        self.used_points += 1;
        if previous == NO_PARAMETER_NODE {
            self.queues[slot].head = node;
        } else {
            self.points[previous].next = node;
        }
        let queue = &mut self.queues[slot];
        if next == NO_PARAMETER_NODE {
            queue.tail = node;
        }
        queue.count += 1;
        if !populated && !self.cache_dirty {
            queue.cache_start = cache_start;
        }
        if let Some(position) = position {
            // Cost is O(total point suffix + later populated queues); empty queues
            // are never walked or read here. Final populated tail appends move none.
            self.ordinal_index.copy_within(position..node, position + 1);
            self.ordinal_index[position] = node;
            for later in later_start..self.used_populated {
                self.queues[self.populated_slots[later]].cache_start += 1;
            }
        }
        if !populated {
            self.populated_slots.copy_within(
                populated_position..self.used_populated,
                populated_position + 1,
            );
            self.populated_slots[populated_position] = slot;
            self.used_populated += 1;
        }
        // A dirty index stays dirty; its next valid read still rebuilds once.
        Ok(index)
    }

    fn rebuild_index(&mut self) -> Result<(), ParameterStorageError> {
        // Validate the bounded linked chains before changing any cached positions.
        let mut total = 0_usize;
        let mut previous_slot = None;
        for populated in 0..self.used_populated {
            let slot = self.populated_slot(populated)?;
            if previous_slot.is_some_and(|previous| previous >= slot) {
                return Err(self.fail(ParameterStorageError::InvalidState));
            }
            previous_slot = Some(slot);
            let queue = self.queues[slot];
            let Some(end) = total.checked_add(queue.count) else {
                return Err(self.fail(ParameterStorageError::InvalidState));
            };
            if end > self.used_points || end > self.ordinal_index.len() {
                return Err(self.fail(ParameterStorageError::InvalidState));
            }
            let mut node = queue.head;
            let mut tail = NO_PARAMETER_NODE;
            for _ in 0..queue.count {
                let point = self.checked_node(node)?;
                tail = node;
                node = point.next;
            }
            if node != NO_PARAMETER_NODE || tail != queue.tail {
                return Err(self.fail(ParameterStorageError::InvalidState));
            }
            total = end;
        }
        if total != self.used_points {
            return Err(self.fail(ParameterStorageError::InvalidState));
        }
        let mut position = 0;
        for populated in 0..self.used_populated {
            let queue = &mut self.queues[self.populated_slots[populated]];
            queue.cache_start = position;
            let mut node = queue.head;
            for _ in 0..queue.count {
                self.ordinal_index[position] = node;
                position += 1;
                node = self.points[node].next;
            }
        }
        self.cache_dirty = false;
        Ok(())
    }

    fn point(&mut self, slot: usize, index: i32) -> Result<(i32, f64), ParameterStorageError> {
        self.validate_counts()?;
        if slot >= self.used_queues {
            return Err(ParameterStorageError::InactiveQueue);
        }
        let queue = self.queues[slot];
        if index < 0 || index as usize >= queue.count {
            return Err(ParameterStorageError::InvalidArgument);
        }
        if self.cache_dirty {
            self.rebuild_index()?;
        }
        // Ordinary valid reads check only this queue's range and selected node.
        // They do not scan the array. A malformed cache fails closed, not by panic.
        let (start, _) = self.cache_range(self.queues[slot])?;
        let node = self.cached_node(start + index as usize)?;
        let point = self.checked_node(node)?;
        Ok((point.offset, point.value))
    }
}

fn lock_parameter_arena(
    arena: &Mutex<ParameterArena>,
) -> Result<std::sync::MutexGuard<'_, ParameterArena>, ParameterStorageError> {
    match arena.lock() {
        Ok(guard) => Ok(guard),
        Err(poison) => {
            // Preserve the first failure, but never use potentially partial state.
            // This guard is acquired once; no nested lock, panic, allocation or log.
            poison.into_inner().fail(ParameterStorageError::Poisoned);
            Err(ParameterStorageError::Poisoned)
        }
    }
}

pub struct ParameterChanges {
    // The owning wrappers never move or change after construction. SDK queue-return
    // methods borrow their pointer, without cloning/dropping a COM reference.
    queues: Box<[ComWrapper<ParameterValueQueue>]>,
    arena: Arc<Mutex<ParameterArena>>,
}

impl Default for ParameterChanges {
    fn default() -> Self {
        Self::new(ParameterQueueLimits::OUTPUT)
    }
}

impl ParameterChanges {
    pub fn new(limits: ParameterQueueLimits) -> Self {
        assert!(limits.max_queues <= i32::MAX as usize);
        assert!(limits.max_points <= i32::MAX as usize);
        let arena = Arc::new(Mutex::new(ParameterArena {
            queues: vec![ParameterQueueSlot::empty(u32::MAX); limits.max_queues].into_boxed_slice(),
            points: vec![
                ParameterPointNode {
                    offset: 0,
                    value: 0.0,
                    next: NO_PARAMETER_NODE
                };
                limits.max_points
            ]
            .into_boxed_slice(),
            ordinal_index: vec![0; limits.max_points].into_boxed_slice(),
            populated_slots: vec![0; limits.max_queues].into_boxed_slice(),
            used_populated: 0,
            cache_dirty: false,
            used_queues: 0,
            used_points: 0,
            failure: None,
        }));
        let queues = (0..limits.max_queues)
            .map(|slot| {
                ComWrapper::new(ParameterValueQueue {
                    slot,
                    arena: Arc::clone(&arena),
                })
            })
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self { queues, arena }
    }

    /// Admit one host point atomically. A rejected new id never activates an empty
    /// queue. No queue creation, allocator traffic or COM reference change occurs.
    pub fn try_enqueue(
        &self,
        id: u32,
        sample_offset: i32,
        value: f64,
    ) -> Result<(), ParameterStorageError> {
        let mut arena = lock_parameter_arena(&self.arena)?;
        arena.validate_counts()?;
        if arena.used_points == arena.points.len() {
            return Err(arena.fail(ParameterStorageError::PointCapacity));
        }
        if let Some(slot) = arena.find_queue(id) {
            return arena.insert_point(slot, sample_offset, value).map(|_| ());
        }
        if arena.used_queues == arena.queues.len() {
            return Err(arena.fail(ParameterStorageError::QueueCapacity));
        }
        let slot = arena.used_queues;
        let recycled = arena.queues[slot];
        arena.activate_queue(id);
        match arena.insert_point(slot, sample_offset, value) {
            Ok(_) => Ok(()),
            Err(error) => {
                // Point/list/index insertion performs all checks before mutation.
                // Restore the exact inactive metadata too, not just its active count.
                arena.used_queues = slot;
                arena.queues[slot] = recycled;
                Err(error)
            }
        }
    }

    /// Logical reset only. Retained queues become inactive until their fixed slot is
    /// reused. Accepted points, wrappers and buffers are neither dropped nor freed.
    /// A sticky failure is deliberately never cleared by reset.
    pub fn clear_all(&self) -> Result<(), ParameterStorageError> {
        let mut arena = lock_parameter_arena(&self.arena)?;
        arena.used_queues = 0;
        arena.used_points = 0;
        arena.used_populated = 0;
        arena.cache_dirty = false;
        Ok(())
    }

    pub fn failure(&self) -> Option<ParameterStorageError> {
        match self.arena.lock() {
            Ok(arena) => arena.failure,
            Err(poison) => {
                let mut arena = poison.into_inner();
                arena.fail(ParameterStorageError::Poisoned);
                arena.failure
            }
        }
    }

    /// Callback runs under the one shared arena lock and must not re-enter this
    /// container or allocate/block. Queues are visited in first-seen id order, then
    /// points in ascending offset and equal-offset arrival order.
    pub fn for_each_active_point(
        &self,
        mut f: impl FnMut(u32, i32, f64),
    ) -> Result<(), ParameterStorageError> {
        let mut arena = lock_parameter_arena(&self.arena)?;
        arena.validate_counts()?;
        let mut visited = 0;
        for slot in 0..arena.used_queues {
            let queue = arena.queues[slot];
            if queue.count > arena.used_points {
                return Err(arena.fail(ParameterStorageError::InvalidState));
            }
            let mut node = queue.head;
            let mut tail = NO_PARAMETER_NODE;
            for _ in 0..queue.count {
                if visited == arena.used_points {
                    return Err(arena.fail(ParameterStorageError::InvalidState));
                }
                let point = arena.checked_node(node)?;
                f(queue.id, point.offset, point.value);
                tail = node;
                node = point.next;
                visited += 1;
            }
            if node != NO_PARAMETER_NODE || tail != queue.tail {
                return Err(arena.fail(ParameterStorageError::InvalidState));
            }
        }
        if visited != arena.used_points {
            return Err(arena.fail(ParameterStorageError::InvalidState));
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn poison_for_test(&self) {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = self.arena.lock().unwrap();
            panic!("controlled parameter arena poison");
        }));
    }
}

impl Class for ParameterChanges {
    type Interfaces = (IParameterChanges,);
}

impl IParameterChangesTrait for ParameterChanges {
    unsafe fn getParameterCount(&self) -> i32 {
        let Ok(mut arena) = lock_parameter_arena(&self.arena) else {
            return 0;
        };
        if arena.validate_counts().is_err() {
            return 0;
        }
        arena.used_queues as i32
    }

    unsafe fn getParameterData(&self, index: i32) -> *mut IParamValueQueue {
        let Ok(mut arena) = lock_parameter_arena(&self.arena) else {
            return ptr::null_mut();
        };
        if arena.validate_counts().is_err() {
            return ptr::null_mut();
        }
        if index < 0 || index as usize >= arena.used_queues {
            return ptr::null_mut();
        }
        match self
            .queues
            .get(index as usize)
            .and_then(|queue| queue.as_com_ref::<IParamValueQueue>())
        {
            Some(queue) => queue.as_ptr(),
            None => {
                arena.fail(ParameterStorageError::InvalidState);
                ptr::null_mut()
            }
        }
    }

    unsafe fn addParameterData(&self, id: *const u32, index: *mut i32) -> *mut IParamValueQueue {
        if !index.is_null() {
            *index = -1;
        }
        let Ok(mut arena) = lock_parameter_arena(&self.arena) else {
            return ptr::null_mut();
        };
        if id.is_null() {
            // Invalid writes imply an edit could not be represented. Unlike a read
            // probe, this latches loss even if the plugin ignores the null return.
            arena.fail(ParameterStorageError::InvalidArgument);
            return ptr::null_mut();
        }
        if arena.validate_counts().is_err() {
            return ptr::null_mut();
        }
        let id = *id;
        let existing = arena.find_queue(id);
        let slot = existing.unwrap_or(arena.used_queues);
        if slot == arena.queues.len() {
            arena.fail(ParameterStorageError::QueueCapacity);
            return ptr::null_mut();
        }
        let Some(queue) = self.queues[slot].as_com_ref::<IParamValueQueue>() else {
            arena.fail(ParameterStorageError::InvalidState);
            return ptr::null_mut();
        };
        if existing.is_none() {
            arena.activate_queue(id);
        }
        if !index.is_null() {
            *index = slot as i32;
        }
        // Same borrowed return as the official SDK ParameterChanges implementation.
        // A caller retaining this beyond the container must explicitly AddRef.
        queue.as_ptr()
    }
}

pub struct ParameterValueQueue {
    slot: usize,
    arena: Arc<Mutex<ParameterArena>>,
}

impl Class for ParameterValueQueue {
    type Interfaces = (IParamValueQueue,);
}

impl IParamValueQueueTrait for ParameterValueQueue {
    unsafe fn getParameterId(&self) -> u32 {
        lock_parameter_arena(&self.arena)
            .map(|arena| arena.queues[self.slot].id)
            .unwrap_or(u32::MAX)
    }

    unsafe fn getPointCount(&self) -> i32 {
        lock_parameter_arena(&self.arena)
            .map(|arena| {
                if self.slot < arena.used_queues {
                    arena.queues[self.slot].count as i32
                } else {
                    0
                }
            })
            .unwrap_or(0)
    }

    unsafe fn getPoint(&self, index: i32, sample_offset: *mut i32, value: *mut f64) -> i32 {
        let Ok(mut arena) = lock_parameter_arena(&self.arena) else {
            return kResultFalse;
        };
        match arena.point(self.slot, index) {
            Ok((offset, point_value)) => {
                if !sample_offset.is_null() {
                    *sample_offset = offset;
                }
                if !value.is_null() {
                    *value = point_value;
                }
                kResultOk
            }
            Err(_) => kResultFalse,
        }
    }

    unsafe fn addPoint(&self, sample_offset: i32, value: f64, index: *mut i32) -> i32 {
        if !index.is_null() {
            *index = -1;
        }
        let Ok(mut arena) = lock_parameter_arena(&self.arena) else {
            return kResultFalse;
        };
        match arena.insert_point(self.slot, sample_offset, value) {
            Ok(position) => {
                if !index.is_null() {
                    *index = position as i32;
                }
                kResultOk
            }
            Err(_) => kResultFalse,
        }
    }
}

#[cfg(test)]
mod host_attr_tests {
    use super::*;

    #[test]
    fn attribute_list_round_trips_each_type() {
        let list = HostAttributeList::new();
        list.put("i".into(), AttrValue::Int(42));
        list.put("f".into(), AttrValue::Float(1.5));
        list.put("s".into(), AttrValue::Str(vec![72, 105])); // "Hi"
        list.put("b".into(), AttrValue::Bin(vec![1, 2, 3]));

        assert_eq!(list.get_value("i"), Some(AttrValue::Int(42)));
        assert_eq!(list.get_value("f"), Some(AttrValue::Float(1.5)));
        assert_eq!(list.get_value("s"), Some(AttrValue::Str(vec![72, 105])));
        assert_eq!(list.get_value("b"), Some(AttrValue::Bin(vec![1, 2, 3])));
        assert_eq!(list.get_value("missing"), None);
    }
}

#[cfg(test)]
mod component_handler_tests {
    use super::*;
    use crate::plugin::{ContextMenuItem, HostNotification, ParameterEdit, ParameterEditKind};

    #[test]
    fn native_dirty_revision_survives_all_feedback_drains() {
        let (handler, mut _native_edits) = ComponentHandler::new(Arc::new(Mutex::new(Vec::new())));
        assert_eq!(handler.native_dirty_revision().unwrap(), 0);
        unsafe {
            handler.beginEdit(7);
            handler.endEdit(7);
            handler.setDirty(0);
            handler.restartComponent(RestartFlags_::kLatencyChanged);
        }
        assert_eq!(handler.native_dirty_revision().unwrap(), 0);
        unsafe {
            handler.performEdit(7, 0.75);
            handler.setDirty(1);
            handler.restartComponent(
                RestartFlags_::kParamValuesChanged | RestartFlags_::kReloadComponent,
            );
            handler.restartComponent(RestartFlags_::kReloadComponent);
        }
        assert_eq!(handler.native_dirty_revision().unwrap(), 4);
        assert!(handler.native_state_capture_revision().is_err());
        handler.take_parameter_edits();
        handler.take_host_notifications();
        handler.take_restart_flags();
        assert_eq!(handler.native_dirty_revision().unwrap(), 4);
        // Polling display values cannot acknowledge native processor delivery.
        handler.parameter_changes.lock().unwrap().clear();
        assert!(handler.native_state_capture_revision().is_err());
        _native_edits.stage(|_| true).unwrap();
        assert!(handler.native_state_capture_revision().is_err());
        _native_edits.finish_process(true).unwrap();
        assert_eq!(handler.native_state_capture_revision().unwrap(), 4);
    }

    #[test]
    fn native_dirty_revision_survives_bounded_feedback_overflow() {
        let (handler, mut _native_edits) = ComponentHandler::new(Arc::new(Mutex::new(Vec::new())));
        unsafe {
            for _ in 0..(MAX_EDITOR_FEEDBACK + 3) {
                handler.performEdit(7, 0.5);
            }
            for _ in 0..MAX_HOST_NOTIFICATIONS {
                assert_eq!(handler.setDirty(0), kResultOk);
            }
            assert_eq!(handler.setDirty(1), kResultFalse);
        }
        assert_eq!(
            handler.native_dirty_revision().unwrap(),
            MAX_EDITOR_FEEDBACK as u64 + 4
        );
        assert_eq!(handler.take_parameter_edits().len(), MAX_EDITOR_FEEDBACK);
        assert_eq!(
            handler.take_host_notifications().len(),
            MAX_HOST_NOTIFICATIONS
        );
        handler.parameter_changes.lock().unwrap().clear();
        assert!(handler.native_state_capture_revision().is_err());
        assert_eq!(
            handler.native_dirty_revision().unwrap(),
            MAX_EDITOR_FEEDBACK as u64 + 4
        );
    }

    #[test]
    fn native_dirty_revision_exhaustion_never_wraps_to_clean() {
        let (handler, mut _native_edits) = ComponentHandler::new(Arc::new(Mutex::new(Vec::new())));
        handler.native_edits.test_set_dirty_revision(u64::MAX - 1);
        assert_eq!(handler.native_dirty_revision().unwrap(), u64::MAX - 1);
        unsafe {
            handler.setDirty(1);
            handler.setDirty(1);
        }
        assert_eq!(handler.native_edits.snapshot().dirty, u64::MAX);
        assert!(handler.native_dirty_revision().is_err());
        assert!(handler.native_state_capture_revision().is_err());
    }

    #[test]
    fn display_feedback_poison_does_not_steal_processor_input() {
        let changes = Arc::new(Mutex::new(Vec::new()));
        let (handler, mut _native_edits) = ComponentHandler::new(changes.clone());
        let _ = std::panic::catch_unwind(|| {
            let _guard = changes.lock().unwrap();
            panic!("simulate a poisoned native feedback queue");
        });
        unsafe {
            handler.performEdit(7, 0.5);
        }
        assert_eq!(handler.native_dirty_revision().unwrap(), 1);
        assert!(handler.native_state_capture_revision().is_err());
        let mut received = Vec::new();
        _native_edits
            .stage(|edit| {
                received.push((edit.id, edit.value));
                true
            })
            .unwrap();
        _native_edits.finish_process(true).unwrap();
        assert_eq!(received, [(7, 0.5)]);
        assert_eq!(handler.native_state_capture_revision().unwrap(), 1);
    }

    struct TestContextMenuTarget {
        calls: Arc<Mutex<Vec<i32>>>,
    }

    impl Class for TestContextMenuTarget {
        type Interfaces = (IContextMenuTarget,);
    }

    impl IContextMenuTargetTrait for TestContextMenuTarget {
        unsafe fn executeMenuItem(&self, tag: i32) -> tresult {
            self.calls
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .push(tag);
            kResultOk
        }
    }

    fn context_menu_item(name: &str, tag: i32, flags: i32) -> IContextMenuItem {
        let mut item = IContextMenuItem {
            name: [0; 128],
            tag,
            flags,
        };
        for (destination, source) in item.name.iter_mut().zip(name.encode_utf16()) {
            *destination = source;
        }
        item
    }

    #[test]
    fn captures_begin_perform_end_in_order_and_drains() {
        let (handler, mut _native_edits) = ComponentHandler::new(Arc::new(Mutex::new(Vec::new())));

        // Drive a full gesture: mouse-down, two drag values, mouse-up.
        unsafe {
            handler.beginEdit(5);
            handler.performEdit(5, 0.25);
            handler.performEdit(5, 0.5);
            handler.endEdit(5);
        }

        let edits = handler.take_parameter_edits();
        assert_eq!(
            edits,
            vec![
                ParameterEdit {
                    id: 5,
                    kind: ParameterEditKind::BeginGesture,
                    value: None,
                },
                ParameterEdit {
                    id: 5,
                    kind: ParameterEditKind::ValueChange,
                    value: Some(0.25),
                },
                ParameterEdit {
                    id: 5,
                    kind: ParameterEditKind::ValueChange,
                    value: Some(0.5),
                },
                ParameterEdit {
                    id: 5,
                    kind: ParameterEditKind::EndGesture,
                    value: None,
                },
            ]
        );

        // The drain empties the buffer; the value-change sink still mirrors the performEdits.
        assert!(handler.take_parameter_edits().is_empty());
        assert_eq!(
            *handler.parameter_changes.lock().unwrap(),
            vec![(5, 0.25), (5, 0.5)]
        );
    }

    /// Both editor-feedback buffers are drained only by an optional host poll, so a host that
    /// never polls must not be able to grow them without bound — dragging a knob emits one
    /// `performEdit` (and one gesture event) per UI frame, for as long as the editor is open.
    #[test]
    fn editor_feedback_is_capped_when_the_host_never_polls() {
        let (handler, mut _native_edits) = ComponentHandler::new(Arc::new(Mutex::new(Vec::new())));

        // Simulate a very long drag: far more edits than the cap, never polled.
        unsafe {
            for i in 0..(MAX_EDITOR_FEEDBACK * 2) {
                handler.performEdit(7, (i % 100) as f64 / 100.0);
            }
        }

        assert_eq!(
            handler.parameter_changes.lock().unwrap().len(),
            MAX_EDITOR_FEEDBACK,
            "the value-change sink must stop at the cap, not grow with the drag"
        );
        let edits = handler.take_parameter_edits();
        assert_eq!(
            edits.len(),
            MAX_EDITOR_FEEDBACK,
            "the gesture log must stop at the cap too"
        );

        // Draining keeps the buffer's capacity, so the next gesture doesn't reallocate on the
        // COM callback path.
        assert!(handler.take_parameter_edits().is_empty());
        assert!(handler.edits.lock().unwrap().capacity() >= MAX_EDITOR_FEEDBACK);
    }

    #[test]
    fn handler2_requests_are_ordered_and_report_backpressure() {
        let (handler, mut _native_edits) = ComponentHandler::new(Arc::new(Mutex::new(Vec::new())));
        unsafe {
            assert_eq!(handler.setDirty(1), kResultOk);
            assert_eq!(handler.requestOpenEditor(c"editor".as_ptr()), kResultOk);
            assert_eq!(handler.startGroupEdit(), kResultOk);
            assert_eq!(handler.finishGroupEdit(), kResultOk);
        }
        let notifications = handler.take_host_notifications();
        assert_eq!(
            notifications,
            vec![
                HostNotification::DirtyChanged(true),
                HostNotification::OpenEditorRequested {
                    name: Some("editor".to_string())
                },
                HostNotification::GroupEditStarted,
                HostNotification::GroupEditFinished,
            ]
        );

        unsafe {
            for _ in 0..MAX_HOST_NOTIFICATIONS {
                assert_eq!(handler.setDirty(0), kResultOk);
            }
            assert_eq!(handler.setDirty(1), kResultFalse);
        }
        assert_eq!(
            handler.take_host_notifications().len(),
            MAX_HOST_NOTIFICATIONS
        );
    }

    #[test]
    fn handler3_context_menu_preserves_items_and_executes_plugin_target() {
        let (handler, mut _native_edits) = ComponentHandler::new(Arc::new(Mutex::new(Vec::new())));
        let handler_wrapper = ComWrapper::new(handler);
        assert!(
            handler_wrapper.as_com_ref::<IComponentHandler3>().is_some(),
            "controllers must be able to query IComponentHandler3"
        );

        let calls = Arc::new(Mutex::new(Vec::new()));
        let target = ComWrapper::new(TestContextMenuTarget {
            calls: calls.clone(),
        });
        let target = target.as_com_ref::<IContextMenuTarget>().unwrap();
        let parameter_id = 42;
        let menu = unsafe {
            handler_wrapper.createContextMenu(
                std::ptr::NonNull::<IPlugView>::dangling().as_ptr(),
                &parameter_id,
            )
        };
        let menu = unsafe { ComPtr::<IContextMenu>::from_raw(menu) }.unwrap();
        let reset = context_menu_item("Reset", 17, 0);
        let separator = context_menu_item("", 0, IContextMenuItem_::Flags_::kIsSeparator as i32);
        unsafe {
            assert_eq!(menu.addItem(&reset, target.as_ptr()), kResultOk);
            assert_eq!(menu.addItem(&separator, ptr::null_mut()), kResultOk);
            assert_eq!(menu.getItemCount(), 2);
            let mut returned = context_menu_item("", 0, 0);
            let mut returned_target = ptr::null_mut();
            assert_eq!(
                menu.getItem(0, &mut returned, &mut returned_target),
                kResultOk
            );
            assert_eq!(
                crate::internal::utils::vst_string_to_string(&returned.name),
                "Reset"
            );
            assert_eq!(returned.tag, 17);
            assert!(!returned_target.is_null());
            drop(ComPtr::<IContextMenuTarget>::from_raw(returned_target));
            assert_eq!(menu.popup(11, 23), kResultOk);
        }
        drop(menu);

        let notifications = handler_wrapper.take_host_notifications();
        let (menu_id, items) = match notifications.as_slice() {
            [HostNotification::ContextMenuRequested {
                menu_id,
                parameter_id: Some(42),
                x: 11,
                y: 23,
                items,
            }] => (*menu_id, items),
            other => panic!("unexpected context-menu notification: {other:?}"),
        };
        assert_eq!(
            items,
            &[
                ContextMenuItem {
                    item_id: 0,
                    name: "Reset".to_string(),
                    tag: 17,
                    flags: 0,
                },
                ContextMenuItem {
                    item_id: 1,
                    name: String::new(),
                    tag: 0,
                    flags: IContextMenuItem_::Flags_::kIsSeparator as i32,
                },
            ]
        );
        assert!(items[1].is_separator());

        handler_wrapper
            .execute_context_menu_item(menu_id, 0)
            .unwrap();
        assert_eq!(*calls.lock().unwrap(), vec![17]);
        assert!(
            handler_wrapper
                .execute_context_menu_item(menu_id, 0)
                .is_err(),
            "a popup can only be completed once"
        );
    }

    #[test]
    fn handler3_rejects_invalid_views_and_releases_dismissed_targets() {
        let (handler, mut _native_edits) = ComponentHandler::new(Arc::new(Mutex::new(Vec::new())));
        assert!(unsafe { handler.createContextMenu(ptr::null_mut(), ptr::null()) }.is_null());

        let menu = unsafe {
            handler.createContextMenu(
                std::ptr::NonNull::<IPlugView>::dangling().as_ptr(),
                ptr::null(),
            )
        };
        let menu = unsafe { ComPtr::<IContextMenu>::from_raw(menu) }.unwrap();
        let separator = context_menu_item("", 0, IContextMenuItem_::Flags_::kIsSeparator as i32);
        unsafe {
            assert_eq!(menu.addItem(&separator, ptr::null_mut()), kResultOk);
            assert_eq!(menu.popup(0, 0), kResultOk);
        }
        let notification = handler.take_host_notifications().pop().unwrap();
        let HostNotification::ContextMenuRequested { menu_id, .. } = notification else {
            panic!("expected context-menu notification");
        };
        handler.dismiss_context_menu(menu_id).unwrap();
        assert!(handler.dismiss_context_menu(menu_id).is_err());
    }

    #[test]
    fn unit_handler_requests_are_ordered_and_preserve_whole_list_changes() {
        let (handler, mut _native_edits) = ComponentHandler::new(Arc::new(Mutex::new(Vec::new())));
        unsafe {
            assert_eq!(handler.notifyUnitSelection(7), kResultOk);
            assert_eq!(handler.notifyProgramListChange(11, 3), kResultOk);
            assert_eq!(handler.notifyProgramListChange(11, -1), kResultOk);
            assert_eq!(handler.notifyUnitByBusChange(), kResultOk);
        }
        let notifications = handler.take_host_notifications();
        assert_eq!(
            notifications,
            vec![
                HostNotification::UnitSelectionChanged { unit_id: 7 },
                HostNotification::ProgramListChanged {
                    list_id: 11,
                    program_index: Some(3),
                },
                HostNotification::ProgramListChanged {
                    list_id: 11,
                    program_index: None,
                },
                HostNotification::UnitByBusChanged,
            ]
        );
        assert!(!notifications[0].invalidates_unit_cache());
        assert!(notifications[1..]
            .iter()
            .all(HostNotification::invalidates_unit_cache));
        assert!(handler.take_host_notifications().is_empty());
    }

    /// `restartComponent` is how a plugin tells the host something about it changed. Every flag
    /// used to be acknowledged and dropped; they must survive until the host drains them.
    #[test]
    fn restart_flags_accumulate_until_drained() {
        use vst3::Steinberg::Vst::RestartFlags_ as Flags;
        let (handler, mut _native_edits) = ComponentHandler::new(Arc::new(Mutex::new(Vec::new())));
        assert!(handler.take_restart_flags().is_empty());

        // Two separate restarts, e.g. a preset load followed by a mode switch.
        unsafe {
            handler.restartComponent(Flags::kParamValuesChanged | Flags::kParamTitlesChanged);
            handler.restartComponent(Flags::kLatencyChanged);
        }

        let flags = handler.take_restart_flags();
        assert!(flags.param_values_changed());
        assert!(flags.param_titles_changed());
        assert!(flags.latency_changed());
        assert!(!flags.io_changed());

        // Draining clears them, so the host doesn't act on the same request twice.
        assert!(handler.take_restart_flags().is_empty());

        // A plugin that spams restarts while nothing polls costs one word, not a queue.
        unsafe {
            for _ in 0..10_000 {
                handler.restartComponent(Flags::kIoChanged);
            }
        }
        let flags = handler.take_restart_flags();
        assert!(flags.io_changed());
        assert_eq!(flags.bits(), Flags::kIoChanged);
    }
}

#[cfg(test)]
mod host_application_tests {
    use super::*;

    #[test]
    fn interface_support_reports_plugin_side_interfaces_only() {
        let host = create_host_application()
            .to_com_ptr::<IPlugInterfaceSupport>()
            .unwrap();
        unsafe {
            let mut process_context = IProcessContextRequirements::IID;
            assert_eq!(
                host.isPlugInterfaceSupported(&mut process_context as *mut _ as *const TUID,),
                kResultTrue
            );
            let mut remap_param_id = IRemapParamID::IID;
            assert_eq!(
                host.isPlugInterfaceSupported(&mut remap_param_id as *mut _ as *const TUID),
                kResultTrue
            );

            let mut component_handler = IComponentHandler::IID;
            assert_eq!(
                host.isPlugInterfaceSupported(&mut component_handler as *mut _ as *const TUID,),
                kResultFalse
            );
            assert_eq!(
                host.isPlugInterfaceSupported(ptr::null_mut()),
                kInvalidArgument
            );
        }
    }

    #[test]
    fn create_instance_requires_matching_class_and_interface_ids() {
        let host = create_host_application()
            .to_com_ptr::<IHostApplication>()
            .unwrap();
        unsafe {
            let mut message_cid = IMessage::IID;
            let mut message_iid = IMessage::IID;
            let mut raw = ptr::null_mut();
            assert_eq!(
                host.createInstance(
                    &mut message_cid as *mut _ as *mut TUID,
                    &mut message_iid as *mut _ as *mut TUID,
                    &mut raw,
                ),
                kResultTrue
            );
            assert!(!raw.is_null());
            drop(ComPtr::<IMessage>::from_raw(raw.cast::<IMessage>()));

            let mut attributes_iid = IAttributeList::IID;
            raw = std::ptr::NonNull::<std::ffi::c_void>::dangling().as_ptr();
            assert_eq!(
                host.createInstance(
                    &mut message_cid as *mut _ as *mut TUID,
                    &mut attributes_iid as *mut _ as *mut TUID,
                    &mut raw,
                ),
                kNoInterface
            );
            assert!(raw.is_null());

            assert_eq!(
                host.createInstance(
                    ptr::null_mut(),
                    &mut message_iid as *mut _ as *mut TUID,
                    &mut raw,
                ),
                kInvalidArgument
            );
        }
    }

    #[test]
    fn progress_callbacks_are_bounded_ordered_and_polled() {
        use crate::plugin::{HostNotification, ProgressKind};

        let host = create_host_application();
        let progress = host.to_com_ptr::<IProgress>().unwrap();
        let description: Vec<u16> = "Loading samples\0".encode_utf16().collect();
        let mut id = 0;
        unsafe {
            assert_eq!(
                progress.start(
                    IProgress_::ProgressType_::AsyncStateRestoration,
                    description.as_ptr(),
                    &mut id,
                ),
                kResultOk
            );
            assert_ne!(id, 0);
            assert_eq!(progress.update(id, 0.5), kResultOk);
            assert_eq!(progress.finish(id), kResultOk);
            assert_eq!(progress.update(id, 0.75), kResultFalse);
            assert_eq!(progress.update(u64::MAX, 0.5), kResultFalse);
            assert_eq!(progress.update(id, f64::NAN), kInvalidArgument);
        }
        assert_eq!(
            host.take_progress_notifications(),
            vec![
                HostNotification::ProgressStarted {
                    id,
                    kind: ProgressKind::AsyncStateRestoration,
                    description: Some("Loading samples".to_string()),
                },
                HostNotification::ProgressUpdated {
                    id,
                    value: crate::plugin::ProgressValue::new(0.5).unwrap(),
                },
                HostNotification::ProgressFinished { id },
            ]
        );
    }

    #[test]
    fn progress_reports_backpressure_instead_of_false_success() {
        let host = create_host_application();
        let progress = host.to_com_ptr::<IProgress>().unwrap();
        let mut id = 0;
        unsafe {
            assert_eq!(
                progress.start(
                    IProgress_::ProgressType_::UIBackgroundTask,
                    ptr::null(),
                    &mut id,
                ),
                kResultOk
            );
            for _ in 1..MAX_HOST_NOTIFICATIONS {
                assert_eq!(progress.update(id, 0.25), kResultOk);
            }
            assert_eq!(progress.update(id, 0.5), kResultFalse);
            assert_eq!(progress.finish(id), kResultFalse);
        }
        assert_eq!(
            host.take_progress_notifications().len(),
            MAX_HOST_NOTIFICATIONS
        );
        unsafe {
            assert_eq!(progress.finish(id), kResultOk);
        }
    }
}

#[cfg(test)]
mod connection_proxy_tests {
    use super::*;

    #[derive(Default)]
    struct RecordingConnectionPoint {
        notifications: AtomicUsize,
    }

    impl Class for RecordingConnectionPoint {
        type Interfaces = (IConnectionPoint,);
    }

    impl IConnectionPointTrait for RecordingConnectionPoint {
        unsafe fn connect(&self, _other: *mut IConnectionPoint) -> tresult {
            kResultOk
        }

        unsafe fn disconnect(&self, _other: *mut IConnectionPoint) -> tresult {
            kResultOk
        }

        unsafe fn notify(&self, _message: *mut IMessage) -> tresult {
            self.notifications.fetch_add(1, Ordering::Relaxed);
            kResultOk
        }
    }

    /// A proxy gated to the calling thread, plus the endpoint it forwards to.
    fn proxy_with_destination() -> (ConnectionProxy, ComWrapper<RecordingConnectionPoint>) {
        let destination = ComWrapper::new(RecordingConnectionPoint::default());
        let destination_ptr = destination
            .to_com_ptr::<IConnectionPoint>()
            .expect("RecordingConnectionPoint declares IConnectionPoint");
        let proxy = ConnectionProxy::new(destination_ptr, thread::current().id(), "test");
        (proxy, destination)
    }

    #[test]
    fn connection_proxy_forwards_only_on_its_control_thread() {
        let destination = ComWrapper::new(RecordingConnectionPoint::default());
        let destination_ptr = destination.to_com_ptr::<IConnectionPoint>().unwrap();
        let proxy = ComWrapper::new(ConnectionProxy::new(
            destination_ptr,
            thread::current().id(),
            "test",
        ));
        let proxy_ptr = proxy.to_com_ptr::<IConnectionPoint>().unwrap();
        let message = create_host_message().to_com_ptr::<IMessage>().unwrap();

        unsafe {
            assert_eq!(proxy_ptr.notify(message.as_ptr()), kResultOk);
        }
        assert_eq!(destination.notifications.load(Ordering::Relaxed), 1);

        let message_ptr = message.as_ptr() as usize;
        let result =
            std::thread::spawn(move || unsafe { proxy_ptr.notify(message_ptr as *mut IMessage) })
                .join()
                .unwrap();
        assert_eq!(result, kResultFalse);
        assert_eq!(destination.notifications.load(Ordering::Relaxed), 1);
    }

    /// The drop is the SDK's behaviour, but it must be countable — otherwise a plugin whose
    /// meters never update looks identical to a plugin that sends nothing.
    #[test]
    fn off_thread_notifies_are_counted_so_the_drop_is_diagnosable() {
        let (proxy, destination) = proxy_with_destination();
        let message = create_host_message().to_com_ptr::<IMessage>().unwrap();
        let message_ptr = message.as_ptr() as usize;

        let results = thread::scope(|scope| {
            scope
                .spawn(|| {
                    (0..3)
                        .map(|_| unsafe { proxy.notify(message_ptr as *mut IMessage) })
                        .collect::<Vec<_>>()
                })
                .join()
                .expect("notify never panics")
        });

        assert_eq!(results, vec![kResultFalse; 3]);
        assert_eq!(destination.notifications.load(Ordering::Relaxed), 0);
        assert_eq!(proxy.dropped_message_count(), 3);
    }

    /// Only the thread gate increments the counter: a forwarded message and a malformed one
    /// must not inflate the "your plugin is messaging off-thread" signal.
    #[test]
    fn forwarded_and_null_notifies_do_not_count_as_off_thread_drops() {
        let (proxy, destination) = proxy_with_destination();
        let message = create_host_message().to_com_ptr::<IMessage>().unwrap();

        assert_eq!(unsafe { proxy.notify(message.as_ptr()) }, kResultOk);
        assert_eq!(unsafe { proxy.notify(ptr::null_mut()) }, kResultFalse);

        assert_eq!(destination.notifications.load(Ordering::Relaxed), 1);
        assert_eq!(proxy.dropped_message_count(), 0);
    }
}

#[cfg(test)]
mod host_event_list_tests {
    use super::*;
    use crate::internal::native_edit_transport::tests::{allocation_free, measure_allocations};

    fn owned(data: PluginEventData, sample_offset: i32) -> PluginEvent {
        PluginEvent {
            bus_index: 3,
            sample_offset,
            ppq_position: 12.25,
            flags: 0x1234,
            data,
        }
    }

    fn note(sample_offset: i32) -> PluginEvent {
        owned(
            PluginEventData::NoteOn {
                channel: 4,
                pitch: 63,
                tuning: -12.5,
                velocity: 0.75,
                length: 127,
                note_id: 901,
            },
            sample_offset,
        )
    }

    fn raw_data(bytes: *const u8, size: u32) -> Event {
        let mut raw: Event = unsafe { std::mem::zeroed() };
        raw.r#type = Event_::EventTypes_::kDataEvent as u16;
        raw.__field0.data = DataEvent {
            size,
            r#type: 0xfeed,
            bytes,
        };
        raw
    }

    fn text_data(kind: usize, text: Vec<u16>) -> PluginEventData {
        match kind {
            0 => PluginEventData::NoteExpressionText {
                type_id: 44,
                note_id: 17,
                text,
            },
            1 => PluginEventData::Chord {
                root: -7,
                bass_note: 9,
                mask: 0x123,
                text,
            },
            2 => PluginEventData::Scale {
                root: -2,
                mask: 0x456,
                text,
            },
            _ => unreachable!(),
        }
    }

    fn state(list: &HostEventList) -> (usize, usize) {
        let events = list.events.lock().unwrap();
        (events.len(), list.payload_bytes.load(Ordering::Relaxed))
    }

    #[test]
    fn checked_scalar_admission_is_allocation_free_cold_through_exact_header_capacity() {
        let list = HostEventList::new();
        allocation_free(|| {
            for index in 0..MAX_QUEUED_EVENTS {
                let event = note(index as i32);
                match index % 3 {
                    0 => list.try_add_event(event).unwrap(),
                    1 => unsafe {
                        list.try_add_raw_event(&plugin_event_to_raw(&event).unwrap())
                            .unwrap();
                    },
                    _ => {
                        let mut raw = plugin_event_to_raw(&event).unwrap();
                        assert_eq!(unsafe { list.addEvent(&mut raw) }, kResultOk);
                    }
                }
            }
            assert_eq!(state(&list), (MAX_QUEUED_EVENTS, 0));
            assert!(!list.take_loss());
            assert_eq!(
                list.try_add_event(note(-1)),
                Err(EventAdmissionError::QueueFull)
            );
            let mut raw = plugin_event_to_raw(&note(-2)).unwrap();
            assert_eq!(
                unsafe { list.try_add_raw_event(&raw) },
                Err(EventAdmissionError::QueueFull)
            );
            assert_eq!(unsafe { list.addEvent(&mut raw) }, kResultFalse);
            assert_eq!(state(&list), (MAX_QUEUED_EVENTS, 0));
            assert!(list.take_loss());
            assert!(!list.take_loss());
        });
        let events = list.events.lock().unwrap();
        for (index, event) in events.iter().enumerate() {
            assert_eq!(*event, note(index as i32));
        }
    }

    #[test]
    fn full_headers_reject_before_touching_or_copying_payload() {
        let list = HostEventList::new();
        for _ in 0..MAX_QUEUED_EVENTS {
            list.try_add_event(note(0)).unwrap();
        }
        // The documented QueueFull short-circuit does not require readable payload
        // memory. This pointer has no backing allocation and must never be read.
        let mut raw = raw_data(ptr::dangling(), MAX_EVENT_PAYLOAD_BYTES as u32);
        allocation_free(|| {
            assert_eq!(
                unsafe { list.try_add_raw_event(&raw) },
                Err(EventAdmissionError::QueueFull)
            );
            assert_eq!(unsafe { list.addEvent(&mut raw) }, kResultFalse);
            assert_eq!(state(&list), (MAX_QUEUED_EVENTS, 0));
        });
    }

    #[test]
    fn checked_data_admission_enforces_exact_individual_limit_for_raw_and_owned() {
        let list = HostEventList::new();
        let source = vec![0x7d; MAX_EVENT_PAYLOAD_BYTES];
        let raw = raw_data(source.as_ptr(), source.len() as u32);
        let (result, stats) = measure_allocations(|| unsafe { list.try_add_raw_event(&raw) });
        assert_eq!(result, Ok(()));
        assert_eq!(
            stats.allocations, 1,
            "successful payload copying still allocates"
        );
        assert_eq!(stats.deallocations, 0);
        assert_eq!(stats.allocated_bytes, MAX_EVENT_PAYLOAD_BYTES);
        assert_eq!(state(&list), (1, MAX_EVENT_PAYLOAD_BYTES));

        let exact = PluginEvent::sysex(vec![0x7e; MAX_EVENT_PAYLOAD_BYTES]);
        allocation_free(|| list.try_add_event(exact).unwrap());
        let oversized = raw_data(ptr::dangling(), (MAX_EVENT_PAYLOAD_BYTES + 1) as u32);
        allocation_free(|| {
            assert_eq!(
                unsafe { list.try_add_raw_event(&oversized) },
                Err(EventAdmissionError::InvalidEvent)
            );
        });
        let too_large = PluginEvent::sysex(vec![0; MAX_EVENT_PAYLOAD_BYTES + 1]);
        let (result, stats) = measure_allocations(|| list.try_add_event(too_large));
        assert_eq!(result, Err(EventAdmissionError::InvalidEvent));
        assert_eq!(stats.allocations, 0);
        assert_eq!(
            stats.deallocations, 1,
            "rejected owned payloads are dropped"
        );
        assert_eq!(state(&list), (2, 2 * MAX_EVENT_PAYLOAD_BYTES));
        let events = list.events.lock().unwrap();
        assert_eq!(
            events[0].data,
            PluginEventData::Data {
                data_type: 0xfeed,
                bytes: source
            }
        );
    }

    #[test]
    fn checked_text_admission_enforces_each_exact_limit_and_preserves_declared_units() {
        for kind in 0..3 {
            let list = HostEventList::new();
            let exact = owned(text_data(kind, vec![0xd800; MAX_EVENT_TEXT_UNITS]), 4);
            let raw = plugin_event_to_raw(&exact).unwrap();
            unsafe { list.try_add_raw_event(&raw) }.unwrap();
            list.try_add_event(exact.clone()).unwrap();
            assert_eq!(state(&list), (2, 4 * MAX_EVENT_TEXT_UNITS));
            assert_eq!(list.events.lock().unwrap()[0], exact);

            let oversized = owned(text_data(kind, vec![0; MAX_EVENT_TEXT_UNITS + 1]), 5);
            let mut raw = plugin_event_to_raw(&oversized).unwrap();
            allocation_free(|| {
                assert_eq!(
                    unsafe { list.try_add_raw_event(&raw) },
                    Err(EventAdmissionError::InvalidEvent)
                );
                assert_eq!(unsafe { list.addEvent(&mut raw) }, kResultFalse);
            });
            assert_eq!(
                list.try_add_event(oversized),
                Err(EventAdmissionError::InvalidEvent)
            );
            assert_eq!(state(&list), (2, 4 * MAX_EVENT_TEXT_UNITS));

            // Embedded NULs and unpaired surrogates are copied verbatim, and no
            // terminator is scanned or appended beyond the declared three units.
            let source = owned(text_data(kind, vec![0xd800, 0, 0xffff]), 7);
            let raw = plugin_event_to_raw(&source).unwrap();
            unsafe { list.try_add_raw_event(&raw) }.unwrap();
            let expected = source.clone();
            drop(source);
            assert_eq!(list.events.lock().unwrap()[2], expected);
            let mut returned: Event = unsafe { std::mem::zeroed() };
            assert_eq!(unsafe { list.getEvent(2, &mut returned) }, kResultOk);
            assert_eq!(
                unsafe { raw_event_to_plugin_event(&returned) }.unwrap(),
                expected
            );
        }
    }

    #[test]
    fn checked_aggregate_budget_rejects_before_copy_and_keeps_the_existing_queue() {
        let list = HostEventList::new();
        let source = vec![0x7d; MAX_EVENT_PAYLOAD_BYTES];
        let raw = raw_data(source.as_ptr(), source.len() as u32);
        for _ in 0..MAX_QUEUED_EVENT_PAYLOAD_BYTES / MAX_EVENT_PAYLOAD_BYTES {
            unsafe { list.try_add_raw_event(&raw) }.unwrap();
        }
        assert_eq!(state(&list), (8, MAX_QUEUED_EVENT_PAYLOAD_BYTES));
        // The budget short-circuit is also documented not to dereference this
        // payload, whose declared metadata would otherwise permit a copy.
        let mut one_more = raw_data(ptr::dangling(), 1);
        allocation_free(|| {
            assert_eq!(
                unsafe { list.try_add_raw_event(&one_more) },
                Err(EventAdmissionError::PayloadBudget)
            );
            assert_eq!(unsafe { list.addEvent(&mut one_more) }, kResultFalse);
        });
        assert_eq!(
            list.try_add_event(PluginEvent::sysex(vec![9])),
            Err(EventAdmissionError::PayloadBudget)
        );
        assert_eq!(state(&list), (8, MAX_QUEUED_EVENT_PAYLOAD_BYTES));
        assert!(list.take_loss());
        assert!(!list.take_loss());
        allocation_free(|| list.try_add_event(note(42)).unwrap());
        assert_eq!(state(&list), (9, MAX_QUEUED_EVENT_PAYLOAD_BYTES));
        assert!(list.events.lock().unwrap()[..8]
            .iter()
            .all(|event| matches!(
                &event.data, PluginEventData::Data { data_type: 0xfeed, bytes } if bytes == &source
            )));
    }

    #[test]
    fn mixed_text_and_data_share_one_exact_aggregate_byte_budget() {
        let list = HostEventList::new();
        for _ in 0..7 {
            list.try_add_event(PluginEvent::sysex(vec![1; MAX_EVENT_PAYLOAD_BYTES]))
                .unwrap();
        }
        let text_bytes = MAX_EVENT_TEXT_UNITS * std::mem::size_of::<u16>();
        for index in 0..MAX_EVENT_PAYLOAD_BYTES / text_bytes {
            let event = owned(
                text_data(index % 3, vec![0x1234; MAX_EVENT_TEXT_UNITS]),
                index as i32,
            );
            list.try_add_event(event).unwrap();
        }
        let before = state(&list);
        assert_eq!(before, (39, MAX_QUEUED_EVENT_PAYLOAD_BYTES));
        assert_eq!(
            list.try_add_event(PluginEvent::sysex(vec![1])),
            Err(EventAdmissionError::PayloadBudget)
        );
        let text = owned(text_data(0, vec![0x4321]), 0);
        let raw = plugin_event_to_raw(&text).unwrap();
        allocation_free(|| {
            assert_eq!(
                unsafe { list.try_add_raw_event(&raw) },
                Err(EventAdmissionError::PayloadBudget)
            );
        });
        assert_eq!(
            list.try_add_event(text),
            Err(EventAdmissionError::PayloadBudget)
        );
        assert_eq!(state(&list), before);
    }

    #[test]
    fn malformed_raw_payloads_and_unknown_variants_fail_without_allocation_or_mutation() {
        let list = HostEventList::new();
        list.try_add_event(PluginEvent::sysex(vec![1, 2, 3]))
            .unwrap();
        let mut unknown: Event = unsafe { std::mem::zeroed() };
        // 0xffff is the supported legacy MIDI CC event, so use the unassigned
        // adjacent value rather than accidentally exercising a valid scalar.
        unknown.r#type = u16::MAX - 1;
        let bad_data = [
            raw_data(ptr::null(), 1),
            raw_data(ptr::dangling(), u32::MAX),
            unknown,
        ];
        allocation_free(|| {
            for mut raw in bad_data {
                assert_eq!(
                    unsafe { list.try_add_raw_event(&raw) },
                    Err(EventAdmissionError::InvalidEvent)
                );
                assert_eq!(unsafe { list.addEvent(&mut raw) }, kResultFalse);
                assert_eq!(state(&list), (1, 3));
            }
        });
        let source = [0u16; 2];
        let misaligned = unsafe { source.as_ptr().cast::<u8>().add(1).cast::<u16>() };
        for kind in 0..3 {
            let event = owned(text_data(kind, vec![3]), 1);
            for pointer in [ptr::null(), misaligned] {
                let mut raw = plugin_event_to_raw(&event).unwrap();
                match kind {
                    0 => raw.__field0.noteExpressionText.text = pointer,
                    1 => raw.__field0.chord.text = pointer,
                    _ => raw.__field0.scale.text = pointer,
                }
                allocation_free(|| {
                    assert_eq!(
                        unsafe { list.try_add_raw_event(&raw) },
                        Err(EventAdmissionError::InvalidEvent)
                    );
                    assert_eq!(unsafe { list.addEvent(&mut raw) }, kResultFalse);
                    assert_eq!(state(&list), (1, 3));
                });
            }
        }
        allocation_free(|| {
            assert_eq!(unsafe { list.addEvent(ptr::null_mut()) }, kResultFalse);
            assert_eq!(
                unsafe { list.addEvent(ptr::dangling_mut::<u8>().cast()) },
                kResultFalse
            );
            assert_eq!(state(&list), (1, 3));
            assert!(list.take_loss());
            assert!(!list.take_loss());
        });
    }

    #[test]
    fn empty_payloads_accept_null_pointers_without_reading_or_allocating() {
        let list = HostEventList::new();
        let raw = raw_data(ptr::null(), 0);
        allocation_free(|| unsafe { list.try_add_raw_event(&raw).unwrap() });
        for kind in 0..3 {
            let event = owned(text_data(kind, Vec::new()), 0);
            let mut raw = plugin_event_to_raw(&event).unwrap();
            match kind {
                0 => raw.__field0.noteExpressionText.text = ptr::null(),
                1 => raw.__field0.chord.text = ptr::null(),
                _ => raw.__field0.scale.text = ptr::null(),
            }
            allocation_free(|| unsafe { list.try_add_raw_event(&raw).unwrap() });
        }
        assert_eq!(state(&list), (4, 0));
        assert!(!list.take_loss());
    }

    #[test]
    fn poisoned_admission_reports_loss_without_allocating_or_mutating() {
        let list = HostEventList::new();
        list.try_add_event(note(17)).unwrap();
        list.try_add_event(PluginEvent::sysex(vec![1, 2, 3]))
            .unwrap();
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = list.events.lock().unwrap();
            panic!("poison the event mutex outside allocation measurement");
        }))
        .is_err());
        let mut raw = plugin_event_to_raw(&note(29)).unwrap();
        allocation_free(|| {
            assert_eq!(
                list.try_add_event(note(19)),
                Err(EventAdmissionError::Poisoned)
            );
            assert_eq!(
                unsafe { list.try_add_raw_event(&raw) },
                Err(EventAdmissionError::Poisoned)
            );
            assert_eq!(unsafe { list.addEvent(&mut raw) }, kResultFalse);
            assert!(list.take_loss());
            assert!(!list.take_loss());
        });
        let events = list.events.lock().unwrap_err().into_inner();
        assert_eq!(
            events.as_slice(),
            &[note(17), PluginEvent::sysex(vec![1, 2, 3])]
        );
        assert_eq!(list.payload_bytes.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn checked_payload_arithmetic_rejects_overflow_without_mutation() {
        assert_eq!(
            checked_payload_bytes::<u16>(usize::MAX, usize::MAX),
            Err(EventAdmissionError::InvalidEvent)
        );
        let list = HostEventList::new();
        list.payload_bytes.store(usize::MAX, Ordering::Relaxed);
        let raw = raw_data(ptr::dangling(), 1);
        allocation_free(|| {
            assert_eq!(
                unsafe { list.try_add_raw_event(&raw) },
                Err(EventAdmissionError::PayloadBudget)
            );
            assert_eq!(
                list.try_add_event(note(0)),
                Err(EventAdmissionError::PayloadBudget)
            );
            assert_eq!(state(&list), (0, usize::MAX));
        });
    }

    #[test]
    fn all_scalar_variants_keep_their_values_and_fifo_offsets() {
        let list = HostEventList::new();
        let scalars = [
            note(93).data,
            PluginEventData::NoteOff {
                channel: 5,
                pitch: 74,
                velocity: 0.625,
                note_id: -8,
                tuning: 3.5,
            },
            PluginEventData::PolyPressure {
                channel: 6,
                pitch: 65,
                pressure: 0.5,
                note_id: 37,
            },
            PluginEventData::NoteExpressionValue {
                type_id: u32::MAX,
                note_id: -10,
                value: -0.125,
            },
            PluginEventData::NoteExpressionIntValue {
                type_id: 42,
                note_id: 7,
                value: u64::MAX,
            },
            PluginEventData::LegacyMidiCcOut {
                control_number: 130,
                channel: -3,
                value: 200,
                value2: 250,
            },
        ];
        allocation_free(|| {
            for (index, data) in scalars.iter().enumerate() {
                let event = owned(data.clone(), 93 - index as i32 * 17);
                let mut raw = plugin_event_to_raw(&event).unwrap();
                assert_eq!(unsafe { list.addEvent(&mut raw) }, kResultOk);
            }
            let events = list.events.lock().unwrap();
            for (index, event) in events.iter().enumerate() {
                assert_eq!(
                    *event,
                    owned(scalars[index].clone(), 93 - index as i32 * 17)
                );
            }
        });
    }

    #[test]
    fn concurrent_admission_cannot_overcommit_payload_budget() {
        let list = HostEventList::new();
        for _ in 0..7 {
            list.try_add_event(PluginEvent::sysex(vec![0; MAX_EVENT_PAYLOAD_BYTES]))
                .unwrap();
        }
        let barrier = std::sync::Barrier::new(2);
        let results = std::thread::scope(|scope| {
            let spawn = || {
                scope.spawn(|| {
                    let source = vec![0x7f; MAX_EVENT_PAYLOAD_BYTES];
                    let raw = raw_data(source.as_ptr(), source.len() as u32);
                    barrier.wait();
                    unsafe { list.try_add_raw_event(&raw) }
                })
            };
            let first = spawn();
            let second = spawn();
            [first.join().unwrap(), second.join().unwrap()]
        });
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| **result == Err(EventAdmissionError::PayloadBudget))
                .count(),
            1
        );
        assert_eq!(state(&list), (8, MAX_QUEUED_EVENT_PAYLOAD_BYTES));
    }

    /// `process()` is the input list's only drain and it returns early while the plugin isn't
    /// processing, so queueing MIDI at a stopped plugin must not grow the list forever.
    #[test]
    fn queued_events_are_capped_so_a_stopped_plugin_cannot_grow_them() {
        let list = HostEventList::new();
        let event: Event = unsafe { std::mem::zeroed() };

        for _ in 0..(MAX_QUEUED_EVENTS + 500) {
            let _ = unsafe { list.try_add_raw_event(&event) };
        }
        assert_eq!(unsafe { list.getEventCount() }, MAX_QUEUED_EVENTS as i32);

        // The plugin-facing COM path is capped too, and reports the refusal.
        let mut event = event;
        assert_eq!(
            unsafe { list.addEvent(&mut event as *mut Event) },
            kResultFalse
        );

        // Clearing (as each block does) makes room again without dropping capacity.
        list.clear();
        assert_eq!(unsafe { list.getEventCount() }, 0);
        assert!(list.events.lock().unwrap().capacity() >= MAX_QUEUED_EVENTS);
        unsafe { list.try_add_raw_event(&event) }.unwrap();
        assert_eq!(unsafe { list.getEventCount() }, 1);
    }

    #[test]
    fn sysex_is_deep_copied_before_plugin_memory_expires() {
        let list = HostEventList::new();
        let source = vec![0xf0, 0x7d, 1, 2, 3, 0xf7];
        let source_ptr = source.as_ptr();
        let mut raw: Event = unsafe { std::mem::zeroed() };
        raw.r#type = Event_::EventTypes_::kDataEvent as u16;
        raw.__field0.data = DataEvent {
            size: source.len() as u32,
            r#type: DataEvent_::DataTypes_::kMidiSysEx,
            bytes: source_ptr,
        };

        assert_eq!(unsafe { list.addEvent(&mut raw) }, kResultOk);
        drop(source);

        let stored = list.events.lock().unwrap();
        let PluginEventData::Data { bytes, .. } = &stored[0].data else {
            panic!("expected data event");
        };
        assert_eq!(bytes, &[0xf0, 0x7d, 1, 2, 3, 0xf7]);
        assert_ne!(bytes.as_ptr(), source_ptr);
        drop(stored);

        let mut returned: Event = unsafe { std::mem::zeroed() };
        assert_eq!(unsafe { list.getEvent(0, &mut returned) }, kResultOk);
        let returned_data = unsafe { returned.__field0.data };
        assert_eq!(
            unsafe { std::slice::from_raw_parts(returned_data.bytes, returned_data.size as usize) },
            &[0xf0, 0x7d, 1, 2, 3, 0xf7]
        );
    }

    #[test]
    fn malformed_or_oversized_pointer_payloads_are_rejected() {
        let list = HostEventList::new();
        let mut raw: Event = unsafe { std::mem::zeroed() };
        raw.r#type = Event_::EventTypes_::kDataEvent as u16;
        raw.__field0.data = DataEvent {
            size: 1,
            r#type: DataEvent_::DataTypes_::kMidiSysEx,
            bytes: ptr::null(),
        };
        assert_eq!(unsafe { list.addEvent(&mut raw) }, kResultFalse);

        raw.__field0.data.size = (MAX_EVENT_PAYLOAD_BYTES + 1) as u32;
        raw.__field0.data.bytes = std::ptr::dangling();
        assert_eq!(unsafe { list.addEvent(&mut raw) }, kResultFalse);
        assert_eq!(unsafe { list.getEventCount() }, 0);
    }
}

#[cfg(test)]
mod plug_frame_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicI32, AtomicPtr, AtomicUsize, Ordering};

    struct FakePlugView {
        on_size_calls: Arc<AtomicUsize>,
        on_size_result: Arc<AtomicI32>,
        observed_unlocked_slot: Arc<AtomicBool>,
        slot: Arc<Mutex<Option<(i32, i32)>>>,
        reenter_once: AtomicBool,
        frame: AtomicPtr<IPlugFrame>,
        view: Arc<AtomicPtr<IPlugView>>,
    }

    impl Class for FakePlugView {
        type Interfaces = (IPlugView,);
    }

    impl IPlugViewTrait for FakePlugView {
        unsafe fn isPlatformTypeSupported(&self, _type: FIDString) -> tresult {
            kResultOk
        }

        unsafe fn attached(&self, _parent: *mut std::ffi::c_void, _type: FIDString) -> tresult {
            kResultOk
        }

        unsafe fn removed(&self) -> tresult {
            kResultOk
        }

        unsafe fn onWheel(&self, _distance: f32) -> tresult {
            kResultOk
        }

        unsafe fn onKeyDown(&self, _key: char16, _key_code: int16, _modifiers: int16) -> tresult {
            kResultOk
        }

        unsafe fn onKeyUp(&self, _key: char16, _key_code: int16, _modifiers: int16) -> tresult {
            kResultOk
        }

        unsafe fn getSize(&self, _size: *mut ViewRect) -> tresult {
            kResultOk
        }

        unsafe fn onSize(&self, _new_size: *mut ViewRect) -> tresult {
            self.on_size_calls.fetch_add(1, Ordering::SeqCst);
            self.observed_unlocked_slot
                .store(self.slot.try_lock().is_ok(), Ordering::SeqCst);

            if self.reenter_once.swap(false, Ordering::SeqCst) {
                let frame = ComRef::<IPlugFrame>::from_raw(self.frame.load(Ordering::SeqCst))
                    .expect("frame pointer");
                let mut nested = ViewRect {
                    left: 0,
                    top: 0,
                    right: 321,
                    bottom: 123,
                };
                assert_eq!(
                    frame.resizeView(self.view.load(Ordering::SeqCst), &mut nested),
                    kResultOk
                );
            }
            self.on_size_result.load(Ordering::SeqCst)
        }

        unsafe fn onFocus(&self, _state: TBool) -> tresult {
            kResultOk
        }

        unsafe fn setFrame(&self, _frame: *mut IPlugFrame) -> tresult {
            kResultOk
        }

        unsafe fn canResize(&self) -> tresult {
            kResultTrue
        }

        unsafe fn checkSizeConstraint(&self, _rect: *mut ViewRect) -> tresult {
            kResultOk
        }
    }

    #[cfg(target_os = "linux")]
    fn make_frame(slot: Arc<Mutex<Option<(i32, i32)>>>) -> HostPlugFrame {
        HostPlugFrame::new(slot, Arc::new(Mutex::new(RunLoopRegistry::new())))
    }

    #[cfg(not(target_os = "linux"))]
    fn make_frame(slot: Arc<Mutex<Option<(i32, i32)>>>) -> HostPlugFrame {
        HostPlugFrame::new(slot)
    }

    #[test]
    fn records_requested_size_and_calls_on_size_synchronously() {
        let slot = Arc::new(Mutex::new(None));
        let frame = make_frame(slot.clone());
        let calls = Arc::new(AtomicUsize::new(0));
        let unlocked = Arc::new(AtomicBool::new(false));
        let view = ComWrapper::new(FakePlugView {
            on_size_calls: calls.clone(),
            on_size_result: Arc::new(AtomicI32::new(kResultOk)),
            observed_unlocked_slot: unlocked.clone(),
            slot: slot.clone(),
            reenter_once: AtomicBool::new(false),
            frame: AtomicPtr::new(std::ptr::null_mut()),
            view: Arc::new(AtomicPtr::new(std::ptr::null_mut())),
        });
        let view = view.to_com_ptr::<IPlugView>().unwrap();
        let mut rect = ViewRect {
            left: 0,
            top: 0,
            right: 640,
            bottom: 480,
        };
        let r = unsafe { frame.resizeView(view.as_ptr(), &mut rect) };
        assert_eq!(r, kResultOk);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(unlocked.load(Ordering::SeqCst));
        assert_eq!(*slot.lock().unwrap(), Some((640, 480)));
    }

    #[test]
    fn rejects_nulls_and_invalid_dimensions_without_recording() {
        let slot = Arc::new(Mutex::new(None));
        let frame = make_frame(slot.clone());
        let mut rect = ViewRect {
            left: 5,
            top: 5,
            right: 5,
            bottom: 10,
        };
        assert_eq!(
            unsafe { frame.resizeView(std::ptr::null_mut(), &mut rect) },
            kInvalidArgument
        );
        assert_eq!(
            unsafe { frame.resizeView(std::ptr::null_mut(), std::ptr::null_mut()) },
            kInvalidArgument
        );
        assert_eq!(*slot.lock().unwrap(), None);
    }

    #[test]
    fn propagates_on_size_failure_and_allows_reentrant_resize() {
        let slot = Arc::new(Mutex::new(None));
        let frame = make_frame(slot.clone());
        let frame_wrapper = {
            #[cfg(target_os = "linux")]
            {
                ComWrapper::new(HostPlugFrame::new(
                    slot.clone(),
                    Arc::new(Mutex::new(RunLoopRegistry::new())),
                ))
            }
            #[cfg(not(target_os = "linux"))]
            {
                ComWrapper::new(HostPlugFrame::new(slot.clone()))
            }
        };
        let frame_ptr = frame_wrapper.to_com_ptr::<IPlugFrame>().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let on_size_result = Arc::new(AtomicI32::new(kResultOk));
        let view_ptr = Arc::new(AtomicPtr::new(std::ptr::null_mut()));
        let view_wrapper = ComWrapper::new(FakePlugView {
            on_size_calls: calls.clone(),
            on_size_result: on_size_result.clone(),
            observed_unlocked_slot: Arc::new(AtomicBool::new(false)),
            slot: slot.clone(),
            reenter_once: AtomicBool::new(true),
            frame: AtomicPtr::new(frame_ptr.as_ptr()),
            view: view_ptr.clone(),
        });
        let view = view_wrapper.to_com_ptr::<IPlugView>().unwrap();
        view_ptr.store(view.as_ptr(), Ordering::SeqCst);

        let mut rect = ViewRect {
            left: 0,
            top: 0,
            right: 640,
            bottom: 480,
        };
        assert_eq!(
            unsafe { frame_ptr.resizeView(view.as_ptr(), &mut rect) },
            kResultOk
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(*slot.lock().unwrap(), Some((321, 123)));

        on_size_result.store(kResultFalse, Ordering::SeqCst);
        assert_eq!(
            unsafe { frame.resizeView(view.as_ptr(), &mut rect) },
            kResultFalse
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn run_loop_rejects_null_registrations_and_starts_empty() {
        use vst3::Steinberg::Linux::IRunLoopTrait;
        let reg = Arc::new(Mutex::new(RunLoopRegistry::new()));
        let frame = HostPlugFrame::new(Arc::new(Mutex::new(None)), reg.clone());
        // A registry with no editor attached is empty.
        assert!(reg.lock().unwrap().handlers.is_empty());
        assert!(reg.lock().unwrap().timers.is_empty());
        // Null handlers are rejected without touching the registry.
        unsafe {
            assert_eq!(
                frame.registerEventHandler(std::ptr::null_mut(), 3),
                kInvalidArgument
            );
            assert_eq!(
                frame.registerTimer(std::ptr::null_mut(), 16),
                kInvalidArgument
            );
        }
        assert!(reg.lock().unwrap().handlers.is_empty());
        assert!(reg.lock().unwrap().timers.is_empty());
    }
}

#[cfg(test)]
mod parameter_changes_tests {
    use super::*;
    use crate::internal::native_edit_transport::tests::{allocation_free, measure_allocations};
    use std::time::Instant;
    use vst3::com_scrape_types::Unknown;

    fn limits(max_queues: usize, max_points: usize) -> ParameterQueueLimits {
        ParameterQueueLimits {
            max_queues,
            max_points,
        }
    }

    fn points(changes: &ParameterChanges) -> Vec<(u32, i32, f64)> {
        let mut points = Vec::new();
        changes
            .for_each_active_point(|id, offset, value| points.push((id, offset, value)))
            .unwrap();
        points
    }

    unsafe fn queue(changes: &ParameterChanges, slot: i32) -> ComRef<'_, IParamValueQueue> {
        ComRef::from_raw(changes.getParameterData(slot)).unwrap()
    }

    #[test]
    fn parameter_storage_derives_thread_safety_without_unsafe_impls() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ParameterChanges>();
        assert_send_sync::<ParameterValueQueue>();
    }

    #[test]
    fn enqueue_groups_orders_preserves_ties_repetitions_and_clears() {
        let changes = ParameterChanges::new(limits(3, 8));
        allocation_free(|| {
            changes.try_enqueue(7, 64, 0.9).unwrap();
            changes.try_enqueue(7, 0, 0.5).unwrap();
            changes.try_enqueue(3, 0, 0.1).unwrap();
            changes.try_enqueue(7, 64, 0.1).unwrap();
            changes.try_enqueue(7, 0, 0.5).unwrap();
            assert_eq!(unsafe { changes.getParameterCount() }, 2);
        });
        assert_eq!(
            points(&changes),
            [
                (7, 0, 0.5),
                (7, 0, 0.5),
                (7, 64, 0.9),
                (7, 64, 0.1),
                (3, 0, 0.1)
            ]
        );
        allocation_free(|| {
            changes.clear_all().unwrap();
            assert_eq!(unsafe { changes.getParameterCount() }, 0);
            changes
                .for_each_active_point(|_, _, _| panic!("stale point"))
                .unwrap();
            changes.try_enqueue(22, 3, 0.8).unwrap();
        });
        assert_eq!(points(&changes), [(22, 3, 0.8)]);
        assert_eq!(changes.failure(), None);
    }

    #[test]
    fn enqueue_with_nan_orders_by_offset_without_changing_value_bits() {
        let changes = ParameterChanges::new(limits(1, 3));
        let nan = f64::from_bits(0x7ff8_0000_0000_1234);
        allocation_free(|| {
            changes.try_enqueue(1, 128, nan).unwrap();
            changes.try_enqueue(1, 0, 0.25).unwrap();
            changes.try_enqueue(1, 64, nan).unwrap();
            let queue = unsafe { queue(&changes, 0) };
            for (i, offset, expected) in [(0, 0, 0.25), (1, 64, nan), (2, 128, nan)] {
                let mut got_offset = -1;
                let mut value = 0.0;
                assert_eq!(
                    unsafe { queue.getPoint(i, &mut got_offset, &mut value) },
                    kResultOk
                );
                assert_eq!(got_offset, offset);
                assert_eq!(value.to_bits(), expected.to_bits());
            }
        });
    }

    #[test]
    fn cold_8192_distinct_ids_fit_input_without_allocation() {
        let changes = ParameterChanges::new(ParameterQueueLimits::INPUT);
        allocation_free(|| {
            for id in 0..8192 {
                changes.try_enqueue(id, 0, id as f64).unwrap();
            }
            assert_eq!(unsafe { changes.getParameterCount() }, 8192);
            for slot in 0..8192 {
                let queue = unsafe { queue(&changes, slot) };
                assert_eq!(unsafe { queue.getParameterId() }, slot as u32);
                assert_eq!(unsafe { queue.getPointCount() }, 1);
                let mut value = 0.0;
                assert_eq!(
                    unsafe { queue.getPoint(0, ptr::null_mut(), &mut value) },
                    kResultOk
                );
                assert_eq!(value, slot as f64);
            }
            assert_eq!(
                changes.try_enqueue(9000, 0, 1.0),
                Err(ParameterStorageError::PointCapacity)
            );
            assert_eq!(unsafe { changes.getParameterCount() }, 8192);
            changes.clear_all().unwrap();
            assert_eq!(
                changes.failure(),
                Some(ParameterStorageError::PointCapacity)
            );
            for id in 0..8192 {
                changes.try_enqueue(20_000 + id, 3, 0.75).unwrap();
            }
            assert_eq!(unsafe { changes.getParameterCount() }, 8192);
        });
    }

    #[test]
    fn cold_dense_8192_curve_and_sequential_reads_do_not_allocate() {
        let changes = ParameterChanges::new(ParameterQueueLimits::INPUT);
        allocation_free(|| {
            for i in 0..8192 {
                changes.try_enqueue(42, i, i as f64).unwrap();
            }
            let queue = unsafe { queue(&changes, 0) };
            assert_eq!(unsafe { queue.getPointCount() }, 8192);
            for i in 0..8192 {
                let mut offset = -1;
                let mut value = -1.0;
                assert_eq!(
                    unsafe { queue.getPoint(i, &mut offset, &mut value) },
                    kResultOk
                );
                assert_eq!((offset, value), (i, i as f64));
            }
            assert_eq!(
                changes.try_enqueue(42, 8192, 1.0),
                Err(ParameterStorageError::PointCapacity)
            );
            assert_eq!(unsafe { queue.getPointCount() }, 8192);
        });
    }

    #[test]
    fn input_combines_4096_host_and_4096_native_points_in_one_arena() {
        let changes = ParameterChanges::new(ParameterQueueLimits::INPUT);
        allocation_free(|| {
            for id in 0..4096 {
                changes.try_enqueue(id, 17, id as f64).unwrap();
            }
            for i in 0..4096 {
                changes.try_enqueue(0, 0, i as f64).unwrap();
            }
            assert_eq!(unsafe { changes.getParameterCount() }, 4096);
            let first = unsafe { queue(&changes, 0) };
            assert_eq!(unsafe { first.getPointCount() }, 4097);
            for i in 0..4096 {
                let mut value = -1.0;
                assert_eq!(
                    unsafe { first.getPoint(i, ptr::null_mut(), &mut value) },
                    kResultOk
                );
                assert_eq!(value, i as f64);
            }
            let mut offset = -1;
            assert_eq!(
                unsafe { first.getPoint(4096, &mut offset, ptr::null_mut()) },
                kResultOk
            );
            assert_eq!(offset, 17);
        });
        assert_eq!(changes.failure(), None);
    }

    #[test]
    fn output_4096_queue_and_shared_point_budget_rejects_next_write() {
        let changes = ParameterChanges::new(ParameterQueueLimits::OUTPUT);
        allocation_free(|| unsafe {
            for id in 0..4096 {
                let mut index = -1;
                let ptr = changes.addParameterData(&id, &mut index);
                assert!(!ptr.is_null());
                assert_eq!(index, id as i32);
                let queue = ComRef::from_raw(ptr).unwrap();
                assert_eq!(queue.addPoint(0, id as f64, &mut index), kResultOk);
                assert_eq!(index, 0);
            }
            let mut index = 10;
            let first = queue(&changes, 0);
            assert_eq!(first.addPoint(1, 0.5, &mut index), kResultFalse);
            assert_eq!(index, -1);
            assert_eq!(first.getPointCount(), 1);
            assert!(changes.addParameterData(&9000, &mut index).is_null());
            assert_eq!(index, -1);
            assert_eq!(changes.getParameterCount(), 4096);
            assert_eq!(
                changes.failure(),
                Some(ParameterStorageError::PointCapacity)
            );
        });
    }

    #[test]
    fn failed_host_insert_never_activates_phantom_queue() {
        let changes = ParameterChanges::new(limits(3, 1));
        allocation_free(|| {
            changes.try_enqueue(1, 0, 0.2).unwrap();
            assert_eq!(
                changes.try_enqueue(2, 0, 0.3),
                Err(ParameterStorageError::PointCapacity)
            );
            assert_eq!(unsafe { changes.getParameterCount() }, 1);
            assert_eq!(unsafe { queue(&changes, 0).getParameterId() }, 1);
            changes.clear_all().unwrap();
            assert_eq!(
                changes.failure(),
                Some(ParameterStorageError::PointCapacity)
            );
            changes.try_enqueue(2, 0, 0.3).unwrap();
            assert_eq!(unsafe { changes.getParameterCount() }, 1);
        });
        assert_eq!(points(&changes), [(2, 0, 0.3)]);
        let empty = ParameterChanges::new(limits(2, 0));
        allocation_free(|| {
            assert_eq!(
                empty.try_enqueue(7, 0, 1.0),
                Err(ParameterStorageError::PointCapacity)
            );
            assert_eq!(unsafe { empty.getParameterCount() }, 0);
        });
    }

    #[test]
    fn queue_capacity_and_empty_com_queue_admission_are_explicit() {
        let changes = ParameterChanges::new(limits(2, 3));
        allocation_free(|| unsafe {
            let mut index = -1;
            let first = changes.addParameterData(&7, &mut index);
            assert_eq!(index, 0);
            assert!(!first.is_null());
            assert_eq!(ComRef::from_raw(first).unwrap().getPointCount(), 0);
            assert_eq!(changes.addParameterData(&7, ptr::null_mut()), first);
            changes.try_enqueue(3, 0, 1.0).unwrap();
            assert_eq!(
                changes.try_enqueue(9, 0, 0.1),
                Err(ParameterStorageError::QueueCapacity)
            );
            assert!(changes.addParameterData(&9, &mut index).is_null());
            assert_eq!(index, -1);
            assert_eq!(changes.getParameterCount(), 2);
            changes.try_enqueue(7, 0, 0.5).unwrap();
            changes.try_enqueue(7, 0, 0.5).unwrap();
            assert_eq!(
                changes.failure(),
                Some(ParameterStorageError::QueueCapacity)
            );
        });
        assert_eq!(points(&changes), [(7, 0, 0.5), (7, 0, 0.5), (3, 0, 1.0)]);
        let zero = ParameterChanges::new(limits(0, 0));
        allocation_free(|| unsafe {
            let mut index = 99;
            assert!(zero.addParameterData(&7, &mut index).is_null());
            assert_eq!(index, -1);
            assert_eq!(zero.getParameterCount(), 0);
            zero.clear_all().unwrap();
            assert_eq!(zero.failure(), Some(ParameterStorageError::QueueCapacity));
        });
    }

    #[test]
    fn invalid_reads_and_optional_null_outputs_fail_without_sticky_loss() {
        let changes = ParameterChanges::new(limits(1, 2));
        allocation_free(|| unsafe {
            assert!(changes.getParameterData(-1).is_null());
            assert!(changes.getParameterData(0).is_null());
            let raw = changes.addParameterData(&4, ptr::null_mut());
            let queue = ComRef::from_raw(raw).unwrap();
            assert_eq!(queue.addPoint(-4, 0.2, ptr::null_mut()), kResultOk);
            assert_eq!(
                queue.getPoint(0, ptr::null_mut(), ptr::null_mut()),
                kResultOk
            );
            for index in [-1, 1, i32::MAX] {
                let mut offset = 80;
                let mut value = 90.0;
                assert_eq!(queue.getPoint(index, &mut offset, &mut value), kResultFalse);
                assert_eq!((offset, value), (80, 90.0));
            }
            assert!(changes.getParameterData(i32::MAX).is_null());
            assert_eq!(changes.failure(), None);
        });
    }

    #[test]
    fn invalid_write_sets_failed_index_and_latches_without_panicking() {
        let changes = ParameterChanges::new(limits(1, 2));
        allocation_free(|| unsafe {
            let mut index = 87;
            assert!(changes.addParameterData(ptr::null(), &mut index).is_null());
            assert_eq!(index, -1);
            assert_eq!(changes.getParameterCount(), 0);
            assert_eq!(
                changes.failure(),
                Some(ParameterStorageError::InvalidArgument)
            );
            changes.clear_all().unwrap();
            assert_eq!(
                changes.failure(),
                Some(ParameterStorageError::InvalidArgument)
            );
        });
    }

    #[test]
    fn alternating_reads_and_insertions_update_the_shared_index() {
        let changes = ParameterChanges::new(limits(1, 16));
        allocation_free(|| unsafe {
            for offset in [0, 20, 40, 60] {
                changes.try_enqueue(8, offset, offset as f64).unwrap();
            }
            let queue = queue(&changes, 0);
            for index in [3, 0, 2, 1, 3, 3, 0] {
                let mut offset = -1;
                assert_eq!(
                    queue.getPoint(index, &mut offset, ptr::null_mut()),
                    kResultOk
                );
                assert_eq!(offset, index * 20);
            }
            let mut index = -1;
            assert_eq!(
                queue.getPoint(3, ptr::null_mut(), ptr::null_mut()),
                kResultOk
            );
            assert_eq!(queue.addPoint(-10, -10.0, &mut index), kResultOk);
            assert_eq!(index, 0);
            assert_eq!(
                queue.getPoint(2, ptr::null_mut(), ptr::null_mut()),
                kResultOk
            );
            assert_eq!(queue.addPoint(20, 99.0, &mut index), kResultOk);
            assert_eq!(index, 3);
            assert_eq!(
                queue.getPoint(4, ptr::null_mut(), ptr::null_mut()),
                kResultOk
            );
            assert_eq!(queue.addPoint(80, 80.0, &mut index), kResultOk);
            assert_eq!(index, 6);
            for (index, (expected_offset, expected_value)) in [
                (-10, -10.0),
                (0, 0.0),
                (20, 20.0),
                (20, 99.0),
                (40, 40.0),
                (60, 60.0),
                (80, 80.0),
            ]
            .into_iter()
            .enumerate()
            {
                let mut offset = -1;
                let mut value = -1.0;
                assert_eq!(
                    queue.getPoint(index as i32, &mut offset, &mut value),
                    kResultOk
                );
                assert_eq!((offset, value), (expected_offset, expected_value));
            }
        });
    }

    #[test]
    fn shared_index_tracks_multiqueue_interleaving_reset_and_retarget() {
        let changes = ParameterChanges::new(limits(4, 8));
        allocation_free(|| unsafe {
            changes.try_enqueue(7, 20, 0.2).unwrap();
            changes.try_enqueue(8, 30, 0.3).unwrap();
            let first = queue(&changes, 0);
            let second = queue(&changes, 1);
            let mut offset = -1;
            assert_eq!(second.getPoint(0, &mut offset, ptr::null_mut()), kResultOk);
            assert_eq!(offset, 30);
            assert!(!changes.arena.lock().unwrap().cache_dirty);
            // Inserting in the first queue shifts the second queue's ordinal slice.
            assert_eq!(first.addPoint(10, 0.1, ptr::null_mut()), kResultOk);
            assert!(!changes.arena.lock().unwrap().cache_dirty);
            assert_eq!(second.getPoint(0, &mut offset, ptr::null_mut()), kResultOk);
            assert_eq!(offset, 30);
            let third = changes.addParameterData(&9, ptr::null_mut());
            let third = ComRef::from_raw(third).unwrap();
            assert_eq!(third.addPoint(40, 0.4, ptr::null_mut()), kResultOk);
            assert_eq!(second.addPoint(0, 0.0, ptr::null_mut()), kResultOk);
            assert_eq!(third.getPoint(0, &mut offset, ptr::null_mut()), kResultOk);
            assert_eq!(offset, 40);
            assert_eq!(second.getPoint(0, &mut offset, ptr::null_mut()), kResultOk);
            assert_eq!(offset, 0);
            assert_eq!(first.getPoint(1, &mut offset, ptr::null_mut()), kResultOk);
            assert_eq!(offset, 20);
            changes.clear_all().unwrap();
            assert_eq!(first.getPointCount(), 0);
            assert_eq!(second.getPointCount(), 0);
            changes.try_enqueue(100, 99, 0.9).unwrap();
            changes.try_enqueue(101, 98, 0.8).unwrap();
            assert_eq!(first.getParameterId(), 100);
            assert_eq!(second.getParameterId(), 101);
            assert_eq!(first.getPoint(0, &mut offset, ptr::null_mut()), kResultOk);
            assert_eq!(offset, 99);
            assert_eq!(second.getPoint(0, &mut offset, ptr::null_mut()), kResultOk);
            assert_eq!(offset, 98);
        });
        assert_eq!(changes.failure(), None);
    }

    #[test]
    fn invalid_read_probes_do_not_rebuild_shared_index() {
        let changes = ParameterChanges::new(limits(2, 2));
        allocation_free(|| unsafe {
            let raw = changes.addParameterData(&7, ptr::null_mut());
            let queue = ComRef::from_raw(raw).unwrap();
            assert_eq!(
                queue.getPoint(0, ptr::null_mut(), ptr::null_mut()),
                kResultFalse
            );
            assert!(!changes.arena.lock().unwrap().cache_dirty);
            changes.try_enqueue(7, 1, 0.1).unwrap();
            changes.try_enqueue(7, 0, 0.0).unwrap();
            // Exercise the supported dirty-state recovery path without relying on
            // normal insertions, which now preserve a valid ordinal index.
            changes.arena.lock().unwrap().cache_dirty = true;
            for index in [-1, 2, i32::MAX] {
                assert_eq!(
                    queue.getPoint(index, ptr::null_mut(), ptr::null_mut()),
                    kResultFalse
                );
                assert!(changes.arena.lock().unwrap().cache_dirty);
            }
            assert_eq!(
                queue.getPoint(0, ptr::null_mut(), ptr::null_mut()),
                kResultOk
            );
            assert!(!changes.arena.lock().unwrap().cache_dirty);
            assert_eq!(changes.failure(), None);
        });
    }

    #[test]
    fn final_queue_tail_appends_extend_a_valid_index_at_small_and_max_budgets() {
        for capacity in [32, 128, 512, 8192] {
            let changes = ParameterChanges::new(limits(4, capacity));
            allocation_free(|| unsafe {
                let first = changes.addParameterData(&7, ptr::null_mut());
                let first = ComRef::from_raw(first).unwrap();
                assert!(!changes.arena.lock().unwrap().cache_dirty);
                let mut checksum = 0.0;
                for index in 0..capacity {
                    assert_eq!(
                        first.addPoint(index as i32, index as f64 * 0.5, ptr::null_mut()),
                        kResultOk
                    );
                    assert!(!changes.arena.lock().unwrap().cache_dirty);
                    let read = (index * 4051) % (index + 1);
                    let mut offset = -1;
                    let mut value = -1.0;
                    assert_eq!(
                        first.getPoint(read as i32, &mut offset, &mut value),
                        kResultOk
                    );
                    assert_eq!((offset, value), (read as i32, read as f64 * 0.5));
                    checksum += offset as f64 + value;
                }
                assert!(checksum > 0.0);
                changes.clear_all().unwrap();
                // Reuse with several queues. A new empty last queue keeps an already
                // valid cache; its first and later tail appends extend that cache.
                for id in 0..3 {
                    changes.try_enqueue(id, 0, id as f64).unwrap();
                }
                let last = changes.addParameterData(&99, ptr::null_mut());
                let last = ComRef::from_raw(last).unwrap();
                for index in 0..capacity - 3 {
                    assert_eq!(
                        last.addPoint(index as i32, index as f64 * 0.5, ptr::null_mut()),
                        kResultOk
                    );
                    assert!(!changes.arena.lock().unwrap().cache_dirty);
                    let mut offset = -1;
                    let mut value = -1.0;
                    assert_eq!(
                        last.getPoint(index as i32, &mut offset, &mut value),
                        kResultOk
                    );
                    assert_eq!((offset, value), (index as i32, index as f64 * 0.5));
                }
                for slot in 0..3 {
                    let queue = queue(&changes, slot);
                    let mut value = -1.0;
                    assert_eq!(queue.getPoint(0, ptr::null_mut(), &mut value), kResultOk);
                    assert_eq!(value, slot as f64);
                }
            });
            assert_eq!(changes.failure(), None);
        }
    }

    #[test]
    fn valid_index_tracks_all_insertions_and_dirty_state_is_preserved() {
        let changes = ParameterChanges::new(limits(4, 12));
        allocation_free(|| unsafe {
            changes.try_enqueue(7, 2, 0.2).unwrap();
            changes.try_enqueue(7, 2, 0.3).unwrap();
            assert!(!changes.arena.lock().unwrap().cache_dirty);
            changes.try_enqueue(7, 4, 0.4).unwrap();
            changes.try_enqueue(7, 2, 0.5).unwrap(); // Equal-offset middle insertion.
            assert!(!changes.arena.lock().unwrap().cache_dirty);
            changes.arena.lock().unwrap().cache_dirty = true;
            let new = changes.addParameterData(&8, ptr::null_mut());
            let new = ComRef::from_raw(new).unwrap();
            assert!(changes.arena.lock().unwrap().cache_dirty);
            assert_eq!(new.addPoint(0, 0.6, ptr::null_mut()), kResultOk);
            assert!(
                changes.arena.lock().unwrap().cache_dirty,
                "append must not bless an already dirty cache"
            );
            let first = queue(&changes, 0);
            for (index, expected) in [(2, 0.2), (2, 0.3), (2, 0.5), (4, 0.4)]
                .into_iter()
                .enumerate()
            {
                let mut offset = -1;
                let mut value = -1.0;
                assert_eq!(
                    first.getPoint(index as i32, &mut offset, &mut value),
                    kResultOk
                );
                assert_eq!((offset, value), expected);
            }
            assert!(!changes.arena.lock().unwrap().cache_dirty);
            // A non-final queue tail append shifts the later queue's ordinal slice
            // while preserving validity of that slice's updated cache_start.
            assert_eq!(first.addPoint(5, 0.7, ptr::null_mut()), kResultOk);
            assert!(!changes.arena.lock().unwrap().cache_dirty);
            let mut value = -1.0;
            assert_eq!(new.getPoint(0, ptr::null_mut(), &mut value), kResultOk);
            assert_eq!(value, 0.6);
            assert!(!changes.arena.lock().unwrap().cache_dirty);
            changes.clear_all().unwrap();
            assert!(!changes.arena.lock().unwrap().cache_dirty);
            changes.try_enqueue(9, 0, 1.0).unwrap();
            assert!(!changes.arena.lock().unwrap().cache_dirty);
            assert_eq!(first.getPoint(0, ptr::null_mut(), &mut value), kResultOk);
            assert_eq!(value, 1.0);
        });
    }

    #[test]
    fn multiqueue_insertions_match_stable_oracle_through_capacity_reset_and_reuse() {
        for capacity in [32, 128, 512, 8192] {
            let changes = ParameterChanges::new(limits(8, capacity));
            let mut oracle: Vec<(usize, i32, f64)> = Vec::with_capacity(capacity);
            allocation_free(|| unsafe {
                for round in 0..2 {
                    changes.clear_all().unwrap();
                    oracle.clear();
                    // Include a permanently empty final queue, so every populated
                    // queue must correctly shift some later queue slice metadata.
                    for slot in 0..8 {
                        assert!(!changes
                            .addParameterData(&(100 + slot), ptr::null_mut())
                            .is_null());
                    }
                    for arrival in 0..capacity {
                        let slot = (arrival * 13) % 7;
                        let offset = ((arrival * 4051 + round * 31) % 97) as i32 - 40;
                        let value = (round * capacity + arrival) as f64;
                        let rank = oracle
                            .iter()
                            .filter(|(old_slot, old_offset, _)| {
                                *old_slot == slot && *old_offset <= offset
                            })
                            .count();
                        let insertion = oracle
                            .iter()
                            .position(|(old_slot, old_offset, _)| {
                                *old_slot > slot || (*old_slot == slot && *old_offset > offset)
                            })
                            .unwrap_or(oracle.len());
                        oracle.insert(insertion, (slot, offset, value));
                        let queue = queue(&changes, slot as i32);
                        let mut index = -1;
                        assert_eq!(queue.addPoint(offset, value, &mut index), kResultOk);
                        assert_eq!(index as usize, rank);
                        assert!(!changes.arena.lock().unwrap().cache_dirty);
                        let count = queue.getPointCount() as usize;
                        let read = (arrival * 4051) % count;
                        let expected = oracle
                            .iter()
                            .filter(|(old_slot, _, _)| *old_slot == slot)
                            .nth(read)
                            .unwrap();
                        let mut got_offset = -1;
                        let mut got_value = -1.0;
                        assert_eq!(
                            queue.getPoint(read as i32, &mut got_offset, &mut got_value),
                            kResultOk
                        );
                        assert_eq!((got_offset, got_value), (expected.1, expected.2));
                    }
                    let mut position = 0;
                    changes
                        .for_each_active_point(|id, offset, value| {
                            let expected = oracle[position];
                            assert_eq!(
                                (id, offset, value),
                                (100 + expected.0 as u32, expected.1, expected.2)
                            );
                            position += 1;
                        })
                        .unwrap();
                    assert_eq!(position, capacity);
                    let mut index = 99;
                    let empty = queue(&changes, 7);
                    assert_eq!(empty.addPoint(0, 9.0, &mut index), kResultFalse);
                    assert_eq!(index, -1);
                    assert_eq!(empty.getPointCount(), 0);
                    assert_eq!(
                        changes.failure(),
                        Some(ParameterStorageError::PointCapacity)
                    );
                    // A failed full-budget insertion cannot shift any accepted index.
                    let mut checksum = 0.0;
                    for slot in 0..8 {
                        let queue = queue(&changes, slot);
                        for index in 0..queue.getPointCount() {
                            let mut offset = -1;
                            let mut value = -1.0;
                            assert_eq!(queue.getPoint(index, &mut offset, &mut value), kResultOk);
                            checksum += offset as f64 + value;
                        }
                    }
                    assert_eq!(
                        checksum,
                        oracle
                            .iter()
                            .map(|(_, offset, value)| *offset as f64 + value)
                            .sum::<f64>()
                    );
                }
            });
        }
    }

    #[test]
    fn indexed_upper_bound_preserves_ties_and_extreme_offsets_in_valid_and_dirty_states() {
        for dirty in [false, true] {
            let changes = ParameterChanges::new(limits(2, 10));
            allocation_free(|| unsafe {
                let raw = changes.addParameterData(&7, ptr::null_mut());
                let queue = ComRef::from_raw(raw).unwrap();
                let arrivals = [
                    (0, 0.0),
                    (i32::MAX, 1.0),
                    (i32::MIN, 2.0),
                    (-1, 3.0),
                    (0, 4.0),
                    (i32::MIN, 5.0),
                    (i32::MAX, 6.0),
                    (-1, 7.0),
                    (1, 8.0),
                    (0, 9.0),
                ];
                let expected_ranks = [0, 1, 0, 1, 3, 1, 6, 3, 6, 6];
                for ((offset, value), expected_rank) in arrivals.into_iter().zip(expected_ranks) {
                    if dirty {
                        changes.arena.lock().unwrap().cache_dirty = true;
                    }
                    let mut index = -1;
                    assert_eq!(queue.addPoint(offset, value, &mut index), kResultOk);
                    assert_eq!(index, expected_rank);
                    assert_eq!(changes.arena.lock().unwrap().cache_dirty, dirty);
                }
                let expected = [
                    (i32::MIN, 2.0),
                    (i32::MIN, 5.0),
                    (-1, 3.0),
                    (-1, 7.0),
                    (0, 0.0),
                    (0, 4.0),
                    (0, 9.0),
                    (1, 8.0),
                    (i32::MAX, 1.0),
                    (i32::MAX, 6.0),
                ];
                for (index, point) in expected.into_iter().enumerate() {
                    let mut offset = 0;
                    let mut value = 0.0;
                    assert_eq!(
                        queue.getPoint(index as i32, &mut offset, &mut value),
                        kResultOk
                    );
                    assert_eq!((offset, value), point);
                }
                assert!(!changes.arena.lock().unwrap().cache_dirty);
                let mut index = 99;
                assert_eq!(queue.addPoint(i32::MIN, 99.0, &mut index), kResultFalse);
                assert_eq!(index, -1);
                assert_eq!(queue.getPointCount(), 10);
                assert!(!changes.arena.lock().unwrap().cache_dirty);
                let empty = changes.addParameterData(&8, &mut index);
                let empty = ComRef::from_raw(empty).unwrap();
                assert_eq!(empty.getPointCount(), 0);
                assert_eq!(empty.addPoint(0, 99.0, &mut index), kResultFalse);
                assert_eq!(index, -1);
            });
        }
    }

    #[test]
    fn corrupt_cache_ranges_and_nodes_fail_closed_before_insertion_mutation() {
        for corruption in 0..5 {
            let changes = ParameterChanges::new(limits(2, 8));
            for offset in [10, 20, 30] {
                changes.try_enqueue(7, offset, offset as f64).unwrap();
            }
            changes.try_enqueue(8, 0, 0.8).unwrap();
            let first = unsafe { changes.getParameterData(0) };
            {
                let mut arena = changes.arena.lock().unwrap();
                match corruption {
                    0 => arena.queues[0].cache_start = usize::MAX,
                    1 => arena.queues[0].cache_start = arena.used_points,
                    2 => arena.ordinal_index[1] = usize::MAX,
                    3 => arena.ordinal_index[1] = arena.used_points,
                    _ => arena.queues[1].cache_start = usize::MAX,
                }
            }
            allocation_free(|| unsafe {
                let first = ComRef::from_raw(first).unwrap();
                if corruption < 4 {
                    let mut offset = 99;
                    let mut value = 99.0;
                    assert_eq!(first.getPoint(1, &mut offset, &mut value), kResultFalse);
                    assert_eq!((offset, value), (99, 99.0));
                }
                let mut index = 99;
                assert_eq!(first.addPoint(15, 0.15, &mut index), kResultFalse);
                assert_eq!(index, -1);
                assert_eq!(changes.failure(), Some(ParameterStorageError::InvalidState));
                let arena = changes.arena.lock().unwrap();
                assert_eq!(arena.used_points, 4);
                assert_eq!(arena.used_queues, 2);
                assert_eq!(arena.queues[0].count, 3);
                assert_eq!(arena.queues[0].head, 0);
                assert_eq!(arena.queues[0].tail, 2);
                for node in 0..3 {
                    assert_eq!(arena.points[node].offset, (node as i32 + 1) * 10);
                    assert_eq!(arena.points[node].value, (node as f64 + 1.0) * 10.0);
                    assert_eq!(
                        arena.points[node].next,
                        if node < 2 {
                            node + 1
                        } else {
                            NO_PARAMETER_NODE
                        }
                    );
                }
            });
            assert_eq!(
                points(&changes),
                [(7, 10, 10.0), (7, 20, 20.0), (7, 30, 30.0), (8, 0, 0.8)]
            );
        }
    }

    #[test]
    fn corrupt_dirty_chain_is_rejected_before_index_rebuild_mutation() {
        let changes = ParameterChanges::new(limits(1, 4));
        changes.try_enqueue(7, 0, 0.0).unwrap();
        changes.try_enqueue(7, 1, 0.1).unwrap();
        let raw = unsafe { changes.getParameterData(0) };
        {
            let mut arena = changes.arena.lock().unwrap();
            arena.cache_dirty = true;
            arena.points[0].next = usize::MAX - 1;
            arena.ordinal_index[0] = 3; // A sentinel proving no partial rebuild writes.
        }
        allocation_free(|| unsafe {
            let queue = ComRef::from_raw(raw).unwrap();
            assert_eq!(
                queue.getPoint(0, ptr::null_mut(), ptr::null_mut()),
                kResultFalse
            );
            assert_eq!(changes.failure(), Some(ParameterStorageError::InvalidState));
            let arena = changes.arena.lock().unwrap();
            assert!(arena.cache_dirty);
            assert_eq!(arena.ordinal_index[0], 3);
            assert_eq!(arena.used_points, 2);
        });
    }

    #[test]
    fn invalid_counts_and_linked_traversal_fail_closed_in_readers() {
        let counts = ParameterChanges::new(limits(1, 2));
        counts.try_enqueue(7, 0, 0.1).unwrap();
        counts.arena.lock().unwrap().used_queues = 2;
        allocation_free(|| unsafe {
            assert_eq!(counts.getParameterCount(), 0);
            assert!(counts.getParameterData(0).is_null());
            assert_eq!(
                counts.for_each_active_point(|_, _, _| panic!("invalid count visitor")),
                Err(ParameterStorageError::InvalidState)
            );
            assert_eq!(counts.failure(), Some(ParameterStorageError::InvalidState));
        });
        let chain = ParameterChanges::new(limits(1, 2));
        chain.try_enqueue(7, 0, 0.0).unwrap();
        chain.try_enqueue(7, 1, 0.1).unwrap();
        chain.arena.lock().unwrap().points[0].next = usize::MAX - 1;
        allocation_free(|| {
            let mut visited = 0;
            assert_eq!(
                chain.for_each_active_point(|_, _, _| visited += 1),
                Err(ParameterStorageError::InvalidState)
            );
            assert_eq!(visited, 1);
            assert_eq!(chain.failure(), Some(ParameterStorageError::InvalidState));
        });
    }

    #[test]
    fn sparse_first_points_follow_registered_order_and_ignore_empty_offsets() {
        let changes = ParameterChanges::new(limits(8, 24));
        let ids = [80, 10, 70, 20, 60, 30, 50, 40];
        allocation_free(|| unsafe {
            for round in 0..2 {
                changes.clear_all().unwrap();
                for id in ids {
                    assert!(!changes
                        .addParameterData(&(id + round * 100), ptr::null_mut())
                        .is_null());
                }
                assert_eq!(changes.getParameterCount(), 8);
                {
                    let arena = changes.arena.lock().unwrap();
                    assert_eq!(arena.used_populated, 0);
                    assert!(arena
                        .queues
                        .iter()
                        .all(|queue| queue.cache_start == NO_PARAMETER_NODE));
                }
                let mut expected_slots = [0_usize; 8];
                for (step, slot) in [7_usize, 0, 5, 2, 6, 1, 4, 3].into_iter().enumerate() {
                    let queue = queue(&changes, slot as i32);
                    assert_eq!(queue.addPoint(10, slot as f64, ptr::null_mut()), kResultOk);
                    expected_slots[step] = slot;
                    expected_slots[..=step].sort_unstable();
                    let arena = changes.arena.lock().unwrap();
                    assert_eq!(arena.used_populated, step + 1);
                    assert_eq!(&arena.populated_slots[..=step], &expected_slots[..=step]);
                    for candidate in 0..8 {
                        if !expected_slots[..=step].contains(&candidate) {
                            assert_eq!(arena.queues[candidate].cache_start, NO_PARAMETER_NODE);
                            assert_eq!(arena.queues[candidate].count, 0);
                        }
                    }
                }
                for slot in 0..8 {
                    let queue = queue(&changes, slot);
                    assert_eq!(queue.getParameterId(), ids[slot as usize] + round * 100);
                    assert_eq!(
                        queue.addPoint(0, slot as f64 + 0.25, ptr::null_mut()),
                        kResultOk
                    );
                    assert_eq!(
                        queue.addPoint(10, slot as f64 + 0.5, ptr::null_mut()),
                        kResultOk
                    );
                }
                let mut seen = 0;
                changes
                    .for_each_active_point(|id, offset, value| {
                        let slot = seen / 3;
                        assert_eq!(id, ids[slot] + round * 100);
                        let expected = [
                            (0, slot as f64 + 0.25),
                            (10, slot as f64),
                            (10, slot as f64 + 0.5),
                        ][seen % 3];
                        assert_eq!((offset, value), expected);
                        seen += 1;
                    })
                    .unwrap();
                assert_eq!(seen, 24);
                let mut index = 99;
                assert_eq!(
                    queue(&changes, 0).addPoint(-1, 99.0, &mut index),
                    kResultFalse
                );
                assert_eq!(index, -1);
                assert_eq!(changes.arena.lock().unwrap().used_populated, 8);
            }
        });
    }

    #[test]
    fn sparse_maximum_empty_queue_sets_admit_early_edits_without_allocating() {
        for count in [4096, 8192] {
            let changes = ParameterChanges::new(limits(count, count));
            allocation_free(|| unsafe {
                for slot in 0..count {
                    assert!(!changes
                        .addParameterData(&(slot as u32), ptr::null_mut())
                        .is_null());
                }
                let first = queue(&changes, 0);
                let mut checksum = 0.0;
                for index in 0..128 {
                    assert_eq!(
                        first.addPoint(index, index as f64 * 0.5, ptr::null_mut()),
                        kResultOk
                    );
                    let mut offset = -1;
                    let mut value = -1.0;
                    assert_eq!(first.getPoint(index, &mut offset, &mut value), kResultOk);
                    checksum += offset as f64 + value;
                }
                assert_eq!(checksum, 1.5 * (127 * 128 / 2) as f64);
                let arena = changes.arena.lock().unwrap();
                assert_eq!(arena.used_queues, count);
                assert_eq!(arena.used_populated, 1);
                assert_eq!(arena.populated_slots[0], 0);
                assert!(arena.queues[1..]
                    .iter()
                    .all(|queue| queue.count == 0 && queue.cache_start == NO_PARAMETER_NODE));
            });
        }
    }

    #[test]
    fn sparse_corruption_rejects_before_any_point_index_or_list_mutation() {
        for corruption in 0..6 {
            let changes = ParameterChanges::new(limits(5, 8));
            for id in 0..5 {
                unsafe {
                    changes.addParameterData(&id, ptr::null_mut());
                }
            }
            for slot in [1, 3] {
                unsafe {
                    queue(&changes, slot).addPoint(0, slot as f64, ptr::null_mut());
                }
            }
            {
                let mut arena = changes.arena.lock().unwrap();
                match corruption {
                    0 => arena.populated_slots[1] = usize::MAX,
                    1 => arena.populated_slots[1] = 4, // Registered, but empty.
                    2 => arena.populated_slots[1] = 1, // Duplicate.
                    3 => arena.populated_slots[..2].swap(0, 1), // Unsorted.
                    4 => arena.used_populated = 1,     // Missing nonempty target.
                    _ => arena.used_populated = 9,
                }
            }
            allocation_free(|| unsafe {
                let mut index = 99;
                let slot = if corruption == 4 { 3 } else { 0 };
                let queue = changes.queues[slot]
                    .as_com_ref::<IParamValueQueue>()
                    .unwrap();
                assert_eq!(queue.addPoint(1, 99.0, &mut index), kResultFalse);
                assert_eq!(index, -1);
                assert_eq!(changes.failure(), Some(ParameterStorageError::InvalidState));
                let arena = changes.arena.lock().unwrap();
                assert_eq!(arena.used_points, 2);
                assert_eq!(arena.used_queues, 5);
                assert_eq!(arena.queues[0].count, 0);
                assert_eq!(arena.queues[1].count, 1);
                assert_eq!(arena.queues[3].count, 1);
                assert_eq!(arena.ordinal_index[0], 0);
                assert_eq!(arena.ordinal_index[1], 1);
                assert_eq!((arena.points[0].value, arena.points[1].value), (1.0, 3.0));
            });
        }
    }

    #[test]
    fn sparse_checked_host_failure_restores_exact_recycled_metadata() {
        let changes = ParameterChanges::new(limits(3, 8));
        for id in [70, 80, 90] {
            changes.try_enqueue(id, 4, id as f64).unwrap();
        }
        let retained_slot = unsafe { changes.getParameterData(1) };
        changes.clear_all().unwrap();
        changes.try_enqueue(7, 0, 0.7).unwrap();
        let before = {
            let mut arena = changes.arena.lock().unwrap();
            let before = arena.queues[1];
            arena.populated_slots[0] = usize::MAX;
            before
        };
        allocation_free(|| unsafe {
            assert_eq!(
                changes.try_enqueue(999, 0, 0.9),
                Err(ParameterStorageError::InvalidState)
            );
            assert_eq!(changes.failure(), Some(ParameterStorageError::InvalidState));
            assert_eq!(changes.getParameterCount(), 1);
            assert_eq!(
                ComRef::from_raw(retained_slot).unwrap().getParameterId(),
                80
            );
            let arena = changes.arena.lock().unwrap();
            let after = arena.queues[1];
            assert_eq!(
                (
                    after.id,
                    after.head,
                    after.tail,
                    after.count,
                    after.cache_start
                ),
                (
                    before.id,
                    before.head,
                    before.tail,
                    before.count,
                    before.cache_start
                )
            );
            assert_eq!(arena.used_queues, 1);
            assert_eq!(arena.used_points, 1);
            assert_eq!(arena.used_populated, 1);
            assert_eq!(arena.queues[0].id, 7);
            assert_eq!(arena.queues[0].count, 1);
            assert_eq!(arena.points[0].value, 0.7);
            assert_eq!(arena.ordinal_index[0], 0);
            assert_eq!(arena.populated_slots[0], usize::MAX);
        });
    }

    #[test]
    fn randomized_insertions_match_a_stable_sorted_oracle() {
        let changes = ParameterChanges::new(limits(1, 512));
        allocation_free(|| unsafe {
            let raw = changes.addParameterData(&7, ptr::null_mut());
            let queue = ComRef::from_raw(raw).unwrap();
            let mut oracle = [(0_i32, 0.0_f64); 512];
            for arrival in 0..512 {
                let offset = ((arrival * 4051) % 97) as i32 - 40;
                let value = arrival as f64;
                let insertion = oracle[..arrival]
                    .iter()
                    .position(|(old, _)| *old > offset)
                    .unwrap_or(arrival);
                for index in (insertion..arrival).rev() {
                    oracle[index + 1] = oracle[index];
                }
                oracle[insertion] = (offset, value);
                let mut index = -1;
                assert_eq!(queue.addPoint(offset, value, &mut index), kResultOk);
                assert_eq!(index as usize, insertion);
                // Read during construction to stress shared-index updates on every
                // subsequent middle/head/tail insertion, not just after one edit.
                let read = arrival / 2;
                let mut got_offset = 0;
                let mut got_value = 0.0;
                assert_eq!(
                    queue.getPoint(read as i32, &mut got_offset, &mut got_value),
                    kResultOk
                );
                assert_eq!((got_offset, got_value), oracle[read]);
            }
            for (index, expected) in oracle.into_iter().enumerate() {
                let mut offset = 0;
                let mut value = 0.0;
                assert_eq!(
                    queue.getPoint(index as i32, &mut offset, &mut value),
                    kResultOk
                );
                assert_eq!((offset, value), expected);
            }
        });
    }

    #[test]
    fn reused_slots_have_stable_borrowed_pointers_and_fresh_points() {
        let changes = ParameterChanges::new(limits(2, 2));
        allocation_free(|| unsafe {
            let first = changes.addParameterData(&7, ptr::null_mut());
            for round in 0..128 {
                let queue = ComRef::from_raw(first).unwrap();
                assert_eq!(queue.addPoint(0, round as f64, ptr::null_mut()), kResultOk);
                assert_eq!(queue.getPointCount(), 1);
                assert_eq!(changes.getParameterData(0), first);
                // Borrowed getter and add calls must not leak one reference per call.
                assert_eq!(IParamValueQueue::add_ref(first), 2);
                assert_eq!(IParamValueQueue::release(first), 1);
                changes.clear_all().unwrap();
                assert_eq!(queue.getPointCount(), 0);
                assert_eq!(changes.getParameterCount(), 0);
                assert_eq!(
                    changes.addParameterData(&(100 + round), ptr::null_mut()),
                    first
                );
                assert_eq!(queue.getPointCount(), 0);
                assert_eq!(queue.getParameterId(), 100 + round);
            }
        });
        assert_eq!(changes.failure(), None);
    }

    #[test]
    fn retained_queue_and_qi_survive_parent_destruction_without_cycle() {
        let changes = ParameterChanges::new(limits(2, 3));
        let arena = Arc::downgrade(&changes.arena);
        let raw = unsafe { changes.addParameterData(&7, ptr::null_mut()) };
        let retained = unsafe {
            // Explicit AddRef, unlike the borrowed return above.
            assert_eq!(IParamValueQueue::add_ref(raw), 2);
            ComPtr::from_raw(raw).unwrap()
        };
        unsafe {
            assert_eq!(retained.addPoint(9, 0.75, ptr::null_mut()), kResultOk);
        }
        let unknown = retained.cast::<FUnknown>().unwrap();
        let queried = unknown.cast::<IParamValueQueue>().unwrap();
        assert_eq!(queried.as_ptr(), raw);
        assert_eq!(retained.cast::<IParameterChanges>().map(|_| ()), None);
        drop(changes);
        assert!(arena.upgrade().is_some());
        allocation_free(|| unsafe {
            assert_eq!(retained.getParameterId(), 7);
            assert_eq!(retained.getPointCount(), 1);
            let mut value = 0.0;
            assert_eq!(retained.getPoint(0, ptr::null_mut(), &mut value), kResultOk);
            assert_eq!(value, 0.75);
            assert_eq!(retained.addPoint(10, 0.9, ptr::null_mut()), kResultOk);
            assert_eq!(queried.getPointCount(), 2);
        });
        drop(queried);
        drop(unknown);
        drop(retained);
        assert!(
            arena.upgrade().is_none(),
            "retained queue must not form an ownership cycle"
        );
    }

    #[test]
    fn inactive_retained_queue_write_fails_without_touching_reused_points() {
        let changes = ParameterChanges::new(limits(2, 3));
        let first = unsafe { changes.addParameterData(&7, ptr::null_mut()) };
        let second = unsafe { changes.addParameterData(&8, ptr::null_mut()) };
        allocation_free(|| unsafe {
            changes.clear_all().unwrap();
            assert_eq!(changes.addParameterData(&9, ptr::null_mut()), first);
            let stale = ComRef::from_raw(second).unwrap();
            let mut index = 99;
            assert_eq!(stale.addPoint(0, 1.0, &mut index), kResultFalse);
            assert_eq!(index, -1);
            assert_eq!(stale.getPointCount(), 0);
            assert_eq!(changes.getParameterCount(), 1);
            assert_eq!(
                changes.failure(),
                Some(ParameterStorageError::InactiveQueue)
            );
            changes.try_enqueue(9, 0, 0.5).unwrap();
        });
        assert_eq!(points(&changes), [(9, 0, 0.5)]);
    }

    #[test]
    fn poisoned_storage_fails_all_entry_points_without_callback_panics() {
        let changes = ParameterChanges::new(limits(2, 2));
        changes.try_enqueue(7, 0, 0.5).unwrap();
        let raw = unsafe { changes.getParameterData(0) };
        changes.poison_for_test();
        allocation_free(|| unsafe {
            let queue = ComRef::from_raw(raw).unwrap();
            let mut index = 99;
            assert_eq!(
                changes.try_enqueue(8, 0, 0.1),
                Err(ParameterStorageError::Poisoned)
            );
            assert_eq!(changes.clear_all(), Err(ParameterStorageError::Poisoned));
            assert_eq!(
                changes.for_each_active_point(|_, _, _| panic!("poisoned visitor")),
                Err(ParameterStorageError::Poisoned)
            );
            assert_eq!(changes.getParameterCount(), 0);
            assert!(changes.getParameterData(0).is_null());
            assert!(changes.addParameterData(&8, &mut index).is_null());
            assert_eq!(index, -1);
            assert_eq!(queue.getParameterId(), u32::MAX);
            assert_eq!(queue.getPointCount(), 0);
            assert_eq!(
                queue.getPoint(0, ptr::null_mut(), ptr::null_mut()),
                kResultFalse
            );
            index = 99;
            assert_eq!(queue.addPoint(0, 0.9, &mut index), kResultFalse);
            assert_eq!(index, -1);
            assert_eq!(changes.failure(), Some(ParameterStorageError::Poisoned));
            assert_eq!(changes.failure(), Some(ParameterStorageError::Poisoned));
        });
    }

    #[test]
    fn constructor_allocations_are_linear_and_dense_timings_are_measured() {
        for limits in [ParameterQueueLimits::INPUT, ParameterQueueLimits::OUTPUT] {
            let (changes, stats) = measure_allocations(|| ParameterChanges::new(limits));
            assert_eq!(stats.allocations, limits.max_queues + 6);
            assert_eq!(stats.deallocations, 0);
            assert_eq!(stats.freed_bytes, 0);
            assert_eq!(changes.queues.len(), limits.max_queues);
            {
                let arena = changes.arena.lock().unwrap();
                assert_eq!(arena.queues.len(), limits.max_queues);
                assert_eq!(arena.points.len(), limits.max_points);
                assert_eq!(arena.ordinal_index.len(), limits.max_points);
                assert_eq!(arena.populated_slots.len(), limits.max_queues);
            }
            let start = Instant::now();
            allocation_free(|| {
                for index in 0..limits.max_points {
                    changes.try_enqueue(7, index as i32, index as f64).unwrap();
                }
            });
            let ascending_write = start.elapsed();
            let start = Instant::now();
            allocation_free(|| unsafe {
                let queue = queue(&changes, 0);
                for index in 0..limits.max_points {
                    assert_eq!(
                        queue.getPoint(index as i32, ptr::null_mut(), ptr::null_mut()),
                        kResultOk
                    );
                }
            });
            let sequential_read = start.elapsed();
            let start = Instant::now();
            allocation_free(|| unsafe {
                let queue = queue(&changes, 0);
                // Odd multiplier gives a deterministic permutation for the power-of-two
                // production budgets, and repeatedly jumps backwards through the curve.
                for index in 0..limits.max_points {
                    let index = (index * 4051) % limits.max_points;
                    assert_eq!(
                        queue.getPoint(index as i32, ptr::null_mut(), ptr::null_mut()),
                        kResultOk
                    );
                }
            });
            let random_read = start.elapsed();
            let start = Instant::now();
            allocation_free(|| changes.clear_all().unwrap());
            let reset = start.elapsed();
            let start = Instant::now();
            allocation_free(|| {
                for index in (0..limits.max_points).rev() {
                    changes.try_enqueue(7, index as i32, index as f64).unwrap();
                }
            });
            let descending_write = start.elapsed();
            allocation_free(|| changes.clear_all().unwrap());
            let start = Instant::now();
            allocation_free(|| {
                for index in 0..limits.max_points {
                    let offset = (index * 4051) % limits.max_points;
                    changes.try_enqueue(7, offset as i32, index as f64).unwrap();
                }
            });
            let random_write = start.elapsed();
            allocation_free(|| changes.clear_all().unwrap());
            let start = Instant::now();
            allocation_free(|| {
                for index in 0..limits.max_queues {
                    changes.try_enqueue(index as u32, 0, index as f64).unwrap();
                }
            });
            let distinct_id_write = start.elapsed();
            eprintln!("parameter storage {:?}: allocations={}, requested_bytes={}, deallocations={}, queue_slot_bytes={}, point_node_bytes={}, ascending_write={:?}, descending_write={:?}, random_write={:?}, sequential_read={:?}, random_read={:?}, distinct_id_write={:?}, reset={:?}; profile={}, arch={}, os={}; observed test timings, not realtime latency guarantees", limits, stats.allocations, stats.allocated_bytes, stats.deallocations, std::mem::size_of::<ParameterQueueSlot>(), std::mem::size_of::<ParameterPointNode>(), ascending_write, descending_write, random_write, sequential_read, random_read, distinct_id_write, reset, if cfg!(debug_assertions) { "debug" } else { "optimized" }, std::env::consts::ARCH, std::env::consts::OS);
        }
    }
}

#[cfg(test)]
mod memory_stream_tests {
    use super::*;

    #[test]
    fn write_then_read_round_trips_from_start() {
        let s = MemoryStream::new(Vec::new());
        assert_eq!(s.write_at_cursor(&[1, 2, 3, 4]), Some(4));
        assert_eq!(s.position(), 4);
        assert_eq!(s.to_vec(), vec![1, 2, 3, 4]);

        // Rewind (mode 0 = SEEK_SET) and read it all back.
        assert_eq!(s.seek_to(0, 0), Some(0));
        assert_eq!(s.read_at_cursor(4), vec![1, 2, 3, 4]);
    }

    #[test]
    fn read_past_end_is_clamped() {
        let s = MemoryStream::new(vec![9, 8]);
        assert_eq!(s.read_at_cursor(10), vec![9, 8]);
        // Cursor now at end; further reads yield nothing.
        assert_eq!(s.read_at_cursor(10), Vec::<u8>::new());
    }

    #[test]
    fn seek_modes_and_overwrite() {
        let s = MemoryStream::new(vec![0, 0, 0, 0]);
        // SEEK_END then write appends.
        assert_eq!(s.seek_to(0, SEEK_END), Some(4));
        s.write_at_cursor(&[5]);
        assert_eq!(s.to_vec(), vec![0, 0, 0, 0, 5]);
        // SEEK_SET to 1 then overwrite in place.
        assert_eq!(s.seek_to(1, 0), Some(1));
        s.write_at_cursor(&[7, 7]);
        assert_eq!(s.to_vec(), vec![0, 7, 7, 0, 5]);
        // SEEK_CUR is relative, and a seek before the start clamps to it.
        assert_eq!(s.seek_to(-3, SEEK_CUR), Some(0));
        assert_eq!(s.seek_to(-100, SEEK_CUR), Some(0));
    }

    /// The cursor and the write length both come from the plugin, and the buffer grows to
    /// `cursor + length`. A wild seek followed by any write must not turn into a multi-gigabyte
    /// allocation — that panics on capacity overflow inside a vtable thunk, which aborts the
    /// process instead of unwinding.
    #[test]
    fn huge_seek_then_write_is_refused_instead_of_allocating() {
        let s = MemoryStream::new(Vec::new());

        // Past the cap: the seek itself is refused, so the cursor never gets there.
        assert_eq!(s.seek_to(i64::MAX / 2, 0), None);
        assert_eq!(s.position(), 0);
        assert_eq!(s.seek_to(MAX_STREAM_BYTES as i64 + 1, 0), None);
        assert_eq!(s.position(), 0);

        // At the cap the seek is allowed, but the write that would grow past it is not, and
        // the stream is left untouched.
        assert_eq!(
            s.seek_to(MAX_STREAM_BYTES as i64, 0),
            Some(MAX_STREAM_BYTES as i64)
        );
        assert_eq!(s.write_at_cursor(&[1]), None);
        assert!(s.to_vec().is_empty());
    }

    /// The same refusal through the COM vtable, which is where a real plugin arrives: a result
    /// code and a zero byte count, never a panic.
    #[test]
    fn com_write_past_the_cap_reports_an_error_code() {
        let s = MemoryStream::new(Vec::new());
        let mut byte = 0u8;
        let mut written: i32 = -1;

        unsafe {
            // A seek beyond the cap is rejected outright...
            let mut pos: i64 = -1;
            assert_eq!(
                s.seek(MAX_STREAM_BYTES as i64 * 4, 0, &mut pos),
                kInvalidArgument
            );

            // ...and a write from a legal cursor that would cross it fails cleanly.
            assert_eq!(s.seek(MAX_STREAM_BYTES as i64, 0, &mut pos), kResultOk);
            assert_eq!(pos, MAX_STREAM_BYTES as i64);
            let result = s.write(
                &mut byte as *mut u8 as *mut std::ffi::c_void,
                1,
                &mut written,
            );
            assert_eq!(result, kOutOfMemory);
            assert_eq!(written, 0);
        }
        assert!(s.to_vec().is_empty());
    }

    /// Growth is bounded like every other host-side buffer: a plugin that keeps writing hits
    /// the cap and is told so, rather than growing the host's memory without limit.
    #[test]
    fn total_size_is_capped() {
        let s = MemoryStream::new(Vec::new());
        // Land one byte short of the cap, then write two.
        assert_eq!(
            s.seek_to(MAX_STREAM_BYTES as i64 - 1, 0),
            Some(MAX_STREAM_BYTES as i64 - 1)
        );
        assert_eq!(s.write_at_cursor(&[1]), Some(1));
        assert_eq!(s.to_vec().len(), MAX_STREAM_BYTES);
        assert_eq!(s.write_at_cursor(&[2]), None);
        assert_eq!(s.to_vec().len(), MAX_STREAM_BYTES);
    }

    /// Read a UTF-16 attribute back the way a plugin would, or `None` when the stream does
    /// not carry it.
    fn read_attribute(stream: &MemoryStream, key: &CStr) -> Option<String> {
        unsafe {
            let attributes = ComRef::<IAttributeList>::from_raw(stream.getAttributes())
                .expect("MemoryStream must vend its owned attribute list");
            let mut buf = [0u16; 512];
            if attributes.getString(
                key.as_ptr(),
                buf.as_mut_ptr(),
                std::mem::size_of_val(&buf) as u32,
            ) != kResultOk
            {
                return None;
            }
            let end = buf.iter().position(|unit| *unit == 0).unwrap_or(buf.len());
            Some(String::from_utf16_lossy(&buf[..end]))
        }
    }

    #[test]
    fn stream_attributes_expose_bounded_filename_and_state_type() {
        let long_name = "x".repeat(300);
        let stream = MemoryStream::with_metadata(
            vec![1, 2, 3],
            StreamMetadata {
                file_name: Some(&long_name),
                ..StreamMetadata::new(StreamStateType::Project)
            },
        );
        unsafe {
            let mut name: String128 = [0; 128];
            assert_eq!(stream.getFileName(&mut name), kResultOk);
            assert_eq!(name[..MAX_STREAM_FILENAME_UNITS], [u16::from(b'x'); 127]);
            assert_eq!(name[MAX_STREAM_FILENAME_UNITS], 0);
        }
        assert_eq!(
            read_attribute(&stream, c"StateType").as_deref(),
            Some("Project")
        );
        // No source file was named, so the path attribute must be absent rather than empty.
        assert_eq!(read_attribute(&stream, c"FilePathString"), None);
    }

    /// A preset restore tells the plugin both that the bytes came from a preset and which
    /// file they came from; a project restore tells it neither.
    #[test]
    fn a_restore_stream_publishes_the_context_it_was_built_from() {
        let preset = create_state_restore_stream(
            vec![7],
            &StateContext::preset_from_path("/Users/me/Presets/Big Lead.vstpreset"),
        );
        assert_eq!(
            read_attribute(&preset, c"StateType").as_deref(),
            Some("TrackPreset")
        );
        assert_eq!(
            read_attribute(&preset, c"FilePathString").as_deref(),
            Some("/Users/me/Presets/Big Lead.vstpreset")
        );
        unsafe {
            let mut name: String128 = [0; 128];
            assert_eq!(preset.getFileName(&mut name), kResultOk);
            let end = name
                .iter()
                .position(|unit| *unit == 0)
                .unwrap_or(name.len());
            assert_eq!(String::from_utf16_lossy(&name[..end]), "Big Lead");
        }

        let pathless = create_state_restore_stream(vec![7], &StateContext::preset());
        assert_eq!(
            read_attribute(&pathless, c"StateType").as_deref(),
            Some("TrackPreset")
        );
        assert_eq!(read_attribute(&pathless, c"FilePathString"), None);

        let project = create_state_restore_stream(vec![7], &StateContext::Project);
        assert_eq!(
            read_attribute(&project, c"StateType").as_deref(),
            Some("Project")
        );
        assert_eq!(read_attribute(&project, c"FilePathString"), None);
    }
}

#[cfg(test)]
mod output_event_loss_tests {
    use super::*;

    #[test]
    fn sdk_output_rejection_is_latched_across_clear_until_taken() {
        let list = HostEventList::new();
        let event = PluginEvent::from(crate::midi::MidiEvent::NoteOff {
            channel: crate::midi::MidiChannel::Ch1,
            note: 60,
            velocity: 0,
        });
        for _ in 0..MAX_QUEUED_EVENTS + 1 {
            let _ = list.try_add_event(event.clone());
        }
        list.clear();
        assert!(list.take_loss());
        assert!(!list.take_loss());
        unsafe {
            assert_eq!(list.addEvent(std::ptr::null_mut()), kResultFalse);
        }
        assert!(list.take_loss());
    }
}

#[cfg(all(test, target_os = "linux"))]
mod linux_run_loop_lifecycle_tests {
    use super::*;
    use vst3::Steinberg::Linux::*;

    struct Probe {
        registry: Arc<Mutex<RunLoopRegistry>>,
        dropped: Arc<AtomicBool>,
        hits: Arc<AtomicUsize>,
    }
    impl Class for Probe {
        type Interfaces = (IEventHandler, ITimerHandler);
    }
    impl IEventHandlerTrait for Probe {
        unsafe fn onFDIsSet(&self, _: FileDescriptor) {
            self.hits.fetch_add(1, Ordering::SeqCst);
        }
    }
    impl ITimerHandlerTrait for Probe {
        unsafe fn onTimer(&self) {
            assert!(self.registry.try_lock().is_ok());
            self.hits.fetch_add(1, Ordering::SeqCst);
        }
    }
    impl Drop for Probe {
        fn drop(&mut self) {
            assert!(
                self.registry.try_lock().is_ok(),
                "plugin release under registry lock"
            );
            self.dropped.store(true, Ordering::SeqCst);
        }
    }
    fn probe(
        registry: &Arc<Mutex<RunLoopRegistry>>,
    ) -> (ComWrapper<Probe>, Arc<AtomicBool>, Arc<AtomicUsize>) {
        let dropped = Arc::new(AtomicBool::new(false));
        let hits = Arc::new(AtomicUsize::new(0));
        (
            ComWrapper::new(Probe {
                registry: registry.clone(),
                dropped: dropped.clone(),
                hits: hits.clone(),
            }),
            dropped,
            hits,
        )
    }
    #[test]
    fn factory_context_exposes_a_separate_run_loop() {
        let host = create_host_application();
        let factory_loop = host
            .to_com_ptr::<IHostApplication>()
            .unwrap()
            .cast::<IRunLoop>()
            .expect("Factory3 context must expose IRunLoop");
        let frame_registry = Arc::new(Mutex::new(RunLoopRegistry::new()));
        let frame = ComWrapper::new(HostPlugFrame::new(
            Arc::new(Mutex::new(None)),
            frame_registry.clone(),
        ));
        let frame_loop = frame
            .to_com_ptr::<IPlugFrame>()
            .unwrap()
            .cast::<IRunLoop>()
            .unwrap();
        let (wrapper, dropped, hits) = probe(&host.run_loop);
        let timer = wrapper.to_com_ptr::<ITimerHandler>().unwrap();
        drop(wrapper);
        unsafe {
            assert_eq!(factory_loop.registerTimer(timer.as_ptr(), 1), kResultOk);
            assert_eq!(frame_loop.unregisterTimer(timer.as_ptr()), kResultOk);
        }
        host.run_loop.lock().unwrap().timers[0].due = std::time::Instant::now();
        host.service_run_loop();
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        assert!(frame_registry.lock().unwrap().timers.is_empty());
        host.clear_run_loop();
        unsafe {
            assert_eq!(factory_loop.registerTimer(timer.as_ptr(), 1), kResultFalse);
        }
        drop(timer);
        assert!(dropped.load(Ordering::SeqCst));
    }
    #[test]
    fn unregister_releases_last_reference_outside_lock() {
        let registry = Arc::new(Mutex::new(RunLoopRegistry::new()));
        let frame = HostPlugFrame::new(Arc::new(Mutex::new(None)), registry.clone());
        let (event, dropped, _) = probe(&registry);
        let raw = event.as_com_ref::<IEventHandler>().unwrap().as_ptr();
        unsafe {
            assert_eq!(frame.registerEventHandler(raw, 7), kResultOk);
            assert_eq!(frame.registerEventHandler(raw, 7), kResultFalse);
            assert_eq!(frame.registerEventHandler(raw, -1), kInvalidArgument);
        }
        drop(event);
        assert!(!dropped.load(Ordering::SeqCst));
        unsafe {
            assert_eq!(frame.unregisterEventHandler(raw), kResultOk);
        }
        assert!(dropped.load(Ordering::SeqCst));
    }
    #[test]
    fn loading_failure_cleanup_releases_callbacks_before_unmap() {
        let host = create_host_application();
        let guard = host.run_loop_cleanup();
        let (timer, dropped, _) = probe(&host.run_loop);
        let raw = timer.as_com_ref::<ITimerHandler>().unwrap().as_ptr();
        unsafe {
            assert_eq!(host.registerTimer(raw, 0), kResultOk);
            assert_eq!(host.registerTimer(raw, 16), kResultFalse);
        }
        drop(timer);
        drop(guard);
        assert!(dropped.load(Ordering::SeqCst));
        assert!(host.run_loop.lock().unwrap().closed);
    }
    #[test]
    fn fd_callbacks_are_nonblocking_and_registration_is_bounded() {
        use std::io::Write;
        use std::os::fd::AsRawFd;
        let registry = Arc::new(Mutex::new(RunLoopRegistry::new()));
        let frame = HostPlugFrame::new(Arc::new(Mutex::new(None)), registry.clone());
        let (event, _, hits) = probe(&registry);
        let raw = event.as_com_ref::<IEventHandler>().unwrap().as_ptr();
        let (read, mut write) = std::os::unix::net::UnixStream::pair().unwrap();
        unsafe {
            assert_eq!(frame.registerEventHandler(raw, read.as_raw_fd()), kResultOk);
        }
        service_linux_run_loop(&registry);
        assert_eq!(hits.load(Ordering::SeqCst), 0);
        write.write_all(&[42]).unwrap();
        service_linux_run_loop(&registry);
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        unsafe {
            assert_eq!(frame.unregisterEventHandler(raw), kResultOk);
        }
        for fd in 0..MAX_RUN_LOOP_REGISTRATIONS as i32 {
            unsafe {
                assert_eq!(frame.registerEventHandler(raw, fd), kResultOk);
            }
        }
        unsafe {
            assert_eq!(
                frame.registerEventHandler(raw, MAX_RUN_LOOP_REGISTRATIONS as i32),
                kOutOfMemory
            );
            assert_eq!(frame.unregisterEventHandler(raw), kResultOk);
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod linux_run_loop_epoch_tests {
    use super::*;
    use vst3::Steinberg::Linux::*;
    struct Timer {
        hits: Arc<AtomicUsize>,
    }
    impl Class for Timer {
        type Interfaces = (ITimerHandler,);
    }
    impl ITimerHandlerTrait for Timer {
        unsafe fn onTimer(&self) {
            self.hits.fetch_add(1, Ordering::SeqCst);
        }
    }
    struct ReRegister {
        frame: HostPlugFrame,
        other: ComPtr<ITimerHandler>,
    }
    impl Class for ReRegister {
        type Interfaces = (ITimerHandler,);
    }
    impl ITimerHandlerTrait for ReRegister {
        unsafe fn onTimer(&self) {
            self.frame.unregisterTimer(self.other.as_ptr());
            assert_eq!(
                self.frame.registerTimer(self.other.as_ptr(), 60_000),
                kResultOk
            );
        }
    }
    #[test]
    fn reregistered_due_timer_does_not_receive_old_dispatch() {
        let registry = Arc::new(Mutex::new(RunLoopRegistry::new()));
        let frame = HostPlugFrame::new(Arc::new(Mutex::new(None)), registry.clone());
        let hits = Arc::new(AtomicUsize::new(0));
        let other = ComWrapper::new(Timer { hits: hits.clone() })
            .to_com_ptr::<ITimerHandler>()
            .unwrap();
        let first = ComWrapper::new(ReRegister {
            frame: HostPlugFrame::new(Arc::new(Mutex::new(None)), registry.clone()),
            other: other.clone(),
        })
        .to_com_ptr::<ITimerHandler>()
        .unwrap();
        unsafe {
            assert_eq!(frame.registerTimer(first.as_ptr(), 1), kResultOk);
            assert_eq!(frame.registerTimer(other.as_ptr(), 1), kResultOk);
        }
        for timer in &mut registry.lock().unwrap().timers {
            timer.due = std::time::Instant::now();
        }
        service_linux_run_loop(&registry);
        assert_eq!(
            hits.load(Ordering::SeqCst),
            0,
            "old snapshot dispatched a newly registered timer early"
        );
        unsafe {
            frame.unregisterTimer(first.as_ptr());
            frame.unregisterTimer(other.as_ptr());
        }
    }
    struct ReRegisterOnDrop {
        frame: HostPlugFrame,
        other: ComPtr<ITimerHandler>,
        result: Arc<AtomicI32>,
    }
    impl Class for ReRegisterOnDrop {
        type Interfaces = (ITimerHandler,);
    }
    impl ITimerHandlerTrait for ReRegisterOnDrop {
        unsafe fn onTimer(&self) {}
    }
    impl Drop for ReRegisterOnDrop {
        fn drop(&mut self) {
            self.result.store(
                unsafe { self.frame.registerTimer(self.other.as_ptr(), 1) },
                Ordering::SeqCst,
            );
        }
    }
    #[test]
    fn closed_registry_cannot_be_resurrected_by_release_callback() {
        let host = create_host_application();
        let result = Arc::new(AtomicI32::new(kInternalError));
        let other = ComWrapper::new(Timer {
            hits: Arc::new(AtomicUsize::new(0)),
        })
        .to_com_ptr::<ITimerHandler>()
        .unwrap();
        let object = ComWrapper::new(ReRegisterOnDrop {
            frame: HostPlugFrame::new(Arc::new(Mutex::new(None)), host.run_loop.clone()),
            other,
            result: result.clone(),
        });
        unsafe {
            assert_eq!(
                host.registerTimer(object.as_com_ref::<ITimerHandler>().unwrap().as_ptr(), 1),
                kResultOk
            );
        }
        drop(object);
        host.clear_run_loop();
        assert_eq!(result.load(Ordering::SeqCst), kResultFalse);
        assert!(host.run_loop.lock().unwrap().timers.is_empty());
    }
    #[test]
    fn registration_identity_exhaustion_is_explicit() {
        let registry = Arc::new(Mutex::new(RunLoopRegistry::new()));
        registry.lock().unwrap().next_registration = u64::MAX;
        let frame = HostPlugFrame::new(Arc::new(Mutex::new(None)), registry.clone());
        let timer = ComWrapper::new(Timer {
            hits: Arc::new(AtomicUsize::new(0)),
        })
        .to_com_ptr::<ITimerHandler>()
        .unwrap();
        unsafe {
            assert_eq!(frame.registerTimer(timer.as_ptr(), 1), kInternalError);
        }
        assert!(registry.lock().unwrap().timers.is_empty());
    }
}

#[cfg(test)]
mod native_parameter_admission_tests {
    use super::*;

    #[test]
    fn checked_admission_refuses_poisoned_existing_or_reset_storage() {
        for reset in [false, true] {
            let changes = ParameterChanges::new(ParameterQueueLimits {
                max_queues: 1,
                max_points: 1,
            });
            changes.try_enqueue(7, 0, 0.25).unwrap();
            if reset {
                changes.clear_all().unwrap();
            }
            changes.poison_for_test();
            crate::internal::native_edit_transport::tests::allocation_free(|| {
                assert_eq!(
                    changes.try_enqueue(7, 0, 0.75),
                    Err(ParameterStorageError::Poisoned)
                );
                assert_eq!(changes.failure(), Some(ParameterStorageError::Poisoned));
            });
        }
    }

    #[test]
    fn display_overflow_and_polling_are_independent_of_native_delivery() {
        let display = Arc::new(Mutex::new(Vec::with_capacity(MAX_EDITOR_FEEDBACK)));
        let (handler, mut receiver) = ComponentHandler::new(display.clone());
        for i in 0..MAX_EDITOR_FEEDBACK + 2 {
            unsafe {
                handler.performEdit(7, i as f64);
            }
            receiver
                .stage(|edit| edit.id == 7 && edit.value == i as f64)
                .unwrap();
            receiver.finish_process(true).unwrap();
        }
        assert_eq!(display.lock().unwrap().len(), MAX_EDITOR_FEEDBACK);
        assert!(handler.native_state_capture_revision().is_ok());
    }
}
