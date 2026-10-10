//! Focused production admission regressions. After the unchanged initial-source
//! fixture settles, these tests use ordinary App/callback paths outside the
//! four-operation qualification ledger. A deliberate independent resync/fault
//! here never becomes a fifth permitted genuine topology transition.
use super::*;

fn current_fixture() -> TopologyFixture {
    let mut test = TopologyFixture::queued_initial();
    test.complete_initial();
    test.wait(None, |test| {
        test.exact_paused()
            && test
                .fixture
                .app
                .timeline_audio_source_has_current_authority()
    });
    test
}

fn service_existing_callback(test: &mut TopologyFixture) -> bool {
    let before = test.snapshot();
    if before.callback_work_pending()
        || before.request_id
            != test
                .fixture
                .app
                .audio
                .as_ref()
                .unwrap()
                .test_transport_request_id()
    {
        test.fixture.pump_callback_only();
        true
    } else {
        false
    }
}

fn ordinary_until(test: &mut TopologyFixture, condition: impl Fn(&TopologyFixture) -> bool) {
    let deadline = Instant::now() + MOCK_WAIT;
    while !condition(test) {
        assert!(
            Instant::now() < deadline,
            "bounded current-source condition failed: {}",
            test.fixture.timeout_metadata()
        );
        test.ordinary_poll();
        service_existing_callback(test);
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Admissions {
    app: observe::TimelineAppState,
    audio_commands: usize,
    timeline_commands: usize,
    activation_pending: bool,
    prepared_seek: Option<usize>,
    prepared_loop: Option<usize>,
    callback_pump: u64,
    topology_revision: u64,
    execution_failures: u64,
}
fn admissions(test: &TopologyFixture) -> Admissions {
    let snapshot = test.snapshot();
    let app = test.fixture.app();
    Admissions {
        app: app.genuine_timeline_app_state(),
        audio_commands: snapshot.pending_audio_commands,
        timeline_commands: snapshot.pending_timeline_commands,
        activation_pending: snapshot.pending_timeline_activation,
        prepared_seek: app
            .timeline_pending_chase
            .as_deref()
            .map(|chase| chase as *const _ as usize),
        prepared_loop: app
            .timeline_pending_loop_chase
            .as_deref()
            .map(|chase| chase as *const _ as usize),
        callback_pump: snapshot.pump_ordinal,
        topology_revision: snapshot.plugin_topology_revision,
        execution_failures: snapshot.timeline_execution_failures,
    }
}

fn current_source_tuple(test: &TopologyFixture) -> (u64, u64, u32) {
    let source = test.fixture.app.timeline_audio_source.as_ref().unwrap();
    (source.generation, source.fingerprint, source.sample_rate)
}

/// Two real callback route changes restore the same final endpoint/route and
/// require real resync without changing Project or compiled/source identity.
/// The two increments are observed, never assigned/reset. No worker is replaced
/// or detached. Both actual RouteSet receipts are subsequently consumed by App.
fn actual_same_project_resync(test: &mut TopologyFixture) -> (u64, u64) {
    assert!(
        test.fixture
            .app
            .timeline_audio_source_has_current_authority()
    );
    let before = test.snapshot();
    let project = project_fingerprint(&test.fixture.app.project);
    let source = current_source_tuple(test);
    let channel = test.fixture.app.project.channels[0].id;
    let running = &test.fixture.app.running_generator_chains[&channel];
    let identity = (
        running.endpoint_id,
        running.instance_id,
        running.control.genuine_worker_id(),
    );
    assert_eq!(running.guard.genuine_worker_id(), identity.2);
    assert_eq!(running.mixer_track, 1);
    assert!(
        test.fixture
            .app
            .audio
            .as_mut()
            .unwrap()
            .set_generator_route(channel, 2)
    );
    assert!(
        test.fixture
            .app
            .audio
            .as_mut()
            .unwrap()
            .set_generator_route(channel, 1)
    );
    assert!(service_existing_callback(test));
    let after = test.snapshot();
    assert_eq!(
        after.plugin_topology_revision,
        before.plugin_topology_revision + 2
    );
    assert_eq!(
        after.timeline_execution_failures,
        before.timeline_execution_failures + 2
    );
    assert!(after.plugin_topology_replan_pending && after.suspended);
    assert!(after.timeline_stats.unwrap().ownership_needs_resync);
    assert!(after.fault.is_none());
    assert_eq!(after.endpoints.len(), 1);
    assert_eq!(after.endpoints[0].endpoint_id, identity.0);
    assert_eq!(after.endpoints[0].fixed.bridge.worker_id, identity.2);
    assert_eq!(after.endpoints[0].mixer_track, 1);
    let receipts = test
        .fixture
        .callback_mut()
        .take_genuine_lifecycle_receipts();
    assert_eq!(receipts.len(), 2);
    for (receipt, route) in receipts.iter().zip([2, 1]) {
        assert!(receipt.producer_present && receipt.enqueue_success);
        assert!(matches!(receipt.event, GeneratorEndpointEvent::RouteSet {
            channel_id, endpoint_id: Some(endpoint), plugin_instance_id: Some(instance), mixer_track, success: true,
        } if channel_id == channel && endpoint == identity.0 && instance == identity.1 && mixer_track == route));
    }
    assert_eq!(project_fingerprint(&test.fixture.app.project), project);
    assert_eq!(current_source_tuple(test), source);
    assert!(
        test.fixture
            .app
            .timeline_audio_source_has_current_authority()
    );
    (
        after.plugin_topology_revision,
        after.timeline_execution_failures,
    )
}

fn consumed_timeline_events(test: &TopologyFixture, start: usize) -> Vec<TimelineRuntimeEvent> {
    test.records()[start..]
        .iter()
        .filter_map(|record| match record.kind {
            RecordKind::AppTimelineRuntimeEvent { event, .. } => Some(event),
            _ => None,
        })
        .collect()
}

#[test]
fn current_source_authority_checks_all_identity_domains_and_live_project() {
    let mut test = current_fixture();
    let app = test.fixture.app_mut();
    assert!(app.timeline_audio_source_has_current_authority());
    let generation = app.timeline_desired_generation;
    app.timeline_desired_generation = generation + 1;
    assert!(!app.timeline_audio_source_has_current_authority());
    app.timeline_desired_generation = generation;
    let sync_source = app.timeline_audio_sync.source_generation;
    app.timeline_audio_sync.source_generation = Some(generation + 1);
    assert!(!app.timeline_audio_source_has_current_authority());
    app.timeline_audio_sync.source_generation = sync_source;
    let compiled = app.timeline_compiled_generation;
    app.timeline_compiled_generation = Some(generation + 1);
    assert!(!app.timeline_audio_source_has_current_authority());
    app.timeline_compiled_generation = compiled;
    let fingerprint = app.timeline_compiled_fingerprint;
    app.timeline_compiled_fingerprint = fingerprint.map(|value| value.wrapping_add(1));
    assert!(!app.timeline_audio_source_has_current_authority());
    app.timeline_compiled_fingerprint = fingerprint;
    let observed = app.timeline_observed_fingerprint;
    app.timeline_observed_fingerprint = observed.wrapping_add(1);
    assert!(!app.timeline_audio_source_has_current_authority());
    app.timeline_observed_fingerprint = observed;
    let rate = app.timeline_audio_source.as_ref().unwrap().sample_rate;
    app.timeline_audio_source.as_mut().unwrap().sample_rate = rate + 1;
    assert!(!app.timeline_audio_source_has_current_authority());
    app.timeline_audio_source.as_mut().unwrap().sample_rate = rate;
    let pan = app.project.channels[0].pan;
    app.project.channels[0].pan = 0.25;
    assert!(!app.timeline_audio_source_has_current_authority());
    app.project.channels[0].pan = pan;
    let mode = app.transport_mode;
    app.transport_mode = TransportMode::Pattern;
    assert!(!app.timeline_audio_source_has_current_authority());
    app.transport_mode = mode;
    // Pending/deferred bookkeeping and compiler liveness alone do not revoke
    // a semantically exact retained source. No work is serviced in this matrix.
    let pending = app.timeline_pending_generation;
    app.timeline_pending_generation = Some(generation + 10);
    let available = app.timeline_worker_available;
    app.timeline_worker_available = false;
    let deferred = app.timeline_deferred_job.take();
    app.timeline_deferred_job = Some(TimelineCompileJob {
        generation: generation + 10,
        fingerprint: observed.wrapping_add(10),
        sample_rate: rate,
        project: app.project.clone(),
        options: transport_timeline_compile_options(&app.project, app.transport_mode),
    });
    assert!(app.timeline_audio_source_has_current_authority());
    app.timeline_deferred_job = deferred;
    app.timeline_worker_available = available;
    app.timeline_pending_generation = pending;
    assert!(app.timeline_audio_source_has_current_authority());
}

#[test]
fn current_source_debounce_edit_with_real_resync_does_not_issue_old_seek() {
    let mut test = current_fixture();
    let expected = actual_same_project_resync(&mut test);
    let generation = test.fixture.app.timeline_desired_generation;
    let observed = test.fixture.app.timeline_observed_fingerprint;
    test.fixture.app.project.channels[0].pan = 0.25;
    // No forced fingerprint refresh: both cached identities still describe A.
    assert_eq!(test.fixture.app.timeline_desired_generation, generation);
    assert_eq!(test.fixture.app.timeline_observed_fingerprint, observed);
    assert_ne!(
        test.fixture
            .app
            .clip_timeline_fingerprint(test.fixture.app.timeline_sample_rate()),
        observed
    );
    let before = admissions(&test);
    test.fixture.app.recover_timeline_runtime_if_needed();
    test.fixture.app.drive_timeline_audio_sync();
    assert_eq!(admissions(&test), before);
    assert_eq!(
        (
            test.snapshot().plugin_topology_revision,
            test.snapshot().timeline_execution_failures
        ),
        expected
    );
    let start = test.records().len();
    ordinary_until(&mut test, |test| {
        test.exact_paused()
            && test
                .fixture
                .app
                .timeline_audio_source_has_current_authority()
    });
    assert!(test.fixture.app.timeline_desired_generation > generation);
    assert!(
        consumed_timeline_events(&test, start)
            .iter()
            .all(|event| event.revision() != generation)
    );
    assert_eq!(
        (
            test.snapshot().plugin_topology_revision,
            test.snapshot().timeline_execution_failures
        ),
        expected
    );
}

#[test]
fn current_source_pending_desired_blocks_ready_dispatch_and_retains_prepared_resources() {
    let mut test = current_fixture();
    let (generation, _, _) = current_source_tuple(&test);
    let source = test
        .fixture
        .app
        .timeline_audio_source
        .as_ref()
        .unwrap()
        .clone();
    let epoch = test.snapshot().epoch + 1;
    test.fixture.app.timeline_pending_chase = Some(
        test.fixture
            .app
            .audio
            .as_ref()
            .unwrap()
            .prepare_timeline_chase(
                &source.timeline,
                generation,
                epoch,
                0,
                TimelineChaseOptions::default(),
            )
            .unwrap(),
    );
    test.fixture.app.timeline_pending_loop_chase = Some(
        test.fixture
            .app
            .audio
            .as_ref()
            .unwrap()
            .prepare_timeline_loop_chase(
                &source.timeline,
                generation,
                0,
                TimelineChaseOptions::default(),
            )
            .unwrap(),
    );
    test.fixture.app.project.channels[0].pan = 0.25;
    let seek = test.fixture.app.timeline_audio_sync.desired_seek.serial;
    let phases = [
        TimelineAudioSyncPhase::InstallReady,
        TimelineAudioSyncPhase::LoopReady,
        TimelineAudioSyncPhase::SeekReady,
        TimelineAudioSyncPhase::AwaitingActivation {
            generation,
            seek_serial: seek,
            epoch,
            frame: 0,
            activation_request_id: 0,
        },
    ];
    // Before refresh, actual prepared owners must remain exactly the same
    // objects: withholding admission must neither enqueue nor replace them.
    for phase in phases {
        test.fixture.app.timeline_audio_sync.phase = phase;
        let before = admissions(&test);
        assert!(before.prepared_seek.is_some() && before.prepared_loop.is_some());
        test.fixture.app.drive_timeline_audio_sync();
        if matches!(phase, TimelineAudioSyncPhase::AwaitingActivation { .. }) {
            test.fixture.app.drive_timeline_transport_activation();
        }
        assert_eq!(admissions(&test), before);
    }
    // Use the normal refresh/invalidation path once to establish a known newer
    // desired generation; the separate debounce test deliberately omits it.
    test.fixture
        .app
        .refresh_timeline_fingerprint(Instant::now(), true);
    assert!(test.fixture.app.timeline_desired_generation > generation);
    assert!(test.fixture.app.timeline_pending_chase.is_none());
    assert!(test.fixture.app.timeline_pending_loop_chase.is_none());
    for phase in phases {
        test.fixture.app.timeline_audio_sync.phase = phase;
        let before = admissions(&test);
        test.fixture.app.drive_timeline_audio_sync();
        if matches!(phase, TimelineAudioSyncPhase::AwaitingActivation { .. }) {
            test.fixture.app.drive_timeline_transport_activation();
        }
        assert_eq!(
            admissions(&test),
            before,
            "stale ready phase must publish no source-bearing work"
        );
    }
}

#[test]
fn current_source_real_chase_confirmation_cannot_activate_after_cached_project_edit() {
    let mut test = current_fixture();
    actual_same_project_resync(&mut test);
    test.ordinary_poll();
    let phase = test.fixture.app.timeline_audio_sync.phase;
    let TimelineAudioSyncPhase::SeekQueued {
        request_id,
        generation,
        ..
    } = phase
    else {
        panic!("real current-source recovery did not enqueue seek: {phase:?}");
    };
    assert!(service_existing_callback(&mut test));
    test.fixture.app.project.channels[0].pan = 0.25;
    let before = test.snapshot();
    let stale = test.fixture.app.timeline_audio_sync.stale_confirmations;
    let start = test.records().len();
    let ctx = test.fixture.ctx();
    // Actual runtime event consumption calls the original handler, whose direct
    // activation call must obey the same fresh source authority predicate.
    test.fixture.app.poll_timeline_audio_runtime(&ctx);
    assert!(
        consumed_timeline_events(&test, start)
            .iter()
            .any(|event| matches!(event,
        TimelineRuntimeEvent::ChaseInstalled { request_id: actual, revision, .. }
            if *actual == request_id && *revision == generation))
    );
    assert!(matches!(test.fixture.app.timeline_audio_sync.phase,
        TimelineAudioSyncPhase::AwaitingActivation { generation: actual, activation_request_id: 0, .. } if actual == generation));
    assert_eq!(
        test.fixture.app.timeline_audio_sync.stale_confirmations,
        stale
    );
    assert_eq!(
        test.snapshot().pending_timeline_commands,
        before.pending_timeline_commands
    );
    assert!(!test.snapshot().pending_timeline_activation);
    assert_eq!(test.snapshot().pump_ordinal, before.pump_ordinal);
}

#[test]
fn current_source_unchanged_real_resync_still_commits_same_generation_new_epoch() {
    let mut test = current_fixture();
    let source = current_source_tuple(&test);
    let old_epoch = test.snapshot().epoch;
    let expected = actual_same_project_resync(&mut test);
    let start = test.records().len();
    ordinary_until(&mut test, |test| {
        test.exact_paused() && test.snapshot().epoch > old_epoch
    });
    assert_eq!(current_source_tuple(&test), source);
    let routes: Vec<_> = test.records()[start..]
        .iter()
        .filter_map(|record| match record.kind {
            RecordKind::AppGeneratorEndpointEvent(GeneratorEndpointEvent::RouteSet {
                channel_id,
                endpoint_id,
                plugin_instance_id,
                mixer_track,
                success,
            }) => Some((
                channel_id,
                endpoint_id,
                plugin_instance_id,
                mixer_track,
                success,
            )),
            _ => None,
        })
        .collect();
    let channel = test.fixture.app.project.channels[0].id;
    let running = &test.fixture.app.running_generator_chains[&channel];
    assert_eq!(
        routes,
        [
            (
                channel,
                Some(running.endpoint_id),
                Some(running.instance_id),
                2,
                true
            ),
            (
                channel,
                Some(running.endpoint_id),
                Some(running.instance_id),
                1,
                true
            ),
        ]
    );
    assert_eq!(
        test.snapshot().endpoints[0].fixed.bridge.worker_id,
        running.guard.genuine_worker_id()
    );
    assert_eq!(test.snapshot().endpoints[0].mixer_track, 1);
    let events = consumed_timeline_events(&test, start);
    let (request, epoch) = events
        .iter()
        .find_map(|event| match event {
            TimelineRuntimeEvent::TransportActivationApplied {
                request_id,
                revision,
                epoch,
                frame: 0,
            } if *revision == source.0 => Some((*request_id, *epoch)),
            _ => None,
        })
        .expect("same current generation requires an actual activation receipt");
    assert_ne!(request, 0);
    assert!(epoch > old_epoch);
    assert_eq!(
        test.fixture
            .app
            .audio
            .as_ref()
            .unwrap()
            .confirmed_timeline_identity(),
        Some((source.0, epoch))
    );
    assert_eq!(
        (
            test.snapshot().plugin_topology_revision,
            test.snapshot().timeline_execution_failures
        ),
        expected
    );
    assert!(events.iter().all(|event| !matches!(
        event,
        TimelineRuntimeEvent::Rejected { .. }
            | TimelineRuntimeEvent::TransportActivationRejected { .. }
    )));
}

/// Delay only App receipt consumption. The value was produced by the actual
/// compiler; this helper never constructs a successful compile/Timeline receipt.
fn consume_actual_compile_result(test: &mut TopologyFixture, result: TimelineCompileWorkerResult) {
    let (sender, receiver) = mpsc::channel();
    assert!(sender.send(result).is_ok());
    let original = std::mem::replace(&mut test.fixture.app.timeline_compile_receiver, receiver);
    let ctx = test.fixture.ctx();
    test.fixture.app.poll_timeline_compile(&ctx);
    test.fixture.app.timeline_compile_receiver = original;
}

#[test]
fn current_source_edit_away_and_back_rejects_actual_superseded_compile() {
    let mut test = current_fixture();
    let old_source = current_source_tuple(&test);
    let pan = test.fixture.app.project.channels[0].pan;
    test.fixture.app.project.channels[0].pan = 0.25;
    test.fixture.app.request_immediate_timeline_compile();
    let away_generation = test.fixture.app.timeline_desired_generation;
    let ctx = test.fixture.ctx();
    test.fixture.app.poll_timeline_compile(&ctx);
    assert_eq!(
        test.fixture.app.timeline_pending_generation,
        Some(away_generation)
    );
    let away = test
        .fixture
        .app
        .timeline_compile_receiver
        .recv_timeout(MOCK_WAIT)
        .unwrap();
    assert_eq!(away.generation, away_generation);
    assert!(away.result.is_ok());
    // Cancel the edit while its real result is waiting to be consumed. Equal
    // content does not give generation A authority for this newer A generation.
    test.fixture.app.project.channels[0].pan = pan;
    test.fixture
        .app
        .refresh_timeline_fingerprint(Instant::now(), true);
    let back_generation = test.fixture.app.timeline_desired_generation;
    assert!(back_generation > away_generation);
    assert_eq!(test.fixture.app.timeline_observed_fingerprint, old_source.1);
    consume_actual_compile_result(&mut test, away);
    assert_eq!(current_source_tuple(&test), old_source);
    assert_eq!(
        test.fixture.app.timeline_compiled_generation,
        Some(old_source.0)
    );
    assert!(
        !test
            .fixture
            .app
            .timeline_audio_source_has_current_authority()
    );
    test.fixture.app.timeline_audio_sync.phase = TimelineAudioSyncPhase::InstallReady;
    let before = admissions(&test);
    test.fixture.app.drive_timeline_audio_sync();
    assert_eq!(admissions(&test), before);
    let start = test.records().len();
    ordinary_until(&mut test, |test| {
        test.exact_paused()
            && test
                .fixture
                .app
                .timeline_audio_source_has_current_authority()
    });
    assert_eq!(
        current_source_tuple(&test),
        (back_generation, old_source.1, old_source.2)
    );
    let events = consumed_timeline_events(&test, start);
    assert!(events.iter().any(|event| matches!(event, TimelineRuntimeEvent::TransportActivationApplied { revision, .. } if *revision == back_generation)));
    assert!(
        events
            .iter()
            .all(|event| event.revision() != away_generation && event.revision() != old_source.0)
    );
}

#[test]
fn current_source_stop_reaches_callback_while_replacement_compile_waits() {
    let mut test = current_fixture();
    test.fixture.app.toggle_play();
    ordinary_until(&mut test, |test| test.snapshot().playing);
    assert!(test.fixture.app.playing);
    test.fixture.app.project.channels[0].pan = 0.25;
    test.fixture.app.request_immediate_timeline_compile();
    let ctx = test.fixture.ctx();
    test.fixture.app.poll_timeline_compile(&ctx);
    let desired = test.fixture.app.timeline_desired_generation;
    assert_eq!(test.fixture.app.timeline_pending_generation, Some(desired));
    assert!(
        !test
            .fixture
            .app
            .timeline_audio_source_has_current_authority()
    );
    let before = test.snapshot();
    test.fixture.app.stop();
    assert!(!test.fixture.app.playing && !test.fixture.app.recording);
    assert_eq!(test.fixture.app.beat_position, 0.0);
    test.fixture.app.drive_timeline_audio_sync();
    assert_eq!(
        test.snapshot().pending_timeline_commands,
        before.pending_timeline_commands
    );
    // One existing changed Stop mailbox is the only callback permission here;
    // no extra stopped DSP pump is used to make the ownership guard pass.
    assert!(service_existing_callback(&mut test));
    let after = test.snapshot();
    assert!(!after.playing);
    assert_eq!(after.generation, before.generation);
    assert_eq!(
        after.plugin_topology_revision,
        before.plugin_topology_revision
    );
    assert_eq!(
        after.timeline_execution_failures,
        before.timeline_execution_failures
    );
    assert!(
        test.fixture
            .callback
            .as_ref()
            .unwrap()
            .last_output()
            .iter()
            .all(|frame| *frame == [0.0; 2])
    );
    assert_eq!(test.fixture.app.timeline_pending_generation, Some(desired));
}

#[test]
fn current_source_real_compile_error_and_disconnect_remain_visible() {
    let mut test = current_fixture();
    let app = test.fixture.app();
    // Exercise the real compiler's deterministic failure, then its ordinary App
    // result path. Only a failing compiler input is constrained by this test.
    let mut options = transport_timeline_compile_options(&app.project, app.transport_mode);
    options.max_work_units = 1;
    let failed = compile_timeline_job(TimelineCompileJob {
        generation: app.timeline_desired_generation,
        fingerprint: app.timeline_observed_fingerprint,
        sample_rate: app.timeline_sample_rate(),
        project: app.clip_timeline_playback_project().unwrap(),
        options,
    });
    let error = failed.result.as_ref().unwrap_err().clone();
    assert!(error.contains("deterministic 1-unit work budget"));
    consume_actual_compile_result(&mut test, failed);
    assert_eq!(
        test.fixture.app.timeline_compile_error.as_ref(),
        Some(&error)
    );
    assert!(test.fixture.app.timeline_compiled_generation.is_none());
    assert!(
        !test
            .fixture
            .app
            .timeline_audio_source_has_current_authority()
    );
    test.fixture.app.timeline_audio_sync.phase = TimelineAudioSyncPhase::SeekReady;
    let before = admissions(&test);
    test.fixture.app.drive_timeline_audio_sync();
    assert_eq!(admissions(&test), before);
    assert_eq!(
        test.fixture.app.timeline_compile_error.as_ref(),
        Some(&error)
    );
    // This is an actually disconnected channel, not an assigned liveness/error
    // flag. Keep the original worker receiver owned until App has observed it.
    let (sender, receiver) = mpsc::channel();
    drop(sender);
    let original = std::mem::replace(&mut test.fixture.app.timeline_compile_receiver, receiver);
    let ctx = test.fixture.ctx();
    test.fixture.app.poll_timeline_compile(&ctx);
    test.fixture.app.timeline_compile_receiver = original;
    assert!(!test.fixture.app.timeline_worker_available);
    assert_eq!(
        test.fixture.app.timeline_compile_error.as_deref(),
        Some("Timeline compiler worker stopped")
    );
    let before = admissions(&test);
    test.fixture.app.drive_timeline_audio_sync();
    assert_eq!(admissions(&test), before);
}

#[test]
fn current_source_independent_worker_fault_and_explicit_retry_keep_precedence() {
    let mut test = current_fixture();
    let channel = test.fixture.app.project.channels[0].id;
    let mut changed = source_state();
    changed.metadata.latency_samples = 1;
    test.fixture.source.set_capture_state(changed);
    let deadline = Instant::now() + MOCK_WAIT;
    while !test.fixture.app.running_generator_chains[&channel]
        .control
        .plugin_endpoint_snapshot()
        .is_some_and(|snapshot| snapshot.total_plugin_latency_samples() == 1)
    {
        assert!(
            Instant::now() < deadline,
            "actual worker did not publish timing change"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    test.fixture.app.project.channels[0].pan = 0.25;
    // An ordinary safety command gives the callback real work. The coherent
    // worker's timing mismatch, not a mutated fault flag, must fault processing.
    assert!(
        test.fixture
            .app
            .audio
            .as_mut()
            .unwrap()
            .command(AudioCommand::StopAll)
    );
    assert!(service_existing_callback(&mut test));
    let fault = test
        .snapshot()
        .fault
        .expect("actual changed worker timing must fault");
    test.fixture.app.poll_plugin_processing_health();
    assert!(test.fixture.app.plugin_processing_retry_required);
    assert_eq!(test.fixture.app.plugin_processing_last_fault, Some(fault));
    let before = admissions(&test);
    test.fixture.app.recover_timeline_runtime_if_needed();
    test.fixture.app.drive_timeline_audio_sync();
    assert_eq!(admissions(&test), before);
    assert_eq!(test.snapshot().fault, Some(fault));
    test.fixture.app.request_plugin_processing_replan(None);
    let pending = test
        .fixture
        .app
        .plugin_processing_retry_pending
        .expect("ordinary explicit retry must retain its real acknowledgment fence");
    assert!(
        !test
            .fixture
            .app
            .timeline_audio_source_has_current_authority()
    );
    let before = admissions(&test);
    test.fixture.app.recover_timeline_runtime_if_needed();
    test.fixture.app.drive_timeline_audio_sync();
    assert_eq!(admissions(&test), before);
    assert_eq!(
        test.fixture.app.plugin_processing_retry_pending,
        Some(pending)
    );
    assert_eq!(test.fixture.app.plugin_processing_last_fault, Some(fault));
    assert!(!test.fixture.app.playing);
}

#[test]
fn current_source_existing_missing_and_mismatched_diagnostics_and_fault_clear_survive() {
    let mut test = current_fixture();
    let generation = test
        .fixture
        .app
        .timeline_audio_source
        .as_ref()
        .unwrap()
        .generation;
    let seek_serial = test.fixture.app.timeline_audio_sync.desired_seek.serial;
    let epoch = test.snapshot().epoch;
    test.fixture.app.project.channels[0].pan = 0.25;
    test.fixture.app.timeline_audio_sync.phase = TimelineAudioSyncPhase::AwaitingActivation {
        generation: generation + 1,
        seek_serial,
        epoch,
        frame: 0,
        activation_request_id: 0,
    };
    let stale = test.fixture.app.timeline_audio_sync.stale_confirmations;
    test.fixture.app.drive_timeline_transport_activation();
    assert_eq!(
        test.fixture.app.timeline_audio_sync.stale_confirmations,
        stale + 1
    );
    let source = test.fixture.app.timeline_audio_source.take();
    assert!(
        !test
            .fixture
            .app
            .timeline_audio_source_has_current_authority()
    );
    test.fixture.app.timeline_audio_sync.phase = TimelineAudioSyncPhase::AwaitingActivation {
        generation,
        seek_serial,
        epoch,
        frame: 0,
        activation_request_id: 0,
    };
    test.fixture.app.drive_timeline_transport_activation();
    assert_eq!(
        test.fixture.app.timeline_audio_sync.last_error.as_deref(),
        Some("Seek chase was confirmed after its compiled source was released")
    );
    assert!(matches!(
        test.fixture.app.timeline_audio_sync.phase,
        TimelineAudioSyncPhase::FaultClearReady { .. }
    ));
    // Actual Clear admission and its matching callback receipt still proceed
    // with a missing source, through the unchanged runtime poll/event handler.
    let ctx = test.fixture.ctx();
    test.fixture.app.poll_timeline_audio_runtime(&ctx);
    let TimelineAudioSyncPhase::FaultClearQueued {
        revision,
        request_id,
    } = test.fixture.app.timeline_audio_sync.phase
    else {
        panic!("fault clear must retain exact queued ownership");
    };
    assert!(service_existing_callback(&mut test));
    let start = test.records().len();
    test.fixture.app.poll_timeline_audio_runtime(&ctx);
    assert!(consumed_timeline_events(&test, start).iter().any(|event| matches!(event,
        TimelineRuntimeEvent::Cleared { revision: actual, request_id: request } if *actual == revision && *request == request_id)));
    assert_eq!(
        test.fixture.app.timeline_audio_sync.phase,
        TimelineAudioSyncPhase::Faulted
    );
    assert!(
        !test
            .fixture
            .app
            .timeline_audio_sync
            .retain_callback_ownership
    );
    assert_eq!(
        test.fixture.app.timeline_audio_sync.last_error.as_deref(),
        Some("Seek chase was confirmed after its compiled source was released")
    );
    assert_eq!(
        test.fixture.app.timeline_audio_sync.stale_confirmations,
        stale + 1
    );
    test.fixture.app.timeline_audio_source = source;
}
