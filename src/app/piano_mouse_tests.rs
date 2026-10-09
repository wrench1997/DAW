use super::*;

fn mouse(ui: &mut UiHarness, pos: Pos2, pressed: Option<bool>, modifiers: egui::Modifiers) {
    let mut events = vec![egui::Event::PointerMoved(pos)];
    if let Some(pressed) = pressed {
        events.push(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers,
        });
    }
    ui.run_with_modifiers(events, modifiers);
}
fn point(ui: &UiHarness, beat: f64, pitch: u8) -> Pos2 {
    let grid = ui.ctx.read_response(Id::new("piano-grid")).unwrap().rect;
    let p = Pos2::new(
        grid.left() + ui.app.piano_viewport.x.pixel_for_content(beat) as f32,
        grid.top()
            + ui.app
                .piano_viewport
                .y
                .pixel_for_content(piano_pitch_row(pitch) + 0.5) as f32,
    );
    assert!(grid.contains(p), "{p:?} not inside {grid:?}");
    p
}
fn body(ui: &UiHarness, id: u64) -> Pos2 {
    ui.ctx
        .read_response(Id::new(("piano-note", id)))
        .unwrap()
        .rect
        .center()
}
fn click(ui: &mut UiHarness, p: Pos2, modifiers: egui::Modifiers) {
    mouse(ui, p, Some(true), modifiers);
    mouse(ui, p, Some(false), modifiers);
    ui.settle();
}
fn drag(ui: &mut UiHarness, from: Pos2, to: Pos2, before: egui::Modifiers, after: egui::Modifiers) {
    mouse(ui, from, Some(true), before);
    mouse(ui, from, None, after);
    mouse(ui, to, None, after);
    mouse(ui, to, Some(false), after);
    ui.settle();
}
fn fixture() -> UiHarness {
    let mut ui = UiHarness::floating();
    piano_clipboard_fixture(&mut ui);
    ui.app.piano_roll_state.tool = PianoRollTool::Draw;
    ui
}
fn note(ui: &UiHarness, id: u64) -> &PianoNote {
    ui.app
        .project
        .active_pattern()
        .notes
        .iter()
        .find(|n| n.id == id)
        .unwrap()
}

#[test]
fn piano_mouse_ctrl_selection_and_marquee_keep_draw_and_history_clean() {
    let mut ui = fixture();
    let before = project_fingerprint(&ui.app.project);
    let ctrl = egui::Modifiers::CTRL;
    let both = egui::Modifiers {
        shift: true,
        ..ctrl
    };
    let p = body(&ui, 90001);
    click(&mut ui, p, ctrl);
    assert_eq!(
        ui.app.piano_roll_state.selection_ids,
        HashSet::from([90001, 90002])
    );
    click(&mut ui, p, both);
    assert!(ui.app.piano_roll_state.selection_ids.is_empty());
    let from = point(&ui, 0.1, 65);
    let to = point(&ui, 1.5, 60);
    drag(&mut ui, from, to, ctrl, ctrl);
    assert_eq!(
        ui.app.piano_roll_state.selection_ids,
        HashSet::from([90001, 90002])
    );
    assert_eq!(ui.app.piano_roll_state.tool, PianoRollTool::Draw);
    let empty = point(&ui, 2.0, 62);
    click(&mut ui, empty, ctrl);
    assert!(ui.app.piano_roll_state.selection_ids.is_empty());
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert!(ui.app.undo_stack.is_empty());
    assert!(!ui.app.dirty);
    // Modifier ownership comes from down, even if Ctrl is released before button-up.
    mouse(&mut ui, empty, Some(true), ctrl);
    mouse(&mut ui, empty, Some(false), egui::Modifiers::NONE);
    assert_eq!(project_fingerprint(&ui.app.project), before);
}

