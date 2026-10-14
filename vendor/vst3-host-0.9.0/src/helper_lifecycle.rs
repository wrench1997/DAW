//! Shared retained-helper evidence used by ordinary default-class probes and
//! experimental strict clients. Transport, delivery and cleanup facts have one
//! definition and conversion; neither API guesses reap from process absence.

use crate::audio::{BusDirection, MediaType};
use crate::discovery::ClassCompatibility;
use crate::fresh_helper::{self, LimitEvidence, RequestView};
use crate::process_isolation::{HostResponse, MetadataGetter};
use crate::MetadataQueryRejection;
use std::process::ExitStatus;

/// Whether request bytes could have reached the helper. No value proves rollback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// No request byte was written.
    NotSent,
    /// At least one request byte was written; even a reported error may have applied.
    MayHaveApplied,
}

/// Closed transport/operation categories, without string-prefix interpretation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// Request is outside the closed subset.
    UnsupportedRequest,
    /// Response contains an unsupported payload, such as output events.
    UnsupportedReply,
    /// Request scalars are invalid.
    InvalidRequest,
    /// Response syntax or required fields are invalid.
    MalformedReply,
    /// Response variant does not match this request.
    WrongReply,
    /// A checked count, length or payload budget was exceeded.
    Limit,
    /// Exact audio shape differs.
    Shape,
    /// The original absolute exchange deadline elapsed.
    Deadline,
    /// The registered sticky cancellation was observed.
    Cancelled,
    /// A checked allocation failed.
    Allocation,
    /// OS error, preserving errno on Linux or GetLastError on Windows.
    Io(i32),
    /// The response pipe closed unexpectedly.
    Eof,
    /// The direct child was observed exited.
    ChildExited,
    /// A write returned zero.
    ZeroWrite,
    /// Unexpected extra response bytes were observed.
    Unsolicited,
    /// The session is permanently sealed.
    Terminal,
    /// The global strict-child reservation is occupied.
    Busy,
    /// The supported ignored-SIGPIPE host precondition failed.
    Sigpipe,
    /// Required child setup failed.
    Spawn,
    /// A fully validated helper Error; this never implies rollback.
    OperationRejected,
    /// A fully validated helper Crashed response.
    ReportedCrash,
    /// A fully validated owner-only metadata refusal.
    MetadataRejected(MetadataQueryRejection),
    /// Nonwaiting cleanup was requested before disposal closed its pipes.
    CleanupNotBegun,
}

/// Fixed context of the exchange that returned a failure. This is an attempted
/// wire request, not proof of delivery; `Failure::delivery` remains authoritative.
/// No plugin path, state, parameter value, title or caller borrow is retained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestContext {
    /// Closed request discriminator; this always points to a fixed static label.
    pub operation: &'static str,
    /// SetBusActive's media type, and None for every other request.
    pub media_type: Option<MediaType>,
    /// SetBusActive's direction, and None for every other request.
    pub direction: Option<BusDirection>,
    /// SetBusActive's bus index, and None for every other request.
    pub bus_index: Option<i32>,
}
impl RequestContext {
    pub(crate) fn from_request(request: RequestView<'_>) -> Self {
        let operation = match request {
            RequestView::ProbeFactory { .. } => "ProbeFactory",
            RequestView::LoadPlugin { .. } => "LoadPlugin",
            RequestView::LoadState { .. } => "LoadState",
            RequestView::SaveState => "SaveState",
            RequestView::Process { .. } => "Process",
            RequestView::ProcessBuses { .. } => "ProcessBuses",
            RequestView::GetParameter { .. } => "GetParameter",
            RequestView::LatencySamples => "LatencySamples",
            RequestView::TailSamples => "TailSamples",
            RequestView::SetProcessMode { .. } => "SetProcessMode",
            RequestView::ResetOriginSupport => "ResetOriginSupport",
            RequestView::StartProcessing => "StartProcessing",
            RequestView::StopProcessing => "StopProcessing",
            RequestView::NativeDirtyRevision => "NativeDirtyRevision",
            RequestView::GetAllParameters => "GetAllParameters",
            RequestView::SetParameter { .. } => "SetParameter",
            RequestView::SendMidiAt { .. } => "SendMidiAt",
            RequestView::CheckedAudioBusLayout => "CheckedAudioBusLayout",
            RequestView::StrictMetadataSnapshot { .. } => "StrictMetadataSnapshot",
            RequestView::EventBusLayout => "EventBusLayout",
            RequestView::SetBusActive {
                media_type,
                direction,
                bus_index,
                ..
            } => {
                return Self {
                    operation: "SetBusActive",
                    media_type: Some(media_type),
                    direction: Some(direction),
                    bus_index: Some(bus_index),
                }
            }
        };
        Self {
            operation,
            media_type: None,
            direction: None,
            bus_index: None,
        }
    }
}

