//! Confirmed, allocation-free realtime ownership handoff for compiled timelines.
//!
//! This module deliberately does not render audio. The control side prepares all
//! owned state and submits revisioned requests; the callback side applies a
//! bounded number only at block boundaries and acknowledges what actually became
//! active. Every replaced callback-owned `Arc` is returned through a dedicated
//! retire queue so its final release and `Vec` destruction never run on the
//! callback.

use std::{
    collections::{BTreeMap, BTreeSet},
    mem,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering, fence},
    },
    thread,
};

use rtrb::{Consumer, Producer, PushError, RingBuffer};
use thiserror::Error;

use crate::model::MIXER_INSERT_SLOT_COUNT;
use crate::pdc::PreparedMixerGraphDelayBank;
use crate::timeline::{
    CompiledAutomationTarget, CompiledPluginRoute, CompiledTimeline, MAX_TIMELINE_DIAGNOSTICS,
    MAX_TIMELINE_EVENTS, PluginRouteDestination, TIMELINE_CALLBACK_MAX_AUDIO_ASSETS,
    TIMELINE_CALLBACK_MAX_EVENTS, TIMELINE_CALLBACK_MAX_FRAMES,
    TIMELINE_CALLBACK_MAX_GENERATOR_ENDPOINTS, TIMELINE_CALLBACK_MAX_MIXER_ENDPOINTS,
    TIMELINE_ENDPOINT_MAX_DRIVEN_PLUGIN_PARAMETERS,
    TIMELINE_ENDPOINT_TIMELINE_MAX_EVENTS_PER_CALLBACK,
    TIMELINE_ENDPOINT_TIMELINE_MAX_EVENTS_PER_QUANTUM, TIMELINE_MAX_DRIVEN_PLUGIN_PARAMETERS,
    TIMELINE_PLUGIN_FIXED_QUANTA_PER_CALLBACK, TIMELINE_PLUGIN_FIXED_QUANTUM_FRAMES,
    TimelineChaseError, TimelineChaseOptions, TimelineDiscontinuityState, TimelineEvent,
    TimelineEventKind, TimelineEventRange, TimelinePacket, TimelinePacketChunk,
    TimelinePacketError,
};
use crate::timeline_executor::{
    MAX_ACTIVE_AUDIO_CLIPS as EXECUTOR_MAX_ACTIVE_AUDIO_CLIPS,
    MAX_ACTIVE_AUTOMATION_LAYERS as EXECUTOR_MAX_ACTIVE_AUTOMATION_LAYERS,
    MAX_ACTIVE_NOTES as EXECUTOR_MAX_ACTIVE_NOTES,
    MAX_AUTOMATION_BASES as EXECUTOR_MAX_AUTOMATION_BASES,
};

pub const DEFAULT_TIMELINE_RUNTIME_COMMAND_CAPACITY: usize = 32;
pub const DEFAULT_TIMELINE_RUNTIME_EVENT_CAPACITY: usize = 64;
pub const DEFAULT_TIMELINE_RUNTIME_RETIRE_CAPACITY: usize = 64;
pub const MAX_TIMELINE_COMMANDS_PER_BLOCK: usize = 8;
/// Fixed mixer-track domain carried by one atomic transport activation.
pub const TIMELINE_MIXER_PAN_RELEASE_TRACKS: usize = 32;

pub const MAX_RUNTIME_AUDIO_CLIPS: usize = 262_144;
pub const MAX_RUNTIME_AUTOMATION_BASES: usize = EXECUTOR_MAX_AUTOMATION_BASES;
pub const MAX_RUNTIME_CHANNEL_BASES: usize = 4_096;
pub const MAX_RUNTIME_CHASE_AUDIO_CLIPS: usize = EXECUTOR_MAX_ACTIVE_AUDIO_CLIPS;
pub const MAX_RUNTIME_CHASE_AUTOMATION_LAYERS: usize = EXECUTOR_MAX_ACTIVE_AUTOMATION_LAYERS;
pub const MAX_RUNTIME_CHASE_NOTES: usize = EXECUTOR_MAX_ACTIVE_NOTES;

const MAX_RETIRES_PER_COMMAND: usize = 2;

/// Physical fixed-quantum endpoint. Mixer plug-ins at different dense slots
/// share one worker/adapter for their track and therefore one event budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TimelineCallbackEndpoint {
    Generator { channel_id: u32 },
    MixerInsert { track: u8 },
}

impl From<PluginRouteDestination> for TimelineCallbackEndpoint {
    fn from(destination: PluginRouteDestination) -> Self {
        match destination {
            PluginRouteDestination::Generator { channel_id, .. } => Self::Generator { channel_id },
            PluginRouteDestination::MixerInsert { track, .. } => Self::MixerInsert { track },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimelineRuntimeResource {
    TimelineEvents,
    TimelineDiagnostics,
    TimelineAudioClips,
    TimelineAutomationBases,
    TimelineChannelBases,
    ChaseAudioClips,
    ChaseAutomationLayers,
    ChaseAutomationBases,
    ChaseNotes,
    ConcurrentNotes,
    ConcurrentAudioClips,
    ConcurrentAutomationLayers,
    ReferencedAudioAssets,
    GeneratorRoutes,
    DrivenPluginParameters,
    EndpointDrivenPluginParameters {
        endpoint: TimelineCallbackEndpoint,
    },
    CallbackEvents {
        window_frames: u16,
    },
    EndpointEvents {
        endpoint: TimelineCallbackEndpoint,
        window_frames: u16,
    },
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum TimelineRuntimeValidationError {
    #[error("timeline revision must be non-zero")]
    InvalidRevision,
    #[error("timeline epoch must be non-zero")]
    InvalidEpoch,
    #[error("unsupported plug-in timing plan")]
    InvalidPluginTiming,
    #[error("{resource:?} contains {actual} items, exceeding the runtime maximum {maximum}")]
    ResourceLimitExceeded {
        resource: TimelineRuntimeResource,
        actual: usize,
        maximum: usize,
    },
    #[error(
        "{resource:?} contains {actual} events in the half-open window starting at frame {window_start_frame}, exceeding the runtime maximum {maximum}"
    )]
    CallbackCapacityExceeded {
        resource: TimelineRuntimeResource,
        window_start_frame: u64,
        actual: usize,
        maximum: usize,
    },
    #[error("plug-in instance {instance_id} has invalid callback route {destination:?}")]
    InvalidPluginRoute {
        instance_id: u64,
        destination: PluginRouteDestination,
    },
    #[error("plug-in instance {instance_id} has more than one callback route")]
    AmbiguousPluginRoute { instance_id: u64 },
    #[error(
        "physical plug-in endpoint {endpoint:?} slot {slot} is assigned to both instances {first_instance_id} and {second_instance_id}"
    )]
    DuplicatePhysicalPluginSlot {
        endpoint: TimelineCallbackEndpoint,
        slot: u8,
        first_instance_id: u64,
        second_instance_id: u64,
    },
    #[error(
        "Mixer Insert endpoint {track} has non-dense runtime slot {actual_slot}; expected slot {expected_slot}"
    )]
    NonDensePluginMixerChain {
        track: u8,
        expected_slot: u8,
        actual_slot: u8,
    },
    #[error(
        "driven plug-in parameter {parameter_id} has no unambiguous callback route for instance {instance_id}"
    )]
    PluginAutomationRouteUnavailable { instance_id: u64, parameter_id: u32 },
    #[error("unable to prepare callback mixer-graph delay resources")]
    MixerGraphDelayResourcesUnavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimelineResourceQueueFailure {
    Validation(TimelineRuntimeValidationError),
    CommandQueueFull,
    RequestIdExhausted,
    ShuttingDown,
}

#[derive(Debug)]
pub struct TimelineResourceQueueError<T> {
    pub reason: TimelineResourceQueueFailure,
    pub resource: T,
}

pub type PreparedTimelineMixerInstallResource =
    (Arc<CompiledTimeline>, Box<PreparedMixerGraphDelayBank>);

enum TimelineMixerDelayBankInstall {
    LegacyNone,
    Prepared(Box<PreparedMixerGraphDelayBank>),
    ReuseActive { fingerprint: u64 },
}

impl TimelineMixerDelayBankInstall {
    fn into_prepared(self) -> Option<Box<PreparedMixerGraphDelayBank>> {
        match self {
            Self::Prepared(bank) => Some(bank),
            Self::LegacyNone | Self::ReuseActive { .. } => None,
        }
    }
}

type TimelineMixerInstallResource = (Arc<CompiledTimeline>, TimelineMixerDelayBankInstall);

