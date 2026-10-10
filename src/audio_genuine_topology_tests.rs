// Source-only qualification fixtures. Every worker is retained before any assertion
// after spawn; cleanup uses finite retained polling, including assertion unwinds.
use super::*;
use crate::audio::genuine_observation::{
    GENUINE_LIFECYCLE_CAPACITY, GenuineLifecycleKind, GenuineLifecycleObservation,
    GenuineSegmentDisposition,
};
use crate::plugins::plugin_runtime::{BridgeStats, RetainedShutdownPoll};
use std::sync::{Mutex, MutexGuard};

const DRAIN_BUDGET: Duration = Duration::from_secs(5);
static SERIAL: Mutex<()> = Mutex::new(());
static ADMISSION_BLOCKED: AtomicBool = AtomicBool::new(false);
static QUARANTINE: Mutex<[RetentionSlot; 4]> = Mutex::new([const { RetentionSlot::Empty }; 4]);

#[allow(clippy::large_enum_variant)] // Fixed quarantine must not allocate during failed cleanup.
enum RetentionSlot {
    Empty,
    Reserved,
    Pending {
        _guard: PluginWorkerGuard,
        _control: PluginChainControl,
        _stats: BridgeStats,
    },
}

struct RetainedWorker {
    audio: Option<AudioThreadEndpoint>,
    control: Option<PluginChainControl>,
    guard: Option<PluginWorkerGuard>,
    retention_slot: usize,
    joined: Arc<AtomicBool>,
}

impl RetainedWorker {
    fn drain(&mut self) -> bool {
        drop(self.audio.take());
        if self.guard.is_none() {
            return self.joined.load(Ordering::Acquire);
        }
        let deadline = Instant::now() + DRAIN_BUDGET;
        loop {
            let outcome = self.guard.as_mut().unwrap().poll_shutdown_retained();
            let stats = self.control.as_ref().unwrap().stats();
            if outcome != RetainedShutdownPoll::Pending {
                let joined = outcome == RetainedShutdownPoll::Joined
                    && stats.detached_workers == 0
                    && stats.shutdown_timeouts == 0;
                self.joined.store(joined, Ordering::Release);
                drop(self.guard.take());
                QUARANTINE.lock().unwrap_or_else(|error| error.into_inner())[self.retention_slot] =
                    RetentionSlot::Empty;
                if !joined {
                    ADMISSION_BLOCKED.store(true, Ordering::Release);
                }
                return joined;
            }
            if Instant::now() >= deadline {
                ADMISSION_BLOCKED.store(true, Ordering::Release);
                // Admission reserved this exact slot before spawn. No growth,
                // overwrite of another owner, or Pending guard Drop is possible.
                QUARANTINE.lock().unwrap_or_else(|error| error.into_inner())[self.retention_slot] =
                    RetentionSlot::Pending {
                        _guard: self.guard.take().unwrap(),
                        _control: self.control.take().unwrap(),
                        _stats: stats,
                    };
                return false;
            }
            thread::sleep(Duration::from_millis(1));
        }
    }
}

impl Drop for RetainedWorker {
    fn drop(&mut self) {
        let _ = self.drain();
    }
}

struct TopologyFixture {
    callback: Option<TestAudioCallback>,
    engine: Option<AudioEngine>,
    workers: [Option<RetainedWorker>; 4],
    admitted: usize,
    _serial: MutexGuard<'static, ()>,
}

impl TopologyFixture {
    fn new(observed: bool) -> Self {
        let serial = SERIAL.lock().unwrap_or_else(|error| error.into_inner());
        assert!(
            !ADMISSION_BLOCKED.load(Ordering::Acquire),
            "unresolved prior mock cleanup"
        );
        let (engine, callback) = AudioEngine::test_engine_with_callback();
        let mut fixture = Self {
            callback: Some(callback),
            engine: Some(engine),
            workers: std::array::from_fn(|_| None),
            admitted: 0,
            _serial: serial,
        };
        assert!(fixture.engine().set_project_session(7));
        if observed {
            fixture
                .callback()
                .enable_genuine_observation([91; 32])
                .unwrap();
        }
        fixture.activate(false, 10, 2);
        let baseline = fixture.callback().genuine_snapshot();
        assert!(baseline.paused_work_quiescent(10, 2));
        assert!(baseline.graph_was_activated && baseline.graph_binding_exact);
        assert_eq!(baseline.timeline_execution_failures, 0);
        assert!(baseline.endpoints.is_empty());
        fixture.callback().take_genuine_segments();
        fixture
    }

    fn callback(&mut self) -> &mut TestAudioCallback {
        self.callback.as_mut().unwrap()
    }
    fn engine(&mut self) -> &mut AudioEngine {
        self.engine.as_mut().unwrap()
    }

    fn pump(&mut self) {
        let (callback, engine) = (
            self.callback.as_mut().unwrap(),
            self.engine.as_ref().unwrap(),
        );
        crate::realtime_test_alloc::assert_no_alloc_or_drop(|| callback.pump(engine, 128));
    }

