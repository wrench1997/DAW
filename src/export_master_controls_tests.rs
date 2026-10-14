use super::*;
use crate::{
    automation::{AutomationCurve, AutomationLane, AutomationPoint, AutomationTarget},
    mixer_graph::MASTER_MIXER_TRACK_ID,
    model::{Clip, ProjectAutomation},
};

fn project(beats: f32) -> Project {
    let mut project = Project::blank();
    project.tempo = 60.0;
    project.swing = 0.0;
    project.song_length_beats = beats;
    let master = project
        .mixer_tracks
        .iter_mut()
        .find(|track| track.id == MASTER_MIXER_TRACK_ID)
        .unwrap();
    master.volume = 0.25;
    master.pan = 0.5;
    project
}

fn lane(
    project: &mut Project,
    id: u64,
    target: AutomationTarget,
    curve: AutomationCurve,
    points: &[(f64, f64)],
) {
    let mut lane = AutomationLane::new(target);
    lane.set_curve(curve);
    lane.replace_points(
        points
            .iter()
            .map(|&(beat, value)| AutomationPoint::new(beat, value)),
    );
    project.automation_lanes.push(ProjectAutomation {
        id,
        name: format!("control {id}"),
        lane,
    });
}

fn placement(id: u32, automation: u64, start: f32, length: f32, slip: f32, muted: bool) -> Clip {
    Clip {
        id,
        track: 0,
        start,
        length,
        source_offset: slip,
        name: format!("placement {id}"),
        color: [200, 140, 40],
        kind: ClipKind::Automation,
        group_id: None,
        pattern_id: 0,
        automation_id: Some(automation),
        audio_asset_id: None,
        audio_source_offset_frame: None,
        audio_source_reference: None,
        audio_length_reference: None,
        fade_in_reference: None,
        fade_out_reference: None,
        gain: 1.0,
        fade_in: 0.0,
        fade_out: 0.0,
        muted,
    }
}

fn render(
    project: &Project,
    tail: usize,
    partitions: &[usize],
) -> (Vec<StereoFrame>, OfflineMasterControls) {
    let map = TempoMap::from_project(project, 8_000).unwrap();
    let mut controls = OfflineMasterControls::prepare(project, &map).unwrap();
    let mut output = vec![
        StereoFrame {
            left: 0.75,
            right: 0.5
        };
        map.duration_frames() as usize + tail
    ];
    let mut start = 0;
    let mut partition = 0;
    while start < output.len() {
        let frames = partitions[partition % partitions.len()].min(output.len() - start);
        controls
            .apply_block(start as u64, &mut output[start..start + frames])
            .unwrap();
        start += frames;
        partition += 1;
    }
    (output, controls)
}

#[track_caller]
fn near(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() <= 1.0e-6,
        "actual {actual}, expected {expected}"
    );
}

#[test]
fn global_master_fade_and_pan_are_sample_controls_without_static_double_gain() {
    let mut project = project(0.5);
    lane(
        &mut project,
        1,
        AutomationTarget::MasterVolume,
        AutomationCurve::Linear,
        &[(0.0, 1.0), (0.5, 0.0)],
    );
    lane(
        &mut project,
        2,
        AutomationTarget::MasterPan,
        AutomationCurve::Linear,
        &[(0.0, -1.0), (0.5, 1.0)],
    );
    let (output, controls) = render(&project, 0, &[1_024]);
    assert!(controls.drives_volume() && controls.drives_pan());
    assert_eq!(output.len(), 4_000);
    for (frame, sample) in output.iter().enumerate() {
        let gain = 1.0 - frame as f32 / 4_000.0;
        let pan = -1.0 + frame as f32 / 2_000.0;
        near(sample.left, 0.75 * gain * (1.0 - pan.max(0.0)));
        near(sample.right, 0.5 * gain * (1.0 + pan.min(0.0)));
    }
}

