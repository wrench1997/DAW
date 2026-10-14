//! Offline master controls using the same compiled layers and sample kernel as
//! the callback. This helper does not qualify other automation or any plug-in.
//! The caller strips only driven controls from the static mixer transfer and
//! invokes this after mixing source audio, before level measurement/encoding.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, ensure};

use crate::{
    automation::{AutomationTarget, canonicalize_automation_target},
    export::StereoFrame,
    export_job::ExportControl,
    mixer_graph::MASTER_MIXER_TRACK_ID,
    model::{ClipKind, Project},
    tempo_map::TempoMap,
    timeline::{
        ChasedAudioClip, ChasedNote, CompiledAutomationTarget, CompiledTimeline,
        TIMELINE_CALLBACK_MAX_EVENTS, TimelineChaseOptions, TimelineCompileOptions, TimelinePacket,
    },
    timeline_automation::{
        RealtimeTimelineAutomation, TimelineAutomationBlockPlan, TimelineAutomationError,
    },
    timeline_executor::{
        TimelineAudioSink, TimelineAutomationBlockEndpoint, TimelineAutomationChaseValue,
        TimelineAutomationTransition, TimelineExecutor,
    },
};

const CONTROL_EPOCH: u64 = 1;
const CONTROL_BLOCK_FRAMES: usize = 1_024;

fn retained_target(target: &AutomationTarget) -> bool {
    matches!(
        canonicalize_automation_target(target.clone()),
        AutomationTarget::MasterVolume
            | AutomationTarget::MasterPan
            | AutomationTarget::Tempo
            | AutomationTarget::MixerMute {
                track: MASTER_MIXER_TRACK_ID
            }
    )
}

fn compiled_control_target(target: CompiledAutomationTarget) -> bool {
    matches!(
        target,
        CompiledAutomationTarget::MasterVolume
            | CompiledAutomationTarget::MasterPan
            | CompiledAutomationTarget::Tempo
            | CompiledAutomationTarget::MixerMute { track: 0 }
    )
}

struct ControlSink {
    block: TimelineAutomationBlockPlan,
    chase: Vec<TimelineAutomationChaseValue>,
    failure: Option<TimelineAutomationError>,
    unexpected_source: bool,
}

impl ControlSink {
    fn new() -> Self {
        Self {
            block: TimelineAutomationBlockPlan::new(),
            chase: Vec::with_capacity(4),
            failure: None,
            unexpected_source: false,
        }
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            !self.unexpected_source,
            "Offline master control plan contains an audio/note source"
        );
        if let Some(error) = self.failure {
            return Err(error)
                .context("Offline master automation plan exceeded its bounded capacity");
        }
        Ok(())
    }

    fn reset_block(&mut self, frames: u32) -> Result<()> {
        self.failure = None;
        self.unexpected_source = false;
        self.block.reset(frames)?;
        Ok(())
    }
}

impl TimelineAudioSink for ControlSink {
    fn wants_automation_curve_values(&self) -> bool {
        true
    }

    fn note_on(&mut self, _: ChasedNote, _: u32) {
        self.unexpected_source = true;
    }
    fn note_off(&mut self, _: ChasedNote, _: u32) {
        self.unexpected_source = true;
    }
    fn audio_start(&mut self, _: ChasedAudioClip, _: u32) {
        self.unexpected_source = true;
    }
    fn audio_stop(&mut self, _: ChasedAudioClip, _: u32) {
        self.unexpected_source = true;
    }

    fn automation_chase_value(&mut self, value: TimelineAutomationChaseValue) {
        self.chase.push(value);
    }

    fn automation_transition(&mut self, transition: TimelineAutomationTransition) {
        if self.failure.is_none() {
            self.failure = self.block.push_transition(transition).err();
        }
    }

    fn automation_block_endpoint(&mut self, endpoint: TimelineAutomationBlockEndpoint) {
        if self.failure.is_none() {
            self.failure = self.block.push_block_endpoint(endpoint).err();
        }
    }
}

pub(crate) struct OfflineMasterControls {
    timeline: CompiledTimeline,
    executor: TimelineExecutor,
    automation: RealtimeTimelineAutomation,
    sink: ControlSink,
    packet: Box<TimelinePacket<TIMELINE_CALLBACK_MAX_EVENTS>>,
    volume_driven: bool,
    pan_driven: bool,
    mute_driven: bool,
    music_frames: u64,
    next_output_frame: u64,
    last_volume: f32,
    last_pan: f32,
    last_muted: bool,
    failed: bool,
}

