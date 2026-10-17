//! Production inspector menu input, shared source boundaries and explicit history.
use super::*;

fn shape_ui() -> UiHarness {
    let mut ui = UiHarness::new();
    ui.app.project = Project::default();
    ui.app.select_playlist_clip_only(9);
    ui.app.focus_editor(StudioView::Playlist);
    let clip = ui
        .app
        .project
        .clips
        .iter_mut()
        .find(|clip| clip.id == 9)
        .unwrap();
    clip.source_offset = 2.0;
    clip.length = 4.0;
    ui.app.project.automation_lanes[0].lane.replace_points([
        AutomationPoint::with_tension(0.0, 0.2, 0.3),
        AutomationPoint::new(2.0, 0.3),
        AutomationPoint::new(6.0, 0.7),
        AutomationPoint::with_tension(8.0, 0.9, -0.3),
    ]);
    ui.app.sync_history_observer();
    // The fixture represents a saved baseline, including the periodic dirty checker.
    ui.app.project_fingerprint = project_fingerprint(&ui.app.project);
    ui.app.undo_stack.clear();
    ui.app.redo_stack.clear();
    ui.app.dirty = false;
    ui.settle();
    ui
}

#[test]
fn automation_shapes_menu_applies_slipped_source_and_one_undo_redo() {
    let mut ui = shape_ui();
    let original = project_fingerprint(&ui.app.project);
    let original_points = ui.app.project.automation_lanes[0].lane.points().to_vec();
    ui.click("SHAPES");
    ui.click("Sine");
    let edited = project_fingerprint(&ui.app.project);
    assert_ne!(edited, original);
    let points = ui.app.project.automation_lanes[0].lane.points();
    assert_eq!(points.first(), original_points.first());
    assert_eq!(points.last(), original_points.last());
    assert_eq!(points[1].position, 2.0);
    assert_eq!(points[points.len() - 2].position, 6.0);
    assert_eq!(points.len(), 35);
    assert_eq!(ui.app.undo_stack.len(), 1);
    assert!(ui.app.dirty);
    let command = egui::Modifiers {
        ctrl: true,
        command: true,
        ..Default::default()
    };
    ui.key(egui::Key::Z, command);
    assert_eq!(project_fingerprint(&ui.app.project), original);
    ui.key(egui::Key::Y, command);
    assert_eq!(project_fingerprint(&ui.app.project), edited);
    ui.capture("automation-shapes-source-region");
    ui.click("SHAPES");
    ui.capture("automation-shapes-menu");
}

#[test]
fn automation_shapes_reapplying_same_preset_adds_no_history() {
    let mut ui = shape_ui();
    ui.click("SHAPES");
    ui.click("Rise");
    let edited = project_fingerprint(&ui.app.project);
    ui.click("SHAPES");
    ui.click("Rise");
    assert_eq!(project_fingerprint(&ui.app.project), edited);
    assert_eq!(ui.app.undo_stack.len(), 1);
}

#[test]
fn demo_automation_addresses_existing_keys_signal_and_honest_name() {
    let project = Project::default();
    let channel = project
        .channels
        .iter()
        .find(|channel| channel.id == 5)
        .unwrap();
    assert_eq!(channel.name, "Neon Keys");
    let automation = &project.automation_lanes[0];
    assert_eq!(automation.name, "Keys volume");
    assert_eq!(
        automation.lane.target(),
        &AutomationTarget::MixerVolume {
            track: channel.mixer_track
        }
    );
    assert_eq!(automation.lane.curve(), AutomationCurve::Tension);
    assert!(
        project
            .active_pattern()
            .notes
            .iter()
            .any(|note| note.channel_id == Some(channel.id))
    );
    assert_eq!(
        project.clips.iter().find(|clip| clip.id == 9).unwrap().name,
        "Keys volume"
    );
}

#[test]
fn automation_summary_shapes_and_curve_value_are_visible_and_read_only() {
    let mut ui = shape_ui();
    let clip = ui
        .app
        .project
        .clips
        .iter()
        .find(|clip| clip.id == 9)
        .unwrap()
        .clone();
    ui.app.beat_position = clip.start + 2.0;
    ui.app.publish_transport_seek(f64::from(clip.start + 2.0));
    ui.settle();
    let before = project_fingerprint(&ui.app.project);
    let shapes = ui.button("SHAPES").bounds().unwrap();
    assert!(
        shapes.y1 < 400.0,
        "Shapes must be near the top without scrolling: {shapes:?}"
    );
    assert!(
        ui.nodes
            .iter()
            .filter_map(|n| n.value())
            .any(|s| s == "At playhead: 0.500 (curve)")
    );
    ui.capture("automation-top-summary");
    ui.click("SHAPES");
    for label in ["Rise", "Fall", "Eased rise", "Steps", "Sine", "Triangle"] {
        let _ = ui.button(label);
    }
    ui.key(egui::Key::Escape, egui::Modifiers::NONE);
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert!(ui.app.undo_stack.is_empty());
    assert!(!ui.app.dirty);
    ui.app.project.automation_lanes[0].lane.set_enabled(false);
    ui.app
        .project
        .clips
        .iter_mut()
        .find(|c| c.id == 9)
        .unwrap()
        .muted = true;
    ui.settle();
    assert!(
        ui.nodes
            .iter()
            .filter_map(|n| n.value())
            .any(|s| s == "Clip muted")
    );
    assert!(
        ui.nodes
            .iter()
            .filter_map(|n| n.value())
            .any(|s| s == "At playhead: automation disabled")
    );
}

