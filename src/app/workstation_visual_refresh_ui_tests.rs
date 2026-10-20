// Genuine App input/layout evidence for the flat, compact workstation refresh.
// Optional captures use the production egui/WGPU renderer, never a mockup.

#[test]
fn workstation_refresh_melody_pointer_repeat_undo_and_compact_layout() {
    let mut ui = UiHarness::new();
    ui.app.install_project(Project::blank(), None, false);
    ui.app.project.name = "Melody study".into();
    ui.key(egui::Key::F7, egui::Modifiers::NONE);
    ui.key(egui::Key::P, egui::Modifiers::NONE);
    ui.app.piano_roll_state.last_note_length = 0.5;
    for (index, pitch) in [60_u8, 64, 67, 72, 71, 67, 64, 62].into_iter().enumerate() {
        let grid = ui.ctx.read_response(Id::new("piano-grid")).unwrap().rect;
        let point = Pos2::new(
            grid.left()
                + ui.app
                    .piano_viewport
                    .x
                    .pixel_for_content(index as f64 * 0.5 + 0.05) as f32,
            grid.top()
                + ui.app
                    .piano_viewport
                    .y
                    .pixel_for_content(piano_pitch_row(pitch) + 0.5) as f32,
        );
        assert!(
            grid.contains(point),
            "authored note must be in the real grid"
        );
        ui.click_pos(point);
        assert_eq!(ui.app.project.active_pattern().notes.len(), index + 1);
    }
    let authored = project_fingerprint(&ui.app.project);
    ui.key(egui::Key::A, piano_clipboard_command());
    ui.key(egui::Key::B, piano_clipboard_command());
    assert_eq!(ui.app.project.active_pattern().notes.len(), 16);
    let repeated = project_fingerprint(&ui.app.project);
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), authored);
    ui.key(egui::Key::Y, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), repeated);
    ui.click("FIT NOTES");
    ui.key(egui::Key::D, piano_clipboard_command());
    let note = ui.app.project.active_pattern().notes.last().unwrap().id;
    let point = ui
        .ctx
        .read_response(Id::new(("piano-note", note)))
        .unwrap()
        .rect
        .center();
    ui.click_pos(point);
    let grid = ui.ctx.read_response(Id::new("piano-grid")).unwrap().rect;
    assert!(
        grid.width() >= 900.0,
        "notes should own the horizontal space: {grid:?}"
    );
    assert!(
        grid.top() <= 180.0,
        "permanent workspace chrome must not return: {grid:?}"
    );
    assert!(
        grid.height() >= 600.0,
        "keep vertical score space: {grid:?}"
    );
    ui.capture("workstation-refresh-authored-melody");

    ui.size = Vec2::new(1080.0, 680.0);
    ui.settle();
    ui.click("FIT NOTES");
    for label in ["PIANO", "MIXER", "WINDOWS", "FIT NOTES", "NOTE EDIT v"] {
        let rect = node_rect(ui.button(label));
        assert!(
            ui.ctx.content_rect().contains_rect(rect),
            "minimum window: {label} {rect:?}"
        );
    }
    assert_eq!(project_fingerprint(&ui.app.project), repeated);
    ui.capture("workstation-refresh-melody-minimum");
}

#[test]
fn workstation_refresh_windows_menu_is_real_repeatable_and_modal_safe() {
    let mut ui = UiHarness::floating();
    let original = project_fingerprint(&ui.app.project);
    assert!(node_rect(ui.button("WINDOWS")).bottom() <= 32.0);
    assert!(
        !ui.nodes
            .iter()
            .any(|node| node.label() == Some("WORKSPACE"))
    );
    for _ in 0..2 {
        ui.workspace_action("Cascade windows");
        ui.workspace_action("Arrange windows");
        ui.workspace_action("Maximize editor");
        assert!(ui.app.workspace.maximized);
        ui.workspace_action("Restore windows");
        assert!(!ui.app.workspace.maximized);
    }
    ui.capture("workstation-refresh-multiwindow");
    ui.key(egui::Key::F10, egui::Modifiers::NONE);
    ui.click("WINDOWS");
    let hide = node_rect(ui.button("Hide editor"));
    assert!(ui.button("Hide editor").is_disabled());
    ui.click_pos(hide.center());
    assert!(ui.app.workspace.windows.iter().all(|window| window.visible));
    assert_eq!(project_fingerprint(&ui.app.project), original);
    assert!(ui.app.undo_stack.is_empty());
}

