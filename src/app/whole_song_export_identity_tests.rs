//! Whole-only loaded-identity admission; mock identity is not SDK evidence.
use super::*;
use crate::instrument_bounce::{BounceAudioBus, BouncePluginState};

const UID: &str = "ABCDEF019182FAEB566D624153675854";

fn identity(path: PathBuf, category: &str, midi_input: bool) -> LoadedVst3Identity {
    LoadedVst3Identity::test_fixture(path, UID, "fixture-1", category, midi_input, false)
}

#[test]
fn whole_unknown_role_plan_requires_actual_loaded_instrument_midi_and_exact_path() {
    let fixture = Fixture::new();
    let selected = options(&fixture);
    let mut project = fixture.app().project.clone();
    let channel = project.channels[0].id;
    // Preserve the old Instrument/None contract for custom backends. No fresh
    // registration or newly required runtime proof is added to that usage.
    assert!(pure_master_plan(&project, channel, selected.sample_rate, selected).is_ok());
    project.plugin_instances[0].role = PluginRole::Unknown;
    let before = serde_json::to_vec(&project).unwrap();
    assert!(pure_master_plan(&project, channel, selected.sample_rate, selected).is_err());
    let actual = identity(
        project.plugin_instances[0].path.clone(),
        "Instrument|Synth",
        true,
    );
    let plan = pure_master_plan_with_loaded_identity(
        &project,
        channel,
        selected.sample_rate,
        selected,
        Some(&actual),
    )
    .unwrap();
    assert!(plan.is_master_song());
    assert_eq!(plan.instance_id(), project.plugin_instances[0].id);
    for case in 0..4 {
        let mut candidate = project.clone();
        let proof = match case {
            0 => identity("different-source.vst3".into(), "Instrument|Synth", true),
            1 => identity(candidate.plugin_instances[0].path.clone(), "Fx", true),
            2 => identity(
                candidate.plugin_instances[0].path.clone(),
                "Instrument|Synth",
                false,
            ),
            _ => {
                candidate.plugin_instances[0].role = PluginRole::Effect;
                actual.clone()
            }
        };
        assert!(
            pure_master_plan_with_loaded_identity(
                &candidate,
                channel,
                selected.sample_rate,
                selected,
                Some(&proof),
            )
            .is_err(),
            "unsupported source case {case}"
        );
    }
    assert_eq!(serde_json::to_vec(&project).unwrap(), before);
    assert_eq!(project.plugin_instances[0].role, PluginRole::Unknown);
}

fn captured_metadata() -> BouncePluginMetadata {
    BouncePluginMetadata {
        class_uid: UID.into(),
        version: "fixture-1".into(),
        name: "Actual capture mock".into(),
        category: "Instrument|Synth".into(),
        event_input_buses: vec![(16, true, Some(true))],
        event_output_buses: Vec::new(),
        input_buses: Vec::new(),
        output_buses: vec![BounceAudioBus::new(2, true, Some(true))],
        latency_samples: 0,
        tail_samples: 0,
    }
}

#[test]
fn whole_reviewed_loaded_identity_rejects_capture_class_or_version_drift() {
    let proof = identity("mock-source.vst3".into(), "Instrument|Synth", true);
    let original = captured_metadata();
    original.validate(48_000).unwrap();
    assert!(check_captured_loaded_identity(Some(&proof), &original).is_ok());
    assert!(check_captured_loaded_identity(None, &original).is_ok());
    for class_drift in [false, true] {
        let mut actual = original.clone();
        if class_drift {
            actual.class_uid = "00112233445566778899AABBCCDDEEFF".into();
        } else {
            actual.version = "fixture-2".into();
        }
        actual.validate(48_000).unwrap();
        assert!(
            check_captured_loaded_identity(Some(&proof), &actual)
                .unwrap_err()
                .contains("class/version"),
            "otherwise valid capture drift must fail before the clone starts"
        );
        // Historical Instrument/None callers retain their original capture
        // validation path; this extra review anchor does not invent one.
        assert!(check_captured_loaded_identity(None, &actual).is_ok());
    }
}

