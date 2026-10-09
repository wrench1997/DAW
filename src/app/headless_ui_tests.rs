//! Display-independent integration tests: the actual eframe App::ui, accessibility bounds,
//! and pointer/key events. No display, native dialog, audio device, or user-profile acceptance.

use super::*;
use eframe::App;
use egui::accesskit::{Node, Role};

struct UiHarness {
    ctx: egui::Context,
    app: Box<CitrusApp>,
    frame: eframe::Frame,
    nodes: Vec<Node>,
    time: f64,
    size: Vec2,
    capture: Option<super::headless_ui_capture::OffscreenCapture>,
    capture_name: Option<&'static str>,
}

impl UiHarness {
    fn new() -> Self {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let app = CitrusApp::new_boxed_with_services(&cc, false);
        assert!(app.audio.is_none());
        assert!(app.autosave_path.is_none());
        assert!(app.plugin_cache_path.is_none());
        assert!(app.recordings_dir.is_none());
        assert!(app.audio_device_catalog_pending_generation.is_none());
        let mut harness = Self {
            ctx,
            app,
            frame: eframe::Frame::_new_kittest(),
            nodes: Vec::new(),
            time: 0.0,
            size: egui::vec2(1440.0, 900.0),
            capture: super::headless_ui_capture::OffscreenCapture::from_env(),
            capture_name: None,
        };
        harness.settle();
        harness
    }

    fn run(&mut self, events: Vec<egui::Event>) -> egui::FullOutput {
        // The constructor performs no device discovery. Prevent the periodic MIDI refresh
        // as well; tests never click device refresh/apply or native file-dialog controls.
        self.app.midi_input.last_refresh = Instant::now();
        self.time += 0.1;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)),
            time: Some(self.time),
            events,
            ..Default::default()
        };
        let output = self
            .ctx
            .run_ui(input, |ui| self.app.ui(ui, &mut self.frame));
        if let Some(capture) = &mut self.capture {
            capture.process(&self.ctx, &output, self.capture_name.take());
        }
        self.nodes = output
            .platform_output
            .accesskit_update
            .as_ref()
            .expect("accessibility must be emitted by production widgets")
            .nodes
            .iter()
            .map(|(_, node)| node.clone())
            .collect();
        assert!(
            !output.shapes.is_empty(),
            "real UI must produce paint shapes"
        );
        output
    }

    fn capture(&mut self, name: &'static str) {
        if self.capture.is_some() {
            self.capture_name = Some(name);
            self.run(Vec::new());
        }
    }

    fn settle(&mut self) {
        for _ in 0..3 {
            self.run(Vec::new());
        }
    }

    fn button(&self, label: &str) -> &Node {
        let matches: Vec<_> = self
            .nodes
            .iter()
            .filter(|node| {
                node.label() == Some(label)
                    && matches!(
                        node.role(),
                        Role::Button | Role::RadioButton | Role::MenuItem
                    )
            })
            .collect();
        assert_eq!(
            matches.len(),
            1,
            "expected one button {label:?}; found {matches:?}; available: {:?}",
            self.nodes
                .iter()
                .filter_map(|node| node.label())
                .collect::<Vec<_>>()
        );
        matches[0]
    }

    fn click(&mut self, label: &str) {
        self.settle();
        let node = self.button(label);
        assert!(!node.is_disabled(), "{label} must be enabled");
        let bounds = node.bounds().expect("button must have real layout bounds");
        let pos = Pos2::new(
            ((bounds.x0 + bounds.x1) / 2.0) as f32,
            ((bounds.y0 + bounds.y1) / 2.0) as f32,
        );
        self.click_pos(pos);
    }

    fn click_pos(&mut self, pos: Pos2) {
        assert!(Rect::from_min_size(Pos2::ZERO, self.size).contains(pos));
        for pressed in [true, false] {
            self.run(vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
        }
        self.settle();
    }

    fn key(&mut self, key: egui::Key, modifiers: egui::Modifiers) {
        for pressed in [true, false] {
            self.run(vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers,
            }]);
        }
        self.settle();
    }

    fn menu(&mut self, item: &str) {
        self.click("FILE");
        self.click(item);
    }

    fn wait_until(&mut self, predicate: impl Fn(&CitrusApp) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !predicate(&self.app) {
            assert!(
                Instant::now() < deadline,
                "UI worker did not finish within five seconds"
            );
            self.run(Vec::new());
            std::thread::sleep(Duration::from_millis(2));
        }
        self.settle();
    }
}

