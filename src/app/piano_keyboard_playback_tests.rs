//! Real device-free callback confirmation drives the App's keyboard activity.
//! These tests render the production UI and never manufacture playback positions,
//! callback receipts, native plug-in state, or evidence of audible output.
use super::*;

// Return callback-owned resources before dropping the App's AudioEngine.
pub(super) struct KeyboardCallbackFixture {
    pub(super) callback: crate::audio::TestAudioCallback,
    pub(super) ui: UiHarness,
    pub(super) channel_id: u32,
}

impl KeyboardCallbackFixture {
    pub(super) fn new() -> Self {
        let mut ui = UiHarness::new();
        ui.app.project = Project::blank();
        ui.app.project.tempo = 240.0;
        let channel_id = ui.app.project.channels[0].id;
        ui.app.project.active_pattern_mut().notes = vec![
            PianoNote {
                id: 91_001,
                channel_id: Some(channel_id),
                group_id: None,
                note: 60,
                start: 0.125,
                length: 0.75,
                velocity: 0.5,
                selected: false,
                muted: false,
            },
            PianoNote {
                id: 91_002,
                channel_id: Some(channel_id),
                group_id: None,
                note: 64,
                start: 1.25,
                length: 0.25,
                velocity: 0.5,
                selected: false,
                muted: false,
            },
        ];
        ui.app.selected_channel = 0;
        ui.app.beat_position = 0.0;
        ui.app.transport_mode = TransportMode::Pattern;
        ui.app.focus_editor(StudioView::PianoRoll);
        ui.app.sync_history_observer();
        ui.app.project_fingerprint = project_fingerprint(&ui.app.project);
        ui.app.undo_stack.clear();
        ui.app.redo_stack.clear();
        ui.app.dirty = false;
        let (mut audio, callback) = AudioEngine::test_engine_with_callback();
        assert!(audio.set_project_session(ui.app.project_session));
        ui.app.audio = Some(audio);
        ui.app.request_immediate_timeline_compile();
        let mut fixture = Self {
            callback,
            ui,
            channel_id,
        };
        fixture.until(|fixture| {
            fixture.ui.app.timeline_audio_matches_desired()
                && fixture.ui.app.piano_keyboard_activation.is_some()
        });
        assert!(!fixture.snapshot().transport_playing);
        fixture
    }

    pub(super) fn snapshot(&self) -> crate::audio::AudioSnapshot {
        self.ui.app.audio.as_ref().unwrap().snapshot()
    }

    pub(super) fn pump(&mut self, frames: usize) {
        self.callback
            .pump(self.ui.app.audio.as_ref().unwrap(), frames);
        self.ui.run(Vec::new());
    }

