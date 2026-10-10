use super::*;

fn fixture() -> UiHarness {
    let mut ui = UiHarness::floating();
    piano_clipboard_fixture(&mut ui);
    ui.app.piano_roll_state.tool = PianoRollTool::Draw;
    ui
}
fn body(ui: &UiHarness, id: u64) -> Pos2 {
    ui.ctx
        .read_response(Id::new(("piano-note", id)))
        .unwrap()
        .rect
        .center()
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
fn pointer(ui: &mut UiHarness, pos: Pos2, pressed: bool) {
    ui.run(mixer_pointer_button(pos, pressed));
}
fn double_click(ui: &mut UiHarness, pos: Pos2) {
    for _ in 0..2 {
        pointer(ui, pos, true);
        pointer(ui, pos, false);
    }
    ui.settle();
}
fn wheel_event(value: f32, fine: bool, unit: egui::MouseWheelUnit) -> egui::Event {
    egui::Event::MouseWheel {
        unit,
        phase: egui::TouchPhase::Move,
        delta: Vec2::new(0.0, value),
        modifiers: egui::Modifiers {
            alt: true,
            ctrl: fine,
            ..egui::Modifiers::NONE
        },
    }
}
fn wheel(ui: &mut UiHarness, pos: Pos2, value: f32, fine: bool) {
    ui.run_with_modifiers(
        vec![
            egui::Event::PointerMoved(pos),
            wheel_event(value, fine, egui::MouseWheelUnit::Line),
        ],
        egui::Modifiers {
            alt: true,
            ctrl: fine,
            ..egui::Modifiers::NONE
        },
    );
}
fn field(ui: &UiHarness, name: &'static str) -> egui::Response {
    let id = ui
        .ctx
        .data(|d| d.get_temp::<Id>(Id::new(("note-properties-control", name))))
        .expect("real properties control ID");
    ui.ctx.read_response(id).expect("live properties widget")
}
fn set_number(ui: &mut UiHarness, name: &'static str, text: &str) {
    let pos = field(ui, name).rect.center();
    double_click(ui, pos);
    assert!(
        ui.ctx.text_edit_focused(),
        "{name} must have real numeric text focus"
    );
    ui.key(egui::Key::A, piano_clipboard_command());
    ui.run(vec![egui::Event::Text(text.into())]);
    ui.key(egui::Key::Enter, egui::Modifiers::NONE);
    ui.settle();
    assert!(
        ui.app.piano_note_properties.is_some(),
        "numeric Enter must not also apply the dialog"
    );
}
fn close_enough(a: f32, b: f32) {
    assert!((a - b).abs() < 0.000_01, "{a} != {b}");
}

#[test]
fn piano_expression_raw_wheel_relative_dynamics_fine_and_individual_event_undo() {
    let mut ui = fixture();
    ui.app.piano_roll_state.selection_ids = HashSet::from([90001, 90002, 90003]);
    let before = project_fingerprint(&ui.app.project);
    let ghost = serde_json::to_value(note(&ui, 90003)).unwrap();
    let pos = body(&ui, 90001);
    let viewport = (ui.app.piano_viewport.x, ui.app.piano_viewport.y);
    wheel(&mut ui, pos, 1.0, false);
    close_enough(note(&ui, 90001).velocity, 0.675);
    close_enough(note(&ui, 90002).velocity, 0.3);
    assert_eq!(serde_json::to_value(note(&ui, 90003)).unwrap(), ghost);
    assert_eq!(ui.app.undo_stack.len(), 1);
    for _ in 0..12 {
        ui.run_with_modifiers(Vec::new(), egui::Modifiers::ALT);
    }
    assert_eq!(
        ui.app.undo_stack.len(),
        1,
        "smooth residue is never another edit"
    );
    wheel(&mut ui, pos, 1.0, true);
    close_enough(note(&ui, 90001).velocity, 0.685);
    close_enough(note(&ui, 90002).velocity, 0.31);
    assert_eq!(ui.app.undo_stack.len(), 2);
    ui.settle();
    assert_eq!((ui.app.piano_viewport.x, ui.app.piano_viewport.y), viewport);
    ui.key(egui::Key::Z, piano_clipboard_command());
    close_enough(note(&ui, 90001).velocity, 0.675);
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), before);
    ui.key(egui::Key::Y, piano_clipboard_command());
    close_enough(note(&ui, 90002).velocity, 0.3);
}

