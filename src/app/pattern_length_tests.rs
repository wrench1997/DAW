//! Real editor input for per-placement length policy; no native device or dialog.
use super::piano_keyboard_playback_tests::KeyboardCallbackFixture;
use super::*;

fn policy_clip(id: u32, start: f32, length: f32, policy: Option<PatternLengthPolicy>) -> Clip {
    let mut clip = Project::default().clips[0].clone();
    assert_eq!(clip.kind, ClipKind::Pattern);
    assert_eq!(clip.pattern_length_policy, None);
    clip.id = id;
    clip.track = id as usize - 1;
    clip.start = start;
    clip.length = length;
    clip.pattern_id = 1;
    clip.pattern_length_policy = policy;
    clip.group_id = None;
    clip.source_offset = 0.0;
    clip.gain = 1.0;
    clip.muted = false;
    clip
}

fn project() -> Project {
    let mut project = Project::blank();
    project.auto_song_length = true;
    project.tempo = 240.0;
    project.active_pattern_mut().notes = vec![PianoNote {
        id: 90_101,
        channel_id: Some(1),
        group_id: None,
        note: 60,
        start: 0.25,
        length: 0.75,
        velocity: 0.8,
        selected: false,
        muted: false,
    }];
    project.clips = vec![
        policy_clip(1, 0.0, 1.0, Some(PatternLengthPolicy::Follow)),
        policy_clip(2, 8.0, 2.0, Some(PatternLengthPolicy::Follow)),
        policy_clip(3, 16.0, 1.5, None),
        policy_clip(4, 20.0, 1.75, Some(PatternLengthPolicy::Manual)),
    ];
    project
}

fn install(ui: &mut UiHarness, project: Project) {
    ui.app.project = project;
    ui.app.transport_mode = TransportMode::Song;
    ui.app.beat_position = 0.0;
    ui.app.selected_channel = 0;
    ui.app.selected_clip = None;
    ui.app.playlist_selection_ids.clear();
    ui.app.piano_roll_state.clear_selection();
    ui.app.piano_roll_state.tool = PianoRollTool::Draw;
    ui.app.piano_roll_state.local_snap = PianoSnap::QuarterBeat;
    ui.app.piano_roll_state.last_note_length = 1.0;
    ui.app.tool_mode = ToolMode::Draw;
    ui.app.snap = 0.25;
    ui.app.focus_editor(StudioView::Playlist);
    ui.app.sync_history_observer();
    ui.app.project_fingerprint = project_fingerprint(&ui.app.project);
    ui.app.undo_stack.clear();
    ui.app.redo_stack.clear();
    ui.app.dirty = false;
    ui.settle();
}

fn fixture() -> UiHarness {
    let mut ui = UiHarness::new();
    install(&mut ui, project());
    ui
}

fn clip(ui: &UiHarness, id: u32) -> &Clip {
    ui.app
        .project
        .clips
        .iter()
        .find(|clip| clip.id == id)
        .unwrap()
}

fn clip_rect(ui: &UiHarness, id: u32) -> Rect {
    ui.ctx
        .read_response(Id::new(("playlist-clip", id)))
        .unwrap()
        .rect
}

