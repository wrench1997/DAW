//! Windows-only editor host, plus platform-independent lifecycle/geometry regressions.
//!
//! The associated Citrus HWND is a *logical* owner, not a Win32 owner/parent. A native
//! cross-process owner can destroy owned windows before we can detach IPlugView. Only
//! this helper owns/destroys the container; owner HWND/PID liveness is checked in the
//! main loop. This deliberately does not promise native owned-window z-order/minimize
//! behavior. No plugin handle crosses the process boundary.

use std::cell::Cell;
use std::io::{self, BufRead};

use vst3_host::IsolatedEditorState;

const MAX_EDITOR_DIMENSION: i32 = 16_384;
const COMMAND_QUEUE_CAPACITY: usize = 4;
const MESSAGE_BATCH: usize = 32;
const COMMAND_BATCH: usize = 4;
// Accommodate upstream's 64 MiB state envelope in base64, while bounding malformed
// input before JSON parsing. The queue is also bounded; the reader backpressures stdin.
const MAX_COMMAND_BYTES: usize = 96 * 1024 * 1024;

fn requires_editor_detach(command: &vst3_host::process_isolation::HostCommand) -> bool {
    use vst3_host::process_isolation::HostCommand;
    matches!(
        command,
        HostCommand::LoadPlugin { .. }
            | HostCommand::UnloadPlugin
            | HostCommand::SaveState
            | HostCommand::LoadState { .. }
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Size {
    width: i32,
    height: i32,
}

impl Size {
    fn checked(width: i32, height: i32) -> Result<Self, String> {
        if !(1..=MAX_EDITOR_DIMENSION).contains(&width)
            || !(1..=MAX_EDITOR_DIMENSION).contains(&height)
        {
            return Err(format!(
                "Editor size {width}x{height} is outside 1..={MAX_EDITOR_DIMENSION}"
            ));
        }
        Ok(Self { width, height })
    }
}

#[derive(Default)]
struct Lifecycle {
    has_editor: bool,
    size: Option<Size>,
    generation: u64,
}

impl Lifecycle {
    fn plugin_changed(&mut self, has_editor: bool) {
        self.has_editor = has_editor;
        self.size = None;
    }

    fn opened(&mut self, size: Size) {
        self.size = Some(size);
        self.generation = self.generation.saturating_add(1);
    }

    fn resized(&mut self, size: Size) {
        self.size = Some(size);
    }

    fn closed(&mut self) {
        self.size = None;
    }

    fn state(&self) -> IsolatedEditorState {
        IsolatedEditorState {
            supported: true,
            has_editor: self.has_editor,
            open: self.size.is_some(),
            width: self.size.map_or(0, |s| s.width),
            height: self.size.map_or(0, |s| s.height),
            generation: self.generation,
        }
    }
}

fn valid_owner_shape(window: u64, process_id: u32, pointer_max: u64) -> bool {
    window != 0 && window <= pointer_max && process_id != 0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Bounds {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DragEdge {
    Caption,
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl DragEdge {
    fn left(self) -> bool {
        matches!(self, Self::Left | Self::TopLeft | Self::BottomLeft)
    }
    fn right(self) -> bool {
        matches!(self, Self::Right | Self::TopRight | Self::BottomRight)
    }
    fn top(self) -> bool {
        matches!(self, Self::Top | Self::TopLeft | Self::TopRight)
    }
    fn bottom(self) -> bool {
        matches!(self, Self::Bottom | Self::BottomLeft | Self::BottomRight)
    }
}

#[derive(Clone, Copy)]
struct Drag {
    edge: DragEdge,
    initial: Bounds,
    cursor: (i32, i32),
}

impl Drag {
    fn moved(self, cursor: (i32, i32)) -> Bounds {
        let dx = i64::from(cursor.0) - i64::from(self.cursor.0);
        let dy = i64::from(cursor.1) - i64::from(self.cursor.1);
        let shift = |value: i32, delta: i64| {
            (i64::from(value) + delta).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
        };
        let mut b = self.initial;
        if self.edge == DragEdge::Caption {
            b.left = shift(b.left, dx);
            b.right = shift(b.right, dx);
            b.top = shift(b.top, dy);
            b.bottom = shift(b.bottom, dy);
        } else {
            if self.edge.left() {
                b.left = shift(b.left, dx).min(b.right.saturating_sub(1));
            }
            if self.edge.right() {
                b.right = shift(b.right, dx).max(b.left.saturating_add(1));
            }
            if self.edge.top() {
                b.top = shift(b.top, dy).min(b.bottom.saturating_sub(1));
            }
            if self.edge.bottom() {
                b.bottom = shift(b.bottom, dy).max(b.top.saturating_add(1));
            }
        }
        b
    }
}

#[derive(Clone, Copy)]
struct DpiChange {
    dpi: u32,
    suggested: Bounds,
}

#[derive(Clone, Copy)]
struct DragUpdate {
    edge: DragEdge,
    bounds: Bounds,
}

/// Stable, boxed storage shared with WndProc. Only Cells are mutated and no
/// exclusive reference to this allocation is held across a Win32/plugin call.
#[derive(Default)]
struct Signals {
    close: Cell<bool>,
    destroyed: Cell<bool>,
    size: Cell<Option<Size>>,
    dpi: Cell<Option<DpiChange>>,
    drag: Cell<Option<Drag>>,
    drag_update: Cell<Option<DragUpdate>>,
    focus: Cell<bool>,
    can_resize: Cell<bool>,
}

impl Signals {
    fn cancel_drag(&self) {
        self.drag.set(None);
        self.drag_update.set(None);
    }
}

#[derive(Debug, PartialEq, Eq)]
enum CommandLine {
    Line(Vec<u8>),
    Oversized,
    Eof,
}

fn read_command_line(reader: &mut impl BufRead, limit: usize) -> io::Result<CommandLine> {
    let mut line = Vec::new();
    let mut oversized = false;
    loop {
        let bytes = reader.fill_buf()?;
        if bytes.is_empty() {
            return Ok(if oversized {
                CommandLine::Oversized
            } else if line.is_empty() {
                CommandLine::Eof
            } else {
                CommandLine::Line(line)
            });
        }
        let newline = bytes.iter().position(|&byte| byte == b'\n');
        let count = newline.unwrap_or(bytes.len());
        if !oversized {
            if line.len().saturating_add(count) <= limit {
                line.extend_from_slice(&bytes[..count]);
            } else {
                oversized = true;
                line.clear();
            }
        }
        reader.consume(count + usize::from(newline.is_some()));
        if newline.is_some() {
            return Ok(if oversized {
                CommandLine::Oversized
            } else {
                CommandLine::Line(line)
            });
        }
    }
}

#[cfg(target_os = "windows")]
pub(super) use native::run;

#[cfg(target_os = "windows")]
mod native {
    use super::*;
    use crate::{SharedPlugin, err, handle, respond};
    use std::mem::zeroed;
    use std::ptr::{null, null_mut};
    use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
    use vst3_host::process_isolation::{HostCommand, HostResponse, ProtocolChannel};
    use vst3_host::{IsolatedEditorCommand, IsolatedEditorOwner, WindowHandle};
    use winapi::shared::minwindef::{LPARAM, LRESULT, UINT, WPARAM};
    use winapi::shared::windef::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, HBRUSH, HWND, POINT, RECT,
    };
    use winapi::um::errhandlingapi::GetLastError;
    use winapi::um::libloaderapi::GetModuleHandleW;
    use winapi::um::processthreadsapi::GetCurrentThreadId;
    use winapi::um::winuser::*;

    const WAKE_MESSAGE: UINT = WM_APP + 31;
    const MAINTENANCE_MS: u32 = 16;

    #[link(name = "ole32")]
    unsafe extern "system" {
        fn OleInitialize(reserved: *mut std::ffi::c_void) -> i32;
        fn OleUninitialize();
    }

    struct OleApartment;

    impl OleApartment {
        fn initialize() -> Result<Self, String> {
            // SAFETY: called once on this helper's UI thread before loading any plugin.
            // Both S_OK and S_FALSE require a matching OleUninitialize on the same thread.
            let status = unsafe { OleInitialize(null_mut()) };
            if matches!(status, 0 | 1) {
                Ok(Self)
            } else {
                Err(format!(
                    "Failed to initialize the editor OLE apartment: {status:#x}"
                ))
            }
        }
    }

    impl Drop for OleApartment {
        fn drop(&mut self) {
            // SAFETY: the guard never leaves the UI thread, and successful initialization
            // owns one balance. Plugin/view teardown runs before this guard is dropped.
            unsafe {
                OleUninitialize();
            }
        }
    }
    const CLASS_NAME: &str = "CitrusVst3EditorContainerV1";

    enum Input {
        Command(Box<HostCommand>),
        Invalid(String),
        Eof,
    }

    fn wide(text: &str) -> Vec<u16> {
        text.replace('\0', " ").encode_utf16().chain([0]).collect()
    }

    fn win_error(action: &str) -> String {
        // SAFETY: reads this thread's last Win32 error without changing state.
        format!("{action} failed (Windows error {})", unsafe {
            GetLastError()
        })
    }

    fn bounds(rect: RECT) -> Bounds {
        Bounds {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        }
    }

    fn drag_edge(hit: usize) -> Option<DragEdge> {
        match hit as isize {
            HTCAPTION => Some(DragEdge::Caption),
            HTLEFT => Some(DragEdge::Left),
            HTRIGHT => Some(DragEdge::Right),
            HTTOP => Some(DragEdge::Top),
            HTBOTTOM => Some(DragEdge::Bottom),
            HTTOPLEFT => Some(DragEdge::TopLeft),
            HTTOPRIGHT => Some(DragEdge::TopRight),
            HTBOTTOMLEFT => Some(DragEdge::BottomLeft),
            HTBOTTOMRIGHT => Some(DragEdge::BottomRight),
            _ => None,
        }
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        message: UINT,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        // SAFETY: Windows invokes this procedure on the creating main thread. The
        // CREATESTRUCT and WM_DPICHANGED RECT are valid for the duration of dispatch.
        // GWLP_USERDATA points to boxed Signals until destruction clears the slot.
        unsafe {
            if message == WM_NCCREATE {
                let create = &*(lparam as *const CREATESTRUCTW);
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
            }
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Signals;
            if ptr.is_null() {
                return DefWindowProcW(hwnd, message, wparam, lparam);
            }
            let signals = &*ptr;
            match message {
                WM_CLOSE => {
                    signals.close.set(true);
                    return 0; // Never let DefWindowProc destroy a still-attached view.
                }
                WM_SIZE if wparam != SIZE_MINIMIZED => {
                    let width = (lparam as u32 & 0xffff) as i32;
                    let height = ((lparam as u32 >> 16) & 0xffff) as i32;
                    if let Ok(size) = Size::checked(width, height) {
                        signals.size.set(Some(size));
                    }
                    return 0;
                }
                WM_DPICHANGED => {
                    let dpi = (wparam & 0xffff) as u32;
                    if (48..=960).contains(&dpi) && lparam != 0 {
                        signals.dpi.set(Some(DpiChange {
                            dpi,
                            suggested: bounds(*(lparam as *const RECT)),
                        }));
                    }
                    return 0;
                }
                WM_SETFOCUS => {
                    signals.focus.set(true);
                    return 0;
                }
                WM_NCLBUTTONDOWN => {
                    if let Some(edge) = drag_edge(wparam)
                        && (edge == DragEdge::Caption || signals.can_resize.get())
                    {
                        let mut cursor: POINT = zeroed();
                        let mut rect: RECT = zeroed();
                        if GetCursorPos(&mut cursor) != 0 && GetWindowRect(hwnd, &mut rect) != 0 {
                            signals.drag.set(Some(Drag {
                                edge,
                                initial: bounds(rect),
                                cursor: (cursor.x, cursor.y),
                            }));
                            signals.focus.set(true);
                            SetCapture(hwnd);
                        }
                        return 0;
                    }
                }
                WM_MOUSEMOVE => {
                    if let Some(drag) = signals.drag.get() {
                        let mut cursor: POINT = zeroed();
                        if GetCursorPos(&mut cursor) != 0 {
                            signals.drag_update.set(Some(DragUpdate {
                                edge: drag.edge,
                                bounds: drag.moved((cursor.x, cursor.y)),
                            }));
                        }
                        return 0;
                    }
                }
                WM_LBUTTONUP => {
                    if signals.drag.take().is_some() {
                        if GetCapture() == hwnd {
                            ReleaseCapture();
                        }
                        return 0;
                    }
                }
                WM_CAPTURECHANGED => {
                    if signals.drag.take().is_some() {
                        signals.drag_update.set(None);
                    }
                    return 0;
                }
                WM_CANCELMODE => {
                    signals.cancel_drag();
                    if GetCapture() == hwnd {
                        ReleaseCapture();
                    }
                    return 0;
                }
                WM_SYSCOMMAND if matches!(wparam & 0xfff0, SC_MOVE | SC_SIZE) => {
                    // The default move/resize modal loop would stop DSP IPC until the
                    // drag ends. Pointer gestures above instead use captured messages.
                    // System-menu keyboard move/size is intentionally unavailable.
                    return 0;
                }
                WM_NCDESTROY => {
                    signals.destroyed.set(true);
                    signals.close.set(true);
                    signals.drag.set(None);
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                }
                _ => {}
            }
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
    }

    fn register_class() -> Result<(), String> {
        let name = wide(CLASS_NAME);
        // SAFETY: module handle belongs to this process, string outlives registration;
        // Win32 copies the class name. The procedure has a stable static address.
        unsafe {
            let module = GetModuleHandleW(null());
            if module.is_null() {
                return Err(win_error("GetModuleHandleW"));
            }
            let class = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(window_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: module,
                hIcon: null_mut(),
                hCursor: LoadCursorW(null_mut(), IDC_ARROW),
                hbrBackground: (COLOR_WINDOW + 1) as usize as HBRUSH,
                lpszMenuName: null(),
                lpszClassName: name.as_ptr(),
            };
            if RegisterClassW(&class) == 0 {
                return Err(win_error("RegisterClassW"));
            }
        }
        Ok(())
    }

    fn validate_owner(owner: Option<&IsolatedEditorOwner>) -> Result<Option<HWND>, String> {
        let Some(owner) = owner else {
            return Ok(None);
        };
        if !valid_owner_shape(owner.window, owner.process_id, usize::MAX as u64) {
            return Err("Invalid editor owner HWND/PID".to_string());
        }
        let hwnd = owner.window as usize as HWND;
        // SAFETY: these Win32 queries validate opaque HWND values without dereferencing
        // them in Rust. A PID match is checked again throughout the window's lifetime.
        unsafe {
            let mut process_id = 0;
            if IsWindow(hwnd) == 0
                || GetWindowThreadProcessId(hwnd, &mut process_id) == 0
                || process_id != owner.process_id
                || GetAncestor(hwnd, GA_ROOT) != hwnd
            {
                return Err(
                    "Editor owner is not a live top-level HWND with the supplied PID".into(),
                );
            }
        }
        Ok(Some(hwnd))
    }

    fn same_owner(a: Option<&IsolatedEditorOwner>, b: Option<&IsolatedEditorOwner>) -> bool {
        a.map(|owner| (owner.window, owner.process_id))
            == b.map(|owner| (owner.window, owner.process_id))
    }

    struct NativeWindow {
        hwnd: HWND,
        signals: Box<Signals>,
        owner: Option<IsolatedEditorOwner>,
        dpi: u32,
        style: u32,
    }

    impl NativeWindow {
        fn new(
            title: &str,
            size: Size,
            owner: Option<IsolatedEditorOwner>,
        ) -> Result<Self, String> {
            let owner_hwnd = validate_owner(owner.as_ref())?;
            let signals = Box::<Signals>::default();
            let name = wide(CLASS_NAME);
            let title = wide(title);
            let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN;
            let mut x = CW_USEDEFAULT;
            let mut y = CW_USEDEFAULT;
            // SAFETY: this creates a real hidden top-level HWND on the main thread.
            // The validated logical owner is queried for placement only, never passed
            // as parent/owner and never handed to the plugin.
            unsafe {
                let dpi = owner_hwnd
                    .map_or_else(|| GetDpiForSystem(), |hwnd| GetDpiForWindow(hwnd))
                    .max(96);
                if let Some(hwnd) = owner_hwnd {
                    let mut rect: RECT = zeroed();
                    if GetWindowRect(hwnd, &mut rect) != 0 {
                        x = rect.left.saturating_add(48);
                        y = rect.top.saturating_add(48);
                    }
                }
                let mut rect = RECT {
                    left: 0,
                    top: 0,
                    right: size.width,
                    bottom: size.height,
                };
                if AdjustWindowRectExForDpi(&mut rect, style, 0, WS_EX_TOOLWINDOW, dpi) == 0 {
                    return Err(win_error("AdjustWindowRectExForDpi"));
                }
                let hwnd = CreateWindowExW(
                    WS_EX_TOOLWINDOW,
                    name.as_ptr(),
                    title.as_ptr(),
                    style,
                    x,
                    y,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                    null_mut(),
                    null_mut(),
                    GetModuleHandleW(null()),
                    (&*signals as *const Signals).cast_mut().cast(),
                );
                if hwnd.is_null() {
                    return Err(win_error("CreateWindowExW"));
                }
                Ok(Self {
                    hwnd,
                    signals,
                    owner,
                    dpi: GetDpiForWindow(hwnd).max(96),
                    style,
                })
            }
        }

        fn set_resizable(&mut self, can_resize: bool) {
            self.signals.can_resize.set(can_resize);
            if can_resize {
                self.style |= WS_THICKFRAME;
            } else {
                self.style &= !WS_THICKFRAME;
            }
            // SAFETY: live helper-owned HWND, main thread. Frame geometry is applied
            // with SWP_FRAMECHANGED by the following set_client_size call.
            unsafe { SetWindowLongPtrW(self.hwnd, GWL_STYLE, self.style as isize) };
        }

        fn set_client_size(&self, size: Size, anchor: Option<DragUpdate>) -> Result<(), String> {
            // SAFETY: native RECT is initialized; helper owns the live HWND. Messages
            // re-entering our WndProc only mutate Cells, never plugin state.
            unsafe {
                let mut frame = RECT {
                    left: 0,
                    top: 0,
                    right: size.width,
                    bottom: size.height,
                };
                if AdjustWindowRectExForDpi(&mut frame, self.style, 0, WS_EX_TOOLWINDOW, self.dpi)
                    == 0
                {
                    return Err(win_error("AdjustWindowRectExForDpi"));
                }
                let width = frame.right - frame.left;
                let height = frame.bottom - frame.top;
                let (x, y, position_flag) = match anchor {
                    Some(anchor) => (
                        if anchor.edge.left() {
                            anchor.bounds.right.saturating_sub(width)
                        } else {
                            anchor.bounds.left
                        },
                        if anchor.edge.top() {
                            anchor.bounds.bottom.saturating_sub(height)
                        } else {
                            anchor.bounds.top
                        },
                        0,
                    ),
                    None => (0, 0, SWP_NOMOVE),
                };
                if SetWindowPos(
                    self.hwnd,
                    null_mut(),
                    x,
                    y,
                    width,
                    height,
                    position_flag | SWP_NOACTIVATE | SWP_NOZORDER | SWP_FRAMECHANGED,
                ) == 0
                {
                    return Err(win_error("SetWindowPos"));
                }
            }
            Ok(())
        }

        fn focus(&self) {
            // Windows may refuse foreground activation under its focus-stealing rules.
            // We make a best-effort request and never attach input queues or bypass that.
            unsafe {
                ShowWindow(
                    self.hwnd,
                    if IsIconic(self.hwnd) != 0 {
                        SW_RESTORE
                    } else {
                        SW_SHOW
                    },
                );
                SetForegroundWindow(self.hwnd);
                let child = GetWindow(self.hwnd, GW_CHILD);
                SetFocus(if child.is_null() { self.hwnd } else { child });
            }
        }

        fn destroy(&mut self) -> Result<(), String> {
            if self.hwnd.is_null() {
                return Ok(());
            }
            // SAFETY: plugin detachment must already have run. Clear userdata first,
            // including on destruction failure, so no callback can outlive Signals.
            unsafe {
                SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
                if IsWindow(self.hwnd) != 0 && DestroyWindow(self.hwnd) == 0 {
                    // Continuing would discard the only tracked HWND and let Query/Close
                    // falsely report successful cleanup. Terminate this isolated helper;
                    // the OS reclaims its native resources without another plugin call.
                    let error = win_error("DestroyWindow");
                    eprintln!("Fatal native editor cleanup: {error}; terminating helper");
                    std::process::exit(1);
                }
            }
            self.hwnd = null_mut();
            Ok(())
        }
    }

    impl Drop for NativeWindow {
        fn drop(&mut self) {
            if let Err(error) = self.destroy() {
                eprintln!("{error}");
            }
        }
    }

    struct Editor {
        plugin: SharedPlugin,
        window: Option<NativeWindow>,
        lifecycle: Lifecycle,
        registration_error: Option<String>,
    }

    impl Editor {
        fn new(plugin: SharedPlugin) -> Self {
            Self {
                plugin,
                window: None,
                lifecycle: Lifecycle::default(),
                registration_error: register_class().err(),
            }
        }

        fn response(&self) -> HostResponse {
            HostResponse::EditorState {
                state: self.lifecycle.state(),
            }
        }

        fn plugin_changed(&mut self) {
            let has_editor = self
                .plugin
                .lock()
                .ok()
                .and_then(|guard| guard.as_ref().map(|plugin| plugin.has_editor()))
                .unwrap_or(false);
            self.lifecycle.plugin_changed(has_editor);
        }

        fn open(&mut self, owner: Option<IsolatedEditorOwner>) -> Result<(), String> {
            validate_owner(owner.as_ref())?;
            if let Some(window) = self.window.as_ref() {
                if !same_owner(window.owner.as_ref(), owner.as_ref()) {
                    return Err("Editor is already open with a different logical owner".into());
                }
                window.focus();
                return Ok(());
            }
            if let Some(error) = &self.registration_error {
                return Err(error.clone());
            }
            let mut guard = self
                .plugin
                .lock()
                .map_err(|_| "plugin lock poisoned".to_string())?;
            let plugin = guard
                .as_mut()
                .ok_or_else(|| "No plugin loaded".to_string())?;
            if !self.lifecycle.has_editor {
                return Err("Plugin does not have a GUI editor".into());
            }
            let (width, height) = plugin
                .get_editor_size()
                .map_err(|error| error.to_string())?;
            let size = Size::checked(width, height)?;
            let mut window =
                NativeWindow::new(&format!("{} - VST3", plugin.info().name), size, owner)?;
            // Clear requests from an earlier closed view; this frame's pending slot is
            // retained by upstream between editor lifetimes.
            let _ = plugin.take_editor_resize_request();
            let _ = plugin.set_editor_scale_factor(window.dpi as f32 / 96.0);
            // SAFETY: this is the real, live helper-owned HWND just created above. It
            // remains alive until close_editor detaches the view, including all errors.
            let handle = unsafe { WindowHandle::from_hwnd(window.hwnd.cast()) };
            if let Err(error) = plugin.open_editor(handle) {
                let _ = plugin.close_editor();
                return Err(error.to_string());
            }
            let result = (|| {
                window.set_resizable(plugin.editor_can_resize());
                // Upstream's resizeView already calls onSize. Do not echo those
                // requests back through resize_editor, including for fixed-size views.
                let (width, height) = plugin
                    .take_editor_resize_request()
                    .map(Ok)
                    .unwrap_or_else(|| plugin.get_editor_size())
                    .map_err(|error| error.to_string())?;
                let size = Size::checked(width, height)?;
                window.set_client_size(size, None)?;
                Ok::<Size, String>(size)
            })();
            match result {
                Ok(size) => {
                    self.lifecycle.opened(size);
                    window.signals.size.set(None);
                    window.focus();
                    self.window = Some(window);
                    Ok(())
                }
                Err(error) => {
                    let _ = plugin.close_editor();
                    Err(error)
                }
            }
        }

        fn close(&mut self) -> Result<(), String> {
            let Some(mut window) = self.window.take() else {
                self.lifecycle.closed();
                return Ok(());
            };
            window.signals.cancel_drag();
            // Release mouse capture before detaching. Any resulting native callback
            // records intent only and cannot re-enter the plugin lifecycle.
            unsafe {
                if GetCapture() == window.hwnd {
                    ReleaseCapture();
                }
            }
            let detach = match self.plugin.lock() {
                Ok(mut guard) => match guard.as_mut() {
                    Some(plugin) => {
                        let result = plugin.close_editor().map_err(|error| error.to_string());
                        let _ = plugin.take_editor_resize_request();
                        result
                    }
                    None => Ok(()),
                },
                Err(_) => Err("plugin lock poisoned while closing editor".into()),
            };
            // Upstream close_editor releases its view and clears setFrame even when
            // removed() reports an error. Always attempt native cleanup afterward.
            let destroy = window.destroy();
            self.lifecycle.closed();
            detach.and(destroy)
        }

        fn command(&mut self, command: IsolatedEditorCommand) -> HostResponse {
            let result = match command {
                IsolatedEditorCommand::Query => Ok(()),
                IsolatedEditorCommand::Open { owner } => self.open(owner),
                IsolatedEditorCommand::Focus => match &self.window {
                    Some(window) => {
                        window.focus();
                        Ok(())
                    }
                    None => Err("Plugin editor is not open".into()),
                },
                IsolatedEditorCommand::Close => self.close(),
            };
            match result {
                Ok(()) => self.response(),
                Err(message) => HostResponse::Error { message },
            }
        }

        /// Called only outside native dispatch and common IPC/plugin handlers.
        fn service(&mut self) {
            let Some(window) = self.window.as_ref() else {
                return;
            };
            if window.signals.close.take()
                || window.signals.destroyed.get()
                || validate_owner(window.owner.as_ref()).is_err()
            {
                if let Err(error) = self.close() {
                    eprintln!("Editor close: {error}");
                }
                return;
            }
            if let Err(error) = self.service_window() {
                // A failed geometry negotiation must not leave a mismatched live
                // container pretending to be healthy. Fail closed, keep DSP alive.
                eprintln!("Editor maintenance: {error}");
                if let Err(error) = self.close() {
                    eprintln!("Editor close: {error}");
                }
            }
        }

        fn service_window(&mut self) -> Result<(), String> {
            let Some(window) = self.window.as_mut() else {
                return Ok(());
            };
            let mut guard = self
                .plugin
                .lock()
                .map_err(|_| "plugin lock poisoned".to_string())?;
            let plugin = guard
                .as_mut()
                .ok_or_else(|| "No plugin loaded".to_string())?;
            let mut size = self
                .lifecycle
                .size
                .ok_or_else(|| "Missing editor size".to_string())?;
            if let Some(change) = window.signals.dpi.take() {
                window.dpi = change.dpi;
                if let Err(error) = plugin.set_editor_scale_factor(change.dpi as f32 / 96.0) {
                    eprintln!("Editor DPI scale: {error}");
                }
                let (width, height) = plugin
                    .get_editor_size()
                    .map_err(|error| error.to_string())?;
                size = Size::checked(width, height)?;
                window.set_client_size(
                    size,
                    Some(DragUpdate {
                        edge: DragEdge::Caption,
                        bounds: change.suggested,
                    }),
                )?;
                self.lifecycle.resized(size);
                window.signals.size.set(None);
            }
            if let Some(update) = window.signals.drag_update.take() {
                if update.edge == DragEdge::Caption {
                    // SAFETY: live helper HWND; this changes position only.
                    unsafe {
                        if SetWindowPos(
                            window.hwnd,
                            null_mut(),
                            update.bounds.left,
                            update.bounds.top,
                            0,
                            0,
                            SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER,
                        ) == 0
                        {
                            return Err(win_error("SetWindowPos"));
                        }
                    }
                } else if window.signals.can_resize.get() {
                    // Convert the requested outer geometry to physical client pixels
                    // using the current DPI/style, then let IPlugView constrain it.
                    let mut frame = RECT {
                        left: 0,
                        top: 0,
                        right: 0,
                        bottom: 0,
                    };
                    unsafe {
                        if AdjustWindowRectExForDpi(
                            &mut frame,
                            window.style,
                            0,
                            WS_EX_TOOLWINDOW,
                            window.dpi,
                        ) == 0
                        {
                            return Err(win_error("AdjustWindowRectExForDpi"));
                        }
                    }
                    let width = update
                        .bounds
                        .right
                        .saturating_sub(update.bounds.left)
                        .saturating_sub(frame.right - frame.left)
                        .clamp(1, MAX_EDITOR_DIMENSION);
                    let height = update
                        .bounds
                        .bottom
                        .saturating_sub(update.bounds.top)
                        .saturating_sub(frame.bottom - frame.top)
                        .clamp(1, MAX_EDITOR_DIMENSION);
                    let (width, height) = plugin
                        .resize_editor(width, height)
                        .map_err(|error| error.to_string())?;
                    size = Size::checked(width, height)?;
                    window.set_client_size(size, Some(update))?;
                    self.lifecycle.resized(size);
                    window.signals.size.set(None);
                }
            }
            if let Some(requested) = window.signals.size.take()
                && requested != size
            {
                if window.signals.can_resize.get() {
                    let (width, height) = plugin
                        .resize_editor(requested.width, requested.height)
                        .map_err(|error| error.to_string())?;
                    size = Size::checked(width, height)?;
                }
                // Fixed-size views are restored without sending a forbidden onSize;
                // resizable views use the size accepted by checkSizeConstraint/onSize.
                window.set_client_size(size, None)?;
                self.lifecycle.resized(size);
                window.signals.size.set(None);
            }
            if let Some((width, height)) = plugin.take_editor_resize_request() {
                let requested = Size::checked(width, height)?;
                if requested != size {
                    window.set_client_size(requested, None)?;
                    self.lifecycle.resized(requested);
                    window.signals.size.set(None);
                }
            }
            if window.signals.focus.take() {
                // Only redirect keyboard focus to the plugin's child. A focus event
                // must not undo minimization or fight the OS foreground policy.
                unsafe {
                    let child = GetWindow(window.hwnd, GW_CHILD);
                    if !child.is_null() {
                        SetFocus(child);
                    }
                }
            }
            Ok(())
        }
    }

    impl Drop for Editor {
        fn drop(&mut self) {
            if let Err(error) = self.close() {
                eprintln!("Editor shutdown: {error}");
            }
        }
    }

    fn queue_input(tx: &SyncSender<Input>, main_thread: u32, input: Input) -> bool {
        if tx.send(input).is_err() {
            return false;
        }
        // SAFETY: main thread initialized its message queue before this reader starts.
        // Posting only wakes the pump; all protocol and plugin work stays on main.
        unsafe { PostThreadMessageW(main_thread, WAKE_MESSAGE, 0, 0) };
        true
    }

    fn read_stdin(tx: SyncSender<Input>, main_thread: u32) {
        let stdin = io::stdin();
        let mut reader = stdin.lock();
        loop {
            let input = match read_command_line(&mut reader, MAX_COMMAND_BYTES) {
                Ok(CommandLine::Line(line)) if line.iter().all(u8::is_ascii_whitespace) => continue,
                Ok(CommandLine::Line(line)) => match serde_json::from_slice::<HostCommand>(&line) {
                    Ok(command) => Input::Command(Box::new(command)),
                    Err(error) => Input::Invalid(format!("Invalid command: {error}")),
                },
                Ok(CommandLine::Oversized) => {
                    Input::Invalid("Command exceeds bounded wire line limit".into())
                }
                Ok(CommandLine::Eof) => break,
                Err(error) => {
                    let _ = queue_input(
                        &tx,
                        main_thread,
                        Input::Invalid(format!("Failed to read stdin: {error}")),
                    );
                    break;
                }
            };
            let shutdown = matches!(&input, Input::Command(command) if matches!(**command, HostCommand::Shutdown));
            if !queue_input(&tx, main_thread, input) || shutdown {
                return;
            }
        }
        let _ = queue_input(&tx, main_thread, Input::Eof);
    }

    fn handle_input(
        input: Input,
        editor: &mut Editor,
        protocol: &mut ProtocolChannel,
        sample_rate: &mut f64,
    ) -> bool {
        let command = match input {
            Input::Command(command) => *command,
            Input::Invalid(message) => {
                respond(protocol, &HostResponse::Error { message });
                return true;
            }
            Input::Eof => return false,
        };
        if matches!(command, HostCommand::Shutdown) {
            return false;
        }
        let response = match command {
            HostCommand::Editor { command } => editor.command(command),
            HostCommand::CreateGui => match editor.open(None) {
                Ok(()) => {
                    let state = editor.lifecycle.state();
                    HostResponse::GuiCreated {
                        width: state.width,
                        height: state.height,
                    }
                }
                Err(message) => HostResponse::Error { message },
            },
            HostCommand::CloseGui => match editor.close() {
                Ok(()) => HostResponse::Success {
                    message: "editor closed".into(),
                },
                Err(message) => HostResponse::Error { message },
            },
            command => {
                let changes_plugin = matches!(
                    command,
                    HostCommand::LoadPlugin { .. } | HostCommand::UnloadPlugin
                );
                if let Some(response) = crate::preflight_state_command(&command, &editor.plugin) {
                    respond(protocol, &response);
                    return true;
                }
                if requires_editor_detach(&command)
                    && let Err(error) = editor.close()
                {
                    respond(
                        protocol,
                        &err(
                            "Failed to close editor before plugin state operation",
                            error,
                        ),
                    );
                    return true;
                }
                let response = handle(command, &editor.plugin, sample_rate, None);
                if changes_plugin {
                    editor.plugin_changed();
                }
                response
            }
        };
        respond(protocol, &response);
        true
    }

    pub(crate) fn run(plugin: SharedPlugin, mut protocol: ProtocolChannel) {
        // This happens before any plugin binary is loaded. On supported Windows 10/11
        // builds physical pixels and WM_DPICHANGED provide the editor's content scale.
        // If a manifest already chose DPI awareness, Windows leaves it unchanged.
        unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        let _ole = match OleApartment::initialize() {
            Ok(apartment) => apartment,
            Err(message) => {
                respond(&mut protocol, &HostResponse::Error { message });
                return;
            }
        };
        let mut editor = Editor::new(plugin.clone());
        // Create the queue before the reader can PostThreadMessage. There is no dummy
        // HWND: the only editor handle is a real window created when Open succeeds.
        let main_thread = unsafe {
            let mut message = zeroed();
            PeekMessageW(&mut message, null_mut(), 0, 0, PM_NOREMOVE);
            GetCurrentThreadId()
        };
        let (tx, rx): (SyncSender<Input>, Receiver<Input>) =
            mpsc::sync_channel(COMMAND_QUEUE_CAPACITY);
        let reader = std::thread::Builder::new()
            .name("vst3-helper-stdin".into())
            .spawn(move || read_stdin(tx, main_thread));
        if let Err(error) = reader {
            respond(&mut protocol, &err("Failed to start stdin reader", error));
            return;
        }
        let mut sample_rate = 44_100.0;
        let mut running = true;
        while running {
            let mut activity = false;
            // Neither native floods nor queued DSP/control commands can drain an
            // unbounded batch and monopolize this loop. Slow third-party calls are
            // still inherently blocking; process isolation is not a realtime guarantee.
            for _ in 0..MESSAGE_BATCH {
                let mut message = unsafe { zeroed() };
                if unsafe { PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) } == 0 {
                    break;
                }
                activity = true;
                if message.message == WM_QUIT {
                    running = false;
                    break;
                }
                unsafe {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            editor.service();
            if !running {
                break;
            }
            for _ in 0..COMMAND_BATCH {
                match rx.try_recv() {
                    Ok(input) => {
                        activity = true;
                        running = handle_input(input, &mut editor, &mut protocol, &mut sample_rate);
                        editor.service();
                        if !running {
                            break;
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        running = false;
                        break;
                    }
                }
            }
            if running && !activity {
                // The reader's wake closes the queue-check/wait race. The timeout
                // additionally services plugin resize requests and logical owner loss.
                unsafe {
                    MsgWaitForMultipleObjectsEx(
                        0,
                        null(),
                        MAINTENANCE_MS,
                        QS_ALLINPUT,
                        MWMO_INPUTAVAILABLE,
                    );
                }
            }
        }
        if let Err(error) = editor.close() {
            eprintln!("Editor shutdown: {error}");
        }
        // Plugin destruction, including DSP shutdown, also stays on this main thread.
        if let Ok(mut guard) = plugin.lock() {
            *guard = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufReader, Cursor};

    #[test]
    fn state_capture_and_restore_require_editor_detachment() {
        use vst3_host::process_isolation::HostCommand;
        assert!(requires_editor_detach(&HostCommand::SaveState));
        assert!(requires_editor_detach(&HostCommand::LoadState {
            data: Vec::new(),
            context: vst3_host::StateContext::Project,
        }));
        assert!(requires_editor_detach(&HostCommand::UnloadPlugin));
        assert!(!requires_editor_detach(&HostCommand::NativeDirtyRevision));
        assert!(!requires_editor_detach(&HostCommand::TakeParameterEdits));
    }

    #[test]
    fn lifecycle_repeated_close_and_queries_do_not_advance_generation() {
        let mut life = Lifecycle::default();
        assert!(!life.state().open);
        life.closed();
        assert_eq!(life.state().generation, 0);
        life.plugin_changed(true);
        let size = Size::checked(560, 400).unwrap();
        life.opened(size);
        let first = life.state().generation;
        life.resized(size);
        assert_eq!(life.state().generation, first);
        assert_eq!(life.state().width, 560);
        life.closed();
        let closed = life.state().generation;
        life.closed();
        assert_eq!(life.state().generation, closed);
        assert_eq!((life.state().width, life.state().height), (0, 0));
        assert!(life.state().has_editor);
        life.opened(size);
        assert!(life.state().generation > closed);
    }

    #[test]
    fn replacement_clears_state_and_next_attachment_gets_a_new_generation() {
        let mut life = Lifecycle::default();
        life.plugin_changed(true);
        life.opened(Size::checked(400, 300).unwrap());
        let old = life.state().generation;
        life.plugin_changed(false);
        assert_eq!(life.state().generation, old);
        assert!(!life.state().has_editor);
        assert!(!life.state().open);
        life.plugin_changed(true);
        life.opened(Size::checked(400, 300).unwrap());
        assert!(life.state().generation > old);
    }

    #[test]
    fn size_checks_reject_zero_negative_and_pathological_geometry() {
        for pair in [(0, 1), (1, 0), (-1, 800), (i32::MAX, 10), (10, 16_385)] {
            assert!(Size::checked(pair.0, pair.1).is_err());
        }
        assert!(Size::checked(1, 1).is_ok());
        assert!(Size::checked(16_384, 16_384).is_ok());
    }

    #[test]
    fn owner_validation_rejects_null_pid_and_pointer_truncation() {
        assert!(!valid_owner_shape(0, 55, u64::MAX));
        assert!(!valid_owner_shape(100, 0, u64::MAX));
        assert!(!valid_owner_shape(
            u64::from(u32::MAX) + 1,
            55,
            u64::from(u32::MAX)
        ));
        assert!(valid_owner_shape(100, 55, u64::from(u32::MAX)));
    }

    #[test]
    fn captured_resize_keeps_opposite_edges_anchored_and_never_inverts() {
        let initial = Bounds {
            left: 100,
            top: 200,
            right: 500,
            bottom: 600,
        };
        let drag = Drag {
            edge: DragEdge::TopLeft,
            initial,
            cursor: (100, 200),
        };
        assert_eq!(
            drag.moved((120, 240)),
            Bounds {
                left: 120,
                top: 240,
                ..initial
            }
        );
        assert_eq!(
            drag.moved((1000, 1000)),
            Bounds {
                left: 499,
                top: 599,
                ..initial
            }
        );
        for edge in [
            DragEdge::Left,
            DragEdge::Right,
            DragEdge::Top,
            DragEdge::Bottom,
            DragEdge::TopLeft,
            DragEdge::TopRight,
            DragEdge::BottomLeft,
            DragEdge::BottomRight,
        ] {
            let moved = Drag { edge, ..drag }.moved((i32::MAX, i32::MIN));
            assert!(moved.right >= moved.left);
            assert!(moved.bottom >= moved.top);
        }
    }

    #[test]
    fn captured_caption_drag_translates_the_window() {
        let drag = Drag {
            edge: DragEdge::Caption,
            initial: Bounds {
                left: 10,
                top: 20,
                right: 410,
                bottom: 320,
            },
            cursor: (100, 100),
        };
        assert_eq!(
            drag.moved((90, 120)),
            Bounds {
                left: 0,
                top: 40,
                right: 400,
                bottom: 340
            }
        );
    }

    #[test]
    fn deferred_native_intent_coalesces_and_cancel_discards_capture_work() {
        let signals = Signals::default();
        let first = Size::checked(400, 300).unwrap();
        let last = Size::checked(560, 400).unwrap();
        let rect = Bounds {
            left: 10,
            top: 20,
            right: 570,
            bottom: 420,
        };
        signals.size.set(Some(first));
        signals.close.set(true);
        signals.size.set(Some(last));
        signals.dpi.set(Some(DpiChange {
            dpi: 120,
            suggested: rect,
        }));
        signals.dpi.set(Some(DpiChange {
            dpi: 144,
            suggested: rect,
        }));
        signals.drag.set(Some(Drag {
            edge: DragEdge::Caption,
            initial: rect,
            cursor: (10, 20),
        }));
        signals.drag_update.set(Some(DragUpdate {
            edge: DragEdge::Caption,
            bounds: rect,
        }));
        assert_eq!(signals.size.take(), Some(last));
        assert!(signals.size.take().is_none());
        let dpi = signals.dpi.take().unwrap();
        assert_eq!((dpi.dpi, dpi.suggested), (144, rect));
        assert!(signals.close.take());
        assert!(!signals.close.take());
        assert!(!signals.destroyed.get());
        assert!(!signals.can_resize.get());
        assert!(!signals.focus.get());
        let update = signals.drag_update.get().unwrap();
        assert_eq!((update.edge, update.bounds), (DragEdge::Caption, rect));
        signals.cancel_drag();
        assert!(signals.drag.get().is_none());
        assert!(signals.drag_update.get().is_none());
    }

    #[test]
    fn bounded_reader_discards_oversized_line_and_recovers_next_command() {
        let mut input = BufReader::with_capacity(2, Cursor::new(b"abcdef\nok\n"));
        assert_eq!(
            read_command_line(&mut input, 4).unwrap(),
            CommandLine::Oversized
        );
        assert_eq!(
            read_command_line(&mut input, 4).unwrap(),
            CommandLine::Line(b"ok".to_vec())
        );
        assert_eq!(read_command_line(&mut input, 4).unwrap(), CommandLine::Eof);
    }

    #[test]
    fn bounded_reader_accepts_exact_limit_and_trailing_partial_line() {
        let mut input = BufReader::with_capacity(2, Cursor::new(b"1234\nx"));
        assert_eq!(
            read_command_line(&mut input, 4).unwrap(),
            CommandLine::Line(b"1234".to_vec())
        );
        assert_eq!(
            read_command_line(&mut input, 4).unwrap(),
            CommandLine::Line(b"x".to_vec())
        );
        assert_eq!(read_command_line(&mut input, 4).unwrap(), CommandLine::Eof);
        let mut input = Cursor::new(b"12345");
        assert_eq!(
            read_command_line(&mut input, 4).unwrap(),
            CommandLine::Oversized
        );
        assert_eq!(read_command_line(&mut input, 4).unwrap(), CommandLine::Eof);
    }

    #[test]
    fn bounded_queue_backpressures_and_batches_are_finite() {
        use std::sync::mpsc::{TrySendError, sync_channel};
        let (tx, rx) = sync_channel(COMMAND_QUEUE_CAPACITY);
        for value in 0..COMMAND_QUEUE_CAPACITY {
            tx.try_send(value).unwrap();
        }
        assert!(matches!(tx.try_send(99), Err(TrySendError::Full(99))));
        for expected in 0..COMMAND_QUEUE_CAPACITY {
            assert_eq!(rx.try_recv().unwrap(), expected);
        }
        assert!((1..=64).contains(&MESSAGE_BATCH));
        assert!((1..=COMMAND_QUEUE_CAPACITY).contains(&COMMAND_BATCH));
        assert!((90..=128).contains(&(MAX_COMMAND_BYTES / 1024 / 1024)));
    }
}
