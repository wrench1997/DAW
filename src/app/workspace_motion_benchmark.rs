// Included by headless_ui_tests. Opt-in measurement, never screenshot-footer FPS.
// This source is also copied unchanged onto c88c7fd for the controlled comparison.
#[test]
fn workspace_motion_measurement() {
    if std::env::var_os("CITRUS_MOTION_BENCHMARK").is_none() {
        return;
    }
    assert!(
        std::env::var_os("CITRUS_UI_CAPTURE_DIR").is_none(),
        "measure UI CPU separately from offscreen GPU/readback"
    );
    let mut ui = UiHarness::floating();
    // Identical scene: all four editors, same explicit rectangles and demo project.
    // Side panels are absent on BOTH builds, not only on the refined default layout.
    ui.app.show_browser = false;
    ui.app.show_inspector = false;
    ui.settle();
    let mut state = workspace::Workspace::default();
    for (view, pos, size) in [
        (
            StudioView::Playlist,
            Pos2::new(40.0, 60.0),
            Vec2::new(800.0, 400.0),
        ),
        (
            StudioView::ChannelRack,
            Pos2::new(960.0, 0.0),
            Vec2::new(780.0, 300.0),
        ),
        (
            StudioView::PianoRoll,
            Pos2::new(960.0, 340.0),
            Vec2::new(800.0, 500.0),
        ),
        (
            StudioView::Mixer,
            Pos2::new(40.0, 500.0),
            Vec2::new(850.0, 420.0),
        ),
    ] {
        state.windows[workspace::index(view)].rect = Some(Rect::from_min_size(pos, size));
    }
    state.focused = StudioView::Playlist;
    let mut storage = WorkspaceTestStorage::default();
    state.save(&mut storage);
    ui.ctx.memory_mut(|memory| memory.reset_areas());
    ui.app.workspace = workspace::Workspace::load(Some(&storage));
    for _ in 0..30 {
        ui.run(Vec::new());
    }
    let fingerprint = project_fingerprint(&ui.app.project);
    for view in workspace::EDITORS {
        eprintln!("MOTION_BENCH rect {view:?} {:?}", ui.editor_rect(view));
    }
    eprintln!(
        "MOTION_BENCH scene logical=1920x1080 editors=4 browser=false inspector=false channels={} clips={} notes={} warmup=30 samples=360 timed=Context_run_ui_plus_accessibility_collection_and_shape_assertion GPU=false native_window=false",
        ui.app.project.channels.len(),
        ui.app.project.clips.len(),
        ui.app.project.active_pattern().notes.len()
    );
    for resizing in [false, true] {
        let before = ui.editor_rect(StudioView::Playlist);
        let origin = if resizing {
            before.right_bottom() - Vec2::splat(2.0)
        } else {
            before.left_top() + Vec2::new(100.0, 13.0)
        };
        ui.run(mixer_pointer_button(origin, true));
        // Activate the drag beyond the native click slop before collecting samples.
        let warm_delta = if resizing {
            Vec2::new(-12.0, -9.0)
        } else {
            Vec2::new(12.0, 9.0)
        };
        ui.run(vec![egui::Event::PointerMoved(origin + warm_delta)]);
        ui.run(Vec::new());
        let mut samples = Vec::new();
        let mut maximum_step = 0.0_f32;
        let mut unchanged = 0usize;
        let mut previous = ui.editor_rect(StudioView::Playlist);
        let mut last_pos = origin + warm_delta;
        for sample in 0..360 {
            let phase = sample % 120;
            let t = if phase < 60 {
                phase + 1
            } else {
                120 - phase - 1
            } as f32;
            let delta = if resizing {
                warm_delta + Vec2::new(-t * 1.5, -t)
            } else {
                warm_delta + Vec2::new(t * 2.0, t)
            };
            last_pos = origin + delta;
            let start = Instant::now();
            ui.run(vec![egui::Event::PointerMoved(last_pos)]);
            samples.push(start.elapsed().as_secs_f64() * 1000.0);
            let actual = ui.editor_rect(StudioView::Playlist);
            let displacement = if resizing {
                (actual.size() - previous.size()).length()
            } else {
                (actual.min - previous.min).length()
            };
            maximum_step = maximum_step.max(displacement);
            unchanged += usize::from(displacement < 0.25);
            assert!(
                displacement < 8.0,
                "unbounded motion jump at {sample}: {previous:?} -> {actual:?}"
            );
            previous = actual;
        }
        ui.run(mixer_pointer_button(last_pos, false));
        ui.settle();
        samples.sort_by(f64::total_cmp);
        eprintln!(
            "MOTION_BENCH {} median_ms={:.3} p95_ms={:.3} max_ms={:.3} max_geometry_step_points={:.3} unchanged_geometry_frames={}/360",
            if resizing { "resize" } else { "drag" },
            samples[180],
            samples[342],
            samples[359],
            maximum_step,
            unchanged
        );
    }
    assert_eq!(project_fingerprint(&ui.app.project), fingerprint);
    assert!(ui.app.undo_stack.is_empty());
}
