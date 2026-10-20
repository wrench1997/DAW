//! Real App key/pointer deletion, source ownership, and exact transaction history.
use super::*;

#[derive(Clone, Copy, Debug)]
enum DeleteInput {
    Keyboard,
    SecondaryClick,
    Eraser,
}

const ALL_INPUTS: [DeleteInput; 3] = [
    DeleteInput::Keyboard,
    DeleteInput::SecondaryClick,
    DeleteInput::Eraser,
];
const AUTOMATION_INPUTS: [DeleteInput; 2] = [DeleteInput::Keyboard, DeleteInput::Eraser];

fn json(value: &(impl serde::Serialize + ?Sized)) -> serde_json::Value {
    serde_json::to_value(value).unwrap()
}

fn source(id: u64, target: AutomationTarget, start: f64, end: f64) -> ProjectAutomation {
    let mut lane = AutomationLane::new(target);
    lane.set_curve(AutomationCurve::Tension);
    lane.replace_points([
        AutomationPoint::with_tension(0.0, start, 0.25),
        AutomationPoint::new(16.0, end),
    ]);
    ProjectAutomation {
        id,
        name: format!("Unchanged complete lane {id}"),
        lane,
    }
}

fn placement(id: u32, track: usize, start: f32, automation_id: Option<u64>) -> Clip {
    let mut clip = Project::default().clips[0].clone();
    clip.id = id;
    clip.track = track;
    clip.start = start;
    clip.length = 4.0;
    clip.name = format!("Placement {id}");
    clip.group_id = None;
    clip.automation_id = automation_id;
    clip.kind = if automation_id.is_some() {
        ClipKind::Automation
    } else {
        ClipKind::Pattern
    };
    clip
}

fn fixture_project() -> Project {
    let mut project = Project::blank();
    project.tempo = 120.0;
    project.song_length_beats = 32.0;
    project.automation_lanes = vec![
        source(500, AutomationTarget::Tempo, 120.0, 90.0),
        source(501, AutomationTarget::MasterVolume, 0.8, 0.2),
        source(600, AutomationTarget::MasterPan, -0.5, 0.5),
        source(601, AutomationTarget::Swing, 0.0, 0.3),
    ];
    project.clips = vec![
        placement(1, 0, 0.0, None),
        placement(2, 0, 8.0, None),
        placement(10, 1, 0.0, Some(600)),
        placement(11, 1, 8.0, Some(600)),
        placement(12, 2, 0.0, Some(601)),
    ];
    project
}

fn harness(project: Project) -> UiHarness {
    let mut ui = UiHarness::new();
    assert!(ui.app.install_project(project, None, false));
    ui.app.piano_roll_state.grouping_enabled = true;
    ui.app.sync_history_observer();
    ui.app.project_fingerprint = project_fingerprint(&ui.app.project);
    ui.app.undo_stack.clear();
    ui.app.redo_stack.clear();
    ui.app.dirty = false;
    ui.key(egui::Key::F5, egui::Modifiers::NONE);
    ui.key(egui::Key::E, egui::Modifiers::NONE);
    ui.settle();
    ui
}

fn delete(ui: &mut UiHarness, id: u32, input: DeleteInput) {
    ui.key(
        if matches!(input, DeleteInput::Eraser) {
            egui::Key::D
        } else {
            egui::Key::E
        },
        egui::Modifiers::NONE,
    );
    assert_eq!(
        ui.app.tool_mode,
        if matches!(input, DeleteInput::Eraser) {
            ToolMode::Delete
        } else {
            ToolMode::Select
        }
    );
    let rect = ui
        .ctx
        .read_response(Id::new(("playlist-clip", id)))
        .expect("the production clip must be visible")
        .rect;
    // Use the clip header, away from automation points and resize handles.
    let pos = Pos2::new(rect.center().x, rect.top() + 8.0);
    assert!(rect.contains(pos));
    match input {
        DeleteInput::Keyboard => {
            ui.click_pos(pos);
            assert!(ui.app.playlist_selection_ids.contains(&id));
            ui.key(egui::Key::Delete, egui::Modifiers::NONE);
        }
        DeleteInput::SecondaryClick => {
            for pressed in [true, false] {
                ui.run(vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Secondary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]);
            }
            ui.settle();
        }
        DeleteInput::Eraser => ui.click_pos(pos),
    }
    assert!(ui.app.project.clips.iter().all(|clip| clip.id != id));
    assert!(
        ui.app
            .playlist_selection_ids
            .iter()
            .all(|id| { ui.app.project.clips.iter().any(|clip| clip.id == *id) })
    );
    assert!(ui.app.playlist_gesture_before.is_none());
}

