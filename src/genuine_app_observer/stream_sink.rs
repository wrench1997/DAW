//! One run-owned, bounded producer for complete borrowed observations.
//! Worker/control threads only: emitting may yield until the fixed deadline.
//! This actor proves framing/durable writer retirement, never SDK retirement.
use super::{
    stream_app_control as app, stream_body as body, stream_codec as native,
    stream_control as control, stream_envelope as envelope, stream_spool as spool,
    stream_state as state,
};
use std::{
    path::Path,
    sync::{Arc, Mutex, TryLockError},
    thread,
    time::Instant,
};

pub(super) const MAX_FILE_BYTES: u64 = 192_000_000;
pub(super) const MAX_RECORDS: u64 = 1_000_000;
const CONFIG: spool::SpoolConfig = spool::SpoolConfig {
    slabs: 64,
    slab_bytes: 2048,
    queue_capacity: 64,
    max_operand_bytes: envelope::MAX_OPERAND,
    max_file_bytes: MAX_FILE_BYTES,
    max_records: MAX_RECORDS,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum FailureKind {
    Allocation,
    Sequence,
    Timestamp,
    Deadline,
    Protocol,
    Bounds,
    Header,
    Encoding(body::Error),
    StateEncoding(state::BodyError),
    Spool(spool::Failure),
    Abandoned,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Failure {
    pub kind: FailureKind,
    /// The exact attempted header, including optional original identity.
    pub attempted_header: Option<envelope::Header>,
}
#[must_use = "startup rejection returns the original dictionary and buffer"]
pub(super) struct StartFailure {
    pub failure: Failure,
    pub dictionary: native::BaselineDictionary,
    pub buffer: Vec<u8>,
}
impl std::fmt::Debug for StartFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StartFailure")
            .field("failure", &self.failure)
            .field("buffer_length", &self.buffer.len())
            .field("buffer_capacity", &self.buffer.capacity())
            .finish_non_exhaustive()
    }
}

struct Inner {
    dictionary: native::BaselineDictionary,
    spool: spool::BoundedSpool,
    // This same allocation is either here or in the spool's pending operand.
    buffer: Option<Vec<u8>>,
    deadline: Instant,
    submitted: spool::Expected,
    last_monotonic_ns: Option<u64>,
    sealed: bool,
    caller_taps_retired: bool,
    failure: Option<Failure>,
}

#[derive(Clone, Copy)]
pub(super) struct Terminal<'a> {
    pub writer: &'a spool::Terminal,
    pub failure: Option<&'a Failure>,
    pub expected: spool::Expected,
    /// Caller supplied only after its actual tap/SDK owners have retired.
    pub caller_taps_retired: bool,
    sealed: bool,
}
impl Terminal<'_> {
    pub(super) fn complete(&self) -> bool {
        self.sealed
            && self.caller_taps_retired
            && self.failure.is_none()
            && self.writer.complete()
            && self.writer.ack.records == self.expected.records
            && self.writer.ack.bytes == self.expected.bytes
    }
}
pub(super) enum FinishPoll<'a> {
    Pending,
    Joined(Terminal<'a>),
}

fn terminal(inner: &Inner) -> Option<Terminal<'_>> {
    inner.spool.terminal().map(|writer| Terminal {
        writer,
        failure: inner.failure.as_ref(),
        expected: inner.submitted,
        caller_taps_retired: inner.caller_taps_retired,
        sealed: inner.sealed,
    })
}
fn reject(inner: &mut Inner, kind: FailureKind, header: Option<envelope::Header>) -> Failure {
    let failure = inner
        .failure
        .get_or_insert(Failure {
            kind,
            attempted_header: header,
        })
        .clone();
    let cause = match &failure.kind {
        FailureKind::Spool(cause) => cause.clone(),
        FailureKind::Deadline => spool::Failure {
            kind: spool::FailureKind::Deadline,
            diagnostic: "fixed stream run deadline expired".into(),
        },
        FailureKind::Abandoned => spool::Failure {
            kind: spool::FailureKind::Abandoned,
            diagnostic: "stream actor retired before its actual writer join".into(),
        },
        _ => spool::Failure {
            kind: spool::FailureKind::Protocol,
            diagnostic: "complete stream producer rejected an original operand".into(),
        },
    };
    inner.spool.cancel(cause);
    failure
}

fn poll_finish_inner(inner: &mut Inner) -> bool {
    if inner.spool.terminal().is_some_and(|t| t.writer_joined) {
        return true;
    }
    if let Some(cause) = inner.spool.failure() {
        reject(inner, FailureKind::Spool(cause), None);
    } else if Instant::now() >= inner.deadline {
        reject(inner, FailureKind::Deadline, None);
    } else if !inner.sealed && inner.failure.is_none() {
        reject(inner, FailureKind::Protocol, None);
    }
    // Only BoundedSpool consumes the actual JoinHandle, after is_finished.
    inner.spool.poll_join()
}

