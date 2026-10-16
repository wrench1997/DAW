//! Shared authored ramps and independent sample arithmetic for serial tests.
use crate::{
    automation::{AutomationCurve, AutomationLane, AutomationPoint, AutomationTarget},
    mixer_graph::MASTER_MIXER_TRACK_ID,
    model::{Project, ProjectAutomation},
};

#[derive(Clone, Copy, Debug)]
pub(crate) enum Mode {
    All,
    Volume,
    Pan,
    Mute,
}

pub(crate) fn configure(project: &mut Project, music: u64, rate: u32, mode: Mode) {
    let master = project
        .mixer_tracks
        .iter_mut()
        .find(|t| t.id == MASTER_MIXER_TRACK_ID)
        .unwrap();
    master.volume = if matches!(mode, Mode::All | Mode::Volume) {
        0.0
    } else {
        0.875
    };
    master.pan = if matches!(mode, Mode::All | Mode::Pan) {
        0.9
    } else {
        -0.25
    };
    master.muted = matches!(mode, Mode::All | Mode::Mute);
    let beat = music as f64 * f64::from(project.tempo) / (f64::from(rate) * 60.0);
    for (id, target, curve, points, enabled) in [
        (
            0xF001,
            AutomationTarget::MixerVolume {
                track: MASTER_MIXER_TRACK_ID,
            },
            AutomationCurve::Linear,
            vec![(0.0, 0.0), (beat, 1.0)],
            matches!(mode, Mode::All | Mode::Volume),
        ),
        (
            0xF002,
            AutomationTarget::MixerPan {
                track: MASTER_MIXER_TRACK_ID,
            },
            AutomationCurve::Linear,
            vec![(0.0, -1.0), (beat, 1.0)],
            matches!(mode, Mode::All | Mode::Pan),
        ),
        (
            0xF003,
            AutomationTarget::MixerMute {
                track: MASTER_MIXER_TRACK_ID,
            },
            AutomationCurve::Hold,
            vec![
                (0.0, 0.0),
                (beat / 4.0, 1.0),
                (beat / 2.0, 0.0),
                (beat, 1.0),
            ],
            matches!(mode, Mode::All | Mode::Mute),
        ),
    ] {
        if enabled {
            let mut lane = AutomationLane::new(target);
            lane.set_curve(curve);
            lane.replace_points(points.into_iter().map(|(b, v)| AutomationPoint::new(b, v)));
            project.automation_lanes.push(ProjectAutomation {
                id,
                name: format!("Serial Master {id}"),
                lane,
            });
        }
    }
}

/// Closed-form authored values, not the production automation evaluator.
pub(crate) fn expected(sample: [f32; 2], output: usize, music: usize, mode: Mode) -> [f32; 2] {
    let frame = output.min(music - 1);
    let fraction = frame as f32 / music as f32;
    let volume = if matches!(mode, Mode::All | Mode::Volume) {
        fraction
    } else {
        0.875
    };
    let pan = if matches!(mode, Mode::All | Mode::Pan) {
        2.0 * fraction - 1.0
    } else {
        -0.25
    };
    let muted = matches!(mode, Mode::All | Mode::Mute) && (music / 4..music / 2).contains(&frame);
    if muted {
        [0.0, 0.0]
    } else {
        [
            (sample[0] * volume * (1.0 - pan.max(0.0))).tanh(),
            (sample[1] * volume * (1.0 + pan.min(0.0))).tanh(),
        ]
    }
}
