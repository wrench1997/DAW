//! Inactive-by-default, run-owned observations for the manual genuine App test.
//!
//! Worker/control threads only. No callback may call this collector. Every tap
//! borrows an existing operand/result; it never calls a backend or changes one.
//! Private state and PCM stay in this run-owned value; Debug deliberately omits
//! their bytes. The driver owns hashing/persistence outside all callback paths.

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
use crate::instrument_bounce::render::{BlockReply, DisposalObservation};
use crate::{
    audio::{AudioAssetEvent, GeneratorEndpointEvent},
    instrument_bounce::{
        BouncePluginMetadata, BouncePluginState, BounceSourceIdentity, CapturedInstrumentBounce,
        render::{BackendFailure, RenderReport},
    },
    plugins::plugin_runtime::{BridgeStats, PluginEndpointManifest, PluginTransport, RuntimeEvent},
};
use std::{
    cell::Cell,
    sync::{Arc, Mutex, Weak},
};

pub(crate) const MAX_RECORDS: usize = 32_768;
pub(crate) const MAX_RECORD_BATCH: usize = 64;
// Cumulative copied payload accounting; this is not an allocator/RSS limit.
pub(crate) const MAX_PRIVATE_BYTES: usize = 256 * 1024 * 1024;
pub(crate) const MAX_STATE_BYTES: usize = 64 * 1024 * 1024;
#[cfg(feature = "vst3")]
pub(crate) const MAX_SOURCE_POSITIVE_ATTEMPTS: usize = 4096;
#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
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
    stage: Stage,
    next_instance: u64,
    phases: [bool; 3],
    record_cap: usize,
    byte_cap: usize,
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
            stage: Stage::Preflight,
            next_instance: 1,
            phases: [false; 3],
            record_cap,
            byte_cap,
        })
    }
    fn fail(&mut self, failure: &'static str) {
        self.failure.get_or_insert(failure);
    }
    // Cap BEFORE invoking build, so even an overflowing copy closure is not run.
    fn push(
        &mut self,
        instance: Option<InstanceIdentity>,
        bytes: usize,
        build: impl FnOnce() -> RecordKind,
    ) {
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
        self.records.push(Arc::new(Record {
            ordinal: self.records.len() as u64 + 1,
            stage: self.stage,
            instance,
            kind: build(),
        }));
    }
    fn bind_instance(
        &mut self,
        phase: InstancePhase,
        worker: Option<WorkerIdentity>,
    ) -> InstanceIdentity {
        let (index, expected) = match phase {
            InstancePhase::InitialSource => (0, Stage::InitialSource),
            InstancePhase::StrictClone => (1, Stage::Conversion),
            InstancePhase::UndoSource => (2, Stage::Undo),
        };
        if self.stage != expected || self.phases[index] || self.next_instance != index as u64 + 1 {
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
        Ok(mut state) => f(&mut state),
        Err(poison) => {
            let mut state = poison.into_inner();
            state.fail("observer mutex poisoned");
            f(&mut state);
        }
    }
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
        let mut slot = ACTIVE
            .lock()
            .map_err(|_| "observer activation mutex poisoned")?;
        if slot.as_ref().and_then(Weak::upgrade).is_some() {
            return Err("an observation run is already active");
        }
        let state = Arc::new(Mutex::new(ObservationSnapshot::new(
            MAX_RECORDS,
            MAX_PRIVATE_BYTES,
        )?));
        *slot = Some(Arc::downgrade(&state));
        Ok(Self { state })
    }
    pub(crate) fn set_stage(&self, next: Stage) -> Result<(), &'static str> {
        let mut result = Ok(());
        with_run(&self.state, |state| result = state.stage(next));
        result
    }
    pub(crate) fn with_snapshot<T>(&self, inspect: impl FnOnce(&ObservationSnapshot) -> T) -> T {
        let state = self.state.lock().unwrap_or_else(|p| {
            let mut state = p.into_inner();
            state.fail("observer mutex poisoned");
            state
        });
        inspect(&state)
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
        std::mem::replace(
            &mut *state,
            ObservationSnapshot::new(0, 0).expect("zero-capacity observer"),
        )
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
                state.source_positive_attempts += 1;
                if state.source_positive_attempts > MAX_SOURCE_POSITIVE_ATTEMPTS {
                    state.fail("source positive processing attempt cap exceeded");
                }
            }
            let bytes = outcome.as_ref().map_or(0, outcome_size);
            if bytes > MAX_TEXT {
                state.fail("observer diagnostic cap exceeded");
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
    #[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
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
    #[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
    pub(crate) fn capture_identity(&self, source: BounceSourceIdentity) {
        self.with(|state, identity| {
            state.push(identity, 0, || RecordKind::StrictCaptureIdentity(source))
        });
    }
    #[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
    pub(crate) fn strict_state_load(&self, bytes: &[u8], result: &Result<(), BackendFailure>) {
        self.with(|state, identity| {
            let diagnostic_bytes = result.as_ref().err().map_or(0, |e| e.diagnostic().len());
            if bytes.len() > MAX_STATE_BYTES || diagnostic_bytes > MAX_TEXT {
                state.fail("observer state-load input cap exceeded");
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
    #[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
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
            state.push(identity, size, || RecordKind::BaseRead {
                site,
                id,
                raw_bits: result.as_ref().map(|v| v.to_bits()).map_err(Clone::clone),
            });
        });
    }
    #[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
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
            state.push(identity, bytes, || RecordKind::AudioDeclaration {
                input_buses: inputs.to_vec(),
                output_buses: outputs.to_vec(),
            });
        });
    }
    #[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
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
                Some(size) => state.push(identity, size, || RecordKind::Metadata {
                    site,
                    result: Ok(value.clone()),
                }),
                None => state.fail("observer metadata cap exceeded"),
            },
            Err(error) => {
                if error.len() > MAX_TEXT {
                    state.fail("observer metadata diagnostic cap exceeded");
                } else {
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
    #[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
    pub(crate) fn pcm(&self, frames: usize, transport: PluginTransport, reply: &BlockReply) {
        self.with(|state, identity| {
            if state.clone_replies >= MAX_CLONE_REPLIES
                || reply.channels.len() > 2
                || reply.channels.iter().any(|c| c.len() > 128)
            {
                state.fail("observer strict PCM cap exceeded");
                return;
            }
            state.clone_replies += 1;
            let bytes = reply
                .channels
                .iter()
                .map(|c| c.len() * std::mem::size_of::<f32>())
                .sum();
            state.push(identity, bytes, || RecordKind::StrictPcm {
                frames,
                transport,
                channels: reply.channels.clone(),
                transport_applied: reply.transport_applied,
                output_events: reply.output_events,
                output_events_lost: reply.output_events_lost,
            });
        });
    }
    #[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
    pub(crate) fn midi(&self, data: [u8; 3], offset: u16) {
        self.with(|state, identity| state.push(identity, 0, || RecordKind::Midi { data, offset }));
    }
    #[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
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
            state.push(identity, size, || RecordKind::Disposal {
                terminated: value.terminated,
                error: value.error.clone(),
                diagnostics: value.diagnostics.clone(),
            });
        });
    }
    #[cfg(feature = "vst3")]
    pub(crate) fn lifecycle(&self, name: &'static str) {
        self.with(|state, identity| state.push(identity, 0, || RecordKind::Lifecycle(name)));
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
    with_active(|state| state.push(None, 0, || RecordKind::AppAudioAssetEvent(event)));
}
pub(crate) fn record_app_generator_event(event: GeneratorEndpointEvent) {
    with_active(|state| state.push(None, 0, || RecordKind::AppGeneratorEndpointEvent(event)));
}
pub(crate) fn record_worker_join(worker_id: usize, outcome: WorkerJoinOutcome) {
    with_active(|state| state.push(None, 0, || RecordKind::WorkerJoin { worker_id, outcome }));
}
pub(crate) fn record_render_report(report: &RenderReport) {
    with_active(|state| state.push(None, 0, || RecordKind::RenderReport(report.clone())));
}
pub(crate) fn record_hooks(site: &'static str, inactive: bool) {
    with_active(|state| {
        if !inactive {
            state.fail("a behavior-changing test hook was active");
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
    with_active(|state| state.push(None, 0, || RecordKind::Lifecycle(site)));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inactive_observer_never_enters_record_builder() {
        InstanceObserver::default().with(|_, _| panic!("inactive observer must not record"));
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
        state.push(None, bytes, || RecordKind::RuntimeEvent {
            endpoint_id,
            project_session,
            instance_id,
            event: event.clone(),
        });
    });
}
