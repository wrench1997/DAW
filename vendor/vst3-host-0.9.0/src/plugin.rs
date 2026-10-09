//! VST3 plugin wrapper with safe API

use crate::{
    audio::{AudioBuffers, AudioBusLayout, AudioLevels, BusAudioBuffers},
    error::{Error, Result},
    midi::{
        MidiChannel, MidiEvent, PluginEvent, PluginEventData, MAX_EVENT_PAYLOAD_BYTES,
        MAX_EVENT_TEXT_UNITS,
    },
    parameters::{Parameter, ParameterUpdate},
};
use crossbeam_queue::ArrayQueue;
use std::sync::{Arc, Mutex};

/// A `Send` + `Sync` handle for draining the MIDI a plugin emits (arpeggiators, MPE, MIDI
/// thru, …) without locking the audio thread.
///
/// Obtain one from [`Plugin::output_midi_handle`]. The plugin's audio thread pushes emitted
/// events into a lock-free bounded queue; this handle pops them from any other thread (e.g. a
/// UI poll loop) with no lock on either side — the lock-free counterpart to the audio-thread
/// drain in [`Plugin::take_output_midi`]. When the queue is full the oldest event is dropped,
/// so a host that stops polling can't grow it without bound.
///
/// Available for in-process plugins; the process-isolation path returns `None` (output MIDI
/// crosses the boundary in the IPC responses instead).
#[derive(Clone)]
pub struct OutputMidiConsumer {
    queue: Arc<ArrayQueue<PluginEvent>>,
}

impl OutputMidiConsumer {
    pub(crate) fn from_queue(queue: Arc<ArrayQueue<PluginEvent>>) -> Self {
        Self { queue }
    }

    /// Pop the oldest emitted event, or `None` if none are queued. Lock-free.
    pub fn pop(&self) -> Option<MidiEvent> {
        while let Some(event) = self.queue.pop() {
            if let Some(midi) = event.to_midi() {
                return Some(midi);
            }
        }
        None
    }

    /// Drain all currently queued events in emission order into a `Vec`. Lock-free pops; the
    /// returned `Vec` allocates on the calling thread (intended for a UI/control thread, not
    /// the audio thread — use [`pop`](Self::pop) in a loop to stay allocation-free).
    pub fn drain(&self) -> Vec<MidiEvent> {
        let mut out = Vec::new();
        while let Some(event) = self.pop() {
            out.push(event);
        }
        out
    }
}

/// A `Send` + `Sync` handle for draining every owned event a plugin emits.
#[derive(Clone)]
pub struct OutputEventConsumer {
    queue: Arc<ArrayQueue<PluginEvent>>,
}

impl OutputEventConsumer {
    pub(crate) fn from_queue(queue: Arc<ArrayQueue<PluginEvent>>) -> Self {
        Self { queue }
    }

    /// Pop the oldest emitted event, or `None` if none are queued.
    pub fn pop(&self) -> Option<PluginEvent> {
        self.queue.pop()
    }

    /// Drain all currently queued events in emission order.
    pub fn drain(&self) -> Vec<PluginEvent> {
        let mut out = Vec::new();
        while let Some(event) = self.pop() {
            out.push(event);
        }
        out
    }
}

/// Information about a VST3 plugin
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PluginInfo {
    /// Full path to the VST3 bundle/file
    pub path: std::path::PathBuf,
    /// Plugin name
    pub name: String,
    /// Vendor/manufacturer name
    pub vendor: String,
    /// Plugin version
    pub version: String,
    /// Plugin category (e.g., "Fx", "Instrument")
    pub category: String,
    /// Unique plugin ID
    pub uid: String,
    /// Number of audio input buses
    pub audio_inputs: u32,
    /// Number of audio output buses
    pub audio_outputs: u32,
    /// Whether the plugin accepts MIDI input
    pub has_midi_input: bool,
    /// Whether the plugin produces MIDI output
    pub has_midi_output: bool,
    /// Whether the plugin has a GUI
    pub has_gui: bool,
}

/// A saved plugin preset: the plugin's identity plus its opaque state blob.
///
/// Written/read by [`Plugin::save_preset`] / [`Plugin::load_preset`]. The `uid` lets a
/// loader reject a preset that belongs to a different plugin (whose state bytes would be
/// meaningless or harmful).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PluginPreset {
    /// The originating plugin's unique class id ([`PluginInfo::uid`]).
    pub uid: String,
    /// The originating plugin's display name (for friendly mismatch messages).
    pub plugin_name: String,
    /// The plugin's opaque serialized state (from [`Plugin::save_state`]).
    pub state: Vec<u8>,
}

/// Why a plugin is being handed a state blob.
///
/// VST3 lets a plugin ask *where* the state it is being given came from: the host attaches an
/// `IStreamAttributes` list to the `IBStream` it passes to `setState`, and the plugin reads the
/// `PresetAttributes::kStateType` key from it. The SDK ships `Vst::Helpers::isProjectState()`
/// for exactly this — it answers "yes" only for `StateType::kProject` and "this came from a
/// preset" for every other value — and plugins use the answer to decide what to restore (a
/// preset should not, for instance, drag a project's per-instance routing along with it).
///
/// Pass this to [`Plugin::load_state_with_context`]. [`Plugin::load_state`] uses
/// [`StateContext::Project`]; [`Plugin::load_vstpreset`] and [`Plugin::load_preset`] use
/// [`StateContext::Preset`] with the file they read.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum StateContext {
    /// The blob is part of a host session/project restore.
    ///
    /// Tags the stream `StateType::kProject` — "the state is restored from a project loading
    /// or it is saved in a project".
    #[default]
    Project,
    /// The blob came from a standalone preset file.
    ///
    /// Tags the stream `StateType::kTrackPreset`. Of the three values the SDK defines,
    /// `kProject` is explicitly the project case and `kDefault` is narrower than it looks —
    /// "the state is restored from a preset (marked as *default*) or the host wants to store a
    /// default state of the plug-in" — so claiming it for a preset the user picked would tell
    /// the plugin this is its initialization patch. `kTrackPreset` ("the state is restored from
    /// a track preset") is the SDK's remaining preset-file value, and it is what makes
    /// `Vst::Helpers::isProjectState()` answer "came from a preset" rather than the
    /// "host doesn't implement this" it returns when the attribute is missing entirely.
    Preset {
        /// Full path of the file the state was read from, when the caller knows it.
        ///
        /// Published as `PresetAttributes::kFilePathStringType` ("full file path string (if
        /// available) where the preset comes from"); left off the stream when `None`.
        ///
        /// Text rather than a `PathBuf` because it is published as a UTF-16 stream attribute
        /// and crosses the process-isolation boundary as JSON, neither of which can carry a
        /// non-UTF-8 path. [`StateContext::preset_from_path`] does the lossy conversion once,
        /// where it is visible.
        path: Option<String>,
    },
}

impl StateContext {
    /// A preset load whose source file is unknown (state handed over in memory, say).
    pub fn preset() -> Self {
        Self::Preset { path: None }
    }

    /// A preset load from `path`, which the plugin will see as the stream's file path.
    pub fn preset_from_path(path: impl AsRef<std::path::Path>) -> Self {
        Self::Preset {
            path: Some(path.as_ref().to_string_lossy().into_owned()),
        }
    }

    /// The source file this state came from, when one is known.
    pub fn file_path(&self) -> Option<&std::path::Path> {
        match self {
            Self::Project => None,
            Self::Preset { path } => path.as_deref().map(std::path::Path::new),
        }
    }
}

/// The two independent state streams defined by VST3.
///
/// This stays private to the crate. Public callers continue to exchange an opaque `Vec<u8>`;
/// the versioned envelope below lets that API preserve both streams while still accepting the
/// raw component blobs returned by older releases.
pub(crate) struct StateSnapshot {
    pub component: Vec<u8>,
    pub controller: Option<Vec<u8>>,
}

const STATE_SNAPSHOT_MAGIC: &[u8; 16] = b"VST3HOST_STATE\0\0";
const STATE_SNAPSHOT_VERSION: u32 = 1;
const STATE_SNAPSHOT_HEADER_SIZE: usize = 16 + 4 + 4 + 4;
const NO_CONTROLLER_STATE: u32 = u32::MAX;
/// Preserve the pre-envelope state capacity: component and controller payloads may together use
/// the same 64 MiB that a host-provided `MemoryStream` permits.
const MAX_STATE_SNAPSHOT_PAYLOAD_BYTES: usize =
    crate::internal::com_implementations::MAX_STREAM_BYTES;
pub(crate) const MAX_STATE_SNAPSHOT_BYTES: usize =
    STATE_SNAPSHOT_HEADER_SIZE + MAX_STATE_SNAPSHOT_PAYLOAD_BYTES;

pub(crate) fn encode_state_snapshot(snapshot: &StateSnapshot) -> Result<Vec<u8>> {
    let component_len = u32::try_from(snapshot.component.len())
        .map_err(|_| Error::Other("component state is too large".to_string()))?;
    let controller_len = match snapshot.controller.as_ref() {
        Some(state) => u32::try_from(state.len())
            .map_err(|_| Error::Other("controller state is too large".to_string()))?,
        None => NO_CONTROLLER_STATE,
    };
    let payload_size = snapshot
        .component
        .len()
        .checked_add(snapshot.controller.as_ref().map_or(0, Vec::len))
        .ok_or_else(|| Error::Other("plugin state size overflow".to_string()))?;
    let total = STATE_SNAPSHOT_HEADER_SIZE
        .checked_add(payload_size)
        .ok_or_else(|| Error::Other("plugin state size overflow".to_string()))?;
    if payload_size > MAX_STATE_SNAPSHOT_PAYLOAD_BYTES {
        return Err(Error::Other(format!(
            "combined plugin state payload is too large ({payload_size} bytes, maximum \
             {MAX_STATE_SNAPSHOT_PAYLOAD_BYTES})"
        )));
    }

    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(STATE_SNAPSHOT_MAGIC);
    out.extend_from_slice(&STATE_SNAPSHOT_VERSION.to_le_bytes());
    out.extend_from_slice(&component_len.to_le_bytes());
    out.extend_from_slice(&controller_len.to_le_bytes());
    out.extend_from_slice(&snapshot.component);
    if let Some(controller) = snapshot.controller.as_ref() {
        out.extend_from_slice(controller);
    }
    Ok(out)
}

pub(crate) fn decode_state_snapshot(data: &[u8]) -> Result<StateSnapshot> {
    if !data.starts_with(STATE_SNAPSHOT_MAGIC) {
        if data.len() > MAX_STATE_SNAPSHOT_PAYLOAD_BYTES {
            return Err(Error::Other(format!(
                "legacy component state is too large ({} bytes, maximum \
                 {MAX_STATE_SNAPSHOT_PAYLOAD_BYTES})",
                data.len()
            )));
        }
        return Ok(StateSnapshot {
            component: data.to_vec(),
            controller: None,
        });
    }
    if data.len() < STATE_SNAPSHOT_HEADER_SIZE {
        return Err(Error::Other(
            "truncated vst3-host state snapshot header".to_string(),
        ));
    }
    let version = read_snapshot_u32(&data[16..20]);
    if version != STATE_SNAPSHOT_VERSION {
        return Err(Error::Other(format!(
            "unsupported vst3-host state snapshot version {version}"
        )));
    }
    let component_len = read_snapshot_u32(&data[20..24]) as usize;
    let encoded_controller_len = read_snapshot_u32(&data[24..28]);
    let controller_len =
        (encoded_controller_len != NO_CONTROLLER_STATE).then_some(encoded_controller_len as usize);
    let payload_size = component_len
        .checked_add(controller_len.unwrap_or(0))
        .ok_or_else(|| Error::Other("plugin state size overflow".to_string()))?;
    let expected = STATE_SNAPSHOT_HEADER_SIZE
        .checked_add(payload_size)
        .ok_or_else(|| Error::Other("plugin state size overflow".to_string()))?;
    if expected != data.len() || expected > MAX_STATE_SNAPSHOT_BYTES {
        return Err(Error::Other(format!(
            "invalid vst3-host state snapshot size (header describes {expected} bytes, got {})",
            data.len()
        )));
    }
    let component_start = STATE_SNAPSHOT_HEADER_SIZE;
    let component_end = component_start + component_len;
    Ok(StateSnapshot {
        component: data[component_start..component_end].to_vec(),
        controller: controller_len.map(|len| data[component_end..component_end + len].to_vec()),
    })
}

fn read_snapshot_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

/// A plugin unit (from `IUnitInfo`) and its program list, if any.
///
/// Units form a hierarchy (via [`parent_id`](Self::parent_id)); a unit may carry a named
/// program list (e.g. a synth's factory patches). Query with [`Plugin::get_units`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PluginUnit {
    /// Unit id (unique within the plugin; the root unit is conventionally `0`).
    pub id: i32,
    /// Parent unit id, or `-1` for the root.
    pub parent_id: i32,
    /// Unit display name.
    pub name: String,
    /// Program-list id associated with this unit, or `None` when it has no program list.
    pub program_list_id: Option<i32>,
    /// Program names in this unit's program list (empty if the unit has none).
    pub programs: Vec<String>,
}

/// A plugin-provided name for a MIDI pitch in a particular program.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProgramPitchName {
    /// MIDI pitch number (`0..=127`).
    pub midi_pitch: i16,
    /// Plugin-provided display name.
    pub name: String,
}

/// Host automation mode reported to controllers implementing `IAutomationState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AutomationState {
    /// Automation is disabled.
    Off,
    /// Read existing automation.
    Read,
    /// Write automation.
    Write,
    /// Read and write automation.
    ReadWrite,
}

/// What kind of parameter-edit gesture event a plugin's editor reported.
///
/// VST3 editors bracket a user gesture with `beginEdit`/`endEdit` (e.g. mouse-down /
/// mouse-up on a knob) and report the values in between with `performEdit`. Capturing the
/// brackets — not just the value changes — lets a host distinguish a deliberate, completed
/// edit from intermediate drag values, coalesce automation into one undo step, or know when a
/// gesture is in progress.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ParameterEditKind {
    /// The user started editing this parameter (`IComponentHandler::beginEdit`).
    BeginGesture,
    /// The parameter's value changed (`IComponentHandler::performEdit`); carries the new
    /// normalized value in [`ParameterEdit::value`].
    ValueChange,
    /// The user finished editing this parameter (`IComponentHandler::endEdit`).
    EndGesture,
}

/// A single parameter-edit gesture event reported by a plugin's own editor.
///
/// Drained in order via [`Plugin::take_parameter_edits`]. This is the richer superset of
/// [`Plugin::get_parameter_changes`]: where that drains only the value changes, this preserves
/// the begin/change/end ordering so a host can reconstruct each gesture.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ParameterEdit {
    /// Parameter id the gesture targets.
    pub id: u32,
    /// Which gesture phase this event is.
    pub kind: ParameterEditKind,
    /// The new normalized value (`0.0..=1.0`), present only for
    /// [`ParameterEditKind::ValueChange`]; `None` for begin/end brackets.
    pub value: Option<f64>,
}

/// A control-plane action requested by a plugin through `IComponentHandler2`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProgressKind {
    /// Deferred restoration of plugin state.
    AsyncStateRestoration,
    /// Work performed for the plugin's user interface.
    UiBackgroundTask,
    /// A newer SDK progress kind unknown to this host version.
    Other(u32),
}

/// An equality-safe normalized progress value.
///
/// Construction rejects NaN, infinities, and values outside `0.0..=1.0`, allowing progress
/// notifications to retain exact equality and lossless process-isolation serialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProgressValue(u64);

impl ProgressValue {
    /// Construct a valid normalized progress value.
    pub fn new(value: f64) -> Option<Self> {
        (value.is_finite() && (0.0..=1.0).contains(&value)).then(|| Self(value.to_bits()))
    }

    /// Return the normalized floating-point value.
    pub fn get(self) -> f64 {
        f64::from_bits(self.0)
    }
}

/// One entry in a context menu a plugin asked the host to display.
///
/// The [`item_id`](Self::item_id) is assigned by the host for one popup and is the value to pass
/// to [`Plugin::execute_context_menu_item`]. The plugin's own `tag` is included for diagnostics;
/// it is not necessarily unique.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ContextMenuItem {
    /// Host-assigned id, unique within the containing popup.
    pub item_id: u32,
    /// Plugin-provided menu label.
    pub name: String,
    /// Plugin-provided command tag.
    pub tag: i32,
    /// Raw VST3 `IContextMenuItem::Flags` bits.
    pub flags: i32,
}

impl ContextMenuItem {
    /// Whether this entry is a separator.
    pub fn is_separator(&self) -> bool {
        self.flags & vst3::Steinberg::Vst::IContextMenuItem_::Flags_::kIsSeparator as i32 != 0
    }

    /// Whether this entry is disabled.
    pub fn is_disabled(&self) -> bool {
        self.flags & vst3::Steinberg::Vst::IContextMenuItem_::Flags_::kIsDisabled as i32 != 0
    }

    /// Whether this entry is checked.
    pub fn is_checked(&self) -> bool {
        self.flags & vst3::Steinberg::Vst::IContextMenuItem_::Flags_::kIsChecked as i32 != 0
    }

    /// Whether this entry begins a logical group.
    pub fn is_group_start(&self) -> bool {
        let group_start = vst3::Steinberg::Vst::IContextMenuItem_::Flags_::kIsGroupStart as i32;
        self.flags & group_start == group_start
    }

    /// Whether this entry ends a logical group.
    pub fn is_group_end(&self) -> bool {
        let group_end = vst3::Steinberg::Vst::IContextMenuItem_::Flags_::kIsGroupEnd as i32;
        self.flags & group_end == group_end
    }
}

/// An owned snapshot sent by a plug-in through VST3's data-exchange API.
///
/// The plug-in-owned exchange block is only valid during the controller callback. The host
/// copies it on a non-realtime thread, so values returned by
/// [`Plugin::take_data_exchange_blocks`] remain valid after that callback returns.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DataExchangeBlock {
    /// Host queue identifier assigned when the processor opened the queue.
    pub queue_id: u32,
    /// Processor-defined context identifier associated with the queue.
    pub user_context_id: u32,
    /// Queue-local block identifier.
    pub block_id: u32,
    /// Complete block payload.
    #[serde(with = "crate::process_isolation::state_codec")]
    pub data: Vec<u8>,
}

/// A control-plane action requested by a plugin through its host callbacks.
///
/// # Ordering
///
/// Notifications keep their order relative to each other, and so do the
/// [`ParameterEdit`]s from [`Plugin::take_parameter_edits`] — but the two streams are buffered
/// independently, so their relative order is not preserved. In particular, the
/// [`GroupEditStarted`](Self::GroupEditStarted) / [`GroupEditFinished`](Self::GroupEditFinished)
/// bracket cannot be correlated with the edits that fell inside it: treat it as "the plugin is
/// currently in a grouped gesture", not as a delimiter around specific parameter edits.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum HostNotification {
    /// The plugin changed whether its state needs saving.
    DirtyChanged(bool),
    /// The plugin asked the host to open an editor, optionally by view name.
    OpenEditorRequested {
        /// Requested view name, or `None` for the default editor.
        name: Option<String>,
    },
    /// The plugin began a grouped edit.
    GroupEditStarted,
    /// The plugin finished a grouped edit.
    GroupEditFinished,
    /// The plugin selected a different unit in its own UI.
    UnitSelectionChanged {
        /// Newly selected unit id.
        unit_id: i32,
    },
    /// A program list, or one program in it, changed in the plugin.
    ProgramListChanged {
        /// Program-list id.
        list_id: i32,
        /// Changed program, or `None` when the whole list changed.
        program_index: Option<i32>,
    },
    /// The plugin changed its bus/channel-to-unit assignments.
    UnitByBusChanged,
    /// The plugin began a bounded progress operation.
    ProgressStarted {
        /// Host-assigned progress operation id.
        id: u64,
        /// Kind of work the plugin is performing.
        kind: ProgressKind,
        /// Optional plugin-provided description.
        description: Option<String>,
    },
    /// The plugin updated an existing progress operation.
    ProgressUpdated {
        /// Host-assigned progress operation id.
        id: u64,
        /// Normalized progress in the inclusive `0.0..=1.0` range.
        value: ProgressValue,
    },
    /// The plugin finished an existing progress operation.
    ProgressFinished {
        /// Host-assigned progress operation id.
        id: u64,
    },
    /// The plugin populated a context menu and asked the host to display it.
    ///
    /// After showing the menu, call [`Plugin::execute_context_menu_item`] for the chosen entry,
    /// or [`Plugin::dismiss_context_menu`] if it was dismissed. Either call releases the
    /// plugin-owned menu targets retained for this popup.
    ContextMenuRequested {
        /// Host-assigned popup id.
        menu_id: u64,
        /// Parameter the menu belongs to, or `None` for a view-wide menu.
        parameter_id: Option<u32>,
        /// Horizontal popup coordinate in the plugin view.
        x: i32,
        /// Vertical popup coordinate in the plugin view.
        y: i32,
        /// Menu entries in display order.
        items: Vec<ContextMenuItem>,
    },
}

