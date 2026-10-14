//! Inactive-by-default, run-owned observations for the manual genuine App test.
//!
//! Worker/control threads only. No callback may call this collector. Every tap
//! borrows an existing operand/result; it never calls a backend or changes one.
//! Private state and PCM stay in this run-owned value; Debug deliberately omits
//! their bytes. The driver owns hashing/persistence outside all callback paths.

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
use crate::instrument_bounce::render::{BlockReply, DisposalObservation};
use crate::{
    audio::{AudioAssetEvent, GeneratorEndpointEvent},
    instrument_bounce::{
        BouncePluginMetadata, BouncePluginState, BounceSourceIdentity, CapturedInstrumentBounce,
        render::{BackendFailure, RenderReport},
    },
    plugins::plugin_runtime::{
        BridgeStats, PluginEndpointManifest, PluginTransport, RuntimeEvent, WorkerIdentityWitness,
    },
};
use std::{
    cell::{Cell, RefCell},
    sync::{Arc, Mutex, Weak},
    time::Instant,
};

// Explicit complete-song transcript building blocks. The existing memory
// collector and its caps remain unchanged until a separately owned sink is
// installed; these modules do not activate observation or an SDK operation.
#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
mod stream_app_control;
#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
mod stream_body;
#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
mod stream_codec;
#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
mod stream_control;
#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
mod stream_envelope;
#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
mod stream_instances;
#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
mod stream_instances_tests;
#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
mod stream_live;
#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
pub(crate) use stream_live::{
    StreamError, StreamFinishPoll, StreamRecoveryHandle, StreamRetirement,
};
#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
mod stream_export_audit;
#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
mod stream_replay;
#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
pub(crate) use stream_export_audit::StreamReplayEvidence;
#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
mod stream_sink;
#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
mod stream_spool;
#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
mod stream_spool_tests;
#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
mod stream_state;

pub(crate) const MAX_RECORDS: usize = 32_768;
pub(crate) const MAX_RECORD_BATCH: usize = 64;
pub(crate) const MAX_WORKER_IDENTITIES: usize = 4;
pub(crate) const MAX_TIMELINE_EVENTS: usize = 256;
// Cumulative copied payload accounting; this is not an allocator/RSS limit.
pub(crate) const MAX_PRIVATE_BYTES: usize = 256 * 1024 * 1024;
pub(crate) const MAX_STATE_BYTES: usize = 64 * 1024 * 1024;
#[cfg(feature = "vst3")]
pub(crate) const MAX_SOURCE_POSITIVE_ATTEMPTS: usize = 4096;
#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
pub(crate) const MAX_CLONE_REPLIES: usize = 282;
const MAX_TEXT: usize = 4096;
const MAX_BUSES: usize = 32;
const MAX_BASES: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(
    not(all(target_os = "linux", feature = "experimental-fresh-helper")),
    allow(dead_code)
)] // Manual genuine consumer is feature-gated.
pub(crate) enum Stage {
    Preflight,
    InitialSource,
    SourceAudibility,
    SourceStop,
    Conversion,
    FirstPlayback,
    Undo,
    Redo,
    SecondPlayback,
    WholeExport,
    Cleanup,
    Finished,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InstancePhase {
    InitialSource,
    StrictClone,
    UndoSource,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct InstanceIdentity {
    pub ordinal: u64,
    pub phase: InstancePhase,
    pub worker_id: Option<usize>,
    pub source_instance_id: Option<u64>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(
    not(all(target_os = "linux", feature = "experimental-fresh-helper")),
    allow(dead_code)
)] // Manual genuine consumer is feature-gated.
pub(crate) enum CaptureLabel {
    Initial,
    Final,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(
    not(all(target_os = "linux", feature = "experimental-fresh-helper")),
    allow(dead_code)
)] // Manual genuine consumer is feature-gated.
#[cfg(feature = "vst3")]
pub(crate) enum Operation {
    SourceLoad,
    SourcePrepare,
    OrdinaryProcess,
    Reset,
    ResetOrigin,
    MidiPanic,
    StopProcessing,
    StartProcessing,
    SaveState,
    StrictPrepare,
    StrictMetadata,
    StrictMidi,
    StrictProcess,
    StrictDispose,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg(feature = "vst3")]
pub(crate) enum Boundary {
    Attempt,
    Returned,
}
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    not(all(target_os = "linux", feature = "experimental-fresh-helper")),
    allow(dead_code)
)] // Manual genuine consumer is feature-gated.
pub(crate) enum CallOutcome {
    Ok,
    Error(String),
    StrictError(BackendFailure),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkerJoinOutcome {
    Joined,
    WorkerPanicked,
}

/// A literal, copy-only mirror of the App confirmation phase. Request IDs,
/// generations, seek serials and epochs remain separate identity domains.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(
    not(all(target_os = "linux", feature = "experimental-fresh-helper")),
    allow(dead_code)
)]
// Every phase operand is retained evidence; the complete consumer is feature-gated.
pub(crate) enum TimelinePhase {
    Unavailable,
    InstallReady,
    InstallQueued {
        request_id: u64,
        generation: u64,
    },
    LoopReady,
    LoopQueued {
        request_id: u64,
        generation: u64,
        frame: u64,
    },
    SeekReady,
    SeekQueued {
        request_id: u64,
        generation: u64,
        seek_serial: u64,
        epoch: u64,
        frame: u64,
    },
    AwaitingActivation {
        generation: u64,
        seek_serial: u64,
        epoch: u64,
        frame: u64,
        activation_request_id: u64,
    },
    Active {
        generation: u64,
        epoch: u64,
    },
    AssetHandoffClearReady {
        revision: u64,
    },
    AssetHandoffClearQueued {
        request_id: u64,
        revision: u64,
    },
    FaultClearReady {
        revision: u64,
    },
    FaultClearQueued {
        request_id: u64,
        revision: u64,
    },
    Faulted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(
    not(all(target_os = "linux", feature = "experimental-fresh-helper")),
    allow(dead_code)
)]
// These exact identity domains are intentionally copied even in core test profiles.
pub(crate) struct TimelineAppState {
    pub phase: TimelinePhase,
    pub project_session: u64,
    pub engine_session: Option<u64>,
    pub sync_session: u64,
    pub desired_generation: u64,
    pub observed_fingerprint: u64,
    pub compiled_generation: Option<u64>,
    pub compiled_fingerprint: Option<u64>,
    pub source_generation: Option<u64>,
    pub source_fingerprint: Option<u64>,
    pub sync_source_generation: Option<u64>,
    pub desired_seek_serial: u64,
    pub desired_seek_frame: u64,
    pub loop_token: Option<u64>,
    pub loop_disable_request: Option<u64>,
    pub retry_count: u64,
    pub stale_confirmations: u64,
    pub confirmed_identity: Option<(u64, u64)>,
    pub retry_required: bool,
    pub retry_pending: Option<(u64, u64)>,
}

/// Exactly the once-sampled operands and result of the canonical worker guard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WorkerCaptureGuard {
    pub expected_epoch: u64,
    pub active_epoch: u64,
    pub requested_epoch: u64,
    pub input_empty: bool,
    pub parameter_admission: u32,
    pub accepted: bool,
}

