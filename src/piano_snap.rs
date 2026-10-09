//! Piano-local time snapping and viewport grid math, independent of the UI.
//!
//! A beat is a quarter note; a bar is the editor's current four-beat bar. These are
//! explicit Citrus rounding policies, not a claim about another DAW's internals.
//! Triplet positions are computed from an integer grid index and a rational step
//! in `f64`, never by accumulating an approximate `f32` step.

use serde::{Deserialize, Serialize};

/// Maximum number of vertical grid lines emitted for a single visible interval.
pub const MAX_GRID_LINES: usize = 2_048;
/// Minimum distance between adjacent emitted lines in viewport pixels.
pub const MIN_GRID_SPACING_PIXELS: f64 = 4.0;

const UNSNAPPED_COMMAND_STEP: f32 = 1.0 / 64.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PianoSnap {
    Off,
    TwentyFourthBeat,
    SixteenthBeat,
    TwelfthBeat,
    EighthBeat,
    SixthBeat,
    #[default]
    QuarterBeat,
    ThirdBeat,
    HalfBeat,
    Beat,
    TwoBeats,
    Bar,
}

impl PianoSnap {
    pub const ALL: [Self; 12] = [
        Self::Off,
        Self::TwentyFourthBeat,
        Self::SixteenthBeat,
        Self::TwelfthBeat,
        Self::EighthBeat,
        Self::SixthBeat,
        Self::QuarterBeat,
        Self::ThirdBeat,
        Self::HalfBeat,
        Self::Beat,
        Self::TwoBeats,
        Self::Bar,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::TwentyFourthBeat => "1/24 beat (Triplet)",
            Self::SixteenthBeat => "1/16 beat",
            Self::TwelfthBeat => "1/12 beat (Triplet)",
            Self::EighthBeat => "1/8 beat",
            Self::SixthBeat => "1/6 beat (Triplet)",
            Self::QuarterBeat => "1/4 beat",
            Self::ThirdBeat => "1/3 beat (Triplet)",
            Self::HalfBeat => "1/2 beat",
            Self::Beat => "1 beat",
            Self::TwoBeats => "2 beats",
            Self::Bar => "Bar (4 beats)",
        }
    }

    /// The selected step in beats. `None` means pointer snapping is disabled.
    #[must_use]
    pub const fn step(self) -> Option<f64> {
        match self.spacing() {
            Some(spacing) => Some(spacing.numerator / spacing.denominator),
            None => None,
        }
    }

    /// Discrete keyboard moves still need a distance when pointer snap is Off.
    #[must_use]
    pub const fn keyboard_step(self) -> f32 {
        match self.step() {
            Some(step) => step as f32,
            None => UNSNAPPED_COMMAND_STEP,
        }
    }

    /// Explicit transforms use the same documented 1/64-beat fallback as keys.
    #[must_use]
    pub const fn transform_step(self) -> f32 {
        self.keyboard_step()
    }

    /// Compatibility with a runtime `f32` snap value; zero represents Off.
    ///
    /// Accepts a few `f32` rounding ULPs around a supported positive step, but
    /// rejects arbitrary, negative, and non-finite values. Persist the enum, not
    /// an approximate floating-point step, for new editor preferences.
    #[must_use]
    pub fn from_step(step: f32) -> Option<Self> {
        if !step.is_finite() || step < 0.0 {
            return None;
        }
        if step == 0.0 {
            return Some(Self::Off);
        }
        Self::ALL.into_iter().find(|snap| {
            snap.step().is_some_and(|expected| {
                let expected = expected as f32;
                (step - expected).abs() <= expected * (4.0 * f32::EPSILON)
            })
        })
    }

    const fn spacing(self) -> Option<GridSpacing> {
        let (numerator, denominator) = match self {
            Self::Off => return None,
            Self::TwentyFourthBeat => (1.0, 24.0),
            Self::SixteenthBeat => (1.0, 16.0),
            Self::TwelfthBeat => (1.0, 12.0),
            Self::EighthBeat => (1.0, 8.0),
            Self::SixthBeat => (1.0, 6.0),
            Self::QuarterBeat => (1.0, 4.0),
            Self::ThirdBeat => (1.0, 3.0),
            Self::HalfBeat => (1.0, 2.0),
            Self::Beat => (1.0, 1.0),
            Self::TwoBeats => (2.0, 1.0),
            Self::Bar => (4.0, 1.0),
        };
        Some(GridSpacing {
            numerator,
            denominator,
        })
    }
}

