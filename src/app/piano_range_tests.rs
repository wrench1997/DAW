//! Production egui pointer/key coverage for independent Piano ranges and snap settings.
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

fn click(ui: &mut UiHarness, pos: Pos2, modifiers: egui::Modifiers) {
    mouse(ui, pos, Some(true), modifiers);
    mouse(ui, pos, Some(false), modifiers);
    ui.settle();
}

fn drag(ui: &mut UiHarness, from: Pos2, to: Pos2, press: egui::Modifiers, held: egui::Modifiers) {
    mouse(ui, from, Some(true), press);
    mouse(ui, from, None, held);
    mouse(ui, to, None, held);
    mouse(ui, to, Some(false), held);
    ui.settle();
}

fn fixture() -> UiHarness {
    let mut ui = UiHarness::floating();
    piano_clipboard_fixture(&mut ui);
    ui.app.piano_roll_state.tool = PianoRollTool::Draw;
    ui
}

fn reset_history(ui: &mut UiHarness) {
    ui.app.sync_history_observer();
    ui.app.undo_stack.clear();
    ui.app.redo_stack.clear();
    ui.app.project_fingerprint = project_fingerprint(&ui.app.project);
    ui.app.dirty = false;
    ui.settle();
}

fn ruler(ui: &UiHarness, beat: f64) -> Pos2 {
    let rect = ui
        .ctx
        .read_response(Id::new("piano-range-ruler"))
        .unwrap()
        .rect;
    let point = Pos2::new(
        rect.left() + ui.app.piano_viewport.x.pixel_for_content(beat) as f32,
        rect.center().y,
    );
    assert!(
        rect.contains(point),
        "ruler beat {beat}: {point:?} outside {rect:?}"
    );
    point
}

fn grid(ui: &UiHarness, beat: f64, pitch: u8) -> Pos2 {
    let rect = ui.ctx.read_response(Id::new("piano-grid")).unwrap().rect;
    let point = Pos2::new(
        rect.left() + ui.app.piano_viewport.x.pixel_for_content(beat) as f32,
        rect.top()
            + ui.app
                .piano_viewport
                .y
                .pixel_for_content(piano_pitch_row(pitch) + 0.5) as f32,
    );
    assert!(
        rect.contains(point),
        "grid beat {beat}: {point:?} outside {rect:?}"
    );
    point
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
        .find(|note| note.id == id)
        .unwrap()
}

fn set_range(ui: &mut UiHarness, start: f64, end: f64) {
    let from = ruler(ui, start);
    let to = ruler(ui, end);
    drag(
        ui,
        from,
        to,
        piano_clipboard_command(),
        piano_clipboard_command(),
    );
}

fn assert_range(ui: &UiHarness, start: f64, end: f64) {
    let range = ui
        .app
        .piano_roll_state
        .repeat_range
        .expect("an actual ruler drag should create a range");
    assert!(
        (range.start - start).abs() < 0.000_001,
        "{range:?}, expected start {start}"
    );
    assert!(
        (range.end - end).abs() < 0.000_001,
        "{range:?}, expected end {end}"
    );
}

#[test]
fn piano_range_ruler_is_half_open_group_aware_and_clear_preserves_notes() {
    let mut ui = fixture();
    let source = note(&ui, 90_001).clone();
    ui.app.project.active_pattern_mut().notes.extend([
        PianoNote {
            id: 90_004,
            start: 0.5,
            note: 62,
            group_id: None,
            ..source.clone()
        },
        PianoNote {
            id: 90_005,
            start: 0.0,
            length: 0.5,
            note: 61,
            group_id: None,
            ..source
        },
    ]);
    ui.app.project.active_pattern_mut().notes[2].start = 0.375;
    reset_history(&mut ui);
    let project = project_fingerprint(&ui.app.project);
    let transport = (ui.app.transport_mode, ui.app.beat_position);
    set_range(&mut ui, 0.25, 0.5);
    assert_range(&ui, 0.25, 0.5);
    assert_eq!(
        ui.app.piano_roll_state.selection_ids,
        HashSet::from([90_001, 90_002])
    );
    assert_eq!(project_fingerprint(&ui.app.project), project);
    assert_eq!((ui.app.transport_mode, ui.app.beat_position), transport);
    assert!(ui.app.undo_stack.is_empty());
    assert!(!ui.app.dirty);

    ui.key(egui::Key::D, piano_clipboard_command());
    assert!(ui.app.piano_roll_state.selection_ids.is_empty());
    assert_range(&ui, 0.25, 0.5);
    ui.key(egui::Key::A, piano_clipboard_command());
    let selection = ui.app.piano_roll_state.selection_ids.clone();
    ui.click("Clear range");
    assert!(ui.app.piano_roll_state.repeat_range.is_none());
    assert_eq!(ui.app.piano_roll_state.selection_ids, selection);
    assert_eq!(project_fingerprint(&ui.app.project), project);
    assert!(ui.app.undo_stack.is_empty());
    assert!(!ui.app.dirty);

    ui.app.piano_roll_state.grouping_enabled = false;
    set_range(&mut ui, 0.25, 0.5);
    assert_eq!(
        ui.app.piano_roll_state.selection_ids,
        HashSet::from([90_001])
    );
}