#[test]
fn piano_mouse_draw_is_one_note_paint_is_many_and_shift_draws_length() {
    let mut ui = fixture();
    let from = point(&ui, 2.0, 62);
    let middle = point(&ui, 3.0, 63);
    let to = point(&ui, 4.0, 65);
    mouse(&mut ui, from, Some(true), egui::Modifiers::NONE);
    mouse(&mut ui, middle, None, egui::Modifiers::NONE);
    mouse(&mut ui, to, None, egui::Modifiers::NONE);
    mouse(&mut ui, to, Some(false), egui::Modifiers::NONE);
    ui.settle();
    assert_eq!(ui.app.project.active_pattern().notes.len(), 4);
    let added = ui
        .app
        .project
        .active_pattern()
        .notes
        .iter()
        .find(|n| n.id < 90000)
        .unwrap();
    assert_eq!((added.start, added.note, added.length), (4.0, 65, 1.0));
    assert_eq!(ui.app.undo_stack.len(), 1);
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(ui.app.project.active_pattern().notes.len(), 3);
    let from = point(&ui, 2.0, 62);
    let to = point(&ui, 3.5, 65);
    drag(
        &mut ui,
        from,
        to,
        egui::Modifiers::SHIFT,
        egui::Modifiers::SHIFT,
    );
    let added = ui
        .app
        .project
        .active_pattern()
        .notes
        .iter()
        .find(|n| n.id < 90000)
        .unwrap();
    assert_eq!((added.start, added.note, added.length), (2.0, 62, 1.5));
    assert_eq!(ui.app.piano_roll_state.last_note_length, 1.5);
    assert_eq!(ui.app.undo_stack.len(), 1);
    ui.app.piano_roll_state.tool = PianoRollTool::Paint;
    let from = point(&ui, 4.0, 61);
    let middle = point(&ui, 5.0, 61);
    let to = point(&ui, 6.0, 61);
    mouse(&mut ui, from, Some(true), egui::Modifiers::NONE);
    mouse(&mut ui, middle, None, egui::Modifiers::NONE);
    mouse(&mut ui, to, None, egui::Modifiers::NONE);
    mouse(&mut ui, to, Some(false), egui::Modifiers::NONE);
    ui.settle();
    assert_eq!(ui.app.project.active_pattern().notes.len(), 7);
    assert!(
        ui.app
            .project
            .active_pattern()
            .notes
            .iter()
            .any(|n| n.note == 61 && n.start == 4.0)
    );
}

#[test]
fn piano_mouse_shift_clone_groups_is_one_undo_and_after_press_locks_axes() {
    let mut ui = fixture();
    // The allocator must consider other patterns, including the smallest available ID.
    let mut another = ui.app.project.active_pattern().clone();
    another.id += 100;
    another.notes = vec![PianoNote {
        id: 1,
        group_id: Some(1),
        ..another.notes[0].clone()
    }];
    ui.app.project.patterns.push(another);
    ui.app.sync_history_observer();
    ui.app.undo_stack.clear();
    let before = project_fingerprint(&ui.app.project);
    let from = body(&ui, 90001);
    let to = from + Vec2::new(96.0, -20.0);
    drag(
        &mut ui,
        from,
        to,
        egui::Modifiers::SHIFT,
        egui::Modifiers::SHIFT,
    );
    assert_eq!(ui.app.project.active_pattern().notes.len(), 5);
    let originals = &ui.app.project.active_pattern().notes[..3];
    assert_eq!(originals[0].start, 0.375);
    assert_eq!(originals[1].start, 0.75);
    let clones = &ui.app.project.active_pattern().notes[3..];
    assert_eq!(clones[0].group_id, clones[1].group_id);
    assert_ne!(clones[0].group_id, Some(123));
    assert_ne!(clones[0].group_id, Some(1));
    assert!(clones.iter().all(|n| n.id != 1));
    assert_eq!(clones[1].start - clones[0].start, 0.375);
    assert_eq!(clones[1].note - clones[0].note, 4);
    assert!(clones[1].muted);
    assert_eq!(clones[1].velocity, 0.25);
    assert_eq!(ui.app.undo_stack.len(), 1);
    ui.capture("piano-mouse-cloned-phrase");
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), before);
    ui.key(egui::Key::Y, piano_clipboard_command());
    assert_eq!(ui.app.project.active_pattern().notes.len(), 5);
    ui.key(egui::Key::Z, piano_clipboard_command());
    let from = body(&ui, 90001);
    drag(
        &mut ui,
        from,
        from + Vec2::new(80.0, -24.0),
        egui::Modifiers::NONE,
        egui::Modifiers::SHIFT,
    );
    assert_eq!(ui.app.project.active_pattern().notes.len(), 3);
    assert_eq!(note(&ui, 90001).note, 60);
    assert!(note(&ui, 90001).start > 0.375);
    ui.key(egui::Key::Z, piano_clipboard_command());
    let from = body(&ui, 90001);
    drag(
        &mut ui,
        from,
        from + Vec2::new(80.0, -24.0),
        egui::Modifiers::NONE,
        egui::Modifiers::CTRL,
    );
    assert_eq!(note(&ui, 90001).start, 0.375);
    assert!(note(&ui, 90001).note > 60);
}

#[test]
fn piano_mouse_shift_click_threshold_and_note_length_inheritance() {
    let mut ui = fixture();
    let before = project_fingerprint(&ui.app.project);
    let from = body(&ui, 90001);
    mouse(&mut ui, from, Some(true), egui::Modifiers::SHIFT);
    mouse(
        &mut ui,
        from + Vec2::splat(1.0),
        None,
        egui::Modifiers::SHIFT,
    );
    for _ in 0..12 {
        mouse(
            &mut ui,
            from + Vec2::splat(1.0),
            None,
            egui::Modifiers::SHIFT,
        );
    }
    mouse(
        &mut ui,
        from + Vec2::splat(1.0),
        Some(false),
        egui::Modifiers::SHIFT,
    );
    ui.settle();
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert!(ui.app.undo_stack.is_empty());
    assert_eq!(ui.app.piano_roll_state.last_note_length, 0.75);
    let p = point(&ui, 2.0, 62);
    click(&mut ui, p, egui::Modifiers::NONE);
    assert_eq!(
        ui.app.project.active_pattern().notes.last().unwrap().length,
        0.75
    );
    assert_eq!(ui.app.undo_stack.len(), 1);
}

