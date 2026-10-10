//! Source-only tests of actual App admission methods and restoration ownership.
//! The callback and identified worker are device-free. Positive restoration tests
//! prove App ownership/admission with an explicitly mock factory; they make no
//! claim that an SDK restoration took place. The runtime adapter has its own tests.
//! No helper, device, native editor, or genuine plug-in is invoked here.
use super::*;
use crate::app::clip_render_job::{
    ClipRenderPhase,
    tests::{FIXTURE_ADMISSION, FIXTURE_POISONED, Fixture},
};
use crate::instrument_bounce::{BounceAudioBus, BouncePluginState};
use crate::plugins::plugin_runtime::{PluginPrepareConfig, test_capture_source::TestCaptureSource};
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicUsize, Ordering},
};

impl PendingClipRestoration {
    pub(in crate::app) fn poll_test_shutdown_retained(&mut self) -> bool {
        let mut complete = true;
        for guard in self
            .chain
            .as_mut()
            .map(|chain| &mut chain.guard)
            .into_iter()
            .chain(self.transferred_guard.as_mut())
        {
            match guard.poll_shutdown_retained() {
                RetainedShutdownPoll::Pending => complete = false,
                RetainedShutdownPoll::Joined => {}
                RetainedShutdownPoll::WorkerPanicked => {
                    panic!("mock restoration worker joined with panic evidence");
                }
            }
        }
        complete
    }
}

fn bare_app() -> Box<CitrusApp> {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx);
    let mut app = CitrusApp::new_boxed_with_services(&cc, false);
    app.project = Project::blank();
    app.history_snapshot = app.project.clone();
    app.history_fingerprint = project_fingerprint(&app.project);
    app.project_fingerprint = app.history_fingerprint;
    app.undo_stack.clear();
    app.redo_stack.clear();
    app.dirty = false;
    app
}

fn project_and_history(app: &CitrusApp) -> Vec<u8> {
    serde_json::to_vec(&(
        &app.project,
        &app.history_snapshot,
        app.history_fingerprint,
        &app.undo_stack,
        &app.redo_stack,
        app.dirty,
    ))
    .unwrap()
}

fn generator_edit_route(fixture: &mut Fixture) -> ParameterEditRoute {
    // Admit generic edits through the mock worker's real ordinary state capture.
    // Its initial deliberately unobserved native dirty state is not bypassed.
    let before_capture = fixture
        .app()
        .running_generator_chains
        .values()
        .next()
        .unwrap()
        .control
        .native_editor_snapshot(0)
        .unwrap()
        .capture_serial;
    assert!(
        fixture
            .app()
            .running_generator_chains
            .values()
            .next()
            .unwrap()
            .control
            .request_state(0)
    );
    fixture.until(|fixture| {
        fixture
            .app()
            .running_generator_chains
            .values()
            .next()
            .unwrap()
            .control
            .native_editor_snapshot(0)
            .is_some_and(|snapshot| {
                snapshot.capture_serial > before_capture && !snapshot.has_uncaptured_changes()
            })
    });
    fixture.app_mut().poll_native_editor_snapshots();
    let app = fixture.app();
    let source = app.running_generator_chains.values().next().unwrap();
    app.plugin_parameter_edit_route(
        app.project_session,
        source.endpoint_id,
        source.instance_id,
        0,
        17,
        true,
    )
    .unwrap()
}

#[test]
fn actual_model_admission_revision_remembers_add_then_undo_before_observer_tick() {
    let mut app = bare_app();
    let before = clip_render::project_snapshot(&app.project).unwrap();
    let revision = app.clip_render_model_revision;
    app.add_pattern();
    assert_eq!(app.clip_render_model_revision, revision + 1);
    assert!(!clip_render::project_matches_snapshot(&app.project, &before).unwrap());
    // Undo itself flushes the unsampled edit. No direct revision or Project
    // assignment manufactures the edit-away/back evidence in this test.
    app.undo();
    assert!(clip_render::project_matches_snapshot(&app.project, &before).unwrap());
    assert_eq!(app.clip_render_model_revision, revision + 2);
    assert_eq!(app.redo_stack.len(), 1);
    app.redo();
    assert_eq!(app.clip_render_model_revision, revision + 3);
    assert!(!clip_render::project_matches_snapshot(&app.project, &before).unwrap());
}

#[test]
fn actual_queued_then_cancelled_parameter_request_keeps_control_revision_advanced() {
    let mut fixture = Fixture::new();
    let route = generator_edit_route(&mut fixture);
    let before = fixture.bytes();
    let revision = fixture.app().clip_render_control_revision;
    fixture.app_mut().request_plugin_parameter_edit(route, 0.5);
    assert_eq!(fixture.app().clip_render_control_revision, revision + 1);
    assert_eq!(
        fixture
            .app()
            .plugin_parameter_edits
            .draft_submissions()
            .len(),
        1
    );
    fixture.app_mut().drive_plugin_parameter_edit_drafts();
    assert!(fixture.app().plugin_parameter_edits.has_pending());
    fixture.app_mut().cancel_plugin_parameter_endpoint_edits(
        route.project_session,
        route.endpoint,
        ParameterEditCancelReason::EditorClosed,
    );
    assert!(!fixture.app().plugin_parameter_edits.has_pending());
    assert_eq!(fixture.app().plugin_parameter_edits.reserved_len(), 0);
    assert_eq!(fixture.app().clip_render_control_revision, revision + 1);
    assert_eq!(fixture.bytes(), before);
    // Cancellation cannot make the request disappear from the source authority
    // revision even when the callback has not yet consumed the queued command.
}

#[test]
fn actual_control_admissions_remember_transport_away_and_back() {
    let mut app = bare_app();
    app.transport_mode = TransportMode::Song;
    let before = clip_render::project_snapshot(&app.project).unwrap();
    let revision = app.clip_render_control_revision;
    app.set_transport_mode(TransportMode::Pattern);
    app.set_transport_mode(TransportMode::Song);
    assert_eq!(app.transport_mode, TransportMode::Song);
    assert_eq!(app.clip_render_control_revision, revision + 2);
    assert!(clip_render::project_matches_snapshot(&app.project, &before).unwrap());
}

#[test]
fn actual_model_entry_points_are_fenced_before_history_dialogs_or_workers() {
    let mut app = bare_app();
    app.undo_stack.push(Project::default());
    app.redo_stack.push(Project::default());
    app.prepare_clip_history_restoration().unwrap();
    assert!(app.clip_operation_fence_active());
    let before = project_and_history(&app);
    let revision = app.clip_render_model_revision;
    let actions: [(&str, fn(&mut CitrusApp)); 12] = [
        ("add Pattern", CitrusApp::add_pattern),
        ("add automation", CitrusApp::add_automation_clip),
        ("Undo", CitrusApp::undo),
        ("Redo", CitrusApp::redo),
        ("New", CitrusApp::new_project),
        ("Open", CitrusApp::open_project),
        ("Save", CitrusApp::save_project),
        ("Save As", CitrusApp::save_project_as),
        ("import audio", CitrusApp::import_audio),
        ("record", CitrusApp::toggle_record),
        ("install Project", |app| {
            assert!(!app.install_project(Project::default(), None, true));
        }),
        ("mixer display move", |app| {
            app.commit_mixer_graph_edit(MixerGraphProjectEdit::MoveDisplay {
                runtime_slot: 1,
                direction: 1,
            });
        }),
    ];
    for (name, action) in actions {
        action(&mut app);
        assert_eq!(
            project_and_history(&app),
            before,
            "{name} crossed the fence"
        );
        assert_eq!(app.clip_render_model_revision, revision, "{name}");
        assert!(app.project_lifecycle.is_idle(), "{name}");
        assert!(app.audio_import_receiver.is_none(), "{name}");
        assert!(app.input_recorder.is_none(), "{name}");
        assert!(app.pending_generator_chains.is_empty(), "{name}");
    }
    app.start_audio_import_path(PathBuf::from("/must-not-open/clip.wav"));
    assert!(app.audio_import_receiver.is_none());
    assert_eq!(project_and_history(&app), before);
}

#[test]
fn actual_controller_fences_parameter_seek_transport_export_and_model_requests() {
    let mut fixture = Fixture::new();
    let route = generator_edit_route(&mut fixture);
    fixture.begin();
    let before = fixture.bytes();
    let model_revision = fixture.app().clip_render_model_revision;
    let control_revision = fixture.app().clip_render_control_revision;
    let transport = fixture.app().transport_mode;
    fixture.app_mut().request_plugin_parameter_edit(route, 0.5);
    fixture.app_mut().publish_transport_seek(2.0);
    fixture.app_mut().set_transport_mode(transport.toggled());
    fixture.app_mut().toggle_play();
    fixture.app_mut().export_wav();
    fixture.app_mut().confirm_wav_export();
    fixture.app_mut().add_pattern();
    fixture.app_mut().undo();
    fixture
        .app_mut()
        .request_project_intent(ProjectIntent::Quit);
    assert!(fixture.app().clip_operation_fence_active());
    assert_eq!(fixture.bytes(), before);
    assert_eq!(fixture.app().clip_render_model_revision, model_revision);
    assert_eq!(fixture.app().clip_render_control_revision, control_revision);
    assert_eq!(fixture.app().transport_mode, transport);
    assert!(!fixture.app().playing);
    assert!(!fixture.app().export_dialog.is_open());
    assert!(!fixture.app().export_job.is_running());
    assert_eq!(fixture.app().plugin_parameter_edits.reserved_len(), 0);
    assert!(fixture.app().project_lifecycle.is_idle());
    fixture.app_mut().stop();
    assert!(!fixture.app().playing);
    assert_eq!(fixture.app().clip_render_control_revision, control_revision);
    assert_eq!(fixture.bytes(), before);
}

