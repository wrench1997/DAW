// Included into the production app UI harness; no alternate shortcut implementation.
fn melody_key(
    key: egui::Key,
    modifiers: egui::Modifiers,
    pressed: bool,
    repeat: bool,
) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat,
        modifiers,
    }
}

#[test]
fn piano_keyboard_real_keys_repeat_phrase_deselect_and_shared_history() {
    let mut ui = UiHarness::floating();
    piano_clipboard_fixture(&mut ui);
    let original = project_fingerprint(&ui.app.project);
    // Higher IDs in another Pattern must never collide with generated note IDs.
    let mut another = ui.app.project.active_pattern().clone();
    another.id = 100;
    another.notes[0].id = 1_000_000;
    another.notes[1].id = 1_000_001;
    another.notes[2].id = 1_000_002;
    ui.app.project.patterns.push(another);
    ui.app.sync_history_observer();
    let original = {
        assert_ne!(original, project_fingerprint(&ui.app.project));
        project_fingerprint(&ui.app.project)
    };
    ui.key(egui::Key::A, piano_clipboard_command());
    ui.key(egui::Key::D, piano_clipboard_command());
    assert!(ui.app.piano_roll_state.selection_ids.is_empty());
    assert_eq!(project_fingerprint(&ui.app.project), original);
    assert!(ui.app.undo_stack.is_empty());
    ui.run(vec![melody_key(
        egui::Key::B,
        piano_clipboard_command(),
        true,
        false,
    )]); // No selection: active Channel only.
    assert_eq!(ui.app.project.active_pattern().notes.len(), 5);
    assert_eq!(
        ui.app.piano_roll_state.selection_ids,
        HashSet::from([1_000_003, 1_000_004])
    );
    assert_eq!(ui.app.project.active_pattern().notes[3].start, 1.125);
    let once = project_fingerprint(&ui.app.project);
    ui.run(vec![melody_key(
        egui::Key::B,
        piano_clipboard_command(),
        true,
        true,
    )]);
    ui.run(vec![melody_key(
        egui::Key::B,
        piano_clipboard_command(),
        false,
        false,
    )]);
    assert_eq!(
        project_fingerprint(&ui.app.project),
        once,
        "duplication never autorepeats"
    );
    ui.key(egui::Key::B, piano_clipboard_command());
    assert_eq!(ui.app.project.active_pattern().notes.len(), 7);
    assert_eq!(ui.app.undo_stack.len(), 2);
    ui.capture("piano-keyboard-repeat-phrase");
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), once);
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), original);
    ui.key(egui::Key::Y, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), once);
}

#[test]
fn piano_keyboard_autorepeat_is_discrete_undo_and_survives_release_focus_modal() {
    let mut ui = UiHarness::floating();
    piano_clipboard_fixture(&mut ui);
    let shift = egui::Modifiers::SHIFT;
    ui.app.piano_roll_state.selection_ids.insert(90_001); // Group expansion includes second, never ghost.
    let original = project_fingerprint(&ui.app.project);
    for repeat in [false, true, true] {
        ui.run(vec![melody_key(egui::Key::ArrowRight, shift, true, repeat)]);
    }
    ui.run(vec![melody_key(egui::Key::ArrowRight, shift, false, false)]);
    assert_eq!(ui.app.project.active_pattern().notes[0].start, 1.125);
    assert_eq!(ui.app.project.active_pattern().notes[1].start, 1.5);
    assert_eq!(ui.app.project.active_pattern().notes[2].start, 0.0);
    assert_eq!(ui.app.undo_stack.len(), 3);
    let moved = project_fingerprint(&ui.app.project);
    ui.settle();
    assert_eq!(project_fingerprint(&ui.app.project), moved);
    ui.key(egui::Key::F9, egui::Modifiers::NONE);
    ui.run(vec![melody_key(egui::Key::ArrowRight, shift, true, true)]);
    assert_eq!(project_fingerprint(&ui.app.project), moved);
    ui.key(egui::Key::F7, egui::Modifiers::NONE);
    ui.key(egui::Key::F10, egui::Modifiers::NONE);
    ui.run(vec![melody_key(egui::Key::ArrowRight, shift, true, true)]);
    assert_eq!(project_fingerprint(&ui.app.project), moved);
    ui.key(egui::Key::Escape, egui::Modifiers::NONE);
    ui.run(vec![melody_key(egui::Key::ArrowRight, shift, false, false)]);
    ui.settle();
    assert_eq!(ui.app.undo_stack.len(), 3);
    for _ in 0..3 {
        ui.key(egui::Key::Z, piano_clipboard_command());
    }
    assert_eq!(project_fingerprint(&ui.app.project), original);
    ui.app.piano_roll_state.snap_to_scale = true;
    ui.key(egui::Key::ArrowUp, shift);
    assert_eq!(ui.app.project.active_pattern().notes[0].note, 61);
    ui.key(egui::Key::ArrowDown, piano_clipboard_command());
    assert_eq!(ui.app.project.active_pattern().notes[0].note, 49);
}

