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
        Self::with_storage(None, true)
    }

    fn with_storage(storage: Option<&dyn eframe::Storage>, maximized_baseline: bool) -> Self {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut cc = eframe::CreationContext::_new_kittest(ctx.clone());
        cc.storage = storage;
        let mut app = CitrusApp::new_boxed_with_services(&cc, false);
        // These pre-existing app-flow checks exercise the supported maximized editor layout.
        // Dedicated multiwindow tests below exercise the default floating workspace.
        if maximized_baseline {
            app.show_inspector = true;
            app.workspace.maximized = true;
            app.focus_editor(StudioView::Playlist);
        }
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
        self.run_with_modifiers(events, egui::Modifiers::NONE)
    }

    fn run_with_modifiers(
        &mut self,
        events: Vec<egui::Event>,
        modifiers: egui::Modifiers,
    ) -> egui::FullOutput {
        // The constructor performs no device discovery. Prevent the periodic MIDI refresh
        // as well; tests never click device refresh/apply or native file-dialog controls.
        self.app.midi_input.last_refresh = Instant::now();
        self.time += 0.1;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)),
            time: Some(self.time),
            modifiers,
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
        assert_eq!(ui.app.workspace.focused, view);
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
        ui.app.workspace.focused,
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
    assert_eq!(ui.app.workspace.focused, StudioView::Playlist);
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
    assert_eq!(ui.app.workspace.focused, StudioView::Playlist);
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
        ui.app.workspace.focused,
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
        let navigation: Vec<_> = [
            "CLICK OFF",
            "PLAYLIST",
            "RACK",
            "PIANO",
            "MIXER",
            "PLUGINS manager",
        ]
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
        assert_eq!(ui.app.workspace.focused, StudioView::Mixer);
        if width == 1080.0 {
            ui.capture("mixer-minimum-window");
        }
        ui.click("PLUGINS manager");
        assert!(ui.app.show_plugins);
        ui.key(egui::Key::Escape, egui::Modifiers::NONE);
        assert!(!ui.app.show_plugins);
        ui.click("PLAYLIST");
        assert_eq!(ui.app.workspace.focused, StudioView::Playlist);
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

