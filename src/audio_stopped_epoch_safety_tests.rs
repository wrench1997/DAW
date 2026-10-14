// Source-only stopped epoch regressions. All safety servicing below goes through
// the production paused callback. Driver waits only observe workers; they never
// supply another DSP block to manufacture settlement.
use super::*;
mod playback_startup_tests {
    include!("audio_play_startup_tests.rs");
}
use crate::fixed_quantum::FrameEventKind;
use crate::plugins::plugin_runtime::{
    BridgeStats, PluginTransport, RetainedShutdownPoll, SubmitStatus,
};
use std::sync::{Mutex, MutexGuard};

const SAFETY_CHANNEL: u32 = 9;
const SAFETY_GENERATOR: u64 = 700;
const SAFETY_INSERT: u64 = 800;
const SAFETY_GENERATOR_ENDPOINT: u64 = 70;
const SAFETY_INSERT_ENDPOINT: u64 = 80;
const SAFETY_REVISION: u64 = 71;
const SAFETY_PROJECT: u64 = 7;
const SAFETY_DRAIN_BUDGET: Duration = Duration::from_secs(5);
static SAFETY_SERIAL: Mutex<()> = Mutex::new(());
static SAFETY_ADMISSION_BLOCKED: AtomicBool = AtomicBool::new(false);
static SAFETY_QUARANTINE: Mutex<Vec<SafetyRetainedFailure>> = Mutex::new(Vec::new());

#[derive(Default)]
struct SafetyGate {
    held: AtomicBool,
    entered: AtomicBool,
}