fn pointer(ui: &mut UiHarness, pos: Pos2, pressed: Option<bool>) {
    let mut events = vec![egui::Event::PointerMoved(pos)];
    if let Some(pressed) = pressed {
        events.push(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
    }
    ui.run(events);
}

fn select_clip(ui: &mut UiHarness, id: u32) {
    if ui.app.workspace.focused != StudioView::Playlist {
        ui.key(egui::Key::F5, egui::Modifiers::NONE);
    }
    let pos = clip_rect(ui, id).center();
    ui.click_pos(pos);
    assert_eq!(ui.app.selected_clip, Some(id));
}

fn open_piano(ui: &mut UiHarness, id: u32) {
    if ui.app.workspace.focused != StudioView::Playlist {
        ui.key(egui::Key::F5, egui::Modifiers::NONE);
    }
    let pos = clip_rect(ui, id).center();
    for _ in 0..2 {
        pointer(ui, pos, Some(true));
        pointer(ui, pos, Some(false));
    }
    ui.settle();
    assert_eq!(ui.app.workspace.focused, StudioView::PianoRoll);
    assert_eq!(ui.app.project.active_pattern().id, clip(ui, id).pattern_id);
}

fn piano_point(ui: &UiHarness, beat: f64, pitch: u8) -> Pos2 {
    let grid = ui.ctx.read_response(Id::new("piano-grid")).unwrap().rect;
    let pos = Pos2::new(
        grid.left() + ui.app.piano_viewport.x.pixel_for_content(beat) as f32,
        grid.top()
            + ui.app
                .piano_viewport
                .y
                .pixel_for_content(piano_pitch_row(pitch) + 0.5) as f32,
    );
    assert!(
        grid.contains(pos),
        "{pos:?} outside actual Piano grid {grid:?}"
    );
    pos
}

fn insert_late_note(ui: &mut UiHarness) -> u64 {
    ui.app.piano_viewport.x.reveal(4.25, 5.25, 12.0).unwrap();
    ui.app
        .piano_viewport
        .y
        .reveal(piano_pitch_row(64), piano_pitch_row(64) + 1.0, 12.0)
        .unwrap();
    ui.settle();
    // Inside the quarter-beat cell, not on its floating-point pixel boundary.
    let pos = piano_point(ui, 4.3125, 64);
    ui.click_pos(pos);
    let notes = &ui.app.project.active_pattern().notes;
    let added = notes
        .iter()
        .find(|note| note.start == 4.25 && note.note == 64)
        .unwrap();
    assert_eq!(added.length, 1.0);
    assert!(ui.app.piano_roll_state.selection_ids.contains(&added.id));
    added.id
}

fn select_note(ui: &mut UiHarness, id: u64) {
    let pos = ui
        .ctx
        .read_response(Id::new(("piano-note", id)))
        .unwrap()
        .rect
        .center();
    ui.click_pos(pos);
    assert_eq!(ui.app.piano_roll_state.selection_ids, HashSet::from([id]));
}

fn effective_lengths(ui: &UiHarness) -> Vec<f32> {
    ui.app
        .project
        .clips
        .iter()
        .map(|clip| ui.app.project.clip_length_beats(clip))
        .collect()
}

fn checkbox_pos(ui: &UiHarness) -> Pos2 {
    let nodes: Vec<_> = ui
        .nodes
        .iter()
        .filter(|node| node.role() == Role::CheckBox && node.label() == Some("Follow Pattern"))
        .collect();
    assert_eq!(nodes.len(), 1, "actual Follow checkbox: {nodes:?}");
    node_rect(nodes[0]).center()
}

fn length_pos(ui: &UiHarness) -> Pos2 {
    let nodes: Vec<_> = ui
        .nodes
        .iter()
        .filter(|node| {
            node.role() == Role::SpinButton
                && node.min_numeric_value() == Some(0.25)
                && node.max_numeric_value() == Some(4096.0)
        })
        .collect();
    assert_eq!(
        nodes.len(),
        1,
        "actual Inspector Length numeric control: {nodes:?}"
    );
    node_rect(nodes[0]).center()
}

fn type_length(ui: &mut UiHarness, digit: egui::Key, text: &str) {
    let pos = length_pos(ui);
    ui.click_pos(pos);
    assert!(ui.ctx.text_edit_focused());
    ui.key(egui::Key::A, piano_clipboard_command());
    ui.run(vec![
        egui::Event::Key {
            key: digit,
            physical_key: Some(digit),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        },
        egui::Event::Text(text.into()),
    ]);
    ui.run(vec![egui::Event::Key {
        key: digit,
        physical_key: Some(digit),
        pressed: false,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }]);
    assert!(ui.ctx.text_edit_focused());
}

#[test]
fn playlist_draw_creates_follow_placement_through_real_canvas_input() {
    let mut ui = fixture();
    let mut empty = project();
    empty.clips.clear();
    install(&mut ui, empty);
    let before = project_fingerprint(&ui.app.project);
    let canvas = ui
        .ctx
        .read_response(Id::new("playlist-canvas"))
        .unwrap()
        .rect;
    let pos = Pos2::new(
        canvas.left() + ui.app.playlist_viewport.x.pixel_for_content(2.125) as f32,
        canvas.top() + ui.app.playlist_viewport.y.pixel_for_content(0.5) as f32,
    );
    assert!(canvas.contains(pos));
    ui.click_pos(pos);
    assert_eq!(ui.app.project.clips.len(), 1);
    let created = &ui.app.project.clips[0];
    assert_eq!(
        created.pattern_length_policy,
        Some(PatternLengthPolicy::Follow)
    );
    assert_eq!((created.start, created.length), (2.0, 4.0));
    assert_eq!(ui.app.undo_stack.len(), 1);
    ui.capture("pattern-length-draw-follow");
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), before);
    ui.key(egui::Key::Y, piano_clipboard_command());
    assert!(ui.app.project.clips[0].follows_pattern());
}

