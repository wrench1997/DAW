//! Actual App/controller/callback/worker proofs of the pre-capture wait. These
//! mock backends hold real reset/Process ownership; no ready tuple is fabricated.
//! Fixture owns the exact worker guard and releases every gate before unwind
//! cleanup, then proves retained shutdown/join using its existing bounded owner.
use super::*;

const SETTLEMENT_WAIT: Duration = Duration::from_secs(4);

impl Fixture {
    fn settlement_control_poll(&mut self) {
        self.app.drive_clip_session_retirement();
        self.app.poll_clip_restorations();
        self.app.poll_generator_endpoint_events();
        self.app.poll_plugin_runtime_events();
        self.app.poll_plugin_processing_health();
        self.app.poll_timeline_compile(&self.ctx);
        self.app.poll_audio_asset_events();
        self.app.poll_timeline_audio_runtime(&self.ctx);
        self.app.drive_clip_render_job();
    }

    fn settlement_control_until(&mut self, condition: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + SETTLEMENT_WAIT;
        while !condition(self) && Instant::now() < deadline {
            self.settlement_control_poll();
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(condition(self), "{}", self.timeout_metadata());
    }

    fn current_settlement(&self) -> BounceSourceSettlementObservation {
        let audio = self.app.audio.as_ref().unwrap();
        let (generation, epoch) = audio.confirmed_timeline_identity().unwrap();
        let channel = self.app.project.channels[0].id;
        let source = &self.app.running_generator_chains[&channel];
        source
            .control
            .observe_bounce_source_settlement(BounceSourceCallbackIdentity {
                engine_session: audio.engine_session(),
                project_session: self.app.project_session,
                timeline_generation: generation,
                epoch,
                transport_request: audio.transport_request_id(),
                channel_id: channel,
                endpoint_id: source.endpoint_id,
                instance_id: source.instance_id,
            })
    }

    fn settle_initial_source(&mut self) {
        // Existing actual callback service is allowed before the measured stop.
        self.until(|fixture| {
            fixture.current_settlement().settlement == BounceSourceSettlement::Ready
        });
        assert_eq!(self.source.capture_count(), 0);
    }

    fn begin_with_held_preparatory_reset(&mut self) -> BounceSourceCallbackIdentity {
        self.settle_initial_source();
        let resets = self.source.reset_count();
        self.source.pause_reset();
        self.begin();
        self.until(|fixture| {
            fixture.source.reset_count() > resets
                && fixture
                    .app
                    .clip_render_job
                    .as_ref()
                    .is_some_and(|job| job.stopped.settlement_target.is_some())
        });
        self.assert_no_stop_capture();
        self.app
            .clip_render_job
            .as_ref()
            .unwrap()
            .stopped
            .settlement_target
            .unwrap()
    }

    fn assert_no_stop_capture(&self) {
        let job = self.app.clip_render_job.as_ref().unwrap();
        assert_eq!(job.phase, ClipRenderPhase::StoppingDraining);
        assert!(
            job.stopped.capture.is_none(),
            "no App capture command/receipt before settlement"
        );
        assert!(job.plan.is_none(), "no plan before settlement");
        assert_eq!(self.source.capture_count(), 0, "no backend capture entry");
        let observed = self.current_settlement();
        assert!(
            !observed.capture_busy,
            "no source capture lease before settlement"
        );
        assert_eq!(observed.settlement, BounceSourceSettlement::Waiting);
        assert_eq!(
            self.app.running_generator_chains.len(),
            1,
            "exact live owner retained"
        );
    }
}

#[test]
fn actual_shared_stop_actor_waits_for_notification_after_callback_snapshot_advances() {
    let mut fixture = Fixture::new();
    fixture.settle_initial_source();
    let before = fixture.bytes();
    fixture.source.pause_capture();
    fixture.begin();
    fixture.app.drive_clip_render_job();
    let issued = fixture
        .app
        .clip_render_job
        .as_ref()
        .unwrap()
        .stopped
        .stop_preparation
        .unwrap();
    let deadline = Instant::now() + SETTLEMENT_WAIT;
    loop {
        fixture.app.poll_timeline_audio_runtime(&fixture.ctx);
        if fixture
            .app
            .clip_render_job
            .as_ref()
            .unwrap()
            .stopped
            .stop_preparation
            .unwrap()
            .activation
            .is_some()
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "actual activation did not queue; {}",
            fixture.timeout_metadata()
        );
        // The preceding StopAll/chase must really run on the callback before
        // the App can consume ChaseInstalled and queue activation. Stop polling
        // notifications as soon as that activation is queued below.
        fixture.pump_callback_only();
        std::thread::sleep(Duration::from_millis(1));
    }
    let activation = fixture
        .app
        .clip_render_job
        .as_ref()
        .unwrap()
        .stopped
        .stop_preparation
        .unwrap()
        .activation
        .unwrap();
    assert!(matches!(
        fixture.app.timeline_audio_sync.phase,
        TimelineAudioSyncPhase::AwaitingActivation {
            generation,
            seek_serial,
            frame,
            activation_request_id,
            ..
        } if generation == issued.generation && seek_serial == issued.seek_serial
            && frame == issued.frame && activation_request_id == activation.request_id
    ));
    let old_epoch = fixture
        .app
        .clip_render_job
        .as_ref()
        .unwrap()
        .stopped
        .old_epoch
        .unwrap();
    let applied_epoch = loop {
        assert!(
            Instant::now() < deadline,
            "actual callback did not apply activation; {}",
            fixture.timeout_metadata()
        );
        // StopAll/chase can themselves advance the snapshot epoch. Pump the
        // actual queued activation and require its callback-published active
        // identity, while leaving TransportActivationApplied unconsumed.
        fixture.pump_callback_only();
        if let Some((generation, epoch)) = fixture
            .app
            .audio
            .as_ref()
            .unwrap()
            .confirmed_timeline_identity()
            && generation == issued.generation
            && epoch != old_epoch
            && timeline_epoch_has_reached(epoch, activation.minimum_epoch)
        {
            break epoch;
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    let callback_snapshot = fixture.app.audio.as_ref().unwrap().snapshot();
    assert_eq!(callback_snapshot.transport_epoch, applied_epoch);
    assert_eq!(callback_snapshot.timeline_frame, issued.frame);
    assert!(!callback_snapshot.transport_playing);
    // The callback really advanced, but its queued event has not been consumed
    // by the production App notification path. A snapshot cannot fill the ACK.
    for _ in 0..3 {
        fixture.app.drive_clip_render_job();
    }
    let job = fixture.app.clip_render_job.as_ref().unwrap();
    let preparation = job.stopped.stop_preparation.unwrap();
    assert_eq!(
        (preparation.generation, preparation.seek_serial),
        (issued.generation, issued.seek_serial)
    );
    assert!(preparation.activation.unwrap().committed_epoch.is_none());
    assert!(matches!(
        fixture.app.timeline_audio_sync.phase,
        TimelineAudioSyncPhase::AwaitingActivation { activation_request_id, .. }
            if activation_request_id == activation.request_id
    ));
    assert!(job.stopped.binding.is_none());
    assert!(!job.stopped.owns_pending_capture());
    assert!(job.plan.is_none());
    assert_eq!(fixture.source.capture_count(), 0);
    assert_eq!(fixture.bytes(), before);
    fixture.until(|fixture| fixture.source.capture_count() == 1);
    let job = fixture.app.clip_render_job.as_ref().unwrap();
    assert_eq!(
        job.stopped
            .stop_preparation
            .unwrap()
            .activation
            .unwrap()
            .committed_epoch,
        Some(applied_epoch)
    );
    assert_eq!(
        job.stopped.binding.unwrap().sample_rate,
        job.stopped.initial.sample_rate
    );
    assert!(job.stopped.owns_pending_capture());
}

#[test]
fn actual_shared_capture_actor_keeps_missing_binding_owner_until_private_worker_ack() {
    let mut fixture = Fixture::new();
    let before = fixture.bytes();
    fixture.source.pause_capture();
    fixture.begin();
    fixture.until(|fixture| fixture.source.capture_count() == 1);
    fixture
        .app
        .clip_render_job
        .as_mut()
        .unwrap()
        .stopped
        .binding = None;
    fixture.app.drive_clip_render_job();
    let job = fixture.app.clip_render_job.as_ref().unwrap();
    assert_eq!(job.phase, ClipRenderPhase::Capturing);
    assert!(job.stopped.owns_pending_capture());
    assert!(job.stopped.capture.as_ref().unwrap().is_cancelling());
    assert!(
        !job.stopped
            .capture
            .as_ref()
            .unwrap()
            .termination_acknowledged()
    );
    assert!(fixture.app.clip_render_fence_active());
    assert_eq!(fixture.bytes(), before);
    fixture.source.release_gates();
    fixture.until(|fixture| {
        fixture
            .app
            .clip_render_job
            .as_ref()
            .unwrap()
            .phase
            .terminal()
    });
    let job = fixture.app.clip_render_job.as_ref().unwrap();
    assert_eq!(job.phase, ClipRenderPhase::Refused);
    assert!(!job.stopped.owns_pending_capture());
    assert!(matches!(
        job.failure_evidence.as_ref(),
        Some(JobFailureEvidence::Capture(CapturePollError::Invalid(reason)))
            if reason.contains("stopped source binding is missing")
    ));
    assert_eq!(fixture.source.capture_count(), 1);
    assert_eq!(fixture.bytes(), before);
    assert_eq!(std::fs::read_dir(&fixture.directory).unwrap().count(), 0);
}

#[test]
fn actual_shared_capture_actor_engine_loss_retains_receipt_and_exact_source_until_join() {
    let mut fixture = Fixture::new();
    let before = fixture.bytes();
    fixture.source.pause_capture();
    fixture.begin();
    fixture.until(|fixture| fixture.source.capture_count() == 1);
    let stamp = fixture
        .app
        .clip_render_job
        .as_ref()
        .unwrap()
        .stopped
        .binding
        .unwrap()
        .stamp;
    fixture.app.retain_clip_source_before_engine_loss();
    assert!(fixture.app.running_generator_chains.is_empty());
    fixture.callback = None;
    fixture.app.audio = None;
    for _ in 0..3 {
        fixture.app.drive_clip_render_job();
    }
    let job = fixture.app.clip_render_job.as_ref().unwrap();
    let retained = job.retired_source.as_ref().unwrap();
    assert_eq!(
        (
            retained.project_session,
            retained.endpoint_id,
            retained.instance_id
        ),
        (stamp.project_session, stamp.endpoint_id, stamp.instance_id)
    );
    assert_eq!(job.phase, ClipRenderPhase::Capturing);
    assert!(job.stopped.owns_pending_capture());
    assert!(job.stopped.capture.as_ref().unwrap().is_cancelling());
    assert!(
        !job.stopped
            .capture
            .as_ref()
            .unwrap()
            .termination_acknowledged()
    );
    assert!(!job.source_joined);
    assert!(job.owns_pending_resource());
    assert!(fixture.app.clip_render_fence_active());
    assert_eq!(fixture.bytes(), before);
    fixture.source.release_gates();
    fixture.settlement_control_until(|fixture| {
        let job = fixture.app.clip_render_job.as_ref().unwrap();
        job.phase.terminal() && job.source_joined && !job.owns_pending_resource()
    });
    let job = fixture.app.clip_render_job.as_ref().unwrap();
    assert_eq!(job.phase, ClipRenderPhase::Refused);
    assert!(job.retired_source.is_none());
    assert!(!job.stopped.owns_pending_capture());
    assert!(fixture.app.audio.is_none());
    assert_eq!(fixture.source.capture_count(), 1);
    assert_eq!(fixture.bytes(), before);
    assert_eq!(std::fs::read_dir(&fixture.directory).unwrap().count(), 0);
}

#[test]
fn actual_app_capture_waits_for_reset_and_input_terminal_without_another_callback() {
    let mut fixture = Fixture::new();
    // Drive the real mailbox independently of Timeline request allocation so
    // the positive case proves distinct ID domains with deliberately unequal IDs.
    for _ in 0..32 {
        fixture.app.audio.as_ref().unwrap().set_playing(false);
    }
    let before = fixture.bytes();
    let target = fixture.begin_with_held_preparatory_reset();
    let started_at = fixture
        .app
        .clip_render_job
        .as_ref()
        .unwrap()
        .stopped
        .started_at;
    let preparation = fixture
        .app
        .clip_render_job
        .as_ref()
        .unwrap()
        .stopped
        .stop_preparation
        .unwrap();
    let activation = preparation.activation.unwrap();
    assert_eq!(activation.committed_epoch, Some(target.epoch));
    assert!(timeline_epoch_has_reached(
        target.epoch,
        activation.minimum_epoch
    ));
    assert_eq!(target.transport_request, preparation.transport_request);
    assert_ne!(activation.request_id, 0);
    assert_ne!(
        activation.request_id, preparation.transport_request,
        "Timeline receipt ID is distinct from the legacy mailbox barrier"
    );
    let callback_before = fixture.callback().genuine_snapshot();
    let processes = fixture.source.process_count();
    fixture.source.pause_process();
    fixture.source.release_reset();
    fixture.settlement_control_until(|fixture| {
        fixture.source.process_count() > processes
            && fixture.current_settlement().coherent
            && fixture.current_settlement().callback_ready
    });
    fixture.assert_no_stop_capture();
    let waiting = fixture.current_settlement();
    assert_eq!(waiting.callback_identity, Some(target));
    assert_eq!(waiting.reset_completed_epoch, target.epoch);
    assert!(waiting.busy > 0);
    assert!(waiting.terminal < waiting.submitted);
    assert_eq!(waiting.consumed, waiting.submitted);
    assert_eq!(fixture.bytes(), before);
    fixture.source.pause_capture();
    fixture.source.release_process();
    fixture.settlement_control_until(|fixture| fixture.source.capture_count() == 1);
    let job = fixture.app.clip_render_job.as_ref().unwrap();
    assert_eq!(job.phase, ClipRenderPhase::Capturing);
    assert_eq!(
        job.stopped.started_at, started_at,
        "STOP_BUDGET origin is retained"
    );
    assert_eq!(job.stopped.settlement_target, Some(target));
    assert_eq!(job.stopped.stop_preparation, Some(preparation));
    let admitted = job.stopped.stop_settlement.unwrap();
    assert_eq!(admitted.settlement, BounceSourceSettlement::Ready);
    assert_eq!(admitted.terminal, admitted.submitted);
    assert_eq!(admitted.consumed, admitted.submitted);
    assert!(job.stopped.capture.is_some());
    assert!(job.plan.is_some());
    let callback_after = fixture.callback().genuine_snapshot();
    assert_eq!(callback_after.device_frame, callback_before.device_frame);
    assert_eq!(callback_after.pump_ordinal, callback_before.pump_ordinal);
    let retained = callback_after
        .endpoints
        .iter()
        .find(|endpoint| endpoint.endpoint_id == target.endpoint_id)
        .unwrap();
    assert!(
        retained.fixed.bridge.output_owned > 0,
        "ordinary safety output stays queued; no callback drain is authorized"
    );
    assert_eq!(fixture.bytes(), before);
    for _ in 0..4 {
        fixture.settlement_control_poll();
    }
    assert_eq!(
        fixture.source.capture_count(),
        1,
        "single admission, no retry"
    );
}

#[test]
fn actual_app_cancel_or_expiry_during_held_reset_never_admits_capture_after_release() {
    for expire in [false, true] {
        let mut fixture = Fixture::new();
        let before = fixture.bytes();
        fixture.begin_with_held_preparatory_reset();
        if expire {
            fixture
                .app
                .clip_render_job
                .as_mut()
                .unwrap()
                .stopped
                .started_at = Instant::now() - STOP_BUDGET;
        } else {
            assert!(fixture.app.cancel_clip_render_job());
        }
        fixture.settlement_control_poll();
        let expected = if expire {
            ClipRenderPhase::Refused
        } else {
            ClipRenderPhase::Cancelled
        };
        assert_eq!(
            fixture.app.clip_render_job.as_ref().unwrap().phase,
            expected
        );
        assert!(
            fixture
                .app
                .clip_render_job
                .as_ref()
                .unwrap()
                .stopped
                .capture
                .is_none()
        );
        fixture.source.release_reset();
        fixture.settlement_control_until(|fixture| {
            fixture.current_settlement().settlement == BounceSourceSettlement::Ready
        });
        assert_eq!(fixture.source.capture_count(), 0);
        assert!(!fixture.current_settlement().capture_busy);
        assert_eq!(fixture.bytes(), before);
        assert_eq!(std::fs::read_dir(&fixture.directory).unwrap().count(), 0);
    }
}

#[test]
fn actual_app_intervening_seek_cannot_become_the_first_preparatory_target() {
    let mut fixture = Fixture::new();
    fixture.settle_initial_source();
    let before = fixture.bytes();
    fixture.begin();
    fixture.app.drive_clip_render_job(); // Actual one preparatory issuance.
    let issued = fixture
        .app
        .clip_render_job
        .as_ref()
        .unwrap()
        .stopped
        .stop_preparation
        .unwrap();
    assert!(issued.activation.is_none());
    assert!(fixture.app.request_timeline_resume_chase()); // A different serial, same frame.
    assert_ne!(
        fixture.app.timeline_audio_sync.desired_seek.serial,
        issued.seek_serial
    );
    fixture.settlement_control_poll();
    let job = fixture.app.clip_render_job.as_ref().unwrap();
    assert_eq!(job.phase, ClipRenderPhase::Refused);
    assert!(job.stopped.settlement_target.is_none());
    assert!(job.stopped.capture.is_none());
    assert_eq!(fixture.source.capture_count(), 0);
    assert_eq!(fixture.bytes(), before);
}

#[test]
fn actual_app_frozen_stop_rejects_request_seek_generation_session_and_endpoint_supersession() {
    for mutation in 0..6 {
        let mut fixture = Fixture::new();
        let before = fixture.bytes();
        let target = fixture.begin_with_held_preparatory_reset();
        match mutation {
            0 => fixture.app.audio.as_ref().unwrap().set_playing(false),
            1 => assert!(fixture.app.request_timeline_resume_chase()),
            2 => {
                // Supersede App intent only; never fabricate callback readiness.
                fixture.app.timeline_desired_generation =
                    next_nonzero_app_id(fixture.app.timeline_desired_generation);
            }
            3 => {
                let session = fixture.app.project_session + 1;
                assert!(
                    fixture
                        .app
                        .audio
                        .as_mut()
                        .unwrap()
                        .set_project_session(session)
                );
            }
            4 => {
                assert!(
                    fixture
                        .app
                        .audio
                        .as_mut()
                        .unwrap()
                        .remove_generator_endpoint(target.channel_id)
                );
                fixture.pump_callback_only();
            }
            5 => {
                let audio = fixture.app.audio.as_ref().unwrap();
                let frame = audio.snapshot().timeline_frame;
                // Timeline ownership may ignore the legacy seek itself. This
                // case proves only supersession of the issued mailbox barrier.
                let request = audio.seek_transport_to_frame(0.0, frame);
                assert_ne!(request, target.transport_request);
                assert_eq!(audio.transport_request_id(), request);
                fixture.pump_callback_only();
            }
            _ => unreachable!(),
        }
        fixture.settlement_control_poll();
        let job = fixture.app.clip_render_job.as_ref().unwrap();
        assert_eq!(job.phase, ClipRenderPhase::Refused, "mutation {mutation}");
        assert_eq!(
            job.stopped.settlement_target,
            Some(target),
            "target is never rebound"
        );
        assert!(job.stopped.capture.is_none());
        assert_eq!(fixture.source.capture_count(), 0);
        assert_eq!(fixture.bytes(), before);
        fixture.source.release_gates();
    }
}

#[test]
fn actual_app_stopped_safety_process_fault_is_terminal_without_capture_retry() {
    let mut fixture = Fixture::new();
    let before = fixture.bytes();
    fixture.begin_with_held_preparatory_reset();
    let processes = fixture.source.process_count();
    fixture.source.pause_process();
    fixture.source.release_reset();
    fixture.settlement_control_until(|fixture| fixture.source.process_count() > processes);
    fixture.assert_no_stop_capture();
    fixture.source.fail_process_after_gate();
    fixture.source.release_process();
    fixture.settlement_control_until(|fixture| {
        fixture
            .app
            .clip_render_job
            .as_ref()
            .unwrap()
            .phase
            .terminal()
    });
    assert_eq!(
        fixture.app.clip_render_job.as_ref().unwrap().phase,
        ClipRenderPhase::Refused
    );
    assert!(
        fixture
            .app
            .clip_render_job
            .as_ref()
            .unwrap()
            .stopped
            .capture
            .is_none()
    );
    assert_eq!(fixture.source.capture_count(), 0);
    assert_eq!(fixture.bytes(), before);
}

#[test]
fn actual_app_legacy_request_supersession_before_chase_does_not_rebind() {
    let mut fixture = Fixture::new();
    fixture.settle_initial_source();
    let before = fixture.bytes();
    fixture.begin();
    let issued = fixture
        .app
        .clip_render_job
        .as_ref()
        .unwrap()
        .stopped
        .stop_transport_request
        .unwrap();
    fixture.app.audio.as_ref().unwrap().set_playing(false);
    assert_ne!(
        fixture.app.audio.as_ref().unwrap().transport_request_id(),
        issued
    );
    fixture.app.drive_clip_render_job();
    let job = fixture.app.clip_render_job.as_ref().unwrap();
    assert_eq!(job.phase, ClipRenderPhase::Refused);
    assert!(job.stopped.old_epoch.is_none());
    assert!(job.stopped.stop_preparation.is_none());
    assert!(job.stopped.capture.is_none());
    assert_eq!(fixture.source.capture_count(), 0);
    assert_eq!(fixture.bytes(), before);
}

#[test]
fn actual_app_reset_and_process_gate_unwind_releases_and_joins_exact_owner() {
    for hold_process in [false, true] {
        let mut retained_directory = None;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut fixture = Fixture::new();
            retained_directory = Some(fixture.directory.clone());
            fixture.begin_with_held_preparatory_reset();
            if hold_process {
                let processes = fixture.source.process_count();
                fixture.source.pause_process();
                fixture.source.release_reset();
                fixture
                    .settlement_control_until(|fixture| fixture.source.process_count() > processes);
            }
            fixture.assert_no_stop_capture();
            panic!("intentional reset/Process settlement fixture unwind");
        }));
        let panic = result.expect_err("the held-gate branch must reach its intentional unwind");
        let panic_text = panic
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| panic.downcast_ref::<String>().map(String::as_str));
        assert_eq!(
            panic_text,
            Some("intentional reset/Process settlement fixture unwind"),
            "a prerequisite assertion must not satisfy the unwind proof (hold_process={hold_process})"
        );
        assert!(
            !FIXTURE_POISONED.load(Ordering::Acquire),
            "gate release must precede the exact retained worker join"
        );
        let directory = retained_directory.unwrap();
        assert!(
            directory.exists(),
            "unwind retains diagnostic files after proven join"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
}
