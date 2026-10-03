//! Allocation-free realtime execution of compiled timeline packets.
//!
//! The compiler and discontinuity chase live on the control thread. This module
//! owns only fixed-capacity callback state and never allocates while resetting,
//! consuming packets, or finishing an audio block.

use std::fmt;

use crate::timeline::{
    AudioClipDescriptor, AutomationBaseValue, AutomationRampDescriptor, AutomationRampShape,
    ChasedAudioClip, ChasedAutomationLayer, ChasedNote, CompiledAutomationTarget,
    TimelineDiscontinuityState, TimelineEventKind, TimelinePacket, TimelinePacketEvent,
};

pub const MAX_ACTIVE_AUTOMATION_LAYERS: usize = 512;
pub const MAX_AUTOMATION_BASES: usize = 512;
pub const MAX_ACTIVE_NOTES: usize = 512;
pub const MAX_ACTIVE_AUDIO_CLIPS: usize = 128;

/// Resolved automation state installed by a discontinuity chase. This value is
/// the starting state for a future block, not an event at sample offset zero.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimelineAutomationChaseValue {
    pub target: CompiledAutomationTarget,
    pub value: f32,
    pub shape: AutomationRampShape,
}

/// One packet-ordered automation state change inside the current block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimelineAutomationTransition {
    pub target: CompiledAutomationTarget,
    pub before_value: f32,
    pub after_value: f32,
    pub after_shape: AutomationRampShape,
    pub sample_offset: u32,
}

/// Resolved automation value at the exclusive end of a completed block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimelineAutomationBlockEndpoint {
    pub target: CompiledAutomationTarget,
    pub value: f32,
}

/// Callback operations emitted in exact packet order.
pub trait TimelineAudioSink {
    fn note_on(&mut self, note: ChasedNote, sample_offset: u32);
    fn note_off(&mut self, note: ChasedNote, sample_offset: u32);
    fn audio_start(&mut self, clip: ChasedAudioClip, sample_offset: u32);
    fn audio_stop(&mut self, clip: ChasedAudioClip, sample_offset: u32);
    /// Installs the resolved starting value and carried interpolation shape
    /// after an epoch chase. It must not be interpreted as a block event.
    fn automation_chase_value(&mut self, value: TimelineAutomationChaseValue);
    /// Schedules a state transition inside the current block. The executor
    /// guarantees `sample_offset < frames`; sinks may safely use it as a current
    /// block sample index.
    fn automation_transition(&mut self, transition: TimelineAutomationTransition);
    /// Commits the resolved value at an exclusive block end. It must not index
    /// the finished sample buffer.
    fn automation_block_endpoint(&mut self, endpoint: TimelineAutomationBlockEndpoint);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimelineExecutorCapacity {
    AutomationLayers,
    AutomationBases,
    Notes,
    AudioClips,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimelineExecutorError {
    ZeroEpoch,
    EpochMismatch {
        expected: u64,
        received: u64,
    },
    FrameCountTooLarge {
        frames: u32,
    },
    FrameRangeOverflow,
    BlockStartMismatch {
        expected: u64,
        received: u64,
    },
    BlockMetadataMismatch,
    EventOffsetOutOfRange {
        offset: u16,
        frames: u32,
    },
    EventOrderViolation {
        previous: u16,
        received: u16,
    },
    CapacityExceeded {
        resource: TimelineExecutorCapacity,
        capacity: usize,
    },
    DuplicateAutomationBase,
    MissingAutomationBase,
    DuplicateAutomationLayer,
    AutomationLayerNotFound,
    DuplicateNoteId {
        note_id: u64,
    },
    NoteNotFound {
        note_id: u64,
    },
    NoteOffMismatch {
        note_id: u64,
    },
    DuplicateAudioClip {
        clip_id: u32,
    },
    AudioClipNotFound {
        clip_id: u32,
    },
    InvalidNote,
    InvalidAudioClip,
    InvalidAutomationRamp,
    NonFiniteValue,
    SequenceOverflow,
}

impl fmt::Display for TimelineExecutorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "timeline executor error: {self:?}")
    }
}

impl std::error::Error for TimelineExecutorError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BlockState {
    start_frame: u64,
    frames: u32,
    last_offset: Option<u16>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct AutomationLayerState {
    automation_id: u64,
    placement_id: Option<u32>,
    precedence: u64,
    target: CompiledAutomationTarget,
    start_frame: u64,
    start_value: f32,
    end_value: f32,
    end_frame: u64,
    shape: AutomationRampShape,
    order: u64,
    base_index: u16,
}

impl AutomationLayerState {
    fn from_chase(
        frame: u64,
        layer: ChasedAutomationLayer,
        order: u64,
        base_index: u16,
    ) -> Result<Self, TimelineExecutorError> {
        if !layer.current_value.is_finite() || !layer.end_value.is_finite() {
            return Err(TimelineExecutorError::NonFiniteValue);
        }
        if layer.end_frame < frame {
            return Err(TimelineExecutorError::InvalidAutomationRamp);
        }
        Ok(Self {
            automation_id: layer.automation_id,
            placement_id: layer.placement_id,
            precedence: layer.precedence,
            target: layer.target,
            start_frame: frame,
            start_value: layer.current_value,
            end_value: layer.end_value,
            end_frame: layer.end_frame,
            shape: layer.shape,
            order,
            base_index,
        })
    }

    fn from_ramp(
        frame: u64,
        ramp: AutomationRampDescriptor,
        order: u64,
        base_index: u16,
    ) -> Result<Self, TimelineExecutorError> {
        if !ramp.start_value.is_finite() || !ramp.end_value.is_finite() {
            return Err(TimelineExecutorError::NonFiniteValue);
        }
        if ramp.end_frame < frame {
            return Err(TimelineExecutorError::InvalidAutomationRamp);
        }
        Ok(Self {
            automation_id: ramp.automation_id,
            placement_id: ramp.placement_id,
            precedence: ramp.precedence,
            target: ramp.target,
            start_frame: frame,
            start_value: ramp.start_value,
            end_value: ramp.end_value,
            end_frame: ramp.end_frame,
            shape: ramp.shape,
            order,
            base_index,
        })
    }

    fn exact_key_matches(
        self,
        target: CompiledAutomationTarget,
        precedence: u64,
        automation_id: u64,
        placement_id: Option<u32>,
    ) -> bool {
        self.target == target
            && self.precedence == precedence
            && self.automation_id == automation_id
            && self.placement_id == placement_id
    }