impl HostNotification {
    pub(crate) fn invalidates_unit_cache(&self) -> bool {
        matches!(
            self,
            Self::ProgramListChanged { .. } | Self::UnitByBusChanged
        )
    }
}

/// What a plugin asked the host to re-read, reported through `IComponentHandler::restartComponent`
/// and drained with [`Plugin::take_restart_flags`].
///
/// A plugin raises these when something about it changed behind the host's back — a preset load
/// that renamed its parameters, a mode switch that changed its latency, an oversampling toggle
/// that changed its bus layout. Poll this alongside [`Plugin::take_parameter_edits`] and respond
/// directly, or use [`Plugin::service_host_requests`] for lifecycle-sensitive changes:
///
/// - [`param_values_changed`](Self::param_values_changed) — re-read values with
///   [`Plugin::get_parameters`].
/// - [`param_titles_changed`](Self::param_titles_changed) — re-read the parameter list itself
///   (ids, names, ranges may all differ).
/// - [`latency_changed`](Self::latency_changed) — re-read [`Plugin::latency_samples`] and adjust
///   delay compensation. Requires stopping processing first, per the VST3 spec.
/// - [`io_changed`](Self::io_changed) — the bus layout changed; re-query
///   [`Plugin::bus_arrangements`] and reconfigure. Nothing is rebuilt for you.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RestartFlags(i32);

impl RestartFlags {
    /// Wrap the raw VST3 `RestartFlags` bitmask.
    pub(crate) fn from_bits(bits: i32) -> Self {
        Self(bits)
    }

    /// The raw VST3 bitmask (`Steinberg::Vst::RestartFlags`), for flags this type doesn't name.
    pub fn bits(self) -> i32 {
        self.0
    }

    /// True when the plugin raised no flags since the last drain.
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    fn has(self, flag: i32) -> bool {
        self.0 & flag != 0
    }

    /// `kParamValuesChanged`: parameter *values* changed (e.g. a preset was loaded).
    pub fn param_values_changed(self) -> bool {
        self.has(vst3::Steinberg::Vst::RestartFlags_::kParamValuesChanged)
    }

    /// `kReloadComponent`: the component must be recreated by the outer host.
    pub fn reload_component(self) -> bool {
        self.has(vst3::Steinberg::Vst::RestartFlags_::kReloadComponent)
    }

    /// `kParamTitlesChanged`: the parameter list itself changed (ids, names, ranges, count).
    pub fn param_titles_changed(self) -> bool {
        self.has(vst3::Steinberg::Vst::RestartFlags_::kParamTitlesChanged)
    }

    /// `kLatencyChanged`: the plugin's reported latency changed.
    pub fn latency_changed(self) -> bool {
        self.has(vst3::Steinberg::Vst::RestartFlags_::kLatencyChanged)
    }

    /// `kIoChanged`: the plugin's bus configuration changed.
    pub fn io_changed(self) -> bool {
        self.has(vst3::Steinberg::Vst::RestartFlags_::kIoChanged)
    }

    /// `kMidiCCAssignmentChanged`: controller-to-parameter mappings changed.
    pub fn midi_cc_assignment_changed(self) -> bool {
        self.has(vst3::Steinberg::Vst::RestartFlags_::kMidiCCAssignmentChanged)
    }

    /// `kNoteExpressionChanged`: note-expression metadata changed.
    pub fn note_expression_changed(self) -> bool {
        self.has(vst3::Steinberg::Vst::RestartFlags_::kNoteExpressionChanged)
    }

    /// `kIoTitlesChanged`: bus names changed.
    pub fn io_titles_changed(self) -> bool {
        self.has(vst3::Steinberg::Vst::RestartFlags_::kIoTitlesChanged)
    }

    /// `kPrefetchableSupportChanged`: prefetch support changed.
    pub fn prefetchable_support_changed(self) -> bool {
        self.has(vst3::Steinberg::Vst::RestartFlags_::kPrefetchableSupportChanged)
    }

    /// `kRoutingInfoChanged`: routing metadata changed.
    pub fn routing_info_changed(self) -> bool {
        self.has(vst3::Steinberg::Vst::RestartFlags_::kRoutingInfoChanged)
    }

    /// `kKeyswitchChanged`: keyswitch metadata changed.
    pub fn keyswitch_changed(self) -> bool {
        self.has(vst3::Steinberg::Vst::RestartFlags_::kKeyswitchChanged)
    }

    /// `kParamIDMappingChanged`: processor/controller parameter-id mappings changed.
    pub fn param_id_mapping_changed(self) -> bool {
        self.has(vst3::Steinberg::Vst::RestartFlags_::kParamIDMappingChanged)
    }
}

#[cfg(test)]
mod restart_flag_tests {
    use super::RestartFlags;
    use vst3::Steinberg::Vst::RestartFlags_ as Flags;

    #[test]
    fn exposes_every_vst3_restart_flag() {
        let bits = Flags::kReloadComponent
            | Flags::kIoChanged
            | Flags::kParamValuesChanged
            | Flags::kLatencyChanged
            | Flags::kParamTitlesChanged
            | Flags::kMidiCCAssignmentChanged
            | Flags::kNoteExpressionChanged
            | Flags::kIoTitlesChanged
            | Flags::kPrefetchableSupportChanged
            | Flags::kRoutingInfoChanged
            | Flags::kKeyswitchChanged
            | Flags::kParamIDMappingChanged;
        let flags = RestartFlags::from_bits(bits);
        assert!(flags.reload_component());
        assert!(flags.io_changed());
        assert!(flags.param_values_changed());
        assert!(flags.latency_changed());
        assert!(flags.param_titles_changed());
        assert!(flags.midi_cc_assignment_changed());
        assert!(flags.note_expression_changed());
        assert!(flags.io_titles_changed());
        assert!(flags.prefetchable_support_changed());
        assert!(flags.routing_info_changed());
        assert!(flags.keyswitch_changed());
        assert!(flags.param_id_mapping_changed());
    }
}

/// How the plugin should run: real-time, read-ahead prefetch, or offline rendering.
/// Maps to VST3 `kRealtime` / `kPrefetch` / `kOffline`; plugins may switch quality or
/// look-ahead accordingly. Defaults to [`ProcessMode::Realtime`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum ProcessMode {
    /// Real-time / live processing (the default; `kRealtime`).
    #[default]
    Realtime,
    /// Real-time playback with data available ahead of the play cursor (`kPrefetch`).
    ///
    /// A plugin implementing VST3 `IPrefetchableSupport` may reject this mode when it reports
    /// that it is never, or not yet, prefetchable.
    Prefetch,
    /// Offline / non-real-time processing such as a render or bounce (`kOffline`).
    Offline,
}

/// Authoritative transport at the start of the next audio block.
///
/// Supply this before every DAW processing quantum, including stopped blocks and seeks.
/// Sample and musical positions are independent: a tempo change must not recompute PPQ
/// from the current sample position. Without a subsequent update, playing positions advance
/// by the processed frame count; stopped project positions stay fixed.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProcessTransport {
    /// Project sample position at the start of the block (negative preroll is supported).
    pub sample_position: i64,
    /// Project position in quarter notes at the start of the block.
    pub quarter_note_position: f64,
    /// Tempo in beats per minute; finite and greater than zero.
    pub tempo: f64,
    /// Whether project time advances during this block.
    pub playing: bool,
    /// Positive time-signature numerator.
    pub time_sig_numerator: i32,
    /// Time-signature denominator: one of 1, 2, 4, 8, or 16.
    pub time_sig_denominator: i32,
}

impl ProcessTransport {
    pub(crate) fn validate(self) -> Result<()> {
        if !self.quarter_note_position.is_finite()
            || !self.tempo.is_finite()
            || self.tempo <= 0.0
            || self.time_sig_numerator <= 0
            || !matches!(self.time_sig_denominator, 1 | 2 | 4 | 8 | 16)
        {
            return Err(Error::InvalidParameter(
                "invalid per-block process transport".into(),
            ));
        }
        Ok(())
    }

    pub(crate) fn advance(&mut self, frames: i64, sample_rate: f64) {
        if self.playing {
            self.sample_position = self.sample_position.saturating_add(frames);
            if sample_rate > 0.0 {
                self.quarter_note_position += frames as f64 / sample_rate * (self.tempo / 60.0);
            }
        }
    }
}

/// VST3 plugin instance
#[allow(clippy::type_complexity)] // callback fields are Box<dyn Fn...>; intrinsic to the API
pub struct Plugin {
    // Internal state is hidden from public API
    pub(crate) info: PluginInfo,
    pub(crate) compatibility: Vec<crate::discovery::ClassCompatibility>,
    pub(crate) is_processing: bool,
    /// Configured sample rate (exposed via [`Plugin::sample_rate`]).
    pub(crate) sample_rate: f64,
    /// Configured max block size (exposed via [`Plugin::block_size`]).
    pub(crate) block_size: usize,
    pub(crate) audio_levels: Arc<Mutex<AudioLevels>>,
    pub(crate) parameter_change_callback: Option<Box<dyn Fn(u32, f64) + Send + 'static>>,
    pub(crate) audio_callback: Option<Box<dyn Fn(&AudioLevels) + Send + 'static>>,

    // These will be populated by the actual implementation
    pub(crate) internal: Option<Box<dyn PluginInternal>>,
}

