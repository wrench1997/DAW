//! One strict mock source, real owned PCM composition and real published WAV.
//! These tests require no audio device, vendor plug-in, SDK helper or desktop.
use super::*;
use crate::{
    audio::{AudioCommand, AudioEngine, GeneratorEndpointEvent},
    automation::{AutomationCurve, AutomationLane, AutomationPoint, AutomationTarget},
    export_options::WavLevelPolicy,
    instrument_bounce::{BounceAudioBus, BouncePluginState, InstrumentBouncePlan},
    mixer_graph::{MASTER_MIXER_TRACK_ID, MixerRoute, MixerRouteDestination, MixerRouteTap},
    model::*,
    plugins::plugin_runtime::{
        BackendSlot, MidiMessage, PluginBackend, PluginChain, PluginPrepareConfig, ShutdownOutcome,
    },
    wav,
};
use std::{collections::BTreeMap, sync::atomic::AtomicU64, thread};
type MidiLedger = Arc<Mutex<Vec<(u64, [u8; 3])>>>;

static NEXT: AtomicU64 = AtomicU64::new(1);
const RATE: u32 = 8_000;
const MUSIC: usize = 40_000;

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "citrus-vst-master-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> PathBuf {
        self.0.join("master.wav")
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let (Ok(path), Ok(temp)) = (self.0.canonicalize(), std::env::temp_dir().canonicalize())
        else {
            return;
        };
        if path.starts_with(temp)
            && path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("citrus-vst-master-"))
        {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}
fn options(format: WavSampleFormat, tail: u32) -> WavExportOptions {
    WavExportOptions {
        sample_rate: RATE,
        sample_format: format,
        level_policy: WavLevelPolicy::PreserveLevel,
        tail_milliseconds: tail,
    }
}
fn render_control() -> RenderControl {
    RenderControl {
        export: ExportControl::default(),
        deadline: Instant::now() + Duration::from_secs(20),
    }
}
fn project() -> Project {
    let mut p = Project {
        tempo: 60.0,
        swing: 0.0,
        song_length_beats: 5.0,
        ..Project::default()
    };
    p.automation_lanes.clear();
    p.channels.truncate(1);
    p.channels[0].volume = 1.0;
    p.channels[0].pan = 0.0;
    p.channels[0].muted = false;
    p.channels[0].solo = false;
    p.channels[0].instrument_plugin_instance_id = Some(41);
    p.plugin_instances = vec![PluginInstance {
        id: 41,
        format: PluginFormat::Vst3,
        role: PluginRole::Instrument,
        path: "mock-master.vst3".into(),
        uid: "mock-path-uid".into(),
        vendor: "Fixture".into(),
        name: "Strict mock".into(),
        enabled: true,
        bypass: false,
        wet: 1.0,
        parameters: BTreeMap::new(),
        opaque_state: vec![1, 2, 3],
        runtime_status: PluginRuntimeStatus::Loaded,
        midi_ports: Default::default(),
    }];
    p.mixer_insert_slots.clear();
    p.piano_notes.clear();
    for track in &mut p.mixer_tracks {
        track.volume = 1.0;
        track.pan = 0.0;
        track.muted = false;
        track.solo = false;
    }
    let template = p.clips[0].clone();
    p.clips.clear();
    p.patterns = (0..5)
        .map(|i| Pattern {
            id: i + 1,
            name: format!("Pattern {}", i + 1),
            length_steps: 16,
            channel_steps: vec![[false; 16]],
            notes: vec![PianoNote {
                id: u64::from(i) + 11,
                channel_id: Some(p.channels[0].id),
                group_id: None,
                note: 60 + i as u8,
                start: 0.0,
                length: 0.5,
                velocity: 1.0,
                selected: false,
                muted: false,
            }],
        })
        .collect();
    for i in 0..5 {
        let mut clip = template.clone();
        clip.id = i + 1;
        clip.kind = ClipKind::Pattern;
        clip.pattern_id = i + 1;
        clip.start = i as f32;
        clip.length = 1.0;
        clip.source_offset = 0.0;
        clip.muted = false;
        clip.gain = 1.0;
        clip.automation_id = None;
        clip.audio_asset_id = None;
        p.clips.push(clip);
    }
    p
}
fn lane(
    p: &mut Project,
    id: u64,
    target: AutomationTarget,
    curve: AutomationCurve,
    points: &[(f64, f64)],
) {
    let mut lane = AutomationLane::new(target);
    lane.set_curve(curve);
    lane.replace_points(
        points
            .iter()
            .map(|&(beat, value)| AutomationPoint::new(beat, value)),
    );
    p.automation_lanes.push(ProjectAutomation {
        id,
        name: format!("Master {id}"),
        lane,
    });
}
fn placement(p: &mut Project, id: u32, lane_id: u64, start: f32, length: f32, slip: f32) {
    let mut clip = p.clips[0].clone();
    clip.id = id;
    clip.kind = ClipKind::Automation;
    clip.automation_id = Some(lane_id);
    clip.start = start;
    clip.length = length;
    clip.source_offset = slip;
    p.clips.push(clip);
}
fn meta(latency: u32) -> BouncePluginMetadata {
    BouncePluginMetadata {
        class_uid: "ABCDEF019182FAEB566D624153675854".into(),
        version: "1".into(),
        name: "Strict mock".into(),
        category: "Instrument|Synth".into(),
        event_input_buses: vec![(16, true, Some(true))],
        event_output_buses: vec![],
        input_buses: vec![],
        output_buses: vec![BounceAudioBus::new(2, true, Some(true))],
        latency_samples: latency,
        tail_samples: u32::MAX,
    }
}
fn capture_for(p: &Project, opts: WavExportOptions, latency: u32) -> CapturedInstrumentBounce {
    let plan = InstrumentBouncePlan::prepare_master_song(
        p,
        p.channels[0].id,
        opts.sample_rate,
        u32::try_from(opts.tail_frames().unwrap()).unwrap(),
    )
    .unwrap();
    assert!(plan.is_master_song());
    assert_eq!(plan.timeline().duration_frames(), MUSIC as u64);
    let identity = BounceSourceIdentity {
        project_session: 11,
        project_fingerprint: plan.project_fingerprint(),
        timeline_revision: 12,
        epoch: 13,
        endpoint_id: 14,
        channel_id: plan.channel_id(),
        instance_id: plan.instance_id(),
    };
    CapturedInstrumentBounce {
        identity,
        plan,
        receipt_token: Arc::new(()),
        state: BouncePluginState {
            metadata: meta(latency),
            opaque_state: vec![1, 2, 3],
            parameter_bases: BTreeMap::new(),
            native_generation: 2,
            native_dirty_revision: 9,
        },
    }
}

