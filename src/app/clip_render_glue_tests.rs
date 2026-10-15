//! Source-only tests of actual App admission methods and restoration ownership.
//! The callback and identified worker are device-free. Positive restoration tests
//! prove App ownership/admission with an explicitly mock factory; they make no
//! claim that an SDK restoration took place. The runtime adapter has its own tests.
//! No helper, device, native editor, or genuine plug-in is invoked here.
use super::*;
#[cfg(any(target_os = "linux", windows))]
use crate::app::clip_render_job::ClipRenderPhase;
use crate::app::clip_render_job::tests::{FIXTURE_POISONED, Fixture, fixture_admission};
use crate::instrument_bounce::{BounceAudioBus, BouncePluginState};
use crate::plugins::plugin_runtime::{PluginPrepareConfig, test_capture_source::TestCaptureSource};
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicUsize, Ordering},
};

// Read-only test identity and failure diagnostics for device-free qualification.
// Borrow retained fields/publication only; never service an owner or reset time.
#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
impl PendingClipRestoration {
    pub(in crate::app) fn genuine_test_candidate(&self) -> Option<&Project> {
        self.candidate.as_ref()
    }

    pub(in crate::app) fn genuine_test_chain_audio_worker(&self) -> Option<usize> {
        self.chain
            .as_ref()
            .map(|chain| chain.audio.genuine_snapshot().worker_id)
    }

    pub(in crate::app) fn genuine_diagnostic_ready(&self) -> bool {
        self.ready
    }

    pub(in crate::app) fn genuine_diagnostic_started_at(&self) -> Instant {
        self.started_at
    }

    pub(in crate::app) fn genuine_diagnostic_chain_workers(&self) -> Option<(usize, usize)> {
        self.chain.as_ref().map(|chain| {
            (
                chain.control.genuine_worker_id(),
                chain.guard.genuine_worker_id(),
            )
        })
    }

    pub(in crate::app) fn genuine_diagnostic_transferred_workers(
        &self,
    ) -> (Option<usize>, Option<usize>) {
        (
            self.transferred_control
                .as_ref()
                .map(PluginChainControl::genuine_worker_id),
            self.transferred_guard
                .as_ref()
                .map(PluginWorkerGuard::genuine_worker_id),
        )
    }
}

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
    type ModelAction = (&'static str, fn(&mut CitrusApp));
    let actions: [ModelAction; 12] = [
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

#[cfg(any(target_os = "linux", windows))]
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

pub(in crate::app) fn source_state() -> BouncePluginState {
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
        vst3_catalog: None,
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
        any(target_os = "linux", windows),
        feature = "experimental-fresh-helper"
    )) {
        app.record_fresh_clip_source_for_new_load(42, &descriptor)
            .unwrap();
        assert_eq!(
            app.fresh_bounce_sources.get(&42),
            Some(&(
                descriptor.id.clone(),
                descriptor.path.clone(),
                FreshSourceClass::Exact(source_state().metadata.class_uid),
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

fn fresh_catalog_descriptor(project: &Project) -> PluginDescriptor {
    let mut descriptor = descriptor(project);
    let uid = descriptor.vst3_metadata.take().unwrap().class_uid;
    descriptor.verified = false;
    descriptor.is_instrument = false;
    descriptor.category = "Unknown".into();
    descriptor.vst3_catalog = Some(plugins::Vst3FactoryCatalog {
        factory: plugins::Vst3FactoryInfo {
            vendor: "Declared vendor".into(),
            url: String::new(),
            email: String::new(),
            flags: 0,
        },
        classes: vec![plugins::Vst3FactoryClass {
            class_id: uid,
            name: "Expected UID declaration only".into(),
            category: "Audio Module Class".into(),
            cardinality: i32::MAX,
            vendor: None,
            version: None,
            sdk_version: None,
            sub_categories: Some(vec!["Instrument".into()]),
            class_flags: None,
        }],
    });
    let mut second = descriptor.vst3_catalog.as_ref().unwrap().classes[0].clone();
    second.class_id = "00112233445566778899AABBCCDDEEFF".into();
    descriptor
        .vst3_catalog
        .as_mut()
        .unwrap()
        .classes
        .push(second);
    descriptor
}

#[test]
fn fresh_source_opt_in_catalog_records_default_request_without_choosing_a_declared_class() {
    let mut app = bare_app();
    app.project = source_project();
    let descriptor = fresh_catalog_descriptor(&app.project);
    assert!(descriptor.is_catalog_only());
    let before = project_and_history(&app);
    let declared = serde_json::to_value(&descriptor).unwrap();
    if cfg!(all(
        any(target_os = "linux", windows),
        feature = "experimental-fresh-helper"
    )) {
        app.record_fresh_clip_source_for_new_load(42, &descriptor)
            .unwrap();
        assert_eq!(
            app.fresh_bounce_sources[&42],
            (
                descriptor.id.clone(),
                descriptor.path.clone(),
                FreshSourceClass::LoadedDefault
            )
        );
        // This is a new-load request, never a class selection or Ready proof.
        assert!(
            !app.fresh_bounce_sources
                .values()
                .any(|(_, _, class)| matches!(class, FreshSourceClass::Exact(_)))
        );
    } else {
        assert!(
            app.record_fresh_clip_source_for_new_load(42, &descriptor)
                .is_err()
        );
        assert!(app.fresh_bounce_sources.is_empty());
    }
    assert_eq!(project_and_history(&app), before);
    assert_eq!(serde_json::to_value(&descriptor).unwrap(), declared);
    assert!(app.pending_generator_chains.is_empty() && app.running_generator_chains.is_empty());
}

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
#[test]
fn fresh_source_catalog_default_intent_refuses_explicit_class_or_non_dry_fallback() {
    let mut app = bare_app();
    app.project = source_project();
    let catalog = fresh_catalog_descriptor(&app.project);
    app.record_fresh_clip_source_for_new_load(42, &catalog)
        .unwrap();
    let before = project_and_history(&app);
    let intent = app.fresh_bounce_sources[&42].clone();
    // Pure pre-spawn refusal matrix: the fixture path is never opened. A
    // fallback to ordinary loading would produce a missing-file error instead.
    for case in 0..4 {
        let mut spec = PluginLoadSpec::from_descriptor(catalog.clone());
        match case {
            0 => spec.class_uid = Some(source_state().metadata.class_uid),
            1 => spec.enabled = false,
            2 => spec.bypassed = true,
            _ => spec.wet = 0.5,
        }
        let reason = app
            .spawn_clip_bound_source(42, spec, &app.project, 48_000.0)
            .err()
            .expect("explicit fresh intent cannot be silently downgraded");
        assert!(
            reason.contains(if case == 0 {
                "explicit class request"
            } else {
                "cannot fall back to ordinary loading"
            }),
            "{reason}"
        );
        assert_eq!(app.fresh_bounce_sources[&42], intent);
        assert_eq!(project_and_history(&app), before);
        assert!(app.pending_generator_chains.is_empty() && app.running_generator_chains.is_empty());
    }
}

/// The only fixture that creates an extra restoration worker owns every location
/// to which its exact guard can move. It is installed before spawning that worker.
struct RestorationFixture {
    app: Option<Box<CitrusApp>>,
    callback: Option<audio::TestAudioCallback>,
    source: Option<TestCaptureSource>,
    fresh_source: Option<TestCaptureSource>,
    untransferred: Option<PluginChain>,
    removed_restoration: Option<PendingClipRestoration>,
    admission: Option<std::sync::MutexGuard<'static, ()>>,
}

struct RetainedRestorationFailure {
    _app: Option<Box<CitrusApp>>,
    _callback: Option<audio::TestAudioCallback>,
    _source: Option<TestCaptureSource>,
    _fresh_source: Option<TestCaptureSource>,
    _untransferred: Option<PluginChain>,
    _removed_restoration: Option<PendingClipRestoration>,
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
        Self::with_max_block_frames(MAX_MIXER_BLOCK_FRAMES)
    }

    fn with_max_block_frames(max_block_frames: usize) -> Self {
        Self::with_preparation(max_block_frames, false, 41)
    }

    fn with_preparation(max_block_frames: usize, held: bool, instance_id: u64) -> Self {
        Self::with_preparation_identity(max_block_frames, held, instance_id, false)
    }

    fn with_preparation_identity(
        max_block_frames: usize,
        held: bool,
        instance_id: u64,
        loaded_identity: bool,
    ) -> Self {
        let admission = fixture_admission();
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
            fresh_source: None,
            untransferred: None,
            removed_restoration: None,
            admission: Some(admission),
        };
        let config = PluginPrepareConfig {
            sample_rate: 48_000.0,
            max_block_frames,
        };
        let loaded_identity = loaded_identity.then(|| {
            crate::plugins::plugin_runtime::LoadedVst3Identity::test_fixture(
                fixture.app().project.plugin_instances[0].path.clone(),
                &source_state().metadata.class_uid,
                &source_state().metadata.version,
                "Instrument",
                true,
                false,
            )
        });
        #[cfg(all(
            any(target_os = "linux", windows),
            feature = "experimental-fresh-helper"
        ))]
        let created = if let Some(identity) = loaded_identity {
            TestCaptureSource::new_with_loaded_identity(
                instance_id,
                source_state(),
                config,
                identity,
                held,
            )
        } else if held {
            TestCaptureSource::new_with_held_prepare(instance_id, source_state(), config)
        } else {
            TestCaptureSource::new(instance_id, source_state(), config)
        };
        #[cfg(not(all(
            any(target_os = "linux", windows),
            feature = "experimental-fresh-helper"
        )))]
        let created = {
            assert!(!held, "held preparation requires the existing test profile");
            if let Some(identity) = loaded_identity {
                TestCaptureSource::new_with_loaded_identity(
                    instance_id,
                    source_state(),
                    config,
                    identity,
                    false,
                )
            } else {
                TestCaptureSource::new(instance_id, source_state(), config)
            }
        };
        let (chain, source) = created.unwrap();
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
        if let Some(source) = &self.fresh_source {
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
            if let Some(restoration) = &mut self.removed_restoration {
                if let Some(chain) = &mut restoration.chain {
                    poll(&mut chain.guard);
                }
                if let Some(guard) = &mut restoration.transferred_guard {
                    poll(guard);
                }
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
            for guard in &mut app.retired_fresh_source_candidates {
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
        if self.app.is_none() {
            // Forced-close tests transferred guards to the production shutdown
            // owner before taking the App. Releasing gates still permits join
            // if a test assertion unwinds before its explicit release.
            if let Some(source) = &self.source {
                source.release_gates();
            }
            if let Some(source) = &self.fresh_source {
                source.release_gates();
            }
            return;
        }
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
                _fresh_source: self.fresh_source.take(),
                _untransferred: self.untransferred.take(),
                _removed_restoration: self.removed_restoration.take(),
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
            // Returning normally from Drop preserves the original assertion panic.
        }
    }
}