#[test]
fn workstation_refresh_automation_mixer_and_high_dpi_remain_operable() {
    let mut ui = automation_point_ui();
    let original = project_fingerprint(&ui.app.project);
    let middle = automation_point_position(&ui, 2);
    ui.drag_pointer(middle, Vec2::new(16.0, -5.0));
    assert_ne!(project_fingerprint(&ui.app.project), original);
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), original);
    ui.capture("workstation-refresh-automation");
    ui.click("MIXER");
    assert_eq!(ui.app.workspace.focused, StudioView::Mixer);
    ui.capture("workstation-refresh-mixer");
    for scale in [1.0, 1.5, 2.0] {
        ui.ctx.set_zoom_factor(scale);
        ui.size = Vec2::new(1080.0, 680.0);
        ui.settle();
        assert!(
            (ui.ctx.content_rect().size() - ui.size).length() < 0.5,
            "DPI coverage must retain the minimum logical viewport"
        );
        assert!((ui.ctx.pixels_per_point() - scale).abs() < 0.01);
        let toolbar = egui::containers::panel::PanelState::load(&ui.ctx, Id::new("toolbar"))
            .expect("production toolbar at every DPI")
            .outer_rect;
        ui.settle();
        for label in [
            "PLAYLIST",
            "RACK",
            "PIANO",
            "MIXER",
            "PLUGINS manager",
            "WINDOWS",
        ] {
            let rect = node_rect(ui.button(label));
            assert!(
                ui.ctx.content_rect().contains_rect(rect),
                "scale {scale}: {label} {rect:?}"
            );
            assert!(
                rect.height() >= 24.0,
                "retain usable hit targets: {label} {rect:?}"
            );
            if label != "WINDOWS" {
                assert!(
                    toolbar.contains_rect(rect),
                    "scale {scale}: {label} is clipped by {toolbar:?}"
                );
            }
        }
        ui.click("PIANO");
        assert_eq!(ui.app.workspace.focused, StudioView::PianoRoll);
        ui.click("MIXER");
        assert_eq!(ui.app.workspace.focused, StudioView::Mixer);
    }
    ui.capture("workstation-refresh-mixer-high-dpi");
    assert_eq!(project_fingerprint(&ui.app.project), original);
}

#[test]
fn workstation_refresh_transport_and_navigation_fit_the_actual_panel_clip() {
    for width in [1080.0, 1179.0, 1180.0, 1240.0, 1440.0] {
        let mut ui = UiHarness::new();
        ui.size = Vec2::new(width, 680.0);
        ui.settle();
        let toolbar = egui::containers::panel::PanelState::load(&ui.ctx, Id::new("toolbar"))
            .expect("the production transport panel must exist")
            .outer_rect;
        let controls: Vec<_> = [
            "PAT",
            "SONG",
            "Play",
            "Stop",
            "Record",
            "CLICK OFF",
            "PLAYLIST",
            "RACK",
            "PIANO",
            "MIXER",
            "PLUGINS manager",
        ]
        .into_iter()
        .map(|label| (label, node_rect(ui.button(label))))
        .collect();
        for (index, (label, rect)) in controls.iter().enumerate() {
            assert!(
                toolbar.contains_rect(*rect),
                "width {width}: {label} is clipped by toolbar {toolbar:?}: {rect:?}"
            );
            assert!(
                rect.height() >= 24.0,
                "width {width}: {label} hit target shrank: {rect:?}"
            );
            for (_, other) in &controls[index + 1..] {
                assert_disjoint(*rect, *other);
            }
        }
        if width == 1080.0 {
            ui.capture("workstation-refresh-toolbar-minimum-unclipped");
        }
    }
}