/// An in-process plugin whose controller, editor, lifecycle and destruction stay on the
/// thread that loaded it. Create it with [`crate::Vst3Host::load_main_thread_plugin`].
///
/// This additive helper entry point preserves the legacy [`Plugin`] API. It deliberately
/// provides no `Deref`, `DerefMut`, mutable `Plugin` reference or owner extraction. Its
/// private `Rc` marker prevents sending or sharing the owner across threads in safe Rust.
/// Processing remains synchronous; this is not yet a real-time-safe worker interface.
/// The caller must load it on its application's native UI/main thread.
///
/// Normal same-thread use remains available:
/// ```no_run
/// # fn main() -> vst3_host::Result<()> {
/// let mut host = vst3_host::Vst3Host::builder().build()?;
/// let mut plugin: vst3_host::MainThreadPlugin = host.load_main_thread_plugin("synth.vst3")?;
/// plugin.start_processing()?;
/// let mut audio = vst3_host::AudioBuffers::new(0, plugin.output_channel_count(), 16, plugin.sample_rate());
/// plugin.process_audio(&mut audio)?;
/// plugin.stop_processing()?;
/// drop(plugin);
/// # Ok(())
/// # }
/// ```
///
/// ```compile_fail
/// use vst3_host::MainThreadPlugin;
/// fn requires_send<T: Send>() {}
/// requires_send::<MainThreadPlugin>();
/// ```
/// ```compile_fail
/// use vst3_host::MainThreadPlugin;
/// fn requires_sync<T: Sync>() {}
/// requires_sync::<MainThreadPlugin>();
/// ```
/// ```compile_fail
/// fn extract(plugin: vst3_host::MainThreadPlugin) -> vst3_host::Plugin {
///     plugin.into_inner()
/// }
/// ```
/// ```compile_fail
/// fn borrow(plugin: &mut vst3_host::MainThreadPlugin) -> &mut vst3_host::Plugin {
///     &mut *plugin
/// }
/// ```
pub struct MainThreadPlugin {
    plugin: Plugin,
    loading_thread: std::thread::ThreadId,
    _same_thread: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl MainThreadPlugin {
    /// Internal same-thread capability seam. The exclusive owner borrow spans the callback;
    /// state/setup/metadata/teardown must wait until both restricted borrows end.
    /// Deliberately does not invoke arbitrary legacy Plugin user callbacks or metering hooks.
    #[allow(dead_code)] // future helper command factoring is a separate checkpoint
    pub(crate) fn with_domain_session(
        &mut self,
        callback: &mut crate::internal::plugin_impl::domain_session::DomainSessionCallback<'_>,
    ) -> Result<()> {
        if std::thread::current().id() != self.loading_thread {
            return Err(Error::Other(
                "domain session must run on the plugin loading thread".to_string(),
            ));
        }
        self.plugin
            .internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .with_domain_session(callback)
    }

    // Only the in-process host loader calls this; there is no public conversion from a
    // legacy movable Plugin that may have initialized its controller on another thread.
    pub(crate) fn from_in_process(plugin: Plugin) -> Self {
        Self {
            plugin,
            loading_thread: std::thread::current().id(),
            _same_thread: std::marker::PhantomData,
        }
    }

    /// Main-thread equivalent of [`Plugin::info`].
    pub fn info(&self) -> &PluginInfo {
        self.plugin.info()
    }
    /// Main-thread equivalent of [`Plugin::class_compatibility`].
    pub fn class_compatibility(&self) -> &[crate::discovery::ClassCompatibility] {
        self.plugin.class_compatibility()
    }
    /// Main-thread equivalent of [`Plugin::replaced_class_ids`].
    pub fn replaced_class_ids(&self) -> &[String] {
        self.plugin.replaced_class_ids()
    }
    /// Main-thread equivalent of [`Plugin::sample_rate`].
    pub fn sample_rate(&self) -> f64 {
        self.plugin.sample_rate()
    }
    /// Main-thread equivalent of [`Plugin::block_size`].
    pub fn block_size(&self) -> usize {
        self.plugin.block_size()
    }
    /// Main-thread equivalent of [`Plugin::reconfigure`].
    pub fn reconfigure(&mut self, sample_rate: f64, block_size: usize) -> Result<()> {
        self.plugin.reconfigure(sample_rate, block_size)
    }
    /// Main-thread equivalent of [`Plugin::set_process_mode`].
    pub fn set_process_mode(&mut self, mode: ProcessMode) -> Result<()> {
        self.plugin.set_process_mode(mode)
    }
    /// Main-thread equivalent of [`Plugin::bus_arrangements`].
    pub fn bus_arrangements(&self) -> Result<crate::audio::BusArrangements> {
        self.plugin.bus_arrangements()
    }
    /// Main-thread equivalent of [`Plugin::set_bus_arrangements`].
    pub fn set_bus_arrangements(
        &mut self,
        inputs: &[crate::audio::SpeakerArrangement],
        outputs: &[crate::audio::SpeakerArrangement],
    ) -> Result<()> {
        self.plugin.set_bus_arrangements(inputs, outputs)
    }
    /// Main-thread equivalent of [`Plugin::set_bus_active`].
    pub fn set_bus_active(
        &mut self,
        media_type: crate::audio::MediaType,
        direction: crate::audio::BusDirection,
        bus_index: i32,
        active: bool,
    ) -> Result<()> {
        self.plugin
            .set_bus_active(media_type, direction, bus_index, active)
    }
    /// Main-thread equivalent of [`Plugin::get_parameters`].
    pub fn get_parameters(&self) -> Result<Vec<Parameter>> {
        self.plugin.get_parameters()
    }
    /// Main-thread equivalent of [`Plugin::set_parameter`].
    pub fn set_parameter(&mut self, id: u32, value: f64) -> Result<()> {
        self.plugin.set_parameter(id, value)
    }
    /// Main-thread equivalent of [`Plugin::set_parameter_at`].
    pub fn set_parameter_at(&mut self, id: u32, value: f64, sample_offset: i32) -> Result<()> {
        self.plugin.set_parameter_at(id, value, sample_offset)
    }
    /// Main-thread equivalent of [`Plugin::set_process_transport`].
    pub fn set_process_transport(&mut self, transport: ProcessTransport) -> Result<()> {
        self.plugin.set_process_transport(transport)
    }
    /// Main-thread equivalent of [`Plugin::set_tempo`].
    pub fn set_tempo(&mut self, bpm: f64) -> Result<()> {
        self.plugin.set_tempo(bpm)
    }
    /// Main-thread equivalent of [`Plugin::set_time_signature`].
    pub fn set_time_signature(&mut self, numerator: i32, denominator: i32) -> Result<()> {
        self.plugin.set_time_signature(numerator, denominator)
    }
    /// Main-thread equivalent of [`Plugin::set_playing`].
    pub fn set_playing(&mut self, playing: bool) -> Result<()> {
        self.plugin.set_playing(playing)
    }
    /// Main-thread equivalent of [`Plugin::get_units`].
    pub fn get_units(&self) -> Result<Vec<PluginUnit>> {
        self.plugin.get_units()
    }
    /// Main-thread equivalent of [`Plugin::select_program`].
    pub fn select_program(&mut self, unit_id: i32, program_index: i32) -> Result<()> {
        self.plugin.select_program(unit_id, program_index)
    }
    /// Main-thread equivalent of [`Plugin::selected_unit`].
    pub fn selected_unit(&self) -> Result<Option<i32>> {
        self.plugin.selected_unit()
    }
    /// Main-thread equivalent of [`Plugin::select_unit`].
    pub fn select_unit(&mut self, unit_id: i32) -> Result<()> {
        self.plugin.select_unit(unit_id)
    }
    /// Main-thread equivalent of [`Plugin::program_pitch_names`].
    pub fn program_pitch_names(
        &self,
        program_list_id: i32,
        program_index: i32,
    ) -> Result<Vec<ProgramPitchName>> {
        self.plugin
            .program_pitch_names(program_list_id, program_index)
    }
    /// Main-thread equivalent of [`Plugin::get_program_data`].
    pub fn get_program_data(
        &self,
        program_list_id: i32,
        program_index: i32,
    ) -> Result<Option<Vec<u8>>> {
        self.plugin.get_program_data(program_list_id, program_index)
    }
    /// Main-thread equivalent of [`Plugin::set_program_data`].
    pub fn set_program_data(
        &mut self,
        program_list_id: i32,
        program_index: i32,
        data: &[u8],
    ) -> Result<()> {
        self.plugin
            .set_program_data(program_list_id, program_index, data)
    }
    /// Main-thread equivalent of [`Plugin::get_unit_data`].
    pub fn get_unit_data(&self, unit_id: i32) -> Result<Option<Vec<u8>>> {
        self.plugin.get_unit_data(unit_id)
    }
    /// Main-thread equivalent of [`Plugin::set_unit_data`].
    pub fn set_unit_data(&mut self, unit_id: i32, data: &[u8]) -> Result<()> {
        self.plugin.set_unit_data(unit_id, data)
    }
    /// Main-thread equivalent of [`Plugin::begin_host_edit`].
    pub fn begin_host_edit(&mut self, parameter_id: u32) -> Result<()> {
        self.plugin.begin_host_edit(parameter_id)
    }
    /// Main-thread equivalent of [`Plugin::end_host_edit`].
    pub fn end_host_edit(&mut self, parameter_id: u32) -> Result<()> {
        self.plugin.end_host_edit(parameter_id)
    }
    /// Main-thread equivalent of [`Plugin::send_midi_learn`].
    pub fn send_midi_learn(&mut self, bus: i32, channel: i16, controller: u16) -> Result<()> {
        self.plugin.send_midi_learn(bus, channel, controller)
    }
    /// Main-thread equivalent of [`Plugin::set_automation_state`].
    pub fn set_automation_state(&mut self, state: AutomationState) -> Result<()> {
        self.plugin.set_automation_state(state)
    }
    /// Main-thread equivalent of [`Plugin::remap_parameter_id`].
    pub fn remap_parameter_id(
        &self,
        old_plugin_uid: &str,
        old_param_id: u32,
    ) -> Result<Option<u32>> {
        self.plugin.remap_parameter_id(old_plugin_uid, old_param_id)
    }
    /// Main-thread equivalent of [`Plugin::latency_samples`].
    pub fn latency_samples(&self) -> u32 {
        self.plugin.latency_samples()
    }
    /// Main-thread equivalent of [`Plugin::tail_samples`].
    pub fn tail_samples(&self) -> u32 {
        self.plugin.tail_samples()
    }
    /// Main-thread equivalent of [`Plugin::midi_cc_to_parameter`].
    pub fn midi_cc_to_parameter(&self, bus: i32, channel: i16, cc: u16) -> Option<u32> {
        self.plugin.midi_cc_to_parameter(bus, channel, cc)
    }
    /// Main-thread equivalent of [`Plugin::get_parameter`].
    pub fn get_parameter(&self, id: u32) -> Result<f64> {
        self.plugin.get_parameter(id)
    }
    /// Main-thread equivalent of [`Plugin::format_parameter`].
    pub fn format_parameter(&self, id: u32, normalized: f64) -> Result<String> {
        self.plugin.format_parameter(id, normalized)
    }
    /// Main-thread equivalent of [`Plugin::set_parameter_by_name`].
    pub fn set_parameter_by_name(&mut self, name: &str, value: f64) -> Result<()> {
        self.plugin.set_parameter_by_name(name, value)
    }
    /// Main-thread equivalent of [`Plugin::find_parameter`].
    pub fn find_parameter(&self, name: &str) -> Result<Parameter> {
        self.plugin.find_parameter(name)
    }
    /// Main-thread equivalent of [`Plugin::send_midi_note`].
    pub fn send_midi_note(&mut self, note: u8, velocity: u8, channel: MidiChannel) -> Result<()> {
        self.plugin.send_midi_note(note, velocity, channel)
    }
    /// Main-thread equivalent of [`Plugin::send_midi_note_off`].
    pub fn send_midi_note_off(&mut self, note: u8, channel: MidiChannel) -> Result<()> {
        self.plugin.send_midi_note_off(note, channel)
    }
    /// Main-thread equivalent of [`Plugin::send_midi_cc`].
    pub fn send_midi_cc(&mut self, controller: u8, value: u8, channel: MidiChannel) -> Result<()> {
        self.plugin.send_midi_cc(controller, value, channel)
    }
    /// Main-thread equivalent of [`Plugin::send_midi_event`].
    pub fn send_midi_event(&mut self, event: MidiEvent) -> Result<()> {
        self.plugin.send_midi_event(event)
    }
    /// Main-thread equivalent of [`Plugin::send_midi_event_at`].
    pub fn send_midi_event_at(&mut self, event: MidiEvent, sample_offset: i32) -> Result<()> {
        self.plugin.send_midi_event_at(event, sample_offset)
    }
    /// Main-thread equivalent of [`Plugin::send_plugin_event`].
    pub fn send_plugin_event(&mut self, event: PluginEvent) -> Result<()> {
        self.plugin.send_plugin_event(event)
    }
    /// Main-thread equivalent of [`Plugin::send_sysex`].
    pub fn send_sysex(&mut self, bytes: Vec<u8>) -> Result<()> {
        self.plugin.send_sysex(bytes)
    }
    /// Main-thread equivalent of [`Plugin::send_sysex_at`].
    pub fn send_sysex_at(&mut self, bytes: Vec<u8>, sample_offset: i32) -> Result<()> {
        self.plugin.send_sysex_at(bytes, sample_offset)
    }
    /// Main-thread equivalent of [`Plugin::note_on`].
    pub fn note_on(
        &mut self,
        channel: MidiChannel,
        note: u8,
        velocity: u8,
    ) -> Result<crate::midi::NoteId> {
        self.plugin.note_on(channel, note, velocity)
    }
    /// Main-thread equivalent of [`Plugin::note_on_at`].
    pub fn note_on_at(
        &mut self,
        channel: MidiChannel,
        note: u8,
        velocity: u8,
        sample_offset: i32,
    ) -> Result<crate::midi::NoteId> {
        self.plugin
            .note_on_at(channel, note, velocity, sample_offset)
    }
    /// Main-thread equivalent of [`Plugin::note_off`].
    pub fn note_off(&mut self, id: crate::midi::NoteId) -> Result<()> {
        self.plugin.note_off(id)
    }
    /// Main-thread equivalent of [`Plugin::note_off_at`].
    pub fn note_off_at(&mut self, id: crate::midi::NoteId, sample_offset: i32) -> Result<()> {
        self.plugin.note_off_at(id, sample_offset)
    }
    /// Main-thread equivalent of [`Plugin::send_note_expression`].
    pub fn send_note_expression(
        &mut self,
        id: crate::midi::NoteId,
        kind: crate::midi::NoteExpressionType,
        value: f64,
    ) -> Result<()> {
        self.plugin.send_note_expression(id, kind, value)
    }
    /// Main-thread equivalent of [`Plugin::send_note_expression_at`].
    pub fn send_note_expression_at(
        &mut self,
        id: crate::midi::NoteId,
        kind: crate::midi::NoteExpressionType,
        value: f64,
        sample_offset: i32,
    ) -> Result<()> {
        self.plugin
            .send_note_expression_at(id, kind, value, sample_offset)
    }
    /// Main-thread equivalent of [`Plugin::note_expressions`].
    pub fn note_expressions(&self) -> Result<Vec<crate::midi::NoteExpressionInfo>> {
        self.plugin.note_expressions()
    }
    /// Main-thread equivalent of [`Plugin::start_processing`].
    pub fn start_processing(&mut self) -> Result<()> {
        self.plugin.start_processing()
    }
    /// Main-thread equivalent of [`Plugin::stop_processing`].
    pub fn stop_processing(&mut self) -> Result<()> {
        self.plugin.stop_processing()
    }
    /// Main-thread equivalent of [`Plugin::process_audio`].
    pub fn process_audio(&mut self, buffers: &mut AudioBuffers) -> Result<()> {
        self.plugin.process_audio(buffers)
    }
    /// Main-thread equivalent of [`Plugin::audio_bus_layout`].
    pub fn audio_bus_layout(&self) -> Result<AudioBusLayout> {
        self.plugin.audio_bus_layout()
    }
    /// Main-thread equivalent of [`Plugin::create_bus_audio_buffers`].
    pub fn create_bus_audio_buffers(&self, block_size: usize) -> Result<BusAudioBuffers> {
        self.plugin.create_bus_audio_buffers(block_size)
    }
    /// Main-thread equivalent of [`Plugin::process_bus_audio`].
    pub fn process_bus_audio(&mut self, buffers: &mut BusAudioBuffers) -> Result<()> {
        self.plugin.process_bus_audio(buffers)
    }
    /// Main-thread equivalent of [`Plugin::get_output_levels`].
    pub fn get_output_levels(&self) -> AudioLevels {
        self.plugin.get_output_levels()
    }
    /// Main-thread equivalent of [`Plugin::is_processing`].
    pub fn is_processing(&self) -> bool {
        self.plugin.is_processing()
    }
    /// Main-thread equivalent of [`Plugin::has_editor`].
    pub fn has_editor(&self) -> bool {
        self.plugin.has_editor()
    }
    /// Main-thread equivalent of [`Plugin::isolated_editor`].
    pub fn isolated_editor(
        &mut self,
        command: IsolatedEditorCommand,
    ) -> Result<IsolatedEditorState> {
        self.plugin.isolated_editor(command)
    }
    /// Main-thread equivalent of [`Plugin::open_editor`].
    pub fn open_editor(&mut self, parent: WindowHandle) -> Result<()> {
        self.plugin.open_editor(parent)
    }
    /// Main-thread equivalent of [`Plugin::service_run_loop`].
    pub fn service_run_loop(&mut self) {
        self.plugin.service_run_loop()
    }
    /// Main-thread equivalent of [`Plugin::close_editor`].
    pub fn close_editor(&mut self) -> Result<()> {
        self.plugin.close_editor()
    }
    /// Main-thread equivalent of [`Plugin::get_editor_size`].
    pub fn get_editor_size(&self) -> Result<(i32, i32)> {
        self.plugin.get_editor_size()
    }
    /// Main-thread equivalent of [`Plugin::editor_can_resize`].
    pub fn editor_can_resize(&self) -> bool {
        self.plugin.editor_can_resize()
    }
    /// Main-thread equivalent of [`Plugin::resize_editor`].
    pub fn resize_editor(&mut self, width: i32, height: i32) -> Result<(i32, i32)> {
        self.plugin.resize_editor(width, height)
    }
    /// Main-thread equivalent of [`Plugin::set_editor_scale_factor`].
    pub fn set_editor_scale_factor(&mut self, factor: f32) -> Result<bool> {
        self.plugin.set_editor_scale_factor(factor)
    }
    /// Main-thread equivalent of [`Plugin::midi_panic`].
    pub fn midi_panic(&mut self) -> Result<()> {
        self.plugin.midi_panic()
    }
    /// Main-thread equivalent of [`Plugin::get_parameter_changes`].
    pub fn get_parameter_changes(&self) -> Vec<(u32, f64)> {
        self.plugin.get_parameter_changes()
    }
    /// Main-thread equivalent of [`Plugin::take_parameter_edits`].
    pub fn take_parameter_edits(&mut self) -> Vec<ParameterEdit> {
        self.plugin.take_parameter_edits()
    }
    /// Main-thread equivalent of [`Plugin::try_take_parameter_edits`].
    pub fn try_take_parameter_edits(&mut self) -> Result<Vec<ParameterEdit>> {
        self.plugin.try_take_parameter_edits()
    }
    /// Main-thread equivalent of [`Plugin::try_take_host_notifications`].
    pub fn try_take_host_notifications(&mut self) -> Result<Vec<HostNotification>> {
        self.plugin.try_take_host_notifications()
    }
    /// Main-thread equivalent of [`Plugin::native_dirty_revision`].
    pub fn native_dirty_revision(&mut self) -> Result<u64> {
        self.plugin.native_dirty_revision()
    }
    /// Main-thread equivalent of [`Plugin::take_host_notifications`].
    pub fn take_host_notifications(&mut self) -> Vec<HostNotification> {
        self.plugin.take_host_notifications()
    }
    /// Main-thread equivalent of [`Plugin::take_data_exchange_blocks`].
    pub fn take_data_exchange_blocks(&mut self) -> Vec<DataExchangeBlock> {
        self.plugin.take_data_exchange_blocks()
    }
    /// Main-thread equivalent of [`Plugin::execute_context_menu_item`].
    pub fn execute_context_menu_item(&mut self, menu_id: u64, item_id: u32) -> Result<()> {
        self.plugin.execute_context_menu_item(menu_id, item_id)
    }
    /// Main-thread equivalent of [`Plugin::dismiss_context_menu`].
    pub fn dismiss_context_menu(&mut self, menu_id: u64) -> Result<()> {
        self.plugin.dismiss_context_menu(menu_id)
    }
    /// Main-thread equivalent of [`Plugin::take_restart_flags`].
    pub fn take_restart_flags(&mut self) -> RestartFlags {
        self.plugin.take_restart_flags()
    }
    /// Main-thread equivalent of [`Plugin::service_host_requests`].
    pub fn service_host_requests(&mut self) -> Result<RestartFlags> {
        self.plugin.service_host_requests()
    }
    /// Main-thread equivalent of [`Plugin::take_output_midi`].
    pub fn take_output_midi(&self) -> Vec<MidiEvent> {
        self.plugin.take_output_midi()
    }
    /// Main-thread equivalent of [`Plugin::take_output_events`].
    pub fn take_output_events(&self) -> Vec<crate::midi::OutputEvent> {
        self.plugin.take_output_events()
    }
    /// Main-thread equivalent of [`Plugin::take_output_events_with_loss`].
    pub fn take_output_events_with_loss(&self) -> (Vec<crate::midi::OutputEvent>, bool) {
        self.plugin.take_output_events_with_loss()
    }
    /// Main-thread equivalent of [`Plugin::output_midi_handle`].
    pub fn output_midi_handle(&self) -> Option<OutputMidiConsumer> {
        self.plugin.output_midi_handle()
    }
    /// Main-thread equivalent of [`Plugin::output_event_handle`].
    pub fn output_event_handle(&self) -> Option<OutputEventConsumer> {
        self.plugin.output_event_handle()
    }
    /// Main-thread equivalent of [`Plugin::save_state`].
    pub fn save_state(&self) -> Result<Vec<u8>> {
        self.plugin.save_state()
    }
    /// Main-thread equivalent of [`Plugin::preflight_state_restore`].
    pub fn preflight_state_restore(&self) -> Result<()> {
        self.plugin.preflight_state_restore()
    }
    /// Main-thread equivalent of [`Plugin::load_state`].
    pub fn load_state(&mut self, data: &[u8]) -> Result<()> {
        self.plugin.load_state(data)
    }
    /// Main-thread equivalent of [`Plugin::load_state_with_context`].
    pub fn load_state_with_context(&mut self, data: &[u8], context: &StateContext) -> Result<()> {
        self.plugin.load_state_with_context(data, context)
    }
    /// Main-thread equivalent of [`Plugin::isolation_pid`].
    pub fn isolation_pid(&self) -> Option<u32> {
        self.plugin.isolation_pid()
    }
    /// Main-thread equivalent of [`Plugin::recovery_count`].
    pub fn recovery_count(&self) -> u64 {
        self.plugin.recovery_count()
    }
    /// Main-thread equivalent of [`Plugin::output_channel_count`].
    pub fn output_channel_count(&self) -> usize {
        self.plugin.output_channel_count()
    }
    /// Main-thread equivalent of [`Plugin::take_editor_resize_request`].
    pub fn take_editor_resize_request(&self) -> Option<(i32, i32)> {
        self.plugin.take_editor_resize_request()
    }
    /// Main-thread equivalent of [`Plugin::recover`].
    pub fn recover(&mut self) -> Result<()> {
        self.plugin.recover()
    }
}

impl Drop for MainThreadPlugin {
    fn drop(&mut self) {
        debug_assert_eq!(self.loading_thread, std::thread::current().id());
        // The private Plugin drops immediately after this on the same thread. Its existing
        // teardown detaches, stops, deactivates, disconnects and terminates before releasing
        // COM references and the module; single-component alias handling is unchanged.
    }
}

// Internal trait for hiding implementation details
pub(crate) trait PluginInternal: Send {
    /// Private synchronous domain visit. Other backends expose no local capabilities.
    fn with_domain_session(
        &mut self,
        _callback: &mut crate::internal::plugin_impl::domain_session::DomainSessionCallback<'_>,
    ) -> Result<()> {
        Err(Error::Other(
            "scoped local plugin domains are unsupported by this backend".to_string(),
        ))
    }