/// Bounded failure evidence; no command, caller borrow, pipe or owner can escape.
#[derive(Debug)]
pub struct Failure {
    /// Original observed failure category.
    pub kind: FailureKind,
    /// Conservative delivery evidence.
    pub delivery: Delivery,
    /// Bounded helper Error/Crashed message, if its diagnostic copy succeeded.
    pub diagnostic: Option<String>,
    /// Diagnostic-copy failure, without erasing the known helper failure.
    pub diagnostic_unavailable: Option<FailureKind>,
    /// The failing exchange's fixed request context, independent of helper text.
    /// Local rejection, spawn and cleanup failures have None, so an earlier
    /// successful query can never be misrepresented as the failed exchange.
    pub request_context: Option<RequestContext>,
    /// Validated actual getter within a bundle, never inferred from transport faults.
    pub metadata_getter: Option<MetadataGetter>,
    /// First checked catalog limit prefix, without assuming the complete size.
    pub limit_evidence: Option<LimitEvidence>,
}

impl From<fresh_helper::ErrorKind> for FailureKind {
    fn from(value: fresh_helper::ErrorKind) -> Self {
        use fresh_helper::ErrorKind as K;
        match value {
            K::UnsupportedRequest => Self::UnsupportedRequest,
            K::UnsupportedReply => Self::UnsupportedReply,
            K::InvalidRequest => Self::InvalidRequest,
            K::MalformedReply => Self::MalformedReply,
            K::WrongReply => Self::WrongReply,
            K::Limit => Self::Limit,
            K::Shape => Self::Shape,
            K::Deadline => Self::Deadline,
            K::Cancelled => Self::Cancelled,
            K::Allocation => Self::Allocation,
            K::Io(n) => Self::Io(n),
            K::Eof => Self::Eof,
            K::ChildExited => Self::ChildExited,
            K::ZeroWrite => Self::ZeroWrite,
            K::Unsolicited => Self::Unsolicited,
            K::Terminal => Self::Terminal,
            K::Busy => Self::Busy,
            K::Sigpipe => Self::Sigpipe,
            K::Spawn => Self::Spawn,
            K::OperationRejected => Self::OperationRejected,
            K::ReportedCrash => Self::ReportedCrash,
            K::MetadataRejected(reason) => Self::MetadataRejected(reason),
            K::CleanupNotBegun => Self::CleanupNotBegun,
        }
    }
}
impl From<fresh_helper::Delivery> for Delivery {
    fn from(value: fresh_helper::Delivery) -> Self {
        match value {
            fresh_helper::Delivery::NotSent => Self::NotSent,
            fresh_helper::Delivery::MayHaveApplied => Self::MayHaveApplied,
        }
    }
}
impl From<fresh_helper::ExchangeFailure> for Failure {
    fn from(value: fresh_helper::ExchangeFailure) -> Self {
        Self {
            kind: value.cause.into(),
            delivery: value.delivery.into(),
            diagnostic: value.reported.and_then(|reply| match *reply {
                HostResponse::Error { message }
                | HostResponse::Crashed { message }
                | HostResponse::StrictMetadataFailed { message, .. } => Some(message),
                _ => None, // Partial/error audio can never become successful output.
            }),
            diagnostic_unavailable: value.diagnostic_unavailable.map(Into::into),
            request_context: None,
            metadata_getter: value.metadata_getter,
            limit_evidence: value.limit_evidence,
        }
    }
}
pub(crate) fn unsent(kind: fresh_helper::ErrorKind) -> Failure {
    Failure {
        kind: kind.into(),
        delivery: Delivery::NotSent,
        diagnostic: None,
        diagnostic_unavailable: None,
        request_context: None,
        metadata_getter: None,
        limit_evidence: None,
    }
}

