//! Production App::ui events, history, and narrow-window regression coverage.
use super::*;
use crate::sampler::SamplerConfig;

fn sampler_fixture(ui: &mut UiHarness) {
    let channel = &mut ui.app.project.channels[0];
    channel.instrument_plugin_instance_id = None;
    channel.sampler = Some(SamplerConfig::new(991));
    let id = channel.id;
    ui.app.project.audio_assets.push(AudioAsset {
        id: 991,
        name: "Melody.wav".into(),
        path: PathBuf::from("Melody.wav"),
        sample_rate: 48000,
        channels: 1,
        bits_per_sample: 16,
        frames: 48000,
        waveform_peaks: vec![0.25, 0.75, 0.5, 0.1],
    });
    ui.app.sync_history_observer();
    ui.app.undo_stack.clear();
    ui.app.redo_stack.clear();
    ui.app.open_sampler(id);
    ui.settle();
}

#[test]
fn sampler_editor_real_input_loop_remove_undo_and_narrow_bounds() {
    for width in [760.0, 1080.0, 1440.0] {
        let mut ui = UiHarness::with_storage(None, false);
        ui.size = Vec2::new(width, 720.0);
        sampler_fixture(&mut ui);
        let bounds = node_rect(ui.button("Load WAV…"));
        assert!(ui.ctx.content_rect().contains_rect(bounds));
        assert!(
            ui.button("Preview (short note)").is_disabled(),
            "offline cannot claim an audible preview"
        );
        let toggle = ui
            .nodes
            .iter()
            .find(|n| n.role() == Role::CheckBox && n.label() == Some("Loop while held"))
            .unwrap();
        ui.click_pos(node_rect(toggle).center());
        assert_eq!(
            ui.app.project.channels[0].sampler.unwrap().loop_start_frame,
            Some(0)
        );
        assert_eq!(ui.app.undo_stack.len(), 1);
        ui.app.undo();
        ui.settle();
        assert_eq!(
            ui.app.project.channels[0].sampler.unwrap().loop_start_frame,
            None
        );
        ui.app.redo();
        ui.settle();
        assert_eq!(
            ui.app.project.channels[0].sampler.unwrap().loop_end_frame,
            Some(48000)
        );
        ui.click("Remove sample");
        assert_eq!(ui.app.project.channels[0].sampler.unwrap().asset_id, None);
        ui.app.undo();
        ui.settle();
        assert!(ui.app.project.channels[0].sampler.is_some());
    }
}

#[test]
fn sampler_editor_slider_drag_commits_exactly_one_undo() {
    let mut ui = UiHarness::with_storage(None, false);
    sampler_fixture(&mut ui);
    let scroll_position = ui.app.sampler_drop_rect().unwrap().center();
    ui.run(vec![
        egui::Event::PointerMoved(scroll_position),
        egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            phase: egui::TouchPhase::Move,
            delta: Vec2::new(0.0, -600.0),
            modifiers: egui::Modifiers::NONE,
        },
    ]);
    ui.settle();
    let slider = ui
        .nodes
        .iter()
        .find(|n| n.role() == Role::Slider && n.label() == Some("Sustain"))
        .unwrap();
    assert!(!slider.is_disabled(), "sampler control must be editable");
    let bounds = node_rect(slider);
    assert!(
        ui.ctx.content_rect().contains_rect(bounds),
        "Sustain slider must be inside viewport: {bounds:?}"
    );
    let start = Pos2::new(bounds.left() + bounds.width() * 0.3, bounds.center().y);
    ui.run(vec![
        egui::Event::PointerMoved(start),
        egui::Event::PointerButton {
            pos: start,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        },
    ]);
    assert!(
        ui.app.sampler_gesture_active(),
        "real press must own a sampler draft; bounds={bounds:?}, value={:?}",
        ui.app.project.channels[0].sampler
    );
    for fraction in [0.4, 0.5, 0.6] {
        ui.run(vec![egui::Event::PointerMoved(Pos2::new(
            bounds.left() + bounds.width() * fraction,
            bounds.center().y,
        ))]);
        assert!(
            ui.app.undo_stack.is_empty(),
            "draft never enters Project mid-drag"
        );
    }
    let end = Pos2::new(bounds.left() + bounds.width() * 0.6, bounds.center().y);
    ui.run(vec![egui::Event::PointerButton {
        pos: end,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    }]);
    ui.settle();
    assert_eq!(ui.app.undo_stack.len(), 1);
    assert_ne!(ui.app.project.channels[0].sampler.unwrap().sustain, 0.8);
    ui.app.undo();
    assert_eq!(ui.app.project.channels[0].sampler.unwrap().sustain, 0.8);
}