#[test]
fn piano_keyboard_quantize_lengths_ghosts_and_noop_history() {
    let mut ui = UiHarness::floating();
    piano_clipboard_fixture(&mut ui);
    ui.app.project.active_pattern_mut().notes[0].length = 0.625;
    ui.app.sync_history_observer();
    let original = project_fingerprint(&ui.app.project);
    let ghosts = ui.app.piano_roll_state.ghosts_visible;
    ui.key(egui::Key::V, egui::Modifiers::ALT);
    assert_eq!(ui.app.piano_roll_state.ghosts_visible, !ghosts);
    assert_eq!(project_fingerprint(&ui.app.project), original);
    assert!(ui.app.undo_stack.is_empty());
    ui.key(egui::Key::Q, egui::Modifiers::SHIFT);
    assert_eq!(ui.app.project.active_pattern().notes[0].start, 0.5);
    assert_eq!(ui.app.project.active_pattern().notes[0].length, 0.625);
    ui.key(egui::Key::D, egui::Modifiers::SHIFT);
    assert_eq!(ui.app.project.active_pattern().notes[0].length, 0.25);
    assert_eq!(ui.app.project.active_pattern().notes[2].length, 0.25);
    assert_eq!(ui.app.undo_stack.len(), 2);
    ui.key(egui::Key::D, egui::Modifiers::SHIFT);
    assert_eq!(
        ui.app.undo_stack.len(),
        2,
        "unchanged edits make no history"
    );
    ui.key(egui::Key::Z, piano_clipboard_command());
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), original);
    let mut quantize_command = piano_clipboard_command();
    quantize_command.alt = cfg!(target_os = "macos");
    ui.key(egui::Key::Q, quantize_command);
    assert_eq!(ui.app.project.active_pattern().notes[0].start, 0.5);
    assert_eq!(ui.app.project.active_pattern().notes[0].length, 0.75);
    assert_eq!(ui.app.undo_stack.len(), 1);
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), original);
}

#[test]
fn piano_keyboard_pointer_drag_text_modal_hidden_focus_and_snapshot_guards() {
    let mut ui = UiHarness::floating();
    let channel = piano_clipboard_fixture(&mut ui);
    ui.key(egui::Key::A, piano_clipboard_command());
    let original = project_fingerprint(&ui.app.project);
    let selection = ui.app.piano_roll_state.selection_ids.clone();
    for barrier in 0..6 {
        match barrier {
            0 => {
                ui.app.queued_save_request = Some(ProjectSaveRequest::Manual {
                    path: PathBuf::from("unused-piano-keyboard.citrus"),
                    lifecycle: None,
                })
            }
            1 => {
                ui.app.deferred_generator_candidate_after_midi =
                    Some(DeferredMidiGeneratorCandidate {
                        channel_id: channel,
                        candidate: ui.app.project.clone(),
                        previous_state: None,
                    })
            }
            2 => ui.app.piano_roll_gesture_before = Some(ui.app.project.clone()),
            3 => ui.app.playlist_gesture_before = Some(ui.app.project.clone()),
            4 => ui.app.workspace.windows[workspace::index(StudioView::PianoRoll)].visible = false,
            _ => ui.app.show_settings = true,
        }
        for action in [
            ShortcutAction::DeselectNotes,
            ShortcutAction::ToggleGhostNotes,
            ShortcutAction::PianoEdit(PianoKeyboardEdit::RepeatRight),
            ShortcutAction::PianoEdit(PianoKeyboardEdit::MoveSteps(1)),
            ShortcutAction::Delete,
            ShortcutAction::QuickLegato,
        ] {
            ui.app.apply_shortcut_action(&ui.ctx, action);
            assert_eq!(project_fingerprint(&ui.app.project), original);
            assert_eq!(ui.app.piano_roll_state.selection_ids, selection);
            assert!(ui.app.undo_stack.is_empty());
        }
        ui.app.queued_save_request = None;
        ui.app.deferred_generator_candidate_after_midi = None;
        ui.app.piano_roll_gesture_before = None;
        ui.app.playlist_gesture_before = None;
        ui.app.workspace.windows[workspace::index(StudioView::PianoRoll)].visible = true;
        ui.app.show_settings = false;
    }
    let pos = ui
        .ctx
        .read_response(Id::new(("piano-note", 90_001_u64)))
        .unwrap()
        .rect
        .center();
    ui.run(mixer_pointer_button(pos, true));
    ui.run(vec![egui::Event::PointerMoved(pos + Vec2::new(35.0, 0.0))]);
    assert!(ui.app.piano_roll_gesture_before.is_some());
    let dragged = project_fingerprint(&ui.app.project);
    for (key, modifiers) in [
        (egui::Key::ArrowUp, egui::Modifiers::SHIFT),
        (egui::Key::B, piano_clipboard_command()),
        (egui::Key::D, piano_clipboard_command()),
    ] {
        ui.key(key, modifiers);
    }
    assert_eq!(project_fingerprint(&ui.app.project), dragged);
    ui.run(mixer_pointer_button(pos + Vec2::new(35.0, 0.0), false));
    ui.settle();
    // An actual numeric TextEdit owns all of these combinations.
    let bounds = ui
        .nodes
        .iter()
        .find(|n| n.role() == Role::SpinButton && n.bounds().is_some_and(|b| b.y1 < 100.0))
        .unwrap()
        .bounds()
        .unwrap();
    let pos = Pos2::new(
        ((bounds.x0 + bounds.x1) / 2.0) as f32,
        ((bounds.y0 + bounds.y1) / 2.0) as f32,
    );
    for _ in 0..2 {
        ui.run(mixer_pointer_button(pos, true));
        ui.run(mixer_pointer_button(pos, false));
    }
    assert!(ui.ctx.text_edit_focused());
    let notes = serde_json::to_string(&ui.app.project.active_pattern().notes).unwrap();
    for (key, modifiers) in [
        (egui::Key::ArrowUp, egui::Modifiers::SHIFT),
        (egui::Key::B, piano_clipboard_command()),
        (egui::Key::D, piano_clipboard_command()),
        (egui::Key::D, egui::Modifiers::SHIFT),
        (egui::Key::V, egui::Modifiers::ALT),
    ] {
        ui.key(key, modifiers);
    }
    assert_eq!(
        serde_json::to_string(&ui.app.project.active_pattern().notes).unwrap(),
        notes
    );
}

