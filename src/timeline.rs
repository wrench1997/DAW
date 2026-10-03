//! Immutable, sample-clock timeline compilation for the audio scheduler.
//!
//! Compilation is deliberately a control-thread operation: it validates project
//! data, expands Playlist repetitions, converts every musical position through
//! [`TempoMap`], and sorts a single immutable event stream. The realtime side can
//! then copy a block into a fixed-capacity [`TimelinePacket`] without allocating.

use std::{
    collections::{BTreeMap, BTreeSet},
    mem::MaybeUninit,
};

use thiserror::Error;

use crate::{
    automation::{AutomationCurve, AutomationTarget, canonicalize_automation_target},
    mixer_graph::{CompiledMixerGraph, MixerTrackId, compile_mixer_graph},
    model::{AudioAsset, Clip, ClipKind, Project},
    tempo_map::TempoMap,
};

/// FL-compatible Channel Rack resolution used by persisted patterns.
pub const STEPS_PER_BEAT: usize = 4;
/// Default number of events that can be delivered in one audio block.
pub const DEFAULT_TIMELINE_PACKET_CAPACITY: usize = 512;

/// Absolute limits are intentionally not caller-configurable. Project loading is
/// an untrusted boundary, and a caller-provided `usize::MAX` must never turn a
/// diagnostic limit into an allocation or CPU denial of service.
pub const MAX_TIMELINE_EVENTS: usize = 1_048_576;
pub const MAX_TIMELINE_DIAGNOSTICS: usize = 4_096;
pub const MAX_TIMELINE_WORK_UNITS: usize = 67_108_864;
pub const MAX_TIMELINE_AUTOMATION_SEGMENTS_PER_LANE: usize = 262_144;

/// Largest timeline render block owned by the audio callback. Runtime
/// installation validates every arbitrarily aligned half-open window of this
/// width, rather than assuming device callbacks are aligned to song frame zero.
pub const TIMELINE_CALLBACK_MAX_FRAMES: usize = 2_048;
/// Fixed event-plan capacity shared by packet execution and the callback sink.
pub const TIMELINE_CALLBACK_MAX_EVENTS: usize = 4_096;
/// Fixed plug-in quantum selected by the audio runtime.
pub const TIMELINE_PLUGIN_FIXED_QUANTUM_FRAMES: usize = 128;
/// Number of complete fixed plug-in quanta in the largest callback block.
pub const TIMELINE_PLUGIN_FIXED_QUANTA_PER_CALLBACK: usize =
    TIMELINE_CALLBACK_MAX_FRAMES / TIMELINE_PLUGIN_FIXED_QUANTUM_FRAMES;
/// Per-endpoint system-event reserve. One CC123 message per MIDI channel is
/// staged at every transport epoch reset; normal timeline and live-control
/// traffic may not consume this lane.
pub const TIMELINE_ENDPOINT_SYSTEM_MAX_EVENTS_PER_QUANTUM: usize = 16;
pub const TIMELINE_ENDPOINT_SYSTEM_MAX_EVENTS_PER_CALLBACK: usize = 16;
/// Per-endpoint sequenced timeline lane. Plug-in parameter automation consumes
/// one event per driven parameter at every fixed-quantum boundary.
pub const TIMELINE_ENDPOINT_TIMELINE_MAX_EVENTS_PER_QUANTUM: usize = 96;
pub const TIMELINE_ENDPOINT_TIMELINE_MAX_EVENTS_PER_CALLBACK: usize = 224;
/// Per-endpoint live-control reserve, isolated from sequenced automation.
pub const TIMELINE_ENDPOINT_LIVE_MAX_EVENTS_PER_QUANTUM: usize = 16;
pub const TIMELINE_ENDPOINT_LIVE_MAX_EVENTS_PER_CALLBACK: usize = 16;
/// Inline capacity of the three fixed-quantum event lanes combined.
pub const TIMELINE_ENDPOINT_MAX_EVENTS_PER_QUANTUM: usize =
    TIMELINE_ENDPOINT_SYSTEM_MAX_EVENTS_PER_QUANTUM
        + TIMELINE_ENDPOINT_TIMELINE_MAX_EVENTS_PER_QUANTUM
        + TIMELINE_ENDPOINT_LIVE_MAX_EVENTS_PER_QUANTUM;
/// Inline callback capacity of the three endpoint event lanes combined.
pub const TIMELINE_ENDPOINT_MAX_EVENTS_PER_CALLBACK: usize =
    TIMELINE_ENDPOINT_SYSTEM_MAX_EVENTS_PER_CALLBACK
        + TIMELINE_ENDPOINT_TIMELINE_MAX_EVENTS_PER_CALLBACK
        + TIMELINE_ENDPOINT_LIVE_MAX_EVENTS_PER_CALLBACK;
/// One CC123 message per MIDI channel is staged at every transport epoch reset.
pub const TIMELINE_ENDPOINT_DISCONTINUITY_EVENTS: usize =
    TIMELINE_ENDPOINT_SYSTEM_MAX_EVENTS_PER_QUANTUM;
/// Maximum number of Q128-driven plug-in parameters assigned to one physical
/// generator endpoint.
pub const TIMELINE_ENDPOINT_MAX_DRIVEN_PLUGIN_PARAMETERS: usize = 8;
/// Maximum number of Q128-driven plug-in parameters in one compiled timeline.
pub const TIMELINE_MAX_DRIVEN_PLUGIN_PARAMETERS: usize = 64;

const _: () = assert!(
    TIMELINE_CALLBACK_MAX_FRAMES.is_multiple_of(TIMELINE_PLUGIN_FIXED_QUANTUM_FRAMES),
    "callback maximum must contain an integral number of plug-in quanta"
);
const _: () = assert!(TIMELINE_ENDPOINT_MAX_EVENTS_PER_QUANTUM == 128);
const _: () = assert!(TIMELINE_ENDPOINT_MAX_EVENTS_PER_CALLBACK == 256);
const _: () = assert!(
    TIMELINE_ENDPOINT_DISCONTINUITY_EVENTS == TIMELINE_ENDPOINT_SYSTEM_MAX_EVENTS_PER_QUANTUM
);
const _: () = assert!(
    TIMELINE_ENDPOINT_DISCONTINUITY_EVENTS <= TIMELINE_ENDPOINT_SYSTEM_MAX_EVENTS_PER_CALLBACK
);
/// Number of decoded assets that can be callback-resident at once.
pub const TIMELINE_CALLBACK_MAX_AUDIO_ASSETS: usize = 128;
/// Number of worker-backed generator identities the callback can route without
/// allocation. Runtime installation rejects a larger compiled route table so
/// control-side validation and callback storage cannot drift apart.
pub const TIMELINE_CALLBACK_MAX_GENERATOR_ENDPOINTS: usize = 64;
/// Number of physical Mixer Insert workers addressable by one callback. Every
/// plug-in slot on a track shares that track's worker and endpoint event budget.
pub const TIMELINE_CALLBACK_MAX_MIXER_ENDPOINTS: usize = 32;
/// Maximum number of physical plug-in endpoints in one callback transaction.
pub const TIMELINE_CALLBACK_MAX_PLUGIN_ENDPOINTS: usize =
    TIMELINE_CALLBACK_MAX_GENERATOR_ENDPOINTS + TIMELINE_CALLBACK_MAX_MIXER_ENDPOINTS;

const _: () = assert!(TIMELINE_CALLBACK_MAX_PLUGIN_ENDPOINTS == 96);

const MAX_TIMELINE_CHANNELS: usize = 4_096;
const MAX_TIMELINE_PATTERNS: usize = 65_536;
const MAX_TIMELINE_CLIPS: usize = 262_144;
const MAX_TIMELINE_AUTOMATION_LANES: usize = 65_536;
const MAX_TIMELINE_AUDIO_ASSETS: usize = 65_536;
const MAX_TIMELINE_PLUGIN_INSTANCES: usize = 65_536;
const MAX_STEP_NOTE_OVERRIDES: usize = 4_096;
const MAX_AUTOMATION_SEGMENT_FRAMES: u32 = u16::MAX as u32 + 1;
const PLUGIN_ROUTE_MIXER_TRACK_COUNT: usize = TIMELINE_CALLBACK_MAX_MIXER_ENDPOINTS;
const PLUGIN_ROUTE_MIXER_SLOT_COUNT: usize = 10;

const LEGACY_STEP_NOTES: [u8; 5] = [36, 39, 54, 43, 64];
const DEFAULT_STEP_NOTE: u8 = 60;
const MAX_PERSISTED_PATTERN_STEPS: usize = 16;

/// Optional per-channel note override for Channel Rack triggers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChannelStepNote {
    pub channel_id: u32,
    pub note: u8,
}

/// Resource limits and explicit legacy Channel Rack timing defaults.
#[derive(Clone, Debug, PartialEq)]
pub struct TimelineCompileOptions {
    /// Optional Channel Rack note overrides, keyed by stable project channel id.
    pub channel_step_notes: Vec<ChannelStepNote>,
    /// Gate used for boolean Channel Rack steps.
    pub step_gate_beats: f64,
    /// Repeat period for legacy Piano Roll data.
    pub legacy_piano_period_beats: f64,
    /// Maximum number of mixer routes accepted from the project.
    pub max_mixer_tracks: usize,
    /// Hard ceiling for the immutable event vector.
    pub max_events: usize,
    /// Hard ceiling for diagnostics retained in memory.
    pub max_diagnostics: usize,
    /// Maximum length of one automation ramp descriptor.
    pub max_automation_segment_frames: u32,
    /// Hard ceiling per automation lane; compilation fails instead of truncating.
    pub max_automation_segments_per_lane: usize,
    /// Deterministic work budget covering expansion even when it emits no events.
    pub max_work_units: usize,
}

impl Default for TimelineCompileOptions {
    fn default() -> Self {
        Self {
            channel_step_notes: Vec::new(),
            step_gate_beats: 0.25,
            legacy_piano_period_beats: 16.0,
            max_mixer_tracks: 32,
            max_events: 1_048_576,
            max_diagnostics: 4_096,
            max_automation_segment_frames: 2_048,
            max_automation_segments_per_lane: MAX_TIMELINE_AUTOMATION_SEGMENTS_PER_LANE,
            max_work_units: MAX_TIMELINE_WORK_UNITS,
        }
    }
}

/// Stable reference to the project entity involved in a diagnostic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimelineEntity {
    Project,
    Channel(u32),
    Pattern(u32),
    Clip(u32),
    Automation(u64),
    AudioAsset(u64),
    PluginInstance(u64),
}

/// Recoverable project problems are retained here instead of being silently repaired.
#[derive(Clone, Debug, PartialEq)]
pub enum TimelineDiagnosticKind {
    InvalidSwing {
        value: f32,
        fallback: f32,
    },
    DuplicateChannelId {
        channel_id: u32,
    },
    InvalidChannelId,
    InvalidChannelVolume {
        value: f32,
    },
    InvalidMixerRoute {
        route: usize,
        maximum: usize,
    },
    DuplicatePatternId {
        pattern_id: u32,
    },
    InvalidClipId,
    DuplicateClipId {
        clip_id: u32,
    },
    InvalidPatternLength {
        length_steps: usize,
    },
    PatternChannelCountMismatch {
        expected: usize,
        actual: usize,
    },
    MissingPattern {
        pattern_id: u32,
    },
    InvalidClipRange {
        start: f32,
        length: f32,
    },
    ClipOutsideTempoMap,
    ClipEndTruncated {
        requested_end_beat: f64,
        map_end_beat: f64,
    },
    InvalidClipGain {
        value: f32,
    },
    InvalidPianoNote {
        note_index: usize,
    },
    NoteRangeCollapsed,
    LegacyPianoChannelRequired {
        note_count: usize,
    },
    LegacyPianoChannelMissing {
        channel_id: u32,
    },
    MissingAudioAsset {
        asset_id: Option<u64>,
    },
    DuplicateAudioAssetId {
        asset_id: u64,
    },
    InvalidAudioAsset,
    InvalidAudioSourceOffset {
        beat: f32,
    },
    MissingNativeAudioSourceOffset,
    AudioSourcePastEnd {
        source_frame: u64,
        asset_frames: u64,
    },
    AudioHasNoPlayableFrames,
    InvalidFade {
        fade_in: f32,
        fade_out: f32,
    },
    InvalidAutomationId,
    DuplicateAutomationId {
        automation_id: u64,
    },
    InvalidAutomationTarget,
    InvalidPluginInstanceId,
    DuplicatePluginInstanceId {
        instance_id: u64,
    },
    MissingPluginInstance {
        instance_id: u64,
    },
    InvalidPluginGeneratorRoute {
        instance_id: u64,
        channel_id: u32,
    },
    InvalidPluginMixerRoute {
        instance_id: u64,
        track: usize,
        slot: usize,
    },
    DuplicatePluginMixerSlot {
        track: usize,
        slot: usize,
        references: usize,
    },
    AmbiguousPluginRoute {
        instance_id: u64,
        references: usize,
    },
    PluginAutomationRouteUnavailable {
        instance_id: u64,
    },
    AutomationPointOutsideTempoMap {
        beat: f64,
    },
    AutomationRangeCollapsed,
    TempoConversionFailed {
        beat: f64,
    },
}

/// One bounded diagnostic emitted by compilation.
#[derive(Clone, Debug, PartialEq)]
pub struct TimelineDiagnostic {
    pub entity: TimelineEntity,
    pub kind: TimelineDiagnosticKind,
}

/// Counts useful for UI reporting and compile-time telemetry.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TimelineCompileStats {
    pub note_pairs: usize,
    pub audio_clips: usize,
    pub automation_segments: usize,
    pub muted_clips_skipped: usize,
    pub inaudible_notes_skipped: usize,
    pub invalid_items_skipped: usize,
    pub diagnostics_suppressed: usize,
    /// Largest number of events sharing one exact frame. Consumers whose fixed
    /// packet is smaller must use [`CompiledTimeline::event_range`] chunks.
    pub max_events_at_frame: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimelineResource {
    Channels,
    Patterns,
    Clips,
    AutomationLanes,
    AudioAssets,
    PluginInstances,
    StepNoteOverrides,
    Events,
    Diagnostics,
    AutomationSegments,
    AutomationSegmentFrames,
    WorkUnits,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum TimelineCompileError {
    #[error("step gate must be finite and greater than zero")]
    InvalidStepGate,
    #[error("legacy Piano Roll period must be finite and greater than zero")]
    InvalidLegacyPianoPeriod,
    #[error("mixer-track, event and diagnostic limits must be non-zero")]
    InvalidCapacity,
    #[error("automation segment frame and per-lane limits must be non-zero")]
    InvalidAutomationLimit,
    #[error("Channel Rack note override for channel {channel_id} is duplicated")]
    DuplicateStepNoteOverride { channel_id: u32 },
    #[error("Channel Rack note override {note} for channel {channel_id} exceeds MIDI note 127")]
    InvalidStepNoteOverride { channel_id: u32, note: u8 },
    #[error("compiled timeline exceeds its {maximum}-event limit")]
    EventLimitExceeded { maximum: usize },
    #[error("automation lane {automation_id} exceeds its {maximum}-segment compilation limit")]
    AutomationSegmentLimitExceeded { automation_id: u64, maximum: usize },
    #[error("tempo map rejected validated timeline frame {frame}")]
    TempoMapFrameConversion { frame: u64 },
    #[error("requested {resource:?} limit {requested} exceeds the absolute maximum {maximum}")]
    RequestedLimitExceedsMaximum {
        resource: TimelineResource,
        requested: usize,
        maximum: usize,
    },
    #[error("project contains {actual} {resource:?}, exceeding the absolute maximum {maximum}")]
    ProjectLimitExceeded {
        resource: TimelineResource,
        actual: usize,
        maximum: usize,
    },
    #[error("timeline compilation exceeds its deterministic {maximum}-unit work budget")]
    WorkLimitExceeded { maximum: usize },
    #[error("project mixer graph is invalid: {reason}")]
    InvalidMixerGraph { reason: String },
}

/// Describes where a note originated without retaining project allocations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteSourceDescriptor {
    ChannelStep {
        clip_id: u32,
        pattern_id: u32,
        step: u16,
        repetition: u32,
    },
    LegacyPiano {
        clip_id: u32,
        pattern_id: u32,
        persistent_note_id: u64,
        note_index: u32,
        repetition: u32,
    },
}

/// Copy-only target representation suitable for a realtime packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CompiledAutomationTarget {
    MasterVolume,
    MasterPan,
    Tempo,
    Swing,
    MixerVolume { track: u16 },
    MixerPan { track: u16 },
    MixerMute { track: u16 },
    ChannelVolume { channel_id: u32 },
    ChannelPan { channel_id: u32 },
    ChannelMute { channel_id: u32 },
    PluginParameter { instance_id: u64, parameter_id: u32 },
}

/// Non-automated value restored after the last active layer for a target ends.
/// The table is compiled on the control thread and is complete for every target
/// emitted by this timeline, including plugin parameters absent from old project
/// files (whose defined fallback is `0.0`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AutomationBaseValue {
    pub target: CompiledAutomationTarget,
    pub value: f32,
}

/// Static Channel Rack state. Notes are always compiled regardless of mute/solo;
/// the consumer combines this state with automation at render time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChannelBaseDescriptor {
    pub channel_id: u32,
    pub volume: f32,
    pub pan: f32,
    pub muted: bool,
    pub solo: bool,
    pub mixer_track: u16,
}

/// Callback-ready destination of one persisted plug-in instance. Mixer slots
/// are dense runtime-chain indices, not the possibly sparse persisted slot
/// numbers. A channel generator currently occupies the only slot in its chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PluginRouteDestination {
    Generator { channel_id: u32, slot: u8 },
    MixerInsert { track: u8, slot: u8 },
}

/// Stable, copy-only lookup row for sample-offset plug-in parameter delivery.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CompiledPluginRoute {
    pub instance_id: u64,
    pub destination: PluginRouteDestination,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutomationRampShape {
    Linear,
    Hold,
}

/// A bounded automation span. Tension curves are compiled into bounded linear spans.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AutomationRampDescriptor {
    pub automation_id: u64,
    /// `None` denotes a global lane with no Playlist placements.
    pub placement_id: Option<u32>,
    /// Project lane index. Higher precedence wins while multiple lanes target
    /// the same control, matching `TempoMap` and the current project runtime.
    pub precedence: u64,
    pub target: CompiledAutomationTarget,
    pub start_value: f32,
    pub end_value: f32,
    /// Absolute exclusive end frame.
    pub end_frame: u64,
    pub shape: AutomationRampShape,
    pub source_curve: AutomationCurve,
}

/// Metadata-only audio placement. No asset file is touched by compilation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudioClipDescriptor {
    pub clip_id: u32,
    pub asset_id: u64,
    /// Absolute inclusive timeline start.
    pub start_frame: u64,
    /// Resolved native asset-frame position. Song tempo never reinterprets it.
    pub source_offset_frame: u64,
    /// Required to advance the source at original speed after a discontinuity.
    pub source_sample_rate: u32,
    /// Requested exclusive Playlist end, before truncation to available media.
    pub clip_end_frame: u64,
    pub stop_frame: u64,
    pub gain: f32,
    pub fade_in_frames: u64,
    pub fade_out_frames: u64,
    pub mixer_track: u16,
}

