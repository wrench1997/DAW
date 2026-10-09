//! Bounded native-editor intent transport, separate from display notifications.
//!
//! Only the control-side producer uses a mutex, always with `try_lock`. The consumer
//! owns its ring endpoint and preallocated staging storage. It never takes that mutex
//! and acknowledges a batch only after admission of every edit and a successful SDK
//! process call. Sticky loss is deliberately not repaired by a later successful batch
//! or state restore. This module grants no permission to move plugin/COM ownership.

use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering::SeqCst},
    Arc, Mutex, TryLockError,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct NativeEdit {
    pub generation: u64,
    pub sequence: u64,
    pub id: u32,
    pub value: f64,
}

/// Errors carry no heap allocation; formatting belongs outside the process path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NativeEditError {
    ProducerBusy,
    ProducerPoisoned,
    Disconnected,
    Full,
    Exhausted,
    DeliveryLost,
    PublicationInFlight,
    PendingEdits,
    ProcessInFlight,
    AdmissionRejected,
    ProcessFailed,
    AbandonedProcess,
    NoProcess,
    Invalidated,
    ChannelMismatch,
    SupersessionFailed,
    GenerationChanged,
}

struct Status {
    dirty: AtomicU64,
    submitted: AtomicU64,
    applied: AtomicU64,
    generation: AtomicU64,
    inflight: AtomicU64,
    processing: AtomicBool,
    lost: AtomicBool,
    exhausted: AtomicBool,
    invalid: AtomicBool,
}

impl Status {
    fn lose(&self) {
        self.lost.store(true, SeqCst);
    }

    fn exhaust(&self) -> NativeEditError {
        self.lose();
        self.exhausted.store(true, SeqCst);
        NativeEditError::Exhausted
    }

    fn begin_publication(&self) -> Result<Publication<'_>, NativeEditError> {
        increment(&self.inflight).ok_or_else(|| self.exhaust())?;
        Ok(Publication(self))
    }

    fn capture_error(&self) -> Option<NativeEditError> {
        if self.exhausted.load(SeqCst) {
            Some(NativeEditError::Exhausted)
        } else if self.invalid.load(SeqCst) {
            Some(NativeEditError::Invalidated)
        } else if self.lost.load(SeqCst) {
            Some(NativeEditError::DeliveryLost)
        } else if self.inflight.load(SeqCst) != 0 {
            Some(NativeEditError::PublicationInFlight)
        } else if self.processing.load(SeqCst) {
            Some(NativeEditError::ProcessInFlight)
        } else {
            None
        }
    }
}

// Keep the crate's Rust 1.85 MSRV; AtomicU64::try_update is newer, and
// fetch_update is deprecated by newer toolchains. Only producer/control paths retry.
fn increment(counter: &AtomicU64) -> Option<u64> {
    let mut current = counter.load(SeqCst);
    loop {
        let next = current.checked_add(1)?;
        match counter.compare_exchange_weak(current, next, SeqCst, SeqCst) {
            Ok(_) => return Some(next),
            Err(observed) => current = observed,
        }
    }
}

/// The publication fence covers dirtying, lock acquisition, queue publication, and
/// watermark publication, including every rejection path. No Arc clone on this path.
struct Publication<'a>(&'a Status);
impl Drop for Publication<'_> {
    fn drop(&mut self) {
        self.0.inflight.fetch_sub(1, SeqCst);
    }
}

struct SenderInner {
    producer: Mutex<Producer<NativeEdit>>,
    status: Arc<Status>,
}

#[derive(Clone)]
pub(crate) struct NativeEditSender {
    inner: Arc<SenderInner>,
}

pub(crate) struct NativeEditReceiver {
    consumer: Consumer<NativeEdit>,
    status: Arc<Status>,
    staging: Vec<NativeEdit>,
    capacity: usize,
    process_active: bool,
}

pub(crate) fn native_edit_channel(capacity: usize) -> (NativeEditSender, NativeEditReceiver) {
    assert!(capacity > 0, "native edit capacity must be positive");
    let (producer, consumer) = RingBuffer::new(capacity);
    let status = Arc::new(Status {
        dirty: AtomicU64::new(0),
        submitted: AtomicU64::new(0),
        applied: AtomicU64::new(0),
        generation: AtomicU64::new(0),
        inflight: AtomicU64::new(0),
        processing: AtomicBool::new(false),
        lost: AtomicBool::new(false),
        exhausted: AtomicBool::new(false),
        invalid: AtomicBool::new(false),
    });
    (
        NativeEditSender {
            inner: Arc::new(SenderInner {
                producer: Mutex::new(producer),
                status: Arc::clone(&status),
            }),
        },
        NativeEditReceiver {
            consumer,
            status,
            staging: Vec::with_capacity(capacity),
            capacity,
            process_active: false,
        },
    )
}

impl NativeEditSender {
    pub(crate) fn mark_dirty(&self) -> Result<u64, NativeEditError> {
        let status = &self.inner.status;
        match increment(&status.dirty) {
            Some(revision) if revision < u64::MAX => Ok(revision),
            _ => Err(status.exhaust()),
        }
    }

