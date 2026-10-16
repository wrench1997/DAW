//! Explicit source-built SDK acceptance of real capture -> production strict serial owners -> WAV.
#![cfg(all(
    feature = "experimental-fresh-helper",
    any(windows, target_os = "linux")
))]

use super::*;
use crate::export::serial_master::automation_test_support::{self as master_automation, Mode};
use crate::{
    export_job::ExportControl,
    export_options::{WavExportOptions, WavLevelPolicy, WavSampleFormat},
    instrument_bounce::{
        BOUNCE_PREPARED_MAXIMUM, BounceSourceIdentity, BounceSourceStatus,
        CapturedInstrumentBounce, InstrumentBounceCapture, InstrumentBouncePlan,
        effect::{CapturedEffectSlot, EffectSourceMetadata},
        render::{
            BackendFailure, BlockReply, DisposalObservation, MasterSongRenderOutcome,
            RenderAdmission, RenderBackend, RenderControl,
            serial::{CapturedSerialEffectChain, EffectRenderBackend},
        },
    },
    mixer_graph::{MASTER_MIXER_TRACK_ID, MixerRoute, MixerRouteDestination, MixerRouteTap},
    model::{
        Clip, ClipKind, MixerInsertSlotRef, PianoNote, PluginInstance, PluginRole,
        PluginRuntimeStatus, Project,
    },
    plugins::{
        PluginDescriptor, PluginFormat,
        strict_backend::{StrictBackend, StrictEffectBackend},
    },
};
use std::{
    cell::RefCell, collections::BTreeMap, mem::ManuallyDrop, path::Path, sync::atomic::AtomicU64,
};

const RATE: u32 = 8_192;
const MUSIC: usize = 256;
const GENERATOR: u64 = 41;
const EFFECTS: [u64; 2] = [91, 92];
const BASES: [f64; 2] = [0.812_345_678_912_345_7, 0.923_456_789_123_456_8];
const GEN_CLASS: &str = "43545253454658440000000000000002";
const FX_CLASS: &str = "43545253454658440000000000000001";
const VENDOR: &str = "Citrus Studio Test Fixtures";
const VERSION: &str = "1.0.0";
const SOURCE_BUDGET: Duration = Duration::from_secs(30);
static NEXT: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static SOURCE_QUARANTINE: RefCell<Vec<ManuallyDrop<PluginChain>>> = const { RefCell::new(Vec::new()) };
}

struct SourceOwner {
    chain: Option<PluginChain>,
    cleanup: Option<RetainedShutdownPoll>,
}
impl SourceOwner {
    fn chain(&self) -> &PluginChain {
        self.chain.as_ref().unwrap()
    }
    fn chain_mut(&mut self) -> &mut PluginChain {
        self.chain.as_mut().unwrap()
    }
    fn finish(&mut self) -> RetainedShutdownPoll {
        if let Some(outcome) = self.cleanup {
            return outcome;
        }
        let deadline = Instant::now() + SOURCE_BUDGET;
        let outcome = loop {
            let outcome = self.chain.as_mut().unwrap().guard.poll_shutdown_retained();
            if outcome != RetainedShutdownPoll::Pending || Instant::now() >= deadline {
                break outcome;
            }
            thread::sleep(Duration::from_millis(1));
        };
        self.cleanup = Some(outcome);
        if outcome != RetainedShutdownPoll::Joined {
            SOURCE_QUARANTINE.with(|slot| {
                slot.borrow_mut()
                    .push(ManuallyDrop::new(self.chain.take().unwrap()))
            });
        }
        outcome
    }
}
impl Drop for SourceOwner {
    fn drop(&mut self) {
        let outcome = self.finish();
        if outcome != RetainedShutdownPoll::Joined && !thread::panicking() {
            panic!("real serial source did not join: {outcome:?}; exact owner retained");
        }
    }
}

fn wait_source(source: &SourceOwner, deadline: Instant) {
    loop {
        let chain = source.chain();
        let metrics = &chain.control.metrics;
        assert_eq!(
            chain.control.stats().faults,
            0,
            "real source load/prepare/admin fault"
        );
        if chain.control.plugin_endpoint_snapshot().is_some()
            && metrics.worker_settlement.valid.load(Ordering::Acquire)
            && metrics
                .worker_settlement
                .reset_completed_epoch
                .load(Ordering::Acquire)
                == INITIAL_TRANSPORT_EPOCH
            && metrics.worker_settlement.busy.load(Ordering::Acquire) == 0
            && !chain.control.has_outstanding_admin_work()
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "real serial source did not settle"
        );
        thread::sleep(Duration::from_millis(1));
    }
}

