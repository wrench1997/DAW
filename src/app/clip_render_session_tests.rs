//! Actual callback/identified-worker retirement at the post-decision intent
//! seam. Source recreation is explicitly mocked; there is no SDK or helper.
#[cfg(target_os = "linux")]
use super::*;
#[cfg(target_os = "linux")]
use crate::app::clip_render_glue::tests::authoritative_mock_restoration_factory;
#[cfg(target_os = "linux")]
use crate::app::clip_render_job::{ClipRenderPhase, tests::Fixture};
#[cfg(target_os = "linux")]
use crate::instrument_bounce::{BouncePluginMetadata, BouncePluginState};
#[cfg(target_os = "linux")]
use crate::plugins::plugin_runtime::{
    PluginChain, PluginPrepareConfig, test_capture_source::TestCaptureSource,
};

#[cfg(target_os = "linux")]
fn converted_and_restored(panic_on_shutdown: bool) -> Fixture {
    converted_and_restored_with_factory(if panic_on_shutdown {
        panicking_restoration_factory
    } else {
        authoritative_mock_restoration_factory
    })
}
#[cfg(target_os = "linux")]
fn converted_and_restored_with_factory(
    factory: crate::app::clip_render_glue::RestorationTestFactory,
) -> Fixture {
    let mut fixture = Fixture::new();
    fixture.app_mut().clip_restoration_test_factory = Some(factory);
    fixture.begin();
    fixture.until(|fixture| {
        fixture
            .app()
            .clip_render_job
            .as_ref()
            .is_some_and(|job| job.phase() == ClipRenderPhase::Complete)
    });
    assert!(fixture.original_source_joined());
    fixture.app_mut().undo();
    fixture.until(|fixture| {
        !fixture.app().clip_restoration_playback_pending
            && !fixture.app().clip_restoration_failed
            && fixture.app().timeline_audio_matches_desired()
    });
    assert_eq!(fixture.app().running_generator_chains.len(), 1);
    assert_eq!(fixture.app().clip_render_restorations.len(), 1);
    assert_eq!(fixture.app().retained_clip_media.len(), 1);
    fixture
}
#[cfg(target_os = "linux")]
fn panicking_restoration_factory(
    candidate: &Project,
    instance_id: u64,
    _spec: PluginLoadSpec,
    config: PluginPrepareConfig,
    metadata: &BouncePluginMetadata,
) -> Result<PluginChain, String> {
    let source = candidate
        .plugin_instances
        .iter()
        .find(|source| source.id == instance_id)
        .ok_or("actual restoration candidate disappeared")?;
    let (chain, handle) = TestCaptureSource::new(
        instance_id,
        BouncePluginState {
            metadata: metadata.clone(),
            opaque_state: source.opaque_state.clone(),
            parameter_bases: source.parameters.clone(),
            native_generation: 0,
            native_dirty_revision: 0,
        },
        config,
    )?;
    handle.panic_on_shutdown();
    drop(handle);
    Ok(chain)
}
#[cfg(target_os = "linux")]
fn reach_removed_but_unpolled_join(fixture: &mut Fixture) {
    fixture.app_mut().drive_clip_session_retirement(); // Queue old-epoch StopAll.
    fixture.pump_callback_only();
    fixture.app_mut().drive_clip_session_retirement(); // Queue exact expected removal.
    assert!(
        fixture
            .app()
            .clip_session_retirement
            .as_ref()
            .unwrap()
            .sources
            .iter()
            .all(|source| source.queued)
    );
    fixture.pump_callback_only();
    fixture.app_mut().poll_generator_endpoint_events();
    let stage = fixture.app().clip_session_retirement.as_ref().unwrap();
    assert!(
        stage
            .sources
            .iter()
            .all(|source| source.acknowledged && source.source.is_some() && !source.joined)
    );
    assert!(fixture.app().running_generator_chains.is_empty());
    assert!(fixture.app().pending_generator_removals.is_empty());
    assert!(!fixture.app().can_cancel_clip_session_retirement());
    assert!(!fixture.app_mut().cancel_clip_session_retirement());
}