#[test]
fn piano_mouse_alt_bypasses_initial_draw_and_stamp_time() {
    let mut ui = fixture();
    let p = point(&ui, 2.37, 62);
    click(&mut ui, p, egui::Modifiers::ALT);
    let n = ui.app.project.active_pattern().notes.last().unwrap();
    assert!((n.start - 2.37).abs() < 0.001, "{}", n.start);
    ui.app.piano_roll_state.tool = PianoRollTool::Stamp;
    let p = point(&ui, 4.37, 62);
    click(&mut ui, p, egui::Modifiers::ALT);
    assert_eq!(ui.app.project.active_pattern().notes.len(), 7);
    assert!(
        ui.app.project.active_pattern().notes[4..]
            .iter()
            .all(|n| (n.start - 4.37).abs() < 0.001)
    );
}

#[test]
fn piano_mouse_minimum_window_composition_cancel_and_undo_keep_one_transaction() {
    let mut ui = fixture();
    ui.size = Vec2::new(1080.0, 680.0);
    ui.settle();
    ui.app
        .piano_viewport
        .y
        .reveal(piano_pitch_row(62), piano_pitch_row(62) + 1.0, 0.0)
        .unwrap();
    ui.settle();
    let before = project_fingerprint(&ui.app.project);
    let from = point(&ui, 2.0, 62);
    let to = from + Vec2::new(75.0, 0.0);
    drag(
        &mut ui,
        from,
        to,
        egui::Modifiers::SHIFT,
        egui::Modifiers::SHIFT,
    );
    assert_eq!(ui.app.project.active_pattern().notes.len(), 4);
    assert_eq!(ui.app.undo_stack.len(), 1);
    assert!(ui.app.dirty);
    ui.capture("piano-mouse-minimum-drawn-length");
    let drawn = project_fingerprint(&ui.app.project);
    ui.key(egui::Key::N, piano_clipboard_command());
    assert!(ui.app.project_lifecycle.is_modal());
    ui.click("CANCEL");
    assert!(ui.app.project_lifecycle.is_idle());
    assert_eq!(project_fingerprint(&ui.app.project), drawn);
    ui.key(egui::Key::N, piano_clipboard_command());
    ui.key(egui::Key::Escape, egui::Modifiers::NONE);
    assert_eq!(project_fingerprint(&ui.app.project), drawn);
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), before);
    ui.key(egui::Key::Y, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), drawn);
}

#[test]
fn piano_mouse_modals_focus_and_geometry_interrupt_clone_without_resuming() {
    for mode in 0..3 {
        let mut ui = fixture();
        let before = project_fingerprint(&ui.app.project);
        let from = body(&ui, 90001);
        mouse(&mut ui, from, Some(true), egui::Modifiers::SHIFT);
        mouse(
            &mut ui,
            from + Vec2::new(50.0, -20.0),
            None,
            egui::Modifiers::SHIFT,
        );
        assert_eq!(ui.app.project.active_pattern().notes.len(), 5);
        assert!(ui.app.piano_roll_gesture_before.is_some());
        match mode {
            0 => {
                ui.key(egui::Key::F10, egui::Modifiers::NONE);
                ui.key(egui::Key::F10, egui::Modifiers::NONE);
            }
            1 => {
                ui.key(egui::Key::F9, egui::Modifiers::NONE);
                ui.key(egui::Key::F7, egui::Modifiers::NONE);
            }
            _ => {
                ui.size = Vec2::new(1080.0, 680.0);
                ui.run(Vec::new());
            }
        }
        let stopped = project_fingerprint(&ui.app.project);
        assert!(ui.app.piano_roll_gesture_before.is_none());
        mouse(
            &mut ui,
            from + Vec2::new(150.0, -30.0),
            None,
            egui::Modifiers::SHIFT,
        );
        mouse(
            &mut ui,
            from + Vec2::new(150.0, -30.0),
            Some(false),
            egui::Modifiers::SHIFT,
        );
        ui.settle();
        assert_eq!(project_fingerprint(&ui.app.project), stopped, "mode {mode}");
        assert_eq!(ui.app.undo_stack.len(), 1);
        ui.key(egui::Key::Z, piano_clipboard_command());
        assert_eq!(project_fingerprint(&ui.app.project), before);
        // A fresh gesture must work after the canceled one has fully released.
        ui.app
            .piano_viewport
            .y
            .reveal(piano_pitch_row(62), piano_pitch_row(62) + 1.0, 0.0)
            .unwrap();
        ui.settle();
        let p = point(&ui, 2.0, 62);
        click(&mut ui, p, egui::Modifiers::NONE);
        assert_eq!(ui.app.project.active_pattern().notes.len(), 4);
    }
}