    fn add_worker(&mut self, instance: u64) -> usize {
        assert!(!ADMISSION_BLOCKED.load(Ordering::Acquire));
        assert!(self.admitted < self.workers.len());
        let index = self.admitted;
        {
            let mut slots = QUARANTINE.lock().unwrap_or_else(|error| error.into_inner());
            assert!(matches!(slots[index], RetentionSlot::Empty));
            slots[index] = RetentionSlot::Reserved;
        }
        self.admitted += 1;
        let joined = Arc::new(AtomicBool::new(false));
        let chain = PluginChain::spawn_identified_with_backend_factory(
            &[instance],
            || {
                vec![BackendSlot::new(Box::new(MockInstrumentBackend {
                    active: false,
                    amplitude: 0.25,
                    process_delay: Duration::ZERO,
                    last_midi: Arc::new(AtomicU32::new(0)),
                    parameter: Arc::new(AtomicU32::new(0)),
                }))]
            },
            PluginPrepareConfig {
                sample_rate: 48_000.0,
                max_block_frames: 128,
            },
        )
        .expect("identified mock spawn");
        self.workers[index] = Some(RetainedWorker {
            audio: Some(chain.audio),
            control: Some(chain.control),
            guard: Some(chain.guard),
            retention_slot: index,
            joined,
        });
        let retained = self.workers[index].as_ref().unwrap();
        assert_eq!(
            retained.guard.as_ref().unwrap().genuine_worker_id(),
            retained.control.as_ref().unwrap().genuine_worker_id(),
            "the retained guard/control must identify the same actual worker before readiness"
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        while !self.workers[index]
            .as_ref()
            .unwrap()
            .control
            .as_ref()
            .unwrap()
            .plugin_latency_snapshot()
            .is_some_and(|snapshot| snapshot.revision != 0 && snapshot.slot_is_active(0))
        {
            assert!(Instant::now() < deadline, "bounded mock preparation");
            thread::sleep(Duration::from_millis(1));
        }
        index
    }

    fn queue_install(&mut self, worker: usize, endpoint_id: u64) {
        let audio = self.workers[worker].as_mut().unwrap().audio.take().unwrap();
        assert!(self.engine().install_generator_endpoint_for_project(
            7,
            9,
            endpoint_id,
            700,
            1,
            audio
        ));
    }

    fn project(with_generator: bool) -> Project {
        let mut project = timeline_test_project(4.0);
        project.mixer_tracks.truncate(2);
        project
            .mixer_routes
            .retain(|route| route.source_mixer_track_id == 1);
        if with_generator {
            let mut channel = timeline_test_channel(9, 1);
            channel.instrument_plugin_instance_id = Some(700);
            project.channels.push(channel);
            project.plugin_instances.push(timeline_test_plugin(700));
        }
        project
    }

    fn timeline(with_generator: bool) -> Arc<CompiledTimeline> {
        compile_timeline_test_project(&Self::project(with_generator))
    }

    fn queue_activation(
        &mut self,
        timeline: Arc<CompiledTimeline>,
        generation: u64,
        epoch: u64,
    ) -> u64 {
        let engine = self.engine();
        let chase = engine
            .prepare_timeline_chase(
                &timeline,
                generation,
                epoch,
                0,
                TimelineChaseOptions::default(),
            )
            .unwrap();
        let loop_chase = engine
            .prepare_timeline_loop_chase(&timeline, generation, 0, TimelineChaseOptions::default())
            .unwrap();
        engine
            .install_compiled_timeline(generation, timeline)
            .unwrap();
        let loop_token = engine.install_timeline_loop_chase(loop_chase).unwrap();
        engine.install_timeline_chase(chase).unwrap();
        engine
            .activate_timeline_transport(TimelineTransportActivationSpec {
                revision: generation,
                target_epoch: epoch,
                minimum_epoch: epoch,
                frame: 0,
                beat_q32: 0,
                loop_start_frame: 0,
                loop_end_frame: 0,
                loop_start_q32: 0,
                loop_end_q32: 0,
                loop_token,
                loop_enabled: false,
                playing: false,
                mixer_pan_release: TimelineMixerPanRelease::EMPTY,
            })
            .unwrap()
    }

    fn activate(&mut self, with_generator: bool, generation: u64, epoch: u64) {
        let request = self.queue_activation(Self::timeline(with_generator), generation, epoch);
        self.pump();
        let mut applied = 0;
        while let Some(event) = self.engine().next_timeline_runtime_event() {
            match event {
                TimelineRuntimeEvent::TransportActivationApplied {
                    request_id,
                    revision,
                    epoch: actual,
                    ..
                } => {
                    assert_eq!((request_id, revision, actual), (request, generation, epoch));
                    applied += 1;
                }
                TimelineRuntimeEvent::TransportActivationRejected { .. }
                | TimelineRuntimeEvent::Rejected { .. } => {
                    panic!("actual activation rejected: {event:?}")
                }
                _ => {}
            }
        }
        assert_eq!(applied, 1);
        let snapshot = self.callback().genuine_snapshot();
        assert_eq!(
            (snapshot.generation, snapshot.active_epoch, snapshot.epoch),
            (Some(generation), Some(epoch), epoch)
        );
        assert!(!snapshot.suspended && !snapshot.plugin_topology_replan_pending);
        assert!(snapshot.graph_binding_exact && snapshot.fault.is_none());
        assert!(!snapshot.timeline_stats.unwrap().ownership_needs_resync);
        assert_eq!(
            self.engine().confirmed_timeline_identity(),
            Some((generation, epoch))
        );
        self.engine().reclaim_retired_timeline_resources();
    }

    fn accepted(
        &mut self,
        before_count: usize,
        after_count: usize,
        counter: u64,
    ) -> GenuineLifecycleObservation {
        let records = self.callback().take_genuine_lifecycle_receipts();
        assert_eq!(records.len(), 1);
        let record = records[0];
        let pre = record.pre.expect("accepted mutation's exact pre-state");
        assert_eq!(record.run_binding, [91; 32]);
        assert!(record.producer_present && record.enqueue_success);
        assert_eq!(pre.generator_endpoints, before_count);
        assert_eq!(record.post.generator_endpoints, after_count);
        assert!(pre.graph_was_activated && pre.graph_binding_exact && !pre.playing);
        assert!(!pre.plugin_topology_replan_pending && !pre.suspended && !pre.needs_resync);
        assert_eq!(pre.timeline_execution_failures, counter);
        assert_eq!(record.post.timeline_execution_failures, counter + 1);
        assert_eq!(
            record.post.plugin_topology_revision,
            pre.plugin_topology_revision + 1
        );
        assert!(
            record.post.plugin_topology_replan_pending
                && record.post.suspended
                && record.post.needs_resync
        );
        assert!(pre.fault.is_none() && record.post.fault.is_none());
        assert_eq!(
            (
                pre.active_plugin_automation_bindings,
                pre.candidate_plugin_automation_bindings,
                pre.compiled_midi_routes
            ),
            (0, 0, 0)
        );
        assert_eq!(
            (
                record.post.channel_binding_revision,
                record.post.channel_binding_epoch
            ),
            (None, None)
        );
        assert_eq!(record.post.graph_identity, pre.graph_identity);
        assert_eq!(
            self.engine().next_generator_endpoint_event(),
            Some(record.event)
        );
        assert!(self.engine().next_generator_endpoint_event().is_none());
        assert!(self.callback().take_genuine_lifecycle_receipts().is_empty());
        let segments = self.callback().take_genuine_segments();
        let segment = segments.last().unwrap();
        assert_eq!(segment.pump_ordinal, record.pump_ordinal);
        assert_eq!(
            segment.disposition,
            GenuineSegmentDisposition::SuspendedSilence
        );
        assert_eq!(segment.before.timeline_execution_failures, counter + 1);
        assert!(
            self.callback()
                .last_output()
                .iter()
                .all(|frame| *frame == [0.0; 2])
        );
        assert!(!self.callback().genuine_snapshot().observation_overflowed);
        record
    }

    fn remove_and_join(&mut self, worker: usize) {
        self.engine().reclaim_retired_insert_endpoints();
        assert!(
            self.workers[worker].as_mut().unwrap().drain(),
            "terminal retained Joined required"
        );
    }

    fn finish(mut self) {
        self.cleanup();
        assert!(
            !ADMISSION_BLOCKED.load(Ordering::Acquire),
            "mock cleanup retained a failed owner"
        );
        assert!(
            self.workers
                .iter()
                .flatten()
                .all(|worker| worker.joined.load(Ordering::Acquire))
        );
    }

    fn cleanup(&mut self) {
        drop(self.callback.take());
        drop(self.engine.take());
        for worker in self.workers.iter_mut().flatten() {
            let _ = worker.drain();
        }
    }
}

impl Drop for TopologyFixture {
    fn drop(&mut self) {
        self.cleanup();
    }
}

#[test]
fn genuine_topology_four_actual_operations_have_exact_pairs_and_owned_recovery() {
    let mut fixture = TopologyFixture::new(true);
    let original = fixture.add_worker(700);
    fixture.queue_install(original, 70);
    fixture.pump();
    let initial = fixture.accepted(0, 1, 0);
    assert_eq!(initial.kind, GenuineLifecycleKind::Install);
    assert_eq!(initial.replaced, None);
    assert_eq!(initial.affected.unwrap().endpoint_id, 70);
    assert_eq!(initial.affected.unwrap().project_session, 7);
    assert_eq!(initial.affected.unwrap().epoch, 2);
    assert_eq!(
        initial.pre.unwrap().engine_session,
        fixture.engine().engine_session()
    );
    assert_eq!(initial.pre.unwrap().project_session, 7);
    assert_eq!(initial.pre.unwrap().transport_epoch, 2);
    assert_eq!(initial.pre.unwrap().generation, Some(10));
    assert_eq!(initial.pre.unwrap().active_epoch, Some(2));
    assert_eq!(initial.pre.unwrap().channel_binding_revision, Some(10));
    assert_eq!(initial.pre.unwrap().channel_binding_epoch, Some(2));
    assert_eq!(
        fixture.workers[original]
            .as_ref()
            .unwrap()
            .control
            .as_ref()
            .unwrap()
            .stats()
            .submitted,
        0
    );
    fixture.activate(true, 11, 3);
    fixture.callback().take_genuine_segments();

    let stamp = GeneratorRemovalStamp {
        operation_id: 101,
        engine_session: fixture.engine().engine_session(),
        project_session: 7,
        channel_id: 9,
        endpoint_id: 70,
        plugin_instance_id: 700,
        stopped_epoch: 3,
    };
    let before = fixture.workers[original]
        .as_ref()
        .unwrap()
        .control
        .as_ref()
        .unwrap()
        .stats()
        .submitted;
    assert!(fixture.engine().remove_generator_endpoint_stamped(stamp));
    fixture.pump();
    let conversion = fixture.accepted(1, 0, 1);
    assert_eq!(
        conversion.event,
        GeneratorEndpointEvent::StampedRemoved {
            stamp,
            result: Ok(())
        }
    );
    assert_eq!(
        conversion.affected.unwrap().worker_id,
        initial.affected.unwrap().worker_id
    );
    assert_eq!(
        fixture.workers[original]
            .as_ref()
            .unwrap()
            .control
            .as_ref()
            .unwrap()
            .stats()
            .submitted,
        before
    );
    fixture.remove_and_join(original);
    fixture.activate(false, 12, 4);
    fixture.callback().take_genuine_segments();

    let restored = fixture.add_worker(700);
    fixture.queue_install(restored, 71);
    fixture.pump();
    let undo = fixture.accepted(0, 1, 2);
    assert_eq!(undo.kind, GenuineLifecycleKind::Install);
    assert_eq!(undo.replaced, None);
    assert_eq!(undo.affected.unwrap().endpoint_id, 71);
    assert_ne!(
        undo.affected.unwrap().worker_id,
        initial.affected.unwrap().worker_id
    );
    fixture.activate(true, 13, 5);
    fixture.callback().take_genuine_segments();

    let before = fixture.workers[restored]
        .as_ref()
        .unwrap()
        .control
        .as_ref()
        .unwrap()
        .stats()
        .submitted;
    assert!(fixture.engine().remove_generator_endpoint(9));
    fixture.pump();
    let redo = fixture.accepted(1, 0, 3);
    assert_eq!(
        redo.event,
        GeneratorEndpointEvent::Removed {
            channel_id: 9,
            endpoint_id: Some(71),
            plugin_instance_id: Some(700)
        }
    );
    assert_eq!(
        redo.affected.unwrap().worker_id,
        undo.affected.unwrap().worker_id
    );
    assert_eq!(
        fixture.workers[restored]
            .as_ref()
            .unwrap()
            .control
            .as_ref()
            .unwrap()
            .stats()
            .submitted,
        before
    );
    fixture.remove_and_join(restored);
    fixture.activate(false, 14, 6);
    let settled = fixture.callback().genuine_snapshot();
    assert_eq!(settled.timeline_execution_failures, 4);
    assert_eq!(settled.lifecycle_observed_count, 4);
    assert!(settled.paused_work_quiescent(14, 6));
    fixture.finish();
}

#[test]
fn genuine_topology_rejected_stamp_and_missing_removal_never_acquire_a_pair() {
    let mut fixture = TopologyFixture::new(true);
    let worker = fixture.add_worker(700);
    fixture.queue_install(worker, 70);
    fixture.pump();
    fixture.accepted(0, 1, 0);
    fixture.activate(true, 11, 3);
    let before = fixture.callback().genuine_snapshot();
    let stamp = GeneratorRemovalStamp {
        operation_id: 101,
        engine_session: fixture.engine().engine_session(),
        project_session: 7,
        channel_id: 9,
        endpoint_id: 69,
        plugin_instance_id: 700,
        stopped_epoch: 3,
    };
    assert!(fixture.engine().remove_generator_endpoint_stamped(stamp));
    fixture.pump();
    let receipts = fixture.callback().take_genuine_lifecycle_receipts();
    assert_eq!(receipts.len(), 1);
    assert_eq!(
        receipts[0].event,
        GeneratorEndpointEvent::StampedRemoved {
            stamp,
            result: Err(GeneratorRemovalRejectReason::EndpointChanged),
        }
    );
    assert!(receipts[0].pre.is_none() && receipts[0].enqueue_success);
    assert_eq!(receipts[0].affected.unwrap().endpoint_id, 70);
    assert_eq!(
        fixture.engine().next_generator_endpoint_event(),
        Some(receipts[0].event)
    );
    assert_eq!(
        receipts[0].post.timeline_execution_failures,
        before.timeline_execution_failures
    );
    assert_eq!(
        receipts[0].post.plugin_topology_revision,
        before.plugin_topology_revision
    );
    assert!(fixture.engine().remove_generator_endpoint(99));
    fixture.pump();
    let missing = fixture.callback().take_genuine_lifecycle_receipts();
    assert_eq!(missing.len(), 1);
    assert!(missing[0].pre.is_none() && missing[0].affected.is_none());
    assert_eq!(
        missing[0].event,
        GeneratorEndpointEvent::Removed {
            channel_id: 99,
            endpoint_id: None,
            plugin_instance_id: None
        }
    );
    assert_eq!(
        missing[0].post.timeline_execution_failures,
        before.timeline_execution_failures
    );
    assert!(
        !fixture
            .callback()
            .genuine_snapshot()
            .lifecycle_inconsistent_pair
    );
    fixture.finish();
}

#[test]
fn genuine_topology_stale_prepared_topology_rejects_without_erasing_owned_receipt() {
    let mut fixture = TopologyFixture::new(true);
    let worker = fixture.add_worker(700);
    // This chase is prepared against the old topology. Production command order
    // accepts installation first, so the already queued chase cannot settle it.
    let request = fixture.queue_activation(TopologyFixture::timeline(true), 11, 3);
    fixture.queue_install(worker, 70);
    fixture.pump();
    let receipts = fixture.callback().take_genuine_lifecycle_receipts();
    assert_eq!(receipts.len(), 1);
    assert!(receipts[0].enqueue_success && receipts[0].pre.is_some());
    let mut rejected = 0;
    while let Some(event) = fixture.engine().next_timeline_runtime_event() {
        if let TimelineRuntimeEvent::TransportActivationRejected {
            request_id,
            revision,
            ..
        } = event
        {
            assert_eq!((request_id, revision), (request, 11));
            rejected += 1;
        }
        assert!(!matches!(
            event,
            TimelineRuntimeEvent::TransportActivationApplied { .. }
        ));
    }
    assert_eq!(rejected, 1);
    let state = fixture.callback().genuine_snapshot();
    assert!(state.suspended && state.plugin_topology_replan_pending);
    assert_ne!(state.active_epoch, Some(3));
    // Deliberately stop at the actual rejection: no recovery retry launders it.
    fixture.finish();
}

#[test]
fn genuine_topology_crossed_legacy_barrier_stays_visible_after_activation() {
    let mut fixture = TopologyFixture::new(true);
    let worker = fixture.add_worker(700);
    fixture.queue_install(worker, 70);
    fixture.pump();
    let accepted = fixture.accepted(0, 1, 0);
    let request = fixture.queue_activation(TopologyFixture::timeline(true), 11, 3);
    fixture.engine().seek_transport_to_frame(0.0, 0);
    fixture.pump();
    let mut applied = false;
    while let Some(event) = fixture.engine().next_timeline_runtime_event() {
        applied |= matches!(event, TimelineRuntimeEvent::TransportActivationApplied { request_id, .. } if request_id == request);
        assert!(!matches!(
            event,
            TimelineRuntimeEvent::TransportActivationRejected { .. }
                | TimelineRuntimeEvent::Rejected { .. }
        ));
    }
    assert!(applied);
    let state = fixture.callback().genuine_snapshot();
    assert!(state.graph_binding_exact);
    assert_ne!(
        state.request_id,
        accepted.pre.unwrap().legacy_request_barrier
    );
    assert_eq!(
        state.request_id,
        fixture.engine().test_transport_request_id()
    );
    // Exact graph health cannot hide the independent changed legacy domain.
    fixture.finish();
}

#[test]
fn genuine_topology_unsolicited_identity_drift_fault_has_no_owned_pair() {
    let mut fixture = TopologyFixture::new(true);
    let worker = fixture.add_worker(700);
    fixture.queue_install(worker, 70);
    fixture.pump();
    fixture.accepted(0, 1, 0);
    fixture.activate(true, 11, 3);
    fixture.callback().take_genuine_segments();
    let submitted_before = fixture.workers[worker]
        .as_ref()
        .unwrap()
        .control
        .as_ref()
        .unwrap()
        .stats()
        .submitted;
    // Negative injection models corruption, never the accepted/recovery path.
    fixture.callback().dsp.generator_endpoints[0]
        .as_mut()
        .unwrap()
        .endpoint_id = 71;
    fixture.pump();
    assert!(
        fixture
            .callback()
            .take_genuine_lifecycle_receipts()
            .is_empty()
    );
    let state = fixture.callback().genuine_snapshot();
    assert_eq!(
        state.fault.unwrap().reason,
        PluginProcessingFaultReason::EndpointChanged
    );
    // Graph/PDC identity checking precedes callback admission, so this fault
    // rejects the callback before render_transport_chunk can report FaultSilence.
    let segments = fixture.callback().take_genuine_segments();
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].frames, 128);
    assert_eq!(
        segments[0].disposition,
        GenuineSegmentDisposition::AdmissionRejected
    );
    assert_eq!(segments[0].before.fault, state.fault);
    assert_eq!(segments[0].after.fault, state.fault);
    assert_eq!(fixture.callback().last_output().len(), 128);
    assert!(
        fixture
            .callback()
            .last_output()
            .iter()
            .all(|frame| *frame == [0.0; 2])
    );
    assert_eq!(
        fixture.workers[worker]
            .as_ref()
            .unwrap()
            .control
            .as_ref()
            .unwrap()
            .stats()
            .submitted,
        submitted_before
    );
    fixture.finish();
}

