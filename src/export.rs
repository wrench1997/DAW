use std::{
    collections::{HashMap, hash_map::Entry},
    f32::consts::TAU,
    fs::{File, OpenOptions},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};

use anyhow::{Context, Result, anyhow, bail, ensure};

use crate::{
    audio_clip::{audio_end_beat, audio_metadata_valid, source_elapsed_frames},
    automation::{AutomationTarget, canonicalize_automation_target},
    clip_fade::{CompiledClipFades, compile_clip_fades},
    export_job::ExportControl,
    export_options::{WavExportOptions, WavExportReport, WavSampleFormat},
    instrument_bounce::{CapturedInstrumentBounce, InstrumentBouncePlan},
    mixer_graph::{
        CompiledMixerGraph, MASTER_MIXER_TRACK_ID, MIXER_GRAPH_MAX_NODES, MixerRouteTap,
        MixerTrackId, compile_mixer_graph,
    },
    model::{AudioAsset, Channel, Clip, ClipKind, Pattern, Project},
    sampler::{SamplerConfig, SamplerVoice},
    tempo_map::TempoMap,
    wav,
};

const PROGRESS_FRAME_INTERVAL: usize = 4096;
#[path = "export_master_controls.rs"]
mod master_controls;
#[path = "export_serial_master.rs"]
pub(crate) mod serial_master;
pub(crate) use master_controls::OfflineMasterControls;
#[cfg(test)]
#[path = "export_channel_controls_tests.rs"]
mod channel_controls_tests;
#[cfg(test)]
#[path = "export_master_mute_tests.rs"]
mod master_mute_tests;
#[cfg(windows)]
#[path = "export_native_file.rs"]
pub(crate) mod native_file;
#[cfg(test)]
#[path = "export_sampler_tests.rs"]
mod sampler_tests;
#[cfg(test)]
#[path = "export_workflow_tests.rs"]
mod workflow_tests;
const WAV_WRITE_BUFFER_BYTES: usize = 64 * 1024;
// Preserve the callback timeline's default minimum while both paths derive a
// longer cycle from the same Piano Roll content. Channel Rack steps retain
// their independent Pattern-length cycle below.
const LEGACY_PIANO_MINIMUM_BEATS: f64 = 16.0;
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
    note: u8,
    sampler: Option<SamplerConfig>,
    note_off_sample: usize,
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