#[test]
fn hold_points_are_half_open_and_aliases_share_the_master_target() {
    let mut project = project(0.5);
    lane(
        &mut project,
        1,
        AutomationTarget::MixerVolume {
            track: MASTER_MIXER_TRACK_ID,
        },
        AutomationCurve::Hold,
        &[(0.0, 0.8), (0.25, 0.2)],
    );
    lane(
        &mut project,
        2,
        AutomationTarget::MixerPan {
            track: MASTER_MIXER_TRACK_ID,
        },
        AutomationCurve::Hold,
        &[(0.0, 0.0), (0.25, 1.0)],
    );
    let (output, controls) = render(&project, 0, &[17, 1, 127, 1_024]);
    assert!(controls.drives_volume() && controls.drives_pan());
    near(output[1_999].left, 0.75 * 0.8);
    near(output[1_999].right, 0.5 * 0.8);
    assert_eq!(output[2_000].left, 0.0);
    near(output[2_000].right, 0.5 * 0.2);
}

#[test]
fn slipped_overlapping_placements_restore_lower_layer_and_base_exactly() {
    let mut project = project(0.5);
    lane(
        &mut project,
        1,
        AutomationTarget::MasterVolume,
        AutomationCurve::Linear,
        &[(0.0, 0.0), (0.5, 1.0)],
    );
    lane(
        &mut project,
        2,
        AutomationTarget::MasterVolume,
        AutomationCurve::Hold,
        &[(0.0, 0.9)],
    );
    project.clips = vec![
        placement(1, 1, 0.125, 0.25, 0.25, false),
        placement(2, 2, 0.25, 0.0625, 0.0, false),
    ];
    let (output, controls) = render(&project, 0, &[1_024, 31, 127]);
    assert!(controls.drives_volume());
    assert!(!controls.drives_pan());
    near(output[999].left, 0.75 * 0.25);
    near(output[1_000].left, 0.75 * 0.5);
    near(output[1_999].left, 0.75 * (0.5 + 999.0 / 4_000.0));
    near(output[2_000].left, 0.75 * 0.9);
    near(output[2_499].left, 0.75 * 0.9);
    near(output[2_500].left, 0.75 * 0.875);
    near(output[2_999].left, 0.75 * (0.5 + 1_999.0 / 4_000.0));
    near(output[3_000].left, 0.75 * 0.25);
    // Un-driven pan remains in the caller's static transfer: this helper does
    // not apply the project's 0.5 base pan a second time.
    near(output[3_000].right, 0.5 * 0.25);
}

#[test]
fn disabled_empty_and_muted_placements_leave_existing_static_controls_alone() {
    let mut project = project(0.125);
    lane(
        &mut project,
        1,
        AutomationTarget::MasterVolume,
        AutomationCurve::Hold,
        &[(0.0, 0.9)],
    );
    project.automation_lanes[0].lane.set_enabled(false);
    lane(
        &mut project,
        2,
        AutomationTarget::MasterPan,
        AutomationCurve::Hold,
        &[(0.0, -1.0)],
    );
    project.clips = vec![placement(1, 2, 0.0, 0.125, 0.0, true)];
    let (output, controls) = render(&project, 137, &[1024]);
    assert!(!controls.drives_volume() && !controls.drives_pan());
    assert!(output.iter().all(|sample| *sample
        == StereoFrame {
            left: 0.75,
            right: 0.5
        }));
    project.automation_lanes[0].lane.set_enabled(true);
    project.automation_lanes[0].lane.replace_points([]);
    let (output, controls) = render(&project, 0, &[31]);
    assert!(!controls.drives_volume());
    assert!(output.iter().all(|sample| *sample
        == StereoFrame {
            left: 0.75,
            right: 0.5
        }));
}