#[cfg(target_os = "linux")]
#[test]
fn actual_restored_source_new_intent_waits_for_callback_then_exact_join() {
    let mut fixture = converted_and_restored(false);
    let before = fixture.bytes();
    let session = fixture.app().project_session;
    let bytes = fixture.app().retained_clip_pcm_bytes;
    fixture.app_mut().commit_project_intent(ProjectIntent::New);
    assert!(fixture.app().clip_session_retirement_pending());
    assert_eq!(fixture.bytes(), before);
    assert_eq!(fixture.app().project_session, session);
    fixture.app_mut().toggle_play();
    assert!(!fixture.app().playing);
    reach_removed_but_unpolled_join(&mut fixture);
    assert_eq!(fixture.app().project_session, session);
    assert_eq!(fixture.bytes(), before);
    assert_eq!(fixture.app().clip_render_restorations.len(), 1);
    assert_eq!(fixture.app().retained_clip_pcm_bytes, bytes);
    fixture.until(|fixture| !fixture.app().clip_session_retirement_pending());
    assert_ne!(fixture.app().project_session, session);
    assert!(fixture.app().clip_render_restorations.is_empty());
    assert!(fixture.app().retained_clip_media.is_empty());
    assert_eq!(fixture.app().retained_clip_pcm_bytes, 0);
    assert!(fixture.app().running_generator_chains.is_empty());
}

#[cfg(target_os = "linux")]
#[test]
fn actual_restored_source_quit_intent_does_not_arm_exit_before_removal_and_join() {
    let mut fixture = converted_and_restored(false);
    let before = fixture.bytes();
    fixture.app_mut().commit_project_intent(ProjectIntent::Quit);
    assert!(!fixture.app().project_lifecycle.exit_is_armed());
    reach_removed_but_unpolled_join(&mut fixture);
    assert!(!fixture.app().project_lifecycle.exit_is_armed());
    assert_eq!(fixture.bytes(), before);
    assert_eq!(fixture.app().clip_render_restorations.len(), 1);
    fixture.until(|fixture| fixture.app().project_lifecycle.exit_is_armed());
    assert!(!fixture.app().clip_session_retirement_pending());
    assert_eq!(fixture.bytes(), before); // Quit never changes the Project itself.
    assert!(fixture.app().running_generator_chains.is_empty());
}

#[cfg(target_os = "linux")]
#[test]
fn actual_session_retirement_queue_full_keeps_old_bindings_and_retries_same_source() {
    let mut fixture = converted_and_restored(false);
    let before = fixture.bytes();
    let session = fixture.app().project_session;
    let endpoint = fixture
        .app()
        .running_generator_chains
        .values()
        .next()
        .unwrap()
        .endpoint_id;
    let mut full = false;
    for _ in 0..=64 {
        if !fixture
            .app_mut()
            .audio
            .as_mut()
            .unwrap()
            .command(AudioCommand::SetMaster(1.0))
        {
            full = true;
            break;
        }
    }
    assert!(full);
    fixture.app_mut().commit_project_intent(ProjectIntent::New);
    fixture.app_mut().drive_clip_session_retirement();
    let stage = fixture.app().clip_session_retirement.as_ref().unwrap();
    assert_eq!(stage.admission, SessionAdmission::NotSent);
    assert!(stage.sources.iter().all(|source| !source.queued));
    assert_eq!(stage.sources[0].endpoint_id, endpoint);
    assert_eq!(fixture.bytes(), before);
    assert_eq!(fixture.app().project_session, session);
    assert_eq!(fixture.app().clip_render_restorations.len(), 1);
    fixture.until(|fixture| !fixture.app().clip_session_retirement_pending());
    assert_ne!(fixture.app().project_session, session);
}

