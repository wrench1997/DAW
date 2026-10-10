//! Owned messages and borrowed ports for an opt-in, library-owned scoped worker.
//!
//! A session keeps the plug-in owner on its control thread. Its callback receives only an
//! attached-editor bridge and a single-credit worker port. Neither port exposes an owner,
//! processor, raw SDK interface, channel endpoint, thread scope, or join handle. Audio replies
//! return the submitted allocations even on failure; failed output is diagnostic data, not a
//! successfully processed block. This bounded transport is not a real-time scheduling claim.

use crate::midi::PluginEvent;
use crate::{AudioBuffers, BusAudioBuffers, Error, MidiChannel, ProcessTransport};
use std::{
    cell::Cell,
    fmt,
    marker::PhantomData,
    rc::Rc,
    sync::{
        atomic::{AtomicU8, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
        Arc,
    },
};

/// The ordinary MIDI messages admitted by a scoped worker.
///
/// Mapped MIDI, parameter changes, batches, and lifecycle operations are deliberately absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrdinaryMidiEvent {
    /// Start a note.
    NoteOn {
        /// MIDI channel.
        channel: MidiChannel,
        /// MIDI note number, validated as 0 through 127 by the worker.
        note: u8,
        /// Velocity, validated as 0 through 127 by the worker.
        velocity: u8,
    },
    /// Release a note.
    NoteOff {
        /// MIDI channel.
        channel: MidiChannel,
        /// MIDI note number, validated as 0 through 127 by the worker.
        note: u8,
        /// Release velocity, validated as 0 through 127 by the worker.
        velocity: u8,
    },
    /// Apply pressure to one note.
    PolyAftertouch {
        /// MIDI channel.
        channel: MidiChannel,
        /// MIDI note number, validated as 0 through 127 by the worker.
        note: u8,
        /// Pressure, validated as 0 through 127 by the worker.
        pressure: u8,
    },
}

impl OrdinaryMidiEvent {
    pub(crate) fn into_midi_event(self) -> crate::MidiEvent {
        match self {
            Self::NoteOn {
                channel,
                note,
                velocity,
            } => crate::MidiEvent::NoteOn {
                channel,
                note,
                velocity,
            },
            Self::NoteOff {
                channel,
                note,
                velocity,
            } => crate::MidiEvent::NoteOff {
                channel,
                note,
                velocity,
            },
            Self::PolyAftertouch {
                channel,
                note,
                pressure,
            } => crate::MidiEvent::PolyAftertouch {
                channel,
                note,
                pressure,
            },
        }
    }
}

/// One owned operation submitted to the scoped worker.
#[derive(Debug)]
pub enum WorkerCommand {
    /// Process flat audio buffers, optionally setting transport immediately before processing.
    Process {
        /// Owned input and output allocations returned in the reply, including on failure.
        buffers: AudioBuffers,
        /// Optional authoritative transport for this block.
        transport: Option<ProcessTransport>,
    },
    /// Process bus-aware audio buffers, optionally setting transport immediately beforehand.
    ProcessBuses {
        /// Owned input and output buses returned in the reply, including on failure.
        buffers: BusAudioBuffers,
        /// Optional authoritative transport for this block.
        transport: Option<ProcessTransport>,
    },
    /// Admit one ordinary MIDI event to the pending input queue.
    SendMidi {
        /// The note or pressure message to admit.
        event: OrdinaryMidiEvent,
        /// Sample offset within the next block.
        sample_offset: i32,
    },
    /// Drain owned output events and their native event-loss indication.
    TakeOutputEvents,
}