fn workstation_tab_to(ui: &mut UiHarness, label: &str) {
    for _ in 0..160 {
        let target = ui
            .nodes
            .iter()
            .find(|node| {
                node.label() == Some(label)
                    && matches!(
                        node.role(),
                        Role::Button | Role::CheckBox | Role::RadioButton | Role::MenuItem
                    )
            })
            .unwrap_or_else(|| panic!("missing keyboard target {label}"));
        let target = node_rect(target);
        if ui
            .ctx
            .memory(|memory| memory.focused())
            .and_then(|id| ui.ctx.read_response(id))
            .is_some_and(|response| response.has_focus() && response.rect == target)
        {
            return;
        }
        ui.key(egui::Key::Tab, egui::Modifiers::NONE);
    }
    panic!("real Tab navigation did not reach {label}");
}

#[test]
fn workstation_refresh_space_has_one_owner_after_real_tab_focus() {
    for label in [
        "Play",
        "Stop",
        "Record",
        "Undo",
        "Redo",
        "PIANO",
        "CLICK OFF",
        "+",
        "Follow Pattern",
    ] {
        let mut ui = UiHarness::new();
        let clip = ui.app.project.clips[0].id;
        ui.app.select_playlist_clip_only(clip);
        ui.settle();
        workstation_tab_to(&mut ui, label);
        let original = project_fingerprint(&ui.app.project);
        let view = ui.app.workspace.focused;
        let metronome = ui.app.audio_preferences.metronome_enabled;
        ui.key(egui::Key::Space, egui::Modifiers::NONE);
        assert!(
            ui.app.playing,
            "{label}: one Space must start transport once"
        );
        assert_eq!(
            project_fingerprint(&ui.app.project),
            original,
            "{label}: Space must not also edit the project"
        );
        assert_eq!(
            ui.app.workspace.focused, view,
            "{label}: Space must not also activate navigation"
        );
        assert_eq!(
            ui.app.audio_preferences.metronome_enabled, metronome,
            "{label}: Space must not toggle metronome"
        );
        assert!(
            !ui.app.recording,
            "{label}: Space must not toggle recording"
        );
        ui.key(egui::Key::Space, egui::Modifiers::NONE);
        assert!(!ui.app.playing, "{label}: the next Space must pause once");
    }
}

#[test]
fn workstation_refresh_enter_and_pointer_keep_native_single_activation() {
    let mut ui = UiHarness::new();
    workstation_tab_to(&mut ui, "Play");
    ui.key(egui::Key::Enter, egui::Modifiers::NONE);
    assert!(ui.app.playing, "Enter activates focused Play once");
    ui.key(egui::Key::Enter, egui::Modifiers::NONE);
    assert!(!ui.app.playing, "Enter activates focused Pause once");
    ui.click("Play");
    assert!(ui.app.playing);
    ui.key(egui::Key::Space, egui::Modifiers::NONE);
    assert!(
        !ui.app.playing,
        "Space after a pointer click still pauses once"
    );
    ui.click("Stop");
    workstation_tab_to(&mut ui, "PIANO");
    ui.key(egui::Key::Enter, egui::Modifiers::NONE);
    assert_eq!(ui.app.workspace.focused, StudioView::PianoRoll);
    assert!(!ui.app.playing, "Enter navigation must not start transport");
}

fn workstation_space_event(pressed: bool, repeat: bool) -> egui::Event {
    egui::Event::Key {
        key: egui::Key::Space,
        physical_key: Some(egui::Key::Space),
        pressed,
        repeat,
        modifiers: egui::Modifiers::NONE,
    }
}

