// Native-reported laptop regression: original divider press ownership must win
// over an overlapping floating edge and a coalesced final position in Piano.

fn sidebar_press_fixture(gap: f32) -> UiHarness {
    let mut ui = UiHarness::floating();
    ui.size = Vec2::new(1180.0, 812.0);
    piano_clipboard_fixture(&mut ui);
    ui.click("Inspector");
    maximize_test_settle(&mut ui);
    let bounds = ui.app.workspace.bounds.unwrap();
    ui.app.workspace.windows[workspace::index(StudioView::PianoRoll)].rect =
        Some(Rect::from_min_size(
            Pos2::new(bounds.width() - 480.0 - gap, 100.0),
            Vec2::new(480.0, 480.0),
        ));
    ui.app.workspace.reset_for_test();
    maximize_test_settle(&mut ui);
    ui.app.piano_roll_state.tool = PianoRollTool::Draw;
    ui
}

fn sidebar_press_drag(mode: u8, inside_sidebar: bool, gap: f32) {
    let mut ui = sidebar_press_fixture(gap);
    if mode == 3 {
        ui.ctx.options_mut(|options| options.max_passes = 2.try_into().unwrap());
        ui.ctx.on_end_pass("sidebar press multi-pass regression", std::sync::Arc::new(|ui| {
            if ui.ctx().current_pass_index() == 0 {
                ui.ctx().request_discard("retain sidebar ownership through layout passes");
            }
        }));
    }
    let divider = ui
        .ctx
        .read_response(Id::new("inspector").with("__resize"))
        .unwrap()
        .rect;
    let grid = ui.ctx.read_response(Id::new("piano-grid")).unwrap().rect;
    let origin = Pos2::new(
        divider.center().x + if inside_sidebar { 1.0 } else { -2.0 },
        grid.center().y,
    );
    let target = origin - Vec2::new(39.0, 0.0);
    assert!(divider.contains(origin));
    assert!(grid.contains(target));
    let music = project_fingerprint(&ui.app.project);
    let window = ui.editor_rect(StudioView::PianoRoll);
    let order = ui.app.workspace.order;
    let undo = ui.app.undo_stack.len();
    let dirty = ui.app.dirty;
    let selection = ui.app.piano_roll_state.selection_ids.clone();
    let before = egui::containers::panel::PanelState::load(&ui.ctx, Id::new("inspector"))
        .unwrap()
        .outer_rect
        .width();
    if mode != 0 {
        let mut events = mixer_pointer_button(origin, true);
        events.push(egui::Event::PointerMoved(target));
        if mode == 1 || mode == 3 {
            events.extend(mixer_pointer_button(target, false));
            let output = ui.run(events);
            if mode == 3 {
                assert_eq!(output.platform_output.num_completed_passes, 2);
            }
        } else {
            ui.run(events);
            ui.run(mixer_pointer_button(target, false));
        }
    } else {
        ui.run(mixer_pointer_button(origin, true));
        for step in 1..=4 {
            ui.run(vec![egui::Event::PointerMoved(
                origin.lerp(target, step as f32 / 4.0),
            )]);
        }
        ui.run(mixer_pointer_button(target, false));
    }
    maximize_test_settle(&mut ui);
    assert_eq!(
        project_fingerprint(&ui.app.project),
        music,
        "divider-origin input must never paint a note"
    );
    assert_eq!(ui.app.undo_stack.len(), undo);
    assert_eq!(ui.app.dirty, dirty);
    assert_eq!(ui.app.piano_roll_state.selection_ids, selection);
    assert!(ui.ctx.dragged_id().is_none(), "release must not leave a phantom native drag");
    assert_eq!(
        ui.app.workspace.order, order,
        "divider must not activate an editor"
    );
    let after = egui::containers::panel::PanelState::load(&ui.ctx, Id::new("inspector"))
        .unwrap()
        .outer_rect
        .width();
    assert!(
        after > before + 30.0,
        "native Inspector must retain the released width: {before} -> {after}"
    );
    let restored = ui.editor_rect(StudioView::PianoRoll);
    assert!(
        (restored.size() - window.size()).length() < 1.0,
        "the floating editor must not acquire the divider resize"
    );
    assert!(
        ui.app
            .workspace
            .bounds
            .unwrap()
            .expand(1.0)
            .contains_rect(restored)
    );
    assert!(!ui.app.editor_pointer_cancellation_active());
}

#[test]
fn sidebar_press_coalesced_inspector_to_piano_cannot_draw() {
    sidebar_press_drag(1, true, 0.0);
}
#[test]
fn sidebar_press_held_inspector_to_piano_cannot_draw() {
    sidebar_press_drag(0, true, 0.0);
}
#[test]
fn sidebar_press_coalesced_overlap_cannot_resize_floating_editor() {
    sidebar_press_drag(1, false, 0.0);
}
#[test]
fn sidebar_press_held_overlap_cannot_resize_floating_editor() {
    sidebar_press_drag(0, false, 0.0);
}