// Intentionally no Debug on payload-bearing records: opaque/PCM must never
// accidentally enter the public evidence archive through a formatted snapshot.
#[allow(dead_code)] // Raw evidence fields are retained even when the fixed oracle reads a subset.
pub(crate) enum RecordKind {
    #[cfg(feature = "vst3")]
    Call {
        operation: Operation,
        boundary: Boundary,
        frames: Option<usize>,
        transport: Option<PluginTransport>,
        outcome: Option<CallOutcome>,
    },
    StateLoad {
        bytes: Vec<u8>,
        outcome: CallOutcome,
    },
    BaseRead {
        site: &'static str,
        id: u32,
        raw_bits: Result<u64, String>,
    },
    Metadata {
        site: &'static str,
        result: Result<BouncePluginMetadata, String>,
    },
    AudioDeclaration {
        input_buses: Vec<crate::instrument_bounce::BounceAudioBus>,
        output_buses: Vec<crate::instrument_bounce::BounceAudioBus>,
    },
    EventDeclaration {
        inputs: Vec<(u32, bool, Option<bool>)>,
        outputs: Vec<(u32, bool, Option<bool>)>,
    },
    StrictPcm {
        frames: usize,
        transport: PluginTransport,
        channels: Vec<Vec<f32>>,
        transport_applied: bool,
        output_events: usize,
        output_events_lost: bool,
    },
    Midi {
        data: [u8; 3],
        offset: u16,
    },
    Capture {
        label: CaptureLabel,
        identity: BounceSourceIdentity,
        receipt_identity: usize,
        state: BouncePluginState,
    },
    AppAudioAssetEvent(AudioAssetEvent),
    AppGeneratorEndpointEvent(GeneratorEndpointEvent),
    AppTimelineRuntimeEvent {
        event: crate::timeline_runtime::TimelineRuntimeEvent,
        before: TimelineAppState,
        after: TimelineAppState,
    },
    WorkerCaptureGuard {
        worker_id: Option<usize>,
        operands: WorkerCaptureGuard,
    },
    WorkerJoin {
        worker_id: usize,
        outcome: WorkerJoinOutcome,
    },
    RuntimeEvent {
        endpoint_id: u64,
        project_session: u64,
        instance_id: u64,
        event: RuntimeEvent,
    },
    OrdinaryState {
        endpoint_id: u64,
        project_session: u64,
        instance_id: u64,
        slot: usize,
        request_id: u64,
        epoch: Option<u64>,
        byte_len: usize,
    },
    ResetReport {
        frames: u32,
        discarded_prior_midi_events: u64,
        discarded_midi_events: u64,
        discarded_prior_parameter_points: u64,
        discarded_parameter_points: u64,
        output_events_lost: bool,
        parameter_output_fault: bool,
        process_error: Option<String>,
    },
    StrictCaptureIdentity(BounceSourceIdentity),
    RenderReport(RenderReport),
    Disposal {
        terminated: bool,
        error: Option<BackendFailure>,
        diagnostics: Vec<String>,
    },
    Worker {
        site: &'static str,
        worker_id: usize,
        manifest: PluginEndpointManifest,
        epoch: u64,
        sequence: Option<u64>,
        stats: BridgeStats,
    },
    HookInventory {
        site: &'static str,
        inactive: bool,
    },
    Lifecycle(&'static str),
}
#[cfg_attr(
    not(all(target_os = "linux", feature = "experimental-fresh-helper")),
    allow(dead_code)
)] // Manual genuine consumer is feature-gated.
pub(crate) struct Record {
    pub ordinal: u64,
    /// Nanoseconds on this installed run's monotonic clock. Worker/control
    /// taps only; inactive observers never sample this clock.
    pub monotonic_elapsed_ns: u64,
    pub stage: Stage,
    pub instance: Option<InstanceIdentity>,
    pub kind: RecordKind,
}
#[cfg_attr(
    not(all(target_os = "linux", feature = "experimental-fresh-helper")),
    allow(dead_code)
)] // Manual genuine consumer is feature-gated.
pub(crate) struct ObservationSnapshot {
    pub records: Vec<Arc<Record>>,
    pub failure: Option<&'static str>,
    pub private_bytes: usize,
    pub source_positive_attempts: usize,
    pub clone_replies: usize,
    #[cfg(all(
        any(target_os = "linux", windows),
        feature = "experimental-fresh-helper"
    ))]
    pub clone_reply_cap: usize,
    pub timeline_events: usize,
    started: Instant,
    stage: Stage,
    next_instance: u64,
    phases: [bool; 3],
    record_cap: usize,
    byte_cap: usize,
    worker_identities: [Option<WorkerIdentityWitness>; MAX_WORKER_IDENTITIES],
    // Detachment stops new lookups, but an in-flight tap can already own an
    // Arc. Retirement under this same mutex prevents that tap from rebuilding
    // records or witness ownership after the driver releases the run.
    retired: bool,
    #[cfg(all(
        any(target_os = "linux", windows),
        feature = "experimental-fresh-helper"
    ))]
    stream: Option<stream_live::LiveStream>,
    #[cfg(all(
        any(target_os = "linux", windows),
        feature = "experimental-fresh-helper"
    ))]
    stream_taken: bool,
}
impl ObservationSnapshot {
    fn new(record_cap: usize, byte_cap: usize) -> Result<Self, &'static str> {
        let mut records = Vec::new();
        records
            .try_reserve_exact(record_cap)
            .map_err(|_| "observer record reservation failed")?;
        Ok(Self {
            records,
            failure: None,
            private_bytes: 0,
            source_positive_attempts: 0,
            clone_replies: 0,
            #[cfg(all(
                any(target_os = "linux", windows),
                feature = "experimental-fresh-helper"
            ))]
            clone_reply_cap: MAX_CLONE_REPLIES,
            timeline_events: 0,
            started: Instant::now(),
            stage: Stage::Preflight,
            next_instance: 1,
            phases: [false; 3],
            record_cap,
            byte_cap,
            worker_identities: std::array::from_fn(|_| None),
            retired: false,
            #[cfg(all(
                any(target_os = "linux", windows),
                feature = "experimental-fresh-helper"
            ))]
            stream: None,
            #[cfg(all(
                any(target_os = "linux", windows),
                feature = "experimental-fresh-helper"
            ))]
            stream_taken: false,
        })
    }
    fn retired() -> Self {
        let mut state = Self::new(0, 0).expect("zero-capacity observer tombstone");
        state.retired = true;
        state
    }
    fn fail(&mut self, failure: &'static str) {
        self.failure.get_or_insert(failure);
        #[cfg(all(
            any(target_os = "linux", windows),
            feature = "experimental-fresh-helper"
        ))]
        if let Some(stream) = &mut self.stream {
            stream.observer_failed(failure);
        }
    }
    #[cfg(all(
        any(target_os = "linux", windows),
        feature = "experimental-fresh-helper"
    ))]
    fn try_stream(
        &mut self,
        instance: Option<InstanceIdentity>,
        view: stream_live::View<'_>,
    ) -> bool {
        if self.stream_taken {
            self.fail("observer tap after stream retirement");
            return true;
        }
        if self.stream.is_none() {
            return false;
        }
        let monotonic_ns = self.elapsed_ns();
        let stream = self.stream.as_mut().expect("checked live stream");
        if stream.failure().is_some() {
            return true;
        }
        let sequence = stream.next_sequence();
        match stream.emit(sequence, monotonic_ns, self.stage, instance, view) {
            Ok(()) => true,
            Err(_) => {
                self.failure
                    .get_or_insert("observer stream original operand rejected");
                false
            }
        }
    }
    #[cfg(all(
        test,
        any(target_os = "linux", windows),
        feature = "experimental-fresh-helper"
    ))]
    fn set_qa_clone_reply_limit(&mut self, limit: usize) -> Result<(), &'static str> {
        if !(1..=4096).contains(&limit) {
            return Err("QA observer clone reply limit must be within 1..=4096");
        }
        self.clone_reply_cap = limit;
        Ok(())
    }

    #[cfg(all(
        any(target_os = "linux", windows),
        feature = "experimental-fresh-helper"
    ))]
    fn strict_pcm(
        &mut self,
        identity: Option<InstanceIdentity>,
        frames: usize,
        transport: PluginTransport,
        reply: &BlockReply,
    ) {
        #[cfg(all(
            any(target_os = "linux", windows),
            feature = "experimental-fresh-helper"
        ))]
        {
            if self.stream.is_some() || self.stream_taken {
                if reply.channels.len() > 2
                    || reply.channels.iter().any(|channel| channel.len() > 128)
                {
                    self.fail("observer strict PCM cap exceeded");
                    return;
                }
                if self.try_stream(
                    identity,
                    stream_live::View::Native(stream_codec::NativeView::StrictPcm {
                        frames,
                        transport: &transport,
                        channels: &reply.channels,
                        transport_applied: reply.transport_applied,
                        output_events: reply.output_events,
                        output_events_lost: reply.output_events_lost,
                    }),
                ) {
                    return;
                }
                let bytes = reply
                    .channels
                    .iter()
                    .map(|c| c.len() * std::mem::size_of::<f32>())
                    .sum();
                self.push(identity, bytes, || RecordKind::StrictPcm {
                    frames,
                    transport,
                    channels: reply.channels.clone(),
                    transport_applied: reply.transport_applied,
                    output_events: reply.output_events,
                    output_events_lost: reply.output_events_lost,
                });
                return;
            }
        }
        if self.clone_replies >= self.clone_reply_cap
            || reply.channels.len() > 2
            || reply.channels.iter().any(|channel| channel.len() > 128)
        {
            self.fail("observer strict PCM cap exceeded");
            return;
        }
        self.clone_replies += 1;
        let bytes = reply
            .channels
            .iter()
            .map(|channel| channel.len() * std::mem::size_of::<f32>())
            .sum();
        self.push(identity, bytes, || RecordKind::StrictPcm {
            frames,
            transport,
            channels: reply.channels.clone(),
            transport_applied: reply.transport_applied,
            output_events: reply.output_events,
            output_events_lost: reply.output_events_lost,
        });
    }
    fn timeline_event(
        &mut self,
        event: crate::timeline_runtime::TimelineRuntimeEvent,
        before: TimelineAppState,
        after: TimelineAppState,
    ) {
        // This lifetime count is never reset by copying/draining records.
        // Overflow is sticky evidence, never input to the App handler.
        if self.timeline_events >= MAX_TIMELINE_EVENTS {
            self.fail("observer Timeline event cap exceeded");
            return;
        }
        self.timeline_events += 1;
        #[cfg(all(
            any(target_os = "linux", windows),
            feature = "experimental-fresh-helper"
        ))]
        if self.try_stream(
            None,
            stream_live::View::App(stream_app_control::AppView::Timeline {
                event: &event,
                before: &before,
                after: &after,
            }),
        ) {
            return;
        }
        self.push(None, 0, || RecordKind::AppTimelineRuntimeEvent {
            event,
            before,
            after,
        });
    }
    // Cap BEFORE invoking build, so even an overflowing copy closure is not run.
    fn push(
        &mut self,
        instance: Option<InstanceIdentity>,
        bytes: usize,
        build: impl FnOnce() -> RecordKind,
    ) {
        #[cfg(all(
            any(target_os = "linux", windows),
            feature = "experimental-fresh-helper"
        ))]
        let streamed_header = if self.stream_taken {
            return;
        } else if let Some(stream) = &mut self.stream {
            let Some(header) = stream.failed_header_to_retain.take() else {
                return;
            };
            Some(header)
        } else {
            None
        };
        if self.records.len() >= self.record_cap {
            self.fail("observer record cap exceeded");
            return;
        }
        let Some(total) = self
            .private_bytes
            .checked_add(bytes)
            .filter(|n| *n <= self.byte_cap)
        else {
            self.fail("observer byte cap exceeded");
            return;
        };
        self.private_bytes = total;
        #[cfg(all(
            any(target_os = "linux", windows),
            feature = "experimental-fresh-helper"
        ))]
        let (ordinal, monotonic_elapsed_ns) = if let Some(header) = streamed_header {
            (header.sequence, header.monotonic_ns)
        } else {
            (self.records.len() as u64 + 1, self.elapsed_ns())
        };
        #[cfg(not(all(
            any(target_os = "linux", windows),
            feature = "experimental-fresh-helper"
        )))]
        let (ordinal, monotonic_elapsed_ns) = (self.records.len() as u64 + 1, self.elapsed_ns());
        let record = Arc::new(Record {
            ordinal,
            monotonic_elapsed_ns,
            stage: self.stage,
            instance,
            kind: build(),
        });
        #[cfg(all(
            any(target_os = "linux", windows),
            feature = "experimental-fresh-helper"
        ))]
        if streamed_header.is_some()
            && let Some(stream) = &mut self.stream
        {
            stream.retain_failed_original(Arc::clone(&record));
        }
        self.records.push(record);
    }
    fn elapsed_ns(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }
    fn bind_instance(
        &mut self,
        phase: InstancePhase,
        worker: Option<WorkerIdentity>,
    ) -> InstanceIdentity {
        let (index, expected_stage) = match phase {
            InstancePhase::InitialSource => (0, self.stage == Stage::InitialSource),
            InstancePhase::StrictClone => (
                1,
                matches!(self.stage, Stage::Conversion | Stage::WholeExport),
            ),
            InstancePhase::UndoSource => (2, self.stage == Stage::Undo),
        };
        if !expected_stage || self.phases[index] || self.next_instance != index as u64 + 1 {
            self.fail("observer unexpected instance phase, order or duplicate owner");
        }
        self.phases[index] = true;
        let identity = InstanceIdentity {
            ordinal: self.next_instance,
            phase,
            worker_id: worker.map(|w| w.id),
            source_instance_id: worker.and_then(|w| w.manifest.instance_id(0)),
        };
        self.next_instance += 1;
        identity
    }
    fn stage(&mut self, next: Stage) -> Result<(), &'static str> {
        if next < self.stage || self.stage == Stage::Finished {
            self.fail("observer phase regression");
            return Err("observer phase regression");
        }
        self.stage = next;
        Ok(())
    }
}