struct OfflineSamplerVoice {
    voice: SamplerVoice,
    asset_id: u64,
    note_off_sample: Option<usize>,
    left_gain: f32,
    right_gain: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct StereoFrame {
    pub(crate) left: f32,
    pub(crate) right: f32,
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
    #[cfg(test)]
    fn build(project: &Project) -> Result<Self> {
        Self::build_with_master_controls(project, false, false, false)
    }

    fn build_with_master_controls(
        project: &Project,
        volume_driven: bool,
        pan_driven: bool,
        mute_driven: bool,
    ) -> Result<Self> {
        Self::build_validated(project, volume_driven, pan_driven, mute_driven, None)
    }

    fn build_validated(
        project: &Project,
        volume_driven: bool,
        pan_driven: bool,
        mute_driven: bool,
        captured_generator: Option<&CapturedGeneratorWhitelist>,
    ) -> Result<Self> {
        match captured_generator {
            Some(source) => ensure_only_captured_generator(project, source)?,
            None => ensure_no_active_plugins(project)?,
        }
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
            // The master mute kernel runs after source mixing. Preserve raw
            // sources even when its persisted base is muted, so an automated
            // unmute can make notes and decoded assets audible again.
            let static_muted = track.muted && !(mute_driven && track.id == MASTER_MIXER_TRACK_ID);
            node_gate[runtime_slot] = !static_muted
                && (!any_solo || solo_ancestors[runtime_slot] || solo_descendants[runtime_slot]);
        }

        let master = graph.nodes()[usize::from(graph.master_dense_index())];
        let master_slot = usize::from(master.runtime_slot);
        let master_track = &project.mixer_tracks[master.project_index];
        // Driven controls are applied after source mixing. Preserve the
        // original project bases in the compiled automation, including zero.
        let master_volume = if volume_driven {
            1.0
        } else {
            master_track.volume
        };
        let (master_pan_left, master_pan_right) =
            pan_balance(if pan_driven { 0.0 } else { master_track.pan });
        let mut left_transfer = [0.0_f32; MIXER_GRAPH_MAX_NODES];
        let mut right_transfer = [0.0_f32; MIXER_GRAPH_MAX_NODES];
        let mut reaches_master = [false; MIXER_GRAPH_MAX_NODES];
        if node_gate[master_slot] {
            reaches_master[master_slot] = true;
            left_transfer[master_slot] = master_volume * master_pan_left;
            right_transfer[master_slot] = master_volume * master_pan_right;
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

/// Only a capture validated against the original immutable project can create
/// this exception to the ordinary active-plug-in guard. No project projection
/// or disabled-instance copy participates in mixer validation.
struct CapturedGeneratorWhitelist {
    channel_id: u32,
    instance_id: u64,
}

fn ensure_only_captured_generator(
    project: &Project,
    source: &CapturedGeneratorWhitelist,
) -> Result<()> {
    // Even bypassed/inactive insert endpoints apply safe_plugin_sample to
    // their bus output. Folding those nonlinear stages into route gains would
    // change the realtime signal, so this bounded seam admits no insert slots.
    ensure!(
        project.mixer_insert_slots.is_empty(),
        "Whole-song master export does not support mixer insert slots, including disabled or bypassed slots"
    );
    let mut selected = 0;
    for channel in &project.channels {
        let Some(id) = channel.instrument_plugin_instance_id else {
            continue;
        };
        let mut instances = project
            .plugin_instances
            .iter()
            .filter(|instance| instance.id == id);
        let instance = instances
            .next()
            .context("Captured mixer references a missing plug-in")?;
        ensure!(
            instances.next().is_none(),
            "Captured mixer references a duplicate plug-in identity"
        );
        if instance.enabled && !instance.bypass {
            ensure!(
                channel.id == source.channel_id && id == source.instance_id,
                "Only the captured generator may be active in whole-song master export"
            );
            selected += 1;
        }
    }
    ensure!(
        selected == 1,
        "Whole-song master export requires exactly one captured active generator"
    );
    Ok(())
}

/// Validated composition of one strict dry instrument render into the original
/// static mixer graph and Master Volume/Pan/Mute timeline. It owns controls but
/// never owns a plug-in or publishes an intermediate file.
pub(crate) struct CapturedMasterSongMix {
    options: WavExportOptions,
    music_frames: usize,
    tail_frames: usize,
    left_gain: f32,
    right_gain: f32,
    master_controls: OfflineMasterControls,
}

impl CapturedMasterSongMix {
    pub(crate) fn prepare(
        project: &Project,
        capture: &CapturedInstrumentBounce,
        options: WavExportOptions,
        control: &ExportControl,
        deadline: Instant,
    ) -> Result<Self> {
        control.check_deadline(deadline)?;
        options.validate()?;
        let plan = capture.plan();
        ensure!(
            plan.is_master_song() && plan.occurrence_request().is_none(),
            "A whole-song master capture is required"
        );
        let tail_frames = options.tail_frames()?;
        ensure!(
            options.sample_rate == plan.sample_rate()
                && u32::try_from(tail_frames)? == plan.tail_frames(),
            "Master WAV options differ from captured sample rate or fixed tail"
        );
        // Revalidate all source/automation restrictions against the *original*
        // snapshot, then bind its complete serialized fingerprint to capture.
        let validated = InstrumentBouncePlan::prepare_master_song(
            project,
            plan.channel_id(),
            options.sample_rate,
            plan.tail_frames(),
        )
        .map_err(anyhow::Error::msg)?;
        ensure!(
            validated.project_fingerprint() == plan.project_fingerprint()
                && capture.identity().project_fingerprint == plan.project_fingerprint()
                && capture.identity().channel_id == plan.channel_id()
                && capture.identity().instance_id == plan.instance_id(),
            "Whole-song master snapshot differs from captured source fingerprint"
        );
        let tempo_map = TempoMap::from_project(project, options.sample_rate)?;
        let master_controls = OfflineMasterControls::prepare(project, &tempo_map)?;
        let master = project
            .mixer_tracks
            .iter()
            .find(|track| track.id == MASTER_MIXER_TRACK_ID)
            .context("Captured master mixer identity is absent")?;
        // Realtime SetMaster and automated bases are canonicalized to 0..1.2.
        // Reject a wider model-only base instead of silently exporting a gain
        // that cannot be represented by that realtime control contract.
        ensure!(
            master.volume.is_finite() && (0.0..=1.2).contains(&master.volume),
            "Whole-song master base volume must match the realtime range 0..1.2"
        );
        let source = CapturedGeneratorWhitelist {
            channel_id: plan.channel_id(),
            instance_id: plan.instance_id(),
        };
        let mixer = OfflineMixerPlan::build_validated(
            project,
            master_controls.drives_volume(),
            master_controls.drives_pan(),
            master_controls.drives_mute(),
            Some(&source),
        )?;
        let channel = project
            .channels
            .iter()
            .find(|channel| channel.id == source.channel_id)
            .context("Captured master source channel is absent")?;
        ensure!(
            channel.volume.is_finite() && channel.pan.is_finite(),
            "Captured master channel controls must be finite"
        );
        let any_solo = project.channels.iter().any(|channel| channel.solo);
        let audible = !channel.muted && (!any_solo || channel.solo);
        let (pan_left, pan_right) = pan_balance(channel.pan);
        let (left_gain, right_gain) = match mixer.route_for_track_id(channel.mixer_track) {
            Some(route) if audible => (
                channel.volume * pan_left * route.gain * route.left_gain,
                channel.volume * pan_right * route.gain * route.right_gain,
            ),
            _ => (0.0, 0.0),
        };
        ensure!(
            left_gain.is_finite() && right_gain.is_finite(),
            "Captured master routing gains must be finite"
        );
        let music_frames = usize::try_from(tempo_map.duration_frames())?;
        ensure!(
            tempo_map.duration_frames() == plan.timeline().duration_frames(),
            "Master controls duration differs from captured musical duration"
        );
        options.sample_format.data_size_for_frames(
            music_frames
                .checked_add(tail_frames)
                .context("Master frame count overflow")?,
        )?;
        control.check_deadline(deadline)?;
        Ok(Self {
            options,
            music_frames,
            tail_frames,
            left_gain,
            right_gain,
            master_controls,
        })
    }

    /// Consumes caller-owned latency-trimmed N+tail PCM. Deadline/cancellation
    /// fences continue through composition, encoding and the only commit CAS.
    pub(crate) fn compose_and_publish(
        mut self,
        mut stereo: Vec<StereoFrame>,
        path: &Path,
        reported_tail: u32,
        control: &ExportControl,
        deadline: Instant,
    ) -> Result<WavExportReport> {
        ensure!(
            stereo.len() == self.music_frames + self.tail_frames,
            "Strict dry PCM does not span the complete master N+tail interval"
        );
        for (block, samples) in stereo.chunks_mut(PROGRESS_FRAME_INTERVAL).enumerate() {
            control.check_deadline(deadline)?;
            let start = block
                .checked_mul(PROGRESS_FRAME_INTERVAL)
                .context("Master block offset overflow")?;
            control.work_progress(start, self.music_frames + self.tail_frames, 6_000, 6_500)?;
            for sample in samples.iter_mut() {
                ensure!(
                    sample.left.is_finite() && sample.right.is_finite(),
                    "Strict dry PCM contains non-finite audio"
                );
                // Realtime generator endpoint samples are protected before
                // channel volume/pan or bus routing, even for finite headroom.
                sample.left = sample.left.clamp(-8.0, 8.0) * self.left_gain;
                sample.right = sample.right.clamp(-8.0, 8.0) * self.right_gain;
            }
            self.master_controls
                .apply_block(u64::try_from(start)?, samples)?;
            // Both realtime graph and legacy master paths apply this once,
            // after route accumulation and Master Volume/Pan/Mute controls.
            for sample in samples {
                sample.left = sample.left.tanh();
                sample.right = sample.right.tanh();
            }
            control.check_deadline(deadline)?;
        }
        let mut peak = 0.0_f32;
        for (index, sample) in stereo.iter().enumerate() {
            if index % PROGRESS_FRAME_INTERVAL == 0 {
                control.work_progress(index, stereo.len(), 6_500, 7_000)?;
                control.check_deadline(deadline)?;
            }
            ensure!(
                sample.left.is_finite() && sample.right.is_finite(),
                "Whole-song master composition contains non-finite audio at frame {index}"
            );
            peak = peak.max(sample.left.abs()).max(sample.right.abs());
        }
        let gain = self.options.gain_for_peak(peak)?;
        write_stereo_wav_core(
            path,
            self.options,
            &stereo,
            gain,
            control,
            WavWritePolicy::checked(deadline),
        )?;
        Ok(WavExportReport {
            sample_rate: self.options.sample_rate,
            frames: stereo.len(),
            music_frames: self.music_frames,
            tail_frames: self.tail_frames,
            // Metadata-based truncation evidence; this is not a silence test.
            tail_limit_reached: u64::from(reported_tail) > self.tail_frames as u64,
            sample_peak: peak,
            applied_gain: gain,
        })
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

/// Validate unsupported processing before the UI asks for a destination. The
/// renderer independently repeats these guards against its cloned snapshot.
pub fn ensure_supported_project(project: &Project) -> Result<()> {
    ensure_supported_automation(project)?;
    let tempo_map = TempoMap::from_project(project, 48_000)?;
    let controls = OfflineMasterControls::prepare(project, &tempo_map)?;
    OfflineMixerPlan::build_with_master_controls(
        project,
        controls.drives_volume(),
        controls.drives_pan(),
        controls.drives_mute(),
    )
    .map(|_| ())
}

/// Pure preflight shared by the UI and renderer. Legacy lanes without any
/// Playlist placement are globally active, matching live automation/TempoMap.
/// Once a lane has placements, only their unmuted half-open windows enable it.
pub fn ensure_supported_automation(project: &Project) -> Result<()> {
    let song_end = f64::from(project.song_end_beat());
    for automation in &project.automation_lanes {
        let lane = &automation.lane;
        if !lane.is_enabled()
            || lane.points().is_empty()
            || matches!(
                canonicalize_automation_target(lane.target().clone()),
                AutomationTarget::Tempo
                    | AutomationTarget::MasterVolume
                    | AutomationTarget::MasterPan
                    | AutomationTarget::MixerMute {
                        track: MASTER_MIXER_TRACK_ID
                    }
            )
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
            song_end > 0.0
        } else {
            placements.any(|clip| {
                let start = f64::from(clip.start);
                let end = start + f64::from(clip.length);
                let overlap_start = start.max(0.0);
                let overlap_end = end.min(song_end);
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
                "Offline WAV export cannot render automation '{}' (lane {}, target {}). Only Tempo, Master Volume, Master Pan and Master Mute automation are supported. Disable this lane or use Realtime Master Capture to preserve the live result.",
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

#[allow(dead_code)]
pub fn render_project_wav_controlled(
    project: &Project,
    path: &Path,
    sample_rate: u32,
    control: &ExportControl,
) -> Result<()> {
    render_project_wav_with_options(
        project,
        path,
        WavExportOptions::legacy(sample_rate),
        control,
    )
    .map(|_| ())
}

/// The same renderer and atomic output path as the compatibility API, with an
/// explicit file format and level policy. A report is returned only after commit.
pub fn render_project_wav_with_options(
    project: &Project,
    path: &Path,
    options: WavExportOptions,
    control: &ExportControl,
) -> Result<WavExportReport> {
    control.checkpoint(0)?;
    options.validate()?;
    let sample_rate = options.sample_rate;
    ensure_supported_automation(project)?;
    let tempo_map = TempoMap::from_project(project, sample_rate)
        .context("Unable to build the project tempo map for WAV export")?;
    let mut master_controls = OfflineMasterControls::prepare(project, &tempo_map)?;
    let mixer_plan = OfflineMixerPlan::build_with_master_controls(
        project,
        master_controls.drives_volume(),
        master_controls.drives_pan(),
        master_controls.drives_mute(),
    )?;
    let music_frames = usize::try_from(tempo_map.duration_frames())
        .context("Project is too long for this platform")?;
    let tail_frames = options.tail_frames()?;
    let frame_count = music_frames
        .checked_add(tail_frames)
        .context("WAV music and tail frame count overflow")?;
    options.sample_format.data_size_for_frames(frame_count)?;
    let (audio_assets, audio_clips) = prepare_audio_clips(
        project,
        &mixer_plan,
        sample_rate,
        &tempo_map,
        music_frames,
        control,
    )?;
    control.checkpoint(1000)?;
    let mut events = collect_events_controlled(project, &mixer_plan, &tempo_map, control)?;
    events.sort_by_key(|event| event.sample);
    let sampler_assets = prepare_sampler_assets(project, &events, control)?;

    control.checkpoint(1200)?;
    let mut stereo = allocate_render_buffer(frame_count, sample_rate)?;
    control.checkpoint(1500)?;
    let mut voices = Vec::<Voice>::with_capacity(96);
    let mut sampler_voices = Vec::<OfflineSamplerVoice>::with_capacity(96);
    let mut next_event = 0;
    for (frame, output) in stereo.iter_mut().enumerate() {
        if frame % PROGRESS_FRAME_INTERVAL == 0 {
            control.work_progress(frame, frame_count, 1500, 5000)?;
        }
        // A fixed tail drains existing voices. It never admits another note
        // or replays the arrangement at its exclusive end.
        while frame < music_frames
            && next_event < events.len()
            && events[next_event].sample <= frame
        {
            let event = events[next_event];
            if let Some(config) = event.sampler {
                let Some(asset_id) = config.asset_id else {
                    next_event += 1;
                    continue;
                };
                let asset = &sampler_assets[&asset_id];
                let voice = SamplerVoice::new(
                    config,
                    event.note,
                    event.velocity,
                    asset.metadata.sample_rate,
                    sample_rate,
                    asset.frame_count(),
                )
                .with_context(|| {
                    format!(
                        "Invalid sampler configuration for audio asset {} at output frame {}",
                        asset_id, event.sample
                    )
                })?;
                sampler_voices.push(OfflineSamplerVoice {
                    voice,
                    asset_id,
                    note_off_sample: Some(event.note_off_sample.min(music_frames)),
                    left_gain: event.left_gain,
                    right_gain: event.right_gain,
                });
                next_event += 1;
                continue;
            }
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
        // Sampler PCM already carries its own velocity/ADSR. Apply the
        // channel/clip/mixer transfer once, without oscillator attenuation.
        for voice in &mut sampler_voices {
            if voice.note_off_sample.is_some_and(|stop| frame >= stop) {
                voice.voice.release();
                voice.note_off_sample = None;
            }
            let asset = &sampler_assets[&voice.asset_id];
            let [left, right] = voice
                .voice
                .next_frame(&asset.samples, usize::from(asset.metadata.channels));
            output.left += left * voice.left_gain;
            output.right += right * voice.right_gain;
        }
        sampler_voices.retain(|voice| voice.voice.is_active());
    }

    control.checkpoint(5000)?;
    mix_prepared_audio(&audio_assets, &audio_clips, &mut stereo, control)?;
    control.checkpoint(6500)?;
    // Decoded source assets can be much larger than the final render. Release
    // them before the output pass so source media and destination encoding do
    // not contribute to the same peak.
    drop(audio_clips);
    drop(audio_assets);
    drop(sampler_assets);
    master_controls.apply_all(&mut stereo, control)?;
    let tail_limit_reached = !voices.is_empty() || !sampler_voices.is_empty();

    let mut peak = 0.0_f32;
    for (index, frame) in stereo.iter().enumerate() {
        if index % PROGRESS_FRAME_INTERVAL == 0 {
            control.work_progress(index, stereo.len(), 6900, 7000)?;
        }
        ensure!(
            frame.left.is_finite() && frame.right.is_finite(),
            "WAV export contains non-finite audio at frame {index}"
        );
        peak = peak.max(frame.left.abs()).max(frame.right.abs());
    }
    let gain = options.gain_for_peak(peak)?;
    write_stereo_wav_controlled(path, options, &stereo, gain, control)
        .with_context(|| format!("Unable to export WAV to {}", path.display()))?;
    Ok(WavExportReport {
        sample_rate,
        frames: frame_count,
        music_frames,
        tail_frames,
        tail_limit_reached,
        sample_peak: peak,
        applied_gain: gain,
    })
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
            project.clip_length_beats(clip),
            clip.source_offset,
            clip.piano_period_beats(pattern, LEGACY_PIANO_MINIMUM_BEATS) as f32,
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
    piano_period: f32,
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
        || f64::from(clip_start) >= tempo_map.max_beat()
    {
        return Ok(());
    }
    let clip_end = (clip_start + clip_length).min(tempo_map.max_beat() as f32);
    let clip_start = clip_start.max(0.0);
    if clip_end <= clip_start {
        return Ok(());
    }
    let any_channel_solo = project.channels.iter().any(|channel| channel.solo);
    for (channel_index, channel) in project.channels.iter().enumerate() {
        if !native_channel_audible(channel, any_channel_solo) {
            continue;
        }
        let Some(route) = mixer_plan.route_for_track_id(channel.mixer_track) else {
            continue;
        };
        let (channel_left, channel_right) = pan_balance(channel.pan);
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
        let midi = channel.sampler.map_or(midi, |sampler| sampler.root_note);
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
                let note_off_sample = tempo_frame(tempo_map, (beat + 0.25).min(clip_end))?;
                let source_gain = channel.volume * route.gain * clip_gain;
                let sampler_gain = if channel.sampler.is_some() {
                    source_gain
                } else {
                    1.0
                };
                events.push(Event {
                    sample,
                    note: midi,
                    sampler: channel.sampler,
                    note_off_sample,
                    frequency: midi_frequency(midi),
                    velocity: if channel.sampler.is_some() {
                        1.0
                    } else {
                        source_gain
                    },
                    duration: (match waveform {
                        Waveform::Kick => 0.42_f32,
                        Waveform::Noise => 0.13,
                        Waveform::Sine => 0.62,
                        Waveform::Triangle => 0.85,
                    })
                    .min(clip_stop.saturating_sub(sample) as f32 / tempo_map.sample_rate() as f32),
                    waveform,
                    left_gain: route.left_gain * channel_left * sampler_gain,
                    right_gain: route.right_gain * channel_right * sampler_gain,
                });
            }
            cycle += cycle_beats;
        }
    }

    let mut cycle = -source_offset.rem_euclid(piano_period);
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
            if !native_channel_audible(channel, any_channel_solo) {
                continue;
            }
            let Some(route) = mixer_plan.route_for_track_id(channel.mixer_track) else {
                continue;
            };
            let (channel_left, channel_right) = pan_balance(channel.pan);
            let beat = clip_start + cycle + note.start;
            if beat < clip_start || beat >= clip_end {
                continue;
            }
            let sample = tempo_frame(tempo_map, beat)?;
            let note_end = (beat + note.length).min(clip_end);
            let end_sample = tempo_frame(tempo_map, note_end)?;
            let source_gain = channel.volume * route.gain * clip_gain;
            let sampler_gain = if channel.sampler.is_some() {
                source_gain
            } else {
                1.0
            };
            events.push(Event {
                sample,
                note: note.note,
                sampler: channel.sampler,
                note_off_sample: end_sample,
                frequency: midi_frequency(note.note),
                velocity: if channel.sampler.is_some() {
                    note.velocity
                } else {
                    note.velocity * channel.volume * route.gain * clip_gain * 0.55
                },
                duration: end_sample.saturating_sub(sample) as f32 / tempo_map.sample_rate() as f32,
                waveform: Waveform::Triangle,
                left_gain: route.left_gain * channel_left * sampler_gain,
                right_gain: route.right_gain * channel_right * sampler_gain,
            });
        }
        cycle += piano_period;
    }
    Ok(())
}

fn native_channel_audible(channel: &Channel, any_solo: bool) -> bool {
    !channel.muted && (!any_solo || channel.solo) && channel.instrument_plugin_instance_id.is_none()
}

fn pan_balance(pan: f32) -> (f32, f32) {
    let pan = pan.clamp(-1.0, 1.0);
    (1.0 - pan.max(0.0), 1.0 + pan.min(0.0))
}

/// Sampler trim/loop positions are native frames. Keep this decode separate
/// from the output-rate-resampled buffers used by Playlist audio clips.
fn prepare_sampler_assets(
    project: &Project,
    events: &[Event],
    control: &ExportControl,
) -> Result<HashMap<u64, wav::WavAsset>> {
    let mut assets = HashMap::new();
    for config in events.iter().filter_map(|event| event.sampler) {
        control.check_cancelled()?;
        let Some(asset_id) = config.asset_id else {
            continue;
        };
        let Entry::Vacant(entry) = assets.entry(asset_id) else {
            continue;
        };
        let asset = project
            .audio_assets
            .iter()
            .find(|asset| asset.id == asset_id)
            .with_context(|| format!("Sampler references missing audio asset {}", asset_id))?;
        let decoded = wav::read_wav(&asset.path).with_context(|| {
            format!(
                "Unable to prepare sampler audio asset {} ('{}') from '{}'",
                asset.id,
                asset.name,
                asset.path.display()
            )
        })?;
        ensure!(
            (1..=2).contains(&decoded.metadata.channels),
            "sampler audio asset {} has {} channels; only mono or stereo sources are supported",
            asset.id,
            decoded.metadata.channels
        );
        ensure!(
            decoded.metadata.sample_rate == asset.sample_rate,
            "sampler audio asset {} changed sample rate from {} Hz to {} Hz; its native-frame trim and loop positions are no longer valid",
            asset.id,
            asset.sample_rate,
            decoded.metadata.sample_rate
        );
        entry.insert(decoded);
    }
    Ok(assets)
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

#[cfg(test)]
fn write_stereo_pcm24(
    path: &Path,
    sample_rate: u32,
    stereo: &[StereoFrame],
    gain: f32,
) -> Result<()> {
    write_stereo_pcm24_controlled(path, sample_rate, stereo, gain, &ExportControl::default())
}

#[cfg(test)]
fn write_stereo_pcm24_controlled(
    path: &Path,
    sample_rate: u32,
    stereo: &[StereoFrame],
    gain: f32,
    control: &ExportControl,
) -> Result<()> {
    write_stereo_wav_controlled(
        path,
        WavExportOptions::legacy(sample_rate),
        stereo,
        gain,
        control,
    )
}

fn write_stereo_wav_controlled(
    path: &Path,
    options: WavExportOptions,
    stereo: &[StereoFrame],
    gain: f32,
    control: &ExportControl,
) -> Result<()> {
    write_stereo_wav_core(
        path,
        options,
        stereo,
        gain,
        control,
        WavWritePolicy::default(),
    )
}

/// Publish already-rendered stereo at unity gain, preserving finite Float32
/// headroom. The caller must finish checked backend cleanup before entering.
///
/// Checks are cooperative around normal filesystem operations, including the
/// final commit CAS; blocking OS writes/sync/rename are not forcibly interrupted.
pub(crate) fn write_stereo_float32_checked(
    path: &Path,
    sample_rate: u32,
    stereo: &[StereoFrame],
    control: &ExportControl,
    deadline: Instant,
) -> Result<()> {
    write_stereo_wav_core(
        path,
        WavExportOptions {
            sample_rate,
            sample_format: WavSampleFormat::Float32,
            level_policy: crate::export_options::WavLevelPolicy::PreserveLevel,
            tail_milliseconds: 0,
        },
        stereo,
        1.0,
        control,
        WavWritePolicy::checked(deadline),
    )
}

// A separate, closed publication policy. Ordinary exports continue to replace by
// rename; only occurrence publication can produce this ownership evidence.
pub(crate) const MAX_OCCURRENCE_MEDIA_FRAMES: usize = 24_000_000;
const MAX_OCCURRENCE_PATH_BYTES: usize = 4096;
const MAX_PUBLICATION_DIAGNOSTIC_BYTES: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PublicationPhase {
    PreLink,
    /// The file cancellation CAS won, but no destination ownership is proven.
    LinkStarted,
    Linked,
    FinalizationFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PublishedFileIdentity {
    pub(crate) device: u64,
    pub(crate) inode: u128,
    pub(crate) bytes: u64,
    pub(crate) modified_seconds: i64,
    pub(crate) modified_nanoseconds: i64,
}

impl PublishedFileIdentity {
    pub(crate) fn from_file(file: &File) -> std::io::Result<Self> {
        #[cfg(windows)]
        {
            native_file::inspect(file).map(|snapshot| snapshot.identity)
        }
        #[cfg(not(windows))]
        {
            Self::from_metadata(&file.metadata()?)
        }
    }

    #[cfg(not(windows))]
    pub(crate) fn from_metadata(metadata: &std::fs::Metadata) -> std::io::Result<Self> {
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::MetadataExt;
            if !metadata.is_file() {
                return Err(std::io::Error::other(
                    "Occurrence output is not a regular file",
                ));
            }
            Ok(Self {
                device: metadata.dev(),
                inode: u128::from(metadata.ino()),
                bytes: metadata.len(),
                modified_seconds: metadata.mtime(),
                modified_nanoseconds: metadata.mtime_nsec(),
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = metadata;
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Owned occurrence publication requires Linux or Windows file identity",
            ))
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PublicationErrorKind {
    Cancelled,
    Deadline,
    Invalid,
    Io,
    Panic,
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PublicationOperation {
    Write,
    Link,
    AfterLink,
    #[cfg(not(windows))]
    StageUnlink,
    #[cfg(not(windows))]
    ParentSync,
    Abort,
}

/// Fixed-size diagnostics cannot retain arbitrary panic payloads or grow with
/// cleanup retries. No audio or captured plug-in state enters this value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PublicationError {
    pub(crate) kind: PublicationErrorKind,
    pub(crate) operation: PublicationOperation,
    pub(crate) io_kind: Option<std::io::ErrorKind>,
    pub(crate) raw_os_error: Option<i32>,
    message: [u8; MAX_PUBLICATION_DIAGNOSTIC_BYTES],
    message_len: usize,
}

impl PublicationError {
    fn from_error(error: &anyhow::Error, operation: PublicationOperation) -> Self {
        let kind = if error.is::<crate::export_job::ExportCancelled>() {
            PublicationErrorKind::Cancelled
        } else if error.is::<crate::export_job::ExportDeadlineExceeded>() {
            PublicationErrorKind::Deadline
        } else if error.is::<WavStagingPanic>() {
            PublicationErrorKind::Panic
        } else if let Some(error) = error.downcast_ref::<std::io::Error>() {
            if error.kind() == std::io::ErrorKind::Unsupported {
                PublicationErrorKind::Unsupported
            } else {
                PublicationErrorKind::Io
            }
        } else {
            PublicationErrorKind::Invalid
        };
        let mut evidence = Self {
            kind,
            operation,
            io_kind: error
                .downcast_ref::<std::io::Error>()
                .map(std::io::Error::kind),
            raw_os_error: error
                .downcast_ref::<std::io::Error>()
                .and_then(std::io::Error::raw_os_error),
            message: [0; MAX_PUBLICATION_DIAGNOSTIC_BYTES],
            message_len: 0,
        };
        use std::fmt::Write as _;
        let _ = write!(evidence, "{error:#}");
        evidence
    }

    pub(crate) fn message(&self) -> &str {
        // fmt::Write admits only complete UTF-8 boundaries.
        std::str::from_utf8(&self.message[..self.message_len]).unwrap_or("Publication failure")
    }
}

impl std::fmt::Write for PublicationError {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        let remaining = self.message.len() - self.message_len;
        let mut count = remaining.min(text.len());
        while !text.is_char_boundary(count) {
            count -= 1;
        }
        self.message[self.message_len..self.message_len + count]
            .copy_from_slice(&text.as_bytes()[..count]);
        self.message_len += count;
        Ok(())
    }
}

impl std::fmt::Display for PublicationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message())
    }
}

#[derive(Debug)]
pub(crate) struct PublicationEvidence {
    pub(crate) phase: PublicationPhase,
    /// Present if and only if this invocation successfully created the link.
    #[allow(dead_code)] // Dormant Stop A: retained for the future App failure owner.
    pub(crate) retained_destination: Option<PathBuf>,
    #[allow(dead_code)] // Dormant Stop A: retained for the future App failure owner.
    pub(crate) retained_stage: Option<PathBuf>,
    #[allow(dead_code)] // Dormant Stop A: retained for the future App failure owner.
    pub(crate) identity: Option<PublishedFileIdentity>,
}

#[derive(Debug, thiserror::Error)]
#[error("Occurrence WAV publication failed ({phase:?}): {primary}", phase = .evidence.phase)]
pub(crate) struct CreateNewFailure {
    pub(crate) evidence: PublicationEvidence,
    pub(crate) primary: PublicationError,
    pub(crate) stage_unlink: Option<PublicationError>,
    pub(crate) parent_sync: Option<PublicationError>,
    pub(crate) abort: Option<PublicationError>,
}

/// Producer-only proof of this writer's successful no-clobber publication and
/// checked stage removal + directory sync. It is not yet insertion authority.
#[derive(Debug)]
pub(crate) struct PublishedFloat32 {
    path: PathBuf,
    identity: PublishedFileIdentity,
    sample_rate: u32,
    frames: usize,
    prewrite_peak: f32,
}

impl PublishedFloat32 {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
    pub(crate) fn identity(&self) -> PublishedFileIdentity {
        self.identity
    }
    pub(crate) fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    pub(crate) fn frames(&self) -> usize {
        self.frames
    }
    pub(crate) fn prewrite_peak(&self) -> f32 {
        self.prewrite_peak
    }
}

struct CreateNewPublication {
    destination: PathBuf,
    #[cfg(not(windows))]
    parent: Option<File>,
    phase: PublicationPhase,
    identity: Option<PublishedFileIdentity>,
    retained_stage: Option<PathBuf>,
    stage_unlink: Option<PublicationError>,
    parent_sync: Option<PublicationError>,
    abort: Option<PublicationError>,
    prewrite_peak: f32,
}

impl CreateNewPublication {
    fn fail(self, error: anyhow::Error) -> CreateNewFailure {
        let operation = match self.phase {
            PublicationPhase::PreLink => PublicationOperation::Write,
            PublicationPhase::LinkStarted => PublicationOperation::Link,
            PublicationPhase::Linked | PublicationPhase::FinalizationFailed => {
                PublicationOperation::AfterLink
            }
        };
        let linked = matches!(
            self.phase,
            PublicationPhase::Linked | PublicationPhase::FinalizationFailed
        );
        CreateNewFailure {
            evidence: PublicationEvidence {
                phase: if linked {
                    PublicationPhase::FinalizationFailed
                } else {
                    self.phase
                },
                retained_destination: linked.then_some(self.destination),
                retained_stage: self.retained_stage,
                identity: self.identity,
            },
            primary: PublicationError::from_error(&error, operation),
            stage_unlink: self.stage_unlink,
            parent_sync: self.parent_sync,
            abort: self.abort,
        }
    }

    #[cfg(not(windows))]
    fn finish_link(&mut self, staged: &mut StagedOutput, _policy: &WavWritePolicy) -> Result<()> {
        // Disable implicit Drop cleanup BEFORE either fallible attempt. We make
        // exactly one observable unlink attempt and retain its path on failure.
        staged.cleanup_on_drop = false;
        let unlink = catch_publication_io(|| {
            #[cfg(test)]
            _policy.operation(StagedOperation::Unlink)?;
            std::fs::remove_file(&staged.path)
        });
        if let Err(error) = unlink {
            self.stage_unlink = Some(PublicationError::from_error(
                &error,
                PublicationOperation::StageUnlink,
            ));
            self.retained_stage = Some(std::mem::take(&mut staged.path));
        }
        // Directory sync is required even when unlink failed or panicked. No
        // unsupported-platform or weaker-durability fallback is admitted.
        let sync = catch_publication_io(|| {
            #[cfg(test)]
            _policy.operation(StagedOperation::ParentSync)?;
            self.parent
                .as_ref()
                .expect("prepared parent directory")
                .sync_all()
        });
        if let Err(error) = sync {
            self.parent_sync = Some(PublicationError::from_error(
                &error,
                PublicationOperation::ParentSync,
            ));
        }
        if self.stage_unlink.is_some() || self.parent_sync.is_some() {
            self.phase = PublicationPhase::FinalizationFailed;
            bail!("Linked occurrence WAV retained; checked publication finalization failed");
        }
        Ok(())
    }

    #[cfg(windows)]
    fn finish_link(&mut self, staged: &mut StagedOutput, _policy: &WavWritePolicy) -> Result<()> {
        // The checked same-volume, no-replace WRITE_THROUGH move consumed the
        // stage. Windows has no Linux directory-fsync step to pretend to perform.
        // Even a post-publication panic must leave the destination retained.
        staged.cleanup_on_drop = false;
        Ok(())
    }
}

#[cfg(not(windows))]
fn catch_publication_io(operation: impl FnOnce() -> std::io::Result<()>) -> Result<()> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation))
        .map_err(|payload| anyhow::Error::from(WavStagingPanic::from_payload(payload)))?
        .map_err(Into::into)
}

/// Closed create-new counterpart to the ordinary replacing writer. The caller
/// must have finished checked backend destruction. File CAS and App cancellation
/// are independent: a won file CAS does not prove a link and cannot revoke one.
pub(crate) fn write_stereo_float32_create_new_checked(
    path: &Path,
    sample_rate: u32,
    stereo: &[StereoFrame],
    control: &ExportControl,
    deadline: Instant,
) -> std::result::Result<PublishedFloat32, Box<CreateNewFailure>> {
    write_float32_create_new_with_policy(
        path,
        sample_rate,
        stereo,
        control,
        WavWritePolicy::checked(deadline),
    )
}

fn write_float32_create_new_with_policy(
    path: &Path,
    sample_rate: u32,
    stereo: &[StereoFrame],
    control: &ExportControl,
    policy: WavWritePolicy,
) -> std::result::Result<PublishedFloat32, Box<CreateNewFailure>> {
    let mut publication = CreateNewPublication {
        destination: PathBuf::new(),
        #[cfg(not(windows))]
        parent: None,
        phase: PublicationPhase::PreLink,
        identity: None,
        retained_stage: None,
        stage_unlink: None,
        parent_sync: None,
        abort: None,
        prewrite_peak: 0.0,
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<()> {
        let path_text = path
            .to_str()
            .context("Occurrence output path must be UTF-8")?;
        ensure!(
            path.is_absolute() && path_text.len() <= MAX_OCCURRENCE_PATH_BYTES,
            "Occurrence output requires a bounded absolute path"
        );
        ensure!(
            path.components()
                .all(|component| !matches!(component, std::path::Component::ParentDir)),
            "Occurrence output cannot contain parent traversal"
        );
        let mut owned_path = String::new();
        owned_path
            .try_reserve_exact(path_text.len())
            .context("Unable to reserve occurrence path")?;
        owned_path.push_str(path_text);
        publication.destination = PathBuf::from(owned_path);
        if !cfg!(any(target_os = "linux", windows)) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Create-new occurrence publication is supported only on Linux and Windows",
            )
            .into());
        }
        ensure!(
            !stereo.is_empty() && stereo.len() <= MAX_OCCURRENCE_MEDIA_FRAMES,
            "Occurrence PCM extent must be between 1 and 24 million frames"
        );
        let parent = path
            .parent()
            .context("Occurrence output has no parent directory")?;
        #[cfg(not(windows))]
        {
            publication.parent = Some(
                File::open(parent)
                    .context("Unable to open occurrence output directory for sync")?,
            );
        }
        #[cfg(windows)]
        ensure!(parent.is_dir(), "Occurrence output directory is absent");
        write_stereo_wav_kernel(
            path,
            WavExportOptions {
                sample_rate,
                sample_format: WavSampleFormat::Float32,
                level_policy: crate::export_options::WavLevelPolicy::PreserveLevel,
                tail_milliseconds: 0,
            },
            stereo,
            1.0,
            control,
            policy,
            Some(&mut publication),
        )
    }))
    .unwrap_or_else(|payload| Err(WavStagingPanic::from_payload(payload).into()));
    match result {
        Err(error) => Err(Box::new(publication.fail(error))),
        Ok(()) => Ok(PublishedFloat32 {
            path: publication.destination,
            identity: publication
                .identity
                .expect("successful publication has staged identity"),
            sample_rate,
            frames: stereo.len(),
            prewrite_peak: publication.prewrite_peak,
        }),
    }
}

