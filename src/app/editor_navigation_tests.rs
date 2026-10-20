//! Real production layout and input regressions for the reported Windows workflow.
use super::*;

fn wheel_event(pos: Pos2, modifiers: egui::Modifiers) -> Vec<egui::Event> {
    vec![
        egui::Event::PointerMoved(pos),
        egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            phase: egui::TouchPhase::Move,
            delta: Vec2::new(0.0, 120.0),
            modifiers,
        },
    ]
}

#[test]
fn creation_rack_long_names_keep_all_four_beats_inside_the_window() {
    for scale in [1.0, 1.5, 2.0] {
        let mut ui = UiHarness::floating();
        ui.ctx.set_zoom_factor(scale);
        ui.size = Vec2::new(1920.0, 1080.0) * scale;
        let name = "Kontakt 8 — NOIRE Pure — a deliberately very long instrument channel name";
        ui.app.project.channels[0].name = name.into();
        ui.settle();
        for width in [800.0, 960.0, 1100.0] {
            ui.arrange_test_windows(
                &[(
                    StudioView::ChannelRack,
                    Rect::from_min_size(Pos2::new(20.0, 20.0), Vec2::new(width, 270.0)),
                )],
                StudioView::ChannelRack,
            );
            let rack = ui.editor_rect(StudioView::ChannelRack);
            let title = node_rect(ui.button(name));
            assert!(
                title.width() <= 105.0,
                "name must truncate, not expand: {title:?}"
            );
            for step in 1..=16 {
                let rect = node_rect(ui.button(&format!("{name} step {step}")));
                assert!(
                    rack.shrink(3.0).contains_rect(rect),
                    "scale {scale}, width {width}, step {step}: {rect:?} / {rack:?}"
                );
                assert!(rect.width() >= 24.0 && rect.height() >= 24.0);
            }
            let before = ui.app.project.active_pattern().channel_steps[0][15];
            ui.click(&format!("{name} step 16"));
            assert_eq!(
                ui.app.project.active_pattern().channel_steps[0][15],
                !before
            );
        }
        if scale == 1.5 {
            ui.capture("creation-rack-four-beats");
        }
    }
}

#[test]
fn creation_piano_real_ctrl_wheel_zooms_both_axes_without_editing_notes() {
    let mut ui = UiHarness::floating();
    piano_clipboard_fixture(&mut ui);
    // Give the time axis room to preserve its pointer anchor rather than clamp at beat zero.
    ui.app.project.active_pattern_mut().notes[0].start = 96.0;
    ui.app.sync_history_observer();
    ui.settle();
    let surface = ui
        .ctx
        .read_response(Id::new("piano-viewport"))
        .unwrap()
        .rect;
    let pos = surface.center();
    let music = project_fingerprint(&ui.app.project);
    let x = ui.app.piano_viewport.x;
    let y = ui.app.piano_viewport.y;
    let grid = ui.ctx.read_response(Id::new("piano-grid")).unwrap().rect;
    assert!(grid.contains(pos));
    let anchor = f64::from(pos.x - grid.left());
    let beat = x.content_at_pixel(anchor);
    let ctrl = piano_clipboard_command();
    ui.run_with_modifiers(wheel_event(pos, ctrl), ctrl);
    assert!(
        ui.app.piano_viewport.x.pixels_per_unit() > x.pixels_per_unit(),
        "Ctrl-wheel must reach the real editor, not disappear in egui"
    );
    assert_eq!(
        ui.app.piano_viewport.y.pixels_per_unit(),
        y.pixels_per_unit()
    );
    let grid_after = ui.ctx.read_response(Id::new("piano-grid")).unwrap().rect;
    let anchor_after = f64::from(pos.x - grid_after.left());
    assert!((ui.app.piano_viewport.x.content_at_pixel(anchor_after) - beat).abs() < 0.01);
    for _ in 0..20 {
        ui.run_with_modifiers(Vec::new(), ctrl);
    }
    let mut vertical = ctrl;
    vertical.shift = true;
    let x = ui.app.piano_viewport.x.pixels_per_unit();
    let y = ui.app.piano_viewport.y.pixels_per_unit();
    ui.run_with_modifiers(wheel_event(pos, vertical), vertical);
    assert!(ui.app.piano_viewport.y.pixels_per_unit() > y);
    // Releasing Shift during the smooth tail must not redirect zoom into the time axis.
    for _ in 0..10 {
        ui.run(Vec::new());
    }
    assert_eq!(ui.app.piano_viewport.x.pixels_per_unit(), x);
    assert_eq!(project_fingerprint(&ui.app.project), music);
    assert!(ui.app.undo_stack.is_empty());
    ui.capture("creation-piano-view-zoom");
}

