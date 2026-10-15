use super::*;

fn rect(ui: &UiHarness, id: &str) -> Rect {
    ui.ctx
        .read_response(Id::new(id))
        .expect("production Playlist widget")
        .rect
}

fn ruler_point(ui: &UiHarness, beat: f64) -> Pos2 {
    let ruler = rect(ui, "playlist-time-ruler");
    Pos2::new(
        ruler.left() + ui.app.playlist_viewport.x.pixel_for_content(beat) as f32,
        ruler.center().y,
    )
}

fn contains_text(shape: &egui::Shape, expected: &str) -> bool {
    match shape {
        egui::Shape::Text(text) => text.galley.text().contains(expected),
        egui::Shape::Vec(shapes) => shapes.iter().any(|shape| contains_text(shape, expected)),
        _ => false,
    }
}

fn assert_painted(output: &egui::FullOutput, expected: &str) {
    assert!(
        output
            .shapes
            .iter()
            .any(|shape| contains_text(&shape.shape, expected)),
        "missing visible transport feedback {expected:?}"
    );
}

fn playback_notes(app: &CitrusApp) -> Vec<(u64, u8)> {
    let timeline = compile_timeline_job(TimelineCompileJob {
        generation: 1,
        fingerprint: timeline_playback_fingerprint(&app.project, app.transport_mode, 8_000),
        sample_rate: 8_000,
        project: transport_playback_project(&app.project, app.transport_mode),
        options: transport_timeline_compile_options(&app.project, app.transport_mode),
    })
    .result
    .unwrap();
    timeline
        .events()
        .iter()
        .filter_map(|event| match event.kind {
            crate::timeline::TimelineEventKind::NoteOn { note, .. } => Some((event.frame, note)),
            _ => None,
        })
        .collect()
}

fn two_patterns(ui: &mut UiHarness) {
    let mut project = Project::blank();
    project.auto_song_length = true;
    project.tempo = 120.0;
    let channel = project.channels[0].id;
    let mut first = project.active_pattern().clone();
    first.name = "Low motif".into();
    first.notes = vec![PianoNote {
        id: 1,
        channel_id: Some(channel),
        group_id: None,
        note: 60,
        start: 0.0,
        length: 1.0,
        velocity: 0.8,
        selected: false,
        muted: false,
    }];
    first.channel_steps.fill([false; 16]);
    let mut second = first.clone();
    second.id = 2;
    second.name = "High answer".into();
    second.notes[0].id = 2;
    second.notes[0].note = 72;
    let mut clip = Project::default().clips[0].clone();
    clip.id = 1;
    clip.start = 0.0;
    clip.length = 4.0;
    clip.pattern_id = first.id;
    let mut answer = clip.clone();
    answer.id = 2;
    answer.start = 8.0;
    answer.pattern_id = second.id;
    project.patterns = vec![first, second];
    project.clips = vec![clip, answer];
    ui.app.project = project;
    ui.app.transport_mode = TransportMode::Song;
    ui.app.sync_history_observer();
    // This replacement is the test's saved starting project, not an unsaved
    // edit of the startup demo. History and the saved baseline are independent.
    ui.app.project_fingerprint = project_fingerprint(&ui.app.project);
    ui.settle();
}

fn assert_clean_after_due_dirty_check(ui: &mut UiHarness, expected: u64) {
    ui.app.last_dirty_check = Instant::now() - Duration::from_secs(1);
    ui.run(Vec::new());
    assert_eq!(project_fingerprint(&ui.app.project), expected);
    assert!(
        !ui.app.dirty,
        "navigation changed dirty state: saved={}, history={}, actual={}, payload={:?}",
        ui.app.project_fingerprint,
        ui.app.history_fingerprint,
        project_fingerprint(&ui.app.project),
        serde_json::to_value(&ui.app.project).unwrap()
    );
}