    fn value_at(self, frame: u64) -> Result<f32, TimelineExecutorError> {
        if self.shape == AutomationRampShape::Hold || self.end_frame <= self.start_frame {
            return Ok(self.start_value);
        }
        let elapsed = frame
            .saturating_sub(self.start_frame)
            .min(self.end_frame - self.start_frame);
        let progress = elapsed as f64 / (self.end_frame - self.start_frame) as f64;
        let value = f64::from(self.start_value)
            + (f64::from(self.end_value) - f64::from(self.start_value)) * progress;
        let value = value as f32;
        if value.is_finite() {
            Ok(value)
        } else {
            Err(TimelineExecutorError::NonFiniteValue)
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TimelineExecutorStats {
    /// Winner-cache rebuilds caused by reset or one target's layer mutation.
    pub winner_recomputations: u64,
    /// Fixed layer slots inspected by those rebuilds. Block finishing never
    /// increments this counter.
    pub winner_layer_slots_scanned: u64,
    pub automation_events_emitted: u64,
    pub block_endpoints_emitted: u64,
}

struct ExecutorState {
    epoch: u64,
    next_frame: u64,
    block: Option<BlockState>,
    next_layer_order: u64,
    automation_bases: [Option<AutomationBaseValue>; MAX_AUTOMATION_BASES],
    automation_layers: [Option<AutomationLayerState>; MAX_ACTIVE_AUTOMATION_LAYERS],
    /// Base slot -> winning layer slot. Layer scans happen only when the target
    /// changes, never while finishing a block.
    automation_winners: [Option<u16>; MAX_AUTOMATION_BASES],
    notes: [Option<ChasedNote>; MAX_ACTIVE_NOTES],
    audio_clips: [Option<ChasedAudioClip>; MAX_ACTIVE_AUDIO_CLIPS],
    stats: TimelineExecutorStats,
}

impl ExecutorState {
    const fn empty() -> Self {
        Self {
            epoch: 0,
            next_frame: 0,
            block: None,
            next_layer_order: 0,
            automation_bases: [None; MAX_AUTOMATION_BASES],
            automation_layers: [None; MAX_ACTIVE_AUTOMATION_LAYERS],
            automation_winners: [None; MAX_AUTOMATION_BASES],
            notes: [None; MAX_ACTIVE_NOTES],
            audio_clips: [None; MAX_ACTIVE_AUDIO_CLIPS],
            stats: TimelineExecutorStats {
                winner_recomputations: 0,
                winner_layer_slots_scanned: 0,
                automation_events_emitted: 0,
                block_endpoints_emitted: 0,
            },
        }
    }

    fn clear_for_reset(&mut self, epoch: u64, frame: u64, stats: TimelineExecutorStats) {
        self.epoch = epoch;
        self.next_frame = frame;
        self.block = None;
        self.next_layer_order = 0;
        self.automation_bases.fill(None);
        self.automation_layers.fill(None);
        self.automation_winners.fill(None);
        self.notes.fill(None);
        self.audio_clips.fill(None);
        self.stats = stats;
    }

    /// Copies directly between the two control-thread-allocated buffers. No
    /// `ExecutorState` temporary is materialized on the callback stack.
    fn copy_from_state(&mut self, source: &Self) {
        self.epoch = source.epoch;
        self.next_frame = source.next_frame;
        self.block = source.block;
        self.next_layer_order = source.next_layer_order;
        self.automation_bases
            .copy_from_slice(&source.automation_bases);
        self.automation_layers
            .copy_from_slice(&source.automation_layers);
        self.automation_winners
            .copy_from_slice(&source.automation_winners);
        self.notes.copy_from_slice(&source.notes);
        self.audio_clips.copy_from_slice(&source.audio_clips);
        self.stats = source.stats;
    }
}

/// Fixed-capacity realtime executor for one installed timeline revision/epoch.
///
/// Both approximately 0.11 MiB state buffers are allocated by [`Self::new`] on
/// the control thread. Callback processing only copies active state directly into
/// the preallocated scratch buffer; no whole state is placed on the callback
/// stack and no heap allocation occurs after construction.
pub struct TimelineExecutor {
    active: Box<ExecutorState>,
    scratch: Box<ExecutorState>,
}

impl TimelineExecutor {
    #[must_use]
    pub fn new() -> Self {
        Self {
            active: Box::new(ExecutorState::empty()),
            scratch: Box::new(ExecutorState::empty()),
        }
    }

    #[must_use]
    pub const fn state_bytes_per_buffer() -> usize {
        std::mem::size_of::<ExecutorState>()
    }

    #[must_use]
    pub const fn preallocated_state_bytes() -> usize {
        std::mem::size_of::<ExecutorState>() * 2
    }

    #[must_use]
    pub const fn stats(&self) -> TimelineExecutorStats {
        self.active.stats
    }

    #[must_use]
    pub const fn epoch(&self) -> u64 {
        self.active.epoch
    }

    #[must_use]
    pub const fn next_frame(&self) -> u64 {
        self.active.next_frame
    }

    #[must_use]
    pub fn active_note_count(&self) -> usize {
        self.active.notes.iter().flatten().count()
    }

    #[must_use]
    pub fn active_audio_clip_count(&self) -> usize {
        self.active.audio_clips.iter().flatten().count()
    }

    #[must_use]
    pub fn active_automation_layer_count(&self) -> usize {
        self.active.automation_layers.iter().flatten().count()
    }

    /// Atomically validates and installs control-thread chase state, then emits
    /// the chased resources at offset zero. Existing callback state is unchanged
    /// when validation fails.
    pub fn reset_from_chase<S: TimelineAudioSink + ?Sized>(
        &mut self,
        epoch: u64,
        state: &TimelineDiscontinuityState,
        sink: &mut S,
    ) -> Result<(), TimelineExecutorError> {
        self.stage_reset_from_chase(epoch, state, sink)?;
        self.commit_staged_reset();
        Ok(())
    }

    /// Validates a discontinuity into the preallocated scratch state and emits
    /// its offset-zero render plan without changing the active executor cursor.
    /// A caller may therefore preflight every other callback resource before
    /// committing the reset at one proven-infallible boundary.
    pub(crate) fn stage_reset_from_chase<S: TimelineAudioSink + ?Sized>(
        &mut self,
        epoch: u64,
        state: &TimelineDiscontinuityState,
        sink: &mut S,
    ) -> Result<(), TimelineExecutorError> {
        if epoch == 0 {
            return Err(TimelineExecutorError::ZeroEpoch);
        }
        let stats = self.active.stats;
        self.scratch.clear_for_reset(epoch, state.frame, stats);

        for base in state.automation_bases.iter().copied() {
            self.scratch.insert_base(base)?;
        }
        for layer in state.automation_layers.iter().copied() {
            self.scratch.insert_chased_layer(state.frame, layer)?;
        }
        for note in state.notes.iter().copied() {
            self.scratch.insert_note(note)?;
        }
        for clip in state.audio_clips.iter().copied() {
            self.scratch.insert_audio_clip(clip)?;
        }
        self.scratch.recompute_all_winners()?;
        self.scratch.validate_all_resolved_values(state.frame)?;

        for clip in state.audio_clips.iter().copied() {
            sink.audio_start(clip, 0);
        }
        for note in state.notes.iter().copied() {
            sink.note_on(note, 0);
        }
        // A chased value initializes a future block; it is neither an event at
        // offset zero nor the endpoint of a block that has not run.
        self.scratch.emit_all_chase_values(state.frame, sink)?;
        Ok(())
    }

    /// Commits the last successfully staged reset with an O(1) pointer swap.
    /// No validation, allocation, sink callback, or endpoint operation occurs.
    pub(crate) fn commit_staged_reset(&mut self) {
        std::mem::swap(&mut self.active, &mut self.scratch);
    }

    /// Discards a staged reset while preserving the active executor byte-for-byte.
    pub(crate) fn abort_staged_reset(&mut self) {
        // Scratch is overwritten in full by the next stage. Keeping this a
        // no-op preserves the executor's two-pointer realtime footprint.
    }

    /// Consumes one packet or one chunk of a packetized block. Multiple chunks
    /// may share metadata; offsets must remain nondecreasing across chunks.
    pub fn process_packet<const CAPACITY: usize, S: TimelineAudioSink + ?Sized>(
        &mut self,
        packet: &TimelinePacket<CAPACITY>,
        sink: &mut S,
    ) -> Result<(), TimelineExecutorError> {
        self.process_event_slice(
            packet.epoch(),
            packet.start_frame(),
            packet.frames(),
            packet.events(),
            sink,
        )
    }

    /// Emits every active automation winner at the exclusive block end and
    /// advances the executor clock. This must be called once after all chunks.
    pub fn finish_block<S: TimelineAudioSink + ?Sized>(
        &mut self,
        start_frame: u64,
        frames: u32,
        sink: &mut S,
    ) -> Result<(), TimelineExecutorError> {
        self.active.finish_block_inner(start_frame, frames, sink)
    }

    fn process_event_slice<S: TimelineAudioSink + ?Sized>(
        &mut self,
        epoch: u64,
        start_frame: u64,
        frames: u32,
        events: &[TimelinePacketEvent],
        sink: &mut S,
    ) -> Result<(), TimelineExecutorError> {
        self.active
            .validate_block_metadata(epoch, start_frame, frames)?;
        if events.is_empty() {
            return self
                .active
                .process_event_slice_inner(epoch, start_frame, frames, events, sink);
        }
        // Transaction validation uses the preallocated scratch state. The copy
        // is field-wise and cannot create a large callback-stack temporary.
        self.scratch.copy_from_state(&self.active);
        self.scratch.process_event_slice_inner(
            epoch,
            start_frame,
            frames,
            events,
            &mut NoopSink,
        )?;
        self.active
            .process_event_slice_inner(epoch, start_frame, frames, events, sink)
    }
}

impl ExecutorState {
    fn process_event_slice_inner<S: TimelineAudioSink + ?Sized>(
        &mut self,
        epoch: u64,
        start_frame: u64,
        frames: u32,
        events: &[TimelinePacketEvent],
        sink: &mut S,
    ) -> Result<(), TimelineExecutorError> {
        self.begin_or_continue_block(epoch, start_frame, frames)?;
        for event in events {
            if u32::from(event.sample_offset) >= frames {
                return Err(TimelineExecutorError::EventOffsetOutOfRange {
                    offset: event.sample_offset,
                    frames,
                });
            }
            let previous = self.block.and_then(|block| block.last_offset);
            if let Some(previous) = previous
                && event.sample_offset < previous
            {
                return Err(TimelineExecutorError::EventOrderViolation {
                    previous,
                    received: event.sample_offset,
                });
            }
            let absolute_frame = start_frame
                .checked_add(u64::from(event.sample_offset))
                .ok_or(TimelineExecutorError::FrameRangeOverflow)?;
            self.apply_event(
                event.kind,
                absolute_frame,
                u32::from(event.sample_offset),
                sink,
            )?;
            if let Some(block) = &mut self.block {
                block.last_offset = Some(event.sample_offset);
            }
        }
        Ok(())
    }

    fn begin_or_continue_block(
        &mut self,
        epoch: u64,
        start_frame: u64,
        frames: u32,
    ) -> Result<(), TimelineExecutorError> {
        self.validate_block_metadata(epoch, start_frame, frames)?;
        if self.block.is_none() {
            self.block = Some(BlockState {
                start_frame,
                frames,
                last_offset: None,
            });
        }
        Ok(())
    }

    fn validate_block_metadata(
        &self,
        epoch: u64,
        start_frame: u64,
        frames: u32,
    ) -> Result<(), TimelineExecutorError> {
        if epoch != self.epoch {
            return Err(TimelineExecutorError::EpochMismatch {
                expected: self.epoch,
                received: epoch,
            });
        }
        validate_frame_count(start_frame, frames)?;
        match self.block {
            Some(block) if block.start_frame == start_frame && block.frames == frames => Ok(()),
            Some(_) => Err(TimelineExecutorError::BlockMetadataMismatch),
            None if start_frame != self.next_frame => {
                Err(TimelineExecutorError::BlockStartMismatch {
                    expected: self.next_frame,
                    received: start_frame,
                })
            }
            None => Ok(()),
        }
    }

    fn finish_block_inner<S: TimelineAudioSink + ?Sized>(
        &mut self,
        start_frame: u64,
        frames: u32,
        sink: &mut S,
    ) -> Result<(), TimelineExecutorError> {
        let end_frame = validate_frame_count(start_frame, frames)?;
        match self.block {
            Some(block) if block.start_frame == start_frame && block.frames == frames => {}
            Some(_) => return Err(TimelineExecutorError::BlockMetadataMismatch),
            None if start_frame != self.next_frame => {
                return Err(TimelineExecutorError::BlockStartMismatch {
                    expected: self.next_frame,
                    received: start_frame,
                });
            }
            None => {}
        }
        self.validate_all_resolved_values(end_frame)?;
        self.emit_all_block_endpoints(end_frame, sink)?;
        self.next_frame = end_frame;
        self.block = None;
        Ok(())
    }

    fn apply_event<S: TimelineAudioSink + ?Sized>(
        &mut self,
        kind: TimelineEventKind,
        absolute_frame: u64,
        sample_offset: u32,
        sink: &mut S,
    ) -> Result<(), TimelineExecutorError> {
        match kind {
            TimelineEventKind::NoteOn {
                note_id,
                channel_id,
                note,
                velocity,
                gain,
                mixer_track,
                source,
            } => {
                let note = ChasedNote {
                    note_id,
                    channel_id,
                    note,
                    velocity,
                    gain,
                    mixer_track,
                    source,
                };
                self.insert_note(note)?;
                sink.note_on(note, sample_offset);
            }
            TimelineEventKind::NoteOff {
                note_id,
                channel_id,
                note,
                mixer_track,
                source,
            } => {
                let active = self.remove_note(note_id)?;
                if active.channel_id != channel_id
                    || active.note != note
                    || active.mixer_track != mixer_track
                    || active.source != source
                {
                    return Err(TimelineExecutorError::NoteOffMismatch { note_id });
                }
                sink.note_off(active, sample_offset);
            }
            TimelineEventKind::AudioStart(descriptor) => {
                let clip = ChasedAudioClip {
                    descriptor,
                    source_position_frame: descriptor.source_offset_frame as f64,
                };
                if descriptor.start_frame != absolute_frame {
                    return Err(TimelineExecutorError::InvalidAudioClip);
                }
                self.insert_audio_clip(clip)?;
                sink.audio_start(clip, sample_offset);
            }
            TimelineEventKind::AudioStop { clip_id, asset_id } => {
                let clip = self.remove_audio_clip(clip_id, asset_id)?;
                sink.audio_stop(clip, sample_offset);
            }
            TimelineEventKind::AutomationRamp(ramp) => {
                let before_value = self.resolved_value(ramp.target, absolute_frame)?;
                self.upsert_ramp(absolute_frame, ramp)?;
                self.emit_automation_transition(
                    ramp.target,
                    before_value,
                    absolute_frame,
                    sample_offset,
                    sink,
                )?;
            }
            TimelineEventKind::AutomationEnd {
                automation_id,
                placement_id,
                precedence,
                target,
            } => {
                let before_value = self.resolved_value(target, absolute_frame)?;
                self.remove_layer(target, precedence, automation_id, placement_id)?;
                self.emit_automation_transition(
                    target,
                    before_value,
                    absolute_frame,
                    sample_offset,
                    sink,
                )?;
            }
        }
        Ok(())
    }

    fn insert_base(&mut self, base: AutomationBaseValue) -> Result<(), TimelineExecutorError> {
        if !base.value.is_finite() {
            return Err(TimelineExecutorError::NonFiniteValue);
        }
        if self
            .automation_bases
            .iter()
            .flatten()
            .any(|existing| existing.target == base.target)
        {
            return Err(TimelineExecutorError::DuplicateAutomationBase);
        }
        let slot = self
            .automation_bases
            .iter_mut()
            .find(|slot| slot.is_none())
            .ok_or(TimelineExecutorError::CapacityExceeded {
                resource: TimelineExecutorCapacity::AutomationBases,
                capacity: MAX_AUTOMATION_BASES,
            })?;
        *slot = Some(base);
        Ok(())
    }

    fn insert_chased_layer(
        &mut self,
        frame: u64,
        layer: ChasedAutomationLayer,
    ) -> Result<(), TimelineExecutorError> {
        let base_index = self.require_base_index(layer.target)?;
        if self
            .find_exact_layer(
                layer.target,
                layer.precedence,
                layer.automation_id,
                layer.placement_id,
            )
            .is_some()
        {
            return Err(TimelineExecutorError::DuplicateAutomationLayer);
        }
        let order = self.take_layer_order()?;
        let layer = AutomationLayerState::from_chase(
            frame,
            layer,
            order,
            u16::try_from(base_index).expect("automation base capacity fits u16"),
        )?;
        self.insert_new_layer(layer).map(|_| ())
    }

    fn upsert_ramp(
        &mut self,
        frame: u64,
        ramp: AutomationRampDescriptor,
    ) -> Result<(), TimelineExecutorError> {
        let base_index = self.require_base_index(ramp.target)?;
        let order = self.take_layer_order()?;
        let layer = AutomationLayerState::from_ramp(
            frame,
            ramp,
            order,
            u16::try_from(base_index).expect("automation base capacity fits u16"),
        )?;
        if let Some(index) = self.find_exact_layer(
            ramp.target,
            ramp.precedence,
            ramp.automation_id,
            ramp.placement_id,
        ) {
            self.automation_layers[index] = Some(layer);
            self.recompute_winner(base_index)?;
            return Ok(());
        }
        self.insert_new_layer(layer)?;
        self.recompute_winner(base_index)
    }

    fn insert_new_layer(
        &mut self,
        layer: AutomationLayerState,
    ) -> Result<usize, TimelineExecutorError> {
        let index = self
            .automation_layers
            .iter()
            .position(Option::is_none)
            .ok_or(TimelineExecutorError::CapacityExceeded {
                resource: TimelineExecutorCapacity::AutomationLayers,
                capacity: MAX_ACTIVE_AUTOMATION_LAYERS,
            })?;
        self.automation_layers[index] = Some(layer);
        Ok(index)
    }

    fn take_layer_order(&mut self) -> Result<u64, TimelineExecutorError> {
        let order = self.next_layer_order;
        self.next_layer_order = self
            .next_layer_order
            .checked_add(1)
            .ok_or(TimelineExecutorError::SequenceOverflow)?;
        Ok(order)
    }

    fn find_exact_layer(
        &self,
        target: CompiledAutomationTarget,
        precedence: u64,
        automation_id: u64,
        placement_id: Option<u32>,
    ) -> Option<usize> {
        self.automation_layers.iter().position(|candidate| {
            candidate.is_some_and(|layer| {
                layer.exact_key_matches(target, precedence, automation_id, placement_id)
            })
        })
    }

    fn remove_layer(
        &mut self,
        target: CompiledAutomationTarget,
        precedence: u64,
        automation_id: u64,
        placement_id: Option<u32>,
    ) -> Result<(), TimelineExecutorError> {
        let base_index = self.require_base_index(target)?;
        let index = self
            .find_exact_layer(target, precedence, automation_id, placement_id)
            .ok_or(TimelineExecutorError::AutomationLayerNotFound)?;
        self.automation_layers[index] = None;
        self.recompute_winner(base_index)
    }

    fn insert_note(&mut self, note: ChasedNote) -> Result<(), TimelineExecutorError> {
        validate_note(note)?;
        if self
            .notes
            .iter()
            .flatten()
            .any(|active| active.note_id == note.note_id)
        {
            return Err(TimelineExecutorError::DuplicateNoteId {
                note_id: note.note_id,
            });
        }
        let slot = self.notes.iter_mut().find(|slot| slot.is_none()).ok_or(
            TimelineExecutorError::CapacityExceeded {
                resource: TimelineExecutorCapacity::Notes,
                capacity: MAX_ACTIVE_NOTES,
            },
        )?;
        *slot = Some(note);
        Ok(())
    }

    fn remove_note(&mut self, note_id: u64) -> Result<ChasedNote, TimelineExecutorError> {
        let slot = self
            .notes
            .iter_mut()
            .find(|slot| slot.is_some_and(|note| note.note_id == note_id))
            .ok_or(TimelineExecutorError::NoteNotFound { note_id })?;
        slot.take()
            .ok_or(TimelineExecutorError::NoteNotFound { note_id })
    }

    fn insert_audio_clip(&mut self, clip: ChasedAudioClip) -> Result<(), TimelineExecutorError> {
        validate_audio_clip(clip)?;
        if self.audio_clips.iter().flatten().any(|active| {
            active.descriptor.clip_id == clip.descriptor.clip_id
                && active.descriptor.asset_id == clip.descriptor.asset_id
        }) {
            return Err(TimelineExecutorError::DuplicateAudioClip {
                clip_id: clip.descriptor.clip_id,
            });
        }
        let slot = self
            .audio_clips
            .iter_mut()
            .find(|slot| slot.is_none())
            .ok_or(TimelineExecutorError::CapacityExceeded {
                resource: TimelineExecutorCapacity::AudioClips,
                capacity: MAX_ACTIVE_AUDIO_CLIPS,
            })?;
        *slot = Some(clip);
        Ok(())
    }

    fn remove_audio_clip(
        &mut self,
        clip_id: u32,
        asset_id: u64,
    ) -> Result<ChasedAudioClip, TimelineExecutorError> {
        let slot = self
            .audio_clips
            .iter_mut()
            .find(|slot| {
                slot.is_some_and(|clip| {
                    clip.descriptor.clip_id == clip_id && clip.descriptor.asset_id == asset_id
                })
            })
            .ok_or(TimelineExecutorError::AudioClipNotFound { clip_id })?;
        slot.take()
            .ok_or(TimelineExecutorError::AudioClipNotFound { clip_id })
    }

    fn require_base_index(
        &self,
        target: CompiledAutomationTarget,
    ) -> Result<usize, TimelineExecutorError> {
        self.automation_bases
            .iter()
            .position(|base| base.is_some_and(|base| base.target == target))
            .ok_or(TimelineExecutorError::MissingAutomationBase)
    }

    fn recompute_all_winners(&mut self) -> Result<(), TimelineExecutorError> {
        self.automation_winners.fill(None);
        for base_index in 0..MAX_AUTOMATION_BASES {
            if self.automation_bases[base_index].is_some() {
                self.recompute_winner(base_index)?;
            }
        }
        Ok(())
    }

    fn recompute_winner(&mut self, base_index: usize) -> Result<(), TimelineExecutorError> {
        let base = self.automation_bases[base_index]
            .ok_or(TimelineExecutorError::MissingAutomationBase)?;
        let mut winner: Option<(usize, u64, u64)> = None;
        for (layer_index, candidate) in self.automation_layers.iter().enumerate() {
            let Some(layer) = candidate else {
                continue;
            };
            if usize::from(layer.base_index) != base_index || layer.target != base.target {
                continue;
            }
            let replace = winner.is_none_or(|(_, precedence, order)| {
                (layer.precedence, layer.order) > (precedence, order)
            });
            if replace {
                winner = Some((layer_index, layer.precedence, layer.order));
            }
        }
        self.automation_winners[base_index] = winner
            .map(|(index, _, _)| u16::try_from(index).expect("automation layer capacity fits u16"));
        self.stats.winner_recomputations = self.stats.winner_recomputations.saturating_add(1);
        self.stats.winner_layer_slots_scanned = self
            .stats
            .winner_layer_slots_scanned
            .saturating_add(MAX_ACTIVE_AUTOMATION_LAYERS as u64);
        Ok(())
    }

    fn resolved_value_by_base_index(
        &self,
        base_index: usize,
        frame: u64,
    ) -> Result<f32, TimelineExecutorError> {
        self.resolved_value_and_shape_by_base_index(base_index, frame)
            .map(|(value, _)| value)
    }

    fn resolved_value_and_shape_by_base_index(
        &self,
        base_index: usize,
        frame: u64,
    ) -> Result<(f32, AutomationRampShape), TimelineExecutorError> {
        let base = self.automation_bases[base_index]
            .ok_or(TimelineExecutorError::MissingAutomationBase)?;
        let (value, shape) = match self.automation_winners[base_index] {
            Some(layer_index) => {
                let layer = self.automation_layers[usize::from(layer_index)]
                    .ok_or(TimelineExecutorError::AutomationLayerNotFound)?;
                (layer.value_at(frame)?, layer.shape)
            }
            None => (base.value, AutomationRampShape::Hold),
        };
        normalize_automation_value(base.target, value).map(|value| (value, shape))
    }

    fn resolved_value(
        &self,
        target: CompiledAutomationTarget,
        frame: u64,
    ) -> Result<f32, TimelineExecutorError> {
        self.resolved_value_by_base_index(self.require_base_index(target)?, frame)
    }

    fn emit_automation_transition<S: TimelineAudioSink + ?Sized>(
        &mut self,
        target: CompiledAutomationTarget,
        before_value: f32,
        frame: u64,
        sample_offset: u32,
        sink: &mut S,
    ) -> Result<(), TimelineExecutorError> {
        let base_index = self.require_base_index(target)?;
        let (after_value, after_shape) =
            self.resolved_value_and_shape_by_base_index(base_index, frame)?;
        sink.automation_transition(TimelineAutomationTransition {
            target,
            before_value,
            after_value,
            after_shape,
            sample_offset,
        });
        self.stats.automation_events_emitted =
            self.stats.automation_events_emitted.saturating_add(1);
        Ok(())
    }

    fn emit_all_chase_values<S: TimelineAudioSink + ?Sized>(
        &mut self,
        frame: u64,
        sink: &mut S,
    ) -> Result<(), TimelineExecutorError> {
        for base_index in 0..MAX_AUTOMATION_BASES {
            let Some(base) = self.automation_bases[base_index] else {
                continue;
            };
            let (value, shape) = self.resolved_value_and_shape_by_base_index(base_index, frame)?;
            sink.automation_chase_value(TimelineAutomationChaseValue {
                target: base.target,
                value,
                shape,
            });
        }
        Ok(())
    }

    fn emit_all_block_endpoints<S: TimelineAudioSink + ?Sized>(
        &mut self,
        frame: u64,
        sink: &mut S,
    ) -> Result<(), TimelineExecutorError> {
        for base_index in 0..MAX_AUTOMATION_BASES {
            let Some(base) = self.automation_bases[base_index] else {
                continue;
            };
            let value = self.resolved_value_by_base_index(base_index, frame)?;
            sink.automation_block_endpoint(TimelineAutomationBlockEndpoint {
                target: base.target,
                value,
            });
            self.stats.block_endpoints_emitted =
                self.stats.block_endpoints_emitted.saturating_add(1);
        }
        Ok(())
    }

    fn validate_all_resolved_values(&self, frame: u64) -> Result<(), TimelineExecutorError> {
        for base_index in 0..MAX_AUTOMATION_BASES {
            if self.automation_bases[base_index].is_some() {
                self.resolved_value_by_base_index(base_index, frame)?;
            }
        }
        Ok(())
    }
}

impl Default for TimelineExecutor {
    fn default() -> Self {
        Self::new()
    }
}

fn validate_frame_count(start_frame: u64, frames: u32) -> Result<u64, TimelineExecutorError> {
    if frames > u32::from(u16::MAX) + 1 {
        return Err(TimelineExecutorError::FrameCountTooLarge { frames });
    }
    start_frame
        .checked_add(u64::from(frames))
        .ok_or(TimelineExecutorError::FrameRangeOverflow)
}

fn validate_note(note: ChasedNote) -> Result<(), TimelineExecutorError> {
    if note.note > 127 {
        return Err(TimelineExecutorError::InvalidNote);
    }
    if !note.velocity.is_finite() || !note.gain.is_finite() {
        return Err(TimelineExecutorError::NonFiniteValue);
    }
    Ok(())
}

fn validate_audio_clip(clip: ChasedAudioClip) -> Result<(), TimelineExecutorError> {
    let descriptor: AudioClipDescriptor = clip.descriptor;
    if descriptor.source_sample_rate == 0
        || descriptor.stop_frame < descriptor.start_frame
        || descriptor.clip_end_frame < descriptor.start_frame
    {
        return Err(TimelineExecutorError::InvalidAudioClip);
    }
    if !descriptor.gain.is_finite() || !clip.source_position_frame.is_finite() {
        return Err(TimelineExecutorError::NonFiniteValue);
    }
    Ok(())
}

fn normalize_automation_value(
    target: CompiledAutomationTarget,
    value: f32,
) -> Result<f32, TimelineExecutorError> {
    if !value.is_finite() {
        return Err(TimelineExecutorError::NonFiniteValue);
    }
    if matches!(
        target,
        CompiledAutomationTarget::MixerMute { .. } | CompiledAutomationTarget::ChannelMute { .. }
    ) {
        Ok(if value >= 0.5 { 1.0 } else { 0.0 })
    } else {
        Ok(value)
    }
}

struct NoopSink;

impl TimelineAudioSink for NoopSink {
    fn note_on(&mut self, _note: ChasedNote, _sample_offset: u32) {}
    fn note_off(&mut self, _note: ChasedNote, _sample_offset: u32) {}
    fn audio_start(&mut self, _clip: ChasedAudioClip, _sample_offset: u32) {}
    fn audio_stop(&mut self, _clip: ChasedAudioClip, _sample_offset: u32) {}
    fn automation_chase_value(&mut self, _value: TimelineAutomationChaseValue) {}
    fn automation_transition(&mut self, _transition: TimelineAutomationTransition) {}
    fn automation_block_endpoint(&mut self, _endpoint: TimelineAutomationBlockEndpoint) {}
}

#[cfg(test)]
mod tests {
    use crate::{
        automation::AutomationCurve,
        timeline::{
            AutomationRampDescriptor, NoteSourceDescriptor, TimelineDiscontinuityState,
            TimelineEventKind, TimelinePacketEvent,
        },
    };

    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    enum Output {
        NoteOn(u64, u32),
        NoteOff(u64, u32),
        AudioStart(u32, f64, u32),
        AudioStop(u32, u32),
        AutomationChase(TimelineAutomationChaseValue),
        AutomationTransition(TimelineAutomationTransition),
        AutomationEndpoint(TimelineAutomationBlockEndpoint),
    }

    #[derive(Default)]
    struct MockSink {
        output: Vec<Output>,
    }

    impl TimelineAudioSink for MockSink {
        fn note_on(&mut self, note: ChasedNote, sample_offset: u32) {
            self.output
                .push(Output::NoteOn(note.note_id, sample_offset));
        }

        fn note_off(&mut self, note: ChasedNote, sample_offset: u32) {
            self.output
                .push(Output::NoteOff(note.note_id, sample_offset));
        }

        fn audio_start(&mut self, clip: ChasedAudioClip, sample_offset: u32) {
            self.output.push(Output::AudioStart(
                clip.descriptor.clip_id,
                clip.source_position_frame,
                sample_offset,
            ));
        }

        fn audio_stop(&mut self, clip: ChasedAudioClip, sample_offset: u32) {
            self.output
                .push(Output::AudioStop(clip.descriptor.clip_id, sample_offset));
        }

        fn automation_chase_value(&mut self, value: TimelineAutomationChaseValue) {
            self.output.push(Output::AutomationChase(value));
        }

        fn automation_transition(&mut self, transition: TimelineAutomationTransition) {
            self.output.push(Output::AutomationTransition(transition));
        }

        fn automation_block_endpoint(&mut self, endpoint: TimelineAutomationBlockEndpoint) {
            self.output.push(Output::AutomationEndpoint(endpoint));
        }
    }

    fn source() -> NoteSourceDescriptor {
        NoteSourceDescriptor::ChannelStep {
            clip_id: 1,
            pattern_id: 2,
            step: 3,
            repetition: 0,
        }
    }

    fn note(note_id: u64, midi_note: u8) -> ChasedNote {
        ChasedNote {
            note_id,
            channel_id: 7,
            note: midi_note,
            velocity: 0.8,
            gain: 1.0,
            mixer_track: 2,
            source: source(),
        }
    }

    fn audio(clip_id: u32, source_position_frame: f64) -> ChasedAudioClip {
        ChasedAudioClip {
            descriptor: AudioClipDescriptor {
                clip_id,
                asset_id: 99,
                start_frame: 0,
                source_offset_frame: 10,
                source_sample_rate: 48_000,
                clip_end_frame: 1_000,
                stop_frame: 1_000,
                gain: 1.0,
                fade_in_frames: 0,
                fade_out_frames: 0,
                mixer_track: 3,
            },
            source_position_frame,
        }
    }

    fn base(target: CompiledAutomationTarget, value: f32) -> AutomationBaseValue {
        AutomationBaseValue { target, value }
    }

    fn chase(frame: u64, bases: Vec<AutomationBaseValue>) -> TimelineDiscontinuityState {
        TimelineDiscontinuityState {
            frame,
            audio_clips: Vec::new(),
            automation_layers: Vec::new(),
            automation_bases: bases,
            notes: Vec::new(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn ramp(
        target: CompiledAutomationTarget,
        automation_id: u64,
        placement_id: Option<u32>,
        precedence: u64,
        start_value: f32,
        end_value: f32,
        end_frame: u64,
        shape: AutomationRampShape,
    ) -> AutomationRampDescriptor {
        AutomationRampDescriptor {
            automation_id,
            placement_id,
            precedence,
            target,
            start_value,
            end_value,
            end_frame,
            shape,
            source_curve: AutomationCurve::Linear,
        }
    }

    fn event(sample_offset: u16, kind: TimelineEventKind) -> TimelinePacketEvent {
        TimelinePacketEvent {
            sample_offset,
            kind,
        }
    }

    fn transition(
        target: CompiledAutomationTarget,
        before_value: f32,
        after_value: f32,
        after_shape: AutomationRampShape,
        sample_offset: u32,
    ) -> Output {
        Output::AutomationTransition(TimelineAutomationTransition {
            target,
            before_value,
            after_value,
            after_shape,
            sample_offset,
        })
    }

    fn endpoint(target: CompiledAutomationTarget, value: f32) -> Output {
        Output::AutomationEndpoint(TimelineAutomationBlockEndpoint { target, value })
    }

    fn chased(target: CompiledAutomationTarget, value: f32, shape: AutomationRampShape) -> Output {
        Output::AutomationChase(TimelineAutomationChaseValue {
            target,
            value,
            shape,
        })
    }

    fn install(
        executor: &mut TimelineExecutor,
        state: &TimelineDiscontinuityState,
        sink: &mut MockSink,
    ) {
        executor.reset_from_chase(4, state, sink).unwrap();
        sink.output.clear();
    }

    #[test]
    fn same_frame_automation_events_preserve_packet_order() {
        let target = CompiledAutomationTarget::MasterVolume;
        let mut executor = TimelineExecutor::new();
        let mut sink = MockSink::default();
        install(&mut executor, &chase(0, vec![base(target, 1.0)]), &mut sink);
        let events = [
            event(
                3,
                TimelineEventKind::AutomationRamp(ramp(
                    target,
                    1,
                    None,
                    0,
                    0.2,
                    0.2,
                    20,
                    AutomationRampShape::Hold,
                )),
            ),
            event(
                3,
                TimelineEventKind::AutomationRamp(ramp(
                    target,
                    2,
                    None,
                    1,
                    0.8,
                    0.8,
                    20,
                    AutomationRampShape::Hold,
                )),
            ),
        ];
        executor
            .process_event_slice(4, 0, 8, &events, &mut sink)
            .unwrap();
        assert_eq!(
            sink.output,
            vec![
                transition(target, 1.0, 0.2, AutomationRampShape::Hold, 3),
                transition(target, 0.2, 0.8, AutomationRampShape::Hold, 3)
            ]
        );
    }

    #[test]
    fn overlapping_same_pitch_notes_are_tracked_by_occurrence_id() {
        let mut executor = TimelineExecutor::new();
        let mut sink = MockSink::default();
        install(&mut executor, &chase(0, Vec::new()), &mut sink);
        let first = note(10, 60);
        let second = note(11, 60);
        let events = [
            event(
                0,
                TimelineEventKind::NoteOn {
                    note_id: first.note_id,
                    channel_id: first.channel_id,
                    note: first.note,
                    velocity: first.velocity,
                    gain: first.gain,
                    mixer_track: first.mixer_track,
                    source: first.source,
                },
            ),
            event(
                1,
                TimelineEventKind::NoteOn {
                    note_id: second.note_id,
                    channel_id: second.channel_id,
                    note: second.note,
                    velocity: second.velocity,
                    gain: second.gain,
                    mixer_track: second.mixer_track,
                    source: second.source,
                },
            ),
            event(
                2,
                TimelineEventKind::NoteOff {
                    note_id: first.note_id,
                    channel_id: first.channel_id,
                    note: first.note,
                    mixer_track: first.mixer_track,
                    source: first.source,
                },
            ),
        ];
        executor
            .process_event_slice(4, 0, 8, &events, &mut sink)
            .unwrap();
        assert_eq!(executor.active_note_count(), 1);
        assert_eq!(
            sink.output,
            vec![
                Output::NoteOn(10, 0),
                Output::NoteOn(11, 1),
                Output::NoteOff(10, 2)
            ]
        );
    }

    #[test]
    fn reset_chases_audio_at_fractional_native_position() {
        let mut state = chase(40, Vec::new());
        state.audio_clips.push(audio(7, 123.5));
        let mut executor = TimelineExecutor::new();
        let mut sink = MockSink::default();
        executor.reset_from_chase(9, &state, &mut sink).unwrap();
        assert_eq!(executor.next_frame(), 40);
        assert_eq!(executor.active_audio_clip_count(), 1);
        assert_eq!(sink.output, vec![Output::AudioStart(7, 123.5, 0)]);
    }

    #[test]
    fn chase_value_and_block_endpoint_are_distinct_typed_operations() {
        let target = CompiledAutomationTarget::MasterVolume;
        let mut state = chase(4, vec![base(target, 0.1)]);
        state.automation_layers.push(ChasedAutomationLayer {
            automation_id: 1,
            placement_id: Some(2),
            precedence: 3,
            target,
            current_value: 0.25,
            end_value: 0.75,
            end_frame: 12,
            shape: AutomationRampShape::Linear,
            source_curve: AutomationCurve::Linear,
        });
        let mut executor = TimelineExecutor::new();
        let mut sink = MockSink::default();

        executor.reset_from_chase(4, &state, &mut sink).unwrap();
        executor.finish_block(4, 4, &mut sink).unwrap();

        assert_eq!(
            sink.output,
            vec![
                chased(target, 0.25, AutomationRampShape::Linear),
                endpoint(target, 0.5),
            ]
        );
        assert!(
            !sink
                .output
                .iter()
                .any(|output| matches!(output, Output::AutomationTransition(_)))
        );
    }

    #[test]
    fn linear_ramp_emits_interpolated_event_and_block_end_values() {
        let target = CompiledAutomationTarget::MasterPan;
        let mut executor = TimelineExecutor::new();
        let mut sink = MockSink::default();
        install(
            &mut executor,
            &chase(0, vec![base(target, -1.0)]),
            &mut sink,
        );
        let events = [
            event(
                0,
                TimelineEventKind::AutomationRamp(ramp(
                    target,
                    1,
                    None,
                    0,
                    0.0,
                    1.0,
                    10,
                    AutomationRampShape::Linear,
                )),
            ),
            event(
                5,
                TimelineEventKind::AutomationRamp(ramp(
                    target,
                    1,
                    None,
                    0,
                    0.5,
                    1.0,
                    10,
                    AutomationRampShape::Linear,
                )),
            ),
        ];
        executor
            .process_event_slice(4, 0, 10, &events, &mut sink)
            .unwrap();
        executor.finish_block(0, 10, &mut sink).unwrap();
        assert_eq!(
            sink.output,
            vec![
                transition(target, -1.0, 0.0, AutomationRampShape::Linear, 0),
                transition(target, 0.5, 0.5, AutomationRampShape::Linear, 5),
                endpoint(target, 1.0)
            ]
        );
    }

    #[test]
    fn hold_ramp_at_offset_three_keeps_old_value_then_steps() {
        let target = CompiledAutomationTarget::Swing;
        let mut executor = TimelineExecutor::new();
        let mut sink = MockSink::default();
        install(&mut executor, &chase(0, vec![base(target, 0.0)]), &mut sink);
        let events = [event(
            3,
            TimelineEventKind::AutomationRamp(ramp(
                target,
                3,
                None,
                0,
                0.25,
                0.9,
                16,
                AutomationRampShape::Hold,
            )),
        )];
        executor
            .process_event_slice(4, 0, 8, &events, &mut sink)
            .unwrap();
        executor.finish_block(0, 8, &mut sink).unwrap();
        assert_eq!(
            sink.output,
            vec![
                transition(target, 0.0, 0.25, AutomationRampShape::Hold, 3),
                endpoint(target, 0.25),
            ]
        );
    }

    #[test]
    fn ending_high_precedence_layer_restores_updated_lower_layer() {
        let target = CompiledAutomationTarget::MasterVolume;
        let mut executor = TimelineExecutor::new();
        let mut sink = MockSink::default();
        install(&mut executor, &chase(0, vec![base(target, 0.1)]), &mut sink);
        let events = [
            event(
                0,
                TimelineEventKind::AutomationRamp(ramp(
                    target,
                    1,
                    Some(1),
                    1,
                    0.4,
                    0.4,
                    20,
                    AutomationRampShape::Hold,
                )),
            ),
            event(
                0,
                TimelineEventKind::AutomationRamp(ramp(
                    target,
                    2,
                    Some(2),
                    2,
                    0.9,
                    0.9,
                    20,
                    AutomationRampShape::Hold,
                )),
            ),
            event(
                4,
                TimelineEventKind::AutomationRamp(ramp(
                    target,
                    1,
                    Some(1),
                    1,
                    0.6,
                    0.8,
                    20,
                    AutomationRampShape::Linear,
                )),
            ),
            event(
                4,
                TimelineEventKind::AutomationEnd {
                    automation_id: 2,
                    placement_id: Some(2),
                    precedence: 2,
                    target,
                },
            ),
        ];
        executor
            .process_event_slice(4, 0, 8, &events, &mut sink)
            .unwrap();
        assert_eq!(
            sink.output.last(),
            Some(&transition(
                target,
                0.9,
                0.6,
                AutomationRampShape::Linear,
                4
            ))
        );
    }

    #[test]
    fn ending_last_layer_restores_compiled_base() {
        let target = CompiledAutomationTarget::MasterVolume;
        let mut executor = TimelineExecutor::new();
        let mut sink = MockSink::default();
        install(
            &mut executor,
            &chase(0, vec![base(target, 0.35)]),
            &mut sink,
        );
        let events = [
            event(
                0,
                TimelineEventKind::AutomationRamp(ramp(
                    target,
                    1,
                    Some(4),
                    0,
                    0.8,
                    0.8,
                    4,
                    AutomationRampShape::Hold,
                )),
            ),
            event(
                3,
                TimelineEventKind::AutomationEnd {
                    automation_id: 1,
                    placement_id: Some(4),
                    precedence: 0,
                    target,
                },
            ),
        ];
        executor
            .process_event_slice(4, 0, 4, &events, &mut sink)
            .unwrap();
        assert_eq!(
            sink.output.last(),
            Some(&transition(target, 0.8, 0.35, AutomationRampShape::Hold, 3))
        );
    }

    #[test]
    fn automation_end_removes_exact_placement_only() {
        let target = CompiledAutomationTarget::MasterVolume;
        let mut state = chase(5, vec![base(target, 0.1)]);
        state.automation_layers.extend([
            ChasedAutomationLayer {
                automation_id: 8,
                placement_id: Some(10),
                precedence: 2,
                target,
                current_value: 0.6,
                end_value: 0.6,
                end_frame: 20,
                shape: AutomationRampShape::Hold,
                source_curve: AutomationCurve::Hold,
            },
            ChasedAutomationLayer {
                automation_id: 8,
                placement_id: Some(11),
                precedence: 2,
                target,
                current_value: 0.9,
                end_value: 0.9,
                end_frame: 20,
                shape: AutomationRampShape::Hold,
                source_curve: AutomationCurve::Hold,
            },
        ]);
        let mut executor = TimelineExecutor::new();
        let mut sink = MockSink::default();
        install(&mut executor, &state, &mut sink);
        let events = [event(
            0,
            TimelineEventKind::AutomationEnd {
                automation_id: 8,
                placement_id: Some(11),
                precedence: 2,
                target,
            },
        )];
        executor
            .process_event_slice(4, 5, 2, &events, &mut sink)
            .unwrap();
        assert_eq!(executor.active_automation_layer_count(), 1);
        assert_eq!(
            sink.output,
            vec![transition(target, 0.9, 0.6, AutomationRampShape::Hold, 0)]
        );
    }

    #[test]
    fn mute_automation_uses_half_open_threshold() {
        let target = CompiledAutomationTarget::MixerMute { track: 3 };
        let mut executor = TimelineExecutor::new();
        let mut sink = MockSink::default();
        install(
            &mut executor,
            &chase(0, vec![base(target, 0.49)]),
            &mut sink,
        );
        let events = [event(
            1,
            TimelineEventKind::AutomationRamp(ramp(
                target,
                1,
                None,
                0,
                0.5,
                0.5,
                8,
                AutomationRampShape::Hold,
            )),
        )];
        executor
            .process_event_slice(4, 0, 4, &events, &mut sink)
            .unwrap();
        assert_eq!(
            sink.output,
            vec![transition(target, 0.0, 1.0, AutomationRampShape::Hold, 1)]
        );
    }

    #[test]
    fn reset_capacity_failure_preserves_previous_epoch_and_state() {
        let mut executor = TimelineExecutor::new();
        let mut sink = MockSink::default();
        let mut initial = chase(0, Vec::new());
        initial.notes.push(note(1, 60));
        executor.reset_from_chase(2, &initial, &mut sink).unwrap();
        sink.output.clear();

        let mut oversized = chase(50, Vec::new());
        oversized.notes = (0..=MAX_ACTIVE_NOTES)
            .map(|index| note(index as u64 + 10, 61))
            .collect();
        assert_eq!(
            executor
                .reset_from_chase(3, &oversized, &mut sink)
                .unwrap_err(),
            TimelineExecutorError::CapacityExceeded {
                resource: TimelineExecutorCapacity::Notes,
                capacity: MAX_ACTIVE_NOTES,
            }
        );
        assert_eq!(executor.epoch(), 2);
        assert_eq!(executor.active_note_count(), 1);
        assert!(sink.output.is_empty());
    }

    #[test]
    fn epoch_mismatch_rejects_packet_without_mutation_or_sink_calls() {
        let mut executor = TimelineExecutor::new();
        let mut sink = MockSink::default();
        install(&mut executor, &chase(0, Vec::new()), &mut sink);
        let packet = TimelinePacket::<0>::new();
        assert_eq!(
            executor.process_packet(&packet, &mut sink).unwrap_err(),
            TimelineExecutorError::EpochMismatch {
                expected: 4,
                received: 0,
            }
        );
        assert_eq!(executor.next_frame(), 0);
        assert!(sink.output.is_empty());
    }

    #[test]
    fn metadata_mismatch_is_transactional() {
        let mut executor = TimelineExecutor::new();
        let mut sink = MockSink::default();
        install(&mut executor, &chase(10, Vec::new()), &mut sink);
        let events = [event(
            0,
            TimelineEventKind::NoteOn {
                note_id: 2,
                channel_id: 7,
                note: 60,
                velocity: 1.0,
                gain: 1.0,
                mixer_track: 2,
                source: source(),
            },
        )];
        assert_eq!(
            executor
                .process_event_slice(4, 11, 8, &events, &mut sink)
                .unwrap_err(),
            TimelineExecutorError::BlockStartMismatch {
                expected: 10,
                received: 11,
            }
        );
        assert_eq!(executor.active_note_count(), 0);
        assert!(sink.output.is_empty());
    }

    #[test]
    fn nonfinite_event_fails_closed_without_partial_packet_output() {
        let mut executor = TimelineExecutor::new();
        let mut sink = MockSink::default();
        install(&mut executor, &chase(0, Vec::new()), &mut sink);
        let valid = note(1, 60);
        let events = [
            event(
                0,
                TimelineEventKind::NoteOn {
                    note_id: valid.note_id,
                    channel_id: valid.channel_id,
                    note: valid.note,
                    velocity: valid.velocity,
                    gain: valid.gain,
                    mixer_track: valid.mixer_track,
                    source: valid.source,
                },
            ),
            event(
                1,
                TimelineEventKind::NoteOn {
                    note_id: 2,
                    channel_id: 7,
                    note: 61,
                    velocity: f32::NAN,
                    gain: 1.0,
                    mixer_track: 2,
                    source: source(),
                },
            ),
        ];
        assert_eq!(
            executor
                .process_event_slice(4, 0, 8, &events, &mut sink)
                .unwrap_err(),
            TimelineExecutorError::NonFiniteValue
        );
        assert_eq!(executor.active_note_count(), 0);
        assert!(sink.output.is_empty());
    }

    #[test]
    fn failed_automation_packet_does_not_emit_or_pollute_active_state() {
        let target = CompiledAutomationTarget::MasterVolume;
        let mut executor = TimelineExecutor::new();
        let mut sink = MockSink::default();
        install(&mut executor, &chase(0, vec![base(target, 0.1)]), &mut sink);
        let before_stats = executor.stats();
        let invalid = [
            event(
                1,
                TimelineEventKind::AutomationRamp(ramp(
                    target,
                    10,
                    Some(1),
                    0,
                    0.8,
                    0.8,
                    8,
                    AutomationRampShape::Hold,
                )),
            ),
            event(
                2,
                TimelineEventKind::AutomationEnd {
                    automation_id: 999,
                    placement_id: Some(9),
                    precedence: 0,
                    target,
                },
            ),
        ];

        assert_eq!(
            executor
                .process_event_slice(4, 0, 8, &invalid, &mut sink)
                .unwrap_err(),
            TimelineExecutorError::AutomationLayerNotFound
        );
        assert!(sink.output.is_empty());
        assert_eq!(executor.active_automation_layer_count(), 0);
        assert_eq!(executor.stats(), before_stats);

        let valid = [event(
            2,
            TimelineEventKind::AutomationRamp(ramp(
                target,
                11,
                Some(2),
                0,
                0.4,
                0.4,
                8,
                AutomationRampShape::Hold,
            )),
        )];
        executor
            .process_event_slice(4, 0, 8, &valid, &mut sink)
            .unwrap();
        assert_eq!(
            sink.output,
            vec![transition(target, 0.1, 0.4, AutomationRampShape::Hold, 2)]
        );
    }

    #[test]
    fn decreasing_offsets_across_chunks_fail_closed() {
        let mut executor = TimelineExecutor::new();
        let mut sink = MockSink::default();
        install(&mut executor, &chase(0, Vec::new()), &mut sink);
        let first = note(1, 60);
        let first_event = [event(
            5,
            TimelineEventKind::NoteOn {
                note_id: first.note_id,
                channel_id: first.channel_id,
                note: first.note,
                velocity: first.velocity,
                gain: first.gain,
                mixer_track: first.mixer_track,
                source: first.source,
            },
        )];
        executor
            .process_event_slice(4, 0, 8, &first_event, &mut sink)
            .unwrap();
        sink.output.clear();
        let second = note(2, 62);
        let second_event = [event(
            4,
            TimelineEventKind::NoteOn {
                note_id: second.note_id,
                channel_id: second.channel_id,
                note: second.note,
                velocity: second.velocity,
                gain: second.gain,
                mixer_track: second.mixer_track,
                source: second.source,
            },
        )];
        assert_eq!(
            executor
                .process_event_slice(4, 0, 8, &second_event, &mut sink)
                .unwrap_err(),
            TimelineExecutorError::EventOrderViolation {
                previous: 5,
                received: 4,
            }
        );
        assert_eq!(executor.active_note_count(), 1);
        assert!(sink.output.is_empty());
    }

    #[test]
    fn maximum_block_uses_typed_endpoint_before_next_block_offset_zero() {
        let target = CompiledAutomationTarget::MasterVolume;
        let mut executor = TimelineExecutor::new();
        let mut sink = MockSink::default();
        install(&mut executor, &chase(0, vec![base(target, 1.0)]), &mut sink);

        let last_sample = [event(
            u16::MAX,
            TimelineEventKind::AutomationRamp(ramp(
                target,
                41,
                None,
                0,
                0.25,
                0.25,
                65_536,
                AutomationRampShape::Hold,
            )),
        )];
        executor
            .process_event_slice(4, 0, 65_536, &last_sample, &mut sink)
            .unwrap();
        executor.finish_block(0, 65_536, &mut sink).unwrap();

        let next_block_zero = [event(
            0,
            TimelineEventKind::AutomationRamp(ramp(
                target,
                41,
                None,
                0,
                0.75,
                0.75,
                65_540,
                AutomationRampShape::Hold,
            )),
        )];
        executor
            .process_event_slice(4, 65_536, 4, &next_block_zero, &mut sink)
            .unwrap();

        assert_eq!(
            sink.output,
            vec![
                transition(
                    target,
                    1.0,
                    0.25,
                    AutomationRampShape::Hold,
                    u32::from(u16::MAX)
                ),
                endpoint(target, 0.25),
                transition(target, 0.25, 0.75, AutomationRampShape::Hold, 0),
            ]
        );
    }

    #[test]
    fn maximum_winner_cache_finishes_with_no_layer_rescan() {
        let mut state = chase(0, Vec::with_capacity(MAX_AUTOMATION_BASES));
        state
            .automation_layers
            .reserve_exact(MAX_ACTIVE_AUTOMATION_LAYERS);
        for index in 0..MAX_AUTOMATION_BASES {
            let stable_id = u64::try_from(index).unwrap() + 1;
            let target = CompiledAutomationTarget::PluginParameter {
                instance_id: stable_id,
                parameter_id: 0,
            };
            state.automation_bases.push(base(target, 0.1));
            state.automation_layers.push(ChasedAutomationLayer {
                automation_id: stable_id,
                placement_id: None,
                precedence: stable_id,
                target,
                current_value: 0.5,
                end_value: 0.75,
                end_frame: 1_000,
                shape: AutomationRampShape::Linear,
                source_curve: AutomationCurve::Linear,
            });
        }

        let mut executor = TimelineExecutor::new();
        let mut sink = MockSink::default();
        executor.reset_from_chase(7, &state, &mut sink).unwrap();
        sink.output.clear();
        let before = executor.stats();
        executor.finish_block(0, 64, &mut sink).unwrap();
        let after = executor.stats();

        assert_eq!(sink.output.len(), MAX_AUTOMATION_BASES);
        assert!(
            sink.output
                .iter()
                .all(|output| matches!(output, Output::AutomationEndpoint(_)))
        );
        assert_eq!(
            after.winner_layer_slots_scanned, before.winner_layer_slots_scanned,
            "finish_block must use cached winners rather than scan layers"
        );
        assert_eq!(
            after.block_endpoints_emitted - before.block_endpoints_emitted,
            MAX_AUTOMATION_BASES as u64
        );
        assert_eq!(
            std::mem::size_of::<TimelineExecutor>(),
            2 * std::mem::size_of::<Box<()>>()
        );
        assert!(TimelineExecutor::state_bytes_per_buffer() > 64 * 1_024);
        assert_eq!(
            TimelineExecutor::preallocated_state_bytes(),
            TimelineExecutor::state_bytes_per_buffer() * 2
        );
    }
}
