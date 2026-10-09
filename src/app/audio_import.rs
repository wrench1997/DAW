//! Shared File-menu / Sounds-browser import preparation, committed by the app as one undo step.

use super::*;

pub(super) fn decode_audio_import(
    project_session: u64,
    path: PathBuf,
    start: f32,
    track: usize,
) -> AudioImportResult {
    validate_import_path(&path).map_err(|error| (project_session, error))?;
    wav::read_wav(&path)
        .map(|asset| (project_session, path, asset, start, track))
        .map_err(|error| (project_session, format!("{error:#}")))
}

fn validate_import_path(path: &Path) -> Result<(), String> {
    if path.to_str().is_none() {
        return Err("This WAV path cannot be saved in a Citrus Project because its file name or a parent folder is not valid UTF-8. Rename it or copy it to a UTF-8 path, then import again.".into());
    }
    Ok(())
}

/// Completion must not mutate a Project owned by another transaction's snapshot.
#[derive(Default)]
pub(super) struct AudioImportCommitBarriers {
    pub project_transition: bool,
    pub save_or_recording: bool,
    pub piano_transform: bool,
    pub playlist_gesture: bool,
    pub piano_gesture: bool,
    pub other_project_dialog: bool,
}

pub(super) fn poll_import_result(
    receiver: &Receiver<AudioImportResult>,
    barriers: AudioImportCommitBarriers,
) -> Result<AudioImportResult, mpsc::TryRecvError> {
    if barriers.project_transition
        || barriers.save_or_recording
        || barriers.piano_transform
        || barriers.playlist_gesture
        || barriers.piano_gesture
        || barriers.other_project_dialog
    {
        return Err(mpsc::TryRecvError::Empty);
    }
    receiver.try_recv()
}

pub(super) struct PreparedAudioImport {
    pub project: Project,
    pub asset_id: u64,
    pub clip_id: u32,
}

