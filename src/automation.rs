//! Automation data and curve evaluation for the DAW timeline.
//!
//! Positions are expressed in beats and values are expressed in the target's
//! native range (for example BPM for [`AutomationTarget::Tempo`]).  Lanes keep
//! their points sorted and unique, so they are cheap to sample from the audio
//! or UI thread after project loading/editing has finished.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::mixer_graph::{MASTER_MIXER_TRACK_ID, MixerTrackId};

/// Points closer than this many beats are treated as the same timeline point.
pub const AUTOMATION_POSITION_EPSILON: f64 = 1.0e-9;

/// A parameter that can be controlled by an automation lane.
///
/// The enum is intentionally independent from the project model. Stable project
/// IDs can be stored in the `track`, `channel`, and `instance` fields when the
/// lane is embedded into a project.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AutomationTarget {
    MasterVolume,
    MasterPan,
    Tempo,
    Swing,
    MixerVolume {
        #[serde(rename = "mixer_track_id", alias = "track")]
        track: MixerTrackId,
    },
    MixerPan {
        #[serde(rename = "mixer_track_id", alias = "track")]
        track: MixerTrackId,
    },
    MixerMute {
        #[serde(rename = "mixer_track_id", alias = "track")]
        track: MixerTrackId,
    },
    ChannelVolume {
        channel: u32,
    },
    ChannelPan {
        channel: u32,
    },
    ChannelMute {
        channel: u32,
    },
    PluginParameter {
        instance: u64,
        parameter: u32,
    },
}

/// Resolves persisted aliases onto the single runtime control they address.
///
/// The fixed MASTER mixer identity addresses the master bus, so its volume and
/// pan controls must not form a second automation namespace. Mute intentionally
/// remains a mixer-track target because master mute is represented by that
/// persisted control. Callers should canonicalize transient/compiler values
/// only; project data can retain the spelling it was saved with for lossless
/// round trips.
#[must_use]
pub fn canonicalize_automation_target(target: AutomationTarget) -> AutomationTarget {
    match target {
        AutomationTarget::MixerVolume {
            track: MASTER_MIXER_TRACK_ID,
        } => AutomationTarget::MasterVolume,
        AutomationTarget::MixerPan {
            track: MASTER_MIXER_TRACK_ID,
        } => AutomationTarget::MasterPan,
        target => target,
    }
}

impl AutomationTarget {
    /// The normal value range used when creating a lane for this target.
    #[must_use]
    pub const fn default_value_range(&self) -> AutomationValueRange {
        match self {
            Self::MasterPan | Self::MixerPan { .. } | Self::ChannelPan { .. } => {
                AutomationValueRange::new_unchecked(-1.0, 1.0)
            }
            Self::Tempo => AutomationValueRange::new_unchecked(10.0, 522.0),
            Self::MasterVolume
            | Self::Swing
            | Self::MixerVolume { .. }
            | Self::MixerMute { .. }
            | Self::ChannelVolume { .. }
            | Self::ChannelMute { .. }
            | Self::PluginParameter { .. } => AutomationValueRange::new_unchecked(0.0, 1.0),
        }
    }

    /// Whether interpolation is normally inappropriate for this target.
    #[must_use]
    pub const fn is_discrete(&self) -> bool {
        matches!(self, Self::MixerMute { .. } | Self::ChannelMute { .. })
    }
}

/// Inclusive native-value limits for an automation lane.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct AutomationValueRange {
    pub min: f64,
    pub max: f64,
}

impl AutomationValueRange {
    /// Creates a finite, ordered value range.
    pub fn new(min: f64, max: f64) -> Result<Self, AutomationError> {
        if !min.is_finite() || !max.is_finite() || min > max {
            return Err(AutomationError::InvalidValueRange);
        }
        Ok(Self { min, max })
    }

    const fn new_unchecked(min: f64, max: f64) -> Self {
        Self { min, max }
    }

