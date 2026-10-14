//! Real keyboard ownership and callback PCM after a driven MasterPan pauses.
//! This is a native, device-free regression, not evidence of NOIRE audition.

use super::piano_keyboard_playback_tests::KeyboardCallbackFixture;
use super::*;
use crate::app::piano_keyboard::PianoKeyboardGeometry;
use crate::audio::{AuditionDestination, AuditionStatus, AuditionToken};

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

fn assert_direction(fixture: &KeyboardCallbackFixture, pan: f32, phase: &str) {
    let mut peaks = [0.0_f32; 2];
    for frame in fixture.callback.last_output() {
        for (peak, sample) in peaks.iter_mut().zip(frame) {
            assert!(sample.is_finite(), "callback PCM must remain finite");
            *peak = peak.max(sample.abs());
        }
    }
    let audible = usize::from(pan > 0.0);
    assert!(
        peaks[audible] > 0.001,
        "phase={phase}, pan={pan}, peaks={peaks:?}, snapshot={:?}",
        fixture.snapshot()
    );
    assert_eq!(
        peaks[1 - audible],
        0.0,
        "hard pan must silence the opposite channel: phase={phase}, pan={pan}, peaks={peaks:?}"
    );
}

fn visible_white_key(fixture: &KeyboardCallbackFixture) -> (u8, Pos2) {
    let geometry = PianoKeyboardGeometry {
        rect: fixture
            .ui
            .ctx
            .read_response(Id::new("piano-keyboard"))
            .expect("the production keyboard must have actual bounds")
            .rect,
        pitch_axis: fixture.ui.app.piano_viewport.y,
    };
    (0_u8..=127)
        .find_map(|pitch| {
            let key = geometry.key(pitch);
            let point = Pos2::new(key.face.right() - 5.0, key.pitch_row.center().y);
            (!key.black && geometry.rect.contains(point) && geometry.pitch_at(point) == Some(pitch))
                .then_some((pitch, point))
        })
        .expect("one actual white key must be visible")
}

fn pointer(fixture: &mut KeyboardCallbackFixture, point: Pos2, pressed: bool) {
    fixture.ui.run(vec![
        egui::Event::PointerMoved(point),
        egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        },
    ]);
}

fn start(fixture: &mut KeyboardCallbackFixture) {
    fixture.ui.key(egui::Key::Space, egui::Modifiers::NONE);
    assert!(fixture.ui.app.playing);
    fixture.until(|fixture| {
        fixture.snapshot().transport_playing
            && fixture.ui.app.timeline_audio_matches_desired()
            && fixture
                .ui
                .app
                .piano_keyboard_activation
                .is_some_and(|activation| activation.4 == fixture.snapshot().transport_epoch)
    });
}

fn advance_to_frame(fixture: &mut KeyboardCallbackFixture, frame: u64) {
    assert!(fixture.snapshot().transport_playing);
    assert!(fixture.snapshot().timeline_frame <= frame);
    let loop_count = fixture.snapshot().transport_loop_count;
    while fixture.snapshot().timeline_frame < frame {
        let remaining = frame - fixture.snapshot().timeline_frame;
        fixture.pump(remaining.min(128) as usize);
        assert_eq!(fixture.snapshot().transport_loop_count, loop_count);
    }
}

