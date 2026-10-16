//! Device-free serial-owner tests. Receipts below are renderer fixtures, not App capture authority.

use super::*;
use crate::export::serial_master::automation_test_support::{self as master_automation, Mode};
use crate::{
    export_options::WavLevelPolicy,
    instrument_bounce::{
        BOUNCE_PREPARED_MAXIMUM, BounceAudioBus, BouncePluginState, InstrumentBouncePlan,
        effect::{EffectCaptureSlotRequest, EffectPluginState, EffectSourceIdentity},
    },
    mixer_graph::{MASTER_MIXER_TRACK_ID, MixerRoute, MixerRouteDestination, MixerRouteTap},
    model::{Clip, ClipKind, PianoNote, PluginInstance, PluginRole, Project},
    plugins::plugin_runtime::{
        PluginEndpointManifest, PluginEndpointSnapshot, PluginLatencySnapshot,
        PluginParameterDescriptor, SlotConfig,
    },
};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::atomic::AtomicU64,
    thread,
};

const RATE: u32 = 8_192;
const MUSIC: usize = 256;
const GENERATOR: u64 = 41;
const FIRST: u64 = 91;
const SECOND: u64 = 92;
const BASES: [f64; 2] = [0.712_345_678_912_345_7, 0.823_456_789_123_456_8];
const SENTINEL: &[u8] = b"serial destination must remain unchanged";
static NEXT: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug)]
struct Fir {
    id: u64,
    latency: usize,
    echo: usize,
    feedback: f32,
    base: f64,
}

fn firs() -> [Fir; 2] {
    [
        Fir {
            id: FIRST,
            latency: 37,
            echo: 139,
            feedback: 0.5,
            base: BASES[0],
        },
        Fir {
            id: SECOND,
            latency: 93,
            echo: 131,
            feedback: -0.25,
            base: BASES[1],
        },
    ]
}

fn options(tail_ms: u32) -> WavExportOptions {
    WavExportOptions {
        sample_rate: RATE,
        sample_format: WavSampleFormat::Float32,
        level_policy: WavLevelPolicy::PreserveLevel,
        tail_milliseconds: tail_ms,
    }
}

fn control() -> RenderControl {
    RenderControl {
        export: ExportControl::default(),
        deadline: Instant::now() + Duration::from_secs(300),
    }
}

fn plugin(id: u64, role: PluginRole) -> PluginInstance {
    PluginInstance {
        id,
        role,
        format: crate::model::PluginFormat::Vst3,
        path: format!("mock-serial-{id}.vst3").into(),
        uid: format!("mock-{id}"),
        vendor: "Renderer fixture".into(),
        name: format!("Mock {id}"),
        enabled: true,
        bypass: false,
        wet: 1.0,
        parameters: match id {
            FIRST => BTreeMap::from([(0, BASES[0] as f32)]),
            SECOND => BTreeMap::from([(0, BASES[1] as f32)]),
            _ => BTreeMap::new(),
        },
        opaque_state: vec![id as u8, 0xE1],
        runtime_status: crate::model::PluginRuntimeStatus::Unloaded,
        midi_ports: Default::default(),
    }
}

