//! Closed control wire v1. Register as a child of genuine_app_observer only
//! with experimental-fresh-helper/vst3 enabled. Borrow operands at their real
//! taps; no RecordKind, opaque state, PCM, or diagnostic formatting is cloned.
//! The parent owns global sequence/time/stage/optional instance identity and
//! the high-frequency codec. This body never invents an instance for Capture.
#![cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
use super::{Boundary, CallOutcome, Operation, RecordKind, WorkerCaptureGuard, WorkerJoinOutcome};
use crate::{
    instrument_bounce::{
        BounceSourceIdentity,
        render::{BackendDelivery, BackendFailure, BackendFailureKind, RenderReport},
    },
    plugins::plugin_runtime::{
        BridgeStats, MAX_PLUGIN_CHAIN_SLOTS, PluginEndpointManifest, PluginTransport,
    },
};
use vst3_host::{
    MetadataQueryRejection,
    audio::{BusDirection, MediaType},
    strict_offline::{
        FailureKind as StrictKind, LimitEvidence, LimitSite, MetadataGetter, RequestContext,
    },
};

pub(super) const MAX_CONTROL_BYTES: usize = 64 * 1024;
const MAX_TEXT: usize = 4096;
const MAX_DIAGNOSTIC: usize = 2048;
const MAX_DISPOSAL_DIAGNOSTICS: usize = 8;
const MAGIC: &[u8; 4] = b"NCTL";
const VERSION: u16 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Error {
    DestinationNotEmpty,
    DestinationCapacity,
    Bound,
    Truncated,
    TrailingBytes,
    InvalidMagic,
    UnknownVersion,
    UnknownCode,
    InvalidBoolean,
    InvalidOption,
    InvalidUtf8,
    InvalidRequestContext,
    InvalidCall,
    KindMismatch,
    UnsupportedKind(u16),
}

/// Native references supplied directly by a tap. The high-frequency caller
/// selects its own compact lane; Call is also available for its *failure* body
/// so a StrictError never collapses into a string or one-bit failure marker.
#[derive(Clone, Copy)]
pub(super) enum OutcomeRef<'a> {
    Ok,
    Error(&'a str),
    StrictError(&'a BackendFailure),
}
impl<'a> From<&'a CallOutcome> for OutcomeRef<'a> {
    fn from(value: &'a CallOutcome) -> Self {
        match value {
            CallOutcome::Ok => Self::Ok,
            CallOutcome::Error(text) => Self::Error(text),
            CallOutcome::StrictError(failure) => Self::StrictError(failure),
        }
    }
}
pub(super) enum View<'a> {
    Call {
        operation: Operation,
        boundary: Boundary,
        frames: Option<usize>,
        transport: Option<&'a PluginTransport>,
        outcome: Option<OutcomeRef<'a>>,
    },
    BaseRead {
        site: &'a str,
        id: u32,
        raw_bits: Result<u64, &'a str>,
    },
    WorkerCaptureGuard {
        worker_id: Option<usize>,
        operands: &'a WorkerCaptureGuard,
    },
    WorkerJoin {
        worker_id: usize,
        outcome: WorkerJoinOutcome,
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
        process_error: Option<&'a str>,
    },
    StrictCaptureIdentity(&'a BounceSourceIdentity),
    RenderReport(&'a RenderReport),
    Disposal {
        terminated: bool,
        error: Option<&'a BackendFailure>,
        diagnostics: &'a [String],
    },
    Worker {
        site: &'a str,
        worker_id: usize,
        manifest: &'a PluginEndpointManifest,
        epoch: u64,
        sequence: Option<u64>,
        stats: &'a BridgeStats,
    },
    HookInventory {
        site: &'a str,
        inactive: bool,
    },
    Lifecycle(&'a str),
}
impl View<'_> {
    pub(super) const fn kind(&self) -> u16 {
        match self {
            Self::Call { .. } => 1,
            Self::BaseRead { .. } => 3,
            Self::WorkerCaptureGuard { .. } => 13,
            Self::WorkerJoin { .. } => 14,
            Self::OrdinaryState { .. } => 16,
            Self::ResetReport { .. } => 17,
            Self::StrictCaptureIdentity(_) => 18,
            Self::RenderReport(_) => 19,
            Self::Disposal { .. } => 20,
            Self::Worker { .. } => 21,
            Self::HookInventory { .. } => 22,
            Self::Lifecycle(_) => 23,
        }
    }
}

/// Adapter for existing retained records. Real streaming taps should call
/// encode(View) directly, avoiding creation/cloning of a RecordKind.
pub(super) fn view_record(kind: &RecordKind) -> Result<View<'_>, Error> {
    Ok(match kind {
        RecordKind::Call {
            operation,
            boundary,
            frames,
            transport,
            outcome,
        } => View::Call {
            operation: *operation,
            boundary: *boundary,
            frames: *frames,
            transport: transport.as_ref(),
            outcome: outcome.as_ref().map(OutcomeRef::from),
        },
        RecordKind::WorkerCaptureGuard {
            worker_id,
            operands,
        } => View::WorkerCaptureGuard {
            worker_id: *worker_id,
            operands,
        },
        RecordKind::WorkerJoin { worker_id, outcome } => View::WorkerJoin {
            worker_id: *worker_id,
            outcome: *outcome,
        },
        RecordKind::OrdinaryState {
            endpoint_id,
            project_session,
            instance_id,
            slot,
            request_id,
            epoch,
            byte_len,
        } => View::OrdinaryState {
            endpoint_id: *endpoint_id,
            project_session: *project_session,
            instance_id: *instance_id,
            slot: *slot,
            request_id: *request_id,
            epoch: *epoch,
            byte_len: *byte_len,
        },
        RecordKind::ResetReport {
            frames,
            discarded_prior_midi_events,
            discarded_midi_events,
            discarded_prior_parameter_points,
            discarded_parameter_points,
            output_events_lost,
            parameter_output_fault,
            process_error,
        } => View::ResetReport {
            frames: *frames,
            discarded_prior_midi_events: *discarded_prior_midi_events,
            discarded_midi_events: *discarded_midi_events,
            discarded_prior_parameter_points: *discarded_prior_parameter_points,
            discarded_parameter_points: *discarded_parameter_points,
            output_events_lost: *output_events_lost,
            parameter_output_fault: *parameter_output_fault,
            process_error: process_error.as_deref(),
        },
        RecordKind::StrictCaptureIdentity(identity) => View::StrictCaptureIdentity(identity),
        RecordKind::RenderReport(report) => View::RenderReport(report),
        RecordKind::Disposal {
            terminated,
            error,
            diagnostics,
        } => View::Disposal {
            terminated: *terminated,
            error: error.as_ref(),
            diagnostics,
        },
        RecordKind::Worker {
            site,
            worker_id,
            manifest,
            epoch,
            sequence,
            stats,
        } => View::Worker {
            site,
            worker_id: *worker_id,
            manifest,
            epoch: *epoch,
            sequence: *sequence,
            stats,
        },
        RecordKind::HookInventory { site, inactive } => View::HookInventory {
            site,
            inactive: *inactive,
        },
        RecordKind::Lifecycle(site) => View::Lifecycle(site),
        RecordKind::StateLoad { .. } => return Err(Error::UnsupportedKind(2)),
        RecordKind::BaseRead { site, id, raw_bits } => View::BaseRead {
            site,
            id: *id,
            raw_bits: raw_bits.as_ref().copied().map_err(String::as_str),
        },
        RecordKind::Metadata { .. } => return Err(Error::UnsupportedKind(4)),
        RecordKind::AudioDeclaration { .. } => return Err(Error::UnsupportedKind(5)),
        RecordKind::EventDeclaration { .. } => return Err(Error::UnsupportedKind(6)),
        RecordKind::StrictPcm { .. } => return Err(Error::UnsupportedKind(7)),
        RecordKind::Midi { .. } => return Err(Error::UnsupportedKind(8)),
        RecordKind::Capture { .. } => return Err(Error::UnsupportedKind(9)),
        RecordKind::AppAudioAssetEvent(_) => return Err(Error::UnsupportedKind(10)),
        RecordKind::AppGeneratorEndpointEvent(_) => return Err(Error::UnsupportedKind(11)),
        RecordKind::AppTimelineRuntimeEvent { .. } => return Err(Error::UnsupportedKind(12)),
        RecordKind::RuntimeEvent { .. } => return Err(Error::UnsupportedKind(15)),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TransportBits {
    pub sample_position: i64,
    pub quarter_note_bits: u64,
    pub tempo_bits: u64,
    pub playing: bool,
    pub numerator: i32,
    pub denominator: i32,
}
impl From<&PluginTransport> for TransportBits {
    fn from(value: &PluginTransport) -> Self {
        Self {
            sample_position: value.sample_position,
            quarter_note_bits: value.quarter_note_position.to_bits(),
            tempo_bits: value.tempo.to_bits(),
            playing: value.playing,
            numerator: value.time_sig_numerator,
            denominator: value.time_sig_denominator,
        }
    }
}
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Failure<'a> {
    pub kind: BackendFailureKind,
    pub delivery: Option<BackendDelivery>,
    pub diagnostic: &'a str,
    pub strict_kind: Option<StrictKind>,
    pub strict_diagnostic_unavailable: Option<StrictKind>,
    pub strict_stage: Option<&'a str>,
    pub last_wire_request: Option<RequestContext>,
    pub strict_metadata_getter: Option<MetadataGetter>,
    pub limit_evidence: Option<LimitEvidence>,
}
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Outcome<'a> {
    Ok,
    Error(&'a str),
    StrictError(Failure<'a>),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Manifest {
    pub slot_count: usize,
    pub identified: bool,
    pub instance_ids: [Option<u64>; MAX_PLUGIN_CHAIN_SLOTS],
}
#[derive(Debug, PartialEq, Eq)]
pub(super) struct TextList<'a> {
    pub len: usize,
    values: [&'a str; MAX_DISPOSAL_DIAGNOSTICS],
}
impl<'a> TextList<'a> {
    pub(super) fn values(&self) -> &[&'a str] {
        &self.values[..self.len]
    }
}
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Render {
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
// The bounded decoded value keeps all manifest slots and all 32 BridgeStats
// fields on the stack. Boxing the Worker variant would allocate on every
// decoded worker record; the complete enum is a fixed, sub-kilobyte value.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Control<'a> {
    Call {
        operation: Operation,
        boundary: Boundary,
        frames: Option<usize>,
        transport: Option<TransportBits>,
        outcome: Option<Outcome<'a>>,
    },
    BaseRead {
        site: &'a str,
        id: u32,
        raw_bits: Result<u64, &'a str>,
    },
    WorkerCaptureGuard {
        worker_id: Option<usize>,
        operands: WorkerCaptureGuard,
    },
    WorkerJoin {
        worker_id: usize,
        outcome: WorkerJoinOutcome,
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
        process_error: Option<&'a str>,
    },
    StrictCaptureIdentity(BounceSourceIdentity),
    RenderReport(Render),
    Disposal {
        terminated: bool,
        error: Option<Failure<'a>>,
        diagnostics: TextList<'a>,
    },
    Worker {
        site: &'a str,
        worker_id: usize,
        manifest: Manifest,
        epoch: u64,
        sequence: Option<u64>,
        stats: BridgeStats,
    },
    HookInventory {
        site: &'a str,
        inactive: bool,
    },
    Lifecycle(&'a str),
}

struct Writer<'a> {
    destination: &'a mut Vec<u8>,
    total_body_budget: usize,
}
impl Writer<'_> {
    fn bytes(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let length = self
            .destination
            .len()
            .checked_add(bytes.len())
            .ok_or(Error::Bound)?;
        if length > self.total_body_budget {
            return Err(Error::Bound);
        }
        if length > self.destination.capacity() {
            return Err(Error::DestinationCapacity);
        }
        self.destination.extend_from_slice(bytes);
        Ok(())
    }
    fn u8(&mut self, value: u8) -> Result<(), Error> {
        self.bytes(&[value])
    }
    fn u16(&mut self, value: u16) -> Result<(), Error> {
        self.bytes(&value.to_le_bytes())
    }
    fn u32(&mut self, value: u32) -> Result<(), Error> {
        self.bytes(&value.to_le_bytes())
    }
    fn u64(&mut self, value: u64) -> Result<(), Error> {
        self.bytes(&value.to_le_bytes())
    }
    fn usize(&mut self, value: usize) -> Result<(), Error> {
        self.u64(u64::try_from(value).map_err(|_| Error::Bound)?)
    }
    fn bool(&mut self, value: bool) -> Result<(), Error> {
        self.u8(u8::from(value))
    }
    fn option<T>(
        &mut self,
        value: Option<T>,
        encode: impl FnOnce(&mut Self, T) -> Result<(), Error>,
    ) -> Result<(), Error> {
        self.bool(value.is_some())?;
        if let Some(value) = value {
            encode(self, value)?;
        }
        Ok(())
    }
    fn text(&mut self, value: &str, maximum: usize) -> Result<(), Error> {
        if value.len() > maximum {
            return Err(Error::Bound);
        }
        self.u32(u32::try_from(value.len()).map_err(|_| Error::Bound)?)?;
        self.bytes(value.as_bytes())
    }
}
struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}
impl<'a> Reader<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8], Error> {
        let end = self.position.checked_add(length).ok_or(Error::Bound)?;
        let value = self.bytes.get(self.position..end).ok_or(Error::Truncated)?;
        self.position = end;
        Ok(value)
    }
    fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, Error> {
        Ok(u16::from_le_bytes(
            self.take(2)?.try_into().map_err(|_| Error::Truncated)?,
        ))
    }
    fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().map_err(|_| Error::Truncated)?,
        ))
    }
    fn u64(&mut self) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().map_err(|_| Error::Truncated)?,
        ))
    }
    fn usize(&mut self) -> Result<usize, Error> {
        usize::try_from(self.u64()?).map_err(|_| Error::Bound)
    }
    fn bool(&mut self) -> Result<bool, Error> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(Error::InvalidBoolean),
        }
    }
    fn option<T>(
        &mut self,
        decode: impl FnOnce(&mut Self) -> Result<T, Error>,
    ) -> Result<Option<T>, Error> {
        match self.u8()? {
            0 => Ok(None),
            1 => decode(self).map(Some),
            _ => Err(Error::InvalidOption),
        }
    }
    fn text(&mut self, maximum: usize) -> Result<&'a str, Error> {
        let length = usize::try_from(self.u32()?).map_err(|_| Error::Bound)?;
        if length > maximum {
            return Err(Error::Bound);
        }
        std::str::from_utf8(self.take(length)?).map_err(|_| Error::InvalidUtf8)
    }
}