#[test]
fn ordinary_close_cancels_but_retains_actual_inflight_capture_owner() {
    let mut fixture = Fixture::new();
    fixture.source_handle().pause_capture();
    fixture.begin();
    fixture.until(|fixture| fixture.source_handle().capture_count() == 1);
    let before = fixture.bytes();
    let ctx = fixture.ctx();
    let mut input = egui::RawInput::default();
    input
        .viewports
        .get_mut(&egui::ViewportId::ROOT)
        .unwrap()
        .events
        .push(egui::ViewportEvent::Close);
    let mut intercepted = false;
    let output = ctx.run_ui(input, |_ui| {
        intercepted = fixture.app_mut().intercept_window_close_request(&ctx);
    });
    assert!(intercepted);
    assert!(
        output.viewport_output[&egui::ViewportId::ROOT]
            .commands
            .iter()
            .any(|command| matches!(command, egui::ViewportCommand::CancelClose))
    );
    assert!(fixture.app().clip_render_fence_active());
    assert!(fixture.app().clip_render_job.is_some());
    assert!(fixture.app().project_lifecycle.is_idle());
    assert_eq!(fixture.bytes(), before);
    // Fixture Drop releases the bounded mock gate and observes exact retained
    // completion. CancelClose must not discard this in-flight worker lease.
}

#[test]
fn failed_restoration_stays_play_blocked_even_when_old_endpoint_and_timeline_are_ready() {
    let mut fixture = Fixture::new();
    assert!(fixture.app().timeline_audio_matches_desired());
    assert!(fixture.app().plugin_runtime_is_ready_for_transport());
    fixture.app_mut().clip_restoration_playback_pending = true;
    fixture.app_mut().clip_restoration_failed = true;
    for _ in 0..3 {
        fixture.app_mut().poll_clip_restorations();
        fixture.app_mut().toggle_play();
        assert!(fixture.app().clip_restoration_playback_pending);
        assert!(fixture.app().clip_restoration_failed);
        assert!(!fixture.app().playing);
    }
    assert!(!fixture.app().clip_restoration_resources_pending());
    assert!(!fixture.app().clip_operation_fence_active());
    // A permanent playback latch must not pretend that cleanup resources still
    // exist or prevent an ordinary user edit once all exact owners have drained.
    let revision = fixture.app().clip_render_model_revision;
    fixture.app_mut().add_pattern();
    assert_eq!(fixture.app().clip_render_model_revision, revision + 1);
    fixture.app_mut().toggle_play();
    assert!(!fixture.app().playing);
}

#[test]
fn actual_committed_failure_blocks_play_after_stop_and_further_polls() {
    let mut fixture = Fixture::new();
    fixture.begin();
    fixture.until(|fixture| {
        fixture
            .app()
            .clip_render_job
            .as_ref()
            .is_some_and(|job| job.phase() == ClipRenderPhase::CommittedSourceJoining)
    });
    let replaced_session = fixture.app().project_session + 1;
    assert!(
        fixture
            .app_mut()
            .audio
            .as_mut()
            .unwrap()
            .set_project_session(replaced_session)
    );
    fixture.until(|fixture| {
        fixture
            .app()
            .clip_render_job
            .as_ref()
            .is_some_and(|job| job.phase() == ClipRenderPhase::CommittedPlaybackFailedPaused)
    });
    let committed = fixture.bytes();
    for _ in 0..3 {
        fixture.app_mut().toggle_play();
        fixture.app_mut().stop();
        fixture.app_mut().drive_clip_render_job();
        assert!(!fixture.app().playing);
        assert!(fixture.app().clip_render_blocks_playback());
    }
    assert_eq!(fixture.bytes(), committed);
}

fn source_state() -> BouncePluginState {
    BouncePluginState {
        metadata: BouncePluginMetadata {
            class_uid: "ABCDEF019182FAEB566D624153675854".into(),
            version: "glue-fixture-1".into(),
            name: "Restoration boundary mock".into(),
            category: "Instrument|Synth".into(),
            event_input_buses: vec![(16, true, Some(true))],
            event_output_buses: vec![],
            input_buses: vec![],
            output_buses: vec![BounceAudioBus::new(2, true, Some(true))],
            latency_samples: 0,
            tail_samples: 0,
        },
        opaque_state: vec![3, 5, 8],
        parameter_bases: BTreeMap::from([(17, 0.25)]),
        native_generation: 0,
        native_dirty_revision: 0,
    }
}

fn source_project() -> Project {
    let mut project = Project::blank();
    project.channels[0].instrument_plugin_instance_id = Some(41);
    project.plugin_instances = vec![PluginInstance {
        id: 41,
        format: ProjectPluginFormat::Vst3,
        role: PluginRole::Instrument,
        path: PathBuf::from("/fixture-only/restoration-boundary.vst3"),
        uid: "descriptor-is-not-a-factory-uid".into(),
        vendor: "Unattested fixture vendor".into(),
        name: source_state().metadata.name,
        enabled: true,
        bypass: false,
        wet: 1.0,
        parameters: BTreeMap::from([(17, 0.25)]),
        opaque_state: vec![3, 5, 8],
        runtime_status: PluginRuntimeStatus::Loaded,
        midi_ports: Default::default(),
    }];
    project
}

fn descriptor(project: &Project) -> PluginDescriptor {
    let instance = &project.plugin_instances[0];
    PluginDescriptor {
        id: instance.uid.clone(),
        name: instance.name.clone(),
        vendor: instance.vendor.clone(),
        path: instance.path.clone(),
        format: ScannedPluginFormat::Vst3,
        category: source_state().metadata.category,
        is_instrument: true,
        verified: true,
        vst3_metadata: Some(plugins::Vst3ScanMetadata {
            class_uid: source_state().metadata.class_uid,
            category: "Instrument|Synth".into(),
            has_midi_input: true,
            has_midi_output: false,
        }),
        scan_error: None,
    }
}

#[test]
fn restoration_expectation_preserves_descriptor_identity_and_rejects_session_or_path_change() {
    let mut app = bare_app();
    app.project = source_project();
    let (audio, callback) = AudioEngine::test_engine_with_callback();
    app.audio = Some(audio);
    let binding = app
        .prepare_clip_render_restoration(41, &source_state().metadata)
        .unwrap();
    assert!(!binding.fresh_load);
    assert_eq!(binding.descriptor_id, "descriptor-is-not-a-factory-uid");
    app.install_clip_render_restoration(binding);
    let mut spec = PluginLoadSpec::from_descriptor(descriptor(&app.project));
    let expectation = app
        .source_restoration_expectation(&app.project, 41, &mut spec, 48_000)
        .unwrap()
        .unwrap();
    assert_eq!(expectation.expected_timing(), (0, 0));
    assert_eq!(
        spec.class_uid.as_deref(),
        Some(source_state().metadata.class_uid.as_str())
    );
    assert_eq!(spec.descriptor.id, "descriptor-is-not-a-factory-uid");
    let mut candidate = app.project.clone();
    candidate.plugin_instances[0].path = "/changed/source.vst3".into();
    assert!(
        app.source_restoration_expectation(&candidate, 41, &mut spec, 48_000)
            .is_err()
    );
    candidate = app.project.clone();
    candidate.plugin_instances[0].uid.push_str("-changed");
    assert!(
        app.source_restoration_expectation(&candidate, 41, &mut spec, 48_000)
            .is_err()
    );
    app.project_session += 1;
    assert!(
        app.source_restoration_expectation(&app.project, 41, &mut spec, 48_000)
            .is_err()
    );
    drop(callback);
}

#[test]
fn restoration_binding_is_optional_and_rejects_changed_dry_configuration_or_bases() {
    let mut app = bare_app();
    app.project = source_project();
    let (audio, callback) = AudioEngine::test_engine_with_callback();
    app.audio = Some(audio);
    let mut spec = PluginLoadSpec::from_descriptor(descriptor(&app.project));
    assert!(
        app.source_restoration_expectation(&app.project, 41, &mut spec, 48_000)
            .unwrap()
            .is_none()
    );
    assert!(spec.class_uid.is_none());
    let binding = app
        .prepare_clip_render_restoration(41, &source_state().metadata)
        .unwrap();
    app.install_clip_render_restoration(binding);
    let mut candidate = app.project.clone();
    candidate.plugin_instances[0].bypass = true;
    assert!(
        app.source_restoration_expectation(&candidate, 41, &mut spec, 48_000)
            .is_err()
    );
    candidate.plugin_instances[0].bypass = false;
    candidate.plugin_instances[0].wet = 0.5;
    assert!(
        app.source_restoration_expectation(&candidate, 41, &mut spec, 48_000)
            .is_err()
    );
    candidate.plugin_instances[0].wet = 1.0;
    candidate.plugin_instances[0]
        .parameters
        .insert(17, f32::NAN);
    assert!(
        app.source_restoration_expectation(&candidate, 41, &mut spec, 48_000)
            .is_err()
    );
    candidate.plugin_instances[0].parameters.insert(17, 0.25);
    candidate.plugin_instances[0].enabled = false;
    assert!(
        app.source_restoration_expectation(&candidate, 41, &mut spec, 48_000)
            .unwrap()
            .is_none()
    );
    drop(callback);
}