#[test]
fn full_app_settings_navigation_and_shortcut_dismissal() {
    let mut ui = UiHarness::new();
    ui.capture("playlist");
    for (key, view) in [
        (egui::Key::F6, StudioView::ChannelRack),
        (egui::Key::F7, StudioView::PianoRoll),
        (egui::Key::F9, StudioView::Mixer),
        (egui::Key::F5, StudioView::Playlist),
    ] {
        ui.key(key, egui::Modifiers::NONE);
        assert_eq!(ui.app.view, view);
        if view == StudioView::Mixer {
            ui.capture("mixer");
        }
    }
    ui.key(egui::Key::F10, egui::Modifiers::NONE);
    assert!(ui.app.show_settings);
    for page in SettingsPage::ALL {
        ui.click(page.label());
        assert_eq!(ui.app.settings_page, page);
        ui.capture(match page {
            SettingsPage::Audio => "settings-audio",
            SettingsPage::Midi => "settings-midi",
            SettingsPage::Files => "settings-files",
            SettingsPage::Project => "settings-project",
            SettingsPage::Debug => "settings-debug",
            SettingsPage::About => "settings-about",
        });
    }
    ui.key(egui::Key::F9, egui::Modifiers::NONE);
    assert_eq!(
        ui.app.view,
        StudioView::Playlist,
        "settings must own shortcuts"
    );
    ui.key(egui::Key::Escape, egui::Modifiers::NONE);
    assert!(!ui.app.show_settings);
    ui.key(egui::Key::F10, egui::Modifiers::NONE);
    assert_eq!(ui.app.settings_page, SettingsPage::About);
    assert!(ui.app.show_settings);
    ui.key(egui::Key::F10, egui::Modifiers::NONE);
    assert!(!ui.app.show_settings);
}

#[test]
fn full_app_export_review_back_close_and_escape_preserve_project() {
    let mut ui = UiHarness::new();
    let original = project_fingerprint(&ui.app.project);
    ui.menu("Export WAV…");
    assert!(ui.app.export_dialog.is_open());
    ui.click(crate::export_options::WavLevelPolicy::PreserveLevel.label());
    ui.click("Review…");
    assert!(!ui.button("Back").is_disabled());
    ui.capture("export-review");
    ui.key(egui::Key::F9, egui::Modifiers::NONE);
    assert_eq!(ui.app.view, StudioView::Playlist);
    ui.click("Back");
    assert_eq!(
        ui.button(crate::export_options::WavLevelPolicy::PreserveLevel.label())
            .toggled(),
        Some(egui::accesskit::Toggled::True)
    );
    ui.click("Review…");
    ui.click("Close");
    assert!(!ui.app.export_dialog.is_open());
    ui.menu("Export WAV…");
    assert_eq!(
        ui.button(crate::export_options::WavLevelPolicy::AttenuatePeaks.label())
            .toggled(),
        Some(egui::accesskit::Toggled::True)
    );
    ui.key(egui::Key::Escape, egui::Modifiers::NONE);
    assert!(!ui.app.export_dialog.is_open());
    assert!(!ui.app.export_job.is_running());
    assert_eq!(project_fingerprint(&ui.app.project), original);
}

#[test]
fn full_app_media_modal_close_escape_and_reopen() {
    let mut ui = UiHarness::new();
    let original = project_fingerprint(&ui.app.project);
    ui.menu("Project media / relink…");
    assert!(ui.app.project_media.open);
    ui.capture("project-media");
    ui.key(egui::Key::F9, egui::Modifiers::NONE);
    assert_eq!(ui.app.view, StudioView::Playlist);
    ui.click("Close");
    assert!(!ui.app.project_media.open);
    ui.menu("Project media / relink…");
    assert!(ui.app.project_media.open);
    ui.key(egui::Key::Escape, egui::Modifiers::NONE);
    assert!(!ui.app.project_media.open);
    assert_eq!(project_fingerprint(&ui.app.project), original);
}

struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "citrus-headless-ui-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&40u32.to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&48000u32.to_le_bytes());
        wav.extend_from_slice(&96000u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&4u32.to_le_bytes());
        wav.extend_from_slice(&1234i16.to_le_bytes());
        wav.extend_from_slice(&(-1234i16).to_le_bytes());
        std::fs::write(root.join("valid.wav"), wav).unwrap();
        std::fs::write(root.join("invalid.wav"), b"not a wav").unwrap();
        Self(root)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn full_app_decoded_waveform_preview_is_real_audio_at_both_window_sizes() {
    let fixture = Fixture::new();
    // A deterministic four-second PCM fixture, clearly named as QA audio.
    // The normal worker must decode it and derive every displayed peak.
    let frames = 48_000_u32 * 4;
    let data_bytes = frames * 2;
    let mut wav = Vec::with_capacity(data_bytes as usize + 44);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&48_000_u32.to_le_bytes());
    wav.extend_from_slice(&96_000_u32.to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_bytes.to_le_bytes());
    for frame in 0..frames {
        let seconds = frame as f32 / 48_000.0;
        let pulse = (1.0 - (seconds * 2.0).fract()).powi(3);
        let sample = ((seconds * 220.0 * std::f32::consts::TAU).sin() * pulse * 24_000.0) as i16;
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    let path = fixture.0.join("QA - decoded pulse.wav");
    std::fs::write(&path, &wav).unwrap();
    let mut ui = UiHarness::new();
    ui.app.sample_browser.navigate(fixture.0.clone());
    ui.wait_until(|app| !app.sample_browser.busy());
    ui.click("QA - decoded pulse.wav");
    ui.click("Import to Playlist");
    ui.wait_until(|app| app.audio_import_receiver.is_none());
    assert!(ui.app.audio_import_error.is_none());
    let asset = ui.app.project.audio_assets.last().unwrap();
    assert_eq!(asset.frames, u64::from(frames));
    assert!(asset.waveform_peaks.len() > 100);
    assert!(asset.waveform_peaks.iter().any(|peak| *peak > 0.5));
    ui.capture("decoded-waveform");
    ui.size = egui::vec2(1080.0, 680.0);
    ui.settle();
    ui.capture("decoded-waveform-minimum-window");
    assert_eq!(std::fs::read(path).unwrap(), wav);
}

#[test]
fn full_app_browser_import_failure_recovery_undo_redo_and_dirty_cancel() {
    let fixture = Fixture::new();
    let mut ui = UiHarness::new();
    assert!(ui.button("Up").is_disabled());
    assert!(ui.button("Import to Playlist").is_disabled());
    // The fixture enters at the native folder-picker return boundary. Listing, selection,
    // validation, import, application/history commit, and all following controls are real.
    ui.app.sample_browser.navigate(fixture.0.clone());
    ui.wait_until(|app| !app.sample_browser.busy());
    let original = project_fingerprint(&ui.app.project);
    let original_clips = ui.app.project.clips.len();
    ui.click("invalid.wav");
    ui.click("Import to Playlist");
    ui.wait_until(|app| app.audio_import_receiver.is_none());
    assert!(ui.app.audio_import_error.is_some());
    ui.capture("browser-import-error");
    assert_eq!(project_fingerprint(&ui.app.project), original);
    assert!(ui.app.undo_stack.is_empty());
    ui.click("valid.wav");
    assert!(ui.app.audio_import_error.is_none());
    ui.click("Import to Playlist");
    ui.wait_until(|app| app.audio_import_receiver.is_none());
    assert!(ui.app.audio_import_error.is_none());
    assert_eq!(ui.app.project.clips.len(), original_clips + 1);
    assert_eq!(ui.app.project.audio_assets.len(), 1);
    assert_eq!(ui.app.undo_stack.len(), 1);
    ui.capture("browser-imported");
    let imported = project_fingerprint(&ui.app.project);
    let command = egui::Modifiers {
        ctrl: true,
        command: true,
        ..Default::default()
    };
    ui.key(egui::Key::Z, command);
    assert_eq!(project_fingerprint(&ui.app.project), original);
    ui.key(egui::Key::Y, command);
    assert_eq!(project_fingerprint(&ui.app.project), imported);
    ui.key(egui::Key::N, command);
    assert!(ui.app.project_lifecycle.is_modal());
    ui.capture("unsaved-project-cancel");
    ui.click("CANCEL");
    assert!(ui.app.project_lifecycle.is_idle());
    assert_eq!(project_fingerprint(&ui.app.project), imported);
    ui.key(egui::Key::N, command);
    assert!(ui.app.project_lifecycle.is_modal());
    ui.key(egui::Key::Escape, egui::Modifiers::NONE);
    assert!(ui.app.project_lifecycle.is_idle());
    assert_eq!(project_fingerprint(&ui.app.project), imported);
    assert_eq!(
        std::fs::read(fixture.0.join("valid.wav")).unwrap().len(),
        48
    );
}