#[test]
fn piano_mouse_control_selection_overrides_destructive_tools_until_release() {
    for tool in [
        PianoRollTool::Paint,
        PianoRollTool::Delete,
        PianoRollTool::Mute,
        PianoRollTool::Slice,
        PianoRollTool::Stamp,
    ] {
        let mut ui = fixture();
        ui.app.piano_roll_state.tool = tool;
        let before = project_fingerprint(&ui.app.project);
        let p = body(&ui, 90001);
        mouse(&mut ui, p, Some(true), egui::Modifiers::CTRL);
        mouse(&mut ui, p, Some(false), egui::Modifiers::NONE);
        ui.settle();
        assert_eq!(
            ui.app.piano_roll_state.selection_ids,
            HashSet::from([90001, 90002]),
            "{tool:?}"
        );
        assert_eq!(project_fingerprint(&ui.app.project), before, "{tool:?}");
        let from = point(&ui, 2.0, 62);
        let to = point(&ui, 3.0, 63);
        mouse(&mut ui, from, Some(true), egui::Modifiers::CTRL);
        mouse(&mut ui, to, None, egui::Modifiers::NONE);
        mouse(&mut ui, to, Some(false), egui::Modifiers::NONE);
        ui.settle();
        assert_eq!(project_fingerprint(&ui.app.project), before, "{tool:?}");
        assert!(ui.app.undo_stack.is_empty());
    }
}

#[test]
fn piano_mouse_group_boundaries_preserve_ghost_and_relative_positions() {
    let mut ui = fixture();
    ui.app.piano_roll_state.selection_ids = HashSet::from([90001, 90002, 90003]);
    let ghost = note(&ui, 90003).clone();
    let from = body(&ui, 90001);
    drag(
        &mut ui,
        from,
        from + Vec2::new(-5000.0, 5000.0),
        egui::Modifiers::NONE,
        egui::Modifiers::ALT,
    );
    assert_eq!(note(&ui, 90001).start, 0.0);
    assert_eq!(note(&ui, 90002).start, 0.375);
    assert_eq!(note(&ui, 90001).note, 0);
    assert_eq!(note(&ui, 90002).note, 4);
    assert_eq!(
        serde_json::to_value(note(&ui, 90003)).unwrap(),
        serde_json::to_value(&ghost).unwrap()
    );
    ui.key(egui::Key::Z, piano_clipboard_command());
    let from = body(&ui, 90001);
    drag(
        &mut ui,
        from,
        from + Vec2::new(500000.0, -50000.0),
        egui::Modifiers::NONE,
        egui::Modifiers::ALT,
    );
    assert_eq!(note(&ui, 90002).note, 127);
    assert_eq!(note(&ui, 90001).note, 123);
    assert!(note(&ui, 90001).start + note(&ui, 90001).length <= 4096.0);
    assert_eq!(note(&ui, 90002).start - note(&ui, 90001).start, 0.375);
    assert_eq!(
        serde_json::to_value(note(&ui, 90003)).unwrap(),
        serde_json::to_value(&ghost).unwrap()
    );
}

#[test]
fn piano_mouse_window_chrome_and_covered_background_never_start_notes() {
    let mut ui = fixture();
    let before = project_fingerprint(&ui.app.project);
    let rect = ui.editor_rect(StudioView::PianoRoll);
    let title = rect.left_top() + Vec2::new(100.0, 10.0);
    drag(
        &mut ui,
        title,
        title + Vec2::new(20.0, 15.0),
        egui::Modifiers::SHIFT,
        egui::Modifiers::SHIFT,
    );
    assert_eq!(project_fingerprint(&ui.app.project), before);
    let rect = ui.editor_rect(StudioView::PianoRoll);
    let edge = Pos2::new(rect.left() + 1.0, rect.center().y);
    drag(
        &mut ui,
        edge,
        edge + Vec2::new(20.0, 0.0),
        egui::Modifiers::CTRL,
        egui::Modifiers::CTRL,
    );
    assert_eq!(project_fingerprint(&ui.app.project), before);
    ui.click("Cascade windows");
    ui.key(egui::Key::F5, egui::Modifiers::NONE);
    let grid = ui.ctx.read_response(Id::new("piano-grid")).unwrap().rect;
    let playlist = ui.editor_rect(StudioView::Playlist);
    let covered = grid.intersect(playlist);
    assert!(covered.is_positive());
    let pos = covered.center();
    assert_eq!(
        ui.ctx.layer_id_at(pos).unwrap().id,
        workspace::window_id(StudioView::Playlist)
    );
    let notes = ui.app.project.active_pattern().notes.clone();
    click(&mut ui, pos, egui::Modifiers::CTRL);
    assert_eq!(
        serde_json::to_value(&ui.app.project.active_pattern().notes).unwrap(),
        serde_json::to_value(&notes).unwrap()
    );
}