#[test]
fn piano_expression_wheel_boundaries_ghosts_units_batch_and_unselected_scope() {
    let mut ui = fixture();
    let pos = body(&ui, 90001);
    // Hovering an unselected note expands its active group, never unrelated selections.
    ui.app.piano_roll_state.selection_ids = HashSet::from([90003]);
    wheel(&mut ui, pos, 1e30, false);
    close_enough(note(&ui, 90001).velocity, 1.0);
    close_enough(note(&ui, 90002).velocity, 0.625);
    let history = ui.app.undo_stack.len();
    wheel(&mut ui, pos, 1.0, false);
    assert_eq!(
        ui.app.undo_stack.len(),
        history,
        "a saturated selection is a no-op"
    );
    wheel(&mut ui, pos, -1e30, false);
    wheel(&mut ui, pos, -1e30, false);
    close_enough(note(&ui, 90001).velocity, 0.375);
    close_enough(note(&ui, 90002).velocity, 0.0);
    let before = project_fingerprint(&ui.app.project);
    // Ghosts do not register a note widget, but remain visually present.
    let grid = ui.ctx.read_response(Id::new("piano-grid")).unwrap().rect;
    let ghost = note(&ui, 90003);
    let pos_ghost = piano_mouse::note_rects(grid, ui.app.piano_viewport, ghost)
        .1
        .center();
    wheel(&mut ui, pos_ghost, 1.0, false);
    assert_eq!(project_fingerprint(&ui.app.project), before);
    ui.app.piano_roll_state.grouping_enabled = false;
    let history = ui.app.undo_stack.len();
    ui.run_with_modifiers(
        vec![
            egui::Event::PointerMoved(pos),
            wheel_event(40.0, false, egui::MouseWheelUnit::Point),
            wheel_event(1.0, true, egui::MouseWheelUnit::Page),
        ],
        egui::Modifiers::ALT,
    );
    close_enough(note(&ui, 90001).velocity, 0.435);
    close_enough(note(&ui, 90002).velocity, 0.0);
    assert_eq!(
        ui.app.undo_stack.len(),
        history + 2,
        "one transaction per actual event"
    );
    let after = project_fingerprint(&ui.app.project);
    wheel(&mut ui, pos, f32::NAN, false);
    wheel(&mut ui, pos, f32::INFINITY, false);
    assert_eq!(project_fingerprint(&ui.app.project), after);
}

#[test]
fn piano_expression_double_click_draft_cancel_reset_and_one_apply_transaction() {
    let mut ui = fixture();
    ui.app.piano_roll_state.grouping_enabled = false;
    let before = project_fingerprint(&ui.app.project);
    let pos = body(&ui, 90001);
    double_click(&mut ui, pos);
    assert!(
        ui.app.piano_note_properties.is_some(),
        "double click opens properties"
    );
    assert_eq!(ui.app.undo_stack.len(), 0);
    set_number(&mut ui, "Pitch (MIDI)", "62");
    set_number(&mut ui, "Start (beats)", "1.125");
    set_number(&mut ui, "Length (beats)", "0.5");
    assert_eq!(
        project_fingerprint(&ui.app.project),
        before,
        "draft fields are not Project state"
    );
    ui.click("Reset");
    ui.click("Apply");
    assert!(ui.app.piano_note_properties.is_none());
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert!(ui.app.undo_stack.is_empty());
    double_click(&mut ui, pos);
    set_number(&mut ui, "Pitch (MIDI)", "62");
    ui.click("Cancel");
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert!(ui.app.undo_stack.is_empty());
    double_click(&mut ui, pos);
    set_number(&mut ui, "Pitch (MIDI)", "62");
    set_number(&mut ui, "Start (beats)", "1.125");
    set_number(&mut ui, "Length (beats)", "0.5");
    ui.capture("piano-properties-single-draft");
    ui.click("Apply");
    assert_eq!(note(&ui, 90001).note, 62);
    assert_eq!(
        (note(&ui, 90001).start, note(&ui, 90001).length),
        (1.125, 0.5)
    );
    assert_eq!(ui.app.undo_stack.len(), 1);
    let after = project_fingerprint(&ui.app.project);
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), before);
    ui.key(egui::Key::Y, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), after);
}

