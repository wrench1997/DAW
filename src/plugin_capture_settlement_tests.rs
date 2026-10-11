//! Plugin-free settlement proofs. Synthetic publication fixtures below test the
//! production join/counters directly; they are not callback readiness authority.

use super::*;

fn identity() -> BounceSourceCallbackIdentity {
    BounceSourceCallbackIdentity {
        engine_session: 3,
        project_session: 5,
        timeline_generation: 7,
        epoch: INITIAL_TRANSPORT_EPOCH,
        transport_request: 11,
        channel_id: 13,
        endpoint_id: 17,
        instance_id: 19,
    }
}

struct SyntheticSettlement {
    metrics: BridgeMetrics,
    admission: AtomicU32,
    target: BounceSourceCallbackIdentity,
    manifest: PluginEndpointManifest,
}
impl SyntheticSettlement {
    fn ready() -> Self {
        let target = identity();
        let metrics = BridgeMetrics::default();
        metrics
            .requested_epoch
            .store(target.epoch, Ordering::Relaxed);
        metrics.callback_settlement.publish(target, true, 0);
        metrics.worker_settlement.publication.write(|| {
            metrics
                .worker_settlement
                .valid
                .store(true, Ordering::Relaxed);
            metrics
                .worker_settlement
                .reset_completed_epoch
                .store(target.epoch, Ordering::Relaxed);
        });
        Self {
            metrics,
            admission: AtomicU32::new(0),
            target,
            manifest: PluginEndpointManifest::identified(&[target.instance_id]).unwrap(),
        }
    }
    fn observe(&self) -> BounceSourceSettlementObservation {
        observe_bounce_source_settlement(&self.metrics, self.manifest, &self.admission, self.target)
    }
}

#[test]
fn synthetic_settlement_waits_for_reset_and_terminal_without_output_delivery() {
    let fixture = SyntheticSettlement::ready();
    let worker = &fixture.metrics.worker_settlement;
    worker.reset_completed_epoch.store(0, Ordering::Relaxed);
    assert_eq!(
        fixture.observe().settlement,
        BounceSourceSettlement::Waiting
    );
    let reset = WorkerSettlementOperation::new(&fixture.metrics, false);
    reset.reset_completed(fixture.target.epoch);
    assert_eq!(fixture.observe().busy, 1);
    assert_eq!(
        fixture.observe().settlement,
        BounceSourceSettlement::Waiting
    );
    drop(reset);
    fixture
        .metrics
        .callback_settlement
        .publish(fixture.target, true, 1);
    // Successful output push cannot substitute for actual input ownership.
    fixture.metrics.completed.store(1, Ordering::Relaxed);
    assert_eq!(
        fixture.observe().settlement,
        BounceSourceSettlement::Waiting
    );
    let input = WorkerSettlementOperation::new(&fixture.metrics, true);
    let observation = fixture.observe();
    assert_eq!(
        (observation.consumed, observation.terminal, observation.busy),
        (1, 0, 1)
    );
    assert_eq!(observation.settlement, BounceSourceSettlement::Waiting);
    // A terminal discard/overflow owns no successful output, but ends the input.
    fixture.metrics.completed.store(0, Ordering::Relaxed);
    drop(input);
    let observation = fixture.observe();
    assert_eq!(
        (observation.consumed, observation.terminal, observation.busy),
        (1, 1, 0)
    );
    assert_eq!(observation.settlement, BounceSourceSettlement::Ready);
    assert_eq!(fixture.metrics.completed.load(Ordering::Relaxed), 0);
}

#[test]
fn synthetic_nested_reset_does_not_clear_outer_input_busy() {
    let fixture = SyntheticSettlement::ready();
    fixture
        .metrics
        .callback_settlement
        .publish(fixture.target, true, 1);
    let input = WorkerSettlementOperation::new(&fixture.metrics, true);
    {
        let reset = WorkerSettlementOperation::new(&fixture.metrics, false);
        reset.reset_completed(fixture.target.epoch);
        assert_eq!(fixture.observe().busy, 2);
    }
    assert_eq!(fixture.observe().busy, 1);
    assert_eq!(fixture.observe().terminal, 0);
    assert_eq!(
        fixture.observe().settlement,
        BounceSourceSettlement::Waiting
    );
    drop(input);
    assert_eq!(fixture.observe().settlement, BounceSourceSettlement::Ready);
}

