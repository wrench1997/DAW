//! Bounded, off-audio-thread plug-in processing for Mixer insert chains.
//!
//! Third-party plug-ins are never called by Citrus Studio's device callback. The callback
//! exchanges fixed-size, inline stereo blocks with one worker per Mixer insert. MIDI and realtime
//! parameter edits travel inside the audio block whose sequence they belong to. A whole slot chain
//! runs serially on that worker, so the bridge adds exactly one callback sequence per insert rather
//! than one block per plug-in. Missed deadlines return the preceding sequence's delayed dry audio;
//! a late or future processed block is never substituted for a different point on the timeline.

use std::{
    collections::HashSet,
    num::NonZeroU64,
    panic::{self, AssertUnwindSafe},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{self, AtomicBool, AtomicU32, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use rtrb::{Consumer, PopError, Producer, PushError, RingBuffer};

#[cfg(feature = "vst3")]
use super::installed_vst3_helper_path;
use super::{PluginDescriptor, PluginFormat};

/// Maximum block accepted by the plug-in bridge. This matches the Mixer's preallocated blocks.
pub const MAX_PLUGIN_BLOCK_FRAMES: usize = 2_048;
// Two maximum 2048-frame callback bursts plus margin at Q128. Queue allocation
// happens off the audio thread; default sequence latency remains one quantum.
const DEFAULT_QUEUE_CAPACITY: usize = 36;
const LEGACY_QUEUE_ADMISSION: usize = 4;
pub const MAX_MIDI_BRIDGE_LOOKAHEAD_QUANTA: usize = 16;
pub const MAX_PLUGIN_CHAIN_SLOTS: usize = 10;
/// Maximum number of parameters exposed by one plug-in instance to the generic control surface.
pub const MAX_PLUGIN_PARAMETER_CATALOG_ITEMS: usize = 4_096;
/// Maximum number of descriptors returned by one tagged catalog request.
pub const MAX_PLUGIN_PARAMETER_PAGE_ITEMS: usize = 64;
/// Maximum UTF-8 byte length of an emitted parameter name.
pub const MAX_PLUGIN_PARAMETER_NAME_BYTES: usize = 128;
/// Maximum UTF-8 byte length of an emitted parameter unit label.
pub const MAX_PLUGIN_PARAMETER_UNIT_BYTES: usize = 64;
const MAX_PLUGIN_PARAMETER_COMMAND_ERROR_BYTES: usize = 512;
const MAX_CHAIN_SLOTS: usize = MAX_PLUGIN_CHAIN_SLOTS;
const LATENCY_SNAPSHOT_READ_ATTEMPTS: usize = 3;
const MAX_ADMIN_COMMANDS_PER_WORKER_TURN: usize = 16;
const MAX_RT_EVENTS_PER_BLOCK: usize = 128;
const INITIAL_TRANSPORT_EPOCH: u64 = 1;
/// Maximum number of reliable callback-to-worker parameter edits that may be admitted per chain.
///
/// The receipt ring and the callback-to-worker failure ring have the same capacity. Admission is
/// released only after the control thread pops a receipt, so a successful tagged enqueue can
/// always terminate in exactly one non-droppable receipt without allocating on the callback.
pub const MAX_OUTSTANDING_PARAMETER_EDITS: u32 = 16;

/// Caller-owned nonzero identity for one reliable realtime parameter edit.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ParameterEditId(NonZeroU64);

impl ParameterEditId {
    #[must_use]
    pub const fn new(value: u64) -> Option<Self> {
        match NonZeroU64::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

impl From<NonZeroU64> for ParameterEditId {
    fn from(value: NonZeroU64) -> Self {
        Self(value)
    }
}

/// Copy-only terminal failure classification for a reliable realtime parameter edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParameterEditFailureReason {
    InputGap,
    EpochReset,
    WorkerEpochMismatch,
    SlotUnavailable,
    SlotFaulted,
    BackendRejected,
    BackendPanicked,
    EndpointDropped,
    WorkerStopped,
}

/// Exactly-once control-thread receipt for a successfully admitted tagged parameter edit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParameterEditReceipt {
    Applied {
        edit_id: ParameterEditId,
        slot: u8,
        id: u32,
        requested: f32,
        effective: f32,
        readback_confirmed: bool,
    },
    Failed {
        edit_id: ParameterEditId,
        slot: u8,
        id: u32,
        requested: f32,
        reason: ParameterEditFailureReason,
    },
}

/// Audio preparation shared by every plug-in in one insert chain.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PluginPrepareConfig {
    pub sample_rate: f64,
    pub max_block_frames: usize,
}

impl PluginPrepareConfig {
    fn validate(self) -> Result<Self, String> {
        if !(self.sample_rate.is_finite() && self.sample_rate > 0.0) {
            return Err(format!(
                "plug-in sample rate must be finite and positive, got {}",
                self.sample_rate
            ));
        }
        if !(1..=MAX_PLUGIN_BLOCK_FRAMES).contains(&self.max_block_frames) {
            return Err(format!(
                "plug-in block size must be in 1..={MAX_PLUGIN_BLOCK_FRAMES}, got {}",
                self.max_block_frames
            ));
        }
        Ok(self)
    }
}

/// A compact, sample-offset MIDI message suitable for one block's inline realtime event batch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MidiMessage {
    pub data: [u8; 3],
    pub sample_offset: u16,
}

/// Maximum MIDI1 messages emitted by one worker quantum. Loss invalidates the whole batch.
pub const MAX_PLUGIN_OUTPUT_EVENTS: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PluginMidiOutput {
    pub slot: u8,
    pub message: MidiMessage,
}

/// Output travels inside the same epoch/sequence/latency-attested block as its audio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PluginMidiBatch {
    pub events: [PluginMidiOutput; MAX_PLUGIN_OUTPUT_EVENTS],
    pub len: usize,
    pub lost: bool,
    /// Processing failed independently of whether generated MIDI is representable.
    pub audio_lost: bool,
}

impl Default for PluginMidiBatch {
    fn default() -> Self {
        Self {
            events: [PluginMidiOutput {
                slot: 0,
                message: MidiMessage {
                    data: [0; 3],
                    sample_offset: 0,
                },
            }; MAX_PLUGIN_OUTPUT_EVENTS],
            len: 0,
            lost: false,
            audio_lost: false,
        }
    }
}

impl PluginMidiBatch {
    pub fn push(&mut self, slot: u8, message: MidiMessage) {
        if self.len == self.events.len() {
            self.lost = true;
        } else {
            self.events[self.len] = PluginMidiOutput { slot, message };
            self.len += 1;
        }
    }
}

/// The authoritative musical context of the first input sample in a worker block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PluginTransport {
    pub sample_position: i64,
    pub quarter_note_position: f64,
    pub tempo: f64,
    pub playing: bool,
    pub time_sig_numerator: i32,
    pub time_sig_denominator: i32,
}

impl Default for PluginTransport {
    fn default() -> Self {
        Self {
            sample_position: 0,
            quarter_note_position: 0.0,
            tempo: 128.0,
            playing: false,
            time_sig_numerator: 4,
            time_sig_denominator: 4,
        }
    }
}

/// One bounded generic-control description of a plug-in parameter.
///
/// Catalog producers clamp normalized values and truncate `name`/`unit` at a UTF-8 boundary to
/// the public byte limits above. `None` means the plug-in format does not expose a trustworthy
/// default or step count; for VST3, `Some(0)` means a continuous parameter.
#[derive(Clone, Debug, PartialEq)]
pub struct PluginParameterDescriptor {
    pub id: u32,
    pub name: String,
    pub unit: String,
    pub current_normalized: f32,
    pub default_normalized: Option<f32>,
    pub step_count: Option<u32>,
    pub automatable: bool,
    pub read_only: bool,
    pub bypass: bool,
}

/// One backend-produced page before the worker attaches request and slot identity.
#[derive(Clone, Debug, PartialEq)]
pub struct PluginParameterCatalogPage {
    pub catalog_revision: u64,
    pub total_items: usize,
    pub items: Vec<PluginParameterDescriptor>,
}

impl MidiMessage {
    pub fn new(data: [u8; 3], sample_offset: usize) -> Self {
        Self {
            data,
            sample_offset: sample_offset.min(MAX_PLUGIN_BLOCK_FRAMES - 1) as u16,
        }
    }
}

/// Native editor commands run only on the insert worker; the HWND is logical owner data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeEditorCommand {
    Open {
        /// Windows logical HWND/PID; Linux uses a standalone helper window with no owner.
        owner: Option<(u64, u32)>,
    },
    Focus,
    Close,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NativeEditorState {
    pub supported: bool,
    pub has_editor: bool,
    pub open: bool,
    pub width: i32,
    pub height: i32,
    pub generation: u64,
}

/// One reliable control-plane publication per slot, independent of RuntimeEvent overflow.
/// This mutex is never read, locked, or modified by the device callback.
#[derive(Clone, Debug, Default)]
pub struct NativeEditorSnapshot {
    pub state: NativeEditorState,
    pub native_used: bool,
    pub dirty_revision: u64,
    pub captured_dirty_revision: u64,
    pub captured_generation: u64,
    pub parameter_capture_serial: u64,
    pub captured_parameters: Option<Arc<Vec<(u32, f32)>>>,
    pub capture_serial: u64,
    pub captured_state: Option<Arc<Vec<u8>>>,
    pub pending_request: Option<u64>,
    pub capture_in_progress: bool,
    pub completed_request: u64,
    pub error: Option<String>,
}

impl NativeEditorSnapshot {
    pub fn has_uncaptured_changes(&self) -> bool {
        self.state.open
            || self.capture_in_progress
            || self.pending_request.is_some()
            || self.dirty_revision != self.captured_dirty_revision
    }
}

#[derive(Clone, Debug, Default)]
pub struct NativeEditorFeedback {
    pub state: NativeEditorState,
    /// Monotonic, non-droppable revision scoped to this backend instance.
    pub dirty_revision: u64,
    pub catalog_invalidated: bool,
}

/// Common processing surface implemented by VST2, VST3 and deterministic test doubles.
///
/// Every method runs on the insert worker. Implementations may allocate, lock or perform IPC;
/// none of those operations occur on the device callback.
pub trait PluginBackend: 'static {
    fn name(&self) -> &str;
    fn prepare(&mut self, config: PluginPrepareConfig) -> Result<(), String>;
    fn process(&mut self, left: &mut [f32], right: &mut [f32], frames: usize)
    -> Result<(), String>;
    fn send_midi(&mut self, message: MidiMessage) -> Result<(), String>;
    fn midi_capabilities(&self) -> (bool, bool) {
        (false, false)
    }
    fn set_transport(&mut self, _transport: PluginTransport) -> Result<(), String> {
        Ok(())
    }
    /// Drain exactly the just-processed block. Defaults preserve non-MIDI-output backends.
    fn drain_midi_output(&mut self, _batch: &mut PluginMidiBatch, _slot: u8, _frames: usize) {}

    fn set_parameter(&mut self, id: u32, normalized: f32) -> Result<(), String>;
    fn get_parameter(&mut self, id: u32) -> Result<f32, String>;
    /// Return one bounded parameter-catalog page. Backends without a generic parameter surface
    /// remain source-compatible and expose an empty, revision-one catalog.
    fn parameter_catalog_page(
        &mut self,
        _cursor: usize,
        _limit: usize,
    ) -> Result<PluginParameterCatalogPage, String> {
        Ok(PluginParameterCatalogPage {
            catalog_revision: 1,
            total_items: 0,
            items: Vec::new(),
        })
    }
    /// Capture the complete generic parameter catalog for one worker-side paging session.
    ///
    /// Real format adapters override this to enumerate third-party metadata exactly once. The
    /// compatibility default assembles a snapshot from bounded pages for custom backends.
    fn parameter_catalog_snapshot(&mut self) -> Result<PluginParameterCatalogPage, String> {
        let mut first = self.parameter_catalog_page(0, MAX_PLUGIN_PARAMETER_PAGE_ITEMS)?;
        if first.total_items > MAX_PLUGIN_PARAMETER_CATALOG_ITEMS {
            return Err(format!(
                "plug-in exposes {} parameters; the generic catalog limit is {MAX_PLUGIN_PARAMETER_CATALOG_ITEMS}",
                first.total_items
            ));
        }
        let revision = first.catalog_revision;
        let total_items = first.total_items;
        if first.items.len() != total_items.min(MAX_PLUGIN_PARAMETER_PAGE_ITEMS) {
            return Err("plug-in returned a malformed first parameter catalog page".into());
        }
        let mut items = std::mem::take(&mut first.items);
        while items.len() < total_items {
            let cursor = items.len();
            let page = self.parameter_catalog_page(cursor, MAX_PLUGIN_PARAMETER_PAGE_ITEMS)?;
            if page.catalog_revision != revision || page.total_items != total_items {
                return Err("plug-in parameter catalog changed while capturing a snapshot".into());
            }
            let expected = MAX_PLUGIN_PARAMETER_PAGE_ITEMS.min(total_items - cursor);
            if page.items.len() != expected {
                return Err(format!(
                    "plug-in returned {} catalog items at cursor {cursor}; expected {expected}",
                    page.items.len()
                ));
            }
            items.extend(page.items);
        }
        Ok(PluginParameterCatalogPage {
            catalog_revision: revision,
            total_items,
            items,
        })
    }
    fn native_editor(&mut self, command: NativeEditorCommand) -> Result<NativeEditorState, String> {
        if command == NativeEditorCommand::Close {
            Ok(NativeEditorState::default())
        } else {
            Err("Native editor is not supported by this backend".into())
        }
    }
    fn native_editor_feedback(&mut self) -> Result<NativeEditorFeedback, String> {
        Ok(NativeEditorFeedback::default())
    }
    fn save_state(&mut self) -> Result<Vec<u8>, String>;
    fn load_state(&mut self, state: &[u8]) -> Result<(), String>;
    /// Reset transport-sensitive processing state after a seek, stop or loop discontinuity.
    ///
    /// This is always called on the plug-in worker, never on the device callback. Backends that
    /// expose a stronger stop/start or reset primitive should override it. The default keeps
    /// custom and legacy backends source-compatible; the worker still sends All Notes Off first.
    fn reset_processing(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn latency_samples(&self) -> u32;
    fn tail_samples(&self) -> u32;
}

/// Persistable information needed to construct one real plug-in instance on its worker.
#[derive(Clone, Debug)]
pub struct PluginLoadSpec {
    pub descriptor: PluginDescriptor,
    /// Optional VST3 class UID used when a bundle exports more than one audio class.
    pub class_uid: Option<String>,
    /// Explicit helper override. Production packaging normally resolves the helper beside the app.
    pub vst3_helper_path: Option<PathBuf>,
    pub initial_state: Vec<u8>,
    pub enabled: bool,
    pub bypassed: bool,
    pub wet: f32,
}

impl PluginLoadSpec {
    pub fn from_descriptor(descriptor: PluginDescriptor) -> Self {
        Self {
            descriptor,
            class_uid: None,
            vst3_helper_path: None,
            initial_state: Vec::new(),
            enabled: true,
            bypassed: false,
            wet: 1.0,
        }
    }
}

/// Runtime controls for one ordered chain slot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SlotConfig {
    pub enabled: bool,
    pub bypassed: bool,
    pub wet: f32,
}

impl Default for SlotConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            bypassed: false,
            wet: 1.0,
        }
    }
}

impl SlotConfig {
    fn normalized(self) -> Self {
        Self {
            enabled: self.enabled,
            bypassed: self.bypassed,
            wet: if self.wet.is_finite() {
                self.wet.clamp(0.0, 1.0)
            } else {
                1.0
            },
        }
    }
}

/// Coherent worker-published latency identity for one serial plug-in chain.
///
/// Slot latency excludes the callback/bridge quantum. Prefix sums therefore describe the
/// control delay before a parameter reaches a particular slot without double-counting the
/// audio block bridge. A zero bit in `active_mask` means that slot is disabled, bypassed,
/// missing, or fault-isolated and its latency entry is zero. `tail_samples` is the saturating
/// serial sum for the same active-slot identity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PluginLatencySnapshot {
    pub revision: u64,
    pub active_mask: u16,
    pub slot_latency_samples: [u32; MAX_PLUGIN_CHAIN_SLOTS],
    pub total_plugin_latency_samples: u32,
    pub tail_samples: u32,
}

impl PluginLatencySnapshot {
    #[must_use]
    pub const fn slot_is_active(self, slot: usize) -> bool {
        slot < MAX_PLUGIN_CHAIN_SLOTS && self.active_mask & (1_u16 << slot) != 0
    }

    /// Saturating latency of every active plug-in before `slot` in serial processing order.
    #[must_use]
    pub fn prefix_latency_before(self, slot: usize) -> Option<u32> {
        if slot >= MAX_PLUGIN_CHAIN_SLOTS {
            return None;
        }
        Some(
            self.slot_latency_samples[..slot]
                .iter()
                .copied()
                .enumerate()
                .filter_map(|(slot, latency)| self.slot_is_active(slot).then_some(latency))
                .fold(0_u32, u32::saturating_add),
        )
    }
}

/// Immutable slot-to-instance identity captured when an insert worker is spawned.
///
/// Identified manifests contain one nonzero, unique instance id for every physical slot in
/// serial processing order. Legacy constructors deliberately expose `None` identities rather
/// than deriving unstable ids from paths, names, or worker readiness. The value is copied into
/// both endpoint owners, so callback reads require no allocation, locking, or atomic retry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PluginEndpointManifest {
    pub slot_count: usize,
    pub instance_ids: [Option<u64>; MAX_PLUGIN_CHAIN_SLOTS],
    identified: bool,
}

impl PluginEndpointManifest {
    const fn unknown(slot_count: usize) -> Self {
        Self {
            slot_count,
            instance_ids: [None; MAX_PLUGIN_CHAIN_SLOTS],
            identified: false,
        }
    }

    /// Build a legacy manifest whose physical slot count is known but whose stable instance
    /// identities are deliberately unavailable.
    pub fn unknown_for_slots(slot_count: usize) -> Result<Self, String> {
        if slot_count > MAX_PLUGIN_CHAIN_SLOTS {
            return Err(format!(
                "an insert supports at most {MAX_PLUGIN_CHAIN_SLOTS} plug-in slots"
            ));
        }
        Ok(Self::unknown(slot_count))
    }

    /// Build an exact physical-slot manifest from caller-owned stable instance identities.
    pub fn identified(instance_ids: &[u64]) -> Result<Self, String> {
        if instance_ids.len() > MAX_PLUGIN_CHAIN_SLOTS {
            return Err(format!(
                "an insert supports at most {MAX_PLUGIN_CHAIN_SLOTS} plug-in slots"
            ));
        }
        let mut manifest = Self {
            slot_count: instance_ids.len(),
            instance_ids: [None; MAX_PLUGIN_CHAIN_SLOTS],
            identified: true,
        };
        for (slot, instance_id) in instance_ids.iter().copied().enumerate() {
            if instance_id == 0 {
                return Err(format!(
                    "plug-in instance id for slot {slot} must be nonzero"
                ));
            }
            if let Some(first_slot) = manifest.instance_ids[..slot]
                .iter()
                .position(|candidate| *candidate == Some(instance_id))
            {
                return Err(format!(
                    "duplicate plug-in instance id {instance_id} in slots {first_slot} and {slot}"
                ));
            }
            manifest.instance_ids[slot] = Some(instance_id);
        }
        Ok(manifest)
    }

    /// Whether every declared slot has a caller-supplied stable instance identity.
    #[must_use]
    pub const fn is_identified(self) -> bool {
        self.identified
    }

    #[must_use]
    pub const fn instance_id(self, slot: usize) -> Option<u64> {
        if slot < self.slot_count && slot < MAX_PLUGIN_CHAIN_SLOTS {
            self.instance_ids[slot]
        } else {
            None
        }
    }

    fn validate_exact(self) -> Result<(), PluginEndpointSnapshotError> {
        if self.slot_count > MAX_PLUGIN_CHAIN_SLOTS {
            return Err(PluginEndpointSnapshotError::SlotCountOutOfRange {
                slot_count: self.slot_count,
            });
        }
        if !self.identified {
            return Err(PluginEndpointSnapshotError::ManifestNotIdentified);
        }
        for slot in 0..self.slot_count {
            let Some(instance_id) = self.instance_ids[slot] else {
                return Err(PluginEndpointSnapshotError::MissingInstanceId { slot });
            };
            if instance_id == 0 {
                return Err(PluginEndpointSnapshotError::ZeroInstanceId { slot });
            }
            if let Some(first_slot) = self.instance_ids[..slot]
                .iter()
                .position(|candidate| *candidate == Some(instance_id))
            {
                return Err(PluginEndpointSnapshotError::DuplicateInstanceId {
                    first_slot,
                    duplicate_slot: slot,
                });
            }
        }
        for slot in self.slot_count..MAX_PLUGIN_CHAIN_SLOTS {
            if self.instance_ids[slot].is_some() {
                return Err(PluginEndpointSnapshotError::InstanceIdOutsideManifest { slot });
            }
        }
        Ok(())
    }
}

impl Default for PluginEndpointManifest {
    fn default() -> Self {
        Self::unknown(0)
    }
}

/// Why an exact endpoint manifest and a worker latency publication could not be paired.
///
/// This error is fixed-size so validation is safe to perform on the audio callback. Production
/// readers normally map an error to `None` and retry or fail closed without formatting it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginEndpointSnapshotError {
    SlotCountOutOfRange {
        slot_count: usize,
    },
    ManifestNotIdentified,
    MissingInstanceId {
        slot: usize,
    },
    ZeroInstanceId {
        slot: usize,
    },
    DuplicateInstanceId {
        first_slot: usize,
        duplicate_slot: usize,
    },
    InstanceIdOutsideManifest {
        slot: usize,
    },
    LatencyNotPublished,
    ActiveSlotOutsideManifest {
        active_mask: u16,
    },
    InactiveSlotHasLatency {
        slot: usize,
        latency_samples: u32,
    },
    TotalLatencyMismatch {
        expected: u32,
        actual: u32,
    },
}

/// One validated physical slot from an exact endpoint snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PluginEndpointSlotSnapshot {
    instance_id: u64,
    active: bool,
    latency_samples: u32,
    prefix_latency_samples: u32,
}

impl PluginEndpointSlotSnapshot {
    #[must_use]
    pub const fn instance_id(self) -> u64 {
        self.instance_id
    }

    #[must_use]
    pub const fn is_active(self) -> bool {
        self.active
    }

    #[must_use]
    pub const fn latency_samples(self) -> u32 {
        self.latency_samples
    }

    /// Saturating latency of active physical slots before this slot.
    #[must_use]
    pub const fn prefix_latency_samples(self) -> u32 {
        self.prefix_latency_samples
    }
}

/// Allocation-free pairing of immutable exact slot identity and one coherent latency publication.
///
/// The manifest is creation-time immutable for production endpoints. `latency` is one bounded
/// seqlock read, so `active_mask`, every slot latency, total latency, tail and revision all belong
/// to the same worker publication. Construction also rejects malformed copies supplied by custom
/// endpoint implementations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PluginEndpointSnapshot {
    manifest: PluginEndpointManifest,
    latency: PluginLatencySnapshot,
}

impl PluginEndpointSnapshot {
    pub fn try_new(
        manifest: PluginEndpointManifest,
        latency: PluginLatencySnapshot,
    ) -> Result<Self, PluginEndpointSnapshotError> {
        manifest.validate_exact()?;
        if latency.revision == 0 {
            return Err(PluginEndpointSnapshotError::LatencyNotPublished);
        }

        let valid_active_mask = if manifest.slot_count == 0 {
            0
        } else {
            (1_u16 << manifest.slot_count) - 1
        };
        if latency.active_mask & !valid_active_mask != 0 {
            return Err(PluginEndpointSnapshotError::ActiveSlotOutsideManifest {
                active_mask: latency.active_mask,
            });
        }

        let mut total_latency_samples = 0_u32;
        for (slot, latency_samples) in latency.slot_latency_samples.iter().copied().enumerate() {
            if !latency.slot_is_active(slot) {
                if latency_samples != 0 {
                    return Err(PluginEndpointSnapshotError::InactiveSlotHasLatency {
                        slot,
                        latency_samples,
                    });
                }
                continue;
            }
            total_latency_samples = total_latency_samples.saturating_add(latency_samples);
        }
        if total_latency_samples != latency.total_plugin_latency_samples {
            return Err(PluginEndpointSnapshotError::TotalLatencyMismatch {
                expected: total_latency_samples,
                actual: latency.total_plugin_latency_samples,
            });
        }

        Ok(Self { manifest, latency })
    }

    #[must_use]
    pub const fn manifest(self) -> PluginEndpointManifest {
        self.manifest
    }

    #[must_use]
    pub const fn latency(self) -> PluginLatencySnapshot {
        self.latency
    }

    #[must_use]
    pub const fn slot_count(self) -> usize {
        self.manifest.slot_count
    }

    #[must_use]
    pub const fn revision(self) -> u64 {
        self.latency.revision
    }

    #[must_use]
    pub const fn active_mask(self) -> u16 {
        self.latency.active_mask
    }

    #[must_use]
    pub const fn total_plugin_latency_samples(self) -> u32 {
        self.latency.total_plugin_latency_samples
    }

    #[must_use]
    pub const fn tail_samples(self) -> u32 {
        self.latency.tail_samples
    }

    /// Exact physical-slot identity and its delay metadata from this publication.
    #[must_use]
    pub fn slot(self, slot: usize) -> Option<PluginEndpointSlotSnapshot> {
        let instance_id = self.manifest.instance_id(slot)?;
        let prefix_latency_samples = self.latency.prefix_latency_before(slot)?;
        Some(PluginEndpointSlotSnapshot {
            instance_id,
            active: self.latency.slot_is_active(slot),
            latency_samples: self.latency.slot_latency_samples[slot],
            prefix_latency_samples,
        })
    }
}

/// Preloaded slot used by tests and by hosts that construct a custom backend.
pub struct BackendSlot {
    pub backend: Box<dyn PluginBackend>,
    pub config: SlotConfig,
}