pub(super) fn prepare_audio_import(
    project: &Project,
    path: &Path,
    asset: &wav::WavAsset,
    start: f32,
    track: usize,
    tempo: f32,
    snap: f32,
) -> Result<PreparedAudioImport, String> {
    validate_import_path(path)?;
    if !start.is_finite()
        || start < 0.0
        || track >= PLAYLIST_TRACK_COUNT
        || !tempo.is_finite()
        || tempo <= 0.0
        || !snap.is_finite()
    {
        return Err("The import placement is no longer valid".into());
    }
    let metadata = &asset.metadata;
    let length = (metadata.duration_seconds as f32 * tempo / 60.0).max(snap.max(0.0625));
    if !length.is_finite() || !(start + length).is_finite() {
        return Err("The WAV duration exceeds the project timeline range".into());
    }
    let asset_id = project
        .audio_assets
        .iter()
        .map(|asset| asset.id)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or("No audio asset IDs remain")?;
    let clip_id = project
        .clips
        .iter()
        .map(|clip| clip.id)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or("No Playlist clip IDs remain")?;
    let name = path
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Imported audio.wav".into());
    let mut project = project.clone();
    project.audio_assets.push(AudioAsset {
        id: asset_id,
        name: name.clone(),
        path: path.to_path_buf(),
        sample_rate: metadata.sample_rate,
        channels: metadata.channels,
        bits_per_sample: metadata.bits_per_sample,
        frames: metadata.frames,
        waveform_peaks: build_waveform_peaks(&asset.samples, metadata.channels, 256),
    });
    project.clips.push(Clip {
        id: clip_id,
        track,
        start,
        length,
        name,
        color: [255, 207, 99],
        kind: ClipKind::Audio,
        group_id: None,
        pattern_id: project.active_pattern().id,
        automation_id: None,
        audio_asset_id: Some(asset_id),
        source_offset: 0.0,
        audio_source_offset_frame: Some(0),
        audio_source_reference: None,
        audio_length_reference: None,
        fade_in_reference: None,
        fade_out_reference: None,
        gain: 1.0,
        fade_in: 0.0,
        fade_out: 0.0,
        muted: false,
    });
    if let Some(mixer_track_id) =
        project.mixer_track_id_at_runtime_slot(track.saturating_add(1).min(31))
    {
        project
            .audio_clip_mixer_destinations
            .push(AudioClipMixerDestination {
                clip_id,
                mixer_track_id,
            });
    }
    project.song_length_beats = project.song_length_beats.max(start + length);
    Ok(PreparedAudioImport {
        project,
        asset_id,
        clip_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decoded_fixture() -> wav::WavAsset {
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&40_u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&8_000_u32.to_le_bytes());
        bytes.extend_from_slice(&16_000_u32.to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&4_u32.to_le_bytes());
        bytes.extend_from_slice(&[0, 0, 0, 64]);
        wav::decode_wav(&bytes).unwrap()
    }

    #[test]
    fn shared_import_preserves_source_identity_placement_and_existing_routing() {
        let project = Project::default();
        let before = project_fingerprint(&project);
        let asset = decoded_fixture();
        let path = Path::new("/local/samples/one.wav");
        let prepared = prepare_audio_import(&project, path, &asset, 20.0, 4, 120.0, 0.25).unwrap();
        assert_eq!(project_fingerprint(&project), before);
        let imported = prepared.project.audio_assets.last().unwrap();
        assert_eq!(imported.path, path);
        assert_eq!(imported.frames, 2);
        assert_eq!(imported.sample_rate, 8_000);
        assert_eq!(imported.waveform_peaks, vec![0.0, 0.5]);
        let clip = prepared.project.clips.last().unwrap();
        assert_eq!(clip.id, prepared.clip_id);
        assert_eq!(clip.audio_asset_id, Some(prepared.asset_id));
        assert_eq!((clip.start, clip.track, clip.length), (20.0, 4, 0.25));
        assert_eq!(clip.audio_source_offset_frame, Some(0));
        assert_eq!(clip.source_offset, 0.0);
        let route = prepared
            .project
            .audio_clip_mixer_destinations
            .last()
            .unwrap();
        assert_eq!(route.clip_id, clip.id);
        assert_eq!(
            Some(route.mixer_track_id),
            project.mixer_track_id_at_runtime_slot(5)
        );
        assert!(prepared.project.song_length_beats >= 20.25);
    }

    #[test]
    fn import_has_one_independent_undo_transaction_and_round_trips_project() {
        let mut project = Project::default();
        let before = project_fingerprint(&project);
        let mut history = project.clone();
        let mut fingerprint = before;
        let mut undo = Vec::new();
        let mut redo = vec![project.clone()];
        let mut dirty = false;
        let prepared = prepare_audio_import(
            &project,
            Path::new("source.wav"),
            &decoded_fixture(),
            2.0,
            4,
            120.0,
            0.25,
        )
        .unwrap();
        assert!(commit_explicit_project_history_transaction(
            &mut project,
            &mut history,
            &mut fingerprint,
            &mut undo,
            &mut redo,
            &mut dirty,
            prepared.project
        ));
        assert_eq!(undo.len(), 1);
        assert!(redo.is_empty());
        assert!(dirty);
        let after = project_fingerprint(&project);
        assert_ne!(before, after);
        assert_eq!(project_fingerprint(&history), after);
        let loaded: Project =
            serde_json::from_str(&serde_json::to_string(&project).unwrap()).unwrap();
        assert_eq!(project_fingerprint(&loaded), after);
        let reverted = undo.pop().unwrap();
        assert_eq!(project_fingerprint(&reverted), before);
        // Undo removes only the project reference. Preparation and history never write/delete files.
        assert_eq!(project_fingerprint(&project), after);
    }

    #[test]
    fn import_does_not_merge_with_an_unobserved_preceding_edit() {
        let mut project = Project::default();
        let mut history = project.clone();
        let mut fingerprint = project_fingerprint(&project);
        let original = fingerprint;
        project.tempo = 147.0;
        let prior_edit = project_fingerprint(&project);
        let mut undo = Vec::new();
        let mut redo = Vec::new();
        let mut dirty = false;
        let prepared = prepare_audio_import(
            &project,
            Path::new("source.wav"),
            &decoded_fixture(),
            2.0,
            4,
            147.0,
            0.25,
        )
        .unwrap();
        commit_explicit_project_history_transaction(
            &mut project,
            &mut history,
            &mut fingerprint,
            &mut undo,
            &mut redo,
            &mut dirty,
            prepared.project,
        );
        assert_eq!(undo.len(), 2);
        assert_eq!(project_fingerprint(&undo.pop().unwrap()), prior_edit);
        assert_eq!(project_fingerprint(&undo.pop().unwrap()), original);
    }

    #[test]
    fn import_id_exhaustion_and_invalid_placement_leave_source_project_untouched() {
        let mut project = Project::default();
        let asset = decoded_fixture();
        let path = Path::new("source.wav");
        for (start, track, tempo) in [
            (f32::NAN, 4, 120.0),
            (-1.0, 4, 120.0),
            (0.0, 32, 120.0),
            (0.0, 4, 0.0),
        ] {
            assert!(
                prepare_audio_import(&project, path, &asset, start, track, tempo, 0.25).is_err()
            );
        }
        project.clips[0].id = u32::MAX;
        let before = project_fingerprint(&project);
        assert!(prepare_audio_import(&project, path, &asset, 0.0, 4, 120.0, 0.25).is_err());
        assert_eq!(project_fingerprint(&project), before);
    }
    #[test]
    fn selected_file_removed_or_replaced_reports_real_decoder_error() {
        let directory =
            std::env::temp_dir().join(format!("citrus-browser-import-{}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("candidate.wav");
        std::fs::write(&path, b"not a WAV despite its extension").unwrap();
        let mut browser = crate::sample_browser::SampleBrowser::default();
        browser.navigate(directory.clone());
        let deadline = Instant::now() + Duration::from_secs(5);
        while browser.busy() {
            assert!(Instant::now() < deadline);
            browser.poll();
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(browser.entries.len(), 1);
        assert_eq!(browser.entries[0].path, path);
        let error = decode_audio_import(42, path.clone(), 1.0, 4).unwrap_err();
        assert_eq!(error.0, 42);
        assert!(error.1.contains("Invalid WAV"));
        std::fs::remove_file(&path).unwrap();
        let missing = decode_audio_import(43, path, 1.0, 4).unwrap_err();
        assert_eq!(missing.0, 43);
        assert!(missing.1.contains("Unable to inspect"));
        std::fs::remove_dir(directory).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn non_utf8_file_or_ancestor_is_rejected_by_shared_decoder_and_preparation() {
        use std::{ffi::OsStr, os::unix::ffi::OsStrExt};
        let project = Project::default();
        let before = project_fingerprint(&project);
        for bytes in [
            b"sample\xff.wav".as_slice(),
            b"/samples\xff/valid.wav".as_slice(),
        ] {
            let path = Path::new(OsStr::from_bytes(bytes));
            let error = decode_audio_import(19, path.to_owned(), 0.0, 4).unwrap_err();
            assert_eq!(error.0, 19);
            assert!(error.1.contains("UTF-8"));
            assert!(error.1.contains("Rename"));
            let prepared =
                prepare_audio_import(&project, path, &decoded_fixture(), 0.0, 4, 120.0, 0.25);
            assert!(matches!(prepared, Err(error) if error.contains("UTF-8")));
            assert_eq!(project_fingerprint(&project), before);
            assert!(serde_json::to_string(&project).is_ok());
        }
    }

    #[test]
    fn every_commit_barrier_keeps_completed_result_in_single_receiver() {
        for flag in 0..6 {
            let (sender, receiver) = mpsc::sync_channel(1);
            sender
                .send(Ok((
                    12,
                    PathBuf::from("source.wav"),
                    decoded_fixture(),
                    0.0,
                    4,
                )))
                .unwrap();
            let barriers = AudioImportCommitBarriers {
                project_transition: flag == 0,
                save_or_recording: flag == 1,
                piano_transform: flag == 2,
                playlist_gesture: flag == 3,
                piano_gesture: flag == 4,
                other_project_dialog: flag == 5,
            };
            assert!(matches!(
                poll_import_result(&receiver, barriers),
                Err(mpsc::TryRecvError::Empty)
            ));
            assert!(
                poll_import_result(&receiver, AudioImportCommitBarriers::default())
                    .unwrap()
                    .is_ok()
            );
            assert!(matches!(
                receiver.try_recv(),
                Err(mpsc::TryRecvError::Empty)
            ));
        }
    }

    #[test]
    fn deferred_import_survives_transform_cancel_or_gesture_commit_with_separate_undo() {
        for accept_edit in [false, true] {
            let mut project = Project::default();
            let before_edit = project.clone();
            let original = project_fingerprint(&project);
            let mut history = project.clone();
            let mut fingerprint = original;
            let mut undo = Vec::new();
            let mut redo = Vec::new();
            let mut dirty = false;
            // A transform or gesture owns the pre-edit Project while the WAV worker finishes.
            project.tempo = 147.0;
            let preview = project.clone();
            let (sender, receiver) = mpsc::sync_channel(1);
            sender
                .send(Ok((
                    12,
                    PathBuf::from("source.wav"),
                    decoded_fixture(),
                    0.0,
                    4,
                )))
                .unwrap();
            assert!(matches!(
                poll_import_result(
                    &receiver,
                    AudioImportCommitBarriers {
                        piano_transform: !accept_edit,
                        playlist_gesture: accept_edit,
                        ..Default::default()
                    }
                ),
                Err(mpsc::TryRecvError::Empty)
            ));
            assert_eq!(project.audio_assets.len(), before_edit.audio_assets.len());
            project = before_edit;
            if accept_edit {
                commit_explicit_project_history_transaction(
                    &mut project,
                    &mut history,
                    &mut fingerprint,
                    &mut undo,
                    &mut redo,
                    &mut dirty,
                    preview,
                );
            }
            let after_edit = project_fingerprint(&project);
            let (_, path, asset, start, track) =
                poll_import_result(&receiver, AudioImportCommitBarriers::default())
                    .unwrap()
                    .unwrap();
            let candidate =
                prepare_audio_import(&project, &path, &asset, start, track, project.tempo, 0.25)
                    .unwrap();
            assert!(commit_explicit_project_history_transaction(
                &mut project,
                &mut history,
                &mut fingerprint,
                &mut undo,
                &mut redo,
                &mut dirty,
                candidate.project
            ));
            assert_eq!(undo.len(), if accept_edit { 2 } else { 1 });
            assert_eq!(project_fingerprint(&undo.pop().unwrap()), after_edit);
            if accept_edit {
                assert_eq!(project_fingerprint(&undo.pop().unwrap()), original);
            }
        }
    }

    #[test]
    fn deferred_result_is_still_checked_against_latest_project_generation() {
        let (sender, receiver) = mpsc::sync_channel(1);
        sender
            .send(Ok((
                12,
                PathBuf::from("source.wav"),
                decoded_fixture(),
                0.0,
                4,
            )))
            .unwrap();
        assert!(matches!(
            poll_import_result(
                &receiver,
                AudioImportCommitBarriers {
                    project_transition: true,
                    ..Default::default()
                }
            ),
            Err(mpsc::TryRecvError::Empty)
        ));
        let (generation, ..) = poll_import_result(&receiver, AudioImportCommitBarriers::default())
            .unwrap()
            .unwrap();
        assert!(!audio_asset_load_is_current(generation, 13));
    }
}
