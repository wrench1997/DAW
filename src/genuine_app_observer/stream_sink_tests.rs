//! Synthetic producer/durability regressions; no SDK, preset, or sound claim.
use super::*;
use crate::{
    audio::{AudioAssetEvent, AudioAssetOperation},
    genuine_app_observer::{InstanceIdentity, InstancePhase, Stage},
    plugins::plugin_runtime::PluginTransport,
};
use std::{
    fs, io,
    sync::{
        Condvar,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
};

fn dictionary() -> native::BaselineDictionary {
    native::BaselineDictionary::new(
        7,
        native::CanonicalMetadata {
            class_uid: "synthetic class".into(),
            version: "1".into(),
            name: "synthetic fixture".into(),
            category: "Instrument".into(),
            audio: native::AudioLayout::default(),
            events: native::EventLayout::default(),
            latency_samples: 0,
            tail_samples: 0,
        },
    )
    .unwrap()
}
fn header(sequence: u64, kind: u16, lane: envelope::Lane) -> envelope::Header {
    envelope::Header {
        sequence,
        monotonic_ns: sequence * 3,
        stage: Stage::WholeExport,
        instance: Some(InstanceIdentity {
            ordinal: 1,
            phase: InstancePhase::StrictClone,
            worker_id: None,
            source_instance_id: Some(0),
        }),
        kind,
        lane,
    }
}
fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(30)
}
type Bytes = Arc<Mutex<Vec<u8>>>;
struct MemorySink {
    bytes: Bytes,
    fail_after: Option<usize>,
    flush_error: bool,
    sync_error: bool,
    length_delta: u64,
}
impl MemorySink {
    fn healthy(bytes: Bytes) -> Self {
        Self {
            bytes,
            fail_after: None,
            flush_error: false,
            sync_error: false,
            length_delta: 0,
        }
    }
}
impl spool::SpoolSink for MemorySink {
    fn write_bytes(&mut self, input: &[u8]) -> io::Result<usize> {
        let mut bytes = self.bytes.lock().unwrap();
        if self.fail_after.is_some_and(|end| bytes.len() >= end) {
            return Err(io::Error::other("synthetic write failure"));
        }
        let used = input
            .len()
            .min(self.fail_after.map_or(usize::MAX, |end| end - bytes.len()));
        bytes.extend_from_slice(&input[..used]);
        Ok(used)
    }
    fn flush_bytes(&mut self) -> io::Result<()> {
        if self.flush_error {
            Err(io::Error::other("synthetic late flush failure"))
        } else {
            Ok(())
        }
    }
    fn sync_bytes(&mut self) -> io::Result<()> {
        if self.sync_error {
            Err(io::Error::other("synthetic late sync failure"))
        } else {
            Ok(())
        }
    }
    fn actual_length(&mut self) -> io::Result<u64> {
        Ok(self.bytes.lock().unwrap().len() as u64 + self.length_delta)
    }
}
fn with_sink(sink: Box<dyn spool::SpoolSink>) -> StreamSink {
    StreamSink::start(dictionary(), deadline(), |config| {
        spool::BoundedSpool::with_sink(config, sink)
    })
    .unwrap()
}
fn memory_actor() -> (StreamSink, Bytes) {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    (
        with_sink(Box::new(MemorySink::healthy(bytes.clone()))),
        bytes,
    )
}
fn emit_lifecycle(actor: &mut StreamSink, sequence: u64) -> Result<(), Failure> {
    actor.emit_control(
        header(sequence, 23, envelope::Lane::Control),
        control::View::Lifecycle("synthetic event"),
    )
}
fn join(actor: &mut StreamSink) {
    let wait_bound = Instant::now() + Duration::from_secs(10);
    loop {
        match actor.poll_finish() {
            FinishPoll::Joined(terminal) => {
                assert!(terminal.writer.writer_joined);
                return;
            }
            FinishPoll::Pending => {
                assert!(Instant::now() < wait_bound);
                thread::yield_now();
            }
        }
    }
}
fn operands(bytes: &[u8]) -> Vec<&[u8]> {
    let mut remaining = bytes;
    let mut records = Vec::new();
    while !remaining.is_empty() {
        let length = u32::from_le_bytes(remaining[..4].try_into().unwrap()) as usize + 4;
        let (record, rest) = remaining.split_at(length);
        records.push(record);
        remaining = rest;
    }
    records
}

