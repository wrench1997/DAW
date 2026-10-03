//! Allocation-free, transactional rendering of compiled Timeline automation.
//!
//! The Timeline executor emits exact control points inside a block and one
//! exclusive endpoint for every compiled target. This module turns those
//! values into deterministic piecewise-linear per-frame values without doing
//! allocation, hashing through the standard library, or mutating committed
//! state until the caller explicitly commits a fully rendered block.

use std::fmt;

use crate::{
    timeline::{
        AutomationBaseValue, AutomationRampShape, CompiledAutomationTarget,
        TIMELINE_CALLBACK_MAX_EVENTS,
    },
    timeline_executor::{
        MAX_AUTOMATION_BASES, TimelineAutomationBlockEndpoint, TimelineAutomationChaseValue,
        TimelineAutomationTransition,
    },
};

pub const MAX_AUTOMATION_CONTROL_POINTS_PER_BLOCK: usize = TIMELINE_CALLBACK_MAX_EVENTS;
pub const MAX_AUTOMATION_BLOCK_FRAMES: u32 = u16::MAX as u32 + 1;

const TARGET_MAP_CAPACITY: usize = 1_024;
const NO_POINT: u16 = u16::MAX;

const _: () = assert!(MAX_AUTOMATION_BASES <= u16::MAX as usize);
const _: () = assert!(MAX_AUTOMATION_CONTROL_POINTS_PER_BLOCK < u16::MAX as usize);
const _: () = assert!(TARGET_MAP_CAPACITY.is_power_of_two());
const _: () = assert!(TARGET_MAP_CAPACITY >= MAX_AUTOMATION_BASES * 2);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimelineAutomationCapacity {
    Targets,
    ControlPoints,
    Endpoints,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimelineAutomationError {
    ZeroEpoch,
    StaleEpoch {
        latest: u64,
        received: u64,
    },
    ZeroFrames,
    FrameCountTooLarge {
        frames: u32,
        maximum: u32,
    },
    FrameRangeOverflow,
    NonFiniteValue,
    CapacityExceeded {
        resource: TimelineAutomationCapacity,
        maximum: usize,
    },
    DuplicateTarget {
        target: CompiledAutomationTarget,
    },
    UnknownTarget {
        target: CompiledAutomationTarget,
    },
    DuplicateEndpoint {
        target: CompiledAutomationTarget,
    },
    MissingEndpoint {
        target: CompiledAutomationTarget,
    },
    EventOffsetOutOfRange {
        offset: u32,
        frames: u32,
    },
    EventOrderViolation {
        previous: u32,
        received: u32,
    },
    EpochMismatch {
        expected: u64,
        received: u64,
    },
    BlockStartMismatch {
        expected: u64,
        received: u64,
    },
    TransactionAlreadyOpen,
    NoOpenTransaction,
    NoFinishedTransaction,
    FrameOrderViolation {
        expected: u32,
        received: u32,
    },
    BlockNotFullyRendered {
        rendered: u32,
        frames: u32,
    },
}

impl fmt::Display for TimelineAutomationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for TimelineAutomationError {}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TimelineAutomationStats {
    pub installed_targets: usize,
    pub rendered_blocks: u64,
    pub rendered_frames: u64,
    pub control_points: u64,
    pub aborted_blocks: u64,
}

/// Fixed-capacity callback plan. The executor may fill this while its own
/// transaction is pending; [`RealtimeTimelineAutomation::begin_block`] performs
/// a complete dry validation before any automation value is exposed to audio.
pub struct TimelineAutomationBlockPlan {
    transitions:
        Box<[Option<TimelineAutomationTransition>; MAX_AUTOMATION_CONTROL_POINTS_PER_BLOCK]>,
    endpoints: Box<[Option<TimelineAutomationBlockEndpoint>; MAX_AUTOMATION_BASES]>,
    transition_len: usize,
    endpoint_len: usize,
    frames: u32,
    previous_offset: Option<u32>,
}

impl TimelineAutomationBlockPlan {
    #[must_use]
    pub fn new() -> Self {
        Self {
            transitions: Box::new([None; MAX_AUTOMATION_CONTROL_POINTS_PER_BLOCK]),
            endpoints: Box::new([None; MAX_AUTOMATION_BASES]),
            transition_len: 0,
            endpoint_len: 0,
            frames: 0,
            previous_offset: None,
        }
    }

    pub fn reset(&mut self, frames: u32) -> Result<(), TimelineAutomationError> {
        validate_frames(frames)?;
        self.transition_len = 0;
        self.endpoint_len = 0;
        self.frames = frames;
        self.previous_offset = None;
        Ok(())
    }

    pub fn push_transition(
        &mut self,
        transition: TimelineAutomationTransition,
    ) -> Result<(), TimelineAutomationError> {
        if !transition.before_value.is_finite() || !transition.after_value.is_finite() {
            return Err(TimelineAutomationError::NonFiniteValue);
        }
        if transition.sample_offset >= self.frames {
            return Err(TimelineAutomationError::EventOffsetOutOfRange {
                offset: transition.sample_offset,
                frames: self.frames,
            });
        }
        if let Some(previous) = self.previous_offset
            && transition.sample_offset < previous
        {
            return Err(TimelineAutomationError::EventOrderViolation {
                previous,
                received: transition.sample_offset,
            });
        }
        let Some(slot) = self.transitions.get_mut(self.transition_len) else {
            return Err(TimelineAutomationError::CapacityExceeded {
                resource: TimelineAutomationCapacity::ControlPoints,
                maximum: MAX_AUTOMATION_CONTROL_POINTS_PER_BLOCK,
            });
        };
        *slot = Some(transition);
        self.transition_len += 1;
        self.previous_offset = Some(transition.sample_offset);
        Ok(())
    }

    pub fn push_block_endpoint(
        &mut self,
        endpoint: TimelineAutomationBlockEndpoint,
    ) -> Result<(), TimelineAutomationError> {
        if !endpoint.value.is_finite() {
            return Err(TimelineAutomationError::NonFiniteValue);
        }
        let Some(slot) = self.endpoints.get_mut(self.endpoint_len) else {
            return Err(TimelineAutomationError::CapacityExceeded {
                resource: TimelineAutomationCapacity::Endpoints,
                maximum: MAX_AUTOMATION_BASES,
            });
        };
        *slot = Some(endpoint);
        self.endpoint_len += 1;
        Ok(())
    }

    #[must_use]
    pub const fn frames(&self) -> u32 {
        self.frames
    }

    #[must_use]
    pub fn transitions(&self) -> &[Option<TimelineAutomationTransition>] {
        &self.transitions[..self.transition_len]
    }

    #[must_use]
    pub fn endpoints(&self) -> &[Option<TimelineAutomationBlockEndpoint>] {
        &self.endpoints[..self.endpoint_len]
    }
}

impl Default for TimelineAutomationBlockPlan {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy)]
struct TargetSlot {
    target: CompiledAutomationTarget,
    value: f32,
    shape: AutomationRampShape,
}