    #[must_use]
    pub fn clamp(self, value: f64) -> f64 {
        value.clamp(self.min, self.max)
    }

    /// Converts a native value into the normalized `0.0..=1.0` domain.
    #[must_use]
    pub fn normalize(self, value: f64) -> f64 {
        let width = self.max - self.min;
        if width <= f64::EPSILON {
            0.0
        } else {
            ((value - self.min) / width).clamp(0.0, 1.0)
        }
    }

    /// Converts a normalized value into the lane's native value domain.
    #[must_use]
    pub fn denormalize(self, value: f64) -> f64 {
        self.min + value.clamp(0.0, 1.0) * (self.max - self.min)
    }
}

impl Default for AutomationValueRange {
    fn default() -> Self {
        Self::new_unchecked(0.0, 1.0)
    }
}

/// The interpolation performed between adjacent automation points.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationCurve {
    /// Straight interpolation between values. Point tension is ignored.
    #[default]
    Linear,
    /// Monotonic curved interpolation controlled by the left point's tension.
    Tension,
    /// Holds the left value until the next point is reached.
    Hold,
}

/// A single point on an automation lane.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct AutomationPoint {
    /// Timeline position in beats.
    pub position: f64,
    /// Value in the lane target's native range.
    pub value: f64,
    /// Outgoing curve tension in `-1.0..=1.0`.
    #[serde(default)]
    pub tension: f64,
}

impl AutomationPoint {
    #[must_use]
    pub const fn new(position: f64, value: f64) -> Self {
        Self {
            position,
            value,
            tension: 0.0,
        }
    }

    #[must_use]
    pub const fn with_tension(position: f64, value: f64, tension: f64) -> Self {
        Self {
            position,
            value,
            tension,
        }
    }
}

/// A half-open beat range (`start..end`) used for repeating a lane.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct AutomationLoop {
    pub start: f64,
    pub end: f64,
}

impl AutomationLoop {
    pub fn new(start: f64, end: f64) -> Result<Self, AutomationError> {
        let region = Self { start, end };
        if !region.is_valid() {
            return Err(AutomationError::InvalidLoopRange);
        }
        Ok(region)
    }

    #[must_use]
    pub fn is_valid(self) -> bool {
        self.start.is_finite() && self.end.is_finite() && self.end > self.start
    }

    #[must_use]
    pub fn length(self) -> f64 {
        self.end - self.start
    }