#[derive(Clone, Copy)]
enum Fault {
    None,
    Process,
    Panic,
    NonFinite,
    Cleanup,
    DisposePanic,
    CancelProcess,
}
struct Mock {
    metadata: BouncePluginMetadata,
    control: RenderControl,
    next: u64,
    fault: Fault,
    left: f32,
    right: f32,
    gate: Arc<AtomicBool>,
    disposing: Arc<AtomicBool>,
    destroyed: Arc<AtomicBool>,
    ledger: MidiLedger,
    disposal_panicked: bool,
}
impl Mock {
    fn new(capture: &CapturedInstrumentBounce, control: RenderControl) -> Self {
        Self {
            metadata: capture.metadata().clone(),
            control,
            next: 0,
            fault: Fault::None,
            left: 0.8,
            right: 0.4,
            gate: Arc::new(AtomicBool::new(true)),
            disposing: Arc::new(AtomicBool::new(false)),
            destroyed: Arc::new(AtomicBool::new(false)),
            ledger: Arc::new(Mutex::new(Vec::new())),
            disposal_panicked: false,
        }
    }
}
impl Drop for Mock {
    fn drop(&mut self) {
        self.destroyed.store(true, Ordering::Release);
    }
}
impl RenderBackend for Mock {
    fn prepare(
        &mut self,
        capture: &CapturedInstrumentBounce,
        _: &RenderControl,
    ) -> Result<BouncePluginMetadata, BackendFailure> {
        assert_eq!(capture.state.opaque_state, vec![1, 2, 3]);
        Ok(self.metadata.clone())
    }
    fn metadata(&mut self) -> Result<BouncePluginMetadata, BackendFailure> {
        Ok(self.metadata.clone())
    }
    fn send_midi(&mut self, data: [u8; 3], offset: u16) -> Result<(), BackendFailure> {
        self.ledger
            .lock()
            .unwrap()
            .push((self.next + u64::from(offset), data));
        Ok(())
    }
    fn process(
        &mut self,
        transport: PluginTransport,
        frames: usize,
    ) -> Result<BlockReply, BackendFailure> {
        assert_eq!(transport.sample_position, self.next as i64);
        assert_eq!(frames, BOUNCE_QUANTUM);
        if matches!(self.fault, Fault::CancelProcess) {
            self.control.export.cancel();
        }
        if matches!(self.fault, Fault::Process) {
            return Err("mock Process failed".into());
        }
        assert!(!matches!(self.fault, Fault::Panic), "mock Process panic");
        let mut channels = vec![vec![0.0; frames]; 2];
        let (left, right) = channels.split_at_mut(1);
        for (i, (left, right)) in left[0].iter_mut().zip(right[0].iter_mut()).enumerate() {
            let latency = self.next + (i as u64) < u64::from(self.metadata.latency_samples);
            *left = if latency { 9.0 } else { self.left };
            *right = if latency { -9.0 } else { self.right };
        }
        if matches!(self.fault, Fault::NonFinite) {
            channels[0][0] = f32::NAN;
        }
        self.next += frames as u64;
        Ok(BlockReply {
            channels,
            transport_applied: true,
            output_events: 0,
            output_events_lost: false,
        })
    }
    fn dispose(&mut self, _: Instant) -> DisposalObservation {
        self.disposing.store(true, Ordering::Release);
        if matches!(self.fault, Fault::DisposePanic) && !self.disposal_panicked {
            self.disposal_panicked = true;
            panic!("mock disposal panic");
        }
        DisposalObservation {
            terminated: self.gate.load(Ordering::Acquire),
            error: matches!(self.fault, Fault::Cleanup).then(|| "mock cleanup failed".into()),
            diagnostics: vec![],
        }
    }
}
fn wait(mut predicate: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(10);
    while !predicate() {
        assert!(Instant::now() < end, "mock completion timeout");
        thread::sleep(Duration::from_millis(1));
    }
}
fn outcome(receipt: &MasterSongRenderReceipt) -> MasterSongRenderOutcome {
    wait(|| receipt.termination_acknowledged());
    receipt.try_recv().unwrap()
}
fn complete(receipt: &MasterSongRenderReceipt) -> MasterSongRenderReport {
    match outcome(receipt) {
        MasterSongRenderOutcome::Complete(report) => report,
        other => panic!("{other:?}"),
    }
}
fn decode(path: &PathBuf) -> Vec<[f32; 2]> {
    wav::read_wav(path)
        .unwrap()
        .samples
        .as_chunks::<2>()
        .0
        .to_vec()
}
fn close(actual: f32, expected: f32, tolerance: f32) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "actual {actual}, expected {expected}"
    );
}
fn render(p: Project, opts: WavExportOptions, path: PathBuf) -> MasterSongRenderReport {
    let control = render_control();
    let capture = capture_for(&p, opts, 257);
    let mock = Mock::new(&capture, control.clone());
    let admission = RenderAdmission::default();
    let receipt = admission
        .start_master_song_with_backend(p, capture, opts, path, control, true, move || mock)
        .unwrap();
    let report = complete(&receipt);
    assert!(!admission.is_busy());
    report
}

