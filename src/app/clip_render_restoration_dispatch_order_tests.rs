//! Plugin-free proofs of restoration-owner successor admission. Callback events,
//! compilation and worker transfers are real. Temporary field perturbations in
//! the boundary matrices are negative inputs, never successful receipt evidence.
use super::restoration_route_pending_tests::{HeldPrepareOwner, held_restoration_factory};
use super::*;

#[derive(Debug, PartialEq, Eq)]
struct PendingOwnerState {
    channel: u32,
    started_at: Instant,
    chain_workers: Option<(usize, usize)>,
    audio_worker: Option<usize>,
    transferred_workers: (Option<usize>, Option<usize>),
    candidate: Option<usize>,
    ready: bool,
}
#[derive(Debug, PartialEq, Eq)]
struct BoundaryState {
    timeline: observe::TimelineAppState,
    error: Option<String>,
    obligation: bool,
    failed: bool,
    issued: Option<(u64, u64)>,
    audio_project: Option<u64>,
    retirement: bool,
    source: Option<(usize, u64, u64, u32)>,
    pending_seek: Option<usize>,
    pending_loop: Option<usize>,
    audio_commands: usize,
    timeline_commands: usize,
    activation_pending: bool,
    pump: u64,
    poll: usize,
    topology_revision: u64,
    execution_failures: u64,
    pending: Vec<PendingOwnerState>,
    running: Vec<(u32, u64, usize, usize)>,
    pending_generators: Vec<(u32, u64, usize, usize)>,
    installs: Vec<(u64, (u64, bool))>,
    retired: usize,
    assets: (usize, usize, usize, usize),
}
fn boundary(test: &TopologyFixture) -> BoundaryState {
    let app = test.fixture.app();
    let callback = test.snapshot();
    let mut pending = app
        .pending_clip_restorations
        .iter()
        .map(|(&channel, owner)| PendingOwnerState {
            channel,
            started_at: owner.genuine_diagnostic_started_at(),
            chain_workers: owner.genuine_diagnostic_chain_workers(),
            audio_worker: owner.genuine_test_chain_audio_worker(),
            transferred_workers: owner.genuine_diagnostic_transferred_workers(),
            candidate: owner
                .genuine_test_candidate()
                .map(|candidate| candidate as *const _ as usize),
            ready: owner.genuine_diagnostic_ready(),
        })
        .collect::<Vec<_>>();
    pending.sort_by_key(|item| item.channel);
    let mut running = app
        .running_generator_chains
        .iter()
        .map(|(&channel, owner)| {
            (
                channel,
                owner.endpoint_id,
                owner.control.genuine_worker_id(),
                owner.guard.genuine_worker_id(),
            )
        })
        .collect::<Vec<_>>();
    running.sort_unstable();
    let mut pending_generators = app
        .pending_generator_chains
        .iter()
        .map(|(&channel, owner)| {
            (
                channel,
                owner.endpoint_id,
                owner.control.genuine_worker_id(),
                owner.guard.genuine_worker_id(),
            )
        })
        .collect::<Vec<_>>();
    pending_generators.sort_unstable();
    let mut installs = app
        .clip_restoration_installs
        .iter()
        .map(|(&instance, &receipt)| (instance, receipt))
        .collect::<Vec<_>>();
    installs.sort_unstable();
    BoundaryState {
        timeline: app.genuine_timeline_app_state(),
        error: app.timeline_audio_sync.last_error.clone(),
        obligation: app.clip_restoration_playback_pending,
        failed: app.clip_restoration_failed,
        issued: app.clip_restoration_timeline,
        audio_project: app.audio.as_ref().map(AudioEngine::project_session),
        retirement: app.clip_session_retirement_pending(),
        source: app.timeline_audio_source.as_ref().map(|source| {
            (
                Arc::as_ptr(&source.timeline) as usize,
                source.generation,
                source.fingerprint,
                source.sample_rate,
            )
        }),
        pending_seek: app
            .timeline_pending_chase
            .as_deref()
            .map(|value| value as *const _ as usize),
        pending_loop: app
            .timeline_pending_loop_chase
            .as_deref()
            .map(|value| value as *const _ as usize),
        audio_commands: callback.pending_audio_commands,
        timeline_commands: callback.pending_timeline_commands,
        activation_pending: callback.pending_timeline_activation,
        pump: callback.pump_ordinal,
        poll: test.poll,
        topology_revision: callback.plugin_topology_revision,
        execution_failures: callback.timeline_execution_failures,
        pending,
        running,
        pending_generators,
        installs,
        retired: app.retired_clip_restorations.len(),
        assets: (
            app.prepared_audio_asset_registrations.len(),
            app.pending_audio_asset_registrations.len(),
            app.pending_audio_asset_unregistrations.len(),
            app.loaded_audio_assets.len(),
        ),
    }
}