impl OfflineMasterControls {
    /// Preparation allocates once. Every later sample/block reuses these
    /// buffers and performs no discontinuity chase or UI-rate evaluation.
    /// Unsupported-processing validation remains the export caller's duty.
    pub(crate) fn prepare(project: &Project, tempo_map: &TempoMap) -> Result<Self> {
        let mut control_project = project.clone();
        control_project
            .automation_lanes
            .retain(|automation| retained_target(automation.lane.target()));
        let ids: BTreeSet<u64> = control_project
            .automation_lanes
            .iter()
            .map(|lane| lane.id)
            .collect();
        control_project.clips.retain(|clip| {
            clip.kind == ClipKind::Automation
                && clip.automation_id.is_some_and(|id| ids.contains(&id))
        });
        // Projection must not make an originally ambiguous control identity
        // unique by removing a music clip or an unsupported automation lane.
        let mut lane_counts = BTreeMap::<u64, usize>::new();
        for automation in &project.automation_lanes {
            *lane_counts.entry(automation.id).or_default() += 1;
        }
        for automation in &control_project.automation_lanes {
            ensure!(
                automation.id != 0 && lane_counts.get(&automation.id) == Some(&1),
                "Offline master automation lane {} has an invalid or duplicate identity",
                automation.id
            );
        }
        let mut clip_counts = BTreeMap::<u32, usize>::new();
        for clip in &project.clips {
            *clip_counts.entry(clip.id).or_default() += 1;
        }
        for clip in &control_project.clips {
            ensure!(
                clip.id != 0 && clip_counts.get(&clip.id) == Some(&1),
                "Offline master placement {} has an invalid or duplicate identity",
                clip.id
            );
        }
        control_project.channels.clear();
        control_project.patterns.clear();
        control_project.piano_notes.clear();
        control_project.audio_assets.clear();
        control_project.audio_clip_mixer_destinations.clear();
        control_project.plugin_instances.clear();
        control_project.mixer_insert_slots.clear();

        let timeline = CompiledTimeline::from_project(
            &control_project,
            tempo_map,
            TimelineCompileOptions::default(),
        )
        .context("Unable to compile offline Master Volume/Pan/Mute automation")?;
        ensure!(
            timeline.diagnostics().is_empty(),
            "Offline master automation has unresolved compiler diagnostics: {:?}",
            timeline.diagnostics()
        );
        let driven = timeline.driven_automation_targets();
        let volume_driven = driven
            .binary_search(&CompiledAutomationTarget::MasterVolume)
            .is_ok();
        let pan_driven = driven
            .binary_search(&CompiledAutomationTarget::MasterPan)
            .is_ok();
        let mute_driven = driven
            .binary_search(&CompiledAutomationTarget::MixerMute { track: 0 })
            .is_ok();
        let mut chase = timeline.chase_discontinuity(0, TimelineChaseOptions::default())?;
        // The retained plan can emit only these four targets. Excluding the
        // other static mixer bases keeps per-sample work bounded to controls.
        chase
            .automation_bases
            .retain(|base| compiled_control_target(base.target));
        let mut executor = TimelineExecutor::new();
        let mut sink = ControlSink::new();
        executor.reset_from_chase(CONTROL_EPOCH, &chase, &mut sink)?;
        sink.validate()?;
        let mut automation = RealtimeTimelineAutomation::new();
        automation.reset_from_chase(CONTROL_EPOCH, 0, &chase.automation_bases, &sink.chase)?;
        let last_volume = automation
            .value_for(CompiledAutomationTarget::MasterVolume)
            .context("Compiled master volume base is absent")?
            .clamp(0.0, 1.2);
        let last_pan = automation
            .value_for(CompiledAutomationTarget::MasterPan)
            .context("Compiled master pan base is absent")?
            .clamp(-1.0, 1.0);
        let last_muted = automation
            .value_for(CompiledAutomationTarget::MixerMute { track: 0 })
            .context("Compiled master mute base is absent")?
            >= 0.5;
        Ok(Self {
            music_frames: timeline.duration_frames(),
            timeline,
            executor,
            automation,
            sink,
            packet: TimelinePacket::new_boxed(),
            volume_driven,
            pan_driven,
            mute_driven,
            next_output_frame: 0,
            last_volume,
            last_pan,
            last_muted,
            failed: false,
        })
    }

    pub(crate) fn drives_volume(&self) -> bool {
        self.volume_driven
    }
    pub(crate) fn drives_pan(&self) -> bool {
        self.pan_driven
    }
    pub(crate) fn drives_mute(&self) -> bool {
        self.mute_driven
    }

