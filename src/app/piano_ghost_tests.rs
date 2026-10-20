// Included in the real egui App harness; keyboard commands use production routing.
#[test]
fn piano_ghost_reference_toggle_and_select_all_leave_source_and_history_untouched() {
    use crate::piano_roll::PianoGhostSource;
    let mut ui = UiHarness::floating();
    let channel = piano_clipboard_fixture(&mut ui);
    let mut reference = ui.app.project.active_pattern().clone();
    reference.id = 987;
    reference.name = "Ghost reference".into();
    for note in &mut reference.notes {
        note.id += 1_000_000;
    }
    ui.app.project.patterns.push(reference);
    ui.app.sync_history_observer();
    ui.app.project_fingerprint = project_fingerprint(&ui.app.project);
    ui.app.dirty = false;
    let before = project_fingerprint(&ui.app.project);
    let history = ui.app.undo_stack.len();
    ui.app.piano_roll_state.ghost_source = PianoGhostSource::ReferencePattern(987);
    ui.settle();
    ui.key(egui::Key::V, egui::Modifiers::ALT);
    assert!(!ui.app.piano_roll_state.ghosts_visible);
    assert_eq!(
        ui.app.piano_roll_state.ghost_source,
        PianoGhostSource::ReferencePattern(987)
    );
    ui.key(egui::Key::V, egui::Modifiers::ALT);
    ui.key(egui::Key::A, piano_clipboard_command());
    let expected = ui
        .app
        .project
        .active_pattern()
        .notes
        .iter()
        .filter(|n| n.channel_id == Some(channel))
        .map(|n| n.id)
        .collect::<HashSet<_>>();
    assert_eq!(ui.app.piano_roll_state.selection_ids, expected);
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert_eq!(ui.app.undo_stack.len(), history);
    assert!(!ui.app.dirty);
    ui.capture("piano-ghost-reference-read-only");
}

fn ghost_source_combo_rect(ui: &UiHarness, value: &str) -> Rect {
    node_rect(
        ui.nodes
            .iter()
            .find(|node| node.role() == Role::ComboBox && node.value() == Some(value))
            .unwrap_or_else(|| panic!("missing ghost source combo {value}")),
    )
}

#[test]
fn piano_ghost_mouse_source_choice_toggle_rename_delete_and_same_source() {
    use crate::piano_roll::PianoGhostSource;
    let mut ui = UiHarness::floating();
    piano_clipboard_fixture(&mut ui);
    let mut reference = ui.app.project.active_pattern().clone();
    reference.id = 987;
    reference.name = "Reference".into();
    reference.notes[0].start = 2.0;
    for note in &mut reference.notes {
        note.id += 1_000_000;
    }
    ui.app.project.patterns.push(reference);
    ui.app.sync_history_observer();
    ui.app.project_fingerprint = project_fingerprint(&ui.app.project);
    ui.app.dirty = false;
    let before = project_fingerprint(&ui.app.project);
    let history = ui.app.undo_stack.len();
    ui.settle();
    ui.click_pos(ghost_source_combo_rect(&ui, "Current Pattern").center());
    ui.click("#987 Reference");
    assert_eq!(
        ui.app.piano_roll_state.ghost_source,
        PianoGhostSource::ReferencePattern(987)
    );
    ui.click("GHOST NOTES");
    assert!(!ui.app.piano_roll_state.ghosts_visible);
    ui.click("GHOST NOTES");
    assert!(ui.app.piano_roll_state.ghosts_visible);
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert_eq!(ui.app.undo_stack.len(), history);
    assert!(!ui.app.dirty);

    // A Delete-tool click on a reference-only rectangle must not edit either Pattern.
    ui.app.piano_roll_state.tool = PianoRollTool::Delete;
    let grid = ui.ctx.read_response(Id::new("piano-grid")).unwrap().rect;
    let reference_note = &ui.app.project.patterns.last().unwrap().notes[0];
    let pos = piano_mouse::note_rects(grid, ui.app.piano_viewport, reference_note)
        .1
        .center();
    assert!(grid.contains(pos));
    assert!(
        ui.ctx
            .read_response(Id::new(("piano-note", reference_note.id)))
            .is_none()
    );
    ui.click_pos(pos);
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert_eq!(ui.app.undo_stack.len(), history);
    assert!(!ui.app.dirty);

    let reference_index = ui.app.project.patterns.len() - 1;
    ui.app.project.patterns[reference_index].name = "Renamed".into();
    ui.settle();
    ghost_source_combo_rect(&ui, "Ref #987: Renamed");
    let original_index = ui.app.project.active_pattern;
    ui.app.project.active_pattern = reference_index;
    ui.settle();
    ui.click_pos(ghost_source_combo_rect(&ui, "Ref #987: Renamed").center());
    assert!(ui.nodes.iter().any(|node| node.value().or_else(|| node.label()) == Some("Other Channels · read only")));
    ui.key(egui::Key::Escape, egui::Modifiers::NONE);
    let target = Some(ui.app.project.channels[ui.app.selected_channel].id);
    assert!(
        ui.app
            .piano_roll_state
            .ghost_source
            .notes(true, &ui.app.project.patterns, reference_index, target)
            .iter()
            .all(|n| n.channel_id != target)
    );
    ui.app.project.active_pattern = original_index;
    ui.app.project.patterns.remove(reference_index);
    ui.settle();
    assert_eq!(
        ui.app.piano_roll_state.ghost_source,
        PianoGhostSource::CurrentPattern
    );
    ghost_source_combo_rect(&ui, "Current Pattern");
}

