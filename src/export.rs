use std::{
    collections::{HashMap, hash_map::Entry},
    f32::consts::TAU,
    fs::{File, OpenOptions},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use anyhow::{Context, Result, anyhow, bail, ensure};

use crate::{
    audio_clip::{audio_end_beat, audio_metadata_valid, source_elapsed_frames},
    automation::AutomationTarget,
    clip_fade::{CompiledClipFades, compile_clip_fades},
    export_job::ExportControl,
    mixer_graph::{
        CompiledMixerGraph, MIXER_GRAPH_MAX_NODES, MixerRouteTap, MixerTrackId, compile_mixer_graph,
    },
    model::{AudioAsset, Clip, ClipKind, Pattern, Project},
    tempo_map::TempoMap,
    wav,
};

const PROGRESS_FRAME_INTERVAL: usize = 4096;
const WAV_WRITE_BUFFER_BYTES: usize = 64 * 1024;
// Keep offline rendering aligned with the callback timeline's legacy Piano Roll period
// (`TimelineCompileOptions::default().legacy_piano_period_beats`). Channel Rack steps retain
// their independent Pattern-length cycle below.
const LEGACY_PIANO_REPEAT_BEATS: f32 = 16.0;
static STAGED_FILE_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy)]
enum Waveform {
    Kick,
    Noise,
    Sine,
    Triangle,
}

#[derive(Clone, Copy)]
struct Event {
    sample: usize,
    frequency: f32,
    velocity: f32,
    duration: f32,
    waveform: Waveform,
    left_gain: f32,
    right_gain: f32,
}

#[derive(Clone, Copy)]
struct Voice {
    phase: f32,
    frequency: f32,
    envelope: f32,
    decay: f32,
    waveform: Waveform,
    noise: u32,
    left_gain: f32,
    right_gain: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct StereoFrame {
    left: f32,
    right: f32,
}

#[derive(Clone, Copy, Debug)]
struct MixerRoute {
    gain: f32,
    left_gain: f32,
    right_gain: f32,
}

/// Static no-plug-in transfer from every mixer raw input to MASTER.
///
/// Offline export is linear, so a pair of channel gains is equivalent to
/// rendering 32 buses per frame while avoiding another song-length allocation.
#[derive(Debug)]
struct OfflineMixerPlan {
    graph: CompiledMixerGraph,
    source_routes: [Option<MixerRoute>; MIXER_GRAPH_MAX_NODES],
}

impl OfflineMixerPlan {
    fn build(project: &Project) -> Result<Self> {
        ensure_no_active_plugins(project)?;
        let graph = compile_mixer_graph(project)
            .context("Unable to compile the mixer graph for offline export")?;

        let mut solo_seed = [false; MIXER_GRAPH_MAX_NODES];
        let mut any_solo = false;
        for node in graph.nodes() {
            let solo = project.mixer_tracks[node.project_index].solo;
            solo_seed[usize::from(node.runtime_slot)] = solo;
            any_solo |= solo;
        }

        let mut solo_ancestors = solo_seed;
        let mut solo_descendants = solo_seed;
        if any_solo {
            for dense in graph.topological_order().iter().copied() {
                let source_slot = usize::from(graph.nodes()[usize::from(dense)].runtime_slot);
                if !solo_descendants[source_slot] {
                    continue;
                }
                for route in graph
                    .routes()
                    .iter()
                    .filter(|route| route.source_dense == dense)
                {
                    solo_descendants[usize::from(route.destination_runtime_slot)] = true;
                }
            }
            for dense in graph.topological_order().iter().copied().rev() {
                let source_slot = usize::from(graph.nodes()[usize::from(dense)].runtime_slot);
                if graph
                    .routes()
                    .iter()
                    .filter(|route| route.source_dense == dense)
                    .any(|route| solo_ancestors[usize::from(route.destination_runtime_slot)])
                {
                    solo_ancestors[source_slot] = true;
                }
            }
        }

        let mut node_gate = [false; MIXER_GRAPH_MAX_NODES];
        for node in graph.nodes() {
            let runtime_slot = usize::from(node.runtime_slot);
            let track = &project.mixer_tracks[node.project_index];
            node_gate[runtime_slot] = !track.muted
                && (!any_solo || solo_ancestors[runtime_slot] || solo_descendants[runtime_slot]);
        }

        let master = graph.nodes()[usize::from(graph.master_dense_index())];
        let master_slot = usize::from(master.runtime_slot);
        let master_track = &project.mixer_tracks[master.project_index];
        let (master_pan_left, master_pan_right) = pan_balance(master_track.pan);
        let mut left_transfer = [0.0_f32; MIXER_GRAPH_MAX_NODES];
        let mut right_transfer = [0.0_f32; MIXER_GRAPH_MAX_NODES];
        let mut reaches_master = [false; MIXER_GRAPH_MAX_NODES];
        if node_gate[master_slot] {
            reaches_master[master_slot] = true;
            left_transfer[master_slot] = master_track.volume * master_pan_left;
            right_transfer[master_slot] = master_track.volume * master_pan_right;
        }

        for dense in graph.topological_order().iter().copied().rev() {
            if dense == graph.master_dense_index() {
                continue;
            }
            let node = graph.nodes()[usize::from(dense)];
            let runtime_slot = usize::from(node.runtime_slot);
            if !node_gate[runtime_slot] {
                continue;
            }
            let track = &project.mixer_tracks[node.project_index];
            let (pan_left, pan_right) = pan_balance(track.pan);
            for route in graph
                .routes()
                .iter()
                .filter(|route| route.source_dense == dense)
            {
                let destination_slot = usize::from(route.destination_runtime_slot);
                if !reaches_master[destination_slot] {
                    continue;
                }
                reaches_master[runtime_slot] = true;
                let (tap_left, tap_right) = match route.tap {
                    MixerRouteTap::PreEffects | MixerRouteTap::PostEffects => (1.0, 1.0),
                    MixerRouteTap::PostFader => (track.volume * pan_left, track.volume * pan_right),
                };
                left_transfer[runtime_slot] +=
                    route.gain * tap_left * left_transfer[destination_slot];
                right_transfer[runtime_slot] +=
                    route.gain * tap_right * right_transfer[destination_slot];
            }
        }

        let source_routes = std::array::from_fn(|runtime_slot| {
            reaches_master[runtime_slot].then_some(MixerRoute {
                gain: 1.0,
                left_gain: left_transfer[runtime_slot],
                right_gain: right_transfer[runtime_slot],
            })
        });
        Ok(Self {
            graph,
            source_routes,
        })
    }