#[test]
fn creation_piano_external_ctrl_wheel_cannot_reuse_a_previous_zoom_owner() {
    let mut ui = UiHarness::floating();
    piano_clipboard_fixture(&mut ui);
    ui.app.project.active_pattern_mut().notes[0].start = 96.0;
    ui.app.sync_history_observer();
    ui.settle();
    let pos = ui
        .ctx
        .read_response(Id::new("piano-viewport"))
        .unwrap()
        .rect
        .center();
    let mut vertical = piano_clipboard_command();
    vertical.shift = true;
    ui.run_with_modifiers(wheel_event(pos, vertical), vertical);
    for _ in 0..20 {
        ui.run(Vec::new());
    }
    let viewport = ui.app.piano_viewport;
    let music = project_fingerprint(&ui.app.project);
    let outside = node_rect(ui.button("CLICK OFF")).center();
    let ctrl = piano_clipboard_command();
    ui.run_with_modifiers(wheel_event(outside, ctrl), ctrl);
    assert_eq!(ui.app.piano_viewport, viewport);
    ui.run(vec![egui::Event::PointerMoved(pos)]);
    assert!(
        (ui.ctx.input(|input| input.zoom_delta()) - 1.0).abs() > f32::EPSILON,
        "the regression must exercise a real external Ctrl-wheel smoothing tail"
    );
    for _ in 0..10 {
        ui.run(Vec::new());
    }
    assert_eq!(ui.app.piano_viewport, viewport);
    assert_eq!(project_fingerprint(&ui.app.project), music);
    assert!(ui.app.undo_stack.is_empty());
}

#[test]
fn creation_piano_ctrl_alt_velocity_tail_cannot_revive_a_previous_zoom_owner() {
    let mut ui = UiHarness::floating();
    piano_clipboard_fixture(&mut ui);
    ui.app.piano_roll_state.tool = PianoRollTool::Draw;
    let note_body = |ui: &UiHarness| {
        ui.ctx
            .read_response(Id::new(("piano-note", 90_001_u64)))
            .unwrap()
            .rect
            .center()
    };
    let ctrl = piano_clipboard_command();
    ui.run_with_modifiers(wheel_event(note_body(&ui), ctrl), ctrl);
    for _ in 0..20 {
        ui.run(Vec::new());
    }
    let viewport = ui.app.piano_viewport;
    let mut fine = egui::Modifiers::ALT;
    fine.ctrl = true;
    ui.run_with_modifiers(
        vec![
            egui::Event::PointerMoved(note_body(&ui)),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                phase: egui::TouchPhase::Move,
                delta: Vec2::new(0.0, 1.0),
                modifiers: fine,
            },
        ],
        fine,
    );
    assert_eq!(ui.app.undo_stack.len(), 1);
    assert!((ui.app.project.active_pattern().notes[0].velocity - 0.635).abs() < 0.000_01);
    let edited = project_fingerprint(&ui.app.project);
    ui.run(Vec::new());
    assert!(
        (ui.ctx.input(|input| input.zoom_delta()) - 1.0).abs() > f32::EPSILON,
        "Ctrl+Alt expression still produces egui's private zoom tail"
    );
    for _ in 0..10 {
        ui.run(Vec::new());
    }
    assert_eq!(ui.app.piano_viewport, viewport);
    assert_eq!(project_fingerprint(&ui.app.project), edited);
    assert_eq!(ui.app.undo_stack.len(), 1);
}