#[derive(Clone, Copy)]
struct AutomationState {
    targets: [Option<TargetSlot>; MAX_AUTOMATION_BASES],
    target_count: usize,
    epoch: u64,
    next_frame: u64,
    stats: TimelineAutomationStats,
}

impl AutomationState {
    const fn empty() -> Self {
        Self {
            targets: [None; MAX_AUTOMATION_BASES],
            target_count: 0,
            epoch: 0,
            next_frame: 0,
            stats: TimelineAutomationStats {
                installed_targets: 0,
                rendered_blocks: 0,
                rendered_frames: 0,
                control_points: 0,
                aborted_blocks: 0,
            },
        }
    }

    fn clear_for_reset(
        &mut self,
        epoch: u64,
        next_frame: u64,
        target_count: usize,
        mut stats: TimelineAutomationStats,
    ) {
        self.targets.fill(None);
        self.target_count = target_count;
        self.epoch = epoch;
        self.next_frame = next_frame;
        stats.installed_targets = target_count;
        self.stats = stats;
    }

    fn copy_from(&mut self, other: &Self) {
        self.targets.copy_from_slice(&other.targets);
        self.target_count = other.target_count;
        self.epoch = other.epoch;
        self.next_frame = other.next_frame;
        self.stats = other.stats;
    }
}

#[derive(Clone, Copy)]
struct TargetMapEntry {
    target: CompiledAutomationTarget,
    slot: u16,
}

#[derive(Clone, Copy)]
struct ResolvedTransition {
    offset: u16,
    before_value: f32,
    after_value: f32,
    after_shape: AutomationRampShape,
    next: u16,
}

const EMPTY_RESOLVED_TRANSITION: ResolvedTransition = ResolvedTransition {
    offset: 0,
    before_value: 0.0,
    after_value: 0.0,
    after_shape: AutomationRampShape::Hold,
    next: NO_POINT,
};

#[derive(Clone, Copy)]
struct BlockTarget {
    point: u16,
    segment_offset: u16,
    segment_value: f32,
    segment_shape: AutomationRampShape,
    endpoint: f32,
}

const EMPTY_BLOCK_TARGET: BlockTarget = BlockTarget {
    point: NO_POINT,
    segment_offset: 0,
    segment_value: 0.0,
    segment_shape: AutomationRampShape::Hold,
    endpoint: 0.0,
};

#[derive(Clone, Copy)]
struct OpenBlock {
    start_frame: u64,
    frames: u32,
    next_offset: u32,
    control_points: usize,
}

/// Transactional per-frame automation renderer. `new` and `reset_from_chase`
/// belong on the control/callback boundary; steady-state block methods perform
/// no allocation, locking, I/O, or destruction of project-owned resources.
pub struct RealtimeTimelineAutomation {
    active: Box<AutomationState>,
    scratch: Box<AutomationState>,
    target_map: Box<[Option<TargetMapEntry>; TARGET_MAP_CAPACITY]>,
    scratch_target_map: Box<[Option<TargetMapEntry>; TARGET_MAP_CAPACITY]>,
    resolved_transitions: Box<[ResolvedTransition; MAX_AUTOMATION_CONTROL_POINTS_PER_BLOCK]>,
    heads: Box<[u16; MAX_AUTOMATION_BASES]>,
    block_targets: Box<[BlockTarget; MAX_AUTOMATION_BASES]>,
    endpoint_seen: Box<[bool; MAX_AUTOMATION_BASES]>,
    open: Option<OpenBlock>,
    finished: bool,
    staged_reset: bool,
}

impl RealtimeTimelineAutomation {
    #[must_use]
    pub fn new() -> Self {
        Self {
            active: Box::new(AutomationState::empty()),
            scratch: Box::new(AutomationState::empty()),
            target_map: Box::new([None; TARGET_MAP_CAPACITY]),
            scratch_target_map: Box::new([None; TARGET_MAP_CAPACITY]),
            resolved_transitions: Box::new(
                [EMPTY_RESOLVED_TRANSITION; MAX_AUTOMATION_CONTROL_POINTS_PER_BLOCK],
            ),
            heads: Box::new([NO_POINT; MAX_AUTOMATION_BASES]),
            block_targets: Box::new([EMPTY_BLOCK_TARGET; MAX_AUTOMATION_BASES]),
            endpoint_seen: Box::new([false; MAX_AUTOMATION_BASES]),
            open: None,
            finished: false,
            staged_reset: false,
        }
    }

