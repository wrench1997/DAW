use super::*;

fn snap_fixture() -> UiHarness {
    let mut ui = UiHarness::floating();
    piano_clipboard_fixture(&mut ui);
    ui.app.project.active_pattern_mut().notes.clear();
    ui.app.piano_roll_state.snap_to_scale = true;
    ui.app.piano_roll_state.scale_root = 0;
    ui.app.piano_roll_state.scale = PianoScale::Major;
    ui.app.sync_history_observer();
    ui.app.undo_stack.clear();
    ui.app.redo_stack.clear();
    ui.app.dirty = false;
    ui.app.project_fingerprint = project_fingerprint(&ui.app.project);
    ui.settle();
    ui
}
fn pointer(ui: &mut UiHarness, pos: Pos2, pressed: bool) {
    ui.run(vec![
        egui::Event::PointerMoved(pos),
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        },
    ]);
}
fn grid_point(ui: &UiHarness, beat: f64, pitch: u8) -> Pos2 {
    let rect = ui.ctx.read_response(Id::new("piano-grid")).unwrap().rect;
    let point = Pos2::new(
        rect.left() + ui.app.piano_viewport.x.pixel_for_content(beat) as f32,
        rect.top()
            + ui.app
                .piano_viewport
                .y
                .pixel_for_content(piano_pitch_row(pitch) + 0.5) as f32,
    );
    assert!(rect.contains(point));
    point
}
#[test]
fn piano_snap_off_and_triplet_paint_stamp_preserve_independent_scale_lock() {
    for snap in [PianoSnap::Off, PianoSnap::TwentyFourthBeat] {
        for tool in [
            PianoRollTool::Draw,
            PianoRollTool::Paint,
            PianoRollTool::Stamp,
        ] {
            let mut ui = snap_fixture();
            ui.app.piano_roll_state.local_snap = snap;
            ui.app.piano_roll_state.last_note_length = 0.125;
            if tool == PianoRollTool::Paint {
                ui.key(egui::Key::B, egui::Modifiers::NONE);
            } else {
                ui.app.piano_roll_state.tool = tool;
            }
            ui.settle();
            let from = grid_point(&ui, 2.137, 61);
            let to = grid_point(&ui, 2.731, 63);
            pointer(&mut ui, from, true);
            if tool == PianoRollTool::Paint {
                ui.run(vec![egui::Event::PointerMoved(to)]);
                pointer(&mut ui, to, false);
            } else {
                pointer(&mut ui, from, false);
            }
            ui.settle();
            let notes = &ui.app.project.active_pattern().notes;
            assert_eq!(
                notes.len(),
                if tool == PianoRollTool::Paint {
                    2
                } else if tool == PianoRollTool::Stamp {
                    3
                } else {
                    1
                }
            );
            assert!(
                notes
                    .iter()
                    .all(|n| crate::piano_roll::pitch_class_in_scale(n.note, 0, PianoScale::Major)),
                "Off must not bypass pitch scale: {snap:?} {tool:?}"
            );
            for note in notes {
                let input = if note.start > 2.5 { 2.731 } else { 2.137 };
                let expected = crate::piano_snap::quantize_floor(input, snap) as f32;
                assert!(
                    (note.start - expected).abs() < 0.00001,
                    "{snap:?} {tool:?}: {note:?}"
                );
                assert!(note.length > 0.0);
            }
            assert_eq!(ui.app.undo_stack.len(), 1, "{snap:?} {tool:?}");
            ui.key(egui::Key::Z, piano_clipboard_command());
            assert!(ui.app.project.active_pattern().notes.is_empty());
        }
    }
}