static ACTIVE: Mutex<Option<Weak<Mutex<ObservationSnapshot>>>> = Mutex::new(None);
fn active() -> Option<Arc<Mutex<ObservationSnapshot>>> {
    ACTIVE
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .and_then(Weak::upgrade)
}
fn with_active(f: impl FnOnce(&mut ObservationSnapshot)) {
    if let Some(run) = active() {
        with_run(&run, f);
    }
}
fn with_run(run: &Arc<Mutex<ObservationSnapshot>>, f: impl FnOnce(&mut ObservationSnapshot)) {
    match run.lock() {
        Ok(mut state) => {
            if !state.retired {
                f(&mut state);
            }
        }
        Err(poison) => {
            let mut state = poison.into_inner();
            if !state.retired {
                state.fail("observer mutex poisoned");
                f(&mut state);
            }
        }
    }
}

/// Synchronous worker/control-side registration before spawn. The builder is
/// never called without an active run and a free, lifetime-owned witness slot.
pub(crate) fn retain_worker_identity(build: impl FnOnce() -> WorkerIdentityWitness) {
    with_active(|state| {
        let Some(slot) = state
            .worker_identities
            .iter_mut()
            .find(|slot| slot.is_none())
        else {
            state.fail("observer worker identity cap exceeded");
            return;
        };
        *slot = Some(build());
    });
}

#[cfg_attr(
    not(all(target_os = "linux", feature = "experimental-fresh-helper")),
    allow(dead_code)
)] // Manual genuine consumer is feature-gated.
pub(crate) struct ObservationRun {
    state: Arc<Mutex<ObservationSnapshot>>,
}
#[cfg_attr(
    not(all(target_os = "linux", feature = "experimental-fresh-helper")),
    allow(dead_code)
)] // Manual genuine consumer is feature-gated.
impl ObservationRun {
    pub(crate) fn install() -> Result<Self, &'static str> {
        Self::install_configured(|_| Ok(()))
    }

    /// Explicit test-driver storage budget for a longer genuine render. This
    /// changes only copied observations; SDK/process and other collector caps
    /// remain unchanged and overflow still rejects the qualification run.
    #[cfg(all(
        test,
        any(target_os = "linux", windows),
        feature = "experimental-fresh-helper"
    ))]
    pub(crate) fn install_with_clone_reply_limit(limit: usize) -> Result<Self, &'static str> {
        if !(1..=4096).contains(&limit) {
            return Err("QA observer clone reply limit must be within 1..=4096");
        }
        Self::install_configured(|state| state.set_qa_clone_reply_limit(limit))
    }

    fn install_configured(
        configure: impl FnOnce(&mut ObservationSnapshot) -> Result<(), &'static str>,
    ) -> Result<Self, &'static str> {
        let mut slot = ACTIVE
            .lock()
            .map_err(|_| "observer activation mutex poisoned")?;
        if slot.as_ref().and_then(Weak::upgrade).is_some() {
            return Err("an observation run is already active");
        }
        let mut snapshot = ObservationSnapshot::new(MAX_RECORDS, MAX_PRIVATE_BYTES)?;
        configure(&mut snapshot)?;
        let state = Arc::new(Mutex::new(snapshot));
        *slot = Some(Arc::downgrade(&state));
        Ok(Self { state })
    }
    pub(crate) fn set_stage(&self, next: Stage) -> Result<(), &'static str> {
        let mut result = Ok(());
        with_run(&self.state, |state| result = state.stage(next));
        result
    }
    #[cfg(all(
        any(target_os = "linux", windows),
        feature = "experimental-fresh-helper"
    ))]
    pub(crate) fn enable_stream(
        &self,
        transcript: &std::path::Path,
        state_sidecar: &std::path::Path,
        source: InstanceIdentity,
        deadline: Instant,
    ) -> Result<(), StreamError> {
        let mut result = Err(stream_live::StreamError::admission(
            "observer stream lock unavailable",
        ));
        with_run(&self.state, |state| {
            if state.stream.is_some()
                || state.stream_taken
                || state.failure.is_some()
                || state.records.len() >= state.record_cap
            {
                result = Err(stream_live::StreamError::admission(
                    "stream requires an intact unstreamed prelude and room for one bounded failed record",
                ));
                return;
            }
            match stream_live::LiveStream::start(
                transcript,
                state_sidecar,
                source,
                deadline,
                &state.records,
            ) {
                Ok(stream) => {
                    let failure = stream.failure().cloned();
                    state.stream = Some(stream);
                    if failure.is_some() {
                        state
                            .failure
                            .get_or_insert("observer stream prelude or startup failed");
                    }
                    result = failure.map_or(Ok(()), Err);
                }
                Err(error) => result = Err(error),
            }
        });
        result
    }
    /// Call only after the driver has actually retired all App/callback/source/clone tap owners.
    #[cfg(all(
        any(target_os = "linux", windows),
        feature = "experimental-fresh-helper"
    ))]
    pub(crate) fn take_stream_after_taps_retired(
        &self,
        taps_retired: bool,
    ) -> Result<StreamRetirement, StreamError> {
        if !taps_retired {
            return Err(stream_live::StreamError::admission(
                "actual caller tap retirement is required",
            ));
        }
        let mut result = Err(stream_live::StreamError::admission(
            "stream owner is absent or already transferred",
        ));
        with_run(&self.state, |state| {
            if let Some(stream) = state.stream.take() {
                state.stream_taken = true;
                result = Ok(stream.retire());
            }
        });
        result
    }
    #[cfg(all(
        any(target_os = "linux", windows),
        feature = "experimental-fresh-helper"
    ))]
    pub(crate) fn stream_recovery_handle(&self) -> Option<StreamRecoveryHandle> {
        self.with_snapshot(|state| {
            state
                .stream
                .as_ref()
                .map(stream_live::LiveStream::recovery_handle)
        })
    }
    pub(crate) fn with_snapshot<T>(&self, inspect: impl FnOnce(&ObservationSnapshot) -> T) -> T {
        let state = self.state.lock().unwrap_or_else(|p| {
            let mut state = p.into_inner();
            state.fail("observer mutex poisoned");
            state
        });
        inspect(&state)
    }
    /// Driver-side alignment with worker taps, outside any callback operation.
    pub(crate) fn monotonic_elapsed_ns(&self) -> u64 {
        self.with_snapshot(ObservationSnapshot::elapsed_ns)
    }
    /// Copy only a bounded batch of immutable record handles. No private
    /// opaque or PCM payload is copied. Hash/format/write after releasing this
    /// lock; drain each batch before requesting another persistence batch.
    pub(crate) fn copy_records_since(&self, start: usize) -> Vec<Arc<Record>> {
        let mut state = self.state.lock().unwrap_or_else(|p| {
            let mut state = p.into_inner();
            state.fail("observer mutex poisoned");
            state
        });
        if start > state.records.len() {
            state.fail("observer copy cursor exceeded record count");
            return Vec::new();
        }
        let end = start
            .saturating_add(MAX_RECORD_BATCH)
            .min(state.records.len());
        state.records[start..end].to_vec()
    }
    pub(crate) fn finish(self) -> ObservationSnapshot {
        self.detach();
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        std::mem::replace(&mut *state, ObservationSnapshot::retired())
    }
    fn detach(&self) {
        let mut slot = ACTIVE.lock().unwrap_or_else(|p| p.into_inner());
        if slot
            .as_ref()
            .and_then(Weak::upgrade)
            .is_some_and(|state| Arc::ptr_eq(&state, &self.state))
        {
            *slot = None;
        }
    }
}
impl Drop for ObservationRun {
    fn drop(&mut self) {
        self.detach();
        // A worker may have upgraded ACTIVE before detach. Release the run's
        // payload now rather than waiting for that temporary Arc to disappear;
        // every delayed tap sees the retired tombstone under the same lock.
        let retired = {
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            std::mem::replace(&mut *state, ObservationSnapshot::retired())
        };
        drop(retired);
    }
}

