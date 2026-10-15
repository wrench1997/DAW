//! Real App::ui frames include the ordinary end-of-frame close dispatcher.
//! Callback/source/capture ownership is the existing retained Fixture; no native
//! window, device discovery, file dialog or real plug-in is introduced here.
use super::*;
use crate::app::clip_render_job::ClipRenderPhase;
use eframe::App as _;

pub(super) fn actual_app_frame(
    fixture: &mut Fixture,
    mut input: egui::RawInput,
) -> egui::FullOutput {
    fixture.pump_callback_only();
    let ctx = fixture.ctx();
    input.screen_rect = Some(egui::Rect::from_min_size(
        egui::Pos2::ZERO,
        egui::vec2(1440.0, 900.0),
    ));
    input.time = Some(ctx.input(|input| input.time) + 0.1);
    // Match the established headless App harness: no periodic MIDI discovery.
    fixture.app_mut().midi_input.last_refresh = Instant::now();
    let mut frame = eframe::Frame::_new_kittest();
    ctx.run_ui(input, |ui| fixture.app_mut().ui(ui, &mut frame))
}

fn escape_input() -> egui::RawInput {
    egui::RawInput {
        events: vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: Some(egui::Key::Escape),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
        ..Default::default()
    }
}

#[test]
fn actual_whole_normal_app_frames_keep_capture_then_escape_retains_first_origin_until_ack() {
    let mut fixture = Fixture::new();
    settle_initial_source(&mut fixture);
    let before = fixture.bytes();
    let source = exact_source(&fixture);
    fixture.source_handle().pause_capture();
    let (path, started) = begin(&mut fixture);
    actual_app_frame(&mut fixture, egui::RawInput::default());
    assert!(
        fixture
            .app()
            .whole_song_export_job
            .as_ref()
            .unwrap()
            .first_cancellation
            .is_none(),
        "the first ordinary App frame after destination admission is not an exit request"
    );
    until(&mut fixture, |fixture| {
        fixture.source_handle().capture_count() == 1
    });

    for _ in 0..3 {
        let output = actual_app_frame(&mut fixture, egui::RawInput::default());
        assert!(!has_whole_viewport_command(&output, true));
        let job = fixture.app().whole_song_export_job.as_ref().unwrap();
        assert_eq!(job.phase, WholeExportPhase::Capturing);
        assert!(!job.cancel_requested && job.first_cancellation.is_none());
        assert!(job.actor.owns_pending_capture() && job.pending());
        assert!(job.failure.is_none(), "{:?}", job.failure);
    }
    let stopped_request = fixture.app().audio.as_ref().unwrap().transport_request_id();
    actual_app_frame(&mut fixture, escape_input());
    assert_eq!(
        fixture.app().audio.as_ref().unwrap().transport_request_id(),
        stopped_request
    );
    let expected = WholeExportCancellation {
        source: WholeExportCancelSource::EscapeKey,
        phase: WholeExportPhase::Capturing,
    };
    assert_eq!(
        fixture
            .app()
            .whole_song_export_job
            .as_ref()
            .unwrap()
            .first_cancellation,
        Some(expected)
    );
    assert!(fixture.app().whole_song_export_fence_active());
    assert!(fixture.app_mut().cancel_whole_song_export());
    fixture.app_mut().stop();
    assert_eq!(
        fixture.app().audio.as_ref().unwrap().transport_request_id(),
        stopped_request
    );
    actual_app_frame(&mut fixture, egui::RawInput::default());
    assert_eq!(
        fixture
            .app()
            .whole_song_export_job
            .as_ref()
            .unwrap()
            .first_cancellation,
        Some(expected)
    );
    assert!(!started.load(Ordering::Acquire) && !path.exists());

    fixture.source_handle().release_gates();
    until(&mut fixture, |fixture| {
        !fixture.app().whole_song_export_fence_active()
    });
    let job = fixture.app().whole_song_export_job.as_ref().unwrap();
    assert_eq!(job.phase, WholeExportPhase::Cancelled, "{:?}", job.failure);
    assert_eq!(job.first_cancellation, Some(expected));
    assert!(!job.actor.owns_pending_capture());
    assert!(
        fixture
            .app()
            .toast
            .as_ref()
            .unwrap()
            .0
            .contains("Escape key during Capturing")
    );
    assert_eq!(fixture.bytes(), before);
    assert_eq!(exact_source(&fixture), source);
}