#[test]
fn fresh_source_opt_in_is_explicit_scanned_and_limited_to_a_new_source_identity() {
    let mut app = bare_app();
    app.project = source_project();
    let descriptor = descriptor(&app.project);
    app.plugin_picker_fresh_bounce = true;
    app.open_plugin_picker(PluginPickerTarget::ChannelDevice { channel: 0 });
    assert!(!app.plugin_picker_fresh_bounce);
    assert!(app.fresh_bounce_sources.is_empty());
    let before = project_and_history(&app);
    assert!(
        app.record_fresh_clip_source_for_new_load(0, &descriptor)
            .is_err()
    );
    assert!(
        app.record_fresh_clip_source_for_new_load(41, &descriptor)
            .is_err()
    );
    if cfg!(all(
        target_os = "linux",
        feature = "experimental-fresh-helper"
    )) {
        app.record_fresh_clip_source_for_new_load(42, &descriptor)
            .unwrap();
        assert_eq!(
            app.fresh_bounce_sources.get(&42),
            Some(&(
                descriptor.id.clone(),
                descriptor.path.clone(),
                source_state().metadata.class_uid,
            ))
        );
        let mut changed = descriptor.clone();
        changed.path = "/different/module.vst3".into();
        assert!(
            app.record_fresh_clip_source_for_new_load(42, &changed)
                .is_err()
        );
        assert_eq!(app.fresh_bounce_sources[&42].1, descriptor.path);
    } else {
        assert!(
            app.record_fresh_clip_source_for_new_load(42, &descriptor)
                .is_err()
        );
        assert!(app.fresh_bounce_sources.is_empty());
    }
    assert_eq!(project_and_history(&app), before);
    assert!(app.pending_generator_chains.is_empty());
    assert!(app.running_generator_chains.is_empty());
    // The helper records intent only. This test never invokes a plugin loader
    // and does not claim that source preparation or restoration succeeded.
}

#[test]
fn fresh_source_opt_in_refuses_unattested_class_and_wrong_format_before_recording_intent() {
    let mut app = bare_app();
    app.project = source_project();
    let original = descriptor(&app.project);
    for uid in ["", "1234", "ZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ"] {
        let mut descriptor = original.clone();
        descriptor.vst3_metadata.as_mut().unwrap().class_uid = uid.into();
        assert!(
            app.record_fresh_clip_source_for_new_load(42, &descriptor)
                .is_err()
        );
        assert!(app.fresh_bounce_sources.is_empty());
    }
    let mut descriptor = original.clone();
    descriptor.vst3_metadata = None;
    assert!(
        app.record_fresh_clip_source_for_new_load(42, &descriptor)
            .is_err()
    );
    descriptor = original;
    descriptor.format = ScannedPluginFormat::Vst2;
    assert!(
        app.record_fresh_clip_source_for_new_load(42, &descriptor)
            .is_err()
    );
    assert!(app.fresh_bounce_sources.is_empty());
}

/// The only fixture that creates an extra restoration worker owns every location
/// to which its exact guard can move. It is installed before spawning that worker.
struct RestorationFixture {
    app: Option<Box<CitrusApp>>,
    callback: Option<audio::TestAudioCallback>,
    source: Option<TestCaptureSource>,
    untransferred: Option<PluginChain>,
    admission: Option<std::sync::MutexGuard<'static, ()>>,
}

struct RetainedRestorationFailure {
    _app: Option<Box<CitrusApp>>,
    _callback: Option<audio::TestAudioCallback>,
    _source: Option<TestCaptureSource>,
    _untransferred: Option<PluginChain>,
    _admission: Option<std::sync::MutexGuard<'static, ()>>,
    _cleanup_result: Result<Result<(), &'static str>, Box<dyn std::any::Any + Send>>,
}

thread_local! {
    // A timeout is a failed test with exact owners/evidence retained, never a
    // successful join or a detached replacement. Poison blocks later admission.
    static RETAINED_RESTORATION_FAILURE: std::cell::RefCell<Option<std::mem::ManuallyDrop<RetainedRestorationFailure>>>
        = const { std::cell::RefCell::new(None) };
}