/// The owned reply corresponding to the single outstanding worker command.
///
/// A worker failure is also retained independently until the private worker has joined.
/// Receiving or ignoring a failure reply cannot turn the session into a success.
#[derive(Debug)]
pub enum WorkerReply {
    /// A flat process attempt, with the original owned allocations.
    Processed {
        /// Original buffers. On failure their contents are partial diagnostic data only.
        buffers: AudioBuffers,
        /// Processing outcome, preserving the worker's original shared error.
        result: std::result::Result<(), Arc<Error>>,
    },
    /// A bus-aware process attempt, with the original owned allocations.
    ProcessedBuses {
        /// Original buffers. On failure their contents are partial diagnostic data only.
        buffers: BusAudioBuffers,
        /// Processing outcome, preserving the worker's original shared error.
        result: std::result::Result<(), Arc<Error>>,
    },
    /// Queue admission outcome for one ordinary MIDI message.
    MidiAdmission {
        /// Admission outcome; success does not mean that an audio block has processed it.
        result: std::result::Result<(), Arc<Error>>,
    },
    /// Drained output events and the runtime's actual event-loss indication.
    OutputEvents {
        /// Events owned by the caller after this reply is received.
        events: Vec<PluginEvent>,
        /// Native event loss. Transport failures are never converted into this flag.
        lost: bool,
    },
}

impl WorkerReply {
    fn failed(&self) -> bool {
        match self {
            Self::Processed { result, .. }
            | Self::ProcessedBuses { result, .. }
            | Self::MidiAdmission { result } => result.is_err(),
            Self::OutputEvents { .. } => false,
        }
    }
}

/// The closed set of requests supported for an already attached editor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AttachedUiRequest {
    /// Read the size of the attached view without creating a new view.
    AttachedSize,
    /// Query whether the attached view supports host-driven resizing.
    CanResize,
    /// Resize the attached view, subject to its constraints.
    Resize {
        /// Requested width in the view's coordinate system.
        width: i32,
        /// Requested height in the view's coordinate system.
        height: i32,
    },
    /// Set the attached view's content scale.
    SetScale {
        /// Logical-to-physical scale factor.
        factor: f32,
    },
    /// Take the latest resize request made by the attached view.
    TakeResizeRequest,
    /// Service platform editor run-loop work on the control thread.
    ServiceRunLoop,
}

/// An owned response from the attached-editor dispatcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachedUiReply {
    /// Current attached-view width and height.
    AttachedSize((i32, i32)),
    /// Whether host-driven resizing is supported.
    CanResize(bool),
    /// Width and height accepted by the view after resizing.
    Resized((i32, i32)),
    /// Whether the view accepted the content-scale request.
    ScaleAccepted(bool),
    /// The latest requested width and height, if a request was pending.
    ResizeRequest(Option<(i32, i32)>),
    /// Platform run-loop servicing completed.
    RunLoopServiced,
}

const CLOSED: u8 = 1;
const TERMINAL: u8 = 2;

/// Readiness only: this contains no plug-in or editor metadata.
pub(crate) struct WorkerStatus {
    flags: AtomicU8,
}

impl WorkerStatus {
    pub(crate) fn new() -> Self {
        Self {
            flags: AtomicU8::new(0),
        }
    }

    pub(crate) fn mark_closed(&self) {
        self.flags.fetch_or(CLOSED, Ordering::Release);
    }

    pub(crate) fn mark_terminal(&self) {
        self.flags.fetch_or(CLOSED | TERMINAL, Ordering::Release);
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.flags.load(Ordering::Acquire) & CLOSED != 0
    }

    pub(crate) fn is_terminal(&self) -> bool {
        self.flags.load(Ordering::Acquire) & TERMINAL != 0
    }
}

/// A main-thread-only, borrowed bridge to the already attached editor.
///
/// The bridge cannot be cloned, sent to another thread, or extracted from its session. It
/// contains no plug-in owner or native interface. Every request requires exclusive access.
pub struct AttachedUiBridge<'session> {
    dispatch:
        &'session mut (dyn FnMut(AttachedUiRequest) -> crate::Result<AttachedUiReply> + 'session),
    status: Arc<WorkerStatus>,
    _session: PhantomData<fn(&'session mut ()) -> &'session mut ()>,
    _main_thread_only: PhantomData<Rc<()>>,
}

