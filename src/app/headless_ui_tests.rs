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
    ui.key(egui::Key::D, command);
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

    for view in [StudioView::Playlist, StudioView::PianoRoll] {
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
            .any(|shape| contains_text(&shape.shape, &format!("{} —", channel_name))),
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
    let node = ui
        .nodes
        .iter()
        .find(|node| {
            node.label() == Some("Pan")
                && node.bounds().is_some_and(|bounds| {
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
            node.label() == Some("Pan")
                && node.bounds().is_some_and(|bounds| {
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