fn service_existing_callback(test: &mut TopologyFixture) -> bool {
    let before = test.snapshot();
    // Identical service eligibility to TopologyFixture::checked_pump_inner.
    // These focused boundary cases are deliberately outside its four-op ledger.
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
            "restoration boundary wait: {}",
            test.fixture.timeout_metadata()
        );
        test.ordinary_poll();
        service_existing_callback(test);
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn current_fixture() -> TopologyFixture {
    let mut test = TopologyFixture::queued_initial();
    test.complete_initial();
    ordinary_until(&mut test, |test| {
        test.exact_paused()
            && test
                .fixture
                .app
                .timeline_audio_source_has_current_authority()
    });
    test
}
fn restored_fixture() -> TopologyFixture {
    let mut test = TopologyFixture::queued_initial();
    test.complete_initial();
    test.complete_conversion();
    test.complete_undo();
    assert!(test.exact_paused());
    assert!(test.fixture.app.clip_restoration_timeline.is_some());
    test
}
fn timeline_events(test: &TopologyFixture, start: usize) -> Vec<TimelineRuntimeEvent> {
    test.records()[start..]
        .iter()
        .filter_map(|record| match record.kind {
            RecordKind::AppTimelineRuntimeEvent { event, .. } => Some(event),
            _ => None,
        })
        .collect()
}
fn assert_clean_counters(test: &TopologyFixture) {
    assert_eq!(test.fixture.app.timeline_audio_sync.stale_confirmations, 0);
    assert_eq!(test.fixture.app.timeline_audio_sync.retry_count, 0);
    assert!(!test.fixture.app.plugin_processing_retry_required);
    assert!(test.fixture.app.plugin_processing_retry_pending.is_none());
    assert!(!test.fixture.app.clip_restoration_failed);
}
fn assert_predicate(test: &TopologyFixture, generation: u64, expected: bool) {
    let before = boundary(test);
    let history = serde_json::to_vec(&(
        &test.fixture.app.history_snapshot,
        &test.fixture.app.undo_stack,
        &test.fixture.app.redo_stack,
    ))
    .unwrap();
    for _ in 0..3 {
        assert_eq!(
            test.fixture
                .app
                .clip_restoration_allows_timeline_generation(generation),
            expected
        );
        assert_eq!(boundary(test), before);
        assert_eq!(
            serde_json::to_vec(&(
                &test.fixture.app.history_snapshot,
                &test.fixture.app.undo_stack,
                &test.fixture.app.redo_stack
            ))
            .unwrap(),
            history
        );
    }
}

struct HeldUndo {
    // Field drop order releases this gate before the retained App/worker drain,
    // including any assertion unwind after the factory has spawned its worker.
    gate: HeldPrepareOwner,
    test: TopologyFixture,
    generation: u64,
    converted_generation: u64,
    channel: u32,
    worker: usize,
    started_at: Instant,
}
fn held_undo() -> HeldUndo {
    let mut test = TopologyFixture::queued_initial();
    test.complete_initial();
    test.complete_conversion();
    let converted_generation = test.fixture.app.timeline_desired_generation;
    let gate = HeldPrepareOwner::reserve();
    test.fixture.app.clip_restoration_test_factory = Some(held_restoration_factory);
    test.observer.set_stage(observe::Stage::Undo).unwrap();
    test.fixture.app.undo();
    let generation = test.fixture.app.timeline_desired_generation;
    let channel = test.fixture.app.project.channels[0].id;
    let worker = latest_worker();
    let started_at =
        test.fixture.app.pending_clip_restorations[&channel].genuine_diagnostic_started_at();
    let deadline = Instant::now() + MOCK_WAIT;
    while !test
        .fixture
        .app
        .timeline_audio_source_has_current_authority()
    {
        assert!(
            Instant::now() < deadline,
            "real Undo compile did not finish under held preparation"
        );
        test.checked_poll().unwrap();
        test.checked_pump(None).unwrap();
        std::thread::sleep(Duration::from_millis(10));
    }
    let held = HeldUndo {
        gate,
        test,
        generation,
        converted_generation,
        channel,
        worker,
        started_at,
    };
    assert!(held.gate.is_held());
    assert_eq!(
        held.test.fixture.app.timeline_desired_generation,
        generation
    );
    assert!(generation > converted_generation);
    assert!(held.test.fixture.app.clip_restoration_timeline.is_none());
    assert!(held.test.fixture.app.clip_restoration_playback_pending);
    assert_eq!(
        held.test.fixture.app.timeline_audio_sync.phase,
        TimelineAudioSyncPhase::InstallReady
    );
    assert_eq!(
        held.test.fixture.app.pending_clip_restorations[&channel]
            .genuine_diagnostic_chain_workers(),
        Some((worker, worker))
    );
    assert_eq!(held.test.snapshot().pending_timeline_commands, 0);
    assert!(!held.test.snapshot().pending_timeline_activation);
    assert_clean_counters(&held.test);
    held
}
fn route_state(test: &TopologyFixture) -> RequiredGeneratorRouteState {
    test.fixture.app.timeline_generator_routes_state(
        &test
            .fixture
            .app
            .timeline_audio_source
            .as_ref()
            .unwrap()
            .timeline,
    )
}
fn assert_only_successor_events(test: &TopologyFixture, start: usize, generation: u64) {
    let events = timeline_events(test, start);
    for event in &events {
        assert_eq!(
            event.revision(),
            generation,
            "preissued or unrelated Timeline receipt: {event:?}"
        );
        assert!(!matches!(
            event,
            TimelineRuntimeEvent::Rejected { .. }
                | TimelineRuntimeEvent::TransportActivationRejected { .. }
        ));
    }
    assert!(events.iter().any(|event| matches!(event, TimelineRuntimeEvent::Installed { request_id, revision } if *request_id != 0 && *revision == generation)));
    assert!(events.iter().any(|event| matches!(event, TimelineRuntimeEvent::TransportActivationApplied { request_id, revision, .. } if *request_id != 0 && *revision == generation)));
}