#[test]
fn creation_piano_zoom_tail_stops_after_the_pointer_leaves_the_editor() {
    let mut ui = UiHarness::floating();
    piano_clipboard_fixture(&mut ui);
    ui.app.project.active_pattern_mut().notes[0].start = 96.0;
    ui.app.sync_history_observer();
    ui.settle();
    let pos = ui
        .ctx
        .read_response(Id::new("piano-viewport"))
        .unwrap()
        .rect
        .center();
    let ctrl = piano_clipboard_command();
    ui.run_with_modifiers(wheel_event(pos, ctrl), ctrl);
    let viewport = ui.app.piano_viewport;
    let music = project_fingerprint(&ui.app.project);
    let outside = node_rect(ui.button("CLICK OFF")).center();
    ui.run(vec![egui::Event::PointerMoved(outside)]);
    assert_eq!(ui.app.piano_viewport, viewport);
    ui.run(vec![egui::Event::PointerMoved(pos)]);
    assert!(
        (ui.ctx.input(|input| input.zoom_delta()) - 1.0).abs() > f32::EPSILON,
        "the regression must return while the original wheel still has a tail"
    );
    for _ in 0..10 {
        ui.run(Vec::new());
    }
    assert_eq!(ui.app.piano_viewport, viewport);
    assert_eq!(project_fingerprint(&ui.app.project), music);
    assert!(ui.app.undo_stack.is_empty());
}

#[test]
fn creation_piano_zoom_tail_does_not_resume_after_switching_maximized_editors() {
    let mut ui = UiHarness::floating();
    piano_clipboard_fixture(&mut ui);
    ui.app.workspace.maximized = true;
    ui.app.project.active_pattern_mut().notes[0].start = 96.0;
    ui.app.sync_history_observer();
    ui.settle();
    let pos = ui
        .ctx
        .read_response(Id::new("piano-viewport"))
        .unwrap()
        .rect
        .center();
    let ctrl = piano_clipboard_command();
    ui.run_with_modifiers(wheel_event(pos, ctrl), ctrl);
    let viewport = ui.app.piano_viewport;
    let music = project_fingerprint(&ui.app.project);
    let key = |key, pressed| egui::Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    // A single frame per switch retains the genuine smoothing tail throughout.
    ui.run(vec![key(egui::Key::F5, true)]);
    assert_eq!(ui.app.workspace.focused, StudioView::Playlist);
    assert_eq!(
        ui.app.piano_viewport, viewport,
        "the old Piano must not consume zoom before the F5 shortcut is dispatched"
    );
    ui.run(vec![key(egui::Key::F5, false), key(egui::Key::F7, true)]);
    assert_eq!(ui.app.workspace.focused, StudioView::PianoRoll);
    assert_eq!(ui.app.piano_viewport, viewport);
    assert!(
        (ui.ctx.input(|input| input.zoom_delta()) - 1.0).abs() > f32::EPSILON,
        "the editor switch must complete before the wheel tail is exhausted"
    );
    ui.run(vec![key(egui::Key::F7, false)]);
    assert_eq!(ui.app.piano_viewport, viewport);
    assert_eq!(project_fingerprint(&ui.app.project), music);
    assert!(ui.app.undo_stack.is_empty());
}

#[test]
fn creation_piano_fresh_ordinary_wheel_after_ctrl_zoom_still_pans() {
    let mut ui = UiHarness::floating();
    piano_clipboard_fixture(&mut ui);
    ui.app.project.active_pattern_mut().notes[0].start = 96.0;
    ui.app.sync_history_observer();
    ui.settle();
    let pos = ui
        .ctx
        .read_response(Id::new("piano-viewport"))
        .unwrap()
        .rect
        .center();
    let ctrl = piano_clipboard_command();
    ui.run_with_modifiers(wheel_event(pos, ctrl), ctrl);
    let horizontal = ui.app.piano_viewport.x;
    let origin = ui.app.piano_viewport.y.origin();
    let music = project_fingerprint(&ui.app.project);
    let mut ordinary = wheel_event(pos, egui::Modifiers::NONE);
    if let egui::Event::MouseWheel { delta, .. } = &mut ordinary[1] {
        delta.y = -120.0;
    }
    ui.run(ordinary);
    assert!(
        ui.app.piano_viewport.y.origin() > origin,
        "fresh reverse ordinary wheel must pan in its own direction"
    );
    for _ in 0..10 {
        ui.run(Vec::new());
    }
    assert_eq!(ui.app.piano_viewport.x, horizontal);
    assert_eq!(project_fingerprint(&ui.app.project), music);
    assert!(ui.app.undo_stack.is_empty());
}