    /// Applies one contiguous caller-owned output span. Music controls advance
    /// only through the compiled duration. Fixed-tail output holds the last
    /// actual music sample's controls; it cannot loop or revive source events.
    pub(crate) fn apply_block(
        &mut self,
        start_frame: u64,
        output: &mut [StereoFrame],
    ) -> Result<()> {
        ensure!(
            !self.failed,
            "Offline master automation is invalid after a rejected block"
        );
        ensure!(
            start_frame == self.next_output_frame,
            "Offline master output starts at {start_frame}; expected {}",
            self.next_output_frame
        );
        let count =
            u64::try_from(output.len()).context("Offline master block length exceeds u64")?;
        let end_frame = start_frame
            .checked_add(count)
            .context("Offline master output frame range overflow")?;
        let result = self.apply_contiguous(output);
        if result.is_ok() {
            self.next_output_frame = end_frame;
        } else {
            self.failed = true;
        }
        result
    }

    /// Convenience pass after source mixing. Cancellation remains cooperative;
    /// no destination is opened or committed by this helper.
    pub(crate) fn apply_all(
        &mut self,
        output: &mut [StereoFrame],
        control: &ExportControl,
    ) -> Result<()> {
        ensure!(
            self.next_output_frame == 0,
            "Offline master full pass has already started"
        );
        let total = output.len();
        for (block, samples) in output.chunks_mut(CONTROL_BLOCK_FRAMES).enumerate() {
            let start = block
                .checked_mul(CONTROL_BLOCK_FRAMES)
                .context("Offline master block offset overflow")?;
            control.work_progress(start, total, 6_500, 6_900)?;
            self.apply_block(u64::try_from(start)?, samples)?;
        }
        control.check_cancelled()?;
        Ok(())
    }

    fn apply_contiguous(&mut self, output: &mut [StereoFrame]) -> Result<()> {
        let mut consumed = 0;
        let mut source_frame = self.next_output_frame;
        while consumed < output.len() && source_frame < self.music_frames {
            let remaining_music =
                usize::try_from(self.music_frames - source_frame).unwrap_or(usize::MAX);
            let frames = (output.len() - consumed)
                .min(remaining_music)
                .min(CONTROL_BLOCK_FRAMES);
            self.sink.reset_block(frames as u32)?;
            let mut range =
                self.timeline
                    .event_range(CONTROL_EPOCH, source_frame, frames as u32)?;
            loop {
                let chunk = range.packetize_next_into(self.packet.as_mut())?;
                self.executor
                    .process_packet(self.packet.as_ref(), &mut self.sink)?;
                self.sink.validate()?;
                if chunk.remaining_events == 0 {
                    break;
                }
            }
            self.executor
                .finish_block(source_frame, frames as u32, &mut self.sink)?;
            self.sink.validate()?;
            self.automation
                .begin_block(CONTROL_EPOCH, source_frame, &self.sink.block)?;
            for offset in 0..frames {
                let mut volume = self.last_volume;
                let mut pan = self.last_pan;
                let mut muted = self.last_muted;
                self.automation
                    .render_frame(offset as u32, |target, value| match target {
                        CompiledAutomationTarget::MasterVolume => volume = value.clamp(0.0, 1.2),
                        CompiledAutomationTarget::MasterPan => pan = value.clamp(-1.0, 1.0),
                        CompiledAutomationTarget::MixerMute { track: 0 } => muted = value >= 0.5,
                        _ => {}
                    })?;
                self.apply_values(&mut output[consumed + offset], volume, pan, muted)?;
                self.last_volume = volume;
                self.last_pan = pan;
                self.last_muted = muted;
            }
            self.automation.finish_block()?;
            self.automation.commit_block()?;
            source_frame += frames as u64;
            consumed += frames;
        }
        for sample in &mut output[consumed..] {
            self.apply_values(sample, self.last_volume, self.last_pan, self.last_muted)?;
        }
        Ok(())
    }

    fn apply_values(
        &self,
        sample: &mut StereoFrame,
        volume: f32,
        pan: f32,
        muted: bool,
    ) -> Result<()> {
        let gain = if self.volume_driven { volume } else { 1.0 };
        let pan = if self.pan_driven { pan } else { 0.0 };
        let left = sample.left * gain * (1.0 - pan.max(0.0));
        let right = sample.right * gain * (1.0 + pan.min(0.0));
        ensure!(
            left.is_finite() && right.is_finite(),
            "Offline master automation produced non-finite audio"
        );
        if self.mute_driven && muted {
            *sample = StereoFrame::default();
        } else {
            sample.left = left;
            sample.right = right;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "export_master_controls_tests.rs"]
mod tests;