    fn route_for_track_id(&self, mixer_track_id: MixerTrackId) -> Option<MixerRoute> {
        let runtime_slot = usize::from(self.graph.runtime_slot_for_id(mixer_track_id)?);
        self.source_routes[runtime_slot]
    }
}

fn ensure_no_active_plugins(project: &Project) -> Result<()> {
    let active_instance = |instance_id: u64| -> Result<bool> {
        let mut instances = project
            .plugin_instances
            .iter()
            .filter(|instance| instance.id == instance_id);
        let instance = instances
            .next()
            .with_context(|| format!("offline mixer references missing plug-in {instance_id}"))?;
        ensure!(
            instances.next().is_none(),
            "offline mixer references duplicate plug-in identity {instance_id}"
        );
        Ok(instance.enabled && !instance.bypass)
    };

    for channel in &project.channels {
        if let Some(instance_id) = channel.instrument_plugin_instance_id
            && active_instance(instance_id)?
        {
            bail!(
                "Offline export cannot render active instrument plug-in {instance_id} on channel {}",
                channel.id
            );
        }
    }
    for slot in &project.mixer_insert_slots {
        if active_instance(slot.plugin_instance_id)? {
            bail!(
                "Offline export cannot render active mixer plug-in {} on track {} slot {}",
                slot.plugin_instance_id,
                slot.track,
                slot.slot
            );
        }
    }
    Ok(())
}

/// Pure preflight shared by the UI and renderer. Legacy lanes without any
/// Playlist placement are globally active, matching live automation/TempoMap.
/// Once a lane has placements, only their unmuted half-open windows enable it.
pub fn ensure_supported_automation(project: &Project) -> Result<()> {
    for automation in &project.automation_lanes {
        let lane = &automation.lane;
        if !lane.is_enabled()
            || lane.points().is_empty()
            || *lane.target() == AutomationTarget::Tempo
        {
            continue;
        }
        let mut placements = project
            .clips
            .iter()
            .filter(|clip| {
                clip.kind == ClipKind::Automation && clip.automation_id == Some(automation.id)
            })
            .peekable();
        let active = if placements.peek().is_none() {
            project.song_length_beats > 0.0
        } else {
            placements.any(|clip| {
                let start = f64::from(clip.start);
                let end = start + f64::from(clip.length);
                let overlap_start = start.max(0.0);
                let overlap_end = end.min(f64::from(project.song_length_beats));
                !clip.muted
                    && start.is_finite()
                    && end.is_finite()
                    && overlap_start < overlap_end
                    && lane
                        .evaluate(f64::from(clip.source_offset) + overlap_start - start)
                        .is_some()
            })
        };
        if active {
            bail!(
                "Offline WAV export cannot render automation '{}' (lane {}, target {}). Only Tempo automation is supported. Disable this lane or use Realtime Master Capture to preserve the live result.",
                automation.name,
                automation.id,
                automation_target_description(lane.target())
            );
        }
    }
    Ok(())
}

fn automation_target_description(target: &AutomationTarget) -> String {
    match target {
        AutomationTarget::MasterVolume => "Master volume".into(),
        AutomationTarget::MasterPan => "Master pan".into(),
        AutomationTarget::Tempo => "Tempo".into(),
        AutomationTarget::Swing => "Swing".into(),
        AutomationTarget::MixerVolume { track } => format!("Mixer {track} volume"),
        AutomationTarget::MixerPan { track } => format!("Mixer {track} pan"),
        AutomationTarget::MixerMute { track } => format!("Mixer {track} mute"),
        AutomationTarget::ChannelVolume { channel } => format!("Channel {channel} volume"),
        AutomationTarget::ChannelPan { channel } => format!("Channel {channel} pan"),
        AutomationTarget::ChannelMute { channel } => format!("Channel {channel} mute"),
        AutomationTarget::PluginParameter {
            instance,
            parameter,
        } => format!("Plug-in {instance} parameter {parameter}"),
    }
}

#[derive(Debug)]
struct PreparedAudioAsset {
    samples: Vec<f32>,
    channels: usize,
    source_sample_rate: u32,
    source_frames: u64,
}

impl PreparedAudioAsset {
    fn frame_count(&self) -> usize {
        self.samples.len() / self.channels
    }
}

#[derive(Clone, Copy, Debug)]
struct PreparedAudioClip {
    asset_id: u64,
    output_start: usize,
    source_start: usize,
    frame_count: usize,
    fades: CompiledClipFades,
    gain: f32,
    route: MixerRoute,
}

// Kept for callers that do not need background progress/cancellation.
#[allow(dead_code)]
pub fn render_project_wav(project: &Project, path: &Path, sample_rate: u32) -> Result<()> {
    render_project_wav_controlled(project, path, sample_rate, &ExportControl::default())
}

pub fn render_project_wav_controlled(
    project: &Project,
    path: &Path,
    sample_rate: u32,
    control: &ExportControl,
) -> Result<()> {
    control.checkpoint(0)?;
    validate_export_sample_rate(sample_rate)?;
    ensure_supported_automation(project)?;
    let mixer_plan = OfflineMixerPlan::build(project)?;
    let tempo_map = TempoMap::from_project(project, sample_rate)
        .context("Unable to build the project tempo map for WAV export")?;
    let frame_count = usize::try_from(tempo_map.duration_frames())
        .context("Project is too long for this platform")?;
    let max_wav_frames = (u32::MAX as usize - 36) / 6;
    ensure!(
        frame_count <= max_wav_frames,
        "Project is too long for a standard PCM WAV file"
    );
    let (audio_assets, audio_clips) = prepare_audio_clips(
        project,
        &mixer_plan,
        sample_rate,
        &tempo_map,
        frame_count,
        control,
    )?;
    control.checkpoint(1000)?;
    let mut events = collect_events_controlled(project, &mixer_plan, &tempo_map, control)?;
    events.sort_by_key(|event| event.sample);

    control.checkpoint(1200)?;
    let mut stereo = allocate_render_buffer(frame_count, sample_rate)?;
    control.checkpoint(1500)?;
    let mut voices = Vec::<Voice>::with_capacity(96);
    let mut next_event = 0;
    for (frame, output) in stereo.iter_mut().enumerate() {
        if frame % PROGRESS_FRAME_INTERVAL == 0 {
            control.work_progress(frame, frame_count, 1500, 5000)?;
        }
        while next_event < events.len() && events[next_event].sample <= frame {
            let event = events[next_event];
            let duration_samples = (event.duration * sample_rate as f32).max(1.0);
            voices.push(Voice {
                phase: 0.0,
                frequency: event.frequency,
                envelope: event.velocity,
                decay: (-7.0 / duration_samples).exp(),
                waveform: event.waveform,
                noise: 0x9e37_79b9_u32.wrapping_add(frame as u32),
                left_gain: event.left_gain,
                right_gain: event.right_gain,
            });
            next_event += 1;
        }

        let mut left = 0.0;
        let mut right = 0.0;
        for voice in &mut voices {
            let oscillator = match voice.waveform {
                Waveform::Kick => {
                    let pitch = voice.frequency * (1.0 + voice.envelope * 2.8);
                    voice.phase = (voice.phase + pitch / sample_rate as f32) % 1.0;
                    (voice.phase * TAU).sin()
                }
                Waveform::Noise => {
                    voice.noise ^= voice.noise << 13;
                    voice.noise ^= voice.noise >> 17;
                    voice.noise ^= voice.noise << 5;
                    voice.noise as f32 / u32::MAX as f32 * 2.0 - 1.0
                }
                Waveform::Sine => {
                    voice.phase = (voice.phase + voice.frequency / sample_rate as f32) % 1.0;
                    (voice.phase * TAU).sin()
                }
                Waveform::Triangle => {
                    voice.phase = (voice.phase + voice.frequency / sample_rate as f32) % 1.0;
                    1.0 - 4.0 * (voice.phase - 0.5).abs()
                }
            };
            let sample = oscillator * voice.envelope;
            left += sample * voice.left_gain;
            right += sample * voice.right_gain;
            voice.envelope *= voice.decay;
        }
        voices.retain(|voice| voice.envelope > 0.0001);
        output.left += left * 0.24;
        output.right += right * 0.24;
    }

    control.checkpoint(5000)?;
    mix_prepared_audio(&audio_assets, &audio_clips, &mut stereo, control)?;
    control.checkpoint(6500)?;
    // Decoded source assets can be much larger than the final render. Release
    // them before the output pass so source media and destination encoding do
    // not contribute to the same peak.
    drop(audio_clips);
    drop(audio_assets);

    let mut peak = 0.0_f32;
    for (index, frame) in stereo.iter().enumerate() {
        if index % PROGRESS_FRAME_INTERVAL == 0 {
            control.work_progress(index, stereo.len(), 6500, 7000)?;
        }
        peak = peak.max(frame.left.abs()).max(frame.right.abs());
    }
    let gain = if peak > 0.95 { 0.95 / peak } else { 1.0 };
    write_stereo_pcm24_controlled(path, sample_rate, &stereo, gain, control)
        .with_context(|| format!("Unable to export WAV to {}", path.display()))
}

#[cfg(test)]
fn collect_events(
    project: &Project,
    mixer_plan: &OfflineMixerPlan,
    tempo_map: &TempoMap,
) -> Result<Vec<Event>> {
    collect_events_controlled(project, mixer_plan, tempo_map, &ExportControl::default())
}

fn collect_events_controlled(
    project: &Project,
    mixer_plan: &OfflineMixerPlan,
    tempo_map: &TempoMap,
    control: &ExportControl,
) -> Result<Vec<Event>> {
    let mut events = Vec::new();
    for clip in &project.clips {
        control.check_cancelled()?;
        if clip.kind != ClipKind::Pattern || clip.muted {
            continue;
        }
        let Some(pattern) = project
            .patterns
            .iter()
            .find(|pattern| pattern.id == clip.pattern_id)
        else {
            continue;
        };
        collect_pattern_events(
            project,
            pattern,
            clip.start,
            clip.length,
            clip.source_offset,
            clip.gain,
            mixer_plan,
            tempo_map,
            &mut events,
            control,
        )?;
    }
    Ok(events)
}

#[allow(clippy::too_many_arguments)]
fn collect_pattern_events(
    project: &Project,
    pattern: &Pattern,
    clip_start: f32,
    clip_length: f32,
    source_offset: f32,
    clip_gain: f32,
    mixer_plan: &OfflineMixerPlan,
    tempo_map: &TempoMap,
    events: &mut Vec<Event>,
    control: &ExportControl,
) -> Result<()> {
    if !clip_start.is_finite()
        || !clip_length.is_finite()
        || !source_offset.is_finite()
        || source_offset < 0.0
        || clip_length <= 0.0
        || clip_start >= project.song_length_beats
    {
        return Ok(());
    }
    let clip_end = (clip_start + clip_length).min(project.song_length_beats);
    let clip_start = clip_start.max(0.0);
    if clip_end <= clip_start {
        return Ok(());
    }
    for (channel_index, channel) in project.channels.iter().enumerate() {
        if channel.muted {
            continue;
        }
        let Some(route) = mixer_plan.route_for_track_id(channel.mixer_track) else {
            continue;
        };
        let Some(steps) = pattern.channel_steps.get(channel_index) else {
            continue;
        };
        let waveform = match channel_index {
            0 => Waveform::Kick,
            1 | 2 => Waveform::Noise,
            3 => Waveform::Sine,
            _ => Waveform::Triangle,
        };
        let midi = [36_u8, 39, 54, 43, 64]
            .get(channel_index)
            .copied()
            .unwrap_or(60);
        let step_count = pattern.length_steps.clamp(1, steps.len());
        let cycle_beats = step_count as f32 * 0.25;
        let mut cycle = -source_offset.rem_euclid(cycle_beats);
        while clip_start + cycle < clip_end {
            control.check_cancelled()?;
            for (step, enabled) in steps.iter().copied().take(step_count).enumerate() {
                if !enabled {
                    continue;
                }
                let beat = clip_start + cycle + step as f32 * 0.25;
                if beat < clip_start || beat >= clip_end {
                    continue;
                }
                let sample = tempo_frame(tempo_map, beat)?;
                let clip_stop = tempo_frame(tempo_map, clip_end)?;
                events.push(Event {
                    sample,
                    frequency: midi_frequency(midi),
                    velocity: channel.volume * route.gain * clip_gain,
                    duration: (match waveform {
                        Waveform::Kick => 0.42_f32,
                        Waveform::Noise => 0.13,
                        Waveform::Sine => 0.62,
                        Waveform::Triangle => 0.85,
                    })
                    .min(clip_stop.saturating_sub(sample) as f32 / tempo_map.sample_rate() as f32),
                    waveform,
                    left_gain: route.left_gain,
                    right_gain: route.right_gain,
                });
            }
            cycle += cycle_beats;
        }
    }

    let mut cycle = -source_offset.rem_euclid(LEGACY_PIANO_REPEAT_BEATS);
    while clip_start + cycle < clip_end {
        control.check_cancelled()?;
        for note in &pattern.notes {
            if note.muted {
                continue;
            }
            let channel_id = note.channel_id.ok_or_else(|| {
                anyhow!(
                    "Piano-roll note {} (id {}) in pattern {} has no project channel assignment",
                    note.note,
                    note.id,
                    pattern.id
                )
            })?;
            let mut channels = project
                .channels
                .iter()
                .filter(|channel| channel.id == channel_id);
            let channel = channels.next().ok_or_else(|| {
                anyhow!(
                    "Piano-roll note id {} references missing project channel {}",
                    note.id,
                    channel_id
                )
            })?;
            ensure!(
                channels.next().is_none(),
                "Piano-roll note id {} references duplicate project channel {}",
                note.id,
                channel_id
            );
            if channel.muted {
                continue;
            }
            let Some(route) = mixer_plan.route_for_track_id(channel.mixer_track) else {
                continue;
            };
            let beat = clip_start + cycle + note.start;
            if beat < clip_start || beat >= clip_end {
                continue;
            }
            let sample = tempo_frame(tempo_map, beat)?;
            let note_end = (beat + note.length).min(clip_end);
            let end_sample = tempo_frame(tempo_map, note_end)?;
            events.push(Event {
                sample,
                frequency: midi_frequency(note.note),
                velocity: note.velocity * channel.volume * route.gain * clip_gain * 0.55,
                duration: end_sample.saturating_sub(sample) as f32 / tempo_map.sample_rate() as f32,
                waveform: Waveform::Triangle,
                left_gain: route.left_gain,
                right_gain: route.right_gain,
            });
        }
        cycle += LEGACY_PIANO_REPEAT_BEATS;
    }
    Ok(())
}

fn pan_balance(pan: f32) -> (f32, f32) {
    let pan = pan.clamp(-1.0, 1.0);
    (1.0 - pan.max(0.0), 1.0 + pan.min(0.0))
}

fn prepare_audio_clips(
    project: &Project,
    mixer_plan: &OfflineMixerPlan,
    output_sample_rate: u32,
    tempo_map: &TempoMap,
    output_frames: usize,
    control: &ExportControl,
) -> Result<(HashMap<u64, PreparedAudioAsset>, Vec<PreparedAudioClip>)> {
    let mut catalog = HashMap::with_capacity(project.audio_assets.len());
    for asset in &project.audio_assets {
        if let Some(previous) = catalog.insert(asset.id, asset) {
            bail!(
                "Duplicate audio asset id {} is used by '{}' and '{}'",
                asset.id,
                previous.name,
                asset.name
            );
        }
    }

    let mut assets = HashMap::new();
    let mut clips = Vec::new();
    for (index, clip) in project.clips.iter().enumerate() {
        control.work_progress(index, project.clips.len(), 0, 1000)?;
        if clip.kind != ClipKind::Audio || clip.muted {
            continue;
        }
        // Playlist lane 0 feeds mixer insert 1. Resolve gating before touching
        // the asset catalog or filesystem: muted inserts and non-solo routes
        // must be cheap even when their media is offline or corrupt.
        let Some(mixer_track_id) = project.audio_clip_mixer_track_id(clip.id) else {
            continue;
        };
        let Some(route) = mixer_plan.route_for_track_id(mixer_track_id) else {
            continue;
        };
        let asset_id = clip.audio_asset_id.ok_or_else(|| {
            anyhow!(
                "Audio clip '{}' (clip id {}) has no audio asset reference",
                clip.name,
                clip.id
            )
        })?;
        let asset = catalog.get(&asset_id).copied().ok_or_else(|| {
            anyhow!(
                "Audio clip '{}' (clip id {}) references missing audio asset {}",
                clip.name,
                clip.id,
                asset_id
            )
        })?;

        match assets.entry(asset_id) {
            Entry::Occupied(_) => {}
            Entry::Vacant(entry) => {
                let prepared = load_audio_asset(asset, output_sample_rate).with_context(|| {
                    format!(
                        "Unable to prepare audio clip '{}' (clip id {}, asset {}) from '{}'",
                        clip.name,
                        clip.id,
                        asset_id,
                        asset.path.display()
                    )
                })?;
                entry.insert(prepared);
            }
        }

        let prepared_asset = assets
            .get(&asset_id)
            .expect("audio asset was inserted immediately above");
        if let Some(prepared) = prepare_audio_clip(
            clip,
            asset_id,
            prepared_asset,
            route,
            tempo_map,
            output_sample_rate,
            output_frames,
        )
        .with_context(|| {
            format!(
                "Invalid timing or gain for audio clip '{}' (clip id {}, asset {})",
                clip.name, clip.id, asset_id
            )
        })? {
            clips.push(prepared);
        }
    }
    Ok((assets, clips))
}

fn load_audio_asset(asset: &AudioAsset, output_sample_rate: u32) -> Result<PreparedAudioAsset> {
    let decoded = wav::read_wav(&asset.path)?;
    ensure!(
        decoded.metadata.sample_rate == asset.sample_rate,
        "audio asset {} changed sample rate from {} Hz to {} Hz; its native-frame clip offsets are no longer valid",
        asset.id,
        asset.sample_rate,
        decoded.metadata.sample_rate
    );
    let source_sample_rate = decoded.metadata.sample_rate;
    let source_frames = decoded.metadata.frames;
    let decoded = if decoded.metadata.sample_rate == output_sample_rate {
        decoded
    } else {
        decoded
            .resample_linear(output_sample_rate)
            .with_context(|| {
                format!(
                    "Unable to resample '{}' from {} Hz to {} Hz",
                    asset.path.display(),
                    decoded.metadata.sample_rate,
                    output_sample_rate
                )
            })?
    };

    Ok(PreparedAudioAsset {
        samples: decoded.samples,
        channels: usize::from(decoded.metadata.channels),
        source_sample_rate,
        source_frames,
    })
}

#[allow(clippy::too_many_arguments)]
fn prepare_audio_clip(
    clip: &Clip,
    asset_id: u64,
    asset: &PreparedAudioAsset,
    route: MixerRoute,
    tempo_map: &TempoMap,
    sample_rate: u32,
    output_frames: usize,
) -> Result<Option<PreparedAudioClip>> {
    ensure!(
        clip.start.is_finite() && clip.start >= 0.0,
        "clip start must be a non-negative finite beat position"
    );
    ensure!(
        clip.length.is_finite() && clip.length > 0.0,
        "clip length must be a positive finite beat duration"
    );
    ensure!(clip.gain.is_finite(), "clip gain must be a finite value");
    ensure!(
        clip.fade_in.is_finite() && clip.fade_out.is_finite(),
        "clip fades must be finite values"
    );

    if f64::from(clip.start) >= tempo_map.max_beat() {
        return Ok(None);
    }
    let clip_end_beats = audio_end_beat(clip, true)
        .ok_or_else(|| anyhow!("clip has invalid exact end metadata"))?
        .min(tempo_map.max_beat());
    ensure!(
        clip_end_beats.is_finite(),
        "clip end overflows the timeline"
    );

    let output_start = tempo_frame(tempo_map, clip.start)?.min(output_frames);
    let output_end = usize::try_from(tempo_map.beat_to_frame(clip_end_beats)?)?.min(output_frames);
    ensure!(
        audio_metadata_valid(clip),
        "clip contains invalid inherited audio timing"
    );
    let elapsed_frames =
        source_elapsed_frames(clip.audio_source_reference.as_ref(), sample_rate)
            .ok_or_else(|| anyhow!("clip source timing exceeds the render frame range"))?;
    let native_source_start = clip.audio_source_offset_frame.ok_or_else(|| {
        anyhow!(
            "Audio clip '{}' (clip id {}) has no resolved native-frame source offset",
            clip.name,
            clip.id
        )
    })?;
    let native_position = native_source_start as f64
        + elapsed_frames as f64 * f64::from(asset.source_sample_rate) / f64::from(sample_rate);
    // Root offsets must still identify real media. An inherited elapsed clock
    // can run beyond it; exhaustion is decided below on the RESAMPLED grid,
    // whose endpoint-preserving interpolation may retain one last output frame.
    ensure!(
        native_source_start < asset.source_frames
            && native_position.is_finite()
            && native_position >= 0.0,
        "native source position is outside the audio asset"
    );
    let scaled_source_start = u128::from(native_source_start)
        .saturating_mul(u128::from(sample_rate))
        .saturating_add(u128::from(asset.source_sample_rate / 2))
        / u128::from(asset.source_sample_rate);
    let source_start =
        usize::try_from(i128::try_from(scaled_source_start)? + i128::from(elapsed_frames))
            .context("native audio source offset exceeds the render frame range")?;
    let timeline_frames = output_end.saturating_sub(output_start);
    let available_source_frames = asset.frame_count().saturating_sub(source_start);
    let frame_count = timeline_frames.min(available_source_frames);
    if frame_count == 0 {
        return Ok(None);
    }

    let fades = compile_clip_fades(
        clip.start,
        clip.length,
        clip.audio_length_reference,
        clip.fade_in,
        clip.fade_out,
        clip.fade_in_reference,
        clip.fade_out_reference,
        tempo_map,
        true,
    )
    .ok_or_else(|| anyhow!("clip has invalid fade reference domains"))?;
    Ok(Some(PreparedAudioClip {
        asset_id,
        output_start,
        source_start,
        frame_count,
        fades,
        gain: clip.gain,
        route,
    }))
}

fn tempo_frame(tempo_map: &TempoMap, beat: f32) -> Result<usize> {
    ensure!(
        beat.is_finite() && beat >= 0.0,
        "beat position must be a non-negative finite value"
    );
    let frame = tempo_map
        .beat_to_frame(f64::from(beat))
        .with_context(|| format!("beat {beat} is outside the project tempo map"))?;
    usize::try_from(frame).context("tempo-map frame exceeds the render frame range")
}

fn mix_prepared_audio(
    assets: &HashMap<u64, PreparedAudioAsset>,
    clips: &[PreparedAudioClip],
    output: &mut [StereoFrame],
    control: &ExportControl,
) -> Result<()> {
    let total_frames = clips
        .iter()
        .fold(0_usize, |sum, clip| sum.saturating_add(clip.frame_count));
    let mut completed_frames = 0_usize;
    for clip in clips {
        control.check_cancelled()?;
        let Some(asset) = assets.get(&clip.asset_id) else {
            debug_assert!(false, "prepared clip refers to an unavailable asset");
            continue;
        };
        let destination = &mut output[clip.output_start..clip.output_start + clip.frame_count];
        for (index, destination) in destination.iter_mut().enumerate() {
            if index % PROGRESS_FRAME_INTERVAL == 0 {
                control.work_progress(
                    completed_frames.saturating_add(index),
                    total_frames,
                    5000,
                    6500,
                )?;
            }
            let envelope = clip.fades.gain_at((clip.output_start + index) as u64);
            let gain = clip.gain * clip.route.gain * envelope;
            let source_frame = clip.source_start + index;
            let source_offset = source_frame * asset.channels;
            let left = asset.samples[source_offset];
            let right = if asset.channels == 1 {
                left
            } else {
                asset.samples[source_offset + 1]
            };
            destination.left += left * gain * clip.route.left_gain;
            destination.right += right * gain * clip.route.right_gain;
        }
        completed_frames = completed_frames.saturating_add(clip.frame_count);
    }
    Ok(())
}

fn midi_frequency(note: u8) -> f32 {
    440.0 * 2.0_f32.powf((note as f32 - 69.0) / 12.0)
}

fn validate_export_sample_rate(sample_rate: u32) -> Result<()> {
    ensure!(
        (8_000..=192_000).contains(&sample_rate),
        "WAV export sample rate {sample_rate} Hz is outside the supported range 8000..=192000 Hz"
    );
    Ok(())
}

#[cfg(test)]
fn write_stereo_pcm24(
    path: &Path,
    sample_rate: u32,
    stereo: &[StereoFrame],
    gain: f32,
) -> Result<()> {
    write_stereo_pcm24_controlled(path, sample_rate, stereo, gain, &ExportControl::default())
}

fn write_stereo_pcm24_controlled(
    path: &Path,
    sample_rate: u32,
    stereo: &[StereoFrame],
    gain: f32,
    control: &ExportControl,
) -> Result<()> {
    control.checkpoint(7000)?;
    validate_export_sample_rate(sample_rate)?;
    ensure!(gain.is_finite(), "WAV export gain must be finite");
    for (index, frame) in stereo.iter().enumerate() {
        if index % PROGRESS_FRAME_INTERVAL == 0 {
            control.work_progress(index, stereo.len(), 7000, 7500)?;
        }
        ensure!(
            frame.left.is_finite()
                && frame.right.is_finite()
                && (frame.left * gain).is_finite()
                && (frame.right * gain).is_finite(),
            "WAV export contains non-finite audio at frame {index}"
        );
    }
    let channels = 2_u16;
    let bits_per_sample = 24_u16;
    let bytes_per_sample = 3_u32;
    let data_size = stereo
        .len()
        .checked_mul(channels as usize * bytes_per_sample as usize)
        .and_then(|size| u32::try_from(size).ok())
        .context("Rendered audio is too large for a standard PCM WAV file")?;
    ensure!(
        data_size <= u32::MAX - 36,
        "Rendered audio is too large for a standard PCM WAV file"
    );

    control.checkpoint(7500)?;
    let (mut staged, file) = create_staged_output(path)?;
    let mut writer = BufWriter::with_capacity(WAV_WRITE_BUFFER_BYTES, file);
    writer.write_all(b"RIFF")?;
    writer.write_all(&(36 + data_size).to_le_bytes())?;
    writer.write_all(b"WAVEfmt ")?;
    writer.write_all(&16_u32.to_le_bytes())?;
    writer.write_all(&1_u16.to_le_bytes())?;
    writer.write_all(&channels.to_le_bytes())?;
    writer.write_all(&sample_rate.to_le_bytes())?;
    let byte_rate = sample_rate * channels as u32 * bytes_per_sample;
    writer.write_all(&byte_rate.to_le_bytes())?;
    let block_align = channels * bytes_per_sample as u16;
    writer.write_all(&block_align.to_le_bytes())?;
    writer.write_all(&bits_per_sample.to_le_bytes())?;
    writer.write_all(b"data")?;
    writer.write_all(&data_size.to_le_bytes())?;
    for (index, frame) in stereo.iter().enumerate() {
        if index % PROGRESS_FRAME_INTERVAL == 0 {
            control.work_progress(index, stereo.len(), 7500, 9800)?;
        }
        let mut encoded_frame = [0_u8; 6];
        for (channel, sample) in [frame.left, frame.right].into_iter().enumerate() {
            let value = (sample * gain).clamp(-1.0, 1.0);
            let integer = (value * 8_388_607.0).round() as i32;
            let encoded = integer.to_le_bytes();
            let offset = channel * 3;
            encoded_frame[offset..offset + 3].copy_from_slice(&encoded[..3]);
        }
        writer.write_all(&encoded_frame)?;
    }
    control.checkpoint(9800)?;
    writer
        .flush()
        .with_context(|| format!("Unable to flush staged WAV for {}", path.display()))?;
    control.check_cancelled()?;
    writer
        .get_ref()
        .sync_all()
        .with_context(|| format!("Unable to sync staged WAV for {}", path.display()))?;
    drop(writer);
    // The cancellation/commit CAS makes the final boundary unambiguous even
    // if the UI requests Cancel concurrently with the worker's rename.
    control.begin_commit()?;
    staged.commit(path)?;
    control.complete();
    Ok(())
}

struct StagedOutput {
    path: PathBuf,
    committed: bool,
}

impl StagedOutput {
    fn commit(&mut self, destination: &Path) -> Result<()> {
        replace_by_rename(&self.path, destination).with_context(|| {
            format!(
                "Unable to commit staged WAV '{}' to '{}'",
                self.path.display(),
                destination.display()
            )
        })?;
        self.committed = true;
        Ok(())
    }
}

fn allocate_render_buffer(frame_count: usize, sample_rate: u32) -> Result<Vec<StereoFrame>> {
    let duration_seconds = frame_count as f64 / f64::from(sample_rate);
    let frame_bytes = std::mem::size_of::<StereoFrame>();
    let render_bytes = frame_count.checked_mul(frame_bytes).ok_or_else(|| {
        anyhow!(
            "Unable to allocate the {:.1}-second stereo render buffer: {} frames overflow this platform's address space",
            duration_seconds,
            frame_count
        )
    })?;
    let render_mib = render_bytes as f64 / (1024.0 * 1024.0);

    let mut stereo = Vec::new();
    stereo.try_reserve_exact(frame_count).map_err(|error| {
        anyhow!(
            "Unable to allocate {render_mib:.1} MiB for a {duration_seconds:.1}-second stereo render ({frame_count} frames at {sample_rate} Hz): {error}"
        )
    })?;
    stereo.resize(frame_count, StereoFrame::default());
    Ok(stereo)
}

impl Drop for StagedOutput {
    fn drop(&mut self) {
        if !self.committed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn create_staged_output(destination: &Path) -> Result<(StagedOutput, File)> {
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_name = destination
        .file_name()
        .ok_or_else(|| {
            anyhow!(
                "WAV output path has no file name: {}",
                destination.display()
            )
        })?
        .to_string_lossy();

    for _ in 0..32 {
        let sequence = STAGED_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temporary = parent.join(format!(
            ".{file_name}.{}.{}.tmp",
            std::process::id(),
            sequence
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => {
                return Ok((
                    StagedOutput {
                        path: temporary,
                        committed: false,
                    },
                    file,
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "Unable to create a staged WAV beside {}",
                        destination.display()
                    )
                });
            }
        }
    }
    bail!(
        "Unable to allocate a unique staged WAV name beside {}",
        destination.display()
    )
}

#[cfg(windows)]
fn replace_by_rename(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x0000_0001;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x0000_0008;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(
            existing_file_name: *const u16,
            new_file_name: *const u16,
            flags: u32,
        ) -> i32;
    }

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // MoveFileExW is the Windows rename primitive. REPLACE_EXISTING leaves the
    // old destination intact if the operation fails; WRITE_THROUGH waits for
    // the same-volume metadata update after the staged file itself was synced.
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_by_rename(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::rename(source, destination)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        automation::{AutomationCurve, AutomationLane, AutomationPoint, AutomationTarget},
        mixer_graph::{
            MASTER_MIXER_TRACK_ID, MixerRoute as ProjectMixerRoute, MixerRouteDestination,
        },
        model::{
            AudioAsset, AudioClipMixerDestination, Clip, MixerInsertSlotRef, PianoNote,
            PluginFormat, PluginInstance, PluginRole, PluginRuntimeStatus, ProjectAutomation,
        },
    };
    use std::collections::BTreeMap;

    fn arrangement_project() -> Project {
        let mut project = Project {
            tempo: 120.0,
            song_length_beats: 8.0,
            ..Project::default()
        };
        project.channels.truncate(1);
        project.channels[0].volume = 1.0;
        project.channels[0].mixer_track = 1;
        project.channels[0].muted = false;

        project.mixer_tracks.truncate(3);
        project.mixer_routes.truncate(2);
        for track in &mut project.mixer_tracks {
            track.volume = 1.0;
            track.muted = false;
            track.solo = false;
        }

        let mut steps = [false; 16];
        steps[0] = true;
        project.patterns = vec![Pattern {
            id: 42,
            name: "Two beat pattern".into(),
            length_steps: 8,
            channel_steps: vec![steps],
            notes: Vec::new(),
        }];
        project.active_pattern = 0;
        project.piano_notes.clear();
        project.automation_lanes.clear();
        project.clips = vec![pattern_clip(1.0, 4.1, 1.0, false)];
        project
    }

    fn pattern_clip(start: f32, length: f32, gain: f32, muted: bool) -> Clip {
        Clip {
            id: 1,
            track: 0,
            start,
            length,
            name: "Pattern 42".into(),
            color: [255, 142, 82],
            kind: ClipKind::Pattern,
            group_id: None,
            pattern_id: 42,
            automation_id: None,
            audio_asset_id: None,
            source_offset: 0.0,
            audio_source_offset_frame: None,
            audio_source_reference: None,
            audio_length_reference: None,
            fade_in_reference: None,
            fade_out_reference: None,
            gain,
            fade_in: 0.0,
            fade_out: 0.0,
            muted,
        }
    }

    fn temporary_wav(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("citrus-{name}-{}.wav", std::process::id()))
    }