#[derive(Clone, Copy)]
struct GridSpacing {
    numerator: f64,
    denominator: f64,
}

impl GridSpacing {
    fn index(self, beat: f64) -> f64 {
        beat * self.denominator / self.numerator
    }

    fn beat(self, index: f64) -> f64 {
        index * self.numerator / self.denominator
    }

    fn floor_index(self, raw: f64) -> f64 {
        let index = self.index(raw).floor();
        // Division/multiplication can round an exact rational grid boundary to
        // either side of an integer. Compare canonical coordinates instead of
        // using an epsilon that would swallow a real just-before-boundary input.
        if self.beat(index) > raw {
            index - 1.0
        } else if self.beat(index + 1.0) <= raw {
            index + 1.0
        } else {
            index
        }
    }
}

/// Snap toward the preceding grid point, with zero as the grid's fixed origin.
///
/// Off and non-finite inputs are returned unchanged. Validation/clamping belongs
/// to the editing operation, rather than silently turning invalid input into a
/// valid note. Finite inputs beyond precise grid-index resolution also retain
/// their value: their representable floating-point spacing already exceeds the
/// finest grid interval.
#[must_use]
pub fn quantize_floor(raw: f64, snap: PianoSnap) -> f64 {
    let Some(spacing) = snap.spacing() else {
        return raw;
    };
    let index = spacing.index(raw);
    if !raw.is_finite() || !index.is_finite() || index.abs() >= (1_u64 << 53) as f64 {
        return raw;
    }
    spacing.beat(spacing.floor_index(raw))
}