    pub(super) fn until(&mut self, condition: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !condition(self) && Instant::now() < deadline {
            self.pump(128);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(
            condition(self),
            "callback keyboard condition failed: phase={:?}, activation={:?}, snapshot={:?}, compile={:?}, toast={:?}",
            self.ui.app.timeline_audio_sync.phase,
            self.ui.app.piano_keyboard_activation,
            self.snapshot(),
            self.ui.app.timeline_compile_error,
            self.ui.app.toast,
        );
    }

    fn frame_for_beat(&self, beat: f64) -> u64 {
        self.ui
            .app
            .timeline_audio_source
            .as_ref()
            .unwrap()
            .tempo_map
            .beat_to_frame(beat)
            .unwrap()
    }

    fn advance_to_frame(&mut self, frame: u64) {
        assert!(self.snapshot().transport_playing);
        assert!(frame >= self.snapshot().timeline_frame);
        let loop_count = self.snapshot().transport_loop_count;
        while self.snapshot().timeline_frame < frame {
            let remaining = frame - self.snapshot().timeline_frame;
            self.pump(remaining.min(128) as usize);
            assert_eq!(
                self.snapshot().transport_loop_count,
                loop_count,
                "the requested frame must stay inside this loop"
            );
        }
    }

    fn start(&mut self) {
        self.ui.key(egui::Key::Space, egui::Modifiers::NONE);
        assert!(self.ui.app.playing);
        self.until(|fixture| {
            fixture.snapshot().transport_playing
                && fixture.ui.app.timeline_audio_matches_desired()
                && fixture
                    .ui
                    .app
                    .piano_keyboard_activation
                    .is_some_and(|activation| activation.4 == fixture.snapshot().transport_epoch)
        });
    }

    fn mask(&mut self) -> [bool; 128] {
        self.ui
            .app
            .piano_keyboard_playing_mask(Some(self.channel_id))
    }
}

#[test]
fn piano_keyboard_callback_activity_requires_current_confirmed_identity_and_clears_on_stop() {
    let mut fixture = KeyboardCallbackFixture::new();
    assert_eq!(fixture.mask(), [false; 128]);
    fixture.start();
    let first_on = fixture.frame_for_beat(0.125);
    fixture.advance_to_frame(first_on + 1);
    let mut first_note = [false; 128];
    first_note[60] = true;
    assert_eq!(fixture.mask(), first_note);

    let activation = fixture.ui.app.piano_keyboard_activation.unwrap();
    let (project, session, generation, seek, epoch) = activation;
    for stale in [
        (project.wrapping_add(1), session, generation, seek, epoch),
        (project, session.wrapping_add(1), generation, seek, epoch),
        (project, session, generation.wrapping_add(1), seek, epoch),
        (project, session, generation, seek.wrapping_add(1), epoch),
    ] {
        fixture.ui.app.piano_keyboard_activation = Some(stale);
        assert_eq!(fixture.mask(), [false; 128]);
        fixture.ui.app.piano_keyboard_activation = Some(activation);
        assert_eq!(fixture.mask(), first_note);
    }

    let first_off = fixture.frame_for_beat(0.875);
    fixture.advance_to_frame(first_off + 1);
    assert_eq!(fixture.mask(), [false; 128]);
    let second_on = fixture.frame_for_beat(1.25);
    fixture.advance_to_frame(second_on + 1);
    assert!(fixture.mask()[64]);
    fixture.ui.app.stop();
    assert_eq!(fixture.mask(), [false; 128]);
    fixture.until(|fixture| {
        !fixture.snapshot().transport_playing && fixture.ui.app.timeline_audio_matches_desired()
    });
    assert_eq!(fixture.mask(), [false; 128]);
}

#[test]
fn piano_keyboard_callback_seek_after_a_real_loop_starts_at_its_new_activation_boundary() {
    let mut fixture = KeyboardCallbackFixture::new();
    fixture.start();
    let initial_epoch = fixture.snapshot().transport_epoch;
    let deadline = Instant::now() + Duration::from_secs(5);
    while fixture.snapshot().transport_loop_count == 0 && Instant::now() < deadline {
        fixture.pump(crate::audio::MAX_MIXER_BLOCK_FRAMES);
    }
    assert!(fixture.snapshot().transport_loop_count > 0);
    assert_ne!(fixture.snapshot().transport_epoch, initial_epoch);
    assert!(fixture.ui.app.timeline_audio_matches_desired());

    // The seek falls between Note On events, within the first note's old
    // sustain. The callback loop counter intentionally survives this seek.
    let seek_beat = 0.375;
    let seek_frame = fixture.frame_for_beat(seek_beat);
    let old_serial = fixture.ui.app.timeline_audio_sync.desired_seek.serial;
    let old_epoch = fixture.snapshot().transport_epoch;
    fixture.ui.app.publish_transport_seek(seek_beat);
    let new_serial = fixture.ui.app.timeline_audio_sync.desired_seek.serial;
    assert_ne!(new_serial, old_serial);
    assert_eq!(fixture.mask(), [false; 128]);
    fixture.until(|fixture| {
        fixture.ui.app.timeline_audio_matches_desired()
            && fixture.snapshot().transport_playing
            && fixture
                .ui
                .app
                .piano_keyboard_activation
                .is_some_and(|activation| {
                    activation.3 == new_serial
                        && activation.4 == fixture.snapshot().transport_epoch
                        && activation.4 != old_epoch
                })
    });
    assert!(fixture.snapshot().transport_loop_count > 0);
    assert!(fixture.snapshot().timeline_frame >= seek_frame);
    assert!(fixture.snapshot().timeline_frame < fixture.frame_for_beat(0.875));
    assert_eq!(
        fixture.mask(),
        [false; 128],
        "the retained loop count must not replay the old first Note On"
    );

    let second_on = fixture.frame_for_beat(1.25);
    fixture.advance_to_frame(second_on + 1);
    let mut second_note = [false; 128];
    second_note[64] = true;
    assert_eq!(fixture.mask(), second_note);
    let second_off = fixture.frame_for_beat(1.5);
    fixture.advance_to_frame(second_off + 1);
    assert_eq!(fixture.mask(), [false; 128]);
}
