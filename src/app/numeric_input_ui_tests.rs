//! Real numeric text entry through App::ui, without native input automation or dialogs.

use super::*;

fn numeric_position(ui: &UiHarness, minimum: f64, maximum: f64) -> Pos2 {
    let matches: Vec<_> = ui
        .nodes
        .iter()
        .filter(|node| {
            node.role() == Role::SpinButton
                && node.min_numeric_value() == Some(minimum)
                && node.max_numeric_value() == Some(maximum)
        })
        .collect();
    assert_eq!(
        matches.len(),
        1,
        "expected one live numeric control with range {minimum}..={maximum}: {matches:?}"
    );
    let bounds = matches[0].bounds().expect("real numeric control bounds");
    Pos2::new(
        ((bounds.x0 + bounds.x1) / 2.0) as f32,
        ((bounds.y0 + bounds.y1) / 2.0) as f32,
    )
}

fn enter_digits(ui: &mut UiHarness, position: Pos2, digits: &[(egui::Key, &str)]) {
    // DragValue enters its real TextEdit on a single click. Feed each digit's
    // key/text pair separately, as the native integration does; no SetValue,
    // clipboard replacement, or direct model mutation performs the edit.
    ui.click_pos(position);
    assert!(
        ui.ctx.text_edit_focused(),
        "numeric text editor must own input"
    );
    ui.key(egui::Key::A, piano_clipboard_command());
    for &(key, text) in digits {
        assert!(ui.ctx.text_edit_focused(), "text focus before digit {text}");
        ui.run(vec![
            egui::Event::Key {
                key,
                physical_key: Some(key),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::Text(text.into()),
        ]);
        ui.run(vec![egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: false,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        assert!(ui.ctx.text_edit_focused(), "text focus after digit {text}");
    }
    ui.key(egui::Key::Enter, egui::Modifiers::NONE);
    assert!(
        !ui.ctx.text_edit_focused(),
        "Enter must finish numeric text entry"
    );
}

#[test]
fn numeric_input_song_length_individual_digits_commit_through_app_ui() {
    let mut ui = UiHarness::new();
    ui.click("AUTO");
    assert!(!ui.app.project.auto_song_length);
    ui.app.project.clips.truncate(1);
    assert_eq!(ui.app.project.clips.len(), 1);
    ui.app.project.clips[0].start = 0.0;
    ui.app.project.clips[0].length = 16.0;
    ui.app.project.song_length_beats = 64.0;
    ui.app.sync_history_observer();
    ui.settle();
    let mut expected = ui.app.project.clone();
    expected.song_length_beats = 16.0;
    let position = numeric_position(&ui, 0.25, 4096.0);

    enter_digits(
        &mut ui,
        position,
        &[(egui::Key::Num1, "1"), (egui::Key::Num6, "6")],
    );

    assert_eq!(ui.app.project.song_length_beats, 16.0);
    assert_eq!(
        project_fingerprint(&ui.app.project),
        project_fingerprint(&expected),
        "numeric typing must change song length without editing the arrangement"
    );
    assert!(!ui.app.playing, "numeric typing must not start transport");
}

#[test]
fn numeric_input_release_tail_individual_digits_reach_export_review() {
    let mut ui = UiHarness::new();
    let original = project_fingerprint(&ui.app.project);
    ui.menu("Export WAV…");
    assert!(ui.app.export_dialog.is_open());
    let position = numeric_position(
        &ui,
        0.0,
        f64::from(crate::export_options::WavExportOptions::MAX_TAIL_MILLISECONDS),
    );

    enter_digits(
        &mut ui,
        position,
        &[
            (egui::Key::Num3, "3"),
            (egui::Key::Num0, "0"),
            (egui::Key::Num0, "0"),
            (egui::Key::Num0, "0"),
        ],
    );

    assert!(
        ui.app.export_dialog.is_open(),
        "numeric Enter must keep settings open"
    );
    ui.click("Review…");
    // egui 0.35 exposes static Label text as an AccessKit value, not a label.
    let review_labels: Vec<_> = ui
        .nodes
        .iter()
        .filter(|node| node.role() == Role::Label)
        .map(|node| (node.value(), node.label()))
        .collect();
    assert!(
        review_labels
            .iter()
            .any(|(value, _)| *value == Some("Release tail: 3000 ms after the arrangement")),
        "expected the keyboard-entered tail in Review; actual Label (value, label) nodes: {review_labels:?}"
    );
    let session = ui.app.project_session;
    let options = ui
        .app
        .export_dialog
        .begin_destination(session, false)
        .expect("the actual review must accept the keyboard-entered settings");
    assert_eq!(options.tail_milliseconds, 3000);
    assert_eq!(project_fingerprint(&ui.app.project), original);
    assert!(!ui.app.playing);
    assert!(!ui.app.export_job.is_running());
}