#[test]
fn creation_playlist_ruler_seeks_and_default_draw_places_the_full_pattern_with_one_undo() {
    let mut ui = UiHarness::new();
    assert!(matches!(ui.app.tool_mode, ToolMode::Draw));
    ui.app.project.clips.clear();
    ui.app.project.active_pattern_mut().name = "Sixteen-beat melody".into();
    ui.app.project.active_pattern_mut().notes[0].start = 14.0;
    ui.app.project.active_pattern_mut().notes[0].length = 2.0;
    ui.app.project.song_length_beats = 16.0;
    ui.app.project.auto_song_length = false;
    ui.app.sync_history_observer();
    ui.settle();
    let canvas = ui
        .ctx
        .read_response(Id::new("playlist-canvas"))
        .unwrap()
        .rect;
    let ruler = ui
        .ctx
        .read_response(Id::new("playlist-time-ruler"))
        .unwrap()
        .rect;
    let original = project_fingerprint(&ui.app.project);
    let ruler_pos = Pos2::new(
        canvas.left() + ui.app.playlist_viewport.x.pixel_for_content(6.5) as f32,
        ruler.center().y,
    );
    ui.run(mixer_pointer_button(ruler_pos, true));
    assert_eq!(ui.app.beat_position, 6.5);
    ui.run(mixer_pointer_button(ruler_pos, false));
    assert_eq!(project_fingerprint(&ui.app.project), original);
    assert!(ui.app.undo_stack.is_empty());

    // Place near the old song end. Full Pattern length extends the song, without trimming notes.
    let pos = Pos2::new(
        canvas.left() + ui.app.playlist_viewport.x.pixel_for_content(12.0) as f32,
        canvas.top() + ui.app.playlist_viewport.y.pixel_for_content(2.5) as f32,
    );
    ui.run(mixer_pointer_button(pos, true));
    assert_eq!(
        ui.app.project.clips.len(),
        1,
        "placement responds on mouse-down"
    );
    assert_eq!(
        ui.app.project.clips[0].pattern_id,
        ui.app.project.active_pattern().id
    );
    assert_eq!(ui.app.project.clips[0].name, "Sixteen-beat melody");
    assert_eq!(ui.app.project.clips[0].length, 16.0);
    assert_eq!(ui.app.project.song_length_beats, 28.0);
    assert!(ui.app.playlist_gesture_before.is_some());
    ui.run(mixer_pointer_button(pos, false));
    assert_eq!(
        ui.app.project.clips.len(),
        1,
        "release must not place a second copy"
    );
    assert!(ui.app.playlist_gesture_before.is_none());
    assert_eq!(ui.app.undo_stack.len(), 1);
    let placed = project_fingerprint(&ui.app.project);
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(
        project_fingerprint(&ui.app.project),
        original,
        "Undo restores both placement and song length"
    );
    ui.key(egui::Key::Y, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), placed);
    ui.capture("creation-playlist-full-pattern");
}