/// Snap to the nearest zero-phase grid point; exact halfway ties go away from
/// zero. Off, invalid input, and extreme finite inputs follow `quantize_floor`.
#[must_use]
pub fn quantize_nearest(raw: f64, snap: PianoSnap) -> f64 {
    let Some(spacing) = snap.spacing() else {
        return raw;
    };
    let index = spacing.index(raw);
    if !raw.is_finite() || !index.is_finite() || index.abs() >= (1_u64 << 53) as f64 {
        return raw;
    }
    spacing.beat(index.round())
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PianoGridLine {
    pub beat: f64,
    pub bar: bool,
    pub beat_line: bool,
    /// False for every line with snap Off, and for reference-only beat lines
    /// between coarser 2-beat or Bar targets. Reference lines never enable snap.
    pub snap_target: bool,
}

impl PianoGridLine {
    fn new(beat: f64, snap: PianoSnap) -> Self {
        Self {
            beat,
            bar: beat.rem_euclid(4.0) == 0.0,
            beat_line: beat.fract() == 0.0,
            snap_target: snap != PianoSnap::Off && quantize_nearest(beat, snap) == beat,
        }
    }
}

/// Generate only lines in the inclusive visible interval, sorted by beat.
///
/// Rendering is bounded by `MAX_GRID_LINES` and a four-pixel minimum spacing.
/// Dense snap grids are thinned by an integer stride on their original lattice,
/// retaining beat/bar alignment. Within a beat, the stride divides the selected
/// subdivision count; beyond a beat it follows 1, 2, 4, then multiples of 4 beats.
/// Thus a thinned triplet grid never invents straight-subdivision snap targets.
/// Snapping itself is never coarsened by this display-only thinning.
///
/// Off and coarse snaps retain beat/bar reference lines when they fit. Their
/// `snap_target` flag distinguishes those references from actual snap targets.
/// At extreme zoom-out, some beat or bar references are necessarily omitted.
/// Non-finite, empty, reversed, or non-positive-scale viewports return no lines.
#[must_use]
pub fn grid_lines(
    visible_start: f64,
    visible_end: f64,
    pixels_per_beat: f64,
    snap: PianoSnap,
) -> Vec<PianoGridLine> {
    if !visible_start.is_finite()
        || !visible_end.is_finite()
        || !pixels_per_beat.is_finite()
        || visible_end <= visible_start
        || pixels_per_beat <= 0.0
    {
        return Vec::new();
    }

    // Divide endpoints separately so an otherwise valid interval spanning both
    // signs cannot overflow merely when its width is computed.
    let line_intervals = (MAX_GRID_LINES - 1) as f64;
    let density_spacing = visible_end / line_intervals - visible_start / line_intervals;
    // Keep generated indices exactly incrementable even for pathological finite
    // coordinates far outside the editor's normal 0..4096-beat horizon.
    let precision_spacing = visible_start.abs().max(visible_end.abs()) / (1_u64 << 52) as f64;
    let minimum_spacing = (MIN_GRID_SPACING_PIXELS / pixels_per_beat)
        .max(density_spacing)
        .max(precision_spacing);

    if !minimum_spacing.is_finite() {
        return if visible_start <= 0.0 && visible_end >= 0.0 {
            vec![PianoGridLine::new(0.0, snap)]
        } else {
            Vec::new()
        };
    }

    let denominator = snap
        .spacing()
        .map_or(1, |spacing| spacing.denominator as u32);
    let spacing = display_spacing(minimum_spacing, denominator);
    let mut first_index = spacing.floor_index(visible_start);
    if spacing.beat(first_index) < visible_start {
        first_index += 1.0;
    }

    let mut lines = Vec::new();
    // A counted loop is an independent hard bound, including under extreme
    // arithmetic or viewport input. No walk starts at beat zero or the song end.
    for offset in 0..MAX_GRID_LINES {
        let beat = spacing.beat(first_index + offset as f64);
        if !beat.is_finite() || beat > visible_end {
            break;
        }
        if beat >= visible_start
            && lines
                .last()
                .is_none_or(|line: &PianoGridLine| beat > line.beat)
        {
            lines.push(PianoGridLine::new(beat, snap));
        }
    }
    lines
}

fn display_spacing(minimum_spacing: f64, denominator: u32) -> GridSpacing {
    if minimum_spacing <= 1.0 {
        for stride in 1..=denominator {
            if denominator.is_multiple_of(stride)
                && f64::from(stride) / f64::from(denominator) >= minimum_spacing
            {
                return GridSpacing {
                    numerator: f64::from(stride),
                    denominator: f64::from(denominator),
                };
            }
        }
    }
    let numerator = if minimum_spacing <= 2.0 {
        2.0
    } else {
        (minimum_spacing / 4.0).ceil() * 4.0
    };
    GridSpacing {
        numerator,
        denominator: 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn previous(value: f64) -> f64 {
        assert!(value > 0.0);
        f64::from_bits(value.to_bits() - 1)
    }

    fn following(value: f64) -> f64 {
        assert!(value > 0.0);
        f64::from_bits(value.to_bits() + 1)
    }

    #[test]
    fn choices_use_explicit_beat_units_and_round_trip_preferences() {
        assert_eq!(PianoSnap::default(), PianoSnap::QuarterBeat);
        assert_eq!(PianoSnap::ALL.len(), 12);
        for snap in PianoSnap::ALL {
            let encoded = serde_json::to_string(&snap).unwrap();
            assert_eq!(serde_json::from_str::<PianoSnap>(&encoded).unwrap(), snap);
            if let Some(step) = snap.step() {
                assert!(snap.label().contains("beat"));
                assert_eq!(PianoSnap::from_step(step as f32), Some(snap));
                assert_eq!(snap.keyboard_step(), step as f32);
                assert_eq!(snap.transform_step(), step as f32);
            }
        }
        for snap in [
            PianoSnap::TwentyFourthBeat,
            PianoSnap::TwelfthBeat,
            PianoSnap::SixthBeat,
            PianoSnap::ThirdBeat,
        ] {
            assert!(snap.label().contains("Triplet"));
        }
    }

    #[test]
    fn runtime_step_conversion_is_tolerant_but_rejects_unsupported_values() {
        assert_eq!(PianoSnap::from_step(0.0), Some(PianoSnap::Off));
        assert_eq!(PianoSnap::from_step(-0.0), Some(PianoSnap::Off));
        for snap in PianoSnap::ALL.into_iter().skip(1) {
            let step = snap.step().unwrap() as f32;
            assert_eq!(
                PianoSnap::from_step(f32::from_bits(step.to_bits() + 1)),
                Some(snap)
            );
        }
        for step in [
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            -0.25,
            0.01,
            0.3,
            3.0,
        ] {
            assert_eq!(PianoSnap::from_step(step), None);
        }
    }

    #[test]
    fn off_preserves_raw_pointer_values_and_has_explicit_command_fallbacks() {
        for raw in [-0.01, 0.0, 0.123_456_789, 4_095.987_654_321] {
            assert_eq!(quantize_floor(raw, PianoSnap::Off), raw);
            assert_eq!(quantize_nearest(raw, PianoSnap::Off), raw);
        }
        assert_eq!(PianoSnap::Off.step(), None);
        assert_eq!(PianoSnap::Off.keyboard_step(), 1.0 / 64.0);
        assert_eq!(PianoSnap::Off.transform_step(), 1.0 / 64.0);
    }

    #[test]
    fn nearest_ties_and_negative_floor_are_explicit() {
        assert_eq!(quantize_nearest(0.125, PianoSnap::QuarterBeat), 0.25);
        assert_eq!(quantize_nearest(-0.125, PianoSnap::QuarterBeat), -0.25);
        assert_eq!(quantize_floor(-0.01, PianoSnap::QuarterBeat), -0.25);
        assert_eq!(quantize_floor(-4.0, PianoSnap::Bar), -4.0);
    }

    #[test]
    fn all_grids_floor_exact_boundaries_and_one_ulp_before_and_after() {
        for snap in PianoSnap::ALL.into_iter().skip(1) {
            let spacing = snap.spacing().unwrap();
            for index in [1.0, 7.0, spacing.index(4096.0) - 1.0, spacing.index(4096.0)] {
                let boundary = spacing.beat(index);
                assert_eq!(
                    quantize_floor(boundary, snap),
                    boundary,
                    "{snap:?} {boundary}"
                );
                assert_eq!(quantize_nearest(boundary, snap), boundary);
                assert_eq!(
                    quantize_floor(previous(boundary), snap),
                    spacing.beat(index - 1.0)
                );
                assert_eq!(quantize_floor(following(boundary), snap), boundary);
            }
        }
    }

    #[test]
    fn triplets_keep_zero_phase_near_the_editor_endpoint() {
        for (snap, denominator) in [
            (PianoSnap::TwentyFourthBeat, 24),
            (PianoSnap::TwelfthBeat, 12),
            (PianoSnap::SixthBeat, 6),
            (PianoSnap::ThirdBeat, 3),
        ] {
            for index in (4095 * denominator)..=(4096 * denominator) {
                let expected = f64::from(index) / f64::from(denominator);
                assert_eq!(
                    quantize_nearest(expected + 0.1 / f64::from(denominator), snap),
                    expected
                );
                assert_eq!(quantize_floor(expected, snap), expected);
            }
        }
        assert_eq!(quantize_floor(4096.0, PianoSnap::ThirdBeat), 4096.0);
        assert_ne!(
            (4096.0_f64 / f64::from(1.0_f32 / 3.0)).floor() * f64::from(1.0_f32 / 3.0),
            4096.0
        );
    }

    #[test]
    fn quantization_preserves_nonfinite_inputs_for_validation() {
        for snap in PianoSnap::ALL {
            assert!(quantize_floor(f64::NAN, snap).is_nan());
            assert!(quantize_nearest(f64::NAN, snap).is_nan());
            for raw in [f64::INFINITY, f64::NEG_INFINITY, f64::MAX, -f64::MAX] {
                assert_eq!(quantize_floor(raw, snap), raw);
                assert_eq!(quantize_nearest(raw, snap), raw);
            }
        }
    }

    #[test]
    fn detailed_triplet_lines_are_exact_visible_snap_targets() {
        let lines = grid_lines(4095.1, 4096.0, 144.0, PianoSnap::SixthBeat);
        assert_eq!(lines.len(), 6);
        for (offset, line) in lines.iter().enumerate() {
            assert_eq!(line.beat, (24571 + offset) as f64 / 6.0);
            assert!(line.snap_target);
            assert_eq!(line.beat_line, offset == 5);
            assert_eq!(line.bar, offset == 5);
        }
    }

    #[test]
    fn off_and_coarse_snap_keep_reference_lines_without_claiming_targets() {
        for snap in [PianoSnap::Off, PianoSnap::TwoBeats, PianoSnap::Bar] {
            let lines = grid_lines(0.0, 4.0, 32.0, snap);
            assert_eq!(lines.len(), 5);
            for (index, line) in lines.iter().enumerate() {
                assert_eq!(line.beat, index as f64);
                assert!(line.beat_line);
                assert_eq!(line.bar, index == 0 || index == 4);
                let target = match snap {
                    PianoSnap::TwoBeats => index % 2 == 0,
                    PianoSnap::Bar => index % 4 == 0,
                    _ => false,
                };
                assert_eq!(line.snap_target, target);
            }
        }
    }

    #[test]
    fn triplet_thinning_is_an_integer_stride_not_a_straight_replacement() {
        let lines = grid_lines(0.0, 8.0, 30.0, PianoSnap::TwentyFourthBeat);
        assert_eq!(lines.len(), 49);
        for (index, line) in lines.iter().enumerate() {
            assert_eq!(line.beat, index as f64 * 4.0 / 24.0);
            assert!(line.snap_target);
        }
        let panned = grid_lines(0.11, 8.11, 30.0, PianoSnap::TwentyFourthBeat);
        for line in panned {
            assert!(lines.iter().any(|original| original.beat == line.beat));
        }
    }

    #[test]
    fn viewport_grid_is_bounded_sorted_visible_and_legible() {
        for snap in PianoSnap::ALL {
            for (start, end) in [
                (0.0, 4096.0),
                (4079.123, 4096.0),
                (-10.3, 20.7),
                (0.0, 1.0e12),
            ] {
                for pixels in [0.0001, 0.01, 1.0, 15.0, 48.0, 1024.0, 1.0e9] {
                    let lines = grid_lines(start, end, pixels, snap);
                    assert!(lines.len() <= MAX_GRID_LINES);
                    assert!(
                        lines
                            .iter()
                            .all(|line| line.beat >= start && line.beat <= end)
                    );
                    for adjacent in lines.windows(2) {
                        assert!(adjacent[0].beat < adjacent[1].beat);
                        assert!((adjacent[1].beat - adjacent[0].beat) * pixels >= 4.0 - 1.0e-8);
                    }
                }
            }
        }
    }

    #[test]
    fn line_cap_thins_the_full_interval_instead_of_truncating_its_front() {
        let lines = grid_lines(0.0, 4096.0, 1.0e9, PianoSnap::TwentyFourthBeat);
        assert!(lines.len() <= MAX_GRID_LINES);
        assert_eq!(lines.first().unwrap().beat, 0.0);
        assert_eq!(lines.last().unwrap().beat, 4096.0);
        assert!(lines.iter().all(|line| line.bar && line.snap_target));
    }

    #[test]
    fn invalid_viewports_are_empty() {
        for snap in PianoSnap::ALL {
            for (start, end, pixels) in [
                (0.0, 0.0, 1.0),
                (10.0, 0.0, 1.0),
                (0.0, 10.0, 0.0),
                (0.0, 10.0, -1.0),
                (f64::NAN, 10.0, 1.0),
                (0.0, f64::INFINITY, 1.0),
                (0.0, 10.0, f64::NAN),
                (0.0, 10.0, f64::INFINITY),
            ] {
                assert!(grid_lines(start, end, pixels, snap).is_empty());
            }
        }
    }

    #[test]
    fn extreme_finite_viewports_do_not_overflow_loop_or_duplicate_lines() {
        for snap in PianoSnap::ALL {
            for (start, end, pixels) in [
                (-f64::MAX, f64::MAX, 1.0),
                (1.0e300, following(1.0e300), f64::MAX),
                (0.0, f64::MAX, f64::MAX),
                (0.0, 4096.0, f64::MIN_POSITIVE),
                (0.0, 4096.0, f64::from_bits(1)),
                (1.0, 4096.0, f64::from_bits(1)),
                (0.0, f64::MIN_POSITIVE, f64::MAX),
            ] {
                let lines = grid_lines(start, end, pixels, snap);
                assert!(lines.len() <= MAX_GRID_LINES);
                assert!(
                    lines.iter().all(|line| line.beat.is_finite()
                        && line.beat >= start
                        && line.beat <= end)
                );
                assert!(lines.windows(2).all(|pair| pair[0].beat < pair[1].beat));
            }
        }
    }
}