#[test]
fn sampler_only_pattern_retains_media_and_runtime_dependency_without_audio_clip() {
    let mut ui = UiHarness::with_storage(None, false);
    sampler_fixture(&mut ui);
    let project = transport_playback_project(&ui.app.project, TransportMode::Pattern);
    assert!(project.audio_assets.iter().any(|a| a.id == 991));
    let map = TempoMap::from_project(&project, 48000).unwrap();
    let timeline = CompiledTimeline::from_project(
        &project,
        &map,
        transport_timeline_compile_options(&project, TransportMode::Pattern),
    )
    .unwrap();
    assert!(timeline.audio_clips().is_empty());
    assert!(timeline_required_audio_asset_ids(&timeline).contains(&991));
}

#[test]
fn sampler_new_channel_uses_real_rack_button_and_is_undoable() {
    let mut ui = UiHarness::with_storage(None, true);
    ui.app.focus_editor(StudioView::ChannelRack);
    let count = ui.app.project.channels.len();
    ui.app.sync_history_observer();
    ui.app.undo_stack.clear();
    ui.settle();
    ui.click("+ Sampler channel");
    assert_eq!(ui.app.project.channels.len(), count + 1);
    assert_eq!(ui.app.selected_channel, count);
    assert_eq!(
        ui.app.project.channels[count].sampler,
        Some(SamplerConfig::empty())
    );
    assert!(
        ui.app
            .project
            .patterns
            .iter()
            .all(|p| p.channel_steps.len() == count + 1 && p.channel_steps[count] == [false; 16])
    );
    assert!(ui.button("Load WAV…").bounds().is_some());
    assert_eq!(ui.app.undo_stack.len(), 1);
    ui.app.undo();
    assert_eq!(ui.app.project.channels.len(), count);
}

#[test]
fn sampler_drop_only_accepts_its_explicit_region_and_keeps_playlist_unchanged() {
    let mut ui = UiHarness::with_storage(None, false);
    let channel = ui.app.project.channels[0].id;
    ui.app.open_sampler(channel);
    ui.settle();
    let target = ui.app.sampler_drop_rect().unwrap();
    let run_drop = |ui: &mut UiHarness, position: Pos2| {
        ui.time += 0.1;
        ui.app.midi_input.last_refresh = Instant::now();
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, ui.size)),
            time: Some(ui.time),
            events: vec![egui::Event::PointerMoved(position)],
            dropped_files: vec![egui::DroppedFile {
                path: Some(PathBuf::from("/missing/sampler-drop-fixture.wav")),
                ..Default::default()
            }],
            ..Default::default()
        };
        let _ = ui
            .ctx
            .run_ui(input, |root_ui| ui.app.ui(root_ui, &mut ui.frame));
    };
    let before = project_fingerprint(&ui.app.project);
    run_drop(&mut ui, target.max + Vec2::new(20.0, 20.0));
    ui.settle();
    assert_eq!(project_fingerprint(&ui.app.project), before);
    run_drop(&mut ui, target.center());
    // The real background decoder rejects this missing path; no Playlist clip or fake
    // sampler may appear while waiting or after the completion is delivered.
    for _ in 0..20 {
        ui.run(Vec::new());
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert!(ui.nodes.iter().any(|node| {
        node.value()
            .is_some_and(|s| s.contains("WAV import failed"))
            || node
                .label()
                .is_some_and(|s| s.contains("WAV import failed"))
    }));
}