    fn set_parameter(&mut self, id: u32, value: f64) -> Result<()>;
    /// Schedule a parameter change at a sample offset within the next process block.
    /// Defaults to a block-start change (ignores the offset) for implementations that don't
    /// support sample-accurate scheduling.
    fn set_parameter_at(&mut self, id: u32, value: f64, _sample_offset: i32) -> Result<()> {
        self.set_parameter(id, value)
    }
    /// Queue automation for the processor without touching `IEditController`.
    ///
    /// Audio callbacks use this path because controller methods belong to the main-thread
    /// domain. Implementations without a split component/controller path may fall back to the
    /// ordinary setter.
    fn queue_processor_parameter_at(
        &mut self,
        id: u32,
        value: f64,
        sample_offset: i32,
    ) -> Result<()> {
        self.set_parameter_at(id, value, sample_offset)
    }
    fn get_parameter(&self, id: u32) -> Result<f64>;
    fn get_all_parameters(&self) -> Result<Vec<Parameter>>;
    fn format_parameter(&self, id: u32, normalized: f64) -> Result<String>;
    fn process(&mut self, buffers: &mut AudioBuffers) -> Result<()>;
    /// Query the current per-bus channel counts and activation state.
    fn audio_bus_layout(&self) -> Result<AudioBusLayout> {
        Err(Error::Other(
            "bus-aware audio processing is not supported for this plugin".to_string(),
        ))
    }
    /// Process buffers while preserving VST3 bus boundaries.
    fn process_buses(&mut self, _buffers: &mut BusAudioBuffers) -> Result<()> {
        Err(Error::Other(
            "bus-aware audio processing is not supported for this plugin".to_string(),
        ))
    }
    /// Re-run `setupProcessing` for a new sample rate / block size. Defaults to unsupported
    /// for implementations that don't support it.
    fn reconfigure(&mut self, _sample_rate: f64, _block_size: usize) -> Result<()> {
        Err(Error::Other(
            "runtime reconfigure is not supported for this plugin".to_string(),
        ))
    }
    /// Switch the plugin's process mode (real-time vs offline), re-running `setupProcessing`.
    /// Defaults to unsupported for implementations that don't support it.
    fn set_process_mode(&mut self, _mode: crate::plugin::ProcessMode) -> Result<()> {
        Err(Error::Other(
            "process mode switching is not supported for this plugin".to_string(),
        ))
    }
    /// Query each audio bus's current speaker arrangement. Defaults to unsupported.
    fn bus_arrangements(&self) -> Result<crate::audio::BusArrangements> {
        Err(Error::Other(
            "bus arrangement query is not supported for this plugin".to_string(),
        ))
    }
    /// Request specific speaker arrangements for the audio buses (re-runs `setupProcessing`).
    /// Defaults to unsupported for implementations that don't support it.
    fn set_bus_arrangements(
        &mut self,
        _inputs: &[crate::audio::SpeakerArrangement],
        _outputs: &[crate::audio::SpeakerArrangement],
    ) -> Result<()> {
        Err(Error::Other(
            "bus arrangement negotiation is not supported for this plugin".to_string(),
        ))
    }
    /// Activate or deactivate a single bus (`IComponent::activateBus`). Defaults to
    /// unsupported.
    fn set_bus_active(
        &mut self,
        _media_type: crate::audio::MediaType,
        _direction: crate::audio::BusDirection,
        _bus_index: i32,
        _active: bool,
    ) -> Result<()> {
        Err(Error::Other(
            "bus activation is not supported for this plugin".to_string(),
        ))
    }
    /// Apply independent authoritative sample and quarter-note positions for the next block.
    fn set_process_transport(&mut self, _transport: ProcessTransport) -> Result<()> {
        Err(Error::Other(
            "per-block transport is not supported for this plugin".into(),
        ))
    }
    /// Change the next block's tempo without changing its musical origin.
    fn set_tempo(&mut self, _bpm: f64) -> Result<()> {
        Err(Error::Other(
            "runtime transport mutation is not supported for this plugin".to_string(),
        ))
    }
    /// Update the transport time signature advertised in the host `ProcessContext`, taking
    /// effect on the next processed block. The caller validates the numerator/denominator.
    /// Defaults to unsupported.
    fn set_time_signature(&mut self, _numerator: i32, _denominator: i32) -> Result<()> {
        Err(Error::Other(
            "runtime transport mutation is not supported for this plugin".to_string(),
        ))
    }
    /// Toggle the transport playing state (`kPlaying`) in the host `ProcessContext`, taking
    /// effect on the next processed block. Defaults to unsupported.
    fn set_playing(&mut self, _playing: bool) -> Result<()> {
        Err(Error::Other(
            "runtime transport mutation is not supported for this plugin".to_string(),
        ))
    }
    fn send_midi_event(&mut self, event: MidiEvent) -> Result<()>;
    /// Schedule a MIDI event at a sample offset within the next process block.
    /// Defaults to a block-start event (ignores the offset) for implementations that don't
    /// support sample-accurate scheduling.
    fn send_midi_event_at(&mut self, event: MidiEvent, _sample_offset: i32) -> Result<()> {
        self.send_midi_event(event)
    }
    /// Send a fully owned VST3 event.
    fn send_plugin_event(&mut self, _event: PluginEvent) -> Result<()> {
        Err(Error::Other(
            "owned VST3 events are not supported for this plugin".to_string(),
        ))
    }
    /// Silence all notes currently tracked by the implementation.
    fn midi_panic(&mut self) -> Result<()> {
        for i in 0..16 {
            if let Some(channel) = MidiChannel::from_index(i) {
                for controller in [123, 120, 121] {
                    self.send_midi_event(MidiEvent::ControlChange {
                        channel,
                        controller,
                        value: 0,
                    })?;
                }
            }
        }
        Ok(())
    }
    /// Start a note and return a per-voice [`NoteId`] for targeting note-expression. Default:
    /// unsupported, for implementations that don't support per-note expression.
    fn note_on(
        &mut self,
        _channel: MidiChannel,
        _note: u8,
        _velocity: u8,
        _sample_offset: i32,
    ) -> Result<crate::midi::NoteId> {
        Err(Error::Other(
            "per-note expression is not supported for this plugin".to_string(),
        ))
    }
    /// Release a note started with [`Self::note_on`]. Default: unsupported.
    fn note_off(&mut self, _id: crate::midi::NoteId, _sample_offset: i32) -> Result<()> {
        Err(Error::Other(
            "per-note expression is not supported for this plugin".to_string(),
        ))
    }
    /// Send a per-note expression value (normalized 0..1) for a voice. Default: unsupported.
    fn send_note_expression(
        &mut self,
        _id: crate::midi::NoteId,
        _kind: crate::midi::NoteExpressionType,
        _value: f64,
        _sample_offset: i32,
    ) -> Result<()> {
        Err(Error::Other(
            "per-note expression is not supported for this plugin".to_string(),
        ))
    }
    /// Enumerate the per-note expressions the plugin advertises (`INoteExpressionController`).
    /// Defaults to empty.
    fn note_expressions(
        &self,
        _bus: i32,
        _channel: i16,
    ) -> Result<Vec<crate::midi::NoteExpressionInfo>> {
        Ok(Vec::new())
    }
    fn start_processing(&mut self) -> Result<()>;
    fn stop_processing(&mut self) -> Result<()>;
    /// Authoritative processing flag where available, including a partially failed stop.
    fn processing_state(&self) -> Option<bool> {
        None
    }
    fn has_editor(&self) -> bool;
    fn open_editor(&mut self, parent: *mut std::ffi::c_void) -> Result<()>;
    fn close_editor(&mut self) -> Result<()>;
    fn get_editor_size(&self) -> Result<(i32, i32)>;
    /// Control a native editor owned by an isolated helper. In-process implementations cannot
    /// satisfy this contract and keep returning an explicit error.
    fn isolated_editor(&mut self, _command: IsolatedEditorCommand) -> Result<IsolatedEditorState> {
        Err(Error::Other(
            "plugin is not hosted by an editor-capable isolated helper".to_string(),
        ))
    }
    /// Whether the editor accepts host-driven size changes.
    fn editor_can_resize(&self) -> bool {
        false
    }
    /// Ask the open editor to accept a host-driven size change. The returned size includes any
    /// constraint adjustment made by the plugin.
    fn resize_editor(&mut self, _width: i32, _height: i32) -> Result<(i32, i32)> {
        Err(Error::Other("plugin editor is not resizable".to_string()))
    }
    /// Set the editor's logical-to-physical content scale. Returns `false` when the view does not
    /// implement `IPlugViewContentScaleSupport`.
    fn set_editor_scale_factor(&mut self, _factor: f32) -> Result<bool> {
        Ok(false)
    }
    /// Service the Linux `IRunLoop` registrations the plugin's editor made
    /// (fire due timers, dispatch ready file descriptors). No-op by default
    /// (non-Linux, or process isolation where the editor isn't bridged).
    fn service_run_loop(&mut self) {}
    fn get_parameter_changes(&self) -> Vec<(u32, f64)>;
    /// Drain the ordered parameter-edit gesture log (begin/change/end) the plugin's editor
    /// reported since the last call. Defaults to empty for implementations that don't capture
    /// gestures.
    fn take_parameter_edits(&mut self) -> Vec<ParameterEdit> {
        Vec::new()
    }
    /// Drain ordered requests reported through `IComponentHandler2`.
    fn take_host_notifications(&mut self) -> Vec<HostNotification> {
        Vec::new()
    }
    /// Checked editor drain for hosts whose dirty-state accounting must not treat IPC failure
    /// as an empty edit batch. In-process implementations have no transport failure.
    fn try_take_parameter_edits(&mut self) -> Result<Vec<ParameterEdit>> {
        Ok(self.take_parameter_edits())
    }
    /// Checked host-notification drain; preserves errors across the isolation boundary.
    fn try_take_host_notifications(&mut self) -> Result<Vec<HostNotification>> {
        Ok(self.take_host_notifications())
    }
    /// Non-draining native-state change revision; unsupported backends fail explicitly.
    fn native_dirty_revision(&mut self) -> Result<u64> {
        Err(Error::Other("native dirty revision is unavailable".into()))
    }
    /// Dispatch any main-thread data-exchange blocks to the controller and drain owned host
    /// snapshots. Background-dispatched queues are copied into the same bounded snapshot sink.
    fn take_data_exchange_blocks(&mut self) -> Vec<DataExchangeBlock> {
        Vec::new()
    }
    /// Execute one entry from a pending plugin context-menu popup.
    fn execute_context_menu_item(&mut self, _menu_id: u64, _item_id: u32) -> Result<()> {
        Err(Error::Other(
            "plugin context menus are not supported".to_string(),
        ))
    }
    /// Dismiss a pending plugin context-menu popup without choosing an entry.
    fn dismiss_context_menu(&mut self, _menu_id: u64) -> Result<()> {
        Err(Error::Other(
            "plugin context menus are not supported".to_string(),
        ))
    }
    /// Take the `restartComponent` flags the plugin has raised since the last call. Defaults to
    /// empty for implementations that don't record them.
    fn take_restart_flags(&mut self) -> RestartFlags {
        RestartFlags::default()
    }
    /// Drain and service restart requests which require a component lifecycle transition.
    fn service_host_requests(&mut self) -> Result<RestartFlags> {
        Ok(self.take_restart_flags())
    }
    /// Take the MIDI events the plugin has emitted since the last call. Defaults to empty
    /// for implementations that don't capture output MIDI.
    fn take_output_events(&self) -> Vec<PluginEvent> {
        Vec::new()
    }
    /// Drain emitted events and acknowledge a latched capture/queue loss indication.
    fn take_output_events_with_loss(&self) -> (Vec<PluginEvent>, bool) {
        (self.take_output_events(), false)
    }
    /// Lock-free output consumer, when an in-process shared queue is available.
    fn output_midi_handle(&self) -> Option<OutputMidiConsumer> {
        None
    }
    fn output_event_handle(&self) -> Option<OutputEventConsumer> {
        None
    }
    /// Enumerate the plugin's units and their program lists (`IUnitInfo`). Defaults to empty
    /// for implementations that don't query it.
    fn get_units(&self) -> Result<Vec<PluginUnit>> {
        Ok(Vec::new())
    }
    /// Select a program in a unit's program list. Defaults to unsupported (e.g. plugins
    /// without `IUnitInfo`); implementations resolve the unit's program-change parameter and
    /// set it to the index's normalized value.
    fn select_program(&mut self, _unit_id: i32, _program_index: i32) -> Result<()> {
        Err(Error::Other(
            "program selection is not supported for this plugin".to_string(),
        ))
    }
    fn selected_unit(&self) -> Result<Option<i32>> {
        Ok(None)
    }
    fn select_unit(&mut self, _unit_id: i32) -> Result<()> {
        Err(Error::Other(
            "unit selection is not supported for this plugin".to_string(),
        ))
    }
    fn program_pitch_names(
        &self,
        _program_list_id: i32,
        _program_index: i32,
    ) -> Result<Vec<ProgramPitchName>> {
        Ok(Vec::new())
    }
    fn get_program_data(
        &self,
        _program_list_id: i32,
        _program_index: i32,
    ) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
    fn set_program_data(
        &mut self,
        _program_list_id: i32,
        _program_index: i32,
        _data: &[u8],
    ) -> Result<()> {
        Err(Error::Other(
            "program data is not supported for this plugin".to_string(),
        ))
    }
    fn get_unit_data(&self, _unit_id: i32) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
    fn set_unit_data(&mut self, _unit_id: i32, _data: &[u8]) -> Result<()> {
        Err(Error::Other(
            "unit data is not supported for this plugin".to_string(),
        ))
    }
    fn begin_host_edit(&mut self, _parameter_id: u32) -> Result<()> {
        Err(Error::Other(
            "host edit sessions are not supported for this plugin".to_string(),
        ))
    }
    fn end_host_edit(&mut self, _parameter_id: u32) -> Result<()> {
        Err(Error::Other(
            "host edit sessions are not supported for this plugin".to_string(),
        ))
    }
    fn send_midi_learn(&mut self, _bus: i32, _channel: i16, _controller: u16) -> Result<()> {
        Err(Error::Other(
            "MIDI learn is not supported for this plugin".to_string(),
        ))
    }
    fn set_automation_state(&mut self, _state: AutomationState) -> Result<()> {
        Err(Error::Other(
            "automation state is not supported for this plugin".to_string(),
        ))
    }
    /// Ask the controller to map a parameter id from a plugin class it replaces.
    fn remap_parameter_id(&self, _old_plugin_uid: &str, _old_param_id: u32) -> Result<Option<u32>> {
        Ok(None)
    }
    /// Processing latency in samples (`IAudioProcessor::getLatencySamples`). Defaults to 0.
    fn latency_samples(&self) -> u32 {
        0
    }
    /// Tail length in samples (`IAudioProcessor::getTailSamples`). Defaults to 0.
    fn tail_samples(&self) -> u32 {
        0
    }
    /// Resolve a MIDI controller `(bus, channel, cc)` to a parameter id via `IMidiMapping`.
    /// Defaults to `None` (plugin doesn't implement the interface, or no mapping).
    fn midi_cc_to_parameter(&self, _bus: i32, _channel: i16, _cc: u16) -> Option<u32> {
        None
    }
    /// Serialize the plugin's current state to an opaque byte blob.
    fn save_state(&self) -> Result<Vec<u8>> {
        Err(Error::Other(
            "state save/restore is not supported".to_string(),
        ))
    }
    /// Local compatibility check before a caller detaches the native editor.
    fn preflight_state_restore(&self) -> Result<()> {
        Ok(())
    }

    /// Restore the plugin's state from a blob previously returned by [`Self::save_state`],
    /// telling the plugin what kind of restore this is via the stream's attributes.
    ///
    /// This is the method implementors override; [`Self::load_state`] is the project-restore
    /// shorthand that delegates here.
    fn load_state_with_context(&mut self, _data: &[u8], _context: &StateContext) -> Result<()> {
        Err(Error::Other(
            "state save/restore is not supported".to_string(),
        ))
    }
    /// Restore the plugin's state from a blob previously returned by [`Self::save_state`],
    /// as a project/session restore ([`StateContext::Project`]).
    fn load_state(&mut self, data: &[u8]) -> Result<()> {
        self.load_state_with_context(data, &StateContext::Project)
    }
    /// OS process id of the isolated helper, if this plugin runs out-of-process.
    fn helper_pid(&self) -> Option<u32> {
        None
    }
    /// Number of times this plugin has been recovered (respawned + reloaded). Defaults to 0
    /// for non-isolated plugins.
    fn recovery_count(&self) -> u64 {
        0
    }
    /// Recover from a crashed isolated helper by respawning and reloading. Only meaningful
    /// for process-isolated plugins.
    fn recover(&mut self) -> Result<()> {
        Err(Error::Other(
            "recovery is only supported for process-isolated plugins".to_string(),
        ))
    }
    /// The size the plugin's editor has requested (via `IPlugFrame`) since the last poll.
    fn take_editor_resize_request(&self) -> Option<(i32, i32)> {
        None
    }
    /// Total output audio channels across the plugin's output buses. Defaults to 2.
    fn output_channel_count(&self) -> usize {
        2
    }
}

impl Plugin {
    /// Get plugin information
    pub fn info(&self) -> &PluginInfo {
        &self.info
    }

    /// Current/retired class-id replacement mappings advertised by this plug-in.
    ///
    /// These come from `moduleinfo.json` when present, otherwise from the factory's optional
    /// `IPluginCompatibility` class.
    pub fn class_compatibility(&self) -> &[crate::discovery::ClassCompatibility] {
        &self.compatibility
    }

    /// Retired class ids which this loaded audio class replaces.
    pub fn replaced_class_ids(&self) -> &[String] {
        self.compatibility
            .iter()
            .find(|mapping| {
                crate::internal::utils::class_uid_matches(&mapping.new_class_id, &self.info.uid)
            })
            .map_or(&[], |mapping| mapping.old_class_ids.as_slice())
    }

    /// The sample rate (Hz) this plugin was configured with at load.
    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// The maximum block size (frames per `process_audio` call) configured at load.
    pub fn block_size(&self) -> usize {
        self.block_size
    }

    /// Reconfigure the plugin for a new sample rate and/or maximum block size, re-running the
    /// plugin's `setupProcessing` and rebuilding its audio buffers.
    ///
    /// Use this when the audio device's sample rate changes mid-session instead of reloading.
    /// The plugin must **not** be processing: call [`Self::stop_processing`] first, reconfigure,
    /// then [`Self::start_processing`] again. Returns an error if called while processing, or
    /// on an invalid sample rate / zero block size. Works both in-process and across process
    /// isolation.
    pub fn reconfigure(&mut self, sample_rate: f64, block_size: usize) -> Result<()> {
        if self.is_processing {
            return Err(Error::Other(
                "cannot reconfigure while processing; call stop_processing() first".to_string(),
            ));
        }
        if !(sample_rate.is_finite() && sample_rate > 0.0) {
            return Err(Error::InvalidParameter(format!(
                "sample rate must be finite and positive, got {sample_rate}"
            )));
        }
        if block_size == 0 || block_size > i32::MAX as usize {
            return Err(Error::InvalidParameter(format!(
                "block size must be in 1..={}, got {block_size}",
                i32::MAX
            )));
        }

        self.internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .reconfigure(sample_rate, block_size)?;

        self.sample_rate = sample_rate;
        self.block_size = block_size;
        Ok(())
    }

    /// Switch the plugin between real-time and offline processing, re-running the plugin's
    /// `setupProcessing` so it can adjust quality / look-ahead for a faster-than-real-time
    /// bounce.
    ///
    /// Like [`Self::reconfigure`], the plugin must **not** be processing: call
    /// [`Self::stop_processing`] first. Returns an error if called while processing. Works both
    /// in-process and across process isolation.
    pub fn set_process_mode(&mut self, mode: ProcessMode) -> Result<()> {
        if self.is_processing {
            return Err(Error::Other(
                "cannot set process mode while processing; call stop_processing() first"
                    .to_string(),
            ));
        }
        self.internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .set_process_mode(mode)
    }

    /// Query the current speaker arrangement of each audio input/output bus. Works both
    /// in-process and across process isolation.
    pub fn bus_arrangements(&self) -> Result<crate::audio::BusArrangements> {
        self.internal
            .as_ref()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .bus_arrangements()
    }

    /// Request specific speaker arrangements for the audio buses (e.g. force stereo, or a
    /// surround layout). The slices give one [`SpeakerArrangement`](crate::audio::SpeakerArrangement)
    /// per input bus and per output bus, in bus-index order.
    ///
    /// Re-runs the plugin's `setupProcessing`, so the plugin must **not** be processing (call
    /// [`Self::stop_processing`] first). A plugin may decline a requested layout and keep its
    /// own; re-query with [`Self::bus_arrangements`] to see what was actually applied. Errors
    /// while processing. Works both in-process and across process isolation.
    pub fn set_bus_arrangements(
        &mut self,
        inputs: &[crate::audio::SpeakerArrangement],
        outputs: &[crate::audio::SpeakerArrangement],
    ) -> Result<()> {
        if self.is_processing {
            return Err(Error::Other(
                "cannot set bus arrangements while processing; call stop_processing() first"
                    .to_string(),
            ));
        }
        self.internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .set_bus_arrangements(inputs, outputs)
    }

    /// Activate or deactivate a single bus on the plugin (`IComponent::activateBus`).
    ///
    /// Hosts must explicitly activate the buses they intend to use; a plugin's secondary
    /// buses (sidechain / aux inputs, extra outputs) commonly start **inactive** and only
    /// receive/produce audio once activated. (The load sequence already activates the main
    /// audio and event buses, so call this to enable the rest.)
    ///
    /// `media_type` selects audio vs event buses and `direction` selects input vs output;
    /// `bus_index` is the 0-based index within that `(media_type, direction)` group (the
    /// same indexing as [`crate::discovery::BusLayout`]). `active` true activates, false
    /// deactivates.
    ///
    /// VST3 requires bus activation to happen while the component is **inactive** — i.e.
    /// before processing starts. This therefore returns an error if called while the plugin
    /// is processing; call [`Self::stop_processing`] first, activate the bus, then
    /// [`Self::start_processing`] again. Returns an error for an out-of-range `bus_index`,
    /// and under process isolation activation marshals across the boundary.
    pub fn set_bus_active(
        &mut self,
        media_type: crate::audio::MediaType,
        direction: crate::audio::BusDirection,
        bus_index: i32,
        active: bool,
    ) -> Result<()> {
        if self.is_processing {
            return Err(Error::Other(
                "cannot activate a bus while processing; call stop_processing() first".to_string(),
            ));
        }
        self.internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .set_bus_active(media_type, direction, bus_index, active)
    }

    /// Get all parameters
    pub fn get_parameters(&self) -> Result<Vec<Parameter>> {
        self.internal
            .as_ref()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .get_all_parameters()
    }

    /// Set a parameter value by ID
    pub fn set_parameter(&mut self, id: u32, value: f64) -> Result<()> {
        if !(0.0..=1.0).contains(&value) {
            return Err(Error::InvalidParameter(format!(
                "Value {} is out of range [0.0, 1.0]",
                value
            )));
        }

        self.internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .set_parameter(id, value)?;

        // Trigger callback if set
        if let Some(ref callback) = self.parameter_change_callback {
            callback(id, value);
        }

        Ok(())
    }

    /// Set a parameter value at a specific sample offset within the next process block.
    ///
    /// This is the sample-accurate building block for automation: call it once per
    /// sub-block point (e.g. from [`ParameterAutomation::points_for_block`]) and the plugin
    /// receives the changes at their offsets in the next `process_audio`. Like
    /// [`Self::set_parameter`], `value` is normalized `0.0..=1.0`.
    ///
    /// `sample_offset` is clamped to the block. Under process isolation the offset **is** now
    /// carried across the boundary and applied by the helper's in-process plugin.
    ///
    /// [`ParameterAutomation::points_for_block`]: crate::parameters::ParameterAutomation::points_for_block
    pub fn set_parameter_at(&mut self, id: u32, value: f64, sample_offset: i32) -> Result<()> {
        if !(0.0..=1.0).contains(&value) {
            return Err(Error::InvalidParameter(format!(
                "Value {} is out of range [0.0, 1.0]",
                value
            )));
        }
        self.internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .set_parameter_at(id, value, sample_offset)
    }

    /// Audio-thread automation path: queue a processor point without invoking the controller
    /// or user callback. Values are validated on the control thread before commands enter the
    /// playback rings; this defensive check keeps direct internal callers honest.
    pub(crate) fn queue_processor_parameter_at(
        &mut self,
        id: u32,
        value: f64,
        sample_offset: i32,
    ) -> Result<()> {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(Error::InvalidParameter(format!(
                "Value {} is out of range [0.0, 1.0]",
                value
            )));
        }
        self.internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .queue_processor_parameter_at(id, value, sample_offset)
    }