impl RestorationFixture {
    fn new() -> Self {
        let deadline = Instant::now() + Duration::from_secs(10);
        let admission = loop {
            assert!(
                !FIXTURE_POISONED.load(Ordering::Acquire),
                "earlier restoration cleanup is unproven; exact owner/evidence remain quarantined"
            );
            match FIXTURE_ADMISSION.try_lock() {
                Ok(guard) => break guard,
                Err(std::sync::TryLockError::Poisoned(poisoned)) => break poisoned.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) => {
                    assert!(
                        Instant::now() < deadline,
                        "restoration fixture admission timed out before spawn"
                    );
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        };
        let mut app = bare_app();
        app.project = source_project();
        app.history_snapshot = app.project.clone();
        app.history_fingerprint = project_fingerprint(&app.project);
        let (mut audio, callback) = AudioEngine::test_engine_with_callback();
        assert!(audio.set_project_session(app.project_session));
        app.audio = Some(audio);
        app.pending_clip_restorations.reserve(1);
        app.retired_clip_restorations.reserve(2);
        let channel = app.project.channels[0].id;
        let live_snapshot = clip_render::project_snapshot(&app.project).unwrap();
        let mut candidate = app.project.clone();
        candidate.name = "Unpublished replacement candidate".into();
        let mut fixture = Self {
            app: Some(app),
            callback: Some(callback),
            source: None,
            untransferred: None,
            admission: Some(admission),
        };
        let (chain, source) = TestCaptureSource::new(
            41,
            source_state(),
            PluginPrepareConfig {
                sample_rate: 48_000.0,
                max_block_frames: MAX_MIXER_BLOCK_FRAMES,
            },
        )
        .unwrap();
        // No assertion, callback, or readiness poll exists between spawn and
        // assigning the exact worker to this already-active cleanup owner.
        fixture.untransferred = Some(chain);
        fixture.source = Some(source);
        let chain = fixture.untransferred.take().unwrap();
        fixture.app_mut().defer_clip_restoration(
            channel,
            41,
            candidate,
            None,
            chain,
            (0, 0),
            live_snapshot,
        );
        fixture
    }

    fn app(&self) -> &CitrusApp {
        self.app.as_deref().unwrap()
    }
    fn app_mut(&mut self) -> &mut CitrusApp {
        self.app.as_deref_mut().unwrap()
    }

    fn wait_for_actual_worker_snapshot(&self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let pending = self
                .app()
                .pending_clip_restorations
                .values()
                .next()
                .unwrap();
            let chain = pending.chain.as_ref().unwrap();
            if let Some(snapshot) = chain.control.plugin_endpoint_snapshot() {
                assert_eq!(chain.control.stats().faults, 0);
                assert!(!chain.control.stats().stopped);
                assert_eq!(snapshot.slot_count(), 1);
                assert_eq!(snapshot.manifest().instance_ids[0], Some(41));
                assert_eq!(snapshot.active_mask(), 1);
                assert_eq!(snapshot.total_plugin_latency_samples(), 0);
                assert_eq!(snapshot.tail_samples(), 0);
                return;
            }
            assert!(
                Instant::now() < deadline,
                "mock never published its real prepared endpoint snapshot"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        // SlotReady remains in the real worker queue for the App to consume.
    }

    fn drain(&mut self, deadline: Instant) -> Result<(), &'static str> {
        if let Some(source) = &self.source {
            source.release_gates();
        }
        self.callback = None;
        let mut worker_panicked = false;
        loop {
            let mut pending = false;
            let mut poll = |guard: &mut PluginWorkerGuard| match guard.poll_shutdown_retained() {
                RetainedShutdownPoll::Pending => pending = true,
                RetainedShutdownPoll::Joined => {}
                RetainedShutdownPoll::WorkerPanicked => worker_panicked = true,
            };
            if let Some(chain) = &mut self.untransferred {
                poll(&mut chain.guard);
            }
            let app = self.app_mut();
            for restoration in app.pending_clip_restorations.values_mut() {
                if let Some(chain) = &mut restoration.chain {
                    poll(&mut chain.guard);
                }
                if let Some(guard) = &mut restoration.transferred_guard {
                    poll(guard);
                }
            }
            for guard in &mut app.retired_clip_restorations {
                poll(guard);
            }
            for chain in app.pending_generator_chains.values_mut() {
                poll(&mut chain.guard);
            }
            for chain in app.running_generator_chains.values_mut() {
                poll(&mut chain.guard);
            }
            if !pending {
                return if worker_panicked {
                    Err("restoration worker joined with panic evidence")
                } else {
                    Ok(())
                };
            }
            if Instant::now() >= deadline {
                return Err("exact restoration worker JoinHandle still pending");
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

impl Drop for RestorationFixture {
    fn drop(&mut self) {
        let was_panicking = std::thread::panicking();
        if let Some(source) = &self.source {
            source.release_gates();
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.drain(Instant::now() + Duration::from_secs(10))
        }));
        if !matches!(result, Ok(Ok(()))) || was_panicking {
            FIXTURE_POISONED.store(true, Ordering::Release);
            let retained = RetainedRestorationFailure {
                _app: self.app.take(),
                _callback: self.callback.take(),
                _source: self.source.take(),
                _untransferred: self.untransferred.take(),
                _admission: self.admission.take(),
                _cleanup_result: result,
            };
            RETAINED_RESTORATION_FAILURE.with(|slot| {
                *slot.borrow_mut() = Some(std::mem::ManuallyDrop::new(retained));
            });
            if !was_panicking {
                panic!(
                    "restoration fixture cleanup unproven; exact owner and Project evidence quarantined; later admission refused"
                );
            }
            return; // Preserve the original assertion panic.
        }
    }
}

#[test]
fn delayed_ready_refuses_stale_project_without_overwriting_unrelated_user_edits() {
    let mut fixture = RestorationFixture::new();
    fixture.wait_for_actual_worker_snapshot();
    fixture.app_mut().project.name =
        "User's unrelated edit while preparation was outstanding".into();
    fixture.app_mut().project.channels[0].name = "Keep this channel rename".into();
    let before = project_and_history(fixture.app());
    fixture.app_mut().poll_clip_restorations();
    assert_eq!(project_and_history(fixture.app()), before);
    assert!(fixture.app().pending_clip_restorations.is_empty());
    assert!(fixture.app().pending_generator_chains.is_empty());
    assert!(fixture.app().running_generator_chains.is_empty());
    assert!(fixture.app().clip_restoration_failed);
    assert!(fixture.app().clip_restoration_playback_pending);
    assert!(!fixture.app().playing);
    assert!(
        fixture
            .app()
            .toast
            .as_ref()
            .unwrap()
            .0
            .contains("live Project changed")
    );
}

#[test]
fn delayed_ready_refuses_changed_project_session_before_candidate_transfer() {
    let mut fixture = RestorationFixture::new();
    fixture.wait_for_actual_worker_snapshot();
    fixture.app_mut().project_session += 1;
    let before = project_and_history(fixture.app());
    fixture.app_mut().poll_clip_restorations();
    assert_eq!(project_and_history(fixture.app()), before);
    assert!(fixture.app().pending_generator_chains.is_empty());
    assert!(fixture.app().clip_restoration_failed);
    assert!(fixture.app().clip_restoration_playback_pending);
}

#[test]
fn delayed_ready_refuses_new_engine_in_same_project_session() {
    let mut fixture = RestorationFixture::new();
    fixture.wait_for_actual_worker_snapshot();
    let old_engine = fixture.app().audio.as_ref().unwrap().engine_session();
    let (mut replacement, callback) = AudioEngine::test_engine_with_callback();
    assert_ne!(replacement.engine_session(), old_engine);
    assert!(replacement.set_project_session(fixture.app().project_session));
    fixture.callback = Some(callback);
    fixture.app_mut().audio = Some(replacement);
    let before = project_and_history(fixture.app());
    fixture.app_mut().poll_clip_restorations();
    assert_eq!(project_and_history(fixture.app()), before);
    assert!(fixture.app().pending_generator_chains.is_empty());
    assert!(fixture.app().clip_restoration_failed);
    assert!(fixture.app().clip_restoration_playback_pending);
}

#[test]
fn delayed_ready_refuses_latest_timing_mismatch_without_endpoint_admission() {
    let mut fixture = RestorationFixture::new();
    fixture.wait_for_actual_worker_snapshot();
    let mut changed = source_state();
    changed.metadata.latency_samples = 1;
    fixture.source.as_ref().unwrap().set_capture_state(changed);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let pending = fixture
            .app()
            .pending_clip_restorations
            .values()
            .next()
            .unwrap();
        assert_eq!(pending.expected_timing, (0, 0));
        if pending
            .chain
            .as_ref()
            .unwrap()
            .control
            .plugin_endpoint_snapshot()
            .is_some_and(|snapshot| snapshot.total_plugin_latency_samples() == 1)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "mock worker did not publish changed coherent timing"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    // The real queued SlotReady still describes zero latency. Its late arrival
    // cannot outweigh the latest coherent worker snapshot read before transfer.
    let before = project_and_history(fixture.app());
    fixture.app_mut().poll_clip_restorations();
    assert_eq!(project_and_history(fixture.app()), before);
    assert!(fixture.app().pending_generator_chains.is_empty());
    assert!(fixture.app().clip_restoration_failed);
    assert!(
        fixture
            .app()
            .toast
            .as_ref()
            .unwrap()
            .0
            .contains("identity/timing")
    );
}

#[test]
fn ordinary_and_already_armed_window_close_are_vetoed_while_exact_restoration_owner_is_pending() {
    let mut fixture = RestorationFixture::new();
    fixture.app_mut().clip_restoration_failed = true;
    assert!(fixture.app().clip_restoration_resources_pending());
    let ctx = egui::Context::default();
    let mut input = egui::RawInput::default();
    input
        .viewports
        .get_mut(&egui::ViewportId::ROOT)
        .unwrap()
        .events
        .push(egui::ViewportEvent::Close);
    let mut intercepted = false;
    let output = ctx.run_ui(input, |_ui| {
        intercepted = fixture.app_mut().intercept_window_close_request(&ctx);
    });
    assert!(intercepted);
    assert!(
        output.viewport_output[&egui::ViewportId::ROOT]
            .commands
            .iter()
            .any(|command| matches!(command, egui::ViewportCommand::CancelClose))
    );
    assert!(fixture.app().project_lifecycle.is_idle());
    fixture.app_mut().project_lifecycle = ProjectLifecycle::ExitArmed {
        close_command_sent: false,
    };
    let output = ctx.run_ui(egui::RawInput::default(), |_ui| {
        fixture.app_mut().dispatch_armed_window_close(&ctx, false);
    });
    let commands = &output.viewport_output[&egui::ViewportId::ROOT].commands;
    assert!(
        commands
            .iter()
            .any(|command| matches!(command, egui::ViewportCommand::CancelClose))
    );
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command, egui::ViewportCommand::Close))
    );
    assert!(fixture.app().clip_restoration_resources_pending());
}

static RESTORATION_FACTORY_CALLS: AtomicUsize = AtomicUsize::new(0);

/// This seam tests the App controller only. The production expectation is built
/// and its exact spec validated before this hook; actual vendor state/base/Start
/// attestation is tested separately by the source-restoration protocol fixtures.
pub(in crate::app) fn authoritative_mock_restoration_factory(
    candidate: &Project,
    instance_id: u64,
    spec: PluginLoadSpec,
    config: PluginPrepareConfig,
    metadata: &BouncePluginMetadata,
) -> Result<PluginChain, String> {
    let instance = candidate
        .plugin_instances
        .iter()
        .find(|item| item.id == instance_id)
        .ok_or("mock restoration candidate lost its exact instance")?;
    if instance_id != 41
        || !instance.enabled
        || instance.bypass
        || instance.wet.to_bits() != 1.0_f32.to_bits()
        || instance.parameters.get(&17).map(|value| value.to_bits()) != Some(0.75_f32.to_bits())
        || instance.opaque_state != [7, 8, 9]
        || spec.initial_state != instance.opaque_state
        || spec.descriptor.id != instance.uid
        || spec.descriptor.path != instance.path
        || spec.class_uid.as_deref() != Some(metadata.class_uid.as_str())
        || spec.descriptor.id == metadata.class_uid
        || config.sample_rate != 48_000.0
        || config.max_block_frames != MAX_MIXER_BLOCK_FRAMES
    {
        return Err(
            "mock factory did not receive exact authoritative source state and identity".into(),
        );
    }
    let state = BouncePluginState {
        metadata: metadata.clone(),
        opaque_state: instance.opaque_state.clone(),
        parameter_bases: instance.parameters.clone(),
        native_generation: 0,
        native_dirty_revision: 0,
    };
    RESTORATION_FACTORY_CALLS.fetch_add(1, Ordering::AcqRel);
    let (chain, source) = TestCaptureSource::new(instance_id, state, config)?;
    // The source handle has no JoinHandle or held gate. The returned real chain
    // is immediately retained by the production pending-restoration owner.
    drop(source);
    Ok(chain)
}