impl SafetyGate {
    fn hold(&self) {
        self.entered.store(false, Ordering::Release);
        self.held.store(true, Ordering::Release);
    }
    fn release(&self) {
        self.held.store(false, Ordering::Release);
    }
    fn pass(&self) -> Result<(), String> {
        self.entered.store(true, Ordering::Release);
        let deadline = Instant::now() + SAFETY_DRAIN_BUDGET;
        while self.held.load(Ordering::Acquire) {
            if Instant::now() >= deadline {
                return Err("bounded mock gate expired".into());
            }
            thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    }
    fn wait_entered(&self) {
        wait_until(|| self.entered.load(Ordering::Acquire));
    }
}

#[derive(Clone, Debug, PartialEq)]
enum SafetyTrace {
    ResetPreflight,
    Midi([u8; 3]),
    ResetReturned,
    ProcessEntered(usize),
    ProcessReturned(usize),
    SaveEntered,
    SaveReturned,
    Parameter(u32, f32),
}

#[derive(Default)]
struct SafetyProbe {
    trace: Mutex<Vec<SafetyTrace>>,
    reset_gate: SafetyGate,
    process_gate: SafetyGate,
    admin_gate: SafetyGate,
    reset_failure: AtomicBool,
    latency: AtomicU32,
    process_count: AtomicU32,
    nonzero_input: AtomicBool,
    distinct_note_output: AtomicBool,
    idle_output_quanta: AtomicU32,
    note_output_quanta: AtomicU32,
}

impl SafetyProbe {
    fn record(&self, event: SafetyTrace) {
        self.trace.lock().unwrap().push(event);
    }
    fn trace(&self) -> Vec<SafetyTrace> {
        self.trace.lock().unwrap().clone()
    }
    fn release_all(&self) {
        self.reset_gate.release();
        self.process_gate.release();
        self.admin_gate.release();
    }
}

struct SafetyBackend {
    probe: Arc<SafetyProbe>,
    parameter: f32,
    note_held: bool,
}

impl PluginBackend for SafetyBackend {
    fn name(&self) -> &str {
        "stopped epoch safety mock"
    }
    fn prepare(&mut self, _: PluginPrepareConfig) -> Result<(), String> {
        Ok(())
    }
    fn midi_capabilities(&self) -> (bool, bool) {
        (true, false)
    }
    fn set_transport(&mut self, _: PluginTransport) -> Result<(), String> {
        Ok(())
    }
    fn preflight_reset_processing(&self) -> Result<(), String> {
        self.probe.record(SafetyTrace::ResetPreflight);
        if self.probe.reset_failure.load(Ordering::Acquire) {
            Err("deliberate stopped safety reset failure".into())
        } else {
            Ok(())
        }
    }
    fn reset_processing(&mut self) -> Result<(), String> {
        self.probe.reset_gate.pass()?;
        self.probe.record(SafetyTrace::ResetReturned);
        Ok(())
    }
    fn send_midi(&mut self, message: MidiMessage) -> Result<(), String> {
        self.probe.record(SafetyTrace::Midi(message.data));
        match message.data {
            [status, _, velocity] if status & 0xf0 == 0x90 && velocity != 0 => {
                self.note_held = true;
            }
            [status, _, _] if status & 0xf0 == 0x80 || status & 0xf0 == 0x90 => {
                self.note_held = false;
            }
            [status, 120 | 123, _] if status & 0xf0 == 0xb0 => {
                self.note_held = false;
            }
            _ => {}
        }
        Ok(())
    }
    fn process(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        frames: usize,
    ) -> Result<(), String> {
        self.probe.record(SafetyTrace::ProcessEntered(frames));
        self.probe.process_gate.pass()?;
        if left[..frames]
            .iter()
            .chain(&right[..frames])
            .any(|sample| *sample != 0.0)
        {
            self.probe.nonzero_input.store(true, Ordering::Release);
        }
        // Deliberately audible generator AND insert output. Silence is the
        // callback's discard contract, never an assumption about the backend.
        let output = if self.probe.distinct_note_output.load(Ordering::Acquire) {
            if self.note_held {
                self.probe.note_output_quanta.fetch_add(1, Ordering::Release);
                0.25
            } else {
                self.probe.idle_output_quanta.fetch_add(1, Ordering::Release);
                0.75
            }
        } else {
            0.75
        };
        left[..frames].fill(output);
        right[..frames].fill(-output);
        self.probe.process_count.fetch_add(1, Ordering::Release);
        self.probe.record(SafetyTrace::ProcessReturned(frames));
        Ok(())
    }
    fn set_parameter(&mut self, id: u32, normalized: f32) -> Result<(), String> {
        self.parameter = normalized;
        self.probe.record(SafetyTrace::Parameter(id, normalized));
        Ok(())
    }
    fn get_parameter(&mut self, _: u32) -> Result<f32, String> {
        Ok(self.parameter)
    }
    fn save_state(&mut self) -> Result<Vec<u8>, String> {
        self.probe.record(SafetyTrace::SaveEntered);
        self.probe.admin_gate.pass()?;
        self.probe.record(SafetyTrace::SaveReturned);
        Ok(Vec::new())
    }
    fn load_state(&mut self, _: &[u8]) -> Result<(), String> {
        Ok(())
    }
    fn latency_samples(&self) -> u32 {
        self.probe.latency.load(Ordering::Acquire)
    }
    fn tail_samples(&self) -> u32 {
        0
    }
}

// A Pending worker is never passed to consuming shutdown/Drop. The exact owner,
// diagnostics, gates and original directory remain retained for diagnosis.
struct SafetyRetainedFailure {
    _guard: PluginWorkerGuard,
    _control: PluginChainControl,
    _probe: Arc<SafetyProbe>,
    _stats: BridgeStats,
    _original_directory: PathBuf,
}

struct SafetyWorker {
    audio: Option<AudioThreadEndpoint>,
    control: Option<PluginChainControl>,
    guard: Option<PluginWorkerGuard>,
    probe: Arc<SafetyProbe>,
    terminal: Option<RetainedShutdownPoll>,
    joined: Arc<AtomicU32>,
    original_directory: PathBuf,
}

impl SafetyWorker {
    fn spawn(instance: u64) -> Self {
        assert!(!SAFETY_ADMISSION_BLOCKED.load(Ordering::Acquire));
        let original_directory = std::env::current_dir().unwrap();
        let joined = Arc::new(AtomicU32::new(0));
        let probe = Arc::new(SafetyProbe::default());
        let backend_probe = Arc::clone(&probe);
        let chain = PluginChain::spawn_identified_with_backend_factory(
            &[instance],
            move || {
                vec![BackendSlot::new(Box::new(SafetyBackend {
                    probe: backend_probe,
                    parameter: 0.0,
                    note_held: false,
                }))]
            },
            PluginPrepareConfig {
                sample_rate: 48_000.0,
                max_block_frames: 128,
            },
        )
        .expect("spawn stopped safety mock");
        // Immediately assume retained ownership before any assertion, wait or
        // fallible preparation after successful spawn.
        Self {
            audio: Some(chain.audio),
            control: Some(chain.control),
            guard: Some(chain.guard),
            probe,
            terminal: None,
            joined,
            original_directory,
        }
    }
    fn control(&self) -> &PluginChainControl {
        self.control.as_ref().unwrap()
    }
    fn control_mut(&mut self) -> &mut PluginChainControl {
        self.control.as_mut().unwrap()
    }
    fn drain(&mut self) -> Option<RetainedShutdownPoll> {
        self.probe.release_all();
        drop(self.audio.take());
        if self.guard.is_none() {
            return self.terminal;
        }
        let deadline = Instant::now() + SAFETY_DRAIN_BUDGET;
        loop {
            let state = self.guard.as_mut().unwrap().poll_shutdown_retained();
            if state != RetainedShutdownPoll::Pending {
                self.terminal = Some(state);
                self.joined.store(
                    if state == RetainedShutdownPoll::Joined {
                        1
                    } else {
                        2
                    },
                    Ordering::Release,
                );
                let stats = self.control().stats();
                drop(self.guard.take());
                if stats.detached_workers != 0 || stats.shutdown_timeouts != 0 {
                    SAFETY_ADMISSION_BLOCKED.store(true, Ordering::Release);
                    return None;
                }
                return Some(state);
            }
            if Instant::now() >= deadline {
                SAFETY_ADMISSION_BLOCKED.store(true, Ordering::Release);
                let stats = self.control().stats();
                SAFETY_QUARANTINE
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push(SafetyRetainedFailure {
                        _guard: self.guard.take().unwrap(),
                        _control: self.control.take().unwrap(),
                        _probe: Arc::clone(&self.probe),
                        _stats: stats,
                        _original_directory: self.original_directory.clone(),
                    });
                return None;
            }
            thread::sleep(Duration::from_millis(1));
        }
    }
}
impl Drop for SafetyWorker {
    fn drop(&mut self) {
        let result = self.drain();
        if !thread::panicking() {
            assert_eq!(
                result,
                Some(RetainedShutdownPoll::Joined),
                "exact mock owner retained after unproven cleanup"
            );
        }
    }
}

#[derive(Clone, Copy)]
enum OldWork {
    None,
    AdminHeld(usize),
    ProcessHeld,
}

struct StoppedSafetyFixture {
    callback: Option<TestAudioCallback>,
    engine: Option<AudioEngine>,
    workers: Vec<SafetyWorker>,
    timeline: Arc<CompiledTimeline>,
    loop_token: u64,
    _admission: MutexGuard<'static, ()>,
}

impl StoppedSafetyFixture {
    fn new() -> Self {
        Self::with_old_work(OldWork::None)
    }
    fn with_old_work(old: OldWork) -> Self {
        let admission = SAFETY_SERIAL
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert!(
            !SAFETY_ADMISSION_BLOCKED.load(Ordering::Acquire),
            "prior stopped-safety worker retained"
        );
        let (mut engine, mut callback) = AudioEngine::test_engine_with_callback();
        assert!(engine.set_project_session(SAFETY_PROJECT));
        callback.enable_genuine_observation([0x53; 32]).unwrap();
        let mut project = timeline_test_project(4.0);
        let mut channel = timeline_test_channel(SAFETY_CHANNEL, 1);
        channel.instrument_plugin_instance_id = Some(SAFETY_GENERATOR);
        project.channels.push(channel);
        let mut insert = timeline_test_plugin(SAFETY_INSERT);
        insert.role = PluginRole::Effect;
        project
            .plugin_instances
            .extend([timeline_test_plugin(SAFETY_GENERATOR), insert]);
        project.mixer_insert_slots.push(MixerInsertSlotRef {
            track: 2,
            slot: 0,
            plugin_instance_id: SAFETY_INSERT,
        });
        let timeline = compile_timeline_test_project(&project);
        let mut fixture = Self {
            callback: Some(callback),
            engine: Some(engine),
            workers: Vec::with_capacity(4),
            timeline,
            loop_token: 0,
            _admission: admission,
        };
        for instance in [SAFETY_GENERATOR, SAFETY_INSERT] {
            fixture.workers.push(SafetyWorker::spawn(instance));
            let worker = fixture.workers.last().unwrap();
            wait_until(|| {
                worker
                    .control()
                    .plugin_latency_snapshot()
                    .is_some_and(|s| s.revision != 0 && s.slot_is_active(0))
            });
        }
        if !matches!(old, OldWork::None) {
            let worker = &mut fixture.workers[0];
            let audio = worker.audio.as_mut().unwrap();
            assert!(
                audio
                    .configure_timing_plan(PreparedPluginTimingPlan::conservative(48_000).unwrap())
            );
            match old {
                OldWork::AdminHeld(count) => {
                    worker.probe.admin_gate.hold();
                    assert!(worker.control().request_state(0));
                    worker.probe.admin_gate.wait_entered();
                    for index in 0..count {
                        let audio = worker.audio.as_mut().unwrap();
                        assert!(audio.try_send_midi(
                            None,
                            MidiMessage::new([0x90, 60 + (index % 2) as u8, 100], 0)
                        ));
                        assert!(matches!(
                            audio.try_submit(&[0.0; 128], &[0.0; 128]),
                            SubmitStatus::Submitted { .. }
                        ));
                    }
                }
                OldWork::ProcessHeld => {
                    worker.probe.process_gate.hold();
                    let audio = worker.audio.as_mut().unwrap();
                    assert!(audio.try_send_midi(None, MidiMessage::new([0x90, 60, 100], 0)));
                    assert!(matches!(
                        audio.try_submit(&[0.0; 128], &[0.0; 128]),
                        SubmitStatus::Submitted { .. }
                    ));
                    worker.probe.process_gate.wait_entered();
                    let audio = worker.audio.as_mut().unwrap();
                    assert!(audio.try_send_midi(None, MidiMessage::new([0x90, 61, 100], 0)));
                    assert!(matches!(
                        audio.try_submit(&[0.0; 128], &[0.0; 128]),
                        SubmitStatus::Submitted { .. }
                    ));
                }
                OldWork::None => unreachable!(),
            }
            // Reset callback sequence for legitimate installation timing setup.
            // This does NOT empty the worker input queue or unblock the mock.
            assert!(worker.audio.as_mut().unwrap().set_epoch(3));
        }
        let mut generator = fixed_adapter(fixture.workers[0].audio.take().unwrap());
        generator.project_session = SAFETY_PROJECT;
        fixture
            .callback
            .as_mut()
            .unwrap()
            .dsp
            .install_generator_endpoint(
                SAFETY_CHANNEL,
                SAFETY_GENERATOR_ENDPOINT,
                SAFETY_GENERATOR,
                1,
                generator,
                test_pdc_delay(),
            );
        let mut insert = fixed_adapter(fixture.workers[1].audio.take().unwrap());
        insert.project_session = SAFETY_PROJECT;
        fixture
            .callback
            .as_mut()
            .unwrap()
            .dsp
            .install_insert_endpoint(2, SAFETY_INSERT_ENDPOINT, insert);
        let controller = fixture
            .engine
            .as_mut()
            .unwrap()
            .timeline_runtime
            .as_mut()
            .unwrap();
        controller
            .install_with_mixer_resources(
                SAFETY_REVISION,
                Arc::clone(&fixture.timeline),
                Box::new(
                    PreparedMixerGraphDelayBank::new(fixture.timeline.mixer_graph(), 8192).unwrap(),
                ),
            )
            .unwrap();
        let chase = controller
            .prepare_loop_chase(
                &fixture.timeline,
                SAFETY_REVISION,
                0,
                TimelineChaseOptions::default(),
            )
            .unwrap();
        fixture.loop_token = controller.install_loop_chase(chase).unwrap();
        assert_eq!(
            fixture
                .callback
                .as_mut()
                .unwrap()
                .dsp
                .apply_pending_timeline_commands(),
            2
        );
        fixture
    }
    fn callback(&self) -> &TestAudioCallback {
        self.callback.as_ref().unwrap()
    }
    fn callback_mut(&mut self) -> &mut TestAudioCallback {
        self.callback.as_mut().unwrap()
    }
    fn generator(&self) -> &PreparedFixedEndpoint {
        &self.callback().dsp.generator_endpoints[0]
            .as_ref()
            .unwrap()
            .endpoint
    }
    fn generator_mut(&mut self) -> &mut PreparedFixedEndpoint {
        &mut self.callback_mut().dsp.generator_endpoints[0]
            .as_mut()
            .unwrap()
            .endpoint
    }
    fn insert(&self) -> &PreparedFixedEndpoint {
        &self.callback().dsp.insert_endpoints[2]
            .as_ref()
            .unwrap()
            .endpoint
    }
    fn activate(&mut self, epoch: u64, playing: bool) {
        self.activate_with_loop(epoch, playing, 0);
    }
    fn activate_with_loop(&mut self, epoch: u64, playing: bool, loop_end_frame: u64) {
        self.activate_with_loop_at(epoch, playing, loop_end_frame, 0);
    }
    fn activate_with_loop_at(
        &mut self,
        epoch: u64,
        playing: bool,
        loop_end_frame: u64,
        frame: u64,
    ) {
        let topology = self.callback().dsp.plugin_topology_revision;
        let timing = self.engine.as_ref().unwrap().prepare_plugin_timing();
        let request_id = self
            .engine
            .as_ref()
            .unwrap()
            .transport_mailbox
            .try_load()
            .unwrap()
            .request_id;
        let controller = self
            .engine
            .as_mut()
            .unwrap()
            .timeline_runtime
            .as_mut()
            .unwrap();
        let mut chase = controller
            .prepare_chase(
                &self.timeline,
                SAFETY_REVISION,
                epoch,
                frame,
                TimelineChaseOptions::default(),
            )
            .unwrap();
        chase.plugin_timing = timing;
        chase.plugin_topology_revision = topology;
        controller.install_chase(chase).unwrap();
        controller
            .activate_transport(
                TimelineTransportActivationSpec {
                    revision: SAFETY_REVISION,
                    target_epoch: epoch,
                    minimum_epoch: epoch,
                    frame,
                    beat_q32: beat_to_q32(frame_as_beat_f64(frame)),
                    loop_start_frame: 0,
                    loop_end_frame,
                    loop_start_q32: 0,
                    loop_end_q32: beat_to_q32(frame_as_beat_f64(loop_end_frame)),
                    loop_token: self.loop_token,
                    loop_enabled: loop_end_frame != 0,
                    playing,
                    mixer_pan_release: TimelineMixerPanRelease::EMPTY,
                },
                request_id,
            )
            .unwrap();
        let status = Arc::clone(&self.engine.as_ref().unwrap().status);
        let callback = self.callback_mut();
        callback.dsp.requested_plugin_timing_revision = timing.revision;
        crate::realtime_test_alloc::assert_no_alloc_or_drop(|| {
            assert_eq!(callback.dsp.apply_pending_timeline_commands(), 2);
            callback.dsp.refresh_pdc_plan(&status, 128);
            callback
                .transport
                .apply_pending_timeline_activation(&status, &mut callback.dsp);
        });
        assert_eq!(callback.transport.epoch, epoch);
        assert_eq!(callback.transport.request.playing, playing);
        assert!(callback.dsp.mixer_graph_binding_is_exact());
    }
    fn pump(&mut self, frames: usize) {
        let engine = self.engine.as_ref().unwrap();
        let callback = self.callback.as_mut().unwrap();
        crate::realtime_test_alloc::assert_no_alloc_or_drop(|| callback.pump(engine, frames));
        assert!(
            callback
                .last_output()
                .iter()
                .all(|frame| *frame == [0.0; 2]),
            "safety-only consumed output must be exactly zero"
        );
    }
    fn settle(&self, epoch: u64) {
        wait_until(|| {
            self.callback()
                .genuine_snapshot()
                .endpoints
                .iter()
                .all(|e| e.input_work_quiescent(epoch))
        });
    }
    fn stats(&self) -> [FixedQuantumStats; 2] {
        [self.generator().stats(), self.insert().stats()]
    }
    fn stamp(&self) -> MidiGeneratorRouteStamp {
        MidiGeneratorRouteStamp {
            project_session: SAFETY_PROJECT,
            channel_id: SAFETY_CHANNEL,
            endpoint_id: SAFETY_GENERATOR_ENDPOINT,
            plugin_instance_id: SAFETY_GENERATOR,
            slot: None,
        }
    }
}

impl Drop for StoppedSafetyFixture {
    fn drop(&mut self) {
        // Release every gate BEFORE callback/engine teardown, even on unwind.
        for worker in &self.workers {
            worker.probe.release_all();
        }
        drop(self.callback.take());
        drop(self.engine.take());
        let mut clean = true;
        for worker in &mut self.workers {
            clean &= worker.drain() == Some(RetainedShutdownPoll::Joined);
        }
        if !thread::panicking() {
            assert!(clean, "stopped safety teardown retained an unproven worker");
        }
    }
}

fn assert_callback_batch_follows_reset(trace: &[SafetyTrace]) {
    let preflight = trace
        .iter()
        .position(|event| *event == SafetyTrace::ResetPreflight)
        .unwrap();
    let returned = trace
        .iter()
        .position(|event| *event == SafetyTrace::ResetReturned)
        .unwrap();
    let reset: Vec<_> = trace[preflight + 1..returned]
        .iter()
        .filter_map(|event| {
            if let SafetyTrace::Midi(data) = event {
                Some(*data)
            } else {
                None
            }
        })
        .collect();
    let expected_reset: Vec<_> = (0_u8..16)
        .flat_map(|channel| [64, 123, 120].map(move |cc| [0xb0 | channel, cc, 0]))
        .collect();
    assert_eq!(
        reset, expected_reset,
        "independent worker reset owns precisely 48 safety messages"
    );
    let process = trace[returned + 1..]
        .iter()
        .position(|event| matches!(event, SafetyTrace::ProcessEntered(128)))
        .unwrap()
        + returned
        + 1;
    let staged: Vec<_> = trace[returned + 1..process]
        .iter()
        .filter_map(|event| {
            if let SafetyTrace::Midi(data) = event {
                Some(*data)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        staged,
        (0_u8..16)
            .map(|channel| [0xb0 | channel, 123, 0])
            .collect::<Vec<_>>(),
        "callback CC123 must follow reset return and precede ordinary Q128 Process"
    );
}

#[test]
fn stopped_epoch_safety_four_32_frame_callbacks_deliver_and_restore_live_admission() {
    let mut f = StoppedSafetyFixture::new();
    f.activate(2, false);
    let staged = f.callback().genuine_snapshot();
    assert_eq!(staged.paused_safety_services, 2);
    assert!(staged.callback_work_pending());
    assert!(!staged.paused_work_quiescent(SAFETY_REVISION, 2));
    for endpoint in &staged.endpoints {
        assert_eq!(endpoint.staged_system_events, 16);
        assert!(endpoint.live_lane_faulted);
        assert_eq!(endpoint.stopped_epoch_safety_epoch, Some(2));
        assert_eq!(endpoint.stopped_epoch_safety_remaining_frames, 128);
        assert!(!endpoint.stopped_epoch_safety_failed);
    }
    assert!(
        !f.generator_mut()
            .stage(FrameEvent::midi(0, None, [0x90, 60, 100]))
    );
    for turn in 1..=4 {
        f.pump(32);
        let observed = f.callback().genuine_snapshot();
        for endpoint in &observed.endpoints {
            assert!(
                !endpoint.live_lane_faulted,
                "real prepare_callback transfers staging"
            );
            assert_eq!(endpoint.staged_system_events, 0);
            assert_eq!(endpoint.fixed.stats.callback_frames, turn * 32);
            assert_eq!(endpoint.fixed.stats.submitted_quanta, u64::from(turn == 4));
            assert_eq!(
                endpoint.stopped_epoch_safety_remaining_frames,
                128 - turn as usize * 32
            );
        }
        if turn < 4 {
            assert!(observed.callback_work_pending());
            assert!(!observed.paused_work_quiescent(SAFETY_REVISION, 2));
        }
    }
    f.settle(2);
    assert!(
        f.callback()
            .genuine_snapshot()
            .paused_work_quiescent(SAFETY_REVISION, 2)
    );
    for worker in &f.workers {
        assert_eq!(worker.control().stats().submitted, 1);
        assert_eq!(worker.control().stats().completed, 1);
        assert_eq!(worker.probe.process_count.load(Ordering::Acquire), 1);
        assert_callback_batch_follows_reset(&worker.probe.trace());
    }
    let complete = f.stats();
    f.pump(32);
    assert_eq!(
        f.stats(),
        complete,
        "a fifth callback must not prime or drain output"
    );
    let segments = f.callback_mut().take_genuine_segments();
    assert_eq!(segments.first().unwrap().before.paused_safety_services, 2);
    assert_eq!(segments.last().unwrap().after.paused_safety_services, 0);
    assert!(
        segments
            .iter()
            .all(|segment| !segment.playing && !segment.timeline_ready)
    );
    // Only after independently proving the settled baseline, exercise the real
    // Live staging API. Never clear a latch or infer acceptance from a counter.
    assert!(
        f.generator_mut()
            .stage(FrameEvent::midi(0, None, [0x90, 60, 100]))
    );
    assert!(
        !f.callback()
            .genuine_snapshot()
            .paused_work_quiescent(SAFETY_REVISION, 2)
    );
}

#[test]
fn stopped_epoch_safety_partition_matrix_is_exactly_one_quantum_per_endpoint() {
    let partitions: &[&[usize]] = &[
        &[1],
        &[31],
        &[32],
        &[64],
        &[127],
        &[128],
        &[129],
        &[2048],
        &[7, 31, 1, 64, 25],
    ];
    for pattern in partitions {
        let mut f = StoppedSafetyFixture::new();
        f.activate(2, false);
        let mut serviced = 0;
        let mut turn = 0;
        while serviced < 128 {
            let frames = pattern[turn % pattern.len()];
            f.pump(frames);
            serviced += frames.min(128 - serviced);
            turn += 1;
            for stats in f.stats() {
                assert_eq!(
                    stats.callback_frames, serviced as u64,
                    "partition {pattern:?}"
                );
                assert_eq!(stats.submitted_quanta, u64::from(serviced == 128));
            }
        }
        f.settle(2);
        assert!(
            f.callback()
                .genuine_snapshot()
                .paused_work_quiescent(SAFETY_REVISION, 2)
        );
        let complete = f.stats();
        for frames in [1, 32, 129, 2048] {
            f.pump(frames);
        }
        assert_eq!(
            f.stats(),
            complete,
            "no further quantum for partition {pattern:?}"
        );
    }
}

#[test]
fn stopped_epoch_safety_freezes_post_activation_cursors_voices_and_every_pdc_history() {
    let mut f = StoppedSafetyFixture::new();
    f.activate(2, false);
    // The legitimate activation reset has already committed. Poison only now,
    // so this comparison says nothing false about state that activation resets.
    f.callback_mut()
        .dsp
        .test_poison_restoration_histories(SAFETY_CHANNEL);
    let before = f.callback().dsp.test_restoration_histories(SAFETY_CHANNEL);
    before.assert_poisoned();
    let snapshot = f.callback().genuine_snapshot();
    let cursors = |callback: &TestAudioCallback| {
        let dsp = &callback.dsp;
        (
            callback.transport.timeline_frame,
            callback.transport.beat_q32,
            dsp.timeline_executor.next_frame(),
            dsp.timeline_automation.next_frame(),
            dsp.timeline_runtime.as_ref().unwrap().next_frame(),
            dsp.timeline_plan.cursor,
            dsp.timeline_plan.len,
            dsp.timeline_plan.automation_chase_len,
            dsp.timeline_plan.automation_block_ready,
            dsp.voices[0].phase,
            dsp.voices[0].envelope,
            dsp.audio_voices[0].source_position,
        )
    };
    let frozen_cursors = cursors(f.callback());
    let frozen_tracks = f.callback().dsp.track_block.to_vec();
    for _ in 0..4 {
        f.pump(32);
    }
    f.settle(2);
    let after = f.callback().dsp.test_restoration_histories(SAFETY_CHANNEL);
    assert_eq!(after.raw, before.raw);
    assert_eq!(after.generator, before.generator);
    assert_eq!(after.graph, before.graph);
    assert_eq!(after.graph_fingerprint, before.graph_fingerprint);
    assert_eq!(after.active_voices, before.active_voices);
    assert_eq!(after.active_audio_voices, before.active_audio_voices);
    assert_eq!(cursors(f.callback()), frozen_cursors);
    assert_eq!(
        f.callback().dsp.track_block.as_ref(),
        frozen_tracks.as_slice()
    );
    let after_snapshot = f.callback().genuine_snapshot();
    assert_eq!(after_snapshot.raw_delays, snapshot.raw_delays);
    assert_eq!(after_snapshot.graph_identity, snapshot.graph_identity);
    assert_eq!(after_snapshot.pdc_revision, snapshot.pdc_revision);
    assert_eq!(after_snapshot.track_gains, snapshot.track_gains);
    assert_eq!(after_snapshot.track_pans, snapshot.track_pans);
    assert_eq!(after_snapshot.track_muted, snapshot.track_muted);
    assert_eq!(after_snapshot.track_solo, snapshot.track_solo);
    assert!(after_snapshot.device_frame > snapshot.device_frame);
    for worker in &f.workers {
        assert_eq!(worker.probe.process_count.load(Ordering::Acquire), 1);
        assert!(
            !worker.probe.nonzero_input.load(Ordering::Acquire),
            "insert safety input must be zero even when mixer buses contain sentinels"
        );
    }
}

#[test]
fn stopped_epoch_safety_already_current_preserves_partial_budget_and_fault_ownership() {
    for partitions in [&[63, 1, 127, 1][..], &[2048, 2048][..]] {
        let mut f = StoppedSafetyFixture::new();
        f.activate(2, false);
        f.pump(64);
        assert_eq!(f.generator().partial_quantum_usage.system, 16);
        let resets = f.generator().stats().epoch_resets;
        f.callback_mut().dsp.generator_endpoints[0]
            .as_mut()
            .unwrap()
            .suppress_output_frames = 777;
        f.callback_mut().dsp.midi_route_faulted = 1;
        let status = Arc::clone(&f.engine.as_ref().unwrap().status);
        f.callback_mut()
            .dsp
            .apply_transport_epoch(&status, 2, 0, false);
        assert_eq!(f.generator().input_phase_frames(), 64);
        assert_eq!(f.generator().stats().epoch_resets, resets);
        assert_eq!(
            f.generator().stopped_epoch_safety.unwrap().remaining_frames,
            192
        );
        assert_eq!(f.generator().event_rejection_defer_offset(), 64);
        assert_eq!(
            f.callback().dsp.generator_endpoints[0]
                .as_ref()
                .unwrap()
                .suppress_output_frames,
            777
        );
        assert_eq!(f.callback().dsp.midi_route_faulted, 1);
        let before = f.stats();
        for &frames in partitions {
            f.pump(frames);
        }
        f.settle(2);
        for (old, new) in before.into_iter().zip(f.stats()) {
            assert_eq!(new.callback_frames - old.callback_frames, 192);
            assert_eq!(new.submitted_quanta - old.submitted_quanta, 2);
        }
        assert!(f.generator().stopped_epoch_safety.is_none());
        assert!(f.insert().stopped_epoch_safety.is_none());
        let complete = f.stats();
        f.pump(2048);
        assert_eq!(f.stats(), complete);
    }
}

#[test]
fn stopped_epoch_safety_worker_gates_remain_nonquiescent_until_read_only_settlement() {
    for gate in ["reset", "process", "admin"] {
        let mut f = StoppedSafetyFixture::new();
        let probe = Arc::clone(&f.workers[0].probe);
        match gate {
            "reset" => probe.reset_gate.hold(),
            "process" => probe.process_gate.hold(),
            "admin" => {
                probe.admin_gate.hold();
                assert!(f.workers[0].control().request_state(0));
                probe.admin_gate.wait_entered();
            }
            _ => unreachable!(),
        }
        f.activate(2, false);
        if gate == "reset" {
            probe.reset_gate.wait_entered();
        }
        f.pump(128);
        if gate == "process" {
            probe.process_gate.wait_entered();
        }
        let pending = f.callback().genuine_snapshot();
        assert!(!pending.callback_work_pending());
        assert!(!pending.paused_work_quiescent(SAFETY_REVISION, 2));
        let generator = pending
            .endpoints
            .iter()
            .find(|e| e.channel_id == Some(SAFETY_CHANNEL))
            .unwrap();
        assert_eq!(generator.stopped_epoch_safety_epoch, None);
        assert!(!generator.input_work_quiescent(2));
        assert!(
            generator.fixed.bridge.input_pending != 0 || generator.fixed.bridge.worker_busy != 0
        );
        let complete = f.stats();
        let device_frame = f.callback().transport.device_frame;
        probe.release_all();
        f.settle(2); // No callback, adapter process, or extra input to settle.
        assert!(
            f.callback()
                .genuine_snapshot()
                .paused_work_quiescent(SAFETY_REVISION, 2)
        );
        assert_eq!(f.stats(), complete);
        assert_eq!(f.callback().transport.device_frame, device_frame);
        assert_callback_batch_follows_reset(&probe.trace());
        if gate == "admin" {
            let trace = probe.trace();
            let saved = trace
                .iter()
                .position(|e| *e == SafetyTrace::SaveReturned)
                .unwrap();
            let reset = trace
                .iter()
                .position(|e| *e == SafetyTrace::ResetPreflight)
                .unwrap();
            assert!(
                saved < reset,
                "epoch arrived after top-of-turn read; post-admin/post-pop recheck must reset"
            );
        }
    }
}

#[test]
fn stopped_epoch_safety_discards_inflight_old_output_and_rejects_queued_old_events() {
    let mut f = StoppedSafetyFixture::with_old_work(OldWork::ProcessHeld);
    let probe = Arc::clone(&f.workers[0].probe);
    f.activate(2, false);
    f.pump(128);
    assert_eq!(f.workers[0].control().stats().submitted, 3);
    assert!(
        !f.callback()
            .genuine_snapshot()
            .paused_work_quiescent(SAFETY_REVISION, 2)
    );
    let callback_stats = f.stats();
    probe.process_gate.release();
    f.settle(2);
    let stats = f.workers[0].control().stats();
    assert_eq!(stats.dropped_old_epoch_outputs, 1);
    assert_eq!(stats.dropped_old_epoch_inputs, 1);
    assert_eq!(stats.completed, 1);
    assert_eq!(f.stats(), callback_stats);
    let trace = probe.trace();
    assert!(trace.contains(&SafetyTrace::Midi([0x90, 60, 100])));
    assert!(!trace.contains(&SafetyTrace::Midi([0x90, 61, 100])));
    assert_eq!(
        trace
            .iter()
            .filter(|e| matches!(e, SafetyTrace::ProcessReturned(128)))
            .count(),
        2
    );
    let old_return = trace
        .iter()
        .position(|e| *e == SafetyTrace::ProcessReturned(128))
        .unwrap();
    let reset = trace
        .iter()
        .position(|e| *e == SafetyTrace::ResetPreflight)
        .unwrap();
    assert!(
        old_return < reset,
        "already admitted Process may finish before the new reset"
    );
    assert_callback_batch_follows_reset(&trace);
    assert!(
        f.callback()
            .genuine_snapshot()
            .paused_work_quiescent(SAFETY_REVISION, 2)
    );
}

#[test]
fn stopped_epoch_safety_full_old_input_queue_fails_once_and_fences_later_owners() {
    let mut f = StoppedSafetyFixture::with_old_work(OldWork::AdminHeld(
        crate::plugin_timing::PLUGIN_QUEUE_CAPACITY,
    ));
    assert_eq!(f.workers[0].control().stats().submitted, 48);
    f.activate(2, false);
    f.pump(128);
    let failed = f.generator().stopped_epoch_safety.unwrap();
    assert!(failed.failed);
    assert!(failed.remaining_frames > 0);
    assert_eq!(f.generator().stats().completed_quanta, 1);
    assert_eq!(f.generator().stats().submitted_quanta, 0);
    assert_eq!(f.generator().stats().bridge_gaps, 1);
    assert_eq!(f.workers[0].control().stats().input_overflows, 1);
    assert_eq!(
        f.callback().dsp.plugin_fault.unwrap().reason,
        PluginProcessingFaultReason::InputLoss
    );
    assert_eq!(
        f.insert().stats().callbacks,
        0,
        "same-segment later endpoint is fenced"
    );
    assert!(
        !f.callback()
            .genuine_snapshot()
            .paused_work_quiescent(SAFETY_REVISION, 2)
    );
    let before = f.stats();
    f.pump(2048);
    assert_eq!(f.stats(), before, "no automatic retry or rearm after gap");
    f.workers[0].probe.admin_gate.release();
    wait_until(|| f.workers[0].control().stats().dropped_old_epoch_inputs == 48);
    assert!(
        !f.callback()
            .genuine_snapshot()
            .paused_work_quiescent(SAFETY_REVISION, 2)
    );
}

#[test]
fn stopped_epoch_safety_event_failure_matrix_keeps_obligation_and_fault_evidence() {
    for failure in ["invalid", "rejected", "overflow", "deferred-rejected"] {
        let mut f = StoppedSafetyFixture::new();
        f.activate(2, false);
        match failure {
            "invalid" => {
                f.generator_mut().events.scratch.system.pending[0] = FrameEvent {
                    sample_offset: 0,
                    kind: FrameEventKind::Parameter {
                        slot: 0,
                        id: 9,
                        normalized: f32::NAN,
                        edit_id: None,
                    },
                }
            }
            "rejected" | "deferred-rejected" => {
                f.generator_mut().events.scratch.system.pending[0] =
                    FrameEvent::midi(0, Some(u8::MAX), [0xb0, 123, 0])
            }
            "overflow" => {
                // Negative-control corruption of already staged input: exercise
                // the real adapter's finite pending-event overflow detection.
                // No obligation, latch, receipt or diagnostic is manually cleared.
                for _ in 0..MAX_FRAME_EVENTS_PER_QUANTUM {
                    f.generator_mut()
                        .events
                        .scratch
                        .timeline
                        .push_preflighted(FrameEvent::midi(0, None, [0x90, 60, 1]));
                }
            }
            _ => unreachable!(),
        }
        if failure == "deferred-rejected" {
            f.pump(64);
            assert_eq!(f.generator().stats().endpoint_event_rejections, 0);
            let status = Arc::clone(&f.engine.as_ref().unwrap().status);
            f.callback_mut()
                .dsp
                .apply_transport_epoch(&status, 2, 0, false);
            assert_eq!(
                f.generator().stopped_epoch_safety.unwrap().remaining_frames,
                192
            );
            f.pump(2048);
            assert_eq!(
                f.generator().stats().callback_frames,
                128,
                "a first-quantum failure must fence the deferred second quantum in this same segment"
            );
        } else {
            f.pump(128);
        }
        let stats = f.generator().stats();
        match failure {
            "invalid" => assert_eq!(stats.invalid_frame_events, 1),
            "rejected" | "deferred-rejected" => assert_eq!(stats.endpoint_event_rejections, 1),
            "overflow" => assert!(stats.frame_event_overflows > 0),
            _ => unreachable!(),
        }
        assert!(f.generator().stopped_epoch_safety.unwrap().failed);
        assert_eq!(
            f.callback().dsp.plugin_fault.unwrap().reason,
            PluginProcessingFaultReason::EventLoss
        );
        assert_eq!(f.insert().stats().submitted_quanta, 0);
        assert!(
            !f.callback()
                .genuine_snapshot()
                .paused_work_quiescent(SAFETY_REVISION, 2)
        );
        let before = f.stats();
        f.pump(2048);
        assert_eq!(f.stats(), before);
    }
}

#[test]
fn stopped_epoch_safety_reset_fault_cannot_be_mistaken_for_delivery() {
    let mut f = StoppedSafetyFixture::new();
    let original_snapshot = f.workers[0].control().plugin_latency_snapshot().unwrap();
    assert!(original_snapshot.slot_is_active(0));
    f.workers[0]
        .probe
        .reset_failure
        .store(true, Ordering::Release);
    f.activate(2, false);
    // A reset fault and its coherent inactive metadata publication are separate
    // worker writes. Await both so the real callback deterministically sees the
    // changed graph identity before its later generic worker-health check.
    wait_until(|| {
        let stats = f.workers[0].control().stats();
        stats.reset_faults == 1
            && stats.faults == 1
            && f.workers[0]
                .control()
                .plugin_latency_snapshot()
                .is_some_and(|snapshot| {
                    snapshot.revision > original_snapshot.revision && !snapshot.slot_is_active(0)
                })
    });
    let mut fault_event = None;
    wait_until(|| {
        while let Some(event) = f.workers[0].control_mut().try_next_event() {
            if matches!(
                event,
                crate::plugins::plugin_runtime::RuntimeEvent::SlotFault { .. }
            ) {
                fault_event = Some(event);
                return true;
            }
        }
        false
    });
    assert!(matches!(
        &fault_event,
        Some(crate::plugins::plugin_runtime::RuntimeEvent::SlotFault { slot: 0, message })
            if message == "transport epoch 2 reset preflight failed: deliberate stopped safety reset failure"
    ));
    assert_eq!(
        f.workers[0].probe.trace(),
        vec![SafetyTrace::ResetPreflight]
    );
    f.pump(128);
    let fault = f.callback().dsp.plugin_fault.unwrap();
    assert_eq!(
        (fault.endpoint_id, fault.epoch, fault.reason),
        (
            SAFETY_GENERATOR_ENDPOINT,
            2,
            PluginProcessingFaultReason::LatencyDrift
        )
    );
    assert!(f.generator().stopped_epoch_safety.is_some());
    assert!(f.stats().iter().all(|stats| stats.submitted_quanta == 0));
    assert!(
        f.workers
            .iter()
            .all(|worker| worker.control().stats().submitted == 0)
    );
    assert_eq!(f.workers[0].probe.process_count.load(Ordering::Acquire), 0);
    assert!(
        !f.callback()
            .genuine_snapshot()
            .paused_work_quiescent(SAFETY_REVISION, 2)
    );
    let before = f.stats();
    f.pump(128);
    assert_eq!(f.stats(), before);
    assert_eq!(
        f.workers[0].probe.trace(),
        vec![SafetyTrace::ResetPreflight]
    );
}

#[test]
fn stopped_epoch_safety_existing_fault_suspension_latency_and_raw_budget_fence_submission() {
    for gate in ["fault", "suspension", "latency", "raw-budget"] {
        let mut f = StoppedSafetyFixture::new();
        f.activate(2, false);
        let frames = match gate {
            "fault" => {
                f.callback_mut().dsp.latch_plugin_processing_fault(
                    SAFETY_GENERATOR_ENDPOINT,
                    0,
                    PluginProcessingFaultReason::InputLoss,
                );
                128
            }
            "suspension" => {
                f.callback_mut().dsp.begin_plugin_topology_replan();
                128
            }
            "latency" => {
                let revision = f.workers[0]
                    .control()
                    .plugin_latency_snapshot()
                    .unwrap()
                    .revision;
                f.workers[0].probe.latency.store(32, Ordering::Release);
                assert!(
                    f.workers[0]
                        .control()
                        .set_slot_config(0, SlotConfig::default())
                );
                wait_until(|| {
                    f.workers[0]
                        .control()
                        .plugin_latency_snapshot()
                        .is_some_and(|snapshot| snapshot.revision != revision)
                });
                128
            }
            "raw-budget" => {
                // Genuine raw telemetry authorizes selecting the existing
                // smaller profile. A fresh activation owns that profile.
                f.pump(1);
                f.engine
                    .as_mut()
                    .unwrap()
                    .request_plugin_timing_profile(128)
                    .unwrap();
                f.activate(3, false);
                129
            }
            _ => unreachable!(),
        };
        let before = f.stats();
        f.pump(frames);
        assert_eq!(
            f.stats(),
            before,
            "gate {gate} must precede all endpoint advancement"
        );
        assert!(f.generator().stopped_epoch_safety.is_some());
        let snapshot = f.callback().genuine_snapshot();
        assert!(!snapshot.paused_work_quiescent(SAFETY_REVISION, snapshot.epoch));
        if gate == "suspension" {
            assert!(snapshot.suspended);
        } else {
            let reason = snapshot.fault.unwrap().reason;
            assert_eq!(
                reason,
                match gate {
                    "fault" => PluginProcessingFaultReason::InputLoss,
                    "latency" => PluginProcessingFaultReason::LatencyDrift,
                    "raw-budget" => PluginProcessingFaultReason::CallbackBudgetExceeded,
                    _ => unreachable!(),
                }
            );
        }
    }
}

fn safety_edit(index: usize) -> ParameterEditSubmission {
    let (kind, endpoint, instance) = if index == 0 {
        (
            ParameterEndpointKind::Generator,
            SAFETY_GENERATOR_ENDPOINT,
            SAFETY_GENERATOR,
        )
    } else {
        (
            ParameterEndpointKind::Insert,
            SAFETY_INSERT_ENDPOINT,
            SAFETY_INSERT,
        )
    };
    ParameterEditSubmission {
        edit_id: crate::plugin_parameter_edit::ParameterEditId(900 + index as u64),
        route: ParameterEditRoute {
            project_session: SAFETY_PROJECT,
            endpoint: crate::plugin_parameter_edit::ParameterEndpoint { kind, id: endpoint },
            instance_id: instance,
            slot: 0,
            parameter_id: 9,
        },
        normalized: 0.625,
    }
}

#[test]
fn stopped_epoch_safety_shared_midi_epoch_and_parameter_owners_decrement_once() {
    for midi_owner in [false, true] {
        let mut f = StoppedSafetyFixture::new();
        // Arm the stamped external-MIDI owner before Timeline ownership exists.
        // The real stopped activation then adds epoch ownership on that same endpoint.
        if midi_owner {
            let stamp = f.stamp();
            f.callback_mut().dsp.panic_exact_midi_destination(stamp);
            assert_eq!(
                f.callback().dsp.paused_midi_safety[0]
                    .unwrap()
                    .remaining_frames,
                128
            );
        }
        f.activate(2, false);
        assert!(
            !f.callback()
                .dsp
                .timeline_runtime
                .as_ref()
                .unwrap()
                .stats()
                .ownership_needs_resync
        );
        assert!(f.callback().dsp.mixer_graph_binding_is_exact());
        for index in 0..2 {
            assert!(
                f.engine
                    .as_mut()
                    .unwrap()
                    .edit_plugin_parameter(safety_edit(index))
            );
        }
        // Admit through the real audio command queue, exposing the marker state
        // before processing without modifying any marker/count/receipt by hand.
        let callback = f.callback_mut();
        crate::realtime_test_alloc::assert_no_alloc_or_drop(|| {
            process_commands(
                &mut callback.dsp,
                &mut callback.commands,
                &mut callback.retired_assets,
                &mut callback.asset_events,
            )
        });
        assert_eq!(f.callback().dsp.admitted_live_edit_endpoint_count, 2);
        assert!(f.generator().has_admitted_live_edit_marker());
        assert!(f.insert().has_admitted_live_edit_marker());
        f.pump(32);
        assert_eq!(f.callback().dsp.admitted_live_edit_endpoint_count, 2);
        f.pump(96);
        assert_eq!(f.callback().dsp.admitted_live_edit_endpoint_count, 0);
        assert!(!f.generator().has_admitted_live_edit_marker());
        assert!(!f.insert().has_admitted_live_edit_marker());
        assert!(f.generator().stopped_epoch_safety.is_none());
        assert!(f.callback().dsp.paused_midi_safety[0].is_none());
        for stats in f.stats() {
            assert_eq!(stats.callback_frames, 128);
            assert_eq!(
                stats.submitted_quanta, 1,
                "three owners share one physical quantum"
            );
        }
        for index in 0..2 {
            let mut receipt = None;
            wait_until(|| {
                receipt = f.workers[index]
                    .control_mut()
                    .try_next_parameter_edit_receipt();
                receipt.is_some()
            });
            assert!(
                matches!(receipt.unwrap(), ParameterEditReceipt::Applied { edit_id, id: 9, effective, .. }
                if edit_id.get() == 900 + index as u64 && effective == 0.625)
            );
            assert_eq!(f.workers[index].control().outstanding_parameter_edits(), 0);
            assert!(
                f.workers[index]
                    .control_mut()
                    .try_next_parameter_edit_receipt()
                    .is_none()
            );
        }
        f.settle(2);
        assert!(
            f.callback()
                .genuine_snapshot()
                .paused_work_quiescent(SAFETY_REVISION, 2)
        );
        assert!(
            f.engine
                .as_mut()
                .unwrap()
                .next_plugin_parameter_edit_callback_receipt()
                .is_none()
        );
        let before = f.stats();
        f.pump(128);
        assert_eq!(f.stats(), before);
        assert_eq!(f.callback().dsp.admitted_live_edit_endpoint_count, 0);
    }
}

#[test]
fn stopped_epoch_safety_monitor_progress_prevents_duplicate_silent_service() {
    let mut f = StoppedSafetyFixture::new();
    let stamp = f.stamp();
    f.callback_mut().dsp.panic_exact_midi_destination(stamp);
    f.activate(2, false);
    assert!(
        !f.callback()
            .dsp
            .timeline_runtime
            .as_ref()
            .unwrap()
            .stats()
            .ownership_needs_resync
    );
    assert!(f.callback().dsp.mixer_graph_binding_is_exact());
    let (_sender, receiver) = crate::midi_device::test_input_mailbox(8, 33);
    let route = PreparedMidiInputRoute::new(receiver, f.stamp()).unwrap();
    f.callback_mut().dsp.install_midi_input(5, route);
    assert!(
        f.engine
            .as_mut()
            .unwrap()
            .edit_plugin_parameter(safety_edit(0))
    );
    assert!(
        !f.callback()
            .dsp
            .timeline_runtime
            .as_ref()
            .unwrap()
            .stats()
            .ownership_needs_resync
    );
    assert!(f.callback().dsp.mixer_graph_binding_is_exact());
    // Monitor may be audible by existing policy. This test inspects physical
    // progress and uses the raw production pump without the safety-only oracle.
    let engine = f.engine.as_ref().unwrap();
    let callback = f.callback.as_mut().unwrap();
    crate::realtime_test_alloc::assert_no_alloc_or_drop(|| callback.pump(engine, 128));
    assert!(
        f.callback().dsp.meter_graph_rendered,
        "the real monitor graph path must have executed"
    );
    let segments = f.callback_mut().take_genuine_segments();
    assert_eq!(segments.len(), 1);
    assert!(!segments[0].playing && !segments[0].timeline_ready);
    assert!(!segments[0].after.needs_resync && segments[0].after.fault.is_none());
    assert_eq!(f.generator().stats().callback_frames, 128);
    assert_eq!(f.generator().stats().submitted_quanta, 1);
    assert_eq!(f.insert().stats().callback_frames, 128);
    assert!(f.generator().stopped_epoch_safety.is_none());
    assert!(f.callback().dsp.paused_midi_safety[0].is_none());
    assert_eq!(f.callback().dsp.admitted_live_edit_endpoint_count, 0);
    let mut receipt = None;
    wait_until(|| {
        receipt = f.workers[0].control_mut().try_next_parameter_edit_receipt();
        receipt.is_some()
    });
    assert!(matches!(
        receipt,
        Some(ParameterEditReceipt::Applied { .. })
    ));
    assert!(
        f.workers[0]
            .control_mut()
            .try_next_parameter_edit_receipt()
            .is_none()
    );
    f.settle(2);
}

#[test]
fn stopped_epoch_safety_endpoint_lifecycle_retires_exact_obligation_without_slot_inheritance() {
    for change in [
        "remove-generator",
        "remove-insert",
        "replace-generator",
        "replace-insert",
        "clear",
        "stamped-remove",
        "project-clear",
    ] {
        let mut f = StoppedSafetyFixture::new();
        f.activate(2, false);
        f.pump(32);
        assert_eq!(
            f.generator().stopped_epoch_safety.unwrap().remaining_frames,
            96
        );
        match change {
            "remove-generator" => f
                .callback_mut()
                .dsp
                .remove_generator_endpoint(SAFETY_CHANNEL),
            "remove-insert" => f.callback_mut().dsp.remove_insert_endpoint(2),
            "clear" | "project-clear" => {
                f.callback_mut().dsp.clear_generator_endpoints(501);
                f.callback_mut().dsp.clear_insert_endpoints(502);
                if change == "project-clear" {
                    assert!(f.engine.as_mut().unwrap().set_project_session(8));
                }
            }
            "stamped-remove" => {
                let stamp = GeneratorRemovalStamp {
                    operation_id: 503,
                    engine_session: f.engine.as_ref().unwrap().engine_session(),
                    project_session: SAFETY_PROJECT,
                    channel_id: SAFETY_CHANNEL,
                    endpoint_id: SAFETY_GENERATOR_ENDPOINT,
                    plugin_instance_id: SAFETY_GENERATOR,
                    stopped_epoch: 2,
                };
                assert!(
                    f.callback()
                        .dsp
                        .validate_generator_removal_stamp(stamp)
                        .is_ok()
                );
                f.callback_mut()
                    .dsp
                    .remove_generator_endpoint_stamped(stamp);
                assert!(f.callback().dsp.generator_endpoints[0].is_none());
            }
            "replace-generator" | "replace-insert" => {
                let generator = change == "replace-generator";
                let instance = if generator {
                    SAFETY_GENERATOR
                } else {
                    SAFETY_INSERT
                };
                f.workers.push(SafetyWorker::spawn(instance));
                wait_until(|| {
                    f.workers[2]
                        .control()
                        .plugin_latency_snapshot()
                        .is_some_and(|s| s.slot_is_active(0))
                });
                let mut replacement = fixed_adapter(f.workers[2].audio.take().unwrap());
                replacement.project_session = SAFETY_PROJECT;
                if generator {
                    f.callback_mut().dsp.install_generator_endpoint(
                        SAFETY_CHANNEL,
                        170,
                        instance,
                        1,
                        replacement,
                        test_pdc_delay(),
                    );
                    assert!(f.generator().stopped_epoch_safety.is_none());
                } else {
                    f.callback_mut()
                        .dsp
                        .install_insert_endpoint(2, 180, replacement);
                    assert!(f.insert().stopped_epoch_safety.is_none());
                }
            }
            _ => unreachable!(),
        }
        // Inspect the actually retired physical endpoint, never a reused index.
        let retired = f
            .engine
            .as_mut()
            .unwrap()
            .retired_insert_endpoints
            .pop()
            .unwrap();
        assert_eq!(
            retired
                ._endpoint
                .stopped_epoch_safety
                .unwrap()
                .remaining_frames,
            96
        );
        assert_eq!(retired._endpoint.stats().submitted_quanta, 0);
        f.pump(128);
        assert!(
            f.workers
                .iter()
                .all(|worker| worker.control().stats().submitted == 0)
        );
        assert!(
            !f.callback()
                .genuine_snapshot()
                .paused_work_quiescent(SAFETY_REVISION, 2)
        );
        drop(retired);
        if change.starts_with("replace-") {
            f.activate(3, false);
            f.pump(128);
            f.settle(3);
            assert_eq!(f.workers[2].control().stats().submitted, 1);
            let retired_worker = usize::from(change == "replace-insert");
            assert_eq!(f.workers[retired_worker].control().stats().submitted, 0);
            assert!(
                f.callback()
                    .genuine_snapshot()
                    .paused_work_quiescent(SAFETY_REVISION, 3)
            );
        }
    }
}

#[test]
fn stopped_epoch_safety_supersession_and_playing_activation_never_inherit_old_work() {
    for playing in [false, true] {
        let mut f = StoppedSafetyFixture::new();
        f.activate(2, false);
        f.pump(32);
        assert_eq!(
            f.generator().stopped_epoch_safety.unwrap().remaining_frames,
            96
        );
        f.activate(3, playing);
        for endpoint in [f.generator(), f.insert()] {
            assert_eq!(endpoint.epoch(), 3);
            assert_eq!(endpoint.stats().partial_input_frames_discarded, 32);
            assert_eq!(
                endpoint.stopped_epoch_safety.map(|owner| owner.epoch),
                (!playing).then_some(3)
            );
        }
        if playing {
            let before = f.stats();
            // Ordinary render consumes its own staged events. Pausing afterward
            // must not resurrect the retired stopped-only obligation.
            let engine = f.engine.as_ref().unwrap();
            let callback = f.callback.as_mut().unwrap();
            crate::realtime_test_alloc::assert_no_alloc_or_drop(|| callback.pump(engine, 128));
            f.settle(3);
            for (old, new) in before.into_iter().zip(f.stats()) {
                assert_eq!(new.submitted_quanta - old.submitted_quanta, 1);
            }
            f.engine
                .as_ref()
                .unwrap()
                .transport_mailbox
                .publish(TransportMutation::SetPlaying(false));
            let complete = f.stats();
            f.pump(128);
            assert_eq!(f.stats(), complete);
        } else {
            f.pump(128);
            f.settle(3);
            assert!(
                f.callback()
                    .genuine_snapshot()
                    .paused_work_quiescent(SAFETY_REVISION, 3)
            );
            assert!(
                !f.callback()
                    .genuine_snapshot()
                    .paused_work_quiescent(SAFETY_REVISION, 2)
            );
            assert!(
                !f.callback()
                    .genuine_snapshot()
                    .paused_work_quiescent(SAFETY_REVISION + 1, 3)
            );
            let complete = f.stats();
            f.pump(2048);
            assert_eq!(f.stats(), complete);
        }
    }
}

#[test]
fn stopped_epoch_safety_stale_adapter_epoch_is_observed_and_never_implicitly_switched() {
    let mut f = StoppedSafetyFixture::new();
    f.activate(2, false);
    // Real endpoint epoch change invalidates its owner. The callback still owns
    // epoch2: the service must not implicitly switch this endpoint back to2.
    assert!(f.generator_mut().set_epoch(3));
    assert!(f.generator().stopped_epoch_safety.is_none());
    let before = f.generator().stats();
    f.pump(128);
    assert_eq!(f.generator().epoch(), 3);
    assert_eq!(f.generator().stats(), before);
    assert!(
        !f.callback()
            .genuine_snapshot()
            .paused_work_quiescent(SAFETY_REVISION, 2)
    );
}

#[test]
fn stopped_epoch_safety_assertion_unwind_releases_gates_joins_exact_workers_and_keeps_admission() {
    let mut f = StoppedSafetyFixture::new();
    let original_directory = std::env::current_dir().unwrap();
    let joined: Vec<_> = f
        .workers
        .iter()
        .map(|worker| Arc::clone(&worker.joined))
        .collect();
    let probes: Vec<_> = f
        .workers
        .iter()
        .map(|worker| Arc::clone(&worker.probe))
        .collect();
    probes[0].process_gate.hold();
    probes[1].reset_gate.hold();
    f.activate(2, false);
    f.pump(128);
    probes[0].process_gate.wait_entered();
    probes[1].reset_gate.wait_entered();
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _fixture = f;
        panic!("intentional stopped-safety assertion witness");
    }))
    .unwrap_err();
    assert_eq!(
        panic.downcast_ref::<&str>(),
        Some(&"intentional stopped-safety assertion witness")
    );
    assert!(
        joined
            .iter()
            .all(|state| state.load(Ordering::Acquire) == 1)
    );
    assert!(
        probes
            .iter()
            .all(|probe| !probe.process_gate.held.load(Ordering::Acquire)
                && !probe.reset_gate.held.load(Ordering::Acquire)
                && !probe.admin_gate.held.load(Ordering::Acquire))
    );
    assert_eq!(std::env::current_dir().unwrap(), original_directory);
    assert!(!SAFETY_ADMISSION_BLOCKED.load(Ordering::Acquire));
    let mut next = StoppedSafetyFixture::new();
    next.activate(2, false);
    next.pump(128);
    next.settle(2);
    assert!(
        next.callback()
            .genuine_snapshot()
            .paused_work_quiescent(SAFETY_REVISION, 2)
    );
}