#[test]
fn workstation_refresh_space_repeat_release_and_native_checkbox_are_separate() {
    let mut ui = UiHarness::new();
    let clip = ui.app.project.clips[0].id;
    ui.app.select_playlist_clip_only(clip);
    ui.settle();
    workstation_tab_to(&mut ui, "Follow Pattern");
    let original = project_fingerprint(&ui.app.project);
    // egui derives repeat from its held-key state, so begin with a real keydown.
    ui.run(vec![workstation_space_event(true, false)]);
    assert!(ui.app.playing, "the initial press starts transport once");
    ui.run(vec![workstation_space_event(true, true)]);
    assert!(ui.app.playing, "held-key repeat must not toggle transport");
    ui.run(vec![workstation_space_event(false, false)]);
    assert!(ui.app.playing, "key release must not toggle transport");
    assert_eq!(
        project_fingerprint(&ui.app.project),
        original,
        "repeat/release must not activate the focused native checkbox"
    );
    ui.key(egui::Key::Space, egui::Modifiers::NONE);
    assert!(!ui.app.playing, "a fresh press still pauses once");
    ui.key(egui::Key::Enter, egui::Modifiers::NONE);
    assert_ne!(
        project_fingerprint(&ui.app.project),
        original,
        "Enter must still activate the native checkbox"
    );
    assert!(!ui.app.playing);
}

#[test]
fn workstation_refresh_space_owned_by_popup_or_numeric_text_cannot_leak_on_close() {
    let mut ui = UiHarness::new();
    ui.click("WINDOWS");
    workstation_tab_to(&mut ui, "Hide editor");
    let hidden = ui.app.workspace.focused;
    ui.key(egui::Key::Space, egui::Modifiers::NONE);
    assert!(!ui.app.workspace.windows[workspace::index(hidden)].visible);
    assert!(!ui.ctx.any_popup_open());
    assert!(
        !ui.app.playing,
        "Space which closes a popup must not also play"
    );
    ui.key(egui::Key::F5, egui::Modifiers::NONE);
    let tempo = ui
        .nodes
        .iter()
        .find(|node| node.role() == Role::SpinButton && node.bounds().is_some_and(|b| b.y1 < 100.0))
        .unwrap();
    ui.click_pos(node_rect(tempo).center());
    assert!(ui.ctx.text_edit_focused());
    ui.key(egui::Key::Space, egui::Modifiers::NONE);
    assert!(!ui.app.playing, "numeric text entry owns Space");
    ui.run(vec![
        egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        },
        workstation_space_event(true, false),
    ]);
    assert!(
        !ui.app.playing,
        "closing a numeric editor must not leak its Space"
    );
}

#[test]
fn workstation_refresh_pointer_space_and_new_input_layers_keep_their_owners() {
    let mut ui = UiHarness::new();
    let metronome = node_rect(ui.button("CLICK OFF")).center();
    ui.run(mixer_pointer_button(metronome, true));
    let mut release = mixer_pointer_button(metronome, false);
    release.push(workstation_space_event(true, false));
    ui.run(release);
    ui.run(vec![workstation_space_event(false, false)]);
    assert!(
        ui.app.audio_preferences.metronome_enabled,
        "the distinct pointer click remains native"
    );
    assert!(
        ui.app.playing,
        "the distinct Space starts transport exactly once"
    );
    ui.key(egui::Key::Escape, egui::Modifiers::NONE);
    let windows = node_rect(ui.button("WINDOWS")).center();
    ui.run(mixer_pointer_button(windows, true));
    assert!(
        !egui::Popup::is_any_open(&ui.ctx),
        "WINDOWS must still be closed before its pointer release"
    );
    let mut release = mixer_pointer_button(windows, false);
    release.push(workstation_space_event(true, false));
    ui.run(release);
    assert!(
        egui::Popup::is_any_open(&ui.ctx),
        "the pointer opened the menu"
    );
    assert!(
        !ui.app.playing,
        "a newly opened popup blocks reserved transport"
    );
    ui.settle();
    assert!(ui.ctx.any_popup_open(), "the new menu must actually render");
    assert!(!ui.button("Hide editor").is_disabled());
    ui.key(egui::Key::Escape, egui::Modifiers::NONE);
    ui.key(egui::Key::F10, egui::Modifiers::NONE);
    assert!(ui.app.show_settings);
    let close = node_rect(ui.button("Close window")).center();
    ui.run(mixer_pointer_button(close, true));
    let mut release = mixer_pointer_button(close, false);
    release.push(workstation_space_event(true, false));
    ui.run(release);
    assert!(!ui.app.show_settings);
    assert!(
        !ui.app.playing,
        "closing settings cannot leak the same-frame Space"
    );
}