#[test]
fn playlist_ruler_drag_continuously_seeks_without_drawing_when_pointer_crosses_lanes() {
    let mut ui = UiHarness::new();
    two_patterns(&mut ui);
    let before = project_fingerprint(&ui.app.project);
    assert_clean_after_due_dirty_check(&mut ui, before);
    let count = ui.app.project.clips.len();
    let origin = ruler_point(&ui, 1.0);
    ui.run(mixer_pointer_button(origin, true));
    assert_eq!(ui.app.beat_position, 1.0);
    for beat in [2.5, 5.0, 9.25] {
        let target = ruler_point(&ui, beat);
        ui.run(vec![egui::Event::PointerMoved(target)]);
        assert_eq!(
            ui.app.beat_position, beat as f32,
            "every held movement must seek"
        );
    }
    let canvas = rect(&ui, "playlist-canvas");
    let lane = Pos2::new(
        ruler_point(&ui, 10.5).x,
        canvas.top() + 2.5 * ui.app.playlist_viewport.y.pixels_per_unit() as f32,
    );
    ui.run(vec![egui::Event::PointerMoved(lane)]);
    assert_eq!(ui.app.beat_position, 10.5);
    assert_eq!(ui.app.project.clips.len(), count);
    ui.run(mixer_pointer_button(lane, false));
    ui.settle();
    assert_clean_after_due_dirty_check(&mut ui, before);
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert!(ui.app.undo_stack.is_empty());
    assert!(!ui.app.dirty);
    assert!(ui.app.playlist_gesture_before.is_none());
    // The same lane is a fresh Draw target only after the ruler's release.
    ui.click_pos(lane);
    assert_eq!(ui.app.project.clips.len(), count + 1);
    assert_eq!(ui.app.undo_stack.len(), 1);
}

#[test]
fn playlist_clean_fixture_adopts_its_saved_baseline_and_real_music_edit_still_becomes_dirty() {
    let mut ui = UiHarness::new();
    let startup_fingerprint = ui.app.project_fingerprint;
    let startup = serde_json::to_value(&ui.app.project).unwrap();
    two_patterns(&mut ui);
    let clean = project_fingerprint(&ui.app.project);
    assert_ne!(startup_fingerprint, clean);
    eprintln!(
        "startup_name={} patterns={} clips={} saved_fingerprint={}; fixture_name={} patterns={} clips={} fingerprint={}",
        startup["name"],
        startup["patterns"].as_array().unwrap().len(),
        startup["clips"].as_array().unwrap().len(),
        startup_fingerprint,
        ui.app.project.name,
        ui.app.project.patterns.len(),
        ui.app.project.clips.len(),
        clean
    );
    assert_clean_after_due_dirty_check(&mut ui, clean);
    // Reproduce the old fixture exactly: its music/history belonged to the new
    // project, while the saved baseline still belonged to the startup demo.
    ui.app.project_fingerprint = startup_fingerprint;
    ui.app.last_dirty_check = Instant::now() - Duration::from_secs(1);
    ui.run(Vec::new());
    assert!(
        ui.app.dirty,
        "the old mismatched baseline dirties on idle, without any navigation"
    );
    assert_eq!(project_fingerprint(&ui.app.project), clean);
    assert!(ui.app.undo_stack.is_empty());
    drop(ui);

    let mut edited = UiHarness::new();
    two_patterns(&mut edited);
    let clean = project_fingerprint(&edited.app.project);
    assert_clean_after_due_dirty_check(&mut edited, clean);
    edited.key(egui::Key::F6, egui::Modifiers::NONE);
    let name = edited.app.project.channels[0].name.clone();
    edited.click(&format!("{name} step 1"));
    assert!(edited.app.project.active_pattern().channel_steps[0][0]);
    edited.key(egui::Key::F5, egui::Modifiers::NONE);
    edited.app.last_dirty_check = Instant::now() - Duration::from_secs(1);
    edited.run(Vec::new());
    assert!(
        edited.app.dirty,
        "an actual step edit must still dirty the saved project"
    );
    assert_ne!(project_fingerprint(&edited.app.project), clean);
    assert_eq!(
        edited.app.project_fingerprint, clean,
        "editing must not overwrite the saved baseline"
    );
    assert!(!edited.app.undo_stack.is_empty());
}

#[test]
fn playlist_ruler_drag_keeps_playing_and_respects_the_mode_range_and_snap() {
    let mut ui = UiHarness::new();
    two_patterns(&mut ui);
    ui.app.playing = true;
    let origin = ruler_point(&ui, 1.0);
    ui.run(mixer_pointer_button(origin, true));
    let beyond = ruler_point(&ui, 14.0);
    ui.run(vec![egui::Event::PointerMoved(beyond)]);
    assert_eq!(ui.app.beat_position, 12.0);
    assert!(ui.app.playing, "seek is not Stop or Pause");
    ui.run(mixer_pointer_button(beyond, false));
    ui.app.playing = false;
    ui.click("PAT");
    let target = ruler_point(&ui, 6.5);
    ui.run(mixer_pointer_button(target, true));
    assert_eq!(
        ui.app.beat_position, 2.5,
        "PAT ruler positions wrap into its four-beat loop"
    );
    ui.run(mixer_pointer_button(target, false));
}