    pub(crate) fn dirty_revision(&self) -> Result<u64, NativeEditError> {
        if self.inner.status.exhausted.load(SeqCst) {
            return Err(NativeEditError::Exhausted);
        }
        let revision = self.inner.status.dirty.load(SeqCst);
        if revision == u64::MAX || self.inner.status.exhausted.load(SeqCst) {
            Err(self.inner.status.exhaust())
        } else {
            Ok(revision)
        }
    }

    /// A linearizable readiness sample, not a long-lived capture lock. The owner must
    /// sample again after serializing state and compare revisions before accepting it.
    pub(crate) fn capture_revision(&self) -> Result<u64, NativeEditError> {
        let status = &self.inner.status;
        if let Some(error) = status.capture_error() {
            return Err(error);
        }
        let revision = self.dirty_revision()?;
        let generation = status.generation.load(SeqCst);
        let submitted = status.submitted.load(SeqCst);
        if submitted != status.applied.load(SeqCst) {
            return Err(NativeEditError::PendingEdits);
        }
        // Double-collect counters as well as fences: a complete publish/process or
        // supersession between the first samples must not produce a false clean read.
        if revision != status.dirty.load(SeqCst)
            || generation != status.generation.load(SeqCst)
            || submitted != status.submitted.load(SeqCst)
            || submitted != status.applied.load(SeqCst)
        {
            return Err(NativeEditError::PublicationInFlight);
        }
        if let Some(error) = status.capture_error() {
            return Err(error);
        }
        Ok(revision)
    }

    /// Every attempt dirties before rejecting, including full/busy/disconnected input.
    pub(crate) fn send(&self, id: u32, value: f64) -> Result<(), NativeEditError> {
        let status = &self.inner.status;
        let entry_generation = status.generation.load(SeqCst);
        let publication = status.begin_publication();
        let dirty = self.mark_dirty();
        let _publication = publication?;
        dirty?;
        self.publish(entry_generation, id, value)
    }

    fn publish(&self, entry_generation: u64, id: u32, value: f64) -> Result<(), NativeEditError> {
        let status = &self.inner.status;
        if status.invalid.load(SeqCst) {
            return Err(NativeEditError::Invalidated);
        }
        if status.exhausted.load(SeqCst) {
            return Err(NativeEditError::Exhausted);
        }
        let mut producer = self.inner.producer.try_lock().map_err(|error| {
            status.lose();
            match error {
                TryLockError::WouldBlock => NativeEditError::ProducerBusy,
                TryLockError::Poisoned(_) => NativeEditError::ProducerPoisoned,
            }
        })?;
        if status.invalid.load(SeqCst) {
            return Err(NativeEditError::Invalidated);
        }
        if entry_generation != status.generation.load(SeqCst) {
            status.lose();
            return Err(NativeEditError::GenerationChanged);
        }
        if producer.is_abandoned() {
            status.lose();
            return Err(NativeEditError::Disconnected);
        }
        let sequence = status
            .submitted
            .load(SeqCst)
            .checked_add(1)
            .ok_or_else(|| status.exhaust())?;
        let edit = NativeEdit {
            generation: entry_generation,
            sequence,
            id,
            value,
        };
        producer.push(edit).map_err(|_| {
            status.lose();
            NativeEditError::Full
        })?;
        status.submitted.store(sequence, SeqCst);
        Ok(())
    }