#[test]
fn genuine_topology_owned_receipt_preserves_independent_admission_fault() {
    let mut fixture = TopologyFixture::new(true);
    let worker = fixture.add_worker(700);
    assert!(!fixture.callback().dsp.admit_plugin_callback(2049));
    let fault = fixture.callback().genuine_snapshot().fault;
    assert!(fault.is_some());
    fixture.queue_install(worker, 70);
    fixture.pump();
    let receipts = fixture.callback().take_genuine_lifecycle_receipts();
    assert_eq!(receipts.len(), 1);
    assert!(receipts[0].pre.is_some() && receipts[0].enqueue_success);
    assert_eq!(receipts[0].pre.unwrap().fault, fault);
    assert_eq!(receipts[0].post.fault, fault);
    assert_eq!(fixture.callback().genuine_snapshot().fault, fault);
    assert_eq!(
        fixture.callback().take_genuine_segments()[0].disposition,
        GenuineSegmentDisposition::AdmissionRejected
    );
    fixture.finish();
}

#[test]
fn genuine_topology_multiple_mutations_in_one_pump_keep_distinct_pre_states() {
    let mut fixture = TopologyFixture::new(true);
    let worker = fixture.add_worker(700);
    fixture.queue_install(worker, 70);
    assert!(fixture.engine().remove_generator_endpoint(9));
    fixture.pump();
    let records = fixture.callback().take_genuine_lifecycle_receipts();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].pump_ordinal, records[1].pump_ordinal);
    assert_eq!(
        (records[0].record_ordinal, records[1].record_ordinal),
        (0, 1)
    );
    assert_eq!(records[0].pre.unwrap().timeline_execution_failures, 0);
    assert_eq!(records[0].post.timeline_execution_failures, 1);
    assert_eq!(records[1].pre.unwrap().timeline_execution_failures, 1);
    assert_eq!(records[1].post.timeline_execution_failures, 2);
    assert!(records[1].pre.unwrap().plugin_topology_replan_pending);
    assert_eq!(
        fixture.engine().next_generator_endpoint_event(),
        Some(records[0].event)
    );
    assert_eq!(
        fixture.engine().next_generator_endpoint_event(),
        Some(records[1].event)
    );
    assert!(fixture.engine().next_generator_endpoint_event().is_none());
    assert!(
        !fixture
            .callback()
            .genuine_snapshot()
            .lifecycle_inconsistent_pair
    );
    fixture.remove_and_join(worker);
    fixture.finish();
}