#[test]
fn zero_base_is_restored_without_division_and_pan_only_does_not_apply_volume() {
    let mut project = project(0.25);
    project
        .mixer_tracks
        .iter_mut()
        .find(|track| track.id == MASTER_MIXER_TRACK_ID)
        .unwrap()
        .volume = 0.0;
    lane(
        &mut project,
        1,
        AutomationTarget::MasterVolume,
        AutomationCurve::Hold,
        &[(0.0, 0.8)],
    );
    project.clips = vec![placement(1, 1, 0.0625, 0.0625, 0.0, false)];
    let (output, _) = render(&project, 0, &[511, 1, 63]);
    assert_eq!(output[499].left, 0.0);
    near(output[500].left, 0.75 * 0.8);
    assert_eq!(output[1_000].left, 0.0);
    project.automation_lanes.clear();
    project.clips.clear();
    lane(
        &mut project,
        2,
        AutomationTarget::MasterPan,
        AutomationCurve::Hold,
        &[(0.0, -1.0)],
    );
    let (output, controls) = render(&project, 0, &[127]);
    assert!(!controls.drives_volume() && controls.drives_pan());
    assert!(
        output
            .iter()
            .all(|sample| sample.left == 0.75 && sample.right == 0.0)
    );
}

#[test]
fn caller_partition_and_serialized_cold_repeat_match_and_tail_holds_last_music_sample() {
    let mut project = project(0.5);
    lane(
        &mut project,
        1,
        AutomationTarget::MasterVolume,
        AutomationCurve::Linear,
        &[(0.0, 1.0), (0.5, 0.0)],
    );
    lane(
        &mut project,
        2,
        AutomationTarget::MasterPan,
        AutomationCurve::Linear,
        &[(0.0, -1.0), (0.5, 1.0)],
    );
    let before = serde_json::to_vec(&project).unwrap();
    let restored: Project = serde_json::from_slice(&before).unwrap();
    let (reference, _) = render(&project, 2_100, &[1_024]);
    for partitions in [&[1, 31, 127][..], &[511, 2_048, 17][..]] {
        let (actual, _) = render(&restored, 2_100, partitions);
        for (actual, expected) in actual.iter().zip(&reference) {
            near(actual.left, expected.left);
            near(actual.right, expected.right);
        }
        assert!(
            actual[4_000..]
                .iter()
                .all(|sample| *sample == actual[3_999])
        );
    }
    assert_eq!(serde_json::to_vec(&project).unwrap(), before);
}

#[test]
fn original_tempo_map_controls_sample_positions_and_tail_does_not_restart_events() {
    let mut project = project(0.5);
    lane(
        &mut project,
        1,
        AutomationTarget::Tempo,
        AutomationCurve::Hold,
        &[(0.0, 60.0), (0.25, 120.0)],
    );
    lane(
        &mut project,
        2,
        AutomationTarget::MasterVolume,
        AutomationCurve::Hold,
        &[(0.0, 0.8), (0.25, 0.2)],
    );
    let (output, controls) = render(&project, 4_000, &[31, 1_024]);
    assert_eq!(controls.music_frames, 3_000);
    near(output[1_999].left, 0.75 * 0.8);
    near(output[2_000].left, 0.75 * 0.2);
    assert!(
        output[3_000..]
            .iter()
            .all(|sample| *sample == output[2_999])
    );
    assert_eq!(controls.executor.next_frame(), 3_000);
    assert_eq!(controls.automation.next_frame(), 3_000);
}

#[test]
fn out_of_order_ranges_reject_before_mutating_pcm_and_cancel_never_opens_a_destination() {
    let mut project = project(0.25);
    lane(
        &mut project,
        1,
        AutomationTarget::MasterVolume,
        AutomationCurve::Hold,
        &[(0.0, 0.8)],
    );
    let map = TempoMap::from_project(&project, 8_000).unwrap();
    let mut controls = OfflineMasterControls::prepare(&project, &map).unwrap();
    let mut samples = [StereoFrame {
        left: 0.75,
        right: 0.5,
    }; 2];
    let before = samples;
    assert!(controls.apply_block(1, &mut samples).is_err());
    assert_eq!(samples, before);
    controls.apply_block(0, &mut samples).unwrap();
    let applied = samples;
    assert!(controls.apply_block(0, &mut samples).is_err());
    assert_eq!(samples, applied);
    let mut controls = OfflineMasterControls::prepare(&project, &map).unwrap();
    let control = ExportControl::default();
    control.cancel_at(6_500);
    let mut samples = vec![
        StereoFrame {
            left: 0.75,
            right: 0.5
        };
        2_000
    ];
    let before = samples.clone();
    let error = controls.apply_all(&mut samples, &control).unwrap_err();
    assert!(
        error
            .downcast_ref::<crate::export_job::ExportCancelled>()
            .is_some()
    );
    assert_eq!(samples, before);
}