#[test]
fn synthetic_callback_publication_collision_and_pending_work_wait_boundedly() {
    let fixture = SyntheticSettlement::ready();
    assert_eq!(LATENCY_SNAPSHOT_READ_ATTEMPTS, 3);
    for callback in [true, false] {
        let sequence = if callback {
            &fixture.metrics.callback_settlement.publication.sequence
        } else {
            &fixture.metrics.worker_settlement.publication.sequence
        };
        let stable = sequence.load(Ordering::Relaxed);
        sequence.store(stable + 1, Ordering::Relaxed);
        let observation = fixture.observe();
        assert!(!observation.coherent);
        assert_eq!(observation.settlement, BounceSourceSettlement::Waiting);
        sequence.store(stable + 2, Ordering::Relaxed);
        assert_eq!(fixture.observe().settlement, BounceSourceSettlement::Ready);
    }
    fixture.metrics.callback_settlement.invalidate();
    let observation = fixture.observe();
    assert!(observation.coherent);
    assert_eq!(observation.callback_identity, None);
    assert_eq!(observation.settlement, BounceSourceSettlement::Waiting);
    fixture
        .metrics
        .callback_settlement
        .publish(fixture.target, false, 0);
    assert_eq!(
        fixture.observe().settlement,
        BounceSourceSettlement::Waiting
    );
}

#[test]
fn synthetic_exact_owner_supersession_is_invalid_in_every_identity_domain() {
    for field in 0..8 {
        let mut fixture = SyntheticSettlement::ready();
        match field {
            0 => fixture.target.engine_session += 1,
            1 => fixture.target.project_session += 1,
            2 => fixture.target.timeline_generation += 1,
            3 => fixture.target.epoch += 1,
            4 => fixture.target.transport_request += 1,
            5 => fixture.target.channel_id += 1,
            6 => fixture.target.endpoint_id += 1,
            7 => fixture.target.instance_id += 1,
            _ => unreachable!(),
        }
        assert_eq!(
            fixture.observe().settlement,
            BounceSourceSettlement::Invalid
        );
    }
    let fixture = SyntheticSettlement::ready();
    // Actual requested-epoch authority defeats even an in-progress publication.
    fixture.metrics.requested_epoch.store(2, Ordering::Release);
    fixture
        .metrics
        .callback_settlement
        .publication
        .sequence
        .store(3, Ordering::Release);
    assert_eq!(
        fixture.observe().settlement,
        BounceSourceSettlement::Invalid
    );
}

#[test]
fn synthetic_settlement_counter_and_version_exhaustion_are_sticky() {
    for callback in [true, false] {
        let fixture = SyntheticSettlement::ready();
        let publication = if callback {
            &fixture.metrics.callback_settlement.publication
        } else {
            &fixture.metrics.worker_settlement.publication
        };
        publication.sequence.store(u64::MAX - 1, Ordering::Relaxed);
        publication.write(|| panic!("exhausted publication must not mutate"));
        publication.sequence.store(0, Ordering::Relaxed);
        publication.write(|| panic!("sticky exhaustion must not reopen"));
        assert!(fixture.observe().exhausted);
        assert_eq!(
            fixture.observe().settlement,
            BounceSourceSettlement::Invalid
        );
    }
    for consumed in [true, false] {
        let fixture = SyntheticSettlement::ready();
        let counter = if consumed {
            &fixture.metrics.worker_settlement.consumed
        } else {
            &fixture.metrics.worker_settlement.terminal
        };
        counter.store(u64::MAX, Ordering::Relaxed);
        drop(WorkerSettlementOperation::new(&fixture.metrics, true));
        assert_eq!(counter.load(Ordering::Relaxed), u64::MAX);
        assert!(fixture.observe().exhausted);
        assert_eq!(
            fixture.observe().settlement,
            BounceSourceSettlement::Invalid
        );
    }
    let fixture = SyntheticSettlement::ready();
    fixture
        .metrics
        .worker_settlement
        .busy
        .store(u32::MAX, Ordering::Relaxed);
    drop(WorkerSettlementOperation::new(&fixture.metrics, false));
    assert!(fixture.observe().exhausted);
    assert_eq!(
        fixture.observe().settlement,
        BounceSourceSettlement::Invalid
    );
}

