//! Inspector capabilities and real pointer -> project -> exported PCM -> history.
//! The historical editable-but-inaudible Pattern-fade failure is retained separately;
//! these tests require unsupported controls to be absent, not silent success.

use super::*;
use crate::export_job::ExportControl;
use crate::export_options::{WavExportOptions, WavLevelPolicy, WavSampleFormat};

const SAMPLE_RATE: u32 = 48_000;

fn blank_ui() -> UiHarness {
    let mut ui = UiHarness::new();
    let mut project = Project::blank();
    project.tempo = 120.0;
    project.song_length_beats = 4.0;
    project.auto_song_length = false;
    assert!(ui.app.install_project(project, None, false));
    ui.app.tool_mode = ToolMode::Select;
    ui.app.focus_editor(StudioView::Playlist);
    ui.settle();
    ui
}

fn saved_baseline(ui: &mut UiHarness) {
    ui.app.sync_history_observer();
    ui.app.project_fingerprint = project_fingerprint(&ui.app.project);
    ui.app.undo_stack.clear();
    ui.app.redo_stack.clear();
    ui.app.dirty = false;
    ui.settle();
}

fn pattern_clip() -> Clip {
    let mut clip = Project::default().clips[0].clone();
    clip.id = 80_001;
    clip.name = "Inspector Pattern fixture".into();
    clip.kind = ClipKind::Pattern;
    clip.track = 0;
    clip.start = 0.0;
    clip.length = 4.0;
    clip.pattern_id = 1;
    clip.pattern_length_policy = Some(PatternLengthPolicy::Manual);
    clip.group_id = None;
    clip.gain = 1.0;
    clip.fade_in = 0.0;
    clip.fade_out = 0.0;
    clip
}

fn select_clip(ui: &mut UiHarness, id: u32) {
    ui.settle();
    let rect = ui
        .ctx
        .read_response(Id::new(("playlist-clip", id)))
        .expect("fixture clip must be visible in the real Playlist")
        .rect;
    ui.click_pos(rect.center());
    assert_eq!(ui.app.selected_clip, Some(id));
}

fn label_present(ui: &UiHarness, label: &str) -> bool {
    ui.nodes
        .iter()
        .any(|node| node.value() == Some(label) || node.label() == Some(label))
}

fn property_slider_rect(ui: &UiHarness, label: &str, maximum: f64) -> Rect {
    let labels: Vec<_> = ui
        .nodes
        .iter()
        // egui emits the static Label and a text-run child with the same text.
        // Use the row's semantic label, then independently require one enabled
        // Slider with the exact range and matching vertical bounds below.
        .filter(|node| {
            node.role() == Role::Label
                && (node.value() == Some(label) || node.label() == Some(label))
        })
        .collect();
    assert_eq!(
        labels.len(),
        1,
        "one static Inspector {label} row label: {labels:?}"
    );
    let y = node_rect(labels[0]).center().y;
    let candidates: Vec<_> = ui
        .nodes
        .iter()
        .filter(|node| {
            node.role() == Role::Slider
                && node.min_numeric_value() == Some(0.0)
                && node.max_numeric_value() == Some(maximum)
                && (node_rect(node).center().y - y).abs() < 15.0
        })
        .collect();
    assert_eq!(
        candidates.len(),
        1,
        "one real slider next to Inspector {label}; never target an unrelated control"
    );
    assert!(!candidates[0].is_disabled());
    let rect = node_rect(candidates[0]);
    assert!(Rect::from_min_size(Pos2::ZERO, ui.size).contains_rect(rect));
    rect
}

fn click_slider(ui: &mut UiHarness, label: &str, maximum: f64, fraction: f32) {
    let rect = property_slider_rect(ui, label, maximum);
    let x = (rect.left() + rect.width() * fraction).clamp(rect.left() + 1.0, rect.right() - 1.0);
    ui.click_pos(Pos2::new(x, rect.center().y));
}