#[test]
fn piano_range_plain_click_drag_and_control_click_do_not_invent_an_interval() {
    let mut ui = fixture();
    ui.app.piano_roll_state.selection_ids = HashSet::from([90_001, 90_002]);
    let project = project_fingerprint(&ui.app.project);
    let selection = ui.app.piano_roll_state.selection_ids.clone();
    let from = ruler(&ui, 1.0);
    let to = ruler(&ui, 2.0);
    click(&mut ui, from, egui::Modifiers::NONE);
    drag(
        &mut ui,
        from,
        to,
        egui::Modifiers::NONE,
        egui::Modifiers::CTRL,
    );
    click(&mut ui, from, piano_clipboard_command());
    assert!(ui.app.piano_roll_state.repeat_range.is_none());
    assert_eq!(ui.app.piano_roll_state.selection_ids, selection);
    assert_eq!(project_fingerprint(&ui.app.project), project);
    assert!(ui.app.undo_stack.is_empty());
    set_range(&mut ui, 1.0, 2.0);
    let from = ruler(&ui, 3.0);
    click(&mut ui, from, piano_clipboard_command());
    assert_range(&ui, 1.0, 2.0);
}

#[test]
fn piano_range_reverse_drag_uses_press_modifiers_and_additive_selection() {
    let mut ui = fixture();
    ui.app.piano_roll_state.selection_ids.insert(90_003);
    let press = egui::Modifiers {
        shift: true,
        ..piano_clipboard_command()
    };
    let from = ruler(&ui, 1.0);
    let to = ruler(&ui, 0.25);
    // The input batch's final modifier state must not replace the button-down state.
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
        egui::Modifiers::NONE,
    );
    mouse(&mut ui, to, None, egui::Modifiers::NONE);
    mouse(&mut ui, to, Some(false), egui::Modifiers::NONE);
    ui.settle();
    assert_range(&ui, 0.25, 1.0);
    assert_eq!(
        ui.app.piano_roll_state.selection_ids,
        HashSet::from([90_001, 90_002, 90_003])
    );
    assert!(ui.app.undo_stack.is_empty());
}

#[test]
fn piano_range_selection_and_width_keys_are_view_local_and_independent_of_notes() {
    let mut ui = fixture();
    let before = project_fingerprint(&ui.app.project);
    // An empty selection does not use the selected-or-all edit fallback.
    ui.key(egui::Key::Enter, piano_clipboard_command());
    assert!(ui.app.piano_roll_state.repeat_range.is_none());
    let p = body(&ui, 90_001);
    click(&mut ui, p, egui::Modifiers::CTRL);
    ui.key(egui::Key::Enter, piano_clipboard_command());
    assert_range(&ui, 0.375, 1.125);
    ui.key(egui::Key::ArrowRight, piano_clipboard_command());
    assert_range(&ui, 1.125, 1.875);
    ui.key(egui::Key::ArrowLeft, piano_clipboard_command());
    ui.key(egui::Key::ArrowLeft, piano_clipboard_command());
    assert_range(&ui, 0.0, 0.75);
    ui.key(egui::Key::ArrowLeft, piano_clipboard_command());
    assert_range(&ui, 0.0, 0.75);
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert!(ui.app.undo_stack.is_empty());
    ui.key(egui::Key::ArrowRight, egui::Modifiers::SHIFT);
    assert_eq!(note(&ui, 90_001).start, 0.625);
    assert_range(&ui, 0.0, 0.75);
    assert_eq!(ui.app.undo_stack.len(), 1);
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert_range(&ui, 0.0, 0.75);
}

#[test]
fn piano_range_repeat_uses_exact_width_even_outside_range_and_keeps_one_undo() {
    let mut ui = fixture();
    let mut other = ui.app.project.active_pattern().clone();
    other.id += 100;
    for (index, note) in other.notes.iter_mut().enumerate() {
        note.id = 1_000_000 + index as u64;
    }
    ui.app.project.patterns.push(other);
    reset_history(&mut ui);
    let before = project_fingerprint(&ui.app.project);
    set_range(&mut ui, 2.0, 2.5);
    assert!(ui.app.piano_roll_state.selection_ids.is_empty());
    let from = body(&ui, 90_001);
    click(&mut ui, from, egui::Modifiers::CTRL);
    assert_range(&ui, 2.0, 2.5);
    ui.key(egui::Key::B, piano_clipboard_command());
    let notes = &ui.app.project.active_pattern().notes;
    assert_eq!(notes.len(), 5);
    let copies = &notes[3..];
    assert_eq!((copies[0].start, copies[1].start), (0.875, 1.25));
    assert_eq!((copies[0].length, copies[1].length), (0.75, 0.25));
    assert_eq!((copies[0].velocity, copies[1].velocity), (0.625, 0.25));
    assert!(!copies[0].muted && copies[1].muted);
    assert_eq!(copies[0].group_id, copies[1].group_id);
    assert_ne!(copies[0].group_id, Some(123));
    assert!(copies.iter().all(|note| note.id > 1_000_002));
    assert_eq!(
        ui.app.piano_roll_state.selection_ids,
        copies.iter().map(|note| note.id).collect()
    );
    assert_range(&ui, 2.0, 2.5);
    assert_eq!(ui.app.undo_stack.len(), 1);
    let repeated = project_fingerprint(&ui.app.project);
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert_range(&ui, 2.0, 2.5);
    ui.key(egui::Key::Y, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), repeated);
    ui.key(egui::Key::D, piano_clipboard_command());
    ui.key(egui::Key::B, piano_clipboard_command());
    assert_eq!(
        ui.app.project.active_pattern().notes.len(),
        9,
        "deselected range repeat includes all four active-channel notes and excludes the ghost"
    );
    assert_range(&ui, 2.0, 2.5);
    let ids: Vec<_> = ui
        .app
        .project
        .patterns
        .iter()
        .flat_map(|pattern| &pattern.notes)
        .map(|note| note.id)
        .collect();
    assert_eq!(ids.len(), ids.iter().collect::<HashSet<_>>().len());
}

