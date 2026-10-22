//! Production input and real device-free callback evidence for linked Pattern editors.
use super::*;

fn install_linked_project(ui: &mut UiHarness) {
    let mut project = Project::blank();
    project.auto_song_length = true;
    project.tempo = 240.0;
    let mut keys = project.channels[0].clone();
    keys.id = 2;
    keys.name = "Linked keys".into();
    project.channels.push(keys);
    let steps = &mut project.patterns[0];
    steps.name = "Step groove".into();
    steps.channel_steps = vec![[false; 16]; 2];
    steps.channel_steps[0][0] = true;
    let mut melody = steps.clone();
    melody.id = 2;
    melody.name = "Linked melody".into();
    melody.channel_steps.fill([false; 16]);
    melody.notes = vec![PianoNote {
        id: 20_001,
        channel_id: Some(2),
        group_id: None,
        note: 64,
        start: 6.0,
        length: 1.0,
        velocity: 0.8,
        selected: false,
        muted: false,
    }];
    project.patterns.push(melody);
    let mut step_clip = Project::default().clips[0].clone();
    step_clip.id = 10;
    step_clip.track = 0;
    step_clip.start = 0.0;
    step_clip.length = 4.0;
    step_clip.pattern_id = 1;
    step_clip.name = "Step instance".into();
    let mut melody_clip = step_clip.clone();
    melody_clip.id = 20;
    melody_clip.track = 1;
    melody_clip.start = 8.0;
    melody_clip.length = 8.0;
    melody_clip.source_offset = 4.0;
    melody_clip.pattern_id = 2;
    melody_clip.name = "First melody instance".into();
    let mut repeated = melody_clip.clone();
    repeated.id = 21;
    repeated.start = 20.0;
    repeated.name = "Repeated melody instance".into();
    project.clips = vec![step_clip, melody_clip, repeated];
    ui.app.project = project;
    ui.app.transport_mode = TransportMode::Song;
    ui.app.beat_position = 0.0;
    ui.app.selected_channel = 0;
    ui.app.focus_editor(StudioView::Playlist);
    ui.app.sync_history_observer();
    ui.app.project_fingerprint = project_fingerprint(&ui.app.project);
    ui.app.undo_stack.clear();
    ui.app.redo_stack.clear();
    ui.app.dirty = false;
    ui.settle();
}

fn clip_body(ui: &UiHarness, id: u32) -> Rect {
    ui.ctx
        .read_response(Id::new(("playlist-clip", id)))
        .unwrap()
        .rect
}

fn click_twice(ui: &mut UiHarness, pos: Pos2, modifiers: egui::Modifiers) {
    for _ in 0..2 {
        for pressed in [true, false] {
            ui.run_with_modifiers(
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers,
                    },
                ],
                modifiers,
            );
        }
    }
    ui.settle();
}

fn open_clip(ui: &mut UiHarness, id: u32, modifiers: egui::Modifiers) {
    ui.key(egui::Key::F5, egui::Modifiers::NONE);
    let pos = clip_body(ui, id).center();
    click_twice(ui, pos, modifiers);
}

fn choose_main_pattern(ui: &mut UiHarness, name: &str) {
    let node = ui
        .nodes
        .iter()
        .find(|node| {
            node.role() == Role::ComboBox
                && node.bounds().is_some_and(|b| b.y1 < 100.0)
                && node.value() == Some(ui.app.project.active_pattern().name.as_str())
        })
        .expect("shared main Pattern selector");
    let bounds = node.bounds().unwrap();
    ui.click_pos(Pos2::new(
        ((bounds.x0 + bounds.x1) * 0.5) as f32,
        ((bounds.y0 + bounds.y1) * 0.5) as f32,
    ));
    ui.click(name);
}

fn has_text(shape: &egui::Shape, text: &str) -> bool {
    match shape {
        egui::Shape::Text(shape) => shape.galley.text().contains(text),
        egui::Shape::Vec(shapes) => shapes.iter().any(|shape| has_text(shape, text)),
        _ => false,
    }
}

