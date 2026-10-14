//! Callback-owned Master Volume/Pan/Mute controls in the master output's time domain.
//!
//! The timeline matrix describes source frame T. The master fader is after source
//! compensation and the master insert, so its control must describe T - output
//! latency. Construction allocates; staging, activation and block processing do
//! not. Each output control has an independent preallocated delay history.

use thiserror::Error;

use crate::{
    pdc::{ControlDelayError, ControlDelayLine},
    timeline::{AutomationBaseValue, CompiledAutomationTarget},
    timeline_executor::{MAX_AUTOMATION_BASES, TimelineAutomationChaseValue},
};

const MASTER_TARGET: CompiledAutomationTarget = CompiledAutomationTarget::MasterVolume;
const MAXIMUM_MASTER_GAIN: f32 = 1.2;

/// All three generations must still describe the same committed callback block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MasterAutomationIdentity {
    pub(crate) revision: u64,
    pub(crate) epoch: u64,
    pub(crate) pdc_revision: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MasterAutomationStage {
    pub(crate) identity: MasterAutomationIdentity,
    pub(crate) source_frame: u64,
    /// GraphPdcPlan::master_output_latency_samples(), or the legacy plan's
    /// output_latency_samples(). This includes the master insert's prefix.
    pub(crate) output_delay_samples: u64,
}

/// A finished matrix's existing borrowed accessors; no backing arrays escape.
///
/// Implementations must return the complete, immutable target column for this
/// block. The production matrix already refuses access until it is finished.
pub(crate) trait MasterAutomationMatrix {
    fn frames(&self) -> usize;
    fn target_at(&self, slot: u16) -> Option<CompiledAutomationTarget>;
    fn values_for_slot(
        &self,
        slot: u16,
        expected_target: CompiledAutomationTarget,
    ) -> Option<&[f32]>;
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub(crate) enum MasterAutomationError {
    #[error(transparent)]
    ControlDelay(#[from] ControlDelayError),
    #[error("master automation block capacity {0} must fit a nonzero u32 frame count")]
    InvalidBlockCapacity(usize),
    #[error("unable to allocate master automation gains for {0} frames")]
    BlockAllocationFailed(usize),
    #[error("master automation requires nonzero timeline, epoch and PDC revisions")]
    InvalidIdentity,
    #[error("master automation table exceeds the {MAX_AUTOMATION_BASES} target limit")]
    TargetLimitExceeded,
    #[error("automation base targets are not strictly sorted at slot {0}")]
    InvalidBaseOrder(usize),
    #[error("automation base value at slot {0} is not finite")]
    NonFiniteBase(usize),
    #[error("driven automation targets are not strictly sorted at slot {0}")]
    InvalidDrivenOrder(usize),
    #[error("driven master output control has no compiled base or matrix slot")]
    MissingMasterBase,
    #[error("master output control has multiple chase values")]
    DuplicateMasterChase,
    #[error("the master output control chase value is not finite")]
    NonFiniteChase,
    #[error("master output delay {requested} exceeds preallocated capacity {maximum}")]
    DelayOutOfRange { requested: u64, maximum: u32 },
    #[error("there is no staged master automation activation")]
    NoStagedActivation,
    #[error("master automation binding does not match the callback identity")]
    IdentityMismatch,
    #[error("master source frame {received} is not contiguous with expected frame {expected}")]
    NonContiguousSourceFrame { expected: u64, received: u64 },
    #[error("master automation block has {frames} frames; capacity is {maximum}")]
    BlockFramesOutOfRange { frames: usize, maximum: usize },
    #[error("master automation source frame range overflowed")]
    FrameRangeOverflow,
    #[error("master automation matrix slot {0} is absent, unfinished or has another target")]
    MatrixTargetMismatch(u16),
    #[error("Master output control column has {received} values; block requires {expected}")]
    MatrixLengthMismatch { expected: usize, received: usize },
    #[error("Master output control matrix value at frame offset {0} is not finite")]
    NonFiniteMatrixValue(usize),
}

#[derive(Clone, Copy, Debug)]
struct MasterConfiguration {
    identity: MasterAutomationIdentity,
    matrix_slot: Option<u16>,
    next_source_frame: u64,
    delay_samples: u32,
    initial_value: f32,
}

#[derive(Clone, Copy, Debug)]
struct PreparedMasterBlock {
    identity: MasterAutomationIdentity,
    source_frame: u64,
    frames: usize,
}

struct ValidatedMasterBlock<'a> {
    configuration: MasterConfiguration,
    values: &'a [f32],
    end_source_frame: u64,
}

#[derive(Debug)]
pub(crate) struct MasterOutputAutomation {
    target: CompiledAutomationTarget,
    minimum: f32,
    maximum: f32,
    delay: ControlDelayLine,
    gains: Box<[f32]>,
    active: Option<MasterConfiguration>,
    staged: Option<MasterConfiguration>,
    prepared: Option<PreparedMasterBlock>,
}

// Keep existing Volume callers and tests source-compatible. Pan and Mute use
// the same bounded transaction algorithm with independent lines and ranges.
pub(crate) type MasterVolumeAutomation = MasterOutputAutomation;

impl MasterOutputAutomation {
    /// Control-thread construction. Every callback-side buffer is allocated here.
    pub(crate) fn new(
        maximum_delay_samples: u32,
        maximum_block_frames: usize,
    ) -> Result<Self, MasterAutomationError> {
        Self::new_target(
            MASTER_TARGET,
            0.0,
            MAXIMUM_MASTER_GAIN,
            maximum_delay_samples,
            maximum_block_frames,
        )
    }