#[test]
fn genuine_topology_all_lifecycle_variants_are_visible_without_extra_pairs() {
    let mut fixture = TopologyFixture::new(true);
    // Missing RouteSet and empty Cleared are real unrelated lifecycle operations.
    let callback = fixture.callback();
    crate::realtime_test_alloc::assert_no_alloc_or_drop(|| {
        callback.dsp.set_generator_route(99, 1);
        callback.dsp.clear_generator_endpoints(123);
    });
    let records = callback.take_genuine_lifecycle_receipts();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].kind, GenuineLifecycleKind::RouteSet);
    assert_eq!(records[1].kind, GenuineLifecycleKind::Clear);
    assert!(
        records
            .iter()
            .all(|record| record.pre.is_none() && record.enqueue_success)
    );
    for record in records {
        assert_eq!(
            fixture.engine().next_generator_endpoint_event(),
            Some(record.event)
        );
    }
    assert!(fixture.engine().next_generator_endpoint_event().is_none());
    fixture.finish();
}

#[test]
fn genuine_topology_disabled_enabled_and_full_collectors_preserve_original_events_and_pcm() {
    let mut reference = None;
    for observed in [false, true] {
        let mut fixture = TopologyFixture::new(observed);
        let mut actual = Vec::new();
        for ordinal in 0..=GENUINE_LIFECYCLE_CAPACITY {
            assert!(fixture.engine().remove_generator_endpoint(99));
            fixture.pump();
            actual.push(fixture.engine().next_generator_endpoint_event().unwrap());
            assert!(fixture.engine().next_generator_endpoint_event().is_none());
            assert!(
                fixture
                    .callback()
                    .last_output()
                    .iter()
                    .all(|frame| *frame == [0.0; 2])
            );
            let drained = fixture.callback().take_genuine_lifecycle_receipts();
            assert_eq!(
                drained.len(),
                usize::from(observed && ordinal < GENUINE_LIFECYCLE_CAPACITY)
            );
            if let Some(record) = drained.first() {
                assert_eq!(record.record_ordinal, ordinal as u64);
                assert!(record.enqueue_success && record.pre.is_none());
            }
            assert!(
                fixture
                    .callback()
                    .take_genuine_lifecycle_receipts()
                    .is_empty()
            );
        }
        if let Some(reference) = reference.as_ref() {
            assert_eq!(&actual, reference);
        } else {
            reference = Some(actual);
        }
        let state = fixture.callback().genuine_snapshot();
        assert_eq!(
            state.lifecycle_observed_count,
            if observed { 17 } else { 0 }
        );
        assert_eq!(state.lifecycle_overflowed, observed);
        assert!(!state.lifecycle_inconsistent_pair);
        assert_eq!(state.timeline_execution_failures, 0);
        assert!(state.graph_binding_exact && !state.suspended);
        assert_eq!(state.run_binding.is_some(), observed);
        fixture.finish();
    }
}

