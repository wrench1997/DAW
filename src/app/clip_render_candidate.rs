//! Pure selected-occurrence reinsertion from a producer-verified media receipt.
//!
//! No file is opened, registered, moved, or deleted here. In particular, every
//! failure returns the published path so its future owner can retain the media.

use super::*;
use crate::{
    instrument_bounce::occurrence::MatchedOccurrenceSource,
    occurrence_media::VerifiedOccurrenceMedia,
};

/// Only this module can construct or alter the admitted Project replacement.
pub(in crate::app) struct PreparedOccurrenceCandidate {
    expected_snapshot: Arc<[u8]>,
    expected_context: OccurrenceContext,
    authoritative_before: Project,
    project: Project,
    next_history_snapshot: Project,
    after_fingerprint: u64,
    source_clip_id: u32,
    audio_clip_id: u32,
    asset_id: u64,
    published_path: PathBuf,
}

#[derive(Debug, PartialEq, Eq)]
pub(in crate::app) struct OccurrenceHistoryCommit {
    pub source_clip_id: u32,
    pub audio_clip_id: u32,
    pub asset_id: u64,
    pub published_path: PathBuf,
}

fn retain_published_path(mut error: OccurrenceError, path: &Path) -> OccurrenceError {
    error.published_path = Some(path.to_path_buf());
    error
}

pub(in crate::app) fn prepare_rendered_occurrence_candidate(
    project: &Project,
    context: OccurrenceContext,
    plan: &OccurrencePlan,
    media: &VerifiedOccurrenceMedia,
    source: &MatchedOccurrenceSource,
) -> Result<PreparedOccurrenceCandidate, OccurrenceError> {
    prepare_candidate(project, context, plan, media, source)
        .map_err(|error| retain_published_path(error, media.path()))
}

