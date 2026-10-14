//! Explicit, bounded byte spool for worker/control-thread observations.
//! Original bytes are fragmented losslessly. Completion requires drain + flush/sync,
//! exact expected byte/record counts, actual file length and actual writer JoinHandle.
//! Legacy submit treats temporary exhaustion as sticky failure. Reusable submit
//! instead retains one operand and its exact fragment offset under a fixed deadline.
use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    panic::{AssertUnwindSafe, catch_unwind},
    path::Path,
    sync::{
        Arc, Mutex, TryLockError,
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    },
    thread::{self, JoinHandle},
    time::Instant,
};

const MAX_SLABS: usize = 64;
const MAX_SLAB_BYTES: usize = 2048;
const MAX_OPERAND_BYTES: usize = 64 * 1024;
const MAX_SPOOL_ARCHIVE_BYTES: u64 = 192_000_000;
const MAX_SPOOL_RECORDS: u64 = 1_000_000;
const MAX_DIAGNOSTIC_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug)]
pub(super) struct SpoolConfig {
    pub slabs: usize,
    pub slab_bytes: usize,
    pub queue_capacity: usize,
    pub max_operand_bytes: usize,
    pub max_file_bytes: u64,
    pub max_records: u64,
}

impl SpoolConfig {
    fn validate(self) -> Result<Self, Failure> {
        if !(1..=MAX_SLABS).contains(&self.slabs)
            || !(1..=MAX_SLAB_BYTES).contains(&self.slab_bytes)
            || !(1..=self.slabs).contains(&self.queue_capacity)
            || !(1..=MAX_OPERAND_BYTES).contains(&self.max_operand_bytes)
            || !(1..=MAX_SPOOL_ARCHIVE_BYTES).contains(&self.max_file_bytes)
            || !(1..=MAX_SPOOL_RECORDS).contains(&self.max_records)
        {
            return Err(Failure::new(
                FailureKind::Admission,
                "invalid explicit spool bounds",
            ));
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FailureKind {
    Admission,
    Allocation,
    Spawn,
    QueueFull,
    Disconnected,
    Protocol,
    Io,
    WorkerPanic,
    Abandoned,
    Poisoned,
    Deadline,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Failure {
    pub kind: FailureKind,
    pub diagnostic: String,
}
impl Failure {
    fn new(kind: FailureKind, diagnostic: impl AsRef<str>) -> Self {
        let text = diagnostic.as_ref();
        let mut end = text.len().min(MAX_DIAGNOSTIC_BYTES);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        // Copy into fresh bounded storage. Truncating the caller's String would
        // leave its possibly very large reserved allocation owned by the spool.
        Self {
            kind,
            diagnostic: text[..end].into(),
        }
    }
    fn bounded(self) -> Self {
        Self::new(self.kind, &self.diagnostic)
    }
}
impl Clone for Failure {
    // Public fields allow callers to construct an oversized diagnostic. Even a
    // clone of that value is normalized instead of duplicating its allocation.
    fn clone(&self) -> Self {
        Self::new(self.kind, &self.diagnostic)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Operand {
    pub record_sequence: u64,
    pub file_offset: u64,
    pub bytes: Vec<u8>,
}

#[derive(Debug)]
pub(super) struct PendingOperand {
    pub original: Operand,
    pub next_unqueued_byte: usize,
}

#[derive(Debug)]
pub(super) struct Fragment {
    pub slot_id: usize,
    pub slot_generation: u64,
    pub fragment_sequence: u64,
    pub record_sequence: u64,
    pub record_offset: u64,
    pub record_length: u64,
    pub operand_offset: usize,
    pub used: usize,
    data: Box<[u8]>,
    pub written_prefix: usize,
}
impl Fragment {
    pub(super) fn original_fragment_bytes(&self) -> &[u8] {
        &self.data[..self.used]
    }
}

struct Slab {
    id: usize,
    generation: u64,
    data: Box<[u8]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Expected {
    pub records: u64,
    pub bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DrainAck {
    pub records: u64,
    pub bytes: u64,
    pub complete_fragments: u64,
    pub last_fragment_sequence: u64,
    pub actual_file_length: Option<u64>,
    pub flushed: bool,
    pub synced: bool,
    pub expected_counts_matched: bool,
}

pub(super) struct Terminal {
    pub ack: DrainAck,
    pub failure: Option<Failure>,
    pub writer_joined: bool,
    pub retained_fragments: Vec<Fragment>,
}
impl Terminal {
    pub(super) fn complete(&self) -> bool {
        self.writer_joined
            && self.failure.is_none()
            && self.ack.flushed
            && self.ack.synced
            && self.ack.expected_counts_matched
            && self.retained_fragments.is_empty()
            && self.ack.actual_file_length == Some(self.ack.bytes)
    }
}

#[must_use = "a rejected operand remains owned here or in pending_operand(); do not discard its owner"]
pub(super) struct SubmitFailure {
    pub failure: Failure,
    pub returned_operand: Option<Operand>,
}

#[derive(Debug)]
#[must_use = "Pending retains the operand in this spool; Submitted returns its reusable allocation"]
pub(super) enum SubmitProgress {
    Pending,
    // Enqueue completion only. Actual write, flush/sync, length and join are
    // separately required by seal(), poll_join() and Terminal::complete().
    Submitted { buffer: Vec<u8> },
}

pub(super) trait SpoolSink: Send {
    fn write_bytes(&mut self, bytes: &[u8]) -> io::Result<usize>;
    fn flush_bytes(&mut self) -> io::Result<()>;
    fn sync_bytes(&mut self) -> io::Result<()>;
    fn actual_length(&mut self) -> io::Result<u64>;
}
struct FileSink(File);
impl SpoolSink for FileSink {
    fn write_bytes(&mut self, b: &[u8]) -> io::Result<usize> {
        self.0.write(b)
    }
    fn flush_bytes(&mut self) -> io::Result<()> {
        self.0.flush()
    }
    fn sync_bytes(&mut self) -> io::Result<()> {
        self.0.sync_all()
    }
    fn actual_length(&mut self) -> io::Result<u64> {
        Ok(self.0.metadata()?.len())
    }
}

#[derive(Default)]
struct Shared {
    first_failure: Option<Failure>,
    expected: Option<Expected>,
}
fn shared_failure(shared: &Arc<Mutex<Shared>>) -> Option<Failure> {
    let s = shared.lock().unwrap_or_else(|p| {
        let mut s = p.into_inner();
        s.first_failure.get_or_insert_with(|| {
            Failure::new(FailureKind::Poisoned, "spool shared state poisoned")
        });
        s
    });
    // Access is immutable apart from preserving poison as the first failure.
    s.first_failure.clone()
}
fn fail(shared: &Arc<Mutex<Shared>>, failure: Failure) -> Failure {
    // All externally supplied failures, including cancel(), cross this boundary
    // before storage, terminal assignment or any Shared clone is possible.
    let failure = failure.bounded();
    let mut s = shared.lock().unwrap_or_else(|p| p.into_inner());
    s.first_failure.get_or_insert(failure).clone()
}

struct Inner {
    config: SpoolConfig,
    tx: Option<SyncSender<Fragment>>,
    free: Receiver<Slab>,
    shared: Arc<Mutex<Shared>>,
    writer: Option<JoinHandle<Terminal>>,
    pending: Option<PendingOperand>,
    reusable_deadline: Option<Instant>,
    unqueued_fragment: Option<Fragment>,
    submitted: Expected,
    next_fragment: u64,
    terminal: Option<Terminal>,
    #[cfg(test)]
    reclaim_probe: Arc<()>,
}

fn fail_inner(inner: &mut Inner, cause: Failure) -> Failure {
    let failure = fail(&inner.shared, cause);
    if let Some(terminal) = &mut inner.terminal {
        terminal.failure = Some(failure.clone());
    }
    failure
}

// Per-spool retention, never a global test-shared registry. If a caller discards
// every recovery handle while a writer is unjoined, preserve the whole owner by
// deliberately retaining it, rather than dropping/detaching its JoinHandle.
struct Retention {
    abandoned: Option<Inner>,
}
impl Drop for Retention {
    fn drop(&mut self) {
        if let Some(owner) = self.abandoned.take() {
            if owner.writer.is_none()
                && owner
                    .terminal
                    .as_ref()
                    .is_some_and(|terminal| terminal.writer_joined)
            {
                // poll_inner has actually consumed and joined the JoinHandle.
                // A failed terminal still permits reclaiming its exact retained
                // operands when the final recovery handle is explicitly dropped.
                drop(owner);
            } else {
                // is_finished / EOF alone cannot authorize release. Only an
                // owner whose actual join remains unproven is quarantined.
                std::mem::forget(owner);
            }
        }
    }
}

#[must_use = "keep the spool owner or its recovery handle until actual writer join"]
pub(super) struct BoundedSpool {
    inner: Option<Inner>,
    retention: Arc<Mutex<Retention>>,
}
#[derive(Clone)]
#[must_use = "a dropped producer may retain its exact writer/operand owner here"]
pub(super) struct ShutdownHandle {
    retention: Arc<Mutex<Retention>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ShutdownPoll {
    NoRetainedOwner,
    Pending,
    Joined,
    Busy,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct InspectionBusy;

impl BoundedSpool {
    pub(super) fn create_new(path: &Path, config: SpoolConfig) -> Result<Self, Failure> {
        let config = config.validate()?;
        if !path.is_absolute() {
            return Err(Failure::new(
                FailureKind::Admission,
                "spool path must be absolute",
            ));
        }
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|e| Failure::new(FailureKind::Io, e.to_string()))?;
        Self::with_sink(config, Box::new(FileSink(file)))
    }

    pub(super) fn with_sink(
        config: SpoolConfig,
        sink: Box<dyn SpoolSink>,
    ) -> Result<Self, Failure> {
        let config = config.validate()?;
        let (tx, receiver) = mpsc::sync_channel(config.queue_capacity);
        let (free_tx, free) = mpsc::sync_channel(config.slabs);
        for id in 0..config.slabs {
            let mut data = Vec::new();
            data.try_reserve_exact(config.slab_bytes)
                .map_err(|_| Failure::new(FailureKind::Allocation, "slab reservation failed"))?;
            data.resize(config.slab_bytes, 0);
            free_tx
                .send(Slab {
                    id,
                    generation: 0,
                    data: data.into_boxed_slice(),
                })
                .map_err(|_| {
                    Failure::new(
                        FailureKind::Disconnected,
                        "initial free-slab queue disconnected",
                    )
                })?;
        }
        let shared = Arc::new(Mutex::new(Shared::default()));
        let mut retained = Vec::new();
        retained.try_reserve_exact(config.slabs).map_err(|_| {
            Failure::new(FailureKind::Allocation, "failure-slot reservation failed")
        })?;
        let mut generations = Vec::new();
        generations.try_reserve_exact(config.slabs).map_err(|_| {
            Failure::new(
                FailureKind::Allocation,
                "slot-generation reservation failed",
            )
        })?;
        generations.resize(config.slabs, 0);
        let worker_shared = shared.clone();
        let worker = Worker {
            config,
            receiver,
            free_tx,
            sink: Some(sink),
            shared: worker_shared,
            current: None,
            retained,
            generations,
            open: None,
            ack: DrainAck {
                records: 0,
                bytes: 0,
                complete_fragments: 0,
                last_fragment_sequence: 0,
                actual_file_length: None,
                flushed: false,
                synced: false,
                expected_counts_matched: false,
            },
        };
        let slot = Arc::new(Mutex::new(Some(worker)));
        let transfer = slot.clone();
        let writer = thread::Builder::new()
            .name("bounded-sidecar-writer".into())
            .spawn(move || {
                let mut worker = transfer
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .take()
                    .expect("unique writer transfer");
                let result = catch_unwind(AssertUnwindSafe(|| worker.run()));
                if result.is_err() {
                    fail(
                        &worker.shared,
                        Failure::new(FailureKind::WorkerPanic, "writer operation panicked"),
                    );
                    worker.retain_current();
                    worker.drain_without_writing();
                }
                // Sink destruction is also a user-supplied operation. Keep the
                // worker's fragment owners outside its unwind, as for write/flush.
                if catch_unwind(AssertUnwindSafe(|| worker.close_sink())).is_err() {
                    fail(
                        &worker.shared,
                        Failure::new(FailureKind::WorkerPanic, "writer sink destruction panicked"),
                    );
                }
                worker.terminal()
            })
            .map_err(|e| {
                // Before any input admission. Retain the exact startup container on
                // spawn failure; no live writer or fragment is claimed terminated.
                std::mem::forget(slot);
                Failure::new(FailureKind::Spawn, e.to_string())
            })?;
        Ok(Self {
            inner: Some(Inner {
                config,
                tx: Some(tx),
                free,
                shared,
                writer: Some(writer),
                pending: None,
                reusable_deadline: None,
                unqueued_fragment: None,
                submitted: Expected {
                    records: 0,
                    bytes: 0,
                },
                next_fragment: 1,
                terminal: None,
                #[cfg(test)]
                reclaim_probe: Arc::new(()),
            }),
            retention: Arc::new(Mutex::new(Retention { abandoned: None })),
        })
    }

    pub(super) fn recovery_handle(&self) -> ShutdownHandle {
        ShutdownHandle {
            retention: self.retention.clone(),
        }
    }
    pub(super) fn pending_operand(&self) -> Option<&PendingOperand> {
        self.inner.as_ref().and_then(|i| i.pending.as_ref())
    }
    pub(super) fn unqueued_fragment(&self) -> Option<&Fragment> {
        self.inner
            .as_ref()
            .and_then(|i| i.unqueued_fragment.as_ref())
    }
    pub(super) fn failure(&self) -> Option<Failure> {
        self.inner.as_ref().and_then(|i| shared_failure(&i.shared))
    }

    #[cfg(test)]
    pub(super) fn owner_reclaim_probe_for_test(&self) -> std::sync::Weak<()> {
        Arc::downgrade(
            &self
                .inner
                .as_ref()
                .expect("active spool owner")
                .reclaim_probe,
        )
    }

    #[cfg(test)]
    pub(super) fn disconnect_slab_return_receiver_for_test(&mut self) {
        // Drop the actual return-channel endpoint, rather than manufacturing
        // a failure/ACK. Only unused free slabs are discarded by this adapter.
        let (sender, disconnected) = mpsc::sync_channel(1);
        drop(sender);
        self.inner.as_mut().expect("active spool owner").free = disconnected;
    }

    pub(super) fn submit(&mut self, operand: Operand) -> Result<(), SubmitFailure> {
        let inner = self.inner.as_mut().expect("active spool owner");
        if let Some(failure) = shared_failure(&inner.shared) {
            return Err(SubmitFailure {
                failure,
                returned_operand: Some(operand),
            });
        }
        let length = operand.bytes.len();
        let total = inner.submitted.bytes.checked_add(length as u64);
        if inner.tx.is_none()
            || inner.pending.is_some()
            || length == 0
            || length > inner.config.max_operand_bytes
            || operand.bytes.capacity() > inner.config.max_operand_bytes
            || operand.record_sequence != inner.submitted.records + 1
            || operand.file_offset != inner.submitted.bytes
            || inner.submitted.records >= inner.config.max_records
            || total.is_none_or(|v| v > inner.config.max_file_bytes)
        {
            let failure = fail_inner(
                inner,
                Failure::new(
                    FailureKind::Admission,
                    "operand identity, offset or explicit quota differs",
                ),
            );
            return Err(SubmitFailure {
                failure,
                returned_operand: Some(operand),
            });
        }
        inner.pending = Some(PendingOperand {
            original: operand,
            next_unqueued_byte: 0,
        });
        loop {
            let mut slab = match inner.free.try_recv() {
                Ok(slab) => slab,
                Err(TryRecvError::Empty) => {
                    let failure = fail_inner(
                        inner,
                        Failure::new(
                            FailureKind::QueueFull,
                            "no free bounded slab; original operand retained",
                        ),
                    );
                    return Err(SubmitFailure {
                        failure,
                        returned_operand: None,
                    });
                }
                Err(TryRecvError::Disconnected) => {
                    let failure = fail_inner(
                        inner,
                        Failure::new(
                            FailureKind::Disconnected,
                            "free-slab return channel disconnected",
                        ),
                    );
                    return Err(SubmitFailure {
                        failure,
                        returned_operand: None,
                    });
                }
            };
            let pending = inner.pending.as_ref().unwrap();
            let start = pending.next_unqueued_byte;
            let used = (length - start).min(inner.config.slab_bytes);
            slab.generation = match slab.generation.checked_add(1) {
                Some(value) => value,
                None => {
                    let failure = fail_inner(
                        inner,
                        Failure::new(FailureKind::Protocol, "slot generation overflow"),
                    );
                    return Err(SubmitFailure {
                        failure,
                        returned_operand: None,
                    });
                }
            };
            slab.data[..used].copy_from_slice(&pending.original.bytes[start..start + used]);
            let fragment = Fragment {
                slot_id: slab.id,
                slot_generation: slab.generation,
                fragment_sequence: inner.next_fragment,
                record_sequence: pending.original.record_sequence,
                record_offset: pending.original.file_offset,
                record_length: length as u64,
                operand_offset: start,
                used,
                data: slab.data,
                written_prefix: 0,
            };
            match inner.tx.as_ref().unwrap().try_send(fragment) {
                Ok(()) => {
                    inner.next_fragment += 1;
                    inner.pending.as_mut().unwrap().next_unqueued_byte += used;
                }
                Err(TrySendError::Full(fragment)) => {
                    inner.unqueued_fragment = Some(fragment);
                    let failure = fail_inner(
                        inner,
                        Failure::new(
                            FailureKind::QueueFull,
                            "bounded writer queue full; exact unsent fragment retained",
                        ),
                    );
                    return Err(SubmitFailure {
                        failure,
                        returned_operand: None,
                    });
                }
                Err(TrySendError::Disconnected(fragment)) => {
                    inner.unqueued_fragment = Some(fragment);
                    let failure = fail_inner(
                        inner,
                        Failure::new(
                            FailureKind::Disconnected,
                            "writer queue disconnected; exact unsent fragment retained",
                        ),
                    );
                    return Err(SubmitFailure {
                        failure,
                        returned_operand: None,
                    });
                }
            }
            if inner.pending.as_ref().unwrap().next_unqueued_byte == length {
                inner.pending.take();
                inner.submitted.records += 1;
                inner.submitted.bytes = total.unwrap();
                return Ok(());
            }
        }
    }

    /// Admit exactly one bounded operand without blocking. A temporarily full
    /// pool/queue produces Pending, retaining its complete bytes and any unsent
    /// fragment. The original deadline is fixed here, and is never refreshed by
    /// polling. Worker/control threads may yield between polls; callbacks may
    /// not call this API. Successful enqueue returns the same, cleared Vec.
    pub(super) fn submit_reusable(
        &mut self,
        operand: Operand,
        deadline: Instant,
    ) -> Result<SubmitProgress, SubmitFailure> {
        let inner = self.inner.as_mut().expect("active spool owner");
        if let Some(failure) = shared_failure(&inner.shared) {
            return Err(SubmitFailure {
                failure,
                returned_operand: Some(operand),
            });
        }
        let length = operand.bytes.len();
        let total = inner.submitted.bytes.checked_add(length as u64);
        if inner.tx.is_none()
            || inner.pending.is_some()
            || inner.unqueued_fragment.is_some()
            || inner.reusable_deadline.is_some()
            || length == 0
            || length > inner.config.max_operand_bytes
            || operand.bytes.capacity() > inner.config.max_operand_bytes
            || operand.record_sequence != inner.submitted.records + 1
            || operand.file_offset != inner.submitted.bytes
            || inner.submitted.records >= inner.config.max_records
            || total.is_none_or(|v| v > inner.config.max_file_bytes)
        {
            let failure = fail_inner(
                inner,
                Failure::new(
                    FailureKind::Admission,
                    "operand identity, offset or explicit quota differs",
                ),
            );
            return Err(SubmitFailure {
                failure,
                returned_operand: Some(operand),
            });
        }
        if Instant::now() >= deadline {
            let failure = fail_inner(
                inner,
                Failure::new(
                    FailureKind::Deadline,
                    "operand enqueue deadline expired before admission",
                ),
            );
            return Err(SubmitFailure {
                failure,
                returned_operand: Some(operand),
            });
        }
        inner.pending = Some(PendingOperand {
            original: operand,
            next_unqueued_byte: 0,
        });
        inner.reusable_deadline = Some(deadline);
        progress_reusable(inner)
    }

    /// Resume the same pending operand at its acknowledged enqueue offset.
    /// Never resubmits an accepted prefix, creates another slab pool, sleeps,
    /// or waits for a writer. On error the operand remains in pending_operand().
    pub(super) fn poll_submit_reusable(&mut self) -> Result<SubmitProgress, SubmitFailure> {
        progress_reusable(self.inner.as_mut().expect("active spool owner"))
    }

    pub(super) fn seal(&mut self, expected: Expected) -> Result<(), Failure> {
        let inner = self.inner.as_mut().expect("active spool owner");
        if let Some(failure) = shared_failure(&inner.shared) {
            inner.tx.take();
            return Err(failure);
        }
        if expected != inner.submitted || inner.pending.is_some() || inner.tx.is_none() {
            let failure = fail_inner(
                inner,
                Failure::new(
                    FailureKind::Protocol,
                    "seal counts differ from fully submitted records",
                ),
            );
            inner.tx.take();
            return Err(failure);
        }
        let mut shared = inner.shared.lock().unwrap_or_else(|p| p.into_inner());
        if shared.expected.replace(expected).is_some() {
            let failure = shared
                .first_failure
                .get_or_insert_with(|| Failure::new(FailureKind::Protocol, "duplicate spool seal"))
                .clone();
            drop(shared);
            inner.tx.take();
            return Err(failure);
        }
        drop(shared);
        inner.tx.take();
        Ok(())
    }

    pub(super) fn cancel(&mut self, cause: Failure) {
        if let Some(inner) = &mut self.inner {
            fail_inner(inner, cause);
            inner.tx.take();
        }
    }
    pub(super) fn poll_join(&mut self) -> bool {
        self.inner.as_mut().is_some_and(poll_inner)
    }
    pub(super) fn terminal(&self) -> Option<&Terminal> {
        self.inner.as_ref().and_then(|i| i.terminal.as_ref())
    }
}

fn progress_reusable(inner: &mut Inner) -> Result<SubmitProgress, SubmitFailure> {
    if let Some(failure) = shared_failure(&inner.shared) {
        return Err(SubmitFailure {
            failure,
            returned_operand: None,
        });
    }
    let Some(deadline) = inner.reusable_deadline.filter(|_| inner.pending.is_some()) else {
        let failure = fail_inner(
            inner,
            Failure::new(
                FailureKind::Protocol,
                "no reusable pending operand is owned by this spool",
            ),
        );
        return Err(SubmitFailure {
            failure,
            returned_operand: None,
        });
    };
    loop {
        if let Some(failure) = shared_failure(&inner.shared) {
            return Err(SubmitFailure {
                failure,
                returned_operand: None,
            });
        }
        if Instant::now() >= deadline {
            let failure = fail_inner(
                inner,
                Failure::new(
                    FailureKind::Deadline,
                    "fixed operand enqueue deadline expired; original operand retained",
                ),
            );
            return Err(SubmitFailure {
                failure,
                returned_operand: None,
            });
        }
        let fragment = if let Some(fragment) = inner.unqueued_fragment.take() {
            // Generation and fragment sequence belong to this exact unsent
            // slab. Do not reacquire/copy it or advance the enqueue offset.
            fragment
        } else {
            let mut slab = match inner.free.try_recv() {
                Ok(slab) => slab,
                Err(TryRecvError::Empty) => return Ok(SubmitProgress::Pending),
                Err(TryRecvError::Disconnected) => {
                    let failure = fail_inner(
                        inner,
                        Failure::new(
                            FailureKind::Disconnected,
                            "free-slab return channel disconnected",
                        ),
                    );
                    return Err(SubmitFailure {
                        failure,
                        returned_operand: None,
                    });
                }
            };
            let pending = inner.pending.as_ref().unwrap();
            let start = pending.next_unqueued_byte;
            let length = pending.original.bytes.len();
            let used = (length - start).min(inner.config.slab_bytes);
            slab.generation = match slab.generation.checked_add(1) {
                Some(value) => value,
                None => {
                    let failure = fail_inner(
                        inner,
                        Failure::new(FailureKind::Protocol, "slot generation overflow"),
                    );
                    return Err(SubmitFailure {
                        failure,
                        returned_operand: None,
                    });
                }
            };
            slab.data[..used].copy_from_slice(&pending.original.bytes[start..start + used]);
            Fragment {
                slot_id: slab.id,
                slot_generation: slab.generation,
                fragment_sequence: inner.next_fragment,
                record_sequence: pending.original.record_sequence,
                record_offset: pending.original.file_offset,
                record_length: length as u64,
                operand_offset: start,
                used,
                data: slab.data,
                written_prefix: 0,
            }
        };
        let used = fragment.used;
        let Some(tx) = &inner.tx else {
            inner.unqueued_fragment = Some(fragment);
            let failure = fail_inner(
                inner,
                Failure::new(
                    FailureKind::Disconnected,
                    "writer sender closed; exact unsent fragment retained",
                ),
            );
            return Err(SubmitFailure {
                failure,
                returned_operand: None,
            });
        };
        match tx.try_send(fragment) {
            Ok(()) => {
                inner.next_fragment += 1;
                inner.pending.as_mut().unwrap().next_unqueued_byte += used;
            }
            Err(TrySendError::Full(fragment)) => {
                inner.unqueued_fragment = Some(fragment);
                return Ok(SubmitProgress::Pending);
            }
            Err(TrySendError::Disconnected(fragment)) => {
                inner.unqueued_fragment = Some(fragment);
                let failure = fail_inner(
                    inner,
                    Failure::new(
                        FailureKind::Disconnected,
                        "writer queue disconnected; exact unsent fragment retained",
                    ),
                );
                return Err(SubmitFailure {
                    failure,
                    returned_operand: None,
                });
            }
        }
        let pending = inner.pending.as_ref().unwrap();
        if pending.next_unqueued_byte == pending.original.bytes.len() {
            // Submitting the final fragment is still not a write/flush/join
            // receipt. Preserve the complete original if failure is already
            // visible or the fixed admission deadline has elapsed.
            if let Some(failure) = shared_failure(&inner.shared) {
                return Err(SubmitFailure {
                    failure,
                    returned_operand: None,
                });
            }
            if Instant::now() >= deadline {
                let failure = fail_inner(
                    inner,
                    Failure::new(
                        FailureKind::Deadline,
                        "fixed operand enqueue deadline expired; original operand retained",
                    ),
                );
                return Err(SubmitFailure {
                    failure,
                    returned_operand: None,
                });
            }
            let mut pending = inner.pending.take().unwrap();
            inner.submitted.records += 1;
            inner.submitted.bytes += pending.original.bytes.len() as u64;
            inner.reusable_deadline = None;
            pending.original.bytes.clear();
            return Ok(SubmitProgress::Submitted {
                buffer: pending.original.bytes,
            });
        }
    }
}

fn poll_inner(inner: &mut Inner) -> bool {
    if let Some(terminal) = &mut inner.terminal {
        if let Some(failure) = shared_failure(&inner.shared) {
            terminal.failure = Some(failure);
        }
        return true;
    }
    if shared_failure(&inner.shared).is_some() {
        inner.tx.take();
    }
    if !inner.writer.as_ref().is_some_and(JoinHandle::is_finished) {
        return false;
    }
    let writer = inner.writer.take().unwrap();
    match writer.join() {
        Ok(mut terminal) => {
            terminal.writer_joined = true;
            if let Some(failure) = shared_failure(&inner.shared) {
                terminal.failure = Some(failure);
            }
            inner.terminal = Some(terminal);
        }
        Err(_) => {
            let failure = fail(
                &inner.shared,
                Failure::new(
                    FailureKind::WorkerPanic,
                    "writer outer thread panicked; drain unproven",
                ),
            );
            inner.terminal = Some(Terminal {
                ack: DrainAck {
                    records: 0,
                    bytes: 0,
                    complete_fragments: 0,
                    last_fragment_sequence: 0,
                    actual_file_length: None,
                    flushed: false,
                    synced: false,
                    expected_counts_matched: false,
                },
                failure: Some(failure),
                writer_joined: true,
                retained_fragments: Vec::new(),
            });
        }
    }
    true
}

impl Drop for BoundedSpool {
    fn drop(&mut self) {
        let Some(mut inner) = self.inner.take() else {
            return;
        };
        if inner.terminal.is_some() {
            return;
        }
        fail(
            &inner.shared,
            Failure::new(
                FailureKind::Abandoned,
                "spool producer dropped before actual writer join",
            ),
        );
        inner.tx.take();
        let mut retention = self.retention.lock().unwrap_or_else(|p| p.into_inner());
        if retention.abandoned.is_some() {
            std::mem::forget(inner);
        } else {
            retention.abandoned = Some(inner);
        }
    }
}
impl ShutdownHandle {
    pub(super) fn poll_join(&self) -> ShutdownPoll {
        let mut r = match self.retention.try_lock() {
            Ok(retention) => retention,
            Err(TryLockError::Poisoned(poison)) => poison.into_inner(),
            Err(TryLockError::WouldBlock) => return ShutdownPoll::Busy,
        };
        let Some(inner) = r.abandoned.as_mut() else {
            return ShutdownPoll::NoRetainedOwner;
        };
        if poll_inner(inner) {
            ShutdownPoll::Joined
        } else {
            ShutdownPoll::Pending
        }
    }
    pub(super) fn inspect<T>(
        &self,
        read: impl FnOnce(Option<&PendingOperand>, Option<&Fragment>, Option<&Terminal>) -> T,
    ) -> Result<T, InspectionBusy> {
        // References stay borrowed from their exact retained owners. Neither
        // this entry point nor poll_join ever waits for this mutex: a callback
        // that reenters either API receives explicit Busy without self-deadlock.
        let r = match self.retention.try_lock() {
            Ok(retention) => retention,
            Err(TryLockError::Poisoned(poison)) => poison.into_inner(),
            Err(TryLockError::WouldBlock) => return Err(InspectionBusy),
        };
        Ok(read(
            r.abandoned.as_ref().and_then(|i| i.pending.as_ref()),
            r.abandoned
                .as_ref()
                .and_then(|i| i.unqueued_fragment.as_ref()),
            r.abandoned.as_ref().and_then(|i| i.terminal.as_ref()),
        ))
    }
}

struct OpenRecord {
    sequence: u64,
    offset: u64,
    length: u64,
    copied: usize,
}
struct Worker {
    config: SpoolConfig,
    receiver: Receiver<Fragment>,
    free_tx: SyncSender<Slab>,
    sink: Option<Box<dyn SpoolSink>>,
    shared: Arc<Mutex<Shared>>,
    current: Option<Fragment>,
    retained: Vec<Fragment>,
    generations: Vec<u64>,
    open: Option<OpenRecord>,
    ack: DrainAck,
}
impl Worker {
    fn run(&mut self) {
        while let Ok(fragment) = self.receiver.recv() {
            self.current = Some(fragment);
            if shared_failure(&self.shared).is_some() {
                self.retain_current();
                continue;
            }
            if let Err(failure) = self.write_current() {
                fail(&self.shared, failure);
                self.retain_current();
                continue;
            }
            let fragment = self.current.take().unwrap();
            let slab = Slab {
                id: fragment.slot_id,
                generation: fragment.slot_generation,
                data: fragment.data,
            };
            if let Err(TrySendError::Disconnected(slab) | TrySendError::Full(slab)) =
                self.free_tx.try_send(slab)
            {
                fail(
                    &self.shared,
                    Failure::new(
                        FailureKind::Disconnected,
                        "written slab return was not acknowledged",
                    ),
                );
                // Preserve the exact owned slab even after output write succeeded.
                self.retained.push(Fragment {
                    slot_id: slab.id,
                    slot_generation: slab.generation,
                    fragment_sequence: fragment.fragment_sequence,
                    record_sequence: fragment.record_sequence,
                    record_offset: fragment.record_offset,
                    record_length: fragment.record_length,
                    operand_offset: fragment.operand_offset,
                    used: fragment.used,
                    data: slab.data,
                    written_prefix: fragment.written_prefix,
                });
            }
        }
        if shared_failure(&self.shared).is_none() {
            let expected = self
                .shared
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .expected;
            if self.open.is_some()
                || expected
                    != Some(Expected {
                        records: self.ack.records,
                        bytes: self.ack.bytes,
                    })
            {
                fail(
                    &self.shared,
                    Failure::new(
                        FailureKind::Protocol,
                        "drain ended without complete record/byte seal",
                    ),
                );
            } else {
                self.ack.expected_counts_matched = true;
            }
        }
        if shared_failure(&self.shared).is_none() {
            match self.sink.as_mut().unwrap().flush_bytes() {
                Ok(()) => self.ack.flushed = true,
                Err(e) => {
                    fail(&self.shared, Failure::new(FailureKind::Io, e.to_string()));
                }
            }
        }
        if shared_failure(&self.shared).is_none() {
            match self.sink.as_mut().unwrap().sync_bytes() {
                Ok(()) => self.ack.synced = true,
                Err(e) => {
                    fail(&self.shared, Failure::new(FailureKind::Io, e.to_string()));
                }
            }
        }
        if shared_failure(&self.shared).is_none() {
            match self.sink.as_mut().unwrap().actual_length() {
                Ok(length) => {
                    self.ack.actual_file_length = Some(length);
                    if length != self.ack.bytes {
                        fail(
                            &self.shared,
                            Failure::new(
                                FailureKind::Io,
                                "actual archive length differs from write acknowledgements",
                            ),
                        );
                    }
                }
                Err(e) => {
                    fail(&self.shared, Failure::new(FailureKind::Io, e.to_string()));
                }
            }
        }
    }

    fn write_current(&mut self) -> Result<(), Failure> {
        let f = self.current.as_ref().unwrap();
        let bad = |message| Failure::new(FailureKind::Protocol, message);
        if f.slot_id >= self.config.slabs
            || f.used == 0
            || f.used > self.config.slab_bytes
            || f.slot_generation != self.generations[f.slot_id] + 1
            || f.fragment_sequence != self.ack.last_fragment_sequence + 1
            || f.written_prefix != 0
            || f.operand_offset
                .checked_add(f.used)
                .is_none_or(|n| n as u64 > f.record_length)
        {
            return Err(bad("fragment sequence, generation or shape differs"));
        }
        if self.open.is_none() {
            if f.operand_offset != 0
                || f.record_sequence != self.ack.records + 1
                || f.record_offset != self.ack.bytes
            {
                return Err(bad("new record sequence or absolute offset differs"));
            }
            self.open = Some(OpenRecord {
                sequence: f.record_sequence,
                offset: f.record_offset,
                length: f.record_length,
                copied: 0,
            });
        }
        let record = self.open.as_ref().unwrap();
        if f.record_sequence != record.sequence
            || f.record_offset != record.offset
            || f.record_length != record.length
            || f.operand_offset != record.copied
        {
            return Err(bad(
                "record fragment is missing, duplicated or out of order",
            ));
        }
        while self.current.as_ref().unwrap().written_prefix < self.current.as_ref().unwrap().used {
            if let Some(failure) = shared_failure(&self.shared) {
                return Err(failure);
            }
            let f = self.current.as_ref().unwrap();
            let start = f.written_prefix;
            let written = match self
                .sink
                .as_mut()
                .unwrap()
                .write_bytes(&f.data[start..f.used])
            {
                Ok(0) => return Err(Failure::new(FailureKind::Io, "archive WriteZero")),
                Ok(n) if n <= f.used - start => n,
                Ok(_) => {
                    return Err(Failure::new(
                        FailureKind::Io,
                        "sink acknowledged more bytes than provided",
                    ));
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(Failure::new(FailureKind::Io, e.to_string())),
            };
            self.current.as_mut().unwrap().written_prefix += written;
            self.ack.bytes = self
                .ack
                .bytes
                .checked_add(written as u64)
                .filter(|n| *n <= self.config.max_file_bytes)
                .ok_or_else(|| bad("archive byte quota overflow"))?;
        }
        let f = self.current.as_ref().unwrap();
        self.generations[f.slot_id] = f.slot_generation;
        self.ack.last_fragment_sequence = f.fragment_sequence;
        self.ack.complete_fragments += 1;
        let record = self.open.as_mut().unwrap();
        record.copied += f.used;
        if record.copied as u64 == record.length {
            self.ack.records += 1;
            self.open = None;
        }
        Ok(())
    }
    fn retain_current(&mut self) {
        if let Some(f) = self.current.take() {
            self.retained.push(f);
        }
    }
    fn drain_without_writing(&mut self) {
        while let Ok(f) = self.receiver.recv() {
            self.current = Some(f);
            self.retain_current();
        }
    }
    fn close_sink(&mut self) {
        drop(self.sink.take());
    }
    fn terminal(self) -> Terminal {
        Terminal {
            ack: self.ack,
            failure: shared_failure(&self.shared),
            writer_joined: false,
            retained_fragments: self.retained,
        }
    }
}