#[test]
fn genuine_topology_original_push_loss_and_absent_producer_are_observed_exactly() {
    for observed in [false, true] {
        let mut fixture = TopologyFixture::new(observed);
        let (mut producer, mut consumer) = RingBuffer::new(1);
        let sentinel = GeneratorEndpointEvent::Cleared {
            request_id: 555,
            removed: 0,
        };
        producer.push(sentinel).unwrap();
        fixture.callback().dsp.generator_endpoint_events = Some(producer);
        let callback = fixture.callback();
        crate::realtime_test_alloc::assert_no_alloc_or_drop(|| {
            callback.dsp.remove_generator_endpoint(99)
        });
        let records = callback.take_genuine_lifecycle_receipts();
        assert_eq!(records.len(), usize::from(observed));
        if observed {
            assert!(records[0].producer_present && !records[0].enqueue_success);
            assert!(records[0].pre.is_none());
        }
        assert_eq!(consumer.pop().unwrap(), sentinel);
        assert!(
            consumer.pop().is_err(),
            "failed original push must not be retried"
        );
        fixture.callback().dsp.generator_endpoint_events = None;
        let callback = fixture.callback();
        crate::realtime_test_alloc::assert_no_alloc_or_drop(|| {
            callback.dsp.remove_generator_endpoint(99)
        });
        let records = callback.take_genuine_lifecycle_receipts();
        assert_eq!(records.len(), usize::from(observed));
        if observed {
            assert!(!records[0].producer_present && !records[0].enqueue_success);
        }
        fixture.finish();
    }
}

