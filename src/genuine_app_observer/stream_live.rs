//! Explicit run-owned streaming. The caller supplies actual tap retirement;
//! this owner proves only transcript/sidecar writer retirement and readback.
use super::{
    CaptureLabel, InstanceIdentity, InstancePhase, Record, RecordKind, Stage,
    stream_app_control as app, stream_codec as native, stream_control as control,
    stream_envelope as envelope, stream_instances as instances, stream_sink as sink,
    stream_state as state,
};
use crate::instrument_bounce::{BouncePluginMetadata, BouncePluginState, BounceSourceIdentity};
use std::{
    mem::ManuallyDrop,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Instant,
};

pub(super) enum View<'a> {
    Control(control::View<'a>),
    App(app::AppView<'a>),
    Native(native::NativeView<'a>),
    StateLoad {
        bytes: &'a [u8],
        outcome: control::OutcomeRef<'a>,
    },
    Capture {
        label: CaptureLabel,
        identity: BounceSourceIdentity,
        receipt_identity: usize,
        state: &'a BouncePluginState,
    },
}
impl View<'_> {
    fn kind(&self) -> u16 {
        match self {
            Self::Control(v) => v.kind(),
            Self::App(v) => v.kind(),
            Self::Native(v) => v.kind(),
            Self::StateLoad { .. } => 2,
            Self::Capture { .. } => 9,
        }
    }
    fn lane(&self) -> envelope::Lane {
        match self {
            Self::Control(_) => envelope::Lane::Control,
            Self::App(_) => envelope::Lane::App,
            Self::Native(v) => envelope::Lane::Native(v.tag()),
            Self::StateLoad { .. } | Self::Capture { .. } => envelope::Lane::State,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StreamError {
    pub(crate) code: &'static str,
    detail: StreamErrorDetail,
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum StreamErrorDetail {
    Admission(&'static str),
    Baseline(native::CodecError),
    Instances(instances::Error),
    State(state::SlotError),
    // The complete typed cause remains in the original actor/StartFailure.
    // Public failure copies retain correlation without duplicating that owner.
    Sink {
        attempted_header: Option<envelope::Header>,
    },
}
impl StreamError {
    pub(crate) fn admission(code: &'static str) -> Self {
        Self {
            code,
            detail: StreamErrorDetail::Admission(code),
        }
    }
    fn baseline(error: native::CodecError) -> Self {
        Self {
            code: "actual source baseline encoding failed",
            detail: StreamErrorDetail::Baseline(error),
        }
    }
    fn instances(error: instances::Error) -> Self {
        Self {
            code: "actual instance dictionary linkage failed",
            detail: StreamErrorDetail::Instances(error),
        }
    }
    fn state(error: state::SlotError) -> Self {
        Self {
            code: "retained state slot rejected original operand",
            detail: StreamErrorDetail::State(error),
        }
    }
    fn sink(error: sink::Failure) -> Self {
        Self {
            code: "transcript actor rejected original operand",
            detail: StreamErrorDetail::Sink {
                attempted_header: error.attempted_header,
            },
        }
    }
}

pub(super) fn from_record(record: &RecordKind) -> Result<View<'_>, StreamError> {
    Ok(match record {
        RecordKind::StateLoad { bytes, outcome } => View::StateLoad {
            bytes,
            outcome: outcome.into(),
        },
        RecordKind::Metadata { site, result } => View::Native(native::NativeView::Metadata {
            site,
            result: result.as_ref().map_err(String::as_str),
        }),
        RecordKind::AudioDeclaration {
            input_buses,
            output_buses,
        } => View::Native(native::NativeView::AudioDeclaration {
            inputs: input_buses,
            outputs: output_buses,
        }),
        RecordKind::EventDeclaration { inputs, outputs } => {
            View::Native(native::NativeView::EventDeclaration { inputs, outputs })
        }
        RecordKind::StrictPcm {
            frames,
            transport,
            channels,
            transport_applied,
            output_events,
            output_events_lost,
        } => View::Native(native::NativeView::StrictPcm {
            frames: *frames,
            transport,
            channels,
            transport_applied: *transport_applied,
            output_events: *output_events,
            output_events_lost: *output_events_lost,
        }),
        RecordKind::Midi { data, offset } => View::Native(native::NativeView::Midi {
            data: *data,
            offset: *offset,
        }),
        RecordKind::Capture {
            label,
            identity,
            receipt_identity,
            state,
        } => View::Capture {
            label: *label,
            identity: *identity,
            receipt_identity: *receipt_identity,
            state,
        },
        RecordKind::AppAudioAssetEvent(_)
        | RecordKind::AppGeneratorEndpointEvent(_)
        | RecordKind::AppTimelineRuntimeEvent { .. }
        | RecordKind::RuntimeEvent { .. } => View::App(app::view_record(record).map_err(|_| {
            StreamError::admission("original App record has no complete borrowed adapter")
        })?),
        _ => View::Control(control::view_record(record).map_err(|_| {
            StreamError::admission("original record has no complete borrowed adapter")
        })?),
    })
}

fn baseline(
    id: u64,
    metadata: &BouncePluginMetadata,
) -> Result<native::BaselineDictionary, StreamError> {
    let buses = |values: &[crate::instrument_bounce::BounceAudioBus]| -> Result<Vec<native::AudioBus>, StreamError> {
        values.iter().map(|bus| Ok(native::AudioBus { channel_count: u32::try_from(bus.channel_count).map_err(|_| StreamError::admission("actual source bus channel count exceeds wire width"))?, is_main: bus.is_main, active: bus.active })).collect()
    };
    let events = |values: &[(u32, bool, Option<bool>)]| {
        values
            .iter()
            .map(|&(channel_count, is_main, active)| native::EventBus {
                channel_count,
                is_main,
                active,
            })
            .collect()
    };
    native::BaselineDictionary::new(
        id,
        native::CanonicalMetadata {
            class_uid: metadata.class_uid.clone(),
            version: metadata.version.clone(),
            name: metadata.name.clone(),
            category: metadata.category.clone(),
            audio: native::AudioLayout {
                inputs: buses(&metadata.input_buses)?,
                outputs: buses(&metadata.output_buses)?,
            },
            events: native::EventLayout {
                inputs: events(&metadata.event_input_buses),
                outputs: events(&metadata.event_output_buses),
            },
            latency_samples: metadata.latency_samples,
            tail_samples: metadata.tail_samples,
        },
    )
    .map_err(StreamError::baseline)
}

pub(super) struct LiveStream {
    actor: Option<sink::StreamSink>,
    startup_failure: Option<sink::StartFailure>,
    states: Option<state::LiveStateSlots>,
    instances: instances::Builder,
    source: InstanceIdentity,
    transcript: PathBuf,
    sidecar_path: PathBuf,
    next_sequence: u64,
    first_capture: Option<u64>,
    failure: Option<StreamError>,
    pub(super) failed_header_to_retain: Option<envelope::Header>,
    pub(super) emitted_records: u64,
    pub(super) pcm_replies: u64,
    pub(super) source_positive_attempts: u64,
    prelude: Vec<Arc<Record>>,
    failed_original: Option<Arc<Record>>,
    recovery: Arc<Mutex<RetainedStreamSlot>>,
    externally_retired: bool,
}
struct RetainedStreamSlot {
    owner: Option<RetainedStream>,
}
#[allow(clippy::large_enum_variant)] // Fixed whole owners must survive failed allocation without allocating a box.
enum RetainedStream {
    Live(ManuallyDrop<LiveStream>),
    Retirement(ManuallyDrop<StreamRetirement>),
}
#[derive(Clone)]
#[must_use = "retain this handle to recover the same abandoned whole stream owner"]
pub(crate) struct StreamRecoveryHandle {
    slot: Arc<Mutex<RetainedStreamSlot>>,
}
impl StreamRecoveryHandle {
    pub(crate) fn take_after_taps_retired(
        &self,
        retired: bool,
    ) -> Result<StreamRetirement, StreamError> {
        if !retired {
            return Err(StreamError::admission(
                "actual caller tap retirement is required",
            ));
        }
        let owner = self
            .slot
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .owner
            .take()
            .ok_or(StreamError::admission(
                "no abandoned whole stream owner is retained",
            ))?;
        Ok(match owner {
            RetainedStream::Live(owner) => ManuallyDrop::into_inner(owner).retire(),
            RetainedStream::Retirement(owner) => ManuallyDrop::into_inner(owner),
        })
    }
}
impl LiveStream {
    pub(super) fn start(
        transcript: &Path,
        sidecar: &Path,
        source: InstanceIdentity,
        deadline: Instant,
        prelude: &[std::sync::Arc<Record>],
    ) -> Result<Self, StreamError> {
        if !transcript.is_absolute()
            || !sidecar.is_absolute()
            || transcript == sidecar
            || transcript.as_os_str().as_encoded_bytes().len() > state::MAX_PATH_BYTES
            || sidecar.as_os_str().as_encoded_bytes().len() > state::MAX_PATH_BYTES
            || source.phase != InstancePhase::InitialSource
            || source.ordinal == 0
            || source.source_instance_id.is_none()
            || Instant::now() >= deadline
        {
            return Err(StreamError::admission(
                "stream requires distinct bounded absolute paths, actual initial source identity and a future deadline",
            ));
        }
        let (metadata_sequence, metadata) = prelude
            .iter()
            .find_map(|record| {
                if record.instance == Some(source)
                    && let RecordKind::Metadata {
                        result: Ok(metadata),
                        ..
                    } = &record.kind
                {
                    Some((record.ordinal, metadata))
                } else {
                    None
                }
            })
            .ok_or(StreamError::admission(
                "stream requires retained successful Metadata from the exact actual source",
            ))?;
        if super::metadata_size(metadata).is_none() {
            return Err(StreamError::admission("observer metadata cap exceeded"));
        }
        let dictionary = baseline(metadata_sequence, metadata)?;
        let mut retained_prelude = Vec::new();
        retained_prelude
            .try_reserve_exact(prelude.len())
            .map_err(|_| StreamError::admission("stream prelude handle reservation failed"))?;
        retained_prelude.extend(prelude.iter().cloned());
        let states = state::LiveStateSlots::new(state::RetentionLimits::default())
            .map_err(StreamError::state)?;
        let mut result = Self {
            actor: None,
            startup_failure: None,
            states: Some(states),
            instances: instances::Builder::new(),
            source,
            transcript: transcript.to_path_buf(),
            sidecar_path: sidecar.to_path_buf(),
            next_sequence: 1,
            first_capture: None,
            failure: None,
            failed_header_to_retain: None,
            emitted_records: 0,
            pcm_replies: 0,
            source_positive_attempts: 0,
            prelude: retained_prelude,
            failed_original: None,
            recovery: Arc::new(Mutex::new(RetainedStreamSlot { owner: None })),
            externally_retired: false,
        };
        match sink::StreamSink::create_new(dictionary, transcript, deadline) {
            Ok(actor) => result.actor = Some(actor),
            Err(failure) => {
                result.failure = Some(StreamError::sink(failure.failure.clone()));
                result.startup_failure = Some(failure);
                return Ok(result);
            }
        }
        for record in prelude {
            let view = match from_record(&record.kind) {
                Ok(view) => view,
                Err(error) => {
                    result.failure = Some(error);
                    result.cancel();
                    break;
                }
            };
            if result
                .emit(
                    record.ordinal,
                    record.monotonic_elapsed_ns,
                    record.stage,
                    record.instance,
                    view,
                )
                .is_err()
            {
                break;
            }
        }
        // Replay failure's operand is already permanently owned by the prelude.
        result.failed_header_to_retain = None;
        Ok(result)
    }
    pub(super) fn failure(&self) -> Option<&StreamError> {
        self.failure.as_ref()
    }
    pub(super) fn next_sequence(&self) -> u64 {
        self.next_sequence
    }
    pub(super) fn observer_failed(&mut self, failure: &'static str) {
        if self.failure.is_none() {
            self.failure = Some(StreamError::admission(failure));
            self.cancel();
        }
    }
    pub(super) fn retain_failed_original(&mut self, record: Arc<Record>) {
        if self.failed_original.is_none() {
            self.failed_original = Some(record);
        }
    }
    pub(super) fn recovery_handle(&self) -> StreamRecoveryHandle {
        StreamRecoveryHandle {
            slot: Arc::clone(&self.recovery),
        }
    }
    fn take_complete_owner(&mut self) -> Self {
        let owner = Self {
            actor: self.actor.take(),
            startup_failure: self.startup_failure.take(),
            states: self.states.take(),
            instances: std::mem::take(&mut self.instances),
            source: self.source,
            transcript: std::mem::take(&mut self.transcript),
            sidecar_path: std::mem::take(&mut self.sidecar_path),
            next_sequence: self.next_sequence,
            first_capture: self.first_capture,
            failure: self.failure.take(),
            failed_header_to_retain: self.failed_header_to_retain.take(),
            emitted_records: self.emitted_records,
            pcm_replies: self.pcm_replies,
            source_positive_attempts: self.source_positive_attempts,
            prelude: std::mem::take(&mut self.prelude),
            failed_original: self.failed_original.take(),
            recovery: Arc::clone(&self.recovery),
            externally_retired: self.externally_retired,
        };
        self.externally_retired = true;
        owner
    }
    fn cancel(&mut self) {
        if let Some(actor) = &mut self.actor {
            actor.cancel();
        }
    }
    pub(super) fn emit(
        &mut self,
        sequence: u64,
        monotonic_ns: u64,
        stage: Stage,
        instance: Option<InstanceIdentity>,
        view: View<'_>,
    ) -> Result<(), StreamError> {
        if let Some(failure) = &self.failure {
            return Err(failure.clone());
        }
        let header = envelope::Header {
            sequence,
            monotonic_ns,
            stage,
            instance,
            kind: view.kind(),
            lane: view.lane(),
        };
        let result = self.emit_original(header, view);
        match result {
            Ok(()) => {
                self.emitted_records += 1;
                self.next_sequence = sequence
                    .checked_add(1)
                    .ok_or(StreamError::admission("observer stream sequence overflow"))?;
                if header.kind == 7 {
                    self.pcm_replies += 1;
                }
                Ok(())
            }
            Err(error) => {
                self.failure = Some(error.clone());
                self.failed_header_to_retain = Some(header);
                self.cancel();
                Err(error)
            }
        }
    }
    fn emit_original(
        &mut self,
        header: envelope::Header,
        view: View<'_>,
    ) -> Result<(), StreamError> {
        let construction = instances::RecordHeader {
            global_sequence: header.sequence,
            monotonic_elapsed_ns: header.monotonic_ns,
        };
        match &view {
            View::Control(control::View::Call {
                operation: super::Operation::OrdinaryProcess | super::Operation::ResetOrigin,
                boundary: super::Boundary::Attempt,
                frames: Some(frames),
                ..
            }) if *frames > 0 => {
                self.source_positive_attempts = self
                    .source_positive_attempts
                    .checked_add(1)
                    .ok_or(StreamError::admission(
                        "source positive processing attempt cap exceeded",
                    ))?;
            }
            View::Control(control::View::Lifecycle("instance construction observed")) => {
                self.instances
                    .add_entry(
                        construction,
                        header.instance.ok_or(StreamError::admission(
                            "instance construction has no original instance",
                        ))?,
                    )
                    .map_err(StreamError::instances)?;
            }
            View::Capture {
                label: CaptureLabel::Initial,
                identity,
                receipt_identity,
                ..
            } => {
                self.instances
                    .record_initial_capture(
                        construction,
                        CaptureLabel::Initial,
                        header.instance,
                        self.source.ordinal,
                        *receipt_identity,
                        *identity,
                    )
                    .map_err(StreamError::instances)?;
                self.first_capture = Some(header.sequence);
            }
            View::Control(control::View::StrictCaptureIdentity(source)) => {
                self.instances
                    .record_strict_consumption(
                        construction,
                        header.instance,
                        self.source.ordinal,
                        self.first_capture.ok_or(StreamError::admission(
                            "strict consumption has no original initial Capture",
                        ))?,
                        **source,
                    )
                    .map_err(StreamError::instances)?;
            }
            _ => {}
        }
        let actor = self
            .actor
            .as_mut()
            .ok_or(StreamError::admission("stream actor unavailable"))?;
        match view {
            View::Control(view) => actor.emit_control(header, view),
            View::App(view) => actor.emit_app(header, view),
            View::Native(view) => actor.emit_native(header, view),
            View::StateLoad { bytes, outcome } => {
                let view = self
                    .states
                    .as_mut()
                    .expect("unsealed states")
                    .retain_state_load(header, None, bytes, outcome)
                    .map_err(StreamError::state)?;
                actor.emit_state(header, view)
            }
            View::Capture {
                label,
                identity,
                receipt_identity,
                state: captured,
            } => {
                let view = self
                    .states
                    .as_mut()
                    .expect("unsealed states")
                    .retain_capture(
                        header,
                        self.source,
                        state::CaptureOperands {
                            label,
                            identity,
                            receipt_identity,
                            state: captured,
                        },
                    )
                    .map_err(StreamError::state)?;
                actor.emit_state(header, view)
            }
        }
        .map_err(StreamError::sink)
    }
    pub(super) fn retire(mut self) -> StreamRetirement {
        self.externally_retired = true;
        let instances = self.instances.clone().freeze();
        let mut seal_error = self.failure.clone();
        if let Some(actor) = &mut self.actor
            && let Err(error) = actor.seal_after_taps_retired(true)
        {
            seal_error.get_or_insert(StreamError::sink(error));
        }
        let state_owner = match self
            .states
            .take()
            .expect("retained unsealed states")
            .into_batch()
        {
            Ok(batch) => match batch.start(state::SidecarConfig {
                absolute_path: self.sidecar_path.clone(),
                max_blob_bytes: state::MAX_BLOB_BYTES,
                max_blobs: state::MAX_BLOB_COUNT,
                max_owned_capacity_bytes: state::MAX_OWNED_CAPACITY_BYTES,
                max_archive_bytes: state::MAX_ARCHIVE_BYTES,
            }) {
                Ok(sidecar) => StateOwner::Running(sidecar),
                Err(failure) => StateOwner::StartFailed(failure),
            },
            Err(failure) => StateOwner::SealFailed(failure),
        };
        StreamRetirement {
            live: self,
            state_owner,
            state_terminal: None,
            state_links: [None; state::MAX_BLOB_COUNT],
            state_linkage: None,
            instances,
            seal_error,
        }
    }
}
impl Drop for LiveStream {
    fn drop(&mut self) {
        if self.externally_retired
            || (self.actor.is_none() && self.startup_failure.is_none() && self.prelude.is_empty())
        {
            return;
        }
        let owner = self.take_complete_owner();
        let mut slot = self.recovery.lock().unwrap_or_else(|p| p.into_inner());
        if slot.owner.is_none() {
            slot.owner = Some(RetainedStream::Live(ManuallyDrop::new(owner)));
        } else {
            std::mem::forget(owner);
        } // Never overwrite another retained complete owner.
    }
}

#[allow(clippy::large_enum_variant)] // Original fixed state slots survive seal/start failure without another allocation.
enum StateOwner {
    Running(state::LiveStateSidecar),
    StartFailed(state::LiveStartFailure),
    SealFailed(state::SealFailure),
    Transferred,
}
#[must_use = "retain the complete actor/state owner until actual writer retirement"]
pub(crate) struct StreamRetirement {
    live: LiveStream,
    state_owner: StateOwner,
    state_terminal: Option<state::Terminal>,
    state_links: [Option<state::SlotLink>; state::MAX_BLOB_COUNT],
    state_linkage: Option<Result<(), state::SlotError>>,
    instances: Result<instances::Dictionary, instances::Error>,
    seal_error: Option<StreamError>,
}
pub(super) struct StreamInspection<'a> {
    pub(super) transcript_path: &'a Path,
    pub(super) sidecar_path: &'a Path,
    pub(super) actor: Option<sink::Inspection<'a>>,
    pub(super) startup_failure: Option<&'a sink::StartFailure>,
    pub(super) instances: Result<&'a instances::Dictionary, &'a instances::Error>,
    pub(super) states: Option<state::LinkedRanges<'a>>,
    pub(super) state_terminal: Option<&'a state::Terminal>,
    pub(super) failure: Option<&'a StreamError>,
    pub(super) state_start_failure: Option<&'a state::LiveStartFailure>,
    pub(super) state_seal_failure: Option<&'a state::SealFailure>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StreamFinishPoll {
    Pending,
    Joined { complete: bool },
}
impl StreamRetirement {
    pub(crate) fn recovery_handle(&self) -> StreamRecoveryHandle {
        self.live.recovery_handle()
    }
    pub(crate) fn cancel(&mut self) {
        self.live.cancel();
        if let StateOwner::Running(sidecar) = &mut self.state_owner {
            sidecar.cancel();
        }
        self.seal_error
            .get_or_insert(StreamError::admission("stream retirement cancelled"));
    }
    pub(crate) fn poll_finish(&mut self) -> StreamFinishPoll {
        let actor_joined = self
            .live
            .actor
            .as_mut()
            .is_none_or(|actor| matches!(actor.poll_finish(), sink::FinishPoll::Joined(_)));
        let states_joined = match &mut self.state_owner {
            StateOwner::Running(sidecar) => match sidecar.poll_join() {
                state::LiveJoinPoll::Pending => false,
                state::LiveJoinPoll::Joined { terminal, linkage } => {
                    self.state_terminal = Some(terminal.clone());
                    self.state_linkage = Some(match linkage {
                        Ok(linkage) => {
                            self.state_links = *linkage.links;
                            Ok(())
                        }
                        Err(error) => Err(error),
                    });
                    true
                }
            },
            StateOwner::StartFailed(failure) => {
                let _ = &failure.failure.owners;
                true
            }
            StateOwner::SealFailed(failure) => {
                let _ = &failure.slots;
                true
            }
            StateOwner::Transferred => false,
        };
        if actor_joined && states_joined {
            let complete = self.seal_error.is_none()
                && self.instances.is_ok()
                && self.state_linkage == Some(Ok(()))
                && self
                    .live
                    .actor
                    .as_ref()
                    .and_then(sink::StreamSink::terminal)
                    .is_some_and(|terminal| terminal.complete());
            StreamFinishPoll::Joined { complete }
        } else {
            StreamFinishPoll::Pending
        }
    }
    pub(super) fn inspect(&self) -> StreamInspection<'_> {
        let states = if self.state_linkage == Some(Ok(())) {
            self.state_terminal
                .as_ref()
                .and_then(|t| t.receipt.as_ref())
                .map(|r| state::LinkedRanges {
                    links: &self.state_links,
                    ranges: &r.references,
                })
        } else {
            None
        };
        StreamInspection {
            transcript_path: &self.live.transcript,
            sidecar_path: &self.live.sidecar_path,
            actor: self.live.actor.as_ref().map(sink::StreamSink::inspect),
            startup_failure: self.live.startup_failure.as_ref(),
            instances: self.instances.as_ref(),
            states,
            state_terminal: self.state_terminal.as_ref(),
            failure: self.seal_error.as_ref().or(self.live.failure.as_ref()),
            state_start_failure: if let StateOwner::StartFailed(failure) = &self.state_owner {
                Some(failure)
            } else {
                None
            },
            state_seal_failure: if let StateOwner::SealFailed(failure) = &self.state_owner {
                Some(failure)
            } else {
                None
            },
        }
    }
}
impl Drop for StreamRetirement {
    fn drop(&mut self) {
        let actor_joined = self.live.actor.as_ref().is_none_or(|actor| {
            actor
                .terminal()
                .is_some_and(|terminal| terminal.writer.writer_joined)
        });
        let state_joined = !matches!(self.state_owner, StateOwner::Running(_))
            || self
                .state_terminal
                .as_ref()
                .is_some_and(|terminal| terminal.actual_writer_joined);
        if actor_joined && state_joined {
            return;
        }
        let live = self.live.take_complete_owner();
        let recovery = Arc::clone(&live.recovery);
        let owner = Self {
            live,
            state_owner: std::mem::replace(&mut self.state_owner, StateOwner::Transferred),
            state_terminal: self.state_terminal.take(),
            state_links: self.state_links,
            state_linkage: self.state_linkage.take(),
            instances: std::mem::replace(&mut self.instances, Err(instances::Error::Empty)),
            seal_error: self.seal_error.take(),
        };
        let mut slot = recovery.lock().unwrap_or_else(|p| p.into_inner());
        if slot.owner.is_none() {
            slot.owner = Some(RetainedStream::Retirement(ManuallyDrop::new(owner)));
        } else {
            std::mem::forget(owner);
        }
    }
}

#[cfg(test)]
#[path = "stream_live_tests.rs"]
mod tests;