fn expected_after(before: &Project, clips: &[u32], lanes: &[u64]) -> Project {
    let mut expected = before.clone();
    expected.clips.retain(|clip| !clips.contains(&clip.id));
    expected
        .automation_lanes
        .retain(|lane| !lanes.contains(&lane.id));
    expected
        .audio_clip_mixer_destinations
        .retain(|route| !clips.contains(&route.clip_id));
    expected
}

fn assert_history(ui: &mut UiHarness, before: &Project, after: &Project) {
    assert_eq!(json(&ui.app.project), json(after));
    assert!(ui.app.dirty);
    assert_eq!(ui.app.undo_stack.len(), 1);
    assert!(ui.app.redo_stack.is_empty());
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(
        json(&ui.app.project),
        json(before),
        "one Undo restores the exact project"
    );
    assert!(ui.app.undo_stack.is_empty());
    assert_eq!(ui.app.redo_stack.len(), 1);
    ui.key(egui::Key::Y, piano_clipboard_command());
    assert_eq!(
        json(&ui.app.project),
        json(after),
        "one Redo restores only the intended deletion"
    );
    assert_eq!(ui.app.undo_stack.len(), 1);
    assert!(ui.app.redo_stack.is_empty());
}

fn assert_only_original_globals_outside_placements(ui: &UiHarness) {
    assert_eq!(
        automation_runtime_lanes_at_beat(&ui.app.project, 24.0)
            .iter()
            .map(|lane| lane.id)
            .collect::<Vec<_>>(),
        [500, 501],
        "deleted placements must never promote their source into global automation"
    );
}

#[test]
fn playlist_delete_pattern_all_inputs_preserve_global_automation_and_history() {
    for input in ALL_INPUTS {
        let mut ui = harness(fixture_project());
        let before = ui.app.project.clone();
        let expected = expected_after(&before, &[2], &[]);
        delete(&mut ui, 2, input);
        assert_eq!(
            json(&ui.app.project.automation_lanes),
            json(&before.automation_lanes)
        );
        assert_history(&mut ui, &before, &expected);
        assert_only_original_globals_outside_placements(&ui);
    }
}

#[test]
fn playlist_delete_final_automation_placement_preserves_unrelated_global_sources() {
    for input in AUTOMATION_INPUTS {
        let mut project = fixture_project();
        project.clips.retain(|clip| clip.id != 11);
        let mut ui = harness(project);
        let before = ui.app.project.clone();
        let expected = expected_after(&before, &[10], &[600]);
        delete(&mut ui, 10, input);
        assert_history(&mut ui, &before, &expected);
        assert_only_original_globals_outside_placements(&ui);
    }
}

