//! Synthetic egui drops, not evidence of Harmony Blueprint OS drag compatibility.
use super::*;

fn score() -> Vec<u8> {
    let mut bytes = b"MThd\0\0\0\x06\0\0\0\x01\x01\xe0MTrk\0\0\0\x14".to_vec();
    bytes.extend([
        0, 0xff, 0x51, 3, 7, 0xa1, 0x20, 0, 0x90, 64, 100, 0x83, 0x60, 0x80, 64, 0, 0, 0xff, 0x2f,
        0,
    ]);
    bytes
}

fn drop_at(ui: &mut UiHarness, position: Pos2, file: egui::DroppedFile) {
    ui.time += 0.1;
    ui.app.midi_input.last_refresh = Instant::now();
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, ui.size)),
        time: Some(ui.time),
        events: vec![egui::Event::PointerMoved(position)],
        dropped_files: vec![file],
        ..Default::default()
    };
    let _ = ui.ctx.run_ui(input, |root| ui.app.ui(root, &mut ui.frame));
}

#[test]
fn midi_drop_sampler_appends_preserves_score_undo_redo_and_disk_roundtrip() {
    let mut ui = UiHarness::with_storage(None, false);
    let channel = ui.app.project.channels[1].id;
    ui.app.project.tempo = 137.0;
    ui.app.sync_history_observer();
    ui.app.undo_stack.clear();
    let before = ui.app.project.clone();
    ui.app.open_sampler(channel);
    ui.settle();
    let position = node_rect(ui.button("Drop MIDI notes here (append at beat 1)")).center();
    drop_at(
        &mut ui,
        position,
        egui::DroppedFile {
            name: "recorded.mid".into(),
            bytes: Some(score().into()),
            ..Default::default()
        },
    );
    assert_eq!(ui.app.undo_stack.len(), 1);
    assert_eq!(ui.app.project.tempo, 137.0);
    let notes = &ui.app.project.active_pattern().notes;
    assert_eq!(notes.len(), before.active_pattern().notes.len() + 1);
    assert_eq!(
        serde_json::to_value(&notes[..notes.len() - 1]).unwrap(),
        serde_json::to_value(&before.active_pattern().notes).unwrap()
    );
    let added = notes.last().unwrap();
    assert_eq!(
        (added.channel_id, added.note, added.start, added.length),
        (Some(channel), 64, 0.0, 1.0)
    );
    assert!((added.velocity - 100.0 / 127.0).abs() < 0.00001);
    assert_eq!(
        serde_json::to_value(&ui.app.project.channels).unwrap(),
        serde_json::to_value(&before.channels).unwrap()
    );
    let after = project_fingerprint(&ui.app.project);
    ui.app.undo();
    assert_eq!(
        project_fingerprint(&ui.app.project),
        project_fingerprint(&before)
    );
    ui.app.redo();
    assert_eq!(project_fingerprint(&ui.app.project), after);
    let path = std::env::temp_dir().join(format!("citrus-midi-drop-{}.citrus", std::process::id()));
    ui.app.project.save(&path).unwrap();
    let reopened = Project::load(&path).unwrap();
    std::fs::remove_file(path).unwrap();
    assert_eq!(
        serde_json::to_value(&reopened.active_pattern().notes).unwrap(),
        serde_json::to_value(&ui.app.project.active_pattern().notes).unwrap()
    );
    assert_eq!(reopened.tempo, 137.0);
}

#[test]
fn midi_drop_invalid_and_missing_target_are_atomic() {
    let mut ui = UiHarness::with_storage(None, false);
    let before = project_fingerprint(&ui.app.project);
    let channel = ui.app.project.channels[0].id;
    assert!(midi::append_to_channel(&mut ui.app.project, b"not midi", channel).is_err());
    assert!(midi::append_to_channel(&mut ui.app.project, &score(), u32::MAX).is_err());
    assert_eq!(project_fingerprint(&ui.app.project), before);
}

#[test]
fn midi_drop_rack_path_and_piano_bytes_target_the_hit_instrument() {
    for piano in [false, true] {
        let mut ui = UiHarness::with_storage(None, true);
        ui.app.selected_channel = 1;
        let channel = ui.app.project.channels[1].id;
        let name = ui.app.project.channels[1].name.clone();
        ui.app.focus_editor(if piano {
            StudioView::PianoRoll
        } else {
            StudioView::ChannelRack
        });
        ui.settle();
        let position = if piano {
            ui.ctx
                .read_response(Id::new("piano-grid"))
                .unwrap()
                .rect
                .center()
        } else {
            node_rect(ui.button(&name)).center()
        };
        let path = std::env::temp_dir().join(format!("citrus-drop-{}.mid", std::process::id()));
        let file = if piano {
            egui::DroppedFile {
                name: "chord-map.mid".into(),
                bytes: Some(score().into()),
                ..Default::default()
            }
        } else {
            std::fs::write(&path, score()).unwrap();
            egui::DroppedFile {
                path: Some(path.clone()),
                ..Default::default()
            }
        };
        let count = ui.app.project.active_pattern().notes.len();
        drop_at(&mut ui, position, file);
        if !piano {
            std::fs::remove_file(path).unwrap();
        }
        assert_eq!(
            ui.app.project.active_pattern().notes.len(),
            count + 1,
            "piano={piano}"
        );
        assert_eq!(
            ui.app
                .project
                .active_pattern()
                .notes
                .last()
                .unwrap()
                .channel_id,
            Some(channel)
        );
    }
}
