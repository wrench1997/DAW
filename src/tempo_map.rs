//! Deterministic musical-time conversion shared by realtime scheduling and rendering.
//!
//! This module is intentionally non-realtime. It pre-integrates the active tempo lane into
//! checkpoints so the scheduler, playhead and renderer can use the same beat/second/frame map.

use std::cmp::Ordering;

use thiserror::Error;

use crate::{
    automation::{AUTOMATION_POSITION_EPSILON, AutomationLane, AutomationTarget},
    model::{ClipKind, Project},
};

const CHECKPOINT_BEATS: f64 = 0.25;
const INTEGRATION_TOLERANCE_SECONDS: f64 = 1.0e-11;
const MAX_INTEGRATION_DEPTH: u32 = 20;
const MAX_CHECKPOINTS: usize = 262_144;
// Keep the compiled clock identical to Project normalization and AudioEngine.
// A wider UI range can be introduced later only by changing all three layers.
const MIN_TEMPO_BPM: f64 = 20.0;
const MAX_TEMPO_BPM: f64 = 400.0;

#[derive(Clone, Debug)]
struct TempoSource {
    base_bpm: f64,
    automation: Vec<TempoLaneSource>,
}

#[derive(Clone, Copy, Debug)]
struct TempoPlacement {
    start: f64,
    end: f64,
    source_start: f64,
}

#[derive(Clone, Debug)]
struct TempoLaneSource {
    lane: AutomationLane,
    /// `None` means the lane has no Playlist placements and is globally active.
    /// `Some` evaluates the lane in placement-local source beats. Later
    /// overlapping placements win, matching stable Playlist order.
    placements: Option<Vec<TempoPlacement>>,
}

impl TempoLaneSource {
    fn value_at(&self, beat: f64) -> Option<f64> {
        let source_beat = self.placements.as_ref().map_or(Some(beat), |placements| {
            placements
                .iter()
                .rev()
                .find(|placement| (placement.start..placement.end).contains(&beat))
                .map(|placement| placement.source_start + beat - placement.start)
        })?;
        self.lane.evaluate(source_beat)
    }
}

impl TempoSource {
    fn bpm_at(&self, beat: f64) -> f64 {
        self.automation
            .iter()
            .rev()
            .find_map(|lane| lane.value_at(beat))
            .filter(|bpm| bpm.is_finite())
            .unwrap_or(self.base_bpm)
            .clamp(MIN_TEMPO_BPM, MAX_TEMPO_BPM)
    }

    fn seconds_per_beat(&self, beat: f64) -> f64 {
        60.0 / self.bpm_at(beat)
    }
}

#[derive(Clone, Copy, Debug)]
struct TempoCheckpoint {
    beat: f64,
    seconds: f64,
}

/// Immutable beat/seconds/sample conversion for one project revision.
#[derive(Clone, Debug)]
pub struct TempoMap {
    source: TempoSource,
    sample_rate: u32,
    max_beat: f64,
    duration_seconds: f64,
    checkpoints: Vec<TempoCheckpoint>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TempoAdvance {
    pub beat: f64,
    pub traveled_beats: f64,
    pub completed_loops: u64,
}

#[derive(Clone, Debug, Error, PartialEq)]
pub enum TempoMapError {
    #[error("sample rate must be non-zero")]
    InvalidSampleRate,
    #[error("tempo-map end beat must be finite and greater than zero")]
    InvalidEndBeat,
    #[error("beat {0} is outside the tempo map")]
    BeatOutOfRange(f64),
    #[error("time {0} seconds is outside the tempo map")]
    SecondsOutOfRange(f64),
    #[error("elapsed time {0} seconds must be finite and non-negative")]
    InvalidElapsedSeconds(f64),
    #[error("tempo automation requires more than {MAX_CHECKPOINTS} checkpoints")]
    TooManyCheckpoints,
}

impl TempoMap {
    pub(crate) fn from_project(project: &Project, sample_rate: u32) -> Result<Self, TempoMapError> {
        let automation = project
            .automation_lanes
            .iter()
            .filter_map(|automation| {
                let lane = &automation.lane;
                if !lane.is_enabled()
                    || lane.points().is_empty()
                    || !matches!(lane.target(), AutomationTarget::Tempo)
                {
                    return None;
                }
                let clips = project
                    .clips
                    .iter()
                    .filter(|clip| {
                        clip.kind == ClipKind::Automation
                            && clip.automation_id == Some(automation.id)
                    })
                    .collect::<Vec<_>>();
                let placements = if clips.is_empty() {
                    None
                } else {
                    Some(
                        clips
                            .into_iter()
                            .filter(|clip| !clip.muted)
                            .filter_map(|clip| {
                                let start = f64::from(clip.start);
                                let end = start + f64::from(clip.length);
                                let source_start = f64::from(clip.source_offset);
                                (start.is_finite()
                                    && end.is_finite()
                                    && source_start.is_finite()
                                    && source_start >= 0.0
                                    && end > start)
                                    .then_some(TempoPlacement {
                                        start,
                                        end,
                                        source_start,
                                    })
                            })
                            .collect(),
                    )
                };
                Some(TempoLaneSource {
                    lane: lane.clone(),
                    placements,
                })
            })
            .collect();
        Self::from_source(
            TempoSource {
                base_bpm: sanitize_base_bpm(f64::from(project.tempo)),
                automation,
            },
            f64::from(project.song_length_beats),
            sample_rate,
        )
    }