#[derive(Default)]
struct WavWritePolicy {
    // None preserves the ordinary writer's original cancellation/Drop behavior.
    deadline: Option<Instant>,
    #[cfg(test)]
    fixture: WavWriteFixture,
}

impl WavWritePolicy {
    fn checked(deadline: Instant) -> Self {
        Self {
            deadline: Some(deadline),
            #[cfg(test)]
            fixture: WavWriteFixture::default(),
        }
    }

    fn check(&self, control: &ExportControl) -> Result<()> {
        if let Some(deadline) = self.deadline {
            control.check_deadline(deadline)?;
        }
        Ok(())
    }

    fn begin_commit(&self, control: &ExportControl) -> Result<()> {
        #[cfg(test)]
        {
            if self.fixture.cancel_before_commit {
                assert!(control.cancel());
            }
            if self.fixture.expire_before_commit {
                return control.begin_commit_before(Instant::now());
            }
        }
        if let Some(deadline) = self.deadline {
            control.begin_commit_before(deadline)
        } else {
            control.begin_commit().map_err(Into::into)
        }
    }

    #[cfg(test)]
    fn operation(&self, operation: StagedOperation) -> std::io::Result<()> {
        assert_ne!(
            self.fixture.panic_at,
            Some(operation),
            "injected staged {operation:?} panic"
        );
        if self.fixture.fail == Some(operation)
            || (operation == StagedOperation::Abort && self.fixture.fail_abort)
            || (operation == StagedOperation::Unlink && self.fixture.fail_unlink)
            || (operation == StagedOperation::ParentSync && self.fixture.fail_parent_sync)
        {
            return Err(std::io::Error::other(format!(
                "injected staged {operation:?} failure"
            )));
        }
        Ok(())
    }
}