#[test]
fn synthetic_unwind_and_shutdown_terminal_ownership_never_reopen_readiness() {
    let fixture = SyntheticSettlement::ready();
    fixture
        .metrics
        .callback_settlement
        .publish(fixture.target, true, 1);
    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        let _input = WorkerSettlementOperation::new(&fixture.metrics, true);
        let _reset = WorkerSettlementOperation::new(&fixture.metrics, false);
        panic!("plugin-free ownership unwind");
    }));
    assert!(result.is_err());
    let observation = fixture.observe();
    assert_eq!(
        (observation.consumed, observation.terminal, observation.busy),
        (1, 1, 0)
    );
    assert!(!observation.worker_valid);
    assert_eq!(observation.settlement, BounceSourceSettlement::Invalid);
    let fixture = SyntheticSettlement::ready();
    fixture.metrics.worker_settlement.invalidate();
    drop(WorkerSettlementOperation::new(&fixture.metrics, true));
    assert_eq!(fixture.observe().terminal, 1);
    assert_eq!(
        fixture.observe().settlement,
        BounceSourceSettlement::Invalid
    );
}

fn manual_endpoint() -> (
    AudioThreadEndpoint,
    Consumer<StereoBlock>,
    Producer<StereoBlock>,
) {
    let (input, input_consumer) = RingBuffer::new(2);
    let (output_producer, output) = RingBuffer::new(2);
    let (parameter_edit_failures, _failures) =
        RingBuffer::new(MAX_OUTSTANDING_PARAMETER_EDITS as usize);
    let metrics = Arc::new(BridgeMetrics::default());
    metrics.max_block_frames.store(128, Ordering::Relaxed);
    metrics
        .requested_epoch
        .store(identity().epoch, Ordering::Relaxed);
    metrics
        .current_epoch
        .store(identity().epoch, Ordering::Relaxed);
    (
        AudioThreadEndpoint {
            input,
            output,
            epoch: identity().epoch,
            next_sequence: 1,
            requested_epoch: Arc::clone(&metrics.requested_epoch),
            scratch: Box::new(EndpointScratch::new()),
            metrics,
            manifest: PluginEndpointManifest::identified(&[identity().instance_id]).unwrap(),
            parameter_edit_admission: Arc::new(AtomicU32::new(0)),
            parameter_edit_failures,
        },
        input_consumer,
        output_producer,
    )
}

#[test]
fn endpoint_submission_wrap_is_detected_and_stays_invalid() {
    let (mut endpoint, mut input, _output) = manual_endpoint();
    endpoint
        .metrics
        .submitted
        .store(u64::MAX, Ordering::Relaxed);
    assert!(matches!(
        endpoint.try_submit(&[0.0; 128], &[0.0; 128]),
        SubmitStatus::Submitted { .. }
    ));
    assert!(input.pop().is_ok());
    endpoint.publish_bounce_source_settlement(identity(), true);
    let observation = observe_bounce_source_settlement(
        &endpoint.metrics,
        endpoint.manifest,
        &endpoint.parameter_edit_admission,
        identity(),
    );
    assert_eq!(observation.submitted, 0);
    assert!(observation.exhausted);
    assert_eq!(observation.settlement, BounceSourceSettlement::Invalid);
    assert!(endpoint.set_epoch(2));
    assert!(
        endpoint
            .metrics
            .callback_settlement
            .publication
            .exhausted
            .load(Ordering::Acquire)
    );
}

