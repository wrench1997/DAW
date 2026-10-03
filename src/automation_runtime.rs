//! Deterministic runtime projection of project automation.
//!
//! This module contains no UI, audio-device, or synchronization code. The UI
//! thread can evaluate project lanes into a compact [`AutomationRuntimeFrame`]
//! and map its stable ordered values to audio commands before crossing the
//! realtime boundary.

use std::collections::BTreeMap;

use crate::automation::AutomationTarget;
use crate::mixer_graph::MixerTrackId;
use crate::model::ProjectAutomation;

/// Master-bus values active at a particular beat.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MasterAutomationValues {
    pub volume: Option<f64>,
    pub pan: Option<f64>,
}

/// Values for one mixer track. Runtime frames sort these by `track`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MixerAutomationValues {
    pub track: MixerTrackId,
    pub volume: Option<f64>,
    pub pan: Option<f64>,
    pub muted: Option<bool>,
}

impl MixerAutomationValues {
    const fn new(track: MixerTrackId) -> Self {
        Self {
            track,
            volume: None,
            pan: None,
            muted: None,
        }
    }
}

/// Values for one channel. Runtime frames sort these by stable channel ID.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChannelAutomationValues {
    pub channel: u32,
    pub volume: Option<f64>,
    pub pan: Option<f64>,
    pub muted: Option<bool>,
}

impl ChannelAutomationValues {
    const fn new(channel: u32) -> Self {
        Self {
            channel,
            volume: None,
            pan: None,
            muted: None,
        }
    }
}

/// One VST parameter value, sorted by `(instance, parameter)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PluginParameterAutomationValue {
    pub instance: u64,
    pub parameter: u32,
    pub value: f64,
}

/// A target/value pair in canonical dispatch order.
///
/// This is intentionally close to an audio command: consumers can match on
/// `target`, convert `value` to `f32`, and push it through their command queue.
#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeAutomationValue {
    pub target: AutomationTarget,
    pub value: f64,
}

impl RuntimeAutomationValue {
    #[must_use]
    pub fn value_f32(&self) -> f32 {
        self.value as f32
    }

    /// Returns a boolean only for discrete mute targets.
    #[must_use]
    pub fn switch_value(&self) -> Option<bool> {
        self.target.is_discrete().then_some(self.value >= 0.5)
    }
}

/// All evaluated automation at one transport beat.
///
/// Ordering is stable regardless of project-lane order: master values, tempo,
/// swing, mixer tracks, channels, then plugin parameters. IDs inside each
/// category are ascending. When several lanes target the same parameter, the
/// last enabled non-empty lane in the project slice wins.
#[derive(Clone, Debug, PartialEq)]
pub struct AutomationRuntimeFrame {
    pub beat: f64,
    pub master: MasterAutomationValues,
    pub tempo: Option<f64>,
    pub swing: Option<f64>,
    pub mixer_tracks: Vec<MixerAutomationValues>,
    pub channels: Vec<ChannelAutomationValues>,
    pub plugin_parameters: Vec<PluginParameterAutomationValue>,
}

impl AutomationRuntimeFrame {
    #[must_use]
    pub const fn empty(beat: f64) -> Self {
        Self {
            beat,
            master: MasterAutomationValues {
                volume: None,
                pan: None,
            },
            tempo: None,
            swing: None,
            mixer_tracks: Vec::new(),
            channels: Vec::new(),
            plugin_parameters: Vec::new(),
        }
    }

    /// Evaluates a project automation slice at `beat`.
    #[must_use]
    pub fn evaluate(automation: &[ProjectAutomation], beat: f64) -> Self {
        evaluate_automation_frame(automation, beat)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.master == MasterAutomationValues::default()
            && self.tempo.is_none()
            && self.swing.is_none()
            && self.mixer_tracks.is_empty()
            && self.channels.is_empty()
            && self.plugin_parameters.is_empty()
    }