    pub(crate) fn new_pan(
        maximum_delay_samples: u32,
        maximum_block_frames: usize,
    ) -> Result<Self, MasterAutomationError> {
        Self::new_target(
            CompiledAutomationTarget::MasterPan,
            -1.0,
            1.0,
            maximum_delay_samples,
            maximum_block_frames,
        )
    }

    pub(crate) fn new_mute(
        maximum_delay_samples: u32,
        maximum_block_frames: usize,
    ) -> Result<Self, MasterAutomationError> {
        Self::new_target(
            CompiledAutomationTarget::MixerMute { track: 0 },
            0.0,
            1.0,
            maximum_delay_samples,
            maximum_block_frames,
        )
    }

    fn new_target(
        target: CompiledAutomationTarget,
        minimum: f32,
        maximum: f32,
        maximum_delay_samples: u32,
        maximum_block_frames: usize,
    ) -> Result<Self, MasterAutomationError> {
        if maximum_block_frames == 0 || u32::try_from(maximum_block_frames).is_err() {
            return Err(MasterAutomationError::InvalidBlockCapacity(
                maximum_block_frames,
            ));
        }
        let delay = ControlDelayLine::new(maximum_delay_samples, 0.0)?;
        let mut gains = Vec::new();
        gains
            .try_reserve_exact(maximum_block_frames)
            .map_err(|_| MasterAutomationError::BlockAllocationFailed(maximum_block_frames))?;
        gains.resize(maximum_block_frames, 0.0);
        Ok(Self {
            target,
            minimum,
            maximum,
            delay,
            gains: gains.into_boxed_slice(),
            active: None,
            staged: None,
            prepared: None,
        })
    }