#[test]
fn endpoint_input_events_and_admission_block_but_retained_outputs_do_not() {
    let (mut endpoint, _input, mut output) = manual_endpoint();
    let metrics = Arc::clone(&endpoint.metrics);
    metrics
        .worker_settlement
        .valid
        .store(true, Ordering::Relaxed);
    metrics
        .worker_settlement
        .reset_completed_epoch
        .store(identity().epoch, Ordering::Relaxed);
    assert!(output.push(StereoBlock::silence()).is_ok());
    endpoint.scratch.future_output_valid[0] = true;
    endpoint
        .scratch
        .delayed_dry
        .store(identity().epoch, 0, &[0.0; 128], &[0.0; 128], false);
    let observe = |endpoint: &AudioThreadEndpoint| {
        observe_bounce_source_settlement(
            &metrics,
            endpoint.manifest,
            &endpoint.parameter_edit_admission,
            identity(),
        )
    };
    endpoint.publish_bounce_source_settlement(identity(), true);
    assert_eq!(observe(&endpoint).settlement, BounceSourceSettlement::Ready);
    assert!(endpoint.try_send_midi(None, MidiMessage::new([0x90, 60, 100], 0)));
    endpoint.publish_bounce_source_settlement(identity(), true);
    assert!(!observe(&endpoint).callback_ready);
    endpoint.scratch.pending_rt_event_count = 0; // Synthetic ownership fixture only.
    endpoint
        .parameter_edit_admission
        .store(1, Ordering::Relaxed);
    endpoint.publish_bounce_source_settlement(identity(), true);
    assert!(!observe(&endpoint).callback_ready);
    endpoint
        .parameter_edit_admission
        .store(0, Ordering::Relaxed);
    endpoint.publish_bounce_source_settlement(identity(), true);
    assert_eq!(observe(&endpoint).settlement, BounceSourceSettlement::Ready);
    drop(endpoint);
    assert!(metrics.callback_settlement.retired.load(Ordering::Acquire));
    assert!(!metrics.callback_settlement.ready.load(Ordering::Acquire));
}

struct GuardBackend;
impl PluginBackend for GuardBackend {
    fn name(&self) -> &str {
        "plugin-free capture guard"
    }
    fn prepare(&mut self, _: PluginPrepareConfig) -> Result<(), String> {
        panic!("no prepare")
    }
    fn process(&mut self, _: &mut [f32], _: &mut [f32], _: usize) -> Result<(), String> {
        panic!("no Process")
    }
    fn send_midi(&mut self, _: MidiMessage) -> Result<(), String> {
        panic!("no MIDI")
    }
    fn set_parameter(&mut self, _: u32, _: f32) -> Result<(), String> {
        panic!("no parameter")
    }
    fn get_parameter(&mut self, _: u32) -> Result<f32, String> {
        panic!("no getter")
    }
    fn save_state(&mut self) -> Result<Vec<u8>, String> {
        panic!("no SaveState")
    }
    fn load_state(&mut self, _: &[u8]) -> Result<(), String> {
        panic!("no LoadState")
    }
    fn latency_samples(&self) -> u32 {
        0
    }
    fn tail_samples(&self) -> u32 {
        0
    }
    fn capture_instrument_bounce_state(
        &mut self,
        _: &[u32],
        _: &BounceCaptureFence,
    ) -> Result<BouncePluginState, BounceCaptureError> {
        // Distinct backend outcome proves passing the canonical guard reaches this call.
        Err(BounceCaptureError::Rejected("mock capture reached".into()))
    }
}

fn guard_result(expected: u64, active: u64, requested: u64, empty: bool, admission: u32) -> String {
    let metrics = BridgeMetrics::default();
    let (mut events, _receiver) = RingBuffer::new(2);
    let mut slots = [WorkerSlot {
        backend: Some(Box::new(GuardBackend)),
        config: SlotConfig::default(),
        fault: None,
        parameter_catalog_cache: None,
        native_base_ids: Vec::new(),
    }];
    match capture_instrument_bounce_on_worker(
        &BounceCaptureFence::new(Instant::now() + Duration::from_secs(1)),
        expected,
        active,
        &AtomicU64::new(requested),
        empty,
        &[],
        &AtomicU32::new(admission),
        &mut slots,
        &mut events,
        &metrics,
    ) {
        Err(BounceCaptureError::Rejected(message)) => message,
        _ => panic!("unexpected capture result"),
    }
}