#[test]
fn sidebar_press_partial_coalesced_inspector_to_piano_cannot_draw() {
    sidebar_press_drag(2, true, 0.0);
}
#[test]
fn sidebar_press_partial_coalesced_overlap_cannot_resize_floating_editor() {
    sidebar_press_drag(2, false, 0.0);
}
#[test]
fn sidebar_press_coalesced_inspector_across_gap_cannot_draw() {
    sidebar_press_drag(1, true, 28.0);
}
#[test]
fn sidebar_press_partial_coalesced_inspector_across_gap_cannot_draw() {
    sidebar_press_drag(2, true, 28.0);
}

fn sidebar_body_press_to_piano(control: bool) {
    let mut ui = sidebar_press_fixture(28.0);
    if control {
        ui.app.piano_roll_state.selection_ids.insert(90_001);
        ui.settle();
    }
    let divider = ui.ctx.read_response(Id::new("inspector").with("__resize")).unwrap().rect;
    let grid = ui.ctx.read_response(Id::new("piano-grid")).unwrap().rect;
    let origin = if control {
        node_rect(ui.button("Edit note properties")).center()
    } else {
        Pos2::new(divider.center().x + 18.0, grid.center().y)
    };
    let target = Pos2::new(grid.right() - 20.0, origin.y.clamp(grid.top() + 10.0, grid.bottom() - 10.0));
    let before = project_fingerprint(&ui.app.project);
    let selection = ui.app.piano_roll_state.selection_ids.clone();
    let windows = ui.app.workspace.windows;
    let order = ui.app.workspace.order;
    let mut events = mixer_pointer_button(origin, true);
    events.push(egui::Event::PointerMoved(target));
    ui.run(events);
    ui.run(mixer_pointer_button(target, false));
    ui.settle();
    assert_eq!(project_fingerprint(&ui.app.project), before, "ordinary sidebar press cannot paint in Piano");
    assert_eq!(ui.app.piano_roll_state.selection_ids, selection);
    assert_eq!(ui.app.workspace.windows, windows);
    assert_eq!(ui.app.workspace.order, order);
    assert!(ui.app.undo_stack.is_empty());
    assert!(!ui.app.dirty);
    assert!(ui.app.piano_note_properties.is_none());
}

#[test]
fn sidebar_press_body_partial_coalesced_cannot_paint() {
    sidebar_body_press_to_piano(false);
}
#[test]
fn sidebar_press_control_partial_coalesced_cannot_paint() {
    sidebar_body_press_to_piano(true);
}

#[test]
fn sidebar_press_body_controls_remain_usable() {
    let mut ui = sidebar_press_fixture(0.0);
    ui.app.piano_roll_state.selection_ids.insert(90_001);
    ui.settle();
    ui.click("Edit note properties");
    assert!(ui.app.piano_note_properties.is_some());
}

#[test]
fn sidebar_press_complete_batch_survives_forced_multiple_passes() {
    sidebar_press_drag(3, true, 28.0);
}

fn sidebar_press_interrupted(kind: u8) {
    let mut ui = sidebar_press_fixture(28.0);
    ui.ctx.options_mut(|options| options.max_passes = 2.try_into().unwrap());
    ui.ctx.on_end_pass("sidebar cancellation multi-pass regression", std::sync::Arc::new(|ui| {
        if ui.ctx().current_pass_index() == 0 {
            ui.ctx().request_discard("retain cancelled sidebar ownership through release passes");
        }
    }));
    let origin = ui.ctx.read_response(Id::new("inspector").with("__resize")).unwrap().rect.center();
    let before = project_fingerprint(&ui.app.project);
    ui.run(mixer_pointer_button(origin, true));
    ui.run(vec![egui::Event::PointerMoved(origin - Vec2::new(15.0, 0.0))]);
    assert_eq!(ui.ctx.dragged_id(), Some(Id::new("inspector").with("__resize")));
    match kind {
        0 => ui.key(egui::Key::Escape, egui::Modifiers::NONE),
        1 => {
            ui.app.show_settings = true;
            ui.run(Vec::new());
            ui.app.show_settings = false;
        }
        _ => {
            ui.time += 0.1;
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, ui.size)),
                time: Some(ui.time),
                focused: false,
                events: vec![egui::Event::WindowFocused(false)],
                ..Default::default()
            };
            let _ = ui.ctx.run_ui(input, |root| ui.app.ui(root, &mut ui.frame));
        }
    }
    let width = egui::containers::panel::PanelState::load(&ui.ctx, Id::new("inspector")).unwrap().outer_rect.width();
    let target = origin - Vec2::new(85.0, 0.0);
    ui.run(vec![egui::Event::PointerMoved(target)]);
    ui.run(mixer_pointer_button(target, false));
    ui.settle();
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert!(ui.app.undo_stack.is_empty());
    assert!(!ui.app.dirty);
    assert!(ui.ctx.dragged_id().is_none());
    assert!(!ui.app.editor_pointer_cancellation_active());
    let after = egui::containers::panel::PanelState::load(&ui.ctx, Id::new("inspector")).unwrap().outer_rect.width();
    assert!((after - width).abs() < 1.0, "interrupted sidebar drag cannot resume");
    let grid = ui.ctx.read_response(Id::new("piano-grid")).unwrap().rect;
    ui.click_pos(Pos2::new(grid.right() - 25.0, grid.center().y));
    assert_ne!(project_fingerprint(&ui.app.project), before, "a later fresh Piano press must work");
    assert_eq!(ui.app.undo_stack.len(), 1);
}