fn wait_for_held_restoration_ready(fixture: &Fixture, channel: u32) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let pending = fixture
            .app()
            .pending_clip_restorations
            .get(&channel)
            .unwrap();
        assert!(
            !pending.ready,
            "App must not consume Ready before the explicit poll"
        );
        let chain = pending.chain.as_ref().unwrap();
        if let Some(snapshot) = chain.control.plugin_endpoint_snapshot() {
            assert_eq!(chain.control.stats().faults, 0);
            assert!(!chain.control.stats().stopped);
            assert_eq!(snapshot.slot_count(), 1);
            assert_eq!(snapshot.manifest().instance_ids[0], Some(41));
            assert_eq!(snapshot.active_mask(), 1);
            assert_eq!(
                (
                    snapshot.total_plugin_latency_samples(),
                    snapshot.tail_samples()
                ),
                pending.expected_timing
            );
            return;
        }
        assert!(
            Instant::now() < deadline,
            "actual mock restoration Ready did not arrive"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn assert_exclusive_compiled_source(app: &CitrusApp, expect_audio: bool) {
    let (_, timeline) = app
        .latest_compiled_timeline()
        .expect("current compiled Timeline");
    let notes = timeline
        .events()
        .iter()
        .filter(|event| {
            matches!(
                event.kind,
                crate::timeline::TimelineEventKind::NoteOn { .. }
            )
        })
        .count();
    let audio = timeline
        .events()
        .iter()
        .filter(|event| {
            matches!(
                event.kind,
                crate::timeline::TimelineEventKind::AudioStart(_)
            )
        })
        .count();
    if expect_audio {
        assert_eq!(
            notes, 0,
            "muted retained MIDI must not double-play with rendered audio"
        );
        assert_eq!(audio, 1);
    } else {
        assert!(
            notes > 0,
            "Undo must restore the actual selected MIDI occurrence"
        );
        assert_eq!(
            audio, 0,
            "Undo must remove rendered audio from the compiled Timeline"
        );
    }
    assert!(!app.playing);
    assert!(!app.audio.as_ref().unwrap().snapshot().transport_playing);
    assert!(app.timeline_audio_matches_desired());
}

#[test]
fn actual_conversion_undo_ready_install_new_timeline_and_redo_preserve_authoritative_source() {
    let mut fixture = Fixture::new();
    RESTORATION_FACTORY_CALLS.store(0, Ordering::Release);
    fixture.app_mut().clip_restoration_test_factory = Some(authoritative_mock_restoration_factory);
    let channel = fixture.app().project.channels[0].id;
    let source_clip = fixture.app().project.clips[0].id;
    assert_eq!(
        fixture.app().project.plugin_instances[0].parameters[&17].to_bits(),
        0.25_f32.to_bits()
    );
    assert_eq!(
        fixture.app().project.plugin_instances[0].opaque_state,
        [1, 2, 3]
    );
    fixture.begin();
    fixture.until(|fixture| {
        fixture
            .app()
            .clip_render_job
            .as_ref()
            .is_some_and(|job| job.phase() == ClipRenderPhase::Complete)
    });
    assert!(fixture.original_source_joined());
    assert_eq!(fixture.source_handle().capture_count(), 2);
    assert!(fixture.app().running_generator_chains.is_empty());
    assert!(fixture.app().pending_generator_chains.is_empty());
    assert_eq!(RESTORATION_FACTORY_CALLS.load(Ordering::Acquire), 0);
    let committed_project = clip_render::project_snapshot(&fixture.app().project).unwrap();
    let authoritative_before =
        clip_render::project_snapshot(fixture.app().undo_stack.last().unwrap()).unwrap();
    let asset_id = fixture.app().project.audio_assets[0].id;
    let committed_pcm = Arc::clone(&fixture.app().cached_audio_assets[&asset_id].samples);
    let converted_generation = fixture.app().timeline_desired_generation;
    assert!(!fixture.app().project.plugin_instances[0].enabled);
    assert_eq!(
        fixture.app().project.plugin_instances[0].parameters[&17].to_bits(),
        0.75_f32.to_bits()
    );
    assert_eq!(
        fixture.app().project.plugin_instances[0].opaque_state,
        [7, 8, 9]
    );
    assert_exclusive_compiled_source(fixture.app(), true);

    fixture.app_mut().undo();
    assert_eq!(RESTORATION_FACTORY_CALLS.load(Ordering::Acquire), 1);
    assert!(
        clip_render::project_matches_snapshot(&fixture.app().project, &authoritative_before)
            .unwrap()
    );
    assert!(fixture.app().project.plugin_instances[0].enabled);
    assert!(fixture.app().project.audio_assets.is_empty());
    assert!(
        !fixture
            .app()
            .project
            .clips
            .iter()
            .find(|clip| clip.id == source_clip)
            .unwrap()
            .muted
    );
    assert!(
        fixture
            .app()
            .pending_clip_restorations
            .contains_key(&channel)
    );
    assert!(fixture.app().pending_generator_chains.is_empty());
    assert!(fixture.app().running_generator_chains.is_empty());
    assert!(fixture.app().clip_restoration_playback_pending);
    fixture.app_mut().toggle_play();
    assert!(!fixture.app().playing);
    wait_for_held_restoration_ready(&fixture, channel);
    let held_project = clip_render::project_snapshot(&fixture.app().project).unwrap();
    let held_session = fixture.app().project_session;
    let held_pcm_bytes = fixture.app().retained_clip_pcm_bytes;
    let held_entries = fixture.app().retained_clip_media.len();
    fixture
        .app_mut()
        .install_project(Project::blank(), None, false);
    assert_eq!(fixture.app().project_session, held_session);
    assert_eq!(fixture.app().retained_clip_pcm_bytes, held_pcm_bytes);
    assert_eq!(fixture.app().retained_clip_media.len(), held_entries);
    assert!(clip_render::project_matches_snapshot(&fixture.app().project, &held_project).unwrap());
    assert!(
        fixture
            .app()
            .pending_clip_restorations
            .contains_key(&channel)
    );
    assert_eq!(RESTORATION_FACTORY_CALLS.load(Ordering::Acquire), 1);
    // The real worker is prepared; only actual App polling and callback receipt
    // may transfer this owner and subsequently release the playback barrier.
    fixture.app_mut().poll_clip_restorations();
    let restored_endpoint = fixture.app().pending_generator_chains[&channel].endpoint_id;
    assert!(fixture.app().pending_clip_restorations.is_empty());
    assert!(fixture.app().running_generator_chains.is_empty());
    assert_eq!(
        fixture.app().clip_restoration_installs.get(&41),
        Some(&(restored_endpoint, false))
    );
    assert!(fixture.app().clip_restoration_playback_pending);
    fixture.app_mut().toggle_play();
    assert!(!fixture.app().playing);
    fixture.until(|fixture| {
        !fixture.app().clip_restoration_playback_pending
            && !fixture.app().clip_restoration_failed
            && fixture.app().timeline_audio_matches_desired()
    });
    assert_eq!(fixture.app().running_generator_chains.len(), 1);
    assert_eq!(
        fixture.app().running_generator_chains[&channel].endpoint_id,
        restored_endpoint
    );
    assert_eq!(
        fixture.app().clip_restoration_installs.get(&41),
        Some(&(restored_endpoint, true))
    );
    assert!(fixture.app().timeline_desired_generation > converted_generation);
    assert_eq!(
        fixture
            .app()
            .audio
            .as_ref()
            .unwrap()
            .confirmed_timeline_revision(),
        Some(fixture.app().timeline_desired_generation)
    );
    assert!(
        clip_render::project_matches_snapshot(&fixture.app().project, &authoritative_before)
            .unwrap()
    );
    assert_exclusive_compiled_source(fixture.app(), false);
    let undo_generation = fixture.app().timeline_desired_generation;

    fixture.app_mut().redo();
    assert_eq!(
        RESTORATION_FACTORY_CALLS.load(Ordering::Acquire),
        1,
        "disabled retained source must not be respawned during Redo"
    );
    assert!(
        clip_render::project_matches_snapshot(&fixture.app().project, &committed_project).unwrap()
    );
    assert!(
        fixture
            .app()
            .pending_generator_removals
            .contains_key(&channel)
    );
    assert!(fixture.app().clip_restoration_playback_pending);
    fixture.app_mut().toggle_play();
    assert!(!fixture.app().playing);
    fixture.until(|fixture| {
        !fixture.app().clip_restoration_playback_pending
            && !fixture.app().clip_restoration_failed
            && fixture.app().timeline_audio_matches_desired()
    });
    assert!(fixture.app().running_generator_chains.is_empty());
    assert!(fixture.app().pending_generator_chains.is_empty());
    assert!(fixture.app().pending_generator_removals.is_empty());
    assert!(fixture.app().pending_clip_restorations.is_empty());
    assert!(
        fixture.app().retired_clip_restorations.is_empty(),
        "gate cannot release before exact restored source join"
    );
    assert!(fixture.app().timeline_desired_generation > undo_generation);
    assert!(
        Arc::ptr_eq(
            &committed_pcm,
            &fixture.app().cached_audio_assets[&asset_id].samples
        ),
        "Redo must reuse the retained verified PCM allocation"
    );
    assert!(
        clip_render::project_matches_snapshot(&fixture.app().project, &committed_project).unwrap()
    );
    assert_eq!(fixture.app().undo_stack.len(), 1);
    assert!(fixture.app().redo_stack.is_empty());
    assert_exclusive_compiled_source(fixture.app(), true);

    // A later new source admission occurs only after the previous real worker
    // has drained and the prior endpoint/Timeline transition has completed.
    fixture.app_mut().undo();
    assert_eq!(RESTORATION_FACTORY_CALLS.load(Ordering::Acquire), 2);
    assert!(fixture.app().running_generator_chains.is_empty());
    assert!(fixture.app().retired_clip_restorations.is_empty());
    assert!(
        fixture
            .app()
            .pending_clip_restorations
            .contains_key(&channel)
    );
    fixture.until(|fixture| {
        !fixture.app().clip_restoration_playback_pending
            && !fixture.app().clip_restoration_failed
            && fixture.app().timeline_audio_matches_desired()
    });
    assert_exclusive_compiled_source(fixture.app(), false);
}

fn assert_redo_uses_retained_pcm_after_external_file_change(delete_file: bool) {
    let mut fixture = Fixture::new();
    RESTORATION_FACTORY_CALLS.store(0, Ordering::Release);
    fixture.app_mut().clip_restoration_test_factory = Some(authoritative_mock_restoration_factory);
    fixture.begin();
    fixture.until(|fixture| {
        fixture
            .app()
            .clip_render_job
            .as_ref()
            .is_some_and(|job| job.phase() == ClipRenderPhase::Complete)
    });
    assert!(fixture.original_source_joined());
    let asset = fixture.app().project.audio_assets[0].clone();
    let pcm = Arc::clone(&fixture.app().cached_audio_assets[&asset.id].samples);
    let charged_bytes = fixture.app().retained_clip_pcm_bytes;
    let converted = clip_render::project_snapshot(&fixture.app().project).unwrap();
    assert!(asset.path.is_file());
    assert_eq!(fixture.app().retained_clip_media.len(), 1);
    fixture.app_mut().undo();
    fixture.until(|fixture| {
        !fixture.app().clip_restoration_playback_pending
            && !fixture.app().clip_restoration_failed
            && fixture.app().timeline_audio_matches_desired()
    });
    assert!(
        !fixture.app().cached_audio_assets.contains_key(&asset.id),
        "ordinary cache pruning must actually occur before testing retained reuse"
    );
    assert_eq!(fixture.app().retained_clip_pcm_bytes, charged_bytes);
    assert_eq!(fixture.app().retained_clip_media.len(), 1);
    // This path belongs to the just-created fixture render. External file
    // mutability is intentional; it is not source authority within this session.
    if delete_file {
        std::fs::remove_file(&asset.path).unwrap();
        assert!(!asset.path.exists());
    } else {
        std::fs::write(
            &asset.path,
            b"changed external file; deliberately not a WAV",
        )
        .unwrap();
    }
    fixture.app_mut().redo();
    fixture.until(|fixture| {
        !fixture.app().clip_restoration_playback_pending
            && !fixture.app().clip_restoration_failed
            && fixture.app().timeline_audio_matches_desired()
    });
    assert!(Arc::ptr_eq(
        &pcm,
        &fixture.app().cached_audio_assets[&asset.id].samples
    ));
    assert_eq!(fixture.app().retained_clip_pcm_bytes, charged_bytes);
    assert_eq!(fixture.app().retained_clip_media.len(), 1);
    assert!(fixture.app().loading_audio_assets.is_empty());
    assert!(!fixture.app().failed_audio_assets.contains_key(&asset.id));
    assert!(clip_render::project_matches_snapshot(&fixture.app().project, &converted).unwrap());
    assert_eq!(RESTORATION_FACTORY_CALLS.load(Ordering::Acquire), 1);
    assert_exclusive_compiled_source(fixture.app(), true);
    // No file restore or immutability assertion: the current session has reused
    // its exact already-verified allocation and exact current asset binding.
}

#[test]
fn actual_undo_redo_reuses_verified_arc_after_published_wav_is_modified() {
    assert_redo_uses_retained_pcm_after_external_file_change(false);
}

#[test]
fn actual_undo_redo_reuses_verified_arc_after_published_wav_is_deleted() {
    assert_redo_uses_retained_pcm_after_external_file_change(true);
}

#[test]
fn actual_retired_restoration_worker_shutdown_panic_is_a_sticky_playback_failure() {
    let mut fixture = RestorationFixture::new();
    fixture.wait_for_actual_worker_snapshot();
    fixture.source.as_ref().unwrap().panic_on_shutdown();
    let channel = fixture.app().project.channels[0].id;
    // Move this actual unpublished owner to the same App retirement list used
    // by failed preparation. No callback installation or Ready is fabricated.
    let mut pending = fixture
        .app_mut()
        .pending_clip_restorations
        .remove(&channel)
        .unwrap();
    let PluginChain {
        audio,
        control,
        guard,
    } = pending.chain.take().unwrap();
    fixture.app_mut().retired_clip_restorations.push(guard);
    drop(audio);
    drop(control);
    assert!(!fixture.app().clip_restoration_failed);
    assert!(fixture.app().clip_restoration_playback_pending);
    let before = project_and_history(fixture.app());
    let deadline = Instant::now() + Duration::from_secs(5);
    while !fixture.app().retired_clip_restorations.is_empty() {
        fixture.app_mut().poll_clip_restorations();
        assert!(
            Instant::now() < deadline,
            "actual panicking worker was not joined by App retirement"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(
        fixture.app().clip_restoration_failed,
        "WorkerPanicked is termination evidence, not a successful restoration"
    );
    assert!(fixture.app().clip_restoration_playback_pending);
    assert_eq!(project_and_history(fixture.app()), before);
    for _ in 0..3 {
        fixture.app_mut().poll_clip_restorations();
        fixture.app_mut().toggle_play();
        assert!(fixture.app().clip_restoration_failed);
        assert!(fixture.app().clip_restoration_playback_pending);
        assert!(!fixture.app().playing);
    }
    assert!(!fixture.app().clip_restoration_resources_pending());
}

fn fill_ordinary_ring_and_prove_stop_all_rejected(app: &mut CitrusApp) -> u64 {
    let audio = app.audio.as_mut().unwrap();
    let mut full = false;
    // TestAudioCallback owns a 64-entry ring. These nonzero gain commands neither
    // pause/mute the graph nor clear its poisoned history.
    for _ in 0..=64 {
        if !audio.command(AudioCommand::SetMaster(1.0)) {
            full = true;
            break;
        }
    }
    assert!(
        full,
        "bounded fixture ring fill must reach real queue pressure"
    );
    assert!(
        !audio.command(AudioCommand::StopAll),
        "ordinary StopAll must really be rejected"
    );
    audio.snapshot().command_queue_full
}

fn drive_restoration_and_assert_silent_gate(
    fixture: &mut Fixture,
    previous_generation: u64,
    previous_epoch: u64,
) -> (u64, u64) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut reset_generation = None;
    assert!(fixture.app().clip_restoration_playback_pending);
    while fixture.app().clip_restoration_playback_pending {
        assert!(!fixture.app().playing);
        assert!(!fixture.app().clip_restoration_failed);
        if let Some((engine, generation)) = fixture.app().clip_restoration_timeline {
            assert_eq!(
                engine,
                fixture.app().audio.as_ref().unwrap().engine_session()
            );
            if let Some(target) = reset_generation {
                assert_eq!(generation, target);
            } else {
                // Source preparation/join can permit an intermediate activation.
                // Observe only the new generation that actually releases this gate.
                fixture
                    .callback_mut()
                    .target_restoration_reset_generation(generation);
                reset_generation = Some(generation);
            }
        }
        fixture.tick();
        let output = fixture.callback().last_output();
        assert_eq!(output.len(), 128);
        assert!(
            output
                .iter()
                .flatten()
                .all(|sample| sample.is_finite() && *sample == 0.0),
            "poisoned old callback state escaped while the App restoration gate was closed"
        );
        assert!(!fixture.app().playing);
        assert!(
            !fixture
                .app()
                .audio
                .as_ref()
                .unwrap()
                .snapshot()
                .transport_playing
        );
        assert!(
            Instant::now() < deadline,
            "actual restoration did not receive its bounded new Timeline receipt"
        );
    }
    let app = fixture.app();
    let audio = app.audio.as_ref().unwrap();
    let snapshot = audio.snapshot();
    assert!(app.timeline_desired_generation > previous_generation);
    assert_ne!(snapshot.transport_epoch, previous_epoch);
    assert_eq!(
        audio.confirmed_timeline_identity(),
        Some((app.timeline_desired_generation, snapshot.transport_epoch))
    );
    assert!(app.timeline_audio_matches_desired());
    assert!(!app.clip_restoration_failed);
    assert_eq!(reset_generation, Some(app.timeline_desired_generation));
    (app.timeline_desired_generation, snapshot.transport_epoch)
}

#[test]
fn actual_undo_redo_new_timeline_clears_callback_residues_when_ordinary_stop_all_queue_is_full() {
    let mut fixture = Fixture::new();
    RESTORATION_FACTORY_CALLS.store(0, Ordering::Release);
    fixture.app_mut().clip_restoration_test_factory = Some(authoritative_mock_restoration_factory);
    fixture.begin();
    fixture.until(|fixture| {
        fixture
            .app()
            .clip_render_job
            .as_ref()
            .is_some_and(|job| job.phase() == ClipRenderPhase::Complete)
    });
    assert!(fixture.original_source_joined());
    let channel = fixture.app().project.channels[0].id;
    let generation = fixture.app().timeline_desired_generation;
    let epoch = fixture
        .app()
        .audio
        .as_ref()
        .unwrap()
        .snapshot()
        .transport_epoch;
    let poisoned = fixture.callback_mut().poison_restoration_residues(channel);
    assert!(poisoned.raw_track_delays > 0);
    assert!(
        !poisoned.generator_pdc,
        "converted source is already removed"
    );
    let rejected = fill_ordinary_ring_and_prove_stop_all_rejected(fixture.app_mut());
    fixture.app_mut().undo();
    assert!(
        fixture
            .app()
            .audio
            .as_ref()
            .unwrap()
            .snapshot()
            .command_queue_full
            > rejected,
        "actual Undo's best-effort commands must encounter the still-full ring"
    );
    assert!(fixture.app().clip_restoration_playback_pending);
    assert_eq!(
        fixture
            .app()
            .audio
            .as_ref()
            .unwrap()
            .confirmed_timeline_revision(),
        Some(generation)
    );
    fixture.app_mut().toggle_play();
    assert!(!fixture.app().playing);
    let (activated_generation, activated_epoch) =
        drive_restoration_and_assert_silent_gate(&mut fixture, generation, epoch);
    // Read the observation taken at this exact Timeline activation, before any
    // render advancement. The callback seam re-poisons immediately before the
    // production reset so ordinary 128-frame advancement cannot fake the proof.
    let reset = fixture.callback_mut().assert_restoration_residues_cleared(
        channel,
        activated_generation,
        activated_epoch,
    );
    assert!(reset.extant.raw_track_delays > 0);
    assert!(
        reset.extant.generator_pdc,
        "Undo has installed its actual new source"
    );
    assert!(reset.extant.active_graph_routes > 0);
    assert_eq!(reset.removed_generator_endpoint, None);
    assert_exclusive_compiled_source(fixture.app(), false);

    let removed_endpoint = fixture.app().running_generator_chains[&channel].endpoint_id;
    let generation = fixture.app().timeline_desired_generation;
    let epoch = fixture
        .app()
        .audio
        .as_ref()
        .unwrap()
        .snapshot()
        .transport_epoch;
    let poisoned = fixture.callback_mut().poison_restoration_residues(channel);
    assert!(poisoned.raw_track_delays > 0);
    assert!(poisoned.generator_pdc);
    assert!(
        poisoned.active_graph_routes > 0,
        "active mock generator bridge must supply an actual positive-delay graph route"
    );
    let rejected = fill_ordinary_ring_and_prove_stop_all_rejected(fixture.app_mut());
    fixture.app_mut().redo();
    assert!(
        fixture
            .app()
            .audio
            .as_ref()
            .unwrap()
            .snapshot()
            .command_queue_full
            > rejected
    );
    assert!(fixture.app().clip_restoration_playback_pending);
    assert_eq!(
        fixture
            .app()
            .audio
            .as_ref()
            .unwrap()
            .confirmed_timeline_revision(),
        Some(generation)
    );
    fixture.app_mut().toggle_play();
    assert!(!fixture.app().playing);
    let (activated_generation, activated_epoch) =
        drive_restoration_and_assert_silent_gate(&mut fixture, generation, epoch);
    let reset = fixture.callback_mut().assert_restoration_residues_cleared(
        channel,
        activated_generation,
        activated_epoch,
    );
    assert!(reset.extant.raw_track_delays > 0);
    assert!(
        !reset.extant.generator_pdc,
        "Redo removed its actual old source"
    );
    assert_eq!(reset.removed_generator_endpoint, Some(removed_endpoint));
    assert!(fixture.app().running_generator_chains.is_empty());
    assert!(fixture.app().retired_clip_restorations.is_empty());
    assert_exclusive_compiled_source(fixture.app(), true);
    // Coverage is exact-activation voice/raw history resets, Undo's newly
    // installed generator/observed graph history reset, and silent gate output.
    // Redo excludes the old generator by removal, not by observing its absent
    // history. Nonzero fixed-quantum FIFO reset has separate stamped-removal tests.
}

fn converted_fixture_with_mock_restoration() -> Fixture {
    let mut fixture = Fixture::new();
    RESTORATION_FACTORY_CALLS.store(0, Ordering::Release);
    fixture.app_mut().clip_restoration_test_factory = Some(authoritative_mock_restoration_factory);
    fixture.begin();
    fixture.until(|fixture| {
        fixture
            .app()
            .clip_render_job
            .as_ref()
            .is_some_and(|job| job.phase() == ClipRenderPhase::Complete)
    });
    assert!(fixture.original_source_joined());
    fixture
}

fn assert_receipt_panic_keeps_admission_closed(fixture: &mut Fixture) {
    assert!(fixture.app().clip_restoration_failed);
    assert!(fixture.app().clip_restoration_playback_pending);
    assert!(fixture.app().clip_restoration_resources_pending());
    assert!(fixture.app().clip_operation_fence_active());
    let before = fixture.bytes();
    let session = fixture.app().project_session;
    let retained_bytes = fixture.app().retained_clip_pcm_bytes;
    let retained_entries = fixture.app().retained_clip_media.len();
    let calls = RESTORATION_FACTORY_CALLS.load(Ordering::Acquire);
    for _ in 0..3 {
        fixture.app_mut().toggle_play();
        fixture.app_mut().add_pattern();
        fixture.app_mut().undo();
        fixture.app_mut().redo();
        fixture
            .app_mut()
            .install_project(Project::blank(), None, false);
        fixture.tick();
        assert!(fixture.app().clip_restoration_failed);
        assert!(fixture.app().clip_restoration_playback_pending);
        assert!(fixture.app().clip_restoration_resources_pending());
        assert!(!fixture.app().playing);
        assert!(
            !fixture
                .app()
                .audio
                .as_ref()
                .unwrap()
                .snapshot()
                .transport_playing
        );
        assert_eq!(fixture.bytes(), before);
        assert_eq!(fixture.app().project_session, session);
        assert_eq!(fixture.app().retained_clip_pcm_bytes, retained_bytes);
        assert_eq!(fixture.app().retained_clip_media.len(), retained_entries);
        assert_eq!(RESTORATION_FACTORY_CALLS.load(Ordering::Acquire), calls);
    }
    let ctx = fixture.ctx();
    let mut input = egui::RawInput::default();
    input
        .viewports
        .get_mut(&egui::ViewportId::ROOT)
        .unwrap()
        .events
        .push(egui::ViewportEvent::Close);
    let mut intercepted = false;
    let output = ctx.run_ui(input, |_ui| {
        intercepted = fixture.app_mut().intercept_window_close_request(&ctx);
    });
    assert!(intercepted);
    let commands = &output.viewport_output[&egui::ViewportId::ROOT].commands;
    assert!(
        commands
            .iter()
            .any(|command| matches!(command, egui::ViewportCommand::CancelClose))
    );
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command, egui::ViewportCommand::Close))
    );
    assert!(fixture.app().project_lifecycle.is_idle());
}