impl<T> TimelineResourceQueueError<T> {
    #[must_use]
    pub fn into_resource(self) -> T {
        self.resource
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum TimelineControlError {
    #[error("timeline revision must be non-zero")]
    InvalidRevision,
    #[error("timeline runtime command queue is full")]
    CommandQueueFull,
    #[error("timeline runtime request ids are exhausted")]
    RequestIdExhausted,
    #[error("timeline runtime is shutting down")]
    ShuttingDown,
    #[error("the realtime timeline endpoint is unavailable")]
    RealtimeUnavailable,
    #[error("timeline transport activation specification is invalid")]
    InvalidTransportActivation,
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum TimelineRuntimeCreateError {
    #[error("timeline command and event capacities must be non-zero")]
    InvalidQueueCapacity,
    #[error("timeline retire capacity must be at least {MAX_RETIRES_PER_COMMAND}")]
    RetireCapacityTooSmall,
}

/// Owned discontinuity state prepared entirely on the control thread.
#[derive(Debug)]
pub struct PreparedTimelineChase {
    pub(crate) plugin_topology_revision: u64,
    pub(crate) plugin_timing: crate::plugin_timing::PreparedPluginTimingPlan,
    revision: u64,
    epoch: u64,
    frame: u64,
    state: TimelineDiscontinuityState,
}

/// Reusable loop-start state prepared entirely on the control thread. Unlike a
/// one-shot chase, the template is deliberately not bound to an epoch: the
/// callback may activate the same owned state for each strictly newer loop
/// epoch without cloning any of its vectors.
#[derive(Debug)]
pub struct PreparedLoopTimelineChase {
    pub(crate) plugin_timing: crate::plugin_timing::PreparedPluginTimingPlan,
    revision: u64,
    frame: u64,
    state: TimelineDiscontinuityState,
}

impl PreparedLoopTimelineChase {
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    #[must_use]
    pub const fn frame(&self) -> u64 {
        self.frame
    }

    #[must_use]
    pub const fn state(&self) -> &TimelineDiscontinuityState {
        &self.state
    }
}

impl PreparedTimelineChase {
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    #[must_use]
    pub const fn epoch(&self) -> u64 {
        self.epoch
    }

    #[must_use]
    pub const fn frame(&self) -> u64 {
        self.frame
    }

    #[must_use]
    pub const fn state(&self) -> &TimelineDiscontinuityState {
        &self.state
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum TimelinePrepareChaseError {
    #[error(transparent)]
    Chase(#[from] TimelineChaseError),
    #[error(transparent)]
    Validation(#[from] TimelineRuntimeValidationError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimelineRejectReason {
    InvalidRevision,
    StaleRevision { latest_revision: u64 },
    RevisionMismatch { active_revision: Option<u64> },
    StaleEpoch { latest_epoch: u64 },
    InvalidChaseFrame,
    ChasePendingDelivery,
    MixerDelayBankReuseUnavailable { fingerprint: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimelineDiscontinuityKind {
    OneShot,
    Loop { token: u64 },
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum TimelineDiscontinuityActivationError {
    #[error("no compiled timeline is active")]
    MissingTimeline,
    #[error("timeline runtime is shut down")]
    Shutdown,
    #[error("timeline epoch must be non-zero")]
    InvalidEpoch,
    #[error("timeline ownership or partial-block state requires an explicit resync")]
    NeedsResync,
    #[error("requested timeline revision {requested} does not match active revision {active}")]
    RevisionMismatch { active: u64, requested: u64 },
    #[error("no prepared {kind:?} discontinuity is installed")]
    MissingPrepared { kind: TimelineDiscontinuityKind },
    #[error("one-shot discontinuity expects epoch {expected}, requested {requested}")]
    OneShotEpochMismatch { expected: u64, requested: u64 },
    #[error("discontinuity epoch {requested} is not newer than {latest}")]
    StaleEpoch { latest: u64, requested: u64 },
    #[error("discontinuity expects frame {expected}, requested {requested}")]
    FrameMismatch { expected: u64, requested: u64 },
    #[error("the prepared one-shot discontinuity has already been consumed")]
    OneShotConsumed,
    #[error("loop discontinuity expects template token {expected}, requested {requested}")]
    LoopTokenMismatch { expected: u64, requested: u64 },
    #[error("an uncommitted timeline block is already active")]
    ChunkInProgress,
    #[error("the staged replacement is missing its timeline, one-shot, or loop bundle member")]
    IncompleteCandidateBundle,
}

/// One complete transport/timeline ownership transaction. Beat fields are UI
/// hints; frame fields are the authoritative sample clock. `minimum_epoch`
/// binds the control-thread chase while `target_epoch` is raised by the callback
/// when the healthy active revision looped after that chase was prepared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimelineMixerPanRelease {
    /// A set bit transfers the corresponding track pan from the legacy bridge
    /// to the value encoded in `pan_bits` at the activation boundary.
    pub track_mask: u32,
    /// IEEE-754 bits keep the activation packet `Eq` without evaluating floats
    /// on the callback. Unselected entries must be canonical positive zero.
    pub pan_bits: [u32; TIMELINE_MIXER_PAN_RELEASE_TRACKS],
}

impl TimelineMixerPanRelease {
    pub const EMPTY: Self = Self {
        track_mask: 0,
        pan_bits: [0; TIMELINE_MIXER_PAN_RELEASE_TRACKS],
    };

    /// Adds one validated release value to a control-thread packet.
    ///
    /// Returns `false` without changing the packet when the track or value is
    /// outside the fixed callback domain.
    pub fn insert(&mut self, track: usize, pan: f32) -> bool {
        if track >= TIMELINE_MIXER_PAN_RELEASE_TRACKS
            || !pan.is_finite()
            || !(-1.0..=1.0).contains(&pan)
        {
            return false;
        }
        self.track_mask |= 1_u32 << track;
        self.pan_bits[track] = pan.to_bits();
        true
    }

    #[must_use]
    pub fn pan_for_track(self, track: usize) -> Option<f32> {
        if track >= TIMELINE_MIXER_PAN_RELEASE_TRACKS || self.track_mask & (1_u32 << track) == 0 {
            return None;
        }
        Some(f32::from_bits(self.pan_bits[track]))
    }

    fn is_valid(self) -> bool {
        self.pan_bits.iter().enumerate().all(|(track, bits)| {
            if self.track_mask & (1_u32 << track) == 0 {
                // A canonical unused tail makes the mask the only authority
                // and prevents hidden/non-deterministic payload state.
                *bits == 0
            } else {
                let pan = f32::from_bits(*bits);
                pan.is_finite() && (-1.0..=1.0).contains(&pan)
            }
        })
    }
}

impl Default for TimelineMixerPanRelease {
    fn default() -> Self {
        Self::EMPTY
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimelineTransportActivationSpec {
    pub revision: u64,
    pub target_epoch: u64,
    pub minimum_epoch: u64,
    pub frame: u64,
    pub beat_q32: u64,
    pub loop_start_frame: u64,
    pub loop_end_frame: u64,
    pub loop_start_q32: u64,
    pub loop_end_q32: u64,
    pub loop_token: u64,
    pub loop_enabled: bool,
    pub playing: bool,
    pub mixer_pan_release: TimelineMixerPanRelease,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimelineTransportActivationRejectReason {
    Runtime(TimelineDiscontinuityActivationError),
    MissingAudioAsset { asset_id: u64 },
    ChannelBaseTable,
    GeneratorRouteBinding,
    MixerGraphPlan,
    GraphPdcPlan,
    MixerDelayBank,
    PluginAutomationBinding,
    ExecutorState,
    RenderPlanCapacity,
    EndpointEventCapacity,
}

/// Callback confirmations for ownership handoff. `ChaseInstalled` and
/// `LoopChaseInstalled` confirm that prepared state is resident on the callback;
/// they deliberately do not mean that the transport cursor has moved. Cursor
/// activation is the synchronous callback-side `activate_discontinuity` step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimelineRuntimeEvent {
    Installed {
        request_id: u64,
        revision: u64,
    },
    ChaseInstalled {
        request_id: u64,
        revision: u64,
        epoch: u64,
        frame: u64,
    },
    LoopChaseInstalled {
        request_id: u64,
        revision: u64,
        frame: u64,
        token: u64,
    },
    TransportActivationApplied {
        request_id: u64,
        revision: u64,
        epoch: u64,
        frame: u64,
    },
    TransportActivationRejected {
        request_id: u64,
        revision: u64,
        reason: TimelineTransportActivationRejectReason,
    },
    Cleared {
        request_id: u64,
        revision: u64,
    },
    Rejected {
        request_id: u64,
        revision: u64,
        reason: TimelineRejectReason,
    },
    ShutdownComplete {
        request_id: u64,
        revision: u64,
    },
}

impl TimelineRuntimeEvent {
    #[must_use]
    pub const fn request_id(self) -> u64 {
        match self {
            Self::Installed { request_id, .. }
            | Self::ChaseInstalled { request_id, .. }
            | Self::LoopChaseInstalled { request_id, .. }
            | Self::TransportActivationApplied { request_id, .. }
            | Self::TransportActivationRejected { request_id, .. }
            | Self::Cleared { request_id, .. }
            | Self::Rejected { request_id, .. }
            | Self::ShutdownComplete { request_id, .. } => request_id,
        }
    }

    #[must_use]
    pub const fn revision(self) -> u64 {
        match self {
            Self::Installed { revision, .. }
            | Self::ChaseInstalled { revision, .. }
            | Self::LoopChaseInstalled { revision, .. }
            | Self::TransportActivationApplied { revision, .. }
            | Self::TransportActivationRejected { revision, .. }
            | Self::Cleared { revision, .. }
            | Self::Rejected { revision, .. }
            | Self::ShutdownComplete { revision, .. } => revision,
        }
    }
}

/// Resources returned to the control thread. Dropping a value obtained from
/// `poll_retired` is the intended destruction path.
#[derive(Debug)]
pub enum RetiredTimelineResource {
    Bundle {
        revision: u64,
        timeline: Arc<CompiledTimeline>,
        mixer_delay_bank: Option<Box<PreparedMixerGraphDelayBank>>,
        one_shot: Option<Box<PreparedTimelineChase>>,
        loop_token: Option<u64>,
        loop_chase: Option<Box<PreparedLoopTimelineChase>>,
    },
    Timeline {
        revision: u64,
        timeline: Arc<CompiledTimeline>,
        mixer_delay_bank: Option<Box<PreparedMixerGraphDelayBank>>,
    },
    Chase {
        revision: u64,
        epoch: u64,
        chase: Box<PreparedTimelineChase>,
    },
    LoopChase {
        revision: u64,
        frame: u64,
        token: u64,
        chase: Box<PreparedLoopTimelineChase>,
    },
    ChaseBundle {
        one_shot: Option<Box<PreparedTimelineChase>>,
        loop_token: Option<u64>,
        loop_chase: Option<Box<PreparedLoopTimelineChase>>,
    },
}

#[derive(Default)]
struct TimelineRuntimeShared {
    command_queue_full: AtomicU64,
    event_backpressure: AtomicU64,
    retire_backpressure: AtomicU64,
    rejected_requests: AtomicU64,
    unexpected_realtime_drops: AtomicU64,
    ownership_needs_resync: AtomicBool,
    realtime_alive: AtomicBool,
    shutdown_complete: AtomicBool,
    active_revision: AtomicU64,
    active_epoch: AtomicU64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TimelineRuntimeStats {
    pub command_queue_full: u64,
    pub event_backpressure: u64,
    pub retire_backpressure: u64,
    pub rejected_requests: u64,
    pub unexpected_realtime_drops: u64,
    pub ownership_needs_resync: bool,
}

impl TimelineRuntimeShared {
    fn snapshot(&self) -> TimelineRuntimeStats {
        TimelineRuntimeStats {
            command_queue_full: self.command_queue_full.load(Ordering::Relaxed),
            event_backpressure: self.event_backpressure.load(Ordering::Relaxed),
            retire_backpressure: self.retire_backpressure.load(Ordering::Relaxed),
            rejected_requests: self.rejected_requests.load(Ordering::Relaxed),
            unexpected_realtime_drops: self.unexpected_realtime_drops.load(Ordering::Relaxed),
            ownership_needs_resync: self.ownership_needs_resync.load(Ordering::Acquire),
        }
    }
}

// This control-plane ring has 32 slots in production. Keeping the fixed pan
// payload inline costs at most 8 KiB for the whole ring, avoids callback-side
// allocation/destruction, and makes command+payload admission one SPSC write.
// `default_timeline_command_ring_payload_stays_within_budget` guards the bound.
#[allow(clippy::large_enum_variant)]
enum TimelineRuntimeCommand {
    Install {
        request_id: u64,
        revision: u64,
        timeline: Arc<CompiledTimeline>,
        mixer_delay_bank: TimelineMixerDelayBankInstall,
    },
    InstallChase {
        request_id: u64,
        chase: Box<PreparedTimelineChase>,
    },
    InstallLoopChase {
        request_id: u64,
        chase: Box<PreparedLoopTimelineChase>,
    },
    ActivateTransport {
        request_id: u64,
        spec: TimelineTransportActivationSpec,
        legacy_transport_barrier: u64,
    },
    Clear {
        request_id: u64,
        revision: u64,
    },
    Shutdown {
        request_id: u64,
    },
}

/// Control-thread endpoint. All allocation, preparation, confirmation polling,
/// and destruction of retired state belongs here.
pub struct TimelineRuntimeController {
    commands: Producer<TimelineRuntimeCommand>,
    events: Consumer<TimelineRuntimeEvent>,
    retired: Consumer<RetiredTimelineResource>,
    shared: Arc<TimelineRuntimeShared>,
    next_request_id: u64,
    last_queued_request: u64,
    last_confirmed_request: u64,
    resident_revision: Option<u64>,
    resident_one_shot_epoch: Option<u64>,
    rejected_since_resync: bool,
    shutdown_request: Option<u64>,
    shutdown_confirmed: bool,
}

impl TimelineRuntimeController {
    pub fn prepare_chase(
        &self,
        timeline: &CompiledTimeline,
        revision: u64,
        epoch: u64,
        frame: u64,
        options: TimelineChaseOptions,
    ) -> Result<Box<PreparedTimelineChase>, TimelinePrepareChaseError> {
        if revision == 0 {
            return Err(TimelineRuntimeValidationError::InvalidRevision.into());
        }
        if epoch == 0 {
            return Err(TimelineRuntimeValidationError::InvalidEpoch.into());
        }
        let state = timeline.chase_discontinuity(frame, options)?;
        validate_chase_state(&state)?;
        validate_chase_endpoint_capacities(timeline, &state)?;
        Ok(Box::new(PreparedTimelineChase {
            plugin_topology_revision: 0,
            plugin_timing: crate::plugin_timing::PreparedPluginTimingPlan::conservative(
                timeline.sample_rate(),
            )
            .map_err(|_| TimelineRuntimeValidationError::InvalidPluginTiming)?,
            revision,
            epoch,
            frame,
            state,
        }))
    }

    pub fn prepare_loop_chase(
        &self,
        timeline: &CompiledTimeline,
        revision: u64,
        frame: u64,
        options: TimelineChaseOptions,
    ) -> Result<Box<PreparedLoopTimelineChase>, TimelinePrepareChaseError> {
        if revision == 0 {
            return Err(TimelineRuntimeValidationError::InvalidRevision.into());
        }
        let state = timeline.chase_discontinuity(frame, options)?;
        validate_chase_state(&state)?;
        validate_chase_endpoint_capacities(timeline, &state)?;
        Ok(Box::new(PreparedLoopTimelineChase {
            plugin_timing: crate::plugin_timing::PreparedPluginTimingPlan::conservative(
                timeline.sample_rate(),
            )
            .map_err(|_| TimelineRuntimeValidationError::InvalidPluginTiming)?,
            revision,
            frame,
            state,
        }))
    }

    pub fn install(
        &mut self,
        revision: u64,
        timeline: Arc<CompiledTimeline>,
    ) -> Result<u64, TimelineResourceQueueError<Arc<CompiledTimeline>>> {
        match self.install_inner(
            revision,
            timeline,
            TimelineMixerDelayBankInstall::LegacyNone,
        ) {
            Ok(request_id) => Ok(request_id),
            Err(error) => {
                let (timeline, mixer_delay_bank) = error.resource;
                debug_assert!(matches!(
                    mixer_delay_bank,
                    TimelineMixerDelayBankInstall::LegacyNone
                ));
                Err(resource_queue_error(error.reason, timeline))
            }
        }
    }

    /// Installs the compiled timeline and its callback-mutable mixer delay
    /// storage as one ownership unit. The bank is prepared and, on enqueue
    /// failure, destroyed by the control-thread caller together with the
    /// returned `Arc`; it is never released on the realtime thread.
    pub fn install_with_mixer_resources(
        &mut self,
        revision: u64,
        timeline: Arc<CompiledTimeline>,
        mixer_delay_bank: Box<PreparedMixerGraphDelayBank>,
    ) -> Result<u64, TimelineResourceQueueError<PreparedTimelineMixerInstallResource>> {
        match self.install_inner(
            revision,
            timeline,
            TimelineMixerDelayBankInstall::Prepared(mixer_delay_bank),
        ) {
            Ok(request_id) => Ok(request_id),
            Err(error) => {
                let (timeline, mixer_delay_bank) = error.resource;
                let TimelineMixerDelayBankInstall::Prepared(mixer_delay_bank) = mixer_delay_bank
                else {
                    unreachable!("mixer-resource install returns its exact prepared bank")
                };
                Err(resource_queue_error(
                    error.reason,
                    (timeline, mixer_delay_bank),
                ))
            }
        }
    }

    /// Requests an O(1) transfer of callback-owned delay storage from the
    /// currently resident graph into a newer timeline with the same compiled
    /// graph identity. Runtime-side validation is authoritative: a stale
    /// control-thread fingerprint is rejected without disturbing either
    /// active or candidate ownership.
    pub fn install_reusing_mixer_resources(
        &mut self,
        revision: u64,
        timeline: Arc<CompiledTimeline>,
        fingerprint: u64,
    ) -> Result<u64, TimelineResourceQueueError<Arc<CompiledTimeline>>> {
        if fingerprint == 0 || timeline.mixer_graph().fingerprint() != fingerprint {
            return Err(resource_queue_error(
                TimelineResourceQueueFailure::Validation(
                    TimelineRuntimeValidationError::MixerGraphDelayResourcesUnavailable,
                ),
                timeline,
            ));
        }
        match self.install_inner(
            revision,
            timeline,
            TimelineMixerDelayBankInstall::ReuseActive { fingerprint },
        ) {
            Ok(request_id) => Ok(request_id),
            Err(error) => {
                let (timeline, mixer_delay_bank) = error.resource;
                debug_assert!(matches!(
                    mixer_delay_bank,
                    TimelineMixerDelayBankInstall::ReuseActive {
                        fingerprint: queued
                    } if queued == fingerprint
                ));
                Err(resource_queue_error(error.reason, timeline))
            }
        }
    }

    fn install_inner(
        &mut self,
        revision: u64,
        timeline: Arc<CompiledTimeline>,
        mixer_delay_bank: TimelineMixerDelayBankInstall,
    ) -> Result<u64, TimelineResourceQueueError<TimelineMixerInstallResource>> {
        if self.shutdown_request.is_some() {
            return Err(resource_queue_error(
                TimelineResourceQueueFailure::ShuttingDown,
                (timeline, mixer_delay_bank),
            ));
        }
        if let Err(error) = validate_timeline(revision, &timeline) {
            return Err(resource_queue_error(
                TimelineResourceQueueFailure::Validation(error),
                (timeline, mixer_delay_bank),
            ));
        }
        let Some(next_request_id) = self.next_request_id.checked_add(1) else {
            return Err(resource_queue_error(
                TimelineResourceQueueFailure::RequestIdExhausted,
                (timeline, mixer_delay_bank),
            ));
        };
        let request_id = self.next_request_id;
        let command = TimelineRuntimeCommand::Install {
            request_id,
            revision,
            timeline,
            mixer_delay_bank,
        };
        match self.commands.push(command) {
            Ok(()) => {
                self.next_request_id = next_request_id;
                self.last_queued_request = request_id;
                Ok(request_id)
            }
            Err(PushError::Full(TimelineRuntimeCommand::Install {
                timeline,
                mixer_delay_bank,
                ..
            })) => {
                self.shared
                    .command_queue_full
                    .fetch_add(1, Ordering::Relaxed);
                Err(resource_queue_error(
                    TimelineResourceQueueFailure::CommandQueueFull,
                    (timeline, mixer_delay_bank),
                ))
            }
            Err(PushError::Full(_)) => unreachable!("install push returned a different command"),
        }
    }

    pub fn install_chase(
        &mut self,
        chase: Box<PreparedTimelineChase>,
    ) -> Result<u64, TimelineResourceQueueError<Box<PreparedTimelineChase>>> {
        if self.shutdown_request.is_some() {
            return Err(resource_queue_error(
                TimelineResourceQueueFailure::ShuttingDown,
                chase,
            ));
        }
        if let Err(error) = validate_prepared_chase(&chase) {
            return Err(resource_queue_error(
                TimelineResourceQueueFailure::Validation(error),
                chase,
            ));
        }
        let Some(next_request_id) = self.next_request_id.checked_add(1) else {
            return Err(resource_queue_error(
                TimelineResourceQueueFailure::RequestIdExhausted,
                chase,
            ));
        };
        let request_id = self.next_request_id;
        let command = TimelineRuntimeCommand::InstallChase { request_id, chase };
        match self.commands.push(command) {
            Ok(()) => {
                self.next_request_id = next_request_id;
                self.last_queued_request = request_id;
                Ok(request_id)
            }
            Err(PushError::Full(TimelineRuntimeCommand::InstallChase { chase, .. })) => {
                self.shared
                    .command_queue_full
                    .fetch_add(1, Ordering::Relaxed);
                Err(resource_queue_error(
                    TimelineResourceQueueFailure::CommandQueueFull,
                    chase,
                ))
            }
            Err(PushError::Full(_)) => {
                unreachable!("chase push returned a different command")
            }
        }
    }

    pub fn install_loop_chase(
        &mut self,
        chase: Box<PreparedLoopTimelineChase>,
    ) -> Result<u64, TimelineResourceQueueError<Box<PreparedLoopTimelineChase>>> {
        if self.shutdown_request.is_some() {
            return Err(resource_queue_error(
                TimelineResourceQueueFailure::ShuttingDown,
                chase,
            ));
        }
        if let Err(error) = validate_prepared_loop_chase(&chase) {
            return Err(resource_queue_error(
                TimelineResourceQueueFailure::Validation(error),
                chase,
            ));
        }
        let Some(next_request_id) = self.next_request_id.checked_add(1) else {
            return Err(resource_queue_error(
                TimelineResourceQueueFailure::RequestIdExhausted,
                chase,
            ));
        };
        let request_id = self.next_request_id;
        let command = TimelineRuntimeCommand::InstallLoopChase { request_id, chase };
        match self.commands.push(command) {
            Ok(()) => {
                self.next_request_id = next_request_id;
                self.last_queued_request = request_id;
                Ok(request_id)
            }
            Err(PushError::Full(TimelineRuntimeCommand::InstallLoopChase { chase, .. })) => {
                self.shared
                    .command_queue_full
                    .fetch_add(1, Ordering::Relaxed);
                Err(resource_queue_error(
                    TimelineResourceQueueFailure::CommandQueueFull,
                    chase,
                ))
            }
            Err(PushError::Full(_)) => {
                unreachable!("loop chase push returned a different command")
            }
        }
    }

    /// Queues one complete timeline/transport activation. Residency events are
    /// not activation receipts: callers must wait for the exact matching
    /// `TransportActivationApplied` or `TransportActivationRejected` event.
    pub(crate) fn activate_transport(
        &mut self,
        spec: TimelineTransportActivationSpec,
        legacy_transport_barrier: u64,
    ) -> Result<u64, TimelineControlError> {
        if self.shutdown_request.is_some() {
            return Err(TimelineControlError::ShuttingDown);
        }
        if !transport_activation_spec_is_valid(spec) {
            return Err(TimelineControlError::InvalidTransportActivation);
        }
        let (request_id, next_request_id) = self.next_request()?;
        match self
            .commands
            .push(TimelineRuntimeCommand::ActivateTransport {
                request_id,
                spec,
                legacy_transport_barrier,
            }) {
            Ok(()) => {
                self.next_request_id = next_request_id;
                self.last_queued_request = request_id;
                Ok(request_id)
            }
            Err(PushError::Full(_)) => {
                self.shared
                    .command_queue_full
                    .fetch_add(1, Ordering::Relaxed);
                Err(TimelineControlError::CommandQueueFull)
            }
        }
    }

    pub fn clear(&mut self, revision: u64) -> Result<u64, TimelineControlError> {
        if revision == 0 {
            return Err(TimelineControlError::InvalidRevision);
        }
        if self.shutdown_request.is_some() {
            return Err(TimelineControlError::ShuttingDown);
        }
        let (request_id, next_request_id) = self.next_request()?;
        match self.commands.push(TimelineRuntimeCommand::Clear {
            request_id,
            revision,
        }) {
            Ok(()) => {
                self.next_request_id = next_request_id;
                self.last_queued_request = request_id;
                Ok(request_id)
            }
            Err(PushError::Full(_)) => {
                self.shared
                    .command_queue_full
                    .fetch_add(1, Ordering::Relaxed);
                Err(TimelineControlError::CommandQueueFull)
            }
        }
    }

    pub fn request_shutdown(&mut self) -> Result<u64, TimelineControlError> {
        if let Some(request_id) = self.shutdown_request {
            return Ok(request_id);
        }
        if !self.shared.realtime_alive.load(Ordering::Acquire) {
            return Err(TimelineControlError::RealtimeUnavailable);
        }
        let (request_id, next_request_id) = self.next_request()?;
        match self
            .commands
            .push(TimelineRuntimeCommand::Shutdown { request_id })
        {
            Ok(()) => {
                self.next_request_id = next_request_id;
                self.last_queued_request = request_id;
                self.shutdown_request = Some(request_id);
                Ok(request_id)
            }
            Err(PushError::Full(_)) => {
                self.shared
                    .command_queue_full
                    .fetch_add(1, Ordering::Relaxed);
                Err(TimelineControlError::CommandQueueFull)
            }
        }
    }

    /// Waits for the callback to acknowledge shutdown while continuously freeing
    /// event and retire capacity. The realtime endpoint must continue servicing
    /// block boundaries on another thread.
    pub fn shutdown_blocking(&mut self) -> Result<(), TimelineControlError> {
        if self.shutdown_confirmed {
            self.drain_retired();
            return Ok(());
        }
        let request_id = loop {
            match self.request_shutdown() {
                Ok(request_id) => break request_id,
                Err(TimelineControlError::CommandQueueFull) => {
                    self.poll_all_events();
                    self.drain_retired();
                    if !self.shared.realtime_alive.load(Ordering::Acquire) {
                        return Err(TimelineControlError::RealtimeUnavailable);
                    }
                    thread::yield_now();
                }
                Err(error) => return Err(error),
            }
        };
        while !self.shutdown_confirmed {
            while let Some(event) = self.poll_event() {
                if matches!(
                    event,
                    TimelineRuntimeEvent::ShutdownComplete {
                        request_id: confirmed,
                        ..
                    } if confirmed == request_id
                ) {
                    self.shutdown_confirmed = true;
                }
            }
            self.drain_retired();
            if self.shutdown_confirmed {
                break;
            }
            if !self.shared.realtime_alive.load(Ordering::Acquire) {
                return Err(TimelineControlError::RealtimeUnavailable);
            }
            thread::yield_now();
        }
        self.drain_retired();
        Ok(())
    }

    pub fn poll_event(&mut self) -> Option<TimelineRuntimeEvent> {
        let event = self.events.pop().ok()?;
        self.last_confirmed_request = self.last_confirmed_request.max(event.request_id());
        match event {
            TimelineRuntimeEvent::Installed { revision, .. } => {
                self.resident_revision = Some(revision);
                self.resident_one_shot_epoch = None;
                self.rejected_since_resync = false;
            }
            TimelineRuntimeEvent::ChaseInstalled {
                revision, epoch, ..
            } => {
                self.resident_revision = Some(revision);
                self.resident_one_shot_epoch = Some(epoch);
                self.rejected_since_resync = false;
            }
            TimelineRuntimeEvent::LoopChaseInstalled { revision, .. } => {
                self.resident_revision = Some(revision);
                self.rejected_since_resync = false;
            }
            TimelineRuntimeEvent::TransportActivationApplied {
                revision, epoch, ..
            } => {
                self.resident_revision = Some(revision);
                self.resident_one_shot_epoch = Some(epoch);
                self.rejected_since_resync = false;
            }
            TimelineRuntimeEvent::TransportActivationRejected { .. } => {
                // Candidate rejection is explicitly non-destructive. A remains
                // synchronized and the candidate may be repaired or replaced.
            }
            TimelineRuntimeEvent::Cleared { .. } => {
                self.resident_revision = None;
                self.resident_one_shot_epoch = None;
                self.rejected_since_resync = false;
            }
            TimelineRuntimeEvent::Rejected { .. } => {
                self.rejected_since_resync = true;
            }
            TimelineRuntimeEvent::ShutdownComplete { .. } => {
                self.resident_revision = None;
                self.resident_one_shot_epoch = None;
                self.shutdown_confirmed = true;
            }
        }
        Some(event)
    }

    pub fn poll_retired(&mut self) -> Option<RetiredTimelineResource> {
        self.retired.pop().ok()
    }

    pub fn drain_retired(&mut self) -> usize {
        let mut drained = 0;
        while let Ok(resource) = self.retired.pop() {
            drop(resource);
            drained += 1;
        }
        drained
    }

    #[must_use]
    pub fn confirmed_revision(&self) -> Option<u64> {
        self.confirmed_identity().map(|identity| identity.0)
    }

    /// Newest compiled revision/chases confirmed resident on the callback. It
    /// may be a staged replacement while [`Self::confirmed_revision`] still
    /// reports the older revision that is actually rendering.
    #[must_use]
    pub const fn resident_revision(&self) -> Option<u64> {
        self.resident_revision
    }

    #[must_use]
    /// Epoch atomically published by callback activation, never merely by
    /// prepared-state residency.
    pub fn confirmed_epoch(&self) -> Option<u64> {
        self.confirmed_identity().map(|identity| identity.1)
    }

    /// Coherent active revision/epoch pair from the shared seqlock. Callers
    /// must not compose the two legacy accessors into a pair themselves.
    #[must_use]
    pub fn confirmed_identity_pair(&self) -> Option<(u64, u64)> {
        self.confirmed_identity()
    }

    /// Epoch of the one-shot chase resident on the callback. This is ownership
    /// confirmation only and does not imply that transport has activated it.
    #[must_use]
    pub fn resident_one_shot_epoch(&self) -> Option<u64> {
        self.resident_one_shot_epoch
    }

    #[must_use]
    pub fn last_queued_request(&self) -> u64 {
        self.last_queued_request
    }

    #[must_use]
    pub fn last_confirmed_request(&self) -> u64 {
        self.last_confirmed_request
    }

    #[must_use]
    pub fn is_synchronized(&self) -> bool {
        let confirmed_identity = self.confirmed_identity();
        !self.rejected_since_resync
            && !self.needs_resync()
            && self.last_confirmed_request == self.last_queued_request
            && confirmed_identity.is_some_and(|identity| {
                self.resident_revision == Some(identity.0) && identity.1 != 0
            })
    }

    #[must_use]
    pub fn needs_resync(&self) -> bool {
        self.shared.ownership_needs_resync.load(Ordering::Acquire)
    }

    /// Clears a defensive ownership/partial-block fault only after the caller
    /// observes a fresh, fully confirmed timeline and epoch.
    pub fn acknowledge_resync(&mut self, revision: u64, epoch: u64) -> bool {
        if self.resident_revision != Some(revision)
            || self.confirmed_identity() != Some((revision, epoch))
            || !self
                .resident_one_shot_epoch
                .is_some_and(|minimum_epoch| minimum_epoch <= epoch)
            || self.last_confirmed_request != self.last_queued_request
        {
            return false;
        }
        self.rejected_since_resync = false;
        true
    }

    fn confirmed_identity(&self) -> Option<(u64, u64)> {
        for _ in 0..3 {
            let before = self.shared.active_epoch.load(Ordering::Acquire);
            if before == 0 {
                return None;
            }
            let revision = self.shared.active_revision.load(Ordering::Relaxed);
            fence(Ordering::Acquire);
            let after = self.shared.active_epoch.load(Ordering::Relaxed);
            if before == after && revision != 0 {
                return Some((revision, after));
            }
        }
        None
    }

    #[must_use]
    pub fn stats(&self) -> TimelineRuntimeStats {
        self.shared.snapshot()
    }

    fn next_request(&self) -> Result<(u64, u64), TimelineControlError> {
        self.next_request_id
            .checked_add(1)
            .map(|next| (self.next_request_id, next))
            .ok_or(TimelineControlError::RequestIdExhausted)
    }

    fn poll_all_events(&mut self) {
        while self.poll_event().is_some() {}
    }
}

impl Drop for TimelineRuntimeController {
    fn drop(&mut self) {
        if self.shared.realtime_alive.load(Ordering::Acquire) && !self.shutdown_confirmed {
            // Destructors must never wait for an audio callback that may already
            // be stopped. Make one best-effort nonblocking enqueue; callers that
            // require confirmed reclamation use `shutdown_blocking` explicitly.
            let _ = self.request_shutdown();
        }
        self.poll_all_events();
        self.drain_retired();
    }
}

fn resource_queue_error<T>(
    reason: TimelineResourceQueueFailure,
    resource: T,
) -> TimelineResourceQueueError<T> {
    TimelineResourceQueueError { reason, resource }
}

struct InstalledTimeline {
    revision: u64,
    timeline: Arc<CompiledTimeline>,
    mixer_delay_bank: Option<Box<PreparedMixerGraphDelayBank>>,
    mixer_delay_bank_reuse_fingerprint: Option<u64>,
}

struct InstalledOneShotChase {
    chase: Box<PreparedTimelineChase>,
    consumed: bool,
}

struct InstalledLoopChase {
    chase: Box<PreparedLoopTimelineChase>,
    token: u64,
}

/// Fixed callback-owned handoff slot used by atomic revision promotion. The
/// old bundle is moved here in O(1) at the transport discontinuity and drained
/// into the retire ring only at a later block boundary with proven capacity.
struct DeferredTimelineRetire {
    timeline: InstalledTimeline,
    one_shot_chase: Option<InstalledOneShotChase>,
    loop_chase: Option<InstalledLoopChase>,
}

#[derive(Clone, Copy)]
struct ActiveDiscontinuity {
    kind: TimelineDiscontinuityKind,
    epoch: u64,
    frame: u64,
    delivered: bool,
}

#[derive(Clone, Copy)]
struct TransportCursor {
    epoch: u64,
    next_frame: u64,
    chunk_in_progress: bool,
}

#[derive(Clone, Copy)]
struct PendingTimelineTransportActivation {
    request_id: u64,
    spec: TimelineTransportActivationSpec,
    legacy_transport_barrier: u64,
}

/// Read-only proof that one queued activation still names the callback's
/// resident bundle. Fields are private so only this runtime can mint a ticket.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TimelineTransportActivationTicket {
    request_id: u64,
    spec: TimelineTransportActivationSpec,
    legacy_transport_barrier: u64,
}

impl TimelineTransportActivationTicket {
    #[must_use]
    pub(crate) const fn spec(self) -> TimelineTransportActivationSpec {
        self.spec
    }

    #[must_use]
    pub(crate) const fn legacy_transport_barrier(self) -> u64 {
        self.legacy_transport_barrier
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CommittedTimelineTransportActivation {
    request_id: u64,
    revision: u64,
    epoch: u64,
    frame: u64,
    recovering: bool,
}

/// Audio-callback endpoint. It owns no mutex and every public processing method
/// is allocation-free and bounded.
pub struct RealtimeTimelineRuntime {
    commands: Consumer<TimelineRuntimeCommand>,
    events: Producer<TimelineRuntimeEvent>,
    retired: Producer<RetiredTimelineResource>,
    shared: Arc<TimelineRuntimeShared>,
    /// Bundle currently rendered by the callback (or the initial resident
    /// bundle before its first activation).
    timeline: Option<InstalledTimeline>,
    one_shot_chase: Option<InstalledOneShotChase>,
    loop_chase: Option<InstalledLoopChase>,
    /// Newer replacement bundle prepared while the active revision continues
    /// rendering. A one-shot discontinuity promotes all three fields together.
    candidate_timeline: Option<InstalledTimeline>,
    candidate_one_shot_chase: Option<InstalledOneShotChase>,
    candidate_loop_chase: Option<InstalledLoopChase>,
    deferred_retire: Option<DeferredTimelineRetire>,
    active_discontinuity: Option<ActiveDiscontinuity>,
    transport: Option<TransportCursor>,
    pending_transport_activation: Option<PendingTimelineTransportActivation>,
    latest_revision: u64,
    latest_epoch: Option<u64>,
    shutdown: bool,
}

impl RealtimeTimelineRuntime {
    #[must_use]
    pub fn pending_commands(&self) -> usize {
        self.commands.slots()
    }

    #[must_use]
    pub fn active_revision(&self) -> Option<u64> {
        self.timeline.as_ref().map(|installed| installed.revision)
    }

    #[must_use]
    fn resident_revision(&self) -> Option<u64> {
        self.candidate_timeline
            .as_ref()
            .or(self.timeline.as_ref())
            .map(|installed| installed.revision)
    }

    fn resident_timeline(&self) -> Option<&InstalledTimeline> {
        self.candidate_timeline.as_ref().or(self.timeline.as_ref())
    }

    /// Immutable compiled data currently owned by the callback. Callers may
    /// copy bounded render metadata into their own preallocated tables; the
    /// returned reference must never escape callback processing.
    #[must_use]
    pub fn active_timeline(&self) -> Option<&CompiledTimeline> {
        self.timeline
            .as_ref()
            .map(|installed| installed.timeline.as_ref())
    }

    /// Callback-owned mutable delay storage paired with the active compiled
    /// timeline. The bank never leaves the installed bundle; callers may only
    /// borrow it for bounded realtime processing.
    #[must_use]
    pub(crate) fn active_mixer_delay_bank_mut(
        &mut self,
    ) -> Option<&mut PreparedMixerGraphDelayBank> {
        self.timeline
            .as_mut()
            .and_then(|installed| installed.mixer_delay_bank.as_deref_mut())
    }

    #[must_use]
    pub(crate) fn active_mixer_delay_bank(&self) -> Option<&PreparedMixerGraphDelayBank> {
        self.timeline
            .as_ref()
            .and_then(|installed| installed.mixer_delay_bank.as_deref())
    }

    #[must_use]
    pub fn active_epoch(&self) -> Option<u64> {
        self.transport.map(|transport| transport.epoch)
    }

    /// Exact generation token of the loop chase currently resident on the
    /// callback. Transport loop activation must use this value so replacing a
    /// same-revision/same-frame template cannot activate stale state.
    #[must_use]
    pub fn installed_loop_token(&self) -> Option<u64> {
        self.loop_chase.as_ref().map(|chase| chase.token)
    }

    /// Revision whose prepared state must service the next discontinuity. A
    /// replacement one-shot targets the staged candidate; loop wraps always
    /// target the bundle already active after that one-shot promotion.
    #[must_use]
    pub fn discontinuity_revision(&self, kind: TimelineDiscontinuityKind) -> Option<u64> {
        match kind {
            TimelineDiscontinuityKind::OneShot => self
                .candidate_one_shot_chase
                .as_ref()
                .map(|installed| installed.chase.revision)
                .or_else(|| {
                    self.one_shot_chase
                        .as_ref()
                        .map(|installed| installed.chase.revision)
                }),
            TimelineDiscontinuityKind::Loop { .. } => self
                .loop_chase
                .as_ref()
                .map(|installed| installed.chase.revision),
        }
    }

    #[must_use]
    pub fn next_frame(&self) -> Option<u64> {
        self.transport.map(|transport| transport.next_frame)
    }

    #[must_use]
    pub const fn is_shutdown(&self) -> bool {
        self.shutdown
    }

    #[must_use]
    pub fn stats(&self) -> TimelineRuntimeStats {
        self.shared.snapshot()
    }

    #[must_use]
    pub(crate) fn pending_transport_activation(&self) -> Option<TimelineTransportActivationTicket> {
        self.pending_transport_activation
            .map(|pending| TimelineTransportActivationTicket {
                request_id: pending.request_id,
                spec: pending.spec,
                legacy_transport_barrier: pending.legacy_transport_barrier,
            })
    }

    fn activation_targets_candidate(&self, revision: u64) -> bool {
        self.candidate_timeline
            .as_ref()
            .is_some_and(|timeline| timeline.revision == revision)
    }

    pub(crate) fn preflight_transport_activation(
        &self,
        ticket: TimelineTransportActivationTicket,
        actual_epoch: u64,
    ) -> Result<(), TimelineDiscontinuityActivationError> {
        let pending = self
            .pending_transport_activation
            .filter(|pending| {
                pending.request_id == ticket.request_id
                    && pending.spec == ticket.spec
                    && pending.legacy_transport_barrier == ticket.legacy_transport_barrier
            })
            .ok_or(TimelineDiscontinuityActivationError::MissingPrepared {
                kind: TimelineDiscontinuityKind::OneShot,
            })?;
        let spec = pending.spec;
        if self.shutdown {
            return Err(TimelineDiscontinuityActivationError::Shutdown);
        }
        if actual_epoch == 0
            || actual_epoch < spec.minimum_epoch
            || actual_epoch < spec.target_epoch
        {
            return Err(TimelineDiscontinuityActivationError::InvalidEpoch);
        }
        if self
            .transport
            .is_some_and(|transport| transport.chunk_in_progress)
        {
            return Err(TimelineDiscontinuityActivationError::ChunkInProgress);
        }
        let promotes_candidate = self.activation_targets_candidate(spec.revision);
        let installed = if promotes_candidate {
            self.candidate_timeline.as_ref()
        } else {
            self.timeline.as_ref()
        }
        .ok_or(TimelineDiscontinuityActivationError::MissingTimeline)?;
        if installed.revision != spec.revision {
            return Err(TimelineDiscontinuityActivationError::RevisionMismatch {
                active: installed.revision,
                requested: spec.revision,
            });
        }
        let one_shot = if promotes_candidate {
            self.candidate_one_shot_chase.as_ref()
        } else {
            self.one_shot_chase.as_ref()
        }
        .ok_or(TimelineDiscontinuityActivationError::MissingPrepared {
            kind: TimelineDiscontinuityKind::OneShot,
        })?;
        if one_shot.chase.revision != spec.revision {
            return Err(TimelineDiscontinuityActivationError::RevisionMismatch {
                active: one_shot.chase.revision,
                requested: spec.revision,
            });
        }
        if one_shot.consumed {
            return Err(TimelineDiscontinuityActivationError::OneShotConsumed);
        }
        if one_shot.chase.epoch != spec.minimum_epoch {
            return Err(TimelineDiscontinuityActivationError::OneShotEpochMismatch {
                expected: one_shot.chase.epoch,
                requested: spec.minimum_epoch,
            });
        }
        if one_shot.chase.frame != spec.frame {
            return Err(TimelineDiscontinuityActivationError::FrameMismatch {
                expected: one_shot.chase.frame,
                requested: spec.frame,
            });
        }
        let loop_chase = if promotes_candidate {
            self.candidate_loop_chase.as_ref()
        } else {
            self.loop_chase.as_ref()
        }
        .ok_or(TimelineDiscontinuityActivationError::IncompleteCandidateBundle)?;
        if loop_chase.chase.revision != spec.revision {
            return Err(TimelineDiscontinuityActivationError::RevisionMismatch {
                active: loop_chase.chase.revision,
                requested: spec.revision,
            });
        }
        if loop_chase.token != spec.loop_token {
            return Err(TimelineDiscontinuityActivationError::LoopTokenMismatch {
                expected: loop_chase.token,
                requested: spec.loop_token,
            });
        }
        if loop_chase.chase.frame != spec.loop_start_frame {
            return Err(TimelineDiscontinuityActivationError::FrameMismatch {
                expected: loop_chase.chase.frame,
                requested: spec.loop_start_frame,
            });
        }
        if let Some(latest) = self.latest_epoch
            && actual_epoch <= latest
        {
            return Err(TimelineDiscontinuityActivationError::StaleEpoch {
                latest,
                requested: actual_epoch,
            });
        }
        if promotes_candidate && self.deferred_retire.is_some() {
            return Err(TimelineDiscontinuityActivationError::ChunkInProgress);
        }
        Ok(())
    }

    pub(crate) fn transport_activation_timeline(
        &self,
        ticket: TimelineTransportActivationTicket,
    ) -> Option<&CompiledTimeline> {
        let pending = self.pending_transport_activation?;
        if pending.request_id != ticket.request_id || pending.spec != ticket.spec {
            return None;
        }
        self.candidate_timeline
            .as_ref()
            .filter(|timeline| timeline.revision == ticket.spec.revision)
            .or_else(|| {
                self.timeline
                    .as_ref()
                    .filter(|timeline| timeline.revision == ticket.spec.revision)
            })
            .map(|timeline| timeline.timeline.as_ref())
    }

    /// Immutable preflight view of the mixer delay bank belonging to the exact
    /// timeline selected by `ticket`. A rejected activation therefore cannot
    /// retarget or advance either the active or candidate bank.
    pub(crate) fn transport_activation_mixer_delay_bank(
        &self,
        ticket: TimelineTransportActivationTicket,
    ) -> Option<&PreparedMixerGraphDelayBank> {
        let pending = self.pending_transport_activation?;
        if pending.request_id != ticket.request_id || pending.spec != ticket.spec {
            return None;
        }
        if let Some(candidate) = self
            .candidate_timeline
            .as_ref()
            .filter(|timeline| timeline.revision == ticket.spec.revision)
        {
            if let Some(bank) = candidate.mixer_delay_bank.as_deref() {
                return Some(bank);
            }
            let fingerprint = candidate.mixer_delay_bank_reuse_fingerprint?;
            return self.timeline.as_ref().and_then(|active| {
                active
                    .mixer_delay_bank
                    .as_deref()
                    .filter(|bank| bank.graph_fingerprint() == fingerprint)
            });
        }
        self.timeline
            .as_ref()
            .filter(|timeline| timeline.revision == ticket.spec.revision)
            .and_then(|timeline| timeline.mixer_delay_bank.as_deref())
    }

    pub(crate) fn transport_activation_plugin_timing(
        &self,
        ticket: TimelineTransportActivationTicket,
    ) -> Option<crate::plugin_timing::PreparedPluginTimingPlan> {
        let pending = self.pending_transport_activation?;
        if pending.request_id != ticket.request_id || pending.spec != ticket.spec {
            return None;
        }
        self.candidate_one_shot_chase
            .as_ref()
            .filter(|chase| chase.chase.revision == ticket.spec.revision)
            .or_else(|| {
                self.one_shot_chase
                    .as_ref()
                    .filter(|chase| chase.chase.revision == ticket.spec.revision)
            })
            .map(|chase| chase.chase.plugin_timing)
    }

    pub(crate) fn transport_activation_plugin_topology_revision(
        &self,
        ticket: TimelineTransportActivationTicket,
    ) -> Option<u64> {
        let pending = self.pending_transport_activation?;
        if pending.request_id != ticket.request_id || pending.spec != ticket.spec {
            return None;
        }
        self.candidate_one_shot_chase
            .as_ref()
            .filter(|chase| chase.chase.revision == ticket.spec.revision)
            .or_else(|| {
                self.one_shot_chase
                    .as_ref()
                    .filter(|chase| chase.chase.revision == ticket.spec.revision)
            })
            .map(|chase| chase.chase.plugin_topology_revision)
    }

    pub(crate) fn transport_activation_chase(
        &self,
        ticket: TimelineTransportActivationTicket,
    ) -> Option<&TimelineDiscontinuityState> {
        let pending = self.pending_transport_activation?;
        if pending.request_id != ticket.request_id || pending.spec != ticket.spec {
            return None;
        }
        self.candidate_one_shot_chase
            .as_ref()
            .filter(|chase| chase.chase.revision == ticket.spec.revision)
            .or_else(|| {
                self.one_shot_chase
                    .as_ref()
                    .filter(|chase| chase.chase.revision == ticket.spec.revision)
            })
            .map(|chase| &chase.chase.state)
    }

    /// Commits only pointer/cursor swaps proven by `preflight_transport_activation`.
    /// Active identity is intentionally withheld until the caller commits every
    /// other callback-owned member of the transaction.
    pub(crate) fn commit_preflighted_transport_activation(
        &mut self,
        ticket: TimelineTransportActivationTicket,
        actual_epoch: u64,
    ) -> CommittedTimelineTransportActivation {
        debug_assert!(
            self.preflight_transport_activation(ticket, actual_epoch)
                .is_ok()
        );
        let pending = self
            .pending_transport_activation
            .take()
            .expect("preflighted timeline transport activation remains pending");
        debug_assert_eq!(pending.request_id, ticket.request_id);
        let spec = pending.spec;
        if self.activation_targets_candidate(spec.revision) {
            self.promote_candidate(spec.revision);
        }
        self.one_shot_chase
            .as_mut()
            .expect("preflighted one-shot remains installed")
            .consumed = true;
        self.active_discontinuity = Some(ActiveDiscontinuity {
            kind: TimelineDiscontinuityKind::OneShot,
            epoch: actual_epoch,
            frame: spec.frame,
            delivered: true,
        });
        self.transport = Some(TransportCursor {
            epoch: actual_epoch,
            next_frame: spec.frame,
            chunk_in_progress: false,
        });
        self.latest_epoch = Some(actual_epoch);
        CommittedTimelineTransportActivation {
            request_id: pending.request_id,
            revision: spec.revision,
            epoch: actual_epoch,
            frame: spec.frame,
            recovering: self.shared.ownership_needs_resync.load(Ordering::Acquire),
        }
    }

    /// Last publication point after transport, executor, routes, voices and PDC
    /// have all committed at the same callback boundary.
    pub(crate) fn publish_committed_transport_activation(
        &mut self,
        committed: CommittedTimelineTransportActivation,
    ) {
        self.publish_active_epoch(committed.revision, committed.epoch);
        if committed.recovering {
            self.shared
                .ownership_needs_resync
                .store(false, Ordering::Release);
        }
        let pushed = self.push_event(TimelineRuntimeEvent::TransportActivationApplied {
            request_id: committed.request_id,
            revision: committed.revision,
            epoch: committed.epoch,
            frame: committed.frame,
        });
        debug_assert!(pushed, "activation receipt slot was preflighted");
    }

    pub(crate) fn reject_pending_transport_activation(
        &mut self,
        ticket: TimelineTransportActivationTicket,
        reason: TimelineTransportActivationRejectReason,
    ) {
        let Some(pending) = self.pending_transport_activation.take() else {
            return;
        };
        if pending.request_id != ticket.request_id || pending.spec != ticket.spec {
            self.pending_transport_activation = Some(pending);
            return;
        }
        self.shared
            .rejected_requests
            .fetch_add(1, Ordering::Relaxed);
        let pushed = self.push_event(TimelineRuntimeEvent::TransportActivationRejected {
            request_id: pending.request_id,
            revision: pending.spec.revision,
            reason,
        });
        debug_assert!(pushed, "activation rejection slot was preflighted");
    }

    /// Fail-closed callback signal used when downstream activation/execution
    /// cannot establish a coherent replacement cursor.
    pub fn require_resync(&mut self) {
        if !self.shutdown {
            self.shared
                .ownership_needs_resync
                .store(true, Ordering::Release);
        }
    }

    /// Applies a bounded number of requests at a block boundary. An unavailable
    /// confirmation or retire slot stops before popping the next command.
    pub fn apply_pending_at_block_boundary(&mut self) -> usize {
        self.apply_pending_at_block_boundary_with_budget(MAX_TIMELINE_COMMANDS_PER_BLOCK)
    }

    pub fn apply_pending_at_block_boundary_with_budget(&mut self, budget: usize) -> usize {
        if self.shutdown {
            return 0;
        }
        // The event slot reserved when this command was popped belongs to its
        // exact activation receipt. No later command may consume it first.
        if self.pending_transport_activation.is_some() {
            return 0;
        }
        if !self.flush_deferred_retire() {
            self.shared
                .retire_backpressure
                .fetch_add(1, Ordering::Relaxed);
            return 0;
        }
        let mut applied = 0;
        let budget = budget.min(MAX_TIMELINE_COMMANDS_PER_BLOCK);
        while applied < budget {
            let retire_slots = match self.commands.peek() {
                Ok(command) => self.required_retire_slots(command),
                Err(_) => break,
            };
            if self.events.slots() == 0 {
                self.shared
                    .event_backpressure
                    .fetch_add(1, Ordering::Relaxed);
                break;
            }
            if self.retired.slots() < retire_slots {
                self.shared
                    .retire_backpressure
                    .fetch_add(1, Ordering::Relaxed);
                break;
            }
            let command = self
                .commands
                .pop()
                .expect("peeked timeline command must remain readable");
            self.apply_command(command);
            applied += 1;
            if self.shutdown || self.pending_transport_activation.is_some() {
                break;
            }
        }
        applied
    }

    /// Atomically switches the callback transport cursor at the actual
    /// stop/seek/loop discontinuity. Installing prepared state never calls this
    /// implicitly, so an early control-thread request cannot move playback.
    pub fn activate_discontinuity(
        &mut self,
        revision: u64,
        epoch: u64,
        frame: u64,
        kind: TimelineDiscontinuityKind,
    ) -> Result<(), TimelineDiscontinuityActivationError> {
        self.activate_discontinuity_inner(revision, epoch, frame, kind, false)
    }

    /// Recovers from a callback-owned partial-block fault in one callback-thread
    /// transaction. Prepared state and all cursor metadata are validated first;
    /// only after the replacement cursor is installed is the shared fault flag
    /// released. The control endpoint cannot perform this transition.
    pub fn activate_discontinuity_after_resync(
        &mut self,
        revision: u64,
        epoch: u64,
        frame: u64,
        kind: TimelineDiscontinuityKind,
    ) -> Result<(), TimelineDiscontinuityActivationError> {
        self.activate_discontinuity_inner(revision, epoch, frame, kind, true)
    }

    fn activate_discontinuity_inner(
        &mut self,
        revision: u64,
        epoch: u64,
        frame: u64,
        kind: TimelineDiscontinuityKind,
        recovering: bool,
    ) -> Result<(), TimelineDiscontinuityActivationError> {
        if self.shutdown {
            return Err(TimelineDiscontinuityActivationError::Shutdown);
        }
        if epoch == 0 {
            return Err(TimelineDiscontinuityActivationError::InvalidEpoch);
        }
        if !recovering && self.shared.ownership_needs_resync.load(Ordering::Acquire) {
            return Err(TimelineDiscontinuityActivationError::NeedsResync);
        }
        let promotes_candidate = matches!(kind, TimelineDiscontinuityKind::OneShot)
            && self
                .candidate_timeline
                .as_ref()
                .is_some_and(|installed| installed.revision == revision);
        let installed = if promotes_candidate {
            self.candidate_timeline.as_ref()
        } else {
            self.timeline.as_ref()
        }
        .ok_or(TimelineDiscontinuityActivationError::MissingTimeline)?;
        if installed.revision != revision {
            return Err(TimelineDiscontinuityActivationError::RevisionMismatch {
                active: installed.revision,
                requested: revision,
            });
        }
        if self
            .transport
            .is_some_and(|transport| transport.chunk_in_progress)
        {
            return Err(TimelineDiscontinuityActivationError::ChunkInProgress);
        }
        match kind {
            TimelineDiscontinuityKind::OneShot => {
                let prepared = if promotes_candidate {
                    self.candidate_one_shot_chase.as_ref()
                } else {
                    self.one_shot_chase.as_ref()
                }
                .ok_or(TimelineDiscontinuityActivationError::MissingPrepared { kind })?;
                if prepared.chase.revision != revision {
                    return Err(TimelineDiscontinuityActivationError::RevisionMismatch {
                        active: prepared.chase.revision,
                        requested: revision,
                    });
                }
                if prepared.consumed {
                    return Err(TimelineDiscontinuityActivationError::OneShotConsumed);
                }
                let epoch_matches = if promotes_candidate {
                    epoch >= prepared.chase.epoch
                } else {
                    prepared.chase.epoch == epoch
                };
                if !epoch_matches {
                    return Err(TimelineDiscontinuityActivationError::OneShotEpochMismatch {
                        expected: prepared.chase.epoch,
                        requested: epoch,
                    });
                }
                if prepared.chase.frame != frame {
                    return Err(TimelineDiscontinuityActivationError::FrameMismatch {
                        expected: prepared.chase.frame,
                        requested: frame,
                    });
                }
            }
            TimelineDiscontinuityKind::Loop { token } => {
                let prepared = self
                    .loop_chase
                    .as_ref()
                    .ok_or(TimelineDiscontinuityActivationError::MissingPrepared { kind })?;
                if prepared.chase.revision != revision {
                    return Err(TimelineDiscontinuityActivationError::RevisionMismatch {
                        active: prepared.chase.revision,
                        requested: revision,
                    });
                }
                if prepared.chase.frame != frame {
                    return Err(TimelineDiscontinuityActivationError::FrameMismatch {
                        expected: prepared.chase.frame,
                        requested: frame,
                    });
                }
                if prepared.token != token {
                    return Err(TimelineDiscontinuityActivationError::LoopTokenMismatch {
                        expected: prepared.token,
                        requested: token,
                    });
                }
            }
        }

        if promotes_candidate
            && !self
                .candidate_loop_chase
                .as_ref()
                .is_some_and(|prepared| prepared.chase.revision == revision)
        {
            return Err(TimelineDiscontinuityActivationError::IncompleteCandidateBundle);
        }

        if let Some(latest) = self.latest_epoch
            && epoch <= latest
        {
            return Err(TimelineDiscontinuityActivationError::StaleEpoch {
                latest,
                requested: epoch,
            });
        }

        if promotes_candidate {
            self.promote_candidate(revision);
        }
        if kind == TimelineDiscontinuityKind::OneShot {
            self.one_shot_chase
                .as_mut()
                .expect("one-shot activation was validated")
                .consumed = true;
        }
        self.active_discontinuity = Some(ActiveDiscontinuity {
            kind,
            epoch,
            frame,
            delivered: false,
        });
        self.transport = Some(TransportCursor {
            epoch,
            next_frame: frame,
            chunk_in_progress: false,
        });
        self.latest_epoch = Some(epoch);
        self.publish_active_epoch(revision, epoch);
        if recovering {
            self.shared
                .ownership_needs_resync
                .store(false, Ordering::Release);
        }
        Ok(())
    }

    fn publish_active_epoch(&self, revision: u64, epoch: u64) {
        self.shared.active_epoch.store(0, Ordering::Release);
        fence(Ordering::Release);
        self.shared
            .active_revision
            .store(revision, Ordering::Relaxed);
        self.shared.active_epoch.store(epoch, Ordering::Release);
    }

    fn clear_published_active_epoch(&self) {
        self.shared.active_epoch.store(0, Ordering::Release);
        fence(Ordering::Release);
        self.shared.active_revision.store(0, Ordering::Relaxed);
    }

    /// Returns the discontinuity state exactly once for the first block of its
    /// epoch. Call this before packetizing that block.
    pub fn chase_for_block(
        &mut self,
        revision: u64,
        epoch: u64,
        start_frame: u64,
    ) -> Result<Option<&TimelineDiscontinuityState>, TimelineRuntimeQueryError> {
        self.validate_query(revision, epoch, start_frame)?;
        let active = self
            .active_discontinuity
            .as_ref()
            .copied()
            .ok_or(TimelineRuntimeQueryError::EpochUnavailable)?;
        if active.delivered {
            return Ok(None);
        }
        if active.frame != start_frame {
            return Err(TimelineRuntimeQueryError::StartFrameMismatch {
                expected: active.frame,
                requested: start_frame,
            });
        }
        debug_assert_eq!(active.epoch, epoch);
        self.active_discontinuity
            .as_mut()
            .expect("active discontinuity was validated")
            .delivered = true;
        let state = match active.kind {
            TimelineDiscontinuityKind::OneShot => {
                &self
                    .one_shot_chase
                    .as_ref()
                    .expect("active one-shot state must remain installed")
                    .chase
                    .state
            }
            TimelineDiscontinuityKind::Loop { .. } => {
                &self
                    .loop_chase
                    .as_ref()
                    .expect("active loop state must remain installed")
                    .chase
                    .state
            }
        };
        Ok(Some(state))
    }

    /// Fills one fixed packet and returns a commit guard. Copying never advances
    /// transport; the caller commits only after the executor accepts and finishes
    /// the block. Dropping the guard is a fail-closed resync fault.
    pub fn packetize_block<const CAPACITY: usize>(
        &mut self,
        revision: u64,
        epoch: u64,
        start_frame: u64,
        frames: u32,
        packet: &mut TimelinePacket<CAPACITY>,
    ) -> Result<PreparedTimelineBlock<'_>, TimelineRuntimePacketError> {
        self.validate_query(revision, epoch, start_frame)?;
        self.ensure_chase_delivered()?;
        let installed = self.timeline.as_ref().expect("query validated timeline");
        installed
            .timeline
            .packetize_into(packet, epoch, start_frame, frames)?;
        let end_frame = start_frame
            .checked_add(u64::from(frames))
            .ok_or(TimelinePacketError::FrameRangeOverflow)?;
        let transport = self.transport.as_mut().expect("query validated transport");
        transport.chunk_in_progress = true;
        Ok(PreparedTimelineBlock {
            next_frame: &mut transport.next_frame,
            block_in_progress: &mut transport.chunk_in_progress,
            shared: &self.shared,
            end_frame,
            committed: false,
        })
    }

    /// Opens an allocation-free cursor for a block containing more events than
    /// one packet. Dropping it after copying a partial block marks resync needed.
    pub fn begin_chunked_block(
        &mut self,
        revision: u64,
        epoch: u64,
        start_frame: u64,
        frames: u32,
    ) -> Result<RealtimeTimelineBlock<'_>, TimelineRuntimePacketError> {
        self.validate_query(revision, epoch, start_frame)?;
        self.ensure_chase_delivered()?;
        let end_frame = start_frame
            .checked_add(u64::from(frames))
            .ok_or(TimelinePacketError::FrameRangeOverflow)?;
        let timeline = &self
            .timeline
            .as_ref()
            .expect("query validated timeline")
            .timeline;
        let range = timeline.event_range(epoch, start_frame, frames)?;
        let transport = self.transport.as_mut().expect("query validated transport");
        transport.chunk_in_progress = true;
        Ok(RealtimeTimelineBlock {
            range,
            next_frame: &mut transport.next_frame,
            block_in_progress: &mut transport.chunk_in_progress,
            shared: &self.shared,
            end_frame,
            copy_complete: false,
            committed: false,
        })
    }

    fn validate_query(
        &self,
        revision: u64,
        epoch: u64,
        start_frame: u64,
    ) -> Result<(), TimelineRuntimeQueryError> {
        if self.shutdown {
            return Err(TimelineRuntimeQueryError::Shutdown);
        }
        if self.shared.ownership_needs_resync.load(Ordering::Acquire) {
            return Err(TimelineRuntimeQueryError::NeedsResync);
        }
        let installed = self
            .timeline
            .as_ref()
            .ok_or(TimelineRuntimeQueryError::MissingTimeline)?;
        if installed.revision != revision {
            return Err(TimelineRuntimeQueryError::RevisionMismatch {
                active: installed.revision,
                requested: revision,
            });
        }
        let transport = self
            .transport
            .as_ref()
            .ok_or(TimelineRuntimeQueryError::EpochUnavailable)?;
        if transport.epoch != epoch {
            return Err(TimelineRuntimeQueryError::EpochMismatch {
                active: transport.epoch,
                requested: epoch,
            });
        }
        if transport.chunk_in_progress {
            return Err(TimelineRuntimeQueryError::ChunkInProgress);
        }
        if transport.next_frame != start_frame {
            return Err(TimelineRuntimeQueryError::StartFrameMismatch {
                expected: transport.next_frame,
                requested: start_frame,
            });
        }
        Ok(())
    }

    fn ensure_chase_delivered(&self) -> Result<(), TimelineRuntimeQueryError> {
        if self
            .active_discontinuity
            .is_some_and(|chase| !chase.delivered)
        {
            Err(TimelineRuntimeQueryError::ChaseRequired)
        } else {
            Ok(())
        }
    }

    fn required_retire_slots(&self, command: &TimelineRuntimeCommand) -> usize {
        match command {
            TimelineRuntimeCommand::Install {
                revision,
                mixer_delay_bank,
                ..
            } => {
                let reuse_available = match mixer_delay_bank {
                    TimelineMixerDelayBankInstall::ReuseActive { fingerprint } => {
                        self.mixer_delay_bank_reuse_available(*fingerprint)
                    }
                    TimelineMixerDelayBankInstall::LegacyNone
                    | TimelineMixerDelayBankInstall::Prepared(_) => true,
                };
                if *revision != 0 && *revision > self.latest_revision && reuse_available {
                    if self.transport.is_some() {
                        usize::from(
                            self.candidate_timeline.is_some()
                                || self.candidate_one_shot_chase.is_some()
                                || self.candidate_loop_chase.is_some(),
                        )
                    } else {
                        usize::from(
                            self.timeline.is_some()
                                || self.one_shot_chase.is_some()
                                || self.loop_chase.is_some(),
                        )
                    }
                } else {
                    1
                }
            }
            TimelineRuntimeCommand::InstallChase { chase, .. } => {
                if self.chase_rejection(chase).is_some() {
                    1
                } else if self
                    .candidate_timeline
                    .as_ref()
                    .is_some_and(|timeline| timeline.revision == chase.revision)
                {
                    usize::from(self.candidate_one_shot_chase.is_some())
                } else {
                    usize::from(self.one_shot_chase.is_some())
                }
            }
            TimelineRuntimeCommand::InstallLoopChase { chase, .. } => {
                if self.loop_chase_rejection(chase).is_some() {
                    1
                } else if self
                    .candidate_timeline
                    .as_ref()
                    .is_some_and(|timeline| timeline.revision == chase.revision)
                {
                    usize::from(self.candidate_loop_chase.is_some())
                } else {
                    usize::from(self.loop_chase.is_some())
                }
            }
            TimelineRuntimeCommand::ActivateTransport { .. } => 0,
            TimelineRuntimeCommand::Clear { revision, .. } => {
                if self.active_revision() == Some(*revision)
                    || self.resident_revision() == Some(*revision)
                {
                    usize::from(
                        self.timeline.is_some()
                            || self.one_shot_chase.is_some()
                            || self.loop_chase.is_some(),
                    ) + usize::from(
                        self.candidate_timeline.is_some()
                            || self.candidate_one_shot_chase.is_some()
                            || self.candidate_loop_chase.is_some(),
                    )
                } else {
                    0
                }
            }
            TimelineRuntimeCommand::Shutdown { .. } => {
                usize::from(
                    self.timeline.is_some()
                        || self.one_shot_chase.is_some()
                        || self.loop_chase.is_some(),
                ) + usize::from(
                    self.candidate_timeline.is_some()
                        || self.candidate_one_shot_chase.is_some()
                        || self.candidate_loop_chase.is_some(),
                )
            }
        }
    }

    fn apply_command(&mut self, command: TimelineRuntimeCommand) {
        match command {
            TimelineRuntimeCommand::Install {
                request_id,
                revision,
                timeline,
                mixer_delay_bank,
            } => self.apply_install(request_id, revision, timeline, mixer_delay_bank),
            TimelineRuntimeCommand::InstallChase { request_id, chase } => {
                self.apply_chase(request_id, chase);
            }
            TimelineRuntimeCommand::InstallLoopChase { request_id, chase } => {
                self.apply_loop_chase(request_id, chase);
            }
            TimelineRuntimeCommand::ActivateTransport {
                request_id,
                spec,
                legacy_transport_barrier,
            } => {
                self.pending_transport_activation = Some(PendingTimelineTransportActivation {
                    request_id,
                    spec,
                    legacy_transport_barrier,
                });
            }
            TimelineRuntimeCommand::Clear {
                request_id,
                revision,
            } => self.apply_clear(request_id, revision),
            TimelineRuntimeCommand::Shutdown { request_id } => self.apply_shutdown(request_id),
        }
    }

    fn apply_install(
        &mut self,
        request_id: u64,
        revision: u64,
        timeline: Arc<CompiledTimeline>,
        mixer_delay_bank: TimelineMixerDelayBankInstall,
    ) {
        let rejection = if revision == 0 {
            Some(TimelineRejectReason::InvalidRevision)
        } else if revision <= self.latest_revision {
            Some(TimelineRejectReason::StaleRevision {
                latest_revision: self.latest_revision,
            })
        } else {
            None
        };
        if let Some(reason) = rejection {
            self.retire_resource(RetiredTimelineResource::Timeline {
                revision,
                timeline,
                mixer_delay_bank: mixer_delay_bank.into_prepared(),
            });
            self.reject(request_id, revision, reason);
            return;
        }

        if self.transport.is_some() {
            // Keep the active bundle/cursor untouched until the replacement's
            // one-shot discontinuity atomically promotes all candidate state.
            let (mixer_delay_bank, mixer_delay_bank_reuse_fingerprint) = match mixer_delay_bank {
                TimelineMixerDelayBankInstall::LegacyNone => (None, None),
                TimelineMixerDelayBankInstall::Prepared(bank) => (Some(bank), None),
                TimelineMixerDelayBankInstall::ReuseActive { fingerprint } => {
                    if !self.mixer_delay_bank_reuse_available(fingerprint) {
                        self.retire_resource(RetiredTimelineResource::Timeline {
                            revision,
                            timeline,
                            mixer_delay_bank: None,
                        });
                        self.reject(
                            request_id,
                            revision,
                            TimelineRejectReason::MixerDelayBankReuseUnavailable { fingerprint },
                        );
                        return;
                    }
                    let candidate_bank = self
                        .candidate_timeline
                        .as_mut()
                        .filter(|candidate| {
                            candidate
                                .mixer_delay_bank
                                .as_ref()
                                .is_some_and(|bank| bank.graph_fingerprint() == fingerprint)
                        })
                        .and_then(|candidate| candidate.mixer_delay_bank.take());
                    if let Some(bank) = candidate_bank {
                        (Some(bank), None)
                    } else {
                        (None, Some(fingerprint))
                    }
                }
            };
            self.retire_candidate_bundle();
            self.candidate_timeline = Some(InstalledTimeline {
                revision,
                timeline,
                mixer_delay_bank,
                mixer_delay_bank_reuse_fingerprint,
            });
        } else {
            let (mixer_delay_bank, mixer_delay_bank_reuse_fingerprint) = match mixer_delay_bank {
                TimelineMixerDelayBankInstall::LegacyNone => (None, None),
                TimelineMixerDelayBankInstall::Prepared(bank) => (Some(bank), None),
                TimelineMixerDelayBankInstall::ReuseActive { fingerprint } => {
                    let Some(bank) = self.timeline.as_mut().and_then(|active| {
                        active
                            .mixer_delay_bank
                            .as_ref()
                            .filter(|bank| bank.graph_fingerprint() == fingerprint)?;
                        active.mixer_delay_bank.take()
                    }) else {
                        self.retire_resource(RetiredTimelineResource::Timeline {
                            revision,
                            timeline,
                            mixer_delay_bank: None,
                        });
                        self.reject(
                            request_id,
                            revision,
                            TimelineRejectReason::MixerDelayBankReuseUnavailable { fingerprint },
                        );
                        return;
                    };
                    (Some(bank), None)
                }
            };
            self.retire_active_bundle();
            self.timeline = Some(InstalledTimeline {
                revision,
                timeline,
                mixer_delay_bank,
                mixer_delay_bank_reuse_fingerprint,
            });
            self.transport = None;
            self.active_discontinuity = None;
            self.clear_published_active_epoch();
        }
        self.latest_revision = revision;
        self.push_event(TimelineRuntimeEvent::Installed {
            request_id,
            revision,
        });
    }

    fn mixer_delay_bank_reuse_available(&self, fingerprint: u64) -> bool {
        if fingerprint == 0 {
            return false;
        }
        if self.transport.is_some()
            && self.candidate_timeline.as_ref().is_some_and(|candidate| {
                candidate
                    .mixer_delay_bank
                    .as_ref()
                    .is_some_and(|bank| bank.graph_fingerprint() == fingerprint)
                    || candidate.mixer_delay_bank_reuse_fingerprint == Some(fingerprint)
            })
        {
            return true;
        }
        self.timeline.as_ref().is_some_and(|active| {
            active
                .mixer_delay_bank
                .as_ref()
                .is_some_and(|bank| bank.graph_fingerprint() == fingerprint)
        })
    }

    fn apply_chase(&mut self, request_id: u64, chase: Box<PreparedTimelineChase>) {
        if let Some(reason) = self.chase_rejection(&chase) {
            let revision = chase.revision;
            let epoch = chase.epoch;
            self.retire_resource(RetiredTimelineResource::Chase {
                revision,
                epoch,
                chase,
            });
            self.reject(request_id, revision, reason);
            return;
        }

        let revision = chase.revision;
        let epoch = chase.epoch;
        let frame = chase.frame;
        let installed = InstalledOneShotChase {
            chase,
            consumed: false,
        };
        let old = if self
            .candidate_timeline
            .as_ref()
            .is_some_and(|timeline| timeline.revision == revision)
        {
            self.candidate_one_shot_chase.replace(installed)
        } else {
            self.one_shot_chase.replace(installed)
        };
        if let Some(old) = old {
            self.retire_resource(RetiredTimelineResource::Chase {
                revision: old.chase.revision,
                epoch: old.chase.epoch,
                chase: old.chase,
            });
        }
        self.push_event(TimelineRuntimeEvent::ChaseInstalled {
            request_id,
            revision,
            epoch,
            frame,
        });
    }

    fn apply_loop_chase(&mut self, request_id: u64, chase: Box<PreparedLoopTimelineChase>) {
        if let Some(reason) = self.loop_chase_rejection(&chase) {
            let revision = chase.revision;
            let frame = chase.frame;
            self.retire_resource(RetiredTimelineResource::LoopChase {
                revision,
                frame,
                token: request_id,
                chase,
            });
            self.reject(request_id, revision, reason);
            return;
        }

        let revision = chase.revision;
        let frame = chase.frame;
        let installed = InstalledLoopChase {
            chase,
            token: request_id,
        };
        let old = if self
            .candidate_timeline
            .as_ref()
            .is_some_and(|timeline| timeline.revision == revision)
        {
            self.candidate_loop_chase.replace(installed)
        } else {
            self.loop_chase.replace(installed)
        };
        if let Some(old) = old {
            self.retire_resource(RetiredTimelineResource::LoopChase {
                revision: old.chase.revision,
                frame: old.chase.frame,
                token: old.token,
                chase: old.chase,
            });
        }
        self.push_event(TimelineRuntimeEvent::LoopChaseInstalled {
            request_id,
            revision,
            frame,
            token: request_id,
        });
    }

    fn apply_clear(&mut self, request_id: u64, revision: u64) {
        if self.active_revision() != Some(revision) && self.resident_revision() != Some(revision) {
            self.reject(
                request_id,
                revision,
                TimelineRejectReason::RevisionMismatch {
                    active_revision: self.resident_revision(),
                },
            );
            return;
        }
        // A confirmed handoff clears both the active scheduler and any staged
        // replacement so a failed B revision cannot outlive cleared A ownership.
        self.retire_candidate_bundle();
        self.retire_active_bundle();
        self.transport = None;
        self.active_discontinuity = None;
        self.clear_published_active_epoch();
        self.push_event(TimelineRuntimeEvent::Cleared {
            request_id,
            revision,
        });
    }

    fn apply_shutdown(&mut self, request_id: u64) {
        self.retire_candidate_bundle();
        self.retire_active_bundle();
        self.transport = None;
        self.active_discontinuity = None;
        self.clear_published_active_epoch();
        let revision = self.latest_revision;
        self.shutdown = true;
        if self.push_event(TimelineRuntimeEvent::ShutdownComplete {
            request_id,
            revision,
        }) {
            self.shared.shutdown_complete.store(true, Ordering::Release);
        }
    }

    fn chase_rejection(&self, chase: &PreparedTimelineChase) -> Option<TimelineRejectReason> {
        let resident_revision = self.resident_revision();
        if resident_revision != Some(chase.revision) {
            return Some(TimelineRejectReason::RevisionMismatch {
                active_revision: resident_revision,
            });
        }
        let targets_candidate = self
            .candidate_timeline
            .as_ref()
            .is_some_and(|timeline| timeline.revision == chase.revision);
        if !targets_candidate
            && self.one_shot_chase.is_some()
            && self.active_discontinuity.is_some_and(|active| {
                active.kind == TimelineDiscontinuityKind::OneShot && !active.delivered
            })
        {
            return Some(TimelineRejectReason::ChasePendingDelivery);
        }
        let newest_epoch = if targets_candidate {
            self.candidate_one_shot_chase
                .as_ref()
                .map(|installed| installed.chase.epoch)
        } else {
            self.latest_epoch
                .into_iter()
                .chain(
                    self.one_shot_chase
                        .as_ref()
                        .map(|installed| installed.chase.epoch),
                )
                .max()
        };
        if newest_epoch.is_some_and(|latest_epoch| chase.epoch <= latest_epoch) {
            return Some(TimelineRejectReason::StaleEpoch {
                latest_epoch: newest_epoch.expect("checked epoch"),
            });
        }
        let timeline = &self
            .resident_timeline()
            .expect("matched resident revision")
            .timeline;
        if chase.frame != chase.state.frame || chase.frame > timeline.duration_frames() {
            return Some(TimelineRejectReason::InvalidChaseFrame);
        }
        None
    }

    fn loop_chase_rejection(
        &self,
        chase: &PreparedLoopTimelineChase,
    ) -> Option<TimelineRejectReason> {
        let resident_revision = self.resident_revision();
        if resident_revision != Some(chase.revision) {
            return Some(TimelineRejectReason::RevisionMismatch {
                active_revision: resident_revision,
            });
        }
        let targets_candidate = self
            .candidate_timeline
            .as_ref()
            .is_some_and(|timeline| timeline.revision == chase.revision);
        if !targets_candidate
            && self.loop_chase.is_some()
            && self.active_discontinuity.is_some_and(|active| {
                matches!(active.kind, TimelineDiscontinuityKind::Loop { .. }) && !active.delivered
            })
        {
            return Some(TimelineRejectReason::ChasePendingDelivery);
        }
        let timeline = &self
            .resident_timeline()
            .expect("matched resident revision")
            .timeline;
        if chase.frame != chase.state.frame || chase.frame > timeline.duration_frames() {
            return Some(TimelineRejectReason::InvalidChaseFrame);
        }
        None
    }

    fn reject(&mut self, request_id: u64, revision: u64, reason: TimelineRejectReason) {
        self.shared
            .rejected_requests
            .fetch_add(1, Ordering::Relaxed);
        self.push_event(TimelineRuntimeEvent::Rejected {
            request_id,
            revision,
            reason,
        });
    }

    fn retire_chase_pair(
        &mut self,
        one_shot: Option<InstalledOneShotChase>,
        loop_chase: Option<InstalledLoopChase>,
    ) {
        let one_shot = one_shot.map(|installed| installed.chase);
        let loop_chase = loop_chase.map(|installed| (installed.token, installed.chase));
        match (one_shot, loop_chase) {
            (Some(one_shot), Some((token, loop_chase))) => {
                self.retire_resource(RetiredTimelineResource::ChaseBundle {
                    one_shot: Some(one_shot),
                    loop_token: Some(token),
                    loop_chase: Some(loop_chase),
                });
            }
            (Some(chase), None) => {
                self.retire_resource(RetiredTimelineResource::Chase {
                    revision: chase.revision,
                    epoch: chase.epoch,
                    chase,
                });
            }
            (None, Some((token, chase))) => {
                self.retire_resource(RetiredTimelineResource::LoopChase {
                    revision: chase.revision,
                    frame: chase.frame,
                    token,
                    chase,
                });
            }
            (None, None) => {}
        }
    }

    fn retire_active_bundle(&mut self) {
        let timeline = self.timeline.take();
        let one_shot = self.one_shot_chase.take();
        let loop_chase = self.loop_chase.take();
        self.retire_bundle_parts(timeline, one_shot, loop_chase);
    }

    fn retire_candidate_bundle(&mut self) {
        let timeline = self.candidate_timeline.take();
        let one_shot = self.candidate_one_shot_chase.take();
        let loop_chase = self.candidate_loop_chase.take();
        self.retire_bundle_parts(timeline, one_shot, loop_chase);
    }

    fn retire_bundle_parts(
        &mut self,
        timeline: Option<InstalledTimeline>,
        one_shot: Option<InstalledOneShotChase>,
        loop_chase: Option<InstalledLoopChase>,
    ) {
        let Some(timeline) = timeline else {
            self.retire_chase_pair(one_shot, loop_chase);
            return;
        };
        let (loop_token, loop_chase) = loop_chase.map_or((None, None), |installed| {
            (Some(installed.token), Some(installed.chase))
        });
        self.retire_resource(RetiredTimelineResource::Bundle {
            revision: timeline.revision,
            timeline: timeline.timeline,
            mixer_delay_bank: timeline.mixer_delay_bank,
            one_shot: one_shot.map(|installed| installed.chase),
            loop_token,
            loop_chase,
        });
    }

    fn promote_candidate(&mut self, revision: u64) {
        debug_assert!(self.deferred_retire.is_none());
        let mut candidate = self
            .candidate_timeline
            .take()
            .expect("validated candidate timeline must remain resident");
        debug_assert_eq!(candidate.revision, revision);
        if let Some(fingerprint) = candidate.mixer_delay_bank_reuse_fingerprint.take() {
            let bank = self
                .timeline
                .as_mut()
                .and_then(|active| {
                    active
                        .mixer_delay_bank
                        .as_ref()
                        .filter(|bank| bank.graph_fingerprint() == fingerprint)?;
                    active.mixer_delay_bank.take()
                })
                .expect("preflighted mixer delay bank reuse remains valid");
            candidate.mixer_delay_bank = Some(bank);
        }
        let old_timeline = self
            .timeline
            .replace(candidate)
            .expect("candidate replacement requires an active timeline");
        let candidate_one_shot = self.candidate_one_shot_chase.take();
        let candidate_loop = self.candidate_loop_chase.take();
        let old_one_shot = std::mem::replace(&mut self.one_shot_chase, candidate_one_shot);
        let old_loop = std::mem::replace(&mut self.loop_chase, candidate_loop);
        self.deferred_retire = Some(DeferredTimelineRetire {
            timeline: old_timeline,
            one_shot_chase: old_one_shot,
            loop_chase: old_loop,
        });
        self.transport = None;
        self.active_discontinuity = None;
    }

    fn flush_deferred_retire(&mut self) -> bool {
        if self.deferred_retire.is_none() {
            return true;
        }
        if self.retired.slots() == 0 {
            return false;
        }
        let deferred = self
            .deferred_retire
            .take()
            .expect("deferred retire was inspected above");
        self.retire_bundle_parts(
            Some(deferred.timeline),
            deferred.one_shot_chase,
            deferred.loop_chase,
        );
        true
    }

    fn retire_resource(&mut self, resource: RetiredTimelineResource) {
        if let Err(PushError::Full(resource)) = self.retired.push(resource) {
            self.shared
                .ownership_needs_resync
                .store(true, Ordering::Release);
            // Defensive last resort: destroying this value here would run Vec/Box
            // destructors on the callback. Capacity is preflighted, so this branch
            // is unreachable for the SPSC topology unless its invariant changes.
            mem::forget(resource);
        }
    }

    fn push_event(&mut self, event: TimelineRuntimeEvent) -> bool {
        if self.events.push(event).is_ok() {
            true
        } else {
            self.shared
                .ownership_needs_resync
                .store(true, Ordering::Release);
            false
        }
    }
}

impl Drop for RealtimeTimelineRuntime {
    fn drop(&mut self) {
        self.shared.realtime_alive.store(false, Ordering::Release);
        if self.shutdown {
            return;
        }
        self.shared
            .unexpected_realtime_drops
            .fetch_add(1, Ordering::Relaxed);
        self.shared
            .ownership_needs_resync
            .store(true, Ordering::Release);
        self.clear_published_active_epoch();
        if let Some(installed) = self.timeline.take() {
            mem::forget(installed.timeline);
            if let Some(bank) = installed.mixer_delay_bank {
                mem::forget(bank);
            }
        }
        if let Some(chase) = self.one_shot_chase.take() {
            mem::forget(chase.chase);
        }
        if let Some(chase) = self.loop_chase.take() {
            mem::forget(chase.chase);
        }
        if let Some(installed) = self.candidate_timeline.take() {
            mem::forget(installed.timeline);
            if let Some(bank) = installed.mixer_delay_bank {
                mem::forget(bank);
            }
        }
        if let Some(chase) = self.candidate_one_shot_chase.take() {
            mem::forget(chase.chase);
        }
        if let Some(chase) = self.candidate_loop_chase.take() {
            mem::forget(chase.chase);
        }
        if let Some(deferred) = self.deferred_retire.take() {
            mem::forget(deferred.timeline.timeline);
            if let Some(bank) = deferred.timeline.mixer_delay_bank {
                mem::forget(bank);
            }
            if let Some(chase) = deferred.one_shot_chase {
                mem::forget(chase.chase);
            }
            if let Some(chase) = deferred.loop_chase {
                mem::forget(chase.chase);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum TimelineRuntimeQueryError {
    #[error("no compiled timeline is active")]
    MissingTimeline,
    #[error("timeline runtime has no installed epoch/chase")]
    EpochUnavailable,
    #[error("requested timeline revision {requested} does not match active revision {active}")]
    RevisionMismatch { active: u64, requested: u64 },
    #[error("requested epoch {requested} does not match active epoch {active}")]
    EpochMismatch { active: u64, requested: u64 },
    #[error("requested block starts at {requested}, expected {expected}")]
    StartFrameMismatch { expected: u64, requested: u64 },
    #[error("the epoch chase must be read before its first event packet")]
    ChaseRequired,
    #[error("an uncommitted timeline block is already active")]
    ChunkInProgress,
    #[error("timeline ownership or partial-block state requires an explicit resync")]
    NeedsResync,
    #[error("timeline runtime is shut down")]
    Shutdown,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum TimelineRuntimePacketError {
    #[error(transparent)]
    Query(#[from] TimelineRuntimeQueryError),
    #[error(transparent)]
    Packet(#[from] TimelinePacketError),
    #[error("chunked timeline block is already complete")]
    BlockAlreadyComplete,
    #[error("not every event chunk has been copied")]
    BlockCopyIncomplete,
}

/// Commit guard for one fixed-capacity packet copy. Packetization only copies
/// immutable events; the cursor advances exclusively when the caller confirms
/// that downstream execution succeeded by consuming this guard with `commit`.
pub struct PreparedTimelineBlock<'a> {
    next_frame: &'a mut u64,
    block_in_progress: &'a mut bool,
    shared: &'a TimelineRuntimeShared,
    end_frame: u64,
    committed: bool,
}

impl PreparedTimelineBlock<'_> {
    pub fn commit(mut self) {
        *self.next_frame = self.end_frame;
        *self.block_in_progress = false;
        self.committed = true;
    }

    pub fn abort(mut self) {
        self.mark_resync();
        self.committed = true;
    }

    fn mark_resync(&mut self) {
        *self.block_in_progress = false;
        self.shared
            .ownership_needs_resync
            .store(true, Ordering::Release);
    }
}

impl Drop for PreparedTimelineBlock<'_> {
    fn drop(&mut self) {
        if !self.committed {
            self.mark_resync();
        }
    }
}

/// Borrowed allocation-free cursor for one validated revision/epoch block.
pub struct RealtimeTimelineBlock<'a> {
    range: TimelineEventRange<'a>,
    next_frame: &'a mut u64,
    block_in_progress: &'a mut bool,
    shared: &'a TimelineRuntimeShared,
    end_frame: u64,
    copy_complete: bool,
    committed: bool,
}

impl RealtimeTimelineBlock<'_> {
    #[must_use]
    pub fn remaining_events(&self) -> usize {
        self.range.remaining_events()
    }

    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.copy_complete
    }

    pub fn packetize_next_into<const CAPACITY: usize>(
        &mut self,
        packet: &mut TimelinePacket<CAPACITY>,
    ) -> Result<TimelinePacketChunk, TimelineRuntimePacketError> {
        if self.copy_complete {
            return Err(TimelineRuntimePacketError::BlockAlreadyComplete);
        }
        let chunk = self.range.packetize_next_into(packet)?;
        if chunk.remaining_events == 0 {
            self.copy_complete = true;
        }
        Ok(chunk)
    }

    pub fn commit(mut self) -> Result<(), TimelineRuntimePacketError> {
        if !self.copy_complete {
            return Err(TimelineRuntimePacketError::BlockCopyIncomplete);
        }
        *self.next_frame = self.end_frame;
        *self.block_in_progress = false;
        self.committed = true;
        Ok(())
    }

    pub fn abort(mut self) {
        self.mark_resync();
        self.committed = true;
    }

    fn mark_resync(&mut self) {
        *self.block_in_progress = false;
        self.shared
            .ownership_needs_resync
            .store(true, Ordering::Release);
    }
}

impl Drop for RealtimeTimelineBlock<'_> {
    fn drop(&mut self) {
        if !self.committed {
            self.mark_resync();
        }
    }
}

pub fn create_timeline_runtime() -> (TimelineRuntimeController, RealtimeTimelineRuntime) {
    create_timeline_runtime_with_capacities(
        DEFAULT_TIMELINE_RUNTIME_COMMAND_CAPACITY,
        DEFAULT_TIMELINE_RUNTIME_EVENT_CAPACITY,
        DEFAULT_TIMELINE_RUNTIME_RETIRE_CAPACITY,
    )
    .expect("default timeline runtime capacities are valid")
}

pub fn create_timeline_runtime_with_capacities(
    command_capacity: usize,
    event_capacity: usize,
    retire_capacity: usize,
) -> Result<(TimelineRuntimeController, RealtimeTimelineRuntime), TimelineRuntimeCreateError> {
    if command_capacity == 0 || event_capacity == 0 {
        return Err(TimelineRuntimeCreateError::InvalidQueueCapacity);
    }
    if retire_capacity < MAX_RETIRES_PER_COMMAND {
        return Err(TimelineRuntimeCreateError::RetireCapacityTooSmall);
    }
    let (command_producer, command_consumer) = RingBuffer::new(command_capacity);
    let (event_producer, event_consumer) = RingBuffer::new(event_capacity);
    let (retire_producer, retire_consumer) = RingBuffer::new(retire_capacity);
    let shared = Arc::new(TimelineRuntimeShared::default());
    shared.realtime_alive.store(true, Ordering::Release);
    let controller = TimelineRuntimeController {
        commands: command_producer,
        events: event_consumer,
        retired: retire_consumer,
        shared: shared.clone(),
        next_request_id: 1,
        last_queued_request: 0,
        last_confirmed_request: 0,
        resident_revision: None,
        resident_one_shot_epoch: None,
        rejected_since_resync: false,
        shutdown_request: None,
        shutdown_confirmed: false,
    };
    let realtime = RealtimeTimelineRuntime {
        commands: command_consumer,
        events: event_producer,
        retired: retire_producer,
        shared,
        timeline: None,
        one_shot_chase: None,
        loop_chase: None,
        candidate_timeline: None,
        candidate_one_shot_chase: None,
        candidate_loop_chase: None,
        deferred_retire: None,
        active_discontinuity: None,
        transport: None,
        pending_transport_activation: None,
        latest_revision: 0,
        latest_epoch: None,
        shutdown: false,
    };
    Ok((controller, realtime))
}

fn transport_activation_spec_is_valid(spec: TimelineTransportActivationSpec) -> bool {
    spec.revision != 0
        && spec.target_epoch != 0
        && spec.minimum_epoch != 0
        && spec.target_epoch >= spec.minimum_epoch
        && spec.loop_token != 0
        && spec.mixer_pan_release.is_valid()
        && (!spec.loop_enabled
            || (spec.loop_end_frame > spec.loop_start_frame
                && spec.loop_end_q32 > spec.loop_start_q32))
}

fn validate_timeline(
    revision: u64,
    timeline: &CompiledTimeline,
) -> Result<(), TimelineRuntimeValidationError> {
    if revision == 0 {
        return Err(TimelineRuntimeValidationError::InvalidRevision);
    }
    validate_resource(
        TimelineRuntimeResource::TimelineEvents,
        timeline.events().len(),
        MAX_TIMELINE_EVENTS,
    )?;
    validate_resource(
        TimelineRuntimeResource::TimelineDiagnostics,
        timeline.diagnostics().len(),
        MAX_TIMELINE_DIAGNOSTICS,
    )?;
    validate_resource(
        TimelineRuntimeResource::TimelineAudioClips,
        timeline.audio_clips().len(),
        MAX_RUNTIME_AUDIO_CLIPS,
    )?;
    validate_resource(
        TimelineRuntimeResource::TimelineAutomationBases,
        timeline.automation_bases().len(),
        MAX_RUNTIME_AUTOMATION_BASES,
    )?;
    validate_resource(
        TimelineRuntimeResource::TimelineChannelBases,
        timeline.channel_bases().len(),
        MAX_RUNTIME_CHANNEL_BASES,
    )?;
    let referenced_audio_assets = timeline
        .audio_clips()
        .iter()
        .map(|clip| clip.asset_id)
        .collect::<BTreeSet<_>>()
        .len();
    validate_resource(
        TimelineRuntimeResource::ReferencedAudioAssets,
        referenced_audio_assets,
        TIMELINE_CALLBACK_MAX_AUDIO_ASSETS,
    )?;
    let endpoint_routes = EndpointRoutes::from_timeline(timeline)?;
    let generator_routes = timeline
        .plugin_routes()
        .iter()
        .filter(|route| matches!(route.destination, PluginRouteDestination::Generator { .. }))
        .count();
    validate_resource(
        TimelineRuntimeResource::GeneratorRoutes,
        generator_routes,
        TIMELINE_CALLBACK_MAX_GENERATOR_ENDPOINTS,
    )?;
    validate_callback_event_capacities(timeline, &endpoint_routes)?;
    validate_executor_event_capacities(timeline)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct WindowMaximum {
    start_frame: u64,
    events: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EventFrameGroup {
    frame: u64,
    events: usize,
}

#[derive(Debug, Default)]
struct EndpointRoutes {
    by_instance: BTreeMap<u64, PluginRouteDestination>,
    generator_by_channel: BTreeMap<u32, TimelineCallbackEndpoint>,
}

impl EndpointRoutes {
    fn from_timeline(timeline: &CompiledTimeline) -> Result<Self, TimelineRuntimeValidationError> {
        Self::from_routes(timeline.plugin_routes())
    }

    fn from_routes(
        compiled_routes: &[CompiledPluginRoute],
    ) -> Result<Self, TimelineRuntimeValidationError> {
        let mut routes = Self::default();
        let mut physical_slots = BTreeMap::<(TimelineCallbackEndpoint, u8), u64>::new();
        for route in compiled_routes {
            if route.instance_id == 0 || !valid_plugin_route_destination(route.destination) {
                return Err(TimelineRuntimeValidationError::InvalidPluginRoute {
                    instance_id: route.instance_id,
                    destination: route.destination,
                });
            }
            if routes.by_instance.contains_key(&route.instance_id) {
                return Err(TimelineRuntimeValidationError::AmbiguousPluginRoute {
                    instance_id: route.instance_id,
                });
            }
            let endpoint = TimelineCallbackEndpoint::from(route.destination);
            let slot = match route.destination {
                PluginRouteDestination::Generator { slot, .. }
                | PluginRouteDestination::MixerInsert { slot, .. } => slot,
            };
            if let Some(first_instance_id) =
                physical_slots.insert((endpoint, slot), route.instance_id)
            {
                return Err(
                    TimelineRuntimeValidationError::DuplicatePhysicalPluginSlot {
                        endpoint,
                        slot,
                        first_instance_id,
                        second_instance_id: route.instance_id,
                    },
                );
            }
            routes
                .by_instance
                .insert(route.instance_id, route.destination);
            if let TimelineCallbackEndpoint::Generator { channel_id } = endpoint {
                routes.generator_by_channel.insert(channel_id, endpoint);
            }
        }

        let mut current_track = None;
        let mut expected_slot = 0_u8;
        for &(endpoint, actual_slot) in physical_slots.keys() {
            let TimelineCallbackEndpoint::MixerInsert { track } = endpoint else {
                continue;
            };
            if current_track != Some(track) {
                current_track = Some(track);
                expected_slot = 0;
            }
            if actual_slot != expected_slot {
                return Err(TimelineRuntimeValidationError::NonDensePluginMixerChain {
                    track,
                    expected_slot,
                    actual_slot,
                });
            }
            expected_slot = expected_slot.saturating_add(1);
        }
        Ok(routes)
    }

    fn event_endpoint(&self, kind: TimelineEventKind) -> Option<TimelineCallbackEndpoint> {
        match kind {
            TimelineEventKind::NoteOn { channel_id, .. }
            | TimelineEventKind::NoteOff { channel_id, .. } => {
                self.generator_by_channel.get(&channel_id).copied()
            }
            TimelineEventKind::AutomationRamp(_)
            | TimelineEventKind::AutomationEnd { .. }
            | TimelineEventKind::AudioStart(_)
            | TimelineEventKind::AudioStop { .. } => None,
        }
    }

    fn automation_endpoint(
        &self,
        target: CompiledAutomationTarget,
    ) -> Result<Option<TimelineCallbackEndpoint>, TimelineRuntimeValidationError> {
        let CompiledAutomationTarget::PluginParameter {
            instance_id,
            parameter_id,
        } = target
        else {
            return Ok(None);
        };
        match self.by_instance.get(&instance_id).copied() {
            Some(PluginRouteDestination::Generator {
                channel_id,
                slot: 0,
            }) => Ok(Some(TimelineCallbackEndpoint::Generator { channel_id })),
            Some(PluginRouteDestination::MixerInsert { track, .. }) => {
                Ok(Some(TimelineCallbackEndpoint::MixerInsert { track }))
            }
            Some(destination) => Err(TimelineRuntimeValidationError::InvalidPluginRoute {
                instance_id,
                destination,
            }),
            None => Err(
                TimelineRuntimeValidationError::PluginAutomationRouteUnavailable {
                    instance_id,
                    parameter_id,
                },
            ),
        }
    }

    fn physical_endpoints(&self) -> impl Iterator<Item = TimelineCallbackEndpoint> + '_ {
        self.by_instance
            .values()
            .copied()
            .map(TimelineCallbackEndpoint::from)
    }
}

fn valid_plugin_route_destination(destination: PluginRouteDestination) -> bool {
    match destination {
        PluginRouteDestination::Generator { channel_id, slot } => channel_id != 0 && slot == 0,
        PluginRouteDestination::MixerInsert { track, slot } => {
            usize::from(track) < TIMELINE_CALLBACK_MAX_MIXER_ENDPOINTS
                && usize::from(slot) < MIXER_INSERT_SLOT_COUNT
        }
    }
}

fn validate_callback_event_capacities(
    timeline: &CompiledTimeline,
    routes: &EndpointRoutes,
) -> Result<(), TimelineRuntimeValidationError> {
    // Deliberately count every TimelineEvent, including automation. This is a
    // conservative contract for the shared packet/executor transaction and
    // reserves capacity before sample-offset automation delivery is enabled.
    validate_global_callback_event_capacity(timeline.events())?;

    let driven_parameters = driven_plugin_parameter_counts(timeline, routes)?;
    let endpoint_events = endpoint_event_groups(timeline.events(), routes);
    validate_endpoint_event_capacities(&endpoint_events, &driven_parameters)
}

fn validate_endpoint_event_capacities(
    endpoint_events: &BTreeMap<TimelineCallbackEndpoint, Vec<EventFrameGroup>>,
    driven_parameters: &BTreeMap<TimelineCallbackEndpoint, usize>,
) -> Result<(), TimelineRuntimeValidationError> {
    let endpoints = endpoint_events
        .keys()
        .chain(driven_parameters.keys())
        .copied()
        .collect::<BTreeSet<_>>();
    for endpoint in endpoints {
        let groups = endpoint_events
            .get(&endpoint)
            .map_or(&[][..], Vec::as_slice);
        let parameter_count = driven_parameters.get(&endpoint).copied().unwrap_or(0);
        let quantum = maximum_grouped_event_window(groups, TIMELINE_PLUGIN_FIXED_QUANTUM_FRAMES);
        let quantum_events = quantum.events.saturating_add(parameter_count);
        if quantum_events > TIMELINE_ENDPOINT_TIMELINE_MAX_EVENTS_PER_QUANTUM {
            return Err(callback_capacity_error(
                TimelineRuntimeResource::EndpointEvents {
                    endpoint,
                    window_frames: TIMELINE_PLUGIN_FIXED_QUANTUM_FRAMES as u16,
                },
                WindowMaximum {
                    events: quantum_events,
                    ..quantum
                },
                TIMELINE_ENDPOINT_TIMELINE_MAX_EVENTS_PER_QUANTUM,
            ));
        }
        let callback = maximum_grouped_event_window(groups, TIMELINE_CALLBACK_MAX_FRAMES);
        let callback_events = callback.events.saturating_add(
            parameter_count.saturating_mul(TIMELINE_PLUGIN_FIXED_QUANTA_PER_CALLBACK),
        );
        if callback_events > TIMELINE_ENDPOINT_TIMELINE_MAX_EVENTS_PER_CALLBACK {
            return Err(callback_capacity_error(
                TimelineRuntimeResource::EndpointEvents {
                    endpoint,
                    window_frames: TIMELINE_CALLBACK_MAX_FRAMES as u16,
                },
                WindowMaximum {
                    events: callback_events,
                    ..callback
                },
                TIMELINE_ENDPOINT_TIMELINE_MAX_EVENTS_PER_CALLBACK,
            ));
        }
    }
    Ok(())
}

fn driven_plugin_parameter_counts(
    timeline: &CompiledTimeline,
    routes: &EndpointRoutes,
) -> Result<BTreeMap<TimelineCallbackEndpoint, usize>, TimelineRuntimeValidationError> {
    driven_plugin_parameter_counts_for_targets(timeline.driven_automation_targets(), routes)
}

fn driven_plugin_parameter_counts_for_targets(
    targets: &[CompiledAutomationTarget],
    routes: &EndpointRoutes,
) -> Result<BTreeMap<TimelineCallbackEndpoint, usize>, TimelineRuntimeValidationError> {
    let mut counts = BTreeMap::<TimelineCallbackEndpoint, usize>::new();
    for &target in targets {
        let Some(endpoint) = routes.automation_endpoint(target)? else {
            continue;
        };
        let count = counts.entry(endpoint).or_default();
        *count = count.saturating_add(1);
        validate_resource(
            TimelineRuntimeResource::EndpointDrivenPluginParameters { endpoint },
            *count,
            TIMELINE_ENDPOINT_MAX_DRIVEN_PLUGIN_PARAMETERS,
        )?;
    }
    let total = counts
        .values()
        .fold(0_usize, |total, &count| total.saturating_add(count));
    validate_resource(
        TimelineRuntimeResource::DrivenPluginParameters,
        total,
        TIMELINE_MAX_DRIVEN_PLUGIN_PARAMETERS,
    )?;
    Ok(counts)
}

fn validate_global_callback_event_capacity(
    events: &[TimelineEvent],
) -> Result<(), TimelineRuntimeValidationError> {
    let maximum = maximum_event_window(events, TIMELINE_CALLBACK_MAX_FRAMES);
    if maximum.events > TIMELINE_CALLBACK_MAX_EVENTS {
        return Err(callback_capacity_error(
            TimelineRuntimeResource::CallbackEvents {
                window_frames: TIMELINE_CALLBACK_MAX_FRAMES as u16,
            },
            maximum,
            TIMELINE_CALLBACK_MAX_EVENTS,
        ));
    }
    Ok(())
}

fn maximum_event_window(events: &[TimelineEvent], window_frames: usize) -> WindowMaximum {
    debug_assert!(window_frames != 0);
    let window_frames = window_frames as u64;
    let mut maximum = WindowMaximum::default();
    let mut left = 0;
    let mut right = 0;
    while right < events.len() {
        let right_frame = events[right].frame;
        right += 1;
        while right < events.len() && events[right].frame == right_frame {
            right += 1;
        }
        let start_frame = right_frame.saturating_sub(window_frames - 1);
        while left < right && events[left].frame < start_frame {
            left += 1;
        }
        let count = right - left;
        if count > maximum.events {
            maximum = WindowMaximum {
                start_frame,
                events: count,
            };
        }
    }
    maximum
}

fn endpoint_event_groups(
    events: &[TimelineEvent],
    routes: &EndpointRoutes,
) -> BTreeMap<TimelineCallbackEndpoint, Vec<EventFrameGroup>> {
    let mut endpoints = BTreeMap::<TimelineCallbackEndpoint, Vec<EventFrameGroup>>::new();
    for event in events {
        let Some(endpoint) = routes.event_endpoint(event.kind) else {
            continue;
        };
        let groups = endpoints.entry(endpoint).or_default();
        if let Some(last) = groups.last_mut()
            && last.frame == event.frame
        {
            last.events = last.events.saturating_add(1);
        } else {
            groups.push(EventFrameGroup {
                frame: event.frame,
                events: 1,
            });
        }
    }
    endpoints
}

fn maximum_grouped_event_window(groups: &[EventFrameGroup], window_frames: usize) -> WindowMaximum {
    debug_assert!(window_frames != 0);
    let window_frames = window_frames as u64;
    let mut maximum = WindowMaximum::default();
    let mut left = 0;
    let mut count = 0_usize;
    for (right, group) in groups.iter().enumerate() {
        count = count.saturating_add(group.events);
        let start_frame = group.frame.saturating_sub(window_frames - 1);
        while left <= right && groups[left].frame < start_frame {
            count = count.saturating_sub(groups[left].events);
            left += 1;
        }
        if count > maximum.events {
            maximum = WindowMaximum {
                start_frame,
                events: count,
            };
        }
    }
    maximum
}

fn callback_capacity_error(
    resource: TimelineRuntimeResource,
    maximum: WindowMaximum,
    limit: usize,
) -> TimelineRuntimeValidationError {
    TimelineRuntimeValidationError::CallbackCapacityExceeded {
        resource,
        window_start_frame: maximum.start_frame,
        actual: maximum.events,
        maximum: limit,
    }
}

type AutomationLayerKey = (CompiledAutomationTarget, u64, u64, Option<u32>);

fn validate_executor_event_capacities(
    timeline: &CompiledTimeline,
) -> Result<(), TimelineRuntimeValidationError> {
    validate_executor_event_slice(timeline.events())
}

fn validate_executor_event_slice(
    events: &[TimelineEvent],
) -> Result<(), TimelineRuntimeValidationError> {
    let mut notes = BTreeSet::<u64>::new();
    let mut audio_clips = BTreeSet::<(u32, u64)>::new();
    let mut automation_layers = BTreeSet::<AutomationLayerKey>::new();

    for event in events {
        match event.kind {
            TimelineEventKind::NoteOn { note_id, .. } => {
                notes.insert(note_id);
                validate_resource(
                    TimelineRuntimeResource::ConcurrentNotes,
                    notes.len(),
                    EXECUTOR_MAX_ACTIVE_NOTES,
                )?;
            }
            TimelineEventKind::NoteOff { note_id, .. } => {
                notes.remove(&note_id);
            }
            TimelineEventKind::AudioStart(descriptor) => {
                audio_clips.insert((descriptor.clip_id, descriptor.asset_id));
                validate_resource(
                    TimelineRuntimeResource::ConcurrentAudioClips,
                    audio_clips.len(),
                    EXECUTOR_MAX_ACTIVE_AUDIO_CLIPS,
                )?;
            }
            TimelineEventKind::AudioStop { clip_id, asset_id } => {
                audio_clips.remove(&(clip_id, asset_id));
            }
            TimelineEventKind::AutomationRamp(ramp) => {
                automation_layers.insert((
                    ramp.target,
                    ramp.precedence,
                    ramp.automation_id,
                    ramp.placement_id,
                ));
                validate_resource(
                    TimelineRuntimeResource::ConcurrentAutomationLayers,
                    automation_layers.len(),
                    EXECUTOR_MAX_ACTIVE_AUTOMATION_LAYERS,
                )?;
            }
            TimelineEventKind::AutomationEnd {
                automation_id,
                placement_id,
                precedence,
                target,
            } => {
                automation_layers.remove(&(target, precedence, automation_id, placement_id));
            }
        }
    }
    Ok(())
}

fn validate_prepared_chase(
    chase: &PreparedTimelineChase,
) -> Result<(), TimelineRuntimeValidationError> {
    if chase.revision == 0 {
        return Err(TimelineRuntimeValidationError::InvalidRevision);
    }
    if chase.epoch == 0 {
        return Err(TimelineRuntimeValidationError::InvalidEpoch);
    }
    validate_chase_state(&chase.state)
}

fn validate_prepared_loop_chase(
    chase: &PreparedLoopTimelineChase,
) -> Result<(), TimelineRuntimeValidationError> {
    if chase.revision == 0 {
        return Err(TimelineRuntimeValidationError::InvalidRevision);
    }
    validate_chase_state(&chase.state)
}

fn validate_chase_state(
    state: &TimelineDiscontinuityState,
) -> Result<(), TimelineRuntimeValidationError> {
    validate_resource(
        TimelineRuntimeResource::ChaseAudioClips,
        state.audio_clips.len(),
        MAX_RUNTIME_CHASE_AUDIO_CLIPS,
    )?;
    validate_resource(
        TimelineRuntimeResource::ChaseAutomationLayers,
        state.automation_layers.len(),
        MAX_RUNTIME_CHASE_AUTOMATION_LAYERS,
    )?;
    validate_resource(
        TimelineRuntimeResource::ChaseAutomationBases,
        state.automation_bases.len(),
        MAX_RUNTIME_AUTOMATION_BASES,
    )?;
    validate_resource(
        TimelineRuntimeResource::ChaseNotes,
        state.notes.len(),
        MAX_RUNTIME_CHASE_NOTES,
    )
}

/// Validates the exact offset-zero batch produced by executor reset together
/// with normal events beginning at the discontinuity. Raw note-off events are
/// intentionally retained in the count even though the renderer may merge an
/// overlapping same-pitch off; this is a conservative, callback-safe bound.
fn validate_chase_endpoint_capacities(
    timeline: &CompiledTimeline,
    state: &TimelineDiscontinuityState,
) -> Result<(), TimelineRuntimeValidationError> {
    validate_chase_render_plan_capacity(timeline.events(), state)?;

    let routes = EndpointRoutes::from_timeline(timeline)?;
    let driven_parameters = driven_plugin_parameter_counts(timeline, &routes)?;
    let event_groups = endpoint_event_groups(timeline.events(), &routes);
    let chased_notes = chase_endpoint_note_counts(&routes, state);
    let endpoints = routes
        .physical_endpoints()
        .chain(event_groups.keys().copied())
        .chain(driven_parameters.keys().copied())
        .collect::<BTreeSet<_>>();

    // CC123 belongs exclusively to the system lane. It must never be added to
    // the sequenced note/parameter count below or charged a second time for the
    // callback window.
    for endpoint in endpoints {
        let groups = event_groups.get(&endpoint).map_or(&[][..], Vec::as_slice);
        let chased_note_count = chased_notes.get(&endpoint).copied().unwrap_or(0);
        let parameter_count = driven_parameters.get(&endpoint).copied().unwrap_or(0);
        let (quantum_events, callback_events) = chase_endpoint_timeline_event_counts(
            groups,
            state.frame,
            chased_note_count,
            parameter_count,
        );
        if quantum_events > TIMELINE_ENDPOINT_TIMELINE_MAX_EVENTS_PER_QUANTUM {
            return Err(TimelineRuntimeValidationError::CallbackCapacityExceeded {
                resource: TimelineRuntimeResource::EndpointEvents {
                    endpoint,
                    window_frames: TIMELINE_PLUGIN_FIXED_QUANTUM_FRAMES as u16,
                },
                window_start_frame: state.frame,
                actual: quantum_events,
                maximum: TIMELINE_ENDPOINT_TIMELINE_MAX_EVENTS_PER_QUANTUM,
            });
        }

        if callback_events > TIMELINE_ENDPOINT_TIMELINE_MAX_EVENTS_PER_CALLBACK {
            return Err(TimelineRuntimeValidationError::CallbackCapacityExceeded {
                resource: TimelineRuntimeResource::EndpointEvents {
                    endpoint,
                    window_frames: TIMELINE_CALLBACK_MAX_FRAMES as u16,
                },
                window_start_frame: state.frame,
                actual: callback_events,
                maximum: TIMELINE_ENDPOINT_TIMELINE_MAX_EVENTS_PER_CALLBACK,
            });
        }
    }
    Ok(())
}

fn chase_endpoint_timeline_event_counts(
    groups: &[EventFrameGroup],
    frame: u64,
    chased_event_count: usize,
    driven_parameter_count: usize,
) -> (usize, usize) {
    let quantum_events = chased_event_count
        .saturating_add(grouped_events_from(
            groups,
            frame,
            TIMELINE_PLUGIN_FIXED_QUANTUM_FRAMES,
        ))
        // The chased parameter value is the first Q128 sample-and-hold value,
        // not a second event in addition to the normal boundary sample.
        .saturating_add(driven_parameter_count);
    let callback_events = chased_event_count
        .saturating_add(grouped_events_from(
            groups,
            frame,
            TIMELINE_CALLBACK_MAX_FRAMES,
        ))
        .saturating_add(
            driven_parameter_count.saturating_mul(TIMELINE_PLUGIN_FIXED_QUANTA_PER_CALLBACK),
        );
    (quantum_events, callback_events)
}

fn validate_chase_render_plan_capacity(
    events: &[TimelineEvent],
    state: &TimelineDiscontinuityState,
) -> Result<(), TimelineRuntimeValidationError> {
    // Executor reset writes chased audio/note resources into the same render
    // plan that subsequently receives normal events at `state.frame`.
    let chase_sink_events = state.audio_clips.len().saturating_add(state.notes.len());
    let callback_sink_events = chase_sink_events.saturating_add(timeline_sink_events_from(
        events,
        state.frame,
        TIMELINE_CALLBACK_MAX_FRAMES,
    ));
    if callback_sink_events > TIMELINE_CALLBACK_MAX_EVENTS {
        Err(TimelineRuntimeValidationError::CallbackCapacityExceeded {
            resource: TimelineRuntimeResource::CallbackEvents {
                window_frames: TIMELINE_CALLBACK_MAX_FRAMES as u16,
            },
            window_start_frame: state.frame,
            actual: callback_sink_events,
            maximum: TIMELINE_CALLBACK_MAX_EVENTS,
        })
    } else {
        Ok(())
    }
}

fn chase_endpoint_note_counts(
    routes: &EndpointRoutes,
    state: &TimelineDiscontinuityState,
) -> BTreeMap<TimelineCallbackEndpoint, usize> {
    let mut notes = BTreeMap::<TimelineCallbackEndpoint, usize>::new();
    for note in &state.notes {
        if let Some(endpoint) = routes.generator_by_channel.get(&note.channel_id) {
            let count = notes.entry(*endpoint).or_default();
            *count = count.saturating_add(1);
        }
    }
    notes
}

fn timeline_sink_events_from(events: &[TimelineEvent], start_frame: u64, frames: usize) -> usize {
    let first = events.partition_point(|event| event.frame < start_frame);
    let last = start_frame
        .checked_add(frames as u64)
        .map_or(events.len(), |end_frame| {
            events.partition_point(|event| event.frame < end_frame)
        });
    events[first..last]
        .iter()
        .filter(|event| {
            matches!(
                event.kind,
                TimelineEventKind::NoteOn { .. }
                    | TimelineEventKind::NoteOff { .. }
                    | TimelineEventKind::AudioStart(_)
                    | TimelineEventKind::AudioStop { .. }
            )
        })
        .count()
}

fn grouped_events_from(groups: &[EventFrameGroup], start_frame: u64, frames: usize) -> usize {
    let first = groups.partition_point(|group| group.frame < start_frame);
    let last = start_frame
        .checked_add(frames as u64)
        .map_or(groups.len(), |end_frame| {
            groups.partition_point(|group| group.frame < end_frame)
        });
    groups[first..last]
        .iter()
        .fold(0_usize, |count, group| count.saturating_add(group.events))
}

fn validate_resource(
    resource: TimelineRuntimeResource,
    actual: usize,
    maximum: usize,
) -> Result<(), TimelineRuntimeValidationError> {
    if actual > maximum {
        Err(TimelineRuntimeValidationError::ResourceLimitExceeded {
            resource,
            actual,
            maximum,
        })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use crate::{
        automation::{AutomationCurve, AutomationLane, AutomationPoint, AutomationTarget},
        model::{
            AudioAsset, Channel, Clip, ClipKind, MixerInsertSlotRef, Pattern, PianoNote,
            PluginFormat, PluginInstance, PluginRole, PluginRuntimeStatus, Project,
            ProjectAutomation,
        },
        tempo_map::TempoMap,
        timeline::{
            AudioClipDescriptor, AutomationBaseValue, AutomationRampDescriptor,
            AutomationRampShape, ChasedNote, CompiledAutomationTarget, LongNoteChasePolicy,
            NoteSourceDescriptor, TimelineCompileOptions, TimelineEventKind,
        },
    };

    fn project(length: f32) -> Project {
        Project {
            format_version: 2,
            name: "runtime".into(),
            tempo: 120.0,
            swing: 0.0,
            song_length_beats: length,
            channels: Vec::new(),
            patterns: Vec::new(),
            active_pattern: 0,
            clips: Vec::new(),
            piano_notes: Vec::new(),
            mixer_tracks: Project::blank().mixer_tracks,
            automation_lanes: Vec::new(),
            audio_assets: Vec::new(),
            plugin_instances: Vec::new(),
            mixer_insert_slots: Vec::new(),
            audio_clip_mixer_destinations: Vec::new(),
            mixer_routes: Project::blank().mixer_routes,
            migration_diagnostics: Vec::new(),
        }
    }

    fn timeline(length: f32) -> Arc<CompiledTimeline> {
        let project = project(length);
        let map = TempoMap::new(120.0, None, f64::from(length), 48_000).unwrap();
        Arc::new(
            CompiledTimeline::from_project(&project, &map, TimelineCompileOptions::default())
                .unwrap(),
        )
    }

    fn plugin(instance_id: u64) -> PluginInstance {
        PluginInstance {
            midi_ports: crate::plugin_midi_routing::PluginMidiPorts::default(),
            id: instance_id,
            format: PluginFormat::Vst3,
            role: PluginRole::Instrument,
            path: PathBuf::from(format!(r"C:\VST3\Runtime-{instance_id}.vst3")),
            uid: format!("runtime-{instance_id}"),
            vendor: "Citrus".into(),
            name: format!("Runtime {instance_id}"),
            enabled: true,
            bypass: false,
            wet: 1.0,
            parameters: BTreeMap::new(),
            opaque_state: Vec::new(),
            runtime_status: PluginRuntimeStatus::Unloaded,
        }
    }

    fn generator_note_timeline(note_count: usize) -> Arc<CompiledTimeline> {
        generator_note_and_parameter_timeline(note_count, 0)
    }

    fn generator_note_and_parameter_timeline(
        note_count: usize,
        parameter_count: usize,
    ) -> Arc<CompiledTimeline> {
        let mut project = project(4.0);
        let mut generator = plugin(1);
        for parameter_id in 0..parameter_count as u32 {
            generator.parameters.insert(parameter_id, 0.0);
            let mut lane = AutomationLane::new(AutomationTarget::PluginParameter {
                instance: 1,
                parameter: parameter_id,
            });
            lane.replace_points([AutomationPoint::new(0.0, 0.5)]);
            project.automation_lanes.push(ProjectAutomation {
                id: u64::from(parameter_id) + 1,
                name: format!("Parameter {parameter_id}"),
                lane,
            });
        }
        project.plugin_instances.push(generator);
        project.channels.push(Channel {
            id: 1,
            name: "Generator".into(),
            color: [0; 3],
            volume: 1.0,
            pan: 0.0,
            muted: false,
            solo: false,
            mixer_track: 1,
            instrument_plugin_instance_id: Some(1),
            steps: [false; 16],
        });
        project.patterns.push(Pattern {
            id: 1,
            name: "Burst".into(),
            length_steps: 16,
            channel_steps: vec![[false; 16]],
            notes: (0..note_count)
                .map(|index| PianoNote {
                    id: index as u64 + 1,
                    channel_id: Some(1),
                    group_id: None,
                    note: (index % 128) as u8,
                    start: 0.0,
                    length: 2.0,
                    velocity: 1.0,
                    selected: false,
                    muted: false,
                })
                .collect(),
        });
        project.clips.push(Clip {
            id: 1,
            track: 0,
            start: 0.0,
            length: 4.0,
            name: "Burst".into(),
            color: [0; 3],
            kind: ClipKind::Pattern,
            group_id: None,
            pattern_id: 1,
            automation_id: None,
            audio_asset_id: None,
            source_offset: 0.0,
            audio_source_offset_frame: Some(0),
            audio_source_reference: None,
            audio_length_reference: None,
            fade_in_reference: None,
            fade_out_reference: None,
            gain: 1.0,
            fade_in: 0.0,
            fade_out: 0.0,
            muted: false,
        });
        let map = TempoMap::new(120.0, None, 4.0, 48_000).unwrap();
        Arc::new(
            CompiledTimeline::from_project(&project, &map, TimelineCompileOptions::default())
                .unwrap(),
        )
    }

    fn generator_parameter_timeline(parameter_counts: &[usize]) -> Arc<CompiledTimeline> {
        let mut project = project(4.0);
        let mut automation_id = 1_u64;
        for (endpoint_index, &parameter_count) in parameter_counts.iter().enumerate() {
            let instance_id = endpoint_index as u64 + 1;
            let channel_id = endpoint_index as u32 + 1;
            let mut generator = plugin(instance_id);
            for parameter_id in 0..parameter_count as u32 {
                generator.parameters.insert(parameter_id, 0.0);
                let mut lane = AutomationLane::new(AutomationTarget::PluginParameter {
                    instance: instance_id,
                    parameter: parameter_id,
                });
                lane.replace_points([AutomationPoint::new(0.0, 0.5)]);
                project.automation_lanes.push(ProjectAutomation {
                    id: automation_id,
                    name: format!("Parameter {instance_id}:{parameter_id}"),
                    lane,
                });
                automation_id += 1;
            }
            project.plugin_instances.push(generator);
            project.channels.push(Channel {
                id: channel_id,
                name: format!("Generator {channel_id}"),
                color: [0; 3],
                volume: 1.0,
                pan: 0.0,
                muted: false,
                solo: false,
                mixer_track: crate::mixer_graph::MASTER_MIXER_TRACK_ID,
                instrument_plugin_instance_id: Some(instance_id),
                steps: [false; 16],
            });
        }
        let map = TempoMap::new(120.0, None, 4.0, 48_000).unwrap();
        Arc::new(
            CompiledTimeline::from_project(&project, &map, TimelineCompileOptions::default())
                .unwrap(),
        )
    }

    fn mixer_parameter_timeline(
        route_parameters: &[(usize, usize, usize)],
    ) -> Arc<CompiledTimeline> {
        let mut project = project(4.0);
        let mut automation_id = 1_u64;
        for (route_index, &(track, persisted_slot, parameter_count)) in
            route_parameters.iter().enumerate()
        {
            let instance_id = route_index as u64 + 1;
            let mut insert = plugin(instance_id);
            for parameter_id in 0..parameter_count as u32 {
                insert.parameters.insert(parameter_id, 0.0);
                let mut lane = AutomationLane::new(AutomationTarget::PluginParameter {
                    instance: instance_id,
                    parameter: parameter_id,
                });
                lane.replace_points([AutomationPoint::new(0.0, 0.5)]);
                project.automation_lanes.push(ProjectAutomation {
                    id: automation_id,
                    name: format!("Insert parameter {instance_id}:{parameter_id}"),
                    lane,
                });
                automation_id += 1;
            }
            project.plugin_instances.push(insert);
            project.mixer_insert_slots.push(MixerInsertSlotRef {
                track: crate::model::mixer_track_id_for_runtime_slot(track.min(31) as u8),
                slot: persisted_slot,
                plugin_instance_id: instance_id,
            });
        }
        let map = TempoMap::new(120.0, None, 4.0, 48_000).unwrap();
        Arc::new(
            CompiledTimeline::from_project(&project, &map, TimelineCompileOptions::default())
                .unwrap(),
        )
    }

    fn phased_generator_timeline(
        notes_at_zero: usize,
        notes_at_quantum: usize,
        parameter_count: usize,
    ) -> Arc<CompiledTimeline> {
        let mut project = project(4.0);
        let mut generator = plugin(1);
        for parameter_id in 0..parameter_count as u32 {
            generator.parameters.insert(parameter_id, 0.0);
            let mut lane = AutomationLane::new(AutomationTarget::PluginParameter {
                instance: 1,
                parameter: parameter_id,
            });
            lane.replace_points([AutomationPoint::new(0.0, 0.5)]);
            project.automation_lanes.push(ProjectAutomation {
                id: u64::from(parameter_id) + 1,
                name: format!("Parameter {parameter_id}"),
                lane,
            });
        }
        project.plugin_instances.push(generator);
        project.channels.push(Channel {
            id: 1,
            name: "Generator".into(),
            color: [0; 3],
            volume: 1.0,
            pan: 0.0,
            muted: false,
            solo: false,
            mixer_track: crate::mixer_graph::MASTER_MIXER_TRACK_ID,
            instrument_plugin_instance_id: Some(1),
            steps: [false; 16],
        });
        let quantum_beat = TIMELINE_PLUGIN_FIXED_QUANTUM_FRAMES as f32 / 24_000.0;
        let mut notes = Vec::with_capacity(notes_at_zero + notes_at_quantum);
        notes.extend((0..notes_at_zero).map(|index| PianoNote {
            id: index as u64 + 1,
            channel_id: Some(1),
            group_id: None,
            note: (index % 128) as u8,
            start: 0.0,
            length: 1.0,
            velocity: 1.0,
            selected: false,
            muted: false,
        }));
        notes.extend((0..notes_at_quantum).map(|index| PianoNote {
            id: (notes_at_zero + index) as u64 + 1,
            channel_id: Some(1),
            group_id: None,
            note: ((notes_at_zero + index) % 128) as u8,
            start: quantum_beat,
            length: 1.0,
            velocity: 1.0,
            selected: false,
            muted: false,
        }));
        project.patterns.push(Pattern {
            id: 1,
            name: "Phased".into(),
            length_steps: 16,
            channel_steps: vec![[false; 16]],
            notes,
        });
        project.clips.push(Clip {
            id: 1,
            track: 0,
            start: 0.0,
            length: 4.0,
            name: "Phased".into(),
            color: [0; 3],
            kind: ClipKind::Pattern,
            group_id: None,
            pattern_id: 1,
            automation_id: None,
            audio_asset_id: None,
            source_offset: 0.0,
            audio_source_offset_frame: Some(0),
            audio_source_reference: None,
            audio_length_reference: None,
            fade_in_reference: None,
            fade_out_reference: None,
            gain: 1.0,
            fade_in: 0.0,
            fade_out: 0.0,
            muted: false,
        });
        let map = TempoMap::new(120.0, None, 4.0, 48_000).unwrap();
        let timeline =
            CompiledTimeline::from_project(&project, &map, TimelineCompileOptions::default())
                .unwrap();
        assert_eq!(
            timeline
                .events()
                .iter()
                .filter_map(
                    |event| matches!(event.kind, TimelineEventKind::NoteOn { .. })
                        .then_some(event.frame)
                )
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([0, TIMELINE_PLUGIN_FIXED_QUANTUM_FRAMES as u64])
        );
        Arc::new(timeline)
    }

    fn generator_route_timeline(route_count: usize) -> Arc<CompiledTimeline> {
        let mut project = project(4.0);
        for index in 0..route_count {
            let instance_id = index as u64 + 1;
            let channel_id = index as u32 + 1;
            project.plugin_instances.push(plugin(instance_id));
            project.channels.push(Channel {
                id: channel_id,
                name: format!("Generator {channel_id}"),
                color: [0; 3],
                volume: 1.0,
                pan: 0.0,
                muted: false,
                solo: false,
                mixer_track: crate::mixer_graph::MASTER_MIXER_TRACK_ID,
                instrument_plugin_instance_id: Some(instance_id),
                steps: [false; 16],
            });
        }
        let map = TempoMap::new(120.0, None, 4.0, 48_000).unwrap();
        Arc::new(
            CompiledTimeline::from_project(&project, &map, TimelineCompileOptions::default())
                .unwrap(),
        )
    }

    fn audio_asset_timeline(asset_count: usize) -> Arc<CompiledTimeline> {
        let mut project = project(4.0);
        for index in 0..asset_count {
            let asset_id = index as u64 + 1;
            project.audio_assets.push(AudioAsset {
                id: asset_id,
                name: format!("Asset {asset_id}"),
                path: PathBuf::from(format!(r"C:\Audio\{asset_id}.wav")),
                sample_rate: 48_000,
                channels: 2,
                bits_per_sample: 24,
                frames: 48_000,
                waveform_peaks: Vec::new(),
            });
            project.clips.push(Clip {
                id: index as u32 + 1,
                track: 0,
                start: 0.0,
                length: 1.0,
                name: format!("Asset {asset_id}"),
                color: [0; 3],
                kind: ClipKind::Audio,
                group_id: None,
                pattern_id: 0,
                automation_id: None,
                audio_asset_id: Some(asset_id),
                source_offset: 0.0,
                audio_source_offset_frame: Some(0),
                audio_source_reference: None,
                audio_length_reference: None,
                fade_in_reference: None,
                fade_out_reference: None,
                gain: 1.0,
                fade_in: 0.0,
                fade_out: 0.0,
                muted: false,
            });
        }
        let map = TempoMap::new(120.0, None, 4.0, 48_000).unwrap();
        Arc::new(
            CompiledTimeline::from_project(&project, &map, TimelineCompileOptions::default())
                .unwrap(),
        )
    }

    fn synthetic_event(frame: u64, id: u64) -> TimelineEvent {
        TimelineEvent {
            frame,
            kind: TimelineEventKind::AudioStop {
                clip_id: id as u32,
                asset_id: id,
            },
        }
    }

    fn synthetic_note_event(frame: u64, note_id: u64, channel_id: u32) -> TimelineEvent {
        TimelineEvent {
            frame,
            kind: TimelineEventKind::NoteOn {
                note_id,
                channel_id,
                note: (note_id % 128) as u8,
                velocity: 1.0,
                gain: 1.0,
                mixer_track: 1,
                source: NoteSourceDescriptor::LegacyPiano {
                    clip_id: 1,
                    pattern_id: 1,
                    persistent_note_id: note_id,
                    note_index: note_id as u32,
                    repetition: 0,
                },
            },
        }
    }

    fn synthetic_chased_note(note_id: u64, channel_id: u32) -> ChasedNote {
        ChasedNote {
            note_id,
            channel_id,
            note: (note_id % 128) as u8,
            velocity: 1.0,
            gain: 1.0,
            mixer_track: 1,
            source: NoteSourceDescriptor::LegacyPiano {
                clip_id: 1,
                pattern_id: 1,
                persistent_note_id: note_id,
                note_index: note_id as u32,
                repetition: 0,
            },
        }
    }

    fn empty_chase_state(frame: u64) -> TimelineDiscontinuityState {
        TimelineDiscontinuityState {
            frame,
            audio_clips: Vec::new(),
            automation_layers: Vec::new(),
            automation_bases: Vec::new(),
            notes: Vec::new(),
        }
    }

    fn burst_timeline() -> (Arc<CompiledTimeline>, u64) {
        let mut project = project(2.0);
        project.channels.push(Channel {
            id: 1,
            name: "Channel".into(),
            color: [0; 3],
            volume: 1.0,
            pan: 0.0,
            muted: false,
            solo: false,
            mixer_track: 1,
            instrument_plugin_instance_id: None,
            steps: [false; 16],
        });
        let mut steps = [false; 16];
        steps[0] = true;
        project.patterns.push(Pattern {
            id: 1,
            name: "Pattern".into(),
            length_steps: 1,
            channel_steps: vec![steps],
            notes: Vec::new(),
        });
        project.clips.push(Clip {
            id: 1,
            track: 0,
            start: 0.0,
            length: 0.5,
            name: "Clip".into(),
            color: [0; 3],
            kind: ClipKind::Pattern,
            group_id: None,
            pattern_id: 1,
            automation_id: None,
            audio_asset_id: None,
            source_offset: 0.0,
            audio_source_offset_frame: Some(0),
            audio_source_reference: None,
            audio_length_reference: None,
            fade_in_reference: None,
            fade_out_reference: None,
            gain: 1.0,
            fade_in: 0.0,
            fade_out: 0.0,
            muted: false,
        });
        let map = TempoMap::new(120.0, None, 2.0, 48_000).unwrap();
        let boundary = map.beat_to_frame(0.25).unwrap();
        let timeline =
            CompiledTimeline::from_project(&project, &map, TimelineCompileOptions::default())
                .unwrap();
        (Arc::new(timeline), boundary)
    }

    fn poll_all(controller: &mut TimelineRuntimeController) -> Vec<TimelineRuntimeEvent> {
        let mut events = Vec::new();
        while let Some(event) = controller.poll_event() {
            events.push(event);
        }
        events
    }

    fn activate(
        controller: &mut TimelineRuntimeController,
        realtime: &mut RealtimeTimelineRuntime,
        timeline: Arc<CompiledTimeline>,
        revision: u64,
        epoch: u64,
        frame: u64,
    ) {
        let chase = controller
            .prepare_chase(
                &timeline,
                revision,
                epoch,
                frame,
                TimelineChaseOptions::default(),
            )
            .unwrap();
        controller.install(revision, timeline).unwrap();
        controller.install_chase(chase).unwrap();
        assert_eq!(realtime.apply_pending_at_block_boundary(), 2);
        assert_eq!(poll_all(controller).len(), 2);
        realtime
            .activate_discontinuity(revision, epoch, frame, TimelineDiscontinuityKind::OneShot)
            .unwrap();
    }

    fn activate_with_mixer_bank(
        controller: &mut TimelineRuntimeController,
        realtime: &mut RealtimeTimelineRuntime,
        timeline: Arc<CompiledTimeline>,
        revision: u64,
        epoch: u64,
        frame: u64,
    ) {
        let chase = controller
            .prepare_chase(
                &timeline,
                revision,
                epoch,
                frame,
                TimelineChaseOptions::default(),
            )
            .unwrap();
        let bank = Box::new(PreparedMixerGraphDelayBank::new(timeline.mixer_graph(), 32).unwrap());
        controller
            .install_with_mixer_resources(revision, timeline, bank)
            .unwrap();
        controller.install_chase(chase).unwrap();
        assert_eq!(realtime.apply_pending_at_block_boundary(), 2);
        assert_eq!(poll_all(controller).len(), 2);
        realtime
            .activate_discontinuity(revision, epoch, frame, TimelineDiscontinuityKind::OneShot)
            .unwrap();
    }

    fn stage_same_graph_reuse_candidate(
        controller: &mut TimelineRuntimeController,
        realtime: &mut RealtimeTimelineRuntime,
        revision: u64,
        epoch: u64,
    ) -> u64 {
        let replacement = timeline(1.0);
        let fingerprint = replacement.mixer_graph().fingerprint();
        let loop_chase = controller
            .prepare_loop_chase(&replacement, revision, 0, TimelineChaseOptions::default())
            .unwrap();
        let one_shot = controller
            .prepare_chase(
                &replacement,
                revision,
                epoch,
                0,
                TimelineChaseOptions::default(),
            )
            .unwrap();
        controller
            .install_reusing_mixer_resources(revision, replacement, fingerprint)
            .unwrap();
        let loop_token = controller.install_loop_chase(loop_chase).unwrap();
        controller.install_chase(one_shot).unwrap();
        assert_eq!(realtime.apply_pending_at_block_boundary(), 3);
        assert_eq!(poll_all(controller).len(), 3);
        loop_token
    }

    fn replacement_activation_spec(
        revision: u64,
        epoch: u64,
        loop_token: u64,
    ) -> TimelineTransportActivationSpec {
        TimelineTransportActivationSpec {
            revision,
            target_epoch: epoch,
            minimum_epoch: epoch,
            frame: 0,
            beat_q32: 0,
            loop_start_frame: 0,
            loop_end_frame: 48_000,
            loop_start_q32: 0,
            loop_end_q32: 2 << 32,
            loop_token,
            loop_enabled: true,
            playing: true,
            mixer_pan_release: TimelineMixerPanRelease::EMPTY,
        }
    }

    fn shutdown_same_thread(
        controller: &mut TimelineRuntimeController,
        realtime: &mut RealtimeTimelineRuntime,
    ) {
        let request = loop {
            match controller.request_shutdown() {
                Ok(request) => break request,
                Err(TimelineControlError::CommandQueueFull) => {
                    realtime.apply_pending_at_block_boundary();
                    poll_all(controller);
                    controller.drain_retired();
                }
                Err(error) => panic!("shutdown request failed: {error}"),
            }
        };
        for _ in 0..32 {
            realtime.apply_pending_at_block_boundary();
            let confirmed = poll_all(controller).into_iter().any(|event| {
                matches!(
                    event,
                    TimelineRuntimeEvent::ShutdownComplete { request_id, .. }
                        if request_id == request
                )
            });
            controller.drain_retired();
            if confirmed || realtime.is_shutdown() {
                return;
            }
        }
        panic!("shutdown did not complete");
    }

    #[test]
    fn callback_capacity_uses_arbitrarily_aligned_half_open_windows() {
        let mut exact = Vec::with_capacity(TIMELINE_CALLBACK_MAX_EVENTS);
        exact.extend(
            (0..TIMELINE_CALLBACK_MAX_EVENTS / 2).map(|id| synthetic_event(2_047, id as u64 + 1)),
        );
        exact.extend(
            (TIMELINE_CALLBACK_MAX_EVENTS / 2..TIMELINE_CALLBACK_MAX_EVENTS)
                .map(|id| synthetic_event(2_048, id as u64 + 1)),
        );
        assert_eq!(
            maximum_event_window(&exact, TIMELINE_CALLBACK_MAX_FRAMES),
            WindowMaximum {
                start_frame: 1,
                events: TIMELINE_CALLBACK_MAX_EVENTS,
            }
        );
        validate_global_callback_event_capacity(&exact).unwrap();

        let mut overflow = exact;
        overflow.push(synthetic_event(
            2_048,
            TIMELINE_CALLBACK_MAX_EVENTS as u64 + 1,
        ));
        assert_eq!(
            maximum_event_window(&overflow, TIMELINE_CALLBACK_MAX_FRAMES),
            WindowMaximum {
                start_frame: 1,
                events: TIMELINE_CALLBACK_MAX_EVENTS + 1,
            }
        );
        assert_eq!(
            validate_global_callback_event_capacity(&overflow),
            Err(TimelineRuntimeValidationError::CallbackCapacityExceeded {
                resource: TimelineRuntimeResource::CallbackEvents {
                    window_frames: TIMELINE_CALLBACK_MAX_FRAMES as u16,
                },
                window_start_frame: 1,
                actual: TIMELINE_CALLBACK_MAX_EVENTS + 1,
                maximum: TIMELINE_CALLBACK_MAX_EVENTS,
            })
        );

        let boundary = [
            synthetic_event(0, 1),
            synthetic_event(TIMELINE_CALLBACK_MAX_FRAMES as u64, 2),
        ];
        assert_eq!(
            maximum_event_window(&boundary, TIMELINE_CALLBACK_MAX_FRAMES).events,
            1
        );
        let near_u64_end = [
            synthetic_event(u64::MAX - TIMELINE_CALLBACK_MAX_FRAMES as u64, 1),
            synthetic_event(u64::MAX, 2),
        ];
        assert_eq!(
            maximum_event_window(&near_u64_end, TIMELINE_CALLBACK_MAX_FRAMES),
            WindowMaximum {
                start_frame: u64::MAX - (TIMELINE_CALLBACK_MAX_FRAMES as u64 * 2 - 1),
                events: 1,
            }
        );
    }

    #[test]
    fn endpoint_quantum_capacity_is_per_physical_endpoint() {
        let first = TimelineCallbackEndpoint::Generator { channel_id: 1 };
        let second = TimelineCallbackEndpoint::Generator { channel_id: 2 };
        let mut routes = EndpointRoutes::default();
        routes.generator_by_channel.insert(1, first);
        routes.generator_by_channel.insert(2, second);

        let mut events = Vec::new();
        events.extend(
            (0..TIMELINE_ENDPOINT_TIMELINE_MAX_EVENTS_PER_QUANTUM - 8)
                .map(|id| synthetic_note_event(127, id as u64 + 1, 1)),
        );
        events.extend(
            (0..TIMELINE_ENDPOINT_TIMELINE_MAX_EVENTS_PER_QUANTUM - 8)
                .map(|id| synthetic_note_event(127, id as u64 + 10_000, 2)),
        );
        let grouped = endpoint_event_groups(&events, &routes);
        let parameters = BTreeMap::from([(first, 8), (second, 8)]);
        assert_eq!(
            maximum_grouped_event_window(
                grouped.get(&first).unwrap(),
                TIMELINE_PLUGIN_FIXED_QUANTUM_FRAMES,
            )
            .events,
            TIMELINE_ENDPOINT_TIMELINE_MAX_EVENTS_PER_QUANTUM - 8
        );
        assert_eq!(
            maximum_grouped_event_window(
                grouped.get(&second).unwrap(),
                TIMELINE_PLUGIN_FIXED_QUANTUM_FRAMES,
            )
            .events,
            TIMELINE_ENDPOINT_TIMELINE_MAX_EVENTS_PER_QUANTUM - 8
        );
        validate_endpoint_event_capacities(&grouped, &parameters).unwrap();

        events.insert(
            TIMELINE_ENDPOINT_TIMELINE_MAX_EVENTS_PER_QUANTUM - 8,
            synthetic_note_event(127, 99_999, 1),
        );
        let grouped = endpoint_event_groups(&events, &routes);
        assert_eq!(
            maximum_grouped_event_window(
                grouped.get(&first).unwrap(),
                TIMELINE_PLUGIN_FIXED_QUANTUM_FRAMES,
            )
            .events,
            TIMELINE_ENDPOINT_TIMELINE_MAX_EVENTS_PER_QUANTUM - 7
        );
        assert_eq!(
            validate_endpoint_event_capacities(&grouped, &parameters),
            Err(TimelineRuntimeValidationError::CallbackCapacityExceeded {
                resource: TimelineRuntimeResource::EndpointEvents {
                    endpoint: first,
                    window_frames: TIMELINE_PLUGIN_FIXED_QUANTUM_FRAMES as u16,
                },
                window_start_frame: 0,
                actual: TIMELINE_ENDPOINT_TIMELINE_MAX_EVENTS_PER_QUANTUM + 1,
                maximum: TIMELINE_ENDPOINT_TIMELINE_MAX_EVENTS_PER_QUANTUM,
            })
        );
    }

    #[test]
    fn mixer_endpoint_callback_capacity_is_independent_of_quantum_alignment() {
        let endpoint = TimelineCallbackEndpoint::MixerInsert { track: 3 };
        let exact = [
            EventFrameGroup {
                frame: 2_047,
                events: 48,
            },
            EventFrameGroup {
                frame: 2_175,
                events: 48,
            },
        ];
        assert_eq!(
            maximum_grouped_event_window(&exact, TIMELINE_PLUGIN_FIXED_QUANTUM_FRAMES).events,
            48
        );
        assert_eq!(
            maximum_grouped_event_window(&exact, TIMELINE_CALLBACK_MAX_FRAMES),
            WindowMaximum {
                start_frame: 128,
                events: 96,
            }
        );
        let parameters = BTreeMap::from([(endpoint, 8)]);
        validate_endpoint_event_capacities(
            &BTreeMap::from([(endpoint, exact.to_vec())]),
            &parameters,
        )
        .unwrap();

        let overflow = [
            exact[0],
            exact[1],
            EventFrameGroup {
                frame: 2_303,
                events: 1,
            },
        ];
        assert_eq!(
            maximum_grouped_event_window(&overflow, TIMELINE_PLUGIN_FIXED_QUANTUM_FRAMES).events,
            48
        );
        assert_eq!(
            maximum_grouped_event_window(&overflow, TIMELINE_CALLBACK_MAX_FRAMES).events,
            97
        );
        assert_eq!(
            validate_endpoint_event_capacities(
                &BTreeMap::from([(endpoint, overflow.to_vec())]),
                &parameters,
            ),
            Err(TimelineRuntimeValidationError::CallbackCapacityExceeded {
                resource: TimelineRuntimeResource::EndpointEvents {
                    endpoint,
                    window_frames: TIMELINE_CALLBACK_MAX_FRAMES as u16,
                },
                window_start_frame: 256,
                actual: TIMELINE_ENDPOINT_TIMELINE_MAX_EVENTS_PER_CALLBACK + 1,
                maximum: TIMELINE_ENDPOINT_TIMELINE_MAX_EVENTS_PER_CALLBACK,
            })
        );
    }

    #[test]
    fn raw_automation_boundaries_are_not_physical_endpoint_events() {
        let endpoint = TimelineCallbackEndpoint::MixerInsert { track: 3 };
        let mut routes = EndpointRoutes::default();
        routes.by_instance.insert(
            10,
            PluginRouteDestination::MixerInsert { track: 3, slot: 0 },
        );
        routes.by_instance.insert(
            20,
            PluginRouteDestination::MixerInsert { track: 3, slot: 1 },
        );
        let events = (0..65)
            .flat_map(|index| {
                [10_u64, 20_u64].map(|instance_id| TimelineEvent {
                    frame: 64,
                    kind: TimelineEventKind::AutomationEnd {
                        automation_id: index,
                        placement_id: None,
                        precedence: index,
                        target: CompiledAutomationTarget::PluginParameter {
                            instance_id,
                            parameter_id: index as u32,
                        },
                    },
                })
            })
            .collect::<Vec<_>>();
        let grouped = endpoint_event_groups(&events, &routes);
        assert!(!grouped.contains_key(&endpoint));
    }

    #[test]
    fn chase_plan_combines_reset_resources_and_normal_window() {
        let mut state = empty_chase_state(0);
        state.notes.push(synthetic_chased_note(1, 1));
        let exact = (0..TIMELINE_CALLBACK_MAX_EVENTS - 1)
            .map(|id| synthetic_event(0, id as u64 + 1))
            .collect::<Vec<_>>();
        validate_chase_render_plan_capacity(&exact, &state).unwrap();

        let mut overflow = exact;
        overflow.push(synthetic_event(0, TIMELINE_CALLBACK_MAX_EVENTS as u64));
        assert_eq!(
            validate_chase_render_plan_capacity(&overflow, &state),
            Err(TimelineRuntimeValidationError::CallbackCapacityExceeded {
                resource: TimelineRuntimeResource::CallbackEvents {
                    window_frames: TIMELINE_CALLBACK_MAX_FRAMES as u16,
                },
                window_start_frame: 0,
                actual: TIMELINE_CALLBACK_MAX_EVENTS + 1,
                maximum: TIMELINE_CALLBACK_MAX_EVENTS,
            })
        );
    }

    #[test]
    fn driven_parameter_manifest_aggregates_mixer_slots_by_physical_track() {
        let generator = TimelineCallbackEndpoint::Generator { channel_id: 1 };
        let insert = TimelineCallbackEndpoint::MixerInsert { track: 3 };
        let routes = EndpointRoutes::from_routes(&[
            CompiledPluginRoute {
                instance_id: 1,
                destination: PluginRouteDestination::Generator {
                    channel_id: 1,
                    slot: 0,
                },
            },
            CompiledPluginRoute {
                instance_id: 2,
                destination: PluginRouteDestination::MixerInsert { track: 3, slot: 0 },
            },
            CompiledPluginRoute {
                instance_id: 3,
                destination: PluginRouteDestination::MixerInsert { track: 3, slot: 1 },
            },
        ])
        .unwrap();
        let targets = [1_u64, 2, 3].map(|instance_id| CompiledAutomationTarget::PluginParameter {
            instance_id,
            parameter_id: 7,
        });
        assert_eq!(
            driven_plugin_parameter_counts_for_targets(&targets, &routes).unwrap(),
            BTreeMap::from([(generator, 1), (insert, 2)])
        );
    }

    #[test]
    fn route_validation_rejects_invalid_ambiguous_duplicate_and_non_dense_routes() {
        let invalid = [CompiledPluginRoute {
            instance_id: 1,
            destination: PluginRouteDestination::Generator {
                channel_id: 1,
                slot: 1,
            },
        }];
        assert_eq!(
            EndpointRoutes::from_routes(&invalid).unwrap_err(),
            TimelineRuntimeValidationError::InvalidPluginRoute {
                instance_id: 1,
                destination: invalid[0].destination,
            }
        );

        let ambiguous = [
            CompiledPluginRoute {
                instance_id: 1,
                destination: PluginRouteDestination::Generator {
                    channel_id: 1,
                    slot: 0,
                },
            },
            CompiledPluginRoute {
                instance_id: 1,
                destination: PluginRouteDestination::MixerInsert { track: 0, slot: 0 },
            },
        ];
        assert_eq!(
            EndpointRoutes::from_routes(&ambiguous).unwrap_err(),
            TimelineRuntimeValidationError::AmbiguousPluginRoute { instance_id: 1 }
        );

        let duplicate_physical = [
            CompiledPluginRoute {
                instance_id: 1,
                destination: PluginRouteDestination::MixerInsert { track: 2, slot: 0 },
            },
            CompiledPluginRoute {
                instance_id: 2,
                destination: PluginRouteDestination::MixerInsert { track: 2, slot: 0 },
            },
        ];
        assert_eq!(
            EndpointRoutes::from_routes(&duplicate_physical).unwrap_err(),
            TimelineRuntimeValidationError::DuplicatePhysicalPluginSlot {
                endpoint: TimelineCallbackEndpoint::MixerInsert { track: 2 },
                slot: 0,
                first_instance_id: 1,
                second_instance_id: 2,
            }
        );

        let non_dense = [CompiledPluginRoute {
            instance_id: 1,
            destination: PluginRouteDestination::MixerInsert { track: 2, slot: 1 },
        }];
        assert_eq!(
            EndpointRoutes::from_routes(&non_dense).unwrap_err(),
            TimelineRuntimeValidationError::NonDensePluginMixerChain {
                track: 2,
                expected_slot: 0,
                actual_slot: 1,
            }
        );

        let routes = EndpointRoutes::default();
        let missing = [CompiledAutomationTarget::PluginParameter {
            instance_id: 99,
            parameter_id: 7,
        }];
        assert_eq!(
            driven_plugin_parameter_counts_for_targets(&missing, &routes).unwrap_err(),
            TimelineRuntimeValidationError::PluginAutomationRouteUnavailable {
                instance_id: 99,
                parameter_id: 7,
            }
        );
    }

    #[test]
    fn driven_parameter_limits_accept_eight_and_sixty_four() {
        let eight = generator_parameter_timeline(&[8]);
        validate_timeline(1, &eight).unwrap();

        let nine = generator_parameter_timeline(&[9]);
        assert_eq!(
            validate_timeline(2, &nine),
            Err(TimelineRuntimeValidationError::ResourceLimitExceeded {
                resource: TimelineRuntimeResource::EndpointDrivenPluginParameters {
                    endpoint: TimelineCallbackEndpoint::Generator { channel_id: 1 },
                },
                actual: 9,
                maximum: TIMELINE_ENDPOINT_MAX_DRIVEN_PLUGIN_PARAMETERS,
            })
        );

        let sixty_four = generator_parameter_timeline(&[8; 8]);
        validate_timeline(3, &sixty_four).unwrap();

        let sixty_five = generator_parameter_timeline(&[8, 8, 8, 8, 8, 8, 8, 8, 1]);
        assert_eq!(
            validate_timeline(4, &sixty_five),
            Err(TimelineRuntimeValidationError::ResourceLimitExceeded {
                resource: TimelineRuntimeResource::DrivenPluginParameters,
                actual: 65,
                maximum: TIMELINE_MAX_DRIVEN_PLUGIN_PARAMETERS,
            })
        );
    }

    #[test]
    fn mixer_parameter_limits_are_shared_across_slots_and_tracks() {
        let two_slots_same_parameter = mixer_parameter_timeline(&[(3, 0, 1), (3, 1, 1)]);
        let routes = EndpointRoutes::from_timeline(&two_slots_same_parameter).unwrap();
        assert_eq!(
            driven_plugin_parameter_counts(&two_slots_same_parameter, &routes).unwrap(),
            BTreeMap::from([(TimelineCallbackEndpoint::MixerInsert { track: 3 }, 2)])
        );

        let eight = mixer_parameter_timeline(&[(3, 0, 4), (3, 1, 4)]);
        validate_timeline(1, &eight).unwrap();

        let nine = mixer_parameter_timeline(&[(3, 0, 4), (3, 1, 5)]);
        assert_eq!(
            validate_timeline(2, &nine),
            Err(TimelineRuntimeValidationError::ResourceLimitExceeded {
                resource: TimelineRuntimeResource::EndpointDrivenPluginParameters {
                    endpoint: TimelineCallbackEndpoint::MixerInsert { track: 3 },
                },
                actual: 9,
                maximum: TIMELINE_ENDPOINT_MAX_DRIVEN_PLUGIN_PARAMETERS,
            })
        );

        let sixty_four_specs = (0..8).map(|track| (track, 0, 8)).collect::<Vec<_>>();
        let sixty_four = mixer_parameter_timeline(&sixty_four_specs);
        validate_timeline(3, &sixty_four).unwrap();

        let mut sixty_five_specs = sixty_four_specs;
        sixty_five_specs.push((8, 0, 1));
        let sixty_five = mixer_parameter_timeline(&sixty_five_specs);
        assert_eq!(
            validate_timeline(4, &sixty_five),
            Err(TimelineRuntimeValidationError::ResourceLimitExceeded {
                resource: TimelineRuntimeResource::DrivenPluginParameters,
                actual: 65,
                maximum: TIMELINE_MAX_DRIVEN_PLUGIN_PARAMETERS,
            })
        );
    }

    #[test]
    fn mixer_chase_uses_the_q128_sample_as_the_chased_parameter_event() {
        assert_eq!(
            chase_endpoint_timeline_event_counts(&[], 777, 0, 8),
            (8, 8 * TIMELINE_PLUGIN_FIXED_QUANTA_PER_CALLBACK)
        );

        let timeline = mixer_parameter_timeline(&[(3, 0, 4), (3, 1, 4)]);
        let state = timeline
            .chase_discontinuity(777, TimelineChaseOptions::default())
            .unwrap();
        validate_chase_endpoint_capacities(&timeline, &state).unwrap();
    }

    #[test]
    fn chase_timeline_lane_combines_chased_and_normal_notes_with_q128_parameters() {
        let (mut controller, mut realtime) = create_timeline_runtime();
        let exact = phased_generator_timeline(44, 44, 8);
        validate_timeline(1, &exact).unwrap();
        controller
            .prepare_chase(
                &exact,
                1,
                1,
                TIMELINE_PLUGIN_FIXED_QUANTUM_FRAMES as u64,
                TimelineChaseOptions {
                    long_notes: LongNoteChasePolicy::Chase,
                },
            )
            .unwrap();
        controller
            .prepare_loop_chase(
                &exact,
                1,
                TIMELINE_PLUGIN_FIXED_QUANTUM_FRAMES as u64,
                TimelineChaseOptions {
                    long_notes: LongNoteChasePolicy::Chase,
                },
            )
            .unwrap();

        let overflow = phased_generator_timeline(44, 45, 8);
        validate_timeline(2, &overflow).unwrap();
        let expected = TimelinePrepareChaseError::Validation(
            TimelineRuntimeValidationError::CallbackCapacityExceeded {
                resource: TimelineRuntimeResource::EndpointEvents {
                    endpoint: TimelineCallbackEndpoint::Generator { channel_id: 1 },
                    window_frames: TIMELINE_PLUGIN_FIXED_QUANTUM_FRAMES as u16,
                },
                window_start_frame: TIMELINE_PLUGIN_FIXED_QUANTUM_FRAMES as u64,
                actual: TIMELINE_ENDPOINT_TIMELINE_MAX_EVENTS_PER_QUANTUM + 1,
                maximum: TIMELINE_ENDPOINT_TIMELINE_MAX_EVENTS_PER_QUANTUM,
            },
        );
        assert_eq!(
            controller
                .prepare_chase(
                    &overflow,
                    2,
                    1,
                    TIMELINE_PLUGIN_FIXED_QUANTUM_FRAMES as u64,
                    TimelineChaseOptions {
                        long_notes: LongNoteChasePolicy::Chase,
                    },
                )
                .unwrap_err(),
            expected
        );
        assert_eq!(
            controller
                .prepare_loop_chase(
                    &overflow,
                    2,
                    TIMELINE_PLUGIN_FIXED_QUANTUM_FRAMES as u64,
                    TimelineChaseOptions {
                        long_notes: LongNoteChasePolicy::Chase,
                    },
                )
                .unwrap_err(),
            expected
        );
        shutdown_same_thread(&mut controller, &mut realtime);
    }

    #[test]
    fn install_accepts_96_and_rejects_97_generator_timeline_events_per_quantum() {
        let accepted = generator_note_timeline(TIMELINE_ENDPOINT_TIMELINE_MAX_EVENTS_PER_QUANTUM);
        validate_timeline(1, &accepted).unwrap();

        let rejected =
            generator_note_timeline(TIMELINE_ENDPOINT_TIMELINE_MAX_EVENTS_PER_QUANTUM + 1);
        assert_eq!(
            validate_timeline(2, &rejected),
            Err(TimelineRuntimeValidationError::CallbackCapacityExceeded {
                resource: TimelineRuntimeResource::EndpointEvents {
                    endpoint: TimelineCallbackEndpoint::Generator { channel_id: 1 },
                    window_frames: TIMELINE_PLUGIN_FIXED_QUANTUM_FRAMES as u16,
                },
                window_start_frame: 0,
                actual: TIMELINE_ENDPOINT_TIMELINE_MAX_EVENTS_PER_QUANTUM + 1,
                maximum: TIMELINE_ENDPOINT_TIMELINE_MAX_EVENTS_PER_QUANTUM,
            })
        );
    }

    #[test]
    fn install_limits_unique_callback_resident_audio_assets() {
        let accepted = audio_asset_timeline(TIMELINE_CALLBACK_MAX_AUDIO_ASSETS);
        validate_timeline(1, &accepted).unwrap();

        let rejected = audio_asset_timeline(TIMELINE_CALLBACK_MAX_AUDIO_ASSETS + 1);
        assert_eq!(
            validate_timeline(2, &rejected),
            Err(TimelineRuntimeValidationError::ResourceLimitExceeded {
                resource: TimelineRuntimeResource::ReferencedAudioAssets,
                actual: TIMELINE_CALLBACK_MAX_AUDIO_ASSETS + 1,
                maximum: TIMELINE_CALLBACK_MAX_AUDIO_ASSETS,
            })
        );
    }

    #[test]
    fn install_accepts_64_generator_routes_and_rejects_65() {
        let accepted = generator_route_timeline(TIMELINE_CALLBACK_MAX_GENERATOR_ENDPOINTS);
        validate_timeline(1, &accepted).unwrap();

        let rejected = generator_route_timeline(TIMELINE_CALLBACK_MAX_GENERATOR_ENDPOINTS + 1);
        assert_eq!(
            validate_timeline(2, &rejected),
            Err(TimelineRuntimeValidationError::ResourceLimitExceeded {
                resource: TimelineRuntimeResource::GeneratorRoutes,
                actual: TIMELINE_CALLBACK_MAX_GENERATOR_ENDPOINTS + 1,
                maximum: TIMELINE_CALLBACK_MAX_GENERATOR_ENDPOINTS,
            })
        );
    }

    #[test]
    fn replacement_is_confirmed_and_old_timeline_is_retired() {
        let (mut controller, mut realtime) = create_timeline_runtime();
        let first_timeline = timeline(2.0);
        let first_observer = Arc::clone(&first_timeline);
        assert_eq!(Arc::strong_count(&first_timeline), 2);
        let first = controller.install(1, first_timeline).unwrap();
        assert_eq!(realtime.apply_pending_at_block_boundary(), 1);
        assert_eq!(Arc::strong_count(&first_observer), 2);
        assert_eq!(
            controller.poll_event(),
            Some(TimelineRuntimeEvent::Installed {
                request_id: first,
                revision: 1,
            })
        );
        controller.install(2, timeline(2.0)).unwrap();
        assert_eq!(realtime.apply_pending_at_block_boundary(), 1);
        assert!(matches!(
            controller.poll_event(),
            Some(TimelineRuntimeEvent::Installed { revision: 2, .. })
        ));
        let retired = controller.poll_retired().unwrap();
        let RetiredTimelineResource::Bundle {
            revision,
            timeline: retired_timeline,
            ..
        } = retired
        else {
            panic!("expected retired timeline");
        };
        assert_eq!(revision, 1);
        assert!(Arc::ptr_eq(&first_observer, &retired_timeline));
        assert_eq!(Arc::strong_count(&first_observer), 2);
        drop(retired_timeline);
        assert_eq!(Arc::strong_count(&first_observer), 1);
        assert_eq!(realtime.active_revision(), Some(2));
        shutdown_same_thread(&mut controller, &mut realtime);
    }

    #[test]
    fn active_revision_runs_until_candidate_bundle_promotes_after_a_loop_epoch() {
        let (mut controller, mut realtime) = create_timeline_runtime();
        let first = timeline(2.0);
        let first_loop = controller
            .prepare_loop_chase(&first, 1, 0, TimelineChaseOptions::default())
            .unwrap();
        activate(&mut controller, &mut realtime, Arc::clone(&first), 1, 1, 0);
        assert!(realtime.chase_for_block(1, 1, 0).unwrap().is_some());
        controller.install_loop_chase(first_loop).unwrap();
        assert_eq!(realtime.apply_pending_at_block_boundary(), 1);
        let first_loop_token = match controller.poll_event().unwrap() {
            TimelineRuntimeEvent::LoopChaseInstalled { token, .. } => token,
            other => panic!("expected first loop chase, got {other:?}"),
        };

        let replacement = timeline(2.0);
        let replacement_loop = controller
            .prepare_loop_chase(&replacement, 2, 0, TimelineChaseOptions::default())
            .unwrap();
        let replacement_one_shot = controller
            .prepare_chase(&replacement, 2, 2, 0, TimelineChaseOptions::default())
            .unwrap();
        controller.install(2, Arc::clone(&replacement)).unwrap();
        controller.install_loop_chase(replacement_loop).unwrap();
        controller.install_chase(replacement_one_shot).unwrap();

        // A may wrap after B's minimum-epoch chase is queued but before the
        // callback even applies the candidate commands.
        realtime
            .activate_discontinuity(
                1,
                2,
                0,
                TimelineDiscontinuityKind::Loop {
                    token: first_loop_token,
                },
            )
            .unwrap();
        assert!(realtime.chase_for_block(1, 2, 0).unwrap().is_some());
        assert_eq!(realtime.apply_pending_at_block_boundary(), 3);
        let events = poll_all(&mut controller);
        let replacement_loop_token = events
            .iter()
            .find_map(|event| match event {
                TimelineRuntimeEvent::LoopChaseInstalled {
                    revision: 2, token, ..
                } => Some(*token),
                _ => None,
            })
            .expect("replacement loop confirmation");

        assert_eq!(realtime.active_revision(), Some(1));
        assert_eq!(realtime.active_epoch(), Some(2));
        assert_eq!(controller.resident_revision(), Some(2));
        assert_eq!(controller.confirmed_revision(), Some(1));
        assert_eq!(realtime.installed_loop_token(), Some(first_loop_token));
        assert!(controller.poll_retired().is_none());

        // The candidate's prepared epoch is a minimum. A may wrap while B is
        // staged; the next strictly newer actual transport epoch still promotes B.
        realtime
            .activate_discontinuity(2, 3, 0, TimelineDiscontinuityKind::OneShot)
            .unwrap();
        assert_eq!(realtime.active_revision(), Some(2));
        assert_eq!(realtime.active_epoch(), Some(3));
        assert_eq!(controller.confirmed_revision(), Some(2));
        assert_eq!(controller.confirmed_epoch(), Some(3));
        assert_eq!(
            realtime.installed_loop_token(),
            Some(replacement_loop_token)
        );
        assert!(realtime.deferred_retire.is_some());
        assert!(controller.poll_retired().is_none());

        assert_eq!(realtime.apply_pending_at_block_boundary(), 0);
        assert!(realtime.deferred_retire.is_none());
        assert!(matches!(
            controller.poll_retired(),
            Some(RetiredTimelineResource::Bundle { revision: 1, .. })
        ));
        shutdown_same_thread(&mut controller, &mut realtime);
    }

    #[test]
    fn atomic_transport_ticket_keeps_a_published_until_infallible_b_commit_finishes() {
        let (mut controller, mut realtime) = create_timeline_runtime();
        let first = timeline(2.0);
        let first_loop = controller
            .prepare_loop_chase(&first, 1, 0, TimelineChaseOptions::default())
            .unwrap();
        activate(&mut controller, &mut realtime, first, 1, 1, 0);
        realtime.chase_for_block(1, 1, 0).unwrap();
        controller.install_loop_chase(first_loop).unwrap();
        realtime.apply_pending_at_block_boundary();
        let first_token = match controller.poll_event().unwrap() {
            TimelineRuntimeEvent::LoopChaseInstalled { token, .. } => token,
            other => panic!("expected A loop receipt, got {other:?}"),
        };

        let replacement = timeline(2.0);
        let replacement_loop = controller
            .prepare_loop_chase(&replacement, 2, 0, TimelineChaseOptions::default())
            .unwrap();
        let replacement_chase = controller
            .prepare_chase(&replacement, 2, 2, 0, TimelineChaseOptions::default())
            .unwrap();
        controller.install(2, replacement).unwrap();
        controller.install_loop_chase(replacement_loop).unwrap();
        controller.install_chase(replacement_chase).unwrap();
        assert_eq!(realtime.apply_pending_at_block_boundary(), 3);
        let replacement_token = poll_all(&mut controller)
            .into_iter()
            .find_map(|event| match event {
                TimelineRuntimeEvent::LoopChaseInstalled { token, .. } => Some(token),
                _ => None,
            })
            .unwrap();

        // A crosses the epoch prepared as B's minimum while B remains resident.
        realtime
            .activate_discontinuity(
                1,
                2,
                0,
                TimelineDiscontinuityKind::Loop { token: first_token },
            )
            .unwrap();
        realtime.chase_for_block(1, 2, 0).unwrap();
        let spec = TimelineTransportActivationSpec {
            revision: 2,
            target_epoch: 2,
            minimum_epoch: 2,
            frame: 0,
            beat_q32: 0,
            loop_start_frame: 0,
            loop_end_frame: 48_000,
            loop_start_q32: 0,
            loop_end_q32: 2 << 32,
            loop_token: replacement_token,
            loop_enabled: true,
            playing: true,
            mixer_pan_release: TimelineMixerPanRelease::EMPTY,
        };
        let request_id = controller.activate_transport(spec, 17).unwrap();
        assert_eq!(realtime.apply_pending_at_block_boundary(), 1);
        let ticket = realtime.pending_transport_activation().unwrap();
        realtime.preflight_transport_activation(ticket, 3).unwrap();
        let committed = realtime.commit_preflighted_transport_activation(ticket, 3);

        // Runtime pointers have committed, but external identity is still A
        // until the caller has committed every sibling DSP/transport field.
        assert_eq!(realtime.active_revision(), Some(2));
        assert_eq!(controller.confirmed_identity_pair(), Some((1, 2)));
        realtime.publish_committed_transport_activation(committed);
        assert_eq!(
            controller.poll_event(),
            Some(TimelineRuntimeEvent::TransportActivationApplied {
                request_id,
                revision: 2,
                epoch: 3,
                frame: 0,
            })
        );
        assert_eq!(controller.confirmed_identity_pair(), Some((2, 3)));
        assert!(controller.is_synchronized());
        realtime.apply_pending_at_block_boundary();
        shutdown_same_thread(&mut controller, &mut realtime);
    }

    #[test]
    fn mixer_pan_release_payload_is_canonical_and_rejected_before_queueing_when_invalid() {
        let mut valid = TimelineMixerPanRelease::EMPTY;
        assert!(valid.insert(0, -1.0));
        assert!(valid.insert(TIMELINE_MIXER_PAN_RELEASE_TRACKS - 1, 1.0));
        assert_eq!(valid.pan_for_track(0), Some(-1.0));
        assert_eq!(
            valid.pan_for_track(TIMELINE_MIXER_PAN_RELEASE_TRACKS - 1),
            Some(1.0)
        );
        assert!(valid.is_valid());
        let copied = valid;
        assert_eq!(copied, valid);

        let before = valid;
        assert!(!valid.insert(TIMELINE_MIXER_PAN_RELEASE_TRACKS, 0.0));
        assert!(!valid.insert(1, f32::NAN));
        assert!(!valid.insert(1, f32::INFINITY));
        assert!(!valid.insert(1, 1.000_001));
        assert_eq!(valid, before, "failed inserts must be transactional");

        let mut invalid_mask = TimelineMixerPanRelease::EMPTY;
        invalid_mask.pan_bits[3] = 0.25_f32.to_bits();
        assert!(
            !invalid_mask.is_valid(),
            "an unmasked payload bit is a non-canonical/invalid mask"
        );

        let mut invalid_value = TimelineMixerPanRelease::EMPTY;
        invalid_value.track_mask = 1 << 7;
        invalid_value.pan_bits[7] = f32::NAN.to_bits();
        assert!(!invalid_value.is_valid());

        let invalid_spec = TimelineTransportActivationSpec {
            revision: 1,
            target_epoch: 1,
            minimum_epoch: 1,
            frame: 0,
            beat_q32: 0,
            loop_start_frame: 0,
            loop_end_frame: 48_000,
            loop_start_q32: 0,
            loop_end_q32: 2 << 32,
            loop_token: 1,
            loop_enabled: true,
            playing: true,
            mixer_pan_release: invalid_mask,
        };
        let (mut controller, mut realtime) = create_timeline_runtime();
        assert_eq!(
            controller.activate_transport(invalid_spec, 1),
            Err(TimelineControlError::InvalidTransportActivation)
        );
        assert_eq!(controller.last_queued_request, 0);
        assert_eq!(controller.next_request_id, 1);
        assert!(realtime.pending_transport_activation().is_none());
        shutdown_same_thread(&mut controller, &mut realtime);
    }

    #[test]
    fn default_timeline_command_ring_payload_stays_within_budget() {
        const COMMAND_SLOT_MAX_BYTES: usize = 256;
        const DEFAULT_RING_PAYLOAD_MAX_BYTES: usize = 8 * 1024;

        let command_bytes = mem::size_of::<TimelineRuntimeCommand>();
        assert!(command_bytes <= COMMAND_SLOT_MAX_BYTES);
        assert!(
            command_bytes * DEFAULT_TIMELINE_RUNTIME_COMMAND_CAPACITY
                <= DEFAULT_RING_PAYLOAD_MAX_BYTES
        );
    }

    #[test]
    fn rejected_atomic_transport_ticket_preserves_a_and_candidate_for_retry() {
        let (mut controller, mut realtime) = create_timeline_runtime();
        activate(&mut controller, &mut realtime, timeline(2.0), 1, 1, 0);
        realtime.chase_for_block(1, 1, 0).unwrap();
        let replacement = timeline(2.0);
        let loop_chase = controller
            .prepare_loop_chase(&replacement, 2, 0, TimelineChaseOptions::default())
            .unwrap();
        let one_shot = controller
            .prepare_chase(&replacement, 2, 2, 0, TimelineChaseOptions::default())
            .unwrap();
        controller.install(2, replacement).unwrap();
        controller.install_loop_chase(loop_chase).unwrap();
        controller.install_chase(one_shot).unwrap();
        realtime.apply_pending_at_block_boundary();
        poll_all(&mut controller);

        let spec = TimelineTransportActivationSpec {
            revision: 2,
            target_epoch: 2,
            minimum_epoch: 2,
            frame: 0,
            beat_q32: 0,
            loop_start_frame: 0,
            loop_end_frame: 48_000,
            loop_start_q32: 0,
            loop_end_q32: 2 << 32,
            loop_token: u64::MAX,
            loop_enabled: true,
            playing: true,
            mixer_pan_release: TimelineMixerPanRelease::EMPTY,
        };
        let request_id = controller.activate_transport(spec, 3).unwrap();
        realtime.apply_pending_at_block_boundary();
        let ticket = realtime.pending_transport_activation().unwrap();
        let reason = TimelineTransportActivationRejectReason::Runtime(
            realtime
                .preflight_transport_activation(ticket, 2)
                .unwrap_err(),
        );
        realtime.reject_pending_transport_activation(ticket, reason);

        assert_eq!(realtime.active_revision(), Some(1));
        assert_eq!(realtime.active_epoch(), Some(1));
        assert_eq!(realtime.next_frame(), Some(0));
        assert_eq!(controller.confirmed_identity_pair(), Some((1, 1)));
        assert_eq!(controller.resident_revision(), Some(2));
        assert!(!controller.needs_resync());
        assert!(matches!(
            controller.poll_event(),
            Some(TimelineRuntimeEvent::TransportActivationRejected {
                request_id: actual,
                revision: 2,
                reason: TimelineTransportActivationRejectReason::Runtime(
                    TimelineDiscontinuityActivationError::LoopTokenMismatch { .. }
                ),
            }) if actual == request_id
        ));
        shutdown_same_thread(&mut controller, &mut realtime);
    }

    #[test]
    fn unexpected_drop_forgets_active_candidate_and_deferred_timeline_owners() {
        let (mut controller, mut realtime) = create_timeline_runtime();
        let first = timeline(2.0);
        let first_observer = Arc::clone(&first);
        activate(&mut controller, &mut realtime, first, 1, 1, 0);
        let candidate = timeline(2.0);
        let candidate_observer = Arc::clone(&candidate);
        controller.install(2, candidate).unwrap();
        realtime.apply_pending_at_block_boundary();
        controller.poll_event();
        drop(realtime);
        assert_eq!(Arc::strong_count(&first_observer), 2);
        assert_eq!(Arc::strong_count(&candidate_observer), 2);
        assert_eq!(controller.stats().unexpected_realtime_drops, 1);

        let (mut controller, mut realtime) = create_timeline_runtime();
        let first = timeline(2.0);
        let first_observer = Arc::clone(&first);
        activate(&mut controller, &mut realtime, first, 1, 1, 0);
        let replacement = timeline(2.0);
        let replacement_observer = Arc::clone(&replacement);
        let one_shot = controller
            .prepare_chase(&replacement, 2, 2, 0, TimelineChaseOptions::default())
            .unwrap();
        let loop_chase = controller
            .prepare_loop_chase(&replacement, 2, 0, TimelineChaseOptions::default())
            .unwrap();
        controller.install(2, replacement).unwrap();
        controller.install_loop_chase(loop_chase).unwrap();
        controller.install_chase(one_shot).unwrap();
        realtime.apply_pending_at_block_boundary();
        poll_all(&mut controller);
        realtime
            .activate_discontinuity(2, 2, 0, TimelineDiscontinuityKind::OneShot)
            .unwrap();
        assert!(realtime.deferred_retire.is_some());
        drop(realtime);
        assert_eq!(Arc::strong_count(&first_observer), 2);
        assert_eq!(Arc::strong_count(&replacement_observer), 2);
        assert_eq!(controller.stats().unexpected_realtime_drops, 1);
    }

    #[test]
    fn command_queue_full_returns_the_exact_arc_without_cloning() {
        let (mut controller, mut realtime) =
            create_timeline_runtime_with_capacities(1, 4, 4).unwrap();
        controller.install(1, timeline(1.0)).unwrap();
        let second = timeline(1.0);
        let observer = Arc::clone(&second);
        let error = controller.install(2, second).unwrap_err();
        assert_eq!(error.reason, TimelineResourceQueueFailure::CommandQueueFull);
        assert!(Arc::ptr_eq(&error.resource, &observer));
        assert_eq!(Arc::strong_count(&observer), 2);
        realtime.apply_pending_at_block_boundary();
        poll_all(&mut controller);
        controller.install(2, error.into_resource()).unwrap();
        realtime.apply_pending_at_block_boundary();
        poll_all(&mut controller);
        controller.drain_retired();
        shutdown_same_thread(&mut controller, &mut realtime);
    }

    #[test]
    fn full_event_ring_stops_before_popping_the_next_command() {
        let (mut controller, mut realtime) =
            create_timeline_runtime_with_capacities(4, 1, 4).unwrap();
        controller.install(1, timeline(1.0)).unwrap();
        controller.install(2, timeline(1.0)).unwrap();
        assert_eq!(realtime.apply_pending_at_block_boundary_with_budget(2), 1);
        assert_eq!(realtime.pending_commands(), 1);
        assert_eq!(realtime.stats().event_backpressure, 1);
        controller.poll_event().unwrap();
        assert_eq!(realtime.apply_pending_at_block_boundary(), 1);
        controller.poll_event().unwrap();
        controller.drain_retired();
        shutdown_same_thread(&mut controller, &mut realtime);
    }

    #[test]
    fn full_retire_ring_stops_before_popping_owner_command() {
        let (mut controller, mut realtime) =
            create_timeline_runtime_with_capacities(8, 8, 2).unwrap();
        for revision in 1..=3 {
            controller.install(revision, timeline(1.0)).unwrap();
            assert_eq!(realtime.apply_pending_at_block_boundary(), 1);
            controller.poll_event().unwrap();
        }
        controller.install(4, timeline(1.0)).unwrap();
        assert_eq!(realtime.apply_pending_at_block_boundary(), 0);
        assert_eq!(realtime.pending_commands(), 1);
        assert_eq!(realtime.stats().retire_backpressure, 1);
        drop(controller.poll_retired().unwrap());
        assert_eq!(realtime.apply_pending_at_block_boundary(), 1);
        controller.poll_event().unwrap();
        controller.drain_retired();
        shutdown_same_thread(&mut controller, &mut realtime);
    }

    #[test]
    fn stale_revision_is_rejected_and_its_arc_is_retired() {
        let (mut controller, mut realtime) = create_timeline_runtime();
        controller.install(2, timeline(1.0)).unwrap();
        realtime.apply_pending_at_block_boundary();
        controller.poll_event().unwrap();
        let request = controller.install(1, timeline(1.0)).unwrap();
        realtime.apply_pending_at_block_boundary();
        assert_eq!(
            controller.poll_event(),
            Some(TimelineRuntimeEvent::Rejected {
                request_id: request,
                revision: 1,
                reason: TimelineRejectReason::StaleRevision { latest_revision: 2 },
            })
        );
        assert!(matches!(
            controller.poll_retired(),
            Some(RetiredTimelineResource::Timeline { revision: 1, .. })
        ));
        assert_eq!(realtime.active_revision(), Some(2));
        shutdown_same_thread(&mut controller, &mut realtime);
    }

    #[test]
    fn query_rejects_missing_epoch_wrong_revision_epoch_and_start() {
        let (mut controller, mut realtime) = create_timeline_runtime();
        let timeline = timeline(2.0);
        let chase = controller
            .prepare_chase(&timeline, 1, 7, 0, TimelineChaseOptions::default())
            .unwrap();
        controller.install(1, timeline).unwrap();
        realtime.apply_pending_at_block_boundary();
        controller.poll_event().unwrap();
        let mut packet = TimelinePacket::<1>::new();
        assert!(matches!(
            realtime.packetize_block(1, 7, 0, 1, &mut packet),
            Err(TimelineRuntimePacketError::Query(
                TimelineRuntimeQueryError::EpochUnavailable
            ))
        ));
        controller.install_chase(chase).unwrap();
        realtime.apply_pending_at_block_boundary();
        controller.poll_event().unwrap();
        realtime
            .activate_discontinuity(1, 7, 0, TimelineDiscontinuityKind::OneShot)
            .unwrap();
        assert!(matches!(
            realtime.chase_for_block(2, 7, 0),
            Err(TimelineRuntimeQueryError::RevisionMismatch { .. })
        ));
        assert!(matches!(
            realtime.chase_for_block(1, 8, 0),
            Err(TimelineRuntimeQueryError::EpochMismatch { .. })
        ));
        assert!(matches!(
            realtime.chase_for_block(1, 7, 1),
            Err(TimelineRuntimeQueryError::StartFrameMismatch { .. })
        ));
        shutdown_same_thread(&mut controller, &mut realtime);
    }

    #[test]
    fn discontinuity_chase_is_read_once_before_first_packet() {
        let (mut controller, mut realtime) = create_timeline_runtime();
        activate(&mut controller, &mut realtime, timeline(2.0), 1, 3, 0);
        let mut packet = TimelinePacket::<1>::new();
        assert!(matches!(
            realtime.packetize_block(1, 3, 0, 1, &mut packet),
            Err(TimelineRuntimePacketError::Query(
                TimelineRuntimeQueryError::ChaseRequired
            ))
        ));
        assert!(realtime.chase_for_block(1, 3, 0).unwrap().is_some());
        assert!(realtime.chase_for_block(1, 3, 0).unwrap().is_none());
        realtime
            .packetize_block(1, 3, 0, 1, &mut packet)
            .unwrap()
            .commit();
        assert_eq!(realtime.next_frame(), Some(1));
        shutdown_same_thread(&mut controller, &mut realtime);
    }

    #[test]
    fn stale_and_wrong_revision_chases_are_rejected_and_retired() {
        let (mut controller, mut realtime) = create_timeline_runtime();
        let timeline = timeline(2.0);
        let wrong = controller
            .prepare_chase(&timeline, 1, 1, 0, TimelineChaseOptions::default())
            .unwrap();
        let current = controller
            .prepare_chase(&timeline, 2, 2, 0, TimelineChaseOptions::default())
            .unwrap();
        let stale = controller
            .prepare_chase(&timeline, 2, 1, 0, TimelineChaseOptions::default())
            .unwrap();
        controller.install(2, timeline).unwrap();
        controller.install_chase(wrong).unwrap();
        controller.install_chase(current).unwrap();
        controller.install_chase(stale).unwrap();
        assert_eq!(realtime.apply_pending_at_block_boundary(), 4);
        let events = poll_all(&mut controller);
        assert!(matches!(
            events[1],
            TimelineRuntimeEvent::Rejected {
                reason: TimelineRejectReason::RevisionMismatch { .. },
                ..
            }
        ));
        assert!(matches!(
            events[2],
            TimelineRuntimeEvent::ChaseInstalled { .. }
        ));
        assert!(matches!(
            events[3],
            TimelineRuntimeEvent::Rejected {
                reason: TimelineRejectReason::StaleEpoch { latest_epoch: 2 },
                ..
            }
        ));
        assert_eq!(controller.drain_retired(), 2);
        shutdown_same_thread(&mut controller, &mut realtime);
    }

    #[test]
    fn candidate_control_rejection_does_not_fault_the_active_render_cursor() {
        let (mut controller, mut realtime) = create_timeline_runtime();
        let active = timeline(2.0);
        activate(&mut controller, &mut realtime, Arc::clone(&active), 1, 1, 0);
        assert!(realtime.chase_for_block(1, 1, 0).unwrap().is_some());

        let wrong = controller
            .prepare_chase(&active, 99, 2, 0, TimelineChaseOptions::default())
            .unwrap();
        controller.install_chase(wrong).unwrap();
        assert_eq!(realtime.apply_pending_at_block_boundary(), 1);
        assert!(matches!(
            controller.poll_event(),
            Some(TimelineRuntimeEvent::Rejected {
                reason: TimelineRejectReason::RevisionMismatch { .. },
                ..
            })
        ));
        assert!(!controller.needs_resync());
        assert!(!realtime.stats().ownership_needs_resync);

        let mut packet = TimelinePacket::<1>::new();
        realtime
            .packetize_block(1, 1, 0, 1, &mut packet)
            .unwrap()
            .commit();
        assert_eq!(realtime.next_frame(), Some(1));
        controller.drain_retired();
        shutdown_same_thread(&mut controller, &mut realtime);
    }

    #[test]
    fn same_frame_burst_uses_chunk_cursor_without_advancing_on_overflow() {
        let (mut controller, mut realtime) = create_timeline_runtime();
        let (timeline, boundary) = burst_timeline();
        activate(&mut controller, &mut realtime, timeline, 1, 1, boundary);
        realtime.chase_for_block(1, 1, boundary).unwrap();
        let mut packet = TimelinePacket::<1>::new();
        assert!(matches!(
            realtime.packetize_block(1, 1, boundary, 1, &mut packet),
            Err(TimelineRuntimePacketError::Packet(
                TimelinePacketError::CapacityExceeded { required: 2, .. }
            ))
        ));
        assert_eq!(realtime.next_frame(), Some(boundary));
        let mut block = realtime.begin_chunked_block(1, 1, boundary, 1).unwrap();
        assert_eq!(
            block
                .packetize_next_into(&mut packet)
                .unwrap()
                .remaining_events,
            1
        );
        assert!(matches!(
            packet.events()[0].kind,
            TimelineEventKind::NoteOff { .. }
        ));
        assert_eq!(
            block
                .packetize_next_into(&mut packet)
                .unwrap()
                .remaining_events,
            0
        );
        assert!(matches!(
            packet.events()[0].kind,
            TimelineEventKind::NoteOn { .. }
        ));
        assert!(block.is_complete());
        block.commit().unwrap();
        assert_eq!(realtime.next_frame(), Some(boundary + 1));
        shutdown_same_thread(&mut controller, &mut realtime);
    }

    #[test]
    fn fixed_packet_copy_advances_only_after_explicit_commit() {
        let (mut controller, mut realtime) = create_timeline_runtime();
        activate(&mut controller, &mut realtime, timeline(2.0), 1, 1, 0);
        realtime.chase_for_block(1, 1, 0).unwrap();
        let mut packet = TimelinePacket::<1>::new();
        let prepared = realtime.packetize_block(1, 1, 0, 4, &mut packet).unwrap();
        assert_eq!(*prepared.next_frame, 0);
        prepared.commit();
        assert_eq!(realtime.next_frame(), Some(4));
        assert!(!controller.needs_resync());
        shutdown_same_thread(&mut controller, &mut realtime);
    }

    #[test]
    fn dropping_uncommitted_fixed_packet_keeps_cursor_and_requires_resync() {
        let (mut controller, mut realtime) = create_timeline_runtime();
        activate(&mut controller, &mut realtime, timeline(2.0), 1, 1, 0);
        realtime.chase_for_block(1, 1, 0).unwrap();
        let mut packet = TimelinePacket::<1>::new();
        let prepared = realtime.packetize_block(1, 1, 0, 4, &mut packet).unwrap();
        drop(prepared);
        assert_eq!(realtime.next_frame(), Some(0));
        assert!(controller.needs_resync());
        shutdown_same_thread(&mut controller, &mut realtime);
    }

    #[test]
    fn dropping_fully_copied_chunk_block_still_requires_explicit_commit() {
        let (mut controller, mut realtime) = create_timeline_runtime();
        let (timeline, boundary) = burst_timeline();
        activate(&mut controller, &mut realtime, timeline, 1, 1, boundary);
        realtime.chase_for_block(1, 1, boundary).unwrap();
        let mut packet = TimelinePacket::<1>::new();
        let mut block = realtime.begin_chunked_block(1, 1, boundary, 1).unwrap();
        while block.remaining_events() != 0 {
            block.packetize_next_into(&mut packet).unwrap();
        }
        assert!(block.is_complete());
        assert_eq!(*block.next_frame, boundary);
        drop(block);
        assert_eq!(realtime.next_frame(), Some(boundary));
        assert!(controller.needs_resync());
        shutdown_same_thread(&mut controller, &mut realtime);
    }

    #[test]
    fn dropping_a_partial_chunk_requires_explicit_fresh_chase_resync() {
        let (mut controller, mut realtime) = create_timeline_runtime();
        let (timeline, boundary) = burst_timeline();
        let fresh = controller
            .prepare_chase(&timeline, 1, 2, boundary, TimelineChaseOptions::default())
            .unwrap();
        activate(&mut controller, &mut realtime, timeline, 1, 1, boundary);
        realtime.chase_for_block(1, 1, boundary).unwrap();
        let mut packet = TimelinePacket::<1>::new();
        {
            let mut block = realtime.begin_chunked_block(1, 1, boundary, 1).unwrap();
            block.packetize_next_into(&mut packet).unwrap();
        }
        assert_eq!(
            realtime.chase_for_block(1, 1, boundary),
            Err(TimelineRuntimeQueryError::NeedsResync)
        );
        controller.install_chase(fresh).unwrap();
        realtime.apply_pending_at_block_boundary();
        controller.poll_event().unwrap();
        assert!(!controller.acknowledge_resync(1, 2));
        assert!(controller.needs_resync());
        assert_eq!(
            realtime.chase_for_block(1, 1, boundary),
            Err(TimelineRuntimeQueryError::NeedsResync)
        );
        realtime
            .activate_discontinuity_after_resync(1, 2, boundary, TimelineDiscontinuityKind::OneShot)
            .unwrap();
        assert!(controller.acknowledge_resync(1, 2));
        assert!(!controller.needs_resync());
        assert_eq!(controller.confirmed_epoch(), Some(2));
        assert!(realtime.chase_for_block(1, 2, boundary).unwrap().is_some());
        controller.drain_retired();
        shutdown_same_thread(&mut controller, &mut realtime);
    }

    #[test]
    fn clear_is_confirmed_and_retires_timeline_and_chase() {
        let (mut controller, mut realtime) = create_timeline_runtime();
        activate(&mut controller, &mut realtime, timeline(1.0), 5, 9, 0);
        let request = controller.clear(5).unwrap();
        assert_eq!(realtime.apply_pending_at_block_boundary(), 1);
        assert_eq!(
            controller.poll_event(),
            Some(TimelineRuntimeEvent::Cleared {
                request_id: request,
                revision: 5,
            })
        );
        assert_eq!(controller.drain_retired(), 1);
        let mut packet = TimelinePacket::<1>::new();
        assert!(matches!(
            realtime.packetize_block(5, 9, 0, 1, &mut packet),
            Err(TimelineRuntimePacketError::Query(
                TimelineRuntimeQueryError::MissingTimeline
            ))
        ));
        shutdown_same_thread(&mut controller, &mut realtime);
    }

    #[test]
    fn prepared_one_shot_does_not_move_cursor_until_atomic_activation() {
        let (mut controller, mut realtime) = create_timeline_runtime();
        let timeline = timeline(2.0);
        let chase = controller
            .prepare_chase(&timeline, 1, 7, 12, TimelineChaseOptions::default())
            .unwrap();
        controller.install(1, timeline).unwrap();
        controller.install_chase(chase).unwrap();
        assert_eq!(realtime.apply_pending_at_block_boundary(), 2);
        assert_eq!(poll_all(&mut controller).len(), 2);

        assert_eq!(realtime.active_epoch(), None);
        assert_eq!(realtime.next_frame(), None);
        assert_eq!(controller.resident_one_shot_epoch(), Some(7));
        assert_eq!(controller.confirmed_epoch(), None);
        assert!(!controller.is_synchronized());
        realtime
            .activate_discontinuity(1, 7, 12, TimelineDiscontinuityKind::OneShot)
            .unwrap();
        assert_eq!(realtime.active_epoch(), Some(7));
        assert_eq!(realtime.next_frame(), Some(12));
        assert_eq!(controller.confirmed_epoch(), Some(7));
        assert!(controller.is_synchronized());
        shutdown_same_thread(&mut controller, &mut realtime);
    }

    #[test]
    fn one_shot_requires_its_exact_epoch_and_can_only_activate_once() {
        let (mut controller, mut realtime) = create_timeline_runtime();
        let timeline = timeline(2.0);
        let chase = controller
            .prepare_chase(&timeline, 1, 7, 0, TimelineChaseOptions::default())
            .unwrap();
        controller.install(1, timeline).unwrap();
        controller.install_chase(chase).unwrap();
        realtime.apply_pending_at_block_boundary();
        poll_all(&mut controller);

        assert_eq!(
            realtime.activate_discontinuity(1, 8, 0, TimelineDiscontinuityKind::OneShot),
            Err(TimelineDiscontinuityActivationError::OneShotEpochMismatch {
                expected: 7,
                requested: 8,
            })
        );
        assert_eq!(realtime.active_epoch(), None);
        realtime
            .activate_discontinuity(1, 7, 0, TimelineDiscontinuityKind::OneShot)
            .unwrap();
        assert_eq!(
            realtime.activate_discontinuity(1, 7, 0, TimelineDiscontinuityKind::OneShot),
            Err(TimelineDiscontinuityActivationError::OneShotConsumed)
        );
        assert_eq!(realtime.active_epoch(), Some(7));
        shutdown_same_thread(&mut controller, &mut realtime);
    }

    #[test]
    fn zero_epoch_is_rejected_during_prepare_install_and_activation() {
        let (mut controller, mut realtime) = create_timeline_runtime();
        let timeline = timeline(2.0);
        assert!(matches!(
            controller.prepare_chase(&timeline, 1, 0, 0, TimelineChaseOptions::default()),
            Err(TimelinePrepareChaseError::Validation(
                TimelineRuntimeValidationError::InvalidEpoch
            ))
        ));
        let invalid = Box::new(PreparedTimelineChase {
            plugin_topology_revision: 0,
            plugin_timing: crate::plugin_timing::PreparedPluginTimingPlan::conservative(48000)
                .unwrap(),
            revision: 1,
            epoch: 0,
            frame: 0,
            state: timeline
                .chase_discontinuity(0, TimelineChaseOptions::default())
                .unwrap(),
        });
        assert!(matches!(
            controller.install_chase(invalid),
            Err(TimelineResourceQueueError {
                reason: TimelineResourceQueueFailure::Validation(
                    TimelineRuntimeValidationError::InvalidEpoch
                ),
                ..
            })
        ));
        controller.install(1, timeline).unwrap();
        realtime.apply_pending_at_block_boundary();
        controller.poll_event().unwrap();
        assert_eq!(
            realtime.activate_discontinuity(1, 0, 0, TimelineDiscontinuityKind::OneShot),
            Err(TimelineDiscontinuityActivationError::InvalidEpoch)
        );
        assert_eq!(realtime.active_epoch(), None);
        shutdown_same_thread(&mut controller, &mut realtime);
    }

    #[test]
    fn loop_template_reuses_the_same_owned_state_across_new_epochs() {
        let (mut controller, mut realtime) = create_timeline_runtime();
        let timeline = timeline(2.0);
        let loop_chase = controller
            .prepare_loop_chase(&timeline, 1, 0, TimelineChaseOptions::default())
            .unwrap();
        controller.install(1, timeline).unwrap();
        let token = controller.install_loop_chase(loop_chase).unwrap();
        realtime.apply_pending_at_block_boundary();
        poll_all(&mut controller);

        realtime
            .activate_discontinuity(1, 10, 0, TimelineDiscontinuityKind::Loop { token })
            .unwrap();
        let first = realtime
            .chase_for_block(1, 10, 0)
            .unwrap()
            .expect("first loop epoch needs chase")
            as *const TimelineDiscontinuityState;
        assert!(realtime.chase_for_block(1, 10, 0).unwrap().is_none());

        realtime
            .activate_discontinuity(1, 11, 0, TimelineDiscontinuityKind::Loop { token })
            .unwrap();
        let second = realtime
            .chase_for_block(1, 11, 0)
            .unwrap()
            .expect("next loop epoch needs chase")
            as *const TimelineDiscontinuityState;
        assert_eq!(first, second);
        assert!(realtime.chase_for_block(1, 11, 0).unwrap().is_none());
        shutdown_same_thread(&mut controller, &mut realtime);
    }

    #[test]
    fn loop_replacement_requires_the_exact_installed_token() {
        let (mut controller, mut realtime) = create_timeline_runtime();
        let timeline = timeline(2.0);
        let first = controller
            .prepare_loop_chase(&timeline, 1, 0, TimelineChaseOptions::default())
            .unwrap();
        controller.install(1, Arc::clone(&timeline)).unwrap();
        let first_token = controller.install_loop_chase(first).unwrap();
        realtime.apply_pending_at_block_boundary();
        poll_all(&mut controller);
        realtime
            .activate_discontinuity(
                1,
                1,
                0,
                TimelineDiscontinuityKind::Loop { token: first_token },
            )
            .unwrap();
        realtime.chase_for_block(1, 1, 0).unwrap();

        let replacement = controller
            .prepare_loop_chase(&timeline, 1, 0, TimelineChaseOptions::default())
            .unwrap();
        let replacement_token = controller.install_loop_chase(replacement).unwrap();
        realtime.apply_pending_at_block_boundary();
        assert!(matches!(
            controller.poll_event(),
            Some(TimelineRuntimeEvent::LoopChaseInstalled { token, .. })
                if token == replacement_token
        ));
        assert!(matches!(
            controller.poll_retired(),
            Some(RetiredTimelineResource::LoopChase { token, .. })
                if token == first_token
        ));
        assert_eq!(
            realtime.activate_discontinuity(
                1,
                2,
                0,
                TimelineDiscontinuityKind::Loop { token: first_token },
            ),
            Err(TimelineDiscontinuityActivationError::LoopTokenMismatch {
                expected: replacement_token,
                requested: first_token,
            })
        );
        assert_eq!(realtime.active_epoch(), Some(1));
        realtime
            .activate_discontinuity(
                1,
                2,
                0,
                TimelineDiscontinuityKind::Loop {
                    token: replacement_token,
                },
            )
            .unwrap();
        shutdown_same_thread(&mut controller, &mut realtime);
    }

    #[test]
    fn retire_backpressure_keeps_the_previous_loop_token_installed() {
        let (mut controller, mut realtime) =
            create_timeline_runtime_with_capacities(8, 8, 2).unwrap();
        let timeline = timeline(2.0);
        controller.install(1, Arc::clone(&timeline)).unwrap();
        realtime.apply_pending_at_block_boundary();
        controller.poll_event().unwrap();

        let mut installed_token = 0;
        for _ in 0..3 {
            let chase = controller
                .prepare_loop_chase(&timeline, 1, 0, TimelineChaseOptions::default())
                .unwrap();
            installed_token = controller.install_loop_chase(chase).unwrap();
            assert_eq!(realtime.apply_pending_at_block_boundary(), 1);
            controller.poll_event().unwrap();
        }
        let blocked = controller
            .prepare_loop_chase(&timeline, 1, 0, TimelineChaseOptions::default())
            .unwrap();
        let blocked_token = controller.install_loop_chase(blocked).unwrap();
        assert_eq!(realtime.apply_pending_at_block_boundary(), 0);
        assert_eq!(realtime.pending_commands(), 1);
        assert_eq!(realtime.stats().retire_backpressure, 1);
        assert_eq!(
            realtime.activate_discontinuity(
                1,
                1,
                0,
                TimelineDiscontinuityKind::Loop {
                    token: blocked_token,
                },
            ),
            Err(TimelineDiscontinuityActivationError::LoopTokenMismatch {
                expected: installed_token,
                requested: blocked_token,
            })
        );
        realtime
            .activate_discontinuity(
                1,
                1,
                0,
                TimelineDiscontinuityKind::Loop {
                    token: installed_token,
                },
            )
            .unwrap();
        realtime.chase_for_block(1, 1, 0).unwrap();

        drop(controller.poll_retired().unwrap());
        assert_eq!(realtime.apply_pending_at_block_boundary(), 1);
        assert!(matches!(
            controller.poll_event(),
            Some(TimelineRuntimeEvent::LoopChaseInstalled { token, .. })
                if token == blocked_token
        ));
        controller.drain_retired();
        shutdown_same_thread(&mut controller, &mut realtime);
    }

    #[test]
    fn failed_loop_activation_leaves_the_active_cursor_unchanged() {
        let (mut controller, mut realtime) = create_timeline_runtime();
        let timeline = timeline(2.0);
        let loop_chase = controller
            .prepare_loop_chase(&timeline, 1, 0, TimelineChaseOptions::default())
            .unwrap();
        controller.install(1, timeline).unwrap();
        let token = controller.install_loop_chase(loop_chase).unwrap();
        realtime.apply_pending_at_block_boundary();
        poll_all(&mut controller);
        realtime
            .activate_discontinuity(1, 5, 0, TimelineDiscontinuityKind::Loop { token })
            .unwrap();
        realtime.chase_for_block(1, 5, 0).unwrap();
        let mut packet = TimelinePacket::<1>::new();
        realtime
            .packetize_block(1, 5, 0, 4, &mut packet)
            .unwrap()
            .commit();

        assert_eq!(
            realtime.activate_discontinuity(1, 6, 1, TimelineDiscontinuityKind::Loop { token },),
            Err(TimelineDiscontinuityActivationError::FrameMismatch {
                expected: 0,
                requested: 1,
            })
        );
        assert_eq!(realtime.active_epoch(), Some(5));
        assert_eq!(realtime.next_frame(), Some(4));
        shutdown_same_thread(&mut controller, &mut realtime);
    }

    #[test]
    fn clear_retires_one_shot_loop_template_and_timeline_off_callback() {
        let (mut controller, mut realtime) = create_timeline_runtime();
        let timeline = timeline(2.0);
        let one_shot = controller
            .prepare_chase(&timeline, 3, 1, 0, TimelineChaseOptions::default())
            .unwrap();
        let loop_chase = controller
            .prepare_loop_chase(&timeline, 3, 0, TimelineChaseOptions::default())
            .unwrap();
        controller.install(3, timeline).unwrap();
        controller.install_chase(one_shot).unwrap();
        controller.install_loop_chase(loop_chase).unwrap();
        assert_eq!(realtime.apply_pending_at_block_boundary(), 3);
        poll_all(&mut controller);

        controller.clear(3).unwrap();
        assert_eq!(realtime.apply_pending_at_block_boundary(), 1);
        controller.poll_event().unwrap();
        let mut saw_timeline = false;
        let mut saw_chases = false;
        while let Some(resource) = controller.poll_retired() {
            match resource {
                RetiredTimelineResource::Bundle {
                    one_shot,
                    loop_token,
                    loop_chase,
                    ..
                } => {
                    saw_timeline = true;
                    saw_chases = one_shot.is_some() && loop_token.is_some() && loop_chase.is_some();
                }
                RetiredTimelineResource::Timeline { .. } => saw_timeline = true,
                RetiredTimelineResource::ChaseBundle {
                    one_shot,
                    loop_token,
                    loop_chase,
                } => {
                    saw_chases = one_shot.is_some() && loop_token.is_some() && loop_chase.is_some();
                }
                RetiredTimelineResource::Chase { .. }
                | RetiredTimelineResource::LoopChase { .. } => {}
            }
        }
        assert!(saw_timeline && saw_chases);
        shutdown_same_thread(&mut controller, &mut realtime);
    }

    #[test]
    fn blocking_shutdown_is_acknowledged_and_retires_on_control_thread() {
        let (mut controller, mut realtime) = create_timeline_runtime();
        activate(&mut controller, &mut realtime, timeline(1.0), 1, 1, 0);
        let worker = thread::spawn(move || {
            while !realtime.is_shutdown() {
                realtime.apply_pending_at_block_boundary();
                thread::yield_now();
            }
            realtime
        });
        controller.shutdown_blocking().unwrap();
        let realtime = worker.join().unwrap();
        assert!(realtime.is_shutdown());
        assert_eq!(controller.confirmed_revision(), None);
        assert_eq!(controller.stats().unexpected_realtime_drops, 0);
        drop(realtime);
    }

    #[test]
    fn controller_drop_never_waits_for_an_unserviced_callback() {
        let (controller, mut realtime) = create_timeline_runtime();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let worker = thread::spawn(move || {
            drop(controller);
            done_tx.send(()).unwrap();
        });
        done_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("controller Drop must be nonblocking");
        worker.join().unwrap();
        assert_eq!(realtime.pending_commands(), 1);
        realtime.apply_pending_at_block_boundary();
        assert!(realtime.is_shutdown());
    }

    #[test]
    fn unexpected_realtime_drop_forgets_active_arc_and_reports_fault() {
        let (mut controller, mut realtime) = create_timeline_runtime();
        let timeline = timeline(1.0);
        let observer = Arc::clone(&timeline);
        controller.install(1, timeline).unwrap();
        realtime.apply_pending_at_block_boundary();
        controller.poll_event().unwrap();
        assert_eq!(Arc::strong_count(&observer), 2);
        drop(realtime);
        assert_eq!(Arc::strong_count(&observer), 2);
        assert_eq!(controller.stats().unexpected_realtime_drops, 1);
        assert!(controller.needs_resync());
        assert_eq!(
            controller.request_shutdown(),
            Err(TimelineControlError::RealtimeUnavailable)
        );
    }

    #[test]
    fn runtime_validates_queue_and_chase_absolute_capacities() {
        assert!(matches!(
            create_timeline_runtime_with_capacities(0, 1, 2),
            Err(TimelineRuntimeCreateError::InvalidQueueCapacity)
        ));
        assert!(matches!(
            create_timeline_runtime_with_capacities(1, 1, 1),
            Err(TimelineRuntimeCreateError::RetireCapacityTooSmall)
        ));
        let (mut controller, mut realtime) = create_timeline_runtime();
        let base = AutomationBaseValue {
            target: CompiledAutomationTarget::MasterVolume,
            value: 1.0,
        };
        let chase = Box::new(PreparedTimelineChase {
            plugin_topology_revision: 0,
            plugin_timing: crate::plugin_timing::PreparedPluginTimingPlan::conservative(48000)
                .unwrap(),
            revision: 1,
            epoch: 1,
            frame: 0,
            state: TimelineDiscontinuityState {
                frame: 0,
                audio_clips: Vec::new(),
                automation_layers: Vec::new(),
                automation_bases: vec![base; MAX_RUNTIME_AUTOMATION_BASES + 1],
                notes: Vec::new(),
            },
        });
        let error = controller.install_chase(chase).unwrap_err();
        assert!(matches!(
            error.reason,
            TimelineResourceQueueFailure::Validation(
                TimelineRuntimeValidationError::ResourceLimitExceeded {
                    resource: TimelineRuntimeResource::ChaseAutomationBases,
                    ..
                }
            )
        ));
        drop(error.into_resource());
        shutdown_same_thread(&mut controller, &mut realtime);
    }

    #[test]
    fn install_preflight_matches_every_executor_capacity() {
        assert_eq!(MAX_RUNTIME_AUTOMATION_BASES, EXECUTOR_MAX_AUTOMATION_BASES);
        assert!(matches!(
            validate_resource(
                TimelineRuntimeResource::TimelineAutomationBases,
                EXECUTOR_MAX_AUTOMATION_BASES + 1,
                MAX_RUNTIME_AUTOMATION_BASES,
            ),
            Err(TimelineRuntimeValidationError::ResourceLimitExceeded {
                resource: TimelineRuntimeResource::TimelineAutomationBases,
                ..
            })
        ));

        let source = NoteSourceDescriptor::ChannelStep {
            clip_id: 1,
            pattern_id: 1,
            step: 0,
            repetition: 0,
        };
        let notes = (1..=EXECUTOR_MAX_ACTIVE_NOTES + 1)
            .map(|note_id| TimelineEvent {
                frame: 0,
                kind: TimelineEventKind::NoteOn {
                    note_id: note_id as u64,
                    channel_id: 1,
                    note: 60,
                    velocity: 1.0,
                    gain: 1.0,
                    mixer_track: 1,
                    source,
                },
            })
            .collect::<Vec<_>>();
        assert!(matches!(
            validate_executor_event_slice(&notes),
            Err(TimelineRuntimeValidationError::ResourceLimitExceeded {
                resource: TimelineRuntimeResource::ConcurrentNotes,
                actual,
                maximum: EXECUTOR_MAX_ACTIVE_NOTES,
            }) if actual == EXECUTOR_MAX_ACTIVE_NOTES + 1
        ));

        let audio = (1..=EXECUTOR_MAX_ACTIVE_AUDIO_CLIPS + 1)
            .map(|clip_id| TimelineEvent {
                frame: 0,
                kind: TimelineEventKind::AudioStart(AudioClipDescriptor {
                    clip_id: clip_id as u32,
                    asset_id: clip_id as u64,
                    start_frame: 0,
                    source_offset_frame: 0,
                    source_elapsed_frames: 0,
                    timeline_sample_rate: 48_000,
                    source_sample_rate: 48_000,
                    clip_end_frame: 100,
                    stop_frame: 100,
                    gain: 1.0,
                    fades: crate::clip_fade::CompiledClipFades::default(),
                    mixer_track: 1,
                }),
            })
            .collect::<Vec<_>>();
        assert!(matches!(
            validate_executor_event_slice(&audio),
            Err(TimelineRuntimeValidationError::ResourceLimitExceeded {
                resource: TimelineRuntimeResource::ConcurrentAudioClips,
                actual,
                maximum: EXECUTOR_MAX_ACTIVE_AUDIO_CLIPS,
            }) if actual == EXECUTOR_MAX_ACTIVE_AUDIO_CLIPS + 1
        ));

        let layers = (1..=EXECUTOR_MAX_ACTIVE_AUTOMATION_LAYERS + 1)
            .map(|automation_id| TimelineEvent {
                frame: 0,
                kind: TimelineEventKind::AutomationRamp(AutomationRampDescriptor {
                    automation_id: automation_id as u64,
                    placement_id: None,
                    precedence: automation_id as u64,
                    target: CompiledAutomationTarget::MasterVolume,
                    start_value: 0.0,
                    end_value: 1.0,
                    end_frame: 100,
                    shape: AutomationRampShape::Linear,
                    source_curve: AutomationCurve::Linear,
                }),
            })
            .collect::<Vec<_>>();
        assert!(matches!(
            validate_executor_event_slice(&layers),
            Err(TimelineRuntimeValidationError::ResourceLimitExceeded {
                resource: TimelineRuntimeResource::ConcurrentAutomationLayers,
                actual,
                maximum: EXECUTOR_MAX_ACTIVE_AUTOMATION_LAYERS,
            }) if actual == EXECUTOR_MAX_ACTIVE_AUTOMATION_LAYERS + 1
        ));
    }

    #[test]
    fn callback_work_budget_and_fixed_packet_path_are_bounded() {
        let (mut controller, mut realtime) =
            create_timeline_runtime_with_capacities(16, 16, 16).unwrap();
        for revision in 1..=9 {
            controller.install(revision, timeline(1.0)).unwrap();
        }
        assert_eq!(
            realtime.apply_pending_at_block_boundary_with_budget(usize::MAX),
            8
        );
        assert_eq!(realtime.pending_commands(), 1);
        poll_all(&mut controller);
        controller.drain_retired();
        assert_eq!(realtime.apply_pending_at_block_boundary(), 1);
        controller.poll_event().unwrap();
        controller.drain_retired();

        let active = timeline(1.0);
        let chase = controller
            .prepare_chase(&active, 10, 1, 0, TimelineChaseOptions::default())
            .unwrap();
        controller.install(10, active).unwrap();
        controller.install_chase(chase).unwrap();
        realtime.apply_pending_at_block_boundary();
        poll_all(&mut controller);
        controller.drain_retired();
        realtime
            .activate_discontinuity(10, 1, 0, TimelineDiscontinuityKind::OneShot)
            .unwrap();
        realtime.chase_for_block(10, 1, 0).unwrap();
        let mut packet = TimelinePacket::<1>::new();
        for frame in 0..4 {
            realtime
                .packetize_block(10, 1, frame, 1, &mut packet)
                .unwrap()
                .commit();
            assert!(packet.is_empty());
        }
        assert!(!realtime.stats().ownership_needs_resync);
        shutdown_same_thread(&mut controller, &mut realtime);
    }

    #[test]
    fn same_graph_install_reuses_the_exact_bank_and_retires_old_bundle_without_it() {
        let (mut controller, mut realtime) = create_timeline_runtime();
        let first = timeline(1.0);
        let fingerprint = first.mixer_graph().fingerprint();
        let bank = Box::new(PreparedMixerGraphDelayBank::new(first.mixer_graph(), 32).unwrap());
        controller
            .install_with_mixer_resources(1, first, bank)
            .unwrap();
        assert_eq!(realtime.apply_pending_at_block_boundary(), 1);
        let bank_address = realtime
            .active_mixer_delay_bank()
            .map(|bank| std::ptr::from_ref(bank).addr())
            .unwrap();
        controller.poll_event().unwrap();

        controller
            .install_reusing_mixer_resources(2, timeline(1.0), fingerprint)
            .unwrap();
        assert_eq!(realtime.apply_pending_at_block_boundary(), 1);
        assert_eq!(realtime.active_revision(), Some(2));
        assert_eq!(
            realtime
                .active_mixer_delay_bank()
                .map(|bank| std::ptr::from_ref(bank).addr()),
            Some(bank_address)
        );
        controller.poll_event().unwrap();
        assert!(matches!(
            controller.poll_retired(),
            Some(RetiredTimelineResource::Bundle {
                revision: 1,
                mixer_delay_bank: None,
                ..
            })
        ));
        shutdown_same_thread(&mut controller, &mut realtime);
    }

    #[test]
    fn unavailable_same_graph_reuse_rejects_without_replacing_legacy_active_bundle() {
        let (mut controller, mut realtime) = create_timeline_runtime();
        let first = timeline(1.0);
        let fingerprint = first.mixer_graph().fingerprint();
        controller.install(1, first).unwrap();
        assert_eq!(realtime.apply_pending_at_block_boundary(), 1);
        controller.poll_event().unwrap();

        controller
            .install_reusing_mixer_resources(2, timeline(1.0), fingerprint)
            .unwrap();
        assert_eq!(realtime.apply_pending_at_block_boundary(), 1);
        assert_eq!(realtime.active_revision(), Some(1));
        assert!(realtime.active_mixer_delay_bank().is_none());
        assert!(matches!(
            controller.poll_event(),
            Some(TimelineRuntimeEvent::Rejected {
                revision: 2,
                reason: TimelineRejectReason::MixerDelayBankReuseUnavailable {
                    fingerprint: rejected,
                },
                ..
            }) if rejected == fingerprint
        ));
        assert!(matches!(
            controller.poll_retired(),
            Some(RetiredTimelineResource::Timeline {
                revision: 2,
                mixer_delay_bank: None,
                ..
            })
        ));
        shutdown_same_thread(&mut controller, &mut realtime);
    }

    #[test]
    fn active_candidate_reuse_borrows_then_promotes_the_same_bank_without_retiring_it() {
        let (mut controller, mut realtime) = create_timeline_runtime();
        activate_with_mixer_bank(&mut controller, &mut realtime, timeline(1.0), 1, 1, 0);
        let bank_address = realtime
            .active_mixer_delay_bank()
            .map(|bank| std::ptr::from_ref(bank).addr())
            .unwrap();
        let loop_token = stage_same_graph_reuse_candidate(&mut controller, &mut realtime, 2, 2);
        assert_eq!(realtime.active_revision(), Some(1));
        assert!(
            realtime
                .candidate_timeline
                .as_ref()
                .is_some_and(|candidate| candidate.mixer_delay_bank.is_none()
                    && candidate.mixer_delay_bank_reuse_fingerprint
                        == Some(candidate.timeline.mixer_graph().fingerprint()))
        );

        controller
            .activate_transport(replacement_activation_spec(2, 2, loop_token), 0)
            .unwrap();
        assert_eq!(realtime.apply_pending_at_block_boundary(), 1);
        let ticket = realtime.pending_transport_activation().unwrap();
        realtime.preflight_transport_activation(ticket, 2).unwrap();
        assert_eq!(
            realtime
                .transport_activation_mixer_delay_bank(ticket)
                .map(|bank| std::ptr::from_ref(bank).addr()),
            Some(bank_address),
            "candidate preflight must borrow A's bank without moving it"
        );
        let committed = realtime.commit_preflighted_transport_activation(ticket, 2);
        assert_eq!(realtime.active_revision(), Some(2));
        assert_eq!(
            realtime
                .active_mixer_delay_bank()
                .map(|bank| std::ptr::from_ref(bank).addr()),
            Some(bank_address)
        );
        realtime.publish_committed_transport_activation(committed);
        controller.poll_event().unwrap();
        realtime.apply_pending_at_block_boundary();
        assert!(matches!(
            controller.poll_retired(),
            Some(RetiredTimelineResource::Bundle {
                revision: 1,
                mixer_delay_bank: None,
                ..
            })
        ));
        shutdown_same_thread(&mut controller, &mut realtime);
    }

    #[test]
    fn rejected_candidate_reuse_preserves_active_bank_address_targets_and_cursor_history() {
        let (mut controller, mut realtime) = create_timeline_runtime();
        let active = timeline(1.0);
        let graph = active.mixer_graph().clone();
        activate_with_mixer_bank(&mut controller, &mut realtime, active, 1, 1, 0);

        let mut stage_latencies = [0_u64; crate::mixer_graph::MIXER_GRAPH_MAX_NODES];
        stage_latencies[1] = 8;
        let plan =
            crate::pdc::GraphPdcPlan::build_for_mixer_graph(&graph, &stage_latencies, &[], 32)
                .unwrap();
        let bank = realtime.active_mixer_delay_bank_mut().unwrap();
        bank.request_plan(&plan, 0).unwrap();
        bank.reset();
        assert_eq!(bank.process_sample(1, [1.0; 2]), Some([0.0; 2]));
        let bank_address = std::ptr::from_ref(&*bank).addr();
        let current_before = bank.current_delay_samples(1);
        let target_before = bank.target_delay_samples(1);

        let valid_loop_token =
            stage_same_graph_reuse_candidate(&mut controller, &mut realtime, 2, 2);
        let invalid_loop_token = valid_loop_token.wrapping_add(1).max(1);
        controller
            .activate_transport(replacement_activation_spec(2, 2, invalid_loop_token), 0)
            .unwrap();
        assert_eq!(realtime.apply_pending_at_block_boundary(), 1);
        let ticket = realtime.pending_transport_activation().unwrap();
        assert_eq!(
            realtime
                .transport_activation_mixer_delay_bank(ticket)
                .map(|bank| std::ptr::from_ref(bank).addr()),
            Some(bank_address)
        );
        let runtime_error = realtime
            .preflight_transport_activation(ticket, 2)
            .unwrap_err();
        realtime.reject_pending_transport_activation(
            ticket,
            TimelineTransportActivationRejectReason::Runtime(runtime_error),
        );

        let bank = realtime.active_mixer_delay_bank_mut().unwrap();
        assert_eq!(std::ptr::from_ref(&*bank).addr(), bank_address);
        assert_eq!(bank.current_delay_samples(1), current_before);
        assert_eq!(bank.target_delay_samples(1), target_before);
        for _ in 0..7 {
            assert_eq!(bank.process_sample(1, [0.0; 2]), Some([0.0; 2]));
        }
        assert_eq!(
            bank.process_sample(1, [0.0; 2]),
            Some([1.0; 2]),
            "rejected activation must neither reset nor advance A's route history"
        );
        assert_eq!(realtime.active_revision(), Some(1));
        shutdown_same_thread(&mut controller, &mut realtime);
    }
}
