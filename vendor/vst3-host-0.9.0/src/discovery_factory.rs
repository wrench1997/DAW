//! Live factory declarations without creating a component, controller, or view.

use super::FactoryInfo;
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::os::raw::c_char;
use std::path::{Path, PathBuf};
use vst3::{ComPtr, Steinberg::Vst::IHostApplication, Steinberg::*};

const MAX_FACTORY_CLASSES: i32 = 4096;

/// Declarations read from a loaded VST3 factory, in its original class order.
///
/// This is not [`crate::PluginInfo`] or proof that any class can be instantiated.
/// Audio/event buses, MIDI, GUI, parameters, latency and tail remain unknown: none
/// is queried, and no runtime capability is inferred from the category strings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FactoryMetadata {
    /// The module path requested by the caller (not a binary-identity attestation).
    pub path: PathBuf,
    /// Identity returned by the successful factory-info getter.
    pub factory: FactoryInfo,
    /// All declared classes, including non-audio classes; no default is selected.
    pub classes: Vec<FactoryClassMetadata>,
}

/// One live factory declaration, not an instantiated or verified instrument.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FactoryClassMetadata {
    /// Canonical, uppercase 32-hex-character class id.
    pub class_id: String,
    /// Class display name; Unicode factory metadata takes precedence when available.
    pub name: String,
    /// Main class category, for example `Audio Module Class` or `Component Controller Class`.
    pub category: String,
    /// Declared instantiation cardinality, not a successfully created instance count.
    pub cardinality: i32,
    /// Class vendor; unknown with only the original factory interface.
    pub vendor: Option<String>,
    /// Class version; unknown with only the original factory interface.
    pub version: Option<String>,
    /// Declared SDK version; unknown with only the original factory interface.
    pub sdk_version: Option<String>,
    /// Declared subcategories, split on `|`; unknown with only the original interface.
    pub sub_categories: Option<Vec<String>>,
    /// Raw class flags; unknown with only the original factory interface.
    pub class_flags: Option<u32>,
}

/// Read only factory declarations through the existing platform module loader.
///
/// No `createInstance`, `initialize`, component query, or `createView` is called.
/// Every factory/interface reference is released before normal module unload;
/// the host context outlives that unload. The result is returned only afterwards.
/// Base class identity/category/cardinality must agree with any successful
/// extended getter; failed optional getters fall back without invented values.
/// Existing `ExitDll`/`FreeLibrary` behavior is unchanged, including failures to
/// return. Class counts and fixed SDK buffers are bounded, but foreign native
/// calls cannot be time-bounded in-process. Use a separately bounded helper for
/// untrusted modules rather than treating this function as a timeout mechanism.
pub fn probe_factory_metadata(path: &Path) -> Result<FactoryMetadata> {
    probe_with_loader(path, || crate::internal::module_loader::load_module(path))
}

fn probe_with_loader(
    path: &Path,
    load: impl FnOnce() -> Result<Box<dyn crate::internal::module_loader::VstModule>>,
) -> Result<FactoryMetadata> {
    // Reverse local drop order is intentional: factory, module, host context,
    // host application. Factories may keep the context without addRef until ExitDll.
    let host_app = crate::internal::com_implementations::create_host_application();
    let host_ctx = host_app.to_com_ptr::<IHostApplication>();
    let context = host_ctx
        .as_ref()
        .map(|pointer| pointer.as_ptr() as *mut FUnknown)
        .unwrap_or(std::ptr::null_mut());
    let module = load()?;
    let factory = unsafe { ComPtr::<IPluginFactory>::from_raw(module.get_factory()?) }
        .ok_or_else(|| invalid("GetPluginFactory returned null"))?;
    let result = read_factory(path, &factory, context);
    drop(factory);
    // In particular, a hung ExitDll never produces a successful metadata reply.
    drop(module);
    result
}

fn invalid(message: impl Into<String>) -> Error {
    Error::PluginLoadFailed(format!("factory-only metadata: {}", message.into()))
}

fn succeeded(result: tresult) -> bool {
    result == kResultOk || result == kResultTrue
}