#[test]
fn workstation_refresh_settings_blocks_space_on_previously_focused_transport() {
    let mut ui = UiHarness::new();
    workstation_tab_to(&mut ui, "Play");
    ui.key(egui::Key::F10, egui::Modifiers::NONE);
    assert!(ui.app.show_settings);
    ui.key(egui::Key::Space, egui::Modifiers::NONE);
    assert!(ui.app.show_settings);
    assert!(
        !ui.app.playing,
        "Settings must prevent stale Play focus from activating transport"
    );
}

#[test]
fn workstation_refresh_accessibility_click_remains_native() {
    let mut ui = UiHarness::new();
    workstation_tab_to(&mut ui, "Play");
    let target = ui
        .ctx
        .memory(|memory| memory.focused())
        .expect("real Tab focus");
    for expected_playing in [true, false] {
        ui.run(vec![egui::Event::AccessKitActionRequest(
            egui::accesskit::ActionRequest {
                action: egui::accesskit::Action::Click,
                target_tree: egui::accesskit::TreeId::ROOT,
                target_node: target.accesskit_id(),
                data: None,
            },
        )]);
        assert_eq!(
            ui.app.playing, expected_playing,
            "accessibility Click activates once"
        );
    }
}

#[test]
fn workstation_refresh_settings_focused_button_still_owns_space() {
    let mut ui = UiHarness::new();
    ui.key(egui::Key::F10, egui::Modifiers::NONE);
    assert!(ui.app.show_settings);
    assert_ne!(ui.app.settings_page, SettingsPage::About);
    workstation_tab_to(&mut ui, "About");
    ui.key(egui::Key::Space, egui::Modifiers::NONE);
    assert_eq!(ui.app.settings_page, SettingsPage::About);
    assert!(ui.app.show_settings);
    assert!(
        !ui.app.playing,
        "Settings button activation must remain local"
    );
}