fn source(bundle: &Path, helper: &Path, id: u64, class: &str, base: f64) -> SourceOwner {
    let generator = class == GEN_CLASS;
    let mut spec = PluginLoadSpec::from_descriptor(PluginDescriptor {
        id: format!("actual-serial-{id}"),
        name: if generator {
            "Citrus Lookahead MIDI Fixture"
        } else {
            "Citrus Stereo Delay Fixture"
        }
        .into(),
        vendor: VENDOR.into(),
        path: bundle.to_path_buf(),
        format: PluginFormat::Vst3,
        category: if generator {
            "Instrument|Synth"
        } else {
            "Fx|Delay"
        }
        .into(),
        is_instrument: generator,
        verified: true,
        vst3_metadata: None,
        vst3_catalog: None,
        scan_error: None,
    });
    spec.class_uid = Some(class.into());
    spec.vst3_helper_path = Some(helper.to_path_buf());
    let config = PluginPrepareConfig {
        sample_rate: f64::from(RATE),
        max_block_frames: BOUNCE_PREPARED_MAXIMUM,
    };
    let chain = PluginChain::spawn_identified_with_backend_factory(
        &[id],
        move || {
            let mut backend =
                Vst3Backend::load(spec, config).expect("real source SDK class loads on its owner");
            backend
                .plugin
                .set_parameter(0, base)
                .expect("actual source f64 setter");
            assert_eq!(
                backend.plugin.get_parameter(0).unwrap().to_bits(),
                base.to_bits()
            );
            backend
                .set_transport(PluginTransport {
                    tempo: 120.0,
                    ..Default::default()
                })
                .expect("stopped source transport context");
            vec![BackendSlot::new(Box::new(backend))]
        },
        config,
    )
    .expect("real identified source worker");
    let owner = SourceOwner {
        chain: Some(chain),
        cleanup: None,
    };
    wait_source(&owner, Instant::now() + SOURCE_BUDGET);
    owner
}

fn query_process_count(source: &mut SourceOwner, request: u64, deadline: Instant) -> f32 {
    assert!(
        source
            .chain()
            .control
            .query_parameter_tagged(0, 1000, request)
    );
    loop {
        while let Some(event) = source.chain_mut().control.try_next_event() {
            match event {
                RuntimeEvent::ParameterValue {
                    request_id,
                    id: 1000,
                    value,
                    ..
                } if request_id == request => return value,
                RuntimeEvent::ParameterCommandFailed { request_id, .. }
                    if request_id == request =>
                {
                    panic!("actual SDK process probe failed")
                }
                RuntimeEvent::SlotFault { message, .. } => {
                    panic!("actual SDK source fault: {message}")
                }
                _ => {}
            }
        }
        assert!(
            Instant::now() < deadline,
            "actual SDK process-count query did not finish"
        );
        thread::sleep(Duration::from_millis(1));
    }
}