#[derive(Clone, Copy)]
struct WorkerIdentity {
    id: usize,
    manifest: PluginEndpointManifest,
}
thread_local! { static WORKER: Cell<Option<WorkerIdentity>> = const { Cell::new(None) }; }
pub(crate) struct WorkerScope(Option<WorkerIdentity>);
pub(crate) fn enter_worker(id: usize, manifest: PluginEndpointManifest) -> WorkerScope {
    let previous = WORKER.with(|slot| slot.replace(Some(WorkerIdentity { id, manifest })));
    WorkerScope(previous)
}
impl Drop for WorkerScope {
    fn drop(&mut self) {
        WORKER.with(|slot| slot.set(self.0));
    }
}

#[derive(Clone, Default)]
pub(crate) struct InstanceObserver {
    run: Option<Weak<Mutex<ObservationSnapshot>>>,
    identity: Option<InstanceIdentity>,
}
impl InstanceObserver {
    #[cfg(feature = "vst3")]
    pub(crate) fn new(phase: InstancePhase) -> Self {
        let Some(run) = active() else {
            return Self::default();
        };
        let worker = WORKER.with(Cell::get);
        let mut identity = None;
        with_run(&run, |state| {
            let value = state.bind_instance(phase, worker);
            identity = Some(value);
            #[cfg(all(
                any(target_os = "linux", windows),
                feature = "experimental-fresh-helper"
            ))]
            if state.try_stream(
                identity,
                stream_live::View::Control(stream_control::View::Lifecycle(
                    "instance construction observed",
                )),
            ) {
                return;
            }
            state.push(identity, 0, || {
                RecordKind::Lifecycle("instance construction observed")
            });
        });
        Self {
            run: Some(Arc::downgrade(&run)),
            identity,
        }
    }
    fn with(&self, f: impl FnOnce(&mut ObservationSnapshot, Option<InstanceIdentity>)) {
        if let Some(run) = self.run.as_ref().and_then(Weak::upgrade) {
            with_run(&run, |state| f(state, self.identity));
        }
    }
    #[cfg(feature = "vst3")]
    pub(crate) fn call(
        &self,
        operation: Operation,
        boundary: Boundary,
        frames: Option<usize>,
        transport: Option<PluginTransport>,
        outcome: Option<CallOutcome>,
    ) {
        self.with(|state, identity| {
            if boundary == Boundary::Attempt
                && matches!(
                    operation,
                    Operation::OrdinaryProcess | Operation::ResetOrigin
                )
                && frames.is_some_and(|n| n > 0)
            {
                #[cfg(all(
                    any(target_os = "linux", windows),
                    feature = "experimental-fresh-helper"
                ))]
                let memory = state.stream.is_none() && !state.stream_taken;
                #[cfg(not(all(
                    any(target_os = "linux", windows),
                    feature = "experimental-fresh-helper"
                )))]
                let memory = true;
                if memory {
                    state.source_positive_attempts += 1;
                    if state.source_positive_attempts > MAX_SOURCE_POSITIVE_ATTEMPTS {
                        state.fail("source positive processing attempt cap exceeded");
                    }
                }
            }
            let bytes = outcome.as_ref().map_or(0, outcome_size);
            if bytes > MAX_TEXT {
                state.fail("observer diagnostic cap exceeded");
                return;
            }
            #[cfg(all(
                any(target_os = "linux", windows),
                feature = "experimental-fresh-helper"
            ))]
            if state.try_stream(
                identity,
                stream_live::View::Control(stream_control::View::Call {
                    operation,
                    boundary,
                    frames,
                    transport: transport.as_ref(),
                    outcome: outcome.as_ref().map(stream_control::OutcomeRef::from),
                }),
            ) {
                return;
            }
            state.push(identity, bytes, || RecordKind::Call {
                operation,
                boundary,
                frames,
                transport,
                outcome,
            });
        });
    }
    #[cfg(feature = "vst3")]
    pub(crate) fn string_result<T>(
        &self,
        operation: Operation,
        frames: Option<usize>,
        transport: Option<PluginTransport>,
        result: &Result<T, String>,
    ) {
        self.with(|state, identity| {
            let bytes = result.as_ref().err().map_or(0, |s| s.len());
            if bytes > MAX_TEXT {
                state.fail("observer diagnostic cap exceeded");
                return;
            }
            #[cfg(all(
                any(target_os = "linux", windows),
                feature = "experimental-fresh-helper"
            ))]
            if state.try_stream(
                identity,
                stream_live::View::Control(stream_control::View::Call {
                    operation,
                    boundary: Boundary::Returned,
                    frames,
                    transport: transport.as_ref(),
                    outcome: Some(match result {
                        Ok(_) => stream_control::OutcomeRef::Ok,
                        Err(error) => stream_control::OutcomeRef::Error(error),
                    }),
                }),
            ) {
                return;
            }
            state.push(identity, bytes, || RecordKind::Call {
                operation,
                boundary: Boundary::Returned,
                frames,
                transport,
                outcome: Some(match result {
                    Ok(_) => CallOutcome::Ok,
                    Err(error) => CallOutcome::Error(error.clone()),
                }),
            });
        });
    }
    #[cfg(all(
        any(target_os = "linux", windows),
        feature = "experimental-fresh-helper"
    ))]
    pub(crate) fn strict_result<T>(
        &self,
        operation: Operation,
        frames: Option<usize>,
        transport: Option<PluginTransport>,
        result: &Result<T, BackendFailure>,
    ) {
        self.with(|state, identity| {
            let bytes = result.as_ref().err().map_or(0, |e| e.diagnostic().len());
            if bytes > MAX_TEXT {
                state.fail("observer strict diagnostic cap exceeded");
                return;
            }
            if state.try_stream(
                identity,
                stream_live::View::Control(stream_control::View::Call {
                    operation,
                    boundary: Boundary::Returned,
                    frames,
                    transport: transport.as_ref(),
                    outcome: Some(match result {
                        Ok(_) => stream_control::OutcomeRef::Ok,
                        Err(error) => stream_control::OutcomeRef::StrictError(error),
                    }),
                }),
            ) {
                return;
            }
            state.push(identity, bytes, || RecordKind::Call {
                operation,
                boundary: Boundary::Returned,
                frames,
                transport,
                outcome: Some(match result {
                    Ok(_) => CallOutcome::Ok,
                    Err(error) => CallOutcome::StrictError(error.clone()),
                }),
            });
        });
    }
    #[cfg(all(
        any(target_os = "linux", windows),
        feature = "experimental-fresh-helper"
    ))]
    pub(crate) fn capture_identity(&self, source: BounceSourceIdentity) {
        self.with(|state, identity| {
            if state.try_stream(
                identity,
                stream_live::View::Control(stream_control::View::StrictCaptureIdentity(&source)),
            ) {
                return;
            }
            state.push(identity, 0, || RecordKind::StrictCaptureIdentity(source))
        });
    }
    #[cfg(all(
        any(target_os = "linux", windows),
        feature = "experimental-fresh-helper"
    ))]
    pub(crate) fn strict_state_load(&self, bytes: &[u8], result: &Result<(), BackendFailure>) {
        self.with(|state, identity| {
            let diagnostic_bytes = result.as_ref().err().map_or(0, |e| e.diagnostic().len());
            if bytes.len() > MAX_STATE_BYTES || diagnostic_bytes > MAX_TEXT {
                state.fail("observer state-load input cap exceeded");
                return;
            }
            if state.try_stream(
                identity,
                stream_live::View::StateLoad {
                    bytes,
                    outcome: match result {
                        Ok(_) => stream_control::OutcomeRef::Ok,
                        Err(error) => stream_control::OutcomeRef::StrictError(error),
                    },
                },
            ) {
                return;
            }
            state.push(identity, bytes.len() + diagnostic_bytes, || {
                RecordKind::StateLoad {
                    bytes: bytes.to_vec(),
                    outcome: match result {
                        Ok(()) => CallOutcome::Ok,
                        Err(e) => CallOutcome::StrictError(e.clone()),
                    },
                }
            });
        });
    }
    #[cfg(all(
        any(target_os = "linux", windows),
        feature = "experimental-fresh-helper"
    ))]
    pub(crate) fn strict_base_read(
        &self,
        site: &'static str,
        id: u32,
        result: &Result<f64, BackendFailure>,
    ) {
        self.with(|state, identity| {
            let size = result.as_ref().err().map_or(0, |e| e.diagnostic().len());
            if size > MAX_TEXT {
                state.fail("observer base diagnostic cap exceeded");
                return;
            }
            if state.try_stream(
                identity,
                stream_live::View::Control(stream_control::View::BaseRead {
                    site,
                    id,
                    raw_bits: result
                        .as_ref()
                        .map(|v| v.to_bits())
                        .map_err(|e| e.diagnostic()),
                }),
            ) {
                return;
            }
            state.push(identity, size, || RecordKind::BaseRead {
                site,
                id,
                raw_bits: result
                    .as_ref()
                    .map(|v| v.to_bits())
                    .map_err(|e| e.diagnostic().to_owned()),
            });
        });
    }
    #[cfg(feature = "vst3")]
    pub(crate) fn state_load(&self, bytes: &[u8], result: &Result<(), String>) {
        self.with(|state, identity| {
            let diagnostic_bytes = result.as_ref().err().map_or(0, |s| s.len());
            if bytes.len() > MAX_STATE_BYTES || diagnostic_bytes > MAX_TEXT {
                state.fail("observer state-load input cap exceeded");
                return;
            }
            #[cfg(all(
                any(target_os = "linux", windows),
                feature = "experimental-fresh-helper"
            ))]
            if state.try_stream(
                identity,
                stream_live::View::StateLoad {
                    bytes,
                    outcome: match result {
                        Ok(_) => stream_control::OutcomeRef::Ok,
                        Err(error) => stream_control::OutcomeRef::Error(error),
                    },
                },
            ) {
                return;
            }
            state.push(identity, bytes.len() + diagnostic_bytes, || {
                RecordKind::StateLoad {
                    bytes: bytes.to_vec(),
                    outcome: match result {
                        Ok(()) => CallOutcome::Ok,
                        Err(e) => CallOutcome::Error(e.clone()),
                    },
                }
            });
        });
    }
    #[cfg(feature = "vst3")]
    pub(crate) fn base_read(&self, site: &'static str, id: u32, result: &Result<f64, String>) {
        self.with(|state, identity| {
            let size = result.as_ref().err().map_or(0, |s| s.len());
            if size > MAX_TEXT {
                state.fail("observer base diagnostic cap exceeded");
                return;
            }
            #[cfg(all(
                any(target_os = "linux", windows),
                feature = "experimental-fresh-helper"
            ))]
            if state.try_stream(
                identity,
                stream_live::View::Control(stream_control::View::BaseRead {
                    site,
                    id,
                    raw_bits: result.as_ref().map(|v| v.to_bits()).map_err(String::as_str),
                }),
            ) {
                return;
            }
            state.push(identity, size, || RecordKind::BaseRead {
                site,
                id,
                raw_bits: result.as_ref().map(|v| v.to_bits()).map_err(Clone::clone),
            });
        });
    }
    #[cfg(all(
        any(target_os = "linux", windows),
        feature = "experimental-fresh-helper"
    ))]
    pub(crate) fn audio_declaration(
        &self,
        inputs: &[crate::instrument_bounce::BounceAudioBus],
        outputs: &[crate::instrument_bounce::BounceAudioBus],
    ) {
        self.with(|state, identity| {
            if inputs.len() > MAX_BUSES || outputs.len() > MAX_BUSES {
                state.fail("observer audio declaration cap exceeded");
                return;
            }
            let bytes = (inputs.len() + outputs.len())
                * std::mem::size_of::<crate::instrument_bounce::BounceAudioBus>();
            if state.try_stream(
                identity,
                stream_live::View::Native(stream_codec::NativeView::AudioDeclaration {
                    inputs,
                    outputs,
                }),
            ) {
                return;
            }
            state.push(identity, bytes, || RecordKind::AudioDeclaration {
                input_buses: inputs.to_vec(),
                output_buses: outputs.to_vec(),
            });
        });
    }
    #[cfg(all(
        any(target_os = "linux", windows),
        feature = "experimental-fresh-helper"
    ))]
    pub(crate) fn event_declaration(
        &self,
        inputs: impl ExactSizeIterator<Item = (u32, bool, Option<bool>)>,
        outputs: impl ExactSizeIterator<Item = (u32, bool, Option<bool>)>,
    ) {
        self.with(|state, identity| {
            if inputs.len() > MAX_BUSES || outputs.len() > MAX_BUSES {
                state.fail("observer event declaration cap exceeded");
                return;
            }
            let bytes =
                (inputs.len() + outputs.len()) * std::mem::size_of::<(u32, bool, Option<bool>)>();
            if state.stream.is_some() || state.stream_taken {
                let mut in_buses = [(0, false, None); MAX_BUSES];
                let mut out_buses = [(0, false, None); MAX_BUSES];
                let mut in_count = 0;
                let mut out_count = 0;
                for value in inputs {
                    if in_count == MAX_BUSES {
                        state.fail("observer event declaration cap exceeded");
                        return;
                    }
                    in_buses[in_count] = value;
                    in_count += 1;
                }
                for value in outputs {
                    if out_count == MAX_BUSES {
                        state.fail("observer event declaration cap exceeded");
                        return;
                    }
                    out_buses[out_count] = value;
                    out_count += 1;
                }
                if state.try_stream(
                    identity,
                    stream_live::View::Native(stream_codec::NativeView::EventDeclaration {
                        inputs: &in_buses[..in_count],
                        outputs: &out_buses[..out_count],
                    }),
                ) {
                    return;
                }
                state.push(identity, bytes, || RecordKind::EventDeclaration {
                    inputs: in_buses[..in_count].to_vec(),
                    outputs: out_buses[..out_count].to_vec(),
                });
                return;
            }
            state.push(identity, bytes, || RecordKind::EventDeclaration {
                inputs: inputs.collect(),
                outputs: outputs.collect(),
            });
        });
    }
    #[cfg(feature = "vst3")]
    pub(crate) fn metadata(&self, site: &'static str, result: Result<&BouncePluginMetadata, &str>) {
        self.with(|state, identity| match result {
            Ok(value) => match metadata_size(value) {
                Some(size) => {
                    #[cfg(all(
                        any(target_os = "linux", windows),
                        feature = "experimental-fresh-helper"
                    ))]
                    if state.try_stream(
                        identity,
                        stream_live::View::Native(stream_codec::NativeView::Metadata {
                            site,
                            result: Ok(value),
                        }),
                    ) {
                        return;
                    }
                    state.push(identity, size, || RecordKind::Metadata {
                        site,
                        result: Ok(value.clone()),
                    });
                }
                None => state.fail("observer metadata cap exceeded"),
            },
            Err(error) => {
                if error.len() > MAX_TEXT {
                    state.fail("observer metadata diagnostic cap exceeded");
                } else {
                    #[cfg(all(
                        any(target_os = "linux", windows),
                        feature = "experimental-fresh-helper"
                    ))]
                    if state.try_stream(
                        identity,
                        stream_live::View::Native(stream_codec::NativeView::Metadata {
                            site,
                            result: Err(error),
                        }),
                    ) {
                        return;
                    }
                    state.push(identity, error.len(), || RecordKind::Metadata {
                        site,
                        result: Err(error.to_owned()),
                    });
                }
            }
        });
    }
    #[cfg(feature = "vst3")]
    pub(crate) fn reset_report(&self, report: &vst3_host::ResetOriginReport) {
        self.with(|state, identity| {
            let size = report.process_error.as_ref().map_or(0, String::len);
            if size > MAX_TEXT {
                state.fail("observer reset diagnostic cap exceeded");
                return;
            }
            #[cfg(all(
                any(target_os = "linux", windows),
                feature = "experimental-fresh-helper"
            ))]
            if state.try_stream(
                identity,
                stream_live::View::Control(stream_control::View::ResetReport {
                    frames: report.frames,
                    discarded_prior_midi_events: report.discarded_prior_midi_events,
                    discarded_midi_events: report.discarded_midi_events,
                    discarded_prior_parameter_points: report.discarded_prior_parameter_points,
                    discarded_parameter_points: report.discarded_parameter_points,
                    output_events_lost: report.output_events_lost,
                    parameter_output_fault: report.parameter_output_fault,
                    process_error: report.process_error.as_deref(),
                }),
            ) {
                return;
            }
            state.push(identity, size, || RecordKind::ResetReport {
                frames: report.frames,
                discarded_prior_midi_events: report.discarded_prior_midi_events,
                discarded_midi_events: report.discarded_midi_events,
                discarded_prior_parameter_points: report.discarded_prior_parameter_points,
                discarded_parameter_points: report.discarded_parameter_points,
                output_events_lost: report.output_events_lost,
                parameter_output_fault: report.parameter_output_fault,
                process_error: report.process_error.clone(),
            });
        });
    }
    #[cfg(all(
        any(target_os = "linux", windows),
        feature = "experimental-fresh-helper"
    ))]
    pub(crate) fn pcm(&self, frames: usize, transport: PluginTransport, reply: &BlockReply) {
        self.with(|state, identity| state.strict_pcm(identity, frames, transport, reply));
    }
    #[cfg(all(
        any(target_os = "linux", windows),
        feature = "experimental-fresh-helper"
    ))]
    pub(crate) fn midi(&self, data: [u8; 3], offset: u16) {
        self.with(|state, identity| {
            if state.try_stream(
                identity,
                stream_live::View::Native(stream_codec::NativeView::Midi { data, offset }),
            ) {
                return;
            }
            state.push(identity, 0, || RecordKind::Midi { data, offset });
        });
    }
    #[cfg(all(
        any(target_os = "linux", windows),
        feature = "experimental-fresh-helper"
    ))]
    pub(crate) fn disposal(&self, value: &DisposalObservation) {
        self.with(|state, identity| {
            if value.diagnostics.len() > 8
                || value.diagnostics.iter().any(|d| d.len() > MAX_TEXT)
                || value
                    .error
                    .as_ref()
                    .is_some_and(|e| e.diagnostic().len() > MAX_TEXT)
            {
                state.fail("observer disposal cap exceeded");
                return;
            }
            let size = value.diagnostics.iter().map(String::len).sum::<usize>()
                + value.error.as_ref().map_or(0, |e| e.diagnostic().len());
            if state.try_stream(
                identity,
                stream_live::View::Control(stream_control::View::Disposal {
                    terminated: value.terminated,
                    error: value.error.as_ref(),
                    diagnostics: &value.diagnostics,
                }),
            ) {
                return;
            }
            state.push(identity, size, || RecordKind::Disposal {
                terminated: value.terminated,
                error: value.error.clone(),
                diagnostics: value.diagnostics.clone(),
            });
        });
    }
    #[cfg(feature = "vst3")]
    pub(crate) fn lifecycle(&self, name: &'static str) {
        self.with(|state, identity| {
            #[cfg(all(
                any(target_os = "linux", windows),
                feature = "experimental-fresh-helper"
            ))]
            if state.try_stream(
                identity,
                stream_live::View::Control(stream_control::View::Lifecycle(name)),
            ) {
                return;
            }
            state.push(identity, 0, || RecordKind::Lifecycle(name));
        });
    }
}
#[cfg(feature = "vst3")]
fn outcome_size(outcome: &CallOutcome) -> usize {
    match outcome {
        CallOutcome::Ok => 0,
        CallOutcome::Error(s) => s.len(),
        CallOutcome::StrictError(e) => e.diagnostic().len(),
    }
}
fn metadata_size(value: &BouncePluginMetadata) -> Option<usize> {
    let texts = [
        &value.class_uid,
        &value.version,
        &value.name,
        &value.category,
    ];
    if texts.iter().any(|s| s.len() > MAX_TEXT)
        || [
            value.input_buses.len(),
            value.output_buses.len(),
            value.event_input_buses.len(),
            value.event_output_buses.len(),
        ]
        .iter()
        .any(|n| *n > MAX_BUSES)
    {
        return None;
    }
    Some(
        texts.iter().map(|s| s.len()).sum::<usize>()
            + (value.input_buses.len() + value.output_buses.len())
                * std::mem::size_of::<crate::instrument_bounce::BounceAudioBus>()
            + (value.event_input_buses.len() + value.event_output_buses.len())
                * std::mem::size_of::<(u32, bool, Option<bool>)>(),
    )
}
pub(crate) fn record_capture(label: CaptureLabel, capture: &CapturedInstrumentBounce) {
    with_active(|state| {
        let Some(metadata_bytes) = metadata_size(&capture.state.metadata) else {
            state.fail("observer capture metadata cap exceeded");
            return;
        };
        if capture.state.opaque_state.len() > MAX_STATE_BYTES
            || capture.state.parameter_bases.len() > MAX_BASES
        {
            state.fail("observer capture state cap exceeded");
            return;
        }
        let bytes = metadata_bytes
            + capture.state.opaque_state.len()
            + capture.state.parameter_bases.len() * 32;
        #[cfg(all(
            any(target_os = "linux", windows),
            feature = "experimental-fresh-helper"
        ))]
        if state.try_stream(
            None,
            stream_live::View::Capture {
                label,
                identity: capture.identity(),
                receipt_identity: capture.genuine_receipt_identity(),
                state: &capture.state,
            },
        ) {
            return;
        }
        state.push(None, bytes, || RecordKind::Capture {
            label,
            identity: capture.identity(),
            receipt_identity: capture.genuine_receipt_identity(),
            state: capture.state.clone(),
        });
    });
}
/// App-owned receipt consumption only, never called by the callback.
pub(crate) fn record_app_asset_event(event: AudioAssetEvent) {
    with_active(|state| {
        #[cfg(all(
            any(target_os = "linux", windows),
            feature = "experimental-fresh-helper"
        ))]
        if state.try_stream(
            None,
            stream_live::View::App(stream_app_control::AppView::AudioAsset(&event)),
        ) {
            return;
        }
        state.push(None, 0, || RecordKind::AppAudioAssetEvent(event));
    });
}
pub(crate) fn record_app_generator_event(event: GeneratorEndpointEvent) {
    with_active(|state| {
        #[cfg(all(
            any(target_os = "linux", windows),
            feature = "experimental-fresh-helper"
        ))]
        if state.try_stream(
            None,
            stream_live::View::App(stream_app_control::AppView::GeneratorEndpoint(&event)),
        ) {
            return;
        }
        state.push(None, 0, || RecordKind::AppGeneratorEndpointEvent(event));
    });
}
pub(crate) fn record_app_timeline_event(
    event: crate::timeline_runtime::TimelineRuntimeEvent,
    before: TimelineAppState,
    after: TimelineAppState,
) {
    with_active(|state| state.timeline_event(event, before, after));
}
// Unit-test isolation affects only collection. The genuine guard's hook inventory
// must find this absent. No production code, backend decision or retry reads it.
thread_local! {
    static CAPTURE_GUARD_TEST_ACTIVE: Cell<bool> = const { Cell::new(false) };
    static CAPTURE_GUARD_TEST_STATE: RefCell<Option<ObservationSnapshot>> = const { RefCell::new(None) };
}
pub(crate) fn capture_guard_test_observer_inactive() -> bool {
    !CAPTURE_GUARD_TEST_ACTIVE.with(Cell::get)
}

