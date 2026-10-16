//! The first nonlinear Whole path: one static post-fader route to Master.
//! A slot endpoint remains a real stage even when all its slots are bypassed.
use super::*;
use crate::model::{MixerInsertSlotRef, PluginFormat};
#[cfg(test)]
#[path = "export_serial_master_tests.rs"]
mod tests;

#[derive(Clone, Debug)]
pub(crate) struct SerialMasterNode {
    pub track_id: MixerTrackId,
    pub runtime_track: usize,
    pub placements: Vec<MixerInsertSlotRef>,
    pub outgoing_gain: f32,
    pub is_master: bool,
    pub volume: f32,
    pub pan_left: f32,
    pub pan_right: f32,
    pub audible: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct StaticSerialMasterPlan {
    pub nodes: Vec<SerialMasterNode>,
    pub channel_volume: f32,
    pub channel_pan_left: f32,
    pub channel_pan_right: f32,
    pub channel_audible: bool,
}

impl StaticSerialMasterPlan {
    pub(crate) fn prepare(project: &Project, channel_id: u32) -> Result<Self> {
        ensure!(
            !project.mixer_insert_slots.is_empty(),
            "Serial Whole requires actual Insert placements"
        );
        ensure!(
            project
                .automation_lanes
                .iter()
                .all(|lane| !lane.lane.is_enabled() || lane.lane.points().is_empty()),
            "Static serial Whole does not yet support active automation"
        );
        let graph = compile_mixer_graph(project)?;
        let mut channels = project.channels.iter().filter(|c| c.id == channel_id);
        let channel = channels
            .next()
            .context("Serial Whole source channel is missing")?;
        ensure!(
            channels.next().is_none(),
            "Serial Whole source channel identity is duplicated"
        );
        ensure!(
            channel.volume.is_finite() && channel.pan.is_finite(),
            "Serial Whole channel controls must be finite"
        );
        let channel_gate =
            !channel.muted && (!project.channels.iter().any(|c| c.solo) || channel.solo);
        let (cl, cr) = pan_balance(channel.pan);
        let channel_left_gain = if channel_gate {
            channel.volume * cl
        } else {
            0.0
        };
        let channel_right_gain = if channel_gate {
            channel.volume * cr
        } else {
            0.0
        };
        let mut seed = [false; MIXER_GRAPH_MAX_NODES];
        for node in graph.nodes() {
            seed[usize::from(node.runtime_slot)] =
                node.id != MASTER_MIXER_TRACK_ID && project.mixer_tracks[node.project_index].solo;
        }
        let any_solo = seed.iter().any(|v| *v);
        let mut ancestors = seed;
        let mut descendants = seed;
        for dense in graph.topological_order().iter().copied() {
            let slot = usize::from(graph.nodes()[usize::from(dense)].runtime_slot);
            if descendants[slot] {
                for route in graph.routes().iter().filter(|r| r.source_dense == dense) {
                    descendants[usize::from(route.destination_runtime_slot)] = true;
                }
            }
        }
        for dense in graph.topological_order().iter().copied().rev() {
            let slot = usize::from(graph.nodes()[usize::from(dense)].runtime_slot);
            if graph
                .routes()
                .iter()
                .filter(|r| r.source_dense == dense)
                .any(|r| ancestors[usize::from(r.destination_runtime_slot)])
            {
                ancestors[slot] = true;
            }
        }
        let mut nodes = Vec::new();
        let mut id = channel.mixer_track;
        loop {
            ensure!(
                nodes.len() < MIXER_GRAPH_MAX_NODES,
                "Serial Whole path is cyclic or too long"
            );
            let node = graph
                .node_by_id(id)
                .context("Serial Whole source route is missing")?;
            let track = &project.mixer_tracks[node.project_index];
            ensure!(
                track.volume.is_finite() && track.pan.is_finite(),
                "Serial Whole mixer controls must be finite"
            );
            let is_master = id == MASTER_MIXER_TRACK_ID;
            if is_master {
                ensure!(
                    (0.0..=1.2).contains(&track.volume),
                    "Whole-song master base volume must match the realtime range 0..1.2"
                );
            }
            ensure!(
                is_master || (0.0..=1.5).contains(&track.volume),
                "Serial Whole track volume must match the realtime range 0..1.5"
            );
            let gate = !track.muted
                && (!any_solo
                    || ancestors[usize::from(node.runtime_slot)]
                    || descendants[usize::from(node.runtime_slot)]);
            let (l, r) = pan_balance(track.pan);
            let mut placements: Vec<_> = project
                .mixer_insert_slots
                .iter()
                .filter(|p| p.track == id)
                .cloned()
                .collect();
            placements.sort_by_key(|p| p.slot);
            ensure!(
                placements.windows(2).all(|p| p[0].slot != p[1].slot),
                "Serial Whole has duplicate Insert slot placements"
            );
            for placement in &placements {
                ensure!(
                    project
                        .mixer_insert_slots
                        .iter()
                        .filter(|p| p.plugin_instance_id == placement.plugin_instance_id)
                        .count()
                        == 1,
                    "Serial Whole requires unique physical Insert instance identities"
                );
                let mut instances = project
                    .plugin_instances
                    .iter()
                    .filter(|p| p.id == placement.plugin_instance_id);
                let instance = instances
                    .next()
                    .context("Serial Whole Insert instance is missing")?;
                ensure!(
                    instances.next().is_none() && instance.format == PluginFormat::Vst3,
                    "Serial Whole requires unambiguous VST3 Insert identities"
                );
                ensure!(
                    instance.wet.is_finite() && (0.0..=1.0).contains(&instance.wet),
                    "Serial Whole Insert wet value is invalid"
                );
                ensure!(
                    !instance.enabled || instance.bypass || instance.wet == 1.0,
                    "Static serial Whole requires fully wet active Inserts; partial wet has an unaligned dry path"
                );
                ensure!(
                    Some(instance.id) != channel.instrument_plugin_instance_id,
                    "Generator cannot also be an Insert"
                );
            }
            let outgoing: Vec<_> = graph
                .routes()
                .iter()
                .filter(|r| r.source_id == id)
                .collect();
            let (outgoing_gain, next) = if is_master {
                ensure!(
                    outgoing.is_empty(),
                    "Master cannot send to another serial stage"
                );
                (1.0, None)
            } else {
                ensure!(
                    outgoing.len() == 1 && outgoing[0].tap == MixerRouteTap::PostFader,
                    "Static serial Whole requires one post-fader main route per participating track; branches and sends are unsupported"
                );
                (outgoing[0].gain, Some(outgoing[0].destination_id))
            };
            nodes.push(SerialMasterNode {
                track_id: id,
                runtime_track: usize::from(node.runtime_slot),
                placements,
                outgoing_gain,
                is_master,
                volume: track.volume,
                pan_left: l,
                pan_right: r,
                audible: gate,
            });
            match next {
                Some(next) => id = next,
                None => break,
            }
        }
        ensure!(
            project
                .mixer_insert_slots
                .iter()
                .all(|p| nodes.iter().any(|n| n.track_id == p.track)),
            "Static serial Whole does not support off-path Inserts, including disabled or bypassed endpoints"
        );
        ensure!(
            channel_left_gain.is_finite()
                && channel_right_gain.is_finite()
                && nodes
                    .iter()
                    .all(|n| (n.volume * n.pan_left * n.outgoing_gain).is_finite()
                        && (n.volume * n.pan_right * n.outgoing_gain).is_finite()),
            "Serial Whole routing gains overflow"
        );
        Ok(Self {
            nodes,
            channel_volume: channel.volume,
            channel_pan_left: cl,
            channel_pan_right: cr,
            channel_audible: channel_gate,
        })
    }