#[test]
fn genuine_topology_assertion_unwind_retains_and_joins_every_spawned_mock() {
    let joined = Arc::new(Mutex::new([const { None }; 4]));
    let evidence = Arc::clone(&joined);
    let outcome = std::panic::catch_unwind(move || {
        let mut fixture = TopologyFixture::new(true);
        for _ in 0..4 {
            let index = fixture.add_worker(700);
            evidence.lock().unwrap()[index] =
                Some(Arc::clone(&fixture.workers[index].as_ref().unwrap().joined));
        }
        panic!("deliberate owner-retention unwind");
    });
    assert!(outcome.is_err());
    let joined = joined.lock().unwrap();
    assert!(joined.iter().all(|joined| {
        joined
            .as_ref()
            .is_some_and(|joined| joined.load(Ordering::Acquire))
    }));
    assert!(!ADMISSION_BLOCKED.load(Ordering::Acquire));
    assert!(
        QUARANTINE
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .all(|slot| matches!(slot, RetentionSlot::Empty))
    );
}

#[test]
fn genuine_topology_replacement_records_both_real_workers_and_rejected_install_has_no_pair() {
    let mut fixture = TopologyFixture::new(true);
    let first = fixture.add_worker(700);
    fixture.queue_install(first, 70);
    fixture.pump();
    let initial = fixture.accepted(0, 1, 0);
    fixture.activate(true, 11, 3);
    fixture.callback().take_genuine_segments();
    let replacement = fixture.add_worker(700);
    fixture.queue_install(replacement, 71);
    fixture.pump();
    let replaced = fixture.accepted(1, 1, 1);
    assert_eq!(
        replaced.replaced,
        Some(genuine_observation::GenuineLifecycleEndpoint {
            epoch: 3,
            ..initial.affected.unwrap()
        })
    );
    assert_eq!(replaced.affected.unwrap().endpoint_id, 71);
    assert_ne!(
        replaced.affected.unwrap().worker_id,
        replaced.replaced.unwrap().worker_id
    );
    fixture.remove_and_join(first);
    fixture.activate(true, 12, 4);
    let rejected = fixture.add_worker(700);
    let audio = fixture.workers[rejected]
        .as_mut()
        .unwrap()
        .audio
        .take()
        .unwrap();
    let mut endpoint = fixed_adapter(audio);
    endpoint.project_session = 7;
    let pdc = test_pdc_delay();
    let callback = fixture.callback();
    crate::realtime_test_alloc::assert_no_alloc_or_drop(|| {
        callback
            .dsp
            .install_generator_endpoint(9, 0, 700, 1, endpoint, pdc);
    });
    let records = callback.take_genuine_lifecycle_receipts();
    assert_eq!(records.len(), 1);
    assert!(records[0].pre.is_none());
    assert_eq!(records[0].affected.unwrap().endpoint_id, 0);
    assert_eq!(records[0].affected.unwrap().project_session, 7);
    assert!(matches!(
        records[0].event,
        GeneratorEndpointEvent::Installed { success: false, .. }
    ));
    assert_eq!(records[0].post.timeline_execution_failures, 2);
    assert!(!records[0].post.suspended);
    fixture.remove_and_join(rejected);
    fixture.finish();
}