    /// Atomically resets target identity and chased values for a new epoch.
    /// `values` must contain exactly one resolved value for every base target.
    pub fn reset_from_chase(
        &mut self,
        epoch: u64,
        frame: u64,
        bases: &[AutomationBaseValue],
        values: &[TimelineAutomationChaseValue],
    ) -> Result<(), TimelineAutomationError> {
        self.stage_reset_from_chase(epoch, frame, bases, values)?;
        self.commit_staged_reset();
        Ok(())
    }

    /// Validates a chased target set into scratch storage without changing the
    /// committed epoch, frame, values, shapes, or target map.
    pub(crate) fn stage_reset_from_chase(
        &mut self,
        epoch: u64,
        frame: u64,
        bases: &[AutomationBaseValue],
        values: &[TimelineAutomationChaseValue],
    ) -> Result<(), TimelineAutomationError> {
        if self.open.is_some() || self.finished || self.staged_reset {
            return Err(TimelineAutomationError::TransactionAlreadyOpen);
        }
        if epoch == 0 {
            return Err(TimelineAutomationError::ZeroEpoch);
        }
        if self.active.epoch != 0 && epoch <= self.active.epoch {
            return Err(TimelineAutomationError::StaleEpoch {
                latest: self.active.epoch,
                received: epoch,
            });
        }
        if bases.len() > MAX_AUTOMATION_BASES {
            return Err(TimelineAutomationError::CapacityExceeded {
                resource: TimelineAutomationCapacity::Targets,
                maximum: MAX_AUTOMATION_BASES,
            });
        }
        if values.len() > MAX_AUTOMATION_BASES {
            return Err(TimelineAutomationError::CapacityExceeded {
                resource: TimelineAutomationCapacity::Endpoints,
                maximum: MAX_AUTOMATION_BASES,
            });
        }

        self.scratch_target_map.fill(None);
        self.endpoint_seen.fill(false);
        self.scratch
            .clear_for_reset(epoch, frame, bases.len(), self.active.stats);

        for (slot, base) in bases.iter().copied().enumerate() {
            if !base.value.is_finite() {
                return Err(TimelineAutomationError::NonFiniteValue);
            }
            insert_target(&mut self.scratch_target_map, base.target, slot)?;
            self.scratch.targets[slot] = Some(TargetSlot {
                target: base.target,
                value: base.value,
                shape: AutomationRampShape::Hold,
            });
        }
        for value in values.iter().copied() {
            if !value.value.is_finite() {
                return Err(TimelineAutomationError::NonFiniteValue);
            }
            let slot = lookup_target_in(&self.scratch_target_map, value.target).ok_or(
                TimelineAutomationError::UnknownTarget {
                    target: value.target,
                },
            )?;
            if self.endpoint_seen[slot] {
                return Err(TimelineAutomationError::DuplicateEndpoint {
                    target: value.target,
                });
            }
            self.endpoint_seen[slot] = true;
            self.scratch.targets[slot]
                .as_mut()
                .expect("target map and state are built together")
                .value = value.value;
            self.scratch.targets[slot]
                .as_mut()
                .expect("target map and state are built together")
                .shape = value.shape;
        }
        for (slot, base) in bases.iter().enumerate() {
            if !self.endpoint_seen[slot] {
                return Err(TimelineAutomationError::MissingEndpoint {
                    target: base.target,
                });
            }
        }
        self.staged_reset = true;
        Ok(())
    }

    /// Commits the last successful staged reset using only pointer swaps.
    pub(crate) fn commit_staged_reset(&mut self) {
        assert!(self.staged_reset, "reset must be staged before commit");
        std::mem::swap(&mut self.active, &mut self.scratch);
        std::mem::swap(&mut self.target_map, &mut self.scratch_target_map);
        self.staged_reset = false;
    }

    /// Discards a staged reset while preserving committed state byte-for-byte.
    #[allow(dead_code)] // Used by the atomic activation coordinator in the next integration step.
    pub(crate) fn abort_staged_reset(&mut self) {
        self.staged_reset = false;
    }