#[test]
fn full_app_browser_search_focus_and_refresh_selection_are_real_ui_events() {
    let fixture = Fixture::new();
    let mut ui = UiHarness::new();
    ui.app.sample_browser.navigate(fixture.0.clone());
    ui.wait_until(|app| !app.sample_browser.busy());
    ui.click("valid.wav");
    assert!(!ui.button("Import to Playlist").is_disabled());
    let fields: Vec<_> = ui
        .nodes
        .iter()
        .filter(|node| node.role() == Role::TextInput)
        .collect();
    assert_eq!(fields.len(), 1, "the browser filter must be identifiable");
    let bounds = fields[0].bounds().unwrap();
    ui.click_pos(Pos2::new(
        ((bounds.x0 + bounds.x1) / 2.0) as f32,
        ((bounds.y0 + bounds.y1) / 2.0) as f32,
    ));
    ui.run(vec![egui::Event::Text("no matching source".into())]);
    ui.settle();
    assert_eq!(ui.app.browser_search, "no matching source");
    assert!(ui.button("Import to Playlist").is_disabled());
    ui.key(egui::Key::F9, egui::Modifiers::NONE);
    assert_eq!(
        ui.app.view,
        StudioView::Playlist,
        "the text editor must own shortcuts"
    );
    let command = egui::Modifiers {
        ctrl: true,
        command: true,
        ..Default::default()
    };
    ui.key(egui::Key::A, command);
    ui.key(egui::Key::Backspace, egui::Modifiers::NONE);
    assert!(ui.app.browser_search.is_empty());
    ui.click("valid.wav");
    ui.click("Refresh");
    ui.wait_until(|app| !app.sample_browser.busy());
    assert!(ui.app.sample_browser.selected.is_none());
    assert!(ui.button("Import to Playlist").is_disabled());
    let subfolder = fixture.0.join("nested");
    std::fs::create_dir(&subfolder).unwrap();
    ui.app.sample_browser.navigate(subfolder);
    ui.wait_until(|app| !app.sample_browser.busy());
    ui.click("Up");
    ui.wait_until(|app| !app.sample_browser.busy());
    assert_eq!(ui.app.sample_browser.directory.as_ref(), Some(&fixture.0));
    assert!(
        ui.app
            .sample_browser
            .entries
            .iter()
            .any(|entry| entry.name == "valid.wav")
    );
}

#[test]
fn full_app_export_footer_remains_clickable_at_minimum_window_size() {
    let mut ui = UiHarness::new();
    ui.size = egui::vec2(1080.0, 680.0);
    ui.settle();
    ui.menu("Export WAV…");
    ui.click("Review…");
    ui.capture("export-review-minimum-window");
    for label in ["Back", "Close", "Choose destination and export…"] {
        let bounds = ui.button(label).bounds().unwrap();
        let rect = Rect::from_min_max(
            Pos2::new(bounds.x0 as f32, bounds.y0 as f32),
            Pos2::new(bounds.x1 as f32, bounds.y1 as f32),
        );
        assert!(
            ui.ctx.content_rect().contains_rect(rect),
            "{label} must remain entirely in the app viewport"
        );
    }
    ui.click("Back");
    ui.click("Close");
    assert!(!ui.app.export_dialog.is_open());
}

fn node_rect(node: &Node) -> Rect {
    let bounds = node.bounds().expect("interactive widget must have bounds");
    Rect::from_min_max(
        Pos2::new(bounds.x0 as f32, bounds.y0 as f32),
        Pos2::new(bounds.x1 as f32, bounds.y1 as f32),
    )
}