#[test]
fn piano_range_session_pattern_index_stable_id_and_channel_changes_drop_old_range() {
    for owner_change in 0..5 {
        let mut ui = fixture();
        set_range(&mut ui, 1.0, 2.0);
        let old_owner = ui.app.piano_roll_state.range_owner;
        let old_channel = ui.app.selected_channel;
        match owner_change {
            0 => ui.app.project_session += 1,
            1 => {
                let mut next = ui.app.project.active_pattern().clone();
                next.id += 100;
                for note in &mut next.notes {
                    note.id += 100;
                }
                ui.app.project.patterns.push(next);
                ui.app.project.active_pattern = ui.app.project.patterns.len() - 1;
            }
            2 => ui.app.project.active_pattern_mut().id += 100,
            3 => ui.app.selected_channel = (old_channel + 1) % ui.app.project.channels.len(),
            _ => ui.app.selected_channel = usize::MAX,
        }
        ui.settle();
        assert!(
            ui.app.piano_roll_state.repeat_range.is_none(),
            "owner change {owner_change}"
        );
        assert_ne!(ui.app.piano_roll_state.range_owner, old_owner);
        if owner_change >= 3 {
            ui.app.selected_channel = old_channel;
            ui.settle();
            assert!(
                ui.app.piano_roll_state.repeat_range.is_none(),
                "switching back cannot resurrect a channel range"
            );
        }
        ui.key(egui::Key::ArrowRight, piano_clipboard_command());
        assert!(ui.app.piano_roll_state.repeat_range.is_none());
    }
}

#[test]
fn piano_range_interruptions_restore_preview_and_held_pointer_never_resumes() {
    for interruption in 0..4 {
        let mut ui = fixture();
        set_range(&mut ui, 2.0, 3.0);
        ui.app.piano_roll_state.selection_ids = HashSet::from([90_003]);
        let before = project_fingerprint(&ui.app.project);
        let from = ruler(&ui, 0.25);
        let to = ruler(&ui, 1.0);
        mouse(&mut ui, from, Some(true), piano_clipboard_command());
        mouse(&mut ui, to, None, piano_clipboard_command());
        assert_range(&ui, 0.25, 1.0);
        assert_eq!(
            ui.app.piano_roll_state.selection_ids,
            HashSet::from([90_001, 90_002])
        );
        match interruption {
            0 => {
                ui.key(egui::Key::F10, egui::Modifiers::NONE);
                ui.key(egui::Key::F10, egui::Modifiers::NONE);
            }
            1 => {
                ui.key(egui::Key::F9, egui::Modifiers::NONE);
                ui.key(egui::Key::F7, egui::Modifiers::NONE);
            }
            2 => {
                ui.size = Vec2::new(1080.0, 680.0);
                ui.run(Vec::new());
            }
            _ => {
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
            }
        }
        assert_range(&ui, 2.0, 3.0);
        assert_eq!(
            ui.app.piano_roll_state.selection_ids,
            HashSet::from([90_003]),
            "interruption {interruption}"
        );
        assert!(!piano_range::active(&ui.ctx));
        mouse(
            &mut ui,
            to + Vec2::new(60.0, 0.0),
            None,
            piano_clipboard_command(),
        );
        mouse(
            &mut ui,
            to + Vec2::new(60.0, 0.0),
            Some(false),
            piano_clipboard_command(),
        );
        ui.settle();
        assert_range(&ui, 2.0, 3.0);
        assert_eq!(
            ui.app.piano_roll_state.selection_ids,
            HashSet::from([90_003])
        );
        assert_eq!(project_fingerprint(&ui.app.project), before);
        assert!(ui.app.undo_stack.is_empty());
        set_range(&mut ui, 0.25, 1.0);
        assert_range(&ui, 0.25, 1.0);
    }
}

