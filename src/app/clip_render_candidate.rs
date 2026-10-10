//! Pure selected-occurrence reinsertion. A receipt is data, not filesystem attestation.
//!
//! No file is opened, registered, moved, or deleted here. In particular, every
//! failure returns the published path so its future owner can retain the media.

use super::*;

/// Only this module can construct or alter the admitted Project replacement.
pub(super) struct PreparedOccurrenceCandidate {
    expected_snapshot: Arc<[u8]>,
    expected_context: OccurrenceContext,
    project: Project,
    source_clip_id: u32,
    audio_clip_id: u32,
    asset_id: u64,
    published_path: PathBuf,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct OccurrenceHistoryCommit {
    pub source_clip_id: u32,
    pub audio_clip_id: u32,
    pub asset_id: u64,
    pub published_path: PathBuf,
}

fn retain_published_path(mut error: OccurrenceError, path: &Path) -> OccurrenceError {
    error.published_path = Some(path.to_path_buf());
    error
}

pub(super) fn prepare_rendered_occurrence_candidate(
    project: &Project,
    context: OccurrenceContext,
    plan: &OccurrencePlan,
    media: &RenderedOccurrenceMedia,
) -> Result<PreparedOccurrenceCandidate, OccurrenceError> {
    prepare_candidate(project, context, plan, media)
        .map_err(|error| retain_published_path(error, &media.path))
}

fn prepare_candidate(
    project: &Project,
    context: OccurrenceContext,
    plan: &OccurrencePlan,
    media: &RenderedOccurrenceMedia,
) -> Result<PreparedOccurrenceCandidate, OccurrenceError> {
    // Admission precedes the sole Project clone. Never turn a stale result into
    // an edit of the current Project, even when only an unrelated field changed.
    plan.check_current(project, context)?;
    validate_rendered_media(plan, media)?;
    if !plan.channel_gain.is_finite() || !(0.0..=1.5).contains(&plan.channel_gain) {
        return Err(failure(
            OccurrenceErrorKind::Unsupported,
            "The captured Channel volume cannot be represented as an Audio Clip gain",
        ));
    }
    let audio_clip_id = project
        .clips
        .iter()
        .map(|clip| clip.id)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| failure(OccurrenceErrorKind::Capacity, "No Playlist clip IDs remain"))?;
    let asset_id = project
        .audio_assets
        .iter()
        .map(|asset| asset.id)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| failure(OccurrenceErrorKind::Capacity, "No audio asset IDs remain"))?;
    let source_index = project
        .clips
        .iter()
        .position(|clip| clip.id == plan.clip_id)
        .ok_or_else(|| failure(OccurrenceErrorKind::Stale, "The source Clip is missing"))?;
    let stop_frame = plan
        .end_frame
        .checked_add(u64::from(plan.tail_frames))
        .ok_or_else(|| {
            failure(
                OccurrenceErrorKind::Range,
                "The requested stop frame overflows",
            )
        })?;
    let end_beat = plan.tempo_map.frame_to_beat(stop_frame).map_err(|error| {
        failure(
            OccurrenceErrorKind::Range,
            format!("The requested audio endpoint is outside the song: {error}"),
        )
    })?;
    let exact_length = end_beat - f64::from(plan.start_beat);
    let displayed_length = if plan.tail_frames == 0 {
        plan.length_beats
    } else {
        exact_length as f32
    };
    let length_reference = AudioLengthReference {
        stored_length_beats: displayed_length,
        length_beats: exact_length,
        export_length_beats: exact_length,
    };
    if !length_reference.is_valid()
        || !(plan.start_beat + displayed_length).is_finite()
        || plan.tempo_map.beat_to_frame(end_beat).ok() != Some(stop_frame)
    {
        return Err(failure(
            OccurrenceErrorKind::Range,
            "The exact rendered audio length cannot be represented",
        ));
    }
    let source_timeline = compile_candidate_timeline(project, &plan.tempo_map)?;
    let name = media
        .path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("Rendered occurrence.wav")
        .to_owned();
    let metadata = &media.asset.metadata;
    let asset = AudioAsset {
        id: asset_id,
        name: name.clone(),
        path: media.path.clone(),
        sample_rate: metadata.sample_rate,
        channels: metadata.channels,
        bits_per_sample: metadata.bits_per_sample,
        frames: metadata.frames,
        waveform_peaks: build_waveform_peaks(&media.asset.samples, metadata.channels, 256),
    };
    let clip = Clip {
        id: audio_clip_id,
        track: plan.row,
        start: plan.start_beat,
        length: displayed_length,
        name,
        color: project.clips[source_index].color,
        kind: ClipKind::Audio,
        group_id: None,
        pattern_id: plan.pattern_id,
        automation_id: None,
        audio_asset_id: Some(asset_id),
        source_offset: 0.0,
        audio_source_offset_frame: Some(0),
        audio_source_reference: None,
        audio_length_reference: Some(length_reference),
        fade_in_reference: None,
        fade_out_reference: None,
        // The source Clip gain is already represented in rendered MIDI velocity.
        // Only the captured Channel volume is transferred to this dry Audio Clip.
        gain: plan.channel_gain,
        fade_in: 0.0,
        fade_out: 0.0,
        muted: false,
    };
    let mut candidate = project.clone();
    candidate.clips[source_index].muted = true;
    candidate.audio_assets.push(asset);
    candidate.clips.push(clip);
    candidate
        .audio_clip_mixer_destinations
        .push(AudioClipMixerDestination {
            clip_id: audio_clip_id,
            mixer_track_id: plan.mixer_track_id,
        });

    let timeline = compile_candidate_timeline(&candidate, &plan.tempo_map)?;
    validate_candidate_geometry(&candidate, &timeline, plan, audio_clip_id, asset_id)?;
    validate_remaining_events(&source_timeline, &timeline, plan, audio_clip_id)?;

    // A save/reopen must preserve the compiled proof without Project::normalize
    // silently repairing geometry, routing, identities, or source metadata.
    let bytes = project_snapshot(&candidate)?;
    let round_tripped: Project = serde_json::from_slice(&bytes).map_err(|error| {
        failure(
            OccurrenceErrorKind::Source,
            format!("The candidate Project cannot round-trip: {error}"),
        )
    })?;
    let restored_map =
        TempoMap::from_project(&round_tripped, plan.sample_rate).map_err(|error| {
            failure(
                OccurrenceErrorKind::Timeline,
                format!("The saved candidate tempo map is invalid: {error}"),
            )
        })?;
    let restored_timeline = compile_candidate_timeline(&round_tripped, &restored_map)?;
    if restored_timeline != timeline {
        return Err(failure(
            OccurrenceErrorKind::Timeline,
            "Project serialization changes the candidate timeline",
        ));
    }
    validate_candidate_geometry(
        &round_tripped,
        &restored_timeline,
        plan,
        audio_clip_id,
        asset_id,
    )?;
    Ok(PreparedOccurrenceCandidate {
        expected_snapshot: Arc::clone(&plan.snapshot),
        expected_context: context,
        project: candidate,
        source_clip_id: plan.clip_id,
        audio_clip_id,
        asset_id,
        published_path: media.path.clone(),
    })
}