#[test]
fn piano_expression_properties_group_relative_mixed_preserved_and_modal_keys() {
    let mut ui = fixture();
    let before = project_fingerprint(&ui.app.project);
    let pos = body(&ui, 90001);
    double_click(&mut ui, pos);
    assert!(ui.app.piano_note_properties.is_some());
    set_number(&mut ui, "Transpose (semitones)", "5");
    // Real slider click adds a common positive velocity offset.
    let slider = field(&ui, "Velocity change").rect;
    let p = Pos2::new(slider.left() + slider.width() * 0.65, slider.center().y);
    pointer(&mut ui, p, true);
    pointer(&mut ui, p, false);
    ui.settle();
    for (key, mods) in [
        (egui::Key::Space, egui::Modifiers::NONE),
        (egui::Key::B, piano_clipboard_command()),
        (egui::Key::Delete, egui::Modifiers::NONE),
        (egui::Key::F5, egui::Modifiers::NONE),
    ] {
        ui.key(key, mods);
    }
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert!(!ui.app.playing);
    ui.capture("piano-properties-group-relative");
    ui.click("Apply");
    assert_eq!((note(&ui, 90001).note, note(&ui, 90002).note), (65, 69));
    close_enough(note(&ui, 90001).velocity - note(&ui, 90002).velocity, 0.375);
    assert!(note(&ui, 90001).velocity > 0.625);
    assert!(!note(&ui, 90001).muted && note(&ui, 90002).muted);
    assert_eq!(note(&ui, 90001).start, 0.375);
    assert_eq!(note(&ui, 90002).length, 0.25);
    assert_eq!(note(&ui, 90003).note, 67);
    assert_eq!(ui.app.undo_stack.len(), 1);
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), before);
}

#[test]
fn piano_expression_wheel_numeric_focus_gesture_modal_and_save_barriers() {
    let mut ui = fixture();
    let pos = body(&ui, 90001);
    let before = project_fingerprint(&ui.app.project);
    let bounds = ui
        .nodes
        .iter()
        .find(|n| n.role() == Role::SpinButton && n.bounds().is_some_and(|b| b.y1 < 100.0))
        .unwrap()
        .bounds()
        .unwrap();
    double_click(
        &mut ui,
        Pos2::new(
            ((bounds.x0 + bounds.x1) / 2.0) as f32,
            ((bounds.y0 + bounds.y1) / 2.0) as f32,
        ),
    );
    assert!(ui.ctx.text_edit_focused());
    wheel(&mut ui, pos, 1.0, false);
    assert_eq!(project_fingerprint(&ui.app.project), before);
    ui.key(egui::Key::Enter, egui::Modifiers::NONE);
    ui.app.show_settings = true;
    ui.settle();
    wheel(&mut ui, pos, 1.0, false);
    assert_eq!(project_fingerprint(&ui.app.project), before);
    ui.app.show_settings = false;
    ui.settle();
    pointer(&mut ui, pos, true);
    wheel(&mut ui, pos, 1.0, false);
    pointer(&mut ui, pos, false);
    ui.settle();
    assert_eq!(project_fingerprint(&ui.app.project), before);
    ui.app.queued_save_request = Some(ProjectSaveRequest::Autosave {
        path: PathBuf::from("/tmp/not-written-expression.citrus"),
        announce: false,
    });
    // Direct production UI with the queued snapshot barrier; don't run worker polling.
    let ctx = ui.ctx.clone();
    let _ = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, ui.size)),
            events: vec![
                egui::Event::PointerMoved(pos),
                wheel_event(1.0, false, egui::MouseWheelUnit::Line),
            ],
            modifiers: egui::Modifiers::ALT,
            ..Default::default()
        },
        |root| ui.app.piano_roll(root),
    );
    assert_eq!(project_fingerprint(&ui.app.project), before);
    ui.app.queued_save_request = None;
}