#[test]
fn piano_range_covered_window_never_claims_a_frontmost_pointer_drag() {
    let mut ui = fixture();
    let notes = serde_json::to_value(&ui.app.project.active_pattern().notes).unwrap();
    ui.click("Cascade windows");
    ui.key(egui::Key::F5, egui::Modifiers::NONE);
    let ruler_rect = ui
        .ctx
        .read_response(Id::new("piano-range-ruler"))
        .unwrap()
        .rect;
    let covered = ruler_rect
        .intersect(ui.editor_rect(StudioView::Playlist))
        .shrink2(Vec2::new(15.0, 2.0));
    assert!(covered.is_positive());
    let from = Pos2::new(covered.left() + 10.0, covered.center().y);
    let to = Pos2::new((from.x + 80.0).min(covered.right()), from.y);
    assert_eq!(
        ui.ctx.layer_id_at(from).unwrap().id,
        workspace::window_id(StudioView::Playlist)
    );
    drag(
        &mut ui,
        from,
        to,
        piano_clipboard_command(),
        piano_clipboard_command(),
    );
    assert!(ui.app.piano_roll_state.repeat_range.is_none());
    assert!(!piano_range::active(&ui.ctx));
    assert_eq!(
        serde_json::to_value(&ui.app.project.active_pattern().notes).unwrap(),
        notes
    );
    ui.key(egui::Key::F7, egui::Modifiers::NONE);
    set_range(&mut ui, 0.25, 1.0);
    assert_range(&ui, 0.25, 1.0);
}

#[test]
fn piano_range_paste_uses_viewport_bar_in_both_transport_modes() {
    for mode in [TransportMode::Pattern, TransportMode::Song] {
        let mut ui = fixture();
        ui.app.project.active_pattern_mut().length_steps = 256;
        reset_history(&mut ui);
        ui.key(egui::Key::A, piano_clipboard_command());
        let output = ui.run(vec![
            clipboard_key(egui::Key::C, true, false),
            egui::Event::Copy,
        ]);
        let text = copied_note_text(&output);
        ui.run(vec![clipboard_key(egui::Key::C, false, false)]);
        let axis = &mut ui.app.piano_viewport.x;
        axis.scroll_by_pixels((5.5 - axis.origin()) * axis.pixels_per_unit())
            .unwrap();
        ui.settle();
        assert!((ui.app.piano_viewport.x.origin() - 5.5).abs() < 0.000_001);
        set_range(&mut ui, 6.0, 7.0);
        ui.app.transport_mode = mode;
        ui.app.beat_position = 13.375;
        let before = project_fingerprint(&ui.app.project);
        ui.run(vec![
            clipboard_key(egui::Key::V, true, false),
            egui::Event::Paste(text),
        ]);
        ui.run(vec![clipboard_key(egui::Key::V, false, false)]);
        ui.settle();
        let notes = &ui.app.project.active_pattern().notes;
        assert_eq!(notes.len(), 5);
        assert_eq!(
            (notes[3].start, notes[4].start),
            (4.0, 4.375),
            "mode {mode:?}"
        );
        assert_eq!(ui.app.beat_position, 13.375);
        assert_range(&ui, 6.0, 7.0);
        assert_eq!(ui.app.undo_stack.len(), 1);
        ui.key(egui::Key::Z, piano_clipboard_command());
        assert_eq!(project_fingerprint(&ui.app.project), before);
    }
}

#[test]
fn piano_range_off_and_alt_press_keep_unsnapped_endpoints() {
    for local_off in [false, true] {
        let mut ui = fixture();
        ui.app.piano_roll_state.local_snap = if local_off {
            PianoSnap::Off
        } else {
            PianoSnap::Beat
        };
        ui.settle();
        let press = egui::Modifiers {
            alt: !local_off,
            ..piano_clipboard_command()
        };
        let from = ruler(&ui, 1.37);
        let to = ruler(&ui, 2.63);
        let rect = ui
            .ctx
            .read_response(Id::new("piano-range-ruler"))
            .unwrap()
            .rect;
        let start = ui
            .app
            .piano_viewport
            .x
            .content_at_pixel(f64::from(from.x - rect.left()));
        let end = ui
            .app
            .piano_viewport
            .x
            .content_at_pixel(f64::from(to.x - rect.left()));
        drag(&mut ui, from, to, press, piano_clipboard_command());
        assert_range(&ui, start, end);
        assert!(ui.app.undo_stack.is_empty());
    }
}

#[test]
fn piano_snap_off_draw_move_resize_use_raw_motion_with_one_step_undo() {
    for operation in 0..3 {
        let mut ui = fixture();
        ui.app.piano_roll_state.local_snap = PianoSnap::Off;
        let original = project_fingerprint(&ui.app.project);
        let scale = ui.app.piano_viewport.x.pixels_per_unit() as f32;
        match operation {
            0 => {
                let p = grid(&ui, 2.37, 62);
                click(&mut ui, p, egui::Modifiers::NONE);
                let drawn = ui.app.project.active_pattern().notes.last().unwrap();
                assert!((drawn.start - 2.37).abs() < 0.000_1, "{}", drawn.start);
            }
            1 => {
                let from = body(&ui, 90_001);
                let to = from + Vec2::new(scale * 0.37, 0.0);
                let expected = 0.375 + (to.x - from.x) / scale;
                drag(
                    &mut ui,
                    from,
                    to,
                    egui::Modifiers::NONE,
                    egui::Modifiers::NONE,
                );
                assert!((note(&ui, 90_001).start - expected).abs() < 0.000_01);
                assert!(
                    (note(&ui, 90_002).start - note(&ui, 90_001).start - 0.375).abs() < 0.000_01
                );
            }
            _ => {
                let from = ui
                    .ctx
                    .read_response(Id::new(("piano-note", 90_001_u64)).with("resize"))
                    .unwrap()
                    .rect
                    .center();
                let to = from + Vec2::new(scale * 0.37, 0.0);
                let expected = 0.75 + (to.x - from.x) / scale;
                drag(
                    &mut ui,
                    from,
                    to,
                    egui::Modifiers::NONE,
                    egui::Modifiers::NONE,
                );
                assert!(
                    (note(&ui, 90_001).length - expected).abs() < 0.000_01,
                    "{} vs {expected}",
                    note(&ui, 90_001).length
                );
            }
        }
        assert_eq!(ui.app.undo_stack.len(), 1, "operation {operation}");
        assert_eq!(note(&ui, 90_003).start, 0.0);
        let edited = project_fingerprint(&ui.app.project);
        assert_ne!(edited, original);
        ui.key(egui::Key::Z, piano_clipboard_command());
        assert_eq!(project_fingerprint(&ui.app.project), original);
        ui.key(egui::Key::Y, piano_clipboard_command());
        assert_eq!(project_fingerprint(&ui.app.project), edited);
    }
}