fn compile_candidate_timeline(
    project: &Project,
    tempo_map: &TempoMap,
) -> Result<CompiledTimeline, OccurrenceError> {
    project.validate_mixer_graph().map_err(|error| {
        failure(
            OccurrenceErrorKind::Timeline,
            format!("Candidate mixer graph validation failed: {error:#}"),
        )
    })?;
    let timeline =
        CompiledTimeline::from_project(project, tempo_map, TimelineCompileOptions::default())
            .map_err(|error| failure(OccurrenceErrorKind::Timeline, error.to_string()))?;
    let stats = timeline.stats();
    if !timeline.diagnostics().is_empty()
        || stats.diagnostics_suppressed != 0
        || stats.invalid_items_skipped != 0
        || stats.inaudible_notes_skipped != 0
    {
        return Err(failure(
            OccurrenceErrorKind::Timeline,
            "Candidate timeline has diagnostics or skipped invalid/inaudible content",
        ));
    }
    Ok(timeline)
}

fn validate_candidate_geometry(
    project: &Project,
    timeline: &CompiledTimeline,
    plan: &OccurrencePlan,
    audio_clip_id: u32,
    asset_id: u64,
) -> Result<(), OccurrenceError> {
    let stop_frame = plan
        .end_frame
        .checked_add(u64::from(plan.tail_frames))
        .ok_or_else(|| {
            failure(
                OccurrenceErrorKind::Range,
                "The requested stop frame overflows",
            )
        })?;
    let descriptors = timeline
        .audio_clips()
        .iter()
        .filter(|descriptor| descriptor.clip_id == audio_clip_id)
        .collect::<Vec<_>>();
    let Some(descriptor) = descriptors.first().filter(|_| descriptors.len() == 1) else {
        return Err(failure(
            OccurrenceErrorKind::Timeline,
            "The candidate must compile exactly one rendered Audio Clip",
        ));
    };
    if descriptor.asset_id != asset_id
        || descriptor.start_frame != plan.start_frame
        || descriptor.clip_end_frame != stop_frame
        || descriptor.stop_frame != stop_frame
        || descriptor.source_offset_frame != 0
        || descriptor.source_elapsed_frames != 0
        || descriptor.timeline_sample_rate != plan.sample_rate
        || descriptor.source_sample_rate != plan.sample_rate
        || descriptor.gain.to_bits() != plan.channel_gain.to_bits()
        || descriptor.fades.fade_in_frames != 0
        || descriptor.fades.fade_out_frames != 0
        || project.mixer_runtime_slot(plan.mixer_track_id)
            != Some(usize::from(descriptor.mixer_track))
        || project.audio_clip_mixer_track_id(audio_clip_id) != Some(plan.mixer_track_id)
    {
        return Err(failure(
            OccurrenceErrorKind::Timeline,
            "The compiled Audio Clip differs from the exact rendered geometry, gain, or route",
        ));
    }
    let clip = project
        .clips
        .iter()
        .find(|clip| clip.id == audio_clip_id)
        .ok_or_else(|| {
            failure(
                OccurrenceErrorKind::Timeline,
                "The rendered Audio Clip is missing",
            )
        })?;
    // Ordinary export deliberately uses a separate legacy-precision switch.
    // Pin and verify both paths rather than assuming their beat math is equal.
    for export in [false, true] {
        let endpoint = crate::audio_clip::audio_end_beat(clip, export)
            .and_then(|beat| plan.tempo_map.beat_to_frame(beat).ok());
        if endpoint != Some(stop_frame) {
            return Err(failure(
                OccurrenceErrorKind::Timeline,
                "The rendered Audio Clip endpoint differs between playback and export",
            ));
        }
    }
    Ok(())
}