#[test]
fn piano_expression_properties_stale_notes_and_changed_context_never_overwrite() {
    let mut ui = fixture();
    ui.app.piano_roll_state.grouping_enabled = false;
    let pos = body(&ui, 90001);
    double_click(&mut ui, pos);
    set_number(&mut ui, "Pitch (MIDI)", "70");
    ui.app.project.active_pattern_mut().notes[0].velocity = 0.9;
    let newer = project_fingerprint(&ui.app.project);
    ui.click("Apply");
    assert_eq!(project_fingerprint(&ui.app.project), newer);
    assert_eq!(note(&ui, 90001).note, 60);
    assert!(ui.app.piano_note_properties.is_some());
    ui.click("Cancel");
    double_click(&mut ui, pos);
    set_number(&mut ui, "Pitch (MIDI)", "70");
    ui.app.project.name = "Independent newer project metadata".into();
    ui.click("Cancel");
    assert_eq!(ui.app.project.name, "Independent newer project metadata");
    assert_eq!(note(&ui, 90001).velocity, 0.9);
    double_click(&mut ui, pos);
    ui.app.selected_channel = (ui.app.selected_channel + 1) % ui.app.project.channels.len();
    ui.settle();
    assert!(ui.app.piano_note_properties.is_none());
    assert_eq!(note(&ui, 90001).note, 60);
}

#[test]
fn piano_expression_properties_minimum_floating_and_inspector_same_editor() {
    let mut ui = fixture();
    ui.size = Vec2::new(1080.0, 680.0);
    ui.settle();
    ui.app.workspace.maximized = true;
    ui.app.show_inspector = true;
    ui.app.piano_roll_state.selection_ids = HashSet::from([90001]);
    ui.settle();
    ui.app.piano_viewport.y.reveal(63.0, 68.0, 0.0).unwrap();
    ui.settle();
    // The minimum layout hides the Inspector; double-click still reaches the full editor.
    let pos = body(&ui, 90001);
    double_click(&mut ui, pos);
    for label in ["Apply", "Cancel", "Reset"] {
        let b = ui.button(label).bounds().unwrap();
        assert!(
            b.x0 >= 0.0
                && b.y0 >= 0.0
                && b.x1 <= f64::from(ui.size.x)
                && b.y1 <= f64::from(ui.size.y)
        );
    }
    ui.capture("piano-properties-minimum");
    ui.key(egui::Key::Escape, egui::Modifiers::NONE);
    assert!(ui.app.piano_note_properties.is_none());
    ui.size = Vec2::new(1920.0, 1080.0);
    ui.settle();
    ui.click("Edit note properties");
    assert!(ui.app.piano_note_properties.is_some());
    ui.click("Cancel");
    assert!(ui.app.undo_stack.is_empty());
}