impl AudioClipDescriptor {
    /// Resolves the fractional asset position immediately before events at
    /// `timeline_frame` are applied.
    #[must_use]
    pub fn source_position_at(self, timeline_frame: u64, timeline_sample_rate: u32) -> f64 {
        let elapsed = timeline_frame.saturating_sub(self.start_frame);
        self.source_offset_frame as f64
            + elapsed as f64 * f64::from(self.source_sample_rate)
                / f64::from(timeline_sample_rate.max(1))
    }
}

/// Copy-only event payload consumed by the later audio callback integration.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TimelineEventKind {
    NoteOn {
        note_id: u64,
        channel_id: u32,
        note: u8,
        velocity: f32,
        gain: f32,
        mixer_track: u16,
        source: NoteSourceDescriptor,
    },
    NoteOff {
        note_id: u64,
        channel_id: u32,
        note: u8,
        mixer_track: u16,
        source: NoteSourceDescriptor,
    },
    AudioStart(AudioClipDescriptor),
    AudioStop {
        clip_id: u32,
        asset_id: u64,
    },
    AutomationRamp(AutomationRampDescriptor),
    AutomationEnd {
        automation_id: u64,
        placement_id: Option<u32>,
        /// Removes exactly this precedence layer. Consumers retain lower layers
        /// so ending a later lane immediately reveals the still-active winner.
        precedence: u64,
        target: CompiledAutomationTarget,
    },
}

/// One event at an absolute device-independent timeline frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimelineEvent {
    pub frame: u64,
    pub kind: TimelineEventKind,
}

