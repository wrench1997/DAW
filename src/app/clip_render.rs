//! Dormant, pure selected-occurrence preparation. No file/runtime/UI actions.
//! Context and published-media attestation must come from a later qualified adapter.
use super::*;
use crate::{
    model::{AudioLengthReference, PluginFormat},
    tempo_map::TempoMap,
    timeline::{
        CompiledTimeline, NoteSourceDescriptor, TimelineCompileOptions, TimelineEvent,
        TimelineEventKind,
    },
};
use std::io::Write;

#[path = "clip_render_candidate.rs"]
mod candidate;

const MAX_SNAPSHOT_BYTES: usize = 64 * 1024 * 1024;
const MAX_OCCURRENCE_FRAMES: u64 = 24_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OccurrenceContext {
    project_session: u64,
    source_revision: u64,
    control_revision: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OccurrenceErrorKind {
    Source,
    Context,
    Unsupported,
    Range,
    Timeline,
    Media,
    Capacity,
    Stale,
    History,
}
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{reason}")]
struct OccurrenceError {
    kind: OccurrenceErrorKind,
    reason: String,
    published_path: Option<PathBuf>,
}
fn failure(kind: OccurrenceErrorKind, reason: impl Into<String>) -> OccurrenceError {
    OccurrenceError {
        kind,
        reason: reason.into(),
        published_path: None,
    }
}

/// Exact persisted bytes, not a hash-only stale guard. Bound encoding before
/// allocation grows; failed serialization never becomes an empty fingerprint.
fn project_snapshot(project: &Project) -> Result<Arc<[u8]>, OccurrenceError> {
    struct Bounded(Vec<u8>);
    impl Write for Bounded {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self
                .0
                .len()
                .checked_add(bytes.len())
                .is_none_or(|len| len > MAX_SNAPSHOT_BYTES)
            {
                return Err(std::io::Error::other("Project snapshot exceeds 64 MiB"));
            }
            self.0
                .try_reserve(bytes.len())
                .map_err(std::io::Error::other)?;
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut output = Bounded(Vec::new());
    serde_json::to_writer(&mut output, project).map_err(|e| {
        failure(
            OccurrenceErrorKind::Source,
            format!("Unable to capture exact Project snapshot: {e}"),
        )
    })?;
    // serde_json encodes nonfinite floats as null. Refuse a snapshot which
    // cannot round-trip as a Project instead of treating NaN/Inf as equal nulls.
    serde_json::from_slice::<Project>(&output.0).map_err(|e| {
        failure(
            OccurrenceErrorKind::Source,
            format!("Project snapshot is not persistable: {e}"),
        )
    })?;
    Ok(output.0.into())
}

struct OccurrencePlan {
    snapshot: Arc<[u8]>,
    context: OccurrenceContext,
    clip_id: u32,
    pattern_id: u32,
    channel_id: u32,
    instance_id: u64,
    start_beat: f32,
    length_beats: f32,
    row: usize,
    start_frame: u64,
    end_frame: u64,
    sample_rate: u32,
    tail_frames: u32,
    channel_gain: f32,
    mixer_track_id: MixerTrackId,
    tempo_map: TempoMap,
    local_events: Vec<TimelineEvent>,
}
impl OccurrencePlan {
    fn prepare(
        project: &Project,
        context: OccurrenceContext,
        clip_id: u32,
        sample_rate: u32,
        tail_frames: u32,
    ) -> Result<Self, OccurrenceError> {
        if context.project_session == 0 || context.source_revision == 0 {
            return Err(failure(
                OccurrenceErrorKind::Context,
                "A current Project session and source revision are required",
            ));
        }
        if !(8000..=192000).contains(&sample_rate) || tail_frames > sample_rate.saturating_mul(30) {
            return Err(failure(
                OccurrenceErrorKind::Range,
                "Occurrence render requires 8000–192000 Hz and a fixed tail of at most 30 seconds",
            ));
        }
        let snapshot = project_snapshot(project)?;
        let clips: Vec<_> = project.clips.iter().filter(|c| c.id == clip_id).collect();
        let [clip] = clips.as_slice() else {
            return Err(failure(
                OccurrenceErrorKind::Source,
                "The source Clip is missing or duplicated",
            ));
        };
        if clip.id == 0
            || clip.kind != ClipKind::Pattern
            || clip.muted
            || clip.track >= PLAYLIST_TRACK_COUNT
        {
            return Err(failure(
                OccurrenceErrorKind::Source,
                "One unmuted Pattern Clip on a valid Playlist row is required",
            ));
        }
        let patterns: Vec<_> = project
            .patterns
            .iter()
            .filter(|p| p.id == clip.pattern_id)
            .collect();
        let [pattern] = patterns.as_slice() else {
            return Err(failure(
                OccurrenceErrorKind::Source,
                "The source Pattern is missing or duplicated",
            ));
        };
        if pattern.id == 0 || pattern.channel_steps.len() != project.channels.len() {
            return Err(failure(
                OccurrenceErrorKind::Source,
                "Pattern channel membership is invalid",
            ));
        }
        let mut members = HashSet::new();
        for (index, steps) in pattern.channel_steps.iter().enumerate() {
            if steps.iter().any(|enabled| *enabled) {
                members.insert(project.channels[index].id);
            }
        }
        for note in pattern.notes.iter().filter(|note| !note.muted) {
            members.insert(note.channel_id.ok_or_else(|| {
                failure(
                    OccurrenceErrorKind::Source,
                    "The Pattern has an unassigned Piano note",
                )
            })?);
        }
        if members.len() != 1 {
            return Err(failure(
                OccurrenceErrorKind::Unsupported,
                "The selected Pattern must contain exactly one musical Channel; other channels are never omitted",
            ));
        }
        let channel_id = *members.iter().next().expect("one member");
        let channels: Vec<_> = project
            .channels
            .iter()
            .filter(|c| c.id == channel_id)
            .collect();
        let [channel] = channels.as_slice() else {
            return Err(failure(
                OccurrenceErrorKind::Source,
                "The source Channel is missing or duplicated",
            ));
        };
        if channel_id == 0
            || channel.muted
            || (project.channels.iter().any(|c| c.solo) && !channel.solo)
        {
            return Err(failure(
                OccurrenceErrorKind::Unsupported,
                "The source Channel is muted or excluded by Solo",
            ));
        }
        if !channel.volume.is_finite()
            || !(0.0..=1.5).contains(&channel.volume)
            || channel.pan != 0.0
        {
            return Err(failure(
                OccurrenceErrorKind::Unsupported,
                "The source requires Channel volume in 0..=1.5 and centered Channel pan",
            ));
        }
        let instance_id = channel.instrument_plugin_instance_id.ok_or_else(|| {
            failure(
                OccurrenceErrorKind::Unsupported,
                "The source Channel has no VST3 instrument",
            )
        })?;
        let instances: Vec<_> = project
            .plugin_instances
            .iter()
            .filter(|p| p.id == instance_id)
            .collect();
        let [instance] = instances.as_slice() else {
            return Err(failure(
                OccurrenceErrorKind::Source,
                "The source instrument is missing or duplicated",
            ));
        };
        if instance_id == 0
            || instance.format != PluginFormat::Vst3
            || instance.role != PluginRole::Instrument
            || !instance.enabled
            || instance.bypass
            || instance.wet != 1.0
            || instance.midi_ports.audio_monitor_muted
        {
            return Err(failure(
                OccurrenceErrorKind::Unsupported,
                "The source requires an enabled fully-wet unbypassed VST3 generator with monitoring enabled",
            ));
        }
        if project
            .automation_lanes
            .iter()
            .any(|lane| lane.lane.is_enabled() && !lane.lane.points().is_empty())
        {
            return Err(failure(
                OccurrenceErrorKind::Unsupported,
                "Active automation is unsupported for occurrence conversion",
            ));
        }
        if project
            .plugin_instances
            .iter()
            .any(|p| p.midi_ports.input.is_some() || p.midi_ports.output.is_some())
        {
            return Err(failure(
                OccurrenceErrorKind::Unsupported,
                "Configured MIDI routing is unsupported for occurrence conversion",
            ));
        }
        project.validate_mixer_graph().map_err(|e| {
            failure(
                OccurrenceErrorKind::Source,
                format!("Invalid stable Mixer graph: {e}"),
            )
        })?;
        let tempo_map = TempoMap::from_project(project, sample_rate)
            .map_err(|e| failure(OccurrenceErrorKind::Timeline, e.to_string()))?;
        let timeline =
            CompiledTimeline::from_project(project, &tempo_map, TimelineCompileOptions::default())
                .map_err(|e| failure(OccurrenceErrorKind::Timeline, e.to_string()))?;
        if !timeline.diagnostics().is_empty()
            || timeline.stats().diagnostics_suppressed != 0
            || timeline.stats().invalid_items_skipped != 0
        {
            return Err(failure(
                OccurrenceErrorKind::Timeline,
                format!(
                    "Occurrence conversion refuses Timeline diagnostics: {:?}",
                    timeline.diagnostics().first()
                ),
            ));
        }
        let start_frame = tempo_map
            .beat_to_frame(f64::from(clip.start))
            .map_err(|e| failure(OccurrenceErrorKind::Range, e.to_string()))?;
        let end_frame = tempo_map
            .beat_to_frame(f64::from(clip.start) + f64::from(clip.length))
            .map_err(|e| failure(OccurrenceErrorKind::Range, e.to_string()))?;
        let length = end_frame
            .checked_sub(start_frame)
            .filter(|n| *n != 0)
            .ok_or_else(|| {
                failure(
                    OccurrenceErrorKind::Range,
                    "The source occurrence has no positive compiled interval",
                )
            })?;
        let stop = end_frame
            .checked_add(u64::from(tail_frames))
            .ok_or_else(|| failure(OccurrenceErrorKind::Range, "Occurrence tail overflow"))?;
        if stop > timeline.duration_frames() {
            return Err(failure(
                OccurrenceErrorKind::Range,
                "The chosen tail extends beyond song end; automatic extension is unsupported",
            ));
        }
        let reserved = length
            .checked_add(u64::from(tail_frames))
            .and_then(|n| n.checked_add(u64::from(sample_rate)))
            .and_then(|n| n.checked_add(127))
            .map(|n| n / 128 * 128);
        if length > u64::from(sample_rate) * 600
            || reserved.is_none_or(|n| n > MAX_OCCURRENCE_FRAMES)
        {
            return Err(failure(
                OccurrenceErrorKind::Capacity,
                "Occurrence exceeds ten-minute / 24-million-frame bounds",
            ));
        }
        let local_events = select_occurrence_events(
            project,
            timeline.events(),
            clip.id,
            pattern.id,
            channel_id,
            instance_id,
            start_frame,
            end_frame,
            stop,
        )?;
        Ok(Self {
            snapshot,
            context,
            clip_id,
            pattern_id: pattern.id,
            channel_id,
            instance_id,
            start_beat: clip.start,
            length_beats: clip.length,
            row: clip.track,
            start_frame,
            end_frame,
            sample_rate,
            tail_frames,
            channel_gain: channel.volume,
            mixer_track_id: channel.mixer_track,
            tempo_map,
            local_events,
        })
    }
    fn check_current(
        &self,
        project: &Project,
        context: OccurrenceContext,
    ) -> Result<(), OccurrenceError> {
        if self.context != context {
            return Err(failure(
                OccurrenceErrorKind::Stale,
                "The source session or source/control revision changed",
            ));
        }
        if self.snapshot != project_snapshot(project)? {
            return Err(failure(
                OccurrenceErrorKind::Stale,
                "The Project changed after occurrence admission",
            ));
        }
        Ok(())
    }
}

fn source_matches(source: NoteSourceDescriptor, clip_id: u32, pattern_id: u32) -> bool {
    match source {
        NoteSourceDescriptor::ChannelStep {
            clip_id: clip,
            pattern_id: pattern,
            ..
        }
        | NoteSourceDescriptor::LegacyPiano {
            clip_id: clip,
            pattern_id: pattern,
            ..
        } => clip == clip_id && pattern == pattern_id,
    }
}
#[derive(Clone, Copy)]
struct ActiveOccurrenceNote {
    frame: u64,
    channel: u32,
    pitch: u8,
    track: u16,
    source: NoteSourceDescriptor,
}
#[allow(clippy::too_many_arguments)]
fn select_occurrence_events(
    project: &Project,
    events: &[TimelineEvent],
    clip_id: u32,
    pattern_id: u32,
    selected_channel: u32,
    instance_id: u64,
    start: u64,
    end: u64,
    tail_end: u64,
) -> Result<Vec<TimelineEvent>, OccurrenceError> {
    let generator_channels: HashSet<_> = project
        .channels
        .iter()
        .filter(|c| c.instrument_plugin_instance_id == Some(instance_id))
        .map(|c| c.id)
        .collect();
    let mut active = HashMap::<u64, ActiveOccurrenceNote>::new();
    let mut selected = Vec::new();
    selected
        .try_reserve(events.len())
        .map_err(|e| failure(OccurrenceErrorKind::Capacity, e.to_string()))?;
    active
        .try_reserve(events.len())
        .map_err(|e| failure(OccurrenceErrorKind::Capacity, e.to_string()))?;
    for event in events {
        match event.kind {
            TimelineEventKind::NoteOn {
                channel_id, source, ..
            }
            | TimelineEventKind::NoteOff {
                channel_id, source, ..
            } if source_matches(source, clip_id, pattern_id) && channel_id != selected_channel => {
                return Err(failure(
                    OccurrenceErrorKind::Timeline,
                    "Selected source descriptor points to another Channel",
                ));
            }
            _ => {}
        }
        let selected_event = match event.kind {
            TimelineEventKind::NoteOn {
                note_id,
                channel_id,
                note,
                mixer_track,
                source,
                ..
            } if generator_channels.contains(&channel_id) => {
                if active
                    .insert(
                        note_id,
                        ActiveOccurrenceNote {
                            frame: event.frame,
                            channel: channel_id,
                            pitch: note,
                            track: mixer_track,
                            source,
                        },
                    )
                    .is_some()
                {
                    return Err(failure(
                        OccurrenceErrorKind::Timeline,
                        "Duplicate active note identity",
                    ));
                }
                let belongs = source_matches(source, clip_id, pattern_id);
                if belongs
                    && (channel_id != selected_channel || event.frame < start || event.frame >= end)
                {
                    return Err(failure(
                        OccurrenceErrorKind::Timeline,
                        "Selected NoteOn falls outside its exact source interval/channel",
                    ));
                }
                belongs
            }
            TimelineEventKind::NoteOff {
                note_id,
                channel_id,
                note,
                mixer_track,
                source,
            } if generator_channels.contains(&channel_id) => {
                let on = active.remove(&note_id).ok_or_else(|| {
                    failure(
                        OccurrenceErrorKind::Timeline,
                        "NoteOff has no admitted NoteOn identity",
                    )
                })?;
                if event.frame <= on.frame
                    || on.channel != channel_id
                    || on.pitch != note
                    || on.track != mixer_track
                    || on.source != source
                {
                    return Err(failure(
                        OccurrenceErrorKind::Timeline,
                        "NoteOff does not match its complete NoteOn identity",
                    ));
                }
                let belongs = source_matches(source, clip_id, pattern_id);
                if belongs && event.frame > end {
                    return Err(failure(
                        OccurrenceErrorKind::Timeline,
                        "Selected NoteOff crosses its occurrence end",
                    ));
                }
                if !belongs && on.frame < tail_end && event.frame > start {
                    return Err(failure(
                        OccurrenceErrorKind::Unsupported,
                        "Another occurrence of the same generator overlaps or crosses the selected interval/tail",
                    ));
                }
                belongs
            }
            _ => false,
        };
        if selected_event {
            selected.push(TimelineEvent {
                frame: event.frame - start,
                kind: event.kind,
            });
        }
    }
    if !active.is_empty() {
        return Err(failure(
            OccurrenceErrorKind::Timeline,
            "Unclosed same-generator note identities",
        ));
    }
    if selected.is_empty() {
        return Err(failure(
            OccurrenceErrorKind::Unsupported,
            "The selected source offset/range contains no compiled notes",
        ));
    }
    Ok(selected)
}

/// Data-only fixture boundary. No production constructor exists until an
/// adapter can attest immutable publication, capture identity and exact bytes.
struct RenderedOccurrenceMedia {
    snapshot: Arc<[u8]>,
    context: OccurrenceContext,
    clip_id: u32,
    pattern_id: u32,
    channel_id: u32,
    instance_id: u64,
    publication_id: u64,
    path: PathBuf,
    asset: wav::WavAsset,
    prewrite_peak: f32,
}
fn validate_rendered_media(
    plan: &OccurrencePlan,
    media: &RenderedOccurrenceMedia,
) -> Result<(), OccurrenceError> {
    let invalid = |reason| failure(OccurrenceErrorKind::Media, reason);
    if media.snapshot != plan.snapshot
        || media.context != plan.context
        || media.clip_id != plan.clip_id
        || media.pattern_id != plan.pattern_id
        || media.channel_id != plan.channel_id
        || media.instance_id != plan.instance_id
    {
        return Err(invalid(
            "Rendered media belongs to a different source occurrence/context",
        ));
    }
    if media.publication_id == 0 || !media.path.is_absolute() || media.path.to_str().is_none() {
        return Err(invalid(
            "A uniquely published absolute UTF-8 media path is required",
        ));
    }
    let frames = plan
        .end_frame
        .checked_sub(plan.start_frame)
        .and_then(|n| n.checked_add(u64::from(plan.tail_frames)))
        .ok_or_else(|| invalid("Rendered frame count overflow"))?;
    let samples = usize::try_from(frames)
        .ok()
        .and_then(|n| n.checked_mul(2))
        .ok_or_else(|| invalid("Rendered stereo sample count overflow"))?;
    let data_bytes = frames
        .checked_mul(8)
        .ok_or_else(|| invalid("Rendered data size overflow"))?;
    let metadata = &media.asset.metadata;
    if metadata.sample_rate != plan.sample_rate
        || metadata.channels != 2
        || metadata.frames != frames
        || metadata.encoding != wav::WavSampleEncoding::IeeeFloat
        || metadata.bits_per_sample != 32
        || metadata.valid_bits_per_sample != 32
        || !matches!(metadata.channel_mask, None | Some(3))
        || metadata.block_align != 8
        || metadata.byte_rate != plan.sample_rate * 8
        || metadata.data_bytes != data_bytes
        || !metadata.duration_seconds.is_finite()
        || metadata.duration_seconds != frames as f64 / f64::from(plan.sample_rate)
        || media.asset.samples.len() != samples
    {
        return Err(invalid(
            "Rendered stereo Float32 PCM/metadata/extent does not exactly match the occurrence",
        ));
    }
    if !media.prewrite_peak.is_finite() || !(0.0..=1.0).contains(&media.prewrite_peak) {
        return Err(invalid(
            "Dry render headroom exceeds the current lossless WAV import subset; retain the published render",
        ));
    }
    let mut peak = 0.0_f32;
    for sample in &media.asset.samples {
        if !sample.is_finite() || sample.abs() > 1.0 {
            return Err(invalid(
                "Rendered PCM is nonfinite or would be clipped on reload; retain the published render",
            ));
        }
        peak = peak.max(sample.abs());
    }
    if peak != media.prewrite_peak {
        return Err(invalid(
            "Rendered PCM peak differs from the prewrite receipt",
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "clip_render_tests.rs"]
mod tests;