#[test]
fn full_app_native_inspector_actions_fit_narrow_and_wide_panels() {
    let mut failures = Vec::new();
    for width in [240.0, 340.0] {
        for view in [StudioView::ChannelRack, StudioView::Mixer] {
            let mut ui = UiHarness::new();
            ui.app.focus_editor(view);
            ui.app.selected_channel = 0;
            ui.app.selected_mixer = 1;
            let target = if view == StudioView::ChannelRack {
                PluginPickerTarget::ChannelDevice { channel: 0 }
            } else {
                PluginPickerTarget::MixerSlot { track: 1, slot: 0 }
            };
            // Model-only descriptor: no file is created and no plug-in is loaded.
            let id = commit_loaded_plugin(
                &mut ui.app.project,
                target,
                &PluginDescriptor {
                    id: "layout-only-native-editor".into(),
                    name: "UI test only: native editor".into(),
                    vendor: "UI TEST ONLY".into(),
                    path: PathBuf::from("/not-a-real-plugin/ui-layout-only.vst3"),
                    format: ScannedPluginFormat::Vst3,
                    category: String::new(),
                    is_instrument: view == StudioView::ChannelRack,
                    verified: false,
                    vst3_metadata: None,
                    scan_error: None,
                },
            )
            .unwrap();
            let mut snapshot = NativeEditorSnapshot::default();
            snapshot.state.supported = true;
            snapshot.state.has_editor = true;
            snapshot.state.open = true;
            ui.app.native_editor_ui_test_snapshot = Some((id, snapshot));
            assert!(ui.app.native_editor_snapshot(id).is_none());
            let original_project = project_fingerprint(&ui.app.project);
            let mut state =
                egui::containers::panel::PanelState::load(&ui.ctx, egui::Id::new("inspector"))
                    .unwrap();
            state.outer_rect.min.x = state.outer_rect.max.x - width;
            ui.ctx
                .data_mut(|data| data.insert_persisted(egui::Id::new("inspector"), state));
            ui.run(Vec::new());
            let first_frame_labels: &[&str] = if view == StudioView::ChannelRack {
                &["REPLACE…", "EDITOR", "CLOSE EDITOR", "PARAMETERS", "REMOVE"]
            } else {
                &["EDITOR", "CLOSE UI", "PARAMS", "X", "LOAD", "BYPASS"]
            };
            for label in first_frame_labels {
                let rect = node_rect(ui.button(label));
                if !state.outer_rect.shrink(10.0).contains_rect(rect) {
                    failures.push(format!("{view:?} first frame width {width}: {label} escaped requested panel: {rect:?} versus {:?}", state.outer_rect));
                }
            }
            ui.settle();
            let panel =
                egui::containers::panel::PanelState::load(&ui.ctx, egui::Id::new("inspector"))
                    .unwrap()
                    .outer_rect;
            let labels: &[&str] = if view == StudioView::ChannelRack {
                &["REPLACE…", "EDITOR", "CLOSE EDITOR", "PARAMETERS", "REMOVE"]
            } else {
                &["EDITOR", "CLOSE UI", "PARAMS", "X", "LOAD", "BYPASS"]
            };
            ui.capture(match (view, width as u32) {
                (StudioView::ChannelRack, 240) => "native-generator-inspector-240",
                (StudioView::ChannelRack, _) => "native-generator-inspector-340",
                (StudioView::Mixer, 240) => "native-effect-inspector-240",
                _ => "native-effect-inspector-340",
            });
            let rects: Vec<_> = labels
                .iter()
                .map(|label| (*label, node_rect(ui.button(label))))
                .collect();
            eprintln!("{view:?} width {width}: panel {panel:?}; actions {rects:?}");
            for (index, (label, rect)) in rects.iter().enumerate() {
                if !panel.shrink(10.0).contains_rect(*rect) {
                    failures.push(format!(
                        "{view:?} width {width}: {label} escaped panel: {rect:?} versus {panel:?}"
                    ));
                }
                for (other_label, other) in &rects[index + 1..] {
                    let overlap = rect.intersect(*other);
                    if overlap.width() > 0.0 && overlap.height() > 0.0 {
                        failures.push(format!(
                            "{view:?} width {width}: {label} overlaps {other_label}"
                        ));
                    }
                }
            }
            // Status remains readable, with every action on a separate, nonoverlapping row.
            for status in ["VST3", "Runtime pending"] {
                let node = ui
                    .nodes
                    .iter()
                    .find(|node| {
                        node.value().or_else(|| node.label()) == Some(status)
                            && node.bounds().is_some()
                            && panel.contains_rect(node_rect(node))
                    })
                    .expect("plug-in status text must remain inside the inspector");
                for (_, rect) in &rects {
                    assert_disjoint(node_rect(node), *rect);
                }
            }
            let close_label = if view == StudioView::ChannelRack {
                "CLOSE EDITOR"
            } else {
                "CLOSE UI"
            };
            for (supported, has_editor, open, pending, enabled) in [
                (true, true, false, None, true),
                (true, true, true, Some(7), false),
                (false, false, false, None, false),
                (true, false, false, None, false),
                (true, true, true, None, true),
            ] {
                let snapshot = &mut ui.app.native_editor_ui_test_snapshot.as_mut().unwrap().1;
                snapshot.state.supported = supported;
                snapshot.state.has_editor = has_editor;
                snapshot.state.open = open;
                snapshot.pending_request = pending;
                ui.settle();
                assert_eq!(!ui.button("EDITOR").is_disabled(), enabled);
                assert_eq!(
                    ui.nodes
                        .iter()
                        .any(|node| node.label() == Some(close_label)),
                    open
                );
                for label in labels.iter().filter(|label| open || **label != close_label) {
                    let rect = node_rect(ui.button(label));
                    assert!(
                        panel.shrink(10.0).contains_rect(rect),
                        "{label} escaped in native presentation state {supported}/{has_editor}/{open}/{pending:?}"
                    );
                    assert!(ui.ctx.content_rect().contains_rect(rect));
                }
                assert!(
                    ui.app.native_editor_snapshot(id).is_none(),
                    "presentation fixture must never create native runtime state"
                );
            }
            // Operate a real action on each wrapped row; Escape/reopen must retain the
            // target and leave project data unchanged without loading any plug-in.
            let replace_label = if view == StudioView::ChannelRack {
                "REPLACE…"
            } else {
                "LOAD"
            };
            for _ in 0..2 {
                ui.click(replace_label);
                assert!(ui.app.show_plugins);
                assert_eq!(ui.app.plugin_picker_target, Some(target));
                ui.key(egui::Key::Escape, egui::Modifiers::NONE);
                assert!(!ui.app.show_plugins);
                assert!(ui.app.plugin_picker_target.is_none());
            }
            assert_eq!(project_fingerprint(&ui.app.project), original_project);
            assert!(ui.app.running_generator_chains.is_empty());
            assert!(ui.app.running_insert_chains.is_empty());
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn full_app_metronome_pointer_toggle_persists_without_project_history_or_export_click() {
    let mut ui = UiHarness::new();
    let original = project_fingerprint(&ui.app.project);
    assert!(!ui.app.audio_preferences.metronome_enabled);
    assert!(!ui.app.dirty);
    ui.click("CLICK OFF");
    assert!(ui.app.audio_preferences.metronome_enabled);
    assert!(ui.app.audio_preferences_dirty);
    ui.key(egui::Key::Space, egui::Modifiers::NONE);
    assert!(ui.app.playing);
    ui.click("CLICK ON");
    assert!(!ui.app.audio_preferences.metronome_enabled);
    assert!(ui.app.playing, "click toggle must not pause playback");
    ui.click("CLICK OFF");
    assert!(ui.app.playing);
    ui.key(egui::Key::Escape, egui::Modifiers::NONE);
    assert!(!ui.app.playing);
    assert!(ui.app.audio_preferences.metronome_enabled);
    assert_eq!(project_fingerprint(&ui.app.project), original);
    assert!(!ui.app.dirty);
    assert!(ui.app.undo_stack.is_empty());
    assert!(ui.app.redo_stack.is_empty());
    ui.capture("metronome-enabled");

    let mut storage = WorkspaceTestStorage::default();
    ui.app.save(&mut storage);
    let mut restored = UiHarness::with_storage(Some(&storage), true);
    assert!(restored.app.audio_preferences.metronome_enabled);
    restored.button("CLICK ON");
    restored.click("CLICK ON");
    restored.app.save(&mut storage);
    let restored_off = UiHarness::with_storage(Some(&storage), true);
    assert!(!restored_off.app.audio_preferences.metronome_enabled);
    restored_off.button("CLICK OFF");

    // Offline rendering takes only Project music, regardless of the app click preference.
    let fixture = Fixture::new();
    let mut blank = Project::blank();
    blank.tempo = 120.0;
    blank.song_length_beats = 2.0;
    let off_path = fixture.0.join("metronome-off.wav");
    let on_path = fixture.0.join("metronome-on.wav");
    restored.app.set_metronome_enabled(false);
    export::render_project_wav(&blank, &off_path, 8_000).unwrap();
    restored.app.set_metronome_enabled(true);
    export::render_project_wav(&blank, &on_path, 8_000).unwrap();
    assert_eq!(
        std::fs::read(&off_path).unwrap(),
        std::fs::read(&on_path).unwrap()
    );
    let decoded = crate::wav::read_wav(&on_path).unwrap();
    assert_eq!(decoded.metadata.frames, 8_000);
    assert!(decoded.samples.iter().all(|sample| *sample == 0.0));
}

#[derive(Default)]
struct WorkspaceTestStorage(HashMap<String, String>);
impl eframe::Storage for WorkspaceTestStorage {
    fn get_string(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }
    fn set_string(&mut self, key: &str, value: String) {
        self.0.insert(key.into(), value);
    }
    fn remove_string(&mut self, key: &str) {
        self.0.remove(key);
    }
    fn flush(&mut self) {}
}

impl UiHarness {
    fn floating() -> Self {
        let mut ui = Self::with_storage(None, false);
        ui.size = Vec2::new(1920.0, 1080.0);
        ui.app.workspace = workspace::Workspace::default();
        ui.settle();
        ui
    }

    fn editor_rect(&self, view: StudioView) -> Rect {
        self.ctx
            .memory(|memory| memory.area_rect(workspace::window_id(view)))
            .unwrap()
    }

    fn drag_pointer(&mut self, origin: Pos2, delta: Vec2) {
        self.run(mixer_pointer_button(origin, true));
        for step in 1..=6 {
            self.run(vec![egui::Event::PointerMoved(
                origin + delta * (step as f32 / 6.0),
            )]);
        }
        self.run(mixer_pointer_button(origin + delta, false));
        self.settle();
    }

    fn close_editor_by_pointer(&mut self, view: StudioView) {
        let rect = self.editor_rect(view);
        let node = self
            .nodes
            .iter()
            .find(|node| {
                node.label() == Some("Close window")
                    && node.bounds().is_some_and(|bounds| {
                        rect.contains(Pos2::new(
                            ((bounds.x0 + bounds.x1) / 2.0) as f32,
                            ((bounds.y0 + bounds.y1) / 2.0) as f32,
                        ))
                    })
            })
            .expect("editor close button must be accessible");
        let bounds = node.bounds().unwrap();
        self.click_pos(Pos2::new(
            ((bounds.x0 + bounds.x1) / 2.0) as f32,
            ((bounds.y0 + bounds.y1) / 2.0) as f32,
        ));
    }
}

#[test]
fn floating_workspace_drag_resize_close_reopen_and_layout_storage() {
    let mut ui = UiHarness::floating();
    let project = project_fingerprint(&ui.app.project);
    let selection = (
        ui.app.selected_channel,
        ui.app.selected_clip,
        ui.app.selected_mixer,
    );
    assert!(ui.app.workspace.windows.iter().all(|window| window.visible));
    for view in workspace::EDITORS {
        assert!(ui.editor_rect(view).is_positive());
    }
    assert!(
        ui.app.piano_viewport.y.origin() > 20.0,
        "window sizing must preserve the initial musical pitch region: {:?}",
        ui.app.piano_viewport.y
    );
    ui.capture("multiwindow-workspace");

    ui.key(egui::Key::F5, egui::Modifiers::NONE);
    let before = ui.editor_rect(StudioView::Playlist);
    let title = before.left_top() + Vec2::new(110.0, 13.0);
    ui.drag_pointer(title, Vec2::new(42.0, 35.0));
    let moved = ui.editor_rect(StudioView::Playlist);
    assert!(
        moved.left() > before.left() + 20.0,
        "title-bar drag must move the real window: {before:?} -> {moved:?}"
    );
    assert!(moved.top() > before.top() + 20.0);
    ui.drag_pointer(
        moved.right_bottom() - Vec2::splat(2.0),
        Vec2::new(-90.0, -45.0),
    );
    let resized = ui.editor_rect(StudioView::Playlist);
    assert!(
        resized.width() < moved.width() - 40.0,
        "resize must shrink the real window: {moved:?} -> {resized:?}"
    );
    assert_eq!(
        project_fingerprint(&ui.app.project),
        project,
        "window chrome must never edit clips or notes beneath it"
    );
    assert_eq!(
        (
            ui.app.selected_channel,
            ui.app.selected_clip,
            ui.app.selected_mixer
        ),
        selection
    );
    assert!(ui.app.undo_stack.is_empty());

    ui.close_editor_by_pointer(StudioView::Playlist);
    assert!(!ui.app.workspace.windows[workspace::index(StudioView::Playlist)].visible);
    assert_eq!(project_fingerprint(&ui.app.project), project);
    ui.key(egui::Key::F5, egui::Modifiers::NONE);
    assert!(ui.app.workspace.windows[workspace::index(StudioView::Playlist)].visible);
    assert_eq!(ui.app.workspace.focused, StudioView::Playlist);
    assert!((ui.editor_rect(StudioView::Playlist).width() - resized.width()).abs() < 2.0);
    ui.click("Maximize editor");
    assert!(ui.app.workspace.maximized);
    ui.click("Restore windows");
    assert!(!ui.app.workspace.maximized);
    ui.capture("multiwindow-moved-resized");

    let mut storage = WorkspaceTestStorage::default();
    ui.app.save(&mut storage);
    let windows = ui.app.workspace.windows;
    let order = ui.app.workspace.order;
    let mut restored = UiHarness::with_storage(Some(&storage), false);
    restored.size = ui.size;
    // Initial test viewport was smaller. Reload the persisted geometry at the matching size.
    restored.app.workspace = workspace::Workspace::load(Some(&storage));
    restored.settle();
    assert_eq!(restored.app.workspace.focused, StudioView::Playlist);
    assert_eq!(restored.app.workspace.order, order);
    for view in workspace::EDITORS {
        let expected = windows[workspace::index(view)].rect.unwrap();
        let actual = restored.app.workspace.windows[workspace::index(view)]
            .rect
            .unwrap();
        assert!(
            (actual.min - expected.min).length() < 2.0,
            "{view:?}: {actual:?} vs {expected:?}"
        );
        assert!((actual.size() - expected.size()).length() < 2.0);
    }
    restored.size = Vec2::new(1080.0, 680.0);
    restored.settle();
    let viewport = restored.ctx.content_rect();
    for view in workspace::EDITORS {
        assert!(
            viewport.contains_rect(restored.editor_rect(view)),
            "{view:?} must remain on screen after resize"
        );
    }
    restored.capture("multiwindow-minimum-window");
}

#[test]
fn floating_workspace_focus_routes_supported_edits_and_shared_undo_once() {
    let mut ui = UiHarness::floating();
    let command = egui::Modifiers {
        ctrl: true,
        command: true,
        ..Default::default()
    };
    let clip_id = ui.app.project.clips[0].id;
    let note_id = ui.app.project.active_pattern().notes[0].id;
    ui.app.playlist_selection_ids.insert(clip_id);
    ui.app.selected_clip = Some(clip_id);
    ui.app.piano_roll_state.selection_ids.insert(note_id);
    let clips = ui.app.project.clips.len();
    let notes = ui.app.project.active_pattern().notes.len();

    ui.key(egui::Key::F9, egui::Modifiers::NONE);
    let mixer = ui.editor_rect(StudioView::Mixer);
    ui.click_pos(mixer.left_top() + Vec2::new(100.0, 13.0));
    ui.key(egui::Key::Delete, egui::Modifiers::NONE);
    ui.key(egui::Key::D, command);
    assert_eq!(ui.app.project.clips.len(), clips);
    assert_eq!(ui.app.project.active_pattern().notes.len(), notes);
    assert!(ui.app.playlist_selection_ids.contains(&clip_id));
    assert!(ui.app.piano_roll_state.selection_ids.contains(&note_id));

    ui.key(egui::Key::F7, egui::Modifiers::NONE);
    ui.key(egui::Key::B, command);
    assert_eq!(ui.app.project.active_pattern().notes.len(), notes + 1);
    assert_eq!(ui.app.project.clips.len(), clips);
    ui.key(egui::Key::Z, command);
    assert_eq!(ui.app.project.active_pattern().notes.len(), notes);
    ui.app.piano_roll_state.selection_ids.insert(note_id);
    ui.key(egui::Key::Delete, egui::Modifiers::NONE);
    assert_eq!(ui.app.project.active_pattern().notes.len(), notes - 1);
    assert_eq!(ui.app.project.clips.len(), clips);
    ui.key(egui::Key::Z, command);
    assert_eq!(ui.app.project.active_pattern().notes.len(), notes);

    ui.key(egui::Key::F5, egui::Modifiers::NONE);
    ui.app.playlist_selection_ids.insert(clip_id);
    ui.key(egui::Key::D, command);
    assert_eq!(ui.app.project.clips.len(), clips + 1);
    assert_eq!(ui.app.project.active_pattern().notes.len(), notes);
    ui.key(egui::Key::Z, command);
    assert_eq!(ui.app.project.clips.len(), clips);
    ui.app.playlist_selection_ids.insert(clip_id);
    ui.key(egui::Key::Delete, egui::Modifiers::NONE);
    assert_eq!(ui.app.project.clips.len(), clips - 1);
    ui.key(egui::Key::F9, egui::Modifiers::NONE);
    ui.key(egui::Key::Z, command);
    assert_eq!(
        ui.app.project.clips.len(),
        clips,
        "Undo is one shared project history even when Mixer owns focus"
    );

    for view in [
        StudioView::Playlist,
        StudioView::ChannelRack,
        StudioView::Mixer,
    ] {
        ui.app.focus_editor(view);
        ui.settle();
        let before = project_fingerprint(&ui.app.project);
        for key in [egui::Key::A, egui::Key::C, egui::Key::X, egui::Key::V] {
            ui.key(key, command);
        }
        ui.run(vec![
            egui::Event::Copy,
            egui::Event::Cut,
            egui::Event::Paste("unrelated text".into()),
        ]);
        assert_eq!(
            project_fingerprint(&ui.app.project),
            before,
            "unsupported canvas clipboard/select-all chords remain no-ops"
        );
    }
}

#[test]
fn floating_workspace_shared_channel_edit_and_modal_text_isolation() {
    let mut ui = UiHarness::floating();
    ui.key(egui::Key::F6, egui::Modifiers::NONE);
    let channel_name = ui.app.project.channels[1].name.clone();
    ui.click(&channel_name);
    assert_eq!(ui.app.selected_channel, 1);
    ui.settle();
    let output = ui.run(Vec::new());
    fn contains_text(shape: &egui::Shape, expected: &str) -> bool {
        match shape {
            egui::Shape::Text(text) => text.galley.text().contains(expected),
            egui::Shape::Vec(shapes) => shapes.iter().any(|shape| contains_text(shape, expected)),
            _ => false,
        }
    }
    assert!(
        output
            .shapes
            .iter()
            .any(|shape| contains_text(&shape.shape, &format!("{} ·", channel_name))),
        "the simultaneously painted Piano editor must show the shared Rack channel selection"
    );
    let step_before = ui.app.project.active_pattern().channel_steps[1][0];
    let notes_before = serde_json::to_string(&ui.app.project.active_pattern().notes).unwrap();
    ui.click(&format!("{} step 1", channel_name));
    assert_eq!(
        ui.app.project.active_pattern().channel_steps[1][0],
        !step_before
    );
    assert_eq!(
        serde_json::to_string(&ui.app.project.active_pattern().notes).unwrap(),
        notes_before
    );
    ui.capture("multiwindow-shared-pattern-edit");
    let before = project_fingerprint(&ui.app.project);
    ui.key(egui::Key::F10, egui::Modifiers::NONE);
    assert!(ui.app.show_settings);
    let focus = ui.app.workspace.focused;
    ui.key(egui::Key::F5, egui::Modifiers::NONE);
    ui.key(egui::Key::Delete, egui::Modifiers::NONE);
    assert_eq!(ui.app.workspace.focused, focus);
    let rack_rect = ui.editor_rect(StudioView::ChannelRack);
    ui.click_pos(rack_rect.left_top() + Vec2::new(110.0, 13.0));
    assert_eq!(ui.app.workspace.focused, focus);
    assert_eq!(project_fingerprint(&ui.app.project), before);
    ui.key(egui::Key::Escape, egui::Modifiers::NONE);
    assert!(!ui.app.show_settings);

    let fields: Vec<_> = ui
        .nodes
        .iter()
        .filter(|node| node.role() == Role::TextInput)
        .collect();
    let field = fields
        .iter()
        .find(|node| node.bounds().is_some_and(|bounds| bounds.x0 < 220.0))
        .unwrap();
    let bounds = field.bounds().unwrap();
    ui.click_pos(Pos2::new(
        ((bounds.x0 + bounds.x1) / 2.0) as f32,
        ((bounds.y0 + bounds.y1) / 2.0) as f32,
    ));
    ui.run(vec![egui::Event::Text("browser test".into())]);
    let command = egui::Modifiers {
        ctrl: true,
        command: true,
        ..Default::default()
    };
    ui.key(egui::Key::A, command);
    ui.run(vec![egui::Event::Copy]);
    ui.run(vec![egui::Event::Cut]);
    assert!(ui.app.browser_search.is_empty());
    ui.run(vec![egui::Event::Paste("new filter".into())]);
    assert_eq!(ui.app.browser_search, "new filter");
    ui.key(egui::Key::F7, egui::Modifiers::NONE);
    ui.key(egui::Key::Delete, egui::Modifiers::NONE);
    assert_eq!(ui.app.workspace.focused, focus);
    assert_eq!(project_fingerprint(&ui.app.project), before);
}

#[test]
fn floating_workspace_all_hidden_keeps_project_and_reopens_keyboard_target() {
    let mut ui = UiHarness::floating();
    let clip_id = ui.app.project.clips[0].id;
    ui.app.playlist_selection_ids.insert(clip_id);
    let before = project_fingerprint(&ui.app.project);
    for _ in 0..4 {
        ui.click("Hide editor");
    }
    assert!(
        ui.app
            .workspace
            .windows
            .iter()
            .all(|window| !window.visible)
    );
    let command = egui::Modifiers {
        ctrl: true,
        command: true,
        ..Default::default()
    };
    ui.key(egui::Key::Delete, egui::Modifiers::NONE);
    ui.key(egui::Key::D, command);
    assert_eq!(project_fingerprint(&ui.app.project), before);
    ui.key(egui::Key::F5, egui::Modifiers::NONE);
    assert!(ui.app.workspace.windows[workspace::index(StudioView::Playlist)].visible);
    assert_eq!(ui.app.workspace.focused, StudioView::Playlist);
    let clips = ui.app.project.clips.len();
    ui.key(egui::Key::Delete, egui::Modifiers::NONE);
    assert_eq!(ui.app.project.clips.len(), clips - 1);
}

#[test]
fn floating_workspace_actual_stacking_survives_restore_and_blocked_clicks() {
    let mut ui = UiHarness::floating();
    ui.click("Cascade windows");
    ui.key(egui::Key::F7, egui::Modifiers::NONE);
    ui.click("Maximize editor");
    ui.key(egui::Key::F5, egui::Modifiers::NONE);
    ui.click("Restore windows");
    for _ in 0..8 {
        ui.run(Vec::new());
    }
    let layers = ui
        .ctx
        .memory(|memory| memory.layer_ids().collect::<Vec<_>>());
    let editor_layers: Vec<_> = layers
        .into_iter()
        .filter_map(|layer| {
            workspace::EDITORS
                .into_iter()
                .find(|view| layer.id == workspace::window_id(*view))
        })
        .collect();
    assert_eq!(editor_layers, ui.app.workspace.order);
    let overlap = ui
        .editor_rect(StudioView::Playlist)
        .intersect(ui.editor_rect(StudioView::PianoRoll));
    assert!(overlap.is_positive());
    assert_eq!(
        ui.ctx.layer_id_at(overlap.center()).unwrap().id,
        workspace::window_id(StudioView::Playlist)
    );
    ui.click("Arrange windows");
    ui.click("Cascade windows");
    for _ in 0..8 {
        ui.run(Vec::new());
    }
    let overlap = ui
        .editor_rect(StudioView::Mixer)
        .intersect(ui.editor_rect(StudioView::PianoRoll));
    assert_eq!(
        ui.ctx.layer_id_at(overlap.center()).unwrap().id,
        workspace::window_id(StudioView::PianoRoll)
    );

    ui.key(egui::Key::F10, egui::Modifiers::NONE);
    let settings_layer = ui
        .ctx
        .top_layer_id()
        .expect("the Settings window must have a layer");
    let settings_id = settings_layer.id;
    let settings = ui
        .ctx
        .memory(|memory| memory.area_rect(settings_id))
        .unwrap();
    let editor = ui.editor_rect(StudioView::Playlist);
    let exposed = editor.left_top() + Vec2::new(35.0, 12.0);
    assert!(!settings.contains(exposed));
    ui.click_pos(exposed);
    assert_eq!(
        ui.ctx.layer_id_at(settings.center()).unwrap().id,
        settings_id,
        "a disabled editor must not cover the active Settings dialog"
    );
    assert!(ui.app.show_settings);
    let title = settings.left_top() + Vec2::new(140.0, 15.0);
    ui.drag_pointer(title, Vec2::new(32.0, 24.0));
    let moved = ui
        .ctx
        .memory(|memory| memory.area_rect(settings_id))
        .unwrap();
    assert!(
        (moved.min - settings.min).length() > 20.0,
        "an already open Settings window must retain its own pointer dragging"
    );
}

#[test]
fn floating_workspace_unfocused_resize_and_knob_drag_work_on_first_press() {
    let mut ui = UiHarness::floating();
    for _ in 0..8 {
        ui.run(Vec::new());
    }
    assert_eq!(ui.app.workspace.focused, StudioView::PianoRoll);
    let before = ui.editor_rect(StudioView::Mixer);
    let origin = before.left_bottom() + Vec2::new(2.0, -2.0);
    assert_eq!(
        ui.ctx.layer_id_at(origin).unwrap().id,
        workspace::window_id(StudioView::Mixer)
    );
    ui.drag_pointer(origin, Vec2::new(48.0, -36.0));
    let after = ui.editor_rect(StudioView::Mixer);
    assert!(
        after.width() < before.width() - 25.0,
        "first drag on an unfocused resize edge must work: {before:?} -> {after:?}"
    );
    assert_eq!(ui.app.workspace.focused, StudioView::Mixer);
    ui.key(egui::Key::F7, egui::Modifiers::NONE);
    let mixer = ui.editor_rect(StudioView::Mixer);
    let node =
        ui.nodes
            .iter()
            .find(|node| {
                node.label().is_some_and(|label| {
                    label == "Master · Pan"
                        || (label.starts_with("Mixer ") && label.ends_with(" · Pan"))
                }) && node.bounds().is_some_and(|bounds| {
                    let pos = Pos2::new(
                        ((bounds.x0 + bounds.x1) / 2.0) as f32,
                        ((bounds.y0 + bounds.y1) / 2.0) as f32,
                    );
                    mixer.contains(pos)
                        && ui.ctx.layer_id_at(pos).is_some_and(|layer| {
                            layer.id == workspace::window_id(StudioView::Mixer)
                        })
                })
            })
            .unwrap();
    let bounds = node.bounds().unwrap();
    let pos = Pos2::new(
        ((bounds.x0 + bounds.x1) / 2.0) as f32,
        ((bounds.y0 + bounds.y1) / 2.0) as f32,
    );
    let pans: Vec<_> = ui
        .app
        .project
        .mixer_tracks
        .iter()
        .map(|track| track.pan)
        .collect();
    ui.drag_pointer(pos, Vec2::new(0.0, -24.0));
    assert_eq!(ui.app.workspace.focused, StudioView::Mixer);
    let after: Vec<_> = ui
        .app
        .project
        .mixer_tracks
        .iter()
        .map(|track| track.pan)
        .collect();
    assert_ne!(
        pans, after,
        "first press and drag in an unfocused Mixer must update the actual knob"
    );
}

#[test]
fn floating_workspace_interrupts_real_editor_drags_until_release() {
    for (view, resize, modal) in [
        (StudioView::PianoRoll, false, false),
        (StudioView::PianoRoll, true, false),
        (StudioView::PianoRoll, true, true),
        (StudioView::Playlist, false, false),
        (StudioView::Playlist, false, true),
    ] {
        let mut ui = UiHarness::floating();
        ui.app.focus_editor(view);
        ui.settle();
        let ids: Vec<Id> = if view == StudioView::PianoRoll {
            ui.app
                .project
                .active_pattern()
                .notes
                .iter()
                .map(|note| {
                    let id = Id::new(("piano-note", note.id));
                    if resize { id.with("resize") } else { id }
                })
                .collect()
        } else {
            ui.app
                .project
                .clips
                .iter()
                .map(|clip| Id::new(("playlist-clip", clip.id)))
                .collect()
        };
        let response = ids
            .into_iter()
            .filter_map(|id| ui.ctx.read_response(id))
            .find(|response| {
                response.rect.width() > 3.0
                    && ui
                        .ctx
                        .layer_id_at(response.rect.center())
                        .is_some_and(|layer| layer.id == workspace::window_id(view))
            })
            .expect("a visible real clip/note gesture target");
        let origin = response.rect.center();
        ui.run(mixer_pointer_button(origin, true));
        ui.run(vec![egui::Event::PointerMoved(
            origin + Vec2::new(38.0, 0.0),
        )]);
        ui.run(vec![egui::Event::PointerMoved(
            origin + Vec2::new(50.0, 0.0),
        )]);
        assert!(
            ui.app.playlist_gesture_before.is_some() || ui.app.piano_roll_gesture_before.is_some(),
            "real {view:?} drag must begin before interruption (resize={resize})"
        );
        if modal {
            ui.key(egui::Key::F10, egui::Modifiers::NONE);
            assert!(ui.app.show_settings);
            ui.key(egui::Key::F10, egui::Modifiers::NONE);
            assert!(!ui.app.show_settings);
        } else {
            ui.key(egui::Key::F9, egui::Modifiers::NONE);
            assert_eq!(ui.app.workspace.focused, StudioView::Mixer);
        }
        let interrupted = project_fingerprint(&ui.app.project);
        ui.run(vec![egui::Event::PointerMoved(
            origin + Vec2::new(130.0, 0.0),
        )]);
        ui.run(vec![egui::Event::PointerMoved(
            origin + Vec2::new(150.0, 0.0),
        )]);
        assert_eq!(
            project_fingerprint(&ui.app.project),
            interrupted,
            "interrupted {view:?} drag must not resume (resize={resize}, modal={modal})"
        );
        assert!(ui.app.playlist_gesture_before.is_none());
        assert!(ui.app.piano_roll_gesture_before.is_none());
        ui.run(mixer_pointer_button(origin + Vec2::new(150.0, 0.0), false));
        ui.settle();
        assert_eq!(project_fingerprint(&ui.app.project), interrupted);
        assert!(!ui.app.project_snapshot_transition_pending());
    }
}

#[test]
fn floating_workspace_hide_separates_rack_and_mixer_undo_transactions() {
    let mut ui = UiHarness::floating();
    let original = project_fingerprint(&ui.app.project);
    ui.key(egui::Key::F6, egui::Modifiers::NONE);
    let channel = ui.app.project.channels[0].name.clone();
    ui.click(&format!("{} step 1", channel));
    let rack_edit = project_fingerprint(&ui.app.project);
    assert_ne!(rack_edit, original);
    ui.click("Hide editor");
    ui.key(egui::Key::F9, egui::Modifiers::NONE);
    let mixer = ui.editor_rect(StudioView::Mixer);
    let node = ui
        .nodes
        .iter()
        .find(|node| {
            node.label().is_some_and(|label| {
                label == "Master · Pan"
                    || (label.starts_with("Mixer ") && label.ends_with(" · Pan"))
            }) && node.bounds().is_some_and(|bounds| {
                mixer.contains(Pos2::new(
                    ((bounds.x0 + bounds.x1) / 2.0) as f32,
                    ((bounds.y0 + bounds.y1) / 2.0) as f32,
                ))
            })
        })
        .unwrap();
    let bounds = node.bounds().unwrap();
    ui.drag_pointer(
        Pos2::new(
            ((bounds.x0 + bounds.x1) / 2.0) as f32,
            ((bounds.y0 + bounds.y1) / 2.0) as f32,
        ),
        Vec2::new(0.0, -24.0),
    );
    assert_ne!(project_fingerprint(&ui.app.project), rack_edit);
    let command = egui::Modifiers {
        ctrl: true,
        command: true,
        ..Default::default()
    };
    ui.key(egui::Key::Z, command);
    assert_eq!(
        project_fingerprint(&ui.app.project),
        rack_edit,
        "first Undo must preserve the earlier Rack edit"
    );
    ui.key(egui::Key::Z, command);
    assert_eq!(project_fingerprint(&ui.app.project), original);
}

fn piano_clipboard_command() -> egui::Modifiers {
    egui::Modifiers {
        ctrl: true,
        command: true,
        ..Default::default()
    }
}

fn clipboard_key(key: egui::Key, pressed: bool, repeat: bool) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat,
        modifiers: piano_clipboard_command(),
    }
}

fn copied_note_text(output: &egui::FullOutput) -> String {
    let commands: Vec<_> = output
        .platform_output
        .commands
        .iter()
        .filter_map(|command| match command {
            egui::OutputCommand::CopyText(text) => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        commands.len(),
        1,
        "an explicit note copy/cut must emit exactly one clipboard write"
    );
    assert!(commands[0].starts_with("CITRUS-NOTES/1\n"));
    commands[0].clone()
}

fn piano_clipboard_fixture(ui: &mut UiHarness) -> u32 {
    ui.key(egui::Key::F7, egui::Modifiers::NONE);
    let channel = ui.app.project.channels[ui.app.selected_channel].id;
    let ghost = ui
        .app
        .project
        .channels
        .iter()
        .find(|c| c.id != channel)
        .unwrap()
        .id;
    ui.app.project.active_pattern_mut().notes = vec![
        PianoNote {
            id: 90_001,
            channel_id: Some(channel),
            group_id: Some(123),
            note: 60,
            start: 0.375,
            length: 0.75,
            velocity: 0.625,
            muted: false,
            selected: false,
        },
        PianoNote {
            id: 90_002,
            channel_id: Some(channel),
            group_id: Some(123),
            note: 64,
            start: 0.75,
            length: 0.25,
            velocity: 0.25,
            muted: true,
            selected: false,
        },
        PianoNote {
            id: 90_003,
            channel_id: Some(ghost),
            group_id: None,
            note: 67,
            start: 0.0,
            length: 0.25,
            velocity: 0.5,
            muted: false,
            selected: false,
        },
    ];
    ui.app.piano_roll_state.selection_ids.clear();
    ui.app.transport_mode = TransportMode::Pattern;
    ui.app.beat_position = 1.49;
    ui.app.piano_roll_state.local_snap = PianoSnap::QuarterBeat;
    ui.app.sync_history_observer();
    ui.app.undo_stack.clear();
    ui.app.redo_stack.clear();
    ui.app.project_fingerprint = project_fingerprint(&ui.app.project);
    ui.app.dirty = false;
    ui.settle();
    ui.app.piano_viewport.y.reveal(63.0, 68.0, 0.0).unwrap();
    ui.settle();
    channel
}

#[test]
fn piano_clipboard_real_semantic_keys_preserve_notes_and_one_step_history() {
    let mut ui = UiHarness::floating();
    let channel = piano_clipboard_fixture(&mut ui);
    let before = project_fingerprint(&ui.app.project);
    ui.key(egui::Key::A, piano_clipboard_command());
    assert_eq!(
        ui.app.piano_roll_state.selection_ids,
        HashSet::from([90_001, 90_002])
    );
    let output = ui.run(vec![
        clipboard_key(egui::Key::C, true, false),
        egui::Event::Copy,
    ]);
    let text = copied_note_text(&output);
    ui.run(vec![clipboard_key(egui::Key::C, false, false)]);
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert!(ui.app.undo_stack.is_empty());
    assert!(!ui.app.dirty);
    let output = ui.run(vec![
        clipboard_key(egui::Key::V, true, false),
        egui::Event::Paste(text.clone()),
    ]);
    assert!(output.platform_output.commands.is_empty());
    ui.run(vec![clipboard_key(egui::Key::V, false, false)]);
    assert_eq!(ui.app.undo_stack.len(), 1);
    let pasted = ui.app.project.active_pattern().notes[3..].to_vec();
    assert_eq!(pasted.len(), 2, "semantic + raw paste must dispatch once");
    assert_eq!((pasted[0].start, pasted[1].start), (0.0, 0.375));
    assert_eq!((pasted[0].length, pasted[1].length), (0.75, 0.25));
    assert_eq!((pasted[0].velocity, pasted[1].velocity), (0.625, 0.25));
    assert_eq!((pasted[0].note, pasted[1].note), (60, 64));
    assert!(!pasted[0].muted && pasted[1].muted);
    assert!(pasted.iter().all(|n| n.channel_id == Some(channel)));
    assert_eq!(pasted[0].group_id, pasted[1].group_id);
    assert_ne!(pasted[0].group_id, Some(123));
    assert_eq!(
        ui.app.piano_roll_state.selection_ids,
        pasted.iter().map(|n| n.id).collect()
    );
    let after = project_fingerprint(&ui.app.project);
    ui.capture("piano-clipboard-pattern-paste");
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), before);
    ui.key(egui::Key::Y, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), after);
    ui.run(vec![egui::Event::Paste(text)]);
    assert_eq!(ui.app.project.active_pattern().notes.len(), 7);
    assert_eq!(ui.app.undo_stack.len(), 2);
    let again = &ui.app.project.active_pattern().notes[5..];
    assert_eq!(again[0].start, pasted[0].start);
    assert_ne!(again[0].id, pasted[0].id);
    assert_ne!(again[0].group_id, pasted[0].group_id);
    ui.run(vec![clipboard_key(egui::Key::V, true, false)]);
    let repeated = project_fingerprint(&ui.app.project);
    ui.run(vec![clipboard_key(egui::Key::V, true, true)]);
    ui.run(vec![clipboard_key(egui::Key::V, false, false)]);
    assert_eq!(
        project_fingerprint(&ui.app.project),
        repeated,
        "raw key autorepeat is ignored"
    );
}

