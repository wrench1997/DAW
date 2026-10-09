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