#[test]
fn sidebar_press_escape_cancels_until_release() { sidebar_press_interrupted(0); }
#[test]
fn sidebar_press_modal_cancels_until_release() { sidebar_press_interrupted(1); }
#[test]
fn sidebar_press_focus_loss_cancels_until_release() { sidebar_press_interrupted(2); }

#[test]
fn sidebar_press_ambiguous_batch_cannot_borrow_later_endpoint() {
    let mut ui = sidebar_press_fixture(28.0);
    let origin = ui.ctx.read_response(Id::new("inspector").with("__resize")).unwrap().rect.center();
    let grid = ui.ctx.read_response(Id::new("piano-grid")).unwrap().rect;
    let target = Pos2::new(grid.right() - 25.0, grid.center().y);
    let before = project_fingerprint(&ui.app.project);
    let windows = ui.app.workspace.windows;
    let width = egui::containers::panel::PanelState::load(&ui.ctx, Id::new("inspector")).unwrap().outer_rect.width();
    let mut events = mixer_pointer_button(origin, true);
    events.extend(mixer_pointer_button(origin, false));
    events.extend(mixer_pointer_button(target, true));
    events.extend(mixer_pointer_button(target, false));
    ui.run(events);
    ui.settle();
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert_eq!(ui.app.workspace.windows, windows);
    assert!(ui.ctx.dragged_id().is_none());
    assert_eq!(egui::containers::panel::PanelState::load(&ui.ctx, Id::new("inspector")).unwrap().outer_rect.width(), width);
}

fn sidebar_press_browser_to_piano(complete: bool) {
    let mut ui = sidebar_press_fixture(0.0);
    ui.app.workspace.windows[workspace::index(StudioView::PianoRoll)].rect = Some(Rect::from_min_size(Pos2::new(0.0, 100.0), Vec2::new(480.0, 480.0)));
    ui.app.workspace.reset_for_test();
    maximize_test_settle(&mut ui);
    let divider = ui.ctx.read_response(Id::new("browser").with("__resize")).unwrap().rect;
    let grid = ui.ctx.read_response(Id::new("piano-grid")).unwrap().rect;
    let origin = Pos2::new(divider.center().x + 2.0, grid.center().y);
    let target = Pos2::new(grid.left() + 20.0, grid.center().y);
    assert!(divider.contains(origin));
    let before = project_fingerprint(&ui.app.project);
    let window = ui.editor_rect(StudioView::PianoRoll);
    let order = ui.app.workspace.order;
    let width = egui::containers::panel::PanelState::load(&ui.ctx, Id::new("browser")).unwrap().outer_rect.width();
    let mut events = mixer_pointer_button(origin, true);
    events.push(egui::Event::PointerMoved(target));
    if complete {
        events.extend(mixer_pointer_button(target, false));
        ui.run(events);
    } else {
        ui.run(events);
        ui.run(mixer_pointer_button(target, false));
    }
    maximize_test_settle(&mut ui);
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert_eq!(ui.app.workspace.order, order);
    assert!(ui.app.undo_stack.is_empty());
    assert!(!ui.app.dirty);
    assert!(ui.ctx.dragged_id().is_none());
    let after = egui::containers::panel::PanelState::load(&ui.ctx, Id::new("browser")).unwrap().outer_rect.width();
    assert!(after > width + 30.0);
    assert!((ui.editor_rect(StudioView::PianoRoll).size() - window.size()).length() < 1.0);
}

#[test]
fn sidebar_press_browser_complete_batch_owns_overlap() { sidebar_press_browser_to_piano(true); }
#[test]
fn sidebar_press_browser_partial_batch_owns_overlap() { sidebar_press_browser_to_piano(false); }