#[test]
fn piano_source_insert_delete_updates_all_follow_references_with_single_step_history() {
    let mut ui = fixture();
    open_piano(&mut ui, 1);
    let before = project_fingerprint(&ui.app.project);
    let stored_clips = serde_json::to_value(&ui.app.project.clips).unwrap();
    assert_eq!(effective_lengths(&ui), vec![4.0, 4.0, 1.5, 1.75]);
    let added_id = insert_late_note(&mut ui);
    let grown = project_fingerprint(&ui.app.project);
    assert_eq!(effective_lengths(&ui), vec![8.0, 8.0, 1.5, 1.75]);
    assert_eq!(
        serde_json::to_value(&ui.app.project.clips).unwrap(),
        stored_clips
    );
    assert_eq!(ui.app.undo_stack.len(), 1);
    ui.key(egui::Key::F5, egui::Modifiers::NONE);
    ui.capture("pattern-length-shared-follow-grown");
    ui.key(egui::Key::F7, egui::Modifiers::NONE);
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert_eq!(effective_lengths(&ui), vec![4.0, 4.0, 1.5, 1.75]);
    ui.key(egui::Key::Y, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), grown);
    select_note(&mut ui, added_id);
    ui.key(egui::Key::Delete, egui::Modifiers::NONE);
    assert_eq!(ui.app.undo_stack.len(), 2);
    assert_eq!(effective_lengths(&ui), vec![4.0, 4.0, 1.5, 1.75]);
    assert_eq!(
        serde_json::to_value(&ui.app.project.clips).unwrap(),
        stored_clips
    );
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), grown);
    ui.key(egui::Key::Y, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert_eq!(ui.app.undo_stack.len(), 2);
}