fn selected_note(event: &TimelineEvent, plan: &OccurrencePlan) -> bool {
    let source = match event.kind {
        TimelineEventKind::NoteOn { source, .. } | TimelineEventKind::NoteOff { source, .. } => {
            source
        }
        _ => return false,
    };
    match source {
        NoteSourceDescriptor::ChannelStep {
            clip_id,
            pattern_id,
            ..
        }
        | NoteSourceDescriptor::LegacyPiano {
            clip_id,
            pattern_id,
            ..
        } => clip_id == plan.clip_id && pattern_id == plan.pattern_id,
    }
}

fn without_transient_note_id(mut event: TimelineEvent) -> TimelineEvent {
    // Compiling after removal renumbers sequential runtime note IDs. Source
    // descriptors remain complete; only this transient sequence is ignored.
    // The original admitted local schedule still retains its original IDs.
    match &mut event.kind {
        TimelineEventKind::NoteOn { note_id, .. } | TimelineEventKind::NoteOff { note_id, .. } => {
            *note_id = 0;
        }
        _ => {}
    }
    event
}

fn validate_remaining_events(
    before: &CompiledTimeline,
    after: &CompiledTimeline,
    plan: &OccurrencePlan,
    audio_clip_id: u32,
) -> Result<(), OccurrenceError> {
    if after
        .events()
        .iter()
        .any(|event| selected_note(event, plan))
    {
        return Err(failure(
            OccurrenceErrorKind::Timeline,
            "The muted source occurrence still compiles MIDI notes",
        ));
    }
    let expected = before
        .events()
        .iter()
        .filter(|event| !selected_note(event, plan))
        .copied()
        .map(without_transient_note_id);
    let remaining = after
        .events()
        .iter()
        .filter(|event| match event.kind {
            TimelineEventKind::AudioStart(descriptor) => descriptor.clip_id != audio_clip_id,
            TimelineEventKind::AudioStop { clip_id, .. } => clip_id != audio_clip_id,
            _ => true,
        })
        .copied()
        .map(without_transient_note_id);
    if !expected.eq(remaining) {
        return Err(failure(
            OccurrenceErrorKind::Timeline,
            "Rendering the selected occurrence changes other compiled content",
        ));
    }
    Ok(())
}