    /// Set the authoritative starting transport for the next processed block.
    ///
    /// Works in-process and in isolation. The isolated implementation includes the context
    /// in the audio request itself, without another per-block IPC exchange. Rejects non-finite
    /// musical position/tempo, non-positive tempo/numerator, and unsupported denominators.
    pub fn set_process_transport(&mut self, transport: ProcessTransport) -> Result<()> {
        transport.validate()?;
        self.internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .set_process_transport(transport)
    }

    /// Change the transport tempo (beats per minute) advertised to the plugin in the host
    /// `ProcessContext`, taking effect on the **next** processed block — even while the plugin
    /// is actively processing. Drives tempo-synced DSP (LFOs, synced delays, arpeggiators).
    ///
    /// `bpm` must be finite and greater than `0` (a non-positive tempo would freeze or reverse
    /// the derived musical playhead). Works both in-process and across process isolation.
    pub fn set_tempo(&mut self, bpm: f64) -> Result<()> {
        if !(bpm.is_finite() && bpm > 0.0) {
            return Err(Error::InvalidParameter(format!(
                "tempo must be finite and positive, got {bpm}"
            )));
        }
        self.internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .set_tempo(bpm)
    }

    /// Change the transport time signature advertised to the plugin in the host
    /// `ProcessContext` (`numerator`/`denominator`, e.g. `7, 8`), taking effect on the
    /// **next** processed block — even while the plugin is actively processing.
    ///
    /// `numerator` must be greater than `0` and `denominator` must be a power of two between
    /// `1` and `16` (`1`, `2`, `4`, `8`, or `16`) — the standard note values a time signature
    /// can denominate. Works both in-process and across process isolation.
    pub fn set_time_signature(&mut self, numerator: i32, denominator: i32) -> Result<()> {
        if numerator <= 0 {
            return Err(Error::InvalidParameter(format!(
                "time signature numerator must be positive, got {numerator}"
            )));
        }
        if !matches!(denominator, 1 | 2 | 4 | 8 | 16) {
            return Err(Error::InvalidParameter(format!(
                "time signature denominator must be one of 1, 2, 4, 8, 16, got {denominator}"
            )));
        }
        self.internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .set_time_signature(numerator, denominator)
    }

    /// Toggle the transport playing state advertised to the plugin in the host
    /// `ProcessContext` (the `kPlaying` flag), taking effect on the **next** processed block —
    /// even while the plugin is actively processing.
    ///
    /// While stopped, project sample/quarter-note positions stay fixed. Continuous processing
    /// time still advances. Works both in-process and across process isolation.
    pub fn set_playing(&mut self, playing: bool) -> Result<()> {
        self.internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .set_playing(playing)
    }

    /// Enumerate the plugin's units and their program lists (`IUnitInfo`).
    ///
    /// Returns an empty list for plugins that don't implement `IUnitInfo`. The root unit (id
    /// `0`) is typically present. Works both in-process and across process isolation.
    pub fn get_units(&self) -> Result<Vec<PluginUnit>> {
        self.internal
            .as_ref()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .get_units()
    }

    /// Select a program (preset) in a unit's program list (`IUnitInfo`).
    ///
    /// `unit_id` is a [`PluginUnit::id`] from [`get_units`](Self::get_units) (the root unit is
    /// `0`); `program_index` is a 0-based index into that unit's [`PluginUnit::programs`].
    /// Internally this locates the unit's program-change parameter (the controller parameter
    /// tied to the unit with the VST3 `kIsProgramChange` flag) and sets it to the normalized
    /// value `program_index / max(1, program_count - 1)`, driving both the controller (for the
    /// editor/display) and the processor (for the audio DSP).
    ///
    /// Returns an error for an unknown unit, a unit with no program list, an out-of-range
    /// index, a plugin that doesn't implement `IUnitInfo`, or a plugin running under process
    /// isolation only if the helper cannot resolve the unit. Works both in-process and across
    /// the isolation boundary.
    pub fn select_program(&mut self, unit_id: i32, program_index: i32) -> Result<()> {
        self.internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .select_program(unit_id, program_index)
    }

    /// Return the unit currently selected by the plugin, or `None` without `IUnitInfo`.
    pub fn selected_unit(&self) -> Result<Option<i32>> {
        self.internal
            .as_ref()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .selected_unit()
    }

    /// Select a unit through `IUnitInfo::selectUnit`.
    pub fn select_unit(&mut self, unit_id: i32) -> Result<()> {
        self.internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .select_unit(unit_id)
    }

    /// Query the plugin's MIDI-pitch names for one program.
    pub fn program_pitch_names(
        &self,
        program_list_id: i32,
        program_index: i32,
    ) -> Result<Vec<ProgramPitchName>> {
        self.internal
            .as_ref()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .program_pitch_names(program_list_id, program_index)
    }

    /// Read opaque per-program data, or `None` when `IProgramListData` is absent/unsupported.
    pub fn get_program_data(
        &self,
        program_list_id: i32,
        program_index: i32,
    ) -> Result<Option<Vec<u8>>> {
        self.internal
            .as_ref()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .get_program_data(program_list_id, program_index)
    }

    /// Restore opaque per-program data through `IProgramListData`.
    pub fn set_program_data(
        &mut self,
        program_list_id: i32,
        program_index: i32,
        data: &[u8],
    ) -> Result<()> {
        self.internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .set_program_data(program_list_id, program_index, data)
    }

    /// Read opaque per-unit data, or `None` when `IUnitData` is absent/unsupported.
    pub fn get_unit_data(&self, unit_id: i32) -> Result<Option<Vec<u8>>> {
        self.internal
            .as_ref()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .get_unit_data(unit_id)
    }

    /// Restore opaque per-unit data through `IUnitData`.
    pub fn set_unit_data(&mut self, unit_id: i32, data: &[u8]) -> Result<()> {
        self.internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .set_unit_data(unit_id, data)
    }

    /// Begin a controller-side host edit session for a parameter.
    pub fn begin_host_edit(&mut self, parameter_id: u32) -> Result<()> {
        self.internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .begin_host_edit(parameter_id)
    }

    /// End a controller-side host edit session previously begun with [`Self::begin_host_edit`].
    pub fn end_host_edit(&mut self, parameter_id: u32) -> Result<()> {
        self.internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .end_host_edit(parameter_id)
    }

    /// Notify a controller implementing `IMidiLearn` of live MIDI-controller input.
    pub fn send_midi_learn(&mut self, bus: i32, channel: i16, controller: u16) -> Result<()> {
        self.internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .send_midi_learn(bus, channel, controller)
    }

    /// Report the host's automation mode to a controller implementing `IAutomationState`.
    pub fn set_automation_state(&mut self, state: AutomationState) -> Result<()> {
        self.internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .set_automation_state(state)
    }

    /// Map a parameter id from an older/replaced plugin class through `IRemapParamID`.
    ///
    /// `old_plugin_uid` must be the canonical separator-free 32-hex-character VST3 class id.
    /// Returns `None` when the controller does not implement remapping or has no mapping for
    /// this class/id pair. On Windows the canonical id is converted to COM-compatible byte
    /// order before the controller is called. Works both in-process and across isolation.
    pub fn remap_parameter_id(
        &self,
        old_plugin_uid: &str,
        old_param_id: u32,
    ) -> Result<Option<u32>> {
        if crate::internal::utils::parse_class_uid(old_plugin_uid).is_none() {
            return Err(Error::InvalidParameter(
                "plugin UID must contain exactly 32 hexadecimal characters".to_string(),
            ));
        }
        self.internal
            .as_ref()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .remap_parameter_id(old_plugin_uid, old_param_id)
    }

    /// The plugin's reported processing latency in samples (e.g. from look-ahead or
    /// oversampling), via `IAudioProcessor::getLatencySamples`. Use it to delay-compensate
    /// when aligning the plugin's output with other signals. `0` if it reports none. Works
    /// both in-process and across process isolation.
    pub fn latency_samples(&self) -> u32 {
        self.internal
            .as_ref()
            .map(|i| i.latency_samples())
            .unwrap_or(0)
    }

    /// The plugin's reported tail length in samples (how long it keeps producing output
    /// after input stops — e.g. reverb/delay), via `IAudioProcessor::getTailSamples`. `0`
    /// means no tail; `u32::MAX` means an infinite tail. Works both in-process and across
    /// process isolation.
    pub fn tail_samples(&self) -> u32 {
        self.internal
            .as_ref()
            .map(|i| i.tail_samples())
            .unwrap_or(0)
    }

    /// Resolve a MIDI controller to the parameter it's mapped to, via the plugin's
    /// `IMidiMapping` (`getMidiControllerAssignment`).
    ///
    /// `bus` is the event input bus index (usually `0`), `channel` the 0-based MIDI channel,
    /// and `cc` the MIDI controller number (`0–127`, or the VST3 specials such as `128`
    /// aftertouch / `129` pitch-bend). Returns the parameter id the controller drives, or
    /// `None` if the plugin doesn't implement `IMidiMapping` or the controller is unmapped.
    /// Works both in-process and across process isolation.
    pub fn midi_cc_to_parameter(&self, bus: i32, channel: i16, cc: u16) -> Option<u32> {
        // VST3 controller numbers are 0..130 (0–127 MIDI CCs + the specials up to pitch-bend).
        // Reject out-of-range values rather than forwarding a meaningless controller number.
        if cc > 129 {
            return None;
        }
        self.internal
            .as_ref()?
            .midi_cc_to_parameter(bus, channel, cc)
    }

    /// Get a parameter value by ID
    pub fn get_parameter(&self, id: u32) -> Result<f64> {
        self.internal
            .as_ref()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .get_parameter(id)
    }

    /// Format a parameter value as the plugin itself would display it.
    ///
    /// VST3 keeps all parameter values normalized (0.0–1.0) and delegates
    /// human-readable formatting to the plugin's controller. This asks the plugin to
    /// render `normalized` for parameter `id`, returning exactly what its own UI would
    /// show — e.g. `"440.00 Hz"`, `"-6.0 dB"`, `"Sine"`. Prefer this over
    /// [`Parameter::format_value`], which can only approximate without the plugin's
    /// internal mapping.
    pub fn format_parameter(&self, id: u32, normalized: f64) -> Result<String> {
        self.internal
            .as_ref()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .format_parameter(id, normalized)
    }

    /// Set a parameter by name
    pub fn set_parameter_by_name(&mut self, name: &str, value: f64) -> Result<()> {
        let params = self.get_parameters()?;
        let param = params
            .iter()
            .find(|p| p.name == name)
            .ok_or_else(|| Error::InvalidParameter(format!("Parameter '{}' not found", name)))?;

        self.set_parameter(param.id, value)
    }

    /// Find a parameter by name
    pub fn find_parameter(&self, name: &str) -> Result<Parameter> {
        let params = self.get_parameters()?;
        params
            .into_iter()
            .find(|p| p.name == name)
            .ok_or_else(|| Error::InvalidParameter(format!("Parameter '{}' not found", name)))
    }

    /// Send a MIDI note on event
    pub fn send_midi_note(&mut self, note: u8, velocity: u8, channel: MidiChannel) -> Result<()> {
        validate_note(note)?;
        validate_velocity(velocity)?;

        let event = MidiEvent::NoteOn {
            channel,
            note,
            velocity,
        };
        self.send_midi_event(event)
    }

    /// Send a MIDI note off event
    pub fn send_midi_note_off(&mut self, note: u8, channel: MidiChannel) -> Result<()> {
        validate_note(note)?;

        let event = MidiEvent::NoteOff {
            channel,
            note,
            velocity: 0,
        };
        self.send_midi_event(event)
    }

    /// Send a MIDI control change event
    pub fn send_midi_cc(&mut self, controller: u8, value: u8, channel: MidiChannel) -> Result<()> {
        validate_controller(controller)?;
        validate_cc_value(value)?;

        let event = MidiEvent::ControlChange {
            channel,
            controller,
            value,
        };
        self.send_midi_event(event)
    }

    /// Send a generic MIDI event.
    ///
    /// Every data field is range-checked against the MIDI spec (`0–127`, or `0–16383` for
    /// pitch bend) before the event reaches the plugin, the same way
    /// [`send_midi_note`](Self::send_midi_note) and [`send_midi_cc`](Self::send_midi_cc)
    /// check theirs.
    pub fn send_midi_event(&mut self, event: MidiEvent) -> Result<()> {
        validate_midi_event(&event)?;
        self.internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .send_midi_event(event)
    }

    /// Schedule a MIDI event at a sample offset within the **next** [`process_audio`] block.
    ///
    /// Use this for sample-accurate sequencing: an event sent with `sample_offset = N` takes
    /// effect `N` frames into the next processed block, rather than at its start. Keep the
    /// offset within the upcoming block's frame count ([`Plugin::block_size`] is the maximum);
    /// a negative offset is treated as 0, and an offset past the block end is plugin-defined.
    ///
    /// Works both in-process and across process isolation — the offset is carried across the
    /// boundary and applied by the helper's in-process plugin. The event's data fields are
    /// range-checked exactly as in [`send_midi_event`](Self::send_midi_event).
    ///
    /// [`process_audio`]: Self::process_audio
    pub fn send_midi_event_at(&mut self, event: MidiEvent, sample_offset: i32) -> Result<()> {
        validate_midi_event(&event)?;
        self.internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .send_midi_event_at(event, sample_offset)
    }

    /// Send a fully owned VST3 event.
    ///
    /// This is the lossless event path for SysEx, note-expression text/integer values, chord,
    /// and scale events. Pointer-backed data is owned by `event` and kept alive until the plugin
    /// has consumed it.
    pub fn send_plugin_event(&mut self, event: PluginEvent) -> Result<()> {
        validate_plugin_event(&event)?;
        self.internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .send_plugin_event(event)
    }

    /// Send MIDI SysEx bytes at block start.
    pub fn send_sysex(&mut self, bytes: Vec<u8>) -> Result<()> {
        self.send_plugin_event(PluginEvent::sysex(bytes))
    }

    /// Send MIDI SysEx bytes at a sample offset within the next process block.
    pub fn send_sysex_at(&mut self, bytes: Vec<u8>, sample_offset: i32) -> Result<()> {
        self.send_plugin_event(PluginEvent::sysex(bytes).at(sample_offset))
    }

    /// Start a note and get a per-voice [`NoteId`](crate::midi::NoteId) handle for sending
    /// per-note (MPE-style) expression to that exact voice via
    /// [`send_note_expression`](Self::send_note_expression).
    ///
    /// Unlike [`send_midi_note`](Self::send_midi_note) (which uses a shared note id and can't be
    /// individually expressed), this allocates a unique voice id. Pair it with
    /// [`note_off`](Self::note_off). Per-note expression works both in-process and under
    /// process isolation — the calls marshal across the boundary.
    ///
    /// `note` and `velocity` are range-checked (`0–127`), as in
    /// [`send_midi_note`](Self::send_midi_note).
    pub fn note_on(
        &mut self,
        channel: MidiChannel,
        note: u8,
        velocity: u8,
    ) -> Result<crate::midi::NoteId> {
        self.note_on_at(channel, note, velocity, 0)
    }

    /// [`note_on`](Self::note_on) scheduled at a sample offset within the next block.
    ///
    /// `note` and `velocity` must be `0–127`, as for
    /// [`send_midi_note`](Self::send_midi_note).
    pub fn note_on_at(
        &mut self,
        channel: MidiChannel,
        note: u8,
        velocity: u8,
        sample_offset: i32,
    ) -> Result<crate::midi::NoteId> {
        validate_note(note)?;
        validate_velocity(velocity)?;
        self.internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .note_on(channel, note, velocity, sample_offset)
    }

    /// Release a note started with [`note_on`](Self::note_on).
    pub fn note_off(&mut self, id: crate::midi::NoteId) -> Result<()> {
        self.note_off_at(id, 0)
    }

    /// [`note_off`](Self::note_off) scheduled at a sample offset within the next block.
    pub fn note_off_at(&mut self, id: crate::midi::NoteId, sample_offset: i32) -> Result<()> {
        self.internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .note_off(id, sample_offset)
    }

    /// Send a per-note expression value for a voice (normalized `0.0..=1.0`; bipolar dimensions
    /// like [`Tuning`](crate::midi::NoteExpressionType::Tuning) center at `0.5`). The plugin
    /// must implement `INoteExpressionController` and the dimension must be one it advertises
    /// (see [`note_expressions`](Self::note_expressions)).
    pub fn send_note_expression(
        &mut self,
        id: crate::midi::NoteId,
        kind: crate::midi::NoteExpressionType,
        value: f64,
    ) -> Result<()> {
        self.send_note_expression_at(id, kind, value, 0)
    }

    /// [`send_note_expression`](Self::send_note_expression) scheduled at a sample offset.
    pub fn send_note_expression_at(
        &mut self,
        id: crate::midi::NoteId,
        kind: crate::midi::NoteExpressionType,
        value: f64,
        sample_offset: i32,
    ) -> Result<()> {
        if !(0.0..=1.0).contains(&value) {
            return Err(Error::InvalidParameter(format!(
                "note-expression value {value} out of range [0.0, 1.0]"
            )));
        }
        self.internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .send_note_expression(id, kind, value, sample_offset)
    }

    /// Enumerate the per-note expression dimensions the plugin advertises for the given event
    /// bus / channel (defaults: bus 0, channel 0), via `INoteExpressionController`. Empty if the
    /// plugin doesn't implement it.
    pub fn note_expressions(&self) -> Result<Vec<crate::midi::NoteExpressionInfo>> {
        self.internal
            .as_ref()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .note_expressions(0, 0)
    }

    /// Start audio processing
    pub fn start_processing(&mut self) -> Result<()> {
        if self.is_processing {
            return Ok(());
        }

        self.internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .start_processing()?;

        self.is_processing = true;
        Ok(())
    }

    /// Stop audio processing
    pub fn stop_processing(&mut self) -> Result<()> {
        if !self.is_processing {
            return Ok(());
        }

        let internal = self
            .internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?;
        let result = internal.stop_processing();
        if let Some(processing) = internal.processing_state() {
            self.is_processing = processing;
        }
        result?;

        self.is_processing = false;
        Ok(())
    }

    /// Process audio buffers.
    ///
    /// # Thread safety
    ///
    /// Both playback paths call this from the **audio thread**, so any callback registered
    /// with [`Self::on_audio_process`] runs there too — see that method's warning.
    pub fn process_audio(&mut self, buffers: &mut AudioBuffers) -> Result<()> {
        if !self.is_processing {
            return Err(Error::NotProcessing);
        }

        self.internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .process(buffers)?;

        // Update audio levels
        if let Ok(mut levels) = self.audio_levels.lock() {
            levels.update_from_buffers(&buffers.outputs);

            // Trigger audio callback if set
            if let Some(ref callback) = self.audio_callback {
                callback(&levels);
            }
        }

        Ok(())
    }

    /// Return every audio bus's current channel count and activation state.
    pub fn audio_bus_layout(&self) -> Result<AudioBusLayout> {
        self.internal
            .as_ref()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .audio_bus_layout()
    }

    /// Allocate a bus-aware silent buffer set matching the plug-in's current configuration.
    ///
    /// Do this on the control thread when configuring an audio stream, then reuse the returned
    /// storage for every callback. If bus activation or arrangements change, query/create again.
    pub fn create_bus_audio_buffers(&self, block_size: usize) -> Result<BusAudioBuffers> {
        if block_size == 0 {
            return Err(Error::Other(
                "bus audio block size must be greater than zero".to_string(),
            ));
        }
        Ok(BusAudioBuffers::new(
            &self.audio_bus_layout()?,
            block_size,
            self.sample_rate,
        ))
    }