#[test]
fn playlist_pattern_double_click_auto_locates_notes_and_clicked_source_bar_without_seek() {
    let mut ui = UiHarness::new();
    install_linked_project(&mut ui);
    // Use the editing zoom to require actual source-bar scrolling. At the new
    // overview zoom this short phrase is already visible without any scroll.
    ui.app.piano_viewport.x.zoom_at_pixel(0.0, 2.5).unwrap();
    let clips = serde_json::to_value(&ui.app.project.clips).unwrap();
    let patterns = serde_json::to_value(&ui.app.project.patterns).unwrap();
    let canvas = ui
        .ctx
        .read_response(Id::new("playlist-canvas"))
        .unwrap()
        .rect;
    let pos = Pos2::new(
        canvas.left() + ui.app.playlist_viewport.x.pixel_for_content(10.0) as f32,
        clip_body(&ui, 20).center().y,
    );
    ui.click_pos(pos);
    assert_eq!(
        ui.app.workspace.focused,
        StudioView::Playlist,
        "one click still only selects"
    );
    ui.settle();
    click_twice(&mut ui, pos, egui::Modifiers::NONE);
    assert_eq!(ui.app.workspace.focused, StudioView::PianoRoll);
    assert_eq!(ui.app.project.active_pattern().id, 2);
    assert_eq!(
        ui.app.selected_channel, 1,
        "the Playlist track is not an instrument binding"
    );
    assert_eq!(ui.app.transport_mode, TransportMode::Song);
    assert_eq!(
        ui.app.beat_position, 0.0,
        "opening must not seek or start playback"
    );
    assert!(ui.app.piano_viewport.x.visible_range().0 > 0.0);
    assert!(ui.app.piano_viewport.x.visible_range().0 <= 6.0);
    assert!(ui.app.piano_viewport.x.visible_range().1 > 6.0);
    assert_eq!(serde_json::to_value(&ui.app.project.clips).unwrap(), clips);
    assert_eq!(
        serde_json::to_value(&ui.app.project.patterns).unwrap(),
        patterns
    );
    let output = ui.run(Vec::new());
    assert!(
        output
            .shapes
            .iter()
            .any(|s| has_text(&s.shape, "Linked keys — Linked melody"))
    );
    ui.capture("linked-pattern-open-piano");
}

#[test]
fn playlist_step_double_click_and_alt_reversal_keep_rack_piano_targets_shared() {
    let mut ui = UiHarness::new();
    install_linked_project(&mut ui);
    let music = serde_json::to_value(&ui.app.project.patterns).unwrap();
    open_clip(&mut ui, 10, egui::Modifiers::NONE);
    assert_eq!(ui.app.workspace.focused, StudioView::ChannelRack);
    assert_eq!(ui.app.project.active_pattern().id, 1);
    assert_eq!(
        ui.app.project.active_pattern().notes.len(),
        0,
        "steps are not converted to notes"
    );
    open_clip(&mut ui, 10, egui::Modifiers::ALT);
    assert_eq!(ui.app.workspace.focused, StudioView::PianoRoll);
    open_clip(&mut ui, 20, egui::Modifiers::ALT);
    assert_eq!(ui.app.workspace.focused, StudioView::ChannelRack);
    assert_eq!(ui.app.project.active_pattern().id, 2);
    assert_eq!(ui.app.selected_channel, 1);
    // The Rack's normal audition click remains selection, not forced navigation.
    ui.click("Channel 1");
    assert_eq!(ui.app.selected_channel, 0);
    assert_eq!(ui.app.workspace.focused, StudioView::ChannelRack);
    let bounds = ui.button("Linked keys").bounds().unwrap();
    let pos = Pos2::new(
        ((bounds.x0 + bounds.x1) * 0.5) as f32,
        ((bounds.y0 + bounds.y1) * 0.5) as f32,
    );
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
    ui.click("Piano roll");
    assert_eq!(ui.app.workspace.focused, StudioView::PianoRoll);
    assert_eq!(ui.app.selected_channel, 1);
    assert_eq!(
        serde_json::to_value(&ui.app.project.patterns).unwrap(),
        music
    );
}

fn preview_rectangles(output: &egui::FullOutput, rect: Rect) -> usize {
    fn count(shape: &egui::Shape, rect: Rect) -> usize {
        match shape {
            egui::Shape::Rect(shape) => {
                usize::from(shape.rect.height() == 2.5 && rect.contains(shape.rect.center()))
            }
            egui::Shape::Vec(shapes) => shapes.iter().map(|shape| count(shape, rect)).sum(),
            _ => 0,
        }
    }
    output
        .shapes
        .iter()
        .map(|shape| count(&shape.shape, rect))
        .sum()
}