#[test]
fn canonical_guard_exact_operands_and_result_survive_inactive_and_overflow_observers() {
    use genuine_observer::{RecordKind, WorkerCaptureGuard, with_capture_guard_test_observer};
    for (expected, active, requested, empty, admission) in [
        (7, 7, 7, true, 0),
        (0, 7, 7, true, 0),
        (7, 6, 7, true, 0),
        (7, 7, 8, true, 0),
        (7, 7, 7, false, 0),
        (7, 7, 7, true, 1),
    ] {
        let accepted =
            expected != 0 && expected == active && expected == requested && empty && admission == 0;
        let call = || guard_result(expected, active, requested, empty, admission);
        let (inactive, no_records) = with_capture_guard_test_observer(None, call);
        assert!(no_records.is_none());
        let (observed, records) = with_capture_guard_test_observer(Some(1), call);
        let records = records.unwrap();
        assert!(records.failure.is_none());
        assert_eq!(records.records.len(), 1);
        let RecordKind::WorkerCaptureGuard { operands, .. } = records.records[0].kind else {
            panic!("guard operands missing");
        };
        assert_eq!(
            operands,
            WorkerCaptureGuard {
                expected_epoch: expected,
                active_epoch: active,
                requested_epoch: requested,
                input_empty: empty,
                parameter_admission: admission,
                accepted,
            }
        );
        let (overflow, records) = with_capture_guard_test_observer(Some(0), call);
        let records = records.unwrap();
        assert_eq!(records.failure, Some("observer record cap exceeded"));
        assert!(records.records.is_empty());
        assert_eq!(inactive, observed);
        assert_eq!(inactive, overflow);
        assert_eq!(
            inactive,
            if accepted {
                "mock capture reached"
            } else {
                "Instrument bounce source is not quiescent at the exact requested epoch"
            }
        );
    }
    assert!(genuine_observer::capture_guard_test_observer_inactive());
}

