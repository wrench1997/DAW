//! Real App::ui input regression for the Inspector below the former 1280 px gate.
use super::*;

fn inspector_is_drawn(ui: &UiHarness) -> bool {
    ui.nodes
        .iter()
        .any(|node| node.value() == Some("INSPECTOR"))
}

fn inspector_panel(ui: &UiHarness) -> Rect {
    assert!(
        inspector_is_drawn(ui),
        "enabled Inspector must actually be visible at {:?}",
        ui.size
    );
    let panel = egui::containers::panel::PanelState::load(&ui.ctx, Id::new("inspector"))
        .expect("actual Inspector panel")
        .outer_rect;
    assert!(Rect::from_min_size(Pos2::ZERO, ui.size).contains_rect(panel));
    panel
}

#[test]
fn narrow_inspector_toggle_and_hide_work_at_minimum_threshold_and_wide_sizes() {
    for size in [
        Vec2::new(1080.0, 680.0),
        Vec2::new(1180.0, 812.0),
        Vec2::new(1279.0, 812.0),
        Vec2::new(1280.0, 812.0),
        Vec2::new(1440.0, 900.0),
        Vec2::new(1920.0, 1080.0),
    ] {
        let mut ui = UiHarness::with_storage(None, false);
        ui.size = size;
        ui.app.focus_editor(StudioView::Playlist);
        ui.app.select_playlist_clip_only(9);
        ui.settle();
        assert!(!ui.app.show_inspector);
        assert!(!inspector_is_drawn(&ui));
        ui.click("Inspector");
        assert!(ui.app.show_inspector);
        let panel = inspector_panel(&ui);
        assert!(panel.contains_rect(node_rect(ui.button("SHAPES"))));
        assert!(panel.contains_rect(node_rect(ui.button("HIDE"))));
        assert!(ui.app.show_browser, "opening Inspector must retain Browser");
        assert!(ui.nodes.iter().any(|node| node.value() == Some("BROWSER")));
        assert!(ui.app.workspace.bounds.unwrap().width() >= 594.0);
        let before = project_fingerprint(&ui.app.project);
        ui.click("SHAPES");
        ui.click("Rise");
        assert_ne!(project_fingerprint(&ui.app.project), before);
        assert!(ui.ctx.content_rect().contains_rect(inspector_panel(&ui)));
        ui.click("HIDE");
        assert!(!ui.app.show_inspector);
        assert!(!inspector_is_drawn(&ui));
        // Repeated top-bar toggles must follow the actual panel state.
        ui.click("Inspector");
        inspector_panel(&ui);
        ui.click("Inspector");
        assert!(!inspector_is_drawn(&ui));
    }
}

#[test]
fn narrow_inspector_visibility_persists_and_view_menu_remains_functional() {
    let mut ui = UiHarness::with_storage(None, false);
    ui.size = Vec2::new(1080.0, 680.0);
    ui.settle();
    for visible in [true, false, true, false] {
        ui.click("VIEW");
        let toggle = ui
            .nodes
            .iter()
            .find(|node| node.role() == Role::CheckBox && node.label() == Some("Inspector"))
            .expect("actual View > Inspector checkbox");
        ui.click_pos(node_rect(toggle).center());
        ui.key(egui::Key::Escape, egui::Modifiers::NONE);
        assert_eq!(ui.app.show_inspector, visible);
        assert_eq!(inspector_is_drawn(&ui), visible);
        let mut storage = WorkspaceTestStorage::default();
        ui.app.save(&mut storage);
        assert_eq!(
            workspace::Workspace::load(Some(&storage)).inspector_visible,
            visible
        );
        let mut restored = UiHarness::with_storage(Some(&storage), false);
        restored.size = ui.size;
        restored.settle();
        assert_eq!(restored.app.show_inspector, visible);
        assert_eq!(inspector_is_drawn(&restored), visible);
    }
}