static PATH_SEQUENCE: AtomicU64 = AtomicU64::new(1);
struct TestFile(std::path::PathBuf);
impl TestFile {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
            "daw-stream-sink-{}-{}.bin",
            std::process::id(),
            PATH_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        )))
    }
}
impl Drop for TestFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[test]
fn actual_file_contains_exact_typed_envelopes_after_actual_join() {
    let path = TestFile::new();
    let mut actor = StreamSink::create_new(dictionary(), &path.0, deadline()).unwrap();
    let initial_pointer = actor.inspect().buffer.unwrap().as_ptr();
    emit_lifecycle(&mut actor, 1).unwrap();
    let asset = AudioAssetEvent::Registered {
        operation: AudioAssetOperation {
            engine_session: 17,
            generation: 19,
            operation_id: 23,
        },
        id: 29,
        success: true,
    };
    actor
        .emit_app(
            header(2, 10, envelope::Lane::App),
            app::AppView::AudioAsset(&asset),
        )
        .unwrap();
    actor
        .emit_native(
            header(3, 8, envelope::Lane::Native(6)),
            native::NativeView::Midi {
                data: [0x90, 60, 100],
                offset: 127,
            },
        )
        .unwrap();
    assert_eq!(actor.inspect().buffer.unwrap().as_ptr(), initial_pointer);
    assert_eq!(actor.inspect().buffer_capacity, Some(envelope::MAX_OPERAND));
    actor.seal_after_taps_retired(true).unwrap();
    join(&mut actor);
    let terminal = actor.terminal().unwrap();
    assert!(terminal.complete());
    assert_eq!(
        terminal.writer.ack.actual_file_length,
        Some(terminal.expected.bytes)
    );
    let bytes = fs::read(&path.0).unwrap();
    assert_eq!(bytes.len() as u64, terminal.expected.bytes);
    let records = operands(&bytes);
    assert_eq!(records.len(), 3);
    let (actual, value) = body::decode_control(records[0]).unwrap();
    assert_eq!(actual, header(1, 23, envelope::Lane::Control));
    assert_eq!(value, control::Control::Lifecycle("synthetic event"));
    let (actual, value) = body::decode_app(records[1]).unwrap();
    assert_eq!(actual, header(2, 10, envelope::Lane::App));
    let app::AppControl::AudioAsset(value) = value else {
        panic!("wrong app kind")
    };
    assert_eq!(value, asset);
    let (actual, value) = body::decode_native(records[2], actor.inspect().dictionary).unwrap();
    assert_eq!(actual, header(3, 8, envelope::Lane::Native(6)));
    let native::NativeBody::Midi { data, offset } = value else {
        panic!("wrong native kind")
    };
    assert_eq!((data, offset), ([0x90, 60, 100], 127));
}

#[test]
fn thousands_of_records_reuse_the_same_only_operand_allocation() {
    let (mut actor, bytes) = memory_actor();
    let pointer = actor.inspect().buffer.unwrap().as_ptr();
    for sequence in 1..=4000 {
        emit_lifecycle(&mut actor, sequence).unwrap();
        assert_eq!(actor.inspect().buffer.unwrap().as_ptr(), pointer);
        assert_eq!(actor.inspect().buffer_capacity, Some(envelope::MAX_OPERAND));
        assert!(actor.inspect().pending_operand.is_none());
    }
    actor.seal_after_taps_retired(true).unwrap();
    join(&mut actor);
    assert!(actor.terminal().unwrap().complete());
    let bytes = bytes.lock().unwrap();
    let records = operands(&bytes);
    assert_eq!(records.len(), 4000);
    for (index, record) in records.iter().enumerate() {
        let (actual, value) = body::decode_control(record).unwrap();
        assert_eq!(actual.sequence, index as u64 + 1);
        assert_eq!(value, control::Control::Lifecycle("synthetic event"));
    }
}