#[test]
fn vst_master_missing_review_consent_refuses_long_or_unknown_tail_before_backend_and_keeps_marker()
{
    for reported in [RATE * 4, u32::MAX] {
        let p = project();
        let opts = options(WavSampleFormat::Float32, 0);
        let mut capture = capture_for(&p, opts, 0);
        capture.state.metadata.tail_samples = reported;
        let directory = Directory::new();
        let path = directory.path();
        std::fs::write(&path, b"existing destination marker").unwrap();
        let admission = RenderAdmission::default();
        let started = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&started);
        let receipt = admission
            .start_master_song_with_backend(
                p,
                capture,
                opts,
                path.clone(),
                render_control(),
                false,
                move || -> Mock {
                    observed.store(true, Ordering::Release);
                    panic!("unchecked fixed-window request must not construct a backend")
                },
            )
            .unwrap();
        let MasterSongRenderOutcome::Failed(failure) = outcome(&receipt) else {
            panic!("missing consent was published");
        };
        assert_eq!(failure.kind, FailureKind::Invalid);
        assert!(
            failure
                .primary
                .contains("Explicit fixed-tail truncation acknowledgment is required")
        );
        assert!(!started.load(Ordering::Acquire));
        assert!(!admission.is_busy());
        assert_eq!(std::fs::read(path).unwrap(), b"existing destination marker");
    }
}

#[test]
fn vst_master_explicit_review_consent_retains_zero_or_three_second_cutoff_with_noteoff_at_song_end()
{
    for reported in [RATE * 4, u32::MAX] {
        for tail_ms in [0, 3_000] {
            let mut p = project();
            p.patterns.last_mut().unwrap().notes[0].length = 1.0;
            let original = serde_json::to_vec(&p).unwrap();
            let opts = options(WavSampleFormat::Float32, tail_ms);
            let mut capture = capture_for(&p, opts, 0);
            capture.state.metadata.tail_samples = reported;
            let expected_fp = capture.plan().project_fingerprint();
            let control = render_control();
            let mock = Mock::new(&capture, control.clone());
            let ledger = Arc::clone(&mock.ledger);
            let directory = Directory::new();
            let admission = RenderAdmission::default();
            let receipt = admission
                .start_master_song_with_backend(
                    p.clone(),
                    capture,
                    opts,
                    directory.path(),
                    control,
                    true,
                    move || mock,
                )
                .unwrap();
            let report = complete(&receipt);
            assert!(!admission.is_busy());
            assert_eq!(report.dry_render.identity.project_fingerprint, expected_fp);
            assert_eq!(report.dry_render.reported_tail, reported);
            assert_eq!(
                report.dry_render.fixed_tail as usize,
                opts.tail_frames().unwrap()
            );
            assert!(report.master_output.tail_limit_reached);
            let notes = ledger.lock().unwrap();
            assert!(
                notes
                    .iter()
                    .any(|(frame, message)| *frame == MUSIC as u64 && message[0] & 0xf0 == 0x80)
            );
            assert!(
                notes
                    .iter()
                    .filter(|(_, message)| message[0] & 0xf0 == 0x90)
                    .all(|(frame, _)| *frame < MUSIC as u64)
            );
            let pcm = decode(&directory.path());
            assert_eq!(pcm.len(), MUSIC + opts.tail_frames().unwrap());
            assert!(pcm.last().unwrap()[0].abs() > 0.01);
            assert_eq!(serde_json::to_vec(&p).unwrap(), original);
        }
    }
}