#[test]
fn piano_mouse_stamp_ids_are_project_wide_and_edge_lengths_stay_in_bounds() {
    let mut ui = fixture();
    let mut other = ui.app.project.active_pattern().clone();
    other.id += 100;
    other.notes = vec![PianoNote {
        id: 90004,
        ..other.notes[0].clone()
    }];
    ui.app.project.patterns.push(other);
    ui.app.sync_history_observer();
    ui.app.piano_roll_state.tool = PianoRollTool::Stamp;
    let p = point(&ui, 3.0, 62);
    click(&mut ui, p, egui::Modifiers::NONE);
    let ids: Vec<_> = ui
        .app
        .project
        .patterns
        .iter()
        .flat_map(|p| &p.notes)
        .map(|n| n.id)
        .collect();
    assert_eq!(ids.len(), ids.iter().collect::<HashSet<_>>().len());
    for tool in [
        PianoRollTool::Draw,
        PianoRollTool::Paint,
        PianoRollTool::Stamp,
    ] {
        ui.app.project.active_pattern_mut().notes[0].start = 4095.0;
        ui.app.project.active_pattern_mut().notes[0].length = 1.0;
        ui.app.piano_roll_state.tool = tool;
        ui.app.piano_roll_state.last_note_length = 4.0;
        ui.settle();
        ui.app.piano_viewport.x.reveal(4095.0, 4096.0, 0.0).unwrap();
        ui.settle();
        let p = point(&ui, 4095.9, 62);
        click(&mut ui, p, egui::Modifiers::ALT);
        assert!(
            ui.app
                .project
                .active_pattern()
                .notes
                .iter()
                .all(|n| n.start + n.length <= 4096.0),
            "{tool:?}"
        );
    }
}

#[test]
fn piano_mouse_resize_remembers_anchor_and_excludes_selected_ghosts() {
    let mut ui = fixture();
    ui.app.piano_roll_state.selection_ids = HashSet::from([90001, 90002, 90003]);
    let ghost = note(&ui, 90003).clone();
    let p = ui
        .ctx
        .read_response(Id::new(("piano-note", 90001_u64)).with("resize"))
        .unwrap()
        .rect
        .center();
    drag(
        &mut ui,
        p,
        p + Vec2::new(30.0, 0.0),
        egui::Modifiers::NONE,
        egui::Modifiers::NONE,
    );
    assert!(note(&ui, 90001).length > 0.75);
    assert_eq!(
        serde_json::to_value(note(&ui, 90003)).unwrap(),
        serde_json::to_value(&ghost).unwrap()
    );
    assert_eq!(
        ui.app.piano_roll_state.last_note_length,
        note(&ui, 90001).length
    );
    assert_eq!(ui.app.undo_stack.len(), 1);
}