fn read_factory(
    path: &Path,
    factory: &ComPtr<IPluginFactory>,
    context: *mut FUnknown,
) -> Result<FactoryMetadata> {
    unsafe {
        let factory3 = factory.cast::<IPluginFactory3>();
        let factory2 = factory.cast::<IPluginFactory2>();
        if let Some(factory3) = &factory3 {
            let result = factory3.setHostContext(context);
            if !succeeded(result) {
                return Err(invalid(format!("setHostContext failed: {result:#x}")));
            }
        }
        let mut info: PFactoryInfo = std::mem::zeroed();
        let result = factory.getFactoryInfo(&mut info);
        if !succeeded(result) {
            return Err(invalid(format!("getFactoryInfo failed: {result:#x}")));
        }
        let factory_info = FactoryInfo {
            vendor: narrow(&info.vendor),
            url: narrow(&info.url),
            email: narrow(&info.email),
            flags: info.flags,
        };
        let count = factory.countClasses();
        if !(0..=MAX_FACTORY_CLASSES).contains(&count) {
            return Err(invalid(format!(
                "class count {count} is outside 0..={MAX_FACTORY_CLASSES}"
            )));
        }
        let mut classes = Vec::with_capacity(count as usize);
        let mut ids = HashSet::with_capacity(count as usize);
        for index in 0..count {
            let class = read_class(factory, factory2.as_ref(), factory3.as_ref(), index)?;
            if !ids.insert(class.class_id.clone()) {
                return Err(invalid(format!("class {index} has a duplicate class id")));
            }
            classes.push(class);
        }
        Ok(FactoryMetadata {
            path: path.to_path_buf(),
            factory: factory_info,
            classes,
        })
    }
}

unsafe fn read_class(
    factory: &ComPtr<IPluginFactory>,
    factory2: Option<&ComPtr<IPluginFactory2>>,
    factory3: Option<&ComPtr<IPluginFactory3>>,
    index: i32,
) -> Result<FactoryClassMetadata> {
    let mut base_info: PClassInfo = std::mem::zeroed();
    let result = factory.getClassInfo(index, &mut base_info);
    if !succeeded(result) {
        return Err(invalid(format!(
            "class {index} getClassInfo failed: {result:#x}"
        )));
    }
    let base = checked_class(
        index,
        &base_info.cid,
        FactoryClassMetadata {
            class_id: String::new(),
            name: narrow(&base_info.name),
            category: narrow(&base_info.category),
            cardinality: base_info.cardinality,
            vendor: None,
            version: None,
            sdk_version: None,
            sub_categories: None,
            class_flags: None,
        },
    )?;
    if let Some(factory3) = factory3 {
        let mut info: PClassInfoW = std::mem::zeroed();
        if succeeded(factory3.getClassInfoUnicode(index, &mut info)) {
            let class = checked_class(
                index,
                &info.cid,
                FactoryClassMetadata {
                    class_id: String::new(),
                    name: wide(&info.name),
                    category: narrow(&info.category),
                    cardinality: info.cardinality,
                    vendor: Some(wide(&info.vendor)),
                    version: Some(wide(&info.version)),
                    sdk_version: Some(wide(&info.sdkVersion)),
                    sub_categories: Some(sub_categories(&info.subCategories)),
                    class_flags: Some(info.classFlags),
                },
            )?;
            return consistent_class(index, &base, class);
        }
    }
    if let Some(factory2) = factory2 {
        let mut info: PClassInfo2 = std::mem::zeroed();
        if succeeded(factory2.getClassInfo2(index, &mut info)) {
            let class = checked_class(
                index,
                &info.cid,
                FactoryClassMetadata {
                    class_id: String::new(),
                    name: narrow(&info.name),
                    category: narrow(&info.category),
                    cardinality: info.cardinality,
                    vendor: Some(narrow(&info.vendor)),
                    version: Some(narrow(&info.version)),
                    sdk_version: Some(narrow(&info.sdkVersion)),
                    sub_categories: Some(sub_categories(&info.subCategories)),
                    class_flags: Some(info.classFlags),
                },
            )?;
            return consistent_class(index, &base, class);
        }
    }
    Ok(base)
}