    /// Preflight a discontinuity/PDC candidate without touching active history.
    ///
    /// Each attempt supersedes the previous staged candidate, including rejected
    /// attempts. A failed attempt cannot accidentally commit that older candidate.
    /// Ordinary contiguous blocks must not stage/commit again: commit resets the
    /// history, even when its generation numbers happen to be unchanged.
    pub(crate) fn stage(
        &mut self,
        request: MasterAutomationStage,
        bases: &[AutomationBaseValue],
        driven_targets: &[CompiledAutomationTarget],
        chase_values: &[TimelineAutomationChaseValue],
    ) -> Result<(), MasterAutomationError> {
        self.staged = None;
        if request.identity.revision == 0
            || request.identity.epoch == 0
            || request.identity.pdc_revision == 0
        {
            return Err(MasterAutomationError::InvalidIdentity);
        }
        if bases.len() > MAX_AUTOMATION_BASES
            || driven_targets.len() > MAX_AUTOMATION_BASES
            || chase_values.len() > MAX_AUTOMATION_BASES
        {
            return Err(MasterAutomationError::TargetLimitExceeded);
        }
        for (slot, base) in bases.iter().enumerate() {
            if slot != 0 && bases[slot - 1].target >= base.target {
                return Err(MasterAutomationError::InvalidBaseOrder(slot));
            }
            if !base.value.is_finite() {
                return Err(MasterAutomationError::NonFiniteBase(slot));
            }
        }
        for (slot, target) in driven_targets.iter().enumerate() {
            if slot != 0 && driven_targets[slot - 1] >= *target {
                return Err(MasterAutomationError::InvalidDrivenOrder(slot));
            }
        }
        let driven = driven_targets.binary_search(&self.target).is_ok();
        // An unbound manual fader needs no delayed control history. Its audio
        // PDC capacity remains governed by the ordinary mixer preflight.
        let requested_delay = if driven {
            request.output_delay_samples
        } else {
            0
        };
        let maximum = self.delay.maximum_delay_samples();
        if requested_delay > u64::from(maximum) {
            return Err(MasterAutomationError::DelayOutOfRange {
                requested: request.output_delay_samples,
                maximum,
            });
        }
        let base_slot = bases
            .binary_search_by_key(&self.target, |base| base.target)
            .ok();
        let matrix_slot = if driven {
            Some(
                u16::try_from(base_slot.ok_or(MasterAutomationError::MissingMasterBase)?)
                    .map_err(|_| MasterAutomationError::TargetLimitExceeded)?,
            )
        } else {
            None
        };
        let mut initial_value = base_slot.map_or(0.0, |slot| bases[slot].value);
        let mut chased = false;
        for chase in chase_values {
            if chase.target != self.target {
                continue;
            }
            if chased {
                return Err(MasterAutomationError::DuplicateMasterChase);
            }
            if !chase.value.is_finite() {
                return Err(MasterAutomationError::NonFiniteChase);
            }
            initial_value = chase.value;
            chased = true;
        }
        self.staged = Some(MasterConfiguration {
            identity: request.identity,
            matrix_slot,
            next_source_frame: request.source_frame,
            // Capacity preflight above proves this u64 fits the line's u32.
            delay_samples: requested_delay as u32,
            initial_value: initial_value.clamp(self.minimum, self.maximum),
        });
        Ok(())
    }

    /// Publish only after the corresponding timeline/PDC activation commits.
    /// The preflighted delay and finite seed make these line operations infallible.
    pub(crate) fn commit(&mut self) -> Result<(), MasterAutomationError> {
        let configuration = self
            .staged
            .ok_or(MasterAutomationError::NoStagedActivation)?;
        self.delay.request_delay(configuration.delay_samples, 0)?;
        self.delay.reset(configuration.initial_value)?;
        self.active = Some(configuration);
        self.staged = None;
        self.prepared = None;
        Ok(())
    }

    pub(crate) fn abort(&mut self) {
        self.staged = None;
    }

    /// Ownership release: old prepared gains and delay history become inaccessible.
    pub(crate) fn clear(&mut self) {
        self.clear_active();
        self.staged = None;
    }

    /// An admitted epoch reset discards audible history but retains the already
    /// preflighted candidate until the matching activation publishes it.
    pub(crate) fn clear_active(&mut self) {
        self.active = None;
        self.prepared = None;
        self.delay
            .reset(0.0)
            .expect("zero is a finite control delay seed");
    }

    pub(crate) fn invalidate_prepared_block(&mut self) {
        self.prepared = None;
    }

    pub(crate) fn identity(&self) -> Option<MasterAutomationIdentity> {
        self.active.map(|configuration| configuration.identity)
    }

    pub(crate) fn matrix_slot(&self) -> Option<u16> {
        self.active
            .and_then(|configuration| configuration.matrix_slot)
    }

    #[cfg(test)]
    pub(crate) fn next_source_frame(&self) -> Option<u64> {
        self.active
            .map(|configuration| configuration.next_source_frame)
    }

    /// Only this instance's exact driven output target is counted.
    pub(crate) fn applied_count(&self, identity: MasterAutomationIdentity) -> u64 {
        u64::from(self.active.is_some_and(|configuration| {
            configuration.identity == identity && configuration.matrix_slot.is_some()
        }))
    }