#[test]
fn playlist_follow_resize_noop_return_to_origin_commit_and_escape_keep_exact_history() {
    let mut ui = fixture();
    select_clip(&mut ui, 1);
    let before = project_fingerprint(&ui.app.project);
    let grip = |ui: &UiHarness| {
        ui.ctx
            .read_response(Id::new(("playlist-clip", 1_u32)).with("resize"))
            .unwrap()
            .rect
            .center()
    };
    let from = grip(&ui);
    pointer(&mut ui, from, Some(true));
    pointer(&mut ui, from, Some(false));
    ui.settle();
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert!(ui.app.undo_stack.is_empty());
    let to = from
        + Vec2::new(
            2.0 * ui.app.playlist_viewport.x.pixels_per_unit() as f32,
            0.0,
        );
    pointer(&mut ui, from, Some(true));
    pointer(&mut ui, to, None);
    assert_eq!(
        clip(&ui, 1).pattern_length_policy,
        Some(PatternLengthPolicy::Manual)
    );
    pointer(&mut ui, from, None);
    pointer(&mut ui, from, Some(false));
    ui.settle();
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert_eq!(clip(&ui, 1).length, 1.0);
    assert!(clip(&ui, 1).follows_pattern());
    assert!(ui.app.undo_stack.is_empty());
    let from = grip(&ui);
    ui.drag_pointer(from, to - from);
    assert_eq!(
        clip(&ui, 1).pattern_length_policy,
        Some(PatternLengthPolicy::Manual)
    );
    assert_eq!(clip(&ui, 1).length, 6.0);
    assert_eq!(ui.app.undo_stack.len(), 1);
    ui.capture("pattern-length-resize-manual-trim");
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert_eq!(ui.app.redo_stack.len(), 1);
    let from = grip(&ui);
    let to = from
        + Vec2::new(
            2.0 * ui.app.playlist_viewport.x.pixels_per_unit() as f32,
            0.0,
        );
    pointer(&mut ui, from, Some(true));
    pointer(&mut ui, to, None);
    assert_eq!(
        clip(&ui, 1).pattern_length_policy,
        Some(PatternLengthPolicy::Manual)
    );
    ui.key(egui::Key::Escape, egui::Modifiers::NONE);
    assert_eq!(project_fingerprint(&ui.app.project), before);
    pointer(&mut ui, to + Vec2::new(40.0, 0.0), None);
    pointer(&mut ui, to + Vec2::new(40.0, 0.0), Some(false));
    ui.settle();
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert!(ui.app.undo_stack.is_empty());
    assert_eq!(ui.app.redo_stack.len(), 1);
    assert!(ui.app.playlist_gesture_before.is_none());
    ui.capture("pattern-length-resize-cancelled-follow");
}

#[test]
fn inspector_follow_checkbox_is_one_reversible_policy_transaction() {
    let mut ui = fixture();
    let mut project = project();
    let late = PianoNote {
        id: 90_102,
        start: 4.25,
        ..project.active_pattern().notes[0].clone()
    };
    project.active_pattern_mut().notes.push(late);
    project.clips[0].pattern_length_policy = Some(PatternLengthPolicy::Manual);
    project.clips[0].length = 2.0;
    install(&mut ui, project);
    select_clip(&mut ui, 1);
    let before = project_fingerprint(&ui.app.project);
    let source = serde_json::to_value(ui.app.project.active_pattern()).unwrap();
    let pos = checkbox_pos(&ui);
    ui.click_pos(pos);
    assert!(clip(&ui, 1).follows_pattern());
    assert_eq!(ui.app.project.clip_length_beats(clip(&ui, 1)), 8.0);
    assert_eq!(ui.app.undo_stack.len(), 1);
    let follow = project_fingerprint(&ui.app.project);
    ui.capture("pattern-length-inspector-follow");
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), before);
    ui.key(egui::Key::Y, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), follow);
    let pos = checkbox_pos(&ui);
    ui.click_pos(pos);
    assert_eq!(
        clip(&ui, 1).pattern_length_policy,
        Some(PatternLengthPolicy::Manual)
    );
    assert_eq!(clip(&ui, 1).length, 8.0);
    assert_eq!(ui.app.undo_stack.len(), 2);
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), follow);
    assert_eq!(
        serde_json::to_value(ui.app.project.active_pattern()).unwrap(),
        source
    );
}