#[test]
fn actual_installed_receipt_panic_retains_exact_pending_owner_and_blocks_later_admission() {
    let mut fixture = converted_fixture_with_mock_restoration();
    let channel = fixture.app().project.channels[0].id;
    fixture.app_mut().undo();
    wait_for_held_restoration_ready(&fixture, channel);
    fixture.app_mut().poll_clip_restorations();
    assert!(fixture.app().pending_clip_restorations.is_empty());
    let pending = &fixture.app().pending_generator_chains[&channel];
    let identity = (
        pending.project_session,
        pending.endpoint_id,
        pending.instance_id,
    );
    let exact_guard = &pending.guard as *const PluginWorkerGuard;
    assert!(!pending.control.stats().stopped);
    assert!(fixture.app().running_generator_chains.is_empty());
    assert_eq!(
        fixture.app().clip_restoration_installs.get(&41),
        Some(&(identity.1, false))
    );
    let before = fixture.bytes();
    fixture.app_mut().clip_restoration_event_panic = Some("installed");
    // No receipt is fabricated: Fixture::tick pumps the real callback and then
    // consumes its Installed event through the production panic boundary.
    fixture.until(|fixture| fixture.app().clip_restoration_event_panic.is_none());
    assert!(fixture.app().clip_restoration_failed);
    assert_eq!(fixture.bytes(), before);
    assert!(fixture.app().running_generator_chains.is_empty());
    assert!(fixture.app().retired_clip_restorations.is_empty());
    let pending = &fixture.app().pending_generator_chains[&channel];
    assert_eq!(
        (
            pending.project_session,
            pending.endpoint_id,
            pending.instance_id
        ),
        identity
    );
    assert!(std::ptr::eq(exact_guard, &pending.guard));
    assert!(!pending.control.stats().stopped);
    assert_eq!(pending.control.stats().detached_workers, 0);
    assert_eq!(
        fixture.app().clip_restoration_installs.get(&41),
        Some(&(identity.1, false))
    );
    assert_receipt_panic_keeps_admission_closed(&mut fixture);
    assert!(std::ptr::eq(
        exact_guard,
        &fixture.app().pending_generator_chains[&channel].guard
    ));
    // Fixture owns this callback and exact pending guard through bounded cleanup.
}

