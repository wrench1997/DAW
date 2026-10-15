//! Mock identity through actual worker Ready/callback Installed, with retained fixture cleanup.
use super::*;
use crate::app::clip_render_job::tests::Fixture;
use crate::plugins::plugin_runtime::LoadedVst3Identity;

#[test]
fn loaded_generator_identity_survives_both_ready_installed_orders_without_persisting_metadata() {
    for ready_first in [false, true] {
        let fixture =
            Fixture::new_with_loaded_identity_for_test("Instrument|Synth", true, ready_first);
        let app = fixture.app();
        let channel = app.project.channels[0].id;
        let before = serde_json::to_value(&app.project).unwrap();
        let history = (
            app.history_fingerprint,
            app.project_fingerprint,
            app.dirty,
            app.undo_stack.len(),
            app.redo_stack.len(),
        );
        let cached = serde_json::to_value(&app.plugins).unwrap();
        let proof = app
            .loaded_generator_identity(channel)
            .expect("real Ready plus actual Installed");
        assert!(proof.matches_current(app));
        assert!(proof.identity().is_instrument_with_midi_input());
        assert_ne!(
            proof.identity().class_uid(),
            app.project.plugin_instances[0].uid
        );
        assert!(proof.identity().requested_class_uid().is_none());
        let reopened: Project = serde_json::from_value(before.clone()).unwrap();
        let restored = &reopened.plugin_instances[0];
        let spec = plugins::plugin_load_spec_from_instance(restored);
        assert!(
            spec.class_uid.is_none(),
            "ordinary saved modules still request the default class"
        );
        assert_eq!(spec.descriptor.path, restored.path);
        assert_eq!(spec.descriptor.id, restored.uid);
        assert_eq!(spec.initial_state, restored.opaque_state);
        assert_eq!(serde_json::to_value(&reopened).unwrap(), before);
        assert_eq!(serde_json::to_value(&app.project).unwrap(), before);
        assert_eq!(serde_json::to_value(&app.plugins).unwrap(), cached);
        assert_eq!(
            (
                app.history_fingerprint,
                app.project_fingerprint,
                app.dirty,
                app.undo_stack.len(),
                app.redo_stack.len()
            ),
            history
        );
    }
}

#[test]
fn loaded_generator_identity_rejects_wrong_session_channel_endpoint_instance_path_and_control() {
    let mut fixture = Fixture::new_with_loaded_identity_for_test("Instrument", true, true);
    let channel = fixture.app().project.channels[0].id;
    let proof = fixture.app().loaded_generator_identity(channel).unwrap();
    fixture.app_mut().project.plugin_instances[0].runtime_status = PluginRuntimeStatus::Unloaded;
    assert!(
        !proof.matches_current(fixture.app()),
        "prepared identity alone cannot bypass persisted-base restoration"
    );
    fixture.app_mut().project.plugin_instances[0].runtime_status = PluginRuntimeStatus::Loaded;
    for case in 0..6 {
        let mut changed = proof.clone();
        match case {
            0 => changed.project_session = changed.project_session.wrapping_add(1),
            1 => changed.channel_id = changed.channel_id.wrapping_add(1),
            2 => changed.endpoint_id = changed.endpoint_id.wrapping_add(1),
            3 => changed.instance_id = changed.instance_id.wrapping_add(1),
            4 => {
                changed.identity = LoadedVst3Identity::test_fixture(
                    "wrong-path.vst3".into(),
                    proof.identity.class_uid(),
                    proof.identity.version(),
                    "Instrument",
                    true,
                    false,
                )
            }
            _ => changed.slot = 1,
        }
        assert!(!changed.matches_current(fixture.app()), "case {case}");
    }
    // A second real control allocation is not the same owner, even when it has
    // the same slot/instance manifest. Its guard stays in the local chain owner.
    let (other, source) =
        crate::plugins::plugin_runtime::test_capture_source::TestCaptureSource::new(
            proof.instance_id,
            crate::app::clip_render_glue::tests::source_state(),
            crate::plugins::plugin_runtime::PluginPrepareConfig {
                sample_rate: 48_000.0,
                max_block_frames: MAX_MIXER_BLOCK_FRAMES,
            },
        )
        .unwrap();
    let mut changed = proof.clone();
    changed.control = other.control.owner_identity();
    let rejected = !changed.matches_current(fixture.app());
    source.release_gates();
    let joined = other.guard.shutdown_blocking(Duration::from_secs(5));
    assert_eq!(joined, plugins::plugin_runtime::ShutdownOutcome::Joined);
    assert!(rejected);
    fixture.app_mut().project.channels[0].instrument_plugin_instance_id = None;
    assert!(!proof.matches_current(fixture.app()));
    assert!(fixture.app().loaded_generator_identity(channel).is_none());
}