/// Never reallocates. Success returns the explicit kind code for the parent's
/// RetainedControl envelope. A partial body from this attempt is cleared on
/// failure; rejecting a preoccupied destination preserves its original bytes.
pub(super) fn encode(view: View<'_>, destination: &mut Vec<u8>) -> Result<u16, Error> {
    encode_with_limit(view, destination, MAX_CONTROL_BYTES)
}

/// Limit the entire typed body, including MAGIC/version/kind. The caller reserves
/// its canonical envelope separately and passes the remaining operand budget;
/// this codec neither guesses that envelope's size nor drops fields to fit it.
/// Budgets above the v1 64KiB bound are rejected, never silently clamped.
pub(super) fn encode_with_limit(
    view: View<'_>,
    destination: &mut Vec<u8>,
    total_body_budget: usize,
) -> Result<u16, Error> {
    if !destination.is_empty() {
        return Err(Error::DestinationNotEmpty);
    }
    if total_body_budget > MAX_CONTROL_BYTES {
        return Err(Error::Bound);
    }
    if destination.capacity() > MAX_CONTROL_BYTES {
        return Err(Error::DestinationCapacity);
    }
    let kind = view.kind();
    let result = (|| {
        let mut w = Writer {
            destination,
            total_body_budget,
        };
        w.bytes(MAGIC)?;
        w.u16(VERSION)?;
        w.u16(kind)?;
        encode_view(view, &mut w)
    })();
    if result.is_err() {
        destination.clear();
    }
    result.map(|()| kind)
}
fn encode_view(view: View<'_>, w: &mut Writer<'_>) -> Result<(), Error> {
    match view {
        View::Call {
            operation,
            boundary,
            frames,
            transport,
            outcome,
        } => {
            validate_call(boundary, outcome.is_some())?;
            w.u8(operation_code(operation))?;
            w.u8(match boundary {
                Boundary::Attempt => 1,
                Boundary::Returned => 2,
            })?;
            w.option(frames, Writer::usize)?;
            w.option(transport, |w, value| write_transport(w, value))?;
            w.option(outcome, write_outcome)?;
        }
        View::BaseRead { site, id, raw_bits } => {
            w.text(site, MAX_TEXT)?;
            w.u32(id)?;
            match raw_bits {
                Ok(bits) => {
                    w.u8(1)?;
                    w.u64(bits)?;
                }
                Err(text) => {
                    w.u8(2)?;
                    w.text(text, MAX_TEXT)?;
                }
            }
        }
        View::WorkerCaptureGuard {
            worker_id,
            operands,
        } => {
            w.option(worker_id, Writer::usize)?;
            w.u64(operands.expected_epoch)?;
            w.u64(operands.active_epoch)?;
            w.u64(operands.requested_epoch)?;
            w.bool(operands.input_empty)?;
            w.u32(operands.parameter_admission)?;
            w.bool(operands.accepted)?;
        }
        View::WorkerJoin { worker_id, outcome } => {
            w.usize(worker_id)?;
            w.u8(match outcome {
                WorkerJoinOutcome::Joined => 1,
                WorkerJoinOutcome::WorkerPanicked => 2,
            })?;
        }
        View::OrdinaryState {
            endpoint_id,
            project_session,
            instance_id,
            slot,
            request_id,
            epoch,
            byte_len,
        } => {
            w.u64(endpoint_id)?;
            w.u64(project_session)?;
            w.u64(instance_id)?;
            w.usize(slot)?;
            w.u64(request_id)?;
            w.option(epoch, Writer::u64)?;
            w.usize(byte_len)?;
        }
        View::ResetReport {
            frames,
            discarded_prior_midi_events,
            discarded_midi_events,
            discarded_prior_parameter_points,
            discarded_parameter_points,
            output_events_lost,
            parameter_output_fault,
            process_error,
        } => {
            w.u32(frames)?;
            w.u64(discarded_prior_midi_events)?;
            w.u64(discarded_midi_events)?;
            w.u64(discarded_prior_parameter_points)?;
            w.u64(discarded_parameter_points)?;
            w.bool(output_events_lost)?;
            w.bool(parameter_output_fault)?;
            w.option(process_error, |w, text| w.text(text, MAX_TEXT))?;
        }
        View::StrictCaptureIdentity(identity) => write_identity(w, identity)?,
        View::RenderReport(report) => {
            write_identity(w, &report.identity)?;
            w.u64(report.origin_frame)?;
            w.u64(report.musical_frames)?;
            w.usize(report.frames)?;
            w.u32(report.native_latency)?;
            w.u32(report.reported_tail)?;
            w.u32(report.fixed_tail)?;
            w.u64(report.generated_frames)?;
            w.u32(report.sample_peak.to_bits())?;
        }
        View::Disposal {
            terminated,
            error,
            diagnostics,
        } => {
            if diagnostics.len() > MAX_DISPOSAL_DIAGNOSTICS {
                return Err(Error::Bound);
            }
            w.bool(terminated)?;
            w.option(error, write_failure)?;
            w.usize(diagnostics.len())?;
            for text in diagnostics {
                w.text(text, MAX_TEXT)?;
            }
        }
        View::Worker {
            site,
            worker_id,
            manifest,
            epoch,
            sequence,
            stats,
        } => {
            if manifest.slot_count > MAX_PLUGIN_CHAIN_SLOTS {
                return Err(Error::Bound);
            }
            w.text(site, MAX_TEXT)?;
            w.usize(worker_id)?;
            w.usize(manifest.slot_count)?;
            w.bool(manifest.is_identified())?;
            for identity in manifest.instance_ids {
                w.option(identity, Writer::u64)?;
            }
            w.u64(epoch)?;
            w.option(sequence, Writer::u64)?;
            write_stats(w, stats)?;
        }
        View::HookInventory { site, inactive } => {
            w.text(site, MAX_TEXT)?;
            w.bool(inactive)?;
        }
        View::Lifecycle(site) => w.text(site, MAX_TEXT)?,
    }
    Ok(())
}

