//! The production keyboard pointer path and its actual audio callback receipts.
//! Native oscillator output here verifies ownership; it is not NOIRE evidence.

use super::piano_keyboard_playback_tests::KeyboardCallbackFixture;
use super::*;
use crate::app::piano_keyboard::PianoKeyboardGeometry;
use crate::audio::{AuditionStatus, AuditionToken};

fn keyboard(fixture: &KeyboardCallbackFixture) -> PianoKeyboardGeometry {
    PianoKeyboardGeometry {
        rect: fixture
            .ui
            .ctx
            .read_response(Id::new("piano-keyboard"))
            .expect("the production keyboard must have actual bounds")
            .rect,
        pitch_axis: fixture.ui.app.piano_viewport.y,
    }
}

fn key_point(fixture: &KeyboardCallbackFixture, pitch: u8) -> Pos2 {
    let geometry = keyboard(fixture);
    let key = geometry.key(pitch);
    let point = if key.black {
        key.face.center()
    } else {
        Pos2::new(key.face.right() - 5.0, key.pitch_row.center().y)
    };
    assert!(
        geometry.rect.contains(point),
        "test pitch {pitch} must be visible: keyboard={:?}, face={:?}, row={:?}, view={:?}, primary_down={}",
        geometry.rect,
        key.face,
        key.pitch_row,
        fixture.ui.app.workspace.focused,
        fixture.ui.ctx.input(|input| input.pointer.primary_down())
    );
    assert_eq!(geometry.pitch_at(point), Some(pitch));
    point
}