impl<'session> AttachedUiBridge<'session> {
    pub(crate) fn new(
        dispatch: &'session mut (dyn FnMut(AttachedUiRequest) -> crate::Result<AttachedUiReply>
                           + 'session),
        status: Arc<WorkerStatus>,
    ) -> Self {
        Self {
            dispatch,
            status,
            _session: PhantomData,
            _main_thread_only: PhantomData,
        }
    }

    /// Perform one attached-editor request on the session's control thread.
    ///
    /// A closed or terminal worker session rejects requests before calling the dispatcher.
    /// Dispatcher errors are returned unchanged, allowing the callback to handle them.
    pub fn request(&mut self, request: AttachedUiRequest) -> crate::Result<AttachedUiReply> {
        if self.status.is_closed() || self.status.is_terminal() {
            return Err(Error::Other("attached UI bridge is closed".into()));
        }
        (self.dispatch)(request)
    }
}

/// Why a command was not admitted to the worker transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandRejectionReason {
    /// The previous command's reply has not yet been consumed.
    Pending,
    /// The session has been explicitly sealed.
    Closed,
    /// A sticky terminal worker failure has been observed.
    Terminal,
    /// The bounded command slot is unexpectedly occupied.
    Full,
    /// The private worker no longer owns the command receiver.
    Disconnected,
}

/// A transport rejection that returns the exact original, unsubmitted command.
#[derive(Debug)]
pub struct CommandRejection {
    /// Why this submission did not occur.
    pub reason: CommandRejectionReason,
    /// The original owned command, including any audio allocations.
    pub command: WorkerCommand,
}

impl fmt::Display for CommandRejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "scoped worker command rejected: {:?}", self.reason)
    }
}

impl std::error::Error for CommandRejection {}

/// A failure to receive the outstanding command's reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyError {
    /// No command currently owns the port's single outstanding credit.
    NoPending,
    /// The worker's reply endpoint disconnected before a reply could be obtained.
    Disconnected,
}

impl fmt::Display for ReplyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoPending => f.write_str("no scoped worker reply is pending"),
            Self::Disconnected => f.write_str("scoped worker reply channel disconnected"),
        }
    }
}

impl std::error::Error for ReplyError {}

/// A session-branded, single-credit client port for the library-owned worker.
///
/// This port is `Send`, but is neither `Sync` nor `Clone`. Both channels have capacity one.
/// One submitted command owns the credit until its reply is consumed; the worker taking a
/// command from its queue does not release that credit. There is no channel extraction API.
/// Dropping the port closes its unique command sender.
pub struct WorkerIo<'session> {
    command: Option<SyncSender<WorkerCommand>>,
    reply: Receiver<WorkerReply>,
    pending: bool,
    status: Arc<WorkerStatus>,
    _session: PhantomData<fn(&'session mut ()) -> &'session mut ()>,
    _exclusive: PhantomData<Cell<()>>,
}

impl<'session> WorkerIo<'session> {
    pub(crate) fn channel(
        status: Arc<WorkerStatus>,
    ) -> (Self, Receiver<WorkerCommand>, SyncSender<WorkerReply>) {
        let (command, commands) = mpsc::sync_channel(1);
        let (replies, reply) = mpsc::sync_channel(1);
        (
            Self {
                command: Some(command),
                reply,
                pending: false,
                status,
                _session: PhantomData,
                _exclusive: PhantomData,
            },
            commands,
            replies,
        )
    }