#[test]
fn transport_pat_song_and_pattern_selection_change_real_compiled_notes_and_visible_scope() {
    let mut ui = UiHarness::new();
    two_patterns(&mut ui);
    let before = project_fingerprint(&ui.app.project);
    let focused = ui.app.workspace.focused;
    assert_eq!(playback_notes(&ui.app), vec![(0, 60), (32_000, 72)]);
    ui.click("PAT");
    assert_eq!(ui.app.transport_mode, TransportMode::Pattern);
    assert_eq!(ui.app.workspace.focused, focused);
    assert_eq!(playback_notes(&ui.app), vec![(0, 60)]);
    assert_painted(&ui.run(Vec::new()), "PAT · 1 bar loop");
    // Use the actual main Pattern chooser, not an assignment to active_pattern.
    let chooser = ui
        .nodes
        .iter()
        .find(|node| {
            node.role() == Role::ComboBox
                && node.value() == Some("Low motif")
                && node.bounds().is_some_and(|bounds| bounds.y1 < 100.0)
        })
        .expect("main Pattern chooser");
    let bounds = chooser.bounds().unwrap();
    ui.click_pos(Pos2::new(
        ((bounds.x0 + bounds.x1) * 0.5) as f32,
        ((bounds.y0 + bounds.y1) * 0.5) as f32,
    ));
    ui.click("High answer");
    assert_eq!(ui.app.project.active_pattern, 1);
    assert_eq!(playback_notes(&ui.app), vec![(0, 72)]);
    assert_painted(&ui.run(Vec::new()), "PAT · High answer · 1 bar loop");
    assert_eq!(
        ui.app.toast.as_ref().unwrap().0,
        "PAT mode — looping the selected Pattern only",
        "mode feedback must not retain the previous Pattern name"
    );
    ui.capture("transport-pattern-mode");
    // Pattern selection is already persisted selection data; only mode is a preference.
    let selection = project_fingerprint(&ui.app.project);
    ui.click("SONG");
    assert_eq!(playback_notes(&ui.app), vec![(0, 60), (32_000, 72)]);
    assert_eq!(project_fingerprint(&ui.app.project), selection);
    assert_eq!(ui.app.workspace.focused, focused);
    assert_painted(&ui.run(Vec::new()), "SONG · 3 bars · AUTO");
    ui.capture("transport-song-mode");
    ui.app.project.active_pattern = 0;
    assert_eq!(
        project_fingerprint(&ui.app.project),
        before,
        "mode switches must not edit music"
    );
}

#[test]
fn transport_mode_recording_guard_keyboard_preference_and_right_click_navigation_remain() {
    let mut ui = UiHarness::new();
    two_patterns(&mut ui);
    let before = project_fingerprint(&ui.app.project);
    ui.app.recording = true;
    ui.click("PAT");
    assert_eq!(ui.app.transport_mode, TransportMode::Song);
    ui.app.recording = false;
    ui.key(egui::Key::L, egui::Modifiers::NONE);
    assert_eq!(ui.app.transport_mode, TransportMode::Pattern);
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert!(ui.app.undo_stack.is_empty());
    let pos = node_rect(ui.button("SONG")).center();
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
    assert_eq!(ui.app.workspace.focused, StudioView::Playlist);
    assert_eq!(
        ui.app.transport_mode,
        TransportMode::Pattern,
        "right-click only opens the editor"
    );
    let mut storage = WorkspaceTestStorage::default();
    ui.app.save(&mut storage);
    assert_eq!(
        decode_transport_mode(
            eframe::Storage::get_string(&storage, TRANSPORT_MODE_STORAGE_KEY).as_deref()
        ),
        TransportMode::Pattern
    );
}

#[test]
fn playlist_zoom_is_not_song_length_and_auto_range_toggle_has_exact_undo() {
    let mut ui = UiHarness::new();
    two_patterns(&mut ui);
    let before = project_fingerprint(&ui.app.project);
    assert_eq!(ui.app.project.song_end_beat(), 12.0);
    for label in ["1 BAR", "8 BARS", "4 BARS", "FIT"] {
        ui.click(label);
        assert_eq!(ui.app.project.song_end_beat(), 12.0);
        assert_eq!(project_fingerprint(&ui.app.project), before);
    }
    assert!(ui.app.undo_stack.is_empty());
    ui.click("AUTO");
    assert!(!ui.app.project.auto_song_length);
    assert_eq!(ui.app.project.song_length_beats, 12.0);
    assert_eq!(ui.app.undo_stack.len(), 1);
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), before);
    ui.key(egui::Key::Y, piano_clipboard_command());
    assert!(!ui.app.project.auto_song_length);
    assert_eq!(ui.app.project.song_end_beat(), 12.0);
}