    /// Process audio without flattening VST3 bus boundaries.
    ///
    /// The buffer set must contain every bus in index order, including inactive buses. Its
    /// activation flags and channel counts are validated against the current component state.
    /// Reuse a set created by [`Self::create_bus_audio_buffers`] for allocation-free in-process
    /// steady-state processing.
    pub fn process_bus_audio(&mut self, buffers: &mut BusAudioBuffers) -> Result<()> {
        if !self.is_processing {
            return Err(Error::NotProcessing);
        }
        self.internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .process_buses(buffers)?;
        if let Ok(mut levels) = self.audio_levels.lock() {
            levels.update_from_bus_buffers(&buffers.outputs);
            if let Some(ref callback) = self.audio_callback {
                callback(&levels);
            }
        }
        Ok(())
    }

    /// Get current output levels.
    ///
    /// Recovers automatically if the audio thread panicked while holding the lock
    /// (poisoned mutex) rather than propagating the panic to the caller — metering
    /// must never take down a UI thread polling it.
    pub fn get_output_levels(&self) -> AudioLevels {
        self.audio_levels
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Check if the plugin is currently processing
    pub fn is_processing(&self) -> bool {
        self.is_processing
    }

    /// Set a callback invoked whenever [`Self::set_parameter`] succeeds, with the parameter id
    /// and its new normalized value.
    ///
    /// # This callback runs on the caller's thread — including the audio thread
    ///
    /// It fires inline from `set_parameter`, so it runs on whichever thread made that call.
    /// Playback-ring automation uses a processor-only queue and does not invoke this callback
    /// (or `IEditController`) on the audio thread.
    pub fn on_parameter_change<F>(&mut self, callback: F)
    where
        F: Fn(u32, f64) + Send + 'static,
    {
        self.parameter_change_callback = Some(Box::new(callback));
    }

    /// Set a callback invoked after each [`Self::process_audio`] cycle with the freshly
    /// computed output levels.
    ///
    /// # This callback runs on the AUDIO thread
    ///
    /// It fires from inside `process_audio`, which both playback paths call on the audio
    /// callback thread, while the level mutex is held. Keep the body real-time safe — no
    /// allocation, no locks, no I/O, no blocking on a UI thread. For metering in a UI, poll
    /// [`Self::get_output_levels`] from the UI thread instead.
    pub fn on_audio_process<F>(&mut self, callback: F)
    where
        F: Fn(&AudioLevels) + Send + 'static,
    {
        self.audio_callback = Some(Box::new(callback));
    }

    /// Check if the plugin has an editor GUI
    pub fn has_editor(&self) -> bool {
        self.internal
            .as_ref()
            .map(|i| i.has_editor())
            .unwrap_or(false)
    }

    /// Query, open/focus or close a native editor owned by the isolated helper.
    ///
    /// This safe API carries only a logical owner's integer identity across IPC. The helper
    /// validates it and constructs a real local native parent before calling the plugin.
    /// Calling this on an in-process plugin returns an error. All work is control-plane work;
    /// do not invoke it from a realtime audio callback.
    pub fn isolated_editor(
        &mut self,
        command: IsolatedEditorCommand,
    ) -> Result<IsolatedEditorState> {
        if let IsolatedEditorCommand::Open { owner: Some(owner) } = command {
            if owner.window == 0 || owner.process_id == 0 {
                return Err(Error::Other(
                    "isolated editor owner must have a nonzero window and process id".to_string(),
                ));
            }
        }
        self.internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .isolated_editor(command)
    }

    /// Open the plugin editor window
    pub fn open_editor(&mut self, parent: WindowHandle) -> Result<()> {
        self.internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .open_editor(parent.0)
    }

    /// Drive the Linux `IRunLoop` services (timers and file-descriptor
    /// events) that the plugin's editor registered with the host frame.
    /// VSTGUI-based editors paint and respond ONLY when this runs - call it
    /// on the UI thread every frame (e.g. 30-60 Hz) while an editor is open.
    /// A no-op when nothing is registered, on non-Linux, or under process
    /// isolation.
    pub fn service_run_loop(&mut self) {
        if let Some(internal) = self.internal.as_mut() {
            internal.service_run_loop();
        }
    }

    /// Close the plugin editor window
    pub fn close_editor(&mut self) -> Result<()> {
        self.internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .close_editor()
    }

    /// Get the preferred editor size
    pub fn get_editor_size(&self) -> Result<(i32, i32)> {
        self.internal
            .as_ref()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .get_editor_size()
    }

    /// Whether the plugin editor accepts host-driven resize requests.
    ///
    /// With an editor open this reads the live view. With no editor open it has to *create* a
    /// throwaway view to ask, which costs on the order of milliseconds (~4.6 ms for Dexed) —
    /// cache the answer rather than calling it per UI frame.
    pub fn editor_can_resize(&self) -> bool {
        self.internal
            .as_ref()
            .is_some_and(|internal| internal.editor_can_resize())
    }

    /// Resize the open plugin editor, honoring the plugin's size constraints.
    ///
    /// Returns the size the plugin accepted, which may differ from the requested dimensions.
    pub fn resize_editor(&mut self, width: i32, height: i32) -> Result<(i32, i32)> {
        self.internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .resize_editor(width, height)
    }

    /// Communicate the editor's logical-to-physical content scale.
    ///
    /// Returns `false` when the editor does not implement VST3 content-scale support.
    pub fn set_editor_scale_factor(&mut self, factor: f32) -> Result<bool> {
        self.internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .set_editor_scale_factor(factor)
    }

    /// Collect several parameter changes with a [`ParameterUpdate`] and apply them in one call.
    ///
    /// # This batch is not atomic
    ///
    /// The queued changes are applied in the order they were `set`, and the first failure
    /// stops the batch and is returned — the changes queued *before* it have already been
    /// applied to the plugin and are **not** rolled back, and the ones after it were never
    /// attempted. The error does not say how far the batch got. If that matters, call
    /// [`Self::set_parameter`] per parameter and handle each result, or re-read the values
    /// with [`Self::get_parameters`] after an error.
    pub fn update_parameters<F>(&mut self, f: F) -> Result<()>
    where
        F: FnOnce(&mut ParameterUpdate) -> Result<()>,
    {
        let mut update = ParameterUpdate::new(self);
        f(&mut update)?;
        update.apply()
    }

    /// Send MIDI panic (all notes off, all sounds off, reset controllers)
    pub fn midi_panic(&mut self) -> Result<()> {
        self.internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .midi_panic()
    }

    /// Drain the parameter values that changed behind the host's back, as
    /// `(parameter_id, normalized_value)` pairs.
    ///
    /// Two sources feed this: edits the plugin's **editor** reported through
    /// `IComponentHandler::performEdit`, and points the **processor** wrote into its
    /// `outputParameterChanges` queue during `process()` (a compressor's gain-reduction readout,
    /// an internal LFO driving a visible control). Call it regularly — every UI frame — to keep
    /// the host's own display in step with the plugin.
    pub fn get_parameter_changes(&self) -> Vec<(u32, f64)> {
        self.internal
            .as_ref()
            .map(|i| i.get_parameter_changes())
            .unwrap_or_default()
    }

    /// Drain the ordered log of parameter-edit gestures the plugin's editor has reported since
    /// the last call.
    ///
    /// This is the richer superset of [`Self::get_parameter_changes`]: rather than just the
    /// value changes, it preserves the begin/change/end ordering of each gesture, so the host
    /// can tell a deliberate, completed edit (`BeginGesture` … `ValueChange`* … `EndGesture`)
    /// from a stream of intermediate drag values. Poll it regularly (e.g. each UI frame) while
    /// the editor is open; an empty vector means nothing was reported. Works across process
    /// isolation — gestures are marshalled back from the helper.
    ///
    /// See [`ParameterEdit`] / [`ParameterEditKind`].
    pub fn take_parameter_edits(&mut self) -> Vec<ParameterEdit> {
        self.internal
            .as_mut()
            .map(|i| i.take_parameter_edits())
            .unwrap_or_default()
    }

    /// Drain editor gestures while surfacing transport/protocol failure. Unlike the legacy
    /// infallible drain, an empty successful batch is never substituted for a failed exchange.
    /// Polling is control-plane work and must not run on a realtime audio callback.
    pub fn try_take_parameter_edits(&mut self) -> Result<Vec<ParameterEdit>> {
        self.internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .try_take_parameter_edits()
    }

    /// Drain host notifications while surfacing transport/protocol failure. Hosts should retain
    /// conservative unsaved-state evidence if this fails after native editing was possible.
    pub fn try_take_host_notifications(&mut self) -> Result<Vec<HostNotification>> {
        self.internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .try_take_host_notifications()
    }

    /// Read durable evidence of native edits for this loaded plugin instance.
    ///
    /// Increases for `performEdit`, `setDirty(true)`, and parameter-value/reload restart
    /// requests, even when the bounded feedback queues are full. Reading or draining feedback
    /// never clears it. Compare only within the same loaded instance; a reload creates a new
    /// baseline. Transport failure and counter exhaustion are errors, never a clean revision.
    /// No transparent helper recovery occurs. This is control-plane work, not realtime work.
    pub fn native_dirty_revision(&mut self) -> Result<u64> {
        self.internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .native_dirty_revision()
    }

    /// Drain ordered requests the plugin reported through `IComponentHandler2`.
    pub fn take_host_notifications(&mut self) -> Vec<HostNotification> {
        self.internal
            .as_mut()
            .map(|i| i.take_host_notifications())
            .unwrap_or_default()
    }

    /// Dispatch and drain blocks sent through VST3's `IDataExchangeHandler`.
    ///
    /// Call this regularly on the plug-in's control/UI thread. Queues whose controller requested
    /// background dispatch are delivered automatically, but their owned snapshots are drained
    /// here too. Storage is bounded; when the host-side snapshot sink is full, newer snapshots
    /// are dropped while controller delivery continues.
    pub fn take_data_exchange_blocks(&mut self) -> Vec<DataExchangeBlock> {
        self.internal
            .as_mut()
            .map(|i| i.take_data_exchange_blocks())
            .unwrap_or_default()
    }

    /// Execute a plugin context-menu entry previously received through
    /// [`Self::take_host_notifications`].
    ///
    /// A popup can be completed once. Calling this invokes the plugin-provided
    /// `IContextMenuTarget` on the plugin control/UI thread and releases all targets retained for
    /// that popup.
    pub fn execute_context_menu_item(&mut self, menu_id: u64, item_id: u32) -> Result<()> {
        self.internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .execute_context_menu_item(menu_id, item_id)
    }

    /// Dismiss a pending plugin context menu and release its retained targets.
    pub fn dismiss_context_menu(&mut self, menu_id: u64) -> Result<()> {
        self.internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .dismiss_context_menu(menu_id)
    }

    /// Take the flags the plugin raised via `IComponentHandler::restartComponent` since the
    /// last call — its way of saying "something about me changed, re-read it".
    ///
    /// Poll this next to [`Self::take_parameter_edits`] (e.g. each UI frame). See
    /// [`RestartFlags`] for what each one asks of the host. Returns an empty set for a plugin
    /// that hasn't raised anything. Works across process isolation.
    pub fn take_restart_flags(&mut self) -> RestartFlags {
        self.internal
            .as_mut()
            .map(|i| i.take_restart_flags())
            .unwrap_or_default()
    }

    /// Service pending restart requests on the caller's control thread.
    ///
    /// Latency and I/O requests are applied through the required stop/deactivate/reactivate
    /// lifecycle. The returned flags still describe every request; in particular,
    /// [`RestartFlags::reload_component`] means the caller must replace this plugin instance.
    pub fn service_host_requests(&mut self) -> Result<RestartFlags> {
        let internal = self
            .internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?;
        let result = internal.service_host_requests();
        // A rejected state/I/O rebuild may have stopped processing. Preserve the confirmed
        // state even on error so a later start cannot falsely succeed from a stale flag.
        if let Some(processing) = internal.processing_state() {
            self.is_processing = processing;
        }
        result
    }

    /// Take the MIDI events the plugin has emitted (e.g. from an arpeggiator or MPE
    /// controller) since the last call, draining the internal buffer.
    ///
    /// Output MIDI is captured while the plugin processes audio, so poll this regularly
    /// (e.g. each UI frame) while the plugin is playing; an empty vector means the plugin
    /// emitted nothing. This works for process-isolated plugins too — emitted events are
    /// marshalled back alongside each processed block.
    ///
    /// The buffer is capped at 4096 events: if you never poll while a chatty plugin keeps
    /// emitting, the oldest events are dropped to bound memory. Use
    /// [`Self::take_output_events_with_loss`] to observe loss before forwarding notes.
    pub fn take_output_midi(&self) -> Vec<MidiEvent> {
        self.internal
            .as_ref()
            .map(|i| {
                i.take_output_events()
                    .into_iter()
                    .filter_map(|event| event.to_midi())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Take every event the plugin has emitted, preserving SysEx and all VST3 event variants.
    pub fn take_output_events(&self) -> Vec<crate::midi::OutputEvent> {
        self.internal
            .as_ref()
            .map(|i| i.take_output_events())
            .unwrap_or_default()
    }

    /// Drain emitted events and atomically acknowledge any observed output-event loss.
    ///
    /// The boolean latches queue overflow, rejected event capture, and isolated buffering
    /// loss since the previous loss-aware drain. A routing host should release notes at its
    /// destination when true, because a note-off may have been lost. Legacy drains do not
    /// clear this loss indication. Use a single draining consumer per plugin.
    pub fn take_output_events_with_loss(&self) -> (Vec<crate::midi::OutputEvent>, bool) {
        self.internal
            .as_ref()
            .map(|i| i.take_output_events_with_loss())
            .unwrap_or_default()
    }

    /// Get a `Send` handle for draining emitted MIDI from another thread without locking the
    /// audio thread (see [`OutputMidiConsumer`]). Returns `None` for an unloaded plugin or the
    /// process-isolation path. Useful with [`RealtimePluginRunner`](crate::RealtimePluginRunner):
    /// take the handle, move the plugin into the runner, and poll it from your UI thread while
    /// the audio thread renders.
    pub fn output_midi_handle(&self) -> Option<OutputMidiConsumer> {
        self.internal.as_ref().and_then(|i| i.output_midi_handle())
    }

    /// Get a lock-free handle for draining all emitted VST3 events.
    pub fn output_event_handle(&self) -> Option<OutputEventConsumer> {
        self.internal.as_ref().and_then(|i| i.output_event_handle())
    }

    /// Save the plugin's current state (parameters, internal settings, loaded preset) to
    /// an opaque byte blob.
    ///
    /// The blob is a versioned envelope holding the two streams VST3 defines — the component's
    /// state and, for a plugin whose controller is a separate object, the controller's — not a
    /// bare copy of either. Treat it as opaque and pair it with the plugin's identity
    /// ([`PluginInfo::uid`]); it only means something to the same plugin, and only
    /// [`Self::load_state`] can unpack it. (Blobs written by older releases, which were the raw
    /// component stream, still load.) Persist it to restore a patch later, or to snapshot a
    /// session. Call this on the main thread (see the
    /// [threading model](https://docs.rs/vst3-host)). For a blob other VST3 hosts can read,
    /// use [`Self::save_vstpreset`].
    ///
    /// Works both in-process and across process isolation (the state blob is marshalled over
    /// the IPC boundary). Returns an error for plugins that don't implement state saving.
    pub fn save_state(&self) -> Result<Vec<u8>> {
        self.internal
            .as_ref()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .save_state()
    }

    /// Check locally known compatibility restrictions without changing plugin/editor state.
    /// Remote helpers remain authoritative and repeat the check when they receive LoadState.
    pub fn preflight_state_restore(&self) -> Result<()> {
        self.internal
            .as_ref()
            .ok_or_else(|| Error::Other("Plugin not initialized".into()))?
            .preflight_state_restore()
    }

    /// Restore plugin state from a blob produced by [`Self::save_state`] on the *same*
    /// plugin. Applies to both the processor and the controller, so parameter values and
    /// the editor reflect the restored state.
    ///
    /// Passing bytes from a different plugin has undefined results (the plugin decides what
    /// to do with bytes it doesn't recognize). Call this on the main thread.
    ///
    /// The plugin is told this is a project/session restore ([`StateContext::Project`]). Use
    /// [`Self::load_state_with_context`] when the bytes came from a preset file instead.
    pub fn load_state(&mut self, data: &[u8]) -> Result<()> {
        let internal = self
            .internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?;
        let result = internal.load_state(data);
        // A rejected state/I/O rebuild may have stopped processing. Preserve the confirmed
        // state even on error so a later start cannot falsely succeed from a stale flag.
        if let Some(processing) = internal.processing_state() {
            self.is_processing = processing;
        }
        result
    }

    /// Restore plugin state as [`Self::load_state`] does, and tell the plugin where the bytes
    /// came from.
    ///
    /// VST3 plugins can read the context off the stream they are given (the SDK's
    /// `Vst::Helpers::isProjectState()` does exactly that) and restore differently for a
    /// session than for a preset. [`Self::load_vstpreset`] and [`Self::load_preset`] already
    /// pass [`StateContext::Preset`] with the file they read; reach for this directly when
    /// your host holds preset bytes it loaded some other way.
    ///
    /// Works both in-process and across process isolation — an isolated plugin's `setState`
    /// sees the same attributes.
    pub fn load_state_with_context(&mut self, data: &[u8], context: &StateContext) -> Result<()> {
        let internal = self
            .internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?;
        let result = internal.load_state_with_context(data, context);
        // A rejected state/I/O rebuild may have stopped processing. Preserve the confirmed
        // state even on error so a later start cannot falsely succeed from a stale flag.
        if let Some(processing) = internal.processing_state() {
            self.is_processing = processing;
        }
        result
    }

    /// Save this plugin's state to a file as a [`PluginPreset`] (JSON: the plugin's `uid`
    /// and name plus the opaque state blob). The embedded `uid` lets [`Self::load_preset`]
    /// reject a preset saved from a different plugin.
    pub fn save_preset<P: AsRef<std::path::Path>>(&self, path: P) -> Result<()> {
        let info = self.info();
        let preset = PluginPreset {
            uid: info.uid.clone(),
            plugin_name: info.name.clone(),
            state: self.save_state()?,
        };
        let json = serde_json::to_vec_pretty(&preset)
            .map_err(|e| Error::Other(format!("serialize preset: {e}")))?;
        std::fs::write(path, json).map_err(|e| Error::Other(format!("write preset: {e}")))?;
        Ok(())
    }

    /// Load a [`PluginPreset`] file written by [`Self::save_preset`] and apply its state.
    /// Returns an error if the preset's `uid` doesn't match this plugin (loading another
    /// plugin's state is undefined).
    ///
    /// The plugin sees this as a preset load ([`StateContext::Preset`]) carrying `path`, not
    /// as a session restore.
    pub fn load_preset<P: AsRef<std::path::Path>>(&mut self, path: P) -> Result<()> {
        let path = path.as_ref();
        let bytes = std::fs::read(path).map_err(|e| Error::Other(format!("read preset: {e}")))?;
        let preset: PluginPreset = serde_json::from_slice(&bytes)
            .map_err(|e| Error::Other(format!("parse preset: {e}")))?;
        if !self.accepts_state_class_id(&preset.uid)? {
            return Err(Error::Other(format!(
                "preset is for a different plugin ({}, expected {})",
                preset.plugin_name,
                self.info().name
            )));
        }
        self.load_state_with_context(&preset.state, &StateContext::preset_from_path(path))
    }

    /// Save this plugin's state to a standard Steinberg `.vstpreset` file.
    ///
    /// Unlike [`Self::save_preset`] (a JSON wrapper specific to this library), the
    /// `.vstpreset` container is the interchange format shared by VST3 hosts and plugins, so
    /// the file can be read by other hosts (and by the plugin's own preset browser). It wraps
    /// the component and optional controller streams from [`Self::save_state`] in `"Comp"` and
    /// `"Cont"` chunks, tagged with this plugin's class id ([`PluginInfo::uid`]) so a loader can
    /// reject presets from a different plugin. Call this on the main thread.
    pub fn save_vstpreset<P: AsRef<std::path::Path>>(&self, path: P) -> Result<()> {
        let state = decode_state_snapshot(&self.save_state()?)?;
        let bytes = vstpreset::build(
            &self.info().uid,
            &state.component,
            state.controller.as_deref(),
        )?;
        std::fs::write(path, bytes).map_err(|e| Error::Other(format!("write vstpreset: {e}")))?;
        Ok(())
    }

    /// Load a Steinberg `.vstpreset` file and apply its component and controller state.
    ///
    /// Parses the `.vstpreset` container written by [`Self::save_vstpreset`] (or another VST3
    /// host), extracts the `"Comp"` and optional `"Cont"` chunks and passes them to
    /// [`Self::load_state`]. Returns an error if the file's magic is invalid, or if its class
    /// id doesn't match this plugin (loading another plugin's state is undefined). Call this
    /// on the main thread.
    ///
    /// The plugin is told this is a preset load ([`StateContext::Preset`]) and is given the
    /// file's full path, the way a DAW's preset browser would — not the project-restore
    /// context [`Self::load_state`] uses.
    pub fn load_vstpreset<P: AsRef<std::path::Path>>(&mut self, path: P) -> Result<()> {
        let path = path.as_ref();
        let bytes =
            std::fs::read(path).map_err(|e| Error::Other(format!("read vstpreset: {e}")))?;
        let parsed = vstpreset::parse(&bytes)?;
        if !self.accepts_state_class_id(&parsed.class_id)? {
            return Err(Error::Other(format!(
                "vstpreset is for a different plugin (class id {}, expected {})",
                parsed.class_id,
                self.info().uid
            )));
        }
        let state = encode_state_snapshot(&StateSnapshot {
            component: parsed.component_state,
            controller: parsed.controller_state,
        })?;
        self.load_state_with_context(&state, &StateContext::preset_from_path(path))
    }

    /// Accept the current class id or a retired id which this bundle's moduleinfo declares as
    /// replaced by the current class. This is deliberately evaluated from validated bundle
    /// metadata rather than trusting a preset to name its own replacement.
    fn accepts_state_class_id(&self, candidate: &str) -> Result<bool> {
        if crate::internal::utils::class_uid_matches(&self.info().uid, candidate) {
            return Ok(true);
        }
        Ok(self.compatibility.iter().any(|mapping| {
            crate::internal::utils::class_uid_matches(&mapping.new_class_id, &self.info().uid)
                && mapping
                    .old_class_ids
                    .iter()
                    .any(|old| crate::internal::utils::class_uid_matches(old, candidate))
        }))
    }

    /// The OS process id of the isolated helper hosting this plugin, or `None` if it runs
    /// in-process. Useful for monitoring an isolated plugin's resource use.
    pub fn isolation_pid(&self) -> Option<u32> {
        self.internal.as_ref().and_then(|i| i.helper_pid())
    }

    /// How many times this plugin has been recovered (helper respawned + reloaded), via either
    /// [`Self::recover`] or automatic recovery ([`Vst3HostBuilder::auto_recover_plugins`]).
    ///
    /// A recovery reloads the plugin from defaults — parameter values and loaded state are NOT
    /// replayed. With auto-recover on, a crash is otherwise invisible (the call returns `Ok`),
    /// so poll this count to detect that a reset happened and re-apply a saved
    /// [`save_state`](Self::save_state) snapshot.
    ///
    /// [`Vst3HostBuilder::auto_recover_plugins`]: crate::Vst3HostBuilder::auto_recover_plugins
    pub fn recovery_count(&self) -> u64 {
        self.internal
            .as_ref()
            .map(|i| i.recovery_count())
            .unwrap_or(0)
    }

    /// Total number of output audio channels across the plugin's output buses.
    ///
    /// Reflects the plugin's actual bus layout (mono / stereo / surround / multi-bus), not a
    /// stereo assumption — useful for sizing meters or output buffers. Returns 2 if unknown.
    pub fn output_channel_count(&self) -> usize {
        self.internal
            .as_ref()
            .map(|i| i.output_channel_count())
            .unwrap_or(2)
    }

    /// Poll for an editor resize the plugin requested via VST3's `IPlugFrame` since the last
    /// call, as `(width, height)` in pixels, or `None`.
    ///
    /// Plugins with resizable editors call back to ask the host to resize the window hosting
    /// their view. Poll this on your UI thread (e.g. each frame) while the editor is open and
    /// resize your editor container to match. Only the in-process editor path reports this.
    pub fn take_editor_resize_request(&self) -> Option<(i32, i32)> {
        self.internal
            .as_ref()
            .and_then(|i| i.take_editor_resize_request())
    }

    /// Recover a process-isolated plugin whose helper has crashed.
    ///
    /// When an isolated plugin's helper process dies, calls return [`Error::PluginCrashed`]
    /// and the host itself stays alive. This respawns the helper and reloads the plugin
    /// from the same path and audio settings, restarting processing if it was running.
    ///
    /// **The reloaded plugin starts from its default state** — parameter values and any
    /// loaded preset are lost. Snapshot with [`Self::save_state`] beforehand and
    /// [`Self::load_state`] after recovering to preserve them. Returns an error for
    /// in-process plugins (an in-process crash takes down the whole host) and if the
    /// reload itself fails.
    pub fn recover(&mut self) -> Result<()> {
        self.internal
            .as_mut()
            .ok_or_else(|| Error::Other("Plugin not initialized".to_string()))?
            .recover()
    }
}

/// Highest value a 7-bit MIDI data byte can carry.
const MIDI_DATA_MAX: u8 = 127;

/// Highest value a 14-bit MIDI pitch-bend can carry.
const MIDI_PITCH_BEND_MAX: u16 = 16383;

pub(crate) fn validate_note(note: u8) -> Result<()> {
    if note > MIDI_DATA_MAX {
        return Err(Error::MidiError(format!("Invalid note number: {}", note)));
    }
    Ok(())
}

pub(crate) fn validate_velocity(velocity: u8) -> Result<()> {
    if velocity > MIDI_DATA_MAX {
        return Err(Error::MidiError(format!("Invalid velocity: {}", velocity)));
    }
    Ok(())
}

fn validate_controller(controller: u8) -> Result<()> {
    if controller > MIDI_DATA_MAX {
        return Err(Error::MidiError(format!(
            "Invalid controller number: {}",
            controller
        )));
    }
    Ok(())
}

fn validate_cc_value(value: u8) -> Result<()> {
    if value > MIDI_DATA_MAX {
        return Err(Error::MidiError(format!("Invalid CC value: {}", value)));
    }
    Ok(())
}

fn validate_pressure(pressure: u8) -> Result<()> {
    if pressure > MIDI_DATA_MAX {
        return Err(Error::MidiError(format!(
            "Invalid pressure value: {}",
            pressure
        )));
    }
    Ok(())
}

/// Range-check every data field of a [`MidiEvent`] before it reaches a plugin.
///
/// `MidiEvent`'s fields are plain `u8`/`u16`, so nothing stops a caller from building an
/// out-of-spec event by hand. The conversion below this layer masks nothing: a note of `255`
/// becomes a `255` pitch in the VST3 event, a velocity of `255` becomes `2.008` where the
/// spec's maximum is `1.0`, and a legacy CC value of `200` becomes a *negative* MIDI byte
/// once cast to `c_char`. Reject them here, with the same messages the typed senders use.
///
/// The match below has no wildcard arm on purpose: `MidiEvent` is `#[non_exhaustive]` only to
/// downstream crates, so a variant added here fails to compile until it states its own ranges.
fn validate_midi_event(event: &MidiEvent) -> Result<()> {
    match *event {
        MidiEvent::NoteOn { note, velocity, .. } | MidiEvent::NoteOff { note, velocity, .. } => {
            validate_note(note)?;
            validate_velocity(velocity)
        }
        MidiEvent::ControlChange {
            controller, value, ..
        } => {
            validate_controller(controller)?;
            validate_cc_value(value)
        }
        MidiEvent::ProgramChange { program, .. } => {
            if program > MIDI_DATA_MAX {
                return Err(Error::MidiError(format!(
                    "Invalid program number: {}",
                    program
                )));
            }
            Ok(())
        }
        MidiEvent::PitchBend { value, .. } => {
            if value > MIDI_PITCH_BEND_MAX {
                return Err(Error::MidiError(format!(
                    "Invalid pitch bend value: {} (0-{})",
                    value, MIDI_PITCH_BEND_MAX
                )));
            }
            Ok(())
        }
        MidiEvent::ChannelAftertouch { pressure, .. } => validate_pressure(pressure),
        MidiEvent::PolyAftertouch { note, pressure, .. } => {
            validate_note(note)?;
            validate_pressure(pressure)
        }
    }
}

pub(crate) fn validate_plugin_event(event: &PluginEvent) -> Result<()> {
    if event.bus_index < 0 {
        return Err(Error::MidiError(format!(
            "Invalid event bus index: {}",
            event.bus_index
        )));
    }
    if event.sample_offset < 0 {
        return Err(Error::MidiError(format!(
            "Invalid event sample offset: {}",
            event.sample_offset
        )));
    }
    if !event.ppq_position.is_finite() {
        return Err(Error::MidiError(
            "Event PPQ position must be finite".to_string(),
        ));
    }

    let validate_channel = |channel: i16| {
        if (0..16).contains(&channel) {
            Ok(())
        } else {
            Err(Error::MidiError(format!(
                "Invalid VST3 event channel: {channel}"
            )))
        }
    };
    let validate_pitch = |pitch: i16| {
        if (0..=127).contains(&pitch) {
            Ok(())
        } else {
            Err(Error::MidiError(format!(
                "Invalid VST3 event pitch: {pitch}"
            )))
        }
    };
    let validate_normalized = |name: &str, value: f64| {
        if value.is_finite() && (0.0..=1.0).contains(&value) {
            Ok(())
        } else {
            Err(Error::MidiError(format!(
                "{name} must be finite and normalized to [0.0, 1.0]"
            )))
        }
    };

    match &event.data {
        PluginEventData::NoteOn {
            channel,
            pitch,
            tuning,
            velocity,
            length,
            ..
        } => {
            validate_channel(*channel)?;
            validate_pitch(*pitch)?;
            validate_normalized("note velocity", f64::from(*velocity))?;
            if !tuning.is_finite() || *length < 0 {
                return Err(Error::MidiError(
                    "note tuning must be finite and length non-negative".to_string(),
                ));
            }
            Ok(())
        }
        PluginEventData::NoteOff {
            channel,
            pitch,
            velocity,
            tuning,
            ..
        } => {
            validate_channel(*channel)?;
            validate_pitch(*pitch)?;
            validate_normalized("note-off velocity", f64::from(*velocity))?;
            if !tuning.is_finite() {
                return Err(Error::MidiError(
                    "note-off tuning must be finite".to_string(),
                ));
            }
            Ok(())
        }
        PluginEventData::Data { data_type, bytes } => {
            if *data_type != 0 {
                return Err(Error::MidiError(format!(
                    "Unsupported VST3 data event type: {data_type}"
                )));
            }
            if bytes.len() > MAX_EVENT_PAYLOAD_BYTES {
                return Err(Error::MidiError(format!(
                    "Event payload is {} bytes; maximum is {MAX_EVENT_PAYLOAD_BYTES}",
                    bytes.len()
                )));
            }
            Ok(())
        }
        PluginEventData::PolyPressure {
            channel,
            pitch,
            pressure,
            ..
        } => {
            validate_channel(*channel)?;
            validate_pitch(*pitch)?;
            validate_normalized("poly pressure", f64::from(*pressure))
        }
        PluginEventData::NoteExpressionValue { value, .. } => {
            validate_normalized("note-expression value", *value)
        }
        PluginEventData::NoteExpressionText { text, .. }
        | PluginEventData::Chord { text, .. }
        | PluginEventData::Scale { text, .. } => {
            if text.len() > MAX_EVENT_TEXT_UNITS {
                return Err(Error::MidiError(format!(
                    "Event text is {} UTF-16 units; maximum is {MAX_EVENT_TEXT_UNITS}",
                    text.len()
                )));
            }
            Ok(())
        }
        PluginEventData::NoteExpressionIntValue { .. } => Ok(()),
        PluginEventData::LegacyMidiCcOut { .. } => Err(Error::MidiError(
            "Legacy MIDI CC events are plugin output only".to_string(),
        )),
    }
}

/// Logical owner of an isolated Windows editor, transported as data rather than a pointer.
///
/// The helper validates both the live HWND and its process id. It owns its editor container
/// independently: Windows must not destroy that container before the plugin can detach when
/// the owner disappears. This is a standalone associated window, not cross-process embedding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IsolatedEditorOwner {
    /// The owner's HWND value. Zero is invalid; the helper also rejects width truncation.
    pub window: u64,
    /// Process which owns that HWND. Zero is invalid.
    pub process_id: u32,
}

/// Control-plane request to a helper-owned native editor. No fabricated `WindowHandle` is needed.
///
/// New helpers implement this additive protocol; an older helper returns an ordinary error.
/// These requests may perform synchronous IPC and must never run on a realtime audio callback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub enum IsolatedEditorCommand {
    /// Query actual helper-side capability and state, including a user-closed editor.
    Query,
    /// Open once, or focus the existing editor. A different live owner is rejected while open.
    Open {
        /// Optional logical owning application window; `None` is a standalone editor.
        owner: Option<IsolatedEditorOwner>,
    },
    /// Focus an already open editor. Fails if it has been closed.
    Focus,
    /// Detach the plugin view before destroying its container. Closing twice is harmless.
    Close,
}

/// Authoritative snapshot from the isolated helper, not a cached "open was requested" flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IsolatedEditorState {
    /// Whether this helper platform implements the native lifecycle protocol.
    pub supported: bool,
    /// Whether the loaded plugin advertises a native editor; false if no plugin is loaded.
    pub has_editor: bool,
    /// Whether a real plugin view is currently attached to a live helper-owned container.
    pub open: bool,
    /// Current accepted client width, or zero when closed.
    pub width: i32,
    /// Current accepted client height, or zero when closed.
    pub height: i32,
    /// Increases on each successful new attachment in this helper process; zero before first open.
    /// Combine with the host's instance/helper identity: a restarted process begins at zero.
    pub generation: u64,
}

/// Platform-specific window handle
pub struct WindowHandle(pub(crate) *mut std::ffi::c_void);

impl WindowHandle {
    /// Create from a raw window handle
    ///
    /// # Safety
    /// The pointer must be a valid window handle for the platform. On Linux this API currently
    /// selects VST3's `X11EmbedWindowID` contract; a `wl_surface` is not accepted because VST 3.8
    /// Wayland embedding additionally requires host-provided `IWaylandHost`/`IWaylandFrame`
    /// services.
    pub unsafe fn from_raw(handle: *mut std::ffi::c_void) -> Self {
        Self(handle)
    }
}

// Safe Send implementation - the window handle is platform-specific
unsafe impl Send for WindowHandle {}

#[cfg(target_os = "macos")]
impl WindowHandle {
    /// Create from an `NSView` pointer on macOS.
    ///
    /// # Safety
    ///
    /// `view` must be a live `NSView` that stays alive for as long as the editor is attached
    /// to it. [`Plugin::open_editor`] hands the pointer straight to the plugin's
    /// `IPlugView::attached`, which dereferences it; nothing on that path can tell a valid
    /// view from a dangling or foreign pointer.
    pub unsafe fn from_nsview(view: *mut std::ffi::c_void) -> Self {
        Self(view)
    }
}

#[cfg(target_os = "windows")]
impl WindowHandle {
    /// Create from an `HWND` on Windows.
    ///
    /// # Safety
    ///
    /// `hwnd` must be a live window handle that stays valid for as long as the editor is
    /// attached to it. [`Plugin::open_editor`] hands it straight to the plugin's
    /// `IPlugView::attached`, which uses it as a window; nothing on that path validates it.
    pub unsafe fn from_hwnd(hwnd: *mut std::ffi::c_void) -> Self {
        Self(hwnd)
    }
}

#[cfg(target_os = "linux")]
impl WindowHandle {
    /// Create from an X11 window id on Linux (for VST3 `X11EmbedWindowID`).
    ///
    /// The VST3 X11 platform type expects the window id itself as the handle value,
    /// not a pointer to it.
    pub fn from_x11(window_id: u32) -> Self {
        Self(window_id as usize as *mut std::ffi::c_void)
    }
}

/// Build and parse the standard Steinberg `.vstpreset` container format.
///
/// Layout (all multi-byte integers little-endian, matching the SDK's `PresetFile`):
///
/// - Header (48 bytes): magic `b"VST3"` (4) + version `i32` = 1 (4) + 32-char ASCII class
///   id (the plugin's FUID hex) (32) + `i64` byte offset from the start of the file to the
///   chunk list (8).
/// - Body: the chunk payloads, written back to back after the header: required `"Comp"`
///   component state and optional `"Cont"` controller state.
/// - Chunk list (at the header's list offset): magic `b"List"` (4) + entry count `i32` (4),
///   then per entry: 4-byte chunk id + `i64` absolute offset + `i64` size.
mod vstpreset {
    use crate::error::{Error, Result};