#[test]
fn piano_snap_off_keyboard_fallback_and_quantize_notice_are_deliberate() {
    let mut ui = fixture();
    ui.app.piano_roll_state.local_snap = PianoSnap::Off;
    ui.app.piano_roll_state.selection_ids.insert(90_001);
    let before = project_fingerprint(&ui.app.project);
    let mut quick = piano_clipboard_command();
    quick.alt = cfg!(target_os = "macos");
    for modifiers in [egui::Modifiers::SHIFT, quick] {
        ui.key(egui::Key::Q, modifiers);
        assert_eq!(project_fingerprint(&ui.app.project), before);
        assert!(ui.app.undo_stack.is_empty());
        assert!(
            ui.app
                .toast
                .as_ref()
                .is_some_and(|(message, _)| message.contains("snap grid"))
        );
    }
    ui.key(egui::Key::ArrowRight, egui::Modifiers::SHIFT);
    assert_eq!(note(&ui, 90_001).start, 0.375 + 1.0 / 64.0);
    assert_eq!(note(&ui, 90_002).start, 0.75 + 1.0 / 64.0);
    assert_eq!(note(&ui, 90_003).start, 0.0);
    assert_eq!(ui.app.undo_stack.len(), 1);
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), before);
    ui.key(egui::Key::D, egui::Modifiers::SHIFT);
    assert_eq!(note(&ui, 90_001).length, 1.0 / 64.0);
    assert_eq!(note(&ui, 90_002).length, 1.0 / 64.0);
    assert_eq!(note(&ui, 90_003).length, 0.25);
    assert_eq!(ui.app.undo_stack.len(), 1);
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), before);
}

#[test]
fn piano_snap_triplet_paint_and_pointer_agree_across_zoom_and_late_horizon() {
    for (snap, denominator) in [
        (PianoSnap::TwentyFourthBeat, 24.0),
        (PianoSnap::TwelfthBeat, 12.0),
        (PianoSnap::SixthBeat, 6.0),
        (PianoSnap::ThirdBeat, 3.0),
    ] {
        for late in [false, true] {
            let mut ui = fixture();
            ui.app.piano_roll_state.local_snap = snap;
            ui.app.piano_roll_state.last_note_length = 0.25;
            if late {
                ui.app.project.active_pattern_mut().notes[0].start = 4095.0;
                ui.app.project.active_pattern_mut().notes[0].length = 0.5;
            }
            reset_history(&mut ui);
            let base = if late { 4094.0 } else { 2.0 };
            let desired_scale = if late { 96.0 } else { 24.0 };
            let scale = ui.app.piano_viewport.x.pixels_per_unit();
            ui.app
                .piano_viewport
                .x
                .zoom_at_pixel(0.0, desired_scale / scale)
                .unwrap();
            ui.app
                .piano_viewport
                .x
                .reveal(base, base + 2.0, 0.0)
                .unwrap();
            ui.settle();
            let output = ui.run(Vec::new());
            let rect = ui.ctx.read_response(Id::new("piano-grid")).unwrap().rect;
            let axis = ui.app.piano_viewport.x;
            let (start, end) = axis.visible_range();
            let lines = crate::piano_snap::grid_lines(start, end, axis.pixels_per_unit(), snap);
            assert!(!lines.is_empty());
            for line in &lines {
                assert!(
                    (line.beat * denominator - (line.beat * denominator).round()).abs() < 0.000_001
                );
                let x = rect.left() + axis.pixel_for_content(line.beat) as f32;
                // Inspect production paint output, not a test-only rendering implementation.
                assert!(
                    output.shapes.iter().any(|shape| match &shape.shape {
                        egui::epaint::Shape::LineSegment { points, .. } =>
                            (points[0].x - x).abs() < 0.01
                                && (points[1].x - x).abs() < 0.01
                                && (points[0].y - rect.top()).abs() < 0.01
                                && (points[1].y - rect.bottom()).abs() < 0.01,
                        _ => false,
                    }),
                    "missing {snap:?} grid line at {} / {x}",
                    line.beat
                );
            }
            let expected = base + 1.0 / denominator;
            let p = grid(&ui, expected + 0.2 / denominator, 62);
            let before = project_fingerprint(&ui.app.project);
            click(&mut ui, p, egui::Modifiers::NONE);
            assert_eq!(
                ui.app.project.active_pattern().notes.last().unwrap().start,
                expected as f32,
                "{snap:?}, late={late}"
            );
            assert_eq!(ui.app.undo_stack.len(), 1);
            ui.key(egui::Key::Z, piano_clipboard_command());
            assert_eq!(project_fingerprint(&ui.app.project), before);
            set_range(&mut ui, expected, expected + 1.0);
            assert_range(&ui, expected, expected + 1.0);
        }
    }
}