#[cfg(target_os = "linux")]
#[test]
fn actual_session_retirement_engine_loss_retains_exact_guard_and_does_not_change_session() {
    let mut fixture = converted_and_restored(false);
    let before = fixture.bytes();
    let session = fixture.app().project_session;
    fixture.app_mut().commit_project_intent(ProjectIntent::New);
    fixture.app_mut().drive_clip_session_retirement();
    fixture.pump_callback_only();
    fixture.app_mut().drive_clip_session_retirement();
    let source = fixture
        .app_mut()
        .running_generator_chains
        .drain()
        .next()
        .unwrap()
        .1;
    assert!(fixture.app_mut().retain_clip_session_source(source).is_ok());
    let old_engine = fixture.app_mut().audio.take();
    fixture.app_mut().drive_clip_session_retirement();
    assert!(
        fixture
            .app()
            .clip_session_retirement_reason()
            .unwrap()
            .contains("unknown")
    );
    assert!(fixture.app().clip_operation_fence_active());
    assert_eq!(fixture.app().project_session, session);
    assert_eq!(fixture.bytes(), before);
    assert_eq!(fixture.app().clip_render_restorations.len(), 1);
    assert_eq!(fixture.app().retained_clip_media.len(), 1);
    // Even returning the exact test engine cannot erase an unknown outcome or
    // turn termination into the missing callback acknowledgment.
    fixture.app_mut().audio = old_engine;
    fixture.app_mut().commit_project_intent(ProjectIntent::Quit);
    assert!(!fixture.app().project_lifecycle.exit_is_armed());
    assert!(fixture.app().clip_session_retirement_pending());
}

#[cfg(target_os = "linux")]
#[test]
fn actual_joined_source_shutdown_panic_keeps_session_intent_failed_paused() {
    let mut fixture = converted_and_restored(true);
    let before = fixture.bytes();
    let session = fixture.app().project_session;
    fixture.app_mut().commit_project_intent(ProjectIntent::Quit);
    fixture.until(|fixture| {
        fixture
            .app()
            .clip_session_retirement
            .as_ref()
            .is_some_and(|stage| stage.failure.is_some())
    });
    let stage = fixture.app().clip_session_retirement.as_ref().unwrap();
    assert!(stage.failure.as_deref().unwrap().contains("panicked"));
    assert!(
        stage
            .sources
            .iter()
            .all(|source| source.acknowledged && source.joined && source.source.is_none())
    );
    assert!(!fixture.app().project_lifecycle.exit_is_armed());
    assert!(fixture.app().clip_operation_fence_active());
    assert_eq!(fixture.app().project_session, session);
    assert_eq!(fixture.bytes(), before);
    assert_eq!(fixture.app().clip_render_restorations.len(), 1);
    fixture.app_mut().toggle_play();
    assert!(!fixture.app().playing);
}

#[cfg(target_os = "linux")]
#[test]
fn actual_open_is_prepared_once_before_removal_and_does_not_reread_after_join() {
    let mut fixture = converted_and_restored(false);
    let before = fixture.bytes();
    let session = fixture.app().project_session;
    let path = fixture.directory().join("prepared-next.citrus");
    let mut next = Project::blank();
    next.name = "Prepared Open exact Project".into();
    next.save(&path).unwrap();
    fixture
        .app_mut()
        .commit_project_intent(ProjectIntent::Open { path: path.clone() });
    let stage = fixture.app().clip_session_retirement.as_ref().unwrap();
    assert!(matches!(&stage.intent, ProjectIntent::Open { path: accepted } if accepted == &path));
    assert_eq!(
        stage.prepared_open.as_ref().unwrap().name,
        "Prepared Open exact Project"
    );
    assert_eq!(stage.admission, SessionAdmission::NotSent);
    assert_eq!(fixture.bytes(), before);
    let mut changed = Project::blank();
    changed.name = "File changed after accepted Open".into();
    changed.save(&path).unwrap(); // A second read after retirement must not occur.
    reach_removed_but_unpolled_join(&mut fixture);
    assert_eq!(fixture.app().project_session, session);
    fixture.until(|fixture| !fixture.app().clip_session_retirement_pending());
    assert_ne!(fixture.app().project_session, session);
    assert_eq!(fixture.app().project.name, "Prepared Open exact Project");
    assert_eq!(fixture.app().saved_path.as_deref(), Some(path.as_path()));
}