/// Actual worker loop with pre-owned input/output queues and no backend. This
/// verifies the loop's terminal ownership paths, not a synthetic source certificate.
static QUEUED_WORKER_CLEANUP_FAILED: AtomicBool = AtomicBool::new(false);
enum QueuedWorkerQuarantine {
    Vacant,
    Reserved,
    Failed {
        _worker_id: usize,
        _outcome: RetainedShutdownPoll,
        _guard: std::mem::ManuallyDrop<PluginWorkerGuard>,
    },
}
thread_local! {
    static QUEUED_WORKER_QUARANTINE: std::cell::RefCell<[QueuedWorkerQuarantine; 2]> =
        const { std::cell::RefCell::new([const { QueuedWorkerQuarantine::Vacant }; 2]) };
}
struct QueuedWorker {
    metrics: Arc<BridgeMetrics>,
    output: Consumer<StereoBlock>,
    guard: Option<PluginWorkerGuard>,
    quarantine_slot: usize,
    cleanup_joined: Arc<AtomicBool>,
    cleanup_outcome: Option<RetainedShutdownPoll>,
}
impl QueuedWorker {
    fn spawn(epochs: &[u64], shutdown_first: bool) -> Self {
        assert!(
            !QUEUED_WORKER_CLEANUP_FAILED.load(Ordering::Acquire),
            "a prior worker cleanup is unproven; later admission refused",
        );
        let quarantine_slot = QUEUED_WORKER_QUARANTINE.with(|owners| {
            let mut owners = owners.borrow_mut();
            let slot = owners
                .iter()
                .position(|owner| matches!(owner, QueuedWorkerQuarantine::Vacant))
                .expect("reserve quarantine before starting a worker");
            owners[slot] = QueuedWorkerQuarantine::Reserved;
            slot
        });
        let cleanup_joined = Arc::new(AtomicBool::new(false));
        let metrics = Arc::new(BridgeMetrics::default());
        metrics.requested_epoch.store(1, Ordering::Relaxed);
        let (mut input, input_rx) = RingBuffer::new(4);
        for (index, &epoch) in epochs.iter().enumerate() {
            let mut block = StereoBlock::silence();
            block.epoch = epoch;
            block.sequence = index as u64 + 1;
            block.frames = 128;
            assert!(input.push(block).is_ok());
            metrics.submitted.fetch_add(1, Ordering::Relaxed);
        }
        let (output_tx, output) = RingBuffer::new(1);
        let (event_tx, event_rx) = RingBuffer::new(8);
        let (failure_tx, failure_rx) = RingBuffer::new(16);
        let (receipt_tx, receipt_rx) = RingBuffer::new(16);
        let (admin, admin_rx) = mpsc::sync_channel(4);
        if shutdown_first {
            assert!(admin.try_send(AdminCommand::Shutdown).is_ok());
        }
        let worker_metrics = Arc::clone(&metrics);
        let requested = Arc::clone(&metrics.requested_epoch);
        let join = thread::Builder::new().spawn(move || {
            // Hold opposite queue owners through worker teardown without fake pops.
            let _queue_owners = (input, event_rx, failure_tx, receipt_rx);
            run_worker(
                Vec::new(),
                PluginPrepareConfig {
                    sample_rate: 48_000.0,
                    max_block_frames: 128,
                },
                input_rx,
                output_tx,
                admin_rx,
                event_tx,
                failure_rx,
                receipt_tx,
                worker_metrics,
                requested,
            );
        });
        let join = match join {
            Ok(join) => join,
            Err(error) => {
                QUEUED_WORKER_QUARANTINE.with(|owners| {
                    owners.borrow_mut()[quarantine_slot] = QueuedWorkerQuarantine::Vacant;
                });
                panic!("worker fixture could not start: {error}");
            }
        };
        // Immediately retain the exact handle; no post-spawn assertion can detach it.
        let guard = PluginWorkerGuard {
            admin,
            join: Some(join),
            metrics: Arc::clone(&metrics),
            retained_shutdown: if shutdown_first {
                RetainedShutdownState::WaitingForJoin
            } else {
                RetainedShutdownState::NotRequested
            },
        };
        Self {
            metrics,
            output,
            guard: Some(guard),
            quarantine_slot,
            cleanup_joined,
            cleanup_outcome: None,
        }
    }
    fn wait_terminal(&self, terminal: u64) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while self
            .metrics
            .worker_settlement
            .terminal
            .load(Ordering::Acquire)
            < terminal
        {
            assert!(
                Instant::now() < deadline,
                "worker terminal ownership timed out"
            );
            thread::sleep(Duration::from_millis(1));
        }
    }
    fn finish(&mut self) -> RetainedShutdownPoll {
        let Some(guard) = self.guard.as_mut() else {
            return self
                .cleanup_outcome
                .expect("cleanup outcome stays attached to its owner");
        };
        let worker_id = guard.genuine_worker_id();
        let deadline = Instant::now() + Duration::from_secs(2);
        let outcome = loop {
            let outcome = guard.poll_shutdown_retained();
            if outcome != RetainedShutdownPoll::Pending || Instant::now() >= deadline {
                break outcome;
            }
            thread::sleep(Duration::from_millis(1));
        };
        let guard = self
            .guard
            .take()
            .expect("exact retained worker remains owned");
        if outcome == RetainedShutdownPoll::Joined {
            self.cleanup_joined.store(true, Ordering::Release);
            drop(guard);
            QUEUED_WORKER_QUARANTINE.with(|owners| {
                owners.borrow_mut()[self.quarantine_slot] = QueuedWorkerQuarantine::Vacant;
            });
        } else {
            QUEUED_WORKER_CLEANUP_FAILED.store(true, Ordering::Release);
            QUEUED_WORKER_QUARANTINE.with(|owners| {
                owners.borrow_mut()[self.quarantine_slot] = QueuedWorkerQuarantine::Failed {
                    _worker_id: worker_id,
                    _outcome: outcome,
                    _guard: std::mem::ManuallyDrop::new(guard),
                };
            });
        }
        self.cleanup_outcome = Some(outcome);
        outcome
    }
    fn stop_and_join(&mut self) {
        assert_eq!(self.finish(), RetainedShutdownPoll::Joined);
    }
}
impl Drop for QueuedWorker {
    fn drop(&mut self) {
        let outcome = self.finish();
        if outcome != RetainedShutdownPoll::Joined && !thread::panicking() {
            panic!("worker fixture cleanup failed: {outcome:?}; exact owner quarantined");
        }
    }
}