#[test]
fn piano_expression_wheel_focus_visibility_selection_change_and_phase_ownership() {
    let mut ui = fixture();
    let pos = body(&ui, 90001);
    wheel(&mut ui, pos, 1.0, false);
    let after = project_fingerprint(&ui.app.project);
    let history = ui.app.undo_stack.len();
    ui.app.piano_roll_state.selection_ids = HashSet::from([90003]);
    ui.run_with_modifiers(
        vec![
            egui::Event::WindowFocused(false),
            egui::Event::PointerMoved(pos),
            wheel_event(1.0, false, egui::MouseWheelUnit::Line),
        ],
        egui::Modifiers::ALT,
    );
    assert_eq!(project_fingerprint(&ui.app.project), after);
    ui.run(vec![egui::Event::WindowFocused(true)]);
    for _ in 0..5 {
        ui.run_with_modifiers(Vec::new(), egui::Modifiers::ALT);
    }
    assert_eq!(project_fingerprint(&ui.app.project), after);
    for phase in [
        egui::TouchPhase::Start,
        egui::TouchPhase::End,
        egui::TouchPhase::Cancel,
    ] {
        let mut event = wheel_event(1.0, false, egui::MouseWheelUnit::Line);
        if let egui::Event::MouseWheel { phase: p, .. } = &mut event {
            *p = phase;
        }
        ui.run_with_modifiers(
            vec![egui::Event::PointerMoved(pos), event],
            egui::Modifiers::ALT,
        );
    }
    assert_eq!(project_fingerprint(&ui.app.project), after);
    assert_eq!(ui.app.undo_stack.len(), history);
    ui.key(egui::Key::F9, egui::Modifiers::NONE);
    wheel(&mut ui, pos, 1.0, false);
    assert_eq!(project_fingerprint(&ui.app.project), after);
    ui.app.workspace.windows[workspace::index(StudioView::PianoRoll)].visible = false;
    ui.settle();
    wheel(&mut ui, pos, 1.0, false);
    assert_eq!(project_fingerprint(&ui.app.project), after);
    ui.key(egui::Key::F7, egui::Modifiers::NONE);
    ui.app.piano_roll_state.grouping_enabled = false;
    ui.app.piano_roll_state.selection_ids = HashSet::from([90002]);
    ui.settle();
    let second = body(&ui, 90002);
    wheel(&mut ui, second, 1.0, false);
    close_enough(note(&ui, 90001).velocity, 0.675);
    close_enough(note(&ui, 90002).velocity, 0.35);
    assert_eq!(ui.app.undo_stack.len(), history + 1);
}

#[test]
fn piano_expression_properties_mute_channel_group_normalization_and_save_reopen() {
    let mut ui = fixture();
    ui.app.piano_roll_state.grouping_enabled = false;
    let before = project_fingerprint(&ui.app.project);
    let pos = body(&ui, 90001);
    wheel(&mut ui, pos, 1.0, false);
    let committed = project_fingerprint(&ui.app.project);
    double_click(&mut ui, pos);
    set_number(&mut ui, "Pitch (MIDI)", "63");
    let path = std::env::temp_dir().join(format!(
        "citrus-expression-draft-{}.citrus",
        std::process::id()
    ));
    ui.app.project.save(&path).unwrap();
    let restored = Project::load(&path).unwrap();
    assert_eq!(
        project_fingerprint(&restored),
        committed,
        "saving while a draft is visible saves only committed notes"
    );
    let mute = field(&ui, "Mute").rect.center();
    pointer(&mut ui, mute, true);
    pointer(&mut ui, mute, false);
    ui.settle();
    ui.click("Muted");
    let destination = ui
        .app
        .project
        .channels
        .iter()
        .find(|c| c.id != note(&ui, 90001).channel_id.unwrap())
        .unwrap()
        .clone();
    let channel = field(&ui, "Channel").rect.center();
    pointer(&mut ui, channel, true);
    pointer(&mut ui, channel, false);
    ui.settle();
    let b = ui
        .nodes
        .iter()
        .find(|n| {
            n.label() == Some(destination.name.as_str())
                && !n.is_disabled()
                && n.role() == Role::Button
        })
        .unwrap()
        .bounds()
        .unwrap();
    let p = Pos2::new(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32);
    pointer(&mut ui, p, true);
    pointer(&mut ui, p, false);
    ui.settle();
    ui.click("Apply");
    assert!(note(&ui, 90001).muted);
    assert_eq!(note(&ui, 90001).channel_id, Some(destination.id));
    assert_eq!(note(&ui, 90001).group_id, None);
    assert_eq!(note(&ui, 90002).group_id, None);
    assert_eq!(ui.app.undo_stack.len(), 2);
    let after = project_fingerprint(&ui.app.project);
    ui.app.project.save(&path).unwrap();
    assert_eq!(project_fingerprint(&Project::load(&path).unwrap()), after);
    std::fs::remove_file(path).unwrap();
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), committed);
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), before);
}