fn project() -> Project {
    let mut p = Project::blank();
    p.tempo = 120.0;
    p.song_length_beats = 0.0625; // RATE / 2 = 4096 frames per beat; N is exactly 256.
    p.channels[0].volume = 1.0;
    p.channels[0].pan = 0.0;
    p.channels[0].instrument_plugin_instance_id = Some(GENERATOR);
    p.channels[0].mixer_track = 1;
    for track in &mut p.mixer_tracks {
        track.volume = 1.0;
        track.pan = 0.0;
    }
    p.mixer_tracks[1].volume = 1.25;
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
    p.plugin_instances = vec![
        plugin(GENERATOR, PluginRole::Instrument),
        plugin(FIRST, PluginRole::Effect),
        plugin(SECOND, PluginRole::Effect),
    ];
    p.mixer_insert_slots = vec![
        MixerInsertSlotRef {
            track: 2,
            slot: 3,
            plugin_instance_id: SECOND,
        },
        MixerInsertSlotRef {
            track: 1,
            slot: 0,
            plugin_instance_id: FIRST,
        },
    ];
    p.patterns[0].notes = vec![PianoNote {
        id: 11,
        channel_id: Some(p.channels[0].id),
        group_id: None,
        note: 60,
        start: 0.0,
        length: p.song_length_beats,
        velocity: 1.0,
        selected: false,
        muted: false,
    }];
    p.clips = vec![Clip {
        id: 1,
        track: 0,
        start: 0.0,
        length: p.song_length_beats,
        name: "N-boundary note".into(),
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

fn generator_metadata(latency: u32) -> BouncePluginMetadata {
    BouncePluginMetadata {
        class_uid: "4354525347454E440000000000000001".into(),
        version: "1.0".into(),
        name: "FIFO delayed generator".into(),
        category: "Instrument|Synth".into(),
        input_buses: Vec::new(),
        output_buses: vec![BounceAudioBus::new(2, true, Some(true))],
        event_input_buses: vec![(16, true, Some(true))],
        event_output_buses: Vec::new(),
        latency_samples: latency,
        tail_samples: 0,
    }
}

fn capture(p: &Project, opts: WavExportOptions, latency: u32) -> CapturedInstrumentBounce {
    let plan = InstrumentBouncePlan::prepare_master_song_serial(
        p,
        p.channels[0].id,
        RATE,
        opts.tail_frames().unwrap() as u32,
    )
    .unwrap();
    assert_eq!(plan.timeline().duration_frames(), MUSIC as u64);
    CapturedInstrumentBounce {
        identity: BounceSourceIdentity {
            project_session: 11,
            project_fingerprint: plan.project_fingerprint(),
            timeline_revision: 12,
            epoch: 13,
            endpoint_id: 14,
            channel_id: plan.channel_id(),
            instance_id: GENERATOR,
        },
        plan,
        receipt_token: Arc::new(()),
        state: BouncePluginState {
            metadata: generator_metadata(latency),
            opaque_state: vec![41, 0xE1],
            parameter_bases: BTreeMap::new(),
            native_generation: 2,
            native_dirty_revision: 9,
        },
    }
}

fn chains(p: &Project, specs: [Fir; 2]) -> Vec<CapturedSerialEffectChain> {
    specs
        .into_iter()
        .map(|spec| {
            let placement = p
                .mixer_insert_slots
                .iter()
                .find(|slot| slot.plugin_instance_id == spec.id)
                .unwrap()
                .clone();
            let instance = p.plugin_instances.iter().find(|p| p.id == spec.id).unwrap();
            let active = instance.enabled && !instance.bypass;
            let request = EffectCaptureSlotRequest {
                instance_id: spec.id,
                config: SlotConfig {
                    enabled: instance.enabled,
                    bypassed: instance.bypass,
                    wet: instance.wet,
                },
                path: format!("mock-serial-{}.vst3", spec.id).into(),
                requested_class_uid: None,
                parameter_ids: vec![0],
            };
            let state = EffectPluginState {
                metadata: EffectSourceMetadata {
                    identity: EffectSourceIdentity {
                        class_uid: format!("{:032X}", spec.id),
                        vendor: "Renderer fixture".into(),
                        name: format!("FIR {}", spec.id),
                        version: "1.0".into(),
                        category: "Fx|Delay".into(),
                    },
                    path: request.path.clone(),
                    requested_class_uid: None,
                    sample_rate: f64::from(RATE),
                    max_block_frames: BOUNCE_PREPARED_MAXIMUM,
                    tempo: 120.0,
                    time_sig_numerator: 4,
                    time_sig_denominator: 4,
                    input_buses: vec![BounceAudioBus::new(2, true, Some(true))],
                    output_buses: vec![BounceAudioBus::new(2, true, Some(true))],
                    event_input_buses: Vec::new(),
                    event_output_buses: Vec::new(),
                    latency_samples: spec.latency as u32,
                    tail_samples: if spec.feedback == 0.0 {
                        spec.latency as u32
                    } else {
                        (spec.latency + spec.echo) as u32
                    },
                    async_started_revision: 17,
                },
                opaque_state: vec![spec.id as u8, 0xE1],
                catalog_revision: 1,
                catalog: vec![PluginParameterDescriptor {
                    id: 0,
                    name: "Gain".into(),
                    unit: String::new(),
                    current_normalized: spec.base as f32,
                    default_normalized: Some(0.5),
                    step_count: Some(0),
                    automatable: true,
                    read_only: false,
                    bypass: false,
                }],
                parameter_bases: BTreeMap::from([(0, spec.base)]),
                native_generation: 2,
                native_dirty_revision: 5,
            };
            state.validate(&[0]).unwrap();
            let mut latency = PluginLatencySnapshot {
                revision: 1,
                active_mask: if active { 1 } else { 0 },
                total_plugin_latency_samples: if active { spec.latency as u32 } else { 0 },
                tail_samples: if active {
                    state.metadata.tail_samples
                } else {
                    0
                },
                ..Default::default()
            };
            latency.slot_latency_samples[0] = if active { spec.latency as u32 } else { 0 };
            let endpoint = PluginEndpointSnapshot::try_new(
                PluginEndpointManifest::identified(&[spec.id]).unwrap(),
                latency,
            )
            .unwrap();
            CapturedSerialEffectChain {
                runtime_track: placement.track as usize,
                placements: vec![placement],
                receipt: EffectChainStateReceipt {
                    epoch: 13,
                    endpoint,
                    slots: vec![CapturedEffectSlot { request, state }],
                    submitted: 0,
                    admin_revision: 0,
                    epoch_resets: 0,
                },
            }
        })
        .collect()
}

#[derive(Default)]
struct Audit {
    live: usize,
    max_live: usize,
    lifecycle: Vec<(u64, &'static str)>,
    deadlines: Vec<(u64, Instant)>,
    midi: Vec<(u64, [u8; 3])>,
    transports: Vec<(u64, PluginTransport)>,
    effect_input_peak: Vec<(u64, f32)>,
    source_base_bits: Vec<(u64, u64)>,
}
type Log = Arc<Mutex<Audit>>;

struct Lease {
    id: u64,
    log: Log,
    polls: usize,
    terminated: bool,
}
impl Lease {
    fn new(id: u64, log: Log) -> Self {
        {
            let mut audit = log.lock().unwrap();
            audit.live += 1;
            audit.max_live = audit.max_live.max(audit.live);
            audit.lifecycle.push((id, "create"));
        }
        Self {
            id,
            log,
            polls: 0,
            terminated: false,
        }
    }
    fn prepared(&self, control: &RenderControl, expected: Instant) {
        assert_eq!(
            control.deadline, expected,
            "each stage must share the original absolute deadline"
        );
        let mut log = self.log.lock().unwrap();
        log.deadlines.push((self.id, control.deadline));
        log.lifecycle.push((self.id, "prepare"));
    }
    fn dispose(&mut self, _: Instant) -> DisposalObservation {
        self.polls += 1;
        self.terminated = self.polls >= 2;
        self.log.lock().unwrap().lifecycle.push((
            self.id,
            if self.terminated {
                "terminated"
            } else {
                "pending"
            },
        ));
        DisposalObservation {
            terminated: self.terminated,
            error: None,
            diagnostics: Vec::new(),
        }
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        assert!(
            self.terminated,
            "mock owner dropped before its disposal acknowledgment"
        );
        let mut log = self.log.lock().unwrap();
        log.lifecycle.push((self.id, "drop"));
        log.live -= 1;
    }
}

struct DelayedGenerator {
    lease: Lease,
    expected_deadline: Instant,
    metadata: BouncePluginMetadata,
    delay: VecDeque<[f32; 2]>,
    frame: u64,
    active: bool,
    scale: f32,
    pending: Vec<(u16, [u8; 3])>,
}
impl DelayedGenerator {
    fn new(latency: u32, log: Log, deadline: Instant) -> Self {
        Self {
            lease: Lease::new(GENERATOR, log),
            expected_deadline: deadline,
            metadata: generator_metadata(latency),
            delay: std::iter::repeat_n([0.0; 2], latency as usize).collect(),
            frame: 0,
            active: false,
            scale: 1.0,
            pending: Vec::new(),
        }
    }
}
fn pulse(frame: usize) -> [f32; 2] {
    match frame {
        0 => [2.0, -1.5],
        127 => [-0.75, 0.5],
        128 => [3.0, -2.0],
        255 => [1.25, -0.25],
        _ => [0.0; 2],
    }
}
impl RenderBackend for DelayedGenerator {
    fn prepare(
        &mut self,
        capture: &CapturedInstrumentBounce,
        control: &RenderControl,
    ) -> Result<BouncePluginMetadata, BackendFailure> {
        self.lease.prepared(control, self.expected_deadline);
        assert_eq!(capture.state.opaque_state, [41, 0xE1]);
        Ok(self.metadata.clone())
    }
    fn metadata(&mut self) -> Result<BouncePluginMetadata, BackendFailure> {
        Ok(self.metadata.clone())
    }
    fn send_midi(&mut self, midi: [u8; 3], offset: u16) -> Result<(), BackendFailure> {
        self.lease
            .log
            .lock()
            .unwrap()
            .midi
            .push((self.frame + u64::from(offset), midi));
        self.pending.push((offset, midi));
        Ok(())
    }
    fn process(
        &mut self,
        transport: PluginTransport,
        frames: usize,
    ) -> Result<BlockReply, BackendFailure> {
        assert_eq!(frames, BOUNCE_QUANTUM);
        assert_eq!(transport.sample_position, self.frame as i64);
        self.lease
            .log
            .lock()
            .unwrap()
            .transports
            .push((GENERATOR, transport));
        let mut outputs = vec![vec![0.0; frames]; 2];
        let (left, right) = outputs.split_at_mut(1);
        for (index, (left, right)) in left[0].iter_mut().zip(right[0].iter_mut()).enumerate() {
            for &(_, midi) in self
                .pending
                .iter()
                .filter(|(offset, _)| usize::from(*offset) == index)
            {
                self.active = midi[0] & 0xf0 == 0x90 && midi[2] != 0;
            }
            self.delay.push_back(if self.active {
                pulse(self.frame as usize + index).map(|sample| sample * self.scale)
            } else {
                [0.0; 2]
            });
            let delayed = self.delay.pop_front().unwrap();
            *left = delayed[0];
            *right = delayed[1];
        }
        self.pending.clear();
        self.frame += frames as u64;
        Ok(reply(outputs))
    }
    fn dispose(&mut self, deadline: Instant) -> DisposalObservation {
        self.lease.dispose(deadline)
    }
}

#[derive(Clone, Copy, Debug)]
enum Fault {
    None,
    Fail,
    Cancel,
    FailAndCancel,
    Deadline,
    Invalid,
}

struct FirEffect {
    lease: Lease,
    spec: Fir,
    control: RenderControl,
    fault: Fault,
    history: Vec<[f32; 2]>,
    upstream: usize,
    restored_base: f64,
}
impl EffectRenderBackend for FirEffect {
    fn prepare(
        &mut self,
        slot: &CapturedEffectSlot,
        control: &RenderControl,
    ) -> Result<EffectSourceMetadata, BackendFailure> {
        self.lease.prepared(control, self.control.deadline);
        assert_eq!(slot.request.instance_id, self.spec.id);
        assert_eq!(slot.state.opaque_state, [self.spec.id as u8, 0xE1]);
        self.restored_base = slot.state.parameter_bases[&0];
        assert_eq!(self.restored_base.to_bits(), self.spec.base.to_bits());
        assert_ne!(
            f64::from(self.restored_base as f32).to_bits(),
            self.restored_base.to_bits()
        );
        self.lease
            .log
            .lock()
            .unwrap()
            .source_base_bits
            .push((self.spec.id, self.restored_base.to_bits()));
        let mut actual = slot.state.metadata.clone();
        actual.async_started_revision = self.spec.id + 700; // Fresh clone, not the source generation.
        Ok(actual)
    }
    fn process(
        &mut self,
        input: &[Vec<f32>],
        transport: PluginTransport,
    ) -> Result<BlockReply, BackendFailure> {
        assert_eq!(input.len(), 2);
        assert!(input.iter().all(|channel| channel.len() == BOUNCE_QUANTUM));
        assert_eq!(
            transport.sample_position,
            self.history.len() as i64 - self.upstream as i64
        );
        assert_eq!(
            transport.quarter_note_position,
            transport.sample_position as f64 / 4096.0
        );
        assert!(transport.playing);
        assert_eq!(
            (
                transport.tempo,
                transport.time_sig_numerator,
                transport.time_sig_denominator
            ),
            (120.0, 4, 4)
        );
        {
            let mut log = self.lease.log.lock().unwrap();
            log.transports.push((self.spec.id, transport));
            log.effect_input_peak.push((
                self.spec.id,
                input.iter().flatten().map(|s| s.abs()).fold(0.0, f32::max),
            ));
        }
        if matches!(self.fault, Fault::Cancel | Fault::FailAndCancel) {
            self.control.export.cancel();
        }
        if matches!(
            self.fault,
            Fault::Fail | Fault::FailAndCancel | Fault::Deadline
        ) {
            return Err(BackendFailure::new(
                if matches!(self.fault, Fault::Deadline) {
                    BackendFailureKind::Deadline
                } else {
                    BackendFailureKind::Backend
                },
                "injected serial Effect failure",
            )
            .with_delivery(BackendDelivery::MayHaveApplied));
        }
        let mut outputs = vec![vec![0.0; BOUNCE_QUANTUM]; 2];
        for (index, (&left, &right)) in input[0].iter().zip(&input[1]).enumerate() {
            self.history.push([left, right]);
            let now = self.history.len() - 1;
            for (channel, output) in outputs.iter_mut().enumerate() {
                let direct = now
                    .checked_sub(self.spec.latency)
                    .map_or(0.0, |frame| self.history[frame][channel]);
                let echo = now
                    .checked_sub(self.spec.latency + self.spec.echo)
                    .map_or(0.0, |frame| self.history[frame][channel]);
                output[index] = self.restored_base as f32 * (direct + self.spec.feedback * echo);
            }
        }
        if matches!(self.fault, Fault::Invalid) {
            outputs[0][0] = f32::NAN;
        }
        Ok(reply(outputs))
    }
    fn dispose(&mut self, deadline: Instant) -> DisposalObservation {
        self.lease.dispose(deadline)
    }
}

fn reply(channels: Vec<Vec<f32>>) -> BlockReply {
    BlockReply {
        channels,
        transport_applied: true,
        output_events: 0,
        output_events_lost: false,
    }
}

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "citrus-serial-render-{}-{}",
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
                .is_some_and(|name| name.to_string_lossy().starts_with("citrus-serial-render-"))
        {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

fn effect_factory(
    specs: [Fir; 2],
    latency: u32,
    log: Log,
    control: RenderControl,
    fault: Fault,
) -> impl FnMut() -> FirEffect + Send + 'static {
    let mut index = 0;
    move || {
        let spec = specs[index];
        let upstream = latency as usize
            + specs[..index]
                .iter()
                .map(|spec| spec.latency)
                .sum::<usize>();
        let stage_fault = if index == 1 { fault } else { Fault::None };
        index += 1;
        FirEffect {
            lease: Lease::new(spec.id, Arc::clone(&log)),
            spec,
            control: control.clone(),
            fault: stage_fault,
            history: Vec::new(),
            upstream,
            restored_base: 0.0,
        }
    }
}

// Independent scatter convolution, not the streaming mocks' history lookup.
fn convolve(input: &[[f32; 2]], spec: Fir) -> Vec<[f32; 2]> {
    let mut output = vec![[0.0; 2]; input.len()];
    for (position, sample) in input.iter().enumerate() {
        for (delay, coefficient) in [
            (spec.latency, 1.0),
            (spec.latency + spec.echo, spec.feedback),
        ] {
            if let Some(target) = output.get_mut(position + delay) {
                for (target, sample) in target.iter_mut().zip(sample) {
                    *target += sample * coefficient * spec.base as f32;
                }
            }
        }
    }
    output
}

fn pre_master_oracle(specs: [Fir; 2], latency: u32, tail: usize) -> (Vec<[f32; 2]>, u64, f32) {
    let total_latency = latency as usize + specs.iter().map(|s| s.latency).sum::<usize>();
    let needed = (MUSIC + tail + total_latency).max(MUSIC + 1);
    let generated = needed.div_ceil(128) * 128;
    let mut raw = vec![[0.0; 2]; generated];
    for frame in 0..MUSIC {
        raw[frame + latency as usize] = pulse(frame);
    }
    raw = convolve(&raw, specs[0]);
    for sample in &mut raw {
        sample[0] = sample[0].clamp(-8.0, 8.0) * 1.25 * 1.0;
        sample[1] = sample[1].clamp(-8.0, 8.0) * 1.25 * 0.75;
        sample[0] *= 0.75;
        sample[1] *= 0.75;
    }
    raw = convolve(&raw, specs[1]);
    for sample in &mut raw {
        sample[0] = sample[0].clamp(-8.0, 8.0) * 1.5 * 0.5;
        sample[1] = sample[1].clamp(-8.0, 8.0) * 1.5 * 1.0;
    }
    let mut peak = 0.0_f32;
    let retained = raw[total_latency..total_latency + MUSIC + tail]
        .iter()
        .map(|sample| {
            peak = peak.max(sample[0].abs()).max(sample[1].abs());
            *sample
        })
        .collect();
    (retained, generated as u64, peak)
}

fn oracle(specs: [Fir; 2], latency: u32, tail: usize) -> (Vec<[f32; 2]>, u64, f32) {
    let (raw, generated, peak) = pre_master_oracle(specs, latency, tail);
    (
        raw.into_iter()
            .map(|s| [(s[0] * 0.875 * 1.0).tanh(), (s[1] * 0.875 * 0.75).tanh()])
            .collect(),
        generated,
        peak,
    )
}

#[test]
fn serial_owned_master_automation_after_positive_latency_matches_every_retained_sample() {
    for mode in [Mode::All, Mode::Volume, Mode::Pan, Mode::Mute] {
        let directory = Directory::new();
        let mut p = project();
        master_automation::configure(&mut p, MUSIC as u64, RATE, mode);
        let original = serde_json::to_vec(&p).unwrap();
        let opts = options(125);
        let specs = firs();
        let ctl = control();
        let log = Log::default();
        let generator_log = Arc::clone(&log);
        let captured = capture(&p, opts, 29);
        assert_eq!(
            captured.identity().project_fingerprint,
            captured.plan().project_fingerprint()
        );
        let result = report(run_serial_owned(
            p.clone(),
            captured,
            chains(&p, specs),
            opts,
            directory.output(),
            &ctl,
            &JobProgress::new(),
            true,
            || DelayedGenerator::new(29, generator_log, ctl.deadline),
            effect_factory(specs, 29, Arc::clone(&log), ctl.clone(), Fault::None),
        ));
        let tail = opts.tail_frames().unwrap();
        let (raw, generated, _) = pre_master_oracle(specs, 29, tail);
        let wav = crate::wav::read_wav(directory.output()).unwrap();
        assert_eq!(result.dry_render.native_latency, 159);
        assert_eq!(result.dry_render.generated_frames, generated);
        assert_eq!(result.master_output.frames, MUSIC + tail);
        for (frame, (actual, raw)) in wav.samples.as_chunks::<2>().0.iter().zip(raw).enumerate() {
            let expected = master_automation::expected(raw, frame, MUSIC, mode);
            for (channel, (&actual, expected)) in actual.iter().zip(expected).enumerate() {
                assert!(
                    (actual - expected).abs() <= 3.0e-6,
                    "{mode:?} retained j={frame}, channel={channel}: {actual} != {expected}"
                );
            }
        }
        assert!(
            wav.samples[MUSIC * 2..].iter().any(|&s| s != 0.0),
            "hold N-1, not N's mute point"
        );
        assert_eq!(serde_json::to_vec(&p).unwrap(), original);
        check_lifetimes(&log, ctl.deadline, &[GENERATOR, FIRST, SECOND]);
    }
}

#[test]
fn serial_master_automated_unmute_does_not_reopen_channel_track_or_solo_gates() {
    for gate in 0..4 {
        let directory = Directory::new();
        let mut p = project();
        master_automation::configure(&mut p, MUSIC as u64, RATE, Mode::All);
        match gate {
            0 => p.channels[0].muted = true,
            1 => p.mixer_tracks[1].muted = true,
            2 => p.mixer_tracks[2].muted = true,
            _ => p.mixer_tracks[3].solo = true,
        }
        let opts = options(125);
        let ctl = control();
        let log = Log::default();
        let generator_log = Arc::clone(&log);
        report(run_serial_owned(
            p.clone(),
            capture(&p, opts, 29),
            chains(&p, firs()),
            opts,
            directory.output(),
            &ctl,
            &JobProgress::new(),
            true,
            || DelayedGenerator::new(29, generator_log, ctl.deadline),
            effect_factory(firs(), 29, Arc::clone(&log), ctl.clone(), Fault::None),
        ));
        assert!(
            crate::wav::read_wav(directory.output())
                .unwrap()
                .samples
                .iter()
                .all(|&s| s == 0.0),
            "gate {gate}"
        );
        check_lifetimes(&log, ctl.deadline, &[GENERATOR, FIRST, SECOND]);
    }
}

fn check_lifetimes(log: &Log, deadline: Instant, stages: &[u64]) {
    let log = log.lock().unwrap();
    assert_eq!(log.live, 0);
    assert_eq!(log.max_live, 1);
    assert_eq!(
        log.deadlines,
        stages
            .iter()
            .map(|&stage| (stage, deadline))
            .collect::<Vec<_>>()
    );
    let expected: Vec<_> = stages
        .iter()
        .flat_map(|&id| {
            ["create", "prepare", "pending", "terminated", "drop"].map(|event| (id, event))
        })
        .collect();
    assert_eq!(
        log.lifecycle, expected,
        "next owner must be created only after termination AND Drop"
    );
}

fn report(outcome: MasterSongRenderOutcome) -> MasterSongRenderReport {
    match outcome {
        MasterSongRenderOutcome::Complete(report) => report,
        MasterSongRenderOutcome::Failed(error) => panic!("{error:?}"),
    }
}

#[test]
fn serial_owned_positive_latency_two_effects_match_independent_convolution_and_crop_once() {
    for tail_ms in [1, 125] {
        let directory = Directory::new();
        let p = project();
        let opts = options(tail_ms);
        let specs = firs();
        let ctl = control();
        let log = Log::default();
        let generator_log = Arc::clone(&log);
        let capture = capture(&p, opts, 29);
        let result = report(run_serial_owned(
            p.clone(),
            capture,
            chains(&p, specs),
            opts,
            directory.output(),
            &ctl,
            &JobProgress::new(),
            true,
            || DelayedGenerator::new(29, generator_log, ctl.deadline),
            effect_factory(specs, 29, Arc::clone(&log), ctl.clone(), Fault::None),
        ));
        let tail = opts.tail_frames().unwrap();
        let (expected, generated, dry_peak) = oracle(specs, 29, tail);
        assert!(
            dry_peak > 1.0,
            "internal PCM must remain above unity until the single final tanh"
        );
        let wav = crate::wav::read_wav(directory.output()).unwrap();
        assert_eq!(
            (
                wav.metadata.sample_rate,
                wav.metadata.channels,
                wav.metadata.frames
            ),
            (RATE, 2, (MUSIC + tail) as u64)
        );
        assert_eq!(result.dry_render.native_latency, 159);
        assert_eq!(result.dry_render.generated_frames, generated);
        assert_eq!(result.dry_render.frames, MUSIC + tail);
        assert_eq!(result.dry_render.fixed_tail, tail as u32);
        assert_eq!(result.dry_render.reported_tail, 400);
        assert!(result.dry_render.sample_peak > 1.0);
        assert_eq!(result.master_output.music_frames, MUSIC);
        assert_eq!(result.master_output.tail_frames, tail);
        assert_eq!(result.master_output.applied_gain, 1.0);
        assert_eq!(result.master_output.tail_limit_reached, 400 > tail);
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
                    (actual - expected).abs() <= 2.0e-6,
                    "frame {frame} channel {channel}: {actual} != {expected}",
                );
            }
        }
        check_lifetimes(&log, ctl.deadline, &[GENERATOR, FIRST, SECOND]);
        let audit = log.lock().unwrap();
        assert_eq!(
            audit.source_base_bits,
            [(FIRST, BASES[0].to_bits()), (SECOND, BASES[1].to_bits())]
        );
        assert!(
            audit
                .effect_input_peak
                .iter()
                .any(|&(id, peak)| id == SECOND && peak > 1.0)
        );
        assert!(
            audit
                .midi
                .iter()
                .any(|&(frame, data)| frame == MUSIC as u64 && data[0] & 0xf0 == 0x80)
        );
        for stage in [GENERATOR, FIRST, SECOND] {
            assert_eq!(
                audit
                    .transports
                    .iter()
                    .filter(|&&(id, _)| id == stage)
                    .count(),
                generated as usize / 128
            );
        }
    }
}

#[test]
fn serial_inactive_effects_skip_factory_and_dsp_latency_but_keep_downstream_endpoint_clamp() {
    for source_volume in [0.5, 1.5] {
        let directory = Directory::new();
        let mut p = project();
        p.channels[0].volume = 1.0;
        p.mixer_tracks[1].volume = source_volume;
        p.mixer_tracks[2].volume = 1.0;
        p.mixer_tracks[0].volume = 0.03125;
        for track in &mut p.mixer_tracks {
            track.pan = 0.0;
        }
        for route in &mut p.mixer_routes {
            route.gain = 1.0;
        }
        for plugin in &mut p.plugin_instances {
            if plugin.id == FIRST {
                plugin.enabled = false;
                plugin.wet = 0.25;
            }
            if plugin.id == SECOND {
                plugin.bypass = true;
                plugin.wet = 0.5;
            }
        }
        let opts = options(0);
        let ctl = control();
        let log = Log::default();
        let generator_log = Arc::clone(&log);
        let rendered = report(run_serial_owned(
            p.clone(),
            capture(&p, opts, 29),
            chains(&p, firs()),
            opts,
            directory.output(),
            &ctl,
            &JobProgress::new(),
            true,
            || {
                let mut generator = DelayedGenerator::new(29, generator_log, ctl.deadline);
                generator.scale = 4.0;
                generator
            },
            || -> FirEffect { panic!("disabled/bypassed Effect factory must not run") },
        ));
        assert_eq!(
            (
                rendered.dry_render.native_latency,
                rendered.dry_render.reported_tail,
                rendered.dry_render.generated_frames
            ),
            (29, 0, 384)
        );
        let actual = crate::wav::read_wav(directory.output()).unwrap();
        assert_eq!(actual.samples.len(), MUSIC * 2);
        for (frame, actual) in actual.samples.as_chunks::<2>().0.iter().enumerate() {
            let expected = pulse(frame).map(|sample| {
                let channel = (sample * 4.0).clamp(-8.0, 8.0);
                let source_endpoint = channel.clamp(-8.0, 8.0) * source_volume;
                let bus_endpoint = source_endpoint.clamp(-8.0, 8.0);
                (bus_endpoint * 0.03125).tanh()
            });
            assert_eq!(
                *actual, expected,
                "inactive endpoint clamp at frame {frame}"
            );
        }
        // The generator exceeds the raw safety range. A legal 1.5 track gain
        // then drives the next bypassed endpoint to 12, which must clamp to 8.
        assert!(pulse(128)[0] * 4.0 > 8.0);
        if source_volume == 1.5 {
            assert_eq!(actual.samples[128 * 2], (8.0_f32 * 0.03125).tanh());
            assert_ne!(actual.samples[128 * 2], (12.0_f32 * 0.03125).tanh());
        }
        check_lifetimes(&log, ctl.deadline, &[GENERATOR]);
        let audit = log.lock().unwrap();
        assert!(audit.effect_input_peak.is_empty() && audit.source_base_bits.is_empty());
    }
}

#[test]
fn serial_async_owner_processes_noteoff_at_n_in_the_extra_q128_then_releases_admission() {
    let directory = Directory::new();
    let p = project();
    let opts = options(0);
    let ctl = control();
    let specs = firs().map(|spec| Fir {
        latency: 0,
        echo: 0,
        feedback: 0.0,
        ..spec
    });
    let capture = capture(&p, opts, 0);
    let receipts = chains(&p, specs);
    let log = Log::default();
    let generator_log = Arc::clone(&log);
    let deadline = ctl.deadline;
    let admission = RenderAdmission::default();
    let receipt = admission
        .start_serial_with_backends(
            p,
            capture,
            receipts,
            opts,
            directory.output(),
            ctl.clone(),
            true,
            move || DelayedGenerator::new(0, generator_log, deadline),
            effect_factory(specs, 0, Arc::clone(&log), ctl, Fault::None),
        )
        .unwrap();
    let wait_deadline = Instant::now() + Duration::from_secs(8);
    while !receipt.termination_acknowledged() {
        assert!(Instant::now() < wait_deadline);
        thread::sleep(Duration::from_millis(1));
    }
    let rendered = report(receipt.try_recv().unwrap());
    assert!(!admission.is_busy());
    assert_eq!(
        (
            rendered.dry_render.frames,
            rendered.dry_render.generated_frames
        ),
        (256, 384)
    );
    check_lifetimes(&log, deadline, &[GENERATOR, FIRST, SECOND]);
    let audit = log.lock().unwrap();
    assert!(
        audit
            .midi
            .iter()
            .any(|&(frame, data)| frame == 256 && data[0] & 0xf0 == 0x80)
    );
    assert_eq!(
        audit
            .transports
            .iter()
            .filter(|&&(id, _)| id == GENERATOR)
            .map(|(_, transport)| transport.sample_position)
            .collect::<Vec<_>>(),
        [0, 128, 256]
    );
    assert_eq!(
        crate::wav::read_wav(directory.output())
            .unwrap()
            .metadata
            .frames,
        256
    );
}

#[test]
fn serial_effect_failure_cancel_deadline_and_invalid_pcm_keep_destination_unchanged() {
    for fault in [
        Fault::Fail,
        Fault::Cancel,
        Fault::FailAndCancel,
        Fault::Deadline,
        Fault::Invalid,
    ] {
        let directory = Directory::new();
        std::fs::write(directory.output(), SENTINEL).unwrap();
        let p = project();
        let opts = options(1);
        let ctl = control();
        let specs = firs();
        let log = Log::default();
        let generator_log = Arc::clone(&log);
        let outcome = run_serial_owned(
            p.clone(),
            capture(&p, opts, 29),
            chains(&p, specs),
            opts,
            directory.output(),
            &ctl,
            &JobProgress::new(),
            true,
            || DelayedGenerator::new(29, generator_log, ctl.deadline),
            effect_factory(specs, 29, Arc::clone(&log), ctl.clone(), fault),
        );
        let MasterSongRenderOutcome::Failed(error) = outcome else {
            panic!("fault {fault:?} was published")
        };
        let expected = match fault {
            Fault::Cancel => FailureKind::Cancelled,
            Fault::Deadline => FailureKind::Deadline,
            Fault::Invalid => FailureKind::Invalid,
            _ => FailureKind::Backend,
        };
        assert_eq!(error.kind, expected, "{fault:?}: {error:?}");
        if matches!(fault, Fault::Fail | Fault::FailAndCancel | Fault::Deadline) {
            assert_eq!(
                error.backend.as_ref().unwrap().delivery,
                Some(BackendDelivery::MayHaveApplied)
            );
        }
        assert_eq!(std::fs::read(directory.output()).unwrap(), SENTINEL);
        assert_eq!(std::fs::read_dir(&directory.0).unwrap().count(), 1);
        check_lifetimes(&log, ctl.deadline, &[GENERATOR, FIRST, SECOND]);
        assert_eq!(
            log.lock()
                .unwrap()
                .transports
                .iter()
                .filter(|&&(id, _)| id == SECOND)
                .count(),
            1
        );
    }
}

#[test]
fn serial_expired_original_budget_starts_no_backend_and_preserves_destination() {
    let directory = Directory::new();
    std::fs::write(directory.output(), SENTINEL).unwrap();
    let p = project();
    let opts = options(1);
    let capture = capture(&p, opts, 29);
    let specs = firs();
    let mut ctl = control();
    ctl.deadline = Instant::now();
    let log = Log::default();
    let outcome = run_serial_owned(
        p.clone(),
        capture,
        chains(&p, specs),
        opts,
        directory.output(),
        &ctl,
        &JobProgress::new(),
        true,
        || -> DelayedGenerator { panic!("expired job constructed a generator") },
        effect_factory(specs, 29, Arc::clone(&log), ctl.clone(), Fault::None),
    );
    let MasterSongRenderOutcome::Failed(error) = outcome else {
        panic!("expired job published")
    };
    assert_eq!(error.kind, FailureKind::Deadline);
    assert_eq!(log.lock().unwrap().live, 0);
    assert!(log.lock().unwrap().lifecycle.is_empty());
    assert_eq!(std::fs::read(directory.output()).unwrap(), SENTINEL);
}

#[test]
fn serial_raw_frame_bounds_include_closing_note_and_round_without_allocating() {
    for (song, tail, latency, expected) in [
        (128, 0, 0, (128, 256)),
        (129, 0, 0, (129, 256)),
        (1, 0, 127, (1, 128)),
        (128, 1, 0, (129, 256)),
        (
            MAX_BOUNCE_FRAMES - 1,
            0,
            0,
            ((MAX_BOUNCE_FRAMES - 1) as usize, MAX_BOUNCE_FRAMES),
        ),
        (
            MAX_BOUNCE_FRAMES - 128,
            0,
            128,
            ((MAX_BOUNCE_FRAMES - 128) as usize, MAX_BOUNCE_FRAMES),
        ),
    ] {
        assert_eq!(render_frame_bounds(song, tail, latency).unwrap(), expected);
    }
    for (song, tail, latency) in [
        (0, 0, 0),
        (MAX_BOUNCE_FRAMES, 0, 0),
        (MAX_BOUNCE_FRAMES - 128, 1, 128),
        (u64::MAX, 1, 0),
        (u64::MAX, 0, 1),
        (u64::MAX, 0, 0),
        (u64::MAX - 1, 0, 0),
    ] {
        assert_eq!(
            render_frame_bounds(song, tail, latency).unwrap_err().kind,
            FailureKind::Invalid
        );
    }
}
