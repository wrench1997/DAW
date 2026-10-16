//! Whole source Generator controls use the compiled RT layers/kernel, sampled
//! only at raw source Q128 starts. Fixed tail retains the last actual write.
use super::*;
use crate::{
    timeline::{
        CompiledAutomationTarget, CompiledTimeline, TIMELINE_CALLBACK_MAX_EVENTS,
        TimelineChaseOptions, TimelinePacket,
    },
    timeline_automation::{
        RealtimeTimelineAutomation, TimelineAutomationBlockPlan, TimelineAutomationError,
    },
    timeline_executor::{
        TimelineAudioSink, TimelineAutomationBlockEndpoint, TimelineAutomationChaseValue,
        TimelineAutomationTransition, TimelineExecutor,
    },
};

struct Sink {
    instance: u64,
    ids: Vec<u32>,
    block: TimelineAutomationBlockPlan,
    chase: Vec<TimelineAutomationChaseValue>,
    failure: Option<TimelineAutomationError>,
}
impl Sink {
    fn accepts(&self, target: CompiledAutomationTarget) -> bool {
        matches!(target, CompiledAutomationTarget::PluginParameter { instance_id, parameter_id }
            if instance_id == self.instance && self.ids.binary_search(&parameter_id).is_ok())
    }
    fn validate(&self) -> Result<(), RenderFailure> {
        if let Some(error) = self.failure {
            return Err(RenderFailure::new(FailureKind::Invalid, error.to_string()));
        }
        Ok(())
    }
}
impl TimelineAudioSink for Sink {
    fn wants_automation_curve_values(&self) -> bool {
        true
    }
    // The same original compiled notes are independently validated/mapped by
    // PreparedRender. This cursor never emits a second MIDI or audio stream.
    fn note_on(&mut self, _: crate::timeline::ChasedNote, _: u32) {}
    fn note_off(&mut self, _: crate::timeline::ChasedNote, _: u32) {}
    fn audio_start(&mut self, _: crate::timeline::ChasedAudioClip, _: u32) {}
    fn audio_stop(&mut self, _: crate::timeline::ChasedAudioClip, _: u32) {}
    fn automation_chase_value(&mut self, value: TimelineAutomationChaseValue) {
        if self.accepts(value.target) {
            self.chase.push(value);
        }
    }
    fn automation_transition(&mut self, value: TimelineAutomationTransition) {
        if self.accepts(value.target) && self.failure.is_none() {
            self.failure = self.block.push_transition(value).err();
        }
    }
    fn automation_block_endpoint(&mut self, value: TimelineAutomationBlockEndpoint) {
        if self.accepts(value.target) && self.failure.is_none() {
            self.failure = self.block.push_block_endpoint(value).err();
        }
    }
}

pub(super) struct GeneratorControls {
    executor: TimelineExecutor,
    kernel: RealtimeTimelineAutomation,
    sink: Sink,
    packet: Box<TimelinePacket<TIMELINE_CALLBACK_MAX_EVENTS>>,
    next_frame: u64,
    values: Vec<(u32, f32)>,
}
impl GeneratorControls {
    pub(super) fn new(capture: &CapturedInstrumentBounce) -> Result<Option<Self>, RenderFailure> {
        let ids = capture.plan().generator_automation_ids();
        if ids.is_empty() {
            return Ok(None);
        }
        let invalid = |e: String| RenderFailure::new(FailureKind::Invalid, e);
        let catalog = capture
            .plan()
            .generator_automation_catalog()
            .ok_or_else(|| invalid("Whole Generator source catalog is absent".into()))?;
        catalog.validate(ids).map_err(invalid)?;
        for id in ids {
            if !capture.state.parameter_bases.contains_key(id) {
                return Err(invalid(format!(
                    "Generator parameter {id} has no captured f32 base"
                )));
            }
        }
        let timeline = capture.plan().timeline();
        let chase = timeline
            .chase_discontinuity(0, TimelineChaseOptions::default())
            .map_err(|e| invalid(e.to_string()))?;
        let mut sink = Sink {
            instance: capture.plan().instance_id(),
            ids: ids.to_vec(),
            block: TimelineAutomationBlockPlan::new(),
            chase: Vec::with_capacity(ids.len()),
            failure: None,
        };
        let mut executor = TimelineExecutor::new();
        executor
            .reset_from_chase(1, &chase, &mut sink)
            .map_err(|e| invalid(e.to_string()))?;
        sink.validate()?;
        let bases: Vec<_> = chase
            .automation_bases
            .iter()
            .copied()
            .filter(|base| sink.accepts(base.target))
            .collect();
        let mut kernel = RealtimeTimelineAutomation::new();
        kernel
            .reset_from_chase(1, 0, &bases, &sink.chase)
            .map_err(|e| invalid(e.to_string()))?;
        Ok(Some(Self {
            executor,
            kernel,
            sink,
            packet: TimelinePacket::new_boxed(),
            next_frame: 0,
            values: Vec::with_capacity(ids.len()),
        }))
    }

    pub(super) fn apply<B: RenderBackend>(
        &mut self,
        timeline: &CompiledTimeline,
        start: u64,
        backend: &mut B,
        control: &RenderControl,
    ) -> Result<(), RenderFailure> {
        let invalid = |e: String| RenderFailure::new(FailureKind::Invalid, e);
        if start >= timeline.duration_frames() {
            // No synthetic N-1 sample or AutomationEnd/base write in fixed tail.
            return Ok(());
        }
        if start != self.next_frame {
            return Err(invalid("Generator control frame order changed".into()));
        }
        let frames = (timeline.duration_frames() - start).min(BOUNCE_QUANTUM as u64) as u32;
        self.sink.failure = None;
        self.sink
            .block
            .reset(frames)
            .map_err(|e| invalid(e.to_string()))?;
        let mut range = timeline
            .event_range(1, start, frames)
            .map_err(|e| invalid(e.to_string()))?;
        loop {
            control.checkpoint()?;
            let chunk = range
                .packetize_next_into(self.packet.as_mut())
                .map_err(|e| invalid(e.to_string()))?;
            self.executor
                .process_packet(self.packet.as_ref(), &mut self.sink)
                .map_err(|e| invalid(e.to_string()))?;
            self.sink.validate()?;
            if chunk.remaining_events == 0 {
                break;
            }
        }
        self.executor
            .finish_block(start, frames, &mut self.sink)
            .map_err(|e| invalid(e.to_string()))?;
        self.sink.validate()?;
        self.kernel
            .begin_block(1, start, &self.sink.block)
            .map_err(|e| invalid(e.to_string()))?;
        self.values.clear();
        for offset in 0..frames {
            self.kernel
                .render_frame(offset, |target, value| {
                    if offset == 0
                        && let CompiledAutomationTarget::PluginParameter { parameter_id, .. } =
                            target
                    {
                        self.values.push((parameter_id, value));
                    }
                })
                .map_err(|e| invalid(e.to_string()))?;
        }
        self.kernel
            .finish_block()
            .map_err(|e| invalid(e.to_string()))?;
        self.kernel
            .commit_block()
            .map_err(|e| invalid(e.to_string()))?;
        for &(id, value) in &self.values {
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                return Err(invalid(format!(
                    "Generator parameter {id} Q128 value is not normalized"
                )));
            }
        }
        control.call(|| backend.set_automated_parameters(&self.values))?;
        self.next_frame = start + u64::from(frames);
        Ok(())
    }
}