/// Policy for notes that began before a discontinuity and have not ended yet.
/// Chasing can be useful for sustained instruments, but can also replay attacks;
/// consequently the safe default is deliberately `DoNotChase`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LongNoteChasePolicy {
    #[default]
    DoNotChase,
    Chase,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TimelineChaseOptions {
    pub long_notes: LongNoteChasePolicy,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChasedAudioClip {
    pub descriptor: AudioClipDescriptor,
    pub source_position_frame: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChasedAutomationLayer {
    pub automation_id: u64,
    pub placement_id: Option<u32>,
    pub precedence: u64,
    pub target: CompiledAutomationTarget,
    pub current_value: f32,
    pub end_value: f32,
    pub end_frame: u64,
    pub shape: AutomationRampShape,
    pub source_curve: AutomationCurve,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChasedNote {
    pub note_id: u64,
    pub channel_id: u32,
    pub note: u8,
    pub velocity: f32,
    /// Clip gain only. Channel volume/mute/solo comes from the base/automation plan.
    pub gain: f32,
    pub mixer_track: u16,
    pub source: NoteSourceDescriptor,
}

/// State to install immediately before processing events at `frame` after a
/// stop/seek/loop epoch reset. Events exactly at `frame` remain in the normal
/// packet, which prevents duplicated starts while preserving exact boundary order.
#[derive(Clone, Debug, PartialEq)]
pub struct TimelineDiscontinuityState {
    pub frame: u64,
    pub audio_clips: Vec<ChasedAudioClip>,
    pub automation_layers: Vec<ChasedAutomationLayer>,
    pub automation_bases: Vec<AutomationBaseValue>,
    pub notes: Vec<ChasedNote>,
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum TimelineChaseError {
    #[error("discontinuity frame {frame} exceeds timeline duration {duration_frames}")]
    FrameOutOfRange { frame: u64, duration_frames: u64 },
}

/// Fully validated, deterministic event stream for one project/tempo-map revision.
#[derive(Clone, Debug, PartialEq)]
pub struct CompiledTimeline {
    sample_rate: u32,
    duration_frames: u64,
    events: Vec<TimelineEvent>,
    audio_clips: Vec<AudioClipDescriptor>,
    automation_bases: Vec<AutomationBaseValue>,
    driven_automation_targets: Vec<CompiledAutomationTarget>,
    channel_bases: Vec<ChannelBaseDescriptor>,
    plugin_routes: Vec<CompiledPluginRoute>,
    mixer_graph: CompiledMixerGraph,
    diagnostics: Vec<TimelineDiagnostic>,
    stats: TimelineCompileStats,
}

impl CompiledTimeline {
    pub fn from_project(
        project: &Project,
        tempo_map: &TempoMap,
        options: TimelineCompileOptions,
    ) -> Result<Self, TimelineCompileError> {
        Compiler::new(project, tempo_map, options)?.compile()
    }

    #[must_use]
    pub const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    #[must_use]
    pub const fn duration_frames(&self) -> u64 {
        self.duration_frames
    }

    #[must_use]
    pub fn events(&self) -> &[TimelineEvent] {
        &self.events
    }

    #[must_use]
    pub fn audio_clips(&self) -> &[AudioClipDescriptor] {
        &self.audio_clips
    }

    #[must_use]
    pub fn automation_bases(&self) -> &[AutomationBaseValue] {
        &self.automation_bases
    }

    /// Sorted, unique controls that have at least one compiled automation
    /// segment. A control-thread owner can use this bounded manifest to prepare
    /// only the delay/state resources that the realtime callback will consume.
    /// Disabled, empty, invalid, and fully skipped lanes are excluded.
    #[must_use]
    pub fn driven_automation_targets(&self) -> &[CompiledAutomationTarget] {
        &self.driven_automation_targets
    }

    #[must_use]
    pub fn channel_bases(&self) -> &[ChannelBaseDescriptor] {
        &self.channel_bases
    }

    /// Sorted by `instance_id`, so a realtime owner may copy the table into
    /// fixed storage or use a bounded binary search without consulting Project.
    #[must_use]
    pub fn plugin_routes(&self) -> &[CompiledPluginRoute] {
        &self.plugin_routes
    }

    #[must_use]
    pub const fn mixer_graph(&self) -> &CompiledMixerGraph {
        &self.mixer_graph
    }

    #[must_use]
    pub fn diagnostics(&self) -> &[TimelineDiagnostic] {
        &self.diagnostics
    }

    #[must_use]
    pub const fn stats(&self) -> TimelineCompileStats {
        self.stats
    }

    /// Reconstructs state immediately before events at `frame` are processed.
    /// This is a control-thread operation and intentionally returns owned vectors
    /// for installation into caller-preallocated realtime state at an epoch swap.
    pub fn chase_discontinuity(
        &self,
        frame: u64,
        options: TimelineChaseOptions,
    ) -> Result<TimelineDiscontinuityState, TimelineChaseError> {
        if frame > self.duration_frames {
            return Err(TimelineChaseError::FrameOutOfRange {
                frame,
                duration_frames: self.duration_frames,
            });
        }

        let audio_clips = self
            .audio_clips
            .iter()
            .copied()
            .filter(|descriptor| descriptor.start_frame < frame && descriptor.stop_frame >= frame)
            .map(|descriptor| ChasedAudioClip {
                source_position_frame: descriptor.source_position_at(frame, self.sample_rate),
                descriptor,
            })
            .collect();

        let event_end = self.events.partition_point(|event| event.frame < frame);
        let mut layers =
            BTreeMap::<(CompiledAutomationTarget, u64), (u64, AutomationRampDescriptor)>::new();
        let mut notes = BTreeMap::<u64, ChasedNote>::new();
        for event in &self.events[..event_end] {
            match event.kind {
                TimelineEventKind::AutomationRamp(ramp) => {
                    layers.insert((ramp.target, ramp.precedence), (event.frame, ramp));
                }
                TimelineEventKind::AutomationEnd {
                    precedence, target, ..
                } => {
                    layers.remove(&(target, precedence));
                }
                TimelineEventKind::NoteOn {
                    note_id,
                    channel_id,
                    note,
                    velocity,
                    gain,
                    mixer_track,
                    source,
                } if options.long_notes == LongNoteChasePolicy::Chase => {
                    notes.insert(
                        note_id,
                        ChasedNote {
                            note_id,
                            channel_id,
                            note,
                            velocity,
                            gain,
                            mixer_track,
                            source,
                        },
                    );
                }
                TimelineEventKind::NoteOff { note_id, .. }
                    if options.long_notes == LongNoteChasePolicy::Chase =>
                {
                    notes.remove(&note_id);
                }
                _ => {}
            }
        }
        let automation_layers = layers
            .into_values()
            .map(|(start_frame, ramp)| ChasedAutomationLayer {
                automation_id: ramp.automation_id,
                placement_id: ramp.placement_id,
                precedence: ramp.precedence,
                target: ramp.target,
                current_value: automation_ramp_value(start_frame, frame, ramp),
                end_value: ramp.end_value,
                end_frame: ramp.end_frame,
                shape: ramp.shape,
                source_curve: ramp.source_curve,
            })
            .collect();

        Ok(TimelineDiscontinuityState {
            frame,
            audio_clips,
            automation_layers,
            automation_bases: self.automation_bases.clone(),
            notes: notes.into_values().collect(),
        })
    }

    /// Starts an allocation-free, explicitly chunkable event range. Chunks keep
    /// exact event order, including more same-frame events than one packet can hold.
    pub fn event_range(
        &self,
        epoch: u64,
        start_frame: u64,
        frames: u32,
    ) -> Result<TimelineEventRange<'_>, TimelinePacketError> {
        let (first, last) = self.event_bounds(start_frame, frames)?;
        Ok(TimelineEventRange {
            timeline: self,
            epoch,
            start_frame,
            frames,
            next: first,
            end: last,
        })
    }

    /// Copies events in `start_frame..start_frame + frames` into caller-owned RT storage.
    ///
    /// The method performs two binary searches and fixed-array copies only. On overflow,
    /// `packet` is left unchanged, so callers never observe a truncated block.
    pub fn packetize_into<const CAPACITY: usize>(
        &self,
        packet: &mut TimelinePacket<CAPACITY>,
        epoch: u64,
        start_frame: u64,
        frames: u32,
    ) -> Result<(), TimelinePacketError> {
        let (first, last) = self.event_bounds(start_frame, frames)?;
        let required = last - first;
        if required > CAPACITY {
            return Err(TimelinePacketError::CapacityExceeded {
                capacity: CAPACITY,
                required,
            });
        }

        packet.clear(epoch, start_frame, frames);
        for event in &self.events[first..last] {
            let relative = event.frame - start_frame;
            // `frames <= 65_536` and the range is half-open, so this always fits.
            let sample_offset = u16::try_from(relative).expect("validated packet offset");
            packet.push(TimelinePacketEvent {
                sample_offset,
                kind: event.kind,
            });
        }
        Ok(())
    }

    fn event_bounds(
        &self,
        start_frame: u64,
        frames: u32,
    ) -> Result<(usize, usize), TimelinePacketError> {
        if frames > u32::from(u16::MAX) + 1 {
            return Err(TimelinePacketError::FrameCountTooLarge { frames });
        }
        let end_frame = start_frame
            .checked_add(u64::from(frames))
            .ok_or(TimelinePacketError::FrameRangeOverflow)?;
        let first = self
            .events
            .partition_point(|event| event.frame < start_frame);
        let last = self.events.partition_point(|event| event.frame < end_frame);
        Ok((first, last))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimelinePacketEvent {
    pub sample_offset: u16,
    pub kind: TimelineEventKind,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum TimelinePacketError {
    #[error("timeline packet frame count {frames} cannot be represented by u16 offsets")]
    FrameCountTooLarge { frames: u32 },
    #[error("timeline packet frame range overflows u64")]
    FrameRangeOverflow,
    #[error("timeline packet needs {required} events but capacity is {capacity}")]
    CapacityExceeded { capacity: usize, required: usize },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimelinePacketChunk {
    pub copied_events: usize,
    pub remaining_events: usize,
}

/// Borrowed cursor for a block whose event burst may exceed packet capacity.
/// Call [`TimelineEventRange::packetize_next_into`] until `remaining_events == 0`;
/// every chunk retains the same epoch/block metadata and exact stable ordering.
pub struct TimelineEventRange<'a> {
    timeline: &'a CompiledTimeline,
    epoch: u64,
    start_frame: u64,
    frames: u32,
    next: usize,
    end: usize,
}

impl TimelineEventRange<'_> {
    #[must_use]
    pub fn remaining_events(&self) -> usize {
        self.end - self.next
    }

    pub fn packetize_next_into<const CAPACITY: usize>(
        &mut self,
        packet: &mut TimelinePacket<CAPACITY>,
    ) -> Result<TimelinePacketChunk, TimelinePacketError> {
        let remaining = self.remaining_events();
        if remaining != 0 && CAPACITY == 0 {
            return Err(TimelinePacketError::CapacityExceeded {
                capacity: 0,
                required: remaining,
            });
        }
        let copied = remaining.min(CAPACITY);
        packet.clear(self.epoch, self.start_frame, self.frames);
        for event in &self.timeline.events[self.next..self.next + copied] {
            let relative = event.frame - self.start_frame;
            let sample_offset = u16::try_from(relative).expect("validated packet offset");
            packet.push(TimelinePacketEvent {
                sample_offset,
                kind: event.kind,
            });
        }
        self.next += copied;
        Ok(TimelinePacketChunk {
            copied_events: copied,
            remaining_events: self.remaining_events(),
        })
    }
}

/// Fixed-capacity callback packet. Construction and refill never allocate.
pub struct TimelinePacket<const CAPACITY: usize = DEFAULT_TIMELINE_PACKET_CAPACITY> {
    epoch: u64,
    start_frame: u64,
    frames: u32,
    len: usize,
    events: [MaybeUninit<TimelinePacketEvent>; CAPACITY],
}

impl<const CAPACITY: usize> TimelinePacket<CAPACITY> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            epoch: 0,
            start_frame: 0,
            frames: 0,
            len: 0,
            events: [MaybeUninit::uninit(); CAPACITY],
        }
    }

    #[must_use]
    pub const fn epoch(&self) -> u64 {
        self.epoch
    }

    #[must_use]
    pub const fn start_frame(&self) -> u64 {
        self.start_frame
    }

    #[must_use]
    pub const fn frames(&self) -> u32 {
        self.frames
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[must_use]
    pub fn events(&self) -> &[TimelinePacketEvent] {
        // SAFETY: indices below `len` are initialized exclusively by `push`, and
        // TimelinePacketEvent is Copy and has no destructor. `clear` only shortens
        // the exposed prefix; it never exposes an uninitialized element.
        unsafe { std::slice::from_raw_parts(self.events.as_ptr().cast(), self.len) }
    }

    fn clear(&mut self, epoch: u64, start_frame: u64, frames: u32) {
        self.epoch = epoch;
        self.start_frame = start_frame;
        self.frames = frames;
        self.len = 0;
    }

    fn push(&mut self, event: TimelinePacketEvent) {
        debug_assert!(self.len < CAPACITY);
        self.events[self.len].write(event);
        self.len += 1;
    }
}

impl<const CAPACITY: usize> Default for TimelinePacket<CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug)]
struct ChannelState {
    id: u32,
    mixer_track: u16,
    step_note: u8,
    valid: bool,
}

impl Default for ChannelState {
    fn default() -> Self {
        Self {
            id: 0,
            mixer_track: 0,
            step_note: DEFAULT_STEP_NOTE,
            valid: false,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct ClipView {
    id: u32,
    start: f32,
    length: f32,
    kind: ClipKind,
    pattern_id: u32,
    automation_id: Option<u64>,
    audio_asset_id: Option<u64>,
    source_offset: f32,
    audio_source_offset_frame: Option<u64>,
    gain: f32,
    fade_in: f32,
    fade_out: f32,
    muted: bool,
}

impl From<&Clip> for ClipView {
    fn from(clip: &Clip) -> Self {
        Self {
            id: clip.id,
            start: clip.start,
            length: clip.length,
            kind: clip.kind,
            pattern_id: clip.pattern_id,
            automation_id: clip.automation_id,
            audio_asset_id: clip.audio_asset_id,
            source_offset: clip.source_offset,
            audio_source_offset_frame: clip.audio_source_offset_frame,
            gain: clip.gain,
            fade_in: clip.fade_in,
            fade_out: clip.fade_out,
            muted: clip.muted,
        }
    }
}

#[derive(Clone, Debug)]
struct SwingLaneSource {
    automation_index: usize,
    /// `None` is globally active; `Some([])` has placements but none active.
    placements: Option<Vec<AutomationPlacement>>,
}

#[derive(Clone, Copy, Debug)]
struct AutomationPlacement {
    clip_id: Option<u32>,
    start: f64,
    end: f64,
    source_offset: f64,
}

impl AutomationPlacement {
    fn source_beat(self, project_beat: f64) -> f64 {
        self.source_offset + project_beat - self.start
    }

    fn project_beat(self, source_beat: f64) -> f64 {
        self.start + source_beat - self.source_offset
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct EventSortKey {
    frame: u64,
    priority: u8,
    primary: u64,
    secondary: u64,
    ordinal: u64,
}

#[derive(Clone, Copy, Debug)]
struct PendingEvent {
    event: TimelineEvent,
    key: EventSortKey,
}

struct Compiler<'a> {
    project: &'a Project,
    tempo_map: &'a TempoMap,
    options: TimelineCompileOptions,
    channel_states: Vec<ChannelState>,
    channel_by_id: BTreeMap<u32, usize>,
    pattern_by_id: BTreeMap<u32, usize>,
    valid_clip_ids: BTreeSet<u32>,
    valid_automation_ids: BTreeSet<u64>,
    asset_by_id: BTreeMap<u64, usize>,
    plugin_ids: BTreeSet<u64>,
    plugin_routes: Vec<CompiledPluginRoute>,
    mixer_graph: CompiledMixerGraph,
    swing_sources: Vec<SwingLaneSource>,
    automation_bases: BTreeMap<CompiledAutomationTarget, f32>,
    driven_automation_targets: BTreeSet<CompiledAutomationTarget>,
    channel_bases: Vec<ChannelBaseDescriptor>,
    audio_clips: Vec<AudioClipDescriptor>,
    pending: Vec<PendingEvent>,
    diagnostics: Vec<TimelineDiagnostic>,
    stats: TimelineCompileStats,
    next_note_id: u64,
    next_ordinal: u64,
    next_automation_precedence: u64,
    base_swing: f64,
    work_units: usize,
}

impl<'a> Compiler<'a> {
    fn new(
        project: &'a Project,
        tempo_map: &'a TempoMap,
        options: TimelineCompileOptions,
    ) -> Result<Self, TimelineCompileError> {
        validate_options(&options)?;
        validate_project_limits(project)?;
        let mixer_graph = compile_mixer_graph(project).map_err(|error| {
            TimelineCompileError::InvalidMixerGraph {
                reason: error.to_string(),
            }
        })?;
        let base_swing = if project.swing.is_finite() && (0.0..=1.0).contains(&project.swing) {
            f64::from(project.swing)
        } else {
            0.0
        };
        let mut compiler = Self {
            project,
            tempo_map,
            channel_states: vec![ChannelState::default(); project.channels.len()],
            channel_by_id: BTreeMap::new(),
            pattern_by_id: BTreeMap::new(),
            valid_clip_ids: BTreeSet::new(),
            valid_automation_ids: BTreeSet::new(),
            asset_by_id: BTreeMap::new(),
            plugin_ids: BTreeSet::new(),
            plugin_routes: Vec::new(),
            mixer_graph,
            swing_sources: Vec::new(),
            automation_bases: BTreeMap::new(),
            driven_automation_targets: BTreeSet::new(),
            channel_bases: Vec::new(),
            audio_clips: Vec::new(),
            pending: Vec::new(),
            diagnostics: Vec::new(),
            stats: TimelineCompileStats::default(),
            next_note_id: 1,
            next_ordinal: 0,
            next_automation_precedence: 0,
            base_swing,
            work_units: 0,
            options,
        };
        if base_swing == 0.0
            && (!project.swing.is_finite() || !(0.0..=1.0).contains(&project.swing))
        {
            compiler.report(
                TimelineEntity::Project,
                TimelineDiagnosticKind::InvalidSwing {
                    value: project.swing,
                    fallback: 0.0,
                },
            );
        }
        compiler.charge_work(
            project
                .channels
                .len()
                .saturating_add(project.patterns.len())
                .saturating_add(project.audio_assets.len())
                .saturating_add(project.plugin_instances.len()),
        )?;
        compiler.index_project();
        compiler.compile_plugin_routes();
        compiler.build_swing_sources()?;
        compiler.seed_base_plan();
        Ok(compiler)
    }

    fn compile(mut self) -> Result<CompiledTimeline, TimelineCompileError> {
        for clip_index in 0..self.project.clips.len() {
            self.charge_work(1)?;
            let clip = ClipView::from(&self.project.clips[clip_index]);
            if !self.valid_clip_ids.contains(&clip.id) {
                continue;
            }
            match clip.kind {
                ClipKind::Pattern => self.compile_pattern_clip(&clip)?,
                ClipKind::Audio => self.compile_audio_clip(&clip)?,
                ClipKind::Automation => {}
            }
        }
        self.compile_automation()?;
        self.pending.sort_by_key(|pending| pending.key);
        self.stats.max_events_at_frame = maximum_same_frame_burst(&self.pending);
        let events = self
            .pending
            .into_iter()
            .map(|pending| pending.event)
            .collect();
        Ok(CompiledTimeline {
            sample_rate: self.tempo_map.sample_rate(),
            duration_frames: self.tempo_map.duration_frames(),
            events,
            audio_clips: self.audio_clips,
            automation_bases: self
                .automation_bases
                .into_iter()
                .map(|(target, value)| AutomationBaseValue { target, value })
                .collect(),
            driven_automation_targets: self.driven_automation_targets.into_iter().collect(),
            channel_bases: self.channel_bases,
            plugin_routes: self.plugin_routes,
            mixer_graph: self.mixer_graph,
            diagnostics: self.diagnostics,
            stats: self.stats,
        })
    }

    fn index_project(&mut self) {
        let mut channel_counts = BTreeMap::<u32, usize>::new();
        for channel in &self.project.channels {
            *channel_counts.entry(channel.id).or_default() += 1;
        }
        for index in 0..self.project.channels.len() {
            let channel = &self.project.channels[index];
            let mut valid = true;
            if channel.id == 0 {
                self.report(
                    TimelineEntity::Channel(channel.id),
                    TimelineDiagnosticKind::InvalidChannelId,
                );
                valid = false;
            } else if channel_counts.get(&channel.id).copied().unwrap_or(0) != 1 {
                self.report(
                    TimelineEntity::Channel(channel.id),
                    TimelineDiagnosticKind::DuplicateChannelId {
                        channel_id: channel.id,
                    },
                );
                valid = false;
            }
            if !channel.volume.is_finite() || !(0.0..=1.0).contains(&channel.volume) {
                self.report(
                    TimelineEntity::Channel(channel.id),
                    TimelineDiagnosticKind::InvalidChannelVolume {
                        value: channel.volume,
                    },
                );
                valid = false;
            }
            let mixer_runtime_slot = self
                .project
                .mixer_runtime_slot(channel.mixer_track)
                .filter(|slot| *slot < self.options.max_mixer_tracks);
            if mixer_runtime_slot.is_none() {
                self.report(
                    TimelineEntity::Channel(channel.id),
                    TimelineDiagnosticKind::InvalidMixerRoute {
                        route: usize::MAX,
                        maximum: self.options.max_mixer_tracks,
                    },
                );
                valid = false;
            }
            let step_note = self
                .options
                .channel_step_notes
                .iter()
                .find(|mapping| mapping.channel_id == channel.id)
                .map_or_else(
                    || {
                        LEGACY_STEP_NOTES
                            .get(index)
                            .copied()
                            .unwrap_or(DEFAULT_STEP_NOTE)
                    },
                    |mapping| mapping.note,
                );
            let state = ChannelState {
                id: channel.id,
                mixer_track: mixer_runtime_slot
                    .and_then(|slot| u16::try_from(slot).ok())
                    .unwrap_or(u16::MAX),
                step_note,
                valid,
            };
            self.channel_states[index] = state;
            if valid {
                self.channel_by_id.insert(channel.id, index);
            } else {
                self.stats.invalid_items_skipped += 1;
            }
        }

        let mut pattern_candidates = BTreeMap::<u32, usize>::new();
        let mut duplicate_patterns = BTreeSet::new();
        for (index, pattern) in self.project.patterns.iter().enumerate() {
            if pattern_candidates.insert(pattern.id, index).is_some() {
                duplicate_patterns.insert(pattern.id);
            }
        }
        for pattern_id in duplicate_patterns {
            pattern_candidates.remove(&pattern_id);
            self.report(
                TimelineEntity::Pattern(pattern_id),
                TimelineDiagnosticKind::DuplicatePatternId { pattern_id },
            );
            self.stats.invalid_items_skipped += 1;
        }
        self.pattern_by_id = pattern_candidates;

        let mut clip_counts = BTreeMap::<u32, usize>::new();
        for clip in &self.project.clips {
            *clip_counts.entry(clip.id).or_default() += 1;
        }
        for (clip_id, count) in clip_counts {
            if clip_id == 0 {
                self.report(
                    TimelineEntity::Clip(clip_id),
                    TimelineDiagnosticKind::InvalidClipId,
                );
                self.stats.invalid_items_skipped =
                    self.stats.invalid_items_skipped.saturating_add(count);
            } else if count > 1 {
                self.report(
                    TimelineEntity::Clip(clip_id),
                    TimelineDiagnosticKind::DuplicateClipId { clip_id },
                );
                self.stats.invalid_items_skipped =
                    self.stats.invalid_items_skipped.saturating_add(count);
            } else {
                self.valid_clip_ids.insert(clip_id);
            }
        }

        let mut automation_counts = BTreeMap::<u64, usize>::new();
        for automation in &self.project.automation_lanes {
            *automation_counts.entry(automation.id).or_default() += 1;
        }
        for (automation_id, count) in automation_counts {
            if automation_id == 0 {
                self.report(
                    TimelineEntity::Automation(automation_id),
                    TimelineDiagnosticKind::InvalidAutomationId,
                );
                self.stats.invalid_items_skipped =
                    self.stats.invalid_items_skipped.saturating_add(count);
            } else if count > 1 {
                self.report(
                    TimelineEntity::Automation(automation_id),
                    TimelineDiagnosticKind::DuplicateAutomationId { automation_id },
                );
                self.stats.invalid_items_skipped =
                    self.stats.invalid_items_skipped.saturating_add(count);
            } else {
                self.valid_automation_ids.insert(automation_id);
            }
        }

        let mut asset_candidates = BTreeMap::<u64, usize>::new();
        let mut duplicate_assets = BTreeSet::new();
        for (index, asset) in self.project.audio_assets.iter().enumerate() {
            if asset_candidates.insert(asset.id, index).is_some() {
                duplicate_assets.insert(asset.id);
            }
        }
        for asset_id in duplicate_assets {
            asset_candidates.remove(&asset_id);
            self.report(
                TimelineEntity::AudioAsset(asset_id),
                TimelineDiagnosticKind::DuplicateAudioAssetId { asset_id },
            );
            self.stats.invalid_items_skipped += 1;
        }
        self.asset_by_id = asset_candidates;
    }

    fn compile_plugin_routes(&mut self) {
        let mut definition_counts = BTreeMap::<u64, usize>::new();
        for instance in &self.project.plugin_instances {
            *definition_counts.entry(instance.id).or_default() += 1;
        }
        let known_plugin_ids = definition_counts
            .keys()
            .copied()
            .filter(|instance_id| *instance_id != 0)
            .collect::<BTreeSet<_>>();
        for (&instance_id, &count) in &definition_counts {
            if instance_id == 0 {
                self.report(
                    TimelineEntity::PluginInstance(instance_id),
                    TimelineDiagnosticKind::InvalidPluginInstanceId,
                );
            } else if count == 1 {
                self.plugin_ids.insert(instance_id);
            } else {
                self.report(
                    TimelineEntity::PluginInstance(instance_id),
                    TimelineDiagnosticKind::DuplicatePluginInstanceId { instance_id },
                );
            }
        }

        let mut reference_counts = BTreeMap::<u64, usize>::new();
        let mut candidates = BTreeMap::<u64, PluginRouteDestination>::new();
        let mut invalid_route_ids = BTreeSet::<u64>::new();
        let mut missing_ids = BTreeSet::<u64>::new();
        let mut invalid_generators = Vec::<(u64, u32)>::new();
        let mut invalid_inserts = Vec::<(u64, usize, usize)>::new();

        for (channel_index, channel) in self.project.channels.iter().enumerate() {
            let Some(instance_id) = channel.instrument_plugin_instance_id else {
                continue;
            };
            if !self.plugin_ids.contains(&instance_id) {
                if instance_id == 0 || !known_plugin_ids.contains(&instance_id) {
                    missing_ids.insert(instance_id);
                }
                continue;
            }
            *reference_counts.entry(instance_id).or_default() += 1;
            let valid_channel = self.channel_by_id.get(&channel.id) == Some(&channel_index);
            let mixer_runtime_slot = self
                .project
                .mixer_runtime_slot(channel.mixer_track)
                .filter(|slot| *slot < PLUGIN_ROUTE_MIXER_TRACK_COUNT);
            if !valid_channel || mixer_runtime_slot.is_none() {
                invalid_route_ids.insert(instance_id);
                invalid_generators.push((instance_id, channel.id));
                continue;
            }
            candidates.insert(
                instance_id,
                PluginRouteDestination::Generator {
                    channel_id: channel.id,
                    slot: 0,
                },
            );
        }

        // One persisted track may be sparse, while the worker chain is dense.
        // Aggregate physical positions first so iteration of this BTreeMap gives
        // the exact stable order used to construct the runtime chain.
        let mut mixer_locations = BTreeMap::<(usize, usize), (usize, u64)>::new();
        for slot_ref in &self.project.mixer_insert_slots {
            let instance_id = slot_ref.plugin_instance_id;
            if !self.plugin_ids.contains(&instance_id) {
                if instance_id == 0 || !known_plugin_ids.contains(&instance_id) {
                    missing_ids.insert(instance_id);
                }
                continue;
            }
            *reference_counts.entry(instance_id).or_default() += 1;
            let mixer_runtime_slot = self
                .project
                .mixer_runtime_slot(slot_ref.track)
                .filter(|slot| *slot < PLUGIN_ROUTE_MIXER_TRACK_COUNT);
            if mixer_runtime_slot.is_none() || slot_ref.slot >= PLUGIN_ROUTE_MIXER_SLOT_COUNT {
                invalid_route_ids.insert(instance_id);
                invalid_inserts.push((
                    instance_id,
                    mixer_runtime_slot
                        .unwrap_or_else(|| usize::try_from(slot_ref.track).unwrap_or(usize::MAX)),
                    slot_ref.slot,
                ));
                continue;
            }
            let mixer_runtime_slot = mixer_runtime_slot.expect("checked above");
            let location = mixer_locations
                .entry((mixer_runtime_slot, slot_ref.slot))
                .or_insert((0, instance_id));
            location.0 = location.0.saturating_add(1);
            if location.0 > 1 {
                invalid_route_ids.insert(location.1);
                invalid_route_ids.insert(instance_id);
            }
        }

        for (instance_id, channel_id) in invalid_generators {
            self.report(
                TimelineEntity::PluginInstance(instance_id),
                TimelineDiagnosticKind::InvalidPluginGeneratorRoute {
                    instance_id,
                    channel_id,
                },
            );
        }
        for (instance_id, track, slot) in invalid_inserts {
            self.report(
                TimelineEntity::PluginInstance(instance_id),
                TimelineDiagnosticKind::InvalidPluginMixerRoute {
                    instance_id,
                    track,
                    slot,
                },
            );
        }
        for instance_id in missing_ids {
            self.report(
                TimelineEntity::PluginInstance(instance_id),
                TimelineDiagnosticKind::MissingPluginInstance { instance_id },
            );
        }

        // Resolve instance ambiguity before assigning dense worker slots. If an
        // invalid instance occupied an earlier persisted position, charging it
        // a runtime slot here would leave a hole after the final retain pass.
        for (&instance_id, &references) in &reference_counts {
            if references > 1 {
                invalid_route_ids.insert(instance_id);
                self.report(
                    TimelineEntity::PluginInstance(instance_id),
                    TimelineDiagnosticKind::AmbiguousPluginRoute {
                        instance_id,
                        references,
                    },
                );
            }
        }

        let mut previous_track = None;
        let mut runtime_slot = 0_u8;
        for ((track, persisted_slot), (references, instance_id)) in mixer_locations {
            if previous_track != Some(track) {
                previous_track = Some(track);
                runtime_slot = 0;
            }
            if references > 1 {
                self.report(
                    TimelineEntity::Project,
                    TimelineDiagnosticKind::DuplicatePluginMixerSlot {
                        track,
                        slot: persisted_slot,
                        references,
                    },
                );
                continue;
            }
            if invalid_route_ids.contains(&instance_id) {
                continue;
            }
            candidates.insert(
                instance_id,
                PluginRouteDestination::MixerInsert {
                    track: u8::try_from(track).expect("plug-in mixer track validated"),
                    slot: runtime_slot,
                },
            );
            runtime_slot = runtime_slot.saturating_add(1);
        }
        candidates.retain(|instance_id, _| {
            reference_counts.get(instance_id) == Some(&1)
                && !invalid_route_ids.contains(instance_id)
        });
        self.plugin_routes = candidates
            .into_iter()
            .map(|(instance_id, destination)| CompiledPluginRoute {
                instance_id,
                destination,
            })
            .collect();
    }

    fn build_swing_sources(&mut self) -> Result<(), TimelineCompileError> {
        for automation_index in 0..self.project.automation_lanes.len() {
            let automation = &self.project.automation_lanes[automation_index];
            if !self.valid_automation_ids.contains(&automation.id)
                || !automation.lane.is_enabled()
                || automation.lane.points().is_empty()
                || !matches!(automation.lane.target(), AutomationTarget::Swing)
            {
                continue;
            }
            self.charge_work(self.project.clips.len())?;
            let mut has_placement = false;
            let mut placements = Vec::new();
            for project_clip in &self.project.clips {
                let clip = ClipView::from(project_clip);
                if clip.kind != ClipKind::Automation || clip.automation_id != Some(automation.id) {
                    continue;
                }
                has_placement = true;
                if !self.valid_clip_ids.contains(&clip.id) {
                    continue;
                }
                if clip.muted {
                    continue;
                }
                let start = f64::from(clip.start);
                let end = start + f64::from(clip.length);
                if start.is_finite() && end.is_finite() && start >= 0.0 && end > start {
                    let end = end.min(self.tempo_map.max_beat());
                    if end > start && start < self.tempo_map.max_beat() {
                        placements.push(AutomationPlacement {
                            clip_id: Some(clip.id),
                            start,
                            end,
                            source_offset: f64::from(clip.source_offset),
                        });
                    }
                }
            }
            self.swing_sources.push(SwingLaneSource {
                automation_index,
                placements: has_placement.then_some(placements),
            });
        }
        Ok(())
    }

    fn seed_base_plan(&mut self) {
        let master = self
            .project
            .mixer_track_by_id(crate::mixer_graph::MASTER_MIXER_TRACK_ID);
        self.insert_base(
            CompiledAutomationTarget::MasterVolume,
            finite_or(master.map_or(1.0, |track| track.volume), 1.0),
        );
        self.insert_base(
            CompiledAutomationTarget::MasterPan,
            finite_or(master.map_or(0.0, |track| track.pan), 0.0),
        );
        self.insert_base(
            CompiledAutomationTarget::Tempo,
            finite_or(self.project.tempo, 128.0).clamp(20.0, 400.0),
        );
        self.insert_base(CompiledAutomationTarget::Swing, self.base_swing as f32);

        for track in &self.project.mixer_tracks {
            let track_index = u16::from(track.runtime_slot);
            if usize::from(track_index) >= self.options.max_mixer_tracks {
                continue;
            }
            // Mixer zero is the master bus. Its volume/pan base was seeded
            // above under the canonical master targets; retaining aliases here
            // would create two independently chased controls for one DSP value.
            if track_index != 0 {
                self.automation_bases.insert(
                    CompiledAutomationTarget::MixerVolume { track: track_index },
                    finite_or(track.volume, 1.0),
                );
                self.automation_bases.insert(
                    CompiledAutomationTarget::MixerPan { track: track_index },
                    finite_or(track.pan, 0.0),
                );
            }
            self.automation_bases.insert(
                CompiledAutomationTarget::MixerMute { track: track_index },
                f32::from(track.muted),
            );
        }

        for (index, channel) in self.project.channels.iter().enumerate() {
            let state = self.channel_states[index];
            if !state.valid {
                continue;
            }
            self.channel_bases.push(ChannelBaseDescriptor {
                channel_id: state.id,
                volume: channel.volume,
                pan: finite_or(channel.pan, 0.0).clamp(-1.0, 1.0),
                muted: channel.muted,
                solo: channel.solo,
                mixer_track: state.mixer_track,
            });
            self.automation_bases.insert(
                CompiledAutomationTarget::ChannelVolume {
                    channel_id: state.id,
                },
                channel.volume,
            );
            self.automation_bases.insert(
                CompiledAutomationTarget::ChannelPan {
                    channel_id: state.id,
                },
                finite_or(channel.pan, 0.0).clamp(-1.0, 1.0),
            );
            self.automation_bases.insert(
                CompiledAutomationTarget::ChannelMute {
                    channel_id: state.id,
                },
                f32::from(channel.muted),
            );
        }

        for plugin in &self.project.plugin_instances {
            if self
                .plugin_routes
                .binary_search_by_key(&plugin.id, |route| route.instance_id)
                .is_err()
            {
                continue;
            }
            for (&parameter_id, &value) in &plugin.parameters {
                self.automation_bases.insert(
                    CompiledAutomationTarget::PluginParameter {
                        instance_id: plugin.id,
                        parameter_id,
                    },
                    finite_or(value, 0.0),
                );
            }
        }
    }

    fn insert_base(&mut self, target: CompiledAutomationTarget, value: f32) {
        self.automation_bases.insert(target, value);
    }

    fn charge_work(&mut self, units: usize) -> Result<(), TimelineCompileError> {
        self.work_units =
            self.work_units
                .checked_add(units)
                .ok_or(TimelineCompileError::WorkLimitExceeded {
                    maximum: self.options.max_work_units,
                })?;
        if self.work_units > self.options.max_work_units {
            return Err(TimelineCompileError::WorkLimitExceeded {
                maximum: self.options.max_work_units,
            });
        }
        Ok(())
    }

    fn charge_work_product(&mut self, factors: &[usize]) -> Result<(), TimelineCompileError> {
        let units = factors
            .iter()
            .try_fold(1_usize, |total, factor| total.checked_mul(*factor))
            .ok_or(TimelineCompileError::WorkLimitExceeded {
                maximum: self.options.max_work_units,
            })?;
        self.charge_work(units)
    }

    fn compile_pattern_clip(&mut self, clip: &ClipView) -> Result<(), TimelineCompileError> {
        if clip.muted {
            self.stats.muted_clips_skipped += 1;
            return Ok(());
        }
        let Some((clip_start, clip_end)) = self.clip_range(clip) else {
            return Ok(());
        };
        if !valid_gain(clip.gain) {
            self.report(
                TimelineEntity::Clip(clip.id),
                TimelineDiagnosticKind::InvalidClipGain { value: clip.gain },
            );
            self.stats.invalid_items_skipped += 1;
            return Ok(());
        }
        let Some(pattern_index) = self.pattern_by_id.get(&clip.pattern_id).copied() else {
            self.report(
                TimelineEntity::Clip(clip.id),
                TimelineDiagnosticKind::MissingPattern {
                    pattern_id: clip.pattern_id,
                },
            );
            self.stats.invalid_items_skipped += 1;
            return Ok(());
        };
        let pattern_id = self.project.patterns[pattern_index].id;
        let pattern_length = self.project.patterns[pattern_index].length_steps;
        let pattern_channel_count = self.project.patterns[pattern_index].channel_steps.len();
        if !(1..=MAX_PERSISTED_PATTERN_STEPS).contains(&pattern_length) {
            self.report(
                TimelineEntity::Pattern(pattern_id),
                TimelineDiagnosticKind::InvalidPatternLength {
                    length_steps: pattern_length,
                },
            );
            self.stats.invalid_items_skipped += 1;
            return Ok(());
        }
        if pattern_channel_count != self.project.channels.len() {
            self.report(
                TimelineEntity::Pattern(pattern_id),
                TimelineDiagnosticKind::PatternChannelCountMismatch {
                    expected: self.project.channels.len(),
                    actual: pattern_channel_count,
                },
            );
        }
        self.compile_channel_steps(
            clip,
            pattern_index,
            pattern_id,
            pattern_length,
            clip_start,
            clip_end,
        )?;
        self.compile_legacy_notes(clip, pattern_index, pattern_id, clip_start, clip_end)
    }

    fn compile_channel_steps(
        &mut self,
        clip: &ClipView,
        pattern_index: usize,
        pattern_id: u32,
        pattern_length: usize,
        clip_start: f64,
        clip_end: f64,
    ) -> Result<(), TimelineCompileError> {
        let period = pattern_length as f64 / STEPS_PER_BEAT as f64;
        let source_phase = f64::from(clip.source_offset).rem_euclid(period);
        let first_repetition_start = clip_start - source_phase;
        let repetitions = ((clip_end - first_repetition_start) / period)
            .ceil()
            .max(0.0) as u64;
        let repetition_count =
            usize::try_from(repetitions).map_err(|_| TimelineCompileError::WorkLimitExceeded {
                maximum: self.options.max_work_units,
            })?;
        self.charge_work_product(&[
            repetition_count,
            pattern_length,
            self.channel_states
                .len()
                .saturating_add(self.swing_sources.len())
                .saturating_add(1),
        ])?;
        for repetition in 0..repetitions {
            let repetition_start = first_repetition_start + repetition as f64 * period;
            for step in 0..pattern_length {
                let note_beat = self.swung_step_beat(repetition_start, step);
                if note_beat < clip_start || note_beat >= clip_end {
                    continue;
                }
                for channel_index in 0..self.channel_states.len() {
                    let state = self.channel_states[channel_index];
                    let enabled = self.project.patterns[pattern_index]
                        .channel_steps
                        .get(channel_index)
                        .is_some_and(|steps| steps[step]);
                    if !enabled || !state.valid {
                        continue;
                    }
                    let source = NoteSourceDescriptor::ChannelStep {
                        clip_id: clip.id,
                        pattern_id,
                        step: u16::try_from(step).expect("pattern step is bounded"),
                        repetition: u32::try_from(repetition).unwrap_or(u32::MAX),
                    };
                    let end_beat = (note_beat + self.options.step_gate_beats).min(clip_end);
                    self.push_note_pair(
                        note_beat,
                        end_beat,
                        state,
                        state.step_note,
                        1.0,
                        clip.gain,
                        source,
                    )?;
                }
            }
        }
        Ok(())
    }

    fn compile_legacy_notes(
        &mut self,
        clip: &ClipView,
        pattern_index: usize,
        pattern_id: u32,
        clip_start: f64,
        clip_end: f64,
    ) -> Result<(), TimelineCompileError> {
        let note_count = self.project.patterns[pattern_index].notes.len();
        if note_count == 0 {
            return Ok(());
        }
        let minimum_period = self.options.legacy_piano_period_beats;
        let content_end = self.project.patterns[pattern_index]
            .notes
            .iter()
            .filter_map(|note| {
                let end = f64::from(note.start) + f64::from(note.length.max(0.0));
                (note.start.is_finite() && note.length.is_finite() && end.is_finite() && end > 0.0)
                    .then_some(end)
            })
            .fold(0.0_f64, f64::max);
        // Legacy projects did not persist a Piano Roll length. Preserve the
        // historical period as a minimum, but let a resized/extended score
        // grow to the next complete four-beat bar so visible notes are also
        // compiled when the containing Pattern Clip is long enough.
        let period = if content_end > minimum_period {
            (content_end / 4.0).ceil() * 4.0
        } else {
            minimum_period
        };
        let source_phase = f64::from(clip.source_offset).rem_euclid(period);
        let first_repetition_start = clip_start - source_phase;
        let repetitions = ((clip_end - first_repetition_start) / period)
            .ceil()
            .max(0.0) as u64;
        let repetition_count =
            usize::try_from(repetitions).map_err(|_| TimelineCompileError::WorkLimitExceeded {
                maximum: self.options.max_work_units,
            })?;
        self.charge_work_product(&[note_count, repetition_count])?;
        for note_index in 0..note_count {
            let note = &self.project.patterns[pattern_index].notes[note_index];
            let (
                persistent_note_id,
                note_channel_id,
                note_number,
                note_start,
                note_length,
                note_velocity,
                note_muted,
            ) = (
                note.id,
                note.channel_id,
                note.note,
                note.start,
                note.length,
                note.velocity,
                note.muted,
            );
            let valid = persistent_note_id != 0
                && note_channel_id.is_some_and(|channel| self.channel_by_id.contains_key(&channel))
                && note_number <= 127
                && note_start.is_finite()
                && note_length.is_finite()
                && note_velocity.is_finite()
                && note_start >= 0.0
                && f64::from(note_start) < period
                && note_length > 0.0
                && (0.0..=1.0).contains(&note_velocity);
            if note_muted {
                continue;
            }
            if !valid {
                self.report(
                    TimelineEntity::Pattern(pattern_id),
                    TimelineDiagnosticKind::InvalidPianoNote { note_index },
                );
                self.stats.invalid_items_skipped += 1;
                continue;
            }
            let channel_id = note_channel_id.expect("validated Piano Roll channel");
            let channel_index = self.channel_by_id[&channel_id];
            let state = self.channel_states[channel_index];
            for repetition in 0..repetitions {
                let note_beat =
                    first_repetition_start + repetition as f64 * period + f64::from(note_start);
                if note_beat < clip_start || note_beat >= clip_end {
                    continue;
                }
                let end_beat = (note_beat + f64::from(note_length)).min(clip_end);
                let source = NoteSourceDescriptor::LegacyPiano {
                    clip_id: clip.id,
                    pattern_id,
                    persistent_note_id,
                    note_index: u32::try_from(note_index).unwrap_or(u32::MAX),
                    repetition: u32::try_from(repetition).unwrap_or(u32::MAX),
                };
                self.push_note_pair(
                    note_beat,
                    end_beat,
                    state,
                    note_number,
                    note_velocity,
                    clip.gain,
                    source,
                )?;
            }
        }
        Ok(())
    }

    /// Canonical Channel Rack swing is local to each Pattern repetition and
    /// Playlist placement. An odd step samples the winning Swing lane at the
    /// start of its local two-step pair, avoiding a circular timing dependency.
    fn swung_step_beat(&self, repetition_start: f64, step: usize) -> f64 {
        let pair_start = repetition_start + (step / 2) as f64 * 0.5;
        if step.is_multiple_of(2) {
            pair_start
        } else {
            pair_start + 0.25 * (1.0 + self.swing_at(pair_start) * 0.75)
        }
    }

    fn swing_at(&self, beat: f64) -> f64 {
        self.swing_sources
            .iter()
            .rev()
            .find_map(|source| {
                let source_beat = match &source.placements {
                    None => beat,
                    Some(placements) => placements
                        .iter()
                        .rev()
                        .find(|placement| (placement.start..placement.end).contains(&beat))
                        .map(|placement| placement.source_beat(beat))?,
                };
                self.project.automation_lanes[source.automation_index]
                    .lane
                    .evaluate(source_beat)
                    .filter(|value| value.is_finite())
            })
            .unwrap_or(self.base_swing)
            .clamp(0.0, 1.0)
    }

    #[allow(clippy::too_many_arguments)]
    fn push_note_pair(
        &mut self,
        start_beat: f64,
        end_beat: f64,
        channel: ChannelState,
        note: u8,
        velocity: f32,
        clip_gain: f32,
        source: NoteSourceDescriptor,
    ) -> Result<(), TimelineCompileError> {
        let Some(start_frame) = self.frame_at(start_beat, TimelineEntity::Channel(channel.id))
        else {
            return Ok(());
        };
        let Some(end_frame) = self.frame_at(end_beat, TimelineEntity::Channel(channel.id)) else {
            return Ok(());
        };
        if end_frame <= start_frame {
            self.report(
                TimelineEntity::Channel(channel.id),
                TimelineDiagnosticKind::NoteRangeCollapsed,
            );
            self.stats.invalid_items_skipped += 1;
            return Ok(());
        }
        let note_id = self.next_note_id;
        self.next_note_id = self.next_note_id.wrapping_add(1).max(1);
        let gain = clip_gain;
        self.push_event(
            start_frame,
            4,
            u64::from(channel.id),
            note_id,
            TimelineEventKind::NoteOn {
                note_id,
                channel_id: channel.id,
                note,
                velocity,
                gain,
                mixer_track: channel.mixer_track,
                source,
            },
        )?;
        self.push_event(
            end_frame,
            0,
            u64::from(channel.id),
            note_id,
            TimelineEventKind::NoteOff {
                note_id,
                channel_id: channel.id,
                note,
                mixer_track: channel.mixer_track,
                source,
            },
        )?;
        self.stats.note_pairs += 1;
        Ok(())
    }

    fn compile_audio_clip(&mut self, clip: &ClipView) -> Result<(), TimelineCompileError> {
        if clip.muted {
            self.stats.muted_clips_skipped += 1;
            return Ok(());
        }
        let Some((start_beat, end_beat)) = self.clip_range(clip) else {
            return Ok(());
        };
        if !valid_gain(clip.gain) {
            self.report(
                TimelineEntity::Clip(clip.id),
                TimelineDiagnosticKind::InvalidClipGain { value: clip.gain },
            );
            self.stats.invalid_items_skipped += 1;
            return Ok(());
        }
        if !clip.fade_in.is_finite()
            || !clip.fade_out.is_finite()
            || !(0.0..=1.0).contains(&clip.fade_in)
            || !(0.0..=1.0).contains(&clip.fade_out)
        {
            self.report(
                TimelineEntity::Clip(clip.id),
                TimelineDiagnosticKind::InvalidFade {
                    fade_in: clip.fade_in,
                    fade_out: clip.fade_out,
                },
            );
            self.stats.invalid_items_skipped += 1;
            return Ok(());
        }
        let Some(asset_id) = clip.audio_asset_id else {
            self.report(
                TimelineEntity::Clip(clip.id),
                TimelineDiagnosticKind::MissingAudioAsset { asset_id: None },
            );
            self.stats.invalid_items_skipped += 1;
            return Ok(());
        };
        let Some(asset_index) = self.asset_by_id.get(&asset_id).copied() else {
            self.report(
                TimelineEntity::Clip(clip.id),
                TimelineDiagnosticKind::MissingAudioAsset {
                    asset_id: Some(asset_id),
                },
            );
            self.stats.invalid_items_skipped += 1;
            return Ok(());
        };
        let asset = &self.project.audio_assets[asset_index];
        let (asset_metadata_id, asset_sample_rate, asset_frames, asset_valid) = (
            asset.id,
            asset.sample_rate,
            asset.frames,
            valid_audio_asset(asset),
        );
        if !asset_valid {
            self.report(
                TimelineEntity::AudioAsset(asset_metadata_id),
                TimelineDiagnosticKind::InvalidAudioAsset,
            );
            self.stats.invalid_items_skipped += 1;
            return Ok(());
        }
        let mixer_track_id = self.project.audio_clip_mixer_track_id(clip.id).or_else(|| {
            (self.project.format_version < 8).then(|| {
                crate::model::mixer_track_id_for_runtime_slot(
                    self.project
                        .clips
                        .iter()
                        .find(|project_clip| project_clip.id == clip.id)
                        .map_or(1, |project_clip| {
                            project_clip.track.saturating_add(1).min(31) as u8
                        }),
                )
            })
        });
        let Some(route) = mixer_track_id
            .and_then(|id| self.project.mixer_runtime_slot(id))
            .filter(|slot| *slot < self.options.max_mixer_tracks)
        else {
            self.report(
                TimelineEntity::Clip(clip.id),
                TimelineDiagnosticKind::InvalidMixerRoute {
                    route: usize::MAX,
                    maximum: self.options.max_mixer_tracks,
                },
            );
            self.stats.invalid_items_skipped += 1;
            return Ok(());
        };
        let Some(source_offset_frame) = clip.audio_source_offset_frame else {
            self.report(
                TimelineEntity::Clip(clip.id),
                TimelineDiagnosticKind::MissingNativeAudioSourceOffset,
            );
            self.stats.invalid_items_skipped += 1;
            return Ok(());
        };
        if source_offset_frame >= asset_frames {
            self.report(
                TimelineEntity::Clip(clip.id),
                TimelineDiagnosticKind::AudioSourcePastEnd {
                    source_frame: source_offset_frame,
                    asset_frames,
                },
            );
            self.stats.invalid_items_skipped += 1;
            return Ok(());
        }
        let Some(start_frame) = self.frame_at(start_beat, TimelineEntity::Clip(clip.id)) else {
            return Ok(());
        };
        let Some(requested_stop_frame) = self.frame_at(end_beat, TimelineEntity::Clip(clip.id))
        else {
            return Ok(());
        };
        let fade_in_end_beat = start_beat + (end_beat - start_beat) * f64::from(clip.fade_in);
        let fade_out_start_beat = end_beat - (end_beat - start_beat) * f64::from(clip.fade_out);
        let Some(fade_in_end_frame) =
            self.frame_at(fade_in_end_beat, TimelineEntity::Clip(clip.id))
        else {
            return Ok(());
        };
        let Some(fade_out_start_frame) =
            self.frame_at(fade_out_start_beat, TimelineEntity::Clip(clip.id))
        else {
            return Ok(());
        };
        let available_asset_frames = asset_frames - source_offset_frame;
        let available_output_frames = scale_frames(
            available_asset_frames,
            self.tempo_map.sample_rate(),
            asset_sample_rate,
        );
        let stop_frame =
            requested_stop_frame.min(start_frame.saturating_add(available_output_frames));
        if stop_frame <= start_frame {
            self.report(
                TimelineEntity::Clip(clip.id),
                TimelineDiagnosticKind::AudioHasNoPlayableFrames,
            );
            self.stats.invalid_items_skipped += 1;
            return Ok(());
        }
        let descriptor = AudioClipDescriptor {
            clip_id: clip.id,
            asset_id,
            start_frame,
            source_offset_frame,
            source_sample_rate: asset_sample_rate,
            clip_end_frame: requested_stop_frame,
            stop_frame,
            gain: clip.gain,
            fade_in_frames: fade_in_end_frame.saturating_sub(start_frame),
            fade_out_frames: requested_stop_frame.saturating_sub(fade_out_start_frame),
            mixer_track: u16::try_from(route).expect("route validated"),
        };
        self.audio_clips.push(descriptor);
        self.push_event(
            start_frame,
            3,
            u64::from(clip.id),
            asset_id,
            TimelineEventKind::AudioStart(descriptor),
        )?;
        self.push_event(
            stop_frame,
            1,
            u64::from(clip.id),
            asset_id,
            TimelineEventKind::AudioStop {
                clip_id: clip.id,
                asset_id,
            },
        )?;
        self.stats.audio_clips += 1;
        Ok(())
    }

    fn compile_automation(&mut self) -> Result<(), TimelineCompileError> {
        for automation_index in 0..self.project.automation_lanes.len() {
            self.charge_work(1)?;
            let automation_id = self.project.automation_lanes[automation_index].id;
            if !self.valid_automation_ids.contains(&automation_id) {
                continue;
            }
            let lane = &self.project.automation_lanes[automation_index].lane;
            if !lane.is_enabled() || lane.points().is_empty() {
                continue;
            }
            let target_model = lane.target().clone();
            let source_curve = lane.curve();
            let Some(target) = self.compile_automation_target(automation_id, &target_model) else {
                self.stats.invalid_items_skipped += 1;
                continue;
            };
            self.ensure_target_base(target);
            let placements = self.automation_placements(automation_id)?;
            let mut lane_segment_count = 0_usize;
            for placement in placements {
                let precedence = self.next_automation_precedence;
                self.next_automation_precedence = self.next_automation_precedence.wrapping_add(1);
                let segments_before_range = lane_segment_count;
                let boundaries =
                    self.automation_boundaries(automation_id, automation_index, placement)?;
                for boundary_pair in boundaries.windows(2) {
                    let mut frame = boundary_pair[0];
                    let boundary_end = boundary_pair[1];
                    while frame < boundary_end {
                        let end_frame =
                            boundary_end.min(frame.saturating_add(u64::from(
                                self.options.max_automation_segment_frames,
                            )));
                        if lane_segment_count >= self.options.max_automation_segments_per_lane {
                            return Err(TimelineCompileError::AutomationSegmentLimitExceeded {
                                automation_id,
                                maximum: self.options.max_automation_segments_per_lane,
                            });
                        }
                        let start_beat = self
                            .tempo_map
                            .frame_to_beat(frame)
                            .map_err(|_| TimelineCompileError::TempoMapFrameConversion { frame })?;
                        let end_beat = self.tempo_map.frame_to_beat(end_frame).map_err(|_| {
                            TimelineCompileError::TempoMapFrameConversion { frame: end_frame }
                        })?;
                        let start_source_beat = placement.source_beat(start_beat);
                        let Some(start_value) = self.project.automation_lanes[automation_index]
                            .lane
                            .evaluate(start_source_beat)
                        else {
                            frame = end_frame;
                            continue;
                        };
                        // A ramp owns `start_frame..end_frame`; its endpoint is
                        // therefore the mathematical left limit. This matters at
                        // Hold points and loop wraps, where exact `end_beat` is the
                        // first value of the following discontinuous span.
                        let end_source_beat =
                            previous_f64(placement.source_beat(end_beat)).max(start_source_beat);
                        let Some(end_value) = self.project.automation_lanes[automation_index]
                            .lane
                            .evaluate(end_source_beat)
                        else {
                            frame = end_frame;
                            continue;
                        };
                        let descriptor = AutomationRampDescriptor {
                            automation_id,
                            placement_id: placement.clip_id,
                            precedence,
                            target,
                            start_value: start_value as f32,
                            end_value: end_value as f32,
                            end_frame,
                            shape: if source_curve == AutomationCurve::Hold {
                                AutomationRampShape::Hold
                            } else {
                                AutomationRampShape::Linear
                            },
                            source_curve,
                        };
                        self.push_event(
                            frame,
                            2,
                            automation_target_sort_key(target),
                            precedence,
                            TimelineEventKind::AutomationRamp(descriptor),
                        )?;
                        lane_segment_count += 1;
                        self.stats.automation_segments += 1;
                        frame = end_frame;
                    }
                }
                if lane_segment_count == segments_before_range {
                    continue;
                }
                let Some(end_frame) =
                    self.frame_at(placement.end, TimelineEntity::Automation(automation_id))
                else {
                    continue;
                };
                self.push_event(
                    end_frame,
                    2,
                    automation_target_sort_key(target),
                    precedence,
                    TimelineEventKind::AutomationEnd {
                        automation_id,
                        placement_id: placement.clip_id,
                        precedence,
                        target,
                    },
                )?;
            }
            if lane_segment_count != 0 {
                self.driven_automation_targets.insert(target);
            }
        }
        Ok(())
    }

    fn compile_automation_target(
        &mut self,
        automation_id: u64,
        target: &AutomationTarget,
    ) -> Option<CompiledAutomationTarget> {
        let compiled = match canonicalize_automation_target(target.clone()) {
            AutomationTarget::MasterVolume => CompiledAutomationTarget::MasterVolume,
            AutomationTarget::MasterPan => CompiledAutomationTarget::MasterPan,
            AutomationTarget::Tempo => CompiledAutomationTarget::Tempo,
            AutomationTarget::Swing => CompiledAutomationTarget::Swing,
            AutomationTarget::MixerVolume { track } => CompiledAutomationTarget::MixerVolume {
                track: self.valid_automation_track(automation_id, track)?,
            },
            AutomationTarget::MixerPan { track } => CompiledAutomationTarget::MixerPan {
                track: self.valid_automation_track(automation_id, track)?,
            },
            AutomationTarget::MixerMute { track } => CompiledAutomationTarget::MixerMute {
                track: self.valid_automation_track(automation_id, track)?,
            },
            AutomationTarget::ChannelVolume { channel } => {
                if !self.channel_by_id.contains_key(&channel) {
                    self.report_invalid_automation_target(automation_id);
                    return None;
                }
                CompiledAutomationTarget::ChannelVolume {
                    channel_id: channel,
                }
            }
            AutomationTarget::ChannelPan { channel } => {
                if !self.channel_by_id.contains_key(&channel) {
                    self.report_invalid_automation_target(automation_id);
                    return None;
                }
                CompiledAutomationTarget::ChannelPan {
                    channel_id: channel,
                }
            }
            AutomationTarget::ChannelMute { channel } => {
                if !self.channel_by_id.contains_key(&channel) {
                    self.report_invalid_automation_target(automation_id);
                    return None;
                }
                CompiledAutomationTarget::ChannelMute {
                    channel_id: channel,
                }
            }
            AutomationTarget::PluginParameter {
                instance,
                parameter,
            } => {
                if self
                    .plugin_routes
                    .binary_search_by_key(&instance, |route| route.instance_id)
                    .is_err()
                {
                    self.report(
                        TimelineEntity::Automation(automation_id),
                        TimelineDiagnosticKind::PluginAutomationRouteUnavailable {
                            instance_id: instance,
                        },
                    );
                    return None;
                }
                CompiledAutomationTarget::PluginParameter {
                    instance_id: instance,
                    parameter_id: parameter,
                }
            }
        };
        Some(compiled)
    }

    fn ensure_target_base(&mut self, target: CompiledAutomationTarget) {
        self.automation_bases
            .entry(target)
            .or_insert_with(|| match target {
                CompiledAutomationTarget::MasterVolume
                | CompiledAutomationTarget::MixerVolume { .. } => 1.0,
                CompiledAutomationTarget::MasterPan
                | CompiledAutomationTarget::MixerPan { .. }
                | CompiledAutomationTarget::ChannelPan { .. }
                | CompiledAutomationTarget::PluginParameter { .. } => 0.0,
                CompiledAutomationTarget::Tempo => {
                    finite_or(self.project.tempo, 128.0).clamp(20.0, 400.0)
                }
                CompiledAutomationTarget::Swing => self.base_swing as f32,
                CompiledAutomationTarget::MixerMute { .. }
                | CompiledAutomationTarget::ChannelMute { .. } => 0.0,
                CompiledAutomationTarget::ChannelVolume { channel_id } => self
                    .channel_by_id
                    .get(&channel_id)
                    .map_or(1.0, |index| self.project.channels[*index].volume),
            });
    }

    fn valid_automation_track(&mut self, automation_id: u64, track: MixerTrackId) -> Option<u16> {
        let runtime_slot = self
            .project
            .mixer_runtime_slot(track)
            .filter(|slot| *slot < self.options.max_mixer_tracks);
        if runtime_slot.is_none() {
            self.report_invalid_automation_target(automation_id);
        }
        runtime_slot.and_then(|slot| u16::try_from(slot).ok())
    }

    fn report_invalid_automation_target(&mut self, automation_id: u64) {
        self.report(
            TimelineEntity::Automation(automation_id),
            TimelineDiagnosticKind::InvalidAutomationTarget,
        );
    }

    fn automation_placements(
        &mut self,
        automation_id: u64,
    ) -> Result<Vec<AutomationPlacement>, TimelineCompileError> {
        self.charge_work(self.project.clips.len())?;
        let mut has_placement = false;
        let mut placements = Vec::new();
        for clip_index in 0..self.project.clips.len() {
            let clip = ClipView::from(&self.project.clips[clip_index]);
            if clip.kind != ClipKind::Automation || clip.automation_id != Some(automation_id) {
                continue;
            }
            has_placement = true;
            if !self.valid_clip_ids.contains(&clip.id) {
                continue;
            }
            if clip.muted {
                self.stats.muted_clips_skipped += 1;
                continue;
            }
            if let Some((start, end)) = self.clip_range(&clip) {
                placements.push(AutomationPlacement {
                    clip_id: Some(clip.id),
                    start,
                    end,
                    source_offset: f64::from(clip.source_offset),
                });
            }
        }
        if !has_placement {
            placements.push(AutomationPlacement {
                clip_id: None,
                start: 0.0,
                end: self.tempo_map.max_beat(),
                source_offset: 0.0,
            });
        }
        Ok(placements)
    }

    fn automation_boundaries(
        &mut self,
        automation_id: u64,
        automation_index: usize,
        placement: AutomationPlacement,
    ) -> Result<Vec<u64>, TimelineCompileError> {
        let source_start = placement.source_offset;
        let source_end = source_start + placement.end - placement.start;
        let mut source_beats = vec![source_start, source_end];
        let limit = self
            .options
            .max_automation_segments_per_lane
            .saturating_add(1);
        let loop_region = self.project.automation_lanes[automation_index]
            .lane
            .loop_region();
        let point_count = self.project.automation_lanes[automation_index]
            .lane
            .points()
            .len();
        if let Some(loop_region) = loop_region {
            for point_index in 0..point_count {
                let point = self.project.automation_lanes[automation_index]
                    .lane
                    .points()[point_index];
                if point.position >= loop_region.start {
                    continue;
                }
                push_bounded_beat(
                    &mut source_beats,
                    point.position,
                    source_start,
                    source_end,
                    limit,
                    automation_id,
                )?;
            }
            if source_end > loop_region.start {
                let first_cycle = if source_start <= loop_region.start {
                    0_u64
                } else {
                    ((source_start - loop_region.start) / loop_region.length())
                        .floor()
                        .max(0.0) as u64
                };
                let mut cycle = first_cycle;
                loop {
                    let cycle_start = loop_region.start + cycle as f64 * loop_region.length();
                    if cycle_start >= source_end {
                        break;
                    }
                    self.charge_work(1)?;
                    push_bounded_beat(
                        &mut source_beats,
                        cycle_start,
                        source_start,
                        source_end,
                        limit,
                        automation_id,
                    )?;
                    for point_index in 0..point_count {
                        let point = self.project.automation_lanes[automation_index]
                            .lane
                            .points()[point_index];
                        if !(loop_region.start..loop_region.end).contains(&point.position) {
                            continue;
                        }
                        let repeated = cycle_start + point.position - loop_region.start;
                        push_bounded_beat(
                            &mut source_beats,
                            repeated,
                            source_start,
                            source_end,
                            limit,
                            automation_id,
                        )?;
                    }
                    cycle = cycle.checked_add(1).ok_or(
                        TimelineCompileError::AutomationSegmentLimitExceeded {
                            automation_id,
                            maximum: self.options.max_automation_segments_per_lane,
                        },
                    )?;
                }
            }
        } else {
            for point_index in 0..point_count {
                let point = self.project.automation_lanes[automation_index]
                    .lane
                    .points()[point_index];
                push_bounded_beat(
                    &mut source_beats,
                    point.position,
                    source_start,
                    source_end,
                    limit,
                    automation_id,
                )?;
            }
        }
        source_beats.sort_by(f64::total_cmp);
        source_beats.dedup_by(|left, right| *left == *right);
        let mut frames = Vec::with_capacity(source_beats.len());
        for source_beat in source_beats {
            let project_beat = placement.project_beat(source_beat);
            if let Some(frame) =
                self.frame_at(project_beat, TimelineEntity::Automation(automation_id))
            {
                if frames.last().copied() == Some(frame) {
                    self.report(
                        TimelineEntity::Automation(automation_id),
                        TimelineDiagnosticKind::AutomationRangeCollapsed,
                    );
                } else {
                    frames.push(frame);
                }
            }
        }
        Ok(frames)
    }

    fn clip_range(&mut self, clip: &ClipView) -> Option<(f64, f64)> {
        let start = f64::from(clip.start);
        let requested_end = start + f64::from(clip.length);
        if !start.is_finite()
            || !requested_end.is_finite()
            || start < 0.0
            || !clip.length.is_finite()
            || clip.length <= 0.0
        {
            self.report(
                TimelineEntity::Clip(clip.id),
                TimelineDiagnosticKind::InvalidClipRange {
                    start: clip.start,
                    length: clip.length,
                },
            );
            self.stats.invalid_items_skipped += 1;
            return None;
        }
        if start >= self.tempo_map.max_beat() || requested_end <= 0.0 {
            self.report(
                TimelineEntity::Clip(clip.id),
                TimelineDiagnosticKind::ClipOutsideTempoMap,
            );
            self.stats.invalid_items_skipped += 1;
            return None;
        }
        let end = requested_end.min(self.tempo_map.max_beat());
        if requested_end > self.tempo_map.max_beat() {
            self.report(
                TimelineEntity::Clip(clip.id),
                TimelineDiagnosticKind::ClipEndTruncated {
                    requested_end_beat: requested_end,
                    map_end_beat: self.tempo_map.max_beat(),
                },
            );
        }
        (end > start).then_some((start, end))
    }

    fn frame_at(&mut self, beat: f64, entity: TimelineEntity) -> Option<u64> {
        match self.tempo_map.beat_to_frame(beat) {
            Ok(frame) => Some(frame),
            Err(_) => {
                self.report(
                    entity,
                    TimelineDiagnosticKind::TempoConversionFailed { beat },
                );
                self.stats.invalid_items_skipped += 1;
                None
            }
        }
    }

    fn report(&mut self, entity: TimelineEntity, kind: TimelineDiagnosticKind) {
        if self.diagnostics.len() < self.options.max_diagnostics {
            self.diagnostics.push(TimelineDiagnostic { entity, kind });
        } else {
            self.stats.diagnostics_suppressed += 1;
        }
    }

    fn push_event(
        &mut self,
        frame: u64,
        priority: u8,
        primary: u64,
        secondary: u64,
        kind: TimelineEventKind,
    ) -> Result<(), TimelineCompileError> {
        self.charge_work(1)?;
        if self.pending.len() >= self.options.max_events {
            return Err(TimelineCompileError::EventLimitExceeded {
                maximum: self.options.max_events,
            });
        }
        let ordinal = self.next_ordinal;
        self.next_ordinal = self.next_ordinal.wrapping_add(1);
        self.pending.push(PendingEvent {
            event: TimelineEvent { frame, kind },
            key: EventSortKey {
                frame,
                priority,
                primary,
                secondary,
                ordinal,
            },
        });
        Ok(())
    }
}

fn validate_options(options: &TimelineCompileOptions) -> Result<(), TimelineCompileError> {
    if !options.step_gate_beats.is_finite() || options.step_gate_beats <= 0.0 {
        return Err(TimelineCompileError::InvalidStepGate);
    }
    if !options.legacy_piano_period_beats.is_finite() || options.legacy_piano_period_beats <= 0.0 {
        return Err(TimelineCompileError::InvalidLegacyPianoPeriod);
    }
    if options.max_mixer_tracks == 0
        || options.max_mixer_tracks > usize::from(u16::MAX) + 1
        || options.max_events == 0
        || options.max_diagnostics == 0
        || options.max_work_units == 0
    {
        return Err(TimelineCompileError::InvalidCapacity);
    }
    if options.max_automation_segment_frames == 0 || options.max_automation_segments_per_lane == 0 {
        return Err(TimelineCompileError::InvalidAutomationLimit);
    }
    for (resource, requested, maximum) in [
        (
            TimelineResource::Events,
            options.max_events,
            MAX_TIMELINE_EVENTS,
        ),
        (
            TimelineResource::Diagnostics,
            options.max_diagnostics,
            MAX_TIMELINE_DIAGNOSTICS,
        ),
        (
            TimelineResource::AutomationSegments,
            options.max_automation_segments_per_lane,
            MAX_TIMELINE_AUTOMATION_SEGMENTS_PER_LANE,
        ),
        (
            TimelineResource::AutomationSegmentFrames,
            options.max_automation_segment_frames as usize,
            MAX_AUTOMATION_SEGMENT_FRAMES as usize,
        ),
        (
            TimelineResource::WorkUnits,
            options.max_work_units,
            MAX_TIMELINE_WORK_UNITS,
        ),
        (
            TimelineResource::StepNoteOverrides,
            options.channel_step_notes.len(),
            MAX_STEP_NOTE_OVERRIDES,
        ),
    ] {
        if requested > maximum {
            return Err(TimelineCompileError::RequestedLimitExceedsMaximum {
                resource,
                requested,
                maximum,
            });
        }
    }
    let mut ids = BTreeSet::new();
    for mapping in &options.channel_step_notes {
        if mapping.note > 127 {
            return Err(TimelineCompileError::InvalidStepNoteOverride {
                channel_id: mapping.channel_id,
                note: mapping.note,
            });
        }
        if !ids.insert(mapping.channel_id) {
            return Err(TimelineCompileError::DuplicateStepNoteOverride {
                channel_id: mapping.channel_id,
            });
        }
    }
    Ok(())
}

fn validate_project_limits(project: &Project) -> Result<(), TimelineCompileError> {
    for (resource, actual, maximum) in [
        (
            TimelineResource::Channels,
            project.channels.len(),
            MAX_TIMELINE_CHANNELS,
        ),
        (
            TimelineResource::Patterns,
            project.patterns.len(),
            MAX_TIMELINE_PATTERNS,
        ),
        (
            TimelineResource::Clips,
            project.clips.len(),
            MAX_TIMELINE_CLIPS,
        ),
        (
            TimelineResource::AutomationLanes,
            project.automation_lanes.len(),
            MAX_TIMELINE_AUTOMATION_LANES,
        ),
        (
            TimelineResource::AudioAssets,
            project.audio_assets.len(),
            MAX_TIMELINE_AUDIO_ASSETS,
        ),
        (
            TimelineResource::PluginInstances,
            project.plugin_instances.len(),
            MAX_TIMELINE_PLUGIN_INSTANCES,
        ),
    ] {
        if actual > maximum {
            return Err(TimelineCompileError::ProjectLimitExceeded {
                resource,
                actual,
                maximum,
            });
        }
    }
    Ok(())
}

fn valid_gain(gain: f32) -> bool {
    gain.is_finite() && (0.0..=1.5).contains(&gain)
}

fn valid_audio_asset(asset: &AudioAsset) -> bool {
    asset.id != 0 && asset.sample_rate != 0 && asset.channels != 0 && asset.frames != 0
}

fn automation_target_sort_key(target: CompiledAutomationTarget) -> u64 {
    match target {
        CompiledAutomationTarget::MasterVolume => 0,
        CompiledAutomationTarget::MasterPan => 1_u64 << 60,
        CompiledAutomationTarget::Tempo => 2_u64 << 60,
        CompiledAutomationTarget::Swing => 3_u64 << 60,
        CompiledAutomationTarget::MixerVolume { track } => (4_u64 << 60) | u64::from(track),
        CompiledAutomationTarget::MixerPan { track } => (5_u64 << 60) | u64::from(track),
        CompiledAutomationTarget::MixerMute { track } => (6_u64 << 60) | u64::from(track),
        CompiledAutomationTarget::ChannelVolume { channel_id } => {
            (7_u64 << 60) | u64::from(channel_id)
        }
        CompiledAutomationTarget::ChannelPan { channel_id } => {
            (8_u64 << 60) | u64::from(channel_id)
        }
        CompiledAutomationTarget::ChannelMute { channel_id } => {
            (9_u64 << 60) | u64::from(channel_id)
        }
        CompiledAutomationTarget::PluginParameter {
            instance_id,
            parameter_id,
        } => {
            (10_u64 << 60)
                | instance_id.rotate_left(17) & ((1_u64 << 60) - 1)
                | u64::from(parameter_id)
        }
    }
}

fn scale_frames(frames: u64, output_rate: u32, source_rate: u32) -> u64 {
    let numerator = u128::from(frames).saturating_mul(u128::from(output_rate));
    let scaled = numerator.div_ceil(u128::from(source_rate));
    u64::try_from(scaled).unwrap_or(u64::MAX)
}

fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() { value } else { fallback }
}

fn previous_f64(value: f64) -> f64 {
    let bits = value.to_bits();
    if value == 0.0 {
        -f64::from_bits(1)
    } else {
        f64::from_bits(if value > 0.0 { bits - 1 } else { bits + 1 })
    }
}

fn automation_ramp_value(start_frame: u64, frame: u64, ramp: AutomationRampDescriptor) -> f32 {
    if ramp.shape == AutomationRampShape::Hold || ramp.end_frame <= start_frame {
        return ramp.start_value;
    }
    let elapsed = frame
        .saturating_sub(start_frame)
        .min(ramp.end_frame - start_frame);
    let progress = elapsed as f64 / (ramp.end_frame - start_frame) as f64;
    ramp.start_value + (ramp.end_value - ramp.start_value) * progress as f32
}

fn maximum_same_frame_burst(pending: &[PendingEvent]) -> usize {
    let mut maximum = 0_usize;
    let mut run = 0_usize;
    let mut previous = None;
    for event in pending {
        if previous == Some(event.event.frame) {
            run += 1;
        } else {
            previous = Some(event.event.frame);
            run = 1;
        }
        maximum = maximum.max(run);
    }
    maximum
}

fn push_bounded_beat(
    beats: &mut Vec<f64>,
    beat: f64,
    range_start: f64,
    range_end: f64,
    limit: usize,
    automation_id: u64,
) -> Result<(), TimelineCompileError> {
    if !(range_start..range_end).contains(&beat) {
        return Ok(());
    }
    if beats.len() >= limit {
        return Err(TimelineCompileError::AutomationSegmentLimitExceeded {
            automation_id,
            maximum: limit.saturating_sub(1),
        });
    }
    beats.push(beat);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        automation::{AutomationLane, AutomationLoop, AutomationPoint, AutomationTarget},
        model::{
            AudioAsset, Channel, MixerInsertSlotRef, Pattern, PianoNote, PluginFormat,
            PluginInstance, PluginRole, PluginRuntimeStatus, ProjectAutomation,
        },
    };
    use std::path::PathBuf;

    fn channel(id: u32, solo: bool, muted: bool, mixer_track: usize) -> Channel {
        Channel {
            id,
            name: format!("Channel {id}"),
            color: [1, 2, 3],
            volume: 0.8,
            pan: 0.0,
            muted,
            solo,
            mixer_track: crate::model::mixer_track_id_for_runtime_slot(mixer_track.min(31) as u8),
            instrument_plugin_instance_id: None,
            steps: [false; 16],
        }
    }

    fn pattern(id: u32, channel_steps: Vec<[bool; 16]>) -> Pattern {
        Pattern {
            id,
            name: format!("Pattern {id}"),
            length_steps: 16,
            channel_steps,
            notes: Vec::new(),
        }
    }

    fn pattern_clip(id: u32, start: f32, length: f32, pattern_id: u32) -> Clip {
        Clip {
            id,
            track: 0,
            start,
            length,
            name: format!("Clip {id}"),
            color: [3, 2, 1],
            kind: ClipKind::Pattern,
            group_id: None,
            pattern_id,
            automation_id: None,
            audio_asset_id: None,
            source_offset: 0.0,
            audio_source_offset_frame: Some(0),
            gain: 1.0,
            fade_in: 0.0,
            fade_out: 0.0,
            muted: false,
        }
    }

    fn empty_project(length: f32) -> Project {
        let mut project = Project {
            format_version: 2,
            name: "Timeline test".into(),
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
        };
        for track in &mut project.mixer_tracks {
            track.volume = 1.0;
        }
        project
    }

    fn note_on_frames(timeline: &CompiledTimeline) -> Vec<u64> {
        timeline
            .events()
            .iter()
            .filter_map(|event| {
                matches!(event.kind, TimelineEventKind::NoteOn { .. }).then_some(event.frame)
            })
            .collect()
    }

    fn automation_lane(id: u64, target: AutomationTarget, value: f64) -> ProjectAutomation {
        let mut lane = AutomationLane::new(target);
        lane.replace_points([AutomationPoint::new(0.0, value)]);
        ProjectAutomation {
            id,
            name: format!("Automation {id}"),
            lane,
        }
    }

    fn plugin(instance_id: u64) -> PluginInstance {
        PluginInstance {
            id: instance_id,
            format: PluginFormat::Vst3,
            role: PluginRole::Unknown,
            path: PathBuf::from(format!(r"C:\VST3\Plugin-{instance_id}.vst3")),
            uid: format!("plugin-{instance_id}"),
            vendor: "Citrus".into(),
            name: format!("Plugin {instance_id}"),
            enabled: true,
            bypass: false,
            wet: 1.0,
            parameters: BTreeMap::from([(7, 0.25)]),
            opaque_state: Vec::new(),
            runtime_status: PluginRuntimeStatus::Unloaded,
        }
    }

    #[test]
    fn plugin_routes_compile_generators_and_are_stable_by_instance_id() {
        let map = TempoMap::new(120.0, None, 1.0, 48_000).unwrap();
        let mut project = empty_project(1.0);
        project.plugin_instances.extend([plugin(30), plugin(10)]);
        let mut first = channel(7, false, false, 2);
        first.instrument_plugin_instance_id = Some(30);
        let mut second = channel(4, false, false, 1);
        second.instrument_plugin_instance_id = Some(10);
        project.channels.extend([first, second]);

        let timeline =
            CompiledTimeline::from_project(&project, &map, TimelineCompileOptions::default())
                .unwrap();
        assert_eq!(
            timeline.plugin_routes(),
            [
                CompiledPluginRoute {
                    instance_id: 10,
                    destination: PluginRouteDestination::Generator {
                        channel_id: 4,
                        slot: 0,
                    },
                },
                CompiledPluginRoute {
                    instance_id: 30,
                    destination: PluginRouteDestination::Generator {
                        channel_id: 7,
                        slot: 0,
                    },
                },
            ]
        );
        assert_eq!(timeline.clone().plugin_routes(), timeline.plugin_routes());
    }

    #[test]
    fn mixer_plugin_routes_use_dense_ordered_runtime_slots_and_follow_moves() {
        let map = TempoMap::new(120.0, None, 1.0, 48_000).unwrap();
        let mut project = empty_project(1.0);
        project.plugin_instances.extend([plugin(10), plugin(20)]);
        project.mixer_insert_slots.extend([
            MixerInsertSlotRef {
                track: 2,
                slot: 8,
                plugin_instance_id: 20,
            },
            MixerInsertSlotRef {
                track: 2,
                slot: 3,
                plugin_instance_id: 10,
            },
        ]);

        let timeline =
            CompiledTimeline::from_project(&project, &map, TimelineCompileOptions::default())
                .unwrap();
        assert_eq!(
            timeline.plugin_routes(),
            [
                CompiledPluginRoute {
                    instance_id: 10,
                    destination: PluginRouteDestination::MixerInsert { track: 2, slot: 0 },
                },
                CompiledPluginRoute {
                    instance_id: 20,
                    destination: PluginRouteDestination::MixerInsert { track: 2, slot: 1 },
                },
            ]
        );

        project.mixer_insert_slots[0].slot = 1;
        let moved =
            CompiledTimeline::from_project(&project, &map, TimelineCompileOptions::default())
                .unwrap();
        assert_eq!(
            moved.plugin_routes(),
            [
                CompiledPluginRoute {
                    instance_id: 10,
                    destination: PluginRouteDestination::MixerInsert { track: 2, slot: 1 },
                },
                CompiledPluginRoute {
                    instance_id: 20,
                    destination: PluginRouteDestination::MixerInsert { track: 2, slot: 0 },
                },
            ]
        );
    }

    #[test]
    fn missing_and_ambiguous_plugin_references_never_produce_routes() {
        let map = TempoMap::new(120.0, None, 1.0, 48_000).unwrap();
        let mut project = empty_project(1.0);
        project.plugin_instances.push(plugin(10));
        let mut generator = channel(1, false, false, 1);
        generator.instrument_plugin_instance_id = Some(10);
        project.channels.push(generator);
        project.mixer_insert_slots.extend([
            MixerInsertSlotRef {
                track: crate::mixer_graph::MASTER_MIXER_TRACK_ID,
                slot: 4,
                plugin_instance_id: 10,
            },
            MixerInsertSlotRef {
                track: crate::mixer_graph::MASTER_MIXER_TRACK_ID,
                slot: 5,
                plugin_instance_id: 999,
            },
        ]);

        let timeline =
            CompiledTimeline::from_project(&project, &map, TimelineCompileOptions::default())
                .unwrap();
        assert!(timeline.plugin_routes().is_empty());
        assert!(timeline.diagnostics().iter().any(|diagnostic| {
            diagnostic.kind
                == TimelineDiagnosticKind::AmbiguousPluginRoute {
                    instance_id: 10,
                    references: 2,
                }
        }));
        assert!(timeline.diagnostics().iter().any(|diagnostic| {
            diagnostic.kind == TimelineDiagnosticKind::MissingPluginInstance { instance_id: 999 }
        }));
    }

    #[test]
    fn invalid_earlier_insert_positions_do_not_leave_runtime_slot_holes() {
        let map = TempoMap::new(120.0, None, 1.0, 48_000).unwrap();
        let mut project = empty_project(1.0);
        project.plugin_instances.extend([plugin(10), plugin(20)]);
        let mut ambiguous_generator = channel(1, false, false, 1);
        ambiguous_generator.instrument_plugin_instance_id = Some(10);
        project.channels.push(ambiguous_generator);
        project.mixer_insert_slots.extend([
            MixerInsertSlotRef {
                track: 2,
                slot: 1,
                plugin_instance_id: 10,
            },
            MixerInsertSlotRef {
                track: 2,
                slot: 8,
                plugin_instance_id: 20,
            },
        ]);

        let timeline =
            CompiledTimeline::from_project(&project, &map, TimelineCompileOptions::default())
                .unwrap();
        assert_eq!(
            timeline.plugin_routes(),
            [CompiledPluginRoute {
                instance_id: 20,
                destination: PluginRouteDestination::MixerInsert { track: 2, slot: 0 },
            }]
        );
    }

    #[test]
    fn plugin_routes_reject_out_of_range_runtime_positions_and_duplicate_insert_references() {
        let map = TempoMap::new(120.0, None, 1.0, 48_000).unwrap();
        let mut project = empty_project(1.0);
        project
            .plugin_instances
            .extend([plugin(10), plugin(20), plugin(30)]);
        project.mixer_insert_slots.extend([
            MixerInsertSlotRef {
                track: 31,
                slot: 9,
                plugin_instance_id: 10,
            },
            MixerInsertSlotRef {
                track: 32,
                slot: 0,
                plugin_instance_id: 20,
            },
            MixerInsertSlotRef {
                track: crate::mixer_graph::MASTER_MIXER_TRACK_ID,
                slot: 10,
                plugin_instance_id: 30,
            },
            MixerInsertSlotRef {
                track: 1,
                slot: 0,
                plugin_instance_id: 10,
            },
        ]);

        let timeline =
            CompiledTimeline::from_project(&project, &map, TimelineCompileOptions::default())
                .unwrap();
        assert!(timeline.plugin_routes().is_empty());
        assert!(timeline.diagnostics().iter().any(|diagnostic| {
            diagnostic.kind
                == TimelineDiagnosticKind::AmbiguousPluginRoute {
                    instance_id: 10,
                    references: 2,
                }
        }));
        assert!(timeline.diagnostics().iter().any(|diagnostic| {
            diagnostic.kind
                == TimelineDiagnosticKind::InvalidPluginMixerRoute {
                    instance_id: 20,
                    track: 32,
                    slot: 0,
                }
        }));
        assert!(timeline.diagnostics().iter().any(|diagnostic| {
            diagnostic.kind
                == TimelineDiagnosticKind::InvalidPluginMixerRoute {
                    instance_id: 30,
                    track: 0,
                    slot: 10,
                }
        }));
    }

    #[test]
    fn plugin_parameter_automation_requires_one_compiled_route() {
        let map = TempoMap::new(120.0, None, 1.0, 48_000).unwrap();
        let mut project = empty_project(1.0);
        project.plugin_instances.extend([plugin(10), plugin(20)]);
        let mut generator = channel(1, false, false, 1);
        generator.instrument_plugin_instance_id = Some(10);
        project.channels.push(generator);
        project.automation_lanes.extend([
            automation_lane(
                1,
                AutomationTarget::PluginParameter {
                    instance: 10,
                    parameter: 7,
                },
                0.75,
            ),
            automation_lane(
                2,
                AutomationTarget::PluginParameter {
                    instance: 20,
                    parameter: 7,
                },
                0.5,
            ),
        ]);

        let timeline =
            CompiledTimeline::from_project(&project, &map, TimelineCompileOptions::default())
                .unwrap();
        let automated_instances = timeline
            .events()
            .iter()
            .filter_map(|event| match event.kind {
                TimelineEventKind::AutomationRamp(AutomationRampDescriptor {
                    target: CompiledAutomationTarget::PluginParameter { instance_id, .. },
                    ..
                }) => Some(instance_id),
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(automated_instances, BTreeSet::from([10]));
        assert!(timeline.diagnostics().iter().any(|diagnostic| {
            diagnostic.entity == TimelineEntity::Automation(2)
                && diagnostic.kind
                    == TimelineDiagnosticKind::PluginAutomationRouteUnavailable { instance_id: 20 }
        }));
        assert!(timeline.automation_bases().iter().all(|base| {
            !matches!(
                base.target,
                CompiledAutomationTarget::PluginParameter {
                    instance_id: 20,
                    ..
                }
            )
        }));
    }

    #[test]
    fn duplicate_overlapping_audio_clip_ids_emit_no_voice_events() {
        let map = TempoMap::new(120.0, None, 4.0, 48_000).unwrap();
        let mut project = empty_project(4.0);
        project.audio_assets.push(AudioAsset {
            id: 77,
            name: "Shared source".into(),
            path: PathBuf::from("offline.wav"),
            sample_rate: 48_000,
            channels: 2,
            bits_per_sample: 24,
            frames: 192_000,
            waveform_peaks: Vec::new(),
        });
        let mut first = pattern_clip(42, 0.0, 3.0, 0);
        first.kind = ClipKind::Audio;
        first.audio_asset_id = Some(77);
        let mut second = pattern_clip(42, 1.0, 3.0, 0);
        second.kind = ClipKind::Audio;
        second.audio_asset_id = Some(77);
        project.clips.extend([first, second]);

        let timeline =
            CompiledTimeline::from_project(&project, &map, TimelineCompileOptions::default())
                .unwrap();

        assert!(timeline.audio_clips().is_empty());
        assert!(timeline.events().iter().all(|event| !matches!(
            event.kind,
            TimelineEventKind::AudioStart(_) | TimelineEventKind::AudioStop { .. }
        )));
        assert_eq!(
            timeline.diagnostics(),
            &[TimelineDiagnostic {
                entity: TimelineEntity::Clip(42),
                kind: TimelineDiagnosticKind::DuplicateClipId { clip_id: 42 },
            }]
        );
        assert_eq!(timeline.stats().invalid_items_skipped, 2);
    }

    #[test]
    fn duplicate_pattern_and_automation_ids_compile_no_ambiguous_sources() {
        let map = TempoMap::new(120.0, None, 2.0, 48_000).unwrap();
        let mut project = empty_project(2.0);
        project
            .patterns
            .extend([pattern(91, Vec::new()), pattern(91, Vec::new())]);
        project.clips.push(pattern_clip(8, 0.0, 1.0, 91));
        project.automation_lanes.extend([
            automation_lane(700, AutomationTarget::MasterVolume, 0.25),
            automation_lane(700, AutomationTarget::MasterPan, 0.75),
        ]);

        let timeline =
            CompiledTimeline::from_project(&project, &map, TimelineCompileOptions::default())
                .unwrap();

        assert!(timeline.events().is_empty());
        assert_eq!(
            timeline.diagnostics(),
            &[
                TimelineDiagnostic {
                    entity: TimelineEntity::Pattern(91),
                    kind: TimelineDiagnosticKind::DuplicatePatternId { pattern_id: 91 },
                },
                TimelineDiagnostic {
                    entity: TimelineEntity::Automation(700),
                    kind: TimelineDiagnosticKind::DuplicateAutomationId { automation_id: 700 },
                },
                TimelineDiagnostic {
                    entity: TimelineEntity::Clip(8),
                    kind: TimelineDiagnosticKind::MissingPattern { pattern_id: 91 },
                },
            ]
        );
    }

    #[test]
    fn duplicate_automation_placement_clip_ids_do_not_fall_back_to_global_automation() {
        let map = TempoMap::new(120.0, None, 2.0, 48_000).unwrap();
        let mut project = empty_project(2.0);
        project
            .automation_lanes
            .push(automation_lane(501, AutomationTarget::MasterVolume, 0.4));
        let mut first = pattern_clip(33, 0.0, 1.5, 0);
        first.kind = ClipKind::Automation;
        first.automation_id = Some(501);
        let mut second = pattern_clip(33, 0.5, 1.5, 0);
        second.kind = ClipKind::Automation;
        second.automation_id = Some(501);
        project.clips.extend([first, second]);

        let timeline =
            CompiledTimeline::from_project(&project, &map, TimelineCompileOptions::default())
                .unwrap();

        assert!(timeline.events().iter().all(|event| !matches!(
            event.kind,
            TimelineEventKind::AutomationRamp(_) | TimelineEventKind::AutomationEnd { .. }
        )));
        assert_eq!(
            timeline.diagnostics(),
            &[TimelineDiagnostic {
                entity: TimelineEntity::Clip(33),
                kind: TimelineDiagnosticKind::DuplicateClipId { clip_id: 33 },
            }]
        );
    }

    #[test]
    fn persistent_identity_diagnostics_have_a_stable_sorted_order() {
        let map = TempoMap::new(120.0, None, 1.0, 48_000).unwrap();
        let mut project = empty_project(1.0);
        project.patterns.extend([
            pattern(9, Vec::new()),
            pattern(3, Vec::new()),
            pattern(9, Vec::new()),
            pattern(3, Vec::new()),
        ]);
        project.clips.extend([
            pattern_clip(20, 0.0, 1.0, 0),
            pattern_clip(0, 0.0, 1.0, 0),
            pattern_clip(5, 0.0, 1.0, 0),
            pattern_clip(20, 0.0, 1.0, 0),
            pattern_clip(5, 0.0, 1.0, 0),
        ]);
        project.automation_lanes.extend([
            automation_lane(11, AutomationTarget::MasterVolume, 0.1),
            automation_lane(0, AutomationTarget::MasterVolume, 0.2),
            automation_lane(7, AutomationTarget::MasterVolume, 0.3),
            automation_lane(11, AutomationTarget::MasterVolume, 0.4),
            automation_lane(7, AutomationTarget::MasterVolume, 0.5),
        ]);

        let first =
            CompiledTimeline::from_project(&project, &map, TimelineCompileOptions::default())
                .unwrap();
        let second =
            CompiledTimeline::from_project(&project, &map, TimelineCompileOptions::default())
                .unwrap();

        let expected = [
            TimelineDiagnostic {
                entity: TimelineEntity::Pattern(3),
                kind: TimelineDiagnosticKind::DuplicatePatternId { pattern_id: 3 },
            },
            TimelineDiagnostic {
                entity: TimelineEntity::Pattern(9),
                kind: TimelineDiagnosticKind::DuplicatePatternId { pattern_id: 9 },
            },
            TimelineDiagnostic {
                entity: TimelineEntity::Clip(0),
                kind: TimelineDiagnosticKind::InvalidClipId,
            },
            TimelineDiagnostic {
                entity: TimelineEntity::Clip(5),
                kind: TimelineDiagnosticKind::DuplicateClipId { clip_id: 5 },
            },
            TimelineDiagnostic {
                entity: TimelineEntity::Clip(20),
                kind: TimelineDiagnosticKind::DuplicateClipId { clip_id: 20 },
            },
            TimelineDiagnostic {
                entity: TimelineEntity::Automation(0),
                kind: TimelineDiagnosticKind::InvalidAutomationId,
            },
            TimelineDiagnostic {
                entity: TimelineEntity::Automation(7),
                kind: TimelineDiagnosticKind::DuplicateAutomationId { automation_id: 7 },
            },
            TimelineDiagnostic {
                entity: TimelineEntity::Automation(11),
                kind: TimelineDiagnosticKind::DuplicateAutomationId { automation_id: 11 },
            },
        ];
        assert_eq!(first.diagnostics(), expected);
        assert_eq!(second.diagnostics(), expected);
        assert!(first.events().is_empty());
    }

    #[test]
    fn tempo_ramp_frames_always_come_from_the_tempo_map() {
        let mut tempo_lane = AutomationLane::new(AutomationTarget::Tempo);
        tempo_lane.replace_points([
            AutomationPoint::new(0.0, 120.0),
            AutomationPoint::new(4.0, 240.0),
        ]);
        let map = TempoMap::new(120.0, Some(tempo_lane), 8.0, 48_000).unwrap();
        let mut project = empty_project(8.0);
        project.channels.push(channel(1, false, false, 1));
        let mut steps = [false; 16];
        steps[0] = true;
        project.patterns.push(pattern(7, vec![steps]));
        project.clips.push(pattern_clip(9, 4.0, 1.0, 7));

        let timeline =
            CompiledTimeline::from_project(&project, &map, TimelineCompileOptions::default())
                .unwrap();
        let expected = map.beat_to_frame(4.0).unwrap();
        assert_eq!(note_on_frames(&timeline), vec![expected]);
        assert_ne!(expected, 96_000);
    }

    #[test]
    fn swing_delays_only_the_offbeat_step() {
        let map = TempoMap::new(120.0, None, 4.0, 48_000).unwrap();
        let mut project = empty_project(4.0);
        project.swing = 1.0;
        project.channels.push(channel(1, false, false, 1));
        let mut steps = [false; 16];
        steps[0] = true;
        steps[1] = true;
        project.patterns.push(pattern(1, vec![steps]));
        project.clips.push(pattern_clip(1, 0.0, 0.5, 1));

        let timeline =
            CompiledTimeline::from_project(&project, &map, TimelineCompileOptions::default())
                .unwrap();
        assert_eq!(
            note_on_frames(&timeline),
            vec![
                map.beat_to_frame(0.0).unwrap(),
                map.beat_to_frame(0.4375).unwrap()
            ]
        );
    }

    #[test]
    fn swing_automation_uses_placement_source_and_pattern_local_phase() {
        let map = TempoMap::new(120.0, None, 4.0, 48_000).unwrap();
        let mut project = empty_project(4.0);
        project.channels.push(channel(1, false, false, 1));
        let mut steps = [false; 16];
        steps[1] = true;
        let mut odd_pattern = pattern(1, vec![steps]);
        odd_pattern.length_steps = 3;
        project.patterns.push(odd_pattern);
        project.clips.push(pattern_clip(1, 0.125, 1.5, 1));

        let mut swing = AutomationLane::new(AutomationTarget::Swing);
        swing.replace_points([
            AutomationPoint::new(0.0, 0.0),
            AutomationPoint::new(1.0, 1.0),
        ]);
        project.automation_lanes.push(ProjectAutomation {
            id: 90,
            name: "Placed swing".into(),
            lane: swing,
        });
        let mut placement = pattern_clip(2, 0.125, 1.5, 0);
        placement.kind = ClipKind::Automation;
        placement.automation_id = Some(90);
        placement.source_offset = 0.5;
        project.clips.push(placement);

        let timeline =
            CompiledTimeline::from_project(&project, &map, TimelineCompileOptions::default())
                .unwrap();
        assert_eq!(
            note_on_frames(&timeline),
            vec![
                map.beat_to_frame(0.46875).unwrap(),
                map.beat_to_frame(1.3125).unwrap(),
            ]
        );
    }

    #[test]
    fn pattern_repeats_and_note_tails_stop_at_the_clip_end() {
        let map = TempoMap::new(120.0, None, 4.0, 48_000).unwrap();
        let mut project = empty_project(4.0);
        project.channels.push(channel(1, false, false, 1));
        let mut steps = [false; 16];
        steps[0] = true;
        let mut repeating = pattern(1, vec![steps]);
        repeating.length_steps = 4;
        project.patterns.push(repeating);
        project.clips.push(pattern_clip(1, 0.0, 2.1, 1));
        let options = TimelineCompileOptions {
            step_gate_beats: 0.5,
            ..TimelineCompileOptions::default()
        };

        let timeline = CompiledTimeline::from_project(&project, &map, options).unwrap();
        assert_eq!(
            note_on_frames(&timeline),
            vec![
                map.beat_to_frame(0.0).unwrap(),
                map.beat_to_frame(1.0).unwrap(),
                map.beat_to_frame(2.0).unwrap(),
            ]
        );
        let final_off = timeline
            .events()
            .iter()
            .filter(|event| matches!(event.kind, TimelineEventKind::NoteOff { .. }))
            .map(|event| event.frame)
            .max()
            .unwrap();
        assert_eq!(final_off, map.beat_to_frame(2.1).unwrap());
    }

    #[test]
    fn pattern_source_offset_slips_steps_and_piano_notes_inside_fixed_clip_edges() {
        let map = TempoMap::new(120.0, None, 4.0, 48_000).unwrap();
        let mut project = empty_project(4.0);
        project.channels.push(channel(1, false, false, 1));
        let mut steps = [false; 16];
        steps[0] = true;
        let mut slipped = pattern(1, vec![steps]);
        slipped.length_steps = 4;
        slipped.notes.push(PianoNote {
            id: 1,
            channel_id: Some(1),
            group_id: None,
            note: 64,
            start: 0.5,
            length: 0.1,
            velocity: 0.8,
            selected: false,
            muted: false,
        });
        project.patterns.push(slipped);
        let mut clip = pattern_clip(1, 1.0, 1.5, 1);
        clip.source_offset = 0.25;
        project.clips.push(clip);

        let timeline = CompiledTimeline::from_project(
            &project,
            &map,
            TimelineCompileOptions {
                legacy_piano_period_beats: 1.0,
                ..TimelineCompileOptions::default()
            },
        )
        .unwrap();

        assert_eq!(
            note_on_frames(&timeline),
            vec![
                map.beat_to_frame(1.25).unwrap(),
                map.beat_to_frame(1.75).unwrap(),
                map.beat_to_frame(2.25).unwrap(),
            ]
        );
        assert!(timeline.events().iter().all(|event| {
            event.frame >= map.beat_to_frame(1.0).unwrap()
                && event.frame <= map.beat_to_frame(2.5).unwrap()
        }));
    }

    #[test]
    fn channel_solo_mute_and_volume_remain_in_the_base_plan() {
        let map = TempoMap::new(120.0, None, 4.0, 48_000).unwrap();
        let mut project = empty_project(4.0);
        project.channels.push(channel(1, false, false, 1));
        project.channels.push(channel(2, true, false, 2));
        project.channels.push(channel(3, true, true, 3));
        let mut enabled = [false; 16];
        enabled[0] = true;
        project.patterns.push(pattern(1, vec![enabled; 3]));
        let mut clip = pattern_clip(1, 0.0, 1.0, 1);
        clip.gain = 0.5;
        project.clips.push(clip);

        let timeline =
            CompiledTimeline::from_project(&project, &map, TimelineCompileOptions::default())
                .unwrap();
        let channels = timeline
            .events()
            .iter()
            .filter_map(|event| match event.kind {
                TimelineEventKind::NoteOn { channel_id, .. } => Some(channel_id),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(channels, vec![1, 2, 3]);
        assert!(
            timeline
                .channel_bases()
                .iter()
                .any(|base| { base.channel_id == 3 && base.muted && base.solo })
        );
        assert!(
            timeline
                .channel_bases()
                .iter()
                .all(|base| base.volume == 0.8)
        );
        assert!(timeline.events().iter().all(|event| match event.kind {
            TimelineEventKind::NoteOn { gain, .. } => gain == 0.5,
            _ => true,
        }));
    }

    #[test]
    fn piano_notes_use_persisted_identity_and_channel() {
        let map = TempoMap::new(120.0, None, 4.0, 48_000).unwrap();
        let mut project = empty_project(4.0);
        project.channels.push(channel(10, false, false, 1));
        let mut legacy = pattern(1, vec![[false; 16]]);
        legacy.notes.push(PianoNote {
            id: 77,
            channel_id: Some(10),
            group_id: None,
            note: 64,
            start: 0.0,
            length: 0.5,
            velocity: 0.75,
            selected: false,
            muted: false,
        });
        project.patterns.push(legacy);
        project.clips.push(pattern_clip(1, 0.0, 2.0, 1));

        let routed = CompiledTimeline::from_project(
            &project,
            &map,
            TimelineCompileOptions {
                legacy_piano_period_beats: 1.0,
                ..TimelineCompileOptions::default()
            },
        )
        .unwrap();
        assert_eq!(
            note_on_frames(&routed),
            vec![0, map.beat_to_frame(1.0).unwrap()]
        );
        let occurrences = routed
            .events()
            .iter()
            .filter_map(|event| match event.kind {
                TimelineEventKind::NoteOn {
                    note_id, source, ..
                } => Some((note_id, source)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_ne!(occurrences[0].0, occurrences[1].0);
        assert!(occurrences.iter().all(|(_, source)| matches!(
            source,
            NoteSourceDescriptor::LegacyPiano {
                persistent_note_id: 77,
                ..
            }
        )));
    }

    #[test]
    fn same_frame_note_off_precedes_retrigger_note_on() {
        let map = TempoMap::new(120.0, None, 2.0, 48_000).unwrap();
        let mut project = empty_project(2.0);
        project.channels.push(channel(1, false, false, 1));
        let mut steps = [false; 16];
        steps[0] = true;
        let mut one_step = pattern(1, vec![steps]);
        one_step.length_steps = 1;
        project.patterns.push(one_step);
        project.clips.push(pattern_clip(1, 0.0, 0.5, 1));
        let timeline =
            CompiledTimeline::from_project(&project, &map, TimelineCompileOptions::default())
                .unwrap();
        let boundary = map.beat_to_frame(0.25).unwrap();
        let at_boundary = timeline
            .events()
            .iter()
            .filter(|event| event.frame == boundary)
            .map(|event| event.kind)
            .collect::<Vec<_>>();
        assert!(matches!(at_boundary[0], TimelineEventKind::NoteOff { .. }));
        assert!(matches!(at_boundary[1], TimelineEventKind::NoteOn { .. }));
        assert_eq!(timeline.stats().max_events_at_frame, 2);
    }

    #[test]
    fn packet_boundaries_are_half_open_and_overflow_is_atomic() {
        let map = TempoMap::new(120.0, None, 2.0, 48_000).unwrap();
        let mut project = empty_project(2.0);
        project.channels.push(channel(1, false, false, 1));
        let mut steps = [false; 16];
        steps[0] = true;
        let mut one_step = pattern(1, vec![steps]);
        one_step.length_steps = 1;
        project.patterns.push(one_step);
        project.clips.push(pattern_clip(1, 0.0, 0.5, 1));
        let timeline =
            CompiledTimeline::from_project(&project, &map, TimelineCompileOptions::default())
                .unwrap();

        let boundary = map.beat_to_frame(0.25).unwrap();
        let mut packet = TimelinePacket::<2>::new();
        timeline
            .packetize_into(&mut packet, 7, 0, u32::try_from(boundary).unwrap())
            .unwrap();
        assert_eq!(packet.epoch(), 7);
        assert_eq!(packet.len(), 1);
        assert_eq!(packet.events()[0].sample_offset, 0);

        timeline
            .packetize_into(&mut packet, 8, boundary, 1)
            .unwrap();
        assert_eq!(packet.len(), 2);
        assert!(matches!(
            packet.events()[0].kind,
            TimelineEventKind::NoteOff { .. }
        ));

        let mut too_small = TimelinePacket::<1>::new();
        timeline.packetize_into(&mut too_small, 9, 0, 1).unwrap();
        let before_epoch = too_small.epoch();
        let before_start = too_small.start_frame();
        let before_frames = too_small.frames();
        let before_overflow = too_small.events().to_vec();
        let error = timeline
            .packetize_into(&mut too_small, 10, boundary, 1)
            .unwrap_err();
        assert_eq!(
            error,
            TimelinePacketError::CapacityExceeded {
                capacity: 1,
                required: 2
            }
        );
        assert_eq!(too_small.epoch(), before_epoch);
        assert_eq!(too_small.start_frame(), before_start);
        assert_eq!(too_small.frames(), before_frames);
        assert_eq!(too_small.events(), before_overflow);

        assert_eq!(
            timeline
                .packetize_into(&mut too_small, 11, 0, u32::from(u16::MAX) + 2)
                .unwrap_err(),
            TimelinePacketError::FrameCountTooLarge { frames: 65_537 }
        );
        assert_eq!(
            timeline
                .packetize_into(&mut too_small, 12, u64::MAX, 1)
                .unwrap_err(),
            TimelinePacketError::FrameRangeOverflow
        );
        assert_eq!(too_small.epoch(), before_epoch);
        assert_eq!(too_small.start_frame(), before_start);
        assert_eq!(too_small.frames(), before_frames);
        assert_eq!(too_small.events(), before_overflow);
    }

    #[test]
    fn packet_offsets_cover_u16_max_and_same_frame_bursts_are_chunkable() {
        let note_source = NoteSourceDescriptor::ChannelStep {
            clip_id: 1,
            pattern_id: 1,
            step: 0,
            repetition: 0,
        };
        let event = TimelineEvent {
            frame: u64::from(u16::MAX),
            kind: TimelineEventKind::NoteOff {
                note_id: 1,
                channel_id: 1,
                note: 60,
                mixer_track: 1,
                source: note_source,
            },
        };
        let timeline = CompiledTimeline {
            sample_rate: 48_000,
            duration_frames: 65_536,
            events: vec![event, event],
            audio_clips: Vec::new(),
            automation_bases: Vec::new(),
            driven_automation_targets: Vec::new(),
            channel_bases: Vec::new(),
            plugin_routes: Vec::new(),
            mixer_graph: crate::mixer_graph::compile_mixer_graph(&empty_project(1.0)).unwrap(),
            diagnostics: Vec::new(),
            stats: TimelineCompileStats {
                max_events_at_frame: 2,
                ..TimelineCompileStats::default()
            },
        };

        let mut range = timeline.event_range(55, 0, 65_536).unwrap();
        let mut packet = TimelinePacket::<1>::new();
        let first = range.packetize_next_into(&mut packet).unwrap();
        assert_eq!(first.copied_events, 1);
        assert_eq!(first.remaining_events, 1);
        assert_eq!(packet.events()[0].sample_offset, u16::MAX);
        assert_eq!(
            (packet.epoch(), packet.start_frame(), packet.frames()),
            (55, 0, 65_536)
        );
        let second = range.packetize_next_into(&mut packet).unwrap();
        assert_eq!(second.remaining_events, 0);
        assert_eq!(packet.events()[0].sample_offset, u16::MAX);

        let mut zero = TimelinePacket::<0>::new();
        timeline.packetize_into(&mut zero, 7, 1, 1).unwrap();
        let before = (zero.epoch(), zero.start_frame(), zero.frames(), zero.len());
        let mut range = timeline.event_range(99, 0, 65_536).unwrap();
        assert_eq!(
            range.packetize_next_into(&mut zero).unwrap_err(),
            TimelinePacketError::CapacityExceeded {
                capacity: 0,
                required: 2,
            }
        );
        assert_eq!(
            (zero.epoch(), zero.start_frame(), zero.frames(), zero.len()),
            before
        );
        assert_eq!(range.remaining_events(), 2);
    }

    #[test]
    fn automation_is_split_into_bounded_ramps() {
        let map = TempoMap::new(120.0, None, 2.0, 48_000).unwrap();
        let mut project = empty_project(2.0);
        let mut lane = AutomationLane::new(AutomationTarget::MasterVolume);
        lane.replace_points([
            AutomationPoint::new(0.0, 0.0),
            AutomationPoint::new(2.0, 1.0),
        ]);
        project.automation_lanes.push(ProjectAutomation {
            id: 42,
            name: "Master ramp".into(),
            lane,
        });
        let timeline = CompiledTimeline::from_project(
            &project,
            &map,
            TimelineCompileOptions {
                max_automation_segment_frames: 1_000,
                ..TimelineCompileOptions::default()
            },
        )
        .unwrap();
        let ramps = timeline
            .events()
            .iter()
            .filter_map(|event| match event.kind {
                TimelineEventKind::AutomationRamp(ramp) => Some((event.frame, ramp)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(!ramps.is_empty());
        assert!(
            ramps
                .iter()
                .all(|(start, ramp)| ramp.end_frame - start <= 1_000)
        );
        assert_eq!(timeline.stats().automation_segments, ramps.len());
    }

    #[test]
    fn looped_automation_ramp_ends_at_the_left_limit() {
        let map = TempoMap::new(120.0, None, 2.0, 48_000).unwrap();
        let mut project = empty_project(2.0);
        let mut lane = AutomationLane::new(AutomationTarget::MasterVolume);
        lane.replace_points([
            AutomationPoint::new(0.0, 0.0),
            AutomationPoint::new(1.0, 1.0),
        ]);
        lane.set_loop_region(Some(AutomationLoop::new(0.0, 1.0).unwrap()))
            .unwrap();
        project.automation_lanes.push(ProjectAutomation {
            id: 70,
            name: "Loop".into(),
            lane,
        });

        let timeline = CompiledTimeline::from_project(
            &project,
            &map,
            TimelineCompileOptions {
                max_automation_segment_frames: 65_536,
                ..TimelineCompileOptions::default()
            },
        )
        .unwrap();
        let wrap_frame = map.beat_to_frame(1.0).unwrap();
        let ramps = timeline
            .events()
            .iter()
            .filter_map(|event| match event.kind {
                TimelineEventKind::AutomationRamp(ramp) => Some((event.frame, ramp)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(ramps.len(), 2);
        assert_eq!(ramps[0].1.end_frame, wrap_frame);
        assert!(ramps[0].1.end_value > 0.999_999);
        assert_eq!(ramps[1].0, wrap_frame);
        assert_eq!(ramps[1].1.start_value, 0.0);
    }

    #[test]
    fn automation_placements_keep_independent_layers_and_source_offsets() {
        let map = TempoMap::new(120.0, None, 3.0, 48_000).unwrap();
        let mut project = empty_project(3.0);
        let mut lane = AutomationLane::new(AutomationTarget::MasterVolume);
        lane.replace_points([
            AutomationPoint::new(0.0, 0.0),
            AutomationPoint::new(1.0, 1.0),
            AutomationPoint::new(2.0, 0.0),
        ]);
        project.automation_lanes.push(ProjectAutomation {
            id: 80,
            name: "Placed".into(),
            lane,
        });
        let mut first = pattern_clip(10, 0.0, 2.0, 0);
        first.kind = ClipKind::Automation;
        first.automation_id = Some(80);
        let mut second = pattern_clip(11, 1.0, 1.0, 0);
        second.kind = ClipKind::Automation;
        second.automation_id = Some(80);
        second.source_offset = 0.5;
        project.clips.extend([first, second]);

        let timeline = CompiledTimeline::from_project(
            &project,
            &map,
            TimelineCompileOptions {
                max_automation_segment_frames: 65_536,
                ..TimelineCompileOptions::default()
            },
        )
        .unwrap();
        let overlap_start = map.beat_to_frame(1.0).unwrap();
        let starts = timeline
            .events()
            .iter()
            .filter_map(|event| match event.kind {
                TimelineEventKind::AutomationRamp(ramp) if event.frame == overlap_start => {
                    Some((ramp.placement_id, ramp.precedence, ramp.start_value))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(starts, vec![(Some(10), 0, 1.0), (Some(11), 1, 0.5)]);

        let state = timeline
            .chase_discontinuity(
                map.beat_to_frame(1.25).unwrap(),
                TimelineChaseOptions::default(),
            )
            .unwrap();
        assert_eq!(
            state
                .automation_layers
                .iter()
                .map(|layer| layer.placement_id)
                .collect::<Vec<_>>(),
            vec![Some(10), Some(11)]
        );
    }

    #[test]
    fn automation_precedence_is_project_order_and_matches_tempo_map_winner() {
        let mut project = empty_project(4.0);
        let mut first = AutomationLane::new(AutomationTarget::Tempo);
        first.replace_points([AutomationPoint::new(0.0, 100.0)]);
        let mut second = AutomationLane::new(AutomationTarget::Tempo);
        second.replace_points([AutomationPoint::new(0.0, 200.0)]);
        // Deliberately reverse numeric ids: ids must not decide precedence.
        project.automation_lanes.push(ProjectAutomation {
            id: 999,
            name: "Earlier".into(),
            lane: first,
        });
        project.automation_lanes.push(ProjectAutomation {
            id: 1,
            name: "Later".into(),
            lane: second,
        });
        let map = TempoMap::from_project(&project, 48_000).unwrap();
        let timeline =
            CompiledTimeline::from_project(&project, &map, TimelineCompileOptions::default())
                .unwrap();

        let at_start = timeline
            .events()
            .iter()
            .filter_map(|event| match event.kind {
                TimelineEventKind::AutomationRamp(ramp)
                    if event.frame == 0 && ramp.target == CompiledAutomationTarget::Tempo =>
                {
                    Some((ramp.precedence, ramp.start_value))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(at_start, vec![(0, 100.0), (1, 200.0)]);
        assert_eq!(map.bpm_at(0.0).unwrap(), 200.0);
    }

    #[test]
    fn mixer_zero_aliases_share_master_bases_precedence_and_chase_target() {
        let map = TempoMap::new(120.0, None, 4.0, 48_000).unwrap();
        let mut project = empty_project(4.0);
        project.mixer_tracks[0].volume = 0.42;
        project.mixer_tracks[0].pan = -0.2;
        project.automation_lanes.extend([
            automation_lane(90, AutomationTarget::MasterVolume, 0.25),
            automation_lane(
                10,
                AutomationTarget::MixerVolume {
                    track: crate::mixer_graph::MASTER_MIXER_TRACK_ID,
                },
                0.75,
            ),
        ]);

        let timeline =
            CompiledTimeline::from_project(&project, &map, TimelineCompileOptions::default())
                .unwrap();

        assert_eq!(
            timeline
                .automation_bases()
                .iter()
                .filter(|base| base.target == CompiledAutomationTarget::MasterVolume)
                .count(),
            1
        );
        assert_eq!(
            timeline
                .automation_bases()
                .iter()
                .find(|base| base.target == CompiledAutomationTarget::MasterVolume)
                .map(|base| base.value),
            Some(0.42)
        );
        assert!(timeline.automation_bases().iter().all(|base| {
            !matches!(
                base.target,
                CompiledAutomationTarget::MixerVolume { track: 0 }
                    | CompiledAutomationTarget::MixerPan { track: 0 }
            )
        }));
        assert_eq!(
            timeline
                .automation_bases()
                .iter()
                .filter(|base| base.target == CompiledAutomationTarget::MasterPan)
                .count(),
            1
        );
        assert!(
            timeline
                .automation_bases()
                .iter()
                .any(|base| { base.target == CompiledAutomationTarget::MixerMute { track: 0 } })
        );

        let at_start = timeline
            .events()
            .iter()
            .filter_map(|event| match event.kind {
                TimelineEventKind::AutomationRamp(ramp)
                    if event.frame == 0
                        && ramp.target == CompiledAutomationTarget::MasterVolume =>
                {
                    Some((ramp.precedence, ramp.start_value))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(at_start, vec![(0, 0.25), (1, 0.75)]);

        let chase = timeline
            .chase_discontinuity(
                map.beat_to_frame(1.0).unwrap(),
                TimelineChaseOptions::default(),
            )
            .unwrap();
        let master_layers = chase
            .automation_layers
            .iter()
            .filter(|layer| layer.target == CompiledAutomationTarget::MasterVolume)
            .collect::<Vec<_>>();
        assert_eq!(master_layers.len(), 2);
        assert!(
            master_layers
                .iter()
                .all(|layer| layer.target == CompiledAutomationTarget::MasterVolume)
        );
        let winner = master_layers
            .iter()
            .max_by_key(|layer| layer.precedence)
            .unwrap();
        assert_eq!((winner.precedence, winner.current_value), (1, 0.75));

        // Compilation canonicalizes transient targets; persisted project data
        // remains byte-for-byte expressible with its original alias.
        assert_eq!(
            project.automation_lanes[1].lane.target(),
            &AutomationTarget::MixerVolume {
                track: crate::mixer_graph::MASTER_MIXER_TRACK_ID,
            }
        );
    }

    #[test]
    fn driven_automation_manifest_is_sorted_unique_and_excludes_skipped_lanes() {
        let map = TempoMap::new(120.0, None, 4.0, 48_000).unwrap();
        let mut project = empty_project(4.0);
        project.automation_lanes.extend([
            automation_lane(1, AutomationTarget::MixerPan { track: 2 }, 0.25),
            automation_lane(2, AutomationTarget::MasterVolume, 0.5),
            // This alias compiles to the same transient target as lane 2.
            automation_lane(
                3,
                AutomationTarget::MixerVolume {
                    track: crate::mixer_graph::MASTER_MIXER_TRACK_ID,
                },
                0.75,
            ),
        ]);
        let mut disabled =
            automation_lane(4, AutomationTarget::ChannelVolume { channel: 999 }, 0.5);
        disabled.lane.set_enabled(false);
        project.automation_lanes.push(disabled);
        project.automation_lanes.push(ProjectAutomation {
            id: 5,
            name: "Empty".into(),
            lane: AutomationLane::new(AutomationTarget::MasterPan),
        });

        let timeline =
            CompiledTimeline::from_project(&project, &map, TimelineCompileOptions::default())
                .unwrap();

        assert_eq!(
            timeline.driven_automation_targets(),
            [
                CompiledAutomationTarget::MasterVolume,
                CompiledAutomationTarget::MixerPan { track: 2 },
            ]
        );
        assert_eq!(
            timeline.clone().driven_automation_targets(),
            timeline.driven_automation_targets()
        );
    }

    #[test]
    fn automation_end_removes_only_its_layer_after_lower_lane_updates() {
        let map = TempoMap::new(120.0, None, 4.0, 48_000).unwrap();
        let mut project = empty_project(4.0);
        let mut lower = AutomationLane::new(AutomationTarget::MasterVolume);
        lower.replace_points([
            AutomationPoint::new(0.0, 0.2),
            AutomationPoint::new(2.0, 0.6),
            AutomationPoint::new(4.0, 0.8),
        ]);
        let mut higher = AutomationLane::new(AutomationTarget::MasterVolume);
        higher.replace_points([AutomationPoint::new(0.0, 0.9)]);
        project.automation_lanes.push(ProjectAutomation {
            id: 10,
            name: "Lower".into(),
            lane: lower,
        });
        project.automation_lanes.push(ProjectAutomation {
            id: 20,
            name: "Temporary override".into(),
            lane: higher,
        });
        let mut placement = pattern_clip(50, 1.0, 1.0, 0);
        placement.kind = ClipKind::Automation;
        placement.automation_id = Some(20);
        project.clips.push(placement);

        let timeline =
            CompiledTimeline::from_project(&project, &map, TimelineCompileOptions::default())
                .unwrap();
        let boundary = map.beat_to_frame(2.0).unwrap();
        let target_events = timeline
            .events()
            .iter()
            .filter(|event| event.frame == boundary)
            .filter_map(|event| match event.kind {
                TimelineEventKind::AutomationRamp(ramp)
                    if ramp.target == CompiledAutomationTarget::MasterVolume =>
                {
                    Some((ramp.precedence, false))
                }
                TimelineEventKind::AutomationEnd {
                    precedence,
                    target: CompiledAutomationTarget::MasterVolume,
                    ..
                } => Some((precedence, true)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(target_events, vec![(0, false), (1, true)]);
    }

    #[test]
    fn audio_clip_descriptors_use_metadata_without_reading_the_asset() {
        let map = TempoMap::new(120.0, None, 4.0, 48_000).unwrap();
        let mut project = empty_project(4.0);
        project.audio_assets.push(AudioAsset {
            id: 77,
            name: "Offline asset".into(),
            path: PathBuf::from("this-file-does-not-exist.wav"),
            sample_rate: 48_000,
            channels: 2,
            bits_per_sample: 24,
            frames: 96_000,
            waveform_peaks: Vec::new(),
        });
        let mut clip = pattern_clip(5, 1.0, 1.0, 0);
        clip.kind = ClipKind::Audio;
        clip.audio_asset_id = Some(77);
        clip.audio_source_offset_frame = Some(12_000);
        clip.fade_in = 0.25;
        clip.fade_out = 0.5;
        clip.track = 2;
        project.clips.push(clip);

        let timeline =
            CompiledTimeline::from_project(&project, &map, TimelineCompileOptions::default())
                .unwrap();
        let descriptor = timeline.events().iter().find_map(|event| match event.kind {
            TimelineEventKind::AudioStart(descriptor) => Some(descriptor),
            _ => None,
        });
        let descriptor = descriptor.unwrap();
        assert_eq!(descriptor.asset_id, 77);
        assert_eq!(descriptor.source_offset_frame, 12_000);
        assert_eq!(descriptor.mixer_track, 3);
        assert_eq!(descriptor.fade_in_frames, 6_000);
        assert_eq!(descriptor.fade_out_frames, 12_000);
    }

    #[test]
    fn audio_fade_handles_follow_tempo_map_boundaries_instead_of_frame_fractions() {
        let mut tempo = AutomationLane::new(AutomationTarget::Tempo);
        tempo.set_curve(AutomationCurve::Hold);
        tempo.replace_points([
            AutomationPoint::new(0.0, 60.0),
            AutomationPoint::new(1.0, 120.0),
        ]);
        let map = TempoMap::new(60.0, Some(tempo), 2.0, 48_000).unwrap();
        let mut project = empty_project(2.0);
        project.audio_assets.push(AudioAsset {
            id: 78,
            name: "Tempo boundary asset".into(),
            path: PathBuf::from("offline.wav"),
            sample_rate: 48_000,
            channels: 2,
            bits_per_sample: 24,
            frames: 96_000,
            waveform_peaks: Vec::new(),
        });
        let mut clip = pattern_clip(6, 0.0, 2.0, 0);
        clip.kind = ClipKind::Audio;
        clip.audio_asset_id = Some(78);
        clip.audio_source_offset_frame = Some(0);
        clip.fade_in = 0.5;
        clip.fade_out = 0.5;
        project.clips.push(clip);

        let timeline =
            CompiledTimeline::from_project(&project, &map, TimelineCompileOptions::default())
                .unwrap();
        let descriptor = timeline.audio_clips()[0];
        assert_eq!(descriptor.start_frame, 0);
        assert_eq!(descriptor.clip_end_frame, 72_000);
        assert_eq!(descriptor.fade_in_frames, 48_000);
        assert_eq!(descriptor.fade_out_frames, 24_000);
    }

    #[test]
    fn audio_asset_bounds_preserve_playlist_fades_and_route_saturates_at_31() {
        let map = TempoMap::new(120.0, None, 3.0, 48_000).unwrap();
        let mut project = empty_project(3.0);
        project.audio_assets.push(AudioAsset {
            id: 88,
            name: "Short".into(),
            path: PathBuf::from("offline.wav"),
            sample_rate: 48_000,
            channels: 2,
            bits_per_sample: 24,
            frames: 30_000,
            waveform_peaks: Vec::new(),
        });
        let mut clip = pattern_clip(8, 0.0, 2.0, 0);
        clip.kind = ClipKind::Audio;
        clip.audio_asset_id = Some(88);
        clip.audio_source_offset_frame = Some(10_000);
        clip.track = usize::MAX;
        clip.fade_in = 0.5;
        clip.fade_out = 0.5;
        project.clips.push(clip);

        let timeline =
            CompiledTimeline::from_project(&project, &map, TimelineCompileOptions::default())
                .unwrap();
        let descriptor = timeline.audio_clips()[0];
        assert_eq!(descriptor.mixer_track, 31);
        assert_eq!(descriptor.clip_end_frame, 48_000);
        assert_eq!(descriptor.stop_frame, 20_000);
        assert_eq!(descriptor.fade_in_frames, 24_000);
        assert_eq!(descriptor.fade_out_frames, 24_000);
    }

    #[test]
    fn audio_source_offset_is_native_and_independent_of_song_tempo() {
        let mut tempo = AutomationLane::new(AutomationTarget::Tempo);
        tempo.set_curve(AutomationCurve::Hold);
        tempo.replace_points([
            AutomationPoint::new(0.0, 60.0),
            AutomationPoint::new(1.0, 120.0),
        ]);
        let map = TempoMap::new(60.0, Some(tempo), 4.0, 48_000).unwrap();
        let mut project = empty_project(4.0);
        project.audio_assets.push(AudioAsset {
            id: 5,
            name: "Asset".into(),
            path: PathBuf::from("offline.wav"),
            sample_rate: 48_000,
            channels: 2,
            bits_per_sample: 24,
            frames: 480_000,
            waveform_peaks: Vec::new(),
        });
        let mut clip = pattern_clip(6, 2.0, 1.0, 0);
        clip.kind = ClipKind::Audio;
        clip.audio_asset_id = Some(5);
        clip.audio_source_offset_frame = Some(12_345);
        project.clips.push(clip);

        let timeline =
            CompiledTimeline::from_project(&project, &map, TimelineCompileOptions::default())
                .unwrap();
        let descriptor = timeline.events().iter().find_map(|event| match event.kind {
            TimelineEventKind::AudioStart(descriptor) => Some(descriptor),
            _ => None,
        });
        assert_eq!(descriptor.unwrap().source_offset_frame, 12_345);
    }

    #[test]
    fn discontinuity_chase_restores_audio_automation_and_optional_long_notes() {
        let map = TempoMap::new(120.0, None, 4.0, 48_000).unwrap();
        let mut project = empty_project(4.0);
        project.channels.push(channel(1, false, false, 1));
        let mut notes = pattern(1, vec![[false; 16]]);
        notes.notes.push(PianoNote {
            id: 200,
            channel_id: Some(1),
            group_id: None,
            note: 67,
            start: 0.0,
            length: 3.0,
            velocity: 0.75,
            selected: false,
            muted: false,
        });
        project.patterns.push(notes);
        project.clips.push(pattern_clip(1, 0.0, 4.0, 1));

        project.audio_assets.push(AudioAsset {
            id: 300,
            name: "Audio".into(),
            path: PathBuf::from("offline.wav"),
            sample_rate: 24_000,
            channels: 2,
            bits_per_sample: 24,
            frames: 100_000,
            waveform_peaks: Vec::new(),
        });
        let mut audio = pattern_clip(2, 0.0, 4.0, 0);
        audio.kind = ClipKind::Audio;
        audio.audio_asset_id = Some(300);
        audio.audio_source_offset_frame = Some(1_000);
        project.clips.push(audio);

        let mut automation = AutomationLane::new(AutomationTarget::MasterVolume);
        automation.replace_points([
            AutomationPoint::new(0.0, 0.2),
            AutomationPoint::new(4.0, 0.8),
        ]);
        project.automation_lanes.push(ProjectAutomation {
            id: 400,
            name: "Master".into(),
            lane: automation,
        });

        let timeline =
            CompiledTimeline::from_project(&project, &map, TimelineCompileOptions::default())
                .unwrap();
        let chase_frame = map.beat_to_frame(1.0).unwrap();
        let default_state = timeline
            .chase_discontinuity(chase_frame, TimelineChaseOptions::default())
            .unwrap();
        assert!(default_state.notes.is_empty());
        assert_eq!(default_state.audio_clips.len(), 1);
        assert_eq!(default_state.audio_clips[0].source_position_frame, 13_000.0);
        assert_eq!(default_state.automation_layers.len(), 1);
        assert!((default_state.automation_layers[0].current_value - 0.35).abs() < 1.0e-4);
        assert!(default_state.automation_bases.iter().any(|base| {
            base.target == CompiledAutomationTarget::MasterVolume && base.value == 1.0
        }));

        let chased = timeline
            .chase_discontinuity(
                chase_frame,
                TimelineChaseOptions {
                    long_notes: LongNoteChasePolicy::Chase,
                },
            )
            .unwrap();
        assert_eq!(chased.notes.len(), 1);
        assert!(matches!(
            chased.notes[0].source,
            NoteSourceDescriptor::LegacyPiano {
                persistent_note_id: 200,
                ..
            }
        ));
        assert_eq!(
            timeline
                .chase_discontinuity(
                    timeline.duration_frames() + 1,
                    TimelineChaseOptions::default(),
                )
                .unwrap_err(),
            TimelineChaseError::FrameOutOfRange {
                frame: timeline.duration_frames() + 1,
                duration_frames: timeline.duration_frames(),
            }
        );
    }

    #[test]
    fn compilation_is_deterministic() {
        let map = TempoMap::new(120.0, None, 4.0, 48_000).unwrap();
        let mut project = empty_project(4.0);
        project.channels.push(channel(8, false, false, 2));
        let mut steps = [false; 16];
        steps[0] = true;
        steps[3] = true;
        project.patterns.push(pattern(99, vec![steps]));
        project.clips.push(pattern_clip(101, 0.25, 3.5, 99));
        let options = TimelineCompileOptions::default();

        let first = CompiledTimeline::from_project(&project, &map, options.clone()).unwrap();
        let second = CompiledTimeline::from_project(&project, &map, options).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn work_and_absolute_option_limits_fail_before_unbounded_expansion() {
        let map = TempoMap::new(120.0, None, 100.0, 48_000).unwrap();
        let mut project = empty_project(100.0);
        project.patterns.push(pattern(1, Vec::new()));
        project.clips.push(pattern_clip(1, 0.0, 100.0, 1));
        assert_eq!(
            CompiledTimeline::from_project(
                &project,
                &map,
                TimelineCompileOptions {
                    max_work_units: 10,
                    ..TimelineCompileOptions::default()
                },
            )
            .unwrap_err(),
            TimelineCompileError::WorkLimitExceeded { maximum: 10 }
        );

        assert_eq!(
            CompiledTimeline::from_project(
                &empty_project(1.0),
                &TempoMap::new(120.0, None, 1.0, 48_000).unwrap(),
                TimelineCompileOptions {
                    max_events: MAX_TIMELINE_EVENTS + 1,
                    ..TimelineCompileOptions::default()
                },
            )
            .unwrap_err(),
            TimelineCompileError::RequestedLimitExceedsMaximum {
                resource: TimelineResource::Events,
                requested: MAX_TIMELINE_EVENTS + 1,
                maximum: MAX_TIMELINE_EVENTS,
            }
        );
    }
}