fn audio_ui() -> (UiHarness, Fixture) {
    let fixture = Fixture::new();
    // One second of constant mono PCM makes both fade shape and unchanged
    // source samples observable without an oscillator, normalizer or limiter.
    let data_bytes = SAMPLE_RATE * 2;
    let mut wav = Vec::with_capacity(data_bytes as usize + 44);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&data_bytes.to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_bytes.to_le_bytes());
    for _ in 0..SAMPLE_RATE {
        wav.extend_from_slice(&12_000_i16.to_le_bytes());
    }
    std::fs::write(fixture.0.join("constant.wav"), wav).unwrap();
    let mut ui = blank_ui();
    ui.click("SOUNDS");
    ui.app.sample_browser.navigate(fixture.0.clone());
    ui.wait_until(|app| !app.sample_browser.busy());
    ui.click("constant.wav");
    ui.click("Import to Playlist");
    ui.wait_until(|app| app.audio_import_receiver.is_none());
    assert!(ui.app.audio_import_error.is_none());
    assert_eq!(ui.app.project.audio_assets.len(), 1);
    assert_eq!(
        ui.app.project.audio_assets[0].frames,
        u64::from(SAMPLE_RATE)
    );
    assert_eq!(ui.app.project.clips.len(), 1);
    assert_eq!(ui.app.project.clips[0].kind, ClipKind::Audio);
    assert_eq!(ui.app.project.clips[0].length, 2.0);
    let id = ui.app.project.clips[0].id;
    select_clip(&mut ui, id);
    saved_baseline(&mut ui);
    (ui, fixture)
}

fn render_pcm(ui: &UiHarness, fixture: &Fixture, name: &str) -> Vec<f32> {
    let path = fixture.0.join(format!("{name}.wav"));
    let report = crate::export::render_project_wav_with_options(
        &ui.app.project,
        &path,
        WavExportOptions {
            sample_rate: SAMPLE_RATE,
            sample_format: WavSampleFormat::Float32,
            level_policy: WavLevelPolicy::PreserveLevel,
            tail_milliseconds: 0,
        },
        &ExportControl::default(),
    )
    .unwrap();
    assert_eq!(report.frames, 96_000);
    assert_eq!(report.applied_gain, 1.0);
    let decoded = crate::wav::read_wav(&path).unwrap();
    assert_eq!(decoded.metadata.channels, 2);
    assert_eq!(decoded.metadata.sample_rate, SAMPLE_RATE);
    assert_eq!(decoded.samples.len(), 192_000);
    assert!(decoded.samples.iter().all(|sample| sample.is_finite()));
    decoded.samples
}

fn assert_changed_pcm(before: &[f32], after: &[f32]) {
    assert_eq!(before.len(), after.len());
    assert!(before.iter().any(|sample| sample.abs() > 0.01));
    let differences = before
        .iter()
        .zip(after)
        .filter(|(a, b)| (*a - *b).abs() > 1.0e-6)
        .count();
    assert!(
        differences > 10_000,
        "substantial audible PCM must change: {differences}"
    );
}

#[test]
fn inspector_clip_capabilities_only_present_effective_controls_on_repeated_selection() {
    let (mut ui, _fixture) = audio_ui();
    let audio_id = ui.app.project.clips[0].id;
    let mut pattern = pattern_clip();
    // Old projects can contain previously editable but ignored fields. Opening
    // Inspector must neither offer them again nor silently rewrite that data.
    pattern.fade_in = 0.6;
    pattern.fade_out = 0.8;
    let pattern_id = pattern.id;
    let mut automation = pattern.clone();
    automation.id += 1;
    automation.kind = ClipKind::Automation;
    automation.name = "Inspector automation fixture".into();
    automation.track = 1;
    automation.automation_id = Some(90_001);
    automation.pattern_length_policy = None;
    automation.gain = 0.4;
    let automation_id = automation.id;
    let mut lane = AutomationLane::new(AutomationTarget::MasterVolume);
    lane.insert_point(AutomationPoint::new(0.0, 0.2)).unwrap();
    lane.insert_point(AutomationPoint::new(4.0, 0.8)).unwrap();
    ui.app.project.automation_lanes.push(ProjectAutomation {
        id: 90_001,
        name: "Inspector automation fixture".into(),
        lane,
    });
    ui.app.project.clips.extend([pattern, automation]);
    saved_baseline(&mut ui);
    let original = serde_json::to_value(&ui.app.project).unwrap();
    for (id, kind) in [
        (audio_id, ClipKind::Audio),
        (pattern_id, ClipKind::Pattern),
        (automation_id, ClipKind::Automation),
        (pattern_id, ClipKind::Pattern),
        (audio_id, ClipKind::Audio),
    ] {
        select_clip(&mut ui, id);
        assert_eq!(label_present(&ui, "Gain"), kind != ClipKind::Automation);
        for label in ["Fade in", "Fade out"] {
            assert_eq!(label_present(&ui, label), kind == ClipKind::Audio);
            if kind == ClipKind::Audio {
                property_slider_rect(&ui, label, 1.0);
            }
        }
        if kind != ClipKind::Automation {
            property_slider_rect(&ui, "Gain", 1.5);
        }
        assert!(label_present(&ui, "MUTED"));
        if kind == ClipKind::Pattern {
            assert!(label_present(
                &ui,
                "For fades, automate volume or render to audio."
            ));
        }
        if kind == ClipKind::Automation {
            assert!(label_present(
                &ui,
                "Edit the automation curve to change its values."
            ));
        }
        assert_eq!(serde_json::to_value(&ui.app.project).unwrap(), original);
        assert!(ui.app.undo_stack.is_empty());
        assert!(!ui.app.dirty);
    }
    ui.capture("inspector-supported-audio-controls");
}