fn assert_disjoint(first: Rect, second: Rect) {
    let overlap = first.intersect(second);
    assert!(
        overlap.width() <= 0.0 || overlap.height() <= 0.0,
        "interactive controls overlap: {first:?} and {second:?}"
    );
}

#[test]
fn full_app_responsive_toolbars_keep_navigation_plugins_group_and_snap_separate() {
    for width in [1080.0, 1240.0, 1280.0, 1440.0] {
        let mut ui = UiHarness::new();
        ui.size = egui::vec2(width, 680.0);
        ui.settle();
        let navigation: Vec<_> = ["PLAYLIST", "RACK", "PIANO", "MIXER", "PLUGINS manager"]
            .into_iter()
            .map(|name| node_rect(ui.button(name)))
            .collect();
        for (index, rect) in navigation.iter().enumerate() {
            assert!(
                ui.ctx.content_rect().contains_rect(*rect),
                "navigation escaped at width {width}: {rect:?}"
            );
            for sibling in &navigation[index + 1..] {
                assert_disjoint(*rect, *sibling);
            }
        }
        let snap = ui
            .nodes
            .iter()
            .find(|node| node.role() == Role::ComboBox && node.value() == Some("Step (1/4 beat)"))
            .expect("actual Playlist snap control");
        let snap_rect = node_rect(snap);
        assert!(ui.ctx.content_rect().contains_rect(snap_rect));
        assert_disjoint(node_rect(ui.button("GROUP v")), snap_rect);
        assert_disjoint(node_rect(ui.button("XFADE")), snap_rect);
        if width == 1440.0 {
            assert!(
                (node_rect(ui.button("GROUP v")).center().y - snap_rect.center().y).abs() < 2.0,
                "wide workspace should keep the tools and snap on one row"
            );
        }
        if width == 1080.0 {
            ui.capture("playlist-minimum-window");
        }
        ui.click("MIXER");
        assert_eq!(ui.app.view, StudioView::Mixer);
        if width == 1080.0 {
            ui.capture("mixer-minimum-window");
        }
        ui.click("PLUGINS manager");
        assert!(ui.app.show_plugins);
        ui.key(egui::Key::Escape, egui::Modifiers::NONE);
        assert!(!ui.app.show_plugins);
        ui.click("PLAYLIST");
        assert_eq!(ui.app.view, StudioView::Playlist);
        ui.click("GROUP v");
        assert!(egui::Popup::is_any_open(&ui.ctx));
        ui.key(egui::Key::Escape, egui::Modifiers::NONE);
        assert!(!egui::Popup::is_any_open(&ui.ctx));
        let snap = ui
            .nodes
            .iter()
            .find(|node| node.role() == Role::ComboBox && node.value() == Some("Step (1/4 beat)"))
            .unwrap();
        ui.click_pos(node_rect(snap).center());
        ui.click("1 beat");
        assert_eq!(ui.app.snap, 1.0);
    }
}

#[test]
fn full_app_about_reports_platform_and_offline_state_without_an_assumed_backend() {
    let mut ui = UiHarness::new();
    ui.key(egui::Key::F10, egui::Modifiers::NONE);
    ui.click("About");
    let text: Vec<_> = ui
        .nodes
        .iter()
        .filter_map(|node| node.value().or_else(|| node.label()))
        .collect();
    assert!(text.contains(&std::env::consts::OS));
    assert!(text.contains(&"CPAL / offline (no active stream)"));
    #[cfg(target_os = "linux")]
    assert!(!text.iter().any(|label| label.contains("WASAPI")));
    ui.capture("settings-about-platform");
}

struct MixerControlFrame {
    gain: Rect,
    pan: Rect,
    thumb: Rect,
}