// Faults are local to one invocation. No process-global timing or I/O overrides
// can affect ordinary export tests running concurrently.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StagedOperation {
    Write,
    Flush,
    Sync,
    Rename,
    Link,
    AfterLink,
    Unlink,
    ParentSync,
    Abort,
}

#[cfg(test)]
#[derive(Default)]
struct WavWriteFixture {
    fail: Option<StagedOperation>,
    panic_at: Option<StagedOperation>,
    fail_abort: bool,
    fail_unlink: bool,
    fail_parent_sync: bool,
    cancel_after_cas: bool,
    cancel_after_link: bool,
    expire_before_commit: bool,
    cancel_before_commit: bool,
}

#[cfg(test)]
impl WavWriteFixture {
    fn genuine_inactive(&self) -> bool {
        self.fail.is_none()
            && self.panic_at.is_none()
            && !self.fail_abort
            && !self.fail_unlink
            && !self.fail_parent_sync
            && !self.cancel_after_cas
            && !self.cancel_after_link
            && !self.expire_before_commit
            && !self.cancel_before_commit
    }
}

fn write_stereo_wav_core(
    path: &Path,
    options: WavExportOptions,
    stereo: &[StereoFrame],
    gain: f32,
    control: &ExportControl,
    policy: WavWritePolicy,
) -> Result<()> {
    write_stereo_wav_kernel(path, options, stereo, gain, control, policy, None)
}

#[allow(clippy::too_many_arguments)]
fn write_stereo_wav_kernel(
    path: &Path,
    options: WavExportOptions,
    stereo: &[StereoFrame],
    gain: f32,
    control: &ExportControl,
    policy: WavWritePolicy,
    mut publication: Option<&mut CreateNewPublication>,
) -> Result<()> {
    #[cfg(test)]
    crate::genuine_app_observer::record_hooks(
        "actual occurrence WAV writer policy",
        policy.fixture.genuine_inactive(),
    );
    control.checkpoint(7000)?;
    policy.check(control)?;
    options.validate()?;
    let sample_rate = options.sample_rate;
    let format = options.sample_format;
    ensure!(gain.is_finite(), "WAV export gain must be finite");
    let mut prewrite_peak = 0.0_f32;
    for (index, frame) in stereo.iter().enumerate() {
        if index % PROGRESS_FRAME_INTERVAL == 0 {
            control.work_progress(index, stereo.len(), 7000, 7500)?;
            policy.check(control)?;
        }
        ensure!(
            frame.left.is_finite()
                && frame.right.is_finite()
                && (frame.left * gain).is_finite()
                && (frame.right * gain).is_finite(),
            "WAV export contains non-finite audio at frame {index}"
        );
        ensure!(
            format == WavSampleFormat::Float32
                || ((frame.left * gain).abs() <= 1.0 && (frame.right * gain).abs() <= 1.0),
            "WAV export would clip integer PCM at frame {index}; the destination was not changed"
        );
        if publication.is_some() {
            prewrite_peak = prewrite_peak.max(frame.left.abs()).max(frame.right.abs());
        }
    }
    if let Some(publication) = publication.as_deref_mut() {
        publication.prewrite_peak = prewrite_peak;
    }
    let channels = 2_u16;
    let bits_per_sample = format.bits();
    let bytes_per_sample = u32::from(bits_per_sample / 8);
    let data_size = format.data_size_for_frames(stereo.len())?;

    control.checkpoint(7500)?;
    policy.check(control)?;
    let (mut staged, file) = create_staged_output(path)?;
    // Keep the writer inside the closure: every error closes it before checked
    // abort attempts removal (also required by Windows file sharing semantics).
    let write_and_commit = || -> Result<()> {
        let mut writer = BufWriter::with_capacity(WAV_WRITE_BUFFER_BYTES, file);
        policy.check(control)?;
        #[cfg(test)]
        policy.operation(StagedOperation::Write)?;
        writer.write_all(b"RIFF")?;
        writer.write_all(&(format.riff_overhead() + data_size).to_le_bytes())?;
        writer.write_all(b"WAVEfmt ")?;
        let fmt_size = if format == WavSampleFormat::Float32 {
            18_u32
        } else {
            16_u32
        };
        writer.write_all(&fmt_size.to_le_bytes())?;
        writer.write_all(&format.format_tag().to_le_bytes())?;
        writer.write_all(&channels.to_le_bytes())?;
        writer.write_all(&sample_rate.to_le_bytes())?;
        let byte_rate = sample_rate * channels as u32 * bytes_per_sample;
        writer.write_all(&byte_rate.to_le_bytes())?;
        let block_align = channels * bytes_per_sample as u16;
        writer.write_all(&block_align.to_le_bytes())?;
        writer.write_all(&bits_per_sample.to_le_bytes())?;
        if format == WavSampleFormat::Float32 {
            writer.write_all(&0_u16.to_le_bytes())?; // cbSize, no extension
            writer.write_all(b"fact")?;
            writer.write_all(&4_u32.to_le_bytes())?;
            writer.write_all(&(stereo.len() as u32).to_le_bytes())?;
        }
        writer.write_all(b"data")?;
        writer.write_all(&data_size.to_le_bytes())?;
        for (index, frame) in stereo.iter().enumerate() {
            if index % PROGRESS_FRAME_INTERVAL == 0 {
                control.work_progress(index, stereo.len(), 7500, 9800)?;
                policy.check(control)?;
            }
            let mut encoded_frame = [0_u8; 8];
            for (channel, sample) in [frame.left, frame.right].into_iter().enumerate() {
                let value = sample * gain;
                let width = bytes_per_sample as usize;
                let offset = channel * width;
                match format {
                    WavSampleFormat::Pcm16 => {
                        let integer = (value * 32_767.0).round() as i16;
                        encoded_frame[offset..offset + width]
                            .copy_from_slice(&integer.to_le_bytes());
                    }
                    WavSampleFormat::Pcm24 => {
                        // Keep the legacy quantizer byte-for-byte, without adding dither.
                        let integer = (value * 8_388_607.0).round() as i32;
                        encoded_frame[offset..offset + width]
                            .copy_from_slice(&integer.to_le_bytes()[..3]);
                    }
                    WavSampleFormat::Float32 => {
                        // Floating point preserves finite headroom; never clamp to unity.
                        encoded_frame[offset..offset + width].copy_from_slice(&value.to_le_bytes());
                    }
                }
            }
            writer.write_all(&encoded_frame[..usize::from(block_align)])?;
        }
        control.checkpoint(9800)?;
        policy.check(control)?;
        #[cfg(test)]
        policy.operation(StagedOperation::Flush)?;
        writer
            .flush()
            .with_context(|| format!("Unable to flush staged WAV for {}", path.display()))?;
        control.check_cancelled()?;
        policy.check(control)?;
        #[cfg(test)]
        policy.operation(StagedOperation::Sync)?;
        writer
            .get_ref()
            .sync_all()
            .with_context(|| format!("Unable to sync staged WAV for {}", path.display()))?;
        if let Some(publication) = publication.as_deref_mut() {
            publication.identity = Some(PublishedFileIdentity::from_file(writer.get_ref())?);
        }
        drop(writer);
        #[cfg(windows)]
        if let Some(publication) = publication.as_deref_mut() {
            // Windows finalizes LastWriteTime when the write handle closes.
            // Reinspect the same file ID rather than sealing a provisional stamp.
            let finalized =
                PublishedFileIdentity::from_file(&native_file::open_unfollowed(&staged.path)?)?;
            let original = publication.identity.expect("staged producer identity");
            ensure!(
                (finalized.device, finalized.inode, finalized.bytes)
                    == (original.device, original.inode, original.bytes),
                "Staged occurrence file changed before publication"
            );
            publication.identity = Some(finalized);
        }
        // The cancellation/commit CAS makes the final boundary unambiguous even
        // if the UI requests Cancel concurrently with the worker's publication.
        policy.begin_commit(control)?;
        if let Some(publication) = publication.as_deref_mut() {
            publication.phase = PublicationPhase::LinkStarted;
            #[cfg(test)]
            {
                if policy.fixture.cancel_after_cas {
                    assert!(!control.cancel());
                }
                policy.operation(StagedOperation::Link)?;
            }
            #[cfg(not(windows))]
            std::fs::hard_link(&staged.path, &publication.destination)?;
            #[cfg(windows)]
            native_file::move_file(&staged.path, &publication.destination, false)?;
            // This assignment MUST be the first operation after successful link.
            // Paths and identity storage already exist outside the unwind scope.
            publication.phase = PublicationPhase::Linked;
            #[cfg(test)]
            {
                if policy.fixture.cancel_after_link {
                    assert!(!control.cancel());
                }
                policy.operation(StagedOperation::AfterLink)?;
            }
        } else {
            #[cfg(test)]
            policy.operation(StagedOperation::Rename)?;
            staged.commit(path)?;
        }
        Ok(())
    };
    let result = if policy.deadline.is_some() {
        // Keep `staged` alive outside the unwind boundary so even a panic has
        // one checked abort outcome, including a retained path on failure.
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(write_and_commit))
            .unwrap_or_else(|payload| Err(WavStagingPanic::from_payload(payload).into()))
    } else {
        write_and_commit()
    };
    if let Some(publication) = publication {
        if publication.phase == PublicationPhase::Linked {
            let finalization = publication.finish_link(&mut staged, &policy);
            // Preserve an immediately-post-link panic as primary, while keeping
            // both independent finalization errors in typed evidence.
            result?;
            finalization?;
            control.complete();
            return Ok(());
        }
        return match result {
            Err(original) => {
                let error = staged.abort_checked(original, &policy);
                if let Some(abort) = error.downcast_ref::<StagedOutputAbortFailure>() {
                    publication.retained_stage = Some(abort.retained_path.clone());
                    let mut evidence =
                        PublicationError::from_error(&error, PublicationOperation::Abort);
                    evidence.io_kind = Some(abort.removal_error.kind());
                    evidence.raw_os_error = abort.removal_error.raw_os_error();
                    evidence.kind = if abort
                        .removal_error
                        .get_ref()
                        .is_some_and(|inner| inner.is::<WavStagingPanic>())
                    {
                        PublicationErrorKind::Panic
                    } else {
                        PublicationErrorKind::Io
                    };
                    publication.abort = Some(evidence);
                }
                Err(error)
            }
            Ok(()) => unreachable!("create-new success requires a linked destination"),
        };
    }
    match result {
        Err(original) if policy.deadline.is_some() => Err(staged.abort_checked(original, &policy)),
        Ok(()) => {
            // No fallible operation remains after a successful rename.
            control.complete();
            Ok(())
        }
        other => other,
    }
}