fn pointer(fixture: &mut KeyboardCallbackFixture, point: Pos2, pressed: Option<bool>) {
    let mut events = vec![egui::Event::PointerMoved(point)];
    if let Some(pressed) = pressed {
        events.push(egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
    }
    fixture.ui.run(events);
}

fn callback(fixture: &mut KeyboardCallbackFixture, frames: usize) {
    fixture
        .callback
        .pump(fixture.ui.app.audio.as_ref().unwrap(), frames);
}

fn receipt(fixture: &KeyboardCallbackFixture, token: AuditionToken) -> Option<AuditionStatus> {
    fixture
        .ui
        .app
        .audio
        .as_ref()
        .unwrap()
        .audition_status(token)
}

fn output_peak(fixture: &KeyboardCallbackFixture) -> f32 {
    fixture
        .callback
        .last_output()
        .iter()
        .flat_map(|frame| frame.iter())
        .map(|sample| {
            assert!(sample.is_finite(), "audition must produce finite samples");
            sample.abs()
        })
        .fold(0.0, f32::max)
}

fn press(fixture: &mut KeyboardCallbackFixture, pitch: u8) -> AuditionToken {
    let point = key_point(fixture, pitch);
    pointer(fixture, point, Some(true));
    let token = fixture
        .ui
        .app
        .keyboard_held_token()
        .expect("real pointer press must enqueue one owned audition");
    assert_eq!(token.note, pitch);
    assert_eq!(receipt(fixture, token), Some(AuditionStatus::Pending));
    callback(fixture, 128);
    assert_eq!(receipt(fixture, token), Some(AuditionStatus::Accepted));
    assert!(output_peak(fixture) > 0.001);
    fixture.ui.run(Vec::new());
    token
}

#[test]
fn held_keyboard_real_press_sustains_slides_and_releases_without_score_or_history_edits() {
    let mut fixture = KeyboardCallbackFixture::new();
    let original = project_fingerprint(&fixture.ui.app.project);
    let history = (
        fixture.ui.app.undo_stack.len(),
        fixture.ui.app.redo_stack.len(),
    );
    let start = fixture.snapshot();
    let first = press(&mut fixture, 60);

    // At 240 BPM the former quarter-beat preview ended after 3,000 frames.
    // Render more than three times that through the actual paused callback.
    for _ in 0..80 {
        fixture.pump(128);
        assert_eq!(fixture.ui.app.keyboard_held_token(), Some(first));
        assert_eq!(receipt(&fixture, first), Some(AuditionStatus::Accepted));
        assert_eq!(
            fixture.ui.app.keyboard_audition_pending_count(),
            1,
            "holding cannot enqueue another Begin on every UI frame"
        );
    }
    assert!(
        output_peak(&fixture) > 0.001,
        "a physical hold must still sound"
    );
    assert!(!fixture.snapshot().transport_playing);
    assert_eq!(fixture.snapshot().timeline_frame, start.timeline_frame);

    let next_point = key_point(&fixture, 64);
    pointer(&mut fixture, next_point, None);
    let second = fixture.ui.app.keyboard_held_token().unwrap();
    assert_ne!(first.id, second.id);
    assert_eq!(second.note, 64);
    assert_eq!(receipt(&fixture, second), Some(AuditionStatus::Pending));
    callback(&mut fixture, 128);
    assert_eq!(receipt(&fixture, first), Some(AuditionStatus::Released));
    assert_eq!(receipt(&fixture, second), Some(AuditionStatus::Accepted));
    fixture.ui.run(Vec::new());
    assert_eq!(fixture.ui.app.keyboard_audition_pending_count(), 1);

    pointer(&mut fixture, next_point, Some(false));
    assert!(fixture.ui.app.keyboard_held_token().is_none());
    callback(&mut fixture, 128);
    assert_eq!(receipt(&fixture, second), Some(AuditionStatus::Released));
    fixture.ui.run(Vec::new());
    assert_eq!(fixture.ui.app.keyboard_audition_pending_count(), 0);
    for _ in 0..24 {
        fixture.pump(128);
    }
    assert_eq!(
        output_peak(&fixture),
        0.0,
        "released native voice must finish its fade"
    );
    assert_eq!(project_fingerprint(&fixture.ui.app.project), original);
    assert_eq!(
        (
            fixture.ui.app.undo_stack.len(),
            fixture.ui.app.redo_stack.len()
        ),
        history
    );
    assert!(!fixture.ui.app.dirty);
}

#[test]
fn held_keyboard_focus_loss_or_hidden_editor_cancels_without_automatic_resume() {
    for focus_loss in [true, false] {
        let mut fixture = KeyboardCallbackFixture::new();
        let original = project_fingerprint(&fixture.ui.app.project);
        let token = press(&mut fixture, 60);
        let point = key_point(&fixture, 60);
        if focus_loss {
            fixture.ui.time += 0.1;
            let _ = fixture.ui.ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, fixture.ui.size)),
                    time: Some(fixture.ui.time),
                    focused: false,
                    events: vec![egui::Event::WindowFocused(false)],
                    ..Default::default()
                },
                |root| fixture.ui.app.ui(root, &mut fixture.ui.frame),
            );
        } else {
            // The key is already physically down. A second Press event at the
            // toolbar is not a real click; release it before clicking Hide.
            pointer(&mut fixture, point, Some(false));
            fixture.ui.click("Hide editor");
            assert!(
                !fixture.ui.app.workspace.windows[workspace::index(StudioView::PianoRoll)].visible
            );
        }
        assert!(fixture.ui.app.keyboard_held_token().is_none());
        callback(&mut fixture, 128);
        assert_eq!(receipt(&fixture, token), Some(AuditionStatus::Released));
        if focus_loss {
            fixture.ui.run(vec![egui::Event::WindowFocused(true)]);
        } else {
            fixture.ui.key(egui::Key::F7, egui::Modifiers::NONE);
        }
        for _ in 0..4 {
            let restored_keyboard_point = keyboard(&fixture).rect.center();
            pointer(&mut fixture, restored_keyboard_point, None);
            fixture.pump(128);
            assert!(
                fixture.ui.app.keyboard_held_token().is_none(),
                "restoring focus/visibility cannot start a new note without a fresh press"
            );
        }
        pointer(&mut fixture, point, Some(false));
        // Hide changes the focused editor and its available layout. Reopen and
        // settle through the real shortcut after the physical release, then
        // locate the newly displayed key instead of reusing pre-Hide bounds.
        fixture.ui.key(egui::Key::F7, egui::Modifiers::NONE);
        fixture.ui.settle();
        assert_eq!(fixture.ui.app.workspace.focused, StudioView::PianoRoll);
        assert!(fixture.ui.app.workspace.windows[workspace::index(StudioView::PianoRoll)].visible);
        // The reopened floating editor can be shorter than the initial
        // maximized one. Choose a fully displayed white key from its real
        // geometry; this test verifies fresh pointer ownership, not pitch 60
        // remaining inside every restored viewport.
        let geometry = keyboard(&fixture);
        let fresh_pitch = (0_u8..=127)
            .find(|pitch| {
                let key = geometry.key(*pitch);
                let candidate = Pos2::new(key.face.right() - 5.0, key.pitch_row.center().y);
                !key.black
                    && geometry.rect.contains(candidate)
                    && geometry.pitch_at(candidate) == Some(*pitch)
            })
            .expect("reopened production keyboard must display a clickable white key");
        let fresh_point = key_point(&fixture, fresh_pitch);
        let new_token = press(&mut fixture, fresh_pitch);
        assert_ne!(token.id, new_token.id);
        assert_eq!(new_token.note, fresh_pitch);
        assert_eq!(receipt(&fixture, new_token), Some(AuditionStatus::Accepted));
        pointer(&mut fixture, fresh_point, Some(false));
        callback(&mut fixture, 128);
        assert_eq!(receipt(&fixture, new_token), Some(AuditionStatus::Released));
        fixture.ui.run(Vec::new());
        assert!(fixture.ui.app.keyboard_held_token().is_none());
        assert_eq!(fixture.ui.app.keyboard_audition_pending_count(), 0);
        assert_eq!(project_fingerprint(&fixture.ui.app.project), original);
        assert!(fixture.ui.app.undo_stack.is_empty());
        assert!(fixture.ui.app.redo_stack.is_empty());
        assert!(!fixture.ui.app.dirty);
    }
}

