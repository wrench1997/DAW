//! Disabled-by-default Linux client for a single disposable offline generator.
//!
//! This closed development API has no App caller. One process-wide reservation
//! admits at most one strict child, including abandoned or pending-cleanup owners.
//! Every exchange uses its caller's original absolute deadline and is never
//! replayed. Deadlines are cooperative: allocation, spawn, scheduling and reap
//! are not universal hard-time guarantees. The host must keep SIGPIPE ignored;
//! this module checks that precondition and never installs a signal handler.
//!
//! Variable in-flight exchange payload is charged up to 256 MiB; caller storage,
//! previously returned replies, allocator overhead and fixed error envelopes are
//! outside that bound. The supported codec rejects nonempty output-event arrays.
//! Audio bits are preserved, including nonfinite values; renderer policy must
//! validate all returned samples before publishing anything.

use crate::discovery::ClassCompatibility;
use crate::fresh_helper::{self, FreshHelperSession, ReplyShape, RequestView, StateContextView};
use crate::midi::MidiEvent;
use crate::plugin::{
    CheckedAudioBusLayout, EventBusLayout, ProcessMode, ProcessTransport, ResetOriginSupport,
};
use crate::process_isolation::HostResponse;
use crate::{MetadataQueryRejection, Parameter};
use std::process::{Command, ExitStatus};
use std::time::Instant;

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
    /// OS error, preserving errno.
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
                HostResponse::Error { message } | HostResponse::Crashed { message } => {
                    Some(message)
                }
                _ => None, // Partial/error audio can never become successful output.
            }),
            diagnostic_unavailable: value.diagnostic_unavailable.map(Into::into),
        }
    }
}
fn unsent(kind: fresh_helper::ErrorKind) -> Failure {
    Failure {
        kind: kind.into(),
        delivery: Delivery::NotSent,
        diagnostic: None,
        diagnostic_unavailable: None,
    }
}

/// Cancellation-only authority. Its shared wake FD remains owned after disposal.
#[derive(Clone)]
pub struct CancelHandle(fresh_helper::CancelHandle);
impl CancelHandle {
    /// Publish sticky cancellation and attempt a bounded nonblocking wake.
    pub fn cancel(&self) {
        self.0.cancel();
    }
}

/// Still-owning spawn failure. Store the cleanup client before returning failure.
#[must_use = "a returned cleanup client retains the process-wide reservation"]
pub struct SpawnFailure {
    /// Original setup/admission failure, before any LoadPlugin request.
    pub failure: Failure,
    /// The actual child owner when spawn succeeded but setup failed.
    pub cleanup: Option<Box<Client>>,
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
    /// First failed kill errno, retained even if a later kill succeeds.
    pub kill_error: Option<i32>,
    /// First failed wait/poll errno, retained even after eventual reap.
    pub wait_error: Option<i32>,
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
            }),
            kill_error: value.kill_error,
            wait_error: value.wait_error,
            kill_requested: value.kill_requested,
        }
    }
}

/// Borrowed load data. The initial configured maximum is fixed at 2048 frames.
#[derive(Debug, Clone, Copy)]
pub struct LoadSpec<'a> {
    /// Exact resolved plugin bundle path.
    pub path: &'a str,
    /// Requested sample rate.
    pub sample_rate: f64,
    /// Requested transport tempo.
    pub tempo: f64,
    /// Requested meter numerator.
    pub time_sig_numerator: i32,
    /// Requested meter denominator.
    pub time_sig_denominator: i32,
    /// Exact current class identifier, never an automatic fallback.
    pub class_id: &'a str,
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

/// Exact Q128 output, available only after a successful strict exchange.
#[derive(Debug)]
pub struct AudioBlock {
    /// Exact expected mono/stereo channels, each containing 128 raw samples.
    pub outputs: Vec<Vec<f32>>,
    /// Supported responses have zero events; nonempty arrays fail the exchange.
    pub output_event_count: usize,
    /// Preserve explicit output loss evidence for renderer policy.
    pub output_events_lost: bool,
    /// Preserve whether this request's explicit transport was applied.
    pub transport_applied: bool,
}