#[test]
fn inspector_audio_gain_and_each_fade_change_actual_pcm_and_undo_redo_exactly() {
    for label in ["Gain", "Fade in", "Fade out"] {
        let (mut ui, fixture) = audio_ui();
        let original = serde_json::to_value(&ui.app.project).unwrap();
        let before = render_pcm(&ui, &fixture, "before");
        assert!(before[..96_000].iter().all(|sample| *sample > 0.01));
        assert!(before[96_000..].iter().all(|sample| *sample == 0.0));
        let gain_edit = label == "Gain";
        click_slider(
            &mut ui,
            label,
            if gain_edit { 1.5 } else { 1.0 },
            if gain_edit { 0.35 } else { 1.0 },
        );
        let clip = &ui.app.project.clips[0];
        let scalar = clip.gain;
        if gain_edit {
            assert!((0.25..0.75).contains(&scalar));
        } else if label == "Fade in" {
            assert_eq!(clip.fade_in, 1.0);
            assert_eq!(clip.fade_out, 0.0);
        } else {
            assert_eq!(clip.fade_out, 1.0);
            assert_eq!(clip.fade_in, 0.0);
        }
        let edited = serde_json::to_value(&ui.app.project).unwrap();
        let after = render_pcm(&ui, &fixture, "after");
        assert_changed_pcm(&before, &after);
        if gain_edit {
            assert!(
                before
                    .iter()
                    .zip(&after)
                    .all(|(a, b)| (*a * scalar - *b).abs() < 1.0e-6)
            );
        } else {
            let first = after[0];
            let middle = after[48_000];
            let last = after[95_998];
            let full = before[0];
            assert!(
                middle > full * 0.65 && middle < full * 0.75,
                "equal-power midpoint: {middle}/{full}"
            );
            if label == "Fade in" {
                assert_eq!(first, 0.0);
                assert!((last - full).abs() < 1.0e-6);
                assert!(
                    after[..96_000]
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|frame| frame[0])
                        .collect::<Vec<_>>()
                        .windows(2)
                        .all(|pair| pair[0] <= pair[1])
                );
            } else {
                assert!((first - full).abs() < 1.0e-6);
                assert_eq!(last, 0.0);
                assert!(
                    after[..96_000]
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|frame| frame[0])
                        .collect::<Vec<_>>()
                        .windows(2)
                        .all(|pair| pair[0] >= pair[1])
                );
            }
        }
        assert_eq!(ui.app.undo_stack.len(), 1);
        assert!(ui.app.dirty);
        ui.key(egui::Key::Z, piano_clipboard_command());
        assert_eq!(serde_json::to_value(&ui.app.project).unwrap(), original);
        assert_eq!(render_pcm(&ui, &fixture, "undo"), before);
        ui.key(egui::Key::Y, piano_clipboard_command());
        assert_eq!(serde_json::to_value(&ui.app.project).unwrap(), edited);
        assert_eq!(render_pcm(&ui, &fixture, "redo"), after);
    }
}