#[test]
fn playlist_drag_moves_beyond_old_end_and_auto_range_shrinks_on_delete_with_history() {
    let mut ui = UiHarness::new();
    two_patterns(&mut ui);
    ui.app.project.clips.truncate(1);
    ui.app.project.song_length_beats = 4.0;
    ui.app.sync_history_observer();
    ui.settle();
    let before = project_fingerprint(&ui.app.project);
    let clip = ui
        .ctx
        .read_response(Id::new(("playlist-clip", 1_u32)))
        .unwrap()
        .rect;
    let origin = Pos2::new(clip.left() + clip.width() * 0.5, clip.top() + 12.0);
    ui.drag_pointer(
        origin,
        Vec2::new(
            8.0 * ui.app.playlist_viewport.x.pixels_per_unit() as f32,
            0.0,
        ),
    );
    assert_eq!(ui.app.project.clips[0].start, 8.0);
    assert_eq!(ui.app.project.clips[0].length, 4.0);
    assert_eq!(ui.app.project.song_end_beat(), 12.0);
    assert_eq!(ui.app.undo_stack.len(), 1);
    ui.key(egui::Key::Delete, egui::Modifiers::NONE);
    assert!(ui.app.project.clips.is_empty());
    assert_eq!(ui.app.project.song_end_beat(), 4.0);
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(ui.app.project.song_end_beat(), 12.0);
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), before);
    ui.key(egui::Key::Y, piano_clipboard_command());
    assert_eq!(ui.app.project.song_end_beat(), 12.0);
}

#[test]
fn playlist_resize_beyond_old_end_changes_auto_playback_and_not_the_source_pattern() {
    let mut ui = UiHarness::new();
    two_patterns(&mut ui);
    ui.app.project.clips.truncate(1);
    ui.app.project.song_length_beats = 4.0;
    ui.app.sync_history_observer();
    ui.settle();
    let source = serde_json::to_value(ui.app.project.active_pattern()).unwrap();
    let resize = ui
        .ctx
        .read_response(Id::new(("playlist-clip", 1_u32)).with("resize"))
        .unwrap()
        .rect
        .center();
    ui.drag_pointer(
        resize,
        Vec2::new(
            8.0 * ui.app.playlist_viewport.x.pixels_per_unit() as f32,
            0.0,
        ),
    );
    assert_eq!(ui.app.project.clips[0].length, 12.0);
    assert_eq!(ui.app.project.song_end_beat(), 12.0);
    assert_eq!(
        serde_json::to_value(ui.app.project.active_pattern()).unwrap(),
        source
    );
    assert_eq!(playback_notes(&ui.app), vec![(0, 60)]);
    assert_eq!(
        TempoMap::from_project(&ui.app.project, 8_000)
            .unwrap()
            .duration_frames(),
        48_000
    );
    assert_eq!(ui.app.undo_stack.len(), 1);
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(ui.app.project.clips[0].length, 4.0);
    assert_eq!(ui.app.project.song_end_beat(), 4.0);
}

#[test]
fn playlist_new_song_defaults_to_auto_and_non_tempo_clip_end_invalidates_tempo_cache() {
    let mut ui = UiHarness::new();
    ui.key(egui::Key::N, piano_clipboard_command());
    assert!(ui.app.project.auto_song_length);
    assert!(ui.app.project.clips.is_empty());
    assert_eq!(ui.app.project.song_end_beat(), 4.0);
    two_patterns(&mut ui);
    let before = transport_tempo_map_fingerprint(&ui.app.project, TransportMode::Song);
    let pattern_before = transport_tempo_map_fingerprint(&ui.app.project, TransportMode::Pattern);
    ui.app.project.clips[1].start = 16.0;
    assert_eq!(ui.app.project.song_end_beat(), 20.0);
    assert_ne!(
        transport_tempo_map_fingerprint(&ui.app.project, TransportMode::Song),
        before
    );
    assert_eq!(
        transport_tempo_map_fingerprint(&ui.app.project, TransportMode::Pattern),
        pattern_before
    );
    let timeline_before = timeline_compile_fingerprint(&ui.app.project, 8_000);
    ui.app.project.auto_song_length = false;
    assert_ne!(
        timeline_compile_fingerprint(&ui.app.project, 8_000),
        timeline_before
    );
}