#[test]
fn piano_keyboard_platform_context_and_unverified_chords_do_not_leak() {
    for macos in [false, true] {
        let policy = ShortcutPolicy::new(ShortcutContext {
            piano_active: true,
            macos,
            ..Default::default()
        });
        let chord = |key, command, shift, alt| ShortcutChord {
            key,
            modifiers: ShortcutModifiers {
                command,
                shift,
                alt,
                ..Default::default()
            },
        };
        assert_eq!(
            policy.resolve(chord(ShortcutKey::D, true, false, false)),
            Some(ShortcutAction::DeselectNotes)
        );
        assert_eq!(
            policy.resolve(chord(ShortcutKey::Q, true, false, false)),
            if macos {
                None
            } else {
                Some(ShortcutAction::PianoEdit(
                    PianoKeyboardEdit::QuickQuantize { starts_only: false },
                ))
            }
        );
        assert_eq!(
            policy.resolve(chord(ShortcutKey::Q, true, false, true)),
            if macos {
                Some(ShortcutAction::PianoEdit(
                    PianoKeyboardEdit::QuickQuantize { starts_only: false },
                ))
            } else {
                None
            }
        );
        assert_eq!(
            policy.resolve(chord(ShortcutKey::ArrowLeft, false, false, true)),
            None
        );
        assert_eq!(
            policy.resolve(chord(ShortcutKey::ArrowLeft, true, true, false)),
            None
        );
    }
    let policy = ShortcutPolicy::new(ShortcutContext::default());
    for key in [ShortcutKey::B, ShortcutKey::ArrowUp, ShortcutKey::Q] {
        assert_eq!(
            policy.resolve(ShortcutChord {
                key,
                modifiers: ShortcutModifiers {
                    command: true,
                    ..Default::default()
                }
            }),
            None
        );
    }
}

#[test]
fn piano_keyboard_tools_and_edit_menus_use_real_pointer_commands() {
    let mut ui = UiHarness::floating();
    piano_clipboard_fixture(&mut ui);
    ui.app.project.active_pattern_mut().notes[0].length = 0.625;
    ui.app.sync_history_observer();
    let original = project_fingerprint(&ui.app.project);
    ui.click("TOOLS v");
    ui.key(egui::Key::D, egui::Modifiers::SHIFT);
    assert_eq!(
        project_fingerprint(&ui.app.project),
        original,
        "popup owns keyboard"
    );
    ui.capture("piano-keyboard-tools-menu");
    ui.click(if cfg!(target_os = "macos") {
        "Quick quantize     Opt+Cmd+Q"
    } else {
        "Quick quantize     Ctrl+Q"
    });
    assert_eq!(ui.app.project.active_pattern().notes[0].start, 0.5);
    assert_eq!(ui.app.project.active_pattern().notes[0].length, 0.75);
    assert_eq!(ui.app.undo_stack.len(), 1);
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), original);
    ui.click("EDIT");
    ui.click("Duplicate right     Ctrl/Cmd+B");
    assert_eq!(ui.app.project.active_pattern().notes.len(), 5);
    assert_eq!(ui.app.undo_stack.len(), 1);
    ui.click("EDIT");
    ui.click("Deselect notes      Ctrl/Cmd+D");
    assert!(ui.app.piano_roll_state.selection_ids.is_empty());
    assert_eq!(ui.app.undo_stack.len(), 1);
}