    /// Visits active values in the canonical dispatch order without allocating.
    pub fn for_each_value(&self, mut visitor: impl FnMut(RuntimeAutomationValue)) {
        if let Some(value) = self.master.volume {
            visitor(RuntimeAutomationValue {
                target: AutomationTarget::MasterVolume,
                value,
            });
        }
        if let Some(value) = self.master.pan {
            visitor(RuntimeAutomationValue {
                target: AutomationTarget::MasterPan,
                value,
            });
        }
        if let Some(value) = self.tempo {
            visitor(RuntimeAutomationValue {
                target: AutomationTarget::Tempo,
                value,
            });
        }
        if let Some(value) = self.swing {
            visitor(RuntimeAutomationValue {
                target: AutomationTarget::Swing,
                value,
            });
        }
        for mixer in &self.mixer_tracks {
            if let Some(value) = mixer.volume {
                visitor(RuntimeAutomationValue {
                    target: AutomationTarget::MixerVolume { track: mixer.track },
                    value,
                });
            }
            if let Some(value) = mixer.pan {
                visitor(RuntimeAutomationValue {
                    target: AutomationTarget::MixerPan { track: mixer.track },
                    value,
                });
            }
            if let Some(value) = mixer.muted {
                visitor(RuntimeAutomationValue {
                    target: AutomationTarget::MixerMute { track: mixer.track },
                    value: f64::from(value),
                });
            }
        }
        for channel in &self.channels {
            if let Some(value) = channel.volume {
                visitor(RuntimeAutomationValue {
                    target: AutomationTarget::ChannelVolume {
                        channel: channel.channel,
                    },
                    value,
                });
            }
            if let Some(value) = channel.pan {
                visitor(RuntimeAutomationValue {
                    target: AutomationTarget::ChannelPan {
                        channel: channel.channel,
                    },
                    value,
                });
            }
            if let Some(value) = channel.muted {
                visitor(RuntimeAutomationValue {
                    target: AutomationTarget::ChannelMute {
                        channel: channel.channel,
                    },
                    value: f64::from(value),
                });
            }
        }
        for parameter in &self.plugin_parameters {
            visitor(RuntimeAutomationValue {
                target: AutomationTarget::PluginParameter {
                    instance: parameter.instance,
                    parameter: parameter.parameter,
                },
                value: parameter.value,
            });
        }
    }

    /// Materializes the canonical dispatch list for queues or diagnostics.
    #[must_use]
    pub fn ordered_values(&self) -> Vec<RuntimeAutomationValue> {
        let mut values = Vec::with_capacity(self.value_count());
        self.for_each_value(|value| values.push(value));
        values
    }

    #[must_use]
    pub fn value_count(&self) -> usize {
        usize::from(self.master.volume.is_some())
            + usize::from(self.master.pan.is_some())
            + usize::from(self.tempo.is_some())
            + usize::from(self.swing.is_some())
            + self
                .mixer_tracks
                .iter()
                .map(|values| {
                    usize::from(values.volume.is_some())
                        + usize::from(values.pan.is_some())
                        + usize::from(values.muted.is_some())
                })
                .sum::<usize>()
            + self
                .channels
                .iter()
                .map(|values| {
                    usize::from(values.volume.is_some())
                        + usize::from(values.pan.is_some())
                        + usize::from(values.muted.is_some())
                })
                .sum::<usize>()
            + self.plugin_parameters.len()
    }

    /// Looks up a target without flattening the frame.
    #[must_use]
    pub fn value_for(&self, target: &AutomationTarget) -> Option<f64> {
        match *target {
            AutomationTarget::MasterVolume => self.master.volume,
            AutomationTarget::MasterPan => self.master.pan,
            AutomationTarget::Tempo => self.tempo,
            AutomationTarget::Swing => self.swing,
            AutomationTarget::MixerVolume { track } => self
                .mixer_tracks
                .binary_search_by_key(&track, |values| values.track)
                .ok()
                .and_then(|index| self.mixer_tracks[index].volume),
            AutomationTarget::MixerPan { track } => self
                .mixer_tracks
                .binary_search_by_key(&track, |values| values.track)
                .ok()
                .and_then(|index| self.mixer_tracks[index].pan),
            AutomationTarget::MixerMute { track } => self
                .mixer_tracks
                .binary_search_by_key(&track, |values| values.track)
                .ok()
                .and_then(|index| self.mixer_tracks[index].muted)
                .map(f64::from),
            AutomationTarget::ChannelVolume { channel } => self
                .channels
                .binary_search_by_key(&channel, |values| values.channel)
                .ok()
                .and_then(|index| self.channels[index].volume),
            AutomationTarget::ChannelPan { channel } => self
                .channels
                .binary_search_by_key(&channel, |values| values.channel)
                .ok()
                .and_then(|index| self.channels[index].pan),
            AutomationTarget::ChannelMute { channel } => self
                .channels
                .binary_search_by_key(&channel, |values| values.channel)
                .ok()
                .and_then(|index| self.channels[index].muted)
                .map(f64::from),
            AutomationTarget::PluginParameter {
                instance,
                parameter,
            } => self
                .plugin_parameters
                .binary_search_by_key(&(instance, parameter), |value| {
                    (value.instance, value.parameter)
                })
                .ok()
                .map(|index| self.plugin_parameters[index].value),
        }
    }

