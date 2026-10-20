//! Inspector pointer ownership at project, save, and egui multi-pass boundaries.
//! Musical edits and lifecycle requests use real production UI input. The manual
//! save-entry variant is an explicit control for non-shortcut callers of that API.
use super::*;

fn json(project: &Project) -> serde_json::Value {
    serde_json::to_value(project).unwrap()
}

fn pattern_gain_ui() -> UiHarness {
    let mut ui = UiHarness::new();
    let mut project = Project::blank();
    project.tempo = 120.0;
    project.song_length_beats = 8.0;
    project.auto_song_length = false;
    let mut clip = Project::default().clips[0].clone();
    clip.id = 80_101;
    clip.name = "Lifecycle gain fixture".into();
    clip.kind = ClipKind::Pattern;
    clip.track = 0;
    clip.start = 0.0;
    clip.length = 4.0;
    clip.pattern_id = 1;
    clip.pattern_length_policy = Some(PatternLengthPolicy::Manual);
    clip.group_id = None;
    clip.automation_id = None;
    clip.audio_asset_id = None;
    clip.gain = 1.0;
    clip.fade_in = 0.0;
    clip.fade_out = 0.0;
    project.clips.push(clip);
    assert!(ui.app.install_project(project, None, false));
    ui.key(egui::Key::F5, egui::Modifiers::NONE);
    ui.key(egui::Key::E, egui::Modifiers::NONE);
    let rect = ui
        .ctx
        .read_response(Id::new(("playlist-clip", 80_101_u32)))
        .expect("fixture must be visible in the production Playlist")
        .rect;
    ui.click_pos(rect.center());
    assert_eq!(ui.app.selected_clip, Some(80_101));
    ui.app.sync_history_observer();
    ui.app.project_fingerprint = project_fingerprint(&ui.app.project);
    ui.app.undo_stack.clear();
    ui.app.redo_stack.clear();
    ui.app.dirty = false;
    ui.settle();
    ui
}

fn gain_position(ui: &UiHarness, fraction: f32) -> Pos2 {
    let labels: Vec<_> = ui
        .nodes
        .iter()
        .filter(|node| {
            node.role() == Role::Label
                && (node.value() == Some("Gain") || node.label() == Some("Gain"))
        })
        .collect();
    assert_eq!(labels.len(), 1, "one Inspector Gain label");
    let y = node_rect(labels[0]).center().y;
    let sliders: Vec<_> = ui
        .nodes
        .iter()
        .filter(|node| {
            node.role() == Role::Slider
                && node.min_numeric_value() == Some(0.0)
                && node.max_numeric_value() == Some(1.5)
                && (node_rect(node).center().y - y).abs() < 15.0
        })
        .collect();
    assert_eq!(sliders.len(), 1, "one real slider beside Gain");
    assert!(!sliders[0].is_disabled());
    let rect = node_rect(sliders[0]);
    assert!(Rect::from_min_size(Pos2::ZERO, ui.size).contains_rect(rect));
    Pos2::new(rect.left() + rect.width() * fraction, rect.center().y)
}