#[test]
fn piano_ghost_controls_stay_discoverable_inside_narrow_editor() {
    for width in [1080.0, 1440.0] {
        let mut ui = UiHarness::floating();
        ui.size = Vec2::new(width, 780.0);
        piano_clipboard_fixture(&mut ui);
        let mut reference = ui.app.project.active_pattern().clone();
        reference.id = 988;
        reference.name =
            "A very long reference Pattern name that must wrap inside the source menu".into();
        for note in &mut reference.notes {
            note.id += 2_000_000;
        }
        ui.app.project.patterns.push(reference);
        ui.app.sync_history_observer();
        // Exercise a narrow floating editor independently of the outer screen width.
        ui.app.workspace.windows[workspace::index(StudioView::PianoRoll)].rect = Some(
            Rect::from_min_size(Pos2::new(120.0, 180.0), Vec2::new(580.0, 510.0)),
        );
        ui.settle();
        let panel = ui.editor_rect(StudioView::PianoRoll);
        let toggle = node_rect(ui.button("GHOST NOTES"));
        let source = ghost_source_combo_rect(&ui, "Current Pattern");
        assert!(
            panel.contains_rect(toggle),
            "ghost toggle overflow: {toggle:?} / {panel:?}"
        );
        assert!(
            panel.contains_rect(source),
            "ghost source overflow: {source:?} / {panel:?}"
        );
        assert_disjoint(toggle, source);
        ui.click_pos(source.center());
        let choice = node_rect(ui.button("Current Pattern: other Channels"));
        assert!(ui.ctx.content_rect().contains_rect(choice));
        let long_choice =
            "#988 A very long reference Pattern name that must wrap inside the source menu";
        assert!(
            ui.ctx
                .content_rect()
                .contains_rect(node_rect(ui.button(long_choice)))
        );
        ui.click(long_choice);
        let selected = ghost_source_combo_rect(&ui, "Ref #988: A very long reference …");
        assert!(
            panel.contains_rect(selected),
            "long source overflow: {selected:?} / {panel:?}"
        );
        ui.click("GHOST NOTES");
        assert!(!ui.app.piano_roll_state.ghosts_visible);
        ui.capture("piano-ghost-narrow-controls");
    }
}

#[test]
fn piano_ghost_reference_does_not_cross_project_install_with_reused_pattern_id() {
    use crate::piano_roll::PianoGhostSource;
    let mut ui = UiHarness::floating();
    piano_clipboard_fixture(&mut ui);
    let id = ui.app.project.active_pattern().id;
    ui.app.piano_roll_state.ghost_source = PianoGhostSource::ReferencePattern(id);
    let mut incoming = Project::default();
    incoming.active_pattern_mut().id = id;
    let old_session = ui.app.project_session;
    assert!(ui.app.install_project(incoming, None, false));
    assert_ne!(ui.app.project_session, old_session);
    assert_eq!(
        ui.app.piano_roll_state.ghost_source,
        PianoGhostSource::CurrentPattern
    );
    ui.key(egui::Key::F7, egui::Modifiers::NONE);
    ghost_source_combo_rect(&ui, "Current Pattern");
    assert!(!ui.app.dirty);
    assert!(ui.app.undo_stack.is_empty());
}