#[test]
fn genuine_topology_nonzero_automation_premise_is_captured_before_binding_clear() {
    let mut fixture = TopologyFixture::new(true);
    let worker = fixture.add_worker(700);
    fixture.queue_install(worker, 70);
    fixture.pump();
    fixture.accepted(0, 1, 0);
    let mut project = TopologyFixture::project(true);
    push_timeline_automation(
        &mut project,
        1,
        AutomationTarget::PluginParameter {
            instance: 700,
            parameter: 9,
        },
        AutomationCurve::Linear,
        [
            AutomationPoint::new(0.0, 0.2),
            AutomationPoint::new(1.0, 0.8),
        ],
    );
    let request = fixture.queue_activation(compile_timeline_test_project(&project), 11, 3);
    fixture.pump();
    let mut applied = false;
    while let Some(event) = fixture.engine().next_timeline_runtime_event() {
        applied |= matches!(event, TimelineRuntimeEvent::TransportActivationApplied { request_id, .. } if request_id == request);
        assert!(!matches!(
            event,
            TimelineRuntimeEvent::TransportActivationRejected { .. }
                | TimelineRuntimeEvent::Rejected { .. }
        ));
    }
    assert!(applied);
    assert_eq!(
        fixture
            .callback()
            .genuine_snapshot()
            .active_plugin_automation_bindings,
        1
    );
    assert!(fixture.engine().remove_generator_endpoint(9));
    fixture.pump();
    let records = fixture.callback().take_genuine_lifecycle_receipts();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].pre.unwrap().active_plugin_automation_bindings, 1);
    assert_eq!(records[0].post.active_plugin_automation_bindings, 0);
    assert!(records[0].enqueue_success);
    // Nonzero pre-route evidence stays disqualifying even after clearing it.
    fixture.remove_and_join(worker);
    fixture.finish();
}

#[test]
fn genuine_topology_playing_install_records_actual_running_precondition() {
    let mut fixture = TopologyFixture::new(true);
    fixture.engine().set_playing(true);
    fixture.pump();
    assert!(fixture.callback().genuine_snapshot().playing);
    let worker = fixture.add_worker(700);
    fixture.queue_install(worker, 70);
    fixture.pump();
    let records = fixture.callback().take_genuine_lifecycle_receipts();
    assert_eq!(records.len(), 1);
    assert!(records[0].pre.unwrap().playing);
    assert!(records[0].enqueue_success && records[0].post.suspended);
    fixture.finish();
}

#[test]
fn genuine_topology_full_collector_cannot_change_accepted_mutation_or_event() {
    let mut reference = None;
    for mode in 0..3 {
        let mut fixture = TopologyFixture::new(mode != 0);
        if mode == 2 {
            for _ in 0..GENUINE_LIFECYCLE_CAPACITY {
                assert!(fixture.engine().remove_generator_endpoint(99));
                fixture.pump();
                fixture.engine().next_generator_endpoint_event().unwrap();
            }
            assert_eq!(
                fixture.callback().take_genuine_lifecycle_receipts().len(),
                GENUINE_LIFECYCLE_CAPACITY
            );
        }
        let worker = fixture.add_worker(700);
        fixture.queue_install(worker, 70);
        fixture.pump();
        let event = fixture.engine().next_generator_endpoint_event().unwrap();
        assert!(matches!(
            event,
            GeneratorEndpointEvent::Installed {
                success: true,
                endpoint_id: 70,
                ..
            }
        ));
        assert!(fixture.engine().next_generator_endpoint_event().is_none());
        let state = fixture.callback().genuine_snapshot();
        let result = (
            event,
            state.plugin_topology_revision,
            state.timeline_execution_failures,
            state.plugin_topology_replan_pending,
            state.endpoints.len(),
            state.suspended,
            fixture.callback().last_output().to_vec(),
        );
        if let Some(reference) = reference.as_ref() {
            assert_eq!(&result, reference);
        } else {
            reference = Some(result);
        }
        let receipts = fixture.callback().take_genuine_lifecycle_receipts();
        assert_eq!(receipts.len(), usize::from(mode == 1));
        assert_eq!(state.lifecycle_overflowed, mode == 2);
        assert!(!state.lifecycle_inconsistent_pair);
        fixture.finish();
    }
}