#[test]
fn sampler_window_controls_remain_inside_short_viewport() {
    let mut ui = UiHarness::with_storage(None, false);
    ui.size = Vec2::new(480.0, 420.0);
    sampler_fixture(&mut ui);
    ui.capture("sampler-short-before-scroll");
    assert!(
        ui.ctx
            .content_rect()
            .contains_rect(node_rect(ui.button("Load WAV…")))
    );
    assert!(
        ui.ctx
            .content_rect()
            .contains_rect(node_rect(ui.button("Remove sample")))
    );
    let scroll_position = ui.app.sampler_drop_rect().unwrap().center();
    ui.run(vec![egui::Event::PointerMoved(scroll_position)]);
    assert_eq!(
        ui.ctx.layer_id_at(scroll_position),
        Some(egui::LayerId::new(
            egui::Order::Foreground,
            Id::new("native-sampler-editor")
        )),
        "workspace restacking must not cover the sampler"
    );
    ui.run(vec![
        egui::Event::PointerMoved(scroll_position),
        egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            phase: egui::TouchPhase::Move,
            delta: Vec2::new(0.0, -600.0),
            modifiers: egui::Modifiers::NONE,
        },
    ]);
    for _ in 0..12 {
        ui.run(Vec::new());
    }
    ui.capture("sampler-short-after-scroll");
    let preview = node_rect(ui.button("Preview (short note)"));
    assert!(
        ui.ctx.content_rect().contains_rect(preview),
        "short windows must scroll to the audition control: preview={preview:?}, viewport={:?}, scroll_position={scroll_position:?}",
        ui.ctx.content_rect()
    );
}

#[test]
fn sampler_rack_step_actual_click_requests_root_pitch_and_unscaled_velocity() {
    let mut ui = UiHarness::with_storage(None, true);
    ui.app.focus_editor(StudioView::ChannelRack);
    let mut sampler = SamplerConfig::new(991);
    sampler.root_note = 67;
    ui.app.project.channels[0].sampler = Some(sampler);
    ui.app.project.channels[0].volume = 0.25;
    ui.app.project.channels[0].instrument_plugin_instance_id = None;
    let id = ui.app.project.channels[0].id;
    let name = ui.app.project.channels[0].name.clone();
    let pattern = ui.app.project.active_pattern;
    ui.app.project.patterns[pattern].channel_steps[0][0] = false;
    ui.settle();
    ui.click(&format!("{name} step 1"));
    assert_eq!(ui.app.requested_audition_note(), Some((id, 67, 1.0)));
}

#[test]
fn sampler_foreground_yields_to_dirty_new_project_modal_and_returns_after_cancel() {
    let mut ui = UiHarness::with_storage(None, false);
    sampler_fixture(&mut ui);
    let toggle = ui
        .nodes
        .iter()
        .find(|n| n.role() == Role::CheckBox && n.label() == Some("Loop while held"))
        .unwrap();
    ui.click_pos(node_rect(toggle).center());
    assert!(ui.app.dirty);
    let before = project_fingerprint(&ui.app.project);
    ui.key(
        egui::Key::N,
        egui::Modifiers {
            ctrl: true,
            command: true,
            ..Default::default()
        },
    );
    assert!(ui.app.project_lifecycle.is_modal());
    assert!(
        !ui.nodes
            .iter()
            .any(|node| node.label() == Some("Load WAV…")),
        "instrument window must be absent behind exclusive project dialogs"
    );
    assert!(!ui.button("CANCEL").is_disabled());
    ui.click("CANCEL");
    assert!(ui.app.project_lifecycle.is_idle());
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert!(!ui.button("Load WAV…").is_disabled());
}