struct Retention {
    owner: Option<Inner>,
}
impl Drop for Retention {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.take() {
            if owner.spool.terminal().is_some_and(|t| t.writer_joined) {
                drop(owner);
            } else {
                // Retain dictionary, reusable/partial operand, and the exact
                // writer owner together. No detached JoinHandle is released.
                std::mem::forget(owner);
            }
        }
    }
}
#[must_use = "retain the actor or a recovery handle until the actual writer has joined"]
pub(super) struct StreamSink {
    inner: Option<Inner>,
    retention: Arc<Mutex<Retention>>,
}
#[derive(Clone)]
#[must_use = "this handle owns recovery of an abandoned whole stream actor"]
pub(super) struct RecoveryHandle {
    retention: Arc<Mutex<Retention>>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RecoveryPoll {
    NoRetainedOwner,
    Pending,
    Joined,
    Busy,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct InspectionBusy;
pub(super) struct Inspection<'a> {
    pub dictionary: &'a native::BaselineDictionary,
    pub buffer: Option<&'a [u8]>,
    pub buffer_capacity: Option<usize>,
    pub pending_operand: Option<&'a spool::PendingOperand>,
    pub unqueued_fragment: Option<&'a spool::Fragment>,
    pub terminal: Option<Terminal<'a>>,
    pub failure: Option<&'a Failure>,
    pub expected: spool::Expected,
}
fn inspection(inner: &Inner) -> Inspection<'_> {
    Inspection {
        dictionary: &inner.dictionary,
        buffer: inner.buffer.as_deref(),
        buffer_capacity: inner.buffer.as_ref().map(Vec::capacity),
        pending_operand: inner.spool.pending_operand(),
        unqueued_fragment: inner.spool.unqueued_fragment(),
        terminal: terminal(inner),
        failure: inner.failure.as_ref(),
        expected: inner.submitted,
    }
}

impl StreamSink {
    #[allow(clippy::result_large_err)] // Startup returns original bounded owners without another allocation.
    pub(super) fn create_new(
        dictionary: native::BaselineDictionary,
        absolute_path: &Path,
        deadline: Instant,
    ) -> Result<Self, StartFailure> {
        Self::start(dictionary, deadline, |config| {
            spool::BoundedSpool::create_new(absolute_path, config)
        })
    }

    #[allow(clippy::result_large_err)] // Same unchanged startup-owner return as create_new.
    fn start(
        dictionary: native::BaselineDictionary,
        deadline: Instant,
        create: impl FnOnce(spool::SpoolConfig) -> Result<spool::BoundedSpool, spool::Failure>,
    ) -> Result<Self, StartFailure> {
        let mut buffer = Vec::new();
        let failure = if Instant::now() >= deadline {
            Some(FailureKind::Deadline)
        } else if buffer.try_reserve_exact(envelope::MAX_OPERAND).is_err()
            || buffer.capacity() > envelope::MAX_OPERAND
        {
            Some(FailureKind::Allocation)
        } else {
            None
        };
        if let Some(kind) = failure {
            return Err(StartFailure {
                failure: Failure {
                    kind,
                    attempted_header: None,
                },
                dictionary,
                buffer,
            });
        }
        let spool = match create(CONFIG) {
            Ok(spool) => spool,
            Err(cause) => {
                return Err(StartFailure {
                    failure: Failure {
                        kind: FailureKind::Spool(cause),
                        attempted_header: None,
                    },
                    dictionary,
                    buffer,
                });
            }
        };
        Ok(Self {
            inner: Some(Inner {
                dictionary,
                spool,
                buffer: Some(buffer),
                deadline,
                submitted: spool::Expected {
                    records: 0,
                    bytes: 0,
                },
                last_monotonic_ns: None,
                sealed: false,
                caller_taps_retired: false,
                failure: None,
            }),
            retention: Arc::new(Mutex::new(Retention { owner: None })),
        })
    }