fn consistent_class(
    index: i32,
    base: &FactoryClassMetadata,
    class: FactoryClassMetadata,
) -> Result<FactoryClassMetadata> {
    if class.class_id != base.class_id
        || class.category != base.category
        || class.cardinality != base.cardinality
    {
        return Err(invalid(format!(
            "class {index} extended metadata disagrees with its base class declaration"
        )));
    }
    Ok(class)
}

fn checked_class(
    index: i32,
    cid: &TUID,
    mut class: FactoryClassMetadata,
) -> Result<FactoryClassMetadata> {
    if cid.iter().all(|&byte| byte == 0) {
        return Err(invalid(format!("class {index} has an empty class id")));
    }
    if class.name.is_empty() || class.category.is_empty() || class.cardinality < 0 {
        return Err(invalid(format!(
            "class {index} has an empty name/category or negative cardinality"
        )));
    }
    // A successful getter supplies exactly one fixed-size TUID. Formatting it
    // through the shared utility preserves Windows' canonical COM byte order.
    class.class_id = crate::internal::utils::format_class_uid(cid);
    Ok(class)
}

fn narrow(value: &[c_char]) -> String {
    crate::internal::utils::c_str_to_string(value)
}

fn wide(value: &[u16]) -> String {
    let end = value.iter().position(|&ch| ch == 0).unwrap_or(value.len());
    String::from_utf16_lossy(&value[..end])
}