    /// Wraps beats at and after the loop start. Earlier beats remain unchanged,
    /// allowing an automation intro before the repeating section.
    #[must_use]
    pub fn wrap(self, beat: f64) -> f64 {
        if !self.is_valid() || !beat.is_finite() || beat < self.start {
            beat
        } else {
            self.start + (beat - self.start).rem_euclid(self.length())
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AutomationNormalizationReport {
    pub invalid_points_removed: usize,
    pub duplicate_points_merged: usize,
    pub positions_clamped: usize,
    pub values_clamped: usize,
    pub tensions_repaired: usize,
    pub value_range_repaired: bool,
    pub invalid_loop_removed: bool,
}

impl AutomationNormalizationReport {
    #[must_use]
    pub const fn changed(self) -> bool {
        self.invalid_points_removed != 0
            || self.duplicate_points_merged != 0
            || self.positions_clamped != 0
            || self.values_clamped != 0
            || self.tensions_repaired != 0
            || self.value_range_repaired
            || self.invalid_loop_removed
    }
}

#[derive(Clone, Debug, PartialEq, Error)]
pub enum AutomationError {
    #[error("automation point position must be finite")]
    NonFinitePosition,
    #[error("automation point value must be finite")]
    NonFiniteValue,
    #[error("automation point index {index} is out of bounds for {len} points")]
    PointIndexOutOfBounds { index: usize, len: usize },
    #[error("automation value range must be finite and min must not exceed max")]
    InvalidValueRange,
    #[error("automation loop must be finite and end after start")]
    InvalidLoopRange,
}

/// A normalized, serializable automation lane.
///
/// Deserialization automatically repairs ordering, duplicate points, ranges,
/// and malformed optional loop data. Editing methods preserve those invariants.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(from = "AutomationLaneData")]
pub struct AutomationLane {
    target: AutomationTarget,
    points: Vec<AutomationPoint>,
    curve: AutomationCurve,
    enabled: bool,
    value_range: AutomationValueRange,
    loop_region: Option<AutomationLoop>,
}

#[derive(Deserialize)]
struct AutomationLaneData {
    target: AutomationTarget,
    #[serde(default)]
    points: Vec<AutomationPoint>,
    #[serde(default)]
    curve: AutomationCurve,
    #[serde(default = "default_enabled")]
    enabled: bool,
    #[serde(default)]
    value_range: Option<AutomationValueRange>,
    #[serde(default)]
    loop_region: Option<AutomationLoop>,
}

const fn default_enabled() -> bool {
    true
}

impl From<AutomationLaneData> for AutomationLane {
    fn from(data: AutomationLaneData) -> Self {
        let default_range = data.target.default_value_range();
        let mut lane = Self {
            target: data.target,
            points: data.points,
            curve: data.curve,
            enabled: data.enabled,
            value_range: data.value_range.unwrap_or(default_range),
            loop_region: data.loop_region,
        };
        lane.normalize();
        lane
    }
}

impl AutomationLane {
    #[must_use]
    pub fn new(target: AutomationTarget) -> Self {
        let value_range = target.default_value_range();
        let curve = if target.is_discrete() {
            AutomationCurve::Hold
        } else {
            AutomationCurve::Linear
        };
        Self {
            target,
            points: Vec::new(),
            curve,
            enabled: true,
            value_range,
            loop_region: None,
        }
    }

    #[must_use]
    pub const fn target(&self) -> &AutomationTarget {
        &self.target
    }

    /// Retargets a lane and adopts the destination's native range. Existing
    /// values are clamped into that range; discrete targets use Hold curves.
    pub fn set_target(&mut self, target: AutomationTarget) {
        let was_discrete = self.target.is_discrete();
        let discrete = target.is_discrete();
        let previous_range = self.value_range;
        let next_range = target.default_value_range();
        for point in &mut self.points {
            point.value = next_range.denormalize(previous_range.normalize(point.value));
        }
        self.value_range = next_range;
        self.target = target;
        if discrete {
            self.curve = AutomationCurve::Hold;
        } else if was_discrete && self.curve == AutomationCurve::Hold {
            self.curve = AutomationCurve::Linear;
        }
        self.normalize();
    }

    #[must_use]
    pub fn points(&self) -> &[AutomationPoint] {
        &self.points
    }

    #[must_use]
    pub const fn curve(&self) -> AutomationCurve {
        self.curve
    }

    pub fn set_curve(&mut self, curve: AutomationCurve) {
        self.curve = curve;
    }

    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    #[must_use]
    pub const fn value_range(&self) -> AutomationValueRange {
        self.value_range
    }

    pub fn set_value_range(
        &mut self,
        value_range: AutomationValueRange,
    ) -> Result<(), AutomationError> {
        let value_range = AutomationValueRange::new(value_range.min, value_range.max)?;
        self.value_range = value_range;
        self.normalize();
        Ok(())
    }

    #[must_use]
    pub const fn loop_region(&self) -> Option<AutomationLoop> {
        self.loop_region
    }

    pub fn set_loop_region(
        &mut self,
        loop_region: Option<AutomationLoop>,
    ) -> Result<(), AutomationError> {
        if loop_region.is_some_and(|region| !region.is_valid()) {
            return Err(AutomationError::InvalidLoopRange);
        }
        self.loop_region = loop_region;
        Ok(())
    }

    pub fn clear(&mut self) {
        self.points.clear();
    }

    /// Replaces all points and restores all lane invariants.
    pub fn replace_points(
        &mut self,
        points: impl IntoIterator<Item = AutomationPoint>,
    ) -> AutomationNormalizationReport {
        self.points = points.into_iter().collect();
        self.normalize()
    }

    /// Sorts points, removes malformed points, clamps values/tensions and merges
    /// duplicate positions. For duplicates, the last point in input order wins.
    pub fn normalize(&mut self) -> AutomationNormalizationReport {
        let mut report = AutomationNormalizationReport::default();

        if !self.value_range.min.is_finite() || !self.value_range.max.is_finite() {
            self.value_range = self.target.default_value_range();
            report.value_range_repaired = true;
        } else if self.value_range.min > self.value_range.max {
            std::mem::swap(&mut self.value_range.min, &mut self.value_range.max);
            report.value_range_repaired = true;
        }

        let mut valid = Vec::with_capacity(self.points.len());
        for (original_index, mut point) in self.points.drain(..).enumerate() {
            if !point.position.is_finite() || !point.value.is_finite() {
                report.invalid_points_removed += 1;
                continue;
            }

            if point.position < 0.0 {
                point.position = 0.0;
                report.positions_clamped += 1;
            }

            let value = self.value_range.clamp(point.value);
            if value != point.value {
                point.value = value;
                report.values_clamped += 1;
            }

            let tension = if point.tension.is_finite() {
                point.tension.clamp(-1.0, 1.0)
            } else {
                0.0
            };
            if tension != point.tension {
                point.tension = tension;
                report.tensions_repaired += 1;
            }

            valid.push((original_index, point));
        }

        valid.sort_by(|left, right| left.1.position.total_cmp(&right.1.position));

        let mut normalized: Vec<(usize, AutomationPoint)> = Vec::with_capacity(valid.len());
        for candidate in valid {
            if let Some(last) = normalized.last_mut()
                && (candidate.1.position - last.1.position).abs() <= AUTOMATION_POSITION_EPSILON
            {
                report.duplicate_points_merged += 1;
                if candidate.0 > last.0 {
                    *last = candidate;
                }
                continue;
            }
            normalized.push(candidate);
        }
        self.points = normalized.into_iter().map(|(_, point)| point).collect();

        if self.loop_region.is_some_and(|region| !region.is_valid()) {
            self.loop_region = None;
            report.invalid_loop_removed = true;
        }

        report
    }

    /// Inserts a point. A point at the same beat is replaced (last edit wins).
    pub fn insert_point(&mut self, point: AutomationPoint) -> Result<usize, AutomationError> {
        let point = self.prepare_point(point)?;
        self.points.retain(|existing| {
            (existing.position - point.position).abs() > AUTOMATION_POSITION_EPSILON
        });
        self.points.push(point);
        self.points
            .sort_by(|left, right| left.position.total_cmp(&right.position));
        Ok(self
            .points
            .partition_point(|existing| existing.position < point.position))
    }

    /// Convenience form of [`Self::insert_point`] for UI edit operations.
    pub fn insert_point_at(
        &mut self,
        position: f64,
        value: f64,
        tension: f64,
    ) -> Result<usize, AutomationError> {
        self.insert_point(AutomationPoint::with_tension(position, value, tension))
    }

    /// Moves a point while retaining its value and tension.
    ///
    /// If the destination overlaps another point, the moved point replaces it.
    pub fn move_point(&mut self, index: usize, position: f64) -> Result<usize, AutomationError> {
        let len = self.points.len();
        let Some(mut point) = self.points.get(index).copied() else {
            return Err(AutomationError::PointIndexOutOfBounds { index, len });
        };
        if !position.is_finite() {
            return Err(AutomationError::NonFinitePosition);
        }
        self.points.remove(index);
        point.position = position;
        self.insert_point(point)
    }

    /// Replaces every editable property of an existing point.
    pub fn update_point(
        &mut self,
        index: usize,
        point: AutomationPoint,
    ) -> Result<usize, AutomationError> {
        let len = self.points.len();
        if index >= len {
            return Err(AutomationError::PointIndexOutOfBounds { index, len });
        }
        let point = self.prepare_point(point)?;
        self.points.remove(index);
        self.insert_point(point)
    }

    pub fn delete_point(&mut self, index: usize) -> Result<AutomationPoint, AutomationError> {
        let len = self.points.len();
        if index >= len {
            return Err(AutomationError::PointIndexOutOfBounds { index, len });
        }
        Ok(self.points.remove(index))
    }

    fn prepare_point(
        &self,
        mut point: AutomationPoint,
    ) -> Result<AutomationPoint, AutomationError> {
        if !point.position.is_finite() {
            return Err(AutomationError::NonFinitePosition);
        }
        if !point.value.is_finite() {
            return Err(AutomationError::NonFiniteValue);
        }
        point.position = point.position.max(0.0);
        point.value = self.value_range.clamp(point.value);
        point.tension = if point.tension.is_finite() {
            point.tension.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        Ok(point)
    }

    /// Evaluates a lane at a beat, applying its optional loop region.
    ///
    /// Empty or disabled lanes return `None`. Values before the first point and
    /// after the last point hold the nearest endpoint value.
    #[must_use]
    pub fn evaluate(&self, beat: f64) -> Option<f64> {
        if !self.enabled || !beat.is_finite() {
            return None;
        }
        let beat = self.loop_region.map_or(beat, |region| region.wrap(beat));
        self.evaluate_unlooped(beat)
    }

    /// Evaluates without applying the lane's configured loop.
    #[must_use]
    pub fn evaluate_unlooped(&self, beat: f64) -> Option<f64> {
        if !self.enabled || !beat.is_finite() {
            return None;
        }
        let first = self.points.first()?;
        if beat <= first.position {
            return Some(first.value);
        }

        let last = self.points.last()?;
        if beat >= last.position {
            return Some(last.value);
        }

        let right_index = self.points.partition_point(|point| point.position <= beat);
        let left = self.points[right_index - 1];
        let right = self.points[right_index];
        let width = right.position - left.position;
        if width <= AUTOMATION_POSITION_EPSILON {
            return Some(right.value);
        }

        let progress = ((beat - left.position) / width).clamp(0.0, 1.0);
        let progress = sample_curve_progress(self.curve, progress, left.tension);
        Some(left.value + (right.value - left.value) * progress)
    }

    /// Evaluates using an explicit loop without modifying lane configuration.
    pub fn evaluate_looped(
        &self,
        beat: f64,
        loop_region: AutomationLoop,
    ) -> Result<Option<f64>, AutomationError> {
        if !loop_region.is_valid() {
            return Err(AutomationError::InvalidLoopRange);
        }
        Ok(self.evaluate_unlooped(loop_region.wrap(beat)))
    }
}

/// Maps linear segment progress through the selected curve.
///
/// Tension interpolation is monotonic and can therefore never overshoot either
/// endpoint. Positive tension bends late; negative tension bends early.
#[must_use]
pub fn sample_curve_progress(curve: AutomationCurve, progress: f64, tension: f64) -> f64 {
    let progress = progress.clamp(0.0, 1.0);
    match curve {
        AutomationCurve::Linear => progress,
        AutomationCurve::Hold => {
            if progress >= 1.0 {
                1.0
            } else {
                0.0
            }
        }
        AutomationCurve::Tension => {
            let tension = if tension.is_finite() {
                tension.clamp(-1.0, 1.0)
            } else {
                0.0
            };
            if tension.abs() <= f64::EPSILON {
                return progress;
            }

            // 1 at zero tension through 16 at maximum tension. Exponential
            // shaping gives a useful musical response while retaining exact
            // endpoints and remaining bounded.
            let exponent = 2.0_f64.powf(tension.abs() * 4.0);
            if tension > 0.0 {
                progress.powf(exponent)
            } else {
                1.0 - (1.0 - progress).powf(exponent)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixer_master_id_volume_and_pan_canonicalize_without_collapsing_mute() {
        assert_eq!(
            canonicalize_automation_target(AutomationTarget::MixerVolume {
                track: MASTER_MIXER_TRACK_ID,
            }),
            AutomationTarget::MasterVolume
        );
        assert_eq!(
            canonicalize_automation_target(AutomationTarget::MixerPan {
                track: MASTER_MIXER_TRACK_ID,
            }),
            AutomationTarget::MasterPan
        );
        assert_eq!(
            canonicalize_automation_target(AutomationTarget::MixerMute {
                track: MASTER_MIXER_TRACK_ID,
            }),
            AutomationTarget::MixerMute {
                track: MASTER_MIXER_TRACK_ID,
            }
        );
        assert_eq!(
            canonicalize_automation_target(AutomationTarget::MixerVolume { track: 1 }),
            AutomationTarget::MixerVolume { track: 1 }
        );
    }

    fn lane_with_points(curve: AutomationCurve) -> AutomationLane {
        let mut lane = AutomationLane::new(AutomationTarget::MasterVolume);
        lane.set_curve(curve);
        lane.insert_point_at(0.0, 0.0, 0.0).unwrap();
        lane.insert_point_at(4.0, 1.0, 0.0).unwrap();
        lane
    }

    fn assert_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() <= 1.0e-10,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn normalization_sorts_clamps_deduplicates_and_drops_invalid_points() {
        let mut lane = AutomationLane::new(AutomationTarget::MasterVolume);
        let report = lane.replace_points([
            AutomationPoint::with_tension(4.0, 0.4, 5.0),
            AutomationPoint::new(f64::NAN, 0.5),
            AutomationPoint::new(-2.0, -4.0),
            AutomationPoint::new(4.0 + AUTOMATION_POSITION_EPSILON / 2.0, 0.8),
            AutomationPoint::with_tension(2.0, 5.0, f64::NAN),
        ]);

        assert!(report.changed());
        assert_eq!(report.invalid_points_removed, 1);
        assert_eq!(report.duplicate_points_merged, 1);
        assert_eq!(report.positions_clamped, 1);
        assert_eq!(report.values_clamped, 2);
        assert_eq!(report.tensions_repaired, 2);
        assert_eq!(lane.points().len(), 3);
        assert_eq!(lane.points()[0], AutomationPoint::new(0.0, 0.0));
        assert_eq!(lane.points()[1], AutomationPoint::new(2.0, 1.0));
        assert_close(lane.points()[2].value, 0.8);
    }

    #[test]
    fn duplicate_point_uses_last_input_value_even_when_positions_are_nearby() {
        let mut lane = AutomationLane::new(AutomationTarget::MasterVolume);
        lane.replace_points([
            AutomationPoint::new(1.0 + AUTOMATION_POSITION_EPSILON / 2.0, 0.2),
            AutomationPoint::new(1.0, 0.9),
        ]);
        assert_eq!(lane.points().len(), 1);
        assert_close(lane.points()[0].position, 1.0);
        assert_close(lane.points()[0].value, 0.9);
    }

    #[test]
    fn linear_sampling_holds_edges_and_interpolates_inside() {
        let lane = lane_with_points(AutomationCurve::Linear);
        assert_close(lane.evaluate(-4.0).unwrap(), 0.0);
        assert_close(lane.evaluate(0.0).unwrap(), 0.0);
        assert_close(lane.evaluate(1.0).unwrap(), 0.25);
        assert_close(lane.evaluate(2.0).unwrap(), 0.5);
        assert_close(lane.evaluate(4.0).unwrap(), 1.0);
        assert_close(lane.evaluate(40.0).unwrap(), 1.0);
        assert_eq!(lane.evaluate(f64::NAN), None);
    }

    #[test]
    fn tension_curve_is_bounded_monotonic_and_zero_tension_is_linear() {
        for tension in [-1.0, -0.5, 0.0, 0.5, 1.0] {
            let mut previous = 0.0;
            for step in 0..=100 {
                let x = f64::from(step) / 100.0;
                let value = sample_curve_progress(AutomationCurve::Tension, x, tension);
                assert!((0.0..=1.0).contains(&value));
                assert!(value + 1.0e-12 >= previous);
                previous = value;
            }
        }

        assert_close(
            sample_curve_progress(AutomationCurve::Tension, 0.25, 0.0),
            0.25,
        );
        assert!(sample_curve_progress(AutomationCurve::Tension, 0.5, 0.8) < 0.5);
        assert!(sample_curve_progress(AutomationCurve::Tension, 0.5, -0.8) > 0.5);
        assert_close(
            sample_curve_progress(AutomationCurve::Tension, 0.0, 1.0),
            0.0,
        );
        assert_close(
            sample_curve_progress(AutomationCurve::Tension, 1.0, -1.0),
            1.0,
        );
    }

    #[test]
    fn descending_tension_segment_never_overshoots_endpoint_values() {
        let mut lane = AutomationLane::new(AutomationTarget::MasterVolume);
        lane.set_curve(AutomationCurve::Tension);
        lane.insert_point_at(0.0, 1.0, -0.75).unwrap();
        lane.insert_point_at(4.0, 0.0, 0.0).unwrap();
        for step in 0..=100 {
            let value = lane.evaluate(f64::from(step) * 0.04).unwrap();
            assert!((0.0..=1.0).contains(&value));
        }
    }

    #[test]
    fn point_editing_preserves_order_and_replaces_collisions() {
        let mut lane = AutomationLane::new(AutomationTarget::MasterVolume);
        assert_eq!(lane.insert_point_at(4.0, 0.4, 0.0).unwrap(), 0);
        assert_eq!(lane.insert_point_at(0.0, 0.1, 0.0).unwrap(), 0);
        assert_eq!(lane.insert_point_at(2.0, 0.2, 0.0).unwrap(), 1);
        assert_eq!(lane.insert_point_at(2.0, 0.7, 0.3).unwrap(), 1);
        assert_eq!(lane.points().len(), 3);
        assert_close(lane.points()[1].value, 0.7);

        let new_index = lane.move_point(2, 1.0).unwrap();
        assert_eq!(new_index, 1);
        assert_eq!(
            lane.points()
                .iter()
                .map(|point| point.position)
                .collect::<Vec<_>>(),
            vec![0.0, 1.0, 2.0]
        );

        let moved = lane
            .update_point(1, AutomationPoint::with_tension(2.0, 0.9, -0.5))
            .unwrap();
        assert_eq!(moved, 1);
        assert_eq!(lane.points().len(), 2);
        assert_close(lane.points()[1].value, 0.9);
        assert_close(lane.points()[1].tension, -0.5);

        let deleted = lane.delete_point(0).unwrap();
        assert_close(deleted.position, 0.0);
        assert_eq!(lane.points().len(), 1);
    }

    #[test]
    fn loop_sampling_wraps_end_to_start_and_preserves_intro() {
        let mut lane = lane_with_points(AutomationCurve::Linear);
        let loop_region = AutomationLoop::new(0.0, 4.0).unwrap();
        lane.set_loop_region(Some(loop_region)).unwrap();

        assert_close(lane.evaluate(1.0).unwrap(), 0.25);
        assert_close(lane.evaluate(4.0).unwrap(), 0.0);
        assert_close(lane.evaluate(5.0).unwrap(), 0.25);
        assert_close(lane.evaluate(13.0).unwrap(), 0.25);

        let intro_loop = AutomationLoop::new(2.0, 4.0).unwrap();
        assert_close(
            lane.evaluate_looped(1.0, intro_loop).unwrap().unwrap(),
            0.25,
        );
        assert_close(lane.evaluate_looped(4.0, intro_loop).unwrap().unwrap(), 0.5);
        assert_close(
            lane.evaluate_looped(5.0, intro_loop).unwrap().unwrap(),
            0.75,
        );
    }

    #[test]
    fn empty_disabled_and_single_point_lanes_have_defined_results() {
        let mut lane = AutomationLane::new(AutomationTarget::Tempo);
        assert_eq!(lane.evaluate(0.0), None);
        lane.insert_point_at(8.0, 128.0, 0.0).unwrap();
        assert_close(lane.evaluate(-1.0).unwrap(), 128.0);
        assert_close(lane.evaluate(100.0).unwrap(), 128.0);
        lane.set_enabled(false);
        assert_eq!(lane.evaluate(8.0), None);
        assert_eq!(lane.evaluate_unlooped(8.0), None);
    }

    #[test]
    fn target_ranges_and_normalized_conversion_are_consistent() {
        let pan = AutomationTarget::MixerPan { track: 3 }.default_value_range();
        assert_close(pan.normalize(-1.0), 0.0);
        assert_close(pan.normalize(0.0), 0.5);
        assert_close(pan.normalize(1.0), 1.0);
        assert_close(pan.denormalize(0.25), -0.5);

        let tempo = AutomationLane::new(AutomationTarget::Tempo);
        assert_eq!(
            tempo.value_range(),
            AutomationValueRange {
                min: 10.0,
                max: 522.0
            }
        );
        let mute = AutomationLane::new(AutomationTarget::MixerMute { track: 2 });
        assert_eq!(mute.curve(), AutomationCurve::Hold);
    }

    #[test]
    fn retargeting_preserves_normalized_values_and_curve_semantics() {
        let mut lane = AutomationLane::new(AutomationTarget::MasterVolume);
        lane.insert_point_at(0.0, 0.25, 0.0).unwrap();

        lane.set_target(AutomationTarget::MasterPan);
        assert!((lane.points()[0].value + 0.5).abs() < 1.0e-12);

        lane.set_target(AutomationTarget::MixerMute { track: 2 });
        assert_eq!(lane.curve(), AutomationCurve::Hold);
        assert!((lane.points()[0].value - 0.25).abs() < 1.0e-12);

        lane.set_target(AutomationTarget::MixerVolume { track: 2 });
        assert_eq!(lane.curve(), AutomationCurve::Linear);
        assert!((lane.points()[0].value - 0.25).abs() < 1.0e-12);
    }

    #[test]
    fn serde_round_trip_preserves_lane_and_loading_repairs_raw_data() {
        let raw = r#"{
            "target":{"kind":"mixer_pan","track":4},
            "points":[
                {"position":4.0,"value":2.0,"tension":0.0},
                {"position":0.0,"value":-2.0,"tension":0.0},
                {"position":0.0,"value":0.25,"tension":0.0}
            ],
            "curve":"tension",
            "enabled":true,
            "value_range":{"min":-1.0,"max":1.0},
            "loop_region":{"start":4.0,"end":4.0}
        }"#;
        let lane: AutomationLane = serde_json::from_str(raw).unwrap();
        assert_eq!(lane.points().len(), 2);
        assert_close(lane.points()[0].value, 0.25);
        assert_close(lane.points()[1].value, 1.0);
        assert_eq!(lane.loop_region(), None);

        let encoded = serde_json::to_string(&lane).unwrap();
        let decoded: AutomationLane = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, lane);
    }

    #[test]
    fn malformed_edits_return_errors_without_mutating_the_lane() {
        let mut lane = lane_with_points(AutomationCurve::Linear);
        let original = lane.clone();
        assert_eq!(
            lane.insert_point_at(f64::NAN, 0.5, 0.0),
            Err(AutomationError::NonFinitePosition)
        );
        assert_eq!(
            lane.update_point(0, AutomationPoint::new(1.0, f64::INFINITY)),
            Err(AutomationError::NonFiniteValue)
        );
        assert_eq!(lane, original);
        assert!(matches!(
            lane.delete_point(99),
            Err(AutomationError::PointIndexOutOfBounds { .. })
        ));
        assert!(AutomationLoop::new(2.0, 2.0).is_err());
    }
}