#[test]
fn inspector_pattern_gain_changes_actual_pcm_without_rewriting_notes_and_undo_redo() {
    let fixture = Fixture::new();
    let mut ui = blank_ui();
    let clip = pattern_clip();
    let id = clip.id;
    ui.app.project.clips.push(clip);
    ui.app.project.active_pattern_mut().notes.push(PianoNote {
        id: 90_002,
        channel_id: Some(1),
        group_id: None,
        note: 60,
        start: 0.25,
        // Sustain to the exact arrangement end. At this level both versions
        // remain above the renderer's voice-retirement threshold throughout,
        // allowing a whole-output scalar comparison instead of a loose peak check.
        length: 3.75,
        velocity: 1.0,
        selected: false,
        muted: false,
    });
    saved_baseline(&mut ui);
    select_clip(&mut ui, id);
    let original = serde_json::to_value(&ui.app.project).unwrap();
    let notes = serde_json::to_value(&ui.app.project.patterns).unwrap();
    let before = render_pcm(&ui, &fixture, "pattern-before");
    click_slider(&mut ui, "Gain", 1.5, 0.35);
    let gain = ui.app.project.clips[0].gain;
    assert!((0.25..0.75).contains(&gain));
    assert_eq!(
        serde_json::to_value(&ui.app.project.patterns).unwrap(),
        notes
    );
    assert_eq!(ui.app.project.clips[0].fade_in, 0.0);
    assert_eq!(ui.app.project.clips[0].fade_out, 0.0);
    let edited = serde_json::to_value(&ui.app.project).unwrap();
    let after = render_pcm(&ui, &fixture, "pattern-after");
    assert_changed_pcm(&before, &after);
    assert!(
        before
            .iter()
            .zip(&after)
            .all(|(a, b)| (*a * gain - *b).abs() < 1.0e-6),
        "native Pattern gain must scale the entire output without changing note timing"
    );
    assert_eq!(ui.app.undo_stack.len(), 1);
    assert!(ui.app.dirty);
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(serde_json::to_value(&ui.app.project).unwrap(), original);
    assert_eq!(render_pcm(&ui, &fixture, "pattern-undo"), before);
    ui.key(egui::Key::Y, piano_clipboard_command());
    assert_eq!(serde_json::to_value(&ui.app.project).unwrap(), edited);
    assert_eq!(render_pcm(&ui, &fixture, "pattern-redo"), after);
    ui.capture("inspector-supported-pattern-gain");
}