pub(crate) fn with_capture_guard_test_observer<T>(
    capacity: Option<usize>,
    operation: impl FnOnce() -> T,
) -> (T, Option<ObservationSnapshot>) {
    struct Restore {
        active: bool,
        state: Option<ObservationSnapshot>,
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            CAPTURE_GUARD_TEST_ACTIVE.with(|active| active.set(self.active));
            CAPTURE_GUARD_TEST_STATE.with(|state| {
                *state.borrow_mut() = self.state.take();
            });
        }
    }
    let state = capacity.map(|cap| {
        assert!(cap <= MAX_RECORDS);
        ObservationSnapshot::new(cap, 0).expect("bounded capture observer")
    });
    let restore = Restore {
        active: CAPTURE_GUARD_TEST_ACTIVE.with(|active| active.replace(true)),
        state: CAPTURE_GUARD_TEST_STATE.with(|slot| slot.replace(state)),
    };
    let result = operation();
    let observation = CAPTURE_GUARD_TEST_STATE.with(|state| state.borrow_mut().take());
    drop(restore);
    (result, observation)
}

pub(crate) fn record_worker_capture_guard(operands: WorkerCaptureGuard) {
    let worker_id = WORKER.with(Cell::get).map(|worker| worker.id);
    if !capture_guard_test_observer_inactive() {
        CAPTURE_GUARD_TEST_STATE.with(|state| {
            if let Some(state) = state.borrow_mut().as_mut() {
                record_capture_guard_into(state, worker_id, operands);
            }
        });
        return;
    }
    with_active(|state| record_capture_guard_into(state, worker_id, operands));
}
fn record_capture_guard_into(
    state: &mut ObservationSnapshot,
    worker_id: Option<usize>,
    operands: WorkerCaptureGuard,
) {
    #[cfg(all(
        any(target_os = "linux", windows),
        feature = "experimental-fresh-helper"
    ))]
    if state.try_stream(
        None,
        stream_live::View::Control(stream_control::View::WorkerCaptureGuard {
            worker_id,
            operands: &operands,
        }),
    ) {
        return;
    }
    state.push(None, 0, || RecordKind::WorkerCaptureGuard {
        worker_id,
        operands,
    });
}