#[test]
fn piano_range_triplet_wide_and_minimum_layouts_keep_controls_on_screen() {
    let mut ui = fixture();
    ui.app.piano_roll_state.local_snap = PianoSnap::SixthBeat;
    ui.click("Maximize editor");
    ui.settle();
    set_range(&mut ui, 1.0 / 3.0, 4.0 / 3.0);
    assert_range(&ui, 1.0 / 3.0, 4.0 / 3.0);
    ui.capture("piano-range-triplet-wide");
    ui.size = Vec2::new(1080.0, 680.0);
    ui.settle();
    let screen = Rect::from_min_size(Pos2::ZERO, ui.size);
    for label in [
        "Range from selection",
        "Range left",
        "Range right",
        "Clear range",
    ] {
        let bounds = ui.button(label).bounds().unwrap();
        assert!(
            screen.contains(Pos2::new(bounds.x0 as f32, bounds.y0 as f32)),
            "{label}"
        );
        assert!(
            screen.contains(Pos2::new(bounds.x1 as f32, bounds.y1 as f32)),
            "{label}"
        );
    }
    let grid_rect = ui.ctx.read_response(Id::new("piano-grid")).unwrap().rect;
    assert!(
        grid_rect.width() > 300.0 && grid_rect.height() > 80.0,
        "{grid_rect:?}"
    );
    // Changing the status text in the narrow layout must not move/cancel the ruler.
    ui.click("Clear range");
    set_range(&mut ui, 2.0 / 3.0, 5.0 / 3.0);
    assert_range(&ui, 2.0 / 3.0, 5.0 / 3.0);
    ui.capture("piano-range-triplet-minimum-window");
    ui.click("Clear range");
    assert!(ui.app.piano_roll_state.repeat_range.is_none());
    assert!(ui.app.undo_stack.is_empty());
}

#[test]
fn piano_snap_slice_accepts_first_triplet_and_off_minimum_boundary_with_one_undo() {
    for (snap, split) in [
        (PianoSnap::TwentyFourthBeat, 1.0_f64 / 24.0),
        (PianoSnap::Off, 1.0_f64 / 64.0),
    ] {
        let mut ui = fixture();
        let source = PianoNote {
            start: 0.0,
            length: 1.0,
            group_id: None,
            ..note(&ui, 90_001).clone()
        };
        let pitch = source.note;
        ui.app.project.active_pattern_mut().notes = vec![source];
        ui.app.piano_roll_state.local_snap = snap;
        reset_history(&mut ui);
        ui.key(egui::Key::C, egui::Modifiers::NONE);
        assert_eq!(ui.app.piano_roll_state.tool, PianoRollTool::Slice);
        let scale = ui.app.piano_viewport.x.pixels_per_unit();
        ui.app
            .piano_viewport
            .x
            .zoom_at_pixel(0.0, 512.0 / scale)
            .unwrap();
        ui.settle();
        let before = project_fingerprint(&ui.app.project);
        if snap == PianoSnap::Off {
            let too_short = grid(&ui, split / 2.0, pitch);
            click(&mut ui, too_short, egui::Modifiers::NONE);
            assert_eq!(project_fingerprint(&ui.app.project), before);
            assert!(
                ui.app.undo_stack.is_empty(),
                "subminimum slices must not create history"
            );
        }
        let point = grid(&ui, split, pitch);
        let body = ui
            .ctx
            .read_response(Id::new(("piano-note", 90_001_u64)))
            .unwrap()
            .rect;
        assert!(
            body.contains(point),
            "fine-grid split must hit the note body: {point:?}, {body:?}"
        );
        click(&mut ui, point, egui::Modifiers::NONE);
        let notes = &ui.app.project.active_pattern().notes;
        assert_eq!(notes.len(), 2, "{snap:?}");
        assert_eq!((notes[0].start, notes[0].length), (0.0, split as f32));
        assert_eq!(
            (notes[1].start, notes[1].length),
            (split as f32, (1.0 - split) as f32)
        );
        assert!(
            notes
                .iter()
                .all(|note| note.length >= crate::piano_roll::MIN_NOTE_LENGTH_BEATS)
        );
        assert_ne!(notes[0].id, notes[1].id);
        assert_eq!(ui.app.undo_stack.len(), 1);
        ui.key(egui::Key::Z, piano_clipboard_command());
        assert_eq!(project_fingerprint(&ui.app.project), before);
    }
}