    const MAGIC: &[u8; 4] = b"VST3";
    const LIST_MAGIC: &[u8; 4] = b"List";
    const COMPONENT_CHUNK: &[u8; 4] = b"Comp";
    const CONTROLLER_CHUNK: &[u8; 4] = b"Cont";
    const VERSION: i32 = 1;
    const CLASS_ID_LEN: usize = 32;
    const HEADER_SIZE: usize = 4 + 4 + CLASS_ID_LEN + 8;
    const LIST_HEADER_SIZE: usize = 8;
    const ENTRY_SIZE: usize = 20;

    /// A parsed `.vstpreset` container.
    pub(super) struct Parsed {
        /// The 32-char ASCII class id from the header.
        pub class_id: String,
        /// The bytes of the `"Comp"` (component state) chunk.
        pub component_state: Vec<u8>,
        /// The bytes of the optional `"Cont"` (controller state) chunk.
        pub controller_state: Option<Vec<u8>>,
    }

    /// Build a `.vstpreset` file containing component and optional controller state.
    pub(super) fn build(
        class_id: &str,
        component_state: &[u8],
        controller_state: Option<&[u8]>,
    ) -> Result<Vec<u8>> {
        let class_bytes = class_id.as_bytes();
        if class_bytes.len() != CLASS_ID_LEN
            || !class_bytes.iter().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(Error::Other(format!(
                "vstpreset class id must be {CLASS_ID_LEN} ASCII hex chars, got {:?}",
                class_id
            )));
        }

        let comp_offset = HEADER_SIZE as i64;
        let comp_size = component_state.len() as i64;
        let controller_offset = HEADER_SIZE
            .checked_add(component_state.len())
            .ok_or_else(|| Error::Other("vstpreset size overflow".to_string()))?;
        let list_offset = controller_offset
            .checked_add(controller_state.map_or(0, <[u8]>::len))
            .ok_or_else(|| Error::Other("vstpreset size overflow".to_string()))?;
        let entry_count = if controller_state.is_some() { 2 } else { 1 };

        let mut out = Vec::with_capacity(list_offset + LIST_HEADER_SIZE + entry_count * ENTRY_SIZE);
        // Header.
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend(class_bytes.iter().map(u8::to_ascii_uppercase));
        out.extend_from_slice(&(list_offset as i64).to_le_bytes());
        // Body.
        out.extend_from_slice(component_state);
        if let Some(controller) = controller_state {
            out.extend_from_slice(controller);
        }
        // Chunk list.
        out.extend_from_slice(LIST_MAGIC);
        out.extend_from_slice(&(entry_count as i32).to_le_bytes());
        out.extend_from_slice(COMPONENT_CHUNK);
        out.extend_from_slice(&comp_offset.to_le_bytes());
        out.extend_from_slice(&comp_size.to_le_bytes());
        if let Some(controller) = controller_state {
            out.extend_from_slice(CONTROLLER_CHUNK);
            out.extend_from_slice(&(controller_offset as i64).to_le_bytes());
            out.extend_from_slice(&(controller.len() as i64).to_le_bytes());
        }

        Ok(out)
    }