#[test]
fn workstation_refresh_inspector_precise_values_and_shift_drag_keep_requested_width() {
    fn inspector_numeric_nodes(ui: &UiHarness) -> Vec<&Node> {
        let nodes: std::collections::HashMap<_, _> =
            ui.node_ids.iter().copied().zip(&ui.nodes).collect();
        let mut ancestor = *ui
            .node_ids
            .iter()
            .zip(&ui.nodes)
            .find(|(_, node)| node.role() == Role::Label && node.value() == Some("INSPECTOR"))
            .expect("actual Inspector header in accessibility tree")
            .0;
        loop {
            ancestor = *nodes
                .iter()
                .find(|(_, node)| node.children().contains(&ancestor))
                .expect("Inspector contents must share an ancestor with its header")
                .0;
            let mut descendants = Vec::new();
            let mut pending = vec![ancestor];
            while let Some(id) = pending.pop() {
                let node = nodes[&id];
                pending.extend_from_slice(node.children());
                descendants.push(node);
            }
            assert!(
                !descendants
                    .iter()
                    .any(|node| node.label() == Some("WINDOWS")),
                "Inspector ownership must not fall back to the whole App root"
            );
            let numeric: Vec<_> = descendants
                .into_iter()
                .filter(|node| matches!(node.role(), Role::Slider | Role::SpinButton))
                .collect();
            if !numeric.is_empty() {
                // The title's horizontal row has no numeric controls. Its nearest
                // ancestor with numeric descendants owns the Inspector contents.
                // Membership comes from real UI ancestry, never from coordinates:
                // offscreen Mixer strips are siblings, while an overflowing
                // Inspector control must remain in this set and fail below.
                assert!(
                    numeric.len() >= 4,
                    "volume/pan rails and readouts must exist"
                );
                return numeric;
            }
        }
    }
    fn check_bounds(ui: &UiHarness, width: f32) -> Rect {
        let panel = egui::containers::panel::PanelState::load(&ui.ctx, Id::new("inspector"))
            .expect("real Inspector panel")
            .outer_rect;
        assert!(
            (panel.width() - width).abs() < 1.5,
            "precise readout must not grow panel: {panel:?} / {width}"
        );
        assert!(ui.ctx.content_rect().contains_rect(panel));
        for node in inspector_numeric_nodes(ui) {
            // Lower sections may be vertically scrolled out of view, but
            // no numeric row may widen the horizontal scroll-free panel.
            assert!(
                node_rect(node).left() >= panel.left() && node_rect(node).right() <= panel.right(),
                "numeric control escaped Inspector width: {:?}",
                node_rect(node)
            );
        }
        panel
    }
    for width in [240.0, 340.0] {
        for view in [StudioView::ChannelRack, StudioView::Mixer] {
            let mut ui = UiHarness::new();
            ui.app.selected_channel = 0;
            ui.app.selected_mixer = 1;
            ui.app.project.channels[0].volume = 0.123456;
            ui.app.project.channels[0].pan = -0.123456;
            ui.app.project.mixer_tracks[1].volume = 0.123456;
            ui.app.project.mixer_tracks[1].pan = -0.123456;
            ui.app.focus_editor(view);
            ui.app.sync_history_observer();
            let original = project_fingerprint(&ui.app.project);
            let mut panel =
                egui::containers::panel::PanelState::load(&ui.ctx, Id::new("inspector")).unwrap();
            panel.outer_rect.min.x = panel.outer_rect.max.x - width;
            ui.ctx
                .data_mut(|data| data.insert_persisted(Id::new("inspector"), panel));
            for _ in 0..12 {
                ui.run(Vec::new());
                check_bounds(&ui, width);
            }
            assert_eq!(
                project_fingerprint(&ui.app.project),
                original,
                "layout must not round precise imported values"
            );
            let panel = check_bounds(&ui, width);
            let readout = inspector_numeric_nodes(&ui)
                .into_iter()
                .find(|node| {
                    node.role() == Role::SpinButton
                        && node
                            .bounds()
                            .is_some_and(|_| panel.contains_rect(node_rect(node)))
                        && node
                            .numeric_value()
                            .is_some_and(|value| (value - 0.123456).abs() < 0.000001)
                })
                .expect("actual native volume readout");
            let origin = node_rect(readout).center();
            let shift = egui::Modifiers::SHIFT;
            let button_event = |position: Pos2, pressed| {
                vec![
                    egui::Event::PointerMoved(position),
                    egui::Event::PointerButton {
                        pos: position,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: shift,
                    },
                ]
            };
            ui.run_with_modifiers(button_event(origin, true), shift);
            check_bounds(&ui, width);
            for dx in [4.0, 8.0, 12.0, 16.0] {
                ui.run_with_modifiers(
                    vec![egui::Event::PointerMoved(origin + Vec2::new(dx, 0.0))],
                    shift,
                );
                check_bounds(&ui, width);
            }
            assert!(
                ui.ctx.dragged_id().is_some(),
                "real numeric drag must own the pointer"
            );
            assert_ne!(
                project_fingerprint(&ui.app.project),
                original,
                "Shift movement must reach the native numeric editor"
            );
            for _ in 0..6 {
                ui.run_with_modifiers(Vec::new(), shift);
                check_bounds(&ui, width);
            }
            ui.run_with_modifiers(button_event(origin + Vec2::new(16.0, 0.0), false), shift);
            for _ in 0..6 {
                ui.run(Vec::new());
                check_bounds(&ui, width);
            }
        }
    }
}