    /// Attempt to submit one command without blocking.
    ///
    /// Every rejection returns the original command. A pending reply, a sealed session, or a
    /// sticky terminal failure prevents submission before the command channel is accessed.
    pub fn try_submit(
        &mut self,
        command: WorkerCommand,
    ) -> std::result::Result<(), CommandRejection> {
        let reason = if self.pending {
            Some(CommandRejectionReason::Pending)
        } else if self.status.is_terminal() {
            Some(CommandRejectionReason::Terminal)
        } else if self.status.is_closed() || self.command.is_none() {
            Some(CommandRejectionReason::Closed)
        } else {
            None
        };
        if let Some(reason) = reason {
            return Err(CommandRejection { reason, command });
        }
        let Some(sender) = self.command.as_ref() else {
            return Err(CommandRejection {
                reason: CommandRejectionReason::Closed,
                command,
            });
        };
        match sender.try_send(command) {
            Ok(()) => {
                self.pending = true;
                Ok(())
            }
            Err(TrySendError::Full(command)) => {
                self.status.mark_terminal();
                Err(CommandRejection {
                    reason: CommandRejectionReason::Full,
                    command,
                })
            }
            Err(TrySendError::Disconnected(command)) => {
                self.status.mark_terminal();
                Err(CommandRejection {
                    reason: CommandRejectionReason::Disconnected,
                    command,
                })
            }
        }
    }

    /// Try to receive the outstanding reply without blocking.
    ///
    /// `Ok(None)` means the command is still outstanding. Only consuming a reply releases the
    /// credit. A terminal status does not hide a failure reply already queued by the worker.
    pub fn try_reply(&mut self) -> std::result::Result<Option<WorkerReply>, ReplyError> {
        if !self.pending {
            return Err(ReplyError::NoPending);
        }
        match self.reply.try_recv() {
            Ok(reply) => Ok(Some(self.consume_reply(reply))),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => {
                self.status.mark_terminal();
                Err(ReplyError::Disconnected)
            }
        }
    }

    /// Wait for the outstanding reply, releasing the credit only when a reply is received.
    ///
    /// This may block the calling thread. Hosts that must continue servicing their editor
    /// should poll [`Self::try_reply`] and service their attached UI between polls instead.
    pub fn recv_reply(&mut self) -> std::result::Result<WorkerReply, ReplyError> {
        if !self.pending {
            return Err(ReplyError::NoPending);
        }
        match self.reply.recv() {
            Ok(reply) => Ok(self.consume_reply(reply)),
            Err(_) => {
                self.status.mark_terminal();
                Err(ReplyError::Disconnected)
            }
        }
    }

    fn consume_reply(&mut self, reply: WorkerReply) -> WorkerReply {
        self.pending = false;
        if reply.failed() {
            self.status.mark_terminal();
        }
        reply
    }

    /// Report whether a submitted command still owns the port's credit.
    pub fn has_pending(&mut self) -> bool {
        self.pending
    }

    /// Report whether the session has been sealed or a terminal failure was observed.
    pub fn is_closed(&mut self) -> bool {
        self.status.is_closed()
    }

    /// Report whether a terminal worker or transport failure has been observed.
    pub fn is_terminal(&mut self) -> bool {
        self.status.is_terminal()
    }

    pub(crate) fn close(&mut self) {
        self.status.mark_closed();
        self.command.take();
    }

    // Called only after the private worker has joined. A missing reply does not release a
    // pending credit, so the driver can distinguish an abandoned operation from consumption.
    pub(crate) fn take_unconsumed_reply(&mut self) -> Option<WorkerReply> {
        self.reply
            .try_recv()
            .ok()
            .map(|reply| self.consume_reply(reply))
    }
}

impl Drop for WorkerIo<'_> {
    fn drop(&mut self) {
        self.close();
    }
}

/// A callback that can borrow both ports only within the library-owned session.
///
/// The higher-ranked session lifetime prevents returning or retaining either branded port.
/// The library retains responsibility for sealing submission and joining the private worker.
pub type ScopedWorkerCallback<'callback> = dyn for<'session> FnMut(
        &mut AttachedUiBridge<'session>,
        &mut WorkerIo<'session>,
    ) -> crate::Result<()>
    + 'callback;

/// Successful completion of a joined scoped worker session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScopedWorkerReport {
    /// Number of commands completed by the worker before it joined.
    pub completed_commands: usize,
}