#[test]
fn actual_worker_old_epoch_and_output_full_each_end_one_input() {
    let mut worker = QueuedWorker::spawn(&[2, 1, 1], false);
    worker.wait_terminal(3);
    worker.stop_and_join();
    let metrics = &worker.metrics;
    assert_eq!(metrics.submitted.load(Ordering::Acquire), 3);
    assert_eq!(
        metrics.worker_settlement.consumed.load(Ordering::Acquire),
        3
    );
    assert_eq!(
        metrics.worker_settlement.terminal.load(Ordering::Acquire),
        3
    );
    assert_eq!(metrics.worker_settlement.busy.load(Ordering::Acquire), 0);
    assert_eq!(metrics.dropped_old_epoch_inputs.load(Ordering::Acquire), 1);
    assert_eq!(metrics.output_overflows.load(Ordering::Acquire), 1);
    assert_eq!(metrics.completed.load(Ordering::Acquire), 1);
    assert_eq!(worker.output.slots(), 1);
    assert!(!metrics.worker_settlement.valid.load(Ordering::Acquire));
    assert!(metrics.worker_settlement.terminated.load(Ordering::Acquire));
}

#[test]
fn actual_worker_shutdown_drain_ends_inputs_without_output_success() {
    let mut worker = QueuedWorker::spawn(&[1, 1, 1], true);
    worker.wait_terminal(3);
    worker.stop_and_join();
    let metrics = &worker.metrics;
    assert_eq!(
        metrics.worker_settlement.consumed.load(Ordering::Acquire),
        3
    );
    assert_eq!(
        metrics.worker_settlement.terminal.load(Ordering::Acquire),
        3
    );
    assert_eq!(metrics.worker_settlement.busy.load(Ordering::Acquire), 0);
    assert_eq!(metrics.completed.load(Ordering::Acquire), 0);
    assert_eq!(worker.output.slots(), 0);
    assert!(metrics.worker_settlement.terminated.load(Ordering::Acquire));
}

#[test]
fn queued_worker_fixture_unwind_retains_and_joins_the_exact_owner() {
    let worker = QueuedWorker::spawn(&[1, 1, 1], false);
    let cleanup_joined = Arc::clone(&worker.cleanup_joined);
    let metrics = Arc::clone(&worker.metrics);
    let slot = worker.quarantine_slot;
    let worker_id = worker.guard.as_ref().unwrap().genuine_worker_id();
    let result = panic::catch_unwind(AssertUnwindSafe(move || {
        let _owner = worker;
        panic!("intentional worker fixture owner unwind");
    }));
    assert!(result.is_err());
    assert!(cleanup_joined.load(Ordering::Acquire));
    assert_eq!(Arc::as_ptr(&metrics) as usize, worker_id);
    assert!(metrics.worker_settlement.terminated.load(Ordering::Acquire));
    assert_eq!(metrics.detached_workers.load(Ordering::Acquire), 0);
    assert_eq!(metrics.shutdown_timeouts.load(Ordering::Acquire), 0);
    QUEUED_WORKER_QUARANTINE.with(|owners| {
        assert!(matches!(
            owners.borrow()[slot],
            QueuedWorkerQuarantine::Vacant
        ));
    });
    assert!(!QUEUED_WORKER_CLEANUP_FAILED.load(Ordering::Acquire));
}