#[test]
fn inspector_length_typing_only_commits_manual_on_a_real_numeric_change() {
    let mut ui = fixture();
    select_clip(&mut ui, 1);
    let before = project_fingerprint(&ui.app.project);
    type_length(&mut ui, egui::Key::Num4, "4");
    assert_eq!(project_fingerprint(&ui.app.project), before);
    ui.key(egui::Key::Enter, egui::Modifiers::NONE);
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert!(ui.app.undo_stack.is_empty());
    type_length(&mut ui, egui::Key::Num6, "6");
    assert_eq!(project_fingerprint(&ui.app.project), before);
    for (stage, event) in [
        ("Escape press", Some(true)),
        ("Escape release", Some(false)),
        ("idle after deferred lost-focus", None),
    ] {
        let events = event.map_or_else(Vec::new, |pressed| {
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: Some(egui::Key::Escape),
                pressed,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }]
        });
        ui.run(events);
        assert_eq!(
            project_fingerprint(&ui.app.project),
            before,
            "{stage} committed canceled Inspector text: {:?}",
            serde_json::to_value(&ui.app.project.clips).unwrap()
        );
        assert_eq!(clip(&ui, 1).length, 1.0, "{stage}");
        assert!(clip(&ui, 1).follows_pattern(), "{stage}");
        assert!(ui.app.undo_stack.is_empty(), "{stage}");
        assert!(!ui.app.dirty, "{stage}");
    }
    assert!(!ui.ctx.text_edit_focused());
    type_length(&mut ui, egui::Key::Num6, "6");
    assert!(clip(&ui, 1).follows_pattern());
    ui.key(egui::Key::Enter, egui::Modifiers::NONE);
    assert!(!ui.ctx.text_edit_focused());
    assert_eq!(
        clip(&ui, 1).pattern_length_policy,
        Some(PatternLengthPolicy::Manual)
    );
    assert_eq!(clip(&ui, 1).length, 6.0);
    assert_eq!(ui.app.undo_stack.len(), 1);
    let entered = project_fingerprint(&ui.app.project);
    ui.capture("pattern-length-inspector-numeric-trim");
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert_eq!(clip(&ui, 1).length, 1.0);
    assert!(clip(&ui, 1).follows_pattern());

    // Ordinary pointer focus loss must still commit the text, unlike Escape.
    type_length(&mut ui, egui::Key::Num6, "6");
    assert_eq!(project_fingerprint(&ui.app.project), before);
    let length_label = ui
        .nodes
        .iter()
        .find(|node| node.role() == Role::Label && node.value() == Some("Length"))
        .expect("the actual Inspector Length label provides a neutral click-away target");
    ui.click_pos(node_rect(length_label).center());
    assert!(!ui.ctx.text_edit_focused());
    assert_eq!(
        project_fingerprint(&ui.app.project),
        entered,
        "click-away commits exactly the same six-beat Manual trim as Enter"
    );
    assert_eq!(
        clip(&ui, 1).pattern_length_policy,
        Some(PatternLengthPolicy::Manual)
    );
    assert_eq!(clip(&ui, 1).length, 6.0);
    assert_eq!(ui.app.undo_stack.len(), 1);
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert_eq!(clip(&ui, 1).length, 1.0);
    assert!(clip(&ui, 1).follows_pattern());
}

#[test]
fn native_automation_menu_uses_follow_extent_for_track_occupancy_after_growth_and_shrink() {
    for stored_length in [4.0, 8.0] {
        for grown in [true, false] {
            let mut ui = fixture();
            let mut project = project();
            project.clips.truncate(1);
            project.clips[0].length = stored_length;
            let late = PianoNote {
                id: 90_102,
                start: 6.0,
                ..project.active_pattern().notes[0].clone()
            };
            project.active_pattern_mut().notes.push(late);
            assert_eq!(project.clip_length_beats(&project.clips[0]), 8.0);
            if !grown {
                project.active_pattern_mut().notes.pop();
                assert_eq!(project.clip_length_beats(&project.clips[0]), 4.0);
            }
            install(&mut ui, project);
            let before = project_fingerprint(&ui.app.project);
            ui.app.beat_position = 6.0;
            assert_eq!(ui.app.playlist_viewport.y.visible_range().0.floor(), 0.0);
            ui.app.focus_editor(StudioView::Mixer);
            ui.settle();
            let control = ui
                .nodes
                .iter()
                .find(|node| node.label() == Some("Master · Pan"))
                .expect("actual native master pan menu");
            let pos = node_rect(control).center();
            for pressed in [true, false] {
                ui.run(vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Secondary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]);
            }
            ui.settle();
            ui.click("Create/open automation clip");
            let created_id = ui.app.selected_clip.expect("created automation selected");
            let created = clip(&ui, created_id);
            assert_eq!(created.kind, ClipKind::Automation);
            assert_eq!((created.start, created.length), (6.0, 4.0));
            assert_eq!(
                created.track,
                usize::from(grown),
                "raw={stored_length}, grown={grown}: source geometry determines occupancy"
            );
            assert_eq!(created.pattern_length_policy, None);
            assert_eq!(clip(&ui, 1).length, stored_length);
            assert!(clip(&ui, 1).follows_pattern());
            assert_eq!(ui.app.undo_stack.len(), 1);
            ui.key(egui::Key::Z, piano_clipboard_command());
            assert_eq!(project_fingerprint(&ui.app.project), before);
        }
    }
}