#[test]
fn restoration_successor_admission_requires_exact_issued_identity_without_mutation() {
    {
        let held = held_undo();
        assert_predicate(&held.test, held.generation, false);
        assert_eq!(
            held.test.fixture.app.pending_clip_restorations[&held.channel]
                .genuine_diagnostic_started_at(),
            held.started_at
        );
    }
    let mut test = restored_fixture();
    let issued = test.fixture.app.clip_restoration_timeline.unwrap();
    let generation = issued.1;
    assert_predicate(&test, generation, true);
    // The tuple below was produced by the real owner, endpoint acknowledgment,
    // compiler and callback. Only negative boundary inputs are perturbed here.
    test.fixture.app.clip_restoration_playback_pending = true;
    assert_predicate(&test, generation, true);
    test.fixture.app.clip_restoration_timeline = None;
    assert_predicate(&test, generation, false);
    for bad in [
        (issued.0.wrapping_add(1), generation),
        (issued.0, generation - 1),
        (issued.0, generation + 1),
    ] {
        test.fixture.app.clip_restoration_timeline = Some(bad);
        assert_predicate(&test, generation, false);
    }
    test.fixture.app.clip_restoration_timeline = Some(issued);
    assert_predicate(&test, generation - 1, false);
    assert_predicate(&test, generation + 1, false);
    test.fixture.app.timeline_desired_generation = generation + 1;
    assert_predicate(&test, generation, false);
    test.fixture.app.timeline_desired_generation = generation;
    let project = test.fixture.app.project_session;
    assert!(
        test.fixture
            .app
            .audio
            .as_mut()
            .unwrap()
            .set_project_session(project + 1)
    );
    assert_predicate(&test, generation, false);
    assert!(
        test.fixture
            .app
            .audio
            .as_mut()
            .unwrap()
            .set_project_session(project)
    );
    test.fixture.app.clip_restoration_failed = true;
    assert_predicate(&test, generation, false);
    test.fixture.app.clip_restoration_playback_pending = false;
    assert_predicate(&test, generation, false);
    test.fixture.app.clip_restoration_failed = false;
    assert_predicate(&test, generation, true);
    test.fixture.app.clip_restoration_playback_pending = true;
    assert!(
        test.fixture
            .app
            .defer_clip_session_intent(ProjectIntent::New)
            .is_none()
    );
    assert!(test.fixture.app.clip_session_retirement_pending());
    assert_predicate(&test, generation, false);
    assert!(test.fixture.app.cancel_clip_session_retirement());
    test.fixture.app.clip_restoration_playback_pending = false;
    // Historical bindings/tuple do not fence an unrelated, genuinely recompiled
    // Project generation after the old restoration is settled.
    test.fixture.app.project.channels[0].pan = 0.25;
    ordinary_until(&mut test, |test| {
        test.exact_paused()
            && test.fixture.app.timeline_desired_generation > generation
            && test
                .fixture
                .app
                .timeline_audio_source_has_current_authority()
    });
    assert_eq!(test.fixture.app.clip_restoration_timeline, Some(issued));
    assert_predicate(&test, test.fixture.app.timeline_desired_generation, true);
    assert_clean_counters(&test);
}

#[test]
fn restoration_successor_bound_poll_order_withholds_preissuance_dispatch() {
    let mut held = held_undo();
    let start = held.test.records().len();
    held.gate.release();
    let claim = held.test.mutation_gap(OwnedTopologyKind::UndoInstall);
    assert_eq!(claim.worker, held.worker);
    let pump = held.test.snapshot().pump_ordinal;
    assert_eq!(
        held.test.fixture.app.clip_restoration_installs.get(&41),
        Some(&(claim.endpoint, false))
    );
    held.test.checked_poll().unwrap();
    assert_eq!(held.test.poll, claim.next_poll);
    assert_eq!(held.test.snapshot().pump_ordinal, pump);
    let app = held.test.fixture.app();
    assert!(app.clip_restoration_timeline.is_none());
    assert!(app.timeline_audio_source_has_current_authority());
    assert_eq!(app.timeline_desired_generation, held.generation);
    assert_eq!(
        app.timeline_audio_source.as_ref().unwrap().generation,
        held.generation
    );
    assert_eq!(
        app.timeline_audio_sync.phase,
        TimelineAudioSyncPhase::InstallReady
    );
    assert_eq!(route_state(&held.test), RequiredGeneratorRouteState::Ready);
    assert_eq!(
        app.clip_restoration_installs.get(&41),
        Some(&(claim.endpoint, true))
    );
    assert_eq!(
        app.running_generator_chains[&held.channel]
            .control
            .genuine_worker_id(),
        held.worker
    );
    assert_eq!(
        app.running_generator_chains[&held.channel]
            .guard
            .genuine_worker_id(),
        held.worker
    );
    assert!(app.pending_clip_restorations.is_empty() && app.pending_generator_chains.is_empty());
    assert_eq!(held.test.snapshot().pending_timeline_commands, 0);
    assert!(!held.test.snapshot().pending_timeline_activation);
    assert!(app.timeline_pending_chase.is_none() && app.timeline_pending_loop_chase.is_none());
    assert_eq!(
        held.test.snapshot().generation,
        Some(held.converted_generation)
    );
    assert!(timeline_events(&held.test, start).is_empty());
    assert_clean_counters(&held.test);
    // The next unchanged ordinary poll issues; no callback service or extra
    // restoration/endpoint poll is inserted into the post-receipt gap.
    held.test.checked_poll().unwrap();
    let issued = held.test.fixture.app.clip_restoration_timeline.unwrap();
    assert_eq!(held.test.poll, claim.next_poll + 1);
    assert!(issued.1 > held.generation);
    assert_eq!(held.test.snapshot().pump_ordinal, pump);
    assert!(held.test.fixture.app.clip_restoration_playback_pending);
    held.test
        .wait(Some(OwnedTopologyKind::UndoInstall), |test| {
            test.settled(OwnedTopologyKind::UndoInstall)
        });
    assert!(held.test.exact_paused());
    assert_eq!(held.test.fixture.app.timeline_desired_generation, issued.1);
    assert!(!held.test.fixture.app.clip_restoration_playback_pending);
    assert_only_successor_events(&held.test, start, issued.1);
    assert_clean_counters(&held.test);
}