#[test]
fn actual_removed_receipt_panic_retains_exact_running_owner_and_blocks_later_admission() {
    let mut fixture = converted_fixture_with_mock_restoration();
    let channel = fixture.app().project.channels[0].id;
    fixture.app_mut().undo();
    fixture.until(|fixture| !fixture.app().clip_restoration_playback_pending);
    assert_exclusive_compiled_source(fixture.app(), false);
    let running = &fixture.app().running_generator_chains[&channel];
    let identity = (
        running.project_session,
        running.endpoint_id,
        running.instance_id,
    );
    let exact_guard = &running.guard as *const PluginWorkerGuard;
    assert!(!running.control.stats().stopped);
    fixture.app_mut().redo();
    let removal = &fixture.app().pending_generator_removals[&channel];
    assert_eq!(
        (
            removal.project_session,
            removal.endpoint_id,
            removal.plugin_instance_id
        ),
        (identity.0, Some(identity.1), Some(identity.2))
    );
    let before = fixture.bytes();
    fixture.app_mut().clip_restoration_event_panic = Some("removed");
    // The real callback removes its endpoint. The panic is only in the App's
    // subsequent receipt consumption, before taking this exact worker guard.
    fixture.until(|fixture| fixture.app().clip_restoration_event_panic.is_none());
    assert!(fixture.app().clip_restoration_failed);
    assert_eq!(fixture.bytes(), before);
    assert!(fixture.app().pending_generator_chains.is_empty());
    assert!(fixture.app().retired_clip_restorations.is_empty());
    let running = &fixture.app().running_generator_chains[&channel];
    assert_eq!(
        (
            running.project_session,
            running.endpoint_id,
            running.instance_id
        ),
        identity
    );
    assert!(std::ptr::eq(exact_guard, &running.guard));
    assert!(!running.control.stats().stopped);
    assert_eq!(running.control.stats().detached_workers, 0);
    let removal = &fixture.app().pending_generator_removals[&channel];
    assert_eq!(
        (
            removal.project_session,
            removal.endpoint_id,
            removal.plugin_instance_id
        ),
        (identity.0, Some(identity.1), Some(identity.2))
    );
    assert_receipt_panic_keeps_admission_closed(&mut fixture);
    assert!(std::ptr::eq(
        exact_guard,
        &fixture.app().running_generator_chains[&channel].guard
    ));
    // A consumed receipt cannot justify dropping its still-unjoined App owner.
    // Fixture's retained-join cleanup owns it even after the callback removed it.
}