fn first_ruler_click(ui: &mut UiHarness, point: Pos2, modifiers: egui::Modifiers) {
    // Do not settle: three synthetic idle frames would consume egui's double-click delay.
    mouse(ui, point, Some(true), modifiers);
    mouse(ui, point, Some(false), modifiers);
}

#[test]
fn piano_range_unmodified_double_click_drag_uses_real_click_timing() {
    for batched in [false, true] {
        let mut ui = fixture();
        let before = project_fingerprint(&ui.app.project);
        let from = ruler(&ui, 0.25);
        let to = ruler(&ui, 1.0);
        first_ruler_click(&mut ui, from, egui::Modifiers::NONE);
        assert!(ui.app.piano_roll_state.repeat_range.is_none());
        if batched {
            ui.run(vec![
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
            ]);
            ui.settle();
        } else {
            drag(
                &mut ui,
                from,
                to,
                egui::Modifiers::NONE,
                egui::Modifiers::NONE,
            );
        }
        assert_range(&ui, 0.25, 1.0);
        assert_eq!(
            ui.app.piano_roll_state.selection_ids,
            HashSet::from([90_001, 90_002])
        );
        assert_eq!(project_fingerprint(&ui.app.project), before);
        assert!(ui.app.undo_stack.is_empty());
        assert!(!piano_range::active(&ui.ctx));
    }
}

#[test]
fn piano_range_plain_double_click_without_drag_preserves_interval_and_selection() {
    let mut ui = fixture();
    set_range(&mut ui, 2.0, 3.0);
    ui.app.piano_roll_state.selection_ids = HashSet::from([90_003]);
    let before = project_fingerprint(&ui.app.project);
    let point = ruler(&ui, 0.25);
    first_ruler_click(&mut ui, point, egui::Modifiers::NONE);
    first_ruler_click(&mut ui, point, egui::Modifiers::NONE);
    ui.settle();
    assert_range(&ui, 2.0, 3.0);
    assert_eq!(
        ui.app.piano_roll_state.selection_ids,
        HashSet::from([90_003])
    );
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert!(ui.app.undo_stack.is_empty());
}

#[test]
fn piano_range_double_drag_interruption_restores_preview_and_does_not_resume() {
    let mut ui = fixture();
    set_range(&mut ui, 2.0, 3.0);
    ui.app.piano_roll_state.selection_ids = HashSet::from([90_003]);
    let before = project_fingerprint(&ui.app.project);
    let from = ruler(&ui, 0.25);
    let to = ruler(&ui, 1.0);
    first_ruler_click(&mut ui, from, egui::Modifiers::NONE);
    mouse(&mut ui, from, Some(true), egui::Modifiers::NONE);
    mouse(&mut ui, to, None, egui::Modifiers::NONE);
    assert_range(&ui, 0.25, 1.0);
    assert_eq!(
        ui.app.piano_roll_state.selection_ids,
        HashSet::from([90_001, 90_002])
    );
    ui.key(egui::Key::F10, egui::Modifiers::NONE);
    ui.key(egui::Key::F10, egui::Modifiers::NONE);
    assert_range(&ui, 2.0, 3.0);
    assert_eq!(
        ui.app.piano_roll_state.selection_ids,
        HashSet::from([90_003])
    );
    assert!(!piano_range::active(&ui.ctx));
    mouse(
        &mut ui,
        to + Vec2::new(60.0, 0.0),
        None,
        egui::Modifiers::NONE,
    );
    mouse(
        &mut ui,
        to + Vec2::new(60.0, 0.0),
        Some(false),
        egui::Modifiers::NONE,
    );
    ui.settle();
    assert_range(&ui, 2.0, 3.0);
    assert_eq!(
        ui.app.piano_roll_state.selection_ids,
        HashSet::from([90_003])
    );
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert!(ui.app.undo_stack.is_empty());
}

#[test]
fn piano_range_late_or_distant_second_press_does_not_arm_unmodified_drag() {
    for timed_out in [false, true] {
        let mut ui = fixture();
        let first = ruler(&ui, 0.25);
        first_ruler_click(&mut ui, first, egui::Modifiers::NONE);
        let from = if timed_out {
            ui.time += ui
                .ctx
                .options(|options| options.input_options.max_double_click_delay)
                + 0.1;
            first
        } else {
            ruler(&ui, 1.5)
        };
        let to = ruler(&ui, 2.5);
        drag(
            &mut ui,
            from,
            to,
            egui::Modifiers::NONE,
            egui::Modifiers::NONE,
        );
        assert!(
            ui.app.piano_roll_state.repeat_range.is_none(),
            "timeout={timed_out}"
        );
        assert!(ui.app.piano_roll_state.selection_ids.is_empty());
        assert!(ui.app.undo_stack.is_empty());
    }
}

#[test]
fn piano_range_modified_first_clicks_do_not_seed_plain_double_drag() {
    for modifiers in [
        egui::Modifiers::CTRL,
        egui::Modifiers::SHIFT,
        egui::Modifiers::ALT,
        piano_clipboard_command(),
    ] {
        let mut ui = fixture();
        let from = ruler(&ui, 0.25);
        let to = ruler(&ui, 1.0);
        first_ruler_click(&mut ui, from, modifiers);
        drag(
            &mut ui,
            from,
            to,
            egui::Modifiers::NONE,
            egui::Modifiers::NONE,
        );
        assert!(
            ui.app.piano_roll_state.repeat_range.is_none(),
            "first click {modifiers:?}"
        );
        assert!(ui.app.piano_roll_state.selection_ids.is_empty());
        assert!(ui.app.undo_stack.is_empty());
    }
}

