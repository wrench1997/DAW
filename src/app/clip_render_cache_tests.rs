//! Actual controller publications supply every sealed PCM used below. No
//! fabricated receipt, replacement sample buffer, SDK, or file decoder is used.
use super::*;
#[cfg(any(target_os = "linux", windows))]
use crate::app::clip_render_job::{ClipRenderPhase, tests::Fixture};

#[cfg(any(target_os = "linux", windows))]
fn complete(fixture: &mut Fixture) -> u64 {
    fixture.begin();
    fixture.until(|fixture| {
        fixture
            .app()
            .clip_render_job
            .as_ref()
            .is_some_and(|job| job.phase() == ClipRenderPhase::Complete)
    });
    fixture.app().project.audio_assets[0].id
}

#[test]
fn exact_aggregate_arithmetic_and_separate_metadata_bounds() {
    assert_eq!(
        retained_budget(MAX_RETAINED_CLIP_PCM_BYTES - 8, 8, 0).unwrap(),
        MAX_RETAINED_CLIP_PCM_BYTES
    );
    assert!(retained_budget(MAX_RETAINED_CLIP_PCM_BYTES - 7, 8, 0).is_err());
    assert!(retained_budget(usize::MAX, 8, 0).is_err());
    assert!(retained_budget(0, 8, MAX_RETAINED_CLIP_ENTRIES).is_err());
    assert!(retained_budget(0, 8, MAX_RETAINED_CLIP_ENTRIES - 1).is_ok());
}

#[cfg(any(target_os = "linux", windows))]
#[test]
fn successful_commit_retains_actual_compact_producer_pcm_once() {
    let mut fixture = Fixture::new();
    let id = complete(&mut fixture);
    let app = fixture.app();
    let entry = &app.retained_clip_media[&id];
    let cache = &app.cached_audio_assets[&id];
    assert_eq!(entry.operation_id, 1);
    assert_eq!(entry.project_session, app.project_session);
    assert_eq!(entry.asset_id, id);
    assert_eq!(entry.pcm.binding().source.instance_id, 41);
    assert_eq!(entry.pcm.binding().start_frame, 24_000);
    assert_eq!(entry.pcm.binding().native_dirty_revision, 9);
    assert_eq!(entry.pcm.binding().sample_rate, 48_000);
    assert_eq!(entry.pcm.frames() * 2, entry.pcm.samples().len());
    assert!(entry.pcm.path().exists());
    assert!(entry.pcm.file_identity().bytes > 0);
    assert!(Arc::ptr_eq(entry.pcm.samples(), &cache.samples));
    let extra_handle = Arc::clone(entry.pcm.samples());
    let bytes = entry.charged_bytes;
    assert_eq!(app.retained_clip_pcm_bytes, bytes);
    assert_eq!(bytes, extra_handle.len() * std::mem::size_of::<f32>());
    assert_eq!(app.retained_clip_media.len(), 1);
    assert!(
        fixture
            .app_mut()
            .try_restore_retained_clip_asset(id)
            .unwrap()
    );
    assert_eq!(fixture.app().retained_clip_pcm_bytes, bytes);
}

#[cfg(any(target_os = "linux", windows))]
#[test]
fn retained_restore_refuses_id_path_rate_channel_frame_and_cache_substitution() {
    let mut fixture = Fixture::new();
    let id = complete(&mut fixture);
    let original = fixture.app().project.audio_assets[0].clone();
    let samples = Arc::clone(fixture.app().retained_clip_media[&id].pcm.samples());
    let bytes = fixture.app().retained_clip_pcm_bytes;
    for mutate in [
        (|asset: &mut AudioAsset| asset.path = PathBuf::from("/different/retained.wav"))
            as fn(&mut AudioAsset),
        |asset| asset.sample_rate += 1,
        |asset| asset.channels = 1,
        |asset| asset.bits_per_sample = 16,
        |asset| asset.frames += 1,
    ] {
        fixture.app_mut().cached_audio_assets.remove(&id);
        mutate(&mut fixture.app_mut().project.audio_assets[0]);
        assert!(
            fixture
                .app_mut()
                .try_restore_retained_clip_asset(id)
                .is_err()
        );
        assert!(!fixture.app().cached_audio_assets.contains_key(&id));
        assert_eq!(fixture.app().retained_clip_pcm_bytes, bytes);
        fixture.app_mut().project.audio_assets[0] = original.clone();
    }
    fixture
        .app_mut()
        .project
        .audio_assets
        .push(original.clone());
    assert!(
        fixture
            .app_mut()
            .try_restore_retained_clip_asset(id)
            .is_err()
    );
    fixture.app_mut().project.audio_assets.pop();
    assert!(
        fixture
            .app_mut()
            .try_restore_retained_clip_asset(id)
            .unwrap()
    );
    fixture
        .app_mut()
        .cached_audio_assets
        .get_mut(&id)
        .unwrap()
        .samples = Arc::from(vec![0.0; samples.len()]);
    assert!(
        fixture
            .app_mut()
            .try_restore_retained_clip_asset(id)
            .is_err()
    );
    fixture.app_mut().cached_audio_assets.remove(&id);
    assert!(
        fixture
            .app_mut()
            .try_restore_retained_clip_asset(id)
            .unwrap()
    );
    assert!(Arc::ptr_eq(
        &fixture.app().cached_audio_assets[&id].samples,
        &samples
    ));
    fixture.app_mut().project.audio_assets[0].id += 1;
    assert!(
        fixture
            .app_mut()
            .try_restore_retained_clip_asset(id)
            .is_err()
    );
    assert!(
        !fixture
            .app_mut()
            .try_restore_retained_clip_asset(id + 1)
            .unwrap()
    );
}