#[test]
fn sequence_gap_is_sticky_and_cannot_seal_past_the_missing_record() {
    let (mut actor, _) = memory_actor();
    emit_lifecycle(&mut actor, 1).unwrap();
    let first = emit_lifecycle(&mut actor, 3).unwrap_err();
    assert_eq!(first.kind, FailureKind::Sequence);
    assert_eq!(
        actor.failure_header(),
        Some(header(3, 23, envelope::Lane::Control))
    );
    assert_eq!(emit_lifecycle(&mut actor, 2).unwrap_err(), first);
    assert_eq!(actor.seal_after_taps_retired(true).unwrap_err(), first);
    join(&mut actor);
    assert!(!actor.terminal().unwrap().complete());
    assert_eq!(actor.inspect().expected.records, 1);
}

#[test]
fn decreasing_caller_time_is_sticky_but_equal_time_is_preserved() {
    let (mut actor, bytes) = memory_actor();
    emit_lifecycle(&mut actor, 1).unwrap();
    let mut equal = header(2, 23, envelope::Lane::Control);
    equal.monotonic_ns = 3;
    actor
        .emit_control(equal, control::View::Lifecycle("same caller timestamp"))
        .unwrap();
    let mut older = header(3, 23, envelope::Lane::Control);
    older.monotonic_ns = 2;
    assert_eq!(
        actor
            .emit_control(older, control::View::Lifecycle("old"))
            .unwrap_err()
            .kind,
        FailureKind::Timestamp
    );
    join(&mut actor);
    assert!(!actor.terminal().unwrap().complete());
    assert_eq!(actor.inspect().expected.records, 2);
    let bytes = bytes.lock().unwrap();
    let records = operands(&bytes);
    // Cancellation may retain queued originals instead of writing them; it
    // never fabricates a successful two-record transcript.
    if records.len() == 2 {
        assert_eq!(envelope::decode(records[1]).unwrap().0, equal);
    }
}

#[test]
fn encode_failure_keeps_original_header_allocation_and_forbids_completion() {
    let (mut actor, _) = memory_actor();
    let pointer = actor.inspect().buffer.unwrap().as_ptr();
    let wrong = header(1, 23, envelope::Lane::App);
    let failure = actor
        .emit_control(wrong, control::View::Lifecycle("original body"))
        .unwrap_err();
    assert_eq!(
        failure.kind,
        FailureKind::Encoding(body::Error::HeaderMismatch)
    );
    assert_eq!(actor.failure_header(), Some(wrong));
    assert_eq!(actor.inspect().buffer.unwrap().as_ptr(), pointer);
    assert_eq!(actor.inspect().expected.records, 0);
    assert!(actor.seal_after_taps_retired(true).is_err());
    join(&mut actor);
    assert!(!actor.terminal().unwrap().complete());
}