    pub fn begin_block(
        &mut self,
        epoch: u64,
        start_frame: u64,
        plan: &TimelineAutomationBlockPlan,
    ) -> Result<(), TimelineAutomationError> {
        if self.open.is_some() || self.finished || self.staged_reset {
            return Err(TimelineAutomationError::TransactionAlreadyOpen);
        }
        validate_frames(plan.frames)?;
        if epoch != self.active.epoch {
            return Err(TimelineAutomationError::EpochMismatch {
                expected: self.active.epoch,
                received: epoch,
            });
        }
        if start_frame != self.active.next_frame {
            return Err(TimelineAutomationError::BlockStartMismatch {
                expected: self.active.next_frame,
                received: start_frame,
            });
        }
        start_frame
            .checked_add(u64::from(plan.frames))
            .ok_or(TimelineAutomationError::FrameRangeOverflow)?;

        self.scratch.copy_from(&self.active);
        self.heads.fill(NO_POINT);
        self.endpoint_seen.fill(false);
        self.block_targets.fill(EMPTY_BLOCK_TARGET);

        for endpoint in plan.endpoints().iter().copied().flatten() {
            if !endpoint.value.is_finite() {
                return Err(TimelineAutomationError::NonFiniteValue);
            }
            let slot = self.lookup_target(endpoint.target).ok_or(
                TimelineAutomationError::UnknownTarget {
                    target: endpoint.target,
                },
            )?;
            if self.endpoint_seen[slot] {
                return Err(TimelineAutomationError::DuplicateEndpoint {
                    target: endpoint.target,
                });
            }
            self.endpoint_seen[slot] = true;
            self.block_targets[slot].endpoint = endpoint.value;
        }
        for slot in 0..self.active.target_count {
            let Some(target) = self.active.targets[slot] else {
                continue;
            };
            if !self.endpoint_seen[slot] {
                return Err(TimelineAutomationError::MissingEndpoint {
                    target: target.target,
                });
            }
            self.block_targets[slot].segment_value = target.value;
            self.block_targets[slot].segment_shape = target.shape;
        }

        for index in (0..plan.transition_len).rev() {
            let transition = plan.transitions[index].expect("transition prefix is initialized");
            if !transition.before_value.is_finite() || !transition.after_value.is_finite() {
                return Err(TimelineAutomationError::NonFiniteValue);
            }
            if transition.sample_offset >= plan.frames {
                return Err(TimelineAutomationError::EventOffsetOutOfRange {
                    offset: transition.sample_offset,
                    frames: plan.frames,
                });
            }
            let slot = self.lookup_target(transition.target).ok_or(
                TimelineAutomationError::UnknownTarget {
                    target: transition.target,
                },
            )?;
            let current_head = self.heads[slot];
            if current_head != NO_POINT
                && u32::from(self.resolved_transitions[usize::from(current_head)].offset)
                    == transition.sample_offset
            {
                // Reverse iteration saw the final write first. Only the first
                // transition's `before` belongs to the preceding segment; the
                // final transition's `after` and shape remain sample-visible.
                self.resolved_transitions[usize::from(current_head)].before_value =
                    transition.before_value;
                continue;
            }
            let point_index = u16::try_from(index).expect("point capacity fits u16");
            self.resolved_transitions[index] = ResolvedTransition {
                offset: u16::try_from(transition.sample_offset)
                    .expect("validated transition offset fits u16"),
                before_value: transition.before_value,
                after_value: transition.after_value,
                after_shape: transition.after_shape,
                next: self.heads[slot],
            };
            self.heads[slot] = point_index;
        }
        for slot in 0..self.active.target_count {
            self.block_targets[slot].point = self.heads[slot];
        }
        self.open = Some(OpenBlock {
            start_frame,
            frames: plan.frames,
            next_offset: 0,
            control_points: plan.transition_len,
        });
        self.finished = false;
        Ok(())
    }

    /// Emits every compiled target for one exact sample. The caller decides
    /// which targets affect native DSP and which are forwarded to plug-ins.
    pub fn render_frame(
        &mut self,
        offset: u32,
        mut consume: impl FnMut(CompiledAutomationTarget, f32),
    ) -> Result<(), TimelineAutomationError> {
        let Some(open) = self.open else {
            return Err(TimelineAutomationError::NoOpenTransaction);
        };
        if offset != open.next_offset {
            return Err(TimelineAutomationError::FrameOrderViolation {
                expected: open.next_offset,
                received: offset,
            });
        }
        if offset >= open.frames {
            return Err(TimelineAutomationError::EventOffsetOutOfRange {
                offset,
                frames: open.frames,
            });
        }

        for slot in 0..self.scratch.target_count {
            let block = &mut self.block_targets[slot];
            while block.point != NO_POINT {
                let transition = self.resolved_transitions[usize::from(block.point)];
                if u32::from(transition.offset) != offset {
                    break;
                }
                block.segment_offset = transition.offset;
                block.segment_value = transition.after_value;
                block.segment_shape = transition.after_shape;
                block.point = transition.next;
            }

            let (next_offset, next_value) = if block.point == NO_POINT {
                (open.frames, block.endpoint)
            } else {
                let next = self.resolved_transitions[usize::from(block.point)];
                (u32::from(next.offset), next.before_value)
            };
            let segment_offset = u32::from(block.segment_offset);
            let value = match block.segment_shape {
                AutomationRampShape::Hold => block.segment_value,
                AutomationRampShape::Linear if next_offset <= segment_offset => block.segment_value,
                AutomationRampShape::Linear => {
                    let numerator = offset.saturating_sub(segment_offset) as f64;
                    let denominator = (next_offset - segment_offset) as f64;
                    (f64::from(block.segment_value)
                        + (f64::from(next_value) - f64::from(block.segment_value))
                            * (numerator / denominator)) as f32
                }
            };
            let target = self.scratch.targets[slot]
                .expect("target prefix is initialized")
                .target;
            consume(target, value);
        }
        self.open
            .as_mut()
            .expect("transaction remains open")
            .next_offset += 1;
        Ok(())
    }

    pub fn finish_block(&mut self) -> Result<(), TimelineAutomationError> {
        let Some(open) = self.open else {
            return Err(TimelineAutomationError::NoOpenTransaction);
        };
        if open.next_offset != open.frames {
            return Err(TimelineAutomationError::BlockNotFullyRendered {
                rendered: open.next_offset,
                frames: open.frames,
            });
        }
        for slot in 0..self.scratch.target_count {
            let target = self.scratch.targets[slot]
                .as_mut()
                .expect("target prefix is initialized");
            target.value = self.block_targets[slot].endpoint;
            target.shape = self.block_targets[slot].segment_shape;
        }
        self.scratch.next_frame = open
            .start_frame
            .checked_add(u64::from(open.frames))
            .ok_or(TimelineAutomationError::FrameRangeOverflow)?;
        self.scratch.stats.rendered_blocks = self.scratch.stats.rendered_blocks.saturating_add(1);
        self.scratch.stats.rendered_frames = self
            .scratch
            .stats
            .rendered_frames
            .saturating_add(u64::from(open.frames));
        self.scratch.stats.control_points = self
            .scratch
            .stats
            .control_points
            .saturating_add(open.control_points as u64);
        self.open = None;
        self.finished = true;
        Ok(())
    }