#[test]
fn held_keyboard_manual_save_waits_for_owned_release_then_writes_the_real_temp_project() {
    let files = Fixture::new();
    let path = files.0.join("held-keyboard-save.citrus");
    let mut fixture = KeyboardCallbackFixture::new();
    let original = project_fingerprint(&fixture.ui.app.project);
    let token = press(&mut fixture, 60);
    fixture
        .ui
        .app
        .begin_project_save(ProjectSaveRequest::Manual {
            path: path.clone(),
            lifecycle: None,
        });
    assert!(fixture.ui.app.queued_save_request.is_some());
    assert!(fixture.ui.app.save_barrier.is_none());
    assert!(
        !path.exists(),
        "enqueueing End cannot impersonate callback release"
    );
    assert_eq!(receipt(&fixture, token), Some(AuditionStatus::Accepted));
    fixture.ui.run(Vec::new());
    assert!(!path.exists());
    callback(&mut fixture, 128);
    assert_eq!(receipt(&fixture, token), Some(AuditionStatus::Released));
    fixture.until(|fixture| {
        path.exists()
            && fixture.ui.app.saved_path.as_ref() == Some(&path)
            && fixture.ui.app.queued_save_request.is_none()
            && fixture.ui.app.save_barrier.is_none()
    });
    assert_eq!(
        project_fingerprint(&Project::load(&path).unwrap()),
        original
    );
    assert_eq!(fixture.ui.app.keyboard_audition_pending_count(), 0);
    for _ in 0..4 {
        fixture.pump(128);
        assert!(
            fixture.ui.app.keyboard_held_token().is_none(),
            "completing Save cannot resume the interrupted physical hold"
        );
    }
    let point = key_point(&fixture, 60);
    pointer(&mut fixture, point, Some(false));
    let new_token = press(&mut fixture, 60);
    assert_ne!(new_token.id, token.id);
    pointer(&mut fixture, point, Some(false));
    fixture.pump(128);
    assert_eq!(project_fingerprint(&fixture.ui.app.project), original);
    assert!(fixture.ui.app.undo_stack.is_empty());
    assert!(fixture.ui.app.redo_stack.is_empty());
    assert!(!fixture.ui.app.dirty);
}