#[test]
fn sampler_offline_cold_reopen_restores_waveform_without_audio_registration_or_history() {
    let fixture = Fixture::new();
    let path = fixture.0.join("sampler.citrus");
    let mut source = UiHarness::with_storage(None, false);
    sampler_fixture(&mut source);
    let asset = source
        .app
        .project
        .audio_assets
        .iter_mut()
        .find(|a| a.id == 991)
        .unwrap();
    asset.path = fixture.0.join("valid.wav");
    asset.frames = 2;
    source.app.project.channels[0]
        .sampler
        .as_mut()
        .unwrap()
        .root_note = 64;
    source.app.project.channels[0]
        .sampler
        .as_mut()
        .unwrap()
        .sustain = 0.25;
    source.app.project.save(&path).unwrap();
    let expected = source.app.project.channels[0].sampler;
    drop(source);

    let mut reopened = UiHarness::with_storage(None, false);
    assert!(reopened.app.audio.is_none());
    assert!(
        reopened
            .app
            .install_project(Project::load(&path).unwrap(), Some(path), false)
    );
    assert!(
        reopened
            .app
            .project
            .audio_assets
            .iter()
            .all(|a| a.waveform_peaks.is_empty())
    );
    assert_eq!(reopened.app.project.channels[0].sampler, expected);
    let before = project_fingerprint(&reopened.app.project);
    let history = reopened.app.undo_stack.len();
    reopened
        .app
        .open_sampler(reopened.app.project.channels[0].id);
    reopened.wait_until(|app| app.sampler_waveform_preview().is_some());
    let peaks = reopened.app.sampler_waveform_preview().unwrap().unwrap();
    assert_eq!(peaks.len(), 2);
    assert!(peaks.iter().all(|peak| *peak > 0.03));
    assert_eq!(project_fingerprint(&reopened.app.project), before);
    assert_eq!(reopened.app.undo_stack.len(), history);
    assert!(!reopened.app.dirty);
    assert!(reopened.app.loaded_audio_assets.is_empty());
    assert!(reopened.app.prepared_audio_asset_registrations.is_empty());
    assert!(reopened.app.pending_audio_asset_registrations.is_empty());
    assert!(reopened.button("Preview (short note)").is_disabled());
}

#[test]
fn sampler_offline_waveform_failure_is_visible_and_does_not_modify_project() {
    let fixture = Fixture::new();
    let mut ui = UiHarness::with_storage(None, false);
    sampler_fixture(&mut ui);
    let asset = ui
        .app
        .project
        .audio_assets
        .iter_mut()
        .find(|a| a.id == 991)
        .unwrap();
    asset.waveform_peaks.clear();
    asset.path = fixture.0.join("invalid.wav");
    let before = project_fingerprint(&ui.app.project);
    ui.wait_until(|app| app.sampler_waveform_preview().is_some());
    assert!(ui.app.sampler_waveform_preview().unwrap().is_err());
    assert!(!ui.button("Retry waveform").is_disabled());
    for _ in 0..6 {
        ui.run(Vec::new());
    }
    assert!(ui.app.sampler_waveform_preview().unwrap().is_err());
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert!(ui.app.loaded_audio_assets.is_empty());
}

#[test]
fn sampler_remove_keeps_silence_and_native_synth_requires_explicit_undoable_action() {
    let mut ui = UiHarness::with_storage(None, false);
    sampler_fixture(&mut ui);
    let loaded = ui.app.project.channels[0].sampler;
    ui.click("Remove sample");
    assert_eq!(
        ui.app.project.channels[0].sampler,
        Some(SamplerConfig::empty())
    );
    assert!(ui.button("Remove sample").is_disabled());
    assert!(ui.button("Preview (short note)").is_disabled());
    assert!(ui.app.sampler_waveform_preview().is_none());
    assert_eq!(ui.app.undo_stack.len(), 1);
    ui.click("Use native synth");
    assert!(ui.app.project.channels[0].sampler.is_none());
    assert_eq!(ui.app.undo_stack.len(), 2);
    ui.app.undo();
    ui.settle();
    assert_eq!(
        ui.app.project.channels[0].sampler,
        Some(SamplerConfig::empty())
    );
    ui.app.undo();
    ui.settle();
    assert_eq!(ui.app.project.channels[0].sampler, loaded);
}