#[test]
fn piano_clipboard_buttons_cut_empty_invalid_text_and_local_paste_are_deliberate() {
    let mut ui = UiHarness::floating();
    piano_clipboard_fixture(&mut ui);
    let before = project_fingerprint(&ui.app.project);
    ui.run(vec![egui::Event::Paste("unrelated external text".into())]);
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert!(ui.app.undo_stack.is_empty());
    ui.click("Select all notes");
    assert_eq!(ui.app.piano_roll_state.selection_ids.len(), 2);
    let text = copied_note_text(&ui.run(vec![
        clipboard_key(egui::Key::X, true, false),
        egui::Event::Cut,
    ]));
    ui.run(vec![clipboard_key(egui::Key::X, false, false)]);
    assert_eq!(ui.app.project.active_pattern().notes.len(), 1);
    assert!(ui.app.piano_roll_state.selection_ids.is_empty());
    assert_eq!(ui.app.undo_stack.len(), 1);
    let cut = project_fingerprint(&ui.app.project);
    for event in [
        egui::Event::Cut,
        egui::Event::Copy,
        egui::Event::Paste("plain text".into()),
        egui::Event::Paste("CITRUS-NOTES/1\n{}".into()),
    ] {
        let output = ui.run(vec![event]);
        assert!(output.platform_output.commands.is_empty());
        assert_eq!(project_fingerprint(&ui.app.project), cut);
        assert_eq!(ui.app.undo_stack.len(), 1);
    }
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), before);
    ui.key(egui::Key::Y, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), cut);
    ui.click("Paste notes");
    assert_eq!(
        ui.app.project.active_pattern().notes.len(),
        3,
        "local Paste works even after unrelated OS paste"
    );
    assert_eq!(ui.app.undo_stack.len(), 2);
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), cut);
    ui.run(vec![egui::Event::Paste(text)]);
    assert_eq!(ui.app.project.active_pattern().notes.len(), 3);
}

#[test]
fn piano_clipboard_cross_pattern_retains_channels_and_visible_zero_bar_anchor() {
    let mut ui = UiHarness::floating();
    let channel = piano_clipboard_fixture(&mut ui);
    ui.key(egui::Key::A, piano_clipboard_command());
    let text = copied_note_text(&ui.run(vec![egui::Event::Copy]));
    let source_notes = serde_json::to_string(&ui.app.project.active_pattern().notes).unwrap();
    let target = ui.app.project.patterns.len();
    ui.app.project.patterns.push(Pattern {
        id: 987,
        name: "Clipboard destination".into(),
        length_steps: 16,
        channel_steps: vec![[false; 16]; ui.app.project.channels.len()],
        notes: Vec::new(),
    });
    ui.app.project.active_pattern = target;
    ui.app.selected_channel = ui
        .app
        .project
        .channels
        .iter()
        .position(|c| c.id != channel)
        .unwrap();
    ui.app.piano_roll_state.selection_ids.clear();
    ui.app.transport_mode = TransportMode::Song;
    ui.app.beat_position = 3072.875;
    ui.app.sync_history_observer();
    ui.app.undo_stack.clear();
    ui.settle();
    let before = project_fingerprint(&ui.app.project);
    ui.run(vec![egui::Event::Paste(text)]);
    let notes = &ui.app.project.active_pattern().notes;
    assert_eq!(notes.len(), 2);
    assert_eq!((notes[0].start, notes[1].start), (0.0, 0.375));
    assert!(
        notes.iter().all(|note| note.channel_id == Some(channel)),
        "TARGET must never silently remap channels"
    );
    assert_eq!(
        serde_json::to_string(&ui.app.project.patterns[0].notes).unwrap(),
        source_notes
    );
    assert_eq!(ui.app.undo_stack.len(), 1);
    ui.capture("piano-clipboard-song-pattern-start");
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), before);
    ui.key(egui::Key::Y, piano_clipboard_command());
    assert_eq!(ui.app.project.active_pattern().notes.len(), 2);
}

#[test]
fn piano_clipboard_focus_text_modal_and_hidden_editors_do_not_leak_actions() {
    let mut ui = UiHarness::floating();
    piano_clipboard_fixture(&mut ui);
    ui.key(egui::Key::A, piano_clipboard_command());
    let text = copied_note_text(&ui.run(vec![egui::Event::Copy]));
    let before = project_fingerprint(&ui.app.project);
    let selected = ui.app.piano_roll_state.selection_ids.clone();
    for key in [egui::Key::F5, egui::Key::F6, egui::Key::F9] {
        ui.key(key, egui::Modifiers::NONE);
        for event in [
            egui::Event::Copy,
            egui::Event::Cut,
            egui::Event::Paste(text.clone()),
        ] {
            assert!(ui.run(vec![event]).platform_output.commands.is_empty());
            assert_eq!(project_fingerprint(&ui.app.project), before);
            assert_eq!(ui.app.piano_roll_state.selection_ids, selected);
        }
    }
    ui.key(egui::Key::F7, egui::Modifiers::NONE);
    ui.key(egui::Key::F10, egui::Modifiers::NONE);
    for event in [
        egui::Event::Copy,
        egui::Event::Cut,
        egui::Event::Paste(text.clone()),
    ] {
        assert!(ui.run(vec![event]).platform_output.commands.is_empty());
    }
    ui.key(egui::Key::Escape, egui::Modifiers::NONE);
    assert_eq!(project_fingerprint(&ui.app.project), before);
    let field = ui
        .nodes
        .iter()
        .find(|node| node.role() == Role::TextInput && node.bounds().is_some_and(|b| b.x0 < 220.0))
        .unwrap()
        .bounds()
        .unwrap();
    ui.click_pos(Pos2::new(
        ((field.x0 + field.x1) / 2.0) as f32,
        ((field.y0 + field.y1) / 2.0) as f32,
    ));
    ui.run(vec![egui::Event::Text("browser text".into())]);
    ui.key(egui::Key::A, piano_clipboard_command());
    let output = ui.run(vec![egui::Event::Copy]);
    assert!(
        output
            .platform_output
            .commands
            .iter()
            .any(|c| matches!(c, egui::OutputCommand::CopyText(t) if t == "browser text"))
    );
    ui.run(vec![egui::Event::Cut]);
    assert!(ui.app.browser_search.is_empty());
    ui.run(vec![egui::Event::Paste("another filter".into())]);
    assert_eq!(ui.app.browser_search, "another filter");
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert_eq!(ui.app.piano_roll_state.selection_ids, selected);
    ui.click_pos(ui.editor_rect(StudioView::PianoRoll).left_top() + Vec2::new(100.0, 13.0));
    for _ in 0..4 {
        ui.click("Hide editor");
    }
    ui.run(vec![egui::Event::Paste(text)]);
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert!(ui.app.undo_stack.is_empty());
}

#[test]
fn piano_clipboard_reset_invalid_channel_and_snapshot_barriers_preserve_history() {
    let mut ui = UiHarness::floating();
    let channel = piano_clipboard_fixture(&mut ui);
    ui.key(egui::Key::A, piano_clipboard_command());
    let text = copied_note_text(&ui.run(vec![egui::Event::Copy]));
    let before = project_fingerprint(&ui.app.project);
    // Exercise each action-time guard directly so no pending file or replacement operation
    // can execute during the guard test. The clipboard itself came from actual UI input.
    for barrier in 0..4 {
        match barrier {
            0 => {
                ui.app.queued_save_request = Some(ProjectSaveRequest::Manual {
                    path: PathBuf::from("unused-clipboard-test.citrus"),
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
            _ => ui.app.playlist_gesture_before = Some(ui.app.project.clone()),
        }
        for action in [
            ShortcutAction::SelectAllNotes,
            ShortcutAction::CopyNotes,
            ShortcutAction::CutNotes,
            ShortcutAction::PasteNotes,
        ] {
            ui.app.apply_shortcut_action(&ui.ctx, action);
            assert_eq!(project_fingerprint(&ui.app.project), before);
            assert!(ui.app.undo_stack.is_empty());
        }
        ui.app.queued_save_request = None;
        ui.app.deferred_generator_candidate_after_midi = None;
        ui.app.piano_roll_gesture_before = None;
        ui.app.playlist_gesture_before = None;
    }
    ui.app.project.channels.retain(|c| c.id != channel);
    ui.app.sync_history_observer();
    let missing_channel = project_fingerprint(&ui.app.project);
    ui.run(vec![egui::Event::Paste(text.clone())]);
    assert_eq!(project_fingerprint(&ui.app.project), missing_channel);
    assert!(ui.app.undo_stack.is_empty());
    ui.app.install_project(Project::default(), None, false);
    ui.key(egui::Key::F7, egui::Modifiers::NONE);
    let new_project = project_fingerprint(&ui.app.project);
    ui.run(vec![egui::Event::Paste(text)]);
    ui.key(egui::Key::V, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), new_project);
    assert!(ui.app.undo_stack.is_empty());
    assert!(ui.button("Paste notes").is_disabled());
}

#[test]
fn piano_clipboard_real_note_drag_and_interrupted_pointer_block_clipboard() {
    let mut ui = UiHarness::floating();
    piano_clipboard_fixture(&mut ui);
    ui.key(egui::Key::A, piano_clipboard_command());
    let text = copied_note_text(&ui.run(vec![egui::Event::Copy]));
    let response = ui
        .ctx
        .read_response(Id::new(("piano-note", 90_001_u64)))
        .unwrap();
    let origin = response.rect.center();
    assert_eq!(
        ui.ctx.layer_id_at(origin).unwrap().id,
        workspace::window_id(StudioView::PianoRoll)
    );
    ui.run(mixer_pointer_button(origin, true));
    ui.run(vec![egui::Event::PointerMoved(
        origin + Vec2::new(40.0, 0.0),
    )]);
    assert!(ui.app.piano_roll_gesture_before.is_some());
    let dragged = project_fingerprint(&ui.app.project);
    for event in [
        egui::Event::Copy,
        egui::Event::Cut,
        egui::Event::Paste(text.clone()),
    ] {
        assert!(ui.run(vec![event]).platform_output.commands.is_empty());
        assert_eq!(project_fingerprint(&ui.app.project), dragged);
    }
    ui.key(egui::Key::F10, egui::Modifiers::NONE);
    ui.key(egui::Key::F10, egui::Modifiers::NONE);
    assert!(ui.app.piano_roll_gesture_before.is_none());
    ui.run(vec![egui::Event::Paste(text.clone())]);
    assert_eq!(
        project_fingerprint(&ui.app.project),
        dragged,
        "dismissal must not resume a held pointer edit"
    );
    ui.run(mixer_pointer_button(origin + Vec2::new(40.0, 0.0), false));
    ui.settle();
    let history = ui.app.undo_stack.len();
    ui.run(vec![egui::Event::Paste(text)]);
    assert_eq!(ui.app.project.active_pattern().notes.len(), 5);
    assert_eq!(ui.app.undo_stack.len(), history + 1);
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(
        project_fingerprint(&ui.app.project),
        dragged,
        "paste undo must preserve preceding drag"
    );
}

#[test]
fn piano_clipboard_numeric_text_focus_keeps_normal_text_clipboard_ownership() {
    let mut ui = UiHarness::floating();
    piano_clipboard_fixture(&mut ui);
    ui.key(egui::Key::A, piano_clipboard_command());
    let text = copied_note_text(&ui.run(vec![egui::Event::Copy]));
    let notes = serde_json::to_string(&ui.app.project.active_pattern().notes).unwrap();
    let selected = ui.app.piano_roll_state.selection_ids.clone();
    let bounds = ui
        .nodes
        .iter()
        .find(|n| n.role() == Role::SpinButton && n.bounds().is_some_and(|b| b.y1 < 100.0))
        .expect("toolbar Tempo is a real numeric editor")
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
    assert!(
        ui.ctx.text_edit_focused(),
        "double-click Tempo must enter numeric text editing"
    );
    ui.key(egui::Key::A, piano_clipboard_command());
    let output = ui.run(vec![egui::Event::Copy]);
    assert!(
        output.platform_output.commands.iter().any(
            |c| matches!(c, egui::OutputCommand::CopyText(t) if !t.starts_with("CITRUS-NOTES/"))
        )
    );
    ui.run(vec![egui::Event::Cut]);
    ui.run(vec![egui::Event::Paste(text)]);
    assert_eq!(
        serde_json::to_string(&ui.app.project.active_pattern().notes).unwrap(),
        notes
    );
    assert_eq!(ui.app.piano_roll_state.selection_ids, selected);
}

#[test]
fn piano_clipboard_copy_cut_buttons_emit_one_payload_and_cut_is_one_undo() {
    let mut ui = UiHarness::floating();
    piano_clipboard_fixture(&mut ui);
    ui.click("Select all notes");
    let before = project_fingerprint(&ui.app.project);
    let mut payloads = Vec::new();
    for label in ["Copy notes", "Cut notes"] {
        ui.settle();
        let bounds = ui.button(label).bounds().unwrap();
        let pos = Pos2::new(
            ((bounds.x0 + bounds.x1) / 2.0) as f32,
            ((bounds.y0 + bounds.y1) / 2.0) as f32,
        );
        let press = ui.run(mixer_pointer_button(pos, true));
        assert!(press.platform_output.commands.is_empty());
        payloads.push(copied_note_text(&ui.run(mixer_pointer_button(pos, false))));
        for _ in 0..3 {
            assert!(ui.run(Vec::new()).platform_output.commands.is_empty());
        }
    }
    assert_eq!(payloads[0], payloads[1]);
    assert_eq!(ui.app.project.active_pattern().notes.len(), 1);
    assert_eq!(ui.app.undo_stack.len(), 1);
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), before);
}

#[test]
fn piano_clipboard_minimum_floating_overflow_buttons_remain_reachable() {
    let mut ui = UiHarness::floating();
    piano_clipboard_fixture(&mut ui);
    ui.key(egui::Key::A, piano_clipboard_command());
    copied_note_text(&ui.run(vec![egui::Event::Copy]));
    ui.size = Vec2::new(1080.0, 680.0);
    ui.settle();
    let in_overflow = ui
        .nodes
        .iter()
        .any(|node| node.label() == Some("NOTE EDIT v"));
    let bounds = if in_overflow {
        ui.click("NOTE EDIT v");
        ui.ctx.content_rect()
    } else {
        ui.editor_rect(StudioView::PianoRoll)
    };
    for label in ["Select all notes", "Copy notes", "Cut notes", "Paste notes"] {
        let b = ui.button(label).bounds().unwrap();
        let rect = Rect::from_min_max(
            Pos2::new(b.x0 as f32, b.y0 as f32),
            Pos2::new(b.x1 as f32, b.y1 as f32),
        );
        assert!(
            bounds.contains_rect(rect),
            "{label} must stay inside the small Piano window or visible overflow"
        );
    }
    ui.click("Paste notes");
    assert_eq!(ui.app.project.active_pattern().notes.len(), 5);
    assert_eq!(ui.app.undo_stack.len(), 1);
    ui.capture("piano-clipboard-minimum-floating");
}