pub(crate) fn record_worker_join(worker_id: usize, outcome: WorkerJoinOutcome) {
    with_active(|state| {
        #[cfg(all(
            any(target_os = "linux", windows),
            feature = "experimental-fresh-helper"
        ))]
        if state.try_stream(
            None,
            stream_live::View::Control(stream_control::View::WorkerJoin { worker_id, outcome }),
        ) {
            return;
        }
        state.push(None, 0, || RecordKind::WorkerJoin { worker_id, outcome });
    });
}
pub(crate) fn record_render_report(report: &RenderReport) {
    with_active(|state| {
        #[cfg(all(
            any(target_os = "linux", windows),
            feature = "experimental-fresh-helper"
        ))]
        if state.try_stream(
            None,
            stream_live::View::Control(stream_control::View::RenderReport(report)),
        ) {
            return;
        }
        state.push(None, 0, || RecordKind::RenderReport(report.clone()));
    });
}
pub(crate) fn record_hooks(site: &'static str, inactive: bool) {
    with_active(|state| {
        if !inactive {
            state.fail("a behavior-changing test hook was active");
        }
        #[cfg(all(
            any(target_os = "linux", windows),
            feature = "experimental-fresh-helper"
        ))]
        if state.try_stream(
            None,
            stream_live::View::Control(stream_control::View::HookInventory { site, inactive }),
        ) {
            return;
        }
        state.push(None, 0, || RecordKind::HookInventory { site, inactive });
    });
}
pub(crate) fn record_worker(
    site: &'static str,
    epoch: u64,
    sequence: Option<u64>,
    stats: BridgeStats,
) {
    let Some(worker) = WORKER.with(Cell::get) else {
        return;
    };
    with_active(|state| {
        #[cfg(all(
            any(target_os = "linux", windows),
            feature = "experimental-fresh-helper"
        ))]
        if state.try_stream(
            None,
            stream_live::View::Control(stream_control::View::Worker {
                site,
                worker_id: worker.id,
                manifest: &worker.manifest,
                epoch,
                sequence,
                stats: &stats,
            }),
        ) {
            return;
        }
        state.push(None, 0, || RecordKind::Worker {
            site,
            worker_id: worker.id,
            manifest: worker.manifest,
            epoch,
            sequence,
            stats,
        })
    });
}
pub(crate) fn record_lifecycle(site: &'static str) {
    with_active(|state| {
        #[cfg(all(
            any(target_os = "linux", windows),
            feature = "experimental-fresh-helper"
        ))]
        if state.try_stream(
            None,
            stream_live::View::Control(stream_control::View::Lifecycle(site)),
        ) {
            return;
        }
        state.push(None, 0, || RecordKind::Lifecycle(site));
    });
}