fn paused_owned_audition_uses_manual_pan(manual_pan: f32) {
    let mut fixture = KeyboardCallbackFixture::new();
    let automated_pan = -manual_pan;
    // Pause ends the original Timeline voice. The production resume chase
    // deliberately does not replay a pre-pause Note On, so a future scored
    // onset must provide the PCM used to check the resumed control boundary.
    fixture
        .ui
        .app
        .project
        .active_pattern_mut()
        .notes
        .push(PianoNote {
            id: 91_003,
            channel_id: Some(fixture.channel_id),
            group_id: None,
            note: 67,
            start: 0.375,
            length: 0.5,
            velocity: 0.5,
            selected: false,
            muted: false,
        });
    let pattern_id = fixture.ui.app.project.active_pattern().id;
    fixture.ui.app.project.song_length_beats = 4.0;
    fixture.ui.app.project.clips = vec![Clip {
        id: 92_001,
        track: 0,
        start: 0.0,
        length: 4.0,
        name: "Callback pan and keyboard source".into(),
        color: [255, 142, 82],
        kind: ClipKind::Pattern,
        group_id: None,
        pattern_id,
        automation_id: None,
        audio_asset_id: None,
        source_offset: 0.0,
        audio_source_offset_frame: None,
        audio_source_reference: None,
        audio_length_reference: None,
        gain: 1.0,
        fade_in: 0.0,
        fade_out: 0.0,
        fade_in_reference: None,
        fade_out_reference: None,
        muted: false,
    }];
    let mut lane = AutomationLane::new(AutomationTarget::MasterPan);
    lane.set_curve(AutomationCurve::Hold);
    lane.insert_point(AutomationPoint::new(0.0, f64::from(automated_pan)))
        .unwrap();
    lane.insert_point(AutomationPoint::new(0.5, f64::from(manual_pan)))
        .unwrap();
    fixture.ui.app.project.automation_lanes = vec![ProjectAutomation {
        id: 92_002,
        name: "Master pan changes at half a beat".into(),
        lane,
    }];
    fixture.ui.app.set_transport_mode(TransportMode::Song);
    fixture.ui.app.sync_history_observer();
    fixture.ui.app.project_fingerprint = project_fingerprint(&fixture.ui.app.project);
    fixture.ui.app.undo_stack.clear();
    fixture.ui.app.redo_stack.clear();
    fixture.ui.app.dirty = false;
    fixture.ui.app.request_immediate_timeline_compile();
    fixture.until(|fixture| {
        fixture.ui.app.timeline_audio_matches_desired()
            && fixture.ui.app.piano_keyboard_activation.is_some()
            && fixture
                .ui
                .app
                .timeline_audio_source
                .as_ref()
                .is_some_and(|source| {
                    source.timeline.driven_automation_targets()
                        == [CompiledAutomationTarget::MasterPan]
                })
    });
    let original = project_fingerprint(&fixture.ui.app.project);
    let original_score = serde_json::to_string(&fixture.ui.app.project.patterns).unwrap();
    let history = (
        fixture.ui.app.undo_stack.len(),
        fixture.ui.app.redo_stack.len(),
    );
    let map = &fixture
        .ui
        .app
        .timeline_audio_source
        .as_ref()
        .unwrap()
        .tempo_map;
    let first_on = map.beat_to_frame(0.125).unwrap();
    let resumed_on = map.beat_to_frame(0.375).unwrap();
    let pan_change = map.beat_to_frame(0.5).unwrap();

    // Confirm a valid callback binding with actual music PCM, rather than only
    // an enqueue result or the compiler's target manifest.
    start(&mut fixture);
    advance_to_frame(&mut fixture, first_on + 256);
    assert_direction(&fixture, automated_pan, "original song Note On");
    assert_eq!(fixture.snapshot().timeline_automation_pending, 0);
    assert!(fixture.ui.app.callback_timeline_owns_note_clip());
    fixture.ui.key(egui::Key::Space, egui::Modifiers::NONE);
    assert!(!fixture.ui.app.playing);
    fixture.until(|fixture| {
        !fixture.snapshot().transport_playing && fixture.ui.app.timeline_audio_matches_desired()
    });
    let paused = fixture.snapshot();
    let identity = fixture
        .ui
        .app
        .audio
        .as_ref()
        .unwrap()
        .confirmed_timeline_identity();
    assert!(identity.is_some());
    assert!(paused.timeline_frame < resumed_on);
    assert!(paused.timeline_frame < pan_change);

    assert!(
        fixture
            .ui
            .app
            .audio
            .as_mut()
            .unwrap()
            .command(AudioCommand::SetMasterPan(manual_pan))
    );
    // Force a fresh production legacy-evaluator pass too: a driven callback
    // must retain ownership instead of overwriting the paused manual control.
    fixture.ui.app.automation_evaluator.reset();
    let (pitch, point) = visible_white_key(&fixture);
    pointer(&mut fixture, point, true);
    let token = fixture.ui.app.keyboard_held_token().unwrap();
    assert_eq!(token.note, pitch);
    assert_eq!(token.transport_epoch, paused.transport_epoch);
    assert!(matches!(
        token.destination,
        AuditionDestination::Native { channel_id, .. } if channel_id == fixture.channel_id
    ));
    assert_eq!(receipt(&fixture, token), Some(AuditionStatus::Pending));
    callback(&mut fixture, 128);
    assert_eq!(receipt(&fixture, token), Some(AuditionStatus::Accepted));
    assert_direction(&fixture, manual_pan, "owned Begin Accepted");
    fixture.ui.run(Vec::new());

    // Device time crosses the automation boundary and the score's Note Off.
    // Neither can advance the paused timeline or leak its old pan into audition.
    for _ in 0..80 {
        fixture.pump(128);
        assert_direction(&fixture, manual_pan, "paused owned hold");
        assert_eq!(receipt(&fixture, token), Some(AuditionStatus::Accepted));
        assert_eq!(fixture.ui.app.keyboard_held_token(), Some(token));
        assert_eq!(fixture.ui.app.keyboard_audition_pending_count(), 1);
        let current = fixture.snapshot();
        assert!(!current.transport_playing);
        assert_eq!(current.timeline_frame, paused.timeline_frame);
        assert_eq!(current.transport_epoch, paused.transport_epoch);
        assert_eq!(current.timeline_automation_pending, 0);
        assert_eq!(
            fixture
                .ui
                .app
                .audio
                .as_ref()
                .unwrap()
                .confirmed_timeline_identity(),
            identity
        );
    }
    assert!(fixture.snapshot().device_frame - paused.device_frame > pan_change);
    pointer(&mut fixture, point, false);
    assert!(fixture.ui.app.keyboard_held_token().is_none());
    callback(&mut fixture, 128);
    assert_eq!(receipt(&fixture, token), Some(AuditionStatus::Released));
    assert_direction(&fixture, manual_pan, "owned End Released tail");
    fixture.ui.run(Vec::new());
    assert_eq!(fixture.ui.app.keyboard_audition_pending_count(), 0);
    for _ in 0..24 {
        fixture.pump(128);
    }
    assert!(
        fixture
            .callback
            .last_output()
            .iter()
            .all(|frame| *frame == [0.0; 2]),
        "released audition tail must reach exact silence"
    );
    assert_eq!(fixture.snapshot().timeline_frame, paused.timeline_frame);
    assert_eq!(fixture.snapshot().transport_epoch, paused.transport_epoch);

    // Resume through the real App chase handshake. The old Note On is not
    // retriggered, but a future scored onset still uses the old control value
    // before the boundary despite all the paused device frames. The later
    // value changes at its original musical position.
    start(&mut fixture);
    fixture.pump(128);
    assert!(fixture.snapshot().timeline_frame < resumed_on);
    assert!(fixture.snapshot().timeline_frame < pan_change);
    assert!(
        fixture
            .callback
            .last_output()
            .iter()
            .all(|frame| *frame == [0.0; 2]),
        "the default resume chase must not replay the pre-pause Note On"
    );
    advance_to_frame(&mut fixture, resumed_on + 256);
    assert!(fixture.snapshot().timeline_frame < pan_change);
    assert_direction(&fixture, automated_pan, "resumed future song Note On");
    advance_to_frame(&mut fixture, pan_change);
    assert_direction(
        &fixture,
        automated_pan,
        "last callback before original pan boundary",
    );
    fixture.pump(128);
    assert_direction(&fixture, manual_pan, "resumed original pan boundary");
    assert_eq!(fixture.snapshot().timeline_automation_pending, 0);
    assert_eq!(project_fingerprint(&fixture.ui.app.project), original);
    assert_eq!(
        serde_json::to_string(&fixture.ui.app.project.patterns).unwrap(),
        original_score
    );
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
fn paused_owned_keyboard_callback_uses_manual_hard_left_after_driven_master_pan() {
    paused_owned_audition_uses_manual_pan(-1.0);
}

#[test]
fn paused_owned_keyboard_callback_uses_manual_hard_right_after_driven_master_pan() {
    paused_owned_audition_uses_manual_pan(1.0);
}