#[test]
fn creation_piano_fit_notes_reaches_far_data_before_the_silent_phrase_tail() {
    let mut ui = UiHarness::floating();
    piano_clipboard_fixture(&mut ui);
    ui.app.project.active_pattern_mut().notes.truncate(1);
    let note = &mut ui.app.project.active_pattern_mut().notes[0];
    note.start = 257.0;
    note.length = 2.0;
    note.group_id = None;
    ui.app.sync_history_observer();
    ui.app.undo_stack.clear();
    ui.app.redo_stack.clear();
    ui.settle();
    assert_eq!(
        piano_pattern_content_end(ui.app.project.active_pattern()),
        276.0
    );
    ui.app.piano_viewport.x.set_normalized_scroll(1.0).unwrap();
    ui.settle();
    assert!(!content_span_is_visible(
        ui.app.piano_viewport.x,
        257.0,
        259.0
    ));
    let before = project_fingerprint(&ui.app.project);
    ui.click("FIT NOTES");
    ui.settle();
    assert!(content_span_is_visible(
        ui.app.piano_viewport.x,
        257.0,
        259.0
    ));
    let note = ui
        .ctx
        .read_response(Id::new(("piano-note", 90_001_u64)))
        .expect("the production FIT NOTES command must expose the actual far note")
        .rect;
    assert!(ui.editor_rect(StudioView::PianoRoll).contains_rect(note));
    let pixel = ui.app.piano_viewport.x.pixel_for_content(258.0);
    assert!((0.0..ui.app.piano_viewport.x.viewport_pixels()).contains(&pixel));
    assert!((ui.app.piano_viewport.x.content_at_pixel(pixel) - 258.0).abs() < 1.0e-9);
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert!(ui.app.undo_stack.is_empty() && ui.app.redo_stack.is_empty());
}

#[test]
fn creation_piano_crossing_four_bars_expands_playback_and_new_playlist_occurrences() {
    let mut ui = UiHarness::floating();
    piano_clipboard_fixture(&mut ui);
    ui.app.project.active_pattern_mut().notes.truncate(1);
    let note = &mut ui.app.project.active_pattern_mut().notes[0];
    note.start = 15.0;
    note.length = 1.0;
    note.group_id = None;
    ui.app.project.song_length_beats = 16.0;
    ui.app.project.clips.truncate(1);
    ui.app.project.clips[0].length = 8.0;
    let old_clip = ui.app.project.clips[0].clone();
    ui.app.sync_history_observer();
    ui.app.undo_stack.clear();
    ui.app.redo_stack.clear();
    ui.settle();
    ui.app.piano_viewport.x.reveal(14.0, 18.0, 0.0).unwrap();
    ui.settle();
    assert_eq!(pattern_playback_length_beats(&ui.app.project), 16.0);
    let before = project_fingerprint(&ui.app.project);
    let resize = ui
        .ctx
        .read_response(Id::new(("piano-note", 90_001_u64)).with("resize"))
        .expect("the actual note end must be visible")
        .rect
        .center();
    let beat_pixels = ui.app.piano_viewport.x.pixels_per_unit() as f32;
    ui.drag_pointer(resize, Vec2::new(beat_pixels, 0.0));
    let note = &ui.app.project.active_pattern().notes[0];
    assert_eq!((note.start, note.length), (15.0, 2.0));
    assert_eq!(pattern_playback_length_beats(&ui.app.project), 32.0);
    assert_eq!(
        piano_pattern_content_end(ui.app.project.active_pattern()),
        36.0
    );
    assert_eq!(
        transport_loop_end_beat(&ui.app.project, TransportMode::Pattern),
        32.0
    );
    assert_eq!(
        serde_json::to_value(&ui.app.project.clips[0]).unwrap(),
        serde_json::to_value(&old_clip).unwrap(),
        "editing must preserve an existing trimmed occurrence"
    );
    assert_eq!(
        ui.app.undo_stack.len(),
        1,
        "one resize gesture is one history entry"
    );
    let edited = project_fingerprint(&ui.app.project);
    let projected = transport_playback_project(&ui.app.project, TransportMode::Pattern);
    assert_eq!(projected.song_length_beats, 32.0);
    assert_eq!(projected.clips[0].length, 32.0);
    let timeline = compile_timeline_job(TimelineCompileJob {
        generation: 1,
        fingerprint: timeline_playback_fingerprint(&ui.app.project, TransportMode::Pattern, 48_000),
        sample_rate: 48_000,
        project: projected,
        options: transport_timeline_compile_options(&ui.app.project, TransportMode::Pattern),
    })
    .result
    .unwrap();
    let tail_frame = (48_000.0 * 60.0 / f64::from(ui.app.project.tempo) * 17.0).round() as u64;
    assert!(
        timeline
            .events()
            .iter()
            .any(|event| event.frame == tail_frame
                && matches!(
                    event.kind,
                    crate::timeline::TimelineEventKind::NoteOff { note: 60, .. }
                )),
        "the compiled Pattern must retain the note-off beyond four bars"
    );
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert_eq!(pattern_playback_length_beats(&ui.app.project), 16.0);
    ui.key(egui::Key::Y, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), edited);
    assert_eq!(pattern_playback_length_beats(&ui.app.project), 32.0);

    ui.key(egui::Key::F5, egui::Modifiers::NONE);
    let canvas = ui
        .ctx
        .read_response(Id::new("playlist-canvas"))
        .unwrap()
        .rect;
    let pos = Pos2::new(
        canvas.left() + ui.app.playlist_viewport.x.pixel_for_content(8.0) as f32,
        canvas.top() + ui.app.playlist_viewport.y.pixel_for_content(2.5) as f32,
    );
    ui.run(mixer_pointer_button(pos, true));
    ui.run(mixer_pointer_button(pos, false));
    let placed = ui
        .app
        .project
        .clips
        .iter()
        .find(|clip| clip.id != old_clip.id)
        .unwrap();
    assert_eq!((placed.start, placed.length), (8.0, 32.0));
    assert_eq!(ui.app.project.song_length_beats, 40.0);
    assert_eq!(
        serde_json::to_value(
            ui.app
                .project
                .clips
                .iter()
                .find(|clip| clip.id == old_clip.id)
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(&old_clip).unwrap()
    );
    assert_eq!(ui.app.undo_stack.len(), 2);
    let complete = project_fingerprint(&ui.app.project);
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), edited);
    ui.key(egui::Key::Y, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), complete);
}

