//! Bounded, typed readback of an actual joined transcript writer's file.
//! A successful replay proves bytes/framing/linkage, never SDK authenticity,
//! native helper reaping, correct rendering, or the caller's audibility.
use super::{
    Boundary, CaptureLabel, InstanceIdentity, InstancePhase, Operation, Stage, WorkerJoinOutcome,
    stream_app_control as app, stream_body as body, stream_codec as native,
    stream_control as control, stream_envelope as outer, stream_instances as instances,
    stream_sink as sink, stream_state as state,
};
use crate::{export::PublishedFileIdentity, instrument_bounce::BounceSourceIdentity};
use std::{
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
};

const MAX_BYTES: u64 = 192_000_000;
const MAX_RECORDS: u64 = 1_000_000;
const MAX_IDENTITIES: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum FailureKind {
    Admission,
    WriterNotJoined,
    Allocation,
    Io {
        kind: io::ErrorKind,
        raw_os_error: Option<i32>,
    },
    FileChanged,
    Bound,
    Truncated,
    Header(outer::Error),
    Body(body::Error),
    State(state::BodyError),
    Sequence,
    Timestamp,
    Stage,
    Counts,
    TrailingBytes,
    StateLinkage,
    InstanceLinkage,
    CallOrdering,
    Visitor(&'static str),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Failure {
    pub kind: FailureKind,
    pub header: Option<outer::Header>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct VisitError {
    pub code: &'static str,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReadProgress {
    Record,
    Finished,
}

// Every decoded field remains available to the borrowed visitor. Boxing any
// variant would allocate for each hot-path record; this enum stays on the stack.
#[allow(clippy::large_enum_variant)]
pub(super) enum Payload<'a> {
    Control(control::Control<'a>),
    App(app::AppControl<'a>),
    Native(native::NativeBody<'a>),
    State(state::StateBody<'a>),
}
pub(super) struct RecordView<'a> {
    pub header: outer::Header,
    pub payload: Payload<'a>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct InstanceProof {
    pub identity: InstanceIdentity,
    pub construction: outer::Header,
    pub disposal_records: u64,
    pub terminated_without_error_records: u64,
    pub worker_joined_records: u64,
    pub worker_panicked_records: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ReportEvidence {
    pub header: outer::Header,
    pub identity: BounceSourceIdentity,
    pub origin_frame: u64,
    pub musical_frames: u64,
    pub frames: usize,
    pub native_latency: u32,
    pub reported_tail: u32,
    pub fixed_tail: u32,
    pub generated_frames: u64,
    pub sample_peak_bits: u32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Summary {
    pub expected_records: u64,
    pub expected_bytes: u64,
    pub accepted_records: u64,
    pub accepted_bytes: u64,
    pub physical_bytes: u64,
    pub by_kind: [u64; 24],
    pub births: [Option<InstanceProof>; MAX_IDENTITIES],
    pub reports: [Option<ReportEvidence>; MAX_IDENTITIES],
    pub birth_count: usize,
    pub report_count: usize,
    pub disposal_records: u64,
    pub terminated_without_error_records: u64,
    pub worker_joined_records: u64,
    pub worker_panicked_records: u64,
    pub unmatched_worker_join_records: u64,
    pub strict_midi_records: u64,
    pub strict_process_attempts: u64,
    pub strict_process_ok_returns: u64,
    pub strict_pcm_records: u64,
    pub strict_pcm_frames: u64,
    pub strict_pcm_samples: u64,
    pub state_records: usize,
    pub actual_eof: bool,
    pub file_stamp_matches: bool,
    pub state_links_match: bool,
    pub instance_links_match: bool,
    pub complete: bool,
}
impl Summary {
    fn new(records: u64, bytes: u64) -> Self {
        Self {
            expected_records: records,
            expected_bytes: bytes,
            accepted_records: 0,
            accepted_bytes: 0,
            physical_bytes: 0,
            by_kind: [0; 24],
            births: [None; MAX_IDENTITIES],
            reports: [None; MAX_IDENTITIES],
            birth_count: 0,
            report_count: 0,
            disposal_records: 0,
            terminated_without_error_records: 0,
            worker_joined_records: 0,
            worker_panicked_records: 0,
            unmatched_worker_join_records: 0,
            strict_midi_records: 0,
            strict_process_attempts: 0,
            strict_process_ok_returns: 0,
            strict_pcm_records: 0,
            strict_pcm_frames: 0,
            strict_pcm_samples: 0,
            state_records: 0,
            actual_eof: false,
            file_stamp_matches: false,
            state_links_match: false,
            instance_links_match: false,
            complete: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Transport {
    sample_position: i64,
    quarter_note_bits: u64,
    tempo_bits: u64,
    playing: bool,
    numerator: i32,
    denominator: i32,
}
impl From<native::TransportBits> for Transport {
    fn from(value: native::TransportBits) -> Self {
        Self {
            sample_position: value.sample_position,
            quarter_note_bits: value.quarter_note_bits,
            tempo_bits: value.tempo_bits,
            playing: value.playing,
            numerator: value.time_sig_numerator,
            denominator: value.time_sig_denominator,
        }
    }
}
impl From<control::TransportBits> for Transport {
    fn from(value: control::TransportBits) -> Self {
        Self {
            sample_position: value.sample_position,
            quarter_note_bits: value.quarter_note_bits,
            tempo_bits: value.tempo_bits,
            playing: value.playing,
            numerator: value.numerator,
            denominator: value.denominator,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Call {
    operation: Operation,
    frames: Option<u64>,
    transport: Option<Transport>,
}
#[derive(Clone, Copy, Default)]
struct StrictFlow {
    pending: Option<Call>,
    awaiting_pcm: Option<Call>,
    midi_before_call: bool,
}

struct Facts {
    summary: Summary,
    previous: Option<outer::Header>,
    flows: [StrictFlow; MAX_IDENTITIES],
    observed_states: [Option<state::SlotLink>; state::MAX_BLOB_COUNT],
    bound_states: Option<[Option<state::SlotLink>; state::MAX_BLOB_COUNT]>,
    bound_state_count: usize,
    bound_instances: Option<instances::Dictionary>,
    initial_capture: Option<(outer::Header, state::SlotLink, usize, BounceSourceIdentity)>,
    consumption: Option<(outer::Header, BounceSourceIdentity)>,
}
impl Facts {
    fn new(records: u64, bytes: u64) -> Self {
        Self {
            summary: Summary::new(records, bytes),
            previous: None,
            flows: [StrictFlow::default(); MAX_IDENTITIES],
            observed_states: [None; state::MAX_BLOB_COUNT],
            bound_states: None,
            bound_state_count: 0,
            bound_instances: None,
            initial_capture: None,
            consumption: None,
        }
    }
    fn instance_index(&self, header: outer::Header) -> Result<Option<usize>, FailureKind> {
        let Some(identity) = header.instance else {
            return Ok(None);
        };
        self.summary.births[..self.summary.birth_count]
            .iter()
            .position(|entry| entry.is_some_and(|entry| entry.identity == identity))
            .map(Some)
            .ok_or(FailureKind::InstanceLinkage)
    }
    fn observe(&mut self, header: outer::Header, payload: &Payload<'_>) -> Result<(), FailureKind> {
        if header.sequence != self.summary.accepted_records + 1 {
            return Err(FailureKind::Sequence);
        }
        if let Some(previous) = self.previous {
            if header.monotonic_ns < previous.monotonic_ns {
                return Err(FailureKind::Timestamp);
            }
            if header.stage < previous.stage {
                return Err(FailureKind::Stage);
            }
        }
        let birth = matches!(
            payload,
            Payload::Control(control::Control::Lifecycle(
                "instance construction observed"
            ))
        );
        if birth {
            let identity = header.instance.ok_or(FailureKind::InstanceLinkage)?;
            let count = self.summary.birth_count;
            if count >= MAX_IDENTITIES
                || identity.ordinal != count as u64 + 1
                || self.summary.births[..count]
                    .iter()
                    .flatten()
                    .any(|entry| entry.identity.phase == identity.phase)
                || !match identity.phase {
                    InstancePhase::InitialSource => header.stage == Stage::InitialSource,
                    InstancePhase::StrictClone => {
                        matches!(header.stage, Stage::Conversion | Stage::WholeExport)
                    }
                    InstancePhase::UndoSource => header.stage == Stage::Undo,
                }
            {
                return Err(FailureKind::InstanceLinkage);
            }
            self.summary.births[count] = Some(InstanceProof {
                identity,
                construction: header,
                disposal_records: 0,
                terminated_without_error_records: 0,
                worker_joined_records: 0,
                worker_panicked_records: 0,
            });
            self.summary.birth_count += 1;
        }
        let instance_index = self.instance_index(header)?;
        if let Payload::State(value) = payload {
            let link = match value {
                state::StateBody::StateLoad { link, .. }
                | state::StateBody::Capture { link, .. } => *link,
            };
            let index = self.summary.state_records;
            if index >= state::MAX_BLOB_COUNT
                || link.slot() != index
                || link.original_header() != header
            {
                return Err(FailureKind::StateLinkage);
            }
            if self
                .bound_states
                .is_some_and(|links| links[index] != Some(link))
            {
                return Err(FailureKind::StateLinkage);
            }
            self.observed_states[index] = Some(link);
            self.summary.state_records += 1;
            if let state::StateBody::Capture {
                label: CaptureLabel::Initial,
                identity,
                receipt_identity,
                ..
            } = value
            {
                if header.instance.is_some() || self.initial_capture.is_some() {
                    return Err(FailureKind::StateLinkage);
                }
                self.initial_capture = Some((header, link, *receipt_identity, *identity));
            }
        }
        if let Payload::Control(value) = payload {
            match value {
                control::Control::Disposal {
                    terminated, error, ..
                } => {
                    self.summary.disposal_records += 1;
                    let healthy = *terminated && error.is_none();
                    self.summary.terminated_without_error_records += u64::from(healthy);
                    if let Some(index) = instance_index {
                        let entry = self.summary.births[index]
                            .as_mut()
                            .ok_or(FailureKind::InstanceLinkage)?;
                        entry.disposal_records += 1;
                        entry.terminated_without_error_records += u64::from(healthy);
                    }
                }
                control::Control::WorkerJoin { worker_id, outcome } => {
                    let joined = *outcome == WorkerJoinOutcome::Joined;
                    self.summary.worker_joined_records += u64::from(joined);
                    self.summary.worker_panicked_records += u64::from(!joined);
                    let mut found = false;
                    for entry in self.summary.births[..self.summary.birth_count]
                        .iter_mut()
                        .flatten()
                    {
                        if entry.identity.worker_id == Some(*worker_id) {
                            if found {
                                return Err(FailureKind::InstanceLinkage);
                            }
                            found = true;
                            entry.worker_joined_records += u64::from(joined);
                            entry.worker_panicked_records += u64::from(!joined);
                        }
                    }
                    self.summary.unmatched_worker_join_records += u64::from(!found);
                }
                control::Control::RenderReport(value) => {
                    let index = self.summary.report_count;
                    if index >= MAX_IDENTITIES {
                        return Err(FailureKind::Bound);
                    }
                    self.summary.reports[index] = Some(ReportEvidence {
                        header,
                        identity: value.identity,
                        origin_frame: value.origin_frame,
                        musical_frames: value.musical_frames,
                        frames: value.frames,
                        native_latency: value.native_latency,
                        reported_tail: value.reported_tail,
                        fixed_tail: value.fixed_tail,
                        generated_frames: value.generated_frames,
                        sample_peak_bits: value.sample_peak_bits,
                    });
                    self.summary.report_count += 1;
                }
                control::Control::StrictCaptureIdentity(identity) => {
                    if self.consumption.is_some() {
                        return Err(FailureKind::InstanceLinkage);
                    }
                    self.consumption = Some((header, *identity));
                }
                _ => {}
            }
        }
        if let Some(index) = instance_index
            && self.summary.births[index]
                .is_some_and(|entry| entry.identity.phase == InstancePhase::StrictClone)
        {
            self.strict_flow(index, payload)?;
        }
        self.previous = Some(header);
        Ok(())
    }
    fn strict_flow(&mut self, index: usize, payload: &Payload<'_>) -> Result<(), FailureKind> {
        let flow = &mut self.flows[index];
        if let Payload::Native(native::NativeBody::Midi { .. }) = payload {
            if flow.midi_before_call || flow.pending.is_some() || flow.awaiting_pcm.is_some() {
                return Err(FailureKind::CallOrdering);
            }
            flow.midi_before_call = true;
            self.summary.strict_midi_records += 1;
        }
        let call = match payload {
            Payload::Native(native::NativeBody::Call {
                operation,
                boundary,
                frames,
                transport,
                outcome,
            }) => Some((
                Call {
                    operation: match operation {
                        native::NativeOperation::Prepare => Operation::StrictPrepare,
                        native::NativeOperation::Metadata => Operation::StrictMetadata,
                        native::NativeOperation::Midi => Operation::StrictMidi,
                        native::NativeOperation::Process => Operation::StrictProcess,
                    },
                    frames: *frames,
                    transport: transport.map(Transport::from),
                },
                match boundary {
                    native::Boundary::Attempt => Boundary::Attempt,
                    native::Boundary::Returned => Boundary::Returned,
                },
                matches!(outcome, native::BorrowedOutcome::Ok),
            )),
            Payload::Control(control::Control::Call {
                operation,
                boundary,
                frames,
                transport,
                outcome,
            }) if matches!(
                operation,
                Operation::StrictPrepare
                    | Operation::StrictMetadata
                    | Operation::StrictMidi
                    | Operation::StrictProcess
                    | Operation::StrictDispose
            ) =>
            {
                Some((
                    Call {
                        operation: *operation,
                        frames: frames.map(|frames| frames as u64),
                        transport: transport.map(Transport::from),
                    },
                    *boundary,
                    matches!(outcome, Some(control::Outcome::Ok)),
                ))
            }
            _ => None,
        };
        if let Some((call, boundary, ok)) = call {
            match boundary {
                Boundary::Attempt => {
                    if flow.pending.is_some()
                        || flow.awaiting_pcm.is_some()
                        || (call.operation == Operation::StrictMidi) != flow.midi_before_call
                    {
                        return Err(FailureKind::CallOrdering);
                    }
                    flow.midi_before_call = false;
                    flow.pending = Some(call);
                    if call.operation == Operation::StrictProcess {
                        self.summary.strict_process_attempts += 1;
                    }
                }
                Boundary::Returned => {
                    if flow.pending.take() != Some(call) {
                        return Err(FailureKind::CallOrdering);
                    }
                    if call.operation == Operation::StrictProcess && ok {
                        flow.awaiting_pcm = Some(call);
                        self.summary.strict_process_ok_returns += 1;
                    }
                }
            }
        }
        if let Payload::Native(native::NativeBody::StrictPcm(pcm)) = payload {
            let previous = flow.awaiting_pcm.take().ok_or(FailureKind::CallOrdering)?;
            if previous.frames != Some(pcm.frames)
                || previous.transport != Some(pcm.transport.into())
                || flow.pending.is_some()
                || flow.midi_before_call
            {
                return Err(FailureKind::CallOrdering);
            }
            let mut samples = 0_u64;
            for channel in pcm.channels() {
                if channel.len() as u64 != pcm.frames {
                    return Err(FailureKind::CallOrdering);
                }
                samples = samples
                    .checked_add(channel.len() as u64)
                    .ok_or(FailureKind::Bound)?;
            }
            self.summary.strict_pcm_records += 1;
            self.summary.strict_pcm_frames = self
                .summary
                .strict_pcm_frames
                .checked_add(pcm.frames)
                .ok_or(FailureKind::Bound)?;
            self.summary.strict_pcm_samples = self
                .summary
                .strict_pcm_samples
                .checked_add(samples)
                .ok_or(FailureKind::Bound)?;
        }
        if matches!(payload, Payload::Control(control::Control::Disposal { .. })) {
            let call = flow.pending.take().ok_or(FailureKind::CallOrdering)?;
            if call.operation != Operation::StrictDispose {
                return Err(FailureKind::CallOrdering);
            }
        }
        Ok(())
    }
    fn finish_links(&mut self) -> Result<(), FailureKind> {
        if self.summary.state_records != 0 {
            let links = self
                .bound_states
                .as_ref()
                .ok_or(FailureKind::StateLinkage)?;
            if self.bound_state_count != self.summary.state_records
                || *links != self.observed_states
            {
                return Err(FailureKind::StateLinkage);
            }
        } else if self.bound_state_count != 0 {
            return Err(FailureKind::StateLinkage);
        }
        self.summary.state_links_match = true;
        if self.summary.birth_count != 0 {
            let dictionary = self
                .bound_instances
                .as_ref()
                .ok_or(FailureKind::InstanceLinkage)?;
            if dictionary.entries().count() != self.summary.birth_count {
                return Err(FailureKind::InstanceLinkage);
            }
            for proof in self.summary.births[..self.summary.birth_count]
                .iter()
                .flatten()
            {
                let entry = dictionary
                    .lookup_exact(proof.identity)
                    .map_err(|_| FailureKind::InstanceLinkage)?;
                if entry.construction.global_sequence != proof.construction.sequence
                    || entry.construction.monotonic_elapsed_ns != proof.construction.monotonic_ns
                {
                    return Err(FailureKind::InstanceLinkage);
                }
            }
            match (self.initial_capture, dictionary.capture()) {
                (None, None) => {}
                (Some((header, link, receipt, identity)), Some(expected))
                    if header.instance.is_none()
                        && expected.header.global_sequence == header.sequence
                        && expected.header.monotonic_elapsed_ns == header.monotonic_ns
                        && expected.known_owner_source_ordinal == link.supplied_owner().ordinal
                        && expected.receipt_identity == receipt
                        && expected.source == identity => {}
                _ => return Err(FailureKind::InstanceLinkage),
            }
            match (self.consumption, dictionary.consumption()) {
                (None, None) => {}
                (Some((header, identity)), Some(expected))
                    if expected.header.global_sequence == header.sequence
                        && expected.header.monotonic_elapsed_ns == header.monotonic_ns
                        && header
                            .instance
                            .is_some_and(|instance| instance.ordinal == expected.clone_ordinal)
                        && identity == expected.source => {}
                _ => return Err(FailureKind::InstanceLinkage),
            }
        } else if self.bound_instances.is_some()
            || self.initial_capture.is_some()
            || self.consumption.is_some()
        {
            return Err(FailureKind::InstanceLinkage);
        }
        if self.flows.iter().any(|flow| {
            flow.pending.is_some() || flow.awaiting_pcm.is_some() || flow.midi_before_call
        }) {
            return Err(FailureKind::CallOrdering);
        }
        self.summary.instance_links_match = true;
        Ok(())
    }
}

#[must_use = "keep failed readback ownership and its exact partial operand for inspection"]
pub(super) struct ReplayReader<'dict> {
    file: File,
    path: PathBuf,
    initial_stamp: PublishedFileIdentity,
    dictionary: &'dict native::BaselineDictionary,
    buffer: Vec<u8>,
    facts: Facts,
    failure: Option<Failure>,
}
impl<'dict> ReplayReader<'dict> {
    pub(super) fn open_joined(
        absolute_path: &Path,
        receipt: &sink::Terminal<'_>,
        dictionary: &'dict native::BaselineDictionary,
    ) -> Result<Self, Failure> {
        let expected = receipt.expected;
        let fail = |kind| Failure { kind, header: None };
        if !receipt.complete() {
            return Err(fail(FailureKind::WriterNotJoined));
        }
        if !absolute_path.is_absolute()
            || expected.bytes > MAX_BYTES
            || expected.records > MAX_RECORDS
        {
            return Err(fail(FailureKind::Admission));
        }
        let file = File::open(absolute_path).map_err(|error| fail(io_kind(&error)))?;
        // Reuse the established cross-platform OS file-handle identity query.
        // This independently opened handle never borrows the writer's handle.
        let initial_stamp =
            PublishedFileIdentity::from_file(&file).map_err(|error| fail(io_kind(&error)))?;
        if initial_stamp.bytes != expected.bytes {
            return Err(fail(FailureKind::Counts));
        }
        let mut buffer = Vec::new();
        if buffer.try_reserve_exact(outer::MAX_OPERAND).is_err()
            || buffer.capacity() > outer::MAX_OPERAND
        {
            return Err(fail(FailureKind::Allocation));
        }
        Ok(Self {
            file,
            path: absolute_path.to_owned(),
            initial_stamp,
            dictionary,
            buffer,
            facts: Facts::new(expected.records, expected.bytes),
            failure: None,
        })
    }
    fn reject(&mut self, kind: FailureKind, header: Option<outer::Header>) -> Failure {
        self.facts.summary.complete = false;
        self.failure.get_or_insert(Failure { kind, header }).clone()
    }
    pub(super) fn bind_instances(
        &mut self,
        dictionary: &instances::Dictionary,
    ) -> Result<(), Failure> {
        if let Some(failure) = &self.failure {
            return Err(failure.clone());
        }
        if self.facts.summary.complete || self.facts.bound_instances.is_some() {
            return Err(self.reject(FailureKind::InstanceLinkage, None));
        }
        self.facts.bound_instances = Some(dictionary.clone());
        Ok(())
    }
    pub(super) fn bind_state_ranges(
        &mut self,
        linked: state::LinkedRanges<'_>,
    ) -> Result<(), Failure> {
        if let Some(failure) = &self.failure {
            return Err(failure.clone());
        }
        if self.facts.summary.complete
            || self.facts.bound_states.is_some()
            || linked.ranges.len() > state::MAX_BLOB_COUNT
            || linked.ranges.is_empty()
        {
            return Err(self.reject(FailureKind::StateLinkage, None));
        }
        for (index, link) in linked.links.iter().enumerate() {
            match (link, linked.ranges.get(index)) {
                (None, None) => {}
                (Some(link), Some(range)) if range_matches(*link, index, range) => {}
                _ => return Err(self.reject(FailureKind::StateLinkage, None)),
            }
        }
        self.facts.bound_states = Some(*linked.links);
        self.facts.bound_state_count = linked.ranges.len();
        Ok(())
    }
    pub(super) fn read_next(
        &mut self,
        visitor: impl FnOnce(RecordView<'_>) -> Result<(), VisitError>,
    ) -> Result<ReadProgress, Failure> {
        if let Some(failure) = &self.failure {
            return Err(failure.clone());
        }
        if self.facts.summary.complete {
            return Ok(ReadProgress::Finished);
        }
        if self.facts.summary.accepted_records == self.facts.summary.expected_records {
            return self.finish();
        }
        self.buffer.clear();
        let mut prefix = [0_u8; 4];
        let mut prefix_read = 0;
        while prefix_read < prefix.len() {
            match self.file.read(&mut prefix[prefix_read..]) {
                Ok(0) => {
                    self.buffer.extend_from_slice(&prefix[..prefix_read]);
                    return Err(self.reject(FailureKind::Truncated, None));
                }
                Ok(used) => {
                    prefix_read += used;
                    self.facts.summary.physical_bytes += used as u64;
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => {
                    self.buffer.extend_from_slice(&prefix[..prefix_read]);
                    return Err(self.reject(io_kind(&error), None));
                }
            }
        }
        self.buffer.extend_from_slice(&prefix);
        let length = u32::from_le_bytes(prefix) as usize;
        let total = length.checked_add(4);
        if total.is_none_or(|total| total > outer::MAX_OPERAND || total <= 4)
            || self
                .facts
                .summary
                .accepted_bytes
                .checked_add(total.unwrap_or(0) as u64)
                .is_none_or(|bytes| bytes > self.facts.summary.expected_bytes)
        {
            return Err(self.reject(FailureKind::Bound, None));
        }
        let total = total.expect("admitted record length");
        self.buffer.resize(total, 0);
        let mut received = 4;
        while received < total {
            match self.file.read(&mut self.buffer[received..]) {
                Ok(0) => {
                    self.buffer.truncate(received);
                    return Err(self.reject(FailureKind::Truncated, None));
                }
                Ok(used) => {
                    received += used;
                    self.facts.summary.physical_bytes += used as u64;
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => {
                    self.buffer.truncate(received);
                    return Err(self.reject(io_kind(&error), None));
                }
            }
        }
        let (header, bytes) = match outer::decode(&self.buffer) {
            Ok(value) => value,
            Err(error) => return Err(self.reject(FailureKind::Header(error), None)),
        };
        let payload = match header.lane {
            outer::Lane::Control => match control::decode(bytes, header.kind) {
                Ok(value) => Payload::Control(value),
                Err(error) => {
                    return Err(
                        self.reject(FailureKind::Body(body::Error::Control(error)), Some(header))
                    );
                }
            },
            outer::Lane::App => match app::decode(bytes, header.kind) {
                Ok(value) => Payload::App(value),
                Err(error) => {
                    return Err(
                        self.reject(FailureKind::Body(body::Error::App(error)), Some(header))
                    );
                }
            },
            outer::Lane::Native(tag) => {
                match native::decode_native_body_with_limit(
                    tag,
                    bytes,
                    self.dictionary,
                    outer::MAX_BODY,
                ) {
                    Ok(value) if value.kind() == header.kind => Payload::Native(value),
                    Ok(_) => {
                        return Err(self
                            .reject(FailureKind::Body(body::Error::HeaderMismatch), Some(header)));
                    }
                    Err(error) => {
                        return Err(self
                            .reject(FailureKind::Body(body::Error::Native(error)), Some(header)));
                    }
                }
            }
            outer::Lane::State => match state::decode_body(header, bytes, Some(self.dictionary)) {
                Ok(value) => Payload::State(value),
                Err(error) => return Err(self.reject(FailureKind::State(error), Some(header))),
            },
        };
        if let Err(kind) = self.facts.observe(header, &payload) {
            return Err(self.reject(kind, Some(header)));
        }
        if let Err(error) = visitor(RecordView { header, payload }) {
            return Err(self.reject(FailureKind::Visitor(error.code), Some(header)));
        }
        self.facts.summary.accepted_records += 1;
        self.facts.summary.accepted_bytes += total as u64;
        self.facts.summary.by_kind[header.kind as usize] += 1;
        Ok(ReadProgress::Record)
    }
    fn finish(&mut self) -> Result<ReadProgress, Failure> {
        if self.facts.summary.accepted_bytes != self.facts.summary.expected_bytes
            || self.facts.summary.physical_bytes != self.facts.summary.expected_bytes
        {
            return Err(self.reject(FailureKind::Counts, None));
        }
        let mut sentinel = [0_u8; 1];
        loop {
            match self.file.read(&mut sentinel) {
                Ok(0) => break,
                Ok(used) => {
                    self.facts.summary.physical_bytes += used as u64;
                    return Err(self.reject(FailureKind::TrailingBytes, None));
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(self.reject(io_kind(&error), None)),
            }
        }
        self.facts.summary.actual_eof = true;
        let stamp = PublishedFileIdentity::from_file(&self.file)
            .map_err(|error| self.reject(io_kind(&error), None))?;
        let path_file =
            File::open(&self.path).map_err(|error| self.reject(io_kind(&error), None))?;
        let path_stamp = PublishedFileIdentity::from_file(&path_file)
            .map_err(|error| self.reject(io_kind(&error), None))?;
        if stamp != self.initial_stamp || path_stamp != self.initial_stamp {
            return Err(self.reject(FailureKind::FileChanged, None));
        }
        self.facts.summary.file_stamp_matches = true;
        if let Err(kind) = self.facts.finish_links() {
            return Err(self.reject(kind, None));
        }
        self.facts.summary.complete = true;
        Ok(ReadProgress::Finished)
    }
    pub(super) fn summary(&self) -> &Summary {
        &self.facts.summary
    }
    pub(super) fn failure(&self) -> Option<&Failure> {
        self.failure.as_ref()
    }
    pub(super) fn retained_operand(&self) -> &[u8] {
        &self.buffer
    }
    pub(super) fn operand_capacity(&self) -> usize {
        self.buffer.capacity()
    }
}
fn io_kind(error: &io::Error) -> FailureKind {
    FailureKind::Io {
        kind: error.kind(),
        raw_os_error: error.raw_os_error(),
    }
}
fn range_matches(
    link: state::SlotLink,
    index: usize,
    range: &state::CheckedRangeReference,
) -> bool {
    let header = link.original_header();
    let owner = link.supplied_owner();
    let expected = state::SuppliedIdentity {
        phase: match owner.phase {
            InstancePhase::InitialSource => state::SuppliedPhase::InitialSource,
            InstancePhase::StrictClone => state::SuppliedPhase::StrictClone,
            InstancePhase::UndoSource => state::SuppliedPhase::RestoredSource,
        },
        worker_id: owner.worker_id.map(|id| id as u64),
        source_id: owner.source_instance_id,
        instance_ordinal: owner.ordinal,
        record_ordinal: header.sequence,
        monotonic_elapsed_ns: header.monotonic_ns,
    };
    link.slot() == index
        && range.identity == expected
        && range.length == link.byte_len() as u64
        && range.original_owned_capacity == link.owned_capacity() as u64
        && range.length > 0
        && range.length <= range.original_owned_capacity
        && range.original_owned_capacity <= state::MAX_BLOB_BYTES as u64
        && (1..=state::MAX_BLOB_COUNT as u64).contains(&range.unique_blob_ordinal)
        && range
            .offset
            .checked_add(range.length)
            .is_some_and(|end| end <= state::MAX_ARCHIVE_BYTES)
}

#[cfg(test)]
#[path = "stream_replay_tests.rs"]
mod tests;