    pub fn commit_block(&mut self) -> Result<(), TimelineAutomationError> {
        if self.open.is_some() {
            return Err(TimelineAutomationError::BlockNotFullyRendered {
                rendered: self.open.map_or(0, |open| open.next_offset),
                frames: self.open.map_or(0, |open| open.frames),
            });
        }
        if !self.finished {
            return Err(TimelineAutomationError::NoFinishedTransaction);
        }
        std::mem::swap(&mut self.active, &mut self.scratch);
        self.finished = false;
        Ok(())
    }

    pub fn abort_block(&mut self) {
        if self.open.take().is_some() || self.finished {
            self.active.stats.aborted_blocks = self.active.stats.aborted_blocks.saturating_add(1);
        }
        self.finished = false;
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
    pub const fn stats(&self) -> TimelineAutomationStats {
        self.active.stats
    }

    #[must_use]
    pub fn value_for(&self, target: CompiledAutomationTarget) -> Option<f32> {
        self.lookup_target(target)
            .and_then(|slot| self.active.targets[slot].map(|target| target.value))
    }

    #[must_use]
    pub fn shape_for(&self, target: CompiledAutomationTarget) -> Option<AutomationRampShape> {
        self.lookup_target(target)
            .and_then(|slot| self.active.targets[slot].map(|target| target.shape))
    }

    fn lookup_target(&self, target: CompiledAutomationTarget) -> Option<usize> {
        lookup_target_in(&self.target_map, target)
    }
}

impl Default for RealtimeTimelineAutomation {
    fn default() -> Self {
        Self::new()
    }
}

fn validate_frames(frames: u32) -> Result<(), TimelineAutomationError> {
    if frames == 0 {
        return Err(TimelineAutomationError::ZeroFrames);
    }
    if frames > MAX_AUTOMATION_BLOCK_FRAMES {
        return Err(TimelineAutomationError::FrameCountTooLarge {
            frames,
            maximum: MAX_AUTOMATION_BLOCK_FRAMES,
        });
    }
    Ok(())
}

fn insert_target(
    map: &mut [Option<TargetMapEntry>; TARGET_MAP_CAPACITY],
    target: CompiledAutomationTarget,
    slot: usize,
) -> Result<(), TimelineAutomationError> {
    let mask = TARGET_MAP_CAPACITY - 1;
    let mut index = target_hash(target) & mask;
    for _ in 0..TARGET_MAP_CAPACITY {
        match map[index] {
            Some(entry) if entry.target == target => {
                return Err(TimelineAutomationError::DuplicateTarget { target });
            }
            Some(_) => index = (index + 1) & mask,
            None => {
                map[index] = Some(TargetMapEntry {
                    target,
                    slot: u16::try_from(slot).expect("target slot fits u16"),
                });
                return Ok(());
            }
        }
    }
    Err(TimelineAutomationError::CapacityExceeded {
        resource: TimelineAutomationCapacity::Targets,
        maximum: MAX_AUTOMATION_BASES,
    })
}

fn lookup_target_in(
    map: &[Option<TargetMapEntry>; TARGET_MAP_CAPACITY],
    target: CompiledAutomationTarget,
) -> Option<usize> {
    let mask = TARGET_MAP_CAPACITY - 1;
    let mut index = target_hash(target) & mask;
    for _ in 0..TARGET_MAP_CAPACITY {
        match map[index] {
            Some(entry) if entry.target == target => return Some(usize::from(entry.slot)),
            Some(_) => index = (index + 1) & mask,
            None => return None,
        }
    }
    None
}

fn target_hash(target: CompiledAutomationTarget) -> usize {
    let (tag, first, second) = match target {
        CompiledAutomationTarget::MasterVolume => (0_u64, 0, 0),
        CompiledAutomationTarget::MasterPan => (1, 0, 0),
        CompiledAutomationTarget::Tempo => (2, 0, 0),
        CompiledAutomationTarget::Swing => (3, 0, 0),
        CompiledAutomationTarget::MixerVolume { track } => (4, u64::from(track), 0),
        CompiledAutomationTarget::MixerPan { track } => (5, u64::from(track), 0),
        CompiledAutomationTarget::MixerMute { track } => (6, u64::from(track), 0),
        CompiledAutomationTarget::ChannelVolume { channel_id } => (7, u64::from(channel_id), 0),
        CompiledAutomationTarget::ChannelPan { channel_id } => (8, u64::from(channel_id), 0),
        CompiledAutomationTarget::ChannelMute { channel_id } => (9, u64::from(channel_id), 0),
        CompiledAutomationTarget::PluginParameter {
            instance_id,
            parameter_id,
        } => (10, instance_id, u64::from(parameter_id)),
    };
    let mut value = tag.wrapping_mul(0x9E37_79B1_85EB_CA87) ^ first.rotate_left(21);
    value ^= second.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    value ^= value >> 33;
    value = value.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    value ^= value >> 33;
    value as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(target: CompiledAutomationTarget, value: f32) -> AutomationBaseValue {
        AutomationBaseValue { target, value }
    }

    fn chased(
        target: CompiledAutomationTarget,
        value: f32,
        shape: AutomationRampShape,
    ) -> TimelineAutomationChaseValue {
        TimelineAutomationChaseValue {
            target,
            value,
            shape,
        }
    }

    fn endpoint(target: CompiledAutomationTarget, value: f32) -> TimelineAutomationBlockEndpoint {
        TimelineAutomationBlockEndpoint { target, value }
    }

    fn transition(
        target: CompiledAutomationTarget,
        before_value: f32,
        after_value: f32,
        after_shape: AutomationRampShape,
        sample_offset: u32,
    ) -> TimelineAutomationTransition {
        TimelineAutomationTransition {
            target,
            before_value,
            after_value,
            after_shape,
            sample_offset,
        }
    }

    fn reset(
        automation: &mut RealtimeTimelineAutomation,
        bases: &[AutomationBaseValue],
        values: &[TimelineAutomationChaseValue],
    ) {
        automation.reset_from_chase(7, 0, bases, values).unwrap();
    }

    fn render_values(automation: &mut RealtimeTimelineAutomation, frames: u32) -> Vec<f32> {
        let mut values = Vec::with_capacity(frames as usize);
        for frame in 0..frames {
            automation
                .render_frame(frame, |_, value| values.push(value))
                .unwrap();
        }
        values
    }

    #[test]
    fn linear_block_is_exact_and_endpoint_carries_to_next_block() {
        let target = CompiledAutomationTarget::MasterVolume;
        let mut automation = RealtimeTimelineAutomation::new();
        reset(
            &mut automation,
            &[base(target, 0.0)],
            &[chased(target, 0.0, AutomationRampShape::Linear)],
        );
        let mut plan = TimelineAutomationBlockPlan::new();
        plan.reset(4).unwrap();
        plan.push_block_endpoint(endpoint(target, 1.0)).unwrap();
        automation.begin_block(7, 0, &plan).unwrap();
        let mut values = Vec::new();
        for frame in 0..4 {
            automation
                .render_frame(frame, |received, value| {
                    assert_eq!(received, target);
                    values.push(value);
                })
                .unwrap();
        }
        assert_eq!(values, vec![0.0, 0.25, 0.5, 0.75]);
        automation.finish_block().unwrap();
        automation.commit_block().unwrap();
        assert_eq!(automation.value_for(target), Some(1.0));
        assert_eq!(automation.next_frame(), 4);

        plan.reset(2).unwrap();
        plan.push_block_endpoint(endpoint(target, 1.0)).unwrap();
        automation.begin_block(7, 4, &plan).unwrap();
        let mut next = Vec::new();
        for frame in 0..2 {
            automation
                .render_frame(frame, |_, value| next.push(value))
                .unwrap();
        }
        assert_eq!(next, vec![1.0, 1.0]);
    }

    #[test]
    fn same_offset_points_preserve_last_write_order() {
        let target = CompiledAutomationTarget::MixerPan { track: 3 };
        let mut automation = RealtimeTimelineAutomation::new();
        reset(
            &mut automation,
            &[base(target, 0.0)],
            &[chased(target, 0.0, AutomationRampShape::Linear)],
        );
        let mut plan = TimelineAutomationBlockPlan::new();
        plan.reset(4).unwrap();
        plan.push_transition(transition(
            target,
            -1.0,
            -0.25,
            AutomationRampShape::Hold,
            3,
        ))
        .unwrap();
        plan.push_transition(transition(
            target,
            -0.25,
            0.5,
            AutomationRampShape::Linear,
            3,
        ))
        .unwrap();
        plan.push_block_endpoint(endpoint(target, 1.0)).unwrap();
        automation.begin_block(7, 0, &plan).unwrap();
        let mut values = Vec::new();
        for frame in 0..4 {
            automation
                .render_frame(frame, |_, value| values.push(value))
                .unwrap();
        }
        assert_eq!(values, vec![0.0, -1.0 / 3.0, -2.0 / 3.0, 0.5]);
        automation.finish_block().unwrap();
        automation.commit_block().unwrap();
        assert_eq!(automation.value_for(target), Some(1.0));
        assert_eq!(
            automation.shape_for(target),
            Some(AutomationRampShape::Linear)
        );
    }

    #[test]
    fn hold_transition_at_offset_three_steps_on_the_transition_sample() {
        let target = CompiledAutomationTarget::MasterVolume;
        let mut automation = RealtimeTimelineAutomation::new();
        reset(
            &mut automation,
            &[base(target, 0.0)],
            &[chased(target, 0.0, AutomationRampShape::Hold)],
        );
        let mut plan = TimelineAutomationBlockPlan::new();
        plan.reset(4).unwrap();
        plan.push_transition(transition(target, 0.0, 1.0, AutomationRampShape::Hold, 3))
            .unwrap();
        plan.push_block_endpoint(endpoint(target, 1.0)).unwrap();

        automation.begin_block(7, 0, &plan).unwrap();
        assert_eq!(render_values(&mut automation, 4), vec![0.0, 0.0, 0.0, 1.0]);
        automation.finish_block().unwrap();
        automation.commit_block().unwrap();
        assert_eq!(automation.value_for(target), Some(1.0));
        assert_eq!(
            automation.shape_for(target),
            Some(AutomationRampShape::Hold)
        );
    }

    #[test]
    fn automation_end_at_offset_two_restores_base_on_that_sample() {
        let target = CompiledAutomationTarget::MasterPan;
        let mut automation = RealtimeTimelineAutomation::new();
        reset(
            &mut automation,
            &[base(target, 0.0)],
            &[chased(target, 1.0, AutomationRampShape::Hold)],
        );
        let mut plan = TimelineAutomationBlockPlan::new();
        plan.reset(4).unwrap();
        plan.push_transition(transition(target, 1.0, 0.0, AutomationRampShape::Hold, 2))
            .unwrap();
        plan.push_block_endpoint(endpoint(target, 0.0)).unwrap();

        automation.begin_block(7, 0, &plan).unwrap();
        assert_eq!(render_values(&mut automation, 4), vec![1.0, 1.0, 0.0, 0.0]);
    }

    #[test]
    fn abort_preserves_committed_epoch_frame_and_value() {
        let target = CompiledAutomationTarget::MasterPan;
        let mut automation = RealtimeTimelineAutomation::new();
        reset(
            &mut automation,
            &[base(target, 0.0)],
            &[chased(target, 0.25, AutomationRampShape::Linear)],
        );
        let mut plan = TimelineAutomationBlockPlan::new();
        plan.reset(2).unwrap();
        plan.push_block_endpoint(endpoint(target, 1.0)).unwrap();
        automation.begin_block(7, 0, &plan).unwrap();
        automation.render_frame(0, |_, _| {}).unwrap();
        automation.abort_block();
        assert_eq!(automation.epoch(), 7);
        assert_eq!(automation.next_frame(), 0);
        assert_eq!(automation.value_for(target), Some(0.25));
        assert_eq!(automation.stats().aborted_blocks, 1);
    }

    #[test]
    fn malformed_plan_is_rejected_before_committed_state_changes() {
        let target = CompiledAutomationTarget::ChannelVolume { channel_id: 9 };
        let unknown = CompiledAutomationTarget::ChannelMute { channel_id: 10 };
        let mut automation = RealtimeTimelineAutomation::new();
        reset(
            &mut automation,
            &[base(target, 0.5)],
            &[chased(target, 0.5, AutomationRampShape::Hold)],
        );
        let mut plan = TimelineAutomationBlockPlan::new();
        plan.reset(8).unwrap();
        plan.push_block_endpoint(endpoint(unknown, 1.0)).unwrap();
        assert_eq!(
            automation.begin_block(7, 0, &plan),
            Err(TimelineAutomationError::UnknownTarget { target: unknown })
        );
        assert_eq!(automation.next_frame(), 0);
        assert_eq!(automation.value_for(target), Some(0.5));
    }

    #[test]
    fn reset_requires_unique_complete_resolved_values() {
        let target = CompiledAutomationTarget::Swing;
        let mut automation = RealtimeTimelineAutomation::new();
        assert_eq!(
            automation.reset_from_chase(1, 0, &[base(target, 0.0)], &[]),
            Err(TimelineAutomationError::MissingEndpoint { target })
        );
        assert_eq!(
            automation.reset_from_chase(
                1,
                0,
                &[base(target, 0.0)],
                &[
                    chased(target, 0.0, AutomationRampShape::Hold),
                    chased(target, 1.0, AutomationRampShape::Linear),
                ]
            ),
            Err(TimelineAutomationError::DuplicateEndpoint { target })
        );
    }

    #[test]
    fn failed_reset_keeps_the_previous_target_map_and_value() {
        let original = CompiledAutomationTarget::MasterVolume;
        let duplicate = CompiledAutomationTarget::MixerMute { track: 2 };
        let mut automation = RealtimeTimelineAutomation::new();
        reset(
            &mut automation,
            &[base(original, 0.5)],
            &[chased(original, 0.75, AutomationRampShape::Linear)],
        );
        assert_eq!(
            automation.reset_from_chase(
                8,
                10,
                &[base(duplicate, 0.0), base(duplicate, 1.0)],
                &[
                    chased(duplicate, 0.0, AutomationRampShape::Hold),
                    chased(duplicate, 1.0, AutomationRampShape::Hold),
                ],
            ),
            Err(TimelineAutomationError::DuplicateTarget { target: duplicate })
        );
        assert_eq!(automation.epoch(), 7);
        assert_eq!(automation.value_for(original), Some(0.75));
        assert_eq!(automation.value_for(duplicate), None);
        for received in [7, 6] {
            assert_eq!(
                automation.reset_from_chase(
                    received,
                    10,
                    &[base(original, 0.0)],
                    &[chased(original, 0.0, AutomationRampShape::Hold)],
                ),
                Err(TimelineAutomationError::StaleEpoch {
                    latest: 7,
                    received,
                })
            );
        }
        assert_eq!(automation.value_for(original), Some(0.75));
    }

    #[test]
    fn staged_reset_abort_preserves_active_and_allows_same_epoch_retry() {
        let target = CompiledAutomationTarget::MasterVolume;
        let mut automation = RealtimeTimelineAutomation::new();
        reset(
            &mut automation,
            &[base(target, 0.0)],
            &[chased(target, 0.25, AutomationRampShape::Linear)],
        );
        let replacement = [chased(target, 0.75, AutomationRampShape::Hold)];

        automation
            .stage_reset_from_chase(8, 10, &[base(target, 0.0)], &replacement)
            .unwrap();
        assert_eq!(automation.epoch(), 7);
        assert_eq!(automation.next_frame(), 0);
        assert_eq!(automation.value_for(target), Some(0.25));
        assert_eq!(
            automation.shape_for(target),
            Some(AutomationRampShape::Linear)
        );
        automation.abort_staged_reset();

        assert_eq!(automation.epoch(), 7);
        assert_eq!(automation.value_for(target), Some(0.25));
        automation
            .stage_reset_from_chase(8, 10, &[base(target, 0.0)], &replacement)
            .unwrap();
        automation.commit_staged_reset();
        assert_eq!(automation.epoch(), 8);
        assert_eq!(automation.next_frame(), 10);
        assert_eq!(automation.value_for(target), Some(0.75));
        assert_eq!(
            automation.shape_for(target),
            Some(AutomationRampShape::Hold)
        );
    }

    #[test]
    fn commit_requires_a_finished_transaction() {
        let target = CompiledAutomationTarget::MasterPan;
        let mut automation = RealtimeTimelineAutomation::new();
        reset(
            &mut automation,
            &[base(target, 0.0)],
            &[chased(target, 0.0, AutomationRampShape::Hold)],
        );
        assert_eq!(
            automation.commit_block(),
            Err(TimelineAutomationError::NoFinishedTransaction)
        );

        let mut plan = TimelineAutomationBlockPlan::new();
        plan.reset(1).unwrap();
        plan.push_block_endpoint(endpoint(target, 1.0)).unwrap();
        automation.begin_block(7, 0, &plan).unwrap();
        assert_eq!(
            automation.reset_from_chase(
                8,
                1,
                &[base(target, 0.5)],
                &[chased(target, 0.5, AutomationRampShape::Hold)],
            ),
            Err(TimelineAutomationError::TransactionAlreadyOpen)
        );
        automation.render_frame(0, |_, _| {}).unwrap();
        automation.finish_block().unwrap();
        assert_eq!(
            automation.reset_from_chase(
                8,
                1,
                &[base(target, 0.5)],
                &[chased(target, 0.5, AutomationRampShape::Hold)],
            ),
            Err(TimelineAutomationError::TransactionAlreadyOpen)
        );
        automation.abort_block();
        assert_eq!(
            automation.commit_block(),
            Err(TimelineAutomationError::NoFinishedTransaction)
        );
        assert_eq!(automation.value_for(target), Some(0.0));
    }

    #[test]
    fn frame_and_metadata_failures_are_explicit() {
        let target = CompiledAutomationTarget::Tempo;
        let mut automation = RealtimeTimelineAutomation::new();
        reset(
            &mut automation,
            &[base(target, 120.0)],
            &[chased(target, 120.0, AutomationRampShape::Hold)],
        );
        let mut plan = TimelineAutomationBlockPlan::new();
        assert_eq!(plan.reset(0), Err(TimelineAutomationError::ZeroFrames));
        plan.reset(2).unwrap();
        plan.push_block_endpoint(endpoint(target, 121.0)).unwrap();
        assert_eq!(
            automation.begin_block(8, 0, &plan),
            Err(TimelineAutomationError::EpochMismatch {
                expected: 7,
                received: 8
            })
        );
        automation.begin_block(7, 0, &plan).unwrap();
        assert_eq!(
            automation.render_frame(1, |_, _| {}),
            Err(TimelineAutomationError::FrameOrderViolation {
                expected: 0,
                received: 1
            })
        );
    }

    #[test]
    fn plan_capacity_and_order_are_bounded() {
        let target = CompiledAutomationTarget::MasterVolume;
        let mut plan = TimelineAutomationBlockPlan::new();
        plan.reset(MAX_AUTOMATION_BLOCK_FRAMES).unwrap();
        for offset in 0..MAX_AUTOMATION_CONTROL_POINTS_PER_BLOCK as u32 {
            plan.push_transition(transition(
                target,
                0.5,
                0.5,
                AutomationRampShape::Hold,
                offset,
            ))
            .unwrap();
        }
        assert_eq!(
            plan.push_transition(transition(
                target,
                0.5,
                0.5,
                AutomationRampShape::Hold,
                MAX_AUTOMATION_CONTROL_POINTS_PER_BLOCK as u32,
            )),
            Err(TimelineAutomationError::CapacityExceeded {
                resource: TimelineAutomationCapacity::ControlPoints,
                maximum: MAX_AUTOMATION_CONTROL_POINTS_PER_BLOCK
            })
        );
    }

    #[test]
    fn maximum_65536_frame_block_accepts_u16_max_transition_offset() {
        let target = CompiledAutomationTarget::MasterVolume;
        let mut automation = RealtimeTimelineAutomation::new();
        reset(
            &mut automation,
            &[base(target, 0.0)],
            &[chased(target, 0.0, AutomationRampShape::Linear)],
        );
        let mut plan = TimelineAutomationBlockPlan::new();
        plan.reset(65_536).unwrap();
        plan.push_transition(transition(
            target,
            1.0,
            0.5,
            AutomationRampShape::Hold,
            u32::from(u16::MAX),
        ))
        .unwrap();
        plan.push_block_endpoint(endpoint(target, 0.5)).unwrap();
        automation.begin_block(7, 0, &plan).unwrap();

        let mut first = None;
        let mut penultimate = None;
        let mut last = None;
        for offset in 0..65_536 {
            automation
                .render_frame(offset, |_, value| match offset {
                    0 => first = Some(value),
                    65_534 => penultimate = Some(value),
                    65_535 => last = Some(value),
                    _ => {}
                })
                .unwrap();
        }
        assert_eq!(first, Some(0.0));
        assert!(penultimate.is_some_and(|value| value > 0.999));
        assert_eq!(last, Some(0.5));
        automation.finish_block().unwrap();
        automation.commit_block().unwrap();
        assert_eq!(automation.next_frame(), 65_536);

        assert_eq!(
            plan.reset(65_537),
            Err(TimelineAutomationError::FrameCountTooLarge {
                frames: 65_537,
                maximum: 65_536,
            })
        );
    }

    #[test]
    fn target_hash_handles_all_target_families() {
        let targets = [
            CompiledAutomationTarget::MasterVolume,
            CompiledAutomationTarget::MasterPan,
            CompiledAutomationTarget::Tempo,
            CompiledAutomationTarget::Swing,
            CompiledAutomationTarget::MixerVolume { track: 31 },
            CompiledAutomationTarget::MixerPan { track: 31 },
            CompiledAutomationTarget::MixerMute { track: 31 },
            CompiledAutomationTarget::ChannelVolume { channel_id: 42 },
            CompiledAutomationTarget::ChannelPan { channel_id: 42 },
            CompiledAutomationTarget::ChannelMute { channel_id: 42 },
            CompiledAutomationTarget::PluginParameter {
                instance_id: 99,
                parameter_id: 7,
            },
        ];
        let bases = targets.map(|target| base(target, 0.0));
        let values = targets.map(|target| chased(target, 0.0, AutomationRampShape::Hold));
        let mut automation = RealtimeTimelineAutomation::new();
        automation.reset_from_chase(1, 0, &bases, &values).unwrap();
        for target in targets {
            assert_eq!(automation.value_for(target), Some(0.0));
        }
    }
}
