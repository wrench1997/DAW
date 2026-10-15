//! Slice and Slip keep each placement's source clock across save/reopen.
use super::*;

fn events(project: &Project) -> Vec<(u64, bool, u8)> {
    let map = TempoMap::from_project(project, 8_000).unwrap();
    CompiledTimeline::from_project(project, &map, TimelineCompileOptions::default())
        .unwrap()
        .events()
        .iter()
        .filter_map(|event| match event.kind {
            crate::timeline::TimelineEventKind::NoteOn { note, .. } => {
                Some((event.frame, true, note))
            }
            crate::timeline::TimelineEventKind::NoteOff { note, .. } => {
                Some((event.frame, false, note))
            }
            _ => None,
        })
        .collect()
}

fn save_reopen_and_compare(project: &Project, prefix: &str, unsplit: Option<&Project>) {
    let base = std::env::temp_dir().join(format!(
        "citrus-pattern-clock-{prefix}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&base).unwrap();
    let source = base.join("project.citrus");
    let before_wav = base.join("before.wav");
    let after_wav = base.join("after.wav");
    project.save(&source).unwrap();
    let reopened = Project::load(&source).unwrap();
    assert_eq!(
        serde_json::to_value(&reopened).unwrap(),
        serde_json::to_value(project).unwrap()
    );
    assert_eq!(events(&reopened), events(project));
    crate::export::render_project_wav(project, &before_wav, 8_000).unwrap();
    crate::export::render_project_wav(&reopened, &after_wav, 8_000).unwrap();
    assert_eq!(
        std::fs::read(&before_wav).unwrap(),
        std::fs::read(&after_wav).unwrap()
    );
    if let Some(unsplit) = unsplit {
        let unsplit_wav = base.join("unsplit.wav");
        crate::export::render_project_wav(unsplit, &unsplit_wav, 8_000).unwrap();
        assert_eq!(
            std::fs::read(&unsplit_wav).unwrap(),
            std::fs::read(&before_wav).unwrap()
        );
        std::fs::remove_file(unsplit_wav).unwrap();
    }
    if project
        .clips
        .iter()
        .all(|clip| clip.pattern_length_policy.is_none())
    {
        let old_file = base.join("old-thirteen.citrus");
        let mut stored = serde_json::to_value(project).unwrap();
        stored["format_version"] = serde_json::json!(13);
        std::fs::write(&old_file, serde_json::to_vec(&stored).unwrap()).unwrap();
        let old = Project::load(&old_file).unwrap();
        assert_eq!(
            serde_json::to_value(&old.clips).unwrap(),
            serde_json::to_value(&project.clips).unwrap()
        );
        assert_eq!(events(&old), events(project));
        crate::export::render_project_wav(&old, &after_wav, 8_000).unwrap();
        assert_eq!(
            std::fs::read(&before_wav).unwrap(),
            std::fs::read(&after_wav).unwrap()
        );
        std::fs::remove_file(old_file).unwrap();
    }
    for file in [source, before_wav, after_wav] {
        std::fs::remove_file(file).unwrap();
    }
    std::fs::remove_dir(base).unwrap();
}

fn fixture(policy: Option<PatternLengthPolicy>) -> UiHarness {
    let mut ui = UiHarness::new();
    let mut project = Project::blank();
    project.auto_song_length = true;
    project.tempo = 120.0;
    project.patterns[0].notes = vec![PianoNote {
        id: 91,
        channel_id: Some(1),
        group_id: None,
        note: 60,
        start: 0.5,
        length: 0.25,
        velocity: 0.8,
        selected: false,
        muted: false,
    }];
    let mut clip = Project::default().clips[0].clone();
    clip.id = 1;
    clip.start = 0.0;
    clip.length = 9.0;
    clip.source_offset = 0.0;
    clip.pattern_length_policy = policy;
    project.clips = vec![clip];
    ui.app.project = project;
    ui.app.transport_mode = TransportMode::Song;
    ui.app.selected_channel = 0;
    ui.app.focus_editor(StudioView::Playlist);
    ui.app.sync_history_observer();
    ui.app.project_fingerprint = project_fingerprint(&ui.app.project);
    ui.app.undo_stack.clear();
    ui.app.redo_stack.clear();
    ui.app.dirty = false;
    ui.settle();
    ui
}

#[test]
fn pattern_slice_uses_each_placement_clock_and_preserves_pcm_through_cold_open_and_undo() {
    for policy in [
        None,
        Some(PatternLengthPolicy::Manual),
        Some(PatternLengthPolicy::Follow),
    ] {
        let mut ui = fixture(policy);
        let unsplit = ui.app.project.clone();
        let before = serde_json::to_value(&ui.app.project).unwrap();
        let before_events = events(&ui.app.project);
        let extent = ui.app.project.clip_length_beats(&ui.app.project.clips[0]);
        let cut = if policy == Some(PatternLengthPolicy::Follow) {
            2.0
        } else {
            5.0
        };
        ui.key(egui::Key::Num3, egui::Modifiers::NONE);
        assert_eq!(ui.app.tool_mode, ToolMode::Slice);
        let body = ui
            .ctx
            .read_response(Id::new(("playlist-clip", 1_u32)))
            .unwrap()
            .rect;
        let canvas = ui
            .ctx
            .read_response(Id::new("playlist-canvas"))
            .unwrap()
            .rect;
        let pos = Pos2::new(
            canvas.left() + ui.app.playlist_viewport.x.pixel_for_content(cut) as f32,
            body.center().y,
        );
        ui.click_pos(pos);
        assert_eq!(ui.app.project.clips.len(), 2);
        let left = &ui.app.project.clips[0];
        let right = &ui.app.project.clips[1];
        assert_eq!(
            (left.length, right.start, right.length),
            (cut as f32, cut as f32, extent - cut as f32)
        );
        let expected_policy = policy.map(|_| PatternLengthPolicy::Manual);
        assert_eq!(
            (left.pattern_length_policy, right.pattern_length_policy),
            (expected_policy, expected_policy)
        );
        assert_eq!(
            right.source_offset,
            if policy.is_none() {
                5.0
            } else if cut == 5.0 {
                1.0
            } else {
                2.0
            }
        );
        assert_eq!(events(&ui.app.project), before_events);
        save_reopen_and_compare(&ui.app.project, "slice", Some(&unsplit));
        let after = serde_json::to_value(&ui.app.project).unwrap();
        ui.key(egui::Key::Z, piano_clipboard_command());
        assert_eq!(serde_json::to_value(&ui.app.project).unwrap(), before);
        ui.key(egui::Key::Y, piano_clipboard_command());
        assert_eq!(serde_json::to_value(&ui.app.project).unwrap(), after);
    }
}

#[test]
fn pattern_slip_keeps_length_policy_and_actual_clock_after_save_reopen() {
    for policy in [None, Some(PatternLengthPolicy::Manual)] {
        let mut ui = fixture(policy);
        let original = serde_json::to_value(&ui.app.project).unwrap();
        ui.app.tool_mode = ToolMode::Slip;
        ui.settle();
        let body = ui
            .ctx
            .read_response(Id::new(("playlist-clip", 1_u32)))
            .unwrap()
            .rect;
        let origin = body.center();
        let delta = 5.0 * ui.app.playlist_viewport.x.pixels_per_unit() as f32;
        ui.drag_pointer(origin, Vec2::new(delta, 0.0));
        let clip = &ui.app.project.clips[0];
        assert_eq!(
            clip.source_offset,
            if policy.is_none() { 11.0 } else { 3.0 }
        );
        assert_eq!(clip.length, 9.0);
        assert_eq!(clip.pattern_length_policy, policy);
        save_reopen_and_compare(&ui.app.project, "slip", None);
        ui.key(egui::Key::Z, piano_clipboard_command());
        assert_eq!(serde_json::to_value(&ui.app.project).unwrap(), original);
    }
}