#[test]
fn actual_whole_normal_app_frames_export_real_mock_wav_without_close_intent() {
    let mut fixture = Fixture::new();
    settle_initial_source(&mut fixture);
    let before = fixture.bytes();
    let source = exact_source(&fixture);
    let (path, started) = begin(&mut fixture);
    let deadline = Instant::now() + Duration::from_secs(10);
    while fixture.app().whole_song_export_fence_active() {
        assert!(
            Instant::now() < deadline,
            "{}",
            whole_fixture_diagnostic(&fixture)
        );
        let output = actual_app_frame(&mut fixture, egui::RawInput::default());
        assert!(!has_whole_viewport_command(&output, true));
        assert!(
            fixture
                .app()
                .whole_song_export_job
                .as_ref()
                .unwrap()
                .first_cancellation
                .is_none()
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    let job = fixture.app().whole_song_export_job.as_ref().unwrap();
    assert_eq!(job.phase, WholeExportPhase::Complete, "{:?}", job.failure);
    assert!(job.actor.capture.is_none() && job.render.is_none() && !job.admission.is_busy());
    assert!(started.load(Ordering::Acquire));
    let wav = crate::wav::read_wav(&path).unwrap();
    assert!(wav.frame_count() > 0 && wav.samples.iter().any(|sample| sample.abs() > 0.01));
    assert_eq!(fixture.bytes(), before);
    assert_eq!(exact_source(&fixture), source);
}

#[test]
fn actual_whole_cancel_button_reports_its_capture_phase_and_waits_for_original_ack() {
    let mut fixture = Fixture::new();
    settle_initial_source(&mut fixture);
    fixture.source_handle().pause_capture();
    let (path, started) = begin(&mut fixture);
    until(&mut fixture, |fixture| {
        fixture.source_handle().capture_count() == 1
    });
    let ctx = fixture.ctx();
    for _ in 0..3 {
        actual_app_frame(&mut fixture, egui::RawInput::default());
    }
    let rect = ctx
        .data(|data| data.get_temp::<egui::Rect>(egui::Id::new("whole-export-cancel-button-test")))
        .expect("the production Cancel export button must have rendered");
    assert!(rect.is_finite() && rect.is_positive());
    let pos = rect.center();
    for pressed in [true, false] {
        actual_app_frame(
            &mut fixture,
            egui::RawInput {
                events: vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                ..Default::default()
            },
        );
    }
    let job = fixture.app().whole_song_export_job.as_ref().unwrap();
    assert_eq!(
        job.first_cancellation,
        Some(WholeExportCancellation {
            source: WholeExportCancelSource::CancelButton,
            phase: WholeExportPhase::Capturing,
        })
    );
    assert!(job.pending() && job.actor.owns_pending_capture());
    assert!(!started.load(Ordering::Acquire) && !path.exists());
    fixture.source_handle().release_gates();
    until(&mut fixture, |fixture| {
        !fixture.app().whole_song_export_fence_active()
    });
    assert_eq!(
        fixture.app().whole_song_export_job.as_ref().unwrap().phase,
        WholeExportPhase::Cancelled
    );
}

#[test]
fn actual_whole_transport_stop_is_distinct_from_escape_and_programmatic_cancel() {
    let mut fixture = Fixture::new();
    settle_initial_source(&mut fixture);
    let (path, started) = begin(&mut fixture);
    let stopped_request = fixture.app().audio.as_ref().unwrap().transport_request_id();
    fixture.app_mut().stop();
    assert_eq!(
        fixture.app().audio.as_ref().unwrap().transport_request_id(),
        stopped_request
    );
    fixture.app_mut().drive_whole_song_export();
    let job = fixture.app().whole_song_export_job.as_ref().unwrap();
    assert_eq!(
        job.first_cancellation,
        Some(WholeExportCancellation {
            source: WholeExportCancelSource::TransportStop,
            phase: WholeExportPhase::Stopping,
        })
    );
    assert_eq!(job.phase, WholeExportPhase::Cancelled);
    assert!(!started.load(Ordering::Acquire) && !path.exists());
}

#[test]
fn actual_whole_stop_overrides_a_foreign_pending_play_request_without_hiding_failure() {
    let mut fixture = Fixture::new();
    settle_initial_source(&mut fixture);
    let before = fixture.bytes();
    let source = exact_source(&fixture);
    fixture.source_handle().pause_capture();
    let (path, started) = begin(&mut fixture);
    until(&mut fixture, |fixture| {
        fixture.source_handle().capture_count() == 1
    });
    let original_request = fixture.app().audio.as_ref().unwrap().transport_request_id();
    fixture.app().audio.as_ref().unwrap().set_playing(true);
    let foreign_request = fixture.app().audio.as_ref().unwrap().transport_request_id();
    assert_ne!(foreign_request, original_request);
    // No callback pump: flags/snapshot still look paused, but the foreign Play
    // is pending. Stop must override its actual mailbox identity immediately.
    assert!(
        !fixture
            .app()
            .audio
            .as_ref()
            .unwrap()
            .snapshot()
            .transport_playing
    );
    fixture.app_mut().stop();
    let pause_request = fixture.app().audio.as_ref().unwrap().transport_request_id();
    assert_ne!(pause_request, foreign_request);
    assert_ne!(pause_request, original_request);
    fixture.app_mut().drive_whole_song_export();
    let job = fixture.app().whole_song_export_job.as_ref().unwrap();
    assert!(job.actor.owns_pending_capture() && job.pending());
    assert!(
        job.failure
            .as_deref()
            .unwrap()
            .contains("preparatory stop intent")
    );
    assert_eq!(
        job.first_cancellation,
        Some(WholeExportCancellation {
            source: WholeExportCancelSource::TransportStop,
            phase: WholeExportPhase::Capturing,
        })
    );
    fixture.pump_callback_only();
    assert!(
        !fixture
            .app()
            .audio
            .as_ref()
            .unwrap()
            .snapshot()
            .transport_playing
    );
    assert_eq!(
        fixture
            .app()
            .audio
            .as_ref()
            .unwrap()
            .snapshot()
            .applied_transport_request,
        pause_request
    );
    fixture.source_handle().release_gates();
    until(&mut fixture, |fixture| {
        !fixture.app().whole_song_export_fence_active()
    });
    let job = fixture.app().whole_song_export_job.as_ref().unwrap();
    assert_eq!(job.phase, WholeExportPhase::Failed);
    assert!(
        job.failure
            .as_deref()
            .unwrap()
            .contains("preparatory stop intent")
    );
    assert!(!started.load(Ordering::Acquire) && !path.exists());
    assert_eq!(fixture.bytes(), before);
    assert_eq!(exact_source(&fixture), source);
}

#[test]
fn actual_whole_nonignored_late_parameter_effects_report_receipt_not_user_cancel() {
    for variant in 0..3 {
        let mut fixture = Fixture::new();
        settle_initial_source(&mut fixture);
        let route = authoritative_whole_parameter_route(&mut fixture);
        let before = fixture.bytes();
        let source = exact_source(&fixture);
        let (path, started) = begin(&mut fixture);
        fixture
            .app_mut()
            .apply_plugin_parameter_edit_effect(ParameterEditEffect::Ignored);
        assert!(
            fixture
                .app()
                .whole_song_export_job
                .as_ref()
                .unwrap()
                .first_cancellation
                .is_none()
        );
        let submission = crate::plugin_parameter_edit::ParameterEditSubmission {
            edit_id: crate::plugin_parameter_edit::ParameterEditId(991),
            route,
            normalized: 0.5,
        };
        let effect = match variant {
            0 => ParameterEditEffect::SubmitNext(submission),
            1 => ParameterEditEffect::CommitBase {
                submission,
                effective_normalized: 0.5,
            },
            _ => ParameterEditEffect::Failed(crate::plugin_parameter_edit::ParameterEditFailure {
                submission,
                reason: crate::plugin_parameter_edit::ParameterEditFailureReason::TimedOut,
                // Model a queued edit's late timeout, not an unqueued draft
                // discarded by fail-all (which would have was_pending=false).
                was_pending: true,
            }),
        };
        fixture.app_mut().apply_plugin_parameter_edit_effect(effect);
        fixture.app_mut().drive_whole_song_export();
        let job = fixture.app().whole_song_export_job.as_ref().unwrap();
        assert_eq!(
            job.first_cancellation,
            Some(WholeExportCancellation {
                source: WholeExportCancelSource::LateParameterReceipt,
                phase: WholeExportPhase::Stopping,
            })
        );
        assert_eq!(job.phase, WholeExportPhase::Cancelled);
        assert!(
            fixture
                .app()
                .toast
                .as_ref()
                .unwrap()
                .0
                .contains("late parameter receipt during Stopping")
        );
        assert!(!started.load(Ordering::Acquire) && !path.exists());
        assert_eq!(fixture.bytes(), before);
        assert_eq!(exact_source(&fixture), source);
    }
}

#[test]
fn actual_whole_late_audio_asset_result_reports_asset_not_user_cancel() {
    let mut fixture = Fixture::new();
    settle_initial_source(&mut fixture);
    let before = fixture.bytes();
    let (path, started) = begin(&mut fixture);
    let app = fixture.app_mut();
    app.audio_asset_sender
        .send(Err(AudioAssetLoadFailure {
            generation: app.audio_asset_generation,
            asset_id: 991,
            message: "test delayed decode failure".into(),
        }))
        .unwrap();
    app.poll_audio_asset_loads();
    app.drive_whole_song_export();
    let job = app.whole_song_export_job.as_ref().unwrap();
    assert_eq!(
        job.first_cancellation,
        Some(WholeExportCancellation {
            source: WholeExportCancelSource::LateAudioAssetResult,
            phase: WholeExportPhase::Stopping,
        })
    );
    assert_eq!(job.phase, WholeExportPhase::Cancelled);
    assert!(
        app.toast
            .as_ref()
            .unwrap()
            .0
            .contains("late audio asset result during Stopping")
    );
    assert!(!started.load(Ordering::Acquire) && !path.exists());
    assert_eq!(fixture.bytes(), before);
}

#[test]
fn actual_selected_clip_normal_app_frames_keep_capture_until_explicit_escape() {
    let mut fixture = Fixture::new();
    settle_initial_source(&mut fixture);
    let before = fixture.bytes();
    let source = exact_source(&fixture);
    fixture.source_handle().pause_capture();
    fixture.begin();
    until(&mut fixture, |fixture| {
        fixture.source_handle().capture_count() == 1
    });
    for _ in 0..3 {
        actual_app_frame(&mut fixture, egui::RawInput::default());
        assert_eq!(
            fixture.app().clip_render_job.as_ref().unwrap().phase(),
            ClipRenderPhase::Capturing
        );
    }
    let stopped_request = fixture.app().audio.as_ref().unwrap().transport_request_id();
    actual_app_frame(&mut fixture, escape_input());
    fixture.app_mut().stop();
    assert_eq!(
        fixture.app().audio.as_ref().unwrap().transport_request_id(),
        stopped_request
    );
    assert!(fixture.app().clip_render_fence_active());
    fixture.source_handle().release_gates();
    until(&mut fixture, |fixture| {
        !fixture.app().clip_render_fence_active()
    });
    assert_eq!(
        fixture.app().clip_render_job.as_ref().unwrap().phase(),
        ClipRenderPhase::Cancelled
    );
    assert_eq!(fixture.bytes(), before);
    assert_eq!(exact_source(&fixture), source);
}