fn project(bundle: &std::path::Path) -> Project {
    let mut p = Project::blank();
    p.tempo = 120.0;
    p.song_length_beats = 0.0625;
    p.channels[0].volume = 1.0;
    p.channels[0].pan = 0.0;
    p.channels[0].mixer_track = 1;
    p.channels[0].instrument_plugin_instance_id = Some(GENERATOR);
    for track in &mut p.mixer_tracks {
        track.volume = 1.0;
        track.pan = 0.0;
    }
    p.mixer_tracks[1].volume = 1.5;
    p.mixer_tracks[1].pan = -0.25;
    p.mixer_tracks[2].volume = 1.5;
    p.mixer_tracks[2].pan = 0.5;
    p.mixer_tracks[0].volume = 0.875;
    p.mixer_tracks[0].pan = -0.25;
    p.mixer_routes = [(1, 2, 0.75), (2, MASTER_MIXER_TRACK_ID, 1.0)]
        .into_iter()
        .enumerate()
        .map(|(index, (source, destination, gain))| MixerRoute {
            id: index as u64 + 1,
            runtime_slot: index as u8,
            source_mixer_track_id: source,
            destination: MixerRouteDestination::MainInput {
                mixer_track_id: destination,
            },
            tap: MixerRouteTap::PostFader,
            gain,
            enabled: true,
        })
        .collect();
    p.plugin_instances = [GENERATOR, EFFECTS[0], EFFECTS[1]]
        .into_iter()
        .enumerate()
        .map(|(index, id)| PluginInstance {
            id,
            format: crate::model::PluginFormat::Vst3,
            role: if index == 0 {
                PluginRole::Instrument
            } else {
                PluginRole::Effect
            },
            path: bundle.to_path_buf(),
            uid: format!("actual-serial-{id}"),
            vendor: VENDOR.into(),
            name: format!("Actual serial {id}"),
            enabled: true,
            bypass: false,
            wet: 1.0,
            parameters: BTreeMap::from([(
                0,
                if index == 0 {
                    1.0
                } else {
                    BASES[index - 1] as f32
                },
            )]),
            opaque_state: Vec::new(),
            runtime_status: PluginRuntimeStatus::Unloaded,
            midi_ports: Default::default(),
        })
        .collect();
    p.mixer_insert_slots = vec![
        MixerInsertSlotRef {
            track: 2,
            slot: 3,
            plugin_instance_id: EFFECTS[1],
        },
        MixerInsertSlotRef {
            track: 1,
            slot: 0,
            plugin_instance_id: EFFECTS[0],
        },
    ];
    p.patterns[0].notes = [60, 72, 84, 96]
        .into_iter()
        .enumerate()
        .map(|(index, note)| PianoNote {
            id: index as u64 + 11,
            channel_id: Some(p.channels[0].id),
            group_id: None,
            note,
            start: 0.0,
            length: p.song_length_beats,
            velocity: 1.0,
            selected: false,
            muted: false,
        })
        .collect();
    p.clips = vec![Clip {
        id: 1,
        track: 0,
        start: 0.0,
        length: p.song_length_beats,
        name: "Real polyphonic N-boundary note".into(),
        color: [0; 3],
        kind: ClipKind::Pattern,
        group_id: None,
        pattern_length_policy: None,
        pattern_id: 1,
        automation_id: None,
        audio_asset_id: None,
        source_offset: 0.0,
        audio_source_offset_frame: None,
        audio_source_reference: None,
        audio_length_reference: None,
        fade_in_reference: None,
        fade_out_reference: None,
        gain: 1.0,
        fade_in: 0.0,
        fade_out: 0.0,
        muted: false,
    }];
    p
}

fn capture_generator(
    p: &Project,
    helper: &Path,
    opts: WavExportOptions,
    source: &mut SourceOwner,
) -> CapturedInstrumentBounce {
    let mut plan = InstrumentBouncePlan::prepare_master_song_serial(
        p,
        p.channels[0].id,
        RATE,
        opts.tail_frames().unwrap() as u32,
    )
    .unwrap();
    assert_eq!(plan.timeline().duration_frames(), MUSIC as u64);
    plan.set_strict_fixture_helper_for_test(helper.to_path_buf());
    let identity = BounceSourceIdentity {
        project_session: 11,
        project_fingerprint: plan.project_fingerprint(),
        timeline_revision: 12,
        epoch: INITIAL_TRANSPORT_EPOCH,
        endpoint_id: 14,
        channel_id: plan.channel_id(),
        instance_id: GENERATOR,
    };
    let status = BounceSourceStatus {
        confirmed_stopped: true,
        exact_timeline: true,
        source_ready: true,
        ..Default::default()
    };
    let mut capture = InstrumentBounceCapture::begin(
        plan,
        identity,
        status,
        &source.chain().control,
        Instant::now(),
    )
    .unwrap();
    let deadline = Instant::now() + SOURCE_BUDGET;
    let captured = loop {
        if let Some(captured) = capture.poll(identity, status, Instant::now()).unwrap() {
            break captured;
        }
        assert!(
            Instant::now() < deadline,
            "real generator capture did not terminate"
        );
        thread::sleep(Duration::from_millis(1));
    };
    assert!(capture.termination_acknowledged());
    assert_eq!(captured.metadata().class_uid, GEN_CLASS);
    assert_eq!(captured.metadata().category, "Instrument|Synth");
    assert_eq!(
        (
            captured.metadata().latency_samples,
            captured.metadata().tail_samples
        ),
        (32, 32)
    );
    assert_eq!(captured.state.opaque_state.len(), 40);
    assert_eq!(
        captured.state.parameter_bases[&0].to_bits(),
        1.0_f32.to_bits()
    );
    assert_eq!(
        query_process_count(source, 901, deadline),
        0.0,
        "real generator source capture must not execute positive frames"
    );
    captured
}