#[test]
fn genuine_topology_real_event_and_retirement_backpressure_defers_without_receipt_or_mutation() {
    for retirement_full in [false, true] {
        let mut fixture = TopologyFixture::new(true);
        let original = fixture.add_worker(700);
        fixture.queue_install(original, 70);
        fixture.pump();
        fixture.accepted(0, 1, 0);
        fixture.activate(true, 11, 3);
        fixture.callback().take_genuine_segments();
        // AudioEngine::command reclaims retired resources. Queue first, then
        // create actual callback-side pressure before the command is serviced.
        assert!(fixture.engine().remove_generator_endpoint(9));
        let extra = if retirement_full {
            let extra = fixture.add_worker(700);
            let (producer, consumer) = RingBuffer::new(1);
            fixture.callback().dsp.retired_insert_endpoints = Some(producer);
            fixture.engine().retired_insert_endpoints = consumer;
            let audio = fixture.workers[extra]
                .as_mut()
                .unwrap()
                .audio
                .take()
                .unwrap();
            let endpoint = fixed_adapter(audio);
            fixture.callback().dsp.retire_insert_endpoint(endpoint);
            Some(extra)
        } else {
            let (mut producer, consumer) = RingBuffer::new(1);
            producer
                .push(GeneratorEndpointEvent::Cleared {
                    request_id: 555,
                    removed: 0,
                })
                .unwrap();
            fixture.callback().dsp.generator_endpoint_events = Some(producer);
            fixture.engine().generator_endpoint_events = consumer;
            None
        };
        let before = fixture.callback().genuine_snapshot();
        fixture.pump();
        let deferred = fixture.callback().genuine_snapshot();
        assert_eq!(deferred.pending_audio_commands, 1);
        assert_eq!(
            deferred.timeline_execution_failures,
            before.timeline_execution_failures
        );
        assert_eq!(
            deferred.plugin_topology_revision,
            before.plugin_topology_revision
        );
        assert_eq!(deferred.graph_identity, before.graph_identity);
        assert!(deferred.graph_binding_exact && !deferred.suspended);
        assert!(!deferred.plugin_topology_replan_pending);
        assert_eq!(deferred.endpoints.len(), 1);
        assert_eq!(deferred.endpoints[0].endpoint_id, 70);
        assert_eq!(
            deferred.endpoints[0].fixed.bridge.worker_id,
            before.endpoints[0].fixed.bridge.worker_id
        );
        assert!(
            fixture
                .callback()
                .take_genuine_lifecycle_receipts()
                .is_empty()
        );
        let segments = fixture.callback().take_genuine_segments();
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].disposition, GenuineSegmentDisposition::Rendered);
        if let Some(extra) = extra {
            fixture.remove_and_join(extra);
        } else {
            assert_eq!(
                fixture.engine().next_generator_endpoint_event(),
                Some(GeneratorEndpointEvent::Cleared {
                    request_id: 555,
                    removed: 0
                })
            );
        }
        fixture.pump();
        let accepted = fixture.accepted(1, 0, 1);
        assert_eq!(
            accepted.affected.unwrap().worker_id,
            before.endpoints[0].fixed.bridge.worker_id
        );
        assert_eq!(
            fixture.callback().genuine_snapshot().pending_audio_commands,
            0
        );
        fixture.remove_and_join(original);
        fixture.finish();
    }
}

#[test]
fn genuine_topology_duplicate_publication_cannot_reuse_a_consumed_mutation_pair() {
    let mut fixture = TopologyFixture::new(true);
    let worker = fixture.add_worker(700);
    fixture.queue_install(worker, 70);
    fixture.pump();
    let accepted = fixture.accepted(0, 1, 0);
    let before = fixture.callback().genuine_snapshot();
    // Collector-only adversarial publication: the original copied event is
    // pushed a second time, with no second kernel mutation to authorize it.
    let callback = fixture.callback();
    crate::realtime_test_alloc::assert_no_alloc_or_drop(|| {
        callback.dsp.emit_generator_endpoint_event(accepted.event);
    });
    let duplicate = callback.take_genuine_lifecycle_receipts();
    assert_eq!(duplicate.len(), 1);
    assert_eq!(duplicate[0].event, accepted.event);
    assert!(duplicate[0].enqueue_success && duplicate[0].pre.is_none());
    assert_eq!(duplicate[0].record_ordinal, accepted.record_ordinal + 1);
    assert_eq!(
        fixture.engine().next_generator_endpoint_event(),
        Some(accepted.event)
    );
    let after = fixture.callback().genuine_snapshot();
    assert!(after.lifecycle_inconsistent_pair && after.observation_overflowed);
    assert_eq!(
        after.plugin_topology_revision,
        before.plugin_topology_revision
    );
    assert_eq!(
        after.timeline_execution_failures,
        before.timeline_execution_failures
    );
    assert!(
        fixture
            .callback()
            .take_genuine_lifecycle_receipts()
            .is_empty()
    );
    assert!(
        fixture
            .callback()
            .genuine_snapshot()
            .lifecycle_inconsistent_pair
    );
    fixture.finish();
}

#[test]
fn genuine_topology_revision_mapping_is_actual_nonzero_wrap() {
    assert_eq!(next_nonzero_id(0), 1);
    assert_eq!(next_nonzero_id(41), 42);
    assert_eq!(next_nonzero_id(u64::MAX), 1);
}