#[test]
fn sampler_foreground_yields_to_export_and_settings_then_returns_unchanged() {
    let mut ui = UiHarness::with_storage(None, false);
    sampler_fixture(&mut ui);
    let before = project_fingerprint(&ui.app.project);
    ui.menu("Export WAV…");
    assert!(ui.app.export_dialog.is_open());
    assert!(
        !ui.nodes
            .iter()
            .any(|node| node.label() == Some("Load WAV…"))
    );
    assert!(!ui.button("Close").is_disabled());
    ui.click("Close");
    assert!(!ui.app.export_dialog.is_open());
    assert!(!ui.button("Load WAV…").is_disabled());
    ui.key(egui::Key::F10, egui::Modifiers::NONE);
    assert!(ui.app.show_settings);
    assert!(
        !ui.nodes
            .iter()
            .any(|node| node.label() == Some("Load WAV…"))
    );
    ui.key(egui::Key::F10, egui::Modifiers::NONE);
    assert!(!ui.app.show_settings);
    assert!(!ui.button("Load WAV…").is_disabled());
    assert_eq!(project_fingerprint(&ui.app.project), before);
}

#[test]
fn sampler_open_and_scroll_preserve_exact_saved_envelope_and_full_end() {
    for seconds in [[10.0, 60.0, 20.0], [0.0001, 0.0001, 0.0001]] {
        let mut ui = UiHarness::with_storage(None, false);
        ui.size = Vec2::new(480.0, 420.0);
        sampler_fixture(&mut ui);
        let value = ui.app.project.channels[0].sampler.as_mut().unwrap();
        value.attack_seconds = seconds[0];
        value.decay_seconds = seconds[1];
        value.release_seconds = seconds[2];
        value.end_frame = Some(48000);
        value.loop_start_frame = Some(0);
        value.loop_end_frame = Some(48000);
        let expected = *value;
        ui.app.sync_history_observer();
        ui.app.undo_stack.clear();
        ui.app.mark_saved();
        assert!(!ui.app.project_is_dirty_now());
        let before = project_fingerprint(&ui.app.project);
        ui.app.open_sampler(ui.app.project.channels[0].id);
        ui.settle();
        let position = ui.app.sampler_drop_rect().unwrap().center();
        ui.run(vec![egui::Event::PointerMoved(position)]);
        ui.run(vec![egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            phase: egui::TouchPhase::Move,
            delta: Vec2::new(0.0, -600.0),
            modifiers: egui::Modifiers::NONE,
        }]);
        for _ in 0..12 {
            ui.run(Vec::new());
        }
        assert!(
            ui.ctx
                .content_rect()
                .contains_rect(node_rect(ui.button("Preview (short note)")))
        );
        let close = ui
            .nodes
            .iter()
            .find(|node| {
                node.label() == Some("Close window")
                    && node.bounds().is_some_and(|_| {
                        ui.ctx.layer_id_at(node_rect(node).center())
                            == Some(egui::LayerId::new(
                                egui::Order::Foreground,
                                Id::new("native-sampler-editor"),
                            ))
                    })
            })
            .expect("Sampler close button must be accessible");
        ui.click_pos(node_rect(close).center());
        ui.settle();
        assert!(
            !ui.nodes
                .iter()
                .any(|node| node.label() == Some("Load WAV…"))
        );
        assert!(!ui.app.project_is_dirty_now());
        assert_eq!(ui.app.project.channels[0].sampler, Some(expected));
        assert_eq!(project_fingerprint(&ui.app.project), before);
        assert!(ui.app.undo_stack.is_empty());
    }
}