struct StagedOutput {
    path: PathBuf,
    cleanup_on_drop: bool,
}

#[derive(Debug, thiserror::Error)]
#[error("WAV staging panicked: {message}")]
struct WavStagingPanic {
    message: String,
}

impl WavStagingPanic {
    fn from_payload(payload: Box<dyn std::any::Any + Send>) -> Self {
        let message = if let Some(message) = payload.downcast_ref::<String>() {
            message.clone()
        } else if let Some(message) = payload.downcast_ref::<&str>() {
            (*message).to_owned()
        } else {
            "non-string panic payload".to_owned()
        };
        Self { message }
    }
}

/// Attached as anyhow context, preserving downcasts to the original failure
/// while also exposing the failed removal and retained temporary path.
#[derive(Debug, thiserror::Error)]
#[error("Unable to remove staged WAV '{path}'; temporary file retained: {removal_error}", path = .retained_path.display())]
pub(crate) struct StagedOutputAbortFailure {
    pub(crate) retained_path: PathBuf,
    pub(crate) removal_error: std::io::Error,
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
        self.cleanup_on_drop = false;
        Ok(())
    }

    fn abort_checked(
        &mut self,
        original: anyhow::Error,
        _policy: &WavWritePolicy,
    ) -> anyhow::Error {
        // One visible cleanup result only. A hidden retry from Drop could erase
        // the retained-path evidence after reporting that removal failed.
        self.cleanup_on_drop = false;
        // Cleanup is independently fallible. Retain both the original error
        // and the owner/path even if removal itself unwinds.
        let removal =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> std::io::Result<()> {
                #[cfg(test)]
                _policy.operation(StagedOperation::Abort)?;
                std::fs::remove_file(&self.path)
            }))
            .unwrap_or_else(|payload| {
                Err(std::io::Error::other(WavStagingPanic::from_payload(
                    payload,
                )))
            });
        match removal {
            Ok(()) => original,
            // Already absent satisfies the cleanup postcondition.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => original,
            Err(removal_error) => original.context(StagedOutputAbortFailure {
                retained_path: self.path.clone(),
                removal_error,
            }),
        }
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
        if self.cleanup_on_drop {
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
                        cleanup_on_drop: true,
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
    use std::{collections::BTreeMap, time::Duration};

    pub(super) fn arrangement_project() -> Project {
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
            pattern_length_policy: None,
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

    pub(super) fn write_pcm16_wav(path: &Path, sample_rate: u32, channels: u16, samples: &[i16]) {
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
            pattern_length_policy: None,
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

    pub(super) fn audio_project(
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

    pub(super) fn test_plugin(id: u64) -> PluginInstance {
        PluginInstance {
            midi_ports: crate::plugin_midi_routing::PluginMidiPorts::default(),
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

    pub(super) fn workflow_test_directory(name: &str) -> PathBuf {
        let sequence = STAGED_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "citrus-export-{name}-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).unwrap();
        directory
    }

    fn checked_writer_with_fixture(
        path: &Path,
        control: &ExportControl,
        fixture: WavWriteFixture,
    ) -> Result<()> {
        let mut policy = WavWritePolicy::checked(Instant::now() + Duration::from_secs(60));
        policy.fixture = fixture;
        write_stereo_wav_core(
            path,
            WavExportOptions {
                sample_rate: 8000,
                sample_format: WavSampleFormat::Float32,
                level_policy: crate::export_options::WavLevelPolicy::PreserveLevel,
                tail_milliseconds: 0,
            },
            &[StereoFrame {
                left: 0.25,
                right: -0.5,
            }; 8193],
            1.0,
            control,
            policy,
        )
    }

    #[test]
    fn checked_float_writer_success_matches_legacy_bytes_at_unity_and_replaces_only_on_success() {
        let directory = workflow_test_directory("checked-float-success");
        let legacy = directory.join("legacy.wav");
        let destination = directory.join("checked.wav");
        let frames = [
            StereoFrame {
                left: -2.25,
                right: 3.5,
            },
            StereoFrame {
                left: f32::MAX,
                right: -f32::MAX,
            },
        ];
        write_stereo_wav_controlled(
            &legacy,
            WavExportOptions {
                sample_rate: 8000,
                sample_format: WavSampleFormat::Float32,
                level_policy: crate::export_options::WavLevelPolicy::PreserveLevel,
                tail_milliseconds: 0,
            },
            &frames,
            1.0,
            &ExportControl::default(),
        )
        .unwrap();
        // Both first publication and replacement use the real checked entry point.
        for replace_existing in [false, true] {
            if replace_existing {
                std::fs::write(&destination, b"original").unwrap();
            }
            let control = ExportControl::default();
            write_stereo_float32_checked(
                &destination,
                8000,
                &frames,
                &control,
                Instant::now() + Duration::from_secs(60),
            )
            .unwrap();
            assert_eq!(
                std::fs::read(&destination).unwrap(),
                std::fs::read(&legacy).unwrap()
            );
            assert_eq!(control.progress().basis_points, 10000);
            assert!(!control.cancel());
            assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 2);
        }
        let bytes = std::fs::read(&destination).unwrap();
        let samples = bytes[58..]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|sample| f32::from_le_bytes(*sample))
            .collect::<Vec<_>>();
        assert_eq!(samples, vec![-2.25, 3.5, f32::MAX, -f32::MAX]);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn checked_float_writer_rejects_early_cancel_expiry_and_invalid_audio_without_staging() {
        use crate::export_job::{ExportCancelled, ExportDeadlineExceeded};
        let directory = workflow_test_directory("checked-float-admission");
        let path = directory.join("original.wav");
        std::fs::write(&path, b"original").unwrap();
        for case in 0..4 {
            let control = ExportControl::default();
            let deadline = if case == 0 {
                Instant::now()
            } else {
                Instant::now() + Duration::from_secs(60)
            };
            if case == 1 {
                assert!(control.cancel());
            }
            let frame = StereoFrame {
                left: if case == 2 { f32::NAN } else { 0.25 },
                right: -0.5,
            };
            let error = write_stereo_float32_checked(
                &path,
                if case == 3 { 0 } else { 8000 },
                &[frame],
                &control,
                deadline,
            )
            .unwrap_err();
            match case {
                0 => assert!(error.is::<ExportDeadlineExceeded>()),
                1 => assert!(error.is::<ExportCancelled>()),
                2 => assert!(error.to_string().contains("non-finite")),
                _ => assert!(error.to_string().contains("sample rate")),
            }
            assert!(control.progress().basis_points < 10000);
            assert_eq!(std::fs::read(&path).unwrap(), b"original");
            assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn checked_float_writer_final_boundary_cancel_and_expiry_abort_before_publication() {
        use crate::export_job::{ExportCancelled, ExportDeadlineExceeded};
        let directory = workflow_test_directory("checked-float-boundary");
        let path = directory.join("original.wav");
        // A successful fixture control rules out an always-rejecting seam.
        checked_writer_with_fixture(&path, &ExportControl::default(), WavWriteFixture::default())
            .unwrap();
        for expiry in [false, true] {
            for fail_abort in [false, true] {
                std::fs::write(&path, b"original").unwrap();
                let control = ExportControl::default();
                let error = checked_writer_with_fixture(
                    &path,
                    &control,
                    WavWriteFixture {
                        fail_abort,
                        expire_before_commit: expiry,
                        cancel_before_commit: !expiry,
                        ..WavWriteFixture::default()
                    },
                )
                .unwrap_err();
                if expiry {
                    assert!(error.is::<ExportDeadlineExceeded>());
                    assert!(
                        control.cancel(),
                        "expired writer must not win the commit CAS"
                    );
                } else {
                    assert!(error.is::<ExportCancelled>());
                    assert!(control.progress().cancelling);
                }
                assert_eq!(control.progress().basis_points, 9900);
                assert_eq!(std::fs::read(&path).unwrap(), b"original");
                assert_checked_abort(&directory, &error, fail_abort);
            }
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    fn assert_checked_abort(directory: &Path, error: &anyhow::Error, failed: bool) {
        if failed {
            let abort = error.downcast_ref::<StagedOutputAbortFailure>().unwrap();
            assert_eq!(abort.retained_path.parent(), Some(directory));
            assert!(
                abort.retained_path.is_file(),
                "Drop must not retry a checked abort"
            );
            assert_eq!(
                abort.removal_error.to_string(),
                "injected staged Abort failure"
            );
            let message = format!("{error:#}");
            assert!(message.contains(&abort.retained_path.display().to_string()));
            assert!(message.contains("injected staged Abort failure"));
            assert_eq!(std::fs::read_dir(directory).unwrap().count(), 2);
            std::fs::remove_file(&abort.retained_path).unwrap();
        } else {
            assert!(error.downcast_ref::<StagedOutputAbortFailure>().is_none());
        }
        assert_eq!(std::fs::read_dir(directory).unwrap().count(), 1);
    }

    #[test]
    fn checked_float_writer_keeps_original_io_error_and_checked_abort_evidence() {
        let directory = workflow_test_directory("checked-float-io");
        let path = directory.join("original.wav");
        for operation in [
            StagedOperation::Write,
            StagedOperation::Flush,
            StagedOperation::Sync,
            StagedOperation::Rename,
        ] {
            for fail_abort in [false, true] {
                std::fs::write(&path, b"original").unwrap();
                let control = ExportControl::default();
                let error = checked_writer_with_fixture(
                    &path,
                    &control,
                    WavWriteFixture {
                        fail: Some(operation),
                        fail_abort,
                        ..WavWriteFixture::default()
                    },
                )
                .unwrap_err();
                let expected = format!("injected staged {operation:?} failure");
                assert_eq!(
                    error.downcast_ref::<std::io::Error>().unwrap().to_string(),
                    expected
                );
                assert!(format!("{error:#}").contains(&expected));
                assert!(control.progress().basis_points < 10000);
                assert_eq!(
                    control.progress().can_cancel,
                    operation != StagedOperation::Rename
                );
                assert_eq!(std::fs::read(&path).unwrap(), b"original");
                assert_checked_abort(&directory, &error, fail_abort);
            }
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn checked_float_writer_real_rename_failure_preserves_destination_and_removes_stage() {
        let directory = workflow_test_directory("checked-float-rename");
        let path = directory.join("existing-directory.wav");
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("original"), b"original").unwrap();
        let control = ExportControl::default();
        let error = write_stereo_float32_checked(
            &path,
            8000,
            &[StereoFrame::default()],
            &control,
            Instant::now() + Duration::from_secs(60),
        )
        .unwrap_err();
        assert!(error.to_string().contains("Unable to commit staged WAV"));
        assert!(error.downcast_ref::<std::io::Error>().is_some());
        assert_eq!(std::fs::read(path.join("original")).unwrap(), b"original");
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        assert_eq!(control.progress().basis_points, 9900);
        assert!(!control.cancel());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn checked_float_writer_panic_is_primary_and_abort_failure_retains_its_path() {
        let directory = workflow_test_directory("checked-float-panic");
        let path = directory.join("original.wav");
        for operation in [
            StagedOperation::Write,
            StagedOperation::Flush,
            StagedOperation::Sync,
            StagedOperation::Rename,
        ] {
            for fail_abort in [false, true] {
                std::fs::write(&path, b"original").unwrap();
                let control = ExportControl::default();
                let error = checked_writer_with_fixture(
                    &path,
                    &control,
                    WavWriteFixture {
                        panic_at: Some(operation),
                        fail_abort,
                        ..WavWriteFixture::default()
                    },
                )
                .unwrap_err();
                let expected = format!("injected staged {operation:?} panic");
                assert!(
                    error
                        .downcast_ref::<WavStagingPanic>()
                        .unwrap()
                        .message
                        .contains(&expected)
                );
                assert!(format!("{error:#}").contains(&expected));
                assert!(control.progress().basis_points < 10000);
                assert_eq!(std::fs::read(&path).unwrap(), b"original");
                assert_checked_abort(&directory, &error, fail_abort);
            }
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn checked_float_writer_abort_panic_preserves_original_failure_and_retained_path() {
        let directory = workflow_test_directory("checked-float-abort-panic");
        let path = directory.join("original.wav");
        for operation in [
            StagedOperation::Write,
            StagedOperation::Flush,
            StagedOperation::Sync,
            StagedOperation::Rename,
        ] {
            std::fs::write(&path, b"original").unwrap();
            let control = ExportControl::default();
            let error = checked_writer_with_fixture(
                &path,
                &control,
                WavWriteFixture {
                    fail: Some(operation),
                    panic_at: Some(StagedOperation::Abort),
                    ..WavWriteFixture::default()
                },
            )
            .unwrap_err();
            let expected = format!("injected staged {operation:?} failure");
            assert_eq!(
                error.downcast_ref::<std::io::Error>().unwrap().to_string(),
                expected
            );
            let abort = error.downcast_ref::<StagedOutputAbortFailure>().unwrap();
            let panic = abort
                .removal_error
                .get_ref()
                .unwrap()
                .downcast_ref::<WavStagingPanic>()
                .unwrap();
            assert!(panic.message.contains("injected staged Abort panic"));
            assert_eq!(abort.retained_path.parent(), Some(directory.as_path()));
            assert!(
                abort.retained_path.is_file(),
                "Drop must not retry a panicking abort"
            );
            let message = format!("{error:#}");
            assert!(message.contains(&expected));
            assert!(message.contains("injected staged Abort panic"));
            assert!(message.contains(&abort.retained_path.display().to_string()));
            assert_eq!(std::fs::read(&path).unwrap(), b"original");
            assert!(control.progress().basis_points < 10000);
            assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 2);
            std::fs::remove_file(&abort.retained_path).unwrap();
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn unsupported_targets_fail_preflight_without_replacing_destination() {
        let directory = workflow_test_directory("automation");
        let path = directory.join("existing.wav");
        let original = b"original destination bytes";
        std::fs::write(&path, original).unwrap();
        let targets = [
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
            .push(unsupported_lane(AutomationTarget::ChannelVolume {
                channel: 1,
            }));
        project.automation_lanes[0].lane.set_enabled(false);
        ensure_supported_automation(&project).unwrap();
        project.automation_lanes[0].lane.set_enabled(true);
        project.automation_lanes[0].lane.replace_points([]);
        ensure_supported_automation(&project).unwrap();
        project.automation_lanes[0] =
            unsupported_lane(AutomationTarget::ChannelVolume { channel: 1 });
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
            .push(unsupported_lane(AutomationTarget::ChannelPan {
                channel: 1,
            }));
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
    fn piano_period_overflow_export_events_repeat_at_bar_nine_with_fixed_clip_tail() {
        let mut project = arrangement_project();
        project.song_length_beats = 33.0;
        project.clips[0] = pattern_clip(0.0, 33.0, 1.0, false);
        project.patterns[0].channel_steps = vec![[false; 16]];
        project.patterns[0].notes = vec![
            PianoNote {
                id: 901,
                channel_id: Some(project.channels[0].id),
                group_id: None,
                note: 60,
                start: 0.0,
                length: 2.0,
                velocity: 1.0,
                selected: false,
                muted: false,
            },
            PianoNote {
                id: 902,
                channel_id: Some(project.channels[0].id),
                group_id: None,
                note: 72,
                start: 16.0,
                length: 1.0 / 64.0,
                velocity: 1.0,
                selected: false,
                muted: false,
            },
        ];
        let mut events = collected_events(&project, 128);
        events.sort_by_key(|event| event.sample);
        assert_eq!(
            events.iter().map(|event| event.sample).collect::<Vec<_>>(),
            vec![0, 1024, 2048]
        );
        assert_eq!(events[0].frequency, midi_frequency(60));
        assert_eq!(events[1].frequency, midi_frequency(72));
        assert_eq!(events[2].frequency, midi_frequency(60));
        assert_eq!(events[2].duration, 0.5);
        assert_eq!(project.clips[0].length, 33.0);
    }

    #[test]
    fn piano_period_overflow_builtin_wav_has_finite_audio_without_early_retrigger_overlap() {
        let mut project = arrangement_project();
        project.song_length_beats = 34.0;
        project.clips[0] = pattern_clip(0.0, 34.0, 1.0, false);
        project.patterns[0].channel_steps = vec![[false; 16]];
        project.patterns[0].notes = vec![
            PianoNote {
                id: 901,
                channel_id: Some(project.channels[0].id),
                group_id: None,
                note: 60,
                start: 0.0,
                length: 1.0,
                velocity: 1.0,
                selected: false,
                muted: false,
            },
            PianoNote {
                id: 902,
                channel_id: Some(project.channels[0].id),
                group_id: None,
                note: 72,
                start: 16.0,
                length: 0.25,
                velocity: 1.0,
                selected: false,
                muted: false,
            },
        ];
        let path = temporary_wav("piano-four-bar-cycle");
        let rendered = render_and_decode(&project, &path);
        assert_eq!(rendered.len(), 136_000);
        assert!(
            rendered
                .iter()
                .all(|frame| frame.left.is_finite() && frame.right.is_finite())
        );
        assert!(rendered[..200].iter().any(|frame| frame.left.abs() > 0.001));
        assert_eq!(
            &rendered[..200],
            &rendered[128_000..128_200],
            "the bar-nine retrigger must contain the original voice without an overlapping bar-five copy"
        );
        assert!(
            rendered[76_000..127_000]
                .iter()
                .all(|frame| frame.left == 0.0 && frame.right == 0.0)
        );
        std::fs::remove_file(path).unwrap();
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
    fn follow_pattern_wav_uses_derived_growth_shrink_and_modern_manual_clock_without_rewriting_midi()
     {
        let directory = workflow_test_directory("follow-pattern-wav");
        let mut project = arrangement_project();
        project.auto_song_length = true;
        project.clips[0] = pattern_clip(0.0, 1.0, 1.0, false);
        project.clips[0].pattern_length_policy = Some(crate::model::PatternLengthPolicy::Follow);
        project.patterns[0].channel_steps.fill([false; 16]);
        project.patterns[0].length_steps = 16;
        project.patterns[0].notes = vec![PianoNote {
            id: 901,
            channel_id: Some(project.channels[0].id),
            group_id: None,
            note: 64,
            start: 0.5,
            length: 0.25,
            velocity: 0.8,
            selected: false,
            muted: false,
        }];
        let short = render_and_decode(&project, &directory.join("short.wav"));
        assert_eq!(short.len(), 16_000);
        let mut later = project.patterns[0].notes[0].clone();
        later.id = 902;
        later.start = 6.0;
        project.patterns[0].notes.push(later);
        let before = serde_json::to_vec(&project).unwrap();
        let grown = render_and_decode(&project, &directory.join("grown.wav"));
        assert_eq!(grown.len(), 32_000);
        assert!(grown[24_000..26_000].iter().any(|s| s.left.abs() > 1.0e-5));
        assert!(
            grown
                .iter()
                .all(|s| s.left.is_finite() && s.right.is_finite())
        );
        assert_eq!(serde_json::to_vec(&project).unwrap(), before);
        let cancelled = ExportControl::default();
        assert!(cancelled.cancel());
        let protected = directory.join("keep.wav");
        std::fs::write(&protected, b"existing user export").unwrap();
        assert!(render_project_wav_controlled(&project, &protected, 8_000, &cancelled).is_err());
        assert_eq!(std::fs::read(&protected).unwrap(), b"existing user export");
        assert_eq!(serde_json::to_vec(&project).unwrap(), before);
        project.patterns[0].notes.pop();
        assert_eq!(
            render_and_decode(&project, &directory.join("shrunk.wav")).len(),
            16_000
        );
        project.clips[0].set_manual_pattern_length(8.0);
        assert_eq!(
            collected_events(&project, 8_000)
                .iter()
                .map(|e| e.sample)
                .collect::<Vec<_>>(),
            vec![2_000, 18_000],
            "a new manual trim retains the actual four-beat Pattern clock"
        );
        project.clips[0].pattern_length_policy = None;
        assert_eq!(
            collected_events(&project, 8_000)
                .iter()
                .map(|e| e.sample)
                .collect::<Vec<_>>(),
            vec![2_000],
            "an old Manual caller keeps the sixteen-beat legacy clock"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn auto_song_range_exports_late_pattern_audio_and_exact_arrangement_frame_count() {
        let directory = workflow_test_directory("auto-song-range");
        let path = directory.join("auto.wav");
        let mut project = arrangement_project();
        project.tempo = 120.0;
        project.song_length_beats = 2.0;
        project.auto_song_length = true;
        project.clips = vec![pattern_clip(8.0, 4.0, 1.0, false)];
        assert_eq!(project.song_end_beat(), 12.0);
        let rendered = render_and_decode(&project, &path);
        assert_eq!(rendered.len(), 48_000);
        assert!(
            rendered[..32_000]
                .iter()
                .all(|sample| sample.left == 0.0 && sample.right == 0.0)
        );
        assert!(
            rendered[32_000..]
                .iter()
                .any(|sample| sample.left.abs() > 1.0e-5)
        );
        project.auto_song_length = false;
        let manual_path = directory.join("manual.wav");
        let manual = render_and_decode(&project, &manual_path);
        assert_eq!(manual.len(), 8_000);
        assert!(
            manual
                .iter()
                .all(|sample| sample.left == 0.0 && sample.right == 0.0)
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn auto_song_range_keeps_unsupported_late_automation_as_a_real_export_failure() {
        let mut project = arrangement_project();
        project.song_length_beats = 2.0;
        project
            .automation_lanes
            .push(unsupported_lane(AutomationTarget::ChannelVolume {
                channel: 1,
            }));
        project.clips = vec![automation_placement(8.0, 4.0, false)];
        ensure_supported_automation(&project).unwrap();
        project.auto_song_length = true;
        assert_eq!(project.song_end_beat(), 12.0);
        let error = ensure_supported_automation(&project)
            .unwrap_err()
            .to_string();
        assert!(error.contains("Audible movement"), "{error}");
        assert!(error.contains("Realtime Master Capture"), "{error}");
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
        let audible_child_count = project
            .clips
            .iter()
            .filter(|clip| !clip.muted && f64::from(clip.start) < map.max_beat())
            .filter(|clip| {
                let start = map.beat_to_frame(f64::from(clip.start)).unwrap();
                let end = map
                    .beat_to_frame(audio_end_beat(clip, true).unwrap().min(map.max_beat()))
                    .unwrap();
                end > start
            })
            .count();
        assert_eq!(
            clips.len(),
            audible_child_count,
            "an in-range split child with fixture audio was not rendered"
        );
        assert!(
            clips
                .iter()
                .all(|clip| clip.output_start + clip.frame_count <= output.len()),
            "a prepared split child extends beyond the exact song frame count"
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

    fn rendered_exactly_silent_wav_bytes(
        project: &Project,
        output_rate: u32,
        files: &mut SplitExportFiles,
    ) -> Vec<u8> {
        let path = files.path("render-exact-silence", "wav");
        render_project_wav(project, &path, output_rate).unwrap();
        let decoded = wav::read_wav(&path).unwrap();
        assert_eq!(decoded.metadata.sample_rate, output_rate);
        assert_eq!(decoded.metadata.channels, 2);
        assert_eq!(decoded.metadata.bits_per_sample, 24);
        assert!(!decoded.samples.is_empty());
        assert!(decoded.samples.iter().all(|sample| *sample == 0.0));
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
            // Normalize before the baseline so load-time clip normalization
            // cannot mask a split regression. The manual song crop stays short.
            project.normalize();
            assert!(!project.auto_song_length);
            assert_eq!(project.song_end_beat(), 0.65);
            let expected_pcm = prepared_audio_pcm(&project, output_rate);
            let expected_wav = rendered_wav_bytes(&project, output_rate, &mut files);
            let song_frames = TempoMap::from_project(&project, output_rate)
                .unwrap()
                .duration_frames() as usize;
            assert_eq!(expected_pcm.len(), song_frames);
            assert_eq!(expected_wav.len(), 44 + song_frames * 2 * 3);
            let start = project.clips[0].start;
            let first_cut = start + 0.75;
            assert!(first_cut > project.song_end_beat());
            let right_id = split_export_clip(&mut project, 70, first_cut, 48_000);
            split_export_clip(&mut project, 70, start + 0.25, 48_000);
            split_export_clip(&mut project, right_id, start + 1.25, 48_000);
            assert_eq!(project.clips.len(), 4);
            assert_eq!(project.song_end_beat(), 0.65);
            assert_eq!(
                project
                    .clips
                    .iter()
                    .filter(|clip| clip.start >= project.song_end_beat())
                    .count(),
                2,
                "tail edits must keep both out-of-song children in the project"
            );
            let path = files.path("v11", "citrus");
            project.save(&path).unwrap();
            let restored = Project::load(&path).unwrap();
            assert_eq!(
                restored.format_version,
                crate::model::CURRENT_PROJECT_FORMAT_VERSION
            );
            assert_eq!(restored.clips.len(), 4);
            assert!(!restored.auto_song_length);
            assert_eq!(restored.song_end_beat(), 0.65);
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
            let moved_map = TempoMap::from_project(&moved, output_rate).unwrap();
            let moved_clip = &moved.clips[0];
            // This child originally started after the manual song crop. Its
            // inherited fade-out still ends before beat zero after the move.
            assert!(
                moved_clip
                    .fade_out_reference
                    .unwrap()
                    .end_limit_beats
                    .unwrap()
                    < 0.0
            );
            let moved_fades = compile_clip_fades(
                moved_clip.start,
                moved_clip.length,
                moved_clip.audio_length_reference,
                moved_clip.fade_in,
                moved_clip.fade_out,
                moved_clip.fade_in_reference,
                moved_clip.fade_out_reference,
                &moved_map,
                true,
            )
            .unwrap();
            assert!(moved_fades.fade_out_end < 0);
            assert!(moved_fades.fade_out_frames > 0);
            let moved_pcm = prepared_audio_pcm(&moved, output_rate);
            assert!(
                moved_pcm
                    .iter()
                    .all(|frame| frame.left == 0.0 && frame.right == 0.0)
            );
            let moved_wav = rendered_exactly_silent_wav_bytes(&moved, output_rate, &mut files);
            assert_eq!(moved.song_end_beat(), 0.65);
            assert_eq!(moved_pcm.len(), moved_map.duration_frames() as usize);
            assert_eq!(moved_wav.len(), 44 + moved_pcm.len() * 2 * 3);

            // Resetting only fade-out must expose the same real source audio;
            // the silent result above must not hide a broken source route.
            let mut audible = moved.clone();
            let mut audible_payload = serde_json::to_value(&moved).unwrap();
            audible_payload["clips"][0]["fade_out"] = serde_json::json!(0.0);
            audible_payload["clips"][0]
                .as_object_mut()
                .unwrap()
                .remove("fade_out_reference");
            crate::clip_fade::set_clip_fade(&mut audible.clips[0], false, 0.0);
            assert_eq!(serde_json::to_value(&audible).unwrap(), audible_payload);
            assert_eq!(audible.song_end_beat(), 0.65);
            let audible_pcm = prepared_audio_pcm(&audible, output_rate);
            assert_eq!(audible_pcm.len(), moved_pcm.len());
            assert!(
                audible_pcm
                    .iter()
                    .any(|frame| frame.left.abs() > 0.01 || frame.right.abs() > 0.01)
            );
            let audible_wav = rendered_wav_bytes(&audible, output_rate, &mut files);
            assert_eq!(audible_wav.len(), moved_wav.len());

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
            assert!(!reloaded.auto_song_length);
            assert_eq!(reloaded.song_end_beat(), 0.65);
            let case = format!("v11 moved/retempo save/load {source_rate}->{output_rate}");
            assert_audio_pcm_bits_equal(
                &moved_pcm,
                &prepared_audio_pcm(&reloaded, output_rate),
                &case,
            );
            assert_eq!(
                rendered_exactly_silent_wav_bytes(&reloaded, output_rate, &mut files),
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
                assert!(!project.auto_song_length);
                assert_eq!(project.song_end_beat(), 0.65);
                let expected_pcm = prepared_audio_pcm(&project, output_rate);
                let expected_wav = rendered_wav_bytes(&project, output_rate, &mut files);
                // Independently preserve the v10 endpoint arithmetic and
                // original equal-power envelope, rather than comparing only
                // two invocations of the new reference compiler.
                let clip = &project.clips[0];
                let map = TempoMap::from_project(&project, output_rate).unwrap();
                let song_end = map.max_beat() as f32;
                assert!(clip.start + clip.length > song_end);
                assert_eq!(expected_pcm.len(), map.duration_frames() as usize);
                assert_eq!(expected_wav.len(), 44 + expected_pcm.len() * 2 * 3);
                let start = tempo_frame(&map, clip.start).unwrap();
                // Retain each original v10 f32 geometry expression, then crop
                // that endpoint to the explicit manual song window.
                let end = tempo_frame(&map, (clip.start + clip.length).min(song_end)).unwrap();
                let fade_in_end = tempo_frame(
                    &map,
                    (clip.start + clip.length * clip.fade_in).min(song_end),
                )
                .unwrap();
                let fade_out_start = tempo_frame(
                    &map,
                    (clip.start + clip.length * (1.0 - clip.fade_out)).min(song_end),
                )
                .unwrap();
                assert_eq!(end, expected_pcm.len());
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
                assert!(!restored.auto_song_length);
                assert_eq!(restored.song_end_beat(), 0.65);
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

    #[test]
    fn each_wav_format_has_exact_headers_frames_and_production_reader_round_trip() {
        let directory = workflow_test_directory("formats");
        let path = directory.join("format.wav");
        let frames = [
            StereoFrame {
                left: -1.0,
                right: 1.0,
            },
            StereoFrame {
                left: -0.5,
                right: 0.25,
            },
            StereoFrame::default(),
        ];
        for sample_format in WavSampleFormat::ALL {
            for sample_rate in [8000, 44100, 48000, 88200, 96000, 176400, 192000] {
                let options = WavExportOptions {
                    sample_rate,
                    sample_format,
                    ..WavExportOptions::default()
                };
                write_stereo_wav_controlled(
                    &path,
                    options,
                    &frames,
                    1.0,
                    &ExportControl::default(),
                )
                .unwrap();
                let bytes = std::fs::read(&path).unwrap();
                let data_offset = sample_format.riff_overhead() as usize + 8;
                let data_len = frames.len() * usize::from(sample_format.block_align());
                assert_eq!(&bytes[..4], b"RIFF");
                assert_eq!(
                    u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize,
                    bytes.len() - 8
                );
                assert_eq!(&bytes[8..16], b"WAVEfmt ");
                assert_eq!(
                    u16::from_le_bytes(bytes[20..22].try_into().unwrap()),
                    sample_format.format_tag()
                );
                assert_eq!(u16::from_le_bytes(bytes[22..24].try_into().unwrap()), 2);
                assert_eq!(
                    u32::from_le_bytes(bytes[24..28].try_into().unwrap()),
                    sample_rate
                );
                assert_eq!(
                    u32::from_le_bytes(bytes[28..32].try_into().unwrap()),
                    sample_rate * u32::from(sample_format.block_align())
                );
                assert_eq!(
                    u16::from_le_bytes(bytes[32..34].try_into().unwrap()),
                    sample_format.block_align()
                );
                assert_eq!(
                    u16::from_le_bytes(bytes[34..36].try_into().unwrap()),
                    sample_format.bits()
                );
                assert_eq!(&bytes[data_offset - 8..data_offset - 4], b"data");
                assert_eq!(
                    u32::from_le_bytes(bytes[data_offset - 4..data_offset].try_into().unwrap())
                        as usize,
                    data_len
                );
                assert_eq!(bytes.len(), data_offset + data_len);
                if sample_format == WavSampleFormat::Float32 {
                    assert_eq!(u32::from_le_bytes(bytes[16..20].try_into().unwrap()), 18);
                    assert_eq!(&bytes[36..38], &[0, 0]);
                    assert_eq!(&bytes[38..42], b"fact");
                    assert_eq!(u32::from_le_bytes(bytes[42..46].try_into().unwrap()), 4);
                    assert_eq!(u32::from_le_bytes(bytes[46..50].try_into().unwrap()), 3);
                } else {
                    assert_eq!(u32::from_le_bytes(bytes[16..20].try_into().unwrap()), 16);
                }
                let decoded = wav::read_wav(&path).unwrap();
                assert_eq!(decoded.metadata.frames, 3);
                assert_eq!(decoded.metadata.bits_per_sample, sample_format.bits());
                assert_eq!(decoded.metadata.block_align, sample_format.block_align());
                assert_eq!(decoded.metadata.sample_rate, sample_rate);
                assert_eq!(
                    decoded.metadata.encoding,
                    if sample_format == WavSampleFormat::Float32 {
                        wav::WavSampleEncoding::IeeeFloat
                    } else {
                        wav::WavSampleEncoding::PcmInteger
                    }
                );
                let tolerance = if sample_format == WavSampleFormat::Pcm16 {
                    1.0 / 32767.0
                } else {
                    1.0e-6
                };
                for (actual, expected) in decoded
                    .samples
                    .iter()
                    .zip(frames.iter().flat_map(|f| [f.left, f.right]))
                {
                    assert!(
                        (actual - expected).abs() <= tolerance,
                        "{sample_format:?}: {actual} != {expected}"
                    );
                }
            }
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn explicit_formats_render_exact_tempo_map_duration_at_each_rate() {
        let directory = workflow_test_directory("format-duration");
        let path = directory.join("render.wav");
        let mut project = arrangement_project();
        project.tempo = 123.0;
        project.song_length_beats = 0.25;
        project.clips[0].muted = true;
        for sample_format in WavSampleFormat::ALL {
            for sample_rate in WavExportOptions::STANDARD_SAMPLE_RATES {
                let options = WavExportOptions {
                    sample_rate,
                    sample_format,
                    ..WavExportOptions::default()
                };
                let report = render_project_wav_with_options(
                    &project,
                    &path,
                    options,
                    &ExportControl::default(),
                )
                .unwrap();
                let expected = TempoMap::from_project(&project, sample_rate)
                    .unwrap()
                    .duration_frames();
                let decoded = wav::read_wav(&path).unwrap();
                assert_eq!(report.frames as u64, expected);
                assert_eq!(decoded.metadata.frames, expected);
                assert_eq!(decoded.metadata.sample_rate, sample_rate);
                assert!(decoded.samples.iter().all(|sample| *sample == 0.0));
                assert_eq!(report.applied_gain, 1.0);
            }
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn level_policy_is_explicit_and_preserve_pcm_overload_keeps_original() {
        use crate::export_options::WavLevelPolicy;
        let directory = workflow_test_directory("level-policy");
        let source = directory.join("source.wav");
        let path = directory.join("result.wav");
        write_pcm16_wav(&source, 8000, 1, &[16384; 8]);
        let mut project = audio_project(&source, 8000, 1, 8);
        project.clips[0].gain = 4.0;
        for sample_format in WavSampleFormat::ALL {
            let mut options = WavExportOptions {
                sample_rate: 8000,
                sample_format,
                level_policy: WavLevelPolicy::PreserveLevel,
                tail_milliseconds: 0,
            };
            std::fs::write(&path, b"original").unwrap();
            let result = render_project_wav_with_options(
                &project,
                &path,
                options,
                &ExportControl::default(),
            );
            if sample_format == WavSampleFormat::Float32 {
                let report = result.unwrap();
                assert_eq!(report.applied_gain, 1.0);
                assert_eq!(report.sample_peak, 2.0);
                let bytes = std::fs::read(&path).unwrap();
                assert_eq!(f32::from_le_bytes(bytes[58..62].try_into().unwrap()), 2.0);
                // Existing import contract deliberately clamps float headroom.
                assert_eq!(wav::read_wav(&path).unwrap().samples[0], 1.0);
            } else {
                let error = result.unwrap_err().to_string();
                assert!(error.contains("Preserve level would clip"));
                assert!(error.contains("32-bit IEEE float"));
                assert!(error.contains("peak attenuation"));
                assert_eq!(std::fs::read(&path).unwrap(), b"original");
            }
            options.level_policy = WavLevelPolicy::AttenuatePeaks;
            let report = render_project_wav_with_options(
                &project,
                &path,
                options,
                &ExportControl::default(),
            )
            .unwrap();
            assert_eq!(report.sample_peak, 2.0);
            assert_eq!(report.applied_gain, 0.475);
            assert!(report.level_summary().contains("Peak attenuation applied"));
            let decoded = wav::read_wav(&path).unwrap();
            assert!((decoded.samples[0] - 0.95).abs() < 4.0e-5);
            assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 2);
        }
        project.clips[0].gain = 1.0;
        let report = render_project_wav_with_options(
            &project,
            &path,
            WavExportOptions::legacy(8000),
            &ExportControl::default(),
        )
        .unwrap();
        assert_eq!(report.sample_peak, 0.5);
        assert_eq!(report.applied_gain, 1.0);
        assert!(report.level_summary().contains("Unity export gain"));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn float_writer_preserves_finite_negative_and_positive_headroom_without_clamping() {
        let directory = workflow_test_directory("float-headroom");
        let path = directory.join("float.wav");
        let options = WavExportOptions {
            sample_format: WavSampleFormat::Float32,
            ..WavExportOptions::default()
        };
        let frames = [
            StereoFrame {
                left: -2.25,
                right: 3.5,
            },
            StereoFrame {
                left: f32::MAX,
                right: -f32::MAX,
            },
        ];
        write_stereo_wav_controlled(&path, options, &frames, 1.0, &ExportControl::default())
            .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let samples = bytes[58..]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|sample| f32::from_le_bytes(*sample))
            .collect::<Vec<_>>();
        assert_eq!(samples, vec![-2.25, 3.5, f32::MAX, -f32::MAX]);
        assert_eq!(
            wav::read_wav(&path).unwrap().samples,
            vec![-1.0, 1.0, 1.0, -1.0]
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn all_formats_reject_non_finite_samples_gain_and_invalid_rates_before_staging() {
        let directory = workflow_test_directory("formats-invalid");
        let path = directory.join("original.wav");
        std::fs::write(&path, b"original").unwrap();
        for sample_format in WavSampleFormat::ALL {
            let options = WavExportOptions {
                sample_format,
                ..WavExportOptions::default()
            };
            for (left, right, gain) in [
                (f32::NAN, 0.0, 1.0),
                (0.0, f32::INFINITY, 1.0),
                (f32::NEG_INFINITY, 0.0, 1.0),
                (0.25, 0.0, f32::NAN),
                (f32::MAX, 0.0, 2.0),
            ] {
                assert!(
                    write_stereo_wav_controlled(
                        &path,
                        options,
                        &[StereoFrame { left, right }],
                        gain,
                        &ExportControl::default()
                    )
                    .is_err()
                );
                assert_eq!(std::fs::read(&path).unwrap(), b"original");
                assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
            }
            for sample_rate in [0, 7999, 192001, u32::MAX] {
                assert!(
                    render_project_wav_with_options(
                        &arrangement_project(),
                        &path,
                        WavExportOptions {
                            sample_rate,
                            ..options
                        },
                        &ExportControl::default()
                    )
                    .is_err()
                );
                assert_eq!(std::fs::read(&path).unwrap(), b"original");
            }
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn all_format_cancellation_phases_preserve_original_and_clean_staging() {
        let directory = workflow_test_directory("formats-cancel");
        let source = directory.join("source.wav");
        let path = directory.join("result.wav");
        write_pcm16_wav(&source, 8000, 1, &vec![8192; 16000]);
        let mut project = audio_project(&source, 8000, 1, 16000);
        project.song_length_beats = 2.0;
        project.clips[0].length = 2.0;
        for sample_format in WavSampleFormat::ALL {
            for level_policy in [
                crate::export_options::WavLevelPolicy::AttenuatePeaks,
                crate::export_options::WavLevelPolicy::PreserveLevel,
            ] {
                for threshold in [
                    1, 1000, 1500, 2000, 5000, 6000, 6800, 7200, 8000, 9800, 9900,
                ] {
                    std::fs::write(&path, b"original").unwrap();
                    let options = WavExportOptions {
                        sample_rate: 8000,
                        sample_format,
                        level_policy,
                        tail_milliseconds: 0,
                    };
                    let control = ExportControl::default();
                    control.cancel_at(threshold);
                    let error = render_project_wav_with_options(&project, &path, options, &control)
                        .unwrap_err();
                    assert!(
                        error
                            .downcast_ref::<crate::export_job::ExportCancelled>()
                            .is_some(),
                        "{sample_format:?}/{threshold}: {error:#}"
                    );
                    assert_eq!(std::fs::read(&path).unwrap(), b"original");
                    assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 2);
                    assert!(control.progress().basis_points < 10000);
                }
            }
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn every_format_failed_commit_keeps_destination_and_cleans_stage() {
        let directory = workflow_test_directory("formats-commit");
        let path = directory.join("directory-not-a-file.wav");
        std::fs::create_dir(&path).unwrap();
        let marker = path.join("keep");
        std::fs::write(&marker, b"original").unwrap();
        for sample_format in WavSampleFormat::ALL {
            let options = WavExportOptions {
                sample_format,
                ..WavExportOptions::default()
            };
            let control = ExportControl::default();
            let error = write_stereo_wav_controlled(
                &path,
                options,
                &[StereoFrame::default()],
                1.0,
                &control,
            )
            .unwrap_err();
            assert!(
                error
                    .downcast_ref::<crate::export_job::ExportCancelled>()
                    .is_none()
            );
            assert!(error.to_string().contains("Unable to commit"));
            assert_eq!(std::fs::read(&marker).unwrap(), b"original");
            assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn default_pcm24_output_matches_pre_options_golden_file_exactly() {
        // Generated by the unmodified bcf0c7d renderer using the same source,
        // project and rate below; includes legacy peak attenuation and silence.
        let directory = workflow_test_directory("legacy-golden");
        let source = directory.join("source.wav");
        let path = directory.join("render.wav");
        write_pcm16_wav(&source, 8000, 1, &[-16384, 8192, 24576, -32768, 32767]);
        let mut project = audio_project(&source, 8000, 1, 5);
        project.clips[0].gain = 1.5;
        let expected = include_bytes!("../tests/fixtures/export-legacy-pcm24.wav");
        render_project_wav(&project, &path, 8000).unwrap();
        assert_eq!(&std::fs::read(&path).unwrap(), expected);
        render_project_wav_with_options(
            &project,
            &path,
            WavExportOptions::legacy(8000),
            &ExportControl::default(),
        )
        .unwrap();
        assert_eq!(&std::fs::read(&path).unwrap(), expected);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn options_keep_plugin_sidechain_and_automation_guards_for_every_encoding() {
        let directory = workflow_test_directory("format-guards");
        let path = directory.join("original.wav");
        std::fs::write(&path, b"original").unwrap();
        let mut instrument = mixer_test_project();
        instrument.plugin_instances.push(test_plugin(700));
        instrument.channels[0].instrument_plugin_instance_id = Some(700);
        let mut insert = mixer_test_project();
        insert.plugin_instances.push(test_plugin(700));
        insert.mixer_insert_slots.push(MixerInsertSlotRef {
            track: 1,
            slot: 0,
            plugin_instance_id: 700,
        });
        let mut sidechain = mixer_test_project();
        sidechain.mixer_routes.push(ProjectMixerRoute {
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
        let mut automation = arrangement_project();
        automation
            .automation_lanes
            .push(unsupported_lane(AutomationTarget::ChannelVolume {
                channel: 1,
            }));
        for (project, reason) in [
            (instrument, "active instrument"),
            (insert, "active mixer"),
            (sidechain, "sidechain"),
            (
                automation,
                "Only Tempo, Master Volume, Master Pan and Master Mute",
            ),
        ] {
            assert!(
                format!("{:#}", ensure_supported_project(&project).unwrap_err()).contains(reason)
            );
            for sample_format in WavSampleFormat::ALL {
                let options = WavExportOptions {
                    sample_rate: 8000,
                    sample_format,
                    ..WavExportOptions::default()
                };
                let error = render_project_wav_with_options(
                    &project,
                    &path,
                    options,
                    &ExportControl::default(),
                )
                .unwrap_err();
                assert!(format!("{error:#}").contains(reason));
                assert_eq!(std::fs::read(&path).unwrap(), b"original");
                assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
            }
        }
        std::fs::remove_dir_all(directory).unwrap();
    }
}

#[cfg(test)]
#[path = "export_occurrence_tests.rs"]
mod occurrence_publication_tests;