fn envelope_pointer_frame(ui: &mut UiHarness, pos: Pos2, pressed: Option<bool>) {
    // Deterministically cross both the 50 ms observer and 500 ms coalescing
    // thresholds. Real pointer input still performs every musical edit; no
    // sleeps, manual history flushes, or direct Undo hide the timing defect.
    ui.app.last_history_check = Instant::now() - Duration::from_secs(2);
    ui.app.last_history_capture = Instant::now() - Duration::from_secs(2);
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

#[test]
fn inspector_envelope_pointer_gestures_ignore_observer_deadlines_and_remain_separate() {
    for (audio, label, maximum) in [
        (false, "Gain", 1.5),
        (true, "Gain", 1.5),
        (true, "Fade in", 1.0),
        (true, "Fade out", 1.0),
    ] {
        let (mut ui, _media) = if audio {
            let (ui, fixture) = audio_ui();
            (ui, Some(fixture))
        } else {
            let mut ui = blank_ui();
            let clip = pattern_clip();
            let id = clip.id;
            ui.app.project.clips.push(clip);
            saved_baseline(&mut ui);
            select_clip(&mut ui, id);
            (ui, None)
        };
        let original = serde_json::to_value(&ui.app.project).unwrap();
        let original_fingerprint = project_fingerprint(&ui.app.project);
        for fraction in [0.2, 0.4, 0.6] {
            let rect = property_slider_rect(&ui, label, maximum);
            let pos = Pos2::new(rect.left() + rect.width() * fraction, rect.center().y);
            envelope_pointer_frame(&mut ui, pos, (fraction == 0.2).then_some(true));
            assert!(
                ui.app.playlist_gesture_before.is_some(),
                "{audio}/{label} pointer must own one pre-edit snapshot"
            );
            assert!(
                ui.app.undo_stack.is_empty(),
                "{audio}/{label} timed observer must not capture an intermediate drag value"
            );
            assert_eq!(ui.app.history_fingerprint, original_fingerprint);
        }
        let rect = property_slider_rect(&ui, label, maximum);
        let pos = Pos2::new(rect.left() + rect.width() * 0.7, rect.center().y);
        envelope_pointer_frame(&mut ui, pos, Some(false));
        ui.settle();
        assert!(ui.app.playlist_gesture_before.is_none());
        let first_edit = serde_json::to_value(&ui.app.project).unwrap();
        assert_ne!(first_edit, original);
        assert_eq!(
            ui.app.undo_stack.len(),
            1,
            "{audio}/{label} release must commit exactly one edit"
        );
        assert_eq!(
            ui.app.history_fingerprint,
            project_fingerprint(&ui.app.project)
        );

        // An immediately following pointer gesture is independently undoable,
        // even when it falls inside the old observer's 500 ms merge window.
        click_slider(&mut ui, label, maximum, 0.3);
        let second_edit = serde_json::to_value(&ui.app.project).unwrap();
        assert_ne!(second_edit, first_edit);
        assert_eq!(ui.app.undo_stack.len(), 2);
        ui.key(egui::Key::Z, piano_clipboard_command());
        assert_eq!(serde_json::to_value(&ui.app.project).unwrap(), first_edit);
        ui.key(egui::Key::Z, piano_clipboard_command());
        assert_eq!(serde_json::to_value(&ui.app.project).unwrap(), original);
        assert!(ui.app.undo_stack.is_empty());
        ui.key(egui::Key::Y, piano_clipboard_command());
        assert_eq!(serde_json::to_value(&ui.app.project).unwrap(), first_edit);
        ui.key(egui::Key::Y, piano_clipboard_command());
        assert_eq!(serde_json::to_value(&ui.app.project).unwrap(), second_edit);
    }
}

fn pattern_history_ui() -> UiHarness {
    let mut ui = blank_ui();
    let clip = pattern_clip();
    let id = clip.id;
    ui.app.project.clips.push(clip);
    saved_baseline(&mut ui);
    select_clip(&mut ui, id);
    ui
}

fn begin_pattern_gain_pointer(ui: &mut UiHarness) -> Pos2 {
    let rect = property_slider_rect(ui, "Gain", 1.5);
    let pos = Pos2::new(rect.left() + rect.width() * 0.35, rect.center().y);
    envelope_pointer_frame(ui, pos, Some(true));
    assert!(ui.app.playlist_gesture_before.is_some());
    assert_ne!(ui.app.project.clips[0].gain, 1.0);
    pos
}

#[test]
fn inspector_gain_gesture_keeps_the_previous_pending_name_edit_separately_undoable() {
    let mut ui = pattern_history_ui();
    let original = serde_json::to_value(&ui.app.project).unwrap();
    let name_fields: Vec<_> = ui
        .nodes
        .iter()
        .filter(|node| {
            node.role() == Role::TextInput && node.value() == Some("Inspector Pattern fixture")
        })
        .collect();
    assert_eq!(name_fields.len(), 1);
    let name_pos = node_rect(name_fields[0]).center();
    ui.click_pos(name_pos);
    assert!(ui.ctx.text_edit_focused());
    ui.key(egui::Key::A, piano_clipboard_command());
    // Keep this actual text edit pending at the observer boundary, then begin
    // the independent pointer gesture without an artificial history flush.
    ui.app.last_history_check = Instant::now() + Duration::from_secs(2);
    ui.run(vec![egui::Event::Text("Renamed before Gain".into())]);
    assert_eq!(ui.app.project.clips[0].name, "Renamed before Gain");
    assert!(ui.app.undo_stack.is_empty());
    let renamed = serde_json::to_value(&ui.app.project).unwrap();
    let pos = begin_pattern_gain_pointer(&mut ui);
    assert!(ui.app.undo_stack.is_empty());
    envelope_pointer_frame(&mut ui, pos, Some(false));
    ui.settle();
    let changed = serde_json::to_value(&ui.app.project).unwrap();
    assert_eq!(ui.app.undo_stack.len(), 2);
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(serde_json::to_value(&ui.app.project).unwrap(), renamed);
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(serde_json::to_value(&ui.app.project).unwrap(), original);
    ui.key(egui::Key::Y, piano_clipboard_command());
    assert_eq!(serde_json::to_value(&ui.app.project).unwrap(), renamed);
    ui.key(egui::Key::Y, piano_clipboard_command());
    assert_eq!(serde_json::to_value(&ui.app.project).unwrap(), changed);
}

#[test]
fn inspector_gain_pointer_focus_modal_and_editor_interruptions_do_not_resume_old_gestures() {
    for interruption in ["focus", "settings", "editor"] {
        let mut ui = pattern_history_ui();
        let original = serde_json::to_value(&ui.app.project).unwrap();
        let pos = begin_pattern_gain_pointer(&mut ui);
        match interruption {
            "focus" => {
                ui.time += 0.1;
                let _ = ui.ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, ui.size)),
                        time: Some(ui.time),
                        focused: false,
                        events: vec![egui::Event::WindowFocused(false)],
                        ..Default::default()
                    },
                    |root| ui.app.ui(root, &mut ui.frame),
                );
            }
            "settings" => {
                ui.key(egui::Key::F10, egui::Modifiers::NONE);
                assert!(ui.app.show_settings);
            }
            "editor" => {
                ui.key(egui::Key::F9, egui::Modifiers::NONE);
                assert_eq!(ui.app.workspace.focused, StudioView::Mixer);
            }
            _ => unreachable!(),
        }
        assert!(
            ui.app.playlist_gesture_before.is_none(),
            "{interruption} must finish the current snapshot owner"
        );
        assert_eq!(ui.app.undo_stack.len(), 1);
        let interrupted = serde_json::to_value(&ui.app.project).unwrap();
        if interruption == "focus" {
            ui.run(vec![egui::Event::WindowFocused(true)]);
        }
        envelope_pointer_frame(&mut ui, pos + Vec2::new(25.0, 0.0), Some(false));
        if interruption == "settings" {
            ui.key(egui::Key::Escape, egui::Modifiers::NONE);
            assert!(!ui.app.show_settings);
        }
        ui.key(egui::Key::F5, egui::Modifiers::NONE);
        ui.settle();
        assert!(ui.app.playlist_gesture_before.is_none());
        assert_eq!(
            serde_json::to_value(&ui.app.project).unwrap(),
            interrupted,
            "{interruption}: release/reentry must not replay the old slider input"
        );
        assert_eq!(ui.app.undo_stack.len(), 1);
        ui.key(egui::Key::Z, piano_clipboard_command());
        assert_eq!(serde_json::to_value(&ui.app.project).unwrap(), original);
        ui.key(egui::Key::Y, piano_clipboard_command());
        assert_eq!(serde_json::to_value(&ui.app.project).unwrap(), interrupted);
    }
}