#[test]
fn source_projection_cannot_hide_cross_namespace_identity_collisions() {
    let mut project = project(0.25);
    lane(
        &mut project,
        1,
        AutomationTarget::MasterVolume,
        AutomationCurve::Hold,
        &[(0.0, 0.8)],
    );
    project.clips = vec![placement(7, 1, 0.0, 0.25, 0.0, false)];
    let mut music = project.clips[0].clone();
    music.kind = ClipKind::Pattern;
    music.automation_id = None;
    project.clips.push(music);
    let map = TempoMap::from_project(&project, 8_000).unwrap();
    assert!(
        OfflineMasterControls::prepare(&project, &map)
            .err()
            .unwrap()
            .to_string()
            .contains("placement 7")
    );
    project.clips.pop();
    lane(
        &mut project,
        1,
        AutomationTarget::ChannelVolume { channel: 1 },
        AutomationCurve::Hold,
        &[(0.0, 0.4)],
    );
    assert!(
        OfflineMasterControls::prepare(&project, &map)
            .err()
            .unwrap()
            .to_string()
            .contains("lane 1")
    );
}

#[test]
fn master_mute_threshold_is_exact_and_does_not_apply_undriven_volume_or_pan() {
    let mut project = project(0.5);
    lane(
        &mut project,
        1,
        AutomationTarget::MixerMute {
            track: MASTER_MIXER_TRACK_ID,
        },
        AutomationCurve::Hold,
        &[(0.0, 0.499), (0.125, 0.5), (0.25, 0.501), (0.375, 0.0)],
    );
    let (output, controls) = render(&project, 0, &[1, 17, 1_024]);
    assert!(controls.drives_mute());
    assert!(!controls.drives_volume() && !controls.drives_pan());
    for (frame, sample) in output.iter().enumerate() {
        let expected = if (1_000..3_000).contains(&frame) {
            StereoFrame::default()
        } else {
            StereoFrame {
                left: 0.75,
                right: 0.5,
            }
        };
        assert_eq!(*sample, expected, "mute threshold at frame {frame}");
    }
    project.automation_lanes[0]
        .lane
        .set_curve(AutomationCurve::Linear);
    project.automation_lanes[0].lane.replace_points([
        AutomationPoint::new(0.0, 0.0),
        AutomationPoint::new(0.5, 1.0),
    ]);
    let (output, _) = render(&project, 0, &[31, 127, 1_024]);
    for (frame, sample) in output.iter().enumerate() {
        let expected = if frame < 2_000 {
            StereoFrame {
                left: 0.75,
                right: 0.5,
            }
        } else {
            StereoFrame::default()
        };
        assert_eq!(*sample, expected, "linear mute threshold at frame {frame}");
    }
}