fn unknown_fixture(category: &str, midi_input: bool, ready_before_install: bool) -> Fixture {
    let mut fixture =
        Fixture::new_with_loaded_identity_for_test(category, midi_input, ready_before_install);
    // Model setup only: admission must not promote the persisted Unknown role.
    // The identity itself came through the actual worker Ready and callback
    // Installed queues, in the order selected by this fixture constructor.
    {
        let app = fixture.app_mut();
        app.project.plugin_instances[0].role = PluginRole::Unknown;
        app.history_snapshot = app.project.clone();
        app.history_fingerprint = project_fingerprint(&app.project);
        app.project_fingerprint = app.history_fingerprint;
        app.request_immediate_timeline_compile();
    }
    settle_initial_source(&mut fixture);
    let channel = fixture.app().project.channels[0].id;
    assert!(fixture.app().loaded_generator_identity(channel).is_some());
    fixture
}

#[test]
fn actual_unknown_whole_export_uses_ready_identity_in_both_callback_event_orders_without_model_edits()
 {
    for ready_before_install in [true, false] {
        let mut fixture = unknown_fixture("Instrument|Synth", true, ready_before_install);
        let before = fixture.bytes();
        let source = exact_source(&fixture);
        let (path, started) = begin(&mut fixture);
        until(&mut fixture, |fixture| {
            fixture
                .app()
                .whole_song_export_job
                .as_ref()
                .is_some_and(|job| !job.pending())
        });
        let job = fixture.app().whole_song_export_job.as_ref().unwrap();
        assert!(job.phase == WholeExportPhase::Complete);
        let proof = job.loaded_identity.as_ref().unwrap();
        assert!(proof.matches_current(fixture.app()));
        assert_eq!(proof.identity().class_uid(), UID);
        assert_eq!(proof.identity().version(), "fixture-1");
        let stop = job.actor.stop_preparation.unwrap();
        assert_eq!(
            stop.activation.unwrap().committed_epoch,
            Some(job.actor.binding.unwrap().stamp.epoch)
        );
        assert_eq!(fixture.source_handle().capture_count(), 1);
        assert!(started.load(Ordering::Acquire));
        let Some(MasterSongRenderOutcome::Complete(report)) = &job.outcome else {
            panic!("actual Unknown master outcome missing");
        };
        let wav = crate::wav::read_wav(&path).unwrap();
        assert_eq!(wav.frame_count(), report.master_output.frames);
        assert!(wav.samples.iter().any(|sample| sample.abs() > 0.01));
        assert_eq!(fixture.bytes(), before);
        assert_eq!(exact_source(&fixture), source);
        assert_eq!(
            fixture.app().project.plugin_instances[0].role,
            PluginRole::Unknown
        );
        assert!(fixture.app().clip_render_job.is_none());
        assert!(
            fixture.app().retained_clip_media.is_empty()
                && fixture.app().clip_render_restorations.is_empty()
        );
    }
}

#[test]
fn actual_unknown_whole_review_rejects_ready_effect_or_missing_midi_before_stop_or_capture() {
    for (category, midi_input) in [("Fx", true), ("Instrument|Synth", false)] {
        let fixture = unknown_fixture(category, midi_input, true);
        let before = fixture.bytes();
        let source = exact_source(&fixture);
        let resets = fixture.source_handle().reset_count();
        let request = fixture.app().audio.as_ref().unwrap().transport_request_id();
        let reason = fixture
            .app()
            .prepare_whole_song_export_review(options(&fixture), true)
            .err()
            .expect("actual loaded category and MIDI input must authorize Unknown");
        assert!(reason.contains("Instrument"));
        assert!(fixture.app().whole_song_export_job.is_none());
        assert_eq!(fixture.source_handle().capture_count(), 0);
        assert_eq!(fixture.source_handle().reset_count(), resets);
        assert_eq!(
            fixture.app().audio.as_ref().unwrap().transport_request_id(),
            request
        );
        assert_eq!(fixture.bytes(), before);
        assert_eq!(exact_source(&fixture), source);
        assert!(!fixture.directory().join("whole-master.wav").exists());
    }
}