/// StateLoad has an outcome but no Operation/Boundary in the original tap.
/// Encode that original value without inventing a Call to carry its failure.
pub(super) fn encode_outcome_only(
    outcome: OutcomeRef<'_>,
    destination: &mut Vec<u8>,
    total_body_budget: usize,
) -> Result<(), Error> {
    if !destination.is_empty() {
        return Err(Error::DestinationNotEmpty);
    }
    if total_body_budget > MAX_CONTROL_BYTES {
        return Err(Error::Bound);
    }
    if destination.capacity() > MAX_CONTROL_BYTES {
        return Err(Error::DestinationCapacity);
    }
    let result = (|| {
        let mut w = Writer {
            destination,
            total_body_budget,
        };
        w.bytes(b"NOUT")?;
        w.u16(VERSION)?;
        write_outcome(&mut w, outcome)
    })();
    if result.is_err() {
        destination.clear();
    }
    result
}

pub(super) fn decode_outcome_only(bytes: &[u8]) -> Result<Outcome<'_>, Error> {
    if bytes.len() > MAX_CONTROL_BYTES {
        return Err(Error::Bound);
    }
    let mut r = Reader { bytes, position: 0 };
    if r.take(4)? != b"NOUT" {
        return Err(Error::InvalidMagic);
    }
    if r.u16()? != VERSION {
        return Err(Error::UnknownVersion);
    }
    let value = read_outcome(&mut r)?;
    if r.position != bytes.len() {
        return Err(Error::TrailingBytes);
    }
    Ok(value)
}
pub(super) fn decode(bytes: &[u8], expected_kind: u16) -> Result<Control<'_>, Error> {
    if bytes.len() > MAX_CONTROL_BYTES {
        return Err(Error::Bound);
    }
    let mut r = Reader { bytes, position: 0 };
    if r.take(4)? != MAGIC {
        return Err(Error::InvalidMagic);
    }
    if r.u16()? != VERSION {
        return Err(Error::UnknownVersion);
    }
    let kind = r.u16()?;
    if kind != expected_kind {
        return Err(Error::KindMismatch);
    }
    let value = match kind {
        1 => Control::Call {
            operation: read_operation(r.u8()?)?,
            boundary: match r.u8()? {
                1 => Boundary::Attempt,
                2 => Boundary::Returned,
                _ => return Err(Error::UnknownCode),
            },
            frames: r.option(Reader::usize)?,
            transport: r.option(read_transport)?,
            outcome: r.option(read_outcome)?,
        },
        3 => Control::BaseRead {
            site: r.text(MAX_TEXT)?,
            id: r.u32()?,
            raw_bits: match r.u8()? {
                1 => Ok(r.u64()?),
                2 => Err(r.text(MAX_TEXT)?),
                _ => return Err(Error::UnknownCode),
            },
        },
        13 => Control::WorkerCaptureGuard {
            worker_id: r.option(Reader::usize)?,
            operands: WorkerCaptureGuard {
                expected_epoch: r.u64()?,
                active_epoch: r.u64()?,
                requested_epoch: r.u64()?,
                input_empty: r.bool()?,
                parameter_admission: r.u32()?,
                accepted: r.bool()?,
            },
        },
        14 => Control::WorkerJoin {
            worker_id: r.usize()?,
            outcome: match r.u8()? {
                1 => WorkerJoinOutcome::Joined,
                2 => WorkerJoinOutcome::WorkerPanicked,
                _ => return Err(Error::UnknownCode),
            },
        },
        16 => Control::OrdinaryState {
            endpoint_id: r.u64()?,
            project_session: r.u64()?,
            instance_id: r.u64()?,
            slot: r.usize()?,
            request_id: r.u64()?,
            epoch: r.option(Reader::u64)?,
            byte_len: r.usize()?,
        },
        17 => Control::ResetReport {
            frames: r.u32()?,
            discarded_prior_midi_events: r.u64()?,
            discarded_midi_events: r.u64()?,
            discarded_prior_parameter_points: r.u64()?,
            discarded_parameter_points: r.u64()?,
            output_events_lost: r.bool()?,
            parameter_output_fault: r.bool()?,
            process_error: r.option(|r| r.text(MAX_TEXT))?,
        },
        18 => Control::StrictCaptureIdentity(read_identity(&mut r)?),
        19 => Control::RenderReport(Render {
            identity: read_identity(&mut r)?,
            origin_frame: r.u64()?,
            musical_frames: r.u64()?,
            frames: r.usize()?,
            native_latency: r.u32()?,
            reported_tail: r.u32()?,
            fixed_tail: r.u32()?,
            generated_frames: r.u64()?,
            sample_peak_bits: r.u32()?,
        }),
        20 => {
            let terminated = r.bool()?;
            let error = r.option(read_failure)?;
            let len = r.usize()?;
            if len > MAX_DISPOSAL_DIAGNOSTICS {
                return Err(Error::Bound);
            }
            let mut values = [""; MAX_DISPOSAL_DIAGNOSTICS];
            for text in &mut values[..len] {
                *text = r.text(MAX_TEXT)?;
            }
            Control::Disposal {
                terminated,
                error,
                diagnostics: TextList { len, values },
            }
        }
        21 => {
            let site = r.text(MAX_TEXT)?;
            let worker_id = r.usize()?;
            let slot_count = r.usize()?;
            if slot_count > MAX_PLUGIN_CHAIN_SLOTS {
                return Err(Error::Bound);
            }
            let identified = r.bool()?;
            let mut instance_ids = [None; MAX_PLUGIN_CHAIN_SLOTS];
            for identity in &mut instance_ids {
                *identity = r.option(Reader::u64)?;
            }
            Control::Worker {
                site,
                worker_id,
                manifest: Manifest {
                    slot_count,
                    identified,
                    instance_ids,
                },
                epoch: r.u64()?,
                sequence: r.option(Reader::u64)?,
                stats: read_stats(&mut r)?,
            }
        }
        22 => Control::HookInventory {
            site: r.text(MAX_TEXT)?,
            inactive: r.bool()?,
        },
        23 => Control::Lifecycle(r.text(MAX_TEXT)?),
        2 | 4..=12 | 15 => return Err(Error::UnsupportedKind(kind)),
        _ => return Err(Error::UnknownCode),
    };
    if let Control::Call {
        boundary, outcome, ..
    } = &value
    {
        validate_call(*boundary, outcome.is_some())?;
    }
    if r.position != bytes.len() {
        return Err(Error::TrailingBytes);
    }
    Ok(value)
}
fn validate_call(boundary: Boundary, has_outcome: bool) -> Result<(), Error> {
    match (boundary, has_outcome) {
        (Boundary::Attempt, false) | (Boundary::Returned, true) => Ok(()),
        _ => Err(Error::InvalidCall),
    }
}
fn write_transport(w: &mut Writer<'_>, v: &PluginTransport) -> Result<(), Error> {
    w.u64(v.sample_position as u64)?;
    w.u64(v.quarter_note_position.to_bits())?;
    w.u64(v.tempo.to_bits())?;
    w.bool(v.playing)?;
    w.u32(v.time_sig_numerator as u32)?;
    w.u32(v.time_sig_denominator as u32)
}
fn read_transport(r: &mut Reader<'_>) -> Result<TransportBits, Error> {
    Ok(TransportBits {
        sample_position: r.u64()? as i64,
        quarter_note_bits: r.u64()?,
        tempo_bits: r.u64()?,
        playing: r.bool()?,
        numerator: r.u32()? as i32,
        denominator: r.u32()? as i32,
    })
}
fn write_identity(w: &mut Writer<'_>, v: &BounceSourceIdentity) -> Result<(), Error> {
    w.u64(v.project_session)?;
    w.u64(v.project_fingerprint)?;
    w.u64(v.timeline_revision)?;
    w.u64(v.epoch)?;
    w.u64(v.endpoint_id)?;
    w.u32(v.channel_id)?;
    w.u64(v.instance_id)
}
fn read_identity(r: &mut Reader<'_>) -> Result<BounceSourceIdentity, Error> {
    Ok(BounceSourceIdentity {
        project_session: r.u64()?,
        project_fingerprint: r.u64()?,
        timeline_revision: r.u64()?,
        epoch: r.u64()?,
        endpoint_id: r.u64()?,
        channel_id: r.u32()?,
        instance_id: r.u64()?,
    })
}
fn write_outcome(w: &mut Writer<'_>, outcome: OutcomeRef<'_>) -> Result<(), Error> {
    match outcome {
        OutcomeRef::Ok => w.u8(1),
        OutcomeRef::Error(text) => {
            w.u8(2)?;
            w.text(text, MAX_TEXT)
        }
        OutcomeRef::StrictError(failure) => {
            w.u8(3)?;
            write_failure(w, failure)
        }
    }
}
fn read_outcome<'a>(r: &mut Reader<'a>) -> Result<Outcome<'a>, Error> {
    match r.u8()? {
        1 => Ok(Outcome::Ok),
        2 => Ok(Outcome::Error(r.text(MAX_TEXT)?)),
        3 => Ok(Outcome::StrictError(read_failure(r)?)),
        _ => Err(Error::UnknownCode),
    }
}
fn write_failure(w: &mut Writer<'_>, failure: &BackendFailure) -> Result<(), Error> {
    w.u8(match failure.kind {
        BackendFailureKind::Cancelled => 1,
        BackendFailureKind::Deadline => 2,
        BackendFailureKind::Backend => 3,
    })?;
    w.option(failure.delivery, |w, value| {
        w.u8(match value {
            BackendDelivery::NotSent => 1,
            BackendDelivery::MayHaveApplied => 2,
        })
    })?;
    w.text(failure.diagnostic(), MAX_DIAGNOSTIC)?;
    w.option(failure.strict_kind, write_strict_kind)?;
    w.option(failure.strict_diagnostic_unavailable, write_strict_kind)?;
    w.option(failure.strict_stage, |w, text| w.text(text, MAX_TEXT))?;
    w.option(failure.last_wire_request, write_request_context)?;
    w.option(failure.strict_metadata_getter, |w, value| {
        w.u8(match value {
            MetadataGetter::CheckedAudioBusLayout => 1,
            MetadataGetter::EventBusLayout => 2,
            MetadataGetter::LatencySamples => 3,
            MetadataGetter::TailSamples => 4,
        })
    })?;
    w.option(failure.limit_evidence, |w, value| {
        w.u8(match value.site {
            LimitSite::ParameterCatalogEntries => 1,
            LimitSite::ParameterTextBytes => 2,
            LimitSite::ParameterCatalogTextBytes => 3,
        })?;
        w.usize(value.observed_at_least)?;
        w.usize(value.maximum)
    })
}
fn read_failure<'a>(r: &mut Reader<'a>) -> Result<Failure<'a>, Error> {
    Ok(Failure {
        kind: match r.u8()? {
            1 => BackendFailureKind::Cancelled,
            2 => BackendFailureKind::Deadline,
            3 => BackendFailureKind::Backend,
            _ => return Err(Error::UnknownCode),
        },
        delivery: r.option(|r| match r.u8()? {
            1 => Ok(BackendDelivery::NotSent),
            2 => Ok(BackendDelivery::MayHaveApplied),
            _ => Err(Error::UnknownCode),
        })?,
        diagnostic: r.text(MAX_DIAGNOSTIC)?,
        strict_kind: r.option(read_strict_kind)?,
        strict_diagnostic_unavailable: r.option(read_strict_kind)?,
        strict_stage: r.option(|r| r.text(MAX_TEXT))?,
        last_wire_request: r.option(read_request_context)?,
        strict_metadata_getter: r.option(|r| match r.u8()? {
            1 => Ok(MetadataGetter::CheckedAudioBusLayout),
            2 => Ok(MetadataGetter::EventBusLayout),
            3 => Ok(MetadataGetter::LatencySamples),
            4 => Ok(MetadataGetter::TailSamples),
            _ => Err(Error::UnknownCode),
        })?,
        limit_evidence: r.option(|r| {
            Ok(LimitEvidence {
                site: match r.u8()? {
                    1 => LimitSite::ParameterCatalogEntries,
                    2 => LimitSite::ParameterTextBytes,
                    3 => LimitSite::ParameterCatalogTextBytes,
                    _ => return Err(Error::UnknownCode),
                },
                observed_at_least: r.usize()?,
                maximum: r.usize()?,
            })
        })?,
    })
}
fn operation_code(operation: Operation) -> u8 {
    match operation {
        Operation::SourceLoad => 1,
        Operation::SourcePrepare => 2,
        Operation::OrdinaryProcess => 3,
        Operation::Reset => 4,
        Operation::ResetOrigin => 5,
        Operation::MidiPanic => 6,
        Operation::StopProcessing => 7,
        Operation::StartProcessing => 8,
        Operation::SaveState => 9,
        Operation::StrictPrepare => 10,
        Operation::StrictMetadata => 11,
        Operation::StrictMidi => 12,
        Operation::StrictProcess => 13,
        Operation::StrictDispose => 14,
    }
}
fn read_operation(code: u8) -> Result<Operation, Error> {
    Ok(match code {
        1 => Operation::SourceLoad,
        2 => Operation::SourcePrepare,
        3 => Operation::OrdinaryProcess,
        4 => Operation::Reset,
        5 => Operation::ResetOrigin,
        6 => Operation::MidiPanic,
        7 => Operation::StopProcessing,
        8 => Operation::StartProcessing,
        9 => Operation::SaveState,
        10 => Operation::StrictPrepare,
        11 => Operation::StrictMetadata,
        12 => Operation::StrictMidi,
        13 => Operation::StrictProcess,
        14 => Operation::StrictDispose,
        _ => return Err(Error::UnknownCode),
    })
}
fn write_strict_kind(w: &mut Writer<'_>, kind: StrictKind) -> Result<(), Error> {
    let code = match kind {
        StrictKind::UnsupportedRequest => 1,
        StrictKind::UnsupportedReply => 2,
        StrictKind::InvalidRequest => 3,
        StrictKind::MalformedReply => 4,
        StrictKind::WrongReply => 5,
        StrictKind::Limit => 6,
        StrictKind::Shape => 7,
        StrictKind::Deadline => 8,
        StrictKind::Cancelled => 9,
        StrictKind::Allocation => 10,
        StrictKind::Io(_) => 11,
        StrictKind::Eof => 12,
        StrictKind::ChildExited => 13,
        StrictKind::ZeroWrite => 14,
        StrictKind::Unsolicited => 15,
        StrictKind::Terminal => 16,
        StrictKind::Busy => 17,
        StrictKind::Sigpipe => 18,
        StrictKind::Spawn => 19,
        StrictKind::OperationRejected => 20,
        StrictKind::ReportedCrash => 21,
        StrictKind::MetadataRejected(_) => 22,
        StrictKind::CleanupNotBegun => 23,
    };
    w.u8(code)?;
    match kind {
        StrictKind::Io(errno) => w.u32(errno as u32),
        StrictKind::MetadataRejected(reason) => w.u8(match reason {
            MetadataQueryRejection::Unsupported => 1,
            MetadataQueryRejection::PendingTopology => 2,
            MetadataQueryRejection::InconsistentTopology => 3,
        }),
        _ => Ok(()),
    }
}
fn read_strict_kind(r: &mut Reader<'_>) -> Result<StrictKind, Error> {
    Ok(match r.u8()? {
        1 => StrictKind::UnsupportedRequest,
        2 => StrictKind::UnsupportedReply,
        3 => StrictKind::InvalidRequest,
        4 => StrictKind::MalformedReply,
        5 => StrictKind::WrongReply,
        6 => StrictKind::Limit,
        7 => StrictKind::Shape,
        8 => StrictKind::Deadline,
        9 => StrictKind::Cancelled,
        10 => StrictKind::Allocation,
        11 => StrictKind::Io(r.u32()? as i32),
        12 => StrictKind::Eof,
        13 => StrictKind::ChildExited,
        14 => StrictKind::ZeroWrite,
        15 => StrictKind::Unsolicited,
        16 => StrictKind::Terminal,
        17 => StrictKind::Busy,
        18 => StrictKind::Sigpipe,
        19 => StrictKind::Spawn,
        20 => StrictKind::OperationRejected,
        21 => StrictKind::ReportedCrash,
        22 => StrictKind::MetadataRejected(match r.u8()? {
            1 => MetadataQueryRejection::Unsupported,
            2 => MetadataQueryRejection::PendingTopology,
            3 => MetadataQueryRejection::InconsistentTopology,
            _ => return Err(Error::UnknownCode),
        }),
        23 => StrictKind::CleanupNotBegun,
        _ => return Err(Error::UnknownCode),
    })
}
fn request_code(label: &str) -> Result<u8, Error> {
    Ok(match label {
        "LoadPlugin" => 1,
        "LoadState" => 2,
        "SaveState" => 3,
        "Process" => 4,
        "ProcessBuses" => 5,
        "GetParameter" => 6,
        "LatencySamples" => 7,
        "TailSamples" => 8,
        "SetProcessMode" => 9,
        "ResetOriginSupport" => 10,
        "StartProcessing" => 11,
        "StopProcessing" => 12,
        "NativeDirtyRevision" => 13,
        "GetAllParameters" => 14,
        "SetParameter" => 15,
        "SendMidiAt" => 16,
        "CheckedAudioBusLayout" => 17,
        "StrictMetadataSnapshot" => 18,
        "EventBusLayout" => 19,
        "SetBusActive" => 20,
        _ => return Err(Error::InvalidRequestContext),
    })
}
fn request_label(code: u8) -> Result<&'static str, Error> {
    Ok(match code {
        1 => "LoadPlugin",
        2 => "LoadState",
        3 => "SaveState",
        4 => "Process",
        5 => "ProcessBuses",
        6 => "GetParameter",
        7 => "LatencySamples",
        8 => "TailSamples",
        9 => "SetProcessMode",
        10 => "ResetOriginSupport",
        11 => "StartProcessing",
        12 => "StopProcessing",
        13 => "NativeDirtyRevision",
        14 => "GetAllParameters",
        15 => "SetParameter",
        16 => "SendMidiAt",
        17 => "CheckedAudioBusLayout",
        18 => "StrictMetadataSnapshot",
        19 => "EventBusLayout",
        20 => "SetBusActive",
        _ => return Err(Error::UnknownCode),
    })
}
fn write_request_context(w: &mut Writer<'_>, value: RequestContext) -> Result<(), Error> {
    let code = request_code(value.operation)?;
    w.u8(code)?;
    if code == 20 {
        w.u8(
            match value.media_type.ok_or(Error::InvalidRequestContext)? {
                MediaType::Audio => 1,
                MediaType::Event => 2,
            },
        )?;
        w.u8(match value.direction.ok_or(Error::InvalidRequestContext)? {
            BusDirection::Input => 1,
            BusDirection::Output => 2,
        })?;
        w.u32(value.bus_index.ok_or(Error::InvalidRequestContext)? as u32)
    } else if value.media_type.is_some() || value.direction.is_some() || value.bus_index.is_some() {
        Err(Error::InvalidRequestContext)
    } else {
        Ok(())
    }
}
fn read_request_context(r: &mut Reader<'_>) -> Result<RequestContext, Error> {
    let code = r.u8()?;
    let operation = request_label(code)?;
    let (media_type, direction, bus_index) = if code == 20 {
        (
            Some(match r.u8()? {
                1 => MediaType::Audio,
                2 => MediaType::Event,
                _ => return Err(Error::UnknownCode),
            }),
            Some(match r.u8()? {
                1 => BusDirection::Input,
                2 => BusDirection::Output,
                _ => return Err(Error::UnknownCode),
            }),
            Some(r.u32()? as i32),
        )
    } else {
        (None, None, None)
    };
    Ok(RequestContext {
        operation,
        media_type,
        direction,
        bus_index,
    })
}