#[test]
fn slipped_master_mute_layers_restore_lower_layer_and_muted_base_at_half_open_ends() {
    let mut project = project(0.5);
    project.mixer_tracks[0].muted = true;
    lane(
        &mut project,
        1,
        AutomationTarget::MixerMute {
            track: MASTER_MIXER_TRACK_ID,
        },
        AutomationCurve::Hold,
        &[(0.0, 1.0), (0.125, 0.0), (0.25, 1.0)],
    );
    lane(
        &mut project,
        2,
        AutomationTarget::MixerMute {
            track: MASTER_MIXER_TRACK_ID,
        },
        AutomationCurve::Hold,
        &[(0.0, 0.0)],
    );
    project.clips = vec![
        placement(1, 1, 0.125, 0.25, 0.125, false),
        placement(2, 2, 0.25, 0.0625, 0.0, false),
    ];
    let (output, controls) = render(&project, 137, &[511, 1, 63]);
    assert!(controls.drives_mute());
    for (frame, sample) in output.iter().enumerate() {
        assert_eq!(
            sample.left,
            if (1_000..2_500).contains(&frame) {
                0.75
            } else {
                0.0
            }
        );
    }
    assert!(controls.last_muted);
}

#[test]
fn inactive_master_mute_lanes_do_not_remove_the_static_master_gate() {
    let mut project = project(0.125);
    project.mixer_tracks[0].muted = true;
    lane(
        &mut project,
        1,
        AutomationTarget::MixerMute {
            track: MASTER_MIXER_TRACK_ID,
        },
        AutomationCurve::Hold,
        &[(0.0, 0.0)],
    );
    for inactive in 0..3 {
        let mut inactive_project = project.clone();
        match inactive {
            0 => inactive_project.automation_lanes[0].lane.set_enabled(false),
            1 => {
                inactive_project.automation_lanes[0].lane.replace_points([]);
            }
            _ => inactive_project
                .clips
                .push(placement(1, 1, 0.0, 0.125, 0.0, true)),
        }
        let (output, controls) = render(&inactive_project, 100, &[31]);
        assert!(!controls.drives_mute());
        // This helper receives audio after static mixing. An undriven mute is
        // the mixer's duty, just as undriven volume and pan are.
        assert!(
            output
                .iter()
                .all(|sample| sample.left == 0.75 && sample.right == 0.5)
        );
    }
}

#[test]
fn master_mute_partitions_and_cold_pcm_match_and_tail_holds_last_music_gate() {
    for last_value in [0.0, 1.0] {
        let mut project = project(0.5);
        project.mixer_tracks[0].muted = true;
        lane(
            &mut project,
            1,
            AutomationTarget::MixerMute {
                track: MASTER_MIXER_TRACK_ID,
            },
            AutomationCurve::Hold,
            &[(0.0, 0.0), (0.25, last_value)],
        );
        project.clips = vec![placement(1, 1, 0.0, 0.5, 0.0, false)];
        let serialized = serde_json::to_vec(&project).unwrap();
        let cold: Project = serde_json::from_slice(&serialized).unwrap();
        let (reference, _) = render(&project, 2_100, &[1_024]);
        for partitions in [&[1, 31, 127][..], &[511, 2_048, 17][..]] {
            let (actual, controls) = render(&cold, 2_100, partitions);
            assert_eq!(actual, reference);
            assert!(
                actual[4_000..]
                    .iter()
                    .all(|sample| *sample == actual[3_999])
            );
            assert_eq!(controls.last_muted, last_value >= 0.5);
            assert_eq!(controls.executor.next_frame(), 4_000);
            assert_eq!(controls.automation.next_frame(), 4_000);
        }
        assert_eq!(serde_json::to_vec(&project).unwrap(), serialized);
    }
}

#[test]
fn automated_master_mute_does_not_hide_non_finite_source_audio() {
    let mut project = project(0.125);
    lane(
        &mut project,
        1,
        AutomationTarget::MixerMute {
            track: MASTER_MIXER_TRACK_ID,
        },
        AutomationCurve::Hold,
        &[(0.0, 1.0)],
    );
    let map = TempoMap::from_project(&project, 8_000).unwrap();
    let mut controls = OfflineMasterControls::prepare(&project, &map).unwrap();
    let mut samples = [StereoFrame {
        left: f32::NAN,
        right: 0.5,
    }];
    let error = controls.apply_block(0, &mut samples).unwrap_err();
    assert!(error.to_string().contains("non-finite"));
    assert!(controls.apply_block(0, &mut samples).is_err());
}