fn recovery_refusal_fixture(restored: bool) -> Fixture {
    let mut fixture = converted_fixture_with_mock_restoration();
    let channel = fixture.app().project.channels[0].id;
    fixture.app_mut().undo();
    if restored {
        fixture.until(|fixture| !fixture.app().clip_restoration_playback_pending);
        assert_exclusive_compiled_source(fixture.app(), false);
        assert!(fixture.app().pending_clip_restorations.is_empty());
        assert!(
            fixture
                .app()
                .running_generator_chains
                .contains_key(&channel)
        );
    } else {
        wait_for_held_restoration_ready(&fixture, channel);
        assert!(fixture.app().running_generator_chains.is_empty());
        assert!(
            fixture
                .app()
                .pending_clip_restorations
                .contains_key(&channel)
        );
    }
    assert!(fixture.app().midi_input.state.is_disconnected());
    fixture.app_mut().recovery_available = true;
    fixture.app_mut().toast = None;
    fixture
}

fn retained_recovery_source_guard(fixture: &Fixture, restored: bool) -> &PluginWorkerGuard {
    let channel = fixture.app().project.channels[0].id;
    if restored {
        &fixture.app().running_generator_chains[&channel].guard
    } else {
        &fixture.app().pending_clip_restorations[&channel]
            .chain
            .as_ref()
            .unwrap()
            .guard
    }
}

fn assert_recovery_refusal_preserves_source(
    fixture: &Fixture,
    restored: bool,
    exact_guard: *const PluginWorkerGuard,
    before: &[u8],
    session: u64,
    retained_bytes: usize,
    retained_entries: usize,
) {
    assert_eq!(fixture.bytes(), before);
    assert_eq!(fixture.app().project_session, session);
    assert_eq!(
        fixture.app().audio.as_ref().unwrap().project_session(),
        session
    );
    assert_eq!(fixture.app().retained_clip_pcm_bytes, retained_bytes);
    assert_eq!(fixture.app().retained_clip_media.len(), retained_entries);
    assert!(fixture.app().recovery_available);
    assert!(fixture.app().project_lifecycle.is_idle());
    assert!(std::ptr::eq(
        exact_guard,
        retained_recovery_source_guard(fixture, restored)
    ));
    let message = &fixture.app().toast.as_ref().expect("refusal notice").0;
    assert!(
        !message.contains("Autosave restored"),
        "refused recovery reported success: {message}"
    );
    assert!(!fixture.app().playing);
}

#[test]
fn actual_direct_recovery_refusal_keeps_recovery_available_with_pending_or_bound_source() {
    for restored in [false, true] {
        let mut fixture = recovery_refusal_fixture(restored);
        let before = fixture.bytes();
        let session = fixture.app().project_session;
        let retained_bytes = fixture.app().retained_clip_pcm_bytes;
        let retained_entries = fixture.app().retained_clip_media.len();
        let exact_guard =
            retained_recovery_source_guard(&fixture, restored) as *const PluginWorkerGuard;
        let mut recovery = Project::blank();
        recovery.name = "Recovery candidate must remain unpublished".into();
        assert!(!fixture.app_mut().finish_recovery_project(recovery));
        assert_recovery_refusal_preserves_source(
            &fixture,
            restored,
            exact_guard,
            &before,
            session,
            retained_bytes,
            retained_entries,
        );
        assert!(fixture.app().deferred_recovery_project_after_midi.is_none());
    }
}

#[test]
fn actual_deferred_recovery_refusal_keeps_recovery_available_with_pending_or_bound_source() {
    for restored in [false, true] {
        let mut fixture = recovery_refusal_fixture(restored);
        let before = fixture.bytes();
        let session = fixture.app().project_session;
        let retained_bytes = fixture.app().retained_clip_pcm_bytes;
        let retained_entries = fixture.app().retained_clip_media.len();
        let exact_guard =
            retained_recovery_source_guard(&fixture, restored) as *const PluginWorkerGuard;
        let mut recovery = Project::blank();
        recovery.name = "Deferred recovery candidate must remain unpublished".into();
        // This is the real post-disconnect intent slot. No MIDI device or
        // fabricated callback receipt is needed once its state is Disconnected.
        fixture.app_mut().deferred_recovery_project_after_midi = Some(recovery);
        fixture.app_mut().drive_actions_after_midi_disconnect();
        assert!(fixture.app().deferred_recovery_project_after_midi.is_none());
        assert_recovery_refusal_preserves_source(
            &fixture,
            restored,
            exact_guard,
            &before,
            session,
            retained_bytes,
            retained_entries,
        );
    }
}

#[test]
fn actual_open_and_new_refusals_do_not_report_success_or_replace_the_session() {
    let assert_refused =
        |fixture: &Fixture, before: &[u8], session, saved_path: &Option<PathBuf>| {
            assert_eq!(fixture.bytes(), before);
            assert_eq!(fixture.app().project_session, session);
            assert_eq!(&fixture.app().saved_path, saved_path);
            let notice = &fixture.app().toast.as_ref().expect("refusal notice").0;
            assert!(
                !notice.starts_with("Opened "),
                "refused Open reported success: {notice}"
            );
            assert!(
                !notice.contains("Created a new blank project"),
                "refused New reported success: {notice}"
            );
        };
    for restored in [false, true] {
        let mut fixture = recovery_refusal_fixture(restored);
        fixture.app_mut().saved_path = Some("/fixture-only/original-project.citrus".into());
        let before = fixture.bytes();
        let session = fixture.app().project_session;
        let saved_path = fixture.app().saved_path.clone();
        let exact_guard =
            retained_recovery_source_guard(&fixture, restored) as *const PluginWorkerGuard;
        // The candidate is already loaded. This exercises the actual completion
        // method without opening a dialog or reading an external project file.
        assert!(!fixture.app_mut().finish_open_project(
            Project::blank(),
            "/fixture-only/rejected-open.citrus".into()
        ));
        assert_refused(&fixture, &before, session, &saved_path);
        assert!(std::ptr::eq(
            exact_guard,
            retained_recovery_source_guard(&fixture, restored)
        ));
        if !restored {
            assert!(fixture.app().clip_restoration_resources_pending());
            fixture.app_mut().toast = None;
            fixture.app_mut().commit_project_intent(ProjectIntent::New);
            assert_refused(&fixture, &before, session, &saved_path);
            assert!(std::ptr::eq(
                exact_guard,
                retained_recovery_source_guard(&fixture, false)
            ));
            assert!(!fixture.app().clip_session_retirement_pending());
        }
    }
    let mut fixture = Fixture::new();
    fixture.begin();
    assert!(fixture.app().clip_render_fence_active());
    let before = fixture.bytes();
    let session = fixture.app().project_session;
    let saved_path = fixture.app().saved_path.clone();
    fixture.app_mut().toast = None;
    fixture.app_mut().commit_project_intent(ProjectIntent::New);
    assert_refused(&fixture, &before, session, &saved_path);
    assert!(fixture.app().clip_render_fence_active());
    assert!(!fixture.app().clip_session_retirement_pending());
}