fn compiled_notes(app: &CitrusApp) -> Vec<(u64, u8)> {
    let result = compile_timeline_job(TimelineCompileJob {
        generation: 1,
        fingerprint: timeline_playback_fingerprint(&app.project, app.transport_mode, 8_000),
        sample_rate: 8_000,
        project: transport_playback_project(&app.project, app.transport_mode),
        options: transport_timeline_compile_options(&app.project, app.transport_mode),
    })
    .result
    .unwrap();
    result
        .events()
        .iter()
        .filter_map(|event| match event.kind {
            crate::timeline::TimelineEventKind::NoteOn { note, .. } => Some((event.frame, note)),
            _ => None,
        })
        .collect()
}

#[test]
fn piano_edit_updates_every_playlist_reference_preview_and_compiled_notes_with_shared_undo() {
    let mut ui = UiHarness::new();
    install_linked_project(&mut ui);
    ui.app.project.active_pattern = 1;
    ui.app.active_pattern_selection_changed();
    ui.app.sync_history_observer();
    ui.app.project_fingerprint = project_fingerprint(&ui.app.project);
    ui.settle();
    let clips = serde_json::to_value(&ui.app.project.clips).unwrap();
    let output = ui.run(Vec::new());
    for id in [20, 21] {
        assert_eq!(preview_rectangles(&output, clip_body(&ui, id)), 1);
    }
    let before_notes = compiled_notes(&ui.app);
    open_clip(&mut ui, 20, egui::Modifiers::NONE);
    ui.app.piano_roll_state.tool = PianoRollTool::Draw;
    let grid = ui.ctx.read_response(Id::new("piano-grid")).unwrap().rect;
    let point = Pos2::new(
        grid.left() + ui.app.piano_viewport.x.pixel_for_content(5.0) as f32,
        grid.top()
            + ui.app
                .piano_viewport
                .y
                .pixel_for_content(piano_pitch_row(67) + 0.5) as f32,
    );
    assert!(grid.contains(point));
    ui.click_pos(point);
    assert_eq!(ui.app.project.active_pattern().notes.len(), 2);
    ui.key(egui::Key::F5, egui::Modifiers::NONE);
    let output = ui.run(Vec::new());
    for id in [20, 21] {
        assert_eq!(
            preview_rectangles(&output, clip_body(&ui, id)),
            2,
            "both occurrences must repaint from the same edited Pattern"
        );
    }
    let after_notes = compiled_notes(&ui.app);
    assert!(
        after_notes.contains(&(18_000, 67)),
        "start8 + source5 - offset4 = beat9"
    );
    assert!(
        after_notes.contains(&(42_000, 67)),
        "the repeated instance uses the same edit at beat21"
    );
    assert_eq!(
        serde_json::to_value(&ui.app.project.clips).unwrap(),
        clips,
        "source edits do not untrim or extend placements"
    );
    ui.capture("linked-pattern-shared-playlist-preview");
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(ui.app.project.active_pattern().notes.len(), 1);
    assert_eq!(compiled_notes(&ui.app), before_notes);
    ui.key(egui::Key::F6, egui::Modifiers::NONE);
    ui.key(egui::Key::Y, piano_clipboard_command());
    assert_eq!(compiled_notes(&ui.app), after_notes);
    assert_eq!(serde_json::to_value(&ui.app.project.clips).unwrap(), clips);
}

#[test]
fn main_pattern_selector_auto_locates_once_but_preserves_manual_channel_selection_on_idle() {
    let mut ui = UiHarness::new();
    install_linked_project(&mut ui);
    ui.key(egui::Key::F6, egui::Modifiers::NONE);
    choose_main_pattern(&mut ui, "Linked melody");
    assert_eq!(ui.app.project.active_pattern().id, 2);
    assert_eq!(ui.app.selected_channel, 1);
    ui.click("Channel 1");
    ui.key(egui::Key::F7, egui::Modifiers::NONE);
    assert_eq!(
        ui.app.selected_channel, 0,
        "F7 and idle frames must not override an explicit Rack target"
    );
    let output = ui.run(Vec::new());
    assert!(
        output
            .shapes
            .iter()
            .any(|s| has_text(&s.shape, "Channel 1 — Linked melody"))
    );
    choose_main_pattern(&mut ui, "Step groove");
    assert_eq!(ui.app.project.active_pattern().id, 1);
    assert_eq!(ui.app.selected_channel, 0);
    choose_main_pattern(&mut ui, "Linked melody");
    assert_eq!(ui.app.selected_channel, 1);
}