#[test]
fn piano_mouse_batched_modifier_changes_obey_pointer_down_event_order() {
    for (press, ongoing, clones) in [
        (egui::Modifiers::NONE, egui::Modifiers::SHIFT, false),
        (egui::Modifiers::SHIFT, egui::Modifiers::NONE, true),
        (egui::Modifiers::NONE, egui::Modifiers::CTRL, false),
    ] {
        let mut ui = fixture();
        let from = body(&ui, 90001);
        // RawInput.modifiers is the frame's final state; pointer event modifiers preserve
        // the initial state even when the key transition lands in the same input batch.
        ui.run_with_modifiers(
            vec![
                egui::Event::PointerMoved(from),
                egui::Event::PointerButton {
                    pos: from,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: press,
                },
            ],
            ongoing,
        );
        mouse(&mut ui, from + Vec2::new(60.0, -24.0), None, ongoing);
        mouse(&mut ui, from + Vec2::new(60.0, -24.0), Some(false), ongoing);
        ui.settle();
        assert_eq!(
            ui.app.project.active_pattern().notes.len(),
            if clones { 5 } else { 3 },
            "{press:?} -> {ongoing:?}"
        );
        if ongoing.shift {
            assert_eq!(note(&ui, 90001).note, 60);
        }
        if ongoing.ctrl {
            assert_eq!(note(&ui, 90001).start, 0.375);
        }
    }
    let mut ui = fixture();
    let p = point(&ui, 2.0, 62);
    ui.run_with_modifiers(
        vec![
            egui::Event::PointerMoved(p),
            egui::Event::PointerButton {
                pos: p,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: p,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ],
        egui::Modifiers::NONE,
    );
    ui.settle();
    assert_eq!(
        ui.app.project.active_pattern().notes.len(),
        4,
        "a rapid down/up batch still draws once"
    );
    assert_eq!(ui.app.undo_stack.len(), 1);
    let from = body(&ui, 90001);
    let to = from + Vec2::new(80.0, 0.0);
    ui.run_with_modifiers(
        vec![
            egui::Event::PointerMoved(from),
            egui::Event::PointerButton {
                pos: from,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::SHIFT,
            },
            egui::Event::PointerMoved(to),
            egui::Event::PointerButton {
                pos: to,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::SHIFT,
            },
        ],
        egui::Modifiers::SHIFT,
    );
    ui.settle();
    assert_eq!(
        ui.app.project.active_pattern().notes.len(),
        6,
        "one batched drag still clones the group"
    );
}

// Exercise production editor input with synthetic retained-project barriers while deliberately
// omitting app worker polling, save completion and native dialogs. Full-app modal tests above
// separately exercise end-to-end interruption.
fn render_piano_only(ui: &mut UiHarness, events: Vec<egui::Event>, modifiers: egui::Modifiers) {
    ui.time += 0.1;
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, ui.size)),
        time: Some(ui.time),
        events,
        modifiers,
        ..Default::default()
    };
    let _ = ui.ctx.run_ui(input, |root| ui.app.piano_roll(root));
}
fn editor_mouse(ui: &mut UiHarness, p: Pos2, pressed: Option<bool>) {
    let mut events = vec![egui::Event::PointerMoved(p)];
    if let Some(pressed) = pressed {
        events.push(egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
    }
    render_piano_only(ui, events, egui::Modifiers::NONE);
}
fn set_snapshot_barrier(ui: &mut UiHarness, kind: usize, enabled: bool) {
    if kind == 0 {
        ui.app.queued_save_request = enabled.then(|| ProjectSaveRequest::Manual {
            path: PathBuf::from("unused-piano-pointer-test.citrus"),
            lifecycle: None,
        });
    } else {
        ui.app.deferred_generator_candidate_after_midi =
            enabled.then(|| DeferredMidiGeneratorCandidate {
                channel_id: ui.app.project.channels[ui.app.selected_channel].id,
                candidate: ui.app.project.clone(),
                previous_state: None,
            });
    }
}

#[test]
fn piano_mouse_synthetic_snapshot_barriers_block_press_and_stop_held_preview() {
    for barrier in 0..2 {
        let mut ui = fixture();
        ui.ctx = egui::Context::default();
        ui.app.workspace.maximized = true;
        for _ in 0..3 {
            render_piano_only(&mut ui, vec![], egui::Modifiers::NONE);
        }
        let before = project_fingerprint(&ui.app.project);
        let p = point(&ui, 2.0, 62);
        set_snapshot_barrier(&mut ui, barrier, true);
        editor_mouse(&mut ui, p, Some(true));
        editor_mouse(&mut ui, p + Vec2::new(50.0, 0.0), None);
        editor_mouse(&mut ui, p + Vec2::new(50.0, 0.0), Some(false));
        assert_eq!(project_fingerprint(&ui.app.project), before);
        assert!(ui.app.undo_stack.is_empty());
        set_snapshot_barrier(&mut ui, barrier, false);
        for _ in 0..2 {
            render_piano_only(&mut ui, vec![], egui::Modifiers::NONE);
        }
        editor_mouse(&mut ui, p, Some(true));
        assert!(ui.app.piano_roll_gesture_before.is_some());
        let preview = project_fingerprint(&ui.app.project);
        set_snapshot_barrier(&mut ui, barrier, true);
        render_piano_only(&mut ui, vec![], egui::Modifiers::NONE);
        set_snapshot_barrier(&mut ui, barrier, false);
        editor_mouse(&mut ui, p + Vec2::new(100.0, 0.0), None);
        editor_mouse(&mut ui, p + Vec2::new(100.0, 0.0), Some(false));
        assert_eq!(project_fingerprint(&ui.app.project), preview);
        assert!(ui.app.piano_roll_gesture_before.is_none());
        assert_eq!(ui.app.undo_stack.len(), 1);
    }
}

#[test]
fn piano_mouse_held_undo_redo_wait_for_release_and_keep_prior_edit_separate() {
    let mut ui = fixture();
    let original = project_fingerprint(&ui.app.project);
    let first = point(&ui, 2.0, 62);
    click(&mut ui, first, egui::Modifiers::NONE);
    let after_first = project_fingerprint(&ui.app.project);
    assert_eq!(ui.app.undo_stack.len(), 1);
    let second = point(&ui, 4.0, 63);
    mouse(&mut ui, second, Some(true), egui::Modifiers::NONE);
    let preview = project_fingerprint(&ui.app.project);
    assert!(ui.app.piano_roll_gesture_before.is_some());
    for key in [egui::Key::Z, egui::Key::Y] {
        ui.key(key, piano_clipboard_command());
        assert_eq!(project_fingerprint(&ui.app.project), preview);
        assert_eq!(ui.app.undo_stack.len(), 1);
    }
    mouse(&mut ui, second, Some(false), egui::Modifiers::NONE);
    ui.settle();
    assert_eq!(ui.app.undo_stack.len(), 2);
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), after_first);
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), original);
    ui.key(egui::Key::Y, piano_clipboard_command());
    ui.key(egui::Key::Y, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), preview);
    // An existing redo branch must also remain untouched during a held fresh gesture.
    ui.key(egui::Key::Z, piano_clipboard_command());
    let second = point(&ui, 5.0, 63);
    mouse(&mut ui, second, Some(true), egui::Modifiers::NONE);
    let held = project_fingerprint(&ui.app.project);
    ui.key(egui::Key::Y, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), held);
    assert_eq!(ui.app.redo_stack.len(), 1);
    mouse(&mut ui, second, Some(false), egui::Modifiers::NONE);
    ui.settle();
    assert!(ui.app.redo_stack.is_empty());
}