    fn emit(
        &mut self,
        header: envelope::Header,
        encode: impl FnOnce(&native::BaselineDictionary, &mut Vec<u8>) -> Result<(), FailureKind>,
    ) -> Result<(), Failure> {
        let inner = self.inner.as_mut().expect("active stream actor");
        if let Some(failure) = &inner.failure {
            return Err(failure.clone());
        }
        if let Some(cause) = inner.spool.failure() {
            return Err(reject(inner, FailureKind::Spool(cause), Some(header)));
        }
        if inner.sealed
            || inner
                .buffer
                .as_ref()
                .is_none_or(|buffer| !buffer.is_empty())
        {
            return Err(reject(inner, FailureKind::Protocol, Some(header)));
        }
        if Instant::now() >= inner.deadline {
            return Err(reject(inner, FailureKind::Deadline, Some(header)));
        }
        if header.sequence != inner.submitted.records + 1 {
            return Err(reject(inner, FailureKind::Sequence, Some(header)));
        }
        if inner
            .last_monotonic_ns
            .is_some_and(|last| header.monotonic_ns < last)
        {
            return Err(reject(inner, FailureKind::Timestamp, Some(header)));
        }
        if inner.submitted.records >= MAX_RECORDS {
            return Err(reject(inner, FailureKind::Bounds, Some(header)));
        }
        let buffer = inner
            .buffer
            .as_mut()
            .expect("one reusable operand allocation");
        if let Err(kind) = encode(&inner.dictionary, buffer) {
            // Codec failures retain their documented destination state. The
            // original borrowed SDK/App operand is still owned by the caller.
            // The active tap must retain its first failed original RecordKind
            // under its existing field/byte bounds before returning the loan.
            // This actor never clones it or advances past the missing record.
            return Err(reject(inner, kind, Some(header)));
        }
        let length = buffer.len() as u64;
        let total = inner.submitted.bytes.checked_add(length);
        if length == 0
            || buffer.len() > envelope::MAX_OPERAND
            || buffer.capacity() > envelope::MAX_OPERAND
            || total.is_none_or(|bytes| bytes > MAX_FILE_BYTES)
        {
            return Err(reject(inner, FailureKind::Bounds, Some(header)));
        }
        let operand = spool::Operand {
            record_sequence: header.sequence,
            file_offset: inner.submitted.bytes,
            bytes: inner.buffer.take().expect("original operand allocation"),
        };
        let mut progress = inner.spool.submit_reusable(operand, inner.deadline);
        loop {
            match progress {
                Ok(spool::SubmitProgress::Submitted { buffer }) => {
                    inner.buffer = Some(buffer);
                    inner.submitted.records += 1;
                    inner.submitted.bytes = total.expect("admitted byte total");
                    inner.last_monotonic_ns = Some(header.monotonic_ns);
                    return Ok(());
                }
                Ok(spool::SubmitProgress::Pending) => {
                    // A healthy busy writer is backpressure, not a lost
                    // record. Poll the same operand at its original offset.
                    thread::yield_now();
                    progress = inner.spool.poll_submit_reusable();
                }
                Err(error) => {
                    if let Some(operand) = error.returned_operand {
                        inner.buffer = Some(operand.bytes);
                    }
                    return Err(reject(
                        inner,
                        FailureKind::Spool(error.failure),
                        Some(header),
                    ));
                }
            }
        }
    }

    pub(super) fn emit_control(
        &mut self,
        header: envelope::Header,
        view: control::View<'_>,
    ) -> Result<(), Failure> {
        self.emit(header, |_, buffer| {
            body::encode_control(header, view, buffer).map_err(FailureKind::Encoding)
        })
    }
    pub(super) fn emit_app(
        &mut self,
        header: envelope::Header,
        view: app::AppView<'_>,
    ) -> Result<(), Failure> {
        self.emit(header, |_, buffer| {
            body::encode_app(header, view, buffer).map_err(FailureKind::Encoding)
        })
    }
    pub(super) fn emit_native(
        &mut self,
        header: envelope::Header,
        view: native::NativeView<'_>,
    ) -> Result<(), Failure> {
        self.emit(header, |dictionary, buffer| {
            body::encode_native(header, view, dictionary, buffer).map_err(FailureKind::Encoding)
        })
    }
    pub(super) fn emit_state(
        &mut self,
        header: envelope::Header,
        view: state::StateView<'_>,
    ) -> Result<(), Failure> {
        self.emit(header, |dictionary, buffer| {
            state::encode_enveloped(view, Some(dictionary), buffer)
                .map_err(FailureKind::StateEncoding)?;
            let (actual, _) = envelope::decode(buffer).map_err(|_| FailureKind::Header)?;
            if actual != header {
                return Err(FailureKind::Header);
            }
            Ok(())
        })
    }