/// An independently retained worker failure, bounded to one outcome per session.
#[derive(Debug)]
pub enum WorkerOutcome {
    /// The original SDK or processing-kernel error, shared with any failure reply.
    Failed(Arc<Error>),
    /// The private worker panicked; no panic payload or thread authority is exposed.
    Panicked {
        /// An exact kernel error recorded before that Rust panic, if any.
        prior_error: Option<Arc<Error>>,
    },
    /// The worker could not publish its owned reply to the client port.
    ReplyUndelivered,
}

/// The primary cause of unsuccessful scoped-session completion.
#[derive(Debug)]
pub enum ScopedWorkerFailure {
    /// Session validation rejected entry; retains the original error.
    Rejected(Error),
    /// Worker creation failed; retains the original error.
    Spawn(Error),
    /// The client callback returned an error; always primary after the worker joins.
    Body(Error),
    /// An SDK or processing-kernel operation failed; retains the original shared error.
    Worker(Arc<Error>),
    /// The private worker panicked.
    WorkerPanicked,
    /// The bounded worker transport failed.
    BridgeFailed,
    /// The callback returned without consuming an outstanding worker reply.
    UnconsumedReply,
}

impl fmt::Display for ScopedWorkerFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rejected(error) => write!(f, "scoped worker session rejected: {error}"),
            Self::Spawn(error) => write!(f, "scoped worker spawn failed: {error}"),
            Self::Body(error) => write!(f, "scoped worker callback failed: {error}"),
            Self::Worker(error) => write!(f, "scoped worker operation failed: {error}"),
            Self::WorkerPanicked => f.write_str("scoped worker panicked"),
            Self::BridgeFailed => f.write_str("scoped worker bridge failed"),
            Self::UnconsumedReply => f.write_str("scoped worker reply was not consumed"),
        }
    }
}

impl std::error::Error for ScopedWorkerFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Rejected(error) | Self::Spawn(error) | Self::Body(error) => Some(error),
            Self::Worker(error) => Some(error.as_ref()),
            Self::WorkerPanicked | Self::BridgeFailed | Self::UnconsumedReply => None,
        }
    }
}

/// Joined-session failure with a preserved primary cause and bounded secondary evidence.
///
/// A callback error stays primary even if the worker also fails. Its original error is never
/// converted into a string or replaced by the worker outcome. An original callback panic is
/// resumed by the driver after join instead of being represented here.
#[derive(Debug)]
pub struct ScopedWorkerError {
    /// The primary failure, with the callback's original error taking precedence after join.
    pub primary: ScopedWorkerFailure,
    /// At most one independently retained worker failure from the joined worker ledger.
    pub secondary_worker: Option<WorkerOutcome>,
    /// An unconsumed owned reply, retaining original audio allocations when one is available.
    /// Recovered audio is diagnostic only, even if the individual command reported success.
    pub unconsumed_reply: Option<WorkerReply>,
}

impl fmt::Display for ScopedWorkerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.primary, f)
    }
}

impl std::error::Error for ScopedWorkerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.primary)
    }
}

/// Result of completing and joining a scoped worker session.
pub type ScopedWorkerResult = std::result::Result<ScopedWorkerReport, ScopedWorkerError>;

#[cfg(test)]
mod tests {
    use super::*;

    fn ports() -> (
        WorkerIo<'static>,
        Receiver<WorkerCommand>,
        SyncSender<WorkerReply>,
    ) {
        WorkerIo::channel(Arc::new(WorkerStatus::new()))
    }

    fn output_reply() -> WorkerReply {
        WorkerReply::OutputEvents {
            events: Vec::new(),
            lost: false,
        }
    }

    fn audio_command() -> (WorkerCommand, *const f32) {
        let mut buffers = AudioBuffers::new(1, 1, 8, 48_000.0);
        buffers.inputs[0][0] = 0.25;
        let allocation = buffers.inputs[0].as_ptr();
        (
            WorkerCommand::Process {
                buffers,
                transport: None,
            },
            allocation,
        )
    }