#[test]
fn playlist_auto_automation_toolbar_uses_dragged_song_playhead_and_remains_saveable() {
    let mut ui = UiHarness::new();
    two_patterns(&mut ui);
    ui.app.project.clips.truncate(1);
    ui.app.project.clips[0].start = 92.0;
    ui.app.project.song_length_beats = 64.0;
    ui.app.sync_history_observer();
    set_axis_visible_units(&mut ui.app.playlist_viewport.x, 112.0);
    ui.settle();
    let clip = ui
        .ctx
        .read_response(Id::new(("playlist-clip", 1_u32)))
        .unwrap()
        .rect;
    ui.drag_pointer(
        clip.center(),
        Vec2::new(
            8.0 * ui.app.playlist_viewport.x.pixels_per_unit() as f32,
            0.0,
        ),
    );
    assert_eq!(ui.app.project.clips[0].start, 100.0);
    assert_eq!(ui.app.project.song_end_beat(), 104.0);
    assert_eq!(
        ui.app.project.song_length_beats, 64.0,
        "the old cache did not grow with the drag"
    );
    let pointer = ruler_point(&ui, 100.0);
    ui.click_pos(pointer);
    assert_eq!(ui.app.beat_position, 100.0);
    let routes = ui.app.project.audio_clip_mixer_destinations.clone();
    ui.click("+ AUTOMATION");
    let created = ui
        .app
        .project
        .clips
        .iter()
        .find(|clip| Some(clip.id) == ui.app.selected_clip)
        .unwrap();
    assert_eq!(created.kind, ClipKind::Automation);
    assert_eq!((created.start, created.length), (100.0, 4.0));
    assert_eq!(
        ui.app.project.audio_clip_mixer_destinations, routes,
        "automation must not acquire an Audio Clip destination"
    );
    ui.app.project.validate_mixer_graph().unwrap();
    let fixture = Fixture::new();
    let path = fixture.0.join("auto-automation.citrus");
    ui.app.project.save(&path).unwrap();
    let reopened = Project::load(&path).unwrap();
    assert_eq!(reopened.song_end_beat(), 104.0);
    assert!(
        reopened
            .clips
            .iter()
            .any(|clip| clip.kind == ClipKind::Automation && clip.start == 100.0)
    );
}

#[test]
fn playlist_automation_creation_entries_share_auto_coordinates_and_short_manual_bounds() {
    for (automatic, requested, expected) in
        [(true, 100.0, (100.0, 4.0)), (false, 0.125, (0.0, 0.125))]
    {
        let mut ui = UiHarness::new();
        ui.app.project = Project::blank();
        ui.app.project.auto_song_length = automatic;
        ui.app.project.song_length_beats = if automatic { 64.0 } else { 0.125 };
        ui.app.snap = 1.0;
        ui.app.beat_position = requested;
        ui.app.sync_history_observer();
        let original_manual_endpoint = ui.app.project.song_length_beats;
        ui.settle();
        assert_eq!(
            ui.app.project.song_length_beats, original_manual_endpoint,
            "idle before creation must preserve the manual endpoint"
        );
        let button = node_rect(ui.button("+ AUTOMATION")).center();
        ui.run(mixer_pointer_button(button, true));
        assert!(
            ui.app.project.clips.is_empty(),
            "the button creates on release"
        );
        assert_eq!(
            ui.app.project.song_length_beats, original_manual_endpoint,
            "press must not change the manual endpoint"
        );
        ui.run(mixer_pointer_button(button, false));
        let released = ui.app.project.clips.last().unwrap();
        assert_eq!(
            (released.start, released.length),
            expected,
            "creation before the next Inspector pass"
        );
        assert_eq!(
            ui.app.project.song_length_beats, original_manual_endpoint,
            "release must preserve the manual endpoint"
        );
        for frame in 0..3 {
            ui.run(Vec::new());
            let shown = ui.app.project.clips.last().unwrap();
            assert_eq!(
                (shown.start, shown.length),
                expected,
                "Inspector idle frame {frame} must preserve the created clip"
            );
            assert_eq!(
                ui.app.project.song_length_beats, original_manual_endpoint,
                "Inspector idle frame {frame} must preserve the manual endpoint"
            );
        }
        let clip = ui.app.project.clips.last().unwrap();
        assert_eq!((clip.start, clip.length), expected);
        ui.app.project.validate_mixer_graph().unwrap();

        let mut project = Project::blank();
        project.auto_song_length = automatic;
        project.song_length_beats = if automatic { 64.0 } else { 0.125 };
        let instance_id = config_capture_plugin(
            &mut project,
            PluginPickerTarget::MixerSlot { track: 1, slot: 0 },
            100,
        );
        let selection = ensure_plugin_parameter_automation(
            &mut project,
            instance_id,
            17,
            0.375,
            "Cutoff",
            requested,
            1.0,
            3,
        )
        .unwrap();
        let clip = project
            .clips
            .iter()
            .find(|clip| clip.id == selection.clip_id)
            .unwrap();
        assert_eq!((clip.start, clip.length), expected);
        let before = project_fingerprint(&project);
        let reopened = ensure_plugin_parameter_automation(
            &mut project,
            instance_id,
            17,
            0.9,
            "Cutoff",
            0.0,
            0.25,
            9,
        )
        .unwrap();
        assert_eq!(reopened.clip_id, selection.clip_id);
        assert!(!reopened.created_clip && !reopened.created_lane);
        assert_eq!(
            project_fingerprint(&project),
            before,
            "opening an existing placement does not move or recreate it"
        );

        ui.app.project = Project::blank();
        ui.app.project.auto_song_length = automatic;
        ui.app.project.song_length_beats = if automatic { 64.0 } else { 0.125 };
        ui.app.beat_position = requested;
        ui.app.sync_history_observer();
        ui.app
            .create_or_open_native_automation(AutomationTarget::MasterPan);
        let clip = ui
            .app
            .project
            .clips
            .iter()
            .find(|clip| Some(clip.id) == ui.app.selected_clip)
            .unwrap();
        assert_eq!((clip.start, clip.length), expected);
        ui.app.project.validate_mixer_graph().unwrap();
    }
}