    /// Read-only transaction preflight; never advances the control history.
    pub(crate) fn validate_matrix<M: MasterAutomationMatrix + ?Sized>(
        &self,
        identity: MasterAutomationIdentity,
        source_frame: u64,
        matrix: &M,
    ) -> Result<bool, MasterAutomationError> {
        self.validated_block(identity, source_frame, matrix)
            .map(|block| block.is_some())
    }

    /// Capture this block's authority after BOTH timeline and matrix transactions
    /// commit, before source rendering clears timeline_plan_render_active. The
    /// later fader reads `value` using this independent prepared-block identity.
    ///
    /// Pause/unbound calls invalidate old prepared gains but do not advance delay
    /// history. Any rejected block similarly leaves active binding/cursor/history
    /// untouched, so replaying or skipping source frames cannot consume controls.
    pub(crate) fn prepare_block<M: MasterAutomationMatrix + ?Sized>(
        &mut self,
        identity: MasterAutomationIdentity,
        source_frame: u64,
        playing: bool,
        matrix: &M,
    ) -> Result<bool, MasterAutomationError> {
        self.prepared = None;
        if !playing {
            return Ok(false);
        }
        let Some(block) = self.validated_block(identity, source_frame, matrix)? else {
            return Ok(false);
        };
        // All fallible checks, including every sample, precede the first write.
        for (offset, input) in block.values.iter().copied().enumerate() {
            self.gains[offset] = self
                .delay
                .process_sample(input.clamp(self.minimum, self.maximum));
        }
        let mut configuration = block.configuration;
        configuration.next_source_frame = block.end_source_frame;
        self.active = Some(configuration);
        self.prepared = Some(PreparedMasterBlock {
            identity,
            source_frame,
            frames: block.values.len(),
        });
        Ok(true)
    }

    /// A stale block, identity or offset uses the current manual control. This
    /// lookup never advances history and does not query the cleared timeline flag.
    pub(crate) fn value(
        &self,
        identity: MasterAutomationIdentity,
        source_frame: u64,
        offset: usize,
        manual: f32,
    ) -> f32 {
        if let Some(block) = self.prepared
            && block.identity == identity
            && block.source_frame == source_frame
            && offset < block.frames
            && self.identity() == Some(identity)
        {
            return self.gains[offset];
        }
        if manual.is_finite() {
            manual.clamp(self.minimum, self.maximum)
        } else {
            0.0
        }
    }