#[cfg(target_os = "linux")]
#[test]
fn actual_open_preflight_failure_retains_exact_intent_without_retiring_source() {
    let mut fixture = converted_and_restored(false);
    let before = fixture.bytes();
    let path = fixture.directory().join("missing-next.citrus");
    let session = fixture.app().project_session;
    fixture
        .app_mut()
        .commit_project_intent(ProjectIntent::Open { path: path.clone() });
    let stage = fixture.app().clip_session_retirement.as_ref().unwrap();
    assert!(matches!(&stage.intent, ProjectIntent::Open { path: accepted } if accepted == &path));
    assert!(
        stage
            .failure
            .as_deref()
            .unwrap()
            .contains("Open could not be prepared")
    );
    assert!(stage.prepared_open.is_none());
    assert_eq!(stage.admission, SessionAdmission::NotSent);
    assert!(
        stage
            .sources
            .iter()
            .all(|source| !source.queued && !source.acknowledged && source.source.is_none())
    );
    assert_eq!(fixture.app().running_generator_chains.len(), 1);
    fixture.app_mut().drive_clip_session_retirement();
    assert_eq!(fixture.app().project_session, session);
    assert_eq!(fixture.bytes(), before);
    assert_eq!(fixture.app().clip_render_restorations.len(), 1);
    assert!(fixture.app().pending_generator_removals.is_empty());
    fixture.app_mut().commit_project_intent(ProjectIntent::Quit);
    assert!(
        matches!(&fixture.app().clip_session_retirement.as_ref().unwrap().intent, ProjectIntent::Open { path: accepted } if accepted == &path)
    );
    assert!(!fixture.app().project_lifecycle.exit_is_armed());
    assert!(fixture.app().can_cancel_clip_session_retirement());
    assert!(fixture.app_mut().cancel_clip_session_retirement());
    assert!(!fixture.app().clip_session_retirement_pending());
    assert!(!fixture.app().playing);
    assert_eq!(fixture.bytes(), before);

    std::fs::write(&path, b"not a Project JSON document").unwrap();
    fixture
        .app_mut()
        .commit_project_intent(ProjectIntent::Open { path: path.clone() });
    let stage = fixture.app().clip_session_retirement.as_ref().unwrap();
    assert!(
        stage
            .failure
            .as_deref()
            .unwrap()
            .contains("Open could not be prepared")
    );
    assert!(matches!(&stage.intent, ProjectIntent::Open { path: accepted } if accepted == &path));
    assert!(stage.sources.iter().all(|source| !source.queued));
    assert_eq!(fixture.app().running_generator_chains.len(), 1);
    assert!(fixture.app_mut().cancel_clip_session_retirement());
    assert_eq!(fixture.bytes(), before);
}

#[cfg(target_os = "linux")]
#[test]
fn early_session_admission_refusal_is_not_sent_and_can_be_dismissed_without_targets() {
    let mut fixture = converted_and_restored(false);
    let before = fixture.bytes();
    let channel = fixture.app().project.channels[0].id;
    let session = fixture.app().project_session;
    let request = fixture
        .app()
        .audio
        .as_ref()
        .unwrap()
        .test_transport_request_id();
    // This is the existing App topology-settling admission seam. It rejects
    // before target reservation or any runtime command/guard transfer.
    fixture
        .app_mut()
        .deferred_generator_rebuilds
        .insert(channel);
    fixture.app_mut().commit_project_intent(ProjectIntent::New);
    let stage = fixture.app().clip_session_retirement.as_ref().unwrap();
    assert!(!stage.preflight_completed);
    assert_eq!(stage.admission, SessionAdmission::NotSent);
    assert!(stage.sources.is_empty());
    assert!(stage.failure.is_some());
    assert!(matches!(stage.intent, ProjectIntent::New));
    assert_eq!(
        fixture
            .app()
            .audio
            .as_ref()
            .unwrap()
            .test_transport_request_id(),
        request
    );
    assert!(fixture.app().can_cancel_clip_session_retirement());
    assert!(fixture.app_mut().cancel_clip_session_retirement());
    assert!(!fixture.app().clip_session_retirement_pending());
    fixture
        .app_mut()
        .deferred_generator_rebuilds
        .remove(&channel);
    assert_eq!(fixture.app().project_session, session);
    assert_eq!(fixture.bytes(), before);
    assert_eq!(fixture.app().running_generator_chains.len(), 1);
    assert!(!fixture.app().playing);
}