#[test]
fn raw_float_bits_and_original_transport_survive_file_framing() {
    let (mut actor, bytes) = memory_actor();
    let bits = [0_u32, 0x8000_0000, 0x7fc0_1234, 0x7f80_0000, 0xff80_0000];
    let channels = [bits.map(f32::from_bits).to_vec()];
    let transport = PluginTransport {
        sample_position: -19,
        quarter_note_position: f64::from_bits(0x8000_0000_0000_0000),
        tempo: f64::from_bits(0x7ff8_0000_0000_1234),
        playing: true,
        time_sig_numerator: 7,
        time_sig_denominator: 8,
    };
    actor
        .emit_native(
            header(1, 7, envelope::Lane::Native(12)),
            native::NativeView::StrictPcm {
                frames: bits.len(),
                transport: &transport,
                channels: &channels,
                transport_applied: true,
                output_events: 17,
                output_events_lost: true,
            },
        )
        .unwrap();
    actor.seal_after_taps_retired(true).unwrap();
    join(&mut actor);
    assert!(actor.terminal().unwrap().complete());
    let bytes = bytes.lock().unwrap();
    let (_, native::NativeBody::StrictPcm(pcm)) =
        body::decode_native(&bytes, actor.inspect().dictionary).unwrap()
    else {
        panic!("wrong PCM kind")
    };
    assert_eq!(
        pcm.channels()
            .next()
            .unwrap()
            .sample_bits()
            .collect::<Vec<_>>(),
        bits
    );
    assert_eq!(
        pcm.transport.quarter_note_bits,
        transport.quarter_note_position.to_bits()
    );
    assert_eq!(pcm.transport.tempo_bits, transport.tempo.to_bits());
    assert_eq!(pcm.output_events, 17);
    assert!(pcm.output_events_lost);
}

#[test]
fn retained_state_slot_is_framed_without_opaque_payload_in_operand() {
    let (mut actor, bytes) = memory_actor();
    let state_header = header(1, 2, envelope::Lane::State);
    let mut slots = state::LiveStateSlots::new(state::RetentionLimits::default()).unwrap();
    let opaque = vec![0x91; 1024];
    let view = slots
        .retain_state_load(state_header, None, &opaque, control::OutcomeRef::Ok)
        .unwrap();
    actor.emit_state(state_header, view).unwrap();
    assert_eq!(slots.len(), 1);
    actor.seal_after_taps_retired(true).unwrap();
    join(&mut actor);
    assert!(actor.terminal().unwrap().complete());
    let bytes = bytes.lock().unwrap();
    assert!(bytes.len() < 1024);
    let (actual, value) =
        state::decode_enveloped(&bytes, Some(actor.inspect().dictionary)).unwrap();
    assert_eq!(actual, state_header);
    let state::StateBody::StateLoad { link, outcome } = value else {
        panic!("wrong state kind")
    };
    assert_eq!(link.original_header(), state_header);
    assert_eq!(slots.opaque(link).unwrap(), opaque);
    assert_eq!(outcome, control::Outcome::Ok);
}

#[test]
fn state_reference_cannot_rewrite_the_original_header() {
    let (mut actor, _) = memory_actor();
    let original = header(1, 2, envelope::Lane::State);
    let mut slots = state::LiveStateSlots::new(state::RetentionLimits::default()).unwrap();
    let view = slots
        .retain_state_load(original, None, &[1, 2, 3], control::OutcomeRef::Ok)
        .unwrap();
    let mut supplied = original;
    supplied.monotonic_ns += 1;
    assert_eq!(
        actor.emit_state(supplied, view).unwrap_err().kind,
        FailureKind::Header
    );
    assert_eq!(actor.failure_header(), Some(supplied));
    assert!(!actor.inspect().buffer.unwrap().is_empty());
    join(&mut actor);
    assert!(!actor.terminal().unwrap().complete());
}