fn assert_cursor(ui: &mut UiHarness, expected: f32) {
    assert_eq!(ui.app.piano_editor_playhead(), Some(expected));
    ui.app
        .piano_viewport
        .x
        .reveal(f64::from(expected), f64::from(expected) + 0.25, 12.0)
        .unwrap();
    let output = ui.run(Vec::new());
    let grid = ui.ctx.read_response(Id::new("piano-grid")).unwrap().rect;
    let x = grid.left()
        + ui.app
            .piano_viewport
            .x
            .pixel_for_content(f64::from(expected)) as f32;
    fn has_cursor(shape: &egui::Shape, grid: Rect, x: f32) -> bool {
        match shape {
            egui::Shape::LineSegment { points, stroke } => {
                stroke.color == theme::ORANGE
                    && stroke.width == 1.6
                    && (points[0].x - x).abs() < 0.01
                    && (points[0].y - grid.top()).abs() < 0.01
                    && (points[1].y - grid.bottom()).abs() < 0.01
            }
            egui::Shape::Vec(shapes) => shapes.iter().any(|shape| has_cursor(shape, grid, x)),
            _ => false,
        }
    }
    assert!(
        output
            .shapes
            .iter()
            .any(|shape| has_cursor(&shape.shape, grid, x))
    );
}

#[test]
fn actual_playlist_open_maps_mixed_policy_song_and_pat_piano_cursors() {
    let mut ui = fixture();
    let mut project = project();
    project.clips = vec![
        policy_clip(1, 0.0, 12.0, None),
        policy_clip(2, 0.0, 12.0, Some(PatternLengthPolicy::Manual)),
        policy_clip(3, 0.0, 1.0, Some(PatternLengthPolicy::Follow)),
    ];
    project.clips[2].source_offset = 4.0;
    install(&mut ui, project);
    let before = project_fingerprint(&ui.app.project);
    open_piano(&mut ui, 1);
    ui.app.beat_position = 5.25; // Known paused clock, after actual Playlist opening.
    ui.app.piano_viewport.x.reveal(5.25, 5.5, 12.0).unwrap();
    assert_cursor(&mut ui, 5.25);
    open_piano(&mut ui, 2);
    ui.app.beat_position = 5.25;
    assert_cursor(&mut ui, 1.25);
    open_piano(&mut ui, 3);
    ui.app.beat_position = 3.25;
    assert_cursor(&mut ui, 3.25);
    ui.capture("pattern-length-mixed-song-source-cursor");
    ui.click("PAT");
    ui.app.beat_position = 5.25;
    assert_cursor(&mut ui, 1.25);
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert!(ui.app.undo_stack.is_empty());
}

fn prove_callback_loop(fixture: &mut KeyboardCallbackFixture, end: f64) {
    let source = fixture.ui.app.timeline_audio_source.as_ref().unwrap();
    assert_eq!(source.loop_end_beat, end);
    assert_eq!(
        source.loop_end_frame,
        source.tempo_map.beat_to_frame(end).unwrap()
    );
    assert_eq!(source.timeline.duration_frames(), source.loop_end_frame);
    let near_end = end - 0.25;
    fixture.ui.app.publish_transport_seek(near_end);
    fixture.until(|f| {
        f.ui.app.timeline_audio_matches_desired()
            && f.snapshot().transport_playing
            && f.ui.app.beat_position >= near_end as f32
    });
    assert!(fixture.snapshot().beat_position >= near_end);
    let loops = fixture.snapshot().transport_loop_count;
    fixture.until(|f| {
        f.snapshot().transport_loop_count > loops
            && f.snapshot().transport_playing
            && f.ui.app.timeline_audio_matches_desired()
    });
    assert!(fixture.ui.app.beat_position < 0.25);
    assert_eq!(
        fixture.ui.app.piano_editor_playhead(),
        Some(fixture.ui.app.beat_position)
    );
}