#[test]
fn piano_clipboard_partial_group_cut_keeps_preceding_edit_separate_in_history() {
    let mut ui = UiHarness::floating();
    piano_clipboard_fixture(&mut ui);
    ui.app.piano_roll_state.grouping_enabled = false;
    ui.app.piano_roll_state.selection_ids = HashSet::from([90_001]);
    let before = project_fingerprint(&ui.app.project);
    // A preceding Rack edit has not yet reached the timed observer.
    ui.app.project.active_pattern_mut().channel_steps[0][0] ^= true;
    let preceding = project_fingerprint(&ui.app.project);
    let text = copied_note_text(&ui.run(vec![egui::Event::Cut]));
    let cut = project_fingerprint(&ui.app.project);
    assert_eq!(ui.app.project.active_pattern().notes.len(), 2);
    assert!(
        ui.app
            .project
            .active_pattern()
            .notes
            .iter()
            .all(|note| note.group_id.is_none())
    );
    assert_eq!(ui.app.undo_stack.len(), 2);
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), preceding);
    assert_eq!(ui.app.project.active_pattern().notes[0].group_id, Some(123));
    assert_eq!(ui.app.project.active_pattern().notes[1].group_id, Some(123));
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), before);
    ui.key(egui::Key::Y, piano_clipboard_command());
    ui.key(egui::Key::Y, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), cut);
    ui.run(vec![egui::Event::Paste(text)]);
    let pasted = ui.app.project.active_pattern().notes.last().unwrap();
    assert!(
        pasted.group_id.is_none(),
        "a one-member copied group must not join the source group"
    );
}

impl UiHarness {
    fn arrange_test_windows(&mut self, windows: &[(StudioView, Rect)], focus: StudioView) {
        for view in workspace::EDITORS {
            let window = &mut self.app.workspace.windows[workspace::index(view)];
            window.visible = false;
        }
        for (view, rect) in windows {
            let window = &mut self.app.workspace.windows[workspace::index(*view)];
            window.visible = true;
            window.rect = Some(*rect);
        }
        self.app.workspace.reset_for_test();
        self.app.focus_editor(focus);
        for _ in 0..8 {
            self.run(Vec::new());
        }
    }
}

#[test]
fn compact_workspace_native_motion_is_continuous_and_snaps_only_on_release() {
    let mut ui = UiHarness::floating();
    let project = project_fingerprint(&ui.app.project);
    let view = StudioView::Playlist;
    let input = Rect::from_min_size(Pos2::new(120.0, 80.0), Vec2::new(600.0, 360.0));
    ui.arrange_test_windows(&[(view, input)], view);
    let before = ui.editor_rect(view);
    let origin = before.left_top() + Vec2::new(110.0, 13.0);
    let pixel = 1.0 / ui.ctx.pixels_per_point();
    let warm = 12.0 * pixel;
    let increment = 4.0 * pixel;
    ui.run(mixer_pointer_button(origin, true));
    // Cross egui's six-point click/drag slop once, then measure every held update.
    ui.run(vec![egui::Event::PointerMoved(
        origin - Vec2::new(warm, 0.0),
    )]);
    for step in 1..=27 {
        ui.run(vec![egui::Event::PointerMoved(
            origin - Vec2::new(warm + step as f32 * increment, 0.0),
        )]);
        let actual = ui.editor_rect(view);
        let expected = before.translate(Vec2::new(-warm - (step as f32) * increment, 0.0));
        assert!(
            (actual.min - expected.min).length() < 1.5,
            "held native title movement: step {step}, {actual:?} vs {expected:?}"
        );
        assert!((actual.size() - before.size()).length() < 1.0);
    }
    let held = ui.editor_rect(view);
    let bounds = ui.app.workspace.bounds.unwrap();
    assert!(
        (2.0..8.0).contains(&(held.left() - bounds.left())),
        "must remain unsnapped while held: {held:?}"
    );
    ui.run(mixer_pointer_button(
        origin - Vec2::new(warm + 27.0 * increment, 0.0),
        false,
    ));
    ui.settle();
    let snapped = ui.editor_rect(view);
    assert!((snapped.left() - bounds.left()).abs() < 1.0);
    assert!((snapped.size() - before.size()).length() < 1.0);
    let away = snapped.left_top() + Vec2::new(110.0, 13.0);
    ui.run(mixer_pointer_button(away, true));
    ui.run(vec![egui::Event::PointerMoved(away + Vec2::new(32.0, 0.0))]);
    ui.run(mixer_pointer_button(away + Vec2::new(32.0, 0.0), false));
    ui.settle();
    assert!(
        (ui.editor_rect(view).left() - bounds.left() - 32.0).abs() < 1.5,
        "a snapped window must release immediately on the next drag"
    );

    // The modifier lives in RawInput as well as the pointer event, as in real egui input.
    let before = ui.editor_rect(view);
    let origin = before.left_top() + Vec2::new(110.0, 13.0);
    let delta = Vec2::new(bounds.left() + 6.0 - before.left(), 0.0);
    ui.run(mixer_pointer_button(origin, true));
    ui.run(vec![egui::Event::PointerMoved(origin + delta)]);
    let mut alt = egui::Modifiers::NONE;
    alt.alt = true;
    ui.run_with_modifiers(mixer_pointer_button(origin + delta, false), alt);
    ui.settle();
    assert!(
        (ui.editor_rect(view).left() - bounds.left() - 6.0).abs() < 1.5,
        "Alt release must bypass magnetic alignment"
    );
    assert_eq!(project_fingerprint(&ui.app.project), project);
    assert!(ui.app.undo_stack.is_empty());
}

#[test]
fn compact_workspace_all_resize_edges_remain_live_and_preserve_opposite_edges() {
    for (left, right, top, bottom) in [
        (true, false, false, false),
        (false, true, false, false),
        (false, false, true, false),
        (false, false, false, true),
        (true, false, true, false),
        (true, false, false, true),
        (false, true, true, false),
        (false, true, false, true),
    ] {
        let mut ui = UiHarness::floating();
        let view = StudioView::Playlist;
        let project = project_fingerprint(&ui.app.project);
        ui.arrange_test_windows(
            &[(
                view,
                Rect::from_min_size(Pos2::new(180.0, 100.0), Vec2::new(720.0, 440.0)),
            )],
            view,
        );
        let before = ui.editor_rect(view);
        let origin = Pos2::new(
            if left {
                before.left() + 1.0
            } else if right {
                before.right() - 1.0
            } else {
                before.center().x
            },
            if top {
                before.top() + 1.0
            } else if bottom {
                before.bottom() - 1.0
            } else {
                before.center().y
            },
        );
        let delta = Vec2::new(
            if left {
                40.0
            } else if right {
                -40.0
            } else {
                0.0
            },
            if top {
                30.0
            } else if bottom {
                -30.0
            } else {
                0.0
            },
        );
        ui.run(mixer_pointer_button(origin, true));
        let mut changes = 0;
        let mut previous = before;
        for step in 1..=20 {
            ui.run(vec![egui::Event::PointerMoved(
                origin + delta * (step as f32 / 20.0),
            )]);
            let actual = ui.editor_rect(view);
            changes += usize::from((actual.size() - previous.size()).length() > 0.5);
            assert!(
                (actual.size() - previous.size()).length() < 8.0,
                "unexpected held resize jump: {previous:?} -> {actual:?}"
            );
            if left {
                // Native egui queues the new width for the next input pass while
                // updating the left/top position now. Allow exactly that one-step lag.
                assert!((actual.right() - before.right()).abs() < delta.x.abs() / 20.0 + 1.0);
            }
            if right {
                assert!((actual.left() - before.left()).abs() < 1.5);
            }
            if top {
                assert!((actual.bottom() - before.bottom()).abs() < delta.y.abs() / 20.0 + 1.0);
            }
            if bottom {
                assert!((actual.top() - before.top()).abs() < 1.5);
            }
            previous = actual;
        }
        assert!(
            changes >= 15,
            "native resize must update continuously while held: {changes}"
        );
        ui.run(mixer_pointer_button(origin + delta, false));
        ui.settle();
        let actual = ui.editor_rect(view);
        assert!((actual.width() - (before.width() - delta.x.abs())).abs() < 2.0);
        assert!((actual.height() - (before.height() - delta.y.abs())).abs() < 2.0);
        assert_eq!(project_fingerprint(&ui.app.project), project);
        assert!(ui.app.undo_stack.is_empty());
    }
}

#[test]
fn compact_workspace_peer_resize_snap_and_interruption_are_bounded() {
    let mut ui = UiHarness::floating();
    let view = StudioView::Playlist;
    ui.arrange_test_windows(
        &[
            (
                view,
                Rect::from_min_size(Pos2::new(70.0, 80.0), Vec2::new(600.0, 360.0)),
            ),
            (
                StudioView::Mixer,
                Rect::from_min_size(Pos2::new(800.0, 80.0), Vec2::new(600.0, 420.0)),
            ),
        ],
        view,
    );
    let before = ui.editor_rect(view);
    let peer = ui.editor_rect(StudioView::Mixer);
    let origin = Pos2::new(before.right() - 1.0, before.center().y);
    let delta = Vec2::new(peer.left() - 6.0 - before.right(), 0.0);
    ui.drag_pointer(origin, delta);
    let snapped = ui.editor_rect(view);
    assert!(
        (snapped.right() - peer.left()).abs() < 1.5,
        "resize edge must align to visible peer: {snapped:?} / {peer:?}"
    );
    assert!((snapped.left() - before.left()).abs() < 1.0);
    ui.drag_pointer(
        Pos2::new(snapped.right() - 1.0, snapped.center().y),
        Vec2::new(-30.0, 0.0),
    );
    assert!((ui.editor_rect(view).right() - (peer.left() - 30.0)).abs() < 2.0);

    let rect = ui.editor_rect(view);
    let title = rect.left_top() + Vec2::new(100.0, 13.0);
    ui.run(mixer_pointer_button(title, true));
    ui.run(vec![egui::Event::PointerMoved(
        title + Vec2::new(20.0, 20.0),
    )]);
    ui.app.show_inspector = true;
    ui.run(vec![egui::Event::PointerMoved(
        title + Vec2::new(40.0, 40.0),
    )]);
    let canceled = ui.editor_rect(view);
    ui.run(vec![egui::Event::PointerMoved(
        title + Vec2::new(80.0, 80.0),
    )]);
    assert!(
        (ui.editor_rect(view).min - canceled.min).length() < 1.0,
        "bounds change must cancel stale held drag"
    );
    ui.run(mixer_pointer_button(title + Vec2::new(80.0, 80.0), false));
    ui.settle();
    assert!(
        (ui.editor_rect(view).min - canceled.min).length() < 1.0,
        "interrupted release must not apply a late snap"
    );
    for view in [view, StudioView::Mixer] {
        assert!(
            ui.app
                .workspace
                .bounds
                .unwrap()
                .expand(1.0)
                .contains_rect(ui.editor_rect(view))
        );
    }
}

#[test]
fn compact_workspace_default_and_repeated_arrange_use_the_available_desktop() {
    let mut ui = UiHarness::floating();
    let project = project_fingerprint(&ui.app.project);
    let original: Vec<_> = workspace::EDITORS
        .into_iter()
        .map(|view| ui.editor_rect(view))
        .collect();
    for _ in 0..10 {
        ui.click("Cascade windows");
        ui.click("Arrange windows");
        for (i, view) in workspace::EDITORS.into_iter().enumerate() {
            let actual = ui.editor_rect(view);
            assert!((actual.min - original[i].min).length() < 1.0);
            assert!((actual.size() - original[i].size()).length() < 1.0);
            assert!(
                ui.app
                    .workspace
                    .bounds
                    .unwrap()
                    .expand(1.0)
                    .contains_rect(actual)
            );
            for other in workspace::EDITORS.into_iter().skip(i + 1) {
                assert!(!actual.intersect(ui.editor_rect(other)).is_positive());
            }
        }
    }
    assert_eq!(project_fingerprint(&ui.app.project), project);
    assert!(ui.app.undo_stack.is_empty());
    ui.capture("compact-workspace-arranged");
    ui.size = Vec2::new(1080.0, 680.0);
    ui.settle();
    ui.click("Arrange windows");
    for view in workspace::EDITORS {
        assert!(
            ui.app
                .workspace
                .bounds
                .unwrap()
                .expand(1.0)
                .contains_rect(ui.editor_rect(view))
        );
    }
    ui.capture("compact-workspace-minimum");
}

include!("workspace_motion_benchmark.rs");

#[test]
fn compact_rack_retains_accessible_controls_and_scrolls_at_minimum_width() {
    let mut ui = UiHarness::floating();
    ui.size = Vec2::new(1080.0, 680.0);
    ui.settle();
    ui.arrange_test_windows(
        &[(
            StudioView::ChannelRack,
            Rect::from_min_size(Pos2::new(20.0, 20.0), Vec2::new(400.0, 260.0)),
        )],
        StudioView::ChannelRack,
    );
    let rack = ui.editor_rect(StudioView::ChannelRack);
    let channel = ui.app.project.channels[0].name.clone();
    for label in [
        &channel,
        &format!("Mute {channel}"),
        &format!("Solo {channel}"),
        &format!("{channel} step 1"),
    ] {
        let node = ui.button(label);
        let bounds = node.bounds().unwrap();
        assert!(
            bounds.y1 - bounds.y0 >= 23.5,
            "compact hit height must remain 24 points: {label} {bounds:?}"
        );
        assert!(
            bounds.x1 - bounds.x0 >= 23.5,
            "compact hit width must remain 24 points: {label} {bounds:?}"
        );
    }
    let muted = ui.app.project.channels[0].muted;
    ui.click(&format!("Mute {channel}"));
    assert_eq!(ui.app.project.channels[0].muted, !muted);
    ui.click(&format!("Mute {channel}"));
    let step_before = ui.app.project.active_pattern().channel_steps[0][15];
    ui.run(vec![
        egui::Event::PointerMoved(rack.center()),
        egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            phase: egui::TouchPhase::Move,
            delta: Vec2::new(-600.0, 0.0),
            modifiers: egui::Modifiers::NONE,
        },
    ]);
    for _ in 0..10 {
        ui.run(Vec::new());
    }
    let bounds = ui.button(&format!("{channel} step 16")).bounds().unwrap();
    let center = Pos2::new(
        ((bounds.x0 + bounds.x1) / 2.0) as f32,
        ((bounds.y0 + bounds.y1) / 2.0) as f32,
    );
    assert!(
        rack.contains(center),
        "last step must be reachable by real horizontal scroll: {center:?} / {rack:?}"
    );
    ui.click(&format!("{channel} step 16"));
    assert_eq!(
        ui.app.project.active_pattern().channel_steps[0][15],
        !step_before
    );
    ui.capture("compact-rack-minimum");
}

include!("piano_keyboard_tests.rs");
#[path = "piano_expression_tests.rs"]
mod piano_expression_tests;
#[path = "piano_mouse_tests.rs"]
mod piano_mouse_tests;

#[path = "piano_range_tests.rs"]
mod piano_range_tests;

#[path = "piano_snap_integration_tests.rs"]
mod piano_snap_integration_tests;