#[test]
fn piano_mouse_window_focus_loss_ends_gesture_before_any_refocused_motion() {
    let mut ui = fixture();
    let original = project_fingerprint(&ui.app.project);
    let p = body(&ui, 90001);
    mouse(&mut ui, p, Some(true), egui::Modifiers::SHIFT);
    mouse(
        &mut ui,
        p + Vec2::new(60.0, 0.0),
        None,
        egui::Modifiers::SHIFT,
    );
    let preview = project_fingerprint(&ui.app.project);
    assert_eq!(ui.app.project.active_pattern().notes.len(), 5);
    ui.time += 0.1;
    let _ = ui.ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, ui.size)),
            time: Some(ui.time),
            focused: false,
            events: vec![egui::Event::WindowFocused(false)],
            ..Default::default()
        },
        |root| ui.app.ui(root, &mut ui.frame),
    );
    assert!(ui.app.piano_roll_gesture_before.is_none());
    ui.run(vec![egui::Event::WindowFocused(true)]);
    mouse(
        &mut ui,
        p + Vec2::new(130.0, -24.0),
        None,
        egui::Modifiers::SHIFT,
    );
    mouse(
        &mut ui,
        p + Vec2::new(130.0, -24.0),
        Some(false),
        egui::Modifiers::SHIFT,
    );
    ui.settle();
    assert_eq!(project_fingerprint(&ui.app.project), preview);
    assert_eq!(ui.app.undo_stack.len(), 1);
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), original);
}

#[test]
fn piano_mouse_released_edit_keeps_normal_undo_redo_menu_clicks_available() {
    let mut ui = fixture();
    let before = project_fingerprint(&ui.app.project);
    let p = point(&ui, 2.0, 62);
    click(&mut ui, p, egui::Modifiers::NONE);
    let after = project_fingerprint(&ui.app.project);
    assert!(!ui.app.editor_pointer_gesture_active());
    ui.click("EDIT");
    ui.click("Undo              Ctrl+Z");
    assert_eq!(project_fingerprint(&ui.app.project), before);
    ui.click("EDIT");
    ui.click("Redo              Ctrl+Y");
    assert_eq!(project_fingerprint(&ui.app.project), after);
}

#[test]
fn piano_mouse_selection_only_focus_loss_cannot_resume_as_paint() {
    let mut ui = fixture();
    ui.app.piano_roll_state.tool = PianoRollTool::Paint;
    let before = project_fingerprint(&ui.app.project);
    let p = point(&ui, 2.0, 62);
    mouse(&mut ui, p, Some(true), egui::Modifiers::CTRL);
    assert!(piano_mouse::active(&ui.ctx));
    assert!(ui.app.piano_roll_gesture_before.is_none());
    ui.time += 0.1;
    let _ = ui.ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, ui.size)),
            time: Some(ui.time),
            focused: false,
            events: vec![egui::Event::WindowFocused(false)],
            ..Default::default()
        },
        |root| ui.app.ui(root, &mut ui.frame),
    );
    ui.run(vec![egui::Event::WindowFocused(true)]);
    mouse(
        &mut ui,
        p + Vec2::new(80.0, 0.0),
        None,
        egui::Modifiers::NONE,
    );
    mouse(
        &mut ui,
        p + Vec2::new(80.0, 0.0),
        Some(false),
        egui::Modifiers::NONE,
    );
    ui.settle();
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert!(!ui.app.editor_pointer_gesture_active());
    assert!(ui.app.undo_stack.is_empty());
}