#[cfg(any(target_os = "linux", windows))]
#[test]
fn deleted_or_replaced_file_cannot_change_the_retained_session_pcm() {
    let mut fixture = Fixture::new();
    let id = complete(&mut fixture);
    let entry = &fixture.app().retained_clip_media[&id];
    let path = entry.pcm.path().to_path_buf();
    let samples = Arc::clone(entry.pcm.samples());
    let first = samples[0];
    let bytes = fixture.app().retained_clip_pcm_bytes;
    std::fs::write(&path, b"externally replaced, deliberately not WAV").unwrap();
    fixture.app_mut().cached_audio_assets.remove(&id);
    assert!(
        fixture
            .app_mut()
            .try_restore_retained_clip_asset(id)
            .unwrap()
    );
    assert!(Arc::ptr_eq(
        &samples,
        &fixture.app().cached_audio_assets[&id].samples
    ));
    assert_eq!(fixture.app().cached_audio_assets[&id].samples[0], first);
    std::fs::remove_file(&path).unwrap();
    fixture.app_mut().cached_audio_assets.remove(&id);
    assert!(
        fixture
            .app_mut()
            .try_restore_retained_clip_asset(id)
            .unwrap()
    );
    assert!(Arc::ptr_eq(
        &samples,
        &fixture.app().cached_audio_assets[&id].samples
    ));
    assert_eq!(fixture.app().retained_clip_pcm_bytes, bytes);
}

#[cfg(any(target_os = "linux", windows))]
#[test]
fn every_existing_asset_id_collision_refuses_without_overwrite() {
    // The first completed actual producer contributes the existing sealed owner.
    // Its source is joined before the second fixture is admitted. The second
    // fixture models a reused session asset ID, not a fabricated publication.
    let mut previous = Fixture::new();
    let id = complete(&mut previous);
    let old = previous.app_mut().retained_clip_media.remove(&id).unwrap();
    previous.app_mut().retained_clip_pcm_bytes = 0;
    let old_samples = Arc::clone(old.pcm.samples());
    drop(previous);
    let mut fixture = Fixture::new();
    let id2 = complete(&mut fixture);
    assert_eq!(id2, id);
    let new = fixture.app_mut().retained_clip_media.remove(&id).unwrap();
    fixture.app_mut().retained_clip_pcm_bytes = old.charged_bytes;
    fixture.app_mut().retained_clip_media.insert(id, old);
    let bytes = fixture.app().retained_clip_pcm_bytes;
    let refused = fixture
        .app_mut()
        .prepare_retained_clip_media(new.operation_id + 1, id, new.pcm)
        .err()
        .expect("all duplicate IDs must refuse");
    assert!(refused.message.contains("already reserved"));
    assert!(Arc::ptr_eq(
        &old_samples,
        fixture.app().retained_clip_media[&id].pcm.samples()
    ));
    assert_eq!(fixture.app().retained_clip_pcm_bytes, bytes);
    assert_eq!(fixture.app().retained_clip_media.len(), 1);

    // A reservation may not replace an entry which appears after preparation.
    // The returned private owner is the same actual verified allocation.
    let new_samples = Arc::clone(refused.pcm.samples());
    let old = fixture.app_mut().retained_clip_media.remove(&id).unwrap();
    fixture.app_mut().retained_clip_pcm_bytes = 0;
    let prepared = fixture
        .app_mut()
        .prepare_retained_clip_media(2, id, refused.pcm)
        .ok()
        .unwrap();
    fixture.app_mut().retained_clip_pcm_bytes = old.charged_bytes;
    fixture.app_mut().retained_clip_media.insert(id, old);
    let returned = fixture
        .app_mut()
        .install_retained_clip_media(prepared)
        .expect_err("late occupied slot must retain prepared owner");
    assert!(Arc::ptr_eq(&new_samples, returned.entry.pcm.samples()));
    assert!(Arc::ptr_eq(
        &old_samples,
        fixture.app().retained_clip_media[&id].pcm.samples()
    ));
    assert_eq!(fixture.app().retained_clip_pcm_bytes, bytes);
    assert_eq!(fixture.app().retained_clip_media.len(), 1);
}

