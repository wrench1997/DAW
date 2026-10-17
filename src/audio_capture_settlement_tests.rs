use crate::plugins::plugin_runtime::{BounceSourceSettlement, RetainedShutdownPoll};

static CAPTURE_CALLBACK_CLEANUP_FAILED: AtomicBool = AtomicBool::new(false);
enum CaptureWorkerQuarantine {
    Vacant,
    Reserved,
    Failed {
        _worker_id: usize,
        _outcome: RetainedShutdownPoll,
        _guard: std::mem::ManuallyDrop<PluginWorkerGuard>,
    },
}

thread_local! {
    // Fixed reserved slots cannot allocate or detach an unresolved owner during unwind.
    static CAPTURE_CALLBACK_QUARANTINE: std::cell::RefCell<[CaptureWorkerQuarantine; 4]> =
        const { std::cell::RefCell::new([const { CaptureWorkerQuarantine::Vacant }; 4]) };
}

struct RetainedCaptureWorker {
    guard: Option<PluginWorkerGuard>,
    quarantine_slot: usize,
}

impl Drop for RetainedCaptureWorker {
    fn drop(&mut self) {
        let Some(guard) = self.guard.as_mut() else {
            return;
        };
        let worker_id = guard.genuine_worker_id();
        let deadline = Instant::now() + Duration::from_secs(2);
        let outcome = loop {
            let outcome = guard.poll_shutdown_retained();
            if outcome != RetainedShutdownPoll::Pending || Instant::now() >= deadline {
                break outcome;
            }
            thread::yield_now();
        };
        let guard = self
            .guard
            .take()
            .expect("the exact retained guard remains owned");
        if outcome == RetainedShutdownPoll::Joined {
            drop(guard);
            CAPTURE_CALLBACK_QUARANTINE.with(|owners| {
                owners.borrow_mut()[self.quarantine_slot] = CaptureWorkerQuarantine::Vacant;
            });
            return;
        }
        CAPTURE_CALLBACK_CLEANUP_FAILED.store(true, Ordering::Release);
        CAPTURE_CALLBACK_QUARANTINE.with(|owners| {
            owners.borrow_mut()[self.quarantine_slot] = CaptureWorkerQuarantine::Failed {
                _worker_id: worker_id,
                _outcome: outcome,
                _guard: std::mem::ManuallyDrop::new(guard),
            };
        });
        if !thread::panicking() {
            panic!(
                "callback fixture worker {worker_id} cleanup failed: {outcome:?}; exact owner quarantined"
            );
        }
    }
}

fn spawn_capture_callback_worker() -> (
    AudioThreadEndpoint,
    PluginChainControl,
    RetainedCaptureWorker,
) {
    assert!(
        !CAPTURE_CALLBACK_CLEANUP_FAILED.load(Ordering::Acquire),
        "a prior callback fixture worker remains unproven; later admission refused"
    );
    let quarantine_slot = CAPTURE_CALLBACK_QUARANTINE.with(|owners| {
        let mut owners = owners.borrow_mut();
        let slot = owners
            .iter()
            .position(|owner| matches!(owner, CaptureWorkerQuarantine::Vacant))
            .expect("callback fixture quarantine capacity must be reserved before spawning");
        owners[slot] = CaptureWorkerQuarantine::Reserved;
        slot
    });
    let chain = PluginChain::spawn_identified_with_backend_factory(
        &[CAPTURE_INSTANCE],
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
            max_block_frames: DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES,
        },
    );
    let PluginChain {
        audio,
        control,
        guard,
    } = match chain {
        Ok(chain) => chain,
        Err(error) => {
            CAPTURE_CALLBACK_QUARANTINE.with(|owners| {
                owners.borrow_mut()[quarantine_slot] = CaptureWorkerQuarantine::Vacant;
            });
            panic!("plugin-free callback fixture worker failed to start: {error}");
        }
    };
    // Acquire exact lifetime ownership before any post-spawn assertion can unwind.
    let guard = RetainedCaptureWorker {
        guard: Some(guard),
        quarantine_slot,
    };
    wait_until(|| {
        control
            .plugin_latency_snapshot()
            .is_some_and(|snapshot| snapshot.revision != 0 && snapshot.slot_is_active(0))
    });
    (audio, control, guard)
}