#[test]
fn automation_curve_readout_maps_slipped_source_and_half_open_native_values() {
    let ui = shape_ui();
    let clip = ui
        .app
        .project
        .clips
        .iter()
        .find(|clip| clip.id == 9)
        .unwrap();
    let lane = &ui.app.project.automation_lanes[0].lane;
    assert_eq!(
        automation_clip_value_readout(clip, lane, clip.start),
        "At playhead: 0.300 (curve)"
    );
    assert_eq!(
        automation_clip_value_readout(clip, lane, clip.start - 0.1),
        "At playhead: outside clip"
    );
    assert_eq!(
        automation_clip_value_readout(clip, lane, clip.start + clip.length),
        "At playhead: outside clip"
    );
    assert_eq!(
        automation_clip_value_readout(clip, lane, f32::NAN),
        "At playhead: unavailable"
    );
    let mut tempo = AutomationLane::new(AutomationTarget::Tempo);
    tempo.replace_points([
        AutomationPoint::new(2.0, 100.0),
        AutomationPoint::new(6.0, 140.0),
    ]);
    assert_eq!(
        automation_clip_value_readout(clip, &tempo, clip.start + 2.0),
        "At playhead: 120.00 BPM (curve)"
    );
    tempo.set_enabled(false);
    assert_eq!(
        automation_clip_value_readout(clip, &tempo, clip.start + 2.0),
        "At playhead: automation disabled"
    );
}

#[test]
fn automation_curve_readout_respects_native_pan_loop_and_empty_lane() {
    let ui = shape_ui();
    let clip = ui
        .app
        .project
        .clips
        .iter()
        .find(|clip| clip.id == 9)
        .unwrap();
    let mut lane = AutomationLane::new(AutomationTarget::MasterPan);
    assert_eq!(
        automation_clip_value_readout(clip, &lane, clip.start),
        "At playhead: no points"
    );
    lane.replace_points([
        AutomationPoint::new(2.0, -0.75),
        AutomationPoint::new(6.0, 0.75),
    ]);
    assert_eq!(
        automation_clip_value_readout(clip, &lane, clip.start),
        "At playhead: -0.750 (curve)"
    );
    lane.set_loop_region(Some(
        crate::automation::AutomationLoop::new(2.0, 4.0).unwrap(),
    ))
    .unwrap();
    assert_eq!(
        automation_clip_value_readout(clip, &lane, clip.start + 2.0),
        "At playhead: -0.750 (curve)"
    );
}

#[test]
fn automation_summary_shared_source_placements_read_their_own_offsets_without_history() {
    let mut ui = shape_ui();
    let mut second = ui
        .app
        .project
        .clips
        .iter()
        .find(|clip| clip.id == 9)
        .unwrap()
        .clone();
    second.id = 90_009;
    second.start = 16.0;
    second.source_offset = 0.0;
    ui.app.project.clips.push(second);
    ui.app.sync_history_observer();
    // The fixture represents a saved baseline, including the periodic dirty checker.
    ui.app.project_fingerprint = project_fingerprint(&ui.app.project);
    ui.app.undo_stack.clear();
    ui.app.redo_stack.clear();
    ui.app.dirty = false;
    let before = project_fingerprint(&ui.app.project);
    for (id, beat, expected) in [
        (9, 8.0, "At playhead: 0.300 (curve)"),
        (90_009, 16.0, "At playhead: 0.200 (curve)"),
    ] {
        ui.app.select_playlist_clip_only(id);
        ui.app.beat_position = beat;
        ui.app.publish_transport_seek(f64::from(beat));
        ui.settle();
        assert!(
            ui.nodes.iter().any(|node| node.value() == Some(expected)),
            "Selected placement {id} must use its own source offset"
        );
        assert_eq!(project_fingerprint(&ui.app.project), before);
        assert!(ui.app.undo_stack.is_empty() && ui.app.redo_stack.is_empty());
        assert!(!ui.app.dirty);
    }
}