/// App consumption tap: preserve tagged replies without draining or replacing
/// normal App handling. Ordinary state publication records length only.
pub(crate) fn record_runtime_event(
    endpoint_id: u64,
    project_session: u64,
    instance_id: u64,
    event: &RuntimeEvent,
) {
    with_active(|state| {
        let bytes = match event {
            RuntimeEvent::SlotReady { name, .. } => {
                if name.len() > MAX_TEXT {
                    state.fail("observer runtime name cap exceeded");
                    return;
                }
                name.len()
            }
            RuntimeEvent::SlotFault { message, .. }
            | RuntimeEvent::ParameterCommandFailed { message, .. } => {
                if message.len() > MAX_TEXT {
                    state.fail("observer runtime error cap exceeded");
                    return;
                }
                message.len()
            }
            RuntimeEvent::ParameterCatalogPage { items, .. } => {
                if items.len() > crate::plugins::plugin_runtime::MAX_PLUGIN_PARAMETER_PAGE_ITEMS
                    || items
                        .iter()
                        .any(|p| p.name.len() > MAX_TEXT || p.unit.len() > MAX_TEXT)
                {
                    state.fail("observer catalog page cap exceeded");
                    return;
                }
                items
                    .iter()
                    .map(|p| {
                        p.name.len()
                            + p.unit.len()
                            + std::mem::size_of::<
                                crate::plugins::plugin_runtime::PluginParameterDescriptor,
                            >()
                    })
                    .sum()
            }
            RuntimeEvent::State {
                slot,
                request_id,
                epoch,
                bytes,
            } => {
                #[cfg(all(
                    any(target_os = "linux", windows),
                    feature = "experimental-fresh-helper"
                ))]
                if state.try_stream(
                    None,
                    stream_live::View::Control(stream_control::View::OrdinaryState {
                        endpoint_id,
                        project_session,
                        instance_id,
                        slot: *slot,
                        request_id: *request_id,
                        epoch: *epoch,
                        byte_len: bytes.len(),
                    }),
                ) {
                    return;
                }
                state.push(None, 0, || RecordKind::OrdinaryState {
                    endpoint_id,
                    project_session,
                    instance_id,
                    slot: *slot,
                    request_id: *request_id,
                    epoch: *epoch,
                    byte_len: bytes.len(),
                });
                return;
            }
            _ => 0,
        };
        #[cfg(all(
            any(target_os = "linux", windows),
            feature = "experimental-fresh-helper"
        ))]
        if state.try_stream(
            None,
            stream_live::View::App(stream_app_control::AppView::Runtime {
                endpoint_id,
                project_session,
                instance_id,
                event,
            }),
        ) {
            return;
        }
        state.push(None, bytes, || RecordKind::RuntimeEvent {
            endpoint_id,
            project_session,
            instance_id,
            event: event.clone(),
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retired_run_releases_payload_with_held_tap_and_never_rebuilds_after_drop_or_finish() {
        use std::{
            panic::{AssertUnwindSafe, catch_unwind},
            sync::mpsc,
            thread,
            time::Duration,
        };

        for poisoned in [false, true] {
            for finish in [false, true] {
                // A private run avoids activating the process-wide collector;
                // the held Arc models a tap that already passed active().
                let mut snapshot = ObservationSnapshot::new(1, 0).unwrap();
                snapshot.push(None, 0, || {
                    RecordKind::Lifecycle("retained before retirement")
                });
                let record = Arc::downgrade(&snapshot.records[0]);
                let run = ObservationRun {
                    state: Arc::new(Mutex::new(snapshot)),
                };
                if poisoned {
                    assert!(
                        catch_unwind(AssertUnwindSafe(|| {
                            run.with_snapshot(|_| panic!("intentional held-tap poison"));
                        }))
                        .is_err()
                    );
                }
                let held = Arc::clone(&run.state);
                let (ready_tx, ready_rx) = mpsc::sync_channel(1);
                let (resume_tx, resume_rx) = mpsc::sync_channel(1);
                thread::scope(|scope| {
                    let tap = scope.spawn(move || {
                        ready_tx.send(()).unwrap();
                        resume_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                        with_run(&held, |_| panic!("retired tap must stay lazy"));
                        let state = held.lock().unwrap_or_else(|p| p.into_inner());
                        assert!(state.retired);
                        assert!(state.records.is_empty());
                        assert!(state.worker_identities.iter().all(Option::is_none));
                        assert_eq!(state.failure, None);
                    });
                    ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    if finish {
                        let snapshot = run.finish();
                        assert_eq!(snapshot.records.len(), 1);
                        assert!(record.upgrade().is_some());
                        drop(snapshot);
                    } else {
                        drop(run);
                    }
                    assert!(
                        record.upgrade().is_none(),
                        "a held tap cannot retain the retired run's payload"
                    );
                    resume_tx.send(()).unwrap();
                    tap.join().unwrap();
                });
            }
        }
    }
    fn timeline_state(phase: TimelinePhase) -> TimelineAppState {
        TimelineAppState {
            phase,
            project_session: 7,
            engine_session: Some(11),
            sync_session: 3,
            desired_generation: 5,
            observed_fingerprint: 101,
            compiled_generation: Some(5),
            compiled_fingerprint: Some(101),
            source_generation: Some(5),
            source_fingerprint: Some(101),
            sync_source_generation: Some(5),
            desired_seek_serial: 13,
            desired_seek_frame: 0,
            loop_token: Some(17),
            loop_disable_request: None,
            retry_count: 0,
            stale_confirmations: 0,
            confirmed_identity: Some((4, 19)),
            retry_required: false,
            retry_pending: None,
        }
    }
    #[test]
    fn timeline_event_cap_is_lifetime_bounded_and_charged_to_total_records() {
        use crate::timeline_runtime::TimelineRuntimeEvent;
        let before = timeline_state(TimelinePhase::InstallQueued {
            request_id: 23,
            generation: 5,
        });
        let after = timeline_state(TimelinePhase::LoopReady);
        let event = TimelineRuntimeEvent::Installed {
            request_id: 23,
            revision: 5,
        };
        let mut state = ObservationSnapshot::new(MAX_RECORDS, 0).unwrap();
        for _ in 0..MAX_TIMELINE_EVENTS {
            state.timeline_event(event, before, after);
        }
        assert_eq!(state.records.len(), MAX_TIMELINE_EVENTS);
        assert_eq!(state.timeline_events, MAX_TIMELINE_EVENTS);
        assert!(state.failure.is_none());
        let run = ObservationRun {
            state: Arc::new(Mutex::new(state)),
        };
        assert_eq!(run.copy_records_since(0).len(), MAX_RECORD_BATCH);
        with_run(&run.state, |state| {
            state.timeline_event(event, before, after)
        });
        let state = run.finish();
        assert_eq!(state.records.len(), MAX_TIMELINE_EVENTS);
        assert_eq!(state.timeline_events, MAX_TIMELINE_EVENTS);
        assert_eq!(state.failure, Some("observer Timeline event cap exceeded"));
        match state.records[0].kind {
            RecordKind::AppTimelineRuntimeEvent {
                event: actual,
                before: actual_before,
                after: actual_after,
            } => {
                assert_eq!(actual, event);
                assert_eq!(actual_before, before);
                assert_eq!(actual_after, after);
            }
            _ => panic!("expected exact Timeline consumption record"),
        }
        let mut total_limited = ObservationSnapshot::new(1, 0).unwrap();
        total_limited.push(None, 0, || RecordKind::Lifecycle("prior record"));
        total_limited.timeline_event(event, before, after);
        assert_eq!(total_limited.records.len(), 1);
        assert_eq!(total_limited.failure, Some("observer record cap exceeded"));
    }
    #[test]
    fn inactive_observer_never_enters_record_builder() {
        InstanceObserver::default().with(|_, _| panic!("inactive observer must not record"));
    }
    #[test]
    fn installed_records_share_a_monotonic_run_clock_and_immutable_timestamps() {
        let mut state = ObservationSnapshot::new(2, 0).unwrap();
        let before = state.elapsed_ns();
        state.push(None, 0, || RecordKind::Lifecycle("first"));
        state.push(None, 0, || RecordKind::Lifecycle("second"));
        let after = state.elapsed_ns();
        let first = &state.records[0];
        let second = &state.records[1];
        assert!(before <= first.monotonic_elapsed_ns);
        assert!(first.monotonic_elapsed_ns <= second.monotonic_elapsed_ns);
        assert!(second.monotonic_elapsed_ns <= after);
        let retained = Arc::clone(first);
        let first_timestamp = first.monotonic_elapsed_ns;
        let run = ObservationRun {
            state: Arc::new(Mutex::new(state)),
        };
        assert!(run.monotonic_elapsed_ns() >= after);
        let finished = run.finish();
        assert!(Arc::ptr_eq(&retained, &finished.records[0]));
        assert_eq!(retained.monotonic_elapsed_ns, first_timestamp);
    }
    #[test]
    fn record_batches_copy_only_bounded_immutable_handles() {
        let mut state = ObservationSnapshot::new(MAX_RECORD_BATCH + 1, 3).unwrap();
        state.push(None, 3, || RecordKind::StateLoad {
            bytes: vec![1, 2, 3],
            outcome: CallOutcome::Ok,
        });
        for _ in 0..MAX_RECORD_BATCH {
            state.push(None, 0, || RecordKind::Lifecycle("bounded"));
        }
        let run = ObservationRun {
            state: Arc::new(Mutex::new(state)),
        };
        let first = run.copy_records_since(0);
        assert_eq!(first.len(), MAX_RECORD_BATCH);
        run.with_snapshot(|state| assert!(Arc::ptr_eq(&first[0], &state.records[0])));
        assert_eq!(run.copy_records_since(MAX_RECORD_BATCH).len(), 1);
        assert!(run.copy_records_since(MAX_RECORD_BATCH + 1).is_empty());
        assert!(run.copy_records_since(MAX_RECORD_BATCH + 2).is_empty());
        let finished = run.finish();
        assert_eq!(finished.records.len(), MAX_RECORD_BATCH + 1);
        assert_eq!(finished.private_bytes, 3);
        assert!(Arc::ptr_eq(&first[0], &finished.records[0]));
        assert_eq!(
            finished.failure,
            Some("observer copy cursor exceeded record count")
        );
    }
    #[test]
    fn record_cap_is_checked_before_payload_copy_and_failure_is_sticky() {
        let mut state = ObservationSnapshot::new(1, 10).unwrap();
        state.push(None, 0, || RecordKind::Lifecycle("first"));
        state.push(None, 0, || panic!("overflow must not build payload"));
        state.fail("later failure");
        assert_eq!(state.failure, Some("observer record cap exceeded"));
        assert_eq!(state.records.len(), 1);
    }
    #[test]
    fn byte_cap_is_checked_before_copy_including_integer_overflow() {
        let mut state = ObservationSnapshot::new(3, 1).unwrap();
        state.push(None, 1, || RecordKind::Lifecycle("first"));
        state.push(None, usize::MAX, || panic!("overflow must not copy"));
        assert_eq!(state.private_bytes, 1);
        assert_eq!(state.records.len(), 1);
        assert_eq!(state.failure, Some("observer byte cap exceeded"));
    }
    #[test]
    fn source_clone_undo_order_and_duplicate_owner_are_checked() {
        let mut state = ObservationSnapshot::new(0, 0).unwrap();
        for (phase, stage, ordinal) in [
            (InstancePhase::InitialSource, Stage::InitialSource, 1),
            (InstancePhase::StrictClone, Stage::Conversion, 2),
            (InstancePhase::UndoSource, Stage::Undo, 3),
        ] {
            state.stage(stage).unwrap();
            assert_eq!(state.bind_instance(phase, None).ordinal, ordinal);
        }
        assert_eq!(state.failure, None);
        state.bind_instance(InstancePhase::UndoSource, None);
        assert_eq!(
            state.failure,
            Some("observer unexpected instance phase, order or duplicate owner")
        );
    }
    #[test]
    fn wrong_phase_or_missing_prior_owner_cannot_be_accepted() {
        let mut wrong_phase = ObservationSnapshot::new(0, 0).unwrap();
        wrong_phase.bind_instance(InstancePhase::InitialSource, None);
        assert!(wrong_phase.failure.is_some());
        let mut missing_source = ObservationSnapshot::new(0, 0).unwrap();
        missing_source.stage(Stage::Conversion).unwrap();
        missing_source.bind_instance(InstancePhase::StrictClone, None);
        assert!(missing_source.failure.is_some());
    }
    #[test]
    fn whole_source_clone_cleanup_keeps_exact_owner_and_record_order() {
        let mut state = ObservationSnapshot::new(3, 0).unwrap();
        state.stage(Stage::InitialSource).unwrap();
        let source = state.bind_instance(InstancePhase::InitialSource, None);
        assert_eq!(source.ordinal, 1);
        state.push(Some(source), 0, || RecordKind::Lifecycle("source observed"));
        state.stage(Stage::WholeExport).unwrap();
        let clone = state.bind_instance(InstancePhase::StrictClone, None);
        assert_eq!(clone.ordinal, 2);
        assert_eq!(clone.phase, InstancePhase::StrictClone);
        state.push(Some(clone), 0, || RecordKind::Lifecycle("clone observed"));
        state.stage(Stage::Cleanup).unwrap();
        state.push(None, 0, || RecordKind::Lifecycle("cleanup observed"));
        assert_eq!(state.failure, None);
        assert_eq!(state.phases, [true, true, false]);
        assert_eq!(state.next_instance, 3);
        assert_eq!(
            state.records.iter().map(|r| r.stage).collect::<Vec<_>>(),
            vec![Stage::InitialSource, Stage::WholeExport, Stage::Cleanup]
        );
        assert_eq!(state.records[0].instance, Some(source));
        assert_eq!(state.records[1].instance, Some(clone));
    }

    #[test]
    fn whole_clone_rejects_source_stop_missing_source_and_duplicate_owner_sticky() {
        let rejected = Some("observer unexpected instance phase, order or duplicate owner");
        let mut wrong_stage = ObservationSnapshot::new(0, 0).unwrap();
        wrong_stage.stage(Stage::InitialSource).unwrap();
        wrong_stage.bind_instance(InstancePhase::InitialSource, None);
        wrong_stage.stage(Stage::SourceStop).unwrap();
        wrong_stage.bind_instance(InstancePhase::StrictClone, None);
        assert_eq!(wrong_stage.failure, rejected);
        wrong_stage.stage(Stage::Cleanup).unwrap();
        wrong_stage.fail("later failure");
        assert_eq!(wrong_stage.failure, rejected);

        let mut missing_source = ObservationSnapshot::new(0, 0).unwrap();
        missing_source.stage(Stage::WholeExport).unwrap();
        missing_source.bind_instance(InstancePhase::StrictClone, None);
        assert_eq!(missing_source.failure, rejected);
        missing_source.stage(Stage::Cleanup).unwrap();
        missing_source.fail("later failure");
        assert_eq!(missing_source.failure, rejected);

        let mut duplicate = ObservationSnapshot::new(0, 0).unwrap();
        duplicate.stage(Stage::InitialSource).unwrap();
        duplicate.bind_instance(InstancePhase::InitialSource, None);
        duplicate.stage(Stage::WholeExport).unwrap();
        assert_eq!(
            duplicate
                .bind_instance(InstancePhase::StrictClone, None)
                .ordinal,
            2
        );
        assert_eq!(duplicate.failure, None);
        duplicate.bind_instance(InstancePhase::StrictClone, None);
        assert_eq!(duplicate.failure, rejected);
        duplicate.stage(Stage::Cleanup).unwrap();
        duplicate.fail("later failure");
        assert_eq!(duplicate.failure, rejected);
    }

    #[test]
    fn whole_cannot_regress_into_conversion_or_undo_and_failure_is_sticky() {
        for earlier in [Stage::Conversion, Stage::Undo] {
            let mut state = ObservationSnapshot::new(0, 0).unwrap();
            state.stage(Stage::InitialSource).unwrap();
            state.bind_instance(InstancePhase::InitialSource, None);
            state.stage(Stage::WholeExport).unwrap();
            state.bind_instance(InstancePhase::StrictClone, None);
            assert_eq!(state.failure, None);
            assert_eq!(state.stage(earlier), Err("observer phase regression"));
            assert_eq!(state.stage, Stage::WholeExport);
            assert_eq!(state.failure, Some("observer phase regression"));
            state.stage(Stage::Cleanup).unwrap();
            state.fail("later failure");
            assert_eq!(state.failure, Some("observer phase regression"));
        }
    }

    #[test]
    fn stage_regression_is_sticky_and_records_keep_actual_order() {
        let mut state = ObservationSnapshot::new(4, 0).unwrap();
        state.stage(Stage::Conversion).unwrap();
        state.push(None, 0, || RecordKind::Lifecycle("dispose returned"));
        state.push(None, 0, || {
            RecordKind::Lifecycle("backend destructor returned")
        });
        state.push(None, 0, || RecordKind::Lifecycle("publication attempt"));
        assert!(state.stage(Stage::SourceStop).is_err());
        assert_eq!(
            state.records.iter().map(|r| r.ordinal).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert!(state.records.iter().all(|r| r.stage == Stage::Conversion));
    }

    #[test]
    #[cfg(all(
        any(target_os = "linux", windows),
        feature = "experimental-fresh-helper"
    ))]
    fn default_and_explicit_qa_pcm_limits_preserve_samples_and_fail_sticky_at_boundary() {
        for limit in [MAX_CLONE_REPLIES, 2048] {
            let mut state = ObservationSnapshot::new(MAX_RECORDS, MAX_PRIVATE_BYTES).unwrap();
            if limit != MAX_CLONE_REPLIES {
                state.set_qa_clone_reply_limit(limit).unwrap();
            }
            let reply = BlockReply {
                channels: vec![vec![-0.375], vec![0.625]],
                transport_applied: true,
                output_events: 0,
                output_events_lost: false,
            };
            let transport = PluginTransport::default();
            for _ in 0..limit {
                state.strict_pcm(None, 1, transport, &reply);
            }
            assert!(state.failure.is_none());
            assert_eq!(state.clone_replies, limit);
            assert_eq!(state.records.len(), limit);
            assert_eq!(state.private_bytes, limit * 8);
            match &state.records[limit - 1].kind {
                RecordKind::StrictPcm {
                    channels,
                    transport_applied,
                    ..
                } => {
                    assert_eq!(channels, &reply.channels);
                    assert!(*transport_applied);
                }
                _ => panic!("last copied record must retain the real reply shape and samples"),
            }
            state.strict_pcm(None, 1, transport, &reply);
            state.fail("later failure");
            assert_eq!(state.failure, Some("observer strict PCM cap exceeded"));
            assert_eq!(state.clone_replies, limit);
            assert_eq!(state.records.len(), limit);
            assert_eq!(state.private_bytes, limit * 8);
        }
    }

    #[test]
    #[cfg(all(
        any(target_os = "linux", windows),
        feature = "experimental-fresh-helper"
    ))]
    fn qa_pcm_limit_is_explicit_bounded_and_does_not_change_other_observer_caps() {
        for invalid in [0, 4097, usize::MAX] {
            assert!(ObservationRun::install_with_clone_reply_limit(invalid).is_err());
        }
        let run = ObservationRun::install_with_clone_reply_limit(2048).unwrap();
        run.with_snapshot(|state| {
            assert_eq!(state.clone_reply_cap, 2048);
            assert_eq!(state.record_cap, MAX_RECORDS);
            assert_eq!(state.byte_cap, MAX_PRIVATE_BYTES);
        });
        assert!(
            ObservationRun::install_with_clone_reply_limit(4096).is_err(),
            "an active run must retain sole observation ownership"
        );
        assert_eq!(run.finish().clone_reply_cap, 2048);
        let ordinary = ObservationRun::install().unwrap();
        assert_eq!(
            ordinary.finish().clone_reply_cap,
            MAX_CLONE_REPLIES,
            "QA storage settings cannot leak into a later ordinary run"
        );
    }
}