#[test]
fn bounded_path_copy_refuses_oversize_and_relative_paths() {
    let prefix = std::env::temp_dir().join("bounded-path-é");
    assert!(prefix.is_absolute());
    let prefix = prefix.to_str().expect("test temporary path must be UTF-8");
    let padding = MAX_RETAINED_PATH_BYTES
        .checked_sub(prefix.len())
        .expect("test temporary path must fit within the byte bound");
    let exact = format!("{prefix}{}", "a".repeat(padding));
    assert_eq!(exact.len(), MAX_RETAINED_PATH_BYTES);
    assert!(Path::new(&exact).is_absolute());
    assert_eq!(
        copied_path(Path::new(&exact)).unwrap(),
        PathBuf::from(&exact)
    );
    let oversize = exact + "a";
    assert_eq!(oversize.len(), MAX_RETAINED_PATH_BYTES + 1);
    assert!(copied_path(Path::new(&oversize)).is_err());
    assert!(copied_path(Path::new("relative.wav")).is_err());
}

#[cfg(any(target_os = "linux", windows))]
#[test]
fn actual_generic_register_and_late_loader_completion_cannot_replace_sealed_pcm() {
    let mut fixture = Fixture::new();
    let id = complete(&mut fixture);
    let path = fixture.app().project.audio_assets[0].path.clone();
    let samples = Arc::clone(fixture.app().retained_clip_media[&id].pcm.samples());
    let before = fixture.bytes();
    let charged = fixture.app().retained_clip_pcm_bytes;
    let mut attempted = crate::wav::read_wav(&path).unwrap();
    attempted.samples.fill(0.75);
    attempted.metadata.sample_rate = 44_100;
    fixture.app_mut().register_audio_asset(id, attempted);
    assert!(Arc::ptr_eq(
        &samples,
        &fixture.app().cached_audio_assets[&id].samples
    ));
    assert_eq!(fixture.bytes(), before);
    assert_eq!(fixture.app().retained_clip_pcm_bytes, charged);

    // Deliver an actual ordinary loader-channel completion after conversion.
    // The incoming decoded WAV is genuine, but no generic decoder result is
    // allowed to replace the sealed producer's allocation or model metadata.
    let mut late = crate::wav::read_wav(&path).unwrap();
    late.samples.fill(-0.75);
    late.metadata.sample_rate = 96_000;
    let generation = fixture.app().audio_asset_generation;
    fixture
        .app_mut()
        .loading_audio_assets
        .insert(id, generation);
    fixture
        .app()
        .audio_asset_sender
        .send(Ok((generation, id, late)))
        .unwrap();
    fixture.app_mut().poll_audio_asset_loads();
    assert!(!fixture.app().loading_audio_assets.contains_key(&id));
    assert!(Arc::ptr_eq(
        &samples,
        &fixture.app().cached_audio_assets[&id].samples
    ));
    assert_eq!(fixture.bytes(), before);
    assert_eq!(fixture.app().retained_clip_pcm_bytes, charged);
    assert!(!fixture.app().failed_audio_assets.contains_key(&id));
    assert!(fixture.app().prepared_audio_asset_registrations.is_empty());

    fixture
        .app()
        .audio_asset_sender
        .send(Err(AudioAssetLoadFailure {
            generation,
            asset_id: id,
            message: "late decoder error cannot invalidate retained PCM".into(),
        }))
        .unwrap();
    fixture.app_mut().poll_audio_asset_loads();
    assert!(!fixture.app().failed_audio_assets.contains_key(&id));
    assert!(Arc::ptr_eq(
        &samples,
        &fixture.app().cached_audio_assets[&id].samples
    ));
}

#[cfg(any(target_os = "linux", windows))]
#[test]
fn actual_session_transition_releases_completed_retained_authority() {
    let mut fixture = Fixture::new();
    let id = complete(&mut fixture);
    assert!(fixture.original_source_joined());
    let old_session = fixture.app().project_session;
    assert!(fixture.app().retained_clip_media.contains_key(&id));
    assert!(fixture.app().retained_clip_pcm_bytes > 0);
    assert!(!fixture.app().clip_operation_fence_active());
    // Default Project contains no VST3 instances; this never loads a helper/SDK.
    let replacement = Project::default();
    assert!(replacement.plugin_instances.is_empty());
    fixture.app_mut().install_project(replacement, None, false);
    assert_ne!(fixture.app().project_session, old_session);
    assert!(fixture.app().retained_clip_media.is_empty());
    assert_eq!(fixture.app().retained_clip_pcm_bytes, 0);
    assert!(!fixture.app().cached_audio_assets.contains_key(&id));
    assert!(
        !fixture
            .app_mut()
            .try_restore_retained_clip_asset(id)
            .unwrap()
    );
    assert_eq!(
        fixture.app().audio.as_ref().unwrap().project_session(),
        fixture.app().project_session
    );
}