/// Compare controller readback at deliberately captured f32 precision. This is
/// neither a clamp/tolerance nor proof that an SDK Process applied the parameter.
pub fn captured_base_matches(raw: f64, captured: f32) -> bool {
    raw.is_finite()
        && (0.0..=1.0).contains(&raw)
        && captured.is_finite()
        && (0.0..=1.0).contains(&captured)
        && raw as f32 == captured
}

/// Main/owner-thread strict session. No Clone, Send, Sync, raw owner or generic
/// command interface. Dropping transfers unresolved ownership to quarantine.
#[must_use = "complete checked disposal; Drop is not evidence of reap"]
pub struct Client {
    session: FreshHelperSession,
}
impl Client {
    /// Spawn exactly once, retaining any post-spawn failure owner. Required stdin
    /// and stdout pipes and inherited stderr override the supplied Command stdio.
    /// Other executable/argument/environment policy belongs to the future caller.
    pub fn spawn(command: &mut Command, deadline: Instant) -> Result<Self, SpawnFailure> {
        FreshHelperSession::spawn(command, deadline)
            .map(|session| Self { session })
            .map_err(|failure| SpawnFailure {
                failure: unsent(failure.cause),
                cleanup: failure
                    .cleanup
                    .map(|session| Box::new(Self { session: *session })),
            })
    }
    /// Obtain cancellation-only authority before the first load exchange.
    pub fn cancellation(&self) -> CancelHandle {
        CancelHandle(self.session.cancellation())
    }
    fn exchange(
        &mut self,
        request: RequestView<'_>,
        shape: ReplyShape,
        deadline: Instant,
    ) -> Result<HostResponse, Failure> {
        self.session
            .exchange_view(request, shape, deadline)
            .map_err(Into::into)
    }
    fn success(&mut self, request: RequestView<'_>, deadline: Instant) -> Result<(), Failure> {
        match self.exchange(request, ReplyShape::Success, deadline)? {
            HostResponse::Success { .. } => Ok(()),
            _ => unreachable!("strict codec enforces expected success"),
        }
    }
    /// Load the chosen class with configured maximum2048; never retry or recover.
    pub fn load_plugin(
        &mut self,
        spec: LoadSpec<'_>,
        deadline: Instant,
    ) -> Result<LoadedPlugin, Failure> {
        if spec.path.is_empty()
            || spec.class_id.len() != 32
            || !spec.class_id.bytes().all(|b| b.is_ascii_hexdigit())
            || !spec.sample_rate.is_finite()
            || spec.sample_rate <= 0.0
            || !spec.tempo.is_finite()
            || spec.tempo <= 0.0
            || spec.time_sig_numerator <= 0
            || !matches!(spec.time_sig_denominator, 1 | 2 | 4 | 8 | 16)
        {
            return Err(self
                .session
                .reject_request(fresh_helper::ErrorKind::InvalidRequest, deadline)
                .into());
        }
        let request = RequestView::LoadPlugin {
            path: spec.path,
            sample_rate: spec.sample_rate,
            block_size: 2048,
            tempo: spec.tempo,
            time_sig_numerator: spec.time_sig_numerator,
            time_sig_denominator: spec.time_sig_denominator,
            class_id: Some(spec.class_id),
        };
        match self.exchange(request, ReplyShape::PluginInfo, deadline)? {
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
            } => Ok(LoadedPlugin {
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
            }),
            _ => unreachable!("strict plugin-info shape"),
        }
    }
    /// Set Offline mode on the helper owner before preparation/start.
    pub fn set_offline(&mut self, deadline: Instant) -> Result<(), Failure> {
        self.success(
            RequestView::SetProcessMode {
                mode: ProcessMode::Offline,
            },
            deadline,
        )
    }
    /// Query configured reset capability/status; this never processes reset audio.
    pub fn reset_origin_support(
        &mut self,
        deadline: Instant,
    ) -> Result<ResetOriginSupport, Failure> {
        match self.exchange(
            RequestView::ResetOriginSupport,
            ReplyShape::ResetOriginSupport,
            deadline,
        )? {
            HostResponse::ResetOriginSupport { support } => Ok(support),
            _ => unreachable!("strict reset support shape"),
        }
    }
    /// Borrow opaque project state synchronously without an eager payload copy.
    pub fn load_state(&mut self, data: &[u8], deadline: Instant) -> Result<(), Failure> {
        self.success(
            RequestView::LoadState {
                data,
                context: StateContextView::Project,
            },
            deadline,
        )
    }
    /// Start prepared processing through the helper's existing owner path.
    pub fn start_processing(&mut self, deadline: Instant) -> Result<(), Failure> {
        self.success(RequestView::StartProcessing, deadline)
    }
    /// Stop a healthy clone. A sealed/cancelled session cannot send this request.
    pub fn stop_processing(&mut self, deadline: Instant) -> Result<(), Failure> {
        self.success(RequestView::StopProcessing, deadline)
    }
    /// Read dirty revision evidence; this alone is not an enforced edit fence.
    pub fn native_dirty_revision(&mut self, deadline: Instant) -> Result<u64, Failure> {
        match self.exchange(
            RequestView::NativeDirtyRevision,
            ReplyShape::NativeDirtyRevision,
            deadline,
        )? {
            HostResponse::NativeDirtyRevision { revision } => Ok(revision),
            _ => unreachable!("strict revision shape"),
        }
    }
    /// Read a bounded catalog retaining every exposed Parameter field.
    pub fn parameters(&mut self, deadline: Instant) -> Result<Vec<Parameter>, Failure> {
        match self.exchange(
            RequestView::GetAllParameters,
            ReplyShape::Parameters,
            deadline,
        )? {
            HostResponse::Parameters { params } => Ok(params),
            _ => unreachable!("strict catalog shape"),
        }
    }
    /// Set one finite normalized captured f32 base through the existing owner path.
    pub fn set_parameter(&mut self, id: u32, value: f32, deadline: Instant) -> Result<(), Failure> {
        self.success(
            RequestView::SetParameter {
                id,
                value: f64::from(value),
            },
            deadline,
        )
    }
    /// Read the raw controller scalar; caller checks captured precision explicitly.
    pub fn parameter(&mut self, id: u32, deadline: Instant) -> Result<f64, Failure> {
        match self.exchange(
            RequestView::GetParameter { id },
            ReplyShape::ParameterValue,
            deadline,
        )? {
            HostResponse::ParameterValue { value } => Ok(value),
            _ => unreachable!("strict parameter shape"),
        }
    }
    /// Read actual/prepared audio layout. Raw main/aux and unknown activation remain.
    pub fn checked_audio_layout(
        &mut self,
        deadline: Instant,
    ) -> Result<CheckedAudioBusLayout, Failure> {
        match self.exchange(
            RequestView::CheckedAudioBusLayout,
            ReplyShape::CheckedAudioBusLayout,
            deadline,
        )? {
            HostResponse::CheckedAudioBusLayout { layout } => Ok(layout),
            _ => unreachable!("strict checked audio shape"),
        }
    }
    /// Read actual event buses on the helper owner thread.
    pub fn event_layout(&mut self, deadline: Instant) -> Result<EventBusLayout, Failure> {
        match self.exchange(
            RequestView::EventBusLayout,
            ReplyShape::EventBusLayout,
            deadline,
        )? {
            HostResponse::EventBusLayout { layout } => Ok(layout),
            _ => unreachable!("strict event layout shape"),
        }
    }
    /// Fresh owner-thread latency read, with no zero fallback.
    pub fn latency_samples(&mut self, deadline: Instant) -> Result<u32, Failure> {
        match self.exchange(
            RequestView::LatencySamples,
            ReplyShape::LatencySamples,
            deadline,
        )? {
            HostResponse::LatencySamples { samples } => Ok(samples),
            _ => unreachable!("strict latency shape"),
        }
    }
    /// Fresh owner-thread tail read, retaining legitimate u32::MAX.
    pub fn tail_samples(&mut self, deadline: Instant) -> Result<u32, Failure> {
        match self.exchange(RequestView::TailSamples, ReplyShape::TailSamples, deadline)? {
            HostResponse::TailSamples { samples } => Ok(samples),
            _ => unreachable!("strict tail shape"),
        }
    }
    /// Admit only ordinary NoteOn/NoteOff at offset0..127. Other MIDI is rejected.
    pub fn send_note(
        &mut self,
        event: MidiEvent,
        sample_offset: i32,
        deadline: Instant,
    ) -> Result<(), Failure> {
        self.success(
            RequestView::SendMidiAt {
                event,
                sample_offset,
            },
            deadline,
        )
    }
    /// Process one Q128 generator block without participating input channels.
    /// Finite/audio-loss acceptance remains a caller policy; no buffers are padded.
    pub fn process(
        &mut self,
        output_channels: usize,
        transport: ProcessTransport,
        deadline: Instant,
    ) -> Result<AudioBlock, Failure> {
        self.process_silent_generator(0, output_channels, transport, deadline)
    }
    /// Process one Q128 dry-generator block with explicit transport and exact shape.
    /// Accepts only input counts 0..=2 and output counts 1..=2. Each participating input
    /// is the same immutable positive-zero Q128 channel; caller audio cannot enter.
    /// Finite/audio-loss acceptance remains a caller policy; no buffers are padded.
    ///
    /// ```compile_fail
    /// use std::time::Instant;
    /// use vst3_host::{ProcessTransport, strict_offline::Client};
    /// fn cannot_supply_audio(client: &mut Client, samples: &[Vec<f32>],
    ///                       transport: ProcessTransport, deadline: Instant) {
    ///     client.process_silent_generator(samples, 2, transport, deadline);
    /// }
    /// ```
    pub fn process_silent_generator(
        &mut self,
        input_channels: usize,
        output_channels: usize,
        transport: ProcessTransport,
        deadline: Instant,
    ) -> Result<AudioBlock, Failure> {
        // Shape validation remains inside the budgeted session, sealing invalid
        // requests with NotSent and preserving deadline/cancellation precedence.
        if input_channels > 2 || !matches!(output_channels, 1 | 2) {
            return Err(self
                .session
                .reject_request(fresh_helper::ErrorKind::Shape, deadline)
                .into());
        }
        match self.exchange(
            RequestView::silent_generator(input_channels, transport),
            ReplyShape::Audio {
                channels: output_channels,
                frames: 128,
            },
            deadline,
        )? {
            HostResponse::AudioOutput {
                outputs,
                output_events,
                output_events_lost,
                transport_applied,
            } => Ok(AudioBlock {
                outputs,
                output_event_count: output_events.len(),
                output_events_lost,
                transport_applied,
            }),
            _ => unreachable!("strict audio shape"),
        }
    }
    /// Seal admission, optionally send one healthy Shutdown before the earlier
    /// protocol/cleanup deadline, close pipes, then observe/kill/reap within the
    /// independent cleanup deadline. Prior terminal exchanges send no protocol.
    pub fn dispose(
        &mut self,
        protocol_deadline: Instant,
        cleanup_deadline: Instant,
    ) -> CleanupObservation {
        self.session
            .dispose_with_deadlines(protocol_deadline, cleanup_deadline)
            .into()
    }
    /// After disposal began, perform one nonwaiting observation/eligible kill.
    /// Never renew grace or send protocol; successful kill alone remains Pending.
    pub fn continue_cleanup(&mut self) -> Result<CleanupObservation, Failure> {
        self.session
            .continue_cleanup()
            .map(Into::into)
            .map_err(unsent)
    }
    /// Recover retained abandoned ownership under an absolute cleanup deadline.
    /// This never sends Shutdown, and blocks any new strict child until reap.
    pub fn cleanup_abandoned(deadline: Instant) -> CleanupObservation {
        FreshHelperSession::cleanup_abandoned(deadline).into()
    }
}