#[test]
fn actual_unknown_whole_missing_ready_identity_reports_reason_without_new_legacy_preconditions() {
    let mut fixture = Fixture::new();
    settle_initial_source(&mut fixture);
    let selected = options(&fixture);
    let channel = fixture.app().project.channels[0].id;
    assert!(fixture.app().loaded_generator_identity(channel).is_none());
    assert!(
        fixture
            .app()
            .prepare_whole_song_export_review(selected, true)
            .is_ok(),
        "legacy Instrument and custom backend None retains its Review contract"
    );
    fixture.app_mut().project.plugin_instances[0].role = PluginRole::Unknown;
    fixture.app_mut().request_immediate_timeline_compile();
    settle_initial_source(&mut fixture);
    let before = fixture.bytes();
    let resets = fixture.source_handle().reset_count();
    let request = fixture.app().audio.as_ref().unwrap().transport_request_id();
    let reason = fixture
        .app()
        .prepare_whole_song_export_review(selected, true)
        .err()
        .unwrap();
    assert_eq!(
        reason,
        fixture
            .app()
            .loaded_generator_identity_unavailable_reason(channel)
    );
    assert!(fixture.app().whole_song_export_job.is_none());
    assert_eq!(fixture.source_handle().capture_count(), 0);
    assert_eq!(fixture.source_handle().reset_count(), resets);
    assert_eq!(
        fixture.app().audio.as_ref().unwrap().transport_request_id(),
        request
    );
    assert_eq!(fixture.bytes(), before);
}

#[test]
fn actual_classified_instrument_with_ready_identity_preserves_legacy_review_and_capture_contract() {
    let mut fixture = Fixture::new_with_loaded_identity_for_test("Instrument|Synth", true, true);
    settle_initial_source(&mut fixture);
    let channel = fixture.app().project.channels[0].id;
    let optional_runtime_identity = fixture.app().loaded_generator_identity(channel).unwrap();
    assert_eq!(
        fixture.app().project.plugin_instances[0].role,
        PluginRole::Instrument
    );
    assert_eq!(optional_runtime_identity.identity().class_uid(), UID);
    assert_eq!(optional_runtime_identity.identity().version(), "fixture-1");
    let selected = options(&fixture);
    assert!(
        pure_master_plan(
            &fixture.app().project,
            channel,
            selected.sample_rate,
            selected
        )
        .is_ok()
    );
    let review = fixture
        .app()
        .prepare_whole_song_export_review(selected, true)
        .unwrap();
    assert!(
        review.loaded_identity.is_none(),
        "optional runtime identity must not tighten classified Instrument Review"
    );
    let before = fixture.bytes();
    let source = exact_source(&fixture);
    let mut metadata = captured_metadata();
    metadata.name = "Controller mock".into();
    metadata.class_uid = "00112233445566778899AABBCCDDEEFF".into();
    metadata.version = "fixture-2".into();
    metadata.validate(selected.sample_rate).unwrap();
    // The existing strict capture/clone metadata contract remains authoritative
    // for classified Instrument; the new Unknown-only Ready anchor is absent.
    fixture
        .source_handle()
        .set_capture_state(BouncePluginState {
            metadata,
            opaque_state: vec![7, 8, 9],
            parameter_bases: std::collections::BTreeMap::from([(17, 0.75)]),
            native_generation: 2,
            native_dirty_revision: 9,
        });
    let (path, started) = begin(&mut fixture);
    until(&mut fixture, |fixture| {
        fixture
            .app()
            .whole_song_export_job
            .as_ref()
            .is_some_and(|job| !job.pending())
    });
    let job = fixture.app().whole_song_export_job.as_ref().unwrap();
    assert!(job.phase == WholeExportPhase::Complete);
    assert!(job.loaded_identity.is_none() && job.capture_failure.is_none());
    assert_eq!(fixture.source_handle().capture_count(), 1);
    assert!(started.load(Ordering::Acquire));
    assert!(optional_runtime_identity.matches_current(fixture.app()));
    let Some(MasterSongRenderOutcome::Complete(report)) = &job.outcome else {
        panic!("legacy Instrument master outcome missing");
    };
    let wav = crate::wav::read_wav(&path).unwrap();
    assert_eq!(wav.frame_count(), report.master_output.frames);
    assert!(wav.samples.iter().any(|sample| sample.abs() > 0.01));
    assert_eq!(fixture.bytes(), before);
    assert_eq!(exact_source(&fixture), source);
}