fn capture_effect(p: &Project, source: &SourceOwner, index: usize) -> CapturedSerialEffectChain {
    let id = EFFECTS[index];
    let deadline = Instant::now() + SOURCE_BUDGET;
    let endpoint = source.chain().control.plugin_endpoint_snapshot().unwrap();
    assert_eq!(
        (endpoint.slot_count(), endpoint.slot(0).unwrap().instance_id),
        (1, id)
    );
    let instance = p.plugin_instances.iter().find(|p| p.id == id).unwrap();
    let request = EffectCaptureSlotRequest {
        instance_id: id,
        config: SlotConfig::default(),
        path: instance.path.clone(),
        requested_class_uid: Some(FX_CLASS.into()),
        parameter_ids: vec![0],
    };
    let receiver = source
        .chain()
        .control
        .request_effect_chain_state(
            INITIAL_TRANSPORT_EPOCH,
            endpoint,
            vec![request.clone()],
            deadline,
        )
        .unwrap();
    let receipt = loop {
        match receiver.try_recv() {
            Ok(result) => break result.expect("actual SDK Effect source receipt"),
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => panic!("real Effect private reply lost"),
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    };
    assert!(receiver.termination_acknowledged());
    while !source
        .chain()
        .control
        .effect_chain_receipt_current(&receipt)
    {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(receipt.slots.len(), 1);
    assert_eq!(receipt.slots[0].request, request);
    let state = &receipt.slots[0].state;
    state.validate(&[0]).unwrap();
    assert_eq!(
        (
            &*state.metadata.identity.class_uid,
            &*state.metadata.identity.vendor,
            &*state.metadata.identity.version,
            &*state.metadata.identity.category
        ),
        (FX_CLASS, VENDOR, VERSION, "Fx|Delay")
    );
    assert_eq!(
        (
            state.metadata.latency_samples,
            state.metadata.tail_samples,
            state.opaque_state.len()
        ),
        (32, 160, 40)
    );
    assert_eq!(state.parameter_bases[&0].to_bits(), BASES[index].to_bits());
    assert_ne!(
        f64::from(BASES[index] as f32).to_bits(),
        BASES[index].to_bits()
    );
    assert_eq!(
        state
            .catalog
            .iter()
            .find(|p| p.id == 1000)
            .unwrap()
            .current_normalized,
        0.0,
        "actual Effect source private capture must not execute positive frames"
    );
    let placement = p
        .mixer_insert_slots
        .iter()
        .find(|p| p.plugin_instance_id == id)
        .unwrap()
        .clone();
    CapturedSerialEffectChain {
        runtime_track: placement.track as usize,
        placements: vec![placement],
        receipt,
    }
}

struct ChildCompletion {
    id: u64,
    spawns: usize,
    reaped: Option<std::process::ExitStatus>,
    clean: bool,
}

#[derive(Default)]
struct Audit {
    live: usize,
    max_live: usize,
    lifecycle: Vec<(u64, &'static str)>,
    completions: Vec<ChildCompletion>,
    deadlines: Vec<Instant>,
    transports: Vec<(u64, PluginTransport)>,
    midi: Vec<(u64, [u8; 3])>,
    generator_peak: f32,
    effect_input_peak: f32,
    offline_effect_probes: Vec<(u64, f64)>,
}
type Log = Arc<Mutex<Audit>>;

struct Witness {
    id: u64,
    log: Log,
    deadline: Instant,
    clean: bool,
}
impl Witness {
    fn new(id: u64, log: Log, deadline: Instant) -> Self {
        {
            let mut audit = log.lock().unwrap();
            audit.live += 1;
            audit.max_live = audit.max_live.max(audit.live);
            audit.lifecycle.push((id, "create"));
        }
        Self {
            id,
            log,
            deadline,
            clean: false,
        }
    }
    fn prepare(&self, control: &RenderControl) {
        assert_eq!(control.deadline, self.deadline);
        self.log.lock().unwrap().deadlines.push(control.deadline);
    }
    fn disposal(&mut self, observation: &DisposalObservation) {
        if observation.terminated {
            self.clean = observation.error.is_none() && observation.diagnostics.is_empty();
            self.log
                .lock()
                .unwrap()
                .lifecycle
                .push((self.id, "terminated"));
        }
    }
    fn complete(&self, completion: (usize, Option<std::process::ExitStatus>)) {
        let mut audit = self.log.lock().unwrap();
        audit.live -= 1;
        audit.lifecycle.push((self.id, "drop"));
        audit.completions.push(ChildCompletion {
            id: self.id,
            spawns: completion.0,
            reaped: completion.1,
            clean: self.clean,
        });
    }
}

struct ObservedGenerator {
    backend: StrictBackend,
    witness: Witness,
    position: u64,
}
impl RenderBackend for ObservedGenerator {
    fn prepare(
        &mut self,
        capture: &CapturedInstrumentBounce,
        control: &RenderControl,
    ) -> Result<crate::instrument_bounce::BouncePluginMetadata, BackendFailure> {
        self.witness.prepare(control);
        self.backend.prepare(capture, control)
    }
    fn metadata(
        &mut self,
    ) -> Result<crate::instrument_bounce::BouncePluginMetadata, BackendFailure> {
        self.backend.metadata()
    }
    fn send_midi(&mut self, midi: [u8; 3], offset: u16) -> Result<(), BackendFailure> {
        self.witness
            .log
            .lock()
            .unwrap()
            .midi
            .push((self.position + u64::from(offset), midi));
        self.backend.send_midi(midi, offset)
    }
    fn process(
        &mut self,
        transport: PluginTransport,
        frames: usize,
    ) -> Result<BlockReply, BackendFailure> {
        assert_eq!(frames, 128);
        assert_eq!(transport.sample_position, self.position as i64);
        self.witness
            .log
            .lock()
            .unwrap()
            .transports
            .push((self.witness.id, transport));
        let reply = self.backend.process(transport, frames)?;
        let peak = reply
            .channels
            .iter()
            .flatten()
            .map(|s| s.abs())
            .fold(0.0, f32::max);
        let mut log = self.witness.log.lock().unwrap();
        log.generator_peak = log.generator_peak.max(peak);
        self.position += frames as u64;
        Ok(reply)
    }
    fn dispose(&mut self, deadline: Instant) -> DisposalObservation {
        let observation = self.backend.dispose(deadline);
        self.witness.disposal(&observation);
        observation
    }
}
impl Drop for ObservedGenerator {
    fn drop(&mut self) {
        self.witness.complete(self.backend.test_child_completion());
    }
}

struct ObservedEffect {
    backend: StrictEffectBackend,
    witness: Witness,
    position: u64,
    upstream: i64,
    samples_per_quarter: f64,
    tempo: f64,
}
impl EffectRenderBackend for ObservedEffect {
    fn prepare(
        &mut self,
        captured: &CapturedEffectSlot,
        control: &RenderControl,
    ) -> Result<EffectSourceMetadata, BackendFailure> {
        self.witness.prepare(control);
        self.backend.prepare(captured, control)
    }
    fn process(
        &mut self,
        input: &[Vec<f32>],
        transport: PluginTransport,
    ) -> Result<BlockReply, BackendFailure> {
        assert_eq!(
            transport.sample_position,
            self.position as i64 - self.upstream
        );
        assert_eq!(
            transport.quarter_note_position,
            transport.sample_position as f64 / self.samples_per_quarter
        );
        assert_eq!(
            (
                transport.tempo,
                transport.time_sig_numerator,
                transport.time_sig_denominator,
                transport.playing
            ),
            (self.tempo, 4, 4, true)
        );
        {
            let mut log = self.witness.log.lock().unwrap();
            log.transports.push((self.witness.id, transport));
            log.effect_input_peak = log
                .effect_input_peak
                .max(input.iter().flatten().map(|s| s.abs()).fold(0.0, f32::max));
        }
        let reply = self.backend.process(input, transport)?;
        if self.position == 0 {
            let offline = self.backend.test_parameter_readback(1005)?;
            assert_eq!(
                offline, 1.0,
                "Actual strict Effect must be active stereo Offline, not matching Realtime"
            );
            self.witness
                .log
                .lock()
                .unwrap()
                .offline_effect_probes
                .push((self.witness.id, offline));
        }
        self.position += 128;
        Ok(reply)
    }
    fn dispose(&mut self, deadline: Instant) -> DisposalObservation {
        let observation = self.backend.dispose(deadline);
        self.witness.disposal(&observation);
        observation
    }
}
impl Drop for ObservedEffect {
    fn drop(&mut self) {
        self.witness.complete(self.backend.test_child_completion());
    }
}

/// Reuses the original closed SDK wrappers for the App Save/Open workflow.
/// These wrappers forward every operand/reply unchanged. They observe only
/// the concrete backend's own retained child completion and owner destruction.
#[derive(Clone, Default)]
pub(crate) struct AppSerialSdkAudit(Log);
impl AppSerialSdkAudit {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn start(
        &self,
        admission: &RenderAdmission,
        project: Project,
        captured: CapturedInstrumentBounce,
        chains: Vec<CapturedSerialEffectChain>,
        options: WavExportOptions,
        path: PathBuf,
        control: RenderControl,
        reviewed: bool,
    ) -> Result<crate::instrument_bounce::render::MasterSongRenderReceipt, String> {
        let generator_id = captured.identity().instance_id;
        let mut prefix = i64::from(captured.metadata().latency_samples);
        let stages: Vec<_> = chains
            .iter()
            .flat_map(|chain| &chain.receipt.slots)
            .filter(|slot| slot.request.config.enabled && !slot.request.config.bypassed)
            .map(|slot| {
                let stage = (slot.request.instance_id, prefix);
                prefix += i64::from(slot.state.metadata.latency_samples);
                stage
            })
            .collect();
        let tempo = f64::from(project.tempo);
        let samples_per_quarter = f64::from(options.sample_rate) * 60.0 / tempo;
        let deadline = control.deadline;
        let generator_log = Arc::clone(&self.0);
        let effect_log = Arc::clone(&self.0);
        let mut index = 0;
        admission.start_serial_with_backends(
            project,
            captured,
            chains,
            options,
            path,
            control,
            reviewed,
            move || ObservedGenerator {
                backend: StrictBackend::new(),
                witness: Witness::new(generator_id, generator_log, deadline),
                position: 0,
            },
            move || {
                let (id, upstream) = stages[index];
                index += 1;
                ObservedEffect {
                    // Ordinary App resolution, not the old SDK explicit-helper override.
                    backend: StrictEffectBackend::new(None),
                    witness: Witness::new(id, Arc::clone(&effect_log), deadline),
                    position: 0,
                    upstream,
                    samples_per_quarter,
                    tempo,
                }
            },
        )
    }
    pub(crate) fn assert_natural_serial_reaps(&self, ids: &[u64], deadline: Instant) {
        let audit = self.0.lock().unwrap();
        assert_eq!((audit.live, audit.max_live), (0, 1));
        assert_eq!(audit.deadlines, vec![deadline; ids.len()]);
        assert_eq!(
            audit.lifecycle,
            ids.iter()
                .flat_map(|&id| [(id, "create"), (id, "terminated"), (id, "drop")])
                .collect::<Vec<_>>()
        );
        assert_eq!(audit.completions.len(), ids.len());
        assert_eq!(
            audit.offline_effect_probes,
            ids.iter().skip(1).map(|&id| (id, 1.0)).collect::<Vec<_>>()
        );
        for (completion, id) in audit.completions.iter().zip(ids) {
            assert_eq!((completion.id, completion.spawns), (*id, 1));
            assert!(
                completion.reaped.is_some_and(|status| status.success()) && completion.clean,
                "actual strict stage {id} did not naturally reap: {:?}, clean={}",
                completion.reaped,
                completion.clean
            );
        }
    }
}

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "citrus-serial-sdk-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn output(&self) -> PathBuf {
        self.0.join("master.wav")
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        if let (Ok(path), Ok(temp)) = (self.0.canonicalize(), std::env::temp_dir().canonicalize())
            && path.starts_with(temp)
            && path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("citrus-serial-sdk-"))
        {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

// Independent four-path convolution of the known musical rectangle. The real
// fixture runs sample-by-sample rings; this oracle scatters each input sample.
fn pre_master_oracle() -> Vec<[f32; 2]> {
    let mut intermediate = vec![[0.0_f32; 2]; 1408];
    let level = (61.0 + 73.0 + 85.0 + 97.0) / 128.0;
    for frame in 0..MUSIC {
        for (delay, coefficient) in [(64, 1.0), (192, 0.5)] {
            intermediate[frame + delay][0] += level * coefficient * BASES[0] as f32;
            intermediate[frame + delay][1] += -0.5 * level * coefficient * BASES[0] as f32;
        }
    }
    for sample in &mut intermediate {
        sample[0] = sample[0] * 1.5 * 1.0 * 0.75;
        sample[1] = sample[1] * 1.5 * 0.75 * 0.75;
    }
    let mut raw = vec![[0.0_f32; 2]; 1408];
    for (frame, sample) in intermediate.into_iter().enumerate() {
        for (delay, coefficient) in [(32, 1.0), (160, 0.5)] {
            if let Some(target) = raw.get_mut(frame + delay) {
                for (target, sample) in target.iter_mut().zip(sample) {
                    *target += sample * coefficient * BASES[1] as f32;
                }
            }
        }
    }
    raw[96..96 + MUSIC + 1024]
        .iter()
        .map(|sample| {
            let left = sample[0] * 1.5 * 0.5;
            let right = sample[1] * 1.5 * 1.0;
            [left, right]
        })
        .collect()
}

#[test]
#[ignore = "requires explicit source-built two-class CITRUS_EFFECT_FIXTURE and matching production CITRUS_EFFECT_HELPER; no installed fallback"]
fn actual_sdk_serial_generator_two_effects_preserves_raw_latency_tail_and_natural_reap() {
    run_sdk_case(false);
}

#[test]
#[ignore = "requires explicit source-built two-class CITRUS_EFFECT_FIXTURE and matching production CITRUS_EFFECT_HELPER; no installed fallback"]
fn actual_sdk_serial_generator_two_effects_master_automation_uses_retained_project_frames() {
    run_sdk_case(true);
}

fn run_sdk_case(automated_master: bool) {
    let _transport_lock = crate::plugins::helper_transport_test_lock();
    let bundle = PathBuf::from(
        std::env::var_os("CITRUS_EFFECT_FIXTURE").expect("set CITRUS_EFFECT_FIXTURE explicitly"),
    )
    .canonicalize()
    .unwrap();
    let expected = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/vst3-effect/target/bundles/CitrusStereoDelayFixture.vst3")
        .canonicalize()
        .unwrap();
    assert_eq!(
        bundle, expected,
        "only the repository's source-built two-class fixture is admitted"
    );
    let helper = PathBuf::from(
        std::env::var_os("CITRUS_EFFECT_HELPER")
            .expect("set matching CITRUS_EFFECT_HELPER explicitly"),
    )
    .canonicalize()
    .unwrap();
    assert!(helper.is_file());
    let mut p = project(&bundle);
    if automated_master {
        master_automation::configure(&mut p, MUSIC as u64, RATE, Mode::All);
    }
    let opts = WavExportOptions {
        sample_rate: RATE,
        sample_format: WavSampleFormat::Float32,
        level_policy: WavLevelPolicy::PreserveLevel,
        tail_milliseconds: 125,
    };

    let mut generator_source = source(&bundle, &helper, GENERATOR, GEN_CLASS, 1.0);
    let captured = capture_generator(&p, &helper, opts, &mut generator_source);
    assert_eq!(generator_source.finish(), RetainedShutdownPoll::Joined);
    assert!(generator_source.chain().guard.join.is_none());
    assert_eq!(generator_source.chain().control.stats().detached_workers, 0);
    drop(generator_source);
    let mut chains = Vec::new();
    for (index, id) in EFFECTS.into_iter().enumerate() {
        let mut owner = source(&bundle, &helper, id, FX_CLASS, BASES[index]);
        chains.push(capture_effect(&p, &owner, index));
        assert_eq!(owner.finish(), RetainedShutdownPoll::Joined);
        assert!(owner.chain().guard.join.is_none());
        assert_eq!(owner.chain().control.stats().detached_workers, 0);
        drop(owner);
    }

    let directory = Directory::new();
    let control = RenderControl {
        export: ExportControl::default(),
        deadline: Instant::now() + Duration::from_secs(300),
    };
    let log = Log::default();
    let generator_log = Arc::clone(&log);
    let effect_log = Arc::clone(&log);
    let deadline = control.deadline;
    let mut index = 0;
    let admission = RenderAdmission::default();
    let receipt = admission
        .start_serial_with_backends(
            p,
            captured,
            chains,
            opts,
            directory.output(),
            control,
            true,
            move || ObservedGenerator {
                backend: StrictBackend::new(),
                witness: Witness::new(GENERATOR, generator_log, deadline),
                position: 0,
            },
            move || {
                let id = EFFECTS[index];
                let upstream = 32 * (index as i64 + 1);
                index += 1;
                ObservedEffect {
                    backend: StrictEffectBackend::new(Some(helper.clone())),
                    witness: Witness::new(id, Arc::clone(&effect_log), deadline),
                    position: 0,
                    upstream,
                    samples_per_quarter: 4096.0,
                    tempo: 120.0,
                }
            },
        )
        .unwrap();
    while !receipt.termination_acknowledged() {
        assert!(
            Instant::now() < deadline,
            "actual serial owner exceeded its original deadline"
        );
        thread::sleep(Duration::from_millis(1));
    }
    let outcome = receipt.try_recv().unwrap();
    assert!(!admission.is_busy());
    let MasterSongRenderOutcome::Complete(report) = outcome else {
        panic!("actual serial SDK render failed: {outcome:?}")
    };
    assert_eq!(
        (
            report.dry_render.native_latency,
            report.dry_render.reported_tail,
            report.dry_render.generated_frames
        ),
        (96, 352, 1408)
    );
    assert_eq!(
        (
            report.master_output.music_frames,
            report.master_output.tail_frames,
            report.master_output.frames
        ),
        (256, 1024, 1280)
    );
    assert_eq!(report.master_output.applied_gain, 1.0);
    assert!(!report.master_output.tail_limit_reached);
    assert!(report.dry_render.sample_peak > 1.0);
    let wav = crate::wav::read_wav(directory.output()).unwrap();
    let expected: Vec<_> = pre_master_oracle()
        .into_iter()
        .enumerate()
        .map(|(frame, sample)| {
            if automated_master {
                master_automation::expected(sample, frame, MUSIC, Mode::All)
            } else {
                [
                    (sample[0] * 0.875 * 1.0).tanh(),
                    (sample[1] * 0.875 * 0.75).tanh(),
                ]
            }
        })
        .collect();
    if automated_master {
        assert_eq!(&wav.samples[..2], &[0.0, 0.0]);
        assert!(wav.samples[64 * 2..128 * 2].iter().all(|&s| s == 0.0));
        assert!(
            wav.samples[MUSIC * 2..].iter().any(|&s| s != 0.0),
            "Tail holds N-1 unmuted, not N's mute point"
        );
    }
    assert_eq!(
        (
            wav.metadata.sample_rate,
            wav.metadata.channels,
            wav.metadata.frames
        ),
        (RATE, 2, 1280)
    );
    assert_eq!(wav.samples.len(), expected.len() * 2);
    for (frame, (actual, expected)) in wav
        .samples
        .as_chunks::<2>()
        .0
        .iter()
        .zip(expected)
        .enumerate()
    {
        for (channel, (&actual, &expected)) in actual.iter().zip(&expected).enumerate() {
            assert!(
                (actual - expected).abs() <= 3.0e-6,
                "actual SDK frame {frame} channel {channel}: {actual} != {expected}",
            );
        }
    }
    let audit = log.lock().unwrap();
    assert_eq!((audit.live, audit.max_live), (0, 1));
    assert_eq!(audit.deadlines, [deadline; 3]);
    assert_eq!(
        audit.lifecycle,
        [GENERATOR, EFFECTS[0], EFFECTS[1]]
            .into_iter()
            .flat_map(|id| [(id, "create"), (id, "terminated"), (id, "drop")])
            .collect::<Vec<_>>()
    );
    assert_eq!(audit.completions.len(), 3);
    assert_eq!(audit.offline_effect_probes, EFFECTS.map(|id| (id, 1.0)));
    for completion in &audit.completions {
        let ChildCompletion {
            id,
            spawns,
            reaped,
            clean,
        } = completion;
        assert_eq!(*spawns, 1, "stage {id}");
        assert!(
            reaped.is_some_and(|status| status.success()) && *clean,
            "stage {id}: {reaped:?}, clean={clean}"
        );
    }
    assert!(audit.generator_peak > 1.0 && audit.effect_input_peak > 1.0);
    assert_eq!(
        audit
            .midi
            .iter()
            .filter(|&&(frame, data)| frame == 256 && data[0] & 0xf0 == 0x80)
            .count(),
        4
    );
    for id in [GENERATOR, EFFECTS[0], EFFECTS[1]] {
        assert_eq!(
            audit
                .transports
                .iter()
                .filter(|&&(stage, _)| stage == id)
                .count(),
            11
        );
    }
    eprintln!(
        "REAL_SERIAL_WHOLE_SDK_PASS source_workers=3_original_joined; source_sdk_process_calls=0_each; source_opaque=40_each; effect_f64_bits={:016x},{:016x}; total_latency=96; reported_tail=352; music=256; fixed_tail=1024; generated=1408; retained=1280; strict_children=3_natural_exit_0_reaped; effect_offline_probe1005=1_each; max_live_strict_owner=1; final_crop=once; final_tanh=once",
        BASES[0].to_bits(),
        BASES[1].to_bits()
    );
    if automated_master {
        eprintln!(
            "REAL_SERIAL_MASTER_AUTOMATION_SDK_PASS total_latency=96; output_control_origin=0; aliases=volume,pan; initially_zero_volume_and_muted_master=unmuted; tail_control_sample=255; crop=once; tanh=once; full_pcm=1280_stereo_samples_compared"
        );
    }
}