fn run_mixer_controls(
    ctx: &egui::Context,
    gain: &mut f32,
    pan: &mut f32,
    height: f32,
    events: Vec<egui::Event>,
    time: &mut f64,
) -> MixerControlFrame {
    *time += 0.1;
    let mut gain_rect = Rect::NOTHING;
    let mut pan_rect = Rect::NOTHING;
    let output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(320.0, 260.0))),
            time: Some(*time),
            events,
            ..Default::default()
        },
        |ui| {
            ui.add_space(24.0);
            ui.horizontal_top(|ui| {
                gain_rect = vertical_fader(ui, gain, height).rect;
                pan_rect = mixer_pan_knob(ui, pan, theme::ORANGE).rect;
            });
        },
    );
    let thumb = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Rect(rect) if rect.fill == Color32::from_rgb(123, 137, 147) => {
                Some(rect.rect)
            }
            _ => None,
        })
        .expect("the production fader must paint its actual thumb");
    assert!(gain_rect.contains_rect(thumb));
    assert!(gain_rect.contains_rect(thumb.translate(Vec2::new(0.0, 2.0))));
    MixerControlFrame {
        gain: gain_rect,
        pan: pan_rect,
        thumb,
    }
}

fn mixer_pointer_button(pos: Pos2, pressed: bool) -> Vec<egui::Event> {
    vec![
        egui::Event::PointerMoved(pos),
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        },
    ]
}

#[test]
fn mixer_fader_pointer_uses_painted_thumb_range_at_endpoints_and_midpoint() {
    for height in [70.0, 180.0] {
        for initial_gain in [0.0, 0.5, 1.0] {
            let ctx = egui::Context::default();
            let mut gain = initial_gain;
            let mut pan = 0.0;
            let mut time = 0.0;
            let mut frame =
                run_mixer_controls(&ctx, &mut gain, &mut pan, height, Vec::new(), &mut time);
            for _ in 0..2 {
                frame =
                    run_mixer_controls(&ctx, &mut gain, &mut pan, height, Vec::new(), &mut time);
            }
            let center = frame.thumb.center();
            for pressed in [true, false] {
                run_mixer_controls(
                    &ctx,
                    &mut gain,
                    &mut pan,
                    height,
                    mixer_pointer_button(center, pressed),
                    &mut time,
                );
            }
            assert!(
                (gain - initial_gain).abs() < 1e-6,
                "clicking the painted thumb changed gain {initial_gain} to {gain} at height {height}"
            );

            let target = if initial_gain < 0.75 {
                Pos2::new(center.x, frame.gain.top() + 8.0)
            } else {
                Pos2::new(center.x, frame.gain.bottom() - 10.0)
            };
            run_mixer_controls(
                &ctx,
                &mut gain,
                &mut pan,
                height,
                mixer_pointer_button(center, true),
                &mut time,
            );
            for _ in 0..2 {
                run_mixer_controls(
                    &ctx,
                    &mut gain,
                    &mut pan,
                    height,
                    vec![egui::Event::PointerMoved(target)],
                    &mut time,
                );
            }
            run_mixer_controls(
                &ctx,
                &mut gain,
                &mut pan,
                height,
                mixer_pointer_button(target, false),
                &mut time,
            );
            assert_eq!(gain, if initial_gain < 0.75 { 1.0 } else { 0.0 });
        }
    }
}

#[test]
fn mixer_pan_knob_pointer_drag_changes_pan_without_changing_gain() {
    let ctx = egui::Context::default();
    let mut gain = 0.5;
    let mut pan = 0.0;
    let mut time = 0.0;
    let mut frame = run_mixer_controls(&ctx, &mut gain, &mut pan, 100.0, Vec::new(), &mut time);
    for _ in 0..2 {
        frame = run_mixer_controls(&ctx, &mut gain, &mut pan, 100.0, Vec::new(), &mut time);
    }
    let origin = frame.pan.center();
    run_mixer_controls(
        &ctx,
        &mut gain,
        &mut pan,
        100.0,
        mixer_pointer_button(origin, true),
        &mut time,
    );
    for delta in [10.0, 20.0, 30.0] {
        run_mixer_controls(
            &ctx,
            &mut gain,
            &mut pan,
            100.0,
            vec![egui::Event::PointerMoved(origin - Vec2::new(0.0, delta))],
            &mut time,
        );
    }
    run_mixer_controls(
        &ctx,
        &mut gain,
        &mut pan,
        100.0,
        mixer_pointer_button(origin - Vec2::new(0.0, 30.0), false),
        &mut time,
    );
    assert!(
        pan > 0.0,
        "vertical dragging must update the real pan control"
    );
    assert_eq!(gain, 0.5);
}