type Gate = Arc<(Mutex<bool>, Condvar)>;
struct ReleaseGate(Gate);
impl ReleaseGate {
    fn open(&self) {
        *self.0.0.lock().unwrap() = true;
        self.0.1.notify_all();
    }
}
impl Drop for ReleaseGate {
    fn drop(&mut self) {
        self.open();
    }
}
struct GateSink {
    memory: MemorySink,
    gate: Gate,
    entered: Option<mpsc::SyncSender<()>>,
    gate_flush: bool,
}
impl GateSink {
    fn wait(&mut self) {
        if let Some(entered) = self.entered.take() {
            entered.send(()).unwrap();
            let (open, changed) = &*self.gate;
            let mut open = open.lock().unwrap();
            while !*open {
                open = changed.wait(open).unwrap();
            }
        }
    }
}
impl spool::SpoolSink for GateSink {
    fn write_bytes(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if !self.gate_flush {
            self.wait();
        }
        self.memory.write_bytes(bytes)
    }
    fn flush_bytes(&mut self) -> io::Result<()> {
        if self.gate_flush {
            self.wait();
        }
        self.memory.flush_bytes()
    }
    fn sync_bytes(&mut self) -> io::Result<()> {
        self.memory.sync_bytes()
    }
    fn actual_length(&mut self) -> io::Result<u64> {
        self.memory.actual_length()
    }
}
fn gated_actor(flush: bool) -> (StreamSink, ReleaseGate, mpsc::Receiver<()>) {
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let (entered, receiver) = mpsc::sync_channel(1);
    let sink = GateSink {
        memory: MemorySink::healthy(Arc::new(Mutex::new(Vec::new()))),
        gate: gate.clone(),
        entered: Some(entered),
        gate_flush: flush,
    };
    (with_sink(Box::new(sink)), ReleaseGate(gate), receiver)
}

#[test]
fn empty_queue_and_blocked_flush_are_not_a_terminal_complete() {
    let (mut actor, gate, entered) = gated_actor(true);
    emit_lifecycle(&mut actor, 1).unwrap();
    actor.seal_after_taps_retired(true).unwrap();
    entered.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(actor.terminal().is_none());
    assert!(matches!(actor.poll_finish(), FinishPoll::Pending));
    gate.open();
    join(&mut actor);
    assert!(actor.terminal().unwrap().complete());
}

#[test]
fn cancellation_after_actual_write_keeps_first_failure_until_same_writer_joins() {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let release = ReleaseGate(gate.clone());
    let (entered, receiver) = mpsc::sync_channel(1);
    let mut actor = with_sink(Box::new(GateSink {
        memory: MemorySink::healthy(bytes.clone()),
        gate,
        entered: Some(entered),
        gate_flush: true,
    }));
    emit_lifecycle(&mut actor, 1).unwrap();
    actor.seal_after_taps_retired(true).unwrap();
    receiver.recv_timeout(Duration::from_secs(10)).unwrap();
    // This gate is reached by the actual writer after all record fragments
    // were written and its exact-count drain succeeded, but before flush.
    let actual_written_bytes = {
        let written = bytes.lock().unwrap();
        let (actual_header, body) = body::decode_control(&written).unwrap();
        assert_eq!(actual_header, header(1, 23, envelope::Lane::Control));
        assert_eq!(body, control::Control::Lifecycle("synthetic event"));
        written.len() as u64
    };
    assert!(actor.failure().is_none());
    actor.cancel();
    let first = actor.failure().unwrap();
    assert_eq!(first.kind, FailureKind::Abandoned);
    assert!(actor.terminal().is_none());
    assert!(matches!(actor.poll_finish(), FinishPoll::Pending));
    actor.cancel();
    assert_eq!(actor.failure().unwrap(), first);
    assert_eq!(actor.seal_after_taps_retired(true).unwrap_err(), first);
    assert_eq!(emit_lifecycle(&mut actor, 2).unwrap_err(), first);
    release.open();
    join(&mut actor);
    let terminal = actor.terminal().unwrap();
    assert!(terminal.writer.writer_joined);
    assert_eq!(terminal.writer.ack.records, 1);
    assert_eq!(terminal.writer.ack.bytes, actual_written_bytes);
    assert!(!terminal.complete());
    assert_eq!(terminal.failure, Some(&first));
    assert_eq!(actor.failure().unwrap(), first);
    assert_eq!(
        terminal.writer.failure.as_ref().unwrap().kind,
        spool::FailureKind::Abandoned
    );
}