impl BackendSlot {
    pub fn new(backend: Box<dyn PluginBackend>) -> Self {
        Self {
            backend,
            config: SlotConfig::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum LatencyAttestation {
    /// Legacy submissions do not bind processing to a latency publication.
    #[default]
    Unchecked,
    /// Every worker checkpoint observed the submitted coherent revision.
    Valid,
    /// At least one worker checkpoint observed another coherent revision.
    Invalid,
}

/// Fixed-size value moved through the two audio SPSC rings.
pub struct StereoBlock {
    transport: PluginTransport,
    midi_output: PluginMidiBatch,
    epoch: u64,
    sequence: u64,
    frames: u16,
    rt_event_count: u16,
    expected_latency_revision: u64,
    latency_attestation: LatencyAttestation,
    rt_events: [RtCommand; MAX_RT_EVENTS_PER_BLOCK],
    left: [f32; MAX_PLUGIN_BLOCK_FRAMES],
    right: [f32; MAX_PLUGIN_BLOCK_FRAMES],
}

impl StereoBlock {
    fn silence() -> Self {
        Self {
            transport: PluginTransport::default(),
            midi_output: PluginMidiBatch::default(),
            epoch: INITIAL_TRANSPORT_EPOCH,
            sequence: 0,
            frames: 0,
            rt_event_count: 0,
            expected_latency_revision: 0,
            latency_attestation: LatencyAttestation::Unchecked,
            rt_events: [RtCommand::EMPTY; MAX_RT_EVENTS_PER_BLOCK],
            left: [0.0; MAX_PLUGIN_BLOCK_FRAMES],
            right: [0.0; MAX_PLUGIN_BLOCK_FRAMES],
        }
    }

    fn frames(&self) -> usize {
        usize::from(self.frames)
    }

    fn rt_events(&self) -> &[RtCommand] {
        &self.rt_events[..usize::from(self.rt_event_count)]
    }
}

/// One callback block of dry audio retained for deterministic bridge fallback.
struct DryBlock {
    epoch: u64,
    sequence: u64,
    frames: u16,
    valid: bool,
    submitted: bool,
    left: [f32; MAX_PLUGIN_BLOCK_FRAMES],
    right: [f32; MAX_PLUGIN_BLOCK_FRAMES],
}

impl DryBlock {
    fn silence() -> Self {
        Self {
            epoch: INITIAL_TRANSPORT_EPOCH,
            sequence: 0,
            frames: 0,
            valid: false,
            submitted: false,
            left: [0.0; MAX_PLUGIN_BLOCK_FRAMES],
            right: [0.0; MAX_PLUGIN_BLOCK_FRAMES],
        }
    }

    fn store(&mut self, epoch: u64, sequence: u64, left: &[f32], right: &[f32], submitted: bool) {
        debug_assert_ne!(epoch, 0);
        self.epoch = epoch;
        self.sequence = sequence;
        self.frames = left.len() as u16;
        self.valid = true;
        self.submitted = submitted;
        self.left[..left.len()].copy_from_slice(left);
        self.right[..right.len()].copy_from_slice(right);
    }
}

/// Heap-backed callback scratch allocated before the endpoint is transferred to the audio engine.
/// Keeping it behind one pointer prevents the bounded `AudioCommand` ring from inheriting the
/// inline audio/event storage size.
struct EndpointScratch {
    transport: PluginTransport,
    midi_output: PluginMidiBatch,
    pending_rt_event_count: u16,
    pending_rt_events: [RtCommand; MAX_RT_EVENTS_PER_BLOCK],
    delayed_dry: DryBlock,
    bridge_lookahead_quanta: usize,
    future_outputs: Box<[StereoBlock]>,
    future_output_valid: [bool; DEFAULT_QUEUE_CAPACITY],
}

impl EndpointScratch {
    fn new() -> Self {
        Self {
            transport: PluginTransport::default(),
            midi_output: PluginMidiBatch::default(),
            pending_rt_event_count: 0,
            pending_rt_events: [RtCommand::EMPTY; MAX_RT_EVENTS_PER_BLOCK],
            delayed_dry: DryBlock::silence(),
            bridge_lookahead_quanta: 1,
            future_outputs: (0..DEFAULT_QUEUE_CAPACITY)
                .map(|_| StereoBlock::silence())
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            future_output_valid: [false; DEFAULT_QUEUE_CAPACITY],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RtTarget {
    Slot(u8),
    All,
}

#[derive(Clone, Copy, Debug)]
enum RtCommand {
    Midi {
        target: RtTarget,
        message: MidiMessage,
    },
    Parameter {
        slot: u8,
        id: u32,
        normalized: f32,
        edit_id: Option<ParameterEditId>,
    },
}

impl RtCommand {
    const EMPTY: Self = Self::Parameter {
        slot: 0,
        id: 0,
        normalized: 0.0,
        edit_id: None,
    };
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct DeferredParameterEditFailure {
    edit_id: ParameterEditId,
    slot: u8,
    id: u32,
    requested: f32,
    reason: ParameterEditFailureReason,
}

impl DeferredParameterEditFailure {
    const fn into_receipt(self) -> ParameterEditReceipt {
        ParameterEditReceipt::Failed {
            edit_id: self.edit_id,
            slot: self.slot,
            id: self.id,
            requested: self.requested,
            reason: self.reason,
        }
    }
}

/// Result of a callback-side nonblocking submit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubmitStatus {
    Submitted {
        sequence: u64,
    },
    /// The callback sequence advanced, but no worker input block was queued.
    Gap {
        sequence: u64,
    },
    InvalidFrameCount,
}

/// Result of a callback-side nonblocking receive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiveStatus {
    Processed {
        sequence: u64,
        frames: usize,
    },
    /// The worker advanced the plug-in chain, but its coherent latency revision did not match
    /// the revision bound to this block. The returned audio range is guaranteed silent.
    LatencyDrift {
        sequence: u64,
        frames: usize,
    },
    Empty,
    OutputTooSmall {
        required: usize,
    },
}

/// Callback-owned half of an insert bridge.
///
/// This type is intentionally not `Clone` or `Copy`: rtrb enforces one producer and one consumer.
/// It has no plugin object and its steady-state methods are lock-free and allocation-free. Replace
/// or drop it only after the device callback has acknowledged detachment.
pub struct AudioThreadEndpoint {
    input: Producer<StereoBlock>,
    output: Consumer<StereoBlock>,
    epoch: u64,
    next_sequence: u64,
    requested_epoch: Arc<AtomicU64>,
    scratch: Box<EndpointScratch>,
    metrics: Arc<BridgeMetrics>,
    manifest: PluginEndpointManifest,
    parameter_edit_admission: Arc<AtomicU32>,
    parameter_edit_failures: Producer<DeferredParameterEditFailure>,
}

impl AudioThreadEndpoint {
    /// Stable creation-time slot identity. This is a plain fixed-size copy on the callback.
    #[must_use]
    pub const fn plugin_endpoint_manifest(&self) -> PluginEndpointManifest {
        self.manifest
    }

    #[must_use]
    pub const fn endpoint_manifest(&self) -> PluginEndpointManifest {
        self.plugin_endpoint_manifest()
    }

    pub fn max_block_frames(&self) -> usize {
        self.metrics.max_block_frames.load(Ordering::Relaxed) as usize
    }

    /// Current nonzero transport generation attached to every submitted and accepted block.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Switch to a caller-provided nonzero transport generation.
    ///
    /// The operation is callback-safe: it performs only bounded SPSC pops, fixed-size cache
    /// clearing and atomics. A change immediately re-primes the one-block bridge, discards staged
    /// realtime events and makes every output from another epoch ineligible for rendering. `false`
    /// means the epoch was zero or already current.
    pub fn set_epoch(&mut self, epoch: u64) -> bool {
        if epoch == 0 || epoch == self.epoch {
            return false;
        }

        self.epoch = epoch;
        self.next_sequence = 1;
        self.metrics.midi_blocked.store(false, Ordering::Release);
        self.requested_epoch.store(epoch, Ordering::Release);
        self.metrics.current_epoch.store(epoch, Ordering::Release);
        self.metrics.epoch_resets.fetch_add(1, Ordering::Relaxed);

        let staged = u64::from(self.scratch.pending_rt_event_count);
        self.fail_pending_parameter_edits(ParameterEditFailureReason::EpochReset);
        self.scratch.pending_rt_event_count = 0;
        self.metrics
            .dropped_rt_events
            .fetch_add(staged, Ordering::Relaxed);
        self.scratch.delayed_dry = DryBlock::silence();
        self.scratch.midi_output = PluginMidiBatch::default();

        let held = self
            .scratch
            .future_output_valid
            .iter()
            .filter(|valid| **valid)
            .count() as u64;
        self.scratch.future_output_valid.fill(false);
        self.metrics
            .dropped_old_epoch_outputs
            .fetch_add(held, Ordering::Relaxed);

        // The output ring is fixed-capacity, so fully draining this callback-owned consumer is a
        // bounded operation. A worker racing after this point stamps its block with the old epoch;
        // the receive matcher rejects that block as well.
        let mut drained = 0_u64;
        for _ in 0..DEFAULT_QUEUE_CAPACITY {
            if self.output.pop().is_err() {
                break;
            }
            drained += 1;
        }
        self.metrics
            .dropped_old_epoch_outputs
            .fetch_add(drained, Ordering::Relaxed);
        true
    }

    /// Advance the transport generation, skipping the reserved zero value on wrap.
    pub fn reset_epoch(&mut self) -> u64 {
        let next = next_nonzero_epoch(self.epoch);
        let changed = self.set_epoch(next);
        debug_assert!(changed);
        next
    }

    pub fn try_submit(&mut self, left: &[f32], right: &[f32]) -> SubmitStatus {
        self.try_submit_with_expected_latency_revision(left, right, 0)
    }

    /// Submit one block bound to an exact coherent plug-in latency publication.
    ///
    /// A zero revision preserves the legacy unchecked bridge contract. A nonzero revision is
    /// attested by the worker before realtime events, after those events update metadata, and
    /// after plug-in processing. Any mismatch still advances the chain but returns a silent
    /// [`ReceiveStatus::LatencyDrift`] block for this exact sequence.
    pub fn try_submit_with_expected_latency_revision(
        &mut self,
        left: &[f32],
        right: &[f32],
        expected_latency_revision: u64,
    ) -> SubmitStatus {
        if left.len() != right.len() || left.is_empty() || left.len() > self.max_block_frames() {
            return SubmitStatus::InvalidFrameCount;
        }
        self.submit_valid_block(left, right, expected_latency_revision)
    }

    fn submit_valid_block(
        &mut self,
        left: &[f32],
        right: &[f32],
        expected_latency_revision: u64,
    ) -> SubmitStatus {
        debug_assert_ne!(self.epoch, 0);
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.wrapping_add(1);
        self.metrics
            .callback_sequences
            .fetch_add(1, Ordering::Relaxed);
        self.metrics.current_bridge_frames.store(
            (left.len() * self.scratch.bridge_lookahead_quanta) as u32,
            Ordering::Relaxed,
        );
        let event_count = usize::from(self.scratch.pending_rt_event_count);

        let capacity = self.input.buffer().capacity();
        let admitted_capacity = if self.scratch.bridge_lookahead_quanta == 1 {
            capacity.min(LEGACY_QUEUE_ADMISSION)
        } else {
            capacity
        };
        if capacity.saturating_sub(self.input.slots()) >= admitted_capacity {
            self.fail_pending_parameter_edits(ParameterEditFailureReason::InputGap);
            self.scratch.pending_rt_event_count = 0;
            self.scratch
                .delayed_dry
                .store(self.epoch, sequence, left, right, false);
            self.metrics.input_overflows.fetch_add(1, Ordering::Relaxed);
            self.metrics.input_gaps.fetch_add(1, Ordering::Relaxed);
            self.metrics
                .dropped_rt_events
                .fetch_add(event_count as u64, Ordering::Relaxed);
            return SubmitStatus::Gap { sequence };
        }

        let mut block = StereoBlock::silence();
        block.epoch = self.epoch;
        block.sequence = sequence;
        block.frames = left.len() as u16;
        block.transport = self.scratch.transport;
        block.rt_event_count = self.scratch.pending_rt_event_count;
        block.expected_latency_revision = expected_latency_revision;
        // Callback Q128 admission may reserve a tagged Live edit before the fixed-quantum
        // adapter has staged the Timeline events for that same block. Preserve each lane's
        // insertion order, but always materialize untagged System/Timeline work before tagged
        // Live edits in the worker block.
        copy_rt_commands_timeline_before_live(
            &self.scratch.pending_rt_events[..event_count],
            &mut block.rt_events[..event_count],
        );
        block.left[..left.len()].copy_from_slice(left);
        block.right[..right.len()].copy_from_slice(right);
        self.scratch.pending_rt_event_count = 0;
        match self.input.push(block) {
            Ok(()) => {
                self.scratch
                    .delayed_dry
                    .store(self.epoch, sequence, left, right, true);
                self.metrics.submitted.fetch_add(1, Ordering::Relaxed);
                SubmitStatus::Submitted { sequence }
            }
            Err(PushError::Full(block)) => {
                // `slots()` is only a snapshot. Preserve the sequence/gap invariant even if the
                // worker and callback raced between the capacity check and the push.
                self.scratch
                    .delayed_dry
                    .store(self.epoch, sequence, left, right, false);
                self.metrics.input_overflows.fetch_add(1, Ordering::Relaxed);
                self.metrics.input_gaps.fetch_add(1, Ordering::Relaxed);
                self.metrics
                    .dropped_rt_events
                    .fetch_add(event_count as u64, Ordering::Relaxed);
                self.fail_parameter_edits_in_block(&block, ParameterEditFailureReason::InputGap);
                SubmitStatus::Gap { sequence }
            }
        }
    }

    pub fn try_receive(&mut self, left: &mut [f32], right: &mut [f32]) -> ReceiveStatus {
        for index in 0..DEFAULT_QUEUE_CAPACITY {
            if !self.scratch.future_output_valid[index] {
                continue;
            }
            self.scratch.future_output_valid[index] = false;
            if self.scratch.future_outputs[index].epoch != self.epoch {
                self.metrics
                    .dropped_old_epoch_outputs
                    .fetch_add(1, Ordering::Relaxed);
                continue;
            }
            return copy_received_block(
                &self.scratch.future_outputs[index],
                left,
                right,
                &self.metrics,
                &mut self.scratch.midi_output,
            );
        }

        for _ in 0..DEFAULT_QUEUE_CAPACITY {
            let Ok(block) = self.output.pop() else {
                break;
            };
            if block.epoch != self.epoch {
                self.metrics
                    .dropped_old_epoch_outputs
                    .fetch_add(1, Ordering::Relaxed);
                continue;
            }
            return copy_received_block(
                &block,
                left,
                right,
                &self.metrics,
                &mut self.scratch.midi_output,
            );
        }
        ReceiveStatus::Empty
    }

    /// Submit the current block and render the exact preceding callback sequence. If that worker
    /// output missed its deadline, render that preceding sequence's retained dry audio. The first
    /// callback renders silence, establishing the bridge delay without ever returning current dry.
    pub fn process_realtime(
        &mut self,
        input_left: &[f32],
        input_right: &[f32],
        output_left: &mut [f32],
        output_right: &mut [f32],
    ) -> RealtimeProcessStatus {
        self.process_realtime_with_expected_latency_revision(
            input_left,
            input_right,
            output_left,
            output_right,
            0,
        )
    }

    /// Process one bridge turn and bind the block submitted by this call to `expected_revision`.
    /// The output consumed by this call still belongs to the preceding sequence and carries that
    /// sequence's own attestation.
    pub fn process_realtime_with_expected_latency_revision(
        &mut self,
        input_left: &[f32],
        input_right: &[f32],
        output_left: &mut [f32],
        output_right: &mut [f32],
        expected_revision: u64,
    ) -> RealtimeProcessStatus {
        let frames = input_left.len();
        if frames != input_right.len()
            || frames == 0
            || frames > self.max_block_frames()
            || output_left.len() < frames
            || output_right.len() < frames
        {
            return RealtimeProcessStatus::InvalidFrameCount;
        }
        self.scratch.midi_output = PluginMidiBatch::default();
        let sequence = self.next_sequence;
        let expected_sequence = if self.scratch.bridge_lookahead_quanta == 1 {
            sequence.wrapping_sub(1)
        } else {
            sequence.saturating_sub(self.scratch.bridge_lookahead_quanta as u64)
        };
        // Consume before submitting. This makes the bridge latency deterministic: even an
        // exceptionally fast worker can never return the block from this same callback.
        let received = if expected_sequence == 0 {
            ReceiveStatus::Empty
        } else {
            self.try_receive_expected(self.epoch, expected_sequence, output_left, output_right)
        };
        let source = match received {
            ReceiveStatus::Processed {
                frames: received_frames,
                ..
            } if received_frames == frames => RealtimeOutputSource::Plugin,
            ReceiveStatus::Processed { .. } => {
                self.metrics
                    .frame_mismatches
                    .fetch_add(1, Ordering::Relaxed);
                self.copy_expected_dry(expected_sequence, frames, output_left, output_right);
                RealtimeOutputSource::DelayedDry
            }
            ReceiveStatus::LatencyDrift {
                frames: received_frames,
                ..
            } => {
                if received_frames != frames {
                    self.metrics
                        .frame_mismatches
                        .fetch_add(1, Ordering::Relaxed);
                }
                // `copy_received_block` already guarantees silence. Repeat the fill here so this
                // branch remains fail-closed if receive internals are ever refactored.
                output_left[..frames].fill(0.0);
                output_right[..frames].fill(0.0);
                RealtimeOutputSource::LatencyDrift
            }
            ReceiveStatus::OutputTooSmall { required } => {
                if required != frames {
                    self.metrics
                        .frame_mismatches
                        .fetch_add(1, Ordering::Relaxed);
                }
                self.copy_expected_dry(expected_sequence, frames, output_left, output_right);
                RealtimeOutputSource::DelayedDry
            }
            ReceiveStatus::Empty => {
                if expected_sequence != 0
                    && (self.scratch.bridge_lookahead_quanta > 1
                        || (self.scratch.delayed_dry.valid
                            && self.scratch.delayed_dry.epoch == self.epoch
                            && self.scratch.delayed_dry.sequence == expected_sequence
                            && self.scratch.delayed_dry.submitted))
                {
                    self.metrics.deadline_misses.fetch_add(1, Ordering::Relaxed);
                }
                self.copy_expected_dry(expected_sequence, frames, output_left, output_right);
                RealtimeOutputSource::DelayedDry
            }
        };
        let submit = self.submit_valid_block(input_left, input_right, expected_revision);
        RealtimeProcessStatus::Processed {
            sequence: expected_sequence,
            frames,
            submit,
            source,
        }
    }

    fn copy_expected_dry(
        &mut self,
        expected_sequence: u64,
        frames: usize,
        left: &mut [f32],
        right: &mut [f32],
    ) {
        if self.scratch.delayed_dry.valid
            && self.scratch.delayed_dry.epoch == self.epoch
            && self.scratch.delayed_dry.sequence == expected_sequence
            && usize::from(self.scratch.delayed_dry.frames) == frames
        {
            left[..frames].copy_from_slice(&self.scratch.delayed_dry.left[..frames]);
            right[..frames].copy_from_slice(&self.scratch.delayed_dry.right[..frames]);
        } else {
            left[..frames].fill(0.0);
            right[..frames].fill(0.0);
        }
    }

    fn try_receive_expected(
        &mut self,
        expected_epoch: u64,
        expected_sequence: u64,
        left: &mut [f32],
        right: &mut [f32],
    ) -> ReceiveStatus {
        for index in 0..DEFAULT_QUEUE_CAPACITY {
            if !self.scratch.future_output_valid[index] {
                continue;
            }
            if self.scratch.future_outputs[index].epoch != expected_epoch {
                self.scratch.future_output_valid[index] = false;
                self.metrics
                    .dropped_old_epoch_outputs
                    .fetch_add(1, Ordering::Relaxed);
                continue;
            }
            match sequence_order(
                self.scratch.future_outputs[index].sequence,
                expected_sequence,
            ) {
                std::cmp::Ordering::Less => {
                    self.scratch.future_output_valid[index] = false;
                    self.metrics.stale_outputs.fetch_add(1, Ordering::Relaxed);
                }
                std::cmp::Ordering::Equal => {
                    self.scratch.future_output_valid[index] = false;
                    return copy_received_block(
                        &self.scratch.future_outputs[index],
                        left,
                        right,
                        &self.metrics,
                        &mut self.scratch.midi_output,
                    );
                }
                std::cmp::Ordering::Greater => {}
            }
        }

        for _ in 0..DEFAULT_QUEUE_CAPACITY {
            let Ok(block) = self.output.pop() else {
                return ReceiveStatus::Empty;
            };
            if block.epoch != expected_epoch {
                self.metrics
                    .dropped_old_epoch_outputs
                    .fetch_add(1, Ordering::Relaxed);
                continue;
            }
            match sequence_order(block.sequence, expected_sequence) {
                std::cmp::Ordering::Less => {
                    self.metrics.stale_outputs.fetch_add(1, Ordering::Relaxed);
                }
                std::cmp::Ordering::Equal => {
                    return copy_received_block(
                        &block,
                        left,
                        right,
                        &self.metrics,
                        &mut self.scratch.midi_output,
                    );
                }
                std::cmp::Ordering::Greater => {
                    if let Some(index) = self
                        .scratch
                        .future_output_valid
                        .iter()
                        .position(|valid| !*valid)
                    {
                        self.scratch.future_outputs[index] = block;
                        self.scratch.future_output_valid[index] = true;
                        self.metrics
                            .future_outputs_held
                            .fetch_add(1, Ordering::Relaxed);
                    } else {
                        self.metrics
                            .future_output_overflows
                            .fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
        ReceiveStatus::Empty
    }

    pub fn bridge_lookahead_quanta(&self) -> usize {
        self.scratch.bridge_lookahead_quanta
    }
    /// Only change at an epoch boundary before any input has been submitted.
    pub fn set_bridge_lookahead_quanta(&mut self, quanta: usize) -> bool {
        if self.next_sequence != 1 || !(1..=MAX_MIDI_BRIDGE_LOOKAHEAD_QUANTA).contains(&quanta) {
            return false;
        }
        self.scratch.bridge_lookahead_quanta = quanta;
        true
    }

    /// Reliable safety latch. The worker resets once and rejects all MIDI until a new epoch.
    pub fn block_midi_until_epoch(&self) {
        self.metrics.midi_blocked.store(true, Ordering::Release);
    }
    pub fn midi_capabilities(&self) -> (bool, bool) {
        let mask = self.metrics.midi_capabilities.load(Ordering::Acquire);
        (mask & 1 != 0, mask & (1 << 16) != 0)
    }

    pub fn set_transport(&mut self, transport: PluginTransport) {
        self.scratch.transport = transport;
    }

    pub fn take_midi_output(&mut self) -> PluginMidiBatch {
        std::mem::take(&mut self.scratch.midi_output)
    }

    pub fn try_send_midi(&mut self, slot: Option<usize>, message: MidiMessage) -> bool {
        let target = match slot {
            Some(slot) if manifest_accepts_slot(self.manifest, slot) => RtTarget::Slot(slot as u8),
            Some(_) => return false,
            None => RtTarget::All,
        };
        self.try_stage_rt(RtCommand::Midi { target, message })
    }

    pub fn try_set_parameter(&mut self, slot: usize, id: u32, normalized: f32) -> bool {
        if !manifest_accepts_slot(self.manifest, slot) || !normalized.is_finite() {
            return false;
        }
        self.try_stage_rt(RtCommand::Parameter {
            slot: slot as u8,
            id,
            normalized: normalized.clamp(0.0, 1.0),
            edit_id: None,
        })
    }

    /// Admit one reliable realtime parameter edit.
    ///
    /// `true` means the edit owns one of the chain's sixteen admission tokens and will produce
    /// exactly one [`ParameterEditReceipt`]. `false` means validation/capacity failed and no
    /// receipt will be produced. Admission is not released until the control thread pops the
    /// terminal receipt.
    pub fn try_set_parameter_tagged(
        &mut self,
        slot: usize,
        id: u32,
        normalized: f32,
        edit_id: ParameterEditId,
    ) -> bool {
        if !manifest_accepts_slot(self.manifest, slot) || !normalized.is_finite() {
            return false;
        }
        if !try_admit_parameter_edit(&self.parameter_edit_admission) {
            return false;
        }
        let accepted = self.try_stage_rt(RtCommand::Parameter {
            slot: slot as u8,
            id,
            normalized: normalized.clamp(0.0, 1.0),
            edit_id: Some(edit_id),
        });
        if !accepted {
            release_parameter_edit_admission(&self.parameter_edit_admission);
        }
        accepted
    }

    fn try_stage_rt(&mut self, command: RtCommand) -> bool {
        let index = usize::from(self.scratch.pending_rt_event_count);
        if index == MAX_RT_EVENTS_PER_BLOCK {
            self.metrics.rt_overflows.fetch_add(1, Ordering::Relaxed);
            self.metrics
                .inline_event_overflows
                .fetch_add(1, Ordering::Relaxed);
            return false;
        }
        self.scratch.pending_rt_events[index] = command;
        self.scratch.pending_rt_event_count += 1;
        true
    }

    fn fail_pending_parameter_edits(&mut self, reason: ParameterEditFailureReason) {
        let count = usize::from(self.scratch.pending_rt_event_count);
        for index in 0..count {
            if let RtCommand::Parameter {
                slot,
                id,
                normalized,
                edit_id: Some(edit_id),
            } = self.scratch.pending_rt_events[index]
            {
                self.push_deferred_parameter_edit_failure(DeferredParameterEditFailure {
                    edit_id,
                    slot,
                    id,
                    requested: normalized,
                    reason,
                });
            }
        }
    }

    fn fail_parameter_edits_in_block(
        &mut self,
        block: &StereoBlock,
        reason: ParameterEditFailureReason,
    ) {
        for command in block.rt_events().iter().copied() {
            if let RtCommand::Parameter {
                slot,
                id,
                normalized,
                edit_id: Some(edit_id),
            } = command
            {
                self.push_deferred_parameter_edit_failure(DeferredParameterEditFailure {
                    edit_id,
                    slot,
                    id,
                    requested: normalized,
                    reason,
                });
            }
        }
    }

    fn push_deferred_parameter_edit_failure(&mut self, failure: DeferredParameterEditFailure) {
        // At most sixteen tagged edits are admitted across every location (pending events, input
        // ring, worker call, deferred failures and receipts). Therefore this equal-capacity SPSC
        // cannot be full for a newly terminal edit unless the accounting invariant was violated.
        if self.parameter_edit_failures.push(failure).is_err() {
            panic!("parameter edit failure ring admission invariant violated");
        }
    }

    pub fn stats(&self) -> BridgeStats {
        self.metrics.snapshot()
    }

    /// Coherent per-slot latency metadata published by the serial worker.
    ///
    /// `None` means the worker has not published its initial snapshot yet or the bounded reader
    /// collided with an in-progress update. The callback must retain its previous graph snapshot
    /// and retry on a later block.
    pub fn plugin_latency_snapshot(&self) -> Option<PluginLatencySnapshot> {
        self.metrics.plugin_latency_snapshot()
    }

    /// Read exact slot identity together with one coherent worker latency publication.
    ///
    /// This is bounded and allocation-free. `None` covers an unknown/malformed manifest, an
    /// unpublished or colliding latency read, or an internally inconsistent publication.
    pub fn plugin_endpoint_snapshot(&self) -> Option<PluginEndpointSnapshot> {
        PluginEndpointSnapshot::try_new(self.manifest, self.plugin_latency_snapshot()?).ok()
    }
}

const fn rt_command_is_tagged_parameter(command: RtCommand) -> bool {
    matches!(
        command,
        RtCommand::Parameter {
            edit_id: Some(_),
            ..
        }
    )
}

fn copy_rt_commands_timeline_before_live(source: &[RtCommand], destination: &mut [RtCommand]) {
    debug_assert_eq!(source.len(), destination.len());
    let mut destination_index = 0;
    for tagged in [false, true] {
        for command in source.iter().copied() {
            if rt_command_is_tagged_parameter(command) == tagged {
                destination[destination_index] = command;
                destination_index += 1;
            }
        }
    }
    debug_assert_eq!(destination_index, source.len());
}

impl Drop for AudioThreadEndpoint {
    fn drop(&mut self) {
        self.fail_pending_parameter_edits(ParameterEditFailureReason::EndpointDropped);
        self.scratch.pending_rt_event_count = 0;
    }
}

fn try_admit_parameter_edit(admission: &AtomicU32) -> bool {
    let mut current = admission.load(Ordering::Acquire);
    loop {
        if current >= MAX_OUTSTANDING_PARAMETER_EDITS {
            return false;
        }
        match admission.compare_exchange_weak(
            current,
            current + 1,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => return true,
            Err(observed) => current = observed,
        }
    }
}

fn release_parameter_edit_admission(admission: &AtomicU32) {
    let previous = admission.fetch_sub(1, Ordering::AcqRel);
    debug_assert!(previous > 0 && previous <= MAX_OUTSTANDING_PARAMETER_EDITS);
}

fn copy_received_block(
    block: &StereoBlock,
    left: &mut [f32],
    right: &mut [f32],
    metrics: &BridgeMetrics,
    midi_output: &mut PluginMidiBatch,
) -> ReceiveStatus {
    let frames = block.frames();
    if left.len() < frames || right.len() < frames {
        metrics.output_too_small.fetch_add(1, Ordering::Relaxed);
        return ReceiveStatus::OutputTooSmall { required: frames };
    }
    if block.latency_attestation == LatencyAttestation::Invalid {
        left[..frames].fill(0.0);
        right[..frames].fill(0.0);
        metrics
            .latency_drift_outputs
            .fetch_add(1, Ordering::Relaxed);
        return ReceiveStatus::LatencyDrift {
            sequence: block.sequence,
            frames,
        };
    }
    left[..frames].copy_from_slice(&block.left[..frames]);
    right[..frames].copy_from_slice(&block.right[..frames]);
    *midi_output = block.midi_output;
    ReceiveStatus::Processed {
        sequence: block.sequence,
        frames,
    }
}

fn sequence_order(candidate: u64, expected: u64) -> std::cmp::Ordering {
    if candidate == expected {
        std::cmp::Ordering::Equal
    } else if candidate.wrapping_sub(expected) < (1_u64 << 63) {
        std::cmp::Ordering::Greater
    } else {
        std::cmp::Ordering::Less
    }
}

fn next_nonzero_epoch(epoch: u64) -> u64 {
    let next = epoch.wrapping_add(1);
    if next == 0 {
        INITIAL_TRANSPORT_EPOCH
    } else {
        next
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RealtimeProcessStatus {
    Processed {
        sequence: u64,
        frames: usize,
        submit: SubmitStatus,
        source: RealtimeOutputSource,
    },
    InvalidFrameCount,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RealtimeOutputSource {
    Plugin,
    DelayedDry,
    /// The matching worker block advanced normally but failed its coherent latency attestation.
    /// The associated output range is guaranteed silent.
    LatencyDrift,
}

enum AdminCommand {
    NativeEditor {
        slot: usize,
        request_id: u64,
        command: NativeEditorCommand,
        base_parameter_ids: Vec<u32>,
    },
    SetSlotConfig {
        slot: usize,
        config: SlotConfig,
    },
    SetParameter {
        slot: usize,
        id: u32,
        value: f32,
        request_id: u64,
    },
    QueryParameter {
        slot: usize,
        id: u32,
        request_id: u64,
    },
    RequestParameterPage {
        slot: usize,
        request_id: u64,
        cursor: u32,
        limit: u8,
    },
    SaveState {
        slot: usize,
        request_id: u64,
    },
    LoadState {
        slot: usize,
        state: Vec<u8>,
    },
    Shutdown,
}

/// Kind of tagged parameter command that failed without fault-isolating the plug-in slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginParameterCommand {
    Catalog,
    Set,
    Query,
}

/// Messages produced by the worker and drained by the UI/control thread.
#[derive(Clone, Debug, PartialEq)]
pub enum RuntimeEvent {
    SlotReady {
        slot: usize,
        name: String,
        latency_samples: u32,
        tail_samples: u32,
    },
    SlotFault {
        slot: usize,
        message: String,
    },
    ParameterValue {
        slot: usize,
        request_id: u64,
        id: u32,
        value: f32,
    },
    ParameterCatalogPage {
        slot: usize,
        request_id: u64,
        cursor: u32,
        next_cursor: Option<u32>,
        done: bool,
        catalog_revision: u64,
        items: Vec<PluginParameterDescriptor>,
    },
    ParameterSetAck {
        slot: usize,
        request_id: u64,
        id: u32,
        value: f32,
    },
    ParameterCommandFailed {
        slot: usize,
        request_id: u64,
        command: PluginParameterCommand,
        id: Option<u32>,
        message: String,
    },
    State {
        slot: usize,
        request_id: u64,
        bytes: Vec<u8>,
    },
    ShutdownComplete,
}

/// UI/control half of a running chain. Its bounded channel may lock; never use it in audio code.
pub struct PluginChainControl {
    admin: SyncSender<AdminCommand>,
    events: Consumer<RuntimeEvent>,
    metrics: Arc<BridgeMetrics>,
    manifest: PluginEndpointManifest,
    parameter_edit_receipts: Consumer<ParameterEditReceipt>,
    parameter_edit_admission: Arc<AtomicU32>,
}

impl PluginChainControl {
    pub fn midi_capabilities(&self) -> (bool, bool) {
        let mask = self.metrics.midi_capabilities.load(Ordering::Acquire);
        (mask & 1 != 0, mask & (1 << 16) != 0)
    }

    #[must_use]
    pub const fn plugin_endpoint_manifest(&self) -> PluginEndpointManifest {
        self.manifest
    }

    #[must_use]
    pub const fn endpoint_manifest(&self) -> PluginEndpointManifest {
        self.plugin_endpoint_manifest()
    }

    pub fn set_slot_config(&self, slot: usize, config: SlotConfig) -> bool {
        if !manifest_accepts_slot(self.manifest, slot) {
            return false;
        }
        self.try_admin(AdminCommand::SetSlotConfig { slot, config })
    }

    /// Queue a legacy untagged parameter edit. Success produces no acknowledgement event.
    pub fn set_parameter(&self, slot: usize, id: u32, value: f32) -> bool {
        self.set_parameter_inner(slot, id, value, 0)
    }

    /// Queue a parameter edit whose nonzero tag is returned in `ParameterSetAck` or
    /// `ParameterCommandFailed`. `false` means validation or bounded enqueue failed.
    pub fn set_parameter_tagged(&self, slot: usize, id: u32, value: f32, request_id: u64) -> bool {
        request_id != 0 && self.set_parameter_inner(slot, id, value, request_id)
    }

    fn set_parameter_inner(&self, slot: usize, id: u32, value: f32, request_id: u64) -> bool {
        if !manifest_accepts_slot(self.manifest, slot) || !value.is_finite() {
            return false;
        }
        self.try_admin(AdminCommand::SetParameter {
            slot,
            id,
            value: value.clamp(0.0, 1.0),
            request_id,
        })
    }

    /// Queue a legacy query whose `ParameterValue` response carries request id zero.
    pub fn query_parameter(&self, slot: usize, id: u32) -> bool {
        self.query_parameter_inner(slot, id, 0)
    }

    /// Queue a query whose nonzero tag is returned in `ParameterValue` or
    /// `ParameterCommandFailed`.
    pub fn query_parameter_tagged(&self, slot: usize, id: u32, request_id: u64) -> bool {
        request_id != 0 && self.query_parameter_inner(slot, id, request_id)
    }

    fn query_parameter_inner(&self, slot: usize, id: u32, request_id: u64) -> bool {
        if !manifest_accepts_slot(self.manifest, slot) {
            return false;
        }
        self.try_admin(AdminCommand::QueryParameter {
            slot,
            id,
            request_id,
        })
    }

    /// Queue one read-only catalog page. Requests are bounded to 64 descriptors and require a
    /// nonzero tag; catalog results never mutate plug-in or project parameter state.
    pub fn request_parameter_page(
        &self,
        slot: usize,
        request_id: u64,
        cursor: u32,
        limit: usize,
    ) -> bool {
        if request_id == 0
            || !manifest_accepts_slot(self.manifest, slot)
            || cursor as usize >= MAX_PLUGIN_PARAMETER_CATALOG_ITEMS
            || !(1..=MAX_PLUGIN_PARAMETER_PAGE_ITEMS).contains(&limit)
        {
            return false;
        }
        self.try_admin(AdminCommand::RequestParameterPage {
            slot,
            request_id,
            cursor,
            limit: limit as u8,
        })
    }

    pub fn native_editor_snapshot(&self, slot: usize) -> Option<NativeEditorSnapshot> {
        if !manifest_accepts_slot(self.manifest, slot) {
            return None;
        }
        self.metrics.native_editors.lock().ok()?.get(slot).cloned()
    }

    pub fn request_native_editor(
        &self,
        slot: usize,
        request_id: u64,
        command: NativeEditorCommand,
        base_parameter_ids: &[u32],
    ) -> bool {
        if request_id == 0
            || !manifest_accepts_slot(self.manifest, slot)
            || base_parameter_ids.len() > MAX_PLUGIN_PARAMETER_CATALOG_ITEMS
        {
            return false;
        }
        let Ok(mut snapshots) = self.metrics.native_editors.lock() else {
            return false;
        };
        let snapshot = &mut snapshots[slot];
        if snapshot.pending_request.is_some() {
            return false;
        }
        if self.try_admin(AdminCommand::NativeEditor {
            slot,
            request_id,
            command,
            base_parameter_ids: base_parameter_ids.to_vec(),
        }) {
            snapshot.pending_request = Some(request_id);
            snapshot.error = None;
            true
        } else {
            false
        }
    }

    pub fn request_state(&self, slot: usize) -> bool {
        self.request_state_tagged(slot, 0)
    }

    pub fn request_state_tagged(&self, slot: usize, request_id: u64) -> bool {
        if !manifest_accepts_slot(self.manifest, slot) {
            return false;
        }
        self.try_admin(AdminCommand::SaveState { slot, request_id })
    }

    pub fn load_state(&self, slot: usize, state: Vec<u8>) -> bool {
        if !manifest_accepts_slot(self.manifest, slot) {
            return false;
        }
        self.try_admin(AdminCommand::LoadState { slot, state })
    }

    pub fn try_next_event(&mut self) -> Option<RuntimeEvent> {
        self.events.pop().ok()
    }

    /// Pop one reliable realtime edit receipt and release its admission token.
    ///
    /// Receipts use a dedicated SPSC and are unaffected by best-effort `RuntimeEvent` overflow.
    pub fn try_next_parameter_edit_receipt(&mut self) -> Option<ParameterEditReceipt> {
        let receipt = self.parameter_edit_receipts.pop().ok()?;
        release_parameter_edit_admission(&self.parameter_edit_admission);
        Some(receipt)
    }

    #[must_use]
    pub fn outstanding_parameter_edits(&self) -> u32 {
        self.parameter_edit_admission.load(Ordering::Acquire)
    }

    pub fn stats(&self) -> BridgeStats {
        self.metrics.snapshot()
    }

    pub fn plugin_latency_snapshot(&self) -> Option<PluginLatencySnapshot> {
        self.metrics.plugin_latency_snapshot()
    }

    /// Control-side view of the same exact/coherent tuple exposed to the callback endpoint.
    pub fn plugin_endpoint_snapshot(&self) -> Option<PluginEndpointSnapshot> {
        PluginEndpointSnapshot::try_new(self.manifest, self.plugin_latency_snapshot()?).ok()
    }

    fn try_admin(&self, command: AdminCommand) -> bool {
        match self.admin.try_send(command) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                self.metrics.admin_overflows.fetch_add(1, Ordering::Relaxed);
                false
            }
            Err(TrySendError::Disconnected(_)) => false,
        }
    }
}

fn manifest_accepts_slot(manifest: PluginEndpointManifest, slot: usize) -> bool {
    slot < manifest.slot_count
        || (manifest.slot_count == 0 && !manifest.is_identified() && slot < MAX_CHAIN_SLOTS)
}

#[derive(Default)]
struct BridgeMetrics {
    midi_capabilities: AtomicU32,
    midi_blocked: AtomicBool,
    native_editors: Mutex<[NativeEditorSnapshot; MAX_PLUGIN_CHAIN_SLOTS]>,
    max_block_frames: AtomicU32,
    current_epoch: AtomicU64,
    epoch_resets: AtomicU64,
    worker_epoch_resets: AtomicU64,
    dropped_old_epoch_inputs: AtomicU64,
    dropped_old_epoch_outputs: AtomicU64,
    reset_faults: AtomicU64,
    callback_sequences: AtomicU64,
    submitted: AtomicU64,
    completed: AtomicU64,
    input_overflows: AtomicU64,
    input_gaps: AtomicU64,
    deadline_misses: AtomicU64,
    output_overflows: AtomicU64,
    output_too_small: AtomicU64,
    frame_mismatches: AtomicU64,
    stale_outputs: AtomicU64,
    future_outputs_held: AtomicU64,
    future_output_overflows: AtomicU64,
    rt_overflows: AtomicU64,
    inline_event_overflows: AtomicU64,
    dropped_rt_events: AtomicU64,
    admin_overflows: AtomicU64,
    event_overflows: AtomicU64,
    faults: AtomicU64,
    latency_drift_blocks: AtomicU64,
    latency_drift_outputs: AtomicU64,
    current_bridge_frames: AtomicU32,
    plugin_latency_samples: AtomicU32,
    tail_samples: AtomicU32,
    plugin_latency_sequence: AtomicU64,
    plugin_latency_revision: AtomicU64,
    plugin_active_mask: AtomicU32,
    plugin_slot_latency_samples: [AtomicU32; MAX_PLUGIN_CHAIN_SLOTS],
    detached_workers: AtomicU64,
    shutdown_timeouts: AtomicU64,
    stopped: AtomicBool,
}

impl BridgeMetrics {
    fn snapshot(&self) -> BridgeStats {
        let bridge_frames = u64::from(self.current_bridge_frames.load(Ordering::Relaxed));
        let plugin_latency = u64::from(self.plugin_latency_samples.load(Ordering::Relaxed));
        BridgeStats {
            current_epoch: self.current_epoch.load(Ordering::Acquire),
            epoch_resets: self.epoch_resets.load(Ordering::Relaxed),
            worker_epoch_resets: self.worker_epoch_resets.load(Ordering::Relaxed),
            dropped_old_epoch_inputs: self.dropped_old_epoch_inputs.load(Ordering::Relaxed),
            dropped_old_epoch_outputs: self.dropped_old_epoch_outputs.load(Ordering::Relaxed),
            reset_faults: self.reset_faults.load(Ordering::Relaxed),
            callback_sequences: self.callback_sequences.load(Ordering::Relaxed),
            submitted: self.submitted.load(Ordering::Relaxed),
            completed: self.completed.load(Ordering::Relaxed),
            input_overflows: self.input_overflows.load(Ordering::Relaxed),
            input_gaps: self.input_gaps.load(Ordering::Relaxed),
            deadline_misses: self.deadline_misses.load(Ordering::Relaxed),
            output_overflows: self.output_overflows.load(Ordering::Relaxed),
            output_too_small: self.output_too_small.load(Ordering::Relaxed),
            frame_mismatches: self.frame_mismatches.load(Ordering::Relaxed),
            stale_outputs: self.stale_outputs.load(Ordering::Relaxed),
            future_outputs_held: self.future_outputs_held.load(Ordering::Relaxed),
            future_output_overflows: self.future_output_overflows.load(Ordering::Relaxed),
            rt_overflows: self.rt_overflows.load(Ordering::Relaxed),
            inline_event_overflows: self.inline_event_overflows.load(Ordering::Relaxed),
            dropped_rt_events: self.dropped_rt_events.load(Ordering::Relaxed),
            admin_overflows: self.admin_overflows.load(Ordering::Relaxed),
            event_overflows: self.event_overflows.load(Ordering::Relaxed),
            faults: self.faults.load(Ordering::Relaxed),
            latency_drift_blocks: self.latency_drift_blocks.load(Ordering::Relaxed),
            latency_drift_outputs: self.latency_drift_outputs.load(Ordering::Relaxed),
            latency_samples: bridge_frames
                .saturating_add(plugin_latency)
                .min(u64::from(u32::MAX)) as u32,
            tail_samples: self.tail_samples.load(Ordering::Relaxed),
            detached_workers: self.detached_workers.load(Ordering::Relaxed),
            shutdown_timeouts: self.shutdown_timeouts.load(Ordering::Relaxed),
            stopped: self.stopped.load(Ordering::Relaxed),
        }
    }

    fn plugin_latency_snapshot(&self) -> Option<PluginLatencySnapshot> {
        for _ in 0..LATENCY_SNAPSHOT_READ_ATTEMPTS {
            // The first Acquire keeps every following payload load behind the even marker. The
            // Acquire fence keeps the validation load behind those payload loads. Together with
            // the writer's odd Acquire RMW and final Release store this is a bounded seqlock read:
            // the callback never waits for the worker and never allocates.
            let before = self.plugin_latency_sequence.load(Ordering::Acquire);
            if before & 1 != 0 {
                continue;
            }
            let revision = self.plugin_latency_revision.load(Ordering::Relaxed);
            let active_mask = self.plugin_active_mask.load(Ordering::Relaxed) as u16;
            let slot_latency_samples = std::array::from_fn(|slot| {
                self.plugin_slot_latency_samples[slot].load(Ordering::Relaxed)
            });
            let total_plugin_latency_samples = self.plugin_latency_samples.load(Ordering::Relaxed);
            let tail_samples = self.tail_samples.load(Ordering::Relaxed);
            atomic::fence(Ordering::Acquire);
            let after = self.plugin_latency_sequence.load(Ordering::Relaxed);
            if before == after {
                if revision == 0 {
                    // The worker has not published its initial (possibly empty) chain yet.
                    return None;
                }
                return Some(PluginLatencySnapshot {
                    revision,
                    active_mask,
                    slot_latency_samples,
                    total_plugin_latency_samples,
                    tail_samples,
                });
            }
        }
        None
    }
}

/// Lock-free best-effort diagnostics for bridge health and reported delay/tail.
///
/// Consumers making graph or compensation decisions must use [`PluginLatencySnapshot`], whose
/// plug-in latency, active-slot identity and tail are one coherent worker publication.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BridgeStats {
    /// Current nonzero callback-side transport generation.
    pub current_epoch: u64,
    /// Callback requests that switched to a different transport generation.
    pub epoch_resets: u64,
    /// Epoch switches applied by the worker before processing the new generation's first block.
    pub worker_epoch_resets: u64,
    /// Queued input blocks discarded because a newer callback epoch had already been requested.
    pub dropped_old_epoch_inputs: u64,
    /// Completed or cached blocks rejected because their epoch was no longer current.
    pub dropped_old_epoch_outputs: u64,
    /// Plug-in reset operations that failed or panicked and isolated the affected slot.
    pub reset_faults: u64,
    /// Valid callback blocks observed, including explicit input gaps.
    pub callback_sequences: u64,
    pub submitted: u64,
    pub completed: u64,
    pub input_overflows: u64,
    /// Valid callback sequences that could not be submitted to the worker.
    pub input_gaps: u64,
    /// Submitted preceding sequences whose processed output missed its callback deadline.
    pub deadline_misses: u64,
    pub output_overflows: u64,
    pub output_too_small: u64,
    pub frame_mismatches: u64,
    /// Older completed blocks discarded when the callback catches up after a stall.
    pub stale_outputs: u64,
    /// Future blocks retained rather than rendered before their callback sequence.
    pub future_outputs_held: u64,
    /// Future blocks discarded because the fixed callback-side holding cache was full.
    pub future_output_overflows: u64,
    pub rt_overflows: u64,
    /// MIDI/parameter events rejected because one callback's inline batch was full.
    pub inline_event_overflows: u64,
    /// Inline events discarded together with a callback input gap.
    pub dropped_rt_events: u64,
    pub admin_overflows: u64,
    pub event_overflows: u64,
    pub faults: u64,
    /// Worker blocks silenced because a nonzero expected coherent latency revision drifted.
    pub latency_drift_blocks: u64,
    /// Attestation-invalid blocks consumed by the callback, including held future outputs.
    pub latency_drift_outputs: u64,
    /// The most recent valid callback block plus active plug-in latencies. Before the first valid
    /// callback this contains plug-in latency only (the bridge contribution is zero).
    pub latency_samples: u32,
    pub tail_samples: u32,
    /// Workers detached from the control thread rather than joined there.
    pub detached_workers: u64,
    /// Explicit bounded shutdowns that expired and detached their worker.
    pub shutdown_timeouts: u64,
    pub stopped: bool,
}

/// Owns the worker lifetime. Keep this on the control thread, not in callback state.
pub struct PluginWorkerGuard {
    admin: SyncSender<AdminCommand>,
    join: Option<JoinHandle<()>>,
    metrics: Arc<BridgeMetrics>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShutdownOutcome {
    Joined,
    WorkerPanicked,
    TimedOutDetached,
}

impl PluginWorkerGuard {
    /// Request shutdown without waiting. This has the same nonblocking semantics as `Drop`.
    pub fn shutdown(self) {
        drop(self);
    }

    /// Request shutdown and wait at most `timeout` for orderly worker teardown.
    ///
    /// A timed-out worker is detached. Rust cannot safely kill a thread stuck in an in-process
    /// VST2 call, so that worker (and plug-in instance) may remain alive until the call returns or
    /// the process exits; the UI/control thread is never held indefinitely.
    pub fn shutdown_blocking(mut self, timeout: Duration) -> ShutdownOutcome {
        let started = Instant::now();
        let deadline = started.checked_add(timeout).unwrap_or(started);
        let mut command = AdminCommand::Shutdown;
        loop {
            match self.admin.try_send(command) {
                Ok(()) | Err(TrySendError::Disconnected(_)) => break,
                Err(TrySendError::Full(returned)) => {
                    command = returned;
                    if Instant::now() >= deadline {
                        return self.detach_after_timeout();
                    }
                    thread::sleep(Duration::from_millis(1));
                }
            }
        }

        let Some(join) = self.join.take() else {
            return ShutdownOutcome::Joined;
        };
        while !join.is_finished() {
            if Instant::now() >= deadline {
                self.join = Some(join);
                return self.detach_after_timeout();
            }
            thread::sleep(Duration::from_millis(1));
        }
        if join.join().is_ok() {
            ShutdownOutcome::Joined
        } else {
            ShutdownOutcome::WorkerPanicked
        }
    }

    fn detach_after_timeout(&mut self) -> ShutdownOutcome {
        if self.join.take().is_some() {
            self.metrics
                .shutdown_timeouts
                .fetch_add(1, Ordering::Relaxed);
            self.metrics
                .detached_workers
                .fetch_add(1, Ordering::Relaxed);
        }
        ShutdownOutcome::TimedOutDetached
    }
}

impl Drop for PluginWorkerGuard {
    fn drop(&mut self) {
        if self.join.take().is_some() {
            let _ = self.admin.try_send(AdminCommand::Shutdown);
            self.metrics
                .detached_workers
                .fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// The three ownership domains of one running insert chain.
pub struct PluginChain {
    pub audio: AudioThreadEndpoint,
    pub control: PluginChainControl,
    pub guard: PluginWorkerGuard,
}

impl PluginChain {
    pub fn spawn(specs: Vec<PluginLoadSpec>, config: PluginPrepareConfig) -> Result<Self, String> {
        let config = config.validate()?;
        if specs.len() > MAX_CHAIN_SLOTS {
            return Err(format!(
                "an insert supports at most {MAX_CHAIN_SLOTS} plug-in slots"
            ));
        }
        let manifest = PluginEndpointManifest::unknown(specs.len());
        Self::spawn_loader(config, manifest, move || {
            specs
                .into_iter()
                .map(|spec| {
                    let slot_config = SlotConfig {
                        enabled: spec.enabled,
                        bypassed: spec.bypassed,
                        wet: spec.wet,
                    };
                    load_backend(spec, config).map(|backend| BackendSlot {
                        backend,
                        config: slot_config,
                    })
                })
                .collect()
        })
    }

    /// Spawn a chain with caller-owned stable instance ids in exact physical slot order.
    ///
    /// Identity validation completes before a worker or callback endpoint is created. Paths,
    /// descriptor ids and worker load results never participate in endpoint identity.
    pub fn spawn_identified(
        specs: Vec<(u64, PluginLoadSpec)>,
        config: PluginPrepareConfig,
    ) -> Result<Self, String> {
        let config = config.validate()?;
        if specs.len() > MAX_CHAIN_SLOTS {
            return Err(format!(
                "an insert supports at most {MAX_CHAIN_SLOTS} plug-in slots"
            ));
        }
        let mut instance_ids = [0_u64; MAX_PLUGIN_CHAIN_SLOTS];
        for (slot, (instance_id, _)) in specs.iter().enumerate() {
            instance_ids[slot] = *instance_id;
        }
        let manifest = PluginEndpointManifest::identified(&instance_ids[..specs.len()])?;
        Self::spawn_loader(config, manifest, move || {
            specs
                .into_iter()
                .map(|(_, spec)| {
                    let slot_config = SlotConfig {
                        enabled: spec.enabled,
                        bypassed: spec.bypassed,
                        wet: spec.wet,
                    };
                    load_backend(spec, config).map(|backend| BackendSlot {
                        backend,
                        config: slot_config,
                    })
                })
                .collect()
        })
    }

    /// Spawn test/custom backends from a factory that runs inside the worker thread. Constructing
    /// them there lets legacy plug-in wrappers remain correctly thread-affine instead of claiming
    /// an unsound blanket `Send` implementation.
    pub fn spawn_with_backend_factory<F>(
        factory: F,
        config: PluginPrepareConfig,
    ) -> Result<Self, String>
    where
        F: FnOnce() -> Vec<BackendSlot> + Send + 'static,
    {
        let config = config.validate()?;
        Self::spawn_loader(config, PluginEndpointManifest::unknown(0), move || {
            let slots = factory();
            if slots.len() > MAX_CHAIN_SLOTS {
                return vec![Err(format!(
                    "an insert supports at most {MAX_CHAIN_SLOTS} plug-in slots"
                ))];
            }
            slots.into_iter().map(Ok).collect()
        })
    }

    #[cfg(test)]
    pub(crate) fn spawn_identified_with_backend_factory<F>(
        instance_ids: &[u64],
        factory: F,
        config: PluginPrepareConfig,
    ) -> Result<Self, String>
    where
        F: FnOnce() -> Vec<BackendSlot> + Send + 'static,
    {
        let config = config.validate()?;
        let manifest = PluginEndpointManifest::identified(instance_ids)?;
        Self::spawn_loader(config, manifest, move || {
            let slots = factory();
            if slots.len() != manifest.slot_count {
                return vec![Err("identified test factory slot count mismatch".to_owned())];
            }
            slots.into_iter().map(Ok).collect()
        })
    }

    fn spawn_loader<F>(
        config: PluginPrepareConfig,
        manifest: PluginEndpointManifest,
        loader: F,
    ) -> Result<Self, String>
    where
        F: FnOnce() -> Vec<Result<BackendSlot, String>> + Send + 'static,
    {
        let capacity = DEFAULT_QUEUE_CAPACITY;
        let (input_tx, input_rx) = RingBuffer::new(capacity);
        let (output_tx, output_rx) = RingBuffer::new(capacity);
        let (event_tx, event_rx) = RingBuffer::new(64);
        let (parameter_edit_failure_tx, parameter_edit_failure_rx) =
            RingBuffer::new(MAX_OUTSTANDING_PARAMETER_EDITS as usize);
        let (parameter_edit_receipt_tx, parameter_edit_receipt_rx) =
            RingBuffer::new(MAX_OUTSTANDING_PARAMETER_EDITS as usize);
        let (admin_tx, admin_rx) = mpsc::sync_channel(64);
        let metrics = Arc::new(BridgeMetrics::default());
        metrics
            .current_epoch
            .store(INITIAL_TRANSPORT_EPOCH, Ordering::Relaxed);
        metrics
            .max_block_frames
            .store(config.max_block_frames as u32, Ordering::Relaxed);
        let requested_epoch = Arc::new(AtomicU64::new(INITIAL_TRANSPORT_EPOCH));
        let parameter_edit_admission = Arc::new(AtomicU32::new(0));

        let worker_metrics = Arc::clone(&metrics);
        let worker_requested_epoch = Arc::clone(&requested_epoch);
        let join = thread::Builder::new()
            .name("citrus-plugin-chain".into())
            .spawn(move || {
                let slots = match panic::catch_unwind(AssertUnwindSafe(loader)) {
                    Ok(slots) => slots,
                    Err(payload) => vec![Err(format!(
                        "plug-in loader panicked: {}",
                        panic_payload_message(payload.as_ref())
                    ))],
                };
                run_worker(
                    slots,
                    config,
                    input_rx,
                    output_tx,
                    admin_rx,
                    event_tx,
                    parameter_edit_failure_rx,
                    parameter_edit_receipt_tx,
                    worker_metrics,
                    worker_requested_epoch,
                );
            })
            .map_err(|error| format!("unable to start plug-in worker: {error}"))?;

        Ok(Self {
            audio: AudioThreadEndpoint {
                input: input_tx,
                output: output_rx,
                epoch: INITIAL_TRANSPORT_EPOCH,
                next_sequence: 1,
                requested_epoch,
                scratch: Box::new(EndpointScratch::new()),
                metrics: Arc::clone(&metrics),
                manifest,
                parameter_edit_admission: Arc::clone(&parameter_edit_admission),
                parameter_edit_failures: parameter_edit_failure_tx,
            },
            control: PluginChainControl {
                admin: admin_tx.clone(),
                events: event_rx,
                metrics: Arc::clone(&metrics),
                manifest,
                parameter_edit_receipts: parameter_edit_receipt_rx,
                parameter_edit_admission,
            },
            guard: PluginWorkerGuard {
                admin: admin_tx,
                join: Some(join),
                metrics,
            },
        })
    }
}

struct WorkerSlot {
    backend: Option<Box<dyn PluginBackend>>,
    config: SlotConfig,
    fault: Option<String>,
    parameter_catalog_cache: Option<WorkerParameterCatalogCache>,
    native_base_ids: Vec<u32>,
}

struct WorkerParameterCatalogCache {
    request_id: u64,
    catalog_revision: u64,
    next_cursor: usize,
    items: Vec<PluginParameterDescriptor>,
}

#[allow(clippy::too_many_arguments)]
fn run_worker(
    loaded: Vec<Result<BackendSlot, String>>,
    config: PluginPrepareConfig,
    mut input: Consumer<StereoBlock>,
    mut output: Producer<StereoBlock>,
    admin: Receiver<AdminCommand>,
    mut events: Producer<RuntimeEvent>,
    mut parameter_edit_failures: Consumer<DeferredParameterEditFailure>,
    mut parameter_edit_receipts: Producer<ParameterEditReceipt>,
    metrics: Arc<BridgeMetrics>,
    requested_epoch: Arc<AtomicU64>,
) {
    let mut slots = Vec::with_capacity(loaded.len());
    for (slot_index, result) in loaded.into_iter().enumerate() {
        match result {
            Ok(mut slot) => {
                slot.config = slot.config.normalized();
                let prepared = catch_backend(|| slot.backend.prepare(config));
                match prepared {
                    Ok(()) => match backend_ready_metadata(slot.backend.as_ref()) {
                        Ok((name, latency_samples, tail_samples)) => {
                            let event = RuntimeEvent::SlotReady {
                                slot: slot_index,
                                name,
                                latency_samples,
                                tail_samples,
                            };
                            push_event(&mut events, event, &metrics);
                            slots.push(WorkerSlot {
                                backend: Some(slot.backend),
                                config: slot.config,
                                fault: None,
                                parameter_catalog_cache: None,
                                native_base_ids: Vec::new(),
                            });
                        }
                        Err(message) => {
                            register_fault(slot_index, message.clone(), &mut events, &metrics);
                            slots.push(WorkerSlot {
                                backend: Some(slot.backend),
                                config: slot.config,
                                fault: Some(message),
                                parameter_catalog_cache: None,
                                native_base_ids: Vec::new(),
                            });
                        }
                    },
                    Err(message) => {
                        register_fault(slot_index, message.clone(), &mut events, &metrics);
                        slots.push(WorkerSlot {
                            backend: None,
                            config: slot.config,
                            fault: Some(message),
                            parameter_catalog_cache: None,
                            native_base_ids: Vec::new(),
                        });
                    }
                }
            }
            Err(message) => {
                register_fault(slot_index, message.clone(), &mut events, &metrics);
                slots.push(WorkerSlot {
                    backend: None,
                    config: SlotConfig::default(),
                    fault: Some(message),
                    parameter_catalog_cache: None,
                    native_base_ids: Vec::new(),
                });
            }
        }
    }
    update_latency_and_tail(&mut slots, &mut events, &metrics);

    let mut dry = StereoBlock::silence();
    let mut active_epoch = INITIAL_TRANSPORT_EPOCH;
    let mut shutdown = false;
    let mut midi_panic_applied = false;
    let mut next_native_poll = Instant::now();
    while !shutdown {
        let requested = requested_epoch.load(Ordering::Acquire);
        if requested != 0 && requested != active_epoch {
            active_epoch = requested;
            dry = StereoBlock::silence();
            reset_worker_epoch(active_epoch, &mut slots, &mut events, &metrics);
            metrics.worker_epoch_resets.fetch_add(1, Ordering::Relaxed);
            update_latency_and_tail(&mut slots, &mut events, &metrics);
        }
        let blocked = metrics.midi_blocked.load(Ordering::Acquire);
        if blocked && !midi_panic_applied {
            reset_worker_epoch(active_epoch, &mut slots, &mut events, &metrics);
        }
        midi_panic_applied = blocked;
        if Instant::now() >= next_native_poll {
            poll_native_editors(&mut slots, &mut events, &metrics);
            update_latency_and_tail(&mut slots, &mut events, &metrics);
            next_native_poll = Instant::now() + Duration::from_millis(100);
        }
        drain_parameter_edit_failures(&mut parameter_edit_failures, &mut parameter_edit_receipts);
        let mut admin_metadata_changed = false;
        for _ in 0..MAX_ADMIN_COMMANDS_PER_WORKER_TURN {
            match admin.try_recv() {
                Ok(AdminCommand::Shutdown) => {
                    shutdown = true;
                    break;
                }
                Ok(command) => {
                    admin_metadata_changed |=
                        handle_admin(command, &mut slots, &mut events, &metrics);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    shutdown = true;
                    break;
                }
            }
        }
        if shutdown {
            break;
        }
        if admin_metadata_changed {
            update_latency_and_tail(&mut slots, &mut events, &metrics);
        }
        match input.pop() {
            Ok(mut block) => {
                let requested = requested_epoch.load(Ordering::Acquire);
                if block.epoch == 0 || block.epoch != requested {
                    fail_parameter_edits_from_worker_block(
                        &block,
                        ParameterEditFailureReason::WorkerEpochMismatch,
                        &mut parameter_edit_receipts,
                    );
                    metrics
                        .dropped_old_epoch_inputs
                        .fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                if block.epoch != active_epoch {
                    active_epoch = block.epoch;
                    dry = StereoBlock::silence();
                    reset_worker_epoch(active_epoch, &mut slots, &mut events, &metrics);
                    metrics.worker_epoch_resets.fetch_add(1, Ordering::Relaxed);
                    update_latency_and_tail(&mut slots, &mut events, &metrics);
                }
                let expected_latency_revision = block.expected_latency_revision;
                let mut latency_attestation_valid = expected_latency_revision == 0
                    || metrics.plugin_latency_revision.load(Ordering::Relaxed)
                        == expected_latency_revision;
                let mut rt_metadata_changed = false;
                let frames = block.frames();
                let had_rt_events = block.rt_event_count != 0;
                for command in block.rt_events().iter().copied() {
                    if matches!(command, RtCommand::Midi { .. })
                        && metrics.midi_blocked.load(Ordering::Acquire)
                    {
                        continue;
                    }
                    let command = clamp_rt_event_to_block(command, frames);
                    rt_metadata_changed |= handle_rt(
                        command,
                        &mut slots,
                        &mut events,
                        &mut parameter_edit_receipts,
                        &metrics,
                    );
                }
                block.rt_event_count = 0;
                if rt_metadata_changed || (expected_latency_revision != 0 && had_rt_events) {
                    update_latency_and_tail(&mut slots, &mut events, &metrics);
                }
                if expected_latency_revision != 0
                    && metrics.plugin_latency_revision.load(Ordering::Relaxed)
                        != expected_latency_revision
                {
                    latency_attestation_valid = false;
                }
                process_chain(
                    &mut slots,
                    &mut block,
                    &mut dry,
                    &mut events,
                    &metrics,
                    config.sample_rate,
                );
                if metrics.midi_blocked.load(Ordering::Acquire) {
                    block.midi_output.lost = true;
                    block.midi_output.audio_lost = true;
                    block.left[..frames].fill(0.0);
                    block.right[..frames].fill(0.0);
                }
                // A plug-in may change its reported latency or tail while processing without a
                // host parameter/configuration command. Polling on the worker after each block
                // keeps the callback's fixed snapshot current without calling plug-in code there.
                update_latency_and_tail(&mut slots, &mut events, &metrics);
                if expected_latency_revision == 0 {
                    block.latency_attestation = LatencyAttestation::Unchecked;
                } else if latency_attestation_valid
                    && metrics.plugin_latency_revision.load(Ordering::Relaxed)
                        == expected_latency_revision
                {
                    block.latency_attestation = LatencyAttestation::Valid;
                } else {
                    block.latency_attestation = LatencyAttestation::Invalid;
                    block.left[..frames].fill(0.0);
                    block.right[..frames].fill(0.0);
                    metrics.latency_drift_blocks.fetch_add(1, Ordering::Relaxed);
                }
                if block.epoch != requested_epoch.load(Ordering::Acquire) {
                    metrics
                        .dropped_old_epoch_outputs
                        .fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                match output.push(block) {
                    Ok(()) => {
                        metrics.completed.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(PushError::Full(_)) => {
                        metrics.output_overflows.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            // Active chains normally receive the next block before this timeout. A millisecond
            // avoids the extreme idle wakeup rate of a sub-100us poll while keeping workers free
            // of callback-side locks/syscalls (the callback deliberately does not call unpark).
            Err(PopError::Empty) => thread::park_timeout(Duration::from_millis(1)),
        }
    }

    drain_parameter_edit_failures(&mut parameter_edit_failures, &mut parameter_edit_receipts);
    while let Ok(block) = input.pop() {
        fail_parameter_edits_from_worker_block(
            &block,
            ParameterEditFailureReason::WorkerStopped,
            &mut parameter_edit_receipts,
        );
    }

    for (slot, runtime) in slots.iter_mut().enumerate() {
        if let Some(backend) = runtime.backend.as_mut() {
            let _ = catch_backend(|| backend.native_editor(NativeEditorCommand::Close));
        }
        if let Ok(mut snapshots) = metrics.native_editors.lock() {
            snapshots[slot].state.open = false;
            snapshots[slot].state.width = 0;
            snapshots[slot].state.height = 0;
        }
    }
    metrics.stopped.store(true, Ordering::Release);
    push_event(&mut events, RuntimeEvent::ShutdownComplete, &metrics);
}

fn push_parameter_edit_receipt(
    receipts: &mut Producer<ParameterEditReceipt>,
    receipt: ParameterEditReceipt,
) {
    // Receipt occupancy plus admitted edits in all earlier pipeline stages is at most sixteen.
    // Since admission is released only after a receipt pop, an equal-capacity ring always owns a
    // slot for the next terminal edit.
    if receipts.push(receipt).is_err() {
        panic!("parameter edit receipt ring admission invariant violated");
    }
}

fn drain_parameter_edit_failures(
    failures: &mut Consumer<DeferredParameterEditFailure>,
    receipts: &mut Producer<ParameterEditReceipt>,
) {
    while let Ok(failure) = failures.pop() {
        push_parameter_edit_receipt(receipts, failure.into_receipt());
    }
}

fn fail_parameter_edits_from_worker_block(
    block: &StereoBlock,
    reason: ParameterEditFailureReason,
    receipts: &mut Producer<ParameterEditReceipt>,
) {
    for command in block.rt_events().iter().copied() {
        if let RtCommand::Parameter {
            slot,
            id,
            normalized,
            edit_id: Some(edit_id),
        } = command
        {
            push_parameter_edit_receipt(
                receipts,
                ParameterEditReceipt::Failed {
                    edit_id,
                    slot,
                    id,
                    requested: normalized,
                    reason,
                },
            );
        }
    }
}

fn catch_backend<T>(operation: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    match panic::catch_unwind(AssertUnwindSafe(operation)) {
        Ok(result) => result,
        Err(payload) => Err(format!(
            "plug-in panicked: {}",
            panic_payload_message(payload.as_ref())
        )),
    }
}

fn reset_worker_epoch(
    epoch: u64,
    slots: &mut [WorkerSlot],
    events: &mut Producer<RuntimeEvent>,
    metrics: &BridgeMetrics,
) {
    for (slot_index, slot) in slots.iter_mut().enumerate() {
        if slot.fault.is_some() {
            continue;
        }
        let Some(backend) = slot.backend.as_mut() else {
            continue;
        };

        let mut first_error = None;
        for channel in 0_u8..16 {
            for controller in [64, 123, 120] {
                let message = MidiMessage::new([0xb0 | channel, controller, 0], 0);
                if let Err(message) = catch_backend(|| backend.send_midi(message)) {
                    first_error = Some(format!("MIDI safety reset failed: {message}"));
                    break;
                }
            }
        }
        if let Err(message) = catch_backend(|| backend.reset_processing())
            && first_error.is_none()
        {
            first_error = Some(format!("processing reset failed: {message}"));
        }

        if let Some(message) = first_error {
            let message = format!("transport epoch {epoch} reset failed: {message}");
            slot.fault = Some(message.clone());
            metrics.reset_faults.fetch_add(1, Ordering::Relaxed);
            register_fault(slot_index, message, events, metrics);
        }
    }
}

fn backend_metadata(backend: &dyn PluginBackend) -> Result<(u32, u32), String> {
    catch_backend(|| Ok((backend.latency_samples(), backend.tail_samples())))
}

fn backend_ready_metadata(backend: &dyn PluginBackend) -> Result<(String, u32, u32), String> {
    catch_backend(|| {
        Ok((
            backend.name().to_owned(),
            backend.latency_samples(),
            backend.tail_samples(),
        ))
    })
}

fn panic_payload_message(payload: &(dyn std::any::Any + Send)) -> &str {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("unknown panic")
}

fn process_chain(
    slots: &mut [WorkerSlot],
    block: &mut StereoBlock,
    dry: &mut StereoBlock,
    events: &mut Producer<RuntimeEvent>,
    metrics: &BridgeMetrics,
    sample_rate: f64,
) {
    let frames = block.frames();
    let mut latency_prefix = 0_u32;
    for (slot_index, slot) in slots.iter_mut().enumerate() {
        if !slot.config.enabled || slot.config.bypassed || slot.fault.is_some() {
            block.midi_output.lost = true;
            block.midi_output.audio_lost |= slot.fault.is_some();
            continue;
        }
        let Some(backend) = slot.backend.as_mut() else {
            continue;
        };
        dry.left[..frames].copy_from_slice(&block.left[..frames]);
        dry.right[..frames].copy_from_slice(&block.right[..frames]);
        let mut transport = block.transport;
        transport.sample_position = transport
            .sample_position
            .saturating_sub(i64::from(latency_prefix));
        transport.quarter_note_position -=
            f64::from(latency_prefix) * transport.tempo / (60.0 * sample_rate);
        let result = catch_backend(|| {
            backend.set_transport(transport)?;
            backend.process(
                &mut block.left[..frames],
                &mut block.right[..frames],
                frames,
            )?;
            backend.drain_midi_output(&mut block.midi_output, slot_index as u8, frames);
            Ok(())
        });
        let finite = block.left[..frames]
            .iter()
            .chain(&block.right[..frames])
            .all(|sample| sample.is_finite());
        if let Err(message) = result {
            block.midi_output.lost = true;
            block.midi_output.audio_lost = true;
            block.left[..frames].copy_from_slice(&dry.left[..frames]);
            block.right[..frames].copy_from_slice(&dry.right[..frames]);
            slot.fault = Some(message.clone());
            register_fault(slot_index, message, events, metrics);
            continue;
        }
        if !finite {
            block.midi_output.lost = true;
            block.midi_output.audio_lost = true;
            block.left[..frames].copy_from_slice(&dry.left[..frames]);
            block.right[..frames].copy_from_slice(&dry.right[..frames]);
            let message = "plug-in produced non-finite audio".to_owned();
            slot.fault = Some(message.clone());
            register_fault(slot_index, message, events, metrics);
            continue;
        }
        // Snapshot is worker-owned and coherent for this block; any process-time latency
        // change invalidates the existing block attestation before callback consumption.
        latency_prefix = latency_prefix.saturating_add(
            metrics.plugin_slot_latency_samples[slot_index].load(Ordering::Relaxed),
        );
        let wet = slot.config.wet;
        if wet < 1.0 {
            let dry_gain = 1.0 - wet;
            for frame in 0..frames {
                block.left[frame] = dry.left[frame] * dry_gain + block.left[frame] * wet;
                block.right[frame] = dry.right[frame] * dry_gain + block.right[frame] * wet;
            }
        }
    }
}

fn clamp_rt_event_to_block(command: RtCommand, frames: usize) -> RtCommand {
    match command {
        RtCommand::Midi {
            target,
            mut message,
        } => {
            message.sample_offset =
                usize::from(message.sample_offset).min(frames.saturating_sub(1)) as u16;
            RtCommand::Midi { target, message }
        }
        parameter @ RtCommand::Parameter { .. } => parameter,
    }
}

fn handle_rt(
    command: RtCommand,
    slots: &mut [WorkerSlot],
    events: &mut Producer<RuntimeEvent>,
    parameter_edit_receipts: &mut Producer<ParameterEditReceipt>,
    metrics: &BridgeMetrics,
) -> bool {
    match command {
        RtCommand::Midi { target, message } => match target {
            RtTarget::Slot(slot) => {
                with_backend(slot as usize, slots, events, metrics, |backend| {
                    backend.send_midi(message)
                })
            }
            RtTarget::All => {
                let mut faulted = false;
                for slot in 0..slots.len() {
                    faulted |= with_backend(slot, slots, events, metrics, |backend| {
                        backend.send_midi(message)
                    });
                }
                faulted
            }
        },
        RtCommand::Parameter {
            slot,
            id,
            normalized,
            edit_id,
        } => {
            if let Some(edit_id) = edit_id {
                handle_tagged_rt_parameter(
                    edit_id,
                    slot,
                    id,
                    normalized,
                    slots,
                    events,
                    parameter_edit_receipts,
                    metrics,
                )
            } else {
                match call_parameter_backend(
                    slot as usize,
                    slots,
                    events,
                    metrics,
                    0,
                    PluginParameterCommand::Set,
                    Some(id),
                    |backend| backend.set_parameter(id, normalized),
                ) {
                    // A successful parameter edit may alter look-ahead or tail length.
                    ParameterCommandOutcome::Success(()) | ParameterCommandOutcome::Faulted => true,
                    ParameterCommandOutcome::Rejected => false,
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_tagged_rt_parameter(
    edit_id: ParameterEditId,
    slot: u8,
    id: u32,
    requested: f32,
    slots: &mut [WorkerSlot],
    events: &mut Producer<RuntimeEvent>,
    receipts: &mut Producer<ParameterEditReceipt>,
    metrics: &BridgeMetrics,
) -> bool {
    let failed = |receipts: &mut Producer<ParameterEditReceipt>, reason| {
        push_parameter_edit_receipt(
            receipts,
            ParameterEditReceipt::Failed {
                edit_id,
                slot,
                id,
                requested,
                reason,
            },
        );
    };
    let slot_index = usize::from(slot);
    let Some(runtime) = slots.get_mut(slot_index) else {
        failed(receipts, ParameterEditFailureReason::SlotUnavailable);
        return false;
    };
    if runtime.fault.is_some() {
        failed(receipts, ParameterEditFailureReason::SlotFaulted);
        return false;
    }
    let Some(backend) = runtime.backend.as_mut() else {
        failed(receipts, ParameterEditFailureReason::SlotUnavailable);
        return false;
    };

    match catch_parameter_backend(|| backend.set_parameter(id, requested)) {
        Ok(()) => match catch_parameter_backend(|| backend.get_parameter(id)) {
            Ok(effective) if effective.is_finite() => {
                push_parameter_edit_receipt(
                    receipts,
                    ParameterEditReceipt::Applied {
                        edit_id,
                        slot,
                        id,
                        requested,
                        effective: effective.clamp(0.0, 1.0),
                        readback_confirmed: true,
                    },
                );
                true
            }
            Ok(_) | Err(ParameterBackendError::Rejected(_)) => {
                push_parameter_edit_receipt(
                    receipts,
                    ParameterEditReceipt::Applied {
                        edit_id,
                        slot,
                        id,
                        requested,
                        effective: requested,
                        readback_confirmed: false,
                    },
                );
                true
            }
            Err(ParameterBackendError::Panicked(message)) => {
                runtime.fault = Some(message.clone());
                register_fault(slot_index, message, events, metrics);
                failed(receipts, ParameterEditFailureReason::BackendPanicked);
                true
            }
        },
        Err(ParameterBackendError::Rejected(_)) => {
            failed(receipts, ParameterEditFailureReason::BackendRejected);
            false
        }
        Err(ParameterBackendError::Panicked(message)) => {
            runtime.fault = Some(message.clone());
            register_fault(slot_index, message, events, metrics);
            failed(receipts, ParameterEditFailureReason::BackendPanicked);
            true
        }
    }
}

fn handle_admin(
    command: AdminCommand,
    slots: &mut [WorkerSlot],
    events: &mut Producer<RuntimeEvent>,
    metrics: &BridgeMetrics,
) -> bool {
    match command {
        AdminCommand::NativeEditor {
            slot,
            request_id,
            command,
            base_parameter_ids,
        } => {
            if slot >= MAX_PLUGIN_CHAIN_SLOTS {
                return false;
            }
            let was_faulted = slots
                .get(slot)
                .is_some_and(|runtime| runtime.fault.is_some());
            let result = slots
                .get_mut(slot)
                .filter(|runtime| runtime.fault.is_none() || command == NativeEditorCommand::Close)
                .and_then(|runtime| runtime.backend.as_mut())
                .ok_or_else(|| "plug-in runtime is not available".to_owned())
                .and_then(|backend| catch_backend(|| backend.native_editor(command)));
            if result.is_ok()
                && let Some(runtime) = slots.get_mut(slot)
            {
                runtime.native_base_ids = base_parameter_ids;
            }
            if let Ok(mut snapshots) = metrics.native_editors.lock() {
                let snapshot = &mut snapshots[slot];
                snapshot.capture_in_progress =
                    command == NativeEditorCommand::Close && result.is_ok();
                if was_faulted && command == NativeEditorCommand::Close {
                    // DSP failure can precede the next native feedback poll. A detached view
                    // is not evidence that its last changes were captured successfully.
                    snapshot.dirty_revision = snapshot
                        .dirty_revision
                        .max(snapshot.captured_dirty_revision.saturating_add(1));
                    snapshot.error = slots.get(slot).and_then(|runtime| runtime.fault.clone());
                }
                match result {
                    Ok(state) => {
                        snapshot.state = state;
                        snapshot.native_used |= state.open;
                        if !was_faulted {
                            snapshot.error = None;
                        }
                    }
                    Err(error) => snapshot.error = Some(error),
                }
            }
            // Closing captures final changes reliably, even when the event ring is full.
            let capture = !was_faulted
                && command == NativeEditorCommand::Close
                && metrics.native_editors.lock().ok().is_some_and(|snapshots| {
                    snapshots[slot].state.supported && snapshots[slot].error.is_none()
                });
            if capture {
                capture_native_slot(slot, slots, events, metrics);
            }
            if let Ok(mut snapshots) = metrics.native_editors.lock() {
                snapshots[slot].capture_in_progress = false;
                snapshots[slot].pending_request = None;
                snapshots[slot].completed_request = request_id;
            }
            false
        }
        AdminCommand::SetSlotConfig { slot, config } => {
            if let Some(runtime) = slots.get_mut(slot) {
                runtime.config = config.normalized();
                true
            } else {
                false
            }
        }
        AdminCommand::SetParameter {
            slot,
            id,
            value,
            request_id,
        } => match call_parameter_backend(
            slot,
            slots,
            events,
            metrics,
            request_id,
            PluginParameterCommand::Set,
            Some(id),
            |backend| backend.set_parameter(id, value),
        ) {
            ParameterCommandOutcome::Success(()) => {
                if request_id != 0 {
                    push_event(
                        events,
                        RuntimeEvent::ParameterSetAck {
                            slot,
                            request_id,
                            id,
                            value,
                        },
                        metrics,
                    );
                }
                true
            }
            ParameterCommandOutcome::Rejected => false,
            ParameterCommandOutcome::Faulted => true,
        },
        AdminCommand::QueryParameter {
            slot,
            id,
            request_id,
        } => {
            match call_parameter_backend(
                slot,
                slots,
                events,
                metrics,
                request_id,
                PluginParameterCommand::Query,
                Some(id),
                |backend| {
                    let value = backend.get_parameter(id)?;
                    if !value.is_finite() {
                        return Err(format!(
                            "plug-in parameter {id} returned a non-finite value"
                        ));
                    }
                    Ok(value.clamp(0.0, 1.0))
                },
            ) {
                ParameterCommandOutcome::Success(value) => push_event(
                    events,
                    RuntimeEvent::ParameterValue {
                        slot,
                        request_id,
                        id,
                        value,
                    },
                    metrics,
                ),
                ParameterCommandOutcome::Rejected => {}
                ParameterCommandOutcome::Faulted => return true,
            }
            false
        }
        AdminCommand::RequestParameterPage {
            slot,
            request_id,
            cursor,
            limit,
        } => handle_parameter_catalog_request(
            slot, request_id, cursor, limit, slots, events, metrics,
        ),
        AdminCommand::SaveState { slot, request_id } => {
            let mut faulted = false;
            if let Some(runtime) = slots.get_mut(slot)
                && runtime.fault.is_none()
                && let Some(backend) = runtime.backend.as_mut()
            {
                runtime.parameter_catalog_cache = None;
                let result = catch_backend(|| {
                    capture_native_backend(
                        slot,
                        backend.as_mut(),
                        &runtime.native_base_ids,
                        metrics,
                    )
                });
                match result {
                    Ok(bytes) => {
                        push_event(
                            events,
                            RuntimeEvent::State {
                                slot,
                                request_id,
                                bytes,
                            },
                            metrics,
                        );
                    }
                    Err(message) => {
                        runtime.fault = Some(message.clone());
                        register_fault(slot, message, events, metrics);
                        faulted = true;
                    }
                }
            }
            faulted
        }
        AdminCommand::LoadState { slot, state } => {
            let faulted = with_backend(slot, slots, events, metrics, |backend| {
                close_native_before_state(backend)?;
                backend.load_state(&state)
            });
            if !faulted
                && metrics.native_editors.lock().ok().is_some_and(|snapshots| {
                    snapshots[slot].native_used || snapshots[slot].dirty_revision != 0
                })
            {
                capture_native_slot(slot, slots, events, metrics);
            }
            faulted || slots.get(slot).is_some_and(|slot| slot.fault.is_none())
        }
        AdminCommand::Shutdown => false,
    }
}

fn close_native_before_state(backend: &mut dyn PluginBackend) -> Result<(), String> {
    if backend.native_editor(NativeEditorCommand::Close)?.open {
        return Err("Native editor remained open; state capture/restore was cancelled".into());
    }
    Ok(())
}

fn publish_native_feedback(slot: usize, feedback: &NativeEditorFeedback, metrics: &BridgeMetrics) {
    if let Ok(mut snapshots) = metrics.native_editors.lock() {
        let snapshot = &mut snapshots[slot];
        if snapshot.state.open && !feedback.state.open {
            snapshot.capture_in_progress = true;
        }
        snapshot.native_used |= feedback.state.open;
        snapshot.state = feedback.state;
        snapshot.dirty_revision = snapshot.dirty_revision.max(feedback.dirty_revision);
    }
}

fn publish_native_capture(
    slot: usize,
    feedback: NativeEditorFeedback,
    bytes: &[u8],
    parameters: Option<Vec<(u32, f32)>>,
    metrics: &BridgeMetrics,
) {
    if let Ok(mut snapshots) = metrics.native_editors.lock() {
        let snapshot = &mut snapshots[slot];
        snapshot.state = feedback.state;
        snapshot.dirty_revision = snapshot.dirty_revision.max(feedback.dirty_revision);
        // Once a native editor has opened, snapshots remain authoritative even if a plugin
        // omits dirty callbacks. Retain them independently of best-effort event delivery.
        if snapshot.native_used || snapshot.dirty_revision != 0 {
            snapshot.captured_dirty_revision = snapshot.dirty_revision;
            snapshot.captured_generation = feedback.state.generation;
            if let Some(parameters) = parameters {
                snapshot.parameter_capture_serial =
                    snapshot.parameter_capture_serial.saturating_add(1);
                snapshot.captured_parameters = Some(Arc::new(parameters));
            }
            snapshot.capture_serial = snapshot.capture_serial.saturating_add(1);
            snapshot.captured_state = Some(Arc::new(bytes.to_vec()));
        }
        snapshot.capture_in_progress = false;
    }
}

fn capture_native_backend(
    slot: usize,
    backend: &mut dyn PluginBackend,
    base_parameter_ids: &[u32],
    metrics: &BridgeMetrics,
) -> Result<Vec<u8>, String> {
    close_native_before_state(backend)?;
    let before = backend.native_editor_feedback()?;
    let refresh_bases = metrics
        .native_editors
        .lock()
        .map_err(|_| "native snapshot lock poisoned".to_owned())?
        .get(slot)
        .is_some_and(|snapshot| {
            snapshot.native_used
                && (snapshot.captured_generation != before.state.generation
                    || snapshot.captured_dirty_revision != before.dirty_revision)
        });
    // Production SaveState flushes pending DSP parameters with zero samples. Read controller
    // values afterward, still inside the same native-revision barrier as the opaque bytes.
    let bytes = backend.save_state()?;
    let parameters = if refresh_bases {
        // VST3 getParamNormalized has no error result: an unknown ID can look like a valid
        // zero. Check membership against current metadata, never the generic UI's cache.
        // Native-only instances need no generic catalog; oversized/invalid catalogs fail
        // explicitly only when saved generic base keys actually require reconciliation.
        let known_ids = if base_parameter_ids.is_empty() {
            HashSet::new()
        } else {
            let (_, catalog) = validate_catalog_snapshot(backend.parameter_catalog_snapshot()?)?;
            catalog.into_iter().map(|parameter| parameter.id).collect()
        };
        let mut values = Vec::with_capacity(base_parameter_ids.len());
        for &id in base_parameter_ids {
            if !known_ids.contains(&id) {
                return Err(format!(
                    "Native parameter base {id} is absent from the current parameter catalog"
                ));
            }
            let value = backend.get_parameter(id).map_err(|error| {
                format!("Native parameter base {id} could not be captured: {error}")
            })?;
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                return Err(format!(
                    "Native parameter base {id} is not a finite normalized value"
                ));
            }
            values.push((id, value));
        }
        Some(values)
    } else {
        None
    };
    let feedback = backend.native_editor_feedback()?;
    if feedback.dirty_revision != before.dirty_revision {
        return Err("Native state changed across the capture barrier".into());
    }
    publish_native_capture(slot, feedback, &bytes, parameters, metrics);
    Ok(bytes)
}

fn capture_native_slot(
    slot: usize,
    slots: &mut [WorkerSlot],
    events: &mut Producer<RuntimeEvent>,
    metrics: &BridgeMetrics,
) {
    let Some(runtime) = slots
        .get_mut(slot)
        .filter(|runtime| runtime.fault.is_none())
    else {
        return;
    };
    runtime.parameter_catalog_cache = None;
    let Some(backend) = runtime.backend.as_mut() else {
        return;
    };
    let result = catch_backend(|| {
        capture_native_backend(slot, backend.as_mut(), &runtime.native_base_ids, metrics)
    });
    if let Err(error) = result {
        if let Ok(mut snapshots) = metrics.native_editors.lock() {
            snapshots[slot].error = Some(error.clone());
            snapshots[slot].capture_in_progress = false;
            // Failure cannot turn potentially unsaved changes into a clean slot.
            snapshots[slot].dirty_revision = snapshots[slot].dirty_revision.saturating_add(1);
        }
        runtime.fault = Some(error.clone());
        register_fault(slot, error, events, metrics);
    }
}

fn poll_native_editors(
    slots: &mut [WorkerSlot],
    events: &mut Producer<RuntimeEvent>,
    metrics: &BridgeMetrics,
) {
    for slot in 0..slots.len() {
        let was_open = metrics
            .native_editors
            .lock()
            .ok()
            .is_some_and(|snapshots| snapshots[slot].state.open);
        let Some(runtime) = slots
            .get_mut(slot)
            .filter(|runtime| runtime.fault.is_none())
        else {
            continue;
        };
        let Some(backend) = runtime.backend.as_mut() else {
            continue;
        };
        match catch_backend(|| backend.native_editor_feedback()) {
            Ok(feedback) => {
                let closed = was_open && !feedback.state.open;
                if feedback.catalog_invalidated {
                    runtime.parameter_catalog_cache = None;
                }
                publish_native_feedback(slot, &feedback, metrics);
                if closed {
                    capture_native_slot(slot, slots, events, metrics);
                }
            }
            Err(error) => {
                if let Ok(mut snapshots) = metrics.native_editors.lock() {
                    snapshots[slot].error = Some(error.clone());
                    snapshots[slot].dirty_revision =
                        snapshots[slot].dirty_revision.saturating_add(1);
                }
                runtime.fault = Some(error.clone());
                register_fault(slot, error, events, metrics);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_parameter_catalog_request(
    slot: usize,
    request_id: u64,
    cursor: u32,
    limit: u8,
    slots: &mut [WorkerSlot],
    events: &mut Producer<RuntimeEvent>,
    metrics: &BridgeMetrics,
) -> bool {
    let Some(runtime) = slots.get_mut(slot) else {
        push_parameter_command_failed(
            events,
            slot,
            request_id,
            PluginParameterCommand::Catalog,
            None,
            "plug-in slot does not exist".into(),
            metrics,
        );
        return false;
    };
    if runtime.fault.is_some() {
        push_parameter_command_failed(
            events,
            slot,
            request_id,
            PluginParameterCommand::Catalog,
            None,
            "plug-in slot is fault-isolated".into(),
            metrics,
        );
        return false;
    }

    if cursor == 0 {
        let Some(backend) = runtime.backend.as_mut() else {
            push_parameter_command_failed(
                events,
                slot,
                request_id,
                PluginParameterCommand::Catalog,
                None,
                "plug-in backend is unavailable".into(),
                metrics,
            );
            return false;
        };
        match catch_parameter_backend(|| backend.parameter_catalog_snapshot()) {
            Ok(snapshot) => match validate_catalog_snapshot(snapshot) {
                Ok((catalog_revision, items)) => {
                    runtime.parameter_catalog_cache = Some(WorkerParameterCatalogCache {
                        request_id,
                        catalog_revision,
                        next_cursor: 0,
                        items,
                    });
                }
                Err(message) => {
                    runtime.parameter_catalog_cache = None;
                    push_parameter_command_failed(
                        events,
                        slot,
                        request_id,
                        PluginParameterCommand::Catalog,
                        None,
                        message,
                        metrics,
                    );
                    return false;
                }
            },
            Err(ParameterBackendError::Rejected(message)) => {
                runtime.parameter_catalog_cache = None;
                push_parameter_command_failed(
                    events,
                    slot,
                    request_id,
                    PluginParameterCommand::Catalog,
                    None,
                    message,
                    metrics,
                );
                return false;
            }
            Err(ParameterBackendError::Panicked(message)) => {
                runtime.parameter_catalog_cache = None;
                runtime.fault = Some(message.clone());
                register_fault(slot, message.clone(), events, metrics);
                push_parameter_command_failed(
                    events,
                    slot,
                    request_id,
                    PluginParameterCommand::Catalog,
                    None,
                    message,
                    metrics,
                );
                return true;
            }
        }
    }

    let Some(cache) = runtime.parameter_catalog_cache.as_ref() else {
        push_parameter_command_failed(
            events,
            slot,
            request_id,
            PluginParameterCommand::Catalog,
            None,
            "parameter catalog continuation has no active snapshot".into(),
            metrics,
        );
        return false;
    };
    if cache.request_id != request_id {
        push_parameter_command_failed(
            events,
            slot,
            request_id,
            PluginParameterCommand::Catalog,
            None,
            "parameter catalog continuation request id does not match the active snapshot".into(),
            metrics,
        );
        return false;
    }
    let cursor_usize = cursor as usize;
    if cache.next_cursor != cursor_usize {
        push_parameter_command_failed(
            events,
            slot,
            request_id,
            PluginParameterCommand::Catalog,
            None,
            format!(
                "parameter catalog cursor {cursor_usize} does not match the snapshot continuation {}",
                cache.next_cursor
            ),
            metrics,
        );
        return false;
    }
    if cursor_usize > cache.items.len() {
        push_parameter_command_failed(
            events,
            slot,
            request_id,
            PluginParameterCommand::Catalog,
            None,
            format!(
                "parameter catalog cursor {cursor_usize} exceeds the snapshot length {}",
                cache.items.len()
            ),
            metrics,
        );
        return false;
    }
    let end = cursor_usize
        .saturating_add(usize::from(limit))
        .min(cache.items.len());
    let done = end == cache.items.len();
    let next_cursor = (!done).then_some(end as u32);
    let catalog_revision = cache.catalog_revision;
    let items = cache.items[cursor_usize..end].to_vec();
    if done {
        runtime.parameter_catalog_cache = None;
    } else if let Some(cache) = runtime.parameter_catalog_cache.as_mut() {
        cache.next_cursor = end;
    }
    push_event(
        events,
        RuntimeEvent::ParameterCatalogPage {
            slot,
            request_id,
            cursor,
            next_cursor,
            done,
            catalog_revision,
            items,
        },
        metrics,
    );
    false
}

enum ParameterCommandOutcome<T> {
    Success(T),
    Rejected,
    Faulted,
}

enum ParameterBackendError {
    Rejected(String),
    Panicked(String),
}

fn catch_parameter_backend<T>(
    operation: impl FnOnce() -> Result<T, String>,
) -> Result<T, ParameterBackendError> {
    match panic::catch_unwind(AssertUnwindSafe(operation)) {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(message)) => Err(ParameterBackendError::Rejected(message)),
        Err(payload) => Err(ParameterBackendError::Panicked(format!(
            "plug-in panicked: {}",
            panic_payload_message(payload.as_ref())
        ))),
    }
}

#[allow(clippy::too_many_arguments)]
fn call_parameter_backend<T>(
    slot: usize,
    slots: &mut [WorkerSlot],
    events: &mut Producer<RuntimeEvent>,
    metrics: &BridgeMetrics,
    request_id: u64,
    command: PluginParameterCommand,
    id: Option<u32>,
    operation: impl FnOnce(&mut dyn PluginBackend) -> Result<T, String>,
) -> ParameterCommandOutcome<T> {
    let Some(runtime) = slots.get_mut(slot) else {
        push_parameter_command_failed(
            events,
            slot,
            request_id,
            command,
            id,
            "plug-in slot does not exist".into(),
            metrics,
        );
        return ParameterCommandOutcome::Rejected;
    };
    if runtime.fault.is_some() {
        push_parameter_command_failed(
            events,
            slot,
            request_id,
            command,
            id,
            "plug-in slot is fault-isolated".into(),
            metrics,
        );
        return ParameterCommandOutcome::Rejected;
    }
    let Some(backend) = runtime.backend.as_mut() else {
        push_parameter_command_failed(
            events,
            slot,
            request_id,
            command,
            id,
            "plug-in backend is unavailable".into(),
            metrics,
        );
        return ParameterCommandOutcome::Rejected;
    };

    match catch_parameter_backend(|| operation(backend.as_mut())) {
        Ok(value) => ParameterCommandOutcome::Success(value),
        Err(ParameterBackendError::Rejected(message)) => {
            push_parameter_command_failed(events, slot, request_id, command, id, message, metrics);
            ParameterCommandOutcome::Rejected
        }
        Err(ParameterBackendError::Panicked(message)) => {
            runtime.fault = Some(message.clone());
            register_fault(slot, message.clone(), events, metrics);
            push_parameter_command_failed(events, slot, request_id, command, id, message, metrics);
            ParameterCommandOutcome::Faulted
        }
    }
}

fn push_parameter_command_failed(
    events: &mut Producer<RuntimeEvent>,
    slot: usize,
    request_id: u64,
    command: PluginParameterCommand,
    id: Option<u32>,
    mut message: String,
    metrics: &BridgeMetrics,
) {
    truncate_utf8(&mut message, MAX_PLUGIN_PARAMETER_COMMAND_ERROR_BYTES);
    push_event(
        events,
        RuntimeEvent::ParameterCommandFailed {
            slot,
            request_id,
            command,
            id,
            message,
        },
        metrics,
    );
}

fn validate_catalog_snapshot(
    mut page: PluginParameterCatalogPage,
) -> Result<(u64, Vec<PluginParameterDescriptor>), String> {
    if page.catalog_revision == 0 {
        return Err("plug-in parameter catalog revision must be nonzero".into());
    }
    if page.total_items > MAX_PLUGIN_PARAMETER_CATALOG_ITEMS {
        return Err(format!(
            "plug-in exposes {} parameters; the generic catalog limit is {MAX_PLUGIN_PARAMETER_CATALOG_ITEMS}",
            page.total_items
        ));
    }
    if page.items.len() != page.total_items {
        return Err(format!(
            "plug-in returned {} snapshot items; expected {}",
            page.items.len(),
            page.total_items
        ));
    }
    let mut ids = HashSet::with_capacity(page.items.len());
    for item in &mut page.items {
        if !ids.insert(item.id) {
            return Err(format!(
                "plug-in returned duplicate parameter id {} in its catalog snapshot",
                item.id
            ));
        }
        bound_parameter_descriptor(item);
    }
    Ok((page.catalog_revision, page.items))
}

fn slice_parameter_catalog_snapshot(
    snapshot: PluginParameterCatalogPage,
    cursor: usize,
    limit: usize,
) -> PluginParameterCatalogPage {
    let end = cursor.saturating_add(limit).min(snapshot.total_items);
    let items = if cursor < snapshot.total_items {
        snapshot.items[cursor..end].to_vec()
    } else {
        Vec::new()
    };
    PluginParameterCatalogPage {
        catalog_revision: snapshot.catalog_revision,
        total_items: snapshot.total_items,
        items,
    }
}

/// Stable FNV-1a fingerprint of the catalog metadata exposed by the generic surface.
/// Live values are deliberately excluded so ordinary automation cannot invalidate pagination.
fn parameter_catalog_revision(items: &[PluginParameterDescriptor]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    fn bytes(hash: &mut u64, value: &[u8]) {
        for byte in value {
            *hash ^= u64::from(*byte);
            *hash = hash.wrapping_mul(PRIME);
        }
    }
    fn u64_value(hash: &mut u64, value: u64) {
        bytes(hash, &value.to_le_bytes());
    }
    fn string(hash: &mut u64, value: &str) {
        u64_value(hash, value.len() as u64);
        bytes(hash, value.as_bytes());
    }

    let mut hash = OFFSET;
    u64_value(&mut hash, items.len() as u64);
    for item in items {
        u64_value(&mut hash, u64::from(item.id));
        string(&mut hash, &item.name);
        string(&mut hash, &item.unit);
        match item.default_normalized {
            Some(value) => {
                bytes(&mut hash, &[1]);
                u64_value(&mut hash, u64::from(value.to_bits()));
            }
            None => bytes(&mut hash, &[0]),
        }
        match item.step_count {
            Some(value) => {
                bytes(&mut hash, &[1]);
                u64_value(&mut hash, u64::from(value));
            }
            None => bytes(&mut hash, &[0]),
        }
        bytes(
            &mut hash,
            &[
                u8::from(item.automatable),
                u8::from(item.read_only),
                u8::from(item.bypass),
            ],
        );
    }
    if hash == 0 { 1 } else { hash }
}

fn bound_parameter_descriptor(descriptor: &mut PluginParameterDescriptor) {
    truncate_utf8(&mut descriptor.name, MAX_PLUGIN_PARAMETER_NAME_BYTES);
    truncate_utf8(&mut descriptor.unit, MAX_PLUGIN_PARAMETER_UNIT_BYTES);
    descriptor.current_normalized = finite_normalized(descriptor.current_normalized, 0.0);
    descriptor.default_normalized = descriptor
        .default_normalized
        .filter(|value| value.is_finite())
        .map(|value| value.clamp(0.0, 1.0));
}

fn finite_normalized(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        fallback.clamp(0.0, 1.0)
    }
}

fn truncate_utf8(value: &mut String, max_bytes: usize) {
    if value.len() <= max_bytes {
        return;
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
}

fn with_backend(
    slot: usize,
    slots: &mut [WorkerSlot],
    events: &mut Producer<RuntimeEvent>,
    metrics: &BridgeMetrics,
    operation: impl FnOnce(&mut dyn PluginBackend) -> Result<(), String>,
) -> bool {
    let Some(runtime) = slots.get_mut(slot) else {
        return false;
    };
    if runtime.fault.is_some() {
        return false;
    }
    let Some(backend) = runtime.backend.as_mut() else {
        return false;
    };
    if let Err(message) = catch_backend(|| operation(backend.as_mut())) {
        runtime.fault = Some(message.clone());
        register_fault(slot, message, events, metrics);
        true
    } else {
        false
    }
}

fn register_fault(
    slot: usize,
    message: String,
    events: &mut Producer<RuntimeEvent>,
    metrics: &BridgeMetrics,
) {
    metrics.faults.fetch_add(1, Ordering::Relaxed);
    push_event(events, RuntimeEvent::SlotFault { slot, message }, metrics);
}

fn push_event(events: &mut Producer<RuntimeEvent>, event: RuntimeEvent, metrics: &BridgeMetrics) {
    if let Err(PushError::Full(_)) = events.push(event) {
        metrics.event_overflows.fetch_add(1, Ordering::Relaxed);
    }
}

fn update_latency_and_tail(
    slots: &mut [WorkerSlot],
    events: &mut Producer<RuntimeEvent>,
    metrics: &BridgeMetrics,
) {
    let mut latency = 0u64;
    let mut tail = 0u32;
    let mut active_mask = 0_u16;
    let mut midi_capabilities = 0_u32;
    let mut slot_latencies = [0_u32; MAX_PLUGIN_CHAIN_SLOTS];
    for (slot_index, slot) in slots.iter_mut().enumerate() {
        if !slot.config.enabled || slot.config.bypassed || slot.fault.is_some() {
            continue;
        }
        if let Some(backend) = slot.backend.as_ref() {
            match backend_metadata(backend.as_ref()) {
                Ok((plugin_latency, plugin_tail)) => {
                    debug_assert!(slot_index < MAX_PLUGIN_CHAIN_SLOTS);
                    active_mask |= 1_u16 << slot_index;
                    let (input, output) = backend.midi_capabilities();
                    midi_capabilities |= u32::from(input) << slot_index;
                    midi_capabilities |= u32::from(output) << (slot_index + 16);
                    slot_latencies[slot_index] = plugin_latency;
                    latency = latency.saturating_add(u64::from(plugin_latency));
                    // Serial tails can accumulate: an upstream reverb tail still has to traverse
                    // every downstream effect. Saturating sum is the safe render/stop upper bound.
                    tail = tail.saturating_add(plugin_tail);
                }
                Err(message) => {
                    slot.fault = Some(message.clone());
                    register_fault(slot_index, message, events, metrics);
                }
            }
        }
    }
    metrics
        .midi_capabilities
        .store(midi_capabilities, Ordering::Release);
    publish_plugin_latency_snapshot(
        metrics,
        active_mask,
        slot_latencies,
        latency.min(u64::from(u32::MAX)) as u32,
        tail,
    );
}

fn publish_plugin_latency_snapshot(
    metrics: &BridgeMetrics,
    active_mask: u16,
    slot_latencies: [u32; MAX_PLUGIN_CHAIN_SLOTS],
    total_latency: u32,
    tail: u32,
) {
    let current_revision = metrics.plugin_latency_revision.load(Ordering::Relaxed);
    let metadata_changed = current_revision == 0
        || metrics.plugin_active_mask.load(Ordering::Relaxed) as u16 != active_mask
        || metrics.plugin_latency_samples.load(Ordering::Relaxed) != total_latency
        || metrics.tail_samples.load(Ordering::Relaxed) != tail
        || slot_latencies.iter().enumerate().any(|(slot, latency)| {
            metrics.plugin_slot_latency_samples[slot].load(Ordering::Relaxed) != *latency
        });
    if !metadata_changed {
        return;
    }

    // Exactly one worker publishes this tuple. Acquire on the even-to-odd RMW plus the following
    // Release fence keeps payload stores behind the odd marker on weak-memory targets. The final
    // Release store publishes the complete tuple; readers validate it with a matching protocol.
    let sequence = metrics
        .plugin_latency_sequence
        .fetch_add(1, Ordering::Acquire);
    debug_assert_eq!(sequence & 1, 0);
    atomic::fence(Ordering::Release);
    metrics
        .plugin_active_mask
        .store(u32::from(active_mask), Ordering::Relaxed);
    for (slot, latency) in slot_latencies.into_iter().enumerate() {
        metrics.plugin_slot_latency_samples[slot].store(latency, Ordering::Relaxed);
    }
    metrics
        .plugin_latency_samples
        .store(total_latency, Ordering::Relaxed);
    metrics.tail_samples.store(tail, Ordering::Relaxed);
    let revision = next_nonzero_epoch(current_revision);
    metrics
        .plugin_latency_revision
        .store(revision, Ordering::Relaxed);
    metrics
        .plugin_latency_sequence
        .store(sequence.wrapping_add(2), Ordering::Release);
}

fn load_backend(
    spec: PluginLoadSpec,
    config: PluginPrepareConfig,
) -> Result<Box<dyn PluginBackend>, String> {
    match spec.descriptor.format {
        PluginFormat::Vst2 => load_vst2_backend(spec, config),
        PluginFormat::Vst3 => load_vst3_backend(spec, config),
    }
}

#[cfg(feature = "vst2")]
fn load_vst2_backend(
    spec: PluginLoadSpec,
    config: PluginPrepareConfig,
) -> Result<Box<dyn PluginBackend>, String> {
    Ok(Box::new(Vst2Backend::load(spec, config)?))
}

#[cfg(not(feature = "vst2"))]
fn load_vst2_backend(
    _spec: PluginLoadSpec,
    _config: PluginPrepareConfig,
) -> Result<Box<dyn PluginBackend>, String> {
    Err("this build does not include VST2 hosting".into())
}

#[cfg(feature = "vst3")]
fn load_vst3_backend(
    spec: PluginLoadSpec,
    config: PluginPrepareConfig,
) -> Result<Box<dyn PluginBackend>, String> {
    Ok(Box::new(Vst3Backend::load(spec, config)?))
}

#[cfg(not(feature = "vst3"))]
fn load_vst3_backend(
    _spec: PluginLoadSpec,
    _config: PluginPrepareConfig,
) -> Result<Box<dyn PluginBackend>, String> {
    Err("this build does not include VST3 hosting".into())
}

#[cfg(feature = "vst2")]
#[allow(deprecated)]
struct Vst2Backend {
    _loader: vst::host::PluginLoader<Vst2Host>,
    instance: vst::host::PluginInstance,
    info: vst::plugin::Info,
    name: String,
    host_buffer: vst::host::HostBuffer<f32>,
    inputs: Vec<Vec<f32>>,
    outputs: Vec<Vec<f32>>,
    midi: vst::buffer::SendEventBuffer,
    pending_state: Vec<u8>,
    prepared: Option<PluginPrepareConfig>,
}

#[cfg(feature = "vst2")]
#[allow(deprecated)]
#[derive(Default)]
struct Vst2Host;

#[cfg(feature = "vst2")]
#[allow(deprecated)]
impl vst::host::Host for Vst2Host {}

#[cfg(feature = "vst2")]
#[allow(deprecated)]
impl Vst2Backend {
    const MAX_IO_CHANNELS: usize = 64;
    const STATE_MAGIC: &'static [u8; 8] = b"C2V2ST\0\x01";

    fn load(spec: PluginLoadSpec, config: PluginPrepareConfig) -> Result<Self, String> {
        use std::sync::Mutex;
        use vst::{host::PluginLoader, plugin::Plugin};

        if !spec.descriptor.path.is_file() {
            return Err(format!(
                "VST2 plug-in does not exist: '{}'",
                spec.descriptor.path.display()
            ));
        }
        let host = Arc::new(Mutex::new(Vst2Host));
        let mut loader = PluginLoader::load(&spec.descriptor.path, host)
            .map_err(|error| format!("unable to load VST2 module: {error}"))?;
        let mut instance = loader
            .instance()
            .map_err(|error| format!("unable to create VST2 instance: {error}"))?;
        instance.init();
        let info = instance.get_info();
        let inputs = usize::try_from(info.inputs.max(0)).unwrap_or(0);
        let outputs = usize::try_from(info.outputs.max(0)).unwrap_or(0);
        if inputs > Self::MAX_IO_CHANNELS || outputs > Self::MAX_IO_CHANNELS {
            return Err(format!(
                "VST2 plug-in requests unsupported I/O ({} inputs, {} outputs; maximum {})",
                inputs,
                outputs,
                Self::MAX_IO_CHANNELS
            ));
        }
        let parameter_count = usize::try_from(info.parameters.max(0)).unwrap_or(0);
        if parameter_count > u16::MAX as usize {
            return Err(format!(
                "VST2 plug-in reports too many parameters ({parameter_count})"
            ));
        }
        let name = if info.vendor.trim().is_empty() {
            info.name.clone()
        } else {
            format!("{} — {}", info.name, info.vendor)
        };
        let channel = || vec![0.0; config.max_block_frames];
        Ok(Self {
            _loader: loader,
            instance,
            info,
            name,
            host_buffer: vst::host::HostBuffer::new(inputs, outputs),
            inputs: (0..inputs).map(|_| channel()).collect(),
            outputs: (0..outputs).map(|_| channel()).collect(),
            midi: vst::buffer::SendEventBuffer::new(256),
            pending_state: spec.initial_state,
            prepared: None,
        })
    }

    fn encode_parameter_state(&mut self) -> Vec<u8> {
        use vst::plugin::Plugin;
        let parameters = self.instance.get_parameter_object();
        let count = self.info.parameters.max(0) as u32;
        let mut state = Vec::with_capacity(Self::STATE_MAGIC.len() + 1 + 4 + count as usize * 4);
        state.extend_from_slice(Self::STATE_MAGIC);
        state.push(0);
        state.extend_from_slice(&count.to_le_bytes());
        for id in 0..count {
            state.extend_from_slice(&parameters.get_parameter(id as i32).to_le_bytes());
        }
        state
    }

    fn encode_chunk_state(&self, chunk: Vec<u8>) -> Vec<u8> {
        let mut state = Vec::with_capacity(Self::STATE_MAGIC.len() + 1 + chunk.len());
        state.extend_from_slice(Self::STATE_MAGIC);
        state.push(1);
        state.extend_from_slice(&chunk);
        state
    }

    fn restore_encoded_state(&mut self, state: &[u8]) -> Result<(), String> {
        use vst::plugin::Plugin;
        let parameters = self.instance.get_parameter_object();
        let Some(payload) = state.strip_prefix(Self::STATE_MAGIC) else {
            if self.info.preset_chunks {
                parameters.load_bank_data(state);
                return Ok(());
            }
            return Err("unrecognized VST2 state blob".into());
        };
        let Some((&kind, payload)) = payload.split_first() else {
            return Err("truncated VST2 state blob".into());
        };
        match kind {
            0 => {
                let Some(count_bytes) = payload.get(..4) else {
                    return Err("truncated VST2 parameter state".into());
                };
                let count = u32::from_le_bytes(count_bytes.try_into().expect("four bytes"));
                let values = &payload[4..];
                if values.len() != count as usize * 4 {
                    return Err("invalid VST2 parameter state length".into());
                }
                let available = self.info.parameters.max(0) as u32;
                for (id, bytes) in values.as_chunks::<4>().0.iter().enumerate() {
                    if id as u32 >= available {
                        break;
                    }
                    let value = f32::from_le_bytes(*bytes);
                    if value.is_finite() {
                        parameters.set_parameter(id as i32, value.clamp(0.0, 1.0));
                    }
                }
                Ok(())
            }
            1 => {
                parameters.load_bank_data(payload);
                Ok(())
            }
            _ => Err(format!("unsupported VST2 state encoding {kind}")),
        }
    }
}

#[cfg(feature = "vst2")]
#[allow(deprecated)]
impl PluginBackend for Vst2Backend {
    fn name(&self) -> &str {
        &self.name
    }

    fn prepare(&mut self, config: PluginPrepareConfig) -> Result<(), String> {
        use vst::plugin::Plugin;
        if config.max_block_frames
            > self.inputs.first().map_or(
                self.outputs
                    .first()
                    .map_or(MAX_PLUGIN_BLOCK_FRAMES, Vec::capacity),
                Vec::capacity,
            )
        {
            return Err("VST2 block size exceeds preallocated storage".into());
        }
        if self.prepared.is_some() {
            self.instance.suspend();
        }
        self.instance.set_sample_rate(config.sample_rate as f32);
        self.instance.set_block_size(config.max_block_frames as i64);
        if !self.pending_state.is_empty() {
            let state = std::mem::take(&mut self.pending_state);
            self.restore_encoded_state(&state)?;
        }
        self.instance.resume();
        self.prepared = Some(config);
        Ok(())
    }

    fn process(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        frames: usize,
    ) -> Result<(), String> {
        use vst::plugin::Plugin;
        if self.prepared.is_none() || frames > left.len() || frames > right.len() {
            return Err("invalid VST2 process block".into());
        }
        for channel in &mut self.inputs {
            channel.resize(frames, 0.0);
            channel.fill(0.0);
        }
        match self.inputs.len() {
            0 => {}
            1 => {
                for frame in 0..frames {
                    self.inputs[0][frame] = (left[frame] + right[frame]) * 0.5;
                }
            }
            _ => {
                self.inputs[0][..frames].copy_from_slice(&left[..frames]);
                self.inputs[1][..frames].copy_from_slice(&right[..frames]);
            }
        }
        for channel in &mut self.outputs {
            channel.resize(frames, 0.0);
            channel.fill(0.0);
        }
        let mut buffer = self.host_buffer.bind(&self.inputs, &mut self.outputs);
        self.instance.process(&mut buffer);
        match self.outputs.len() {
            0 => {
                left[..frames].fill(0.0);
                right[..frames].fill(0.0);
            }
            1 => {
                left[..frames].copy_from_slice(&self.outputs[0][..frames]);
                right[..frames].copy_from_slice(&self.outputs[0][..frames]);
            }
            _ => {
                left[..frames].copy_from_slice(&self.outputs[0][..frames]);
                right[..frames].copy_from_slice(&self.outputs[1][..frames]);
            }
        }
        Ok(())
    }

    fn send_midi(&mut self, message: MidiMessage) -> Result<(), String> {
        use vst::{event::MidiEvent, plugin::Plugin};
        let event = MidiEvent {
            data: message.data,
            delta_frames: i32::from(message.sample_offset),
            live: false,
            note_length: None,
            note_offset: None,
            detune: 0,
            note_off_velocity: 0,
        };
        self.midi.store_events(std::iter::once(event));
        self.instance.process_events(self.midi.events());
        Ok(())
    }

    fn set_parameter(&mut self, id: u32, normalized: f32) -> Result<(), String> {
        if id >= self.info.parameters.max(0) as u32 {
            return Err(format!("VST2 parameter {id} does not exist"));
        }
        use vst::plugin::Plugin;
        self.instance
            .get_parameter_object()
            .set_parameter(id as i32, normalized.clamp(0.0, 1.0));
        Ok(())
    }

    fn get_parameter(&mut self, id: u32) -> Result<f32, String> {
        if id >= self.info.parameters.max(0) as u32 {
            return Err(format!("VST2 parameter {id} does not exist"));
        }
        use vst::plugin::Plugin;
        Ok(self
            .instance
            .get_parameter_object()
            .get_parameter(id as i32))
    }

    fn parameter_catalog_snapshot(&mut self) -> Result<PluginParameterCatalogPage, String> {
        let total_items = usize::try_from(self.info.parameters.max(0)).unwrap_or(0);
        if total_items > MAX_PLUGIN_PARAMETER_CATALOG_ITEMS {
            return Err(format!(
                "VST2 plug-in exposes {total_items} parameters; the generic catalog limit is {MAX_PLUGIN_PARAMETER_CATALOG_ITEMS}"
            ));
        }
        use vst::plugin::Plugin;
        let parameters = self.instance.get_parameter_object();
        let mut catalog = Vec::with_capacity(total_items);
        for id in 0..total_items {
            let id_i32 = id as i32;
            catalog.push(PluginParameterDescriptor {
                id: id as u32,
                name: parameters.get_parameter_name(id_i32),
                unit: parameters.get_parameter_label(id_i32),
                current_normalized: parameters.get_parameter(id_i32),
                default_normalized: None,
                step_count: None,
                automatable: parameters.can_be_automated(id_i32),
                read_only: false,
                bypass: false,
            });
        }
        for descriptor in &mut catalog {
            bound_parameter_descriptor(descriptor);
        }
        let catalog_revision = parameter_catalog_revision(&catalog);
        Ok(PluginParameterCatalogPage {
            catalog_revision,
            total_items,
            items: catalog,
        })
    }

    fn parameter_catalog_page(
        &mut self,
        cursor: usize,
        limit: usize,
    ) -> Result<PluginParameterCatalogPage, String> {
        Ok(slice_parameter_catalog_snapshot(
            self.parameter_catalog_snapshot()?,
            cursor,
            limit,
        ))
    }

    fn save_state(&mut self) -> Result<Vec<u8>, String> {
        Ok(if self.info.preset_chunks {
            use vst::plugin::Plugin;
            let chunk = self.instance.get_parameter_object().get_bank_data();
            self.encode_chunk_state(chunk)
        } else {
            self.encode_parameter_state()
        })
    }

    fn load_state(&mut self, state: &[u8]) -> Result<(), String> {
        self.restore_encoded_state(state)
    }

    fn reset_processing(&mut self) -> Result<(), String> {
        use vst::plugin::Plugin;
        if self.prepared.is_some() {
            // VST2 has no universal tail-reset opcode. The public mains-off/mains-on lifecycle is
            // the strongest portable reset boundary and stays on this isolated worker thread.
            self.instance.suspend();
            self.instance.resume();
        }
        Ok(())
    }

    fn latency_samples(&self) -> u32 {
        self.info.initial_delay.max(0) as u32
    }

    fn tail_samples(&self) -> u32 {
        use vst::plugin::Plugin;
        match self.instance.get_tail_size() {
            value if value < 0 => u32::MAX,
            value if value <= 1 => 0,
            value => u32::try_from(value).unwrap_or(u32::MAX),
        }
    }
}

#[cfg(feature = "vst2")]
#[allow(deprecated)]
impl Drop for Vst2Backend {
    fn drop(&mut self) {
        use vst::plugin::Plugin;
        self.instance.suspend();
    }
}

#[cfg(feature = "vst3")]
struct Vst3Backend {
    last_transport: PluginTransport,
    native_revision_base: u64,
    plugin: vst3_host::Plugin,
    buffers: vst3_host::AudioBuffers,
    name: String,
    pending_state: Vec<u8>,
    prepared: bool,
    max_block_frames: usize,
    input_channels: usize,
    output_channels: usize,
}

#[cfg(feature = "vst3")]
impl Vst3Backend {
    fn load(spec: PluginLoadSpec, config: PluginPrepareConfig) -> Result<Self, String> {
        let helper_path = match spec.vst3_helper_path {
            Some(path) if path.is_file() => path,
            Some(path) => {
                return Err(format!(
                    "VST3 isolation helper does not exist: '{}'",
                    path.display()
                ));
            }
            None => installed_vst3_helper_path()?,
        };
        let mut host = vst3_host::Vst3Host::builder()
            .sample_rate(config.sample_rate)
            .block_size(config.max_block_frames)
            .with_process_isolation(true)
            .auto_recover_plugins(false)
            .helper_path(&helper_path)
            .build()
            .map_err(|error| {
                format!(
                    "unable to initialize isolated VST3 host with '{}': {error}",
                    helper_path.display()
                )
            })?;
        let plugin = match spec.class_uid.as_deref() {
            Some(uid) => host.load_plugin_class(&spec.descriptor.path, uid),
            None => host.load_plugin(&spec.descriptor.path),
        }
        .map_err(|error| {
            format!(
                "unable to load VST3 plug-in '{}' through '{}': {error}",
                spec.descriptor.path.display(),
                helper_path.display()
            )
        })?;
        let layout = plugin.audio_bus_layout().ok();
        let input_channels = layout.as_ref().map_or_else(
            || usize::from(plugin.info().audio_inputs != 0) * 2,
            |layout| {
                layout
                    .inputs
                    .iter()
                    .filter(|bus| bus.active)
                    .map(|bus| bus.channel_count)
                    .sum()
            },
        );
        let output_channels = layout.as_ref().map_or(2, |layout| {
            layout
                .outputs
                .iter()
                .filter(|bus| bus.active)
                .map(|bus| bus.channel_count)
                .sum()
        });
        let name = if plugin.info().vendor.trim().is_empty() {
            plugin.info().name.clone()
        } else {
            format!("{} — {}", plugin.info().name, plugin.info().vendor)
        };
        Ok(Self {
            last_transport: PluginTransport::default(),
            native_revision_base: 0,
            plugin,
            buffers: vst3_host::AudioBuffers::new(
                2,
                2,
                config.max_block_frames,
                config.sample_rate,
            ),
            name,
            pending_state: spec.initial_state,
            prepared: false,
            max_block_frames: config.max_block_frames,
            input_channels,
            output_channels,
        })
    }
}

#[cfg(feature = "vst3")]
impl PluginBackend for Vst3Backend {
    fn name(&self) -> &str {
        &self.name
    }

    fn prepare(&mut self, config: PluginPrepareConfig) -> Result<(), String> {
        if self.prepared {
            self.plugin
                .stop_processing()
                .map_err(|error| error.to_string())?;
        }
        if self.plugin.sample_rate() != config.sample_rate
            || self.plugin.block_size() != config.max_block_frames
        {
            self.plugin
                .reconfigure(config.sample_rate, config.max_block_frames)
                .map_err(|error| error.to_string())?;
        }
        if !self.pending_state.is_empty() {
            let state = std::mem::take(&mut self.pending_state);
            self.plugin
                .load_state(&state)
                .map_err(|error| error.to_string())?;
        }
        self.plugin
            .start_processing()
            .map_err(|error| error.to_string())?;
        self.native_revision_base = self
            .plugin
            .native_dirty_revision()
            .map_err(|error| error.to_string())?;
        self.prepared = true;
        Ok(())
    }

    fn process(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        frames: usize,
    ) -> Result<(), String> {
        if !self.prepared || frames > self.max_block_frames {
            return Err("invalid VST3 process block".into());
        }
        for channel in &mut self.buffers.inputs {
            channel.resize(frames, 0.0);
            channel.fill(0.0);
        }
        for channel in &mut self.buffers.outputs {
            channel.resize(frames, 0.0);
            channel.fill(0.0);
        }
        if self.input_channels == 1 {
            for frame in 0..frames {
                self.buffers.inputs[0][frame] = (left[frame] + right[frame]) * 0.5;
            }
        } else if self.input_channels > 1 {
            self.buffers.inputs[0][..frames].copy_from_slice(&left[..frames]);
            self.buffers.inputs[1][..frames].copy_from_slice(&right[..frames]);
        }
        self.buffers.block_size = frames;
        self.plugin
            .process_audio(&mut self.buffers)
            .map_err(|error| error.to_string())?;
        match self.output_channels {
            0 => {
                left[..frames].fill(0.0);
                right[..frames].fill(0.0);
            }
            1 => {
                left[..frames].copy_from_slice(&self.buffers.outputs[0][..frames]);
                right[..frames].copy_from_slice(&self.buffers.outputs[0][..frames]);
            }
            _ => {
                left[..frames].copy_from_slice(&self.buffers.outputs[0][..frames]);
                right[..frames].copy_from_slice(&self.buffers.outputs[1][..frames]);
            }
        }
        Ok(())
    }

    fn send_midi(&mut self, message: MidiMessage) -> Result<(), String> {
        use vst3_host::{MidiChannel, MidiEvent};
        let status = message.data[0];
        let channel = MidiChannel::from_index(status & 0x0f)
            .ok_or_else(|| "invalid MIDI channel".to_owned())?;
        let data1 = message.data[1] & 0x7f;
        let data2 = message.data[2] & 0x7f;
        let event = match status & 0xf0 {
            0x80 => MidiEvent::NoteOff {
                channel,
                note: data1,
                velocity: data2,
            },
            0x90 if data2 == 0 => MidiEvent::NoteOff {
                channel,
                note: data1,
                velocity: 0,
            },
            0x90 => MidiEvent::NoteOn {
                channel,
                note: data1,
                velocity: data2,
            },
            0xa0 => MidiEvent::PolyAftertouch {
                channel,
                note: data1,
                pressure: data2,
            },
            0xb0 => MidiEvent::ControlChange {
                channel,
                controller: data1,
                value: data2,
            },
            0xc0 => MidiEvent::ProgramChange {
                channel,
                program: data1,
            },
            0xd0 => MidiEvent::ChannelAftertouch {
                channel,
                pressure: data1,
            },
            0xe0 => MidiEvent::PitchBend {
                channel,
                value: u16::from(data1) | (u16::from(data2) << 7),
            },
            other => return Err(format!("unsupported MIDI status 0x{other:02x}")),
        };
        self.plugin
            .send_midi_event_at(event, i32::from(message.sample_offset))
            .map_err(|error| error.to_string())
    }

    fn midi_capabilities(&self) -> (bool, bool) {
        (
            self.plugin.info().has_midi_input,
            self.plugin.info().has_midi_output,
        )
    }

    fn set_transport(&mut self, transport: PluginTransport) -> Result<(), String> {
        self.last_transport = transport;
        self.plugin
            .set_process_transport(vst3_host::ProcessTransport {
                sample_position: transport.sample_position,
                quarter_note_position: transport.quarter_note_position,
                tempo: transport.tempo,
                playing: transport.playing,
                time_sig_numerator: transport.time_sig_numerator,
                time_sig_denominator: transport.time_sig_denominator,
            })
            .map_err(|error| error.to_string())
    }

    fn drain_midi_output(&mut self, batch: &mut PluginMidiBatch, slot: u8, frames: usize) {
        let (events, lost) = self.plugin.take_output_events_with_loss();
        batch.lost |= lost;
        for event in events {
            if event.bus_index != 0
                || event.sample_offset < 0
                || event.sample_offset as usize >= frames
            {
                batch.lost = true;
                continue;
            }
            // The first slice intentionally narrows to MIDI1. Rich per-note IDs, tuning,
            // SysEx and expression are not silently presented as lossless MPE/MIDI2.
            let compatible = match &event.data {
                vst3_host::PluginEventData::NoteOn {
                    velocity, tuning, ..
                }
                | vst3_host::PluginEventData::NoteOff {
                    velocity, tuning, ..
                } => velocity.is_finite() && *tuning == 0.0,
                vst3_host::PluginEventData::PolyPressure { pressure, .. } => pressure.is_finite(),
                vst3_host::PluginEventData::LegacyMidiCcOut { .. } => true,
                _ => false,
            };
            let Some(midi) = event.to_midi().filter(|_| compatible) else {
                batch.lost = true;
                continue;
            };
            use vst3_host::MidiEvent;
            let data = match midi {
                MidiEvent::NoteOn {
                    channel,
                    note,
                    velocity,
                } => [
                    if velocity == 0 { 0x80 } else { 0x90 } | channel.as_index(),
                    note,
                    velocity,
                ],
                MidiEvent::NoteOff {
                    channel,
                    note,
                    velocity,
                } => [0x80 | channel.as_index(), note, velocity],
                MidiEvent::PolyAftertouch {
                    channel,
                    note,
                    pressure,
                } => [0xa0 | channel.as_index(), note, pressure],
                MidiEvent::ControlChange {
                    channel,
                    controller,
                    value,
                } => [0xb0 | channel.as_index(), controller, value],
                MidiEvent::ProgramChange { channel, program } => {
                    [0xc0 | channel.as_index(), program, 0]
                }
                MidiEvent::ChannelAftertouch { channel, pressure } => {
                    [0xd0 | channel.as_index(), pressure, 0]
                }
                MidiEvent::PitchBend { channel, value } => [
                    0xe0 | channel.as_index(),
                    (value & 127) as u8,
                    ((value >> 7) & 127) as u8,
                ],
                _ => {
                    batch.lost = true;
                    continue;
                }
            };
            batch.push(slot, MidiMessage::new(data, event.sample_offset as usize));
        }
    }

    fn set_parameter(&mut self, id: u32, normalized: f32) -> Result<(), String> {
        self.plugin
            .set_parameter(id, f64::from(normalized.clamp(0.0, 1.0)))
            .map_err(|error| error.to_string())
    }

    fn get_parameter(&mut self, id: u32) -> Result<f32, String> {
        self.plugin
            .get_parameter(id)
            .map(|value| value as f32)
            .map_err(|error| error.to_string())
    }

    fn parameter_catalog_snapshot(&mut self) -> Result<PluginParameterCatalogPage, String> {
        let parameters = self
            .plugin
            .get_parameters()
            .map_err(|error| error.to_string())?;
        let total_items = parameters.len();
        if total_items > MAX_PLUGIN_PARAMETER_CATALOG_ITEMS {
            return Err(format!(
                "VST3 plug-in exposes {total_items} parameters; the generic catalog limit is {MAX_PLUGIN_PARAMETER_CATALOG_ITEMS}"
            ));
        }
        let mut catalog: Vec<_> = parameters
            .iter()
            .map(|parameter| PluginParameterDescriptor {
                id: parameter.id,
                name: parameter.name.clone(),
                unit: parameter.unit.clone(),
                current_normalized: parameter.value as f32,
                default_normalized: Some(parameter.default as f32),
                step_count: u32::try_from(parameter.step_count).ok(),
                automatable: parameter.can_automate,
                read_only: parameter.is_read_only,
                bypass: parameter.is_bypass,
            })
            .collect();
        for descriptor in &mut catalog {
            bound_parameter_descriptor(descriptor);
        }
        let catalog_revision = parameter_catalog_revision(&catalog);
        Ok(PluginParameterCatalogPage {
            catalog_revision,
            total_items,
            items: catalog,
        })
    }

    fn parameter_catalog_page(
        &mut self,
        cursor: usize,
        limit: usize,
    ) -> Result<PluginParameterCatalogPage, String> {
        Ok(slice_parameter_catalog_snapshot(
            self.parameter_catalog_snapshot()?,
            cursor,
            limit,
        ))
    }

    fn native_editor(&mut self, command: NativeEditorCommand) -> Result<NativeEditorState, String> {
        use vst3_host::{IsolatedEditorCommand as Command, IsolatedEditorOwner};
        let command = match command {
            NativeEditorCommand::Open { owner } => Command::Open {
                owner: owner.map(|(window, process_id)| IsolatedEditorOwner { window, process_id }),
            },
            NativeEditorCommand::Focus => Command::Focus,
            NativeEditorCommand::Close => Command::Close,
        };
        self.plugin
            .isolated_editor(command)
            .map(native_editor_state)
            .map_err(|error| error.to_string())
    }

    fn native_editor_feedback(&mut self) -> Result<NativeEditorFeedback, String> {
        let state = self
            .plugin
            .isolated_editor(vst3_host::IsolatedEditorCommand::Query)
            .map(native_editor_state)
            .map_err(|error| error.to_string())?;
        let edits = self
            .plugin
            .try_take_parameter_edits()
            .map_err(|error| error.to_string())?;
        let notifications = self
            .plugin
            .try_take_host_notifications()
            .map_err(|error| error.to_string())?;
        // Service lifecycle-sensitive restart requests on the helper's main thread. Returned
        // flags refresh host-side caches; failures never masquerade as an empty feedback batch.
        let flags = self
            .plugin
            .service_host_requests()
            .map_err(|error| error.to_string())?;
        let revision = self
            .plugin
            .native_dirty_revision()
            .map_err(|error| error.to_string())?;
        let dirty_revision = revision
            .checked_sub(self.native_revision_base)
            .ok_or_else(|| {
                "VST3 native revision reset; helper state may have been lost".to_owned()
            })?;
        if flags.reload_component() {
            return Err(
                "VST3 requested component reload; native state must be recovered before reloading"
                    .into(),
            );
        }
        if flags.io_changed() {
            let layout = self
                .plugin
                .audio_bus_layout()
                .map_err(|error| error.to_string())?;
            self.input_channels = layout
                .inputs
                .iter()
                .filter(|bus| bus.active)
                .map(|bus| bus.channel_count)
                .sum();
            self.output_channels = layout
                .outputs
                .iter()
                .filter(|bus| bus.active)
                .map(|bus| bus.channel_count)
                .sum();
        }
        let catalog_invalidated = !edits.is_empty()
            || flags.param_values_changed()
            || flags.param_titles_changed()
            || notifications
                .iter()
                .any(|item| matches!(item, vst3_host::HostNotification::DirtyChanged(true)));
        Ok(NativeEditorFeedback {
            state,
            dirty_revision,
            catalog_invalidated,
        })
    }

    fn save_state(&mut self) -> Result<Vec<u8>, String> {
        self.plugin.save_state().map_err(|error| error.to_string())
    }

    fn load_state(&mut self, state: &[u8]) -> Result<(), String> {
        self.plugin
            .load_state(state)
            .map_err(|error| error.to_string())
    }

    fn reset_processing(&mut self) -> Result<(), String> {
        if !self.prepared {
            return Ok(());
        }
        // CC mapping is optional in VST3. Queue native NoteOff for tracked ordinary notes
        // and consume it on this worker even when no more callback blocks will arrive.
        let stopped = PluginTransport {
            playing: false,
            ..self.last_transport
        };
        self.set_transport(stopped)?;
        self.plugin
            .midi_panic()
            .map_err(|error| error.to_string())?;
        self.process(&mut [0.0], &mut [0.0], 1)?;
        let _ = self.plugin.take_output_events_with_loss();
        self.plugin
            .stop_processing()
            .map_err(|error| error.to_string())?;
        self.plugin
            .start_processing()
            .map_err(|error| error.to_string())
    }

    fn latency_samples(&self) -> u32 {
        self.plugin.latency_samples()
    }

    fn tail_samples(&self) -> u32 {
        self.plugin.tail_samples()
    }
}

#[cfg(feature = "vst3")]
fn native_editor_state(state: vst3_host::IsolatedEditorState) -> NativeEditorState {
    NativeEditorState {
        supported: state.supported,
        has_editor: state.has_editor,
        open: state.open,
        width: state.width,
        height: state.height,
        generation: state.generation,
    }
}

#[cfg(feature = "vst3")]
impl Drop for Vst3Backend {
    fn drop(&mut self) {
        let _ = self
            .plugin
            .isolated_editor(vst3_host::IsolatedEditorCommand::Close);
        if self.prepared {
            let _ = self.plugin.stop_processing();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct NativeMock {
        open: bool,
        revision: u64,
        bytes: Vec<u8>,
        calls: Vec<&'static str>,
        fail_feedback: bool,
        reject_open: bool,
        parameter_value: f32,
        query_error: bool,
        missing_parameter: bool,
        edit_during_capture: bool,
    }

    struct NativeMockBackend(Arc<Mutex<NativeMock>>);
    impl PluginBackend for NativeMockBackend {
        fn name(&self) -> &str {
            "native fixture"
        }
        fn prepare(&mut self, _: PluginPrepareConfig) -> Result<(), String> {
            Ok(())
        }
        fn process(&mut self, _: &mut [f32], _: &mut [f32], _: usize) -> Result<(), String> {
            Ok(())
        }
        fn send_midi(&mut self, _: MidiMessage) -> Result<(), String> {
            Ok(())
        }
        fn set_parameter(&mut self, _: u32, _: f32) -> Result<(), String> {
            Ok(())
        }
        fn get_parameter(&mut self, _: u32) -> Result<f32, String> {
            let mut mock = self.0.lock().unwrap();
            mock.calls.push("parameter");
            if mock.query_error {
                Err("parameter disappeared".into())
            } else {
                Ok(mock.parameter_value)
            }
        }
        fn parameter_catalog_snapshot(&mut self) -> Result<PluginParameterCatalogPage, String> {
            let mut mock = self.0.lock().unwrap();
            mock.calls.push("catalog");
            let items = if mock.missing_parameter {
                Vec::new()
            } else {
                vec![parameter_descriptor(17)]
            };
            Ok(PluginParameterCatalogPage {
                catalog_revision: 1,
                total_items: items.len(),
                items,
            })
        }
        fn save_state(&mut self) -> Result<Vec<u8>, String> {
            let mut mock = self.0.lock().unwrap();
            assert!(!mock.open, "snapshot must detach editor first");
            mock.calls.push("save");
            if mock.edit_during_capture {
                mock.revision += 1;
            }
            Ok(mock.bytes.clone())
        }
        fn load_state(&mut self, bytes: &[u8]) -> Result<(), String> {
            let mut mock = self.0.lock().unwrap();
            assert!(!mock.open, "restore must detach editor first");
            mock.calls.push("load");
            mock.bytes = bytes.to_vec();
            Ok(())
        }
        fn native_editor(
            &mut self,
            command: NativeEditorCommand,
        ) -> Result<NativeEditorState, String> {
            let mut mock = self.0.lock().unwrap();
            match command {
                NativeEditorCommand::Close => {
                    mock.calls.push("close");
                    mock.open = false;
                }
                NativeEditorCommand::Open { .. } => {
                    if mock.reject_open {
                        return Err("no editor".into());
                    }
                    mock.calls.push("open");
                    mock.open = true;
                }
                NativeEditorCommand::Focus => {
                    mock.calls.push("focus");
                }
            }
            Ok(NativeEditorState {
                supported: true,
                has_editor: true,
                open: mock.open,
                width: if mock.open { 400 } else { 0 },
                height: if mock.open { 300 } else { 0 },
                generation: 1,
            })
        }
        fn native_editor_feedback(&mut self) -> Result<NativeEditorFeedback, String> {
            let mock = self.0.lock().unwrap();
            if mock.fail_feedback {
                return Err("helper disconnected".into());
            }
            Ok(NativeEditorFeedback {
                state: NativeEditorState {
                    supported: true,
                    has_editor: true,
                    open: mock.open,
                    width: if mock.open { 400 } else { 0 },
                    height: if mock.open { 300 } else { 0 },
                    generation: 1,
                },
                dirty_revision: mock.revision,
                catalog_invalidated: false,
            })
        }
        fn latency_samples(&self) -> u32 {
            0
        }
        fn tail_samples(&self) -> u32 {
            0
        }
    }

    fn native_slots(mock: Arc<Mutex<NativeMock>>) -> Vec<WorkerSlot> {
        vec![WorkerSlot {
            backend: Some(Box::new(NativeMockBackend(mock))),
            config: SlotConfig::default(),
            fault: None,
            parameter_catalog_cache: None,
            native_base_ids: Vec::new(),
        }]
    }

    #[test]
    fn native_dirty_and_capture_survive_full_best_effort_event_ring() {
        let mock = Arc::new(Mutex::new(NativeMock {
            open: true,
            revision: 8193,
            bytes: vec![9, 8, 7],
            ..Default::default()
        }));
        let mut slots = native_slots(mock.clone());
        let metrics = BridgeMetrics::default();
        let (mut events, _rx) = RingBuffer::new(1);
        events.push(RuntimeEvent::ShutdownComplete).unwrap();
        poll_native_editors(&mut slots, &mut events, &metrics);
        assert_eq!(
            metrics.native_editors.lock().unwrap()[0].dirty_revision,
            8193
        );
        handle_admin(
            AdminCommand::SaveState {
                slot: 0,
                request_id: 42,
            },
            &mut slots,
            &mut events,
            &metrics,
        );
        let snapshot = metrics.native_editors.lock().unwrap()[0].clone();
        assert_eq!(snapshot.captured_state.as_deref(), Some(&vec![9, 8, 7]));
        assert_eq!(snapshot.captured_dirty_revision, 8193);
        assert!(!snapshot.has_uncaptured_changes());
        assert!(metrics.event_overflows.load(Ordering::Relaxed) > 0);
        assert_eq!(mock.lock().unwrap().calls, ["close", "save"]);
    }

    #[test]
    fn native_titlebar_close_captures_once_and_preserves_revision() {
        let mock = Arc::new(Mutex::new(NativeMock {
            open: true,
            revision: 3,
            bytes: vec![4],
            ..Default::default()
        }));
        let mut slots = native_slots(mock.clone());
        let metrics = BridgeMetrics::default();
        let (mut events, _rx) = RingBuffer::new(1);
        poll_native_editors(&mut slots, &mut events, &metrics);
        mock.lock().unwrap().open = false;
        poll_native_editors(&mut slots, &mut events, &metrics);
        poll_native_editors(&mut slots, &mut events, &metrics);
        let snapshot = metrics.native_editors.lock().unwrap()[0].clone();
        assert_eq!(snapshot.capture_serial, 1);
        assert_eq!(snapshot.captured_dirty_revision, 3);
        assert_eq!(mock.lock().unwrap().calls, ["close", "save"]);
    }

    #[test]
    fn rejected_native_open_does_not_fault_healthy_plugin() {
        let mock = Arc::new(Mutex::new(NativeMock {
            reject_open: true,
            ..Default::default()
        }));
        let mut slots = native_slots(mock);
        let metrics = BridgeMetrics::default();
        let (mut events, _rx) = RingBuffer::new(1);
        handle_admin(
            AdminCommand::NativeEditor {
                slot: 0,
                request_id: 7,
                base_parameter_ids: vec![],
                command: NativeEditorCommand::Open {
                    owner: Some((123, 456)),
                },
            },
            &mut slots,
            &mut events,
            &metrics,
        );
        assert!(slots[0].fault.is_none());
        let snapshot = metrics.native_editors.lock().unwrap()[0].clone();
        assert_eq!(snapshot.completed_request, 7);
        assert_eq!(snapshot.error.as_deref(), Some("no editor"));
        assert_eq!(metrics.faults.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn lost_native_feedback_stays_unsaved_even_if_fault_event_overflows() {
        let mock = Arc::new(Mutex::new(NativeMock {
            fail_feedback: true,
            ..Default::default()
        }));
        let mut slots = native_slots(mock);
        let metrics = BridgeMetrics::default();
        let (mut events, _rx) = RingBuffer::new(1);
        events.push(RuntimeEvent::ShutdownComplete).unwrap();
        poll_native_editors(&mut slots, &mut events, &metrics);
        let snapshot = metrics.native_editors.lock().unwrap()[0].clone();
        assert!(snapshot.has_uncaptured_changes());
        assert!(snapshot.error.as_ref().unwrap().contains("disconnected"));
        assert!(slots[0].fault.is_some());
    }

    #[test]
    fn faulted_native_close_cannot_make_unpolled_edits_clean() {
        let mock = Arc::new(Mutex::new(NativeMock {
            open: true,
            ..Default::default()
        }));
        let mut slots = native_slots(mock.clone());
        slots[0].fault = Some("DSP failed before native poll".into());
        let metrics = BridgeMetrics::default();
        metrics.native_editors.lock().unwrap()[0].state.open = true;
        let (mut events, _rx) = RingBuffer::new(1);
        handle_admin(
            AdminCommand::NativeEditor {
                slot: 0,
                request_id: 8,
                command: NativeEditorCommand::Close,
                base_parameter_ids: vec![],
            },
            &mut slots,
            &mut events,
            &metrics,
        );
        let snapshot = metrics.native_editors.lock().unwrap()[0].clone();
        assert!(!snapshot.state.open);
        assert!(snapshot.has_uncaptured_changes());
        assert!(snapshot.error.unwrap().contains("DSP failed"));
        assert_eq!(mock.lock().unwrap().calls, ["close"]);
    }

    #[test]
    fn native_capture_refreshes_only_known_bases_once_per_native_change() {
        let mock = Arc::new(Mutex::new(NativeMock {
            open: true,
            bytes: vec![7],
            parameter_value: 0.625,
            ..Default::default()
        }));
        let mut slots = native_slots(mock.clone());
        slots[0].native_base_ids = vec![17];
        let metrics = BridgeMetrics::default();
        let (mut events, _rx) = RingBuffer::new(1);
        poll_native_editors(&mut slots, &mut events, &metrics);
        mock.lock().unwrap().open = false;
        poll_native_editors(&mut slots, &mut events, &metrics);
        let first = metrics.native_editors.lock().unwrap()[0].clone();
        assert_eq!(
            first.captured_parameters.as_deref(),
            Some(&vec![(17, 0.625)])
        );
        assert_eq!(first.parameter_capture_serial, 1);
        // Later generic edit changes live value but does not create another native revision.
        mock.lock().unwrap().parameter_value = 0.875;
        handle_admin(
            AdminCommand::SaveState {
                slot: 0,
                request_id: 2,
            },
            &mut slots,
            &mut events,
            &metrics,
        );
        let second = metrics.native_editors.lock().unwrap()[0].clone();
        assert_eq!(
            second.parameter_capture_serial,
            first.parameter_capture_serial
        );
        assert!(second.capture_serial > first.capture_serial);
        assert_eq!(
            mock.lock()
                .unwrap()
                .calls
                .iter()
                .filter(|call| **call == "parameter")
                .count(),
            1
        );
    }

    #[test]
    fn missing_native_automation_base_fails_capture_without_clean_receipt() {
        let mock = Arc::new(Mutex::new(NativeMock {
            open: true,
            query_error: true,
            ..Default::default()
        }));
        let mut slots = native_slots(mock.clone());
        slots[0].native_base_ids = vec![17];
        let metrics = BridgeMetrics::default();
        let (mut events, _rx) = RingBuffer::new(1);
        poll_native_editors(&mut slots, &mut events, &metrics);
        mock.lock().unwrap().open = false;
        poll_native_editors(&mut slots, &mut events, &metrics);
        let snapshot = metrics.native_editors.lock().unwrap()[0].clone();
        assert!(snapshot.has_uncaptured_changes());
        assert!(snapshot.captured_state.is_none());
        assert!(snapshot.error.unwrap().contains("base 17"));
    }

    #[test]
    fn unknown_native_base_returning_zero_is_rejected_by_current_metadata() {
        let mock = Arc::new(Mutex::new(NativeMock {
            open: true,
            missing_parameter: true,
            parameter_value: 0.0, // Real VST3 controllers may return zero for unknown IDs.
            ..Default::default()
        }));
        let mut slots = native_slots(mock.clone());
        slots[0].native_base_ids = vec![17];
        let metrics = BridgeMetrics::default();
        let (mut events, _rx) = RingBuffer::new(1);
        poll_native_editors(&mut slots, &mut events, &metrics);
        mock.lock().unwrap().open = false;
        poll_native_editors(&mut slots, &mut events, &metrics);
        let snapshot = metrics.native_editors.lock().unwrap()[0].clone();
        assert!(snapshot.has_uncaptured_changes());
        assert!(snapshot.captured_state.is_none());
        assert!(snapshot.captured_parameters.is_none());
        assert!(
            snapshot
                .error
                .unwrap()
                .contains("absent from the current parameter catalog")
        );
        let calls = &mock.lock().unwrap().calls;
        assert!(calls.contains(&"catalog"));
        assert!(!calls.contains(&"parameter"));
    }

    #[test]
    fn native_revision_change_rejects_mismatched_parameter_and_opaque_snapshots() {
        let mock = Arc::new(Mutex::new(NativeMock {
            open: true,
            edit_during_capture: true,
            parameter_value: 0.625,
            ..Default::default()
        }));
        let mut slots = native_slots(mock.clone());
        slots[0].native_base_ids = vec![17];
        let metrics = BridgeMetrics::default();
        let (mut events, _rx) = RingBuffer::new(1);
        poll_native_editors(&mut slots, &mut events, &metrics);
        mock.lock().unwrap().open = false;
        poll_native_editors(&mut slots, &mut events, &metrics);
        let snapshot = metrics.native_editors.lock().unwrap()[0].clone();
        assert!(snapshot.captured_state.is_none());
        assert!(snapshot.captured_parameters.is_none());
        assert!(snapshot.error.unwrap().contains("capture barrier"));
    }

    #[test]
    fn native_close_retains_state_even_when_plugin_omits_dirty_callbacks() {
        let mock = Arc::new(Mutex::new(NativeMock {
            open: true,
            bytes: vec![77],
            ..Default::default()
        }));
        let mut slots = native_slots(mock.clone());
        let metrics = BridgeMetrics::default();
        let (mut events, _rx) = RingBuffer::new(1);
        poll_native_editors(&mut slots, &mut events, &metrics);
        mock.lock().unwrap().open = false;
        poll_native_editors(&mut slots, &mut events, &metrics);
        let snapshot = metrics.native_editors.lock().unwrap()[0].clone();
        assert_eq!(snapshot.dirty_revision, 0);
        assert_eq!(snapshot.capture_serial, 1);
        assert_eq!(snapshot.captured_state.as_deref(), Some(&vec![77]));
    }

    #[test]
    fn authoritative_restore_closes_native_editor_before_loading() {
        let mock = Arc::new(Mutex::new(NativeMock {
            open: true,
            ..Default::default()
        }));
        let mut slots = native_slots(mock.clone());
        let metrics = BridgeMetrics::default();
        let (mut events, _rx) = RingBuffer::new(1);
        handle_admin(
            AdminCommand::LoadState {
                slot: 0,
                state: vec![1, 2],
            },
            &mut slots,
            &mut events,
            &metrics,
        );
        assert_eq!(mock.lock().unwrap().calls, ["close", "load"]);
        assert_eq!(mock.lock().unwrap().bytes, [1, 2]);
    }

    #[test]
    fn worker_block_stably_orders_untagged_timeline_before_tagged_live_edits() {
        let first_live = ParameterEditId::new(1).unwrap();
        let second_live = ParameterEditId::new(2).unwrap();
        let source = [
            RtCommand::Parameter {
                slot: 0,
                id: 10,
                normalized: 0.1,
                edit_id: Some(first_live),
            },
            RtCommand::Midi {
                target: RtTarget::All,
                message: MidiMessage::new([0x90, 60, 1], 0),
            },
            RtCommand::Parameter {
                slot: 0,
                id: 11,
                normalized: 0.2,
                edit_id: None,
            },
            RtCommand::Parameter {
                slot: 0,
                id: 12,
                normalized: 0.3,
                edit_id: Some(second_live),
            },
        ];
        let mut ordered = [RtCommand::EMPTY; 4];
        copy_rt_commands_timeline_before_live(&source, &mut ordered);
        assert!(matches!(ordered[0], RtCommand::Midi { .. }));
        assert!(matches!(
            ordered[1],
            RtCommand::Parameter {
                id: 11,
                edit_id: None,
                ..
            }
        ));
        assert!(matches!(
            ordered[2],
            RtCommand::Parameter {
                id: 10,
                edit_id: Some(edit_id),
                ..
            } if edit_id == first_live
        ));
        assert!(matches!(
            ordered[3],
            RtCommand::Parameter {
                id: 12,
                edit_id: Some(edit_id),
                ..
            } if edit_id == second_live
        ));
    }
    use std::{
        sync::atomic::{AtomicBool, Ordering},
        time::Instant,
    };

    #[derive(Clone, Copy)]
    enum MockProcess {
        Gain(f32),
        ParameterGain,
        ParameterSetsLatency(u32),
        Add(f32),
        SetMetadata { latency: u32, tail: u32 },
        Tail,
        Panic,
        NonFinite,
        Slow(Duration),
    }

    struct MockBackend {
        name: String,
        process: MockProcess,
        parameter: f32,
        state: Vec<u8>,
        latency: u32,
        tail: u32,
        entered: Option<Arc<AtomicBool>>,
        last_midi: Option<Arc<AtomicU64>>,
        note_on_count: Option<Arc<AtomicU64>>,
        reset_count: Option<Arc<AtomicU64>>,
        reset_fails: bool,
        tail_left: f32,
        tail_right: f32,
        parameter_catalog: Vec<PluginParameterDescriptor>,
        parameter_catalog_revision: u64,
        parameter_snapshot_getters: Option<Arc<AtomicU64>>,
        transports: Option<Arc<std::sync::Mutex<Vec<PluginTransport>>>>,
    }

    impl MockBackend {
        fn new(name: &str, process: MockProcess) -> Self {
            Self {
                name: name.into(),
                process,
                parameter: 0.0,
                state: vec![1, 2, 3],
                latency: 0,
                tail: 0,
                entered: None,
                last_midi: None,
                note_on_count: None,
                reset_count: None,
                reset_fails: false,
                tail_left: 0.0,
                tail_right: 0.0,
                parameter_catalog: Vec::new(),
                parameter_catalog_revision: 1,
                parameter_snapshot_getters: None,
                transports: None,
            }
        }
    }

    impl PluginBackend for MockBackend {
        fn name(&self) -> &str {
            &self.name
        }

        fn prepare(&mut self, _config: PluginPrepareConfig) -> Result<(), String> {
            Ok(())
        }

        fn set_transport(&mut self, transport: PluginTransport) -> Result<(), String> {
            if let Some(transports) = &self.transports {
                transports.lock().unwrap().push(transport);
            }
            Ok(())
        }

        fn process(
            &mut self,
            left: &mut [f32],
            right: &mut [f32],
            frames: usize,
        ) -> Result<(), String> {
            match self.process {
                MockProcess::Gain(gain) => {
                    for sample in left[..frames].iter_mut().chain(&mut right[..frames]) {
                        *sample *= gain;
                    }
                }
                MockProcess::ParameterGain => {
                    for sample in left[..frames].iter_mut().chain(&mut right[..frames]) {
                        *sample *= self.parameter;
                    }
                }
                MockProcess::ParameterSetsLatency(_) => {}
                MockProcess::Add(value) => {
                    for sample in left[..frames].iter_mut().chain(&mut right[..frames]) {
                        *sample += value;
                    }
                }
                MockProcess::SetMetadata { latency, tail } => {
                    self.latency = latency;
                    self.tail = tail;
                }
                MockProcess::Tail => {
                    for frame in 0..frames {
                        let next_left = left[frame] + self.tail_left * 0.5;
                        let next_right = right[frame] + self.tail_right * 0.5;
                        self.tail_left = next_left;
                        self.tail_right = next_right;
                        left[frame] = next_left;
                        right[frame] = next_right;
                    }
                }
                MockProcess::Panic => panic!("mock exploded"),
                MockProcess::NonFinite => left[0] = f32::NAN,
                MockProcess::Slow(duration) => {
                    if let Some(entered) = &self.entered {
                        entered.store(true, Ordering::Release);
                    }
                    thread::sleep(duration);
                }
            }
            Ok(())
        }

        fn send_midi(&mut self, message: MidiMessage) -> Result<(), String> {
            if message.data[0] & 0xf0 == 0x90
                && message.data[2] != 0
                && let Some(note_on_count) = &self.note_on_count
            {
                note_on_count.fetch_add(1, Ordering::Relaxed);
            }
            if let Some(last_midi) = &self.last_midi {
                let packed = u64::from(message.data[0])
                    | (u64::from(message.data[1]) << 8)
                    | (u64::from(message.data[2]) << 16)
                    | (u64::from(message.sample_offset) << 24);
                last_midi.store(packed, Ordering::Release);
            }
            Ok(())
        }

        fn set_parameter(&mut self, id: u32, normalized: f32) -> Result<(), String> {
            if id == u32::MAX {
                return Err("mock parameter does not exist".into());
            }
            if id == u32::MAX - 1 {
                panic!("mock parameter setter exploded");
            }
            self.parameter = normalized;
            if let MockProcess::ParameterSetsLatency(latency) = self.process {
                self.latency = latency;
            }
            Ok(())
        }

        fn get_parameter(&mut self, id: u32) -> Result<f32, String> {
            if id == u32::MAX {
                return Err("mock parameter does not exist".into());
            }
            if id == u32::MAX - 2 {
                return Ok(f32::NAN);
            }
            if id == u32::MAX - 3 {
                return Err("mock readback unavailable".into());
            }
            Ok(self.parameter)
        }

        fn parameter_catalog_page(
            &mut self,
            cursor: usize,
            limit: usize,
        ) -> Result<PluginParameterCatalogPage, String> {
            let total_items = self.parameter_catalog.len();
            if total_items > MAX_PLUGIN_PARAMETER_CATALOG_ITEMS {
                return Err("mock catalog exceeds the host limit".into());
            }
            let end = cursor.saturating_add(limit).min(total_items);
            let items = if cursor < total_items {
                self.parameter_catalog[cursor..end].to_vec()
            } else {
                Vec::new()
            };
            Ok(PluginParameterCatalogPage {
                catalog_revision: self.parameter_catalog_revision,
                total_items,
                items,
            })
        }

        fn parameter_catalog_snapshot(&mut self) -> Result<PluginParameterCatalogPage, String> {
            if let Some(getters) = &self.parameter_snapshot_getters {
                getters.fetch_add(self.parameter_catalog.len() as u64, Ordering::Relaxed);
            }
            Ok(PluginParameterCatalogPage {
                catalog_revision: self.parameter_catalog_revision,
                total_items: self.parameter_catalog.len(),
                items: self.parameter_catalog.clone(),
            })
        }

        fn save_state(&mut self) -> Result<Vec<u8>, String> {
            Ok(self.state.clone())
        }

        fn load_state(&mut self, state: &[u8]) -> Result<(), String> {
            self.state.clear();
            self.state.extend_from_slice(state);
            Ok(())
        }

        fn reset_processing(&mut self) -> Result<(), String> {
            if let Some(reset_count) = &self.reset_count {
                reset_count.fetch_add(1, Ordering::Relaxed);
            }
            if self.reset_fails {
                return Err("mock reset rejected".into());
            }
            self.tail_left = 0.0;
            self.tail_right = 0.0;
            Ok(())
        }

        fn latency_samples(&self) -> u32 {
            self.latency
        }

        fn tail_samples(&self) -> u32 {
            self.tail
        }
    }

    #[test]
    fn midi_lookahead_counts_real_deadline_misses_but_not_sixteen_startup_quanta() {
        let (mut audio, _input, _output) = manual_endpoint(DEFAULT_QUEUE_CAPACITY, 64);
        assert!(audio.set_bridge_lookahead_quanta(16));
        let mut left = [0.0; 64];
        let mut right = [0.0; 64];
        for _ in 0..16 {
            audio.process_realtime(&[0.0; 64], &[0.0; 64], &mut left, &mut right);
        }
        assert_eq!(audio.stats().deadline_misses, 0);
        audio.process_realtime(&[0.0; 64], &[0.0; 64], &mut left, &mut right);
        assert_eq!(audio.stats().deadline_misses, 1);
    }

    #[test]
    fn chain_transport_subtracts_active_preceding_plugin_latency() {
        let first_context = Arc::new(std::sync::Mutex::new(Vec::new()));
        let second_context = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut chain = PluginChain::spawn_with_backend_factory(
            {
                let first_context = Arc::clone(&first_context);
                let second_context = Arc::clone(&second_context);
                move || {
                    let mut first = MockBackend::new("first", MockProcess::Gain(1.0));
                    first.latency = 37;
                    first.transports = Some(first_context);
                    let mut second = MockBackend::new("second", MockProcess::Gain(1.0));
                    second.latency = 19;
                    second.transports = Some(second_context);
                    vec![
                        BackendSlot::new(Box::new(first)),
                        BackendSlot::new(Box::new(second)),
                    ]
                }
            },
            config(),
        )
        .unwrap();
        wait_until(|| {
            chain
                .control
                .plugin_latency_snapshot()
                .is_some_and(|snapshot| snapshot.total_plugin_latency_samples == 56)
        });
        let transport = PluginTransport {
            sample_position: 12,
            quarter_note_position: 0.0005,
            tempo: 120.0,
            playing: true,
            ..PluginTransport::default()
        };
        chain.audio.set_transport(transport);
        chain.audio.try_submit(&[0.0; 64], &[0.0; 64]);
        receive(&mut chain.audio, 64);
        assert_eq!(first_context.lock().unwrap()[0], transport);
        let second = second_context.lock().unwrap()[0];
        assert_eq!(second.sample_position, -25);
        assert!(
            (second.quarter_note_position - (transport.quarter_note_position - 37.0 / 24_000.0))
                .abs()
                < 1e-12
        );
        assert_eq!(second.tempo, 120.0);
        assert!(second.playing);
    }

    fn config() -> PluginPrepareConfig {
        PluginPrepareConfig {
            sample_rate: 48_000.0,
            max_block_frames: 64,
        }
    }

    fn load_spec(name: &str) -> PluginLoadSpec {
        PluginLoadSpec::from_descriptor(PluginDescriptor {
            id: format!("test-{name}"),
            name: name.to_owned(),
            vendor: "Citrus tests".to_owned(),
            path: PathBuf::from(format!("missing-{name}.dll")),
            format: PluginFormat::Vst2,
            category: "Effect".to_owned(),
            is_instrument: false,
            verified: false,
            vst3_metadata: None,
            scan_error: None,
        })
    }

    fn spawn_error(specs: Vec<(u64, PluginLoadSpec)>) -> String {
        match PluginChain::spawn_identified(specs, config()) {
            Ok(chain) => {
                chain.guard.shutdown();
                panic!("invalid identified manifest unexpectedly spawned")
            }
            Err(error) => error,
        }
    }

    fn manual_endpoint(
        capacity: usize,
        max_block_frames: usize,
    ) -> (
        AudioThreadEndpoint,
        Consumer<StereoBlock>,
        Producer<StereoBlock>,
    ) {
        let (input, input_consumer) = RingBuffer::new(capacity);
        let (output_producer, output) = RingBuffer::new(capacity);
        let metrics = Arc::new(BridgeMetrics::default());
        metrics
            .max_block_frames
            .store(max_block_frames as u32, Ordering::Relaxed);
        metrics
            .current_epoch
            .store(INITIAL_TRANSPORT_EPOCH, Ordering::Relaxed);
        let requested_epoch = Arc::new(AtomicU64::new(INITIAL_TRANSPORT_EPOCH));
        let parameter_edit_admission = Arc::new(AtomicU32::new(0));
        let (parameter_edit_failures, _parameter_edit_failure_rx) =
            RingBuffer::new(MAX_OUTSTANDING_PARAMETER_EDITS as usize);
        (
            AudioThreadEndpoint {
                input,
                output,
                epoch: INITIAL_TRANSPORT_EPOCH,
                next_sequence: 1,
                requested_epoch,
                scratch: Box::new(EndpointScratch::new()),
                metrics,
                manifest: PluginEndpointManifest::default(),
                parameter_edit_admission,
                parameter_edit_failures,
            },
            input_consumer,
            output_producer,
        )
    }

    fn completed_block(sequence: u64, frames: usize, value: f32) -> StereoBlock {
        completed_block_in_epoch(INITIAL_TRANSPORT_EPOCH, sequence, frames, value)
    }

    fn completed_block_in_epoch(
        epoch: u64,
        sequence: u64,
        frames: usize,
        value: f32,
    ) -> StereoBlock {
        let mut block = StereoBlock::silence();
        block.epoch = epoch;
        block.sequence = sequence;
        block.frames = frames as u16;
        block.left[..frames].fill(value);
        block.right[..frames].fill(value);
        block
    }

    fn receive(audio: &mut AudioThreadEndpoint, expected_frames: usize) -> ([f32; 64], [f32; 64]) {
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut left = [0.0; 64];
        let mut right = [0.0; 64];
        loop {
            match audio.try_receive(&mut left, &mut right) {
                ReceiveStatus::Processed { frames, .. } => {
                    assert_eq!(frames, expected_frames);
                    return (left, right);
                }
                ReceiveStatus::Empty if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(1));
                }
                status => panic!("timed out waiting for worker output: {status:?}"),
            }
        }
    }

    fn wait_until(mut predicate: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !predicate() {
            assert!(Instant::now() < deadline, "timed out waiting for worker");
            thread::sleep(Duration::from_millis(1));
        }
    }

    fn wait_for_event(
        control: &mut PluginChainControl,
        mut predicate: impl FnMut(&RuntimeEvent) -> bool,
    ) -> RuntimeEvent {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(event) = control.try_next_event()
                && predicate(&event)
            {
                return event;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for runtime event"
            );
            thread::sleep(Duration::from_millis(1));
        }
    }

    fn wait_for_parameter_edit_receipt(control: &mut PluginChainControl) -> ParameterEditReceipt {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(receipt) = control.try_next_parameter_edit_receipt() {
                return receipt;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for parameter edit receipt"
            );
            thread::sleep(Duration::from_millis(1));
        }
    }

    fn parameter_descriptor(id: u32) -> PluginParameterDescriptor {
        PluginParameterDescriptor {
            id,
            name: format!("Parameter {id}"),
            unit: "%".into(),
            current_normalized: id as f32 / 100.0,
            default_normalized: Some(0.5),
            step_count: Some(0),
            automatable: true,
            read_only: false,
            bypass: false,
        }
    }

    #[test]
    fn catalog_revision_ignores_live_values_but_tracks_ordered_metadata() {
        let original = vec![parameter_descriptor(1), parameter_descriptor(2)];
        let revision = parameter_catalog_revision(&original);
        assert_ne!(revision, 0);

        let mut live_change = original.clone();
        live_change[0].current_normalized = 0.987;
        live_change[1].current_normalized = 0.123;
        assert_eq!(parameter_catalog_revision(&live_change), revision);

        let mut metadata_change = original.clone();
        metadata_change[0].name.push_str(" changed");
        assert_ne!(parameter_catalog_revision(&metadata_change), revision);

        let mut reordered = original.clone();
        reordered.swap(0, 1);
        assert_ne!(parameter_catalog_revision(&reordered), revision);
    }

    #[test]
    fn identified_manifest_preserves_slot_order_and_matches_both_endpoint_owners() {
        let chain = PluginChain::spawn_identified(
            vec![
                (91, load_spec("first")),
                (7, load_spec("second")),
                (42, load_spec("third")),
            ],
            config(),
        )
        .unwrap();
        let PluginChain {
            audio,
            control,
            guard,
        } = chain;
        let moved_audio = audio;
        let moved_control = control;
        let audio_manifest = moved_audio.plugin_endpoint_manifest();
        let control_manifest = moved_control.plugin_endpoint_manifest();

        assert_eq!(audio_manifest, control_manifest);
        assert!(audio_manifest.is_identified());
        assert_eq!(audio_manifest.slot_count, 3);
        assert_eq!(
            audio_manifest.instance_ids,
            [
                Some(91),
                Some(7),
                Some(42),
                None,
                None,
                None,
                None,
                None,
                None,
                None
            ]
        );
        assert_eq!(audio_manifest.instance_id(0), Some(91));
        assert_eq!(audio_manifest.instance_id(2), Some(42));
        assert_eq!(audio_manifest.instance_id(3), None);
        assert_eq!(
            guard.shutdown_blocking(Duration::from_secs(2)),
            ShutdownOutcome::Joined
        );
    }

    #[test]
    fn identified_spawn_rejects_zero_duplicate_and_excess_instance_ids() {
        assert!(spawn_error(vec![(0, load_spec("zero"))]).contains("slot 0 must be nonzero"));
        assert!(
            spawn_error(vec![(9, load_spec("first")), (9, load_spec("duplicate"))])
                .contains("duplicate plug-in instance id 9 in slots 0 and 1")
        );
        let excessive = (0..=MAX_PLUGIN_CHAIN_SLOTS)
            .map(|slot| (slot as u64 + 1, load_spec(&format!("slot-{slot}"))))
            .collect();
        assert!(spawn_error(excessive).contains("an insert supports at most 10 plug-in slots"));
    }

    #[test]
    fn exact_endpoint_snapshot_exposes_slot_identity_and_one_latency_publication() {
        let manifest = PluginEndpointManifest::identified(&[91, 7, 42]).unwrap();
        let latency = PluginLatencySnapshot {
            revision: 73,
            active_mask: 0b101,
            slot_latency_samples: [11, 0, 17, 0, 0, 0, 0, 0, 0, 0],
            total_plugin_latency_samples: 28,
            tail_samples: 4_096,
        };
        let snapshot = PluginEndpointSnapshot::try_new(manifest, latency).unwrap();

        assert_eq!(snapshot.manifest(), manifest);
        assert_eq!(snapshot.latency(), latency);
        assert_eq!(snapshot.slot_count(), 3);
        assert_eq!(snapshot.revision(), 73);
        assert_eq!(snapshot.active_mask(), 0b101);
        assert_eq!(snapshot.total_plugin_latency_samples(), 28);
        assert_eq!(snapshot.tail_samples(), 4_096);
        assert_eq!(
            snapshot.slot(0),
            Some(PluginEndpointSlotSnapshot {
                instance_id: 91,
                active: true,
                latency_samples: 11,
                prefix_latency_samples: 0,
            })
        );
        assert_eq!(
            snapshot.slot(1),
            Some(PluginEndpointSlotSnapshot {
                instance_id: 7,
                active: false,
                latency_samples: 0,
                prefix_latency_samples: 11,
            })
        );
        assert_eq!(
            snapshot.slot(2),
            Some(PluginEndpointSlotSnapshot {
                instance_id: 42,
                active: true,
                latency_samples: 17,
                prefix_latency_samples: 11,
            })
        );
        assert_eq!(snapshot.slot(3), None);
    }

    #[test]
    fn exact_endpoint_snapshot_rejects_every_manifest_and_latency_incoherency() {
        let valid_manifest = PluginEndpointManifest::identified(&[91, 7]).unwrap();
        let valid_latency = PluginLatencySnapshot {
            revision: 1,
            active_mask: 0b11,
            slot_latency_samples: [3, 5, 0, 0, 0, 0, 0, 0, 0, 0],
            total_plugin_latency_samples: 8,
            tail_samples: 13,
        };

        assert_eq!(
            PluginEndpointSnapshot::try_new(
                PluginEndpointManifest::unknown_for_slots(2).unwrap(),
                valid_latency,
            ),
            Err(PluginEndpointSnapshotError::ManifestNotIdentified)
        );

        let mut invalid_count = valid_manifest;
        invalid_count.slot_count = MAX_PLUGIN_CHAIN_SLOTS + 1;
        assert_eq!(
            PluginEndpointSnapshot::try_new(invalid_count, valid_latency),
            Err(PluginEndpointSnapshotError::SlotCountOutOfRange {
                slot_count: MAX_PLUGIN_CHAIN_SLOTS + 1,
            })
        );

        let mut missing = valid_manifest;
        missing.instance_ids[1] = None;
        assert_eq!(
            PluginEndpointSnapshot::try_new(missing, valid_latency),
            Err(PluginEndpointSnapshotError::MissingInstanceId { slot: 1 })
        );

        let mut zero = valid_manifest;
        zero.instance_ids[1] = Some(0);
        assert_eq!(
            PluginEndpointSnapshot::try_new(zero, valid_latency),
            Err(PluginEndpointSnapshotError::ZeroInstanceId { slot: 1 })
        );

        let mut duplicate = valid_manifest;
        duplicate.instance_ids[1] = Some(91);
        assert_eq!(
            PluginEndpointSnapshot::try_new(duplicate, valid_latency),
            Err(PluginEndpointSnapshotError::DuplicateInstanceId {
                first_slot: 0,
                duplicate_slot: 1,
            })
        );

        let mut outside = valid_manifest;
        outside.instance_ids[2] = Some(42);
        assert_eq!(
            PluginEndpointSnapshot::try_new(outside, valid_latency),
            Err(PluginEndpointSnapshotError::InstanceIdOutsideManifest { slot: 2 })
        );

        assert_eq!(
            PluginEndpointSnapshot::try_new(
                valid_manifest,
                PluginLatencySnapshot {
                    revision: 0,
                    ..valid_latency
                },
            ),
            Err(PluginEndpointSnapshotError::LatencyNotPublished)
        );
        assert_eq!(
            PluginEndpointSnapshot::try_new(
                valid_manifest,
                PluginLatencySnapshot {
                    active_mask: 0b100,
                    slot_latency_samples: [0, 0, 7, 0, 0, 0, 0, 0, 0, 0],
                    total_plugin_latency_samples: 7,
                    ..valid_latency
                },
            ),
            Err(PluginEndpointSnapshotError::ActiveSlotOutsideManifest { active_mask: 0b100 })
        );
        assert_eq!(
            PluginEndpointSnapshot::try_new(
                valid_manifest,
                PluginLatencySnapshot {
                    active_mask: 0b01,
                    slot_latency_samples: [3, 5, 0, 0, 0, 0, 0, 0, 0, 0],
                    total_plugin_latency_samples: 3,
                    ..valid_latency
                },
            ),
            Err(PluginEndpointSnapshotError::InactiveSlotHasLatency {
                slot: 1,
                latency_samples: 5,
            })
        );
        assert_eq!(
            PluginEndpointSnapshot::try_new(
                valid_manifest,
                PluginLatencySnapshot {
                    total_plugin_latency_samples: 9,
                    ..valid_latency
                },
            ),
            Err(PluginEndpointSnapshotError::TotalLatencyMismatch {
                expected: 8,
                actual: 9,
            })
        );
    }

    #[test]
    fn callback_endpoint_pairs_immutable_manifest_with_coherent_metrics_read() {
        let (mut audio, _input, _output) = manual_endpoint(2, 64);
        audio.manifest = PluginEndpointManifest::identified(&[91, 7]).unwrap();
        publish_plugin_latency_snapshot(
            &audio.metrics,
            0b10,
            [0, 19, 0, 0, 0, 0, 0, 0, 0, 0],
            19,
            31,
        );

        let snapshot = audio.plugin_endpoint_snapshot().unwrap();
        assert_eq!(snapshot.manifest(), audio.plugin_endpoint_manifest());
        assert_eq!(snapshot.revision(), 1);
        assert_eq!(snapshot.active_mask(), 0b10);
        assert_eq!(snapshot.slot(1).unwrap().instance_id(), 7);
        assert_eq!(snapshot.slot(1).unwrap().prefix_latency_samples(), 0);
        assert_eq!(snapshot.tail_samples(), 31);
    }

    #[test]
    fn legacy_constructors_expose_unknown_identity_without_fabricating_ids() {
        let legacy = PluginChain::spawn(vec![load_spec("legacy")], config()).unwrap();
        let audio_manifest = legacy.audio.plugin_endpoint_manifest();
        assert!(!audio_manifest.is_identified());
        assert_eq!(audio_manifest.slot_count, 1);
        assert_eq!(audio_manifest.instance_ids, [None; MAX_PLUGIN_CHAIN_SLOTS]);
        assert_eq!(audio_manifest, legacy.control.plugin_endpoint_manifest());
        assert_eq!(
            legacy.guard.shutdown_blocking(Duration::from_secs(2)),
            ShutdownOutcome::Joined
        );

        let factory = PluginChain::spawn_with_backend_factory(
            || {
                vec![BackendSlot::new(Box::new(MockBackend::new(
                    "legacy factory",
                    MockProcess::Gain(1.0),
                )))]
            },
            config(),
        )
        .unwrap();
        let factory_manifest = factory.audio.plugin_endpoint_manifest();
        assert!(!factory_manifest.is_identified());
        assert_eq!(factory_manifest.slot_count, 0);
        assert_eq!(
            factory_manifest.instance_ids,
            [None; MAX_PLUGIN_CHAIN_SLOTS]
        );
        assert_eq!(factory_manifest, factory.control.plugin_endpoint_manifest());
        assert_eq!(
            factory.guard.shutdown_blocking(Duration::from_secs(2)),
            ShutdownOutcome::Joined
        );
    }

    #[test]
    fn chain_processes_slots_in_order() {
        let mut chain = PluginChain::spawn_with_backend_factory(
            || {
                vec![
                    BackendSlot::new(Box::new(MockBackend::new("add", MockProcess::Add(1.0)))),
                    BackendSlot::new(Box::new(MockBackend::new(
                        "multiply",
                        MockProcess::Gain(2.0),
                    ))),
                ]
            },
            config(),
        )
        .unwrap();
        let input = [1.0; 8];
        assert!(matches!(
            chain.audio.try_submit(&input, &input),
            SubmitStatus::Submitted { .. }
        ));
        let (left, right) = receive(&mut chain.audio, input.len());
        assert_eq!(&left[..input.len()], &[4.0; 8]);
        assert_eq!(&right[..input.len()], &[4.0; 8]);
    }

    #[test]
    fn wet_and_bypass_are_applied_inside_the_serial_worker() {
        let mut chain = PluginChain::spawn_with_backend_factory(
            || {
                let mut wet = BackendSlot::new(Box::new(MockBackend::new(
                    "wet gain",
                    MockProcess::Gain(3.0),
                )));
                wet.config.wet = 0.5;
                let mut bypassed = BackendSlot::new(Box::new(MockBackend::new(
                    "bypassed add",
                    MockProcess::Add(100.0),
                )));
                bypassed.config.bypassed = true;
                vec![wet, bypassed]
            },
            config(),
        )
        .unwrap();
        let input = [1.0; 8];
        chain.audio.try_submit(&input, &input);
        let (left, _) = receive(&mut chain.audio, input.len());
        assert_eq!(&left[..input.len()], &[2.0; 8]);
    }

    #[test]
    fn latency_before_first_submit_includes_only_active_plugins() {
        let chain = PluginChain::spawn_with_backend_factory(
            || {
                let mut first = MockBackend::new("first", MockProcess::Gain(1.0));
                first.latency = 5;
                first.tail = 17;
                let mut second = MockBackend::new("second", MockProcess::Gain(1.0));
                second.latency = 7;
                second.tail = 11;
                vec![
                    BackendSlot::new(Box::new(first)),
                    BackendSlot::new(Box::new(second)),
                ]
            },
            config(),
        )
        .unwrap();
        wait_until(|| chain.control.stats().latency_samples != 0);
        let stats = chain.control.stats();
        assert_eq!(stats.latency_samples, 5 + 7);
        assert_eq!(stats.tail_samples, 17 + 11);

        let snapshot = chain
            .control
            .plugin_latency_snapshot()
            .expect("worker latency publication must be coherent");
        assert_ne!(snapshot.revision, 0);
        assert_eq!(snapshot.active_mask, 0b11);
        assert_eq!(&snapshot.slot_latency_samples[..2], &[5, 7]);
        assert_eq!(snapshot.total_plugin_latency_samples, 12);
        assert_eq!(snapshot.tail_samples, 28);
        assert_eq!(snapshot.prefix_latency_before(0), Some(0));
        assert_eq!(snapshot.prefix_latency_before(1), Some(5));
        assert_eq!(snapshot.prefix_latency_before(2), Some(12));
        assert_eq!(snapshot.prefix_latency_before(MAX_PLUGIN_CHAIN_SLOTS), None);

        let initial_revision = snapshot.revision;
        assert!(chain.control.set_slot_config(
            0,
            SlotConfig {
                bypassed: true,
                ..SlotConfig::default()
            }
        ));
        wait_until(|| {
            chain
                .control
                .plugin_latency_snapshot()
                .is_some_and(|current| current.revision != initial_revision)
        });
        let bypassed = chain.control.plugin_latency_snapshot().unwrap();
        assert_eq!(bypassed.active_mask, 0b10);
        assert_eq!(&bypassed.slot_latency_samples[..2], &[0, 7]);
        assert_eq!(bypassed.total_plugin_latency_samples, 7);
        assert_eq!(bypassed.tail_samples, 11);
    }

    #[test]
    fn empty_chain_publishes_a_nonzero_initial_latency_revision() {
        let chain = PluginChain::spawn_with_backend_factory(Vec::new, config()).unwrap();
        wait_until(|| chain.control.plugin_latency_snapshot().is_some());

        let snapshot = chain.control.plugin_latency_snapshot().unwrap();
        assert_ne!(snapshot.revision, 0);
        assert_eq!(snapshot.active_mask, 0);
        assert_eq!(snapshot.slot_latency_samples, [0; MAX_PLUGIN_CHAIN_SLOTS]);
        assert_eq!(snapshot.total_plugin_latency_samples, 0);
        assert_eq!(snapshot.tail_samples, 0);
    }

    #[test]
    fn stable_expected_latency_revision_returns_plugin_output() {
        let mut chain = PluginChain::spawn_with_backend_factory(
            || {
                vec![BackendSlot::new(Box::new(MockBackend::new(
                    "attested stable",
                    MockProcess::Gain(2.0),
                )))]
            },
            config(),
        )
        .unwrap();
        wait_until(|| chain.control.plugin_latency_snapshot().is_some());
        let revision = chain.control.plugin_latency_snapshot().unwrap().revision;
        let input = [0.25; 8];
        let mut left = [9.0; 8];
        let mut right = [9.0; 8];

        let first = chain.audio.process_realtime_with_expected_latency_revision(
            &input, &input, &mut left, &mut right, revision,
        );
        assert!(matches!(
            first,
            RealtimeProcessStatus::Processed {
                source: RealtimeOutputSource::DelayedDry,
                ..
            }
        ));
        wait_until(|| chain.audio.stats().completed == 1);
        let second = chain.audio.process_realtime_with_expected_latency_revision(
            &input, &input, &mut left, &mut right, revision,
        );
        assert!(matches!(
            second,
            RealtimeProcessStatus::Processed {
                source: RealtimeOutputSource::Plugin,
                ..
            }
        ));
        assert_eq!(left, [0.5; 8]);
        assert_eq!(right, [0.5; 8]);
        assert_eq!(chain.audio.stats().latency_drift_blocks, 0);
        assert_eq!(chain.audio.stats().latency_drift_outputs, 0);
    }

    #[test]
    fn preexisting_latency_revision_mismatch_is_silent_and_explicit() {
        let mut chain = PluginChain::spawn_with_backend_factory(
            || {
                vec![BackendSlot::new(Box::new(MockBackend::new(
                    "attested mismatch",
                    MockProcess::Gain(2.0),
                )))]
            },
            config(),
        )
        .unwrap();
        wait_until(|| chain.control.plugin_latency_snapshot().is_some());
        let revision = chain.control.plugin_latency_snapshot().unwrap().revision;
        let wrong_revision = revision.wrapping_add(1).max(1);
        let input = [0.5; 8];
        assert!(matches!(
            chain
                .audio
                .try_submit_with_expected_latency_revision(&input, &input, wrong_revision),
            SubmitStatus::Submitted { sequence: 1 }
        ));
        wait_until(|| chain.audio.stats().completed == 1);
        let mut left = [7.0; 8];
        let mut right = [7.0; 8];
        assert_eq!(
            chain.audio.try_receive(&mut left, &mut right),
            ReceiveStatus::LatencyDrift {
                sequence: 1,
                frames: 8
            }
        );
        assert_eq!(left, [0.0; 8]);
        assert_eq!(right, [0.0; 8]);
        let stats = chain.audio.stats();
        assert_eq!(stats.completed, 1, "the worker chain must still advance");
        assert_eq!(stats.latency_drift_blocks, 1);
        assert_eq!(stats.latency_drift_outputs, 1);
    }

    #[test]
    fn parameter_latency_change_invalidates_and_silences_that_same_block() {
        let mut chain = PluginChain::spawn_with_backend_factory(
            || {
                let mut backend =
                    MockBackend::new("parameter latency", MockProcess::ParameterSetsLatency(37));
                backend.latency = 3;
                vec![BackendSlot::new(Box::new(backend))]
            },
            config(),
        )
        .unwrap();
        wait_until(|| chain.control.plugin_latency_snapshot().is_some());
        let initial = chain.control.plugin_latency_snapshot().unwrap();
        assert_eq!(initial.total_plugin_latency_samples, 3);
        assert!(chain.audio.try_set_parameter(0, 9, 0.75));
        let input = [1.0; 8];
        let mut left = [9.0; 8];
        let mut right = [9.0; 8];
        let first = chain.audio.process_realtime_with_expected_latency_revision(
            &input,
            &input,
            &mut left,
            &mut right,
            initial.revision,
        );
        assert!(matches!(
            first,
            RealtimeProcessStatus::Processed {
                source: RealtimeOutputSource::DelayedDry,
                ..
            }
        ));
        wait_until(|| chain.audio.stats().completed == 1);
        let changed = chain.control.plugin_latency_snapshot().unwrap();
        assert_ne!(changed.revision, initial.revision);
        assert_eq!(changed.total_plugin_latency_samples, 37);

        left.fill(9.0);
        right.fill(9.0);
        let second = chain.audio.process_realtime_with_expected_latency_revision(
            &input,
            &input,
            &mut left,
            &mut right,
            changed.revision,
        );
        assert!(matches!(
            second,
            RealtimeProcessStatus::Processed {
                sequence: 1,
                source: RealtimeOutputSource::LatencyDrift,
                ..
            }
        ));
        assert_eq!(left, [0.0; 8]);
        assert_eq!(right, [0.0; 8]);
        let stats = chain.audio.stats();
        assert_eq!(stats.latency_drift_blocks, 1);
        assert_eq!(stats.latency_drift_outputs, 1);
    }

    #[test]
    fn latency_change_discovered_after_processing_is_attestation_invalid() {
        let mut chain = PluginChain::spawn_with_backend_factory(
            || {
                vec![BackendSlot::new(Box::new(MockBackend::new(
                    "process latency",
                    MockProcess::SetMetadata {
                        latency: 19,
                        tail: 0,
                    },
                )))]
            },
            config(),
        )
        .unwrap();
        wait_until(|| chain.control.plugin_latency_snapshot().is_some());
        let revision = chain.control.plugin_latency_snapshot().unwrap().revision;
        let input = [1.0; 8];
        assert!(matches!(
            chain
                .audio
                .try_submit_with_expected_latency_revision(&input, &input, revision),
            SubmitStatus::Submitted { sequence: 1 }
        ));
        wait_until(|| chain.audio.stats().completed == 1);
        let mut left = [5.0; 8];
        let mut right = [5.0; 8];
        assert!(matches!(
            chain.audio.try_receive(&mut left, &mut right),
            ReceiveStatus::LatencyDrift {
                sequence: 1,
                frames: 8
            }
        ));
        assert_eq!(left, [0.0; 8]);
        assert_eq!(right, [0.0; 8]);
        assert_eq!(
            chain
                .control
                .plugin_latency_snapshot()
                .unwrap()
                .total_plugin_latency_samples,
            19
        );
    }

    #[test]
    fn latency_revision_tracks_active_slot_identity_even_when_total_is_unchanged() {
        let metrics = BridgeMetrics::default();
        publish_plugin_latency_snapshot(
            &metrics,
            0b0000_0001,
            [11, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            11,
            0,
        );
        let first = metrics.plugin_latency_snapshot().unwrap();

        publish_plugin_latency_snapshot(
            &metrics,
            0b0000_0010,
            [0, 11, 0, 0, 0, 0, 0, 0, 0, 0],
            11,
            0,
        );
        let second = metrics.plugin_latency_snapshot().unwrap();
        assert!(second.revision > first.revision);
        assert_eq!(second.active_mask, 0b0000_0010);
        assert_eq!(second.total_plugin_latency_samples, 11);
        assert_eq!(second.tail_samples, 0);

        publish_plugin_latency_snapshot(
            &metrics,
            0b0000_0010,
            [0, 11, 0, 0, 0, 0, 0, 0, 0, 0],
            11,
            23,
        );
        let tail_changed = metrics.plugin_latency_snapshot().unwrap();
        assert!(tail_changed.revision > second.revision);
        assert_eq!(tail_changed.tail_samples, 23);

        publish_plugin_latency_snapshot(
            &metrics,
            0b0000_0010,
            [0, 11, 0, 0, 0, 0, 0, 0, 0, 0],
            11,
            23,
        );
        assert_eq!(
            metrics.plugin_latency_snapshot().unwrap().revision,
            tail_changed.revision
        );
    }

    #[test]
    fn latency_prefix_is_active_slot_aware_and_saturating() {
        let snapshot = PluginLatencySnapshot {
            revision: 1,
            active_mask: 0b0000_0011,
            slot_latency_samples: [u32::MAX - 4, 10, 99, 0, 0, 0, 0, 0, 0, 0],
            total_plugin_latency_samples: u32::MAX,
            tail_samples: 0,
        };

        assert_eq!(snapshot.prefix_latency_before(0), Some(0));
        assert_eq!(snapshot.prefix_latency_before(1), Some(u32::MAX - 4));
        assert_eq!(snapshot.prefix_latency_before(2), Some(u32::MAX));
        assert_eq!(snapshot.prefix_latency_before(3), Some(u32::MAX));
        assert_eq!(snapshot.prefix_latency_before(MAX_PLUGIN_CHAIN_SLOTS), None);
        assert!(!snapshot.slot_is_active(MAX_PLUGIN_CHAIN_SLOTS));
    }

    #[test]
    fn latency_snapshot_reader_is_bounded_while_publication_is_in_progress() {
        let metrics = BridgeMetrics::default();
        metrics.plugin_latency_sequence.store(1, Ordering::Release);
        assert_eq!(metrics.plugin_latency_snapshot(), None);
    }

    #[test]
    fn latency_snapshot_reader_never_combines_two_worker_publications() {
        let metrics = Arc::new(BridgeMetrics::default());
        let writer_metrics = Arc::clone(&metrics);
        let writer = thread::spawn(move || {
            for iteration in 0..50_000 {
                if iteration & 1 == 0 {
                    publish_plugin_latency_snapshot(
                        &writer_metrics,
                        0b0000_0011,
                        [3, 5, 0, 0, 0, 0, 0, 0, 0, 0],
                        8,
                        13,
                    );
                } else {
                    publish_plugin_latency_snapshot(
                        &writer_metrics,
                        0b0000_0100,
                        [0, 0, 17, 0, 0, 0, 0, 0, 0, 0],
                        17,
                        19,
                    );
                }
            }
        });

        while !writer.is_finished() {
            let Some(snapshot) = metrics.plugin_latency_snapshot() else {
                continue;
            };
            let first = snapshot.active_mask == 0b0000_0011
                && snapshot.slot_latency_samples == [3, 5, 0, 0, 0, 0, 0, 0, 0, 0]
                && snapshot.total_plugin_latency_samples == 8
                && snapshot.tail_samples == 13
                && snapshot.revision & 1 == 1;
            let second = snapshot.active_mask == 0b0000_0100
                && snapshot.slot_latency_samples == [0, 0, 17, 0, 0, 0, 0, 0, 0, 0]
                && snapshot.total_plugin_latency_samples == 17
                && snapshot.tail_samples == 19
                && snapshot.revision & 1 == 0;
            assert_ne!(snapshot.revision, 0);
            assert!(first || second, "torn snapshot: {snapshot:?}");
        }
        writer.join().unwrap();
        let final_snapshot = metrics.plugin_latency_snapshot().unwrap();
        assert_eq!(final_snapshot.revision, 50_000);
        assert_eq!(final_snapshot.active_mask, 0b0000_0100);
        assert_eq!(
            final_snapshot.slot_latency_samples,
            [0, 0, 17, 0, 0, 0, 0, 0, 0, 0]
        );
        assert_eq!(final_snapshot.total_plugin_latency_samples, 17);
        assert_eq!(final_snapshot.tail_samples, 19);
    }

    #[test]
    fn worker_publishes_latency_changed_by_successful_processing() {
        let mut chain = PluginChain::spawn_with_backend_factory(
            || {
                vec![BackendSlot::new(Box::new(MockBackend::new(
                    "dynamic latency",
                    MockProcess::SetMetadata {
                        latency: 73,
                        tail: 31,
                    },
                )))]
            },
            config(),
        )
        .unwrap();
        wait_until(|| chain.control.plugin_latency_snapshot().is_some());
        let initial = chain.control.plugin_latency_snapshot().unwrap();
        assert_eq!(initial.active_mask, 1);
        assert_eq!(initial.total_plugin_latency_samples, 0);
        assert_eq!(initial.tail_samples, 0);

        let input = [0.0; 8];
        assert!(matches!(
            chain.audio.try_submit(&input, &input),
            SubmitStatus::Submitted { .. }
        ));
        let _ = receive(&mut chain.audio, input.len());
        wait_until(|| {
            chain
                .control
                .plugin_latency_snapshot()
                .is_some_and(|snapshot| snapshot.total_plugin_latency_samples == 73)
        });
        let updated = chain.control.plugin_latency_snapshot().unwrap();
        assert!(updated.revision > initial.revision);
        assert_eq!(updated.active_mask, 1);
        assert_eq!(updated.slot_latency_samples[0], 73);
        assert_eq!(updated.total_plugin_latency_samples, 73);
        assert_eq!(updated.tail_samples, 31);
    }

    #[test]
    fn latency_tracks_actual_submitted_block_sizes_and_saturates() {
        let mut chain = PluginChain::spawn_with_backend_factory(
            || {
                let mut backend = MockBackend::new("dynamic latency", MockProcess::Gain(1.0));
                backend.latency = 12;
                vec![BackendSlot::new(Box::new(backend))]
            },
            PluginPrepareConfig {
                sample_rate: 48_000.0,
                max_block_frames: 512,
            },
        )
        .unwrap();
        wait_until(|| chain.control.stats().latency_samples == 12);

        let block_256 = vec![0.0; 256];
        assert!(matches!(
            chain.audio.try_submit(&block_256, &block_256),
            SubmitStatus::Submitted { .. }
        ));
        assert_eq!(chain.audio.stats().latency_samples, 256 + 12);

        let block_512 = vec![0.0; 512];
        assert!(matches!(
            chain.audio.try_submit(&block_512, &block_512),
            SubmitStatus::Submitted { .. }
        ));
        assert_eq!(chain.audio.stats().latency_samples, 512 + 12);

        let mut saturating = PluginChain::spawn_with_backend_factory(
            || {
                let mut backend = MockBackend::new("huge latency", MockProcess::Gain(1.0));
                backend.latency = u32::MAX;
                vec![BackendSlot::new(Box::new(backend))]
            },
            PluginPrepareConfig {
                sample_rate: 48_000.0,
                max_block_frames: 512,
            },
        )
        .unwrap();
        wait_until(|| saturating.control.stats().latency_samples == u32::MAX);
        assert!(matches!(
            saturating.audio.try_submit(&block_512, &block_512),
            SubmitStatus::Submitted { .. }
        ));
        assert_eq!(saturating.audio.stats().latency_samples, u32::MAX);
    }

    #[test]
    fn full_input_queue_is_nonblocking_and_observable() {
        let entered = Arc::new(AtomicBool::new(false));
        let worker_entered = Arc::clone(&entered);
        let mut chain = PluginChain::spawn_with_backend_factory(
            move || {
                let mut backend =
                    MockBackend::new("slow", MockProcess::Slow(Duration::from_millis(100)));
                backend.entered = Some(worker_entered);
                vec![BackendSlot::new(Box::new(backend))]
            },
            config(),
        )
        .unwrap();
        let input = [0.0; 8];
        chain.audio.try_submit(&input, &input);
        wait_until(|| entered.load(Ordering::Acquire));
        let mut saw_full = false;
        for _ in 0..16 {
            if matches!(
                chain.audio.try_submit(&input, &input),
                SubmitStatus::Gap { .. }
            ) {
                saw_full = true;
                break;
            }
        }
        assert!(saw_full);
        let stats = chain.audio.stats();
        assert!(stats.input_overflows > 0);
        assert!(stats.input_gaps > 0);
        assert_eq!(stats.callback_sequences, stats.submitted + stats.input_gaps);
    }

    #[test]
    fn panic_and_nonfinite_output_fault_the_slot_and_pass_dry_audio() {
        for process in [MockProcess::Panic, MockProcess::NonFinite] {
            let mut chain = PluginChain::spawn_with_backend_factory(
                move || {
                    let mut backend = MockBackend::new("broken", process);
                    backend.latency = 9;
                    backend.tail = 7;
                    vec![BackendSlot::new(Box::new(backend))]
                },
                config(),
            )
            .unwrap();
            wait_until(|| chain.control.plugin_latency_snapshot().is_some());
            let initial_latency = chain.control.plugin_latency_snapshot().unwrap();
            assert_eq!(initial_latency.active_mask, 1);
            assert_eq!(initial_latency.total_plugin_latency_samples, 9);
            let input = [0.25; 8];
            chain.audio.try_submit(&input, &input);
            let (left, right) = receive(&mut chain.audio, input.len());
            assert_eq!(&left[..input.len()], &input);
            assert_eq!(&right[..input.len()], &input);
            wait_until(|| {
                chain.audio.stats().faults == 1
                    && chain
                        .control
                        .plugin_latency_snapshot()
                        .is_some_and(|snapshot| snapshot.active_mask == 0)
            });
            assert_eq!(chain.audio.stats().latency_samples, input.len() as u32);
            assert_eq!(chain.audio.stats().tail_samples, 0);
            let faulted_latency = chain.control.plugin_latency_snapshot().unwrap();
            assert!(faulted_latency.revision > initial_latency.revision);
            assert_eq!(faulted_latency.active_mask, 0);
            assert_eq!(
                faulted_latency.slot_latency_samples,
                [0; MAX_PLUGIN_CHAIN_SLOTS]
            );
            assert_eq!(faulted_latency.total_plugin_latency_samples, 0);
            assert_eq!(faulted_latency.tail_samples, 0);
        }
    }

    #[test]
    fn mismatched_completed_block_falls_back_for_the_entire_current_block() {
        let mut chain = PluginChain::spawn_with_backend_factory(
            || {
                vec![BackendSlot::new(Box::new(MockBackend::new(
                    "gain",
                    MockProcess::Gain(2.0),
                )))]
            },
            config(),
        )
        .unwrap();
        let old = [1.0; 32];
        chain.audio.try_submit(&old, &old);
        wait_until(|| chain.audio.stats().completed == 1);

        let current_left = [0.25; 16];
        let current_right = [0.5; 16];
        let mut output_left = [9.0; 16];
        let mut output_right = [9.0; 16];
        let status = chain.audio.process_realtime(
            &current_left,
            &current_right,
            &mut output_left,
            &mut output_right,
        );
        assert!(matches!(
            status,
            RealtimeProcessStatus::Processed {
                source: RealtimeOutputSource::DelayedDry,
                ..
            }
        ));
        assert_eq!(output_left, [0.0; 16]);
        assert_eq!(output_right, [0.0; 16]);
        assert_eq!(chain.audio.stats().frame_mismatches, 1);
    }

    #[test]
    fn realtime_path_never_returns_the_block_submitted_by_that_same_call() {
        let mut chain = PluginChain::spawn_with_backend_factory(
            || {
                vec![BackendSlot::new(Box::new(MockBackend::new(
                    "instant",
                    MockProcess::Gain(2.0),
                )))]
            },
            config(),
        )
        .unwrap();
        let input = [0.5; 8];
        let mut left = [0.0; 8];
        let mut right = [0.0; 8];
        let first = chain
            .audio
            .process_realtime(&input, &input, &mut left, &mut right);
        assert!(matches!(
            first,
            RealtimeProcessStatus::Processed {
                sequence: 0,
                source: RealtimeOutputSource::DelayedDry,
                ..
            }
        ));
        assert_eq!(left, [0.0; 8]);
        wait_until(|| chain.audio.stats().completed == 1);
        let second = chain
            .audio
            .process_realtime(&input, &input, &mut left, &mut right);
        assert!(matches!(second, RealtimeProcessStatus::Processed { .. }));
        assert_eq!(left, [1.0; 8]);
    }

    #[test]
    fn realtime_path_discards_stale_backlog_only_until_exact_preceding_sequence() {
        let mut chain = PluginChain::spawn_with_backend_factory(
            || {
                vec![BackendSlot::new(Box::new(MockBackend::new(
                    "identity",
                    MockProcess::Gain(1.0),
                )))]
            },
            config(),
        )
        .unwrap();
        for value in 1..=4 {
            let input = [value as f32; 8];
            loop {
                if matches!(
                    chain.audio.try_submit(&input, &input),
                    SubmitStatus::Submitted { .. }
                ) {
                    break;
                }
                thread::sleep(Duration::from_millis(1));
            }
        }
        wait_until(|| chain.audio.stats().completed == 4);
        let current = [9.0; 8];
        let mut left = [0.0; 8];
        let mut right = [0.0; 8];
        let status = chain
            .audio
            .process_realtime(&current, &current, &mut left, &mut right);
        assert!(matches!(status, RealtimeProcessStatus::Processed { .. }));
        assert_eq!(left, [4.0; 8]);
        assert_eq!(chain.audio.stats().stale_outputs, 3);
    }

    #[test]
    fn slow_worker_miss_returns_preceding_dry_never_current_dry() {
        let entered = Arc::new(AtomicBool::new(false));
        let worker_entered = Arc::clone(&entered);
        let mut chain = PluginChain::spawn_with_backend_factory(
            move || {
                let mut backend = MockBackend::new(
                    "slow exact bridge",
                    MockProcess::Slow(Duration::from_millis(100)),
                );
                backend.entered = Some(worker_entered);
                vec![BackendSlot::new(Box::new(backend))]
            },
            config(),
        )
        .unwrap();
        let first = [0.25; 8];
        let second = [0.75; 8];
        let mut left = [9.0; 8];
        let mut right = [9.0; 8];

        let prime = chain
            .audio
            .process_realtime(&first, &first, &mut left, &mut right);
        assert!(matches!(
            prime,
            RealtimeProcessStatus::Processed {
                sequence: 0,
                source: RealtimeOutputSource::DelayedDry,
                ..
            }
        ));
        assert_eq!(left, [0.0; 8]);
        wait_until(|| entered.load(Ordering::Acquire));

        let missed = chain
            .audio
            .process_realtime(&second, &second, &mut left, &mut right);
        assert!(matches!(
            missed,
            RealtimeProcessStatus::Processed {
                sequence: 1,
                source: RealtimeOutputSource::DelayedDry,
                ..
            }
        ));
        assert_eq!(left, first);
        assert_eq!(right, first);
        assert_eq!(chain.audio.stats().deadline_misses, 1);
    }

    #[test]
    fn future_output_is_held_until_its_exact_callback_sequence() {
        let (mut audio, _input, mut output) = manual_endpoint(4, 8);
        let first = [1.0; 8];
        let second = [2.0; 8];
        let third = [3.0; 8];
        let mut left = [0.0; 8];
        let mut right = [0.0; 8];

        let _ = audio.process_realtime(&first, &first, &mut left, &mut right);
        output.push(completed_block(2, 8, 20.0)).unwrap();
        let waiting = audio.process_realtime(&second, &second, &mut left, &mut right);
        assert!(matches!(
            waiting,
            RealtimeProcessStatus::Processed {
                sequence: 1,
                source: RealtimeOutputSource::DelayedDry,
                ..
            }
        ));
        assert_eq!(left, first);

        let exact = audio.process_realtime(&third, &third, &mut left, &mut right);
        assert!(matches!(
            exact,
            RealtimeProcessStatus::Processed {
                sequence: 2,
                source: RealtimeOutputSource::Plugin,
                ..
            }
        ));
        assert_eq!(left, [20.0; 8]);
        assert_eq!(audio.stats().future_outputs_held, 1);
    }

    #[test]
    fn attestation_invalid_future_output_stays_silent_when_later_consumed() {
        let (mut audio, _input, mut output) = manual_endpoint(4, 8);
        let first = [1.0; 8];
        let second = [2.0; 8];
        let third = [3.0; 8];
        let mut left = [9.0; 8];
        let mut right = [9.0; 8];

        let _ = audio.process_realtime(&first, &first, &mut left, &mut right);
        let mut future = completed_block(2, 8, 20.0);
        future.latency_attestation = LatencyAttestation::Invalid;
        output.push(future).unwrap();

        let waiting = audio.process_realtime(&second, &second, &mut left, &mut right);
        assert!(matches!(
            waiting,
            RealtimeProcessStatus::Processed {
                sequence: 1,
                source: RealtimeOutputSource::DelayedDry,
                ..
            }
        ));
        assert_eq!(left, first);

        left.fill(9.0);
        right.fill(9.0);
        let exact = audio.process_realtime(&third, &third, &mut left, &mut right);
        assert!(matches!(
            exact,
            RealtimeProcessStatus::Processed {
                sequence: 2,
                source: RealtimeOutputSource::LatencyDrift,
                ..
            }
        ));
        assert_eq!(left, [0.0; 8]);
        assert_eq!(right, [0.0; 8]);
        assert_eq!(audio.stats().future_outputs_held, 1);
        assert_eq!(audio.stats().latency_drift_outputs, 1);
    }

    #[test]
    fn bounded_future_cache_recovers_reordered_completed_blocks() {
        let (mut audio, _input, mut output) = manual_endpoint(4, 8);
        let input = [1.0; 8];
        let mut left = [0.0; 8];
        let mut right = [0.0; 8];
        let _ = audio.process_realtime(&input, &input, &mut left, &mut right);
        let _ = audio.process_realtime(&input, &input, &mut left, &mut right);
        output.push(completed_block(3, 8, 30.0)).unwrap();
        output.push(completed_block(2, 8, 20.0)).unwrap();

        let exact_two = audio.process_realtime(&input, &input, &mut left, &mut right);
        assert!(matches!(
            exact_two,
            RealtimeProcessStatus::Processed {
                sequence: 2,
                source: RealtimeOutputSource::Plugin,
                ..
            }
        ));
        assert_eq!(left, [20.0; 8]);

        let exact_three = audio.process_realtime(&input, &input, &mut left, &mut right);
        assert!(matches!(
            exact_three,
            RealtimeProcessStatus::Processed {
                sequence: 3,
                source: RealtimeOutputSource::Plugin,
                ..
            }
        ));
        assert_eq!(left, [30.0; 8]);
        assert_eq!(audio.stats().future_outputs_held, 1);
        assert_eq!(audio.stats().future_output_overflows, 0);
    }

    #[test]
    fn receiver_drops_stale_blocks_but_stops_at_exact_sequence() {
        let (mut audio, _input, mut output) = manual_endpoint(8, 8);
        for value in 1..=3 {
            let input = [value as f32; 8];
            assert!(matches!(
                audio.try_submit(&input, &input),
                SubmitStatus::Submitted { .. }
            ));
        }
        for sequence in 1..=4 {
            output
                .push(completed_block(sequence, 8, sequence as f32 * 10.0))
                .unwrap();
        }
        let current = [4.0; 8];
        let mut left = [0.0; 8];
        let mut right = [0.0; 8];
        let exact_three = audio.process_realtime(&current, &current, &mut left, &mut right);
        assert!(matches!(
            exact_three,
            RealtimeProcessStatus::Processed {
                sequence: 3,
                source: RealtimeOutputSource::Plugin,
                ..
            }
        ));
        assert_eq!(left, [30.0; 8]);
        assert_eq!(audio.stats().stale_outputs, 2);

        let next = [5.0; 8];
        let exact_four = audio.process_realtime(&next, &next, &mut left, &mut right);
        assert!(matches!(
            exact_four,
            RealtimeProcessStatus::Processed {
                sequence: 4,
                source: RealtimeOutputSource::Plugin,
                ..
            }
        ));
        assert_eq!(left, [40.0; 8]);
    }

    #[test]
    fn current_input_gap_does_not_hide_ready_expected_output() {
        let (mut audio, _input, mut output) = manual_endpoint(1, 8);
        let first = [1.0; 8];
        let second = [2.0; 8];
        let mut left = [0.0; 8];
        let mut right = [0.0; 8];
        let _ = audio.process_realtime(&first, &first, &mut left, &mut right);
        output.push(completed_block(1, 8, 9.0)).unwrap();

        let status = audio.process_realtime(&second, &second, &mut left, &mut right);
        assert!(matches!(
            status,
            RealtimeProcessStatus::Processed {
                sequence: 1,
                submit: SubmitStatus::Gap { sequence: 2 },
                source: RealtimeOutputSource::Plugin,
                ..
            }
        ));
        assert_eq!(left, [9.0; 8]);
        let stats = audio.stats();
        assert_eq!(stats.callback_sequences, 2);
        assert_eq!(stats.submitted, 1);
        assert_eq!(stats.input_gaps, 1);
    }

    #[test]
    fn realtime_events_are_applied_only_with_their_audio_block_and_midi_is_clamped() {
        let last_midi = Arc::new(AtomicU64::new(0));
        let worker_last_midi = Arc::clone(&last_midi);
        let mut chain = PluginChain::spawn_with_backend_factory(
            move || {
                let mut backend = MockBackend::new("inline events", MockProcess::ParameterGain);
                backend.last_midi = Some(worker_last_midi);
                vec![BackendSlot::new(Box::new(backend))]
            },
            config(),
        )
        .unwrap();
        let input = [1.0; 8];

        assert!(matches!(
            chain.audio.try_submit(&input, &input),
            SubmitStatus::Submitted { sequence: 1 }
        ));
        assert!(chain.audio.try_set_parameter(0, 7, 0.25));
        assert!(
            chain
                .audio
                .try_send_midi(Some(0), MidiMessage::new([0x90, 64, 100], 99))
        );
        assert!(matches!(
            chain.audio.try_submit(&input, &input),
            SubmitStatus::Submitted { sequence: 2 }
        ));

        let (first, _) = receive(&mut chain.audio, input.len());
        let (second, _) = receive(&mut chain.audio, input.len());
        assert_eq!(&first[..8], &[0.0; 8]);
        assert_eq!(&second[..8], &[0.25; 8]);
        let midi = last_midi.load(Ordering::Acquire);
        assert_eq!(midi & 0x00ff_ffff, 0x0064_4090);
        assert_eq!(midi >> 24, 7);

        let one = [1.0; 1];
        assert!(
            chain
                .audio
                .try_send_midi(Some(0), MidiMessage::new([0x80, 64, 0], 512))
        );
        assert!(matches!(
            chain.audio.try_submit(&one, &one),
            SubmitStatus::Submitted { sequence: 3 }
        ));
        let _ = receive(&mut chain.audio, 1);
        let midi = last_midi.load(Ordering::Acquire);
        assert_eq!(midi & 0x00ff_ffff, 0x0000_4080);
        assert_eq!(midi >> 24, 0);
    }

    #[test]
    fn inline_event_capacity_and_gap_drops_are_observable() {
        let (mut audio, mut input_consumer, _output) = manual_endpoint(1, 8);
        let input = [0.0; 8];
        let mut left = [0.0; 8];
        let mut right = [0.0; 8];
        let _ = audio.process_realtime(&input, &input, &mut left, &mut right);
        for id in 0..MAX_RT_EVENTS_PER_BLOCK {
            assert!(audio.try_set_parameter(0, id as u32, 0.5));
        }
        assert!(!audio.try_set_parameter(0, u32::MAX, 0.5));
        let status = audio.process_realtime(&input, &input, &mut left, &mut right);
        assert!(matches!(
            status,
            RealtimeProcessStatus::Processed {
                submit: SubmitStatus::Gap { sequence: 2 },
                ..
            }
        ));
        let stats = audio.stats();
        assert_eq!(stats.inline_event_overflows, 1);
        assert_eq!(stats.rt_overflows, 1);
        assert_eq!(stats.dropped_rt_events, MAX_RT_EVENTS_PER_BLOCK as u64);

        let first_block = input_consumer.pop().unwrap();
        assert_eq!(first_block.sequence, 1);
        let next = audio.process_realtime(&input, &input, &mut left, &mut right);
        assert!(matches!(
            next,
            RealtimeProcessStatus::Processed {
                submit: SubmitStatus::Submitted { sequence: 3 },
                ..
            }
        ));
        let next_block = input_consumer.pop().unwrap();
        assert_eq!(next_block.sequence, 3);
        assert_eq!(next_block.rt_event_count, 0, "gap events must not run late");
    }

    #[test]
    fn reliable_parameter_edit_admission_is_exactly_sixteen_until_receipts_are_popped() {
        let mut chain = PluginChain::spawn_with_backend_factory(
            || {
                vec![BackendSlot::new(Box::new(MockBackend::new(
                    "reliable edits",
                    MockProcess::Gain(1.0),
                )))]
            },
            config(),
        )
        .unwrap();
        for index in 0..MAX_OUTSTANDING_PARAMETER_EDITS {
            assert!(chain.audio.try_set_parameter_tagged(
                0,
                index,
                index as f32 / MAX_OUTSTANDING_PARAMETER_EDITS as f32,
                ParameterEditId::new(u64::from(index) + 1).unwrap(),
            ));
        }
        assert!(!chain.audio.try_set_parameter_tagged(
            0,
            99,
            0.5,
            ParameterEditId::new(99).unwrap(),
        ));
        assert_eq!(
            chain.control.outstanding_parameter_edits(),
            MAX_OUTSTANDING_PARAMETER_EDITS
        );

        let samples = [1.0; 8];
        assert!(matches!(
            chain.audio.try_submit(&samples, &samples),
            SubmitStatus::Submitted { .. }
        ));
        for index in 0..MAX_OUTSTANDING_PARAMETER_EDITS {
            assert!(matches!(
                wait_for_parameter_edit_receipt(&mut chain.control),
                ParameterEditReceipt::Applied { edit_id, slot: 0, id, .. }
                    if edit_id.get() == u64::from(index) + 1 && id == index
            ));
        }
        assert_eq!(chain.control.outstanding_parameter_edits(), 0);
        assert!(chain.audio.try_set_parameter_tagged(
            0,
            100,
            0.5,
            ParameterEditId::new(100).unwrap(),
        ));
    }

    #[test]
    fn reliable_parameter_edit_gap_and_pending_epoch_reset_return_failures() {
        let entered = Arc::new(AtomicBool::new(false));
        let entered_worker = Arc::clone(&entered);
        let mut chain = PluginChain::spawn_with_backend_factory(
            move || {
                let mut backend = MockBackend::new(
                    "slow reliable edits",
                    MockProcess::Slow(Duration::from_millis(80)),
                );
                backend.entered = Some(entered_worker);
                vec![BackendSlot::new(Box::new(backend))]
            },
            config(),
        )
        .unwrap();
        let samples = [0.0; 8];
        assert!(matches!(
            chain.audio.try_submit(&samples, &samples),
            SubmitStatus::Submitted { .. }
        ));
        wait_until(|| entered.load(Ordering::Acquire));
        for _ in 0..LEGACY_QUEUE_ADMISSION {
            assert!(matches!(
                chain.audio.try_submit(&samples, &samples),
                SubmitStatus::Submitted { .. }
            ));
        }
        assert!(chain.audio.try_set_parameter_tagged(
            0,
            1,
            0.25,
            ParameterEditId::new(201).unwrap(),
        ));
        assert!(matches!(
            chain.audio.try_submit(&samples, &samples),
            SubmitStatus::Gap { .. }
        ));
        assert!(matches!(
            wait_for_parameter_edit_receipt(&mut chain.control),
            ParameterEditReceipt::Failed {
                edit_id,
                reason: ParameterEditFailureReason::InputGap,
                ..
            } if edit_id.get() == 201
        ));

        assert!(chain.audio.try_set_parameter_tagged(
            0,
            2,
            0.5,
            ParameterEditId::new(202).unwrap(),
        ));
        assert!(chain.audio.set_epoch(2));
        assert!(matches!(
            wait_for_parameter_edit_receipt(&mut chain.control),
            ParameterEditReceipt::Failed {
                edit_id,
                reason: ParameterEditFailureReason::EpochReset,
                ..
            } if edit_id.get() == 202
        ));
    }

    #[test]
    fn reliable_parameter_edit_worker_mismatch_reject_and_panic_are_terminal() {
        let entered = Arc::new(AtomicBool::new(false));
        let entered_worker = Arc::clone(&entered);
        let mut chain = PluginChain::spawn_with_backend_factory(
            move || {
                let mut backend = MockBackend::new(
                    "worker outcomes",
                    MockProcess::Slow(Duration::from_millis(50)),
                );
                backend.entered = Some(entered_worker);
                vec![BackendSlot::new(Box::new(backend))]
            },
            config(),
        )
        .unwrap();
        let samples = [0.0; 8];
        assert!(matches!(
            chain.audio.try_submit(&samples, &samples),
            SubmitStatus::Submitted { .. }
        ));
        wait_until(|| entered.load(Ordering::Acquire));
        assert!(chain.audio.try_set_parameter_tagged(
            0,
            3,
            0.3,
            ParameterEditId::new(301).unwrap(),
        ));
        assert!(matches!(
            chain.audio.try_submit(&samples, &samples),
            SubmitStatus::Submitted { .. }
        ));
        assert!(chain.audio.set_epoch(2));
        assert!(matches!(
            wait_for_parameter_edit_receipt(&mut chain.control),
            ParameterEditReceipt::Failed {
                edit_id,
                reason: ParameterEditFailureReason::WorkerEpochMismatch,
                ..
            } if edit_id.get() == 301
        ));

        assert!(chain.audio.try_set_parameter_tagged(
            0,
            u32::MAX,
            0.4,
            ParameterEditId::new(302).unwrap(),
        ));
        assert!(matches!(
            chain.audio.try_submit(&samples, &samples),
            SubmitStatus::Submitted { .. }
        ));
        assert!(matches!(
            wait_for_parameter_edit_receipt(&mut chain.control),
            ParameterEditReceipt::Failed {
                edit_id,
                reason: ParameterEditFailureReason::BackendRejected,
                ..
            } if edit_id.get() == 302
        ));

        assert!(chain.audio.try_set_parameter_tagged(
            0,
            u32::MAX - 1,
            0.5,
            ParameterEditId::new(303).unwrap(),
        ));
        assert!(matches!(
            chain.audio.try_submit(&samples, &samples),
            SubmitStatus::Submitted { .. }
        ));
        assert!(matches!(
            wait_for_parameter_edit_receipt(&mut chain.control),
            ParameterEditReceipt::Failed {
                edit_id,
                reason: ParameterEditFailureReason::BackendPanicked,
                ..
            } if edit_id.get() == 303
        ));
        assert!(chain.control.stats().faults >= 1);
    }

    #[test]
    fn reliable_parameter_edit_readback_and_receipt_ring_ignore_runtime_event_overflow() {
        let mut chain = PluginChain::spawn_with_backend_factory(
            || {
                vec![BackendSlot::new(Box::new(MockBackend::new(
                    "readback",
                    MockProcess::Gain(1.0),
                )))]
            },
            config(),
        )
        .unwrap();
        let mut accepted = 0_u64;
        while accepted < 96 {
            if chain
                .control
                .query_parameter_tagged(0, 0, 10_000 + accepted)
            {
                accepted += 1;
            } else {
                thread::sleep(Duration::from_millis(1));
            }
        }
        wait_until(|| chain.control.stats().event_overflows > 0);

        let samples = [0.0; 8];
        for (edit, id) in [(401, 7), (402, u32::MAX - 2), (403, u32::MAX - 3)] {
            assert!(chain.audio.try_set_parameter_tagged(
                0,
                id,
                0.625,
                ParameterEditId::new(edit).unwrap(),
            ));
        }
        assert!(matches!(
            chain.audio.try_submit(&samples, &samples),
            SubmitStatus::Submitted { .. }
        ));
        assert!(matches!(
            wait_for_parameter_edit_receipt(&mut chain.control),
            ParameterEditReceipt::Applied {
                edit_id,
                effective: 0.625,
                readback_confirmed: true,
                ..
            } if edit_id.get() == 401
        ));
        for expected in [402, 403] {
            assert!(matches!(
                wait_for_parameter_edit_receipt(&mut chain.control),
                ParameterEditReceipt::Applied {
                    edit_id,
                    effective: 0.625,
                    readback_confirmed: false,
                    ..
                } if edit_id.get() == expected
            ));
        }
        assert_eq!(chain.control.outstanding_parameter_edits(), 0);
    }

    #[test]
    fn epoch_switch_flushes_future_cache_and_reprimes_with_silence() {
        let (mut audio, _input, mut output) = manual_endpoint(4, 8);
        let first = [1.0; 8];
        let second = [2.0; 8];
        let mut left = [0.0; 8];
        let mut right = [0.0; 8];

        let _ = audio.process_realtime(&first, &first, &mut left, &mut right);
        output.push(completed_block(2, 8, 20.0)).unwrap();
        let _ = audio.process_realtime(&second, &second, &mut left, &mut right);
        assert_eq!(audio.stats().future_outputs_held, 1);

        assert!(audio.set_epoch(2));
        assert_eq!(audio.epoch(), 2);
        let status = audio.process_realtime(&second, &second, &mut left, &mut right);
        assert!(matches!(
            status,
            RealtimeProcessStatus::Processed {
                sequence: 0,
                submit: SubmitStatus::Submitted { sequence: 1 },
                source: RealtimeOutputSource::DelayedDry,
                ..
            }
        ));
        assert_eq!(left, [0.0; 8]);
        let stats = audio.stats();
        assert_eq!(stats.current_epoch, 2);
        assert_eq!(stats.epoch_resets, 1);
        assert_eq!(stats.dropped_old_epoch_outputs, 1);
    }

    #[test]
    fn output_matcher_requires_both_epoch_and_sequence() {
        let (mut audio, _input, mut output) = manual_endpoint(4, 8);
        assert!(audio.set_epoch(2));
        let input = [1.0; 8];
        let mut left = [0.0; 8];
        let mut right = [0.0; 8];
        let _ = audio.process_realtime(&input, &input, &mut left, &mut right);

        output
            .push(completed_block_in_epoch(1, 1, 8, 10.0))
            .unwrap();
        output
            .push(completed_block_in_epoch(2, 1, 8, 20.0))
            .unwrap();
        let status = audio.process_realtime(&input, &input, &mut left, &mut right);
        assert!(matches!(
            status,
            RealtimeProcessStatus::Processed {
                sequence: 1,
                source: RealtimeOutputSource::Plugin,
                ..
            }
        ));
        assert_eq!(left, [20.0; 8]);
        assert_eq!(audio.stats().dropped_old_epoch_outputs, 1);
    }

    #[test]
    fn queued_old_epoch_midi_and_slow_output_are_rejected() {
        let entered = Arc::new(AtomicBool::new(false));
        let note_on_count = Arc::new(AtomicU64::new(0));
        let reset_count = Arc::new(AtomicU64::new(0));
        let worker_entered = Arc::clone(&entered);
        let worker_note_on_count = Arc::clone(&note_on_count);
        let worker_reset_count = Arc::clone(&reset_count);
        let mut chain = PluginChain::spawn_with_backend_factory(
            move || {
                let mut backend =
                    MockBackend::new("slow epoch", MockProcess::Slow(Duration::from_millis(80)));
                backend.entered = Some(worker_entered);
                backend.note_on_count = Some(worker_note_on_count);
                backend.reset_count = Some(worker_reset_count);
                vec![BackendSlot::new(Box::new(backend))]
            },
            config(),
        )
        .unwrap();

        let old = [7.0; 8];
        assert!(matches!(
            chain.audio.try_submit(&old, &old),
            SubmitStatus::Submitted { sequence: 1 }
        ));
        wait_until(|| entered.load(Ordering::Acquire));
        assert!(
            chain
                .audio
                .try_send_midi(None, MidiMessage::new([0x90, 64, 100], 0))
        );
        assert!(matches!(
            chain.audio.try_submit(&old, &old),
            SubmitStatus::Submitted { sequence: 2 }
        ));

        assert!(chain.audio.set_epoch(2));
        let current = [2.0; 8];
        assert!(matches!(
            chain.audio.try_submit(&current, &current),
            SubmitStatus::Submitted { sequence: 1 }
        ));
        let (left, right) = receive(&mut chain.audio, current.len());
        assert_eq!(&left[..current.len()], &current);
        assert_eq!(&right[..current.len()], &current);
        assert_eq!(note_on_count.load(Ordering::Acquire), 0);
        assert_eq!(reset_count.load(Ordering::Acquire), 1);
        let stats = chain.audio.stats();
        assert_eq!(stats.worker_epoch_resets, 1);
        assert!(stats.dropped_old_epoch_inputs >= 1);
        assert!(stats.dropped_old_epoch_outputs >= 1);
    }

    #[test]
    fn backend_tail_is_cleared_before_new_epoch_audio() {
        let reset_count = Arc::new(AtomicU64::new(0));
        let worker_reset_count = Arc::clone(&reset_count);
        let mut chain = PluginChain::spawn_with_backend_factory(
            move || {
                let mut backend = MockBackend::new("tail", MockProcess::Tail);
                backend.reset_count = Some(worker_reset_count);
                vec![BackendSlot::new(Box::new(backend))]
            },
            config(),
        )
        .unwrap();

        let impulse = [1.0];
        chain.audio.try_submit(&impulse, &impulse);
        let (first, _) = receive(&mut chain.audio, 1);
        assert_eq!(first[0], 1.0);

        assert!(chain.audio.set_epoch(9));
        let silence = [0.0];
        chain.audio.try_submit(&silence, &silence);
        let (after_reset, _) = receive(&mut chain.audio, 1);
        assert_eq!(after_reset[0], 0.0);
        assert_eq!(reset_count.load(Ordering::Acquire), 1);
    }

    #[test]
    fn reset_failure_faults_only_the_slot_and_reports_it() {
        let mut chain = PluginChain::spawn_with_backend_factory(
            || {
                let mut backend = MockBackend::new("reset failure", MockProcess::Gain(2.0));
                backend.reset_fails = true;
                vec![BackendSlot::new(Box::new(backend))]
            },
            config(),
        )
        .unwrap();
        assert!(chain.audio.set_epoch(2));
        let input = [0.25; 8];
        chain.audio.try_submit(&input, &input);
        let (left, right) = receive(&mut chain.audio, input.len());
        assert_eq!(&left[..input.len()], &input);
        assert_eq!(&right[..input.len()], &input);

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut reset_fault = None;
        while Instant::now() < deadline && reset_fault.is_none() {
            match chain.control.try_next_event() {
                Some(RuntimeEvent::SlotFault { message, .. })
                    if message.contains("transport epoch 2 reset failed") =>
                {
                    reset_fault = Some(message);
                }
                Some(_) | None => thread::sleep(Duration::from_millis(1)),
            }
        }
        assert!(reset_fault.is_some());
        let stats = chain.control.stats();
        assert_eq!(stats.reset_faults, 1);
        assert_eq!(stats.faults, 1);
    }

    #[test]
    fn epoch_and_sequence_wrap_skip_only_reserved_epoch_zero() {
        let (mut audio, mut input, _output) = manual_endpoint(4, 8);
        assert!(!audio.set_epoch(0));
        assert_eq!(audio.epoch(), INITIAL_TRANSPORT_EPOCH);
        assert!(audio.set_epoch(u64::MAX));
        assert_eq!(audio.reset_epoch(), INITIAL_TRANSPORT_EPOCH);
        assert_eq!(audio.stats().epoch_resets, 2);

        audio.next_sequence = u64::MAX;
        let samples = [0.0; 8];
        assert!(matches!(
            audio.try_submit(&samples, &samples),
            SubmitStatus::Submitted { sequence: u64::MAX }
        ));
        assert!(matches!(
            audio.try_submit(&samples, &samples),
            SubmitStatus::Submitted { sequence: 0 }
        ));
        let before_wrap = input.pop().unwrap();
        let after_wrap = input.pop().unwrap();
        assert_eq!(before_wrap.epoch, INITIAL_TRANSPORT_EPOCH);
        assert_eq!(before_wrap.sequence, u64::MAX);
        assert_eq!(after_wrap.epoch, INITIAL_TRANSPORT_EPOCH);
        assert_eq!(after_wrap.sequence, 0);
    }

    #[test]
    fn wrapping_sequence_order_distinguishes_stale_exact_and_future() {
        assert_eq!(sequence_order(u64::MAX, 0), std::cmp::Ordering::Less);
        assert_eq!(sequence_order(0, 0), std::cmp::Ordering::Equal);
        assert_eq!(sequence_order(0, u64::MAX), std::cmp::Ordering::Greater);
    }

    #[test]
    fn full_admin_queue_cannot_deadlock_worker_guard_drop() {
        let entered = Arc::new(AtomicBool::new(false));
        let worker_entered = Arc::clone(&entered);
        let mut chain = PluginChain::spawn_with_backend_factory(
            move || {
                let mut backend =
                    MockBackend::new("slow admin", MockProcess::Slow(Duration::from_millis(300)));
                backend.entered = Some(worker_entered);
                vec![BackendSlot::new(Box::new(backend))]
            },
            config(),
        )
        .unwrap();
        let input = [0.0; 8];
        chain.audio.try_submit(&input, &input);
        wait_until(|| entered.load(Ordering::Acquire));
        while chain.control.set_slot_config(0, SlotConfig::default()) {}
        let PluginChain {
            audio,
            control,
            guard,
        } = chain;
        let started = Instant::now();
        drop(guard);
        assert!(started.elapsed() < Duration::from_millis(100));
        assert_eq!(control.stats().detached_workers, 1);
        drop(control);
        drop(audio);
    }

    #[test]
    fn explicit_shutdown_joins_a_responsive_worker_within_timeout() {
        let chain = PluginChain::spawn_with_backend_factory(
            || {
                vec![BackendSlot::new(Box::new(MockBackend::new(
                    "responsive",
                    MockProcess::Gain(1.0),
                )))]
            },
            config(),
        )
        .unwrap();
        let PluginChain {
            audio,
            control,
            guard,
        } = chain;
        drop(audio);
        drop(control);
        assert_eq!(
            guard.shutdown_blocking(Duration::from_secs(1)),
            ShutdownOutcome::Joined
        );
    }

    #[test]
    fn explicit_shutdown_timeout_detaches_a_stuck_worker() {
        let entered = Arc::new(AtomicBool::new(false));
        let worker_entered = Arc::clone(&entered);
        let mut chain = PluginChain::spawn_with_backend_factory(
            move || {
                let mut backend =
                    MockBackend::new("stuck", MockProcess::Slow(Duration::from_millis(300)));
                backend.entered = Some(worker_entered);
                vec![BackendSlot::new(Box::new(backend))]
            },
            config(),
        )
        .unwrap();
        let input = [0.0; 8];
        chain.audio.try_submit(&input, &input);
        wait_until(|| entered.load(Ordering::Acquire));
        let PluginChain {
            audio,
            control,
            guard,
        } = chain;
        let started = Instant::now();
        assert_eq!(
            guard.shutdown_blocking(Duration::from_millis(10)),
            ShutdownOutcome::TimedOutDetached
        );
        assert!(started.elapsed() < Duration::from_millis(100));
        assert_eq!(control.stats().shutdown_timeouts, 1);
        assert_eq!(control.stats().detached_workers, 1);
        drop(control);
        drop(audio);
    }

    #[test]
    fn parameter_catalog_is_paged_tagged_and_string_bounded() {
        let mut chain = PluginChain::spawn_with_backend_factory(
            || {
                let mut backend = MockBackend::new("catalog", MockProcess::Gain(1.0));
                backend.parameter_catalog_revision = 9;
                backend.parameter_catalog = (0..70).map(parameter_descriptor).collect();
                backend.parameter_catalog[0].name = "界".repeat(100);
                backend.parameter_catalog[0].unit = "单位".repeat(100);
                backend.parameter_catalog[0].current_normalized = f32::NAN;
                backend.parameter_catalog[0].default_normalized = Some(4.0);
                vec![BackendSlot::new(Box::new(backend))]
            },
            config(),
        )
        .unwrap();

        assert!(!chain.control.request_parameter_page(0, 0, 0, 64));
        assert!(!chain.control.request_parameter_page(0, 1, 0, 0));
        assert!(!chain.control.request_parameter_page(0, 1, 0, 65));
        assert!(!chain.control.request_parameter_page(
            0,
            1,
            MAX_PLUGIN_PARAMETER_CATALOG_ITEMS as u32,
            1,
        ));
        assert!(chain.control.request_parameter_page(0, 41, 0, 64));
        let first = wait_for_event(&mut chain.control, |event| {
            matches!(
                event,
                RuntimeEvent::ParameterCatalogPage { request_id: 41, .. }
            )
        });
        let RuntimeEvent::ParameterCatalogPage {
            cursor,
            next_cursor,
            done,
            catalog_revision,
            items,
            ..
        } = first
        else {
            unreachable!()
        };
        assert_eq!(cursor, 0);
        assert_eq!(next_cursor, Some(64));
        assert!(!done);
        assert_eq!(catalog_revision, 9);
        assert_eq!(items.len(), 64);
        assert!(items[0].name.len() <= MAX_PLUGIN_PARAMETER_NAME_BYTES);
        assert!(items[0].unit.len() <= MAX_PLUGIN_PARAMETER_UNIT_BYTES);
        assert_eq!(items[0].current_normalized, 0.0);
        assert_eq!(items[0].default_normalized, Some(1.0));
        assert_eq!(items[63].id, 63);

        assert!(chain.control.request_parameter_page(0, 42, 64, 64));
        let mismatch = wait_for_event(&mut chain.control, |event| {
            matches!(
                event,
                RuntimeEvent::ParameterCommandFailed { request_id: 42, .. }
            )
        });
        assert!(matches!(
            mismatch,
            RuntimeEvent::ParameterCommandFailed {
                request_id: 42,
                command: PluginParameterCommand::Catalog,
                ..
            }
        ));

        assert!(chain.control.request_parameter_page(0, 41, 64, 64));
        let second = wait_for_event(&mut chain.control, |event| {
            matches!(
                event,
                RuntimeEvent::ParameterCatalogPage { request_id: 41, .. }
            )
        });
        let RuntimeEvent::ParameterCatalogPage {
            cursor,
            next_cursor,
            done,
            items,
            ..
        } = second
        else {
            unreachable!()
        };
        assert_eq!(cursor, 64);
        assert_eq!(next_cursor, None);
        assert!(done);
        assert_eq!(items.len(), 6);
        assert_eq!(items[0].id, 64);
    }

    #[test]
    fn complete_catalog_paging_enumerates_backend_metadata_only_once() {
        let getters = Arc::new(AtomicU64::new(0));
        let worker_getters = Arc::clone(&getters);
        let mut chain = PluginChain::spawn_with_backend_factory(
            move || {
                let mut backend = MockBackend::new("large catalog", MockProcess::Gain(1.0));
                backend.parameter_catalog = (0..MAX_PLUGIN_PARAMETER_CATALOG_ITEMS as u32)
                    .map(parameter_descriptor)
                    .collect();
                backend.parameter_catalog_revision = 77;
                backend.parameter_snapshot_getters = Some(worker_getters);
                vec![BackendSlot::new(Box::new(backend))]
            },
            config(),
        )
        .unwrap();

        let request_id = 7001;
        for cursor in (0..MAX_PLUGIN_PARAMETER_CATALOG_ITEMS).step_by(64) {
            assert!(
                chain
                    .control
                    .request_parameter_page(0, request_id, cursor as u32, 64,)
            );
            let page = wait_for_event(&mut chain.control, |event| {
                matches!(
                    event,
                    RuntimeEvent::ParameterCatalogPage {
                        request_id: 7001,
                        cursor: event_cursor,
                        ..
                    } if usize::try_from(*event_cursor).ok() == Some(cursor)
                )
            });
            assert!(matches!(
                page,
                RuntimeEvent::ParameterCatalogPage {
                    catalog_revision: 77,
                    items,
                    done,
                    ..
                } if items.len() == 64
                    && done == (cursor + 64 == MAX_PLUGIN_PARAMETER_CATALOG_ITEMS)
            ));
        }
        assert_eq!(
            getters.load(Ordering::Acquire),
            MAX_PLUGIN_PARAMETER_CATALOG_ITEMS as u64
        );

        assert!(chain.control.request_parameter_page(0, request_id, 64, 64));
        let no_snapshot = wait_for_event(&mut chain.control, |event| {
            matches!(
                event,
                RuntimeEvent::ParameterCommandFailed {
                    request_id: 7001,
                    command: PluginParameterCommand::Catalog,
                    ..
                }
            )
        });
        assert!(matches!(
            no_snapshot,
            RuntimeEvent::ParameterCommandFailed { .. }
        ));
        assert_eq!(
            getters.load(Ordering::Acquire),
            MAX_PLUGIN_PARAMETER_CATALOG_ITEMS as u64
        );
    }

    #[test]
    fn tagged_parameter_set_and_query_keep_exact_fifo_identity() {
        let mut chain = PluginChain::spawn_with_backend_factory(
            || {
                vec![BackendSlot::new(Box::new(MockBackend::new(
                    "tagged parameters",
                    MockProcess::Gain(1.0),
                )))]
            },
            config(),
        )
        .unwrap();

        assert!(!chain.control.set_parameter_tagged(0, 7, 0.1, 0));
        assert!(!chain.control.query_parameter_tagged(0, 7, 0));
        assert!(chain.control.set_parameter_tagged(0, 7, 0.25, 11));
        assert!(chain.control.query_parameter_tagged(0, 7, 12));
        assert!(chain.control.set_parameter(0, 7, 0.75));
        assert!(chain.control.query_parameter(0, 7));

        let mut parameter_events = Vec::new();
        while parameter_events.len() < 3 {
            let event = wait_for_event(&mut chain.control, |event| {
                matches!(
                    event,
                    RuntimeEvent::ParameterSetAck { .. } | RuntimeEvent::ParameterValue { .. }
                )
            });
            parameter_events.push(event);
        }
        assert!(matches!(
            parameter_events[0],
            RuntimeEvent::ParameterSetAck {
                request_id: 11,
                id: 7,
                value: 0.25,
                ..
            }
        ));
        assert!(matches!(
            parameter_events[1],
            RuntimeEvent::ParameterValue {
                request_id: 12,
                id: 7,
                value: 0.25,
                ..
            }
        ));
        assert!(matches!(
            parameter_events[2],
            RuntimeEvent::ParameterValue {
                request_id: 0,
                id: 7,
                value: 0.75,
                ..
            }
        ));
    }

    #[test]
    fn rejected_parameter_command_is_tagged_without_faulting_the_slot() {
        let mut chain = PluginChain::spawn_with_backend_factory(
            || {
                vec![BackendSlot::new(Box::new(MockBackend::new(
                    "command rejection",
                    MockProcess::Gain(1.0),
                )))]
            },
            config(),
        )
        .unwrap();

        assert!(chain.control.set_parameter_tagged(0, u32::MAX, 0.5, 51));
        let rejected = wait_for_event(&mut chain.control, |event| {
            matches!(
                event,
                RuntimeEvent::ParameterCommandFailed { request_id: 51, .. }
            )
        });
        assert!(matches!(
            rejected,
            RuntimeEvent::ParameterCommandFailed {
                request_id: 51,
                command: PluginParameterCommand::Set,
                id: Some(u32::MAX),
                ..
            }
        ));
        assert_eq!(chain.control.stats().faults, 0);

        assert!(chain.control.set_parameter_tagged(0, 7, 0.375, 52));
        let ack = wait_for_event(&mut chain.control, |event| {
            matches!(event, RuntimeEvent::ParameterSetAck { request_id: 52, .. })
        });
        assert!(matches!(
            ack,
            RuntimeEvent::ParameterSetAck {
                request_id: 52,
                value: 0.375,
                ..
            }
        ));
        assert_eq!(chain.control.stats().faults, 0);

        assert!(chain.audio.try_set_parameter(0, u32::MAX, 0.5));
        let samples = [0.0; 8];
        assert!(matches!(
            chain.audio.try_submit(&samples, &samples),
            SubmitStatus::Submitted { .. }
        ));
        let _ = receive(&mut chain.audio, samples.len());
        let realtime_rejection = wait_for_event(&mut chain.control, |event| {
            matches!(
                event,
                RuntimeEvent::ParameterCommandFailed {
                    request_id: 0,
                    command: PluginParameterCommand::Set,
                    id: Some(u32::MAX),
                    ..
                }
            )
        });
        assert!(matches!(
            realtime_rejection,
            RuntimeEvent::ParameterCommandFailed {
                request_id: 0,
                command: PluginParameterCommand::Set,
                id: Some(u32::MAX),
                ..
            }
        ));
        assert_eq!(chain.control.stats().faults, 0);
    }

    #[test]
    fn public_slot_operations_fail_fast_at_the_manifest_boundary() {
        let (admin, _admin_rx) = mpsc::sync_channel(16);
        let (_event_tx, events) = RingBuffer::new(16);
        let (_parameter_edit_receipt_tx, parameter_edit_receipts) =
            RingBuffer::new(MAX_OUTSTANDING_PARAMETER_EDITS as usize);
        let metrics = Arc::new(BridgeMetrics::default());
        let control = PluginChainControl {
            admin,
            events,
            metrics,
            manifest: PluginEndpointManifest::unknown_for_slots(1).unwrap(),
            parameter_edit_receipts,
            parameter_edit_admission: Arc::new(AtomicU32::new(0)),
        };
        assert!(control.set_parameter(0, 1, 0.5));
        assert!(!control.set_parameter(1, 1, 0.5));
        assert!(!control.set_parameter_tagged(1, 1, 0.5, 1));
        assert!(!control.query_parameter(1, 1));
        assert!(!control.query_parameter_tagged(1, 1, 1));
        assert!(!control.request_parameter_page(1, 1, 0, 1));
        assert!(!control.set_slot_config(1, SlotConfig::default()));
        assert!(!control.request_state(1));
        assert!(!control.load_state(1, vec![1]));

        let (mut audio, _input, _output) = manual_endpoint(1, 8);
        audio.manifest = PluginEndpointManifest::unknown_for_slots(1).unwrap();
        assert!(audio.try_set_parameter(0, 1, 0.5));
        assert!(!audio.try_set_parameter(1, 1, 0.5));
        assert!(audio.try_send_midi(Some(0), MidiMessage::new([0x90, 60, 100], 0)));
        assert!(!audio.try_send_midi(Some(1), MidiMessage::new([0x90, 60, 100], 0)));
    }

    #[test]
    fn oversized_catalog_is_a_command_failure_not_a_slot_fault() {
        let mut chain = PluginChain::spawn_with_backend_factory(
            || {
                let mut backend = MockBackend::new("oversized catalog", MockProcess::Gain(1.0));
                backend.parameter_catalog = (0..=MAX_PLUGIN_PARAMETER_CATALOG_ITEMS as u32)
                    .map(parameter_descriptor)
                    .collect();
                vec![BackendSlot::new(Box::new(backend))]
            },
            config(),
        )
        .unwrap();
        assert!(chain.control.request_parameter_page(0, 61, 0, 64));
        let failure = wait_for_event(&mut chain.control, |event| {
            matches!(
                event,
                RuntimeEvent::ParameterCommandFailed { request_id: 61, .. }
            )
        });
        assert!(matches!(
            failure,
            RuntimeEvent::ParameterCommandFailed {
                command: PluginParameterCommand::Catalog,
                id: None,
                ..
            }
        ));
        assert_eq!(chain.control.stats().faults, 0);
        assert!(chain.control.set_parameter_tagged(0, 1, 0.25, 62));
        let _ = wait_for_event(&mut chain.control, |event| {
            matches!(event, RuntimeEvent::ParameterSetAck { request_id: 62, .. })
        });
    }

    #[test]
    fn state_and_parameter_round_trip_over_control_plane() {
        let mut chain = PluginChain::spawn_with_backend_factory(
            || {
                vec![BackendSlot::new(Box::new(MockBackend::new(
                    "stateful",
                    MockProcess::Gain(1.0),
                )))]
            },
            config(),
        )
        .unwrap();
        assert!(chain.control.set_parameter(0, 12, 0.75));
        assert!(chain.control.query_parameter(0, 12));
        assert!(chain.control.load_state(0, vec![9, 8, 7]));
        assert!(chain.control.request_state_tagged(0, 42));

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut parameter = None;
        let mut state = None;
        while Instant::now() < deadline && (parameter.is_none() || state.is_none()) {
            match chain.control.try_next_event() {
                Some(RuntimeEvent::ParameterValue { value, .. }) => parameter = Some(value),
                Some(RuntimeEvent::State {
                    request_id, bytes, ..
                }) => state = Some((request_id, bytes)),
                Some(_) | None => thread::sleep(Duration::from_millis(1)),
            }
        }
        assert_eq!(parameter, Some(0.75));
        assert_eq!(state, Some((42, vec![9, 8, 7])));
    }
}