#[test]
fn piano_expression_properties_draw_paint_repeated_clicks_do_not_duplicate_or_history() {
    for tool in [
        PianoRollTool::Draw,
        PianoRollTool::Paint,
        PianoRollTool::Select,
        PianoRollTool::Stamp,
    ] {
        let mut ui = fixture();
        ui.app.piano_roll_state.tool = tool;
        ui.settle();
        let before = project_fingerprint(&ui.app.project);
        let pos = body(&ui, 90001);
        for _ in 0..2 {
            double_click(&mut ui, pos);
            assert!(
                ui.app.piano_note_properties.is_some(),
                "{tool:?} actual {:?}, note count {}, gesture {}, active {}, selected {:?}, point {:?}, rect {:?}",
                ui.app.piano_roll_state.tool,
                ui.app.project.active_pattern().notes.len(),
                ui.app.piano_roll_gesture_before.is_some(),
                ui.app.editor_pointer_gesture_active(),
                ui.app.piano_roll_state.selection_ids,
                pos,
                ui.ctx
                    .read_response(Id::new(("piano-note", 90001_u64)))
                    .map(|r| r.rect)
            );
            ui.key(egui::Key::Escape, egui::Modifiers::NONE);
            assert!(ui.app.piano_note_properties.is_none());
            assert_eq!(project_fingerprint(&ui.app.project), before);
            assert!(ui.app.undo_stack.is_empty());
        }
        for _ in 0..2 {
            for pressed in [true, false] {
                ui.run_with_modifiers(
                    vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::SHIFT,
                        },
                    ],
                    egui::Modifiers::SHIFT,
                );
            }
        }
        ui.settle();
        assert!(
            ui.app.piano_note_properties.is_none(),
            "Shift clone ownership survives double clicks"
        );
        assert_eq!(project_fingerprint(&ui.app.project), before);
    }
}

#[test]
fn piano_expression_properties_invalid_end_stays_open_and_escape_numeric_is_local() {
    let mut ui = fixture();
    ui.app.piano_roll_state.grouping_enabled = false;
    let before = project_fingerprint(&ui.app.project);
    let pos = body(&ui, 90001);
    double_click(&mut ui, pos);
    set_number(&mut ui, "Start (beats)", "4095.9");
    ui.click("Apply");
    assert!(
        ui.app.piano_note_properties.is_some(),
        "invalid draft must remain repairable"
    );
    assert_eq!(project_fingerprint(&ui.app.project), before);
    let value = field(&ui, "Start (beats)").rect.center();
    double_click(&mut ui, value);
    ui.key(egui::Key::Escape, egui::Modifiers::NONE);
    assert!(
        ui.app.piano_note_properties.is_some(),
        "Escape exits numeric text before closing the dialog"
    );
    ui.click("Reset");
    ui.key(egui::Key::Escape, egui::Modifiers::NONE);
    assert!(ui.app.piano_note_properties.is_none());
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert!(ui.app.undo_stack.is_empty());
}

#[test]
fn piano_expression_wheel_tail_leaving_grid_does_not_pan_keyboard_or_replay() {
    let mut ui = fixture();
    let pos = body(&ui, 90001);
    let grid = ui.ctx.read_response(Id::new("piano-grid")).unwrap().rect;
    let viewport = ui.app.piano_viewport;
    wheel(&mut ui, pos, 1.0, false);
    let edited = project_fingerprint(&ui.app.project);
    ui.run(vec![egui::Event::PointerMoved(Pos2::new(
        grid.left() - 20.0,
        pos.y,
    ))]);
    for _ in 0..10 {
        ui.run(Vec::new());
    }
    assert_eq!(
        ui.app.piano_viewport, viewport,
        "owned wheel residue must not pan the keyboard gutter"
    );
    assert_eq!(project_fingerprint(&ui.app.project), edited);
    // A fresh reverse wheel immediately after a larger velocity event must use
    // its own direction, not egui's still-positive, privately accumulated tail.
    wheel(&mut ui, pos, 8.0, false);
    let committed = project_fingerprint(&ui.app.project);
    let origin = ui.app.piano_viewport.y.origin();
    let mut ordinary = wheel_event(-1.0, false, egui::MouseWheelUnit::Line);
    if let egui::Event::MouseWheel { modifiers, .. } = &mut ordinary {
        *modifiers = egui::Modifiers::NONE;
    }
    ui.run(vec![
        egui::Event::PointerMoved(Pos2::new(grid.left() - 20.0, pos.y)),
        ordinary,
    ]);
    assert!(
        ui.app.piano_viewport.y.origin() > origin,
        "fresh reverse scroll must move in its own direction"
    );
    assert_eq!(project_fingerprint(&ui.app.project), committed);
}