fn pattern_discrete_edit_ui(grouped: bool) -> UiHarness {
    let mut ui = pattern_history_ui();
    let mut second = pattern_clip();
    second.id += 1;
    second.track = 1;
    if grouped {
        ui.app.project.clips[0].group_id = Some(42);
        second.group_id = Some(42);
    }
    let second_id = second.id;
    ui.app.project.clips.push(second);
    ui.app.playlist_selection_ids.insert(second_id);
    saved_baseline(&mut ui);
    ui
}

#[test]
fn inspector_gain_pointer_cannot_interleave_playlist_discrete_edits() {
    for (key, modifiers, grouped) in [
        (egui::Key::Delete, egui::Modifiers::NONE, false),
        (egui::Key::D, piano_clipboard_command(), false),
        (egui::Key::G, egui::Modifiers::SHIFT, false),
        (egui::Key::G, egui::Modifiers::ALT, true),
    ] {
        let mut ui = pattern_discrete_edit_ui(grouped);
        let mut no_key = pattern_discrete_edit_ui(grouped);
        let original = serde_json::to_value(&ui.app.project).unwrap();
        assert_eq!(serde_json::to_value(&no_key.app.project).unwrap(), original);
        let selection = ui.app.playlist_selection_ids.clone();
        let pos = begin_pattern_gain_pointer(&mut ui);
        let control_pos = begin_pattern_gain_pointer(&mut no_key);
        assert_eq!(pos, control_pos);
        ui.settle();
        no_key.settle();
        let held = serde_json::to_value(&ui.app.project).unwrap();
        assert_eq!(serde_json::to_value(&no_key.app.project).unwrap(), held);

        // Same five frames as UiHarness::key: key down, key up, then settle.
        // The still-held slider legitimately keeps sampling its pointer while
        // layout settles. A no-key peer distinguishes that from a keyboard
        // command changing the Project; compare every frame including gain.
        for (frame, pressed) in [Some(true), Some(false), None, None, None]
            .into_iter()
            .enumerate()
        {
            let events = pressed
                .map(|pressed| egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers,
                })
                .into_iter()
                .collect();
            ui.run(events);
            no_key.run(Vec::new());
            let actual = serde_json::to_value(&ui.app.project).unwrap();
            assert_eq!(
                actual,
                serde_json::to_value(&no_key.app.project).unwrap(),
                "held {key:?} frame {frame} must equal the no-key control, including gain; slider bounds {:?} / {:?}",
                property_slider_rect(&ui, "Gain", 1.5),
                property_slider_rect(&no_key, "Gain", 1.5),
            );
            let mut permitted_pointer_edit = held.clone();
            permitted_pointer_edit["clips"][0]["gain"] = actual["clips"][0]["gain"].clone();
            assert_eq!(
                actual, permitted_pointer_edit,
                "held {key:?} frame {frame}: every non-Gain Project field is unchanged"
            );
            assert_eq!(ui.app.playlist_selection_ids, selection);
            assert_eq!(no_key.app.playlist_selection_ids, selection);
            assert!(ui.app.playlist_gesture_before.is_some());
            assert!(no_key.app.playlist_gesture_before.is_some());
            assert!(ui.app.undo_stack.is_empty());
            assert!(no_key.app.undo_stack.is_empty());
        }
        envelope_pointer_frame(&mut ui, pos, Some(false));
        envelope_pointer_frame(&mut no_key, control_pos, Some(false));
        ui.settle();
        no_key.settle();
        let edited = serde_json::to_value(&ui.app.project).unwrap();
        assert_eq!(serde_json::to_value(&no_key.app.project).unwrap(), edited);
        assert_eq!(ui.app.undo_stack.len(), 1);
        assert_eq!(no_key.app.undo_stack.len(), 1);
        ui.key(egui::Key::Z, piano_clipboard_command());
        assert_eq!(serde_json::to_value(&ui.app.project).unwrap(), original);
        ui.key(egui::Key::Y, piano_clipboard_command());
        assert_eq!(serde_json::to_value(&ui.app.project).unwrap(), edited);
        ui.key(key, modifiers);
        assert_ne!(
            serde_json::to_value(&ui.app.project).unwrap(),
            edited,
            "{key:?} must work again after pointer release"
        );
        assert_eq!(ui.app.undo_stack.len(), 2);
        ui.key(egui::Key::Z, piano_clipboard_command());
        assert_eq!(serde_json::to_value(&ui.app.project).unwrap(), edited);
    }
}

