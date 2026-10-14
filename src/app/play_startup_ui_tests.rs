//! Real UI Play/Stop, production Timeline activation and a real fake worker.
use super::*;
use plugins::plugin_runtime::{
    BackendSlot, MidiMessage, PluginBackend, PluginChain, PluginPrepareConfig, PluginTransport,
    RetainedShutdownPoll,
};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicU32, Ordering},
};

#[test]
fn master_capture_processing_fault_ui_stops_at_callback_ack_and_reports_partial_take() {
    let files = Fixture::new();
    let path = files.0.join("Partial-master.wav");
    let mut ui = UiHarness::new();
    let (mut audio, mut callback) = AudioEngine::test_engine_with_callback();
    let session = MasterCaptureSession::start_to_path(&path, 48_000).unwrap();
    let (mut endpoint, control) = session.into_parts();
    let capture_id = control.session_id();
    let fault = crate::plugin_timing::PluginProcessingFault {
        endpoint_id: 7,
        epoch: 42,
        expected_sequence: 9,
        raw_callback_frames: 480,
        callback_budget_frames: 2048,
        lookahead_quanta: 18,
        timing_revision: 1,
        reason: crate::plugin_timing::PluginProcessingFaultReason::DeadlineMiss,
    };
    assert!(endpoint.mark_processing_fault(0, fault));
    assert!(audio.install_master_capture(capture_id, endpoint));
    ui.app.audio = Some(audio);
    ui.app.master_capture = Some(ActiveMasterCapture {
        capture_id,
        path: path.clone(),
        control,
        phase: MasterCapturePhase::Installing,
        start_device_frame: None,
    });
    callback.pump(ui.app.audio.as_ref().unwrap(), 128);
    ui.app.poll_master_capture();
    assert_eq!(
        ui.app.master_capture.as_ref().unwrap().phase,
        MasterCapturePhase::Stopping
    );
    assert!(ui.app.pending_master_capture.is_none());
    assert!(ui.app.last_master_capture.is_none());

    // The UI requests Stop; it may finalize only after callback ownership comes back.
    callback.pump(ui.app.audio.as_ref().unwrap(), 128);
    ui.app.poll_master_capture();
    assert!(ui.app.master_capture.is_none());
    let deadline = Instant::now() + Duration::from_secs(5);
    while ui.app.pending_master_capture.is_some() {
        assert!(Instant::now() < deadline, "capture writer did not finish");
        std::thread::sleep(Duration::from_millis(1));
        ui.app.poll_master_capture();
    }
    let metadata = ui.app.last_master_capture.as_ref().unwrap();
    assert_eq!(metadata.path, path);
    assert_eq!(metadata.processing_fault.unwrap().fault, fault);
    assert!(!metadata.is_valid());
    let diagnostic = master_capture_metadata_diagnostics(metadata);
    assert!(diagnostic.contains("partial capture"));
    assert!(diagnostic.contains("DeadlineMiss, epoch 42, sequence 9, device frame 0"));
    assert!(
        path.exists(),
        "partial audio remains available for inspection"
    );
}

#[derive(Default)]
struct StartProbe {
    reset_held: AtomicBool,
    reset_entered: AtomicBool,
    reset_completed: AtomicU32,
    admin_held: AtomicBool,
    admin_entered: AtomicBool,
    save_calls: AtomicU32,
    save_error: AtomicBool,
    music_processes: AtomicU32,
    first_music_position: Mutex<Option<i64>>,
}

struct StartBackend {
    probe: Arc<StartProbe>,
    note: bool,
    transport: PluginTransport,
}