fn gui_relevant_poll(test: &mut TopologyFixture) {
    test.poll += 1;
    let ctx = test.fixture.ctx();
    let app = test.fixture.app_mut();
    // Relevant real methods in their eframe::App::ui order. This device-free
    // fixture has no MIDI/save/device services. This is not a rendered GUI run.
    app.poll_plugin_processing_health();
    app.retry_deferred_plugin_rebuilds();
    app.poll_insert_endpoint_events();
    app.poll_generator_endpoint_events();
    app.poll_plugin_runtime_events();
    app.drive_clip_session_retirement();
    app.poll_clip_restorations();
    app.poll_plugin_parameter_callback_receipts();
    app.poll_plugin_parameter_worker_receipts();
    app.sync_generator_routes();
    app.refresh_transport_clock();
    app.poll_timeline_compile(&ctx);
    app.poll_audio_asset_events();
    app.poll_timeline_audio_runtime(&ctx);
    app.drive_clip_render_job();
}
#[test]
fn restoration_successor_gui_order_issues_before_dispatch() {
    let mut held = held_undo();
    let start = held.test.records().len();
    held.gate.release();
    let deadline = Instant::now() + MOCK_WAIT;
    while held.test.fixture.app.pending_generator_chains.is_empty() {
        assert!(
            Instant::now() < deadline,
            "GUI-order exact preparation never transferred"
        );
        gui_relevant_poll(&mut held.test);
        if held.test.fixture.app.pending_generator_chains.is_empty() {
            service_existing_callback(&mut held.test);
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    let endpoint = held.test.fixture.app.pending_generator_chains[&held.channel].endpoint_id;
    assert_eq!(
        held.test.fixture.app.pending_generator_chains[&held.channel]
            .control
            .genuine_worker_id(),
        held.worker
    );
    assert_eq!(
        held.test.fixture.app.pending_generator_chains[&held.channel]
            .guard
            .genuine_worker_id(),
        held.worker
    );
    assert!(held.test.fixture.app.pending_clip_restorations.is_empty());
    assert!(held.test.fixture.app.clip_restoration_timeline.is_none());
    assert_eq!(
        held.test.fixture.app.clip_restoration_installs.get(&41),
        Some(&(endpoint, false))
    );
    assert!(service_existing_callback(&mut held.test));
    let receipts = held
        .test
        .fixture
        .callback_mut()
        .take_genuine_lifecycle_receipts();
    assert_eq!(receipts.len(), 1);
    assert!(receipts[0].producer_present && receipts[0].enqueue_success);
    assert!(
        matches!(receipts[0].event, GeneratorEndpointEvent::Installed { channel_id, endpoint_id, plugin_instance_id: 41, mixer_track: 1, replaced_endpoint_id: None, replaced_plugin_instance_id: None, success: true } if channel_id == held.channel && endpoint_id == endpoint)
    );
    let before = held.test.snapshot();
    let receipt_poll = held.test.poll + 1;
    gui_relevant_poll(&mut held.test);
    let issued = held.test.fixture.app.clip_restoration_timeline.unwrap();
    assert_eq!(held.test.poll, receipt_poll);
    assert_eq!(held.test.snapshot().pump_ordinal, before.pump_ordinal);
    assert!(issued.1 > held.generation);
    assert_eq!(issued.0, before.engine_session);
    assert_eq!(
        held.test.fixture.app.clip_restoration_installs.get(&41),
        Some(&(endpoint, true))
    );
    assert_eq!(
        held.test.fixture.app.running_generator_chains[&held.channel]
            .control
            .genuine_worker_id(),
        held.worker
    );
    assert_eq!(
        held.test.fixture.app.running_generator_chains[&held.channel]
            .guard
            .genuine_worker_id(),
        held.worker
    );
    assert_eq!(
        held.test.snapshot().generation,
        Some(held.converted_generation)
    );
    assert!(held.test.fixture.app.clip_restoration_playback_pending);
    let deadline = Instant::now() + MOCK_WAIT;
    while !held.test.exact_paused() || held.test.fixture.app.clip_restoration_playback_pending {
        assert!(
            Instant::now() < deadline,
            "GUI-order successor did not settle paused"
        );
        gui_relevant_poll(&mut held.test);
        service_existing_callback(&mut held.test);
        assert_ne!(held.test.snapshot().generation, Some(held.generation));
        assert_clean_counters(&held.test);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(held.test.fixture.app.timeline_desired_generation, issued.1);
    assert_only_successor_events(&held.test, start, issued.1);
    let consumed = held.test.records()[start..].iter().filter(|record| matches!(record.kind, RecordKind::AppGeneratorEndpointEvent(GeneratorEndpointEvent::Installed { endpoint_id, plugin_instance_id: 41, success: true, .. }) if endpoint_id == endpoint)).count();
    assert_eq!(consumed, 1);
    assert_clean_counters(&held.test);
}

fn drive_ready_boundary(test: &mut TopologyFixture, phase: TimelineAudioSyncPhase, direct: bool) {
    test.fixture.app.timeline_audio_sync.phase = phase;
    if direct {
        match phase {
            TimelineAudioSyncPhase::LoopReady => test.fixture.app.drive_timeline_loop_chase(),
            TimelineAudioSyncPhase::SeekReady => test.fixture.app.drive_timeline_seek_chase(),
            _ => panic!("only loop/seek have direct helper boundaries"),
        }
    } else {
        test.fixture.app.drive_timeline_audio_sync();
    }
}
const READY_BOUNDARIES: [(TimelineAudioSyncPhase, bool); 5] = [
    (TimelineAudioSyncPhase::InstallReady, false),
    (TimelineAudioSyncPhase::LoopReady, false),
    (TimelineAudioSyncPhase::SeekReady, false),
    (TimelineAudioSyncPhase::LoopReady, true),
    (TimelineAudioSyncPhase::SeekReady, true),
];
fn prepare_retained_chases(test: &mut TopologyFixture) {
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
                source.generation,
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
                source.generation,
                0,
                TimelineChaseOptions::default(),
            )
            .unwrap(),
    );
}
#[test]
fn restoration_successor_ready_dispatch_checks_routes_before_handoff() {
    {
        let mut held = held_undo();
        for (phase, direct) in READY_BOUNDARIES {
            held.test.fixture.app.timeline_audio_sync.phase = phase;
            held.test.fixture.app.timeline_audio_sync.last_error = None;
            let mut before = boundary(&held.test);
            drive_ready_boundary(&mut held.test, phase, direct);
            let after = boundary(&held.test);
            assert!(
                matches!(route_state(&held.test), RequiredGeneratorRouteState::Pending { channel_id, plugin_instance_id: 41 } if channel_id == held.channel)
            );
            assert!(after.error.as_deref().is_some_and(|error| error.contains("Waiting for callback-confirmed generator")));
            // Route-first validation may publish its existing wait message only.
            before.error = after.error.clone();
            assert_eq!(after, before);
            assert!(held.gate.is_held());
        }
        // Invalid retained owners must still reach the existing route fault;
        // they must not disappear behind the successor-admission fence.
        for invalid in 0..2 {
            let project = held.test.fixture.app.project_session;
            if invalid == 0 {
                held.test.fixture.app.clip_restoration_failed = true;
            } else {
                assert!(
                    held.test
                        .fixture
                        .app
                        .audio
                        .as_mut()
                        .unwrap()
                        .set_project_session(project + 1)
                );
            }
            for (phase, direct) in READY_BOUNDARIES {
                held.test.fixture.app.timeline_audio_sync.phase = phase;
                let before = boundary(&held.test);
                drive_ready_boundary(&mut held.test, phase, direct);
                let after = boundary(&held.test);
                assert!(matches!(
                    route_state(&held.test),
                    RequiredGeneratorRouteState::Failed(_)
                ));
                assert!(matches!(
                    held.test.fixture.app.timeline_audio_sync.phase,
                    TimelineAudioSyncPhase::FaultClearReady { .. }
                        | TimelineAudioSyncPhase::Faulted
                ));
                assert!(after.error.as_deref().is_some_and(|error| {
                    error.contains("Compiled generator route is not callback-ready")
                }));
                assert_eq!(
                    (
                        after.audio_commands,
                        after.timeline_commands,
                        after.activation_pending,
                        after.pump
                    ),
                    (
                        before.audio_commands,
                        before.timeline_commands,
                        before.activation_pending,
                        before.pump
                    )
                );
                assert_eq!(after.pending, before.pending);
                assert_eq!(
                    after.timeline.stale_confirmations,
                    before.timeline.stale_confirmations
                );
                assert_eq!(after.timeline.retry_count, before.timeline.retry_count);
            }
            held.test.fixture.app.clip_restoration_failed = false;
            assert!(
                held.test
                    .fixture
                    .app
                    .audio
                    .as_mut()
                    .unwrap()
                    .set_project_session(project)
            );
        }
    }
    let mut held = held_undo();
    held.gate.release();
    let claim = held.test.mutation_gap(OwnedTopologyKind::UndoInstall);
    held.test.checked_poll().unwrap();
    assert_eq!(held.test.poll, claim.next_poll);
    assert_eq!(route_state(&held.test), RequiredGeneratorRouteState::Ready);
    assert!(held.test.fixture.app.clip_restoration_timeline.is_none());
    let start = held.test.records().len();
    for retained in [false, true] {
        if retained {
            prepare_retained_chases(&mut held.test);
        }
        for (phase, direct) in READY_BOUNDARIES {
            held.test.fixture.app.timeline_audio_sync.phase = phase;
            let before = boundary(&held.test);
            assert_eq!(before.pending_seek.is_some(), retained);
            assert_eq!(before.pending_loop.is_some(), retained);
            assert!(
                held.test
                    .fixture
                    .app
                    .timeline_audio_source_has_current_authority()
            );
            drive_ready_boundary(&mut held.test, phase, direct);
            assert_eq!(boundary(&held.test), before);
        }
    }
    // Restore only the matrix's initial phase. Actual owner issuance invalidates
    // these exact prepared resources and the real compiler supplies its source.
    held.test.fixture.app.timeline_audio_sync.phase = TimelineAudioSyncPhase::InstallReady;
    held.test.checked_poll().unwrap();
    let issued = held.test.fixture.app.clip_restoration_timeline.unwrap();
    assert!(issued.1 > held.generation);
    assert!(held.test.fixture.app.timeline_pending_chase.is_none());
    assert!(held.test.fixture.app.timeline_pending_loop_chase.is_none());
    held.test
        .wait(Some(OwnedTopologyKind::UndoInstall), |test| {
            test.settled(OwnedTopologyKind::UndoInstall)
        });
    assert_only_successor_events(&held.test, start, issued.1);
    assert_clean_counters(&held.test);
}

fn actual_same_project_resync(test: &mut TopologyFixture) {
    assert!(
        test.fixture
            .app
            .timeline_audio_source_has_current_authority()
    );
    let before = test.snapshot();
    let source = boundary(test).source;
    let project = project_fingerprint(&test.fixture.app.project);
    let channel = test.fixture.app.project.channels[0].id;
    let owner = &test.fixture.app.running_generator_chains[&channel];
    let identity = (
        owner.endpoint_id,
        owner.instance_id,
        owner.control.genuine_worker_id(),
    );
    assert_eq!(owner.guard.genuine_worker_id(), identity.2);
    assert_eq!(owner.mixer_track, 1);
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
    assert_eq!(after.endpoints[0].endpoint_id, identity.0);
    assert_eq!(after.endpoints[0].fixed.bridge.worker_id, identity.2);
    let receipts = test
        .fixture
        .callback_mut()
        .take_genuine_lifecycle_receipts();
    assert_eq!(receipts.len(), 2);
    for (receipt, route) in receipts.iter().zip([2, 1]) {
        assert!(receipt.producer_present && receipt.enqueue_success);
        assert!(
            matches!(receipt.event, GeneratorEndpointEvent::RouteSet { channel_id, endpoint_id: Some(endpoint), plugin_instance_id: Some(instance), mixer_track, success: true } if channel_id == channel && endpoint == identity.0 && instance == identity.1 && mixer_track == route)
        );
    }
    assert_eq!(boundary(test).source, source);
    assert_eq!(project_fingerprint(&test.fixture.app.project), project);
    assert!(
        test.fixture
            .app
            .timeline_audio_source_has_current_authority()
    );
}
fn real_seek_receipt_pending(test: &mut TopologyFixture) -> (u64, u64) {
    actual_same_project_resync(test);
    test.ordinary_poll();
    let TimelineAudioSyncPhase::SeekQueued {
        request_id,
        generation,
        ..
    } = test.fixture.app.timeline_audio_sync.phase
    else {
        panic!("ordinary real resync did not queue its seek");
    };
    assert!(service_existing_callback(test));
    (request_id, generation)
}
#[test]
fn restoration_successor_direct_activation_preserves_identity_errors_and_receipts() {
    {
        let mut test = restored_fixture();
        let (request, generation) = real_seek_receipt_pending(&mut test);
        let issued = test.fixture.app.clip_restoration_timeline;
        // Isolate the admission fence while preserving an actually queued chase
        // and its receipt identity. This is not a fabricated restoration event.
        test.fixture.app.clip_restoration_playback_pending = true;
        test.fixture.app.clip_restoration_timeline = None;
        let start = test.records().len();
        let before = boundary(&test);
        let ctx = test.fixture.ctx();
        test.fixture.app.poll_timeline_audio_runtime(&ctx);
        assert!(timeline_events(&test, start).iter().any(|event| matches!(event, TimelineRuntimeEvent::ChaseInstalled { request_id, revision, .. } if *request_id == request && *revision == generation)));
        assert!(
            matches!(test.fixture.app.timeline_audio_sync.phase, TimelineAudioSyncPhase::AwaitingActivation { generation: actual, activation_request_id: 0, .. } if actual == generation)
        );
        assert_eq!(
            test.snapshot().pending_timeline_commands,
            before.timeline_commands
        );
        assert!(!test.snapshot().pending_timeline_activation);
        assert_eq!(test.snapshot().pump_ordinal, before.pump);
        assert_eq!(
            test.fixture.app.timeline_audio_sync.stale_confirmations,
            before.timeline.stale_confirmations
        );
        let awaiting = boundary(&test);
        for _ in 0..3 {
            test.fixture.app.drive_timeline_transport_activation();
            test.fixture.app.drive_timeline_audio_sync();
            assert_eq!(boundary(&test), awaiting);
        }
        // Source-generation mismatch keeps its existing stale-confirmation path,
        // even while no successor is issued. It must not be hidden by the fence.
        test.fixture
            .app
            .timeline_audio_source
            .as_mut()
            .unwrap()
            .generation = generation + 1;
        test.fixture.app.drive_timeline_transport_activation();
        assert_eq!(
            test.fixture.app.timeline_audio_sync.stale_confirmations,
            before.timeline.stale_confirmations + 1
        );
        assert!(!test.snapshot().pending_timeline_activation);
        test.fixture
            .app
            .timeline_audio_source
            .as_mut()
            .unwrap()
            .generation = generation;
        // Missing compiled source keeps the original fault handoff. This Arc
        // source carries no worker ownership; all worker owners stay in Fixture.
        let source = test.fixture.app.timeline_audio_source.take();
        test.fixture.app.drive_timeline_transport_activation();
        test.fixture.app.timeline_audio_source = source;
        assert!(
            test.fixture
                .app
                .timeline_audio_sync
                .last_error
                .as_deref()
                .is_some_and(|error| error.contains("compiled source was released"))
        );
        assert!(matches!(
            test.fixture.app.timeline_audio_sync.phase,
            TimelineAudioSyncPhase::FaultClearReady { .. } | TimelineAudioSyncPhase::Faulted
        ));
        assert!(!test.snapshot().pending_timeline_activation);
        assert_eq!(
            test.fixture.app.timeline_audio_sync.retry_count,
            before.timeline.retry_count
        );
        test.fixture.app.clip_restoration_timeline = issued;
    }
    {
        let mut test = restored_fixture();
        let (_, generation) = real_seek_receipt_pending(&mut test);
        let ctx = test.fixture.ctx();
        test.fixture.app.poll_timeline_audio_runtime(&ctx);
        let TimelineAudioSyncPhase::AwaitingActivation {
            activation_request_id,
            epoch,
            frame,
            ..
        } = test.fixture.app.timeline_audio_sync.phase
        else {
            panic!("actual chase receipt did not queue activation");
        };
        assert_ne!(activation_request_id, 0);
        assert_eq!(test.snapshot().pending_timeline_commands, 1);
        // New admission is blocked; this already queued exact receipt remains
        // valid because no phase/source/session identity is changed. A real new
        // Undo may invalidate it and is not claimed to preserve that validity.
        test.fixture.app.clip_restoration_playback_pending = true;
        test.fixture.app.clip_restoration_timeline = None;
        assert!(service_existing_callback(&mut test));
        let start = test.records().len();
        let before = boundary(&test);
        test.fixture.app.poll_timeline_audio_runtime(&ctx);
        let actual_epoch = timeline_events(&test, start)
            .iter()
            .find_map(|event| match event {
                TimelineRuntimeEvent::TransportActivationApplied {
                    request_id,
                    revision,
                    epoch: actual_epoch,
                    frame: actual_frame,
                } if *request_id == activation_request_id
                    && *revision == generation
                    && *actual_frame == frame =>
                {
                    Some(*actual_epoch)
                }
                _ => None,
            })
            .expect("exact actual queued activation receipt was consumed");
        assert!(timeline_epoch_has_reached(actual_epoch, epoch));
        assert!(
            matches!(test.fixture.app.timeline_audio_sync.phase, TimelineAudioSyncPhase::Active { generation: actual, epoch: committed_epoch } if actual == generation && committed_epoch == actual_epoch)
        );
        assert_eq!(
            test.fixture
                .app
                .audio
                .as_ref()
                .unwrap()
                .confirmed_timeline_identity(),
            Some((generation, actual_epoch))
        );
        assert_eq!(
            test.fixture.app.timeline_audio_sync.stale_confirmations,
            before.timeline.stale_confirmations
        );
        assert!(test.fixture.app.clip_restoration_playback_pending);
        assert!(test.fixture.app.clip_restoration_timeline.is_none());
        assert_eq!(test.snapshot().pump_ordinal, before.pump);
        assert_clean_counters(&test);
    }
}

#[test]
fn restoration_successor_recovery_waits_without_changing_current_authority() {
    let mut test = restored_fixture();
    let issued = test.fixture.app.clip_restoration_timeline.unwrap();
    actual_same_project_resync(&mut test);
    // Temporary boundary inputs on this exact restored owner. The saved tuple
    // comes solely from the actual issuer, not an adopted compiler generation.
    test.fixture.app.clip_restoration_playback_pending = true;
    test.fixture.app.clip_restoration_timeline = None;
    let ctx = test.fixture.ctx();
    let before = boundary(&test);
    assert!(
        test.fixture
            .app
            .timeline_audio_source_has_current_authority()
    );
    for _ in 0..3 {
        test.fixture.app.recover_timeline_runtime_if_needed();
        assert_eq!(boundary(&test), before);
        test.fixture.app.poll_timeline_audio_runtime(&ctx);
        assert_eq!(boundary(&test), before);
        assert!(
            test.fixture
                .app
                .timeline_audio_source_has_current_authority()
        );
    }
    test.fixture.app.clip_restoration_timeline = Some(issued);
    assert_predicate(&test, issued.1, true);
    // Issuance is necessary but cannot replace any existing current-source check.
    for mismatch in 0..4 {
        let generation = test.fixture.app.timeline_desired_generation;
        let fingerprint = test.fixture.app.timeline_compiled_fingerprint;
        let source_generation = test
            .fixture
            .app
            .timeline_audio_source
            .as_ref()
            .unwrap()
            .generation;
        let pan = test.fixture.app.project.channels[0].pan;
        match mismatch {
            0 => test.fixture.app.timeline_desired_generation = generation + 1,
            1 => {
                test.fixture.app.timeline_compiled_fingerprint =
                    fingerprint.map(|value| value.wrapping_add(1))
            }
            2 => {
                test.fixture
                    .app
                    .timeline_audio_source
                    .as_mut()
                    .unwrap()
                    .generation = source_generation + 1
            }
            _ => test.fixture.app.project.channels[0].pan = 0.25,
        }
        assert!(
            !test
                .fixture
                .app
                .timeline_audio_source_has_current_authority()
        );
        let stale = boundary(&test);
        test.fixture.app.recover_timeline_runtime_if_needed();
        test.fixture.app.poll_timeline_audio_runtime(&ctx);
        assert_eq!(boundary(&test), stale);
        test.fixture.app.timeline_desired_generation = generation;
        test.fixture.app.timeline_compiled_fingerprint = fingerprint;
        test.fixture
            .app
            .timeline_audio_source
            .as_mut()
            .unwrap()
            .generation = source_generation;
        test.fixture.app.project.channels[0].pan = pan;
    }
    let old_seek = test.fixture.app.timeline_audio_sync.desired_seek.serial;
    let old_epoch = test.snapshot().epoch;
    assert!(
        test.fixture
            .app
            .timeline_audio_source_has_current_authority()
    );
    test.fixture.app.recover_timeline_runtime_if_needed();
    assert!(test.fixture.app.timeline_audio_sync.desired_seek.serial > old_seek);
    assert_eq!(
        test.fixture.app.timeline_audio_sync.phase,
        TimelineAudioSyncPhase::SeekReady
    );
    let start = test.records().len();
    ordinary_until(&mut test, |test| {
        test.exact_paused()
            && test.snapshot().epoch > old_epoch
            && !test.fixture.app.clip_restoration_playback_pending
    });
    assert_eq!(test.fixture.app.clip_restoration_timeline, Some(issued));
    assert_eq!(test.fixture.app.timeline_desired_generation, issued.1);
    assert!(timeline_events(&test, start).iter().any(|event| matches!(event, TimelineRuntimeEvent::TransportActivationApplied { revision, epoch, .. } if *revision == issued.1 && *epoch > old_epoch)));
    assert_clean_counters(&test);
}

#[test]
fn restoration_successor_gate_preserves_event_drain_stop_fault_clear_and_cleanup() {
    {
        let mut test = current_fixture();
        test.fixture.app.toggle_play();
        ordinary_until(&mut test, |test| {
            test.snapshot().playing
                && matches!(
                    test.fixture.app.timeline_audio_sync.phase,
                    TimelineAudioSyncPhase::Active { .. }
                )
        });
        test.fixture.app.clip_restoration_playback_pending = true;
        test.fixture.app.clip_restoration_timeline = None;
        let before_stop = boundary(&test);
        test.fixture.app.stop();
        assert!(!test.fixture.app.playing);
        assert!(
            test.fixture
                .app
                .audio
                .as_ref()
                .unwrap()
                .test_transport_request_id()
                > test.snapshot().request_id
        );
        assert!(test.fixture.app.clip_restoration_playback_pending);
        assert!(test.fixture.app.clip_restoration_timeline.is_none());
        assert_eq!(test.snapshot().pump_ordinal, before_stop.pump);
        assert_eq!(boundary(&test).running, before_stop.running);
        assert!(service_existing_callback(&mut test));
        assert!(!test.snapshot().playing);
        test.fixture
            .app
            .fault_timeline_audio("focused restoration-fence clear proof".into());
        test.fixture.app.drive_timeline_fault_clear();
        let TimelineAudioSyncPhase::FaultClearQueued {
            request_id,
            revision,
        } = test.fixture.app.timeline_audio_sync.phase
        else {
            panic!("active restoration fence hid ordinary fault-clear dispatch");
        };
        assert!(service_existing_callback(&mut test));
        let start = test.records().len();
        let ctx = test.fixture.ctx();
        let stale = test.fixture.app.timeline_audio_sync.stale_confirmations;
        test.fixture.app.poll_timeline_audio_runtime(&ctx);
        assert!(timeline_events(&test, start).iter().any(|event| matches!(event, TimelineRuntimeEvent::Cleared { request_id: actual, revision: generation } if *actual == request_id && *generation == revision)));
        assert_eq!(
            test.fixture.app.timeline_audio_sync.phase,
            TimelineAudioSyncPhase::Faulted
        );
        assert!(
            test.fixture
                .app
                .audio
                .as_mut()
                .unwrap()
                .next_timeline_runtime_event()
                .is_none()
        );
        assert!(
            test.fixture
                .app
                .audio
                .as_ref()
                .unwrap()
                .resident_timeline_revision()
                .is_none()
        );
        assert_eq!(
            test.fixture.app.timeline_audio_sync.stale_confirmations,
            stale
        );
        assert!(test.fixture.app.clip_restoration_playback_pending);
        assert_eq!(boundary(&test).running, before_stop.running);
    }
    let mut held = held_undo();
    let original = boundary(&held.test);
    // This is the real original 35-second preparation expiry, not a rewritten
    // deadline. No App poll, callback/DSP service, retry or worker replacement
    // fills this wait. MOCK_WAIT remains the separate five-second cleanup cap.
    let expiry = held.started_at + Duration::from_secs(35);
    let expiry_observation_limit = expiry + MOCK_WAIT;
    while Instant::now() <= expiry {
        assert!(Instant::now() < expiry_observation_limit);
        assert_eq!(
            held.test.fixture.app.pending_clip_restorations[&held.channel]
                .genuine_diagnostic_started_at(),
            held.started_at
        );
        assert!(held.gate.is_held());
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(boundary(&held.test), original);
    for (phase, direct) in READY_BOUNDARIES {
        held.test.fixture.app.timeline_audio_sync.phase = phase;
        let before = boundary(&held.test);
        drive_ready_boundary(&mut held.test, phase, direct);
        assert!(matches!(
            route_state(&held.test),
            RequiredGeneratorRouteState::Failed(_)
        ));
        assert!(matches!(
            held.test.fixture.app.timeline_audio_sync.phase,
            TimelineAudioSyncPhase::FaultClearReady { .. } | TimelineAudioSyncPhase::Faulted
        ));
        let after = boundary(&held.test);
        assert_eq!(
            (
                after.audio_commands,
                after.timeline_commands,
                after.activation_pending,
                after.pump
            ),
            (
                before.audio_commands,
                before.timeline_commands,
                before.activation_pending,
                before.pump
            )
        );
        assert_eq!(after.pending, original.pending);
    }
    held.test.fixture.app.poll_clip_restorations();
    assert!(held.test.fixture.app.clip_restoration_failed);
    assert!(held.test.fixture.app.clip_restoration_playback_pending);
    assert!(held.test.fixture.app.clip_restoration_timeline.is_none());
    assert!(held.test.fixture.app.pending_clip_restorations.is_empty());
    assert!(held.test.fixture.app.pending_generator_chains.is_empty());
    assert!(held.test.fixture.app.running_generator_chains.is_empty());
    assert!(!held.test.fixture.app.retired_clip_restorations.is_empty());
    assert!(!held.test.joined_once(held.worker));
    assert_predicate(&held.test, held.generation, false);
    // Release BEFORE any bounded same-worker shutdown polling. The held wrapper
    // also enforces this ordering on assertion unwind; no unwind PASS is claimed.
    held.gate.release();
    let deadline = Instant::now() + MOCK_WAIT;
    while !held.test.fixture.app.retired_clip_restorations.is_empty() {
        assert!(
            Instant::now() < deadline,
            "expired exact worker did not join within original mock cleanup budget"
        );
        held.test.fixture.app.poll_clip_restorations();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(held.test.joined_once(held.worker));
    assert!(held.test.joined_once(held.test.initial_worker));
    assert_eq!(held.test.snapshot().pump_ordinal, original.pump);
    drop(held);
    assert!(!FIXTURE_POISONED.load(Ordering::Acquire));
    SOURCE_SLOTS.with(|slots| assert!(slots.borrow().iter().all(Option::is_none)));
}