#[test]
fn pattern_editor_phase_preserves_independent_periods_overlap_context_and_session_identity() {
    let mut ui = UiHarness::new();
    install_linked_project(&mut ui);
    open_clip(&mut ui, 20, egui::Modifiers::NONE);
    ui.app.project.clips[1].length = 28.0;
    ui.app.project.clips[2].start = 9.0;
    ui.app.project.clips[2].length = 28.0;
    ui.app.project.clips[2].source_offset = 1.0;
    ui.app.beat_position = 10.25;
    assert_eq!(
        ui.app.piano_editor_playhead(),
        Some(6.25),
        "explicitly opened overlap wins"
    );
    ui.app.beat_position = 24.25;
    assert_eq!(
        ui.app.piano_editor_playhead(),
        Some(4.25),
        "SONG Piano repeats at sixteen, not four"
    );
    ui.app.playing = true;
    ui.app.transport_clock_playing = true;
    assert_eq!(
        ui.app.rack_editor_playing_step(),
        Some(1),
        "Rack still uses its four-beat period"
    );
    ui.app.playing = false;
    ui.app.project_session += 1;
    assert_eq!(
        ui.app.piano_editor_playhead(),
        Some(0.25),
        "stale origin must not bias a new session"
    );
    ui.app.project.clips[2].muted = true;
    assert_eq!(ui.app.piano_editor_playhead(), Some(4.25));
    ui.app.project.clips[1].muted = true;
    assert_eq!(ui.app.piano_editor_playhead(), None);
    ui.app.project.clips[1].muted = false;
    ui.app.beat_position = 36.0;
    assert_eq!(
        ui.app.piano_editor_playhead(),
        None,
        "placement end is exclusive"
    );
    ui.app.beat_position = 6.0;
    assert_eq!(
        ui.app.piano_editor_playhead(),
        None,
        "a gap cannot paint an unrelated global cursor"
    );
    ui.app.project.auto_song_length = false;
    ui.app.project.song_length_beats = 10.0;
    ui.app.beat_position = 10.0;
    assert_eq!(
        ui.app.piano_editor_playhead(),
        None,
        "manual end remains a real playback boundary"
    );
    ui.app.transport_mode = TransportMode::Pattern;
    ui.app.beat_position = 10.25;
    assert_eq!(
        ui.app.piano_editor_playhead(),
        Some(2.25),
        "PAT uses its actual eight-beat period"
    );
}

#[test]
fn deleted_occurrence_context_cannot_capture_a_newly_drawn_clip_reusing_its_id() {
    let mut ui = UiHarness::new();
    install_linked_project(&mut ui);
    open_clip(&mut ui, 21, egui::Modifiers::NONE);
    assert!(ui.app.pattern_editor_origin.is_some());
    ui.key(egui::Key::F5, egui::Modifiers::NONE);
    ui.key(egui::Key::Delete, egui::Modifiers::NONE);
    assert!(!ui.app.project.clips.iter().any(|clip| clip.id == 21));
    assert!(ui.app.pattern_editor_origin.is_none());
    let canvas = ui
        .ctx
        .read_response(Id::new("playlist-canvas"))
        .unwrap()
        .rect;
    let pos = Pos2::new(
        canvas.left() + ui.app.playlist_viewport.x.pixel_for_content(6.0) as f32,
        canvas.top() + ui.app.playlist_viewport.y.pixel_for_content(2.5) as f32,
    );
    ui.click_pos(pos);
    let created = ui
        .app
        .project
        .clips
        .iter()
        .find(|clip| clip.id == 21)
        .unwrap();
    assert_eq!((created.start, created.pattern_id), (6.0, 2));
    assert!(
        ui.app.pattern_editor_origin.is_none(),
        "Draw is not explicitly opening an occurrence"
    );
    ui.app.beat_position = 10.25;
    assert_eq!(
        ui.app.piano_editor_playhead(),
        Some(6.25),
        "latest-starting original clip wins, not a stale ID bound to the new earlier clip"
    );
}

