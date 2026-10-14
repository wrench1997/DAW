//! Synthetic byte-spool regressions, no SDK/observer/native source claim.
use super::stream_spool::*;
use std::{
    fs, io,
    sync::atomic::{AtomicU64, Ordering},
    sync::{Arc, Condvar, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

fn finish_reusable(spool: &mut BoundedSpool, mut progress: SubmitProgress) -> Vec<u8> {
    let bound = Instant::now() + Duration::from_secs(10);
    loop {
        match progress {
            SubmitProgress::Submitted { buffer } => return buffer,
            SubmitProgress::Pending => {
                assert!(Instant::now() < bound, "healthy spool failed to resume");
                thread::yield_now();
                progress = spool
                    .poll_submit_reusable()
                    .unwrap_or_else(|error| panic!("resume failed: {:?}", error.failure));
            }
        }
    }
}

fn config() -> SpoolConfig {
    SpoolConfig {
        slabs: 64,
        slab_bytes: 128,
        queue_capacity: 64,
        max_operand_bytes: 64 * 1024,
        max_file_bytes: 100_000,
        max_records: 32,
    }
}
fn operand(sequence: u64, offset: u64, bytes: Vec<u8>) -> Operand {
    Operand {
        record_sequence: sequence,
        file_offset: offset,
        bytes,
    }
}
fn wait_join(spool: &mut BoundedSpool) {
    let bound = Instant::now() + Duration::from_secs(10);
    while !spool.poll_join() {
        assert!(
            Instant::now() < bound,
            "actual writer JoinHandle did not finish"
        );
        thread::yield_now();
    }
}
fn wait_recovery(handle: &ShutdownHandle) {
    let bound = Instant::now() + Duration::from_secs(10);
    loop {
        match handle.poll_join() {
            ShutdownPoll::Joined => break,
            ShutdownPoll::Pending | ShutdownPoll::Busy => {
                assert!(
                    Instant::now() < bound,
                    "retained actual writer JoinHandle did not finish"
                );
                thread::yield_now();
            }
            ShutdownPoll::NoRetainedOwner => panic!("recovery handle has no retained writer owner"),
        }
    }
}

struct MemorySink {
    bytes: Arc<Mutex<Vec<u8>>>,
    max_write: usize,
    fail_after: Option<usize>,
    flush_error: bool,
    sync_error: bool,
    length_delta: u64,
}
impl MemorySink {
    fn normal(bytes: Arc<Mutex<Vec<u8>>>) -> Self {
        Self {
            bytes,
            max_write: usize::MAX,
            fail_after: None,
            flush_error: false,
            sync_error: false,
            length_delta: 0,
        }
    }
}
impl SpoolSink for MemorySink {
    fn write_bytes(&mut self, input: &[u8]) -> io::Result<usize> {
        let mut bytes = self.bytes.lock().unwrap();
        if self.fail_after.is_some_and(|limit| bytes.len() >= limit) {
            return Err(io::Error::other("deterministic partial disk failure"));
        }
        let n = input
            .len()
            .min(self.max_write)
            .min(self.fail_after.map_or(usize::MAX, |l| l - bytes.len()));
        bytes.extend_from_slice(&input[..n]);
        Ok(n)
    }
    fn flush_bytes(&mut self) -> io::Result<()> {
        if self.flush_error {
            Err(io::Error::other("deterministic flush failure"))
        } else {
            Ok(())
        }
    }
    fn sync_bytes(&mut self) -> io::Result<()> {
        if self.sync_error {
            Err(io::Error::other("deterministic sync failure"))
        } else {
            Ok(())
        }
    }
    fn actual_length(&mut self) -> io::Result<u64> {
        Ok(self.bytes.lock().unwrap().len() as u64 + self.length_delta)
    }
}

type SharedBytes = Arc<Mutex<Vec<u8>>>;
type SharedGate = Arc<(Mutex<bool>, Condvar)>;
struct GateRelease(SharedGate);
impl Drop for GateRelease {
    fn drop(&mut self) {
        open_gate(self);
    }
}
type GateParts = (
    Box<dyn SpoolSink>,
    GateRelease,
    mpsc::Receiver<()>,
    SharedBytes,
);

struct GateSink {
    delegate: Box<dyn SpoolSink>,
    gate: SharedGate,
    entered: Option<mpsc::SyncSender<()>>,
    panic_after_gate: bool,
}
impl SpoolSink for GateSink {
    fn write_bytes(&mut self, input: &[u8]) -> io::Result<usize> {
        if let Some(entered) = self.entered.take() {
            entered.send(()).unwrap();
            let (open, changed) = &*self.gate;
            let mut open = open.lock().unwrap();
            while !*open {
                open = changed.wait(open).unwrap();
            }
            drop(open);
            if self.panic_after_gate {
                panic!("deterministic writer operation panic");
            }
        }
        self.delegate.write_bytes(input)
    }
    fn flush_bytes(&mut self) -> io::Result<()> {
        self.delegate.flush_bytes()
    }
    fn sync_bytes(&mut self) -> io::Result<()> {
        self.delegate.sync_bytes()
    }
    fn actual_length(&mut self) -> io::Result<u64> {
        self.delegate.actual_length()
    }
}
fn open_gate(release: &GateRelease) {
    *release.0.0.lock().unwrap_or_else(|p| p.into_inner()) = true;
    release.0.1.notify_all();
}
fn gated_sink(
    delegate: Box<dyn SpoolSink>,
    bytes: SharedBytes,
    panic_after_gate: bool,
) -> GateParts {
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let (tx, rx) = mpsc::sync_channel(1);
    (
        Box::new(GateSink {
            delegate,
            gate: gate.clone(),
            entered: Some(tx),
            panic_after_gate,
        }),
        GateRelease(gate),
        rx,
        bytes,
    )
}
fn gate_sink(panic_after_gate: bool) -> GateParts {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    gated_sink(
        Box::new(MemorySink::normal(bytes.clone())),
        bytes,
        panic_after_gate,
    )
}

#[test]
fn arbitrary_large_record_fragments_losslessly_and_requires_actual_join_ack() {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let mut sink = MemorySink::normal(bytes.clone());
    sink.max_write = 3;
    let mut spool = BoundedSpool::with_sink(config(), Box::new(sink)).unwrap();
    let original = (0..5000).map(|n| (n % 251) as u8).collect::<Vec<_>>();
    spool
        .submit(operand(1, 0, original.clone()))
        .unwrap_or_else(|_| panic!("bounded record admission failed"));
    assert!(spool.terminal().is_none());
    spool
        .seal(Expected {
            records: 1,
            bytes: 5000,
        })
        .unwrap();
    wait_join(&mut spool);
    let terminal = spool.terminal().unwrap();
    assert!(terminal.complete());
    assert!(terminal.writer_joined);
    assert_eq!(terminal.ack.records, 1);
    assert_eq!(terminal.ack.bytes, 5000);
    assert_eq!(terminal.ack.complete_fragments, 40);
    assert_eq!(terminal.ack.actual_file_length, Some(5000));
    assert_eq!(*bytes.lock().unwrap(), original);
}

#[test]
fn original_ieee_bits_are_never_sanitized_or_converted_by_spool() {
    let bits = [
        0_u32,
        0x8000_0000,
        0x7fc0_1234,
        0x7f80_0000,
        0xff80_0000,
        0x3f80_0000,
    ];
    let original = bits
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect::<Vec<_>>();
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let mut c = config();
    c.slab_bytes = 3;
    let mut spool =
        BoundedSpool::with_sink(c, Box::new(MemorySink::normal(bytes.clone()))).unwrap();
    spool
        .submit(operand(1, 0, original.clone()))
        .unwrap_or_else(|_| panic!("IEEE byte admission failed"));
    spool
        .seal(Expected {
            records: 1,
            bytes: original.len() as u64,
        })
        .unwrap();
    wait_join(&mut spool);
    assert!(spool.terminal().unwrap().complete());
    assert_eq!(*bytes.lock().unwrap(), original);
}

#[test]
fn queue_full_retains_complete_original_and_exact_unsent_fragment_and_offset() {
    let (sink, gate, entered, bytes) = gate_sink(false);
    let mut c = config();
    c.slabs = 3;
    c.slab_bytes = 4;
    c.queue_capacity = 1;
    let mut spool = BoundedSpool::with_sink(c, sink).unwrap();
    spool
        .submit(operand(1, 0, vec![1, 2, 3, 4]))
        .unwrap_or_else(|_| panic!("first operand failed"));
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    let original = (10..22).collect::<Vec<u8>>();
    let failure = spool.submit(operand(2, 4, original.clone())).unwrap_err();
    assert_eq!(failure.failure.kind, FailureKind::QueueFull);
    assert!(failure.returned_operand.is_none());
    let pending = spool.pending_operand().unwrap();
    assert_eq!(pending.original.bytes, original);
    assert_eq!(pending.original.file_offset, 4);
    assert_eq!(pending.next_unqueued_byte, 4);
    let unqueued = spool.unqueued_fragment().unwrap();
    assert_eq!(unqueued.operand_offset, 4);
    assert_eq!(unqueued.record_sequence, 2);
    assert_eq!(unqueued.original_fragment_bytes(), &original[4..8]);
    open_gate(&gate);
    wait_join(&mut spool);
    let terminal = spool.terminal().unwrap();
    assert!(!terminal.complete());
    assert_eq!(terminal.failure.as_ref().unwrap(), &failure.failure);
    assert_eq!(terminal.retained_fragments.len(), 1);
    assert_eq!(
        terminal.retained_fragments[0].original_fragment_bytes(),
        &original[..4]
    );
    assert_eq!(*bytes.lock().unwrap(), vec![1, 2, 3, 4]);
    assert_eq!(spool.pending_operand().unwrap().original.bytes, original);
}

#[test]
fn partial_disk_failure_preserves_actual_written_prefix_and_all_unwritten_slabs() {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let mut sink = MemorySink::normal(bytes.clone());
    sink.fail_after = Some(3);
    let mut c = config();
    c.slab_bytes = 8;
    let (sink, gate, entered, bytes) = gated_sink(Box::new(sink), bytes, false);
    let mut spool = BoundedSpool::with_sink(c, sink).unwrap();
    let original = (0..24).collect::<Vec<u8>>();
    spool
        .submit(operand(1, 0, original.clone()))
        .unwrap_or_else(|_| panic!("bounded admission failed before gate release"));
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    spool
        .seal(Expected {
            records: 1,
            bytes: 24,
        })
        .unwrap();
    open_gate(&gate);
    wait_join(&mut spool);
    let terminal = spool.terminal().unwrap();
    assert_eq!(terminal.failure.as_ref().unwrap().kind, FailureKind::Io);
    assert!(!terminal.complete());
    assert_eq!(terminal.ack.bytes, 3);
    assert_eq!(terminal.ack.records, 0);
    assert_eq!(terminal.retained_fragments.len(), 3);
    assert_eq!(terminal.retained_fragments[0].written_prefix, 3);
    let retained = terminal
        .retained_fragments
        .iter()
        .flat_map(|f| f.original_fragment_bytes())
        .copied()
        .collect::<Vec<_>>();
    assert_eq!(retained, original);
    assert_eq!(*bytes.lock().unwrap(), vec![0, 1, 2]);
}

#[test]
fn flush_sync_and_actual_length_failures_never_become_complete() {
    for mode in 0..3 {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let mut sink = MemorySink::normal(bytes);
        match mode {
            0 => sink.flush_error = true,
            1 => sink.sync_error = true,
            _ => sink.length_delta = 1,
        }
        let mut spool = BoundedSpool::with_sink(config(), Box::new(sink)).unwrap();
        spool
            .submit(operand(1, 0, vec![9; 128]))
            .unwrap_or_else(|_| panic!("small admission failed"));
        let _ = spool.seal(Expected {
            records: 1,
            bytes: 128,
        });
        wait_join(&mut spool);
        let terminal = spool.terminal().unwrap();
        assert!(terminal.writer_joined);
        assert_eq!(terminal.failure.as_ref().unwrap().kind, FailureKind::Io);
        assert!(!terminal.complete());
    }
}

#[test]
fn late_writer_requires_real_gate_completion_and_join_not_time_or_queue_empty() {
    let (sink, gate, entered, bytes) = gate_sink(false);
    let mut spool = BoundedSpool::with_sink(config(), sink).unwrap();
    spool
        .submit(operand(1, 0, vec![5; 100]))
        .unwrap_or_else(|_| panic!("small admission failed"));
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    spool
        .seal(Expected {
            records: 1,
            bytes: 100,
        })
        .unwrap();
    assert!(!spool.poll_join());
    assert!(spool.terminal().is_none());
    open_gate(&gate);
    wait_join(&mut spool);
    assert!(spool.terminal().unwrap().complete());
    assert_eq!(bytes.lock().unwrap().len(), 100);
}

#[test]
fn dropped_producer_hands_exact_unjoined_owner_to_private_recovery_handle() {
    let (sink, gate, entered, _) = gate_sink(false);
    let mut spool = BoundedSpool::with_sink(config(), sink).unwrap();
    let recovery = spool.recovery_handle();
    spool
        .submit(operand(1, 0, vec![7; 100]))
        .unwrap_or_else(|_| panic!("small admission failed"));
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    drop(spool);
    assert_eq!(recovery.poll_join(), ShutdownPoll::Pending);
    recovery
        .inspect(|_, _, terminal| assert!(terminal.is_none()))
        .unwrap();
    open_gate(&gate);
    wait_recovery(&recovery);
    recovery
        .inspect(|_, _, terminal| {
            let terminal = terminal.unwrap();
            assert!(terminal.writer_joined);
            assert!(!terminal.complete());
            assert_eq!(
                terminal.failure.as_ref().unwrap().kind,
                FailureKind::Abandoned
            );
        })
        .unwrap();
}

#[test]
fn writer_operation_panic_keeps_original_fragment_and_is_joined_as_failed() {
    let (sink, gate, entered, _) = gate_sink(true);
    let mut spool = BoundedSpool::with_sink(config(), sink).unwrap();
    let original = vec![6; 100];
    spool
        .submit(operand(1, 0, original.clone()))
        .unwrap_or_else(|_| panic!("small admission failed"));
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    spool
        .seal(Expected {
            records: 1,
            bytes: 100,
        })
        .unwrap();
    open_gate(&gate);
    wait_join(&mut spool);
    let terminal = spool.terminal().unwrap();
    assert!(terminal.writer_joined);
    assert!(!terminal.complete());
    assert_eq!(
        terminal.failure.as_ref().unwrap().kind,
        FailureKind::WorkerPanic
    );
    assert_eq!(terminal.retained_fragments.len(), 1);
    assert_eq!(
        terminal.retained_fragments[0].original_fragment_bytes(),
        original
    );
}

#[test]
fn rejected_identity_quota_or_wrong_seal_keeps_first_failure_and_returned_operand() {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let mut spool = BoundedSpool::with_sink(config(), Box::new(MemorySink::normal(bytes))).unwrap();
    let original = vec![1; 17];
    let first = spool.submit(operand(2, 0, original.clone())).unwrap_err();
    assert_eq!(first.returned_operand.unwrap().bytes, original);
    let second = spool.submit(operand(1, 0, vec![2; 9])).unwrap_err();
    assert_eq!(second.failure, first.failure);
    assert_eq!(second.returned_operand.unwrap().bytes, vec![2; 9]);
    spool.cancel(Failure {
        kind: FailureKind::Abandoned,
        diagnostic: "later cancellation".into(),
    });
    wait_join(&mut spool);
    assert_eq!(
        spool.terminal().unwrap().failure.as_ref(),
        Some(&first.failure)
    );
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let mut spool = BoundedSpool::with_sink(config(), Box::new(MemorySink::normal(bytes))).unwrap();
    spool
        .submit(operand(1, 0, vec![3; 8]))
        .unwrap_or_else(|_| panic!("small admission failed"));
    assert!(
        spool
            .seal(Expected {
                records: 2,
                bytes: 8
            })
            .is_err()
    );
    wait_join(&mut spool);
    assert!(!spool.terminal().unwrap().complete());
}

#[test]
fn create_new_real_file_is_exclusive_and_readback_matches_all_bytes() {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let path = std::env::temp_dir().join(format!(
        "citrus-private-spool-{}-{}.bin",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    assert!(!path.exists());
    let mut spool = BoundedSpool::create_new(&path, config()).unwrap();
    let original = (0..1025).map(|n| (n % 256) as u8).collect::<Vec<_>>();
    spool
        .submit(operand(1, 0, original.clone()))
        .unwrap_or_else(|_| panic!("file admission failed"));
    spool
        .seal(Expected {
            records: 1,
            bytes: 1025,
        })
        .unwrap();
    wait_join(&mut spool);
    assert!(spool.terminal().unwrap().complete());
    assert_eq!(fs::read(&path).unwrap(), original);
    assert!(BoundedSpool::create_new(&path, config()).is_err());
    assert_eq!(fs::read(&path).unwrap(), original);
    drop(spool);
    fs::remove_file(path).unwrap();
}

#[test]
fn actual_return_channel_disconnect_retains_the_written_original_fragment() {
    let (sink, gate, entered, bytes) = gate_sink(false);
    let mut spool = BoundedSpool::with_sink(config(), sink).unwrap();
    let original = vec![0x5a; 100];
    spool
        .submit(operand(1, 0, original.clone()))
        .unwrap_or_else(|_| panic!("small admission failed"));
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    spool.disconnect_slab_return_receiver_for_test();
    spool
        .seal(Expected {
            records: 1,
            bytes: 100,
        })
        .unwrap();
    open_gate(&gate);
    wait_join(&mut spool);
    let terminal = spool.terminal().unwrap();
    assert!(terminal.writer_joined);
    assert!(!terminal.complete());
    assert_eq!(
        terminal.failure.as_ref().unwrap().kind,
        FailureKind::Disconnected
    );
    assert_eq!(terminal.ack.bytes, 100);
    assert_eq!(terminal.retained_fragments.len(), 1);
    assert_eq!(terminal.retained_fragments[0].written_prefix, 100);
    assert_eq!(
        terminal.retained_fragments[0].original_fragment_bytes(),
        original
    );
    assert_eq!(*bytes.lock().unwrap(), original);
}

struct DropPanicSink(MemorySink);
impl SpoolSink for DropPanicSink {
    fn write_bytes(&mut self, input: &[u8]) -> io::Result<usize> {
        self.0.write_bytes(input)
    }
    fn flush_bytes(&mut self) -> io::Result<()> {
        self.0.flush_bytes()
    }
    fn sync_bytes(&mut self) -> io::Result<()> {
        self.0.sync_bytes()
    }
    fn actual_length(&mut self) -> io::Result<u64> {
        self.0.actual_length()
    }
}
impl Drop for DropPanicSink {
    fn drop(&mut self) {
        panic!("deterministic sink Drop panic");
    }
}

#[test]
fn sink_destruction_panic_is_sticky_even_after_all_byte_flush_and_length_acks() {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let mut spool = BoundedSpool::with_sink(
        config(),
        Box::new(DropPanicSink(MemorySink::normal(bytes.clone()))),
    )
    .unwrap();
    spool
        .submit(operand(1, 0, vec![4; 100]))
        .unwrap_or_else(|_| panic!("small admission failed"));
    spool
        .seal(Expected {
            records: 1,
            bytes: 100,
        })
        .unwrap();
    wait_join(&mut spool);
    let terminal = spool.terminal().unwrap();
    assert!(terminal.writer_joined);
    assert!(!terminal.complete());
    assert_eq!(
        terminal.failure.as_ref().unwrap().kind,
        FailureKind::WorkerPanic
    );
    assert!(terminal.ack.flushed && terminal.ack.synced && terminal.ack.expected_counts_matched);
    assert_eq!(terminal.ack.actual_file_length, Some(100));
    assert_eq!(*bytes.lock().unwrap(), vec![4; 100]);
}

#[test]
fn actual_join_completion_is_revoked_by_later_invalid_input_without_more_output() {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let mut spool =
        BoundedSpool::with_sink(config(), Box::new(MemorySink::normal(bytes.clone()))).unwrap();
    spool
        .submit(operand(1, 0, vec![8; 10]))
        .unwrap_or_else(|_| panic!("small admission failed"));
    spool
        .seal(Expected {
            records: 1,
            bytes: 10,
        })
        .unwrap();
    wait_join(&mut spool);
    assert!(spool.terminal().unwrap().complete());
    let failure = spool.submit(operand(2, 10, vec![9; 10])).unwrap_err();
    assert_eq!(failure.failure.kind, FailureKind::Admission);
    assert_eq!(failure.returned_operand.unwrap().bytes, vec![9; 10]);
    assert!(!spool.terminal().unwrap().complete());
    assert_eq!(
        spool.terminal().unwrap().failure.as_ref(),
        Some(&failure.failure)
    );
    spool.cancel(Failure {
        kind: FailureKind::Abandoned,
        diagnostic: "later cancellation".into(),
    });
    assert_eq!(
        spool.terminal().unwrap().failure.as_ref(),
        Some(&failure.failure)
    );
    assert_eq!(*bytes.lock().unwrap(), vec![8; 10]);
}

#[test]
fn oversized_opaque_operand_is_rejected_and_returned_losslessly_not_truncated() {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let mut spool =
        BoundedSpool::with_sink(config(), Box::new(MemorySink::normal(bytes.clone()))).unwrap();
    // Actual Saved13 capture length. This is not an SDK capture fixture.
    let original = (0..591_772).map(|n| (n % 251) as u8).collect::<Vec<_>>();
    let failure = spool.submit(operand(1, 0, original.clone())).unwrap_err();
    assert_eq!(failure.failure.kind, FailureKind::Admission);
    assert_eq!(failure.returned_operand.unwrap().bytes, original);
    assert!(spool.pending_operand().is_none());
    wait_join(&mut spool);
    assert_eq!(spool.terminal().unwrap().ack.bytes, 0);
    assert!(!spool.terminal().unwrap().complete());
    assert!(bytes.lock().unwrap().is_empty());
}

#[test]
fn small_length_with_overbound_reserved_storage_is_not_admitted_into_pending_owner() {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let mut spool =
        BoundedSpool::with_sink(config(), Box::new(MemorySink::normal(bytes.clone()))).unwrap();
    let mut reserved = Vec::with_capacity(64 * 1024 + 1);
    reserved.extend_from_slice(&[1, 2, 3]);
    let capacity = reserved.capacity();
    let failure = spool.submit(operand(1, 0, reserved)).unwrap_err();
    let returned = failure.returned_operand.unwrap();
    assert_eq!(returned.bytes, vec![1, 2, 3]);
    assert_eq!(returned.bytes.capacity(), capacity);
    assert_eq!(failure.failure.kind, FailureKind::Admission);
    assert!(spool.pending_operand().is_none());
    wait_join(&mut spool);
    assert!(!spool.terminal().unwrap().complete());
    assert!(bytes.lock().unwrap().is_empty());
}

#[test]
fn abandoned_partial_submission_recovery_retains_original_and_exact_unsent_offset() {
    let (sink, gate, entered, _) = gate_sink(false);
    let mut c = config();
    c.slabs = 3;
    c.slab_bytes = 4;
    c.queue_capacity = 1;
    let mut spool = BoundedSpool::with_sink(c, sink).unwrap();
    let recovery = spool.recovery_handle();
    spool
        .submit(operand(1, 0, vec![1; 4]))
        .unwrap_or_else(|_| panic!("first admission failed"));
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    let original = (20..32).collect::<Vec<u8>>();
    let first = spool.submit(operand(2, 4, original.clone())).unwrap_err();
    assert_eq!(first.failure.kind, FailureKind::QueueFull);
    drop(spool);
    recovery
        .inspect(|pending, fragment, terminal| {
            let pending = pending.unwrap();
            assert_eq!(pending.original.bytes, original);
            assert_eq!(pending.next_unqueued_byte, 4);
            let fragment = fragment.unwrap();
            assert_eq!(fragment.operand_offset, 4);
            assert_eq!(fragment.original_fragment_bytes(), &original[4..8]);
            assert!(terminal.is_none());
        })
        .unwrap();
    assert_eq!(recovery.poll_join(), ShutdownPoll::Pending);
    open_gate(&gate);
    wait_recovery(&recovery);
    recovery
        .inspect(|pending, fragment, terminal| {
            assert_eq!(pending.unwrap().original.bytes, original);
            assert_eq!(fragment.unwrap().original_fragment_bytes(), &original[4..8]);
            let terminal = terminal.unwrap();
            assert!(terminal.writer_joined);
            assert!(!terminal.complete());
            assert_eq!(terminal.failure.as_ref(), Some(&first.failure));
            assert_eq!(
                terminal.retained_fragments[0].original_fragment_bytes(),
                &original[..4]
            );
        })
        .unwrap();
}

#[test]
fn sealed_foundation_codec_event_crosses_slabs_and_decodes_all_original_fields() {
    use super::stream_codec::*;
    let dictionary = BaselineDictionary::new(
        7,
        CanonicalMetadata {
            class_uid: "synthetic exact baseline".into(),
            version: "1".into(),
            name: "fixture".into(),
            category: "Instrument".into(),
            audio: AudioLayout::default(),
            events: EventLayout::default(),
            latency_samples: 0,
            tail_samples: u32::MAX,
        },
    )
    .unwrap();
    let event = CanonicalEvent::new(
        901,
        123_456_789,
        STAGE_WHOLE_EXPORT,
        2,
        Payload::RetainedControl {
            kind: 37,
            canonical_bytes: (0..5000).map(|n| (n % 251) as u8).collect(),
        },
    );
    let encoded = encode_event(&event, &dictionary).unwrap();
    // Explicit length envelope belongs to the future reader/adapter, not to
    // spool authority. The generic spool archives every envelope byte.
    let mut framed = (encoded.len() as u32).to_le_bytes().to_vec();
    framed.extend_from_slice(&encoded);
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let mut spool =
        BoundedSpool::with_sink(config(), Box::new(MemorySink::normal(bytes.clone()))).unwrap();
    spool
        .submit(operand(1, 0, framed.clone()))
        .unwrap_or_else(|_| panic!("bounded codec event failed"));
    spool
        .seal(Expected {
            records: 1,
            bytes: framed.len() as u64,
        })
        .unwrap();
    wait_join(&mut spool);
    assert!(spool.terminal().unwrap().complete());
    let archive = bytes.lock().unwrap();
    assert_eq!(*archive, framed);
    let length = u32::from_le_bytes(archive[..4].try_into().unwrap()) as usize;
    assert_eq!(length, archive.len() - 4);
    assert_eq!(
        decode_event(&archive[4..], &dictionary).unwrap().event,
        event
    );
}

#[test]
fn final_recovery_handle_drop_reclaims_the_actually_joined_failed_owner() {
    let (sink, gate, entered, _) = gate_sink(false);
    let mut c = config();
    c.slabs = 3;
    c.slab_bytes = 4;
    c.queue_capacity = 1;
    let mut spool = BoundedSpool::with_sink(c, sink).unwrap();
    let probe = spool.owner_reclaim_probe_for_test();
    let recovery = spool.recovery_handle();
    assert_eq!(recovery.poll_join(), ShutdownPoll::NoRetainedOwner);
    spool
        .submit(operand(1, 0, vec![1; 4]))
        .unwrap_or_else(|_| panic!("first admission failed"));
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    let original = (20..32).collect::<Vec<u8>>();
    let first = spool.submit(operand(2, 4, original.clone())).unwrap_err();
    assert_eq!(first.failure.kind, FailureKind::QueueFull);
    drop(spool);
    assert!(
        probe.upgrade().is_some(),
        "unjoined exact owner must remain retained"
    );
    assert_eq!(recovery.poll_join(), ShutdownPoll::Pending);
    open_gate(&gate);
    wait_recovery(&recovery);
    recovery
        .inspect(|pending, unsent, terminal| {
            assert_eq!(pending.unwrap().original.bytes, original);
            assert_eq!(unsent.unwrap().original_fragment_bytes(), &original[4..8]);
            let terminal = terminal.unwrap();
            assert!(terminal.writer_joined);
            assert!(!terminal.complete());
            assert_eq!(terminal.retained_fragments.len(), 1);
            assert_eq!(
                terminal.retained_fragments[0].original_fragment_bytes(),
                &original[..4]
            );
        })
        .unwrap();
    let last = recovery.clone();
    drop(recovery);
    assert!(
        probe.upgrade().is_some(),
        "remaining recovery handle owns the joined data"
    );
    drop(last);
    assert!(
        probe.upgrade().is_none(),
        "actual joined Inner was leaked by final handle Drop"
    );
}

fn assert_bounded_failure(failure: &Failure, kind: FailureKind, expected: &str) {
    assert_eq!(failure.kind, kind);
    assert_eq!(failure.diagnostic, expected);
    assert!(failure.diagnostic.len() <= 4096);
    assert!(
        failure.diagnostic.capacity() <= 4096,
        "diagnostic retained overbound capacity"
    );
}

#[test]
fn oversized_public_diagnostic_is_normalized_before_shared_terminal_and_returned_clones() {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let mut spool =
        BoundedSpool::with_sink(config(), Box::new(MemorySink::normal(bytes.clone()))).unwrap();
    let mut diagnostic = String::with_capacity(128 * 1024);
    diagnostic.push_str(&"汉".repeat(20_000));
    assert!(diagnostic.len() > 4096 && diagnostic.capacity() > 4096);
    let cause = Failure {
        kind: FailureKind::Io,
        diagnostic,
    };
    let expected = "汉".repeat(1365); // 4095 bytes: never split a UTF-8 scalar.
    assert_bounded_failure(&cause.clone(), FailureKind::Io, &expected);
    spool.cancel(cause);
    assert_bounded_failure(&spool.failure().unwrap(), FailureKind::Io, &expected);
    let rejection = spool.submit(operand(1, 0, vec![4; 3])).unwrap_err();
    assert_eq!(rejection.returned_operand.unwrap().bytes, vec![4; 3]);
    assert_bounded_failure(&rejection.failure, FailureKind::Io, &expected);
    wait_join(&mut spool);
    assert_bounded_failure(
        spool.terminal().unwrap().failure.as_ref().unwrap(),
        FailureKind::Io,
        &expected,
    );
    assert!(spool.terminal().unwrap().writer_joined);
    assert!(!spool.terminal().unwrap().complete());
    assert_eq!(spool.terminal().unwrap().ack.bytes, 0);
    assert!(bytes.lock().unwrap().is_empty());
    spool.cancel(Failure {
        kind: FailureKind::Abandoned,
        diagnostic: "later ".repeat(10_000),
    });
    assert_bounded_failure(
        spool.terminal().unwrap().failure.as_ref().unwrap(),
        FailureKind::Io,
        &expected,
    );
}

#[test]
fn small_public_diagnostic_with_large_capacity_is_reallocated_and_revokes_cached_completion() {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let mut spool =
        BoundedSpool::with_sink(config(), Box::new(MemorySink::normal(bytes.clone()))).unwrap();
    spool
        .submit(operand(1, 0, vec![8; 8]))
        .unwrap_or_else(|_| panic!("small admission failed"));
    spool
        .seal(Expected {
            records: 1,
            bytes: 8,
        })
        .unwrap();
    wait_join(&mut spool);
    assert!(spool.terminal().unwrap().complete());
    let mut diagnostic = String::with_capacity(128 * 1024);
    diagnostic.push_str("abc");
    assert_eq!(diagnostic.len(), 3);
    assert!(diagnostic.capacity() > 4096);
    let cause = Failure {
        kind: FailureKind::Abandoned,
        diagnostic,
    };
    assert_bounded_failure(&cause.clone(), FailureKind::Abandoned, "abc");
    spool.cancel(cause);
    assert_bounded_failure(&spool.failure().unwrap(), FailureKind::Abandoned, "abc");
    assert_bounded_failure(
        spool.terminal().unwrap().failure.as_ref().unwrap(),
        FailureKind::Abandoned,
        "abc",
    );
    assert_bounded_failure(
        &spool
            .seal(Expected {
                records: 1,
                bytes: 8,
            })
            .unwrap_err(),
        FailureKind::Abandoned,
        "abc",
    );
    assert!(!spool.terminal().unwrap().complete());
    assert!(spool.terminal().unwrap().writer_joined);
    assert_eq!(*bytes.lock().unwrap(), vec![8; 8]);
}

#[test]
fn reentrant_recovery_inspection_returns_busy_and_both_actual_threads_are_joined_and_reclaimed() {
    let (sink, gate, entered, _) = gate_sink(false);
    let mut spool = BoundedSpool::with_sink(config(), sink).unwrap();
    let probe = spool.owner_reclaim_probe_for_test();
    let recovery = spool.recovery_handle();
    spool
        .submit(operand(1, 0, vec![7; 100]))
        .unwrap_or_else(|_| panic!("small admission failed"));
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    drop(spool);
    let threaded_handle = recovery.clone();
    let (completed, observed) = mpsc::sync_channel(1);
    let inspection_thread = thread::spawn(move || {
        threaded_handle
            .inspect(|_, _, terminal| {
                assert!(terminal.is_none());
                assert_eq!(threaded_handle.poll_join(), ShutdownPoll::Busy);
                assert_eq!(threaded_handle.inspect(|_, _, _| ()), Err(InspectionBusy));
            })
            .unwrap();
        completed.send(()).unwrap();
        threaded_handle
    });
    observed
        .recv_timeout(Duration::from_secs(5))
        .expect("reentrant recovery API deadlocked");
    let threaded_handle = inspection_thread
        .join()
        .expect("actual inspection thread failed to join");
    assert_eq!(recovery.poll_join(), ShutdownPoll::Pending);
    assert!(probe.upgrade().is_some());
    open_gate(&gate);
    wait_recovery(&recovery);
    recovery
        .inspect(|_, _, terminal| {
            let terminal = terminal.unwrap();
            assert!(terminal.writer_joined);
            assert!(!terminal.complete());
        })
        .unwrap();
    drop(threaded_handle);
    drop(recovery);
    assert!(
        probe.upgrade().is_none(),
        "reentrant inspection left its joined owner unreclaimed"
    );
}

#[test]
fn no_free_slab_retains_original_at_zero_offset_without_inventing_an_unsent_fragment() {
    let (sink, gate, entered, bytes) = gate_sink(false);
    let mut c = config();
    c.slabs = 1;
    c.slab_bytes = 4;
    c.queue_capacity = 1;
    let mut spool = BoundedSpool::with_sink(c, sink).unwrap();
    spool
        .submit(operand(1, 0, vec![1; 4]))
        .unwrap_or_else(|_| panic!("first admission failed"));
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    let original = (10..18).collect::<Vec<u8>>();
    let failure = spool.submit(operand(2, 4, original.clone())).unwrap_err();
    assert_eq!(failure.failure.kind, FailureKind::QueueFull);
    assert!(
        failure
            .failure
            .diagnostic
            .starts_with("no free bounded slab")
    );
    assert!(failure.returned_operand.is_none());
    assert_eq!(spool.pending_operand().unwrap().original.bytes, original);
    assert_eq!(spool.pending_operand().unwrap().next_unqueued_byte, 0);
    assert!(spool.unqueued_fragment().is_none());
    open_gate(&gate);
    wait_join(&mut spool);
    let terminal = spool.terminal().unwrap();
    assert!(terminal.writer_joined);
    assert!(!terminal.complete());
    assert_eq!(terminal.failure.as_ref(), Some(&failure.failure));
    assert_eq!(terminal.ack.bytes, 4);
    assert_eq!(terminal.ack.records, 1);
    assert!(terminal.retained_fragments.is_empty());
    assert_eq!(*bytes.lock().unwrap(), vec![1; 4]);
    assert_eq!(spool.pending_operand().unwrap().original.bytes, original);
}

struct ScriptedWriteSink {
    bytes: SharedBytes,
    script: std::collections::VecDeque<Result<usize, io::ErrorKind>>,
    calls: Arc<AtomicU64>,
}
impl SpoolSink for ScriptedWriteSink {
    fn write_bytes(&mut self, input: &[u8]) -> io::Result<usize> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let count = match self.script.pop_front() {
            Some(Err(kind)) => return Err(io::Error::from(kind)),
            Some(Ok(count)) => count.min(input.len()),
            None => input.len(),
        };
        self.bytes
            .lock()
            .unwrap()
            .extend_from_slice(&input[..count]);
        Ok(count)
    }
    fn flush_bytes(&mut self) -> io::Result<()> {
        Ok(())
    }
    fn sync_bytes(&mut self) -> io::Result<()> {
        Ok(())
    }
    fn actual_length(&mut self) -> io::Result<u64> {
        Ok(self.bytes.lock().unwrap().len() as u64)
    }
}

#[test]
fn interrupted_retries_preserve_bytes_but_write_zero_keeps_only_actual_partial_ack() {
    for write_zero in [false, true] {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let calls = Arc::new(AtomicU64::new(0));
        let script = if write_zero {
            vec![Err(io::ErrorKind::Interrupted), Ok(2), Ok(0)]
        } else {
            vec![
                Err(io::ErrorKind::Interrupted),
                Ok(2),
                Err(io::ErrorKind::Interrupted),
            ]
        };
        let sink = ScriptedWriteSink {
            bytes: bytes.clone(),
            script: script.into(),
            calls: calls.clone(),
        };
        let mut spool = BoundedSpool::with_sink(config(), Box::new(sink)).unwrap();
        let original = (0..8).collect::<Vec<u8>>();
        spool
            .submit(operand(1, 0, original.clone()))
            .unwrap_or_else(|_| panic!("small admission failed"));
        let _ = spool.seal(Expected {
            records: 1,
            bytes: 8,
        });
        wait_join(&mut spool);
        let terminal = spool.terminal().unwrap();
        assert!(terminal.writer_joined);
        if write_zero {
            assert!(!terminal.complete());
            assert_eq!(terminal.failure.as_ref().unwrap().kind, FailureKind::Io);
            assert_eq!(
                terminal.failure.as_ref().unwrap().diagnostic,
                "archive WriteZero"
            );
            assert_eq!(terminal.ack.bytes, 2);
            assert_eq!(terminal.ack.records, 0);
            assert_eq!(terminal.retained_fragments.len(), 1);
            assert_eq!(terminal.retained_fragments[0].written_prefix, 2);
            assert_eq!(
                terminal.retained_fragments[0].original_fragment_bytes(),
                original
            );
            assert_eq!(*bytes.lock().unwrap(), vec![0, 1]);
            assert_eq!(calls.load(Ordering::SeqCst), 3);
        } else {
            assert!(terminal.complete());
            assert_eq!(terminal.ack.bytes, 8);
            assert_eq!(terminal.ack.records, 1);
            assert_eq!(*bytes.lock().unwrap(), original);
            assert_eq!(calls.load(Ordering::SeqCst), 4);
        }
    }
}

#[test]
fn exact_archive_and_record_quotas_complete_but_each_one_past_boundary_is_losslessly_rejected() {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let mut c = config();
    c.max_records = 2;
    c.max_file_bytes = 8;
    let mut spool =
        BoundedSpool::with_sink(c, Box::new(MemorySink::normal(bytes.clone()))).unwrap();
    spool
        .submit(operand(1, 0, vec![1; 4]))
        .unwrap_or_else(|_| panic!("first boundary admission failed"));
    spool
        .submit(operand(2, 4, vec![2; 4]))
        .unwrap_or_else(|_| panic!("exact quota must be admitted"));
    spool
        .seal(Expected {
            records: 2,
            bytes: 8,
        })
        .unwrap();
    wait_join(&mut spool);
    assert!(spool.terminal().unwrap().complete());
    assert_eq!(*bytes.lock().unwrap(), vec![1, 1, 1, 1, 2, 2, 2, 2]);
    for exceed_records in [false, true] {
        let (sink, gate, entered, bytes) = gate_sink(false);
        let mut c = config();
        c.max_records = if exceed_records { 1 } else { 2 };
        c.max_file_bytes = if exceed_records { 9 } else { 8 };
        let mut spool = BoundedSpool::with_sink(c, sink).unwrap();
        spool
            .submit(operand(1, 0, vec![3; 8]))
            .unwrap_or_else(|_| panic!("first boundary admission failed"));
        entered.recv_timeout(Duration::from_secs(5)).unwrap();
        let failure = spool.submit(operand(2, 8, vec![9])).unwrap_err();
        assert_eq!(failure.failure.kind, FailureKind::Admission);
        let returned = failure.returned_operand.unwrap();
        assert_eq!(returned.record_sequence, 2);
        assert_eq!(returned.file_offset, 8);
        assert_eq!(returned.bytes, vec![9]);
        assert!(spool.pending_operand().is_none());
        open_gate(&gate);
        wait_join(&mut spool);
        let terminal = spool.terminal().unwrap();
        assert!(terminal.writer_joined);
        assert!(!terminal.complete());
        assert_eq!(terminal.failure.as_ref(), Some(&failure.failure));
        assert_eq!(terminal.ack.bytes, 8);
        assert_eq!(terminal.ack.records, 1);
        assert_eq!(*bytes.lock().unwrap(), vec![3; 8]);
    }
}

#[test]
fn reusable_pool_pressure_is_pending_then_returns_the_same_allocation_after_real_drain() {
    let (sink, gate, entered, bytes) = gate_sink(false);
    let mut c = config();
    c.slabs = 1;
    c.slab_bytes = 4;
    c.queue_capacity = 1;
    let mut spool = BoundedSpool::with_sink(c, sink).unwrap();
    let mut buffer = Vec::with_capacity(64);
    buffer.extend_from_slice(&[1, 2, 3, 4]);
    let allocation = buffer.as_ptr();
    let capacity = buffer.capacity();
    let deadline = Instant::now() + Duration::from_secs(10);
    let progress = spool
        .submit_reusable(operand(1, 0, buffer), deadline)
        .unwrap_or_else(|error| panic!("first reusable admission failed: {:?}", error.failure));
    let mut buffer = finish_reusable(&mut spool, progress);
    assert!(buffer.is_empty());
    assert_eq!(buffer.as_ptr(), allocation);
    assert_eq!(buffer.capacity(), capacity);
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    buffer.extend(10..18);
    let original = buffer.clone();
    let progress = spool
        .submit_reusable(operand(2, 4, buffer), deadline)
        .unwrap_or_else(|error| panic!("pressure must be healthy: {:?}", error.failure));
    assert!(matches!(progress, SubmitProgress::Pending));
    for _ in 0..100 {
        assert!(matches!(
            spool
                .poll_submit_reusable()
                .unwrap_or_else(|_| panic!("pressure became failure")),
            SubmitProgress::Pending
        ));
        assert!(spool.failure().is_none());
        let pending = spool.pending_operand().unwrap();
        assert_eq!(pending.next_unqueued_byte, 0);
        assert_eq!(pending.original.bytes, original);
        assert_eq!(pending.original.bytes.as_ptr(), allocation);
        assert!(spool.unqueued_fragment().is_none());
        assert!(spool.terminal().is_none());
    }
    open_gate(&gate);
    let buffer = finish_reusable(&mut spool, SubmitProgress::Pending);
    assert!(buffer.is_empty());
    assert_eq!(buffer.as_ptr(), allocation);
    assert_eq!(buffer.capacity(), capacity);
    spool
        .seal(Expected {
            records: 2,
            bytes: 12,
        })
        .unwrap();
    wait_join(&mut spool);
    assert!(spool.terminal().unwrap().complete());
    assert_eq!(spool.terminal().unwrap().ack.complete_fragments, 3);
    let mut expected = vec![1, 2, 3, 4];
    expected.extend_from_slice(&original);
    assert_eq!(*bytes.lock().unwrap(), expected);
}

#[test]
fn reusable_queue_pressure_resends_only_the_exact_unsent_fragment_and_never_an_accepted_prefix() {
    let (sink, gate, entered, bytes) = gate_sink(false);
    let mut c = config();
    c.slabs = 3;
    c.slab_bytes = 4;
    c.queue_capacity = 1;
    let mut spool = BoundedSpool::with_sink(c, sink).unwrap();
    spool
        .submit(operand(1, 0, vec![1; 4]))
        .unwrap_or_else(|_| panic!("first failed"));
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    let original = (10..22).collect::<Vec<u8>>();
    let progress = spool
        .submit_reusable(
            operand(2, 4, original.clone()),
            Instant::now() + Duration::from_secs(10),
        )
        .unwrap_or_else(|error| panic!("queue pressure must be healthy: {:?}", error.failure));
    assert!(matches!(progress, SubmitProgress::Pending));
    // Compare the actual retained allocation, rather than the independent
    // expectation clone allocated above.
    let allocation = spool.pending_operand().unwrap().original.bytes.as_ptr();
    let capacity = spool.pending_operand().unwrap().original.bytes.capacity();
    assert_ne!(allocation, original.as_ptr());
    let unsent = spool.unqueued_fragment().unwrap();
    let unsent_identity = (
        unsent.slot_id,
        unsent.slot_generation,
        unsent.fragment_sequence,
    );
    let unsent_allocation = unsent.original_fragment_bytes().as_ptr();
    assert_eq!(unsent.operand_offset, 4);
    assert_eq!(unsent.original_fragment_bytes(), &original[4..8]);
    for _ in 0..100 {
        assert!(matches!(
            spool
                .poll_submit_reusable()
                .unwrap_or_else(|_| panic!("queue pressure became failure")),
            SubmitProgress::Pending
        ));
        let pending = spool.pending_operand().unwrap();
        assert_eq!(pending.next_unqueued_byte, 4);
        assert_eq!(pending.original.bytes, original);
        let unsent = spool.unqueued_fragment().unwrap();
        assert_eq!(
            (
                unsent.slot_id,
                unsent.slot_generation,
                unsent.fragment_sequence
            ),
            unsent_identity
        );
        assert_eq!(unsent.original_fragment_bytes().as_ptr(), unsent_allocation);
        assert!(spool.failure().is_none());
    }
    open_gate(&gate);
    let buffer = finish_reusable(&mut spool, SubmitProgress::Pending);
    assert!(buffer.is_empty());
    assert_eq!(buffer.as_ptr(), allocation);
    assert_eq!(buffer.capacity(), capacity);
    spool
        .seal(Expected {
            records: 2,
            bytes: 16,
        })
        .unwrap();
    wait_join(&mut spool);
    assert!(spool.terminal().unwrap().complete());
    assert_eq!(spool.terminal().unwrap().ack.complete_fragments, 4);
    let mut expected = vec![1; 4];
    expected.extend_from_slice(&original);
    assert_eq!(*bytes.lock().unwrap(), expected);
}

#[test]
fn reusable_deadline_is_not_refreshed_by_polls_and_keeps_original_partial_owner() {
    let (sink, gate, entered, bytes) = gate_sink(false);
    let mut c = config();
    c.slabs = 3;
    c.slab_bytes = 4;
    c.queue_capacity = 1;
    let mut spool = BoundedSpool::with_sink(c, sink).unwrap();
    spool
        .submit(operand(1, 0, vec![1; 4]))
        .unwrap_or_else(|_| panic!("first failed"));
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    let original = (10..22).collect::<Vec<u8>>();
    let progress = spool
        .submit_reusable(
            operand(2, 4, original.clone()),
            Instant::now() + Duration::from_millis(50),
        )
        .unwrap_or_else(|error| panic!("initial pressure failed: {:?}", error.failure));
    assert!(matches!(progress, SubmitProgress::Pending));
    let allocation = spool.pending_operand().unwrap().original.bytes.as_ptr();
    let unsent_allocation = spool
        .unqueued_fragment()
        .unwrap()
        .original_fragment_bytes()
        .as_ptr();
    // Polls have no replacement deadline argument and cannot extend the owner.
    thread::sleep(Duration::from_millis(60));
    let error = spool.poll_submit_reusable().unwrap_err();
    assert_eq!(error.failure.kind, FailureKind::Deadline);
    assert!(error.returned_operand.is_none());
    let pending = spool.pending_operand().unwrap();
    assert_eq!(pending.original.bytes, original);
    assert_eq!(pending.original.bytes.as_ptr(), allocation);
    assert_eq!(pending.next_unqueued_byte, 4);
    assert_eq!(
        spool
            .unqueued_fragment()
            .unwrap()
            .original_fragment_bytes()
            .as_ptr(),
        unsent_allocation
    );
    assert_eq!(
        spool.poll_submit_reusable().unwrap_err().failure,
        error.failure
    );
    open_gate(&gate);
    wait_join(&mut spool);
    let terminal = spool.terminal().unwrap();
    assert!(terminal.writer_joined);
    assert!(!terminal.complete());
    assert_eq!(terminal.failure.as_ref(), Some(&error.failure));
    assert_eq!(terminal.ack.bytes, 4);
    assert_eq!(*bytes.lock().unwrap(), vec![1; 4]);
    assert_eq!(spool.pending_operand().unwrap().original.bytes, original);
}

#[test]
fn reusable_expired_before_admission_returns_the_original_allocation_without_any_fragment() {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let mut spool =
        BoundedSpool::with_sink(config(), Box::new(MemorySink::normal(bytes.clone()))).unwrap();
    let mut original = Vec::with_capacity(64);
    original.extend(1..10);
    let allocation = original.as_ptr();
    let capacity = original.capacity();
    let error = spool
        .submit_reusable(operand(1, 0, original), Instant::now())
        .unwrap_err();
    assert_eq!(error.failure.kind, FailureKind::Deadline);
    let returned = error.returned_operand.unwrap();
    assert_eq!(returned.bytes.as_ptr(), allocation);
    assert_eq!(returned.bytes.capacity(), capacity);
    assert_eq!(returned.bytes, (1..10).collect::<Vec<_>>());
    assert!(spool.pending_operand().is_none());
    assert!(spool.unqueued_fragment().is_none());
    wait_join(&mut spool);
    assert!(!spool.terminal().unwrap().complete());
    assert_eq!(spool.terminal().unwrap().ack.bytes, 0);
    assert!(bytes.lock().unwrap().is_empty());
}

#[test]
fn reusable_actual_slab_return_disconnect_is_sticky_and_keeps_the_full_owner() {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let mut spool =
        BoundedSpool::with_sink(config(), Box::new(MemorySink::normal(bytes.clone()))).unwrap();
    spool.disconnect_slab_return_receiver_for_test();
    let original = vec![1, 2, 3, 4];
    let error = spool
        .submit_reusable(
            operand(1, 0, original.clone()),
            Instant::now() + Duration::from_secs(10),
        )
        .unwrap_err();
    assert_eq!(error.failure.kind, FailureKind::Disconnected);
    assert!(error.returned_operand.is_none());
    assert_eq!(spool.pending_operand().unwrap().original.bytes, original);
    assert_eq!(spool.pending_operand().unwrap().next_unqueued_byte, 0);
    assert!(spool.unqueued_fragment().is_none());
    wait_join(&mut spool);
    assert!(spool.terminal().unwrap().writer_joined);
    assert!(!spool.terminal().unwrap().complete());
    assert_eq!(spool.terminal().unwrap().ack.bytes, 0);
    assert!(bytes.lock().unwrap().is_empty());
}

#[test]
fn reusable_actual_writer_panic_retains_pending_and_unsent_fragment_without_claiming_completion() {
    let (sink, gate, entered, bytes) = gate_sink(true);
    let mut c = config();
    c.slabs = 3;
    c.slab_bytes = 4;
    c.queue_capacity = 1;
    let mut spool = BoundedSpool::with_sink(c, sink).unwrap();
    spool
        .submit(operand(1, 0, vec![1; 4]))
        .unwrap_or_else(|_| panic!("first failed"));
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    let original = (10..22).collect::<Vec<u8>>();
    let progress = spool
        .submit_reusable(
            operand(2, 4, original.clone()),
            Instant::now() + Duration::from_secs(10),
        )
        .unwrap_or_else(|error| panic!("initial pressure failed: {:?}", error.failure));
    assert!(matches!(progress, SubmitProgress::Pending));
    let unsent_allocation = spool
        .unqueued_fragment()
        .unwrap()
        .original_fragment_bytes()
        .as_ptr();
    open_gate(&gate);
    let bound = Instant::now() + Duration::from_secs(10);
    while spool.failure().is_none() {
        assert!(Instant::now() < bound);
        thread::yield_now();
    }
    let error = spool.poll_submit_reusable().unwrap_err();
    assert_eq!(error.failure.kind, FailureKind::WorkerPanic);
    assert!(error.returned_operand.is_none());
    assert_eq!(spool.pending_operand().unwrap().original.bytes, original);
    assert_eq!(spool.pending_operand().unwrap().next_unqueued_byte, 4);
    assert_eq!(
        spool
            .unqueued_fragment()
            .unwrap()
            .original_fragment_bytes()
            .as_ptr(),
        unsent_allocation
    );
    wait_join(&mut spool);
    let terminal = spool.terminal().unwrap();
    assert!(terminal.writer_joined);
    assert!(!terminal.complete());
    assert_eq!(terminal.failure.as_ref(), Some(&error.failure));
    assert_eq!(terminal.ack.bytes, 0);
    assert_eq!(terminal.retained_fragments.len(), 2);
    assert!(bytes.lock().unwrap().is_empty());
}

#[test]
fn reusable_second_operand_is_rejected_losslessly_while_the_original_pending_owner_stays_intact() {
    let (sink, gate, entered, _) = gate_sink(false);
    let mut c = config();
    c.slabs = 1;
    c.slab_bytes = 4;
    c.queue_capacity = 1;
    let mut spool = BoundedSpool::with_sink(c, sink).unwrap();
    spool
        .submit(operand(1, 0, vec![1; 4]))
        .unwrap_or_else(|_| panic!("first failed"));
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    let original = vec![2; 8];
    let progress = spool
        .submit_reusable(
            operand(2, 4, original.clone()),
            Instant::now() + Duration::from_secs(10),
        )
        .unwrap_or_else(|error| panic!("initial pressure failed: {:?}", error.failure));
    assert!(matches!(progress, SubmitProgress::Pending));
    let rejected = operand(3, 12, vec![3; 4]);
    let allocation = rejected.bytes.as_ptr();
    let error = spool
        .submit_reusable(rejected, Instant::now() + Duration::from_secs(20))
        .unwrap_err();
    assert_eq!(error.failure.kind, FailureKind::Admission);
    let returned = error.returned_operand.unwrap();
    assert_eq!(returned.bytes.as_ptr(), allocation);
    assert_eq!(returned.record_sequence, 3);
    assert_eq!(returned.file_offset, 12);
    assert_eq!(returned.bytes, vec![3; 4]);
    assert_eq!(spool.pending_operand().unwrap().original.bytes, original);
    assert_eq!(spool.pending_operand().unwrap().next_unqueued_byte, 0);
    open_gate(&gate);
    wait_join(&mut spool);
    assert!(!spool.terminal().unwrap().complete());
}

// Synthetic long-run pressure regression. This is byte/order proof for the
// spool and does not claim actual Kontakt processing or 405-note restoration.
struct PatternSink {
    accepted: Arc<AtomicU64>,
    record_bytes: u64,
}
impl SpoolSink for PatternSink {
    fn write_bytes(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let start = self.accepted.load(Ordering::SeqCst);
        for (offset, byte) in bytes.iter().enumerate() {
            let position = start + offset as u64;
            let sequence = position / self.record_bytes + 1;
            let within = position % self.record_bytes;
            if *byte != ((sequence + within) % 251) as u8 {
                return Err(io::Error::other("synthetic archive byte/order differs"));
            }
        }
        self.accepted
            .fetch_add(bytes.len() as u64, Ordering::SeqCst);
        Ok(bytes.len())
    }
    fn flush_bytes(&mut self) -> io::Result<()> {
        Ok(())
    }
    fn sync_bytes(&mut self) -> io::Result<()> {
        Ok(())
    }
    fn actual_length(&mut self) -> io::Result<u64> {
        Ok(self.accepted.load(Ordering::SeqCst))
    }
}

#[test]
fn reusable_continuous_44000_pcm_sized_records_use_one_buffer_and_the_original_64_slab_bounds() {
    const RECORDS: u64 = 44_000;
    const RECORD_BYTES: u64 = 1059;
    let accepted = Arc::new(AtomicU64::new(0));
    let c = SpoolConfig {
        slabs: 64,
        slab_bytes: 2048,
        queue_capacity: 64,
        max_operand_bytes: 64 * 1024,
        max_file_bytes: 192_000_000,
        max_records: 1_000_000,
    };
    let mut spool = BoundedSpool::with_sink(
        c,
        Box::new(PatternSink {
            accepted: accepted.clone(),
            record_bytes: RECORD_BYTES,
        }),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(40);
    let mut buffer = Vec::with_capacity(64 * 1024);
    let allocation = buffer.as_ptr();
    let capacity = buffer.capacity();
    for sequence in 1..=RECORDS {
        for within in 0..RECORD_BYTES {
            buffer.push(((sequence + within) % 251) as u8);
        }
        let progress = spool
            .submit_reusable(
                operand(sequence, (sequence - 1) * RECORD_BYTES, buffer),
                deadline,
            )
            .unwrap_or_else(|error| {
                panic!("long-run admission {sequence} failed: {:?}", error.failure)
            });
        buffer = finish_reusable(&mut spool, progress);
        assert!(buffer.is_empty());
        assert_eq!(buffer.as_ptr(), allocation);
        assert_eq!(buffer.capacity(), capacity);
        assert!(spool.failure().is_none());
    }
    spool
        .seal(Expected {
            records: RECORDS,
            bytes: RECORDS * RECORD_BYTES,
        })
        .unwrap();
    wait_join(&mut spool);
    let terminal = spool.terminal().unwrap();
    assert!(terminal.complete());
    assert_eq!(terminal.ack.records, RECORDS);
    assert_eq!(terminal.ack.complete_fragments, RECORDS);
    assert_eq!(accepted.load(Ordering::SeqCst), RECORDS * RECORD_BYTES);
}