    fn assert_original_audio(command: WorkerCommand, allocation: *const f32) {
        let WorkerCommand::Process { buffers, transport } = command else {
            panic!("expected original process command");
        };
        assert_eq!(buffers.inputs[0].as_ptr(), allocation);
        assert_eq!(buffers.inputs[0][0], 0.25);
        assert!(transport.is_none());
    }

    #[test]
    fn worker_port_and_owned_messages_are_send() {
        fn assert_send<T: Send>() {}
        assert_send::<WorkerIo<'static>>();
        assert_send::<WorkerCommand>();
        assert_send::<WorkerReply>();
    }

    #[test]
    fn receiving_a_command_does_not_release_the_client_credit() {
        let (mut io, commands, replies) = ports();
        assert_eq!(io.try_reply().unwrap_err(), ReplyError::NoPending);
        io.try_submit(WorkerCommand::TakeOutputEvents).unwrap();
        assert!(matches!(
            commands.try_recv(),
            Ok(WorkerCommand::TakeOutputEvents)
        ));
        assert!(io.has_pending());
        assert!(io.try_reply().unwrap().is_none());
        let (command, allocation) = audio_command();
        let rejected = io.try_submit(command).unwrap_err();
        assert_eq!(rejected.reason, CommandRejectionReason::Pending);
        assert_original_audio(rejected.command, allocation);
        replies.try_send(output_reply()).unwrap();
        assert!(io.has_pending());
        assert!(matches!(
            io.try_reply().unwrap(),
            Some(WorkerReply::OutputEvents { lost: false, .. })
        ));
        assert!(!io.has_pending());
        io.try_submit(WorkerCommand::TakeOutputEvents).unwrap();
        assert!(matches!(
            commands.try_recv(),
            Ok(WorkerCommand::TakeOutputEvents)
        ));
    }

    #[test]
    fn command_and_reply_channels_each_have_capacity_one() {
        let (mut io, commands, replies) = ports();
        // Deliberately bypass credit from inside this module to check the actual capacity and
        // defensive Full path. No public API exposes this sender or permits this bypass.
        io.command
            .as_ref()
            .unwrap()
            .try_send(WorkerCommand::TakeOutputEvents)
            .unwrap();
        let (command, allocation) = audio_command();
        let rejected = io.try_submit(command).unwrap_err();
        assert_eq!(rejected.reason, CommandRejectionReason::Full);
        assert_original_audio(rejected.command, allocation);
        assert!(io.is_terminal());
        assert!(matches!(
            commands.try_recv(),
            Ok(WorkerCommand::TakeOutputEvents)
        ));
        replies.try_send(output_reply()).unwrap();
        assert!(matches!(
            replies.try_send(output_reply()),
            Err(TrySendError::Full(_))
        ));
    }

    #[test]
    fn close_returns_unsubmitted_audio_and_disconnects_the_unique_sender() {
        let (mut io, commands, _replies) = ports();
        io.close();
        let (command, allocation) = audio_command();
        let rejected = io.try_submit(command).unwrap_err();
        assert_eq!(rejected.reason, CommandRejectionReason::Closed);
        assert_original_audio(rejected.command, allocation);
        assert!(matches!(
            commands.try_recv(),
            Err(TryRecvError::Disconnected)
        ));
        assert!(!io.is_terminal());
    }