/// Cancellation-only authority. Its shared wake FD remains owned after disposal.
#[derive(Clone)]
pub struct CancelHandle(pub(crate) fresh_helper::CancelHandle);
impl CancelHandle {
    /// Publish sticky cancellation and attempt a bounded nonblocking wake.
    pub fn cancel(&self) {
        self.0.cancel();
    }
}

/// Checked cleanup state. Only Reaped proves direct-child resource completion.
#[derive(Debug, Clone, Copy)]
pub enum CleanupState {
    /// Actual try_wait status, cached after the child owner is released.
    Reaped(ExitStatus),
    /// The same child remains owned and blocks another strict spawn.
    Pending,
    /// No abandoned child exists; a live client may still own the global slot.
    Empty,
    /// A live owner or another cleanup observation owns the global reservation.
    Busy,
}

/// Bounded first cleanup evidence remains available across continuation/reap.
#[derive(Debug)]
#[must_use = "Pending/Busy cannot authorize render publication or another clone"]
pub struct CleanupObservation {
    /// Concrete reap or retained-ownership state.
    pub state: CleanupState,
    /// First Shutdown failure with original delivery; cancellation remains typed.
    pub shutdown_failure: Option<Failure>,
    /// First failed kill OS error, retained even if a later kill succeeds.
    pub kill_error: Option<i32>,
    /// First failed wait OS error, retained even after eventual reap.
    pub wait_error: Option<i32>,
    /// First pipe cancellation/completion OS error, retained after eventual reap.
    pub pipe_error: Option<i32>,
    /// Whether a force-kill request succeeded; this is not proof of reap.
    pub kill_requested: bool,
}
impl From<fresh_helper::CleanupReport> for CleanupObservation {
    fn from(value: fresh_helper::CleanupReport) -> Self {
        Self {
            state: match value.state {
                fresh_helper::CleanupState::Reaped(s) => CleanupState::Reaped(s),
                fresh_helper::CleanupState::Pending => CleanupState::Pending,
                fresh_helper::CleanupState::Empty => CleanupState::Empty,
                fresh_helper::CleanupState::Busy => CleanupState::Busy,
            },
            shutdown_failure: value.shutdown_error.map(|kind| Failure {
                kind: kind.into(),
                delivery: value
                    .shutdown_delivery
                    .unwrap_or(fresh_helper::Delivery::NotSent)
                    .into(),
                diagnostic: None,
                diagnostic_unavailable: None,
                request_context: None,
                metadata_getter: None,
                limit_evidence: None,
            }),
            kill_error: value.kill_error,
            wait_error: value.wait_error,
            pipe_error: value.pipe_error,
            kill_requested: value.kill_requested,
        }
    }
}

/// Bounded, un-clamped PluginInfo fields returned by the helper.
#[derive(Debug)]
pub struct LoadedPlugin {
    /// Reported vendor.
    pub vendor: String,
    /// Reported name.
    pub name: String,
    /// Reported version.
    pub version: String,
    /// Reported category.
    pub category: String,
    /// Actual instantiated class identifier.
    pub uid: String,
    /// Reported editor capability; the closed client has no editor operations.
    pub has_gui: bool,
    /// Reported input buses; checked layouts are separate owner queries.
    pub audio_inputs: i32,
    /// Reported output buses.
    pub audio_outputs: i32,
    /// Reported total output channels.
    pub output_channels: i32,
    /// Reported event input capability.
    pub has_midi_input: bool,
    /// Reported event output capability.
    pub has_midi_output: bool,
    /// Bounded current/retired class mappings.
    pub compatibility: Vec<ClassCompatibility>,
}

impl LoadedPlugin {
    pub(crate) fn from_plugin_info(reply: HostResponse) -> Self {
        match reply {
            HostResponse::PluginInfo {
                vendor,
                name,
                version,
                category,
                uid,
                has_gui,
                audio_inputs,
                audio_outputs,
                output_channels,
                has_midi_input,
                has_midi_output,
                compatibility,
            } => Self {
                vendor,
                name,
                version,
                category,
                uid,
                has_gui,
                audio_inputs,
                audio_outputs,
                output_channels,
                has_midi_input,
                has_midi_output,
                compatibility,
            },
            _ => unreachable!("closed plugin-info shape"),
        }
    }
}