#[test]
fn piano_ghost_long_reference_tail_is_reachable_without_changing_playback_or_fit_target() {
    use crate::piano_roll::PianoGhostSource;
    for delete_source in [false, true] {
        let mut ui = UiHarness::floating();
        piano_clipboard_fixture(&mut ui);
        let mut reference = ui.app.project.active_pattern().clone();
        reference.id = 989;
        reference.name = "Long reference".into();
        reference.notes.truncate(1);
        reference.notes[0].id = 2_000_001;
        reference.notes[0].start = 257.0;
        reference.notes[0].length = 2.0;
        ui.app.project.patterns.push(reference);
        ui.app.sync_history_observer();
        ui.app.project_fingerprint = project_fingerprint(&ui.app.project);
        ui.app.dirty = false;
        let original = project_fingerprint(&ui.app.project);
        let active_end = piano_pattern_content_end(ui.app.project.active_pattern());
        let playback_length = pattern_playback_length_beats(&ui.app.project);
        let selected = HashSet::from([90_001]);
        ui.app.piano_roll_state.selection_ids = selected.clone();
        ui.settle();
        ui.click_pos(ghost_source_combo_rect(&ui, "Current Pattern").center());
        ui.click("#989 Long reference");
        assert_eq!(
            ui.app.piano_roll_state.ghost_source,
            PianoGhostSource::ReferencePattern(989)
        );
        assert!(ui.app.piano_viewport.x.content_range().1 > 259.0);
        ui.app.piano_viewport.x.reveal(257.0, 259.0, 0.0).unwrap();
        ui.settle();
        assert!(content_span_is_visible(
            ui.app.piano_viewport.x,
            257.0,
            259.0
        ));
        assert_eq!(
            pattern_playback_length_beats(&ui.app.project),
            playback_length
        );
        assert_eq!(project_fingerprint(&ui.app.project), original);
        assert_eq!(ui.app.piano_roll_state.selection_ids, selected);
        assert!(!ui.app.dirty);
        assert!(ui.app.undo_stack.is_empty());

        // Fit remains a target-only operation, even while a far reference is visible.
        ui.click("FIT NOTES");
        assert!(content_span_is_visible(
            ui.app.piano_viewport.x,
            0.375,
            1.125
        ));
        assert!(!content_span_is_visible(
            ui.app.piano_viewport.x,
            257.0,
            259.0
        ));
        ui.app.piano_viewport.x.reveal(257.0, 259.0, 0.0).unwrap();
        ui.settle();
        if delete_source {
            ui.app.project.patterns.pop();
            ui.app.sync_history_observer();
        } else {
            ui.click("GHOST NOTES");
        }
        ui.settle();
        assert_eq!(ui.app.piano_viewport.x.content_range().1, active_end);
        let visible = ui.app.piano_viewport.x.visible_range();
        assert!(visible.0.is_finite() && visible.1 <= active_end && visible.1 > visible.0);
        assert_eq!(ui.app.piano_roll_state.selection_ids, selected);
        assert_eq!(
            pattern_playback_length_beats(&ui.app.project),
            playback_length
        );
        // Clamping a far view cannot invalidate note positioning: Fit still reveals
        // the exact target note and its real editable widget at the same local beat.
        ui.click("FIT NOTES");
        let grid = ui.ctx.read_response(Id::new("piano-grid")).unwrap().rect;
        let target = ui
            .ctx
            .read_response(Id::new(("piano-note", 90_001_u64)))
            .unwrap();
        assert!(grid.contains_rect(target.rect));
        assert_eq!(ui.app.project.active_pattern().notes[0].start, 0.375);
        assert_eq!(ui.app.piano_roll_state.selection_ids, selected);
    }
}

#[test]
fn piano_ghost_scope_toggle_keeps_grid_origin_and_toolbar_rows_stable() {
    for width in [480.0, 580.0, 740.0] {
        let mut ui = UiHarness::floating();
        piano_clipboard_fixture(&mut ui);
        let mut reference = ui.app.project.active_pattern().clone();
        reference.id = 990;
        reference.name = "Stable reference".into();
        for note in &mut reference.notes {
            note.id += 3_000_000;
        }
        ui.app.project.patterns.push(reference);
        ui.app.sync_history_observer();
        ui.app.project_fingerprint = project_fingerprint(&ui.app.project);
        ui.app.dirty = false;
        ui.app.piano_roll_state.selection_ids = HashSet::from([90_001]);
        ui.arrange_test_windows(
            &[(
                StudioView::PianoRoll,
                Rect::from_min_size(Pos2::new(20.0, 20.0), Vec2::new(width, 420.0)),
            )],
            StudioView::PianoRoll,
        );
        ui.settle();
        let project = project_fingerprint(&ui.app.project);
        let selection = ui.app.piano_roll_state.selection_ids.clone();
        let grid = ui.ctx.read_response(Id::new("piano-grid")).unwrap().rect;
        let view = node_rect(ui.button("VIEW v"));
        let fit = node_rect(ui.button("FIT NOTES"));
        let window = ui.editor_rect(StudioView::PianoRoll);
        assert!(
            window.width() <= width + 4.0,
            "toolbar expanded {width}px window: {window:?}"
        );
        assert!(
            grid.height() >= 144.0,
            "compact grid remains usable: {grid:?}"
        );
        ui.click_pos(ghost_source_combo_rect(&ui, "Current Pattern").center());
        ui.click("#990 Stable reference");
        for _ in 0..4 {
            let current_grid = ui.ctx.read_response(Id::new("piano-grid")).unwrap().rect;
            assert_eq!(
                current_grid.top(),
                grid.top(),
                "scope changed grid origin at width {width}"
            );
            assert_eq!(node_rect(ui.button("VIEW v")), view);
            assert_eq!(node_rect(ui.button("FIT NOTES")), fit);
            assert_eq!(project_fingerprint(&ui.app.project), project);
            assert_eq!(ui.app.piano_roll_state.selection_ids, selection);
            assert!(!ui.app.dirty);
            assert!(ui.app.undo_stack.is_empty());
            ui.key(egui::Key::V, egui::Modifiers::ALT);
        }
    }
}