    #[test]
    fn dropping_the_port_closes_submission_and_marks_shared_readiness() {
        let status = Arc::new(WorkerStatus::new());
        let (io, commands, _replies) = WorkerIo::channel(status.clone());
        drop(io);
        assert!(status.is_closed());
        assert!(matches!(
            commands.try_recv(),
            Err(TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn disconnected_submission_returns_original_audio_and_becomes_terminal() {
        let (mut io, commands, _replies) = ports();
        drop(commands);
        let (command, allocation) = audio_command();
        let rejected = io.try_submit(command).unwrap_err();
        assert_eq!(rejected.reason, CommandRejectionReason::Disconnected);
        assert_original_audio(rejected.command, allocation);
        assert!(io.is_terminal());
        assert!(!io.has_pending());
        assert_eq!(
            io.try_submit(WorkerCommand::TakeOutputEvents)
                .unwrap_err()
                .reason,
            CommandRejectionReason::Terminal,
        );
    }

    #[test]
    fn disconnected_reply_does_not_release_an_unfulfilled_credit() {
        let (mut io, _commands, replies) = ports();
        io.try_submit(WorkerCommand::TakeOutputEvents).unwrap();
        drop(replies);
        assert_eq!(io.try_reply().unwrap_err(), ReplyError::Disconnected);
        assert!(io.has_pending());
        assert!(io.is_terminal());
    }

    #[test]
    fn a_failure_reply_keeps_its_buffers_and_the_independent_error_identity() {
        let (mut io, commands, replies) = ports();
        let (command, allocation) = audio_command();
        io.try_submit(command).unwrap();
        let WorkerCommand::Process { mut buffers, .. } = commands.try_recv().unwrap() else {
            panic!("expected process command");
        };
        buffers.outputs[0][0] = 0.75;
        let ledger_error = Arc::new(Error::ProcessFailed(-17));
        replies
            .try_send(WorkerReply::Processed {
                buffers,
                result: Err(ledger_error.clone()),
            })
            .unwrap();
        let WorkerReply::Processed { buffers, result } = io.recv_reply().unwrap() else {
            panic!("expected process reply");
        };
        assert_eq!(buffers.inputs[0].as_ptr(), allocation);
        assert_eq!(buffers.outputs[0][0], 0.75);
        assert!(Arc::ptr_eq(&ledger_error, &result.unwrap_err()));
        assert!(!io.has_pending());
        assert!(io.is_terminal());
        assert_eq!(
            io.try_submit(WorkerCommand::TakeOutputEvents)
                .unwrap_err()
                .reason,
            CommandRejectionReason::Terminal,
        );
        assert!(matches!(ledger_error.as_ref(), Error::ProcessFailed(-17)));
    }

    #[test]
    fn a_terminal_status_does_not_hide_the_already_published_failure_reply() {
        let status = Arc::new(WorkerStatus::new());
        let (mut io, commands, replies) = WorkerIo::channel(status.clone());
        io.try_submit(WorkerCommand::TakeOutputEvents).unwrap();
        commands.try_recv().unwrap();
        status.mark_terminal();
        replies.try_send(output_reply()).unwrap();
        assert!(io.try_reply().unwrap().is_some());
        assert!(io.is_terminal());
        assert!(!io.has_pending());
    }

    #[test]
    fn an_unread_reply_can_be_recovered_after_sealing() {
        let (mut io, commands, replies) = ports();
        let (command, allocation) = audio_command();
        io.try_submit(command).unwrap();
        let WorkerCommand::Process { buffers, .. } = commands.try_recv().unwrap() else {
            panic!("expected process command");
        };
        replies
            .try_send(WorkerReply::Processed {
                buffers,
                result: Ok(()),
            })
            .unwrap();
        drop(replies);
        io.close();
        assert!(io.has_pending());
        let WorkerReply::Processed { buffers, result } = io.take_unconsumed_reply().unwrap() else {
            panic!("expected unconsumed process reply");
        };
        assert_eq!(buffers.inputs[0].as_ptr(), allocation);
        assert!(result.is_ok());
        assert!(!io.has_pending());
        assert!(io.take_unconsumed_reply().is_none());
    }

    #[test]
    fn missing_unread_reply_preserves_pending_evidence() {
        let (mut io, _commands, replies) = ports();
        io.try_submit(WorkerCommand::TakeOutputEvents).unwrap();
        drop(replies);
        io.close();
        assert!(io.take_unconsumed_reply().is_none());
        assert!(io.has_pending());
    }

    #[test]
    fn output_event_loss_is_not_invented_by_transport_readiness() {
        let status = Arc::new(WorkerStatus::new());
        let (mut io, commands, replies) = WorkerIo::channel(status.clone());
        io.try_submit(WorkerCommand::TakeOutputEvents).unwrap();
        commands.try_recv().unwrap();
        replies.try_send(output_reply()).unwrap();
        status.mark_terminal();
        let WorkerReply::OutputEvents { lost, events } = io.recv_reply().unwrap() else {
            panic!("expected output events");
        };
        assert!(!lost);
        assert!(events.is_empty());
    }

    #[test]
    fn ui_dispatch_errors_are_returned_unchanged_and_may_be_handled() {
        let mut calls = 0;
        let mut dispatch = |_request| {
            calls += 1;
            if calls == 1 {
                Err(Error::ProcessFailed(-31))
            } else {
                Ok(AttachedUiReply::CanResize(true))
            }
        };
        let mut bridge = AttachedUiBridge::new(&mut dispatch, Arc::new(WorkerStatus::new()));
        assert!(matches!(
            bridge.request(AttachedUiRequest::CanResize),
            Err(Error::ProcessFailed(-31))
        ));
        assert_eq!(
            bridge.request(AttachedUiRequest::CanResize).unwrap(),
            AttachedUiReply::CanResize(true)
        );
    }

    #[test]
    fn closed_status_prevents_ui_dispatch() {
        let status = Arc::new(WorkerStatus::new());
        let mut dispatch = |_request| -> crate::Result<AttachedUiReply> {
            panic!("closed bridge must not dispatch");
        };
        let mut bridge = AttachedUiBridge::new(&mut dispatch, status.clone());
        status.mark_closed();
        assert!(bridge.request(AttachedUiRequest::ServiceRunLoop).is_err());
    }

    #[test]
    fn ordinary_midi_conversion_preserves_values_before_validation() {
        let messages = [
            (
                OrdinaryMidiEvent::NoteOn {
                    channel: MidiChannel::Ch16,
                    note: 255,
                    velocity: 254,
                },
                crate::MidiEvent::NoteOn {
                    channel: MidiChannel::Ch16,
                    note: 255,
                    velocity: 254,
                },
            ),
            (
                OrdinaryMidiEvent::NoteOff {
                    channel: MidiChannel::Ch2,
                    note: 17,
                    velocity: 93,
                },
                crate::MidiEvent::NoteOff {
                    channel: MidiChannel::Ch2,
                    note: 17,
                    velocity: 93,
                },
            ),
            (
                OrdinaryMidiEvent::PolyAftertouch {
                    channel: MidiChannel::Ch9,
                    note: 3,
                    pressure: 200,
                },
                crate::MidiEvent::PolyAftertouch {
                    channel: MidiChannel::Ch9,
                    note: 3,
                    pressure: 200,
                },
            ),
        ];
        for (ordinary, expected) in messages {
            assert_eq!(ordinary.into_midi_event(), expected);
        }
    }

    #[test]
    fn callback_error_and_worker_error_remain_separate_owned_causes() {
        let worker_error = Arc::new(Error::ProcessFailed(-41));
        let error = ScopedWorkerError {
            primary: ScopedWorkerFailure::Body(Error::ProcessFailed(-53)),
            secondary_worker: Some(WorkerOutcome::Failed(worker_error.clone())),
            unconsumed_reply: None,
        };
        assert!(matches!(
            error.primary,
            ScopedWorkerFailure::Body(Error::ProcessFailed(-53))
        ));
        let Some(WorkerOutcome::Failed(secondary)) = error.secondary_worker else {
            panic!("expected independent worker failure");
        };
        assert!(Arc::ptr_eq(&worker_error, &secondary));
    }
}