#[test]
fn plugin_scan_rescan_notice_and_failure_details_use_production_ui() {
    let mut ui = UiHarness::new();
    ui.app.plugin_cache_needs_rescan = true;
    ui.app.show_plugins = true;
    ui.app.plugins = vec![PluginDescriptor {
        id: "scan-failure-fixture".into(),
        name: "Scan failure fixture".into(),
        vendor: "Unknown vendor".into(),
        path: PathBuf::from("/not-a-real-plugin/scan-failure.vst3"),
        format: ScannedPluginFormat::Vst3,
        category: "Unknown".into(),
        is_instrument: false,
        verified: false,
        vst3_metadata: None,
        scan_error: Some("required helper is missing; reinstall Citrus Studio".into()),
    }];
    ui.settle();
    assert!(ui.nodes.iter().any(|node| {
        node.value().or_else(|| node.label())
            == Some("VST3 index needs a rescan: old filename-based classifications were discarded.")
    }));
    let unknown = ui
        .nodes
        .iter()
        .find(|node| node.value().or_else(|| node.label()) == Some("Unknown"))
        .expect("scan failure classification must be visible");
    let pointer = node_rect(unknown).center();
    ui.run(vec![egui::Event::PointerMoved(pointer)]);
    for _ in 0..15 {
        ui.run(Vec::new());
    }
    assert!(ui.nodes.iter().any(|node| {
        node.value().or_else(|| node.label())
            == Some(
                "VST3 metadata unavailable: required helper is missing; reinstall Citrus Studio",
            )
    }), "pointer={pointer:?}, nodes={:?}", ui.nodes.iter().map(|node| (node.value(), node.label(), node.bounds())).collect::<Vec<_>>());

    // Completing an explicit empty-folder scan clears the stale-cache notice. No fixture
    // module, real plugin, native window, device or user-profile path is opened here.
    ui.app.scan_paths.clear();
    ui.app.start_plugin_scan();
    let deadline = Instant::now() + Duration::from_secs(2);
    while ui.app.scan_receiver.is_some() {
        ui.app.poll_plugin_scan();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(!ui.app.plugin_cache_needs_rescan);
    assert!(ui.app.plugins.is_empty());
}

#[test]
fn plugin_midi_port_controls_change_real_model_and_keep_monitor_separate() {
    let mut ui = UiHarness::new();
    ui.size = egui::vec2(1440.0, 1200.0);
    ui.app.project = Project::blank();
    let plugin: PluginInstance = serde_json::from_value(serde_json::json!({
        "id":501,"format":"vst3","path":"/not-a-real-plugin/midi-ui.vst3",
        "uid":"midi-ui-fixture","name":"MIDI processor fixture",
        "midi_ports":{"output":0}
    }))
    .unwrap();
    ui.app.project.channels[0].instrument_plugin_instance_id = Some(501);
    ui.app.project.plugin_instances.push(plugin);
    ui.app.selected_channel = 0;
    ui.app.focus_editor(StudioView::ChannelRack);
    ui.app.sync_history_observer();
    ui.settle();
    assert!(
        ui.nodes
            .iter()
            .any(|node| node.value().or_else(|| node.label()) == Some("PLUGIN MIDI PORTS"))
    );
    let output_label_y = ui
        .nodes
        .iter()
        .find(|node| node.value().or_else(|| node.label()) == Some("Output"))
        .map(|node| node_rect(node).center().y)
        .unwrap();
    let port = ui
        .nodes
        .iter()
        .filter(|node| node.value().or_else(|| node.label()) == Some("0") && !node.is_disabled())
        .min_by(|left, right| {
            (node_rect(left).center().y - output_label_y)
                .abs()
                .total_cmp(&(node_rect(right).center().y - output_label_y).abs())
        })
        .unwrap_or_else(|| {
            panic!(
                "saved output port zero must be an actual enabled ComboBox: {:?}",
                ui.nodes
                    .iter()
                    .map(|node| (node.role(), node.value(), node.label(), node.is_disabled()))
                    .collect::<Vec<_>>()
            )
        });
    ui.click_pos(node_rect(port).center());
    // The real popup scrolls its 257 choices. Choose a currently visible row,
    // rather than an accessibility node below the popup's clipped viewport.
    ui.click("1");
    assert_eq!(
        ui.app.project.plugin_instances[0].midi_ports.output,
        Some(1),
        "toast={:?}",
        ui.app.toast
    );
    assert_eq!(ui.app.project.plugin_instances[0].midi_ports.input, None);
    let port_position = |ui: &UiHarness, value: &str| {
        ui.nodes
            .iter()
            .find(|node| {
                node.role() == Role::ComboBox && node.value() == Some(value) && !node.is_disabled()
            })
            .map(|node| node_rect(node).center())
            .expect("actual port ComboBox")
    };
    let pointer = port_position(&ui, "1");
    ui.click_pos(pointer);
    // Hover the popup itself, then scroll its bounded viewport to the far end.
    // Wheel must not leak into Piano/Playlist zoom or mutate musical content.
    for _ in 0..8 {
        ui.run(vec![
            egui::Event::PointerMoved(pointer + egui::vec2(0.0, 60.0)),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                phase: egui::TouchPhase::Move,
                delta: egui::vec2(0.0, -2_000.0),
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        ui.settle();
    }
    ui.click("255");
    assert_eq!(
        ui.app.project.plugin_instances[0].midi_ports.output,
        Some(255)
    );
    ui.click_pos(port_position(&ui, "255"));
    for _ in 0..8 {
        ui.run(vec![
            egui::Event::PointerMoved(pointer + egui::vec2(0.0, 60.0)),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                phase: egui::TouchPhase::Move,
                delta: egui::vec2(0.0, 2_000.0),
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        ui.settle();
    }
    ui.click("0");
    assert_eq!(
        ui.app.project.plugin_instances[0].midi_ports.output,
        Some(0)
    );
    let mute = ui
        .nodes
        .iter()
        .find(|node| node.label() == Some("Mute device audio monitor"))
        .expect("independent audio-monitor checkbox");
    ui.click_pos(node_rect(mute).center());
    let ports = ui.app.project.plugin_instances[0].midi_ports;
    assert!(ports.audio_monitor_muted);
    assert_eq!(ports.output, Some(0));
    assert!(!ui.app.project.plugin_instances[0].bypass);
    assert!(!ui.app.project.channels[0].muted);
    assert!(
        ui.app
            .undo_stack
            .last()
            .is_some_and(|before| !before.plugin_instances[0].midi_ports.audio_monitor_muted)
    );
    ui.capture("plugin-midi-ports");
    ui.click_pos(port_position(&ui, "0"));
    ui.click("Off");
    assert_eq!(ui.app.project.plugin_instances[0].midi_ports.output, None);
    assert!(
        ui.app.project.plugin_instances[0]
            .midi_ports
            .audio_monitor_muted
    );
    // A reopened unavailable input remains editable to Off without trusting stale
    // scan capabilities. Exercise the actual Input control independently.
    ui.app.project.plugin_instances[0].midi_ports.input = Some(0);
    ui.app.sync_history_observer();
    ui.settle();
    ui.click_pos(port_position(&ui, "0"));
    ui.click("Off");
    assert_eq!(ui.app.project.plugin_instances[0].midi_ports.input, None);
}

fn plugin_processing_fixture(
    health: crate::plugin_timing::PluginProcessingHealth,
    revision: u64,
) -> crate::plugin_timing::PluginProcessingSnapshot {
    use crate::plugin_timing::{
        PluginProcessingFault, PluginProcessingFaultReason, PreparedPluginTimingPlan,
    };
    let plan = PreparedPluginTimingPlan::new(revision, 48_000, 2_048).unwrap();
    crate::plugin_timing::PluginProcessingSnapshot {
        plan,
        health,
        fault: (health == PluginProcessingHealth::Faulted).then_some(PluginProcessingFault {
            endpoint_id: 17,
            epoch: 6,
            expected_sequence: 29,
            raw_callback_frames: 4_096,
            callback_budget_frames: plan.callback_budget_frames,
            lookahead_quanta: plan.lookahead_quanta,
            timing_revision: revision,
            reason: PluginProcessingFaultReason::CallbackBudgetExceeded,
        }),
        fault_count: 1,
        recovered_count: u64::from(health == PluginProcessingHealth::Recovered),
        deadline_misses: 3,
        input_losses: 2,
        output_losses: 1,
    }
}

#[test]
fn full_app_plugin_profile_controls_require_observations_and_keep_device_preferences() {
    let mut ui = UiHarness::new();
    let before_preferences = ui.app.audio_preferences.clone();
    let before_output = ui.app.audio_output_draft.clone();
    let before_input = ui.app.audio_input_draft.clone();
    let before_project = project_fingerprint(&ui.app.project);
    ui.key(egui::Key::F10, egui::Modifiers::NONE);
    assert!(ui.button("128 frames (Experimental)").is_disabled());
    assert!(ui.button("256 frames (Experimental)").is_disabled());
    assert!(ui.button("512 frames (Experimental)").is_disabled());
    assert!(!ui.button("2048 frames").is_disabled());
    assert_eq!(ui.app.audio_preferences.plugin_callback_budget_frames, 2048);
    assert!(
        ui.nodes
            .iter()
            .filter_map(|node| node.value().or_else(|| node.label()))
            .any(|label| label.contains("worker deadlines can fail even when callbacks fit"))
    );
    assert!(ui.button("RETRY AUDIO PROCESSING").is_disabled());
    assert!(!ui.app.audio_preferences.metronome_enabled);

    let engine = AudioEngine::test_engine();
    engine.observe_test_callback(256);
    ui.app.audio = Some(engine);
    ui.settle();
    assert!(ui.button("128 frames (Experimental)").is_disabled());
    assert!(!ui.button("256 frames (Experimental)").is_disabled());
    assert!(!ui.button("512 frames (Experimental)").is_disabled());
    let generation = ui.app.timeline_desired_generation;
    ui.app.playing = true;
    ui.click("512 frames (Experimental)");
    assert_eq!(ui.app.audio.as_ref().unwrap().plugin_timing_profile(), 512);
    assert_eq!(ui.app.audio_preferences.plugin_callback_budget_frames, 512);
    assert!(ui.app.plugin_processing_retry_pending.is_some());
    assert!(ui.app.timeline_desired_generation > generation);
    assert!(!ui.app.playing);
    assert!(ui.button("512 frames (Experimental)").is_disabled());
    assert!(ui.button("RETRY AUDIO PROCESSING").is_disabled());
    assert!(
        ui.nodes
            .iter()
            .filter_map(|node| node.value().or_else(|| node.label()))
            .any(|label| label.contains("stopped replan pending"))
    );
    assert!(
        ui.nodes
            .iter()
            .filter_map(|node| node.value().or_else(|| node.label()))
            .any(|label| label.contains("Active B=2048")),
        "requested ceiling must not be shown as already active"
    );
    assert_eq!(
        ui.app.audio_preferences.requested_output,
        before_preferences.requested_output
    );
    assert_eq!(
        ui.app.audio_preferences.requested_input,
        before_preferences.requested_input
    );
    assert_eq!(
        ui.app.audio_preferences.last_known_good_output,
        before_preferences.last_known_good_output
    );
    assert_eq!(ui.app.audio_output_draft, before_output);
    assert_eq!(ui.app.audio_input_draft, before_input);
    assert!(!ui.app.audio_preferences.metronome_enabled);
    assert_eq!(project_fingerprint(&ui.app.project), before_project);
    ui.app.toggle_play();
    assert!(
        !ui.app.playing,
        "a pending exact acknowledgment cannot start playback"
    );

    let mut storage = WorkspaceTestStorage::default();
    ui.app.save(&mut storage);
    let restored = UiHarness::with_storage(Some(&storage), true);
    assert_eq!(
        restored.app.audio_preferences.plugin_callback_budget_frames,
        512
    );
    assert_eq!(restored.app.audio_output_draft, before_output);
    assert!(!restored.app.audio_preferences.metronome_enabled);
}

#[test]
fn full_app_plugin_fault_requires_explicit_retry_and_never_automatically_replays() {
    let mut ui = UiHarness::new();
    let engine = AudioEngine::test_engine();
    engine.observe_test_callback(128);
    engine.set_plugin_processing_test_snapshot(plugin_processing_fixture(
        PluginProcessingHealth::Faulted,
        1,
    ));
    ui.app.audio = Some(engine);
    ui.app.playing = true;
    ui.key(egui::Key::F10, egui::Modifiers::NONE);
    assert!(!ui.app.playing);
    assert!(ui.app.plugin_processing_retry_required);
    assert!(
        ui.nodes
            .iter()
            .filter_map(|node| node.value().or_else(|| node.label()))
            .any(|label| label == "Device XRUNs: 0")
    );
    assert!(
        ui.nodes
            .iter()
            .filter_map(|node| node.value().or_else(|| node.label()))
            .any(|label| label.contains("Faulted · explicit retry required"))
    );
    assert!(
        ui.nodes
            .iter()
            .filter_map(|node| node.value().or_else(|| node.label()))
            .any(|label| label.contains("endpoint 17 · epoch 6 · expected sequence 29"))
    );
    assert!(
        ui.nodes
            .iter()
            .filter_map(|node| node.value().or_else(|| node.label()))
            .any(|label| label.contains("Raw callback 4096 frames"))
    );
    assert!(
        ui.nodes
            .iter()
            .filter_map(|node| node.value().or_else(|| node.label()))
            .any(|label| label.contains("cumulative faults 1 / recoveries 0"))
    );
    let seek_serial = ui.app.timeline_audio_sync.desired_seek.serial;
    ui.app.recover_timeline_runtime_if_needed();
    ui.app.publish_transport_seek(4.0);
    ui.app.publish_current_transport_loop();
    ui.app.toggle_play();
    ui.app.toggle_record();
    assert_eq!(ui.app.timeline_audio_sync.desired_seek.serial, seek_serial);
    assert!(!ui.app.playing);
    assert!(!ui.app.recording);
    assert!(ui.app.plugin_processing_retry_required);
    ui.capture("settings-plugin-fault");

    let generation = ui.app.timeline_desired_generation;
    ui.click("RETRY AUDIO PROCESSING");
    assert!(!ui.app.plugin_processing_retry_required);
    assert!(ui.app.plugin_processing_retry_pending.is_some());
    assert!(ui.app.timeline_desired_generation > generation);
    assert!(ui.app.timeline_audio_sync.desired_seek.serial > seek_serial);
    assert!(!ui.app.playing);
    assert!(!ui.app.audio_preferences.metronome_enabled);
    ui.settle();
    assert!(
        !ui.app.plugin_processing_retry_required,
        "the same old fault snapshot must not undo an explicit retry"
    );
    assert!(ui.button("RETRY AUDIO PROCESSING").is_disabled());
    let requested_generation = ui.app.timeline_desired_generation;
    ui.app.request_plugin_processing_replan(None);
    assert_eq!(
        ui.app.timeline_desired_generation, requested_generation,
        "a repeated retry cannot enqueue a second replan"
    );

    // Presentation injection is intentionally not a DSP recovery proof: even a
    // recovered snapshot cannot authorize playback without the exact activation.
    ui.app
        .audio
        .as_ref()
        .unwrap()
        .set_plugin_processing_test_snapshot(plugin_processing_fixture(
            PluginProcessingHealth::Recovered,
            2,
        ));
    ui.settle();
    assert!(ui.app.plugin_processing_retry_pending.is_some());
    ui.app.toggle_play();
    assert!(!ui.app.playing);
    ui.app
        .audio
        .as_ref()
        .unwrap()
        .set_plugin_processing_test_snapshot(plugin_processing_fixture(
            PluginProcessingHealth::Faulted,
            2,
        ));
    ui.settle();
    assert!(
        ui.app.plugin_processing_retry_required,
        "a fault in the requested revision must latch again"
    );
    assert!(ui.app.plugin_processing_retry_pending.is_none());
    assert!(!ui.app.playing);
}

#[test]
fn plugin_processing_retry_acknowledgment_requires_timing_epoch_and_exact_timeline() {
    let engine = AudioEngine::test_engine();
    let mut snapshot = engine.snapshot();
    snapshot.plugin_processing_health = PluginProcessingHealth::Recovered;
    snapshot.plugin_timing_revision = 2;
    snapshot.transport_epoch = 2;
    assert!(plugin_processing_retry_acknowledged(1, 1, &snapshot, true));
    assert!(!plugin_processing_retry_acknowledged(2, 1, &snapshot, true));
    assert!(!plugin_processing_retry_acknowledged(1, 2, &snapshot, true));
    assert!(!plugin_processing_retry_acknowledged(
        1, 1, &snapshot, false
    ));
    snapshot.plugin_processing_health = PluginProcessingHealth::Faulted;
    assert!(!plugin_processing_retry_acknowledged(1, 1, &snapshot, true));
}

#[test]
fn plugin_profile_preference_migration_keeps_metronome_off_and_device_requests() {
    let preferences = AppAudioPreferences {
        requested_output: AudioDeviceProfile {
            buffer_size: AudioBufferSizeRequest::Fixed(256),
            ..AudioDeviceProfile::system_default_output()
        },
        ..Default::default()
    };
    let mut legacy = serde_json::to_value(&preferences).unwrap();
    legacy
        .as_object_mut()
        .unwrap()
        .remove("plugin_callback_budget_frames");
    let (migrated, error) = decode_audio_preferences(Some(&legacy.to_string()));
    assert!(error.is_none());
    assert_eq!(migrated.plugin_callback_budget_frames, 2_048);
    assert_eq!(migrated.requested_output, preferences.requested_output);
    assert!(!migrated.metronome_enabled);
    legacy["plugin_callback_budget_frames"] = serde_json::json!(777);
    let (repaired, error) = decode_audio_preferences(Some(&legacy.to_string()));
    assert!(error.is_some());
    assert_eq!(repaired.plugin_callback_budget_frames, 2_048);
    assert_eq!(repaired.requested_output, preferences.requested_output);
    assert!(!repaired.metronome_enabled);
}

#[test]
fn full_app_plugin_fault_diagnostic_survives_device_replacement_without_recovery_permission() {
    let mut ui = UiHarness::new();
    let faulted = AudioEngine::test_engine();
    faulted.set_plugin_processing_test_snapshot(plugin_processing_fixture(
        PluginProcessingHealth::Faulted,
        7,
    ));
    ui.app.audio = Some(faulted);
    ui.settle();
    assert!(ui.app.plugin_processing_retry_required);
    assert_eq!(ui.app.plugin_processing_last_fault.unwrap().epoch, 6);
    ui.app.audio = Some(AudioEngine::test_engine());
    ui.key(egui::Key::F10, egui::Modifiers::NONE);
    assert!(ui.app.plugin_processing_retry_required);
    assert!(
        ui.nodes
            .iter()
            .filter_map(|node| node.value().or_else(|| node.label()))
            .any(|text| text.contains("Last latched fault"))
    );
    assert!(
        ui.nodes
            .iter()
            .filter_map(|node| node.value().or_else(|| node.label()))
            .any(|text| text.contains("timing revision 7"))
    );
    assert!(
        ui.nodes
            .iter()
            .filter_map(|node| node.value().or_else(|| node.label()))
            .any(|text| text.contains("cumulative faults 0 / recoveries 0")),
        "replacement counters must not be confused with the retained fault"
    );
    ui.app.toggle_play();
    assert!(!ui.app.playing);
    assert!(!ui.app.audio_preferences.metronome_enabled);
}

#[test]
fn full_app_plugin_settings_retry_and_device_footer_remain_clickable_at_small_size() {
    for (size, capture) in [
        (egui::vec2(1440.0, 900.0), "settings-plugin-fault-full"),
        (egui::vec2(1000.0, 700.0), "settings-plugin-fault-small"),
    ] {
        let mut ui = UiHarness::new();
        ui.size = size;
        let engine = AudioEngine::test_engine();
        engine.observe_test_callback(128);
        engine.set_plugin_processing_test_snapshot(plugin_processing_fixture(
            PluginProcessingHealth::Faulted,
            1,
        ));
        ui.app.audio = Some(engine);
        ui.key(egui::Key::F10, egui::Modifiers::NONE);
        ui.app.audio_output_draft.buffer_size = AudioBufferSizeRequest::Fixed(256);
        ui.settle();
        assert!(
            ui.nodes
                .iter()
                .filter_map(|node| node.value().or_else(|| node.label()))
                .any(|label| label.contains("worker deadlines can fail even when callbacks fit"))
        );
        assert!(!ui.button("2048 frames").is_disabled());
        ui.capture(capture);
        ui.click("REVERT");
        assert_eq!(
            ui.app.audio_output_draft, ui.app.audio_preferences.requested_output,
            "the device footer must accept its actual pointer event at {size:?}"
        );
        ui.click("RETRY AUDIO PROCESSING");
        assert!(
            ui.app.plugin_processing_retry_pending.is_some(),
            "processing Retry must accept its actual pointer event at {size:?}"
        );
        assert!(!ui.app.playing);
    }
}

struct ConfigCaptureBackend {
    value: std::sync::Arc<std::sync::atomic::AtomicU32>,
    dirty: std::sync::Arc<std::sync::atomic::AtomicU64>,
    open: bool,
    note_on: bool,
    fail_metadata: bool,
}

impl plugins::plugin_runtime::PluginBackend for ConfigCaptureBackend {
    fn name(&self) -> &str {
        "configuration capture test"
    }
    fn prepare(&mut self, _: plugins::plugin_runtime::PluginPrepareConfig) -> Result<(), String> {
        if self.get_parameter(0)?.is_nan() {
            return Err("synthetic prepare failure".into());
        }
        Ok(())
    }
    fn process(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        frames: usize,
    ) -> Result<(), String> {
        let sample = if self.note_on {
            self.get_parameter(0)?
        } else {
            0.0
        };
        left[..frames].fill(sample);
        right[..frames].fill(sample);
        Ok(())
    }
    fn send_midi(&mut self, message: plugins::plugin_runtime::MidiMessage) -> Result<(), String> {
        self.note_on = message.data[0] & 0xf0 == 0x90 && message.data[2] != 0;
        Ok(())
    }
    fn set_parameter(&mut self, id: u32, value: f32) -> Result<(), String> {
        if id == u32::MAX {
            return Err("synthetic replay rejection".into());
        }
        self.value
            .store(value.to_bits(), std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
    fn get_parameter(&mut self, _: u32) -> Result<f32, String> {
        Ok(f32::from_bits(
            self.value.load(std::sync::atomic::Ordering::SeqCst),
        ))
    }
    fn parameter_catalog_snapshot(
        &mut self,
    ) -> Result<plugins::plugin_runtime::PluginParameterCatalogPage, String> {
        Ok(plugins::plugin_runtime::PluginParameterCatalogPage {
            catalog_revision: 1,
            total_items: 1,
            items: vec![plugins::plugin_runtime::PluginParameterDescriptor {
                id: 0,
                name: "Value".into(),
                unit: String::new(),
                current_normalized: self.get_parameter(0)?,
                default_normalized: Some(0.2),
                step_count: Some(0),
                automatable: true,
                read_only: false,
                bypass: false,
            }],
        })
    }
    fn save_state(&mut self) -> Result<Vec<u8>, String> {
        Ok(self.get_parameter(0)?.to_le_bytes().to_vec())
    }
    fn load_state(&mut self, bytes: &[u8]) -> Result<(), String> {
        let bytes: [u8; 4] = bytes
            .try_into()
            .map_err(|_| "invalid test state".to_owned())?;
        self.set_parameter(0, f32::from_le_bytes(bytes))
    }
    fn native_editor(
        &mut self,
        command: NativeEditorCommand,
    ) -> Result<plugins::plugin_runtime::NativeEditorState, String> {
        self.open = command != NativeEditorCommand::Close;
        Ok(plugins::plugin_runtime::NativeEditorState {
            supported: true,
            has_editor: true,
            open: self.open,
            ..Default::default()
        })
    }
    fn native_editor_feedback(
        &mut self,
    ) -> Result<plugins::plugin_runtime::NativeEditorFeedback, String> {
        Ok(plugins::plugin_runtime::NativeEditorFeedback {
            state: plugins::plugin_runtime::NativeEditorState {
                supported: true,
                has_editor: true,
                open: self.open,
                ..Default::default()
            },
            dirty_revision: self.dirty.load(std::sync::atomic::Ordering::SeqCst),
            catalog_invalidated: false,
        })
    }
    fn processing_metadata(&self) -> Result<(u32, u32), String> {
        if self.fail_metadata {
            Err("synthetic candidate metadata failure".into())
        } else {
            Ok((0, 0))
        }
    }
    fn latency_samples(&self) -> u32 {
        0
    }
    fn tail_samples(&self) -> u32 {
        0
    }
}

fn config_capture_plugin(project: &mut Project, target: PluginPickerTarget, index: usize) -> u64 {
    commit_loaded_plugin(
        project,
        target,
        &PluginDescriptor {
            id: format!("config-state-{index}"),
            name: format!("State {index}"),
            vendor: "TEST".into(),
            path: PathBuf::from(format!("/not-a-real-plugin/state-{index}.so")),
            format: ScannedPluginFormat::Vst2,
            category: "Effect".into(),
            is_instrument: matches!(target, PluginPickerTarget::ChannelDevice { .. }),
            verified: false,
            vst3_metadata: None,
            scan_error: None,
        },
    )
    .unwrap()
}

fn wait_config_condition(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !predicate() {
        assert!(
            Instant::now() < deadline,
            "synthetic state worker did not answer"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn plugin_config_native_capture_all_slots_survives_bypass_enable_and_save_reopen() {
    use plugins::plugin_runtime::{BackendSlot, PluginBackend, PluginChain, PluginPrepareConfig};
    use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
    let mut ui = UiHarness::new();
    let ids = (0..2)
        .map(|slot| {
            config_capture_plugin(
                &mut ui.app.project,
                PluginPickerTarget::MixerSlot { track: 1, slot },
                slot,
            )
        })
        .collect::<Vec<_>>();
    for instance in &mut ui.app.project.plugin_instances {
        instance.parameters.insert(0, 0.2);
        instance.opaque_state = 0.2_f32.to_le_bytes().to_vec();
    }
    let values = [
        Arc::new(AtomicU32::new(0.2_f32.to_bits())),
        Arc::new(AtomicU32::new(0.2_f32.to_bits())),
    ];
    let dirty = [Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0))];
    let backend_values = values.clone();
    let backend_dirty = dirty.clone();
    let PluginChain {
        audio: _endpoint,
        control,
        guard,
    } = PluginChain::spawn_identified_with_backend_factory(
        &ids,
        move || {
            backend_values
                .into_iter()
                .zip(backend_dirty)
                .map(|(value, dirty)| {
                    BackendSlot::new(Box::new(ConfigCaptureBackend {
                        value,
                        dirty,
                        open: false,
                        note_on: false,
                        fail_metadata: false,
                    }))
                })
                .collect()
        },
        PluginPrepareConfig {
            sample_rate: 48_000.0,
            max_block_frames: 128,
        },
    )
    .unwrap();
    ui.app.running_insert_chains.insert(
        1,
        RunningInsertChain {
            project_session: ui.app.project_session,
            endpoint_id: 77,
            instance_ids: ids.clone(),
            control,
            guard,
        },
    );
    ui.app.audio = Some(AudioEngine::test_engine());
    wait_config_condition(|| {
        ui.app.poll_plugin_runtime_events();
        ui.app
            .project
            .plugin_instances
            .iter()
            .all(|instance| instance.runtime_status == PluginRuntimeStatus::Loaded)
    });
    // Exercise the actual synthetic worker's native open/edit/close state-capture
    // protocol. This is not a real plug-in window or a DSP callback proof.
    for slot in 0..2 {
        let control = &ui.app.running_insert_chains[&1].control;
        assert!(control.request_native_editor(
            slot,
            100 + slot as u64,
            NativeEditorCommand::Open { owner: None },
            &[0]
        ));
        wait_config_condition(|| {
            control
                .native_editor_snapshot(slot)
                .is_some_and(|snapshot| snapshot.pending_request.is_none() && snapshot.state.open)
        });
        values[slot].store([0.73_f32, 0.91][slot].to_bits(), Ordering::SeqCst);
        dirty[slot].store(1, Ordering::SeqCst);
        assert!(control.request_native_editor(
            slot,
            200 + slot as u64,
            NativeEditorCommand::Close,
            &[0]
        ));
        wait_config_condition(|| {
            control
                .native_editor_snapshot(slot)
                .is_some_and(|snapshot| {
                    assert!(
                        snapshot.error.is_none(),
                        "native fixture capture failed: {:?}",
                        snapshot.error
                    );
                    snapshot.pending_request.is_none()
                        && snapshot.captured_state.is_some()
                        && !snapshot.state.open
                })
        });
    }
    ui.app.poll_native_editor_snapshots();
    for (instance, expected) in ui.app.project.plugin_instances.iter().zip([0.73_f32, 0.91]) {
        assert_eq!(
            instance.parameters[&0], expected,
            "fresh native bases must replace stale generic replay values"
        );
    }
    ui.app.persist_plugin_slot_config(ids[0], true, true, 1.0);
    let mut pending = ui
        .app
        .pending_plugin_slot_config
        .take()
        .expect("live config uses a state barrier");
    assert_eq!(
        pending.probes.len(),
        2,
        "replacement captures every serial FX slot"
    );
    assert!(!pending.matches_receipt(ui.app.project_session + 1, 77, ids[0]));
    assert!(!pending.matches_receipt(ui.app.project_session, 78, ids[0]));
    // The exact stopped-epoch fence is covered by the callback integration tests.
    // Here drive real tagged worker receipts through the production App event path.
    pending.snapshot.requests_initialized = true;
    pending.snapshot.parameter_edits_drained = true;
    let capture_epoch = ui.app.running_insert_chains[&1]
        .control
        .stats()
        .current_epoch;
    pending.captured_identity = Some((1, capture_epoch));
    for probe in &pending.probes {
        assert_eq!(
            ui.app.request_plugin_state_probe_for_epoch(
                *probe,
                pending.snapshot.request_id,
                Some(capture_epoch)
            ),
            Some(true)
        );
        ui.app.suppress_plugin_config_native_capture(*probe);
        pending.snapshot.waiting.insert(probe.instance_id());
    }
    ui.app.pending_plugin_slot_config = Some(pending);
    wait_config_condition(|| {
        ui.app.poll_plugin_runtime_events();
        ui.app
            .pending_plugin_slot_config
            .as_ref()
            .unwrap()
            .snapshot
            .ready()
    });
    let pending = ui.app.pending_plugin_slot_config.take().unwrap();
    assert_eq!(pending.snapshot.states.len(), 2);
    // A late config-owned receipt and its native snapshot side channel must not
    // mutate the old model after the intent has been removed or rejected.
    let before_late = project_fingerprint(&ui.app.project);
    let previous_serial = ui.app.running_insert_chains[&1]
        .control
        .native_editor_snapshot(0)
        .unwrap()
        .capture_serial;
    values[0].store(0.55f32.to_bits(), Ordering::SeqCst);
    assert!(
        ui.app.running_insert_chains[&1]
            .control
            .request_state_tagged_for_epoch(0, pending.snapshot.request_id, capture_epoch)
    );
    wait_config_condition(|| {
        ui.app.poll_plugin_runtime_events();
        ui.app.running_insert_chains[&1]
            .control
            .native_editor_snapshot(0)
            .unwrap()
            .capture_serial
            > previous_serial
    });
    ui.app.poll_plugin_runtime_events();
    assert_eq!(project_fingerprint(&ui.app.project), before_late);
    values[0].store(0.73f32.to_bits(), Ordering::SeqCst);
    let before = ui.app.project.clone();
    let mut candidate =
        plugin_slot_config_candidate(&before, ids[0], pending.config, &pending.snapshot.states)
            .unwrap();
    assert!(!before.plugin_instances[0].bypass);
    assert!(candidate.plugin_instances[0].bypass);
    for (instance, expected) in candidate.plugin_instances.iter().zip([0.73_f32, 0.91]) {
        let spec = plugin_instance_runtime_spec(instance);
        let restored_value = Arc::new(AtomicU32::new(0));
        let mut restored = ConfigCaptureBackend {
            value: Arc::clone(&restored_value),
            dirty: Arc::new(AtomicU64::new(0)),
            open: false,
            note_on: false,
            fail_metadata: false,
        };
        restored
            .prepare(PluginPrepareConfig {
                sample_rate: 48_000.0,
                max_block_frames: 128,
            })
            .unwrap();
        restored.load_state(&spec.initial_state).unwrap();
        for (&parameter, &value) in &instance.parameters {
            restored.set_parameter(parameter, value).unwrap();
        }
        assert_eq!(
            restored.get_parameter(0).unwrap(),
            expected,
            "opaque restoration followed by generic base replay must retain the native edit"
        );
        restored
            .send_midi(plugins::plugin_runtime::MidiMessage::new(
                [0x90, 60, 100],
                0,
            ))
            .unwrap();
        let mut left = [0.0; 128];
        let mut right = [0.0; 128];
        restored.process(&mut left, &mut right, 128).unwrap();
        assert_eq!(
            restored.get_parameter(0).unwrap(),
            expected,
            "state must survive the first actual Process call"
        );
        assert!(
            left.iter().chain(&right).all(|sample| *sample == expected),
            "the first note after fresh restoration must sound with its retained parameter"
        );
    }
    for config in [
        SlotConfig {
            enabled: false,
            bypassed: false,
            wet: 0.5,
        },
        SlotConfig::default(),
    ] {
        candidate =
            plugin_slot_config_candidate(&candidate, ids[0], config, &pending.snapshot.states)
                .unwrap();
    }
    let fixture = Fixture::new();
    let path = fixture.0.join("captured-config.citrus");
    candidate.save(&path).unwrap();
    let reopened = Project::load(&path).unwrap();
    for (instance, expected) in reopened.plugin_instances.iter().zip([0.73_f32, 0.91]) {
        assert_eq!(instance.parameters[&0], expected);
        assert_eq!(instance.opaque_state, expected.to_le_bytes());
    }
    assert_eq!(
        ui.app.running_insert_chains[&1].endpoint_id, 77,
        "capture/candidate preparation never mutates the old endpoint"
    );
    for failure in ["capture", "session", "epoch", "project"] {
        let mut before = project_fingerprint(&ui.app.project);
        ui.app.persist_plugin_slot_config(ids[0], true, true, 1.0);
        let pending = ui.app.pending_plugin_slot_config.as_mut().unwrap();
        if failure == "session" {
            if let PluginStateProbe::Insert {
                project_session, ..
            } = &mut pending.probes[0]
            {
                *project_session += 1;
            }
        } else if failure == "project" {
            pending.captured_project_fingerprint = Some(before);
            ui.app.project.name = "new user title during capture".into();
            before = project_fingerprint(&ui.app.project);
        } else if failure == "epoch" {
            pending.snapshot.requests_initialized = true;
            pending.snapshot.parameter_edits_drained = true;
            pending.captured_identity = Some((1, capture_epoch + 1));
            pending.snapshot.states = reopened
                .plugin_instances
                .iter()
                .map(|i| (i.id, i.opaque_state.clone()))
                .collect();
        } else {
            pending.snapshot.failure = Some("state capture failed".into());
        }
        ui.app.drive_plugin_slot_config_capture();
        assert!(ui.app.pending_plugin_slot_config.is_none());
        assert_eq!(project_fingerprint(&ui.app.project), before);
        assert_eq!(ui.app.running_insert_chains[&1].endpoint_id, 77);
        assert_eq!(f32::from_bits(values[0].load(Ordering::SeqCst)), 0.73);
        assert!(!ui.app.playing);
        assert!(!ui.app.plugin_processing_retry_required);
    }
}

#[test]
fn plugin_config_failed_capture_and_stale_generation_leave_project_and_chain_unchanged() {
    for stale in [false, true] {
        let mut ui = UiHarness::new();
        let id = config_capture_plugin(
            &mut ui.app.project,
            PluginPickerTarget::MixerSlot { track: 1, slot: 0 },
            0,
        );
        let before = ui.app.project.clone();
        let mut snapshot = RestartPluginStateSnapshot::new(42);
        snapshot.failure = (!stale).then(|| "synthetic state capture failed".into());
        snapshot.waiting.insert(id);
        assert!(
            !snapshot.observe_state(id, 41, &[9]),
            "stale request cannot complete capture"
        );
        ui.app.pending_plugin_slot_config = Some(PendingPluginSlotConfig {
            instance_id: id,
            config: SlotConfig {
                bypassed: true,
                ..Default::default()
            },
            probes: vec![PluginStateProbe::Insert {
                project_session: ui.app.project_session + u64::from(stale),
                track: 1,
                endpoint_id: 77,
                slot: 0,
                instance_id: id,
            }],
            snapshot,
            resume_playback: true,
            stopped_epoch: None,
            captured_identity: None,
            captured_project_fingerprint: None,
            prepared: None,
        });
        ui.app.drive_plugin_slot_config_capture();
        assert!(ui.app.pending_plugin_slot_config.is_none());
        assert_eq!(
            project_fingerprint(&ui.app.project),
            project_fingerprint(&before)
        );
        assert_eq!(
            ui.app.project.plugin_instances[0].opaque_state,
            before.plugin_instances[0].opaque_state
        );
        assert!(ui.app.running_insert_chains.is_empty());
        assert!(ui.app.pending_insert_chains.is_empty());
        assert!(!ui.app.playing);
        assert!(!ui.app.plugin_processing_retry_required);
    }
}

#[test]
fn plugin_config_routed_generator_bypass_is_rejected_but_offline_fx_remains_editable() {
    let mut ui = UiHarness::new();
    let first = config_capture_plugin(
        &mut ui.app.project,
        PluginPickerTarget::ChannelDevice { channel: 0 },
        0,
    );
    let second = config_capture_plugin(
        &mut ui.app.project,
        PluginPickerTarget::ChannelDevice { channel: 1 },
        1,
    );
    ui.app
        .project
        .plugin_instances
        .iter_mut()
        .find(|instance| instance.id == first)
        .unwrap()
        .midi_ports
        .output = Some(0);
    ui.app
        .project
        .plugin_instances
        .iter_mut()
        .find(|instance| instance.id == second)
        .unwrap()
        .midi_ports
        .input = Some(0);
    let fx = config_capture_plugin(
        &mut ui.app.project,
        PluginPickerTarget::MixerSlot { track: 1, slot: 0 },
        2,
    );
    for instance in [first, second] {
        ui.app.persist_plugin_slot_config(instance, true, true, 1.0);
        ui.app
            .persist_plugin_slot_config(instance, false, false, 1.0);
        let unchanged = ui
            .app
            .project
            .plugin_instances
            .iter()
            .find(|plugin| plugin.id == instance)
            .unwrap();
        assert!(unchanged.enabled);
        assert!(!unchanged.bypass);
        assert!(ui.app.pending_plugin_slot_config.is_none());
    }
    ui.app.persist_plugin_slot_config(fx, true, true, 0.25);
    let changed = ui
        .app
        .project
        .plugin_instances
        .iter()
        .find(|plugin| plugin.id == fx)
        .unwrap();
    assert!(changed.bypass);
    assert_eq!(changed.wet, 0.25);
    assert!(!ui.app.audio_preferences.metronome_enabled);
}

#[test]
fn plugin_config_capture_rejects_seek_or_new_activation_after_state_requests() {
    assert!(plugin_config_capture_identity_matches(
        (5, 9),
        Some((5, 9)),
        9,
        true
    ));
    assert!(!plugin_config_capture_identity_matches(
        (5, 9),
        Some((5, 10)),
        10,
        true
    ));
    assert!(!plugin_config_capture_identity_matches(
        (5, 9),
        Some((6, 9)),
        9,
        true
    ));
    assert!(!plugin_config_capture_identity_matches(
        (5, 9),
        Some((5, 9)),
        10,
        true
    ));
    assert!(!plugin_config_capture_identity_matches(
        (5, 9),
        Some((5, 9)),
        9,
        false
    ));
}

fn spawn_config_candidate_fixture(
    candidate: &Project,
    ids: &[u64],
    fail_prepare: bool,
    fail_restore: bool,
) -> plugins::plugin_runtime::PluginChain {
    spawn_config_candidate_fixture_checked(candidate, ids, fail_prepare, fail_restore, false)
}

fn spawn_config_candidate_fixture_checked(
    candidate: &Project,
    ids: &[u64],
    fail_prepare: bool,
    fail_restore: bool,
    fail_metadata: bool,
) -> plugins::plugin_runtime::PluginChain {
    use plugins::plugin_runtime::{BackendSlot, PluginBackend, PluginChain, PluginPrepareConfig};
    use std::sync::atomic::{AtomicU32, AtomicU64};
    let instances = ids
        .iter()
        .map(|id| {
            candidate
                .plugin_instances
                .iter()
                .find(|i| i.id == *id)
                .unwrap()
                .clone()
        })
        .collect::<Vec<_>>();
    PluginChain::spawn_identified_with_backend_factory(
        ids,
        move || {
            instances
                .into_iter()
                .map(|instance| {
                    let mut backend = ConfigCaptureBackend {
                        value: Arc::new(AtomicU32::new(0)),
                        dirty: Arc::new(AtomicU64::new(0)),
                        open: false,
                        note_on: false,
                        fail_metadata,
                    };
                    if fail_restore {
                        backend
                            .load_state(&[])
                            .expect("synthetic loader restore failure");
                    }
                    backend.load_state(&instance.opaque_state).unwrap();
                    if fail_prepare {
                        backend
                            .value
                            .store(f32::NAN.to_bits(), std::sync::atomic::Ordering::SeqCst);
                    }
                    let mut slot = BackendSlot::new(Box::new(backend));
                    slot.config = SlotConfig {
                        enabled: instance.enabled,
                        bypassed: instance.bypass,
                        wet: instance.wet,
                    };
                    slot
                })
                .collect()
        },
        PluginPrepareConfig {
            sample_rate: 48_000.0,
            max_block_frames: 128,
        },
    )
    .unwrap()
}

#[test]
fn plugin_config_prepared_candidate_waits_for_all_slots_and_replay_before_install() {
    let mut ui = UiHarness::new();
    let ids = (0..2)
        .map(|slot| {
            config_capture_plugin(
                &mut ui.app.project,
                PluginPickerTarget::MixerSlot { track: 1, slot },
                slot,
            )
        })
        .collect::<Vec<_>>();
    for (instance, value) in ui
        .app
        .project
        .plugin_instances
        .iter_mut()
        .zip([0.73f32, 0.91])
    {
        instance.opaque_state = 0.2f32.to_le_bytes().to_vec();
        instance.parameters.insert(0, value);
        instance.runtime_status = PluginRuntimeStatus::Loaded;
    }
    let old = spawn_config_candidate_fixture(&ui.app.project, &ids, false, false);
    let plugins::plugin_runtime::PluginChain {
        audio: _old_endpoint,
        control,
        guard,
    } = old;
    ui.app.running_insert_chains.insert(
        1,
        RunningInsertChain {
            project_session: ui.app.project_session,
            endpoint_id: 77,
            instance_ids: ids.clone(),
            control,
            guard,
        },
    );
    ui.app.audio = Some(AudioEngine::test_engine());
    let before = project_fingerprint(&ui.app.project);
    let mut candidate = ui.app.project.clone();
    candidate.plugin_instances[0].bypass = true;
    let chain = spawn_config_candidate_fixture(&candidate, &ids, false, false);
    let mut prepared = PreparedPluginSlotConfig::new(candidate, chain, ids.clone(), 701);
    wait_config_condition(|| prepared.poll().unwrap());
    assert_eq!(prepared.ready.len(), 2);
    assert!(prepared.parameter_waiting.is_empty());
    assert_eq!(
        prepared
            .chain
            .control
            .plugin_latency_snapshot()
            .unwrap()
            .active_mask,
        2
    );
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert_eq!(ui.app.running_insert_chains[&1].endpoint_id, 77);
    for (instance, expected) in prepared
        .candidate
        .plugin_instances
        .iter()
        .zip([0.73f32, 0.91])
    {
        assert_eq!(
            instance.opaque_state,
            0.2f32.to_le_bytes(),
            "replacement readiness must preserve the authoritative captured blob"
        );
        assert_eq!(instance.parameters[&0], expected);
    }
    assert!(ui.app.install_plugin_slot_config_candidate(
        ids[0],
        prepared.candidate,
        prepared.chain
    ));
    assert!(ui.app.pending_insert_chains.contains_key(&1));
    assert_eq!(
        ui.app.running_insert_chains[&1].endpoint_id, 77,
        "old guard stays owned until callback admission"
    );
    let phase = TimelineAudioSyncPhase::AwaitingActivation {
        generation: 5,
        seek_serial: 1,
        epoch: 2,
        frame: 0,
        activation_request_id: 0,
    };
    ui.app.timeline_audio_sync.phase = phase;
    ui.app.drive_timeline_transport_activation();
    assert_eq!(
        ui.app.timeline_audio_sync.phase, phase,
        "queued but unacknowledged Mixer install cannot activate against same-instance old FX"
    );
    assert!(!ui.app.timeline_insert_replacements_ready());
    // Simulate only the control-side ownership transfer after exact callback
    // admission. Callback admission itself is covered by audio integration tests.
    let pending = ui.app.pending_insert_chains.remove(&1).unwrap();
    ui.app.running_insert_chains.insert(
        1,
        RunningInsertChain {
            project_session: pending.project_session,
            endpoint_id: pending.endpoint_id,
            instance_ids: pending.instance_ids,
            control: pending.control,
            guard: pending.guard,
        },
    );
    ui.app.poll_native_editor_snapshots();
    assert!(
        ui.app
            .project
            .plugin_instances
            .iter()
            .all(|instance| instance.opaque_state == 0.2f32.to_le_bytes()),
        "first App poll must preserve the authoritative captured blob"
    );
}

#[test]
fn plugin_config_candidate_prepare_restore_and_replay_failures_preserve_old_model() {
    for failure in ["prepare", "restore", "replay", "metadata", "load"] {
        let mut ui = UiHarness::new();
        let id = config_capture_plugin(
            &mut ui.app.project,
            PluginPickerTarget::MixerSlot { track: 1, slot: 0 },
            0,
        );
        ui.app.project.plugin_instances[0].opaque_state = 0.73f32.to_le_bytes().to_vec();
        let old = spawn_config_candidate_fixture(&ui.app.project, &[id], false, false);
        let plugins::plugin_runtime::PluginChain {
            audio: _old_endpoint,
            control,
            guard,
        } = old;
        ui.app.running_insert_chains.insert(
            1,
            RunningInsertChain {
                project_session: ui.app.project_session,
                endpoint_id: 77,
                instance_ids: vec![id],
                control,
                guard,
            },
        );
        let before = project_fingerprint(&ui.app.project);
        let mut candidate = ui.app.project.clone();
        candidate.plugin_instances[0].bypass = true;
        if failure == "replay" {
            candidate.plugin_instances[0]
                .parameters
                .insert(u32::MAX, 0.9);
        }
        let chain = if failure == "load" {
            plugins::plugin_runtime::PluginChain::spawn_identified(
                vec![(
                    id,
                    plugin_instance_runtime_spec(&candidate.plugin_instances[0]),
                )],
                plugins::plugin_runtime::PluginPrepareConfig {
                    sample_rate: 48_000.0,
                    max_block_frames: 2048,
                },
            )
            .unwrap()
        } else {
            spawn_config_candidate_fixture_checked(
                &candidate,
                &[id],
                failure == "prepare",
                failure == "restore",
                failure == "metadata",
            )
        };
        let mut prepared = PreparedPluginSlotConfig::new(candidate, chain, vec![id], 701);
        wait_config_condition(|| match prepared.poll() {
            Err(_) => true,
            Ok(false) => false,
            Ok(true) => panic!("{failure} must reject candidate"),
        });
        drop(prepared);
        assert_eq!(project_fingerprint(&ui.app.project), before);
        assert_eq!(ui.app.running_insert_chains[&1].endpoint_id, 77);
        assert!(ui.app.pending_insert_chains.is_empty());
        assert!(!ui.app.playing);
    }
}

#[test]
fn plugin_config_prepared_candidate_timeout_drops_only_candidate_and_preserves_old_chain() {
    let mut ui = UiHarness::new();
    let id = config_capture_plugin(
        &mut ui.app.project,
        PluginPickerTarget::MixerSlot { track: 1, slot: 0 },
        0,
    );
    ui.app.project.plugin_instances[0].opaque_state = 0.73f32.to_le_bytes().to_vec();
    let old = spawn_config_candidate_fixture(&ui.app.project, &[id], false, false);
    let plugins::plugin_runtime::PluginChain {
        audio: _old_endpoint,
        control,
        guard,
    } = old;
    ui.app.running_insert_chains.insert(
        1,
        RunningInsertChain {
            project_session: ui.app.project_session,
            endpoint_id: 77,
            instance_ids: vec![id],
            control,
            guard,
        },
    );
    ui.app.audio = Some(AudioEngine::test_engine());
    let before = project_fingerprint(&ui.app.project);
    let mut candidate = ui.app.project.clone();
    candidate.plugin_instances[0].bypass = true;
    let chain = spawn_config_candidate_fixture(&candidate, &[id], false, false);
    let prepared = PreparedPluginSlotConfig::new(candidate, chain, vec![id], 701);
    let mut snapshot = RestartPluginStateSnapshot::new(700);
    snapshot.started_at =
        Instant::now() - AUDIO_RESTART_PLUGIN_SNAPSHOT_TIMEOUT - Duration::from_millis(1);
    snapshot.parameter_edits_drained = true;
    snapshot.requests_initialized = true;
    ui.app.pending_plugin_slot_config = Some(PendingPluginSlotConfig {
        instance_id: id,
        config: SlotConfig {
            bypassed: true,
            ..Default::default()
        },
        probes: ui.app.current_plugin_state_probes(),
        snapshot,
        resume_playback: true,
        stopped_epoch: Some(0),
        captured_identity: None,
        captured_project_fingerprint: None,
        prepared: Some(prepared),
    });
    ui.app.drive_plugin_slot_config_capture();
    assert!(ui.app.pending_plugin_slot_config.is_none());
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert_eq!(ui.app.running_insert_chains[&1].endpoint_id, 77);
    assert!(ui.app.pending_insert_chains.is_empty());
    assert!(!ui.app.playing);
    assert!(!ui.app.plugin_processing_retry_required);
}

#[test]
fn plugin_config_pending_shortcuts_preserve_project_and_escape_cancels() {
    let mut ui = UiHarness::new();
    ui.app.pending_plugin_slot_config = Some(PendingPluginSlotConfig {
        instance_id: 1,
        config: SlotConfig::default(),
        probes: Vec::new(),
        snapshot: RestartPluginStateSnapshot::new(42),
        resume_playback: true,
        stopped_epoch: None,
        captured_identity: None,
        captured_project_fingerprint: None,
        prepared: None,
    });
    let before = project_fingerprint(&ui.app.project);
    let mode = ui.app.transport_mode;
    assert!(ui.app.shortcut_blocking_layer_active());
    let ctx = ui.ctx.clone();
    for (key, modifiers) in [
        (egui::Key::Delete, egui::Modifiers::NONE),
        (egui::Key::D, egui::Modifiers::COMMAND),
        (egui::Key::L, egui::Modifiers::NONE),
        (egui::Key::Space, egui::Modifiers::NONE),
    ] {
        let _ = ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                }],
                ..Default::default()
            },
            |_| {
                ui.app.handle_shortcuts(&ctx, None, false);
            },
        );
        assert_eq!(project_fingerprint(&ui.app.project), before);
        assert_eq!(ui.app.transport_mode, mode);
        assert!(ui.app.pending_plugin_slot_config.is_some());
        assert!(!ui.app.playing);
    }
    let _ = ctx.run_ui(
        egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |_| {
            ui.app.handle_shortcuts(&ctx, None, false);
        },
    );
    assert!(ui.app.pending_plugin_slot_config.is_none());
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert!(!ui.app.playing);
}

struct DeferredConfigBackend {
    inner: ConfigCaptureBackend,
    deferred: Option<f32>,
    saves: Arc<std::sync::atomic::AtomicU64>,
    processes: Arc<std::sync::atomic::AtomicU64>,
}

impl plugins::plugin_runtime::PluginBackend for DeferredConfigBackend {
    fn name(&self) -> &str {
        "deferred configuration fixture"
    }
    fn prepare(
        &mut self,
        config: plugins::plugin_runtime::PluginPrepareConfig,
    ) -> Result<(), String> {
        self.inner.prepare(config)
    }
    fn process(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        frames: usize,
    ) -> Result<(), String> {
        self.processes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if let Some(value) = self.deferred.take() {
            self.inner.set_parameter(0, value)?;
            self.inner.note_on = false;
        }
        self.inner.process(left, right, frames)
    }
    fn send_midi(&mut self, message: plugins::plugin_runtime::MidiMessage) -> Result<(), String> {
        self.inner.send_midi(message)
    }
    fn set_parameter(&mut self, id: u32, value: f32) -> Result<(), String> {
        self.inner.set_parameter(id, value)
    }
    fn get_parameter(&mut self, id: u32) -> Result<f32, String> {
        self.inner.get_parameter(id)
    }
    fn save_state(&mut self) -> Result<Vec<u8>, String> {
        self.saves.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.save_state()
    }
    fn load_state(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.deferred = Some(f32::from_le_bytes(
            bytes.try_into().map_err(|_| "invalid state")?,
        ));
        Ok(())
    }
    fn latency_samples(&self) -> u32 {
        0
    }
    fn tail_samples(&self) -> u32 {
        0
    }
}

#[test]
fn plugin_config_deferred_restore_preserves_blob_without_resave_or_hidden_process() {
    use plugins::plugin_runtime::{
        BackendSlot, PluginBackend, PluginChain, PluginPrepareConfig, SubmitStatus,
    };
    use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
    let mut project = Project::default();
    let id = config_capture_plugin(
        &mut project,
        PluginPickerTarget::MixerSlot { track: 1, slot: 0 },
        0,
    );
    project.plugin_instances[0].opaque_state = 0.73f32.to_le_bytes().to_vec();
    let saves = Arc::new(AtomicU64::new(0));
    let processes = Arc::new(AtomicU64::new(0));
    let value = Arc::new(AtomicU32::new(1f32.to_bits()));
    let (worker_saves, worker_processes, worker_value) = (
        Arc::clone(&saves),
        Arc::clone(&processes),
        Arc::clone(&value),
    );
    let chain = PluginChain::spawn_identified_with_backend_factory(
        &[id],
        move || {
            let mut backend = DeferredConfigBackend {
                inner: ConfigCaptureBackend {
                    value: worker_value,
                    dirty: Arc::new(AtomicU64::new(0)),
                    open: false,
                    note_on: false,
                    fail_metadata: false,
                },
                deferred: None,
                saves: worker_saves,
                processes: worker_processes,
            };
            backend.load_state(&0.73f32.to_le_bytes()).unwrap();
            assert_eq!(
                backend.save_state().unwrap(),
                1f32.to_le_bytes(),
                "fixture really exposes stale pre-Process state"
            );
            vec![BackendSlot::new(Box::new(backend))]
        },
        PluginPrepareConfig {
            sample_rate: 48_000.0,
            max_block_frames: 128,
        },
    )
    .unwrap();
    let mut prepared = PreparedPluginSlotConfig::new(project, chain, vec![id], 41);
    wait_config_condition(|| prepared.poll().unwrap());
    assert_eq!(
        prepared.candidate.plugin_instances[0].opaque_state,
        0.73f32.to_le_bytes()
    );
    assert_eq!(
        saves.load(Ordering::SeqCst),
        1,
        "readiness adds no replacement SaveState"
    );
    assert_eq!(
        processes.load(Ordering::SeqCst),
        0,
        "readiness adds no hidden Process"
    );
    assert_eq!(f32::from_bits(value.load(Ordering::SeqCst)), 1.0);
    // An explicit test-driven real worker Process, separate from preparation,
    // demonstrates why the pre-Process readback was not authoritative.
    assert!(matches!(
        prepared.chain.audio.try_submit(&[0.0; 128], &[0.0; 128]),
        SubmitStatus::Submitted { .. }
    ));
    wait_config_condition(|| prepared.chain.control.stats().completed != 0);
    assert_eq!(f32::from_bits(value.load(Ordering::SeqCst)), 0.73);
    assert_eq!(
        prepared.candidate.plugin_instances[0].opaque_state,
        0.73f32.to_le_bytes()
    );
}

#[test]
fn native_automation_context_menu_dismiss_create_and_reopen() {
    let mut ui = UiHarness::new();
    ui.app.install_project(Project::blank(), None, false);
    let channel = ui.app.project.channels[0].id;
    for (view, label, target) in [
        (
            StudioView::ChannelRack,
            format!("Channel {channel} · Volume"),
            AutomationTarget::ChannelVolume { channel },
        ),
        (
            StudioView::ChannelRack,
            format!("Channel {channel} · Pan"),
            AutomationTarget::ChannelPan { channel },
        ),
        (
            StudioView::Mixer,
            "Master · Volume".into(),
            AutomationTarget::MasterVolume,
        ),
        (
            StudioView::Mixer,
            "Master · Pan".into(),
            AutomationTarget::MasterPan,
        ),
    ] {
        let open_menu = |ui: &mut UiHarness| {
            ui.app.focus_editor(view);
            ui.settle();
            let node = ui
                .nodes
                .iter()
                .find(|node| node.label() == Some(label.as_str()))
                .expect("native control has accessible bounds");
            let bounds = node.bounds().unwrap();
            let pos = Pos2::new(
                ((bounds.x0 + bounds.x1) / 2.0) as f32,
                ((bounds.y0 + bounds.y1) / 2.0) as f32,
            );
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
            ui.settle();
            ui.button("Create/open automation clip");
        };
        let before = project_fingerprint(&ui.app.project);
        open_menu(&mut ui);
        ui.key(egui::Key::Escape, egui::Modifiers::NONE);
        assert_eq!(project_fingerprint(&ui.app.project), before);
        open_menu(&mut ui);
        ui.click("Create/open automation clip");
        let clip_id = ui.app.selected_clip.expect("new automation selected");
        let clip = ui
            .app
            .project
            .clips
            .iter()
            .find(|clip| clip.id == clip_id)
            .unwrap();
        let lane = ui
            .app
            .project
            .automation_lanes
            .iter()
            .find(|lane| Some(lane.id) == clip.automation_id)
            .unwrap();
        assert_eq!(lane.lane.target(), &target);
        assert!(matches!(ui.app.tool_mode, ToolMode::Select));
        let fingerprint = project_fingerprint(&ui.app.project);
        let undo_len = ui.app.undo_stack.len();
        open_menu(&mut ui);
        ui.click("Create/open automation clip");
        assert_eq!(ui.app.selected_clip, Some(clip_id));
        assert_eq!(project_fingerprint(&ui.app.project), fingerprint);
        assert_eq!(ui.app.undo_stack.len(), undo_len);
        let command = egui::Modifiers {
            ctrl: true,
            command: true,
            ..Default::default()
        };
        ui.key(egui::Key::Z, command);
        assert_eq!(
            project_fingerprint(&ui.app.project),
            before,
            "Undo removes only this lane and clip"
        );
        ui.key(egui::Key::Y, command);
        assert_eq!(
            project_fingerprint(&ui.app.project),
            fingerprint,
            "Redo restores the exact lane and clip"
        );
    }
}

fn automation_point_ui() -> UiHarness {
    let mut ui = UiHarness::new();
    ui.app.install_project(Project::blank(), None, false);
    ui.app
        .create_or_open_native_automation(AutomationTarget::MasterPan);
    ui.app.project.automation_lanes[0].lane.replace_points([
        AutomationPoint::new(0.0, -0.5),
        AutomationPoint::new(2.0, 0.0),
        AutomationPoint::new(4.0, 0.5),
    ]);
    ui.app.sync_history_observer();
    ui.app.undo_stack.clear();
    ui.app.redo_stack.clear();
    ui.settle();
    ui
}

fn automation_point_position(ui: &UiHarness, index: usize) -> Pos2 {
    let label = format!("Automation point {index}");
    let node = ui
        .nodes
        .iter()
        .find(|node| node.label() == Some(label.as_str()))
        .expect("automation point accessibility");
    let bounds = node.bounds().unwrap();
    Pos2::new(
        ((bounds.x0 + bounds.x1) / 2.0) as f32,
        ((bounds.y0 + bounds.y1) / 2.0) as f32,
    )
}

fn automation_secondary_click(ui: &mut UiHarness, pos: Pos2) {
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
    ui.settle();
}

#[test]
fn automation_point_menu_cancel_copy_paste_delete_atomic_history() {
    let mut ui = automation_point_ui();
    let before = project_fingerprint(&ui.app.project);
    let pos = automation_point_position(&ui, 2);
    automation_secondary_click(&mut ui, pos);
    ui.click("Cancel");
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert!(ui.app.undo_stack.is_empty());
    let pos = automation_point_position(&ui, 2);
    automation_secondary_click(&mut ui, pos);
    ui.click("Copy value");
    assert_eq!(ui.app.automation_point_clipboard, Some(0.5));
    assert!(ui.app.undo_stack.is_empty());
    let pos = automation_point_position(&ui, 3);
    automation_secondary_click(&mut ui, pos);
    ui.click("Paste value");
    assert_eq!(
        ui.app.project.automation_lanes[0].lane.points()[2].value,
        0.0
    );
    assert_eq!(ui.app.project.clips.len(), 1);
    assert_eq!(ui.app.undo_stack.len(), 1);
    let command = egui::Modifiers {
        ctrl: true,
        command: true,
        ..Default::default()
    };
    ui.key(egui::Key::Z, command);
    assert_eq!(project_fingerprint(&ui.app.project), before);
    let pos = automation_point_position(&ui, 2);
    automation_secondary_click(&mut ui, pos);
    ui.click("Delete point");
    assert_eq!(ui.app.project.automation_lanes[0].lane.points().len(), 2);
    assert_eq!(ui.app.project.clips.len(), 1);
    ui.key(egui::Key::Z, command);
    assert_eq!(project_fingerprint(&ui.app.project), before);
}

#[test]
fn automation_point_blank_insert_and_drag_modifiers_and_cancel() {
    let mut ui = automation_point_ui();
    let middle = automation_point_position(&ui, 2);
    let left = automation_point_position(&ui, 1);
    automation_secondary_click(&mut ui, Pos2::new((middle.x + left.x) / 2.0, middle.y));
    assert_eq!(ui.app.project.automation_lanes[0].lane.points().len(), 4);
    assert_eq!(ui.app.project.clips.len(), 1);
    assert_eq!(ui.app.undo_stack.len(), 1);
    let command = egui::Modifiers {
        ctrl: true,
        command: true,
        ..Default::default()
    };
    ui.key(egui::Key::Z, command);
    let before = project_fingerprint(&ui.app.project);
    for (modifiers, cancel) in [
        (egui::Modifiers::CTRL, false),
        (egui::Modifiers::SHIFT, false),
        (egui::Modifiers::ALT, true),
    ] {
        let start = automation_point_position(&ui, 2);
        let end = start + Vec2::new(19.0, -7.0);
        ui.run_with_modifiers(
            vec![
                egui::Event::PointerMoved(start),
                egui::Event::PointerButton {
                    pos: start,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers,
                },
            ],
            modifiers,
        );
        ui.run_with_modifiers(vec![egui::Event::PointerMoved(end)], modifiers);
        ui.run_with_modifiers(vec![egui::Event::PointerMoved(end)], modifiers);
        if cancel {
            ui.run(vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }]);
        }
        ui.run_with_modifiers(
            vec![egui::Event::PointerButton {
                pos: end,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers,
            }],
            modifiers,
        );
        ui.settle();
        if cancel {
            assert_eq!(project_fingerprint(&ui.app.project), before);
        } else {
            let point = ui.app.project.automation_lanes[0].lane.points()[1];
            if modifiers.ctrl {
                assert_eq!(point.position, 2.0);
                assert_ne!(point.value, 0.0);
            }
            if modifiers.shift {
                assert_eq!(point.value, 0.0);
                assert_ne!(point.position, 2.0);
            }
            assert_eq!(ui.app.undo_stack.len(), 1);
            ui.key(egui::Key::Z, command);
            assert_eq!(project_fingerprint(&ui.app.project), before);
        }
    }
}

#[test]
fn automation_point_type_value_escape_redo_and_preceding_edit() {
    let mut ui = automation_point_ui();
    let command = egui::Modifiers {
        ctrl: true,
        command: true,
        ..Default::default()
    };
    let original = project_fingerprint(&ui.app.project);
    let pos = automation_point_position(&ui, 2);
    automation_secondary_click(&mut ui, pos);
    ui.key(egui::Key::Escape, egui::Modifiers::NONE);
    assert_eq!(project_fingerprint(&ui.app.project), original);
    ui.app.project.swing = 0.25;
    let preceding = project_fingerprint(&ui.app.project);
    let pos = automation_point_position(&ui, 2);
    automation_secondary_click(&mut ui, pos);
    let edit = ui
        .nodes
        .iter()
        .find(|node| node.role() == Role::TextInput && node.value() == Some("0.5"))
        .expect("normalized value editor");
    let b = edit.bounds().unwrap();
    ui.click_pos(Pos2::new(
        ((b.x0 + b.x1) / 2.0) as f32,
        ((b.y0 + b.y1) / 2.0) as f32,
    ));
    ui.key(egui::Key::A, command);
    ui.run(vec![egui::Event::Text("NaN".into())]);
    ui.settle();
    assert!(ui.button("Apply value").is_disabled());
    ui.key(egui::Key::A, command);
    ui.run(vec![egui::Event::Text("0.75".into())]);
    ui.click("Apply value");
    assert_eq!(
        ui.app.project.automation_lanes[0].lane.points()[1].value,
        0.5
    );
    let edited = project_fingerprint(&ui.app.project);
    ui.key(egui::Key::Z, command);
    assert_eq!(project_fingerprint(&ui.app.project), preceding);
    ui.key(egui::Key::Z, command);
    assert_eq!(project_fingerprint(&ui.app.project), original);
    ui.key(egui::Key::Y, command);
    ui.key(egui::Key::Y, command);
    assert_eq!(project_fingerprint(&ui.app.project), edited);
    let pos = automation_point_position(&ui, 2);
    automation_secondary_click(&mut ui, pos);
    ui.click("Apply value");
    assert_eq!(
        project_fingerprint(&ui.app.project),
        edited,
        "repeated identical value is a no-op"
    );
    assert_eq!(ui.app.undo_stack.len(), 2);
}

#[test]
fn automation_point_source_offset_snaps_to_playlist_grid() {
    let mut ui = automation_point_ui();
    let clip = &mut ui.app.project.clips[0];
    clip.start = 8.0;
    clip.source_offset = 0.125;
    ui.app.project.automation_lanes[0].lane.replace_points([
        AutomationPoint::new(0.125, -0.5),
        AutomationPoint::new(2.125, 0.0),
        AutomationPoint::new(4.125, 0.5),
    ]);
    let _ = ui.app.playlist_viewport.x.reveal(8.0, 12.0, 0.0);
    ui.app.sync_history_observer();
    ui.settle();
    let left = automation_point_position(&ui, 1);
    let middle = automation_point_position(&ui, 2);
    automation_secondary_click(&mut ui, Pos2::new((left.x + middle.x) / 2.0, middle.y));
    let point = ui.app.project.automation_lanes[0].lane.points()[1];
    assert!(
        (point.position - 1.125).abs() < 0.00001,
        "source offset follows snapped timeline beat"
    );
    assert_eq!(ui.app.project.clips[0].start, 8.0);
}

#[test]
fn automation_point_double_click_is_retained_and_unselected_right_click_is_safe() {
    let mut ui = automation_point_ui();
    let left = automation_point_position(&ui, 1);
    let middle = automation_point_position(&ui, 2);
    let pos = Pos2::new((left.x + middle.x) / 2.0, middle.y);
    for pressed in [true, false, true, false] {
        ui.run(vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
    }
    ui.settle();
    assert_eq!(ui.app.project.automation_lanes[0].lane.points().len(), 4);
    assert_eq!(ui.app.undo_stack.len(), 1);
    ui.app.clear_playlist_selection();
    ui.settle();
    let before = project_fingerprint(&ui.app.project);
    automation_secondary_click(&mut ui, pos);
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert_eq!(ui.app.project.clips.len(), 1);
}

#[test]
fn automation_point_invalid_source_span_has_no_editable_nodes_or_mutation() {
    let mut ui = automation_point_ui();
    let pos = automation_point_position(&ui, 2);
    ui.app.project.clips[0].source_offset = f32::NAN;
    ui.app.sync_history_observer();
    ui.settle();
    let before = project_fingerprint(&ui.app.project);
    assert!(
        !ui.nodes
            .iter()
            .any(|node| node.label() == Some("Automation point 2"))
    );
    automation_secondary_click(&mut ui, pos);
    assert_eq!(project_fingerprint(&ui.app.project), before);
}

fn automation_tension_ui() -> UiHarness {
    let mut ui = automation_point_ui();
    ui.app.project.automation_lanes[0]
        .lane
        .set_curve(AutomationCurve::Tension);
    ui.app.sync_history_observer();
    ui.app.project_fingerprint = project_fingerprint(&ui.app.project);
    ui.app.dirty = false;
    ui.settle();
    ui
}

fn automation_tension_position(ui: &UiHarness, index: usize) -> Option<Pos2> {
    let label = format!("Automation tension {index}");
    let node = ui
        .nodes
        .iter()
        .find(|node| node.label() == Some(label.as_str()))?;
    let bounds = node.bounds()?;
    Some(Pos2::new(
        ((bounds.x0 + bounds.x1) / 2.0) as f32,
        ((bounds.y0 + bounds.y1) / 2.0) as f32,
    ))
}

fn automation_tension_pointer(
    ui: &mut UiHarness,
    pos: Pos2,
    pressed: bool,
    modifiers: egui::Modifiers,
) {
    ui.run_with_modifiers(
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers,
            },
        ],
        modifiers,
    );
}

#[test]
fn automation_tension_drag_reset_and_exact_history() {
    let mut ui = automation_tension_ui();
    let before = project_fingerprint(&ui.app.project);
    let points = ui.app.project.automation_lanes[0].lane.points().to_vec();
    let pos = automation_tension_position(&ui, 1).unwrap();
    let end = pos + Vec2::new(13.0, 20.0);
    automation_tension_pointer(&mut ui, pos, true, egui::Modifiers::NONE);
    ui.run(vec![egui::Event::PointerMoved(end)]);
    ui.run(vec![egui::Event::PointerMoved(end)]);
    automation_tension_pointer(&mut ui, end, false, egui::Modifiers::NONE);
    ui.settle();
    let edited = project_fingerprint(&ui.app.project);
    let lane = &ui.app.project.automation_lanes[0].lane;
    assert!((lane.points()[0].tension - 0.2).abs() < 1e-6);
    assert_eq!(lane.points()[0].position, points[0].position);
    assert_eq!(lane.points()[0].value, points[0].value);
    assert_eq!(&lane.points()[1..], &points[1..]);
    assert_eq!(ui.app.undo_stack.len(), 1);
    assert!(ui.app.dirty);
    let command = egui::Modifiers {
        ctrl: true,
        command: true,
        ..Default::default()
    };
    ui.key(egui::Key::Z, command);
    assert_eq!(project_fingerprint(&ui.app.project), before);
    ui.key(egui::Key::Y, command);
    assert_eq!(project_fingerprint(&ui.app.project), edited);
    let pos = automation_tension_position(&ui, 1).unwrap();
    automation_secondary_click(&mut ui, pos);
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert_eq!(ui.app.project.clips.len(), 1);
    assert_eq!(
        ui.app.project.automation_lanes[0].lane.points().len(),
        points.len()
    );
    assert_eq!(ui.app.undo_stack.len(), 2);
    assert!(
        !ui.nodes
            .iter()
            .any(|node| node.label() == Some("Copy value"))
    );
    ui.key(egui::Key::Z, command);
    assert_eq!(project_fingerprint(&ui.app.project), edited);
}

#[test]
fn automation_tension_cancel_and_noop_preserve_saved_state_and_redo() {
    let mut ui = automation_tension_ui();
    // An existing redo must survive no-op reset and a canceled gesture.
    let mut future = ui.app.project.clone();
    future.swing = 0.25;
    ui.app.redo_stack.push(future);
    let before = project_fingerprint(&ui.app.project);
    let redo = project_fingerprint(&ui.app.redo_stack[0]);
    let pos = automation_tension_position(&ui, 1).unwrap();
    automation_secondary_click(&mut ui, pos);
    assert!(!ui.app.dirty);
    assert!(ui.app.undo_stack.is_empty());
    assert_eq!(ui.app.redo_stack.len(), 1);
    assert_eq!(project_fingerprint(&ui.app.redo_stack[0]), redo);
    automation_tension_pointer(&mut ui, pos, true, egui::Modifiers::NONE);
    let end = pos + Vec2::new(0.0, -20.0);
    ui.run(vec![egui::Event::PointerMoved(end)]);
    ui.run(vec![egui::Event::PointerMoved(end)]);
    assert_ne!(project_fingerprint(&ui.app.project), before);
    ui.key(egui::Key::Escape, egui::Modifiers::NONE);
    automation_tension_pointer(&mut ui, end, false, egui::Modifiers::NONE);
    ui.settle();
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert!(!ui.app.dirty);
    assert!(ui.app.undo_stack.is_empty());
    assert_eq!(ui.app.redo_stack.len(), 1);
    assert_eq!(project_fingerprint(&ui.app.redo_stack[0]), redo);
    // A horizontal-only held gesture is also a no-op.
    let pos = automation_tension_position(&ui, 1).unwrap();
    automation_tension_pointer(&mut ui, pos, true, egui::Modifiers::NONE);
    let end = pos + Vec2::new(30.0, 0.0);
    ui.run(vec![egui::Event::PointerMoved(end)]);
    automation_tension_pointer(&mut ui, end, false, egui::Modifiers::NONE);
    ui.settle();
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert!(!ui.app.dirty);
    assert!(ui.app.undo_stack.is_empty());
    assert_eq!(ui.app.redo_stack.len(), 1);
}

#[test]
fn automation_tension_ctrl_switch_and_outside_release_preserve_preceding_edit() {
    let mut ui = automation_tension_ui();
    let original = project_fingerprint(&ui.app.project);
    ui.app.project.swing = 0.25;
    let preceding = project_fingerprint(&ui.app.project);
    let pos = automation_tension_position(&ui, 1).unwrap();
    automation_tension_pointer(&mut ui, pos, true, egui::Modifiers::NONE);
    let end = pos + Vec2::new(0.0, 20.0);
    ui.run(vec![egui::Event::PointerMoved(end)]);
    let coarse = ui.app.project.automation_lanes[0].lane.points()[0].tension;
    ui.run_with_modifiers(vec![egui::Event::PointerMoved(end)], egui::Modifiers::CTRL);
    assert_eq!(
        ui.app.project.automation_lanes[0].lane.points()[0].tension,
        coarse
    );
    let fine_end = end + Vec2::new(0.0, 10.0);
    ui.run_with_modifiers(
        vec![egui::Event::PointerMoved(fine_end)],
        egui::Modifiers::CTRL,
    );
    assert!(
        (ui.app.project.automation_lanes[0].lane.points()[0].tension - coarse - 0.01).abs() < 1e-6
    );
    let limit = pos + Vec2::new(0.0, 300.0);
    ui.run(vec![egui::Event::PointerMoved(limit)]);
    assert_eq!(
        ui.app.project.automation_lanes[0].lane.points()[0].tension,
        1.0
    );
    let reversed = limit - Vec2::new(0.0, 1.0);
    ui.run(vec![egui::Event::PointerMoved(reversed)]);
    assert!((ui.app.project.automation_lanes[0].lane.points()[0].tension - 0.99).abs() < 1e-6);
    // Leave horizontally, without introducing another vertical edit.
    let outside = Pos2::new(-100.0, reversed.y);
    automation_tension_pointer(&mut ui, outside, false, egui::Modifiers::CTRL);
    ui.settle();
    assert!(ui.app.playlist_gesture_before.is_none());
    assert_eq!(ui.app.undo_stack.len(), 2);
    let edited = project_fingerprint(&ui.app.project);
    let command = egui::Modifiers {
        ctrl: true,
        command: true,
        ..Default::default()
    };
    ui.key(egui::Key::Z, command);
    assert_eq!(project_fingerprint(&ui.app.project), preceding);
    ui.key(egui::Key::Z, command);
    assert_eq!(project_fingerprint(&ui.app.project), original);
    ui.key(egui::Key::Y, command);
    ui.key(egui::Key::Y, command);
    assert_eq!(project_fingerprint(&ui.app.project), edited);
}

#[test]
fn automation_tension_visibility_and_clipped_midpoint_alignment() {
    let mut ui = automation_tension_ui();
    let left = automation_point_position(&ui, 1);
    let right = automation_point_position(&ui, 2);
    let handle = automation_tension_position(&ui, 1).unwrap();
    assert!((handle.x - (left.x + right.x) / 2.0).abs() < 0.01);
    assert!((handle.y - (left.y + right.y) / 2.0).abs() < 0.01);
    for mode in [AutomationCurve::Linear, AutomationCurve::Hold] {
        ui.app.project.automation_lanes[0].lane.set_curve(mode);
        ui.settle();
        assert!(automation_tension_position(&ui, 1).is_none());
    }
    ui.app.project.automation_lanes[0]
        .lane
        .set_curve(AutomationCurve::Tension);
    ui.app.tool_mode = ToolMode::Draw;
    ui.settle();
    assert!(automation_tension_position(&ui, 1).is_none());
    ui.app.tool_mode = ToolMode::Select;
    ui.app.playlist_selection_ids.clear();
    ui.app.selected_clip = None;
    ui.settle();
    assert!(automation_tension_position(&ui, 1).is_none());
    let clip_id = ui.app.project.clips[0].id;
    ui.app.select_playlist_clip_only(clip_id);
    ui.app.project.automation_lanes[0].lane.replace_points([
        AutomationPoint::new(0.0, -1.0),
        AutomationPoint::new(8.0, 1.0),
    ]);
    ui.app.project.clips[0].source_offset = 2.0;
    ui.app.project.clips[0].length = 2.0;
    ui.settle();
    assert!(!ui.nodes.iter().any(|node| {
        node.label()
            .is_some_and(|label| label.starts_with("Automation point "))
    }));
    // The clipped source midpoint is 3, corresponding to Playlist beat 1.
    let clipped = automation_tension_position(&ui, 1).unwrap();
    assert!((clipped.x - handle.x).abs() < 0.01);
    // Scroll past the fixed source midpoint while a tail of the clip remains visible.
    let beat_width = right.x - left.x;
    ui.app
        .playlist_viewport
        .x
        .scroll_by_pixels(f64::from(beat_width * 0.75))
        .unwrap();
    ui.settle();
    assert!(automation_tension_position(&ui, 1).is_none());
    ui.app
        .playlist_viewport
        .x
        .scroll_by_pixels(-f64::from(beat_width * 0.75))
        .unwrap();
    ui.settle();
    let restored = automation_tension_position(&ui, 1).unwrap();
    assert!((restored.x - clipped.x).abs() < 0.01);
    ui.app.project.automation_lanes[0].lane.set_enabled(false);
    ui.settle();
    assert!(automation_tension_position(&ui, 1).is_none());
}