    fn validated_block<'a, M: MasterAutomationMatrix + ?Sized>(
        &self,
        identity: MasterAutomationIdentity,
        source_frame: u64,
        matrix: &'a M,
    ) -> Result<Option<ValidatedMasterBlock<'a>>, MasterAutomationError> {
        let Some(configuration) = self.active else {
            return Ok(None);
        };
        let Some(slot) = configuration.matrix_slot else {
            return Ok(None);
        };
        if configuration.identity != identity {
            return Err(MasterAutomationError::IdentityMismatch);
        }
        if source_frame != configuration.next_source_frame {
            return Err(MasterAutomationError::NonContiguousSourceFrame {
                expected: configuration.next_source_frame,
                received: source_frame,
            });
        }
        let frames = matrix.frames();
        if frames == 0 || frames > self.gains.len() {
            return Err(MasterAutomationError::BlockFramesOutOfRange {
                frames,
                maximum: self.gains.len(),
            });
        }
        let end_source_frame = source_frame
            .checked_add(frames as u64)
            .ok_or(MasterAutomationError::FrameRangeOverflow)?;
        if matrix.target_at(slot) != Some(self.target) {
            return Err(MasterAutomationError::MatrixTargetMismatch(slot));
        }
        let values = matrix
            .values_for_slot(slot, self.target)
            .ok_or(MasterAutomationError::MatrixTargetMismatch(slot))?;
        if values.len() != frames {
            return Err(MasterAutomationError::MatrixLengthMismatch {
                expected: frames,
                received: values.len(),
            });
        }
        if let Some(offset) = values.iter().position(|value| !value.is_finite()) {
            return Err(MasterAutomationError::NonFiniteMatrixValue(offset));
        }
        Ok(Some(ValidatedMasterBlock {
            configuration,
            values,
            end_source_frame,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::AutomationRampShape;

    struct Matrix<'a>(&'a [f32]);

    impl MasterAutomationMatrix for Matrix<'_> {
        fn frames(&self) -> usize {
            self.0.len()
        }
        fn target_at(&self, slot: u16) -> Option<CompiledAutomationTarget> {
            (slot == 0).then_some(MASTER_TARGET)
        }
        fn values_for_slot(&self, slot: u16, target: CompiledAutomationTarget) -> Option<&[f32]> {
            (slot == 0 && target == MASTER_TARGET).then_some(self.0)
        }
    }

    fn identity(epoch: u64) -> MasterAutomationIdentity {
        MasterAutomationIdentity {
            revision: 1,
            epoch,
            pdc_revision: 1,
        }
    }

    fn stage(controls: &mut MasterVolumeAutomation, epoch: u64, frame: u64, delay: u64, seed: f32) {
        controls
            .stage(
                MasterAutomationStage {
                    identity: identity(epoch),
                    source_frame: frame,
                    output_delay_samples: delay,
                },
                &[AutomationBaseValue {
                    target: MASTER_TARGET,
                    value: 0.7,
                }],
                &[MASTER_TARGET],
                &[TimelineAutomationChaseValue {
                    target: MASTER_TARGET,
                    value: seed,
                    shape: AutomationRampShape::Linear,
                }],
            )
            .unwrap();
        controls.commit().unwrap();
    }

    #[test]
    fn rejected_activation_preserves_the_prior_delayed_controls() {
        let mut controls = MasterVolumeAutomation::new(4, 8).unwrap();
        stage(&mut controls, 1, 0, 2, 0.6);
        controls
            .prepare_block(identity(1), 0, true, &Matrix(&[0.6, 0.5]))
            .unwrap();
        assert_eq!(controls.value(identity(1), 0, 1, 0.0), 0.6);
        assert!(matches!(
            controls.stage(
                MasterAutomationStage {
                    identity: identity(2),
                    source_frame: 99,
                    output_delay_samples: 5,
                },
                &[AutomationBaseValue {
                    target: MASTER_TARGET,
                    value: 0.9
                }],
                &[MASTER_TARGET],
                &[],
            ),
            Err(MasterAutomationError::DelayOutOfRange { .. })
        ));
        assert_eq!(
            controls.commit(),
            Err(MasterAutomationError::NoStagedActivation)
        );
        assert_eq!(controls.identity(), Some(identity(1)));
        assert_eq!(controls.next_source_frame(), Some(2));
        controls
            .stage(
                MasterAutomationStage {
                    identity: identity(2),
                    source_frame: 99,
                    output_delay_samples: 3,
                },
                &[AutomationBaseValue {
                    target: MASTER_TARGET,
                    value: 0.9,
                }],
                &[MASTER_TARGET],
                &[],
            )
            .unwrap();
        controls.abort();
        assert_eq!(controls.value(identity(1), 0, 1, 0.0), 0.6);
        controls
            .prepare_block(identity(1), 2, true, &Matrix(&[0.4, 0.3]))
            .unwrap();
        assert_eq!(controls.value(identity(1), 2, 0, 0.0), 0.6);
        assert_eq!(controls.value(identity(1), 2, 1, 0.0), 0.5);
    }

    #[test]
    fn invalid_and_noncontiguous_blocks_do_not_consume_history() {
        let mut controls = MasterVolumeAutomation::new(4, 8).unwrap();
        stage(&mut controls, 1, 0, 1, 0.7);
        controls
            .prepare_block(identity(1), 0, true, &Matrix(&[0.8, 0.4]))
            .unwrap();
        assert!(matches!(
            controls.prepare_block(identity(1), 0, true, &Matrix(&[0.2])),
            Err(MasterAutomationError::NonContiguousSourceFrame {
                expected: 2,
                received: 0
            })
        ));
        assert!(matches!(
            controls.prepare_block(identity(1), 3, true, &Matrix(&[0.2])),
            Err(MasterAutomationError::NonContiguousSourceFrame {
                expected: 2,
                received: 3
            })
        ));
        assert_eq!(
            controls.prepare_block(identity(2), 2, true, &Matrix(&[0.2])),
            Err(MasterAutomationError::IdentityMismatch)
        );
        assert_eq!(
            controls.prepare_block(identity(1), 2, true, &Matrix(&[0.2, f32::NAN])),
            Err(MasterAutomationError::NonFiniteMatrixValue(1))
        );
        assert_eq!(controls.next_source_frame(), Some(2));
        assert_eq!(controls.value(identity(1), 0, 0, 0.25), 0.25);
        assert!(
            controls
                .validate_matrix(identity(1), 2, &Matrix(&[0.2, 0.1]))
                .unwrap()
        );
        assert_eq!(controls.next_source_frame(), Some(2));
        controls
            .prepare_block(identity(1), 2, true, &Matrix(&[0.2, 0.1]))
            .unwrap();
        assert_eq!(controls.value(identity(1), 2, 0, 0.0), 0.4);
        assert_eq!(controls.value(identity(1), 2, 1, 0.0), 0.2);
    }

    #[test]
    fn pause_does_not_advance_and_new_epoch_seeds_without_old_gains() {
        let mut controls = MasterVolumeAutomation::new(4, 8).unwrap();
        stage(&mut controls, 1, 10, 1, 0.7);
        assert!(
            !controls
                .prepare_block(identity(1), 10, false, &Matrix(&[f32::NAN]))
                .unwrap()
        );
        assert_eq!(controls.next_source_frame(), Some(10));
        assert_eq!(controls.value(identity(1), 10, 0, 0.25), 0.25);
        controls
            .prepare_block(identity(1), 10, true, &Matrix(&[0.5, 0.25]))
            .unwrap();
        assert_eq!(controls.value(identity(1), 10, 1, 0.0), 0.5);
        controls.invalidate_prepared_block();
        assert_eq!(controls.value(identity(1), 10, 1, 0.4), 0.4);
        stage(&mut controls, 2, 100, 1, 0.3);
        assert_eq!(controls.value(identity(1), 10, 1, 0.4), 0.4);
        controls
            .prepare_block(identity(2), 100, true, &Matrix(&[0.2]))
            .unwrap();
        assert_eq!(controls.value(identity(2), 100, 0, 0.0), 0.3);
        assert_eq!(controls.applied_count(identity(1)), 0);
        assert_eq!(controls.applied_count(identity(2)), 1);
    }

    #[test]
    fn admitted_epoch_clear_retains_only_the_staged_candidate() {
        let mut controls = MasterVolumeAutomation::new(4, 8).unwrap();
        stage(&mut controls, 1, 0, 1, 0.9);
        controls
            .prepare_block(identity(1), 0, true, &Matrix(&[0.8, 0.7]))
            .unwrap();
        controls
            .stage(
                MasterAutomationStage {
                    identity: identity(2),
                    source_frame: 100,
                    output_delay_samples: 2,
                },
                &[AutomationBaseValue {
                    target: MASTER_TARGET,
                    value: 0.4,
                }],
                &[MASTER_TARGET],
                &[],
            )
            .unwrap();
        controls.clear_active();
        assert_eq!(controls.identity(), None);
        assert_eq!(controls.value(identity(1), 0, 0, 0.25), 0.25);
        controls.commit().unwrap();
        controls
            .prepare_block(identity(2), 100, true, &Matrix(&[0.2, 0.1]))
            .unwrap();
        assert_eq!(controls.value(identity(2), 100, 0, 0.0), 0.4);
        assert_eq!(controls.value(identity(2), 100, 1, 0.0), 0.4);
        controls.clear();
        assert_eq!(
            controls.commit(),
            Err(MasterAutomationError::NoStagedActivation)
        );
    }

    #[test]
    fn unbound_manual_fader_needs_no_control_delay_capacity_or_matrix() {
        let mut controls = MasterVolumeAutomation::new(0, 8).unwrap();
        controls
            .stage(
                MasterAutomationStage {
                    identity: identity(1),
                    source_frame: 0,
                    output_delay_samples: u64::MAX,
                },
                &[AutomationBaseValue {
                    target: MASTER_TARGET,
                    value: 0.4,
                }],
                &[CompiledAutomationTarget::MasterPan],
                &[],
            )
            .unwrap();
        controls.commit().unwrap();
        assert_eq!(controls.applied_count(identity(1)), 0);
        assert!(
            !controls
                .prepare_block(identity(1), 0, true, &Matrix(&[f32::NAN]))
                .unwrap()
        );
        assert_eq!(controls.value(identity(1), 0, 0, 0.8), 0.8);
        assert_eq!(controls.next_source_frame(), Some(0));
    }
    struct PanMatrix<'a>(&'a [f32]);