    fn from_ordered_values(
        beat: f64,
        values: impl IntoIterator<Item = RuntimeAutomationValue>,
    ) -> Self {
        let mut frame = Self::empty(beat);
        let mut mixers = BTreeMap::<MixerTrackId, MixerAutomationValues>::new();
        let mut channels = BTreeMap::<u32, ChannelAutomationValues>::new();
        let mut plugins = BTreeMap::<(u64, u32), f64>::new();

        for runtime_value in values {
            let value = runtime_value.value;
            match runtime_value.target {
                AutomationTarget::MasterVolume => frame.master.volume = Some(value),
                AutomationTarget::MasterPan => frame.master.pan = Some(value),
                AutomationTarget::Tempo => frame.tempo = Some(value),
                AutomationTarget::Swing => frame.swing = Some(value),
                AutomationTarget::MixerVolume { track } => {
                    mixers
                        .entry(track)
                        .or_insert_with(|| MixerAutomationValues::new(track))
                        .volume = Some(value);
                }
                AutomationTarget::MixerPan { track } => {
                    mixers
                        .entry(track)
                        .or_insert_with(|| MixerAutomationValues::new(track))
                        .pan = Some(value);
                }
                AutomationTarget::MixerMute { track } => {
                    mixers
                        .entry(track)
                        .or_insert_with(|| MixerAutomationValues::new(track))
                        .muted = Some(value >= 0.5);
                }
                AutomationTarget::ChannelVolume { channel } => {
                    channels
                        .entry(channel)
                        .or_insert_with(|| ChannelAutomationValues::new(channel))
                        .volume = Some(value);
                }
                AutomationTarget::ChannelPan { channel } => {
                    channels
                        .entry(channel)
                        .or_insert_with(|| ChannelAutomationValues::new(channel))
                        .pan = Some(value);
                }
                AutomationTarget::ChannelMute { channel } => {
                    channels
                        .entry(channel)
                        .or_insert_with(|| ChannelAutomationValues::new(channel))
                        .muted = Some(value >= 0.5);
                }
                AutomationTarget::PluginParameter {
                    instance,
                    parameter,
                } => {
                    plugins.insert((instance, parameter), value);
                }
            }
        }

        frame.mixer_tracks = mixers.into_values().collect();
        frame.channels = channels.into_values().collect();
        frame.plugin_parameters = plugins
            .into_iter()
            .map(
                |((instance, parameter), value)| PluginParameterAutomationValue {
                    instance,
                    parameter,
                    value,
                },
            )
            .collect();
        frame
    }
}