#[test]
fn narrow_inspector_resizing_preserves_browser_and_editor_budget() {
    let mut ui = UiHarness::floating();
    ui.app.focus_editor(StudioView::Playlist);
    ui.app.select_playlist_clip_only(9);
    ui.click("Inspector");
    let before = project_fingerprint(&ui.app.project);
    // Restore explicitly saved wide sizes. Actual divider gesture behavior has
    // its own workspace regression; this case verifies viewport/panel budgeting.
    for (panel, width) in [("browser", 320.0), ("inspector", 340.0)] {
        let mut state = egui::containers::panel::PanelState::load(&ui.ctx, Id::new(panel)).unwrap();
        if panel == "browser" {
            state.outer_rect.max.x = state.outer_rect.min.x + width;
        } else {
            state.outer_rect.min.x = state.outer_rect.max.x - width;
        }
        ui.ctx
            .data_mut(|data| data.insert_persisted(Id::new(panel), state));
    }
    ui.settle();
    for (panel, expected) in [("browser", 320.0), ("inspector", 340.0)] {
        let width = egui::containers::panel::PanelState::load(&ui.ctx, Id::new(panel))
            .unwrap()
            .outer_rect
            .width();
        assert!((width - expected).abs() < 1.0);
    }
    for width in [1080.0, 1180.0, 1279.0, 1280.0, 1920.0] {
        ui.size = Vec2::new(width, 680.0);
        ui.settle();
        let inspector = inspector_panel(&ui);
        let browser = egui::containers::panel::PanelState::load(&ui.ctx, Id::new("browser"))
            .unwrap()
            .outer_rect;
        assert!(browser.width() >= 168.0);
        assert!(inspector.width() >= 220.0);
        assert!(
            ui.app.workspace.bounds.unwrap().width() >= 594.0,
            "600 px editor panel must survive saved/dragged sidebar widths: browser={browser:?}, inspector={inspector:?}"
        );
        assert!(browser.right() + 600.0 <= inspector.left() + 0.1);
        assert!(inspector.contains_rect(node_rect(ui.button("SHAPES"))));
        ui.click("Browser");
        assert!(!ui.app.show_browser);
        inspector_panel(&ui);
        ui.click("Browser");
        assert!(ui.app.show_browser);
        inspector_panel(&ui);
        assert!(ui.app.workspace.bounds.unwrap().width() >= 594.0);
    }
    assert_eq!(
        project_fingerprint(&ui.app.project),
        before,
        "layout and sidebar visibility must not edit music"
    );
}

fn scroll_inspector_to_combo(ui: &mut UiHarness, value: &str) -> Pos2 {
    for _ in 0..16 {
        let panel = inspector_panel(ui);
        if let Some(node) = ui.nodes.iter().find(|node| {
            node.role() == Role::ComboBox
                && node.value() == Some(value)
                && panel.shrink(2.0).contains_rect(node_rect(node))
        }) {
            assert!(!node.is_disabled());
            return node_rect(node).center();
        }
        ui.run(vec![
            egui::Event::PointerMoved(panel.center()),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                phase: egui::TouchPhase::Move,
                delta: Vec2::new(0.0, -50.0),
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        ui.settle();
    }
    panic!(
        "Inspector control {value:?} must be reachable by scrolling at {:?}",
        ui.size
    );
}

#[test]
fn narrow_inspector_automation_target_and_curve_accept_real_input() {
    for size in [
        Vec2::new(1080.0, 680.0),
        Vec2::new(1180.0, 812.0),
        Vec2::new(1280.0, 812.0),
        Vec2::new(1920.0, 1080.0),
    ] {
        let mut ui = UiHarness::with_storage(None, false);
        ui.size = size;
        ui.app.focus_editor(StudioView::Playlist);
        ui.app.select_playlist_clip_only(9);
        ui.click("Inspector");
        let mixer_ids = ui
            .app
            .project
            .mixer_tracks
            .iter()
            .map(|track| (usize::from(track.runtime_slot), track.id))
            .collect::<Vec<_>>();
        let target_label = automation_target_label_with_slots(
            ui.app.project.automation_lanes[0].lane.target(),
            &mixer_ids,
        );
        let target_pos = scroll_inspector_to_combo(&mut ui, &target_label);
        ui.click_pos(target_pos);
        let choice = node_rect(ui.button("Master · Pan"));
        assert!(ui.ctx.content_rect().contains_rect(choice));
        ui.click("Master · Pan");
        assert_eq!(
            ui.app.project.automation_lanes[0].lane.target(),
            &AutomationTarget::MasterPan
        );
        let curve_pos = scroll_inspector_to_combo(&mut ui, "Tension");
        ui.click_pos(curve_pos);
        let choice = node_rect(ui.button("Linear"));
        assert!(ui.ctx.content_rect().contains_rect(choice));
        ui.click("Linear");
        assert_eq!(
            ui.app.project.automation_lanes[0].lane.curve(),
            AutomationCurve::Linear
        );
        assert!(ui.app.show_browser);
        ui.click("HIDE");
        assert!(!inspector_is_drawn(&ui));
    }
}