#[test]
fn pool_backpressure_retains_exact_operand_until_fixed_deadline() {
    let (mut actor, gate, entered) = gated_actor(false);
    emit_lifecycle(&mut actor, 1).unwrap();
    entered.recv_timeout(Duration::from_secs(10)).unwrap();
    for sequence in 2..=64 {
        emit_lifecycle(&mut actor, sequence).unwrap();
    }
    // Narrow only this synthetic deadline. No poll is allowed to refresh it.
    actor.inner.as_mut().unwrap().deadline = Instant::now() + Duration::from_millis(30);
    let failed_header = header(65, 23, envelope::Lane::Control);
    let failure = emit_lifecycle(&mut actor, 65).unwrap_err();
    assert!(matches!(
        failure.kind,
        FailureKind::Spool(spool::Failure {
            kind: spool::FailureKind::Deadline,
            ..
        })
    ));
    assert_eq!(actor.failure_header(), Some(failed_header));
    let inspection = actor.inspect();
    assert!(inspection.buffer.is_none());
    // All 64 slabs are queued or held by the blocked writer. The pending
    // original has acquired no slab, so there is no unsent fragment owner.
    assert!(inspection.unqueued_fragment.is_none());
    let pending = inspection.pending_operand.unwrap();
    assert_eq!(pending.next_unqueued_byte, 0);
    assert_eq!(pending.original.record_sequence, 65);
    assert_eq!(pending.original.file_offset, inspection.expected.bytes);
    assert_eq!(
        body::decode_control(&pending.original.bytes).unwrap().0,
        failed_header
    );
    assert_eq!(pending.original.bytes.capacity(), envelope::MAX_OPERAND);
    assert!(actor.seal_after_taps_retired(true).is_err());
    gate.open();
    join(&mut actor);
    assert!(!actor.terminal().unwrap().complete());
}

#[test]
fn late_flush_sync_or_length_failures_never_become_complete() {
    for (flush_error, sync_error, length_delta) in
        [(true, false, 0), (false, true, 0), (false, false, 1)]
    {
        let mut sink = MemorySink::healthy(Arc::new(Mutex::new(Vec::new())));
        sink.flush_error = flush_error;
        sink.sync_error = sync_error;
        sink.length_delta = length_delta;
        let mut actor = with_sink(Box::new(sink));
        emit_lifecycle(&mut actor, 1).unwrap();
        actor.seal_after_taps_retired(true).unwrap();
        join(&mut actor);
        let terminal = actor.terminal().unwrap();
        assert!(terminal.writer.writer_joined);
        assert!(!terminal.complete());
        assert!(terminal.writer.failure.is_some());
    }
}

#[test]
fn partial_disk_write_retains_exact_unwritten_fragment_after_join() {
    let mut sink = MemorySink::healthy(Arc::new(Mutex::new(Vec::new())));
    sink.fail_after = Some(9);
    let mut actor = with_sink(Box::new(sink));
    // The real writer may publish its error either before or after enqueue
    // returns. Both schedules must retain the same original fragment.
    let _ = emit_lifecycle(&mut actor, 1);
    let _ = actor.seal_after_taps_retired(true);
    join(&mut actor);
    let terminal = actor.terminal().unwrap();
    assert!(!terminal.complete());
    assert!(terminal.writer.writer_joined);
    let fragment = &terminal.writer.retained_fragments[0];
    assert_eq!(fragment.written_prefix, 9);
    assert_eq!(
        body::decode_control(fragment.original_fragment_bytes())
            .unwrap()
            .0,
        header(1, 23, envelope::Lane::Control)
    );
}