#[test]
fn creation_buttons_depress_on_press_and_activate_once_on_release() {
    let mut ui = UiHarness::new();
    ui.app.set_metronome_enabled(false);
    ui.settle();
    let rect = node_rect(ui.button("CLICK OFF"));
    let text_pos = |output: &egui::FullOutput| {
        output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.job.text == "CLICK OFF" => Some(text.pos),
                _ => None,
            })
            .expect("native button text must still be painted")
    };
    fn painted_fill(shape: &egui::Shape, bounds: Rect) -> Option<Color32> {
        match shape {
            egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| painted_fill(shape, bounds)),
            egui::Shape::Rect(face) if face.rect == bounds && face.fill != Color32::TRANSPARENT => {
                Some(face.fill)
            }
            _ => None,
        }
    }
    let face_fill = |output: &egui::FullOutput| {
        output
            .shapes
            .iter()
            .find_map(|shape| painted_fill(&shape.shape, rect))
            .expect("the real button face must be painted")
    };
    let before = ui.run(vec![egui::Event::PointerMoved(rect.center())]);
    let before_text = text_pos(&before);
    assert_eq!(face_fill(&before), theme::HIGHLIGHT);
    let held = ui.run(mixer_pointer_button(rect.center(), true));
    assert!(
        !ui.app.audio_preferences.metronome_enabled,
        "press must not activate the momentary command"
    );
    assert_eq!(
        node_rect(ui.button("CLICK OFF")),
        rect,
        "pressed visuals must not move hit bounds"
    );
    assert_eq!(
        text_pos(&held),
        before_text,
        "flat pressed paint keeps its text anchored"
    );
    assert_eq!(face_fill(&held), theme::WELL);
    assert_ne!(
        face_fill(&held),
        face_fill(&before),
        "press must have visible feedback"
    );
    ui.run(Vec::new());
    assert!(
        !ui.app.audio_preferences.metronome_enabled,
        "holding must not activate"
    );
    ui.capture("creation-button-held");
    ui.run(mixer_pointer_button(rect.center(), false));
    assert!(ui.app.audio_preferences.metronome_enabled);
    ui.settle();
    assert!(
        ui.app.audio_preferences.metronome_enabled,
        "release activates exactly once"
    );
    assert_eq!(node_rect(ui.button("CLICK ON")), rect);
    ui.capture("creation-button-released");
}