#[test]
fn rack_cursor_samples_placed_swing_at_pair_start_like_the_actual_compiled_step() {
    let mut ui = UiHarness::new();
    install_linked_project(&mut ui);
    open_clip(&mut ui, 20, egui::Modifiers::ALT);
    ui.app.project.patterns[1].channel_steps[1][1] = true;
    let mut swing = AutomationLane::new(AutomationTarget::Swing);
    swing.set_curve(AutomationCurve::Hold);
    swing.replace_points([
        AutomationPoint::new(0.0, 0.0),
        AutomationPoint::new(0.3, 1.0),
    ]);
    ui.app.project.automation_lanes.push(ProjectAutomation {
        id: 77,
        name: "Pair-start swing".into(),
        lane: swing,
    });
    let mut placement = ui.app.project.clips[0].clone();
    placement.id = 30;
    placement.kind = ClipKind::Automation;
    placement.start = 8.0;
    placement.length = 4.0;
    placement.automation_id = Some(77);
    ui.app.project.clips.push(placement);
    assert!(
        compiled_notes(&ui.app).contains(&(16_500, 39)),
        "odd step is at beat8.25, before the Swing change at8.3"
    );
    ui.app.beat_position = 8.35;
    ui.app.playing = true;
    ui.app.transport_clock_playing = true;
    ui.app.effective_swing = 1.0;
    assert_eq!(
        ui.app.rack_editor_playing_step(),
        Some(1),
        "current Swing would incorrectly delay the display to8.4375"
    );
}

#[test]
fn rack_pair_sampling_keeps_f64_precision_before_a_nearby_automation_placement_edge() {
    let mut ui = UiHarness::new();
    install_linked_project(&mut ui);
    ui.app.project.active_pattern = 1;
    ui.app.project.patterns[1].channel_steps[1][1] = true;
    ui.app.project.clips[1].start = 1000.0;
    ui.app.project.clips[1].source_offset = 0.1;
    ui.app.project.clips[1].length = 4.0;
    let pair = 1000.0 - f64::from(0.1_f32);
    let mut swing = AutomationLane::new(AutomationTarget::Swing);
    swing.replace_points([AutomationPoint::new(0.0, 1.0)]);
    ui.app.project.automation_lanes.push(ProjectAutomation {
        id: 77,
        name: "Nearby Swing start".into(),
        lane: swing,
    });
    let mut placement = ui.app.project.clips[0].clone();
    placement.id = 30;
    placement.kind = ClipKind::Automation;
    placement.start = 999.9;
    placement.length = 4.0;
    placement.automation_id = Some(77);
    assert!(pair < f64::from(placement.start));
    assert_eq!(
        pair as f32, placement.start,
        "the legacy f32 query would cross this edge"
    );
    ui.app.project.clips.push(placement);
    let map = TempoMap::from_project(&ui.app.project, 8_000).unwrap();
    let actual_odd_frame = map.beat_to_frame(pair + 0.25).unwrap();
    assert!(compiled_notes(&ui.app).contains(&(actual_odd_frame, 39)));
    ui.app.beat_position = 1000.2;
    ui.app.playing = true;
    ui.app.transport_clock_playing = true;
    assert_eq!(ui.app.rack_editor_playing_step(), Some(1));
}

fn cursor_lines(output: &egui::FullOutput, grid: Rect) -> Vec<f32> {
    fn visit(shape: &egui::Shape, grid: Rect, lines: &mut Vec<f32>) {
        match shape {
            egui::Shape::LineSegment { points, stroke }
                if stroke.color == theme::ORANGE
                    && stroke.width == 1.6
                    && (points[0].y - grid.top()).abs() < 0.01
                    && (points[1].y - grid.bottom()).abs() < 0.01 =>
            {
                lines.push(points[0].x)
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    visit(shape, grid, lines);
                }
            }
            _ => {}
        }
    }
    let mut lines = Vec::new();
    for shape in &output.shapes {
        visit(&shape.shape, grid, &mut lines);
    }
    lines
}

