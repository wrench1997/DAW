//! Opt-in, fixed-stage load diagnostics. No environment, clock, formatting,
//! allocation or thread-local state is accessed by the disabled gate.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    OnceLock,
};

type Callback = fn(&'static str, bool, Option<i32>);

struct DiagnosticGate {
    enabled: AtomicBool,
    callback: OnceLock<Callback>,
}

impl DiagnosticGate {
    const fn new() -> Self {
        Self {
            enabled: AtomicBool::new(false),
            callback: OnceLock::new(),
        }
    }

    fn install(&self, callback: Callback) -> bool {
        if self.callback.set(callback).is_err() {
            return false;
        }
        self.enabled.store(true, Ordering::Release);
        true
    }

    #[inline]
    fn emit(&self, stage: Stage, before: bool, result: Option<i32>) {
        if !self.enabled.load(Ordering::Acquire) {
            return;
        }
        if let Some(callback) = self.callback.get() {
            callback(stage.name(), before, result);
        }
    }

    #[inline]
    fn around<T>(&self, stage: Stage, invoke: impl FnOnce() -> T) -> T {
        self.emit(stage, true, None);
        let value = invoke();
        // This marks an actual wrapper return, including Err/None. It never
        // interprets either as a missing SDK return or manufactures HRESULT.
        self.emit(stage, false, None);
        value
    }
}

static GATE: DiagnosticGate = DiagnosticGate::new();

/// Install the process-wide, opt-in VST3 load-phase diagnostic callback once.
///
/// Returns `true` only for the first installation; subsequent calls do not
/// replace the callback. `before` is `true` immediately before an observed
/// operation and `false` only after that operation returns. `result` contains
/// the original SDK HRESULT where available; `None` carries no result claim.
/// All stage names come from a closed, static vocabulary, without plugin paths,
/// state, PCM or error text. Callbacks must not panic, call back into a plugin,
/// or retain plugin objects. The caller controls timing and active load scope.
/// Without installation, each marker only reads a disabled atomic gate.
pub fn install_load_phase_diagnostics(callback: fn(&'static str, bool, Option<i32>)) -> bool {
    GATE.install(callback)
}

#[derive(Clone, Copy)]
pub(crate) enum Stage {
    #[cfg(target_os = "windows")]
    LibraryNew,
    #[cfg(target_os = "windows")]
    InitDll,
    #[cfg(target_os = "windows")]
    GetPluginFactory,
    #[cfg(target_os = "windows")]
    ExitDll,
    ComponentCreate,
    ComponentCreateInstance,
    ComponentInitialize,
    DefaultBusActivation,
    ProcessorInterface,
    SampleFormatSelection,
    ControllerAcquisition,
    ControllerCreateInstance,
    ControllerInitialize,
    ConnectComponentController,
    SetComponentHandler,
    InitialGetState,
    InitialSetComponentState,
    ExtractPluginInfo,
    CreateView,
    ReleaseProbeView,
    MidiMappingCache,
    ProgramChangeCache,
    DefaultBusArrangements,
    SetBusArrangements,
    SetupProcessing,
    SetActive,
    InitializedComponentDrop,
    PluginDrop,
    DisconnectComponentController,
    ControllerTerminate,
    ComponentTerminate,
}

impl Stage {
    fn name(self) -> &'static str {
        match self {
            #[cfg(target_os = "windows")]
            Self::LibraryNew => "windows_library_new",
            #[cfg(target_os = "windows")]
            Self::InitDll => "windows_init_dll",
            #[cfg(target_os = "windows")]
            Self::GetPluginFactory => "windows_get_plugin_factory",
            #[cfg(target_os = "windows")]
            Self::ExitDll => "windows_exit_dll",
            Self::ComponentCreate => "component_create",
            Self::ComponentCreateInstance => "component_create_instance",
            Self::ComponentInitialize => "component_initialize",
            Self::DefaultBusActivation => "default_bus_activation",
            Self::ProcessorInterface => "processor_interface",
            Self::SampleFormatSelection => "sample_format_selection",
            Self::ControllerAcquisition => "controller_acquisition",
            Self::ControllerCreateInstance => "controller_create_instance",
            Self::ControllerInitialize => "controller_initialize",
            Self::ConnectComponentController => "connect_component_controller",
            Self::SetComponentHandler => "set_component_handler",
            Self::InitialGetState => "initial_get_state",
            Self::InitialSetComponentState => "initial_set_component_state",
            Self::ExtractPluginInfo => "extract_plugin_info",
            Self::CreateView => "create_view",
            Self::ReleaseProbeView => "release_probe_view",
            Self::MidiMappingCache => "midi_mapping_cache",
            Self::ProgramChangeCache => "program_change_cache",
            Self::DefaultBusArrangements => "default_bus_arrangements",
            Self::SetBusArrangements => "processor_set_bus_arrangements",
            Self::SetupProcessing => "processor_setup_processing",
            Self::SetActive => "component_set_active",
            Self::InitializedComponentDrop => "initialized_component_drop_teardown",
            Self::PluginDrop => "plugin_drop_teardown",
            Self::DisconnectComponentController => "disconnect_component_controller",
            Self::ControllerTerminate => "controller_terminate",
            Self::ComponentTerminate => "component_terminate",
        }
    }
}

#[inline]
pub(crate) fn emit(stage: Stage, before: bool, result: Option<i32>) {
    GATE.emit(stage, before, result);
}

#[inline]
pub(crate) fn around<T>(stage: Stage, invoke: impl FnOnce() -> T) -> T {
    GATE.around(stage, invoke)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    thread_local! {
        static EVENTS: RefCell<Vec<(&'static str, bool, Option<i32>)>> = const { RefCell::new(Vec::new()) };
    }

    fn record(stage: &'static str, before: bool, result: Option<i32>) {
        EVENTS.with(|events| events.borrow_mut().push((stage, before, result)));
    }
    fn forbidden(_: &'static str, _: bool, _: Option<i32>) {
        panic!("replacement callback must never run");
    }
    fn take_events() -> Vec<(&'static str, bool, Option<i32>)> {
        EVENTS.with(|events| std::mem::take(&mut *events.borrow_mut()))
    }

    #[test]
    fn disabled_local_gate_preserves_values_without_callback_installation() {
        let gate = DiagnosticGate::new();
        let _ = take_events();
        gate.emit(Stage::ComponentInitialize, true, None);
        let result: Result<Box<[u8]>, i32> = Err(-123);
        assert_eq!(gate.around(Stage::ComponentCreate, || result), Err(-123));
        assert!(take_events().is_empty());
        assert!(!gate.enabled.load(Ordering::Acquire));
        assert!(gate.callback.get().is_none());
    }

    #[test]
    fn local_install_once_preserves_first_callback_and_hresult() {
        let gate = DiagnosticGate::new();
        let _ = take_events();
        assert!(gate.install(record));
        assert!(!gate.install(forbidden));
        gate.emit(Stage::ComponentInitialize, true, None);
        gate.emit(Stage::ComponentInitialize, false, Some(-19));
        assert_eq!(
            take_events(),
            [
                ("component_initialize", true, None),
                ("component_initialize", false, Some(-19))
            ]
        );
    }

    #[test]
    fn actual_wrapper_err_is_returned_with_identity_and_closed_stage() {
        let gate = DiagnosticGate::new();
        let _ = take_events();
        assert!(gate.install(record));
        let error = Box::new([7_u8, 13, 29]);
        let pointer = error.as_ptr();
        let original: Result<(), Box<[u8; 3]>> = Err(error);
        let returned = gate.around(Stage::ControllerAcquisition, || original);
        let returned_error = returned.unwrap_err();
        assert_eq!(returned_error.as_ptr(), pointer);
        assert_eq!(
            take_events(),
            [
                ("controller_acquisition", true, None),
                ("controller_acquisition", false, None)
            ]
        );
    }

    #[test]
    fn actual_wrapper_ok_and_none_are_not_reinterpreted() {
        let gate = DiagnosticGate::new();
        let _ = take_events();
        assert!(gate.install(record));
        let object = Box::new([1_u8, 2, 3]);
        let pointer = object.as_ptr();
        let returned = gate.around(Stage::ComponentCreate, || Ok::<_, i32>(object));
        let returned_object = returned.unwrap();
        assert_eq!(returned_object.as_ptr(), pointer);
        assert_eq!(
            gate.around(Stage::ControllerAcquisition, || None::<u32>),
            None
        );
        assert_eq!(take_events().len(), 4);
    }

    #[test]
    fn fixed_stage_vocabulary_is_unique_bounded_and_contains_no_payload_text() {
        let stages = [
            #[cfg(target_os = "windows")]
            Stage::LibraryNew,
            #[cfg(target_os = "windows")]
            Stage::InitDll,
            #[cfg(target_os = "windows")]
            Stage::GetPluginFactory,
            #[cfg(target_os = "windows")]
            Stage::ExitDll,
            Stage::ComponentCreate,
            Stage::ComponentCreateInstance,
            Stage::ComponentInitialize,
            Stage::DefaultBusActivation,
            Stage::ProcessorInterface,
            Stage::SampleFormatSelection,
            Stage::ControllerAcquisition,
            Stage::ControllerCreateInstance,
            Stage::ControllerInitialize,
            Stage::ConnectComponentController,
            Stage::SetComponentHandler,
            Stage::InitialGetState,
            Stage::InitialSetComponentState,
            Stage::ExtractPluginInfo,
            Stage::CreateView,
            Stage::ReleaseProbeView,
            Stage::MidiMappingCache,
            Stage::ProgramChangeCache,
            Stage::DefaultBusArrangements,
            Stage::SetBusArrangements,
            Stage::SetupProcessing,
            Stage::SetActive,
            Stage::InitializedComponentDrop,
            Stage::PluginDrop,
            Stage::DisconnectComponentController,
            Stage::ControllerTerminate,
            Stage::ComponentTerminate,
        ];
        let names: std::collections::BTreeSet<_> =
            stages.iter().map(|stage| stage.name()).collect();
        assert_eq!(names.len(), stages.len());
        for name in names {
            assert!(!name.is_empty() && name.len() <= 48);
            assert!(name
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte == b'_'));
        }
    }
}