/// This backend runs through the real worker bridge, generator endpoint,
/// callback mixer and master protection. Its only fixture behavior is a fixed
/// raw stereo output; the expected master PCM is read from TestAudioCallback.
struct RealtimeConstantGenerator([f32; 2]);
impl PluginBackend for RealtimeConstantGenerator {
    fn name(&self) -> &str {
        "Realtime PCM oracle"
    }
    fn prepare(&mut self, _: PluginPrepareConfig) -> Result<(), String> {
        Ok(())
    }
    fn process(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        frames: usize,
    ) -> Result<(), String> {
        left[..frames].fill(self.0[0]);
        right[..frames].fill(self.0[1]);
        Ok(())
    }
    fn send_midi(&mut self, _: MidiMessage) -> Result<(), String> {
        Ok(())
    }
    fn midi_capabilities(&self) -> (bool, bool) {
        (true, false)
    }
    fn set_parameter(&mut self, _: u32, _: f32) -> Result<(), String> {
        Ok(())
    }
    fn get_parameter(&mut self, _: u32) -> Result<f32, String> {
        Ok(0.0)
    }
    fn save_state(&mut self) -> Result<Vec<u8>, String> {
        Ok(vec![])
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

fn realtime_generator_oracle(
    raw: [f32; 2],
    source_mixer_gain: f32,
    master_gain: f32,
    master_pan: f32,
) -> Vec<[f32; 2]> {
    let chain = PluginChain::spawn_identified_with_backend_factory(
        &[41],
        move || vec![BackendSlot::new(Box::new(RealtimeConstantGenerator(raw)))],
        PluginPrepareConfig {
            sample_rate: 48_000.0,
            max_block_frames: BOUNCE_QUANTUM,
        },
    )
    .unwrap();
    wait(|| {
        chain
            .control
            .plugin_latency_snapshot()
            .is_some_and(|snapshot| snapshot.revision != 0 && snapshot.slot_is_active(0))
    });
    let PluginChain {
        audio,
        control,
        guard,
    } = chain;
    let (mut engine, mut callback) = AudioEngine::test_engine_with_callback();
    assert!(engine.install_generator_endpoint(42, 420, 41, 1, audio));
    assert!(engine.command(AudioCommand::SetTrackGain {
        track: 1,
        gain: source_mixer_gain
    }));
    assert!(engine.command(AudioCommand::SetMaster(master_gain)));
    assert!(engine.command(AudioCommand::SetMasterPan(master_pan)));
    callback.pump(&engine, BOUNCE_QUANTUM);
    let submitted = control.stats().submitted;
    wait(|| control.stats().completed >= submitted);
    assert!(matches!(
        engine.next_generator_endpoint_event(),
        Some(GeneratorEndpointEvent::Installed {
            channel_id: 42,
            endpoint_id: 420,
            plugin_instance_id: 41,
            mixer_track: 1,
            success: true,
            ..
        })
    ));
    engine.set_playing(true);
    // Drain the actual fixed-quantum bridge/PDC startup. Waiting is based on
    // completed real submissions, never synthetic PCM or manufactured timing.
    for _ in 0..64 {
        callback.pump(&engine, BOUNCE_QUANTUM);
        let submitted = control.stats().submitted;
        wait(|| control.stats().completed >= submitted);
    }
    let output = callback.last_output().to_vec();
    assert!(
        output.iter().any(|frame| *frame != [0.0; 2]),
        "real callback source was not audible"
    );
    drop(callback);
    drop(engine);
    assert_eq!(
        guard.shutdown_blocking(Duration::from_secs(5)),
        ShutdownOutcome::Joined
    );
    output
}

#[test]
fn vst_master_five_patterns_full_fingerprint_fade_pan_and_fixed_tail_match_every_wav_sample() {
    let directory = Directory::new();
    let mut p = project();
    let master = p
        .mixer_tracks
        .iter_mut()
        .find(|track| track.id == MASTER_MIXER_TRACK_ID)
        .unwrap();
    master.volume = 0.17;
    master.pan = 0.6;
    lane(
        &mut p,
        101,
        AutomationTarget::MasterVolume,
        AutomationCurve::Linear,
        &[(0.0, 1.0), (5.0, 0.0)],
    );
    lane(
        &mut p,
        102,
        AutomationTarget::MasterPan,
        AutomationCurve::Linear,
        &[(0.0, -1.0), (5.0, 1.0)],
    );
    placement(&mut p, 101, 101, 0.0, 5.0, 0.0);
    placement(&mut p, 102, 102, 0.0, 5.0, 0.0);
    let original = serde_json::to_vec(&p).unwrap();
    let opts = options(WavSampleFormat::Float32, 100);
    let capture = capture_for(&p, opts, 257);
    let fingerprint = capture.plan().project_fingerprint();
    assert_eq!(
        capture
            .plan()
            .timeline()
            .events()
            .iter()
            .filter(|event| matches!(event.kind, TimelineEventKind::NoteOn { .. }))
            .count(),
        5
    );
    let control = render_control();
    let mock = Mock::new(&capture, control.clone());
    let ledger = Arc::clone(&mock.ledger);
    let destroyed = Arc::clone(&mock.destroyed);
    let admission = RenderAdmission::default();
    let receipt = admission
        .start_master_song_with_backend(
            p.clone(),
            capture,
            opts,
            directory.path(),
            control,
            true,
            move || mock,
        )
        .unwrap();
    let report = complete(&receipt);
    assert!(destroyed.load(Ordering::Acquire));
    assert_eq!(serde_json::to_vec(&p).unwrap(), original);
    assert_eq!(report.dry_render.identity.project_fingerprint, fingerprint);
    assert_eq!(
        (
            report.dry_render.native_latency,
            report.dry_render.fixed_tail,
            report.dry_render.reported_tail
        ),
        (257, 800, u32::MAX)
    );
    assert_eq!(report.dry_render.sample_peak, 0.8);
    assert_eq!(report.master_output.frames, MUSIC + 800);
    assert_eq!(report.master_output.applied_gain, 1.0);
    assert_eq!(report.sample_format, WavSampleFormat::Float32);
    let notes = ledger.lock().unwrap();
    assert_eq!(
        notes
            .iter()
            .filter(|(_, data)| data[0] & 0xf0 == 0x90)
            .count(),
        5
    );
    assert!(
        notes
            .iter()
            .filter(|(_, data)| data[0] & 0xf0 == 0x90)
            .all(|(frame, _)| *frame < MUSIC as u64)
    );
    let samples = decode(&directory.path());
    let mut expected_peak = 0.0_f32;
    for (frame, samples) in samples.iter().enumerate() {
        let musical = frame.min(MUSIC - 1) as f32;
        let gain = 1.0 - musical / MUSIC as f32;
        let pan = -1.0 + 2.0 * musical / MUSIC as f32;
        let left = (0.8 * gain * (1.0 - pan.max(0.0))).tanh();
        let right = (0.4 * gain * (1.0 + pan.min(0.0))).tanh();
        close(samples[0], left, 2e-6);
        close(samples[1], right, 2e-6);
        expected_peak = expected_peak.max(left).max(right);
    }
    close(report.master_output.sample_peak, expected_peak, 2e-6);
}

#[test]
fn vst_master_static_channel_routing_and_master_controls_are_applied_once() {
    let directory = Directory::new();
    let mut p = project();
    p.channels[0].volume = 0.5;
    p.channels[0].pan = -0.25;
    let source_id = p.channels[0].mixer_track;
    let destination_id = p
        .mixer_tracks
        .iter()
        .find(|track| track.id != source_id && track.id != MASTER_MIXER_TRACK_ID)
        .unwrap()
        .id;
    assert_ne!(destination_id, source_id);
    assert_ne!(destination_id, MASTER_MIXER_TRACK_ID);
    for track in &mut p.mixer_tracks {
        if track.id == source_id {
            track.volume = 0.8;
            track.pan = 0.5;
        }
        if track.id == destination_id {
            track.volume = 0.4;
            track.pan = -0.5;
        }
        if track.id == MASTER_MIXER_TRACK_ID {
            track.volume = 0.5;
            track.pan = -0.2;
        }
    }
    p.mixer_routes.retain(|route| {
        route.source_mixer_track_id != source_id && route.source_mixer_track_id != destination_id
    });
    p.mixer_routes.extend([
        MixerRoute {
            id: 201,
            runtime_slot: 100,
            source_mixer_track_id: source_id,
            destination: MixerRouteDestination::MainInput {
                mixer_track_id: destination_id,
            },
            tap: MixerRouteTap::PostFader,
            gain: 0.5,
            enabled: true,
        },
        MixerRoute {
            id: 202,
            runtime_slot: 101,
            source_mixer_track_id: destination_id,
            destination: MixerRouteDestination::MainInput {
                mixer_track_id: MASTER_MIXER_TRACK_ID,
            },
            tap: MixerRouteTap::PostFader,
            gain: 1.0,
            enabled: true,
        },
    ]);
    let report = render(p, options(WavSampleFormat::Float32, 0), directory.path());
    let samples = decode(&directory.path());
    let left = (0.8_f32 * 0.5 * 1.0 * 0.8 * 0.5 * 0.5 * 0.4 * 1.0 * 0.5 * 1.0).tanh();
    let right = (0.4_f32 * 0.5 * 0.75 * 0.8 * 1.0 * 0.5 * 0.4 * 0.5 * 0.5 * 0.8).tanh();
    for samples in samples {
        close(samples[0], left, 1e-7);
        close(samples[1], right, 1e-7);
    }
    close(report.master_output.sample_peak, left.max(right), 1e-7);
}

#[test]
fn vst_master_muted_base_dynamic_unmute_and_nonmaster_gates_preserve_sources() {
    let directory = Directory::new();
    let mut p = project();
    p.mixer_tracks
        .iter_mut()
        .find(|track| track.id == MASTER_MIXER_TRACK_ID)
        .unwrap()
        .muted = true;
    lane(
        &mut p,
        101,
        AutomationTarget::MixerMute {
            track: MASTER_MIXER_TRACK_ID,
        },
        AutomationCurve::Hold,
        &[(0.0, 1.0), (1.0, 0.0), (4.0, 1.0)],
    );
    render(
        p.clone(),
        options(WavSampleFormat::Float32, 100),
        directory.path(),
    );
    let ungated = decode(&directory.path());
    for (frame, samples) in ungated.iter().enumerate() {
        assert_eq!(
            *samples,
            if (8_000..32_000).contains(&frame) {
                [0.8_f32.tanh(), 0.4_f32.tanh()]
            } else {
                [0.0, 0.0]
            }
        );
    }
    for gate in 0..4 {
        let mut gated = p.clone();
        match gate {
            0 => gated.channels[0].muted = true,
            1 => {
                let mut other = gated.channels[0].clone();
                other.id += 100;
                other.solo = true;
                other.instrument_plugin_instance_id = None;
                gated.channels.push(other);
                for pattern in &mut gated.patterns {
                    pattern.channel_steps.push([false; 16]);
                }
            }
            2 => {
                let track = gated.channels[0].mixer_track;
                gated
                    .mixer_tracks
                    .iter_mut()
                    .find(|candidate| candidate.id == track)
                    .unwrap()
                    .muted = true;
            }
            _ => {
                let source_track = gated.channels[0].mixer_track;
                let other = gated
                    .mixer_tracks
                    .iter_mut()
                    .find(|track| track.id != source_track && track.id != MASTER_MIXER_TRACK_ID)
                    .unwrap();
                assert_ne!(other.id, source_track);
                assert_ne!(other.id, MASTER_MIXER_TRACK_ID);
                other.solo = true;
            }
        }
        render(
            gated,
            options(WavSampleFormat::Float32, 0),
            directory.path(),
        );
        assert!(
            decode(&directory.path())
                .iter()
                .all(|sample| *sample == [0.0; 2]),
            "nonmaster gate {gate} must silence every output frame"
        );
    }
    // Soloing the captured source itself must retain the same dynamic MASTER
    // mute/unmute PCM; it is a distinct case from soloing an unrelated insert.
    let mut source_solo = p;
    let source_track = source_solo.channels[0].mixer_track;
    source_solo
        .mixer_tracks
        .iter_mut()
        .find(|track| track.id == source_track)
        .unwrap()
        .solo = true;
    render(
        source_solo,
        options(WavSampleFormat::Float32, 0),
        directory.path(),
    );
    assert_eq!(decode(&directory.path()).as_slice(), &ungated[..MUSIC]);
}

#[test]
fn vst_master_slipped_placement_and_exclusive_end_hold_last_musical_control() {
    let directory = Directory::new();
    let mut p = project();
    p.mixer_tracks
        .iter_mut()
        .find(|track| track.id == MASTER_MIXER_TRACK_ID)
        .unwrap()
        .volume = 0.25;
    lane(
        &mut p,
        101,
        AutomationTarget::MasterVolume,
        AutomationCurve::Linear,
        &[(0.0, 0.0), (5.0, 1.0)],
    );
    placement(&mut p, 101, 101, 1.0, 1.0, 2.0);
    lane(
        &mut p,
        102,
        AutomationTarget::MasterVolume,
        AutomationCurve::Hold,
        &[(0.0, 0.8), (1.0, 0.0)],
    );
    placement(&mut p, 102, 102, 4.0, 1.0, 0.0);
    render(p, options(WavSampleFormat::Float32, 100), directory.path());
    for (frame, samples) in decode(&directory.path()).iter().enumerate() {
        let gain = match frame {
            8_000..16_000 => 0.4 + (frame - 8_000) as f32 / MUSIC as f32,
            32_000.. => 0.8,
            _ => 0.25,
        };
        close(samples[0], (0.8 * gain).tanh(), 2e-6);
        close(samples[1], (0.4 * gain).tanh(), 2e-6);
    }
}

#[test]
fn vst_master_all_encodings_and_level_policies_use_protected_final_master_peak() {
    for format in WavSampleFormat::ALL {
        let directory = Directory::new();
        let p = project();
        let mut opts = options(format, 0);
        opts.level_policy = WavLevelPolicy::AttenuatePeaks;
        let capture = capture_for(&p, opts, 0);
        let control = render_control();
        let mut mock = Mock::new(&capture, control.clone());
        mock.left = 1.9;
        mock.right = -0.95;
        let admission = RenderAdmission::default();
        let receipt = admission
            .start_master_song_with_backend(
                p,
                capture,
                opts,
                directory.path(),
                control,
                true,
                move || mock,
            )
            .unwrap();
        let report = complete(&receipt);
        assert_eq!(report.master_output.sample_peak, 1.9_f32.tanh());
        let gain = 0.95 / 1.9_f32.tanh();
        assert_eq!(report.master_output.applied_gain, gain);
        let decoded = wav::read_wav(directory.path()).unwrap();
        assert_eq!(decoded.metadata.bits_per_sample, format.bits());
        for samples in decoded.samples.as_chunks::<2>().0 {
            close(samples[0], 0.95, 4e-5);
            close(samples[1], (-0.95_f32).tanh() * gain, 4e-5);
        }
    }
    let directory = Directory::new();
    let p = project();
    let opts = options(WavSampleFormat::Float32, 0);
    let capture = capture_for(&p, opts, 0);
    let control = render_control();
    let mut mock = Mock::new(&capture, control.clone());
    mock.left = 1.9;
    let admission = RenderAdmission::default();
    let receipt = admission
        .start_master_song_with_backend(
            p,
            capture,
            opts,
            directory.path(),
            control,
            true,
            move || mock,
        )
        .unwrap();
    assert_eq!(complete(&receipt).master_output.applied_gain, 1.0);
    assert!(
        decode(&directory.path())
            .iter()
            .all(|sample| sample[0] == 1.9_f32.tanh())
    );
}

#[test]
fn vst_master_level_policy_uses_composed_peak_rather_than_the_louder_dry_source() {
    for format in WavSampleFormat::ALL {
        let directory = Directory::new();
        let mut p = project();
        p.mixer_tracks
            .iter_mut()
            .find(|track| track.id == MASTER_MIXER_TRACK_ID)
            .unwrap()
            .volume = 0.25;
        let opts = options(format, 0);
        let capture = capture_for(&p, opts, 0);
        let control = render_control();
        let mut mock = Mock::new(&capture, control.clone());
        mock.left = 1.9;
        mock.right = -0.95;
        let admission = RenderAdmission::default();
        let receipt = admission
            .start_master_song_with_backend(
                p,
                capture,
                opts,
                directory.path(),
                control,
                true,
                move || mock,
            )
            .unwrap();
        let report = complete(&receipt);
        assert_eq!(report.dry_render.sample_peak, 1.9);
        assert_eq!(report.master_output.sample_peak, 0.475_f32.tanh());
        assert_eq!(report.master_output.applied_gain, 1.0);
        for sample in decode(&directory.path()) {
            close(sample[0], 0.475_f32.tanh(), 4e-5);
            close(sample[1], (-0.2375_f32).tanh(), 4e-5);
        }
    }
}

#[test]
fn vst_master_snapshot_and_option_mismatch_never_construct_backend_or_replace_existing_path() {
    for fault in 0..4 {
        let directory = Directory::new();
        std::fs::write(directory.path(), b"existing destination").unwrap();
        let mut p = project();
        let mut opts = options(WavSampleFormat::Float32, 0);
        let capture = capture_for(&p, opts, 0);
        match fault {
            0 => p.name.push_str(" stale"),
            1 => opts.sample_rate = 16_000,
            2 => opts.tail_milliseconds = 1,
            _ => opts.tail_milliseconds = WavExportOptions::MAX_TAIL_MILLISECONDS + 1,
        }
        let constructed = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&constructed);
        let control = render_control();
        let mock = Mock::new(&capture, control.clone());
        let admission = RenderAdmission::default();
        let receipt = admission
            .start_master_song_with_backend(
                p,
                capture,
                opts,
                directory.path(),
                control,
                true,
                move || {
                    flag.store(true, Ordering::Release);
                    mock
                },
            )
            .unwrap();
        assert!(matches!(
            outcome(&receipt),
            MasterSongRenderOutcome::Failed(_)
        ));
        assert!(!constructed.load(Ordering::Acquire));
        assert_eq!(
            std::fs::read(directory.path()).unwrap(),
            b"existing destination"
        );
    }
}

#[test]
fn vst_master_backend_failures_panics_and_invalid_discarded_pcm_never_publish() {
    for fault in [
        Fault::Process,
        Fault::Panic,
        Fault::NonFinite,
        Fault::Cleanup,
        Fault::DisposePanic,
        Fault::CancelProcess,
    ] {
        let directory = Directory::new();
        std::fs::write(directory.path(), b"existing destination").unwrap();
        let p = project();
        let opts = options(WavSampleFormat::Float32, 0);
        let capture = capture_for(&p, opts, 257);
        let control = render_control();
        let mut mock = Mock::new(&capture, control.clone());
        mock.fault = fault;
        let destroyed = Arc::clone(&mock.destroyed);
        let admission = RenderAdmission::default();
        let receipt = admission
            .start_master_song_with_backend(
                p,
                capture,
                opts,
                directory.path(),
                control,
                true,
                move || mock,
            )
            .unwrap();
        assert!(matches!(
            outcome(&receipt),
            MasterSongRenderOutcome::Failed(_)
        ));
        assert!(destroyed.load(Ordering::Acquire));
        assert!(!admission.is_busy());
        assert_eq!(
            std::fs::read(directory.path()).unwrap(),
            b"existing destination"
        );
    }
}

#[test]
fn vst_master_unreaped_cleanup_retains_admission_and_grace_expiry_forbids_later_publication() {
    let directory = Directory::new();
    std::fs::write(directory.path(), b"existing destination").unwrap();
    let p = project();
    let opts = options(WavSampleFormat::Float32, 0);
    let capture = capture_for(&p, opts, 0);
    let control = render_control();
    let mock = Mock::new(&capture, control.clone());
    mock.gate.store(false, Ordering::Release);
    let gate = Arc::clone(&mock.gate);
    let disposing = Arc::clone(&mock.disposing);
    let destroyed = Arc::clone(&mock.destroyed);
    let admission = RenderAdmission::default();
    let receipt = admission
        .start_master_song_with_backend(
            p.clone(),
            capture,
            opts,
            directory.path(),
            control,
            true,
            move || mock,
        )
        .unwrap();
    wait(|| disposing.load(Ordering::Acquire));
    receipt.expire_cleanup_grace();
    wait(|| matches!(receipt.status(), RenderStatus::CleanupPending { .. }));
    assert!(admission.is_busy());
    assert!(!destroyed.load(Ordering::Acquire));
    assert!(!receipt.termination_acknowledged());
    assert!(matches!(receipt.try_recv(), Err(TryRecvError::Empty)));
    let another = capture_for(&p, opts, 0);
    assert!(
        admission
            .start_master_song_with_backend(
                p,
                another,
                opts,
                directory.path(),
                render_control(),
                true,
                || -> Mock { panic!("must not spawn") }
            )
            .is_err()
    );
    assert_eq!(
        std::fs::read(directory.path()).unwrap(),
        b"existing destination"
    );
    gate.store(true, Ordering::Release);
    let MasterSongRenderOutcome::Failed(failure) = outcome(&receipt) else {
        panic!("late publication");
    };
    assert!(failure.cleanup_grace_elapsed);
    assert!(destroyed.load(Ordering::Acquire));
    assert!(!admission.is_busy());
    assert_eq!(
        std::fs::read(directory.path()).unwrap(),
        b"existing destination"
    );
}

#[test]
fn vst_master_cancel_during_composition_encoding_or_final_commit_preserves_destination() {
    for phase in [6_000, 6_250, 6_500, 7_500, 8_000, 9_900] {
        let directory = Directory::new();
        std::fs::write(directory.path(), b"existing destination").unwrap();
        let p = project();
        let opts = options(WavSampleFormat::Float32, 100);
        let capture = capture_for(&p, opts, 0);
        let control = render_control();
        control.export.cancel_at(phase);
        let mock = Mock::new(&capture, control.clone());
        let admission = RenderAdmission::default();
        let receipt = admission
            .start_master_song_with_backend(
                p,
                capture,
                opts,
                directory.path(),
                control,
                true,
                move || mock,
            )
            .unwrap();
        let MasterSongRenderOutcome::Failed(error) = outcome(&receipt) else {
            panic!("cancelled export published");
        };
        assert_eq!(error.kind, FailureKind::Cancelled);
        assert_eq!(
            std::fs::read(directory.path()).unwrap(),
            b"existing destination"
        );
        assert_eq!(std::fs::read_dir(&directory.0).unwrap().count(), 1);
    }
}

#[test]
fn vst_master_deadline_through_composition_and_final_commit_preserves_destination() {
    for phase in [6_000, 6_250, 6_500, 8_000, 9_900] {
        let directory = Directory::new();
        std::fs::write(directory.path(), b"existing destination").unwrap();
        let p = project();
        let opts = options(WavSampleFormat::Float32, 100);
        let capture = capture_for(&p, opts, 0);
        let control = render_control();
        control.export.deadline_at(phase);
        let mock = Mock::new(&capture, control.clone());
        let admission = RenderAdmission::default();
        let receipt = admission
            .start_master_song_with_backend(
                p,
                capture,
                opts,
                directory.path(),
                control,
                true,
                move || mock,
            )
            .unwrap();
        let MasterSongRenderOutcome::Failed(error) = outcome(&receipt) else {
            panic!("expired export published");
        };
        assert_eq!(error.kind, FailureKind::Deadline);
        assert_eq!(
            std::fs::read(directory.path()).unwrap(),
            b"existing destination"
        );
        assert_eq!(std::fs::read_dir(&directory.0).unwrap().count(), 1);
    }
}

#[test]
fn vst_master_preserve_integer_encodes_protected_master_and_writer_rename_failure_keeps_existing_path()
 {
    let directory = Directory::new();
    std::fs::write(directory.path(), b"existing destination").unwrap();
    let p = project();
    let opts = options(WavSampleFormat::Pcm24, 0);
    let capture = capture_for(&p, opts, 0);
    let control = render_control();
    let mut mock = Mock::new(&capture, control.clone());
    mock.left = 1.1;
    let admission = RenderAdmission::default();
    let receipt = admission
        .start_master_song_with_backend(
            p,
            capture,
            opts,
            directory.path(),
            control,
            true,
            move || mock,
        )
        .unwrap();
    let report = complete(&receipt);
    assert_eq!(report.master_output.applied_gain, 1.0);
    assert_eq!(report.master_output.sample_peak, 1.1_f32.tanh());
    for sample in decode(&directory.path()) {
        close(sample[0], 1.1_f32.tanh(), 2e-7);
    }
    let blocked = directory.0.join("existing-directory.wav");
    std::fs::create_dir(&blocked).unwrap();
    std::fs::write(blocked.join("keep"), b"existing child").unwrap();
    let p = project();
    let opts = options(WavSampleFormat::Float32, 0);
    let capture = capture_for(&p, opts, 0);
    let control = render_control();
    let mock = Mock::new(&capture, control.clone());
    let receipt = admission
        .start_master_song_with_backend(
            p,
            capture,
            opts,
            blocked.clone(),
            control,
            true,
            move || mock,
        )
        .unwrap();
    assert!(matches!(
        outcome(&receipt),
        MasterSongRenderOutcome::Failed(_)
    ));
    assert_eq!(
        std::fs::read(blocked.join("keep")).unwrap(),
        b"existing child"
    );
    assert_eq!(std::fs::read_dir(&directory.0).unwrap().count(), 2);
}

#[test]
fn vst_master_ordinary_export_guard_still_rejects_active_generator() {
    let p = project();
    assert!(crate::export::ensure_supported_project(&p).is_err());
}

#[test]
fn vst_master_pcm_matches_actual_realtime_generator_clamp_and_final_master_protection() {
    // No Timeline is activated in this oracle: channel bases remain exactly
    // unity volume/center pan. source_gain below is a mixer fader in both paths.
    for (raw, source_gain, master_gain, master_pan) in [
        ([10.0, -10.0], 1.0, 0.01, 0.0),
        ([8.0, -8.0], 1.0, 0.01, 0.0),
        ([4.0, -4.0], 1.0, 0.01, 0.0),
        ([0.25, -0.125], 1.0, 1.0, 0.0),
        ([10.0, -10.0], 0.5, 0.02, 0.5),
        ([f32::MAX, -f32::MAX], 1.0, 0.01, 0.0),
    ] {
        let oracle = realtime_generator_oracle(raw, source_gain, master_gain, master_pan);
        let directory = Directory::new();
        let mut p = project();
        let source = p.channels[0].mixer_track;
        p.mixer_tracks
            .iter_mut()
            .find(|track| track.id == source)
            .unwrap()
            .volume = source_gain;
        let master = p
            .mixer_tracks
            .iter_mut()
            .find(|track| track.id == MASTER_MIXER_TRACK_ID)
            .unwrap();
        master.volume = master_gain;
        master.pan = master_pan;
        let opts = options(WavSampleFormat::Float32, 0);
        let capture = capture_for(&p, opts, 0);
        let control = render_control();
        let mut mock = Mock::new(&capture, control.clone());
        mock.left = raw[0];
        mock.right = raw[1];
        let admission = RenderAdmission::default();
        let receipt = admission
            .start_master_song_with_backend(
                p,
                capture,
                opts,
                directory.path(),
                control,
                true,
                move || mock,
            )
            .unwrap();
        let report = complete(&receipt);
        assert_eq!(
            report.dry_render.sample_peak,
            raw[0].abs().max(raw[1].abs())
        );
        let samples = decode(&directory.path());
        let expected = oracle[oracle.len() - 1];
        for actual in samples {
            close(actual[0], expected[0], 2e-7);
            close(actual[1], expected[1], 2e-7);
        }
        close(
            report.master_output.sample_peak,
            expected[0].abs().max(expected[1].abs()),
            2e-7,
        );
        if raw[0] == 10.0 && source_gain == 1.0 && master_pan == 0.0 {
            close(expected[0], 0.08_f32.tanh(), 2e-7);
            assert!((expected[0] - 0.1_f32.tanh()).abs() > 0.01);
        }
    }
}

#[test]
fn vst_master_all_insert_slots_including_disabled_and_bypassed_are_rejected_before_backend() {
    for (enabled, bypass) in [(false, false), (true, true), (false, true)] {
        let directory = Directory::new();
        std::fs::write(directory.path(), b"existing destination").unwrap();
        let mut p = project();
        let mut effect = p.plugin_instances[0].clone();
        effect.id = 99;
        effect.role = PluginRole::Effect;
        effect.enabled = enabled;
        effect.bypass = bypass;
        p.plugin_instances.push(effect);
        p.mixer_insert_slots.push(MixerInsertSlotRef {
            track: p.channels[0].mixer_track,
            slot: 0,
            plugin_instance_id: 99,
        });
        let opts = options(WavSampleFormat::Float32, 0);
        let capture = capture_for(&p, opts, 0);
        let control = render_control();
        let mock = Mock::new(&capture, control.clone());
        let constructed = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&constructed);
        let admission = RenderAdmission::default();
        let receipt = admission
            .start_master_song_with_backend(
                p,
                capture,
                opts,
                directory.path(),
                control,
                true,
                move || {
                    flag.store(true, Ordering::Release);
                    mock
                },
            )
            .unwrap();
        let MasterSongRenderOutcome::Failed(error) = outcome(&receipt) else {
            panic!("insert slot was admitted");
        };
        assert!(error.primary.contains("insert slots"));
        assert!(!constructed.load(Ordering::Acquire));
        assert_eq!(
            std::fs::read(directory.path()).unwrap(),
            b"existing destination"
        );
    }
}