#[test]
fn piano_expression_properties_short_resize_only_note_and_legacy_fields_stay_exact() {
    let mut ui = fixture();
    ui.app.piano_roll_state.grouping_enabled = false;
    ui.app.project.active_pattern_mut().notes[0].length = 0.01;
    ui.app.sync_history_observer();
    ui.app.undo_stack.clear();
    ui.settle();
    let edge = ui
        .ctx
        .read_response(Id::new(("piano-note", 90001_u64)).with("resize"))
        .unwrap()
        .rect
        .center();
    let before = project_fingerprint(&ui.app.project);
    double_click(&mut ui, edge);
    assert!(
        ui.app.piano_note_properties.is_some(),
        "resize-only small note remains double-clickable"
    );
    assert_eq!(project_fingerprint(&ui.app.project), before);
    ui.click("Apply");
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert!(ui.app.undo_stack.is_empty());
    ui.app.show_inspector = true;
    ui.size = Vec2::new(1920.0, 1080.0);
    for (start, length) in [(0.375, 0.01), (4096.5, 0.01), (0.375, 8192.0)] {
        ui.app.project.active_pattern_mut().notes[0].start = start;
        ui.app.project.active_pattern_mut().notes[0].length = length;
        ui.app.piano_roll_state.selection_ids = HashSet::from([90001]);
        ui.app.sync_history_observer();
        ui.app.undo_stack.clear();
        ui.settle();
        ui.click("Edit note properties");
        let slider = field(&ui, "Velocity").rect;
        let p = Pos2::new(slider.left() + slider.width() * 0.25, slider.center().y);
        pointer(&mut ui, p, true);
        pointer(&mut ui, p, false);
        ui.settle();
        ui.click("Apply");
        assert!(ui.app.piano_note_properties.is_none());
        assert_eq!(
            (note(&ui, 90001).start, note(&ui, 90001).length),
            (start, length)
        );
    }
}

#[test]
fn piano_expression_properties_start_only_moves_short_notes_with_one_undo() {
    for length in [crate::piano_roll::MIN_NOTE_LENGTH_BEATS, 1.0 / 24.0] {
        let mut ui = fixture();
        ui.app.piano_roll_state.grouping_enabled = false;
        ui.app.project.active_pattern_mut().notes[0].length = length;
        ui.app.sync_history_observer();
        ui.app.undo_stack.clear();
        ui.settle();
        let before = project_fingerprint(&ui.app.project);
        let edge = ui
            .ctx
            .read_response(Id::new(("piano-note", 90001_u64)).with("resize"))
            .unwrap()
            .rect
            .center();
        double_click(&mut ui, edge);
        set_number(&mut ui, "Start (beats)", "1.125");
        ui.click("Apply");
        assert!(
            ui.app.piano_note_properties.is_none(),
            "valid short-note timing must apply"
        );
        assert_eq!(
            (note(&ui, 90001).start, note(&ui, 90001).length),
            (1.125, length)
        );
        assert_eq!(ui.app.undo_stack.len(), 1);
        let after = project_fingerprint(&ui.app.project);
        ui.key(egui::Key::Z, piano_clipboard_command());
        assert_eq!(project_fingerprint(&ui.app.project), before);
        ui.key(egui::Key::Y, piano_clipboard_command());
        assert_eq!(project_fingerprint(&ui.app.project), after);
    }
}