#[test]
fn held_keyboard_full_command_ring_retains_exact_end_and_retries_outside_the_piano_editor() {
    let mut fixture = KeyboardCallbackFixture::new();
    let token = press(&mut fixture, 60);
    let point = key_point(&fixture, 60);
    let master = fixture.ui.app.master_volume;
    let audio = fixture.ui.app.audio.as_mut().unwrap();
    let mut queued = 0;
    while audio.command(AudioCommand::SetMaster(master)) {
        queued += 1;
        assert!(queued < 65_536, "the command ring must be bounded");
    }
    assert!(queued > 0);
    let full = fixture.snapshot().command_queue_full;
    pointer(&mut fixture, point, Some(false));
    assert!(fixture.ui.app.keyboard_held_token().is_none());
    assert_eq!(
        fixture.ui.app.keyboard_audition_pending_count(),
        1,
        "failed End enqueue must retain the exact receipt/token"
    );
    assert_eq!(receipt(&fixture, token), Some(AuditionStatus::Accepted));
    assert!(fixture.snapshot().command_queue_full > full);
    fixture.ui.app.focus_editor(StudioView::Playlist);

    // This callback drains unrelated controls. The refused End has not yet run.
    callback(&mut fixture, 128);
    assert_eq!(receipt(&fixture, token), Some(AuditionStatus::Accepted));
    fixture.ui.run(Vec::new());
    assert_eq!(fixture.ui.app.workspace.focused, StudioView::Playlist);
    callback(&mut fixture, 128);
    assert_eq!(receipt(&fixture, token), Some(AuditionStatus::Released));
    fixture.ui.run(Vec::new());
    assert_eq!(
        fixture.ui.app.keyboard_audition_pending_count(),
        0,
        "global App tick must retry releases while another editor is visible"
    );
    for _ in 0..24 {
        fixture.pump(128);
    }
    assert_eq!(output_peak(&fixture), 0.0);
}

#[test]
fn callback_timeline_ui_tick_preserves_not_yet_due_preview_release_then_drains_it() {
    let mut fixture = KeyboardCallbackFixture::new();
    assert!(
        fixture.ui.app.callback_timeline_owns_note_clip(),
        "fixture must enter the production CallbackTimeline branch"
    );
    let snapshot = fixture.snapshot();
    let pending = PendingGeneratorNoteOff {
        note_instance_id: 99_001,
        channel_id: fixture.channel_id,
        note: 60,
        transport_epoch: snapshot.transport_epoch,
        due_frame: snapshot.device_frame + 512,
    };
    fixture.ui.app.pending_generator_note_offs.push(pending);
    for _ in 0..3 {
        fixture.ui.run(Vec::new());
        assert_eq!(
            fixture.ui.app.pending_generator_note_offs,
            vec![pending],
            "Timeline-owned Song scheduling must retain independent preview releases"
        );
    }
    for _ in 0..4 {
        fixture.pump(128);
    }
    assert_eq!(fixture.snapshot().transport_epoch, pending.transport_epoch);
    assert!(fixture.snapshot().device_frame >= pending.due_frame);
    assert!(fixture.ui.app.callback_timeline_owns_note_clip());
    assert!(
        fixture.ui.app.pending_generator_note_offs.is_empty(),
        "the same production branch must still advance the due UI release"
    );
    // This fixture has no plug-in endpoint. Emptying the vector is queue admission,
    // not evidence that any plug-in accepted or sounded this compatibility MIDI.
}

#[test]
fn held_keyboard_audio_absence_cannot_resume_an_interrupted_physical_press() {
    let mut fixture = KeyboardCallbackFixture::new();
    let token = press(&mut fixture, 60);
    let point = key_point(&fixture, 60);
    let audio = fixture.ui.app.audio.take().unwrap();
    pointer(&mut fixture, point, None);
    assert!(fixture.ui.app.keyboard_held_token().is_none());
    fixture.ui.app.audio = Some(audio);
    for _ in 0..3 {
        pointer(&mut fixture, point, None);
        fixture.pump(128);
        assert!(
            fixture.ui.app.keyboard_held_token().is_none(),
            "restoring an engine requires a fresh physical key press"
        );
    }
    // The real device-switch path shuts down/clears the replaced engine. This
    // reversible fixture restores it only to prove the App press-resume guard.
    assert!(fixture.ui.app.audio.as_mut().unwrap().end_audition(token));
    callback(&mut fixture, 128);
    pointer(&mut fixture, point, Some(false));
    let fresh = press(&mut fixture, 60);
    assert_ne!(fresh.id, token.id);
    pointer(&mut fixture, point, Some(false));
    fixture.pump(128);
}