    pub(crate) fn publish(
        self,
        mut stereo: Vec<StereoFrame>,
        options: WavExportOptions,
        path: &Path,
        reported_tail: u32,
        control: &ExportControl,
        deadline: Instant,
    ) -> Result<WavExportReport> {
        let tail_frames = options.tail_frames()?;
        let music_frames = stereo
            .len()
            .checked_sub(tail_frames)
            .filter(|n| *n != 0)
            .context("Serial Whole PCM does not span a positive N+tail")?;
        let master = self
            .nodes
            .last()
            .filter(|n| n.is_master)
            .context("Serial Whole Master stage is absent")?;
        let mut peak = 0.0_f32;
        for (block, samples) in stereo.chunks_mut(PROGRESS_FRAME_INTERVAL).enumerate() {
            control.check_deadline(deadline)?;
            control.work_progress(
                block * PROGRESS_FRAME_INTERVAL,
                music_frames + tail_frames,
                6_000,
                7_000,
            )?;
            for sample in samples {
                ensure!(
                    sample.left.is_finite() && sample.right.is_finite(),
                    "Serial Whole contains non-finite PCM"
                );
                let left = if master.audible {
                    sample.left * master.volume * master.pan_left
                } else {
                    0.0
                };
                let right = if master.audible {
                    sample.right * master.volume * master.pan_right
                } else {
                    0.0
                };
                ensure!(
                    left.is_finite() && right.is_finite(),
                    "Serial Whole controls overflow"
                );
                sample.left = left.tanh();
                sample.right = right.tanh();
                peak = peak.max(sample.left.abs()).max(sample.right.abs());
            }
        }
        let gain = options.gain_for_peak(peak)?;
        write_stereo_wav_core(
            path,
            options,
            &stereo,
            gain,
            control,
            WavWritePolicy::checked(deadline),
        )?;
        Ok(WavExportReport {
            sample_rate: options.sample_rate,
            frames: stereo.len(),
            music_frames,
            tail_frames,
            tail_limit_reached: u64::from(reported_tail) > tail_frames as u64,
            sample_peak: peak,
            applied_gain: gain,
        })
    }
}