#[test]
fn playlist_delete_shared_source_then_final_placement_are_separate_transactions() {
    for input in AUTOMATION_INPUTS {
        let mut ui = harness(fixture_project());
        let before = ui.app.project.clone();
        let after_first = expected_after(&before, &[10], &[]);
        let after_second = expected_after(&before, &[10, 11], &[600]);
        delete(&mut ui, 10, input);
        assert_eq!(json(&ui.app.project), json(&after_first));
        assert_only_original_globals_outside_placements(&ui);
        delete(&mut ui, 11, input);
        assert_eq!(json(&ui.app.project), json(&after_second));
        assert_eq!(ui.app.undo_stack.len(), 2);
        assert_only_original_globals_outside_placements(&ui);
        ui.key(egui::Key::Z, piano_clipboard_command());
        assert_eq!(json(&ui.app.project), json(&after_first));
        ui.key(egui::Key::Z, piano_clipboard_command());
        assert_eq!(json(&ui.app.project), json(&before));
        ui.key(egui::Key::Y, piano_clipboard_command());
        assert_eq!(json(&ui.app.project), json(&after_first));
        ui.key(egui::Key::Y, piano_clipboard_command());
        assert_eq!(json(&ui.app.project), json(&after_second));
    }
}

#[test]
fn playlist_delete_mixed_group_all_inputs_preserve_global_and_shared_sources() {
    for input in ALL_INPUTS {
        for keep_shared_placement in [false, true] {
            let mut project = fixture_project();
            if !keep_shared_placement {
                project.clips.retain(|clip| clip.id != 11);
            }
            for clip in &mut project.clips {
                if [1, 10].contains(&clip.id) {
                    clip.group_id = Some(77);
                }
            }
            let mut ui = harness(project);
            let before = ui.app.project.clone();
            let expected = expected_after(
                &before,
                &[1, 10],
                if keep_shared_placement { &[] } else { &[600] },
            );
            delete(&mut ui, 1, input);
            assert_history(&mut ui, &before, &expected);
            assert_only_original_globals_outside_placements(&ui);
        }
    }
}

#[test]
fn playlist_delete_disabled_grouping_leaves_automation_and_normalizes_survivor() {
    for input in ALL_INPUTS {
        let mut project = fixture_project();
        for clip in &mut project.clips {
            if [1, 10].contains(&clip.id) {
                clip.group_id = Some(77);
            }
        }
        let mut ui = harness(project);
        ui.app.piano_roll_state.grouping_enabled = false;
        let before = ui.app.project.clone();
        let mut expected = expected_after(&before, &[1], &[]);
        expected
            .clips
            .iter_mut()
            .find(|clip| clip.id == 10)
            .unwrap()
            .group_id = None;
        delete(&mut ui, 1, input);
        assert_history(&mut ui, &before, &expected);
    }
}

#[test]
fn playlist_delete_audio_all_inputs_preserve_automation_and_other_audio_route() {
    for input in ALL_INPUTS {
        let mut project = fixture_project();
        project.audio_assets.push(AudioAsset {
            id: 700,
            name: "Shared source".into(),
            path: "unavailable-playlist-deletion-fixture.wav".into(),
            sample_rate: 48_000,
            channels: 1,
            bits_per_sample: 32,
            frames: 192_000,
            waveform_peaks: Vec::new(),
        });
        for id in [1, 2] {
            let clip = project.clips.iter_mut().find(|clip| clip.id == id).unwrap();
            clip.kind = ClipKind::Audio;
            clip.audio_asset_id = Some(700);
            clip.audio_source_offset_frame = Some(0);
            project
                .audio_clip_mixer_destinations
                .push(AudioClipMixerDestination {
                    clip_id: id,
                    mixer_track_id: project.mixer_tracks[1].id,
                });
        }
        let mut ui = harness(project);
        let before = ui.app.project.clone();
        let expected = expected_after(&before, &[2], &[]);
        delete(&mut ui, 2, input);
        assert_history(&mut ui, &before, &expected);
        ui.app.project.validate_mixer_graph().unwrap();
    }
}

#[test]
fn playlist_delete_without_selection_preserves_project_dirty_state_and_history() {
    let mut ui = harness(fixture_project());
    let before = json(&ui.app.project);
    assert!(ui.app.playlist_selection_ids.is_empty());
    ui.key(egui::Key::Delete, egui::Modifiers::NONE);
    assert_eq!(json(&ui.app.project), before);
    assert!(!ui.app.dirty);
    assert!(ui.app.undo_stack.is_empty());
    assert!(ui.app.redo_stack.is_empty());
}