const CAPTURE_PROJECT: u64 = 71;
const CAPTURE_CHANNEL: u32 = 9;
const CAPTURE_ENDPOINT: u64 = 901;
const CAPTURE_INSTANCE: u64 = 9001;
const CAPTURE_GENERATION: u64 = 81;
const CAPTURE_EPOCH: u64 = 17;

/// Plugin-free fixture: installation, stopped activation, finite safety service and worker
/// settlement use the real production rings/helpers. No readiness publication is fabricated.
struct CaptureCallbackFixture {
    callback: TestAudioCallback,
    engine: AudioEngine,
    control: PluginChainControl,
    _guard: RetainedCaptureWorker,
    target: BounceSourceCallbackIdentity,
}

impl CaptureCallbackFixture {
    fn new() -> Self {
        let (audio, control, guard) = spawn_capture_callback_worker();
        let (mut engine, mut callback) = AudioEngine::test_engine_with_callback();
        assert!(engine.set_project_session(CAPTURE_PROJECT));
        assert!(engine.install_generator_endpoint_for_project(
            CAPTURE_PROJECT,
            CAPTURE_CHANNEL,
            CAPTURE_ENDPOINT,
            CAPTURE_INSTANCE,
            1,
            audio,
        ));
        callback.pump(&engine, DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
        assert!(matches!(
            engine.next_generator_endpoint_event(),
            Some(GeneratorEndpointEvent::Installed { success: true, .. })
        ));

        let mut project = timeline_test_project(4.0);
        let mut channel = timeline_test_channel(CAPTURE_CHANNEL, 1);
        channel.instrument_plugin_instance_id = Some(CAPTURE_INSTANCE);
        project.channels.push(channel);
        project
            .plugin_instances
            .push(timeline_test_plugin(CAPTURE_INSTANCE));
        let timeline = compile_timeline_test_project(&project);
        let chase = engine
            .prepare_timeline_chase(
                &timeline,
                CAPTURE_GENERATION,
                CAPTURE_EPOCH,
                0,
                TimelineChaseOptions::default(),
            )
            .unwrap();
        let loop_chase = engine
            .prepare_timeline_loop_chase(
                &timeline,
                CAPTURE_GENERATION,
                0,
                TimelineChaseOptions::default(),
            )
            .unwrap();
        engine
            .install_compiled_timeline(CAPTURE_GENERATION, timeline)
            .unwrap();
        let loop_token = engine.install_timeline_loop_chase(loop_chase).unwrap();
        engine.install_timeline_chase(chase).unwrap();
        engine.set_playing(false);
        let legacy_request = engine.transport_request_id();
        let activation = engine
            .activate_timeline_transport(TimelineTransportActivationSpec {
                revision: CAPTURE_GENERATION,
                target_epoch: CAPTURE_EPOCH,
                minimum_epoch: CAPTURE_EPOCH,
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
            .unwrap();
        assert_ne!(
            activation, legacy_request,
            "the ID domains must remain distinct"
        );
        callback.pump(&engine, DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
        let mut confirmed = false;
        while let Some(event) = engine.next_timeline_runtime_event() {
            if let TimelineRuntimeEvent::TransportActivationApplied {
                request_id,
                revision,
                epoch,
                frame,
            } = event
            {
                assert_eq!(
                    (request_id, revision, epoch, frame),
                    (activation, CAPTURE_GENERATION, CAPTURE_EPOCH, 0)
                );
                assert!(!callback.transport.request.playing);
                confirmed = true;
            }
        }
        assert!(confirmed);
        let target = BounceSourceCallbackIdentity {
            engine_session: engine.engine_session(),
            project_session: CAPTURE_PROJECT,
            timeline_generation: CAPTURE_GENERATION,
            epoch: CAPTURE_EPOCH,
            transport_request: legacy_request,
            channel_id: CAPTURE_CHANNEL,
            endpoint_id: CAPTURE_ENDPOINT,
            instance_id: CAPTURE_INSTANCE,
        };
        // Worker-only settlement must not receive another callback or extra DSP service.
        wait_until(|| control.bounce_source_settlement(target) == BounceSourceSettlement::Ready);
        Self {
            callback,
            engine,
            control,
            _guard: guard,
            target,
        }
    }

    fn slot(&self) -> &GeneratorEndpointSlot {
        self.callback
            .dsp
            .generator_endpoints
            .iter()
            .flatten()
            .next()
            .unwrap()
    }

    fn slot_mut(&mut self) -> &mut GeneratorEndpointSlot {
        self.callback
            .dsp
            .generator_endpoints
            .iter_mut()
            .flatten()
            .next()
            .unwrap()
    }

    fn republish_after_read_only_check(&self) {
        self.callback
            .dsp
            .finish_bounce_source_callback(&self.callback.transport, false);
    }

    fn assert_waiting(&self) {
        assert_eq!(
            self.control.bounce_source_settlement(self.target),
            BounceSourceSettlement::Waiting
        );
    }
}

#[test]
fn capture_callback_exact_identity_and_retained_output_use_production_boundary() {
    let fixture = CaptureCallbackFixture::new();
    let before = fixture.slot().endpoint.adapter.genuine_snapshot();
    let observation = fixture
        .control
        .observe_bounce_source_settlement(fixture.target);
    assert!(observation.coherent && observation.callback_ready);
    assert_eq!(observation.callback_identity, Some(fixture.target));
    assert_eq!(observation.submitted, observation.consumed);
    assert_eq!(observation.submitted, observation.terminal);
    assert!(before.output_owned_frames > 0 || before.bridge.output_owned > 0);
    fixture.republish_after_read_only_check();
    let after = fixture.slot().endpoint.adapter.genuine_snapshot();
    assert_eq!(before.stats, after.stats);
    assert_eq!(before.output_owned_frames, after.output_owned_frames);
    assert_eq!(before.output_position, after.output_position);
    assert_eq!(
        before.accumulation_preroll_frames_remaining,
        after.accumulation_preroll_frames_remaining
    );
    for changed in [
        BounceSourceCallbackIdentity {
            engine_session: fixture.target.engine_session + 1,
            ..fixture.target
        },
        BounceSourceCallbackIdentity {
            project_session: CAPTURE_PROJECT + 1,
            ..fixture.target
        },
        BounceSourceCallbackIdentity {
            timeline_generation: CAPTURE_GENERATION + 1,
            ..fixture.target
        },
        BounceSourceCallbackIdentity {
            epoch: CAPTURE_EPOCH + 1,
            ..fixture.target
        },
        BounceSourceCallbackIdentity {
            transport_request: fixture.target.transport_request + 1,
            ..fixture.target
        },
        BounceSourceCallbackIdentity {
            channel_id: CAPTURE_CHANNEL + 1,
            ..fixture.target
        },
        BounceSourceCallbackIdentity {
            endpoint_id: CAPTURE_ENDPOINT + 1,
            ..fixture.target
        },
        BounceSourceCallbackIdentity {
            instance_id: CAPTURE_INSTANCE + 1,
            ..fixture.target
        },
    ] {
        assert_eq!(
            fixture.control.bounce_source_settlement(changed),
            BounceSourceSettlement::Invalid
        );
    }
}

#[test]
fn capture_callback_synthetic_pending_lanes_partial_usage_safety_and_edits_prevent_ready() {
    let mut fixture = CaptureCallbackFixture::new();
    for class in [
        EndpointEventClass::System,
        EndpointEventClass::Timeline,
        EndpointEventClass::Live,
    ] {
        assert!(
            fixture
                .slot_mut()
                .endpoint
                .stage_class(class, FrameEvent::midi(0, None, [0xB0, 123, 0]))
        );
        fixture.republish_after_read_only_check();
        fixture.assert_waiting();
        // Synthetic lane-isolation portion: do not service DSP to clear another lane.
        let scratch = &mut fixture.slot_mut().endpoint.events.scratch;
        scratch.system.clear();
        scratch.timeline.clear();
        scratch.live.clear();
    }
    fixture.slot_mut().endpoint.partial_quantum_usage.live = 1;
    fixture.republish_after_read_only_check();
    fixture.assert_waiting();
    fixture.slot_mut().endpoint.partial_quantum_usage = EndpointQuantumClassUsage::default();
    fixture.slot_mut().endpoint.events.scratch.live_faulted = true;
    fixture.republish_after_read_only_check();
    fixture.assert_waiting();
    fixture.slot_mut().endpoint.events.scratch.live_faulted = false;
    fixture.slot_mut().endpoint.stopped_epoch_safety = Some(StoppedEpochSafetyService {
        epoch: CAPTURE_EPOCH,
        remaining_frames: 128,
        failed: false,
    });
    fixture.republish_after_read_only_check();
    fixture.assert_waiting();
    fixture.slot_mut().endpoint.stopped_epoch_safety = None;
    fixture.slot_mut().endpoint.admitted_live_edit_markers = 1;
    fixture.republish_after_read_only_check();
    fixture.assert_waiting();
    fixture.slot_mut().endpoint.admitted_live_edit_markers = 0;
    fixture
        .callback
        .dsp
        .parameter_edit_callback_admission
        .store(1, Ordering::Release);
    fixture.republish_after_read_only_check();
    fixture.assert_waiting();
    fixture
        .callback
        .dsp
        .parameter_edit_callback_admission
        .store(0, Ordering::Release);
    fixture.callback.dsp.paused_midi_safety[0] = Some(PausedMidiSafetyService {
        stamp: MidiGeneratorRouteStamp {
            project_session: CAPTURE_PROJECT,
            channel_id: CAPTURE_CHANNEL,
            endpoint_id: CAPTURE_ENDPOINT,
            plugin_instance_id: CAPTURE_INSTANCE,
            slot: Some(0),
        },
        remaining_frames: 128,
    });
    fixture.republish_after_read_only_check();
    fixture.assert_waiting();
    fixture.callback.dsp.paused_midi_safety[0] = None;
    fixture.republish_after_read_only_check();
    assert_eq!(
        fixture.control.bounce_source_settlement(fixture.target),
        BounceSourceSettlement::Ready
    );
}

#[test]
fn capture_callback_actual_partial_input_and_fixed_events_prevent_ready() {
    let mut fixture = CaptureCallbackFixture::new();
    fixture.callback.dsp.begin_bounce_source_callback();
    let mut left = [0.0];
    let mut right = [0.0];
    let endpoint = &mut fixture.slot_mut().endpoint;
    assert!(endpoint.stage_class(
        EndpointEventClass::Timeline,
        FrameEvent::midi(0, None, [0xB0, 123, 0])
    ));
    let _ = endpoint.process_generator(CAPTURE_EPOCH, 1, &mut left, &mut right);
    let actual = fixture.slot().endpoint.adapter.genuine_snapshot();
    assert_eq!(actual.input_phase_frames, 1);
    assert_ne!(actual.pending_event_count, 0);
    fixture.republish_after_read_only_check();
    fixture.assert_waiting();
}

#[test]
fn capture_callback_entry_revokes_before_commands_and_admission_fault_stays_invalid() {
    let mut fixture = CaptureCallbackFixture::new();
    fixture.callback.dsp.begin_bounce_source_callback();
    fixture.assert_waiting();
    // A pending command leaves the shared completion helper unpublished.
    fixture
        .callback
        .dsp
        .finish_bounce_source_callback(&fixture.callback.transport, true);
    fixture.assert_waiting();
    fixture.republish_after_read_only_check();
    // The callback publication is restored, but independent worker/admin work
    // may transiently prevent a coherent Ready observation. Observe only;
    // another callback or publication must not repair this boundary.
    wait_until(|| {
        fixture.control.bounce_source_settlement(fixture.target) == BounceSourceSettlement::Ready
    });
    // Force the real admission early return without modifying any shared publication.
    fixture.callback.dsp.plugin_timing.callback_budget_frames = 1;
    fixture
        .callback
        .pump(&fixture.engine, DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
    assert!(fixture.callback.dsp.plugin_fault.is_some());
    let observed = fixture
        .control
        .observe_bounce_source_settlement(fixture.target);
    assert!(!observed.callback_ready);
    assert_ne!(observed.settlement, BounceSourceSettlement::Ready);
}

#[test]
fn capture_callback_synthetic_suspension_resync_and_unhealthy_segment_never_publish() {
    let mut fixture = CaptureCallbackFixture::new();
    fixture.callback.dsp.begin_bounce_source_callback();
    fixture.callback.dsp.plugin_topology_replan_pending = true;
    fixture.republish_after_read_only_check();
    fixture.assert_waiting();
    fixture.callback.dsp.plugin_topology_replan_pending = false;
    fixture.callback.dsp.bounce_source_callback_healthy = false;
    fixture.republish_after_read_only_check();
    fixture.assert_waiting();
    fixture.callback.dsp.bounce_source_callback_healthy = true;
    fixture
        .callback
        .dsp
        .timeline_runtime
        .as_mut()
        .unwrap()
        .require_resync();
    fixture.republish_after_read_only_check();
    fixture.assert_waiting();
}

#[test]
fn capture_callback_removal_retirement_and_dsp_forget_revoke_old_certificate() {
    let mut removed = CaptureCallbackFixture::new();
    removed
        .callback
        .dsp
        .remove_generator_endpoint(CAPTURE_CHANNEL);
    assert_ne!(
        removed.control.bounce_source_settlement(removed.target),
        BounceSourceSettlement::Ready
    );
    assert!(
        removed
            .callback
            .dsp
            .generator_endpoints
            .iter()
            .all(Option::is_none)
    );
    let retired = removed.engine.retired_insert_endpoints.pop().unwrap();
    drop(retired);

    let mut cleared = CaptureCallbackFixture::new();
    cleared.callback.dsp.clear_generator_endpoints(0);
    assert_ne!(
        cleared.control.bounce_source_settlement(cleared.target),
        BounceSourceSettlement::Ready
    );

    let mut abandoned = CaptureCallbackFixture::new();
    let empty = Box::write(Box::new_uninit(), DspState::new(48_000.0));
    let old = std::mem::replace(&mut abandoned.callback.dsp, empty);
    drop(old); // The production fail-safe mem::forgets the endpoint after explicit revocation.
    assert_ne!(
        abandoned.control.bounce_source_settlement(abandoned.target),
        BounceSourceSettlement::Ready
    );
    drop((removed, cleared, abandoned));

    // Intentional assertion unwind exercises the exact retained owner. A reservation becomes
    // Vacant only after poll_shutdown_retained reports Joined for that same handle.
    let reservation = std::cell::Cell::new(None);
    let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let fixture = CaptureCallbackFixture::new();
        reservation.set(Some((
            fixture._guard.quarantine_slot,
            fixture._guard.guard.as_ref().unwrap().genuine_worker_id(),
        )));
        assert_eq!(
            fixture.target.endpoint_id,
            CAPTURE_ENDPOINT + 1,
            "intentional callback fixture ownership unwind"
        );
    }));
    assert!(unwind.is_err());
    let (slot, worker_id) = reservation.get().expect("exact fixture owner was captured");
    assert_ne!(worker_id, 0);
    CAPTURE_CALLBACK_QUARANTINE.with(|owners| {
        assert!(matches!(
            owners.borrow()[slot],
            CaptureWorkerQuarantine::Vacant
        ));
    });
    assert!(!CAPTURE_CALLBACK_CLEANUP_FAILED.load(Ordering::Acquire));
    drop(CaptureCallbackFixture::new());
}

#[test]
fn capture_callback_replacement_and_stamped_removal_revoke_exact_source() {
    let mut replaced = CaptureCallbackFixture::new();
    let (audio, control, guard) = spawn_capture_callback_worker();
    assert!(replaced.engine.install_generator_endpoint_for_project(
        CAPTURE_PROJECT,
        CAPTURE_CHANNEL,
        CAPTURE_ENDPOINT + 1,
        CAPTURE_INSTANCE,
        1,
        audio,
    ));
    replaced
        .callback
        .pump(&replaced.engine, DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
    assert_eq!(replaced.slot().endpoint_id, CAPTURE_ENDPOINT + 1);
    assert_ne!(
        replaced.control.bounce_source_settlement(replaced.target),
        BounceSourceSettlement::Ready
    );
    drop(replaced);
    drop(guard);
    drop(control);

    let mut removed = CaptureCallbackFixture::new();
    let stamp = GeneratorRemovalStamp {
        operation_id: 1,
        engine_session: removed.target.engine_session,
        project_session: CAPTURE_PROJECT,
        channel_id: CAPTURE_CHANNEL,
        endpoint_id: CAPTURE_ENDPOINT,
        plugin_instance_id: CAPTURE_INSTANCE,
        stopped_epoch: CAPTURE_EPOCH,
    };
    assert!(removed.engine.remove_generator_endpoint_stamped(stamp));
    removed
        .callback
        .pump(&removed.engine, DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
    assert!(matches!(removed.engine.next_generator_endpoint_event(),
        Some(GeneratorEndpointEvent::StampedRemoved { stamp: returned, result: Ok(()) }) if returned == stamp));
    assert_ne!(
        removed.control.bounce_source_settlement(removed.target),
        BounceSourceSettlement::Ready
    );
}