fn prepare_candidate(
    project: &Project,
    context: OccurrenceContext,
    plan: &OccurrencePlan,
    media: &VerifiedOccurrenceMedia,
    source: &MatchedOccurrenceSource,
) -> Result<PreparedOccurrenceCandidate, OccurrenceError> {
    // Admission precedes all Project image copies. The sealed source exists
    // only after a distinct second actual capture matched the rendered source.
    plan.check_current(project, context)?;
    validate_rendered_media(plan, media)?;
    if !media.provenance().same_capture(source.provenance())
        || !std::ptr::eq(source.provenance().request(), plan.request.as_ref())
    {
        return Err(failure(
            OccurrenceErrorKind::Source,
            "Authoritative source recapture differs from the rendered capture",
        ));
    }
    let source_instance_index = project
        .plugin_instances
        .iter()
        .position(|instance| instance.id == plan.instance_id)
        .ok_or_else(|| {
            failure(
                OccurrenceErrorKind::Source,
                "The retained generator is missing",
            )
        })?;
    let existing = &project.plugin_instances[source_instance_index];
    if source.identity().instance_id != existing.id
        || existing.parameters.len() != source.parameter_bases().len()
        || !existing
            .parameters
            .keys()
            .copied()
            .eq(source.parameter_bases().iter().map(|(id, _)| *id))
        || source
            .parameter_bases()
            .iter()
            .any(|(_, value)| !value.is_finite() || !(0.0..=1.0).contains(value))
        || source.opaque_state().is_empty()
        || source.opaque_state().len() > MAX_SNAPSHOT_BYTES
    {
        return Err(failure(
            OccurrenceErrorKind::Source,
            "Actual capture must contain exactly the existing generic-base IDs and bounded opaque state",
        ));
    }

    if !plan.channel_gain.is_finite() || !(0.0..=1.0).contains(&plan.channel_gain) {
        return Err(failure(
            OccurrenceErrorKind::Unsupported,
            "The captured Channel volume is outside the supported source range",
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
        .path()
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("Rendered occurrence.wav")
        .to_owned();
    let metadata = &media.asset().metadata;
    let asset = AudioAsset {
        id: asset_id,
        name: name.clone(),
        path: media.path().to_path_buf(),
        sample_rate: metadata.sample_rate,
        channels: metadata.channels,
        bits_per_sample: metadata.bits_per_sample,
        frames: metadata.frames,
        waveform_peaks: build_waveform_peaks(&media.asset().samples, metadata.channels, 256),
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
    // Image 1: reconcile only this retained instance's actual captured state
    // and the admitted existing bases. Descriptor UID is never a factory UID.
    let mut authoritative_before = project.clone();
    let target = &mut authoritative_before.plugin_instances[source_instance_index];
    reservation_gate(ReservationSite::AuthoritativeState)?;
    target.opaque_state.clear();
    target
        .opaque_state
        .try_reserve_exact(source.opaque_state().len())
        .map_err(|error| failure(OccurrenceErrorKind::Capacity, error.to_string()))?;
    target.opaque_state.extend_from_slice(source.opaque_state());
    for &(id, value) in source.parameter_bases() {
        *target
            .parameters
            .get_mut(&id)
            .expect("exact existing IDs admitted") = value;
    }
    let before_timeline = compile_candidate_timeline(&authoritative_before, &plan.tempo_map)?;
    if before_timeline.events() != source_timeline.events()
        || before_timeline.audio_clips() != source_timeline.audio_clips()
        || before_timeline.channel_bases() != source_timeline.channel_bases()
        || before_timeline.plugin_routes() != source_timeline.plugin_routes()
        || before_timeline.duration_frames() != source_timeline.duration_frames()
    {
        return Err(failure(
            OccurrenceErrorKind::Timeline,
            "Actual state reconciliation changes the selected compiled source",
        ));
    }
    // One temporary decode while only the first image is owned. Release the
    // canonical verification bytes before allocating the next image.
    let before_bytes = serialize_project_bounded(&authoritative_before)?;
    drop(decode_exact_project(&before_bytes)?);
    drop(before_bytes);
    drop(before_timeline);

    // Image 2 contains exactly the authoritative before-state plus conversion.
    let mut candidate = authoritative_before.clone();
    candidate.clips[source_index].muted = true;
    candidate.plugin_instances[source_instance_index].enabled = false;
    reservation_gate(ReservationSite::CandidateAssets)?;
    candidate
        .audio_assets
        .try_reserve(1)
        .map_err(|error| failure(OccurrenceErrorKind::Capacity, error.to_string()))?;
    reservation_gate(ReservationSite::CandidateClips)?;
    candidate
        .clips
        .try_reserve(1)
        .map_err(|error| failure(OccurrenceErrorKind::Capacity, error.to_string()))?;
    reservation_gate(ReservationSite::CandidateRoutes)?;
    candidate
        .audio_clip_mixer_destinations
        .try_reserve(1)
        .map_err(|error| failure(OccurrenceErrorKind::Capacity, error.to_string()))?;
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
    drop(source_timeline);

    // A save/reopen must preserve the compiled proof without Project::normalize
    // silently repairing geometry, routing, identities, or source metadata.
    let bytes = serialize_project_bounded(&candidate)?;
    let after_fingerprint = snapshot_fingerprint(&bytes);
    if bytes.as_slice() == plan.snapshot.as_ref() {
        return Err(failure(
            OccurrenceErrorKind::History,
            "The rendered occurrence transaction is a no-op",
        ));
    }
    // Image 3 is the verified decoded after-image. Never create a fourth
    // temporary Project after this point. Restore nonpersisted runtime fields
    // from the existing images without changing persisted equality.
    let mut round_tripped = decode_exact_project(&bytes)?;
    for (decoded, current) in round_tripped
        .plugin_instances
        .iter_mut()
        .zip(&candidate.plugin_instances)
    {
        decoded.runtime_status = current.runtime_status;
    }
    round_tripped.piano_notes.clone_from(&candidate.piano_notes);
    round_tripped
        .migration_diagnostics
        .clone_from(&candidate.migration_diagnostics);
    drop(bytes);
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
        authoritative_before,
        project: candidate,
        next_history_snapshot: round_tripped,
        after_fingerprint,
        source_clip_id: plan.clip_id,
        audio_clip_id,
        asset_id,
        published_path: media.path().to_path_buf(),
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

/// Prepared entirely before runtime removal. The later App fence must retain
/// the same Project/history owners until the exact callback removal receipt.
pub(in crate::app) struct PreparedOccurrenceHistory {
    expected_snapshot: Arc<[u8]>,
    expected_context: OccurrenceContext,
    expected_history_fingerprint: u64,
    expected_undo_len: usize,
    expected_redo_len: usize,
    history: PreparedExplicitBeforeHistory,
    result: OccurrenceHistoryCommit,
}

#[allow(clippy::too_many_arguments)]
pub(in crate::app) fn prepare_rendered_occurrence_history_transaction(
    context: OccurrenceContext,
    project: &Project,
    history_fingerprint: u64,
    undo_stack: &mut Vec<Project>,
    redo_stack: &[Project],
    candidate: PreparedOccurrenceCandidate,
) -> Result<PreparedOccurrenceHistory, OccurrenceError> {
    let path = &candidate.published_path;
    if context != candidate.expected_context {
        return Err(retain_published_path(
            failure(
                OccurrenceErrorKind::Stale,
                "The exact occurrence source/runtime stamp changed",
            ),
            path,
        ));
    }
    if !project_matches_snapshot(project, &candidate.expected_snapshot)
        .map_err(|error| retain_published_path(error, path))?
    {
        return Err(retain_published_path(
            failure(
                OccurrenceErrorKind::Stale,
                "The Project changed after occurrence preparation",
            ),
            path,
        ));
    }
    if project_matches_snapshot(&candidate.project, &candidate.expected_snapshot)
        .map_err(|error| retain_published_path(error, path))?
    {
        return Err(retain_published_path(
            failure(
                OccurrenceErrorKind::History,
                "The rendered occurrence transaction is a no-op",
            ),
            path,
        ));
    }
    let expected_undo_len = undo_stack.len();
    let history = prepare_explicit_before_project_history_transaction(
        snapshot_fingerprint(&candidate.expected_snapshot),
        history_fingerprint,
        candidate.after_fingerprint,
        undo_stack,
        candidate.authoritative_before,
        candidate.project,
        candidate.next_history_snapshot,
    )
    .map_err(|reason| {
        retain_published_path(failure(OccurrenceErrorKind::Capacity, reason), path)
    })?;
    Ok(PreparedOccurrenceHistory {
        expected_snapshot: candidate.expected_snapshot,
        expected_context: context,
        expected_history_fingerprint: history_fingerprint,
        expected_undo_len,
        expected_redo_len: redo_stack.len(),
        history,
        result: OccurrenceHistoryCommit {
            source_clip_id: candidate.source_clip_id,
            audio_clip_id: candidate.audio_clip_id,
            asset_id: candidate.asset_id,
            published_path: candidate.published_path,
        },
    })
}

impl PreparedOccurrenceHistory {
    pub(in crate::app) fn result(&self) -> &OccurrenceHistoryCommit {
        &self.result
    }

    /// The last fallible comparison belongs BEFORE CommitAccepted/removal.
    /// It serializes into the comparison writer without another canonical
    /// buffer or Project clone. Existing serde-owned field allocations remain
    /// possible, so this never runs after successful callback removal.
    pub(in crate::app) fn revalidate(
        &self,
        project: &Project,
        context: OccurrenceContext,
        history_fingerprint: u64,
        undo_len: usize,
        redo_len: usize,
    ) -> Result<(), OccurrenceError> {
        if context != self.expected_context
            || history_fingerprint != self.expected_history_fingerprint
            || undo_len != self.expected_undo_len
            || redo_len != self.expected_redo_len
            || !project_matches_snapshot(project, &self.expected_snapshot)
                .map_err(|error| retain_published_path(error, &self.result.published_path))?
        {
            return Err(retain_published_path(
                failure(
                    OccurrenceErrorKind::Stale,
                    "The prepared conversion or history changed before removal",
                ),
                &self.result.published_path,
            ));
        }
        Ok(())
    }

    /// The owner invokes this once only after its exact removal acknowledgment.
    /// The held fence and prior revalidate are prerequisites, not inferred here.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::app) fn apply(
        self,
        project: &mut Project,
        history_snapshot: &mut Project,
        history_fingerprint: &mut u64,
        undo_stack: &mut Vec<Project>,
        redo_stack: &mut Vec<Project>,
        dirty: &mut bool,
    ) -> OccurrenceHistoryCommit {
        apply_explicit_before_project_history_transaction(
            project,
            history_snapshot,
            history_fingerprint,
            undo_stack,
            redo_stack,
            dirty,
            self.history,
        );
        self.result
    }
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn commit_rendered_occurrence_history_transaction(
    context: OccurrenceContext,
    project: &mut Project,
    history_snapshot: &mut Project,
    history_fingerprint: &mut u64,
    undo_stack: &mut Vec<Project>,
    redo_stack: &mut Vec<Project>,
    dirty: &mut bool,
    candidate: PreparedOccurrenceCandidate,
) -> Result<OccurrenceHistoryCommit, OccurrenceError> {
    let prepared = prepare_rendered_occurrence_history_transaction(
        context,
        project,
        *history_fingerprint,
        undo_stack,
        redo_stack,
        candidate,
    )?;
    prepared.revalidate(
        project,
        context,
        *history_fingerprint,
        undo_stack.len(),
        redo_stack.len(),
    )?;
    Ok(prepared.apply(
        project,
        history_snapshot,
        history_fingerprint,
        undo_stack,
        redo_stack,
        dirty,
    ))
}

#[cfg(test)]
pub(in crate::app) fn test_commit_verified_occurrence(
    project: &mut Project,
    media: &VerifiedOccurrenceMedia,
    source: &MatchedOccurrenceSource,
) -> Result<PathBuf, String> {
    let request = source.provenance().request_handle();
    let context = request.stamp();
    let mut plan = OccurrencePlan::prepare(
        project,
        context,
        request.clip_id(),
        request.sample_rate(),
        request.tail_frames(),
        Some(Arc::clone(request.consent())),
    )
    .map_err(|error| error.to_string())?;
    // Reconstruct the entire pure planner proof, then share the already
    // captured request only after every render-relevant value compares equal.
    if plan.request.pattern_id() != request.pattern_id()
        || plan.request.channel_id() != request.channel_id()
        || plan.request.instance_id() != request.instance_id()
        || plan.request.start_frame() != request.start_frame()
        || plan.request.end_frame() != request.end_frame()
        || plan.request.tempo().to_bits() != request.tempo().to_bits()
        || plan.request.local_events() != request.local_events()
    {
        return Err("Integrated capture request differs from the original pure planner".into());
    }
    plan.request = Arc::clone(request);
    let original = serialize_project_bounded(project).map_err(|error| error.to_string())?;
    let prepared = prepare_rendered_occurrence_candidate(project, context, &plan, media, source)
        .map_err(|error| error.to_string())?;
    assert!(project_matches_snapshot(project, &original).unwrap());
    let actual_before = serialize_project_bounded(&prepared.authoritative_before).unwrap();
    let expected_after = serialize_project_bounded(&prepared.project).unwrap();
    let source_index = project
        .plugin_instances
        .iter()
        .position(|instance| instance.id == request.instance_id())
        .unwrap();
    assert_eq!(
        prepared.authoritative_before.plugin_instances[source_index]
            .opaque_state
            .as_slice(),
        source.opaque_state()
    );
    assert!(prepared.authoritative_before.plugin_instances[source_index].enabled);
    assert!(!prepared.project.plugin_instances[source_index].enabled);
    for &(id, value) in source.parameter_bases() {
        assert_eq!(
            prepared.authoritative_before.plugin_instances[source_index].parameters[&id].to_bits(),
            value.to_bits()
        );
    }
    let mut history = project.clone();
    history.name.push_str(" previously observed");
    let mut fingerprint = project_fingerprint(&history);
    let pending = serialize_project_bounded(&history).unwrap();
    let mut undo = Vec::new();
    let mut redo = vec![project.clone()];
    let mut dirty = false;
    let committed = commit_rendered_occurrence_history_transaction(
        context,
        project,
        &mut history,
        &mut fingerprint,
        &mut undo,
        &mut redo,
        &mut dirty,
        prepared,
    )
    .map_err(|error| error.to_string())?;
    assert!(dirty);
    assert!(redo.is_empty());
    assert_eq!(undo.len(), 2);
    assert!(project_matches_snapshot(project, &expected_after).unwrap());
    assert!(project_matches_snapshot(&undo[0], &pending).unwrap());
    assert_eq!(fingerprint, project_fingerprint(project));
    let before = undo.pop().unwrap();
    redo.push(std::mem::replace(project, before));
    assert!(project_matches_snapshot(project, &actual_before).unwrap());
    assert!(committed.published_path.exists());
    let after = redo.pop().unwrap();
    undo.push(std::mem::replace(project, after));
    assert!(project_matches_snapshot(project, &expected_after).unwrap());
    assert!(committed.published_path.exists());
    let reopened = decode_exact_project(&expected_after).unwrap();
    assert_eq!(
        reopened.audio_assets.last().unwrap().path,
        committed.published_path
    );
    assert!(!reopened.plugin_instances[source_index].enabled);
    Ok(committed.published_path)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    const RATE: u32 = 8_000;

    fn context() -> OccurrenceContext {
        OccurrenceContext {
            project_session: 19,
            project_revision: 23,
            timeline_revision: 29,
            epoch: 31,
            engine_session: 37,
            endpoint_id: 41,
            channel_id: 1,
            instance_id: 71,
            mixer_track_id: 7,
            native_generation: 0,
            app_native_dirty_revision: 0,
            native_capture_serial: 0,
            parameter_capture_serial: 0,
            native_control_request: 0,
            parameter_control_request: 0,
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
        let mut other_channel = project.channels[0].clone();
        other_channel.id = 2;
        other_channel.instrument_plugin_instance_id = None;
        other_channel.mixer_track = project.mixer_track_id_at_runtime_slot(3).unwrap();
        project.channels.push(other_channel);
        project.patterns[0].channel_steps.push([false; 16]);
        let mut other_pattern = project.patterns[0].clone();
        other_pattern.id = 88;
        other_pattern.channel_steps[0] = [false; 16];
        other_pattern.channel_steps[1][0] = true;
        for note in &mut other_pattern.notes {
            note.id += 1;
            note.channel_id = Some(2);
        }
        project.patterns.push(other_pattern);
        let mut other = project.clips[0].clone();
        other.pattern_id = 88;
        other.id = 12;
        other.start = 2.0;
        other.name = "Same Pattern, untouched".into();
        project.clips.push(other);
        // Bind the stamp to the actual stable assignment before display reordering.
        assert_eq!(context().mixer_track_id, project.channels[0].mixer_track);
        // Display order deliberately disagrees with the stable bus identity.
        project.mixer_tracks.swap(1, 7);
        project
    }

    struct FixtureMedia {
        verified: VerifiedOccurrenceMedia,
        source: MatchedOccurrenceSource,
        directory: PathBuf,
    }
    impl std::ops::Deref for FixtureMedia {
        type Target = VerifiedOccurrenceMedia;
        fn deref(&self) -> &Self::Target {
            &self.verified
        }
    }
    impl Drop for FixtureMedia {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }
    fn media(plan: &OccurrencePlan) -> FixtureMedia {
        let project: Project = serde_json::from_slice(&plan.snapshot).unwrap();
        let instance = project
            .plugin_instances
            .iter()
            .find(|instance| instance.id == plan.instance_id)
            .unwrap();
        media_with_state(
            plan,
            instance.opaque_state.clone(),
            instance
                .parameters
                .iter()
                .map(|(&id, &value)| (id, value))
                .collect(),
        )
    }
    fn media_with_state(
        plan: &OccurrencePlan,
        opaque_state: Vec<u8>,
        bases: Vec<(u32, f32)>,
    ) -> FixtureMedia {
        use crate::{
            export::StereoFrame,
            export_job::ExportControl,
            instrument_bounce::occurrence::test_capture_pair,
            occurrence_media::{publish_occurrence_render, verify_published_occurrence},
        };
        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
        static SEQUENCE: AtomicU64 = AtomicU64::new(1);
        let project: Project = serde_json::from_slice(&plan.snapshot).unwrap();
        // Fixture captures exercise real attestation comparison with distinct
        // completed-receipt tokens. No native plug-in is invoked by this test.
        let (provenance, source) =
            test_capture_pair(&project, Arc::clone(&plan.request), opaque_state, bases);
        let directory = std::env::temp_dir().join(format!(
            "citrus-occurrence-history-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&directory).unwrap();
        let frames =
            usize::try_from(plan.end_frame - plan.start_frame + u64::from(plan.tail_frames))
                .unwrap();
        let pcm = vec![
            StereoFrame {
                left: 0.25,
                right: 0.25
            };
            frames
        ];
        let published = publish_occurrence_render(
            provenance,
            &directory.join("render.wav"),
            plan.sample_rate,
            &pcm,
            &ExportControl::default(),
            Instant::now() + Duration::from_secs(60),
        )
        .unwrap();
        drop(pcm);
        let verified = verify_published_occurrence(published, &AtomicBool::new(false)).unwrap();
        FixtureMedia {
            verified,
            source,
            directory,
        }
    }

    fn admitted_plan(project: &Project, clip_id: u32, tail: u32) -> OccurrencePlan {
        let acknowledgment = Arc::new(
            FixedTailAcknowledgment::review(
                project_snapshot(project).unwrap(),
                context(),
                clip_id,
                RATE,
                tail,
            )
            .unwrap(),
        );
        OccurrencePlan::prepare(
            project,
            context(),
            clip_id,
            RATE,
            tail,
            Some(acknowledgment),
        )
        .unwrap()
    }

    fn prepare(project: &Project, tail: u32) -> PreparedOccurrenceCandidate {
        let plan = admitted_plan(project, project.clips[0].id, tail);
        let rendered = media(&plan);
        prepare_rendered_occurrence_candidate(
            project,
            context(),
            &plan,
            &rendered,
            &rendered.source,
        )
        .unwrap()
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
        let plan = admitted_plan(&project, 11, 0);
        assert!(plan.local_events.iter().any(|event| matches!(event.kind,
            TimelineEventKind::NoteOn { gain, .. } if gain == 0.4)));
        let rendered = media(&plan);
        let prepared = prepare_rendered_occurrence_candidate(
            &project,
            context(),
            &plan,
            &rendered,
            &rendered.source,
        )
        .unwrap();
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
        assert_eq!(installed.path.as_path(), rendered.path());
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
        restored.plugin_instances[0].enabled = true;
        assert_eq!(bytes(&restored), before);
    }

    #[test]
    fn candidate_preserves_frame_geometry_for_quantum_edges_and_fixed_tails() {
        for start_frame in [0, 1, 127, 128, 129] {
            for length_frames in [1, 127, 128, 129, 257] {
                for tail in [0, 1, 127, 129] {
                    let mut project = fixture();
                    project.clips[0].start = start_frame as f32 / 4_000.0;
                    project.clips[0].length = length_frames as f32 / 4_000.0;
                    let plan = admitted_plan(&project, 11, tail);
                    assert_eq!(plan.start_frame, start_frame);
                    assert_eq!(plan.end_frame, start_frame + length_frames);
                    let rendered = media(&plan);
                    let candidate = prepare_rendered_occurrence_candidate(
                        &project,
                        context(),
                        &plan,
                        &rendered,
                        &rendered.source,
                    )
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
        reverted.plugin_instances[0].enabled = true;
        reverted.clips.pop();
        reverted.audio_assets.pop();
        reverted.audio_clip_mixer_destinations.pop();
        assert_eq!(bytes(&reverted), before);
    }

    #[test]
    fn candidate_accepts_exact_song_end_and_transfers_supported_gain_without_scaling_pcm() {
        for volume in [0.0, 0.25, 0.75, 1.0] {
            let mut project = fixture();
            project.clips[0].start = 7.9375;
            project.channels[0].volume = volume;
            let plan = admitted_plan(&project, 11, 0);
            assert_eq!(plan.end_frame, plan.tempo_map.duration_frames());
            let rendered = media(&plan);
            let pcm_before = rendered.asset().samples.clone();
            let candidate = prepare_rendered_occurrence_candidate(
                &project,
                context(),
                &plan,
                &rendered,
                &rendered.source,
            )
            .unwrap();
            assert_eq!(
                candidate.project.clips.last().unwrap().gain.to_bits(),
                volume.to_bits()
            );
            assert_eq!(rendered.asset().samples, pcm_before);
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
            let plan = admitted_plan(&project, 11, 0);
            let rendered = media(&plan);
            let before = bytes(&project);
            let error = prepare_rendered_occurrence_candidate(
                &project,
                context(),
                &plan,
                &rendered,
                &rendered.source,
            )
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
            assert_eq!(error.published_path.as_deref(), Some(rendered.path()));
            assert_eq!(bytes(&project), before);
        }
    }

    #[test]
    fn candidate_refuses_unrelated_capture_or_request_and_retains_real_file() {
        let project = fixture();
        let plan = admitted_plan(&project, 11, 0);
        let rendered = media(&plan);
        let unrelated_capture = media(&plan);
        let before = bytes(&project);
        let error = prepare_rendered_occurrence_candidate(
            &project,
            context(),
            &plan,
            &rendered,
            &unrelated_capture.source,
        )
        .err()
        .unwrap();
        assert_eq!(error.kind, OccurrenceErrorKind::Source);
        assert_eq!(error.published_path.as_deref(), Some(rendered.path()));
        assert!(rendered.path().exists());
        let different_request = admitted_plan(&project, 11, 0);
        let error = prepare_rendered_occurrence_candidate(
            &project,
            context(),
            &different_request,
            &rendered,
            &rendered.source,
        )
        .err()
        .unwrap();
        assert_eq!(error.kind, OccurrenceErrorKind::Media);
        assert_eq!(bytes(&project), before);
        assert!(rendered.path().exists());
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
                1 => now.project_revision += 1,
                2 => now.parameter_control_request += 1,
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
                    "The exact occurrence source/runtime stamp changed"
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
    fn authoritative_capture_is_the_one_step_undo_image_and_preserves_pending_edit() {
        let mut project = fixture();
        project.plugin_instances[0].parameters.insert(17, 0.25);
        project.plugin_instances[0].opaque_state = vec![1, 2, 3];
        let mut history = project.clone();
        let earlier = bytes(&history);
        let mut fingerprint = project_fingerprint(&history);
        project.name = "Pending edit before conversion".into();
        let original = bytes(&project);
        let plan = admitted_plan(&project, 11, 129);
        let rendered = media_with_state(&plan, vec![9, 8, 7, 6], vec![(17, 0.75)]);
        let candidate = prepare_rendered_occurrence_candidate(
            &project,
            context(),
            &plan,
            &rendered,
            &rendered.source,
        )
        .unwrap();
        let expected_before = bytes(&candidate.authoritative_before);
        let expected_after = bytes(&candidate.project);
        assert_eq!(bytes(&project), original);
        assert!(candidate.authoritative_before.plugin_instances[0].enabled);
        assert!(!candidate.authoritative_before.clips[0].muted);
        assert!(!candidate.project.plugin_instances[0].enabled);
        assert_eq!(
            candidate.project.plugin_instances[0].uid,
            project.plugin_instances[0].uid
        );
        let mut allowed_delta_reversed = candidate.project.clone();
        allowed_delta_reversed.plugin_instances[0].enabled = true;
        allowed_delta_reversed.plugin_instances[0].parameters =
            project.plugin_instances[0].parameters.clone();
        allowed_delta_reversed.plugin_instances[0].opaque_state =
            project.plugin_instances[0].opaque_state.clone();
        allowed_delta_reversed.clips[0].muted = false;
        allowed_delta_reversed.clips.pop();
        allowed_delta_reversed.audio_assets.pop();
        allowed_delta_reversed.audio_clip_mixer_destinations.pop();
        assert_eq!(bytes(&allowed_delta_reversed), original);
        drop(allowed_delta_reversed);
        let mut undo = Vec::new();
        let mut redo = vec![project.clone()];
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
        assert!(redo.is_empty());
        assert!(dirty);
        assert_eq!(bytes(&project), expected_after);
        assert_eq!(project.plugin_instances[0].parameters[&17], 0.75);
        assert_eq!(project.plugin_instances[0].opaque_state, vec![9, 8, 7, 6]);
        assert_eq!(bytes(&undo[0]), earlier);
        let before = undo.pop().unwrap();
        redo.push(std::mem::replace(&mut project, before));
        assert_eq!(bytes(&project), expected_before);
        assert!(project.plugin_instances[0].enabled);
        assert_eq!(project.name, "Pending edit before conversion");
        assert!(rendered.path().exists());
        let after = redo.pop().unwrap();
        undo.push(std::mem::replace(&mut project, after));
        assert_eq!(bytes(&project), expected_after);
        assert_eq!(project.audio_assets.last().unwrap().path, rendered.path());
        let saved: Project = serde_json::from_slice(&bytes(&project)).unwrap();
        assert_eq!(saved.audio_assets.last().unwrap().path, rendered.path());
        assert!(rendered.path().exists());
    }

    #[test]
    fn all_new_precommit_reservation_failures_preserve_original_history_and_media() {
        for site in [
            ReservationSite::AuthoritativeState,
            ReservationSite::Snapshot,
            ReservationSite::CandidateAssets,
            ReservationSite::CandidateClips,
            ReservationSite::CandidateRoutes,
            ReservationSite::History,
        ] {
            let mut project = fixture();
            let plan = admitted_plan(&project, 11, 0);
            let rendered = media_with_state(&plan, vec![7, 8, 9], vec![(17, 0.75)]);
            let mut history = project.clone();
            history.name = "Prior snapshot".into();
            let mut fingerprint = project_fingerprint(&history);
            let mut undo = vec![history.clone()];
            let mut redo = vec![project.clone()];
            let mut dirty = false;
            let before = history_bytes(&project, &history, fingerprint, &undo, &redo, dirty);
            RESERVATION_FAILURE.with(|failure| failure.set(Some(site)));
            let error = prepare_rendered_occurrence_candidate(
                &project,
                context(),
                &plan,
                &rendered,
                &rendered.source,
            )
            .and_then(|candidate| {
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
            })
            .err()
            .unwrap();
            assert_eq!(error.kind, OccurrenceErrorKind::Capacity);
            assert_eq!(
                history_bytes(&project, &history, fingerprint, &undo, &redo, dirty),
                before
            );
            assert!(rendered.path().exists());
            assert!(RESERVATION_FAILURE.with(|failure| failure.get()).is_none());
        }
    }

    #[test]
    fn prepared_history_refuses_edit_away_back_and_history_drift_before_removal() {
        let project = fixture();
        let candidate = prepare(&project, 0);
        let fingerprint = project_fingerprint(&project);
        let mut undo = Vec::new();
        let redo = Vec::new();
        let prepared = prepare_rendered_occurrence_history_transaction(
            context(),
            &project,
            fingerprint,
            &mut undo,
            &redo,
            candidate,
        )
        .unwrap();
        prepared
            .revalidate(&project, context(), fingerprint, 0, 0)
            .unwrap();
        let mut changed = context();
        changed.project_revision += 2;
        assert_eq!(
            prepared
                .revalidate(&project, changed, fingerprint, 0, 0)
                .unwrap_err()
                .kind,
            OccurrenceErrorKind::Stale
        );
        assert_eq!(
            prepared
                .revalidate(&project, context(), fingerprint, 1, 0)
                .unwrap_err()
                .kind,
            OccurrenceErrorKind::Stale
        );
        assert_eq!(
            prepared
                .revalidate(&project, context(), fingerprint + 1, 0, 0)
                .unwrap_err()
                .kind,
            OccurrenceErrorKind::Stale
        );
    }

    #[test]
    fn conversion_preserves_the_existing_bounded_history_limit() {
        let mut project = fixture();
        let mut history = project.clone();
        let mut fingerprint = project_fingerprint(&history);
        let mut undo = (0..128)
            .map(|index| {
                let mut entry = project.clone();
                entry.name = format!("Old {index}");
                entry
            })
            .collect::<Vec<_>>();
        project.name = "A preceding unrecorded edit".into();
        let candidate = prepare(&project, 0);
        let authoritative_before = bytes(&candidate.authoritative_before);
        let pending = bytes(&history);
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
        assert_eq!(undo.len(), 128);
        assert_eq!(undo[0].name, "Old 2");
        assert_eq!(bytes(&undo[126]), pending);
        assert_eq!(bytes(&undo[127]), authoritative_before);
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