    /// The caller must derive this witness from actual SDK/tap retirement,
    /// not merely a quiet queue. It is retained as supplied, not authenticated.
    pub(super) fn seal_after_taps_retired(&mut self, taps_retired: bool) -> Result<(), Failure> {
        let inner = self.inner.as_mut().expect("active stream actor");
        if let Some(failure) = &inner.failure {
            return Err(failure.clone());
        }
        if let Some(cause) = inner.spool.failure() {
            return Err(reject(inner, FailureKind::Spool(cause), None));
        }
        if Instant::now() >= inner.deadline {
            return Err(reject(inner, FailureKind::Deadline, None));
        }
        if !taps_retired || inner.sealed || inner.buffer.as_ref().is_none_or(|b| !b.is_empty()) {
            return Err(reject(inner, FailureKind::Protocol, None));
        }
        if let Err(cause) = inner.spool.seal(inner.submitted) {
            return Err(reject(inner, FailureKind::Spool(cause), None));
        }
        inner.caller_taps_retired = true;
        inner.sealed = true;
        Ok(())
    }
    pub(super) fn cancel(&mut self) {
        reject(
            self.inner.as_mut().expect("active stream actor"),
            FailureKind::Abandoned,
            None,
        );
    }
    pub(super) fn poll_finish(&mut self) -> FinishPoll<'_> {
        let inner = self.inner.as_mut().expect("active stream actor");
        if poll_finish_inner(inner) {
            FinishPoll::Joined(terminal(inner).expect("actual joined spool terminal"))
        } else {
            FinishPoll::Pending
        }
    }
    pub(super) fn terminal(&self) -> Option<Terminal<'_>> {
        terminal(self.inner.as_ref().expect("active stream actor"))
    }
    pub(super) fn failure(&self) -> Option<Failure> {
        let inner = self.inner.as_ref().expect("active stream actor");
        inner.failure.clone().or_else(|| {
            inner.spool.failure().map(|cause| Failure {
                kind: FailureKind::Spool(cause),
                attempted_header: None,
            })
        })
    }
    pub(super) fn failure_header(&self) -> Option<envelope::Header> {
        self.inner
            .as_ref()
            .expect("active stream actor")
            .failure
            .as_ref()
            .and_then(|failure| failure.attempted_header)
    }
    pub(super) fn inspect(&self) -> Inspection<'_> {
        inspection(self.inner.as_ref().expect("active stream actor"))
    }
    pub(super) fn recovery_handle(&self) -> RecoveryHandle {
        RecoveryHandle {
            retention: self.retention.clone(),
        }
    }
    pub(super) fn into_recovery(mut self) -> RecoveryHandle {
        let handle = self.recovery_handle();
        if let Some(mut inner) = self.inner.take() {
            if !inner.spool.terminal().is_some_and(|t| t.writer_joined) {
                reject(&mut inner, FailureKind::Abandoned, None);
            }
            let mut retained = self.retention.lock().unwrap_or_else(|p| p.into_inner());
            if retained.owner.is_none() {
                // Explicit transfer also preserves a joined failed actor for
                // subsequent readback. Ordinary Drop may release joined owners.
                retained.owner = Some(inner);
            } else {
                std::mem::forget(inner);
            }
        }
        handle
    }
}
impl Drop for StreamSink {
    fn drop(&mut self) {
        let Some(mut inner) = self.inner.take() else {
            return;
        };
        if inner.spool.terminal().is_some_and(|t| t.writer_joined) {
            return;
        }
        reject(&mut inner, FailureKind::Abandoned, None);
        let mut retained = self.retention.lock().unwrap_or_else(|p| p.into_inner());
        if retained.owner.is_none() {
            retained.owner = Some(inner);
        } else {
            std::mem::forget(inner);
        }
    }
}
impl RecoveryHandle {
    pub(super) fn poll_finish(&self) -> RecoveryPoll {
        let mut retention = match self.retention.try_lock() {
            Ok(retention) => retention,
            Err(TryLockError::Poisoned(poison)) => poison.into_inner(),
            Err(TryLockError::WouldBlock) => return RecoveryPoll::Busy,
        };
        let Some(inner) = &mut retention.owner else {
            return RecoveryPoll::NoRetainedOwner;
        };
        if poll_finish_inner(inner) {
            RecoveryPoll::Joined
        } else {
            RecoveryPoll::Pending
        }
    }
    pub(super) fn inspect<T>(
        &self,
        read: impl FnOnce(Option<Inspection<'_>>) -> T,
    ) -> Result<T, InspectionBusy> {
        let retention = match self.retention.try_lock() {
            Ok(retention) => retention,
            Err(TryLockError::Poisoned(poison)) => poison.into_inner(),
            Err(TryLockError::WouldBlock) => return Err(InspectionBusy),
        };
        Ok(read(retention.owner.as_ref().map(inspection)))
    }
}

#[cfg(test)]
#[path = "stream_sink_tests.rs"]
mod tests;