    /// Parse a `.vstpreset` file, extracting component and optional controller state.
    pub(super) fn parse(bytes: &[u8]) -> Result<Parsed> {
        if bytes.len() < HEADER_SIZE {
            return Err(Error::Other("vstpreset too short for header".to_string()));
        }
        if &bytes[0..4] != MAGIC {
            return Err(Error::Other(format!(
                "bad vstpreset magic: expected {:?}, got {:?}",
                MAGIC,
                &bytes[0..4]
            )));
        }
        let version = read_i32(&bytes[4..8]);
        if version != VERSION {
            return Err(Error::Other(format!(
                "unsupported vstpreset version {version} (expected {VERSION})"
            )));
        }
        let class_id = String::from_utf8(bytes[8..8 + CLASS_ID_LEN].to_vec())
            .map_err(|e| Error::Other(format!("vstpreset class id not UTF-8: {e}")))?;
        if !class_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(Error::Other(
                "vstpreset class id is not 32 ASCII hex characters".to_string(),
            ));
        }
        let list_offset = read_i64(&bytes[8 + CLASS_ID_LEN..HEADER_SIZE]);
        if list_offset < HEADER_SIZE as i64 || list_offset as usize > bytes.len() {
            return Err(Error::Other(format!(
                "vstpreset chunk-list offset {list_offset} out of bounds (len {})",
                bytes.len()
            )));
        }
        let list = &bytes[list_offset as usize..];
        if list.len() < 8 || &list[0..4] != LIST_MAGIC {
            return Err(Error::Other(
                "vstpreset chunk list missing or malformed".to_string(),
            ));
        }
        let count = read_i32(&list[4..8]);
        if count < 0 {
            return Err(Error::Other("vstpreset negative entry count".to_string()));
        }
        let count = count as usize;
        let list_size = LIST_HEADER_SIZE
            .checked_add(
                count
                    .checked_mul(ENTRY_SIZE)
                    .ok_or_else(|| Error::Other("vstpreset entry count overflow".to_string()))?,
            )
            .ok_or_else(|| Error::Other("vstpreset list size overflow".to_string()))?;
        if list.len() < list_size {
            return Err(Error::Other(
                "vstpreset chunk-list entry truncated".to_string(),
            ));
        }

        let body_end = list_offset as usize;
        let mut cursor = LIST_HEADER_SIZE;
        let mut component_state = None;
        let mut controller_state = None;
        let mut ranges: Vec<(usize, usize)> = Vec::with_capacity(count);
        for _ in 0..count {
            let id = &list[cursor..cursor + 4];
            let offset = read_i64(&list[cursor + 4..cursor + 12]);
            let size = read_i64(&list[cursor + 12..cursor + 20]);
            cursor += ENTRY_SIZE;
            if offset < HEADER_SIZE as i64 || size < 0 {
                return Err(Error::Other(
                    "vstpreset chunk has invalid offset/size".to_string(),
                ));
            }
            let start = usize::try_from(offset)
                .map_err(|_| Error::Other("vstpreset chunk offset overflow".to_string()))?;
            let size = usize::try_from(size)
                .map_err(|_| Error::Other("vstpreset chunk size overflow".to_string()))?;
            let end = start
                .checked_add(size)
                .ok_or_else(|| Error::Other("vstpreset chunk size overflow".to_string()))?;
            if end > body_end {
                return Err(Error::Other(format!(
                    "vstpreset chunk [{start}..{end}] is outside the payload body \
                     [{HEADER_SIZE}..{body_end}]"
                )));
            }
            if ranges
                .iter()
                .any(|&(other_start, other_end)| start < other_end && other_start < end)
            {
                return Err(Error::Other("vstpreset chunk payloads overlap".to_string()));
            }
            ranges.push((start, end));

            match id {
                id if id == COMPONENT_CHUNK => {
                    if component_state.is_some() {
                        return Err(Error::Other(
                            "vstpreset has duplicate component chunks".to_string(),
                        ));
                    }
                    component_state = Some(bytes[start..end].to_vec());
                }
                id if id == CONTROLLER_CHUNK => {
                    if controller_state.is_some() {
                        return Err(Error::Other(
                            "vstpreset has duplicate controller chunks".to_string(),
                        ));
                    }
                    controller_state = Some(bytes[start..end].to_vec());
                }
                _ => {}
            }
        }
        let component_state = component_state.ok_or_else(|| {
            Error::Other("vstpreset has no component (\"Comp\") chunk".to_string())
        })?;
        Ok(Parsed {
            class_id,
            component_state,
            controller_state,
        })
    }

    fn read_i32(b: &[u8]) -> i32 {
        i32::from_le_bytes([b[0], b[1], b[2], b[3]])
    }

    fn read_i64(b: &[u8]) -> i64 {
        i64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
    }
}

#[cfg(test)]
mod public_surface_tests {
    use super::*;

    #[test]
    fn unloaded_native_feedback_checks_never_report_a_clean_result() {
        let mut plugin = unloaded_plugin();
        assert!(plugin.native_dirty_revision().is_err());
        assert!(plugin.try_take_parameter_edits().is_err());
        assert!(plugin.try_take_host_notifications().is_err());
    }

    /// A `Plugin` with no backing implementation. Enough to exercise the checks the public
    /// surface performs *before* it reaches into `internal`.
    fn unloaded_plugin() -> Plugin {
        Plugin {
            info: PluginInfo {
                path: std::path::PathBuf::from("/none.vst3"),
                name: "None".to_string(),
                vendor: String::new(),
                version: String::new(),
                category: String::new(),
                uid: String::new(),
                audio_inputs: 0,
                audio_outputs: 2,
                has_midi_input: true,
                has_midi_output: false,
                has_gui: false,
            },
            compatibility: Vec::new(),
            is_processing: false,
            sample_rate: 44_100.0,
            block_size: 512,
            audio_levels: Arc::new(Mutex::new(AudioLevels::new(2))),
            parameter_change_callback: None,
            audio_callback: None,
            internal: None,
        }
    }

    /// The "not processing" rejection happens once per block on the audio thread, so it must
    /// be the allocation-free unit variant rather than a freshly formatted `Error::Other`.
    #[test]
    fn process_audio_while_stopped_is_the_allocation_free_variant() {
        let mut plugin = unloaded_plugin();
        let mut buffers = AudioBuffers::new(0, 2, 64, 44_100.0);
        let err = plugin
            .process_audio(&mut buffers)
            .expect_err("processing is stopped");
        assert!(
            matches!(err, Error::NotProcessing),
            "expected Error::NotProcessing, got {err:?}"
        );
    }

    /// `send_midi_event`/`send_midi_event_at` used to forward whatever a caller built by hand,
    /// so an out-of-range field reached the VST3 conversion unmasked (velocity 255 → 2.008
    /// where the spec maximum is 1.0; a legacy CC value of 200 → a negative MIDI byte).
    #[test]
    fn send_midi_event_rejects_out_of_range_fields() {
        let mut plugin = unloaded_plugin();
        let bad = [
            MidiEvent::NoteOn {
                channel: MidiChannel::Ch1,
                note: 255,
                velocity: 100,
            },
            MidiEvent::NoteOn {
                channel: MidiChannel::Ch1,
                note: 60,
                velocity: 255,
            },
            MidiEvent::NoteOff {
                channel: MidiChannel::Ch1,
                note: 128,
                velocity: 0,
            },
            MidiEvent::ControlChange {
                channel: MidiChannel::Ch1,
                controller: 200,
                value: 0,
            },
            MidiEvent::ControlChange {
                channel: MidiChannel::Ch1,
                controller: 1,
                value: 200,
            },
            MidiEvent::ProgramChange {
                channel: MidiChannel::Ch1,
                program: 200,
            },
            MidiEvent::PitchBend {
                channel: MidiChannel::Ch1,
                value: 16_384,
            },
            MidiEvent::ChannelAftertouch {
                channel: MidiChannel::Ch1,
                pressure: 200,
            },
            MidiEvent::PolyAftertouch {
                channel: MidiChannel::Ch1,
                note: 200,
                pressure: 1,
            },
            MidiEvent::PolyAftertouch {
                channel: MidiChannel::Ch1,
                note: 60,
                pressure: 200,
            },
        ];
        for event in bad {
            for err in [
                plugin.send_midi_event(event).expect_err("out of range"),
                plugin
                    .send_midi_event_at(event, 0)
                    .expect_err("out of range"),
            ] {
                assert!(
                    matches!(err, Error::MidiError(_)),
                    "expected a MidiError for {event:?}, got {err:?}"
                );
            }
        }
    }

    /// In-range events pass validation and fail later, at the uninitialized plugin — proof the
    /// check rejects the field values and not the events themselves.
    #[test]
    fn send_midi_event_accepts_in_range_fields() {
        let mut plugin = unloaded_plugin();
        let ok = [
            MidiEvent::NoteOn {
                channel: MidiChannel::Ch1,
                note: 127,
                velocity: 127,
            },
            MidiEvent::ControlChange {
                channel: MidiChannel::Ch1,
                controller: 127,
                value: 127,
            },
            MidiEvent::ProgramChange {
                channel: MidiChannel::Ch1,
                program: 127,
            },
            MidiEvent::PitchBend {
                channel: MidiChannel::Ch1,
                value: 16_383,
            },
            MidiEvent::ChannelAftertouch {
                channel: MidiChannel::Ch1,
                pressure: 127,
            },
            MidiEvent::PolyAftertouch {
                channel: MidiChannel::Ch1,
                note: 127,
                pressure: 127,
            },
        ];
        for event in ok {
            let err = plugin.send_midi_event(event).expect_err("no plugin loaded");
            assert!(
                matches!(err, Error::Other(_)),
                "expected the uninitialized-plugin error for {event:?}, got {err:?}"
            );
        }
    }

    /// `note_on`/`note_on_at` mint a per-voice id, and skipped the range check its
    /// `send_midi_note` sibling performs.
    #[test]
    fn note_on_rejects_out_of_range_note_and_velocity() {
        let mut plugin = unloaded_plugin();
        for (note, velocity) in [(128, 100), (60, 128), (255, 255)] {
            let err = plugin
                .note_on(MidiChannel::Ch1, note, velocity)
                .expect_err("out of range");
            assert!(
                matches!(err, Error::MidiError(_)),
                "expected a MidiError for note {note} velocity {velocity}, got {err:?}"
            );
            let err = plugin
                .note_on_at(MidiChannel::Ch1, note, velocity, 0)
                .expect_err("out of range");
            assert!(matches!(err, Error::MidiError(_)), "got {err:?}");
        }
    }
}

#[cfg(test)]
mod output_midi_consumer_tests {
    use super::*;

    fn note(n: u8) -> MidiEvent {
        MidiEvent::NoteOn {
            channel: MidiChannel::Ch1,
            note: n,
            velocity: 100,
        }
    }

    #[test]
    fn drains_in_order_and_drops_oldest_when_full() {
        let q = Arc::new(ArrayQueue::new(2));
        let consumer = OutputMidiConsumer::from_queue(q.clone());

        // force_push mirrors what process() does: when full, the oldest is dropped.
        q.force_push(note(60).into());
        q.force_push(note(61).into());
        q.force_push(note(62).into()); // capacity 2 → drops note 60

        assert_eq!(consumer.drain(), vec![note(61), note(62)]);
        // Drained: now empty.
        assert_eq!(consumer.pop(), None);
        assert_eq!(consumer.drain(), vec![]);
    }

    #[test]
    fn handle_is_send_and_shares_the_queue_across_threads() {
        let q = Arc::new(ArrayQueue::new(8));
        let consumer = OutputMidiConsumer::from_queue(q.clone());
        // Push from another thread (the audio side is a different thread in practice).
        let producer = q.clone();
        std::thread::spawn(move || {
            producer.force_push(note(64).into());
        })
        .join()
        .unwrap();
        assert_eq!(consumer.pop(), Some(note(64)));
    }
}

#[cfg(test)]
mod vstpreset_tests {
    use super::{
        decode_state_snapshot, encode_state_snapshot, vstpreset, StateSnapshot,
        STATE_SNAPSHOT_MAGIC,
    };

    const TEST_CLASS_ID: &str = "0123456789ABCDEF0123456789ABCDEF";

    #[test]
    fn build_parse_round_trip() {
        let state = b"opaque plugin state \x00\x01\x02\xff bytes".to_vec();
        let controller = b"controller-only state".to_vec();
        let bytes = vstpreset::build(TEST_CLASS_ID, &state, Some(&controller)).expect("build");

        // Sanity-check the header layout.
        assert_eq!(&bytes[0..4], b"VST3");
        assert_eq!(
            i32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
            1
        );
        assert_eq!(&bytes[8..40], TEST_CLASS_ID.as_bytes());

        let parsed = vstpreset::parse(&bytes).expect("parse");
        assert_eq!(parsed.class_id, TEST_CLASS_ID);
        assert_eq!(parsed.component_state, state);
        assert_eq!(parsed.controller_state, Some(controller));
    }

    #[test]
    fn round_trip_empty_state() {
        let bytes = vstpreset::build(TEST_CLASS_ID, &[], None).expect("build");
        let parsed = vstpreset::parse(&bytes).expect("parse");
        assert_eq!(parsed.class_id, TEST_CLASS_ID);
        assert!(parsed.component_state.is_empty());
        assert!(parsed.controller_state.is_none());
    }

    #[test]
    fn round_trip_empty_controller_state() {
        let bytes = vstpreset::build(TEST_CLASS_ID, b"component", Some(&[])).expect("build");
        let parsed = vstpreset::parse(&bytes).expect("parse");
        assert_eq!(parsed.controller_state, Some(Vec::new()));
    }

    #[test]
    fn build_rejects_wrong_length_class_id() {
        assert!(vstpreset::build("short", b"x", None).is_err());
        assert!(vstpreset::build("Z123456789ABCDEF0123456789ABCDEF", b"x", None).is_err());
    }

    #[test]
    fn parse_rejects_bad_magic() {
        let mut bytes = vstpreset::build(TEST_CLASS_ID, b"x", None).expect("build");
        bytes[0] = b'X';
        assert!(vstpreset::parse(&bytes).is_err());
    }

    #[test]
    fn parse_rejects_truncated_header() {
        assert!(vstpreset::parse(b"VST3").is_err());
    }

    #[test]
    fn parse_rejects_out_of_bounds_list_offset() {
        let mut bytes = vstpreset::build(TEST_CLASS_ID, b"hello", None).expect("build");
        // Corrupt the list offset (bytes 40..48) to point past the end.
        let bad = (bytes.len() as i64 + 100).to_le_bytes();
        bytes[40..48].copy_from_slice(&bad);
        assert!(vstpreset::parse(&bytes).is_err());
    }

    #[test]
    fn parser_accepts_unknown_chunk_and_controller_before_component() {
        let comp = b"component";
        let cont = b"controller";
        let unknown = b"metadata";
        let body_len = comp.len() + cont.len() + unknown.len();
        let list_offset = 48 + body_len;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"VST3");
        bytes.extend_from_slice(&1i32.to_le_bytes());
        bytes.extend_from_slice(TEST_CLASS_ID.as_bytes());
        bytes.extend_from_slice(&(list_offset as i64).to_le_bytes());
        bytes.extend_from_slice(comp);
        bytes.extend_from_slice(cont);
        bytes.extend_from_slice(unknown);
        bytes.extend_from_slice(b"List");
        bytes.extend_from_slice(&3i32.to_le_bytes());
        for (id, offset, state) in [
            (b"Cont", 48 + comp.len(), cont.as_slice()),
            (b"Info", 48 + comp.len() + cont.len(), unknown.as_slice()),
            (b"Comp", 48, comp.as_slice()),
        ] {
            bytes.extend_from_slice(id);
            bytes.extend_from_slice(&(offset as i64).to_le_bytes());
            bytes.extend_from_slice(&(state.len() as i64).to_le_bytes());
        }
        let parsed = vstpreset::parse(&bytes).expect("parse");
        assert_eq!(parsed.component_state, comp);
        assert_eq!(parsed.controller_state.as_deref(), Some(cont.as_slice()));
    }

    #[test]
    fn parser_rejects_duplicate_or_overlapping_chunks() {
        let mut duplicate =
            vstpreset::build(TEST_CLASS_ID, b"component", Some(b"controller")).expect("build");
        let list_offset = i64::from_le_bytes(duplicate[40..48].try_into().unwrap()) as usize;
        duplicate[list_offset + 28..list_offset + 32].copy_from_slice(b"Comp");
        assert!(vstpreset::parse(&duplicate).is_err());

        let mut overlap =
            vstpreset::build(TEST_CLASS_ID, b"component", Some(b"controller")).expect("build");
        let list_offset = i64::from_le_bytes(overlap[40..48].try_into().unwrap()) as usize;
        let comp_offset = i64::from_le_bytes(
            overlap[list_offset + 12..list_offset + 20]
                .try_into()
                .unwrap(),
        );
        overlap[list_offset + 32..list_offset + 40]
            .copy_from_slice(&(comp_offset + 1).to_le_bytes());
        assert!(vstpreset::parse(&overlap).is_err());
    }

    #[test]
    fn state_snapshot_round_trip_and_legacy_component_compatibility() {
        let snapshot = StateSnapshot {
            component: b"component".to_vec(),
            controller: Some(b"controller".to_vec()),
        };
        let bytes = encode_state_snapshot(&snapshot).expect("encode");
        assert!(bytes.starts_with(STATE_SNAPSHOT_MAGIC));
        let decoded = decode_state_snapshot(&bytes).expect("decode");
        assert_eq!(decoded.component, snapshot.component);
        assert_eq!(decoded.controller, snapshot.controller);

        let legacy = decode_state_snapshot(b"old raw component blob").expect("legacy");
        assert_eq!(legacy.component, b"old raw component blob");
        assert!(legacy.controller.is_none());
    }

    #[test]
    fn state_snapshot_rejects_bad_lengths_and_versions() {
        let snapshot = StateSnapshot {
            component: b"component".to_vec(),
            controller: Some(b"controller".to_vec()),
        };
        let mut bytes = encode_state_snapshot(&snapshot).expect("encode");
        bytes[16..20].copy_from_slice(&2u32.to_le_bytes());
        assert!(decode_state_snapshot(&bytes).is_err());

        let mut bytes = encode_state_snapshot(&snapshot).expect("encode");
        bytes[20..24].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode_state_snapshot(&bytes).is_err());
    }
}

#[cfg(test)]
mod process_transport_tests {
    use super::ProcessTransport;

    fn transport() -> ProcessTransport {
        ProcessTransport {
            sample_position: 96000,
            quarter_note_position: 7.25,
            tempo: 90.0,
            playing: true,
            time_sig_numerator: 7,
            time_sig_denominator: 8,
        }
    }

    #[test]
    fn transport_validation_and_increment_preserve_independent_ppq() {
        let mut t = transport();
        assert!(t.validate().is_ok());
        t.advance(48000, 48000.0);
        assert_eq!(t.sample_position, 144000);
        assert_eq!(t.quarter_note_position, 8.75);
        t.tempo = 120.0;
        t.advance(48000, 48000.0);
        assert_eq!(t.quarter_note_position, 10.75);
        t.playing = false;
        let stopped = t;
        t.advance(48000, 48000.0);
        assert_eq!(t, stopped);
        for tempo in [0.0, -1.0, f64::INFINITY, f64::NAN] {
            assert!(ProcessTransport {
                tempo,
                ..transport()
            }
            .validate()
            .is_err());
        }
        assert!(ProcessTransport {
            quarter_note_position: f64::NAN,
            ..transport()
        }
        .validate()
        .is_err());
        assert!(ProcessTransport {
            time_sig_numerator: 0,
            ..transport()
        }
        .validate()
        .is_err());
        assert!(ProcessTransport {
            time_sig_denominator: 3,
            ..transport()
        }
        .validate()
        .is_err());
        assert!(ProcessTransport {
            sample_position: -48000,
            quarter_note_position: -2.0,
            ..transport()
        }
        .validate()
        .is_ok());
    }
}