    fn write_pcm16_wav(path: &Path, sample_rate: u32, channels: u16, samples: &[i16]) {
        assert!(channels > 0);
        assert!(samples.len().is_multiple_of(usize::from(channels)));
        let data_size = u32::try_from(samples.len() * 2).unwrap();
        let block_align = channels * 2;
        let mut bytes = Vec::with_capacity(44 + data_size as usize);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_size).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&sample_rate.to_le_bytes());
        bytes.extend_from_slice(&(sample_rate * u32::from(block_align)).to_le_bytes());
        bytes.extend_from_slice(&block_align.to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_size.to_le_bytes());
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        std::fs::write(path, bytes).unwrap();
    }

    fn audio_clip(start: f32, length: f32, source_offset_frame: u64) -> Clip {
        Clip {
            id: 70,
            track: 0,
            start,
            length,
            name: "Test Audio Clip".into(),
            color: [255, 207, 99],
            kind: ClipKind::Audio,
            group_id: None,
            pattern_id: 1,
            automation_id: None,
            audio_asset_id: Some(700),
            source_offset: 0.0,
            audio_source_offset_frame: Some(source_offset_frame),
            audio_source_reference: None,
            audio_length_reference: None,
            fade_in_reference: None,
            fade_out_reference: None,
            gain: 1.0,
            fade_in: 0.0,
            fade_out: 0.0,
            muted: false,
        }
    }

    fn audio_project(
        source: &Path,
        source_rate: u32,
        channels: u16,
        source_frames: u64,
    ) -> Project {
        let mut project = Project {
            tempo: 60.0,
            song_length_beats: 0.01,
            ..Project::default()
        };
        for track in &mut project.mixer_tracks {
            track.volume = 1.0;
            track.pan = 0.0;
            track.muted = false;
            track.solo = false;
        }
        project.automation_lanes.clear();
        project.clips = vec![audio_clip(0.0, 0.001, 0)];
        project.audio_clip_mixer_destinations = vec![AudioClipMixerDestination {
            clip_id: 70,
            mixer_track_id: project.mixer_track_id_at_runtime_slot(1).unwrap(),
        }];
        project.audio_assets = vec![AudioAsset {
            id: 700,
            name: "Synthetic source".into(),
            path: source.to_path_buf(),
            sample_rate: source_rate,
            channels,
            bits_per_sample: 16,
            frames: source_frames,
            waveform_peaks: Vec::new(),
        }];
        project
    }

    fn collected_events(project: &Project, sample_rate: u32) -> Vec<Event> {
        let tempo_map = TempoMap::from_project(project, sample_rate).unwrap();
        let mixer_plan = OfflineMixerPlan::build(project).unwrap();
        collect_events(project, &mixer_plan, &tempo_map).unwrap()
    }

    fn render_and_decode(project: &Project, output: &Path) -> Vec<StereoFrame> {
        render_project_wav(project, output, 8_000).unwrap();
        let rendered = wav::read_wav(output).unwrap();
        assert_eq!(rendered.metadata.channels, 2);
        rendered
            .samples
            .as_chunks::<2>()
            .0
            .iter()
            .map(|frame| StereoFrame {
                left: frame[0],
                right: frame[1],
            })
            .collect()
    }

    fn pcm16(value: i16) -> f32 {
        f32::from(value) / 32_768.0
    }

    fn mixer_test_project() -> Project {
        let mut project = Project::blank();
        project.mixer_routes.clear();
        for track in &mut project.mixer_tracks {
            track.volume = 1.0;
            track.pan = 0.0;
            track.muted = false;
            track.solo = false;
        }
        project
    }

    fn main_route(
        id: u64,
        runtime_slot: u8,
        source_id: MixerTrackId,
        destination_id: MixerTrackId,
        tap: MixerRouteTap,
        gain: f32,
    ) -> ProjectMixerRoute {
        ProjectMixerRoute {
            id,
            runtime_slot,
            source_mixer_track_id: source_id,
            destination: MixerRouteDestination::MainInput {
                mixer_track_id: destination_id,
            },
            tap,
            gain,
            enabled: true,
        }
    }

    fn source_transfer(project: &Project, source_id: MixerTrackId) -> (f32, f32) {
        let route = OfflineMixerPlan::build(project)
            .unwrap()
            .route_for_track_id(source_id)
            .unwrap();
        (route.gain * route.left_gain, route.gain * route.right_gain)
    }

    fn assert_stereo_close(actual: (f32, f32), expected: (f32, f32)) {
        assert!(
            (actual.0 - expected.0).abs() < 1.0e-6,
            "left: actual {}, expected {}",
            actual.0,
            expected.0
        );
        assert!(
            (actual.1 - expected.1).abs() < 1.0e-6,
            "right: actual {}, expected {}",
            actual.1,
            expected.1
        );
    }

    fn test_plugin(id: u64) -> PluginInstance {
        PluginInstance {
            id,
            format: PluginFormat::Vst3,
            role: PluginRole::Unknown,
            path: PathBuf::from(format!(r"C:\VST3\Export-{id}.vst3")),
            uid: format!("export-{id}"),
            vendor: "Citrus".into(),
            name: format!("Export plug-in {id}"),
            enabled: true,
            bypass: false,
            wet: 1.0,
            parameters: BTreeMap::new(),
            opaque_state: Vec::new(),
            runtime_status: PluginRuntimeStatus::Unloaded,
        }
    }

    #[test]
    fn mixer_dag_migration_star_matches_the_legacy_transfer() {
        let mut project = mixer_test_project();
        project.mixer_routes.push(main_route(
            1,
            0,
            1,
            MASTER_MIXER_TRACK_ID,
            MixerRouteTap::PostFader,
            1.0,
        ));
        project.mixer_tracks[1].volume = 0.4;
        project.mixer_tracks[1].pan = -0.25;
        project.mixer_tracks[0].volume = 0.5;
        project.mixer_tracks[0].pan = 0.5;

        // Legacy star math was insert volume/pan followed by MASTER
        // volume/pan. The compiled graph must be bit-for-bit equivalent.
        assert_stereo_close(source_transfer(&project, 1), (0.1, 0.15));
    }

    #[test]
    fn mixer_dag_chain_fanout_and_diamond_sum_every_path() {
        let mut chain = mixer_test_project();
        chain.mixer_routes.extend([
            main_route(1, 0, 1, 2, MixerRouteTap::PostFader, 0.5),
            main_route(
                2,
                1,
                2,
                MASTER_MIXER_TRACK_ID,
                MixerRouteTap::PostFader,
                0.25,
            ),
        ]);
        chain.mixer_tracks[1].volume = 0.8;
        chain.mixer_tracks[2].volume = 0.4;
        chain.mixer_tracks[0].volume = 0.5;
        assert_stereo_close(source_transfer(&chain, 1), (0.02, 0.02));

        let mut fanout = mixer_test_project();
        fanout.mixer_routes.extend([
            main_route(10, 0, 1, 2, MixerRouteTap::PostFader, 0.5),
            main_route(11, 1, 1, 3, MixerRouteTap::PostFader, 0.25),
            main_route(
                12,
                2,
                2,
                MASTER_MIXER_TRACK_ID,
                MixerRouteTap::PostFader,
                0.2,
            ),
            main_route(
                13,
                3,
                3,
                MASTER_MIXER_TRACK_ID,
                MixerRouteTap::PostFader,
                0.4,
            ),
        ]);
        assert_stereo_close(source_transfer(&fanout, 1), (0.2, 0.2));

        let mut diamond = mixer_test_project();
        diamond.mixer_routes.extend([
            main_route(20, 0, 1, 2, MixerRouteTap::PostFader, 0.5),
            main_route(21, 1, 1, 3, MixerRouteTap::PostFader, 0.25),
            main_route(22, 2, 2, 4, MixerRouteTap::PostFader, 0.4),
            main_route(23, 3, 3, 4, MixerRouteTap::PostFader, 0.8),
            main_route(
                24,
                4,
                4,
                MASTER_MIXER_TRACK_ID,
                MixerRouteTap::PostFader,
                0.5,
            ),
        ]);
        assert_stereo_close(source_transfer(&diamond, 1), (0.2, 0.2));
    }

    #[test]
    fn mixer_dag_taps_apply_fader_only_to_post_fader() {
        for (tap, expected) in [
            (MixerRouteTap::PreEffects, (1.0, 1.0)),
            (MixerRouteTap::PostEffects, (1.0, 1.0)),
            (MixerRouteTap::PostFader, (0.0, 0.25)),
        ] {
            let mut project = mixer_test_project();
            project
                .mixer_routes
                .push(main_route(1, 0, 1, MASTER_MIXER_TRACK_ID, tap, 1.0));
            project.mixer_tracks[1].volume = 0.25;
            project.mixer_tracks[1].pan = 1.0;
            assert_stereo_close(source_transfer(&project, 1), expected);

            // Mute gates before every tap, including pre-effects sends.
            project.mixer_tracks[1].muted = true;
            assert!(
                OfflineMixerPlan::build(&project)
                    .unwrap()
                    .route_for_track_id(1)
                    .is_none()
            );
        }
    }

    #[test]
    fn mixer_dag_solo_uses_exact_ancestor_and_descendant_closure() {
        let mut project = mixer_test_project();
        project.mixer_routes.extend([
            main_route(1, 0, 1, 2, MixerRouteTap::PostFader, 1.0),
            main_route(2, 1, 3, 2, MixerRouteTap::PostFader, 1.0),
            main_route(
                3,
                2,
                2,
                MASTER_MIXER_TRACK_ID,
                MixerRouteTap::PostFader,
                1.0,
            ),
            main_route(
                4,
                3,
                4,
                MASTER_MIXER_TRACK_ID,
                MixerRouteTap::PostFader,
                1.0,
            ),
        ]);

        project.mixer_tracks[1].solo = true;
        let plan = OfflineMixerPlan::build(&project).unwrap();
        assert!(plan.route_for_track_id(1).is_some());
        assert!(plan.route_for_track_id(2).is_some());
        assert!(plan.route_for_track_id(3).is_none());
        assert!(plan.route_for_track_id(4).is_none());

        project.mixer_tracks[1].solo = false;
        project.mixer_tracks[2].solo = true;
        let plan = OfflineMixerPlan::build(&project).unwrap();
        assert!(plan.route_for_track_id(1).is_some());
        assert!(plan.route_for_track_id(2).is_some());
        assert!(plan.route_for_track_id(3).is_some());
        assert!(plan.route_for_track_id(4).is_none());

        project.mixer_tracks[1].muted = true;
        let plan = OfflineMixerPlan::build(&project).unwrap();
        assert!(plan.route_for_track_id(1).is_none());
        assert!(plan.route_for_track_id(3).is_some());

        project.mixer_tracks[1].muted = false;
        project.mixer_tracks[2].muted = true;
        let plan = OfflineMixerPlan::build(&project).unwrap();
        assert!(plan.route_for_track_id(1).is_none());
        assert!(plan.route_for_track_id(2).is_none());
        assert!(plan.route_for_track_id(3).is_none());
    }

    #[test]
    fn mixer_dag_uses_stable_ids_when_display_order_changes() {
        let mut project = mixer_test_project();
        project.mixer_routes.extend([
            main_route(1, 0, 1, 7, MixerRouteTap::PostFader, 1.0),
            main_route(
                2,
                1,
                7,
                MASTER_MIXER_TRACK_ID,
                MixerRouteTap::PostFader,
                1.0,
            ),
        ]);
        project.mixer_tracks[1].volume = 0.3;
        project.mixer_tracks[7].volume = 0.4;
        project.mixer_tracks[0].volume = 0.5;
        let before = source_transfer(&project, 1);

        project.mixer_tracks.swap(1, 7);
        let after = source_transfer(&project, 1);
        assert_stereo_close(before, (0.06, 0.06));
        assert_stereo_close(after, before);
    }

    #[test]
    fn offline_export_rejects_active_plugins_and_sidechains() {
        let mut project = mixer_test_project();
        project.plugin_instances.push(test_plugin(700));
        project.channels[0].instrument_plugin_instance_id = Some(700);
        let error = OfflineMixerPlan::build(&project).unwrap_err();
        assert!(format!("{error:#}").contains("active instrument plug-in 700"));

        project.channels[0].instrument_plugin_instance_id = None;
        project.mixer_insert_slots.push(MixerInsertSlotRef {
            track: 1,
            slot: 0,
            plugin_instance_id: 700,
        });
        let error = OfflineMixerPlan::build(&project).unwrap_err();
        assert!(format!("{error:#}").contains("active mixer plug-in 700"));

        project.plugin_instances[0].bypass = true;
        project.mixer_insert_slots.clear();
        project.mixer_routes.push(ProjectMixerRoute {
            id: 1,
            runtime_slot: 0,
            source_mixer_track_id: 1,
            destination: MixerRouteDestination::PluginSidechain {
                mixer_track_id: MASTER_MIXER_TRACK_ID,
                slot: 0,
                input_bus: 1,
            },
            tap: MixerRouteTap::PostFader,
            gain: 1.0,
            enabled: true,
        });
        let error = OfflineMixerPlan::build(&project).unwrap_err();
        assert!(format!("{error:#}").contains("sidechain"));
    }

    fn unsupported_lane(target: AutomationTarget) -> ProjectAutomation {
        let mut lane = AutomationLane::new(target);
        // Endpoint holding makes this active even before its first point.
        lane.replace_points([AutomationPoint::new(128.0, 0.5)]);
        ProjectAutomation {
            id: 900,
            name: "Audible movement".into(),
            lane,
        }
    }

    fn automation_placement(start: f32, length: f32, muted: bool) -> Clip {
        let mut clip = pattern_clip(start, length, 1.0, muted);
        clip.kind = ClipKind::Automation;
        clip.automation_id = Some(900);
        clip
    }

    fn workflow_test_directory(name: &str) -> PathBuf {
        let sequence = STAGED_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "citrus-export-{name}-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).unwrap();
        directory
    }

    #[test]
    fn all_non_tempo_targets_fail_preflight_without_replacing_destination() {
        let directory = workflow_test_directory("automation");
        let path = directory.join("existing.wav");
        let original = b"original destination bytes";
        std::fs::write(&path, original).unwrap();
        let targets = [
            AutomationTarget::MasterVolume,
            AutomationTarget::MasterPan,
            AutomationTarget::Swing,
            AutomationTarget::MixerVolume { track: 1 },
            AutomationTarget::MixerPan { track: 1 },
            AutomationTarget::MixerMute { track: 1 },
            AutomationTarget::ChannelVolume { channel: 1 },
            AutomationTarget::ChannelPan { channel: 1 },
            AutomationTarget::ChannelMute { channel: 1 },
            AutomationTarget::PluginParameter {
                instance: 12,
                parameter: 3,
            },
        ];
        for target in targets {
            let mut project = arrangement_project();
            project
                .automation_lanes
                .push(unsupported_lane(target.clone()));
            let error = render_project_wav(&project, &path, 8000)
                .unwrap_err()
                .to_string();
            assert!(error.contains("Audible movement"));
            assert!(error.contains("lane 900"));
            assert!(error.contains(&automation_target_description(&target)));
            assert!(error.contains("Realtime Master Capture"));
            assert_eq!(std::fs::read(&path).unwrap(), original);
            assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn preflight_respects_disabled_empty_and_half_open_placement_gates() {
        let mut project = arrangement_project();
        project
            .automation_lanes
            .push(unsupported_lane(AutomationTarget::MasterVolume));
        project.automation_lanes[0].lane.set_enabled(false);
        ensure_supported_automation(&project).unwrap();
        project.automation_lanes[0].lane.set_enabled(true);
        project.automation_lanes[0].lane.replace_points([]);
        ensure_supported_automation(&project).unwrap();
        project.automation_lanes[0] = unsupported_lane(AutomationTarget::MasterVolume);
        for (start, length, muted) in [
            (0.0, 8.0, true),
            (8.0, 1.0, false),
            (10.0, 1.0, false),
            (-2.0, 2.0, false),
            (2.0, 0.0, false),
        ] {
            project.clips = vec![automation_placement(start, length, muted)];
            ensure_supported_automation(&project).unwrap();
        }
        for (start, length) in [(0.0, 1.0), (7.999, 1.0), (-1.0, 2.0)] {
            project.clips = vec![automation_placement(start, length, false)];
            assert!(ensure_supported_automation(&project).is_err());
        }
        project.clips = vec![
            automation_placement(0.0, 1.0, true),
            automation_placement(7.0, 1.0, false),
        ];
        assert!(ensure_supported_automation(&project).is_err());
    }

    #[test]
    fn unplaced_legacy_lanes_remain_global_and_unrelated_clips_do_not_gate_them() {
        let mut project = arrangement_project();
        project
            .automation_lanes
            .push(unsupported_lane(AutomationTarget::MasterPan));
        assert!(ensure_supported_automation(&project).is_err());
        let mut unrelated = automation_placement(0.0, 1.0, true);
        unrelated.automation_id = Some(901);
        project.clips.push(unrelated);
        assert!(ensure_supported_automation(&project).is_err());
        project.automation_lanes[0]
            .lane
            .set_target(AutomationTarget::Tempo);
        ensure_supported_automation(&project).unwrap();
    }

    #[test]
    fn cancellation_before_preflight_does_not_create_or_replace_a_file() {
        let directory = workflow_test_directory("early-cancel");
        let path = directory.join("not-created.wav");
        let control = ExportControl::default();
        assert!(control.cancel());
        let error = render_project_wav_controlled(&arrangement_project(), &path, 8000, &control)
            .unwrap_err();
        assert!(
            error
                .downcast_ref::<crate::export_job::ExportCancelled>()
                .is_some()
        );
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 0);
        std::fs::write(&path, b"original").unwrap();
        assert!(
            render_project_wav_controlled(&arrangement_project(), &path, 8000, &control).is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn cancellation_during_render_mix_encode_and_finalization_preserves_original_and_cleans_stage()
    {
        let directory = workflow_test_directory("phase-cancel");
        let source = directory.join("source.wav");
        let destination = directory.join("existing.wav");
        write_pcm16_wav(&source, 8000, 1, &vec![8192; 16000]);
        let mut project = audio_project(&source, 8000, 1, 16000);
        project.song_length_beats = 2.0;
        project.clips[0].length = 2.0;
        for threshold in [1000, 1500, 2000, 5000, 6000, 6800, 7200, 8000, 9800, 9900] {
            std::fs::write(&destination, b"original bytes").unwrap();
            let control = ExportControl::default();
            control.cancel_at(threshold);
            let error =
                render_project_wav_controlled(&project, &destination, 8000, &control).unwrap_err();
            assert!(
                error
                    .downcast_ref::<crate::export_job::ExportCancelled>()
                    .is_some(),
                "threshold {threshold}: {error:#}"
            );
            assert!(control.progress().cancelling);
            assert!(control.progress().basis_points < 10000);
            assert_eq!(
                std::fs::read(&destination).unwrap(),
                b"original bytes",
                "threshold {threshold}"
            );
            assert_eq!(
                std::fs::read_dir(&directory).unwrap().count(),
                2,
                "staging leaked at {threshold}"
            );
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn controlled_export_matches_compatible_wrapper_byte_for_byte() {
        let directory = workflow_test_directory("normal");
        let old_api = directory.join("wrapper.wav");
        let controlled = directory.join("controlled.wav");
        let project = arrangement_project();
        render_project_wav(&project, &old_api, 8000).unwrap();
        let control = ExportControl::default();
        render_project_wav_controlled(&project, &controlled, 8000, &control).unwrap();
        assert_eq!(
            std::fs::read(&old_api).unwrap(),
            std::fs::read(&controlled).unwrap()
        );
        assert_eq!(control.progress().basis_points, 10000);
        assert!(!control.cancel());
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 2);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rendered_wave_has_valid_header_and_audio() {
        let path = temporary_wav("export-audio");
        let mut project = arrangement_project();
        project.song_length_beats = 2.0;
        project.clips[0] = pattern_clip(0.0, 2.0, 1.0, false);
        render_project_wav(&project, &path, 8_000).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert!(bytes.len() > 44);
        assert!(bytes[44..].iter().any(|byte| *byte != 0));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn pattern_clip_honors_start_length_and_repeat_period() {
        let project = arrangement_project();
        let mut events = collected_events(&project, 100);
        events.sort_by_key(|event| event.sample);

        // The pattern is eight sixteenth-note steps (two beats), so a clip
        // beginning at beat 1 repeats at beats 1, 3 and 5. Its end is beat 5.1,
        // which excludes the next repeat and truncates the final voice tail.
        assert_eq!(
            events.iter().map(|event| event.sample).collect::<Vec<_>>(),
            vec![50, 150, 250]
        );
        assert!((events[2].duration - 0.05).abs() < 0.000_01);
    }

    #[test]
    fn pattern_clip_source_offset_slips_exported_events_without_moving_clip_edges() {
        let mut project = arrangement_project();
        project.clips[0].source_offset = 0.5;
        let mut events = collected_events(&project, 100);
        events.sort_by_key(|event| event.sample);

        assert_eq!(
            events.iter().map(|event| event.sample).collect::<Vec<_>>(),
            vec![125, 225]
        );
        assert!(
            events
                .iter()
                .all(|event| event.sample >= 50 && event.sample < 255)
        );
    }

    #[test]
    fn piano_note_does_not_repeat_at_the_short_channel_rack_period() {
        let mut project = arrangement_project();
        project.song_length_beats = 20.0;
        project.clips[0] = pattern_clip(0.0, 12.0, 1.0, false);
        project.patterns[0].length_steps = 16;
        project.patterns[0].channel_steps = vec![[false; 16]];
        project.patterns[0].notes = vec![PianoNote {
            id: 900,
            channel_id: Some(project.channels[0].id),
            group_id: None,
            note: 72,
            start: 0.5,
            length: 0.25,
            velocity: 1.0,
            selected: false,
            muted: false,
        }];

        let events = collected_events(&project, 100);
        assert_eq!(
            events.iter().map(|event| event.sample).collect::<Vec<_>>(),
            vec![25],
            "a Piano note must not inherit the two-beat Channel Rack cycle or repeat at beat 4"
        );
    }

    #[test]
    fn piano_note_repeats_at_beat_sixteen_and_the_clip_truncates_its_tail() {
        let mut project = arrangement_project();
        project.song_length_beats = 20.0;
        project.clips[0] = pattern_clip(0.0, 17.0, 1.0, false);
        project.patterns[0].length_steps = 16;
        project.patterns[0].channel_steps = vec![[false; 16]];
        project.patterns[0].notes = vec![PianoNote {
            id: 901,
            channel_id: Some(project.channels[0].id),
            group_id: None,
            note: 72,
            start: 0.0,
            length: 2.0,
            velocity: 1.0,
            selected: false,
            muted: false,
        }];

        let events = collected_events(&project, 100);
        assert_eq!(
            events.iter().map(|event| event.sample).collect::<Vec<_>>(),
            vec![0, 800]
        );
        assert!((events[0].duration - 1.0).abs() < 0.000_01);
        assert!((events[1].duration - 0.5).abs() < 0.000_01);
    }

    #[test]
    fn muted_pattern_clip_does_not_trigger_active_pattern_fallback() {
        let mut project = arrangement_project();
        project.clips[0].muted = true;

        assert!(collected_events(&project, 100).is_empty());
    }

    #[test]
    fn clip_gain_scales_every_generated_event() {
        let mut project = arrangement_project();
        project.clips[0] = pattern_clip(0.0, 1.0, 0.25, false);
        project.patterns[0].notes.push(PianoNote {
            id: 900,
            channel_id: Some(project.channels[0].id),
            group_id: None,
            note: 72,
            start: 0.5,
            length: 0.25,
            velocity: 1.0,
            selected: false,
            muted: false,
        });

        let events = collected_events(&project, 100);
        assert_eq!(events.len(), 2);
        assert!((events[0].velocity - 0.25).abs() < f32::EPSILON);
        assert!((events[1].velocity - 0.25 * 0.55).abs() < f32::EPSILON);
    }

    #[test]
    fn piano_notes_follow_their_stable_project_channel_routes() {
        let mut project = arrangement_project();
        let second_channel = Project::default().channels[1].clone();
        project.channels.push(second_channel);
        project.patterns[0].channel_steps = vec![[false; 16]; 2];
        project.clips[0] = pattern_clip(0.0, 1.0, 1.0, false);

        project.channels[0].volume = 0.25;
        project.channels[0].mixer_track = 1;
        project.mixer_tracks[1].pan = -1.0;
        project.channels[1].volume = 0.75;
        project.channels[1].mixer_track = 2;
        project.mixer_tracks[2].pan = 1.0;
        let first_channel_id = project.channels[0].id;
        let second_channel_id = project.channels[1].id;
        project.patterns[0].notes = vec![
            PianoNote {
                id: 901,
                channel_id: Some(first_channel_id),
                group_id: None,
                note: 60,
                start: 0.25,
                length: 0.25,
                velocity: 1.0,
                selected: false,
                muted: false,
            },
            PianoNote {
                id: 902,
                channel_id: Some(second_channel_id),
                group_id: None,
                note: 67,
                start: 0.25,
                length: 0.25,
                velocity: 1.0,
                selected: false,
                muted: false,
            },
        ];

        let events = collected_events(&project, 100);
        assert_eq!(events.len(), 2);
        let first = events
            .iter()
            .find(|event| (event.frequency - midi_frequency(60)).abs() < f32::EPSILON)
            .unwrap();
        let second = events
            .iter()
            .find(|event| (event.frequency - midi_frequency(67)).abs() < f32::EPSILON)
            .unwrap();
        assert!((first.velocity - 0.25 * 0.55).abs() < f32::EPSILON);
        assert_eq!((first.left_gain, first.right_gain), (1.0, 0.0));
        assert!((second.velocity - 0.75 * 0.55).abs() < f32::EPSILON);
        assert_eq!((second.left_gain, second.right_gain), (0.0, 1.0));

        project.patterns[0].notes[0].channel_id = None;
        let tempo_map = TempoMap::from_project(&project, 100).unwrap();
        let mixer_plan = OfflineMixerPlan::build(&project).unwrap();
        let error = match collect_events(&project, &mixer_plan, &tempo_map) {
            Ok(_) => panic!("unassigned Piano Roll note was accepted"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("note 60 (id 901)"));
        assert!(error.to_string().contains("no project channel assignment"));
    }

    #[test]
    fn pattern_clip_start_and_end_follow_tempo_automation() {
        let mut project = arrangement_project();
        project.clips[0] = pattern_clip(1.0, 0.25, 1.0, false);
        let mut tempo = AutomationLane::new(AutomationTarget::Tempo);
        tempo.set_curve(AutomationCurve::Hold);
        tempo.replace_points([AutomationPoint::new(0.0, 60.0)]);
        project.automation_lanes.push(ProjectAutomation {
            id: 800,
            name: "Export tempo".into(),
            lane: tempo,
        });

        let events = collected_events(&project, 100);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].sample, 100);
        assert!((events[0].duration - 0.25).abs() < 0.000_01);
    }

    #[test]
    fn mixer_mute_and_solo_gate_routed_pattern_events() {
        let mut project = arrangement_project();
        project.clips[0] = pattern_clip(0.0, 1.0, 1.0, false);

        project.mixer_tracks[1].muted = true;
        assert!(collected_events(&project, 100).is_empty());

        project.mixer_tracks[1].muted = false;
        project.mixer_tracks[2].solo = true;
        assert!(collected_events(&project, 100).is_empty());

        project.mixer_tracks[1].solo = true;
        assert_eq!(collected_events(&project, 100).len(), 1);

        project.mixer_tracks[1].muted = true;
        assert!(collected_events(&project, 100).is_empty());

        project.mixer_tracks[1].muted = false;
        project.mixer_tracks[1].solo = false;
        project.mixer_tracks[2].solo = false;
        project.mixer_tracks[0].muted = true;
        assert!(collected_events(&project, 100).is_empty());
    }

    #[test]
    fn audio_clip_resamples_at_original_speed_and_honors_timing_and_source_offset() {
        let source = temporary_wav("audio-source-resample");
        let output = temporary_wav("audio-render-resample");
        let input = [3_277_i16, 6_554, 9_830];
        write_pcm16_wav(&source, 4_000, 1, &input);
        let mut project = audio_project(&source, 4_000, 1, input.len() as u64);
        project.clips[0].start = 1.0 / 8_000.0;
        project.clips[0].length = 3.0 / 8_000.0;
        // One source frame at 4 kHz becomes two frames after resampling to 8 kHz.
        project.clips[0].audio_source_offset_frame = Some(1);

        let rendered = render_and_decode(&project, &output);
        let middle = (pcm16(input[1]) + pcm16(input[2])) * 0.5;
        assert!(rendered[0].left.abs() < 1.0e-6);
        assert!((rendered[1].left - pcm16(input[1])).abs() < 2.0e-5);
        assert!((rendered[2].left - middle).abs() < 2.0e-5);
        assert!((rendered[3].left - pcm16(input[2])).abs() < 2.0e-5);
        assert!(rendered[4].left.abs() < 1.0e-6);
        for frame in &rendered[..5] {
            assert!((frame.left - frame.right).abs() < 1.0e-6);
        }

        let _ = std::fs::remove_file(source);
        let _ = std::fs::remove_file(output);
    }

    #[test]
    fn audio_clip_gain_and_normalized_fades_shape_the_rendered_envelope() {
        let source = temporary_wav("audio-source-fade");
        let output = temporary_wav("audio-render-fade");
        let input = [26_214_i16; 4];
        write_pcm16_wav(&source, 8_000, 1, &input);
        let mut project = audio_project(&source, 8_000, 1, input.len() as u64);
        project.clips[0].length = 4.0 / 8_000.0;
        project.clips[0].gain = 0.5;
        project.clips[0].fade_in = 0.5;
        project.clips[0].fade_out = 0.5;

        let rendered = render_and_decode(&project, &output);
        let full = pcm16(input[0]) * 0.5;
        assert!(rendered[0].left.abs() < 1.0e-6);
        assert!((rendered[1].left - full).abs() < 2.0e-5);
        assert!((rendered[2].left - full).abs() < 2.0e-5);
        assert!(rendered[3].left.abs() < 1.0e-6);

        let _ = std::fs::remove_file(source);
        let _ = std::fs::remove_file(output);
    }

    #[test]
    fn audio_clip_fades_follow_tempo_boundaries_and_equal_power_in_offline_render() {
        let source = temporary_wav("audio-source-tempo-fade");
        let output = temporary_wav("audio-render-tempo-fade");
        let input = vec![16_384_i16; 16_000];
        write_pcm16_wav(&source, 8_000, 1, &input);
        let mut project = audio_project(&source, 8_000, 1, input.len() as u64);
        project.song_length_beats = 2.0;
        project.clips[0].length = 2.0;
        project.clips[0].fade_in = 0.5;
        project.clips[0].fade_out = 0.5;
        let mut tempo = AutomationLane::new(AutomationTarget::Tempo);
        tempo.set_curve(AutomationCurve::Hold);
        tempo.replace_points([
            AutomationPoint::new(0.0, 60.0),
            AutomationPoint::new(1.0, 120.0),
        ]);
        project.automation_lanes.push(ProjectAutomation {
            id: 91,
            name: "Tempo boundary".into(),
            lane: tempo,
        });

        let rendered = render_and_decode(&project, &output);
        let full = pcm16(input[0]);
        assert_eq!(rendered.len(), 12_000);
        assert!(rendered[0].left.abs() < 1.0e-6);
        assert!((rendered[4_000].left - full * std::f32::consts::FRAC_1_SQRT_2).abs() < 2.0e-4);
        assert!((rendered[7_999].left - full).abs() < 2.0e-5);
        assert!((rendered[8_000].left - full).abs() < 2.0e-5);
        assert!((rendered[10_000].left - full * std::f32::consts::FRAC_1_SQRT_2).abs() < 2.0e-4);
        assert!(rendered[11_999].left.abs() < 2.0e-5);

        let _ = std::fs::remove_file(source);
        let _ = std::fs::remove_file(output);
    }

    #[test]
    fn audio_clip_uses_first_lr_channels_and_playlist_mixer_route() {
        let source = temporary_wav("audio-source-routing");
        let output = temporary_wav("audio-render-routing");
        let input = [6_554_i16, 19_661, 29_491];
        write_pcm16_wav(&source, 8_000, 3, &input);
        let mut project = audio_project(&source, 8_000, 3, 1);
        project.clips[0].length = 1.0 / 8_000.0;
        // Playlist track 0 routes to insert 1. Hard-right balance proves both
        // that insert pan is applied and the third source channel is ignored.
        project.mixer_tracks[0].volume = 0.5;
        project.mixer_tracks[1].volume = 0.5;
        project.mixer_tracks[1].pan = 1.0;

        let rendered = render_and_decode(&project, &output);
        assert!(rendered[0].left.abs() < 1.0e-6);
        assert!((rendered[0].right - pcm16(input[1]) * 0.25).abs() < 2.0e-5);

        project.mixer_tracks[2].solo = true;
        let rendered = render_and_decode(&project, &output);
        assert!(
            rendered
                .iter()
                .all(|frame| frame.left == 0.0 && frame.right == 0.0)
        );

        project.mixer_tracks[1].solo = true;
        let rendered = render_and_decode(&project, &output);
        assert!(rendered[0].right > 0.0);

        project.mixer_tracks[1].muted = true;
        let rendered = render_and_decode(&project, &output);
        assert!(
            rendered
                .iter()
                .all(|frame| frame.left == 0.0 && frame.right == 0.0)
        );

        let _ = std::fs::remove_file(source);
        let _ = std::fs::remove_file(output);
    }

    #[test]
    fn stereo_audio_clip_traverses_the_compiled_submix_chain() {
        let source = temporary_wav("audio-source-dag-stereo");
        let output = temporary_wav("audio-render-dag-stereo");
        let input = [6_554_i16, 19_661_i16];
        write_pcm16_wav(&source, 8_000, 2, &input);
        let mut project = audio_project(&source, 8_000, 2, 1);
        project.clips[0].length = 1.0 / 8_000.0;
        project.mixer_routes.clear();
        project.mixer_routes.extend([
            main_route(1, 0, 1, 2, MixerRouteTap::PostFader, 0.5),
            main_route(
                2,
                1,
                2,
                MASTER_MIXER_TRACK_ID,
                MixerRouteTap::PostFader,
                1.0,
            ),
        ]);
        project.mixer_tracks[1].volume = 0.5;
        project.mixer_tracks[2].volume = 0.5;
        project.mixer_tracks[0].volume = 0.5;

        let rendered = render_and_decode(&project, &output);
        let transfer = 0.5 * 0.5 * 0.5 * 0.5;
        assert!((rendered[0].left - pcm16(input[0]) * transfer).abs() < 2.0e-5);
        assert!((rendered[0].right - pcm16(input[1]) * transfer).abs() < 2.0e-5);

        let _ = std::fs::remove_file(source);
        let _ = std::fs::remove_file(output);
    }

    #[test]
    fn audio_clip_playlist_route_saturates_at_insert_31() {
        let source = temporary_wav("audio-source-route31");
        let output = temporary_wav("audio-render-route31");
        write_pcm16_wav(&source, 8_000, 1, &[8_192]);
        let mut project = audio_project(&source, 8_000, 1, 1);
        project.clips[0].length = 1.0 / 8_000.0;
        project.clips[0].track = usize::MAX;
        project.audio_clip_mixer_destinations[0].mixer_track_id =
            project.mixer_track_id_at_runtime_slot(31).unwrap();
        while project.mixer_tracks.len() < 32 {
            let mut track = project.mixer_tracks.last().unwrap().clone();
            track.volume = 1.0;
            track.pan = 0.0;
            track.muted = false;
            track.solo = false;
            project.mixer_tracks.push(track);
        }
        project.mixer_tracks[31].muted = true;

        let rendered = render_and_decode(&project, &output);
        assert!(
            rendered
                .iter()
                .all(|frame| frame.left == 0.0 && frame.right == 0.0)
        );

        let _ = std::fs::remove_file(source);
        let _ = std::fs::remove_file(output);
    }

    #[test]
    fn audio_clip_reports_missing_and_corrupt_assets_but_muted_clips_skip_them() {
        let source = temporary_wav("audio-source-corrupt");
        let output = temporary_wav("audio-render-errors");
        std::fs::write(&source, b"not a RIFF/WAVE file").unwrap();
        let mut project = audio_project(&source, 8_000, 1, 1);

        project.clips[0].audio_asset_id = None;
        let error = render_project_wav(&project, &output, 8_000).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("Test Audio Clip"));
        assert!(message.contains("no audio asset reference"));

        project.clips[0].audio_asset_id = Some(700);
        let asset = project.audio_assets.pop().unwrap();
        let error = render_project_wav(&project, &output, 8_000).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("Test Audio Clip"));
        assert!(message.contains("missing audio asset 700"));

        project.audio_assets.push(asset);
        let error = render_project_wav(&project, &output, 8_000).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("Test Audio Clip"));
        assert!(message.contains(&source.display().to_string()));
        assert!(message.contains("RIFF"));

        // Mixer-gated clips must not touch the corrupt file at all.
        project.mixer_tracks[1].muted = true;
        let rendered = render_and_decode(&project, &output);
        assert!(
            rendered
                .iter()
                .all(|frame| frame.left == 0.0 && frame.right == 0.0)
        );

        project.mixer_tracks[1].muted = false;
        project.mixer_tracks[2].solo = true;
        let rendered = render_and_decode(&project, &output);
        assert!(
            rendered
                .iter()
                .all(|frame| frame.left == 0.0 && frame.right == 0.0)
        );

        project.mixer_tracks[2].solo = false;
        project.clips[0].muted = true;
        project.clips[0].audio_asset_id = None;
        project.audio_assets.clear();
        let rendered = render_and_decode(&project, &output);
        assert!(
            rendered
                .iter()
                .all(|frame| frame.left == 0.0 && frame.right == 0.0)
        );

        let _ = std::fs::remove_file(source);
        let _ = std::fs::remove_file(output);
    }

    #[test]
    fn audio_clip_rejects_an_unresolved_native_frame_offset() {
        let source = temporary_wav("audio-source-unresolved-offset");
        let output = temporary_wav("audio-render-unresolved-offset");
        write_pcm16_wav(&source, 8_000, 1, &[8_192]);
        let mut project = audio_project(&source, 8_000, 1, 1);
        // A legacy beat offset must never be guessed into a native source frame.
        project.clips[0].source_offset = 3.5;
        project.clips[0].audio_source_offset_frame = None;

        let error = render_project_wav(&project, &output, 8_000).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("Test Audio Clip"));
        assert!(message.contains("no resolved native-frame source offset"));

        let _ = std::fs::remove_file(source);
        let _ = std::fs::remove_file(output);
    }

    #[test]
    fn song_length_produces_exact_stereo_pcm24_frame_count() {
        let path = temporary_wav("export-length");
        let mut project = arrangement_project();
        project.tempo = 123.0;
        project.song_length_beats = 2.25;
        project.clips[0].muted = true;

        render_project_wav(&project, &path, 8_000).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let data_size = u32::from_le_bytes(bytes[40..44].try_into().unwrap()) as usize;
        let expected_frames = (2.25_f64 * 60.0 / 123.0 * 8_000.0).round() as usize;

        assert_eq!(expected_frames, 8_780);
        assert_eq!(data_size, expected_frames * 2 * 3);
        assert_eq!(bytes.len(), 44 + data_size);
        assert_eq!(
            u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize,
            36 + data_size
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn render_buffer_size_overflow_is_reported_without_allocating() {
        let error = allocate_render_buffer(usize::MAX, 48_000).unwrap_err();
        let message = format!("{error:#}");

        assert!(message.contains("Unable to allocate"));
        assert!(message.contains("stereo render buffer"));
        assert!(message.contains("address space"));
    }

    #[test]
    fn unsupported_export_sample_rates_preserve_existing_destination() {
        let path = temporary_wav("invalid-export-rate");
        std::fs::write(&path, b"previous export").unwrap();
        let project = arrangement_project();
        for rate in [0, 7_999, 192_001, u32::MAX] {
            let error = render_project_wav(&project, &path, rate).unwrap_err();
            assert!(error.to_string().contains("sample rate"));
            assert_eq!(std::fs::read(&path).unwrap(), b"previous export");
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn export_sample_rate_boundaries_are_written_exactly() {
        let path = temporary_wav("export-rate-boundaries");
        for rate in [8_000, 192_000] {
            write_stereo_pcm24(&path, rate, &[StereoFrame::default()], 1.0).unwrap();
            let decoded = wav::read_wav(&path).unwrap();
            assert_eq!(decoded.metadata.sample_rate, rate);
            assert_eq!(decoded.metadata.frames, 1);
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn pcm24_writer_rejects_non_finite_audio_and_preserves_destination() {
        let path = temporary_wav("non-finite-export");
        std::fs::write(&path, b"previous export").unwrap();
        for (sample, gain) in [
            (f32::NAN, 1.0),
            (f32::INFINITY, 1.0),
            (f32::NEG_INFINITY, 1.0),
            (0.5, f32::NAN),
            (f32::MAX, 2.0),
        ] {
            let frames = [StereoFrame {
                left: 0.25,
                right: sample,
            }];
            assert!(write_stereo_pcm24(&path, 48_000, &frames, gain).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), b"previous export");
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn streaming_pcm24_writer_replaces_existing_file_across_buffer_boundaries() {
        let path = temporary_wav("streaming-replace");
        std::fs::write(&path, b"previous export must survive until commit").unwrap();
        let frames = vec![
            StereoFrame {
                left: 0.0,
                right: 0.25,
            };
            20_000
        ];

        write_stereo_pcm24(&path, 48_000, &frames, 1.0).unwrap();
        let decoded = wav::read_wav(&path).unwrap();
        assert_eq!(decoded.metadata.bits_per_sample, 24);
        assert_eq!(decoded.metadata.frames, 20_000);
        assert_eq!(decoded.samples[0], 0.0);
        assert!((decoded.samples[1] - 0.25).abs() < 2.0e-6);

        let file_name = path.file_name().unwrap().to_string_lossy();
        let prefix = format!(".{file_name}.");
        let leftovers = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                name.starts_with(&prefix) && name.ends_with(".tmp")
            })
            .count();
        assert_eq!(leftovers, 0);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn failed_staged_commit_keeps_destination_and_cleans_temporary_file() {
        let sequence = STAGED_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let destination = std::env::temp_dir().join(format!(
            "citrus-export-preserve-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir(&destination).unwrap();
        let marker = destination.join("existing-export-marker");
        std::fs::write(&marker, b"keep me").unwrap();

        let control = ExportControl::default();
        let result = write_stereo_pcm24_controlled(
            &destination,
            48_000,
            &[StereoFrame {
                left: 0.1,
                right: -0.1,
            }],
            1.0,
            &control,
        );
        let error = result.unwrap_err();
        assert!(
            error
                .downcast_ref::<crate::export_job::ExportCancelled>()
                .is_none()
        );
        assert_eq!(control.progress().basis_points, 9900);
        assert!(
            !control.cancel(),
            "commit failure is not a cancelled result"
        );
        assert_eq!(std::fs::read(&marker).unwrap(), b"keep me");

        let file_name = destination.file_name().unwrap().to_string_lossy();
        let prefix = format!(".{file_name}.");
        let leftovers = std::fs::read_dir(destination.parent().unwrap())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                name.starts_with(&prefix) && name.ends_with(".tmp")
            })
            .count();
        assert_eq!(leftovers, 0);

        std::fs::remove_file(marker).unwrap();
        std::fs::remove_dir(destination).unwrap();
    }
    /// Exercise the production edit and render paths with real media. The
    /// fixtures intentionally have a non-grid start and native-frame in-point:
    /// restarting resampling at each child can pass a DC-only envelope test.
    struct SplitExportFiles(Vec<PathBuf>);

    impl SplitExportFiles {
        fn new() -> Self {
            Self(Vec::new())
        }

        fn path(&mut self, label: &str, extension: &str) -> PathBuf {
            let serial = STAGED_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "citrus-split-export-{label}-{}-{serial}.{extension}",
                std::process::id()
            ));
            self.0.push(path.clone());
            path
        }
    }

    impl Drop for SplitExportFiles {
        fn drop(&mut self) {
            for path in &self.0 {
                let _ = std::fs::remove_file(path);
            }
        }
    }

    fn split_export_source(
        files: &mut SplitExportFiles,
        source_rate: u32,
        nonconstant: bool,
    ) -> Project {
        let source = files.path("source", "wav");
        let frames = source_rate as usize * 2;
        let mut input = Vec::with_capacity(frames * 2);
        for frame in 0..frames {
            let (left, right) = if nonconstant {
                // Different ramps plus sparse, asymmetric impulses make a
                // one-frame source-phase error visible in either channel.
                let left = if frame % 997 == 137 {
                    27_000
                } else {
                    ((frame * 73) % 24_001) as i16 - 12_000
                };
                let right = if frame % 619 == 71 {
                    -25_000
                } else {
                    10_000 - ((frame * 31) % 20_003) as i16
                };
                (left, right)
            } else {
                (12_288, -8_192)
            };
            input.extend_from_slice(&[left, right]);
        }
        write_pcm16_wav(&source, source_rate, 2, &input);
        let mut project = audio_project(&source, source_rate, 2, frames as u64);
        project.tempo = 137.0;
        project.song_length_beats = 0.65;
        project.clips[0] = audio_clip(0.037_031_25, 0.5, 137);
        project.clips[0].gain = 0.63;
        project
    }

    fn set_split_export_tempo(project: &mut Project, curve: Option<AutomationCurve>) {
        project.automation_lanes.clear();
        if let Some(curve) = curve {
            let mut lane = AutomationLane::new(AutomationTarget::Tempo);
            lane.set_curve(curve);
            lane.replace_points([
                AutomationPoint::new(0.0, 103.0),
                AutomationPoint::new(0.18, 191.0),
                AutomationPoint::new(0.37, 83.0),
                AutomationPoint::new(0.65, 151.0),
            ]);
            project.automation_lanes.push(ProjectAutomation {
                id: 991,
                name: "Split export tempo".into(),
                lane,
            });
        }
    }

    fn split_export_clip(project: &mut Project, clip_id: u32, beat: f32, edit_rate: u32) -> u32 {
        let index = project
            .clips
            .iter()
            .position(|clip| clip.id == clip_id)
            .unwrap();
        let right_id = project.clips.iter().map(|clip| clip.id).max().unwrap() + 1;
        let route = project.audio_clip_mixer_track_id(clip_id).unwrap();
        let map = TempoMap::from_project(project, edit_rate).unwrap();
        let (left, right) = crate::audio_clip::split_audio_clip(
            &project.clips[index],
            beat,
            right_id,
            Some(&map),
            project.tempo,
        )
        .unwrap();
        project.clips[index] = left;
        project.clips.insert(index + 1, right);
        project
            .audio_clip_mixer_destinations
            .push(AudioClipMixerDestination {
                clip_id: right_id,
                mixer_track_id: route,
            });
        right_id
    }

    fn prepared_audio_pcm(project: &Project, output_rate: u32) -> Vec<StereoFrame> {
        let map = TempoMap::from_project(project, output_rate).unwrap();
        let plan = OfflineMixerPlan::build(project).unwrap();
        let mut output = vec![StereoFrame::default(); map.duration_frames() as usize];
        let (assets, clips) = prepare_audio_clips(
            project,
            &plan,
            output_rate,
            &map,
            output.len(),
            &ExportControl::default(),
        )
        .unwrap();
        assert_eq!(
            clips.len(),
            project.clips.len(),
            "a split child was not rendered"
        );
        mix_prepared_audio(&assets, &clips, &mut output, &ExportControl::default()).unwrap();
        output
    }

    fn split_export_pcm_with_assets(
        project: &Project,
        output_rate: u32,
        assets: &HashMap<u64, PreparedAudioAsset>,
    ) -> Vec<StereoFrame> {
        let map = TempoMap::from_project(project, output_rate).unwrap();
        let plan = OfflineMixerPlan::build(project).unwrap();
        let mut output = vec![StereoFrame::default(); map.duration_frames() as usize];
        let clips: Vec<_> = project
            .clips
            .iter()
            .map(|clip| {
                let asset_id = clip.audio_asset_id.unwrap();
                let route = plan
                    .route_for_track_id(project.audio_clip_mixer_track_id(clip.id).unwrap())
                    .unwrap();
                prepare_audio_clip(
                    clip,
                    asset_id,
                    &assets[&asset_id],
                    route,
                    &map,
                    output_rate,
                    output.len(),
                )
                .unwrap()
                .expect("a split child was not rendered")
            })
            .collect();
        mix_prepared_audio(assets, &clips, &mut output, &ExportControl::default()).unwrap();
        output
    }

    fn assert_audio_pcm_bits_equal(expected: &[StereoFrame], actual: &[StereoFrame], case: &str) {
        assert_eq!(
            actual.len(),
            expected.len(),
            "{case}: render length changed"
        );
        if let Some((index, (expected, actual))) =
            expected.iter().zip(actual).enumerate().find(|(_, (a, b))| {
                a.left.to_bits() != b.left.to_bits() || a.right.to_bits() != b.right.to_bits()
            })
        {
            panic!(
                "{case}: PCM differs at frame {index}: expected {expected:?}, actual {actual:?}"
            );
        }
    }

    fn rendered_wav_bytes(
        project: &Project,
        output_rate: u32,
        files: &mut SplitExportFiles,
    ) -> Vec<u8> {
        let path = files.path("render", "wav");
        render_project_wav(project, &path, output_rate).unwrap();
        let decoded = wav::read_wav(&path).unwrap();
        assert_eq!(decoded.metadata.sample_rate, output_rate);
        assert_eq!(decoded.metadata.channels, 2);
        assert_eq!(decoded.metadata.bits_per_sample, 24);
        assert!(decoded.samples.iter().any(|sample| sample.abs() > 0.01));
        std::fs::read(path).unwrap()
    }

    #[test]
    fn audio_split_export_pcm_is_bitwise_identical_across_fades_rates_and_tempo() {
        let mut files = SplitExportFiles::new();
        for nonconstant in [false, true] {
            for (source_rate, output_rate) in [(48_000, 48_000), (44_100, 48_000), (48_000, 44_100)]
            {
                let source_project = split_export_source(&mut files, source_rate, nonconstant);
                // Decode/resample each actual WAV once; all test permutations
                // still run the production clip preparation and mixer.
                let asset = load_audio_asset(&source_project.audio_assets[0], output_rate).unwrap();
                let assets = HashMap::from([(700, asset)]);
                for curve in [
                    None,
                    Some(AutomationCurve::Hold),
                    Some(AutomationCurve::Linear),
                ] {
                    for (fade_in, fade_out) in [
                        (0.0, 0.0),
                        (0.25, 0.0),
                        (0.0, 0.25),
                        (0.25, 0.25),
                        (0.75, 0.75),
                        (1.0, 0.0),
                        (0.0, 1.0),
                        (1.0, 1.0),
                    ] {
                        let mut project = source_project.clone();
                        set_split_export_tempo(&mut project, curve);
                        project.clips[0].fade_in = fade_in;
                        project.clips[0].fade_out = fade_out;
                        let expected = split_export_pcm_with_assets(&project, output_rate, &assets);
                        assert!(expected.iter().any(|frame| frame.left.abs() > 0.01));
                        assert!(expected.iter().any(|frame| frame.left != frame.right));
                        // Cover inside/outside each fade and both exact 25%/75%
                        // boundaries, with both children subsequently split.
                        for fraction in [0.125, 0.25, 0.375, 0.5, 0.625, 0.75, 0.875] {
                            let mut split = project.clone();
                            let beat = split.clips[0].start + split.clips[0].length * fraction;
                            // Edits use a fixed 48 kHz clock even when export is
                            // 44.1 kHz; provenance must compile on either grid.
                            let right_id = split_export_clip(&mut split, 70, beat, 48_000);
                            let case = format!(
                                "nonconstant={nonconstant}, {source_rate}->{output_rate}, tempo={curve:?}, fades={fade_in}/{fade_out}, cut={fraction}"
                            );
                            assert_audio_pcm_bits_equal(
                                &expected,
                                &split_export_pcm_with_assets(&split, output_rate, &assets),
                                &case,
                            );
                            let left = split.clips.iter().find(|clip| clip.id == 70).unwrap();
                            let left_cut = left.start + left.length * 0.5;
                            split_export_clip(&mut split, 70, left_cut, 48_000);
                            let right =
                                split.clips.iter().find(|clip| clip.id == right_id).unwrap();
                            let right_cut = right.start + right.length * 0.5;
                            split_export_clip(&mut split, right_id, right_cut, 48_000);
                            assert_eq!(split.clips.len(), 4);
                            assert_audio_pcm_bits_equal(
                                &expected,
                                &split_export_pcm_with_assets(&split, output_rate, &assets),
                                &format!("{case}, nested both children"),
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn audio_split_export_wav_is_identical_after_moving_right_child_and_changing_tempo() {
        let mut files = SplitExportFiles::new();
        for (source_rate, output_rate) in [(44_100, 48_000), (48_000, 44_100)] {
            for original_curve in [
                None,
                Some(AutomationCurve::Hold),
                Some(AutomationCurve::Linear),
            ] {
                let mut project = split_export_source(&mut files, source_rate, true);
                set_split_export_tempo(&mut project, original_curve);
                project.clips[0].fade_in = 0.75;
                project.clips[0].fade_out = 1.0;
                let cut = project.clips[0].start + project.clips[0].length * 0.375;
                let right_id = split_export_clip(&mut project, 70, cut, 48_000);
                project.clips.retain(|clip| clip.id == right_id);
                project
                    .audio_clip_mixer_destinations
                    .retain(|route| route.clip_id == right_id);
                project.clips[0].start = 0.0;
                set_split_export_tempo(&mut project, Some(AutomationCurve::Linear));
                project.automation_lanes[0].lane.replace_points([
                    AutomationPoint::new(0.0, 181.0),
                    AutomationPoint::new(0.22, 97.0),
                    AutomationPoint::new(0.65, 143.0),
                ]);
                let expected_pcm = prepared_audio_pcm(&project, output_rate);
                let expected_wav = rendered_wav_bytes(&project, output_rate, &mut files);
                let next_cut = project.clips[0].length * 0.375;
                let nested_right = split_export_clip(&mut project, right_id, next_cut, 48_000);
                let last = project
                    .clips
                    .iter()
                    .find(|clip| clip.id == nested_right)
                    .unwrap();
                let last_cut = last.start + last.length * 0.5;
                split_export_clip(&mut project, nested_right, last_cut, 48_000);
                let case = format!(
                    "moved to zero, {source_rate}->{output_rate}, original tempo={original_curve:?}"
                );
                assert_audio_pcm_bits_equal(
                    &expected_pcm,
                    &prepared_audio_pcm(&project, output_rate),
                    &case,
                );
                assert_eq!(
                    rendered_wav_bytes(&project, output_rate, &mut files),
                    expected_wav,
                    "{case}"
                );
            }
        }
    }

    #[test]
    fn audio_split_export_v11_save_load_preserves_nested_split_pcm_and_wav() {
        let mut files = SplitExportFiles::new();
        for (source_rate, output_rate) in [(44_100, 48_000), (48_000, 44_100)] {
            let mut project = split_export_source(&mut files, source_rate, true);
            project.clips[0].length = 2.0;
            project.clips[0].fade_in = 0.75;
            project.clips[0].fade_out = 1.0;
            set_split_export_tempo(&mut project, Some(AutomationCurve::Linear));
            // Normalize before the baseline so ordinary project load rules
            // (minimum song and clip lengths) cannot mask a split regression.
            project.normalize();
            let expected_pcm = prepared_audio_pcm(&project, output_rate);
            let expected_wav = rendered_wav_bytes(&project, output_rate, &mut files);
            let start = project.clips[0].start;
            let right_id = split_export_clip(&mut project, 70, start + 0.75, 48_000);
            split_export_clip(&mut project, 70, start + 0.25, 48_000);
            split_export_clip(&mut project, right_id, start + 1.25, 48_000);
            let path = files.path("v11", "citrus");
            project.save(&path).unwrap();
            let restored = Project::load(&path).unwrap();
            assert_eq!(restored.format_version, 11);
            assert_eq!(restored.clips.len(), 4);
            for (before, after) in project.clips.iter().zip(&restored.clips) {
                assert_eq!(before.audio_source_reference, after.audio_source_reference);
                assert_eq!(before.audio_length_reference, after.audio_length_reference);
                assert_eq!(before.fade_in_reference, after.fade_in_reference);
                assert_eq!(before.fade_out_reference, after.fade_out_reference);
                assert_eq!(after.audio_source_offset_frame, Some(137));
            }
            let case = format!("v11 nested save/load {source_rate}->{output_rate}");
            assert_audio_pcm_bits_equal(
                &expected_pcm,
                &prepared_audio_pcm(&restored, output_rate),
                &case,
            );
            assert_eq!(
                rendered_wav_bytes(&restored, output_rate, &mut files),
                expected_wav,
                "{case}"
            );

            // A moved slice followed by a tempo edit must retain both the old
            // source interval and its new split interval through persistence.
            let mut moved = restored;
            let moved_id = moved.clips.last().unwrap().id;
            moved.clips.retain(|clip| clip.id == moved_id);
            moved
                .audio_clip_mixer_destinations
                .retain(|route| route.clip_id == moved_id);
            moved.clips[0].start = 0.0;
            set_split_export_tempo(&mut moved, Some(AutomationCurve::Hold));
            let moved_pcm = prepared_audio_pcm(&moved, output_rate);
            let moved_wav = rendered_wav_bytes(&moved, output_rate, &mut files);
            let moved_cut = moved.clips[0].length * 0.375;
            let moved_right = split_export_clip(&mut moved, moved_id, moved_cut, 48_000);
            let reference = moved
                .clips
                .iter()
                .find(|clip| clip.id == moved_right)
                .unwrap()
                .audio_source_reference
                .as_ref()
                .unwrap();
            assert_eq!(reference.elapsed_spans_seconds.len(), 2);
            let moved_path = files.path("v11-moved", "citrus");
            moved.save(&moved_path).unwrap();
            let reloaded = Project::load(&moved_path).unwrap();
            let case = format!("v11 moved/retempo save/load {source_rate}->{output_rate}");
            assert_audio_pcm_bits_equal(
                &moved_pcm,
                &prepared_audio_pcm(&reloaded, output_rate),
                &case,
            );
            assert_eq!(
                rendered_wav_bytes(&reloaded, output_rate, &mut files),
                moved_wav,
                "{case}"
            );
        }
    }

    #[test]
    fn audio_split_export_v10_without_references_preserves_legacy_pcm_and_wav() {
        let mut files = SplitExportFiles::new();
        for (source_rate, output_rate) in [(44_100, 48_000), (48_000, 44_100)] {
            for curve in [
                None,
                Some(AutomationCurve::Hold),
                Some(AutomationCurve::Linear),
            ] {
                let mut project = split_export_source(&mut files, source_rate, true);
                set_split_export_tempo(&mut project, curve);
                project.clips[0].length = 1.0;
                project.clips[0].fade_in = 0.625;
                project.clips[0].fade_out = 0.875;
                project.normalize();
                let expected_pcm = prepared_audio_pcm(&project, output_rate);
                let expected_wav = rendered_wav_bytes(&project, output_rate, &mut files);
                // Independently preserve the v10 endpoint arithmetic and
                // original equal-power envelope, rather than comparing only
                // two invocations of the new reference compiler.
                let clip = &project.clips[0];
                let map = TempoMap::from_project(&project, output_rate).unwrap();
                let start = tempo_frame(&map, clip.start).unwrap();
                let end = tempo_frame(&map, clip.start + clip.length).unwrap();
                let fade_in_end =
                    tempo_frame(&map, clip.start + clip.length * clip.fade_in).unwrap();
                let fade_out_start =
                    tempo_frame(&map, clip.start + clip.length * (1.0 - clip.fade_out)).unwrap();
                let source = load_audio_asset(&project.audio_assets[0], output_rate).unwrap();
                let source_start = ((137_u64 * u64::from(output_rate) + u64::from(source_rate / 2))
                    / u64::from(source_rate)) as usize;
                let mut legacy_pcm = vec![StereoFrame::default(); expected_pcm.len()];
                for (index, frame) in legacy_pcm[start..end].iter_mut().enumerate() {
                    let envelope = crate::clip_fade::equal_power_frame_gain(
                        index as u64,
                        (end - start) as u64,
                        (fade_in_end - start) as u64,
                        (end - fade_out_start) as u64,
                    );
                    let gain = clip.gain * envelope;
                    frame.left += source.samples[(source_start + index) * 2] * gain;
                    frame.right += source.samples[(source_start + index) * 2 + 1] * gain;
                }
                let case = format!("v10 {source_rate}->{output_rate}, tempo={curve:?}");
                assert_audio_pcm_bits_equal(&legacy_pcm, &expected_pcm, &case);
                let mut value = serde_json::to_value(&project).unwrap();
                value["format_version"] = serde_json::json!(10);
                for clip in value["clips"].as_array().unwrap() {
                    for field in [
                        "audio_source_reference",
                        "audio_length_reference",
                        "fade_in_reference",
                        "fade_out_reference",
                    ] {
                        assert!(
                            clip.get(field).is_none(),
                            "legacy fixture unexpectedly contains {field}"
                        );
                    }
                }
                let path = files.path("v10", "citrus");
                std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
                let restored = Project::load(&path).unwrap();
                assert!(
                    restored
                        .clips
                        .iter()
                        .all(|clip| clip.audio_source_reference.is_none()
                            && clip.audio_length_reference.is_none()
                            && clip.fade_in_reference.is_none()
                            && clip.fade_out_reference.is_none())
                );
                assert_audio_pcm_bits_equal(
                    &legacy_pcm,
                    &prepared_audio_pcm(&restored, output_rate),
                    &case,
                );
                assert_eq!(
                    rendered_wav_bytes(&restored, output_rate, &mut files),
                    expected_wav,
                    "{case}"
                );
            }
        }
    }

    #[test]
    fn audio_split_export_preserves_silence_after_source_exhaustion() {
        let mut files = SplitExportFiles::new();
        let path = files.path("short-source", "wav");
        let samples: Vec<i16> = (0..800).map(|frame| frame * 23 - 9_000).collect();
        write_pcm16_wav(&path, 8_000, 1, &samples);
        for source_offset in [0, 137] {
            for (fade_in, fade_out) in [(0.0, 0.0), (0.75, 1.0)] {
                let mut project = audio_project(&path, 8_000, 1, samples.len() as u64);
                project.tempo = 120.0;
                project.song_length_beats = 1.0;
                project.clips[0] = audio_clip(0.0, 1.0, source_offset);
                project.clips[0].fade_in = fade_in;
                project.clips[0].fade_out = fade_out;
                let expected = rendered_wav_bytes(&project, 8_000, &mut files);
                for cut in [0.1, 0.2, 0.5] {
                    let mut split = project.clone();
                    let right_id = split_export_clip(&mut split, 70, cut, 8_000);
                    assert_eq!(
                        rendered_wav_bytes(&split, 8_000, &mut files),
                        expected,
                        "source exhausted: offset={source_offset}, fades={fade_in}/{fade_out}, cut={cut}"
                    );
                    split_export_clip(&mut split, right_id, cut + (1.0 - cut) * 0.5, 8_000);
                    assert_eq!(
                        rendered_wav_bytes(&split, 8_000, &mut files),
                        expected,
                        "nested exhausted child: offset={source_offset}, fades={fade_in}/{fade_out}, cut={cut}"
                    );
                }
            }
        }
    }

    #[test]
    fn audio_split_export_preserves_nonbinary_endpoints_at_high_sample_rates() {
        let mut files = SplitExportFiles::new();
        for output_rate in [44_100, 48_000, 96_000] {
            for (fade_in, fade_out) in [(0.0, 0.0), (0.73, 0.82), (1.0, 1.0)] {
                let mut project = split_export_source(&mut files, 48_000, true);
                project.tempo = 127.0;
                project.song_length_beats = 4.5;
                project.clips[0].start = 0.071;
                project.clips[0].length = 4.223;
                project.clips[0].fade_in = fade_in;
                project.clips[0].fade_out = fade_out;
                let expected = prepared_audio_pcm(&project, output_rate);
                let right_id = split_export_clip(&mut project, 70, 1.409_691, 48_000);
                let case = format!(
                    "nonbinary endpoints, 48000->{output_rate}, fades={fade_in}/{fade_out}"
                );
                assert_audio_pcm_bits_equal(
                    &expected,
                    &prepared_audio_pcm(&project, output_rate),
                    &case,
                );
                split_export_clip(&mut project, 70, 0.533_71, 48_000);
                let last_id = split_export_clip(&mut project, right_id, 1.791_139, 48_000);
                split_export_clip(&mut project, last_id, 3.137_93, 48_000);
                assert_audio_pcm_bits_equal(
                    &expected,
                    &prepared_audio_pcm(&project, output_rate),
                    &format!("{case}, nested"),
                );
            }
        }
    }
    #[test]
    fn split_at_downsampled_native_end_keeps_the_resampler_endpoint_frame() {
        use crate::audio_clip::split_audio_clip;
        let source = temporary_wav("split-downsample-tail-source");
        let output = temporary_wav("split-downsample-tail-output");
        write_pcm16_wav(&source, 16_000, 1, &[16_384; 800]);
        let mut project = audio_project(&source, 16_000, 1, 800);
        project.tempo = 120.0;
        project.song_length_beats = 1.0;
        project.clips[0].length = 0.5;
        render_project_wav(&project, &output, 8_000).unwrap();
        let before = std::fs::read(&output).unwrap();
        let map = TempoMap::from_project(&project, 8_000).unwrap();
        let (left, right) =
            split_audio_clip(&project.clips[0], 0.1, 71, Some(&map), 120.0).unwrap();
        project.clips = vec![left, right];
        project
            .audio_clip_mixer_destinations
            .push(AudioClipMixerDestination {
                clip_id: 71,
                mixer_track_id: project.audio_clip_mixer_destinations[0].mixer_track_id,
            });
        render_project_wav(&project, &output, 8_000).unwrap();
        assert_eq!(std::fs::read(&output).unwrap(), before);
        let _ = std::fs::remove_file(source);
        let _ = std::fs::remove_file(output);
    }
}