#[test]
fn vst_master_static_base_must_fit_actual_realtime_master_control_range() {
    let directory = Directory::new();
    std::fs::write(directory.path(), b"existing destination").unwrap();
    let mut p = project();
    p.mixer_tracks
        .iter_mut()
        .find(|track| track.id == MASTER_MIXER_TRACK_ID)
        .unwrap()
        .volume = 1.5;
    let opts = options(WavSampleFormat::Float32, 0);
    let capture = capture_for(&p, opts, 0);
    let control = render_control();
    let mock = Mock::new(&capture, control.clone());
    let constructed = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&constructed);
    let admission = RenderAdmission::default();
    let receipt = admission
        .start_master_song_with_backend(
            p,
            capture,
            opts,
            directory.path(),
            control,
            true,
            move || {
                flag.store(true, Ordering::Release);
                mock
            },
        )
        .unwrap();
    let MasterSongRenderOutcome::Failed(error) = outcome(&receipt) else {
        panic!("model-only master base was admitted");
    };
    assert!(error.primary.contains("realtime range 0..1.2"));
    assert!(!constructed.load(Ordering::Acquire));
    assert_eq!(
        std::fs::read(directory.path()).unwrap(),
        b"existing destination"
    );
    let oracle = realtime_generator_oracle([0.8, 0.4], 1.0, 1.2, 0.0);
    let mut p = project();
    p.mixer_tracks
        .iter_mut()
        .find(|track| track.id == MASTER_MIXER_TRACK_ID)
        .unwrap()
        .volume = 1.2;
    let report = render(p, opts, directory.path());
    for actual in decode(&directory.path()) {
        close(actual[0], oracle[0][0], 2e-7);
        close(actual[1], oracle[0][1], 2e-7);
    }
    close(report.master_output.sample_peak, oracle[0][0], 2e-7);
}