    impl MasterAutomationMatrix for PanMatrix<'_> {
        fn frames(&self) -> usize {
            self.0.len()
        }
        fn target_at(&self, slot: u16) -> Option<CompiledAutomationTarget> {
            (slot == 0).then_some(CompiledAutomationTarget::MasterPan)
        }
        fn values_for_slot(&self, slot: u16, target: CompiledAutomationTarget) -> Option<&[f32]> {
            (slot == 0 && target == CompiledAutomationTarget::MasterPan).then_some(self.0)
        }
    }

    fn stage_pan(
        controls: &mut MasterOutputAutomation,
        epoch: u64,
        frame: u64,
        delay: u64,
        seed: f32,
    ) {
        controls
            .stage(
                MasterAutomationStage {
                    identity: identity(epoch),
                    source_frame: frame,
                    output_delay_samples: delay,
                },
                &[AutomationBaseValue {
                    target: CompiledAutomationTarget::MasterPan,
                    value: seed,
                }],
                &[CompiledAutomationTarget::MasterPan],
                &[],
            )
            .unwrap();
        controls.commit().unwrap();
    }

    #[test]
    fn pan_and_volume_have_independent_delay_histories_and_signed_ranges() {
        let mut volume = MasterVolumeAutomation::new(4, 8).unwrap();
        let mut pan = MasterOutputAutomation::new_pan(4, 8).unwrap();
        stage(&mut volume, 1, 0, 2, 0.7);
        stage_pan(&mut pan, 1, 0, 2, -0.75);
        volume
            .prepare_block(identity(1), 0, true, &Matrix(&[0.2, 0.3]))
            .unwrap();
        pan.prepare_block(identity(1), 0, true, &PanMatrix(&[-2.0, 2.0]))
            .unwrap();
        assert_eq!(volume.value(identity(1), 0, 0, 0.0), 0.7);
        assert_eq!(pan.value(identity(1), 0, 0, 0.0), -0.75);
        volume
            .prepare_block(identity(1), 2, true, &Matrix(&[0.4, 0.5]))
            .unwrap();
        pan.prepare_block(identity(1), 2, true, &PanMatrix(&[0.25, -0.25]))
            .unwrap();
        assert_eq!(volume.value(identity(1), 2, 0, 0.0), 0.2);
        assert_eq!(pan.value(identity(1), 2, 0, 0.0), -1.0);
        assert_eq!(pan.value(identity(1), 2, 1, 0.0), 1.0);
        pan.invalidate_prepared_block();
        assert_eq!(pan.value(identity(1), 2, 0, -0.4), -0.4);
        assert_eq!(pan.value(identity(1), 2, 0, -2.0), -1.0);
    }

    #[test]
    fn pan_capacity_rejection_keeps_prior_signed_history_and_epoch_reset_seeds_new_pan() {
        let mut pan = MasterOutputAutomation::new_pan(2, 8).unwrap();
        stage_pan(&mut pan, 1, 0, 1, -0.5);
        pan.prepare_block(identity(1), 0, true, &PanMatrix(&[0.75]))
            .unwrap();
        assert!(matches!(
            pan.stage(
                MasterAutomationStage {
                    identity: identity(2),
                    source_frame: 50,
                    output_delay_samples: 3
                },
                &[AutomationBaseValue {
                    target: CompiledAutomationTarget::MasterPan,
                    value: 1.0
                }],
                &[CompiledAutomationTarget::MasterPan],
                &[],
            ),
            Err(MasterAutomationError::DelayOutOfRange {
                requested: 3,
                maximum: 2
            })
        ));
        assert_eq!(pan.identity(), Some(identity(1)));
        assert_eq!(pan.next_source_frame(), Some(1));
        pan.prepare_block(identity(1), 1, true, &PanMatrix(&[-0.25]))
            .unwrap();
        assert_eq!(pan.value(identity(1), 1, 0, 0.0), 0.75);
        stage_pan(&mut pan, 2, 50, 1, -0.8);
        pan.prepare_block(identity(2), 50, true, &PanMatrix(&[0.2]))
            .unwrap();
        assert_eq!(pan.value(identity(2), 50, 0, 0.0), -0.8);
    }
}