fn sub_categories(value: &[c_char]) -> Vec<String> {
    narrow(value)
        .split('|')
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::module_loader::VstModule;
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use vst3::{Class, ComWrapper};

    struct State {
        count: i32,
        factory_result: tresult,
        unicode_result: tresult,
        extended_result: tresult,
        base_result: tresult,
        context_result: tresult,
        zero_uid: bool,
        duplicate_uid: bool,
        empty_name: bool,
        unicode_mismatch: bool,
        extended_mismatch: bool,
        negative_cardinality: bool,
        create_calls: AtomicUsize,
        context_received: AtomicBool,
        trace: Mutex<Vec<String>>,
    }

    impl Default for State {
        fn default() -> Self {
            Self {
                count: 2,
                factory_result: kResultOk,
                unicode_result: kResultOk,
                extended_result: kResultOk,
                base_result: kResultOk,
                context_result: kResultOk,
                zero_uid: false,
                duplicate_uid: false,
                empty_name: false,
                unicode_mismatch: false,
                extended_mismatch: false,
                negative_cardinality: false,
                create_calls: AtomicUsize::new(0),
                context_received: AtomicBool::new(false),
                trace: Mutex::new(Vec::new()),
            }
        }
    }

    impl State {
        fn record(&self, event: impl Into<String>) {
            self.trace.lock().unwrap().push(event.into());
        }

        fn cid(&self, index: i32) -> TUID {
            let mut cid = [0 as c_char; 16];
            if !self.zero_uid {
                cid[15] = if self.duplicate_uid { 1 } else { index + 1 } as c_char;
            }
            cid
        }
    }

    struct MockFactory<const EXTENDED: bool>(Arc<State>);

    impl Class for MockFactory<false> {
        type Interfaces = (IPluginFactory,);
    }

    impl Class for MockFactory<true> {
        type Interfaces = (IPluginFactory3,);
    }

    impl<const EXTENDED: bool> Drop for MockFactory<EXTENDED> {
        fn drop(&mut self) {
            self.0.record("factory.drop");
        }
    }

    fn put<const N: usize>(buffer: &mut [c_char; N], text: &str) {
        assert!(text.len() < N);
        for (slot, byte) in buffer.iter_mut().zip(text.bytes()) {
            *slot = byte as c_char;
        }
    }

    fn put_wide<const N: usize>(buffer: &mut [u16; N], text: &str) {
        assert!(text.encode_utf16().count() < N);
        for (slot, word) in buffer.iter_mut().zip(text.encode_utf16()) {
            *slot = word;
        }
    }

    fn category(index: i32) -> &'static str {
        if index == 0 {
            "Component Controller Class"
        } else {
            "Audio Module Class"
        }
    }

    impl<const EXTENDED: bool> IPluginFactoryTrait for MockFactory<EXTENDED> {
        unsafe fn getFactoryInfo(&self, info: *mut PFactoryInfo) -> tresult {
            self.0.record("factory.info");
            assert!(!info.is_null());
            put(&mut (*info).vendor, "Factory vendor");
            put(&mut (*info).url, "https://example.invalid");
            put(&mut (*info).email, "factory@example.invalid");
            (*info).flags = 16;
            self.0.factory_result
        }

        unsafe fn countClasses(&self) -> i32 {
            self.0.record("factory.count");
            self.0.count
        }

        unsafe fn getClassInfo(&self, index: i32, info: *mut PClassInfo) -> tresult {
            self.0.record(format!("class.base.{index}"));
            (*info).cid = self.0.cid(index);
            (*info).cardinality = if self.0.negative_cardinality {
                -1
            } else {
                i32::MAX
            };
            put(&mut (*info).category, category(index));
            if !self.0.empty_name {
                put(&mut (*info).name, "Base name");
            }
            self.0.base_result
        }

        unsafe fn createInstance(
            &self,
            _cid: FIDString,
            _iid: FIDString,
            object: *mut *mut c_void,
        ) -> tresult {
            self.0.create_calls.fetch_add(1, Ordering::Relaxed);
            if !object.is_null() {
                *object = std::ptr::null_mut();
            }
            kNotImplemented
        }
    }

    impl IPluginFactory2Trait for MockFactory<true> {
        unsafe fn getClassInfo2(&self, index: i32, info: *mut PClassInfo2) -> tresult {
            self.0.record(format!("class.extended.{index}"));
            (*info).cid = self.0.cid(index);
            if self.0.extended_mismatch {
                (*info).cid[0] = 1;
            }
            (*info).cardinality = i32::MAX;
            put(&mut (*info).category, category(index));
            if !self.0.empty_name {
                put(&mut (*info).name, "Extended name");
            }
            put(&mut (*info).vendor, "Extended vendor");
            put(&mut (*info).version, "2.0");
            put(&mut (*info).sdkVersion, "VST 3.7.0");
            put(&mut (*info).subCategories, "Instrument|Piano");
            (*info).classFlags = u32::MAX;
            self.0.extended_result
        }
    }

    impl IPluginFactory3Trait for MockFactory<true> {
        unsafe fn getClassInfoUnicode(&self, index: i32, info: *mut PClassInfoW) -> tresult {
            self.0.record(format!("class.unicode.{index}"));
            (*info).cid = self.0.cid(index);
            if self.0.unicode_mismatch {
                (*info).cid[0] = 1;
            }
            (*info).cardinality = i32::MAX;
            put(&mut (*info).category, category(index));
            if !self.0.empty_name {
                put_wide(&mut (*info).name, "夜雨 🎹");
            }
            put_wide(&mut (*info).vendor, "音源厂商");
            put_wide(&mut (*info).version, "3.0");
            // Deliberately empty: successful empty metadata stays Some("").
            put(&mut (*info).subCategories, "Instrument|Piano");
            (*info).classFlags = 2;
            self.0.unicode_result
        }

        unsafe fn setHostContext(&self, context: *mut FUnknown) -> tresult {
            self.0.record("factory.context");
            self.0
                .context_received
                .store(!context.is_null(), Ordering::Relaxed);
            self.0.context_result
        }
    }

    struct MockModule {
        state: Arc<State>,
        extended: bool,
    }

    impl VstModule for MockModule {
        fn get_factory(&self) -> Result<*mut IPluginFactory> {
            self.state.record("module.factory");
            let pointer = if self.extended {
                ComWrapper::new(MockFactory::<true>(self.state.clone()))
                    .to_com_ptr::<IPluginFactory>()
                    .unwrap()
            } else {
                ComWrapper::new(MockFactory::<false>(self.state.clone()))
                    .to_com_ptr::<IPluginFactory>()
                    .unwrap()
            };
            Ok(pointer.into_raw())
        }

        fn path(&self) -> &Path {
            Path::new("mock.vst3")
        }
    }

    impl Drop for MockModule {
        fn drop(&mut self) {
            self.state.record("module.drop");
        }
    }

    fn probe(state: &Arc<State>, extended: bool) -> Result<FactoryMetadata> {
        let result = probe_with_loader(Path::new("mock.vst3"), || {
            Ok(Box::new(MockModule {
                state: state.clone(),
                extended,
            }))
        });
        state.record("probe.returned");
        assert_eq!(state.create_calls.load(Ordering::Relaxed), 0);
        let trace = state.trace.lock().unwrap();
        assert_eq!(
            &trace[trace.len() - 3..],
            ["factory.drop", "module.drop", "probe.returned"]
        );
        result
    }

    #[test]
    fn unicode_factory_keeps_order_non_audio_class_and_unknown_runtime() {
        let state = Arc::new(State::default());
        let result = probe(&state, true).unwrap();
        assert_eq!(result.path, Path::new("mock.vst3"));
        assert_eq!(result.factory.vendor, "Factory vendor");
        assert_eq!(result.factory.flags, 16);
        assert_eq!(result.classes.len(), 2);
        assert_eq!(result.classes[0].category, "Component Controller Class");
        assert_eq!(result.classes[1].category, "Audio Module Class");
        assert_eq!(
            result.classes[0].class_id,
            "00000000000000000000000000000001"
        );
        assert_eq!(
            result.classes[1].class_id,
            "00000000000000000000000000000002"
        );
        assert_eq!(result.classes[0].name, "夜雨 🎹");
        assert_eq!(result.classes[0].vendor.as_deref(), Some("音源厂商"));
        assert_eq!(result.classes[0].sdk_version.as_deref(), Some(""));
        assert_eq!(result.classes[0].class_flags, Some(2));
        assert_eq!(result.classes[0].cardinality, i32::MAX);
        assert!(state.context_received.load(Ordering::Relaxed));
        assert_eq!(
            state.trace.lock().unwrap().as_slice(),
            [
                "module.factory",
                "factory.context",
                "factory.info",
                "factory.count",
                "class.base.0",
                "class.unicode.0",
                "class.base.1",
                "class.unicode.1",
                "factory.drop",
                "module.drop",
                "probe.returned",
            ]
        );
        let json = serde_json::to_value(&result).unwrap();
        for unknown in [
            "has_gui",
            "has_midi_input",
            "audio_inputs",
            "latency_samples",
            "tail_samples",
            "parameters",
        ] {
            assert!(json.get(unknown).is_none());
            assert!(json["classes"][0].get(unknown).is_none());
        }
    }

    #[test]
    fn failed_unicode_getter_falls_back_to_factory2_without_creating_instance() {
        let state = Arc::new(State {
            unicode_result: kNotImplemented,
            ..State::default()
        });
        let result = probe(&state, true).unwrap();
        assert_eq!(result.classes[0].name, "Extended name");
        assert_eq!(result.classes[0].version.as_deref(), Some("2.0"));
        assert_eq!(result.classes[0].class_flags, Some(u32::MAX));
        assert_eq!(
            result.classes[0].sub_categories,
            Some(vec!["Instrument".into(), "Piano".into()])
        );
        assert!(state
            .trace
            .lock()
            .unwrap()
            .iter()
            .any(|event| event == "class.base.0"));
    }

    #[test]
    fn failed_extended_getters_fall_back_to_factory1_with_explicit_unknowns() {
        let state = Arc::new(State {
            unicode_result: kNotImplemented,
            extended_result: kNotImplemented,
            ..State::default()
        });
        let result = probe(&state, true).unwrap();
        let class = &result.classes[0];
        assert_eq!(class.name, "Base name");
        assert_eq!(class.vendor, None);
        assert_eq!(class.version, None);
        assert_eq!(class.sdk_version, None);
        assert_eq!(class.sub_categories, None);
        assert_eq!(class.class_flags, None);
    }

    #[test]
    fn original_factory_without_optional_interfaces_is_supported() {
        let state = Arc::new(State::default());
        let result = probe(&state, false).unwrap();
        assert_eq!(result.classes[0].name, "Base name");
        assert_eq!(result.classes[0].class_flags, None);
        assert!(!state.context_received.load(Ordering::Relaxed));
    }

    #[test]
    fn negative_or_oversized_class_counts_do_not_enumerate() {
        for count in [-1, MAX_FACTORY_CLASSES + 1, i32::MAX] {
            let state = Arc::new(State {
                count,
                ..State::default()
            });
            let error = probe(&state, true).unwrap_err().to_string();
            assert!(error.contains("class count"));
            assert!(!state
                .trace
                .lock()
                .unwrap()
                .iter()
                .any(|event| event.starts_with("class.")));
        }
    }

    #[test]
    fn zero_classes_is_empty_metadata_not_a_fallback_instrument() {
        let state = Arc::new(State {
            count: 0,
            ..State::default()
        });
        assert!(probe(&state, true).unwrap().classes.is_empty());
    }

    #[test]
    fn failed_factory_info_is_not_replaced_by_zero_initialized_metadata() {
        let state = Arc::new(State {
            factory_result: kNotImplemented,
            ..State::default()
        });
        let error = probe(&state, true).unwrap_err().to_string();
        assert!(error.contains("getFactoryInfo failed"));
        assert!(!state
            .trace
            .lock()
            .unwrap()
            .iter()
            .any(|event| event == "factory.count"));
    }

    #[test]
    fn failed_host_context_does_not_continue_querying_factory() {
        let state = Arc::new(State {
            context_result: kInvalidArgument,
            ..State::default()
        });
        let error = probe(&state, true).unwrap_err().to_string();
        assert!(error.contains("setHostContext failed"));
        assert!(!state
            .trace
            .lock()
            .unwrap()
            .iter()
            .any(|event| event == "factory.info"));
    }

    #[test]
    fn failed_base_class_getter_is_an_error_not_an_omitted_class() {
        let state = Arc::new(State {
            unicode_result: kNotImplemented,
            extended_result: kNotImplemented,
            base_result: kInvalidArgument,
            ..State::default()
        });
        assert!(probe(&state, true)
            .unwrap_err()
            .to_string()
            .contains("class 0 getClassInfo failed"));
        assert!(!state
            .trace
            .lock()
            .unwrap()
            .iter()
            .any(|event| event.starts_with("class.unicode")));
    }

    #[test]
    fn successful_invalid_class_metadata_is_not_hidden_by_fallback() {
        for state in [
            State {
                zero_uid: true,
                ..State::default()
            },
            State {
                empty_name: true,
                ..State::default()
            },
            State {
                duplicate_uid: true,
                ..State::default()
            },
            State {
                negative_cardinality: true,
                ..State::default()
            },
        ] {
            let state = Arc::new(state);
            assert!(probe(&state, true).is_err());
            assert!(!state
                .trace
                .lock()
                .unwrap()
                .iter()
                .any(|event| event.starts_with("class.extended")));
        }
    }

    #[test]
    fn successful_advanced_getter_cannot_change_base_class_identity() {
        for state in [
            State {
                unicode_mismatch: true,
                ..State::default()
            },
            State {
                unicode_result: kNotImplemented,
                extended_mismatch: true,
                ..State::default()
            },
        ] {
            let state = Arc::new(state);
            let error = probe(&state, true).unwrap_err().to_string();
            assert!(error.contains("extended metadata disagrees"));
            assert!(!state
                .trace
                .lock()
                .unwrap()
                .iter()
                .any(|event| event == "class.base.1"));
        }
    }

    #[test]
    fn extended_category_and_cardinality_must_agree_but_name_may_be_localized() {
        let state = Arc::new(State::default());
        let result = probe(&state, false).unwrap();
        let base = &result.classes[0];
        let mut localized = base.clone();
        localized.name = "本地名称".into();
        assert!(consistent_class(0, base, localized).is_ok());
        let mut wrong_category = base.clone();
        wrong_category.category = "Audio Module Class".into();
        assert!(consistent_class(0, base, wrong_category).is_err());
        let mut wrong_cardinality = base.clone();
        wrong_cardinality.cardinality = 1;
        assert!(consistent_class(0, base, wrong_cardinality).is_err());
    }

    #[test]
    fn fixed_buffers_are_bounded_even_without_nul_terminator() {
        let ascii = [b'x' as c_char; 64];
        assert_eq!(narrow(&ascii).len(), 64);
        let unicode = [0x4e00; 64];
        assert_eq!(wide(&unicode).chars().count(), 64);
        assert!(sub_categories(&[0; 128]).is_empty());
    }
}
