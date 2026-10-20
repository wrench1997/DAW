// Real divider ownership and viewport-interruption regressions in App::ui.

fn sidebar_divider_fixture(maximized: bool, panel: &str) {
    let mut ui = UiHarness::floating();
    ui.app.focus_editor(StudioView::Playlist);
    ui.app.select_playlist_clip_only(9);
    ui.click("Inspector");
    ui.app.workspace.maximized = maximized;
    ui.settle();
    let music = project_fingerprint(&ui.app.project);
    let before_undo = ui.app.undo_stack.len();
    let before_dirty = ui.app.dirty;
    let id = Id::new(panel).with("__resize");
    let origin = ui.ctx.read_response(id).unwrap().rect.center();
    let delta = if panel == "browser" { 500.0 } else { -500.0 };
    ui.run(mixer_pointer_button(origin, true));
    for step in 1..=6 {
        ui.run(vec![egui::Event::PointerMoved(
            origin + Vec2::new(delta * step as f32 / 6.0, 0.0),
        )]);
        assert_eq!(
            ui.ctx.dragged_id(),
            Some(id),
            "the native {panel} divider must keep ownership while held"
        );
        assert!(!ui.app.editor_pointer_cancellation_active());
    }
    ui.run(mixer_pointer_button(origin + Vec2::new(delta, 0.0), false));
    ui.settle();
    let actual = egui::containers::panel::PanelState::load(&ui.ctx, Id::new(panel))
        .unwrap()
        .outer_rect
        .width();
    let expected = if panel == "browser" { 320.0 } else { 340.0 };
    assert!(
        (actual - expected).abs() < 1.0,
        "{panel} must retain released divider size, maximized={maximized}: {actual}"
    );
    let handle = ui.ctx.read_response(id).unwrap().rect.center();
    let shrink = if panel == "browser" { -56.0 } else { 56.0 };
    ui.drag_pointer(handle, Vec2::new(shrink, 0.0));
    let released = egui::containers::panel::PanelState::load(&ui.ctx, Id::new(panel))
        .unwrap()
        .outer_rect
        .width();
    assert!(
        (released - (expected - 56.0)).abs() < 1.0,
        "a second {panel} drag must also save its smaller width: {released}"
    );
    assert_eq!(ui.app.workspace.maximized, maximized);
    assert_eq!(project_fingerprint(&ui.app.project), music);
    assert_eq!(ui.app.undo_stack.len(), before_undo);
    assert_eq!(ui.app.dirty, before_dirty);
}

#[test]
fn workspace_sidebar_floating_browser_drag_persists() {
    sidebar_divider_fixture(false, "browser");
}
#[test]
fn workspace_sidebar_floating_inspector_drag_persists() {
    sidebar_divider_fixture(false, "inspector");
}
#[test]
fn workspace_sidebar_maximized_browser_drag_persists() {
    sidebar_divider_fixture(true, "browser");
}
#[test]
fn workspace_sidebar_maximized_inspector_drag_persists() {
    sidebar_divider_fixture(true, "inspector");
}

#[test]
fn workspace_sidebar_bounds_change_still_cancels_held_note_edit() {
    let mut ui = UiHarness::floating();
    piano_clipboard_fixture(&mut ui);
    let before = project_fingerprint(&ui.app.project);
    let origin = ui
        .ctx
        .read_response(Id::new(("piano-note", 90_001_u64)))
        .unwrap()
        .rect
        .center();
    ui.run(mixer_pointer_button(origin, true));
    ui.run(vec![egui::Event::PointerMoved(
        origin + Vec2::new(40.0, 0.0),
    )]);
    assert!(ui.app.piano_roll_gesture_before.is_some());
    assert_ne!(project_fingerprint(&ui.app.project), before);
    ui.app.show_inspector = true;
    ui.run(Vec::new());
    assert!(ui.app.editor_pointer_cancellation_active());
    assert!(ui.app.piano_roll_gesture_before.is_none());
    let interrupted = project_fingerprint(&ui.app.project);
    for dx in [60.0, 90.0, 130.0] {
        ui.run(vec![egui::Event::PointerMoved(origin + Vec2::new(dx, 0.0))]);
        assert_eq!(project_fingerprint(&ui.app.project), interrupted);
    }
    ui.run(mixer_pointer_button(origin + Vec2::new(130.0, 0.0), false));
    ui.settle();
    assert_eq!(project_fingerprint(&ui.app.project), interrupted);
    assert_eq!(ui.app.undo_stack.len(), 1);
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), before);
}