#[test]
fn callback_follow_source_edits_activate_real_new_song_ranges_without_forging_confirmation() {
    let mut fixture = KeyboardCallbackFixture::new();
    let mut project = project();
    project.clips.truncate(1);
    install(&mut fixture.ui, project);
    fixture.ui.app.request_immediate_timeline_compile();
    fixture.until(|f| f.ui.app.timeline_audio_matches_desired());
    open_piano(&mut fixture.ui, 1);
    fixture.ui.key(egui::Key::Space, egui::Modifiers::NONE);
    fixture.until(|f| f.snapshot().transport_playing && f.ui.app.timeline_audio_matches_desired());
    let original = fixture
        .ui
        .app
        .audio
        .as_ref()
        .unwrap()
        .confirmed_timeline_revision()
        .unwrap();
    let late = insert_late_note(&mut fixture.ui);
    assert_eq!(fixture.ui.app.project.song_end_beat(), 8.0);
    assert_eq!(
        fixture
            .ui
            .app
            .audio
            .as_ref()
            .unwrap()
            .confirmed_timeline_revision(),
        Some(original)
    );
    assert!(
        fixture.snapshot().transport_playing,
        "the old active revision still owns playback"
    );
    assert!(!fixture.ui.app.timeline_audio_matches_desired());
    assert_eq!(fixture.ui.app.piano_editor_playhead(), None);
    fixture.until(|f| {
        f.snapshot().transport_playing
            && f.ui.app.timeline_audio_matches_desired()
            && f.ui
                .app
                .audio
                .as_ref()
                .unwrap()
                .confirmed_timeline_revision()
                != Some(original)
    });
    assert_eq!(fixture.ui.app.project.clips[0].length, 1.0);
    fixture.ui.capture("pattern-length-callback-grown");
    prove_callback_loop(&mut fixture, 8.0);
    select_note(&mut fixture.ui, late);
    let grown = fixture
        .ui
        .app
        .audio
        .as_ref()
        .unwrap()
        .confirmed_timeline_revision()
        .unwrap();
    fixture.ui.key(egui::Key::Delete, egui::Modifiers::NONE);
    assert_eq!(fixture.ui.app.project.song_end_beat(), 4.0);
    assert_eq!(
        fixture
            .ui
            .app
            .audio
            .as_ref()
            .unwrap()
            .confirmed_timeline_revision(),
        Some(grown)
    );
    assert!(fixture.snapshot().transport_playing);
    assert!(!fixture.ui.app.timeline_audio_matches_desired());
    assert_eq!(fixture.ui.app.piano_editor_playhead(), None);
    fixture.until(|f| {
        f.snapshot().transport_playing
            && f.ui.app.timeline_audio_matches_desired()
            && f.ui
                .app
                .audio
                .as_ref()
                .unwrap()
                .confirmed_timeline_revision()
                != Some(grown)
    });
    fixture.ui.capture("pattern-length-callback-shrunk");
    prove_callback_loop(&mut fixture, 4.0);
    assert_eq!(fixture.ui.app.project.active_pattern().notes.len(), 1);
    assert!(fixture.ui.app.project.clips[0].follows_pattern());
    assert_eq!(fixture.ui.app.undo_stack.len(), 2);
    fixture.ui.key(egui::Key::Space, egui::Modifiers::NONE);
    fixture.until(|f| !f.snapshot().transport_playing && f.ui.app.timeline_audio_matches_desired());
}