#[test]
fn inspector_gain_pointer_defers_background_recovery_without_clearing_its_deadline() {
    let fixture = Fixture::new();
    let path = fixture.0.join("gesture-recovery.citrus");
    let mut ui = pattern_history_ui();
    let original = serde_json::to_value(&ui.app.project).unwrap();
    let pos = begin_pattern_gain_pointer(&mut ui);
    ui.app.autosave_path = Some(path.clone());
    let due = Instant::now() - Duration::from_secs(60);
    ui.app.last_autosave = due;
    ui.app.autosave_with_notice(true);
    assert_eq!(ui.app.last_autosave, due);
    assert!(ui.app.save_barrier.is_none());
    assert!(!path.exists());
    // The common save entry must also refuse an intermediate background
    // preview when reached directly rather than through the periodic caller.
    ui.app.begin_project_save(ProjectSaveRequest::Autosave {
        path: path.clone(),
        announce: false,
    });
    assert!(ui.app.save_barrier.is_none());
    assert!(ui.app.playlist_gesture_before.is_some());
    assert!(ui.app.undo_stack.is_empty());
    envelope_pointer_frame(&mut ui, pos, Some(false));
    ui.wait_until(|app| app.save_barrier.is_none() && path.exists());
    assert!(ui.app.playlist_gesture_before.is_none());
    assert_eq!(ui.app.undo_stack.len(), 1);
    let edited = serde_json::to_value(&ui.app.project).unwrap();
    assert_eq!(
        serde_json::to_value(Project::load(&path).unwrap()).unwrap(),
        edited
    );
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(serde_json::to_value(&ui.app.project).unwrap(), original);
    ui.key(egui::Key::Y, piano_clipboard_command());
    assert_eq!(serde_json::to_value(&ui.app.project).unwrap(), edited);
}