#[test]
fn piano_mouse_legacy_overhorizon_group_resize_never_inverts_shorter_notes() {
    let mut ui = fixture();
    let notes = &mut ui.app.project.active_pattern_mut().notes;
    notes[0].start = 0.0;
    notes[0].length = 0.25;
    notes[1].start = 4096.0;
    notes[1].length = 1.0;
    ui.app.sync_history_observer();
    ui.settle();
    let before = project_fingerprint(&ui.app.project);
    let p = ui
        .ctx
        .read_response(Id::new(("piano-note", 90001_u64)).with("resize"))
        .unwrap()
        .rect
        .center();
    drag(
        &mut ui,
        p,
        p + Vec2::new(40.0, 0.0),
        egui::Modifiers::NONE,
        egui::Modifiers::NONE,
    );
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert!(
        ui.app
            .project
            .active_pattern()
            .notes
            .iter()
            .all(|n| n.length > 0.0)
    );
    assert!(ui.app.undo_stack.is_empty());
}

#[test]
fn piano_mouse_batched_paint_includes_mouse_down_cell_and_covered_press_stays_safe() {
    let mut ui = fixture();
    ui.app.piano_roll_state.tool = PianoRollTool::Paint;
    let from = point(&ui, 4.0, 62);
    let to = point(&ui, 5.0, 62);
    ui.run_with_modifiers(
        vec![
            egui::Event::PointerMoved(from),
            egui::Event::PointerButton {
                pos: from,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerMoved(to),
        ],
        egui::Modifiers::NONE,
    );
    mouse(&mut ui, to, Some(false), egui::Modifiers::NONE);
    ui.settle();
    for start in [4.0, 5.0] {
        assert!(
            ui.app
                .project
                .active_pattern()
                .notes
                .iter()
                .any(|n| n.start == start && n.note == 62)
        );
    }
    assert_eq!(ui.app.undo_stack.len(), 1);
    ui.click("Cascade windows");
    ui.key(egui::Key::F5, egui::Modifiers::NONE);
    let covered = ui
        .ctx
        .read_response(Id::new("piano-grid"))
        .unwrap()
        .rect
        .intersect(ui.editor_rect(StudioView::Playlist));
    assert!(covered.is_positive());
    let from = covered.center();
    let to = from + Vec2::new(15.0, 0.0);
    let notes = serde_json::to_value(&ui.app.project.active_pattern().notes).unwrap();
    ui.run_with_modifiers(
        vec![
            egui::Event::PointerMoved(from),
            egui::Event::PointerButton {
                pos: from,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::SHIFT,
            },
            egui::Event::PointerMoved(to),
            egui::Event::PointerButton {
                pos: to,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::SHIFT,
            },
        ],
        egui::Modifiers::SHIFT,
    );
    ui.settle();
    assert_eq!(
        serde_json::to_value(&ui.app.project.active_pattern().notes).unwrap(),
        notes
    );
}

#[test]
fn piano_mouse_batched_paint_note_and_resize_own_the_press_over_the_grid() {
    for resize in [false, true] {
        let mut ui = fixture();
        ui.app.piano_roll_state.tool = PianoRollTool::Paint;
        let from = if resize {
            ui.ctx
                .read_response(Id::new(("piano-note", 90001_u64)).with("resize"))
                .unwrap()
                .rect
                .center()
        } else {
            body(&ui, 90001)
        };
        let to = from + Vec2::new(40.0, 0.0);
        ui.run_with_modifiers(
            vec![
                egui::Event::PointerMoved(from),
                egui::Event::PointerButton {
                    pos: from,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerMoved(to),
                egui::Event::PointerButton {
                    pos: to,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            egui::Modifiers::NONE,
        );
        ui.settle();
        assert_eq!(
            ui.app.project.active_pattern().notes.len(),
            3,
            "resize {resize}"
        );
        if resize {
            assert!(note(&ui, 90001).length > 0.75);
        } else {
            assert!(note(&ui, 90001).start > 0.375);
        }
        assert_eq!(ui.app.undo_stack.len(), 1, "resize {resize}");
    }
}

#[test]
fn piano_mouse_batched_overlap_uses_topmost_note_not_an_underlying_resize_grip() {
    let mut ui = fixture();
    ui.app.piano_roll_state.grouping_enabled = false;
    let notes = &mut ui.app.project.active_pattern_mut().notes;
    notes[0].start = 0.0;
    notes[0].length = 2.0;
    notes[1].start = 1.8;
    notes[1].length = 1.0;
    notes[1].note = 60;
    ui.app.sync_history_observer();
    ui.settle();
    let from = point(&ui, 1.95, 60);
    let to = from + Vec2::new(60.0, 0.0);
    ui.run_with_modifiers(
        vec![
            egui::Event::PointerMoved(from),
            egui::Event::PointerButton {
                pos: from,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerMoved(to),
            egui::Event::PointerButton {
                pos: to,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ],
        egui::Modifiers::NONE,
    );
    ui.settle();
    assert_eq!(
        (note(&ui, 90001).start, note(&ui, 90001).length),
        (0.0, 2.0)
    );
    assert!(note(&ui, 90002).start > 1.8);
    assert_eq!(ui.app.project.active_pattern().notes.len(), 3);
    assert_eq!(ui.app.undo_stack.len(), 1);
}