#[cfg(target_os = "linux")]
fn faulting_restoration_factory(
    candidate: &Project,
    instance_id: u64,
    _spec: PluginLoadSpec,
    config: PluginPrepareConfig,
    metadata: &BouncePluginMetadata,
) -> Result<PluginChain, String> {
    let source = candidate
        .plugin_instances
        .iter()
        .find(|source| source.id == instance_id)
        .ok_or("actual restored source disappeared")?;
    let (chain, handle) = TestCaptureSource::new(
        instance_id,
        BouncePluginState {
            metadata: metadata.clone(),
            opaque_state: source.opaque_state.clone(),
            parameter_bases: source.parameters.clone(),
            native_generation: 0,
            native_dirty_revision: 0,
        },
        config,
    )?;
    handle
        .fail_capture_after_gate("actual restored backend fault before session retirement".into());
    drop(handle);
    Ok(chain)
}
#[cfg(target_os = "linux")]
fn already_faulted_source_intent(intent: ProjectIntent) {
    let mut fixture = converted_and_restored_with_factory(faulting_restoration_factory);
    let channel = fixture.app().project.channels[0].id;
    let epoch = fixture
        .app()
        .audio
        .as_ref()
        .unwrap()
        .snapshot()
        .transport_epoch;
    let instance = fixture.app().running_generator_chains[&channel].instance_id;
    let endpoint = fixture.app().running_generator_chains[&channel].endpoint_id;
    let receiver = fixture.app().running_generator_chains[&channel]
        .control
        .request_instrument_bounce_state(instance, epoch, vec![17])
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let outcome = loop {
        match receiver.try_recv() {
            Ok(outcome) => break outcome,
            Err(mpsc::TryRecvError::Empty) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(1))
            }
            other => panic!("actual bounded source fault receipt unavailable: {other:?}"),
        }
    };
    assert!(receiver.termination_acknowledged());
    assert!(matches!(
        outcome,
        Err(crate::instrument_bounce::BounceCaptureError::BackendFailed(
            _
        ))
    ));
    drop(receiver);
    let source = &fixture.app().running_generator_chains[&channel];
    let inactive = source.control.plugin_endpoint_snapshot().unwrap();
    assert_eq!(inactive.manifest().instance_ids[0], Some(instance));
    assert_eq!(inactive.active_mask(), 0);
    assert!(source.control.stats().faults > 0);
    fixture.app_mut().poll_plugin_runtime_events();
    let before = fixture.bytes();
    let session = fixture.app().project_session;
    let quit = matches!(intent, ProjectIntent::Quit);
    fixture.app_mut().commit_project_intent(intent);
    let stage = fixture.app().clip_session_retirement.as_ref().unwrap();
    assert!(
        stage.preflight_completed,
        "retirement must not inherit conversion's healthy-active requirement"
    );
    assert!(stage.failure.is_none());
    assert_eq!(stage.sources[0].endpoint_id, endpoint);
    reach_removed_but_unpolled_join(&mut fixture);
    assert_eq!(fixture.bytes(), before);
    assert_eq!(fixture.app().project_session, session);
    assert!(!fixture.app().project_lifecycle.exit_is_armed());
    let stage = fixture.app().clip_session_retirement.as_ref().unwrap();
    assert_eq!(
        stage.sources[0].source.as_ref().unwrap().endpoint_id,
        endpoint
    );
    assert_eq!(
        stage.sources[0]
            .source
            .as_ref()
            .unwrap()
            .control
            .plugin_endpoint_snapshot()
            .unwrap()
            .active_mask(),
        0
    );
    fixture.until(|fixture| !fixture.app().clip_session_retirement_pending());
    if quit {
        assert!(fixture.app().project_lifecycle.exit_is_armed());
        assert_eq!(fixture.bytes(), before);
    } else {
        assert_ne!(fixture.app().project_session, session);
    }
    assert!(fixture.app().running_generator_chains.is_empty());
}
#[cfg(target_os = "linux")]
#[test]
fn actual_already_faulted_bound_source_new_still_waits_for_exact_removal_and_join() {
    already_faulted_source_intent(ProjectIntent::New);
}
#[cfg(target_os = "linux")]
#[test]
fn actual_already_faulted_bound_source_quit_still_waits_for_exact_removal_and_join() {
    already_faulted_source_intent(ProjectIntent::Quit);
}

