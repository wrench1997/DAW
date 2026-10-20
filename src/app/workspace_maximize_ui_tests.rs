// Actual production App::ui pointer/accessibility regressions. These checks do
// not claim native desktop, audio-device, or platform acceptance.

fn maximize_test_settle(ui: &mut UiHarness) {
    for _ in 0..8 {
        ui.run(Vec::new());
    }
}

fn maximize_test_double_click(ui: &mut UiHarness, pos: Pos2) {
    for pressed in [true, false, true, false] {
        ui.run(mixer_pointer_button(pos, pressed));
    }
    maximize_test_settle(ui);
}

fn maximize_test_title(ui: &UiHarness, view: StudioView) -> Pos2 {
    let rect = ui
        .ctx
        .read_response(workspace::window_id(view).with("__title_click"))
        .unwrap()
        .rect;
    Pos2::new(rect.left() + 85.0, rect.center().y)
}

#[test]
fn direct_editor_maximize_buttons_restore_geometry_order_and_music_at_laptop_size() {
    let mut ui = UiHarness::floating();
    ui.size = Vec2::new(1180.0, 812.0);
    maximize_test_settle(&mut ui);
    let music = project_fingerprint(&ui.app.project);
    let selection = (
        ui.app.selected_channel,
        ui.app.selected_clip,
        ui.app.selected_mixer,
    );
    for (view, key, name) in [
        (StudioView::Playlist, egui::Key::F5, "Playlist"),
        (StudioView::ChannelRack, egui::Key::F6, "Channel Rack"),
        (StudioView::Mixer, egui::Key::F9, "Mixer"),
        (StudioView::PianoRoll, egui::Key::F7, "Piano Roll"),
    ] {
        ui.key(key, egui::Modifiers::NONE);
        maximize_test_settle(&mut ui);
        for _ in 0..2 {
            let windows = ui.app.workspace.windows;
            let order = ui.app.workspace.order;
            let label = format!("Maximize {name}");
            let button = node_rect(ui.button(&label));
            let title = ui
                .ctx
                .read_response(workspace::window_id(view).with("__title_click"))
                .unwrap()
                .rect;
            assert!(
                title.contains_rect(button),
                "maximize must be on the native title bar"
            );
            ui.click(&label);
            assert!(ui.app.workspace.maximized);
            assert_eq!(ui.app.workspace.focused, view);
            assert_eq!(ui.app.workspace.windows, windows);
            ui.click("Restore");
            maximize_test_settle(&mut ui);
            assert!(!ui.app.workspace.maximized);
            assert_eq!(ui.app.workspace.windows, windows);
            assert_eq!(ui.app.workspace.order, order);
            let actual_order: Vec<_> = ui.ctx.memory(|memory| {
                memory
                    .layer_ids()
                    .filter_map(|layer| {
                        workspace::EDITORS
                            .into_iter()
                            .find(|view| layer.id == workspace::window_id(*view))
                    })
                    .collect()
            });
            assert_eq!(actual_order, order);
            assert_eq!(project_fingerprint(&ui.app.project), music);
            assert_eq!(
                (
                    ui.app.selected_channel,
                    ui.app.selected_clip,
                    ui.app.selected_mixer
                ),
                selection
            );
            assert!(ui.app.undo_stack.is_empty());
        }
    }
    ui.capture("direct-editor-maximize-floating");
    ui.click("Maximize Piano Roll");
    ui.capture("direct-editor-maximize-piano");
}

#[test]
fn direct_editor_maximize_native_double_click_and_restore_title_are_view_only() {
    let mut ui = UiHarness::floating();
    ui.size = Vec2::new(1180.0, 812.0);
    piano_clipboard_fixture(&mut ui);
    maximize_test_settle(&mut ui);
    let music = project_fingerprint(&ui.app.project);
    let before = ui.app.workspace.windows;
    let pos = maximize_test_title(&ui, StudioView::PianoRoll);
    ui.run(mixer_pointer_button(pos, true));
    ui.run(mixer_pointer_button(pos, false));
    assert!(
        !ui.app.workspace.maximized,
        "single title click only focuses"
    );
    ui.run(mixer_pointer_button(pos, true));
    assert!(
        !ui.app.workspace.maximized,
        "the second held press must not change layouts"
    );
    ui.run(mixer_pointer_button(pos, false));
    maximize_test_settle(&mut ui);
    assert!(
        ui.app.workspace.maximized,
        "native title double click must maximize"
    );
    let title = ui
        .ctx
        .read_response(workspace::window_id(StudioView::PianoRoll).with("maximized-title"))
        .unwrap()
        .rect;
    maximize_test_double_click(&mut ui, title.center());
    assert!(!ui.app.workspace.maximized);
    assert_eq!(ui.app.workspace.windows, before);
    assert_eq!(project_fingerprint(&ui.app.project), music);
    assert!(ui.app.undo_stack.is_empty());
    assert!(!ui.app.dirty);
}