#[test]
fn real_callback_clock_links_song_seek_and_pat_loop_to_piano_cursor_and_rack_step() {
    let mut fixture = piano_keyboard_playback_tests::KeyboardCallbackFixture::new();
    install_linked_project(&mut fixture.ui);
    fixture.ui.app.request_immediate_timeline_compile();
    fixture.until(|f| f.ui.app.timeline_audio_matches_desired());
    open_clip(&mut fixture.ui, 20, egui::Modifiers::NONE);
    fixture.ui.key(egui::Key::Space, egui::Modifiers::NONE);
    fixture.until(|f| f.snapshot().transport_playing && f.ui.app.timeline_audio_matches_desired());
    fixture.ui.app.publish_transport_seek(10.25);
    fixture.until(|f| {
        f.snapshot().transport_playing
            && f.ui.app.timeline_audio_matches_desired()
            && f.ui.app.beat_position >= 10.25
    });
    let local = fixture.ui.app.beat_position - 8.0 + 4.0;
    assert!((fixture.ui.app.piano_editor_playhead().unwrap() - local).abs() < 0.0001);
    let output = fixture.ui.run(Vec::new());
    let grid = fixture
        .ui
        .ctx
        .read_response(Id::new("piano-grid"))
        .unwrap()
        .rect;
    let lines = cursor_lines(&output, grid);
    assert_eq!(lines.len(), 1);
    let expected_x = grid.left()
        + fixture
            .ui
            .app
            .piano_viewport
            .x
            .pixel_for_content(f64::from(local)) as f32;
    assert!(
        (lines[0] - expected_x).abs() < 0.01,
        "render must consume the mapped callback position"
    );
    fixture.ui.capture("linked-pattern-song-local-cursor");
    fixture.ui.key(egui::Key::F6, egui::Modifiers::NONE);
    assert_eq!(
        fixture.ui.app.last_step,
        usize::MAX,
        "callback owns scheduling; display must not use this cursor"
    );
    let step = ((local % 4.0) * 4.0).floor() as usize;
    assert_eq!(fixture.ui.app.rack_editor_playing_step(), Some(step));
    let output = fixture.ui.run(Vec::new());
    let bounds = fixture
        .ui
        .button(&format!("Linked keys step {}", step + 1))
        .bounds()
        .unwrap();
    assert!(
        output
            .shapes
            .iter()
            .any(|s| matches!(&s.shape, egui::Shape::Rect(r)
        if r.stroke.color == Color32::WHITE && r.stroke.width == 1.0
            && (f64::from(r.rect.left()) - bounds.x0).abs() < 0.01
            && (f64::from(r.rect.top()) - bounds.y0).abs() < 0.01))
    );
    fixture.ui.capture("linked-pattern-rack-callback-step");
    fixture.ui.app.publish_transport_seek(6.0);
    fixture.until(|f| {
        f.snapshot().transport_playing
            && f.ui.app.timeline_audio_matches_desired()
            && (6.0..7.0).contains(&f.ui.app.beat_position)
    });
    assert_eq!(fixture.ui.app.rack_editor_playing_step(), None);
    fixture.ui.key(egui::Key::F7, egui::Modifiers::NONE);
    let output = fixture.ui.run(Vec::new());
    let grid = fixture
        .ui
        .ctx
        .read_response(Id::new("piano-grid"))
        .unwrap()
        .rect;
    assert!(cursor_lines(&output, grid).is_empty());
    fixture.ui.click("PAT");
    fixture.until(|f| f.snapshot().transport_playing && f.ui.app.timeline_audio_matches_desired());
    fixture.ui.app.publish_transport_seek(6.5);
    fixture.until(|f| {
        f.snapshot().transport_playing
            && f.ui.app.timeline_audio_matches_desired()
            && f.ui.app.beat_position >= 6.5
    });
    assert!(
        (fixture.ui.app.piano_editor_playhead().unwrap() - fixture.ui.app.beat_position).abs()
            < 0.0001
    );
    let loop_count = fixture.snapshot().transport_loop_count;
    let deadline = Instant::now() + Duration::from_secs(5);
    while fixture.snapshot().transport_loop_count == loop_count && Instant::now() < deadline {
        fixture.pump(crate::audio::MAX_MIXER_BLOCK_FRAMES);
    }
    assert!(fixture.snapshot().transport_loop_count > loop_count);
    fixture.until(|f| f.snapshot().transport_playing && f.ui.app.timeline_audio_matches_desired());
    assert!(
        (fixture.ui.app.piano_editor_playhead().unwrap() - fixture.ui.app.beat_position).abs()
            < 0.0001
    );
    fixture.ui.app.stop();
    fixture.until(|f| !f.snapshot().transport_playing && f.ui.app.timeline_audio_matches_desired());
    assert_eq!(fixture.ui.app.rack_editor_playing_step(), None);
}