    /// Call only at the successful state-application queue-clear boundary, with the
    /// receiver quiescent. Never keep a producer guard across a plugin call.
    ///
    /// Old packets are intentionally superseded, NOT acknowledged as processed. A
    /// failed post-application lock cannot safely replay them: it permanently closes
    /// native input and fails capture. Any publisher already holding the lock may
    /// finish, but invalidated receiver paths will only discard its packets.
    pub(crate) fn supersede(
        &self,
        receiver: &mut NativeEditReceiver,
    ) -> Result<(), NativeEditError> {
        let status = &self.inner.status;
        if !Arc::ptr_eq(status, &receiver.status) {
            return Err(NativeEditError::ChannelMismatch);
        }
        let publication = status.begin_publication();
        let _publication = match publication {
            Ok(publication) => publication,
            Err(error) => {
                receiver.invalidate();
                return Err(error);
            }
        };
        if status.invalid.load(SeqCst) {
            receiver.discard();
            return Err(NativeEditError::Invalidated);
        }
        let _producer = match self.inner.producer.try_lock() {
            Ok(producer) => producer,
            Err(_) => {
                receiver.invalidate();
                return Err(NativeEditError::SupersessionFailed);
            }
        };
        // A publisher can be paused before try_lock rather than inside the lock.
        // Treat that race as a partial-restore failure too. The send entry-generation
        // check also covers a publisher that has not raised its fence yet.
        if status.inflight.load(SeqCst) != 1 {
            receiver.invalidate();
            return Err(NativeEditError::SupersessionFailed);
        }
        let Some(generation) = status.generation.load(SeqCst).checked_add(1) else {
            let error = status.exhaust();
            receiver.invalidate();
            return Err(error);
        };
        receiver.discard();
        status.generation.store(generation, SeqCst);
        status.submitted.store(0, SeqCst);
        status.applied.store(0, SeqCst);
        // Lifetime lost/exhausted flags are intentionally not reset.
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> NativeEditSnapshot {
        let status = &self.inner.status;
        NativeEditSnapshot {
            generation: status.generation.load(SeqCst),
            submitted: status.submitted.load(SeqCst),
            applied: status.applied.load(SeqCst),
            dirty: status.dirty.load(SeqCst),
            lost: status.lost.load(SeqCst),
            invalid: status.invalid.load(SeqCst),
            inflight: status.inflight.load(SeqCst),
            processing: status.processing.load(SeqCst),
            exhausted: status.exhausted.load(SeqCst),
        }
    }

    #[cfg(test)]
    pub(crate) fn test_set_dirty_revision(&self, revision: u64) {
        self.inner.status.dirty.store(revision, SeqCst);
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct NativeEditSnapshot {
    pub generation: u64,
    pub submitted: u64,
    pub applied: u64,
    pub dirty: u64,
    pub lost: bool,
    pub invalid: bool,
    pub inflight: u64,
    pub processing: bool,
    pub exhausted: bool,
}

impl NativeEditReceiver {
    fn discard(&mut self) {
        self.staging.clear();
        self.process_active = false;
        self.status.processing.store(false, SeqCst);
        for _ in 0..self.capacity {
            if self.consumer.pop().is_err() {
                break;
            }
        }
    }

    fn invalidate(&mut self) {
        // Publish permanent loss BEFORE invalidating or discarding. A racing capture
        // must never mistake the discarded pending work for acknowledged work.
        self.status.lose();
        self.status.invalid.store(true, SeqCst);
        self.discard();
    }

    fn abandon(&mut self) {
        if !self.staging.is_empty() {
            self.status.lose();
        }
        self.staging.clear();
        self.process_active = false;
        self.status.processing.store(false, SeqCst);
    }

    /// Stage at most capacity packets for ONE actual SDK process invocation. The
    /// callback must only admit the edit into process input; false aborts this batch.
    /// FIFO and repeated same-parameter changes are preserved without coalescing.
    pub(crate) fn stage(
        &mut self,
        mut admit: impl FnMut(NativeEdit) -> bool,
    ) -> Result<(), NativeEditError> {
        if self.status.invalid.load(SeqCst) {
            self.discard();
            return Err(NativeEditError::Invalidated);
        }
        if self.process_active {
            self.abandon();
            return Err(NativeEditError::AbandonedProcess);
        }
        self.process_active = true;
        self.status.processing.store(true, SeqCst);
        let generation = self.status.generation.load(SeqCst);
        for _ in 0..self.capacity {
            let Ok(edit) = self.consumer.pop() else {
                break;
            };
            if edit.generation < generation {
                continue;
            }
            if edit.generation != generation {
                self.status.lose();
                self.abandon();
                return Err(NativeEditError::AdmissionRejected);
            }
            self.staging.push(edit);
            if !admit(edit) {
                self.abandon();
                return Err(NativeEditError::AdmissionRejected);
            }
        }
        Ok(())
    }

    pub(crate) fn finish_process(&mut self, success: bool) -> Result<(), NativeEditError> {
        if self.status.invalid.load(SeqCst) {
            self.discard();
            return Err(NativeEditError::Invalidated);
        }
        if !self.process_active {
            return Err(NativeEditError::NoProcess);
        }
        if !success {
            self.abandon();
            return Err(NativeEditError::ProcessFailed);
        }
        if !self.status.lost.load(SeqCst) {
            if let Some(last) = self.staging.last() {
                self.status.applied.store(last.sequence, SeqCst);
            }
        }
        self.staging.clear();
        self.process_active = false;
        self.status.processing.store(false, SeqCst);
        Ok(())
    }
}

impl Drop for NativeEditReceiver {
    fn drop(&mut self) {
        if !self.staging.is_empty() || !self.consumer.is_empty() {
            self.status.lose();
        }
        self.status.processing.store(false, SeqCst);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::{
        alloc::{GlobalAlloc, Layout, System},
        cell::Cell,
        sync::mpsc,
        thread,
        time::{Duration, Instant},
    };

    // Per-thread counters avoid noise from concurrent tests or the intentionally
    // blocked control/display workers. Const TLS has no lazy heap initialization.
    thread_local! {
        static COUNTING: Cell<bool> = const { Cell::new(false) };
        static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
        static DEALLOCATIONS: Cell<usize> = const { Cell::new(0) };
        static ALLOCATED_BYTES: Cell<usize> = const { Cell::new(0) };
        static FREED_BYTES: Cell<usize> = const { Cell::new(0) };
    }

    struct CountingAllocator;
    #[global_allocator]
    static ALLOCATOR: CountingAllocator = CountingAllocator;

    unsafe impl GlobalAlloc for CountingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            if COUNTING.try_with(Cell::get).unwrap_or(false) {
                let _ = ALLOCATIONS.try_with(|count| count.set(count.get() + 1));
                let _ = ALLOCATED_BYTES.try_with(|count| count.set(count.get() + layout.size()));
            }
            unsafe { System.alloc(layout) }
        }
        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            if COUNTING.try_with(Cell::get).unwrap_or(false) {
                let _ = DEALLOCATIONS.try_with(|count| count.set(count.get() + 1));
                let _ = FREED_BYTES.try_with(|count| count.set(count.get() + layout.size()));
            }
            unsafe { System.dealloc(pointer, layout) }
        }
        unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            if COUNTING.try_with(Cell::get).unwrap_or(false) {
                let _ = ALLOCATIONS.try_with(|count| count.set(count.get() + 1));
                let _ = DEALLOCATIONS.try_with(|count| count.set(count.get() + 1));
                let _ = ALLOCATED_BYTES.try_with(|count| count.set(count.get() + size));
                let _ = FREED_BYTES.try_with(|count| count.set(count.get() + layout.size()));
            }
            unsafe { System.realloc(pointer, layout, size) }
        }
    }

