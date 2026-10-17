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