// This explicit list is the entire native BridgeStats surface. Destructuring
// without '..' makes a newly added field a compile-time codec audit failure.
fn write_stats(w: &mut Writer<'_>, stats: &BridgeStats) -> Result<(), Error> {
    let BridgeStats {
        current_epoch,
        epoch_resets,
        worker_epoch_resets,
        dropped_old_epoch_inputs,
        dropped_old_epoch_outputs,
        reset_faults,
        callback_sequences,
        submitted,
        completed,
        input_overflows,
        input_gaps,
        deadline_misses,
        output_overflows,
        output_loss_epoch,
        output_too_small,
        frame_mismatches,
        stale_outputs,
        future_outputs_held,
        future_output_overflows,
        rt_overflows,
        inline_event_overflows,
        dropped_rt_events,
        admin_overflows,
        event_overflows,
        faults,
        latency_drift_blocks,
        latency_drift_outputs,
        latency_samples,
        tail_samples,
        detached_workers,
        shutdown_timeouts,
        stopped,
    } = stats;
    for value in [
        current_epoch,
        epoch_resets,
        worker_epoch_resets,
        dropped_old_epoch_inputs,
        dropped_old_epoch_outputs,
        reset_faults,
        callback_sequences,
        submitted,
        completed,
        input_overflows,
        input_gaps,
        deadline_misses,
        output_overflows,
        output_loss_epoch,
        output_too_small,
        frame_mismatches,
        stale_outputs,
        future_outputs_held,
        future_output_overflows,
        rt_overflows,
        inline_event_overflows,
        dropped_rt_events,
        admin_overflows,
        event_overflows,
        faults,
        latency_drift_blocks,
        latency_drift_outputs,
    ] {
        w.u64(*value)?;
    }
    w.u32(*latency_samples)?;
    w.u32(*tail_samples)?;
    w.u64(*detached_workers)?;
    w.u64(*shutdown_timeouts)?;
    w.bool(*stopped)
}
fn read_stats(r: &mut Reader<'_>) -> Result<BridgeStats, Error> {
    Ok(BridgeStats {
        current_epoch: r.u64()?,
        epoch_resets: r.u64()?,
        worker_epoch_resets: r.u64()?,
        dropped_old_epoch_inputs: r.u64()?,
        dropped_old_epoch_outputs: r.u64()?,
        reset_faults: r.u64()?,
        callback_sequences: r.u64()?,
        submitted: r.u64()?,
        completed: r.u64()?,
        input_overflows: r.u64()?,
        input_gaps: r.u64()?,
        deadline_misses: r.u64()?,
        output_overflows: r.u64()?,
        output_loss_epoch: r.u64()?,
        output_too_small: r.u64()?,
        frame_mismatches: r.u64()?,
        stale_outputs: r.u64()?,
        future_outputs_held: r.u64()?,
        future_output_overflows: r.u64()?,
        rt_overflows: r.u64()?,
        inline_event_overflows: r.u64()?,
        dropped_rt_events: r.u64()?,
        admin_overflows: r.u64()?,
        event_overflows: r.u64()?,
        faults: r.u64()?,
        latency_drift_blocks: r.u64()?,
        latency_drift_outputs: r.u64()?,
        latency_samples: r.u32()?,
        tail_samples: r.u32()?,
        detached_workers: r.u64()?,
        shutdown_timeouts: r.u64()?,
        stopped: r.bool()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(view: View<'_>) -> (u16, Vec<u8>) {
        let mut value = Vec::with_capacity(MAX_CONTROL_BYTES);
        let pointer = value.as_ptr();
        let capacity = value.capacity();
        let kind = encode(view, &mut value).unwrap();
        assert_eq!(value.as_ptr(), pointer);
        assert_eq!(value.capacity(), capacity);
        (kind, value)
    }
    fn failure() -> BackendFailure {
        let mut value = BackendFailure::new(BackendFailureKind::Deadline, "SDK失败；不是回滚");
        value.delivery = Some(BackendDelivery::MayHaveApplied);
        value.strict_kind = Some(StrictKind::Limit);
        value.strict_diagnostic_unavailable = Some(StrictKind::Io(-123));
        value.strict_stage = Some("render.metadata.tail_samples");
        value.last_wire_request = Some(RequestContext {
            operation: "StrictMetadataSnapshot",
            media_type: None,
            direction: None,
            bus_index: None,
        });
        value.strict_metadata_getter = Some(MetadataGetter::TailSamples);
        value.limit_evidence = Some(LimitEvidence {
            site: LimitSite::ParameterCatalogTextBytes,
            observed_at_least: 65537,
            maximum: 65536,
        });
        value
    }
    fn check_failure(observed: &Failure<'_>, expected: &BackendFailure) {
        assert_eq!(observed.kind, expected.kind);
        assert_eq!(observed.delivery, expected.delivery);
        assert_eq!(observed.diagnostic, expected.diagnostic());
        assert_eq!(observed.strict_kind, expected.strict_kind);
        assert_eq!(
            observed.strict_diagnostic_unavailable,
            expected.strict_diagnostic_unavailable
        );
        assert_eq!(observed.strict_stage, expected.strict_stage);
        assert_eq!(observed.last_wire_request, expected.last_wire_request);
        assert_eq!(
            observed.strict_metadata_getter,
            expected.strict_metadata_getter
        );
        assert_eq!(observed.limit_evidence, expected.limit_evidence);
    }
    fn identity() -> BounceSourceIdentity {
        BounceSourceIdentity {
            project_session: 3,
            project_fingerprint: 0,
            timeline_revision: 5,
            epoch: 7,
            endpoint_id: 11,
            channel_id: 13,
            instance_id: 17,
        }
    }

    #[test]
    fn typed_control_strict_failure_keeps_all_actual_fields_without_diagnostic_parsing() {
        let expected = failure();
        let outcome = CallOutcome::StrictError(expected.clone());
        let (kind, encoded) = bytes(View::Call {
            operation: Operation::StrictMetadata,
            boundary: Boundary::Returned,
            frames: None,
            transport: None,
            outcome: Some(OutcomeRef::from(&outcome)),
        });
        let Control::Call {
            outcome: Some(Outcome::StrictError(observed)),
            ..
        } = decode(&encoded, kind).unwrap()
        else {
            panic!("typed failure lost");
        };
        check_failure(&observed, &expected);
        let start = encoded.as_ptr() as usize;
        let end = start + encoded.len();
        let diagnostic = observed.diagnostic.as_ptr() as usize;
        assert!(diagnostic >= start && diagnostic + observed.diagnostic.len() <= end);
    }

    #[test]
    fn typed_control_all_strict_failure_kinds_round_trip_with_payloads() {
        let variants = [
            StrictKind::UnsupportedRequest,
            StrictKind::UnsupportedReply,
            StrictKind::InvalidRequest,
            StrictKind::MalformedReply,
            StrictKind::WrongReply,
            StrictKind::Limit,
            StrictKind::Shape,
            StrictKind::Deadline,
            StrictKind::Cancelled,
            StrictKind::Allocation,
            StrictKind::Io(i32::MIN),
            StrictKind::Io(i32::MAX),
            StrictKind::Eof,
            StrictKind::ChildExited,
            StrictKind::ZeroWrite,
            StrictKind::Unsolicited,
            StrictKind::Terminal,
            StrictKind::Busy,
            StrictKind::Sigpipe,
            StrictKind::Spawn,
            StrictKind::OperationRejected,
            StrictKind::ReportedCrash,
            StrictKind::MetadataRejected(MetadataQueryRejection::Unsupported),
            StrictKind::MetadataRejected(MetadataQueryRejection::PendingTopology),
            StrictKind::MetadataRejected(MetadataQueryRejection::InconsistentTopology),
            StrictKind::CleanupNotBegun,
        ];
        for variant in variants {
            let mut expected = failure();
            expected.strict_kind = Some(variant);
            expected.strict_diagnostic_unavailable = Some(variant);
            let (kind, encoded) = bytes(View::Disposal {
                terminated: false,
                error: Some(&expected),
                diagnostics: &[],
            });
            let Control::Disposal {
                error: Some(observed),
                ..
            } = decode(&encoded, kind).unwrap()
            else {
                panic!("failure absent");
            };
            check_failure(&observed, &expected);
        }
    }

    #[test]
    fn state_outcome_only_has_independent_wire_and_no_invented_call_fields() {
        let mut encoded = Vec::with_capacity(MAX_CONTROL_BYTES);
        let allocation = encoded.as_ptr();
        encode_outcome_only(OutcomeRef::Ok, &mut encoded, 7).unwrap();
        assert_eq!(encoded, b"NOUT\x01\x00\x01");
        assert_eq!(encoded.as_ptr(), allocation);
        assert_eq!(decode_outcome_only(&encoded), Ok(Outcome::Ok));
        encoded.clear();
        encode_outcome_only(
            OutcomeRef::Error("actual load error"),
            &mut encoded,
            MAX_CONTROL_BYTES,
        )
        .unwrap();
        assert_eq!(
            decode_outcome_only(&encoded),
            Ok(Outcome::Error("actual load error"))
        );
        assert!(decode(&encoded, 1).is_err());
    }

    #[test]
    fn state_outcome_only_preserves_complete_strict_failure_without_allocation() {
        let expected = failure();
        let mut encoded = Vec::with_capacity(MAX_CONTROL_BYTES);
        let pointer = encoded.as_ptr();
        encode_outcome_only(
            OutcomeRef::StrictError(&expected),
            &mut encoded,
            MAX_CONTROL_BYTES,
        )
        .unwrap();
        assert_eq!(encoded.as_ptr(), pointer);
        let Outcome::StrictError(observed) = decode_outcome_only(&encoded).unwrap() else {
            panic!("strict load failure lost");
        };
        check_failure(&observed, &expected);
        for end in 0..encoded.len() {
            assert!(decode_outcome_only(&encoded[..end]).is_err());
        }
        encoded.push(0);
        assert_eq!(decode_outcome_only(&encoded), Err(Error::TrailingBytes));
    }

    #[test]
    fn state_outcome_only_budget_and_preoccupied_buffer_fail_without_partial_success() {
        let mut encoded = Vec::with_capacity(MAX_CONTROL_BYTES);
        encode_outcome_only(OutcomeRef::Ok, &mut encoded, 6).unwrap_err();
        assert!(encoded.is_empty());
        encode_outcome_only(OutcomeRef::Ok, &mut encoded, MAX_CONTROL_BYTES + 1).unwrap_err();
        assert!(encoded.is_empty());
        encoded.extend_from_slice(b"retained");
        assert_eq!(
            encode_outcome_only(OutcomeRef::Ok, &mut encoded, 7),
            Err(Error::DestinationNotEmpty)
        );
        assert_eq!(encoded, b"retained");
    }

    #[test]
    fn typed_control_error_options_none_and_all_delivery_kinds_are_distinct() {
        for kind in [
            BackendFailureKind::Cancelled,
            BackendFailureKind::Deadline,
            BackendFailureKind::Backend,
        ] {
            for delivery in [
                None,
                Some(BackendDelivery::NotSent),
                Some(BackendDelivery::MayHaveApplied),
            ] {
                let mut expected = BackendFailure::new(kind, "");
                expected.delivery = delivery;
                let (kind, encoded) = bytes(View::Disposal {
                    terminated: false,
                    error: Some(&expected),
                    diagnostics: &[],
                });
                let Control::Disposal {
                    error: Some(observed),
                    ..
                } = decode(&encoded, kind).unwrap()
                else {
                    panic!("failure absent");
                };
                check_failure(&observed, &expected);
            }
        }
    }

    #[test]
    fn typed_control_all_closed_request_contexts_and_bus_operands_round_trip() {
        for code in 1..=20 {
            for (media, direction, bus) in [
                (MediaType::Audio, BusDirection::Input, -1),
                (MediaType::Event, BusDirection::Output, i32::MAX),
            ] {
                let mut expected = failure();
                expected.last_wire_request = Some(RequestContext {
                    operation: request_label(code).unwrap(),
                    media_type: (code == 20).then_some(media),
                    direction: (code == 20).then_some(direction),
                    bus_index: (code == 20).then_some(bus),
                });
                let (kind, encoded) = bytes(View::Disposal {
                    terminated: false,
                    error: Some(&expected),
                    diagnostics: &[],
                });
                let Control::Disposal {
                    error: Some(observed),
                    ..
                } = decode(&encoded, kind).unwrap()
                else {
                    panic!("failure absent");
                };
                check_failure(&observed, &expected);
            }
        }
    }

    #[test]
    fn typed_control_getter_and_limit_site_enums_preserve_first_refusal_numbers() {
        for getter in [
            MetadataGetter::CheckedAudioBusLayout,
            MetadataGetter::EventBusLayout,
            MetadataGetter::LatencySamples,
            MetadataGetter::TailSamples,
        ] {
            for site in [
                LimitSite::ParameterCatalogEntries,
                LimitSite::ParameterTextBytes,
                LimitSite::ParameterCatalogTextBytes,
            ] {
                let mut expected = failure();
                expected.strict_metadata_getter = Some(getter);
                expected.limit_evidence = Some(LimitEvidence {
                    site,
                    observed_at_least: usize::MAX,
                    maximum: 0,
                });
                let (kind, encoded) = bytes(View::Disposal {
                    terminated: true,
                    error: Some(&expected),
                    diagnostics: &[],
                });
                let Control::Disposal {
                    error: Some(observed),
                    ..
                } = decode(&encoded, kind).unwrap()
                else {
                    panic!("failure absent");
                };
                check_failure(&observed, &expected);
            }
        }
    }

    #[test]
    fn typed_control_all_call_operations_and_boundary_outcomes_round_trip() {
        for code in 1..=14 {
            let operation = read_operation(code).unwrap();
            let (kind, encoded) = bytes(View::Call {
                operation,
                boundary: Boundary::Attempt,
                frames: Some(0),
                transport: None,
                outcome: None,
            });
            assert_eq!(
                decode(&encoded, kind).unwrap(),
                Control::Call {
                    operation,
                    boundary: Boundary::Attempt,
                    frames: Some(0),
                    transport: None,
                    outcome: None
                }
            );
            for outcome in [
                CallOutcome::Ok,
                CallOutcome::Error("same text can denote any typed operation".into()),
            ] {
                let (kind, encoded) = bytes(View::Call {
                    operation,
                    boundary: Boundary::Returned,
                    frames: None,
                    transport: None,
                    outcome: Some(OutcomeRef::from(&outcome)),
                });
                let Control::Call {
                    operation: actual,
                    outcome: Some(actual_outcome),
                    ..
                } = decode(&encoded, kind).unwrap()
                else {
                    panic!("call absent");
                };
                assert_eq!(actual, operation);
                match (&outcome, actual_outcome) {
                    (CallOutcome::Ok, Outcome::Ok) => {}
                    (CallOutcome::Error(text), Outcome::Error(actual)) => assert_eq!(text, actual),
                    _ => panic!("outcome type changed"),
                }
            }
        }
    }

    #[test]
    fn typed_control_transport_preserves_nan_payload_signed_zero_and_integer_extremes() {
        let transport = PluginTransport {
            sample_position: i64::MIN,
            quarter_note_position: f64::from_bits(0x7ff8_0000_0000_1234),
            tempo: -0.0,
            playing: true,
            time_sig_numerator: i32::MIN,
            time_sig_denominator: i32::MAX,
        };
        let (kind, encoded) = bytes(View::Call {
            operation: Operation::OrdinaryProcess,
            boundary: Boundary::Attempt,
            frames: Some(usize::MAX),
            transport: Some(&transport),
            outcome: None,
        });
        let Control::Call {
            frames: Some(frames),
            transport: Some(actual),
            ..
        } = decode(&encoded, kind).unwrap()
        else {
            panic!("transport absent");
        };
        assert_eq!(frames, usize::MAX);
        assert_eq!(actual, TransportBits::from(&transport));
    }

    #[test]
    fn typed_control_attempt_outcome_and_missing_return_are_rejected() {
        let mut destination = Vec::with_capacity(MAX_CONTROL_BYTES);
        for (boundary, outcome) in [
            (Boundary::Attempt, Some(OutcomeRef::Ok)),
            (Boundary::Returned, None),
        ] {
            assert_eq!(
                encode(
                    View::Call {
                        operation: Operation::SourceLoad,
                        boundary,
                        frames: None,
                        transport: None,
                        outcome
                    },
                    &mut destination
                ),
                Err(Error::InvalidCall)
            );
            assert!(destination.is_empty());
        }
        let (kind, mut encoded) = bytes(View::Call {
            operation: Operation::SourceLoad,
            boundary: Boundary::Attempt,
            frames: None,
            transport: None,
            outcome: None,
        });
        encoded[9] = 2;
        assert_eq!(decode(&encoded, kind), Err(Error::InvalidCall));
    }

    #[test]
    fn typed_control_worker_guard_retains_none_vs_some_zero_and_all_operands() {
        let operands = WorkerCaptureGuard {
            expected_epoch: 3,
            active_epoch: 4,
            requested_epoch: 5,
            input_empty: false,
            parameter_admission: u32::MAX,
            accepted: false,
        };
        for worker_id in [None, Some(0), Some(usize::MAX)] {
            let (kind, encoded) = bytes(View::WorkerCaptureGuard {
                worker_id,
                operands: &operands,
            });
            assert_eq!(
                decode(&encoded, kind).unwrap(),
                Control::WorkerCaptureGuard {
                    worker_id,
                    operands
                }
            );
        }
    }

    #[test]
    fn typed_control_join_panic_never_becomes_healthy_join() {
        for outcome in [WorkerJoinOutcome::Joined, WorkerJoinOutcome::WorkerPanicked] {
            let (kind, encoded) = bytes(View::WorkerJoin {
                worker_id: 913,
                outcome,
            });
            assert_eq!(
                decode(&encoded, kind).unwrap(),
                Control::WorkerJoin {
                    worker_id: 913,
                    outcome
                }
            );
        }
    }

    #[test]
    fn typed_control_worker_preserves_every_stats_field_and_physical_manifest_slot() {
        let stats = BridgeStats {
            current_epoch: 1,
            epoch_resets: 2,
            worker_epoch_resets: 3,
            dropped_old_epoch_inputs: 4,
            dropped_old_epoch_outputs: 5,
            reset_faults: 6,
            callback_sequences: 7,
            submitted: 8,
            completed: 9,
            input_overflows: 10,
            input_gaps: 11,
            deadline_misses: 12,
            output_overflows: 13,
            output_loss_epoch: 14,
            output_too_small: 15,
            frame_mismatches: 16,
            stale_outputs: 17,
            future_outputs_held: 18,
            future_output_overflows: 19,
            rt_overflows: 20,
            inline_event_overflows: 21,
            dropped_rt_events: 22,
            admin_overflows: 23,
            event_overflows: 24,
            faults: 25,
            latency_drift_blocks: 26,
            latency_drift_outputs: 27,
            latency_samples: 28,
            tail_samples: 29,
            detached_workers: 30,
            shutdown_timeouts: u64::MAX,
            stopped: true,
        };
        for manifest in [
            PluginEndpointManifest::identified(&[71, 73]).unwrap(),
            PluginEndpointManifest::unknown_for_slots(2).unwrap(),
        ] {
            let (kind, encoded) = bytes(View::Worker {
                site: "actual guard before join",
                worker_id: 81,
                manifest: &manifest,
                epoch: 82,
                sequence: Some(0),
                stats: &stats,
            });
            let Control::Worker {
                stats: actual,
                manifest: actual_manifest,
                sequence,
                ..
            } = decode(&encoded, kind).unwrap()
            else {
                panic!("worker absent");
            };
            assert_eq!(actual, stats);
            assert_eq!(sequence, Some(0));
            assert_eq!(actual_manifest.slot_count, manifest.slot_count);
            assert_eq!(actual_manifest.identified, manifest.is_identified());
            assert_eq!(actual_manifest.instance_ids, manifest.instance_ids);
        }
    }

    #[test]
    fn typed_control_worker_manifest_bound_is_checked_on_both_sides() {
        let mut manifest = PluginEndpointManifest::unknown_for_slots(1).unwrap();
        manifest.slot_count = MAX_PLUGIN_CHAIN_SLOTS + 1;
        let stats = BridgeStats::default();
        let mut destination = Vec::with_capacity(MAX_CONTROL_BYTES);
        assert_eq!(
            encode(
                View::Worker {
                    site: "",
                    worker_id: 1,
                    manifest: &manifest,
                    epoch: 0,
                    sequence: None,
                    stats: &stats
                },
                &mut destination
            ),
            Err(Error::Bound)
        );
        assert!(destination.is_empty());
        manifest.slot_count = 1;
        let (kind, mut encoded) = bytes(View::Worker {
            site: "",
            worker_id: 1,
            manifest: &manifest,
            epoch: 0,
            sequence: None,
            stats: &stats,
        });
        encoded[20..28].copy_from_slice(&((MAX_PLUGIN_CHAIN_SLOTS + 1) as u64).to_le_bytes());
        assert_eq!(decode(&encoded, kind), Err(Error::Bound));
    }

    #[test]
    fn typed_control_disposal_retains_error_and_all_diagnostic_strings() {
        let expected = failure();
        let diagnostics = vec!["first error".into(), "second\nerror".into(), "声音".into()];
        let (kind, encoded) = bytes(View::Disposal {
            terminated: false,
            error: Some(&expected),
            diagnostics: &diagnostics,
        });
        let Control::Disposal {
            terminated,
            error: Some(observed),
            diagnostics: actual,
        } = decode(&encoded, kind).unwrap()
        else {
            panic!("disposal absent");
        };
        assert!(!terminated);
        check_failure(&observed, &expected);
        assert_eq!(
            actual.values(),
            &diagnostics.iter().map(String::as_str).collect::<Vec<_>>()
        );
    }

    #[test]
    fn typed_control_disposal_diagnostic_count_and_text_bounds_are_explicit() {
        let excessive = vec![String::new(); MAX_DISPOSAL_DIAGNOSTICS + 1];
        let mut destination = Vec::with_capacity(MAX_CONTROL_BYTES);
        assert_eq!(
            encode(
                View::Disposal {
                    terminated: true,
                    error: None,
                    diagnostics: &excessive
                },
                &mut destination
            ),
            Err(Error::Bound)
        );
        let excessive_text = vec!["x".repeat(MAX_TEXT + 1)];
        assert_eq!(
            encode(
                View::Disposal {
                    terminated: true,
                    error: None,
                    diagnostics: &excessive_text
                },
                &mut destination
            ),
            Err(Error::Bound)
        );
        assert!(destination.is_empty());
    }

    #[test]
    fn typed_control_source_identity_zero_fingerprint_and_render_peak_bits_are_lossless() {
        let expected = identity();
        let (kind, encoded) = bytes(View::StrictCaptureIdentity(&expected));
        assert_eq!(
            decode(&encoded, kind).unwrap(),
            Control::StrictCaptureIdentity(expected)
        );
        let report = RenderReport {
            identity: expected,
            origin_frame: 19,
            musical_frames: 23,
            frames: 29,
            native_latency: 31,
            reported_tail: u32::MAX,
            fixed_tail: 37,
            generated_frames: 41,
            sample_peak: f32::from_bits(0x7fc0_0137),
        };
        let (kind, encoded) = bytes(View::RenderReport(&report));
        let Control::RenderReport(actual) = decode(&encoded, kind).unwrap() else {
            panic!("render absent");
        };
        assert_eq!(actual.identity, expected);
        assert_eq!(actual.origin_frame, 19);
        assert_eq!(actual.musical_frames, 23);
        assert_eq!(actual.frames, 29);
        assert_eq!(actual.native_latency, 31);
        assert_eq!(actual.reported_tail, u32::MAX);
        assert_eq!(actual.fixed_tail, 37);
        assert_eq!(actual.generated_frames, 41);
        assert_eq!(actual.sample_peak_bits, 0x7fc0_0137);
    }

    #[test]
    fn typed_control_ordinary_state_retains_length_epoch_and_all_receipt_domains() {
        for epoch in [None, Some(0), Some(89)] {
            let (kind, encoded) = bytes(View::OrdinaryState {
                endpoint_id: 43,
                project_session: 47,
                instance_id: 53,
                slot: 3,
                request_id: 59,
                epoch,
                byte_len: 591772,
            });
            assert_eq!(
                decode(&encoded, kind).unwrap(),
                Control::OrdinaryState {
                    endpoint_id: 43,
                    project_session: 47,
                    instance_id: 53,
                    slot: 3,
                    request_id: 59,
                    epoch,
                    byte_len: 591772
                }
            );
        }
    }

    #[test]
    fn typed_control_reset_report_does_not_drop_discard_or_fault_evidence() {
        let (kind, encoded) = bytes(View::ResetReport {
            frames: 128,
            discarded_prior_midi_events: 61,
            discarded_midi_events: 67,
            discarded_prior_parameter_points: 71,
            discarded_parameter_points: 73,
            output_events_lost: true,
            parameter_output_fault: false,
            process_error: Some("reset failure retained"),
        });
        assert_eq!(
            decode(&encoded, kind).unwrap(),
            Control::ResetReport {
                frames: 128,
                discarded_prior_midi_events: 61,
                discarded_midi_events: 67,
                discarded_prior_parameter_points: 71,
                discarded_parameter_points: 73,
                output_events_lost: true,
                parameter_output_fault: false,
                process_error: Some("reset failure retained")
            }
        );
    }

    #[test]
    fn typed_control_hook_active_and_lifecycle_site_are_exact_borrowed_evidence() {
        for inactive in [false, true] {
            let (kind, encoded) = bytes(View::HookInventory {
                site: "actual worker guard",
                inactive,
            });
            assert_eq!(
                decode(&encoded, kind).unwrap(),
                Control::HookInventory {
                    site: "actual worker guard",
                    inactive
                }
            );
        }
        let (kind, encoded) = bytes(View::Lifecycle("backend destructor returned"));
        assert_eq!(
            decode(&encoded, kind).unwrap(),
            Control::Lifecycle("backend destructor returned")
        );
    }

    #[test]
    fn typed_control_every_truncated_failure_body_and_trailing_suffix_are_rejected() {
        let expected = failure();
        let (kind, encoded) = bytes(View::Disposal {
            terminated: true,
            error: Some(&expected),
            diagnostics: &["full final diagnostic".into()],
        });
        for end in 0..encoded.len() {
            assert!(
                decode(&encoded[..end], kind).is_err(),
                "truncated prefix {end} accepted"
            );
        }
        let mut extra = encoded;
        extra.push(0);
        assert_eq!(decode(&extra, kind), Err(Error::TrailingBytes));
    }

    #[test]
    fn typed_control_magic_version_kind_unknown_enum_and_boolean_are_not_opaque_success() {
        let (kind, encoded) = bytes(View::HookInventory {
            site: "",
            inactive: true,
        });
        for (offset, value, expected) in [
            (0, 0, Error::InvalidMagic),
            (4, 2, Error::UnknownVersion),
            (12, 2, Error::InvalidBoolean),
        ] {
            let mut mutated = encoded.clone();
            mutated[offset] = value;
            assert_eq!(decode(&mutated, kind), Err(expected));
        }
        assert_eq!(decode(&encoded, kind + 1), Err(Error::KindMismatch));
        let mut unknown = encoded.clone();
        unknown[6..8].copy_from_slice(&u16::MAX.to_le_bytes());
        assert_eq!(decode(&unknown, u16::MAX), Err(Error::UnknownCode));
        let (kind, mut join) = bytes(View::WorkerJoin {
            worker_id: 1,
            outcome: WorkerJoinOutcome::Joined,
        });
        join[16] = 3;
        assert_eq!(decode(&join, kind), Err(Error::UnknownCode));
    }

    #[test]
    fn typed_control_invalid_utf8_option_flag_text_length_and_byte_cap_are_rejected() {
        let (kind, mut encoded) = bytes(View::Lifecycle("x"));
        encoded[12] = 0xff;
        assert_eq!(decode(&encoded, kind), Err(Error::InvalidUtf8));
        encoded[8..12].copy_from_slice(&((MAX_TEXT + 1) as u32).to_le_bytes());
        assert_eq!(decode(&encoded, kind), Err(Error::Bound));
        let operands = WorkerCaptureGuard {
            expected_epoch: 0,
            active_epoch: 0,
            requested_epoch: 0,
            input_empty: true,
            parameter_admission: 0,
            accepted: false,
        };
        let (kind, mut option) = bytes(View::WorkerCaptureGuard {
            worker_id: None,
            operands: &operands,
        });
        option[8] = 2;
        assert_eq!(decode(&option, kind), Err(Error::InvalidOption));
        assert_eq!(
            decode(&vec![0; MAX_CONTROL_BYTES + 1], 23),
            Err(Error::Bound)
        );
    }

    #[test]
    fn typed_control_destination_contract_never_reallocates_or_leaves_partial_failure() {
        let mut tiny = Vec::with_capacity(9);
        let pointer = tiny.as_ptr();
        let capacity = tiny.capacity();
        assert_eq!(
            encode(View::Lifecycle("cannot fit"), &mut tiny),
            Err(Error::DestinationCapacity)
        );
        assert!(tiny.is_empty());
        assert_eq!(tiny.as_ptr(), pointer);
        assert_eq!(tiny.capacity(), capacity);
        let mut excessive = Vec::with_capacity(MAX_CONTROL_BYTES + 1);
        assert_eq!(
            encode(View::Lifecycle(""), &mut excessive),
            Err(Error::DestinationCapacity)
        );
        let mut occupied = vec![0x19];
        assert_eq!(
            encode(View::Lifecycle(""), &mut occupied),
            Err(Error::DestinationNotEmpty)
        );
        assert_eq!(occupied, vec![0x19]);
        let mut destination = Vec::with_capacity(MAX_CONTROL_BYTES);
        assert_eq!(
            encode(View::Lifecycle(&"x".repeat(MAX_TEXT + 1)), &mut destination),
            Err(Error::Bound)
        );
        assert!(destination.is_empty());
    }

    #[test]
    fn typed_control_caller_budget_keeps_full_failure_or_rejects_the_entire_body() {
        let expected = failure();
        let view = || View::Call {
            operation: Operation::StrictMetadata,
            boundary: Boundary::Returned,
            frames: Some(0),
            transport: None,
            outcome: Some(OutcomeRef::StrictError(&expected)),
        };
        let (kind, original) = bytes(view());
        let mut limited = Vec::with_capacity(MAX_CONTROL_BYTES);
        let pointer = limited.as_ptr();
        let capacity = limited.capacity();
        assert_eq!(
            encode_with_limit(view(), &mut limited, original.len()),
            Ok(kind)
        );
        assert_eq!(limited, original);
        let Control::Call {
            frames,
            outcome: Some(Outcome::StrictError(observed)),
            ..
        } = decode(&limited, kind).unwrap()
        else {
            panic!("bounded failure lost");
        };
        assert_eq!(frames, Some(0));
        check_failure(&observed, &expected);
        assert_eq!(limited.as_ptr(), pointer);
        assert_eq!(limited.capacity(), capacity);
        limited.clear();
        assert_eq!(
            encode_with_limit(view(), &mut limited, original.len() - 1),
            Err(Error::Bound)
        );
        assert!(limited.is_empty());
        assert_eq!(limited.as_ptr(), pointer);
        assert_eq!(limited.capacity(), capacity);
    }

    #[test]
    fn typed_control_budget_rejects_invalid_or_header_only_limits_without_allocation() {
        let mut destination = Vec::with_capacity(MAX_CONTROL_BYTES);
        let pointer = destination.as_ptr();
        let capacity = destination.capacity();
        for budget in [0, 7, MAX_CONTROL_BYTES + 1, usize::MAX] {
            assert_eq!(
                encode_with_limit(View::Lifecycle(""), &mut destination, budget),
                Err(Error::Bound)
            );
            assert!(destination.is_empty());
            assert_eq!(destination.as_ptr(), pointer);
            assert_eq!(destination.capacity(), capacity);
        }
        destination.extend_from_slice(&[0x19, 0x23]);
        assert_eq!(
            encode_with_limit(View::Lifecycle(""), &mut destination, usize::MAX),
            Err(Error::DestinationNotEmpty)
        );
        assert_eq!(destination, [0x19, 0x23]);
    }

    #[test]
    fn typed_control_unknown_or_incomplete_request_context_refuses_before_publication() {
        for context in [
            RequestContext {
                operation: "arbitrary unknown operation",
                media_type: None,
                direction: None,
                bus_index: None,
            },
            RequestContext {
                operation: "SetBusActive",
                media_type: None,
                direction: Some(BusDirection::Output),
                bus_index: Some(0),
            },
            RequestContext {
                operation: "StrictMetadataSnapshot",
                media_type: Some(MediaType::Audio),
                direction: None,
                bus_index: None,
            },
        ] {
            let mut expected = failure();
            expected.last_wire_request = Some(context);
            let mut destination = Vec::with_capacity(MAX_CONTROL_BYTES);
            assert_eq!(
                encode(
                    View::Disposal {
                        terminated: false,
                        error: Some(&expected),
                        diagnostics: &[]
                    },
                    &mut destination
                ),
                Err(Error::InvalidRequestContext)
            );
            assert!(destination.is_empty());
        }
    }

    #[test]
    fn typed_control_unimplemented_state_and_app_kinds_are_explicit_refusals() {
        for kind in [2_u16, 4, 5, 6, 7, 8, 9, 10, 11, 12, 15] {
            let mut encoded = Vec::new();
            encoded.extend_from_slice(MAGIC);
            encoded.extend_from_slice(&VERSION.to_le_bytes());
            encoded.extend_from_slice(&kind.to_le_bytes());
            assert_eq!(decode(&encoded, kind), Err(Error::UnsupportedKind(kind)));
        }
        let runtime = RecordKind::RuntimeEvent {
            endpoint_id: 1,
            project_session: 2,
            instance_id: 3,
            event: crate::plugins::plugin_runtime::RuntimeEvent::ShutdownComplete,
        };
        assert!(matches!(
            view_record(&runtime),
            Err(Error::UnsupportedKind(15))
        ));
        let state = RecordKind::StateLoad {
            bytes: vec![0xaa],
            outcome: CallOutcome::Ok,
        };
        assert!(matches!(
            view_record(&state),
            Err(Error::UnsupportedKind(2))
        ));
    }
    #[test]
    fn typed_control_base_read_preserves_raw_f64_bits_and_error_type() {
        for raw_bits in [
            Ok(0x7ff8_0000_0000_0041),
            Ok((-0.0_f64).to_bits()),
            Err("actual getter rejected"),
        ] {
            let (kind, encoded) = bytes(View::BaseRead {
                site: "actual parameter base read",
                id: 4093,
                raw_bits,
            });
            assert_eq!(
                decode(&encoded, kind).unwrap(),
                Control::BaseRead {
                    site: "actual parameter base read",
                    id: 4093,
                    raw_bits
                }
            );
        }
    }
    #[test]
    fn typed_control_nested_failure_request_and_operation_codes_have_no_unknown_fallback() {
        for code in [0, 15, 255] {
            assert_eq!(read_operation(code), Err(Error::UnknownCode));
        }
        for wire in [&[0][..], &[24][..], &[255][..], &[22, 0][..], &[22, 4][..]] {
            assert_eq!(
                read_strict_kind(&mut Reader {
                    bytes: wire,
                    position: 0
                }),
                Err(Error::UnknownCode)
            );
        }
        for wire in [
            &[0][..],
            &[21][..],
            &[255][..],
            &[20, 3, 1, 0, 0, 0, 0][..],
            &[20, 1, 3, 0, 0, 0, 0][..],
        ] {
            assert_eq!(
                read_request_context(&mut Reader {
                    bytes: wire,
                    position: 0
                }),
                Err(Error::UnknownCode)
            );
        }
        for wire in [&[0][..], &[4][..]] {
            assert_eq!(
                read_outcome(&mut Reader {
                    bytes: wire,
                    position: 0
                }),
                Err(Error::UnknownCode)
            );
        }
    }
}