    pub(crate) fn allocation_free<T>(run: impl FnOnce() -> T) -> T {
        ALLOCATIONS.with(|count| count.set(0));
        DEALLOCATIONS.with(|count| count.set(0));
        COUNTING.with(|active| assert!(!active.replace(true)));
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                COUNTING.with(|active| active.set(false));
            }
        }
        let reset = Reset;
        let result = run();
        drop(reset);
        assert_eq!(ALLOCATIONS.with(Cell::get), 0, "channel path allocated");
        assert_eq!(
            DEALLOCATIONS.with(Cell::get),
            0,
            "channel path freed memory"
        );
        result
    }

    /// Test-only constructor accounting; reports requested bytes, not allocator metadata/RSS.
    #[derive(Debug, Clone, Copy)]
    pub(crate) struct AllocationStats {
        pub allocations: usize,
        pub deallocations: usize,
        pub allocated_bytes: usize,
        pub freed_bytes: usize,
    }

    pub(crate) fn measure_allocations<T>(run: impl FnOnce() -> T) -> (T, AllocationStats) {
        ALLOCATIONS.with(|count| count.set(0));
        DEALLOCATIONS.with(|count| count.set(0));
        ALLOCATED_BYTES.with(|count| count.set(0));
        FREED_BYTES.with(|count| count.set(0));
        COUNTING.with(|active| assert!(!active.replace(true)));
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                COUNTING.with(|active| active.set(false));
            }
        }
        let reset = Reset;
        let result = run();
        drop(reset);
        (
            result,
            AllocationStats {
                allocations: ALLOCATIONS.with(Cell::get),
                deallocations: DEALLOCATIONS.with(Cell::get),
                allocated_bytes: ALLOCATED_BYTES.with(Cell::get),
                freed_bytes: FREED_BYTES.with(Cell::get),
            },
        )
    }

    #[test]
    fn transport_endpoints_use_only_derived_thread_safety() {
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}
        assert_send::<NativeEditSender>();
        assert_sync::<NativeEditSender>();
        assert_send::<NativeEditReceiver>();
    }

    #[test]
    fn fifo_wraparound_repeated_parameter_and_ack_after_process() {
        let (sender, mut receiver) = native_edit_channel(4);
        allocation_free(|| {
            for round in 0..128_u64 {
                for offset in 0..4 {
                    sender.send(7, (round * 4 + offset) as f64).unwrap();
                }
                assert_eq!(
                    sender.capture_revision(),
                    Err(NativeEditError::PendingEdits)
                );
                let mut seen = 0;
                receiver
                    .stage(|edit| {
                        assert_eq!(edit.generation, 0);
                        assert_eq!(edit.sequence, round * 4 + seen + 1);
                        assert_eq!(edit.id, 7);
                        assert_eq!(edit.value, (round * 4 + seen) as f64);
                        seen += 1;
                        true
                    })
                    .unwrap();
                assert_eq!(seen, 4);
                assert_eq!(sender.snapshot().applied, round * 4);
                assert_eq!(
                    sender.capture_revision(),
                    Err(NativeEditError::ProcessInFlight)
                );
                receiver.finish_process(true).unwrap();
                assert_eq!(sender.snapshot().applied, (round + 1) * 4);
                assert_eq!(sender.capture_revision(), Ok((round + 1) * 4));
            }
        });
    }

    #[test]
    fn empty_channel_and_first_use_do_not_allocate() {
        let (sender, mut receiver) = native_edit_channel(8);
        allocation_free(|| {
            assert_eq!(sender.capture_revision(), Ok(0));
            assert_eq!(
                receiver.finish_process(true),
                Err(NativeEditError::NoProcess)
            );
            receiver
                .stage(|_| panic!("empty queue admitted a packet"))
                .unwrap();
            assert_eq!(
                sender.capture_revision(),
                Err(NativeEditError::ProcessInFlight)
            );
            receiver.finish_process(true).unwrap();
            assert_eq!(sender.capture_revision(), Ok(0));
            assert_eq!(sender.mark_dirty(), Ok(1));
            assert_eq!(sender.capture_revision(), Ok(1));
        });
    }

    #[test]
    fn refilling_during_stage_is_bounded_by_capacity() {
        let (sender, mut receiver) = native_edit_channel(4);
        allocation_free(|| {
            for id in 0..4 {
                sender.send(id, id as f64).unwrap();
            }
            let mut admitted = 0;
            receiver
                .stage(|edit| {
                    assert_eq!(edit.id, admitted);
                    sender.send(edit.id + 4, edit.value + 4.0).unwrap();
                    admitted += 1;
                    true
                })
                .unwrap();
            assert_eq!(admitted, 4);
            receiver.finish_process(true).unwrap();
            assert_eq!(sender.snapshot().applied, 4);
            assert_eq!(sender.snapshot().submitted, 8);
            assert_eq!(
                sender.capture_revision(),
                Err(NativeEditError::PendingEdits)
            );
            receiver
                .stage(|edit| {
                    assert_eq!(edit.id, admitted);
                    admitted += 1;
                    true
                })
                .unwrap();
            receiver.finish_process(true).unwrap();
            assert_eq!(admitted, 8);
            assert_eq!(sender.capture_revision(), Ok(8));
        });
    }

    #[test]
    fn full_is_dirty_sticky_and_never_acknowledged_by_later_success() {
        let (sender, mut receiver) = native_edit_channel(2);
        allocation_free(|| {
            sender.send(1, 0.1).unwrap();
            sender.send(1, 0.2).unwrap();
            assert_eq!(sender.send(1, 0.3), Err(NativeEditError::Full));
            assert_eq!(sender.dirty_revision(), Ok(3));
            assert_eq!(sender.snapshot().submitted, 2);
            receiver.stage(|_| true).unwrap();
            receiver.finish_process(true).unwrap();
            sender.send(1, 0.4).unwrap();
            receiver.stage(|_| true).unwrap();
            receiver.finish_process(true).unwrap();
            assert_eq!(sender.snapshot().applied, 0);
            assert_eq!(
                sender.capture_revision(),
                Err(NativeEditError::DeliveryLost)
            );
            sender.supersede(&mut receiver).unwrap();
            assert_eq!(sender.snapshot().generation, 1);
            assert_eq!(sender.snapshot().submitted, 0);
            assert_eq!(sender.snapshot().applied, 0);
            assert_eq!(
                sender.capture_revision(),
                Err(NativeEditError::DeliveryLost)
            );
        });
    }

    #[test]
    fn busy_producer_rejects_without_wait_or_allocation_and_dirties() {
        let (sender, _receiver) = native_edit_channel(2);
        let _held = sender.inner.producer.lock().unwrap();
        allocation_free(|| {
            assert_eq!(sender.send(1, 0.5), Err(NativeEditError::ProducerBusy));
            assert_eq!(sender.snapshot().dirty, 1);
            assert_eq!(sender.snapshot().submitted, 0);
            assert_eq!(sender.snapshot().inflight, 0);
            assert_eq!(
                sender.capture_revision(),
                Err(NativeEditError::DeliveryLost)
            );
        });
    }

    #[test]
    fn disconnected_endpoints_are_bounded_and_do_not_allocate() {
        let (sender, receiver) = native_edit_channel(2);
        drop(receiver);
        allocation_free(|| {
            assert_eq!(sender.send(1, 0.5), Err(NativeEditError::Disconnected));
            assert_eq!(sender.snapshot().dirty, 1);
            assert_eq!(
                sender.capture_revision(),
                Err(NativeEditError::DeliveryLost)
            );
        });
        let (sender, mut receiver) = native_edit_channel(2);
        sender.send(3, 0.75).unwrap();
        drop(sender);
        allocation_free(|| {
            let mut count = 0;
            receiver
                .stage(|edit| {
                    assert_eq!(edit.id, 3);
                    count += 1;
                    true
                })
                .unwrap();
            assert_eq!(count, 1);
            receiver.finish_process(true).unwrap();
            receiver.stage(|_| false).unwrap();
            receiver.finish_process(true).unwrap();
        });
    }

    #[test]
    fn dirty_sequence_generation_and_publication_counters_never_wrap() {
        let (sender, mut receiver) = native_edit_channel(2);
        sender.test_set_dirty_revision(u64::MAX - 1);
        allocation_free(|| {
            assert_eq!(sender.mark_dirty(), Err(NativeEditError::Exhausted));
            assert_eq!(sender.mark_dirty(), Err(NativeEditError::Exhausted));
            assert_eq!(sender.send(1, 0.1), Err(NativeEditError::Exhausted));
            assert_eq!(sender.dirty_revision(), Err(NativeEditError::Exhausted));
            assert_eq!(sender.snapshot().dirty, u64::MAX);
            assert_eq!(sender.snapshot().inflight, 0);
            sender.supersede(&mut receiver).unwrap();
            assert_eq!(sender.capture_revision(), Err(NativeEditError::Exhausted));
            assert!(!sender.snapshot().invalid);
        });
        let (sender, mut receiver) = native_edit_channel(2);
        sender.inner.status.submitted.store(u64::MAX - 1, SeqCst);
        sender.inner.status.applied.store(u64::MAX - 1, SeqCst);
        allocation_free(|| {
            sender.send(1, 0.1).unwrap();
            receiver.stage(|edit| edit.sequence == u64::MAX).unwrap();
            receiver.finish_process(true).unwrap();
            assert_eq!(sender.snapshot().applied, u64::MAX);
            assert_eq!(sender.send(1, 0.2), Err(NativeEditError::Exhausted));
            assert_eq!(sender.snapshot().submitted, u64::MAX);
            assert_eq!(sender.capture_revision(), Err(NativeEditError::Exhausted));
        });
        let (sender, mut receiver) = native_edit_channel(2);
        sender.inner.status.generation.store(u64::MAX, SeqCst);
        allocation_free(|| {
            assert_eq!(
                sender.supersede(&mut receiver),
                Err(NativeEditError::Exhausted)
            );
            assert_eq!(sender.snapshot().generation, u64::MAX);
            assert!(sender.snapshot().invalid);
        });
        let (sender, _receiver) = native_edit_channel(2);
        sender.inner.status.inflight.store(u64::MAX, SeqCst);
        allocation_free(|| {
            assert_eq!(sender.send(1, 0.2), Err(NativeEditError::Exhausted));
            assert_eq!(sender.snapshot().inflight, u64::MAX);
            assert_eq!(sender.snapshot().dirty, 1);
        });
    }

    #[test]
    fn admission_and_process_failure_freeze_acknowledgement() {
        for reject_admission in [true, false] {
            let (sender, mut receiver) = native_edit_channel(3);
            allocation_free(|| {
                sender.send(1, 0.1).unwrap();
                sender.send(2, 0.2).unwrap();
                if reject_admission {
                    assert_eq!(
                        receiver.stage(|edit| edit.id == 1),
                        Err(NativeEditError::AdmissionRejected)
                    );
                    assert_eq!(
                        receiver.finish_process(true),
                        Err(NativeEditError::NoProcess)
                    );
                } else {
                    receiver.stage(|_| true).unwrap();
                    assert_eq!(
                        receiver.finish_process(false),
                        Err(NativeEditError::ProcessFailed)
                    );
                }
                assert_eq!(sender.snapshot().applied, 0);
                sender.send(3, 0.3).unwrap();
                receiver.stage(|_| true).unwrap();
                receiver.finish_process(true).unwrap();
                assert_eq!(sender.snapshot().applied, 0);
                assert_eq!(
                    sender.capture_revision(),
                    Err(NativeEditError::DeliveryLost)
                );
            });
        }
    }

    #[test]
    fn empty_failed_process_preserves_capture_and_newly_queued_edits() {
        let (sender, mut receiver) = native_edit_channel(2);
        allocation_free(|| {
            receiver.stage(|_| true).unwrap();
            assert_eq!(
                receiver.finish_process(false),
                Err(NativeEditError::ProcessFailed)
            );
            assert!(!sender.snapshot().lost);
            assert_eq!(sender.capture_revision(), Ok(0));
            receiver.stage(|_| true).unwrap();
            // A callback during the SDK call is future input, not part of the
            // current empty batch and must neither be lost nor acknowledged.
            sender.send(1, 0.5).unwrap();
            assert_eq!(
                receiver.finish_process(false),
                Err(NativeEditError::ProcessFailed)
            );
            assert!(!sender.snapshot().lost);
            assert_eq!(sender.snapshot().applied, 0);
            assert_eq!(
                sender.capture_revision(),
                Err(NativeEditError::PendingEdits)
            );
            receiver.stage(|edit| edit.id == 1).unwrap();
            receiver.finish_process(true).unwrap();
            assert_eq!(sender.capture_revision(), Ok(1));
        });
    }

    #[test]
    fn abandoned_stage_is_sticky_and_unfinished_drop_is_detected() {
        let (sender, mut receiver) = native_edit_channel(2);
        allocation_free(|| {
            sender.send(1, 0.1).unwrap();
            receiver.stage(|_| true).unwrap();
            assert_eq!(
                receiver.stage(|_| true),
                Err(NativeEditError::AbandonedProcess)
            );
            assert_eq!(
                receiver.finish_process(true),
                Err(NativeEditError::NoProcess)
            );
            sender.send(2, 0.2).unwrap();
            receiver.stage(|_| true).unwrap();
            receiver.finish_process(true).unwrap();
            assert_eq!(sender.snapshot().applied, 0);
            assert_eq!(
                sender.capture_revision(),
                Err(NativeEditError::DeliveryLost)
            );
        });
        let (sender, mut receiver) = native_edit_channel(2);
        sender.send(1, 0.1).unwrap();
        receiver.stage(|_| true).unwrap();
        drop(receiver);
        assert_eq!(
            sender.capture_revision(),
            Err(NativeEditError::DeliveryLost)
        );
    }

    #[test]
    fn supersession_discards_queued_and_staged_intent_without_claiming_applied() {
        let (sender, mut receiver) = native_edit_channel(3);
        allocation_free(|| {
            sender.send(1, 0.1).unwrap();
            receiver.stage(|_| true).unwrap();
            sender.send(2, 0.2).unwrap();
            sender.supersede(&mut receiver).unwrap();
            let status = sender.snapshot();
            assert_eq!(status.generation, 1);
            assert_eq!(status.submitted, 0);
            assert_eq!(status.applied, 0);
            assert!(!status.lost);
            assert_eq!(
                receiver.finish_process(true),
                Err(NativeEditError::NoProcess)
            );
            assert_eq!(sender.capture_revision(), Ok(2));
            receiver
                .stage(|_| panic!("old state intent replayed"))
                .unwrap();
            receiver.finish_process(true).unwrap();
            sender.send(3, 0.3).unwrap();
            receiver
                .stage(|edit| {
                    assert_eq!(edit.generation, 1);
                    assert_eq!(edit.sequence, 1);
                    assert_eq!(edit.id, 3);
                    true
                })
                .unwrap();
            receiver.finish_process(true).unwrap();
            assert_eq!(sender.snapshot().applied, 1);
            assert_eq!(sender.capture_revision(), Ok(3));
        });
    }

    #[test]
    fn mismatched_supersession_leaves_both_channels_unchanged() {
        let (sender, _receiver) = native_edit_channel(2);
        let (other_sender, mut other_receiver) = native_edit_channel(2);
        let before = sender.snapshot();
        let other_before = other_sender.snapshot();
        allocation_free(|| {
            assert_eq!(
                sender.supersede(&mut other_receiver),
                Err(NativeEditError::ChannelMismatch)
            );
            assert_eq!(sender.snapshot(), before);
            assert_eq!(other_sender.snapshot(), other_before);
        });
    }

    #[test]
    fn inflight_publication_is_fenced_and_contended_supersession_closes_input() {
        let (sender, mut receiver) = native_edit_channel(2);
        let publisher = sender.clone();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            // Explicit latches stop publication AFTER dirtying and acquiring the
            // producer but BEFORE publishing its ring entry and submitted watermark.
            let publication = publisher.inner.status.begin_publication().unwrap();
            publisher.mark_dirty().unwrap();
            let mut producer = publisher.inner.producer.lock().unwrap();
            ready_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            producer
                .push(NativeEdit {
                    generation: 0,
                    sequence: 1,
                    id: 9,
                    value: 0.9,
                })
                .unwrap();
            publisher.inner.status.submitted.store(1, SeqCst);
            drop(producer);
            drop(publication);
        });
        ready_rx.recv().unwrap();
        allocation_free(|| {
            assert_eq!(
                sender.capture_revision(),
                Err(NativeEditError::PublicationInFlight)
            );
            assert_eq!(
                sender.supersede(&mut receiver),
                Err(NativeEditError::SupersessionFailed)
            );
            assert_eq!(sender.snapshot().generation, 0);
            assert_eq!(sender.snapshot().submitted, 0);
            assert_eq!(sender.snapshot().applied, 0);
            assert!(sender.snapshot().lost);
            assert!(sender.snapshot().invalid);
        });
        release_tx.send(()).unwrap();
        worker.join().unwrap();
        allocation_free(|| {
            assert_eq!(sender.snapshot().inflight, 0);
            assert_eq!(sender.snapshot().submitted, 1);
            assert_eq!(
                receiver.stage(|_| panic!("post-restore old edit replayed")),
                Err(NativeEditError::Invalidated)
            );
            assert_eq!(sender.send(10, 1.0), Err(NativeEditError::Invalidated));
            assert_eq!(sender.snapshot().dirty, 2);
            assert_eq!(sender.snapshot().applied, 0);
            assert_eq!(sender.capture_revision(), Err(NativeEditError::Invalidated));
        });
    }

    #[test]
    fn publisher_paused_before_lock_forces_failed_closed_supersession() {
        let (sender, mut receiver) = native_edit_channel(2);
        let publisher = sender.clone();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let generation = publisher.inner.status.generation.load(SeqCst);
            let _publication = publisher.inner.status.begin_publication().unwrap();
            publisher.mark_dirty().unwrap();
            ready_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            allocation_free(|| {
                assert_eq!(
                    publisher.publish(generation, 9, 0.9),
                    Err(NativeEditError::Invalidated)
                );
            });
        });
        ready_rx.recv().unwrap();
        allocation_free(|| {
            assert_eq!(
                sender.capture_revision(),
                Err(NativeEditError::PublicationInFlight)
            );
            // The producer mutex is free: the independent in-flight fence, not
            // mutex contention, must reject this post-application supersession.
            assert_eq!(
                sender.supersede(&mut receiver),
                Err(NativeEditError::SupersessionFailed)
            );
            assert!(sender.snapshot().invalid);
            assert_eq!(sender.snapshot().generation, 0);
            assert_eq!(sender.snapshot().submitted, 0);
        });
        release_tx.send(()).unwrap();
        worker.join().unwrap();
        allocation_free(|| {
            assert_eq!(
                receiver.stage(|_| panic!("old edit replayed")),
                Err(NativeEditError::Invalidated)
            );
            assert_eq!(sender.snapshot().applied, 0);
        });
    }

    #[test]
    fn entry_generation_rejects_publisher_paused_before_raising_its_fence() {
        let (sender, mut receiver) = native_edit_channel(2);
        let publisher = sender.clone();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let generation = publisher.inner.status.generation.load(SeqCst);
            ready_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            allocation_free(|| {
                let _publication = publisher.inner.status.begin_publication().unwrap();
                publisher.mark_dirty().unwrap();
                assert_eq!(
                    publisher.publish(generation, 9, 0.9),
                    Err(NativeEditError::GenerationChanged)
                );
            });
        });
        ready_rx.recv().unwrap();
        allocation_free(|| sender.supersede(&mut receiver).unwrap());
        release_tx.send(()).unwrap();
        worker.join().unwrap();
        allocation_free(|| {
            assert_eq!(sender.snapshot().generation, 1);
            assert_eq!(sender.snapshot().submitted, 0);
            assert_eq!(sender.snapshot().applied, 0);
            assert_eq!(
                sender.capture_revision(),
                Err(NativeEditError::DeliveryLost)
            );
            receiver
                .stage(|_| panic!("old edit entered new generation"))
                .unwrap();
            receiver.finish_process(true).unwrap();
        });
    }

    #[test]
    fn capture_is_fenced_between_ring_push_and_submitted_watermark() {
        let (sender, mut receiver) = native_edit_channel(2);
        let publication = sender.inner.status.begin_publication().unwrap();
        sender.mark_dirty().unwrap();
        sender
            .inner
            .producer
            .lock()
            .unwrap()
            .push(NativeEdit {
                generation: 0,
                sequence: 1,
                id: 1,
                value: 0.5,
            })
            .unwrap();
        allocation_free(|| {
            receiver.stage(|_| true).unwrap();
            receiver.finish_process(true).unwrap();
            assert_eq!(sender.snapshot().applied, 1);
            assert_eq!(sender.snapshot().submitted, 0);
            assert_eq!(
                sender.capture_revision(),
                Err(NativeEditError::PublicationInFlight)
            );
        });
        sender.inner.status.submitted.store(1, SeqCst);
        drop(publication);
        assert_eq!(sender.capture_revision(), Ok(1));
    }

    #[test]
    fn poisoned_producer_fails_closed_without_allocating() {
        let (sender, mut receiver) = native_edit_channel(2);
        let worker_sender = sender.clone();
        let worker = thread::spawn(move || {
            let _producer = worker_sender.inner.producer.lock().unwrap();
            panic!("poison test producer");
        });
        assert!(worker.join().is_err());
        allocation_free(|| {
            assert_eq!(sender.send(1, 0.1), Err(NativeEditError::ProducerPoisoned));
            assert_eq!(
                sender.supersede(&mut receiver),
                Err(NativeEditError::SupersessionFailed)
            );
            assert!(sender.snapshot().invalid);
            assert!(sender.snapshot().lost);
        });
    }

    /// This validates ONLY receiver independence with an allocation-free mock. It
    /// does not run a helper, a COM plugin, or promise a real helper-stall fix.
    #[test]
    fn receiver_finishes_while_producer_and_display_are_independently_blocked_350ms() {
        struct MockProcessor {
            edits: [Option<NativeEdit>; 8],
            len: usize,
            calls: usize,
        }
        impl MockProcessor {
            fn admit(&mut self, edit: NativeEdit) -> bool {
                if self.len == self.edits.len() {
                    return false;
                }
                self.edits[self.len] = Some(edit);
                self.len += 1;
                true
            }
            fn process(&mut self) -> bool {
                self.calls += 1;
                self.len == 8
            }
        }
        let (sender, mut receiver) = native_edit_channel(8);
        for id in 0..8 {
            sender.send(id, id as f64).unwrap();
        }
        let producer_sender = sender.clone();
        let display = Arc::new(Mutex::new(()));
        let display_worker = Arc::clone(&display);
        let (producer_ready_tx, producer_ready_rx) = mpsc::channel();
        let (display_ready_tx, display_ready_rx) = mpsc::channel();
        let (producer_release_tx, producer_release_rx) = mpsc::channel();
        let (display_release_tx, display_release_rx) = mpsc::channel();
        let producer = thread::spawn(move || {
            let _held = producer_sender.inner.producer.lock().unwrap();
            producer_ready_tx.send(()).unwrap();
            producer_release_rx.recv().unwrap();
        });
        let display = thread::spawn(move || {
            let _held = display_worker.lock().unwrap();
            display_ready_tx.send(()).unwrap();
            display_release_rx.recv().unwrap();
        });
        producer_ready_rx.recv().unwrap();
        display_ready_rx.recv().unwrap();
        let held_since = Instant::now();
        let (finished_tx, finished_rx) = mpsc::channel();
        let processor = thread::spawn(move || {
            let mut mock = MockProcessor {
                edits: [None; 8],
                len: 0,
                calls: 0,
            };
            allocation_free(|| {
                receiver.stage(|edit| mock.admit(edit)).unwrap();
                let success = mock.process();
                receiver.finish_process(success).unwrap();
                assert_eq!(mock.calls, 1);
                for (index, edit) in mock.edits.iter().enumerate() {
                    assert_eq!(edit.unwrap().id, index as u32);
                }
            });
            finished_tx.send(()).unwrap();
            receiver
        });
        // Both release latches remain closed for the entire observation window.
        // Receiving completion before opening either latch proves independence; a
        // strict 350 ms scheduling deadline would instead be flaky under CPU load.
        let finished_before_release = finished_rx.recv_timeout(Duration::from_secs(3)).is_ok();
        let remaining_hold = Duration::from_millis(350).saturating_sub(held_since.elapsed());
        thread::sleep(remaining_hold);
        producer_release_tx.send(()).unwrap();
        display_release_tx.send(()).unwrap();
        producer.join().unwrap();
        display.join().unwrap();
        let _receiver = processor.join().unwrap();
        assert!(
            finished_before_release,
            "receiver waited for a control/display lock"
        );
        assert_eq!(sender.capture_revision(), Ok(8));
    }
}