#[test]
fn direct_editor_maximize_press_then_drag_does_not_move_window_or_edit_notes() {
    let mut ui = UiHarness::floating();
    piano_clipboard_fixture(&mut ui);
    ui.app.piano_roll_state.tool = PianoRollTool::Draw;
    maximize_test_settle(&mut ui);
    let music = project_fingerprint(&ui.app.project);
    let window = ui.editor_rect(StudioView::PianoRoll);
    let button = node_rect(ui.button("Maximize Piano Roll")).center();
    let grid = ui
        .ctx
        .read_response(Id::new("piano-grid"))
        .unwrap()
        .rect
        .center();
    ui.run(mixer_pointer_button(button, true));
    for step in 1..=6 {
        ui.run(vec![egui::Event::PointerMoved(
            button.lerp(grid, step as f32 / 6.0),
        )]);
        assert!(!ui.app.workspace.maximized);
    }
    ui.run(mixer_pointer_button(grid, false));
    maximize_test_settle(&mut ui);
    assert!(!ui.app.workspace.maximized);
    assert_eq!(ui.editor_rect(StudioView::PianoRoll), window);
    assert_eq!(project_fingerprint(&ui.app.project), music);
    assert!(ui.app.undo_stack.is_empty());
    assert!(!ui.app.dirty);
    // A fresh click still works after an abandoned button press.
    ui.click("Maximize Piano Roll");
    assert!(ui.app.workspace.maximized);
    ui.click("Restore");
    assert!(!ui.app.workspace.maximized);
}

#[test]
fn direct_editor_maximize_second_title_press_can_still_be_a_native_drag() {
    let mut ui = UiHarness::floating();
    ui.arrange_test_windows(
        &[(
            StudioView::PianoRoll,
            Rect::from_min_size(Pos2::new(100.0, 80.0), Vec2::new(680.0, 480.0)),
        )],
        StudioView::PianoRoll,
    );
    let music = project_fingerprint(&ui.app.project);
    let before = ui.editor_rect(StudioView::PianoRoll);
    let pos = maximize_test_title(&ui, StudioView::PianoRoll);
    ui.run(mixer_pointer_button(pos, true));
    ui.run(mixer_pointer_button(pos, false));
    ui.run(mixer_pointer_button(pos, true));
    for step in 1..=6 {
        ui.run(vec![egui::Event::PointerMoved(
            pos + Vec2::new(step as f32 * 8.0, 0.0),
        )]);
        assert!(!ui.app.workspace.maximized);
    }
    ui.run(mixer_pointer_button(pos + Vec2::new(48.0, 0.0), false));
    maximize_test_settle(&mut ui);
    assert!(!ui.app.workspace.maximized);
    let after = ui.editor_rect(StudioView::PianoRoll);
    assert!((after.left() - before.left() - 48.0).abs() < 1.5);
    assert_eq!(after.size(), before.size());
    assert_eq!(project_fingerprint(&ui.app.project), music);
    assert!(ui.app.undo_stack.is_empty());
}

#[test]
fn direct_editor_maximize_focus_switch_and_modal_barrier_preserve_layout() {
    let mut ui = UiHarness::floating();
    ui.workspace_action("Cascade windows");
    ui.key(egui::Key::F7, egui::Modifiers::NONE);
    let music = project_fingerprint(&ui.app.project);
    let windows = ui.app.workspace.windows;
    let button = node_rect(ui.button("Maximize Piano Roll")).center();
    ui.key(egui::Key::F10, egui::Modifiers::NONE);
    ui.click_pos(button);
    assert!(!ui.app.workspace.maximized);
    assert!(ui.app.show_settings);
    ui.key(egui::Key::Escape, egui::Modifiers::NONE);
    ui.click("Maximize Piano Roll");
    assert!(ui.app.workspace.maximized);
    ui.key(egui::Key::F5, egui::Modifiers::NONE);
    assert!(ui.app.workspace.maximized);
    assert_eq!(ui.app.workspace.focused, StudioView::Playlist);
    ui.click("Restore");
    maximize_test_settle(&mut ui);
    assert!(!ui.app.workspace.maximized);
    assert_eq!(ui.app.workspace.windows, windows);
    assert_eq!(ui.app.workspace.order.last(), Some(&StudioView::Playlist));
    assert_eq!(project_fingerprint(&ui.app.project), music);
    assert!(ui.app.undo_stack.is_empty());
}

#[test]
fn direct_editor_maximize_unfocused_button_activates_only_its_editor() {
    let mut ui = UiHarness::floating();
    maximize_test_settle(&mut ui);
    assert_eq!(ui.app.workspace.focused, StudioView::PianoRoll);
    let windows = ui.app.workspace.windows;
    let order = ui.app.workspace.order;
    let music = project_fingerprint(&ui.app.project);
    let button = node_rect(ui.button("Maximize Mixer")).center();
    assert_eq!(
        ui.ctx.layer_id_at(button).unwrap().id,
        workspace::window_id(StudioView::Mixer)
    );
    ui.click_pos(button);
    assert!(ui.app.workspace.maximized);
    assert_eq!(ui.app.workspace.focused, StudioView::Mixer);
    ui.click("Restore");
    maximize_test_settle(&mut ui);
    let others = |order: [StudioView; 4]| -> Vec<_> {
        order
            .into_iter()
            .filter(|view| *view != StudioView::Mixer)
            .collect()
    };
    assert_eq!(others(ui.app.workspace.order), others(order));
    assert_eq!(ui.app.workspace.order.last(), Some(&StudioView::Mixer));
    assert_eq!(ui.app.workspace.windows, windows);
    assert_eq!(project_fingerprint(&ui.app.project), music);
    assert!(ui.app.undo_stack.is_empty());
}