    pub fn new(
        base_bpm: f64,
        automation: Option<AutomationLane>,
        max_beat: f64,
        sample_rate: u32,
    ) -> Result<Self, TempoMapError> {
        if sample_rate == 0 {
            return Err(TempoMapError::InvalidSampleRate);
        }
        if !max_beat.is_finite() || max_beat <= 0.0 {
            return Err(TempoMapError::InvalidEndBeat);
        }
        let source = TempoSource {
            base_bpm: sanitize_base_bpm(base_bpm),
            automation: automation
                .filter(|lane| {
                    lane.is_enabled()
                        && !lane.points().is_empty()
                        && matches!(lane.target(), AutomationTarget::Tempo)
                })
                .map(|lane| TempoLaneSource {
                    lane,
                    placements: None,
                })
                .into_iter()
                .collect(),
        };
        Self::from_source(source, max_beat, sample_rate)
    }

    fn from_source(
        source: TempoSource,
        max_beat: f64,
        sample_rate: u32,
    ) -> Result<Self, TempoMapError> {
        if sample_rate == 0 {
            return Err(TempoMapError::InvalidSampleRate);
        }
        if !max_beat.is_finite() || max_beat <= 0.0 {
            return Err(TempoMapError::InvalidEndBeat);
        }
        let beats = checkpoint_beats(&source, max_beat)?;
        let mut checkpoints = Vec::with_capacity(beats.len());
        let mut seconds = 0.0;
        let mut previous = beats[0];
        checkpoints.push(TempoCheckpoint {
            beat: previous,
            seconds,
        });
        for beat in beats.into_iter().skip(1) {
            seconds += integrate_seconds(&source, previous, beat);
            checkpoints.push(TempoCheckpoint { beat, seconds });
            previous = beat;
        }
        Ok(Self {
            source,
            sample_rate,
            max_beat,
            duration_seconds: seconds,
            checkpoints,
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn max_beat(&self) -> f64 {
        self.max_beat
    }

    pub fn duration_seconds(&self) -> f64 {
        self.duration_seconds
    }

    pub fn duration_frames(&self) -> u64 {
        seconds_to_frame(self.duration_seconds, self.sample_rate)
    }

    pub fn bpm_at(&self, beat: f64) -> Result<f64, TempoMapError> {
        self.validate_beat(beat)?;
        Ok(self.source.bpm_at(beat))
    }

    pub fn beat_to_seconds(&self, beat: f64) -> Result<f64, TempoMapError> {
        self.validate_beat(beat)?;
        if beat == self.max_beat {
            return Ok(self.duration_seconds);
        }
        let index = self
            .checkpoints
            .partition_point(|checkpoint| checkpoint.beat <= beat)
            .saturating_sub(1);
        let checkpoint = self.checkpoints[index];
        Ok(checkpoint.seconds + integrate_seconds(&self.source, checkpoint.beat, beat))
    }

    pub fn seconds_to_beat(&self, seconds: f64) -> Result<f64, TempoMapError> {
        self.validate_seconds(seconds)?;
        if seconds == self.duration_seconds {
            return Ok(self.max_beat);
        }
        let index = self
            .checkpoints
            .partition_point(|checkpoint| checkpoint.seconds <= seconds)
            .saturating_sub(1);
        let left = self.checkpoints[index];
        let right = self.checkpoints[index + 1];
        let target = seconds - left.seconds;
        let mut low = left.beat;
        let mut high = right.beat;
        for _ in 0..52 {
            let middle = (low + high) * 0.5;
            let elapsed = integrate_seconds(&self.source, left.beat, middle);
            if elapsed < target {
                low = middle;
            } else {
                high = middle;
            }
        }
        Ok((low + high) * 0.5)
    }

    pub fn beat_to_frame(&self, beat: f64) -> Result<u64, TempoMapError> {
        self.beat_to_seconds(beat)
            .map(|seconds| seconds_to_frame(seconds, self.sample_rate))
    }

    pub fn frame_to_beat(&self, frame: u64) -> Result<f64, TempoMapError> {
        let seconds = frame as f64 / f64::from(self.sample_rate);
        if frame > self.duration_frames() {
            return Err(TempoMapError::SecondsOutOfRange(seconds));
        }
        let seconds = seconds.min(self.duration_seconds);
        self.seconds_to_beat(seconds)
    }

    /// Advances through this map and wraps at its end without approximating
    /// tempo ramps as a single BPM. This is control/UI-thread work.
    pub fn advance_looping(
        &self,
        beat: f64,
        elapsed_seconds: f64,
    ) -> Result<TempoAdvance, TempoMapError> {
        self.validate_beat(beat)?;
        if !elapsed_seconds.is_finite() || elapsed_seconds < 0.0 {
            return Err(TempoMapError::InvalidElapsedSeconds(elapsed_seconds));
        }
        if elapsed_seconds == 0.0 {
            return Ok(TempoAdvance {
                beat,
                traveled_beats: 0.0,
                completed_loops: 0,
            });
        }
        let elapsed = self.beat_to_seconds(beat)? + elapsed_seconds;
        if !elapsed.is_finite() {
            return Err(TempoMapError::InvalidElapsedSeconds(elapsed_seconds));
        }
        let loop_count_f64 = (elapsed / self.duration_seconds).floor().max(0.0);
        let completed_loops = if loop_count_f64 >= u64::MAX as f64 {
            u64::MAX
        } else {
            loop_count_f64 as u64
        };
        let target_seconds = if completed_loops == 0 {
            elapsed
        } else {
            elapsed.rem_euclid(self.duration_seconds)
        };
        let next_beat = self.seconds_to_beat(target_seconds)?;
        let traveled_beats = if completed_loops == 0 {
            (next_beat - beat).max(0.0)
        } else {
            (self.max_beat - beat).max(0.0)
                + next_beat
                + self.max_beat * completed_loops.saturating_sub(1) as f64
        };
        Ok(TempoAdvance {
            beat: next_beat,
            traveled_beats,
            completed_loops,
        })
    }

    fn validate_beat(&self, beat: f64) -> Result<(), TempoMapError> {
        if beat.is_finite() && (0.0..=self.max_beat).contains(&beat) {
            Ok(())
        } else {
            Err(TempoMapError::BeatOutOfRange(beat))
        }
    }

    fn validate_seconds(&self, seconds: f64) -> Result<(), TempoMapError> {
        if seconds.is_finite() && (0.0..=self.duration_seconds).contains(&seconds) {
            Ok(())
        } else {
            Err(TempoMapError::SecondsOutOfRange(seconds))
        }
    }
}

fn sanitize_base_bpm(base_bpm: f64) -> f64 {
    if base_bpm.is_finite() {
        base_bpm.clamp(MIN_TEMPO_BPM, MAX_TEMPO_BPM)
    } else {
        128.0
    }
}

fn seconds_to_frame(seconds: f64, sample_rate: u32) -> u64 {
    let frames = seconds * f64::from(sample_rate);
    if frames >= u64::MAX as f64 {
        u64::MAX
    } else {
        frames.round().max(0.0) as u64
    }
}

fn checkpoint_beats(source: &TempoSource, max_beat: f64) -> Result<Vec<f64>, TempoMapError> {
    let fixed_count = (max_beat / CHECKPOINT_BEATS).ceil() as usize;
    if fixed_count.saturating_add(2) > MAX_CHECKPOINTS {
        return Err(TempoMapError::TooManyCheckpoints);
    }
    let mut beats = Vec::with_capacity(fixed_count.saturating_add(16));
    beats.push(0.0);
    for index in 1..fixed_count {
        beats.push((index as f64 * CHECKPOINT_BEATS).min(max_beat));
    }
    beats.push(max_beat);

    for source_lane in &source.automation {
        let lane = &source_lane.lane;
        if let Some(placements) = &source_lane.placements {
            for placement in placements {
                push_checkpoint(&mut beats, placement.start, max_beat)?;
                push_checkpoint(&mut beats, placement.end, max_beat)?;
                let source_end = placement.source_start + placement.end - placement.start;
                push_mapped_lane_boundaries(
                    &mut beats,
                    lane,
                    placement.start,
                    placement.source_start,
                    source_end,
                    max_beat,
                )?;
            }
        } else {
            push_mapped_lane_boundaries(&mut beats, lane, 0.0, 0.0, max_beat, max_beat)?;
        }
    }

    beats.sort_by(|left, right| left.partial_cmp(right).unwrap_or(Ordering::Equal));
    beats.dedup_by(|left, right| (*left - *right).abs() <= AUTOMATION_POSITION_EPSILON);
    if beats.len() > MAX_CHECKPOINTS {
        return Err(TempoMapError::TooManyCheckpoints);
    }
    Ok(beats)
}

fn push_checkpoint(beats: &mut Vec<f64>, beat: f64, max_beat: f64) -> Result<(), TempoMapError> {
    if (0.0..=max_beat).contains(&beat) {
        if beats.len() >= MAX_CHECKPOINTS {
            return Err(TempoMapError::TooManyCheckpoints);
        }
        beats.push(beat);
    }
    Ok(())
}

fn push_mapped_lane_boundaries(
    beats: &mut Vec<f64>,
    lane: &AutomationLane,
    timeline_start: f64,
    source_start: f64,
    source_end: f64,
    max_beat: f64,
) -> Result<(), TempoMapError> {
    let map_source = |source_beat: f64| timeline_start + source_beat - source_start;
    if let Some(loop_region) = lane.loop_region() {
        for point in lane.points().iter().filter(|point| {
            point.position < loop_region.start
                && (source_start..=source_end).contains(&point.position)
        }) {
            push_checkpoint(beats, map_source(point.position), max_beat)?;
        }
        if source_end <= loop_region.start {
            return Ok(());
        }
        let loop_length = loop_region.length();
        let first_cycle = if source_start <= loop_region.start {
            0_u64
        } else {
            ((source_start - loop_region.start) / loop_length)
                .floor()
                .max(0.0) as u64
        };
        let mut cycle = first_cycle;
        loop {
            let cycle_start = loop_region.start + cycle as f64 * loop_length;
            if cycle_start > source_end {
                break;
            }
            if cycle_start >= source_start {
                push_checkpoint(beats, map_source(cycle_start), max_beat)?;
            }
            for point in lane
                .points()
                .iter()
                .filter(|point| (loop_region.start..loop_region.end).contains(&point.position))
            {
                let repeated = cycle_start + point.position - loop_region.start;
                if (source_start..=source_end).contains(&repeated) {
                    push_checkpoint(beats, map_source(repeated), max_beat)?;
                }
            }
            cycle = cycle
                .checked_add(1)
                .ok_or(TempoMapError::TooManyCheckpoints)?;
        }
    } else {
        for point in lane
            .points()
            .iter()
            .filter(|point| (source_start..=source_end).contains(&point.position))
        {
            push_checkpoint(beats, map_source(point.position), max_beat)?;
        }
    }
    Ok(())
}

fn integrate_seconds(source: &TempoSource, start: f64, end: f64) -> f64 {
    if end <= start {
        return 0.0;
    }
    let middle = (start + end) * 0.5;
    let start_value = source.seconds_per_beat(start);
    let middle_value = source.seconds_per_beat(middle);
    // Automation points and loop boundaries may be discontinuous. Checkpoint
    // construction splits at each of them, so an interval's final sample must
    // use the left-hand limit; the exact boundary value belongs to the next
    // interval and has zero measure in this integral.
    let end_value = source.seconds_per_beat(previous_f64(end).max(start));
    let whole = simpson(start, end, start_value, middle_value, end_value);
    adaptive_simpson(
        source,
        start,
        end,
        start_value,
        middle_value,
        end_value,
        whole,
        INTEGRATION_TOLERANCE_SECONDS * (end - start).max(1.0),
        MAX_INTEGRATION_DEPTH,
    )
}

fn previous_f64(value: f64) -> f64 {
    debug_assert!(value.is_finite() && value > 0.0);
    f64::from_bits(value.to_bits() - 1)
}

#[allow(clippy::too_many_arguments)]
fn adaptive_simpson(
    source: &TempoSource,
    start: f64,
    end: f64,
    start_value: f64,
    middle_value: f64,
    end_value: f64,
    whole: f64,
    tolerance: f64,
    depth: u32,
) -> f64 {
    let middle = (start + end) * 0.5;
    let left_middle = (start + middle) * 0.5;
    let right_middle = (middle + end) * 0.5;
    let left_middle_value = source.seconds_per_beat(left_middle);
    let right_middle_value = source.seconds_per_beat(right_middle);
    let left = simpson(start, middle, start_value, left_middle_value, middle_value);
    let right = simpson(middle, end, middle_value, right_middle_value, end_value);
    let refined = left + right;
    if depth == 0 || (refined - whole).abs() <= tolerance * 15.0 {
        return refined + (refined - whole) / 15.0;
    }
    adaptive_simpson(
        source,
        start,
        middle,
        start_value,
        left_middle_value,
        middle_value,
        left,
        tolerance * 0.5,
        depth - 1,
    ) + adaptive_simpson(
        source,
        middle,
        end,
        middle_value,
        right_middle_value,
        end_value,
        right,
        tolerance * 0.5,
        depth - 1,
    )
}

fn simpson(start: f64, end: f64, start_value: f64, middle_value: f64, end_value: f64) -> f64 {
    (end - start) * (start_value + 4.0 * middle_value + end_value) / 6.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::{AutomationCurve, AutomationLoop, AutomationPoint};

    fn assert_close(actual: f64, expected: f64, tolerance: f64) {
        assert!(
            (actual - expected).abs() <= tolerance,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn constant_tempo_converts_beats_seconds_and_frames_exactly() {
        let map = TempoMap::new(120.0, None, 16.0, 48_000).unwrap();
        assert_eq!(map.sample_rate(), 48_000);
        assert_eq!(map.max_beat(), 16.0);
        assert_close(map.duration_seconds(), 8.0, 1.0e-12);
        assert_eq!(map.duration_frames(), 384_000);
        assert_close(map.beat_to_seconds(4.0).unwrap(), 2.0, 1.0e-12);
        assert_eq!(map.beat_to_frame(4.0).unwrap(), 96_000);
        assert_close(map.frame_to_beat(96_000).unwrap(), 4.0, 1.0e-11);
    }

    #[test]
    fn linear_tempo_ramp_matches_the_analytic_integral() {
        let mut lane = AutomationLane::new(AutomationTarget::Tempo);
        lane.set_curve(AutomationCurve::Linear);
        lane.replace_points([
            AutomationPoint::new(0.0, 120.0),
            AutomationPoint::new(4.0, 240.0),
        ]);
        let map = TempoMap::new(120.0, Some(lane), 4.0, 48_000).unwrap();
        let expected = 2.0 * 2.0_f64.ln();
        assert_close(map.duration_seconds(), expected, 1.0e-9);
        assert_close(map.seconds_to_beat(expected).unwrap(), 4.0, 1.0e-10);
    }

    #[test]
    fn tension_and_loop_maps_are_monotonic_and_round_trip() {
        let mut lane = AutomationLane::new(AutomationTarget::Tempo);
        lane.set_curve(AutomationCurve::Tension);
        lane.replace_points([
            AutomationPoint::with_tension(0.0, 90.0, 0.75),
            AutomationPoint::new(2.0, 180.0),
            AutomationPoint::new(4.0, 120.0),
        ]);
        lane.set_loop_region(Some(AutomationLoop::new(0.0, 4.0).unwrap()))
            .unwrap();
        let map = TempoMap::new(120.0, Some(lane), 12.0, 44_100).unwrap();
        let mut previous_seconds = 0.0;
        for step in 0..=96 {
            let beat = f64::from(step) / 8.0;
            let seconds = map.beat_to_seconds(beat).unwrap();
            assert!(seconds >= previous_seconds);
            assert_close(map.seconds_to_beat(seconds).unwrap(), beat, 1.0e-9);
            previous_seconds = seconds;
        }
        assert_close(map.bpm_at(0.5).unwrap(), map.bpm_at(4.5).unwrap(), 1.0e-9);
    }

    #[test]
    fn hold_loop_integrates_left_limits_at_discontinuous_boundaries() {
        let mut lane = AutomationLane::new(AutomationTarget::Tempo);
        lane.set_curve(AutomationCurve::Hold);
        lane.replace_points([
            AutomationPoint::new(0.0, 60.0),
            AutomationPoint::new(1.0, 120.0),
        ]);
        lane.set_loop_region(Some(AutomationLoop::new(0.0, 2.0).unwrap()))
            .unwrap();
        let map = TempoMap::new(60.0, Some(lane), 4.0, 48_000).unwrap();
        assert_close(map.duration_seconds(), 3.0, 1.0e-10);
        assert_close(map.beat_to_seconds(2.0).unwrap(), 1.5, 1.0e-10);
        assert_eq!(map.duration_frames(), 144_000);
    }

    #[test]
    fn looping_advance_preserves_elapsed_tempo_map_distance() {
        let mut lane = AutomationLane::new(AutomationTarget::Tempo);
        lane.set_curve(AutomationCurve::Hold);
        lane.replace_points([
            AutomationPoint::new(0.0, 60.0),
            AutomationPoint::new(1.0, 120.0),
        ]);
        let map = TempoMap::new(60.0, Some(lane), 2.0, 48_000).unwrap();
        let advanced = map.advance_looping(0.5, 3.0).unwrap();
        assert_eq!(advanced.completed_loops, 2);
        assert_close(advanced.beat, 0.5, 1.0e-10);
        assert_close(advanced.traveled_beats, 4.0, 1.0e-10);
        assert_eq!(map.advance_looping(2.0, 0.0).unwrap().beat, 2.0);
        assert!(matches!(
            map.advance_looping(0.0, f64::NAN),
            Err(TempoMapError::InvalidElapsedSeconds(_))
        ));
    }

    #[test]
    fn project_uses_the_last_enabled_tempo_lane() {
        let mut project = Project::default();
        let mut first = AutomationLane::new(AutomationTarget::Tempo);
        first.replace_points([AutomationPoint::new(0.0, 100.0)]);
        let mut disabled = AutomationLane::new(AutomationTarget::Tempo);
        disabled.replace_points([AutomationPoint::new(0.0, 300.0)]);
        disabled.set_enabled(false);
        let mut last = AutomationLane::new(AutomationTarget::Tempo);
        last.replace_points([AutomationPoint::new(0.0, 150.0)]);
        project
            .automation_lanes
            .push(crate::model::ProjectAutomation {
                id: 100,
                name: "First tempo".into(),
                lane: first,
            });
        project
            .automation_lanes
            .push(crate::model::ProjectAutomation {
                id: 101,
                name: "Disabled tempo".into(),
                lane: disabled,
            });
        project
            .automation_lanes
            .push(crate::model::ProjectAutomation {
                id: 102,
                name: "Last tempo".into(),
                lane: last,
            });
        let map = TempoMap::from_project(&project, 48_000).unwrap();
        assert_close(map.bpm_at(1.0).unwrap(), 150.0, 1.0e-12);
    }

    #[test]
    fn project_tempo_lanes_follow_half_open_automation_clip_gates() {
        let mut project = Project {
            song_length_beats: 6.0,
            ..Project::default()
        };
        project.automation_lanes.clear();
        project
            .clips
            .retain(|clip| clip.kind != ClipKind::Automation);

        let mut global = AutomationLane::new(AutomationTarget::Tempo);
        global.replace_points([AutomationPoint::new(0.0, 120.0)]);
        let mut placed = AutomationLane::new(AutomationTarget::Tempo);
        placed.replace_points([AutomationPoint::new(0.0, 240.0)]);
        project.automation_lanes.extend([
            crate::model::ProjectAutomation {
                id: 500,
                name: "Global tempo".into(),
                lane: global,
            },
            crate::model::ProjectAutomation {
                id: 501,
                name: "Placed tempo".into(),
                lane: placed,
            },
        ]);
        project.clips.push(crate::model::Clip {
            id: 900,
            track: 6,
            start: 2.0,
            length: 2.0,
            name: "Tempo region".into(),
            color: [1, 2, 3],
            kind: ClipKind::Automation,
            group_id: None,
            pattern_id: 1,
            automation_id: Some(501),
            audio_asset_id: None,
            source_offset: 0.0,
            audio_source_offset_frame: None,
            gain: 1.0,
            fade_in: 0.0,
            fade_out: 0.0,
            muted: false,
        });

        let map = TempoMap::from_project(&project, 48_000).unwrap();
        assert_close(map.bpm_at(1.0).unwrap(), 120.0, 1.0e-12);
        assert_close(map.bpm_at(2.0).unwrap(), 240.0, 1.0e-12);
        assert_close(map.bpm_at(3.999).unwrap(), 240.0, 1.0e-12);
        assert_close(map.bpm_at(4.0).unwrap(), 120.0, 1.0e-12);
        assert_close(map.duration_seconds(), 2.5, 1.0e-9);
    }

    #[test]
    fn tempo_placements_evaluate_source_local_beats_and_survive_moves() {
        let mut project = Project {
            song_length_beats: 8.0,
            ..Project::default()
        };
        project.automation_lanes.clear();
        project
            .clips
            .retain(|clip| clip.kind != ClipKind::Automation);
        let mut lane = AutomationLane::new(AutomationTarget::Tempo);
        lane.replace_points([
            AutomationPoint::new(0.0, 120.0),
            AutomationPoint::new(1.0, 240.0),
        ]);
        project
            .automation_lanes
            .push(crate::model::ProjectAutomation {
                id: 700,
                name: "Movable tempo source".into(),
                lane,
            });
        project.clips.push(crate::model::Clip {
            id: 901,
            track: 6,
            start: 2.0,
            length: 1.0,
            name: "Placed source window".into(),
            color: [4, 5, 6],
            kind: ClipKind::Automation,
            group_id: None,
            pattern_id: 1,
            automation_id: Some(700),
            audio_asset_id: None,
            source_offset: 0.5,
            audio_source_offset_frame: None,
            gain: 1.0,
            fade_in: 0.0,
            fade_out: 0.0,
            muted: false,
        });

        let placed = TempoMap::from_project(&project, 48_000).unwrap();
        assert_close(placed.bpm_at(2.0).unwrap(), 180.0, 1.0e-9);
        assert_close(placed.bpm_at(2.5).unwrap(), 240.0, 1.0e-9);
        assert_close(placed.bpm_at(3.0).unwrap(), 128.0, 1.0e-9);

        project.clips.last_mut().unwrap().start = 5.0;
        let moved = TempoMap::from_project(&project, 48_000).unwrap();
        assert_close(moved.bpm_at(5.0).unwrap(), 180.0, 1.0e-9);
        assert_close(moved.bpm_at(5.5).unwrap(), 240.0, 1.0e-9);
        assert_close(moved.bpm_at(2.0).unwrap(), 128.0, 1.0e-9);
    }

    #[test]
    fn rejects_invalid_domains_and_out_of_range_queries() {
        assert_eq!(
            TempoMap::new(120.0, None, 4.0, 0).unwrap_err(),
            TempoMapError::InvalidSampleRate
        );
        assert_eq!(
            TempoMap::new(120.0, None, f64::NAN, 48_000).unwrap_err(),
            TempoMapError::InvalidEndBeat
        );
        let map = TempoMap::new(120.0, None, 4.0, 48_000).unwrap();
        assert!(matches!(
            map.beat_to_seconds(-0.1),
            Err(TempoMapError::BeatOutOfRange(_))
        ));
        assert!(matches!(
            map.seconds_to_beat(map.duration_seconds() + 0.1),
            Err(TempoMapError::SecondsOutOfRange(_))
        ));
    }
}