#[test]
fn generator_ready_is_one_shot_and_fault_or_late_removed_binding_cannot_requalify() {
    let mut fixture = Fixture::new_with_loaded_identity_for_test("Instrument", true, true);
    let app = fixture.app_mut();
    let channel = app.project.channels[0].id;
    let proof = app.loaded_generator_identity(channel).unwrap();
    let chain = app.running_generator_chains.get_mut(&channel).unwrap();
    let ready = RuntimeEvent::SlotReady {
        slot: 0,
        name: "a matching name cannot qualify a late event".into(),
        latency_samples: 0,
        tail_samples: 0,
        loaded_vst3_identity: Some(proof.identity.clone()),
        loaded_vst3_identity_error: None,
    };
    chain.readiness.observe(
        &app.project,
        app.project_session,
        chain.project_session,
        channel,
        chain.endpoint_id,
        chain.instance_id,
        &chain.control,
        &RuntimeEvent::SlotFault {
            slot: 0,
            message: "actual source fault".into(),
        },
    );
    chain.readiness.observe(
        &app.project,
        app.project_session,
        chain.project_session,
        channel,
        chain.endpoint_id,
        chain.instance_id,
        &chain.control,
        &ready,
    );
    assert!(app.loaded_generator_identity(channel).is_none());
    assert!(!proof.matches_current(app));
}

#[test]
fn actual_stopped_worker_invalidates_identity_before_shutdown_event_is_consumed() {
    let mut fixture = Fixture::new_with_loaded_identity_for_test("Instrument", true, true);
    let channel = fixture.app().project.channels[0].id;
    let proof = fixture.app().loaded_generator_identity(channel).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let chain = fixture
            .app_mut()
            .running_generator_chains
            .get_mut(&channel)
            .unwrap();
        if chain.guard.poll_shutdown_retained()
            == plugins::plugin_runtime::RetainedShutdownPoll::Joined
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "actual worker shutdown not joined"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    // Do not poll runtime events: queued/dropped ShutdownComplete is not needed
    // for the control owner's observed stopped bit to invalidate this proof.
    assert!(matches!(
        &fixture.app().running_generator_chains[&channel].readiness,
        GeneratorReadiness::Ready(Some(_), None)
    ));
    assert!(!proof.matches_current(fixture.app()));
    assert!(fixture.app().loaded_generator_identity(channel).is_none());
}

#[test]
fn first_unavailable_or_rejected_ready_never_upgrades_on_duplicate_after_binding_recovers() {
    let fixture = Fixture::new_with_loaded_identity_for_test("Instrument", true, true);
    let app = fixture.app();
    let channel = app.project.channels[0].id;
    let chain = &app.running_generator_chains[&channel];
    let proof = app.loaded_generator_identity(channel).unwrap();
    let ready = RuntimeEvent::SlotReady {
        slot: 0,
        name: "synthetic duplicate state-machine input".into(),
        latency_samples: 0,
        tail_samples: 0,
        loaded_vst3_identity: Some(proof.identity.clone()),
        loaded_vst3_identity_error: None,
    };
    // Pure observe matrix only. These events never enter a worker/callback
    // queue and cannot replace the fixture's actual Ready or Installed receipt.
    for case in 0..4 {
        let mut first_project = app.project.clone();
        let mut owner_session = app.project_session;
        let mut first_ready = ready.clone();
        match case {
            0 => {
                first_ready = RuntimeEvent::SlotReady {
                    slot: 0,
                    name: "identity unavailable".into(),
                    latency_samples: 0,
                    tail_samples: 0,
                    loaded_vst3_identity: None,
                    loaded_vst3_identity_error: Some("actual identity getter failed".into()),
                }
            }
            1 => first_project.channels[0].instrument_plugin_instance_id = None,
            2 => owner_session = owner_session.wrapping_add(1),
            _ => first_project.plugin_instances[0].path = "wrong-source.vst3".into(),
        }
        let mut readiness = GeneratorReadiness::default();
        readiness.observe(
            &first_project,
            app.project_session,
            owner_session,
            channel,
            chain.endpoint_id,
            chain.instance_id,
            &chain.control,
            &first_ready,
        );
        let GeneratorReadiness::Ready(None, Some(first_reason)) = &readiness else {
            panic!("first Ready must permanently record unavailable qualification: case {case}");
        };
        let first_reason = first_reason.clone();
        // The assignment/session/path is now exact and metadata is present,
        // but it is still the same owner's duplicate Ready, never a new load.
        readiness.observe(
            &app.project,
            app.project_session,
            chain.project_session,
            channel,
            chain.endpoint_id,
            chain.instance_id,
            &chain.control,
            &ready,
        );
        assert!(
            matches!(&readiness, GeneratorReadiness::Ready(None, Some(reason))
            if reason == &first_reason),
            "duplicate upgraded qualification: case {case}"
        );
    }
}
