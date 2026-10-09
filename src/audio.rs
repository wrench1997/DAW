use std::{
    f32::consts::TAU,
    fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering, fence},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow};
use cpal::{
    ErrorKind, FromSample, Sample, SampleFormat, SizedSample, Stream, StreamConfig,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use rtrb::{Consumer, Producer, PushError, RingBuffer};

use crate::audio_device::{
    AudioBufferSizeRequest, AudioDeviceDirection, AudioDeviceProfile, AudioEffectiveBufferSize,
    AudioEffectiveStreamConfig, AudioStreamTelemetrySnapshot, CallbackTelemetry,
    resolve_audio_device,
};
use crate::audio_meter::{
    MeterFrame, MeterIdentity, MeterPublisher, MeterReader, MeterReadings, TrackPeak, meter_channel,
};
use crate::clip_fade::CompiledClipFades;
use crate::fixed_quantum::{
    FixedQuantumAdapter, FixedQuantumError, FixedQuantumProcessStatus, FixedQuantumStats,
    FrameEvent, MAX_FRAME_EVENTS_PER_CALLBACK, MAX_FRAME_EVENTS_PER_QUANTUM,
};
use crate::master_capture::MasterCaptureEndpoint;
use crate::midi_device::MidiInputReceiver;
use crate::midi_recording::{
    MidiRecordClockAnchor, MidiRecordPacket, MidiRecordRealtimeStamp, MidiTakeInvalidReason,
    PreparedMidiRecordEndpoint,
};
use crate::midi_runtime::{
    LiveMidiEvent, MidiClockAnchor, MidiEventScratch, MidiScheduleDecision, MidiScheduleWindow,
    MidiScratchPush, MidiTimestampMapper,
};
use crate::mixer_graph::{
    CompiledMixerGraph, FixedMixerGraphLayout, MIXER_GRAPH_MAX_NODES, MIXER_MASTER_RUNTIME_SLOT,
    MixerRouteTap,
};
use crate::pdc::{
    CompensationDelay, GeneratorPathLatency, GraphPdcGenerator, GraphPdcPlan,
    PDC_DEFAULT_MAX_DELAY_SAMPLES, PDC_MAX_GENERATORS, PDC_TRACK_COUNT, PdcPlan,
    PreparedMixerGraphDelayBank, Q128ControlHistory, StereoDelayLine,
};
use crate::plugin_parameter_edit::{
    CallbackEditReceipt, CallbackRejectReason, ParameterEditSubmission, ParameterEndpointKind,
};
use crate::plugins::plugin_runtime::{
    AudioThreadEndpoint, ParameterEditId as RuntimeParameterEditId, PluginEndpointManifest,
    PluginEndpointSnapshot, PluginLatencySnapshot,
};
use crate::timeline::{
    AutomationRampShape, ChannelBaseDescriptor, ChasedAudioClip, ChasedNote,
    CompiledAutomationTarget, CompiledPluginRoute, CompiledTimeline, PluginRouteDestination,
    TIMELINE_CALLBACK_MAX_AUDIO_ASSETS, TIMELINE_CALLBACK_MAX_EVENTS, TIMELINE_CALLBACK_MAX_FRAMES,
    TIMELINE_CALLBACK_MAX_GENERATOR_ENDPOINTS, TIMELINE_CALLBACK_MAX_PLUGIN_ENDPOINTS,
    TIMELINE_ENDPOINT_DISCONTINUITY_EVENTS, TIMELINE_ENDPOINT_LIVE_MAX_EVENTS_PER_CALLBACK,
    TIMELINE_ENDPOINT_LIVE_MAX_EVENTS_PER_QUANTUM, TIMELINE_ENDPOINT_MAX_DRIVEN_PLUGIN_PARAMETERS,
    TIMELINE_ENDPOINT_SYSTEM_MAX_EVENTS_PER_CALLBACK,
    TIMELINE_ENDPOINT_SYSTEM_MAX_EVENTS_PER_QUANTUM,
    TIMELINE_ENDPOINT_TIMELINE_MAX_EVENTS_PER_CALLBACK,
    TIMELINE_ENDPOINT_TIMELINE_MAX_EVENTS_PER_QUANTUM, TIMELINE_MAX_DRIVEN_PLUGIN_PARAMETERS,
    TIMELINE_PLUGIN_FIXED_QUANTUM_FRAMES, TimelineChaseOptions, TimelinePacket,
};
use crate::timeline_automation::{RealtimeTimelineAutomation, TimelineAutomationBlockPlan};
use crate::timeline_executor::{
    MAX_ACTIVE_NOTES, MAX_AUTOMATION_BASES, TimelineAudioSink, TimelineAutomationBlockEndpoint,
    TimelineAutomationChaseValue, TimelineAutomationTransition, TimelineExecutor,
};
use crate::timeline_plugin_automation::{
    EndpointEventClass, TimelineEndpointAddress, TimelineEndpointBatchPlan, TimelineEndpointHandle,
    TimelineEndpointKey, TimelineEndpointQuantumUsage,
};
use crate::timeline_runtime::{
    CommittedTimelineTransportActivation, PreparedLoopTimelineChase, PreparedTimelineChase,
    RealtimeTimelineRuntime, RetiredTimelineResource, TIMELINE_MIXER_PAN_RELEASE_TRACKS,
    TimelineControlError, TimelineDiscontinuityKind, TimelinePrepareChaseError,
    TimelineResourceQueueError, TimelineResourceQueueFailure, TimelineRuntimeController,
    TimelineRuntimeEvent, TimelineRuntimeStats, TimelineRuntimeValidationError,
    TimelineTransportActivationRejectReason, TimelineTransportActivationSpec,
    TimelineTransportActivationTicket, create_timeline_runtime,
};

const MAX_VOICES: usize = 32;
const COMMAND_CAPACITY: usize = 1024;
const MAX_COMMANDS_PER_CALLBACK: usize = 256;
const RETIRED_ASSET_CAPACITY: usize = COMMAND_CAPACITY;
const ASSET_EVENT_CAPACITY: usize = COMMAND_CAPACITY;
const RETIRED_INSERT_ENDPOINT_CAPACITY: usize = COMMAND_CAPACITY;
const INSERT_ENDPOINT_EVENT_CAPACITY: usize = COMMAND_CAPACITY;
const GENERATOR_ENDPOINT_EVENT_CAPACITY: usize = COMMAND_CAPACITY;
const MASTER_CAPTURE_EVENT_CAPACITY: usize = 16;
const PARAMETER_EDIT_CALLBACK_EVENT_CAPACITY: usize = 128;
const MIDI_INPUT_ROUTE_EVENT_CAPACITY: usize = 16;
const RETIRED_MIDI_INPUT_CAPACITY: usize = 16;
const MIDI_RECORDING_ENDPOINT_EVENT_CAPACITY: usize = 16;
const MIDI_INPUT_MAX_DRAIN_PER_CHUNK: usize = 64;
const PARAMETER_EDIT_CALLBACK_ADMISSION_CAPACITY: u32 =
    PARAMETER_EDIT_CALLBACK_EVENT_CAPACITY as u32;
const ENDPOINT_SHUTDOWN_REQUEST_ID: u64 = u64::MAX;
const TRACK_COUNT: usize = 32;
const BEAT_Q32_ONE: u64 = 1_u64 << 32;
const TRANSPORT_MAILBOX_READ_ATTEMPTS: usize = 3;
#[allow(dead_code)]
pub const MAX_INSERT_PLUGIN_SLOTS: usize = 10;
pub const MAX_GENERATOR_ENDPOINTS: usize = TIMELINE_CALLBACK_MAX_GENERATOR_ENDPOINTS;
/// Stable plug-in processing quantum, independent of device callback partitioning.
pub const DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES: usize = TIMELINE_PLUGIN_FIXED_QUANTUM_FRAMES;
/// The callback is rendered in bounded chunks backed by preallocated mixer buses.
/// This is also the maximum block handed to an insert plug-in worker.
pub const MAX_MIXER_BLOCK_FRAMES: usize = TIMELINE_CALLBACK_MAX_FRAMES;
const MAX_TIMELINE_PLANNED_EVENTS: usize = TIMELINE_CALLBACK_MAX_EVENTS;
// Match the transactional render-plan capacity so every block we can accept is
// validated with one executor state copy. A larger burst enters one overflow
// chunk and fails closed instead of multiplying 0.11 MiB state copies.
const TIMELINE_PACKET_CAPACITY: usize = MAX_TIMELINE_PLANNED_EVENTS;
const TIMELINE_CHANNEL_BASE_TABLE_CAPACITY: usize = 8_192;
const SYNC_DRIFT_OUTPUT_FRAMES: f64 = 256.0;
const PDC_TAP_CROSSFADE_FRAMES: u32 = 128;

const _: () = assert!(TRACK_COUNT == PDC_TRACK_COUNT);
const _: () = assert!(TRACK_COUNT == TIMELINE_MIXER_PAN_RELEASE_TRACKS);
const _: () = assert!(MAX_GENERATOR_ENDPOINTS == PDC_MAX_GENERATORS);
const _: () = assert!(TIMELINE_ENDPOINT_DISCONTINUITY_EVENTS == 16);

/// Maximum number of immutable sample buffers resident in the real-time asset table.
pub const MAX_REGISTERED_AUDIO_ASSETS: usize = TIMELINE_CALLBACK_MAX_AUDIO_ASSETS;
/// Maximum number of concurrently playing audio clips.
pub const MAX_AUDIO_CLIP_VOICES: usize = 128;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct TransportRequest {
    request_id: u64,
    discontinuity_id: u64,
    target_beat_q32: u64,
    target_timeline_frame: u64,
    playing: bool,
    loop_enabled: bool,
    loop_start_q32: u64,
    loop_end_q32: u64,
    loop_start_frame: u64,
    loop_end_frame: u64,
}

enum TransportMutation {
    SetPlaying(bool),
    Discontinuity {
        target_beat_q32: u64,
        target_timeline_frame: u64,
        playing: Option<bool>,
    },
    SetLoop {
        enabled: bool,
        start_q32: u64,
        end_q32: u64,
        start_frame: u64,
        end_frame: u64,
    },
}

/// A latest-state transport mailbox. The UI is the only practical writer, but
/// the CAS keeps accidental concurrent writers safe. The callback performs only
/// bounded seqlock reads and retains its previous state if it catches a write.
#[derive(Debug, Default)]
struct TransportMailbox {
    sequence: AtomicU64,
    request_id: AtomicU64,
    discontinuity_id: AtomicU64,
    target_beat_q32: AtomicU64,
    target_timeline_frame: AtomicU64,
    playing: AtomicBool,
    loop_enabled: AtomicBool,
    loop_start_q32: AtomicU64,
    loop_end_q32: AtomicU64,
    loop_start_frame: AtomicU64,
    loop_end_frame: AtomicU64,
}

impl TransportMailbox {
    fn publish(&self, mutation: TransportMutation) -> u64 {
        let sequence = loop {
            let sequence = self.sequence.load(Ordering::Acquire);
            if sequence & 1 == 0
                && self
                    .sequence
                    .compare_exchange_weak(
                        sequence,
                        sequence.wrapping_add(1),
                        Ordering::Acquire,
                        Ordering::Relaxed,
                    )
                    .is_ok()
            {
                break sequence;
            }
            std::hint::spin_loop();
        };
        fence(Ordering::Release);

        let mut request = self.load_relaxed();
        // An atomic Timeline activation may deliberately move callback state
        // without rewriting this legacy/latest-state mailbox. Therefore every
        // explicit control-thread mutation must receive a fresh request id,
        // even when its value matches the mailbox's cached value: a repeated
        // `false` can still be the pause that overrides an active Timeline.
        match mutation {
            TransportMutation::SetPlaying(playing) => {
                request.playing = playing;
            }
            TransportMutation::Discontinuity {
                target_beat_q32,
                target_timeline_frame,
                playing,
            } => {
                request.discontinuity_id = next_nonzero_id(request.discontinuity_id);
                request.target_beat_q32 = target_beat_q32;
                request.target_timeline_frame = target_timeline_frame;
                if let Some(playing) = playing {
                    request.playing = playing;
                }
            }
            TransportMutation::SetLoop {
                enabled,
                start_q32,
                end_q32,
                start_frame,
                end_frame,
            } => {
                let enabled = enabled && end_frame > start_frame;
                request.loop_enabled = enabled;
                request.loop_start_q32 = start_q32;
                request.loop_end_q32 = end_q32;
                request.loop_start_frame = start_frame;
                request.loop_end_frame = end_frame;
            }
        }
        request.request_id = next_nonzero_id(request.request_id);
        self.store_relaxed(request);
        self.sequence
            .store(sequence.wrapping_add(2), Ordering::Release);
        request.request_id
    }

    fn try_load(&self) -> Option<TransportRequest> {
        for _ in 0..TRANSPORT_MAILBOX_READ_ATTEMPTS {
            let before = self.sequence.load(Ordering::Acquire);
            if before & 1 != 0 {
                continue;
            }
            let request = self.load_relaxed();
            fence(Ordering::Acquire);
            let after = self.sequence.load(Ordering::Relaxed);
            if before == after {
                return Some(request);
            }
        }
        None
    }

    fn control_thread_request_id(&self) -> u64 {
        loop {
            if let Some(request) = self.try_load() {
                return request.request_id;
            }
            std::hint::spin_loop();
        }
    }

    fn load_relaxed(&self) -> TransportRequest {
        TransportRequest {
            request_id: self.request_id.load(Ordering::Relaxed),
            discontinuity_id: self.discontinuity_id.load(Ordering::Relaxed),
            target_beat_q32: self.target_beat_q32.load(Ordering::Relaxed),
            target_timeline_frame: self.target_timeline_frame.load(Ordering::Relaxed),
            playing: self.playing.load(Ordering::Relaxed),
            loop_enabled: self.loop_enabled.load(Ordering::Relaxed),
            loop_start_q32: self.loop_start_q32.load(Ordering::Relaxed),
            loop_end_q32: self.loop_end_q32.load(Ordering::Relaxed),
            loop_start_frame: self.loop_start_frame.load(Ordering::Relaxed),
            loop_end_frame: self.loop_end_frame.load(Ordering::Relaxed),
        }
    }

    fn store_relaxed(&self, request: TransportRequest) {
        self.request_id.store(request.request_id, Ordering::Relaxed);
        self.discontinuity_id
            .store(request.discontinuity_id, Ordering::Relaxed);
        self.target_beat_q32
            .store(request.target_beat_q32, Ordering::Relaxed);
        self.target_timeline_frame
            .store(request.target_timeline_frame, Ordering::Relaxed);
        self.playing.store(request.playing, Ordering::Relaxed);
        self.loop_enabled
            .store(request.loop_enabled, Ordering::Relaxed);
        self.loop_start_q32
            .store(request.loop_start_q32, Ordering::Relaxed);
        self.loop_end_q32
            .store(request.loop_end_q32, Ordering::Relaxed);
        self.loop_start_frame
            .store(request.loop_start_frame, Ordering::Relaxed);
        self.loop_end_frame
            .store(request.loop_end_frame, Ordering::Relaxed);
    }
}

const fn next_nonzero_id(current: u64) -> u64 {
    let next = current.wrapping_add(1);
    if next == 0 { 1 } else { next }
}

pub(crate) fn beat_to_q32(beat: f64) -> u64 {
    if !beat.is_finite() || beat <= 0.0 {
        return 0;
    }
    (beat * BEAT_Q32_ONE as f64)
        .round()
        .clamp(0.0, u64::MAX as f64) as u64
}

fn q32_to_beat(beat_q32: u64) -> f64 {
    beat_q32 as f64 / BEAT_Q32_ONE as f64
}

fn midi_safety_frames_to_complete_quantum(input_phase: usize, deferred: usize) -> usize {
    debug_assert!(input_phase < DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
    if deferred == 0 {
        DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES - input_phase
    } else {
        deferred.saturating_add(DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES)
    }
}

/// Exact destination stamped onto one control-thread-owned MIDI input connection.
/// This first production path intentionally supports only one Generator endpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MidiGeneratorRouteStamp {
    pub project_session: u64,
    pub channel_id: u32,
    pub endpoint_id: u64,
    pub plugin_instance_id: u64,
    pub slot: Option<usize>,
}

/// Ownership package prepared off the audio callback. The receiver is moved through
/// lifecycle/retire rings and is never destroyed by the realtime thread.
pub struct PreparedMidiInputRoute {
    stamp: MidiGeneratorRouteStamp,
    receiver: MidiInputReceiver,
}

impl PreparedMidiInputRoute {
    pub fn new(receiver: MidiInputReceiver, stamp: MidiGeneratorRouteStamp) -> Option<Self> {
        if stamp.project_session == 0
            || stamp.endpoint_id == 0
            || stamp.plugin_instance_id == 0
            || stamp
                .slot
                .is_some_and(|slot| slot >= MAX_INSERT_PLUGIN_SLOTS)
        {
            return None;
        }
        Some(Self { stamp, receiver })
    }

    pub const fn stamp(&self) -> MidiGeneratorRouteStamp {
        self.stamp
    }

    pub const fn connection_epoch(&self) -> u64 {
        self.receiver.connection_epoch()
    }
}

impl fmt::Debug for PreparedMidiInputRoute {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedMidiInputRoute")
            .field("stamp", &self.stamp)
            .field("connection_epoch", &self.connection_epoch())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MidiInputRouteEvent {
    Installed {
        route_id: u64,
        connection_epoch: u64,
        replaced_route_id: Option<u64>,
        success: bool,
    },
    Removed {
        route_id: u64,
        removed: bool,
    },
    Cleared {
        request_id: u64,
        removed_route_id: Option<u64>,
    },
}

/// Exact callback lifecycle for one live-MIDI take. Endpoint ownership is always returned on
/// rejection, stop, or clear; the audio thread never destroys the SPSC producer.
pub enum MidiRecordingEndpointEvent {
    Started {
        stamp: MidiRecordRealtimeStamp,
        start: MidiRecordClockAnchor,
        success: bool,
        returned_endpoint: Option<PreparedMidiRecordEndpoint>,
    },
    Stopped {
        requested_session_id: u64,
        stamp: Option<MidiRecordRealtimeStamp>,
        stop: MidiRecordClockAnchor,
        returned_endpoint: Option<PreparedMidiRecordEndpoint>,
    },
    Cleared {
        request_id: u64,
        stamp: Option<MidiRecordRealtimeStamp>,
        stop: MidiRecordClockAnchor,
        returned_endpoint: Option<PreparedMidiRecordEndpoint>,
    },
}

impl fmt::Debug for MidiRecordingEndpointEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Started {
                stamp,
                start,
                success,
                returned_endpoint,
            } => formatter
                .debug_struct("Started")
                .field("stamp", stamp)
                .field("start", start)
                .field("success", success)
                .field("returned_endpoint", &returned_endpoint.is_some())
                .finish(),
            Self::Stopped {
                requested_session_id,
                stamp,
                stop,
                returned_endpoint,
            } => formatter
                .debug_struct("Stopped")
                .field("requested_session_id", requested_session_id)
                .field("stamp", stamp)
                .field("stop", stop)
                .field("returned_endpoint", &returned_endpoint.is_some())
                .finish(),
            Self::Cleared {
                request_id,
                stamp,
                stop,
                returned_endpoint,
            } => formatter
                .debug_struct("Cleared")
                .field("request_id", request_id)
                .field("stamp", stamp)
                .field("stop", stop)
                .field("returned_endpoint", &returned_endpoint.is_some())
                .finish(),
        }
    }
}

pub enum AudioCommand {
    RegisterAsset {
        operation: AudioAssetOperation,
        id: u64,
        samples: Arc<[f32]>,
        sample_rate: u32,
        channels: u16,
    },
    #[allow(dead_code)]
    UnregisterAsset {
        operation: AudioAssetOperation,
        id: u64,
    },
    ClearAssets {
        operation: AudioAssetOperation,
    },
    /// Starts (or exactly restarts) one clip voice at a source-frame position.
    PlayClip {
        clip_id: u64,
        asset_id: u64,
        source_frame: f64,
        gain: f32,
        mixer_track: usize,
    },
    /// Keeps one clip voice aligned with the timeline. Small position differences
    /// are left alone; gain/routing are always updated.
    SyncClip {
        clip_id: u64,
        asset_id: u64,
        source_frame: f64,
        gain: f32,
        mixer_track: usize,
    },
    StopClip {
        clip_id: u64,
    },
    NoteOn {
        note: u8,
        velocity: f32,
        mixer_track: usize,
    },
    StopAll,
    SetMaster(f32),
    SetMasterPan(f32),
    SetTrackGain {
        track: usize,
        gain: f32,
    },
    SetTrackPan {
        track: usize,
        pan: f32,
    },
    SetTrackMuted {
        track: usize,
        muted: bool,
    },
    SetTrackSolo {
        track: usize,
        solo: bool,
    },
    InstallInsertEndpoint {
        insert: usize,
        endpoint_id: u64,
        endpoint: PreparedFixedEndpoint,
    },
    RemoveInsertEndpoint {
        insert: usize,
    },
    ClearInsertEndpoints {
        request_id: u64,
    },
    SendInsertMidi {
        insert: usize,
        slot: Option<usize>,
        data: [u8; 3],
        sample_offset: usize,
    },
    SetInsertParameter {
        insert: usize,
        slot: usize,
        id: u32,
        normalized: f32,
    },
    InstallGeneratorEndpoint {
        channel_id: u32,
        endpoint_id: u64,
        plugin_instance_id: u64,
        mixer_track: usize,
        endpoint: PreparedFixedEndpoint,
        pdc_delay: StereoDelayLine,
    },
    RemoveGeneratorEndpoint {
        channel_id: u32,
    },
    ClearGeneratorEndpoints {
        request_id: u64,
    },
    SetGeneratorRoute {
        channel_id: u32,
        mixer_track: usize,
    },
    SendGeneratorMidi {
        channel_id: u32,
        slot: Option<usize>,
        data: [u8; 3],
        sample_offset: usize,
    },
    SetGeneratorParameter {
        channel_id: u32,
        slot: usize,
        id: u32,
        normalized: f32,
    },
    /// Exact, callback-validated generic parameter edit. Queue admission is not worker
    /// admission; the caller confirms submission after this command enters the ring and then
    /// waits for either a callback rejection or the owning worker's reliable receipt.
    EditPluginParameter(ParameterEditSubmission),
    InstallMidiInput {
        route_id: u64,
        prepared: PreparedMidiInputRoute,
    },
    RemoveMidiInput {
        route_id: u64,
    },
    ClearMidiInput {
        request_id: u64,
    },
    StartMidiRecording {
        endpoint: PreparedMidiRecordEndpoint,
    },
    StopMidiRecording {
        session_id: u64,
    },
    ClearMidiRecording {
        request_id: u64,
    },
    InstallMasterCapture {
        capture_id: u64,
        endpoint: MasterCaptureEndpoint,
    },
    StopMasterCapture {
        capture_id: u64,
    },
    ClearMasterCapture {
        request_id: u64,
    },
}

/// Confirmation emitted by the audio callback after an asset lifecycle command
/// has actually taken effect. A successful command enqueue is not registration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioAssetOperation {
    pub engine_session: u64,
    pub generation: u64,
    pub operation_id: u64,
}

impl AudioAssetOperation {
    const SHUTDOWN: Self = Self {
        engine_session: u64::MAX,
        generation: u64::MAX,
        operation_id: ENDPOINT_SHUTDOWN_REQUEST_ID,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioAssetEvent {
    Registered {
        operation: AudioAssetOperation,
        id: u64,
        success: bool,
    },
    Unregistered {
        operation: AudioAssetOperation,
        id: u64,
        removed: bool,
    },
    Cleared {
        operation: AudioAssetOperation,
        removed: usize,
    },
}

/// Callback-confirmed lifecycle for one worker-backed Mixer insert endpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InsertEndpointEvent {
    Installed {
        insert: usize,
        endpoint_id: u64,
        replaced_endpoint_id: Option<u64>,
        success: bool,
    },
    Removed {
        insert: usize,
        endpoint_id: Option<u64>,
    },
    Cleared {
        request_id: u64,
        removed: usize,
    },
}

/// Callback-confirmed lifecycle for one Channel instrument worker endpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeneratorEndpointEvent {
    Installed {
        channel_id: u32,
        endpoint_id: u64,
        plugin_instance_id: u64,
        mixer_track: usize,
        replaced_endpoint_id: Option<u64>,
        replaced_plugin_instance_id: Option<u64>,
        success: bool,
    },
    Removed {
        channel_id: u32,
        endpoint_id: Option<u64>,
        plugin_instance_id: Option<u64>,
    },
    Cleared {
        request_id: u64,
        removed: usize,
    },
    RouteSet {
        channel_id: u32,
        endpoint_id: Option<u64>,
        plugin_instance_id: Option<u64>,
        mixer_track: usize,
        success: bool,
    },
}

/// Callback-confirmed lifecycle for the real-time stereo master tap.
///
/// Endpoint ownership is returned in failure, stop, and clear events so the
/// device callback never destroys the capture producer or joins its writer.
pub enum MasterCaptureEndpointEvent {
    Installed {
        capture_id: u64,
        start_device_frame: u64,
        success: bool,
        returned_endpoint: Option<MasterCaptureEndpoint>,
    },
    Stopped {
        capture_id: u64,
        end_device_frame: u64,
        returned_endpoint: Option<MasterCaptureEndpoint>,
    },
    Cleared {
        request_id: u64,
        capture_id: Option<u64>,
        end_device_frame: u64,
        returned_endpoint: Option<MasterCaptureEndpoint>,
    },
}

impl fmt::Debug for MasterCaptureEndpointEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Installed {
                capture_id,
                start_device_frame,
                success,
                returned_endpoint,
            } => formatter
                .debug_struct("Installed")
                .field("capture_id", capture_id)
                .field("start_device_frame", start_device_frame)
                .field("success", success)
                .field("returned_endpoint", &returned_endpoint.is_some())
                .finish(),
            Self::Stopped {
                capture_id,
                end_device_frame,
                returned_endpoint,
            } => formatter
                .debug_struct("Stopped")
                .field("capture_id", capture_id)
                .field("end_device_frame", end_device_frame)
                .field("returned_endpoint", &returned_endpoint.is_some())
                .finish(),
            Self::Cleared {
                request_id,
                capture_id,
                end_device_frame,
                returned_endpoint,
            } => formatter
                .debug_struct("Cleared")
                .field("request_id", request_id)
                .field("capture_id", capture_id)
                .field("end_device_frame", end_device_frame)
                .field("returned_endpoint", &returned_endpoint.is_some())
                .finish(),
        }
    }
}

impl fmt::Debug for AudioCommand {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RegisterAsset {
                operation,
                id,
                samples,
                sample_rate,
                channels,
            } => formatter
                .debug_struct("RegisterAsset")
                .field("operation", operation)
                .field("id", id)
                .field("sample_count", &samples.len())
                .field("sample_rate", sample_rate)
                .field("channels", channels)
                .finish(),
            Self::UnregisterAsset { operation, id } => formatter
                .debug_struct("UnregisterAsset")
                .field("operation", operation)
                .field("id", id)
                .finish(),
            Self::ClearAssets { operation } => formatter
                .debug_struct("ClearAssets")
                .field("operation", operation)
                .finish(),
            Self::PlayClip {
                clip_id,
                asset_id,
                source_frame,
                gain,
                mixer_track,
            } => formatter
                .debug_struct("PlayClip")
                .field("clip_id", clip_id)
                .field("asset_id", asset_id)
                .field("source_frame", source_frame)
                .field("gain", gain)
                .field("mixer_track", mixer_track)
                .finish(),
            Self::SyncClip {
                clip_id,
                asset_id,
                source_frame,
                gain,
                mixer_track,
            } => formatter
                .debug_struct("SyncClip")
                .field("clip_id", clip_id)
                .field("asset_id", asset_id)
                .field("source_frame", source_frame)
                .field("gain", gain)
                .field("mixer_track", mixer_track)
                .finish(),
            Self::StopClip { clip_id } => formatter
                .debug_struct("StopClip")
                .field("clip_id", clip_id)
                .finish(),
            Self::NoteOn {
                note,
                velocity,
                mixer_track,
            } => formatter
                .debug_struct("NoteOn")
                .field("note", note)
                .field("velocity", velocity)
                .field("mixer_track", mixer_track)
                .finish(),
            Self::StopAll => formatter.write_str("StopAll"),
            Self::SetMaster(value) => formatter.debug_tuple("SetMaster").field(value).finish(),
            Self::SetMasterPan(value) => {
                formatter.debug_tuple("SetMasterPan").field(value).finish()
            }
            Self::SetTrackGain { track, gain } => formatter
                .debug_struct("SetTrackGain")
                .field("track", track)
                .field("gain", gain)
                .finish(),
            Self::SetTrackPan { track, pan } => formatter
                .debug_struct("SetTrackPan")
                .field("track", track)
                .field("pan", pan)
                .finish(),
            Self::SetTrackMuted { track, muted } => formatter
                .debug_struct("SetTrackMuted")
                .field("track", track)
                .field("muted", muted)
                .finish(),
            Self::SetTrackSolo { track, solo } => formatter
                .debug_struct("SetTrackSolo")
                .field("track", track)
                .field("solo", solo)
                .finish(),
            Self::InstallInsertEndpoint {
                insert,
                endpoint_id,
                ..
            } => formatter
                .debug_struct("InstallInsertEndpoint")
                .field("insert", insert)
                .field("endpoint_id", endpoint_id)
                .finish_non_exhaustive(),
            Self::RemoveInsertEndpoint { insert } => formatter
                .debug_struct("RemoveInsertEndpoint")
                .field("insert", insert)
                .finish(),
            Self::ClearInsertEndpoints { request_id } => formatter
                .debug_struct("ClearInsertEndpoints")
                .field("request_id", request_id)
                .finish(),
            Self::SendInsertMidi {
                insert,
                slot,
                data,
                sample_offset,
            } => formatter
                .debug_struct("SendInsertMidi")
                .field("insert", insert)
                .field("slot", slot)
                .field("data", data)
                .field("sample_offset", sample_offset)
                .finish(),
            Self::SetInsertParameter {
                insert,
                slot,
                id,
                normalized,
            } => formatter
                .debug_struct("SetInsertParameter")
                .field("insert", insert)
                .field("slot", slot)
                .field("id", id)
                .field("normalized", normalized)
                .finish(),
            Self::InstallGeneratorEndpoint {
                channel_id,
                endpoint_id,
                plugin_instance_id,
                mixer_track,
                ..
            } => formatter
                .debug_struct("InstallGeneratorEndpoint")
                .field("channel_id", channel_id)
                .field("endpoint_id", endpoint_id)
                .field("plugin_instance_id", plugin_instance_id)
                .field("mixer_track", mixer_track)
                .finish_non_exhaustive(),
            Self::RemoveGeneratorEndpoint { channel_id } => formatter
                .debug_struct("RemoveGeneratorEndpoint")
                .field("channel_id", channel_id)
                .finish(),
            Self::ClearGeneratorEndpoints { request_id } => formatter
                .debug_struct("ClearGeneratorEndpoints")
                .field("request_id", request_id)
                .finish(),
            Self::SetGeneratorRoute {
                channel_id,
                mixer_track,
            } => formatter
                .debug_struct("SetGeneratorRoute")
                .field("channel_id", channel_id)
                .field("mixer_track", mixer_track)
                .finish(),
            Self::SendGeneratorMidi {
                channel_id,
                slot,
                data,
                sample_offset,
            } => formatter
                .debug_struct("SendGeneratorMidi")
                .field("channel_id", channel_id)
                .field("slot", slot)
                .field("data", data)
                .field("sample_offset", sample_offset)
                .finish(),
            Self::SetGeneratorParameter {
                channel_id,
                slot,
                id,
                normalized,
            } => formatter
                .debug_struct("SetGeneratorParameter")
                .field("channel_id", channel_id)
                .field("slot", slot)
                .field("id", id)
                .field("normalized", normalized)
                .finish(),
            Self::EditPluginParameter(submission) => formatter
                .debug_tuple("EditPluginParameter")
                .field(submission)
                .finish(),
            Self::InstallMidiInput { route_id, prepared } => formatter
                .debug_struct("InstallMidiInput")
                .field("route_id", route_id)
                .field("prepared", prepared)
                .finish(),
            Self::RemoveMidiInput { route_id } => formatter
                .debug_struct("RemoveMidiInput")
                .field("route_id", route_id)
                .finish(),
            Self::ClearMidiInput { request_id } => formatter
                .debug_struct("ClearMidiInput")
                .field("request_id", request_id)
                .finish(),
            Self::StartMidiRecording { endpoint } => formatter
                .debug_struct("StartMidiRecording")
                .field("endpoint", endpoint)
                .finish(),
            Self::StopMidiRecording { session_id } => formatter
                .debug_struct("StopMidiRecording")
                .field("session_id", session_id)
                .finish(),
            Self::ClearMidiRecording { request_id } => formatter
                .debug_struct("ClearMidiRecording")
                .field("request_id", request_id)
                .finish(),
            Self::InstallMasterCapture { capture_id, .. } => formatter
                .debug_struct("InstallMasterCapture")
                .field("capture_id", capture_id)
                .finish_non_exhaustive(),
            Self::StopMasterCapture { capture_id } => formatter
                .debug_struct("StopMasterCapture")
                .field("capture_id", capture_id)
                .finish(),
            Self::ClearMasterCapture { request_id } => formatter
                .debug_struct("ClearMasterCapture")
                .field("request_id", request_id)
                .finish(),
        }
    }
}

#[derive(Debug)]
pub struct AudioStatus {
    pub playing: AtomicBool,
    pub recording: AtomicBool,
    pub tempo_milli: AtomicU32,
    pub sample_rate: AtomicU32,
    pub rendered_frames: AtomicU64,
    pub device_frame: AtomicU64,
    pub timeline_frame: AtomicU64,
    pub beat_q32: AtomicU64,
    pub transport_epoch: AtomicU64,
    pub transport_loop_count: AtomicU64,
    pub applied_transport_request: AtomicU64,
    pub plugin_epoch_resets: AtomicU64,
    pub plugin_epoch_reset_failures: AtomicU64,
    pub last_plugin_endpoint_epoch: AtomicU64,
    pub plugin_fixed_quantum_frames: AtomicU32,
    pub plugin_fixed_quantum_event_overflows: AtomicU64,
    pub plugin_fixed_quantum_invalid_events: AtomicU64,
    pub plugin_fixed_quantum_event_rejections: AtomicU64,
    pub plugin_fixed_quantum_bridge_gaps: AtomicU64,
    pub plugin_fixed_quantum_output_underflow_frames: AtomicU64,
    pub timeline_execution_failures: AtomicU64,
    pub timeline_missing_assets: AtomicU64,
    pub timeline_automation_pending: AtomicU64,
    pub timeline_automation_unsupported: AtomicU64,
    pub transport_sequence: AtomicU64,
    pdc_sequence: AtomicU64,
    pdc_plan_revision: AtomicU64,
    pdc_reference_latency_samples: AtomicU64,
    pdc_master_latency_samples: AtomicU32,
    pdc_output_latency_samples: AtomicU64,
    pdc_clamped_path_count: AtomicU32,
    pdc_maximum_delay_samples: AtomicU32,
    pub xruns: AtomicU64,
    pub command_queue_full: AtomicU64,
}

impl Default for AudioStatus {
    fn default() -> Self {
        Self {
            playing: AtomicBool::new(false),
            recording: AtomicBool::new(false),
            tempo_milli: AtomicU32::new(128_000),
            sample_rate: AtomicU32::new(48_000),
            rendered_frames: AtomicU64::new(0),
            device_frame: AtomicU64::new(0),
            timeline_frame: AtomicU64::new(0),
            beat_q32: AtomicU64::new(0),
            transport_epoch: AtomicU64::new(1),
            transport_loop_count: AtomicU64::new(0),
            applied_transport_request: AtomicU64::new(0),
            plugin_epoch_resets: AtomicU64::new(0),
            plugin_epoch_reset_failures: AtomicU64::new(0),
            last_plugin_endpoint_epoch: AtomicU64::new(0),
            plugin_fixed_quantum_frames: AtomicU32::new(DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES as u32),
            plugin_fixed_quantum_event_overflows: AtomicU64::new(0),
            plugin_fixed_quantum_invalid_events: AtomicU64::new(0),
            plugin_fixed_quantum_event_rejections: AtomicU64::new(0),
            plugin_fixed_quantum_bridge_gaps: AtomicU64::new(0),
            plugin_fixed_quantum_output_underflow_frames: AtomicU64::new(0),
            timeline_execution_failures: AtomicU64::new(0),
            timeline_missing_assets: AtomicU64::new(0),
            timeline_automation_pending: AtomicU64::new(0),
            timeline_automation_unsupported: AtomicU64::new(0),
            transport_sequence: AtomicU64::new(0),
            pdc_sequence: AtomicU64::new(0),
            pdc_plan_revision: AtomicU64::new(0),
            pdc_reference_latency_samples: AtomicU64::new(0),
            pdc_master_latency_samples: AtomicU32::new(0),
            pdc_output_latency_samples: AtomicU64::new(0),
            pdc_clamped_path_count: AtomicU32::new(0),
            pdc_maximum_delay_samples: AtomicU32::new(PDC_DEFAULT_MAX_DELAY_SAMPLES),
            xruns: AtomicU64::new(0),
            command_queue_full: AtomicU64::new(0),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct TransportStatusFields {
    device_frame: u64,
    timeline_frame: u64,
    beat_q32: u64,
    epoch: u64,
    loop_count: u64,
    applied_request: u64,
    playing: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct PdcStatusFields {
    plan_revision: u64,
    reference_latency_samples: u64,
    master_latency_samples: u32,
    output_latency_samples: u64,
    clamped_path_count: u32,
    maximum_delay_samples: u32,
}

impl AudioStatus {
    fn transport_fields(&self) -> TransportStatusFields {
        loop {
            let before = self.transport_sequence.load(Ordering::Acquire);
            if before & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }
            let fields = TransportStatusFields {
                device_frame: self.device_frame.load(Ordering::Relaxed),
                timeline_frame: self.timeline_frame.load(Ordering::Relaxed),
                beat_q32: self.beat_q32.load(Ordering::Relaxed),
                epoch: self.transport_epoch.load(Ordering::Relaxed),
                loop_count: self.transport_loop_count.load(Ordering::Relaxed),
                applied_request: self.applied_transport_request.load(Ordering::Relaxed),
                playing: self.playing.load(Ordering::Relaxed),
            };
            fence(Ordering::Acquire);
            let after = self.transport_sequence.load(Ordering::Relaxed);
            if before == after {
                return fields;
            }
        }
    }

    fn pdc_fields(&self) -> PdcStatusFields {
        loop {
            let before = self.pdc_sequence.load(Ordering::Acquire);
            if before & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }
            let fields = PdcStatusFields {
                plan_revision: self.pdc_plan_revision.load(Ordering::Relaxed),
                reference_latency_samples: self
                    .pdc_reference_latency_samples
                    .load(Ordering::Relaxed),
                master_latency_samples: self.pdc_master_latency_samples.load(Ordering::Relaxed),
                output_latency_samples: self.pdc_output_latency_samples.load(Ordering::Relaxed),
                clamped_path_count: self.pdc_clamped_path_count.load(Ordering::Relaxed),
                maximum_delay_samples: self.pdc_maximum_delay_samples.load(Ordering::Relaxed),
            };
            fence(Ordering::Acquire);
            let after = self.pdc_sequence.load(Ordering::Relaxed);
            if before == after {
                return fields;
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct AudioSnapshot {
    pub device: String,
    pub sample_rate: u32,
    /// Original control-plane request. `BackendDefault` means no concrete
    /// callback size was requested from the driver.
    pub requested_buffer_size: AudioBufferSizeRequest,
    /// Concrete stream request selected by deterministic device negotiation.
    /// A backend-default request remains unknown until a callback is observed.
    pub effective_buffer_size: AudioEffectiveBufferSize,
    /// Most recent complete raw backend callback size, before internal render
    /// chunking. `None` means that this stream has not delivered a callback.
    pub actual_buffer_size: Option<u32>,
    pub rendered_frames: u64,
    pub device_frame: u64,
    pub timeline_frame: u64,
    pub beat_q32: u64,
    pub beat_position: f64,
    pub transport_epoch: u64,
    pub transport_loop_count: u64,
    pub applied_transport_request: u64,
    pub transport_playing: bool,
    pub plugin_epoch_resets: u64,
    pub plugin_epoch_reset_failures: u64,
    pub last_plugin_endpoint_epoch: u64,
    pub plugin_fixed_quantum_frames: u32,
    pub plugin_fixed_quantum_event_overflows: u64,
    pub plugin_fixed_quantum_invalid_events: u64,
    pub plugin_fixed_quantum_event_rejections: u64,
    pub plugin_fixed_quantum_bridge_gaps: u64,
    pub plugin_fixed_quantum_output_underflow_frames: u64,
    /// Current block's driven target streams still awaiting a DSP application
    /// path. Native Channel volume/pan/mute streams are excluded once bound;
    /// generator channels and every other target family remain pending.
    pub timeline_automation_pending: u64,
    /// Cumulative chased values and per-sample kernel values intentionally not
    /// applied to DSP. Successfully bound native Channel targets are excluded.
    pub timeline_automation_unsupported: u64,
    pub timeline_execution_failures: u64,
    pub timeline_missing_assets: u64,
    #[allow(dead_code)]
    pub pdc_plan_revision: u64,
    #[allow(dead_code)]
    pub pdc_reference_latency_samples: u64,
    #[allow(dead_code)]
    pub pdc_master_latency_samples: u32,
    #[allow(dead_code)]
    pub pdc_output_latency_samples: u64,
    #[allow(dead_code)]
    pub pdc_clamped_path_count: u32,
    #[allow(dead_code)]
    pub pdc_maximum_delay_samples: u32,
    pub xruns: u64,
    pub command_queue_full: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum DeviceStreamLifecycle {
    #[default]
    Prepared,
    Playing,
}

impl DeviceStreamLifecycle {
    fn observe_play_result(&mut self, succeeded: bool) {
        if succeeded {
            *self = Self::Playing;
        }
    }

    const fn requires_callback_shutdown(self) -> bool {
        matches!(self, Self::Playing)
    }
}

pub struct AudioEngine {
    stream: Option<Stream>,
    stream_lifecycle: DeviceStreamLifecycle,
    meter_reader: MeterReader,
    timeline_runtime: Option<TimelineRuntimeController>,
    /// Control-thread optimization hint only. The callback validates every
    /// reuse request against its actual active/candidate ownership.
    timeline_mixer_graph_fingerprint: Option<u64>,
    producer: Producer<AudioCommand>,
    retired_assets: Consumer<Arc<[f32]>>,
    asset_events: Consumer<AudioAssetEvent>,
    retired_insert_endpoints: Consumer<RetiredEndpointResource>,
    insert_endpoint_events: Consumer<InsertEndpointEvent>,
    generator_endpoint_events: Consumer<GeneratorEndpointEvent>,
    retired_midi_inputs: Consumer<RetiredMidiInputResource>,
    midi_input_route_events: Consumer<MidiInputRouteEvent>,
    midi_recording_endpoint_events: Consumer<MidiRecordingEndpointEvent>,
    parameter_edit_callback_events: Consumer<CallbackEditReceipt>,
    parameter_edit_callback_admission: Arc<AtomicU32>,
    master_capture_events: Consumer<MasterCaptureEndpointEvent>,
    status: Arc<AudioStatus>,
    transport_mailbox: Arc<TransportMailbox>,
    callback_telemetry: Arc<CallbackTelemetry>,
    device_name: String,
    device_profile: AudioDeviceProfile,
    effective_device_profile: AudioDeviceProfile,
    effective_stream_config: AudioEffectiveStreamConfig,
    has_realtime_owned_resources: bool,
    timeline_runtime_shutdown_confirmed: bool,
}

impl AudioEngine {
    pub fn start() -> Result<Self> {
        Self::start_with_profile(&AudioDeviceProfile::system_default_output())
    }

    /// Resolves and negotiates an output profile, builds every callback-owned
    /// resource, and starts the resulting device stream.
    pub fn start_with_profile(profile: &AudioDeviceProfile) -> Result<Self> {
        let mut engine = Self::prepare_with_profile(profile)?;
        engine.start_device_stream()?;
        Ok(engine)
    }

    /// Resolves and negotiates an output profile and builds its stream without
    /// starting callbacks. This is the safe candidate phase used by atomic
    /// device switching: dropping the returned engine before a successful
    /// [`Self::start_device_stream`] destroys it directly on the control thread.
    pub fn prepare_with_profile(profile: &AudioDeviceProfile) -> Result<Self> {
        if profile.direction != AudioDeviceDirection::Output {
            return Err(anyhow!(
                "Audio playback requires an output device profile, not {:?}",
                profile.direction
            ));
        }
        let resolved = resolve_audio_device(profile).with_context(|| {
            "Unable to resolve the requested audio output. Check device availability and output permissions"
        })?;
        let sample_format = resolved.sample_format();
        let config = resolved.stream_config();
        let device_profile = resolved.negotiated.requested.clone();
        let effective_device_profile = resolved.negotiated.resolved_profile();
        let effective_stream_config = resolved.negotiated.effective;
        let device_name = resolved.name.clone();
        let device = resolved.device;

        let status = Arc::new(AudioStatus::default());
        let transport_mailbox = Arc::new(TransportMailbox::default());
        let callback_telemetry = Arc::new(CallbackTelemetry::default());
        status
            .sample_rate
            .store(config.sample_rate, Ordering::Relaxed);
        let (producer, consumer) = RingBuffer::new(COMMAND_CAPACITY);
        let (retired_producer, retired_assets) = RingBuffer::new(RETIRED_ASSET_CAPACITY);
        let (asset_event_producer, asset_events) = RingBuffer::new(ASSET_EVENT_CAPACITY);
        let (retired_endpoint_producer, retired_insert_endpoints) =
            RingBuffer::new(RETIRED_INSERT_ENDPOINT_CAPACITY);
        let (insert_endpoint_event_producer, insert_endpoint_events) =
            RingBuffer::new(INSERT_ENDPOINT_EVENT_CAPACITY);
        let (generator_endpoint_event_producer, generator_endpoint_events) =
            RingBuffer::new(GENERATOR_ENDPOINT_EVENT_CAPACITY);
        let (retired_midi_input_producer, retired_midi_inputs) =
            RingBuffer::new(RETIRED_MIDI_INPUT_CAPACITY);
        let (midi_input_route_event_producer, midi_input_route_events) =
            RingBuffer::new(MIDI_INPUT_ROUTE_EVENT_CAPACITY);
        let (midi_recording_endpoint_event_producer, midi_recording_endpoint_events) =
            RingBuffer::new(MIDI_RECORDING_ENDPOINT_EVENT_CAPACITY);
        let (parameter_edit_callback_event_producer, parameter_edit_callback_events) =
            RingBuffer::new(PARAMETER_EDIT_CALLBACK_EVENT_CAPACITY);
        let parameter_edit_callback_admission = Arc::new(AtomicU32::new(0));
        let (master_capture_event_producer, master_capture_events) =
            RingBuffer::new(MASTER_CAPTURE_EVENT_CAPACITY);
        let (timeline_runtime, realtime_timeline_runtime) = create_timeline_runtime();

        let (meter_publisher, meter_reader) = meter_channel();
        let stream = match sample_format {
            SampleFormat::F32 => build_stream::<f32>(
                &device,
                &config,
                consumer,
                retired_producer,
                asset_event_producer,
                retired_endpoint_producer,
                insert_endpoint_event_producer,
                generator_endpoint_event_producer,
                retired_midi_input_producer,
                midi_input_route_event_producer,
                midi_recording_endpoint_event_producer,
                parameter_edit_callback_event_producer,
                Arc::clone(&parameter_edit_callback_admission),
                master_capture_event_producer,
                realtime_timeline_runtime,
                status.clone(),
                Arc::clone(&transport_mailbox),
                Arc::clone(&callback_telemetry),
                meter_publisher,
            ),
            SampleFormat::I16 => build_stream::<i16>(
                &device,
                &config,
                consumer,
                retired_producer,
                asset_event_producer,
                retired_endpoint_producer,
                insert_endpoint_event_producer,
                generator_endpoint_event_producer,
                retired_midi_input_producer,
                midi_input_route_event_producer,
                midi_recording_endpoint_event_producer,
                parameter_edit_callback_event_producer,
                Arc::clone(&parameter_edit_callback_admission),
                master_capture_event_producer,
                realtime_timeline_runtime,
                status.clone(),
                Arc::clone(&transport_mailbox),
                Arc::clone(&callback_telemetry),
                meter_publisher,
            ),
            SampleFormat::U16 => build_stream::<u16>(
                &device,
                &config,
                consumer,
                retired_producer,
                asset_event_producer,
                retired_endpoint_producer,
                insert_endpoint_event_producer,
                generator_endpoint_event_producer,
                retired_midi_input_producer,
                midi_input_route_event_producer,
                midi_recording_endpoint_event_producer,
                parameter_edit_callback_event_producer,
                Arc::clone(&parameter_edit_callback_admission),
                master_capture_event_producer,
                realtime_timeline_runtime,
                status.clone(),
                Arc::clone(&transport_mailbox),
                Arc::clone(&callback_telemetry),
                meter_publisher,
            ),
            other => return Err(anyhow!("Unsupported output sample format: {other}")),
        }
        .with_context(|| {
            format!(
                "Unable to open audio output '{device_name}' ({} channels at {} Hz, {sample_format})",
                config.channels, config.sample_rate
            )
        })?;

        Ok(Self {
            stream: Some(stream),
            stream_lifecycle: DeviceStreamLifecycle::Prepared,
            meter_reader,
            timeline_runtime: Some(timeline_runtime),
            timeline_mixer_graph_fingerprint: None,
            producer,
            retired_assets,
            asset_events,
            retired_insert_endpoints,
            insert_endpoint_events,
            generator_endpoint_events,
            retired_midi_inputs,
            midi_input_route_events,
            midi_recording_endpoint_events,
            parameter_edit_callback_events,
            parameter_edit_callback_admission,
            master_capture_events,
            status,
            transport_mailbox,
            callback_telemetry,
            device_name,
            device_profile,
            effective_device_profile,
            effective_stream_config,
            has_realtime_owned_resources: false,
            timeline_runtime_shutdown_confirmed: false,
        })
    }

    /// Starts a stream returned by [`Self::prepare_with_profile`]. Repeated
    /// calls after a successful start are idempotent. A failed play attempt
    /// deliberately leaves the engine in `Prepared`, so normal destruction is
    /// safe and never waits for a callback that did not start.
    pub fn start_device_stream(&mut self) -> Result<()> {
        if self.stream_lifecycle == DeviceStreamLifecycle::Playing {
            return Ok(());
        }
        let play_result = self
            .stream
            .as_ref()
            .ok_or_else(|| anyhow!("The prepared audio device stream is unavailable"))?
            .play();
        self.stream_lifecycle
            .observe_play_result(play_result.is_ok());
        play_result.with_context(|| {
            format!(
                "Unable to start audio output '{}'. Check device availability and output permissions",
                self.device_name
            )
        })
    }

    /// The original flexible control-plane request used for this engine.
    pub fn device_profile(&self) -> &AudioDeviceProfile {
        &self.device_profile
    }

    /// Exact stable device/configuration selected by negotiation. This is
    /// suitable for persistence as a last-known-good profile.
    pub fn effective_device_profile(&self) -> &AudioDeviceProfile {
        &self.effective_device_profile
    }

    /// Allocation-free snapshot of raw backend callback sizes and classified
    /// stream faults.
    pub fn stream_telemetry(&self) -> AudioStreamTelemetrySnapshot {
        self.callback_telemetry.snapshot()
    }

    /// Measured post-fader, pre-output-protection stereo bus peaks. No transport
    /// playing check: paused live MIDI monitoring can genuinely be audible.
    pub fn poll_meters(
        &mut self,
        now: Instant,
        expected_graph: Option<(u64, u64)>,
    ) -> MeterReadings {
        let identity = if self.stream_telemetry().last_error_kind.invalidates_stream() {
            None
        } else {
            self.confirmed_timeline_identity()
                .and_then(|(revision, epoch)| {
                    expected_graph
                        .filter(|(generation, _)| *generation == revision)
                        .map(|(_, graph_fingerprint)| MeterIdentity {
                            revision,
                            epoch,
                            graph_fingerprint,
                        })
                })
        };
        self.meter_reader.poll(
            now,
            identity,
            self.status.device_frame.load(Ordering::Acquire),
            self.status.sample_rate.load(Ordering::Relaxed),
        )
    }

    pub fn reset_meter(&mut self, runtime_slot: u8, id: crate::mixer_graph::MixerTrackId) {
        self.meter_reader.reset_track(runtime_slot, id);
    }

    /// Transfers one immutable control-thread compilation to the callback. The
    /// generation is used verbatim as the runtime revision and must be non-zero.
    /// Enqueue success is not activation; wait for the matching
    /// [`TimelineRuntimeEvent::Installed`] confirmation.
    pub fn install_compiled_timeline(
        &mut self,
        generation: u64,
        timeline: Arc<CompiledTimeline>,
    ) -> Result<u64, TimelineResourceQueueError<Arc<CompiledTimeline>>> {
        let mixer_graph_fingerprint = timeline.mixer_graph().fingerprint();
        if self.timeline_mixer_graph_fingerprint == Some(mixer_graph_fingerprint) {
            let result = self.timeline_runtime_mut().install_reusing_mixer_resources(
                generation,
                timeline,
                mixer_graph_fingerprint,
            );
            if result.is_ok() {
                self.timeline_mixer_graph_fingerprint = Some(mixer_graph_fingerprint);
            }
            return result;
        }
        let mixer_delay_bank = match PreparedMixerGraphDelayBank::new(
            timeline.mixer_graph(),
            PDC_DEFAULT_MAX_DELAY_SAMPLES,
        ) {
            Ok(bank) => Box::new(bank),
            Err(_) => {
                return Err(TimelineResourceQueueError {
                    reason: TimelineResourceQueueFailure::Validation(
                        TimelineRuntimeValidationError::MixerGraphDelayResourcesUnavailable,
                    ),
                    resource: timeline,
                });
            }
        };
        match self.timeline_runtime_mut().install_with_mixer_resources(
            generation,
            timeline,
            mixer_delay_bank,
        ) {
            Ok(request_id) => {
                self.timeline_mixer_graph_fingerprint = Some(mixer_graph_fingerprint);
                Ok(request_id)
            }
            Err(error) => {
                let (timeline, mixer_delay_bank) = error.resource;
                // Queue rejection returns ownership to this control-thread API.
                // Destroy the potentially large sample bank here, never in the
                // callback or through an error path visible to App.
                drop(mixer_delay_bank);
                Err(TimelineResourceQueueError {
                    reason: error.reason,
                    resource: timeline,
                })
            }
        }
    }

    /// Builds discontinuity state off the callback thread. Call this while the
    /// control side retains its `Arc`; installing a shallow Arc clone does not
    /// copy the compiled event/vector storage.
    pub fn prepare_timeline_chase(
        &self,
        timeline: &CompiledTimeline,
        generation: u64,
        epoch: u64,
        frame: u64,
        options: TimelineChaseOptions,
    ) -> Result<Box<PreparedTimelineChase>, TimelinePrepareChaseError> {
        self.timeline_runtime_ref()
            .prepare_chase(timeline, generation, epoch, frame, options)
    }

    /// Queues already prepared discontinuity state. It becomes active only after
    /// a matching [`TimelineRuntimeEvent::ChaseInstalled`] confirmation.
    pub fn install_timeline_chase(
        &mut self,
        chase: Box<PreparedTimelineChase>,
    ) -> Result<u64, TimelineResourceQueueError<Box<PreparedTimelineChase>>> {
        self.timeline_runtime_mut().install_chase(chase)
    }

    /// Builds reusable loop-start chase state off the callback thread. The
    /// callback assigns the exact activation token when this state is installed.
    pub fn prepare_timeline_loop_chase(
        &self,
        timeline: &CompiledTimeline,
        generation: u64,
        frame: u64,
        options: TimelineChaseOptions,
    ) -> Result<Box<PreparedLoopTimelineChase>, TimelinePrepareChaseError> {
        self.timeline_runtime_ref()
            .prepare_loop_chase(timeline, generation, frame, options)
    }

    /// Queues a reusable loop-start chase template. Wait for the matching
    /// [`TimelineRuntimeEvent::LoopChaseInstalled`] and its exact token before
    /// enabling the transport loop.
    pub fn install_timeline_loop_chase(
        &mut self,
        chase: Box<PreparedLoopTimelineChase>,
    ) -> Result<u64, TimelineResourceQueueError<Box<PreparedLoopTimelineChase>>> {
        self.timeline_runtime_mut().install_loop_chase(chase)
    }

    /// Queues one complete callback transaction for timeline identity, cursor,
    /// loop and playing state. The returned id is complete only after an exact
    /// `TransportActivationApplied` receipt.
    pub fn activate_timeline_transport(
        &mut self,
        spec: TimelineTransportActivationSpec,
    ) -> Result<u64, TimelineControlError> {
        let legacy_transport_barrier = self.transport_mailbox.control_thread_request_id();
        self.timeline_runtime_mut()
            .activate_transport(spec, legacy_transport_barrier)
    }

    /// Requests removal of exactly `generation`; stale generations are rejected
    /// by the callback and reported through [`Self::next_timeline_runtime_event`].
    pub fn clear_compiled_timeline(
        &mut self,
        generation: u64,
    ) -> Result<u64, TimelineControlError> {
        self.timeline_runtime_mut().clear(generation)
    }

    /// Pops one callback confirmation without blocking.
    pub fn next_timeline_runtime_event(&mut self) -> Option<TimelineRuntimeEvent> {
        let event = self.timeline_runtime_mut().poll_event()?;
        if matches!(
            event,
            TimelineRuntimeEvent::Cleared { .. }
                | TimelineRuntimeEvent::Rejected { .. }
                | TimelineRuntimeEvent::ShutdownComplete { .. }
        ) {
            self.timeline_mixer_graph_fingerprint = None;
        }
        Some(event)
    }

    /// Returns callback-retired ownership to the control thread. Dropping the
    /// returned value here (or handing it to another non-realtime owner) is safe.
    pub fn next_retired_timeline_resource(&mut self) -> Option<RetiredTimelineResource> {
        self.timeline_runtime_mut().poll_retired()
    }

    /// Destroys every callback-retired timeline/chase on this control thread.
    pub fn reclaim_retired_timeline_resources(&mut self) -> usize {
        self.timeline_runtime_mut().drain_retired()
    }

    #[must_use]
    pub fn timeline_runtime_stats(&self) -> TimelineRuntimeStats {
        self.timeline_runtime_ref().stats()
    }

    #[must_use]
    pub fn confirmed_timeline_revision(&self) -> Option<u64> {
        self.timeline_runtime_ref().confirmed_revision()
    }

    /// Newest callback-resident revision. During a seamless replacement this
    /// is the candidate while `confirmed_timeline_revision` remains the older
    /// revision that is still rendering until one-shot activation.
    #[must_use]
    pub fn resident_timeline_revision(&self) -> Option<u64> {
        self.timeline_runtime_ref().resident_revision()
    }

    #[must_use]
    pub fn confirmed_timeline_epoch(&self) -> Option<u64> {
        self.timeline_runtime_ref().confirmed_epoch()
    }

    #[must_use]
    pub fn confirmed_timeline_identity(&self) -> Option<(u64, u64)> {
        self.timeline_runtime_ref().confirmed_identity_pair()
    }

    #[must_use]
    pub fn timeline_runtime_is_synchronized(&self) -> bool {
        self.timeline_runtime_ref().is_synchronized()
    }

    #[must_use]
    pub fn timeline_runtime_needs_resync(&self) -> bool {
        self.timeline_runtime_ref().needs_resync()
    }

    pub fn acknowledge_timeline_resync(&mut self, generation: u64, epoch: u64) -> bool {
        self.timeline_runtime_mut()
            .acknowledge_resync(generation, epoch)
    }

    fn timeline_runtime_ref(&self) -> &TimelineRuntimeController {
        self.timeline_runtime
            .as_ref()
            .expect("timeline runtime is available until AudioEngine teardown")
    }

    fn timeline_runtime_mut(&mut self) -> &mut TimelineRuntimeController {
        self.timeline_runtime
            .as_mut()
            .expect("timeline runtime is available until AudioEngine teardown")
    }

    /// Queues a command without blocking. `false` means the bounded queue was full.
    /// Any rejected command (including a large asset Arc) is released here on the
    /// caller/UI thread rather than by the real-time callback.
    pub fn command(&mut self, command: AudioCommand) -> bool {
        self.reclaim_retired_assets();
        self.reclaim_retired_insert_endpoints();
        self.reclaim_retired_midi_inputs();
        let reserves_parameter_edit_callback =
            matches!(&command, AudioCommand::EditPluginParameter(_));
        if reserves_parameter_edit_callback
            && !try_admit_callback_parameter_edit(&self.parameter_edit_callback_admission)
        {
            return false;
        }
        let transfers_ownership = matches!(
            &command,
            AudioCommand::RegisterAsset { .. }
                | AudioCommand::InstallInsertEndpoint { .. }
                | AudioCommand::InstallGeneratorEndpoint { .. }
                | AudioCommand::InstallMidiInput { .. }
                | AudioCommand::StartMidiRecording { .. }
                | AudioCommand::InstallMasterCapture { .. }
        );
        let queued = queue_audio_command(&mut self.producer, &self.status, command);
        if !queued && reserves_parameter_edit_callback {
            release_callback_parameter_edit(&self.parameter_edit_callback_admission);
        }
        if queued && transfers_ownership {
            self.has_realtime_owned_resources = true;
        }
        queued
    }

    /// Registers immutable, normalized interleaved samples for later clip playback.
    pub fn register_asset(
        &mut self,
        operation: AudioAssetOperation,
        id: u64,
        samples: Arc<[f32]>,
        sample_rate: u32,
        channels: u16,
    ) -> bool {
        if !asset_layout_is_valid(&samples, sample_rate, channels) {
            return false;
        }
        self.command(AudioCommand::RegisterAsset {
            operation,
            id,
            samples,
            sample_rate,
            channels,
        })
    }

    /// Queues removal of one asset. Completion is reported by `next_asset_event`.
    #[allow(dead_code)]
    pub fn unregister_asset(&mut self, operation: AudioAssetOperation, id: u64) -> bool {
        self.command(AudioCommand::UnregisterAsset { operation, id })
    }

    /// Queues removal of every asset and stops all audio clip voices. Completion
    /// is reported by `next_asset_event`.
    pub fn clear_assets(&mut self, operation: AudioAssetOperation) -> bool {
        self.command(AudioCommand::ClearAssets { operation })
    }

    pub fn play_clip(
        &mut self,
        clip_id: u64,
        asset_id: u64,
        source_frame: f64,
        gain: f32,
        mixer_track: usize,
    ) -> bool {
        self.command(AudioCommand::PlayClip {
            clip_id,
            asset_id,
            source_frame,
            gain,
            mixer_track,
        })
    }

    pub fn sync_clip(
        &mut self,
        clip_id: u64,
        asset_id: u64,
        source_frame: f64,
        gain: f32,
        mixer_track: usize,
    ) -> bool {
        self.command(AudioCommand::SyncClip {
            clip_id,
            asset_id,
            source_frame,
            gain,
            mixer_track,
        })
    }

    pub fn stop_clip(&mut self, clip_id: u64) -> bool {
        self.command(AudioCommand::StopClip { clip_id })
    }

    /// Drops asset buffers handed back by the callback. Callers normally get this
    /// automatically whenever they queue a command.
    pub fn reclaim_retired_assets(&mut self) {
        while let Ok(_asset) = self.retired_assets.pop() {}
    }

    /// Pops one callback-confirmed asset lifecycle result without blocking.
    pub fn next_asset_event(&mut self) -> Option<AudioAssetEvent> {
        self.reclaim_retired_assets();
        self.asset_events.pop().ok()
    }

    /// Queues one prepared worker endpoint for an insert. `true` only means the
    /// command entered the queue; wait for [`InsertEndpointEvent::Installed`].
    #[allow(dead_code)]
    pub fn install_insert_endpoint(
        &mut self,
        insert: usize,
        endpoint_id: u64,
        endpoint: AudioThreadEndpoint,
    ) -> bool {
        self.install_insert_endpoint_inner(0, insert, endpoint_id, endpoint)
    }

    /// Project-stamped production endpoint installation used by exact generic edits.
    pub fn install_insert_endpoint_for_project(
        &mut self,
        project_session: u64,
        insert: usize,
        endpoint_id: u64,
        endpoint: AudioThreadEndpoint,
    ) -> bool {
        if project_session == 0 {
            return false;
        }
        self.install_insert_endpoint_inner(project_session, insert, endpoint_id, endpoint)
    }

    fn install_insert_endpoint_inner(
        &mut self,
        project_session: u64,
        insert: usize,
        endpoint_id: u64,
        endpoint: AudioThreadEndpoint,
    ) -> bool {
        if insert >= TRACK_COUNT
            || endpoint_id == 0
            || endpoint.max_block_frames() < DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES
        {
            return false;
        }
        let Ok(mut endpoint) = PreparedFixedEndpoint::new(endpoint) else {
            return false;
        };
        endpoint.project_session = project_session;
        self.command(AudioCommand::InstallInsertEndpoint {
            insert,
            endpoint_id,
            endpoint,
        })
    }

    #[allow(dead_code)]
    pub fn remove_insert_endpoint(&mut self, insert: usize) -> bool {
        if insert >= TRACK_COUNT {
            return false;
        }
        self.command(AudioCommand::RemoveInsertEndpoint { insert })
    }

    #[allow(dead_code)]
    pub fn clear_insert_endpoints(&mut self) -> bool {
        self.command(AudioCommand::ClearInsertEndpoints { request_id: 0 })
    }

    #[allow(dead_code)]
    pub fn send_insert_midi(
        &mut self,
        insert: usize,
        slot: Option<usize>,
        data: [u8; 3],
        sample_offset: usize,
    ) -> bool {
        if insert >= TRACK_COUNT
            || slot.is_some_and(|slot| slot >= MAX_INSERT_PLUGIN_SLOTS)
            || sample_offset > u16::MAX as usize
        {
            return false;
        }
        self.command(AudioCommand::SendInsertMidi {
            insert,
            slot,
            data,
            sample_offset,
        })
    }

    #[allow(dead_code)]
    pub fn set_insert_parameter(
        &mut self,
        insert: usize,
        slot: usize,
        id: u32,
        normalized: f32,
    ) -> bool {
        if insert >= TRACK_COUNT || slot >= MAX_INSERT_PLUGIN_SLOTS || !normalized.is_finite() {
            return false;
        }
        self.command(AudioCommand::SetInsertParameter {
            insert,
            slot,
            id,
            normalized: normalized.clamp(0.0, 1.0),
        })
    }

    /// Drops callback-retired endpoint handles on this non-real-time thread.
    pub fn reclaim_retired_insert_endpoints(&mut self) {
        while let Ok(_endpoint) = self.retired_insert_endpoints.pop() {}
    }

    #[allow(dead_code)]
    pub fn next_insert_endpoint_event(&mut self) -> Option<InsertEndpointEvent> {
        self.reclaim_retired_insert_endpoints();
        self.insert_endpoint_events.pop().ok()
    }

    /// Installs one worker-backed Channel instrument. Channel IDs and endpoint
    /// IDs are project/runtime stable identifiers; `true` only confirms enqueue.
    /// The bridge adds at least one submitted block of latency; its control-side
    /// `BridgeStats` is the authoritative latency/tail measurement.
    #[allow(dead_code)]
    pub fn install_generator_endpoint(
        &mut self,
        channel_id: u32,
        endpoint_id: u64,
        plugin_instance_id: u64,
        mixer_track: usize,
        endpoint: AudioThreadEndpoint,
    ) -> bool {
        self.install_generator_endpoint_inner(
            0,
            channel_id,
            endpoint_id,
            plugin_instance_id,
            mixer_track,
            endpoint,
        )
    }

    /// Project-stamped production generator installation used by exact generic edits.
    #[allow(clippy::too_many_arguments)]
    pub fn install_generator_endpoint_for_project(
        &mut self,
        project_session: u64,
        channel_id: u32,
        endpoint_id: u64,
        plugin_instance_id: u64,
        mixer_track: usize,
        endpoint: AudioThreadEndpoint,
    ) -> bool {
        if project_session == 0 {
            return false;
        }
        self.install_generator_endpoint_inner(
            project_session,
            channel_id,
            endpoint_id,
            plugin_instance_id,
            mixer_track,
            endpoint,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn install_generator_endpoint_inner(
        &mut self,
        project_session: u64,
        channel_id: u32,
        endpoint_id: u64,
        plugin_instance_id: u64,
        mixer_track: usize,
        endpoint: AudioThreadEndpoint,
    ) -> bool {
        if endpoint_id == 0
            || plugin_instance_id == 0
            || mixer_track >= TRACK_COUNT
            || endpoint.max_block_frames() < DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES
        {
            return false;
        }
        let Ok(mut endpoint) = PreparedFixedEndpoint::new(endpoint) else {
            return false;
        };
        endpoint.project_session = project_session;
        let Ok(pdc_delay) = StereoDelayLine::new(PDC_DEFAULT_MAX_DELAY_SAMPLES) else {
            return false;
        };
        self.command(AudioCommand::InstallGeneratorEndpoint {
            channel_id,
            endpoint_id,
            plugin_instance_id,
            mixer_track,
            endpoint,
            pdc_delay,
        })
    }

    #[allow(dead_code)]
    pub fn remove_generator_endpoint(&mut self, channel_id: u32) -> bool {
        self.command(AudioCommand::RemoveGeneratorEndpoint { channel_id })
    }

    #[allow(dead_code)]
    pub fn clear_generator_endpoints(&mut self) -> bool {
        self.command(AudioCommand::ClearGeneratorEndpoints { request_id: 0 })
    }

    #[allow(dead_code)]
    pub fn set_generator_route(&mut self, channel_id: u32, mixer_track: usize) -> bool {
        if mixer_track >= TRACK_COUNT {
            return false;
        }
        self.command(AudioCommand::SetGeneratorRoute {
            channel_id,
            mixer_track,
        })
    }

    #[allow(dead_code)]
    pub fn send_generator_midi(
        &mut self,
        channel_id: u32,
        slot: Option<usize>,
        data: [u8; 3],
        sample_offset: usize,
    ) -> bool {
        if slot.is_some_and(|slot| slot >= MAX_INSERT_PLUGIN_SLOTS)
            || sample_offset > u16::MAX as usize
        {
            return false;
        }
        self.command(AudioCommand::SendGeneratorMidi {
            channel_id,
            slot,
            data,
            sample_offset,
        })
    }

    #[allow(dead_code)]
    pub fn set_generator_parameter(
        &mut self,
        channel_id: u32,
        slot: usize,
        id: u32,
        normalized: f32,
    ) -> bool {
        if slot >= MAX_INSERT_PLUGIN_SLOTS || !normalized.is_finite() {
            return false;
        }
        self.command(AudioCommand::SetGeneratorParameter {
            channel_id,
            slot,
            id,
            normalized: normalized.clamp(0.0, 1.0),
        })
    }

    #[allow(dead_code)]
    pub fn next_generator_endpoint_event(&mut self) -> Option<GeneratorEndpointEvent> {
        self.reclaim_retired_insert_endpoints();
        self.generator_endpoint_events.pop().ok()
    }

    /// Transfers the sole consumer of one OS MIDI callback's dedicated SPSC to the
    /// audio callback. Enqueue is not activation; wait for the exact Installed receipt.
    pub fn install_midi_input(
        &mut self,
        route_id: u64,
        prepared: PreparedMidiInputRoute,
    ) -> Result<(), PreparedMidiInputRoute> {
        self.reclaim_retired_midi_inputs();
        if route_id == 0 {
            return Err(prepared);
        }
        match self
            .producer
            .push(AudioCommand::InstallMidiInput { route_id, prepared })
        {
            Ok(()) => {
                self.has_realtime_owned_resources = true;
                Ok(())
            }
            Err(PushError::Full(AudioCommand::InstallMidiInput { prepared, .. })) => {
                self.status
                    .command_queue_full
                    .fetch_add(1, Ordering::Relaxed);
                Err(prepared)
            }
            Err(PushError::Full(_)) => unreachable!("the rejected command preserves its variant"),
        }
    }

    pub fn remove_midi_input(&mut self, route_id: u64) -> bool {
        route_id != 0 && self.command(AudioCommand::RemoveMidiInput { route_id })
    }

    /// Drops detached MIDI receiver ownership only on this non-realtime thread.
    pub fn reclaim_retired_midi_inputs(&mut self) {
        while let Ok(_receiver) = self.retired_midi_inputs.pop() {}
    }

    pub fn next_midi_input_route_event(&mut self) -> Option<MidiInputRouteEvent> {
        self.reclaim_retired_midi_inputs();
        self.midi_input_route_events.pop().ok()
    }

    /// Transfers one prepared live-MIDI take producer to the callback. Queue admission is not a
    /// recording start; only the exact [`MidiRecordingEndpointEvent::Started`] receipt is.
    #[allow(clippy::result_large_err)]
    pub fn start_midi_recording(
        &mut self,
        endpoint: PreparedMidiRecordEndpoint,
    ) -> Result<(), PreparedMidiRecordEndpoint> {
        if !endpoint.stamp().is_valid() {
            return Err(endpoint);
        }
        match self
            .producer
            .push(AudioCommand::StartMidiRecording { endpoint })
        {
            Ok(()) => {
                self.has_realtime_owned_resources = true;
                Ok(())
            }
            Err(PushError::Full(AudioCommand::StartMidiRecording { endpoint })) => {
                self.status
                    .command_queue_full
                    .fetch_add(1, Ordering::Relaxed);
                Err(endpoint)
            }
            Err(PushError::Full(_)) => unreachable!("the rejected command preserves its variant"),
        }
    }

    /// Requests an exact callback-boundary stop. The caller must retain its control half until a
    /// matching Stopped receipt returns producer ownership.
    pub fn stop_midi_recording(&mut self, session_id: u64) -> bool {
        session_id != 0 && self.command(AudioCommand::StopMidiRecording { session_id })
    }

    /// Lifecycle barrier used by project/audio-engine teardown. Any active or pending endpoint is
    /// returned in the matching Cleared receipt.
    pub fn clear_midi_recording(&mut self, request_id: u64) -> bool {
        request_id != 0 && self.command(AudioCommand::ClearMidiRecording { request_id })
    }

    pub fn next_midi_recording_event(&mut self) -> Option<MidiRecordingEndpointEvent> {
        self.midi_recording_endpoint_events.pop().ok()
    }

    /// Queue one exact generic edit without blocking. `true` means only that the command entered
    /// the callback queue; the caller must then wait for callback rejection or worker receipt.
    pub fn edit_plugin_parameter(&mut self, submission: ParameterEditSubmission) -> bool {
        self.command(AudioCommand::EditPluginParameter(submission))
    }

    /// Pops one exact callback-side rejection. Successfully admitted edits terminate only on the
    /// owning worker control's reliable receipt ring and never appear here.
    pub fn next_plugin_parameter_edit_callback_receipt(&mut self) -> Option<CallbackEditReceipt> {
        let receipt = self.parameter_edit_callback_events.pop().ok()?;
        release_callback_parameter_edit(&self.parameter_edit_callback_admission);
        Some(receipt)
    }

    /// Queues a callback-owned stereo master tap. The capture is active only
    /// after a matching [`MasterCaptureEndpointEvent::Installed`] arrives.
    pub fn install_master_capture(
        &mut self,
        capture_id: u64,
        endpoint: MasterCaptureEndpoint,
    ) -> bool {
        if capture_id == 0 || endpoint.session_id() != capture_id {
            return false;
        }
        self.command(AudioCommand::InstallMasterCapture {
            capture_id,
            endpoint,
        })
    }

    /// Requests detachment at the next callback boundary. The returned endpoint
    /// in [`MasterCaptureEndpointEvent::Stopped`] must be passed to its control.
    pub fn stop_master_capture(&mut self, capture_id: u64) -> bool {
        capture_id != 0 && self.command(AudioCommand::StopMasterCapture { capture_id })
    }

    pub fn next_master_capture_event(&mut self) -> Option<MasterCaptureEndpointEvent> {
        self.master_capture_events.pop().ok()
    }

    pub fn set_playing(&self, playing: bool) {
        self.transport_mailbox
            .publish(TransportMutation::SetPlaying(playing));
    }

    /// Requests a transport stop that cannot be crowded out by `AudioCommand`.
    /// The callback clears voices and applies the request at its next mixer chunk.
    pub fn stop_transport(&self) -> u64 {
        self.transport_mailbox
            .publish(TransportMutation::Discontinuity {
                target_beat_q32: 0,
                target_timeline_frame: 0,
                playing: Some(false),
            })
    }

    /// Legacy constant-tempo seek retained for compatibility.
    ///
    /// Callers with a tempo map should use [`Self::seek_transport_to_frame`] so
    /// the callback receives the authoritative precomputed timeline frame.
    pub fn seek_transport(&self, beat: f64) -> u64 {
        let target_beat_q32 = beat_to_q32(beat);
        let target_timeline_frame = timeline_frame_for_beat(
            target_beat_q32,
            self.status.tempo_milli.load(Ordering::Relaxed),
            self.status.sample_rate.load(Ordering::Relaxed),
        );
        self.transport_mailbox
            .publish(TransportMutation::Discontinuity {
                target_beat_q32,
                target_timeline_frame,
                playing: None,
            })
    }

    /// Requests an exact sample-clock seek while preserving play/pause state.
    /// `beat` is a compatibility/UI hint; `timeline_frame` is authoritative.
    pub fn seek_transport_to_frame(&self, beat: f64, timeline_frame: u64) -> u64 {
        self.transport_mailbox
            .publish(TransportMutation::Discontinuity {
                target_beat_q32: beat_to_q32(beat),
                target_timeline_frame: timeline_frame,
                playing: None,
            })
    }

    /// Legacy constant-tempo loop API retained for compatibility.
    ///
    /// Callers with a tempo map should use [`Self::set_transport_loop_frames`].
    pub fn set_transport_loop(&self, start_beat: f64, end_beat: f64, enabled: bool) -> u64 {
        let start_q32 = beat_to_q32(start_beat);
        let end_q32 = beat_to_q32(end_beat);
        let tempo_milli = self.status.tempo_milli.load(Ordering::Relaxed);
        let sample_rate = self.status.sample_rate.load(Ordering::Relaxed);
        let start_frame = timeline_frame_for_beat(start_q32, tempo_milli, sample_rate);
        let end_frame = timeline_frame_for_beat(end_q32, tempo_milli, sample_rate);
        self.transport_mailbox.publish(TransportMutation::SetLoop {
            enabled,
            start_q32,
            end_q32,
            start_frame,
            end_frame,
        })
    }

    /// Atomically publishes a loop's UI beat hints and authoritative half-open
    /// frame range `[start_frame, end_frame)`. Invalid frame ranges are disabled.
    pub fn set_transport_loop_frames(
        &self,
        start_beat: f64,
        start_frame: u64,
        end_beat: f64,
        end_frame: u64,
        enabled: bool,
    ) -> u64 {
        self.transport_mailbox.publish(TransportMutation::SetLoop {
            enabled,
            start_q32: beat_to_q32(start_beat),
            end_q32: beat_to_q32(end_beat),
            start_frame,
            end_frame,
        })
    }

    pub fn set_recording(&self, recording: bool) {
        self.status.recording.store(recording, Ordering::Release);
    }

    pub fn set_tempo(&self, tempo: f32) {
        self.status.tempo_milli.store(
            (tempo.clamp(20.0, 400.0) * 1000.0) as u32,
            Ordering::Relaxed,
        );
    }

    pub fn snapshot(&self) -> AudioSnapshot {
        let transport = self.status.transport_fields();
        let pdc = self.status.pdc_fields();
        let telemetry = self.stream_telemetry();
        let requested_buffer_size = self.device_profile.buffer_size;
        let effective_buffer_size = self.effective_stream_config.buffer_size;
        let actual_buffer_size = telemetry.last_frames;
        AudioSnapshot {
            device: self.device_name.clone(),
            sample_rate: self.status.sample_rate.load(Ordering::Relaxed),
            requested_buffer_size,
            effective_buffer_size,
            actual_buffer_size,
            rendered_frames: transport.device_frame,
            device_frame: transport.device_frame,
            timeline_frame: transport.timeline_frame,
            beat_q32: transport.beat_q32,
            beat_position: q32_to_beat(transport.beat_q32),
            transport_epoch: transport.epoch,
            transport_loop_count: transport.loop_count,
            applied_transport_request: transport.applied_request,
            transport_playing: transport.playing,
            plugin_epoch_resets: self.status.plugin_epoch_resets.load(Ordering::Relaxed),
            plugin_epoch_reset_failures: self
                .status
                .plugin_epoch_reset_failures
                .load(Ordering::Relaxed),
            last_plugin_endpoint_epoch: self
                .status
                .last_plugin_endpoint_epoch
                .load(Ordering::Relaxed),
            plugin_fixed_quantum_frames: self
                .status
                .plugin_fixed_quantum_frames
                .load(Ordering::Relaxed),
            plugin_fixed_quantum_event_overflows: self
                .status
                .plugin_fixed_quantum_event_overflows
                .load(Ordering::Relaxed),
            plugin_fixed_quantum_invalid_events: self
                .status
                .plugin_fixed_quantum_invalid_events
                .load(Ordering::Relaxed),
            plugin_fixed_quantum_event_rejections: self
                .status
                .plugin_fixed_quantum_event_rejections
                .load(Ordering::Relaxed),
            plugin_fixed_quantum_bridge_gaps: self
                .status
                .plugin_fixed_quantum_bridge_gaps
                .load(Ordering::Relaxed),
            plugin_fixed_quantum_output_underflow_frames: self
                .status
                .plugin_fixed_quantum_output_underflow_frames
                .load(Ordering::Relaxed),
            timeline_automation_pending: self
                .status
                .timeline_automation_pending
                .load(Ordering::Relaxed),
            timeline_automation_unsupported: self
                .status
                .timeline_automation_unsupported
                .load(Ordering::Relaxed),
            timeline_execution_failures: self
                .status
                .timeline_execution_failures
                .load(Ordering::Relaxed),
            timeline_missing_assets: self.status.timeline_missing_assets.load(Ordering::Relaxed),
            pdc_plan_revision: pdc.plan_revision,
            pdc_reference_latency_samples: pdc.reference_latency_samples,
            pdc_master_latency_samples: pdc.master_latency_samples,
            pdc_output_latency_samples: pdc.output_latency_samples,
            pdc_clamped_path_count: pdc.clamped_path_count,
            pdc_maximum_delay_samples: pdc.maximum_delay_samples,
            xruns: self.status.xruns.load(Ordering::Relaxed),
            command_queue_full: self.status.command_queue_full.load(Ordering::Relaxed),
        }
    }

    fn shutdown_realtime_resources(&mut self, timeout: Duration) -> bool {
        // Queue all ownership barriers while the callback is still alive. The
        // final master-capture marker acknowledges every earlier audio command;
        // the independent timeline shutdown confirmation guarantees that its
        // Boxes and Vecs have reached the control-thread retire queue.
        let deadline = Instant::now() + timeout;
        let audio_shutdown_required = self.has_realtime_owned_resources;
        let mut assets_clear_queued = false;
        let mut insert_clear_queued = false;
        let mut generator_clear_queued = false;
        let mut midi_recording_clear_queued = false;
        let mut midi_input_clear_queued = false;
        let mut capture_clear_queued = false;
        let mut capture_clear_confirmed = !audio_shutdown_required;
        let mut timeline_shutdown_request = None;
        let mut timeline_shutdown_confirmed = self.timeline_runtime_shutdown_confirmed;
        while Instant::now() < deadline {
            self.reclaim_retired_assets();
            self.reclaim_retired_insert_endpoints();
            self.reclaim_retired_midi_inputs();
            while self.asset_events.pop().is_ok() {}
            while self.insert_endpoint_events.pop().is_ok() {}
            while let Ok(event) = self.generator_endpoint_events.pop() {
                let _ = event;
            }
            while self.midi_input_route_events.pop().is_ok() {}
            while let Ok(_event) = self.midi_recording_endpoint_events.pop() {}
            while let Ok(event) = self.master_capture_events.pop() {
                if matches!(
                    event,
                    MasterCaptureEndpointEvent::Cleared {
                        request_id: ENDPOINT_SHUTDOWN_REQUEST_ID,
                        ..
                    }
                ) {
                    capture_clear_confirmed = true;
                }
            }

            if !timeline_shutdown_confirmed {
                let Some(controller) = self.timeline_runtime.as_mut() else {
                    return false;
                };
                if timeline_shutdown_request.is_none() {
                    match controller.request_shutdown() {
                        Ok(request_id) => timeline_shutdown_request = Some(request_id),
                        Err(TimelineControlError::CommandQueueFull) => {}
                        Err(_) => return false,
                    }
                }
                while let Some(event) = controller.poll_event() {
                    if matches!(
                        event,
                        TimelineRuntimeEvent::ShutdownComplete { request_id, .. }
                            if Some(request_id) == timeline_shutdown_request
                    ) {
                        timeline_shutdown_confirmed = true;
                    }
                }
                controller.drain_retired();
            }

            if audio_shutdown_required {
                if !assets_clear_queued {
                    assets_clear_queued = self
                        .producer
                        .push(AudioCommand::ClearAssets {
                            operation: AudioAssetOperation::SHUTDOWN,
                        })
                        .is_ok();
                } else if !insert_clear_queued {
                    insert_clear_queued = self
                        .producer
                        .push(AudioCommand::ClearInsertEndpoints {
                            request_id: ENDPOINT_SHUTDOWN_REQUEST_ID,
                        })
                        .is_ok();
                } else if !generator_clear_queued {
                    generator_clear_queued = self
                        .producer
                        .push(AudioCommand::ClearGeneratorEndpoints {
                            request_id: ENDPOINT_SHUTDOWN_REQUEST_ID,
                        })
                        .is_ok();
                } else if !midi_recording_clear_queued {
                    midi_recording_clear_queued = self
                        .producer
                        .push(AudioCommand::ClearMidiRecording {
                            request_id: ENDPOINT_SHUTDOWN_REQUEST_ID,
                        })
                        .is_ok();
                } else if !midi_input_clear_queued {
                    midi_input_clear_queued = self
                        .producer
                        .push(AudioCommand::ClearMidiInput {
                            request_id: ENDPOINT_SHUTDOWN_REQUEST_ID,
                        })
                        .is_ok();
                } else if !capture_clear_queued {
                    capture_clear_queued = self
                        .producer
                        .push(AudioCommand::ClearMasterCapture {
                            request_id: ENDPOINT_SHUTDOWN_REQUEST_ID,
                        })
                        .is_ok();
                }
            }

            if capture_clear_confirmed && timeline_shutdown_confirmed {
                self.reclaim_retired_assets();
                self.reclaim_retired_insert_endpoints();
                self.reclaim_retired_midi_inputs();
                if let Some(controller) = self.timeline_runtime.as_mut() {
                    controller.drain_retired();
                }
                self.has_realtime_owned_resources = false;
                self.timeline_runtime_shutdown_confirmed = true;
                return true;
            }
            thread::yield_now();
        }
        false
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        if !self.stream_lifecycle.requires_callback_shutdown() {
            // No callback ever acquired these resources. Dropping the CPAL
            // stream destroys its closure, realtime timeline endpoint, queued
            // commands, and the matching controller synchronously here. In
            // particular, a prepared device-switch candidate and a failed
            // `play()` attempt must never enter the two-second callback barrier
            // or leak through the fail-safe `forget` path below.
            drop(self.stream.take());
            drop(self.timeline_runtime.take());
            return;
        }
        if !self.has_realtime_owned_resources && self.timeline_runtime_shutdown_confirmed {
            return;
        }
        if self.shutdown_realtime_resources(Duration::from_secs(2)) {
            return;
        }

        // If the backend stopped invoking callbacks, destroying its closure could
        // run queued endpoint destructors on an undocumented backend thread. Keep
        // the inert stream ownership alive instead; process teardown can reclaim it.
        if let Some(stream) = self.stream.take() {
            std::mem::forget(stream);
        }
        if let Some(controller) = self.timeline_runtime.take() {
            std::mem::forget(controller);
        }
    }
}

pub fn output_devices() -> Vec<String> {
    let host = cpal::default_host();
    let Ok(devices) = host.output_devices() else {
        return Vec::new();
    };
    let mut names = devices
        .filter_map(|device| {
            device
                .description()
                .ok()
                .map(|description| description.name().to_owned())
        })
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    names
}

fn queue_audio_command(
    producer: &mut Producer<AudioCommand>,
    status: &AudioStatus,
    command: AudioCommand,
) -> bool {
    match producer.push(command) {
        Ok(()) => true,
        Err(PushError::Full(_rejected)) => {
            status.command_queue_full.fetch_add(1, Ordering::Relaxed);
            false
        }
    }
}

fn try_admit_callback_parameter_edit(admission: &AtomicU32) -> bool {
    let mut current = admission.load(Ordering::Acquire);
    loop {
        if current >= PARAMETER_EDIT_CALLBACK_ADMISSION_CAPACITY {
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

fn release_callback_parameter_edit(admission: &AtomicU32) {
    let previous = admission.fetch_sub(1, Ordering::AcqRel);
    debug_assert!(previous > 0 && previous <= PARAMETER_EDIT_CALLBACK_ADMISSION_CAPACITY);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RealtimeTransport {
    request: TransportRequest,
    applied_discontinuity_id: u64,
    device_frame: u64,
    timeline_frame: u64,
    beat_q32: u64,
    epoch: u64,
    loop_count: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TransportDiscontinuity {
    OneShot,
    Loop,
}

impl Default for RealtimeTransport {
    fn default() -> Self {
        Self {
            request: TransportRequest::default(),
            applied_discontinuity_id: 0,
            device_frame: 0,
            timeline_frame: 0,
            beat_q32: 0,
            epoch: 1,
            loop_count: 0,
        }
    }
}

impl RealtimeTransport {
    fn apply_pending_timeline_activation(&mut self, status: &AudioStatus, dsp: &mut DspState) {
        let Some(ticket) = dsp.pending_timeline_transport_activation() else {
            return;
        };
        let spec = ticket.spec();
        let actual_epoch = next_nonzero_id(self.epoch)
            .max(spec.minimum_epoch)
            .max(spec.target_epoch);
        if let Err(reason) = dsp.preflight_timeline_transport_activation(ticket, actual_epoch) {
            dsp.reject_timeline_transport_activation(ticket, reason);
            return;
        }

        let committed = dsp.commit_timeline_transport_activation(status, ticket, actual_epoch);
        self.beat_q32 = spec.beat_q32;
        self.timeline_frame = spec.frame;
        self.epoch = actual_epoch;
        self.request.request_id = ticket.legacy_transport_barrier();
        self.request.target_beat_q32 = spec.beat_q32;
        self.request.target_timeline_frame = spec.frame;
        self.request.playing = spec.playing;
        self.request.loop_enabled =
            spec.loop_enabled && spec.loop_end_frame > spec.loop_start_frame;
        self.request.loop_start_q32 = spec.loop_start_q32;
        self.request.loop_end_q32 = spec.loop_end_q32;
        self.request.loop_start_frame = spec.loop_start_frame;
        self.request.loop_end_frame = spec.loop_end_frame;
        self.publish(status);
        // This shared identity and its exact receipt are the final writes of the
        // proven callback transaction.
        dsp.publish_timeline_transport_activation(committed);
    }

    fn apply_latest_request(
        &mut self,
        mailbox: &TransportMailbox,
        status: &AudioStatus,
        dsp: &mut DspState,
    ) {
        let Some(request) = mailbox.try_load() else {
            return;
        };
        if request.request_id == self.request.request_id {
            return;
        }

        let timeline_owned = dsp.timeline_transport_owned();
        if request.discontinuity_id != self.applied_discontinuity_id && !timeline_owned {
            self.beat_q32 = request.target_beat_q32;
            self.timeline_frame = request.target_timeline_frame;
            self.epoch = next_nonzero_id(self.epoch);
            self.applied_discontinuity_id = request.discontinuity_id;
            dsp.apply_transport_discontinuity(
                status,
                self.epoch,
                self.beat_q32,
                self.timeline_frame,
                TransportDiscontinuity::OneShot,
            );
        }

        if timeline_owned {
            // Legacy seek/loop APIs remain available for fallback, but only an
            // exact activation bundle may mutate a callback-owned timeline.
            self.applied_discontinuity_id = request.discontinuity_id;
        }

        if self.request.playing && !request.playing {
            dsp.pause_timeline();
        }

        if timeline_owned {
            self.request.request_id = request.request_id;
            self.request.discontinuity_id = request.discontinuity_id;
            self.request.playing = request.playing;
        } else {
            self.request = request;
        }
        self.publish(status);
    }

    fn frames_before_loop(&self, available: usize) -> usize {
        if !self.request.playing
            || !self.request.loop_enabled
            || self.timeline_frame >= self.request.loop_end_frame
        {
            return if self.request.playing
                && self.request.loop_enabled
                && self.timeline_frame >= self.request.loop_end_frame
            {
                0
            } else {
                available
            };
        }
        let remaining = self.request.loop_end_frame - self.timeline_frame;
        let frames = remaining.min(usize::MAX as u64) as usize;
        frames.min(available)
    }

    fn advance(&mut self, status: &AudioStatus, dsp: &mut DspState, frames: usize) {
        self.device_frame = self.device_frame.saturating_add(frames as u64);
        if self.request.playing {
            self.timeline_frame = self.timeline_frame.saturating_add(frames as u64);
            self.beat_q32 = self
                .beat_q32
                .saturating_add(beat_step_q32(status).saturating_mul(frames as u64));
        }
        if self.request.playing
            && self.request.loop_enabled
            && self.timeline_frame >= self.request.loop_end_frame
        {
            self.wrap_loop(status, dsp);
        }
        self.publish(status);
    }

    fn wrap_loop(&mut self, status: &AudioStatus, dsp: &mut DspState) {
        self.beat_q32 = self.request.loop_start_q32;
        self.timeline_frame = self.request.loop_start_frame;
        self.epoch = next_nonzero_id(self.epoch);
        self.loop_count = self.loop_count.wrapping_add(1);
        dsp.apply_transport_discontinuity(
            status,
            self.epoch,
            self.beat_q32,
            self.timeline_frame,
            TransportDiscontinuity::Loop,
        );
    }

    fn publish(&self, status: &AudioStatus) {
        // There is exactly one callback writer. Acquire on the odd-marking RMW
        // prevents the following relaxed field stores from moving before the
        // reader-visible "write in progress" marker; the final Release publishes
        // the complete coherent snapshot.
        let sequence = status.transport_sequence.fetch_add(1, Ordering::Acquire);
        debug_assert_eq!(sequence & 1, 0);
        fence(Ordering::Release);
        status
            .device_frame
            .store(self.device_frame, Ordering::Relaxed);
        status
            .timeline_frame
            .store(self.timeline_frame, Ordering::Relaxed);
        status.beat_q32.store(self.beat_q32, Ordering::Relaxed);
        status.transport_epoch.store(self.epoch, Ordering::Relaxed);
        status
            .transport_loop_count
            .store(self.loop_count, Ordering::Relaxed);
        status
            .applied_transport_request
            .store(self.request.request_id, Ordering::Relaxed);
        status
            .playing
            .store(self.request.playing, Ordering::Relaxed);
        status
            .rendered_frames
            .store(self.device_frame, Ordering::Relaxed);
        status
            .transport_sequence
            .store(sequence.wrapping_add(2), Ordering::Release);
    }
}

fn beat_step_q32(status: &AudioStatus) -> u64 {
    let tempo_milli = u128::from(status.tempo_milli.load(Ordering::Relaxed).max(1));
    let sample_rate = u128::from(status.sample_rate.load(Ordering::Relaxed).max(1));
    let numerator = tempo_milli * u128::from(BEAT_Q32_ONE);
    let denominator = 60_000_u128 * sample_rate;
    u64::try_from((numerator + denominator / 2) / denominator)
        .unwrap_or(u64::MAX)
        .max(1)
}

fn timeline_frame_for_beat(beat_q32: u64, tempo_milli: u32, sample_rate: u32) -> u64 {
    let numerator = u128::from(beat_q32)
        .saturating_mul(60_000)
        .saturating_mul(u128::from(sample_rate.max(1)));
    let denominator = u128::from(tempo_milli.max(1)) * u128::from(BEAT_Q32_ONE);
    u64::try_from((numerator + denominator / 2) / denominator).unwrap_or(u64::MAX)
}

fn render_transport_chunk(
    dsp: &mut DspState,
    status: &AudioStatus,
    mailbox: &TransportMailbox,
    transport: &mut RealtimeTransport,
    frames: usize,
    mut consume: impl FnMut(usize, &[[f32; 2]]),
) {
    let mut rendered = 0;
    while rendered < frames {
        transport.apply_latest_request(mailbox, status, dsp);
        let segment_frames = transport.frames_before_loop(frames - rendered);
        if segment_frames == 0 {
            transport.wrap_loop(status, dsp);
            transport.publish(status);
            continue;
        }
        let capture_device_frame = transport.device_frame;
        let timeline_ready = dsp.prepare_timeline_render(
            transport.epoch,
            transport.timeline_frame,
            transport.beat_q32,
            segment_frames,
            transport.request.playing,
        );
        dsp.service_midi_recording_boundary(
            MidiRecordClockAnchor {
                device_frame: capture_device_frame,
                timeline_frame: transport.timeline_frame,
                transport_epoch: transport.epoch,
                loop_count: transport.loop_count,
            },
            transport.request.playing,
            timeline_ready,
        );
        let midi_monitor_endpoint = if transport.request.playing && !timeline_ready {
            None
        } else {
            dsp.service_midi_input(
                capture_device_frame,
                transport.timeline_frame,
                segment_frames,
                transport.epoch,
            )
        };
        dsp.meter_graph_rendered = false;
        if let Some(publisher) = dsp.meter_publisher.as_mut() {
            publisher.begin_block();
        }
        if !transport.request.playing
            && let Some(monitor) = midi_monitor_endpoint
        {
            let progress = dsp.render_paused_midi_monitor(status, segment_frames, monitor);
            // The monitor graph was already advanced audibly. Generic edits on every unrelated
            // endpoint still receive their bounded silent paused service.
            dsp.observe_midi_safety_progress(progress.generator_mask, segment_frames);
            let safety_generator_mask =
                dsp.service_paused_midi_safety(segment_frames, progress.generator_mask);
            dsp.service_paused_parameter_edits(
                segment_frames,
                progress.generator_mask | safety_generator_mask,
                progress.insert_mask,
            );
        } else if timeline_ready {
            dsp.render_block(status, segment_frames);
            dsp.observe_midi_safety_progress(u64::MAX, segment_frames);
        } else {
            dsp.master_block[..segment_frames].fill([0.0; 2]);
            if !transport.request.playing {
                let safety_generator_mask = dsp.service_paused_midi_safety(segment_frames, 0);
                dsp.service_paused_parameter_edits(segment_frames, safety_generator_mask, 0);
            }
        }
        dsp.publish_plugin_epoch_status(status);
        dsp.capture_rendered_master(capture_device_frame, segment_frames);
        dsp.publish_meters(
            capture_device_frame.saturating_add(segment_frames as u64),
            segment_frames,
        );
        consume(rendered, &dsp.master_block[..segment_frames]);
        transport.advance(status, dsp, segment_frames);
        rendered += segment_frames;
    }
}

fn observe_backend_output_callback(
    telemetry: &CallbackTelemetry,
    interleaved_samples: usize,
    channels: usize,
) {
    let _ = telemetry.observe_callback(interleaved_samples, channels);
}

fn observe_backend_stream_error(
    status: &AudioStatus,
    telemetry: &CallbackTelemetry,
    kind: ErrorKind,
) {
    if matches!(kind, ErrorKind::Xrun) {
        status.xruns.fetch_add(1, Ordering::Relaxed);
    }
    telemetry.observe_error(kind);
}

#[allow(clippy::too_many_arguments)]
fn build_stream<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    mut commands: Consumer<AudioCommand>,
    mut retired_assets: Producer<Arc<[f32]>>,
    mut asset_events: Producer<AudioAssetEvent>,
    retired_insert_endpoints: Producer<RetiredEndpointResource>,
    insert_endpoint_events: Producer<InsertEndpointEvent>,
    generator_endpoint_events: Producer<GeneratorEndpointEvent>,
    retired_midi_inputs: Producer<RetiredMidiInputResource>,
    midi_input_route_events: Producer<MidiInputRouteEvent>,
    midi_recording_endpoint_events: Producer<MidiRecordingEndpointEvent>,
    parameter_edit_callback_events: Producer<CallbackEditReceipt>,
    parameter_edit_callback_admission: Arc<AtomicU32>,
    master_capture_events: Producer<MasterCaptureEndpointEvent>,
    realtime_timeline_runtime: RealtimeTimelineRuntime,
    status: Arc<AudioStatus>,
    transport_mailbox: Arc<TransportMailbox>,
    callback_telemetry: Arc<CallbackTelemetry>,
    meter_publisher: MeterPublisher,
) -> Result<Stream>
where
    T: Sample + SizedSample + FromSample<f32>,
{
    let channels = config.channels as usize;
    let sample_rate = config.sample_rate as f32;
    let error_status = status.clone();
    let error_telemetry = Arc::clone(&callback_telemetry);
    let mut dsp = DspState::try_new_with_endpoint_io(
        sample_rate,
        retired_insert_endpoints,
        insert_endpoint_events,
        generator_endpoint_events,
        master_capture_events,
        PDC_DEFAULT_MAX_DELAY_SAMPLES,
        realtime_timeline_runtime,
    )?;
    dsp.meter_publisher = Some(meter_publisher);
    dsp.retired_midi_inputs = Some(retired_midi_inputs);
    dsp.midi_input_route_events = Some(midi_input_route_events);
    dsp.midi_recording_endpoint_events = Some(midi_recording_endpoint_events);
    dsp.parameter_edit_callback_events = Some(parameter_edit_callback_events);
    dsp.parameter_edit_callback_admission = parameter_edit_callback_admission;
    let mut transport = RealtimeTransport::default();

    Ok(device.build_output_stream(
        *config,
        move |output: &mut [T], _| {
            observe_backend_output_callback(&callback_telemetry, output.len(), channels);
            dsp.set_device_frame(transport.device_frame);
            dsp.set_callback_transport_boundary(
                MidiRecordClockAnchor {
                    device_frame: transport.device_frame,
                    timeline_frame: transport.timeline_frame,
                    transport_epoch: transport.epoch,
                    loop_count: transport.loop_count,
                },
                transport.request.playing,
            );
            process_commands(
                &mut dsp,
                &mut commands,
                &mut retired_assets,
                &mut asset_events,
            );

            for output_block in output.chunks_mut(channels * MAX_MIXER_BLOCK_FRAMES) {
                let frames = output_block.len() / channels;
                dsp.apply_pending_timeline_commands();
                // Atomic activation snapshots bind against this exact coherent
                // PDC revision even while transport is paused. Render performs
                // a third read later to fail closed on worker drift.
                dsp.refresh_pdc_plan(&status, frames);
                transport.apply_pending_timeline_activation(&status, &mut dsp);
                render_transport_chunk(
                    &mut dsp,
                    &status,
                    &transport_mailbox,
                    &mut transport,
                    frames,
                    |frame_offset, rendered| {
                        for (relative_index, [left, right]) in rendered.iter().copied().enumerate()
                        {
                            let frame_index = frame_offset + relative_index;
                            let start = frame_index * channels;
                            let frame = &mut output_block[start..start + channels];
                            for (index, channel) in frame.iter_mut().enumerate() {
                                let sample = if channels == 1 {
                                    (left + right) * 0.5
                                } else if index % 2 == 0 {
                                    left
                                } else {
                                    right
                                };
                                *channel = T::from_sample(sample.clamp(-0.98, 0.98));
                            }
                        }
                    },
                );
            }
        },
        move |error| {
            observe_backend_stream_error(&error_status, &error_telemetry, error.kind());
        },
        None,
    )?)
}

fn process_commands(
    dsp: &mut DspState,
    commands: &mut Consumer<AudioCommand>,
    retired_assets: &mut Producer<Arc<[f32]>>,
    asset_events: &mut Producer<AudioAssetEvent>,
) {
    for _ in 0..MAX_COMMANDS_PER_CALLBACK {
        let Ok(command) = commands.peek() else {
            break;
        };
        let retirements = dsp.asset_retirements_required(command);
        let emits_asset_event = command_emits_asset_event(command);
        // Do not pop an asset lifecycle command unless every Arc it can remove has
        // a guaranteed non-real-time destination and its acknowledgement can be kept.
        if retired_assets.slots() < retirements || (emits_asset_event && asset_events.slots() == 0)
        {
            break;
        }
        if !dsp.endpoint_command_ready(command) {
            break;
        }
        let Ok(command) = commands.pop() else {
            break;
        };
        dsp.handle(command, retired_assets);
        if let Some(event) = dsp.pending_asset_event.take() {
            let _ = asset_events.push(event);
        }
    }
}

fn command_emits_asset_event(command: &AudioCommand) -> bool {
    matches!(
        command,
        AudioCommand::RegisterAsset { .. }
            | AudioCommand::UnregisterAsset { .. }
            | AudioCommand::ClearAssets { .. }
    )
}

#[derive(Clone, Copy, Default)]
struct Voice {
    phase: f32,
    phase_step: f32,
    envelope: f32,
    decay: f32,
    active: bool,
    mixer_track: usize,
    timeline_note_id: Option<u64>,
    timeline_channel_id: Option<u32>,
}

#[derive(Default)]
struct AudioAssetSlot {
    id: u64,
    samples: Option<Arc<[f32]>>,
    sample_rate: u32,
    channels: u16,
    frames: usize,
}

#[derive(Clone, Copy, Default)]
struct AudioClipVoice {
    active: bool,
    clip_id: u64,
    asset_slot: usize,
    source_position: f64,
    gain: f32,
    mixer_track: usize,
    timeline_asset_id: Option<u64>,
    timeline_start_frame: u64,
    timeline_clip_end_frame: u64,
    timeline_stop_frame: u64,
    timeline_frame: u64,
    fades: CompiledClipFades,
    timeline_source_root_frame: u64,
    timeline_source_elapsed_frames: i64,
}

#[derive(Clone, Copy)]
enum TimelinePlannedEventKind {
    NoteOn(ChasedNote),
    NoteOff(ChasedNote),
    GeneratorNoteOn,
    GeneratorNoteOff,
    AudioStart(ChasedAudioClip),
    AudioStop(ChasedAudioClip),
}

#[derive(Clone, Copy)]
struct TimelinePlannedEvent {
    sample_offset: u16,
    kind: TimelinePlannedEventKind,
}

struct TimelineRenderPlan {
    events: Box<[Option<TimelinePlannedEvent>]>,
    len: usize,
    cursor: usize,
    frames: usize,
    overflowed: bool,
    automation_chase_values: Box<[TimelineAutomationChaseValue]>,
    automation_chase_len: usize,
    automation_block: TimelineAutomationBlockPlan,
    automation_block_ready: bool,
}

impl TimelineRenderPlan {
    fn new() -> Self {
        const EMPTY_AUTOMATION_CHASE_VALUE: TimelineAutomationChaseValue =
            TimelineAutomationChaseValue {
                target: CompiledAutomationTarget::MasterVolume,
                value: 0.0,
                shape: AutomationRampShape::Hold,
            };
        Self {
            events: vec![None; MAX_TIMELINE_PLANNED_EVENTS].into_boxed_slice(),
            len: 0,
            cursor: 0,
            frames: 0,
            overflowed: false,
            automation_chase_values: vec![EMPTY_AUTOMATION_CHASE_VALUE; MAX_AUTOMATION_BASES]
                .into_boxed_slice(),
            automation_chase_len: 0,
            automation_block: TimelineAutomationBlockPlan::new(),
            automation_block_ready: false,
        }
    }

    fn clear(&mut self) {
        self.len = 0;
        self.cursor = 0;
        self.frames = 0;
        self.overflowed = false;
        self.automation_chase_len = 0;
        self.automation_block_ready = false;
    }

    fn prepare_automation_block(&mut self, frames: u32) -> bool {
        self.automation_block_ready = self.automation_block.reset(frames).is_ok();
        if !self.automation_block_ready {
            self.overflowed = true;
        }
        self.automation_block_ready
    }

    fn automation_chase_values(&self) -> &[TimelineAutomationChaseValue] {
        &self.automation_chase_values[..self.automation_chase_len]
    }

    fn push(&mut self, sample_offset: u32, kind: TimelinePlannedEventKind) {
        let Some(sample_offset) = u16::try_from(sample_offset).ok() else {
            self.overflowed = true;
            return;
        };
        let Some(slot) = self.events.get_mut(self.len) else {
            self.overflowed = true;
            return;
        };
        *slot = Some(TimelinePlannedEvent {
            sample_offset,
            kind,
        });
        self.len += 1;
    }

    fn begin_render(&mut self, frames: usize) {
        self.cursor = 0;
        self.frames = frames;
    }

    fn next_at(&mut self, frame: usize) -> Option<TimelinePlannedEventKind> {
        if self.cursor >= self.len {
            return None;
        }
        let event = self.events.get(self.cursor).copied().flatten()?;
        if usize::from(event.sample_offset) != frame {
            return None;
        }
        self.cursor += 1;
        Some(event.kind)
    }

    fn rendered_completely(&self) -> bool {
        self.cursor == self.len
    }
}

impl TimelineAudioSink for TimelineRenderPlan {
    fn note_on(&mut self, note: ChasedNote, sample_offset: u32) {
        self.push(sample_offset, TimelinePlannedEventKind::NoteOn(note));
    }

    fn note_off(&mut self, note: ChasedNote, sample_offset: u32) {
        self.push(sample_offset, TimelinePlannedEventKind::NoteOff(note));
    }

    fn audio_start(&mut self, clip: ChasedAudioClip, sample_offset: u32) {
        self.push(sample_offset, TimelinePlannedEventKind::AudioStart(clip));
    }

    fn audio_stop(&mut self, clip: ChasedAudioClip, sample_offset: u32) {
        self.push(sample_offset, TimelinePlannedEventKind::AudioStop(clip));
    }

    fn automation_chase_value(&mut self, value: TimelineAutomationChaseValue) {
        let Some(slot) = self
            .automation_chase_values
            .get_mut(self.automation_chase_len)
        else {
            self.overflowed = true;
            return;
        };
        *slot = value;
        self.automation_chase_len += 1;
    }

    fn automation_transition(&mut self, transition: TimelineAutomationTransition) {
        if !self.automation_block_ready
            || self.automation_block.push_transition(transition).is_err()
        {
            self.overflowed = true;
        }
    }

    fn automation_block_endpoint(&mut self, endpoint: TimelineAutomationBlockEndpoint) {
        if !self.automation_block_ready
            || self.automation_block.push_block_endpoint(endpoint).is_err()
        {
            self.overflowed = true;
        }
    }
}

/// Preallocated, target-major per-sample automation values. The callback fills
/// every compiled base so kernel continuity remains complete, then swaps this
/// object with the committed matrix after both transactions pass preflight.
/// DSP consumers must filter its target prefix through the active timeline's
/// sorted `driven_automation_targets` manifest; undriven bases never override
/// live controls and are excluded from unsupported/pending diagnostics.
struct TimelineAutomationValueMatrix {
    targets: Box<[CompiledAutomationTarget]>,
    values: Box<[f32]>,
    frames: usize,
    target_count: usize,
    rendered_frames: usize,
    finished: bool,
}

impl TimelineAutomationValueMatrix {
    fn new() -> Self {
        Self {
            targets: vec![CompiledAutomationTarget::MasterVolume; MAX_AUTOMATION_BASES]
                .into_boxed_slice(),
            values: vec![0.0; MAX_AUTOMATION_BASES * MAX_MIXER_BLOCK_FRAMES].into_boxed_slice(),
            frames: 0,
            target_count: 0,
            rendered_frames: 0,
            finished: false,
        }
    }

    fn begin(&mut self, frames: usize) -> bool {
        if frames == 0 || frames > MAX_MIXER_BLOCK_FRAMES {
            return false;
        }
        self.frames = frames;
        self.target_count = 0;
        self.rendered_frames = 0;
        self.finished = false;
        true
    }

    fn write(
        &mut self,
        frame: usize,
        slot: usize,
        target: CompiledAutomationTarget,
        value: f32,
    ) -> bool {
        if frame != self.rendered_frames
            || frame >= self.frames
            || slot >= MAX_AUTOMATION_BASES
            || !value.is_finite()
        {
            return false;
        }
        if frame == 0 {
            self.targets[slot] = target;
        } else if self.targets[slot] != target {
            return false;
        }
        self.values[slot * MAX_MIXER_BLOCK_FRAMES + frame] = value;
        true
    }

    fn finish_frame(&mut self, frame: usize, target_count: usize) -> bool {
        if frame != self.rendered_frames
            || frame >= self.frames
            || target_count > MAX_AUTOMATION_BASES
        {
            return false;
        }
        if frame == 0 {
            self.target_count = target_count;
        } else if self.target_count != target_count {
            return false;
        }
        self.rendered_frames += 1;
        true
    }

    fn finish(&mut self) -> bool {
        if self.rendered_frames != self.frames {
            return false;
        }
        self.finished = true;
        true
    }

    fn target_at(&self, slot: u16) -> Option<CompiledAutomationTarget> {
        let slot = usize::from(slot);
        if !self.finished || slot >= self.target_count {
            return None;
        }
        self.targets.get(slot).copied()
    }

    fn value_at_slot(
        &self,
        slot: u16,
        expected_target: CompiledAutomationTarget,
        frame: usize,
    ) -> Option<f32> {
        if self.target_at(slot) != Some(expected_target) || frame >= self.frames {
            return None;
        }
        let value = self.values.get(
            usize::from(slot)
                .saturating_mul(MAX_MIXER_BLOCK_FRAMES)
                .saturating_add(frame),
        )?;
        value.is_finite().then_some(*value)
    }

    fn values_for_slot(
        &self,
        slot: u16,
        expected_target: CompiledAutomationTarget,
    ) -> Option<&[f32]> {
        if self.target_at(slot) != Some(expected_target) {
            return None;
        }
        let start = usize::from(slot).checked_mul(MAX_MIXER_BLOCK_FRAMES)?;
        self.values.get(start..start.checked_add(self.frames)?)
    }

    #[cfg(test)]
    fn value_for(&self, target: CompiledAutomationTarget, frame: usize) -> Option<f32> {
        if !self.finished || frame >= self.frames {
            return None;
        }
        let slot = self.targets[..self.target_count]
            .iter()
            .position(|candidate| *candidate == target)?;
        self.values
            .get(slot * MAX_MIXER_BLOCK_FRAMES + frame)
            .copied()
    }
}

#[derive(Clone, Copy)]
struct TimelineGeneratorNote {
    note_id: u64,
    channel_id: u32,
    note: u8,
    endpoint_id: u64,
    plugin_instance_id: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TimelineGeneratorRoute {
    channel_id: u32,
    plugin_instance_id: u64,
    mixer_track: usize,
    endpoint_id: u64,
}

/// Callback-local snapshot of the exact generator identity contract compiled
/// for one activated revision+epoch. Extra or stale live endpoints never change
/// whether a timeline channel is native or worker-backed.
struct TimelineGeneratorRouteTable {
    routes: [Option<TimelineGeneratorRoute>; MAX_GENERATOR_ENDPOINTS],
    len: usize,
    revision: Option<u64>,
    epoch: Option<u64>,
}

impl TimelineGeneratorRouteTable {
    fn new() -> Self {
        Self {
            routes: [None; MAX_GENERATOR_ENDPOINTS],
            len: 0,
            revision: None,
            epoch: None,
        }
    }

    fn clear(&mut self) {
        self.routes.fill(None);
        self.len = 0;
        self.revision = None;
        self.epoch = None;
    }

    fn reset_from(
        &mut self,
        revision: u64,
        epoch: u64,
        plugin_routes: &[CompiledPluginRoute],
        channel_bases: &[ChannelBaseDescriptor],
    ) -> bool {
        self.clear();
        if revision == 0 || epoch == 0 {
            return false;
        }
        for route in plugin_routes {
            let PluginRouteDestination::Generator { channel_id, .. } = route.destination else {
                continue;
            };
            if route.instance_id == 0
                || self.len == self.routes.len()
                || self.routes[..self.len].iter().flatten().any(|existing| {
                    existing.channel_id == channel_id
                        || existing.plugin_instance_id == route.instance_id
                })
            {
                self.clear();
                return false;
            }
            let Some(mixer_track) = channel_bases
                .iter()
                .find(|base| base.channel_id == channel_id)
                .map(|base| usize::from(base.mixer_track))
                .filter(|mixer_track| *mixer_track < TRACK_COUNT)
            else {
                self.clear();
                return false;
            };
            self.routes[self.len] = Some(TimelineGeneratorRoute {
                channel_id,
                plugin_instance_id: route.instance_id,
                mixer_track,
                endpoint_id: 0,
            });
            self.len += 1;
        }
        self.revision = Some(revision);
        self.epoch = Some(epoch);
        true
    }

    fn bind_installed_endpoints(
        &mut self,
        endpoints: &[Option<GeneratorEndpointSlot>; MAX_GENERATOR_ENDPOINTS],
    ) -> bool {
        for index in 0..self.len {
            let Some(mut route) = self.routes[index] else {
                self.clear();
                return false;
            };
            let Some(endpoint) = endpoints.iter().flatten().find(|endpoint| {
                endpoint.channel_id == route.channel_id
                    && endpoint.plugin_instance_id == route.plugin_instance_id
                    && endpoint.mixer_track == route.mixer_track
                    && endpoint.endpoint_id != 0
            }) else {
                self.clear();
                return false;
            };
            route.endpoint_id = endpoint.endpoint_id;
            self.routes[index] = Some(route);
        }
        true
    }

    fn is_bound_to(&self, revision: u64, epoch: u64) -> bool {
        self.revision == Some(revision) && self.epoch == Some(epoch)
    }

    fn routes(&self) -> impl Iterator<Item = TimelineGeneratorRoute> + '_ {
        self.routes[..self.len].iter().flatten().copied()
    }

    fn route_for_channel(&self, channel_id: u32) -> Option<TimelineGeneratorRoute> {
        self.routes().find(|route| route.channel_id == channel_id)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TimelinePluginEndpointIdentity {
    Generator {
        channel_id: u32,
        endpoint_id: u64,
        plugin_instance_id: u64,
    },
    MixerInsert {
        track: u8,
        endpoint_id: u64,
    },
}

impl TimelinePluginEndpointIdentity {
    fn batch_key(self) -> TimelineEndpointKey {
        match self {
            Self::Generator {
                channel_id,
                endpoint_id,
                plugin_instance_id,
            } => TimelineEndpointKey::new(channel_id, endpoint_id, plugin_instance_id),
            Self::MixerInsert { track, endpoint_id } => {
                TimelineEndpointKey::mixer_insert(track, endpoint_id)
            }
        }
    }

    fn sort_key(self) -> (u8, u32, u64) {
        match self {
            Self::Generator {
                channel_id,
                endpoint_id,
                ..
            } => (0, channel_id, endpoint_id),
            Self::MixerInsert { track, endpoint_id } => (1, u32::from(track), endpoint_id),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TimelinePluginAutomationBinding {
    matrix_slot: u16,
    target: CompiledAutomationTarget,
    parameter_id: u32,
    parameter_slot: u8,
    endpoint: TimelinePluginEndpointIdentity,
    endpoint_snapshot: PluginEndpointSnapshot,
    pdc_plan_revision: Option<u64>,
    control_delay_samples: u32,
    committed_q128: bool,
}

/// Transactional, callback-owned plug-in parameter routing. The active table
/// is never modified while a candidate is being validated.
struct TimelinePluginAutomationBindings {
    bindings: Box<[Option<TimelinePluginAutomationBinding>]>,
    len: usize,
    revision: Option<u64>,
    epoch: Option<u64>,
}

trait TimelinePluginControlPdcPlan {
    fn raw_source_delay_for_runtime_slot(&self, runtime_slot: usize) -> Option<CompensationDelay>;
}

impl TimelinePluginControlPdcPlan for PdcPlan {
    fn raw_source_delay_for_runtime_slot(&self, runtime_slot: usize) -> Option<CompensationDelay> {
        self.raw_track_delay(runtime_slot)
    }
}

impl TimelinePluginControlPdcPlan for GraphPdcPlan {
    fn raw_source_delay_for_runtime_slot(&self, runtime_slot: usize) -> Option<CompensationDelay> {
        GraphPdcPlan::raw_source_delay_for_runtime_slot(self, runtime_slot)
    }
}

impl TimelinePluginAutomationBindings {
    fn new() -> Self {
        Self {
            bindings: vec![None; TIMELINE_MAX_DRIVEN_PLUGIN_PARAMETERS].into_boxed_slice(),
            len: 0,
            revision: None,
            epoch: None,
        }
    }

    fn clear(&mut self) {
        self.bindings.fill(None);
        self.len = 0;
        self.revision = None;
        self.epoch = None;
    }

    // The activation candidate must validate these exact, independently owned snapshots together.
    #[allow(clippy::too_many_arguments)]
    fn reset_from<P: TimelinePluginControlPdcPlan>(
        &mut self,
        revision: u64,
        epoch: u64,
        bases: &[crate::timeline::AutomationBaseValue],
        driven_targets: &[CompiledAutomationTarget],
        plugin_routes: &[CompiledPluginRoute],
        generator_routes: &TimelineGeneratorRouteTable,
        generator_endpoints: &mut [Option<GeneratorEndpointSlot>; MAX_GENERATOR_ENDPOINTS],
        insert_endpoints: &mut [Option<InsertEndpointSlot>; TRACK_COUNT],
        pdc_plan: &P,
        pdc_plan_revision: u64,
        maximum_control_delay_samples: u32,
    ) -> bool {
        self.clear();
        if revision == 0 || epoch == 0 || !generator_routes.is_bound_to(revision, epoch) {
            return false;
        }

        for (matrix_slot, base) in bases.iter().copied().enumerate() {
            if driven_targets.binary_search(&base.target).is_err() {
                continue;
            }
            let CompiledAutomationTarget::PluginParameter {
                instance_id,
                parameter_id,
            } = base.target
            else {
                continue;
            };
            let Some(compiled_route) = plugin_routes
                .iter()
                .find(|route| route.instance_id == instance_id)
            else {
                self.clear();
                return false;
            };
            let (endpoint, endpoint_snapshot, parameter_slot, control_delay_samples, pdc_revision) =
                match compiled_route.destination {
                    PluginRouteDestination::Generator {
                        channel_id,
                        slot: 0,
                    } => {
                        let Some(route) =
                            generator_routes
                                .route_for_channel(channel_id)
                                .filter(|route| {
                                    route.plugin_instance_id == instance_id
                                        && route.endpoint_id != 0
                                })
                        else {
                            self.clear();
                            return false;
                        };
                        let Some(endpoint) =
                            generator_endpoints.iter_mut().flatten().find(|endpoint| {
                                endpoint.channel_id == route.channel_id
                                    && endpoint.endpoint_id == route.endpoint_id
                                    && endpoint.plugin_instance_id == route.plugin_instance_id
                                    && endpoint.mixer_track == route.mixer_track
                            })
                        else {
                            self.clear();
                            return false;
                        };
                        let Some(snapshot) = endpoint.endpoint.cached_exact_endpoint_snapshot()
                        else {
                            self.clear();
                            return false;
                        };
                        let Some(slot) = snapshot.slot(0) else {
                            self.clear();
                            return false;
                        };
                        if snapshot.slot_count() != 1
                            || slot.instance_id() != instance_id
                            || !slot.is_active()
                            || slot.prefix_latency_samples() != 0
                        {
                            self.clear();
                            return false;
                        }
                        (
                            TimelinePluginEndpointIdentity::Generator {
                                channel_id,
                                endpoint_id: endpoint.endpoint_id,
                                plugin_instance_id: instance_id,
                            },
                            snapshot,
                            0,
                            0,
                            None,
                        )
                    }
                    PluginRouteDestination::Generator { .. } => {
                        self.clear();
                        return false;
                    }
                    PluginRouteDestination::MixerInsert { track, slot } => {
                        // Master automation needs the distinct R+prefix time domain. Keep it
                        // explicitly pending until that path receives its own validated contract.
                        if track == 0 || usize::from(track) >= TRACK_COUNT {
                            continue;
                        }
                        let Some(endpoint) = insert_endpoints[usize::from(track)].as_mut() else {
                            self.clear();
                            return false;
                        };
                        let Some(snapshot) = endpoint.endpoint.cached_exact_endpoint_snapshot()
                        else {
                            self.clear();
                            return false;
                        };
                        if !mixer_manifest_matches(snapshot, plugin_routes, track) {
                            self.clear();
                            return false;
                        }
                        let Some(slot_snapshot) = snapshot.slot(usize::from(slot)) else {
                            self.clear();
                            return false;
                        };
                        if slot_snapshot.instance_id() != instance_id || !slot_snapshot.is_active()
                        {
                            self.clear();
                            return false;
                        }
                        if pdc_plan_revision == 0 {
                            self.clear();
                            return false;
                        }
                        let Some(raw_delay) =
                            pdc_plan.raw_source_delay_for_runtime_slot(usize::from(track))
                        else {
                            self.clear();
                            return false;
                        };
                        let control_delay = raw_delay
                            .requested_samples()
                            .checked_add(u64::from(slot_snapshot.prefix_latency_samples()));
                        let Some(control_delay) = control_delay
                            .filter(|_| !raw_delay.is_clamped())
                            .filter(|delay| *delay <= u64::from(maximum_control_delay_samples))
                            .and_then(|delay| u32::try_from(delay).ok())
                        else {
                            self.clear();
                            return false;
                        };
                        (
                            TimelinePluginEndpointIdentity::MixerInsert {
                                track,
                                endpoint_id: endpoint.endpoint_id,
                            },
                            snapshot,
                            slot,
                            control_delay,
                            Some(pdc_plan_revision),
                        )
                    }
                };
            if self.len == self.bindings.len()
                || self.bindings[..self.len]
                    .iter()
                    .flatten()
                    .filter(|binding| binding.endpoint == endpoint)
                    .count()
                    == TIMELINE_ENDPOINT_MAX_DRIVEN_PLUGIN_PARAMETERS
            {
                self.clear();
                return false;
            }
            let Ok(matrix_slot) = u16::try_from(matrix_slot) else {
                self.clear();
                return false;
            };
            let binding = TimelinePluginAutomationBinding {
                matrix_slot,
                target: base.target,
                parameter_id,
                parameter_slot,
                endpoint,
                endpoint_snapshot,
                pdc_plan_revision: pdc_revision,
                control_delay_samples,
                committed_q128: false,
            };
            if self.bindings[..self.len]
                .iter()
                .flatten()
                .any(|existing| existing.target == binding.target)
            {
                self.clear();
                return false;
            }
            self.bindings[self.len] = Some(binding);
            self.len += 1;
        }
        // Canonical endpoint/slot/parameter order makes same-boundary delivery
        // invariant under automation-base and persisted instance ordering.
        for index in 1..self.len {
            let binding = self.bindings[index]
                .take()
                .expect("the active binding prefix is dense");
            let key = (
                binding.endpoint.sort_key(),
                binding.parameter_slot,
                binding.parameter_id,
            );
            let mut destination = index;
            while destination > 0 {
                let previous = self.bindings[destination - 1]
                    .expect("the sorted binding prefix remains dense");
                let previous_key = (
                    previous.endpoint.sort_key(),
                    previous.parameter_slot,
                    previous.parameter_id,
                );
                if previous_key <= key {
                    break;
                }
                self.bindings[destination] = Some(previous);
                destination -= 1;
            }
            self.bindings[destination] = Some(binding);
        }
        self.revision = Some(revision);
        self.epoch = Some(epoch);
        true
    }

    fn is_bound_to(&self, revision: u64, epoch: u64) -> bool {
        self.revision == Some(revision) && self.epoch == Some(epoch)
    }

    fn iter(&self) -> impl Iterator<Item = TimelinePluginAutomationBinding> + '_ {
        self.bindings[..self.len].iter().flatten().copied()
    }

    fn slots_match(&self, matrix: &TimelineAutomationValueMatrix) -> bool {
        self.iter()
            .all(|binding| matrix.target_at(binding.matrix_slot) == Some(binding.target))
    }

    fn identities_match(
        &self,
        generator_endpoints: &mut [Option<GeneratorEndpointSlot>; MAX_GENERATOR_ENDPOINTS],
        insert_endpoints: &mut [Option<InsertEndpointSlot>; TRACK_COUNT],
        pdc_plan_revision: u64,
    ) -> bool {
        self.iter().all(|binding| {
            if binding
                .pdc_plan_revision
                .is_some_and(|revision| revision != pdc_plan_revision)
            {
                return false;
            }
            let snapshot = match binding.endpoint {
                TimelinePluginEndpointIdentity::Generator {
                    channel_id,
                    endpoint_id,
                    plugin_instance_id,
                } => generator_endpoints
                    .iter_mut()
                    .flatten()
                    .find(|endpoint| {
                        endpoint.channel_id == channel_id
                            && endpoint.endpoint_id == endpoint_id
                            && endpoint.plugin_instance_id == plugin_instance_id
                    })
                    .and_then(|endpoint| endpoint.endpoint.exact_endpoint_snapshot()),
                TimelinePluginEndpointIdentity::MixerInsert { track, endpoint_id } => {
                    insert_endpoints
                        .get_mut(usize::from(track))
                        .and_then(Option::as_mut)
                        .filter(|endpoint| endpoint.endpoint_id == endpoint_id)
                        .and_then(|endpoint| endpoint.endpoint.exact_endpoint_snapshot())
                }
            };
            snapshot == Some(binding.endpoint_snapshot)
        })
    }

    fn applied_automation_target_count(&self) -> u64 {
        self.bindings[..self.len]
            .iter()
            .flatten()
            .filter(|binding| binding.committed_q128)
            .count() as u64
    }

    fn mark_committed_from_batch(&mut self, batch: &TimelineEndpointBatchPlan) {
        for binding in self.bindings[..self.len].iter_mut().flatten() {
            if binding.committed_q128 {
                continue;
            }
            let key = binding.endpoint.batch_key();
            binding.committed_q128 = (0..batch.endpoint_count()).any(|index| {
                batch.prepared_endpoint_at(index).is_some_and(|prepared| {
                    prepared.identity().key == key
                        && prepared.events().iter().any(|event| {
                            matches!(
                                event.kind,
                                crate::fixed_quantum::FrameEventKind::Parameter {
                                    slot,
                                    id,
                                    ..
                                } if slot == binding.parameter_slot && id == binding.parameter_id
                            )
                        })
                })
            });
        }
    }

    fn references_generator_endpoint(&self, endpoint_id: u64, plugin_instance_id: u64) -> bool {
        self.iter().any(|binding| {
            matches!(
                binding.endpoint,
                TimelinePluginEndpointIdentity::Generator {
                    endpoint_id: actual_endpoint_id,
                    plugin_instance_id: actual_instance_id,
                    ..
                } if actual_endpoint_id == endpoint_id && actual_instance_id == plugin_instance_id
            )
        })
    }

    fn references_insert_endpoint(&self, track: usize, endpoint_id: u64) -> bool {
        u8::try_from(track).ok().is_some_and(|track| {
            self.iter().any(|binding| {
                binding.endpoint
                    == TimelinePluginEndpointIdentity::MixerInsert { track, endpoint_id }
            })
        })
    }

    fn owns_generator_parameter(&self, channel_id: u32, slot: usize, parameter_id: u32) -> bool {
        slot == 0
            && self.iter().any(|binding| {
                matches!(
                    binding.endpoint,
                    TimelinePluginEndpointIdentity::Generator {
                        channel_id: actual_channel_id,
                        ..
                    } if actual_channel_id == channel_id
                ) && binding.parameter_slot == 0
                    && binding.parameter_id == parameter_id
            })
    }

    fn owns_insert_parameter(&self, track: usize, slot: usize, parameter_id: u32) -> bool {
        let (Ok(track), Ok(slot)) = (u8::try_from(track), u8::try_from(slot)) else {
            return false;
        };
        self.iter().any(|binding| {
            matches!(
                binding.endpoint,
                TimelinePluginEndpointIdentity::MixerInsert {
                    track: actual_track,
                    ..
                } if actual_track == track
            ) && binding.parameter_slot == slot
                && binding.parameter_id == parameter_id
        })
    }
}

fn mixer_manifest_matches(
    snapshot: PluginEndpointSnapshot,
    plugin_routes: &[CompiledPluginRoute],
    track: u8,
) -> bool {
    let mut expected = [None; MAX_INSERT_PLUGIN_SLOTS];
    let mut slot_count = 0_usize;
    for route in plugin_routes {
        let PluginRouteDestination::MixerInsert {
            track: route_track,
            slot,
        } = route.destination
        else {
            continue;
        };
        if route_track != track {
            continue;
        }
        let slot = usize::from(slot);
        if slot >= expected.len() || route.instance_id == 0 || expected[slot].is_some() {
            return false;
        }
        expected[slot] = Some(route.instance_id);
        slot_count = slot_count.max(slot + 1);
    }
    slot_count != 0
        && snapshot.slot_count() == slot_count
        && expected[..slot_count]
            .iter()
            .enumerate()
            .all(|(slot, expected)| {
                expected.is_some()
                    && snapshot
                        .slot(slot)
                        .is_some_and(|actual| Some(actual.instance_id()) == *expected)
            })
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct TimelineChannelRenderBase {
    channel_id: u32,
    volume: f32,
    pan: f32,
    muted: bool,
    solo: bool,
    mixer_track: usize,
    volume_automation_slot: Option<u16>,
    pan_automation_slot: Option<u16>,
    mute_automation_slot: Option<u16>,
}

impl From<ChannelBaseDescriptor> for TimelineChannelRenderBase {
    fn from(base: ChannelBaseDescriptor) -> Self {
        Self {
            channel_id: base.channel_id,
            volume: base.volume,
            pan: base.pan,
            muted: base.muted,
            solo: base.solo,
            mixer_track: usize::from(base.mixer_track).min(TRACK_COUNT - 1),
            volume_automation_slot: None,
            pan_automation_slot: None,
            mute_automation_slot: None,
        }
    }
}

#[derive(Clone, Copy)]
struct TimelineChannelAutomationBinding {
    slot: u16,
    target: CompiledAutomationTarget,
}

struct TimelineChannelBaseTable {
    slots: Box<[Option<TimelineChannelRenderBase>]>,
    any_solo: bool,
    automation_bindings: Box<[Option<TimelineChannelAutomationBinding>]>,
    automation_binding_len: usize,
}

impl TimelineChannelBaseTable {
    fn new() -> Self {
        debug_assert!(TIMELINE_CHANNEL_BASE_TABLE_CAPACITY.is_power_of_two());
        Self {
            slots: vec![None; TIMELINE_CHANNEL_BASE_TABLE_CAPACITY].into_boxed_slice(),
            any_solo: false,
            automation_bindings: vec![None; MAX_AUTOMATION_BASES].into_boxed_slice(),
            automation_binding_len: 0,
        }
    }

    fn clear(&mut self) {
        self.slots.fill(None);
        self.any_solo = false;
        self.automation_bindings.fill(None);
        self.automation_binding_len = 0;
    }

    fn reset_from(&mut self, bases: &[ChannelBaseDescriptor]) -> bool {
        self.clear();
        for base in bases.iter().copied() {
            let base = TimelineChannelRenderBase::from(base);
            self.any_solo |= base.solo;
            if !self.insert(base) {
                self.clear();
                return false;
            }
        }
        true
    }

    fn insert(&mut self, base: TimelineChannelRenderBase) -> bool {
        let mask = self.slots.len() - 1;
        let start = (base.channel_id as usize).wrapping_mul(0x9E37_79B1) & mask;
        for probe in 0..self.slots.len() {
            let index = start.wrapping_add(probe) & mask;
            match self.slots[index] {
                Some(existing) if existing.channel_id != base.channel_id => {}
                _ => {
                    self.slots[index] = Some(base);
                    return true;
                }
            }
        }
        false
    }

    fn get(&self, channel_id: u32) -> Option<TimelineChannelRenderBase> {
        let mask = self.slots.len() - 1;
        let start = (channel_id as usize).wrapping_mul(0x9E37_79B1) & mask;
        for probe in 0..self.slots.len() {
            let index = start.wrapping_add(probe) & mask;
            match self.slots[index] {
                Some(base) if base.channel_id == channel_id => return Some(base),
                Some(_) => {}
                None => return None,
            }
        }
        None
    }

    fn get_mut(&mut self, channel_id: u32) -> Option<&mut TimelineChannelRenderBase> {
        let mask = self.slots.len() - 1;
        let start = (channel_id as usize).wrapping_mul(0x9E37_79B1) & mask;
        for probe in 0..self.slots.len() {
            let index = start.wrapping_add(probe) & mask;
            match self.slots[index] {
                Some(base) if base.channel_id == channel_id => return self.slots[index].as_mut(),
                Some(_) => {}
                None => return None,
            }
        }
        None
    }

    fn bind_driven_automation(
        &mut self,
        bases: &[crate::timeline::AutomationBaseValue],
        driven_targets: &[CompiledAutomationTarget],
        plugin_routes: &[CompiledPluginRoute],
    ) -> bool {
        self.automation_bindings.fill(None);
        self.automation_binding_len = 0;
        for base in self.slots.iter_mut().flatten() {
            base.volume_automation_slot = None;
            base.pan_automation_slot = None;
            base.mute_automation_slot = None;
        }

        for (slot, base) in bases.iter().copied().enumerate() {
            if driven_targets.binary_search(&base.target).is_err() {
                continue;
            }
            let (channel_id, field) = match base.target {
                CompiledAutomationTarget::ChannelVolume { channel_id } => (channel_id, 0_u8),
                CompiledAutomationTarget::ChannelPan { channel_id } => (channel_id, 1_u8),
                CompiledAutomationTarget::ChannelMute { channel_id } => (channel_id, 2_u8),
                _ => continue,
            };
            let generator_backed = plugin_routes.iter().any(|route| {
                matches!(
                    route.destination,
                    PluginRouteDestination::Generator {
                        channel_id: routed_channel,
                        ..
                    } if routed_channel == channel_id
                )
            });
            if generator_backed {
                continue;
            }
            let Ok(slot) = u16::try_from(slot) else {
                self.clear();
                return false;
            };
            let Some(channel) = self.get_mut(channel_id) else {
                self.clear();
                return false;
            };
            let automation_slot = match field {
                0 => &mut channel.volume_automation_slot,
                1 => &mut channel.pan_automation_slot,
                _ => &mut channel.mute_automation_slot,
            };
            if automation_slot.replace(slot).is_some() {
                self.clear();
                return false;
            }
            let Some(binding) = self
                .automation_bindings
                .get_mut(self.automation_binding_len)
            else {
                self.clear();
                return false;
            };
            *binding = Some(TimelineChannelAutomationBinding {
                slot,
                target: base.target,
            });
            self.automation_binding_len += 1;
        }
        true
    }

    fn automation_slots_match(&self, matrix: &TimelineAutomationValueMatrix) -> bool {
        self.automation_bindings[..self.automation_binding_len]
            .iter()
            .copied()
            .flatten()
            .all(|binding| matrix.target_at(binding.slot) == Some(binding.target))
    }

    fn applied_automation_target_count(&self) -> u64 {
        self.automation_binding_len as u64
    }

    fn is_audible(&self, base: TimelineChannelRenderBase) -> bool {
        !base.muted && (!self.any_solo || base.solo)
    }
}

const EMPTY_ENDPOINT_FRAME_EVENT: FrameEvent = FrameEvent::parameter(0, 0, 0, 0.0);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct EndpointQuantumClassUsage {
    system: usize,
    timeline: usize,
    live: usize,
    total: usize,
}

impl EndpointQuantumClassUsage {
    fn count(self, class: EndpointEventClass) -> usize {
        match class {
            EndpointEventClass::System => self.system,
            EndpointEventClass::Timeline => self.timeline,
            EndpointEventClass::Live => self.live,
        }
    }

    fn increment(&mut self, class: EndpointEventClass) {
        match class {
            EndpointEventClass::System => self.system += 1,
            EndpointEventClass::Timeline => self.timeline += 1,
            EndpointEventClass::Live => self.live += 1,
        }
        self.total += 1;
    }
}

#[derive(Clone, Copy)]
struct EndpointEventLane<const CAPACITY: usize> {
    pending: [FrameEvent; CAPACITY],
    pending_len: usize,
}

impl<const CAPACITY: usize> EndpointEventLane<CAPACITY> {
    fn new() -> Self {
        Self {
            pending: [EMPTY_ENDPOINT_FRAME_EVENT; CAPACITY],
            pending_len: 0,
        }
    }

    fn clear(&mut self) {
        self.pending_len = 0;
    }

    fn push_preflighted(&mut self, event: FrameEvent) {
        debug_assert!(self.pending_len < CAPACITY);
        self.pending[self.pending_len] = event;
        self.pending_len += 1;
    }

    fn retain(&mut self, mut keep: impl FnMut(FrameEvent) -> bool) {
        let mut retained = 0;
        for index in 0..self.pending_len {
            let event = self.pending[index];
            if keep(event) {
                self.pending[retained] = event;
                retained += 1;
            }
        }
        self.pending_len = retained;
    }

    fn stage(&mut self, event: FrameEvent, input_phase: usize, quantum_capacity: usize) -> bool {
        if self.pending_len == CAPACITY {
            return false;
        }
        let event_quantum =
            (input_phase + usize::from(event.sample_offset)) / DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES;
        let events_in_quantum = self.pending[..self.pending_len]
            .iter()
            .filter(|pending| {
                (input_phase + usize::from(pending.sample_offset))
                    / DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES
                    == event_quantum
            })
            .count();
        if events_in_quantum == quantum_capacity {
            return false;
        }
        self.pending[self.pending_len] = event;
        self.pending_len += 1;
        true
    }

    fn count_in_quantum(&self, input_phase: usize, quantum: usize) -> usize {
        self.pending[..self.pending_len]
            .iter()
            .filter(|pending| {
                (input_phase + usize::from(pending.sample_offset))
                    / DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES
                    == quantum
            })
            .count()
    }

    fn drain_callback(
        &mut self,
        frames: usize,
        current: &mut [FrameEvent; MAX_FRAME_EVENTS_PER_CALLBACK],
        current_classes: &mut [EndpointEventClass; MAX_FRAME_EVENTS_PER_CALLBACK],
        current_len: &mut usize,
        class: EndpointEventClass,
    ) {
        let mut retained = 0;
        for index in 0..self.pending_len {
            let mut event = self.pending[index];
            if usize::from(event.sample_offset) < frames {
                debug_assert!(*current_len < current.len());
                current[*current_len] = event;
                current_classes[*current_len] = class;
                *current_len += 1;
            } else {
                event.sample_offset = event.sample_offset.saturating_sub(frames as u16);
                self.pending[retained] = event;
                retained += 1;
            }
        }
        self.pending_len = retained;
    }
}

struct EndpointFrameEventScratch {
    system: EndpointEventLane<TIMELINE_ENDPOINT_SYSTEM_MAX_EVENTS_PER_CALLBACK>,
    timeline: EndpointEventLane<TIMELINE_ENDPOINT_TIMELINE_MAX_EVENTS_PER_CALLBACK>,
    live: EndpointEventLane<TIMELINE_ENDPOINT_LIVE_MAX_EVENTS_PER_CALLBACK>,
    current: [FrameEvent; MAX_FRAME_EVENTS_PER_CALLBACK],
    current_classes: [EndpointEventClass; MAX_FRAME_EVENTS_PER_CALLBACK],
    current_len: usize,
    live_faulted: bool,
    fail_closed_defer_offset: usize,
}

struct EndpointFrameEvents {
    scratch: Box<EndpointFrameEventScratch>,
}

impl EndpointFrameEvents {
    fn new() -> Self {
        Self {
            scratch: Box::new(EndpointFrameEventScratch {
                system: EndpointEventLane::new(),
                timeline: EndpointEventLane::new(),
                live: EndpointEventLane::new(),
                current: [EMPTY_ENDPOINT_FRAME_EVENT; MAX_FRAME_EVENTS_PER_CALLBACK],
                current_classes: [EndpointEventClass::System; MAX_FRAME_EVENTS_PER_CALLBACK],
                current_len: 0,
                live_faulted: false,
                fail_closed_defer_offset: 0,
            }),
        }
    }

    fn has_admitted_live_edit_marker(&self) -> bool {
        self.scratch.live.pending[..self.scratch.live.pending_len]
            .iter()
            .any(|event| {
                matches!(
                    event.kind,
                    crate::fixed_quantum::FrameEventKind::Parameter {
                        edit_id: Some(_),
                        ..
                    }
                )
            })
    }

    /// Class quotas are independent, so a live-control overflow cannot evict a
    /// committed timeline batch or the transport safety lane.
    fn stage(
        &mut self,
        class: EndpointEventClass,
        event: FrameEvent,
        input_phase: usize,
        partial_usage: EndpointQuantumClassUsage,
    ) -> bool {
        if class == EndpointEventClass::Live && self.scratch.live_faulted {
            return false;
        }
        let quantum =
            (input_phase + usize::from(event.sample_offset)) / DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES;
        let mut usage = self.quantum_usage(input_phase, quantum);
        if quantum == 0 {
            usage.system += partial_usage.system;
            usage.timeline += partial_usage.timeline;
            usage.live += partial_usage.live;
            usage.total += partial_usage.total;
        }
        let class_capacity = match class {
            EndpointEventClass::System => TIMELINE_ENDPOINT_SYSTEM_MAX_EVENTS_PER_QUANTUM,
            EndpointEventClass::Timeline => TIMELINE_ENDPOINT_TIMELINE_MAX_EVENTS_PER_QUANTUM,
            EndpointEventClass::Live => TIMELINE_ENDPOINT_LIVE_MAX_EVENTS_PER_QUANTUM,
        };
        if usage.count(class) == class_capacity || usage.total == MAX_FRAME_EVENTS_PER_QUANTUM {
            if class == EndpointEventClass::Live {
                self.fail_live_lane(input_phase, partial_usage);
            }
            return false;
        }
        let staged = match class {
            EndpointEventClass::System => self.scratch.system.stage(
                event,
                input_phase,
                TIMELINE_ENDPOINT_SYSTEM_MAX_EVENTS_PER_QUANTUM,
            ),
            EndpointEventClass::Timeline => self.scratch.timeline.stage(
                event,
                input_phase,
                TIMELINE_ENDPOINT_TIMELINE_MAX_EVENTS_PER_QUANTUM,
            ),
            EndpointEventClass::Live => self.scratch.live.stage(
                event,
                input_phase,
                TIMELINE_ENDPOINT_LIVE_MAX_EVENTS_PER_QUANTUM,
            ),
        };
        if !staged && class == EndpointEventClass::Live {
            self.fail_live_lane(input_phase, partial_usage);
        }
        staged
    }

    fn fail_live_lane(&mut self, input_phase: usize, partial_usage: EndpointQuantumClassUsage) {
        let _ = self.panic_midi_preserving_parameter_edits(input_phase, partial_usage, None, false);
    }

    fn can_stage_batch(
        &self,
        events: &[FrameEvent],
        classes: &[EndpointEventClass],
        input_phase: usize,
        partial_usage: EndpointQuantumClassUsage,
    ) -> bool {
        if events.len() != classes.len() {
            return false;
        }

        const QUANTUM_BUCKETS: usize =
            MAX_MIXER_BLOCK_FRAMES / DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES + 1;
        let mut usage = [EndpointQuantumClassUsage::default(); QUANTUM_BUCKETS];
        usage[0] = partial_usage;
        for event in &self.scratch.system.pending[..self.scratch.system.pending_len] {
            let quantum = (input_phase + usize::from(event.sample_offset))
                / DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES;
            if let Some(usage) = usage.get_mut(quantum) {
                usage.increment(EndpointEventClass::System);
            }
        }
        for event in &self.scratch.timeline.pending[..self.scratch.timeline.pending_len] {
            let quantum = (input_phase + usize::from(event.sample_offset))
                / DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES;
            if let Some(usage) = usage.get_mut(quantum) {
                usage.increment(EndpointEventClass::Timeline);
            }
        }
        for event in &self.scratch.live.pending[..self.scratch.live.pending_len] {
            let quantum = (input_phase + usize::from(event.sample_offset))
                / DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES;
            if let Some(usage) = usage.get_mut(quantum) {
                usage.increment(EndpointEventClass::Live);
            }
        }
        let mut callback_usage = EndpointQuantumClassUsage {
            system: self.scratch.system.pending_len,
            timeline: self.scratch.timeline.pending_len,
            live: self.scratch.live.pending_len,
            total: self.scratch.system.pending_len
                + self.scratch.timeline.pending_len
                + self.scratch.live.pending_len,
        };
        for (event, class) in events.iter().copied().zip(classes.iter().copied()) {
            let quantum = (input_phase + usize::from(event.sample_offset))
                / DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES;
            let Some(usage) = usage.get_mut(quantum) else {
                return false;
            };
            let quantum_capacity = match class {
                EndpointEventClass::System => TIMELINE_ENDPOINT_SYSTEM_MAX_EVENTS_PER_QUANTUM,
                EndpointEventClass::Timeline => TIMELINE_ENDPOINT_TIMELINE_MAX_EVENTS_PER_QUANTUM,
                EndpointEventClass::Live => TIMELINE_ENDPOINT_LIVE_MAX_EVENTS_PER_QUANTUM,
            };
            let callback_capacity = match class {
                EndpointEventClass::System => TIMELINE_ENDPOINT_SYSTEM_MAX_EVENTS_PER_CALLBACK,
                EndpointEventClass::Timeline => TIMELINE_ENDPOINT_TIMELINE_MAX_EVENTS_PER_CALLBACK,
                EndpointEventClass::Live => TIMELINE_ENDPOINT_LIVE_MAX_EVENTS_PER_CALLBACK,
            };
            if usage.count(class) >= quantum_capacity
                || usage.total >= MAX_FRAME_EVENTS_PER_QUANTUM
                || callback_usage.count(class) >= callback_capacity
            {
                return false;
            }
            usage.increment(class);
            callback_usage.increment(class);
        }
        true
    }

    fn stage_preflighted_batch(&mut self, events: &[FrameEvent], classes: &[EndpointEventClass]) {
        debug_assert_eq!(events.len(), classes.len());
        for (event, class) in events.iter().copied().zip(classes.iter().copied()) {
            match class {
                EndpointEventClass::System => self.scratch.system.push_preflighted(event),
                EndpointEventClass::Timeline => self.scratch.timeline.push_preflighted(event),
                EndpointEventClass::Live => self.scratch.live.push_preflighted(event),
            }
        }
    }

    fn quantum_usage(&self, input_phase: usize, quantum: usize) -> EndpointQuantumClassUsage {
        let system = self.scratch.system.count_in_quantum(input_phase, quantum);
        let timeline = self.scratch.timeline.count_in_quantum(input_phase, quantum);
        let live = self.scratch.live.count_in_quantum(input_phase, quantum);
        EndpointQuantumClassUsage {
            system,
            timeline,
            live,
            total: system + timeline + live,
        }
    }

    fn stage_all_notes_off_at(&mut self, sample_offset: u16, input_phase: usize) {
        self.stage_targeted_all_notes_off_at(sample_offset, input_phase, None);
    }

    fn stage_targeted_all_notes_off_at(
        &mut self,
        sample_offset: u16,
        input_phase: usize,
        slot: Option<u8>,
    ) {
        self.scratch.system.clear();
        for channel in 0..16_u8 {
            let staged = self.scratch.system.stage(
                FrameEvent::midi(sample_offset, slot, [0xB0 | channel, 123, 0]),
                input_phase,
                TIMELINE_ENDPOINT_SYSTEM_MAX_EVENTS_PER_QUANTUM,
            );
            debug_assert!(staged);
        }
    }

    fn panic_midi_preserving_parameter_edits(
        &mut self,
        input_phase: usize,
        partial_usage: EndpointQuantumClassUsage,
        slot: Option<u8>,
        clear_timeline: bool,
    ) -> usize {
        self.scratch.live.retain(|event| {
            matches!(
                event.kind,
                crate::fixed_quantum::FrameEventKind::Parameter {
                    edit_id: Some(_),
                    ..
                }
            )
        });
        if clear_timeline {
            self.scratch.timeline.clear();
            // `current` is the already handed-off adapter view from the latest prepare call.
            // Re-queuing reliable markers here would apply an admitted edit twice.
            self.scratch.current_len = 0;
        }
        self.scratch.system.clear();
        let pending_usage = self.quantum_usage(input_phase, 0);
        let occupied = pending_usage.total.saturating_add(partial_usage.total);
        let defer = pending_usage.system.saturating_add(partial_usage.system) != 0
            || occupied.saturating_add(TIMELINE_ENDPOINT_DISCONTINUITY_EVENTS)
                > MAX_FRAME_EVENTS_PER_QUANTUM;
        let offset = if defer {
            DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES - input_phase
        } else {
            0
        };
        self.stage_targeted_all_notes_off_at(offset as u16, input_phase, slot);
        self.scratch.live_faulted = true;
        self.scratch.fail_closed_defer_offset = offset;
        offset
    }

    fn clear_and_stage_all_notes_off(&mut self, sample_offset: u16, input_phase: usize) {
        self.scratch.system.clear();
        self.scratch.timeline.clear();
        self.scratch.live.clear();
        self.scratch.current_len = 0;
        self.stage_all_notes_off_at(sample_offset, input_phase);
        self.scratch.live_faulted = true;
        self.scratch.fail_closed_defer_offset = usize::from(sample_offset);
    }

    fn fail_closed_defer_offset(&self) -> usize {
        self.scratch.fail_closed_defer_offset
    }

    /// Selects half-open events for this render chunk and carries future offsets
    /// forward without allocation. Equal-offset parameters are ordered before
    /// MIDI so Q128 automation is visible to the note rendered by that quantum.
    fn prepare_callback(&mut self, frames: usize) -> (&[FrameEvent], &[EndpointEventClass]) {
        debug_assert!(frames <= MAX_MIXER_BLOCK_FRAMES);
        self.scratch.current_len = 0;
        self.scratch.system.drain_callback(
            frames,
            &mut self.scratch.current,
            &mut self.scratch.current_classes,
            &mut self.scratch.current_len,
            EndpointEventClass::System,
        );
        self.scratch.timeline.drain_callback(
            frames,
            &mut self.scratch.current,
            &mut self.scratch.current_classes,
            &mut self.scratch.current_len,
            EndpointEventClass::Timeline,
        );
        self.scratch.live.drain_callback(
            frames,
            &mut self.scratch.current,
            &mut self.scratch.current_classes,
            &mut self.scratch.current_len,
            EndpointEventClass::Live,
        );
        self.scratch.live_faulted = false;
        self.scratch.fail_closed_defer_offset = 0;
        (
            &self.scratch.current[..self.scratch.current_len],
            &self.scratch.current_classes[..self.scratch.current_len],
        )
    }
}

/// Control-thread prepared plug-in endpoint. Its adapter and event scratch move
/// together through the command and retire rings; construction never occurs on
/// the device callback.
pub struct PreparedFixedEndpoint {
    adapter: FixedQuantumAdapter<AudioThreadEndpoint>,
    events: EndpointFrameEvents,
    project_session: u64,
    manifest: PluginEndpointManifest,
    coherent_latency: Option<PluginLatencySnapshot>,
    partial_quantum_usage: EndpointQuantumClassUsage,
    admitted_live_edit_markers: u8,
    #[cfg(test)]
    fresh_snapshot_script: [Option<PluginLatencySnapshot>; 2],
    #[cfg(test)]
    fresh_snapshot_script_len: u8,
    #[cfg(test)]
    fresh_snapshot_script_cursor: u8,
}

impl PreparedFixedEndpoint {
    fn new(endpoint: AudioThreadEndpoint) -> Result<Self, FixedQuantumError> {
        let adapter = FixedQuantumAdapter::new(endpoint, DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES)?;
        let manifest = adapter.plugin_endpoint_manifest();
        let coherent_latency = adapter
            .plugin_latency_snapshot()
            .filter(|snapshot| snapshot.revision != 0);
        Ok(Self {
            adapter,
            events: EndpointFrameEvents::new(),
            project_session: 0,
            manifest,
            coherent_latency,
            partial_quantum_usage: EndpointQuantumClassUsage::default(),
            admitted_live_edit_markers: 0,
            #[cfg(test)]
            fresh_snapshot_script: [None; 2],
            #[cfg(test)]
            fresh_snapshot_script_len: 0,
            #[cfg(test)]
            fresh_snapshot_script_cursor: 0,
        })
    }

    fn epoch(&self) -> u64 {
        self.adapter.epoch()
    }

    fn set_epoch(&mut self, epoch: u64) -> bool {
        let changed = self.adapter.set_epoch(epoch);
        if changed {
            self.partial_quantum_usage = EndpointQuantumClassUsage::default();
            self.admitted_live_edit_markers = 0;
        }
        changed
    }

    fn stats(&self) -> FixedQuantumStats {
        self.adapter.stats()
    }

    /// Refresh the callback-owned coherent cache. A bounded seqlock collision
    /// (`None`) deliberately retains the previous graph identity.
    fn refresh_latency_snapshot(&mut self) -> bool {
        let Some(snapshot) = self
            .adapter
            .plugin_latency_snapshot()
            .filter(|snapshot| snapshot.revision != 0)
        else {
            return false;
        };
        let changed = self.coherent_latency != Some(snapshot);
        self.coherent_latency = Some(snapshot);
        changed
    }

    fn coherent_latency_snapshot(&self) -> Option<PluginLatencySnapshot> {
        self.coherent_latency
    }

    /// Return the exact endpoint identity represented by the latency cache that
    /// most recently participated in PDC planning. Candidate binding must not
    /// refresh here: doing so could combine a new slot prefix with an older PDC
    /// compensation plan. The later precommit read is fresh and rejects any
    /// worker publication that happened after this cached generation.
    fn cached_exact_endpoint_snapshot(&self) -> Option<PluginEndpointSnapshot> {
        PluginEndpointSnapshot::try_new(self.manifest, self.coherent_latency?).ok()
    }

    fn exact_endpoint_snapshot(&mut self) -> Option<PluginEndpointSnapshot> {
        #[cfg(test)]
        if self.fresh_snapshot_script_cursor < self.fresh_snapshot_script_len {
            let index = usize::from(self.fresh_snapshot_script_cursor);
            self.fresh_snapshot_script_cursor += 1;
            let snapshot =
                self.fresh_snapshot_script[index].filter(|snapshot| snapshot.revision != 0);
            return self.accept_fresh_endpoint_snapshot(snapshot);
        }
        // Identity-bearing activation/precommit reads are fail-closed: a
        // bounded seqlock collision must not silently reuse an older cache and
        // advance the timeline cursor under an unproven graph revision.
        let snapshot = self
            .adapter
            .plugin_latency_snapshot()
            .filter(|snapshot| snapshot.revision != 0);
        self.accept_fresh_endpoint_snapshot(snapshot)
    }

    #[cfg(test)]
    fn script_fresh_endpoint_snapshots(&mut self, snapshots: [Option<PluginLatencySnapshot>; 2]) {
        self.fresh_snapshot_script = snapshots;
        self.fresh_snapshot_script_len = 2;
        self.fresh_snapshot_script_cursor = 0;
    }

    fn accept_fresh_endpoint_snapshot(
        &mut self,
        snapshot: Option<PluginLatencySnapshot>,
    ) -> Option<PluginEndpointSnapshot> {
        let snapshot = snapshot?;
        let exact = PluginEndpointSnapshot::try_new(self.manifest, snapshot).ok()?;
        self.coherent_latency = Some(snapshot);
        Some(exact)
    }

    fn set_expected_latency_revision(&mut self, revision: u64) {
        self.adapter.set_expected_latency_revision(revision);
    }

    #[cfg(test)]
    fn expected_latency_revision(&self) -> u64 {
        self.adapter.expected_latency_revision()
    }

    fn input_phase_frames(&self) -> usize {
        self.adapter.input_phase_frames()
    }

    fn try_admit_parameter_edit(
        &mut self,
        slot: usize,
        id: u32,
        normalized: f32,
        edit_id: RuntimeParameterEditId,
    ) -> bool {
        self.adapter
            .try_set_parameter_tagged(slot, id, normalized, edit_id)
    }

    fn timeline_batch_seed(&self) -> TimelineEndpointQuantumUsage {
        if self.adapter.input_phase_frames() == 0 {
            return TimelineEndpointQuantumUsage::default();
        }
        TimelineEndpointQuantumUsage {
            system: self.partial_quantum_usage.system as u16,
            timeline: self.partial_quantum_usage.timeline as u16,
            live: self.partial_quantum_usage.live as u16,
            total: self.partial_quantum_usage.total as u16,
        }
    }

    fn stage(&mut self, event: FrameEvent) -> bool {
        self.stage_class(EndpointEventClass::Live, event)
    }

    fn event_rejection_defer_offset(&self) -> usize {
        self.events.fail_closed_defer_offset()
    }

    fn stage_class(&mut self, class: EndpointEventClass, event: FrameEvent) -> bool {
        self.events.stage(
            class,
            event,
            self.adapter.input_phase_frames(),
            self.partial_quantum_usage,
        )
    }

    fn can_stage_batch(&self, events: &[FrameEvent], classes: &[EndpointEventClass]) -> bool {
        self.events.can_stage_batch(
            events,
            classes,
            self.adapter.input_phase_frames(),
            self.partial_quantum_usage,
        )
    }

    fn stage_preflighted_batch(&mut self, events: &[FrameEvent], classes: &[EndpointEventClass]) {
        let admitted = events
            .iter()
            .filter(|event| {
                matches!(
                    event.kind,
                    crate::fixed_quantum::FrameEventKind::Parameter {
                        edit_id: Some(_),
                        ..
                    }
                )
            })
            .count();
        debug_assert!(usize::from(self.admitted_live_edit_markers) + admitted <= u8::MAX as usize);
        self.admitted_live_edit_markers = self
            .admitted_live_edit_markers
            .saturating_add(admitted as u8);
        self.events.stage_preflighted_batch(events, classes);
    }

    fn has_admitted_live_edit_marker(&self) -> bool {
        self.admitted_live_edit_markers != 0 || self.events.has_admitted_live_edit_marker()
    }

    fn clear_and_stage_all_notes_off(&mut self) -> usize {
        let phase = self.adapter.input_phase_frames();
        let defer_to_boundary = phase != 0
            && (self.partial_quantum_usage.system != 0
                || self
                    .partial_quantum_usage
                    .total
                    .saturating_add(TIMELINE_ENDPOINT_DISCONTINUITY_EVENTS)
                    > MAX_FRAME_EVENTS_PER_QUANTUM);
        let offset = if defer_to_boundary {
            self.adapter.next_quantum_boundary_offset()
        } else {
            0
        };
        debug_assert!(u16::try_from(offset).is_ok());
        let offset = offset as u16;
        self.events.clear_and_stage_all_notes_off(offset, phase);
        usize::from(offset)
    }

    fn panic_midi_preserving_parameter_edits(
        &mut self,
        slot: Option<u8>,
        clear_timeline: bool,
    ) -> usize {
        self.events.panic_midi_preserving_parameter_edits(
            self.adapter.input_phase_frames(),
            self.partial_quantum_usage,
            slot,
            clear_timeline,
        )
    }

    fn process(
        &mut self,
        epoch: u64,
        input_left: &[f32],
        input_right: &[f32],
        output_left: &mut [f32],
        output_right: &mut [f32],
    ) -> FixedQuantumProcessStatus {
        let old_phase = self.adapter.input_phase_frames();
        let old_usage = self.partial_quantum_usage;
        let (events, classes) = self.events.prepare_callback(input_left.len());
        let status = self.adapter.process(
            epoch,
            input_left,
            input_right,
            output_left,
            output_right,
            events,
        );
        self.partial_quantum_usage = next_partial_quantum_usage(
            old_phase,
            input_left.len(),
            old_usage,
            events,
            classes,
            self.adapter.input_phase_frames(),
        );
        if matches!(
            status,
            FixedQuantumProcessStatus::Processed {
                completed_quanta: 1..,
                ..
            }
        ) {
            self.admitted_live_edit_markers = 0;
        }
        status
    }

    fn process_generator(
        &mut self,
        epoch: u64,
        frames: usize,
        output_left: &mut [f32],
        output_right: &mut [f32],
    ) -> FixedQuantumProcessStatus {
        let old_phase = self.adapter.input_phase_frames();
        let old_usage = self.partial_quantum_usage;
        let (events, classes) = self.events.prepare_callback(frames);
        let status =
            self.adapter
                .process_generator(epoch, frames, output_left, output_right, events);
        self.partial_quantum_usage = next_partial_quantum_usage(
            old_phase,
            frames,
            old_usage,
            events,
            classes,
            self.adapter.input_phase_frames(),
        );
        if matches!(
            status,
            FixedQuantumProcessStatus::Processed {
                completed_quanta: 1..,
                ..
            }
        ) {
            self.admitted_live_edit_markers = 0;
        }
        status
    }
}

fn next_partial_quantum_usage(
    old_phase: usize,
    frames: usize,
    old_usage: EndpointQuantumClassUsage,
    events: &[FrameEvent],
    classes: &[EndpointEventClass],
    new_phase: usize,
) -> EndpointQuantumClassUsage {
    debug_assert_eq!(events.len(), classes.len());
    if new_phase == 0 {
        return EndpointQuantumClassUsage::default();
    }
    let completed_quanta = (old_phase + frames) / DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES;
    let final_quantum = completed_quanta;
    let mut usage = if completed_quanta == 0 {
        old_usage
    } else {
        EndpointQuantumClassUsage::default()
    };
    for (event, class) in events.iter().copied().zip(classes.iter().copied()) {
        let quantum =
            (old_phase + usize::from(event.sample_offset)) / DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES;
        if quantum == final_quantum {
            usage.increment(class);
        }
    }
    usage
}

struct InsertEndpointSlot {
    endpoint_id: u64,
    endpoint: PreparedFixedEndpoint,
    suppress_output_frames: usize,
}

struct GeneratorEndpointSlot {
    channel_id: u32,
    endpoint_id: u64,
    plugin_instance_id: u64,
    mixer_track: usize,
    endpoint: PreparedFixedEndpoint,
    pdc_delay: StereoDelayLine,
    pdc_initialized: bool,
    suppress_output_frames: usize,
}

struct MasterCaptureSlot {
    capture_id: u64,
    endpoint: MasterCaptureEndpoint,
}

struct RetiredEndpointResource {
    _endpoint: PreparedFixedEndpoint,
    _pdc_delay: Option<StereoDelayLine>,
}

struct RetiredMidiInputResource {
    _prepared: PreparedMidiInputRoute,
}

struct RealtimeMidiInputRoute {
    route_id: u64,
    prepared: PreparedMidiInputRoute,
    mapper: Option<MidiTimestampMapper>,
    pending_future: Option<LiveMidiEvent>,
    scratch: MidiEventScratch,
    observed_transport_epoch: u64,
    discard_until_empty: bool,
    panic_deferred: bool,
}

struct RealtimeMidiRecordingSlot {
    endpoint: PreparedMidiRecordEndpoint,
    start: MidiRecordClockAnchor,
    input_dropped_noncritical: u64,
    input_rejected_messages: u64,
}

#[derive(Clone, Copy)]
struct PausedMidiMonitorRoute {
    generator_index: usize,
    mixer_track: usize,
}

#[derive(Clone, Copy, Default)]
struct PausedEndpointProgress {
    generator_mask: u64,
    insert_mask: u32,
}

#[derive(Clone, Copy)]
struct PausedMidiSafetyService {
    stamp: MidiGeneratorRouteStamp,
    remaining_frames: usize,
}

impl RealtimeMidiInputRoute {
    fn new(route_id: u64, prepared: PreparedMidiInputRoute, transport_epoch: u64) -> Self {
        Self {
            route_id,
            prepared,
            mapper: None,
            pending_future: None,
            scratch: MidiEventScratch::new(),
            observed_transport_epoch: transport_epoch,
            discard_until_empty: false,
            panic_deferred: false,
        }
    }

    fn reset_timing(&mut self) {
        self.mapper = None;
        self.pending_future = None;
        self.scratch.clear();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EndpointEpochSync {
    AlreadyCurrent,
    Reset,
    Failed,
}

fn synchronize_endpoint_epoch(
    endpoint: &mut PreparedFixedEndpoint,
    target_epoch: u64,
) -> EndpointEpochSync {
    if target_epoch == 0 {
        return EndpointEpochSync::Failed;
    }
    if endpoint.epoch() == target_epoch {
        return EndpointEpochSync::AlreadyCurrent;
    }
    if endpoint.set_epoch(target_epoch) && endpoint.epoch() == target_epoch {
        EndpointEpochSync::Reset
    } else {
        EndpointEpochSync::Failed
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MixerGraphBindingIdentity {
    revision: u64,
    epoch: u64,
    fingerprint: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MixerGraphInsertLatencyIdentity {
    runtime_slot: u8,
    endpoint_id: u64,
    snapshot: PluginEndpointSnapshot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MixerGraphGeneratorLatencyIdentity {
    endpoint_index: u8,
    runtime_slot: u8,
    channel_id: u32,
    endpoint_id: u64,
    plugin_instance_id: u64,
    snapshot: PluginEndpointSnapshot,
}

/// Fixed callback scratch proving that every worker contributing to one graph
/// PDC plan still publishes the exact latency generation used to build it.
#[derive(PartialEq, Eq)]
struct MixerGraphEndpointIdentityTable {
    inserts: [Option<MixerGraphInsertLatencyIdentity>; TRACK_COUNT],
    generators: [Option<MixerGraphGeneratorLatencyIdentity>; MAX_GENERATOR_ENDPOINTS],
    insert_count: usize,
    generator_count: usize,
}

impl MixerGraphEndpointIdentityTable {
    fn new() -> Self {
        Self {
            inserts: [None; TRACK_COUNT],
            generators: [None; MAX_GENERATOR_ENDPOINTS],
            insert_count: 0,
            generator_count: 0,
        }
    }

    fn clear(&mut self) {
        self.inserts.fill(None);
        self.generators.fill(None);
        self.insert_count = 0;
        self.generator_count = 0;
    }

    fn capture(
        &mut self,
        graph: &CompiledMixerGraph,
        insert_endpoints: &mut [Option<InsertEndpointSlot>; TRACK_COUNT],
        generator_endpoints: &mut [Option<GeneratorEndpointSlot>; MAX_GENERATOR_ENDPOINTS],
    ) -> bool {
        self.clear();
        for node in graph.nodes() {
            let runtime_slot = usize::from(node.runtime_slot);
            let Some(endpoint) = insert_endpoints[runtime_slot].as_mut() else {
                continue;
            };
            let Some(snapshot) = endpoint.endpoint.exact_endpoint_snapshot() else {
                self.clear();
                return false;
            };
            self.inserts[self.insert_count] = Some(MixerGraphInsertLatencyIdentity {
                runtime_slot: node.runtime_slot,
                endpoint_id: endpoint.endpoint_id,
                snapshot,
            });
            self.insert_count += 1;
        }
        for (endpoint_index, endpoint) in generator_endpoints.iter_mut().enumerate() {
            let Some(endpoint) = endpoint else {
                continue;
            };
            let Some(node) = graph
                .nodes()
                .iter()
                .find(|node| usize::from(node.runtime_slot) == endpoint.mixer_track)
            else {
                continue;
            };
            let Some(snapshot) = endpoint.endpoint.exact_endpoint_snapshot() else {
                self.clear();
                return false;
            };
            let Ok(endpoint_index) = u8::try_from(endpoint_index) else {
                self.clear();
                return false;
            };
            self.generators[self.generator_count] = Some(MixerGraphGeneratorLatencyIdentity {
                endpoint_index,
                runtime_slot: node.runtime_slot,
                channel_id: endpoint.channel_id,
                endpoint_id: endpoint.endpoint_id,
                plugin_instance_id: endpoint.plugin_instance_id,
                snapshot,
            });
            self.generator_count += 1;
        }
        true
    }

    fn build_pdc_plan(
        &self,
        graph: &CompiledMixerGraph,
        maximum_delay_samples: u32,
    ) -> Option<GraphPdcPlan> {
        let bridge_latency = (DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES as u64).checked_mul(2)?;
        let mut stage_latencies = [0_u64; MIXER_GRAPH_MAX_NODES];
        for identity in self.inserts[..self.insert_count].iter().flatten() {
            stage_latencies[usize::from(identity.runtime_slot)] = bridge_latency
                .checked_add(u64::from(identity.snapshot.total_plugin_latency_samples()))?;
        }
        let empty_generator = GraphPdcGenerator {
            endpoint_id: 0,
            channel_id: 0,
            destination_id: 0,
            latency_samples: 0,
        };
        let mut generators = [empty_generator; MAX_GENERATOR_ENDPOINTS];
        for (index, identity) in self.generators[..self.generator_count]
            .iter()
            .flatten()
            .enumerate()
        {
            let destination_id = graph
                .nodes()
                .iter()
                .find(|node| node.runtime_slot == identity.runtime_slot)?
                .id;
            generators[index] = GraphPdcGenerator {
                endpoint_id: identity.endpoint_id,
                channel_id: identity.channel_id,
                destination_id,
                latency_samples: bridge_latency
                    .checked_add(u64::from(identity.snapshot.total_plugin_latency_samples()))?,
            };
        }
        GraphPdcPlan::build_for_mixer_graph(
            graph,
            &stage_latencies,
            &generators[..self.generator_count],
            maximum_delay_samples,
        )
        .ok()
        .filter(|plan| {
            !plan.diagnostics().has_arithmetic_overflow()
                && !plan.diagnostics().has_clamped_delays()
        })
    }

    fn matches_fresh(
        &self,
        insert_endpoints: &mut [Option<InsertEndpointSlot>; TRACK_COUNT],
        generator_endpoints: &mut [Option<GeneratorEndpointSlot>; MAX_GENERATOR_ENDPOINTS],
    ) -> bool {
        for identity in self.inserts[..self.insert_count].iter().flatten() {
            let Some(snapshot) = insert_endpoints[usize::from(identity.runtime_slot)]
                .as_mut()
                .filter(|endpoint| endpoint.endpoint_id == identity.endpoint_id)
                .and_then(|endpoint| endpoint.endpoint.exact_endpoint_snapshot())
            else {
                return false;
            };
            if snapshot != identity.snapshot {
                return false;
            }
        }
        for identity in self.generators[..self.generator_count].iter().flatten() {
            let Some(snapshot) = generator_endpoints
                .get_mut(usize::from(identity.endpoint_index))
                .and_then(Option::as_mut)
                .filter(|endpoint| {
                    endpoint.mixer_track == usize::from(identity.runtime_slot)
                        && endpoint.channel_id == identity.channel_id
                        && endpoint.endpoint_id == identity.endpoint_id
                        && endpoint.plugin_instance_id == identity.plugin_instance_id
                })
                .and_then(|endpoint| endpoint.endpoint.exact_endpoint_snapshot())
            else {
                return false;
            };
            if snapshot != identity.snapshot {
                return false;
            }
        }
        true
    }

    fn apply_expected_revisions(
        &self,
        insert_endpoints: &mut [Option<InsertEndpointSlot>; TRACK_COUNT],
        generator_endpoints: &mut [Option<GeneratorEndpointSlot>; MAX_GENERATOR_ENDPOINTS],
    ) {
        for endpoint in insert_endpoints.iter_mut().flatten() {
            endpoint.endpoint.set_expected_latency_revision(0);
        }
        for endpoint in generator_endpoints.iter_mut().flatten() {
            endpoint.endpoint.set_expected_latency_revision(0);
        }
        for identity in self.inserts[..self.insert_count].iter().flatten() {
            let endpoint = insert_endpoints[usize::from(identity.runtime_slot)]
                .as_mut()
                .filter(|endpoint| endpoint.endpoint_id == identity.endpoint_id)
                .expect("preflighted graph insert endpoint remains installed");
            endpoint
                .endpoint
                .set_expected_latency_revision(identity.snapshot.revision());
        }
        for identity in self.generators[..self.generator_count].iter().flatten() {
            let endpoint = generator_endpoints[usize::from(identity.endpoint_index)]
                .as_mut()
                .filter(|endpoint| {
                    endpoint.mixer_track == usize::from(identity.runtime_slot)
                        && endpoint.channel_id == identity.channel_id
                        && endpoint.endpoint_id == identity.endpoint_id
                        && endpoint.plugin_instance_id == identity.plugin_instance_id
                })
                .expect("preflighted graph generator endpoint remains installed");
            endpoint
                .endpoint
                .set_expected_latency_revision(identity.snapshot.revision());
        }
    }
}

struct DspState {
    voices: [Voice; MAX_VOICES],
    next_voice: usize,
    audio_assets: [AudioAssetSlot; MAX_REGISTERED_AUDIO_ASSETS],
    audio_voices: [AudioClipVoice; MAX_AUDIO_CLIP_VOICES],
    next_audio_voice: usize,
    pending_asset_event: Option<AudioAssetEvent>,
    master: f32,
    master_pan: f32,
    sample_rate: f32,
    beat_phase: f32,
    click_envelope: f32,
    click_phase: f32,
    track_gains: [f32; TRACK_COUNT],
    track_pans: [f32; TRACK_COUNT],
    track_muted: [bool; TRACK_COUNT],
    track_solo: [bool; TRACK_COUNT],
    /// Fixed-stride, preallocated stereo buses. Track N starts at
    /// `N * MAX_MIXER_BLOCK_FRAMES`; only the current callback prefix is cleared.
    track_block: Box<[[f32; 2]]>,
    master_block: Box<[[f32; 2]]>,
    meter_publisher: Option<MeterPublisher>,
    meter_graph_rendered: bool,
    pdc_raw_track_delays: Box<[StereoDelayLine]>,
    pdc_plan: PdcPlan,
    mixer_graph_plan: Box<FixedMixerGraphLayout>,
    mixer_graph_activation_plan: Box<FixedMixerGraphLayout>,
    mixer_graph_identity: Option<MixerGraphBindingIdentity>,
    mixer_graph_activation_identity: Option<MixerGraphBindingIdentity>,
    mixer_graph_was_activated: bool,
    graph_pdc_plan: Box<Option<GraphPdcPlan>>,
    graph_pdc_activation_plan: Box<Option<GraphPdcPlan>>,
    graph_pdc_activation_revision: Option<u64>,
    mixer_graph_endpoint_identities: Box<MixerGraphEndpointIdentityTable>,
    mixer_graph_activation_endpoint_identities: Box<MixerGraphEndpointIdentityTable>,
    pdc_plan_revision: u64,
    pdc_maximum_delay_samples: u32,
    transport_epoch: u64,
    plugin_epoch_resets: u64,
    plugin_epoch_reset_failures: u64,
    last_plugin_endpoint_epoch: u64,
    fixed_quantum_event_overflows: u64,
    fixed_quantum_invalid_events: u64,
    fixed_quantum_endpoint_event_rejections: u64,
    fixed_quantum_bridge_gaps: u64,
    fixed_quantum_output_underflow_frames: u64,
    admitted_live_edit_endpoint_count: usize,
    insert_endpoints: [Option<InsertEndpointSlot>; TRACK_COUNT],
    generator_endpoints: [Option<GeneratorEndpointSlot>; MAX_GENERATOR_ENDPOINTS],
    master_capture: Option<MasterCaptureSlot>,
    device_frame: u64,
    retired_insert_endpoints: Option<Producer<RetiredEndpointResource>>,
    insert_endpoint_events: Option<Producer<InsertEndpointEvent>>,
    generator_endpoint_events: Option<Producer<GeneratorEndpointEvent>>,
    midi_input: Option<RealtimeMidiInputRoute>,
    midi_recording: Option<RealtimeMidiRecordingSlot>,
    pending_midi_recording_start: Option<PreparedMidiRecordEndpoint>,
    midi_recording_endpoint_events: Option<Producer<MidiRecordingEndpointEvent>>,
    midi_recording_event_reservations: usize,
    callback_transport_anchor: MidiRecordClockAnchor,
    callback_transport_playing: bool,
    paused_midi_safety: [Option<PausedMidiSafetyService>; MAX_GENERATOR_ENDPOINTS],
    retired_midi_inputs: Option<Producer<RetiredMidiInputResource>>,
    midi_input_route_events: Option<Producer<MidiInputRouteEvent>>,
    parameter_edit_callback_events: Option<Producer<CallbackEditReceipt>>,
    parameter_edit_callback_admission: Arc<AtomicU32>,
    master_capture_events: Option<Producer<MasterCaptureEndpointEvent>>,
    timeline_runtime: Option<RealtimeTimelineRuntime>,
    timeline_executor: TimelineExecutor,
    timeline_automation: RealtimeTimelineAutomation,
    timeline_packet: Box<TimelinePacket<TIMELINE_PACKET_CAPACITY>>,
    timeline_plan: Box<TimelineRenderPlan>,
    timeline_activation_plan: Box<TimelineRenderPlan>,
    timeline_automation_values: TimelineAutomationValueMatrix,
    timeline_automation_staged_values: TimelineAutomationValueMatrix,
    timeline_plan_has_chase: bool,
    timeline_plan_render_active: bool,
    timeline_render_start_frame: u64,
    timeline_transport_beat_q32: u64,
    timeline_generator_notes: Box<[Option<TimelineGeneratorNote>; MAX_ACTIVE_NOTES]>,
    timeline_generator_staged_notes: Box<[Option<TimelineGeneratorNote>; MAX_ACTIVE_NOTES]>,
    timeline_channel_bases: TimelineChannelBaseTable,
    timeline_activation_channel_bases: TimelineChannelBaseTable,
    timeline_generator_routes: TimelineGeneratorRouteTable,
    timeline_activation_generator_routes: TimelineGeneratorRouteTable,
    timeline_plugin_automation_bindings: TimelinePluginAutomationBindings,
    timeline_activation_plugin_automation_bindings: TimelinePluginAutomationBindings,
    timeline_plugin_control_histories: Box<[Q128ControlHistory]>,
    timeline_endpoint_batch: Box<TimelineEndpointBatchPlan>,
    timeline_channel_revision: Option<u64>,
    timeline_channel_epoch: Option<u64>,
    timeline_execution_failures: u64,
    timeline_missing_assets: u64,
    timeline_automation_pending: u64,
    timeline_automation_unsupported: u64,
    plugin_input_left: Box<[f32]>,
    plugin_input_right: Box<[f32]>,
    plugin_output_left: Box<[f32]>,
    plugin_output_right: Box<[f32]>,
}

fn boxed_timeline_generator_notes() -> Box<[Option<TimelineGeneratorNote>; MAX_ACTIVE_NOTES]> {
    match vec![None; MAX_ACTIVE_NOTES].into_boxed_slice().try_into() {
        Ok(notes) => notes,
        Err(_) => unreachable!("generator note table uses the requested fixed length"),
    }
}

impl DspState {
    #[cfg(test)]
    fn new(sample_rate: f32) -> Self {
        Self::try_new_inner(sample_rate, None, None, None, None, 512, None).unwrap()
    }

    #[cfg(test)]
    fn new_with_insert_io(
        sample_rate: f32,
        retired_insert_endpoints: Producer<RetiredEndpointResource>,
        insert_endpoint_events: Producer<InsertEndpointEvent>,
    ) -> Self {
        Self::try_new_inner(
            sample_rate,
            Some(retired_insert_endpoints),
            Some(insert_endpoint_events),
            None,
            None,
            512,
            None,
        )
        .unwrap()
    }

    #[cfg(test)]
    fn new_with_endpoint_io(
        sample_rate: f32,
        retired_insert_endpoints: Producer<RetiredEndpointResource>,
        insert_endpoint_events: Producer<InsertEndpointEvent>,
        generator_endpoint_events: Producer<GeneratorEndpointEvent>,
    ) -> Self {
        Self::try_new_inner(
            sample_rate,
            Some(retired_insert_endpoints),
            Some(insert_endpoint_events),
            Some(generator_endpoint_events),
            None,
            512,
            None,
        )
        .unwrap()
    }

    #[cfg(test)]
    fn new_with_master_capture_io(
        sample_rate: f32,
        master_capture_events: Producer<MasterCaptureEndpointEvent>,
    ) -> Self {
        Self::try_new_inner(
            sample_rate,
            None,
            None,
            None,
            Some(master_capture_events),
            512,
            None,
        )
        .unwrap()
    }

    fn try_new_with_endpoint_io(
        sample_rate: f32,
        retired_insert_endpoints: Producer<RetiredEndpointResource>,
        insert_endpoint_events: Producer<InsertEndpointEvent>,
        generator_endpoint_events: Producer<GeneratorEndpointEvent>,
        master_capture_events: Producer<MasterCaptureEndpointEvent>,
        pdc_maximum_delay_samples: u32,
        timeline_runtime: RealtimeTimelineRuntime,
    ) -> Result<Self> {
        Self::try_new_inner(
            sample_rate,
            Some(retired_insert_endpoints),
            Some(insert_endpoint_events),
            Some(generator_endpoint_events),
            Some(master_capture_events),
            pdc_maximum_delay_samples,
            Some(timeline_runtime),
        )
    }

    fn try_new_inner(
        sample_rate: f32,
        retired_insert_endpoints: Option<Producer<RetiredEndpointResource>>,
        insert_endpoint_events: Option<Producer<InsertEndpointEvent>>,
        generator_endpoint_events: Option<Producer<GeneratorEndpointEvent>>,
        master_capture_events: Option<Producer<MasterCaptureEndpointEvent>>,
        pdc_maximum_delay_samples: u32,
        timeline_runtime: Option<RealtimeTimelineRuntime>,
    ) -> Result<Self> {
        let mut pdc_raw_track_delays = Vec::with_capacity(TRACK_COUNT);
        for _ in 0..TRACK_COUNT {
            pdc_raw_track_delays.push(StereoDelayLine::new(pdc_maximum_delay_samples)?);
        }
        let pdc_plan = PdcPlan::build([0; TRACK_COUNT], 0, &[], pdc_maximum_delay_samples)?;
        let mut timeline_plugin_control_histories =
            Vec::with_capacity(TIMELINE_MAX_DRIVEN_PLUGIN_PARAMETERS);
        for _ in 0..TIMELINE_MAX_DRIVEN_PLUGIN_PARAMETERS {
            timeline_plugin_control_histories
                .push(Q128ControlHistory::new(pdc_maximum_delay_samples)?);
        }

        Ok(Self {
            voices: [Voice::default(); MAX_VOICES],
            next_voice: 0,
            audio_assets: std::array::from_fn(|_| AudioAssetSlot::default()),
            audio_voices: [AudioClipVoice::default(); MAX_AUDIO_CLIP_VOICES],
            next_audio_voice: 0,
            pending_asset_event: None,
            master: 0.72,
            master_pan: 0.0,
            sample_rate,
            beat_phase: 0.0,
            click_envelope: 0.0,
            click_phase: 0.0,
            track_gains: [1.0; TRACK_COUNT],
            track_pans: [0.0; TRACK_COUNT],
            track_muted: [false; TRACK_COUNT],
            track_solo: [false; TRACK_COUNT],
            track_block: vec![[0.0; 2]; TRACK_COUNT * MAX_MIXER_BLOCK_FRAMES].into_boxed_slice(),
            meter_publisher: None,
            meter_graph_rendered: false,
            master_block: vec![[0.0; 2]; MAX_MIXER_BLOCK_FRAMES].into_boxed_slice(),
            pdc_raw_track_delays: pdc_raw_track_delays.into_boxed_slice(),
            pdc_plan,
            mixer_graph_plan: Box::new(FixedMixerGraphLayout::default()),
            mixer_graph_activation_plan: Box::new(FixedMixerGraphLayout::default()),
            mixer_graph_identity: None,
            mixer_graph_activation_identity: None,
            mixer_graph_was_activated: false,
            graph_pdc_plan: Box::new(None),
            graph_pdc_activation_plan: Box::new(None),
            graph_pdc_activation_revision: None,
            mixer_graph_endpoint_identities: Box::new(MixerGraphEndpointIdentityTable::new()),
            mixer_graph_activation_endpoint_identities: Box::new(
                MixerGraphEndpointIdentityTable::new(),
            ),
            pdc_plan_revision: 0,
            pdc_maximum_delay_samples,
            transport_epoch: 1,
            plugin_epoch_resets: 0,
            plugin_epoch_reset_failures: 0,
            last_plugin_endpoint_epoch: 0,
            fixed_quantum_event_overflows: 0,
            fixed_quantum_invalid_events: 0,
            fixed_quantum_endpoint_event_rejections: 0,
            fixed_quantum_bridge_gaps: 0,
            fixed_quantum_output_underflow_frames: 0,
            admitted_live_edit_endpoint_count: 0,
            insert_endpoints: std::array::from_fn(|_| None),
            generator_endpoints: std::array::from_fn(|_| None),
            master_capture: None,
            device_frame: 0,
            retired_insert_endpoints,
            insert_endpoint_events,
            generator_endpoint_events,
            midi_input: None,
            midi_recording: None,
            pending_midi_recording_start: None,
            midi_recording_endpoint_events: None,
            midi_recording_event_reservations: 0,
            callback_transport_anchor: MidiRecordClockAnchor {
                device_frame: 0,
                timeline_frame: 0,
                transport_epoch: 1,
                loop_count: 0,
            },
            callback_transport_playing: false,
            paused_midi_safety: [None; MAX_GENERATOR_ENDPOINTS],
            retired_midi_inputs: None,
            midi_input_route_events: None,
            parameter_edit_callback_events: None,
            parameter_edit_callback_admission: Arc::new(AtomicU32::new(0)),
            master_capture_events,
            timeline_runtime,
            timeline_executor: TimelineExecutor::new(),
            timeline_automation: RealtimeTimelineAutomation::new(),
            timeline_packet: TimelinePacket::new_boxed(),
            timeline_plan: Box::new(TimelineRenderPlan::new()),
            timeline_activation_plan: Box::new(TimelineRenderPlan::new()),
            timeline_automation_values: TimelineAutomationValueMatrix::new(),
            timeline_automation_staged_values: TimelineAutomationValueMatrix::new(),
            timeline_plan_has_chase: false,
            timeline_plan_render_active: false,
            timeline_render_start_frame: 0,
            timeline_transport_beat_q32: 0,
            timeline_generator_notes: boxed_timeline_generator_notes(),
            timeline_generator_staged_notes: boxed_timeline_generator_notes(),
            timeline_channel_bases: TimelineChannelBaseTable::new(),
            timeline_activation_channel_bases: TimelineChannelBaseTable::new(),
            timeline_generator_routes: TimelineGeneratorRouteTable::new(),
            timeline_activation_generator_routes: TimelineGeneratorRouteTable::new(),
            timeline_plugin_automation_bindings: TimelinePluginAutomationBindings::new(),
            timeline_activation_plugin_automation_bindings: TimelinePluginAutomationBindings::new(),
            timeline_plugin_control_histories: timeline_plugin_control_histories.into_boxed_slice(),
            timeline_endpoint_batch: TimelineEndpointBatchPlan::new_boxed(),
            timeline_channel_revision: None,
            timeline_channel_epoch: None,
            timeline_execution_failures: 0,
            timeline_missing_assets: 0,
            timeline_automation_pending: 0,
            timeline_automation_unsupported: 0,
            plugin_input_left: vec![0.0; MAX_MIXER_BLOCK_FRAMES].into_boxed_slice(),
            plugin_input_right: vec![0.0; MAX_MIXER_BLOCK_FRAMES].into_boxed_slice(),
            plugin_output_left: vec![0.0; MAX_MIXER_BLOCK_FRAMES].into_boxed_slice(),
            plugin_output_right: vec![0.0; MAX_MIXER_BLOCK_FRAMES].into_boxed_slice(),
        })
    }

    /// Services the bounded ownership/lifecycle mailbox and invalidates any
    /// callback-owned render state whose active timeline identity changed.
    fn apply_pending_timeline_commands(&mut self) -> usize {
        let applied = self
            .timeline_runtime
            .as_mut()
            .map_or(0, RealtimeTimelineRuntime::apply_pending_at_block_boundary);
        let active_revision = self
            .timeline_runtime
            .as_ref()
            .and_then(RealtimeTimelineRuntime::active_revision);
        let active_epoch = self
            .timeline_runtime
            .as_ref()
            .and_then(RealtimeTimelineRuntime::active_epoch);
        if self.timeline_channel_revision.is_some()
            && (self.timeline_channel_revision != active_revision
                || self.timeline_channel_epoch != active_epoch)
        {
            self.clear_timeline_render_binding();
        }
        applied
    }

    fn clear_timeline_render_binding(&mut self) {
        self.clear_timeline_expected_latency_revisions();
        *self.mixer_graph_plan = FixedMixerGraphLayout::default();
        *self.mixer_graph_activation_plan = FixedMixerGraphLayout::default();
        self.mixer_graph_identity = None;
        self.mixer_graph_activation_identity = None;
        *self.graph_pdc_plan = None;
        *self.graph_pdc_activation_plan = None;
        self.graph_pdc_activation_revision = None;
        self.mixer_graph_endpoint_identities.clear();
        self.mixer_graph_activation_endpoint_identities.clear();
        for delay in &mut self.pdc_raw_track_delays {
            delay.reset();
        }
        self.timeline_channel_bases.clear();
        self.timeline_generator_routes.clear();
        self.timeline_plugin_automation_bindings.clear();
        self.timeline_activation_plugin_automation_bindings.clear();
        self.timeline_channel_revision = None;
        self.timeline_channel_epoch = None;
        self.timeline_plan.clear();
        self.timeline_plan_has_chase = false;
        self.timeline_plan_render_active = false;
        self.timeline_automation_pending = 0;
        for voice in &mut self.voices {
            if voice.timeline_note_id.is_some() {
                voice.active = false;
                voice.timeline_note_id = None;
                voice.timeline_channel_id = None;
            }
        }
        for voice in &mut self.audio_voices {
            if voice.timeline_asset_id.is_some() {
                voice.active = false;
            }
        }
        if self.timeline_generator_notes.iter().any(Option::is_some) {
            for slot in self.generator_endpoints.iter_mut().flatten() {
                let deferred = slot.endpoint.clear_and_stage_all_notes_off();
                slot.suppress_output_frames = slot
                    .suppress_output_frames
                    .max(fixed_quantum_fail_closed_frames(&slot.endpoint).saturating_add(deferred));
            }
        }
        self.timeline_generator_notes.fill(None);
        self.timeline_generator_staged_notes.fill(None);
    }

    fn pause_timeline(&mut self) {
        if self
            .timeline_runtime
            .as_ref()
            .and_then(RealtimeTimelineRuntime::active_epoch)
            .is_none()
        {
            return;
        }
        self.timeline_plan.clear();
        self.timeline_plan_has_chase = false;
        self.timeline_plan_render_active = false;
        self.timeline_automation_pending = 0;
        for voice in &mut self.voices {
            if voice.timeline_note_id.is_some() {
                voice.active = false;
                voice.timeline_note_id = None;
                voice.timeline_channel_id = None;
            }
        }
        for voice in &mut self.audio_voices {
            if voice.timeline_asset_id.is_some() {
                voice.active = false;
            }
        }
        if self.timeline_generator_notes.iter().any(Option::is_some) {
            for slot in self.generator_endpoints.iter_mut().flatten() {
                let deferred = slot.endpoint.clear_and_stage_all_notes_off();
                slot.suppress_output_frames = slot
                    .suppress_output_frames
                    .max(fixed_quantum_fail_closed_frames(&slot.endpoint).saturating_add(deferred));
            }
        }
        self.timeline_generator_notes.fill(None);
        self.timeline_generator_staged_notes.fill(None);
    }

    /// Builds one callback render transaction. Runtime cursor ownership is
    /// committed only after every packet chunk, executor finish, render-plan
    /// capacity check, and generator MIDI staging have succeeded.
    fn prepare_timeline_render(
        &mut self,
        epoch: u64,
        start_frame: u64,
        beat_q32: u64,
        frames: usize,
        playing: bool,
    ) -> bool {
        self.timeline_transport_beat_q32 = beat_q32;
        let Some(revision) = self
            .timeline_runtime
            .as_ref()
            .and_then(RealtimeTimelineRuntime::active_revision)
        else {
            self.timeline_plan_render_active = false;
            return true;
        };
        let Some(active_epoch) = self
            .timeline_runtime
            .as_ref()
            .and_then(RealtimeTimelineRuntime::active_epoch)
        else {
            // A resident timeline has not taken transport ownership until a
            // prepared chase is atomically activated. Legacy note/audio
            // commands remain authoritative throughout this initial install
            // window and must continue to render.
            self.timeline_plan_render_active = false;
            return true;
        };
        if self.mixer_graph_was_activated && !self.mixer_graph_binding_is_exact() {
            self.fail_mixer_graph_render(0);
            self.timeline_plan_render_active = false;
            return false;
        }
        if self
            .timeline_runtime
            .as_ref()
            .is_some_and(|runtime| runtime.stats().ownership_needs_resync)
        {
            // The block that detected the callback transaction failure already
            // cleared all timeline-owned voices and failed closed. Subsequent
            // blocks return authority to the explicitly selected legacy
            // fallback while the control plane prepares a fresh chase.
            self.timeline_plan_render_active = false;
            return true;
        }
        if !playing {
            // A paused callback may still service ownership commands, but it
            // never reads events, moves either runtime/executor cursor, or
            // advances timeline-owned native/plugin/audio voice state.
            self.timeline_plan_render_active = false;
            return false;
        }
        if active_epoch != epoch {
            // A previously activated callback timeline remains authoritative
            // across an in-flight discontinuity. Never reopen the legacy
            // scheduler for an epoch that has not atomically activated yet.
            self.timeline_plan_render_active = false;
            return false;
        }
        let Ok(frames_u32) = u32::try_from(frames) else {
            self.fail_timeline_block();
            return false;
        };
        if !self.timeline_plan_has_chase {
            self.timeline_plan.clear();
        }
        if !self.timeline_plan.prepare_automation_block(frames_u32) {
            self.fail_timeline_block();
            return false;
        }

        let succeeded = 'transaction: {
            let DspState {
                timeline_runtime,
                timeline_executor,
                timeline_automation,
                timeline_packet,
                timeline_plan,
                timeline_automation_values,
                timeline_automation_staged_values,
                timeline_channel_bases,
                insert_endpoints,
                generator_endpoints,
                timeline_generator_notes,
                timeline_generator_staged_notes,
                timeline_generator_routes,
                timeline_plugin_automation_bindings,
                timeline_plugin_control_histories,
                timeline_endpoint_batch,
                pdc_plan_revision,
                ..
            } = self;
            let runtime = timeline_runtime
                .as_mut()
                .expect("active revision requires a realtime timeline runtime");
            let mut block =
                match runtime.begin_chunked_block(revision, epoch, start_frame, frames_u32) {
                    Ok(block) => block,
                    Err(_) => break 'transaction false,
                };

            loop {
                let chunk = match block.packetize_next_into(timeline_packet.as_mut()) {
                    Ok(chunk) => chunk,
                    Err(_) => {
                        block.abort();
                        break 'transaction false;
                    }
                };
                if timeline_executor
                    .process_packet(timeline_packet.as_ref(), timeline_plan.as_mut())
                    .is_err()
                {
                    block.abort();
                    break 'transaction false;
                }
                if chunk.remaining_events == 0 {
                    break;
                }
            }
            if timeline_executor
                .finish_block(start_frame, frames_u32, timeline_plan.as_mut())
                .is_err()
                || timeline_plan.overflowed
            {
                timeline_automation.abort_block();
                block.abort();
                break 'transaction false;
            }
            if !timeline_automation_staged_values.begin(frames)
                || timeline_automation
                    .begin_block(epoch, start_frame, &timeline_plan.automation_block)
                    .is_err()
            {
                timeline_automation.abort_block();
                block.abort();
                break 'transaction false;
            }
            let mut automation_rendered = true;
            for offset in 0..frames_u32 {
                let mut slot = 0_usize;
                let mut matrix_valid = true;
                if timeline_automation
                    .render_frame(offset, |target, value| {
                        if !timeline_automation_staged_values.write(
                            offset as usize,
                            slot,
                            target,
                            value,
                        ) {
                            matrix_valid = false;
                        }
                        slot = slot.saturating_add(1);
                    })
                    .is_err()
                    || !matrix_valid
                    || !timeline_automation_staged_values.finish_frame(offset as usize, slot)
                {
                    automation_rendered = false;
                    break;
                }
            }
            if !automation_rendered
                || timeline_automation.finish_block().is_err()
                || !timeline_automation_staged_values.finish()
                || !timeline_channel_bases.automation_slots_match(timeline_automation_staged_values)
            {
                timeline_automation.abort_block();
                block.abort();
                break 'transaction false;
            }
            if !Self::prepare_timeline_endpoint_batch(
                timeline_plan.as_mut(),
                Some(timeline_automation_staged_values),
                generator_endpoints,
                insert_endpoints,
                timeline_generator_notes,
                timeline_generator_staged_notes,
                timeline_generator_routes,
                timeline_plugin_automation_bindings,
                timeline_plugin_control_histories,
                revision,
                active_epoch,
                frames,
                false,
                None,
                *pdc_plan_revision,
                timeline_endpoint_batch.as_mut(),
            ) {
                timeline_automation.abort_block();
                block.abort();
                break 'transaction false;
            }
            if !Self::preflight_timeline_endpoint_batch_commit(
                timeline_endpoint_batch.as_ref(),
                generator_endpoints,
                insert_endpoints,
                timeline_plugin_automation_bindings,
                *pdc_plan_revision,
            ) {
                timeline_automation.abort_block();
                timeline_endpoint_batch.abort();
                block.abort();
                break 'transaction false;
            }
            if block.commit().is_err() {
                timeline_automation.abort_block();
                timeline_endpoint_batch.abort();
                break 'transaction false;
            }
            timeline_automation
                .commit_block()
                .expect("a fully rendered automation transaction commits infallibly");
            std::mem::swap(
                timeline_automation_values,
                timeline_automation_staged_values,
            );
            Self::commit_timeline_endpoint_batch(
                timeline_endpoint_batch.as_ref(),
                generator_endpoints,
                insert_endpoints,
            );
            timeline_plugin_automation_bindings
                .mark_committed_from_batch(timeline_endpoint_batch.as_ref());
            Self::commit_timeline_plugin_control_histories(
                timeline_plugin_control_histories,
                timeline_plugin_automation_bindings,
                epoch,
                timeline_automation_values,
            );
            std::mem::swap(timeline_generator_notes, timeline_generator_staged_notes);
            timeline_endpoint_batch.reset();
            true
        };

        if !succeeded {
            self.fail_timeline_block();
            return false;
        }
        self.timeline_plan_has_chase = false;
        self.timeline_plan_render_active = true;
        self.timeline_render_start_frame = start_frame;
        self.timeline_plan.begin_render(frames);
        let driven_target_count = self
            .timeline_runtime
            .as_ref()
            .and_then(RealtimeTimelineRuntime::active_timeline)
            .map_or(0, |timeline| {
                timeline.driven_automation_targets().len() as u64
            });
        let unsupported_target_count = driven_target_count.saturating_sub(
            self.timeline_channel_bases
                .applied_automation_target_count()
                .saturating_add(
                    self.timeline_plugin_automation_bindings
                        .applied_automation_target_count(),
                ),
        );
        self.timeline_automation_pending = unsupported_target_count;
        self.timeline_automation_unsupported = self.timeline_automation_unsupported.saturating_add(
            unsupported_target_count.saturating_mul(u64::try_from(frames).unwrap_or(u64::MAX)),
        );
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_timeline_endpoint_batch(
        plan: &mut TimelineRenderPlan,
        matrix: Option<&TimelineAutomationValueMatrix>,
        generator_endpoints: &mut [Option<GeneratorEndpointSlot>; MAX_GENERATOR_ENDPOINTS],
        insert_endpoints: &mut [Option<InsertEndpointSlot>; TRACK_COUNT],
        generator_notes: &[Option<TimelineGeneratorNote>; MAX_ACTIVE_NOTES],
        staged_generator_notes: &mut [Option<TimelineGeneratorNote>; MAX_ACTIVE_NOTES],
        routes: &TimelineGeneratorRouteTable,
        automation_bindings: &TimelinePluginAutomationBindings,
        control_histories: &mut [Q128ControlHistory],
        revision: u64,
        epoch: u64,
        frames: usize,
        reset_notes: bool,
        endpoint_phase_override: Option<usize>,
        pdc_plan_revision: u64,
        batch: &mut TimelineEndpointBatchPlan,
    ) -> bool {
        batch.reset();
        if reset_notes {
            staged_generator_notes.fill(None);
        } else {
            staged_generator_notes.copy_from_slice(generator_notes);
        }
        if !routes.is_bound_to(revision, epoch)
            || !automation_bindings.is_bound_to(revision, epoch)
            || matrix.is_some_and(|matrix| !automation_bindings.slots_match(matrix))
            || !automation_bindings.identities_match(
                generator_endpoints,
                insert_endpoints,
                pdc_plan_revision,
            )
            || routes.routes().any(|route| {
                !generator_endpoints.iter().flatten().any(|endpoint| {
                    endpoint.channel_id == route.channel_id
                        && endpoint.plugin_instance_id == route.plugin_instance_id
                        && endpoint.mixer_track == route.mixer_track
                        && endpoint.endpoint_id == route.endpoint_id
                })
            })
        {
            return false;
        }

        let mut handles: [Option<(TimelineEndpointKey, TimelineEndpointHandle)>;
            TIMELINE_CALLBACK_MAX_PLUGIN_ENDPOINTS] =
            [None; TIMELINE_CALLBACK_MAX_PLUGIN_ENDPOINTS];
        let mut handle_count = 0_usize;

        for (binding_index, binding) in automation_bindings.iter().enumerate() {
            let key = binding.endpoint.batch_key();
            let Some((endpoint_phase, handle)) = Self::timeline_batch_endpoint(
                generator_endpoints,
                insert_endpoints,
                batch,
                &mut handles,
                &mut handle_count,
                key,
                frames,
                endpoint_phase_override,
            ) else {
                batch.abort();
                return false;
            };
            if let Some(matrix) = matrix {
                let Some(values) = matrix.values_for_slot(binding.matrix_slot, binding.target)
                else {
                    batch.abort();
                    return false;
                };
                let Some(history) = control_histories.get_mut(binding_index) else {
                    batch.abort();
                    return false;
                };
                let history_start = history.next_frame();
                let Ok(history_block) = history.begin_block(epoch, history_start, values) else {
                    batch.abort();
                    return false;
                };
                for boundary_index in 0..history_block.boundary_count() {
                    let Some(boundary_frame) = history_block.boundary_frame(boundary_index) else {
                        batch.abort();
                        return false;
                    };
                    let Some(boundary) = boundary_frame
                        .checked_sub(history_start)
                        .and_then(|offset| usize::try_from(offset).ok())
                    else {
                        batch.abort();
                        return false;
                    };
                    let Some(value) = history_block.delayed_value(boundary_index) else {
                        batch.abort();
                        return false;
                    };
                    let Ok(sample_offset) = u16::try_from(boundary) else {
                        batch.abort();
                        return false;
                    };
                    if (endpoint_phase + boundary) % DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES != 0 {
                        batch.abort();
                        return false;
                    }
                    if batch
                        .push_parameter_at_slot_q128(
                            handle,
                            sample_offset,
                            binding.parameter_slot,
                            binding.parameter_id,
                            value.clamp(0.0, 1.0),
                        )
                        .is_err()
                    {
                        batch.abort();
                        return false;
                    }
                }
            } else {
                let Some(value) = plan
                    .automation_chase_values()
                    .iter()
                    .find_map(|value| (value.target == binding.target).then_some(value.value))
                else {
                    batch.abort();
                    return false;
                };
                if batch
                    .push_parameter_at_slot_q128(
                        handle,
                        0,
                        binding.parameter_slot,
                        binding.parameter_id,
                        value.clamp(0.0, 1.0),
                    )
                    .is_err()
                {
                    batch.abort();
                    return false;
                }
            }
        }

        for event_index in 0..plan.len {
            let Some(mut event) = plan.events[event_index] else {
                batch.abort();
                return false;
            };
            match event.kind {
                TimelinePlannedEventKind::NoteOn(note) => {
                    let Some(route) = routes.route_for_channel(note.channel_id) else {
                        // Compiled-native channels stay native even if a stale
                        // endpoint for the same model channel remains queued.
                        continue;
                    };
                    let key = TimelineEndpointKey::new(
                        route.channel_id,
                        route.endpoint_id,
                        route.plugin_instance_id,
                    );
                    let Some((_endpoint, handle)) = Self::timeline_batch_endpoint(
                        generator_endpoints,
                        insert_endpoints,
                        batch,
                        &mut handles,
                        &mut handle_count,
                        key,
                        frames,
                        endpoint_phase_override,
                    ) else {
                        batch.abort();
                        return false;
                    };
                    if staged_generator_notes
                        .iter()
                        .flatten()
                        .any(|active| active.note_id == note.note_id)
                    {
                        batch.abort();
                        return false;
                    }
                    let Some(binding_index) =
                        staged_generator_notes.iter().position(Option::is_none)
                    else {
                        batch.abort();
                        return false;
                    };
                    let velocity = (note.velocity * note.gain).clamp(0.0, 1.0);
                    let velocity = ((velocity * 127.0).round() as u8).max(1);
                    let midi =
                        FrameEvent::midi(event.sample_offset, None, [0x90, note.note, velocity]);
                    if batch
                        .push_event(handle, EndpointEventClass::Timeline, midi)
                        .is_err()
                    {
                        batch.abort();
                        return false;
                    }
                    staged_generator_notes[binding_index] = Some(TimelineGeneratorNote {
                        note_id: note.note_id,
                        channel_id: note.channel_id,
                        note: note.note,
                        endpoint_id: route.endpoint_id,
                        plugin_instance_id: route.plugin_instance_id,
                    });
                    event.kind = TimelinePlannedEventKind::GeneratorNoteOn;
                    plan.events[event_index] = Some(event);
                }
                TimelinePlannedEventKind::NoteOff(note) => {
                    let Some(binding_index) = staged_generator_notes.iter().position(|active| {
                        active.is_some_and(|active| active.note_id == note.note_id)
                    }) else {
                        continue;
                    };
                    let binding = staged_generator_notes[binding_index]
                        .take()
                        .expect("located generator note binding remains active");
                    let last_pitch = !staged_generator_notes.iter().flatten().any(|active| {
                        active.channel_id == binding.channel_id
                            && active.note == binding.note
                            && active.endpoint_id == binding.endpoint_id
                            && active.plugin_instance_id == binding.plugin_instance_id
                    });
                    if last_pitch {
                        let key = TimelineEndpointKey::new(
                            binding.channel_id,
                            binding.endpoint_id,
                            binding.plugin_instance_id,
                        );
                        let Some((_endpoint, handle)) = Self::timeline_batch_endpoint(
                            generator_endpoints,
                            insert_endpoints,
                            batch,
                            &mut handles,
                            &mut handle_count,
                            key,
                            frames,
                            endpoint_phase_override,
                        ) else {
                            batch.abort();
                            return false;
                        };
                        let midi =
                            FrameEvent::midi(event.sample_offset, None, [0x80, binding.note, 0]);
                        if batch
                            .push_event(handle, EndpointEventClass::Timeline, midi)
                            .is_err()
                        {
                            batch.abort();
                            return false;
                        }
                    }
                    event.kind = TimelinePlannedEventKind::GeneratorNoteOff;
                    plan.events[event_index] = Some(event);
                }
                TimelinePlannedEventKind::GeneratorNoteOn
                | TimelinePlannedEventKind::GeneratorNoteOff
                | TimelinePlannedEventKind::AudioStart(_)
                | TimelinePlannedEventKind::AudioStop(_) => {}
            }
        }
        if batch.preflight().is_err() {
            batch.abort();
            return false;
        }
        true
    }

    fn commit_timeline_plugin_control_histories(
        control_histories: &mut [Q128ControlHistory],
        automation_bindings: &TimelinePluginAutomationBindings,
        epoch: u64,
        matrix: &TimelineAutomationValueMatrix,
    ) {
        for (binding_index, binding) in automation_bindings.iter().enumerate() {
            let history = control_histories
                .get_mut(binding_index)
                .expect("prevalidated plug-in control history remains allocated");
            let values = matrix
                .values_for_slot(binding.matrix_slot, binding.target)
                .expect("prevalidated automation matrix retains its target stream");
            history
                .begin_block(epoch, history.next_frame(), values)
                .expect("prevalidated plug-in control history commits infallibly")
                .commit_block();
        }
    }

    fn timeline_plugin_control_histories_can_reset(
        control_histories: &[Q128ControlHistory],
        automation_bindings: &TimelinePluginAutomationBindings,
        plan: &TimelineRenderPlan,
    ) -> bool {
        automation_bindings
            .iter()
            .enumerate()
            .all(|(index, binding)| {
                control_histories.get(index).is_some_and(|history| {
                    binding.control_delay_samples <= history.maximum_delay_samples()
                        && plan
                            .automation_chase_values()
                            .iter()
                            .any(|value| value.target == binding.target && value.value.is_finite())
                })
            })
    }

    fn reset_timeline_plugin_control_histories(
        control_histories: &mut [Q128ControlHistory],
        automation_bindings: &TimelinePluginAutomationBindings,
        epoch: u64,
        plan: &TimelineRenderPlan,
    ) {
        for (index, binding) in automation_bindings.iter().enumerate() {
            let initial_value = plan
                .automation_chase_values()
                .iter()
                .find_map(|value| (value.target == binding.target).then_some(value.value))
                .expect("prevalidated plug-in automation chase value remains present");
            control_histories[index]
                .reset(epoch, initial_value, binding.control_delay_samples)
                .expect("prevalidated plug-in control history reset is infallible");
        }
    }

    #[cfg(test)]
    fn apply_timeline_expected_latency_revisions(
        automation_bindings: &TimelinePluginAutomationBindings,
        generator_endpoints: &mut [Option<GeneratorEndpointSlot>; MAX_GENERATOR_ENDPOINTS],
        insert_endpoints: &mut [Option<InsertEndpointSlot>; TRACK_COUNT],
    ) {
        for endpoint in generator_endpoints.iter_mut().flatten() {
            endpoint.endpoint.set_expected_latency_revision(0);
        }
        for endpoint in insert_endpoints.iter_mut().flatten() {
            endpoint.endpoint.set_expected_latency_revision(0);
        }
        for binding in automation_bindings.iter() {
            let expected_revision = binding.endpoint_snapshot.revision();
            match binding.endpoint {
                TimelinePluginEndpointIdentity::Generator {
                    channel_id,
                    endpoint_id,
                    plugin_instance_id,
                } => {
                    let endpoint = generator_endpoints
                        .iter_mut()
                        .flatten()
                        .find(|endpoint| {
                            endpoint.channel_id == channel_id
                                && endpoint.endpoint_id == endpoint_id
                                && endpoint.plugin_instance_id == plugin_instance_id
                        })
                        .expect("prevalidated generator endpoint remains installed");
                    debug_assert!(
                        endpoint.endpoint.expected_latency_revision() == 0
                            || endpoint.endpoint.expected_latency_revision() == expected_revision
                    );
                    endpoint
                        .endpoint
                        .set_expected_latency_revision(expected_revision);
                }
                TimelinePluginEndpointIdentity::MixerInsert { track, endpoint_id } => {
                    let endpoint = insert_endpoints[usize::from(track)]
                        .as_mut()
                        .filter(|endpoint| endpoint.endpoint_id == endpoint_id)
                        .expect("prevalidated mixer endpoint remains installed");
                    debug_assert!(
                        endpoint.endpoint.expected_latency_revision() == 0
                            || endpoint.endpoint.expected_latency_revision() == expected_revision
                    );
                    endpoint
                        .endpoint
                        .set_expected_latency_revision(expected_revision);
                }
            }
        }
    }

    fn clear_timeline_expected_latency_revisions(&mut self) {
        for endpoint in self.generator_endpoints.iter_mut().flatten() {
            endpoint.endpoint.set_expected_latency_revision(0);
        }
        for endpoint in self.insert_endpoints.iter_mut().flatten() {
            endpoint.endpoint.set_expected_latency_revision(0);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn timeline_batch_endpoint(
        generator_endpoints: &mut [Option<GeneratorEndpointSlot>; MAX_GENERATOR_ENDPOINTS],
        insert_endpoints: &mut [Option<InsertEndpointSlot>; TRACK_COUNT],
        batch: &mut TimelineEndpointBatchPlan,
        handles: &mut [Option<(TimelineEndpointKey, TimelineEndpointHandle)>;
                 TIMELINE_CALLBACK_MAX_PLUGIN_ENDPOINTS],
        handle_count: &mut usize,
        key: TimelineEndpointKey,
        frames: usize,
        endpoint_phase_override: Option<usize>,
    ) -> Option<(usize, TimelineEndpointHandle)> {
        let (endpoint_phase, seed) = match key.address() {
            TimelineEndpointAddress::Generator {
                channel_id,
                plugin_instance_id,
            } => {
                let endpoint = generator_endpoints.iter_mut().flatten().find(|endpoint| {
                    endpoint.channel_id == channel_id
                        && endpoint.endpoint_id == key.endpoint_id()
                        && endpoint.plugin_instance_id == plugin_instance_id
                })?;
                let phase = endpoint_phase_override
                    .unwrap_or_else(|| endpoint.endpoint.input_phase_frames());
                let seed = if phase == 0 {
                    TimelineEndpointQuantumUsage::default()
                } else {
                    endpoint.endpoint.timeline_batch_seed()
                };
                (phase, seed)
            }
            TimelineEndpointAddress::MixerInsert { track } => {
                let endpoint = insert_endpoints
                    .get_mut(usize::from(track))
                    .and_then(Option::as_mut)
                    .filter(|endpoint| endpoint.endpoint_id == key.endpoint_id())?;
                let phase = endpoint_phase_override
                    .unwrap_or_else(|| endpoint.endpoint.input_phase_frames());
                let seed = if phase == 0 {
                    TimelineEndpointQuantumUsage::default()
                } else {
                    endpoint.endpoint.timeline_batch_seed()
                };
                (phase, seed)
            }
        };
        let existing_handle = handles[..*handle_count]
            .iter()
            .flatten()
            .find_map(|(candidate, handle)| (*candidate == key).then_some(*handle));
        let handle = if let Some(handle) = existing_handle {
            handle
        } else {
            if *handle_count == handles.len() {
                return None;
            }
            let handle = batch
                .register_endpoint_with_seed(key, endpoint_phase, frames, seed)
                .ok()?;
            handles[*handle_count] = Some((key, handle));
            *handle_count += 1;
            handle
        };
        Some((endpoint_phase, handle))
    }

    fn preflight_timeline_endpoint_batch_commit(
        batch: &TimelineEndpointBatchPlan,
        generator_endpoints: &mut [Option<GeneratorEndpointSlot>; MAX_GENERATOR_ENDPOINTS],
        insert_endpoints: &mut [Option<InsertEndpointSlot>; TRACK_COUNT],
        automation_bindings: &TimelinePluginAutomationBindings,
        pdc_plan_revision: u64,
    ) -> bool {
        // A second coherent read closes the worker-publication window opened
        // while packetization and the per-sample matrix were rendered.
        if !automation_bindings.identities_match(
            generator_endpoints,
            insert_endpoints,
            pdc_plan_revision,
        ) {
            return false;
        }
        for index in 0..batch.endpoint_count() {
            let Some(prepared) = batch.prepared_endpoint_at(index) else {
                return false;
            };
            let identity = prepared.identity();
            let key = identity.key;
            let endpoint = match key.address() {
                TimelineEndpointAddress::Generator {
                    channel_id,
                    plugin_instance_id,
                } => generator_endpoints
                    .iter()
                    .flatten()
                    .find(|endpoint| {
                        endpoint.channel_id == channel_id
                            && endpoint.endpoint_id == key.endpoint_id()
                            && endpoint.plugin_instance_id == plugin_instance_id
                    })
                    .map(|endpoint| &endpoint.endpoint),
                TimelineEndpointAddress::MixerInsert { track } => insert_endpoints
                    .get(usize::from(track))
                    .and_then(Option::as_ref)
                    .filter(|endpoint| endpoint.endpoint_id == key.endpoint_id())
                    .map(|endpoint| &endpoint.endpoint),
            };
            let Some(endpoint) = endpoint else {
                return false;
            };
            if endpoint.input_phase_frames() != usize::from(identity.phase)
                || !endpoint.can_stage_batch(prepared.events(), prepared.classes())
            {
                return false;
            }
        }
        true
    }

    fn commit_timeline_endpoint_batch(
        batch: &TimelineEndpointBatchPlan,
        generator_endpoints: &mut [Option<GeneratorEndpointSlot>; MAX_GENERATOR_ENDPOINTS],
        insert_endpoints: &mut [Option<InsertEndpointSlot>; TRACK_COUNT],
    ) {
        for index in 0..batch.endpoint_count() {
            let prepared = batch
                .prepared_endpoint_at(index)
                .expect("prevalidated batch retains every endpoint");
            let key = prepared.identity().key;
            let endpoint = match key.address() {
                TimelineEndpointAddress::Generator {
                    channel_id,
                    plugin_instance_id,
                } => generator_endpoints
                    .iter_mut()
                    .flatten()
                    .find(|endpoint| {
                        endpoint.channel_id == channel_id
                            && endpoint.endpoint_id == key.endpoint_id()
                            && endpoint.plugin_instance_id == plugin_instance_id
                    })
                    .map(|endpoint| &mut endpoint.endpoint),
                TimelineEndpointAddress::MixerInsert { track } => insert_endpoints
                    .get_mut(usize::from(track))
                    .and_then(Option::as_mut)
                    .filter(|endpoint| endpoint.endpoint_id == key.endpoint_id())
                    .map(|endpoint| &mut endpoint.endpoint),
            };
            let endpoint = endpoint.expect("prevalidated endpoint remains installed");
            debug_assert_eq!(
                endpoint.input_phase_frames(),
                usize::from(prepared.identity().phase)
            );
            endpoint.stage_preflighted_batch(prepared.events(), prepared.classes());
        }
    }

    #[cfg(test)]
    fn stage_timeline_generator_plan(
        plan: &mut TimelineRenderPlan,
        generator_endpoints: &mut [Option<GeneratorEndpointSlot>; MAX_GENERATOR_ENDPOINTS],
        generator_notes: &mut [Option<TimelineGeneratorNote>; MAX_ACTIVE_NOTES],
        routes: &TimelineGeneratorRouteTable,
        revision: u64,
        epoch: u64,
        _event_overflows: &mut u64,
    ) -> bool {
        let mut bindings = TimelinePluginAutomationBindings::new();
        bindings.revision = Some(revision);
        bindings.epoch = Some(epoch);
        let mut insert_endpoints: [Option<InsertEndpointSlot>; TRACK_COUNT] =
            std::array::from_fn(|_| None);
        let mut staged_notes = [None; MAX_ACTIVE_NOTES];
        let mut control_histories = Vec::with_capacity(TIMELINE_MAX_DRIVEN_PLUGIN_PARAMETERS);
        for _ in 0..TIMELINE_MAX_DRIVEN_PLUGIN_PARAMETERS {
            control_histories.push(
                Q128ControlHistory::new(0)
                    .expect("zero-delay test control history always allocates"),
            );
        }
        let mut batch = TimelineEndpointBatchPlan::new_boxed();
        if !Self::prepare_timeline_endpoint_batch(
            plan,
            None,
            generator_endpoints,
            &mut insert_endpoints,
            generator_notes,
            &mut staged_notes,
            routes,
            &bindings,
            &mut control_histories,
            revision,
            epoch,
            MAX_MIXER_BLOCK_FRAMES,
            false,
            None,
            0,
            batch.as_mut(),
        ) || !Self::preflight_timeline_endpoint_batch_commit(
            batch.as_ref(),
            generator_endpoints,
            &mut insert_endpoints,
            &bindings,
            0,
        ) {
            batch.abort();
            return false;
        }
        Self::commit_timeline_endpoint_batch(
            batch.as_ref(),
            generator_endpoints,
            &mut insert_endpoints,
        );
        *generator_notes = staged_notes;
        true
    }

    fn handle(&mut self, command: AudioCommand, retired_assets: &mut Producer<Arc<[f32]>>) {
        match command {
            AudioCommand::RegisterAsset {
                operation,
                id,
                samples,
                sample_rate,
                channels,
            } => {
                let success =
                    self.register_asset(id, samples, sample_rate, channels, retired_assets);
                self.pending_asset_event = Some(AudioAssetEvent::Registered {
                    operation,
                    id,
                    success,
                });
            }
            AudioCommand::UnregisterAsset { operation, id } => {
                let removed = self.unregister_asset(id, retired_assets);
                self.pending_asset_event = Some(AudioAssetEvent::Unregistered {
                    operation,
                    id,
                    removed,
                });
            }
            AudioCommand::ClearAssets { operation } => {
                let removed = self.clear_assets(retired_assets);
                self.pending_asset_event = Some(AudioAssetEvent::Cleared { operation, removed });
            }
            AudioCommand::PlayClip {
                clip_id,
                asset_id,
                source_frame,
                gain,
                mixer_track,
            } => self.play_audio_clip(clip_id, asset_id, source_frame, gain, mixer_track, false),
            AudioCommand::SyncClip {
                clip_id,
                asset_id,
                source_frame,
                gain,
                mixer_track,
            } => self.play_audio_clip(clip_id, asset_id, source_frame, gain, mixer_track, true),
            AudioCommand::StopClip { clip_id } => {
                for voice in &mut self.audio_voices {
                    if voice.active && voice.clip_id == clip_id {
                        voice.active = false;
                    }
                }
            }
            AudioCommand::NoteOn {
                note,
                velocity,
                mixer_track,
            } => {
                let frequency = 440.0 * 2.0_f32.powf((note as f32 - 69.0) / 12.0);
                self.voices[self.next_voice] = Voice {
                    phase: 0.0,
                    phase_step: frequency / self.sample_rate,
                    envelope: finite_clamp(velocity, 0.0, 1.0, 0.0) * 0.28,
                    decay: 0.99984,
                    active: true,
                    mixer_track: mixer_track.min(TRACK_COUNT - 1),
                    timeline_note_id: None,
                    timeline_channel_id: None,
                };
                self.next_voice = (self.next_voice + 1) % MAX_VOICES;
            }
            AudioCommand::StopAll => {
                self.stage_all_notes_off();
                self.reset_voice_and_pdc_state(0);
            }
            AudioCommand::SetMaster(value) => {
                self.master = finite_clamp(value, 0.0, 1.2, 0.0);
            }
            AudioCommand::SetMasterPan(value) => {
                self.master_pan = finite_clamp(value, -1.0, 1.0, 0.0);
            }
            AudioCommand::SetTrackGain { track, gain } => {
                if let Some(value) = self.track_gains.get_mut(track) {
                    *value = finite_clamp(gain, 0.0, 1.5, 0.0);
                }
            }
            AudioCommand::SetTrackPan { track, pan } => {
                if let Some(value) = self.track_pans.get_mut(track) {
                    *value = finite_clamp(pan, -1.0, 1.0, 0.0);
                }
            }
            AudioCommand::SetTrackMuted { track, muted } => {
                if let Some(value) = self.track_muted.get_mut(track) {
                    *value = muted;
                }
            }
            AudioCommand::SetTrackSolo { track, solo } => {
                if let Some(value) = self.track_solo.get_mut(track) {
                    *value = solo;
                }
            }
            AudioCommand::InstallInsertEndpoint {
                insert,
                endpoint_id,
                endpoint,
            } => self.install_insert_endpoint(insert, endpoint_id, endpoint),
            AudioCommand::RemoveInsertEndpoint { insert } => {
                self.remove_insert_endpoint(insert);
            }
            AudioCommand::ClearInsertEndpoints { request_id } => {
                self.clear_insert_endpoints(request_id);
            }
            AudioCommand::SendInsertMidi {
                insert,
                slot,
                data,
                sample_offset,
            } => {
                let event = u16::try_from(sample_offset).ok().and_then(|offset| {
                    slot.map(u8::try_from)
                        .transpose()
                        .ok()
                        .map(|slot| FrameEvent::midi(offset, slot, data))
                });
                let mut overflowed = false;
                if let Some(endpoint) = self
                    .insert_endpoints
                    .get_mut(insert)
                    .and_then(Option::as_mut)
                {
                    if let Some(event) = event {
                        if !endpoint.endpoint.stage(event) {
                            let deferred = endpoint.endpoint.event_rejection_defer_offset();
                            endpoint.suppress_output_frames = endpoint.suppress_output_frames.max(
                                fixed_quantum_fail_closed_frames(&endpoint.endpoint)
                                    .saturating_add(deferred),
                            );
                            overflowed = true;
                        }
                    } else {
                        self.fixed_quantum_invalid_events =
                            self.fixed_quantum_invalid_events.saturating_add(1);
                    }
                } else {
                    self.fixed_quantum_endpoint_event_rejections = self
                        .fixed_quantum_endpoint_event_rejections
                        .saturating_add(1);
                }
                if overflowed {
                    self.fixed_quantum_event_overflows =
                        self.fixed_quantum_event_overflows.saturating_add(1);
                }
            }
            AudioCommand::SetInsertParameter {
                insert,
                slot,
                id,
                normalized,
            } => {
                if self
                    .timeline_plugin_automation_bindings
                    .owns_insert_parameter(insert, slot, id)
                {
                    self.fixed_quantum_endpoint_event_rejections = self
                        .fixed_quantum_endpoint_event_rejections
                        .saturating_add(1);
                    return;
                }
                if let Some(endpoint) = self
                    .insert_endpoints
                    .get_mut(insert)
                    .and_then(Option::as_mut)
                {
                    let event = u8::try_from(slot)
                        .ok()
                        .map(|slot| FrameEvent::parameter(0, slot, id, normalized.clamp(0.0, 1.0)));
                    if let Some(event) = event {
                        if !endpoint.endpoint.stage(event) {
                            let deferred = endpoint.endpoint.event_rejection_defer_offset();
                            endpoint.suppress_output_frames = endpoint.suppress_output_frames.max(
                                fixed_quantum_fail_closed_frames(&endpoint.endpoint)
                                    .saturating_add(deferred),
                            );
                            self.fixed_quantum_event_overflows =
                                self.fixed_quantum_event_overflows.saturating_add(1);
                        }
                    } else {
                        self.fixed_quantum_invalid_events =
                            self.fixed_quantum_invalid_events.saturating_add(1);
                    }
                } else {
                    self.fixed_quantum_endpoint_event_rejections = self
                        .fixed_quantum_endpoint_event_rejections
                        .saturating_add(1);
                }
            }
            AudioCommand::InstallGeneratorEndpoint {
                channel_id,
                endpoint_id,
                plugin_instance_id,
                mixer_track,
                endpoint,
                pdc_delay,
            } => self.install_generator_endpoint(
                channel_id,
                endpoint_id,
                plugin_instance_id,
                mixer_track,
                endpoint,
                pdc_delay,
            ),
            AudioCommand::RemoveGeneratorEndpoint { channel_id } => {
                self.remove_generator_endpoint(channel_id);
            }
            AudioCommand::ClearGeneratorEndpoints { request_id } => {
                self.clear_generator_endpoints(request_id);
            }
            AudioCommand::SetGeneratorRoute {
                channel_id,
                mixer_track,
            } => self.set_generator_route(channel_id, mixer_track),
            AudioCommand::SendGeneratorMidi {
                channel_id,
                slot,
                data,
                sample_offset,
            } => {
                let event = u16::try_from(sample_offset).ok().and_then(|offset| {
                    slot.map(u8::try_from)
                        .transpose()
                        .ok()
                        .map(|slot| FrameEvent::midi(offset, slot, data))
                });
                if let Some(index) = self.find_generator_slot(channel_id) {
                    let endpoint = self.generator_endpoints[index]
                        .as_mut()
                        .expect("located generator endpoint must remain installed");
                    if let Some(event) = event {
                        if !endpoint.endpoint.stage(event) {
                            let deferred = endpoint.endpoint.event_rejection_defer_offset();
                            endpoint.suppress_output_frames = endpoint.suppress_output_frames.max(
                                fixed_quantum_fail_closed_frames(&endpoint.endpoint)
                                    .saturating_add(deferred),
                            );
                            self.fixed_quantum_event_overflows =
                                self.fixed_quantum_event_overflows.saturating_add(1);
                        }
                    } else {
                        self.fixed_quantum_invalid_events =
                            self.fixed_quantum_invalid_events.saturating_add(1);
                    }
                } else {
                    self.fixed_quantum_endpoint_event_rejections = self
                        .fixed_quantum_endpoint_event_rejections
                        .saturating_add(1);
                }
            }
            AudioCommand::SetGeneratorParameter {
                channel_id,
                slot,
                id,
                normalized,
            } => {
                if self
                    .timeline_plugin_automation_bindings
                    .owns_generator_parameter(channel_id, slot, id)
                {
                    self.fixed_quantum_endpoint_event_rejections = self
                        .fixed_quantum_endpoint_event_rejections
                        .saturating_add(1);
                    return;
                }
                if let Some(index) = self.find_generator_slot(channel_id) {
                    let endpoint = self.generator_endpoints[index]
                        .as_mut()
                        .expect("located generator endpoint must remain installed");
                    let event = u8::try_from(slot)
                        .ok()
                        .map(|slot| FrameEvent::parameter(0, slot, id, normalized.clamp(0.0, 1.0)));
                    if let Some(event) = event {
                        if !endpoint.endpoint.stage(event) {
                            let deferred = endpoint.endpoint.event_rejection_defer_offset();
                            endpoint.suppress_output_frames = endpoint.suppress_output_frames.max(
                                fixed_quantum_fail_closed_frames(&endpoint.endpoint)
                                    .saturating_add(deferred),
                            );
                            self.fixed_quantum_event_overflows =
                                self.fixed_quantum_event_overflows.saturating_add(1);
                        }
                    } else {
                        self.fixed_quantum_invalid_events =
                            self.fixed_quantum_invalid_events.saturating_add(1);
                    }
                } else {
                    self.fixed_quantum_endpoint_event_rejections = self
                        .fixed_quantum_endpoint_event_rejections
                        .saturating_add(1);
                }
            }
            AudioCommand::EditPluginParameter(submission) => {
                self.handle_plugin_parameter_edit(submission);
            }
            AudioCommand::InstallMidiInput { route_id, prepared } => {
                self.install_midi_input(route_id, prepared);
            }
            AudioCommand::RemoveMidiInput { route_id } => {
                self.remove_midi_input(route_id);
            }
            AudioCommand::ClearMidiInput { request_id } => {
                self.clear_midi_input(request_id);
            }
            AudioCommand::StartMidiRecording { endpoint } => {
                self.start_midi_recording(endpoint);
            }
            AudioCommand::StopMidiRecording { session_id } => {
                self.stop_midi_recording(session_id);
            }
            AudioCommand::ClearMidiRecording { request_id } => {
                self.clear_midi_recording(request_id);
            }
            AudioCommand::InstallMasterCapture {
                capture_id,
                endpoint,
            } => self.install_master_capture(capture_id, endpoint),
            AudioCommand::StopMasterCapture { capture_id } => {
                self.stop_master_capture(capture_id);
            }
            AudioCommand::ClearMasterCapture { request_id } => {
                self.clear_master_capture(request_id);
            }
        }
    }

    fn timeline_transport_owned(&self) -> bool {
        self.timeline_runtime
            .as_ref()
            .and_then(RealtimeTimelineRuntime::active_epoch)
            .is_some()
    }

    fn handle_plugin_parameter_edit(&mut self, submission: ParameterEditSubmission) {
        let route = submission.route;
        let invalid = route.project_session == 0
            || route.endpoint.id == 0
            || route.instance_id == 0
            || submission.edit_id.0 == 0
            || !submission.normalized.is_finite()
            || !(0.0..=1.0).contains(&submission.normalized);
        if invalid {
            self.reject_plugin_parameter_edit(submission, CallbackRejectReason::InvalidParameter);
            return;
        }

        let result = match route.endpoint.kind {
            ParameterEndpointKind::Insert => {
                let mut matches =
                    self.insert_endpoints
                        .iter()
                        .enumerate()
                        .filter(|(_, endpoint)| {
                            endpoint
                                .as_ref()
                                .is_some_and(|endpoint| endpoint.endpoint_id == route.endpoint.id)
                        });
                let Some((insert, _)) = matches.next() else {
                    self.reject_plugin_parameter_edit(
                        submission,
                        CallbackRejectReason::StaleEndpoint,
                    );
                    return;
                };
                if matches.next().is_some() {
                    self.reject_plugin_parameter_edit(
                        submission,
                        CallbackRejectReason::StaleEndpoint,
                    );
                    return;
                }
                drop(matches);
                let timeline_owned = self
                    .timeline_plugin_automation_bindings
                    .owns_insert_parameter(insert, route.slot, route.parameter_id)
                    || self
                        .timeline_activation_plugin_automation_bindings
                        .owns_insert_parameter(insert, route.slot, route.parameter_id);
                let endpoint = self.insert_endpoints[insert]
                    .as_mut()
                    .expect("located insert endpoint remains installed");
                let Ok(track) = u8::try_from(insert) else {
                    self.reject_plugin_parameter_edit(
                        submission,
                        CallbackRejectReason::StaleEndpoint,
                    );
                    return;
                };
                Self::try_admit_plugin_parameter_edit(
                    &mut endpoint.endpoint,
                    TimelineEndpointKey::mixer_insert(track, route.endpoint.id),
                    timeline_owned,
                    submission,
                    self.timeline_endpoint_batch.as_mut(),
                )
            }
            ParameterEndpointKind::Generator => {
                let mut matches =
                    self.generator_endpoints
                        .iter()
                        .enumerate()
                        .filter(|(_, endpoint)| {
                            endpoint
                                .as_ref()
                                .is_some_and(|endpoint| endpoint.endpoint_id == route.endpoint.id)
                        });
                let Some((index, _)) = matches.next() else {
                    self.reject_plugin_parameter_edit(
                        submission,
                        CallbackRejectReason::StaleEndpoint,
                    );
                    return;
                };
                if matches.next().is_some() {
                    self.reject_plugin_parameter_edit(
                        submission,
                        CallbackRejectReason::StaleEndpoint,
                    );
                    return;
                }
                drop(matches);
                let slot = self.generator_endpoints[index]
                    .as_ref()
                    .expect("located generator endpoint remains installed");
                let timeline_owned = self
                    .timeline_plugin_automation_bindings
                    .owns_generator_parameter(slot.channel_id, route.slot, route.parameter_id)
                    || self
                        .timeline_activation_plugin_automation_bindings
                        .owns_generator_parameter(slot.channel_id, route.slot, route.parameter_id);
                let key = TimelineEndpointKey::new(
                    slot.channel_id,
                    route.endpoint.id,
                    slot.plugin_instance_id,
                );
                let endpoint = self.generator_endpoints[index]
                    .as_mut()
                    .expect("located generator endpoint remains installed");
                Self::try_admit_plugin_parameter_edit(
                    &mut endpoint.endpoint,
                    key,
                    timeline_owned,
                    submission,
                    self.timeline_endpoint_batch.as_mut(),
                )
            }
        };

        match result {
            Ok(first_marker_for_endpoint) => {
                if first_marker_for_endpoint {
                    self.admitted_live_edit_endpoint_count =
                        self.admitted_live_edit_endpoint_count.saturating_add(1);
                }
                release_callback_parameter_edit(&self.parameter_edit_callback_admission);
            }
            Err(reason) => self.reject_plugin_parameter_edit(submission, reason),
        }
    }

    fn try_admit_plugin_parameter_edit(
        endpoint: &mut PreparedFixedEndpoint,
        key: TimelineEndpointKey,
        timeline_owned: bool,
        submission: ParameterEditSubmission,
        batch: &mut TimelineEndpointBatchPlan,
    ) -> Result<bool, CallbackRejectReason> {
        let route = submission.route;
        if endpoint.project_session != route.project_session {
            return Err(CallbackRejectReason::StaleProjectSession);
        }
        let Some(instance_id) = endpoint.manifest.instance_id(route.slot) else {
            return Err(CallbackRejectReason::MissingSlot);
        };
        if instance_id != route.instance_id {
            return Err(CallbackRejectReason::MissingInstance);
        }
        if timeline_owned {
            return Err(CallbackRejectReason::TimelineOwned);
        }
        let Some(edit_id) = RuntimeParameterEditId::new(submission.edit_id.0) else {
            return Err(CallbackRejectReason::InvalidParameter);
        };
        let Ok(slot) = u8::try_from(route.slot) else {
            return Err(CallbackRejectReason::MissingSlot);
        };

        let endpoint_already_marked = endpoint.has_admitted_live_edit_marker();
        batch.reset();
        let phase = endpoint.input_phase_frames();
        let seed = endpoint.timeline_batch_seed();
        let handle = match batch.register_endpoint_with_seed(key, phase, 1, seed) {
            Ok(handle) => handle,
            Err(_) => {
                batch.abort();
                return Err(CallbackRejectReason::QueueFault);
            }
        };
        if batch
            .push_admitted_live_parameter_reservation(
                handle,
                0,
                slot,
                route.parameter_id,
                submission.normalized,
                edit_id,
            )
            .is_err()
            || batch.preflight().is_err()
        {
            batch.abort();
            return Err(CallbackRejectReason::QueueFault);
        }
        let Some(prepared) = batch.prepared_endpoint(handle) else {
            batch.abort();
            return Err(CallbackRejectReason::QueueFault);
        };
        if !endpoint.can_stage_batch(prepared.events(), prepared.classes()) {
            batch.abort();
            return Err(CallbackRejectReason::QueueFault);
        }

        // Admission happens only after every callback-side capacity proof. Once true, endpoint
        // epoch/drop/gap paths own the exactly-once terminal worker receipt.
        if !endpoint.try_admit_parameter_edit(
            route.slot,
            route.parameter_id,
            submission.normalized,
            edit_id,
        ) {
            batch.abort();
            return Err(CallbackRejectReason::QueueFault);
        }
        endpoint.stage_preflighted_batch(prepared.events(), prepared.classes());
        batch.reset();
        Ok(!endpoint_already_marked)
    }

    fn reject_plugin_parameter_edit(
        &mut self,
        submission: ParameterEditSubmission,
        reason: CallbackRejectReason,
    ) {
        let receipt = CallbackEditReceipt::Rejected {
            edit_id: submission.edit_id,
            route: submission.route,
            reason,
        };
        let pushed = self
            .parameter_edit_callback_events
            .as_mut()
            .is_some_and(|events| events.push(receipt).is_ok());
        debug_assert!(
            pushed,
            "edit commands reserve callback receipt capacity before pop"
        );
    }

    fn pending_timeline_transport_activation(&self) -> Option<TimelineTransportActivationTicket> {
        self.timeline_runtime
            .as_ref()
            .and_then(RealtimeTimelineRuntime::pending_transport_activation)
    }

    fn build_graph_pdc_plan(
        graph: &CompiledMixerGraph,
        endpoint_identities: &mut MixerGraphEndpointIdentityTable,
        insert_endpoints: &mut [Option<InsertEndpointSlot>; TRACK_COUNT],
        generator_endpoints: &mut [Option<GeneratorEndpointSlot>; MAX_GENERATOR_ENDPOINTS],
        maximum_delay_samples: u32,
    ) -> Option<GraphPdcPlan> {
        endpoint_identities
            .capture(graph, insert_endpoints, generator_endpoints)
            .then(|| endpoint_identities.build_pdc_plan(graph, maximum_delay_samples))?
    }

    /// Writes only preallocated scratch members. Every active runtime, voice,
    /// endpoint, PDC tap and transport field remains untouched on rejection.
    fn preflight_timeline_transport_activation(
        &mut self,
        ticket: TimelineTransportActivationTicket,
        actual_epoch: u64,
    ) -> Result<(), TimelineTransportActivationRejectReason> {
        let DspState {
            timeline_runtime,
            timeline_executor,
            timeline_automation,
            timeline_activation_plan,
            timeline_activation_channel_bases,
            timeline_activation_generator_routes,
            timeline_activation_plugin_automation_bindings,
            timeline_plugin_control_histories,
            mixer_graph_activation_plan,
            mixer_graph_activation_identity,
            graph_pdc_activation_plan,
            graph_pdc_activation_revision,
            mixer_graph_activation_endpoint_identities,
            insert_endpoints,
            generator_endpoints,
            pdc_plan_revision,
            pdc_maximum_delay_samples,
            audio_assets,
            ..
        } = self;
        let runtime =
            timeline_runtime
                .as_ref()
                .ok_or(TimelineTransportActivationRejectReason::Runtime(
                    crate::timeline_runtime::TimelineDiscontinuityActivationError::MissingTimeline,
                ))?;
        runtime
            .preflight_transport_activation(ticket, actual_epoch)
            .map_err(TimelineTransportActivationRejectReason::Runtime)?;
        let timeline = runtime.transport_activation_timeline(ticket).ok_or(
            TimelineTransportActivationRejectReason::Runtime(
                crate::timeline_runtime::TimelineDiscontinuityActivationError::MissingTimeline,
            ),
        )?;
        let chase =
            runtime
                .transport_activation_chase(ticket)
                .ok_or(TimelineTransportActivationRejectReason::Runtime(
                crate::timeline_runtime::TimelineDiscontinuityActivationError::MissingPrepared {
                    kind: TimelineDiscontinuityKind::OneShot,
                },
            ))?;

        if !mixer_graph_activation_plan.reset_from(timeline.mixer_graph()) {
            return Err(TimelineTransportActivationRejectReason::MixerGraphPlan);
        }
        *mixer_graph_activation_identity = Some(MixerGraphBindingIdentity {
            revision: ticket.spec().revision,
            epoch: actual_epoch,
            fingerprint: timeline.mixer_graph().fingerprint(),
        });
        let next_graph_pdc_plan = Self::build_graph_pdc_plan(
            timeline.mixer_graph(),
            mixer_graph_activation_endpoint_identities,
            insert_endpoints,
            generator_endpoints,
            *pdc_maximum_delay_samples,
        )
        .ok_or(TimelineTransportActivationRejectReason::GraphPdcPlan)?;
        runtime
            .transport_activation_mixer_delay_bank(ticket)
            .ok_or(TimelineTransportActivationRejectReason::MixerDelayBank)?
            .preflight_plan(&next_graph_pdc_plan)
            .map_err(|_| TimelineTransportActivationRejectReason::MixerDelayBank)?;
        let next_graph_pdc_revision = next_nonzero_id(*pdc_plan_revision);
        **graph_pdc_activation_plan = Some(next_graph_pdc_plan);
        *graph_pdc_activation_revision = Some(next_graph_pdc_revision);

        for clip in timeline.audio_clips() {
            if !audio_assets
                .iter()
                .any(|slot| slot.samples.is_some() && slot.id == clip.asset_id)
            {
                return Err(TimelineTransportActivationRejectReason::MissingAudioAsset {
                    asset_id: clip.asset_id,
                });
            }
        }
        if !timeline_activation_channel_bases.reset_from(timeline.channel_bases()) {
            return Err(TimelineTransportActivationRejectReason::ChannelBaseTable);
        }
        let spec = ticket.spec();
        if !timeline_activation_generator_routes.reset_from(
            spec.revision,
            actual_epoch,
            timeline.plugin_routes(),
            timeline.channel_bases(),
        ) {
            return Err(TimelineTransportActivationRejectReason::GeneratorRouteBinding);
        }
        if !timeline_activation_channel_bases.bind_driven_automation(
            timeline.automation_bases(),
            timeline.driven_automation_targets(),
            timeline.plugin_routes(),
        ) {
            return Err(TimelineTransportActivationRejectReason::ChannelBaseTable);
        }
        if !timeline_activation_generator_routes.bind_installed_endpoints(generator_endpoints) {
            return Err(TimelineTransportActivationRejectReason::GeneratorRouteBinding);
        }
        if !timeline_activation_plugin_automation_bindings.reset_from(
            spec.revision,
            actual_epoch,
            timeline.automation_bases(),
            timeline.driven_automation_targets(),
            timeline.plugin_routes(),
            timeline_activation_generator_routes,
            generator_endpoints,
            insert_endpoints,
            graph_pdc_activation_plan
                .as_ref()
                .as_ref()
                .expect("candidate graph PDC plan was preflighted"),
            next_graph_pdc_revision,
            *pdc_maximum_delay_samples,
        ) {
            return Err(TimelineTransportActivationRejectReason::PluginAutomationBinding);
        }

        timeline_activation_plan.clear();
        if timeline_executor
            .stage_reset_from_chase(actual_epoch, chase, timeline_activation_plan.as_mut())
            .is_err()
        {
            timeline_executor.abort_staged_reset();
            timeline_automation.abort_staged_reset();
            return Err(TimelineTransportActivationRejectReason::ExecutorState);
        }
        if timeline_activation_plan.overflowed {
            timeline_executor.abort_staged_reset();
            timeline_automation.abort_staged_reset();
            return Err(TimelineTransportActivationRejectReason::RenderPlanCapacity);
        }
        if timeline_automation
            .stage_reset_from_chase(
                actual_epoch,
                chase.frame,
                timeline.automation_bases(),
                timeline_activation_plan.automation_chase_values(),
            )
            .is_err()
        {
            timeline_executor.abort_staged_reset();
            timeline_automation.abort_staged_reset();
            return Err(TimelineTransportActivationRejectReason::ExecutorState);
        }
        if !Self::timeline_plugin_control_histories_can_reset(
            timeline_plugin_control_histories,
            timeline_activation_plugin_automation_bindings,
            timeline_activation_plan.as_ref(),
        ) {
            timeline_executor.abort_staged_reset();
            timeline_automation.abort_staged_reset();
            return Err(TimelineTransportActivationRejectReason::ExecutorState);
        }

        let mut endpoint_note_events = [0_usize; MAX_GENERATOR_ENDPOINTS];
        for note in &chase.notes {
            let Some(route) =
                timeline_activation_generator_routes.route_for_channel(note.channel_id)
            else {
                continue;
            };
            let Some(endpoint_index) = generator_endpoints.iter().position(|slot| {
                slot.as_ref().is_some_and(|endpoint| {
                    endpoint.channel_id == route.channel_id
                        && endpoint.plugin_instance_id == route.plugin_instance_id
                        && endpoint.mixer_track == route.mixer_track
                        && endpoint.endpoint_id == route.endpoint_id
                })
            }) else {
                timeline_executor.abort_staged_reset();
                timeline_automation.abort_staged_reset();
                return Err(TimelineTransportActivationRejectReason::GeneratorRouteBinding);
            };
            endpoint_note_events[endpoint_index] =
                endpoint_note_events[endpoint_index].saturating_add(1);
        }
        for binding in timeline_activation_plugin_automation_bindings.iter() {
            let TimelinePluginEndpointIdentity::Generator {
                channel_id,
                endpoint_id,
                plugin_instance_id,
            } = binding.endpoint
            else {
                continue;
            };
            let Some(endpoint_index) = generator_endpoints.iter().position(|slot| {
                slot.as_ref().is_some_and(|endpoint| {
                    endpoint.channel_id == channel_id
                        && endpoint.endpoint_id == endpoint_id
                        && endpoint.plugin_instance_id == plugin_instance_id
                })
            }) else {
                timeline_executor.abort_staged_reset();
                timeline_automation.abort_staged_reset();
                return Err(TimelineTransportActivationRejectReason::GeneratorRouteBinding);
            };
            endpoint_note_events[endpoint_index] =
                endpoint_note_events[endpoint_index].saturating_add(1);
        }
        let endpoint_batch_fits = endpoint_note_events
            .iter()
            .copied()
            .all(|count| count <= TIMELINE_ENDPOINT_TIMELINE_MAX_EVENTS_PER_QUANTUM);
        if !endpoint_batch_fits {
            timeline_executor.abort_staged_reset();
            timeline_automation.abort_staged_reset();
            return Err(TimelineTransportActivationRejectReason::EndpointEventCapacity);
        }
        if !mixer_graph_activation_endpoint_identities
            .matches_fresh(insert_endpoints, generator_endpoints)
        {
            timeline_executor.abort_staged_reset();
            timeline_automation.abort_staged_reset();
            return Err(TimelineTransportActivationRejectReason::GraphPdcPlan);
        }
        Ok(())
    }

    fn reject_timeline_transport_activation(
        &mut self,
        ticket: TimelineTransportActivationTicket,
        reason: TimelineTransportActivationRejectReason,
    ) {
        self.timeline_executor.abort_staged_reset();
        self.timeline_automation.abort_staged_reset();
        self.timeline_activation_plan.clear();
        self.timeline_activation_plugin_automation_bindings.clear();
        *self.mixer_graph_activation_plan = FixedMixerGraphLayout::default();
        self.mixer_graph_activation_identity = None;
        *self.graph_pdc_activation_plan = None;
        self.graph_pdc_activation_revision = None;
        self.mixer_graph_activation_endpoint_identities.clear();
        if let Some(runtime) = self.timeline_runtime.as_mut() {
            runtime.reject_pending_transport_activation(ticket, reason);
        }
    }

    /// All operations below are bounded and infallible after the matching
    /// preflight. Shared active identity is deliberately published separately.
    fn commit_timeline_transport_activation(
        &mut self,
        status: &AudioStatus,
        ticket: TimelineTransportActivationTicket,
        actual_epoch: u64,
    ) -> CommittedTimelineTransportActivation {
        let spec = ticket.spec();
        self.apply_transport_epoch(status, actual_epoch, spec.beat_q32);
        self.timeline_executor.commit_staged_reset();
        self.timeline_automation.commit_staged_reset();
        std::mem::swap(&mut self.timeline_plan, &mut self.timeline_activation_plan);
        std::mem::swap(
            &mut self.timeline_channel_bases,
            &mut self.timeline_activation_channel_bases,
        );
        std::mem::swap(
            &mut self.timeline_generator_routes,
            &mut self.timeline_activation_generator_routes,
        );
        std::mem::swap(
            &mut self.timeline_plugin_automation_bindings,
            &mut self.timeline_activation_plugin_automation_bindings,
        );
        self.timeline_activation_plugin_automation_bindings.clear();
        std::mem::swap(
            &mut self.mixer_graph_plan,
            &mut self.mixer_graph_activation_plan,
        );
        self.mixer_graph_identity = self.mixer_graph_activation_identity.take();
        *self.mixer_graph_activation_plan = FixedMixerGraphLayout::default();
        std::mem::swap(
            &mut self.graph_pdc_plan,
            &mut self.graph_pdc_activation_plan,
        );
        *self.graph_pdc_activation_plan = None;
        std::mem::swap(
            &mut self.mixer_graph_endpoint_identities,
            &mut self.mixer_graph_activation_endpoint_identities,
        );
        self.mixer_graph_activation_endpoint_identities.clear();
        self.pdc_plan_revision = self
            .graph_pdc_activation_revision
            .take()
            .expect("preflighted graph PDC revision remains staged");
        self.mixer_graph_was_activated = true;
        Self::reset_timeline_plugin_control_histories(
            &mut self.timeline_plugin_control_histories,
            &self.timeline_plugin_automation_bindings,
            actual_epoch,
            self.timeline_plan.as_ref(),
        );
        let mut pan_release_mask = spec.mixer_pan_release.track_mask;
        while pan_release_mask != 0 {
            let track = pan_release_mask.trailing_zeros() as usize;
            let pan = spec
                .mixer_pan_release
                .pan_for_track(track)
                .expect("control-validated pan release remains selected");
            debug_assert!(pan.is_finite() && (-1.0..=1.0).contains(&pan));
            self.track_pans[track] = pan;
            pan_release_mask &= pan_release_mask - 1;
        }
        let committed = self
            .timeline_runtime
            .as_mut()
            .expect("preflighted activation retains its runtime")
            .commit_preflighted_transport_activation(ticket, actual_epoch);
        let graph_pdc_plan = self
            .graph_pdc_plan
            .as_ref()
            .as_ref()
            .expect("preflighted graph PDC plan became active");
        let bank_request = self
            .timeline_runtime
            .as_mut()
            .and_then(RealtimeTimelineRuntime::active_mixer_delay_bank_mut)
            .expect("preflighted mixer delay bank promoted with its timeline")
            .request_plan(graph_pdc_plan, 0);
        debug_assert!(
            bank_request.is_ok(),
            "preflighted route delay targets remain valid"
        );
        self.timeline_runtime
            .as_mut()
            .and_then(RealtimeTimelineRuntime::active_mixer_delay_bank_mut)
            .expect("active mixer delay bank remains installed")
            .reset();
        for node in graph_pdc_plan.nodes() {
            let delay = &mut self.pdc_raw_track_delays[node.runtime_slot];
            let request = delay.request_delay(node.raw_source_delay.applied_samples(), 0);
            debug_assert!(request.is_ok(), "preflighted raw delay fits its fixed line");
        }
        for compensation in graph_pdc_plan.generators() {
            let endpoint = self
                .generator_endpoints
                .iter_mut()
                .flatten()
                .find(|endpoint| {
                    endpoint.endpoint_id == compensation.endpoint_id
                        && endpoint.channel_id == compensation.channel_id
                });
            if let Some(endpoint) = endpoint {
                let request = endpoint
                    .pdc_delay
                    .request_delay(compensation.delay.applied_samples(), 0);
                debug_assert!(request.is_ok(), "preflighted generator delay fits its line");
                endpoint.pdc_initialized = true;
            }
        }
        self.mixer_graph_endpoint_identities
            .apply_expected_revisions(&mut self.insert_endpoints, &mut self.generator_endpoints);
        // Publish only after the graph, delay bank, endpoint identities, and
        // worker attestations are one committed generation. The first render
        // sees an unchanged plan and therefore cannot be relied on to publish
        // the activation's graph-level PDC status.
        self.publish_graph_pdc_status(status);
        let driven_target_count = self
            .timeline_runtime
            .as_ref()
            .and_then(RealtimeTimelineRuntime::active_timeline)
            .map_or(0, |timeline| {
                timeline.driven_automation_targets().len() as u64
            });
        self.timeline_automation_pending = driven_target_count.saturating_sub(
            self.timeline_channel_bases
                .applied_automation_target_count()
                .saturating_add(
                    self.timeline_plugin_automation_bindings
                        .applied_automation_target_count(),
                ),
        );
        self.timeline_automation_unsupported = self
            .timeline_automation_unsupported
            .saturating_add(self.timeline_automation_pending);
        // Chased generator MIDI remains in the staged render plan. The first
        // rendered block batches it with Q128 parameter chase values and only
        // publishes both after the runtime/automation block commits.
        self.timeline_channel_revision = Some(spec.revision);
        self.timeline_channel_epoch = Some(actual_epoch);
        self.timeline_plan_has_chase = true;
        self.timeline_plan_render_active = false;
        self.timeline_transport_beat_q32 = spec.beat_q32;
        committed
    }

    fn publish_timeline_transport_activation(
        &mut self,
        committed: CommittedTimelineTransportActivation,
    ) {
        self.timeline_runtime
            .as_mut()
            .expect("committed activation retains its runtime")
            .publish_committed_transport_activation(committed);
    }

    fn apply_transport_discontinuity(
        &mut self,
        status: &AudioStatus,
        epoch: u64,
        beat_q32: u64,
        frame: u64,
        discontinuity: TransportDiscontinuity,
    ) {
        self.apply_transport_epoch(status, epoch, beat_q32);
        self.timeline_plan.clear();
        self.timeline_plan_has_chase = false;
        self.timeline_plan_render_active = false;

        let activation_succeeded = {
            let Some(runtime) = self.timeline_runtime.as_mut() else {
                return;
            };
            let kind = match discontinuity {
                TransportDiscontinuity::OneShot => TimelineDiscontinuityKind::OneShot,
                TransportDiscontinuity::Loop => {
                    let Some(token) = runtime.installed_loop_token() else {
                        runtime.require_resync();
                        self.timeline_execution_failures =
                            self.timeline_execution_failures.saturating_add(1);
                        self.stage_all_notes_off();
                        return;
                    };
                    TimelineDiscontinuityKind::Loop { token }
                }
            };
            let Some(revision) = runtime.discontinuity_revision(kind) else {
                // With no prepared timeline discontinuity, legacy AudioCommand
                // scheduling remains authoritative and transport is unchanged.
                return;
            };
            let activation = if runtime.stats().ownership_needs_resync {
                runtime.activate_discontinuity_after_resync(revision, epoch, frame, kind)
            } else {
                runtime.activate_discontinuity(revision, epoch, frame, kind)
            };
            activation.is_ok()
        };
        if !activation_succeeded {
            if let Some(runtime) = self.timeline_runtime.as_mut() {
                runtime.require_resync();
            }
            self.fail_timeline_block();
            return;
        }

        let render_binding_loaded = {
            let DspState {
                timeline_runtime,
                timeline_activation_channel_bases,
                timeline_activation_generator_routes,
                timeline_activation_plugin_automation_bindings,
                insert_endpoints,
                generator_endpoints,
                pdc_plan,
                graph_pdc_plan,
                pdc_plan_revision,
                pdc_maximum_delay_samples,
                ..
            } = self;
            timeline_runtime.as_ref().is_some_and(|runtime| {
                let Some(revision) = runtime.active_revision() else {
                    return false;
                };
                let Some(timeline) = runtime.active_timeline() else {
                    return false;
                };
                let common_loaded = timeline_activation_channel_bases
                    .reset_from(timeline.channel_bases())
                    && timeline_activation_channel_bases.bind_driven_automation(
                        timeline.automation_bases(),
                        timeline.driven_automation_targets(),
                        timeline.plugin_routes(),
                    )
                    && timeline_activation_generator_routes.reset_from(
                        revision,
                        epoch,
                        timeline.plugin_routes(),
                        timeline.channel_bases(),
                    )
                    && timeline_activation_generator_routes
                        .bind_installed_endpoints(generator_endpoints);
                if !common_loaded {
                    return false;
                }
                let graph_plan = graph_pdc_plan.as_ref().as_ref();
                if let Some(graph_plan) = graph_plan {
                    timeline_activation_plugin_automation_bindings.reset_from(
                        revision,
                        epoch,
                        timeline.automation_bases(),
                        timeline.driven_automation_targets(),
                        timeline.plugin_routes(),
                        timeline_activation_generator_routes,
                        generator_endpoints,
                        insert_endpoints,
                        graph_plan,
                        *pdc_plan_revision,
                        *pdc_maximum_delay_samples,
                    )
                } else {
                    timeline_activation_plugin_automation_bindings.reset_from(
                        revision,
                        epoch,
                        timeline.automation_bases(),
                        timeline.driven_automation_targets(),
                        timeline.plugin_routes(),
                        timeline_activation_generator_routes,
                        generator_endpoints,
                        insert_endpoints,
                        pdc_plan,
                        *pdc_plan_revision,
                        *pdc_maximum_delay_samples,
                    )
                }
            })
        };
        if !render_binding_loaded {
            self.poison_active_timeline_cursor(epoch, frame);
            self.fail_timeline_block();
            return;
        }

        let reset_succeeded = {
            let DspState {
                timeline_runtime,
                timeline_executor,
                timeline_automation,
                timeline_plan,
                timeline_activation_plugin_automation_bindings,
                timeline_plugin_control_histories,
                ..
            } = self;
            let runtime = timeline_runtime
                .as_mut()
                .expect("timeline runtime was present during activation");
            let revision = runtime
                .active_revision()
                .expect("activated timeline retains its revision");
            match runtime.chase_for_block(revision, epoch, frame) {
                Ok(Some(state)) => {
                    let executor_staged = timeline_executor
                        .stage_reset_from_chase(epoch, state, timeline_plan.as_mut())
                        .is_ok();
                    let reset_staged = executor_staged
                        && !timeline_plan.overflowed
                        && timeline_automation
                            .stage_reset_from_chase(
                                epoch,
                                state.frame,
                                &state.automation_bases,
                                timeline_plan.automation_chase_values(),
                            )
                            .is_ok()
                        && Self::timeline_plugin_control_histories_can_reset(
                            timeline_plugin_control_histories,
                            timeline_activation_plugin_automation_bindings,
                            timeline_plan.as_ref(),
                        );
                    if !reset_staged {
                        timeline_executor.abort_staged_reset();
                        timeline_automation.abort_staged_reset();
                        false
                    } else {
                        timeline_executor.commit_staged_reset();
                        timeline_automation.commit_staged_reset();
                        true
                    }
                }
                Ok(None) | Err(_) => {
                    timeline_executor.abort_staged_reset();
                    timeline_automation.abort_staged_reset();
                    false
                }
            }
        };
        if !reset_succeeded {
            self.poison_active_timeline_cursor(epoch, frame);
            self.fail_timeline_block();
            return;
        }
        std::mem::swap(
            &mut self.timeline_channel_bases,
            &mut self.timeline_activation_channel_bases,
        );
        std::mem::swap(
            &mut self.timeline_generator_routes,
            &mut self.timeline_activation_generator_routes,
        );
        std::mem::swap(
            &mut self.timeline_plugin_automation_bindings,
            &mut self.timeline_activation_plugin_automation_bindings,
        );
        self.timeline_activation_plugin_automation_bindings.clear();
        Self::reset_timeline_plugin_control_histories(
            &mut self.timeline_plugin_control_histories,
            &self.timeline_plugin_automation_bindings,
            epoch,
            self.timeline_plan.as_ref(),
        );
        self.mixer_graph_endpoint_identities
            .apply_expected_revisions(&mut self.insert_endpoints, &mut self.generator_endpoints);
        let driven_target_count = self
            .timeline_runtime
            .as_ref()
            .and_then(RealtimeTimelineRuntime::active_timeline)
            .map_or(0, |timeline| {
                timeline.driven_automation_targets().len() as u64
            });
        self.timeline_automation_pending = driven_target_count.saturating_sub(
            self.timeline_channel_bases
                .applied_automation_target_count()
                .saturating_add(
                    self.timeline_plugin_automation_bindings
                        .applied_automation_target_count(),
                ),
        );
        self.timeline_automation_unsupported = self
            .timeline_automation_unsupported
            .saturating_add(self.timeline_automation_pending);
        self.timeline_channel_revision = self
            .timeline_runtime
            .as_ref()
            .and_then(RealtimeTimelineRuntime::active_revision);
        self.timeline_channel_epoch = Some(epoch);
        if self.mixer_graph_was_activated {
            let Some(identity) = self.mixer_graph_identity.as_mut() else {
                self.poison_active_timeline_cursor(epoch, frame);
                self.fail_timeline_block();
                return;
            };
            if Some(identity.revision) != self.timeline_channel_revision
                || identity.fingerprint != self.mixer_graph_plan.fingerprint()
            {
                self.poison_active_timeline_cursor(epoch, frame);
                self.fail_timeline_block();
                return;
            }
            identity.epoch = epoch;
        }
        self.timeline_plan_has_chase = true;
    }

    fn poison_active_timeline_cursor(&mut self, epoch: u64, frame: u64) {
        let Some(runtime) = self.timeline_runtime.as_mut() else {
            return;
        };
        let Some(revision) = runtime.active_revision() else {
            return;
        };
        let mut empty_packet = TimelinePacket::<0>::new();
        if let Ok(block) = runtime.packetize_block(revision, epoch, frame, 0, &mut empty_packet) {
            block.abort();
        }
    }

    fn fail_timeline_block(&mut self) {
        self.timeline_execution_failures = self.timeline_execution_failures.saturating_add(1);
        self.clear_timeline_expected_latency_revisions();
        self.timeline_automation.abort_block();
        self.timeline_plan.clear();
        self.timeline_plan_has_chase = false;
        self.timeline_plan_render_active = false;
        self.timeline_automation_pending = 0;
        self.timeline_channel_bases.clear();
        self.timeline_generator_routes.clear();
        self.timeline_plugin_automation_bindings.clear();
        self.timeline_activation_plugin_automation_bindings.clear();
        self.timeline_channel_revision = None;
        self.timeline_channel_epoch = None;
        self.stage_all_notes_off();
        self.reset_voice_and_pdc_state(self.timeline_transport_beat_q32);
    }

    fn apply_transport_epoch(&mut self, status: &AudioStatus, epoch: u64, beat_q32: u64) {
        debug_assert_ne!(epoch, 0);
        self.transport_epoch = epoch;
        self.timeline_transport_beat_q32 = beat_q32;

        let mut resets = 0_u64;
        let mut failures = 0_u64;
        let mut synchronized = false;
        for slot in self.insert_endpoints.iter_mut().flatten() {
            match synchronize_endpoint_epoch(&mut slot.endpoint, epoch) {
                EndpointEpochSync::AlreadyCurrent => synchronized = true,
                EndpointEpochSync::Reset => {
                    resets += 1;
                    synchronized = true;
                }
                EndpointEpochSync::Failed => failures += 1,
            }
            let _ = slot.endpoint.clear_and_stage_all_notes_off();
        }
        for slot in self.generator_endpoints.iter_mut().flatten() {
            match synchronize_endpoint_epoch(&mut slot.endpoint, epoch) {
                EndpointEpochSync::AlreadyCurrent => synchronized = true,
                EndpointEpochSync::Reset => {
                    resets += 1;
                    synchronized = true;
                }
                EndpointEpochSync::Failed => failures += 1,
            }
            let _ = slot.endpoint.clear_and_stage_all_notes_off();
        }
        self.admitted_live_edit_endpoint_count = 0;
        self.plugin_epoch_resets = self.plugin_epoch_resets.saturating_add(resets);
        self.plugin_epoch_reset_failures =
            self.plugin_epoch_reset_failures.saturating_add(failures);
        if synchronized {
            self.last_plugin_endpoint_epoch = epoch;
        }

        // Endpoint generations are switched before any local history is cleared. No new timeline
        // event can subsequently observe a worker, delayed-dry cache, or PDC tap from the old
        // transport generation.
        self.reset_voice_and_pdc_state(beat_q32);
        self.publish_plugin_epoch_status(status);
    }

    fn reset_voice_and_pdc_state(&mut self, beat_q32: u64) {
        for voice in &mut self.voices {
            voice.active = false;
            voice.timeline_note_id = None;
            voice.timeline_channel_id = None;
        }
        for voice in &mut self.audio_voices {
            voice.active = false;
        }
        self.click_envelope = 0.0;
        self.click_phase = 0.0;
        self.beat_phase = (beat_q32 & (BEAT_Q32_ONE - 1)) as f32 / BEAT_Q32_ONE as f32;
        for delay in &mut self.pdc_raw_track_delays {
            delay.reset();
        }
        for slot in self.generator_endpoints.iter_mut().flatten() {
            slot.pdc_delay.reset();
        }
        if let Some(bank) = self
            .timeline_runtime
            .as_mut()
            .and_then(RealtimeTimelineRuntime::active_mixer_delay_bank_mut)
        {
            bank.reset();
        }
        self.timeline_generator_notes.fill(None);
        self.timeline_generator_staged_notes.fill(None);
    }

    fn stage_all_notes_off(&mut self) {
        for slot in self.insert_endpoints.iter_mut().flatten() {
            let deferred = slot
                .endpoint
                .panic_midi_preserving_parameter_edits(None, true);
            slot.suppress_output_frames = slot
                .suppress_output_frames
                .max(fixed_quantum_fail_closed_frames(&slot.endpoint).saturating_add(deferred));
        }
        for slot in self.generator_endpoints.iter_mut().flatten() {
            let deferred = slot
                .endpoint
                .panic_midi_preserving_parameter_edits(None, true);
            slot.suppress_output_frames = slot
                .suppress_output_frames
                .max(fixed_quantum_fail_closed_frames(&slot.endpoint).saturating_add(deferred));
        }
    }

    fn publish_plugin_epoch_status(&self, status: &AudioStatus) {
        status
            .plugin_epoch_resets
            .store(self.plugin_epoch_resets, Ordering::Relaxed);
        status
            .plugin_epoch_reset_failures
            .store(self.plugin_epoch_reset_failures, Ordering::Relaxed);
        status
            .last_plugin_endpoint_epoch
            .store(self.last_plugin_endpoint_epoch, Ordering::Relaxed);
        status.plugin_fixed_quantum_frames.store(
            DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES as u32,
            Ordering::Relaxed,
        );
        status
            .plugin_fixed_quantum_event_overflows
            .store(self.fixed_quantum_event_overflows, Ordering::Relaxed);
        status
            .plugin_fixed_quantum_invalid_events
            .store(self.fixed_quantum_invalid_events, Ordering::Relaxed);
        status.plugin_fixed_quantum_event_rejections.store(
            self.fixed_quantum_endpoint_event_rejections,
            Ordering::Relaxed,
        );
        status
            .plugin_fixed_quantum_bridge_gaps
            .store(self.fixed_quantum_bridge_gaps, Ordering::Relaxed);
        status.plugin_fixed_quantum_output_underflow_frames.store(
            self.fixed_quantum_output_underflow_frames,
            Ordering::Relaxed,
        );
        status
            .timeline_execution_failures
            .store(self.timeline_execution_failures, Ordering::Relaxed);
        status
            .timeline_missing_assets
            .store(self.timeline_missing_assets, Ordering::Relaxed);
        status
            .timeline_automation_pending
            .store(self.timeline_automation_pending, Ordering::Relaxed);
        status
            .timeline_automation_unsupported
            .store(self.timeline_automation_unsupported, Ordering::Relaxed);
    }

    fn record_endpoint_epoch_sync(&mut self, sync: EndpointEpochSync, epoch: u64) -> bool {
        match sync {
            EndpointEpochSync::AlreadyCurrent => {
                self.last_plugin_endpoint_epoch = epoch;
                true
            }
            EndpointEpochSync::Reset => {
                self.plugin_epoch_resets = self.plugin_epoch_resets.saturating_add(1);
                self.last_plugin_endpoint_epoch = epoch;
                true
            }
            EndpointEpochSync::Failed => {
                self.plugin_epoch_reset_failures =
                    self.plugin_epoch_reset_failures.saturating_add(1);
                false
            }
        }
    }

    fn refresh_pdc_plan(&mut self, status: &AudioStatus, _frames: usize) {
        if self.mixer_graph_was_activated {
            if !self.refresh_graph_pdc_plan(status) {
                self.fail_mixer_graph_render(0);
            }
            return;
        }
        let mut insert_latencies = [0_u32; TRACK_COUNT];
        let mut master_latency_samples = 0;
        let mut latency_identity_changed = false;
        for (insert, slot) in self.insert_endpoints.iter_mut().enumerate() {
            let Some(slot) = slot else {
                continue;
            };
            latency_identity_changed |= slot.endpoint.refresh_latency_snapshot();
            let latency = fixed_quantum_endpoint_latency(&slot.endpoint);
            if insert == 0 {
                master_latency_samples = latency;
            } else {
                insert_latencies[insert] = latency;
            }
        }

        let empty_generator = GeneratorPathLatency {
            endpoint_id: 0,
            channel_id: 0,
            mixer_track: 0,
            latency_samples: 0,
        };
        let mut generator_paths = [empty_generator; MAX_GENERATOR_ENDPOINTS];
        let mut generator_indices = [0_usize; MAX_GENERATOR_ENDPOINTS];
        let mut generator_count = 0;
        for (index, slot) in self.generator_endpoints.iter_mut().enumerate() {
            let Some(slot) = slot else {
                continue;
            };
            latency_identity_changed |= slot.endpoint.refresh_latency_snapshot();
            generator_paths[generator_count] = GeneratorPathLatency {
                endpoint_id: slot.endpoint_id,
                channel_id: slot.channel_id,
                mixer_track: slot.mixer_track,
                latency_samples: fixed_quantum_endpoint_latency(&slot.endpoint),
            };
            generator_indices[generator_count] = index;
            generator_count += 1;
        }

        let Ok(next_plan) = PdcPlan::build(
            insert_latencies,
            master_latency_samples,
            &generator_paths[..generator_count],
            self.pdc_maximum_delay_samples,
        ) else {
            return;
        };
        let changed = next_plan != self.pdc_plan;
        let raw_crossfade_frames = if self.pdc_plan_revision == 0 {
            0
        } else {
            PDC_TAP_CROSSFADE_FRAMES
        };
        for (track, delay_line) in self.pdc_raw_track_delays.iter_mut().enumerate() {
            if let Some(delay) = next_plan.raw_track_delay(track) {
                let _ = delay_line.request_delay(delay.applied_samples(), raw_crossfade_frames);
            }
        }
        for (index, compensation) in generator_indices[..generator_count]
            .iter()
            .copied()
            .zip(next_plan.generator_delays())
        {
            if let Some(slot) = self.generator_endpoints[index].as_mut() {
                let crossfade_frames = if slot.pdc_initialized {
                    PDC_TAP_CROSSFADE_FRAMES
                } else {
                    0
                };
                let _ = slot
                    .pdc_delay
                    .request_delay(compensation.delay.applied_samples(), crossfade_frames);
                slot.pdc_initialized = true;
            }
        }
        if changed {
            self.pdc_plan = next_plan;
        }

        if changed || latency_identity_changed || self.pdc_plan_revision == 0 {
            self.pdc_plan_revision = next_nonzero_id(self.pdc_plan_revision);
            self.publish_pdc_status(status);
        }

        if let (Some(revision), Some(epoch)) =
            (self.timeline_channel_revision, self.timeline_channel_epoch)
        {
            let automation_identity_matches = self
                .timeline_plugin_automation_bindings
                .is_bound_to(revision, epoch)
                && self.timeline_plugin_automation_bindings.identities_match(
                    &mut self.generator_endpoints,
                    &mut self.insert_endpoints,
                    self.pdc_plan_revision,
                );
            if !automation_identity_matches {
                if let Some(runtime) = self.timeline_runtime.as_mut() {
                    runtime.require_resync();
                }
                self.fail_timeline_block();
            }
        }
    }

    fn refresh_graph_pdc_plan(&mut self, status: &AudioStatus) -> bool {
        if !self.mixer_graph_binding_is_exact() {
            return false;
        }

        let changed = {
            let DspState {
                timeline_runtime,
                insert_endpoints,
                generator_endpoints,
                pdc_raw_track_delays,
                pdc_plan_revision,
                pdc_maximum_delay_samples,
                graph_pdc_plan,
                graph_pdc_activation_plan,
                mixer_graph_endpoint_identities,
                mixer_graph_activation_endpoint_identities,
                timeline_plugin_automation_bindings,
                ..
            } = self;
            let Some(runtime) = timeline_runtime.as_ref() else {
                return false;
            };
            let Some(graph) = runtime.active_timeline().map(CompiledTimeline::mixer_graph) else {
                return false;
            };
            let Some(next_plan) = Self::build_graph_pdc_plan(
                graph,
                mixer_graph_activation_endpoint_identities,
                insert_endpoints,
                generator_endpoints,
                *pdc_maximum_delay_samples,
            ) else {
                mixer_graph_activation_endpoint_identities.clear();
                return false;
            };
            if runtime
                .active_mixer_delay_bank()
                .and_then(|bank| bank.preflight_plan(&next_plan).ok())
                .is_none()
                || !mixer_graph_activation_endpoint_identities
                    .matches_fresh(insert_endpoints, generator_endpoints)
            {
                mixer_graph_activation_endpoint_identities.clear();
                return false;
            }

            let plan_changed = graph_pdc_plan.as_ref().as_ref() != Some(&next_plan);
            let identity_changed =
                **mixer_graph_endpoint_identities != **mixer_graph_activation_endpoint_identities;
            if !plan_changed && !identity_changed {
                mixer_graph_activation_endpoint_identities.clear();
                return true;
            }

            // Retiming an already-running Q128 parameter history requires a new
            // transport chase. Preserve the old graph transaction and fail
            // closed instead of mixing control and audio time domains.
            if timeline_plugin_automation_bindings.iter().next().is_some() {
                mixer_graph_activation_endpoint_identities.clear();
                return false;
            }

            for node in next_plan.nodes() {
                if node.raw_source_delay.applied_samples()
                    > pdc_raw_track_delays[node.runtime_slot].maximum_delay_samples()
                {
                    mixer_graph_activation_endpoint_identities.clear();
                    return false;
                }
            }
            for compensation in next_plan.generators() {
                let Some(endpoint) = generator_endpoints.iter().flatten().find(|endpoint| {
                    endpoint.endpoint_id == compensation.endpoint_id
                        && endpoint.channel_id == compensation.channel_id
                }) else {
                    mixer_graph_activation_endpoint_identities.clear();
                    return false;
                };
                if compensation.delay.applied_samples() > endpoint.pdc_delay.maximum_delay_samples()
                {
                    mixer_graph_activation_endpoint_identities.clear();
                    return false;
                }
            }

            **graph_pdc_activation_plan = Some(next_plan);
            let candidate_plan = graph_pdc_activation_plan
                .as_ref()
                .as_ref()
                .expect("validated dynamic graph plan remains staged");
            let bank_request = timeline_runtime
                .as_mut()
                .and_then(RealtimeTimelineRuntime::active_mixer_delay_bank_mut)
                .expect("exact graph identity retains its delay bank")
                .request_plan(candidate_plan, PDC_TAP_CROSSFADE_FRAMES);
            debug_assert!(
                bank_request.is_ok(),
                "preflighted graph route delays remain valid"
            );
            for node in candidate_plan.nodes() {
                let request = pdc_raw_track_delays[node.runtime_slot].request_delay(
                    node.raw_source_delay.applied_samples(),
                    PDC_TAP_CROSSFADE_FRAMES,
                );
                debug_assert!(request.is_ok(), "preflighted graph raw delay remains valid");
            }
            for compensation in candidate_plan.generators() {
                let endpoint = generator_endpoints
                    .iter_mut()
                    .flatten()
                    .find(|endpoint| {
                        endpoint.endpoint_id == compensation.endpoint_id
                            && endpoint.channel_id == compensation.channel_id
                    })
                    .expect("preflighted graph generator remains installed");
                let crossfade_frames = if endpoint.pdc_initialized {
                    PDC_TAP_CROSSFADE_FRAMES
                } else {
                    0
                };
                let request = endpoint
                    .pdc_delay
                    .request_delay(compensation.delay.applied_samples(), crossfade_frames);
                debug_assert!(request.is_ok(), "preflighted generator delay remains valid");
                endpoint.pdc_initialized = true;
            }
            std::mem::swap(graph_pdc_plan, graph_pdc_activation_plan);
            **graph_pdc_activation_plan = None;
            std::mem::swap(
                mixer_graph_endpoint_identities,
                mixer_graph_activation_endpoint_identities,
            );
            mixer_graph_activation_endpoint_identities.clear();
            *pdc_plan_revision = next_nonzero_id(*pdc_plan_revision);
            mixer_graph_endpoint_identities
                .apply_expected_revisions(insert_endpoints, generator_endpoints);
            true
        };
        if changed {
            self.publish_graph_pdc_status(status);
        }
        true
    }

    fn publish_graph_pdc_status(&self, status: &AudioStatus) {
        let Some(plan) = self.graph_pdc_plan.as_ref().as_ref() else {
            return;
        };
        let master_stage_latency = plan
            .node(plan.master_node_id())
            .and_then(|node| u32::try_from(node.stage_latency_samples).ok())
            .unwrap_or(u32::MAX);
        let output_latency = plan.master_output_latency_samples();
        let sequence = status.pdc_sequence.fetch_add(1, Ordering::Acquire);
        debug_assert_eq!(sequence & 1, 0);
        fence(Ordering::Release);
        status
            .pdc_plan_revision
            .store(self.pdc_plan_revision, Ordering::Relaxed);
        status
            .pdc_reference_latency_samples
            .store(output_latency, Ordering::Relaxed);
        status
            .pdc_master_latency_samples
            .store(master_stage_latency, Ordering::Relaxed);
        status
            .pdc_output_latency_samples
            .store(output_latency, Ordering::Relaxed);
        status.pdc_clamped_path_count.store(
            u32::from(plan.diagnostics().total_clamped_delays()),
            Ordering::Relaxed,
        );
        status
            .pdc_maximum_delay_samples
            .store(plan.maximum_delay_samples(), Ordering::Relaxed);
        status
            .pdc_sequence
            .store(sequence.wrapping_add(2), Ordering::Release);
    }

    fn publish_pdc_status(&self, status: &AudioStatus) {
        let clamped_path_count = self
            .pdc_raw_track_delays
            .iter()
            .enumerate()
            .filter(|(track, _)| {
                self.pdc_plan
                    .raw_track_delay(*track)
                    .is_some_and(|delay| delay.is_clamped())
            })
            .count()
            .saturating_add(
                self.pdc_plan
                    .generator_delays()
                    .filter(|generator| generator.delay.is_clamped())
                    .count(),
            )
            .min(u32::MAX as usize) as u32;
        let sequence = status.pdc_sequence.fetch_add(1, Ordering::Acquire);
        debug_assert_eq!(sequence & 1, 0);
        fence(Ordering::Release);
        status
            .pdc_plan_revision
            .store(self.pdc_plan_revision, Ordering::Relaxed);
        status
            .pdc_reference_latency_samples
            .store(self.pdc_plan.reference_latency_samples(), Ordering::Relaxed);
        status
            .pdc_master_latency_samples
            .store(self.pdc_plan.master_latency_samples(), Ordering::Relaxed);
        status
            .pdc_output_latency_samples
            .store(self.pdc_plan.output_latency_samples(), Ordering::Relaxed);
        status
            .pdc_clamped_path_count
            .store(clamped_path_count, Ordering::Relaxed);
        status
            .pdc_maximum_delay_samples
            .store(self.pdc_maximum_delay_samples, Ordering::Relaxed);
        status
            .pdc_sequence
            .store(sequence.wrapping_add(2), Ordering::Release);
    }

    fn process_raw_source_pdc(&mut self, frames: usize) {
        for track in 0..TRACK_COUNT {
            let start = track * MAX_MIXER_BLOCK_FRAMES;
            let delay = &mut self.pdc_raw_track_delays[track];
            for frame_index in 0..frames {
                let index = start + frame_index;
                self.track_block[index] = delay.process_sample(self.track_block[index]);
            }
        }
    }

    fn endpoint_command_ready(&self, command: &AudioCommand) -> bool {
        match command {
            AudioCommand::InstallInsertEndpoint {
                insert,
                endpoint_id,
                ..
            } => {
                let retirements = if *insert >= TRACK_COUNT || *endpoint_id == 0 {
                    1
                } else {
                    usize::from(self.insert_endpoints[*insert].is_some())
                };
                self.insert_lifecycle_command_ready(retirements)
            }
            AudioCommand::RemoveInsertEndpoint { insert } => {
                let retirements = self
                    .insert_endpoints
                    .get(*insert)
                    .map_or(0, |slot| usize::from(slot.is_some()));
                self.insert_lifecycle_command_ready(retirements)
            }
            AudioCommand::ClearInsertEndpoints { .. } => {
                let retirements = self
                    .insert_endpoints
                    .iter()
                    .filter(|slot| slot.is_some())
                    .count();
                self.insert_lifecycle_command_ready(retirements)
            }
            AudioCommand::InstallGeneratorEndpoint {
                channel_id,
                endpoint_id,
                plugin_instance_id,
                mixer_track,
                pdc_delay,
                ..
            } => {
                let invalid = *endpoint_id == 0
                    || *plugin_instance_id == 0
                    || *mixer_track >= TRACK_COUNT
                    || pdc_delay.maximum_delay_samples() < self.pdc_maximum_delay_samples;
                let retirements = usize::from(
                    invalid
                        || self.find_generator_slot(*channel_id).is_some()
                        || self.generator_endpoints.iter().all(Option::is_some),
                );
                self.generator_lifecycle_command_ready(retirements)
            }
            AudioCommand::RemoveGeneratorEndpoint { channel_id } => {
                let retirements = usize::from(self.find_generator_slot(*channel_id).is_some());
                self.generator_lifecycle_command_ready(retirements)
            }
            AudioCommand::ClearGeneratorEndpoints { .. } => {
                let retirements = self
                    .generator_endpoints
                    .iter()
                    .filter(|slot| slot.is_some())
                    .count();
                self.generator_lifecycle_command_ready(retirements)
            }
            AudioCommand::SetGeneratorRoute { .. } => self.generator_lifecycle_command_ready(0),
            AudioCommand::EditPluginParameter(_) => self
                .parameter_edit_callback_events
                .as_ref()
                .is_some_and(|events| events.slots() > 0),
            AudioCommand::InstallMidiInput { .. }
            | AudioCommand::RemoveMidiInput { .. }
            | AudioCommand::ClearMidiInput { .. } => self.midi_input_lifecycle_command_ready(),
            AudioCommand::StartMidiRecording { .. }
            | AudioCommand::StopMidiRecording { .. }
            | AudioCommand::ClearMidiRecording { .. } => self.midi_recording_command_ready(),
            AudioCommand::InstallMasterCapture { .. }
            | AudioCommand::StopMasterCapture { .. }
            | AudioCommand::ClearMasterCapture { .. } => self.master_capture_command_ready(),
            _ => true,
        }
    }

    fn insert_lifecycle_command_ready(&self, retirements: usize) -> bool {
        let Some(retired) = self.retired_insert_endpoints.as_ref() else {
            return false;
        };
        let Some(events) = self.insert_endpoint_events.as_ref() else {
            return false;
        };
        retired.slots() >= retirements && events.slots() > 0
    }

    fn generator_lifecycle_command_ready(&self, retirements: usize) -> bool {
        let Some(retired) = self.retired_insert_endpoints.as_ref() else {
            return false;
        };
        let Some(events) = self.generator_endpoint_events.as_ref() else {
            return false;
        };
        retired.slots() >= retirements && events.slots() > 0
    }

    fn master_capture_command_ready(&self) -> bool {
        self.master_capture_events
            .as_ref()
            .is_some_and(|events| events.slots() > 0)
    }

    fn midi_input_lifecycle_command_ready(&self) -> bool {
        self.retired_midi_inputs
            .as_ref()
            .is_some_and(|retired| retired.slots() > 0)
            && self
                .midi_input_route_events
                .as_ref()
                .is_some_and(|events| events.slots() > 0)
    }

    fn midi_recording_command_ready(&self) -> bool {
        self.midi_recording_endpoint_events
            .as_ref()
            .is_some_and(|events| events.slots() > self.midi_recording_event_reservations)
    }

    fn install_insert_endpoint(
        &mut self,
        insert: usize,
        endpoint_id: u64,
        mut endpoint: PreparedFixedEndpoint,
    ) {
        if insert >= TRACK_COUNT || endpoint_id == 0 {
            self.retire_insert_endpoint(endpoint);
            self.emit_insert_endpoint_event(InsertEndpointEvent::Installed {
                insert,
                endpoint_id,
                replaced_endpoint_id: None,
                success: false,
            });
            return;
        }

        let epoch_sync = synchronize_endpoint_epoch(&mut endpoint, self.transport_epoch);
        if !self.record_endpoint_epoch_sync(epoch_sync, self.transport_epoch) {
            self.retire_insert_endpoint(endpoint);
            self.emit_insert_endpoint_event(InsertEndpointEvent::Installed {
                insert,
                endpoint_id,
                replaced_endpoint_id: None,
                success: false,
            });
            return;
        }

        let replaced = self.insert_endpoints[insert].take();
        let replaced_endpoint_id = replaced.as_ref().map(|slot| slot.endpoint_id);
        let invalidates_timeline = replaced_endpoint_id.is_some_and(|endpoint_id| {
            self.timeline_plugin_automation_bindings
                .references_insert_endpoint(insert, endpoint_id)
                || self
                    .timeline_activation_plugin_automation_bindings
                    .references_insert_endpoint(insert, endpoint_id)
        });
        if let Some(slot) = replaced {
            self.retire_insert_endpoint(slot.endpoint);
        }
        self.insert_endpoints[insert] = Some(InsertEndpointSlot {
            endpoint_id,
            endpoint,
            suppress_output_frames: 0,
        });
        self.emit_insert_endpoint_event(InsertEndpointEvent::Installed {
            insert,
            endpoint_id,
            replaced_endpoint_id,
            success: true,
        });
        if invalidates_timeline {
            if let Some(runtime) = self.timeline_runtime.as_mut() {
                runtime.require_resync();
            }
            self.fail_timeline_block();
        }
    }

    fn remove_insert_endpoint(&mut self, insert: usize) {
        let removed = self.insert_endpoints.get_mut(insert).and_then(Option::take);
        let endpoint_id = removed.as_ref().map(|slot| slot.endpoint_id);
        let invalidates_timeline = endpoint_id.is_some_and(|endpoint_id| {
            self.timeline_plugin_automation_bindings
                .references_insert_endpoint(insert, endpoint_id)
                || self
                    .timeline_activation_plugin_automation_bindings
                    .references_insert_endpoint(insert, endpoint_id)
        });
        if let Some(slot) = removed {
            self.retire_insert_endpoint(slot.endpoint);
        }
        self.emit_insert_endpoint_event(InsertEndpointEvent::Removed {
            insert,
            endpoint_id,
        });
        if invalidates_timeline {
            if let Some(runtime) = self.timeline_runtime.as_mut() {
                runtime.require_resync();
            }
            self.fail_timeline_block();
        }
    }

    fn clear_insert_endpoints(&mut self, request_id: u64) {
        let invalidates_timeline = self
            .timeline_plugin_automation_bindings
            .iter()
            .any(|binding| {
                matches!(
                    binding.endpoint,
                    TimelinePluginEndpointIdentity::MixerInsert { .. }
                )
            })
            || self
                .timeline_activation_plugin_automation_bindings
                .iter()
                .any(|binding| {
                    matches!(
                        binding.endpoint,
                        TimelinePluginEndpointIdentity::MixerInsert { .. }
                    )
                });
        let mut removed = 0;
        for insert in 0..TRACK_COUNT {
            if let Some(slot) = self.insert_endpoints[insert].take() {
                self.retire_insert_endpoint(slot.endpoint);
                removed += 1;
            }
        }
        self.emit_insert_endpoint_event(InsertEndpointEvent::Cleared {
            request_id,
            removed,
        });
        if invalidates_timeline {
            if let Some(runtime) = self.timeline_runtime.as_mut() {
                runtime.require_resync();
            }
            self.fail_timeline_block();
        }
    }

    fn retire_insert_endpoint(&mut self, endpoint: PreparedFixedEndpoint) {
        if endpoint.has_admitted_live_edit_marker() {
            self.admitted_live_edit_endpoint_count =
                self.admitted_live_edit_endpoint_count.saturating_sub(1);
        }
        self.retire_endpoint_resource(RetiredEndpointResource {
            _endpoint: endpoint,
            _pdc_delay: None,
        });
    }

    fn retire_generator_endpoint(&mut self, slot: GeneratorEndpointSlot) {
        if slot.endpoint.has_admitted_live_edit_marker() {
            self.admitted_live_edit_endpoint_count =
                self.admitted_live_edit_endpoint_count.saturating_sub(1);
        }
        self.retire_endpoint_resource(RetiredEndpointResource {
            _endpoint: slot.endpoint,
            _pdc_delay: Some(slot.pdc_delay),
        });
    }

    fn retire_endpoint_resource(&mut self, resource: RetiredEndpointResource) {
        let Some(retired) = self.retired_insert_endpoints.as_mut() else {
            std::mem::forget(resource);
            return;
        };
        if let Err(PushError::Full(resource)) = retired.push(resource) {
            // Lifecycle commands are capacity-checked before being popped. If the
            // invariant is ever broken, leaking is safer than running endpoint
            // destruction on the device callback.
            std::mem::forget(resource);
        }
    }

    fn emit_insert_endpoint_event(&mut self, event: InsertEndpointEvent) {
        if let Some(events) = self.insert_endpoint_events.as_mut() {
            let _ = events.push(event);
        }
    }

    fn find_generator_slot(&self, channel_id: u32) -> Option<usize> {
        self.generator_endpoints.iter().position(|slot| {
            slot.as_ref()
                .is_some_and(|slot| slot.channel_id == channel_id)
        })
    }

    fn install_generator_endpoint(
        &mut self,
        channel_id: u32,
        endpoint_id: u64,
        plugin_instance_id: u64,
        mixer_track: usize,
        mut endpoint: PreparedFixedEndpoint,
        pdc_delay: StereoDelayLine,
    ) {
        if endpoint_id == 0
            || plugin_instance_id == 0
            || mixer_track >= TRACK_COUNT
            || pdc_delay.maximum_delay_samples() < self.pdc_maximum_delay_samples
        {
            self.retire_endpoint_resource(RetiredEndpointResource {
                _endpoint: endpoint,
                _pdc_delay: Some(pdc_delay),
            });
            self.emit_generator_endpoint_event(GeneratorEndpointEvent::Installed {
                channel_id,
                endpoint_id,
                plugin_instance_id,
                mixer_track,
                replaced_endpoint_id: None,
                replaced_plugin_instance_id: None,
                success: false,
            });
            return;
        }

        let target = self
            .find_generator_slot(channel_id)
            .or_else(|| self.generator_endpoints.iter().position(Option::is_none));
        let Some(target) = target else {
            self.retire_endpoint_resource(RetiredEndpointResource {
                _endpoint: endpoint,
                _pdc_delay: Some(pdc_delay),
            });
            self.emit_generator_endpoint_event(GeneratorEndpointEvent::Installed {
                channel_id,
                endpoint_id,
                plugin_instance_id,
                mixer_track,
                replaced_endpoint_id: None,
                replaced_plugin_instance_id: None,
                success: false,
            });
            return;
        };

        let epoch_sync = synchronize_endpoint_epoch(&mut endpoint, self.transport_epoch);
        if !self.record_endpoint_epoch_sync(epoch_sync, self.transport_epoch) {
            self.retire_endpoint_resource(RetiredEndpointResource {
                _endpoint: endpoint,
                _pdc_delay: Some(pdc_delay),
            });
            self.emit_generator_endpoint_event(GeneratorEndpointEvent::Installed {
                channel_id,
                endpoint_id,
                plugin_instance_id,
                mixer_track,
                replaced_endpoint_id: None,
                replaced_plugin_instance_id: None,
                success: false,
            });
            return;
        }

        let replaced = self.generator_endpoints[target].take();
        let replaced_endpoint_id = replaced.as_ref().map(|slot| slot.endpoint_id);
        let replaced_plugin_instance_id = replaced.as_ref().map(|slot| slot.plugin_instance_id);
        if let Some(slot) = replaced {
            self.clear_timeline_generator_bindings(slot.endpoint_id, slot.plugin_instance_id);
            self.retire_generator_endpoint(slot);
        }
        self.generator_endpoints[target] = Some(GeneratorEndpointSlot {
            channel_id,
            endpoint_id,
            plugin_instance_id,
            mixer_track,
            endpoint,
            pdc_delay,
            pdc_initialized: false,
            suppress_output_frames: 0,
        });
        self.emit_generator_endpoint_event(GeneratorEndpointEvent::Installed {
            channel_id,
            endpoint_id,
            plugin_instance_id,
            mixer_track,
            replaced_endpoint_id,
            replaced_plugin_instance_id,
            success: true,
        });
    }

    fn remove_generator_endpoint(&mut self, channel_id: u32) {
        let removed = self
            .find_generator_slot(channel_id)
            .and_then(|index| self.generator_endpoints[index].take());
        let endpoint_id = removed.as_ref().map(|slot| slot.endpoint_id);
        let plugin_instance_id = removed.as_ref().map(|slot| slot.plugin_instance_id);
        if let Some(slot) = removed {
            self.clear_timeline_generator_bindings(slot.endpoint_id, slot.plugin_instance_id);
            self.retire_generator_endpoint(slot);
        }
        self.emit_generator_endpoint_event(GeneratorEndpointEvent::Removed {
            channel_id,
            endpoint_id,
            plugin_instance_id,
        });
    }

    fn clear_generator_endpoints(&mut self, request_id: u64) {
        let mut removed = 0;
        for index in 0..MAX_GENERATOR_ENDPOINTS {
            if let Some(slot) = self.generator_endpoints[index].take() {
                self.clear_timeline_generator_bindings(slot.endpoint_id, slot.plugin_instance_id);
                self.retire_generator_endpoint(slot);
                removed += 1;
            }
        }
        self.emit_generator_endpoint_event(GeneratorEndpointEvent::Cleared {
            request_id,
            removed,
        });
    }

    fn set_generator_route(&mut self, channel_id: u32, mixer_track: usize) {
        let success = if mixer_track < TRACK_COUNT {
            if let Some(index) = self.find_generator_slot(channel_id) {
                let epoch_sync = self.generator_endpoints[index].as_mut().map(|slot| {
                    synchronize_endpoint_epoch(&mut slot.endpoint, self.transport_epoch)
                });
                let epoch_ready = epoch_sync.is_some_and(|sync| {
                    self.record_endpoint_epoch_sync(sync, self.transport_epoch)
                });
                if epoch_ready {
                    let slot = self.generator_endpoints[index]
                        .as_mut()
                        .expect("located generator endpoint must remain installed");
                    if slot.mixer_track != mixer_track {
                        slot.mixer_track = mixer_track;
                        slot.pdc_delay.reset();
                        slot.pdc_initialized = false;
                        slot.suppress_output_frames = slot
                            .suppress_output_frames
                            .max(fixed_quantum_fail_closed_frames(&slot.endpoint));
                    }
                    true
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        };
        let identity = self.find_generator_slot(channel_id).and_then(|index| {
            self.generator_endpoints[index]
                .as_ref()
                .map(|slot| (slot.endpoint_id, slot.plugin_instance_id))
        });
        self.emit_generator_endpoint_event(GeneratorEndpointEvent::RouteSet {
            channel_id,
            endpoint_id: identity.map(|identity| identity.0),
            plugin_instance_id: identity.map(|identity| identity.1),
            mixer_track,
            success,
        });
    }

    fn clear_timeline_generator_bindings(&mut self, endpoint_id: u64, plugin_instance_id: u64) {
        for binding in self.timeline_generator_notes.iter_mut() {
            if binding.is_some_and(|binding| {
                binding.endpoint_id == endpoint_id
                    && binding.plugin_instance_id == plugin_instance_id
            }) {
                *binding = None;
            }
        }
        for binding in self.timeline_generator_staged_notes.iter_mut() {
            if binding.is_some_and(|binding| {
                binding.endpoint_id == endpoint_id
                    && binding.plugin_instance_id == plugin_instance_id
            }) {
                *binding = None;
            }
        }
        let invalidates_active = self
            .timeline_plugin_automation_bindings
            .references_generator_endpoint(endpoint_id, plugin_instance_id);
        if self
            .timeline_activation_plugin_automation_bindings
            .references_generator_endpoint(endpoint_id, plugin_instance_id)
        {
            self.timeline_activation_plugin_automation_bindings.clear();
        }
        if invalidates_active {
            if let Some(runtime) = self.timeline_runtime.as_mut() {
                runtime.require_resync();
            }
            self.fail_timeline_block();
        }
    }

    fn emit_generator_endpoint_event(&mut self, event: GeneratorEndpointEvent) {
        if let Some(events) = self.generator_endpoint_events.as_mut() {
            let _ = events.push(event);
        }
    }

    fn exact_midi_generator_index(&self, stamp: MidiGeneratorRouteStamp) -> Option<usize> {
        let index = self.find_generator_slot(stamp.channel_id)?;
        let slot = self.generator_endpoints[index].as_ref()?;
        let target_slot = stamp.slot.unwrap_or(0);
        (slot.endpoint_id == stamp.endpoint_id
            && slot.plugin_instance_id == stamp.plugin_instance_id
            && slot.endpoint.project_session == stamp.project_session
            && (stamp.slot.is_some() || slot.endpoint.manifest.slot_count == 1)
            && slot.endpoint.manifest.instance_id(target_slot) == Some(stamp.plugin_instance_id))
        .then_some(index)
    }

    fn effective_generator_mixer_track(&self, index: usize) -> usize {
        let slot = self.generator_endpoints[index]
            .as_ref()
            .expect("Generator endpoint index is occupied");
        self.timeline_channel_bases
            .get(slot.channel_id)
            .map_or(slot.mixer_track, |base| base.mixer_track)
    }

    fn install_midi_input(&mut self, route_id: u64, prepared: PreparedMidiInputRoute) {
        let connection_epoch = prepared.connection_epoch();
        let valid = route_id != 0
            && connection_epoch != 0
            && self.exact_midi_generator_index(prepared.stamp()).is_some();
        if !valid {
            self.retire_midi_input(prepared);
            self.emit_midi_input_route_event(MidiInputRouteEvent::Installed {
                route_id,
                connection_epoch,
                replaced_route_id: None,
                success: false,
            });
            return;
        }

        let replaced = self.midi_input.take();
        let replaced_route_id = replaced.as_ref().map(|route| route.route_id);
        if let Some(route) = replaced {
            self.panic_exact_midi_destination(route.prepared.stamp());
            self.retire_midi_input(route.prepared);
        }
        self.midi_input = Some(RealtimeMidiInputRoute::new(
            route_id,
            prepared,
            self.transport_epoch,
        ));
        self.emit_midi_input_route_event(MidiInputRouteEvent::Installed {
            route_id,
            connection_epoch,
            replaced_route_id,
            success: true,
        });
    }

    fn remove_midi_input(&mut self, route_id: u64) {
        let removed = self
            .midi_input
            .as_ref()
            .is_some_and(|route| route.route_id == route_id);
        if removed {
            let route = self.midi_input.take().expect("matching MIDI route exists");
            self.panic_exact_midi_destination(route.prepared.stamp());
            self.retire_midi_input(route.prepared);
        }
        self.emit_midi_input_route_event(MidiInputRouteEvent::Removed { route_id, removed });
    }

    fn clear_midi_input(&mut self, request_id: u64) {
        let removed_route_id = self.midi_input.as_ref().map(|route| route.route_id);
        if let Some(route) = self.midi_input.take() {
            self.panic_exact_midi_destination(route.prepared.stamp());
            self.retire_midi_input(route.prepared);
        }
        self.emit_midi_input_route_event(MidiInputRouteEvent::Cleared {
            request_id,
            removed_route_id,
        });
    }

    fn panic_exact_midi_destination(&mut self, stamp: MidiGeneratorRouteStamp) {
        let Some(index) = self.exact_midi_generator_index(stamp) else {
            return;
        };
        let timeline_owns_destination = self
            .timeline_generator_routes
            .route_for_channel(stamp.channel_id)
            .is_some_and(|route| {
                route.endpoint_id == stamp.endpoint_id
                    && route.plugin_instance_id == stamp.plugin_instance_id
            });
        if timeline_owns_destination {
            if let Some(runtime) = self.timeline_runtime.as_mut() {
                runtime.require_resync();
            }
            self.fail_timeline_block();
        }
        let slot = self.generator_endpoints[index]
            .as_mut()
            .expect("exact MIDI endpoint remains installed");
        let phase = slot.endpoint.input_phase_frames();
        let deferred = slot
            .endpoint
            .panic_midi_preserving_parameter_edits(Some(stamp.slot.unwrap_or(0) as u8), false);
        slot.suppress_output_frames = slot
            .suppress_output_frames
            .max(fixed_quantum_fail_closed_frames(&slot.endpoint).saturating_add(deferred));
        self.paused_midi_safety[index] = Some(PausedMidiSafetyService {
            stamp,
            remaining_frames: midi_safety_frames_to_complete_quantum(phase, deferred),
        });
    }

    fn retire_midi_input(&mut self, prepared: PreparedMidiInputRoute) {
        let resource = RetiredMidiInputResource {
            _prepared: prepared,
        };
        let Some(retired) = self.retired_midi_inputs.as_mut() else {
            std::mem::forget(resource);
            return;
        };
        if let Err(PushError::Full(resource)) = retired.push(resource) {
            std::mem::forget(resource);
        }
    }

    fn emit_midi_input_route_event(&mut self, event: MidiInputRouteEvent) {
        if let Some(events) = self.midi_input_route_events.as_mut() {
            let _ = events.push(event);
        }
    }

    fn start_midi_recording(&mut self, mut endpoint: PreparedMidiRecordEndpoint) {
        self.midi_recording_event_reservations =
            self.midi_recording_event_reservations.saturating_add(1);
        let stamp = endpoint.stamp();
        if !stamp.is_valid()
            || self.midi_recording.is_some()
            || self.pending_midi_recording_start.is_some()
        {
            endpoint.invalidate(MidiTakeInvalidReason::ProjectTargetChanged);
            self.emit_midi_recording_endpoint_event(MidiRecordingEndpointEvent::Started {
                stamp,
                start: self.callback_transport_anchor,
                success: false,
                returned_endpoint: Some(endpoint),
            });
            return;
        }
        self.pending_midi_recording_start = Some(endpoint);
    }

    fn stop_midi_recording(&mut self, session_id: u64) {
        self.midi_recording_event_reservations =
            self.midi_recording_event_reservations.saturating_add(1);
        if self
            .pending_midi_recording_start
            .as_ref()
            .is_some_and(|endpoint| endpoint.stamp().session_id == session_id)
        {
            let mut endpoint = self
                .pending_midi_recording_start
                .take()
                .expect("matching pending MIDI recorder exists");
            let stamp = endpoint.stamp();
            endpoint.invalidate(MidiTakeInvalidReason::TransportNotPlaying);
            // The pending Start owns an earlier receipt reservation. Resolve it before the Stop
            // receipt so control observes a total, ordered ownership history.
            self.emit_midi_recording_endpoint_event(MidiRecordingEndpointEvent::Started {
                stamp,
                start: self.callback_transport_anchor,
                success: false,
                returned_endpoint: Some(endpoint),
            });
            self.emit_midi_recording_endpoint_event(MidiRecordingEndpointEvent::Stopped {
                requested_session_id: session_id,
                stamp: None,
                stop: self.callback_transport_anchor,
                returned_endpoint: None,
            });
            return;
        }

        let matching = self
            .midi_recording
            .as_ref()
            .is_some_and(|recording| recording.endpoint.stamp().session_id == session_id);
        let (stamp, returned_endpoint) = if matching {
            self.validate_active_midi_recording_at_callback_boundary();
            let mut recording = self
                .midi_recording
                .take()
                .expect("matching active MIDI recorder exists");
            recording.endpoint.seal();
            (Some(recording.endpoint.stamp()), Some(recording.endpoint))
        } else {
            (None, None)
        };
        self.emit_midi_recording_endpoint_event(MidiRecordingEndpointEvent::Stopped {
            requested_session_id: session_id,
            stamp,
            stop: self.callback_transport_anchor,
            returned_endpoint,
        });
    }

    fn clear_midi_recording(&mut self, request_id: u64) {
        self.midi_recording_event_reservations =
            self.midi_recording_event_reservations.saturating_add(1);
        let mut pending_endpoint = None;
        if let Some(mut endpoint) = self.pending_midi_recording_start.take() {
            let stamp = endpoint.stamp();
            endpoint.invalidate(MidiTakeInvalidReason::ProjectTargetChanged);
            // Clear normally returns ownership, but if an invariant breach left both an active
            // and pending endpoint, return the pending owner in its own reserved Start receipt so
            // neither value can be destroyed on the callback.
            let active_also_exists = self.midi_recording.is_some();
            if active_also_exists {
                self.emit_midi_recording_endpoint_event(MidiRecordingEndpointEvent::Started {
                    stamp,
                    start: self.callback_transport_anchor,
                    success: false,
                    returned_endpoint: Some(endpoint),
                });
            } else {
                self.emit_midi_recording_endpoint_event(MidiRecordingEndpointEvent::Started {
                    stamp,
                    start: self.callback_transport_anchor,
                    success: false,
                    returned_endpoint: None,
                });
                pending_endpoint = Some(endpoint);
            }
        }
        self.validate_active_midi_recording_at_callback_boundary();
        let active = self.midi_recording.take();
        let (stamp, returned_endpoint) = if let Some(mut recording) = active {
            recording.endpoint.seal();
            (Some(recording.endpoint.stamp()), Some(recording.endpoint))
        } else if let Some(endpoint) = pending_endpoint {
            (Some(endpoint.stamp()), Some(endpoint))
        } else {
            (None, None)
        };
        self.emit_midi_recording_endpoint_event(MidiRecordingEndpointEvent::Cleared {
            request_id,
            stamp,
            stop: self.callback_transport_anchor,
            returned_endpoint,
        });
    }

    fn set_callback_transport_boundary(&mut self, anchor: MidiRecordClockAnchor, playing: bool) {
        self.callback_transport_anchor = anchor;
        self.callback_transport_playing = playing;
    }

    fn service_midi_recording_boundary(
        &mut self,
        anchor: MidiRecordClockAnchor,
        playing: bool,
        timeline_ready: bool,
    ) {
        self.set_callback_transport_boundary(anchor, playing);
        if let Some(mut endpoint) = self.pending_midi_recording_start.take() {
            let stamp = endpoint.stamp();
            if let Some(reason) =
                self.midi_recording_start_invalid_reason(stamp, anchor, playing, timeline_ready)
            {
                endpoint.invalidate(reason);
                self.emit_midi_recording_endpoint_event(MidiRecordingEndpointEvent::Started {
                    stamp,
                    start: anchor,
                    success: false,
                    returned_endpoint: Some(endpoint),
                });
            } else if !endpoint.activate(anchor) {
                self.emit_midi_recording_endpoint_event(MidiRecordingEndpointEvent::Started {
                    stamp,
                    start: anchor,
                    success: false,
                    returned_endpoint: Some(endpoint),
                });
            } else {
                let route = self
                    .midi_input
                    .as_ref()
                    .expect("validated MIDI recording start retains its input route");
                let input_dropped_noncritical =
                    route.prepared.receiver.overload().dropped_noncritical();
                let input_rejected_messages =
                    route.prepared.receiver.overload().rejected_messages();
                self.midi_recording = Some(RealtimeMidiRecordingSlot {
                    endpoint,
                    start: anchor,
                    input_dropped_noncritical,
                    input_rejected_messages,
                });
                self.emit_midi_recording_endpoint_event(MidiRecordingEndpointEvent::Started {
                    stamp,
                    start: anchor,
                    success: true,
                    returned_endpoint: None,
                });
            }
        }

        let invalid_reason = self.midi_recording.as_ref().and_then(|recording| {
            self.active_midi_recording_invalid_reason(recording, anchor, playing, timeline_ready)
        });
        if let Some(reason) = invalid_reason {
            self.invalidate_active_midi_recording(reason);
        }
    }

    fn midi_recording_start_invalid_reason(
        &self,
        stamp: MidiRecordRealtimeStamp,
        anchor: MidiRecordClockAnchor,
        playing: bool,
        timeline_ready: bool,
    ) -> Option<MidiTakeInvalidReason> {
        if !playing {
            return Some(MidiTakeInvalidReason::TransportNotPlaying);
        }
        if !timeline_ready {
            return Some(MidiTakeInvalidReason::TimelineNotReady);
        }
        if let Some(reason) = self.midi_recording_identity_invalid_reason(stamp, anchor) {
            return Some(reason);
        }
        let route = self
            .midi_input
            .as_ref()
            .expect("validated MIDI recording start retains its route");
        if route.observed_transport_epoch != anchor.transport_epoch {
            return Some(MidiTakeInvalidReason::TransportEpochChanged);
        }
        if route.discard_until_empty
            || route.panic_deferred
            || route.prepared.receiver.overload().panic_required()
        {
            return Some(MidiTakeInvalidReason::InputMustPreserveOverflow);
        }
        None
    }

    fn active_midi_recording_invalid_reason(
        &self,
        recording: &RealtimeMidiRecordingSlot,
        anchor: MidiRecordClockAnchor,
        playing: bool,
        timeline_ready: bool,
    ) -> Option<MidiTakeInvalidReason> {
        if recording.endpoint.status().sealed {
            return None;
        }
        if !playing {
            return Some(MidiTakeInvalidReason::TransportNotPlaying);
        }
        if !timeline_ready {
            return Some(MidiTakeInvalidReason::TimelineNotReady);
        }
        if anchor.transport_epoch != recording.start.transport_epoch
            || anchor.loop_count != recording.start.loop_count
        {
            return Some(MidiTakeInvalidReason::TransportEpochChanged);
        }
        if let Some(reason) =
            self.midi_recording_identity_invalid_reason(recording.endpoint.stamp(), anchor)
        {
            return Some(reason);
        }
        let route = self
            .midi_input
            .as_ref()
            .expect("validated MIDI recorder retains an input route");
        let overload = route.prepared.receiver.overload();
        if overload.panic_required() {
            return Some(MidiTakeInvalidReason::InputMustPreserveOverflow);
        }
        if overload.dropped_noncritical() != recording.input_dropped_noncritical {
            return Some(MidiTakeInvalidReason::InputEventDrop);
        }
        if overload.rejected_messages() != recording.input_rejected_messages {
            return Some(MidiTakeInvalidReason::InputMessageRejected);
        }
        None
    }

    fn midi_recording_identity_invalid_reason(
        &self,
        stamp: MidiRecordRealtimeStamp,
        anchor: MidiRecordClockAnchor,
    ) -> Option<MidiTakeInvalidReason> {
        let Some(route) = self.midi_input.as_ref() else {
            return Some(MidiTakeInvalidReason::RouteIdentityChanged);
        };
        if route.route_id != stamp.route_id || route.prepared.stamp() != stamp.generator {
            return Some(MidiTakeInvalidReason::RouteIdentityChanged);
        }
        if route.prepared.connection_epoch() != stamp.connection_epoch {
            return Some(MidiTakeInvalidReason::ConnectionEpochChanged);
        }
        if self.exact_midi_generator_index(stamp.generator).is_none() {
            return Some(MidiTakeInvalidReason::RouteIdentityChanged);
        }
        if self.timeline_channel_revision != Some(stamp.timeline_revision)
            || self.timeline_channel_epoch != Some(anchor.transport_epoch)
        {
            return Some(MidiTakeInvalidReason::TimelineRevisionChanged);
        }
        if let Some(runtime) = self.timeline_runtime.as_ref()
            && runtime.active_revision() != Some(stamp.timeline_revision)
        {
            return Some(MidiTakeInvalidReason::TimelineRevisionChanged);
        }
        None
    }

    fn invalidate_active_midi_recording(&mut self, reason: MidiTakeInvalidReason) {
        if let Some(recording) = self.midi_recording.as_mut() {
            recording.endpoint.invalidate(reason);
        }
    }

    fn validate_active_midi_recording_at_callback_boundary(&mut self) {
        let invalid_reason = self.midi_recording.as_ref().and_then(|recording| {
            self.active_midi_recording_invalid_reason(
                recording,
                self.callback_transport_anchor,
                self.callback_transport_playing,
                true,
            )
        });
        if let Some(reason) = invalid_reason {
            self.invalidate_active_midi_recording(reason);
        }
    }

    fn mirror_active_midi_record_batch(&mut self, packets: &[MidiRecordPacket]) {
        let Some(recording) = self.midi_recording.as_mut() else {
            return;
        };
        let _ = recording.endpoint.mirror_batch(packets);
    }

    fn emit_midi_recording_endpoint_event(&mut self, event: MidiRecordingEndpointEvent) {
        debug_assert!(self.midi_recording_event_reservations > 0);
        self.midi_recording_event_reservations =
            self.midi_recording_event_reservations.saturating_sub(1);
        let Some(events) = self.midi_recording_endpoint_events.as_mut() else {
            forget_midi_recording_event_endpoint(event);
            return;
        };
        if let Err(PushError::Full(event)) = events.push(event) {
            // Every lifecycle command reserves its receipt before being popped. Leaking is safer
            // than callback-side destruction if that invariant is ever violated.
            forget_midi_recording_event_endpoint(event);
        }
    }

    /// Drains the dedicated OS-input SPSC before one bounded render segment and stages an
    /// all-or-nothing Live-lane batch against the exact Generator identity.
    fn service_midi_input(
        &mut self,
        chunk_device_frame: u64,
        chunk_timeline_frame: u64,
        frames: usize,
        transport_epoch: u64,
    ) -> Option<PausedMidiMonitorRoute> {
        let mut route = self.midi_input.take()?;
        let stamp = route.prepared.stamp();
        let connection_epoch = route.prepared.connection_epoch();

        if route.observed_transport_epoch != transport_epoch {
            route.observed_transport_epoch = transport_epoch;
            route.reset_timing();
            route.discard_until_empty = true;
            route.panic_deferred = true;
        }
        if route.prepared.receiver.overload().take_panic_required() {
            self.invalidate_active_midi_recording(MidiTakeInvalidReason::InputMustPreserveOverflow);
            route.reset_timing();
            route.discard_until_empty = true;
            route.panic_deferred = true;
        }

        let Some(endpoint_index) = self.exact_midi_generator_index(stamp) else {
            self.invalidate_active_midi_recording(MidiTakeInvalidReason::RouteIdentityChanged);
            route.reset_timing();
            route.discard_until_empty = true;
            route.panic_deferred = true;
            for _ in 0..MIDI_INPUT_MAX_DRAIN_PER_CHUNK {
                if route.prepared.receiver.try_pop().is_none() {
                    route.discard_until_empty = false;
                    break;
                }
            }
            self.midi_input = Some(route);
            return None;
        };

        if route.panic_deferred {
            self.panic_exact_midi_destination(stamp);
            route.panic_deferred = false;
        }

        if route.discard_until_empty {
            for _ in 0..MIDI_INPUT_MAX_DRAIN_PER_CHUNK {
                if route.prepared.receiver.try_pop().is_none() {
                    route.discard_until_empty = false;
                    break;
                }
            }
            self.midi_input = Some(route);
            let mixer_track = self.effective_generator_mixer_track(endpoint_index);
            return Some(PausedMidiMonitorRoute {
                generator_index: endpoint_index,
                mixer_track,
            });
        }

        let end_device_frame = chunk_device_frame.saturating_add(frames as u64);
        let window = MidiScheduleWindow {
            start_frame: chunk_device_frame,
            end_frame: end_device_frame,
        };
        let sample_rate_hz = self.sample_rate.round().clamp(1.0, u32::MAX as f32) as u32;
        let mut popped = 0_usize;
        while popped < MIDI_INPUT_MAX_DRAIN_PER_CHUNK {
            let event = if let Some(event) = route.pending_future.take() {
                event
            } else {
                let Some(event) = route.prepared.receiver.try_pop() else {
                    break;
                };
                popped += 1;
                event
            };
            if event.connection_epoch != connection_epoch {
                self.invalidate_active_midi_recording(
                    MidiTakeInvalidReason::ConnectionEpochChanged,
                );
                route.reset_timing();
                route.discard_until_empty = true;
                route.panic_deferred = true;
                break;
            }
            if route.mapper.is_none() {
                let anchor = MidiClockAnchor::new(
                    connection_epoch,
                    event.timestamp_us,
                    chunk_device_frame,
                    sample_rate_hz,
                )
                .expect("the callback sample rate is nonzero");
                route.mapper = Some(MidiTimestampMapper::new(anchor));
            }
            let mapping = route
                .mapper
                .as_mut()
                .expect("MIDI mapper was initialized")
                .map_event(event, window);
            let Ok(mapping) = mapping else {
                self.invalidate_active_midi_recording(MidiTakeInvalidReason::InvalidPacket);
                route.reset_timing();
                route.discard_until_empty = true;
                route.panic_deferred = true;
                break;
            };
            if mapping.timestamp_regressed {
                self.invalidate_active_midi_recording(MidiTakeInvalidReason::TimestampRegression);
            }
            let mut effective = event;
            effective.timestamp_us = mapping.effective_timestamp_us;
            if mapping.decision == MidiScheduleDecision::FutureRetained {
                route.pending_future = Some(effective);
                break;
            }
            match route.scratch.push(effective) {
                MidiScratchPush::Stored => {}
                MidiScratchPush::Coalesced | MidiScratchPush::Dropped => {
                    self.invalidate_active_midi_recording(MidiTakeInvalidReason::InputEventDrop);
                }
                MidiScratchPush::PanicRequired => {
                    self.invalidate_active_midi_recording(
                        MidiTakeInvalidReason::LiveScratchOverflow,
                    );
                    route.scratch.clear();
                    route.pending_future = None;
                    route.discard_until_empty = true;
                    route.panic_deferred = true;
                    break;
                }
            }
        }

        if route.panic_deferred {
            self.panic_exact_midi_destination(stamp);
            route.panic_deferred = false;
            route.scratch.clear();
            self.midi_input = Some(route);
            let mixer_track = self.effective_generator_mixer_track(endpoint_index);
            return Some(PausedMidiMonitorRoute {
                generator_index: endpoint_index,
                mixer_track,
            });
        }

        let mut events = [EMPTY_ENDPOINT_FRAME_EVENT; 16];
        let mut classes = [EndpointEventClass::Live; 16];
        let mut recording_packets = [MidiRecordPacket::default(); 16];
        let mut recording_packet_count = 0_usize;
        let mut event_count = 0_usize;
        let slot = Some(stamp.slot.unwrap_or(0) as u8);
        let anchor = route.mapper.map(|mapper| mapper.anchor());
        for event in route.scratch.events().iter().copied() {
            let target = anchor
                .expect("nonempty MIDI scratch has a mapper")
                .device_frame_at(event.timestamp_us)
                .max(chunk_device_frame);
            let offset = target
                .saturating_sub(chunk_device_frame)
                .min(u16::MAX as u64) as u16;
            events[event_count] = FrameEvent::midi(offset, slot, event.data);
            classes[event_count] = EndpointEventClass::Live;
            if self.callback_transport_playing && (event.is_note_on() || event.is_note_off()) {
                recording_packets[recording_packet_count] = MidiRecordPacket {
                    session_id: self
                        .midi_recording
                        .as_ref()
                        .map_or(0, |recording| recording.endpoint.stamp().session_id),
                    route_id: route.route_id,
                    connection_epoch,
                    transport_epoch,
                    scheduled_device_frame: target,
                    timeline_frame: chunk_timeline_frame
                        .saturating_add(target.saturating_sub(chunk_device_frame)),
                    event,
                };
                recording_packet_count += 1;
            }
            event_count += 1;
        }
        route.scratch.clear();

        if event_count == 0 {
            // No new edge is still a valid route: paused monitoring must keep rendering held
            // notes and let later NoteOff/CC123 traverse the same path.
        } else {
            let endpoint = &mut self.generator_endpoints[endpoint_index]
                .as_mut()
                .expect("exact MIDI endpoint remains installed")
                .endpoint;
            if endpoint.can_stage_batch(&events[..event_count], &classes[..event_count]) {
                endpoint.stage_preflighted_batch(&events[..event_count], &classes[..event_count]);
                if recording_packet_count != 0 {
                    self.mirror_active_midi_record_batch(
                        &recording_packets[..recording_packet_count],
                    );
                }
            } else {
                self.invalidate_active_midi_recording(MidiTakeInvalidReason::LiveBatchRejected);
                self.panic_exact_midi_destination(stamp);
                self.fixed_quantum_event_overflows =
                    self.fixed_quantum_event_overflows.saturating_add(1);
                route.discard_until_empty = true;
            }
        }
        self.midi_input = Some(route);
        let mixer_track = self.effective_generator_mixer_track(endpoint_index);
        Some(PausedMidiMonitorRoute {
            generator_index: endpoint_index,
            mixer_track,
        })
    }

    fn set_device_frame(&mut self, device_frame: u64) {
        self.device_frame = device_frame;
    }

    fn install_master_capture(&mut self, capture_id: u64, endpoint: MasterCaptureEndpoint) {
        let success =
            capture_id != 0 && endpoint.session_id() == capture_id && self.master_capture.is_none();
        let returned_endpoint = if success {
            self.master_capture = Some(MasterCaptureSlot {
                capture_id,
                endpoint,
            });
            None
        } else {
            Some(endpoint)
        };
        self.emit_master_capture_event(MasterCaptureEndpointEvent::Installed {
            capture_id,
            start_device_frame: self.device_frame,
            success,
            returned_endpoint,
        });
    }

    fn stop_master_capture(&mut self, capture_id: u64) {
        let returned_endpoint = if self
            .master_capture
            .as_ref()
            .is_some_and(|capture| capture.capture_id == capture_id)
        {
            self.master_capture.take().map(|capture| capture.endpoint)
        } else {
            None
        };
        self.emit_master_capture_event(MasterCaptureEndpointEvent::Stopped {
            capture_id,
            end_device_frame: self.device_frame,
            returned_endpoint,
        });
    }

    fn clear_master_capture(&mut self, request_id: u64) {
        let capture = self.master_capture.take();
        let capture_id = capture.as_ref().map(|capture| capture.capture_id);
        self.emit_master_capture_event(MasterCaptureEndpointEvent::Cleared {
            request_id,
            capture_id,
            end_device_frame: self.device_frame,
            returned_endpoint: capture.map(|capture| capture.endpoint),
        });
    }

    fn capture_rendered_master(&mut self, first_device_frame: u64, frames: usize) {
        let frames = frames.min(MAX_MIXER_BLOCK_FRAMES);
        if let Some(capture) = self.master_capture.as_mut() {
            let _ = capture
                .endpoint
                .push_block(first_device_frame, &self.master_block[..frames]);
        }
        self.device_frame = first_device_frame.saturating_add(frames as u64);
    }

    fn emit_master_capture_event(&mut self, event: MasterCaptureEndpointEvent) {
        let Some(events) = self.master_capture_events.as_mut() else {
            forget_master_capture_event_endpoint(event);
            return;
        };
        if let Err(PushError::Full(event)) = events.push(event) {
            // Commands reserve an event slot before being popped. If that
            // invariant is broken, leaking is safer than callback-side drop.
            forget_master_capture_event_endpoint(event);
        }
    }

    fn asset_retirements_required(&self, command: &AudioCommand) -> usize {
        match command {
            AudioCommand::RegisterAsset {
                id,
                samples,
                sample_rate,
                channels,
                ..
            } => usize::from(
                !asset_layout_is_valid(samples, *sample_rate, *channels)
                    || self.find_asset_slot(*id).is_some()
                    || self.audio_assets.iter().all(|slot| slot.samples.is_some()),
            ),
            AudioCommand::UnregisterAsset { id, .. } => {
                usize::from(self.find_asset_slot(*id).is_some())
            }
            AudioCommand::ClearAssets { .. } => self
                .audio_assets
                .iter()
                .filter(|slot| slot.samples.is_some())
                .count(),
            _ => 0,
        }
    }

    fn find_asset_slot(&self, asset_id: u64) -> Option<usize> {
        self.audio_assets
            .iter()
            .position(|slot| slot.samples.is_some() && slot.id == asset_id)
    }

    fn register_asset(
        &mut self,
        id: u64,
        samples: Arc<[f32]>,
        sample_rate: u32,
        channels: u16,
        retired_assets: &mut Producer<Arc<[f32]>>,
    ) -> bool {
        if !asset_layout_is_valid(&samples, sample_rate, channels) {
            retire_asset(samples, retired_assets);
            return false;
        }
        let frames = samples.len() / usize::from(channels);
        if let Some(index) = self.find_asset_slot(id) {
            let slot = &mut self.audio_assets[index];
            let old_samples = slot.samples.replace(samples);
            slot.sample_rate = sample_rate;
            slot.channels = channels;
            slot.frames = frames;
            if let Some(old_samples) = old_samples {
                retire_asset(old_samples, retired_assets);
            }
            return true;
        }
        if let Some(slot) = self
            .audio_assets
            .iter_mut()
            .find(|slot| slot.samples.is_none())
        {
            *slot = AudioAssetSlot {
                id,
                samples: Some(samples),
                sample_rate,
                channels,
                frames,
            };
        } else {
            retire_asset(samples, retired_assets);
            return false;
        }
        true
    }

    fn unregister_asset(&mut self, id: u64, retired_assets: &mut Producer<Arc<[f32]>>) -> bool {
        let Some(index) = self.find_asset_slot(id) else {
            return false;
        };
        for voice in &mut self.audio_voices {
            if voice.active && voice.asset_slot == index {
                voice.active = false;
            }
        }
        let samples = self.audio_assets[index].samples.take();
        self.audio_assets[index] = AudioAssetSlot::default();
        if let Some(samples) = samples {
            retire_asset(samples, retired_assets);
        }
        true
    }

    fn clear_assets(&mut self, retired_assets: &mut Producer<Arc<[f32]>>) -> usize {
        for voice in &mut self.audio_voices {
            voice.active = false;
        }
        let mut removed = 0;
        for slot in &mut self.audio_assets {
            if let Some(samples) = slot.samples.take() {
                retire_asset(samples, retired_assets);
                removed += 1;
            }
            *slot = AudioAssetSlot::default();
        }
        removed
    }

    #[allow(clippy::too_many_arguments)]
    fn play_audio_clip(
        &mut self,
        clip_id: u64,
        asset_id: u64,
        source_frame: f64,
        gain: f32,
        mixer_track: usize,
        sync: bool,
    ) {
        let Some(asset_slot) = self.find_asset_slot(asset_id) else {
            for voice in &mut self.audio_voices {
                if voice.active && voice.clip_id == clip_id {
                    voice.active = false;
                }
            }
            return;
        };
        let asset = &self.audio_assets[asset_slot];
        let expected_position = if source_frame.is_finite() {
            source_frame.clamp(0.0, asset.frames as f64)
        } else {
            0.0
        };
        let gain = if gain.is_finite() {
            gain.clamp(0.0, 4.0)
        } else {
            0.0
        };
        let mixer_track = mixer_track.min(TRACK_COUNT - 1);

        if let Some(index) = self
            .audio_voices
            .iter()
            .position(|voice| voice.active && voice.clip_id == clip_id)
        {
            let voice = &mut self.audio_voices[index];
            let same_asset = voice.asset_slot == asset_slot;
            voice.gain = gain;
            voice.mixer_track = mixer_track;
            voice.timeline_asset_id = None;
            voice.timeline_start_frame = 0;
            voice.timeline_clip_end_frame = 0;
            voice.timeline_stop_frame = 0;
            voice.timeline_frame = 0;
            voice.fades = CompiledClipFades::default();
            voice.timeline_source_root_frame = 0;
            voice.timeline_source_elapsed_frames = 0;
            if !sync || !same_asset {
                voice.asset_slot = asset_slot;
                voice.source_position = expected_position;
            } else {
                let source_frames_per_output =
                    f64::from(asset.sample_rate) / f64::from(self.sample_rate.max(1.0));
                let drift_threshold = SYNC_DRIFT_OUTPUT_FRAMES * source_frames_per_output;
                if (voice.source_position - expected_position).abs() > drift_threshold {
                    voice.source_position = expected_position;
                }
            }
            voice.active = voice.source_position < asset.frames as f64;
            return;
        }

        let voice_index = self
            .audio_voices
            .iter()
            .position(|voice| !voice.active)
            .unwrap_or(self.next_audio_voice);
        self.audio_voices[voice_index] = AudioClipVoice {
            active: expected_position < asset.frames as f64,
            clip_id,
            asset_slot,
            source_position: expected_position,
            gain,
            mixer_track,
            timeline_asset_id: None,
            timeline_start_frame: 0,
            timeline_clip_end_frame: 0,
            timeline_stop_frame: 0,
            timeline_frame: 0,
            fades: CompiledClipFades::default(),
            timeline_source_root_frame: 0,
            timeline_source_elapsed_frames: 0,
        };
        self.next_audio_voice = (voice_index + 1) % MAX_AUDIO_CLIP_VOICES;
    }

    fn play_timeline_audio_clip(&mut self, clip: ChasedAudioClip, timeline_frame: u64) {
        let descriptor = clip.descriptor;
        let Some(asset_slot) = self.find_asset_slot(descriptor.asset_id) else {
            for voice in &mut self.audio_voices {
                if voice.active
                    && voice.clip_id == u64::from(descriptor.clip_id)
                    && voice.timeline_asset_id == Some(descriptor.asset_id)
                {
                    voice.active = false;
                }
            }
            self.timeline_missing_assets = self.timeline_missing_assets.saturating_add(1);
            return;
        };
        let asset_frames = self.audio_assets[asset_slot].frames as f64;
        let source_position = if clip.source_position_frame.is_finite() {
            clip.source_position_frame.clamp(0.0, asset_frames)
        } else {
            asset_frames
        };
        let voice_index = self
            .audio_voices
            .iter()
            .position(|voice| {
                voice.active
                    && voice.clip_id == u64::from(descriptor.clip_id)
                    && voice.timeline_asset_id == Some(descriptor.asset_id)
            })
            .or_else(|| self.audio_voices.iter().position(|voice| !voice.active))
            .unwrap_or(self.next_audio_voice);
        self.audio_voices[voice_index] = AudioClipVoice {
            active: source_position < asset_frames && timeline_frame < descriptor.stop_frame,
            clip_id: u64::from(descriptor.clip_id),
            asset_slot,
            source_position,
            gain: descriptor.gain.clamp(0.0, 4.0),
            mixer_track: usize::from(descriptor.mixer_track).min(TRACK_COUNT - 1),
            timeline_asset_id: Some(descriptor.asset_id),
            timeline_start_frame: descriptor.start_frame,
            timeline_clip_end_frame: descriptor.clip_end_frame,
            timeline_stop_frame: descriptor.stop_frame,
            timeline_frame,
            fades: descriptor.fades,
            timeline_source_root_frame: descriptor.source_offset_frame,
            timeline_source_elapsed_frames: descriptor.source_elapsed_frames,
        };
        self.next_audio_voice = (voice_index + 1) % MAX_AUDIO_CLIP_VOICES;
    }

    fn stop_timeline_audio_clip(&mut self, clip: ChasedAudioClip) {
        for voice in &mut self.audio_voices {
            if voice.active
                && voice.clip_id == u64::from(clip.descriptor.clip_id)
                && voice.timeline_asset_id == Some(clip.descriptor.asset_id)
            {
                voice.active = false;
            }
        }
    }

    fn start_timeline_native_note(&mut self, note: ChasedNote) {
        let frequency = 440.0 * 2.0_f32.powf((note.note as f32 - 69.0) / 12.0);
        self.voices[self.next_voice] = Voice {
            phase: 0.0,
            phase_step: frequency / self.sample_rate,
            envelope: (note.velocity * note.gain).clamp(0.0, 1.0) * 0.28,
            decay: 0.99984,
            active: true,
            mixer_track: usize::from(note.mixer_track).min(TRACK_COUNT - 1),
            timeline_note_id: Some(note.note_id),
            timeline_channel_id: Some(note.channel_id),
        };
        self.next_voice = (self.next_voice + 1) % MAX_VOICES;
    }

    fn stop_timeline_native_note(&mut self, note_id: u64) {
        if let Some(voice) = self
            .voices
            .iter_mut()
            .find(|voice| voice.active && voice.timeline_note_id == Some(note_id))
        {
            voice.active = false;
            voice.timeline_note_id = None;
            voice.timeline_channel_id = None;
        }
    }

    fn apply_timeline_events_for_frame(&mut self, frame_index: usize) {
        if !self.timeline_plan_render_active {
            return;
        }
        let timeline_frame = self
            .timeline_render_start_frame
            .saturating_add(frame_index as u64);
        while let Some(event) = self.timeline_plan.next_at(frame_index) {
            match event {
                TimelinePlannedEventKind::NoteOn(note) => {
                    self.start_timeline_native_note(note);
                }
                TimelinePlannedEventKind::NoteOff(note) => {
                    self.stop_timeline_native_note(note.note_id);
                }
                TimelinePlannedEventKind::GeneratorNoteOn
                | TimelinePlannedEventKind::GeneratorNoteOff => {}
                TimelinePlannedEventKind::AudioStart(clip) => {
                    self.play_timeline_audio_clip(clip, timeline_frame);
                }
                TimelinePlannedEventKind::AudioStop(clip) => {
                    self.stop_timeline_audio_clip(clip);
                }
            }
        }
    }

    fn timeline_voice_fade(voice: &AudioClipVoice) -> f32 {
        let Some(_) = voice.timeline_asset_id else {
            return 1.0;
        };
        if voice.timeline_frame >= voice.timeline_stop_frame
            || voice.timeline_frame >= voice.timeline_clip_end_frame
        {
            return 0.0;
        }
        voice.fades.gain_at(voice.timeline_frame)
    }

    #[cfg(test)]
    fn next_audio_mix(&mut self, any_solo: bool) -> (f32, f32) {
        let assets = &self.audio_assets;
        let track_gains = &self.track_gains;
        let track_pans = &self.track_pans;
        let track_muted = &self.track_muted;
        let track_solo = &self.track_solo;
        let output_sample_rate = f64::from(self.sample_rate.max(1.0));
        let mut mixed_left = 0.0;
        let mut mixed_right = 0.0;

        for voice in &mut self.audio_voices {
            if !voice.active {
                continue;
            }
            let Some(asset) = assets.get(voice.asset_slot) else {
                voice.active = false;
                continue;
            };
            let Some(samples) = asset.samples.as_deref() else {
                voice.active = false;
                continue;
            };
            if asset.frames == 0 || voice.source_position >= asset.frames as f64 {
                voice.active = false;
                continue;
            }

            let left_frame = voice.source_position.floor() as usize;
            let right_frame = (left_frame + 1).min(asset.frames - 1);
            let fraction = (voice.source_position - left_frame as f64) as f32;
            let channel_count = usize::from(asset.channels);
            let left_a = safe_asset_sample(samples[left_frame * channel_count]);
            let left_b = safe_asset_sample(samples[right_frame * channel_count]);
            let source_left = left_a + (left_b - left_a) * fraction;
            let source_right = if channel_count == 1 {
                source_left
            } else {
                let right_a = safe_asset_sample(samples[left_frame * channel_count + 1]);
                let right_b = safe_asset_sample(samples[right_frame * channel_count + 1]);
                right_a + (right_b - right_a) * fraction
            };

            let track = voice.mixer_track.min(TRACK_COUNT - 1);
            let track_gain = if track_muted[track] || (any_solo && !track_solo[track]) {
                0.0
            } else {
                track_gains[track]
            };
            let gain = voice.gain * track_gain;
            let pan = track_pans[track];
            mixed_left += source_left * gain * if pan > 0.0 { 1.0 - pan } else { 1.0 };
            mixed_right += source_right * gain * if pan < 0.0 { 1.0 + pan } else { 1.0 };

            voice.source_position += f64::from(asset.sample_rate) / output_sample_rate;
            if voice.source_position >= asset.frames as f64 {
                voice.active = false;
            }
        }
        (mixed_left, mixed_right)
    }

    fn render_synth_frame(&mut self, frame_index: usize) {
        let channel_bases = &self.timeline_channel_bases;
        let automation_values = self
            .timeline_plan_render_active
            .then_some(&self.timeline_automation_values);
        for voice in &mut self.voices {
            if !voice.active {
                continue;
            }
            let fundamental = (voice.phase * TAU).sin();
            let harmonic = (voice.phase * TAU * 2.0).sin() * 0.18;
            let (track, volume, pan, audible) = voice
                .timeline_channel_id
                .and_then(|channel_id| channel_bases.get(channel_id))
                .map_or(
                    (voice.mixer_track.min(TRACK_COUNT - 1), 1.0, 0.0, true),
                    |base| {
                        let volume = base
                            .volume_automation_slot
                            .and_then(|slot| {
                                automation_values.and_then(|values| {
                                    values.value_at_slot(
                                        slot,
                                        CompiledAutomationTarget::ChannelVolume {
                                            channel_id: base.channel_id,
                                        },
                                        frame_index,
                                    )
                                })
                            })
                            .map_or(base.volume, |value| value.clamp(0.0, 1.0));
                        let pan = base
                            .pan_automation_slot
                            .and_then(|slot| {
                                automation_values.and_then(|values| {
                                    values.value_at_slot(
                                        slot,
                                        CompiledAutomationTarget::ChannelPan {
                                            channel_id: base.channel_id,
                                        },
                                        frame_index,
                                    )
                                })
                            })
                            .map_or(base.pan, |value| value.clamp(-1.0, 1.0));
                        let muted = base
                            .mute_automation_slot
                            .and_then(|slot| {
                                automation_values.and_then(|values| {
                                    values.value_at_slot(
                                        slot,
                                        CompiledAutomationTarget::ChannelMute {
                                            channel_id: base.channel_id,
                                        },
                                        frame_index,
                                    )
                                })
                            })
                            .map_or(base.muted, |value| value >= 0.5);
                        (
                            base.mixer_track,
                            volume,
                            pan,
                            !muted && (!channel_bases.any_solo || base.solo),
                        )
                    },
                );
            let sample = (fundamental + harmonic) * voice.envelope * volume;
            let bus = &mut self.track_block[track * MAX_MIXER_BLOCK_FRAMES + frame_index];
            if audible {
                bus[0] += sample * if pan > 0.0 { 1.0 - pan } else { 1.0 };
                bus[1] += sample * if pan < 0.0 { 1.0 + pan } else { 1.0 };
            }
            voice.phase = (voice.phase + voice.phase_step).fract();
            voice.envelope *= voice.decay;
            if voice.envelope < 0.0001 {
                voice.active = false;
            }
        }
    }

    fn render_audio_frame(&mut self, frame_index: usize) {
        let output_sample_rate = f64::from(self.sample_rate);
        let assets = &self.audio_assets;
        let voices = &mut self.audio_voices;
        let track_block = &mut self.track_block;
        for voice in voices {
            if !voice.active {
                continue;
            }
            if voice.timeline_asset_id.is_some()
                && voice.timeline_frame >= voice.timeline_stop_frame
            {
                voice.active = false;
                continue;
            }
            let Some(asset) = assets.get(voice.asset_slot) else {
                voice.active = false;
                continue;
            };
            let Some(samples) = asset.samples.as_deref() else {
                voice.active = false;
                continue;
            };
            if asset.frames == 0
                || !voice.source_position.is_finite()
                || voice.source_position < 0.0
                || voice.source_position >= asset.frames as f64
            {
                voice.active = false;
                continue;
            }

            let left_frame = voice.source_position.floor() as usize;
            let right_frame = (left_frame + 1).min(asset.frames - 1);
            let fraction = (voice.source_position - left_frame as f64) as f32;
            let channel_count = usize::from(asset.channels);
            let left_a = safe_asset_sample(samples[left_frame * channel_count]);
            let left_b = safe_asset_sample(samples[right_frame * channel_count]);
            let source_left = left_a + (left_b - left_a) * fraction;
            let source_right = if channel_count == 1 {
                source_left
            } else {
                let right_a = safe_asset_sample(samples[left_frame * channel_count + 1]);
                let right_b = safe_asset_sample(samples[right_frame * channel_count + 1]);
                right_a + (right_b - right_a) * fraction
            };

            let track = voice.mixer_track.min(TRACK_COUNT - 1);
            let bus = &mut track_block[track * MAX_MIXER_BLOCK_FRAMES + frame_index];
            let gain = voice.gain * Self::timeline_voice_fade(voice);
            bus[0] += source_left * gain;
            bus[1] += source_right * gain;

            if voice.timeline_asset_id.is_some() {
                voice.timeline_frame = voice.timeline_frame.saturating_add(1);
                let elapsed = i128::from(voice.timeline_source_elapsed_frames)
                    + i128::from(
                        voice
                            .timeline_frame
                            .saturating_sub(voice.timeline_start_frame),
                    );
                // Derive from the preserved root clock, never cumulative float
                // increments: split points, seeks and callback partitions agree.
                voice.source_position = voice.timeline_source_root_frame as f64
                    + elapsed as f64 * f64::from(asset.sample_rate) / output_sample_rate;
            } else {
                voice.source_position += f64::from(asset.sample_rate) / output_sample_rate;
            }
            if voice.source_position >= asset.frames as f64
                || (voice.timeline_asset_id.is_some()
                    && voice.timeline_frame >= voice.timeline_stop_frame)
            {
                voice.active = false;
            }
        }
    }

    fn render_metronome_frame(&mut self, status: &AudioStatus, frame_index: usize) {
        let transport_playing = status.playing.load(Ordering::Acquire);
        if transport_playing {
            let tempo = status.tempo_milli.load(Ordering::Relaxed) as f32 / 1000.0;
            self.beat_phase += tempo / 60.0 / self.sample_rate;
            if self.beat_phase >= 1.0 {
                self.beat_phase -= 1.0;
                self.click_envelope = 0.1;
                self.click_phase = 0.0;
            }
        }
        if self.click_envelope > 0.0001 {
            let click = (self.click_phase * TAU).sin() * self.click_envelope;
            // Mixer track zero is the direct-master source path. Keeping the
            // metronome here lets the same source-level PDC tap align it with
            // every insert and generator path before the master endpoint.
            self.track_block[frame_index][0] += click;
            self.track_block[frame_index][1] += click;
            self.click_phase = (self.click_phase + 1100.0 / self.sample_rate).fract();
            self.click_envelope *= 0.992;
        }
    }

    fn process_generator_endpoints(&mut self, frames: usize) -> bool {
        self.process_generator_endpoints_impl(frames, None, false)
    }

    fn process_mixer_graph_generator_endpoints(
        &mut self,
        frames: usize,
        graph_nodes: &[bool; TRACK_COUNT],
    ) -> bool {
        self.process_generator_endpoints_impl(frames, Some(graph_nodes), true)
    }

    fn process_generator_endpoints_impl(
        &mut self,
        frames: usize,
        graph_nodes: Option<&[bool; TRACK_COUNT]>,
        strict: bool,
    ) -> bool {
        if frames == 0 {
            return true;
        }

        let transport_epoch = self.transport_epoch;
        let channel_bases = &self.timeline_channel_bases;
        let mut latency_drifted = false;
        let mut completed_marker_endpoints = 0_usize;
        for index in 0..MAX_GENERATOR_ENDPOINTS {
            let Some(slot) = self.generator_endpoints[index].as_mut() else {
                continue;
            };
            if graph_nodes
                .is_some_and(|nodes| slot.mixer_track >= TRACK_COUNT || !nodes[slot.mixer_track])
            {
                continue;
            }
            let (mixer_track, channel_volume, channel_pan, channel_audible) = channel_bases
                .get(slot.channel_id)
                .map_or((slot.mixer_track, 1.0, 0.0, true), |base| {
                    (
                        base.mixer_track,
                        base.volume,
                        base.pan,
                        channel_bases.is_audible(base),
                    )
                });
            let had_admitted_marker = slot.endpoint.has_admitted_live_edit_marker();
            let before = slot.endpoint.stats();
            let status = slot.endpoint.process_generator(
                transport_epoch,
                frames,
                &mut self.plugin_output_left[..frames],
                &mut self.plugin_output_right[..frames],
            );
            let diagnostics = fixed_quantum_stats_delta(before, slot.endpoint.stats());
            self.fixed_quantum_event_overflows = self
                .fixed_quantum_event_overflows
                .saturating_add(diagnostics.event_overflows);
            self.fixed_quantum_invalid_events = self
                .fixed_quantum_invalid_events
                .saturating_add(diagnostics.invalid_events);
            self.fixed_quantum_endpoint_event_rejections = self
                .fixed_quantum_endpoint_event_rejections
                .saturating_add(diagnostics.endpoint_event_rejections);
            self.fixed_quantum_bridge_gaps = self
                .fixed_quantum_bridge_gaps
                .saturating_add(diagnostics.bridge_gaps);
            self.fixed_quantum_output_underflow_frames = self
                .fixed_quantum_output_underflow_frames
                .saturating_add(diagnostics.output_underflow_frames);
            latency_drifted |= diagnostics.latency_drift_quanta != 0;
            if had_admitted_marker && !slot.endpoint.has_admitted_live_edit_marker() {
                completed_marker_endpoints += 1;
            }
            if diagnostics.event_overflows != 0
                || diagnostics.endpoint_event_rejections != 0
                || diagnostics.invalid_events != 0
                || diagnostics.latency_drift_quanta != 0
            {
                let deferred = slot.endpoint.clear_and_stage_all_notes_off();
                slot.suppress_output_frames = slot
                    .suppress_output_frames
                    .max(fixed_quantum_fail_closed_frames(&slot.endpoint).saturating_add(deferred));
            }
            let has_exact_output = matches!(
                status,
                FixedQuantumProcessStatus::Processed {
                    frames: processed_frames,
                    ..
                } if processed_frames == frames
            );
            let suppressed = slot.suppress_output_frames != 0;
            slot.suppress_output_frames = slot.suppress_output_frames.saturating_sub(frames);

            if strict
                && (!has_exact_output
                    || suppressed
                    || diagnostics.event_overflows != 0
                    || diagnostics.endpoint_event_rejections != 0
                    || diagnostics.invalid_events != 0
                    || diagnostics.latency_drift_quanta != 0)
            {
                return false;
            }

            let start = mixer_track * MAX_MIXER_BLOCK_FRAMES;
            for frame_index in 0..frames {
                let source = if has_exact_output && !suppressed && channel_audible {
                    let left = safe_plugin_sample(self.plugin_output_left[frame_index])
                        * channel_volume
                        * if channel_pan > 0.0 {
                            1.0 - channel_pan
                        } else {
                            1.0
                        };
                    let right = safe_plugin_sample(self.plugin_output_right[frame_index])
                        * channel_volume
                        * if channel_pan < 0.0 {
                            1.0 + channel_pan
                        } else {
                            1.0
                        };
                    [left, right]
                } else {
                    [0.0; 2]
                };
                let delayed = slot.pdc_delay.process_sample(source);
                if mixer_track < TRACK_COUNT {
                    let bus = &mut self.track_block[start + frame_index];
                    bus[0] += delayed[0];
                    bus[1] += delayed[1];
                }
            }
        }
        self.admitted_live_edit_endpoint_count = self
            .admitted_live_edit_endpoint_count
            .saturating_sub(completed_marker_endpoints);
        if latency_drifted {
            if let Some(runtime) = self.timeline_runtime.as_mut() {
                runtime.require_resync();
            }
            self.fail_timeline_block();
            return false;
        }
        true
    }

    /// Audible external-MIDI monitor while transport is paused. Only the exact routed Generator,
    /// its routed track insert and the master insert advance. Timeline/native/audio voices and
    /// every unrelated Generator/PDC path remain frozen.
    fn render_paused_midi_monitor(
        &mut self,
        status: &AudioStatus,
        frames: usize,
        monitor: PausedMidiMonitorRoute,
    ) -> PausedEndpointProgress {
        if self.mixer_graph_was_activated {
            return self.render_paused_midi_monitor_graph(status, frames, monitor);
        }
        let mut progress = PausedEndpointProgress::default();
        for track in 0..TRACK_COUNT {
            let start = track * MAX_MIXER_BLOCK_FRAMES;
            self.track_block[start..start + frames].fill([0.0; 2]);
        }
        self.master_block[..frames].fill([0.0; 2]);

        let Some(slot) = self.generator_endpoints[monitor.generator_index].as_mut() else {
            return progress;
        };
        let (mixer_track, channel_volume, channel_pan, channel_audible) = self
            .timeline_channel_bases
            .get(slot.channel_id)
            .map_or((slot.mixer_track, 1.0, 0.0, true), |base| {
                (
                    base.mixer_track,
                    base.volume,
                    base.pan,
                    self.timeline_channel_bases.is_audible(base),
                )
            });
        debug_assert_eq!(mixer_track, monitor.mixer_track);
        let had_admitted_marker = slot.endpoint.has_admitted_live_edit_marker();
        let before = slot.endpoint.stats();
        let process_status = slot.endpoint.process_generator(
            self.transport_epoch,
            frames,
            &mut self.plugin_output_left[..frames],
            &mut self.plugin_output_right[..frames],
        );
        progress.generator_mask |= 1_u64 << monitor.generator_index;
        let diagnostics = fixed_quantum_stats_delta(before, slot.endpoint.stats());
        if had_admitted_marker && !slot.endpoint.has_admitted_live_edit_marker() {
            self.admitted_live_edit_endpoint_count =
                self.admitted_live_edit_endpoint_count.saturating_sub(1);
        }
        if diagnostics.event_overflows != 0
            || diagnostics.endpoint_event_rejections != 0
            || diagnostics.invalid_events != 0
            || diagnostics.latency_drift_quanta != 0
        {
            let deferred = slot.endpoint.clear_and_stage_all_notes_off();
            slot.suppress_output_frames = slot
                .suppress_output_frames
                .max(fixed_quantum_fail_closed_frames(&slot.endpoint).saturating_add(deferred));
        }
        let exact_output = matches!(
            process_status,
            FixedQuantumProcessStatus::Processed {
                frames: processed_frames,
                ..
            } if processed_frames == frames
        );
        let suppressed = slot.suppress_output_frames != 0;
        slot.suppress_output_frames = slot.suppress_output_frames.saturating_sub(frames);
        let start = mixer_track * MAX_MIXER_BLOCK_FRAMES;
        for frame_index in 0..frames {
            let source = if exact_output && !suppressed && channel_audible {
                [
                    safe_plugin_sample(self.plugin_output_left[frame_index])
                        * channel_volume
                        * if channel_pan > 0.0 {
                            1.0 - channel_pan
                        } else {
                            1.0
                        },
                    safe_plugin_sample(self.plugin_output_right[frame_index])
                        * channel_volume
                        * if channel_pan < 0.0 {
                            1.0 + channel_pan
                        } else {
                            1.0
                        },
                ]
            } else {
                [0.0; 2]
            };
            let delayed = slot.pdc_delay.process_sample(source);
            self.track_block[start + frame_index] = delayed;
        }
        self.accumulate_fixed_quantum_diagnostics(diagnostics);
        if diagnostics.latency_drift_quanta != 0 {
            if let Some(runtime) = self.timeline_runtime.as_mut() {
                runtime.require_resync();
            }
            self.fail_timeline_block();
            self.master_block[..frames].fill([0.0; 2]);
            return progress;
        }

        if mixer_track != 0 {
            let has_insert = self.insert_endpoints[mixer_track].is_some();
            let succeeded = self.process_insert_endpoint(mixer_track, frames);
            if has_insert {
                progress.insert_mask |= 1_u32 << mixer_track;
            }
            if !succeeded {
                self.master_block[..frames].fill([0.0; 2]);
                return progress;
            }
        }
        let any_insert_solo = self.track_solo[1..].iter().any(|solo| *solo);
        for frame_index in 0..frames {
            if mixer_track != 0
                && (self.track_muted[mixer_track]
                    || (any_insert_solo && !self.track_solo[mixer_track]))
            {
                continue;
            }
            let [source_left, source_right] =
                self.track_block[mixer_track * MAX_MIXER_BLOCK_FRAMES + frame_index];
            if mixer_track == 0 {
                self.master_block[frame_index] = [source_left, source_right];
            } else {
                let gain = self.track_gains[mixer_track];
                let pan = self.track_pans[mixer_track];
                self.master_block[frame_index] = [
                    source_left * gain * if pan > 0.0 { 1.0 - pan } else { 1.0 },
                    source_right * gain * if pan < 0.0 { 1.0 + pan } else { 1.0 },
                ];
            }
        }
        let has_master_insert = self.insert_endpoints[0].is_some();
        let master_succeeded = self.process_insert_endpoint(0, frames);
        if has_master_insert {
            progress.insert_mask |= 1;
        }
        if !master_succeeded {
            self.master_block[..frames].fill([0.0; 2]);
            return progress;
        }
        for frame in &mut self.master_block[..frames] {
            if self.track_muted[0] {
                *frame = [0.0; 2];
                continue;
            }
            let left_pan = if self.master_pan > 0.0 {
                1.0 - self.master_pan
            } else {
                1.0
            };
            let right_pan = if self.master_pan < 0.0 {
                1.0 + self.master_pan
            } else {
                1.0
            };
            *frame = [
                (frame[0] * self.master * left_pan).tanh(),
                (frame[1] * self.master * right_pan).tanh(),
            ];
        }
        self.publish_plugin_epoch_status(status);
        progress
    }

    fn render_paused_midi_monitor_graph(
        &mut self,
        status: &AudioStatus,
        frames: usize,
        monitor: PausedMidiMonitorRoute,
    ) -> PausedEndpointProgress {
        self.meter_graph_rendered = false;
        let mut progress = PausedEndpointProgress::default();
        for track in 0..TRACK_COUNT {
            let start = track * MAX_MIXER_BLOCK_FRAMES;
            self.track_block[start..start + frames].fill([0.0; 2]);
        }
        self.master_block[..frames].fill([0.0; 2]);
        if !self.mixer_graph_binding_is_exact() || !self.refresh_graph_pdc_plan(status) {
            self.fail_mixer_graph_render(frames);
            return progress;
        }

        let Some(slot) = self.generator_endpoints[monitor.generator_index].as_mut() else {
            return progress;
        };
        let (mixer_track, channel_volume, channel_pan, channel_audible) = self
            .timeline_channel_bases
            .get(slot.channel_id)
            .map_or((slot.mixer_track, 1.0, 0.0, true), |base| {
                (
                    base.mixer_track,
                    base.volume,
                    base.pan,
                    self.timeline_channel_bases.is_audible(base),
                )
            });
        if mixer_track != monitor.mixer_track
            || mixer_track >= TRACK_COUNT
            || self
                .mixer_graph_plan
                .node_at_runtime_slot(mixer_track as u8)
                .is_none()
        {
            self.fail_mixer_graph_render(frames);
            return progress;
        }
        let had_admitted_marker = slot.endpoint.has_admitted_live_edit_marker();
        let before = slot.endpoint.stats();
        let process_status = slot.endpoint.process_generator(
            self.transport_epoch,
            frames,
            &mut self.plugin_output_left[..frames],
            &mut self.plugin_output_right[..frames],
        );
        progress.generator_mask |= 1_u64 << monitor.generator_index;
        let diagnostics = fixed_quantum_stats_delta(before, slot.endpoint.stats());
        if had_admitted_marker && !slot.endpoint.has_admitted_live_edit_marker() {
            self.admitted_live_edit_endpoint_count =
                self.admitted_live_edit_endpoint_count.saturating_sub(1);
        }
        if diagnostics.event_overflows != 0
            || diagnostics.endpoint_event_rejections != 0
            || diagnostics.invalid_events != 0
            || diagnostics.latency_drift_quanta != 0
        {
            let deferred = slot.endpoint.clear_and_stage_all_notes_off();
            slot.suppress_output_frames = slot
                .suppress_output_frames
                .max(fixed_quantum_fail_closed_frames(&slot.endpoint).saturating_add(deferred));
        }
        let exact_output = matches!(
            process_status,
            FixedQuantumProcessStatus::Processed {
                frames: processed_frames,
                ..
            } if processed_frames == frames
        );
        let suppressed = slot.suppress_output_frames != 0;
        slot.suppress_output_frames = slot.suppress_output_frames.saturating_sub(frames);
        let source_start = mixer_track * MAX_MIXER_BLOCK_FRAMES;
        for frame_index in 0..frames {
            let source = if exact_output && !suppressed && channel_audible {
                [
                    safe_plugin_sample(self.plugin_output_left[frame_index])
                        * channel_volume
                        * if channel_pan > 0.0 {
                            1.0 - channel_pan
                        } else {
                            1.0
                        },
                    safe_plugin_sample(self.plugin_output_right[frame_index])
                        * channel_volume
                        * if channel_pan < 0.0 {
                            1.0 + channel_pan
                        } else {
                            1.0
                        },
                ]
            } else {
                [0.0; 2]
            };
            self.track_block[source_start + frame_index] = slot.pdc_delay.process_sample(source);
        }
        self.accumulate_fixed_quantum_diagnostics(diagnostics);
        if !exact_output
            || suppressed
            || diagnostics.event_overflows != 0
            || diagnostics.endpoint_event_rejections != 0
            || diagnostics.invalid_events != 0
            || diagnostics.latency_drift_quanta != 0
        {
            self.fail_mixer_graph_render(frames);
            return progress;
        }

        let mut downstream = [false; TRACK_COUNT];
        downstream[mixer_track] = true;
        for source_runtime_slot in self
            .mixer_graph_plan
            .topological_runtime_slots()
            .iter()
            .copied()
        {
            if !downstream[usize::from(source_runtime_slot)] {
                continue;
            }
            for route_runtime_slot in self
                .mixer_graph_plan
                .outgoing_route_slots(source_runtime_slot)
            {
                let Some(route) = self
                    .mixer_graph_plan
                    .route_at_runtime_slot(*route_runtime_slot)
                else {
                    self.fail_mixer_graph_render(frames);
                    return progress;
                };
                downstream[usize::from(route.destination_runtime_slot)] = true;
            }
        }
        let (_, audible_nodes) = self.mixer_graph_node_masks();
        let mut topological_runtime_slots = [0_u8; TRACK_COUNT];
        let node_count = self.mixer_graph_plan.node_count();
        topological_runtime_slots[..node_count]
            .copy_from_slice(self.mixer_graph_plan.topological_runtime_slots());
        for runtime_slot in topological_runtime_slots[..node_count].iter().copied() {
            let runtime_index = usize::from(runtime_slot);
            if !downstream[runtime_index] {
                continue;
            }
            let source_audible = !self.track_muted[runtime_index] && audible_nodes[runtime_index];
            let has_insert = self.insert_endpoints[runtime_index].is_some();
            if !self.process_mixer_graph_routes(
                runtime_slot,
                MixerRouteTap::PreEffects,
                frames,
                source_audible,
            ) {
                self.fail_mixer_graph_render(frames);
                return progress;
            }
            let insert_succeeded = self.process_mixer_graph_insert_endpoint(runtime_index, frames);
            if has_insert {
                progress.insert_mask |= 1_u32 << runtime_index;
            }
            if !insert_succeeded
                || !self.process_mixer_graph_routes(
                    runtime_slot,
                    MixerRouteTap::PostEffects,
                    frames,
                    source_audible,
                )
            {
                self.fail_mixer_graph_render(frames);
                return progress;
            }

            let gain = if runtime_slot == MIXER_MASTER_RUNTIME_SLOT {
                self.master
            } else {
                self.track_gains[runtime_index]
            };
            let pan = if runtime_slot == MIXER_MASTER_RUNTIME_SLOT {
                self.master_pan
            } else {
                self.track_pans[runtime_index]
            };
            if !gain.is_finite() || !pan.is_finite() {
                self.fail_mixer_graph_render(frames);
                return progress;
            }
            let start = runtime_index * MAX_MIXER_BLOCK_FRAMES;
            for frame_index in 0..frames {
                if !source_audible {
                    self.track_block[start + frame_index] = [0.0; 2];
                    continue;
                }
                let [left, right] = self.track_block[start + frame_index];
                let processed = [
                    left * gain * if pan > 0.0 { 1.0 - pan } else { 1.0 },
                    right * gain * if pan < 0.0 { 1.0 + pan } else { 1.0 },
                ];
                if !processed[0].is_finite() || !processed[1].is_finite() {
                    self.fail_mixer_graph_render(frames);
                    return progress;
                }
                self.track_block[start + frame_index] = processed;
            }
            if !self.process_mixer_graph_routes(
                runtime_slot,
                MixerRouteTap::PostFader,
                frames,
                source_audible,
            ) {
                self.fail_mixer_graph_render(frames);
                return progress;
            }
        }

        if downstream[usize::from(MIXER_MASTER_RUNTIME_SLOT)] {
            let start = usize::from(MIXER_MASTER_RUNTIME_SLOT) * MAX_MIXER_BLOCK_FRAMES;
            for frame_index in 0..frames {
                let [left, right] = self.track_block[start + frame_index];
                self.master_block[frame_index] = [left.tanh(), right.tanh()];
            }
        }
        self.meter_graph_rendered = true;
        self.publish_plugin_epoch_status(status);
        progress
    }

    fn service_paused_midi_safety(&mut self, frames: usize, skip_mask: u64) -> u64 {
        let mut processed_mask = 0_u64;
        for index in 0..MAX_GENERATOR_ENDPOINTS {
            if skip_mask & (1_u64 << index) != 0 {
                continue;
            }
            let Some(mut safety) = self.paused_midi_safety[index].take() else {
                continue;
            };
            if self.exact_midi_generator_index(safety.stamp) != Some(index) {
                continue;
            }
            let process_frames = frames.min(safety.remaining_frames);
            let slot = self.generator_endpoints[index]
                .as_mut()
                .expect("exact MIDI safety endpoint remains installed");
            let before = slot.endpoint.stats();
            let _ = slot.endpoint.process_generator(
                self.transport_epoch,
                process_frames,
                &mut self.plugin_output_left[..process_frames],
                &mut self.plugin_output_right[..process_frames],
            );
            let diagnostics = fixed_quantum_stats_delta(before, slot.endpoint.stats());
            let mut rearmed_budget = None;
            if diagnostics.event_overflows != 0
                || diagnostics.endpoint_event_rejections != 0
                || diagnostics.invalid_events != 0
                || diagnostics.latency_drift_quanta != 0
            {
                let phase = slot.endpoint.input_phase_frames();
                let deferred = slot.endpoint.panic_midi_preserving_parameter_edits(
                    Some(safety.stamp.slot.unwrap_or(0) as u8),
                    false,
                );
                slot.suppress_output_frames = slot
                    .suppress_output_frames
                    .max(fixed_quantum_fail_closed_frames(&slot.endpoint).saturating_add(deferred));
                rearmed_budget = Some(midi_safety_frames_to_complete_quantum(phase, deferred));
            }
            self.accumulate_fixed_quantum_diagnostics(diagnostics);
            safety.remaining_frames = rearmed_budget
                .unwrap_or_else(|| safety.remaining_frames.saturating_sub(process_frames));
            if safety.remaining_frames != 0 {
                self.paused_midi_safety[index] = Some(safety);
            }
            processed_mask |= 1_u64 << index;
        }
        processed_mask
    }

    fn observe_midi_safety_progress(&mut self, processed_mask: u64, frames: usize) {
        for index in 0..MAX_GENERATOR_ENDPOINTS {
            if processed_mask & (1_u64 << index) == 0 {
                continue;
            }
            let Some(mut safety) = self.paused_midi_safety[index].take() else {
                continue;
            };
            if self.exact_midi_generator_index(safety.stamp) != Some(index) {
                continue;
            }
            safety.remaining_frames = safety.remaining_frames.saturating_sub(frames);
            if safety.remaining_frames != 0 {
                self.paused_midi_safety[index] = Some(safety);
            }
        }
    }

    /// While an active timeline is paused, advance only the exact worker endpoints that own an
    /// admitted Live edit, and only through the remainder of their current physical Q128 quantum.
    /// Output is discarded; timeline/native/audio cursors and every PDC tap remain untouched.
    fn service_paused_parameter_edits(
        &mut self,
        device_frames: usize,
        skip_generator_mask: u64,
        skip_insert_mask: u32,
    ) {
        if device_frames == 0 || self.admitted_live_edit_endpoint_count == 0 {
            return;
        }
        let mut latency_drifted = false;
        let mut completed_endpoints = 0_usize;

        for index in 0..MAX_GENERATOR_ENDPOINTS {
            if skip_generator_mask & (1_u64 << index) != 0 {
                continue;
            }
            let Some(slot) = self.generator_endpoints[index].as_mut() else {
                continue;
            };
            if !slot.endpoint.has_admitted_live_edit_marker() {
                continue;
            }
            let phase = slot.endpoint.input_phase_frames();
            let frames = device_frames.min(DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES - phase);
            let before = slot.endpoint.stats();
            let _ = slot.endpoint.process_generator(
                self.transport_epoch,
                frames,
                &mut self.plugin_output_left[..frames],
                &mut self.plugin_output_right[..frames],
            );
            let diagnostics = fixed_quantum_stats_delta(before, slot.endpoint.stats());
            if diagnostics.event_overflows != 0
                || diagnostics.endpoint_event_rejections != 0
                || diagnostics.invalid_events != 0
                || diagnostics.latency_drift_quanta != 0
            {
                let deferred = slot.endpoint.clear_and_stage_all_notes_off();
                slot.suppress_output_frames = slot
                    .suppress_output_frames
                    .max(fixed_quantum_fail_closed_frames(&slot.endpoint).saturating_add(deferred));
            }
            let completed_marker = !slot.endpoint.has_admitted_live_edit_marker();
            self.accumulate_fixed_quantum_diagnostics(diagnostics);
            latency_drifted |= diagnostics.latency_drift_quanta != 0;
            if completed_marker {
                completed_endpoints += 1;
            }
        }

        for insert in 0..TRACK_COUNT {
            if skip_insert_mask & (1_u32 << insert) != 0 {
                continue;
            }
            let Some(slot) = self.insert_endpoints[insert].as_mut() else {
                continue;
            };
            if !slot.endpoint.has_admitted_live_edit_marker() {
                continue;
            }
            let phase = slot.endpoint.input_phase_frames();
            let frames = device_frames.min(DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES - phase);
            self.plugin_input_left[..frames].fill(0.0);
            self.plugin_input_right[..frames].fill(0.0);
            let before = slot.endpoint.stats();
            let _ = slot.endpoint.process(
                self.transport_epoch,
                &self.plugin_input_left[..frames],
                &self.plugin_input_right[..frames],
                &mut self.plugin_output_left[..frames],
                &mut self.plugin_output_right[..frames],
            );
            let diagnostics = fixed_quantum_stats_delta(before, slot.endpoint.stats());
            if diagnostics.event_overflows != 0
                || diagnostics.endpoint_event_rejections != 0
                || diagnostics.invalid_events != 0
                || diagnostics.latency_drift_quanta != 0
            {
                let deferred = slot.endpoint.clear_and_stage_all_notes_off();
                slot.suppress_output_frames = slot
                    .suppress_output_frames
                    .max(fixed_quantum_fail_closed_frames(&slot.endpoint).saturating_add(deferred));
            }
            let completed_marker = !slot.endpoint.has_admitted_live_edit_marker();
            self.accumulate_fixed_quantum_diagnostics(diagnostics);
            latency_drifted |= diagnostics.latency_drift_quanta != 0;
            if completed_marker {
                completed_endpoints += 1;
            }
        }

        self.admitted_live_edit_endpoint_count = self
            .admitted_live_edit_endpoint_count
            .saturating_sub(completed_endpoints);

        if latency_drifted {
            if let Some(runtime) = self.timeline_runtime.as_mut() {
                runtime.require_resync();
            }
            self.fail_timeline_block();
        }
    }

    fn accumulate_fixed_quantum_diagnostics(&mut self, diagnostics: FixedQuantumStatsDelta) {
        self.fixed_quantum_event_overflows = self
            .fixed_quantum_event_overflows
            .saturating_add(diagnostics.event_overflows);
        self.fixed_quantum_invalid_events = self
            .fixed_quantum_invalid_events
            .saturating_add(diagnostics.invalid_events);
        self.fixed_quantum_endpoint_event_rejections = self
            .fixed_quantum_endpoint_event_rejections
            .saturating_add(diagnostics.endpoint_event_rejections);
        self.fixed_quantum_bridge_gaps = self
            .fixed_quantum_bridge_gaps
            .saturating_add(diagnostics.bridge_gaps);
        self.fixed_quantum_output_underflow_frames = self
            .fixed_quantum_output_underflow_frames
            .saturating_add(diagnostics.output_underflow_frames);
    }

    fn process_insert_endpoint(&mut self, insert: usize, frames: usize) -> bool {
        self.process_insert_endpoint_impl(insert, frames, insert == 0, false)
    }

    fn process_mixer_graph_insert_endpoint(&mut self, insert: usize, frames: usize) -> bool {
        self.process_insert_endpoint_impl(insert, frames, false, true)
    }

    fn process_insert_endpoint_impl(
        &mut self,
        insert: usize,
        frames: usize,
        use_master_bus: bool,
        strict: bool,
    ) -> bool {
        if self.insert_endpoints[insert].is_none() || frames == 0 {
            return true;
        }

        for frame_index in 0..frames {
            let [left, right] = if use_master_bus {
                self.master_block[frame_index]
            } else {
                self.track_block[insert * MAX_MIXER_BLOCK_FRAMES + frame_index]
            };
            self.plugin_input_left[frame_index] = left;
            self.plugin_input_right[frame_index] = right;
        }

        let Some(slot) = self.insert_endpoints[insert].as_mut() else {
            return true;
        };
        let had_admitted_marker = slot.endpoint.has_admitted_live_edit_marker();
        let before = slot.endpoint.stats();
        let status = slot.endpoint.process(
            self.transport_epoch,
            &self.plugin_input_left[..frames],
            &self.plugin_input_right[..frames],
            &mut self.plugin_output_left[..frames],
            &mut self.plugin_output_right[..frames],
        );
        let diagnostics = fixed_quantum_stats_delta(before, slot.endpoint.stats());
        if had_admitted_marker && !slot.endpoint.has_admitted_live_edit_marker() {
            self.admitted_live_edit_endpoint_count =
                self.admitted_live_edit_endpoint_count.saturating_sub(1);
        }
        self.fixed_quantum_event_overflows = self
            .fixed_quantum_event_overflows
            .saturating_add(diagnostics.event_overflows);
        self.fixed_quantum_invalid_events = self
            .fixed_quantum_invalid_events
            .saturating_add(diagnostics.invalid_events);
        self.fixed_quantum_endpoint_event_rejections = self
            .fixed_quantum_endpoint_event_rejections
            .saturating_add(diagnostics.endpoint_event_rejections);
        self.fixed_quantum_bridge_gaps = self
            .fixed_quantum_bridge_gaps
            .saturating_add(diagnostics.bridge_gaps);
        self.fixed_quantum_output_underflow_frames = self
            .fixed_quantum_output_underflow_frames
            .saturating_add(diagnostics.output_underflow_frames);
        let latency_drifted = diagnostics.latency_drift_quanta != 0;
        if diagnostics.event_overflows != 0
            || diagnostics.endpoint_event_rejections != 0
            || diagnostics.invalid_events != 0
            || latency_drifted
        {
            let deferred = slot.endpoint.clear_and_stage_all_notes_off();
            slot.suppress_output_frames = slot
                .suppress_output_frames
                .max(fixed_quantum_fail_closed_frames(&slot.endpoint).saturating_add(deferred));
        }
        if latency_drifted {
            if use_master_bus {
                self.master_block[..frames].fill([0.0; 2]);
            } else {
                let start = insert * MAX_MIXER_BLOCK_FRAMES;
                self.track_block[start..start + frames].fill([0.0; 2]);
            }
            if let Some(runtime) = self.timeline_runtime.as_mut() {
                runtime.require_resync();
            }
            self.fail_timeline_block();
            return false;
        }
        let FixedQuantumProcessStatus::Processed {
            frames: processed_frames,
            ..
        } = status
        else {
            if use_master_bus {
                self.master_block[..frames].fill([0.0; 2]);
            } else {
                let start = insert * MAX_MIXER_BLOCK_FRAMES;
                self.track_block[start..start + frames].fill([0.0; 2]);
            }
            return !strict;
        };
        if processed_frames != frames {
            if use_master_bus {
                self.master_block[..frames].fill([0.0; 2]);
            } else {
                let start = insert * MAX_MIXER_BLOCK_FRAMES;
                self.track_block[start..start + frames].fill([0.0; 2]);
            }
            return !strict;
        }
        let suppressed = slot.suppress_output_frames != 0;
        slot.suppress_output_frames = slot.suppress_output_frames.saturating_sub(frames);
        if suppressed {
            if use_master_bus {
                self.master_block[..frames].fill([0.0; 2]);
            } else {
                let start = insert * MAX_MIXER_BLOCK_FRAMES;
                self.track_block[start..start + frames].fill([0.0; 2]);
            }
            return !strict;
        }

        for frame_index in 0..frames {
            let processed = [
                safe_plugin_sample(self.plugin_output_left[frame_index]),
                safe_plugin_sample(self.plugin_output_right[frame_index]),
            ];
            if use_master_bus {
                self.master_block[frame_index] = processed;
            } else {
                self.track_block[insert * MAX_MIXER_BLOCK_FRAMES + frame_index] = processed;
            }
        }
        true
    }

    fn mixer_graph_binding_is_exact(&self) -> bool {
        let Some(identity) = self.mixer_graph_identity else {
            return false;
        };
        if identity.revision == 0
            || identity.epoch == 0
            || identity.fingerprint == 0
            || self.timeline_channel_revision != Some(identity.revision)
            || self.timeline_channel_epoch != Some(identity.epoch)
            || self.mixer_graph_plan.fingerprint() != identity.fingerprint
            || self
                .mixer_graph_plan
                .node_at_runtime_slot(MIXER_MASTER_RUNTIME_SLOT)
                .is_none()
        {
            return false;
        }
        let Some(graph_pdc_plan) = self.graph_pdc_plan.as_ref().as_ref() else {
            return false;
        };
        if graph_pdc_plan.graph_fingerprint() != Some(identity.fingerprint)
            || graph_pdc_plan.node_count() != self.mixer_graph_plan.node_count()
            || graph_pdc_plan.main_input_count() != self.mixer_graph_plan.route_count()
        {
            return false;
        }
        self.timeline_runtime.as_ref().is_some_and(|runtime| {
            runtime.active_revision() == Some(identity.revision)
                && runtime.active_epoch() == Some(identity.epoch)
                && runtime.active_timeline().is_some_and(|timeline| {
                    timeline.mixer_graph().fingerprint() == identity.fingerprint
                })
                && runtime.active_mixer_delay_bank().is_some_and(|bank| {
                    bank.graph_fingerprint() == identity.fingerprint
                        && bank.route_count() == self.mixer_graph_plan.route_count()
                })
        })
    }

    fn fail_mixer_graph_render(&mut self, frames: usize) {
        for track in 0..TRACK_COUNT {
            let start = track * MAX_MIXER_BLOCK_FRAMES;
            self.track_block[start..start + frames].fill([0.0; 2]);
        }
        self.master_block[..frames].fill([0.0; 2]);
        if let Some(runtime) = self.timeline_runtime.as_mut() {
            runtime.require_resync();
        }
        if self.timeline_channel_revision.is_some() {
            self.fail_timeline_block();
        }
    }

    fn mixer_graph_node_masks(&self) -> ([bool; TRACK_COUNT], [bool; TRACK_COUNT]) {
        let mut occupied = [false; TRACK_COUNT];
        for runtime_slot in self.mixer_graph_plan.topological_runtime_slots() {
            occupied[usize::from(*runtime_slot)] = true;
        }
        let any_solo = (1..TRACK_COUNT).any(|slot| occupied[slot] && self.track_solo[slot]);
        if !any_solo {
            return (occupied, occupied);
        }

        let mut upstream = [false; TRACK_COUNT];
        let mut downstream = [false; TRACK_COUNT];
        for slot in 1..TRACK_COUNT {
            if occupied[slot] && self.track_solo[slot] {
                upstream[slot] = true;
                downstream[slot] = true;
            }
        }
        for source_runtime_slot in self
            .mixer_graph_plan
            .topological_runtime_slots()
            .iter()
            .rev()
            .copied()
        {
            for route_runtime_slot in self
                .mixer_graph_plan
                .outgoing_route_slots(source_runtime_slot)
            {
                let Some(route) = self
                    .mixer_graph_plan
                    .route_at_runtime_slot(*route_runtime_slot)
                else {
                    continue;
                };
                if upstream[usize::from(route.destination_runtime_slot)] {
                    upstream[usize::from(source_runtime_slot)] = true;
                }
            }
        }
        for source_runtime_slot in self
            .mixer_graph_plan
            .topological_runtime_slots()
            .iter()
            .copied()
        {
            if !downstream[usize::from(source_runtime_slot)] {
                continue;
            }
            for route_runtime_slot in self
                .mixer_graph_plan
                .outgoing_route_slots(source_runtime_slot)
            {
                let Some(route) = self
                    .mixer_graph_plan
                    .route_at_runtime_slot(*route_runtime_slot)
                else {
                    continue;
                };
                downstream[usize::from(route.destination_runtime_slot)] = true;
            }
        }
        let mut audible = [false; TRACK_COUNT];
        for slot in 0..TRACK_COUNT {
            audible[slot] = occupied[slot] && (upstream[slot] || downstream[slot]);
        }
        (occupied, audible)
    }

    fn process_mixer_graph_routes(
        &mut self,
        source_runtime_slot: u8,
        tap: MixerRouteTap,
        frames: usize,
        source_audible: bool,
    ) -> bool {
        let route_count = self
            .mixer_graph_plan
            .outgoing_route_slots(source_runtime_slot)
            .len();
        for route_index in 0..route_count {
            let route_runtime_slot = self
                .mixer_graph_plan
                .outgoing_route_slots(source_runtime_slot)[route_index];
            let Some(route) = self
                .mixer_graph_plan
                .route_at_runtime_slot(route_runtime_slot)
            else {
                return false;
            };
            if route.tap != tap {
                continue;
            }
            if route.source_runtime_slot != source_runtime_slot
                || self
                    .mixer_graph_plan
                    .node_at_runtime_slot(route.destination_runtime_slot)
                    .is_none()
                || !route.gain.is_finite()
            {
                return false;
            }
            let source_start = usize::from(source_runtime_slot) * MAX_MIXER_BLOCK_FRAMES;
            let destination_start =
                usize::from(route.destination_runtime_slot) * MAX_MIXER_BLOCK_FRAMES;
            for frame_index in 0..frames {
                let source = if source_audible {
                    self.track_block[source_start + frame_index]
                } else {
                    [0.0; 2]
                };
                let input = [source[0] * route.gain, source[1] * route.gain];
                if !input[0].is_finite() || !input[1].is_finite() {
                    return false;
                }
                let delayed = self
                    .timeline_runtime
                    .as_mut()
                    .and_then(RealtimeTimelineRuntime::active_mixer_delay_bank_mut)
                    .and_then(|bank| bank.process_sample(usize::from(route_runtime_slot), input));
                let Some(delayed) = delayed else {
                    return false;
                };
                let destination = &mut self.track_block[destination_start + frame_index];
                let left = destination[0] + delayed[0];
                let right = destination[1] + delayed[1];
                if !left.is_finite() || !right.is_finite() {
                    return false;
                }
                *destination = [left, right];
            }
        }
        true
    }

    fn process_mixer_graph_raw_source_pdc(&mut self, frames: usize) {
        for runtime_slot in self
            .mixer_graph_plan
            .topological_runtime_slots()
            .iter()
            .copied()
        {
            let slot = usize::from(runtime_slot);
            let start = slot * MAX_MIXER_BLOCK_FRAMES;
            let delay = &mut self.pdc_raw_track_delays[slot];
            for frame_index in 0..frames {
                let index = start + frame_index;
                self.track_block[index] = delay.process_sample(self.track_block[index]);
            }
        }
    }

    fn render_mixer_graph_block(&mut self, status: &AudioStatus, frames: usize) {
        self.meter_graph_rendered = false;
        self.publish_plugin_epoch_status(status);
        if !self.mixer_graph_binding_is_exact() {
            self.fail_mixer_graph_render(frames);
            self.publish_plugin_epoch_status(status);
            return;
        }
        if !self.refresh_graph_pdc_plan(status) {
            self.fail_mixer_graph_render(frames);
            self.publish_plugin_epoch_status(status);
            return;
        }

        for track in 0..TRACK_COUNT {
            let start = track * MAX_MIXER_BLOCK_FRAMES;
            self.track_block[start..start + frames].fill([0.0; 2]);
        }
        self.master_block[..frames].fill([0.0; 2]);

        let transport_playing = status.playing.load(Ordering::Acquire);
        for frame_index in 0..frames {
            self.apply_timeline_events_for_frame(frame_index);
            self.render_synth_frame(frame_index);
            if transport_playing {
                self.render_audio_frame(frame_index);
            }
            self.render_metronome_frame(status, frame_index);
        }
        if self.timeline_plan_render_active {
            if !self.timeline_plan.rendered_completely() {
                let end_frame = self
                    .timeline_render_start_frame
                    .saturating_add(frames as u64);
                self.poison_active_timeline_cursor(self.transport_epoch, end_frame);
                self.fail_mixer_graph_render(frames);
                self.publish_plugin_epoch_status(status);
                return;
            }
            self.timeline_plan_render_active = false;
            self.timeline_plan.clear();
        }

        self.process_mixer_graph_raw_source_pdc(frames);
        let (graph_nodes, audible_nodes) = self.mixer_graph_node_masks();
        if !self.process_mixer_graph_generator_endpoints(frames, &graph_nodes) {
            self.fail_mixer_graph_render(frames);
            self.publish_plugin_epoch_status(status);
            return;
        }

        let mut topological_runtime_slots = [0_u8; TRACK_COUNT];
        let node_count = self.mixer_graph_plan.node_count();
        topological_runtime_slots[..node_count]
            .copy_from_slice(self.mixer_graph_plan.topological_runtime_slots());
        for runtime_slot in topological_runtime_slots[..node_count].iter().copied() {
            let slot = usize::from(runtime_slot);
            let source_audible = !self.track_muted[slot] && audible_nodes[slot];
            if !self.process_mixer_graph_routes(
                runtime_slot,
                MixerRouteTap::PreEffects,
                frames,
                source_audible,
            ) || !self.process_mixer_graph_insert_endpoint(slot, frames)
                || !self.process_mixer_graph_routes(
                    runtime_slot,
                    MixerRouteTap::PostEffects,
                    frames,
                    source_audible,
                )
            {
                self.fail_mixer_graph_render(frames);
                self.publish_plugin_epoch_status(status);
                return;
            }

            let muted = !source_audible;
            let gain = if runtime_slot == MIXER_MASTER_RUNTIME_SLOT {
                self.master
            } else {
                self.track_gains[slot]
            };
            let pan = if runtime_slot == MIXER_MASTER_RUNTIME_SLOT {
                self.master_pan
            } else {
                self.track_pans[slot]
            };
            if !gain.is_finite() || !pan.is_finite() {
                self.fail_mixer_graph_render(frames);
                self.publish_plugin_epoch_status(status);
                return;
            }
            let start = slot * MAX_MIXER_BLOCK_FRAMES;
            for frame_index in 0..frames {
                if muted {
                    self.track_block[start + frame_index] = [0.0; 2];
                    continue;
                }
                let [left, right] = self.track_block[start + frame_index];
                let processed = [
                    left * gain * if pan > 0.0 { 1.0 - pan } else { 1.0 },
                    right * gain * if pan < 0.0 { 1.0 + pan } else { 1.0 },
                ];
                if !processed[0].is_finite() || !processed[1].is_finite() {
                    self.fail_mixer_graph_render(frames);
                    self.publish_plugin_epoch_status(status);
                    return;
                }
                self.track_block[start + frame_index] = processed;
            }
            if !self.process_mixer_graph_routes(
                runtime_slot,
                MixerRouteTap::PostFader,
                frames,
                source_audible,
            ) {
                self.fail_mixer_graph_render(frames);
                self.publish_plugin_epoch_status(status);
                return;
            }
        }

        let master_start = usize::from(MIXER_MASTER_RUNTIME_SLOT) * MAX_MIXER_BLOCK_FRAMES;
        for frame_index in 0..frames {
            let [left, right] = self.track_block[master_start + frame_index];
            self.master_block[frame_index] = [left.tanh(), right.tanh()];
        }
        self.meter_graph_rendered = true;
        self.publish_plugin_epoch_status(status);
    }

    fn publish_meters(&mut self, end_device_frame: u64, frames: usize) {
        if self.meter_publisher.is_none() || frames == 0 {
            return;
        }
        let mut frame = MeterFrame {
            end_device_frame,
            ..MeterFrame::default()
        };
        if self.mixer_graph_binding_is_exact() {
            let identity = self.mixer_graph_identity.expect("exact graph identity");
            frame.identity = Some(MeterIdentity {
                revision: identity.revision,
                epoch: identity.epoch,
                graph_fingerprint: identity.fingerprint,
            });
            for runtime_slot in self
                .mixer_graph_plan
                .topological_runtime_slots()
                .iter()
                .copied()
            {
                let node = self
                    .mixer_graph_plan
                    .node_at_runtime_slot(runtime_slot)
                    .expect("validated graph slot");
                let slot = usize::from(runtime_slot);
                // Silent/unrendered segments must not sample an old track_block.
                frame.tracks[slot] = if self.meter_graph_rendered {
                    let start = slot * MAX_MIXER_BLOCK_FRAMES;
                    TrackPeak::measure(node.id, &self.track_block[start..start + frames])
                } else {
                    TrackPeak::measure(node.id, &[])
                };
            }
        }
        if let Some(publisher) = self.meter_publisher.as_mut() {
            publisher.publish(frame);
        }
    }

    fn render_block(&mut self, status: &AudioStatus, frames: usize) {
        debug_assert!(frames <= MAX_MIXER_BLOCK_FRAMES);
        if self.mixer_graph_was_activated {
            self.render_mixer_graph_block(status, frames);
            return;
        }
        self.publish_plugin_epoch_status(status);
        self.refresh_pdc_plan(status, frames);
        for track in 0..TRACK_COUNT {
            let start = track * MAX_MIXER_BLOCK_FRAMES;
            self.track_block[start..start + frames].fill([0.0; 2]);
        }
        self.master_block[..frames].fill([0.0; 2]);

        let transport_playing = status.playing.load(Ordering::Acquire);
        for frame_index in 0..frames {
            self.apply_timeline_events_for_frame(frame_index);
            // Native Channel automation is consumed while producing the raw
            // synth source, before the block enters source-level PDC below.
            self.render_synth_frame(frame_index);
            if transport_playing {
                self.render_audio_frame(frame_index);
            }
            self.render_metronome_frame(status, frame_index);
        }
        if self.timeline_plan_render_active {
            if !self.timeline_plan.rendered_completely() {
                let end_frame = self
                    .timeline_render_start_frame
                    .saturating_add(frames as u64);
                self.poison_active_timeline_cursor(self.transport_epoch, end_frame);
                self.fail_timeline_block();
                for track in 0..TRACK_COUNT {
                    let start = track * MAX_MIXER_BLOCK_FRAMES;
                    self.track_block[start..start + frames].fill([0.0; 2]);
                }
            } else {
                self.timeline_plan_render_active = false;
                self.timeline_plan.clear();
            }
        }

        // Native synth/audio/track-zero sources are compensated independently
        // from worker-backed generators, then converge before each track insert.
        self.process_raw_source_pdc(frames);

        // Instruments receive a silent source block and accumulate into their
        // routed track before that track's ordered insert worker is submitted.
        if !self.process_generator_endpoints(frames) {
            for track in 0..TRACK_COUNT {
                let start = track * MAX_MIXER_BLOCK_FRAMES;
                self.track_block[start..start + frames].fill([0.0; 2]);
            }
            self.master_block[..frames].fill([0.0; 2]);
            self.publish_plugin_epoch_status(status);
            return;
        }

        // Track inserts process their raw source buses (including instruments)
        // before mute/solo/fader/pan. Insert 0 is the summed master below.
        for insert in 1..TRACK_COUNT {
            if !self.process_insert_endpoint(insert, frames) {
                for track in 0..TRACK_COUNT {
                    let start = track * MAX_MIXER_BLOCK_FRAMES;
                    self.track_block[start..start + frames].fill([0.0; 2]);
                }
                self.master_block[..frames].fill([0.0; 2]);
                self.publish_plugin_epoch_status(status);
                return;
            }
        }

        let any_insert_solo = self.track_solo[1..].iter().any(|solo| *solo);
        for frame_index in 0..frames {
            let mut left = self.master_block[frame_index][0];
            let mut right = self.master_block[frame_index][1];
            for track in 0..TRACK_COUNT {
                if track != 0
                    && (self.track_muted[track] || (any_insert_solo && !self.track_solo[track]))
                {
                    continue;
                }
                let [source_left, source_right] =
                    self.track_block[track * MAX_MIXER_BLOCK_FRAMES + frame_index];
                if track == 0 {
                    left += source_left;
                    right += source_right;
                    continue;
                }
                let gain = self.track_gains[track];
                let pan = self.track_pans[track];
                left += source_left * gain * if pan > 0.0 { 1.0 - pan } else { 1.0 };
                right += source_right * gain * if pan < 0.0 { 1.0 + pan } else { 1.0 };
            }

            self.master_block[frame_index] = [left, right];
        }

        // The master insert sees track 0, every routed insert, and the metronome,
        // but remains pre-fader/pre-pan/pre-mute like the regular track chains.
        if !self.process_insert_endpoint(0, frames) {
            self.master_block[..frames].fill([0.0; 2]);
            self.publish_plugin_epoch_status(status);
            return;
        }

        for frame_index in 0..frames {
            if self.track_muted[0] {
                self.master_block[frame_index] = [0.0; 2];
                continue;
            }
            let [left, right] = self.master_block[frame_index];
            let left_pan = if self.master_pan > 0.0 {
                1.0 - self.master_pan
            } else {
                1.0
            };
            let right_pan = if self.master_pan < 0.0 {
                1.0 + self.master_pan
            } else {
                1.0
            };
            self.master_block[frame_index] = [
                (left * self.master * left_pan).tanh(),
                (right * self.master * right_pan).tanh(),
            ];
        }
        self.publish_plugin_epoch_status(status);
    }

    #[cfg(test)]
    fn next_frame(&mut self, status: &AudioStatus) -> (f32, f32) {
        self.render_block(status, 1);
        let [left, right] = self.master_block[0];
        (left, right)
    }
}

impl Drop for DspState {
    fn drop(&mut self) {
        // Normal shutdown clears and retires every endpoint before the CPAL stream
        // is released. This guard prevents an unexpected backend-side callback
        // teardown from ever running endpoint destruction on its real-time thread.
        for slot in &mut self.insert_endpoints {
            if let Some(slot) = slot.take() {
                std::mem::forget(slot.endpoint);
            }
        }
        for slot in &mut self.generator_endpoints {
            if let Some(slot) = slot.take() {
                std::mem::forget(slot);
            }
        }
        if let Some(capture) = self.master_capture.take() {
            std::mem::forget(capture.endpoint);
        }
        if let Some(route) = self.midi_input.take() {
            std::mem::forget(route);
        }
        if let Some(recording) = self.midi_recording.take() {
            std::mem::forget(recording.endpoint);
        }
        if let Some(endpoint) = self.pending_midi_recording_start.take() {
            std::mem::forget(endpoint);
        }
    }
}

fn forget_master_capture_event_endpoint(event: MasterCaptureEndpointEvent) {
    let endpoint = match event {
        MasterCaptureEndpointEvent::Installed {
            returned_endpoint, ..
        }
        | MasterCaptureEndpointEvent::Stopped {
            returned_endpoint, ..
        }
        | MasterCaptureEndpointEvent::Cleared {
            returned_endpoint, ..
        } => returned_endpoint,
    };
    if let Some(endpoint) = endpoint {
        std::mem::forget(endpoint);
    }
}

fn forget_midi_recording_event_endpoint(event: MidiRecordingEndpointEvent) {
    let endpoint = match event {
        MidiRecordingEndpointEvent::Started {
            returned_endpoint, ..
        }
        | MidiRecordingEndpointEvent::Stopped {
            returned_endpoint, ..
        }
        | MidiRecordingEndpointEvent::Cleared {
            returned_endpoint, ..
        } => returned_endpoint,
    };
    if let Some(endpoint) = endpoint {
        std::mem::forget(endpoint);
    }
}

fn asset_layout_is_valid(samples: &[f32], sample_rate: u32, channels: u16) -> bool {
    sample_rate != 0
        && channels != 0
        && !samples.is_empty()
        && samples.len().is_multiple_of(usize::from(channels))
}

fn retire_asset(asset: Arc<[f32]>, retired_assets: &mut Producer<Arc<[f32]>>) {
    if let Err(PushError::Full(asset)) = retired_assets.push(asset) {
        // `process_commands` reserves a slot before any operation that can retire
        // an Arc, so this branch is only a defensive invariant guard. Leaking is
        // preferable to a potentially huge deallocation on the audio callback.
        std::mem::forget(asset);
    }
}

fn safe_asset_sample(sample: f32) -> f32 {
    if sample.is_finite() {
        sample.clamp(-8.0, 8.0)
    } else {
        0.0
    }
}

fn safe_plugin_sample(sample: f32) -> f32 {
    if sample.is_finite() {
        sample.clamp(-8.0, 8.0)
    } else {
        0.0
    }
}

fn fixed_quantum_endpoint_latency(endpoint: &PreparedFixedEndpoint) -> u32 {
    let adapter_and_bridge = (DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES as u32).saturating_mul(2);
    adapter_and_bridge.saturating_add(
        endpoint
            .coherent_latency_snapshot()
            .map_or(0, |snapshot| snapshot.total_plugin_latency_samples),
    )
}

fn fixed_quantum_fail_closed_frames(endpoint: &PreparedFixedEndpoint) -> usize {
    fixed_quantum_endpoint_latency(endpoint) as usize
}

#[derive(Clone, Copy, Default)]
struct FixedQuantumStatsDelta {
    event_overflows: u64,
    invalid_events: u64,
    endpoint_event_rejections: u64,
    bridge_gaps: u64,
    output_underflow_frames: u64,
    latency_drift_quanta: u64,
}

fn fixed_quantum_stats_delta(
    before: FixedQuantumStats,
    after: FixedQuantumStats,
) -> FixedQuantumStatsDelta {
    FixedQuantumStatsDelta {
        event_overflows: after
            .frame_event_overflows
            .saturating_sub(before.frame_event_overflows),
        invalid_events: after
            .invalid_frame_events
            .saturating_sub(before.invalid_frame_events),
        endpoint_event_rejections: after
            .endpoint_event_rejections
            .saturating_sub(before.endpoint_event_rejections),
        bridge_gaps: after.bridge_gaps.saturating_sub(before.bridge_gaps),
        output_underflow_frames: after
            .output_underflow_frames
            .saturating_sub(before.output_underflow_frames),
        latency_drift_quanta: after
            .latency_drift_quanta
            .saturating_sub(before.latency_drift_quanta),
    }
}

fn finite_clamp(value: f32, minimum: f32, maximum: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(minimum, maximum)
    } else {
        fallback
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_parameter_edit::ParameterEditRoute;
    use crate::plugins::plugin_runtime::{
        BackendSlot, MidiMessage, ParameterEditReceipt, PluginBackend, PluginChain,
        PluginChainControl, PluginPrepareConfig, PluginWorkerGuard, SlotConfig,
    };
    use crate::{
        automation::{AutomationCurve, AutomationLane, AutomationPoint, AutomationTarget},
        master_capture::MasterCaptureSession,
        midi_device::TestMidiInputSender,
        midi_recording::prepare_midi_recording,
        mixer_graph::{MixerRoute, MixerRouteDestination},
        model::{
            AudioAsset, Channel, Clip, ClipKind, MixerInsertSlotRef, Pattern, PianoNote,
            PluginFormat, PluginInstance, PluginRole, PluginRuntimeStatus, Project,
            ProjectAutomation,
        },
        tempo_map::TempoMap,
        timeline::{
            AudioClipDescriptor, NoteSourceDescriptor, TimelineCompileOptions, TimelineEventKind,
        },
        timeline_runtime::{TimelineMixerPanRelease, create_timeline_runtime_with_capacities},
    };
    use std::path::PathBuf;

    type TestReclaimer = (Producer<Arc<[f32]>>, Consumer<Arc<[f32]>>);

    struct MidiRecordingAudioFixture {
        dsp: DspState,
        sender: TestMidiInputSender,
        recording_events: Consumer<MidiRecordingEndpointEvent>,
        retired_endpoints: Consumer<RetiredEndpointResource>,
        retired_midi_inputs: Consumer<RetiredMidiInputResource>,
        plugin_control: PluginChainControl,
        guard: PluginWorkerGuard,
        generator_stamp: MidiGeneratorRouteStamp,
    }

    impl MidiRecordingAudioFixture {
        fn new() -> Self {
            let chain = spawn_identified_mock_instrument(
                700,
                0.2,
                Arc::new(AtomicU32::new(0)),
                Arc::new(AtomicU32::new(0)),
            );
            let PluginChain {
                audio,
                control: plugin_control,
                guard,
            } = chain;
            let (retired_endpoint_tx, retired_endpoints) = RingBuffer::new(8);
            let (insert_event_tx, _insert_events) = RingBuffer::new(4);
            let (generator_event_tx, _generator_events) = RingBuffer::new(8);
            let mut dsp = DspState::new_with_endpoint_io(
                48_000.0,
                retired_endpoint_tx,
                insert_event_tx,
                generator_event_tx,
            );
            dsp.transport_epoch = 9;
            let mut endpoint = fixed_adapter(audio);
            endpoint.project_session = 7;
            dsp.install_generator_endpoint(9, 70, 700, 1, endpoint, test_pdc_delay());

            let (retired_midi_tx, retired_midi_inputs) = RingBuffer::new(8);
            let (midi_event_tx, _midi_events) = RingBuffer::new(8);
            let (recording_event_tx, recording_events) = RingBuffer::new(8);
            dsp.retired_midi_inputs = Some(retired_midi_tx);
            dsp.midi_input_route_events = Some(midi_event_tx);
            dsp.midi_recording_endpoint_events = Some(recording_event_tx);
            let (sender, receiver) = crate::midi_device::test_input_mailbox(32, 33);
            let generator_stamp = MidiGeneratorRouteStamp {
                project_session: 7,
                channel_id: 9,
                endpoint_id: 70,
                plugin_instance_id: 700,
                slot: None,
            };
            dsp.install_midi_input(
                5,
                PreparedMidiInputRoute::new(receiver, generator_stamp).unwrap(),
            );
            dsp.timeline_channel_revision = Some(41);
            dsp.timeline_channel_epoch = Some(9);
            Self {
                dsp,
                sender,
                recording_events,
                retired_endpoints,
                retired_midi_inputs,
                plugin_control,
                guard,
                generator_stamp,
            }
        }

        fn record_stamp(&self, session_id: u64) -> MidiRecordRealtimeStamp {
            MidiRecordRealtimeStamp {
                session_id,
                project_session: 7,
                timeline_revision: 41,
                route_id: 5,
                connection_epoch: 33,
                generator: self.generator_stamp,
            }
        }

        fn finish(mut self) {
            assert!(self.dsp.midi_recording.is_none());
            assert!(self.dsp.pending_midi_recording_start.is_none());
            self.dsp.remove_midi_input(5);
            drop(self.retired_midi_inputs.pop().unwrap());
            self.dsp.remove_generator_endpoint(9);
            drop(self.retired_endpoints.pop().unwrap());
            drop(self.dsp);
            self.guard.shutdown();
            drop(self.plugin_control);
        }
    }

    #[test]
    fn callback_parameter_edit_admission_is_exactly_receipt_capacity() {
        let admission = AtomicU32::new(0);
        for _ in 0..PARAMETER_EDIT_CALLBACK_ADMISSION_CAPACITY {
            assert!(try_admit_callback_parameter_edit(&admission));
        }
        assert!(!try_admit_callback_parameter_edit(&admission));
        for _ in 0..PARAMETER_EDIT_CALLBACK_ADMISSION_CAPACITY {
            release_callback_parameter_edit(&admission);
        }
        assert_eq!(admission.load(Ordering::Acquire), 0);
    }

    #[test]
    fn midi_safety_budget_finishes_the_quantum_that_contains_all_off() {
        assert_eq!(midi_safety_frames_to_complete_quantum(0, 0), 128);
        assert_eq!(midi_safety_frames_to_complete_quantum(64, 0), 64);
        assert_eq!(midi_safety_frames_to_complete_quantum(64, 64), 192);
        assert_eq!(midi_safety_frames_to_complete_quantum(127, 1), 129);
    }

    #[test]
    fn midi_panic_preserves_only_reliable_parameter_markers_and_targets_one_slot() {
        let mut events = EndpointFrameEvents::new();
        let partial = EndpointQuantumClassUsage::default();
        assert!(events.stage(
            EndpointEventClass::Live,
            FrameEvent::midi(0, Some(0), [0x90, 60, 100]),
            0,
            partial,
        ));
        assert!(events.stage(
            EndpointEventClass::Live,
            FrameEvent::parameter(0, 0, 7, 0.25),
            0,
            partial,
        ));
        assert!(events.stage(
            EndpointEventClass::Live,
            FrameEvent::admitted_parameter(0, 0, 8, 0.75, RuntimeParameterEditId::new(9).unwrap(),),
            0,
            partial,
        ));

        assert_eq!(
            events.panic_midi_preserving_parameter_edits(0, partial, Some(0), false),
            0
        );
        assert_eq!(events.scratch.live.pending_len, 1);
        assert!(matches!(
            events.scratch.live.pending[0].kind,
            crate::fixed_quantum::FrameEventKind::Parameter {
                edit_id: Some(_),
                id: 8,
                ..
            }
        ));
        assert_eq!(events.scratch.system.pending_len, 16);
        assert!(events.scratch.system.pending[..16].iter().all(|event| {
            matches!(
                event.kind,
                crate::fixed_quantum::FrameEventKind::Midi {
                    slot: Some(0),
                    data: [status, 123, 0],
                } if status & 0xf0 == 0xb0
            )
        }));
    }

    #[test]
    fn timeline_failure_clears_timeline_lane_but_preserves_reliable_live_marker() {
        let mut events = EndpointFrameEvents::new();
        let partial = EndpointQuantumClassUsage::default();
        assert!(events.stage(
            EndpointEventClass::Timeline,
            FrameEvent::midi(0, Some(0), [0x90, 64, 100]),
            0,
            partial,
        ));
        assert!(events.stage(
            EndpointEventClass::Live,
            FrameEvent::admitted_parameter(0, 0, 3, 0.5, RuntimeParameterEditId::new(77).unwrap(),),
            0,
            partial,
        ));

        let _ = events.panic_midi_preserving_parameter_edits(0, partial, None, true);
        assert_eq!(events.scratch.timeline.pending_len, 0);
        assert_eq!(events.scratch.live.pending_len, 1);
        assert!(matches!(
            events.scratch.live.pending[0].kind,
            crate::fixed_quantum::FrameEventKind::Parameter {
                edit_id: Some(_),
                ..
            }
        ));
        assert_eq!(events.scratch.system.pending_len, 16);
    }

    #[test]
    fn live_lane_overflow_keeps_all_admitted_markers_and_rejects_legacy_midi() {
        let mut events = EndpointFrameEvents::new();
        let partial = EndpointQuantumClassUsage::default();
        for index in 0..TIMELINE_ENDPOINT_LIVE_MAX_EVENTS_PER_QUANTUM {
            assert!(events.stage(
                EndpointEventClass::Live,
                FrameEvent::admitted_parameter(
                    0,
                    0,
                    index as u32,
                    0.5,
                    RuntimeParameterEditId::new(index as u64 + 1).unwrap(),
                ),
                0,
                partial,
            ));
        }
        assert!(!events.stage(
            EndpointEventClass::Live,
            FrameEvent::midi(0, Some(0), [0x90, 60, 100]),
            0,
            partial,
        ));
        assert_eq!(
            events.scratch.live.pending_len,
            TIMELINE_ENDPOINT_LIVE_MAX_EVENTS_PER_QUANTUM
        );
        assert!(
            events.scratch.live.pending[..events.scratch.live.pending_len]
                .iter()
                .all(|event| matches!(
                    event.kind,
                    crate::fixed_quantum::FrameEventKind::Parameter {
                        edit_id: Some(_),
                        ..
                    }
                ))
        );
        assert_eq!(events.scratch.system.pending_len, 16);
    }

    #[test]
    fn midi_input_first_anchor_splits_future_event_and_retires_off_callback() {
        let last_midi = Arc::new(AtomicU32::new(0));
        let chain = spawn_identified_mock_instrument(
            700,
            0.2,
            Arc::clone(&last_midi),
            Arc::new(AtomicU32::new(0)),
        );
        let PluginChain {
            audio,
            control,
            guard,
        } = chain;
        let (retired_endpoint_tx, mut retired_endpoint_rx) = RingBuffer::new(4);
        let (insert_event_tx, _insert_event_rx) = RingBuffer::new(4);
        let (generator_event_tx, _generator_event_rx) = RingBuffer::new(4);
        let mut dsp = DspState::new_with_endpoint_io(
            48_000.0,
            retired_endpoint_tx,
            insert_event_tx,
            generator_event_tx,
        );
        let mut endpoint = fixed_adapter(audio);
        endpoint.project_session = 7;
        dsp.install_generator_endpoint(9, 70, 700, 1, endpoint, test_pdc_delay());

        let (retired_midi_tx, mut retired_midi_rx) = RingBuffer::new(4);
        let (midi_event_tx, mut midi_event_rx) = RingBuffer::new(4);
        dsp.retired_midi_inputs = Some(retired_midi_tx);
        dsp.midi_input_route_events = Some(midi_event_tx);
        let (mut sender, receiver) = crate::midi_device::test_input_mailbox(8, 33);
        let stamp = MidiGeneratorRouteStamp {
            project_session: 7,
            channel_id: 9,
            endpoint_id: 70,
            plugin_instance_id: 700,
            slot: None,
        };
        let prepared = PreparedMidiInputRoute::new(receiver, stamp).unwrap();
        dsp.install_midi_input(5, prepared);
        assert!(matches!(
            midi_event_rx.pop().unwrap(),
            MidiInputRouteEvent::Installed {
                route_id: 5,
                connection_epoch: 33,
                success: true,
                ..
            }
        ));

        sender.send(1_000, &[0x90, 60, 100]);
        sender.send(2_000, &[0x80, 60, 0]);
        let monitor = dsp.service_midi_input(10_000, 0, 32, 1).unwrap();
        assert_eq!(monitor.generator_index, 0);
        let endpoint = &dsp.generator_endpoints[0].as_ref().unwrap().endpoint;
        assert_eq!(endpoint.events.scratch.live.pending_len, 1);
        assert_eq!(endpoint.events.scratch.live.pending[0].sample_offset, 0);
        assert!(dsp.midi_input.as_ref().unwrap().pending_future.is_some());

        let mut left = [0.0; 32];
        let mut right = [0.0; 32];
        let _ = dsp.generator_endpoints[0]
            .as_mut()
            .unwrap()
            .endpoint
            .process_generator(1, 32, &mut left, &mut right);
        let _ = dsp.service_midi_input(10_032, 32, 32, 1).unwrap();
        let endpoint = &dsp.generator_endpoints[0].as_ref().unwrap().endpoint;
        assert_eq!(endpoint.events.scratch.live.pending_len, 1);
        assert_eq!(endpoint.events.scratch.live.pending[0].sample_offset, 16);

        dsp.remove_midi_input(5);
        assert!(matches!(
            midi_event_rx.pop().unwrap(),
            MidiInputRouteEvent::Removed {
                route_id: 5,
                removed: true,
            }
        ));
        drop(retired_midi_rx.pop().unwrap());
        assert!(dsp.midi_input.is_none());

        dsp.remove_generator_endpoint(9);
        drop(retired_endpoint_rx.pop().unwrap());
        drop(dsp);
        guard.shutdown();
        drop(control);
    }

    #[test]
    fn midi_recording_mirrors_only_staged_note_edges_at_exact_callback_frames() {
        let mut fixture = MidiRecordingAudioFixture::new();
        let stamp = fixture.record_stamp(101);
        let (endpoint, mut record_control) = prepare_midi_recording(stamp, 16).unwrap();
        fixture.dsp.start_midi_recording(endpoint);
        let start = MidiRecordClockAnchor {
            device_frame: 1_000,
            timeline_frame: 2_000,
            transport_epoch: 9,
            loop_count: 0,
        };
        fixture
            .dsp
            .service_midi_recording_boundary(start, true, true);
        let started = fixture.recording_events.pop().unwrap();
        assert!(matches!(
            started,
            MidiRecordingEndpointEvent::Started {
                stamp: observed,
                start: observed_start,
                success: true,
                returned_endpoint: None,
            } if observed == stamp && observed_start == start
        ));
        assert!(record_control.set_start_anchor(start));

        fixture.sender.send(1_000, &[0x90, 60, 100]);
        fixture.sender.send(1_500, &[0x80, 60, 7]);
        let monitor = fixture.dsp.service_midi_input(1_000, 2_000, 64, 9).unwrap();
        assert_eq!(monitor.generator_index, 0);
        let staged = &fixture.dsp.generator_endpoints[0]
            .as_ref()
            .unwrap()
            .endpoint
            .events
            .scratch
            .live;
        assert_eq!(staged.pending_len, 2);
        assert_eq!(staged.pending[0].sample_offset, 0);
        assert_eq!(staged.pending[1].sample_offset, 24);
        let drain = record_control.drain(16);
        assert_eq!(drain.drained_packets, 2);
        assert_eq!(drain.completed_notes, 1);
        let note = record_control.completed_notes()[0];
        assert_eq!(note.start_device_frame, 1_000);
        assert_eq!(note.end_device_frame, 1_024);
        assert_eq!(note.start_timeline_frame, 2_000);
        assert_eq!(note.end_timeline_frame, 2_024);

        let stop = MidiRecordClockAnchor {
            device_frame: 1_064,
            timeline_frame: 2_064,
            transport_epoch: 9,
            loop_count: 0,
        };
        fixture.dsp.set_callback_transport_boundary(stop, true);
        fixture.dsp.stop_midi_recording(stamp.session_id);
        let returned_endpoint = match fixture.recording_events.pop().unwrap() {
            MidiRecordingEndpointEvent::Stopped {
                requested_session_id,
                stamp: Some(observed),
                stop: observed_stop,
                returned_endpoint: Some(endpoint),
            } => {
                assert_eq!(requested_session_id, stamp.session_id);
                assert_eq!(observed, stamp);
                assert_eq!(observed_stop, stop);
                endpoint
            }
            event => panic!("unexpected MIDI recording stop receipt: {event:?}"),
        };
        assert_eq!(
            record_control
                .finish(&returned_endpoint, stop)
                .invalid_reason,
            None
        );
        drop(returned_endpoint);
        fixture.finish();
    }

    #[test]
    fn paused_midi_audition_rejects_start_and_never_enters_record_ring() {
        let mut fixture = MidiRecordingAudioFixture::new();
        let stamp = fixture.record_stamp(102);
        let (endpoint, mut record_control) = prepare_midi_recording(stamp, 8).unwrap();
        fixture.dsp.start_midi_recording(endpoint);
        let boundary = MidiRecordClockAnchor {
            device_frame: 4_000,
            timeline_frame: 8_000,
            transport_epoch: 9,
            loop_count: 0,
        };
        fixture
            .dsp
            .service_midi_recording_boundary(boundary, false, true);
        let returned_endpoint = match fixture.recording_events.pop().unwrap() {
            MidiRecordingEndpointEvent::Started {
                stamp: observed,
                start,
                success: false,
                returned_endpoint: Some(endpoint),
            } => {
                assert_eq!(observed, stamp);
                assert_eq!(start, boundary);
                endpoint
            }
            event => panic!("unexpected paused MIDI recording receipt: {event:?}"),
        };
        assert_eq!(
            returned_endpoint.status().invalid_reason,
            Some(MidiTakeInvalidReason::TransportNotPlaying)
        );

        fixture.sender.send(1_000, &[0x90, 60, 100]);
        assert!(
            fixture
                .dsp
                .service_midi_input(4_000, 8_000, 64, 9)
                .is_some()
        );
        assert_eq!(record_control.drain(8).drained_packets, 0);
        drop(returned_endpoint);
        fixture.finish();
    }

    #[test]
    fn midi_recording_start_rejects_an_input_route_still_in_panic_recovery() {
        let mut fixture = MidiRecordingAudioFixture::new();
        let stamp = fixture.record_stamp(109);
        let (endpoint, _record_control) = prepare_midi_recording(stamp, 8).unwrap();
        fixture.dsp.midi_input.as_mut().unwrap().discard_until_empty = true;
        fixture.dsp.start_midi_recording(endpoint);
        let boundary = MidiRecordClockAnchor {
            device_frame: 5_000,
            timeline_frame: 9_000,
            transport_epoch: 9,
            loop_count: 0,
        };
        fixture
            .dsp
            .service_midi_recording_boundary(boundary, true, true);
        let endpoint = match fixture.recording_events.pop().unwrap() {
            MidiRecordingEndpointEvent::Started {
                success: false,
                returned_endpoint: Some(endpoint),
                ..
            } => endpoint,
            event => panic!("unexpected panic-recovery start receipt: {event:?}"),
        };
        assert_eq!(
            endpoint.status().invalid_reason,
            Some(MidiTakeInvalidReason::InputMustPreserveOverflow)
        );
        drop(endpoint);
        fixture.finish();
    }

    #[test]
    fn midi_recording_start_rejects_a_route_that_has_not_observed_the_transport_epoch() {
        let mut fixture = MidiRecordingAudioFixture::new();
        let stamp = fixture.record_stamp(110);
        let (endpoint, mut record_control) = prepare_midi_recording(stamp, 8).unwrap();
        fixture
            .dsp
            .midi_input
            .as_mut()
            .unwrap()
            .observed_transport_epoch = 8;
        fixture.dsp.start_midi_recording(endpoint);
        let boundary = MidiRecordClockAnchor {
            device_frame: 6_000,
            timeline_frame: 10_000,
            transport_epoch: 9,
            loop_count: 0,
        };
        fixture
            .dsp
            .service_midi_recording_boundary(boundary, true, true);
        let endpoint = match fixture.recording_events.pop().unwrap() {
            MidiRecordingEndpointEvent::Started {
                success: false,
                returned_endpoint: Some(endpoint),
                ..
            } => endpoint,
            event => panic!("unexpected stale-route-epoch start receipt: {event:?}"),
        };
        assert_eq!(
            endpoint.status().invalid_reason,
            Some(MidiTakeInvalidReason::TransportEpochChanged)
        );
        fixture.sender.send(1_000, &[0x90, 60, 100]);
        let _ = fixture
            .dsp
            .service_midi_input(6_000, 10_000, 64, 9)
            .unwrap();
        assert_eq!(record_control.drain(8).drained_packets, 0);
        drop(endpoint);
        fixture.finish();
    }

    #[test]
    fn midi_recording_epoch_change_seals_take_until_exact_stop_returns_endpoint() {
        let mut fixture = MidiRecordingAudioFixture::new();
        let stamp = fixture.record_stamp(103);
        let (endpoint, mut record_control) = prepare_midi_recording(stamp, 8).unwrap();
        fixture.dsp.start_midi_recording(endpoint);
        let start = MidiRecordClockAnchor {
            device_frame: 10_000,
            timeline_frame: 20_000,
            transport_epoch: 9,
            loop_count: 0,
        };
        fixture
            .dsp
            .service_midi_recording_boundary(start, true, true);
        assert!(matches!(
            fixture.recording_events.pop().unwrap(),
            MidiRecordingEndpointEvent::Started { success: true, .. }
        ));
        assert!(record_control.set_start_anchor(start));

        let discontinuity = MidiRecordClockAnchor {
            device_frame: 10_128,
            timeline_frame: 3_000,
            transport_epoch: 10,
            loop_count: 1,
        };
        fixture
            .dsp
            .service_midi_recording_boundary(discontinuity, true, true);
        assert_eq!(
            record_control.status().invalid_reason,
            Some(MidiTakeInvalidReason::TransportEpochChanged)
        );
        assert!(fixture.dsp.midi_recording.is_some());

        fixture
            .dsp
            .set_callback_transport_boundary(discontinuity, true);
        fixture.dsp.stop_midi_recording(stamp.session_id);
        let endpoint = match fixture.recording_events.pop().unwrap() {
            MidiRecordingEndpointEvent::Stopped {
                returned_endpoint: Some(endpoint),
                ..
            } => endpoint,
            event => panic!("unexpected epoch-stop receipt: {event:?}"),
        };
        let _ = record_control.finish(&endpoint, discontinuity);
        assert_eq!(
            record_control.status().invalid_reason,
            Some(MidiTakeInvalidReason::TransportEpochChanged)
        );
        drop(endpoint);
        fixture.finish();
    }

    #[test]
    fn midi_recording_observes_os_rejection_counter_from_start_baseline() {
        let mut fixture = MidiRecordingAudioFixture::new();
        let stamp = fixture.record_stamp(104);
        let (endpoint, mut record_control) = prepare_midi_recording(stamp, 8).unwrap();
        fixture.dsp.start_midi_recording(endpoint);
        let start = MidiRecordClockAnchor {
            device_frame: 12_000,
            timeline_frame: 22_000,
            transport_epoch: 9,
            loop_count: 0,
        };
        fixture
            .dsp
            .service_midi_recording_boundary(start, true, true);
        assert!(matches!(
            fixture.recording_events.pop().unwrap(),
            MidiRecordingEndpointEvent::Started { success: true, .. }
        ));
        assert!(record_control.set_start_anchor(start));

        fixture.sender.send(2_000, &[0xf8]);
        let next = MidiRecordClockAnchor {
            device_frame: 12_064,
            timeline_frame: 22_064,
            ..start
        };
        // Stop is processed before the next render boundary. It must still sample input counters
        // so a just-before-stop backend rejection cannot produce a falsely valid take.
        fixture.dsp.set_callback_transport_boundary(next, true);
        fixture.dsp.stop_midi_recording(stamp.session_id);
        let endpoint = match fixture.recording_events.pop().unwrap() {
            MidiRecordingEndpointEvent::Stopped {
                returned_endpoint: Some(endpoint),
                ..
            } => endpoint,
            event => panic!("unexpected rejected-input stop receipt: {event:?}"),
        };
        let _ = record_control.finish(&endpoint, next);
        assert_eq!(
            record_control.status().invalid_reason,
            Some(MidiTakeInvalidReason::InputMessageRejected)
        );
        drop(endpoint);
        fixture.finish();
    }

    #[test]
    fn midi_recording_observes_os_noncritical_drop_counter_from_start_baseline() {
        let mut fixture = MidiRecordingAudioFixture::new();
        let stamp = fixture.record_stamp(108);
        let (endpoint, mut record_control) = prepare_midi_recording(stamp, 8).unwrap();
        fixture.dsp.start_midi_recording(endpoint);
        let start = MidiRecordClockAnchor {
            device_frame: 13_000,
            timeline_frame: 23_000,
            transport_epoch: 9,
            loop_count: 0,
        };
        fixture
            .dsp
            .service_midi_recording_boundary(start, true, true);
        assert!(matches!(
            fixture.recording_events.pop().unwrap(),
            MidiRecordingEndpointEvent::Started { success: true, .. }
        ));
        assert!(record_control.set_start_anchor(start));

        for value in 0..33_u8 {
            fixture.sender.send(u64::from(value), &[0xb0, 7, value]);
        }
        let next = MidiRecordClockAnchor {
            device_frame: 13_064,
            timeline_frame: 23_064,
            ..start
        };
        fixture
            .dsp
            .service_midi_recording_boundary(next, true, true);
        assert_eq!(
            record_control.status().invalid_reason,
            Some(MidiTakeInvalidReason::InputEventDrop)
        );

        fixture.dsp.set_callback_transport_boundary(next, true);
        fixture.dsp.stop_midi_recording(stamp.session_id);
        let endpoint = match fixture.recording_events.pop().unwrap() {
            MidiRecordingEndpointEvent::Stopped {
                returned_endpoint: Some(endpoint),
                ..
            } => endpoint,
            event => panic!("unexpected dropped-input stop receipt: {event:?}"),
        };
        let _ = record_control.finish(&endpoint, next);
        drop(endpoint);
        fixture.finish();
    }

    #[test]
    fn midi_recording_ring_full_writes_zero_of_an_audible_live_batch() {
        let mut fixture = MidiRecordingAudioFixture::new();
        let stamp = fixture.record_stamp(105);
        let (endpoint, mut record_control) = prepare_midi_recording(stamp, 2).unwrap();
        fixture.dsp.start_midi_recording(endpoint);
        let start = MidiRecordClockAnchor {
            device_frame: 14_000,
            timeline_frame: 24_000,
            transport_epoch: 9,
            loop_count: 0,
        };
        fixture
            .dsp
            .service_midi_recording_boundary(start, true, true);
        assert!(matches!(
            fixture.recording_events.pop().unwrap(),
            MidiRecordingEndpointEvent::Started { success: true, .. }
        ));
        assert!(record_control.set_start_anchor(start));

        fixture.sender.send(1_000, &[0x90, 60, 100]);
        fixture.sender.send(1_100, &[0x80, 60, 0]);
        fixture.sender.send(1_200, &[0x90, 64, 100]);
        assert!(
            fixture
                .dsp
                .service_midi_input(14_000, 24_000, 64, 9)
                .is_some()
        );
        assert_eq!(
            fixture.dsp.generator_endpoints[0]
                .as_ref()
                .unwrap()
                .endpoint
                .events
                .scratch
                .live
                .pending_len,
            3
        );
        assert_eq!(record_control.drain(8).drained_packets, 0);
        assert_eq!(
            record_control.status().invalid_reason,
            Some(MidiTakeInvalidReason::MirrorRingFull)
        );

        let stop = MidiRecordClockAnchor {
            device_frame: 14_064,
            timeline_frame: 24_064,
            ..start
        };
        fixture.dsp.set_callback_transport_boundary(stop, true);
        fixture.dsp.stop_midi_recording(stamp.session_id);
        let endpoint = match fixture.recording_events.pop().unwrap() {
            MidiRecordingEndpointEvent::Stopped {
                returned_endpoint: Some(endpoint),
                ..
            } => endpoint,
            event => panic!("unexpected full-ring stop receipt: {event:?}"),
        };
        let report = record_control.finish(&endpoint, stop);
        assert_eq!(report.completed_notes, 0);
        assert_eq!(
            report.invalid_reason,
            Some(MidiTakeInvalidReason::MirrorRingFull)
        );
        drop(endpoint);
        fixture.finish();
    }

    #[test]
    fn midi_recording_future_retained_edge_is_mirrored_once_in_its_segment() {
        let mut fixture = MidiRecordingAudioFixture::new();
        let stamp = fixture.record_stamp(107);
        let (endpoint, mut record_control) = prepare_midi_recording(stamp, 8).unwrap();
        fixture.dsp.start_midi_recording(endpoint);
        let start = MidiRecordClockAnchor {
            device_frame: 18_000,
            timeline_frame: 28_000,
            transport_epoch: 9,
            loop_count: 0,
        };
        fixture
            .dsp
            .service_midi_recording_boundary(start, true, true);
        assert!(matches!(
            fixture.recording_events.pop().unwrap(),
            MidiRecordingEndpointEvent::Started { success: true, .. }
        ));
        assert!(record_control.set_start_anchor(start));

        fixture.sender.send(1_000, &[0x90, 60, 100]);
        fixture.sender.send(2_500, &[0x80, 60, 0]);
        let _ = fixture
            .dsp
            .service_midi_input(18_000, 28_000, 64, 9)
            .unwrap();
        assert!(
            fixture
                .dsp
                .midi_input
                .as_ref()
                .unwrap()
                .pending_future
                .is_some()
        );
        assert_eq!(record_control.drain(8).drained_packets, 1);
        assert!(record_control.completed_notes().is_empty());

        let mut left = [0.0; 64];
        let mut right = [0.0; 64];
        let _ = fixture.dsp.generator_endpoints[0]
            .as_mut()
            .unwrap()
            .endpoint
            .process_generator(9, 64, &mut left, &mut right);
        let next = MidiRecordClockAnchor {
            device_frame: 18_064,
            timeline_frame: 28_064,
            ..start
        };
        fixture
            .dsp
            .service_midi_recording_boundary(next, true, true);
        let _ = fixture
            .dsp
            .service_midi_input(18_064, 28_064, 64, 9)
            .unwrap();
        assert!(
            fixture
                .dsp
                .midi_input
                .as_ref()
                .unwrap()
                .pending_future
                .is_none()
        );
        let drain = record_control.drain(8);
        assert_eq!(drain.drained_packets, 1);
        assert_eq!(drain.completed_notes, 1);
        assert_eq!(record_control.completed_notes().len(), 1);
        assert_eq!(record_control.completed_notes()[0].end_device_frame, 18_072);
        assert_eq!(
            record_control.completed_notes()[0].end_timeline_frame,
            28_072
        );

        let stop = MidiRecordClockAnchor {
            device_frame: 18_128,
            timeline_frame: 28_128,
            ..start
        };
        fixture.dsp.set_callback_transport_boundary(stop, true);
        fixture.dsp.stop_midi_recording(stamp.session_id);
        let endpoint = match fixture.recording_events.pop().unwrap() {
            MidiRecordingEndpointEvent::Stopped {
                returned_endpoint: Some(endpoint),
                ..
            } => endpoint,
            event => panic!("unexpected future-edge stop receipt: {event:?}"),
        };
        assert_eq!(record_control.finish(&endpoint, stop).invalid_reason, None);
        drop(endpoint);
        fixture.finish();
    }

    #[test]
    fn midi_timestamp_regression_seals_recording_but_audition_batch_still_stages() {
        let mut fixture = MidiRecordingAudioFixture::new();
        let stamp = fixture.record_stamp(106);
        let (endpoint, mut record_control) = prepare_midi_recording(stamp, 8).unwrap();
        fixture.dsp.start_midi_recording(endpoint);
        let start = MidiRecordClockAnchor {
            device_frame: 16_000,
            timeline_frame: 26_000,
            transport_epoch: 9,
            loop_count: 0,
        };
        fixture
            .dsp
            .service_midi_recording_boundary(start, true, true);
        assert!(matches!(
            fixture.recording_events.pop().unwrap(),
            MidiRecordingEndpointEvent::Started { success: true, .. }
        ));
        assert!(record_control.set_start_anchor(start));

        fixture.sender.send(2_000, &[0x90, 60, 100]);
        fixture.sender.send(1_500, &[0x80, 60, 0]);
        assert!(
            fixture
                .dsp
                .service_midi_input(16_000, 26_000, 64, 9)
                .is_some()
        );
        assert_eq!(
            fixture.dsp.generator_endpoints[0]
                .as_ref()
                .unwrap()
                .endpoint
                .events
                .scratch
                .live
                .pending_len,
            2
        );
        assert_eq!(record_control.drain(8).drained_packets, 0);
        assert_eq!(
            record_control.status().invalid_reason,
            Some(MidiTakeInvalidReason::TimestampRegression)
        );

        let stop = MidiRecordClockAnchor {
            device_frame: 16_064,
            timeline_frame: 26_064,
            ..start
        };
        fixture.dsp.set_callback_transport_boundary(stop, true);
        fixture.dsp.stop_midi_recording(stamp.session_id);
        let endpoint = match fixture.recording_events.pop().unwrap() {
            MidiRecordingEndpointEvent::Stopped {
                returned_endpoint: Some(endpoint),
                ..
            } => endpoint,
            event => panic!("unexpected timestamp-regression stop receipt: {event:?}"),
        };
        let _ = record_control.finish(&endpoint, stop);
        drop(endpoint);
        fixture.finish();
    }

    #[test]
    fn paused_midi_monitor_stays_audible_without_new_events_and_freezes_unrelated_state() {
        let target_last_midi = Arc::new(AtomicU32::new(0));
        let target = spawn_identified_mock_instrument(
            700,
            0.25,
            Arc::clone(&target_last_midi),
            Arc::new(AtomicU32::new(0)),
        );
        let unrelated = spawn_identified_mock_instrument(
            701,
            0.25,
            Arc::new(AtomicU32::new(0)),
            Arc::new(AtomicU32::new(0)),
        );
        let PluginChain {
            audio: target_audio,
            control: target_control,
            guard: target_guard,
        } = target;
        let PluginChain {
            audio: unrelated_audio,
            control: unrelated_control,
            guard: unrelated_guard,
        } = unrelated;
        let (retired_endpoint_tx, mut retired_endpoint_rx) = RingBuffer::new(8);
        let (insert_event_tx, _insert_event_rx) = RingBuffer::new(4);
        let (generator_event_tx, _generator_event_rx) = RingBuffer::new(8);
        let mut dsp = DspState::new_with_endpoint_io(
            48_000.0,
            retired_endpoint_tx,
            insert_event_tx,
            generator_event_tx,
        );
        let mut target_endpoint = fixed_adapter(target_audio);
        target_endpoint.project_session = 7;
        let mut unrelated_endpoint = fixed_adapter(unrelated_audio);
        unrelated_endpoint.project_session = 7;
        dsp.install_generator_endpoint(9, 70, 700, 1, target_endpoint, test_pdc_delay());
        dsp.install_generator_endpoint(10, 71, 701, 2, unrelated_endpoint, test_pdc_delay());
        let unrelated_callbacks = dsp.generator_endpoints[1]
            .as_ref()
            .unwrap()
            .endpoint
            .stats()
            .callbacks;
        dsp.voices[0] = Voice {
            active: true,
            phase: 0.25,
            phase_step: 0.01,
            envelope: 1.0,
            ..Voice::default()
        };
        dsp.audio_voices[0].active = true;
        dsp.audio_voices[0].source_position = 12.5;
        let native_phase = dsp.voices[0].phase;
        let audio_position = dsp.audio_voices[0].source_position;
        let raw_delay = dsp.pdc_raw_track_delays[0].current_delay_samples();

        let (retired_midi_tx, mut retired_midi_rx) = RingBuffer::new(4);
        let (midi_event_tx, _midi_event_rx) = RingBuffer::new(4);
        dsp.retired_midi_inputs = Some(retired_midi_tx);
        dsp.midi_input_route_events = Some(midi_event_tx);
        let (mut sender, receiver) = crate::midi_device::test_input_mailbox(8, 33);
        let stamp = MidiGeneratorRouteStamp {
            project_session: 7,
            channel_id: 9,
            endpoint_id: 70,
            plugin_instance_id: 700,
            slot: None,
        };
        dsp.install_midi_input(5, PreparedMidiInputRoute::new(receiver, stamp).unwrap());
        sender.send(1_000, &[0x90, 60, 100]);

        let status = AudioStatus::default();
        let mailbox = TransportMailbox::default();
        let mut transport = RealtimeTransport::default();
        let mut audible_callbacks = 0;
        let audible_deadline = Instant::now() + Duration::from_secs(2);
        let mut callback = 0_usize;
        while audible_callbacks < 2 && Instant::now() < audible_deadline {
            let mut peak = 0.0_f32;
            render_transport_chunk(
                &mut dsp,
                &status,
                &mailbox,
                &mut transport,
                DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES,
                |_, rendered| {
                    peak = rendered.iter().fold(peak, |peak, frame| {
                        peak.max(frame[0].abs()).max(frame[1].abs())
                    });
                },
            );
            if peak > 0.001 {
                audible_callbacks += 1;
            }
            if callback == 0 {
                wait_until(|| target_last_midi.load(Ordering::Acquire) != 0);
            }
            callback += 1;
            thread::sleep(Duration::from_millis(1));
        }
        assert!(
            audible_callbacks >= 2,
            "held MIDI note must remain audibly monitored"
        );
        assert_eq!(transport.timeline_frame, 0);
        assert_eq!(dsp.voices[0].phase, native_phase);
        assert_eq!(dsp.audio_voices[0].source_position, audio_position);
        assert_eq!(
            dsp.pdc_raw_track_delays[0].current_delay_samples(),
            raw_delay
        );
        assert_eq!(
            dsp.generator_endpoints[1]
                .as_ref()
                .unwrap()
                .endpoint
                .stats()
                .callbacks,
            unrelated_callbacks
        );

        dsp.remove_midi_input(5);
        drop(retired_midi_rx.pop().unwrap());
        dsp.remove_generator_endpoint(9);
        dsp.remove_generator_endpoint(10);
        drop(retired_endpoint_rx.pop().unwrap());
        drop(retired_endpoint_rx.pop().unwrap());
        drop(dsp);
        target_guard.shutdown();
        unrelated_guard.shutdown();
        drop((target_control, unrelated_control));
    }

    #[test]
    fn deferred_partial_q_midi_safety_runs_through_the_following_quantum() {
        let chain = spawn_identified_mock_instrument(
            700,
            0.0,
            Arc::new(AtomicU32::new(0)),
            Arc::new(AtomicU32::new(0)),
        );
        let PluginChain {
            audio,
            control,
            guard,
        } = chain;
        let (retired_endpoint_tx, mut retired_endpoint_rx) = RingBuffer::new(4);
        let (insert_event_tx, _insert_event_rx) = RingBuffer::new(4);
        let (generator_event_tx, _generator_event_rx) = RingBuffer::new(4);
        let mut dsp = DspState::new_with_endpoint_io(
            48_000.0,
            retired_endpoint_tx,
            insert_event_tx,
            generator_event_tx,
        );
        let mut endpoint = fixed_adapter(audio);
        endpoint.project_session = 7;
        let mut left = [0.0; 64];
        let mut right = [0.0; 64];
        let _ = endpoint.process_generator(1, 64, &mut left, &mut right);
        endpoint.partial_quantum_usage.system = 1;
        endpoint.partial_quantum_usage.total = 1;
        dsp.install_generator_endpoint(9, 70, 700, 1, endpoint, test_pdc_delay());
        let stamp = MidiGeneratorRouteStamp {
            project_session: 7,
            channel_id: 9,
            endpoint_id: 70,
            plugin_instance_id: 700,
            slot: None,
        };
        dsp.panic_exact_midi_destination(stamp);
        assert_eq!(dsp.paused_midi_safety[0].unwrap().remaining_frames, 192);
        assert_eq!(dsp.service_paused_midi_safety(63, 0), 1);
        assert_eq!(dsp.paused_midi_safety[0].unwrap().remaining_frames, 129);
        assert_eq!(dsp.service_paused_midi_safety(1, 0), 1);
        assert_eq!(dsp.paused_midi_safety[0].unwrap().remaining_frames, 128);
        assert_eq!(dsp.service_paused_midi_safety(127, 0), 1);
        assert_eq!(dsp.paused_midi_safety[0].unwrap().remaining_frames, 1);
        assert_eq!(dsp.service_paused_midi_safety(1, 0), 1);
        assert!(dsp.paused_midi_safety[0].is_none());

        dsp.remove_generator_endpoint(9);
        drop(retired_endpoint_rx.pop().unwrap());
        drop(dsp);
        guard.shutdown();
        drop(control);
    }

    #[test]
    fn replacement_monitor_never_clears_another_generators_safety_service() {
        let first = spawn_identified_mock_instrument(
            700,
            0.0,
            Arc::new(AtomicU32::new(0)),
            Arc::new(AtomicU32::new(0)),
        );
        let second = spawn_identified_mock_instrument(
            701,
            0.0,
            Arc::new(AtomicU32::new(0)),
            Arc::new(AtomicU32::new(0)),
        );
        let PluginChain {
            audio: first_audio,
            control: first_control,
            guard: first_guard,
        } = first;
        let PluginChain {
            audio: second_audio,
            control: second_control,
            guard: second_guard,
        } = second;
        let (retired_endpoint_tx, mut retired_endpoint_rx) = RingBuffer::new(8);
        let (insert_event_tx, _insert_event_rx) = RingBuffer::new(4);
        let (generator_event_tx, _generator_event_rx) = RingBuffer::new(8);
        let mut dsp = DspState::new_with_endpoint_io(
            48_000.0,
            retired_endpoint_tx,
            insert_event_tx,
            generator_event_tx,
        );
        let mut first_endpoint = fixed_adapter(first_audio);
        first_endpoint.project_session = 7;
        let mut second_endpoint = fixed_adapter(second_audio);
        second_endpoint.project_session = 7;
        dsp.install_generator_endpoint(9, 70, 700, 1, first_endpoint, test_pdc_delay());
        dsp.install_generator_endpoint(10, 71, 701, 2, second_endpoint, test_pdc_delay());
        let first_stamp = MidiGeneratorRouteStamp {
            project_session: 7,
            channel_id: 9,
            endpoint_id: 70,
            plugin_instance_id: 700,
            slot: None,
        };
        dsp.panic_exact_midi_destination(first_stamp);
        assert_eq!(dsp.paused_midi_safety[0].unwrap().remaining_frames, 128);

        let (retired_midi_tx, mut retired_midi_rx) = RingBuffer::new(4);
        let (midi_event_tx, _midi_event_rx) = RingBuffer::new(4);
        dsp.retired_midi_inputs = Some(retired_midi_tx);
        dsp.midi_input_route_events = Some(midi_event_tx);
        let (_sender, receiver) = crate::midi_device::test_input_mailbox(4, 44);
        let second_stamp = MidiGeneratorRouteStamp {
            project_session: 7,
            channel_id: 10,
            endpoint_id: 71,
            plugin_instance_id: 701,
            slot: None,
        };
        dsp.install_midi_input(
            6,
            PreparedMidiInputRoute::new(receiver, second_stamp).unwrap(),
        );
        let monitor = dsp.service_midi_input(1_000, 0, 64, 1).unwrap();
        assert_eq!(monitor.generator_index, 1);
        dsp.render_paused_midi_monitor(&AudioStatus::default(), 64, monitor);
        assert_eq!(dsp.paused_midi_safety[0].unwrap().remaining_frames, 128);
        assert_eq!(dsp.service_paused_midi_safety(64, 1_u64 << 1), 1);
        assert_eq!(dsp.paused_midi_safety[0].unwrap().remaining_frames, 64);
        assert_eq!(dsp.service_paused_midi_safety(64, 1_u64 << 1), 1);
        assert!(dsp.paused_midi_safety[0].is_none());

        dsp.remove_midi_input(6);
        drop(retired_midi_rx.pop().unwrap());
        dsp.remove_generator_endpoint(9);
        dsp.remove_generator_endpoint(10);
        drop(retired_endpoint_rx.pop().unwrap());
        drop(retired_endpoint_rx.pop().unwrap());
        drop(dsp);
        first_guard.shutdown();
        second_guard.shutdown();
        drop((first_control, second_control));
    }

    #[test]
    fn partial_q_live_edit_reserves_only_its_own_sixteenth_slot() {
        let parameter = Arc::new(AtomicU32::new(0));
        let backend_parameter = Arc::clone(&parameter);
        let chain = PluginChain::spawn_identified_with_backend_factory(
            &[90],
            move || {
                vec![BackendSlot::new(Box::new(MockInstrumentBackend {
                    active: false,
                    amplitude: 0.0,
                    process_delay: Duration::ZERO,
                    last_midi: Arc::new(AtomicU32::new(0)),
                    parameter: backend_parameter,
                }))]
            },
            PluginPrepareConfig {
                sample_rate: 48_000.0,
                max_block_frames: DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES,
            },
        )
        .unwrap();
        let PluginChain {
            audio,
            mut control,
            guard,
        } = chain;
        let mut endpoint = fixed_adapter(audio);
        endpoint.project_session = 7;
        endpoint.partial_quantum_usage = EndpointQuantumClassUsage {
            live: 15,
            total: 15,
            ..EndpointQuantumClassUsage::default()
        };
        let mut output_left = [0.0; 64];
        let mut output_right = [0.0; 64];
        let _ = endpoint.process_generator(1, 64, &mut output_left, &mut output_right);
        assert_eq!(endpoint.input_phase_frames(), 64);

        let route = ParameterEditRoute {
            project_session: 7,
            endpoint: crate::plugin_parameter_edit::ParameterEndpoint {
                kind: ParameterEndpointKind::Insert,
                id: 70,
            },
            instance_id: 90,
            slot: 0,
            parameter_id: 9,
        };
        let mut batch = TimelineEndpointBatchPlan::new_boxed();
        let first = ParameterEditSubmission {
            edit_id: crate::plugin_parameter_edit::ParameterEditId(1),
            route,
            normalized: 0.5,
        };
        assert_eq!(
            DspState::try_admit_plugin_parameter_edit(
                &mut endpoint,
                TimelineEndpointKey::mixer_insert(1, 70),
                false,
                first,
                batch.as_mut(),
            ),
            Ok(true)
        );
        assert_eq!(endpoint.events.scratch.live.pending_len, 1);

        let second = ParameterEditSubmission {
            edit_id: crate::plugin_parameter_edit::ParameterEditId(2),
            normalized: 0.75,
            ..first
        };
        assert_eq!(
            DspState::try_admit_plugin_parameter_edit(
                &mut endpoint,
                TimelineEndpointKey::mixer_insert(1, 70),
                false,
                second,
                batch.as_mut(),
            ),
            Err(CallbackRejectReason::QueueFault)
        );
        assert_eq!(endpoint.events.scratch.live.pending_len, 1);

        let idle_chain = spawn_empty_chain(DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
        let PluginChain {
            audio: idle_audio,
            control: idle_control,
            guard: idle_guard,
        } = idle_chain;
        let mut dsp = DspState::new(48_000.0);
        dsp.admitted_live_edit_endpoint_count = 1;
        dsp.timeline_render_start_frame = 777;
        dsp.pdc_raw_track_delays[0].request_delay(7, 64).unwrap();
        let pdc_delay_before = dsp.pdc_raw_track_delays[0].current_delay_samples();
        dsp.insert_endpoints[1] = Some(InsertEndpointSlot {
            endpoint_id: 70,
            endpoint,
            suppress_output_frames: 0,
        });
        dsp.insert_endpoints[2] = Some(InsertEndpointSlot {
            endpoint_id: 71,
            endpoint: fixed_adapter(idle_audio),
            suppress_output_frames: 0,
        });
        let target_callbacks = dsp.insert_endpoints[1]
            .as_ref()
            .unwrap()
            .endpoint
            .stats()
            .callbacks;
        let idle_callbacks = dsp.insert_endpoints[2]
            .as_ref()
            .unwrap()
            .endpoint
            .stats()
            .callbacks;
        dsp.service_paused_parameter_edits(2_048, 0, 0);
        let target = dsp.insert_endpoints[1].as_ref().unwrap();
        assert_eq!(target.endpoint.stats().callbacks, target_callbacks + 1);
        assert_eq!(target.endpoint.input_phase_frames(), 0);
        assert!(!target.endpoint.has_admitted_live_edit_marker());
        assert_eq!(dsp.admitted_live_edit_endpoint_count, 0);
        assert_eq!(
            dsp.insert_endpoints[2]
                .as_ref()
                .unwrap()
                .endpoint
                .stats()
                .callbacks,
            idle_callbacks
        );
        assert_eq!(dsp.timeline_render_start_frame, 777);
        assert_eq!(dsp.pdc_plan_revision, 0);
        assert_eq!(
            dsp.pdc_raw_track_delays[0].current_delay_samples(),
            pdc_delay_before
        );
        let target_callbacks_after_service = dsp.insert_endpoints[1]
            .as_ref()
            .unwrap()
            .endpoint
            .stats()
            .callbacks;
        dsp.service_paused_parameter_edits(2_048, 0, 0);
        assert_eq!(
            dsp.insert_endpoints[1]
                .as_ref()
                .unwrap()
                .endpoint
                .stats()
                .callbacks,
            target_callbacks_after_service
        );

        let receipt_deadline = Instant::now() + Duration::from_secs(2);
        let receipt = loop {
            if let Some(receipt) = control.try_next_parameter_edit_receipt() {
                break receipt;
            }
            assert!(
                Instant::now() < receipt_deadline,
                "paused parameter edit did not produce a worker receipt"
            );
            thread::yield_now();
        };
        assert!(matches!(
            receipt,
            ParameterEditReceipt::Applied {
                edit_id,
                slot: 0,
                id: 9,
                effective,
                ..
            } if edit_id.get() == 1 && (effective - 0.5).abs() < f32::EPSILON
        ));
        assert_eq!(parameter.load(Ordering::Acquire), 0.5_f32.to_bits());

        let playing = ParameterEditSubmission {
            edit_id: crate::plugin_parameter_edit::ParameterEditId(3),
            normalized: 0.25,
            ..first
        };
        let mut playing_batch = TimelineEndpointBatchPlan::new_boxed();
        assert_eq!(
            DspState::try_admit_plugin_parameter_edit(
                &mut dsp.insert_endpoints[1].as_mut().unwrap().endpoint,
                TimelineEndpointKey::mixer_insert(1, 70),
                false,
                playing,
                playing_batch.as_mut(),
            ),
            Ok(true)
        );
        dsp.admitted_live_edit_endpoint_count = 1;
        assert!(dsp.process_insert_endpoint(1, DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES));
        assert_eq!(dsp.admitted_live_edit_endpoint_count, 0);
        let receipt_deadline = Instant::now() + Duration::from_secs(2);
        let playing_receipt = loop {
            if let Some(receipt) = control.try_next_parameter_edit_receipt() {
                break receipt;
            }
            assert!(Instant::now() < receipt_deadline);
            thread::yield_now();
        };
        assert!(matches!(
            playing_receipt,
            ParameterEditReceipt::Applied {
                edit_id,
                effective,
                ..
            } if edit_id.get() == 3 && (effective - 0.25).abs() < f32::EPSILON
        ));

        let target = dsp.insert_endpoints[1].take().unwrap();
        let idle = dsp.insert_endpoints[2].take().unwrap();
        drop((target.endpoint, idle.endpoint));
        guard.shutdown();
        idle_guard.shutdown();
        drop((control, idle_control));
    }

    fn test_reclaimer() -> TestReclaimer {
        RingBuffer::new(MAX_REGISTERED_AUDIO_ASSETS + 16)
    }

    fn asset_operation(operation_id: u64) -> AudioAssetOperation {
        AudioAssetOperation {
            engine_session: 3,
            generation: 5,
            operation_id,
        }
    }

    fn test_pdc_delay() -> StereoDelayLine {
        StereoDelayLine::new(512).unwrap()
    }

    fn fixed_adapter(endpoint: AudioThreadEndpoint) -> PreparedFixedEndpoint {
        PreparedFixedEndpoint::new(endpoint).unwrap()
    }

    fn compiled_test_timeline() -> Arc<CompiledTimeline> {
        let project = Project::default();
        let tempo_map = TempoMap::from_project(&project, 48_000).unwrap();
        Arc::new(
            CompiledTimeline::from_project(&project, &tempo_map, TimelineCompileOptions::default())
                .unwrap(),
        )
    }

    fn timeline_test_project(length_beats: f32) -> Project {
        Project {
            format_version: 2,
            name: "Audio callback timeline test".into(),
            tempo: 120.0,
            swing: 0.0,
            song_length_beats: length_beats,
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

    fn timeline_test_channel(id: u32, mixer_track: usize) -> Channel {
        Channel {
            id,
            name: format!("Channel {id}"),
            color: [1, 2, 3],
            volume: 1.0,
            pan: 0.0,
            muted: false,
            solo: false,
            mixer_track: crate::model::mixer_track_id_for_runtime_slot(mixer_track.min(31) as u8),
            instrument_plugin_instance_id: None,
            steps: [false; 16],
        }
    }

    fn timeline_pattern_clip(id: u32, length: f32, pattern_id: u32) -> Clip {
        Clip {
            id,
            track: 0,
            start: 0.0,
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
            audio_source_reference: None,
            audio_length_reference: None,
            fade_in_reference: None,
            fade_out_reference: None,
            gain: 1.0,
            fade_in: 0.0,
            fade_out: 0.0,
            muted: false,
        }
    }

    fn frame_as_beat(frame: u64) -> f32 {
        frame as f32 / 24_000.0
    }

    fn frame_as_beat_f64(frame: u64) -> f64 {
        frame as f64 / 24_000.0
    }

    fn push_timeline_automation(
        project: &mut Project,
        id: u64,
        target: AutomationTarget,
        curve: AutomationCurve,
        points: impl IntoIterator<Item = AutomationPoint>,
    ) {
        let mut lane = AutomationLane::new(target);
        lane.set_curve(curve);
        lane.replace_points(points);
        project.automation_lanes.push(ProjectAutomation {
            id,
            name: format!("Automation {id}"),
            lane,
        });
    }

    fn timeline_test_plugin(id: u64) -> PluginInstance {
        PluginInstance {
            id,
            format: PluginFormat::Vst3,
            role: PluginRole::Instrument,
            path: PathBuf::from(format!(r"C:\VST3\Test-{id}.vst3")),
            uid: format!("test-{id}"),
            vendor: "Citrus".into(),
            name: format!("Test {id}"),
            enabled: true,
            bypass: false,
            wet: 1.0,
            parameters: Default::default(),
            opaque_state: Vec::new(),
            runtime_status: PluginRuntimeStatus::Loaded,
        }
    }

    fn install_constant_timeline_voice(dsp: &mut DspState, channel_id: u32, mixer_track: usize) {
        dsp.voices[0] = Voice {
            phase: 0.25,
            phase_step: 0.0,
            envelope: 1.0,
            decay: 1.0,
            active: true,
            mixer_track,
            timeline_note_id: Some(1),
            timeline_channel_id: Some(channel_id),
        };
    }

    fn render_constant_timeline_voice(dsp: &mut DspState, mixer_track: usize, frames: usize) {
        let start = mixer_track * MAX_MIXER_BLOCK_FRAMES;
        dsp.track_block[start..start + frames].fill([0.0; 2]);
        for frame in 0..frames {
            dsp.render_synth_frame(frame);
        }
    }

    fn compile_timeline_test_project(project: &Project) -> Arc<CompiledTimeline> {
        let tempo_map = TempoMap::from_project(project, 48_000).unwrap();
        Arc::new(
            CompiledTimeline::from_project(
                project,
                &tempo_map,
                TimelineCompileOptions {
                    legacy_piano_period_beats: 1.0,
                    ..TimelineCompileOptions::default()
                },
            )
            .unwrap(),
        )
    }

    fn mixer_submix_timeline(first_tap: MixerRouteTap) -> Arc<CompiledTimeline> {
        let mut project = Project::blank();
        let first_track = crate::model::mixer_track_id_for_runtime_slot(1);
        let submix_track = crate::model::mixer_track_id_for_runtime_slot(2);
        let first_route = project
            .mixer_routes
            .iter_mut()
            .find(|route| route.source_mixer_track_id == first_track)
            .unwrap();
        first_route.destination = MixerRouteDestination::MainInput {
            mixer_track_id: submix_track,
        };
        first_route.tap = first_tap;
        compile_timeline_test_project(&project)
    }

    fn mixer_fanout_timeline() -> Arc<CompiledTimeline> {
        let mut project = Project::blank();
        let source = crate::model::mixer_track_id_for_runtime_slot(1);
        let first_destination = crate::model::mixer_track_id_for_runtime_slot(2);
        let second_destination = crate::model::mixer_track_id_for_runtime_slot(3);
        project
            .mixer_routes
            .iter_mut()
            .find(|route| route.source_mixer_track_id == source)
            .expect("default source route")
            .destination = MixerRouteDestination::MainInput {
            mixer_track_id: first_destination,
        };
        project.mixer_routes.push(MixerRoute {
            id: 100,
            runtime_slot: 31,
            source_mixer_track_id: source,
            destination: MixerRouteDestination::MainInput {
                mixer_track_id: second_destination,
            },
            tap: MixerRouteTap::PostFader,
            gain: 1.0,
            enabled: true,
        });
        compile_timeline_test_project(&project)
    }

    fn install_test_timeline(
        controller: &mut TimelineRuntimeController,
        revision: u64,
        timeline: Arc<CompiledTimeline>,
    ) -> u64 {
        let bank = Box::new(
            PreparedMixerGraphDelayBank::new(timeline.mixer_graph(), 512)
                .expect("test mixer graph delay bank"),
        );
        controller
            .install_with_mixer_resources(revision, timeline, bank)
            .unwrap()
    }

    fn install_and_activate_timeline(
        timeline: Arc<CompiledTimeline>,
        revision: u64,
        epoch: u64,
        frame: u64,
        options: TimelineChaseOptions,
    ) -> (TimelineRuntimeController, DspState, AudioStatus) {
        let (mut controller, realtime) = create_timeline_runtime();
        let chase = controller
            .prepare_chase(&timeline, revision, epoch, frame, options)
            .unwrap();
        install_test_timeline(&mut controller, revision, timeline);
        controller.install_chase(chase).unwrap();
        let mut dsp = timeline_test_dsp(realtime);
        assert_eq!(dsp.apply_pending_timeline_commands(), 2);
        let status = AudioStatus::default();
        status.playing.store(true, Ordering::Release);
        dsp.apply_transport_discontinuity(
            &status,
            epoch,
            0,
            frame,
            TransportDiscontinuity::OneShot,
        );
        (controller, dsp, status)
    }

    fn install_and_activate_mixer_graph_timeline(
        timeline: Arc<CompiledTimeline>,
        revision: u64,
        epoch: u64,
    ) -> (TimelineRuntimeController, Box<DspState>, AudioStatus) {
        let (mut controller, realtime) = create_timeline_runtime();
        let chase = controller
            .prepare_chase(
                &timeline,
                revision,
                epoch,
                0,
                TimelineChaseOptions::default(),
            )
            .unwrap();
        let loop_chase = controller
            .prepare_loop_chase(&timeline, revision, 0, TimelineChaseOptions::default())
            .unwrap();
        install_test_timeline(&mut controller, revision, timeline);
        let loop_token = controller.install_loop_chase(loop_chase).unwrap();
        controller.install_chase(chase).unwrap();
        let mut dsp = Box::write(Box::new_uninit(), timeline_test_dsp(realtime));
        assert_eq!(dsp.apply_pending_timeline_commands(), 3);
        let status = AudioStatus::default();
        status.playing.store(true, Ordering::Release);
        let spec = TimelineTransportActivationSpec {
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
        };
        controller.activate_transport(spec, 0).unwrap();
        assert_eq!(dsp.apply_pending_timeline_commands(), 1);
        let ticket = dsp.pending_timeline_transport_activation().unwrap();
        dsp.preflight_timeline_transport_activation(ticket, epoch)
            .unwrap();
        let committed = dsp.commit_timeline_transport_activation(&status, ticket, epoch);
        dsp.publish_timeline_transport_activation(committed);
        assert!(dsp.mixer_graph_was_activated);
        let graph_pdc = dsp
            .graph_pdc_plan
            .as_ref()
            .as_ref()
            .expect("activated graph PDC plan");
        let pdc_fields = status.pdc_fields();
        assert_eq!(pdc_fields.plan_revision, dsp.pdc_plan_revision);
        assert_eq!(
            pdc_fields.reference_latency_samples,
            graph_pdc.master_output_latency_samples()
        );
        assert_eq!(
            pdc_fields.master_latency_samples,
            u32::try_from(
                graph_pdc
                    .node(graph_pdc.master_node_id())
                    .expect("MASTER graph PDC node")
                    .stage_latency_samples
            )
            .unwrap()
        );
        assert_eq!(
            pdc_fields.output_latency_samples,
            graph_pdc.master_output_latency_samples()
        );
        assert_eq!(
            pdc_fields.maximum_delay_samples,
            graph_pdc.maximum_delay_samples()
        );
        (controller, dsp, status)
    }

    fn measured_callback_fixture(
        samples: &[f32],
        channels: u16,
        observation: bool,
    ) -> (
        TimelineRuntimeController,
        Box<DspState>,
        AudioStatus,
        RealtimeTransport,
        MeterReader,
        MeterIdentity,
    ) {
        let timeline = compile_timeline_test_project(&timeline_test_project(4.0));
        let identity = MeterIdentity {
            revision: 1,
            epoch: 2,
            graph_fingerprint: timeline.mixer_graph().fingerprint(),
        };
        let (controller, mut dsp, status) =
            install_and_activate_mixer_graph_timeline(timeline, 1, 2);
        let (publisher, mut reader) = meter_channel();
        reader.poll(Instant::now(), Some(identity), 0, 48_000);
        if observation {
            dsp.meter_publisher = Some(publisher);
        }
        let (mut retired, _reclaimed) = test_reclaimer();
        register_test_asset(&mut dsp, &mut retired, 9001, samples, 48_000, channels);
        dsp.handle(
            AudioCommand::PlayClip {
                clip_id: 9001,
                asset_id: 9001,
                source_frame: 0.0,
                gain: 1.0,
                mixer_track: 1,
            },
            &mut retired,
        );
        dsp.master = 1.0;
        dsp.track_gains[1] = 1.0;
        let transport = RealtimeTransport {
            epoch: 2,
            request: TransportRequest {
                playing: true,
                ..TransportRequest::default()
            },
            ..RealtimeTransport::default()
        };
        (controller, dsp, status, transport, reader, identity)
    }

    #[test]
    fn measured_meters_callback_stereo_levels_clipping_and_silent_bus_are_exact() {
        let samples: Vec<f32> = (0..256)
            .flat_map(|index| {
                if index == 17 {
                    [1.25, -0.5]
                } else {
                    [0.25, -0.125]
                }
            })
            .collect();
        let (_controller, mut dsp, status, mut transport, mut reader, identity) =
            measured_callback_fixture(&samples, 2, true);
        let mailbox = TransportMailbox::default();
        let now = Instant::now();
        let mut output = Vec::new();
        render_transport_chunk(
            &mut dsp,
            &status,
            &mailbox,
            &mut transport,
            128,
            |_, block| output.extend_from_slice(block),
        );
        let readings = reader.poll(now, Some(identity), transport.device_frame, 48_000);
        let track = readings.track(1, crate::model::mixer_track_id_for_runtime_slot(1));
        let master = readings.track(0, crate::mixer_graph::MASTER_MIXER_TRACK_ID);
        assert_eq!(track.peak, [1.25, 0.5]);
        assert_eq!(master.peak, track.peak);
        assert!(track.clipped && master.clipped);
        assert_eq!(output[17], [1.25_f32.tanh(), (-0.5_f32).tanh()]);
        assert!(
            output[17][0] < 1.0,
            "meter is deliberately before existing master protection"
        );
        let silent = readings.track(2, crate::model::mixer_track_id_for_runtime_slot(2));
        assert!(silent.available);
        assert_eq!(silent.peak, [0.0; 2]);
        dsp.track_muted[1] = true;
        render_transport_chunk(
            &mut dsp,
            &status,
            &mailbox,
            &mut transport,
            128,
            |_, block| assert!(block.iter().all(|frame| *frame == [0.0; 2])),
        );
        let readings = reader.poll(now, Some(identity), transport.device_frame, 48_000);
        assert_eq!(
            readings
                .track(1, crate::model::mixer_track_id_for_runtime_slot(1))
                .peak,
            [0.0; 2]
        );
        assert_eq!(
            readings
                .track(0, crate::mixer_graph::MASTER_MIXER_TRACK_ID)
                .peak,
            [0.0; 2]
        );
        assert_eq!(dsp.timeline_missing_assets, 0);
        assert_eq!(dsp.timeline_execution_failures, 0);
    }

    #[test]
    fn measured_meters_callback_mono_source_fader_pan_and_stopped_silence() {
        let samples = [0.5; 256];
        let (_controller, mut dsp, status, mut transport, mut reader, identity) =
            measured_callback_fixture(&samples, 1, true);
        dsp.track_gains[1] = 0.5;
        dsp.track_pans[1] = 0.5;
        dsp.master = 0.5;
        let mailbox = TransportMailbox::default();
        let now = Instant::now();
        render_transport_chunk(&mut dsp, &status, &mailbox, &mut transport, 128, |_, _| {});
        let readings = reader.poll(now, Some(identity), transport.device_frame, 48_000);
        assert_eq!(
            readings
                .track(1, crate::model::mixer_track_id_for_runtime_slot(1))
                .peak,
            [0.125, 0.25]
        );
        assert_eq!(
            readings
                .track(0, crate::mixer_graph::MASTER_MIXER_TRACK_ID)
                .peak,
            [0.0625, 0.125]
        );
        // The paused path leaves the previous track buffers untouched. It must
        // report measured silence, never sample those stale post-fader buffers.
        transport.request.playing = false;
        status.playing.store(false, Ordering::Release);
        render_transport_chunk(
            &mut dsp,
            &status,
            &mailbox,
            &mut transport,
            128,
            |_, block| assert!(block.iter().all(|frame| *frame == [0.0; 2])),
        );
        let readings = reader.poll(now, Some(identity), transport.device_frame, 48_000);
        assert_eq!(
            readings
                .track(1, crate::model::mixer_track_id_for_runtime_slot(1))
                .peak,
            [0.0; 2]
        );
        assert_eq!(
            readings
                .track(0, crate::mixer_graph::MASTER_MIXER_TRACK_ID)
                .peak,
            [0.0; 2]
        );
        assert_eq!(transport.timeline_frame, 128);
    }

    #[test]
    fn measured_meters_observation_preserves_callback_pcm_bits_for_generated_signals() {
        for channels in [1, 2] {
            let samples: Vec<f32> = (0..2048)
                .flat_map(|index| {
                    let left = if index % 127 == 0 {
                        0.9
                    } else {
                        (index as f32 * 0.037).sin() * 0.4
                    };
                    [left, -left * 0.5].into_iter().take(channels as usize)
                })
                .collect();
            let mut rendered = Vec::new();
            for observation in [false, true] {
                let (_controller, mut dsp, status, mut transport, mut reader, identity) =
                    measured_callback_fixture(&samples, channels, observation);
                let mailbox = TransportMailbox::default();
                let now = Instant::now();
                let mut pcm = Vec::new();
                for frames in [1, 31, 96, 128, 257, 511, 1024] {
                    render_transport_chunk(
                        &mut dsp,
                        &status,
                        &mailbox,
                        &mut transport,
                        frames,
                        |_, block| {
                            pcm.extend(
                                block
                                    .iter()
                                    .map(|frame| [frame[0].to_bits(), frame[1].to_bits()]),
                            );
                        },
                    );
                    if observation {
                        let meter =
                            reader.poll(now, Some(identity), transport.device_frame, 48_000);
                        assert!(
                            meter
                                .track(1, crate::model::mixer_track_id_for_runtime_slot(1))
                                .available
                        );
                    }
                }
                assert_eq!(transport.device_frame, 2048);
                assert_eq!(dsp.timeline_execution_failures, 0);
                assert_eq!(dsp.timeline_missing_assets, 0);
                rendered.push(pcm);
            }
            assert_eq!(rendered[0], rendered[1]);
        }
    }

    #[test]
    fn measured_meters_callback_sanitized_invalid_asset_is_silent() {
        let samples: Vec<f32> = (0..128).flat_map(|_| [f32::NAN, f32::INFINITY]).collect();
        let (_controller, mut dsp, status, mut transport, mut reader, identity) =
            measured_callback_fixture(&samples, 2, true);
        render_transport_chunk(
            &mut dsp,
            &status,
            &TransportMailbox::default(),
            &mut transport,
            128,
            |_, block| {
                assert!(block.iter().all(|frame| *frame == [0.0; 2]));
            },
        );
        let reading = reader
            .poll(
                Instant::now(),
                Some(identity),
                transport.device_frame,
                48_000,
            )
            .track(1, crate::model::mixer_track_id_for_runtime_slot(1));
        assert!(reading.available);
        assert_eq!(reading.peak, [0.0; 2]);
        assert!(!reading.clipped);
    }

    #[test]
    fn measured_meters_graph_uses_stable_ids_despite_display_and_runtime_remap() {
        let mut project = timeline_test_project(4.0);
        let id = crate::model::mixer_track_id_for_runtime_slot(1);
        project.mixer_tracks[1].runtime_slot = 2;
        project.mixer_tracks[2].runtime_slot = 1;
        project.mixer_tracks.swap(1, 9);
        let timeline = compile_timeline_test_project(&project);
        let identity = MeterIdentity {
            revision: 7,
            epoch: 3,
            graph_fingerprint: timeline.mixer_graph().fingerprint(),
        };
        let (_controller, mut dsp, status) =
            install_and_activate_mixer_graph_timeline(timeline, 7, 3);
        let (publisher, mut reader) = meter_channel();
        let now = Instant::now();
        reader.poll(now, Some(identity), 0, 48_000);
        dsp.meter_publisher = Some(publisher);
        dsp.voices[0] = Voice {
            phase: 0.25,
            phase_step: 0.0,
            envelope: 0.25,
            decay: 1.0,
            active: true,
            mixer_track: 2,
            ..Voice::default()
        };
        let mut transport = RealtimeTransport {
            epoch: 3,
            request: TransportRequest {
                playing: true,
                ..TransportRequest::default()
            },
            ..RealtimeTransport::default()
        };
        render_transport_chunk(
            &mut dsp,
            &status,
            &TransportMailbox::default(),
            &mut transport,
            8,
            |_, _| {},
        );
        let readings = reader.poll(now, Some(identity), transport.device_frame, 48_000);
        assert!(readings.track(2, id).available);
        assert!(readings.track(2, id).peak[0] > 0.1);
        assert!(!readings.track(1, id).available);
        assert!(!readings.track(9, id).available);
        assert_eq!(dsp.timeline_execution_failures, 0);
    }

    #[test]
    fn measured_meters_engine_invalidating_device_fault_clears_live_readings() {
        let samples = [0.5; 256];
        let (controller, mut dsp, status, mut transport, reader, identity) =
            measured_callback_fixture(&samples, 1, true);
        let mut engine = timeline_test_engine(controller);
        engine.status = Arc::new(status);
        engine.meter_reader = reader;
        let now = Instant::now();
        let expected = Some((identity.revision, identity.graph_fingerprint));
        render_transport_chunk(
            &mut dsp,
            &engine.status,
            &TransportMailbox::default(),
            &mut transport,
            128,
            |_, _| {},
        );
        let id = crate::model::mixer_track_id_for_runtime_slot(1);
        assert!(engine.poll_meters(now, expected).track(1, id).available);
        observe_backend_stream_error(&engine.status, &engine.callback_telemetry, ErrorKind::Xrun);
        assert!(
            engine.poll_meters(now, expected).track(1, id).available,
            "xrun warning does not pretend the device vanished"
        );
        observe_backend_stream_error(
            &engine.status,
            &engine.callback_telemetry,
            ErrorKind::DeviceNotAvailable,
        );
        assert!(!engine.poll_meters(now, expected).track(1, id).available);
    }

    #[test]
    fn measured_meters_unbound_compatibility_graph_is_unavailable() {
        let (publisher, mut reader) = meter_channel();
        let mut dsp = DspState::new(48_000.0);
        dsp.meter_publisher = Some(publisher);
        dsp.track_block[0] = [1.0; 2];
        dsp.publish_meters(1, 1);
        assert!(
            !reader
                .poll(Instant::now(), None, 1, 48_000)
                .track(0, crate::mixer_graph::MASTER_MIXER_TRACK_ID)
                .available
        );
    }

    #[test]
    fn mixer_graph_submix_routes_all_taps_and_mute_solo_gate_every_send() {
        for (revision, tap) in [
            MixerRouteTap::PreEffects,
            MixerRouteTap::PostEffects,
            MixerRouteTap::PostFader,
        ]
        .into_iter()
        .enumerate()
        {
            let timeline = mixer_submix_timeline(tap);
            let (_controller, mut dsp, status) =
                install_and_activate_mixer_graph_timeline(timeline, revision as u64 + 1, 2);
            install_constant_timeline_voice(&mut dsp, 1, 1);
            dsp.render_block(&status, 1);
            assert!(
                dsp.master_block[0][0].abs() > 0.1,
                "{tap:?} must traverse track 1 -> submix 2 -> MASTER"
            );

            dsp.track_muted[1] = true;
            dsp.render_block(&status, 1);
            assert_eq!(
                dsp.master_block[0], [0.0; 2],
                "muting a source must gate its {tap:?} send while advancing silence"
            );
        }

        let timeline = mixer_submix_timeline(MixerRouteTap::PreEffects);
        let (_controller, mut dsp, status) =
            install_and_activate_mixer_graph_timeline(timeline, 10, 2);
        install_constant_timeline_voice(&mut dsp, 1, 1);
        dsp.track_solo[3] = true;
        dsp.render_block(&status, 1);
        assert_eq!(
            dsp.master_block[0], [0.0; 2],
            "a source outside the solo upstream/downstream closure must not leak through pre-FX sends"
        );
    }

    #[test]
    fn paused_mixer_graph_fanout_advances_only_downstream_and_marks_failed_insert() {
        let timeline = mixer_fanout_timeline();
        let target = spawn_identified_mock_instrument(
            700,
            0.25,
            Arc::new(AtomicU32::new(0)),
            Arc::new(AtomicU32::new(0)),
        );
        let unrelated = spawn_identified_mock_instrument(
            701,
            0.25,
            Arc::new(AtomicU32::new(0)),
            Arc::new(AtomicU32::new(0)),
        );
        let first_downstream =
            spawn_identified_mock_latency_transform(802, Arc::new(AtomicU32::new(0)));
        let second_downstream = spawn_identified_empty_chain();
        let unrelated_insert = spawn_identified_empty_chain();
        wait_until(|| target.control.plugin_latency_snapshot().is_some());
        wait_until(|| unrelated.control.plugin_latency_snapshot().is_some());
        wait_until(|| first_downstream.control.plugin_latency_snapshot().is_some());
        wait_until(|| {
            second_downstream
                .control
                .plugin_latency_snapshot()
                .is_some()
        });
        wait_until(|| unrelated_insert.control.plugin_latency_snapshot().is_some());
        let PluginChain {
            audio: target_audio,
            control: target_control,
            guard: target_guard,
        } = target;
        let PluginChain {
            audio: unrelated_audio,
            control: unrelated_control,
            guard: unrelated_guard,
        } = unrelated;
        let PluginChain {
            audio: first_downstream_audio,
            control: first_downstream_control,
            guard: first_downstream_guard,
        } = first_downstream;
        let PluginChain {
            audio: second_downstream_audio,
            control: second_downstream_control,
            guard: second_downstream_guard,
        } = second_downstream;
        let PluginChain {
            audio: unrelated_insert_audio,
            control: unrelated_insert_control,
            guard: unrelated_insert_guard,
        } = unrelated_insert;

        let revision = 61;
        let epoch = 9;
        let (mut controller, realtime) = create_timeline_runtime();
        let chase = controller
            .prepare_chase(
                &timeline,
                revision,
                epoch,
                0,
                TimelineChaseOptions::default(),
            )
            .unwrap();
        let loop_chase = controller
            .prepare_loop_chase(&timeline, revision, 0, TimelineChaseOptions::default())
            .unwrap();
        install_test_timeline(&mut controller, revision, Arc::clone(&timeline));
        let loop_token = controller.install_loop_chase(loop_chase).unwrap();
        controller.install_chase(chase).unwrap();
        let mut dsp = timeline_test_dsp(realtime);
        assert_eq!(dsp.apply_pending_timeline_commands(), 3);
        dsp.install_generator_endpoint(
            1,
            70,
            700,
            1,
            fixed_adapter(target_audio),
            test_pdc_delay(),
        );
        dsp.install_generator_endpoint(
            99,
            71,
            701,
            5,
            fixed_adapter(unrelated_audio),
            test_pdc_delay(),
        );
        dsp.install_insert_endpoint(2, 202, fixed_adapter(first_downstream_audio));
        dsp.install_insert_endpoint(3, 203, fixed_adapter(second_downstream_audio));
        dsp.install_insert_endpoint(4, 204, fixed_adapter(unrelated_insert_audio));

        let status = AudioStatus::default();
        let spec = TimelineTransportActivationSpec {
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
            playing: false,
            mixer_pan_release: TimelineMixerPanRelease::EMPTY,
        };
        controller.activate_transport(spec, 0).unwrap();
        assert_eq!(dsp.apply_pending_timeline_commands(), 1);
        let ticket = dsp.pending_timeline_transport_activation().unwrap();
        dsp.preflight_timeline_transport_activation(ticket, epoch)
            .unwrap();
        let committed = dsp.commit_timeline_transport_activation(&status, ticket, epoch);
        dsp.publish_timeline_transport_activation(committed);

        assert_eq!(dsp.mixer_graph_endpoint_identities.insert_count, 3);
        assert_eq!(dsp.mixer_graph_endpoint_identities.generator_count, 2);
        for runtime_slot in [2, 3, 4] {
            let endpoint = &dsp.insert_endpoints[runtime_slot]
                .as_ref()
                .expect("installed graph insert")
                .endpoint;
            assert_eq!(
                endpoint.expected_latency_revision(),
                endpoint
                    .cached_exact_endpoint_snapshot()
                    .expect("exact insert snapshot")
                    .revision(),
                "non-driven insert {runtime_slot} must attest the activated graph PDC generation"
            );
        }
        for endpoint in dsp.generator_endpoints[..2].iter().flatten() {
            assert_eq!(
                endpoint.endpoint.expected_latency_revision(),
                endpoint
                    .endpoint
                    .cached_exact_endpoint_snapshot()
                    .expect("exact generator snapshot")
                    .revision(),
                "non-driven generator must attest the activated graph PDC generation"
            );
        }

        // Activation deliberately faults the Live lane until its all-notes-off
        // boundary has physically reached the worker. Drain that boundary on
        // the selected endpoint before admitting the test's external note.
        let target_completed = target_control.stats().completed;
        let _ = dsp.generator_endpoints[0]
            .as_mut()
            .unwrap()
            .endpoint
            .process_generator(
                dsp.transport_epoch,
                DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES,
                &mut dsp.plugin_output_left[..DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES],
                &mut dsp.plugin_output_right[..DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES],
            );
        wait_until(|| target_control.stats().completed > target_completed);
        assert!(
            dsp.generator_endpoints[0]
                .as_mut()
                .unwrap()
                .endpoint
                .stage(FrameEvent::midi(0, Some(0), [0x90, 60, 127]))
        );
        dsp.voices[0] = Voice {
            phase: 0.25,
            phase_step: 0.01,
            envelope: 1.0,
            decay: 1.0,
            active: true,
            mixer_track: 4,
            timeline_note_id: None,
            timeline_channel_id: None,
        };
        let (mut retired_assets, _reclaimed_assets) = test_reclaimer();
        register_test_asset(&mut dsp, &mut retired_assets, 901, &[0.2; 512], 48_000, 1);
        dsp.handle(
            AudioCommand::PlayClip {
                clip_id: 901,
                asset_id: 901,
                source_frame: 3.0,
                gain: 1.0,
                mixer_track: 4,
            },
            &mut retired_assets,
        );
        let native_phase = dsp.voices[0].phase;
        let audio_source_position = dsp.audio_voices[0].source_position;
        let executor_frame = dsp.timeline_executor.next_frame();
        let runtime_frame = dsp.timeline_runtime.as_ref().unwrap().next_frame();
        let status_frame = status.timeline_frame.load(Ordering::Acquire);
        let unrelated_generator_callbacks = dsp.generator_endpoints[1]
            .as_ref()
            .unwrap()
            .endpoint
            .stats()
            .callbacks;
        let unrelated_insert_callbacks = dsp.insert_endpoints[4]
            .as_ref()
            .unwrap()
            .endpoint
            .stats()
            .callbacks;
        dsp.pdc_raw_track_delays[4].request_delay(2, 0).unwrap();
        dsp.pdc_raw_track_delays[4].reset();
        assert_eq!(
            dsp.pdc_raw_track_delays[4].process_sample([1.0, 1.0]),
            [0.0; 2]
        );

        let monitor = PausedMidiMonitorRoute {
            generator_index: 0,
            mixer_track: 1,
        };
        let mut generator_mask = 0_u64;
        let mut insert_mask = 0_u32;
        let mut first_peak = 0.0_f32;
        let mut second_peak = 0.0_f32;
        let mut master_peak = 0.0_f32;
        let meter_identity = MeterIdentity {
            revision,
            epoch,
            graph_fingerprint: dsp.mixer_graph_plan.fingerprint(),
        };
        let (publisher, mut meter_reader) = meter_channel();
        meter_reader.poll(Instant::now(), Some(meter_identity), 0, 48_000);
        dsp.meter_publisher = Some(publisher);
        let mut meter_frame = 0;
        let mut measured_monitor_peak = 0.0_f32;
        assert!(!status.playing.load(Ordering::Acquire));

        for _ in 0..24 {
            let target_completed = target_control.stats().completed;
            let first_completed = first_downstream_control.stats().completed;
            let second_completed = second_downstream_control.stats().completed;
            dsp.meter_publisher.as_mut().unwrap().begin_block();
            let progress = dsp.render_paused_midi_monitor_graph(
                &status,
                DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES,
                monitor,
            );
            meter_frame += DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES as u64;
            dsp.publish_meters(meter_frame, DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
            let meters =
                meter_reader.poll(Instant::now(), Some(meter_identity), meter_frame, 48_000);
            let measured = meters.track(2, crate::model::mixer_track_id_for_runtime_slot(2));
            assert!(measured.available);
            measured_monitor_peak = measured_monitor_peak
                .max(measured.peak[0])
                .max(measured.peak[1]);
            assert_eq!(
                meters
                    .track(4, crate::model::mixer_track_id_for_runtime_slot(4))
                    .peak,
                [0.0; 2]
            );
            generator_mask |= progress.generator_mask;
            insert_mask |= progress.insert_mask;
            wait_until(|| {
                target_control.stats().completed > target_completed
                    && first_downstream_control.stats().completed > first_completed
                    && second_downstream_control.stats().completed > second_completed
            });
            let first_start = 2 * MAX_MIXER_BLOCK_FRAMES;
            let second_start = 3 * MAX_MIXER_BLOCK_FRAMES;
            first_peak = first_peak.max(
                dsp.track_block[first_start..first_start + DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES]
                    .iter()
                    .map(|frame| frame[0].abs().max(frame[1].abs()))
                    .fold(0.0, f32::max),
            );
            second_peak = second_peak.max(
                dsp.track_block[second_start..second_start + DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES]
                    .iter()
                    .map(|frame| frame[0].abs().max(frame[1].abs()))
                    .fold(0.0, f32::max),
            );
            master_peak = master_peak.max(
                dsp.master_block[..DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES]
                    .iter()
                    .map(|frame| frame[0].abs().max(frame[1].abs()))
                    .fold(0.0, f32::max),
            );
            if first_peak > 0.01 && second_peak > 0.01 && master_peak > 0.01 {
                break;
            }
        }
        assert_eq!(
            measured_monitor_peak, first_peak,
            "paused monitor telemetry must match the genuinely rendered downstream bus"
        );
        assert!(first_peak > 0.01, "first downstream branch must be audible");
        assert!(second_peak > 0.01, "fanout branch must be audible");
        assert!(master_peak > 0.01, "fanout must converge at MASTER");
        assert_eq!(generator_mask, 1);
        assert_ne!(insert_mask & (1 << 2), 0);
        assert_ne!(insert_mask & (1 << 3), 0);
        assert_eq!(insert_mask & (1 << 4), 0);
        assert_eq!(
            dsp.generator_endpoints[1]
                .as_ref()
                .unwrap()
                .endpoint
                .stats()
                .callbacks,
            unrelated_generator_callbacks
        );
        assert_eq!(
            dsp.insert_endpoints[4]
                .as_ref()
                .unwrap()
                .endpoint
                .stats()
                .callbacks,
            unrelated_insert_callbacks
        );
        assert_eq!(unrelated_control.stats().completed, 0);
        assert_eq!(unrelated_insert_control.stats().completed, 0);
        assert_eq!(dsp.voices[0].phase, native_phase);
        assert_eq!(dsp.audio_voices[0].source_position, audio_source_position);
        assert_eq!(dsp.timeline_executor.next_frame(), executor_frame);
        assert_eq!(
            dsp.timeline_runtime.as_ref().unwrap().next_frame(),
            runtime_frame
        );
        assert_eq!(status.timeline_frame.load(Ordering::Acquire), status_frame);
        assert_eq!(
            dsp.pdc_raw_track_delays[4].process_sample([0.0; 2]),
            [0.0; 2]
        );
        assert_eq!(
            dsp.pdc_raw_track_delays[4].process_sample([0.0; 2]),
            [1.0, 1.0],
            "paused monitor must not advance unrelated raw-PDC history"
        );

        dsp.track_gains[2] = f32::NAN;
        let failed_insert_callbacks = dsp.insert_endpoints[2]
            .as_ref()
            .unwrap()
            .endpoint
            .stats()
            .callbacks;
        let progress = dsp.render_paused_midi_monitor_graph(
            &status,
            DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES,
            monitor,
        );
        assert_ne!(progress.insert_mask & (1 << 2), 0);
        assert_eq!(
            dsp.insert_endpoints[2]
                .as_ref()
                .unwrap()
                .endpoint
                .stats()
                .callbacks,
            failed_insert_callbacks + 1,
            "the insert was physically invoked before the nonfinite graph failure"
        );
        assert!(
            dsp.master_block[..DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES]
                .iter()
                .all(|frame| *frame == [0.0; 2])
        );
        assert!(controller.needs_resync());
        dsp.service_paused_parameter_edits(
            DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES,
            progress.generator_mask,
            progress.insert_mask,
        );
        assert_eq!(
            dsp.insert_endpoints[2]
                .as_ref()
                .unwrap()
                .endpoint
                .stats()
                .callbacks,
            failed_insert_callbacks + 1,
            "an insert already invoked by paused graph rendering must not be serviced twice"
        );

        drop(dsp);
        target_guard.shutdown();
        unrelated_guard.shutdown();
        first_downstream_guard.shutdown();
        second_downstream_guard.shutdown();
        unrelated_insert_guard.shutdown();
        drop((
            target_control,
            unrelated_control,
            first_downstream_control,
            second_downstream_control,
            unrelated_insert_control,
        ));
    }

    #[test]
    fn dynamic_graph_pdc_commit_and_candidate_second_read_rejection_are_atomic() {
        let timeline = compile_timeline_test_project(&Project::blank());
        let latency = Arc::new(AtomicU32::new(8));
        let insert = spawn_identified_mock_latency_transform(880, Arc::clone(&latency));
        wait_until(|| {
            insert
                .control
                .plugin_latency_snapshot()
                .is_some_and(|snapshot| {
                    snapshot.revision != 0 && snapshot.total_plugin_latency_samples == 8
                })
        });
        let PluginChain {
            audio,
            control,
            guard,
        } = insert;

        let revision_a = 71;
        let epoch_a = 11;
        let (mut controller, realtime) = create_timeline_runtime();
        let chase_a = controller
            .prepare_chase(
                &timeline,
                revision_a,
                epoch_a,
                0,
                TimelineChaseOptions::default(),
            )
            .unwrap();
        let loop_chase_a = controller
            .prepare_loop_chase(&timeline, revision_a, 0, TimelineChaseOptions::default())
            .unwrap();
        install_test_timeline(&mut controller, revision_a, Arc::clone(&timeline));
        let loop_token_a = controller.install_loop_chase(loop_chase_a).unwrap();
        controller.install_chase(chase_a).unwrap();
        let mut dsp = timeline_test_dsp(realtime);
        assert_eq!(dsp.apply_pending_timeline_commands(), 3);
        dsp.install_insert_endpoint(1, 101, fixed_adapter(audio));
        let status = AudioStatus::default();
        let activation_a = TimelineTransportActivationSpec {
            revision: revision_a,
            target_epoch: epoch_a,
            minimum_epoch: epoch_a,
            frame: 0,
            beat_q32: 0,
            loop_start_frame: 0,
            loop_end_frame: 48_000,
            loop_start_q32: 0,
            loop_end_q32: 2 << 32,
            loop_token: loop_token_a,
            loop_enabled: true,
            playing: true,
            mixer_pan_release: TimelineMixerPanRelease::EMPTY,
        };
        controller.activate_transport(activation_a, 0).unwrap();
        assert_eq!(dsp.apply_pending_timeline_commands(), 1);
        let ticket_a = dsp.pending_timeline_transport_activation().unwrap();
        dsp.preflight_timeline_transport_activation(ticket_a, epoch_a)
            .unwrap();
        let committed_a = dsp.commit_timeline_transport_activation(&status, ticket_a, epoch_a);
        dsp.publish_timeline_transport_activation(committed_a);
        while controller.poll_event().is_some() {}
        controller.drain_retired();

        let route_slot = usize::from(
            *dsp.mixer_graph_plan
                .outgoing_route_slots(2)
                .first()
                .expect("track 2 default MASTER route"),
        );
        let initial_identity =
            dsp.mixer_graph_endpoint_identities.inserts[0].expect("track 1 insert identity");
        let initial_expected_revision = dsp.insert_endpoints[1]
            .as_ref()
            .unwrap()
            .endpoint
            .expected_latency_revision();
        assert_eq!(
            initial_expected_revision,
            initial_identity.snapshot.revision()
        );
        let initial_plan_revision = dsp.pdc_plan_revision;
        let initial_target = dsp
            .timeline_runtime
            .as_ref()
            .unwrap()
            .active_mixer_delay_bank()
            .unwrap()
            .target_delay_samples(route_slot)
            .unwrap();
        assert_eq!(
            initial_target,
            dsp.graph_pdc_plan
                .as_ref()
                .as_ref()
                .unwrap()
                .main_input_for_runtime_slot(route_slot)
                .unwrap()
                .delay
                .applied_samples()
        );

        latency.store(40, Ordering::Release);
        assert!(control.set_slot_config(0, SlotConfig::default()));
        wait_until(|| {
            control.plugin_latency_snapshot().is_some_and(|snapshot| {
                snapshot.revision != initial_identity.snapshot.revision()
                    && snapshot.total_plugin_latency_samples == 40
            })
        });
        let published = control.plugin_latency_snapshot().unwrap();
        assert!(dsp.refresh_graph_pdc_plan(&status));
        let dynamic_identity = dsp.mixer_graph_endpoint_identities.inserts[0]
            .expect("dynamically refreshed insert identity");
        let dynamic_plan = dsp.graph_pdc_plan.as_ref().as_ref().unwrap();
        let dynamic_target = dsp
            .timeline_runtime
            .as_ref()
            .unwrap()
            .active_mixer_delay_bank()
            .unwrap()
            .target_delay_samples(route_slot)
            .unwrap();
        assert!(dsp.pdc_plan_revision > initial_plan_revision);
        assert_eq!(dynamic_identity.snapshot.revision(), published.revision);
        assert_eq!(
            dsp.insert_endpoints[1]
                .as_ref()
                .unwrap()
                .endpoint
                .expected_latency_revision(),
            published.revision
        );
        assert_ne!(dynamic_target, initial_target);
        assert_eq!(
            dynamic_target,
            dynamic_plan
                .main_input_for_runtime_slot(route_slot)
                .unwrap()
                .delay
                .applied_samples()
        );
        assert!(dsp.graph_pdc_activation_plan.is_none());
        assert_eq!(
            dsp.mixer_graph_activation_endpoint_identities.insert_count,
            0
        );
        assert_eq!(status.pdc_fields().plan_revision, dsp.pdc_plan_revision);

        let active_graph_identity = dsp.mixer_graph_identity;
        let active_graph_fingerprint = dsp.mixer_graph_plan.fingerprint();
        let active_pdc = dynamic_plan.clone();
        let active_endpoint_identity = dynamic_identity;
        let active_expected_revision = published.revision;
        let active_runtime_frame = dsp.timeline_runtime.as_ref().unwrap().next_frame();
        let (active_bank_address, active_bank_current, active_bank_targets) = {
            let bank = dsp
                .timeline_runtime
                .as_ref()
                .unwrap()
                .active_mixer_delay_bank()
                .unwrap();
            (
                bank as *const PreparedMixerGraphDelayBank as usize,
                std::array::from_fn::<_, { crate::mixer_graph::MIXER_GRAPH_MAX_EDGES }, _>(
                    |slot| bank.current_delay_samples(slot),
                ),
                std::array::from_fn::<_, { crate::mixer_graph::MIXER_GRAPH_MAX_EDGES }, _>(
                    |slot| bank.target_delay_samples(slot),
                ),
            )
        };

        let revision_b = 72;
        let epoch_b = 12;
        let chase_b = controller
            .prepare_chase(
                &timeline,
                revision_b,
                epoch_b,
                0,
                TimelineChaseOptions::default(),
            )
            .unwrap();
        let loop_chase_b = controller
            .prepare_loop_chase(&timeline, revision_b, 0, TimelineChaseOptions::default())
            .unwrap();
        install_test_timeline(&mut controller, revision_b, Arc::clone(&timeline));
        let loop_token_b = controller.install_loop_chase(loop_chase_b).unwrap();
        controller.install_chase(chase_b).unwrap();
        assert_eq!(dsp.apply_pending_timeline_commands(), 3);
        while controller.poll_event().is_some() {}
        controller.drain_retired();
        let first_read = control.plugin_latency_snapshot().unwrap();
        let second_read = PluginLatencySnapshot {
            revision: next_nonzero_id(first_read.revision),
            ..first_read
        };
        dsp.insert_endpoints[1]
            .as_mut()
            .unwrap()
            .endpoint
            .script_fresh_endpoint_snapshots([Some(first_read), Some(second_read)]);
        let activation_b = TimelineTransportActivationSpec {
            revision: revision_b,
            target_epoch: epoch_b,
            minimum_epoch: epoch_b,
            frame: 0,
            beat_q32: 0,
            loop_start_frame: 0,
            loop_end_frame: 48_000,
            loop_start_q32: 0,
            loop_end_q32: 2 << 32,
            loop_token: loop_token_b,
            loop_enabled: true,
            playing: true,
            mixer_pan_release: TimelineMixerPanRelease::EMPTY,
        };
        controller.activate_transport(activation_b, 0).unwrap();
        assert_eq!(dsp.apply_pending_timeline_commands(), 1);
        let ticket_b = dsp.pending_timeline_transport_activation().unwrap();
        let rejection = dsp
            .preflight_timeline_transport_activation(ticket_b, epoch_b)
            .unwrap_err();
        assert_eq!(
            rejection,
            TimelineTransportActivationRejectReason::GraphPdcPlan
        );
        dsp.reject_timeline_transport_activation(ticket_b, rejection);

        let active_bank = dsp
            .timeline_runtime
            .as_ref()
            .unwrap()
            .active_mixer_delay_bank()
            .unwrap();
        assert_eq!(
            active_bank as *const PreparedMixerGraphDelayBank as usize,
            active_bank_address
        );
        assert_eq!(
            std::array::from_fn::<_, { crate::mixer_graph::MIXER_GRAPH_MAX_EDGES }, _>(|slot| {
                active_bank.current_delay_samples(slot)
            }),
            active_bank_current
        );
        assert_eq!(
            std::array::from_fn::<_, { crate::mixer_graph::MIXER_GRAPH_MAX_EDGES }, _>(|slot| {
                active_bank.target_delay_samples(slot)
            }),
            active_bank_targets
        );
        assert_eq!(
            dsp.timeline_runtime.as_ref().unwrap().active_revision(),
            Some(revision_a)
        );
        assert_eq!(
            dsp.timeline_runtime.as_ref().unwrap().active_epoch(),
            Some(epoch_a)
        );
        assert_eq!(
            dsp.timeline_runtime.as_ref().unwrap().next_frame(),
            active_runtime_frame
        );
        assert_eq!(dsp.mixer_graph_identity, active_graph_identity);
        assert_eq!(dsp.mixer_graph_plan.fingerprint(), active_graph_fingerprint);
        assert_eq!(dsp.graph_pdc_plan.as_ref().as_ref(), Some(&active_pdc));
        assert_eq!(
            dsp.mixer_graph_endpoint_identities.inserts[0],
            Some(active_endpoint_identity)
        );
        assert_eq!(
            dsp.insert_endpoints[1]
                .as_ref()
                .unwrap()
                .endpoint
                .expected_latency_revision(),
            active_expected_revision
        );
        assert!(dsp.graph_pdc_activation_plan.is_none());
        assert_eq!(
            dsp.mixer_graph_activation_endpoint_identities.insert_count,
            0
        );

        while controller.poll_event().is_some() {}
        controller.drain_retired();
        drop(dsp);
        guard.shutdown();
        drop(control);
    }

    fn chased_note(note_id: u64, channel_id: u32, note: u8) -> ChasedNote {
        ChasedNote {
            note_id,
            channel_id,
            note,
            velocity: 1.0,
            gain: 1.0,
            mixer_track: 1,
            source: NoteSourceDescriptor::ChannelStep {
                clip_id: 1,
                pattern_id: 1,
                step: 0,
                repetition: 0,
            },
        }
    }

    fn timeline_generator_route_table(
        revision: u64,
        epoch: u64,
        channel_id: u32,
        plugin_instance_id: u64,
    ) -> TimelineGeneratorRouteTable {
        let mut routes = TimelineGeneratorRouteTable::new();
        assert!(routes.reset_from(
            revision,
            epoch,
            &[CompiledPluginRoute {
                instance_id: plugin_instance_id,
                destination: PluginRouteDestination::Generator {
                    channel_id,
                    slot: 0,
                },
            }],
            &[ChannelBaseDescriptor {
                channel_id,
                volume: 1.0,
                pan: 0.0,
                muted: false,
                solo: false,
                mixer_track: 1,
            }],
        ));
        routes.routes[0]
            .as_mut()
            .expect("test generator route exists")
            .endpoint_id = 70;
        routes
    }

    fn timeline_test_engine(controller: TimelineRuntimeController) -> AudioEngine {
        let (producer, _commands) = RingBuffer::new(4);
        let (_retired_asset_tx, retired_assets) = RingBuffer::new(4);
        let (_asset_event_tx, asset_events) = RingBuffer::new(4);
        let (_retired_endpoint_tx, retired_insert_endpoints) = RingBuffer::new(4);
        let (_insert_event_tx, insert_endpoint_events) = RingBuffer::new(4);
        let (_generator_event_tx, generator_endpoint_events) = RingBuffer::new(4);
        let (_retired_midi_tx, retired_midi_inputs) = RingBuffer::new(4);
        let (_midi_event_tx, midi_input_route_events) = RingBuffer::new(4);
        let (_midi_recording_event_tx, midi_recording_endpoint_events) = RingBuffer::new(4);
        let (_parameter_edit_event_tx, parameter_edit_callback_events) = RingBuffer::new(4);
        let (_capture_event_tx, master_capture_events) = RingBuffer::new(4);
        AudioEngine {
            stream: None,
            stream_lifecycle: DeviceStreamLifecycle::Playing,
            meter_reader: meter_channel().1,
            timeline_runtime: Some(controller),
            timeline_mixer_graph_fingerprint: None,
            producer,
            retired_assets,
            asset_events,
            retired_insert_endpoints,
            insert_endpoint_events,
            generator_endpoint_events,
            retired_midi_inputs,
            midi_input_route_events,
            midi_recording_endpoint_events,
            parameter_edit_callback_events,
            parameter_edit_callback_admission: Arc::new(AtomicU32::new(0)),
            master_capture_events,
            status: Arc::new(AudioStatus::default()),
            transport_mailbox: Arc::new(TransportMailbox::default()),
            callback_telemetry: Arc::new(CallbackTelemetry::default()),
            device_name: "timeline lifecycle test".into(),
            device_profile: AudioDeviceProfile::system_default_output(),
            effective_device_profile: AudioDeviceProfile::system_default_output(),
            effective_stream_config: AudioEffectiveStreamConfig {
                channels: 2,
                sample_rate: 48_000,
                sample_format: crate::audio_device::AudioSampleFormat::F32,
                buffer_size: AudioEffectiveBufferSize::Fixed(2_048),
            },
            has_realtime_owned_resources: false,
            timeline_runtime_shutdown_confirmed: false,
        }
    }

    fn timeline_test_dsp(realtime: RealtimeTimelineRuntime) -> DspState {
        DspState::try_new_inner(48_000.0, None, None, None, None, 512, Some(realtime)).unwrap()
    }

    fn run_timeline_callback_until_shutdown(
        mut dsp: DspState,
    ) -> thread::JoinHandle<(bool, TimelineRuntimeStats)> {
        thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !dsp
                .timeline_runtime
                .as_ref()
                .expect("test DSP has a timeline runtime")
                .is_shutdown()
                && Instant::now() < deadline
            {
                dsp.apply_pending_timeline_commands();
                thread::yield_now();
            }
            let runtime = dsp
                .timeline_runtime
                .as_ref()
                .expect("test DSP has a timeline runtime");
            (runtime.is_shutdown(), runtime.stats())
        })
    }

    fn master_capture_test_path(label: &str) -> std::path::PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "citrus-audio-master-capture-{label}-{}-{nonce}.wav",
            std::process::id()
        ))
    }

    struct MockInsertBackend {
        add: f32,
        gain: f32,
        last_midi: Arc<AtomicU32>,
        parameter: Arc<AtomicU32>,
        latency: Arc<AtomicU32>,
        process_delay: Duration,
        delay_left: [f32; 512],
        delay_right: [f32; 512],
        delay_index: usize,
    }

    impl PluginBackend for MockInsertBackend {
        fn name(&self) -> &str {
            "audio integration mock"
        }

        fn prepare(&mut self, _config: PluginPrepareConfig) -> Result<(), String> {
            Ok(())
        }

        fn process(
            &mut self,
            left: &mut [f32],
            right: &mut [f32],
            frames: usize,
        ) -> Result<(), String> {
            if !self.process_delay.is_zero() {
                thread::sleep(self.process_delay);
            }
            let delay = self.latency.load(Ordering::Acquire).min(511) as usize;
            for frame in 0..frames {
                let transformed_left = left[frame] * self.gain + self.add;
                let transformed_right = right[frame] * self.gain + self.add;
                self.delay_left[self.delay_index] = transformed_left;
                self.delay_right[self.delay_index] = transformed_right;
                if delay == 0 {
                    left[frame] = transformed_left;
                    right[frame] = transformed_right;
                } else {
                    let read =
                        (self.delay_index + self.delay_left.len() - delay) % self.delay_left.len();
                    left[frame] = self.delay_left[read];
                    right[frame] = self.delay_right[read];
                }
                self.delay_index = (self.delay_index + 1) % self.delay_left.len();
            }
            Ok(())
        }

        fn send_midi(&mut self, message: MidiMessage) -> Result<(), String> {
            self.last_midi.store(
                u32::from_le_bytes([
                    message.data[0],
                    message.data[1],
                    message.data[2],
                    message.sample_offset.min(u16::from(u8::MAX)) as u8,
                ]),
                Ordering::Release,
            );
            Ok(())
        }

        fn set_parameter(&mut self, _id: u32, normalized: f32) -> Result<(), String> {
            self.parameter
                .store(normalized.to_bits(), Ordering::Release);
            Ok(())
        }

        fn get_parameter(&mut self, _id: u32) -> Result<f32, String> {
            Ok(f32::from_bits(self.parameter.load(Ordering::Acquire)))
        }

        fn save_state(&mut self) -> Result<Vec<u8>, String> {
            Ok(Vec::new())
        }

        fn load_state(&mut self, _state: &[u8]) -> Result<(), String> {
            Ok(())
        }

        fn latency_samples(&self) -> u32 {
            self.latency.load(Ordering::Acquire)
        }

        fn tail_samples(&self) -> u32 {
            0
        }
    }

    struct MockInstrumentBackend {
        active: bool,
        amplitude: f32,
        process_delay: Duration,
        last_midi: Arc<AtomicU32>,
        parameter: Arc<AtomicU32>,
    }

    impl PluginBackend for MockInstrumentBackend {
        fn name(&self) -> &str {
            "instrument integration mock"
        }

        fn prepare(&mut self, _config: PluginPrepareConfig) -> Result<(), String> {
            Ok(())
        }

        fn process(
            &mut self,
            left: &mut [f32],
            right: &mut [f32],
            frames: usize,
        ) -> Result<(), String> {
            if !self.process_delay.is_zero() {
                thread::sleep(self.process_delay);
            }
            if self.active {
                for sample in &mut left[..frames] {
                    *sample += self.amplitude;
                }
                for sample in &mut right[..frames] {
                    *sample += self.amplitude;
                }
            }
            Ok(())
        }

        fn send_midi(&mut self, message: MidiMessage) -> Result<(), String> {
            self.last_midi.store(
                u32::from_le_bytes([
                    message.data[0],
                    message.data[1],
                    message.data[2],
                    message.sample_offset.min(u16::from(u8::MAX)) as u8,
                ]),
                Ordering::Release,
            );
            match message.data[0] & 0xF0 {
                0x90 => self.active = message.data[2] != 0,
                0x80 => self.active = false,
                0xB0 if message.data[1] == 123 => self.active = false,
                _ => {}
            }
            Ok(())
        }

        fn set_parameter(&mut self, _id: u32, normalized: f32) -> Result<(), String> {
            self.amplitude = normalized;
            self.parameter
                .store(normalized.to_bits(), Ordering::Release);
            Ok(())
        }

        fn get_parameter(&mut self, _id: u32) -> Result<f32, String> {
            Ok(self.amplitude)
        }

        fn save_state(&mut self) -> Result<Vec<u8>, String> {
            Ok(self.amplitude.to_le_bytes().to_vec())
        }

        fn load_state(&mut self, state: &[u8]) -> Result<(), String> {
            let bytes: [u8; 4] = state
                .try_into()
                .map_err(|_| "invalid mock instrument state".to_owned())?;
            self.amplitude = f32::from_le_bytes(bytes);
            Ok(())
        }

        fn latency_samples(&self) -> u32 {
            0
        }

        fn tail_samples(&self) -> u32 {
            0
        }
    }

    struct MockImpulseGeneratorBackend {
        latency: u32,
        emitted: bool,
        delay: [f32; 32],
        delay_index: usize,
    }

    impl PluginBackend for MockImpulseGeneratorBackend {
        fn name(&self) -> &str {
            "PDC impulse generator"
        }

        fn prepare(&mut self, _config: PluginPrepareConfig) -> Result<(), String> {
            Ok(())
        }

        fn process(
            &mut self,
            left: &mut [f32],
            right: &mut [f32],
            frames: usize,
        ) -> Result<(), String> {
            let delay = self.latency.min((self.delay.len() - 1) as u32) as usize;
            for frame in 0..frames {
                let input = if self.emitted { 0.0 } else { 0.1 };
                self.emitted = true;
                self.delay[self.delay_index] = input;
                let output = if delay == 0 {
                    input
                } else {
                    self.delay[(self.delay_index + self.delay.len() - delay) % self.delay.len()]
                };
                self.delay_index = (self.delay_index + 1) % self.delay.len();
                left[frame] = output;
                right[frame] = output;
            }
            Ok(())
        }

        fn send_midi(&mut self, _message: MidiMessage) -> Result<(), String> {
            Ok(())
        }

        fn set_parameter(&mut self, _id: u32, _normalized: f32) -> Result<(), String> {
            Ok(())
        }

        fn get_parameter(&mut self, _id: u32) -> Result<f32, String> {
            Ok(0.0)
        }

        fn save_state(&mut self) -> Result<Vec<u8>, String> {
            Ok(Vec::new())
        }

        fn load_state(&mut self, _state: &[u8]) -> Result<(), String> {
            Ok(())
        }

        fn latency_samples(&self) -> u32 {
            self.latency
        }

        fn tail_samples(&self) -> u32 {
            0
        }
    }

    fn spawn_mock_insert(
        add: f32,
        last_midi: Arc<AtomicU32>,
        parameter: Arc<AtomicU32>,
        max_block_frames: usize,
    ) -> PluginChain {
        spawn_mock_transform(1.0, add, last_midi, parameter, max_block_frames)
    }

    fn spawn_mock_transform(
        gain: f32,
        add: f32,
        last_midi: Arc<AtomicU32>,
        parameter: Arc<AtomicU32>,
        max_block_frames: usize,
    ) -> PluginChain {
        PluginChain::spawn_with_backend_factory(
            move || {
                vec![BackendSlot::new(Box::new(MockInsertBackend {
                    add,
                    gain,
                    last_midi,
                    parameter,
                    latency: Arc::new(AtomicU32::new(0)),
                    process_delay: Duration::ZERO,
                    delay_left: [0.0; 512],
                    delay_right: [0.0; 512],
                    delay_index: 0,
                }))]
            },
            PluginPrepareConfig {
                sample_rate: 48_000.0,
                max_block_frames: max_block_frames.max(DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES),
            },
        )
        .expect("mock insert worker should start")
    }

    fn spawn_mock_latency_transform(
        latency: Arc<AtomicU32>,
        process_delay: Duration,
        max_block_frames: usize,
    ) -> PluginChain {
        PluginChain::spawn_with_backend_factory(
            move || {
                vec![BackendSlot::new(Box::new(MockInsertBackend {
                    add: 0.0,
                    gain: 1.0,
                    last_midi: Arc::new(AtomicU32::new(0)),
                    parameter: Arc::new(AtomicU32::new(0)),
                    latency,
                    process_delay,
                    delay_left: [0.0; 512],
                    delay_right: [0.0; 512],
                    delay_index: 0,
                }))]
            },
            PluginPrepareConfig {
                sample_rate: 48_000.0,
                max_block_frames: max_block_frames.max(DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES),
            },
        )
        .expect("latency mock worker should start")
    }

    fn spawn_identified_mock_latency_transform(
        instance_id: u64,
        latency: Arc<AtomicU32>,
    ) -> PluginChain {
        PluginChain::spawn_identified_with_backend_factory(
            &[instance_id],
            move || {
                vec![BackendSlot::new(Box::new(MockInsertBackend {
                    add: 0.0,
                    gain: 1.0,
                    last_midi: Arc::new(AtomicU32::new(0)),
                    parameter: Arc::new(AtomicU32::new(0)),
                    latency,
                    process_delay: Duration::ZERO,
                    delay_left: [0.0; 512],
                    delay_right: [0.0; 512],
                    delay_index: 0,
                }))]
            },
            PluginPrepareConfig {
                sample_rate: 48_000.0,
                max_block_frames: DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES,
            },
        )
        .expect("identified latency mock worker should start")
    }

    fn spawn_two_slot_latency_insert(
        first_latency_samples: u32,
        first_parameter: Arc<AtomicU32>,
        second_parameter: Arc<AtomicU32>,
    ) -> PluginChain {
        PluginChain::spawn_with_backend_factory(
            move || {
                vec![
                    BackendSlot::new(Box::new(MockInsertBackend {
                        add: 0.0,
                        gain: 1.0,
                        last_midi: Arc::new(AtomicU32::new(0)),
                        parameter: first_parameter,
                        latency: Arc::new(AtomicU32::new(first_latency_samples)),
                        process_delay: Duration::ZERO,
                        delay_left: [0.0; 512],
                        delay_right: [0.0; 512],
                        delay_index: 0,
                    })),
                    BackendSlot::new(Box::new(MockInsertBackend {
                        add: 0.0,
                        gain: 1.0,
                        last_midi: Arc::new(AtomicU32::new(0)),
                        parameter: second_parameter,
                        latency: Arc::new(AtomicU32::new(0)),
                        process_delay: Duration::ZERO,
                        delay_left: [0.0; 512],
                        delay_right: [0.0; 512],
                        delay_index: 0,
                    })),
                ]
            },
            PluginPrepareConfig {
                sample_rate: 48_000.0,
                max_block_frames: MAX_MIXER_BLOCK_FRAMES,
            },
        )
        .expect("two-slot latency insert worker should start")
    }

    fn spawn_mock_impulse_generator(latency: u32, max_block_frames: usize) -> PluginChain {
        PluginChain::spawn_with_backend_factory(
            move || {
                vec![BackendSlot::new(Box::new(MockImpulseGeneratorBackend {
                    latency,
                    emitted: false,
                    delay: [0.0; 32],
                    delay_index: 0,
                }))]
            },
            PluginPrepareConfig {
                sample_rate: 48_000.0,
                max_block_frames: max_block_frames.max(DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES),
            },
        )
        .expect("impulse generator worker should start")
    }

    fn spawn_mock_instrument(
        amplitude: f32,
        last_midi: Arc<AtomicU32>,
        parameter: Arc<AtomicU32>,
        max_block_frames: usize,
    ) -> PluginChain {
        spawn_mock_instrument_with_delay(
            amplitude,
            last_midi,
            parameter,
            max_block_frames,
            Duration::ZERO,
        )
    }

    fn spawn_identified_mock_instrument(
        instance_id: u64,
        amplitude: f32,
        last_midi: Arc<AtomicU32>,
        parameter: Arc<AtomicU32>,
    ) -> PluginChain {
        PluginChain::spawn_identified_with_backend_factory(
            &[instance_id],
            move || {
                vec![BackendSlot::new(Box::new(MockInstrumentBackend {
                    active: false,
                    amplitude,
                    process_delay: Duration::ZERO,
                    last_midi,
                    parameter,
                }))]
            },
            PluginPrepareConfig {
                sample_rate: 48_000.0,
                max_block_frames: DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES,
            },
        )
        .expect("identified mock instrument should start")
    }

    fn spawn_mock_instrument_with_delay(
        amplitude: f32,
        last_midi: Arc<AtomicU32>,
        parameter: Arc<AtomicU32>,
        max_block_frames: usize,
        process_delay: Duration,
    ) -> PluginChain {
        PluginChain::spawn_with_backend_factory(
            move || {
                vec![BackendSlot::new(Box::new(MockInstrumentBackend {
                    active: false,
                    amplitude,
                    process_delay,
                    last_midi,
                    parameter,
                }))]
            },
            PluginPrepareConfig {
                sample_rate: 48_000.0,
                max_block_frames: max_block_frames.max(DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES),
            },
        )
        .expect("mock instrument worker should start")
    }

    fn spawn_empty_chain(max_block_frames: usize) -> PluginChain {
        PluginChain::spawn_with_backend_factory(
            Vec::<BackendSlot>::new,
            PluginPrepareConfig {
                sample_rate: 48_000.0,
                max_block_frames: max_block_frames.max(DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES),
            },
        )
        .expect("empty endpoint worker should start")
    }

    fn spawn_identified_empty_chain() -> PluginChain {
        PluginChain::spawn_identified_with_backend_factory(
            &[],
            Vec::<BackendSlot>::new,
            PluginPrepareConfig {
                sample_rate: 48_000.0,
                max_block_frames: DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES,
            },
        )
        .expect("identified empty endpoint worker should start")
    }

    fn wait_until(mut predicate: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !predicate() {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for mock worker"
            );
            thread::yield_now();
        }
    }

    fn register_test_asset(
        dsp: &mut DspState,
        retired: &mut Producer<Arc<[f32]>>,
        id: u64,
        samples: &[f32],
        sample_rate: u32,
        channels: u16,
    ) {
        dsp.handle(
            AudioCommand::RegisterAsset {
                operation: asset_operation(id),
                id,
                samples: Arc::from(samples),
                sample_rate,
                channels,
            },
            retired,
        );
    }

    #[test]
    fn dsp_stays_finite_and_bounded() {
        let status = AudioStatus::default();
        let mut dsp = DspState::new(48_000.0);
        let (mut retired, _reclaimed) = test_reclaimer();
        register_test_asset(
            &mut dsp,
            &mut retired,
            1,
            &[f32::NAN, f32::INFINITY, f32::MAX, -f32::MAX],
            48_000,
            2,
        );
        dsp.handle(
            AudioCommand::PlayClip {
                clip_id: 1,
                asset_id: 1,
                source_frame: 0.0,
                gain: 4.0,
                mixer_track: 1,
            },
            &mut retired,
        );
        dsp.handle(
            AudioCommand::NoteOn {
                note: 60,
                velocity: 1.0,
                mixer_track: 1,
            },
            &mut retired,
        );
        dsp.handle(AudioCommand::SetMaster(f32::NAN), &mut retired);
        dsp.handle(AudioCommand::SetMaster(1.0), &mut retired);
        status.playing.store(true, Ordering::Release);
        for _ in 0..96_000 {
            let (left, right) = dsp.next_frame(&status);
            assert!(left.is_finite() && right.is_finite());
            assert!(left.abs() <= 1.0 && right.abs() <= 1.0);
        }
    }

    #[test]
    fn audio_clip_plays_mono_with_linear_sample_rate_conversion() {
        let mut dsp = DspState::new(4.0);
        let (mut retired, _reclaimed) = test_reclaimer();
        register_test_asset(&mut dsp, &mut retired, 7, &[0.0, 1.0], 2, 1);
        dsp.handle(
            AudioCommand::PlayClip {
                clip_id: 11,
                asset_id: 7,
                source_frame: 0.0,
                gain: 1.0,
                mixer_track: 0,
            },
            &mut retired,
        );

        let frames = (0..4)
            .map(|_| dsp.next_audio_mix(false))
            .collect::<Vec<_>>();

        assert_eq!(frames, vec![(0.0, 0.0), (0.5, 0.5), (1.0, 1.0), (1.0, 1.0)]);
        assert!(dsp.audio_voices.iter().all(|voice| !voice.active));
    }

    #[test]
    fn multichannel_audio_uses_first_stereo_pair_and_mixer_controls() {
        let mut dsp = DspState::new(48_000.0);
        let (mut retired, _reclaimed) = test_reclaimer();
        register_test_asset(
            &mut dsp,
            &mut retired,
            8,
            &[0.25, 0.75, 7.0, 0.25, 0.75, 7.0],
            48_000,
            3,
        );
        dsp.handle(
            AudioCommand::SetTrackGain {
                track: 4,
                gain: 0.5,
            },
            &mut retired,
        );
        dsp.handle(
            AudioCommand::SetTrackPan {
                track: 4,
                pan: -1.0,
            },
            &mut retired,
        );
        dsp.handle(
            AudioCommand::PlayClip {
                clip_id: 12,
                asset_id: 8,
                source_frame: 0.0,
                gain: 2.0,
                mixer_track: 4,
            },
            &mut retired,
        );
        assert_eq!(dsp.next_audio_mix(false), (0.25, 0.0));

        dsp.handle(
            AudioCommand::SetTrackMuted {
                track: 4,
                muted: true,
            },
            &mut retired,
        );
        dsp.handle(
            AudioCommand::PlayClip {
                clip_id: 12,
                asset_id: 8,
                source_frame: 0.0,
                gain: 1.0,
                mixer_track: 4,
            },
            &mut retired,
        );
        assert_eq!(dsp.next_audio_mix(false), (0.0, 0.0));
    }

    #[test]
    fn master_gain_and_pan_are_applied_after_audio_mixer() {
        let status = AudioStatus::default();
        status.playing.store(true, Ordering::Release);
        let mut dsp = DspState::new(48_000.0);
        let (mut retired, _reclaimed) = test_reclaimer();
        register_test_asset(&mut dsp, &mut retired, 20, &[0.5, 0.5], 48_000, 2);
        dsp.handle(AudioCommand::SetMaster(0.5), &mut retired);
        dsp.handle(AudioCommand::SetMasterPan(1.0), &mut retired);
        dsp.handle(
            AudioCommand::PlayClip {
                clip_id: 20,
                asset_id: 20,
                source_frame: 0.0,
                gain: 1.0,
                mixer_track: 0,
            },
            &mut retired,
        );

        let (left, right) = dsp.next_frame(&status);

        assert_eq!(left, 0.0);
        assert!((right - 0.25_f32.tanh()).abs() < 1.0e-6);
    }

    #[test]
    fn block_mixer_applies_master_mute_and_ignores_master_solo_for_inserts() {
        let status = AudioStatus::default();
        status.playing.store(true, Ordering::Release);
        let mut dsp = DspState::new(48_000.0);
        let (mut retired, _reclaimed) = test_reclaimer();
        register_test_asset(&mut dsp, &mut retired, 21, &[0.5; 8], 48_000, 1);
        dsp.handle(
            AudioCommand::SetTrackSolo {
                track: 0,
                solo: true,
            },
            &mut retired,
        );
        dsp.handle(
            AudioCommand::PlayClip {
                clip_id: 21,
                asset_id: 21,
                source_frame: 0.0,
                gain: 1.0,
                mixer_track: 1,
            },
            &mut retired,
        );

        let (_, right) = dsp.next_frame(&status);
        assert!((right - (0.5_f32 * 0.72).tanh()).abs() < 1.0e-6);

        dsp.handle(
            AudioCommand::SetTrackMuted {
                track: 0,
                muted: true,
            },
            &mut retired,
        );
        dsp.handle(
            AudioCommand::PlayClip {
                clip_id: 21,
                asset_id: 21,
                source_frame: 0.0,
                gain: 1.0,
                mixer_track: 1,
            },
            &mut retired,
        );
        assert_eq!(dsp.next_frame(&status), (0.0, 0.0));
    }

    #[test]
    fn block_mixer_renders_bounded_prefix_and_advances_audio_once_per_frame() {
        let status = AudioStatus::default();
        status.playing.store(true, Ordering::Release);
        let mut dsp = DspState::new(48_000.0);
        let (mut retired, _reclaimed) = test_reclaimer();
        register_test_asset(&mut dsp, &mut retired, 22, &[0.0, 0.5, 1.0], 48_000, 1);
        dsp.handle(
            AudioCommand::PlayClip {
                clip_id: 22,
                asset_id: 22,
                source_frame: 0.0,
                gain: 1.0,
                mixer_track: 1,
            },
            &mut retired,
        );

        dsp.render_block(&status, 3);

        assert_eq!(dsp.master_block[0], [0.0, 0.0]);
        let expected_mid = (0.5_f32 * 0.72).tanh();
        let expected_end = 0.72_f32.tanh();
        assert!((dsp.master_block[1][0] - expected_mid).abs() < 1.0e-6);
        assert!((dsp.master_block[2][1] - expected_end).abs() < 1.0e-6);
        assert!(dsp.audio_voices.iter().all(|voice| !voice.active));
    }

    #[test]
    fn repeated_sync_updates_gain_but_only_corrects_large_drift() {
        let mut dsp = DspState::new(48_000.0);
        let (mut retired, _reclaimed) = test_reclaimer();
        let samples = vec![0.5; 2_000];
        register_test_asset(&mut dsp, &mut retired, 9, &samples, 48_000, 1);
        dsp.handle(
            AudioCommand::PlayClip {
                clip_id: 13,
                asset_id: 9,
                source_frame: 0.0,
                gain: 1.0,
                mixer_track: 0,
            },
            &mut retired,
        );
        let _ = dsp.next_audio_mix(false);
        assert_eq!(dsp.audio_voices[0].source_position, 1.0);

        dsp.handle(
            AudioCommand::SyncClip {
                clip_id: 13,
                asset_id: 9,
                source_frame: 2.0,
                gain: 0.25,
                mixer_track: 3,
            },
            &mut retired,
        );
        assert_eq!(dsp.audio_voices[0].source_position, 1.0);
        assert_eq!(dsp.audio_voices[0].gain, 0.25);
        assert_eq!(dsp.audio_voices[0].mixer_track, 3);

        dsp.handle(
            AudioCommand::SyncClip {
                clip_id: 13,
                asset_id: 9,
                source_frame: 1_000.0,
                gain: 0.5,
                mixer_track: 2,
            },
            &mut retired,
        );
        assert_eq!(dsp.audio_voices[0].source_position, 1_000.0);
    }

    #[test]
    fn stop_clip_stop_all_and_solo_routing_silence_expected_voices() {
        let mut dsp = DspState::new(48_000.0);
        let (mut retired, _reclaimed) = test_reclaimer();
        register_test_asset(&mut dsp, &mut retired, 10, &[0.5; 16], 48_000, 1);
        dsp.handle(
            AudioCommand::PlayClip {
                clip_id: 14,
                asset_id: 10,
                source_frame: 0.0,
                gain: 1.0,
                mixer_track: 1,
            },
            &mut retired,
        );
        dsp.handle(
            AudioCommand::SetTrackSolo {
                track: 2,
                solo: true,
            },
            &mut retired,
        );
        assert_eq!(dsp.next_audio_mix(true), (0.0, 0.0));

        dsp.handle(AudioCommand::StopClip { clip_id: 14 }, &mut retired);
        assert!(dsp.audio_voices.iter().all(|voice| !voice.active));
        dsp.handle(
            AudioCommand::PlayClip {
                clip_id: 14,
                asset_id: 10,
                source_frame: 0.0,
                gain: 1.0,
                mixer_track: 2,
            },
            &mut retired,
        );
        dsp.handle(
            AudioCommand::NoteOn {
                note: 60,
                velocity: 1.0,
                mixer_track: 2,
            },
            &mut retired,
        );
        dsp.handle(AudioCommand::StopAll, &mut retired);
        assert!(dsp.audio_voices.iter().all(|voice| !voice.active));
        assert!(dsp.voices.iter().all(|voice| !voice.active));
    }

    #[test]
    fn transport_pause_holds_audio_voice_source_position() {
        let status = AudioStatus::default();
        let mut dsp = DspState::new(48_000.0);
        let (mut retired, _reclaimed) = test_reclaimer();
        register_test_asset(&mut dsp, &mut retired, 15, &[0.5; 8], 48_000, 1);
        dsp.handle(
            AudioCommand::PlayClip {
                clip_id: 15,
                asset_id: 15,
                source_frame: 2.0,
                gain: 1.0,
                mixer_track: 0,
            },
            &mut retired,
        );

        assert_eq!(dsp.next_frame(&status), (0.0, 0.0));
        assert_eq!(dsp.audio_voices[0].source_position, 2.0);

        status.playing.store(true, Ordering::Release);
        let _ = dsp.next_frame(&status);
        assert_eq!(dsp.audio_voices[0].source_position, 3.0);
    }

    #[test]
    fn full_asset_table_returns_rejected_arc_to_non_realtime_consumer() {
        let mut dsp = DspState::new(48_000.0);
        let (mut retired, mut reclaimed) = test_reclaimer();
        for id in 0..MAX_REGISTERED_AUDIO_ASSETS as u64 {
            register_test_asset(&mut dsp, &mut retired, id, &[0.0], 48_000, 1);
        }
        assert!(dsp.audio_assets.iter().all(|slot| slot.samples.is_some()));

        let rejected: Arc<[f32]> = Arc::from([0.75_f32]);
        let observer = rejected.clone();
        dsp.handle(
            AudioCommand::RegisterAsset {
                operation: asset_operation(10_000),
                id: 10_000,
                samples: rejected,
                sample_rate: 48_000,
                channels: 1,
            },
            &mut retired,
        );

        let reclaimed_asset = reclaimed.pop().unwrap();
        assert!(Arc::ptr_eq(&observer, &reclaimed_asset));
        assert_eq!(
            dsp.pending_asset_event.take(),
            Some(AudioAssetEvent::Registered {
                operation: asset_operation(10_000),
                id: 10_000,
                success: false,
            })
        );
    }

    #[test]
    fn clear_and_unregister_release_slots_stop_voices_and_emit_confirmations() {
        let mut dsp = DspState::new(48_000.0);
        let (mut command_tx, mut command_rx) = RingBuffer::new(16);
        let (mut retired_tx, mut retired_rx) = RingBuffer::new(16);
        let (mut event_tx, mut event_rx) = RingBuffer::new(16);
        let samples: Arc<[f32]> = Arc::from([0.25_f32; 8]);
        let observer = samples.clone();

        command_tx
            .push(AudioCommand::RegisterAsset {
                operation: asset_operation(77),
                id: 77,
                samples,
                sample_rate: 48_000,
                channels: 1,
            })
            .unwrap();
        process_commands(&mut dsp, &mut command_rx, &mut retired_tx, &mut event_tx);
        assert_eq!(
            event_rx.pop().unwrap(),
            AudioAssetEvent::Registered {
                operation: asset_operation(77),
                id: 77,
                success: true,
            }
        );
        assert_eq!(dsp.find_asset_slot(77), Some(0));

        command_tx
            .push(AudioCommand::PlayClip {
                clip_id: 9,
                asset_id: 77,
                source_frame: 0.0,
                gain: 1.0,
                mixer_track: 0,
            })
            .unwrap();
        command_tx
            .push(AudioCommand::ClearAssets {
                operation: asset_operation(78),
            })
            .unwrap();
        process_commands(&mut dsp, &mut command_rx, &mut retired_tx, &mut event_tx);
        assert_eq!(
            event_rx.pop().unwrap(),
            AudioAssetEvent::Cleared {
                operation: asset_operation(78),
                removed: 1,
            }
        );
        assert!(dsp.audio_assets.iter().all(|slot| slot.samples.is_none()));
        assert!(dsp.audio_voices.iter().all(|voice| !voice.active));
        let retired = retired_rx.pop().unwrap();
        assert!(Arc::ptr_eq(&observer, &retired));

        command_tx
            .push(AudioCommand::RegisterAsset {
                operation: asset_operation(88),
                id: 88,
                samples: Arc::from([0.5_f32; 4]),
                sample_rate: 48_000,
                channels: 1,
            })
            .unwrap();
        command_tx
            .push(AudioCommand::UnregisterAsset {
                operation: asset_operation(89),
                id: 88,
            })
            .unwrap();
        process_commands(&mut dsp, &mut command_rx, &mut retired_tx, &mut event_tx);
        assert_eq!(
            event_rx.pop().unwrap(),
            AudioAssetEvent::Registered {
                operation: asset_operation(88),
                id: 88,
                success: true,
            }
        );
        assert_eq!(
            event_rx.pop().unwrap(),
            AudioAssetEvent::Unregistered {
                operation: asset_operation(89),
                id: 88,
                removed: true,
            }
        );
        assert!(dsp.find_asset_slot(88).is_none());
    }

    #[test]
    fn bounded_command_queue_reports_full_without_blocking() {
        let status = AudioStatus::default();
        let (mut producer, _consumer) = RingBuffer::new(1);

        assert!(queue_audio_command(
            &mut producer,
            &status,
            AudioCommand::SetMaster(0.5)
        ));
        assert!(!queue_audio_command(
            &mut producer,
            &status,
            AudioCommand::StopAll
        ));
        assert_eq!(status.command_queue_full.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn timeline_lifecycle_is_confirmed_and_retired_without_audio_hardware() {
        let (controller, realtime) = create_timeline_runtime();
        let mut engine = timeline_test_engine(controller);
        let mut dsp = timeline_test_dsp(realtime);
        let timeline = compiled_test_timeline();
        let timeline_observer = Arc::clone(&timeline);
        assert_eq!(Arc::strong_count(&timeline_observer), 2);
        let chase = engine
            .prepare_timeline_chase(&timeline, 41, 7, 0, TimelineChaseOptions::default())
            .unwrap();
        let install_request = engine.install_compiled_timeline(41, timeline).unwrap();
        let chase_request = engine.install_timeline_chase(chase).unwrap();

        assert_eq!(dsp.apply_pending_timeline_commands(), 2);
        assert_eq!(Arc::strong_count(&timeline_observer), 2);
        assert_eq!(
            engine.next_timeline_runtime_event(),
            Some(TimelineRuntimeEvent::Installed {
                request_id: install_request,
                revision: 41,
            })
        );
        assert_eq!(
            engine.next_timeline_runtime_event(),
            Some(TimelineRuntimeEvent::ChaseInstalled {
                request_id: chase_request,
                revision: 41,
                epoch: 7,
                frame: 0,
            })
        );
        dsp.timeline_runtime
            .as_mut()
            .unwrap()
            .activate_discontinuity(
                41,
                7,
                0,
                crate::timeline_runtime::TimelineDiscontinuityKind::OneShot,
            )
            .unwrap();
        assert_eq!(engine.confirmed_timeline_revision(), Some(41));
        assert_eq!(engine.confirmed_timeline_epoch(), Some(7));
        assert!(engine.timeline_runtime_is_synchronized());
        assert!(
            dsp.timeline_runtime
                .as_mut()
                .unwrap()
                .chase_for_block(41, 7, 0)
                .unwrap()
                .is_some()
        );

        // The control cache keeps the same allocation and can prepare every
        // later seek/loop chase after the callback installation.
        let later_chase = engine
            .prepare_timeline_chase(
                &timeline_observer,
                41,
                8,
                0,
                TimelineChaseOptions::default(),
            )
            .unwrap();
        let later_chase_request = engine.install_timeline_chase(later_chase).unwrap();
        assert_eq!(dsp.apply_pending_timeline_commands(), 1);
        assert_eq!(
            engine.next_timeline_runtime_event(),
            Some(TimelineRuntimeEvent::ChaseInstalled {
                request_id: later_chase_request,
                revision: 41,
                epoch: 8,
                frame: 0,
            })
        );
        dsp.timeline_runtime
            .as_mut()
            .unwrap()
            .activate_discontinuity(
                41,
                8,
                0,
                crate::timeline_runtime::TimelineDiscontinuityKind::OneShot,
            )
            .unwrap();
        assert!(
            dsp.timeline_runtime
                .as_mut()
                .unwrap()
                .chase_for_block(41, 8, 0)
                .unwrap()
                .is_some()
        );
        assert!(matches!(
            engine.next_retired_timeline_resource(),
            Some(RetiredTimelineResource::Chase {
                revision: 41,
                epoch: 7,
                ..
            })
        ));
        assert_eq!(engine.confirmed_timeline_epoch(), Some(8));
        assert_eq!(Arc::strong_count(&timeline_observer), 2);

        let clear_request = engine.clear_compiled_timeline(41).unwrap();
        assert_eq!(dsp.apply_pending_timeline_commands(), 1);
        assert_eq!(
            engine.next_timeline_runtime_event(),
            Some(TimelineRuntimeEvent::Cleared {
                request_id: clear_request,
                revision: 41,
            })
        );
        let retired = engine.next_retired_timeline_resource().unwrap();
        let RetiredTimelineResource::Bundle {
            revision,
            timeline: retired_timeline,
            one_shot: Some(retired_chase),
            ..
        } = retired
        else {
            panic!("expected retired timeline bundle");
        };
        assert_eq!(revision, 41);
        assert_eq!(retired_chase.epoch(), 8);
        assert!(Arc::ptr_eq(&timeline_observer, &retired_timeline));
        assert_eq!(Arc::strong_count(&timeline_observer), 2);
        drop(retired_timeline);
        assert_eq!(Arc::strong_count(&timeline_observer), 1);
        assert!(engine.next_retired_timeline_resource().is_none());

        let callback = run_timeline_callback_until_shutdown(dsp);
        assert!(engine.shutdown_realtime_resources(Duration::from_secs(2)));
        let (shutdown, stats) = callback.join().unwrap();
        assert!(shutdown);
        assert_eq!(stats.unexpected_realtime_drops, 0);
        drop(engine);
    }

    #[test]
    fn timeline_paused_cursor_freezes_then_chunked_burst_commits_once() {
        let mut project = timeline_test_project(frame_as_beat(256));
        let mut enabled = [false; 16];
        enabled[0] = true;
        for channel_id in 1..=40 {
            project.channels.push(timeline_test_channel(channel_id, 1));
        }
        project.patterns.push(Pattern {
            id: 1,
            name: "forty-way burst".into(),
            length_steps: 16,
            channel_steps: vec![enabled; 40],
            notes: Vec::new(),
        });
        project
            .clips
            .push(timeline_pattern_clip(1, frame_as_beat(128), 1));
        let timeline = compile_timeline_test_project(&project);
        assert_eq!(timeline.stats().max_events_at_frame, 40);
        let (_controller, mut dsp, status) =
            install_and_activate_timeline(timeline, 101, 2, 0, TimelineChaseOptions::default());

        status.playing.store(false, Ordering::Release);
        assert!(!dsp.prepare_timeline_render(2, 0, 0, 64, false));
        assert_eq!(dsp.timeline_runtime.as_ref().unwrap().next_frame(), Some(0));
        assert_eq!(dsp.timeline_executor.next_frame(), 0);

        status.playing.store(true, Ordering::Release);
        assert!(dsp.prepare_timeline_render(2, 0, 0, 64, true));
        dsp.render_block(&status, 64);
        assert_eq!(
            dsp.timeline_runtime.as_ref().unwrap().next_frame(),
            Some(64)
        );
        assert_eq!(dsp.timeline_executor.next_frame(), 64);
        assert_eq!(dsp.timeline_executor.active_note_count(), 40);
        assert_eq!(dsp.timeline_execution_failures, 0);
    }

    #[test]
    fn timeline_automation_first_block_separates_chase_and_renders_linear_hold_and_end() {
        let mut project = timeline_test_project(frame_as_beat(16));
        push_timeline_automation(
            &mut project,
            1,
            AutomationTarget::MasterVolume,
            AutomationCurve::Linear,
            [
                AutomationPoint::new(0.0, 0.0),
                AutomationPoint::new(frame_as_beat_f64(4), 1.0),
                AutomationPoint::new(frame_as_beat_f64(8), 1.0),
            ],
        );
        push_timeline_automation(
            &mut project,
            2,
            AutomationTarget::MasterPan,
            AutomationCurve::Hold,
            [
                AutomationPoint::new(0.0, -1.0),
                AutomationPoint::new(frame_as_beat_f64(4), 1.0),
            ],
        );
        push_timeline_automation(
            &mut project,
            3,
            AutomationTarget::MixerMute {
                track: crate::mixer_graph::MASTER_MIXER_TRACK_ID,
            },
            AutomationCurve::Hold,
            [AutomationPoint::new(0.0, 1.0)],
        );
        let mut mute_clip = timeline_pattern_clip(900, frame_as_beat(4), 0);
        mute_clip.kind = ClipKind::Automation;
        mute_clip.automation_id = Some(3);
        project.clips.push(mute_clip);

        let timeline = compile_timeline_test_project(&project);
        let base_count = timeline.automation_bases().len();
        let expected_transitions = timeline
            .events()
            .iter()
            .filter(|event| event.frame < 8)
            .filter(|event| {
                matches!(
                    event.kind,
                    TimelineEventKind::AutomationRamp(_) | TimelineEventKind::AutomationEnd { .. }
                )
            })
            .count();
        assert_eq!(timeline.driven_automation_targets().len(), 3);
        let (_controller, mut dsp, status) =
            install_and_activate_timeline(timeline, 124, 2, 0, TimelineChaseOptions::default());

        assert_eq!(dsp.timeline_plan.automation_chase_len, base_count);
        assert_eq!(dsp.timeline_automation.epoch(), 2);
        assert_eq!(dsp.timeline_automation.next_frame(), 0);
        assert_eq!(dsp.timeline_automation_pending, 3);
        assert!(dsp.prepare_timeline_render(2, 0, 0, 8, true));

        // Chase values remain typed reset input. Only packet transitions enter
        // the first block plan, so frame zero is not counted twice.
        assert_eq!(dsp.timeline_plan.automation_chase_len, base_count);
        assert_eq!(
            dsp.timeline_plan.automation_block.transitions().len(),
            expected_transitions
        );
        assert_eq!(
            dsp.timeline_plan.automation_block.endpoints().len(),
            base_count
        );
        assert_eq!(dsp.timeline_automation.stats().rendered_blocks, 1);
        assert_eq!(
            dsp.timeline_automation.stats().control_points,
            expected_transitions as u64
        );
        assert_eq!(dsp.timeline_automation.next_frame(), 8);
        assert_eq!(dsp.timeline_runtime.as_ref().unwrap().next_frame(), Some(8));
        assert_eq!(dsp.timeline_automation_pending, 3);
        assert_eq!(dsp.timeline_automation_unsupported, 3 + 3 * 8);

        let value = |target, frame| {
            dsp.timeline_automation_values
                .value_for(target, frame)
                .expect("compiled automation target has a matrix value")
        };
        let close = |actual: f32, expected: f32| {
            assert!(
                (actual - expected).abs() <= 1.0e-5,
                "expected {expected}, got {actual}"
            );
        };
        for (frame, expected) in [0.0, 0.25, 0.5, 0.75, 1.0].into_iter().enumerate() {
            close(
                value(CompiledAutomationTarget::MasterVolume, frame),
                expected,
            );
        }
        for frame in 0..4 {
            close(value(CompiledAutomationTarget::MasterPan, frame), -1.0);
            close(
                value(CompiledAutomationTarget::MixerMute { track: 0 }, frame),
                1.0,
            );
        }
        for frame in 4..8 {
            close(value(CompiledAutomationTarget::MasterPan, frame), 1.0);
            close(
                value(CompiledAutomationTarget::MixerMute { track: 0 }, frame),
                0.0,
            );
        }
        dsp.render_block(&status, 8);
    }

    #[test]
    fn timeline_runtime_abort_discards_finished_kernel_and_staged_matrix() {
        let project = timeline_test_project(frame_as_beat(64));
        let timeline = compile_timeline_test_project(&project);
        let (_controller, mut dsp, _status) =
            install_and_activate_timeline(timeline, 125, 2, 0, TimelineChaseOptions::default());
        let kernel_before = dsp.timeline_automation.stats();
        let active_matrix_frames = dsp.timeline_automation_values.frames;
        let active_matrix_finished = dsp.timeline_automation_values.finished;

        // This binding check runs after executor finish and automation render,
        // forcing the runtime guard and the finished kernel transaction down
        // their shared abort path.
        dsp.timeline_generator_routes.revision = None;
        assert!(!dsp.prepare_timeline_render(2, 0, 0, 8, true));
        assert_eq!(dsp.timeline_runtime.as_ref().unwrap().next_frame(), Some(0));
        assert!(
            dsp.timeline_runtime
                .as_ref()
                .unwrap()
                .stats()
                .ownership_needs_resync
        );
        assert_eq!(dsp.timeline_automation.epoch(), 2);
        assert_eq!(dsp.timeline_automation.next_frame(), 0);
        assert_eq!(
            dsp.timeline_automation.stats().rendered_blocks,
            kernel_before.rendered_blocks
        );
        assert_eq!(
            dsp.timeline_automation.stats().aborted_blocks,
            kernel_before.aborted_blocks + 1
        );
        assert_eq!(dsp.timeline_automation_values.frames, active_matrix_frames);
        assert_eq!(
            dsp.timeline_automation_values.finished,
            active_matrix_finished
        );
        assert_eq!(dsp.timeline_automation_pending, 0);
    }

    #[test]
    fn native_channel_linear_volume_is_sample_exact_before_raw_pdc() {
        let mut project = timeline_test_project(frame_as_beat(16));
        project.channels.push(timeline_test_channel(7, 1));
        push_timeline_automation(
            &mut project,
            10,
            AutomationTarget::ChannelVolume { channel: 7 },
            AutomationCurve::Linear,
            [
                AutomationPoint::new(0.0, 0.0),
                AutomationPoint::new(frame_as_beat_f64(4), 1.0),
                AutomationPoint::new(frame_as_beat_f64(8), 1.0),
            ],
        );
        let timeline = compile_timeline_test_project(&project);
        let (mut controller, mut dsp, status) = install_and_activate_timeline(
            Arc::clone(&timeline),
            126,
            2,
            0,
            TimelineChaseOptions::default(),
        );
        let base = dsp.timeline_channel_bases.get(7).unwrap();
        let slot = base
            .volume_automation_slot
            .expect("native Channel volume is bound by stable base slot");
        assert_eq!(
            dsp.timeline_channel_bases.applied_automation_target_count(),
            1
        );
        assert_eq!(dsp.timeline_automation_pending, 0);
        assert_eq!(dsp.timeline_automation_unsupported, 0);
        assert!(dsp.prepare_timeline_render(2, 0, 0, 8, true));
        assert_eq!(
            dsp.timeline_automation_values.target_at(slot),
            Some(CompiledAutomationTarget::ChannelVolume { channel_id: 7 })
        );
        assert_eq!(
            dsp.timeline_automation_values.value_at_slot(
                slot,
                CompiledAutomationTarget::ChannelPan { channel_id: 7 },
                0,
            ),
            None,
            "slot reads must reject a mismatched target identity"
        );

        install_constant_timeline_voice(&mut dsp, 7, 1);
        render_constant_timeline_voice(&mut dsp, 1, 8);
        let start = MAX_MIXER_BLOCK_FRAMES;
        for (frame, expected) in [0.0, 0.25, 0.5, 0.75, 1.0].into_iter().enumerate() {
            let [left, right] = dsp.track_block[start + frame];
            assert!((left - expected).abs() <= 1.0e-5);
            assert!((right - expected).abs() <= 1.0e-5);
        }

        dsp.pdc_raw_track_delays[1].request_delay(1, 0).unwrap();
        dsp.process_raw_source_pdc(8);
        assert_eq!(dsp.track_block[start], [0.0; 2]);
        for frame in 1..8 {
            let previous = if frame <= 4 {
                (frame - 1) as f32 * 0.25
            } else {
                1.0
            };
            let [left, right] = dsp.track_block[start + frame];
            assert!((left - previous).abs() <= 1.0e-5);
            assert!((right - previous).abs() <= 1.0e-5);
        }
        assert_eq!(dsp.timeline_automation_pending, 0);
        assert_eq!(dsp.timeline_automation_unsupported, 0);

        let loop_chase = controller
            .prepare_loop_chase(&timeline, 126, 0, TimelineChaseOptions::default())
            .unwrap();
        controller.install_loop_chase(loop_chase).unwrap();
        assert_eq!(dsp.apply_pending_timeline_commands(), 1);
        dsp.apply_transport_discontinuity(&status, 3, 0, 0, TransportDiscontinuity::Loop);
        let loop_base = dsp.timeline_channel_bases.get(7).unwrap();
        assert_eq!(loop_base.volume_automation_slot, Some(slot));
        assert_eq!(
            dsp.timeline_channel_bases.applied_automation_target_count(),
            1
        );
        assert_eq!(dsp.timeline_automation.epoch(), 3);
    }

    #[test]
    fn native_channel_hold_pan_and_mute_step_on_exact_samples() {
        let mut project = timeline_test_project(frame_as_beat(16));
        project.channels.push(timeline_test_channel(8, 1));
        push_timeline_automation(
            &mut project,
            11,
            AutomationTarget::ChannelPan { channel: 8 },
            AutomationCurve::Hold,
            [
                AutomationPoint::new(0.0, -1.0),
                AutomationPoint::new(frame_as_beat_f64(4), 1.0),
            ],
        );
        push_timeline_automation(
            &mut project,
            12,
            AutomationTarget::ChannelMute { channel: 8 },
            AutomationCurve::Hold,
            [AutomationPoint::new(0.0, 1.0)],
        );
        let mut mute_clip = timeline_pattern_clip(901, frame_as_beat(4), 0);
        mute_clip.kind = ClipKind::Automation;
        mute_clip.start = frame_as_beat(6);
        mute_clip.automation_id = Some(12);
        project.clips.push(mute_clip);
        let timeline = compile_timeline_test_project(&project);
        let (_controller, mut dsp, _status) =
            install_and_activate_timeline(timeline, 127, 2, 0, TimelineChaseOptions::default());
        let base = dsp.timeline_channel_bases.get(8).unwrap();
        assert!(base.pan_automation_slot.is_some());
        assert!(base.mute_automation_slot.is_some());
        assert_eq!(
            dsp.timeline_channel_bases.applied_automation_target_count(),
            2
        );
        assert!(dsp.prepare_timeline_render(2, 0, 0, 8, true));
        let mute_values = (0..8)
            .map(|frame| {
                dsp.timeline_automation_values
                    .value_for(
                        CompiledAutomationTarget::ChannelMute { channel_id: 8 },
                        frame,
                    )
                    .unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(mute_values, [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0]);
        install_constant_timeline_voice(&mut dsp, 8, 1);
        render_constant_timeline_voice(&mut dsp, 1, 8);
        let start = MAX_MIXER_BLOCK_FRAMES;
        for frame in 0..4 {
            let [left, right] = dsp.track_block[start + frame];
            assert!((left - 1.0).abs() <= 1.0e-5);
            assert!(right.abs() <= 1.0e-5);
        }
        for frame in 4..6 {
            let [left, right] = dsp.track_block[start + frame];
            assert!(left.abs() <= 1.0e-5);
            assert!((right - 1.0).abs() <= 1.0e-5);
        }
        assert_eq!(dsp.track_block[start + 6..start + 8], [[0.0; 2]; 2]);
        assert_eq!(dsp.timeline_automation_pending, 0);
        assert_eq!(dsp.timeline_automation_unsupported, 0);
    }

    #[test]
    fn undriven_channel_bases_remain_static_and_unbound() {
        let mut project = timeline_test_project(frame_as_beat(16));
        let mut channel = timeline_test_channel(9, 1);
        channel.volume = 0.4;
        project.channels.push(channel);
        let timeline = compile_timeline_test_project(&project);
        assert!(timeline.driven_automation_targets().is_empty());
        let (_controller, mut dsp, _status) =
            install_and_activate_timeline(timeline, 128, 2, 0, TimelineChaseOptions::default());
        let base = dsp.timeline_channel_bases.get(9).unwrap();
        assert_eq!(base.volume_automation_slot, None);
        assert_eq!(base.pan_automation_slot, None);
        assert_eq!(base.mute_automation_slot, None);
        assert_eq!(
            dsp.timeline_channel_bases.applied_automation_target_count(),
            0
        );
        assert!(dsp.prepare_timeline_render(2, 0, 0, 4, true));
        assert_eq!(
            dsp.timeline_automation_values
                .value_for(CompiledAutomationTarget::ChannelVolume { channel_id: 9 }, 0),
            Some(0.4)
        );
        install_constant_timeline_voice(&mut dsp, 9, 1);
        render_constant_timeline_voice(&mut dsp, 1, 4);
        let start = MAX_MIXER_BLOCK_FRAMES;
        for frame in 0..4 {
            let [left, right] = dsp.track_block[start + frame];
            assert!((left - 0.4).abs() <= 1.0e-5);
            assert!((right - 0.4).abs() <= 1.0e-5);
        }
    }

    #[test]
    fn generator_channel_automation_stays_unsupported_and_never_drives_native_source() {
        let mut project = timeline_test_project(frame_as_beat(16));
        let mut channel = timeline_test_channel(10, 1);
        channel.volume = 0.8;
        channel.instrument_plugin_instance_id = Some(700);
        project.channels.push(channel);
        project.plugin_instances.push(timeline_test_plugin(700));
        push_timeline_automation(
            &mut project,
            13,
            AutomationTarget::ChannelVolume { channel: 10 },
            AutomationCurve::Linear,
            [
                AutomationPoint::new(0.0, 0.0),
                AutomationPoint::new(frame_as_beat_f64(4), 1.0),
            ],
        );
        let timeline = compile_timeline_test_project(&project);
        assert_eq!(timeline.driven_automation_targets().len(), 1);
        let (mut controller, realtime) = create_timeline_runtime();
        let chase = controller
            .prepare_chase(&timeline, 129, 2, 0, TimelineChaseOptions::default())
            .unwrap();
        install_test_timeline(&mut controller, 129, timeline);
        controller.install_chase(chase).unwrap();

        let chain = spawn_empty_chain(DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
        let PluginChain {
            audio,
            control,
            guard,
        } = chain;
        let mut dsp = timeline_test_dsp(realtime);
        dsp.generator_endpoints[0] = Some(GeneratorEndpointSlot {
            channel_id: 10,
            endpoint_id: 70,
            plugin_instance_id: 700,
            mixer_track: 1,
            endpoint: fixed_adapter(audio),
            pdc_delay: test_pdc_delay(),
            pdc_initialized: false,
            suppress_output_frames: 0,
        });
        assert_eq!(dsp.apply_pending_timeline_commands(), 2);
        let status = AudioStatus::default();
        status.playing.store(true, Ordering::Release);
        dsp.apply_transport_discontinuity(&status, 2, 0, 0, TransportDiscontinuity::OneShot);

        let base = dsp.timeline_channel_bases.get(10).unwrap();
        assert_eq!(base.volume_automation_slot, None);
        assert_eq!(
            dsp.timeline_channel_bases.applied_automation_target_count(),
            0
        );
        assert_eq!(dsp.timeline_automation_pending, 1);
        assert_eq!(dsp.timeline_automation_unsupported, 1);
        assert!(dsp.prepare_timeline_render(2, 0, 0, 4, true));
        assert_eq!(dsp.timeline_automation_pending, 1);
        assert_eq!(dsp.timeline_automation_unsupported, 5);

        // Even a defensive native voice carrying that channel id uses its
        // static row; the generator's driven value is never misapplied here.
        install_constant_timeline_voice(&mut dsp, 10, 1);
        render_constant_timeline_voice(&mut dsp, 1, 1);
        let [left, right] = dsp.track_block[MAX_MIXER_BLOCK_FRAMES];
        assert!((left - 0.8).abs() <= 1.0e-5);
        assert!((right - 0.8).abs() <= 1.0e-5);

        let slot = dsp.generator_endpoints[0].take().unwrap();
        drop(slot);
        drop(dsp);
        guard.shutdown();
        drop(control);
    }

    #[test]
    fn resident_timeline_before_activation_keeps_legacy_rendering_without_fault_spam() {
        let (mut controller, realtime) = create_timeline_runtime();
        controller
            .install(
                120,
                compile_timeline_test_project(&timeline_test_project(1.0)),
            )
            .unwrap();
        let mut dsp = timeline_test_dsp(realtime);
        let status = AudioStatus::default();
        status.playing.store(true, Ordering::Release);
        let (mut retired, _reclaimed) = test_reclaimer();
        dsp.handle(
            AudioCommand::NoteOn {
                note: 69,
                velocity: 1.0,
                mixer_track: 0,
            },
            &mut retired,
        );
        assert_eq!(dsp.apply_pending_timeline_commands(), 1);
        for _ in 0..8 {
            assert!(dsp.prepare_timeline_render(1, 0, 0, 64, true));
            dsp.render_block(&status, 64);
            assert!(
                dsp.master_block[..64]
                    .iter()
                    .any(|sample| sample[0].abs() > f32::EPSILON)
            );
        }
        assert_eq!(dsp.timeline_execution_failures, 0);
        let runtime = dsp.timeline_runtime.as_ref().unwrap();
        assert_eq!(runtime.active_revision(), Some(120));
        assert_eq!(runtime.active_epoch(), None);
        assert!(!runtime.stats().ownership_needs_resync);
    }

    #[test]
    fn mailbox_pause_silences_timeline_voices_even_when_audio_command_ring_is_full() {
        let mut project = timeline_test_project(frame_as_beat(64));
        project.channels.push(timeline_test_channel(1, 1));
        let mut enabled = [false; 16];
        enabled[0] = true;
        project.patterns.push(Pattern {
            id: 1,
            name: "pause".into(),
            length_steps: 16,
            channel_steps: vec![enabled],
            notes: Vec::new(),
        });
        project
            .clips
            .push(timeline_pattern_clip(1, frame_as_beat(32), 1));
        let timeline = compile_timeline_test_project(&project);
        let (_controller, mut dsp, status) =
            install_and_activate_timeline(timeline, 121, 2, 0, TimelineChaseOptions::default());
        assert!(dsp.prepare_timeline_render(2, 0, 0, 2, true));
        dsp.render_block(&status, 2);
        assert!(
            dsp.voices
                .iter()
                .any(|voice| voice.active && voice.timeline_note_id.is_some())
        );

        let (mut retired, _reclaimed) = test_reclaimer();
        dsp.handle(
            AudioCommand::NoteOn {
                note: 72,
                velocity: 1.0,
                mixer_track: 1,
            },
            &mut retired,
        );
        let (mut command_tx, _command_rx) = RingBuffer::new(1);
        command_tx.push(AudioCommand::SetMaster(0.5)).unwrap();
        assert_eq!(command_tx.slots(), 0);

        let mailbox = TransportMailbox::default();
        let mut transport = RealtimeTransport::default();
        mailbox.publish(TransportMutation::SetPlaying(true));
        transport.apply_latest_request(&mailbox, &status, &mut dsp);
        mailbox.publish(TransportMutation::SetPlaying(false));
        transport.apply_latest_request(&mailbox, &status, &mut dsp);

        assert!(
            dsp.voices
                .iter()
                .all(|voice| !voice.active || voice.timeline_note_id.is_none())
        );
        assert!(
            dsp.voices
                .iter()
                .any(|voice| voice.active && voice.timeline_note_id.is_none())
        );
        assert_eq!(dsp.timeline_runtime.as_ref().unwrap().next_frame(), Some(2));
        assert!(!status.playing.load(Ordering::Acquire));
        assert_eq!(command_tx.slots(), 0);
    }

    #[test]
    fn timeline_clear_immediately_invalidates_channel_bases_and_owned_voices() {
        let mut project = timeline_test_project(frame_as_beat(64));
        let mut channel = timeline_test_channel(9, 2);
        channel.muted = true;
        channel.volume = 0.25;
        channel.pan = 1.0;
        project.channels.push(channel);
        let mut enabled = [false; 16];
        enabled[0] = true;
        project.patterns.push(Pattern {
            id: 1,
            name: "clear".into(),
            length_steps: 16,
            channel_steps: vec![enabled],
            notes: Vec::new(),
        });
        project
            .clips
            .push(timeline_pattern_clip(1, frame_as_beat(32), 1));
        let timeline = compile_timeline_test_project(&project);
        let (mut controller, mut dsp, status) =
            install_and_activate_timeline(timeline, 122, 2, 0, TimelineChaseOptions::default());
        assert!(dsp.prepare_timeline_render(2, 0, 0, 2, true));
        dsp.render_block(&status, 2);
        assert!(dsp.timeline_channel_bases.get(9).is_some());
        assert!(
            dsp.voices
                .iter()
                .any(|voice| voice.active && voice.timeline_note_id.is_some())
        );

        controller.clear(122).unwrap();
        assert_eq!(dsp.apply_pending_timeline_commands(), 1);
        assert!(dsp.timeline_channel_bases.get(9).is_none());
        assert_eq!(
            dsp.timeline_channel_bases.applied_automation_target_count(),
            0
        );
        assert_eq!(dsp.timeline_channel_revision, None);
        assert_eq!(dsp.timeline_channel_epoch, None);
        assert!(
            dsp.voices
                .iter()
                .all(|voice| !voice.active || voice.timeline_note_id.is_none())
        );
    }

    #[test]
    fn timeline_native_note_off_uses_stable_id_for_overlapping_same_pitch() {
        let mut project = timeline_test_project(frame_as_beat(32));
        project.channels.push(timeline_test_channel(7, 1));
        project.patterns.push(Pattern {
            id: 1,
            name: "overlap".into(),
            length_steps: 16,
            channel_steps: vec![[false; 16]],
            notes: vec![
                PianoNote {
                    id: 11,
                    channel_id: Some(7),
                    group_id: None,
                    note: 64,
                    start: 0.0,
                    length: frame_as_beat(4),
                    velocity: 1.0,
                    selected: false,
                    muted: false,
                },
                PianoNote {
                    id: 12,
                    channel_id: Some(7),
                    group_id: None,
                    note: 64,
                    start: frame_as_beat(1),
                    length: frame_as_beat(8),
                    velocity: 1.0,
                    selected: false,
                    muted: false,
                },
            ],
        });
        project
            .clips
            .push(timeline_pattern_clip(1, frame_as_beat(16), 1));
        let timeline = compile_timeline_test_project(&project);
        let mut starts = timeline
            .events()
            .iter()
            .filter_map(|event| match event.kind {
                TimelineEventKind::NoteOn { note_id, .. } => Some((event.frame, note_id)),
                _ => None,
            })
            .collect::<Vec<_>>();
        starts.sort_unstable();
        assert_eq!(starts.len(), 2);
        let first_id = starts[0].1;
        let second_id = starts[1].1;
        let first_off = timeline
            .events()
            .iter()
            .find_map(|event| match event.kind {
                TimelineEventKind::NoteOff { note_id, .. } if note_id == first_id => {
                    Some(event.frame)
                }
                _ => None,
            })
            .unwrap();
        let second_off = timeline
            .events()
            .iter()
            .find_map(|event| match event.kind {
                TimelineEventKind::NoteOff { note_id, .. } if note_id == second_id => {
                    Some(event.frame)
                }
                _ => None,
            })
            .unwrap();
        let (_controller, mut dsp, status) =
            install_and_activate_timeline(timeline, 102, 2, 0, TimelineChaseOptions::default());

        let first_frames = usize::try_from(first_off + 1).unwrap();
        assert!(dsp.prepare_timeline_render(2, 0, 0, first_frames, true));
        dsp.render_block(&status, first_frames);
        assert!(
            !dsp.voices
                .iter()
                .any(|voice| voice.active && voice.timeline_note_id == Some(first_id))
        );
        assert!(
            dsp.voices
                .iter()
                .any(|voice| voice.active && voice.timeline_note_id == Some(second_id))
        );

        let remaining = usize::try_from(second_off + 1 - first_off - 1).unwrap();
        assert!(dsp.prepare_timeline_render(2, first_off + 1, 0, remaining, true));
        dsp.render_block(&status, remaining);
        assert!(!dsp.voices.iter().any(|voice| voice.active));
    }

    #[test]
    fn timeline_executor_failure_aborts_cursor_and_silences_the_whole_block() {
        let project = timeline_test_project(frame_as_beat(64));
        let timeline = compile_timeline_test_project(&project);
        let (_controller, mut dsp, status) =
            install_and_activate_timeline(timeline, 103, 2, 0, TimelineChaseOptions::default());
        // Deliberately replace the initialized executor to exercise the
        // fail-closed boundary after runtime packet copying.
        dsp.timeline_executor = TimelineExecutor::new();
        dsp.master_block[..8].fill([1.0; 2]);
        let ready = dsp.prepare_timeline_render(2, 0, 0, 8, true);
        if !ready {
            dsp.master_block[..8].fill([0.0; 2]);
        }
        assert!(!ready);
        assert_eq!(dsp.master_block[..8], [[0.0; 2]; 8]);
        let runtime = dsp.timeline_runtime.as_ref().unwrap();
        assert_eq!(runtime.next_frame(), Some(0));
        assert!(runtime.stats().ownership_needs_resync);
        assert_eq!(dsp.timeline_execution_failures, 1);

        // The failed transaction stays silent, but the following block must
        // allow the App-selected legacy fallback instead of becoming a
        // permanent active-epoch black hole.
        let (mut retired, _reclaimed) = test_reclaimer();
        dsp.handle(
            AudioCommand::NoteOn {
                note: 72,
                velocity: 1.0,
                mixer_track: 0,
            },
            &mut retired,
        );
        assert!(dsp.prepare_timeline_render(2, 0, 0, 8, true));
        dsp.render_block(&status, 8);
        assert!(
            dsp.master_block[..8]
                .iter()
                .any(|sample| sample[0].abs() > f32::EPSILON)
        );
        assert_eq!(dsp.timeline_runtime.as_ref().unwrap().next_frame(), Some(0));
    }

    #[test]
    fn unconsumed_render_plan_poisons_the_committed_end_cursor() {
        let project = timeline_test_project(frame_as_beat(64));
        let timeline = compile_timeline_test_project(&project);
        let (_controller, mut dsp, status) =
            install_and_activate_timeline(timeline, 104, 2, 0, TimelineChaseOptions::default());
        assert!(dsp.prepare_timeline_render(2, 0, 0, 4, true));
        dsp.timeline_plan.events[0] = Some(TimelinePlannedEvent {
            sample_offset: 4,
            kind: TimelinePlannedEventKind::NoteOn(chased_note(99, 1, 60)),
        });
        dsp.timeline_plan.len = 1;
        dsp.render_block(&status, 4);

        let runtime = dsp.timeline_runtime.as_ref().unwrap();
        assert_eq!(runtime.next_frame(), Some(4));
        assert!(runtime.stats().ownership_needs_resync);
        assert!(dsp.prepare_timeline_render(2, 4, 0, 4, true));
        assert!(!dsp.timeline_plan_render_active);
        assert_eq!(dsp.timeline_runtime.as_ref().unwrap().next_frame(), Some(4));
    }

    #[test]
    fn no_installed_timeline_preserves_legacy_audio_command_rendering() {
        let mut dsp = DspState::new(48_000.0);
        let status = AudioStatus::default();
        status.playing.store(true, Ordering::Release);
        let (mut retired, _reclaimed) = test_reclaimer();
        dsp.handle(
            AudioCommand::NoteOn {
                note: 60,
                velocity: 1.0,
                mixer_track: 1,
            },
            &mut retired,
        );
        assert!(dsp.prepare_timeline_render(1, 0, 0, 8, true));
        dsp.render_block(&status, 8);
        assert!(
            dsp.master_block[..8]
                .iter()
                .any(|frame| frame[0] != 0.0 || frame[1] != 0.0)
        );
    }

    #[test]
    fn timeline_audio_chase_starts_at_native_source_once_and_loop_restarts_exactly() {
        let mut project = timeline_test_project(frame_as_beat(64));
        project.audio_assets.push(AudioAsset {
            id: 77,
            name: "resident".into(),
            path: PathBuf::from("resident.wav"),
            sample_rate: 48_000,
            channels: 1,
            bits_per_sample: 32,
            frames: 64,
            waveform_peaks: Vec::new(),
        });
        let mut clip = timeline_pattern_clip(9, frame_as_beat(32), 0);
        clip.kind = ClipKind::Audio;
        clip.audio_asset_id = Some(77);
        project.clips.push(clip);
        let timeline = compile_timeline_test_project(&project);
        let options = TimelineChaseOptions::default();
        let (mut controller, mut dsp, status) =
            install_and_activate_timeline(Arc::clone(&timeline), 105, 2, 5, options);
        let (mut retired, _reclaimed) = test_reclaimer();
        register_test_asset(&mut dsp, &mut retired, 77, &[1.0; 64], 48_000, 1);

        assert!(dsp.prepare_timeline_render(2, 5, 0, 2, true));
        dsp.render_block(&status, 2);
        let voice = dsp
            .audio_voices
            .iter()
            .find(|voice| voice.active && voice.timeline_asset_id == Some(77))
            .unwrap();
        assert_eq!(voice.source_position, 7.0);
        assert_eq!(voice.timeline_frame, 7);

        assert!(dsp.prepare_timeline_render(2, 7, 0, 2, true));
        dsp.render_block(&status, 2);
        let voice = dsp
            .audio_voices
            .iter()
            .find(|voice| voice.active && voice.timeline_asset_id == Some(77))
            .unwrap();
        assert_eq!(
            voice.source_position, 9.0,
            "chase must not replay each block"
        );

        let loop_chase = controller
            .prepare_loop_chase(&timeline, 105, 5, options)
            .unwrap();
        controller.install_loop_chase(loop_chase).unwrap();
        assert_eq!(dsp.apply_pending_timeline_commands(), 1);
        dsp.apply_transport_discontinuity(&status, 3, 0, 5, TransportDiscontinuity::Loop);
        assert_eq!(dsp.timeline_automation.epoch(), 3);
        assert_eq!(dsp.timeline_automation.next_frame(), 5);
        assert!(dsp.prepare_timeline_render(3, 5, 0, 1, true));
        assert_eq!(dsp.timeline_automation.next_frame(), 6);
        dsp.render_block(&status, 1);
        let voice = dsp
            .audio_voices
            .iter()
            .find(|voice| voice.active && voice.timeline_asset_id == Some(77))
            .unwrap();
        assert_eq!(voice.source_position, 6.0);
        assert_eq!(voice.timeline_frame, 6);
    }

    #[test]
    fn rejected_atomic_candidate_preserves_a_transport_voice_executor_and_pdc_history() {
        let mut first_project = timeline_test_project(4.0);
        first_project.channels.push(timeline_test_channel(77, 1));
        push_timeline_automation(
            &mut first_project,
            30,
            AutomationTarget::ChannelVolume { channel: 77 },
            AutomationCurve::Hold,
            [AutomationPoint::new(0.0, 0.35)],
        );
        let first = compile_timeline_test_project(&first_project);
        let (mut controller, mut dsp, status) = install_and_activate_timeline(
            Arc::clone(&first),
            301,
            1,
            0,
            TimelineChaseOptions::default(),
        );
        while controller.poll_event().is_some() {}
        let first_loop = controller
            .prepare_loop_chase(&first, 301, 0, TimelineChaseOptions::default())
            .unwrap();
        controller.install_loop_chase(first_loop).unwrap();
        dsp.apply_pending_timeline_commands();
        while controller.poll_event().is_some() {}

        let mut replacement_project = timeline_test_project(4.0);
        replacement_project.audio_assets.push(AudioAsset {
            id: 9_001,
            name: "missing callback PCM".into(),
            path: PathBuf::from("missing.wav"),
            sample_rate: 48_000,
            channels: 1,
            bits_per_sample: 32,
            frames: 96_000,
            waveform_peaks: Vec::new(),
        });
        let mut clip = timeline_pattern_clip(44, 4.0, 0);
        clip.kind = ClipKind::Audio;
        clip.audio_asset_id = Some(9_001);
        replacement_project.clips.push(clip);
        let replacement = compile_timeline_test_project(&replacement_project);
        let replacement_loop = controller
            .prepare_loop_chase(&replacement, 302, 0, TimelineChaseOptions::default())
            .unwrap();
        let replacement_chase = controller
            .prepare_chase(&replacement, 302, 2, 0, TimelineChaseOptions::default())
            .unwrap();
        install_test_timeline(&mut controller, 302, replacement);
        controller.install_loop_chase(replacement_loop).unwrap();
        controller.install_chase(replacement_chase).unwrap();
        assert_eq!(dsp.apply_pending_timeline_commands(), 3);
        let mut replacement_token = None;
        for _ in 0..3 {
            if let TimelineRuntimeEvent::LoopChaseInstalled { token, .. } =
                controller.poll_event().unwrap()
            {
                replacement_token = Some(token);
            }
        }
        let replacement_token = replacement_token.unwrap();

        dsp.voices[0].active = true;
        dsp.voices[0].timeline_note_id = Some(77);
        dsp.pdc_raw_track_delays[0].request_delay(1, 0).unwrap();
        assert_eq!(
            dsp.pdc_raw_track_delays[0].process_sample([1.0, 1.0]),
            [0.0, 0.0]
        );
        let executor_epoch = dsp.timeline_executor.epoch();
        let automation_epoch = dsp.timeline_automation.epoch();
        let automation_next_frame = dsp.timeline_automation.next_frame();
        let automation_stats = dsp.timeline_automation.stats();
        let automation_master = dsp
            .timeline_automation
            .value_for(CompiledAutomationTarget::MasterVolume);
        let automation_channel = dsp
            .timeline_automation
            .value_for(CompiledAutomationTarget::ChannelVolume { channel_id: 77 });
        let channel_binding = dsp.timeline_channel_bases.get(77).unwrap();
        assert!(channel_binding.volume_automation_slot.is_some());
        dsp.track_pans[5] = 0.625;
        let active_track_pan = dsp.track_pans[5];
        let transport_before = RealtimeTransport {
            request: TransportRequest {
                request_id: 8,
                playing: true,
                loop_enabled: true,
                loop_start_frame: 0,
                loop_end_frame: 96_000,
                loop_end_q32: 4 << 32,
                ..TransportRequest::default()
            },
            epoch: 1,
            ..RealtimeTransport::default()
        };
        let mut transport = transport_before;
        let mut mixer_pan_release = TimelineMixerPanRelease::EMPTY;
        assert!(mixer_pan_release.insert(5, -0.75));
        let spec = TimelineTransportActivationSpec {
            revision: 302,
            target_epoch: 2,
            minimum_epoch: 2,
            frame: 0,
            beat_q32: 0,
            loop_start_frame: 0,
            loop_end_frame: 96_000,
            loop_start_q32: 0,
            loop_end_q32: 4 << 32,
            loop_token: replacement_token,
            loop_enabled: true,
            playing: true,
            mixer_pan_release,
        };
        let request_id = controller.activate_transport(spec, 8).unwrap();
        assert_eq!(dsp.apply_pending_timeline_commands(), 1);
        assert_eq!(
            dsp.track_pans[5].to_bits(),
            active_track_pan.to_bits(),
            "a queued candidate must not release A's mixer pan"
        );
        transport.apply_pending_timeline_activation(&status, &mut dsp);

        assert_eq!(transport, transport_before);
        assert_eq!(
            dsp.timeline_runtime.as_ref().unwrap().active_revision(),
            Some(301)
        );
        assert_eq!(
            dsp.timeline_runtime.as_ref().unwrap().active_epoch(),
            Some(1)
        );
        assert_eq!(dsp.timeline_executor.epoch(), executor_epoch);
        assert_eq!(dsp.timeline_automation.epoch(), automation_epoch);
        assert_eq!(dsp.timeline_automation.next_frame(), automation_next_frame);
        assert_eq!(dsp.timeline_automation.stats(), automation_stats);
        assert_eq!(
            dsp.timeline_automation
                .value_for(CompiledAutomationTarget::MasterVolume),
            automation_master
        );
        assert_eq!(
            dsp.timeline_automation
                .value_for(CompiledAutomationTarget::ChannelVolume { channel_id: 77 }),
            automation_channel
        );
        assert_eq!(dsp.timeline_channel_bases.get(77), Some(channel_binding));
        assert_eq!(
            dsp.track_pans[5].to_bits(),
            active_track_pan.to_bits(),
            "rejected preflight must preserve every active mixer-pan bit"
        );
        assert!(dsp.voices[0].active);
        assert_eq!(dsp.voices[0].timeline_note_id, Some(77));
        assert_eq!(
            dsp.pdc_raw_track_delays[0].process_sample([0.0, 0.0]),
            [1.0, 1.0],
            "rejected preflight must not reset delayed PDC history"
        );
        assert!(matches!(
            controller.poll_event(),
            Some(TimelineRuntimeEvent::TransportActivationRejected {
                request_id: actual,
                revision: 302,
                reason: TimelineTransportActivationRejectReason::MissingAudioAsset {
                    asset_id: 9_001
                },
            }) if actual == request_id
        ));
    }

    #[test]
    fn atomic_activation_releases_mixer_pan_after_preflight_and_before_identity_publish() {
        let first = compile_timeline_test_project(&timeline_test_project(4.0));
        let (mut controller, mut dsp, status) =
            install_and_activate_timeline(first, 601, 1, 0, TimelineChaseOptions::default());
        while controller.poll_event().is_some() {}

        let replacement = compile_timeline_test_project(&timeline_test_project(4.0));
        let replacement_loop = controller
            .prepare_loop_chase(&replacement, 602, 0, TimelineChaseOptions::default())
            .unwrap();
        let replacement_chase = controller
            .prepare_chase(&replacement, 602, 2, 0, TimelineChaseOptions::default())
            .unwrap();
        install_test_timeline(&mut controller, 602, replacement);
        controller.install_loop_chase(replacement_loop).unwrap();
        controller.install_chase(replacement_chase).unwrap();
        assert_eq!(dsp.apply_pending_timeline_commands(), 3);
        let mut replacement_token = None;
        for _ in 0..3 {
            if let Some(TimelineRuntimeEvent::LoopChaseInstalled { token, .. }) =
                controller.poll_event()
            {
                replacement_token = Some(token);
            }
        }

        dsp.track_pans[7] = 0.625;
        let mut mixer_pan_release = TimelineMixerPanRelease::EMPTY;
        assert!(mixer_pan_release.insert(7, -0.375));
        let spec = TimelineTransportActivationSpec {
            revision: 602,
            target_epoch: 2,
            minimum_epoch: 2,
            frame: 0,
            beat_q32: 0,
            loop_start_frame: 0,
            loop_end_frame: 96_000,
            loop_start_q32: 0,
            loop_end_q32: 4 << 32,
            loop_token: replacement_token.unwrap(),
            loop_enabled: true,
            playing: true,
            mixer_pan_release,
        };
        let request_id = controller.activate_transport(spec, 0).unwrap();
        assert_eq!(dsp.apply_pending_timeline_commands(), 1);
        let ticket = dsp.pending_timeline_transport_activation().unwrap();

        assert_eq!(dsp.track_pans[7].to_bits(), 0.625_f32.to_bits());
        assert_eq!(controller.confirmed_identity_pair(), Some((601, 1)));
        dsp.preflight_timeline_transport_activation(ticket, 2)
            .unwrap();
        assert_eq!(
            dsp.track_pans[7].to_bits(),
            0.625_f32.to_bits(),
            "preflight is scratch-only"
        );

        let committed = dsp.commit_timeline_transport_activation(&status, ticket, 2);
        assert_eq!(
            dsp.track_pans[7].to_bits(),
            (-0.375_f32).to_bits(),
            "B's first render observes the released mixer pan"
        );
        assert_eq!(
            controller.confirmed_identity_pair(),
            Some((601, 1)),
            "active identity remains A until every DSP member commits"
        );
        dsp.publish_timeline_transport_activation(committed);
        assert_eq!(controller.confirmed_identity_pair(), Some((602, 2)));
        assert_eq!(
            controller.poll_event(),
            Some(TimelineRuntimeEvent::TransportActivationApplied {
                request_id,
                revision: 602,
                epoch: 2,
                frame: 0,
            })
        );
    }

    #[test]
    fn timeline_activation_queue_rejection_never_releases_active_mixer_pan() {
        let first = compile_timeline_test_project(&timeline_test_project(4.0));
        let (mut controller, mut dsp, _status) =
            install_and_activate_timeline(first, 611, 1, 0, TimelineChaseOptions::default());
        while controller.poll_event().is_some() {}
        dsp.track_pans[9] = 0.875;

        let mut queued = 0_u64;
        loop {
            match controller.clear(10_000 + queued) {
                Ok(_) => queued += 1,
                Err(TimelineControlError::CommandQueueFull) => break,
                Err(error) => panic!("unexpected queue fill error: {error}"),
            }
        }
        assert!(queued > 0);

        let mut mixer_pan_release = TimelineMixerPanRelease::EMPTY;
        assert!(mixer_pan_release.insert(9, -0.875));
        let spec = TimelineTransportActivationSpec {
            revision: 612,
            target_epoch: 2,
            minimum_epoch: 2,
            frame: 0,
            beat_q32: 0,
            loop_start_frame: 0,
            loop_end_frame: 96_000,
            loop_start_q32: 0,
            loop_end_q32: 4 << 32,
            loop_token: 1,
            loop_enabled: true,
            playing: true,
            mixer_pan_release,
        };
        assert_eq!(
            controller.activate_transport(spec, 0),
            Err(TimelineControlError::CommandQueueFull)
        );
        assert_eq!(dsp.track_pans[9].to_bits(), 0.875_f32.to_bits());
        assert_eq!(controller.confirmed_identity_pair(), Some((611, 1)));
    }

    #[test]
    fn rejected_candidate_after_binding_preflight_preserves_active_channel_slots_and_values() {
        let mut active_project = timeline_test_project(4.0);
        active_project.channels.push(timeline_test_channel(41, 1));
        push_timeline_automation(
            &mut active_project,
            41,
            AutomationTarget::ChannelVolume { channel: 41 },
            AutomationCurve::Hold,
            [AutomationPoint::new(0.0, 0.4)],
        );
        let active = compile_timeline_test_project(&active_project);
        let (mut controller, mut dsp, _status) =
            install_and_activate_timeline(active, 401, 1, 0, TimelineChaseOptions::default());
        while controller.poll_event().is_some() {}
        let active_row = dsp.timeline_channel_bases.get(41).unwrap();
        let active_value = dsp
            .timeline_automation
            .value_for(CompiledAutomationTarget::ChannelVolume { channel_id: 41 });
        dsp.track_pans[6] = 0.45;
        let active_track_pan = dsp.track_pans[6];

        let mut candidate_project = timeline_test_project(4.0);
        candidate_project
            .channels
            .push(timeline_test_channel(42, 1));
        let mut generator = timeline_test_channel(43, 1);
        generator.instrument_plugin_instance_id = Some(4_300);
        candidate_project.channels.push(generator);
        candidate_project
            .plugin_instances
            .push(timeline_test_plugin(4_300));
        push_timeline_automation(
            &mut candidate_project,
            42,
            AutomationTarget::ChannelPan { channel: 42 },
            AutomationCurve::Hold,
            [AutomationPoint::new(0.0, 0.75)],
        );
        let candidate = compile_timeline_test_project(&candidate_project);
        let loop_chase = controller
            .prepare_loop_chase(&candidate, 402, 0, TimelineChaseOptions::default())
            .unwrap();
        let one_shot = controller
            .prepare_chase(&candidate, 402, 2, 0, TimelineChaseOptions::default())
            .unwrap();
        install_test_timeline(&mut controller, 402, candidate);
        controller.install_loop_chase(loop_chase).unwrap();
        controller.install_chase(one_shot).unwrap();
        assert_eq!(dsp.apply_pending_timeline_commands(), 3);
        let mut loop_token = None;
        for _ in 0..3 {
            if let Some(TimelineRuntimeEvent::LoopChaseInstalled { token, .. }) =
                controller.poll_event()
            {
                loop_token = Some(token);
            }
        }
        let mut mixer_pan_release = TimelineMixerPanRelease::EMPTY;
        assert!(mixer_pan_release.insert(6, -0.45));
        let spec = TimelineTransportActivationSpec {
            revision: 402,
            target_epoch: 2,
            minimum_epoch: 2,
            frame: 0,
            beat_q32: 0,
            loop_start_frame: 0,
            loop_end_frame: 96_000,
            loop_start_q32: 0,
            loop_end_q32: 4 << 32,
            loop_token: loop_token.unwrap(),
            loop_enabled: true,
            playing: true,
            mixer_pan_release,
        };
        controller.activate_transport(spec, 0).unwrap();
        assert_eq!(dsp.apply_pending_timeline_commands(), 1);
        let ticket = dsp.pending_timeline_transport_activation().unwrap();
        let reason = dsp
            .preflight_timeline_transport_activation(ticket, 2)
            .unwrap_err();
        assert_eq!(
            reason,
            TimelineTransportActivationRejectReason::GeneratorRouteBinding
        );
        let staged_row = dsp.timeline_activation_channel_bases.get(42).unwrap();
        assert!(staged_row.pan_automation_slot.is_some());
        dsp.reject_timeline_transport_activation(ticket, reason);

        assert_eq!(dsp.timeline_channel_bases.get(41), Some(active_row));
        assert!(dsp.timeline_channel_bases.get(42).is_none());
        assert_eq!(
            dsp.timeline_automation
                .value_for(CompiledAutomationTarget::ChannelVolume { channel_id: 41 }),
            active_value
        );
        assert_eq!(dsp.timeline_automation.epoch(), 1);
        assert_eq!(
            dsp.track_pans[6].to_bits(),
            active_track_pan.to_bits(),
            "late preflight rejection must preserve A's mixer pan"
        );
    }

    #[test]
    fn timeline_audio_fades_are_half_open_and_chase_uses_absolute_frame() {
        let mut dsp = DspState::new(48_000.0);
        let (mut retired, _reclaimed) = test_reclaimer();
        register_test_asset(&mut dsp, &mut retired, 88, &[1.0; 32], 48_000, 1);
        let descriptor = AudioClipDescriptor {
            clip_id: 7,
            asset_id: 88,
            start_frame: 10,
            source_offset_frame: 0,
            source_elapsed_frames: 0,
            timeline_sample_rate: 48_000,
            source_sample_rate: 48_000,
            clip_end_frame: 16,
            stop_frame: 16,
            gain: 1.0,
            fades: CompiledClipFades {
                fade_in_start: 10,
                fade_out_end: 16,
                fade_in_frames: 2,
                fade_out_frames: 2,
            },
            mixer_track: 1,
        };

        dsp.play_timeline_audio_clip(
            ChasedAudioClip {
                descriptor,
                source_position_frame: 0.0,
            },
            10,
        );
        let route = MAX_MIXER_BLOCK_FRAMES;
        dsp.track_block[route..route + MAX_MIXER_BLOCK_FRAMES].fill([0.0; 2]);
        dsp.render_audio_frame(0);
        assert_eq!(dsp.track_block[route], [0.0; 2]);
        dsp.render_audio_frame(1);
        assert_eq!(dsp.track_block[route + 1], [1.0; 2]);

        dsp.play_timeline_audio_clip(
            ChasedAudioClip {
                descriptor,
                source_position_frame: 5.0,
            },
            15,
        );
        dsp.render_audio_frame(2);
        assert_eq!(dsp.track_block[route + 2], [0.0; 2]);
        assert!(!dsp.audio_voices.iter().any(|voice| voice.active));

        dsp.play_timeline_audio_clip(
            ChasedAudioClip {
                descriptor,
                source_position_frame: 6.0,
            },
            16,
        );
        assert!(!dsp.audio_voices.iter().any(|voice| voice.active));
    }

    // Exercise the production edit, compiler, scheduler and callback together.
    // The source buffers are deliberately independent of the compiled metadata.
    fn split_callback_timelines(
        native_rate: u32,
        output_rate: u32,
        fade_in: f32,
        fade_out: f32,
    ) -> [Arc<CompiledTimeline>; 3] {
        let mut project = timeline_test_project(0.5);
        project.audio_assets.push(AudioAsset {
            id: 89,
            name: "Split callback PCM fixture".into(),
            path: PathBuf::from("not-decoded-by-this-test.wav"),
            sample_rate: native_rate,
            channels: 2,
            bits_per_sample: 24,
            frames: 8_192,
            waveform_peaks: Vec::new(),
        });
        let map = TempoMap::from_project(&project, output_rate).unwrap();
        let mut original = timeline_pattern_clip(90, 0.1289, 0);
        original.kind = ClipKind::Audio;
        original.start = 0.0073;
        original.audio_asset_id = Some(89);
        original.audio_source_offset_frame = Some(137);
        original.gain = 0.625;
        original.fade_in = fade_in;
        original.fade_out = fade_out;
        let (left, right) = crate::audio_clip::split_audio_clip(
            &original,
            original.start + 0.0317,
            91,
            Some(&map),
            project.tempo,
        )
        .unwrap();
        let (middle, last) = crate::audio_clip::split_audio_clip(
            &right,
            original.start + 0.0793,
            92,
            Some(&map),
            project.tempo,
        )
        .unwrap();
        [
            vec![original],
            vec![left.clone(), right],
            vec![left, middle, last],
        ]
        .map(|clips| {
            let expected_clips = clips.len();
            project.clips = clips;
            let timeline =
                CompiledTimeline::from_project(&project, &map, TimelineCompileOptions::default())
                    .unwrap();
            assert_eq!(timeline.audio_clips().len(), expected_clips);
            Arc::new(timeline)
        })
    }

    fn split_callback_samples(kind: usize) -> Vec<f32> {
        (0..8_192)
            .flat_map(|frame| match kind {
                0 => [0.375, -0.25],
                1 => [
                    (frame % 257) as f32 / 256.0 - 0.5,
                    ((frame * 17) % 509) as f32 / 512.0 - 0.5,
                ],
                2 => [
                    if frame % 113 == 0 { 0.8 } else { 0.0 },
                    if frame % 79 == 3 { -0.7 } else { 0.0 },
                ],
                _ => unreachable!("only DC, ramp and impulse fixtures are defined"),
            })
            .collect()
    }

    fn render_split_callback_pcm(
        timeline: Arc<CompiledTimeline>,
        samples: &[f32],
        native_rate: u32,
        start_frame: u64,
        end_frame: u64,
        partitions: &[usize],
    ) -> Vec<[u32; 2]> {
        let (mut controller, realtime) = create_timeline_runtime();
        let chase = controller
            .prepare_chase(
                &timeline,
                1,
                3,
                start_frame,
                TimelineChaseOptions::default(),
            )
            .unwrap();
        let mut dsp = DspState::try_new_inner(
            timeline.sample_rate() as f32,
            None,
            None,
            None,
            None,
            512,
            Some(realtime),
        )
        .unwrap();
        let (mut retired, _reclaimed) = test_reclaimer();
        register_test_asset(&mut dsp, &mut retired, 89, samples, native_rate, 2);
        install_test_timeline(&mut controller, 1, timeline);
        controller.install_chase(chase).unwrap();
        assert_eq!(dsp.apply_pending_timeline_commands(), 2);
        // No plugin history or nonzero PDC is present; master gain/tanh is
        // memoryless. These short fixtures finish before the first metronome beat.
        assert!(dsp.insert_endpoints.iter().all(Option::is_none));
        assert!(dsp.generator_endpoints.iter().all(Option::is_none));
        let status = AudioStatus::default();
        status.playing.store(true, Ordering::Release);
        dsp.apply_transport_discontinuity(
            &status,
            3,
            0,
            start_frame,
            TransportDiscontinuity::OneShot,
        );

        let mut pcm = Vec::new();
        let mut position = start_frame;
        let mut partition = 0;
        while position < end_frame {
            let frames =
                partitions[partition % partitions.len()].min((end_frame - position) as usize);
            assert!(frames > 0 && frames <= MAX_MIXER_BLOCK_FRAMES);
            assert!(dsp.prepare_timeline_render(3, position, 0, frames, true));
            // This calls apply_timeline_events_for_frame and render_audio_frame,
            // including linear interpolation and the real AudioClipVoice clock.
            dsp.render_block(&status, frames);
            pcm.extend(
                dsp.master_block[..frames]
                    .iter()
                    .map(|frame| [frame[0].to_bits(), frame[1].to_bits()]),
            );
            position += frames as u64;
            partition += 1;
        }
        let runtime = dsp.timeline_runtime.as_ref().unwrap();
        assert_eq!(runtime.next_frame(), Some(end_frame));
        assert!(!runtime.stats().ownership_needs_resync);
        assert_eq!(dsp.timeline_missing_assets, 0);
        assert!(dsp.audio_voices.iter().all(|voice| !voice.active));
        pcm
    }

    #[test]
    fn timeline_split_callback_pcm_is_bit_exact_for_rates_signals_fades_and_partitions() {
        for (native_rate, output_rate) in [(44_100, 48_000), (48_000, 44_100)] {
            for (fade_in, fade_out) in [
                (0.0, 0.0),
                (0.75, 0.625),
                (1.0, 0.0),
                (0.0, 1.0),
                (1.0, 1.0),
            ] {
                let timelines =
                    split_callback_timelines(native_rate, output_rate, fade_in, fade_out);
                let root = timelines[0].audio_clips()[0];
                let end_frame = root.stop_frame + 3;
                for kind in 0..3 {
                    let samples = split_callback_samples(kind);
                    let before = render_split_callback_pcm(
                        timelines[0].clone(),
                        &samples,
                        native_rate,
                        0,
                        end_frame,
                        &[256],
                    );
                    assert!(before.iter().any(|frame| *frame != [0, 0]));
                    assert!(before.iter().any(|frame| frame[0] != frame[1]));
                    assert!(
                        before[..root.start_frame as usize]
                            .iter()
                            .all(|frame| *frame == [0, 0])
                    );
                    assert!(
                        before[root.stop_frame as usize..]
                            .iter()
                            .all(|frame| *frame == [0, 0])
                    );
                    for (edit, timeline) in timelines.iter().enumerate() {
                        for partitions in [&[1, 7, 63, 257][..], &[17, 128, 31, 509][..]] {
                            let after = render_split_callback_pcm(
                                timeline.clone(),
                                &samples,
                                native_rate,
                                0,
                                end_frame,
                                partitions,
                            );
                            assert_eq!(
                                before, after,
                                "native={native_rate}, output={output_rate}, signal={kind}, fades=({fade_in},{fade_out}), edit={edit}, partitions={partitions:?}",
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn timeline_split_callback_chase_matches_continuous_pcm_at_and_around_cuts() {
        for (native_rate, output_rate) in [(44_100, 48_000), (48_000, 44_100)] {
            let timelines = split_callback_timelines(native_rate, output_rate, 1.0, 0.75);
            let root = timelines[0].audio_clips()[0];
            let end_frame = root.stop_frame + 3;
            let samples = split_callback_samples(1);
            let before = render_split_callback_pcm(
                timelines[0].clone(),
                &samples,
                native_rate,
                0,
                end_frame,
                &[251],
            );
            let mut seek_frames = vec![
                root.start_frame,
                root.start_frame + 1,
                root.stop_frame - 1,
                root.stop_frame,
            ];
            for clip in &timelines[2].audio_clips()[1..] {
                seek_frames.extend([clip.start_frame - 1, clip.start_frame, clip.start_frame + 1]);
            }
            for seek_frame in seek_frames {
                for (edit, timeline) in timelines.iter().enumerate() {
                    let chased = render_split_callback_pcm(
                        timeline.clone(),
                        &samples,
                        native_rate,
                        seek_frame,
                        end_frame,
                        &[1, 29, 127, 509],
                    );
                    assert_eq!(
                        before[seek_frame as usize..],
                        chased,
                        "native={native_rate}, output={output_rate}, seek={seek_frame}, edit={edit}",
                    );
                }
            }
        }
    }

    #[test]
    fn timeline_audio_voice_long_clock_is_closed_form_and_chase_deterministic() {
        // A 1 Hz native fixture keeps 24 hours of source addressable without
        // allocating gigabytes; the real callback still resamples every frame.
        let output_rate = 48_000;
        let after_day = 24 * 60 * 60 * u64::from(output_rate);
        let samples = (0..90_000)
            .map(|frame| (frame % 19) as f32 / 32.0)
            .collect::<Vec<_>>();
        let mut dsp = DspState::new(output_rate as f32);
        let (mut retired, _reclaimed) = test_reclaimer();
        register_test_asset(&mut dsp, &mut retired, 93, &samples, 1, 1);
        let descriptor = AudioClipDescriptor {
            clip_id: 94,
            asset_id: 93,
            start_frame: 0,
            source_offset_frame: 137,
            source_elapsed_frames: 0,
            timeline_sample_rate: output_rate,
            source_sample_rate: 1,
            clip_end_frame: after_day + 129,
            stop_frame: after_day + 129,
            gain: 1.0,
            fades: CompiledClipFades::default(),
            mixer_track: 0,
        };
        dsp.play_timeline_audio_clip(
            ChasedAudioClip {
                descriptor,
                source_position_frame: descriptor.source_position_at(after_day, output_rate),
            },
            after_day,
        );
        let mut continuous = Vec::new();
        for frame in after_day..descriptor.stop_frame {
            dsp.track_block[0] = [0.0; 2];
            dsp.render_audio_frame(0);
            continuous.push(dsp.track_block[0].map(f32::to_bits));
            let voice = dsp
                .audio_voices
                .iter()
                .find(|voice| voice.clip_id == 94)
                .unwrap();
            assert_eq!(
                voice.source_position.to_bits(),
                descriptor
                    .source_position_at(frame + 1, output_rate)
                    .to_bits(),
            );
            let rational_position = 137.0
                + ((frame + 1) / u64::from(output_rate)) as f64
                + ((frame + 1) % u64::from(output_rate)) as f64 / f64::from(output_rate);
            assert!(
                (voice.source_position - rational_position).abs()
                    <= 2.0 * f64::EPSILON * rational_position
            );
        }
        let child = AudioClipDescriptor {
            start_frame: after_day - 7,
            source_elapsed_frames: (after_day - 7) as i64,
            ..descriptor
        };
        let seek_frame = after_day + 13;
        dsp.play_timeline_audio_clip(
            ChasedAudioClip {
                descriptor: child,
                source_position_frame: child.source_position_at(seek_frame, output_rate),
            },
            seek_frame,
        );
        for expected in &continuous[13..] {
            dsp.track_block[0] = [0.0; 2];
            dsp.render_audio_frame(0);
            assert_eq!(*expected, dsp.track_block[0].map(f32::to_bits));
        }
        assert!(dsp.audio_voices.iter().all(|voice| !voice.active));

        // Quantify the intentionally changed legacy arithmetic, rather than
        // asserting old incremental playback has identical floating-point PCM.
        for (native_rate, output_rate) in [(44_100, 48_000), (48_000, 44_100)] {
            let frames = output_rate * 60;
            let mut incremental = 137.0;
            for _ in 0..frames {
                incremental += f64::from(native_rate) / f64::from(output_rate);
            }
            let closed_form =
                137.0 + f64::from(frames) * f64::from(native_rate) / f64::from(output_rate);
            assert_eq!(closed_form, 137.0 + f64::from(native_rate) * 60.0);
            let legacy_drift = (incremental - closed_form).abs();
            assert!(
                legacy_drift > 1.0e-6 && legacy_drift < 1.0e-3,
                "legacy drift in source frames: {legacy_drift}"
            );
        }
    }

    #[test]
    fn timeline_missing_asset_is_silent_diagnostic_without_cursor_failure() {
        let mut project = timeline_test_project(frame_as_beat(32));
        project.audio_assets.push(AudioAsset {
            id: 404,
            name: "missing runtime buffer".into(),
            path: PathBuf::from("missing.wav"),
            sample_rate: 48_000,
            channels: 1,
            bits_per_sample: 16,
            frames: 32,
            waveform_peaks: Vec::new(),
        });
        let mut clip = timeline_pattern_clip(4, frame_as_beat(16), 0);
        clip.kind = ClipKind::Audio;
        clip.audio_asset_id = Some(404);
        project.clips.push(clip);
        let timeline = compile_timeline_test_project(&project);
        let (_controller, mut dsp, status) =
            install_and_activate_timeline(timeline, 106, 2, 0, TimelineChaseOptions::default());

        assert!(dsp.prepare_timeline_render(2, 0, 0, 4, true));
        dsp.render_block(&status, 4);
        assert_eq!(dsp.timeline_missing_assets, 1);
        assert_eq!(dsp.timeline_runtime.as_ref().unwrap().next_frame(), Some(4));
        assert!(
            !dsp.timeline_runtime
                .as_ref()
                .unwrap()
                .stats()
                .ownership_needs_resync
        );
    }

    #[test]
    fn timeline_plan_accepts_4096_events_and_rejects_the_4097th() {
        let mut plan = TimelineRenderPlan::new();
        let note = chased_note(1, 1, 60);
        for index in 0..MAX_TIMELINE_PLANNED_EVENTS {
            plan.note_on(
                ChasedNote {
                    note_id: index as u64 + 1,
                    ..note
                },
                0,
            );
        }
        assert_eq!(plan.len, MAX_TIMELINE_PLANNED_EVENTS);
        assert!(!plan.overflowed);
        plan.note_on(
            ChasedNote {
                note_id: MAX_TIMELINE_PLANNED_EVENTS as u64 + 1,
                ..note
            },
            0,
        );
        assert_eq!(plan.len, MAX_TIMELINE_PLANNED_EVENTS);
        assert!(plan.overflowed);
        assert_eq!(TIMELINE_PACKET_CAPACITY, MAX_TIMELINE_PLANNED_EVENTS);
    }

    #[test]
    fn timeline_generator_retriggers_overlaps_and_only_last_instance_sends_note_off() {
        let chain = spawn_empty_chain(DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
        let PluginChain {
            audio,
            control,
            guard,
        } = chain;
        let mut dsp = DspState::new(48_000.0);
        dsp.generator_endpoints[0] = Some(GeneratorEndpointSlot {
            channel_id: 7,
            endpoint_id: 70,
            plugin_instance_id: 700,
            mixer_track: 1,
            endpoint: fixed_adapter(audio),
            pdc_delay: test_pdc_delay(),
            pdc_initialized: false,
            suppress_output_frames: 0,
        });
        let mut plan = TimelineRenderPlan::new();
        plan.note_on(chased_note(1, 7, 64), 1);
        plan.note_on(chased_note(2, 7, 64), 2);
        plan.note_off(chased_note(1, 7, 64), 3);
        plan.note_off(chased_note(2, 7, 64), 4);
        let routes = timeline_generator_route_table(1, 1, 7, 700);
        assert!(DspState::stage_timeline_generator_plan(
            &mut plan,
            &mut dsp.generator_endpoints,
            &mut dsp.timeline_generator_notes,
            &routes,
            1,
            1,
            &mut dsp.fixed_quantum_event_overflows,
        ));
        let endpoint = &dsp.generator_endpoints[0].as_ref().unwrap().endpoint;
        assert_eq!(endpoint.events.scratch.timeline.pending_len, 3);
        let events = &endpoint.events.scratch.timeline.pending[..3];
        assert_eq!(events[0], FrameEvent::midi(1, None, [0x90, 64, 127]));
        assert_eq!(events[1], FrameEvent::midi(2, None, [0x90, 64, 127]));
        assert_eq!(events[2], FrameEvent::midi(4, None, [0x80, 64, 0]));
        assert!(dsp.timeline_generator_notes.iter().all(Option::is_none));

        let slot = dsp.generator_endpoints[0].take().unwrap();
        drop(slot);
        drop(dsp);
        guard.shutdown();
        drop(control);
    }

    #[test]
    fn generator_plugin_parameter_binding_is_exact_and_q128_batch_is_transactional() {
        let last_midi = Arc::new(AtomicU32::new(0));
        let parameter = Arc::new(AtomicU32::new(0));
        let chain = spawn_mock_instrument(
            0.25,
            Arc::clone(&last_midi),
            Arc::clone(&parameter),
            DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES,
        );
        let PluginChain {
            audio,
            control,
            guard,
        } = chain;
        wait_until(|| control.plugin_latency_snapshot().is_some());

        let mut endpoint = fixed_adapter(audio);
        endpoint.manifest = PluginEndpointManifest::identified(&[700]).unwrap();
        assert!(endpoint.refresh_latency_snapshot() || endpoint.coherent_latency.is_some());
        let mut endpoints: [Option<GeneratorEndpointSlot>; MAX_GENERATOR_ENDPOINTS] =
            std::array::from_fn(|_| None);
        endpoints[0] = Some(GeneratorEndpointSlot {
            channel_id: 7,
            endpoint_id: 70,
            plugin_instance_id: 700,
            mixer_track: 1,
            endpoint,
            pdc_delay: test_pdc_delay(),
            pdc_initialized: false,
            suppress_output_frames: 0,
        });
        let routes = timeline_generator_route_table(1, 1, 7, 700);
        let target = CompiledAutomationTarget::PluginParameter {
            instance_id: 700,
            parameter_id: 9,
        };
        let bases = [crate::timeline::AutomationBaseValue { target, value: 0.0 }];
        let driven = [target];
        let generator_route = [CompiledPluginRoute {
            instance_id: 700,
            destination: PluginRouteDestination::Generator {
                channel_id: 7,
                slot: 0,
            },
        }];

        let mut bindings = TimelinePluginAutomationBindings::new();
        let mut insert_endpoints: [Option<InsertEndpointSlot>; TRACK_COUNT] =
            std::array::from_fn(|_| None);
        let pdc_plan = PdcPlan::build([0; TRACK_COUNT], 0, &[], 512).unwrap();
        endpoints[0].as_mut().unwrap().endpoint.manifest =
            PluginEndpointManifest::unknown_for_slots(1).unwrap();
        assert!(!bindings.reset_from(
            1,
            1,
            &bases,
            &driven,
            &generator_route,
            &routes,
            &mut endpoints,
            &mut insert_endpoints,
            &pdc_plan,
            1,
            512,
        ));
        endpoints[0].as_mut().unwrap().endpoint.manifest =
            PluginEndpointManifest::identified(&[701]).unwrap();
        assert!(!bindings.reset_from(
            1,
            1,
            &bases,
            &driven,
            &generator_route,
            &routes,
            &mut endpoints,
            &mut insert_endpoints,
            &pdc_plan,
            1,
            512,
        ));
        endpoints[0].as_mut().unwrap().endpoint.manifest =
            PluginEndpointManifest::identified(&[700, 701]).unwrap();
        assert!(!bindings.reset_from(
            1,
            1,
            &bases,
            &driven,
            &generator_route,
            &routes,
            &mut endpoints,
            &mut insert_endpoints,
            &pdc_plan,
            1,
            512,
        ));
        endpoints[0].as_mut().unwrap().endpoint.manifest =
            PluginEndpointManifest::identified(&[700]).unwrap();
        let wrong_generator_slot = [CompiledPluginRoute {
            instance_id: 700,
            destination: PluginRouteDestination::Generator {
                channel_id: 7,
                slot: 1,
            },
        }];
        assert!(!bindings.reset_from(
            1,
            1,
            &bases,
            &driven,
            &wrong_generator_slot,
            &routes,
            &mut endpoints,
            &mut insert_endpoints,
            &pdc_plan,
            1,
            512,
        ));
        let insert_route = [CompiledPluginRoute {
            instance_id: 700,
            destination: PluginRouteDestination::MixerInsert { track: 1, slot: 0 },
        }];
        assert!(!bindings.reset_from(
            1,
            1,
            &bases,
            &driven,
            &insert_route,
            &routes,
            &mut endpoints,
            &mut insert_endpoints,
            &pdc_plan,
            1,
            512,
        ));
        assert_eq!(bindings.applied_automation_target_count(), 0);
        assert!(control.set_slot_config(
            0,
            SlotConfig {
                enabled: true,
                bypassed: true,
                wet: 1.0,
            },
        ));
        wait_until(|| {
            control
                .plugin_latency_snapshot()
                .is_some_and(|snapshot| snapshot.active_mask == 0)
        });
        endpoints[0]
            .as_mut()
            .unwrap()
            .endpoint
            .refresh_latency_snapshot();
        assert!(!bindings.reset_from(
            1,
            1,
            &bases,
            &driven,
            &generator_route,
            &routes,
            &mut endpoints,
            &mut insert_endpoints,
            &pdc_plan,
            1,
            512,
        ));
        assert!(control.set_slot_config(0, SlotConfig::default()));
        wait_until(|| {
            control
                .plugin_latency_snapshot()
                .is_some_and(|snapshot| snapshot.active_mask == 1)
        });
        assert!(
            endpoints[0]
                .as_mut()
                .unwrap()
                .endpoint
                .refresh_latency_snapshot()
        );
        assert!(bindings.reset_from(
            1,
            1,
            &bases,
            &driven,
            &generator_route,
            &routes,
            &mut endpoints,
            &mut insert_endpoints,
            &pdc_plan,
            1,
            512,
        ));
        assert_eq!(bindings.len, 1);
        assert_eq!(bindings.applied_automation_target_count(), 0);

        let mut output_left = vec![0.0; 64];
        let mut output_right = vec![0.0; 64];
        let status = endpoints[0].as_mut().unwrap().endpoint.process_generator(
            1,
            64,
            &mut output_left,
            &mut output_right,
        );
        assert!(matches!(
            status,
            FixedQuantumProcessStatus::Processed { .. }
        ));
        assert_eq!(
            endpoints[0].as_ref().unwrap().endpoint.input_phase_frames(),
            64
        );

        let frames = 200;
        let mut matrix = TimelineAutomationValueMatrix::new();
        assert!(matrix.begin(frames));
        for frame in 0..frames {
            assert!(matrix.write(frame, 0, target, frame as f32 / frames as f32));
            assert!(matrix.finish_frame(frame, 1));
        }
        assert!(matrix.finish());
        let mut plan = TimelineRenderPlan::new();
        plan.note_on(chased_note(1, 7, 64), 64);
        let notes = [None; MAX_ACTIVE_NOTES];
        let mut staged_notes = [None; MAX_ACTIVE_NOTES];
        let mut control_histories = Vec::with_capacity(TIMELINE_MAX_DRIVEN_PLUGIN_PARAMETERS);
        for index in 0..TIMELINE_MAX_DRIVEN_PLUGIN_PARAMETERS {
            let mut history = Q128ControlHistory::new(512).unwrap();
            history.reset(1, 0.0, 0).unwrap();
            if index == 0 {
                history
                    .begin_block(1, 0, &[0.0; 64])
                    .unwrap()
                    .commit_block();
            }
            control_histories.push(history);
        }
        let mut batch = TimelineEndpointBatchPlan::new_boxed();
        assert!(DspState::prepare_timeline_endpoint_batch(
            &mut plan,
            Some(&matrix),
            &mut endpoints,
            &mut insert_endpoints,
            &notes,
            &mut staged_notes,
            &routes,
            &bindings,
            &mut control_histories,
            1,
            1,
            frames,
            false,
            None,
            1,
            batch.as_mut(),
        ));
        let prepared = batch.prepared_endpoint_at(0).unwrap();
        assert_eq!(prepared.events().len(), 3);
        assert!(matches!(
            prepared.events()[0],
            FrameEvent {
                sample_offset: 64,
                kind: crate::fixed_quantum::FrameEventKind::Parameter { slot: 0, id: 9, .. },
            }
        ));
        assert_eq!(
            prepared.events()[1],
            FrameEvent::midi(64, None, [0x90, 64, 127])
        );
        assert!(matches!(
            prepared.events()[2],
            FrameEvent {
                sample_offset: 192,
                kind: crate::fixed_quantum::FrameEventKind::Parameter { id: 9, .. },
            }
        ));

        let original_snapshot = bindings.bindings[0].unwrap().endpoint_snapshot;
        let mut stale_latency = original_snapshot.latency();
        stale_latency.revision = next_nonzero_id(stale_latency.revision);
        bindings.bindings[0].as_mut().unwrap().endpoint_snapshot =
            PluginEndpointSnapshot::try_new(original_snapshot.manifest(), stale_latency).unwrap();
        assert!(!DspState::preflight_timeline_endpoint_batch_commit(
            batch.as_ref(),
            &mut endpoints,
            &mut insert_endpoints,
            &bindings,
            1,
        ));
        assert_eq!(
            endpoints[0]
                .as_ref()
                .unwrap()
                .endpoint
                .events
                .scratch
                .timeline
                .pending_len,
            0
        );
        assert!(notes.iter().all(Option::is_none));
        bindings.bindings[0].as_mut().unwrap().endpoint_snapshot = original_snapshot;
        assert!(DspState::preflight_timeline_endpoint_batch_commit(
            batch.as_ref(),
            &mut endpoints,
            &mut insert_endpoints,
            &bindings,
            1,
        ));
        DspState::commit_timeline_endpoint_batch(
            batch.as_ref(),
            &mut endpoints,
            &mut insert_endpoints,
        );
        bindings.mark_committed_from_batch(batch.as_ref());
        assert_eq!(bindings.applied_automation_target_count(), 1);
        assert_eq!(
            endpoints[0]
                .as_ref()
                .unwrap()
                .endpoint
                .events
                .scratch
                .timeline
                .pending_len,
            3
        );
        let before_quantized = endpoints[0]
            .as_ref()
            .unwrap()
            .endpoint
            .stats()
            .parameter_events_quantized_to_block_start;
        let mut output_left = vec![0.0; frames];
        let mut output_right = vec![0.0; frames];
        let _ = endpoints[0].as_mut().unwrap().endpoint.process_generator(
            1,
            frames,
            &mut output_left,
            &mut output_right,
        );
        assert_eq!(
            endpoints[0]
                .as_ref()
                .unwrap()
                .endpoint
                .stats()
                .parameter_events_quantized_to_block_start,
            before_quantized
        );
        let mut tail_left = vec![0.0; 120];
        let mut tail_right = vec![0.0; 120];
        let _ = endpoints[0].as_mut().unwrap().endpoint.process_generator(
            1,
            120,
            &mut tail_left,
            &mut tail_right,
        );
        wait_until(|| {
            (f32::from_bits(parameter.load(Ordering::Acquire)) - 192.0 / frames as f32).abs()
                < f32::EPSILON
        });
        assert_eq!(
            endpoints[0]
                .as_ref()
                .unwrap()
                .endpoint
                .stats()
                .parameter_events_quantized_to_block_start,
            before_quantized
        );

        drop(endpoints[0].take());
        guard.shutdown();
        drop(control);
    }

    #[test]
    fn mixer_insert_slot_uses_exact_manifest_shared_batch_and_pdc_delayed_q128_values() {
        let first_latency = Arc::new(AtomicU32::new(65));
        let first_parameter = Arc::new(AtomicU32::new(0));
        let second_parameter = Arc::new(AtomicU32::new(0));
        let chain = PluginChain::spawn_with_backend_factory(
            {
                let first_latency = Arc::clone(&first_latency);
                let first_parameter = Arc::clone(&first_parameter);
                let second_parameter = Arc::clone(&second_parameter);
                move || {
                    vec![
                        BackendSlot::new(Box::new(MockInsertBackend {
                            add: 0.0,
                            gain: 1.0,
                            last_midi: Arc::new(AtomicU32::new(0)),
                            parameter: first_parameter,
                            latency: first_latency,
                            process_delay: Duration::ZERO,
                            delay_left: [0.0; 512],
                            delay_right: [0.0; 512],
                            delay_index: 0,
                        })),
                        BackendSlot::new(Box::new(MockInsertBackend {
                            add: 0.0,
                            gain: 1.0,
                            last_midi: Arc::new(AtomicU32::new(0)),
                            parameter: second_parameter,
                            latency: Arc::new(AtomicU32::new(0)),
                            process_delay: Duration::ZERO,
                            delay_left: [0.0; 512],
                            delay_right: [0.0; 512],
                            delay_index: 0,
                        })),
                    ]
                }
            },
            PluginPrepareConfig {
                sample_rate: 48_000.0,
                max_block_frames: MAX_MIXER_BLOCK_FRAMES,
            },
        )
        .unwrap();
        let PluginChain {
            audio,
            control,
            guard,
        } = chain;
        wait_until(|| {
            control.plugin_latency_snapshot().is_some_and(|snapshot| {
                snapshot.active_mask == 0b11
                    && snapshot.total_plugin_latency_samples == 65
                    && snapshot.slot_latency_samples[0] == 65
            })
        });

        let mut endpoint = fixed_adapter(audio);
        endpoint.manifest = PluginEndpointManifest::identified(&[101, 202]).unwrap();
        assert!(endpoint.refresh_latency_snapshot() || endpoint.coherent_latency.is_some());
        let snapshot = endpoint.exact_endpoint_snapshot().unwrap();
        let snapshot_revision = snapshot.revision();
        let mut insert_endpoints: [Option<InsertEndpointSlot>; TRACK_COUNT] =
            std::array::from_fn(|_| None);
        insert_endpoints[1] = Some(InsertEndpointSlot {
            endpoint_id: 9001,
            endpoint,
            suppress_output_frames: 0,
        });
        let mut generator_endpoints: [Option<GeneratorEndpointSlot>; MAX_GENERATOR_ENDPOINTS] =
            std::array::from_fn(|_| None);
        let mut generator_routes = TimelineGeneratorRouteTable::new();
        let plugin_routes = [
            CompiledPluginRoute {
                instance_id: 101,
                destination: PluginRouteDestination::MixerInsert { track: 1, slot: 0 },
            },
            CompiledPluginRoute {
                instance_id: 202,
                destination: PluginRouteDestination::MixerInsert { track: 1, slot: 1 },
            },
        ];
        assert!(generator_routes.reset_from(7, 11, &plugin_routes, &[]));

        let target = CompiledAutomationTarget::PluginParameter {
            instance_id: 202,
            parameter_id: 9,
        };
        let bases = [crate::timeline::AutomationBaseValue { target, value: 0.0 }];
        let mut insert_latencies = [0_u32; TRACK_COUNT];
        insert_latencies[1] = (DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES * 2 + 65) as u32;
        insert_latencies[2] = (DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES * 2 + 130) as u32;
        let pdc_plan = PdcPlan::build(insert_latencies, 0, &[], 512).unwrap();
        assert_eq!(pdc_plan.raw_track_delay(1).unwrap().requested_samples(), 65);

        let mut bindings = TimelinePluginAutomationBindings::new();
        assert!(bindings.reset_from(
            7,
            11,
            &bases,
            &[target],
            &plugin_routes,
            &generator_routes,
            &mut generator_endpoints,
            &mut insert_endpoints,
            &pdc_plan,
            19,
            512,
        ));
        let binding = bindings.iter().next().unwrap();
        assert_eq!(binding.parameter_slot, 1);
        assert_eq!(binding.control_delay_samples, 130);
        assert_eq!(binding.endpoint_snapshot, snapshot);

        let frames = 384;
        let mut matrix = TimelineAutomationValueMatrix::new();
        assert!(matrix.begin(frames));
        for frame in 0..frames {
            assert!(matrix.write(frame, 0, target, frame as f32 / frames as f32));
            assert!(matrix.finish_frame(frame, 1));
        }
        assert!(matrix.finish());
        let mut control_histories = Vec::with_capacity(TIMELINE_MAX_DRIVEN_PLUGIN_PARAMETERS);
        for index in 0..TIMELINE_MAX_DRIVEN_PLUGIN_PARAMETERS {
            let mut history = Q128ControlHistory::new(512).unwrap();
            if index == 0 {
                history
                    .reset(11, 0.0, binding.control_delay_samples)
                    .unwrap();
            }
            control_histories.push(history);
        }
        let mut plan = TimelineRenderPlan::new();
        let notes = [None; MAX_ACTIVE_NOTES];
        let mut staged_notes = [None; MAX_ACTIVE_NOTES];
        let mut batch = TimelineEndpointBatchPlan::new_boxed();
        assert!(DspState::prepare_timeline_endpoint_batch(
            &mut plan,
            Some(&matrix),
            &mut generator_endpoints,
            &mut insert_endpoints,
            &notes,
            &mut staged_notes,
            &generator_routes,
            &bindings,
            &mut control_histories,
            7,
            11,
            frames,
            false,
            None,
            19,
            batch.as_mut(),
        ));
        assert_eq!(batch.endpoint_count(), 1);
        let prepared = batch.prepared_endpoint_at(0).unwrap();
        assert_eq!(prepared.identity().key.mixer_track(), Some(1));
        assert_eq!(prepared.events().len(), 3);
        let expected = [0.0, 0.0, 126.0 / frames as f32];
        for (index, event) in prepared.events().iter().copied().enumerate() {
            let FrameEvent {
                sample_offset,
                kind:
                    crate::fixed_quantum::FrameEventKind::Parameter {
                        slot,
                        id,
                        normalized,
                        ..
                    },
            } = event
            else {
                panic!("insert batch must contain only parameter samples");
            };
            assert_eq!(usize::from(sample_offset), index * 128);
            assert_eq!(slot, 1);
            assert_eq!(id, 9);
            assert!((normalized - expected[index]).abs() < f32::EPSILON);
        }
        assert!(!DspState::preflight_timeline_endpoint_batch_commit(
            batch.as_ref(),
            &mut generator_endpoints,
            &mut insert_endpoints,
            &bindings,
            20,
        ));
        assert_eq!(
            insert_endpoints[1]
                .as_ref()
                .unwrap()
                .endpoint
                .events
                .scratch
                .timeline
                .pending_len,
            0
        );
        assert!(DspState::preflight_timeline_endpoint_batch_commit(
            batch.as_ref(),
            &mut generator_endpoints,
            &mut insert_endpoints,
            &bindings,
            19,
        ));
        DspState::commit_timeline_endpoint_batch(
            batch.as_ref(),
            &mut generator_endpoints,
            &mut insert_endpoints,
        );
        bindings.mark_committed_from_batch(batch.as_ref());
        DspState::apply_timeline_expected_latency_revisions(
            &bindings,
            &mut generator_endpoints,
            &mut insert_endpoints,
        );
        assert_eq!(bindings.applied_automation_target_count(), 1);
        assert_eq!(
            insert_endpoints[1]
                .as_ref()
                .unwrap()
                .endpoint
                .expected_latency_revision(),
            snapshot_revision
        );

        // A worker publication between PDC planning and activation binding must
        // not create a mixed generation. Binding consumes the still-cached
        // snapshot that produced `pdc_plan`; the fresh precommit read observes
        // the new revision and rejects before any second batch is appended.
        first_latency.store(66, Ordering::Release);
        let input_left = vec![0.0; DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES];
        let input_right = vec![0.0; DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES];
        let mut output_left = vec![0.0; DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES];
        let mut output_right = vec![0.0; DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES];
        let _ = insert_endpoints[1]
            .as_mut()
            .unwrap()
            .endpoint
            .adapter
            .process(
                11,
                &input_left,
                &input_right,
                &mut output_left,
                &mut output_right,
                &[],
            );
        wait_until(|| {
            control.plugin_latency_snapshot().is_some_and(|current| {
                current.revision != snapshot_revision && current.total_plugin_latency_samples == 66
            })
        });
        assert_eq!(
            insert_endpoints[1]
                .as_ref()
                .unwrap()
                .endpoint
                .cached_exact_endpoint_snapshot()
                .unwrap()
                .revision(),
            snapshot_revision
        );
        let mut drift_candidate = TimelinePluginAutomationBindings::new();
        assert!(drift_candidate.reset_from(
            7,
            11,
            &bases,
            &[target],
            &plugin_routes,
            &generator_routes,
            &mut generator_endpoints,
            &mut insert_endpoints,
            &pdc_plan,
            19,
            512,
        ));
        assert_eq!(
            drift_candidate
                .iter()
                .next()
                .unwrap()
                .endpoint_snapshot
                .revision(),
            snapshot_revision
        );
        assert!(!DspState::preflight_timeline_endpoint_batch_commit(
            batch.as_ref(),
            &mut generator_endpoints,
            &mut insert_endpoints,
            &drift_candidate,
            19,
        ));

        drop(insert_endpoints[1].take());
        guard.shutdown();
        drop(control);
    }

    #[test]
    fn master_insert_plugin_automation_stays_explicitly_pending() {
        let target = CompiledAutomationTarget::PluginParameter {
            instance_id: 900,
            parameter_id: 3,
        };
        let bases = [crate::timeline::AutomationBaseValue { target, value: 0.5 }];
        let plugin_routes = [CompiledPluginRoute {
            instance_id: 900,
            destination: PluginRouteDestination::MixerInsert { track: 0, slot: 0 },
        }];
        let mut generator_routes = TimelineGeneratorRouteTable::new();
        assert!(generator_routes.reset_from(4, 8, &plugin_routes, &[]));
        let mut generator_endpoints: [Option<GeneratorEndpointSlot>; MAX_GENERATOR_ENDPOINTS] =
            std::array::from_fn(|_| None);
        let mut insert_endpoints: [Option<InsertEndpointSlot>; TRACK_COUNT] =
            std::array::from_fn(|_| None);
        let pdc_plan = PdcPlan::build([0; TRACK_COUNT], 0, &[], 512).unwrap();
        let mut bindings = TimelinePluginAutomationBindings::new();

        assert!(bindings.reset_from(
            4,
            8,
            &bases,
            &[target],
            &plugin_routes,
            &generator_routes,
            &mut generator_endpoints,
            &mut insert_endpoints,
            &pdc_plan,
            1,
            512,
        ));
        assert_eq!(bindings.len, 0);
        assert_eq!(bindings.applied_automation_target_count(), 0);
    }

    #[test]
    fn exact_endpoint_identity_never_reuses_cached_latency_on_read_collision() {
        let chain = spawn_mock_instrument(
            0.25,
            Arc::new(AtomicU32::new(0)),
            Arc::new(AtomicU32::new(0)),
            DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES,
        );
        let PluginChain {
            audio,
            control,
            guard,
        } = chain;
        wait_until(|| control.plugin_latency_snapshot().is_some());
        let mut endpoint = fixed_adapter(audio);
        endpoint.manifest = PluginEndpointManifest::identified(&[700]).unwrap();
        let exact = endpoint.exact_endpoint_snapshot().unwrap();
        let cached = endpoint.coherent_latency_snapshot().unwrap();

        assert!(endpoint.accept_fresh_endpoint_snapshot(None).is_none());
        assert_eq!(endpoint.coherent_latency_snapshot(), Some(cached));
        let mut invalid = cached;
        invalid.revision = 0;
        assert!(
            endpoint
                .accept_fresh_endpoint_snapshot(Some(invalid))
                .is_none()
        );
        assert_eq!(endpoint.coherent_latency_snapshot(), Some(cached));
        assert_eq!(exact.revision(), cached.revision);

        drop(endpoint);
        guard.shutdown();
        drop(control);
    }

    #[test]
    fn activated_mixer_insert_automation_keeps_exact_delay_across_callback_splits() {
        let mut project = timeline_test_project(2.0);
        project.mixer_tracks[1].volume = 1.0;
        let mut first_plugin = timeline_test_plugin(101);
        first_plugin.role = PluginRole::Effect;
        let mut second_plugin = timeline_test_plugin(202);
        second_plugin.role = PluginRole::Effect;
        project
            .plugin_instances
            .extend([first_plugin, second_plugin]);
        project.mixer_insert_slots.extend([
            MixerInsertSlotRef {
                track: 1,
                slot: 0,
                plugin_instance_id: 101,
            },
            MixerInsertSlotRef {
                track: 1,
                slot: 1,
                plugin_instance_id: 202,
            },
        ]);
        let target = CompiledAutomationTarget::PluginParameter {
            instance_id: 202,
            parameter_id: 9,
        };
        push_timeline_automation(
            &mut project,
            1,
            AutomationTarget::PluginParameter {
                instance: 202,
                parameter: 9,
            },
            AutomationCurve::Linear,
            [
                AutomationPoint::new(0.0, 0.0),
                AutomationPoint::new(frame_as_beat_f64(512), 1.0),
            ],
        );
        let timeline = compile_timeline_test_project(&project);
        assert_eq!(timeline.driven_automation_targets(), &[target]);

        let first_parameter = Arc::new(AtomicU32::new(0));
        let second_parameter = Arc::new(AtomicU32::new(0));
        let chain = spawn_two_slot_latency_insert(
            65,
            Arc::clone(&first_parameter),
            Arc::clone(&second_parameter),
        );
        let PluginChain {
            audio,
            control,
            guard,
        } = chain;
        wait_until(|| {
            control.plugin_latency_snapshot().is_some_and(|snapshot| {
                snapshot.active_mask == 0b11
                    && snapshot.slot_latency_samples[0] == 65
                    && snapshot.total_plugin_latency_samples == 65
            })
        });
        let mut endpoint = fixed_adapter(audio);
        endpoint.manifest = PluginEndpointManifest::identified(&[101, 202]).unwrap();
        assert!(endpoint.refresh_latency_snapshot() || endpoint.coherent_latency.is_some());

        let (mut controller, realtime) = create_timeline_runtime();
        let loop_chase = controller
            .prepare_loop_chase(&timeline, 41, 0, TimelineChaseOptions::default())
            .unwrap();
        let chase = controller
            .prepare_chase(&timeline, 41, 2, 0, TimelineChaseOptions::default())
            .unwrap();
        install_test_timeline(&mut controller, 41, timeline);
        controller.install_loop_chase(loop_chase).unwrap();
        controller.install_chase(chase).unwrap();
        let mut dsp = timeline_test_dsp(realtime);
        dsp.insert_endpoints[1] = Some(InsertEndpointSlot {
            endpoint_id: 9001,
            endpoint,
            suppress_output_frames: 0,
        });
        let status = AudioStatus::default();
        dsp.refresh_pdc_plan(&status, 256);
        assert_ne!(dsp.pdc_plan_revision, 0);
        assert_eq!(
            dsp.pdc_plan.raw_track_delay(1).unwrap().requested_samples(),
            0
        );
        assert_eq!(dsp.apply_pending_timeline_commands(), 3);
        let mut loop_token = None;
        for _ in 0..3 {
            if let Some(TimelineRuntimeEvent::LoopChaseInstalled { token, .. }) =
                controller.poll_event()
            {
                loop_token = Some(token);
            }
        }
        controller
            .activate_transport(
                TimelineTransportActivationSpec {
                    revision: 41,
                    target_epoch: 2,
                    minimum_epoch: 2,
                    frame: 0,
                    beat_q32: 0,
                    loop_start_frame: 0,
                    loop_end_frame: 0,
                    loop_start_q32: 0,
                    loop_end_q32: 0,
                    loop_token: loop_token.unwrap(),
                    loop_enabled: false,
                    playing: true,
                    mixer_pan_release: TimelineMixerPanRelease::EMPTY,
                },
                0,
            )
            .unwrap();
        assert_eq!(dsp.apply_pending_timeline_commands(), 1);
        let mut transport = RealtimeTransport::default();
        transport.apply_pending_timeline_activation(&status, &mut dsp);
        assert_eq!(
            dsp.timeline_runtime.as_ref().unwrap().active_revision(),
            Some(41)
        );
        assert_eq!(
            dsp.timeline_runtime.as_ref().unwrap().active_epoch(),
            Some(2)
        );
        let binding = dsp
            .timeline_plugin_automation_bindings
            .iter()
            .next()
            .unwrap();
        assert_eq!(binding.parameter_slot, 1);
        assert_eq!(binding.control_delay_samples, 65);
        assert_eq!(dsp.timeline_plugin_control_histories[0].next_frame(), 0);

        // Deliberately split two physical quanta at non-Q boundaries. History,
        // endpoint phase, and parameter delivery must stay invariant.
        assert!(dsp.prepare_timeline_render(2, 0, 0, 63, true));
        assert_eq!(dsp.timeline_plugin_control_histories[0].next_frame(), 63);
        let first_events = &dsp.insert_endpoints[1]
            .as_ref()
            .unwrap()
            .endpoint
            .events
            .scratch
            .timeline
            .pending[..1];
        assert!(matches!(
            first_events[0],
            FrameEvent {
                sample_offset: 0,
                kind: crate::fixed_quantum::FrameEventKind::Parameter {
                    slot: 1,
                    id: 9,
                    normalized: 0.0,
                    ..
                },
            }
        ));
        assert!(dsp.process_insert_endpoint(1, 63));
        assert_eq!(
            dsp.insert_endpoints[1]
                .as_ref()
                .unwrap()
                .endpoint
                .input_phase_frames(),
            63
        );

        assert!(dsp.prepare_timeline_render(2, 63, 0, 65, true));
        assert_eq!(dsp.timeline_plugin_control_histories[0].next_frame(), 128);
        let source_at_63 = dsp.timeline_automation_values.value_for(target, 0).unwrap();
        assert_eq!(
            dsp.insert_endpoints[1]
                .as_ref()
                .unwrap()
                .endpoint
                .events
                .scratch
                .timeline
                .pending_len,
            0
        );
        assert!(dsp.process_insert_endpoint(1, 65));
        assert_eq!(
            dsp.insert_endpoints[1]
                .as_ref()
                .unwrap()
                .endpoint
                .input_phase_frames(),
            0
        );

        assert!(dsp.prepare_timeline_render(2, 128, 0, 17, true));
        assert_eq!(dsp.timeline_plugin_control_histories[0].next_frame(), 145);
        let delayed_event = dsp.insert_endpoints[1]
            .as_ref()
            .unwrap()
            .endpoint
            .events
            .scratch
            .timeline
            .pending[0];
        let crate::fixed_quantum::FrameEventKind::Parameter {
            slot,
            id,
            normalized,
            ..
        } = delayed_event.kind
        else {
            panic!("second Q128 event must be the delayed insert parameter");
        };
        assert_eq!((delayed_event.sample_offset, slot, id), (0, 1, 9));
        assert!((normalized - source_at_63).abs() < f32::EPSILON);
        assert!(dsp.process_insert_endpoint(1, 17));

        assert!(dsp.prepare_timeline_render(2, 145, 0, 111, true));
        assert_eq!(dsp.timeline_plugin_control_histories[0].next_frame(), 256);
        let source_at_191 = dsp
            .timeline_automation_values
            .value_for(target, 46)
            .unwrap();
        assert_eq!(
            dsp.insert_endpoints[1]
                .as_ref()
                .unwrap()
                .endpoint
                .events
                .scratch
                .timeline
                .pending_len,
            0
        );
        assert!(dsp.process_insert_endpoint(1, 111));
        assert!(dsp.prepare_timeline_render(2, 256, 0, 128, true));
        assert_eq!(dsp.timeline_plugin_control_histories[0].next_frame(), 384);
        let next_events = &dsp.insert_endpoints[1]
            .as_ref()
            .unwrap()
            .endpoint
            .events
            .scratch
            .timeline
            .pending[..1];
        let crate::fixed_quantum::FrameEventKind::Parameter {
            slot,
            id,
            normalized,
            ..
        } = next_events[0].kind
        else {
            panic!("split callback must retain one delayed insert parameter");
        };
        assert_eq!((next_events[0].sample_offset, slot, id), (0, 1, 9));
        assert!((normalized - source_at_191).abs() < f32::EPSILON);
        assert_eq!(dsp.timeline_execution_failures, 0);

        drop(dsp.insert_endpoints[1].take());
        drop(dsp);
        guard.shutdown();
        drop(control);
    }

    #[test]
    fn repeated_all_off_in_one_partial_quantum_defers_and_extends_suppression() {
        let chain = spawn_empty_chain(DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
        let PluginChain {
            audio,
            control,
            guard,
        } = chain;
        let mut endpoint = fixed_adapter(audio);
        assert_eq!(endpoint.clear_and_stage_all_notes_off(), 0);
        let mut output_left = vec![0.0; 64];
        let mut output_right = vec![0.0; 64];
        let _ = endpoint.process_generator(1, 64, &mut output_left, &mut output_right);
        assert_eq!(endpoint.input_phase_frames(), 64);
        assert_eq!(endpoint.partial_quantum_usage.system, 16);

        let deferred = endpoint.clear_and_stage_all_notes_off();
        assert_eq!(deferred, 64);
        assert_eq!(endpoint.events.scratch.system.pending_len, 16);
        assert!(
            endpoint.events.scratch.system.pending[..16]
                .iter()
                .all(|event| event.sample_offset == 64)
        );
        assert_eq!(
            fixed_quantum_fail_closed_frames(&endpoint).saturating_add(deferred),
            DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES * 2 + 64
        );
        let before = endpoint.stats().frame_event_overflows;
        let _ = endpoint.process_generator(1, 64, &mut output_left, &mut output_right);
        let _ = endpoint.process_generator(1, 128, &mut vec![0.0; 128], &mut vec![0.0; 128]);
        assert_eq!(endpoint.stats().frame_event_overflows, before);

        drop(endpoint);
        guard.shutdown();
        drop(control);
    }

    #[test]
    fn live_plugin_controls_reject_only_the_owned_parameter_target() {
        let chain = spawn_empty_chain(DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
        let PluginChain {
            audio,
            control,
            guard,
        } = chain;
        let insert_chain = spawn_empty_chain(DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
        let PluginChain {
            audio: insert_audio,
            control: insert_control,
            guard: insert_guard,
        } = insert_chain;
        let mut dsp = DspState::new(48_000.0);
        dsp.generator_endpoints[0] = Some(GeneratorEndpointSlot {
            channel_id: 7,
            endpoint_id: 70,
            plugin_instance_id: 700,
            mixer_track: 1,
            endpoint: fixed_adapter(audio),
            pdc_delay: test_pdc_delay(),
            pdc_initialized: false,
            suppress_output_frames: 0,
        });
        dsp.insert_endpoints[2] = Some(InsertEndpointSlot {
            endpoint_id: 720,
            endpoint: fixed_adapter(insert_audio),
            suppress_output_frames: 0,
        });
        dsp.timeline_plugin_automation_bindings.bindings[0] =
            Some(TimelinePluginAutomationBinding {
                matrix_slot: 0,
                target: CompiledAutomationTarget::PluginParameter {
                    instance_id: 700,
                    parameter_id: 9,
                },
                parameter_id: 9,
                parameter_slot: 0,
                endpoint: TimelinePluginEndpointIdentity::Generator {
                    channel_id: 7,
                    endpoint_id: 70,
                    plugin_instance_id: 700,
                },
                endpoint_snapshot: PluginEndpointSnapshot::try_new(
                    PluginEndpointManifest::identified(&[700]).unwrap(),
                    PluginLatencySnapshot {
                        revision: 1,
                        active_mask: 1,
                        slot_latency_samples: [0; MAX_INSERT_PLUGIN_SLOTS],
                        total_plugin_latency_samples: 0,
                        tail_samples: 0,
                    },
                )
                .unwrap(),
                pdc_plan_revision: None,
                control_delay_samples: 0,
                committed_q128: true,
            });
        dsp.timeline_plugin_automation_bindings.bindings[1] =
            Some(TimelinePluginAutomationBinding {
                matrix_slot: 1,
                target: CompiledAutomationTarget::PluginParameter {
                    instance_id: 720,
                    parameter_id: 42,
                },
                parameter_id: 42,
                parameter_slot: 0,
                endpoint: TimelinePluginEndpointIdentity::MixerInsert {
                    track: 2,
                    endpoint_id: 720,
                },
                endpoint_snapshot: PluginEndpointSnapshot::try_new(
                    PluginEndpointManifest::identified(&[720]).unwrap(),
                    PluginLatencySnapshot {
                        revision: 1,
                        active_mask: 1,
                        slot_latency_samples: [0; MAX_INSERT_PLUGIN_SLOTS],
                        total_plugin_latency_samples: 0,
                        tail_samples: 0,
                    },
                )
                .unwrap(),
                pdc_plan_revision: Some(1),
                control_delay_samples: 0,
                committed_q128: true,
            });
        dsp.timeline_plugin_automation_bindings.len = 2;
        dsp.timeline_plugin_automation_bindings.revision = Some(1);
        dsp.timeline_plugin_automation_bindings.epoch = Some(1);
        let (mut retired, _reclaimed) = test_reclaimer();

        dsp.handle(
            AudioCommand::SetGeneratorParameter {
                channel_id: 7,
                slot: 0,
                id: 9,
                normalized: 0.5,
            },
            &mut retired,
        );
        assert_eq!(dsp.fixed_quantum_endpoint_event_rejections, 1);
        assert_eq!(
            dsp.generator_endpoints[0]
                .as_ref()
                .unwrap()
                .endpoint
                .events
                .scratch
                .live
                .pending_len,
            0
        );
        dsp.handle(
            AudioCommand::SetGeneratorParameter {
                channel_id: 7,
                slot: 0,
                id: 10,
                normalized: 0.25,
            },
            &mut retired,
        );
        dsp.handle(
            AudioCommand::SendGeneratorMidi {
                channel_id: 7,
                slot: Some(0),
                data: [0x90, 60, 100],
                sample_offset: 0,
            },
            &mut retired,
        );
        assert_eq!(
            dsp.generator_endpoints[0]
                .as_ref()
                .unwrap()
                .endpoint
                .events
                .scratch
                .live
                .pending_len,
            2
        );
        dsp.handle(
            AudioCommand::SetInsertParameter {
                insert: 2,
                slot: 0,
                id: 42,
                normalized: 0.75,
            },
            &mut retired,
        );
        assert_eq!(dsp.fixed_quantum_endpoint_event_rejections, 2);
        assert_eq!(
            dsp.insert_endpoints[2]
                .as_ref()
                .unwrap()
                .endpoint
                .events
                .scratch
                .live
                .pending_len,
            0
        );
        dsp.handle(
            AudioCommand::SetInsertParameter {
                insert: 2,
                slot: 0,
                id: 43,
                normalized: 0.25,
            },
            &mut retired,
        );
        assert_eq!(
            dsp.insert_endpoints[2]
                .as_ref()
                .unwrap()
                .endpoint
                .events
                .scratch
                .live
                .pending_len,
            1
        );

        drop(dsp.generator_endpoints[0].take());
        drop(dsp.insert_endpoints[2].take());
        drop(dsp);
        guard.shutdown();
        drop(control);
        insert_guard.shutdown();
        drop(insert_control);
    }

    #[test]
    fn paused_plugin_automation_stays_pending_until_first_q128_commit() {
        let mut project = timeline_test_project(2.0);
        let mut channel = timeline_test_channel(7, 1);
        channel.instrument_plugin_instance_id = Some(700);
        project.channels.push(channel);
        project.plugin_instances.push(timeline_test_plugin(700));
        push_timeline_automation(
            &mut project,
            1,
            AutomationTarget::PluginParameter {
                instance: 700,
                parameter: 9,
            },
            AutomationCurve::Linear,
            [
                AutomationPoint {
                    position: 0.0,
                    value: 0.25,
                    tension: 0.0,
                },
                AutomationPoint {
                    position: 1.0,
                    value: 0.75,
                    tension: 0.0,
                },
            ],
        );
        let tempo_map = TempoMap::from_project(&project, 48_000).unwrap();
        let timeline = Arc::new(
            CompiledTimeline::from_project(&project, &tempo_map, TimelineCompileOptions::default())
                .unwrap(),
        );
        assert_eq!(
            timeline.driven_automation_targets(),
            &[CompiledAutomationTarget::PluginParameter {
                instance_id: 700,
                parameter_id: 9,
            }]
        );

        let observed_parameter = Arc::new(AtomicU32::new(0));
        let chain = spawn_mock_instrument(
            0.25,
            Arc::new(AtomicU32::new(0)),
            Arc::clone(&observed_parameter),
            DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES,
        );
        let PluginChain {
            audio,
            control,
            guard,
        } = chain;
        wait_until(|| control.plugin_latency_snapshot().is_some());
        let mut endpoint = fixed_adapter(audio);
        endpoint.manifest = PluginEndpointManifest::identified(&[700]).unwrap();
        endpoint.refresh_latency_snapshot();

        let (mut controller, realtime) = create_timeline_runtime();
        let loop_chase = controller
            .prepare_loop_chase(&timeline, 1, 0, TimelineChaseOptions::default())
            .unwrap();
        let chase = controller
            .prepare_chase(&timeline, 1, 1, 0, TimelineChaseOptions::default())
            .unwrap();
        install_test_timeline(&mut controller, 1, Arc::clone(&timeline));
        controller.install_loop_chase(loop_chase).unwrap();
        controller.install_chase(chase).unwrap();
        let mut dsp = timeline_test_dsp(realtime);
        dsp.generator_endpoints[0] = Some(GeneratorEndpointSlot {
            channel_id: 7,
            endpoint_id: 70,
            plugin_instance_id: 700,
            mixer_track: 1,
            endpoint,
            pdc_delay: test_pdc_delay(),
            pdc_initialized: false,
            suppress_output_frames: 0,
        });
        assert_eq!(dsp.apply_pending_timeline_commands(), 3);
        let status = AudioStatus::default();
        dsp.apply_transport_discontinuity(&status, 1, 0, 0, TransportDiscontinuity::OneShot);
        assert_eq!(dsp.timeline_automation_pending, 1);
        assert_eq!(
            dsp.timeline_plugin_automation_bindings
                .applied_automation_target_count(),
            0
        );
        assert!(!dsp.prepare_timeline_render(1, 0, 0, 64, false));
        assert_eq!(dsp.timeline_automation_pending, 1);

        assert!(dsp.prepare_timeline_render(1, 0, 0, 64, true));
        assert_eq!(dsp.timeline_automation_pending, 0);
        assert_eq!(
            dsp.timeline_plugin_automation_bindings
                .applied_automation_target_count(),
            1
        );

        dsp.render_block(&status, 64);
        dsp.apply_transport_discontinuity(&status, 2, 0, 0, TransportDiscontinuity::Loop);
        assert!(dsp.timeline_plugin_automation_bindings.is_bound_to(1, 2));
        assert_eq!(dsp.timeline_automation_pending, 1);
        assert!(dsp.prepare_timeline_render(2, 0, 0, 64, true));
        assert_eq!(dsp.timeline_automation_pending, 0);
        assert_eq!(
            dsp.generator_endpoints[0]
                .as_ref()
                .unwrap()
                .endpoint
                .events
                .scratch
                .timeline
                .pending_len,
            1
        );
        assert!(matches!(
            dsp.generator_endpoints[0]
                .as_ref()
                .unwrap()
                .endpoint
                .events
                .scratch
                .timeline
                .pending[0],
            FrameEvent {
                sample_offset: 0,
                kind: crate::fixed_quantum::FrameEventKind::Parameter { slot: 0, id: 9, .. },
            }
        ));
        let old_latency_revision = dsp.generator_endpoints[0]
            .as_ref()
            .unwrap()
            .endpoint
            .coherent_latency_snapshot()
            .unwrap()
            .revision;
        assert!(control.set_slot_config(
            0,
            SlotConfig {
                enabled: true,
                bypassed: true,
                wet: 1.0,
            },
        ));
        wait_until(|| {
            control
                .plugin_latency_snapshot()
                .is_some_and(|snapshot| snapshot.active_mask == 0)
        });
        assert!(control.set_slot_config(0, SlotConfig::default()));
        wait_until(|| {
            control.plugin_latency_snapshot().is_some_and(|snapshot| {
                snapshot.active_mask == 1 && snapshot.revision > old_latency_revision
            })
        });
        let pdc_revision_before_drift = dsp.pdc_plan_revision;
        dsp.render_block(&status, 64);
        assert!(
            dsp.timeline_runtime
                .as_ref()
                .unwrap()
                .stats()
                .ownership_needs_resync
        );
        assert!(dsp.pdc_plan_revision > pdc_revision_before_drift);
        assert_eq!(
            dsp.generator_endpoints[0]
                .as_ref()
                .unwrap()
                .endpoint
                .events
                .scratch
                .timeline
                .pending_len,
            0
        );
        assert_eq!(
            observed_parameter.load(Ordering::Acquire),
            0.0_f32.to_bits()
        );

        drop(dsp.generator_endpoints[0].take());
        drop(dsp);
        drop(controller);
        guard.shutdown();
        drop(control);
    }

    #[test]
    fn timeline_generator_routes_ignore_stale_native_endpoints_and_fail_closed_on_identity_drift() {
        let chain = spawn_empty_chain(DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
        let PluginChain {
            audio,
            control,
            guard,
        } = chain;
        let mut dsp = DspState::new(48_000.0);
        dsp.generator_endpoints[0] = Some(GeneratorEndpointSlot {
            channel_id: 7,
            endpoint_id: 70,
            plugin_instance_id: 700,
            mixer_track: 1,
            endpoint: fixed_adapter(audio),
            pdc_delay: test_pdc_delay(),
            pdc_initialized: false,
            suppress_output_frames: 0,
        });

        let mut native_routes = TimelineGeneratorRouteTable::new();
        assert!(native_routes.reset_from(2, 3, &[], &[]));
        let mut native_plan = TimelineRenderPlan::new();
        native_plan.note_on(chased_note(1, 7, 60), 0);
        assert!(DspState::stage_timeline_generator_plan(
            &mut native_plan,
            &mut dsp.generator_endpoints,
            &mut dsp.timeline_generator_notes,
            &native_routes,
            2,
            3,
            &mut dsp.fixed_quantum_event_overflows,
        ));
        assert!(matches!(
            native_plan.events[0].unwrap().kind,
            TimelinePlannedEventKind::NoteOn(_)
        ));
        assert_eq!(
            dsp.generator_endpoints[0]
                .as_ref()
                .unwrap()
                .endpoint
                .events
                .scratch
                .timeline
                .pending_len,
            0
        );

        let mismatched_routes = timeline_generator_route_table(2, 3, 7, 701);
        let mut mismatch_plan = TimelineRenderPlan::new();
        mismatch_plan.note_on(chased_note(2, 7, 61), 0);
        assert!(!DspState::stage_timeline_generator_plan(
            &mut mismatch_plan,
            &mut dsp.generator_endpoints,
            &mut dsp.timeline_generator_notes,
            &mismatched_routes,
            2,
            3,
            &mut dsp.fixed_quantum_event_overflows,
        ));

        let matching_routes = timeline_generator_route_table(2, 3, 7, 700);
        assert!(!DspState::stage_timeline_generator_plan(
            &mut mismatch_plan,
            &mut dsp.generator_endpoints,
            &mut dsp.timeline_generator_notes,
            &matching_routes,
            2,
            4,
            &mut dsp.fixed_quantum_event_overflows,
        ));
        dsp.generator_endpoints[0].as_mut().unwrap().mixer_track = 2;
        assert!(!DspState::stage_timeline_generator_plan(
            &mut mismatch_plan,
            &mut dsp.generator_endpoints,
            &mut dsp.timeline_generator_notes,
            &matching_routes,
            2,
            3,
            &mut dsp.fixed_quantum_event_overflows,
        ));
        let endpoint = dsp.generator_endpoints[0].as_mut().unwrap();
        endpoint.mixer_track = 1;
        endpoint.endpoint_id = 71;
        assert!(!DspState::stage_timeline_generator_plan(
            &mut mismatch_plan,
            &mut dsp.generator_endpoints,
            &mut dsp.timeline_generator_notes,
            &matching_routes,
            2,
            3,
            &mut dsp.fixed_quantum_event_overflows,
        ));
        dsp.generator_endpoints[0].as_mut().unwrap().endpoint_id = 70;
        assert!(DspState::stage_timeline_generator_plan(
            &mut mismatch_plan,
            &mut dsp.generator_endpoints,
            &mut dsp.timeline_generator_notes,
            &matching_routes,
            2,
            3,
            &mut dsp.fixed_quantum_event_overflows,
        ));

        let slot = dsp.generator_endpoints[0].take().unwrap();
        drop(slot);
        drop(dsp);
        guard.shutdown();
        drop(control);
    }

    #[test]
    fn replacing_generator_clears_endpoint_identity_and_emits_no_ghost_note_off() {
        let first = spawn_empty_chain(DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
        let second = spawn_empty_chain(DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
        let PluginChain {
            audio: first_audio,
            control: first_control,
            guard: first_guard,
        } = first;
        let PluginChain {
            audio: second_audio,
            control: second_control,
            guard: second_guard,
        } = second;
        let (retired_tx, mut retired_rx) = RingBuffer::new(4);
        let (insert_events, _insert_events_rx) = RingBuffer::new(1);
        let (generator_events, _generator_events_rx) = RingBuffer::new(4);
        let mut dsp =
            DspState::new_with_endpoint_io(48_000.0, retired_tx, insert_events, generator_events);
        dsp.install_generator_endpoint(7, 70, 700, 1, fixed_adapter(first_audio), test_pdc_delay());
        let mut on = TimelineRenderPlan::new();
        on.note_on(chased_note(1, 7, 64), 1);
        let routes = timeline_generator_route_table(1, 1, 7, 700);
        assert!(DspState::stage_timeline_generator_plan(
            &mut on,
            &mut dsp.generator_endpoints,
            &mut dsp.timeline_generator_notes,
            &routes,
            1,
            1,
            &mut dsp.fixed_quantum_event_overflows,
        ));
        assert!(
            dsp.timeline_generator_notes
                .iter()
                .flatten()
                .any(|binding| binding.endpoint_id == 70)
        );

        dsp.install_generator_endpoint(
            7,
            71,
            701,
            1,
            fixed_adapter(second_audio),
            test_pdc_delay(),
        );
        assert!(dsp.timeline_generator_notes.iter().all(Option::is_none));
        let mut off = TimelineRenderPlan::new();
        off.note_off(chased_note(1, 7, 64), 2);
        assert!(!DspState::stage_timeline_generator_plan(
            &mut off,
            &mut dsp.generator_endpoints,
            &mut dsp.timeline_generator_notes,
            &routes,
            1,
            1,
            &mut dsp.fixed_quantum_event_overflows,
        ));
        let replacement = dsp.generator_endpoints[0].as_ref().unwrap();
        assert_eq!(replacement.endpoint_id, 71);
        assert_eq!(replacement.endpoint.events.scratch.timeline.pending_len, 0);

        drop(retired_rx.pop().unwrap());
        dsp.remove_generator_endpoint(7);
        drop(retired_rx.pop().unwrap());
        drop(dsp);
        first_guard.shutdown();
        second_guard.shutdown();
        drop((first_control, second_control));
    }

    #[test]
    fn timeline_native_channel_bases_apply_solo_mute_volume_pan_and_route() {
        let mut dsp = DspState::new(48_000.0);
        assert!(dsp.timeline_channel_bases.reset_from(&[
            ChannelBaseDescriptor {
                channel_id: 1,
                volume: 0.5,
                pan: 1.0,
                muted: false,
                solo: true,
                mixer_track: 2,
            },
            ChannelBaseDescriptor {
                channel_id: 2,
                volume: 1.0,
                pan: 0.0,
                muted: false,
                solo: false,
                mixer_track: 3,
            },
        ]));
        dsp.start_timeline_native_note(chased_note(1, 1, 60));
        dsp.start_timeline_native_note(chased_note(2, 2, 60));
        for voice in &mut dsp.voices {
            if voice.active {
                voice.phase = 0.25;
            }
        }
        dsp.render_synth_frame(0);
        let channel_one = dsp.track_block[2 * MAX_MIXER_BLOCK_FRAMES];
        let channel_two = dsp.track_block[3 * MAX_MIXER_BLOCK_FRAMES];
        assert_eq!(channel_one[0], 0.0);
        assert!(channel_one[1] > 0.1 && channel_one[1] < 0.2);
        assert_eq!(channel_two, [0.0; 2]);

        assert!(dsp.timeline_channel_bases.reset_from(&[
            ChannelBaseDescriptor {
                channel_id: 1,
                volume: 1.0,
                pan: 0.0,
                muted: true,
                solo: true,
                mixer_track: 2,
            },
            ChannelBaseDescriptor {
                channel_id: 2,
                volume: 1.0,
                pan: 0.0,
                muted: false,
                solo: false,
                mixer_track: 3,
            },
        ]));
        dsp.track_block[..4 * MAX_MIXER_BLOCK_FRAMES].fill([0.0; 2]);
        dsp.render_synth_frame(1);
        assert_eq!(dsp.track_block[2 * MAX_MIXER_BLOCK_FRAMES + 1], [0.0; 2]);
        assert_eq!(dsp.track_block[3 * MAX_MIXER_BLOCK_FRAMES + 1], [0.0; 2]);
    }

    #[test]
    fn timeline_generator_channel_base_gates_and_scales_worker_output() {
        let last_midi = Arc::new(AtomicU32::new(0));
        let chain = spawn_mock_instrument(
            0.5,
            Arc::clone(&last_midi),
            Arc::new(AtomicU32::new(0)),
            DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES,
        );
        let PluginChain {
            audio,
            control,
            guard,
        } = chain;
        let mut dsp = DspState::new(48_000.0);
        dsp.generator_endpoints[0] = Some(GeneratorEndpointSlot {
            channel_id: 9,
            endpoint_id: 90,
            plugin_instance_id: 900,
            mixer_track: 1,
            endpoint: fixed_adapter(audio),
            pdc_delay: test_pdc_delay(),
            pdc_initialized: false,
            suppress_output_frames: 0,
        });
        assert!(dsp.timeline_channel_bases.reset_from(&[
            ChannelBaseDescriptor {
                channel_id: 9,
                volume: 0.5,
                pan: 1.0,
                muted: false,
                solo: true,
                mixer_track: 2,
            },
            ChannelBaseDescriptor {
                channel_id: 10,
                volume: 1.0,
                pan: 0.0,
                muted: false,
                solo: false,
                mixer_track: 3,
            },
        ]));
        assert!(
            dsp.generator_endpoints[0]
                .as_mut()
                .unwrap()
                .endpoint
                .stage(FrameEvent::midi(0, None, [0x90, 60, 127]))
        );

        let mut saw_output = false;
        for completed in 1..=8 {
            dsp.track_block[..3 * MAX_MIXER_BLOCK_FRAMES].fill([0.0; 2]);
            dsp.process_generator_endpoints(DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
            wait_until(|| control.stats().completed >= completed);
            let first = dsp.track_block[2 * MAX_MIXER_BLOCK_FRAMES];
            if first[1] > 0.0 {
                assert_eq!(first[0], 0.0);
                assert!((first[1] - 0.25).abs() < 1.0e-6);
                saw_output = true;
                break;
            }
        }
        assert!(saw_output);
        assert_eq!(
            last_midi.load(Ordering::Acquire),
            u32::from_le_bytes([0x90, 60, 127, 0])
        );

        assert!(dsp.timeline_channel_bases.reset_from(&[
            ChannelBaseDescriptor {
                channel_id: 9,
                volume: 1.0,
                pan: 0.0,
                muted: true,
                solo: true,
                mixer_track: 2,
            },
            ChannelBaseDescriptor {
                channel_id: 10,
                volume: 1.0,
                pan: 0.0,
                muted: false,
                solo: false,
                mixer_track: 3,
            },
        ]));
        dsp.track_block[..3 * MAX_MIXER_BLOCK_FRAMES].fill([0.0; 2]);
        dsp.process_generator_endpoints(DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
        assert_eq!(
            dsp.track_block[2 * MAX_MIXER_BLOCK_FRAMES
                ..2 * MAX_MIXER_BLOCK_FRAMES + DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES],
            vec![[0.0; 2]; DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES]
        );

        let slot = dsp.generator_endpoints[0].take().unwrap();
        drop(slot);
        drop(dsp);
        guard.shutdown();
        drop(control);
    }

    #[test]
    fn timeline_event_backpressure_stops_before_replacing_callback_ownership() {
        let (controller, realtime) = create_timeline_runtime_with_capacities(4, 1, 2).unwrap();
        let mut engine = timeline_test_engine(controller);
        let mut dsp = timeline_test_dsp(realtime);

        engine
            .install_compiled_timeline(1, compiled_test_timeline())
            .unwrap();
        assert_eq!(dsp.apply_pending_timeline_commands(), 1);
        engine
            .install_compiled_timeline(2, compiled_test_timeline())
            .unwrap();
        assert_eq!(dsp.apply_pending_timeline_commands(), 0);
        assert_eq!(
            dsp.timeline_runtime.as_ref().unwrap().active_revision(),
            Some(1)
        );
        assert_eq!(engine.timeline_runtime_stats().event_backpressure, 1);

        assert!(matches!(
            engine.next_timeline_runtime_event(),
            Some(TimelineRuntimeEvent::Installed { revision: 1, .. })
        ));
        assert_eq!(dsp.apply_pending_timeline_commands(), 1);
        assert_eq!(
            dsp.timeline_runtime.as_ref().unwrap().active_revision(),
            Some(2)
        );
        assert!(matches!(
            engine.next_timeline_runtime_event(),
            Some(TimelineRuntimeEvent::Installed { revision: 2, .. })
        ));
        assert!(matches!(
            engine.next_retired_timeline_resource(),
            Some(RetiredTimelineResource::Bundle { revision: 1, .. })
        ));

        let callback = run_timeline_callback_until_shutdown(dsp);
        assert!(engine.shutdown_realtime_resources(Duration::from_secs(2)));
        let (shutdown, stats) = callback.join().unwrap();
        assert!(shutdown);
        assert_eq!(stats.unexpected_realtime_drops, 0);
    }

    #[test]
    fn timeline_callback_applies_at_most_its_fixed_per_block_budget() {
        let (controller, realtime) = create_timeline_runtime();
        let mut engine = timeline_test_engine(controller);
        let mut dsp = timeline_test_dsp(realtime);
        for revision in 1..=10 {
            engine
                .install_compiled_timeline(revision, compiled_test_timeline())
                .unwrap();
        }

        assert_eq!(dsp.apply_pending_timeline_commands(), 8);
        assert_eq!(
            dsp.timeline_runtime.as_ref().unwrap().active_revision(),
            Some(8)
        );
        for revision in 1..=8 {
            assert!(matches!(
                engine.next_timeline_runtime_event(),
                Some(TimelineRuntimeEvent::Installed {
                    revision: confirmed,
                    ..
                }) if confirmed == revision
            ));
        }
        assert_eq!(engine.reclaim_retired_timeline_resources(), 7);

        assert_eq!(dsp.apply_pending_timeline_commands(), 2);
        assert_eq!(
            dsp.timeline_runtime.as_ref().unwrap().active_revision(),
            Some(10)
        );
        assert!(engine.next_timeline_runtime_event().is_some());
        assert!(engine.next_timeline_runtime_event().is_some());
        assert_eq!(engine.reclaim_retired_timeline_resources(), 2);

        let callback = run_timeline_callback_until_shutdown(dsp);
        assert!(engine.shutdown_realtime_resources(Duration::from_secs(2)));
        assert!(callback.join().unwrap().0);
    }

    #[test]
    fn audio_engine_drop_handshakes_timeline_retirement_without_hardware() {
        let (controller, realtime) = create_timeline_runtime();
        let mut engine = timeline_test_engine(controller);
        engine
            .install_compiled_timeline(9, compiled_test_timeline())
            .unwrap();
        let callback = run_timeline_callback_until_shutdown(timeline_test_dsp(realtime));
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if matches!(
                engine.next_timeline_runtime_event(),
                Some(TimelineRuntimeEvent::Installed { revision: 9, .. })
            ) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "timeline install was not confirmed"
            );
            thread::yield_now();
        }

        drop(engine);
        let (shutdown, stats) = callback.join().unwrap();
        assert!(shutdown);
        assert_eq!(stats.unexpected_realtime_drops, 0);
        assert!(!stats.ownership_needs_resync);
    }

    fn transport_status(sample_rate: u32, tempo: f32) -> AudioStatus {
        let status = AudioStatus::default();
        status.sample_rate.store(sample_rate, Ordering::Relaxed);
        status
            .tempo_milli
            .store((tempo * 1_000.0).round() as u32, Ordering::Relaxed);
        status
    }

    fn render_test_transport(
        dsp: &mut DspState,
        status: &AudioStatus,
        mailbox: &TransportMailbox,
        transport: &mut RealtimeTransport,
        chunks: &[usize],
    ) {
        for &frames in chunks {
            assert!(frames <= MAX_MIXER_BLOCK_FRAMES);
            render_transport_chunk(dsp, status, mailbox, transport, frames, |_, _| {});
        }
    }

    #[test]
    fn transport_stop_and_seek_bypass_a_full_audio_command_queue() {
        let status = transport_status(48_000, 120.0);
        let mailbox = TransportMailbox::default();
        let mut transport = RealtimeTransport::default();
        let mut dsp = DspState::new(48_000.0);
        let (mut command_tx, _command_rx) = RingBuffer::new(1);
        command_tx.push(AudioCommand::SetMaster(0.5)).unwrap();
        assert_eq!(command_tx.slots(), 0);

        dsp.voices[0].active = true;
        dsp.audio_voices[0].active = true;
        let seek_request = mailbox.publish(TransportMutation::Discontinuity {
            target_beat_q32: beat_to_q32(4.0),
            target_timeline_frame: 96_000,
            playing: Some(true),
        });
        transport.apply_latest_request(&mailbox, &status, &mut dsp);
        assert_eq!(transport.beat_q32, beat_to_q32(4.0));
        assert_eq!(transport.timeline_frame, 96_000);
        assert_eq!(transport.epoch, 2);
        assert_eq!(
            status.applied_transport_request.load(Ordering::Relaxed),
            seek_request
        );
        assert!(!dsp.voices[0].active);
        assert!(!dsp.audio_voices[0].active);

        dsp.voices[0].active = true;
        dsp.audio_voices[0].active = true;
        let stop_request = mailbox.publish(TransportMutation::Discontinuity {
            target_beat_q32: 0,
            target_timeline_frame: 0,
            playing: Some(false),
        });
        transport.apply_latest_request(&mailbox, &status, &mut dsp);
        assert_eq!(transport.beat_q32, 0);
        assert_eq!(transport.timeline_frame, 0);
        assert_eq!(transport.epoch, 3);
        assert!(!status.playing.load(Ordering::Relaxed));
        assert_eq!(
            status.applied_transport_request.load(Ordering::Relaxed),
            stop_request
        );
        assert!(!dsp.voices[0].active);
        assert!(!dsp.audio_voices[0].active);
        assert_eq!(command_tx.slots(), 0);
    }

    #[test]
    fn paused_transport_freezes_timeline_while_device_clock_advances() {
        let status = transport_status(48_000, 120.0);
        let mailbox = TransportMailbox::default();
        let mut transport = RealtimeTransport::default();
        let mut dsp = DspState::new(48_000.0);
        mailbox.publish(TransportMutation::SetPlaying(true));
        render_test_transport(&mut dsp, &status, &mailbox, &mut transport, &[128]);
        let running_beat = transport.beat_q32;
        assert_eq!(transport.device_frame, 128);
        assert_eq!(transport.timeline_frame, 128);

        mailbox.publish(TransportMutation::SetPlaying(false));
        render_test_transport(&mut dsp, &status, &mailbox, &mut transport, &[256]);
        assert_eq!(transport.device_frame, 384);
        assert_eq!(transport.timeline_frame, 128);
        assert_eq!(transport.beat_q32, running_beat);
    }

    #[test]
    fn explicit_pause_overrides_callback_state_even_when_the_mailbox_already_cached_false() {
        let status = transport_status(48_000, 120.0);
        let mailbox = TransportMailbox::default();
        let mut transport = RealtimeTransport::default();
        let mut dsp = DspState::new(48_000.0);

        // Atomic Timeline activation changes the callback-owned request but
        // intentionally leaves the legacy mailbox untouched. This reproduces
        // the critical UI-false/mailbox-false/callback-true state.
        transport.request.playing = true;
        transport.request.request_id = mailbox.control_thread_request_id();
        transport.publish(&status);
        assert!(!mailbox.try_load().unwrap().playing);
        assert!(status.playing.load(Ordering::Relaxed));

        let pause_request = mailbox.publish(TransportMutation::SetPlaying(false));
        assert_ne!(pause_request, 0);
        transport.apply_latest_request(&mailbox, &status, &mut dsp);
        assert!(!transport.request.playing);
        assert!(!status.playing.load(Ordering::Relaxed));
        assert_eq!(transport.request.request_id, pause_request);

        let repeated_pause = mailbox.publish(TransportMutation::SetPlaying(false));
        assert_ne!(repeated_pause, pause_request);
    }

    #[test]
    fn explicit_loop_disable_advances_even_when_the_mailbox_already_cached_disabled() {
        let mailbox = TransportMailbox::default();
        let first = mailbox.publish(TransportMutation::SetLoop {
            enabled: false,
            start_q32: 0,
            end_q32: 0,
            start_frame: 0,
            end_frame: 0,
        });
        let second = mailbox.publish(TransportMutation::SetLoop {
            enabled: false,
            start_q32: 0,
            end_q32: 0,
            start_frame: 0,
            end_frame: 0,
        });
        assert_ne!(first, 0);
        assert_ne!(second, first);
        assert!(!mailbox.try_load().unwrap().loop_enabled);
    }

    #[test]
    fn transport_status_snapshot_never_observes_torn_fields() {
        const PUBLISHES: u64 = 20_000;
        let status = Arc::new(AudioStatus::default());
        let writer_status = Arc::clone(&status);
        let writer = thread::spawn(move || {
            for value in 1..=PUBLISHES {
                RealtimeTransport {
                    request: TransportRequest {
                        request_id: value,
                        playing: value % 2 != 0,
                        ..TransportRequest::default()
                    },
                    device_frame: value,
                    timeline_frame: value * 2,
                    beat_q32: value * 3,
                    epoch: value,
                    loop_count: value * 4,
                    ..RealtimeTransport::default()
                }
                .publish(&writer_status);
            }
        });

        loop {
            let snapshot = status.transport_fields();
            let value = snapshot.applied_request;
            if value != 0 {
                assert_eq!(snapshot.device_frame, value);
                assert_eq!(snapshot.timeline_frame, value * 2);
                assert_eq!(snapshot.beat_q32, value * 3);
                assert_eq!(snapshot.epoch, value);
                assert_eq!(snapshot.loop_count, value * 4);
                assert_eq!(snapshot.playing, value % 2 != 0);
            }
            if value == PUBLISHES {
                break;
            }
        }
        writer.join().unwrap();
    }

    #[test]
    fn seek_increments_epoch_and_reports_the_applied_request() {
        let status = transport_status(48_000, 120.0);
        let mailbox = TransportMailbox::default();
        let mut transport = RealtimeTransport::default();
        let mut dsp = DspState::new(48_000.0);

        let first = mailbox.publish(TransportMutation::Discontinuity {
            target_beat_q32: beat_to_q32(2.5),
            target_timeline_frame: 60_000,
            playing: None,
        });
        transport.apply_latest_request(&mailbox, &status, &mut dsp);
        assert_eq!(transport.epoch, 2);
        assert_eq!(transport.timeline_frame, 60_000);
        assert_eq!(
            status.applied_transport_request.load(Ordering::Relaxed),
            first
        );

        let second = mailbox.publish(TransportMutation::Discontinuity {
            target_beat_q32: beat_to_q32(7.0),
            target_timeline_frame: 168_000,
            playing: None,
        });
        transport.apply_latest_request(&mailbox, &status, &mut dsp);
        assert_eq!(transport.epoch, 3);
        assert_eq!(transport.timeline_frame, 168_000);
        assert_eq!(
            status.applied_transport_request.load(Ordering::Relaxed),
            second
        );
    }

    #[test]
    fn exact_seek_frame_is_authoritative_over_the_scalar_callback_tempo() {
        let status = transport_status(48_000, 120.0);
        let mailbox = TransportMailbox::default();
        let mut transport = RealtimeTransport::default();
        let mut dsp = DspState::new(48_000.0);

        mailbox.publish(TransportMutation::Discontinuity {
            target_beat_q32: beat_to_q32(8.0),
            // A tempo-map precomputation can deliberately differ from the
            // constant-tempo fallback (which would be frame 192_000 here).
            target_timeline_frame: 321_987,
            playing: Some(true),
        });
        render_test_transport(&mut dsp, &status, &mailbox, &mut transport, &[13]);

        assert_eq!(transport.timeline_frame, 322_000);
        assert_eq!(transport.device_frame, 13);
        assert_eq!(transport.epoch, 2);
    }

    #[test]
    fn exact_and_legacy_engine_transport_apis_publish_complete_frame_requests() {
        let (controller, _realtime) = create_timeline_runtime();
        let mut engine = timeline_test_engine(controller);
        engine.status.sample_rate.store(48_000, Ordering::Relaxed);
        engine.status.tempo_milli.store(120_000, Ordering::Relaxed);

        engine.seek_transport_to_frame(8.0, 321_987);
        let exact_seek = engine.transport_mailbox.try_load().unwrap();
        assert_eq!(exact_seek.target_beat_q32, beat_to_q32(8.0));
        assert_eq!(exact_seek.target_timeline_frame, 321_987);

        engine.set_transport_loop_frames(3.0, 70_123, 7.0, 333_777, true);
        let exact_loop = engine.transport_mailbox.try_load().unwrap();
        assert!(exact_loop.loop_enabled);
        assert_eq!(exact_loop.loop_start_q32, beat_to_q32(3.0));
        assert_eq!(exact_loop.loop_end_q32, beat_to_q32(7.0));
        assert_eq!(exact_loop.loop_start_frame, 70_123);
        assert_eq!(exact_loop.loop_end_frame, 333_777);

        engine.seek_transport(4.0);
        let legacy_seek = engine.transport_mailbox.try_load().unwrap();
        assert_eq!(legacy_seek.target_timeline_frame, 96_000);

        engine.set_transport_loop(2.0, 3.0, true);
        let legacy_loop = engine.transport_mailbox.try_load().unwrap();
        assert_eq!(legacy_loop.loop_start_frame, 48_000);
        assert_eq!(legacy_loop.loop_end_frame, 72_000);

        // This test has no callback thread to service a lifecycle shutdown.
        engine.timeline_runtime_shutdown_confirmed = true;
    }

    #[test]
    fn transport_clock_is_independent_of_callback_block_partitioning() {
        fn simulate(chunks: &[usize]) -> RealtimeTransport {
            let status = transport_status(48_000, 123.0);
            let mailbox = TransportMailbox::default();
            let mut transport = RealtimeTransport::default();
            let mut dsp = DspState::new(48_000.0);
            mailbox.publish(TransportMutation::SetPlaying(true));
            render_test_transport(&mut dsp, &status, &mailbox, &mut transport, chunks);
            transport
        }

        let uniform = simulate(&[2_000, 2_000, 2_000, 2_000, 2_000]);
        let irregular = simulate(&[64, 257, 2_048, 2_048, 2_048, 2_048, 1_487]);
        assert_eq!(uniform.device_frame, 10_000);
        assert_eq!(uniform, irregular);
    }

    #[test]
    fn loop_boundary_is_exact_across_different_block_sizes() {
        fn simulate(chunks: &[usize]) -> RealtimeTransport {
            let status = transport_status(100, 60.0);
            let mailbox = TransportMailbox::default();
            let mut transport = RealtimeTransport::default();
            let mut dsp = DspState::new(100.0);
            mailbox.publish(TransportMutation::SetLoop {
                enabled: true,
                start_q32: 0,
                end_q32: beat_to_q32(1.0),
                start_frame: 0,
                end_frame: 100,
            });
            mailbox.publish(TransportMutation::SetPlaying(true));
            render_test_transport(&mut dsp, &status, &mailbox, &mut transport, chunks);
            transport
        }

        let one_block = simulate(&[150]);
        let split = simulate(&[64, 86]);
        assert_eq!(one_block, split);
        assert_eq!(one_block.device_frame, 150);
        assert_eq!(one_block.timeline_frame, 50);
        assert_eq!(one_block.loop_count, 1);
        assert_eq!(one_block.epoch, 2);
        assert!((q32_to_beat(one_block.beat_q32) - 0.5).abs() < 1.0e-6);
    }

    #[test]
    fn precomputed_nonlinear_loop_frames_are_exact_for_one_frame_and_tail_partitions() {
        fn simulate(chunks: &[usize]) -> RealtimeTransport {
            let status = transport_status(100, 60.0);
            let mailbox = TransportMailbox::default();
            let mut transport = RealtimeTransport::default();
            let mut dsp = DspState::new(100.0);
            mailbox.publish(TransportMutation::Discontinuity {
                target_beat_q32: beat_to_q32(4.0),
                target_timeline_frame: 1_000,
                playing: Some(true),
            });
            mailbox.publish(TransportMutation::SetLoop {
                enabled: true,
                start_q32: beat_to_q32(4.0),
                end_q32: beat_to_q32(10.0),
                // The six-beat range is intentionally only 123 frames. These
                // values stand in for conversion by a nonlinear TempoMap.
                start_frame: 1_000,
                end_frame: 1_123,
            });
            render_test_transport(&mut dsp, &status, &mailbox, &mut transport, chunks);
            transport
        }

        let one_block = simulate(&[521]);
        let one_frame_and_tails = simulate(&[1, 2, 127, 1, 256, 134]);
        assert_eq!(one_block, one_frame_and_tails);
        assert_eq!(one_block.device_frame, 521);
        assert_eq!(one_block.timeline_frame, 1_029);
        assert_eq!(one_block.loop_count, 4);
        assert_eq!(one_block.epoch, 6);
    }

    #[test]
    fn loop_validity_and_boundaries_depend_only_on_exact_frames() {
        let mailbox = TransportMailbox::default();

        mailbox.publish(TransportMutation::SetLoop {
            enabled: true,
            start_q32: beat_to_q32(10.0),
            end_q32: beat_to_q32(2.0),
            start_frame: 100,
            end_frame: 200,
        });
        assert!(mailbox.try_load().unwrap().loop_enabled);

        mailbox.publish(TransportMutation::SetLoop {
            enabled: true,
            start_q32: beat_to_q32(2.0),
            end_q32: beat_to_q32(10.0),
            start_frame: 200,
            end_frame: 200,
        });
        assert!(!mailbox.try_load().unwrap().loop_enabled);

        mailbox.publish(TransportMutation::SetLoop {
            enabled: true,
            start_q32: beat_to_q32(2.0),
            end_q32: beat_to_q32(10.0),
            start_frame: 201,
            end_frame: 200,
        });
        assert!(!mailbox.try_load().unwrap().loop_enabled);
    }

    #[test]
    fn transport_mailbox_never_returns_a_torn_exact_loop_snapshot() {
        fn assert_coherent(request: TransportRequest) {
            match request.loop_start_frame {
                0 => assert_eq!(request, TransportRequest::default()),
                11 => {
                    assert!(request.loop_enabled);
                    assert_eq!(request.loop_start_q32, beat_to_q32(1.0));
                    assert_eq!(request.loop_end_q32, beat_to_q32(2.0));
                    assert_eq!(request.loop_end_frame, 29);
                }
                1_011 => {
                    assert!(request.loop_enabled);
                    assert_eq!(request.loop_start_q32, beat_to_q32(7.0));
                    assert_eq!(request.loop_end_q32, beat_to_q32(11.0));
                    assert_eq!(request.loop_end_frame, 9_999);
                }
                frame => panic!("torn loop start frame: {frame}"),
            }
        }

        const WRITES_PER_WRITER: usize = 5_000;
        let mailbox = Arc::new(TransportMailbox::default());
        let first_mailbox = Arc::clone(&mailbox);
        let first_writer = thread::spawn(move || {
            for _ in 0..WRITES_PER_WRITER {
                first_mailbox.publish(TransportMutation::SetLoop {
                    enabled: true,
                    start_q32: beat_to_q32(1.0),
                    end_q32: beat_to_q32(2.0),
                    start_frame: 11,
                    end_frame: 29,
                });
            }
        });
        let second_mailbox = Arc::clone(&mailbox);
        let second_writer = thread::spawn(move || {
            for _ in 0..WRITES_PER_WRITER {
                second_mailbox.publish(TransportMutation::SetLoop {
                    enabled: true,
                    start_q32: beat_to_q32(7.0),
                    end_q32: beat_to_q32(11.0),
                    start_frame: 1_011,
                    end_frame: 9_999,
                });
            }
        });

        while !first_writer.is_finished() || !second_writer.is_finished() {
            if let Some(request) = mailbox.try_load() {
                assert_coherent(request);
            }
        }
        first_writer.join().unwrap();
        second_writer.join().unwrap();
        assert_coherent(mailbox.try_load().unwrap());
    }

    #[test]
    fn callback_command_work_is_bounded() {
        let mut dsp = DspState::new(48_000.0);
        let command_count = MAX_COMMANDS_PER_CALLBACK + 17;
        let (mut command_tx, mut command_rx) = RingBuffer::new(command_count + 1);
        let (mut retired_tx, _retired_rx) = RingBuffer::new(1);
        let (mut event_tx, _event_rx) = RingBuffer::new(1);
        for index in 0..command_count {
            command_tx
                .push(AudioCommand::SetMaster(index as f32 / command_count as f32))
                .unwrap();
        }

        process_commands(&mut dsp, &mut command_rx, &mut retired_tx, &mut event_tx);

        let mut remaining = 0;
        while command_rx.pop().is_ok() {
            remaining += 1;
        }
        assert_eq!(remaining, 17);
    }

    #[test]
    fn insert_endpoint_replace_remove_and_confirmations_are_realtime_safe() {
        let chain_one = spawn_mock_insert(
            0.0,
            Arc::new(AtomicU32::new(0)),
            Arc::new(AtomicU32::new(0)),
            8,
        );
        let chain_two = spawn_mock_insert(
            0.0,
            Arc::new(AtomicU32::new(0)),
            Arc::new(AtomicU32::new(0)),
            8,
        );
        let PluginChain {
            audio: audio_one,
            control: control_one,
            guard: guard_one,
        } = chain_one;
        let PluginChain {
            audio: audio_two,
            control: control_two,
            guard: guard_two,
        } = chain_two;
        let (retired_endpoint_tx, mut retired_endpoint_rx) = RingBuffer::new(4);
        let (endpoint_event_tx, mut endpoint_event_rx) = RingBuffer::new(8);
        let mut dsp =
            DspState::new_with_insert_io(48_000.0, retired_endpoint_tx, endpoint_event_tx);
        let (mut command_tx, mut command_rx) = RingBuffer::new(8);
        let (mut retired_asset_tx, _retired_asset_rx) = RingBuffer::new(1);
        let (mut asset_event_tx, _asset_event_rx) = RingBuffer::new(1);

        command_tx
            .push(AudioCommand::InstallInsertEndpoint {
                insert: 3,
                endpoint_id: 11,
                endpoint: fixed_adapter(audio_one),
            })
            .unwrap();
        process_commands(
            &mut dsp,
            &mut command_rx,
            &mut retired_asset_tx,
            &mut asset_event_tx,
        );
        assert_eq!(
            endpoint_event_rx.pop().unwrap(),
            InsertEndpointEvent::Installed {
                insert: 3,
                endpoint_id: 11,
                replaced_endpoint_id: None,
                success: true,
            }
        );

        command_tx
            .push(AudioCommand::InstallInsertEndpoint {
                insert: 3,
                endpoint_id: 12,
                endpoint: fixed_adapter(audio_two),
            })
            .unwrap();
        process_commands(
            &mut dsp,
            &mut command_rx,
            &mut retired_asset_tx,
            &mut asset_event_tx,
        );
        assert_eq!(
            endpoint_event_rx.pop().unwrap(),
            InsertEndpointEvent::Installed {
                insert: 3,
                endpoint_id: 12,
                replaced_endpoint_id: Some(11),
                success: true,
            }
        );
        let retired = retired_endpoint_rx.pop().unwrap();
        assert_eq!(
            retired._endpoint.adapter.quantum_frames(),
            DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES
        );
        assert!(retired._pdc_delay.is_none());
        drop(retired);

        command_tx
            .push(AudioCommand::RemoveInsertEndpoint { insert: 3 })
            .unwrap();
        process_commands(
            &mut dsp,
            &mut command_rx,
            &mut retired_asset_tx,
            &mut asset_event_tx,
        );
        assert_eq!(
            endpoint_event_rx.pop().unwrap(),
            InsertEndpointEvent::Removed {
                insert: 3,
                endpoint_id: Some(12),
            }
        );
        let retired = retired_endpoint_rx.pop().unwrap();
        assert_eq!(
            retired._endpoint.adapter.quantum_frames(),
            DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES
        );
        assert!(retired._pdc_delay.is_none());
        drop(retired);
        drop(dsp);
        guard_one.shutdown();
        guard_two.shutdown();
        drop((control_one, control_two));
    }

    #[test]
    fn mock_insert_processes_before_fader_and_receives_realtime_controls() {
        let last_midi = Arc::new(AtomicU32::new(0));
        let parameter = Arc::new(AtomicU32::new(0));
        let chain = spawn_mock_insert(0.25, Arc::clone(&last_midi), Arc::clone(&parameter), 4);
        let PluginChain {
            audio,
            control,
            guard,
        } = chain;
        let (retired_endpoint_tx, mut retired_endpoint_rx) = RingBuffer::new(4);
        let (endpoint_event_tx, mut endpoint_event_rx) = RingBuffer::new(8);
        let mut dsp =
            DspState::new_with_insert_io(48_000.0, retired_endpoint_tx, endpoint_event_tx);
        let (mut command_tx, mut command_rx) = RingBuffer::new(16);
        let (mut retired_asset_tx, _retired_asset_rx) = RingBuffer::new(4);
        let (mut asset_event_tx, _asset_event_rx) = RingBuffer::new(4);

        command_tx
            .push(AudioCommand::InstallInsertEndpoint {
                insert: 1,
                endpoint_id: 21,
                endpoint: fixed_adapter(audio),
            })
            .unwrap();
        process_commands(
            &mut dsp,
            &mut command_rx,
            &mut retired_asset_tx,
            &mut asset_event_tx,
        );
        assert!(matches!(
            endpoint_event_rx.pop().unwrap(),
            InsertEndpointEvent::Installed {
                endpoint_id: 21,
                success: true,
                ..
            }
        ));

        register_test_asset(
            &mut dsp,
            &mut retired_asset_tx,
            201,
            &[0.25; 512],
            48_000,
            1,
        );
        dsp.handle(
            AudioCommand::SetTrackGain {
                track: 1,
                gain: 0.5,
            },
            &mut retired_asset_tx,
        );
        let status = AudioStatus::default();
        status.playing.store(true, Ordering::Release);
        dsp.handle(
            AudioCommand::PlayClip {
                clip_id: 201,
                asset_id: 201,
                source_frame: 0.0,
                gain: 1.0,
                mixer_track: 1,
            },
            &mut retired_asset_tx,
        );
        dsp.render_block(&status, DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
        wait_until(|| control.stats().completed >= 1);

        dsp.handle(
            AudioCommand::PlayClip {
                clip_id: 201,
                asset_id: 201,
                source_frame: 0.0,
                gain: 1.0,
                mixer_track: 1,
            },
            &mut retired_asset_tx,
        );
        dsp.render_block(&status, DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
        wait_until(|| control.stats().completed >= 2);
        dsp.render_block(&status, DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
        wait_until(|| control.stats().completed >= 3);
        let expected = (0.5_f32 * 0.5 * 0.72).tanh();
        for frame in &dsp.master_block[..DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES] {
            assert!((frame[0] - expected).abs() < 1.0e-6);
            assert!((frame[1] - expected).abs() < 1.0e-6);
        }

        command_tx
            .push(AudioCommand::SendInsertMidi {
                insert: 1,
                slot: Some(0),
                data: [0x90, 64, 100],
                sample_offset: 100,
            })
            .unwrap();
        command_tx
            .push(AudioCommand::SetInsertParameter {
                insert: 1,
                slot: 0,
                id: 7,
                normalized: 0.75,
            })
            .unwrap();
        process_commands(
            &mut dsp,
            &mut command_rx,
            &mut retired_asset_tx,
            &mut asset_event_tx,
        );
        dsp.render_block(&status, 64);
        dsp.render_block(&status, 64);
        wait_until(|| {
            last_midi.load(Ordering::Acquire) == u32::from_le_bytes([0x90, 64, 100, 100])
                && f32::from_bits(parameter.load(Ordering::Acquire)) == 0.75
        });

        dsp.handle(AudioCommand::StopAll, &mut retired_asset_tx);
        dsp.render_block(&status, DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
        wait_until(|| last_midi.load(Ordering::Acquire) == u32::from_le_bytes([0xBF, 123, 0, 0]));

        command_tx
            .push(AudioCommand::RemoveInsertEndpoint { insert: 1 })
            .unwrap();
        process_commands(
            &mut dsp,
            &mut command_rx,
            &mut retired_asset_tx,
            &mut asset_event_tx,
        );
        assert!(matches!(
            endpoint_event_rx.pop().unwrap(),
            InsertEndpointEvent::Removed {
                endpoint_id: Some(21),
                ..
            }
        ));
        drop(retired_endpoint_rx.pop().unwrap());
        drop(dsp);
        guard.shutdown();
        drop(control);
    }

    #[test]
    fn mock_instrument_routes_through_insert_and_fader_then_stops_on_midi() {
        let instrument_midi = Arc::new(AtomicU32::new(0));
        let instrument_parameter = Arc::new(AtomicU32::new(0));
        let instrument = spawn_mock_instrument(
            0.25,
            Arc::clone(&instrument_midi),
            Arc::clone(&instrument_parameter),
            4,
        );
        let effect = spawn_mock_transform(
            2.0,
            0.0,
            Arc::new(AtomicU32::new(0)),
            Arc::new(AtomicU32::new(0)),
            4,
        );
        let PluginChain {
            audio: instrument_audio,
            control: instrument_control,
            guard: instrument_guard,
        } = instrument;
        let PluginChain {
            audio: effect_audio,
            control: effect_control,
            guard: effect_guard,
        } = effect;

        let (retired_endpoint_tx, mut retired_endpoint_rx) = RingBuffer::new(8);
        let (insert_event_tx, mut insert_event_rx) = RingBuffer::new(8);
        let (generator_event_tx, mut generator_event_rx) = RingBuffer::new(8);
        let mut dsp = DspState::new_with_endpoint_io(
            48_000.0,
            retired_endpoint_tx,
            insert_event_tx,
            generator_event_tx,
        );
        let (mut command_tx, mut command_rx) = RingBuffer::new(16);
        let (mut retired_asset_tx, _retired_asset_rx) = RingBuffer::new(1);
        let (mut asset_event_tx, _asset_event_rx) = RingBuffer::new(1);

        command_tx
            .push(AudioCommand::InstallGeneratorEndpoint {
                channel_id: 42,
                endpoint_id: 420,
                plugin_instance_id: 4_200,
                mixer_track: 2,
                endpoint: fixed_adapter(instrument_audio),
                pdc_delay: test_pdc_delay(),
            })
            .unwrap();
        command_tx
            .push(AudioCommand::InstallInsertEndpoint {
                insert: 2,
                endpoint_id: 220,
                endpoint: fixed_adapter(effect_audio),
            })
            .unwrap();
        process_commands(
            &mut dsp,
            &mut command_rx,
            &mut retired_asset_tx,
            &mut asset_event_tx,
        );
        assert!(matches!(
            generator_event_rx.pop().unwrap(),
            GeneratorEndpointEvent::Installed {
                channel_id: 42,
                endpoint_id: 420,
                mixer_track: 2,
                success: true,
                ..
            }
        ));
        assert!(matches!(
            insert_event_rx.pop().unwrap(),
            InsertEndpointEvent::Installed {
                insert: 2,
                endpoint_id: 220,
                success: true,
                ..
            }
        ));
        dsp.handle(
            AudioCommand::SetTrackGain {
                track: 2,
                gain: 0.5,
            },
            &mut retired_asset_tx,
        );

        command_tx
            .push(AudioCommand::SetGeneratorParameter {
                channel_id: 42,
                slot: 0,
                id: 9,
                normalized: 0.4,
            })
            .unwrap();
        command_tx
            .push(AudioCommand::SendGeneratorMidi {
                channel_id: 42,
                slot: Some(0),
                data: [0x90, 60, 100],
                sample_offset: 0,
            })
            .unwrap();
        process_commands(
            &mut dsp,
            &mut command_rx,
            &mut retired_asset_tx,
            &mut asset_event_tx,
        );
        let status = AudioStatus::default();
        dsp.render_block(&status, DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
        assert_eq!(
            &dsp.master_block[..DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES],
            &[[0.0; 2]; DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES]
        );
        wait_until(|| {
            f32::from_bits(instrument_parameter.load(Ordering::Acquire)) == 0.4
                && instrument_midi.load(Ordering::Acquire) == u32::from_le_bytes([0x90, 60, 100, 0])
                && instrument_control.stats().completed >= 1
                && effect_control.stats().completed >= 1
        });

        for completed in 2..=5 {
            dsp.render_block(&status, DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
            wait_until(|| {
                instrument_control.stats().completed >= completed
                    && effect_control.stats().completed >= completed
            });
        }
        let expected = (0.4_f32 * 2.0 * 0.5 * 0.72).tanh();
        for frame in &dsp.master_block[..DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES] {
            assert!((frame[0] - expected).abs() < 1.0e-6);
            assert!((frame[1] - expected).abs() < 1.0e-6);
        }

        command_tx
            .push(AudioCommand::SendGeneratorMidi {
                channel_id: 42,
                slot: Some(0),
                data: [0x80, 60, 0],
                sample_offset: 0,
            })
            .unwrap();
        process_commands(
            &mut dsp,
            &mut command_rx,
            &mut retired_asset_tx,
            &mut asset_event_tx,
        );
        dsp.render_block(&status, DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
        wait_until(|| {
            instrument_midi.load(Ordering::Acquire) == u32::from_le_bytes([0x80, 60, 0, 0])
                && instrument_control.stats().completed >= 6
                && effect_control.stats().completed >= 6
        });
        for completed in 7..=10 {
            dsp.render_block(&status, DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
            wait_until(|| {
                instrument_control.stats().completed >= completed
                    && effect_control.stats().completed >= completed
            });
        }
        assert_eq!(
            &dsp.master_block[..DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES],
            &[[0.0; 2]; DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES]
        );

        command_tx
            .push(AudioCommand::SendGeneratorMidi {
                channel_id: 42,
                slot: Some(0),
                data: [0x90, 64, 127],
                sample_offset: 0,
            })
            .unwrap();
        process_commands(
            &mut dsp,
            &mut command_rx,
            &mut retired_asset_tx,
            &mut asset_event_tx,
        );
        dsp.render_block(&status, DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
        wait_until(|| {
            instrument_midi.load(Ordering::Acquire) == u32::from_le_bytes([0x90, 64, 127, 0])
        });
        dsp.handle(AudioCommand::StopAll, &mut retired_asset_tx);
        dsp.render_block(&status, DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
        wait_until(|| {
            instrument_midi.load(Ordering::Acquire) == u32::from_le_bytes([0xBF, 123, 0, 0])
        });

        command_tx
            .push(AudioCommand::RemoveGeneratorEndpoint { channel_id: 42 })
            .unwrap();
        command_tx
            .push(AudioCommand::RemoveInsertEndpoint { insert: 2 })
            .unwrap();
        process_commands(
            &mut dsp,
            &mut command_rx,
            &mut retired_asset_tx,
            &mut asset_event_tx,
        );
        assert!(matches!(
            generator_event_rx.pop().unwrap(),
            GeneratorEndpointEvent::Removed {
                channel_id: 42,
                endpoint_id: Some(420),
                plugin_instance_id: Some(4_200),
            }
        ));
        assert!(matches!(
            insert_event_rx.pop().unwrap(),
            InsertEndpointEvent::Removed {
                insert: 2,
                endpoint_id: Some(220),
            }
        ));
        let retired_generator = retired_endpoint_rx.pop().unwrap();
        assert_eq!(
            retired_generator._endpoint.adapter.quantum_frames(),
            DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES
        );
        assert!(retired_generator._pdc_delay.is_some());
        drop(retired_generator);
        let retired_insert = retired_endpoint_rx.pop().unwrap();
        assert_eq!(
            retired_insert._endpoint.adapter.quantum_frames(),
            DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES
        );
        assert!(retired_insert._pdc_delay.is_none());
        drop(retired_insert);
        drop(dsp);
        instrument_guard.shutdown();
        effect_guard.shutdown();
        drop((instrument_control, effect_control));
    }

    #[test]
    fn generator_replace_bad_route_and_remove_are_confirmed_and_retired() {
        let first = spawn_empty_chain(4);
        let replacement = spawn_empty_chain(4);
        let bad_route = spawn_empty_chain(4);
        let PluginChain {
            audio: first_audio,
            control: first_control,
            guard: first_guard,
        } = first;
        let PluginChain {
            audio: replacement_audio,
            control: replacement_control,
            guard: replacement_guard,
        } = replacement;
        let PluginChain {
            audio: bad_audio,
            control: bad_control,
            guard: bad_guard,
        } = bad_route;
        let (retired_endpoint_tx, mut retired_endpoint_rx) = RingBuffer::new(8);
        let (insert_event_tx, _insert_event_rx) = RingBuffer::new(1);
        let (generator_event_tx, mut generator_event_rx) = RingBuffer::new(8);
        let mut dsp = DspState::new_with_endpoint_io(
            48_000.0,
            retired_endpoint_tx,
            insert_event_tx,
            generator_event_tx,
        );
        let (mut command_tx, mut command_rx) = RingBuffer::new(8);
        let (mut retired_asset_tx, _retired_asset_rx) = RingBuffer::new(1);
        let (mut asset_event_tx, _asset_event_rx) = RingBuffer::new(1);

        for command in [
            AudioCommand::InstallGeneratorEndpoint {
                channel_id: 9,
                endpoint_id: 91,
                plugin_instance_id: 901,
                mixer_track: 4,
                endpoint: fixed_adapter(first_audio),
                pdc_delay: test_pdc_delay(),
            },
            AudioCommand::InstallGeneratorEndpoint {
                channel_id: 9,
                endpoint_id: 92,
                plugin_instance_id: 902,
                mixer_track: 4,
                endpoint: fixed_adapter(replacement_audio),
                pdc_delay: test_pdc_delay(),
            },
            AudioCommand::SetGeneratorRoute {
                channel_id: 9,
                mixer_track: TRACK_COUNT,
            },
            AudioCommand::InstallGeneratorEndpoint {
                channel_id: 99,
                endpoint_id: 99,
                plugin_instance_id: 999,
                mixer_track: TRACK_COUNT,
                endpoint: fixed_adapter(bad_audio),
                pdc_delay: test_pdc_delay(),
            },
        ] {
            command_tx.push(command).unwrap();
        }
        process_commands(
            &mut dsp,
            &mut command_rx,
            &mut retired_asset_tx,
            &mut asset_event_tx,
        );
        assert!(matches!(
            generator_event_rx.pop().unwrap(),
            GeneratorEndpointEvent::Installed {
                endpoint_id: 91,
                replaced_endpoint_id: None,
                success: true,
                ..
            }
        ));
        assert!(matches!(
            generator_event_rx.pop().unwrap(),
            GeneratorEndpointEvent::Installed {
                endpoint_id: 92,
                replaced_endpoint_id: Some(91),
                success: true,
                ..
            }
        ));
        assert_eq!(
            generator_event_rx.pop().unwrap(),
            GeneratorEndpointEvent::RouteSet {
                channel_id: 9,
                endpoint_id: Some(92),
                plugin_instance_id: Some(902),
                mixer_track: TRACK_COUNT,
                success: false,
            }
        );
        assert!(matches!(
            generator_event_rx.pop().unwrap(),
            GeneratorEndpointEvent::Installed {
                channel_id: 99,
                success: false,
                ..
            }
        ));
        let installed = dsp.find_generator_slot(9).unwrap();
        assert_eq!(
            dsp.generator_endpoints[installed]
                .as_ref()
                .unwrap()
                .mixer_track,
            4
        );
        for _ in 0..2 {
            let retired = retired_endpoint_rx.pop().unwrap();
            assert_eq!(
                retired._endpoint.adapter.quantum_frames(),
                DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES
            );
            assert!(retired._pdc_delay.is_some());
            drop(retired);
        }

        command_tx
            .push(AudioCommand::SetGeneratorRoute {
                channel_id: 9,
                mixer_track: 5,
            })
            .unwrap();
        process_commands(
            &mut dsp,
            &mut command_rx,
            &mut retired_asset_tx,
            &mut asset_event_tx,
        );
        assert_eq!(
            generator_event_rx.pop().unwrap(),
            GeneratorEndpointEvent::RouteSet {
                channel_id: 9,
                endpoint_id: Some(92),
                plugin_instance_id: Some(902),
                mixer_track: 5,
                success: true,
            }
        );
        assert_eq!(
            dsp.generator_endpoints[installed]
                .as_ref()
                .unwrap()
                .mixer_track,
            5
        );

        command_tx
            .push(AudioCommand::RemoveGeneratorEndpoint { channel_id: 9 })
            .unwrap();
        process_commands(
            &mut dsp,
            &mut command_rx,
            &mut retired_asset_tx,
            &mut asset_event_tx,
        );
        assert_eq!(
            generator_event_rx.pop().unwrap(),
            GeneratorEndpointEvent::Removed {
                channel_id: 9,
                endpoint_id: Some(92),
                plugin_instance_id: Some(902),
            }
        );
        let retired = retired_endpoint_rx.pop().unwrap();
        assert_eq!(
            retired._endpoint.adapter.quantum_frames(),
            DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES
        );
        assert!(retired._pdc_delay.is_some());
        drop(retired);
        drop(dsp);
        first_guard.shutdown();
        replacement_guard.shutdown();
        bad_guard.shutdown();
        drop((first_control, replacement_control, bad_control));
    }

    #[test]
    fn generator_fixed_quantum_accepts_split_callbacks_and_one_frame_tail() {
        let last_midi = Arc::new(AtomicU32::new(0));
        let chain =
            spawn_mock_instrument(0.5, Arc::clone(&last_midi), Arc::new(AtomicU32::new(0)), 4);
        let PluginChain {
            audio,
            control,
            guard,
        } = chain;
        let (retired_endpoint_tx, mut retired_endpoint_rx) = RingBuffer::new(2);
        let (insert_event_tx, _insert_event_rx) = RingBuffer::new(1);
        let (generator_event_tx, mut generator_event_rx) = RingBuffer::new(4);
        let mut dsp = DspState::new_with_endpoint_io(
            48_000.0,
            retired_endpoint_tx,
            insert_event_tx,
            generator_event_tx,
        );
        let (mut command_tx, mut command_rx) = RingBuffer::new(4);
        let (mut retired_asset_tx, _retired_asset_rx) = RingBuffer::new(1);
        let (mut asset_event_tx, _asset_event_rx) = RingBuffer::new(1);
        command_tx
            .push(AudioCommand::InstallGeneratorEndpoint {
                channel_id: 0,
                endpoint_id: 1,
                plugin_instance_id: 10,
                mixer_track: 1,
                endpoint: fixed_adapter(audio),
                pdc_delay: test_pdc_delay(),
            })
            .unwrap();
        command_tx
            .push(AudioCommand::SendGeneratorMidi {
                channel_id: 0,
                slot: Some(0),
                data: [0x90, 60, 127],
                sample_offset: 0,
            })
            .unwrap();
        process_commands(
            &mut dsp,
            &mut command_rx,
            &mut retired_asset_tx,
            &mut asset_event_tx,
        );
        assert!(matches!(
            generator_event_rx.pop().unwrap(),
            GeneratorEndpointEvent::Installed { success: true, .. }
        ));
        for frames in [63, 64, 1] {
            dsp.process_generator_endpoints(frames);
        }
        wait_until(|| {
            last_midi.load(Ordering::Acquire) == u32::from_le_bytes([0x90, 60, 127, 0])
                && control.stats().completed >= 1
        });
        let slot = dsp.generator_endpoints[0].as_ref().unwrap();
        assert_eq!(slot.endpoint.stats().callbacks, 3);
        assert_eq!(slot.endpoint.stats().callback_frames, 128);
        assert_eq!(slot.endpoint.stats().completed_quanta, 1);
        assert_eq!(control.stats().frame_mismatches, 0);

        command_tx
            .push(AudioCommand::RemoveGeneratorEndpoint { channel_id: 0 })
            .unwrap();
        process_commands(
            &mut dsp,
            &mut command_rx,
            &mut retired_asset_tx,
            &mut asset_event_tx,
        );
        assert!(matches!(
            generator_event_rx.pop().unwrap(),
            GeneratorEndpointEvent::Removed {
                endpoint_id: Some(1),
                ..
            }
        ));
        drop(retired_endpoint_rx.pop().unwrap());
        drop(dsp);
        guard.shutdown();
        drop(control);
    }

    #[test]
    fn fixed_quantum_event_buffer_overflow_fails_closed_and_is_published() {
        let last_midi = Arc::new(AtomicU32::new(0));
        let chain = spawn_mock_insert(
            0.0,
            Arc::clone(&last_midi),
            Arc::new(AtomicU32::new(0)),
            DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES,
        );
        let PluginChain {
            audio,
            control,
            guard,
        } = chain;
        let mut dsp = DspState::new(48_000.0);
        dsp.insert_endpoints[1] = Some(InsertEndpointSlot {
            endpoint_id: 1,
            endpoint: fixed_adapter(audio),
            suppress_output_frames: 0,
        });
        let (mut retired, _reclaimed) = test_reclaimer();

        for note in 0..=MAX_FRAME_EVENTS_PER_CALLBACK {
            dsp.handle(
                AudioCommand::SendInsertMidi {
                    insert: 1,
                    slot: Some(0),
                    data: [0x90, note as u8, 100],
                    sample_offset: 0,
                },
                &mut retired,
            );
        }
        let initial_fail_closed =
            fixed_quantum_fail_closed_frames(&dsp.insert_endpoints[1].as_ref().unwrap().endpoint);
        assert_eq!(
            dsp.fixed_quantum_event_overflows,
            (MAX_FRAME_EVENTS_PER_CALLBACK + 1 - TIMELINE_ENDPOINT_LIVE_MAX_EVENTS_PER_CALLBACK)
                as u64
        );
        assert_eq!(
            dsp.insert_endpoints[1]
                .as_ref()
                .unwrap()
                .suppress_output_frames,
            initial_fail_closed
        );

        let start = MAX_MIXER_BLOCK_FRAMES;
        dsp.track_block[start..start + DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES].fill([1.0; 2]);
        dsp.process_insert_endpoint(1, DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
        assert_eq!(
            &dsp.track_block[start..start + DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES],
            &[[0.0; 2]; DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES]
        );
        wait_until(|| {
            control.stats().completed >= 1
                && last_midi.load(Ordering::Acquire) == u32::from_le_bytes([0xBF, 123, 0, 0])
        });

        let status = AudioStatus::default();
        dsp.publish_plugin_epoch_status(&status);
        assert_eq!(
            status
                .plugin_fixed_quantum_event_overflows
                .load(Ordering::Acquire),
            dsp.fixed_quantum_event_overflows
        );
        assert_eq!(
            status.plugin_fixed_quantum_frames.load(Ordering::Acquire),
            DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES as u32
        );

        let slot = dsp.insert_endpoints[1].take().unwrap();
        drop(slot.endpoint);
        guard.shutdown();
        drop(control);
    }

    #[test]
    fn fixed_quantum_endpoint_event_rejection_fails_closed_and_sends_all_off() {
        let last_midi = Arc::new(AtomicU32::new(0));
        let chain = spawn_mock_insert(
            0.0,
            Arc::clone(&last_midi),
            Arc::new(AtomicU32::new(0)),
            DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES,
        );
        let PluginChain {
            audio,
            control,
            guard,
        } = chain;
        let mut dsp = DspState::new(48_000.0);
        dsp.insert_endpoints[1] = Some(InsertEndpointSlot {
            endpoint_id: 1,
            endpoint: fixed_adapter(audio),
            suppress_output_frames: 0,
        });
        let (mut retired, _reclaimed) = test_reclaimer();
        dsp.handle(
            AudioCommand::SendInsertMidi {
                insert: 1,
                // This survives command validation but is outside the bridge's
                // fixed plug-in-slot range, exercising endpoint rejection.
                slot: Some(usize::from(u8::MAX)),
                data: [0x90, 60, 100],
                sample_offset: 0,
            },
            &mut retired,
        );

        let start = MAX_MIXER_BLOCK_FRAMES;
        dsp.track_block[start..start + DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES].fill([1.0; 2]);
        dsp.process_insert_endpoint(1, DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
        let slot = dsp.insert_endpoints[1].as_ref().unwrap();
        assert_eq!(slot.endpoint.stats().endpoint_event_rejections, 1);
        assert_eq!(dsp.fixed_quantum_endpoint_event_rejections, 1);
        assert_eq!(
            slot.suppress_output_frames,
            DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES
        );
        assert_eq!(
            &dsp.track_block[start..start + DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES],
            &[[0.0; 2]; DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES]
        );
        wait_until(|| control.stats().completed >= 1);

        dsp.track_block[start..start + DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES].fill([1.0; 2]);
        dsp.process_insert_endpoint(1, DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
        wait_until(|| {
            control.stats().completed >= 2
                && last_midi.load(Ordering::Acquire) == u32::from_le_bytes([0xBF, 123, 0, 0])
        });
        assert_eq!(
            dsp.insert_endpoints[1]
                .as_ref()
                .unwrap()
                .suppress_output_frames,
            0
        );

        let status = AudioStatus::default();
        dsp.publish_plugin_epoch_status(&status);
        assert_eq!(
            status
                .plugin_fixed_quantum_event_rejections
                .load(Ordering::Acquire),
            1
        );

        let slot = dsp.insert_endpoints[1].take().unwrap();
        drop(slot.endpoint);
        guard.shutdown();
        drop(control);
    }

    #[test]
    fn fixed_quantum_epoch_reset_discards_partial_midi_and_delivers_new_epoch_all_off() {
        let last_midi = Arc::new(AtomicU32::new(0));
        let chain = spawn_mock_insert(
            0.0,
            Arc::clone(&last_midi),
            Arc::new(AtomicU32::new(0)),
            DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES,
        );
        let PluginChain {
            audio,
            control,
            guard,
        } = chain;
        let mut dsp = DspState::new(48_000.0);
        dsp.insert_endpoints[1] = Some(InsertEndpointSlot {
            endpoint_id: 1,
            endpoint: fixed_adapter(audio),
            suppress_output_frames: 0,
        });
        let (mut retired, _reclaimed) = test_reclaimer();
        dsp.handle(
            AudioCommand::SendInsertMidi {
                insert: 1,
                slot: Some(0),
                data: [0x90, 60, 100],
                sample_offset: 100,
            },
            &mut retired,
        );

        let start = MAX_MIXER_BLOCK_FRAMES;
        dsp.track_block[start..start + 64].fill([1.0; 2]);
        dsp.process_insert_endpoint(1, 64);
        assert_eq!(last_midi.load(Ordering::Acquire), 0);
        assert_eq!(
            dsp.insert_endpoints[1]
                .as_ref()
                .unwrap()
                .endpoint
                .stats()
                .completed_quanta,
            0
        );

        let status = AudioStatus::default();
        dsp.apply_transport_epoch(&status, 2, 0);
        let slot = dsp.insert_endpoints[1].as_ref().unwrap();
        assert_eq!(slot.endpoint.epoch(), 2);
        assert_eq!(slot.endpoint.stats().epoch_resets, 1);
        assert_eq!(slot.endpoint.stats().partial_input_frames_discarded, 64);
        assert_eq!(slot.endpoint.stats().output_frames_discarded_on_reset, 64);

        dsp.track_block[start..start + DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES].fill([0.0; 2]);
        dsp.process_insert_endpoint(1, DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
        wait_until(|| {
            control.stats().completed >= 1
                && control.stats().current_epoch == 2
                && last_midi.load(Ordering::Acquire) == u32::from_le_bytes([0xBF, 123, 0, 0])
        });
        assert_eq!(status.plugin_epoch_resets.load(Ordering::Acquire), 1);
        assert_eq!(status.last_plugin_endpoint_epoch.load(Ordering::Acquire), 2);

        let slot = dsp.insert_endpoints[1].take().unwrap();
        drop(slot.endpoint);
        guard.shutdown();
        drop(control);
    }

    #[test]
    fn generator_input_queue_full_is_silent_and_observable() {
        let last_midi = Arc::new(AtomicU32::new(0));
        let chain = spawn_mock_instrument_with_delay(
            0.5,
            Arc::clone(&last_midi),
            Arc::new(AtomicU32::new(0)),
            1,
            Duration::from_millis(500),
        );
        let PluginChain {
            audio,
            control,
            guard,
        } = chain;
        let (retired_endpoint_tx, mut retired_endpoint_rx) = RingBuffer::new(2);
        let (insert_event_tx, _insert_event_rx) = RingBuffer::new(1);
        let (generator_event_tx, mut generator_event_rx) = RingBuffer::new(4);
        let mut dsp = DspState::new_with_endpoint_io(
            48_000.0,
            retired_endpoint_tx,
            insert_event_tx,
            generator_event_tx,
        );
        let (mut command_tx, mut command_rx) = RingBuffer::new(4);
        let (mut retired_asset_tx, _retired_asset_rx) = RingBuffer::new(1);
        let (mut asset_event_tx, _asset_event_rx) = RingBuffer::new(1);
        command_tx
            .push(AudioCommand::InstallGeneratorEndpoint {
                channel_id: 5,
                endpoint_id: 5,
                plugin_instance_id: 50,
                mixer_track: 1,
                endpoint: fixed_adapter(audio),
                pdc_delay: test_pdc_delay(),
            })
            .unwrap();
        command_tx
            .push(AudioCommand::SendGeneratorMidi {
                channel_id: 5,
                slot: Some(0),
                data: [0x90, 60, 127],
                sample_offset: 0,
            })
            .unwrap();
        process_commands(
            &mut dsp,
            &mut command_rx,
            &mut retired_asset_tx,
            &mut asset_event_tx,
        );
        let _ = generator_event_rx.pop().unwrap();
        dsp.process_generator_endpoints(DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
        wait_until(|| last_midi.load(Ordering::Acquire) == u32::from_le_bytes([0x90, 60, 127, 0]));
        for _ in 0..10 {
            dsp.process_generator_endpoints(DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
        }
        assert!(control.stats().input_overflows > 0);

        command_tx
            .push(AudioCommand::RemoveGeneratorEndpoint { channel_id: 5 })
            .unwrap();
        process_commands(
            &mut dsp,
            &mut command_rx,
            &mut retired_asset_tx,
            &mut asset_event_tx,
        );
        let _ = generator_event_rx.pop().unwrap();
        drop(retired_endpoint_rx.pop().unwrap());
        drop(dsp);
        guard.shutdown();
        drop(control);
    }

    #[test]
    fn full_generator_table_rejects_extra_and_clear_marker_retires_every_endpoint() {
        let (retired_endpoint_tx, mut retired_endpoint_rx) =
            RingBuffer::new(MAX_GENERATOR_ENDPOINTS + 1);
        let (insert_event_tx, _insert_event_rx) = RingBuffer::new(1);
        let (generator_event_tx, mut generator_event_rx) =
            RingBuffer::new(MAX_GENERATOR_ENDPOINTS + 2);
        let mut dsp = DspState::new_with_endpoint_io(
            48_000.0,
            retired_endpoint_tx,
            insert_event_tx,
            generator_event_tx,
        );
        let (mut command_tx, mut command_rx) = RingBuffer::new(MAX_GENERATOR_ENDPOINTS + 2);
        let (mut retired_asset_tx, _retired_asset_rx) = RingBuffer::new(1);
        let (mut asset_event_tx, _asset_event_rx) = RingBuffer::new(1);
        let mut controls = Vec::with_capacity(MAX_GENERATOR_ENDPOINTS + 1);
        let mut guards = Vec::with_capacity(MAX_GENERATOR_ENDPOINTS + 1);

        for index in 0..=MAX_GENERATOR_ENDPOINTS {
            let chain = spawn_empty_chain(1);
            controls.push(chain.control);
            guards.push(chain.guard);
            command_tx
                .push(AudioCommand::InstallGeneratorEndpoint {
                    channel_id: index as u32,
                    endpoint_id: (index + 1) as u64,
                    plugin_instance_id: (index + 101) as u64,
                    mixer_track: index % TRACK_COUNT,
                    endpoint: fixed_adapter(chain.audio),
                    pdc_delay: test_pdc_delay(),
                })
                .unwrap();
        }
        process_commands(
            &mut dsp,
            &mut command_rx,
            &mut retired_asset_tx,
            &mut asset_event_tx,
        );
        for _ in 0..MAX_GENERATOR_ENDPOINTS {
            assert!(matches!(
                generator_event_rx.pop().unwrap(),
                GeneratorEndpointEvent::Installed { success: true, .. }
            ));
        }
        assert!(matches!(
            generator_event_rx.pop().unwrap(),
            GeneratorEndpointEvent::Installed {
                channel_id,
                success: false,
                ..
            } if channel_id == MAX_GENERATOR_ENDPOINTS as u32
        ));
        assert_eq!(
            dsp.generator_endpoints
                .iter()
                .filter(|slot| slot.is_some())
                .count(),
            MAX_GENERATOR_ENDPOINTS
        );
        drop(retired_endpoint_rx.pop().unwrap());

        command_tx
            .push(AudioCommand::ClearGeneratorEndpoints { request_id: 77 })
            .unwrap();
        process_commands(
            &mut dsp,
            &mut command_rx,
            &mut retired_asset_tx,
            &mut asset_event_tx,
        );
        assert_eq!(
            generator_event_rx.pop().unwrap(),
            GeneratorEndpointEvent::Cleared {
                request_id: 77,
                removed: MAX_GENERATOR_ENDPOINTS,
            }
        );
        for _ in 0..MAX_GENERATOR_ENDPOINTS {
            drop(retired_endpoint_rx.pop().unwrap());
        }
        assert!(dsp.generator_endpoints.iter().all(Option::is_none));
        drop(dsp);
        for guard in guards {
            guard.shutdown();
        }
        drop(controls);
    }

    #[test]
    fn pdc_aligns_raw_insert_and_generator_insert_impulses() {
        let insert_latency = Arc::new(AtomicU32::new(3));
        let insert = spawn_mock_latency_transform(Arc::clone(&insert_latency), Duration::ZERO, 1);
        let generator = spawn_mock_impulse_generator(2, 1);
        let PluginChain {
            audio: insert_audio,
            control: insert_control,
            guard: insert_guard,
        } = insert;
        let PluginChain {
            audio: generator_audio,
            control: generator_control,
            guard: generator_guard,
        } = generator;
        wait_until(|| insert_control.stats().latency_samples == 3);
        wait_until(|| generator_control.stats().latency_samples == 2);

        let status = AudioStatus::default();
        status.playing.store(true, Ordering::Release);
        let mut dsp = DspState::new(48_000.0);
        dsp.insert_endpoints[2] = Some(InsertEndpointSlot {
            endpoint_id: 20,
            endpoint: fixed_adapter(insert_audio),
            suppress_output_frames: 0,
        });
        dsp.generator_endpoints[0] = Some(GeneratorEndpointSlot {
            channel_id: 30,
            endpoint_id: 30,
            plugin_instance_id: 300,
            mixer_track: 2,
            endpoint: fixed_adapter(generator_audio),
            pdc_delay: test_pdc_delay(),
            pdc_initialized: false,
            suppress_output_frames: 0,
        });
        dsp.refresh_pdc_plan(&status, DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
        assert_eq!(
            dsp.pdc_plan.reference_latency_samples(),
            (DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES * 4 + 5) as u64
        );
        let generator_delay = dsp.pdc_plan.generator_delays().next().unwrap();
        assert_eq!(generator_delay.delay.applied_samples(), 0);
        let raw_direct_delay = dsp.pdc_plan.raw_track_delay(1).unwrap();
        assert_eq!(
            raw_direct_delay.requested_samples(),
            (DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES * 4 + 5) as u64
        );
        assert_eq!(raw_direct_delay.applied_samples(), 512);
        assert!(raw_direct_delay.is_clamped());
        assert_eq!(dsp.pdc_raw_track_delays[1].target_delay_samples(), 512);
        assert_eq!(
            dsp.pdc_raw_track_delays[2].target_delay_samples(),
            (DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES * 2 + 2) as u32
        );

        dsp.render_block(&status, DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
        wait_until(|| {
            insert_control.stats().completed >= 1 && generator_control.stats().completed >= 1
        });
        dsp.refresh_pdc_plan(&status, DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
        assert_eq!(
            dsp.pdc_plan.reference_latency_samples(),
            (DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES * 4 + 5) as u64
        );

        let insert = dsp.insert_endpoints[2].take().unwrap();
        let generator = dsp.generator_endpoints[0].take().unwrap();
        drop((insert.endpoint, generator.endpoint, generator.pdc_delay));
        insert_guard.shutdown();
        generator_guard.shutdown();
        drop((insert_control, generator_control));
    }

    #[test]
    fn pdc_dynamic_latency_master_bypass_clamp_and_mixer_controls_are_diagnostic() {
        let track_latency = Arc::new(AtomicU32::new(20));
        let master_latency = Arc::new(AtomicU32::new(7));
        let track = spawn_mock_latency_transform(Arc::clone(&track_latency), Duration::ZERO, 4);
        let master = spawn_mock_latency_transform(Arc::clone(&master_latency), Duration::ZERO, 4);
        let PluginChain {
            audio: track_audio,
            control: track_control,
            guard: track_guard,
        } = track;
        let PluginChain {
            audio: master_audio,
            control: master_control,
            guard: master_guard,
        } = master;
        wait_until(|| track_control.stats().latency_samples == 20);
        wait_until(|| master_control.stats().latency_samples == 7);

        let status = AudioStatus::default();
        let mut dsp = DspState::new(48_000.0);
        dsp.insert_endpoints[3] = Some(InsertEndpointSlot {
            endpoint_id: 3,
            endpoint: fixed_adapter(track_audio),
            suppress_output_frames: 0,
        });
        dsp.insert_endpoints[0] = Some(InsertEndpointSlot {
            endpoint_id: 1,
            endpoint: fixed_adapter(master_audio),
            suppress_output_frames: 0,
        });

        dsp.render_block(&status, DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
        wait_until(|| {
            track_control.stats().completed >= 1 && master_control.stats().completed >= 1
        });
        let first = status.pdc_fields();
        assert_eq!(
            first.reference_latency_samples,
            (DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES * 2 + 20) as u64
        );
        assert_eq!(
            first.master_latency_samples,
            (DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES * 2 + 7) as u32
        );
        assert_eq!(
            first.output_latency_samples,
            (DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES * 4 + 27) as u64
        );
        assert_eq!(dsp.pdc_raw_track_delays[3].target_delay_samples(), 0);
        assert_eq!(
            dsp.pdc_raw_track_delays[0].target_delay_samples(),
            (DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES * 2 + 20) as u32
        );

        dsp.refresh_pdc_plan(&status, DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
        let stable_revision = status.pdc_fields().plan_revision;
        let (mut retired, _reclaimed) = test_reclaimer();
        dsp.handle(
            AudioCommand::SetTrackMuted {
                track: 3,
                muted: true,
            },
            &mut retired,
        );
        dsp.handle(
            AudioCommand::SetTrackSolo {
                track: 3,
                solo: true,
            },
            &mut retired,
        );
        dsp.refresh_pdc_plan(&status, DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
        assert_eq!(status.pdc_fields().plan_revision, stable_revision);

        track_latency.store(40, Ordering::Release);
        assert!(track_control.set_slot_config(0, SlotConfig::default()));
        wait_until(|| {
            track_control.stats().latency_samples
                == (DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES + 40) as u32
        });
        dsp.refresh_pdc_plan(&status, DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
        assert_eq!(
            status.pdc_fields().reference_latency_samples,
            (DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES * 2 + 40) as u64
        );
        assert!(status.pdc_fields().plan_revision > stable_revision);

        assert!(track_control.set_slot_config(
            0,
            SlotConfig {
                bypassed: true,
                ..SlotConfig::default()
            },
        ));
        wait_until(|| {
            track_control.stats().latency_samples == DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES as u32
        });
        dsp.refresh_pdc_plan(&status, DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
        assert_eq!(
            status.pdc_fields().reference_latency_samples,
            (DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES * 2) as u64
        );
        assert_eq!(
            status.pdc_fields().output_latency_samples,
            (DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES * 4 + 7) as u64
        );

        track_latency.store(1_000, Ordering::Release);
        assert!(track_control.set_slot_config(0, SlotConfig::default()));
        wait_until(|| {
            track_control.stats().latency_samples
                == (DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES + 1_000) as u32
        });
        dsp.refresh_pdc_plan(&status, DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
        let clamped = status.pdc_fields();
        assert_eq!(
            clamped.reference_latency_samples,
            (DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES * 2 + 1_000) as u64
        );
        assert_eq!(clamped.maximum_delay_samples, 512);
        assert!(clamped.clamped_path_count > 0);
        assert_eq!(dsp.pdc_raw_track_delays[0].target_delay_samples(), 512);

        let master = dsp.insert_endpoints[0].take().unwrap();
        let track = dsp.insert_endpoints[3].take().unwrap();
        drop((master.endpoint, track.endpoint));
        master_guard.shutdown();
        track_guard.shutdown();
        drop((master_control, track_control));
    }

    #[test]
    fn generator_pdc_advances_on_invalid_blocks_and_route_reset_clears_history() {
        let chain = spawn_empty_chain(1);
        let PluginChain {
            audio,
            control,
            guard,
        } = chain;
        let mut delay = test_pdc_delay();
        delay.request_delay(2, 0).unwrap();
        let _ = delay.process_sample([1.0; 2]);
        let mut dsp = DspState::new(48_000.0);
        dsp.generator_endpoints[0] = Some(GeneratorEndpointSlot {
            channel_id: 9,
            endpoint_id: 9,
            plugin_instance_id: 90,
            mixer_track: 1,
            endpoint: fixed_adapter(audio),
            pdc_delay: delay,
            pdc_initialized: true,
            suppress_output_frames: 0,
        });

        dsp.process_generator_endpoints(2);
        let track_one = MAX_MIXER_BLOCK_FRAMES;
        assert_eq!(dsp.track_block[track_one], [0.0; 2]);
        assert_eq!(dsp.track_block[track_one + 1], [1.0; 2]);

        dsp.track_block[..TRACK_COUNT * MAX_MIXER_BLOCK_FRAMES].fill([0.0; 2]);
        let slot = dsp.generator_endpoints[0].as_mut().unwrap();
        let _ = slot.pdc_delay.process_sample([2.0; 2]);
        dsp.set_generator_route(9, 2);
        dsp.process_generator_endpoints(2);
        let track_two = 2 * MAX_MIXER_BLOCK_FRAMES;
        assert_eq!(&dsp.track_block[track_two..track_two + 2], &[[0.0; 2]; 2]);
        assert_eq!(
            dsp.generator_endpoints[0]
                .as_ref()
                .unwrap()
                .suppress_output_frames,
            DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES * 2 - 2
        );

        dsp.process_generator_endpoints(1);
        assert_eq!(
            dsp.generator_endpoints[0]
                .as_ref()
                .unwrap()
                .suppress_output_frames,
            DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES * 2 - 3
        );
        let generator = dsp.generator_endpoints[0].take().unwrap();
        drop((generator.endpoint, generator.pdc_delay));
        guard.shutdown();
        drop(control);
    }

    #[test]
    fn pdc_counts_input_accumulation_plus_fixed_bridge_and_plugin_once() {
        let plugin_latency = Arc::new(AtomicU32::new(20));
        let chain = spawn_mock_latency_transform(
            Arc::clone(&plugin_latency),
            Duration::ZERO,
            DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES,
        );
        let PluginChain {
            audio,
            control,
            guard,
        } = chain;
        wait_until(|| control.stats().latency_samples == 20);
        let mut adapter = fixed_adapter(audio);
        assert_eq!(
            fixed_quantum_endpoint_latency(&adapter),
            (DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES * 2) as u32 + 20
        );
        let input = [0.0; DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES];
        let mut left = [0.0; DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES];
        let mut right = [0.0; DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES];
        let _ = adapter.process(1, &input, &input, &mut left, &mut right);
        assert_eq!(
            fixed_quantum_endpoint_latency(&adapter),
            (DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES * 2) as u32 + 20
        );
        drop(adapter);
        guard.shutdown();
        drop(control);

        let mut dsp = DspState::new(48_000.0);
        dsp.pdc_raw_track_delays[1].request_delay(2, 0).unwrap();
        let start = MAX_MIXER_BLOCK_FRAMES;
        dsp.track_block[start] = [1.0; 2];
        dsp.process_raw_source_pdc(1);
        dsp.reset_voice_and_pdc_state(0);
        dsp.track_block[start..start + 3].fill([0.0; 2]);
        dsp.process_raw_source_pdc(3);
        assert_eq!(&dsp.track_block[start..start + 3], &[[0.0; 2]; 3]);
        assert_eq!(dsp.pdc_raw_track_delays[1].target_delay_samples(), 2);
    }

    #[test]
    fn pdc_insert_fail_closed_for_invalid_or_incomplete_delayed_dry() {
        let latency = Arc::new(AtomicU32::new(2));
        let delayed = spawn_mock_latency_transform(
            latency,
            Duration::ZERO,
            DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES,
        );
        let invalid = spawn_empty_chain(1);
        let PluginChain {
            audio: delayed_audio,
            control: delayed_control,
            guard: delayed_guard,
        } = delayed;
        let PluginChain {
            audio: invalid_audio,
            control: invalid_control,
            guard: invalid_guard,
        } = invalid;
        wait_until(|| delayed_control.stats().latency_samples == 2);

        let mut dsp = DspState::new(48_000.0);
        dsp.insert_endpoints[1] = Some(InsertEndpointSlot {
            endpoint_id: 1,
            endpoint: fixed_adapter(delayed_audio),
            suppress_output_frames: 0,
        });
        let start = MAX_MIXER_BLOCK_FRAMES;
        dsp.track_block[start..start + DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES].fill([1.0; 2]);
        dsp.process_insert_endpoint(1, DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
        assert_eq!(
            &dsp.track_block[start..start + DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES],
            &[[0.0; 2]; DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES]
        );
        assert!(
            dsp.insert_endpoints[1]
                .as_ref()
                .unwrap()
                .endpoint
                .stats()
                .unaligned_delayed_dry_quanta
                > 0
        );
        wait_until(|| delayed_control.stats().completed >= 1);

        let delayed_slot = dsp.insert_endpoints[1].take().unwrap();
        dsp.insert_endpoints[1] = Some(InsertEndpointSlot {
            endpoint_id: 2,
            endpoint: fixed_adapter(invalid_audio),
            suppress_output_frames: DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES * 2,
        });
        dsp.track_block[start] = [1.0; 2];
        dsp.process_insert_endpoint(1, 1);
        assert_eq!(dsp.track_block[start], [0.0; 2]);
        assert_eq!(
            dsp.insert_endpoints[1]
                .as_ref()
                .unwrap()
                .suppress_output_frames,
            DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES * 2 - 1
        );

        let invalid_slot = dsp.insert_endpoints[1].take().unwrap();
        drop((delayed_slot.endpoint, invalid_slot.endpoint));
        delayed_guard.shutdown();
        invalid_guard.shutdown();
        drop((delayed_control, invalid_control));
    }

    #[test]
    fn master_capture_install_tap_and_stop_are_callback_confirmed_at_exact_frames() {
        let target = master_capture_test_path("lifecycle");
        let session = MasterCaptureSession::start_to_path(&target, 48_000).unwrap();
        let (endpoint, control) = session.into_parts();
        let capture_id = control.session_id();
        let (mut command_tx, mut command_rx) = RingBuffer::new(8);
        let (mut retired_tx, _retired_rx) = test_reclaimer();
        let (mut asset_event_tx, _asset_event_rx) = RingBuffer::new(8);
        let (capture_event_tx, mut capture_event_rx) = RingBuffer::new(8);
        let mut dsp = DspState::new_with_master_capture_io(48_000.0, capture_event_tx);

        dsp.set_device_frame(10_000);
        command_tx
            .push(AudioCommand::InstallMasterCapture {
                capture_id,
                endpoint,
            })
            .unwrap();
        process_commands(
            &mut dsp,
            &mut command_rx,
            &mut retired_tx,
            &mut asset_event_tx,
        );
        assert!(matches!(
            capture_event_rx.pop().unwrap(),
            MasterCaptureEndpointEvent::Installed {
                capture_id: id,
                start_device_frame: 10_000,
                success: true,
                returned_endpoint: None,
            } if id == capture_id
        ));

        dsp.master_block[..4].copy_from_slice(&[
            [0.1, -0.1],
            [0.2, -0.2],
            [0.3, -0.3],
            [0.4, -0.4],
        ]);
        dsp.capture_rendered_master(10_000, 4);
        let status = dsp.master_capture.as_ref().unwrap().endpoint.status();
        assert_eq!(status.first_device_frame, Some(10_000));
        assert_eq!(status.last_device_frame, Some(10_003));
        assert_eq!(status.source_gap_frames, 0);

        command_tx
            .push(AudioCommand::StopMasterCapture { capture_id })
            .unwrap();
        process_commands(
            &mut dsp,
            &mut command_rx,
            &mut retired_tx,
            &mut asset_event_tx,
        );
        let endpoint = match capture_event_rx.pop().unwrap() {
            MasterCaptureEndpointEvent::Stopped {
                capture_id: id,
                end_device_frame: 10_004,
                returned_endpoint: Some(endpoint),
            } if id == capture_id => endpoint,
            event => panic!("unexpected event: {event:?}"),
        };
        let metadata = control.stop_blocking(endpoint).unwrap();
        assert_eq!(metadata.frames, 4);
        assert_eq!(metadata.first_device_frame, Some(10_000));
        assert_eq!(metadata.last_device_frame, Some(10_003));
        assert!(metadata.is_valid());
        std::fs::remove_file(target).unwrap();
    }

    #[test]
    fn master_capture_lifecycle_waits_for_event_capacity_without_losing_ownership() {
        let target = master_capture_test_path("event-backpressure");
        let session = MasterCaptureSession::start_to_path(&target, 48_000).unwrap();
        let (endpoint, control) = session.into_parts();
        let capture_id = control.session_id();
        let (mut command_tx, mut command_rx) = RingBuffer::new(4);
        let (mut retired_tx, _retired_rx) = test_reclaimer();
        let (mut asset_event_tx, _asset_event_rx) = RingBuffer::new(4);
        let (mut capture_event_tx, mut capture_event_rx) = RingBuffer::new(1);
        capture_event_tx
            .push(MasterCaptureEndpointEvent::Stopped {
                capture_id: 999,
                end_device_frame: 0,
                returned_endpoint: None,
            })
            .unwrap();
        let mut dsp = DspState::new_with_master_capture_io(48_000.0, capture_event_tx);
        command_tx
            .push(AudioCommand::InstallMasterCapture {
                capture_id,
                endpoint,
            })
            .unwrap();

        process_commands(
            &mut dsp,
            &mut command_rx,
            &mut retired_tx,
            &mut asset_event_tx,
        );
        assert!(matches!(
            command_rx.peek(),
            Ok(AudioCommand::InstallMasterCapture { .. })
        ));
        let _ = capture_event_rx.pop().unwrap();
        process_commands(
            &mut dsp,
            &mut command_rx,
            &mut retired_tx,
            &mut asset_event_tx,
        );
        let _ = capture_event_rx.pop().unwrap();
        dsp.master_block[0] = [0.0; 2];
        dsp.capture_rendered_master(0, 1);
        command_tx
            .push(AudioCommand::StopMasterCapture { capture_id })
            .unwrap();
        process_commands(
            &mut dsp,
            &mut command_rx,
            &mut retired_tx,
            &mut asset_event_tx,
        );
        let endpoint = match capture_event_rx.pop().unwrap() {
            MasterCaptureEndpointEvent::Stopped {
                returned_endpoint: Some(endpoint),
                ..
            } => endpoint,
            event => panic!("unexpected event: {event:?}"),
        };
        control.stop_blocking(endpoint).unwrap();
        std::fs::remove_file(target).unwrap();
    }

    #[test]
    fn full_audio_command_queue_rejects_master_capture_on_the_control_thread() {
        let target = master_capture_test_path("queue-full");
        let session = MasterCaptureSession::start_to_path(&target, 48_000).unwrap();
        let (endpoint, control) = session.into_parts();
        let capture_id = control.session_id();
        let (mut producer, _consumer) = RingBuffer::new(1);
        producer.push(AudioCommand::SetMaster(0.5)).unwrap();
        let status = AudioStatus::default();
        assert!(!queue_audio_command(
            &mut producer,
            &status,
            AudioCommand::InstallMasterCapture {
                capture_id,
                endpoint,
            },
        ));
        drop(control);
        assert!(!target.exists());
        assert_eq!(status.command_queue_full.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn device_stream_lifecycle_requires_shutdown_only_after_successful_play() {
        let mut lifecycle = DeviceStreamLifecycle::Prepared;
        assert!(!lifecycle.requires_callback_shutdown());

        lifecycle.observe_play_result(false);
        assert_eq!(lifecycle, DeviceStreamLifecycle::Prepared);
        assert!(!lifecycle.requires_callback_shutdown());

        lifecycle.observe_play_result(true);
        assert_eq!(lifecycle, DeviceStreamLifecycle::Playing);
        assert!(lifecycle.requires_callback_shutdown());

        // A later failed/idempotent attempt must never demote a stream whose
        // callback may already own realtime resources.
        lifecycle.observe_play_result(false);
        assert_eq!(lifecycle, DeviceStreamLifecycle::Playing);
    }

    #[test]
    fn output_engine_rejects_an_input_profile_before_touching_hardware() {
        let error = AudioEngine::prepare_with_profile(&AudioDeviceProfile::system_default_input())
            .err()
            .expect("an input profile must be rejected");
        assert!(
            error
                .to_string()
                .contains("requires an output device profile")
        );
    }

    #[test]
    fn prepared_engine_drop_never_waits_for_an_unstarted_callback() {
        let (controller, realtime) = create_timeline_runtime();
        let mut engine = timeline_test_engine(controller);
        engine.stream_lifecycle = DeviceStreamLifecycle::Prepared;
        // Exercise the strongest branch: even conservative ownership hints do
        // not justify a callback barrier before `play()` succeeds.
        engine.has_realtime_owned_resources = true;
        engine.timeline_runtime_shutdown_confirmed = false;

        let started = Instant::now();
        drop(engine);
        assert!(
            started.elapsed() < Duration::from_millis(1_500),
            "a prepared engine waited for a callback that was never started"
        );
        drop(realtime);
    }

    #[test]
    fn backend_callback_helpers_preserve_full_sizes_and_classify_faults() {
        let telemetry = CallbackTelemetry::default();
        let status = AudioStatus::default();
        let multi_chunk_frames = MAX_MIXER_BLOCK_FRAMES * 2 + 17;

        observe_backend_output_callback(&telemetry, multi_chunk_frames * 2, 2);
        observe_backend_output_callback(&telemetry, 386, 2);
        observe_backend_stream_error(&status, &telemetry, ErrorKind::Xrun);
        observe_backend_stream_error(&status, &telemetry, ErrorKind::DeviceNotAvailable);

        let snapshot = telemetry.snapshot();
        assert_eq!(snapshot.callback_count, 2);
        assert_eq!(
            snapshot.total_frames,
            u64::try_from(multi_chunk_frames).unwrap() + 193
        );
        assert_eq!(snapshot.last_frames, Some(193));
        assert_eq!(snapshot.minimum_frames, Some(193));
        assert_eq!(
            snapshot.maximum_frames,
            Some(u32::try_from(multi_chunk_frames).unwrap())
        );
        assert_eq!(snapshot.size_changes, 1);
        assert_eq!(snapshot.unaligned_callbacks, 0);
        assert_eq!(snapshot.xrun_count, 1);
        assert_eq!(snapshot.stream_error_count, 1);
        assert_eq!(
            snapshot.last_error_kind,
            crate::audio_device::AudioStreamFaultKind::DeviceNotAvailable
        );
        assert_eq!(snapshot.last_error_revision, 2);
        assert_eq!(status.xruns.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn audio_snapshot_exposes_requested_effective_and_actual_callback_sizes() {
        let (controller, realtime) = create_timeline_runtime();
        let mut engine = timeline_test_engine(controller);
        engine.stream_lifecycle = DeviceStreamLifecycle::Prepared;
        engine.device_profile.buffer_size = AudioBufferSizeRequest::Nearest(300);
        engine.effective_device_profile.buffer_size = AudioBufferSizeRequest::Fixed(256);
        engine.effective_stream_config.buffer_size = AudioEffectiveBufferSize::Fixed(256);

        assert_eq!(
            engine.device_profile().buffer_size,
            AudioBufferSizeRequest::Nearest(300)
        );
        assert_eq!(
            engine.effective_device_profile().buffer_size,
            AudioBufferSizeRequest::Fixed(256)
        );

        let before = engine.snapshot();
        assert_eq!(
            before.requested_buffer_size,
            AudioBufferSizeRequest::Nearest(300)
        );
        assert_eq!(
            before.effective_buffer_size,
            AudioEffectiveBufferSize::Fixed(256)
        );
        assert_eq!(before.actual_buffer_size, None);

        observe_backend_output_callback(&engine.callback_telemetry, 514, 2);
        let after = engine.snapshot();
        assert_eq!(after.actual_buffer_size, Some(257));
        assert_eq!(engine.stream_telemetry().last_frames, Some(257));

        drop(engine);
        drop(realtime);
    }

    #[test]
    fn audio_engines_never_share_callback_telemetry() {
        let (first_controller, first_realtime) = create_timeline_runtime();
        let (second_controller, second_realtime) = create_timeline_runtime();
        let mut first = timeline_test_engine(first_controller);
        let mut second = timeline_test_engine(second_controller);
        first.stream_lifecycle = DeviceStreamLifecycle::Prepared;
        second.stream_lifecycle = DeviceStreamLifecycle::Prepared;

        observe_backend_output_callback(&first.callback_telemetry, 256, 2);
        observe_backend_stream_error(
            &first.status,
            &first.callback_telemetry,
            ErrorKind::DeviceChanged,
        );

        assert_eq!(first.stream_telemetry().callback_count, 1);
        assert_eq!(first.stream_telemetry().stream_error_count, 1);
        assert_eq!(
            second.stream_telemetry(),
            AudioStreamTelemetrySnapshot::default()
        );

        drop((first, second));
        drop((first_realtime, second_realtime));
    }

    #[test]
    fn audio_engine_drop_handshake_retires_insert_and_generator_without_hardware() {
        let insert = spawn_empty_chain(MAX_MIXER_BLOCK_FRAMES);
        let generator = spawn_empty_chain(MAX_MIXER_BLOCK_FRAMES);
        let PluginChain {
            audio: insert_audio,
            control: insert_control,
            guard: insert_guard,
        } = insert;
        let PluginChain {
            audio: generator_audio,
            control: generator_control,
            guard: generator_guard,
        } = generator;
        let capture_target = master_capture_test_path("engine-shutdown");
        let capture_session = MasterCaptureSession::start_to_path(&capture_target, 48_000).unwrap();
        let (capture_endpoint, capture_control) = capture_session.into_parts();
        let capture_id = capture_control.session_id();

        let status = Arc::new(AudioStatus::default());
        let (producer, mut command_rx) = RingBuffer::new(16);
        let (mut retired_asset_tx, retired_assets) = RingBuffer::new(4);
        let (mut asset_event_tx, asset_events) = RingBuffer::new(4);
        let (retired_endpoint_tx, retired_insert_endpoints) = RingBuffer::new(4);
        let (insert_event_tx, insert_endpoint_events) = RingBuffer::new(4);
        let (generator_event_tx, generator_endpoint_events) = RingBuffer::new(4);
        let (retired_midi_tx, retired_midi_inputs) = RingBuffer::new(4);
        let (midi_event_tx, midi_input_route_events) = RingBuffer::new(4);
        let (midi_recording_event_tx, midi_recording_endpoint_events) = RingBuffer::new(4);
        let (_parameter_edit_event_tx, parameter_edit_callback_events) = RingBuffer::new(4);
        let (master_capture_event_tx, master_capture_events) = RingBuffer::new(4);
        let (timeline_runtime, realtime_timeline_runtime) = create_timeline_runtime();
        let mut engine = AudioEngine {
            stream: None,
            stream_lifecycle: DeviceStreamLifecycle::Playing,
            meter_reader: meter_channel().1,
            timeline_runtime: Some(timeline_runtime),
            timeline_mixer_graph_fingerprint: None,
            producer,
            retired_assets,
            asset_events,
            retired_insert_endpoints,
            insert_endpoint_events,
            generator_endpoint_events,
            retired_midi_inputs,
            midi_input_route_events,
            midi_recording_endpoint_events,
            parameter_edit_callback_events,
            parameter_edit_callback_admission: Arc::new(AtomicU32::new(0)),
            master_capture_events,
            status,
            transport_mailbox: Arc::new(TransportMailbox::default()),
            callback_telemetry: Arc::new(CallbackTelemetry::default()),
            device_name: "test".into(),
            device_profile: AudioDeviceProfile::system_default_output(),
            effective_device_profile: AudioDeviceProfile::system_default_output(),
            effective_stream_config: AudioEffectiveStreamConfig {
                channels: 2,
                sample_rate: 48_000,
                sample_format: crate::audio_device::AudioSampleFormat::F32,
                buffer_size: AudioEffectiveBufferSize::Fixed(1),
            },
            has_realtime_owned_resources: false,
            timeline_runtime_shutdown_confirmed: false,
        };
        let callback_done = Arc::new(AtomicBool::new(false));
        let callback_done_worker = Arc::clone(&callback_done);
        let callback = thread::spawn(move || {
            let mut dsp = DspState::try_new_inner(
                48_000.0,
                Some(retired_endpoint_tx),
                Some(insert_event_tx),
                Some(generator_event_tx),
                Some(master_capture_event_tx),
                512,
                Some(realtime_timeline_runtime),
            )
            .unwrap();
            dsp.retired_midi_inputs = Some(retired_midi_tx);
            dsp.midi_input_route_events = Some(midi_event_tx);
            dsp.midi_recording_endpoint_events = Some(midi_recording_event_tx);
            while !callback_done_worker.load(Ordering::Acquire) {
                dsp.apply_pending_timeline_commands();
                process_commands(
                    &mut dsp,
                    &mut command_rx,
                    &mut retired_asset_tx,
                    &mut asset_event_tx,
                );
                thread::yield_now();
            }
            (
                dsp.insert_endpoints.iter().all(Option::is_none),
                dsp.generator_endpoints.iter().all(Option::is_none),
            )
        });

        assert!(engine.install_insert_endpoint(1, 1, insert_audio));
        assert!(engine.install_generator_endpoint(7, 2, 70, 1, generator_audio));
        assert!(engine.install_master_capture(capture_id, capture_endpoint));
        assert!(engine.shutdown_realtime_resources(Duration::from_secs(2)));
        callback_done.store(true, Ordering::Release);
        assert_eq!(callback.join().unwrap(), (true, true));
        assert!(!engine.has_realtime_owned_resources);
        drop(engine);
        insert_guard.shutdown();
        generator_guard.shutdown();
        drop((insert_control, generator_control));
        drop(capture_control);
        assert!(!capture_target.exists());
    }
}