/// The App adapter must supply real session/revision tokens and its existing
/// gesture/save barriers. This pure wrapper does not claim to own that state.
#[allow(clippy::too_many_arguments)]
pub(super) fn commit_rendered_occurrence_history_transaction(
    context: OccurrenceContext,
    project: &mut Project,
    history_snapshot: &mut Project,
    history_fingerprint: &mut u64,
    undo_stack: &mut Vec<Project>,
    redo_stack: &mut Vec<Project>,
    dirty: &mut bool,
    candidate: PreparedOccurrenceCandidate,
) -> Result<OccurrenceHistoryCommit, OccurrenceError> {
    let path = &candidate.published_path;
    if context != candidate.expected_context {
        return Err(retain_published_path(
            failure(
                OccurrenceErrorKind::Stale,
                "The project session or source/control revision changed",
            ),
            path,
        ));
    }
    let snapshot = project_snapshot(project).map_err(|error| retain_published_path(error, path))?;
    if snapshot != candidate.expected_snapshot {
        return Err(retain_published_path(
            failure(
                OccurrenceErrorKind::Stale,
                "The Project changed after occurrence preparation",
            ),
            path,
        ));
    }
    // The helper rejects a fingerprint no-op before touching Project/history.
    // A collision therefore fails closed too; exact bytes above are the guard.
    if !commit_explicit_project_history_transaction(
        project,
        history_snapshot,
        history_fingerprint,
        undo_stack,
        redo_stack,
        dirty,
        candidate.project,
    ) {
        return Err(retain_published_path(
            failure(
                OccurrenceErrorKind::History,
                "The rendered occurrence transaction is a no-op",
            ),
            path,
        ));
    }
    Ok(OccurrenceHistoryCommit {
        source_clip_id: candidate.source_clip_id,
        audio_clip_id: candidate.audio_clip_id,
        asset_id: candidate.asset_id,
        published_path: candidate.published_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 8_000;

    fn context() -> OccurrenceContext {
        OccurrenceContext {
            project_session: 19,
            source_revision: 23,
            control_revision: 29,
        }
    }

    fn published_path() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"C:\durable\rendered-occurrence-101.wav")
        } else {
            PathBuf::from("/durable/rendered-occurrence-101.wav")
        }
    }

    fn fixture() -> Project {
        let mut project = Project::blank();
        project.tempo = 120.0;
        project.swing = 0.0;
        project.song_length_beats = 8.0;
        project.channels[0].volume = 0.625;
        project.channels[0].mixer_track = project.mixer_track_id_at_runtime_slot(7).unwrap();
        project.channels[0].instrument_plugin_instance_id = Some(71);
        project.channels[0].steps[0] = true;
        project.patterns[0].channel_steps[0][0] = true;
        project.patterns[0].notes.push(PianoNote {
            id: 43,
            channel_id: Some(project.channels[0].id),
            group_id: None,
            note: 64,
            start: 0.0,
            length: 0.5,
            velocity: 0.8,
            selected: false,
            muted: false,
        });
        project.plugin_instances.push(PluginInstance {
            midi_ports: Default::default(),
            id: 71,
            format: PluginFormat::Vst3,
            role: PluginRole::Instrument,
            path: PathBuf::from("/fixture/not-loaded.vst3"),
            uid: "fixture-instrument".into(),
            vendor: "plugin-free fixture".into(),
            name: "Unloaded fixture".into(),
            enabled: true,
            bypass: false,
            wet: 1.0,
            parameters: [(17, 0.375)].into_iter().collect(),
            opaque_state: vec![0, 127, 255, 13],
            runtime_status: PluginRuntimeStatus::Unloaded,
        });
        project.clips.push(Clip {
            id: 11,
            track: 19,
            start: 0.03175,
            length: 0.0625,
            name: "Only this occurrence".into(),
            color: [71, 89, 103],
            kind: ClipKind::Pattern,
            group_id: Some(13),
            pattern_id: project.patterns[0].id,
            automation_id: None,
            audio_asset_id: None,
            source_offset: 0.0,
            audio_source_offset_frame: None,
            audio_source_reference: None,
            audio_length_reference: None,
            fade_in_reference: None,
            fade_out_reference: None,
            gain: 0.4,
            fade_in: 0.0,
            fade_out: 0.0,
            muted: false,
        });
        let mut other = project.clips[0].clone();
        other.id = 12;
        other.start = 2.0;
        other.name = "Same Pattern, untouched".into();
        project.clips.push(other);
        // Display order deliberately disagrees with the stable bus identity.
        project.mixer_tracks.swap(1, 7);
        project
    }

    fn media(plan: &OccurrencePlan) -> RenderedOccurrenceMedia {
        let frames = plan.end_frame - plan.start_frame + u64::from(plan.tail_frames);
        RenderedOccurrenceMedia {
            snapshot: Arc::clone(&plan.snapshot),
            context: plan.context,
            clip_id: plan.clip_id,
            pattern_id: plan.pattern_id,
            channel_id: plan.channel_id,
            instance_id: plan.instance_id,
            publication_id: 101,
            path: published_path(),
            asset: wav::WavAsset {
                metadata: wav::WavMetadata {
                    sample_rate: plan.sample_rate,
                    channels: 2,
                    encoding: wav::WavSampleEncoding::IeeeFloat,
                    bits_per_sample: 32,
                    valid_bits_per_sample: 32,
                    channel_mask: None,
                    block_align: 8,
                    byte_rate: plan.sample_rate * 8,
                    frames,
                    duration_seconds: frames as f64 / f64::from(plan.sample_rate),
                    data_bytes: frames * 8,
                },
                samples: vec![0.25; usize::try_from(frames * 2).unwrap()],
            },
            prewrite_peak: 0.25,
        }
    }

    fn prepare(project: &Project, tail: u32) -> PreparedOccurrenceCandidate {
        let plan =
            OccurrencePlan::prepare(project, context(), project.clips[0].id, RATE, tail).unwrap();
        prepare_rendered_occurrence_candidate(project, context(), &plan, &media(&plan)).unwrap()
    }

    fn bytes(project: &Project) -> Vec<u8> {
        serde_json::to_vec(project).unwrap()
    }

    fn history_bytes(
        project: &Project,
        history: &Project,
        fingerprint: u64,
        undo: &[Project],
        redo: &[Project],
        dirty: bool,
    ) -> Vec<u8> {
        serde_json::to_vec(&(project, history, fingerprint, undo, redo, dirty)).unwrap()
    }

    #[test]
    fn candidate_preserves_exact_source_and_other_content_with_gain_and_stable_route() {
        let project = fixture();
        let before = bytes(&project);
        let plan = OccurrencePlan::prepare(&project, context(), 11, RATE, 0).unwrap();
        assert!(plan.local_events.iter().any(|event| matches!(event.kind,
            TimelineEventKind::NoteOn { gain, .. } if gain == 0.4)));
        let rendered = media(&plan);
        let prepared =
            prepare_rendered_occurrence_candidate(&project, context(), &plan, &rendered).unwrap();
        assert_eq!(bytes(&project), before);
        let audio = prepared.project.clips.last().unwrap();
        assert_eq!(
            (audio.id, audio.audio_asset_id),
            (prepared.audio_clip_id, Some(prepared.asset_id))
        );
        assert_eq!(
            (audio.start, audio.track, audio.length),
            (project.clips[0].start, 19, project.clips[0].length)
        );
        assert_eq!(audio.gain.to_bits(), 0.625_f32.to_bits());
        assert_eq!(audio.group_id, None);
        assert_eq!(audio.source_offset, 0.0);
        assert_eq!(audio.audio_source_offset_frame, Some(0));
        assert_eq!(audio.audio_source_reference, None);
        assert_eq!((audio.fade_in, audio.fade_out), (0.0, 0.0));
        assert_eq!(
            (audio.fade_in_reference, audio.fade_out_reference),
            (None, None)
        );
        assert_eq!(
            prepared.project.audio_clip_mixer_track_id(audio.id),
            Some(project.channels[0].mixer_track)
        );
        let timeline = compile_candidate_timeline(&prepared.project, &plan.tempo_map).unwrap();
        let descriptor = timeline.audio_clips().last().unwrap();
        assert_eq!(
            (
                descriptor.start_frame,
                descriptor.clip_end_frame,
                descriptor.stop_frame
            ),
            (plan.start_frame, plan.end_frame, plan.end_frame)
        );
        assert_eq!(descriptor.mixer_track, 7);
        assert_eq!(descriptor.gain, project.channels[0].volume);
        assert_eq!(descriptor.source_position_at(plan.start_frame, RATE), 0.0);
        assert!(
            !timeline
                .events()
                .iter()
                .any(|event| selected_note(event, &plan))
        );
        assert!(timeline.events().iter().any(|event| matches!(
            event.kind,
            TimelineEventKind::NoteOn {
                source: NoteSourceDescriptor::LegacyPiano { clip_id: 12, .. },
                ..
            }
        )));
        let installed = prepared.project.audio_assets.last().unwrap();
        assert_eq!(installed.path, rendered.path);
        assert_eq!(
            (
                installed.channels,
                installed.bits_per_sample,
                installed.frames
            ),
            (2, 32, plan.end_frame - plan.start_frame)
        );
        assert!(installed.waveform_peaks.iter().all(|peak| *peak == 0.25));

        // Remove exactly the permitted delta; all serialized source/plugin and
        // other occurrence data must be byte-for-byte identical to the input.
        let mut restored = prepared.project;
        restored.clips.pop();
        restored.audio_assets.pop();
        restored.audio_clip_mixer_destinations.pop();
        restored.clips[0].muted = false;
        assert_eq!(bytes(&restored), before);
    }

    #[test]
    fn candidate_preserves_frame_geometry_for_quantum_edges_and_fixed_tails() {
        for start_frame in [0, 1, 127, 128, 129] {
            for tail in [0, 1, 127, 129] {
                let mut project = fixture();
                project.clips[0].start = start_frame as f32 / 4_000.0;
                let plan = OccurrencePlan::prepare(&project, context(), 11, RATE, tail).unwrap();
                assert_eq!(plan.start_frame, start_frame);
                let rendered = media(&plan);
                let candidate =
                    prepare_rendered_occurrence_candidate(&project, context(), &plan, &rendered)
                        .unwrap();
                let audio = candidate.project.clips.last().unwrap();
                let reference = audio.audio_length_reference.unwrap();
                let stop = plan.end_frame + u64::from(tail);
                assert_eq!(
                    reference.length_beats,
                    plan.tempo_map.frame_to_beat(stop).unwrap() - f64::from(audio.start)
                );
                assert_eq!(reference.export_length_beats, reference.length_beats);
                if tail == 0 {
                    assert_eq!(audio.length.to_bits(), project.clips[0].length.to_bits());
                } else {
                    assert_eq!(audio.length, reference.length_beats as f32);
                }
                for export in [false, true] {
                    assert_eq!(
                        plan.tempo_map
                            .beat_to_frame(
                                crate::audio_clip::audio_end_beat(audio, export).unwrap()
                            )
                            .unwrap(),
                        stop
                    );
                }
                let timeline =
                    compile_candidate_timeline(&candidate.project, &plan.tempo_map).unwrap();
                let descriptor = timeline.audio_clips().last().unwrap();
                assert_eq!(
                    (
                        descriptor.start_frame,
                        descriptor.clip_end_frame,
                        descriptor.stop_frame
                    ),
                    (start_frame, stop, stop)
                );
                assert_eq!(
                    candidate.project.song_length_beats,
                    project.song_length_beats
                );
            }
        }
    }

    #[test]
    fn candidate_keeps_slipped_source_group_and_plugin_bytes_unchanged() {
        let mut project = fixture();
        project.clips[0].source_offset = 0.25;
        project.patterns[0].channel_steps[0][0] = false;
        project.patterns[0].channel_steps[0][1] = true;
        project.patterns[0].notes[0].start = 0.25;
        project.clips[1].length = 0.5;
        let before = bytes(&project);
        let candidate = prepare(&project, 17);
        assert_eq!(candidate.project.clips[0].source_offset, 0.25);
        assert_eq!(candidate.project.clips[0].group_id, Some(13));
        let audio = candidate.project.clips.last().unwrap();
        assert_eq!(audio.group_id, None);
        assert_eq!(audio.audio_source_offset_frame, Some(0));
        let mut reverted = candidate.project;
        reverted.clips[0].muted = false;
        reverted.clips.pop();
        reverted.audio_assets.pop();
        reverted.audio_clip_mixer_destinations.pop();
        assert_eq!(bytes(&reverted), before);
    }

    #[test]
    fn candidate_accepts_exact_song_end_and_transfers_supported_gain_without_scaling_pcm() {
        for volume in [0.0, 0.25, 1.0, 1.5] {
            let mut project = fixture();
            project.clips[0].start = 7.9375;
            project.channels[0].volume = volume;
            let plan = OccurrencePlan::prepare(&project, context(), 11, RATE, 0).unwrap();
            assert_eq!(plan.end_frame, plan.tempo_map.duration_frames());
            let rendered = media(&plan);
            let pcm_before = rendered.asset.samples.clone();
            let candidate =
                prepare_rendered_occurrence_candidate(&project, context(), &plan, &rendered)
                    .unwrap();
            assert_eq!(
                candidate.project.clips.last().unwrap().gain.to_bits(),
                volume.to_bits()
            );
            assert_eq!(rendered.asset.samples, pcm_before);
            assert_eq!(candidate.project.song_length_beats, 8.0);
        }
    }

    #[test]
    fn exhausted_ids_fail_without_changing_source_or_losing_published_path() {
        let control = fixture();
        let _ = prepare(&control, 0);
        for asset_exhausted in [false, true] {
            let mut project = fixture();
            if asset_exhausted {
                project.audio_assets.push(AudioAsset {
                    id: u64::MAX,
                    name: "Existing unused media".into(),
                    path: PathBuf::from("/durable/existing.wav"),
                    sample_rate: RATE,
                    channels: 2,
                    bits_per_sample: 32,
                    frames: 1,
                    waveform_peaks: vec![0.0],
                });
            } else {
                project.clips[1].id = u32::MAX;
            }
            let plan = OccurrencePlan::prepare(&project, context(), 11, RATE, 0).unwrap();
            let rendered = media(&plan);
            let before = bytes(&project);
            let error =
                prepare_rendered_occurrence_candidate(&project, context(), &plan, &rendered)
                    .err()
                    .unwrap();
            assert_eq!(error.kind, OccurrenceErrorKind::Capacity);
            assert_eq!(
                error.reason,
                if asset_exhausted {
                    "No audio asset IDs remain"
                } else {
                    "No Playlist clip IDs remain"
                }
            );
            assert_eq!(error.published_path.as_ref(), Some(&rendered.path));
            assert_eq!(bytes(&project), before);
        }
    }

    #[test]
    fn candidate_failures_retain_published_path_and_never_mutate_source() {
        let project = fixture();
        let plan = OccurrencePlan::prepare(&project, context(), 11, RATE, 0).unwrap();
        let good_media = media(&plan);
        let _ =
            prepare_rendered_occurrence_candidate(&project, context(), &plan, &good_media).unwrap();
        let before = bytes(&project);
        for variant in 0..3 {
            let mut rendered = media(&plan);
            let mut current_context = context();
            match variant {
                0 => current_context.project_session += 1,
                1 => rendered.publication_id = 0,
                _ => rendered.prewrite_peak = 1.25,
            }
            let error =
                prepare_rendered_occurrence_candidate(&project, current_context, &plan, &rendered)
                    .err()
                    .unwrap();
            assert_eq!(
                error.kind,
                if variant == 0 {
                    OccurrenceErrorKind::Stale
                } else {
                    OccurrenceErrorKind::Media
                }
            );
            assert_eq!(
                error.reason,
                match variant {
                    0 => "The source session or source/control revision changed",
                    1 => "A uniquely published absolute UTF-8 media path is required",
                    _ =>
                        "Dry render headroom exceeds the current lossless WAV import subset; retain the published render",
                }
            );
            assert_eq!(error.published_path.as_ref(), Some(&rendered.path));
            assert_eq!(bytes(&project), before);
        }
    }

    #[test]
    fn history_conversion_is_one_step_and_retains_media_path_through_undo_redo() {
        let mut project = fixture();
        let before = bytes(&project);
        let candidate = prepare(&project, 129);
        let expected_after = bytes(&candidate.project);
        let expected_path = candidate.published_path.clone();
        let mut history = project.clone();
        let mut fingerprint = project_fingerprint(&project);
        let mut undo = Vec::new();
        let mut redo = vec![project.clone()];
        let mut dirty = false;
        let commit = commit_rendered_occurrence_history_transaction(
            context(),
            &mut project,
            &mut history,
            &mut fingerprint,
            &mut undo,
            &mut redo,
            &mut dirty,
            candidate,
        )
        .unwrap();
        assert_eq!(commit.source_clip_id, 11);
        assert_eq!(commit.published_path, expected_path);
        assert_eq!(undo.len(), 1);
        assert!(redo.is_empty());
        assert!(dirty);
        assert_eq!(bytes(&project), expected_after);
        assert_eq!(bytes(&history), expected_after);
        assert_eq!(fingerprint, project_fingerprint(&project));
        let restored: Project = serde_json::from_slice(&expected_after).unwrap();
        assert_eq!(bytes(&restored), expected_after);
        let previous = undo.pop().unwrap();
        redo.push(std::mem::replace(&mut project, previous));
        assert_eq!(bytes(&project), before);
        assert_eq!(redo[0].audio_assets.last().unwrap().path, expected_path);
        let next = redo.pop().unwrap();
        undo.push(std::mem::replace(&mut project, next));
        assert_eq!(bytes(&project), expected_after);
        assert_eq!(project.audio_assets.last().unwrap().path, expected_path);
    }

    #[test]
    fn history_preserves_an_unobserved_preceding_edit_as_its_own_step() {
        let mut project = fixture();
        let initial = bytes(&project);
        let mut history = project.clone();
        let mut fingerprint = project_fingerprint(&project);
        project.name = "Unobserved preceding edit".into();
        let before_conversion = bytes(&project);
        let candidate = prepare(&project, 0);
        let mut undo = Vec::new();
        let mut redo = Vec::new();
        let mut dirty = false;
        commit_rendered_occurrence_history_transaction(
            context(),
            &mut project,
            &mut history,
            &mut fingerprint,
            &mut undo,
            &mut redo,
            &mut dirty,
            candidate,
        )
        .unwrap();
        assert_eq!(undo.len(), 2);
        assert_eq!(bytes(&undo.pop().unwrap()), before_conversion);
        assert_eq!(bytes(&undo.pop().unwrap()), initial);
    }

    #[test]
    fn stale_context_or_any_persisted_edit_leaves_entire_history_untouched() {
        let mut control = fixture();
        let candidate = prepare(&control, 0);
        let mut control_history = control.clone();
        let mut control_fingerprint = project_fingerprint(&control);
        commit_rendered_occurrence_history_transaction(
            context(),
            &mut control,
            &mut control_history,
            &mut control_fingerprint,
            &mut Vec::new(),
            &mut Vec::new(),
            &mut false,
            candidate,
        )
        .unwrap();
        for variant in 0..6 {
            let mut project = fixture();
            let candidate = prepare(&project, 0);
            let expected_path = candidate.published_path.clone();
            let mut history = project.clone();
            let mut fingerprint = project_fingerprint(&project);
            let mut undo = vec![project.clone()];
            let mut redo = vec![project.clone()];
            let mut dirty = variant % 2 == 0;
            let mut now = context();
            match variant {
                0 => now.project_session += 1,
                1 => now.source_revision += 1,
                2 => now.control_revision += 1,
                3 => project.name.push_str(" changed elsewhere"),
                4 => project.clips[1].gain = 0.75,
                _ => project.plugin_instances[0].opaque_state.push(99),
            }
            let before = history_bytes(&project, &history, fingerprint, &undo, &redo, dirty);
            let error = commit_rendered_occurrence_history_transaction(
                now,
                &mut project,
                &mut history,
                &mut fingerprint,
                &mut undo,
                &mut redo,
                &mut dirty,
                candidate,
            )
            .unwrap_err();
            assert_eq!(error.kind, OccurrenceErrorKind::Stale);
            assert_eq!(
                error.reason,
                if variant < 3 {
                    "The project session or source/control revision changed"
                } else {
                    "The Project changed after occurrence preparation"
                }
            );
            assert_eq!(error.published_path, Some(expected_path));
            assert_eq!(
                history_bytes(&project, &history, fingerprint, &undo, &redo, dirty),
                before
            );
        }
    }

    #[test]
    fn history_no_op_is_rejected_before_any_bookkeeping_change() {
        let mut project = fixture();
        let mut candidate = prepare(&project, 0);
        let expected_path = candidate.published_path.clone();
        // A private fixture seam exercises the helper's defensive no-op path;
        // production callers cannot replace the sealed candidate Project.
        candidate.project = project.clone();
        let mut history = project.clone();
        history.name = "Older observed history".into();
        let mut fingerprint = project_fingerprint(&history);
        let mut undo = vec![history.clone()];
        let mut redo = vec![project.clone()];
        let mut dirty = false;
        let before = history_bytes(&project, &history, fingerprint, &undo, &redo, dirty);
        let error = commit_rendered_occurrence_history_transaction(
            context(),
            &mut project,
            &mut history,
            &mut fingerprint,
            &mut undo,
            &mut redo,
            &mut dirty,
            candidate,
        )
        .unwrap_err();
        assert_eq!(error.kind, OccurrenceErrorKind::History);
        assert_eq!(
            error.reason,
            "The rendered occurrence transaction is a no-op"
        );
        assert_eq!(error.published_path, Some(expected_path));
        assert_eq!(
            history_bytes(&project, &history, fingerprint, &undo, &redo, dirty),
            before
        );
    }
}
