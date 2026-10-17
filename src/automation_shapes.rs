//! Bounded, deterministic edits of a shared automation source.
use super::{AUTOMATION_POSITION_EPSILON, AutomationError, AutomationLane, AutomationPoint};

pub const MAX_SHAPE_POINTS: usize = 513;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutomationShape {
    Rise,
    Fall,
    EasedRise,
    Steps,
    Sine,
    Triangle,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AutomationShapeOptions {
    /// Oscillator cycles across the selected region; clamped to 1..=16.
    pub cycles: f64,
    /// Peak-to-peak normalized depth around the range midpoint; clamped to 0..=1.
    pub depth: f64,
    /// Oscillator phase in cycles; wrapped into 0..1.
    pub phase: f64,
}

impl Default for AutomationShapeOptions {
    fn default() -> Self {
        Self {
            cycles: 1.0,
            depth: 1.0,
            phase: 0.0,
        }
    }
}

impl AutomationLane {
    /// Replaces points inside the inclusive source-beat region with a shape.
    ///
    /// All outside points and lane settings (including interpolation) are retained.
    /// Boundary-adjacent segments can change; other placements using this source
    /// see the edit too. Hold interpolation remains stepped for all presets.
    /// Steps use very short ramps at jumps under Linear/Tension interpolation.
    /// Rise/Fall/EasedRise ignore cycles and phase. Other shapes repeat by cycles.
    /// Values map from normalized depth to this lane's native range (BPM, pan, etc.).
    ///
    /// Invalid inputs or unrepresentable/epsilon-colliding positions return an
    /// error without mutation. The boolean reports an actual edit: callers should
    /// apply to a candidate project and commit it as one explicit Undo transaction.
    pub fn apply_shape(
        &mut self,
        start: f64,
        end: f64,
        shape: AutomationShape,
        options: AutomationShapeOptions,
    ) -> Result<bool, AutomationError> {
        let width = end - start;
        if !start.is_finite()
            || !end.is_finite()
            || start < 0.0
            || !width.is_finite()
            || width <= AUTOMATION_POSITION_EPSILON
        {
            return Err(AutomationError::InvalidShapeRegion);
        }
        if !options.cycles.is_finite() || !options.depth.is_finite() || !options.phase.is_finite() {
            return Err(AutomationError::InvalidShapeOptions);
        }
        let cycles = options.cycles.clamp(1.0, 16.0);
        let depth = options.depth.clamp(0.0, 1.0);
        let phase = options.phase.rem_euclid(1.0);
        let mut samples = Vec::new();
        if shape == AutomationShape::Steps {
            // Eight ascending plateaus per cycle. Put both sides of each jump
            // into the source rather than changing the entire lane to Hold.
            let sample = |x: f64| ((x * cycles + phase).rem_euclid(1.0) * 8.0).floor() / 7.0;
            samples.push((0.0, sample(0.0)));
            for step in 1..=136 {
                let x = (f64::from(step) / 8.0 - phase) / cycles;
                if x <= 0.0 || x >= 1.0 {
                    continue;
                }
                let before = x - 1.0e-6 / cycles;
                if before > 0.0 {
                    samples.push((before, sample(before)));
                }
                // Floating-point division can place x just below the boundary.
                let value = f64::from(step % 8) / 7.0;
                samples.push((x, value));
            }
            samples.push((1.0, sample(1.0 - 1.0e-10)));
        } else {
            let segments = match shape {
                AutomationShape::Rise | AutomationShape::Fall
                    if self.curve() != super::AutomationCurve::Hold =>
                {
                    1
                }
                AutomationShape::Rise | AutomationShape::Fall => 32,
                AutomationShape::EasedRise => 32,
                _ => (cycles * 32.0).ceil() as usize,
            };
            for i in 0..=segments {
                let x = i as f64 / segments as f64;
                let angle = (x * cycles + phase).rem_euclid(1.0);
                let value = match shape {
                    AutomationShape::Rise => x,
                    AutomationShape::Fall => 1.0 - x,
                    AutomationShape::EasedRise => x * x * (3.0 - 2.0 * x),
                    AutomationShape::Sine => 0.5 - 0.5 * (std::f64::consts::TAU * angle).cos(),
                    AutomationShape::Triangle => 1.0 - (2.0 * angle - 1.0).abs(),
                    AutomationShape::Steps => unreachable!(),
                };
                samples.push((x, value));
            }
        }
        debug_assert!(samples.len() <= MAX_SHAPE_POINTS);
        let generated = samples
            .into_iter()
            .map(|(x, value)| {
                // Convex interpolation avoids overflow in unusual finite native ranges.
                let normalized = (0.5 + (value - 0.5) * depth).clamp(0.0, 1.0);
                let range = self.value_range();
                let value = (1.0 - normalized) * range.min + normalized * range.max;
                AutomationPoint::new(if x == 1.0 { end } else { start + x * width }, value)
            })
            .collect::<Vec<_>>();
        let mut replacement = self
            .points()
            .iter()
            .copied()
            .filter(|point| point.position < start || point.position > end)
            .chain(generated)
            .collect::<Vec<_>>();
        replacement.sort_by(|a, b| a.position.total_cmp(&b.position));
        if replacement
            .iter()
            .any(|p| !p.position.is_finite() || !p.value.is_finite())
            || replacement
                .windows(2)
                .any(|p| p[1].position - p[0].position <= AUTOMATION_POSITION_EPSILON)
        {
            return Err(AutomationError::InvalidShapeRegion);
        }
        if replacement == self.points() {
            return Ok(false);
        }
        self.replace_points(replacement);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::{
        AutomationCurve, AutomationLoop, AutomationTarget, AutomationValueRange,
    };

    const SHAPES: [AutomationShape; 6] = [
        AutomationShape::Rise,
        AutomationShape::Fall,
        AutomationShape::EasedRise,
        AutomationShape::Steps,
        AutomationShape::Sine,
        AutomationShape::Triangle,
    ];

    #[test]
    fn all_shapes_are_bounded_deterministic_native_range_edits() {
        for target in [
            AutomationTarget::MasterPan,
            AutomationTarget::Tempo,
            AutomationTarget::MasterVolume,
        ] {
            for shape in SHAPES {
                let mut lane = AutomationLane::new(target.clone());
                let range = lane.value_range();
                lane.apply_shape(
                    4.0,
                    12.0,
                    shape,
                    AutomationShapeOptions {
                        cycles: 999.0,
                        depth: 99.0,
                        phase: -0.25,
                    },
                )
                .unwrap();
                let expected = lane.clone();
                assert!(
                    !lane
                        .apply_shape(
                            4.0,
                            12.0,
                            shape,
                            AutomationShapeOptions {
                                cycles: 999.0,
                                depth: 99.0,
                                phase: -0.25
                            }
                        )
                        .unwrap()
                );
                assert_eq!(lane, expected);
                assert!(lane.points().len() <= MAX_SHAPE_POINTS);
                assert_eq!(lane.points().first().unwrap().position, 4.0);
                assert_eq!(lane.points().last().unwrap().position, 12.0);
                assert!(
                    lane.points().iter().all(|p| p.value.is_finite()
                        && p.value >= range.min
                        && p.value <= range.max)
                );
            }
        }
    }

    #[test]
    fn shape_preserves_outside_points_and_every_lane_setting() {
        let mut lane = AutomationLane::new(AutomationTarget::MasterPan);
        lane.set_curve(AutomationCurve::Tension);
        lane.set_loop_region(Some(AutomationLoop::new(0.0, 16.0).unwrap()))
            .unwrap();
        lane.set_enabled(false);
        lane.replace_points([
            AutomationPoint::with_tension(0.0, -0.7, 0.3),
            AutomationPoint::new(4.0, 0.2),
            AutomationPoint::new(8.0, 0.4),
            AutomationPoint::with_tension(16.0, 0.8, -0.3),
        ]);
        let original = lane.clone();
        lane.apply_shape(
            4.0,
            8.0,
            AutomationShape::Sine,
            AutomationShapeOptions::default(),
        )
        .unwrap();
        assert_eq!(lane.points().first(), original.points().first());
        assert_eq!(lane.points().last(), original.points().last());
        assert_eq!(lane.target(), original.target());
        assert_eq!(lane.curve(), original.curve());
        assert_eq!(lane.loop_region(), original.loop_region());
        assert_eq!(lane.is_enabled(), original.is_enabled());
        assert_eq!(lane.value_range(), original.value_range());
    }

    #[test]
    fn invalid_shapes_are_atomic_including_boundary_epsilon_collisions() {
        let mut lane = AutomationLane::new(AutomationTarget::MasterVolume);
        lane.insert_point_at(4.0 - AUTOMATION_POSITION_EPSILON / 2.0, 0.7, 0.4)
            .unwrap();
        let before = lane.clone();
        for (start, end) in [
            (-1.0, 4.0),
            (4.0, 4.0),
            (f64::NAN, 4.0),
            (0.0, f64::INFINITY),
            (4.0, 8.0),
            (1.0, 1.0 + 1.0e-8),
            (1.0e20, 1.0e20 + 32768.0),
        ] {
            assert!(
                lane.apply_shape(
                    start,
                    end,
                    AutomationShape::Sine,
                    AutomationShapeOptions::default()
                )
                .is_err()
            );
            assert_eq!(lane, before);
        }
        for options in [
            AutomationShapeOptions {
                cycles: f64::NAN,
                ..Default::default()
            },
            AutomationShapeOptions {
                depth: f64::INFINITY,
                ..Default::default()
            },
            AutomationShapeOptions {
                phase: f64::NEG_INFINITY,
                ..Default::default()
            },
        ] {
            assert!(
                lane.apply_shape(10.0, 20.0, AutomationShape::Rise, options)
                    .is_err()
            );
            assert_eq!(lane, before);
        }
    }

    #[test]
    fn waveform_values_phase_depth_and_steps_are_meaningful() {
        let mut lane = AutomationLane::new(AutomationTarget::MasterVolume);
        for (shape, middle) in [
            (AutomationShape::Rise, 0.5),
            (AutomationShape::Fall, 0.5),
            (AutomationShape::EasedRise, 0.5),
            (AutomationShape::Sine, 1.0),
            (AutomationShape::Triangle, 1.0),
        ] {
            lane.apply_shape(0.0, 8.0, shape, AutomationShapeOptions::default())
                .unwrap();
            assert!((lane.evaluate(4.0).unwrap() - middle).abs() < 1e-12);
        }
        lane.apply_shape(
            0.0,
            8.0,
            AutomationShape::Sine,
            AutomationShapeOptions {
                phase: 0.5,
                depth: 0.5,
                ..Default::default()
            },
        )
        .unwrap();
        assert!((lane.evaluate(0.0).unwrap() - 0.75).abs() < 1e-12);
        assert!((lane.evaluate(4.0).unwrap() - 0.25).abs() < 1e-12);
        lane.apply_shape(
            0.0,
            8.0,
            AutomationShape::Steps,
            AutomationShapeOptions::default(),
        )
        .unwrap();
        assert_eq!(lane.evaluate(0.5), Some(0.0));
        assert!((lane.evaluate(1.5).unwrap() - 1.0 / 7.0).abs() < 1e-12);
        assert_eq!(lane.evaluate(7.5), Some(1.0));
        assert_eq!(lane.curve(), AutomationCurve::Linear);
        lane.set_curve(AutomationCurve::Hold);
        lane.apply_shape(
            0.0,
            8.0,
            AutomationShape::Sine,
            AutomationShapeOptions::default(),
        )
        .unwrap();
        assert_eq!(lane.curve(), AutomationCurve::Hold);
    }

    #[test]
    fn finite_extreme_and_zero_width_native_ranges_stay_finite() {
        for range in [
            AutomationValueRange::new(-f64::MAX, f64::MAX).unwrap(),
            AutomationValueRange::new(12.0, 12.0).unwrap(),
        ] {
            let mut lane = AutomationLane::new(AutomationTarget::MasterVolume);
            lane.set_value_range(range).unwrap();
            lane.apply_shape(
                0.0,
                8.0,
                AutomationShape::Sine,
                AutomationShapeOptions::default(),
            )
            .unwrap();
            assert!(lane.points().iter().all(|point| point.value.is_finite()));
        }
    }

    #[test]
    fn hold_ramps_have_audible_interior_steps_without_changing_interpolation() {
        let mut lane = AutomationLane::new(AutomationTarget::MasterVolume);
        lane.set_curve(AutomationCurve::Hold);
        for shape in [AutomationShape::Rise, AutomationShape::Fall] {
            lane.apply_shape(0.0, 8.0, shape, AutomationShapeOptions::default())
                .unwrap();
            assert_eq!(lane.curve(), AutomationCurve::Hold);
            assert_eq!(lane.points().len(), 33);
            assert_eq!(lane.evaluate(4.0), Some(0.5));
            let quarter = lane.evaluate(2.0).unwrap();
            assert_eq!(
                quarter,
                if shape == AutomationShape::Rise {
                    0.25
                } else {
                    0.75
                }
            );
        }
    }
}