#[test]
fn actual_unknown_whole_review_rejects_source_path_drift_while_destination_is_open() {
    let mut fixture = unknown_fixture("Instrument|Synth", true, true);
    let selected = options(&fixture);
    let review = fixture
        .app()
        .prepare_whole_song_export_review(selected, true)
        .unwrap();
    let proof = review.loaded_identity.as_ref().unwrap().clone();
    let source = exact_source(&fixture);
    let path = fixture.directory().join("existing-whole.wav");
    std::fs::write(&path, b"existing destination").unwrap();
    fixture.app_mut().project.plugin_instances[0].path = "changed-source.vst3".into();
    assert!(!proof.matches_current(fixture.app()));
    let before_rejection = fixture.bytes();
    let resets = fixture.source_handle().reset_count();
    let request = fixture.app().audio.as_ref().unwrap().transport_request_id();
    assert!(
        fixture
            .app_mut()
            .begin_whole_song_export(review, path.clone())
            .is_err()
    );
    assert!(fixture.app().whole_song_export_job.is_none());
    assert_eq!(fixture.source_handle().capture_count(), 0);
    assert_eq!(fixture.source_handle().reset_count(), resets);
    assert_eq!(
        fixture.app().audio.as_ref().unwrap().transport_request_id(),
        request
    );
    assert_eq!(std::fs::read(path).unwrap(), b"existing destination");
    assert_eq!(fixture.bytes(), before_rejection);
    assert_eq!(exact_source(&fixture), source);
}

#[test]
fn actual_unknown_whole_stale_proof_rejects_before_first_stop_all_reset_or_capture() {
    let mut fixture = unknown_fixture("Instrument|Synth", true, false);
    let source = exact_source(&fixture);
    let resets = fixture.source_handle().reset_count();
    let (path, started) = begin(&mut fixture);
    std::fs::write(&path, b"existing destination").unwrap();
    fixture.app_mut().project.plugin_instances[0].path = "changed-source.vst3".into();
    let before_rejection = fixture.bytes();
    let request = fixture.app().audio.as_ref().unwrap().transport_request_id();
    fixture.app_mut().drive_whole_song_export();
    let job = fixture.app().whole_song_export_job.as_ref().unwrap();
    assert!(job.phase == WholeExportPhase::Failed && !job.pending());
    assert!(
        job.failure
            .as_deref()
            .unwrap()
            .contains("actual loaded VST3 source or control owner changed")
    );
    assert!(
        !job.loaded_identity
            .as_ref()
            .unwrap()
            .matches_current(fixture.app())
    );
    assert!(job.actor.old_epoch.is_none() && job.actor.stop_preparation.is_none());
    assert!(!job.actor.owns_pending_capture());
    assert_eq!(fixture.source_handle().capture_count(), 0);
    assert_eq!(fixture.source_handle().reset_count(), resets);
    assert_eq!(
        fixture.app().audio.as_ref().unwrap().transport_request_id(),
        request
    );
    assert!(!started.load(Ordering::Acquire));
    assert_eq!(std::fs::read(path).unwrap(), b"existing destination");
    assert_eq!(fixture.bytes(), before_rejection);
    assert_eq!(exact_source(&fixture), source);
}