#[test]
fn piano_snap_dropdown_persists_local_preference_without_project_or_playlist_changes() {
    let mut ui = snap_fixture();
    let before = project_fingerprint(&ui.app.project);
    let playlist_snap = ui.app.snap;
    let node = ui
        .nodes
        .iter()
        .find(|node| node.role() == Role::ComboBox && node.value() == Some("1/4 beat"))
        .expect("local snap combo is accessible");
    let bounds = node.bounds().unwrap();
    ui.click_pos(Pos2::new(
        ((bounds.x0 + bounds.x1) / 2.0) as f32,
        ((bounds.y0 + bounds.y1) / 2.0) as f32,
    ));
    ui.click("Off");
    assert_eq!(ui.app.piano_roll_state.local_snap, PianoSnap::Off);
    assert!(ui.app.piano_roll_preferences_dirty);
    assert_eq!(ui.app.snap, playlist_snap);
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert!(!ui.app.dirty);
    assert!(ui.app.undo_stack.is_empty());
    let encoded = serde_json::to_string(&ui.app.piano_roll_state.preferences()).unwrap();
    let (prefs, error) = decode_piano_roll_preferences(Some(&encoded));
    assert!(error.is_none());
    assert_eq!(prefs.local_snap, PianoSnap::Off);
    let (prefs, error) = decode_piano_roll_preferences(Some(r#"{"version":1,"scale_root":2}"#));
    assert!(error.is_none());
    assert_eq!(prefs.local_snap, PianoSnap::QuarterBeat);
    let (prefs, error) =
        decode_piano_roll_preferences(Some(r#"{"version":1,"local_snap":"unknown"}"#));
    assert!(error.is_some());
    assert_eq!(prefs, PianoRollPreferences::default());
}

#[test]
fn piano_range_actual_minimum_floating_window_has_usable_grid_and_controls() {
    let mut ui = UiHarness::floating();
    piano_clipboard_fixture(&mut ui);
    ui.arrange_test_windows(
        &[(
            StudioView::PianoRoll,
            Rect::from_min_size(Pos2::new(20.0, 20.0), Vec2::new(480.0, 420.0)),
        )],
        StudioView::PianoRoll,
    );
    ui.app.piano_roll_state.local_snap = PianoSnap::ThirdBeat;
    ui.app.piano_roll_state.repeat_range = crate::piano_roll::PianoTimeRange::new(0.0, 4.0);
    ui.settle();
    // Keep the user's ordinary zoom; reveal just scrolls this short melody into view.
    ui.app.piano_viewport.y.reveal(63.0, 68.0, 0.0).unwrap();
    ui.settle();
    let window = ui.editor_rect(StudioView::PianoRoll);
    assert!(
        window.width() < 510.0 && window.height() < 450.0,
        "genuine compact floating window: {window:?}"
    );
    let grid = ui.ctx.read_response(Id::new("piano-grid")).unwrap().rect;
    let ruler = ui
        .ctx
        .read_response(Id::new("piano-range-ruler"))
        .unwrap()
        .rect;
    assert!(
        grid.width() > 200.0 && grid.height() >= 144.0,
        "at least six ordinary pitch rows: {grid:?}, window {window:?}"
    );
    assert!(window.contains_rect(grid) && window.contains_rect(ruler));
    assert!(
        grid.bottom() + 75.0 <= window.bottom(),
        "velocity lane stays in window"
    );
    for id in [90_001_u64, 90_002] {
        let note = ui.ctx.read_response(Id::new(("piano-note", id))).unwrap();
        assert!(note.rect.height() > 10.0 && grid.contains_rect(note.rect));
        let velocity = ui
            .ctx
            .read_response(Id::new(("piano-note", id)).with("velocity"))
            .unwrap();
        assert!(window.contains_rect(velocity.rect));
    }
    ui.capture("piano-range-floating-480x420");
    let project = project_fingerprint(&ui.app.project);
    ui.click("NOTE EDIT v");
    for label in [
        "Select all notes",
        "Copy notes",
        "Cut notes",
        "Paste notes",
        "Range from selection",
        "Range left",
        "Range right",
        "Clear range",
    ] {
        let _ = ui.button(label);
    }
    ui.capture("piano-range-floating-edit-menu");
    ui.click("Clear range");
    assert!(ui.app.piano_roll_state.repeat_range.is_none());
    ui.click("NOTE EDIT v");
    ui.click("Select all notes");
    assert_eq!(ui.app.piano_roll_state.selection_ids.len(), 2);
    ui.click("NOTE EDIT v");
    ui.click("Range from selection");
    assert_eq!(
        ui.app.piano_roll_state.repeat_range,
        crate::piano_roll::PianoTimeRange::new(0.375, 1.125)
    );
    assert_eq!(project_fingerprint(&ui.app.project), project);
    assert!(ui.app.undo_stack.is_empty());
    ui.click("NOTE EDIT v");
    ui.click("Copy notes");
    ui.click("NOTE EDIT v");
    ui.click("Paste notes");
    assert_eq!(ui.app.project.active_pattern().notes.len(), 5);
    assert_eq!(ui.app.undo_stack.len(), 1);
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), project);
    ui.click("SCALE / CHORD v");
    let shade = ui.app.piano_roll_state.scale_highlighting;
    ui.click("SHADE");
    assert_eq!(ui.app.piano_roll_state.scale_highlighting, !shade);
    ui.key(egui::Key::Escape, egui::Modifiers::NONE);
    assert_eq!(project_fingerprint(&ui.app.project), project);
}