#[cfg(target_os = "linux")]
#[test]
fn dirty_discard_quit_cancel_preserves_old_session_and_recovery_authority() {
    let mut fixture = converted_and_restored(false);
    let recovery = fixture.directory().join("old-session-recovery.citrus");
    fixture.app().project.save(&recovery).unwrap();
    let recovery_bytes = std::fs::read(&recovery).unwrap();
    fixture.app_mut().autosave_path = Some(recovery.clone());
    fixture.app_mut().recovery_available = true;
    fixture.app_mut().suppress_drop_recovery = false;
    fixture.app_mut().dirty = true;
    let before = fixture.bytes();
    let session = fixture.app().project_session;
    fixture.app_mut().project_lifecycle = ProjectLifecycle::AwaitingDecision {
        intent: ProjectIntent::Quit,
        failure: None,
    };
    fixture.app_mut().discard_project_for_lifecycle();
    let stage = fixture.app().clip_session_retirement.as_ref().unwrap();
    assert_eq!(stage.admission, SessionAdmission::NotSent);
    assert!(matches!(stage.intent, ProjectIntent::Quit));
    assert_eq!(fixture.app().pending_discard_quit_session, Some(session));
    assert!(!fixture.app().suppress_drop_recovery);
    assert!(fixture.app().recovery_available);
    assert_eq!(std::fs::read(&recovery).unwrap(), recovery_bytes);
    assert!(fixture.app_mut().cancel_clip_session_retirement());
    assert_eq!(fixture.app().pending_discard_quit_session, None);
    assert!(!fixture.app().project_lifecycle.exit_is_armed());
    assert!(!fixture.app().suppress_drop_recovery);
    assert!(fixture.app().recovery_available);
    assert!(fixture.app().dirty);
    assert_eq!(fixture.app().project_session, session);
    assert_eq!(fixture.bytes(), before);
    assert_eq!(std::fs::read(&recovery).unwrap(), recovery_bytes);
    assert_eq!(fixture.app().running_generator_chains.len(), 1);
    assert!(!fixture.app().playing);
}

#[cfg(target_os = "linux")]
#[test]
fn incomplete_open_preflight_still_drains_an_exact_guard_transferred_on_engine_loss() {
    let mut fixture = converted_and_restored(false);
    let before = fixture.bytes();
    let path = fixture
        .directory()
        .join("missing-before-engine-loss.citrus");
    let channel = fixture.app().project.channels[0].id;
    let request = fixture
        .app()
        .audio
        .as_ref()
        .unwrap()
        .test_transport_request_id();
    fixture
        .app_mut()
        .commit_project_intent(ProjectIntent::Open { path });
    assert!(
        !fixture
            .app()
            .clip_session_retirement
            .as_ref()
            .unwrap()
            .preflight_completed
    );
    let source = fixture
        .app_mut()
        .running_generator_chains
        .remove(&channel)
        .unwrap();
    assert!(fixture.app_mut().retain_clip_session_source(source).is_ok());
    let old_engine = fixture.app_mut().audio.take();
    assert!(!fixture.app().can_cancel_clip_session_retirement());
    let deadline = Instant::now() + Duration::from_secs(5);
    while !fixture
        .app()
        .clip_session_retirement
        .as_ref()
        .unwrap()
        .sources[0]
        .joined
    {
        assert!(
            Instant::now() < deadline,
            "transferred preflight source did not actually join"
        );
        fixture.app_mut().drive_clip_session_retirement();
        std::thread::sleep(Duration::from_millis(1));
    }
    let stage = fixture.app().clip_session_retirement.as_ref().unwrap();
    assert_eq!(stage.admission, SessionAdmission::NotSent);
    assert!(!stage.sources[0].queued);
    assert!(!stage.sources[0].acknowledged);
    assert!(stage.sources[0].source.is_none());
    assert!(!fixture.app().can_cancel_clip_session_retirement());
    assert!(fixture.app().pending_generator_removals.is_empty());
    assert_eq!(fixture.bytes(), before);
    assert_eq!(
        old_engine.as_ref().unwrap().test_transport_request_id(),
        request
    );
    fixture.app_mut().audio = old_engine;
}