#[test]
fn actual_unknown_whole_stale_proof_retains_held_capture_owner_and_fence_until_ack() {
    let mut fixture = unknown_fixture("Instrument|Synth", true, false);
    let source = exact_source(&fixture);
    fixture.source_handle().pause_capture();
    let (path, started) = begin(&mut fixture);
    std::fs::write(&path, b"existing destination").unwrap();
    until(&mut fixture, |fixture| {
        fixture.source_handle().capture_count() == 1
    });
    fixture.app_mut().project.plugin_instances[0].path = "changed-source.vst3".into();
    let before_rejection = fixture.bytes();
    for _ in 0..3 {
        fixture.app_mut().drive_whole_song_export();
    }
    let job = fixture.app().whole_song_export_job.as_ref().unwrap();
    assert!(job.failure.is_some());
    assert!(job.actor.owns_pending_capture() && job.pending());
    assert!(
        !job.loaded_identity
            .as_ref()
            .unwrap()
            .matches_current(fixture.app())
    );
    assert!(fixture.app().whole_song_export_fence_active());
    assert!(!started.load(Ordering::Acquire));
    assert_eq!(std::fs::read(&path).unwrap(), b"existing destination");
    fixture.source_handle().release_gates();
    until(&mut fixture, |fixture| {
        !fixture.app().whole_song_export_fence_active()
    });
    let job = fixture.app().whole_song_export_job.as_ref().unwrap();
    assert!(job.phase == WholeExportPhase::Failed);
    assert!(!job.actor.owns_pending_capture());
    assert_eq!(fixture.source_handle().capture_count(), 1);
    assert!(!started.load(Ordering::Acquire));
    assert_eq!(std::fs::read(path).unwrap(), b"existing destination");
    assert_eq!(fixture.bytes(), before_rejection);
    assert_eq!(exact_source(&fixture), source);
}

#[test]
fn actual_unknown_whole_capture_class_or_version_drift_fails_before_clone_without_model_edits() {
    for class_drift in [false, true] {
        let mut fixture = unknown_fixture("Instrument|Synth", true, true);
        let before = fixture.bytes();
        let source = exact_source(&fixture);
        let mut metadata = captured_metadata();
        metadata.name = "Controller mock".into();
        if class_drift {
            metadata.class_uid = "00112233445566778899AABBCCDDEEFF".into();
        } else {
            metadata.version = "fixture-2".into();
        }
        metadata.validate(options(&fixture).sample_rate).unwrap();
        // Only the worker's otherwise-valid captured CID/version drifts. Keep
        // its real native stamps, opaque state and parameter authority intact.
        fixture
            .source_handle()
            .set_capture_state(BouncePluginState {
                metadata,
                opaque_state: vec![7, 8, 9],
                parameter_bases: std::collections::BTreeMap::from([(17, 0.75)]),
                native_generation: 2,
                native_dirty_revision: 9,
            });
        let (path, started) = begin(&mut fixture);
        std::fs::write(&path, b"existing destination").unwrap();
        until(&mut fixture, |fixture| {
            fixture
                .app()
                .whole_song_export_job
                .as_ref()
                .is_some_and(|job| !job.pending())
        });
        let job = fixture.app().whole_song_export_job.as_ref().unwrap();
        assert!(job.phase == WholeExportPhase::Failed);
        assert!(matches!(
            job.capture_failure.as_ref(),
            Some(CapturePollError::Invalid(reason)) if reason.contains("class/version")
        ));
        assert_eq!(fixture.source_handle().capture_count(), 1);
        assert!(!job.actor.owns_pending_capture());
        assert!(job.render.is_none() && !job.admission.is_busy());
        assert!(!started.load(Ordering::Acquire));
        let proof = job.loaded_identity.as_ref().unwrap();
        assert_eq!(proof.identity().class_uid(), UID);
        assert_eq!(proof.identity().version(), "fixture-1");
        assert!(proof.matches_current(fixture.app()));
        assert_eq!(std::fs::read(path).unwrap(), b"existing destination");
        assert_eq!(fixture.bytes(), before);
        assert_eq!(exact_source(&fixture), source);
    }
}
