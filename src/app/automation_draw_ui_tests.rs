//! Draw-mode automation handles use real App pointer/key input and project history.
use super::*;

fn json(project: &Project) -> serde_json::Value {
    serde_json::to_value(project).unwrap()
}

fn saved_baseline(ui: &mut UiHarness) {
    ui.app.sync_history_observer();
    ui.app.project_fingerprint = project_fingerprint(&ui.app.project);
    ui.app.undo_stack.clear();
    ui.app.redo_stack.clear();
    ui.app.dirty = false;
    ui.settle();
}

fn draw_ui() -> UiHarness {
    let mut ui = automation_tension_ui();
    ui.key(egui::Key::P, egui::Modifiers::NONE);
    assert_eq!(ui.app.tool_mode, ToolMode::Draw);
    saved_baseline(&mut ui);
    ui
}

fn pointer(ui: &mut UiHarness, pos: Pos2, button: egui::PointerButton, pressed: Option<bool>) {
    // Force deadline-crossing gestures to detect timed-history fragmentation.
    ui.app.last_history_check = Instant::now() - Duration::from_secs(2);
    ui.app.last_history_capture = Instant::now() - Duration::from_secs(2);
    let mut events = vec![egui::Event::PointerMoved(pos)];
    if let Some(pressed) = pressed {
        events.push(egui::Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
    }
    ui.run(events);
}

fn drag(ui: &mut UiHarness, start: Pos2, end: Pos2) {
    pointer(ui, start, egui::PointerButton::Primary, Some(true));
    pointer(ui, end, egui::PointerButton::Primary, None);
    pointer(ui, end, egui::PointerButton::Primary, None);
    pointer(ui, end, egui::PointerButton::Primary, Some(false));
    ui.settle();
}

fn undo_redo(ui: &mut UiHarness, before: &serde_json::Value, after: &serde_json::Value) {
    assert_eq!(ui.app.undo_stack.len(), 1);
    assert!(ui.app.playlist_gesture_before.is_none());
    assert!(!ui.app.automation_point_drag_active);
    assert!(
        ui.ctx.dragged_id().is_none(),
        "completed gestures cannot leave a phantom native drag"
    );
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(&json(&ui.app.project), before);
    assert!(ui.app.undo_stack.is_empty());
    ui.key(egui::Key::Y, piano_clipboard_command());
    assert_eq!(&json(&ui.app.project), after);
}

#[test]
fn automation_draw_new_clip_sine_nodes_edit_without_switching_tools() {
    let mut ui = UiHarness::new();
    assert!(ui.app.install_project(Project::blank(), None, false));
    assert_eq!(ui.app.tool_mode, ToolMode::Draw, "fresh App default");
    ui.click("+ AUTOMATION");
    ui.click("SHAPES");
    ui.click("Sine");
    assert_eq!(ui.app.tool_mode, ToolMode::Draw);
    assert_eq!(ui.app.project.automation_lanes[0].lane.points().len(), 33);
    saved_baseline(&mut ui);
    let before = json(&ui.app.project);
    let clip = ui.app.project.clips[0].clone();
    let original_points = ui.app.project.automation_lanes[0].lane.points().to_vec();
    // Address the actual painted/accessibility peak, not a direct model operation.
    let peak = automation_point_position(&ui, 9);
    drag(&mut ui, peak, peak + Vec2::new(0.0, 10.0));
    assert_ne!(
        ui.app.project.automation_lanes[0].lane.points(),
        original_points
    );
    assert_eq!(
        serde_json::to_value(&ui.app.project.clips[0]).unwrap(),
        serde_json::to_value(clip).unwrap()
    );
    assert_eq!(
        ui.app.project.clips.len(),
        1,
        "no Pattern is painted under a handle"
    );
    let edited = json(&ui.app.project);
    undo_redo(&mut ui, &before, &edited);
}

#[test]
fn automation_draw_point_and_tension_drags_have_one_exact_transaction() {
    for tension in [false, true] {
        let mut ui = draw_ui();
        let before = json(&ui.app.project);
        let original = ui.app.project.clone();
        let start = if tension {
            automation_tension_position(&ui, 1).expect("Draw exposes tension handles")
        } else {
            automation_point_position(&ui, 2)
        };
        drag(&mut ui, start, start + Vec2::new(0.0, 8.0));
        let mut expected = original;
        expected.automation_lanes = ui.app.project.automation_lanes.clone();
        assert_eq!(
            json(&ui.app.project),
            json(&expected),
            "only curve data changes"
        );
        assert_ne!(json(&ui.app.project), before);
        if tension {
            assert!(
                (ui.app.project.automation_lanes[0].lane.points()[0].tension - 0.08).abs() < 1e-6
            );
            let handle = automation_tension_position(&ui, 1).unwrap();
            // A reset is an additional explicit, independently undoable edit.
            let edited = json(&ui.app.project);
            undo_redo(&mut ui, &before, &edited);
            automation_secondary_click(&mut ui, handle);
            assert_eq!(json(&ui.app.project), before);
            assert_eq!(ui.app.undo_stack.len(), 2);
        } else {
            assert_ne!(
                ui.app.project.automation_lanes[0].lane.points()[1].value,
                0.0
            );
            let edited = json(&ui.app.project);
            undo_redo(&mut ui, &before, &edited);
        }
    }
}

#[test]
fn automation_draw_right_click_menu_cancel_and_blank_click_leave_music_unchanged() {
    let mut ui = draw_ui();
    let before = json(&ui.app.project);
    let pos = automation_point_position(&ui, 2);
    automation_secondary_click(&mut ui, pos);
    ui.button("Copy value");
    ui.button("Delete point");
    ui.click("Cancel");
    assert!(!ui.ctx.any_popup_open());
    let blank = automation_blank_position(&ui);
    ui.click_pos(blank);
    assert_eq!(json(&ui.app.project), before);
    assert!(!ui.app.dirty);
    assert!(ui.app.undo_stack.is_empty());
    // Existing secondary-click insertion on the curve body stays available.
    automation_secondary_click(&mut ui, blank);
    assert_eq!(ui.app.project.automation_lanes[0].lane.points().len(), 4);
    assert_eq!(ui.app.project.clips.len(), 1);
    let inserted = json(&ui.app.project);
    undo_redo(&mut ui, &before, &inserted);
}

#[test]
fn automation_draw_secondary_drag_cannot_edit_a_point() {
    let mut ui = draw_ui();
    let before = json(&ui.app.project);
    let pos = automation_point_position(&ui, 2);
    pointer(&mut ui, pos, egui::PointerButton::Secondary, Some(true));
    let end = pos + Vec2::new(0.0, 10.0);
    pointer(&mut ui, end, egui::PointerButton::Secondary, None);
    pointer(&mut ui, end, egui::PointerButton::Secondary, Some(false));
    ui.settle();
    assert_eq!(json(&ui.app.project), before);
    assert!(!ui.app.dirty);
    assert!(ui.app.undo_stack.is_empty());
    assert!(ui.app.playlist_gesture_before.is_none());
}

#[test]
fn automation_draw_escape_cancel_preserves_redo_then_outside_release_commits_once() {
    for tension in [false, true] {
        let mut ui = draw_ui();
        let before = json(&ui.app.project);
        let mut future = ui.app.project.clone();
        future.swing = 0.2;
        let redo = json(&future);
        ui.app.redo_stack.push(future);
        let start = if tension {
            automation_tension_position(&ui, 1).unwrap()
        } else {
            automation_point_position(&ui, 2)
        };
        pointer(&mut ui, start, egui::PointerButton::Primary, Some(true));
        let end = start + Vec2::new(0.0, 8.0);
        pointer(&mut ui, end, egui::PointerButton::Primary, None);
        assert_ne!(json(&ui.app.project), before);
        ui.key(egui::Key::Escape, egui::Modifiers::NONE);
        pointer(&mut ui, end, egui::PointerButton::Primary, Some(false));
        ui.settle();
        assert_eq!(json(&ui.app.project), before);
        assert!(!ui.app.dirty);
        assert!(ui.app.undo_stack.is_empty());
        assert_eq!(ui.app.redo_stack.len(), 1);
        assert_eq!(json(&ui.app.redo_stack[0]), redo);
        pointer(&mut ui, start, egui::PointerButton::Primary, Some(true));
        pointer(&mut ui, end, egui::PointerButton::Primary, None);
        pointer(
            &mut ui,
            Pos2::new(-20.0, end.y),
            egui::PointerButton::Primary,
            Some(false),
        );
        ui.settle();
        let edited = json(&ui.app.project);
        assert_ne!(edited, before);
        undo_redo(&mut ui, &before, &edited);
    }
}

#[test]
fn automation_draw_new_project_cancel_retires_held_point_and_tension_drag() {
    for tension in [false, true] {
        let mut ui = draw_ui();
        let before = json(&ui.app.project);
        let start = if tension {
            automation_tension_position(&ui, 1).unwrap()
        } else {
            automation_point_position(&ui, 2)
        };
        pointer(&mut ui, start, egui::PointerButton::Primary, Some(true));
        let end = start + Vec2::new(0.0, 8.0);
        pointer(&mut ui, end, egui::PointerButton::Primary, None);
        assert_ne!(json(&ui.app.project), before);
        ui.key(egui::Key::N, piano_clipboard_command());
        assert!(ui.app.project_lifecycle.is_modal());
        assert!(ui.app.playlist_gesture_before.is_none());
        let edited = json(&ui.app.project);
        ui.key(egui::Key::Escape, egui::Modifiers::NONE);
        assert!(ui.app.project_lifecycle.is_idle());
        pointer(
            &mut ui,
            end + Vec2::new(30.0, 8.0),
            egui::PointerButton::Primary,
            None,
        );
        pointer(
            &mut ui,
            end + Vec2::new(30.0, 8.0),
            egui::PointerButton::Primary,
            Some(false),
        );
        ui.settle();
        assert_eq!(json(&ui.app.project), edited);
        undo_redo(&mut ui, &before, &edited);
    }
}

#[test]
fn automation_draw_clip_header_move_and_resize_keep_curve_source_unchanged() {
    for resize in [false, true] {
        let mut ui = draw_ui();
        let before = json(&ui.app.project);
        let lanes = serde_json::to_value(&ui.app.project.automation_lanes).unwrap();
        let clip_id = ui.app.project.clips[0].id;
        let response_id = Id::new(("playlist-clip", clip_id));
        let rect = ui
            .ctx
            .read_response(if resize {
                response_id.with("resize")
            } else {
                response_id
            })
            .unwrap()
            .rect;
        // The header corner belongs to resize; the curve endpoint below it is a node.
        let start = Pos2::new(rect.center().x, rect.top() + 3.0);
        let beat_w = ui.app.playlist_viewport.x.pixels_per_unit() as f32;
        drag(&mut ui, start, start + Vec2::new(beat_w, 0.0));
        if resize {
            assert_eq!(ui.app.project.clips[0].length, 5.0);
            assert_eq!(ui.app.project.clips[0].start, 0.0);
        } else {
            assert_eq!(ui.app.project.clips[0].start, 1.0);
            assert_eq!(ui.app.project.clips[0].length, 4.0);
        }
        assert_eq!(
            serde_json::to_value(&ui.app.project.automation_lanes).unwrap(),
            lanes
        );
        assert_eq!(ui.app.selected_clip, Some(clip_id));
        let edited = json(&ui.app.project);
        undo_redo(&mut ui, &before, &edited);
    }
}

#[test]
fn automation_draw_handles_do_not_steal_other_playlist_tools_or_selection() {
    for (key, tool) in [
        (egui::Key::D, ToolMode::Delete),
        (egui::Key::C, ToolMode::Slice),
        (egui::Key::T, ToolMode::Mute),
        (egui::Key::S, ToolMode::Slip),
        (egui::Key::B, ToolMode::Paint),
    ] {
        let mut ui = draw_ui();
        let point = automation_point_position(&ui, 2);
        let before = json(&ui.app.project);
        let lanes = serde_json::to_value(&ui.app.project.automation_lanes).unwrap();
        ui.key(key, egui::Modifiers::NONE);
        assert_eq!(ui.app.tool_mode, tool);
        assert!(!ui.nodes.iter().any(|node| {
            node.label().is_some_and(|label| {
                label.starts_with("Automation point ") || label.starts_with("Automation tension ")
            })
        }));
        assert_eq!(
            json(&ui.app.project),
            before,
            "tool changes alone cannot edit music"
        );
        ui.click_pos(point);
        match tool {
            ToolMode::Delete => assert!(ui.app.project.clips.is_empty()),
            ToolMode::Slice => assert_eq!(ui.app.project.clips.len(), 2),
            ToolMode::Mute => assert!(ui.app.project.clips[0].muted),
            ToolMode::Slip | ToolMode::Paint => assert_eq!(json(&ui.app.project), before),
            _ => unreachable!(),
        }
        if tool != ToolMode::Delete {
            assert_eq!(
                serde_json::to_value(&ui.app.project.automation_lanes).unwrap(),
                lanes
            );
        }
    }
    let mut ui = draw_ui();
    let before = json(&ui.app.project);
    let point = automation_point_position(&ui, 2);
    ui.app.clear_playlist_selection();
    ui.settle();
    assert!(
        !ui.nodes
            .iter()
            .any(|node| node.label() == Some("Automation point 2"))
    );
    ui.click_pos(point);
    assert_eq!(ui.app.selected_clip, Some(ui.app.project.clips[0].id));
    assert_eq!(
        json(&ui.app.project),
        before,
        "first click selects the clip"
    );
    ui.key(egui::Key::E, egui::Modifiers::NONE);
    let point = automation_point_position(&ui, 2);
    drag(&mut ui, point, point + Vec2::new(0.0, 8.0));
    assert_ne!(
        json(&ui.app.project),
        before,
        "Select retains direct point editing"
    );
}

#[test]
fn automation_draw_point_and_tension_keep_coalesced_motion_and_release() {
    for one_frame in [false, true] {
        for tension in [false, true] {
            let mut ui = draw_ui();
            let before = json(&ui.app.project);
            let start = if tension {
                automation_tension_position(&ui, 1).unwrap()
            } else {
                automation_point_position(&ui, 2)
            };
            let end = start + Vec2::new(0.0, 8.0);
            let press = egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            };
            let mut events = if one_frame {
                vec![egui::Event::PointerMoved(start), press]
            } else {
                ui.run(vec![egui::Event::PointerMoved(start), press]);
                Vec::new()
            };
            events.extend([
                egui::Event::PointerMoved(end),
                egui::Event::PointerButton {
                    pos: end,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
            ui.run(events);
            ui.settle();
            let edited = json(&ui.app.project);
            assert_ne!(edited, before, "coalesced={one_frame}, tension={tension}");
            assert_eq!(ui.app.project.clips.len(), 1);
            undo_redo(&mut ui, &before, &edited);
        }
    }
}

fn batched_drag(ui: &mut UiHarness, start: Pos2, end: Pos2) -> egui::FullOutput {
    ui.run(vec![
        egui::Event::PointerMoved(start),
        egui::Event::PointerButton {
            pos: start,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        },
        egui::Event::PointerMoved(start.lerp(end, 0.25)),
        egui::Event::PointerMoved(start.lerp(end, 0.5)),
        egui::Event::PointerMoved(start.lerp(end, 0.75)),
        egui::Event::PointerMoved(end),
        egui::Event::PointerButton {
            pos: end,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        },
    ])
}

#[test]
fn automation_draw_dense_sine_peak_tracks_fast_drag_at_one_bar_zoom() {
    let mut ui = UiHarness::new();
    assert!(ui.app.install_project(Project::blank(), None, false));
    ui.click("+ AUTOMATION");
    ui.click("SHAPES");
    ui.click("Sine");
    ui.click("1 BAR");
    saved_baseline(&mut ui);
    let before = json(&ui.app.project);
    let original = ui.app.project.automation_lanes[0].lane.points().to_vec();
    let peak_index = original
        .iter()
        .position(|point| point.value == 1.0)
        .expect("Sine has a full-depth peak");
    let peak = automation_point_position(&ui, peak_index + 1);
    batched_drag(&mut ui, peak, peak + Vec2::new(0.0, 10.0));
    ui.settle();
    let points = ui.app.project.automation_lanes[0].lane.points();
    assert!(
        points[peak_index].value < 0.9,
        "the exact peak changes, not a nearby node"
    );
    for (index, (actual, expected)) in points.iter().zip(&original).enumerate() {
        if index != peak_index {
            assert_eq!(actual, expected, "unchanged neighboring point {index}");
        }
    }
    let edited = json(&ui.app.project);
    undo_redo(&mut ui, &before, &edited);
}

#[test]
fn automation_draw_batched_drag_has_one_owner_and_respects_covering_clip() {
    let mut ui = draw_ui();
    let original = ui.app.project.automation_lanes[0].lane.points().to_vec();
    let start = automation_point_position(&ui, 2);
    let end = automation_point_position(&ui, 3);
    batched_drag(&mut ui, start, end);
    ui.settle();
    let points = ui.app.project.automation_lanes[0].lane.points();
    assert_ne!(points[1], original[1]);
    assert_eq!(points[0], original[0]);
    assert_eq!(
        points[2], original[2],
        "release over another node cannot begin another edit"
    );
    assert_eq!(ui.app.undo_stack.len(), 1);

    let mut ui = draw_ui();
    let start = automation_point_position(&ui, 2);
    let mut cover = ui.app.project.clips[0].clone();
    cover.id = 2;
    cover.kind = ClipKind::Pattern;
    cover.automation_id = None;
    cover.pattern_length_policy = Some(PatternLengthPolicy::Manual);
    ui.app.project.clips.push(cover);
    saved_baseline(&mut ui);
    let before = json(&ui.app.project);
    batched_drag(&mut ui, start, start + Vec2::new(0.0, 8.0));
    ui.settle();
    assert_eq!(
        json(&ui.app.project),
        before,
        "frontmost Pattern owns the covered point"
    );
    assert!(ui.app.undo_stack.is_empty());
}

#[test]
fn automation_draw_batched_drag_is_applied_once_across_layout_passes() {
    for tension in [false, true] {
        let mut ui = draw_ui();
        let before = json(&ui.app.project);
        let start = if tension {
            automation_tension_position(&ui, 1).unwrap()
        } else {
            automation_point_position(&ui, 2)
        };
        ui.ctx
            .options_mut(|options| options.max_passes = 2.try_into().unwrap());
        ui.ctx.on_end_pass(
            "Automation pointer multi-pass regression",
            std::sync::Arc::new(|ui| {
                if ui.ctx().current_pass_index() == 0 {
                    ui.ctx()
                        .request_discard("repeat input without repeating curve edits");
                }
            }),
        );
        let output = batched_drag(&mut ui, start, start + Vec2::new(0.0, 8.0));
        assert_eq!(output.platform_output.num_completed_passes, 2);
        ui.settle();
        if tension {
            assert!(
                (ui.app.project.automation_lanes[0].lane.points()[0].tension - 0.08).abs() < 1e-6
            );
        } else {
            assert!((ui.app.project.automation_lanes[0].lane.points()[1].value + 0.8).abs() < 1e-6);
        }
        let edited = json(&ui.app.project);
        assert_ne!(edited, before);
        undo_redo(&mut ui, &before, &edited);
    }
}

#[test]
fn automation_draw_held_save_retires_pointer_without_rewriting_saved_curve() {
    for tension in [false, true] {
        let fixture = Fixture::new();
        let path = fixture.0.join("held-automation-save.citrus");
        let mut ui = draw_ui();
        ui.app.project.save(&path).unwrap();
        ui.app.saved_path = Some(path.clone());
        let before = json(&ui.app.project);
        let start = if tension {
            automation_tension_position(&ui, 1).unwrap()
        } else {
            automation_point_position(&ui, 2)
        };
        pointer(&mut ui, start, egui::PointerButton::Primary, Some(true));
        let end = start + Vec2::new(0.0, 8.0);
        pointer(&mut ui, end, egui::PointerButton::Primary, None);
        ui.key(egui::Key::S, piano_clipboard_command());
        ui.wait_until(|app| app.save_barrier.is_none() && app.queued_save_request.is_none());
        let saved = json(&ui.app.project);
        assert_ne!(saved, before);
        assert_eq!(json(&Project::load(&path).unwrap()), saved);
        assert!(!ui.app.project_is_dirty_now());
        pointer(
            &mut ui,
            end + Vec2::new(30.0, 8.0),
            egui::PointerButton::Primary,
            None,
        );
        pointer(
            &mut ui,
            end + Vec2::new(30.0, 8.0),
            egui::PointerButton::Primary,
            Some(false),
        );
        ui.settle();
        assert_eq!(json(&ui.app.project), saved);
        assert_eq!(json(&Project::load(&path).unwrap()), saved);
        assert!(!ui.app.project_is_dirty_now());
        undo_redo(&mut ui, &before, &saved);
    }
}

#[test]
fn automation_draw_multiple_click_batch_cannot_borrow_another_release() {
    let mut ui = draw_ui();
    let before = json(&ui.app.project);
    let first = automation_point_position(&ui, 1);
    let second = automation_point_position(&ui, 2);
    let mut events = Vec::new();
    for pos in [first, second] {
        events.push(egui::Event::PointerMoved(pos));
        for pressed in [true, false] {
            events.push(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
    }
    ui.run(events);
    ui.settle();
    assert_eq!(json(&ui.app.project), before);
    assert!(!ui.app.dirty);
    assert!(ui.app.undo_stack.is_empty());
    assert!(ui.app.playlist_gesture_before.is_none());
}

#[test]
fn automation_draw_coalesced_press_move_then_release_cannot_resize_clip() {
    for tension in [false, true] {
        let mut ui = draw_ui();
        let before = json(&ui.app.project);
        let clips = serde_json::to_value(&ui.app.project.clips).unwrap();
        let start = if tension {
            automation_tension_position(&ui, 1).unwrap()
        } else {
            automation_point_position(&ui, 2)
        };
        let rect = ui
            .ctx
            .read_response(Id::new(("playlist-clip", ui.app.project.clips[0].id)).with("resize"))
            .unwrap()
            .rect;
        let end = Pos2::new(rect.center().x, rect.top() + 3.0);
        ui.run(vec![
            egui::Event::PointerMoved(start),
            egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerMoved(end),
        ]);
        assert!(ui.app.automation_point_drag_active);
        assert_ne!(json(&ui.app.project), before);
        assert_eq!(serde_json::to_value(&ui.app.project.clips).unwrap(), clips);
        pointer(&mut ui, end, egui::PointerButton::Primary, Some(false));
        ui.settle();
        assert_eq!(serde_json::to_value(&ui.app.project.clips).unwrap(), clips);
        let edited = json(&ui.app.project);
        undo_redo(&mut ui, &before, &edited);
    }
}

#[test]
fn automation_draw_new_press_cannot_revive_hidden_previous_point_owner() {
    let mut ui = draw_ui();
    let first = automation_point_position(&ui, 2);
    let last = automation_point_position(&ui, 3);
    pointer(&mut ui, last, egui::PointerButton::Primary, Some(true));
    pointer(
        &mut ui,
        last + Vec2::new(0.0, 6.0),
        egui::PointerButton::Primary,
        None,
    );
    let last_edited = ui.app.project.automation_lanes[0].lane.points()[2];
    assert_ne!(last_edited.value, 0.5);
    let key = |key, pressed| egui::Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    // Do not settle Playlist between the interrupted owner and the new press.
    ui.run(vec![key(egui::Key::F9, true)]);
    assert_eq!(ui.app.workspace.focused, StudioView::Mixer);
    pointer(&mut ui, last, egui::PointerButton::Primary, Some(false));
    ui.run(vec![key(egui::Key::F9, false), key(egui::Key::F5, true)]);
    assert_eq!(ui.app.workspace.focused, StudioView::Playlist);
    pointer(&mut ui, first, egui::PointerButton::Primary, Some(true));
    pointer(
        &mut ui,
        first + Vec2::new(0.0, 4.0),
        egui::PointerButton::Primary,
        None,
    );
    pointer(
        &mut ui,
        first + Vec2::new(0.0, 4.0),
        egui::PointerButton::Primary,
        Some(false),
    );
    ui.settle();
    let points = ui.app.project.automation_lanes[0].lane.points();
    assert_ne!(points[1].value, 0.0);
    assert_eq!(
        points[2], last_edited,
        "the retired hidden owner must stay unchanged"
    );
    assert_eq!(ui.app.undo_stack.len(), 2);
}

#[test]
fn automation_draw_raw_press_owns_cross_clip_body_resize_and_fade_endpoints() {
    for earlier in [true, false] {
        for endpoint in ["body", "resize", "fade"] {
            for tension in [false, true] {
                let mut ui = draw_ui();
                let automation_id = ui.app.project.clips[0].id;
                let mut other = ui.app.project.clips[0].clone();
                other.id = 100;
                other.track = 1;
                other.automation_id = None;
                other.kind = if endpoint == "fade" {
                    ClipKind::Audio
                } else {
                    ClipKind::Pattern
                };
                other.pattern_length_policy =
                    (other.kind == ClipKind::Pattern).then_some(PatternLengthPolicy::Manual);
                other.fade_in = if endpoint == "fade" { 0.25 } else { 0.0 };
                if earlier {
                    ui.app.project.clips.insert(0, other);
                } else {
                    ui.app.project.clips.push(other);
                }
                ui.app.show_playlist_fades = true;
                saved_baseline(&mut ui);
                let original = ui.app.project.clone();
                let before = json(&original);
                let selection = ui.app.playlist_selection_ids.clone();
                let start = if tension {
                    automation_tension_position(&ui, 1).unwrap()
                } else {
                    automation_point_position(&ui, 2)
                };
                let id = Id::new(("playlist-clip", 100_u32));
                let id = match endpoint {
                    "resize" => id.with("resize"),
                    "fade" => id.with("fade-in"),
                    _ => id,
                };
                let end = ui
                    .ctx
                    .read_response(id)
                    .expect("real target Clip input region")
                    .rect
                    .center();
                ui.run(vec![
                    egui::Event::PointerMoved(start),
                    egui::Event::PointerButton {
                        pos: start,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    },
                    egui::Event::PointerMoved(end),
                ]);
                assert!(
                    ui.app.automation_point_drag_active,
                    "the displaced native stop cannot finish the held automation gesture"
                );
                assert!(ui.app.playlist_gesture_before.is_some());
                assert!(ui.app.undo_stack.is_empty());
                pointer(
                    &mut ui,
                    end + Vec2::new(5.0, 2.0),
                    egui::PointerButton::Primary,
                    None,
                );
                pointer(
                    &mut ui,
                    end + Vec2::new(5.0, 2.0),
                    egui::PointerButton::Primary,
                    Some(false),
                );
                ui.settle();
                let mut expected = original;
                expected.automation_lanes = ui.app.project.automation_lanes.clone();
                assert_eq!(
                    json(&ui.app.project),
                    json(&expected),
                    "only the pressed curve edits: earlier={earlier}, endpoint={endpoint}, tension={tension}"
                );
                assert_ne!(json(&ui.app.project), before);
                assert_eq!(ui.app.selected_clip, Some(automation_id));
                assert_eq!(ui.app.playlist_selection_ids, selection);
                let edited = json(&ui.app.project);
                undo_redo(&mut ui, &before, &edited);
            }
        }
    }
}

#[test]
fn automation_draw_offscreen_selected_nodes_cannot_reserve_visible_clip_click() {
    let mut ui = draw_ui();
    ui.app.project.automation_lanes[0].lane.replace_points([
        AutomationPoint::new(0.0, 1.0),
        AutomationPoint::new(2.0, 1.0),
        AutomationPoint::new(4.0, 1.0),
    ]);
    let mut visible = ui.app.project.clips[0].clone();
    visible.id = 100;
    visible.kind = ClipKind::Pattern;
    visible.track = 1;
    visible.automation_id = None;
    visible.pattern_length_policy = Some(PatternLengthPolicy::Manual);
    ui.app.project.clips.insert(0, visible);
    let row_h = ui.app.playlist_viewport.y.pixels_per_unit();
    ui.app.playlist_viewport.y.scroll_by_pixels(row_h).unwrap();
    saved_baseline(&mut ui);
    let before = json(&ui.app.project);
    let rect = ui
        .ctx
        .read_response(Id::new(("playlist-clip", 100_u32)))
        .unwrap()
        .rect;
    ui.click_pos(Pos2::new(rect.center().x, rect.top() + 2.0));
    assert_eq!(ui.app.selected_clip, Some(100));
    assert_eq!(json(&ui.app.project), before);
    assert!(ui.app.undo_stack.is_empty());
    assert!(ui.ctx.dragged_id().is_none());
}