#[test]
fn piano_expression_properties_accepts_typed_short_lengths_and_final_legal_start() {
    for (text, length, start) in [
        (
            "0.015625",
            crate::piano_roll::MIN_NOTE_LENGTH_BEATS,
            "4095.984375",
        ),
        ("0.041666667", 1.0 / 24.0, "1.125"),
    ] {
        let mut ui = fixture();
        ui.app.piano_roll_state.grouping_enabled = false;
        let before = project_fingerprint(&ui.app.project);
        let pos = body(&ui, 90001);
        double_click(&mut ui, pos);
        set_number(&mut ui, "Length (beats)", text);
        set_number(&mut ui, "Start (beats)", start);
        assert_eq!(
            project_fingerprint(&ui.app.project),
            before,
            "numeric edits stay draft-only"
        );
        ui.click("Apply");
        assert!(ui.app.piano_note_properties.is_none());
        assert_eq!(
            note(&ui, 90001).length,
            length,
            "typed lengths must not clamp to 0.05"
        );
        assert_eq!(note(&ui, 90001).start, start.parse::<f32>().unwrap());
        assert_eq!(ui.app.undo_stack.len(), 1);
        let after = project_fingerprint(&ui.app.project);
        ui.key(egui::Key::Z, piano_clipboard_command());
        assert_eq!(project_fingerprint(&ui.app.project), before);
        ui.key(egui::Key::Y, piano_clipboard_command());
        assert_eq!(project_fingerprint(&ui.app.project), after);
    }
}

#[test]
fn piano_expression_double_click_batched_and_resize_modifiers_keep_press_ownership() {
    let mut ui = fixture();
    let before = project_fingerprint(&ui.app.project);
    let pos = body(&ui, 90001);
    for _ in 0..2 {
        ui.run(vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::CTRL,
            },
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
    }
    assert!(
        ui.app.piano_note_properties.is_none(),
        "Ctrl selection on the down event must not become properties"
    );
    assert_eq!(project_fingerprint(&ui.app.project), before);
    ui.settle();
    let edge = ui
        .ctx
        .read_response(Id::new(("piano-note", 90001_u64)).with("resize"))
        .unwrap()
        .rect
        .center();
    for _ in 0..2 {
        ui.run_with_modifiers(
            vec![
                egui::Event::PointerMoved(edge),
                egui::Event::PointerButton {
                    pos: edge,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::ALT,
                },
            ],
            egui::Modifiers::ALT,
        );
        pointer(&mut ui, edge, false);
    }
    assert!(
        ui.app.piano_note_properties.is_none(),
        "resize remembers Alt at press, even when released first"
    );
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert!(ui.app.undo_stack.is_empty());
}

#[test]
fn piano_expression_properties_import_completion_waits_and_keeps_own_history() {
    let mut ui = fixture();
    ui.app.piano_roll_state.grouping_enabled = false;
    let pos = body(&ui, 90001);
    let before = project_fingerprint(&ui.app.project);
    double_click(&mut ui, pos);
    set_number(&mut ui, "Pitch (MIDI)", "62");
    let (sender, receiver) = mpsc::channel();
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&40_u32.to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&8_000_u32.to_le_bytes());
    bytes.extend_from_slice(&16_000_u32.to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&4_u32.to_le_bytes());
    bytes.extend_from_slice(&[0, 0, 0, 64]);
    let decoded = wav::decode_wav(&bytes).unwrap();
    sender
        .send(Ok((
            ui.app.audio_asset_generation,
            PathBuf::from("expression-pending-import.wav"),
            decoded,
            0.0,
            4,
        )))
        .unwrap();
    ui.app.audio_import_receiver = Some(receiver);
    ui.settle();
    assert!(ui.app.audio_import_receiver.is_some());
    assert_eq!(project_fingerprint(&ui.app.project), before);
    ui.click("Apply");
    assert!(ui.app.audio_import_receiver.is_none());
    assert_eq!(note(&ui, 90001).note, 62);
    assert_eq!(ui.app.undo_stack.len(), 2);
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(
        note(&ui, 90001).note,
        62,
        "import Undo preserves properties edit"
    );
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), before);
}