#[test]
fn false_tap_retirement_and_late_emit_invalidate_completion() {
    let (mut actor, _) = memory_actor();
    emit_lifecycle(&mut actor, 1).unwrap();
    assert_eq!(
        actor.seal_after_taps_retired(false).unwrap_err().kind,
        FailureKind::Protocol
    );
    join(&mut actor);
    assert!(!actor.terminal().unwrap().complete());
    let (mut actor, _) = memory_actor();
    emit_lifecycle(&mut actor, 1).unwrap();
    actor.seal_after_taps_retired(true).unwrap();
    join(&mut actor);
    assert!(actor.terminal().unwrap().complete());
    assert_eq!(
        emit_lifecycle(&mut actor, 2).unwrap_err().kind,
        FailureKind::Protocol
    );
    assert!(!actor.terminal().unwrap().complete());
}

#[test]
fn recovery_keeps_entire_actor_and_polls_the_same_actual_writer() {
    let (mut actor, gate, entered) = gated_actor(false);
    let pointer = actor.inspect().buffer.unwrap().as_ptr();
    emit_lifecycle(&mut actor, 1).unwrap();
    entered.recv_timeout(Duration::from_secs(10)).unwrap();
    let recovery = actor.into_recovery();
    assert_eq!(recovery.poll_finish(), RecoveryPoll::Pending);
    recovery
        .inspect(|value| {
            let value = value.unwrap();
            assert_eq!(value.buffer.unwrap().as_ptr(), pointer);
            assert_eq!(value.dictionary.id(), 7);
            assert_eq!(value.expected.records, 1);
            assert!(value.failure.is_some());
            assert!(value.terminal.is_none());
        })
        .unwrap();
    gate.open();
    let wait_bound = Instant::now() + Duration::from_secs(10);
    while recovery.poll_finish() != RecoveryPoll::Joined {
        assert!(Instant::now() < wait_bound);
        thread::yield_now();
    }
    recovery
        .inspect(|value| {
            let value = value.unwrap();
            assert!(value.terminal.unwrap().writer.writer_joined);
            assert!(!value.terminal.unwrap().complete());
            assert_eq!(value.buffer.unwrap().as_ptr(), pointer);
        })
        .unwrap();
}

#[test]
fn explicit_recovery_preserves_joined_failed_actor_for_readback() {
    let (mut actor, _) = memory_actor();
    let failed = header(2, 23, envelope::Lane::Control);
    let pointer = actor.inspect().buffer.unwrap().as_ptr();
    emit_lifecycle(&mut actor, 2).unwrap_err();
    join(&mut actor);
    let recovery = actor.into_recovery();
    assert_eq!(recovery.poll_finish(), RecoveryPoll::Joined);
    recovery
        .inspect(|value| {
            let value = value.unwrap();
            assert_eq!(value.buffer.unwrap().as_ptr(), pointer);
            assert_eq!(value.failure.unwrap().attempted_header, Some(failed));
            assert!(value.terminal.unwrap().writer.writer_joined);
            assert!(!value.terminal.unwrap().complete());
        })
        .unwrap();
}

#[test]
fn create_new_never_overwrites_an_existing_private_archive() {
    let path = TestFile::new();
    fs::write(&path.0, b"existing original bytes").unwrap();
    let failure = StreamSink::create_new(dictionary(), &path.0, deadline())
        .err()
        .unwrap();
    assert!(matches!(failure.failure.kind, FailureKind::Spool(_)));
    assert_eq!(failure.dictionary.id(), 7);
    assert_eq!(failure.buffer.capacity(), envelope::MAX_OPERAND);
    assert_eq!(fs::read(&path.0).unwrap(), b"existing original bytes");
}

#[test]
fn already_expired_start_cannot_create_file_or_refresh_deadline() {
    let path = TestFile::new();
    let failure = StreamSink::create_new(dictionary(), &path.0, Instant::now())
        .err()
        .unwrap();
    assert_eq!(failure.failure.kind, FailureKind::Deadline);
    assert_eq!(failure.dictionary.id(), 7);
    assert!(!path.0.exists());
}