impl PluginBackend for StartBackend {
    fn name(&self) -> &str {
        "Play preparation fixture"
    }
    fn prepare(&mut self, _: PluginPrepareConfig) -> Result<(), String> {
        Ok(())
    }
    fn midi_capabilities(&self) -> (bool, bool) {
        (true, false)
    }
    fn set_transport(&mut self, transport: PluginTransport) -> Result<(), String> {
        self.transport = transport;
        Ok(())
    }
    fn send_midi(&mut self, message: MidiMessage) -> Result<(), String> {
        match message.data[0] & 0xf0 {
            0x90 if message.data[2] != 0 => self.note = true,
            0x80 | 0xb0 => self.note = false,
            _ => {}
        }
        Ok(())
    }
    fn reset_processing(&mut self) -> Result<(), String> {
        self.probe.reset_entered.store(true, Ordering::Release);
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.probe.reset_held.load(Ordering::Acquire) {
            if Instant::now() >= deadline {
                return Err("fixture reset gate expired".into());
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        self.note = false;
        self.probe.reset_completed.fetch_add(1, Ordering::Release);
        Ok(())
    }
    fn process(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        frames: usize,
    ) -> Result<(), String> {
        let value = if self.transport.playing {
            self.probe.music_processes.fetch_add(1, Ordering::Release);
            let mut first = self.probe.first_music_position.lock().unwrap();
            if first.is_none() {
                *first = Some(self.transport.sample_position);
            }
            if self.note { 0.35 } else { 0.0 }
        } else {
            0.9 // Audible stopped safety output must be fenced, not leaked.
        };
        left[..frames].fill(value);
        right[..frames].fill(value);
        Ok(())
    }
    fn set_parameter(&mut self, _: u32, _: f32) -> Result<(), String> {
        Ok(())
    }
    fn get_parameter(&mut self, _: u32) -> Result<f32, String> {
        Ok(0.0)
    }
    fn save_state(&mut self) -> Result<Vec<u8>, String> {
        self.probe.save_calls.fetch_add(1, Ordering::Release);
        self.probe.admin_entered.store(true, Ordering::Release);
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.probe.admin_held.load(Ordering::Acquire) {
            if Instant::now() >= deadline {
                return Err("fixture admin gate expired".into());
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        if self.probe.save_error.load(Ordering::Acquire) {
            return Err("fixture native state capture failed".into());
        }
        Ok(Vec::new())
    }
    fn load_state(&mut self, _: &[u8]) -> Result<(), String> {
        Ok(())
    }
    fn latency_samples(&self) -> u32 {
        0
    }
    fn tail_samples(&self) -> u32 {
        0
    }
}

struct StartFixture {
    callback: Option<crate::audio::TestAudioCallback>,
    ui: UiHarness,
    probe: Arc<StartProbe>,
    channel: u32,
    initial_save_calls: u32,
}

impl StartFixture {
    fn new() -> Self {
        let mut ui = UiHarness::new();
        ui.app.project = Project::blank();
        ui.app.transport_mode = TransportMode::Pattern;
        let channel = ui.app.project.channels[0].id;
        let track_id = ui.app.project.mixer_tracks[0].id;
        ui.app.project.channels[0].mixer_track = track_id;
        ui.app.project.channels[0].instrument_plugin_instance_id = Some(700);
        ui.app.project.plugin_instances.push(PluginInstance {
            midi_ports: Default::default(),
            id: 700,
            format: ProjectPluginFormat::Vst3,
            role: PluginRole::Instrument,
            path: PathBuf::new(),
            uid: "test".into(),
            vendor: "test".into(),
            name: "Play preparation fixture".into(),
            enabled: true,
            bypass: false,
            wet: 1.0,
            parameters: Default::default(),
            opaque_state: Vec::new(),
            runtime_status: PluginRuntimeStatus::Loaded,
        });
        ui.app.project.active_pattern_mut().notes = vec![PianoNote {
            id: 1,
            channel_id: Some(channel),
            group_id: None,
            note: 60,
            start: 0.0,
            length: 2.0,
            velocity: 0.5,
            selected: false,
            muted: false,
        }];
        ui.app.sync_history_observer();
        ui.app.project_fingerprint = project_fingerprint(&ui.app.project);
        let probe = Arc::new(StartProbe::default());
        let worker_probe = Arc::clone(&probe);
        let chain = PluginChain::spawn_identified_with_backend_factory(
            &[700],
            move || {
                vec![BackendSlot::new(Box::new(StartBackend {
                    probe: worker_probe,
                    note: false,
                    transport: PluginTransport::default(),
                }))]
            },
            PluginPrepareConfig {
                sample_rate: 48_000.0,
                max_block_frames: 128,
            },
        )
        .unwrap();
        let (mut audio, callback) = AudioEngine::test_engine_with_callback();
        assert!(audio.set_project_session(ui.app.project_session));
        assert!(audio.install_generator_endpoint_for_project(
            ui.app.project_session,
            channel,
            70,
            700,
            0,
            chain.audio
        ));
        ui.app.running_generator_chains.insert(
            channel,
            RunningGeneratorChain {
                project_session: ui.app.project_session,
                endpoint_id: 70,
                instance_id: 700,
                mixer_track: 0,
                control: chain.control,
                guard: chain.guard,
            },
        );
        ui.app.audio = Some(audio);
        ui.app.request_immediate_timeline_compile();
        let mut fixture = Self {
            callback: Some(callback),
            ui,
            probe,
            channel,
            initial_save_calls: 0,
        };
        fixture.until(|f| f.ui.app.timeline_audio_matches_desired());
        fixture.until(|f| {
            f.ui.app
                .audio
                .as_ref()
                .unwrap()
                .prepared_playback_token()
                .is_some()
        });
        fixture.initial_save_calls = fixture.probe.save_calls.load(Ordering::Acquire);
        fixture.probe.admin_entered.store(false, Ordering::Release);
        fixture
    }
    fn snapshot(&self) -> AudioSnapshot {
        self.ui.app.audio.as_ref().unwrap().snapshot()
    }
    fn pump(&mut self) {
        self.callback
            .as_mut()
            .unwrap()
            .pump(self.ui.app.audio.as_ref().unwrap(), 128);
        self.ui.run(Vec::new());
        std::thread::sleep(Duration::from_millis(1));
    }
    fn until(&mut self, predicate: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !predicate(self) && Instant::now() < deadline {
            self.pump();
        }
        assert!(
            predicate(self),
            "phase={:?}, snapshot={:?}, toast={:?}",
            self.ui.app.timeline_audio_sync.phase,
            self.snapshot(),
            self.ui.app.toast
        );
    }
    fn hold_reset_and_press_play(&mut self) {
        self.probe.reset_entered.store(false, Ordering::Release);
        self.probe.reset_held.store(true, Ordering::Release);
        self.ui.key(egui::Key::Space, egui::Modifiers::NONE);
        assert!(self.ui.app.prepared_playback_start.is_some());
        assert!(!self.ui.app.playing);
        self.until(|f| f.probe.reset_entered.load(Ordering::Acquire));
    }
}

impl Drop for StartFixture {
    fn drop(&mut self) {
        self.probe.reset_held.store(false, Ordering::Release);
        self.probe.admin_held.store(false, Ordering::Release);
        drop(self.callback.take());
        if let Some(mut chain) = self.ui.app.running_generator_chains.remove(&self.channel) {
            let deadline = Instant::now() + Duration::from_secs(5);
            let terminal = loop {
                let poll = chain.guard.poll_shutdown_retained();
                if poll != RetainedShutdownPoll::Pending || Instant::now() >= deadline {
                    break poll;
                }
                std::thread::sleep(Duration::from_millis(1));
            };
            if terminal == RetainedShutdownPoll::Pending {
                std::mem::forget(chain); // Retain the exact unproven worker; never consume it.
                if !std::thread::panicking() {
                    panic!("play preparation fixture worker retained");
                }
            } else if !std::thread::panicking() {
                assert_eq!(terminal, RetainedShutdownPoll::Joined);
            }
        }
    }
}

fn hold_first_loop_restart(f: &mut StartFixture) {
    // Seek through the production control path, rather than spending the test's
    // five-second wall-clock budget rendering an entire musical period in UI frames.
    let near_end = f64::from(f.ui.app.transport_loop_end_beat()) - 0.05;
    let near_end_frame =
        f.ui.app
            .timeline_audio_source
            .as_ref()
            .unwrap()
            .tempo_map
            .beat_to_frame(near_end)
            .unwrap();
    f.ui.app.publish_transport_seek(near_end);
    f.until(|f| {
        f.ui.app.timeline_audio_matches_desired()
            && f.snapshot().timeline_frame == near_end_frame
            && f.ui
                .app
                .audio
                .as_ref()
                .unwrap()
                .prepared_playback_token()
                .is_some()
    });
    f.ui.key(egui::Key::Space, egui::Modifiers::NONE);
    f.until(|f| f.snapshot().transport_playing);
    let before_wrap = f.snapshot();
    let completed_resets = f.probe.reset_completed.load(Ordering::Acquire);
    f.probe.reset_entered.store(false, Ordering::Release);
    f.probe.reset_held.store(true, Ordering::Release);
    f.until(|f| {
        f.snapshot().transport_loop_restart_pending && f.probe.reset_entered.load(Ordering::Acquire)
    });
    let waiting = f.snapshot();
    assert_eq!(
        waiting.transport_loop_count,
        before_wrap.transport_loop_count + 1
    );
    assert_eq!(waiting.transport_epoch, before_wrap.transport_epoch + 1);
    assert_eq!(waiting.timeline_frame, 0);
    assert_eq!(
        f.probe.reset_completed.load(Ordering::Acquire),
        completed_resets
    );
    assert!(
        f.ui.app.playing,
        "retain the user's loop Play intent while waiting"
    );
    assert!(!f.ui.app.transport_clock_playing);
    assert!(f.ui.app.loop_restart_wait.is_some());
    assert_eq!(
        f.ui.app.plugin_processing_status_label(),
        "Loop restart · transport stopped"
    );
}

#[test]
fn playback_loop_ui_waits_then_first_note_onset_matches_pdc_in_the_same_epoch() {
    let mut f = StartFixture::new();
    hold_first_loop_restart(&mut f);
    let epoch = f.snapshot().transport_epoch;
    for _ in 0..24 {
        f.pump();
        let snapshot = f.snapshot();
        assert!(snapshot.transport_loop_restart_pending);
        assert!(!snapshot.transport_playing);
        assert_eq!(snapshot.timeline_frame, 0);
        assert!(snapshot.plugin_processing_fault.is_none());
        assert!(
            f.callback
                .as_ref()
                .unwrap()
                .last_output()
                .iter()
                .all(|v| *v == [0.0; 2])
        );
    }
    *f.probe.first_music_position.lock().unwrap() = None;
    f.probe.reset_held.store(false, Ordering::Release);
    let latency = f.snapshot().pdc_output_latency_samples as usize;
    let mut output = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while output.len() < latency + 256 {
        assert!(Instant::now() < deadline, "loop restart did not settle");
        f.pump();
        let snapshot = f.snapshot();
        assert!(snapshot.plugin_processing_fault.is_none());
        assert_eq!(snapshot.transport_epoch, epoch);
        if snapshot.transport_playing {
            output.extend_from_slice(f.callback.as_ref().unwrap().last_output());
        }
    }
    assert!(f.ui.app.loop_restart_wait.is_none());
    assert!(f.ui.app.playing);
    let onset = output
        .iter()
        .position(|v| v.iter().any(|sample| sample.abs() > 1e-6))
        .expect("the first note of the repeated phrase must sound");
    assert_eq!(
        onset, latency,
        "the stopped safety Q must be fenced before music"
    );
    assert_eq!(*f.probe.first_music_position.lock().unwrap(), Some(0));
}

#[test]
fn playback_loop_ui_timeout_cancels_before_late_reset_can_resume() {
    let mut f = StartFixture::new();
    hold_first_loop_restart(&mut f);
    let music_count = f.probe.music_processes.load(Ordering::Acquire);
    let reset_count = f.probe.reset_completed.load(Ordering::Acquire);
    f.ui.app.loop_restart_wait.as_mut().unwrap().deadline = Instant::now();
    f.ui.run(Vec::new());
    assert!(!f.ui.app.playing);
    assert!(!f.ui.app.transport_clock_playing);
    assert!(f.ui.app.loop_restart_wait.is_none());
    assert!(
        f.ui.app
            .toast
            .as_ref()
            .unwrap()
            .0
            .contains("Loop restart preparation timed out")
    );
    f.probe.reset_held.store(false, Ordering::Release);
    f.until(|f| {
        f.probe.reset_completed.load(Ordering::Acquire) > reset_count
            && f.ui
                .app
                .audio
                .as_ref()
                .unwrap()
                .prepared_playback_token()
                .is_some()
    });
    for _ in 0..25 {
        f.pump();
    }
    assert!(!f.snapshot().transport_playing);
    assert!(!f.snapshot().transport_loop_restart_pending);
    assert_eq!(f.snapshot().timeline_frame, 0);
    assert_eq!(f.probe.music_processes.load(Ordering::Acquire), music_count);
}

#[test]
fn playback_loop_ui_pause_and_stop_cancel_without_delayed_resume() {
    for stop in [false, true] {
        let mut f = StartFixture::new();
        hold_first_loop_restart(&mut f);
        let music_count = f.probe.music_processes.load(Ordering::Acquire);
        let reset_count = f.probe.reset_completed.load(Ordering::Acquire);
        if stop {
            f.ui.app.stop();
        } else {
            f.ui.key(egui::Key::Space, egui::Modifiers::NONE);
        }
        f.probe.reset_held.store(false, Ordering::Release);
        f.until(|f| {
            f.probe.reset_completed.load(Ordering::Acquire) > reset_count
                && f.ui
                    .app
                    .audio
                    .as_ref()
                    .unwrap()
                    .prepared_playback_token()
                    .is_some()
        });
        for _ in 0..25 {
            f.pump();
        }
        assert!(!f.ui.app.playing);
        assert!(!f.snapshot().transport_playing);
        assert!(!f.snapshot().transport_loop_restart_pending);
        assert_eq!(f.probe.music_processes.load(Ordering::Acquire), music_count);
    }
}

#[test]
fn playback_loop_ui_seek_cancels_wait_before_publishing_a_new_activation() {
    let mut f = StartFixture::new();
    hold_first_loop_restart(&mut f);
    let reset_count = f.probe.reset_completed.load(Ordering::Acquire);
    let music_count = f.probe.music_processes.load(Ordering::Acquire);
    f.ui.app.publish_transport_seek(1.0);
    assert!(!f.ui.app.playing);
    assert!(f.ui.app.loop_restart_wait.is_none());
    f.probe.reset_held.store(false, Ordering::Release);
    f.until(|f| {
        f.probe.reset_completed.load(Ordering::Acquire) > reset_count
            && f.ui.app.timeline_audio_matches_desired()
            && f.ui
                .app
                .audio
                .as_ref()
                .unwrap()
                .prepared_playback_token()
                .is_some()
    });
    for _ in 0..25 {
        f.pump();
    }
    assert!(!f.snapshot().transport_playing);
    assert!(!f.snapshot().transport_loop_restart_pending);
    assert!(f.snapshot().plugin_processing_fault.is_none());
    assert_eq!(f.probe.music_processes.load(Ordering::Acquire), music_count);
}

#[test]
fn playback_loop_ui_observed_cancel_survives_reset_finishing_before_the_action() {
    let mut f = StartFixture::new();
    hold_first_loop_restart(&mut f);
    f.ui.app.loop_restart_wait = None;
    assert!(f.ui.app.has_current_loop_restart_wait());
    f.probe.reset_held.store(false, Ordering::Release);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !f.snapshot().transport_playing {
        assert!(Instant::now() < deadline, "loop did not resume");
        f.callback
            .as_mut()
            .unwrap()
            .pump(f.ui.app.audio.as_ref().unwrap(), 128);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(f.ui.app.loop_restart_wait.is_none());
    assert!(!f.ui.app.has_current_loop_restart_wait());
    // The action belongs to the earlier real pending observation, even though
    // both later observations now contain no pending work and no cached UI wait.
    f.ui.app.cancel_current_loop_restart();
    assert!(!f.ui.app.playing);
    assert!(!f.ui.app.transport_clock_playing);
    for _ in 0..25 {
        f.pump();
    }
    assert!(!f.snapshot().transport_playing);
    assert!(!f.snapshot().transport_loop_restart_pending);
    assert!(f.snapshot().plugin_processing_fault.is_none());
}

#[test]
fn playback_loop_ui_resumed_playback_uses_normal_pause_before_ui_wait_cleanup() {
    let mut f = StartFixture::new();
    hold_first_loop_restart(&mut f);
    f.probe.reset_held.store(false, Ordering::Release);
    let deadline = Instant::now() + Duration::from_secs(5);
    // Let the real callback resume while the UI deliberately retains its last
    // waiting observation. That stale observation cannot suppress normal Pause.
    while !f.snapshot().transport_playing {
        assert!(Instant::now() < deadline, "loop did not resume");
        f.callback
            .as_mut()
            .unwrap()
            .pump(f.ui.app.audio.as_ref().unwrap(), 128);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(f.ui.app.loop_restart_wait.is_some());
    assert!(!f.ui.app.has_current_loop_restart_wait());
    f.ui.key(egui::Key::Space, egui::Modifiers::NONE);
    assert!(!f.ui.app.playing);
    assert!(f.ui.app.loop_restart_wait.is_none());
    assert!(
        f.ui.app
            .toast
            .as_ref()
            .is_none_or(|toast| { !toast.0.contains("Loop restart preparation canceled") })
    );
    for _ in 0..3 {
        f.pump();
    }
    assert!(!f.snapshot().transport_playing);
    assert!(f.snapshot().plugin_processing_fault.is_none());
}

#[test]
fn playback_loop_ui_old_wait_deadline_does_not_control_a_replacement_engine() {
    for cancel_directly in [false, true] {
        let mut f = StartFixture::new();
        hold_first_loop_restart(&mut f);
        f.ui.app.loop_restart_wait.as_mut().unwrap().deadline = Instant::now();
        let (replacement, mut callback) = AudioEngine::test_engine_with_callback();
        replacement.set_playing(true);
        callback.pump(&replacement, 128);
        assert!(replacement.snapshot().transport_playing);
        let old_engine = f.ui.app.audio.replace(replacement).unwrap();
        if cancel_directly {
            f.ui.app.cancel_prepared_playback_start();
        } else {
            f.ui.app.poll_loop_restart_wait(&f.ui.ctx);
        }
        assert!(f.ui.app.loop_restart_wait.is_none());
        assert!(f.ui.app.playing);
        assert!(
            f.ui.app
                .audio
                .as_ref()
                .unwrap()
                .snapshot()
                .transport_playing
        );
        callback.pump(f.ui.app.audio.as_ref().unwrap(), 128);
        assert!(
            f.ui.app
                .audio
                .as_ref()
                .unwrap()
                .snapshot()
                .transport_playing
        );
        // Restore fixture ownership so the held original worker is joined normally.
        let replacement = f.ui.app.audio.replace(old_engine).unwrap();
        drop(callback);
        drop(replacement);
    }
}

#[test]
fn playback_startup_ui_delayed_reset_stays_stopped_then_first_note_onset_matches_pdc() {
    let mut f = StartFixture::new();
    f.hold_reset_and_press_play();
    let epoch = f.snapshot().transport_epoch;
    for _ in 0..12 {
        f.pump();
        assert!(!f.snapshot().transport_playing);
        assert_eq!(f.snapshot().timeline_frame, 0);
        assert!(
            f.callback
                .as_ref()
                .unwrap()
                .last_output()
                .iter()
                .all(|frame| *frame == [0.0; 2])
        );
        assert_eq!(f.probe.music_processes.load(Ordering::Acquire), 0);
    }
    f.probe.reset_held.store(false, Ordering::Release);
    // Stop before callback playback to collect its actual first output frame.
    f.until(|f| {
        f.ui.app
            .prepared_playback_start
            .as_ref()
            .is_some_and(|pending| pending.issued_request.is_some())
    });
    let latency = f.snapshot().pdc_output_latency_samples as usize;
    let mut output = Vec::new();
    while output.len() < latency + 256 {
        f.pump();
        output.extend_from_slice(f.callback.as_ref().unwrap().last_output());
        assert!(f.snapshot().plugin_processing_fault.is_none());
        assert_eq!(
            f.snapshot().transport_epoch,
            epoch,
            "release must reuse the prepared epoch"
        );
    }
    assert!(f.ui.app.playing);
    assert_eq!(f.snapshot().timeline_frame, output.len() as u64);
    let onset = output
        .iter()
        .position(|frame| frame.iter().any(|value| value.abs() > 1e-6))
        .expect("first MIDI note must sound");
    assert_eq!(
        onset, latency,
        "stopped safety Q must be silent and first music Q must not be swallowed"
    );
    assert_eq!(*f.probe.first_music_position.lock().unwrap(), Some(0));
}

#[test]
fn playback_startup_ui_stop_and_second_play_cancel_without_delayed_autoplay() {
    for cancel_by_stop in [true, false] {
        let mut f = StartFixture::new();
        f.hold_reset_and_press_play();
        if cancel_by_stop {
            f.ui.app.stop();
        } else {
            f.ui.key(egui::Key::Space, egui::Modifiers::NONE);
        }
        assert!(f.ui.app.prepared_playback_start.is_none());
        f.probe.reset_held.store(false, Ordering::Release);
        for _ in 0..25 {
            f.pump();
        }
        assert!(!f.ui.app.playing);
        assert!(!f.snapshot().transport_playing);
        assert_eq!(f.snapshot().timeline_frame, 0);
        assert_eq!(f.probe.music_processes.load(Ordering::Acquire), 0);
    }
}

#[test]
fn playback_startup_ui_edit_seek_and_timeout_invalidate_pending_start() {
    for change in 0..3 {
        let mut f = StartFixture::new();
        f.hold_reset_and_press_play();
        match change {
            0 => {
                let mut candidate = f.ui.app.project.clone();
                candidate.active_pattern_mut().notes[0].note = 64;
                f.ui.app.commit_editor_project(candidate);
            }
            1 => f.ui.app.publish_transport_seek(1.0),
            _ => f.ui.app.prepared_playback_start.as_mut().unwrap().deadline = Instant::now(),
        }
        f.ui.run(Vec::new());
        assert!(f.ui.app.prepared_playback_start.is_none());
        f.probe.reset_held.store(false, Ordering::Release);
        for _ in 0..25 {
            f.pump();
        }
        assert!(!f.ui.app.playing);
        assert!(!f.snapshot().transport_playing);
        assert_eq!(f.probe.music_processes.load(Ordering::Acquire), 0);
    }
}

#[test]
fn playback_startup_ui_edit_cancels_an_issued_guard_before_the_next_ui_poll() {
    let mut f = StartFixture::new();
    f.ui.key(egui::Key::Space, egui::Modifiers::NONE);
    f.until(|f| {
        f.ui.app
            .prepared_playback_start
            .as_ref()
            .is_some_and(|pending| pending.issued_request.is_some())
    });
    assert!(!f.snapshot().transport_playing);
    f.probe.admin_entered.store(false, Ordering::Release);
    f.probe.admin_held.store(true, Ordering::Release);
    assert!(
        f.ui.app.running_generator_chains[&f.channel]
            .control
            .request_state(0)
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while !f.probe.admin_entered.load(Ordering::Acquire) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(f.probe.admin_entered.load(Ordering::Acquire));
    // The issued guard cannot yet be consumed because real admin work is held.
    f.callback
        .as_mut()
        .unwrap()
        .pump(f.ui.app.audio.as_ref().unwrap(), 128);
    assert!(!f.snapshot().transport_playing);
    let mut candidate = f.ui.app.project.clone();
    candidate.active_pattern_mut().notes[0].note = 64;
    f.ui.app.commit_editor_project(candidate);
    f.probe.admin_held.store(false, Ordering::Release);
    // No UI poll may be required to cancel an already-issued stale guard.
    for _ in 0..25 {
        f.callback
            .as_mut()
            .unwrap()
            .pump(f.ui.app.audio.as_ref().unwrap(), 128);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(!f.snapshot().transport_playing);
    assert_eq!(f.snapshot().timeline_frame, 0);
    assert_eq!(f.probe.music_processes.load(Ordering::Acquire), 0);
    assert!(f.ui.app.prepared_playback_start.is_none());
}

#[test]
fn state_save_ui_overdue_recovery_defers_until_stop_ack_then_captures_once() {
    let files = Fixture::new();
    let path = files.0.join("Recovery.citrus");
    let mut f = StartFixture::new();
    f.ui.key(egui::Key::Space, egui::Modifiers::NONE);
    f.until(|f| f.ui.app.playing && f.snapshot().transport_playing);
    f.ui.app.autosave_path = Some(path.clone());
    f.ui.app.dirty = true;
    let due = Instant::now() - AUTOSAVE_INTERVAL - Duration::from_secs(1);
    f.ui.app.last_autosave = due;
    for _ in 0..25 {
        f.pump();
        assert!(f.ui.app.save_barrier.is_none());
        assert_eq!(f.ui.app.last_autosave, due);
        assert_eq!(
            f.probe.save_calls.load(Ordering::Acquire),
            f.initial_save_calls
        );
        assert!(f.snapshot().plugin_processing_fault.is_none());
    }
    assert!(!path.exists());
    f.probe.admin_held.store(true, Ordering::Release);
    f.ui.app.stop();
    // Stop is a mailbox request. Neither a false UI flag nor the old callback
    // position proves it has been applied to the actual callback yet.
    assert!(!f.ui.app.playing);
    f.ui.app.autosave();
    assert!(f.ui.app.save_barrier.is_none());
    assert_eq!(
        f.probe.save_calls.load(Ordering::Acquire),
        f.initial_save_calls
    );
    f.until(|f| f.probe.admin_entered.load(Ordering::Acquire));
    assert!(!f.snapshot().transport_playing);
    assert!(f.ui.app.save_barrier.is_some());
    f.ui.app.toggle_play();
    assert!(f.ui.app.prepared_playback_start.is_none());
    assert!(!f.ui.app.playing);
    f.ui.app.toggle_record();
    assert!(f.ui.app.input_recorder.is_none());
    assert!(f.ui.app.midi_recording.is_idle());
    assert!(!f.ui.app.playing);
    f.probe.admin_held.store(false, Ordering::Release);
    f.until(|f| path.exists() && f.ui.app.save_barrier.is_none());
    assert_eq!(
        f.probe.save_calls.load(Ordering::Acquire),
        f.initial_save_calls + 1
    );
    assert!(
        f.ui.app.dirty,
        "a recovery copy does not mark the project saved"
    );
    let restored = Project::load(&path).unwrap();
    assert_eq!(restored.active_pattern().notes[0].note, 60);
    for _ in 0..10 {
        f.pump();
    }
    assert_eq!(
        f.probe.save_calls.load(Ordering::Acquire),
        f.initial_save_calls + 1
    );
}

#[test]
fn state_save_ui_recovery_does_not_capture_during_play_preparation() {
    let files = Fixture::new();
    let mut f = StartFixture::new();
    f.hold_reset_and_press_play();
    f.ui.app.autosave_path = Some(files.0.join("Recovery.citrus"));
    f.ui.app.dirty = true;
    let due = Instant::now() - AUTOSAVE_INTERVAL - Duration::from_secs(1);
    f.ui.app.last_autosave = due;
    for _ in 0..12 {
        f.pump();
        assert!(!f.ui.app.playing);
        assert!(f.ui.app.prepared_playback_start.is_some());
        assert!(f.ui.app.save_barrier.is_none());
        assert_eq!(f.ui.app.last_autosave, due);
        assert_eq!(
            f.probe.save_calls.load(Ordering::Acquire),
            f.initial_save_calls
        );
    }
    f.ui.app.stop();
    f.ui.app.autosave();
    assert!(f.ui.app.save_barrier.is_none());
    f.probe.reset_held.store(false, Ordering::Release);
    f.probe.admin_held.store(true, Ordering::Release);
    f.until(|f| f.probe.admin_entered.load(Ordering::Acquire));
    assert_eq!(f.probe.music_processes.load(Ordering::Acquire), 0);
    assert!(!f.snapshot().transport_playing);
}

#[test]
fn state_save_ui_manual_wait_is_not_a_capture_timeout_and_stop_keys_work() {
    for stop_key in [egui::Key::Escape, egui::Key::Space] {
        let files = Fixture::new();
        let path = files.0.join("Manual.citrus");
        let mut f = StartFixture::new();
        f.ui.key(egui::Key::Space, egui::Modifiers::NONE);
        f.until(|f| f.ui.app.playing && f.snapshot().transport_playing);
        f.ui.app.dirty = true;
        f.probe.admin_held.store(true, Ordering::Release);
        f.ui.app.begin_project_save(ProjectSaveRequest::Manual {
            path: path.clone(),
            lifecycle: None,
        });
        f.ui.app.save_barrier.as_mut().unwrap().started_at =
            Instant::now() - PLUGIN_STATE_SAVE_TIMEOUT - Duration::from_secs(1);
        f.pump();
        assert!(!path.exists());
        assert!(!f.ui.app.save_barrier.as_ref().unwrap().requests_initialized);
        assert_eq!(
            f.probe.save_calls.load(Ordering::Acquire),
            f.initial_save_calls
        );
        assert!(f.snapshot().transport_playing);
        f.ui.key(stop_key, egui::Modifiers::NONE);
        assert!(!f.ui.app.playing);
        assert_eq!(
            f.probe.save_calls.load(Ordering::Acquire),
            f.initial_save_calls
        );
        f.until(|f| f.probe.admin_entered.load(Ordering::Acquire));
        assert!(!f.snapshot().transport_playing);
        f.ui.app.begin_prepared_playback_start(true);
        assert!(f.ui.app.prepared_playback_start.is_none());
        f.probe.admin_held.store(false, Ordering::Release);
        f.until(|f| path.exists() && f.ui.app.save_barrier.is_none());
        assert!(!f.ui.app.dirty);
        assert_eq!(
            f.probe.save_calls.load(Ordering::Acquire),
            f.initial_save_calls + 1
        );
        assert!(f.snapshot().plugin_processing_fault.is_none());
    }
}

#[test]
fn state_save_ui_failed_native_capture_keeps_dirty_and_last_confirmed_state() {
    let files = Fixture::new();
    let path = files.0.join("Failed-state.citrus");
    let mut f = StartFixture::new();
    f.ui.app.project.plugin_instances[0].opaque_state = vec![4, 5, 6];
    f.ui.app.dirty = true;
    f.probe.admin_held.store(true, Ordering::Release);
    f.probe.save_error.store(true, Ordering::Release);
    f.ui.app.begin_project_save(ProjectSaveRequest::Manual {
        path: path.clone(),
        lifecycle: None,
    });
    f.until(|f| f.probe.admin_entered.load(Ordering::Acquire));
    f.probe.admin_held.store(false, Ordering::Release);
    f.until(|f| path.exists() && f.ui.app.save_barrier.is_none());
    assert!(f.ui.app.dirty);
    assert_eq!(f.ui.app.project.plugin_instances[0].opaque_state, [4, 5, 6]);
    assert_eq!(
        Project::load(&path).unwrap().plugin_instances[0].opaque_state,
        [4, 5, 6]
    );
    assert!(
        f.ui.app
            .toast
            .as_ref()
            .unwrap()
            .0
            .contains("stale plug-in state")
    );
}

#[test]
fn state_save_ui_dispatched_capture_timeout_still_finishes_with_unacked_transport() {
    let files = Fixture::new();
    let path = files.0.join("Timed-out-state.citrus");
    let mut f = StartFixture::new();
    f.ui.app.dirty = true;
    f.probe.admin_held.store(true, Ordering::Release);
    f.ui.app.begin_project_save(ProjectSaveRequest::Manual {
        path: path.clone(),
        lifecycle: None,
    });
    f.until(|f| f.probe.admin_entered.load(Ordering::Acquire));
    assert!(f.ui.app.save_barrier.as_ref().unwrap().requests_initialized);
    assert!(!f.ui.app.save_barrier.as_ref().unwrap().waiting.is_empty());
    // An external transport request remains unacknowledged while the real
    // backend is held. A dispatched capture must still reach its own timeout.
    f.ui.app.audio.as_ref().unwrap().set_playing(true);
    f.ui.app.save_barrier.as_mut().unwrap().started_at =
        Instant::now() - PLUGIN_STATE_SAVE_TIMEOUT - Duration::from_secs(1);
    f.ui.app.poll_save_barrier();
    assert!(f.ui.app.save_barrier.is_none());
    assert!(path.exists());
    assert!(f.ui.app.dirty);
    assert!(
        f.ui.app
            .toast
            .as_ref()
            .unwrap()
            .0
            .contains("stale plug-in state")
    );
    assert_eq!(
        f.probe.save_calls.load(Ordering::Acquire),
        f.initial_save_calls + 1
    );
    assert!(
        f.ui.app.running_generator_chains[&f.channel]
            .control
            .has_outstanding_admin_work()
    );
    f.ui.app.toggle_record();
    assert!(f.ui.app.input_recorder.is_none());
    assert!(f.ui.app.midi_recording.is_idle());
    assert!(!f.ui.app.playing);
    assert!(!f.snapshot().transport_playing);
}

#[test]
fn state_save_ui_explicit_clean_recovery_request_survives_playback_then_saves_once() {
    let files = Fixture::new();
    let path = files.0.join("Clean-recovery.citrus");
    let mut f = StartFixture::new();
    f.ui.key(egui::Key::Space, egui::Modifiers::NONE);
    f.until(|f| f.ui.app.playing && f.snapshot().transport_playing);
    f.ui.app.autosave_path = Some(path.clone());
    f.ui.app.dirty = false;
    f.ui.app.autosave_with_notice(true);
    assert!(f.ui.app.deferred_recovery_save_notice.is_some());
    assert!(f.ui.app.save_barrier.is_none());
    for _ in 0..12 {
        f.pump();
        assert!(!f.ui.app.dirty);
        assert_eq!(
            f.probe.save_calls.load(Ordering::Acquire),
            f.initial_save_calls
        );
    }
    f.ui.app.stop();
    f.until(|f| path.exists() && f.ui.app.save_barrier.is_none());
    assert!(f.ui.app.deferred_recovery_save_notice.is_none());
    assert_eq!(
        f.probe.save_calls.load(Ordering::Acquire),
        f.initial_save_calls + 1
    );
    for _ in 0..10 {
        f.pump();
    }
    assert_eq!(
        f.probe.save_calls.load(Ordering::Acquire),
        f.initial_save_calls + 1
    );
    // An explicit request belongs only to the project that requested it.
    f.ui.app.deferred_recovery_save_notice = Some(f.ui.app.project_session);
    f.ui.app.project_session += 1;
    f.ui.app.update_project_state();
    assert!(f.ui.app.deferred_recovery_save_notice.is_none());
    assert!(f.ui.app.save_barrier.is_none());
}