#[test]
fn piano_range_other_click_owner_geometry_and_modal_interruptions_disarm_first_click() {
    for interruption in 0..4 {
        let mut ui = fixture();
        // Isolate invalidation from elapsed-time expiry, including an owner/axis round trip.
        ui.ctx
            .options_mut(|options| options.input_options.max_double_click_delay = 10.0);
        let from = ruler(&ui, 0.25);
        first_ruler_click(&mut ui, from, egui::Modifiers::NONE);
        match interruption {
            0 => {
                let point = grid(&ui, 3.0, 62);
                first_ruler_click(&mut ui, point, egui::Modifiers::CTRL);
            }
            1 => {
                let owner = ui.app.selected_channel;
                ui.app.selected_channel = (owner + 1) % ui.app.project.channels.len();
                ui.run(Vec::new());
                ui.app.selected_channel = owner;
                ui.run(Vec::new());
            }
            2 => {
                ui.app.piano_viewport.x.zoom_at_pixel(0.0, 2.0).unwrap();
                ui.run(Vec::new());
                ui.app.piano_viewport.x.zoom_at_pixel(0.0, 0.5).unwrap();
                ui.run(Vec::new());
            }
            _ => {
                ui.key(egui::Key::F10, egui::Modifiers::NONE);
                ui.key(egui::Key::F10, egui::Modifiers::NONE);
            }
        }
        let from = ruler(&ui, 0.25);
        let to = ruler(&ui, 1.0);
        drag(
            &mut ui,
            from,
            to,
            egui::Modifiers::NONE,
            egui::Modifiers::NONE,
        );
        assert!(
            ui.app.piano_roll_state.repeat_range.is_none(),
            "interruption {interruption}"
        );
        assert!(ui.app.undo_stack.is_empty());
    }
}

#[test]
fn piano_range_held_pointer_blocks_alt_transform_keys_without_stale_selection() {
    for (key, kind) in [
        (egui::Key::Q, PianoRollTransformKind::Quantize),
        (egui::Key::U, PianoRollTransformKind::Chop),
        (egui::Key::A, PianoRollTransformKind::Arpeggiate),
    ] {
        let mut ui = fixture();
        let before = project_fingerprint(&ui.app.project);
        let from = ruler(&ui, 0.25);
        let to = ruler(&ui, 1.0);
        mouse(&mut ui, from, Some(true), piano_clipboard_command());
        mouse(&mut ui, to, None, piano_clipboard_command());
        // Ctrl has been released, but the ruler still owns this primary-button gesture.
        mouse(&mut ui, to, None, egui::Modifiers::NONE);
        let selection = ui.app.piano_roll_state.selection_ids.clone();
        ui.key(key, egui::Modifiers::ALT);
        assert!(ui.app.piano_roll_transform.is_none(), "held Alt+{key:?}");
        assert_eq!(ui.app.piano_roll_state.selection_ids, selection);
        assert_range(&ui, 0.25, 1.0);
        assert_eq!(project_fingerprint(&ui.app.project), before);
        mouse(&mut ui, to, Some(false), egui::Modifiers::NONE);
        ui.settle();
        ui.app.piano_roll_state.grouping_enabled = false;
        ui.key(egui::Key::D, piano_clipboard_command());
        let point = body(&ui, 90_002);
        click(&mut ui, point, egui::Modifiers::CTRL);
        assert_eq!(
            ui.app.piano_roll_state.selection_ids,
            HashSet::from([90_002])
        );
        ui.key(key, egui::Modifiers::ALT);
        let transform = ui
            .app
            .piano_roll_transform
            .as_ref()
            .expect("released pointer permits transform");
        assert_eq!(transform.kind, kind);
        assert_eq!(transform.before_selection_ids, HashSet::from([90_002]));
        assert_eq!(transform.target_ids, HashSet::from([90_002]));
        ui.key(egui::Key::Escape, egui::Modifiers::NONE);
        assert!(ui.app.piano_roll_transform.is_none());
        assert_eq!(project_fingerprint(&ui.app.project), before);
        assert!(ui.app.undo_stack.is_empty());
    }
}

#[test]
fn piano_range_held_pointer_does_not_pop_earlier_project_history() {
    let mut ui = fixture();
    let before = project_fingerprint(&ui.app.project);
    ui.key(egui::Key::ArrowRight, egui::Modifiers::SHIFT);
    let moved = project_fingerprint(&ui.app.project);
    assert_ne!(before, moved);
    let from = ruler(&ui, 2.0);
    let to = ruler(&ui, 3.0);
    mouse(&mut ui, from, Some(true), piano_clipboard_command());
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), moved);
    mouse(&mut ui, to, None, piano_clipboard_command());
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), moved);
    mouse(&mut ui, to, Some(false), piano_clipboard_command());
    ui.settle();
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert_range(&ui, 2.0, 3.0);
}