fn restoration_owner_ids(pending: &PendingClipRestoration) -> (usize, usize, usize) {
    let chain = pending.chain.as_ref().expect("exact source still retained");
    (
        chain.audio.genuine_snapshot().worker_id,
        chain.control.genuine_worker_id(),
        chain.guard.genuine_worker_id(),
    )
}

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
#[test]
fn actual_restoration_retains_its_unique_worker_ready_identity_through_installed_receipt() {
    let mut fixture =
        RestorationFixture::with_preparation_identity(MAX_MIXER_BLOCK_FRAMES, false, 41, true);
    let channel = fixture.app().project.channels[0].id;
    let binding = fixture
        .app_mut()
        .prepare_clip_render_restoration(41, &source_state().metadata)
        .unwrap();
    fixture.app_mut().install_clip_render_restoration(binding);
    fixture.wait_for_actual_worker_snapshot();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !fixture
        .app()
        .pending_generator_chains
        .contains_key(&channel)
    {
        fixture.app_mut().poll_clip_restorations();
        assert!(!fixture.app().clip_restoration_failed);
        assert!(
            Instant::now() < deadline,
            "actual Ready did not transfer its owner"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(fixture.app().pending_clip_restorations.is_empty());
    assert!(matches!(
        &fixture.app().pending_generator_chains[&channel].readiness,
        GeneratorReadiness::Ready(Some(_), None)
    ));
    assert!(
        fixture.app().loaded_generator_identity(channel).is_none(),
        "Ready alone never substitutes for callback Installed"
    );
    let before = project_and_history(fixture.app());
    let deadline = Instant::now() + Duration::from_secs(5);
    while !fixture
        .app()
        .running_generator_chains
        .contains_key(&channel)
    {
        fixture
            .callback
            .as_mut()
            .unwrap()
            .pump(fixture.app.as_ref().unwrap().audio.as_ref().unwrap(), 128);
        fixture.app_mut().poll_generator_endpoint_events();
        assert!(!fixture.app().clip_restoration_failed);
        assert!(
            Instant::now() < deadline,
            "actual Installed did not transfer its owner"
        );
    }
    let proof = fixture
        .app()
        .loaded_generator_identity(channel)
        .expect("restoration already consumed the one real Ready before transfer");
    assert!(proof.matches_current(fixture.app()));
    assert_eq!(
        proof.identity().class_uid(),
        source_state().metadata.class_uid
    );
    assert_eq!(proof.identity().version(), source_state().metadata.version);
    assert_eq!(project_and_history(fixture.app()), before);
}

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
fn ordinary_source_before_fresh_candidate() -> RestorationFixture {
    let mut fixture = RestorationFixture::new();
    fixture.wait_for_actual_worker_snapshot();
    let channel = fixture.app().project.channels[0].id;
    let mut pending = fixture
        .app_mut()
        .pending_clip_restorations
        .remove(&channel)
        .unwrap();
    let chain = pending.chain.take().unwrap();
    fixture.untransferred = Some(chain);
    fixture.app_mut().clip_restoration_playback_pending = false;
    let candidate = fixture.app().project.clone();
    let previous = snapshot_channel_generator(&candidate, channel);
    let chain = fixture.untransferred.take().unwrap();
    fixture
        .app_mut()
        .queue_channel_generator_candidate_prepared(channel, candidate, previous, Some(chain))
        .unwrap();
    fixture
        .callback
        .as_mut()
        .unwrap()
        .pump(fixture.app.as_ref().unwrap().audio.as_ref().unwrap(), 128);
    fixture.app_mut().poll_generator_endpoint_events();
    assert_eq!(
        fixture.app().running_generator_chains[&channel].instance_id,
        41
    );
    assert!(fixture.app().pending_generator_chains.is_empty());
    fixture
}

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
fn prepare_explicit_fresh_candidate(fixture: &mut RestorationFixture, held: bool) -> Project {
    let descriptor = descriptor(&fixture.app().project);
    fixture
        .app_mut()
        .record_fresh_clip_source_for_new_load(42, &descriptor)
        .unwrap();
    // Test ownership capacity is prepared before spawning the actual worker.
    fixture
        .app_mut()
        .retired_fresh_source_candidates
        .try_reserve(2)
        .unwrap();
    let mut candidate = fixture.app().project.clone();
    let mut instance = candidate.plugin_instances[0].clone();
    instance.id = 42;
    candidate.plugin_instances.push(instance);
    candidate.channels[0].instrument_plugin_instance_id = Some(42);
    let config = PluginPrepareConfig {
        sample_rate: 48_000.0,
        max_block_frames: MAX_MIXER_BLOCK_FRAMES,
    };
    let (chain, source) = if held {
        TestCaptureSource::new_with_held_prepare(42, source_state(), config)
    } else {
        TestCaptureSource::new(42, source_state(), config)
    }
    .unwrap();
    fixture.untransferred = Some(chain);
    fixture.fresh_source = Some(source);
    candidate
}

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
fn source_join_outcomes(
    observer: &crate::genuine_app_observer::ObservationRun,
    worker: usize,
) -> Vec<crate::genuine_app_observer::WorkerJoinOutcome> {
    use crate::genuine_app_observer::RecordKind;
    observer.with_snapshot(|snapshot| {
        assert!(snapshot.failure.is_none(), "{:?}", snapshot.failure);
        for record in &snapshot.records {
            if let RecordKind::Worker {
                worker_id, stats, ..
            } = &record.kind
                && *worker_id == worker
            {
                assert_eq!(stats.detached_workers, 0, "actual fresh worker detached");
            }
        }
        snapshot
            .records
            .iter()
            .filter_map(|record| match &record.kind {
                RecordKind::WorkerJoin { worker_id, outcome } if *worker_id == worker => {
                    Some(*outcome)
                }
                _ => None,
            })
            .collect()
    })
}

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
fn drain_actual_fresh_candidate(
    fixture: &mut RestorationFixture,
    observer: &crate::genuine_app_observer::ObservationRun,
    worker: usize,
) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !fixture.app().retired_fresh_source_candidates.is_empty() {
        fixture.app_mut().poll_clip_restorations();
        assert!(Instant::now() < deadline, "exact fresh worker did not join");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        source_join_outcomes(observer, worker),
        [crate::genuine_app_observer::WorkerJoinOutcome::Joined]
    );
}

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
#[test]
fn actual_fresh_source_install_enqueue_rejection_retains_late_worker_until_join() {
    let mut fixture = ordinary_source_before_fresh_candidate();
    let channel = fixture.app().project.channels[0].id;
    let old_worker = fixture.app().running_generator_chains[&channel]
        .guard
        .genuine_worker_id();
    let old_endpoint = fixture.app().running_generator_chains[&channel].endpoint_id;
    let candidate = prepare_explicit_fresh_candidate(&mut fixture, true);
    let fresh_worker = fixture
        .untransferred
        .as_ref()
        .unwrap()
        .guard
        .genuine_worker_id();
    let before = project_and_history(fixture.app());
    let callback_before = fixture.callback.as_ref().unwrap().genuine_snapshot();
    fill_ordinary_ring_and_prove_stop_all_rejected(fixture.app_mut());
    let observer = crate::genuine_app_observer::ObservationRun::install().unwrap();
    let previous = snapshot_channel_generator(&fixture.app().project, channel);
    let chain = fixture.untransferred.take().unwrap();
    let error = fixture
        .app_mut()
        .queue_channel_generator_candidate_prepared(channel, candidate, previous, Some(chain))
        .unwrap_err();
    assert!(error.contains("installation queue is full"));
    assert!(fixture.app().pending_generator_chains.is_empty());
    assert_eq!(fixture.app().retired_fresh_source_candidates.len(), 1);
    for _ in 0..3 {
        fixture.app_mut().poll_clip_restorations();
        assert_eq!(
            fixture.app().retired_fresh_source_candidates[0].genuine_worker_id(),
            fresh_worker
        );
        assert!(source_join_outcomes(&observer, fresh_worker).is_empty());
        assert_eq!(project_and_history(fixture.app()), before);
    }
    assert!(!fixture.app().clip_restoration_failed);
    assert!(!fixture.app().clip_restoration_playback_pending);
    assert!(fixture.app().clip_restoration_resources_pending());
    assert_eq!(
        fixture.app().running_generator_chains[&channel]
            .guard
            .genuine_worker_id(),
        old_worker
    );
    assert_eq!(
        fixture.app().running_generator_chains[&channel].endpoint_id,
        old_endpoint
    );
    let callback_after = fixture.callback.as_ref().unwrap().genuine_snapshot();
    assert_eq!(callback_after.device_frame, callback_before.device_frame);
    assert_eq!(callback_after.endpoints[0].endpoint_id, old_endpoint);
    fixture.fresh_source.as_ref().unwrap().release_prepare();
    drain_actual_fresh_candidate(&mut fixture, &observer, fresh_worker);
    assert!(!fixture.app().clip_restoration_resources_pending());
    assert_eq!(project_and_history(fixture.app()), before);
}

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
#[test]
fn actual_fresh_source_callback_fault_install_refusal_keeps_old_source_and_joins_failed_worker() {
    let mut fixture = ordinary_source_before_fresh_candidate();
    let channel = fixture.app().project.channels[0].id;
    let old_endpoint = fixture.app().running_generator_chains[&channel].endpoint_id;
    let old_worker = fixture.app().running_generator_chains[&channel]
        .guard
        .genuine_worker_id();
    let candidate = prepare_explicit_fresh_candidate(&mut fixture, false);
    let fresh_worker = fixture
        .untransferred
        .as_ref()
        .unwrap()
        .guard
        .genuine_worker_id();
    let deadline = Instant::now() + Duration::from_secs(5);
    while fixture
        .untransferred
        .as_ref()
        .unwrap()
        .control
        .plugin_endpoint_snapshot()
        .is_none()
    {
        assert!(
            Instant::now() < deadline,
            "fresh mock did not actually prepare"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let observer = crate::genuine_app_observer::ObservationRun::install().unwrap();
    fixture
        .fresh_source
        .as_ref()
        .unwrap()
        .fail_capture_after_gate("actual fresh candidate backend fault".into());
    let control = &fixture.untransferred.as_ref().unwrap().control;
    let receiver = control
        .request_instrument_bounce_state(42, control.stats().current_epoch, vec![17])
        .unwrap();
    loop {
        match receiver.try_recv() {
            Ok(Err(crate::instrument_bounce::BounceCaptureError::BackendFailed(_))) => break,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                assert!(
                    Instant::now() < deadline,
                    "actual fresh fault did not return"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
            other => panic!("unexpected actual capture result: {other:?}"),
        }
    }
    assert!(receiver.termination_acknowledged());
    assert!(control.stats().faults > 0);
    let before = project_and_history(fixture.app());
    let previous = snapshot_channel_generator(&fixture.app().project, channel);
    let chain = fixture.untransferred.take().unwrap();
    fixture
        .app_mut()
        .queue_channel_generator_candidate_prepared(channel, candidate, previous, Some(chain))
        .unwrap();
    let fresh_endpoint = fixture.app().pending_generator_chains[&channel].endpoint_id;
    fixture
        .callback
        .as_mut()
        .unwrap()
        .pump(fixture.app.as_ref().unwrap().audio.as_ref().unwrap(), 128);
    fixture.app_mut().poll_generator_endpoint_events();
    observer.with_snapshot(|snapshot| {
        let refused = snapshot.records.iter().filter(|record| matches!(&record.kind,
            crate::genuine_app_observer::RecordKind::AppGeneratorEndpointEvent(
                GeneratorEndpointEvent::Installed { endpoint_id, plugin_instance_id: 42, success: false, .. }
            ) if *endpoint_id == fresh_endpoint
        )).count();
        assert_eq!(refused, 1, "the production callback must emit the actual refusal");
    });
    assert!(fixture.app().pending_generator_chains.is_empty());
    assert_eq!(fixture.app().retired_fresh_source_candidates.len(), 1);
    assert_eq!(
        fixture.app().retired_fresh_source_candidates[0].genuine_worker_id(),
        fresh_worker
    );
    assert_eq!(project_and_history(fixture.app()), before);
    assert_eq!(
        fixture.app().running_generator_chains[&channel].endpoint_id,
        old_endpoint
    );
    assert_eq!(
        fixture.app().running_generator_chains[&channel]
            .guard
            .genuine_worker_id(),
        old_worker
    );
    assert_eq!(
        fixture
            .callback
            .as_ref()
            .unwrap()
            .genuine_snapshot()
            .endpoints[0]
            .endpoint_id,
        old_endpoint
    );
    assert!(!fixture.app().clip_restoration_failed);
    assert!(!fixture.app().clip_restoration_playback_pending);
    drain_actual_fresh_candidate(&mut fixture, &observer, fresh_worker);
    assert_eq!(project_and_history(fixture.app()), before);
}

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
#[test]
fn actual_fresh_source_shutdown_panic_is_reported_as_panicked_join_without_restoration_lock() {
    let mut fixture = ordinary_source_before_fresh_candidate();
    let candidate = prepare_explicit_fresh_candidate(&mut fixture, true);
    let channel = candidate.channels[0].id;
    fixture.fresh_source.as_ref().unwrap().panic_on_shutdown();
    let worker = fixture
        .untransferred
        .as_ref()
        .unwrap()
        .guard
        .genuine_worker_id();
    let before = project_and_history(fixture.app());
    fill_ordinary_ring_and_prove_stop_all_rejected(fixture.app_mut());
    let observer = crate::genuine_app_observer::ObservationRun::install().unwrap();
    let chain = fixture.untransferred.take().unwrap();
    assert!(
        fixture
            .app_mut()
            .queue_channel_generator_candidate_prepared(channel, candidate, None, Some(chain))
            .is_err()
    );
    fixture.app_mut().poll_clip_restorations();
    assert_eq!(fixture.app().retired_fresh_source_candidates.len(), 1);
    assert!(source_join_outcomes(&observer, worker).is_empty());
    fixture.fresh_source.as_ref().unwrap().release_prepare();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !fixture.app().retired_fresh_source_candidates.is_empty() {
        fixture.app_mut().poll_clip_restorations();
        assert!(
            Instant::now() < deadline,
            "panicking fresh worker was not actually joined"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        source_join_outcomes(&observer, worker),
        [crate::genuine_app_observer::WorkerJoinOutcome::WorkerPanicked]
    );
    assert!(
        fixture
            .app()
            .toast
            .as_ref()
            .unwrap()
            .0
            .contains("shutdown panic")
    );
    assert!(!fixture.app().clip_restoration_failed);
    assert!(!fixture.app().clip_restoration_playback_pending);
    assert_eq!(project_and_history(fixture.app()), before);
}

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
#[test]
fn actual_fresh_source_forced_app_drop_retains_late_worker_for_background_join_once() {
    let mut fixture = RestorationFixture::with_preparation(MAX_MIXER_BLOCK_FRAMES, true, 42);
    let channel = fixture.app().project.channels[0].id;
    let mut pending = fixture
        .app_mut()
        .pending_clip_restorations
        .remove(&channel)
        .unwrap();
    let PluginChain {
        audio,
        control,
        mut guard,
    } = pending.chain.take().unwrap();
    fixture
        .app_mut()
        .retired_fresh_source_candidates
        .try_reserve(1)
        .unwrap();
    guard.mark_fresh_retained_shutdown();
    let descriptor = descriptor(&fixture.app().project);
    fixture
        .app_mut()
        .record_fresh_clip_source_for_new_load(42, &descriptor)
        .unwrap();
    let worker = guard.genuine_worker_id();
    let observer = crate::genuine_app_observer::ObservationRun::install().unwrap();
    fixture.app_mut().retire_failed_clip_candidate(42, guard);
    drop(audio);
    assert_eq!(fixture.app().retired_fresh_source_candidates.len(), 1);
    fixture.callback = None;
    let app = fixture.app.take().unwrap();
    drop(app);
    for _ in 0..3 {
        assert_eq!(control.stats().detached_workers, 0);
        assert!(source_join_outcomes(&observer, worker).is_empty());
        std::thread::sleep(Duration::from_millis(10));
    }
    fixture.source.as_ref().unwrap().release_prepare();
    let deadline = Instant::now() + Duration::from_secs(5);
    while source_join_outcomes(&observer, worker).is_empty() {
        assert!(
            Instant::now() < deadline,
            "forced App close lost its exact late worker owner"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        source_join_outcomes(&observer, worker),
        [crate::genuine_app_observer::WorkerJoinOutcome::Joined]
    );
    assert_eq!(control.stats().detached_workers, 0);
    assert!(control.stats().stopped);
}

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
fn actually_installed_fresh_source() -> RestorationFixture {
    let mut fixture = RestorationFixture::with_preparation(MAX_MIXER_BLOCK_FRAMES, false, 42);
    let channel = fixture.app().project.channels[0].id;
    let deadline = Instant::now() + Duration::from_secs(5);
    while fixture.app().pending_clip_restorations[&channel]
        .chain
        .as_ref()
        .unwrap()
        .control
        .plugin_endpoint_snapshot()
        .is_none()
    {
        assert!(
            Instant::now() < deadline,
            "actual fresh worker did not prepare"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let descriptor = descriptor(&fixture.app().project);
    fixture
        .app_mut()
        .record_fresh_clip_source_for_new_load(42, &descriptor)
        .unwrap();
    let mut candidate = fixture.app().project.clone();
    candidate.channels[0].instrument_plugin_instance_id = Some(42);
    candidate.plugin_instances[0].id = 42;
    let previous = snapshot_channel_generator(&fixture.app().project, channel);
    let mut pending = fixture
        .app_mut()
        .pending_clip_restorations
        .remove(&channel)
        .unwrap();
    fixture.untransferred = pending.chain.take();
    fixture.app_mut().clip_restoration_playback_pending = false;
    let chain = fixture.untransferred.take().unwrap();
    fixture
        .app_mut()
        .queue_channel_generator_candidate_prepared(channel, candidate, previous, Some(chain))
        .unwrap();
    // The production admission path reserved this running owner's eventual
    // retirement capacity before transferring it to the actual callback.
    assert!(fixture.app().retired_fresh_source_candidates.capacity() >= 1);
    fixture
        .callback
        .as_mut()
        .unwrap()
        .pump(fixture.app.as_ref().unwrap().audio.as_ref().unwrap(), 128);
    fixture.app_mut().poll_generator_endpoint_events();
    assert!(fixture.app().pending_generator_chains.is_empty());
    let running = &fixture.app().running_generator_chains[&channel];
    assert_eq!(running.instance_id, 42);
    assert_eq!(running.control.stats().detached_workers, 0);
    assert_eq!(
        fixture
            .callback
            .as_ref()
            .unwrap()
            .genuine_snapshot()
            .endpoints[0]
            .endpoint_id,
        running.endpoint_id
    );
    fixture
}

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
fn pause_actual_running_fresh_capture(
    fixture: &RestorationFixture,
) -> crate::plugins::plugin_runtime::BounceStateReceiver {
    let channel = fixture.app().project.channels[0].id;
    let source = fixture.source.as_ref().unwrap();
    let before = source.capture_count();
    source.pause_capture();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let control = &fixture.app().running_generator_chains[&channel].control;
        let receiver = control
            .request_instrument_bounce_state(42, control.stats().current_epoch, vec![17])
            .unwrap();
        loop {
            if source.capture_count() > before {
                assert!(
                    !receiver.termination_acknowledged(),
                    "held actual capture cannot have a terminal ACK"
                );
                return receiver;
            }
            assert!(
                Instant::now() < deadline,
                "actual running fresh worker never entered held capture"
            );
            if receiver.termination_acknowledged() {
                match receiver.try_recv() {
                    Ok(Err(crate::instrument_bounce::BounceCaptureError::Rejected(_))) => break,
                    other => panic!("unexpected capture before its actual gate: {other:?}"),
                }
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
#[test]
fn actual_fresh_source_running_engine_loss_retains_late_worker_until_join_without_model_change() {
    let mut fixture = actually_installed_fresh_source();
    let channel = fixture.app().project.channels[0].id;
    let worker = fixture.app().running_generator_chains[&channel]
        .guard
        .genuine_worker_id();
    let before = project_and_history(fixture.app());
    let observer = crate::genuine_app_observer::ObservationRun::install().unwrap();
    let receiver = pause_actual_running_fresh_capture(&fixture);
    // This is the existing engine-loss safe owner point: callback endpoint
    // ownership has been destroyed before the retained source transfer.
    fixture.callback = None;
    drop(fixture.app_mut().audio.take());
    fixture.app_mut().retain_recreated_sources_before_clear();
    assert!(fixture.app().running_generator_chains.is_empty());
    assert_eq!(fixture.app().retired_fresh_source_candidates.len(), 1);
    for _ in 0..3 {
        fixture.app_mut().poll_clip_restorations();
        assert_eq!(
            fixture.app().retired_fresh_source_candidates[0].genuine_worker_id(),
            worker
        );
        assert!(source_join_outcomes(&observer, worker).is_empty());
        assert!(!receiver.termination_acknowledged());
        assert_eq!(project_and_history(fixture.app()), before);
    }
    assert!(!fixture.app().clip_restoration_failed);
    assert!(!fixture.app().clip_restoration_playback_pending);
    fixture.source.as_ref().unwrap().release_gates();
    drain_actual_fresh_candidate(&mut fixture, &observer, worker);
    assert!(receiver.termination_acknowledged());
    assert_eq!(project_and_history(fixture.app()), before);
}

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
#[test]
fn actual_fresh_source_running_forced_app_drop_retains_late_worker_for_join_once() {
    let mut fixture = actually_installed_fresh_source();
    let channel = fixture.app().project.channels[0].id;
    let worker = fixture.app().running_generator_chains[&channel]
        .guard
        .genuine_worker_id();
    let before = project_and_history(fixture.app());
    let observer = crate::genuine_app_observer::ObservationRun::install().unwrap();
    let receiver = pause_actual_running_fresh_capture(&fixture);
    fixture.callback = None;
    drop(fixture.app_mut().audio.take());
    // The exact source is still running in the App at destruction. Only its
    // production Drop handoff can move this JoinHandle to the shutdown owner.
    assert_eq!(
        fixture.app().running_generator_chains[&channel]
            .guard
            .genuine_worker_id(),
        worker
    );
    assert!(fixture.app().retired_fresh_source_candidates.is_empty());
    assert_eq!(project_and_history(fixture.app()), before);
    let app = fixture.app.take().unwrap();
    drop(app);
    for _ in 0..3 {
        assert!(source_join_outcomes(&observer, worker).is_empty());
        assert!(!receiver.termination_acknowledged());
        std::thread::sleep(Duration::from_millis(10));
    }
    fixture.source.as_ref().unwrap().release_gates();
    let deadline = Instant::now() + Duration::from_secs(5);
    while source_join_outcomes(&observer, worker).is_empty() {
        assert!(
            Instant::now() < deadline,
            "forced close lost the actual running fresh owner"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        source_join_outcomes(&observer, worker),
        [crate::genuine_app_observer::WorkerJoinOutcome::Joined]
    );
    assert!(receiver.termination_acknowledged());
}

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
fn unpublished_qualified_fresh_worker() -> (RestorationFixture, Project) {
    let mut fixture = RestorationFixture::with_preparation(MAX_MIXER_BLOCK_FRAMES, false, 42);
    let channel = fixture.app().project.channels[0].id;
    let deadline = Instant::now() + Duration::from_secs(5);
    while fixture.app().pending_clip_restorations[&channel]
        .chain
        .as_ref()
        .unwrap()
        .control
        .plugin_endpoint_snapshot()
        .is_none()
    {
        assert!(
            Instant::now() < deadline,
            "actual unpublished fresh worker did not prepare"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let descriptor = descriptor(&fixture.app().project);
    fixture
        .app_mut()
        .record_fresh_clip_source_for_new_load(42, &descriptor)
        .unwrap();
    fixture
        .app_mut()
        .retired_fresh_source_candidates
        .try_reserve(1)
        .unwrap();
    let mut pending = fixture
        .app_mut()
        .pending_clip_restorations
        .remove(&channel)
        .unwrap();
    fixture.untransferred = pending.chain.take();
    fixture
        .untransferred
        .as_mut()
        .unwrap()
        .guard
        .mark_fresh_retained_shutdown();
    fixture.app_mut().clip_restoration_playback_pending = false;
    let mut candidate = fixture.app().project.clone();
    candidate.channels[0].instrument_plugin_instance_id = Some(42);
    candidate.plugin_instances[0].id = 42;
    (fixture, candidate)
}

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
fn fresh_guard_drop_timeout_diagnostic(
    observer: &crate::genuine_app_observer::ObservationRun,
    worker: usize,
    control: &PluginChainControl,
    source: &TestCaptureSource,
    worker_panics: bool,
) -> String {
    let join = control.genuine_retained_join_observation();
    let backend_drop = source.shutdown_drop_observation();
    let stats = control.stats();
    observer.with_snapshot(|snapshot| {
        use crate::genuine_app_observer::{RecordKind, WorkerJoinOutcome};
        let mut joined = 0;
        let mut panicked = 0;
        let mut owner_started = 0;
        let mut owner_start_failed = 0;
        for record in &snapshot.records {
            match &record.kind {
                RecordKind::WorkerJoin { worker_id, outcome } if *worker_id == worker => {
                    match outcome {
                        WorkerJoinOutcome::Joined => joined += 1,
                        WorkerJoinOutcome::WorkerPanicked => panicked += 1,
                    }
                }
                RecordKind::Lifecycle("fresh guard retained shutdown owner started") => {
                    owner_started += 1;
                }
                RecordKind::Lifecycle(
                    "fresh guard retained shutdown owner start failed; source retained unjoined",
                ) => owner_start_failed += 1,
                _ => {}
            }
        }
        format!(
            "worker_panics={worker_panics}, exact_worker={worker}, backend_drop={backend_drop:?}, last_actual_retained_join={join:?}, observer(failure,records,exact_healthy_joins,exact_panicked_joins)=({:?},{},{},{}), run_owner_markers(started,start_failed)=({owner_started},{owner_start_failed}), worker_stats={stats:?}; stopped/ShutdownComplete are not thread-finished or Join evidence",
            snapshot.failure,
            snapshot.records.len(),
            joined,
            panicked,
        )
    })
}

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
#[test]
fn actual_fresh_source_guard_drop_retains_late_worker_and_records_healthy_or_panicked_join_once() {
    for worker_panics in [false, true] {
        let mut fixture = RestorationFixture::with_preparation(MAX_MIXER_BLOCK_FRAMES, true, 42);
        let channel = fixture.app().project.channels[0].id;
        fixture
            .app_mut()
            .retired_fresh_source_candidates
            .try_reserve(1)
            .unwrap();
        let mut pending = fixture
            .app_mut()
            .pending_clip_restorations
            .remove(&channel)
            .unwrap();
        let PluginChain {
            audio,
            control,
            mut guard,
        } = pending.chain.take().unwrap();
        guard.mark_fresh_retained_shutdown();
        let worker = guard.genuine_worker_id();
        if worker_panics {
            fixture.source.as_ref().unwrap().panic_on_shutdown();
        }
        let before = project_and_history(fixture.app());
        let observer = crate::genuine_app_observer::ObservationRun::install().unwrap();
        drop(audio);
        drop(guard);
        for _ in 0..3 {
            assert_eq!(control.stats().detached_workers, 0);
            assert!(source_join_outcomes(&observer, worker).is_empty());
            std::thread::sleep(Duration::from_millis(10));
        }
        fixture.source.as_ref().unwrap().release_prepare();
        let deadline = Instant::now() + Duration::from_secs(5);
        while source_join_outcomes(&observer, worker).is_empty() {
            let before_deadline = Instant::now() < deadline;
            if !before_deadline {
                let diagnostic = fresh_guard_drop_timeout_diagnostic(
                    &observer,
                    worker,
                    &control,
                    fixture.source.as_ref().unwrap(),
                    worker_panics,
                );
                // Report before the original assertion/unwind can quarantine
                // this fixture and generate secondary sticky-admission failures.
                use std::io::Write;
                let _ = writeln!(
                    std::io::stderr().lock(),
                    "fresh Guard Drop first timeout: {diagnostic}"
                );
            }
            assert!(
                before_deadline,
                "fresh Guard Drop lost its exact held worker; worker_panics={worker_panics}, exact_worker={worker}"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        let expected = if worker_panics {
            crate::genuine_app_observer::WorkerJoinOutcome::WorkerPanicked
        } else {
            crate::genuine_app_observer::WorkerJoinOutcome::Joined
        };
        assert_eq!(source_join_outcomes(&observer, worker), [expected]);
        assert_eq!(
            fixture.source.as_ref().unwrap().shutdown_drop_observation(),
            crate::plugins::plugin_runtime::test_capture_source::ShutdownDropObservation::DropBodyExitObserved,
        );
        use crate::plugins::plugin_runtime::RetainedJoinObservation;
        let join = control.genuine_retained_join_observation();
        assert!(if worker_panics {
            matches!(
                join,
                RetainedJoinObservation::PanickedJoinReturned
                    | RetainedJoinObservation::PanickedJoinObserverCallReturned
            )
        } else {
            matches!(
                join,
                RetainedJoinObservation::HealthyJoinReturned
                    | RetainedJoinObservation::HealthyJoinObserverCallReturned
            )
        });
        assert_eq!(control.stats().detached_workers, 0);
        assert_eq!(project_and_history(fixture.app()), before);
    }
}

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
#[test]
fn actual_fresh_source_guard_drop_after_join_starts_no_owner_and_does_not_join_twice() {
    let (mut fixture, _) = unpublished_qualified_fresh_worker();
    let PluginChain {
        audio,
        control,
        mut guard,
    } = fixture.untransferred.take().unwrap();
    let worker = guard.genuine_worker_id();
    let observer = crate::genuine_app_observer::ObservationRun::install().unwrap();
    drop(audio);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match guard.poll_shutdown_retained() {
            RetainedShutdownPoll::Pending => {
                assert!(
                    Instant::now() < deadline,
                    "actual fresh worker did not join"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
            RetainedShutdownPoll::Joined => break,
            RetainedShutdownPoll::WorkerPanicked => {
                panic!("healthy mock joined with panic evidence")
            }
        }
    }
    assert_eq!(
        source_join_outcomes(&observer, worker),
        [crate::genuine_app_observer::WorkerJoinOutcome::Joined]
    );
    drop(guard);
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(
        source_join_outcomes(&observer, worker),
        [crate::genuine_app_observer::WorkerJoinOutcome::Joined]
    );
    observer.with_snapshot(|snapshot| {
        assert!(
            !snapshot.records.iter().any(|record| matches!(&record.kind,
                crate::genuine_app_observer::RecordKind::Lifecycle(site)
                    if site.starts_with("fresh guard retained shutdown owner")
            )),
            "a terminal guard must not start another shutdown owner"
        );
    });
    assert_eq!(control.stats().detached_workers, 0);
}

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
fn switch_fresh_fixture_session(fixture: &mut RestorationFixture, open: bool) {
    let previous = fixture.app().project_session;
    if open {
        let mut project = Project::blank();
        project.name = "Validated mock Open destination".into();
        // Exercise the real post-load Open commit entry; this test supplies a
        // device-free Project and makes no file-reader or SDK claim.
        assert!(
            fixture
                .app_mut()
                .finish_open_project(project, PathBuf::from("/fixture-only/new-session.citrus"))
        );
    } else {
        fixture.app_mut().commit_project_intent(ProjectIntent::New);
    }
    assert_ne!(fixture.app().project_session, previous);
    assert!(fixture.app().fresh_bounce_sources.is_empty());
    assert!(fixture.app().clip_render_restorations.is_empty());
}

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
#[test]
fn actual_fresh_source_new_and_open_marker_clear_removal_retains_late_worker_until_join() {
    for open in [false, true] {
        let mut fixture = actually_installed_fresh_source();
        let channel = fixture.app().project.channels[0].id;
        let worker = fixture.app().running_generator_chains[&channel]
            .guard
            .genuine_worker_id();
        let old_endpoint = fixture.app().running_generator_chains[&channel].endpoint_id;
        let observer = crate::genuine_app_observer::ObservationRun::install().unwrap();
        let receiver = pause_actual_running_fresh_capture(&fixture);
        switch_fresh_fixture_session(&mut fixture, open);
        let destination = project_and_history(fixture.app());
        let deadline = Instant::now() + Duration::from_secs(5);
        while fixture
            .app()
            .running_generator_chains
            .contains_key(&channel)
        {
            fixture
                .callback
                .as_mut()
                .unwrap()
                .pump(fixture.app.as_ref().unwrap().audio.as_ref().unwrap(), 128);
            fixture.app_mut().poll_generator_endpoint_events();
            if fixture
                .app()
                .running_generator_chains
                .contains_key(&channel)
            {
                fixture.app_mut().request_channel_generator_rebuild(channel);
            }
            assert!(
                Instant::now() < deadline,
                "old fresh endpoint did not receive an actual removal"
            );
        }
        observer.with_snapshot(|snapshot| {
            assert_eq!(snapshot.records.iter().filter(|record| matches!(&record.kind,
                crate::genuine_app_observer::RecordKind::AppGeneratorEndpointEvent(
                    GeneratorEndpointEvent::Removed { endpoint_id: Some(id), plugin_instance_id: Some(42), .. }
                ) if *id == old_endpoint
            )).count(), 1);
        });
        assert_eq!(fixture.app().retired_fresh_source_candidates.len(), 1);
        for _ in 0..3 {
            fixture.app_mut().poll_clip_restorations();
            assert_eq!(
                fixture.app().retired_fresh_source_candidates[0].genuine_worker_id(),
                worker
            );
            assert!(source_join_outcomes(&observer, worker).is_empty());
            assert!(!receiver.termination_acknowledged());
            assert_eq!(project_and_history(fixture.app()), destination);
        }
        fixture.source.as_ref().unwrap().release_gates();
        drain_actual_fresh_candidate(&mut fixture, &observer, worker);
        assert!(receiver.termination_acknowledged());
        assert_eq!(project_and_history(fixture.app()), destination);
        assert!(!fixture.app().clip_restoration_failed);
        assert!(!fixture.app().clip_restoration_playback_pending);
    }
}

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
#[test]
fn actual_fresh_source_old_pending_marker_clear_installed_false_retains_exact_guard() {
    let (mut fixture, candidate) = unpublished_qualified_fresh_worker();
    let channel = candidate.channels[0].id;
    let worker = fixture
        .untransferred
        .as_ref()
        .unwrap()
        .guard
        .genuine_worker_id();
    let observer = crate::genuine_app_observer::ObservationRun::install().unwrap();
    fixture
        .source
        .as_ref()
        .unwrap()
        .fail_capture_after_gate("actual old-session candidate failure".into());
    let control = &fixture.untransferred.as_ref().unwrap().control;
    let receiver = control
        .request_instrument_bounce_state(42, control.stats().current_epoch, vec![17])
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match receiver.try_recv() {
            Ok(Err(crate::instrument_bounce::BounceCaptureError::BackendFailed(_))) => break,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                assert!(
                    Instant::now() < deadline,
                    "actual candidate fault did not terminate"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
            other => panic!("unexpected actual candidate fault result: {other:?}"),
        }
    }
    assert!(control.stats().faults > 0);
    // A previously qualified prepared guard keeps its lifetime policy even
    // when its intent registry has disappeared before this later admission.
    fixture.app_mut().fresh_bounce_sources.clear();
    fixture.app_mut().retired_fresh_source_candidates = Vec::new();
    assert_eq!(fixture.app().retired_fresh_source_candidates.capacity(), 0);
    let previous = snapshot_channel_generator(&fixture.app().project, channel);
    let chain = fixture.untransferred.take().unwrap();
    fixture
        .app_mut()
        .queue_channel_generator_candidate_prepared(channel, candidate, previous, Some(chain))
        .unwrap();
    assert!(fixture.app().retired_fresh_source_candidates.capacity() >= 1);
    let endpoint = fixture.app().pending_generator_chains[&channel].endpoint_id;
    switch_fresh_fixture_session(&mut fixture, false);
    let destination = project_and_history(fixture.app());
    fixture
        .callback
        .as_mut()
        .unwrap()
        .pump(fixture.app.as_ref().unwrap().audio.as_ref().unwrap(), 128);
    fixture.app_mut().poll_generator_endpoint_events();
    observer.with_snapshot(|snapshot| {
        assert_eq!(snapshot.records.iter().filter(|record| matches!(&record.kind,
            crate::genuine_app_observer::RecordKind::AppGeneratorEndpointEvent(
                GeneratorEndpointEvent::Installed { endpoint_id, plugin_instance_id: 42, success: false, .. }
            ) if *endpoint_id == endpoint
        )).count(), 1);
    });
    assert!(fixture.app().pending_generator_chains.is_empty());
    assert_eq!(
        fixture.app().retired_fresh_source_candidates[0].genuine_worker_id(),
        worker
    );
    assert_eq!(project_and_history(fixture.app()), destination);
    drain_actual_fresh_candidate(&mut fixture, &observer, worker);
    assert_eq!(project_and_history(fixture.app()), destination);
}

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
#[test]
fn actual_fresh_source_old_pending_marker_clear_installed_success_then_drop_joins_late_worker() {
    let (mut fixture, candidate) = unpublished_qualified_fresh_worker();
    let channel = candidate.channels[0].id;
    let worker = fixture
        .untransferred
        .as_ref()
        .unwrap()
        .guard
        .genuine_worker_id();
    let observer = crate::genuine_app_observer::ObservationRun::install().unwrap();
    let source = fixture.source.as_ref().unwrap();
    let before_capture = source.capture_count();
    source.pause_capture();
    let control = &fixture.untransferred.as_ref().unwrap().control;
    let receiver = control
        .request_instrument_bounce_state(42, control.stats().current_epoch, vec![17])
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while source.capture_count() == before_capture {
        assert!(!receiver.termination_acknowledged());
        assert!(
            Instant::now() < deadline,
            "actual unpublished source did not enter held capture"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let previous = snapshot_channel_generator(&fixture.app().project, channel);
    let chain = fixture.untransferred.take().unwrap();
    fixture
        .app_mut()
        .queue_channel_generator_candidate_prepared(channel, candidate, previous, Some(chain))
        .unwrap();
    switch_fresh_fixture_session(&mut fixture, false);
    let destination = project_and_history(fixture.app());
    fixture
        .callback
        .as_mut()
        .unwrap()
        .pump(fixture.app.as_ref().unwrap().audio.as_ref().unwrap(), 128);
    fixture.app_mut().poll_generator_endpoint_events();
    assert!(fixture.app().pending_generator_chains.is_empty());
    assert_eq!(
        fixture.app().running_generator_chains[&channel]
            .guard
            .genuine_worker_id(),
        worker
    );
    assert!(
        fixture.app().running_generator_chains[&channel]
            .guard
            .requires_retained_fresh_shutdown()
    );
    assert_eq!(project_and_history(fixture.app()), destination);
    fixture.callback = None;
    let app = fixture.app.take().unwrap();
    drop(app);
    assert!(source_join_outcomes(&observer, worker).is_empty());
    assert!(!receiver.termination_acknowledged());
    fixture.source.as_ref().unwrap().release_gates();
    while source_join_outcomes(&observer, worker).is_empty() {
        assert!(
            Instant::now() < deadline,
            "old-session running fresh guard was lost by Drop"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        source_join_outcomes(&observer, worker),
        [crate::genuine_app_observer::WorkerJoinOutcome::Joined]
    );
    assert!(receiver.termination_acknowledged());
}

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
#[test]
fn actual_fresh_source_instance_id_reuse_does_not_mark_new_ordinary_worker_for_retirement() {
    let mut fixture = actually_installed_fresh_source();
    let channel = fixture.app().project.channels[0].id;
    switch_fresh_fixture_session(&mut fixture, false);
    let deadline = Instant::now() + Duration::from_secs(5);
    while fixture
        .app()
        .running_generator_chains
        .contains_key(&channel)
    {
        fixture
            .callback
            .as_mut()
            .unwrap()
            .pump(fixture.app.as_ref().unwrap().audio.as_ref().unwrap(), 128);
        fixture.app_mut().poll_generator_endpoint_events();
        fixture.app_mut().request_channel_generator_rebuild(channel);
        assert!(
            Instant::now() < deadline,
            "old marked worker did not leave the callback"
        );
    }
    while !fixture.app().retired_fresh_source_candidates.is_empty() {
        fixture.app_mut().poll_clip_restorations();
        assert!(Instant::now() < deadline, "old marked worker did not join");
        std::thread::sleep(Duration::from_millis(1));
    }
    let mut candidate = fixture.app().project.clone();
    candidate.plugin_instances = source_project().plugin_instances;
    candidate.plugin_instances[0].id = 42;
    candidate.channels[0].instrument_plugin_instance_id = Some(42);
    let previous = snapshot_channel_generator(&fixture.app().project, channel);
    let (chain, source) = TestCaptureSource::new(
        42,
        source_state(),
        PluginPrepareConfig {
            sample_rate: 48_000.0,
            max_block_frames: MAX_MIXER_BLOCK_FRAMES,
        },
    )
    .unwrap();
    fixture.untransferred = Some(chain);
    fixture.fresh_source = Some(source);
    let deadline = Instant::now() + Duration::from_secs(5);
    while fixture
        .untransferred
        .as_ref()
        .unwrap()
        .control
        .plugin_endpoint_snapshot()
        .is_none()
    {
        assert!(
            Instant::now() < deadline,
            "ordinary reused-id worker did not prepare"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(
        !fixture
            .untransferred
            .as_ref()
            .unwrap()
            .guard
            .requires_retained_fresh_shutdown()
    );
    let chain = fixture.untransferred.take().unwrap();
    fixture
        .app_mut()
        .queue_channel_generator_candidate_prepared(channel, candidate, previous, Some(chain))
        .unwrap();
    fixture
        .callback
        .as_mut()
        .unwrap()
        .pump(fixture.app.as_ref().unwrap().audio.as_ref().unwrap(), 128);
    fixture.app_mut().poll_generator_endpoint_events();
    let ordinary_worker = fixture.app().running_generator_chains[&channel]
        .guard
        .genuine_worker_id();
    assert!(
        !fixture.app().running_generator_chains[&channel]
            .guard
            .requires_retained_fresh_shutdown()
    );
    let destination = project_and_history(fixture.app());
    fixture.app_mut().retain_recreated_sources_before_clear();
    assert_eq!(
        fixture.app().running_generator_chains[&channel]
            .guard
            .genuine_worker_id(),
        ordinary_worker
    );
    assert!(fixture.app().retired_fresh_source_candidates.is_empty());
    assert_eq!(project_and_history(fixture.app()), destination);
    // Fixture cleanup retained-joins this actual ordinary worker before App
    // destruction. This negative classification test never detaches a worker.
}

fn assert_one_healthy_restoration_join(
    observer: &crate::genuine_app_observer::ObservationRun,
    worker: usize,
) {
    use crate::genuine_app_observer::{RecordKind, WorkerJoinOutcome};
    observer.with_snapshot(|snapshot| {
        assert!(snapshot.failure.is_none(), "{:?}", snapshot.failure);
        let joins = snapshot
            .records
            .iter()
            .filter_map(|record| match &record.kind {
                RecordKind::WorkerJoin { worker_id, outcome } if *worker_id == worker => {
                    Some(*outcome)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(joins, [WorkerJoinOutcome::Joined]);
    });
}

fn assert_restoration_refusal_keeps_exact_cleanup(
    fixture: &mut RestorationFixture,
    expected_error: &str,
    endpoint_id_consumed: bool,
) {
    let channel = fixture.app().project.channels[0].id;
    let owner = restoration_owner_ids(&fixture.app().pending_clip_restorations[&channel]);
    assert_eq!(owner, (owner.0, owner.0, owner.0));
    let before = project_and_history(fixture.app());
    let next_id = fixture.app().next_generator_endpoint_id;
    let calls = RESTORATION_FACTORY_CALLS.load(Ordering::Acquire);
    let captures = fixture.source.as_ref().unwrap().capture_count();
    let callback_before = fixture.callback.as_ref().unwrap().genuine_snapshot();
    let full_count = fixture
        .app()
        .audio
        .as_ref()
        .unwrap()
        .snapshot()
        .command_queue_full;
    let observer = crate::genuine_app_observer::ObservationRun::install().unwrap();
    fixture.app_mut().poll_clip_restorations();
    assert!(fixture.app().pending_clip_restorations.is_empty());
    assert!(fixture.app().pending_generator_chains.is_empty());
    assert!(fixture.app().running_generator_chains.is_empty());
    assert!(fixture.app().clip_restoration_installs.is_empty());
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
            .contains(expected_error)
    );
    assert_eq!(project_and_history(fixture.app()), before);
    assert_eq!(
        fixture.app().next_generator_endpoint_id,
        if endpoint_id_consumed {
            next_id + 1
        } else {
            next_id
        }
    );
    assert_eq!(RESTORATION_FACTORY_CALLS.load(Ordering::Acquire), calls);
    assert_eq!(fixture.source.as_ref().unwrap().capture_count(), captures);
    let callback_after = fixture.callback.as_ref().unwrap().genuine_snapshot();
    assert_eq!(
        callback_after.pending_audio_commands,
        callback_before.pending_audio_commands
    );
    assert_eq!(callback_after.device_frame, callback_before.device_frame);
    assert!(callback_after.endpoints.is_empty());
    assert_eq!(
        fixture
            .app()
            .audio
            .as_ref()
            .unwrap()
            .snapshot()
            .command_queue_full,
        full_count
    );
    // The actual poll may already have joined a fast worker. Otherwise every
    // retained guard must still be this exact owner, until its real join below.
    assert!(fixture.app().retired_clip_restorations.len() <= 1);
    for guard in &fixture.app().retired_clip_restorations {
        assert_eq!(guard.genuine_worker_id(), owner.0);
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while !fixture.app().retired_clip_restorations.is_empty() {
        fixture.app_mut().poll_clip_restorations();
        assert!(
            Instant::now() < deadline,
            "exact refused source did not join"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_one_healthy_restoration_join(&observer, owner.0);
    fixture.app_mut().poll_clip_restorations();
    assert!(fixture.app().clip_restoration_failed);
    assert!(fixture.app().clip_restoration_playback_pending);
    assert_eq!(project_and_history(fixture.app()), before);
    assert_eq!(RESTORATION_FACTORY_CALLS.load(Ordering::Acquire), calls);
}

fn full_ring_ready_restoration() -> RestorationFixture {
    let mut fixture = RestorationFixture::new();
    fixture.wait_for_actual_worker_snapshot();
    fill_ordinary_ring_and_prove_stop_all_rejected(fixture.app_mut());
    let channel = fixture.app().project.channels[0].id;
    let owner = restoration_owner_ids(&fixture.app().pending_clip_restorations[&channel]);
    let started_at = fixture.app().pending_clip_restorations[&channel].started_at;
    fixture.app_mut().poll_clip_restorations();
    let pending = &fixture.app().pending_clip_restorations[&channel];
    assert!(pending.ready);
    assert_eq!(restoration_owner_ids(pending), owner);
    assert_eq!(pending.started_at, started_at);
    assert!(!fixture.app().clip_restoration_failed);
    assert_eq!(
        fixture
            .callback
            .as_ref()
            .unwrap()
            .genuine_snapshot()
            .pending_audio_commands,
        64
    );
    fixture
}

#[test]
fn ready_restoration_full_ring_expiry_is_terminal_without_resetting_deadline() {
    let mut fixture = full_ring_ready_restoration();
    let channel = fixture.app().project.channels[0].id;
    let owner = restoration_owner_ids(&fixture.app().pending_clip_restorations[&channel]);
    // Explicit test-only aging crosses the existing 35-second deadline. It
    // retains the actual prepared owner; production polls never reset its age.
    fixture
        .app_mut()
        .pending_clip_restorations
        .get_mut(&channel)
        .unwrap()
        .started_at = Instant::now() - Duration::from_secs(35);
    assert_eq!(
        restoration_owner_ids(&fixture.app().pending_clip_restorations[&channel]),
        owner
    );
    assert_restoration_refusal_keeps_exact_cleanup(&mut fixture, "expired", false);
}

#[test]
fn ready_restoration_full_ring_session_change_is_terminal_before_capacity() {
    let mut fixture = full_ring_ready_restoration();
    fixture.app_mut().project_session += 1;
    assert_restoration_refusal_keeps_exact_cleanup(&mut fixture, "changed session", false);
}

#[test]
fn ready_restoration_full_ring_live_project_change_is_terminal_before_capacity() {
    let mut fixture = full_ring_ready_restoration();
    fixture.app_mut().project.channels[0].name = "Keep the user's new channel name".into();
    assert_restoration_refusal_keeps_exact_cleanup(&mut fixture, "live Project changed", false);
}

#[test]
fn ready_restoration_available_capacity_invalid_quantum_is_terminal_with_exact_cleanup() {
    // This is valid worker preparation, but too small for the actual Q128
    // installer. No allocation fault or installer return value is fabricated.
    let mut fixture = RestorationFixture::with_max_block_frames(1);
    fixture.wait_for_actual_worker_snapshot();
    let channel = fixture.app().project.channels[0].id;
    assert_eq!(
        fixture.app().pending_clip_restorations[&channel]
            .chain
            .as_ref()
            .unwrap()
            .audio
            .max_block_frames(),
        1
    );
    assert!(
        fixture
            .app()
            .audio
            .as_ref()
            .unwrap()
            .ordinary_command_capacity_available()
    );
    assert_eq!(
        fixture
            .callback
            .as_ref()
            .unwrap()
            .genuine_snapshot()
            .pending_audio_commands,
        0
    );
    // The existing text is deliberately not interpreted as a typed queue-full
    // cause: the actual installer rejects this endpoint before any enqueue.
    assert_restoration_refusal_keeps_exact_cleanup(
        &mut fixture,
        "Restoration callback installation queue is full",
        true,
    );
    assert!(
        fixture
            .app()
            .audio
            .as_ref()
            .unwrap()
            .ordinary_command_capacity_available()
    );
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
#[cfg(any(target_os = "linux", windows))]
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

#[cfg(any(target_os = "linux", windows))]
fn restoration_diagnostic_text(text: &str) -> String {
    let mut end = text.len().min(4096);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

#[cfg(any(target_os = "linux", windows))]
fn restoration_admission_diagnostics(app: &CitrusApp) -> String {
    // Read only bounded control metadata. A missing pending owner is not proof
    // of any particular renderer/callback fault or of termination.
    format!(
        "phase={:?}, toast={:?}, fences(operation,clip,restoration,resources)={:?}, topology_settling={}, undo={}, redo={}, owners(clip_pending,clip_retired,fresh_retired,generator_pending,generator_removal,generator_route,generator_deferred,insert_pending,insert_removal,insert_deferred)={:?}, save_barrier={}, playing={}, pointer_preview={:?}, native_topology_error={:?}",
        app.clip_render_job.as_ref().map(|job| job.phase()),
        app.toast
            .as_ref()
            .map(|(message, _)| restoration_diagnostic_text(message)),
        (
            app.clip_operation_fence_active(),
            app.clip_render_fence_active(),
            app.clip_restoration_pending(),
            app.clip_restoration_resources_pending(),
        ),
        app.plugin_topology_is_settling(),
        app.undo_stack.len(),
        app.redo_stack.len(),
        (
            app.pending_clip_restorations.len(),
            app.retired_clip_restorations.len(),
            app.retired_fresh_source_candidates.len(),
            app.pending_generator_chains.len(),
            app.pending_generator_removals.len(),
            app.pending_generator_routes.len(),
            app.deferred_generator_rebuilds.len(),
            app.pending_insert_chains.len(),
            app.pending_insert_removals.len(),
            app.deferred_insert_rebuilds.len(),
        ),
        app.save_barrier.is_some(),
        app.playing,
        (
            app.piano_note_properties.is_some(),
            app.editor_pointer_gesture_active(),
            app.piano_roll_gesture_before.is_some(),
            app.playlist_gesture_before.is_some(),
        ),
        app.native_editor_topology_guard()
            .err()
            .map(|error| restoration_diagnostic_text(&error)),
    )
}

#[cfg(any(target_os = "linux", windows))]
fn wait_for_held_restoration_ready(fixture: &Fixture, channel: u32) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let pending = fixture
            .app()
            .pending_clip_restorations
            .get(&channel)
            .unwrap_or_else(|| {
                panic!(
                    "Undo did not create the expected unpublished restoration owner for channel {channel}; {}",
                    restoration_admission_diagnostics(fixture.app()),
                )
            });
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

#[cfg(any(target_os = "linux", windows))]
fn assert_ordinary_ring_full_before_handoff(fixture: &mut Fixture) {
    assert!(
        !fixture
            .app_mut()
            .audio
            .as_mut()
            .unwrap()
            .command(AudioCommand::SetMaster(1.0)),
        "actual ordinary command enqueue must reject before this callback drain"
    );
}

#[cfg(any(target_os = "linux", windows))]
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

#[cfg(any(target_os = "linux", windows))]
#[test]
fn actual_ready_restoration_full_ring_retains_owner_until_same_owner_callback_install() {
    let mut fixture = converted_fixture_with_mock_restoration();
    let observer = crate::genuine_app_observer::ObservationRun::install().unwrap();
    assert!(fixture.original_source_joined());
    let channel = fixture.app().project.channels[0].id;
    // The converted fixture may need another real tick to finish its admission
    // fences. Establish this test's own bounded-ring pressure after those ticks
    // with actual accepted/rejected commands, before issuing Undo.
    fill_ordinary_ring_and_prove_stop_all_rejected(fixture.app_mut());
    fixture.app_mut().undo();
    wait_for_held_restoration_ready(&fixture, channel);
    assert_eq!(RESTORATION_FACTORY_CALLS.load(Ordering::Acquire), 1);
    let pending = &fixture.app().pending_clip_restorations[&channel];
    let owner = restoration_owner_ids(pending);
    assert_eq!(owner, (owner.0, owner.0, owner.0));
    let started_at = pending.started_at;
    let source_identity = (
        pending.project_session,
        pending.engine_session,
        pending.instance_id,
    );
    let timing = pending.expected_timing;
    let live_revision = pending.live_revision;
    let live_snapshot = Arc::clone(&pending.live_snapshot);
    let candidate_channels = pending.candidate.as_ref().unwrap().channels.as_ptr();
    let previous = pending.previous.clone();
    let next_id = fixture.app().next_generator_endpoint_id;
    let before = project_and_history(fixture.app());
    let callback_before = fixture.callback().genuine_snapshot();
    assert_eq!(callback_before.pending_audio_commands, 64);
    assert!(callback_before.endpoints.is_empty());
    assert!(
        !fixture
            .app()
            .audio
            .as_ref()
            .unwrap()
            .ordinary_command_capacity_available()
    );
    let full_count = fixture
        .app()
        .audio
        .as_ref()
        .unwrap()
        .snapshot()
        .command_queue_full;
    // These are real production polls with no callback drain, reload, capture,
    // or clock reset. The first consumes the actual queued SlotReady event.
    fixture.app_mut().poll_clip_restorations();
    let candidate = clip_render::project_snapshot(
        fixture.app().pending_clip_restorations[&channel]
            .candidate
            .as_ref()
            .unwrap(),
    )
    .unwrap();
    for _ in 0..3 {
        fixture.app_mut().poll_clip_restorations();
        let pending = &fixture.app().pending_clip_restorations[&channel];
        assert!(pending.ready);
        assert_eq!(restoration_owner_ids(pending), owner);
        assert_eq!(pending.started_at, started_at);
        assert_eq!(
            (
                pending.project_session,
                pending.engine_session,
                pending.instance_id
            ),
            source_identity
        );
        assert_eq!(pending.expected_timing, timing);
        assert_eq!(pending.live_revision, live_revision);
        assert!(Arc::ptr_eq(&pending.live_snapshot, &live_snapshot));
        assert_eq!(
            pending.candidate.as_ref().unwrap().channels.as_ptr(),
            candidate_channels
        );
        assert!(
            clip_render::project_matches_snapshot(pending.candidate.as_ref().unwrap(), &candidate)
                .unwrap()
        );
        assert_eq!(pending.previous, previous);
        assert!(pending.transferred_control.is_none());
        assert!(pending.transferred_guard.is_none());
        assert_eq!(fixture.app().next_generator_endpoint_id, next_id);
        assert_eq!(project_and_history(fixture.app()), before);
        assert_eq!(RESTORATION_FACTORY_CALLS.load(Ordering::Acquire), 1);
        assert_eq!(fixture.source_handle().capture_count(), 2);
        assert!(fixture.app().pending_generator_chains.is_empty());
        assert!(fixture.app().running_generator_chains.is_empty());
        assert!(fixture.app().retired_clip_restorations.is_empty());
        assert!(fixture.app().clip_restoration_installs.is_empty());
        assert!(!fixture.app().clip_restoration_failed);
        assert!(fixture.app().clip_restoration_playback_pending);
        assert!(!fixture.app().playing);
        let callback = fixture.callback().genuine_snapshot();
        assert_eq!(callback.pending_audio_commands, 64);
        assert_eq!(callback.device_frame, callback_before.device_frame);
        assert!(
            callback.endpoints.is_empty(),
            "the removed old endpoint must stay absent"
        );
        assert_eq!(
            fixture
                .app()
                .audio
                .as_ref()
                .unwrap()
                .snapshot()
                .command_queue_full,
            full_count
        );
    }
    fixture.pump_callback_only();
    assert_eq!(
        fixture.callback().genuine_snapshot().pending_audio_commands,
        0
    );
    assert!(
        fixture
            .app()
            .audio
            .as_ref()
            .unwrap()
            .ordinary_command_capacity_available()
    );
    fixture.app_mut().poll_clip_restorations();
    assert!(fixture.app().pending_clip_restorations.is_empty());
    let installed = &fixture.app().pending_generator_chains[&channel];
    assert_eq!(installed.endpoint_id, next_id);
    assert_eq!(installed.control.genuine_worker_id(), owner.0);
    assert_eq!(installed.guard.genuine_worker_id(), owner.0);
    assert_eq!(fixture.app().next_generator_endpoint_id, next_id + 1);
    assert_eq!(
        fixture.app().clip_restoration_installs.get(&41),
        Some(&(next_id, false))
    );
    assert!(fixture.app().running_generator_chains.is_empty());
    assert!(clip_render::project_matches_snapshot(&fixture.app().project, &candidate).unwrap());
    assert_eq!(
        fixture.callback().genuine_snapshot().pending_audio_commands,
        1
    );
    // Re-polling before the real callback receipt cannot install a second copy.
    fixture.app_mut().poll_clip_restorations();
    assert_eq!(
        fixture.callback().genuine_snapshot().pending_audio_commands,
        1
    );
    assert_eq!(fixture.app().next_generator_endpoint_id, next_id + 1);
    fixture.until(|fixture| {
        !fixture.app().clip_restoration_playback_pending
            && !fixture.app().clip_restoration_failed
            && fixture.app().timeline_audio_matches_desired()
    });
    assert_eq!(RESTORATION_FACTORY_CALLS.load(Ordering::Acquire), 1);
    assert_eq!(fixture.app().running_generator_chains.len(), 1);
    let running = &fixture.app().running_generator_chains[&channel];
    assert_eq!(running.endpoint_id, next_id);
    assert_eq!(running.control.genuine_worker_id(), owner.0);
    assert_eq!(running.guard.genuine_worker_id(), owner.0);
    assert_eq!(running.control.stats().detached_workers, 0);
    assert_eq!(
        fixture.app().clip_restoration_installs.get(&41),
        Some(&(next_id, true))
    );
    let callback = fixture.callback().genuine_snapshot();
    assert_eq!(callback.endpoints.len(), 1);
    assert_eq!(callback.endpoints[0].endpoint_id, next_id);
    assert_eq!(callback.endpoints[0].fixed.bridge.worker_id, owner.0);
    observer.with_snapshot(|snapshot| {
        let installed = snapshot
            .records
            .iter()
            .filter_map(|record| match &record.kind {
                crate::genuine_app_observer::RecordKind::AppGeneratorEndpointEvent(event)
                    if matches!(event, GeneratorEndpointEvent::Installed { .. }) =>
                {
                    Some(*event)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(installed.len(), 1);
        assert!(matches!(installed[0], GeneratorEndpointEvent::Installed {
            channel_id, endpoint_id, plugin_instance_id: 41, success: true,
            replaced_endpoint_id: None, replaced_plugin_instance_id: None, ..
        } if channel_id == channel && endpoint_id == next_id));
    });
    // Existing bounded Fixture cleanup owns the same restored guard, including
    // assertion/unwind paths. Its exact join is observed, never inferred.
    drop(fixture);
    assert_one_healthy_restoration_join(&observer, owner.0);
}

#[cfg(any(target_os = "linux", windows))]
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
    // Drain Undo's ordinary mixer commands before testing successful admission.
    // No installation is queued yet and no generator receipt is consumed here.
    assert!(
        fixture
            .app()
            .pending_clip_restorations
            .contains_key(&channel)
    );
    assert!(
        !fixture
            .app()
            .pending_generator_chains
            .contains_key(&channel)
    );
    assert_ordinary_ring_full_before_handoff(&mut fixture);
    fixture.pump_callback_only();
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
    // Redo's mixer commands can defer removal behind the real bounded ring.
    // Drain them and issue the real retry without consuming its later receipt.
    assert!(
        !fixture
            .app()
            .pending_generator_removals
            .contains_key(&channel)
    );
    assert!(fixture.app().deferred_generator_rebuilds.contains(&channel));
    assert_ordinary_ring_full_before_handoff(&mut fixture);
    fixture.pump_callback_only();
    fixture.app_mut().poll_clip_restorations();
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

#[cfg(any(target_os = "linux", windows))]
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

#[cfg(any(target_os = "linux", windows))]
#[test]
fn actual_undo_redo_reuses_verified_arc_after_published_wav_is_modified() {
    assert_redo_uses_retained_pcm_after_external_file_change(false);
}

#[cfg(any(target_os = "linux", windows))]
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

#[cfg(any(target_os = "linux", windows))]
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

#[cfg(any(target_os = "linux", windows))]
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

#[cfg(any(target_os = "linux", windows))]
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
            && !fixture.app().clip_operation_fence_active()
            && !fixture.app().plugin_topology_is_settling()
    });
    assert!(fixture.original_source_joined());
    fixture
}

#[cfg(any(target_os = "linux", windows))]
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

#[cfg(any(target_os = "linux", windows))]
#[test]
fn actual_installed_receipt_panic_retains_exact_pending_owner_and_blocks_later_admission() {
    let mut fixture = converted_fixture_with_mock_restoration();
    let channel = fixture.app().project.channels[0].id;
    fixture.app_mut().undo();
    wait_for_held_restoration_ready(&fixture, channel);
    // Undo filled the ordinary ring; this test targets receipt consumption,
    // not the separate queue-capacity deferral path.
    assert!(
        fixture
            .app()
            .pending_clip_restorations
            .contains_key(&channel)
    );
    assert!(
        !fixture
            .app()
            .pending_generator_chains
            .contains_key(&channel)
    );
    assert_ordinary_ring_full_before_handoff(&mut fixture);
    fixture.pump_callback_only();
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

#[cfg(any(target_os = "linux", windows))]
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
    // Admit the deferred removal after draining Redo's ordinary commands.
    // Leave generator receipt polling to the panic-armed Fixture tick below.
    assert!(
        !fixture
            .app()
            .pending_generator_removals
            .contains_key(&channel)
    );
    assert!(fixture.app().deferred_generator_rebuilds.contains(&channel));
    assert_ordinary_ring_full_before_handoff(&mut fixture);
    fixture.pump_callback_only();
    fixture.app_mut().poll_clip_restorations();
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

#[cfg(any(target_os = "linux", windows))]
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

#[cfg(any(target_os = "linux", windows))]
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

#[cfg(any(target_os = "linux", windows))]
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

#[cfg(any(target_os = "linux", windows))]
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

#[cfg(any(target_os = "linux", windows))]
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

#[cfg(any(target_os = "linux", windows))]
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

// Route-predicate matrices use one retained mock owner at a time. They compile
// immutable model inputs for the real App classifier; none is a dispatched or
// injected compiler result, callback receipt, successful install, or Ready event.
#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
fn pending_route_fixture(held: bool, worker_instance: u64) -> RestorationFixture {
    let mut fixture =
        RestorationFixture::with_preparation(MAX_MIXER_BLOCK_FRAMES, held, worker_instance);
    let binding = fixture
        .app_mut()
        .prepare_clip_render_restoration(41, &source_state().metadata)
        .unwrap();
    fixture.app_mut().install_clip_render_restoration(binding);
    fixture
}

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
fn route_test_timeline(project: &Project) -> CompiledTimeline {
    let map = TempoMap::from_project(project, 48_000).unwrap();
    CompiledTimeline::from_project(project, &map, TimelineCompileOptions::default()).unwrap()
}

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
fn exact_route(timeline: &CompiledTimeline) -> GeneratorRouteRequirement {
    let requirements = timeline_generator_route_requirements(timeline).unwrap();
    assert_eq!(requirements.len(), 1);
    requirements[0]
}

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
fn assert_pending_route(fixture: &RestorationFixture, timeline: &CompiledTimeline, eligible: bool) {
    let app = fixture.app();
    let requirement = exact_route(timeline);
    assert!(app.pending_generator_chains.is_empty());
    assert!(app.pending_generator_removals.is_empty());
    assert!(app.pending_generator_routes.is_empty());
    assert!(app.running_generator_chains.is_empty());
    assert_eq!(
        app.clip_restoration_pending_for_timeline_route(&requirement),
        eligible
    );
    let actual = app.timeline_generator_routes_state(timeline);
    if eligible {
        assert_eq!(
            actual,
            RequiredGeneratorRouteState::Pending {
                channel_id: requirement.channel_id,
                plugin_instance_id: requirement.plugin_instance_id,
            }
        );
    } else {
        assert!(
            matches!(actual, RequiredGeneratorRouteState::Failed(_)),
            "{actual:?}"
        );
    }
}

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
fn refresh_pending_route_snapshot(fixture: &mut RestorationFixture) {
    let snapshot = clip_render::project_snapshot(&fixture.app().project).unwrap();
    let channel = fixture.app().project.channels[0].id;
    fixture
        .app_mut()
        .pending_clip_restorations
        .get_mut(&channel)
        .unwrap()
        .live_snapshot = snapshot;
}

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
#[test]
fn restoration_pending_route_accepts_exact_unready_owner_without_mutation() {
    let mut fixture = pending_route_fixture(true, 41);
    // Stable mixer identity is deliberately unrelated to its actual runtime slot.
    let channel = fixture.app().project.channels[0].id;
    let old_id = fixture.app().project.channels[0].mixer_track;
    let remap = |project: &mut Project| {
        let index = project.mixer_track_index(old_id).unwrap();
        project.mixer_tracks[index].id = 8_123;
        project.channels[0].mixer_track = 8_123;
        for route in &mut project.mixer_routes {
            if route.source_mixer_track_id == old_id {
                route.source_mixer_track_id = 8_123;
            }
            let (MixerRouteDestination::MainInput { mixer_track_id }
            | MixerRouteDestination::PluginSidechain { mixer_track_id, .. }) =
                &mut route.destination;
            if *mixer_track_id == old_id {
                *mixer_track_id = 8_123;
            }
        }
    };
    remap(&mut fixture.app_mut().project);
    remap(
        fixture
            .app_mut()
            .pending_clip_restorations
            .get_mut(&channel)
            .unwrap()
            .candidate
            .as_mut()
            .unwrap(),
    );
    refresh_pending_route_snapshot(&mut fixture);
    let timeline = route_test_timeline(&fixture.app().project);
    assert_ne!(exact_route(&timeline).mixer_track as u64, 8_123);
    let pending = &fixture.app().pending_clip_restorations[&channel];
    assert!(!pending.ready);
    assert!(
        pending
            .chain
            .as_ref()
            .unwrap()
            .control
            .plugin_endpoint_snapshot()
            .is_none()
    );
    let owner = restoration_owner_ids(pending);
    let started_at = pending.started_at;
    let candidate_bytes =
        clip_render::project_snapshot(pending.candidate.as_ref().unwrap()).unwrap();
    let history = project_and_history(fixture.app());
    let state = fixture.app().genuine_timeline_app_state();
    let next_endpoint = fixture.app().next_generator_endpoint_id;
    let callback_before = fixture.callback.as_ref().unwrap().genuine_snapshot();
    let captures = fixture.source.as_ref().unwrap().capture_count();
    for _ in 0..8 {
        assert_pending_route(&fixture, &timeline, true);
        let pending = &fixture.app().pending_clip_restorations[&channel];
        assert!(!pending.ready);
        assert_eq!(restoration_owner_ids(pending), owner);
        assert_eq!(pending.started_at, started_at);
        assert!(
            clip_render::project_matches_snapshot(
                pending.candidate.as_ref().unwrap(),
                &candidate_bytes
            )
            .unwrap()
        );
        assert_eq!(project_and_history(fixture.app()), history);
        assert_eq!(fixture.app().genuine_timeline_app_state(), state);
        assert_eq!(fixture.app().next_generator_endpoint_id, next_endpoint);
    }
    assert_eq!(fixture.source.as_ref().unwrap().capture_count(), captures);
    let callback_after = fixture.callback.as_ref().unwrap().genuine_snapshot();
    assert_eq!(
        callback_after.pending_audio_commands,
        callback_before.pending_audio_commands
    );
    assert_eq!(callback_after.device_frame, callback_before.device_frame);
    assert!(callback_after.endpoints.is_empty());
    // Published metadata before App consumption of the actual Ready is still Pending.
    fixture.source.as_ref().unwrap().release_prepare();
    fixture.wait_for_actual_worker_snapshot();
    assert_pending_route(&fixture, &timeline, true);
    assert!(!fixture.app().pending_clip_restorations[&channel].ready);
    fill_ordinary_ring_and_prove_stop_all_rejected(fixture.app_mut());
    fixture.app_mut().poll_clip_restorations();
    let pending = &fixture.app().pending_clip_restorations[&channel];
    assert!(
        pending.ready,
        "read-only classification must leave the actual Ready queued"
    );
    assert_eq!(restoration_owner_ids(pending), owner);
    assert_eq!(pending.started_at, started_at);
    assert_pending_route(&fixture, &timeline, true);
}

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
#[test]
fn restoration_pending_route_rejects_channel_instance_mixer_and_session_mismatch() {
    let mut fixture = pending_route_fixture(false, 41);
    let timeline = route_test_timeline(&fixture.app().project);
    let requirement = exact_route(&timeline);
    let channel = requirement.channel_id;
    for mismatch in 0..3 {
        let mut model = fixture.app().project.clone();
        match mismatch {
            0 => model.channels[0].id += 100,
            1 => {
                model.channels[0].instrument_plugin_instance_id = Some(42);
                model.plugin_instances[0].id = 42;
            }
            _ => {
                model.channels[0].mixer_track = model
                    .mixer_tracks
                    .iter()
                    .find(|track| usize::from(track.runtime_slot) != requirement.mixer_track)
                    .unwrap()
                    .id
            }
        }
        assert_pending_route(&fixture, &route_test_timeline(&model), false);
    }
    let owner = fixture
        .app_mut()
        .pending_clip_restorations
        .remove(&channel)
        .unwrap();
    fixture
        .app_mut()
        .pending_clip_restorations
        .insert(channel + 100, owner);
    assert_pending_route(&fixture, &timeline, false);
    let owner = fixture
        .app_mut()
        .pending_clip_restorations
        .remove(&(channel + 100))
        .unwrap();
    fixture
        .app_mut()
        .pending_clip_restorations
        .insert(channel, owner);
    let original = fixture.app().pending_clip_restorations[&channel]
        .candidate
        .as_ref()
        .unwrap()
        .clone();
    for mismatch in 0..4 {
        let candidate = fixture
            .app_mut()
            .pending_clip_restorations
            .get_mut(&channel)
            .unwrap()
            .candidate
            .as_mut()
            .unwrap();
        match mismatch {
            0 => candidate.channels[0].id += 100,
            1 => candidate.channels[0].instrument_plugin_instance_id = Some(42),
            2 => candidate.plugin_instances[0].id = 42,
            _ => {
                candidate.channels[0].mixer_track = candidate
                    .mixer_tracks
                    .iter()
                    .find(|track| usize::from(track.runtime_slot) != requirement.mixer_track)
                    .unwrap()
                    .id
            }
        }
        assert_pending_route(&fixture, &timeline, false);
        fixture
            .app_mut()
            .pending_clip_restorations
            .get_mut(&channel)
            .unwrap()
            .candidate = Some(original.clone());
    }
    let session = fixture.app().project_session;
    fixture.app_mut().project_session += 1;
    assert_pending_route(&fixture, &timeline, false);
    fixture.app_mut().project_session = session;
    fixture
        .app_mut()
        .pending_clip_restorations
        .get_mut(&channel)
        .unwrap()
        .project_session += 1;
    assert_pending_route(&fixture, &timeline, false);
    fixture
        .app_mut()
        .pending_clip_restorations
        .get_mut(&channel)
        .unwrap()
        .project_session = session;
    assert!(
        fixture
            .app_mut()
            .audio
            .as_mut()
            .unwrap()
            .set_project_session(session + 1)
    );
    assert_pending_route(&fixture, &timeline, false);
    assert!(
        fixture
            .app_mut()
            .audio
            .as_mut()
            .unwrap()
            .set_project_session(session)
    );
    let audio = fixture.app_mut().audio.take();
    assert_pending_route(&fixture, &timeline, false);
    fixture.app_mut().audio = audio;
    let (mut replacement, callback) = AudioEngine::test_engine_with_callback();
    assert!(replacement.set_project_session(session));
    assert_ne!(
        replacement.engine_session(),
        fixture.app().audio.as_ref().unwrap().engine_session()
    );
    fixture.callback = Some(callback);
    fixture.app_mut().audio = Some(replacement);
    assert_pending_route(&fixture, &timeline, false);
}

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
#[test]
fn restoration_pending_route_rejects_stale_live_revision_snapshot_and_candidate_binding() {
    let mut fixture = pending_route_fixture(false, 41);
    let timeline = route_test_timeline(&fixture.app().project);
    let channel = exact_route(&timeline).channel_id;
    let original = fixture.app().project.clone();
    fixture.app_mut().clip_render_model_revision += 1;
    assert_pending_route(&fixture, &timeline, false);
    fixture.app_mut().clip_render_model_revision -= 1;
    fixture
        .app_mut()
        .project
        .name
        .push_str(" live changed with same revision");
    assert_pending_route(&fixture, &timeline, false);
    fixture.app_mut().project = original;
    let snapshot = fixture.app().pending_clip_restorations[&channel]
        .live_snapshot
        .clone();
    fixture
        .app_mut()
        .pending_clip_restorations
        .get_mut(&channel)
        .unwrap()
        .live_snapshot = Arc::from([0_u8]);
    assert_pending_route(&fixture, &timeline, false);
    fixture
        .app_mut()
        .pending_clip_restorations
        .get_mut(&channel)
        .unwrap()
        .live_snapshot = snapshot;
    let candidate = fixture
        .app_mut()
        .pending_clip_restorations
        .get_mut(&channel)
        .unwrap()
        .candidate
        .take();
    assert_pending_route(&fixture, &timeline, false);
    fixture
        .app_mut()
        .pending_clip_restorations
        .get_mut(&channel)
        .unwrap()
        .candidate = candidate;
    fixture
        .app_mut()
        .pending_clip_restorations
        .get_mut(&channel)
        .unwrap()
        .candidate
        .as_mut()
        .unwrap()
        .plugin_instances
        .clear();
    assert_pending_route(&fixture, &timeline, false);
}

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
#[test]
fn restoration_pending_route_rejects_missing_or_changed_binding_and_dry_configuration() {
    let mut fixture = pending_route_fixture(false, 41);
    let timeline = route_test_timeline(&fixture.app().project);
    let channel = exact_route(&timeline).channel_id;
    let binding = fixture
        .app_mut()
        .clip_render_restorations
        .remove(&41)
        .unwrap();
    assert_pending_route(&fixture, &timeline, false);
    fixture
        .app_mut()
        .clip_render_restorations
        .insert(41, binding.clone());
    for mismatch in 0..6 {
        let changed = fixture
            .app_mut()
            .clip_render_restorations
            .get_mut(&41)
            .unwrap();
        match mismatch {
            0 => changed.project_session += 1,
            1 => changed.instance_id += 1,
            2 => changed.descriptor_id.push_str(" changed"),
            3 => changed.module_path = PathBuf::from("/changed-binding.vst3"),
            4 => changed.metadata.latency_samples += 1,
            _ => changed.metadata.tail_samples += 1,
        }
        assert_pending_route(&fixture, &timeline, false);
        fixture
            .app_mut()
            .clip_render_restorations
            .insert(41, binding.clone());
    }
    let live = fixture.app().project.clone();
    let candidate = fixture.app().pending_clip_restorations[&channel]
        .candidate
        .as_ref()
        .unwrap()
        .clone();
    for change_live in [false, true] {
        for mismatch in 0..8 {
            let project = if change_live {
                &mut fixture.app_mut().project
            } else {
                fixture
                    .app_mut()
                    .pending_clip_restorations
                    .get_mut(&channel)
                    .unwrap()
                    .candidate
                    .as_mut()
                    .unwrap()
            };
            let instance = &mut project.plugin_instances[0];
            match mismatch {
                0 => instance.uid.push_str(" changed"),
                1 => instance.path = PathBuf::from("/changed-instance.vst3"),
                2 => instance.format = ProjectPluginFormat::Vst2,
                3 => instance.role = PluginRole::Effect,
                4 => instance.enabled = false,
                5 => instance.bypass = true,
                6 => instance.wet = 0.0,
                _ => instance.wet = 0.5,
            }
            // For live cases keep currentness exact, isolating the live binding/config guard.
            if change_live {
                refresh_pending_route_snapshot(&mut fixture);
            }
            assert_pending_route(&fixture, &timeline, false);
            fixture.app_mut().project = live.clone();
            fixture
                .app_mut()
                .pending_clip_restorations
                .get_mut(&channel)
                .unwrap()
                .candidate = Some(candidate.clone());
            refresh_pending_route_snapshot(&mut fixture);
        }
    }
    assert_pending_route(&fixture, &timeline, true);
}

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
#[test]
fn restoration_pending_route_rejects_expiry_terminal_failure_cancelled_and_missing_owner() {
    {
        let mut fixture = pending_route_fixture(false, 41);
        let timeline = route_test_timeline(&fixture.app().project);
        let channel = exact_route(&timeline).channel_id;
        let started_at = fixture.app().pending_clip_restorations[&channel].started_at;
        fixture
            .app_mut()
            .pending_clip_restorations
            .get_mut(&channel)
            .unwrap()
            .started_at = Instant::now() - Duration::from_secs(36);
        assert_pending_route(&fixture, &timeline, false);
        fixture
            .app_mut()
            .pending_clip_restorations
            .get_mut(&channel)
            .unwrap()
            .started_at = started_at;
        fixture.app_mut().clip_restoration_failed = true;
        assert_pending_route(&fixture, &timeline, false);
        fixture.app_mut().clip_restoration_failed = false;
        fixture.app_mut().clip_restoration_playback_pending = false;
        assert_pending_route(&fixture, &timeline, false);
        fixture.app_mut().clip_restoration_playback_pending = true;
        fixture.removed_restoration = fixture.app_mut().pending_clip_restorations.remove(&channel);
        assert_pending_route(&fixture, &timeline, false);
        let pending = fixture.removed_restoration.take().unwrap();
        fixture
            .app_mut()
            .pending_clip_restorations
            .insert(channel, pending);
        fixture.untransferred = fixture
            .app_mut()
            .pending_clip_restorations
            .get_mut(&channel)
            .unwrap()
            .chain
            .take();
        assert_pending_route(&fixture, &timeline, false);
        let chain = fixture.untransferred.take().unwrap();
        fixture
            .app_mut()
            .pending_clip_restorations
            .get_mut(&channel)
            .unwrap()
            .chain = Some(chain);
        assert_pending_route(&fixture, &timeline, true);
        // A split/transferred owner is retained for cleanup, never a route certificate.
        let pending = fixture
            .app_mut()
            .pending_clip_restorations
            .get_mut(&channel)
            .unwrap();
        let PluginChain {
            audio,
            control,
            guard,
        } = pending.chain.take().unwrap();
        pending.transferred_control = Some(control);
        pending.transferred_guard = Some(guard);
        drop(audio);
        assert_pending_route(&fixture, &timeline, false);
        let guard = fixture
            .app_mut()
            .pending_clip_restorations
            .get_mut(&channel)
            .unwrap()
            .transferred_guard
            .take()
            .unwrap();
        fixture.app_mut().retired_clip_restorations.push(guard);
        fixture
            .app_mut()
            .pending_clip_restorations
            .get_mut(&channel)
            .unwrap()
            .transferred_control = None;
        assert_pending_route(&fixture, &timeline, false);
    }
    // Obtain an actual refused session intent with no moved source owners. Its
    // original real callback-confirmed worker is drained by Fixture before reuse.
    let retirement = {
        let mut fixture = Fixture::new();
        let binding = fixture
            .app_mut()
            .prepare_clip_render_restoration(41, &source_state().metadata)
            .unwrap();
        fixture.app_mut().install_clip_render_restoration(binding);
        fixture.app_mut().clip_restoration_playback_pending = true;
        assert!(
            fixture
                .app_mut()
                .defer_clip_session_intent(ProjectIntent::Quit)
                .is_none()
        );
        fixture.app_mut().clip_session_retirement.take().unwrap()
    };
    let mut fixture = pending_route_fixture(false, 41);
    let timeline = route_test_timeline(&fixture.app().project);
    assert_pending_route(&fixture, &timeline, true);
    fixture.app_mut().clip_session_retirement = Some(retirement);
    assert_pending_route(&fixture, &timeline, false);
    assert!(fixture.app_mut().cancel_clip_session_retirement());
    assert_pending_route(&fixture, &timeline, true);
}

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
#[test]
fn restoration_pending_route_rejects_observed_worker_fault_stop_and_published_identity_timing_drift()
 {
    {
        let mut fixture = pending_route_fixture(true, 41);
        let timeline = route_test_timeline(&fixture.app().project);
        let channel = exact_route(&timeline).channel_id;
        assert!(
            fixture.app().pending_clip_restorations[&channel]
                .chain
                .as_ref()
                .unwrap()
                .control
                .plugin_endpoint_snapshot()
                .is_none()
        );
        assert_pending_route(&fixture, &timeline, true);
        // Synthetic missing-read input only: None can also be a coherent-read
        // collision after a real Ready. It grants only Pending, never Ready or
        // admission. No successful worker or callback receipt is fabricated.
        fixture
            .app_mut()
            .pending_clip_restorations
            .get_mut(&channel)
            .unwrap()
            .ready = true;
        assert_pending_route(&fixture, &timeline, true);
        fixture
            .app_mut()
            .pending_clip_restorations
            .get_mut(&channel)
            .unwrap()
            .ready = false;
        assert_pending_route(&fixture, &timeline, true);
    }
    {
        let fixture = pending_route_fixture(false, 42);
        let timeline = route_test_timeline(&fixture.app().project);
        let channel = exact_route(&timeline).channel_id;
        let deadline = Instant::now() + Duration::from_secs(5);
        while fixture.app().pending_clip_restorations[&channel]
            .chain
            .as_ref()
            .unwrap()
            .control
            .plugin_endpoint_snapshot()
            .is_none()
        {
            assert!(
                Instant::now() < deadline,
                "real mismatched manifest did not publish"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_pending_route(&fixture, &timeline, false);
        assert!(!fixture.app().pending_clip_restorations[&channel].ready);
    }
    {
        let mut fixture = pending_route_fixture(false, 41);
        fixture.wait_for_actual_worker_snapshot();
        let timeline = route_test_timeline(&fixture.app().project);
        let channel = exact_route(&timeline).channel_id;
        assert_pending_route(&fixture, &timeline, true);
        let deadline = Instant::now() + Duration::from_secs(5);
        // Explicit retained shutdown observes the real stopped flag; no stats are overwritten.
        loop {
            let result = fixture
                .app_mut()
                .pending_clip_restorations
                .get_mut(&channel)
                .unwrap()
                .chain
                .as_mut()
                .unwrap()
                .guard
                .poll_shutdown_retained();
            if result == RetainedShutdownPoll::Joined {
                break;
            }
            assert_eq!(result, RetainedShutdownPoll::Pending);
            assert!(
                Instant::now() < deadline,
                "exact mock shutdown did not join"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(
            fixture.app().pending_clip_restorations[&channel]
                .chain
                .as_ref()
                .unwrap()
                .control
                .stats()
                .stopped
        );
        assert_pending_route(&fixture, &timeline, false);
    }
    {
        let mut fixture = pending_route_fixture(false, 41);
        fixture.wait_for_actual_worker_snapshot();
        let timeline = route_test_timeline(&fixture.app().project);
        let channel = exact_route(&timeline).channel_id;
        let mut changed = source_state();
        changed.metadata.latency_samples = 1;
        fixture.source.as_ref().unwrap().set_capture_state(changed);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !fixture.app().pending_clip_restorations[&channel]
            .chain
            .as_ref()
            .unwrap()
            .control
            .plugin_endpoint_snapshot()
            .is_some_and(|snapshot| snapshot.total_plugin_latency_samples() == 1)
        {
            assert!(
                Instant::now() < deadline,
                "real changed timing did not publish"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!fixture.app().pending_clip_restorations[&channel].ready);
        assert_pending_route(&fixture, &timeline, true);
        // The actual driver consumes Ready and rejects the published mismatch.
        fixture.removed_restoration = fixture.app_mut().pending_clip_restorations.remove(&channel);
        // Keep the sole worker inside the active cleanup fixture throughout the call.
        let RestorationFixture {
            app,
            removed_restoration,
            ..
        } = &mut fixture;
        let result = app
            .as_deref_mut()
            .unwrap()
            .drive_clip_restoration(channel, removed_restoration.as_mut().unwrap());
        assert!(result.unwrap_err().contains("identity/timing"));
        let pending = fixture.removed_restoration.take().unwrap();
        fixture
            .app_mut()
            .pending_clip_restorations
            .insert(channel, pending);
        assert!(fixture.app().pending_clip_restorations[&channel].ready);
        assert_pending_route(&fixture, &timeline, false);
    }
    {
        let fixture = pending_route_fixture(false, 41);
        fixture.wait_for_actual_worker_snapshot();
        let timeline = route_test_timeline(&fixture.app().project);
        let channel = exact_route(&timeline).channel_id;
        fixture
            .source
            .as_ref()
            .unwrap()
            .fail_capture_after_gate("actual predicate-matrix backend failure".into());
        let control = &fixture.app().pending_clip_restorations[&channel]
            .chain
            .as_ref()
            .unwrap()
            .control;
        let receiver = control
            .request_instrument_bounce_state(41, control.stats().current_epoch, vec![17])
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let outcome = loop {
            match receiver.try_recv() {
                Ok(outcome) => break outcome,
                Err(std::sync::mpsc::TryRecvError::Empty) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(1))
                }
                other => panic!("actual failure capture did not terminate: {other:?}"),
            }
        };
        assert!(matches!(
            outcome,
            Err(crate::instrument_bounce::BounceCaptureError::BackendFailed(
                _
            ))
        ));
        assert!(receiver.termination_acknowledged());
        assert!(control.stats().faults > 0);
        assert_pending_route(&fixture, &timeline, false);
    }
}

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
#[test]
fn restoration_pending_route_preserves_ordinary_pending_and_confirmed_route_results() {
    let mut fixture = pending_route_fixture(false, 41);
    fixture.wait_for_actual_worker_snapshot();
    let timeline = route_test_timeline(&fixture.app().project);
    let requirement = exact_route(&timeline);
    let channel = requirement.channel_id;
    let empty = route_test_timeline(&Project::blank());
    assert_eq!(
        fixture.app().timeline_generator_routes_state(&empty),
        RequiredGeneratorRouteState::Ready
    );
    fixture.app_mut().clip_restoration_playback_pending = false;
    assert_pending_route(&fixture, &timeline, false);
    let project_session = fixture.app().project_session;
    for ordinary in 0..2 {
        match ordinary {
            0 => {
                fixture
                    .app_mut()
                    .pending_generator_routes
                    .insert(channel, requirement.mixer_track);
            }
            _ => {
                fixture.app_mut().pending_generator_removals.insert(
                    channel,
                    PendingGeneratorRemoval {
                        project_session,
                        endpoint_id: None,
                        plugin_instance_id: Some(41),
                    },
                );
            }
        }
        assert_eq!(
            fixture.app().timeline_generator_routes_state(&timeline),
            RequiredGeneratorRouteState::Pending {
                channel_id: channel,
                plugin_instance_id: 41,
            }
        );
        fixture.app_mut().pending_generator_routes.clear();
        fixture.app_mut().pending_generator_removals.clear();
    }
    fixture.app_mut().clip_restoration_playback_pending = true;
    let owner = restoration_owner_ids(&fixture.app().pending_clip_restorations[&channel]);
    fixture.app_mut().poll_clip_restorations();
    assert!(fixture.app().pending_clip_restorations.is_empty());
    assert!(fixture.app().running_generator_chains.is_empty());
    let pending = &fixture.app().pending_generator_chains[&channel];
    assert_eq!(
        (
            pending.control.genuine_worker_id(),
            pending.guard.genuine_worker_id()
        ),
        (owner.0, owner.0)
    );
    assert_eq!(
        fixture.app().timeline_generator_routes_state(&timeline),
        RequiredGeneratorRouteState::Pending {
            channel_id: channel,
            plugin_instance_id: 41,
        }
    );
    // Only the actual callback Installed receipt may create the confirmed route.
    fixture
        .callback
        .as_mut()
        .unwrap()
        .pump(fixture.app.as_ref().unwrap().audio.as_ref().unwrap(), 128);
    fixture.app_mut().poll_generator_endpoint_events();
    assert!(fixture.app().pending_generator_chains.is_empty());
    let running = &fixture.app().running_generator_chains[&channel];
    assert_eq!(
        (
            running.control.genuine_worker_id(),
            running.guard.genuine_worker_id()
        ),
        (owner.0, owner.0)
    );
    assert_eq!(
        fixture.app().timeline_generator_routes_state(&timeline),
        RequiredGeneratorRouteState::Ready
    );
    fixture
        .app_mut()
        .pending_generator_routes
        .insert(channel, requirement.mixer_track);
    assert_eq!(
        fixture.app().timeline_generator_routes_state(&timeline),
        RequiredGeneratorRouteState::Pending {
            channel_id: channel,
            plugin_instance_id: 41,
        }
    );
}