#[test]
fn playlist_auto_native_automation_context_menu_keeps_late_playhead_and_undo() {
    let mut ui = UiHarness::new();
    two_patterns(&mut ui);
    ui.app.project.clips[1].start = 100.0;
    ui.app.beat_position = 100.0;
    ui.app.project.song_length_beats = 64.0;
    ui.app.sync_history_observer();
    let before = project_fingerprint(&ui.app.project);
    ui.app.focus_editor(StudioView::Mixer);
    ui.settle();
    let control = ui
        .nodes
        .iter()
        .find(|node| node.label() == Some("Master · Pan"))
        .expect("production Master pan control");
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
    ui.click("Create/open automation clip");
    let clip = ui
        .app
        .project
        .clips
        .iter()
        .find(|clip| Some(clip.id) == ui.app.selected_clip)
        .unwrap();
    assert_eq!((clip.start, clip.length), (100.0, 4.0));
    assert_eq!(ui.app.undo_stack.len(), 1);
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), before);
    ui.key(egui::Key::Y, piano_clipboard_command());
    assert!(
        ui.app
            .project
            .clips
            .iter()
            .any(|clip| clip.kind == ClipKind::Automation && clip.start == 100.0)
    );
}

#[test]
fn transport_scope_layout_stays_readable_in_minimum_floating_and_wide_workspaces() {
    let mut ui = UiHarness::floating();
    two_patterns(&mut ui);
    for (size, capture) in [
        (Vec2::new(1280.0, 800.0), "transport-scope-minimum"),
        (Vec2::new(1920.0, 1080.0), "transport-scope-wide"),
    ] {
        ui.size = size;
        ui.app.focus_editor(StudioView::Playlist);
        ui.settle();
        for label in ["PAT", "SONG", "AUTO"] {
            let bounds = node_rect(ui.button(label));
            assert!(Rect::from_min_size(Pos2::ZERO, size).contains_rect(bounds));
            assert!(bounds.width() >= 25.0 && bounds.height() >= 16.0);
        }
        let playlist_layer = ui
            .ctx
            .read_response(Id::new("playlist-canvas"))
            .unwrap()
            .layer_id;
        assert_eq!(
            ui.ctx.layer_id_at(node_rect(ui.button("AUTO")).center()),
            Some(playlist_layer),
            "the captured AUTO control must be on the exposed Playlist, not behind another editor"
        );
        assert_painted(&ui.run(Vec::new()), "SONG · 3 bars · AUTO");
        ui.capture(capture);
    }
}