impl Default for AutomationRuntimeFrame {
    fn default() -> Self {
        Self::empty(0.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum RuntimeTargetKey {
    MasterVolume,
    MasterPan,
    Tempo,
    Swing,
    MixerVolume(MixerTrackId),
    MixerPan(MixerTrackId),
    MixerMute(MixerTrackId),
    ChannelVolume(u32),
    ChannelPan(u32),
    ChannelMute(u32),
    PluginParameter(u64, u32),
}

impl From<&AutomationTarget> for RuntimeTargetKey {
    fn from(target: &AutomationTarget) -> Self {
        match *target {
            AutomationTarget::MasterVolume => Self::MasterVolume,
            AutomationTarget::MasterPan => Self::MasterPan,
            AutomationTarget::Tempo => Self::Tempo,
            AutomationTarget::Swing => Self::Swing,
            AutomationTarget::MixerVolume { track } => Self::MixerVolume(track),
            AutomationTarget::MixerPan { track } => Self::MixerPan(track),
            AutomationTarget::MixerMute { track } => Self::MixerMute(track),
            AutomationTarget::ChannelVolume { channel } => Self::ChannelVolume(channel),
            AutomationTarget::ChannelPan { channel } => Self::ChannelPan(channel),
            AutomationTarget::ChannelMute { channel } => Self::ChannelMute(channel),
            AutomationTarget::PluginParameter {
                instance,
                parameter,
            } => Self::PluginParameter(instance, parameter),
        }
    }
}

/// Evaluates all enabled non-empty lanes and returns a stable classified frame.
#[must_use]
pub fn evaluate_automation_frame(
    automation: &[ProjectAutomation],
    beat: f64,
) -> AutomationRuntimeFrame {
    if !beat.is_finite() {
        return AutomationRuntimeFrame::empty(0.0);
    }
    let mut values = BTreeMap::<RuntimeTargetKey, RuntimeAutomationValue>::new();

    for project_lane in automation {
        let lane = &project_lane.lane;
        if !lane.is_enabled() || lane.points().is_empty() {
            continue;
        }
        let Some(value) = lane.evaluate(beat).filter(|value| value.is_finite()) else {
            continue;
        };
        let target = lane.target().clone();
        values.insert(
            RuntimeTargetKey::from(&target),
            RuntimeAutomationValue { target, value },
        );
    }

    AutomationRuntimeFrame::from_ordered_values(beat, values.into_values())
}

/// A sparse frame plus targets that stopped being automated since the previous
/// frame. Absence from `changes` means "unchanged"; entries in `cleared_targets`
/// mean the consumer should restore its non-automated/project value.
#[derive(Clone, Debug, PartialEq)]
pub struct AutomationRuntimeDelta {
    pub beat: f64,
    pub changes: AutomationRuntimeFrame,
    pub cleared_targets: Vec<AutomationTarget>,
}

impl AutomationRuntimeDelta {
    #[must_use]
    pub fn between(
        previous: &AutomationRuntimeFrame,
        current: &AutomationRuntimeFrame,
        epsilon: f64,
    ) -> Self {
        let epsilon = finite_epsilon(epsilon);
        let previous_values = value_map(previous);
        let current_values = value_map(current);
        let mut changed_values = Vec::new();

        for (key, current_value) in &current_values {
            let changed = previous_values.get(key).is_none_or(|previous_value| {
                if current_value.target.is_discrete() {
                    (current_value.value >= 0.5) != (previous_value.value >= 0.5)
                } else {
                    (current_value.value - previous_value.value).abs() > epsilon
                }
            });
            if changed {
                changed_values.push(current_value.clone());
            }
        }

        let cleared_targets = previous_values
            .iter()
            .filter(|(key, _)| !current_values.contains_key(key))
            .map(|(_, value)| value.target.clone())
            .collect();
        let changes = AutomationRuntimeFrame::from_ordered_values(current.beat, changed_values);

        Self {
            beat: current.beat,
            changes,
            cleared_targets,
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty() && self.cleared_targets.is_empty()
    }
}

fn value_map(frame: &AutomationRuntimeFrame) -> BTreeMap<RuntimeTargetKey, RuntimeAutomationValue> {
    let mut values = BTreeMap::new();
    frame.for_each_value(|value| {
        values.insert(RuntimeTargetKey::from(&value.target), value);
    });
    values
}

fn finite_epsilon(epsilon: f64) -> f64 {
    if epsilon.is_finite() {
        epsilon.abs()
    } else {
        0.0
    }
}

/// Selects whether a stateful evaluator emits a complete frame or a sparse one.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum AutomationOutputMode {
    #[default]
    Full,
    Delta {
        epsilon: f64,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum AutomationRuntimeOutput {
    Full(AutomationRuntimeFrame),
    Delta(AutomationRuntimeDelta),
}

/// Stateful helper for one UI/playback session.
///
/// The first delta contains every active value. Later deltas contain only
/// changes beyond the selected epsilon plus explicit cleared targets.
#[derive(Clone, Debug, Default)]
pub struct AutomationRuntimeEvaluator {
    previous: Option<AutomationRuntimeFrame>,
}

impl AutomationRuntimeEvaluator {
    #[must_use]
    pub const fn new() -> Self {
        Self { previous: None }
    }

    pub fn evaluate(
        &mut self,
        automation: &[ProjectAutomation],
        beat: f64,
        mode: AutomationOutputMode,
    ) -> AutomationRuntimeOutput {
        let current = evaluate_automation_frame(automation, beat);
        match mode {
            AutomationOutputMode::Full => {
                self.previous = Some(current.clone());
                AutomationRuntimeOutput::Full(current)
            }
            AutomationOutputMode::Delta { epsilon } => {
                let delta = self.previous.as_ref().map_or_else(
                    || AutomationRuntimeDelta {
                        beat: current.beat,
                        changes: current.clone(),
                        cleared_targets: Vec::new(),
                    },
                    |previous| AutomationRuntimeDelta::between(previous, &current, epsilon),
                );
                self.previous = Some(current);
                AutomationRuntimeOutput::Delta(delta)
            }
        }
    }

    #[must_use]
    pub const fn previous(&self) -> Option<&AutomationRuntimeFrame> {
        self.previous.as_ref()
    }

    pub fn reset(&mut self) {
        self.previous = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::{AutomationLane, AutomationLoop, AutomationPoint};

    fn project_lane(id: u64, target: AutomationTarget, points: &[(f64, f64)]) -> ProjectAutomation {
        let mut lane = AutomationLane::new(target);
        lane.replace_points(
            points
                .iter()
                .map(|&(position, value)| AutomationPoint::new(position, value)),
        );
        ProjectAutomation {
            id,
            name: format!("Automation {id}"),
            lane,
        }
    }

    fn constant(id: u64, target: AutomationTarget, value: f64) -> ProjectAutomation {
        project_lane(id, target, &[(0.0, value)])
    }

    fn assert_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() <= 1.0e-10,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn empty_disabled_and_empty_lanes_are_filtered() {
        let mut disabled = constant(1, AutomationTarget::MasterVolume, 0.8);
        disabled.lane.set_enabled(false);
        let empty = ProjectAutomation {
            id: 2,
            name: "Empty".into(),
            lane: AutomationLane::new(AutomationTarget::Tempo),
        };

        let frame = evaluate_automation_frame(&[disabled, empty], 4.0);
        assert!(frame.is_empty());
        assert_eq!(frame.value_count(), 0);
        assert_eq!(frame.beat, 4.0);
    }

    #[test]
    fn values_are_classified_and_ids_are_sorted_stably() {
        let lanes = vec![
            constant(
                1,
                AutomationTarget::PluginParameter {
                    instance: 8,
                    parameter: 2,
                },
                0.3,
            ),
            constant(2, AutomationTarget::ChannelPan { channel: 42 }, -0.2),
            constant(3, AutomationTarget::MixerVolume { track: 9 }, 0.9),
            constant(4, AutomationTarget::MasterVolume, 0.75),
            constant(5, AutomationTarget::Tempo, 132.0),
            constant(6, AutomationTarget::MixerPan { track: 2 }, 0.25),
            constant(7, AutomationTarget::ChannelVolume { channel: 3 }, 0.6),
            constant(
                8,
                AutomationTarget::PluginParameter {
                    instance: 1,
                    parameter: 9,
                },
                0.8,
            ),
            constant(9, AutomationTarget::Swing, 0.15),
        ];

        let frame = evaluate_automation_frame(&lanes, 0.0);
        assert_close(frame.master.volume.unwrap(), 0.75);
        assert_close(frame.tempo.unwrap(), 132.0);
        assert_close(frame.swing.unwrap(), 0.15);
        assert_eq!(
            frame
                .mixer_tracks
                .iter()
                .map(|values| values.track)
                .collect::<Vec<_>>(),
            vec![2, 9]
        );
        assert_eq!(
            frame
                .channels
                .iter()
                .map(|values| values.channel)
                .collect::<Vec<_>>(),
            vec![3, 42]
        );
        assert_eq!(
            frame
                .plugin_parameters
                .iter()
                .map(|value| (value.instance, value.parameter))
                .collect::<Vec<_>>(),
            vec![(1, 9), (8, 2)]
        );
    }

    #[test]
    fn last_enabled_non_empty_lane_wins_for_the_same_target() {
        let target = AutomationTarget::MixerVolume { track: 4 };
        let first = constant(1, target.clone(), 0.2);
        let second = constant(2, target.clone(), 0.8);
        let mut disabled_last = constant(3, target.clone(), 0.1);
        disabled_last.lane.set_enabled(false);
        let empty_last = ProjectAutomation {
            id: 4,
            name: "Empty".into(),
            lane: AutomationLane::new(target.clone()),
        };

        let frame = evaluate_automation_frame(
            &[first.clone(), second.clone(), disabled_last, empty_last],
            0.0,
        );
        assert_close(frame.value_for(&target).unwrap(), 0.8);

        let reversed = evaluate_automation_frame(&[second, first], 0.0);
        assert_close(reversed.value_for(&target).unwrap(), 0.2);
    }

    #[test]
    fn canonical_dispatch_order_does_not_depend_on_lane_order() {
        let lanes = vec![
            constant(1, AutomationTarget::ChannelMute { channel: 7 }, 1.0),
            constant(2, AutomationTarget::MasterPan, -0.1),
            constant(3, AutomationTarget::MixerPan { track: 4 }, 0.2),
            constant(4, AutomationTarget::Tempo, 120.0),
            constant(5, AutomationTarget::MixerVolume { track: 4 }, 0.6),
        ];
        let mut reversed = lanes.clone();
        reversed.reverse();
        let left = evaluate_automation_frame(&lanes, 2.0).ordered_values();
        let right = evaluate_automation_frame(&reversed, 2.0).ordered_values();
        assert_eq!(left, right);
        assert_eq!(
            left.into_iter()
                .map(|value| value.target)
                .collect::<Vec<_>>(),
            vec![
                AutomationTarget::MasterPan,
                AutomationTarget::Tempo,
                AutomationTarget::MixerVolume { track: 4 },
                AutomationTarget::MixerPan { track: 4 },
                AutomationTarget::ChannelMute { channel: 7 },
            ]
        );
    }

    #[test]
    fn delta_filters_continuous_changes_at_or_below_epsilon() {
        let previous = AutomationRuntimeFrame::from_ordered_values(
            0.0,
            [RuntimeAutomationValue {
                target: AutomationTarget::MasterVolume,
                value: 0.5,
            }],
        );
        // Use a binary-exact difference for the strict-boundary assertion.
        let at_epsilon = AutomationRuntimeFrame::from_ordered_values(
            1.0,
            [RuntimeAutomationValue {
                target: AutomationTarget::MasterVolume,
                value: 0.625,
            }],
        );
        let exact = AutomationRuntimeDelta::between(&previous, &at_epsilon, 0.125);
        assert!(exact.is_empty());

        let above = AutomationRuntimeDelta::between(&previous, &at_epsilon, 0.124);
        assert_close(
            above
                .changes
                .value_for(&AutomationTarget::MasterVolume)
                .unwrap(),
            0.625,
        );
        assert!(above.cleared_targets.is_empty());
    }

    #[test]
    fn delta_reports_new_changed_and_cleared_targets_in_stable_order() {
        let previous = AutomationRuntimeFrame::from_ordered_values(
            0.0,
            [
                RuntimeAutomationValue {
                    target: AutomationTarget::MasterVolume,
                    value: 0.5,
                },
                RuntimeAutomationValue {
                    target: AutomationTarget::MixerPan { track: 2 },
                    value: 0.0,
                },
                RuntimeAutomationValue {
                    target: AutomationTarget::ChannelVolume { channel: 9 },
                    value: 0.2,
                },
            ],
        );
        let current = AutomationRuntimeFrame::from_ordered_values(
            1.0,
            [
                RuntimeAutomationValue {
                    target: AutomationTarget::MasterVolume,
                    value: 0.500_01,
                },
                RuntimeAutomationValue {
                    target: AutomationTarget::Tempo,
                    value: 128.0,
                },
            ],
        );

        let delta = AutomationRuntimeDelta::between(&previous, &current, 0.001);
        assert_eq!(delta.changes.value_count(), 1);
        assert_close(delta.changes.tempo.unwrap(), 128.0);
        assert_eq!(
            delta.cleared_targets,
            vec![
                AutomationTarget::MixerPan { track: 2 },
                AutomationTarget::ChannelVolume { channel: 9 },
            ]
        );
    }

    #[test]
    fn discrete_changes_ignore_continuous_epsilon() {
        let previous = AutomationRuntimeFrame::from_ordered_values(
            0.0,
            [RuntimeAutomationValue {
                target: AutomationTarget::MixerMute { track: 1 },
                value: 0.0,
            }],
        );
        let current = AutomationRuntimeFrame::from_ordered_values(
            1.0,
            [RuntimeAutomationValue {
                target: AutomationTarget::MixerMute { track: 1 },
                value: 1.0,
            }],
        );
        let delta = AutomationRuntimeDelta::between(&previous, &current, 100.0);
        assert_eq!(delta.changes.mixer_tracks[0].muted, Some(true));
    }

    #[test]
    fn stateful_evaluator_emits_initial_values_then_sparse_deltas_and_can_reset() {
        let lanes = [project_lane(
            1,
            AutomationTarget::MasterVolume,
            &[(0.0, 0.0), (4.0, 1.0)],
        )];
        let mut evaluator = AutomationRuntimeEvaluator::new();

        let AutomationRuntimeOutput::Delta(first) =
            evaluator.evaluate(&lanes, 0.0, AutomationOutputMode::Delta { epsilon: 0.01 })
        else {
            panic!("expected delta");
        };
        assert_eq!(first.changes.value_count(), 1);

        let AutomationRuntimeOutput::Delta(unchanged) =
            evaluator.evaluate(&lanes, 0.001, AutomationOutputMode::Delta { epsilon: 0.01 })
        else {
            panic!("expected delta");
        };
        assert!(unchanged.is_empty());

        let AutomationRuntimeOutput::Delta(changed) =
            evaluator.evaluate(&lanes, 1.0, AutomationOutputMode::Delta { epsilon: 0.01 })
        else {
            panic!("expected delta");
        };
        assert_close(changed.changes.master.volume.unwrap(), 0.25);
        assert_eq!(evaluator.previous().unwrap().beat, 1.0);

        evaluator.reset();
        assert!(evaluator.previous().is_none());
        let AutomationRuntimeOutput::Delta(after_reset) =
            evaluator.evaluate(&lanes, 1.0, AutomationOutputMode::Delta { epsilon: 1.0 })
        else {
            panic!("expected delta");
        };
        assert_eq!(after_reset.changes.value_count(), 1);
    }

    #[test]
    fn full_mode_refreshes_delta_baseline() {
        let lanes = [constant(1, AutomationTarget::Swing, 0.3)];
        let mut evaluator = AutomationRuntimeEvaluator::new();
        let output = evaluator.evaluate(&lanes, 0.0, AutomationOutputMode::Full);
        let AutomationRuntimeOutput::Full(frame) = output else {
            panic!("expected full frame");
        };
        assert_close(frame.swing.unwrap(), 0.3);

        let AutomationRuntimeOutput::Delta(delta) =
            evaluator.evaluate(&lanes, 2.0, AutomationOutputMode::Delta { epsilon: 0.0 })
        else {
            panic!("expected delta");
        };
        assert!(delta.is_empty());
    }

    #[test]
    fn lane_looping_is_reflected_in_runtime_values() {
        let mut lane = project_lane(
            1,
            AutomationTarget::ChannelVolume { channel: 3 },
            &[(0.0, 0.0), (4.0, 1.0)],
        );
        lane.lane
            .set_loop_region(Some(AutomationLoop::new(0.0, 4.0).unwrap()))
            .unwrap();
        let target = AutomationTarget::ChannelVolume { channel: 3 };
        assert_close(
            evaluate_automation_frame(std::slice::from_ref(&lane), 1.0)
                .value_for(&target)
                .unwrap(),
            0.25,
        );
        assert_close(
            evaluate_automation_frame(std::slice::from_ref(&lane), 5.0)
                .value_for(&target)
                .unwrap(),
            0.25,
        );
    }

    #[test]
    fn non_finite_beat_produces_no_dispatch_values() {
        let lanes = [constant(1, AutomationTarget::Tempo, 128.0)];
        let frame = evaluate_automation_frame(&lanes, f64::NAN);
        assert!(frame.is_empty());
        assert_eq!(frame.beat, 0.0);
    }

    #[test]
    fn runtime_types_are_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<AutomationRuntimeFrame>();
        assert_send_sync::<AutomationRuntimeDelta>();
        assert_send_sync::<AutomationRuntimeEvaluator>();
    }
}