fn pointer_frame(ui: &mut UiHarness, pos: Pos2, pressed: Option<bool>) -> egui::FullOutput {
    // Make a split transaction deterministic rather than depending on render speed.
    ui.app.last_history_check = Instant::now() - Duration::from_secs(2);
    ui.app.last_history_capture = Instant::now() - Duration::from_secs(2);
    let mut events = vec![egui::Event::PointerMoved(pos)];
    if let Some(pressed) = pressed {
        events.push(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
    }
    ui.run(events)
}

fn begin_gain_drag(ui: &mut UiHarness) -> Pos2 {
    let pos = gain_position(ui, 0.3);
    pointer_frame(ui, pos, Some(true));
    assert!(ui.ctx.input(|input| input.pointer.primary_down()));
    assert!(ui.app.playlist_gesture_before.is_some());
    assert!(ui.app.undo_stack.is_empty());
    assert!(ui.app.project.clips[0].gain < 0.8);
    pos
}

#[test]
fn inspector_gain_new_project_cancel_while_held_retains_one_undoable_edit() {
    let mut ui = pattern_gain_ui();
    let original = json(&ui.app.project);
    let session = ui.app.project_session;
    let pos = begin_gain_drag(&mut ui);
    ui.key(egui::Key::N, piano_clipboard_command());
    assert!(ui.app.project_lifecycle.is_modal());
    assert!(ui.ctx.input(|input| input.pointer.primary_down()));
    assert!(ui.app.playlist_gesture_before.is_none());
    assert_eq!(ui.app.undo_stack.len(), 1);
    let edited = json(&ui.app.project);
    assert_ne!(edited, original);

    // Cancel the real New dialog without releasing the original pointer first.
    ui.key(egui::Key::Escape, egui::Modifiers::NONE);
    assert!(ui.app.project_lifecycle.is_idle());
    assert_eq!(ui.app.project_session, session);
    pointer_frame(&mut ui, pos + Vec2::new(45.0, 0.0), None);
    pointer_frame(&mut ui, pos + Vec2::new(45.0, 0.0), Some(false));
    ui.settle();
    assert_eq!(json(&ui.app.project), edited);
    assert!(ui.app.playlist_gesture_before.is_none());
    assert_eq!(ui.app.undo_stack.len(), 1);
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(json(&ui.app.project), original);
    assert!(ui.app.undo_stack.is_empty());
    ui.key(egui::Key::Y, piano_clipboard_command());
    assert_eq!(json(&ui.app.project), edited);
}

#[test]
fn inspector_gain_new_project_discard_cannot_restore_the_retired_session() {
    let mut ui = pattern_gain_ui();
    let old_session = ui.app.project_session;
    let pos = begin_gain_drag(&mut ui);
    ui.key(egui::Key::N, piano_clipboard_command());
    assert!(ui.app.project_lifecycle.is_modal());
    assert!(ui.ctx.input(|input| input.pointer.primary_down()));
    assert!(ui.app.playlist_gesture_before.is_none());
    assert_eq!(ui.app.undo_stack.len(), 1);
    // Finish the canceled old pointer before an ordinary click on Discard.
    pointer_frame(&mut ui, pos, Some(false));
    ui.click("DISCARD");
    assert!(ui.app.project_lifecycle.is_idle());
    assert_ne!(ui.app.project_session, old_session);
    assert!(ui.app.project.clips.is_empty());
    assert!(ui.app.playlist_gesture_before.is_none());
    assert!(ui.app.undo_stack.is_empty());
    assert!(ui.app.redo_stack.is_empty());
    let fresh = json(&ui.app.project);
    ui.run(vec![egui::Event::PointerMoved(pos)]);
    ui.key(egui::Key::Z, piano_clipboard_command());
    ui.key(egui::Key::Y, piano_clipboard_command());
    ui.settle();
    assert_eq!(json(&ui.app.project), fresh);
    assert!(ui.app.undo_stack.is_empty());
    assert!(ui.app.redo_stack.is_empty());
    assert!(!ui.app.project_is_dirty_now());
}

#[test]
fn inspector_gain_pointer_transaction_survives_forced_egui_multiple_passes() {
    let mut ui = pattern_gain_ui();
    ui.ctx
        .options_mut(|options| options.max_passes = 2.try_into().unwrap());
    ui.ctx.on_end_pass(
        "Inspector gain transaction multi-pass regression",
        std::sync::Arc::new(|ui| {
            if ui.ctx().current_pass_index() == 0 {
                ui.ctx()
                    .request_discard("exercise Inspector transaction across layout passes");
            }
        }),
    );
    let original = json(&ui.app.project);
    let fingerprint = project_fingerprint(&ui.app.project);
    for (fraction, pressed) in [(0.2, Some(true)), (0.45, None), (0.6, None)] {
        let pos = gain_position(&ui, fraction);
        let output = pointer_frame(&mut ui, pos, pressed);
        assert_eq!(output.platform_output.num_completed_passes, 2);
        assert!(ui.app.playlist_gesture_before.is_some());
        assert!(ui.app.undo_stack.is_empty());
        assert_eq!(ui.app.history_fingerprint, fingerprint);
    }
    let pos = gain_position(&ui, 0.7);
    let output = pointer_frame(&mut ui, pos, Some(false));
    assert_eq!(output.platform_output.num_completed_passes, 2);
    ui.settle();
    let edited = json(&ui.app.project);
    assert_ne!(edited, original);
    assert!(ui.app.playlist_gesture_before.is_none());
    assert_eq!(ui.app.undo_stack.len(), 1);
    assert_eq!(
        ui.app.history_fingerprint,
        project_fingerprint(&ui.app.project)
    );
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(json(&ui.app.project), original);
    assert!(ui.app.undo_stack.is_empty());
    ui.key(egui::Key::Y, piano_clipboard_command());
    assert_eq!(json(&ui.app.project), edited);
}

#[test]
fn inspector_gain_held_manual_save_commits_once_and_old_pointer_cannot_rewrite() {
    for through_shortcut in [true, false] {
        let fixture = Fixture::new();
        let path = fixture.0.join("held-inspector-save.citrus");
        let mut ui = pattern_gain_ui();
        ui.app.project.save(&path).unwrap();
        ui.app.saved_path = Some(path.clone());
        let original = json(&ui.app.project);
        assert_eq!(json(&Project::load(&path).unwrap()), original);
        let pos = begin_gain_drag(&mut ui);
        if through_shortcut {
            ui.key(egui::Key::S, piano_clipboard_command());
        } else {
            // Same production entry used by Save As/lifecycle, with the native
            // path picker supplied by this isolated test fixture.
            ui.app.begin_project_save(ProjectSaveRequest::Manual {
                path: path.clone(),
                lifecycle: None,
            });
            ui.settle();
        }
        ui.wait_until(|app| app.save_barrier.is_none() && app.queued_save_request.is_none());
        assert!(ui.ctx.input(|input| input.pointer.primary_down()));
        assert!(ui.app.playlist_gesture_before.is_none());
        let saved = json(&ui.app.project);
        assert_ne!(saved, original);
        assert_eq!(json(&Project::load(&path).unwrap()), saved);
        assert!(!ui.app.project_is_dirty_now());
        assert_eq!(
            ui.app.undo_stack.len(),
            1,
            "manual path: {through_shortcut}"
        );

        // The save is complete, but this is still the original mouse press.
        // Fast save completion must not reactivate the retired slider drag.
        pointer_frame(&mut ui, pos + Vec2::new(45.0, 0.0), None);
        assert_eq!(json(&ui.app.project), saved);
        pointer_frame(&mut ui, pos + Vec2::new(45.0, 0.0), Some(false));
        ui.settle();
        assert_eq!(json(&ui.app.project), saved);
        assert!(!ui.app.project_is_dirty_now());
        assert!(ui.app.playlist_gesture_before.is_none());
        assert_eq!(ui.app.undo_stack.len(), 1);
        ui.key(egui::Key::Z, piano_clipboard_command());
        assert_eq!(json(&ui.app.project), original);
        assert!(ui.app.undo_stack.is_empty());
        assert!(ui.app.project_is_dirty_now());
        ui.key(egui::Key::Y, piano_clipboard_command());
        assert_eq!(json(&ui.app.project), saved);
        assert_eq!(json(&Project::load(&path).unwrap()), saved);
    }
}
