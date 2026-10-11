//! Ignored, explicit-run-only qualification of the actual selected-clip App path.
//!
//! SOURCE PREPARATION ONLY. Ordinary builds/discovery cannot run this test. The
//! independent IEEE/WAV oracle preserves the imported numerical operations and
//! its 22 tests. Adaptations remove the obsolete standalone Gate test-module
//! registration and apply mechanical Clippy style changes. Original
//! import SHA256 1bff180934231ca40cf92c079038f45b4dddfdd2ad29a6df9216174b12a0d8b2.
//! The UI boundary is a headlessly rendered real review plus direct invocation
//! of the exact begin_clip_render_stop method used by its Render-and-convert
//! button. This does not claim pointer activation, a native window or a device.
use super::*;
use crate::{
    audio::genuine_observation::{
        GenuineCallbackSnapshot, GenuineSegmentDisposition, GenuineSegmentObservation,
    },
    genuine_app_observer::{
        self as observe, Boundary, CallOutcome, CaptureLabel, InstancePhase, ObservationRun,
        Operation, RecordKind, Stage,
    },
    instrument_bounce::{BounceAudioBus, BouncePluginMetadata},
    mixer_graph::{MASTER_MIXER_TRACK_ID, MixerRoute, MixerRouteDestination, MixerRouteTap},
    plugins::plugin_runtime::RuntimeEvent,
};
use serde_json::{Value, json};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::fs::OpenOptionsExt,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::atomic::AtomicBool,
};
#[path = "clip_render_genuine_health.rs"]
mod health;
#[path = "clip_render_genuine_oracles.rs"]
mod oracles;
#[path = "clip_render_genuine_review.rs"]
mod review;
#[path = "clip_render_genuine_startup.rs"]
mod startup;
#[path = "clip_render_genuine_support.rs"]
mod support;
#[path = "clip_render_genuine_topology.rs"]
pub(crate) mod topology;
use health::{HealthClass, HealthStatus};
use support::*;
use topology::{OwnedTopologyLedger, PumpPurpose};
const GLOBAL_VOLUME: u32 = 1_336_600_346;
const SET_REQUEST: u64 = 0xA660_0002;
const QUERY_REQUEST: u64 = 0xA660_0003;
const CATALOG_REQUEST: u64 = 0xA660_0001;
const SENTINEL: &[u8] = b"selected-clip genuine App sibling sentinel; never rewritten\n";
static ADMITTED: AtomicBool = AtomicBool::new(false);
type Check<T = ()> = Result<T, String>;
fn require(ok: bool, reason: &str) -> Check {
    if ok { Ok(()) } else { Err(reason.into()) }
}
fn error(error: impl std::fmt::Display) -> String {
    error.to_string()
}
fn json_bytes(value: &impl serde::Serialize) -> Check<Vec<u8>> {
    serde_json::to_vec(value).map_err(error)
}
fn private_write(path: &Path, bytes: &[u8]) -> Check {
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(error)?;
    f.write_all(bytes).and_then(|_| f.sync_all()).map_err(error)
}

struct Journal {
    file: File,
    bytes: usize,
    ordinal: usize,
}
impl Journal {
    fn new(path: &Path) -> Check<Self> {
        Ok(Self {
            file: OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path)
                .map_err(error)?,
            bytes: 0,
            ordinal: 0,
        })
    }
    fn emit(&mut self, kind: &str, payload: Value) -> Check {
        let authoritative = matches!(
            kind,
            "declaration"
                | "stage_begin"
                | "stage_complete"
                | "stage_failed"
                | "phase"
                | "helper_ready"
                | "summary"
        );
        let mut row = payload
            .as_object()
            .cloned()
            .ok_or("journal payload must be object")?;
        if !authoritative {
            row.insert("observation_kind".into(), json!(kind));
        }
        row.insert("schema".into(), json!(1));
        row.insert("seq".into(), json!(self.ordinal));
        row.insert(
            "kind".into(),
            json!(if authoritative { kind } else { "observation" }),
        );
        let bytes = json_bytes(&row)?;
        require(
            bytes.len() <= 256 * 1024 && self.bytes + bytes.len() < 16 * 1024 * 1024,
            "bounded journal overflow",
        )?;
        self.file
            .write_all(&bytes)
            .and_then(|_| self.file.write_all(b"\n"))
            .and_then(|_| self.file.flush())
            .map_err(error)?;
        self.bytes += bytes.len() + 1;
        self.ordinal += 1;
        Ok(())
    }
}
fn fixture_project() -> Project {
    let mut p = Project {
        tempo: 120.0,
        swing: 0.0,
        song_length_beats: 4.0,
        ..Project::default()
    };
    p.channels.truncate(1);
    p.patterns.truncate(1);
    p.clips.truncate(1);
    p.plugin_instances.clear();
    p.mixer_insert_slots.clear();
    p.automation_lanes.clear();
    p.audio_assets.clear();
    p.audio_clip_mixer_destinations.clear();
    p.piano_notes.clear();
    p.active_pattern = 0;
    let channel = &mut p.channels[0];
    channel.volume = 0.75;
    channel.pan = 0.0;
    channel.muted = false;
    channel.solo = false;
    channel.steps = [false; 16];
    channel.mixer_track = 1;
    channel.instrument_plugin_instance_id = None;
    let channel_id = channel.id;
    p.patterns[0].length_steps = 4;
    p.patterns[0].channel_steps = vec![[false; 16]];
    p.patterns[0].notes = vec![PianoNote {
        id: 1,
        channel_id: Some(channel_id),
        group_id: None,
        note: 60,
        start: 0.0,
        length: 0.5,
        velocity: 100.0 / 127.0,
        selected: false,
        muted: false,
    }];
    let clip = &mut p.clips[0];
    clip.start = 1.25;
    clip.length = 1.0;
    clip.kind = ClipKind::Pattern;
    clip.muted = false;
    clip.gain = 1.0;
    clip.source_offset = 0.0;
    clip.audio_source_offset_frame = None;
    clip.audio_source_reference = None;
    clip.audio_length_reference = None;
    clip.fade_in = 0.0;
    clip.fade_out = 0.0;
    clip.fade_in_reference = None;
    clip.fade_out_reference = None;
    clip.group_id = None;
    clip.audio_asset_id = None;
    clip.automation_id = None;
    clip.pattern_id = p.patterns[0].id;
    // Keep the schema's 32 unique stable IDs/runtime slots. Only these two
    // tracks contribute to the sole route; other default tracks are disconnected.
    for track in p.mixer_tracks.iter_mut().take(2) {
        track.volume = 1.0;
        track.pan = 0.0;
        track.muted = false;
        track.solo = false;
        track.peak = 0.0;
    }
    p.mixer_routes = vec![MixerRoute {
        id: 1,
        runtime_slot: 0,
        source_mixer_track_id: 1,
        destination: MixerRouteDestination::MainInput {
            mixer_track_id: MASTER_MIXER_TRACK_ID,
        },
        tap: MixerRouteTap::PostFader,
        gain: 1.0,
        enabled: true,
    }];
    p
}
fn descriptor() -> PluginDescriptor {
    PluginDescriptor {
        id: "genuine-qa-project-identity".into(),
        name: "Surge XT".into(),
        vendor: "Surge Synth Team".into(),
        path: PLUGIN.into(),
        format: ScannedPluginFormat::Vst3,
        category: "Instrument|Synth".into(),
        is_instrument: true,
        verified: true,
        vst3_metadata: Some(plugins::Vst3ScanMetadata {
            class_uid: CLASS.into(),
            category: "Instrument|Synth".into(),
            has_midi_input: true,
            has_midi_output: false,
        }),
        scan_error: None,
    }
}
fn expected_metadata(m: &BouncePluginMetadata) -> Check {
    require(
        m.class_uid == CLASS
            && m.version == "1.3.4"
            && m.name == "Surge XT"
            && m.category == "Instrument|Synth"
            && m.event_input_buses == [(16, true, Some(true))]
            && m.event_output_buses.is_empty()
            && m.input_buses == [BounceAudioBus::new(2, false, Some(true))]
            && m.output_buses
                == [
                    BounceAudioBus::new(2, true, Some(true)),
                    BounceAudioBus::new(2, true, Some(false)),
                    BounceAudioBus::new(2, true, Some(false)),
                ]
            && m.latency_samples == 0
            && m.tail_samples == 96_000,
        "actual fixed Surge metadata differs from declared qualification",
    )
}
fn metadata_json(m: &BouncePluginMetadata) -> Value {
    json!({"class_uid":m.class_uid,"version":m.version,"name":m.name,"category":m.category,"event_inputs":m.event_input_buses,"event_outputs":m.event_output_buses,"audio_inputs":m.input_buses.iter().map(|b|json!([b.channel_count,b.is_main,b.active])).collect::<Vec<_>>(),"audio_outputs":m.output_buses.iter().map(|b|json!([b.channel_count,b.is_main,b.active])).collect::<Vec<_>>(),"latency":m.latency_samples,"tail":m.tail_samples})
}

/// All live owners are fields before the outer body is caught. No Pending guard
/// is dropped by a body unwind. Failed, unproven cleanup moves this whole owner
/// to ManuallyDrop quarantine until the external owned-group watchdog ends it.
struct GenuineFixture {
    app: Option<Box<CitrusApp>>,
    callback: Option<audio::TestAudioCallback>,
    accepting_job: Option<ClipRenderJob>,
    ctx: egui::Context,
    observer: ObservationRun,
    binding: RunBinding,
    launch: Value,
    journal: Journal,
    outer: Instant,
    pacer: Pacer,
    budget: Budget,
    stage_results: [&'static str; 10],
    active_stage: Option<usize>,
    dumped_records: usize,
    generated_path: Option<PathBuf>,
    original_wav: Option<Vec<u8>>,
    committed_project: Option<Arc<[u8]>>,
    authoritative_before: Option<Arc<[u8]>>,
    initial_project: Option<Project>,
    verified_pcm: Option<Arc<[f32]>>,
    asset_id: Option<u64>,
    first_playback: Option<Vec<f32>>,
    original_endpoint: Option<u64>,
    initial_state_identity: Option<(u64, u64, u64)>,
    original_worker: Option<usize>,
    undo_endpoint: Option<u64>,
    undo_worker: Option<usize>,
    initial_asset_operation: Option<AudioAssetOperation>,
    last_health_evidence: Option<Value>,
    topology: OwnedTopologyLedger,
    topology_records: usize,
    last_topology_health_evidence: Option<Value>,
    last_stop_settlement_evidence: Option<Value>,
}
impl GenuineFixture {
    fn new(binding: RunBinding, token: [u8; 32], entered: Instant) -> Check<Self> {
        let outer = entered + Duration::from_secs(118);
        let journal = Journal::new(&binding.run.join("events.jsonl"))?;
        let observer = ObservationRun::install().map_err(error)?;
        let ctx = egui::Context::default();
        // These constructors cannot create a plugin. Retain each returned owner
        // immediately before running any assertion or observable source load.
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let app = Some(CitrusApp::new_boxed_with_services(&cc, false));
        let mut f = Self {
            app,
            callback: None,
            accepting_job: None,
            ctx,
            observer,
            binding,
            launch: Value::Null,
            journal,
            outer,
            pacer: Pacer::default(),
            budget: Budget::default(),
            stage_results: ["unexecuted"; 10],
            active_stage: None,
            dumped_records: 0,
            generated_path: None,
            original_wav: None,
            committed_project: None,
            authoritative_before: None,
            initial_project: None,
            verified_pcm: None,
            asset_id: None,
            first_playback: None,
            original_endpoint: None,
            initial_state_identity: None,
            original_worker: None,
            undo_endpoint: None,
            undo_worker: None,
            initial_asset_operation: None,
            last_health_evidence: None,
            topology: OwnedTopologyLedger::new(token),
            topology_records: 0,
            last_topology_health_evidence: None,
            last_stop_settlement_evidence: None,
        };
        let (engine, callback) = AudioEngine::test_engine_with_callback();
        f.callback = Some(callback);
        f.app.as_mut().unwrap().audio = Some(engine);
        f.callback
            .as_mut()
            .unwrap()
            .enable_genuine_observation(token)
            .map_err(error)?;
        Ok(f)
    }
    fn app(&self) -> &CitrusApp {
        self.app.as_deref().expect("retained App")
    }
    fn app_mut(&mut self) -> &mut CitrusApp {
        self.app.as_deref_mut().expect("retained App")
    }
    fn snapshot(&self) -> GenuineCallbackSnapshot {
        self.callback
            .as_ref()
            .expect("retained callback")
            .genuine_snapshot()
    }
    fn stage(&mut self, index: usize, observer: Stage) -> Check {
        require(
            self.active_stage.is_none()
                && self.stage_results[..index]
                    .iter()
                    .all(|s| *s == "completed"),
            "declared stage order",
        )?;
        require(
            self.topology.clear(),
            "stage entry has unresolved owned topology",
        )?;
        self.active_stage = Some(index);
        self.stage_results[index] = "incomplete";
        self.observer.set_stage(observer).map_err(error)?;
        self.journal
            .emit("stage_begin", json!({"index":index,"stage":STAGES[index]}))
    }
    fn passed(&mut self) -> Check {
        if self
            .active_stage
            .is_some_and(|index| (1..=8).contains(&index))
        {
            require(
                self.healthy()?.class == HealthClass::Clear,
                "pending App health cannot complete a qualification stage",
            )?;
        }
        let index = self.active_stage.take().ok_or("stage owner missing")?;
        self.stage_results[index] = "completed";
        self.journal.emit(
            "stage_complete",
            json!({"index":index,"stage":STAGES[index]}),
        )
    }
    fn hooks(&self) -> Check {
        require(
            self.app().genuine_hooks_inactive()
                && self
                    .callback
                    .as_ref()
                    .is_some_and(|c| c.genuine_hooks_inactive())
                && self
                    .accepting_job
                    .as_ref()
                    .is_none_or(|j| j.render_starter.is_none()),
            "behavior-changing test hook active",
        )
    }
    fn healthy(&mut self) -> Check<HealthStatus> {
        self.hooks()?;
        self.observer.with_snapshot(|s| {
            require(s.failure.is_none(), s.failure.unwrap_or("observer failure"))
        })?;
        require(
            !self
                .callback
                .as_ref()
                .unwrap()
                .genuine_observation_overflowed(),
            "callback observation overflow",
        )?;
        let observed = health::observe(self.app());
        if observed.status.class == HealthClass::Failed
            || self.last_health_evidence.as_ref() != Some(&observed.evidence)
        {
            self.journal.emit("app_health", observed.evidence.clone())?;
            self.last_health_evidence = Some(observed.evidence);
        }
        require(
            observed.status.class != HealthClass::Failed,
            "actual App health failed; exact bounded operands precede this failure",
        )?;
        let app = self.app();
        if let Some(job) = &app.clip_render_job {
            require(
                job.failure.is_none()
                    && job.secondary_failure.is_none()
                    && !job.removal_outcome_unknown
                    && !job.publication_outcome_unknown,
                job.failure
                    .as_deref()
                    .unwrap_or("actual controller failure or unknown outcome"),
            )?;
        }
        if let Some(job) = &app.clip_render_job
            && matches!(
                job.phase,
                ClipRenderPhase::Capturing
                    | ClipRenderPhase::RenderingDisposing
                    | ClipRenderPhase::VerifyingMedia
                    | ClipRenderPhase::RecapturingSource
            )
        {
            let stamp = job.binding.ok_or("conversion live binding absent")?.stamp;
            let source = app
                .running_generator_chains
                .get(&stamp.channel_id)
                .ok_or("conversion exact source owner missing")?;
            let native = source
                .control
                .native_editor_snapshot(0)
                .ok_or("conversion native observation missing")?;
            require(
                native.capture_serial == stamp.native_capture_serial
                    && native.parameter_capture_serial == stamp.parameter_capture_serial
                    && native.pending_request.is_none()
                    && !native.capture_in_progress,
                "unexpected ordinary native capture/publication during conversion",
            )?;
        }
        let snapshot = self.snapshot();
        self.topology.snapshot(&snapshot)?;
        for endpoint in &snapshot.endpoints {
            let b = &endpoint.fixed.bridge.stats;
            require(
                b.reset_faults == 0
                    && b.input_overflows == 0
                    && b.input_gaps == 0
                    && b.deadline_misses == 0
                    && b.output_overflows == 0
                    && b.output_too_small == 0
                    && b.frame_mismatches == 0
                    && b.future_output_overflows == 0
                    && b.rt_overflows == 0
                    && b.inline_event_overflows == 0
                    && b.dropped_rt_events == 0
                    && b.admin_overflows == 0
                    && b.event_overflows == 0
                    && b.faults == 0
                    && b.latency_drift_blocks == 0
                    && b.latency_drift_outputs == 0
                    && b.detached_workers == 0
                    && b.shutdown_timeouts == 0,
                "actual source bridge fault/underrun/loss/detachment",
            )?;
        }
        self.observer.with_snapshot(|s| -> Check {
            let mut ordinary_states = 0usize;
            for r in &s.records { match &r.kind {
                RecordKind::Call{boundary:Boundary::Returned,outcome:Some(CallOutcome::Error(_) | CallOutcome::StrictError(_)),..} => return Err("actual host operation failed".into()),
                RecordKind::RuntimeEvent{event:RuntimeEvent::SlotFault{..}|RuntimeEvent::StateRejected{..}|RuntimeEvent::ParameterCommandFailed{..},..} => return Err("actual runtime event reports failure".into()),
                RecordKind::WorkerJoin{outcome:observe::WorkerJoinOutcome::WorkerPanicked,..}=>return Err("actual retained worker joined with panic".into()),
                RecordKind::AppAudioAssetEvent(AudioAssetEvent::Registered{success:false,..})=>return Err("actual asset registration failed".into()),
                RecordKind::AppGeneratorEndpointEvent(audio::GeneratorEndpointEvent::Installed{success:false,..}|audio::GeneratorEndpointEvent::StampedRemoved{result:Err(_),..})=>return Err("actual generator lifecycle receipt rejected".into()),
                RecordKind::OrdinaryState{endpoint_id,project_session,instance_id,slot,request_id,..} => {
                    ordinary_states += 1;
                    require(
                        r.stage == Stage::InitialSource
                            && *request_id == 0
                            && *slot == 0
                            && Some((*endpoint_id, *project_session, *instance_id)) == self.initial_state_identity
                            && ordinary_states == 1,
                        &format!("unexpected ordinary state publication at {:?}, endpoint {}, project {}, instance {}, request {}; only the exact initial source request0 once is allowed", r.stage, endpoint_id, project_session, instance_id, request_id),
                    )?;
                },
                _=>{}
            }} Ok(())
        })?;
        self.refresh_owned_topology(observed.status.class == HealthClass::Clear)?;
        Ok(observed
            .status
            .with_owned_topology_wait(!self.topology.clear()))
    }
    fn pace(&mut self, cadence: Cadence, deadline: Instant) -> Check {
        if let Some(outer) = self.launch["watchdog_deadline_monotonic_ns"].as_u64() {
            require(
                monotonic_ns()? < outer,
                "actual outer watchdog deadline elapsed",
            )?;
        }
        self.pacer.wait(cadence, deadline.min(self.outer))
    }
    /// Read only the exact production sample that the App already used. This
    /// adds bounded evidence, never a new settlement query or pump decision.
    fn record_stop_settlement(&mut self) -> Check {
        let Some(job) = self.app().clip_render_job.as_ref() else {
            return Ok(());
        };
        let Some(stop) = job.stop_preparation else {
            return Ok(());
        };
        let identity = |target: BounceSourceCallbackIdentity| {
            json!({
                "engine": target.engine_session, "project": target.project_session,
                "generation": target.timeline_generation, "epoch": target.epoch,
                "legacy_request": target.transport_request, "channel": target.channel_id,
                "endpoint": target.endpoint_id, "instance": target.instance_id,
            })
        };
        let observed = job.stop_settlement.map(|sample| {
            json!({
                "result": format!("{:?}", sample.settlement),
                "coherent": sample.coherent, "requested_epoch": sample.requested_epoch,
                "callback": {
                    "identity": sample.callback_identity.map(identity),
                    "ready": sample.callback_ready, "submitted": sample.submitted,
                },
                "worker": {
                    "reset_completed_epoch": sample.reset_completed_epoch,
                    "consumed": sample.consumed, "terminal": sample.terminal,
                    "busy": sample.busy, "valid": sample.worker_valid,
                },
                "exhausted": sample.exhausted,
                "parameter_admission": sample.parameter_admission,
                "capture_busy": sample.capture_busy,
            })
        });
        let evidence = json!({
            "operation": job.operation_id,
            "phase": format!("{:?}", job.phase),
            "intent": {
                "generation": stop.generation, "seek_serial": stop.seek_serial,
                "frame": stop.frame, "legacy_request": stop.transport_request,
                "superseded": stop.superseded,
                "activation": stop.activation.map(|activation| json!({
                    "timeline_request": activation.request_id,
                    "minimum_epoch": activation.minimum_epoch,
                    "committed_epoch": activation.committed_epoch,
                })),
            },
            "target": job.settlement_target.map(identity),
            "last_admission_sample": observed,
            "failure": job.failure.as_deref().map(bounded_clip_timeline_text),
        });
        if self.last_stop_settlement_evidence.as_ref() != Some(&evidence) {
            self.journal
                .emit("capture_stop_settlement", evidence.clone())?;
            self.last_stop_settlement_evidence = Some(evidence);
        }
        Ok(())
    }

    fn poll(&mut self, reason: &str) -> Check<HealthStatus> {
        self.pace(Cadence::Poll, self.outer)?;
        self.budget.poll()?;
        let ctx = self.ctx.clone();
        let app = self.app_mut();
        app.drive_clip_session_retirement();
        app.poll_clip_restorations();
        app.poll_generator_endpoint_events();
        app.poll_plugin_runtime_events();
        app.poll_plugin_parameter_worker_receipts();
        app.poll_plugin_processing_health();
        app.poll_timeline_compile(&ctx);
        app.poll_audio_asset_events();
        app.poll_timeline_audio_runtime(&ctx);
        app.drive_clip_render_job();
        self.journal.emit("controller_poll",json!({"ordinal":self.budget.polls,"reason":reason,"stage":self.active_stage,"phase":format!("{:?}",self.app().clip_render_job.as_ref().map(|j|j.phase)),"desired_generation":self.app().timeline_desired_generation,"confirmed":self.app().audio.as_ref().and_then(AudioEngine::confirmed_timeline_identity)}))?;
        self.record_stop_settlement()?;
        self.persist_new_observations()?;
        self.consume_owned_topology_poll()?;
        self.healthy()
    }
    fn pump(
        &mut self,
        cadence: Cadence,
        purpose: PumpPurpose,
    ) -> Check<(GenuineSegmentObservation, Vec<f32>)> {
        require(
            self.budget.total < MAX_PUMPS,
            "total pump ceiling before callback",
        )?;
        self.pace(cadence, self.outer)?;
        let role = self.prepare_owned_topology_pump(purpose)?;
        let app = self.app.as_ref().unwrap();
        let callback = self.callback.as_mut().unwrap();
        callback.pump(app.audio.as_ref().unwrap(), 128);
        let segments = callback.take_genuine_segments();
        let receipts = callback.take_genuine_lifecycle_receipts();
        // Every actual invocation consumes exactly one budget charge even if
        // the immutable evidence is subsequently rejected.
        self.budget.charge(purpose.charge(segments.first()))?;
        let output = callback
            .last_output()
            .iter()
            .flat_map(|x| x.iter().copied())
            .collect::<Vec<_>>();
        // Persist actual raw segment stamps before interpreting acceptance.
        for s in &segments {
            self.journal.emit("callback_segment",json!({"pump":s.pump_ordinal,"segment":s.segment_ordinal,"stage":self.active_stage,"start":s.start_frame,"frames":s.frames,"device_offset":s.device_offset,"device_frame":s.device_frame,"playing":s.playing,"timeline_ready":s.timeline_ready,"generation":s.generation,"active_epoch":s.active_epoch,"epoch":s.epoch,"request_id":s.request_id,"loop_count":s.loop_count,"loop_enabled":s.loop_enabled,"loop_start":s.loop_start_frame,"loop_end":s.loop_end_frame,"disposition":format!("{:?}",s.disposition),"before":topology::segment_state_evidence(s.before),"after":topology::segment_state_evidence(s.after)}))?;
        }
        for receipt in &receipts {
            self.journal.emit(
                "callback_lifecycle_receipt",
                topology::receipt_evidence(*receipt),
            )?;
        }
        self.journal.emit("callback_attempt_accounting", json!({"total":self.budget.total,"controls":self.budget.controls,"playing":self.budget.playing,"stopped":self.budget.stopped,"role":format!("{role:?}")}))?;
        require(
            segments.len() == 1,
            "pump did not produce exactly one accounted Q128 segment",
        )?;
        let segment = segments[0];
        require(
            segment.frames == 128 && segment.device_offset == 0 && output.len() == 256,
            "Q128 output extent mismatch",
        )?;
        self.topology
            .callback_receipts(&receipts, segment.pump_ordinal, self.budget.polls + 1)?;
        if !receipts.is_empty() {
            self.journal
                .emit("owned_topology_callback", self.topology.evidence())?;
        }
        self.topology.segment(&segment, &output, role)?;
        self.topology.snapshot(&self.snapshot())?;
        Ok((segment, output))
    }
    fn control_tick(&mut self, control: usize, reason: &str) -> Check {
        self.poll(reason)?;
        let snapshot = self.snapshot();
        if snapshot.callback_work_pending()
            || self
                .app()
                .audio
                .as_ref()
                .is_some_and(|a| snapshot.request_id != a.test_transport_request_id())
        {
            require(
                self.budget.controls[control] < CONTROL_CEILINGS[control],
                "control stage pump ceiling before callback",
            )?;
            let (segment, _) = self.pump(Cadence::Control, PumpPurpose::Control(control))?;
            require(
                !segment.playing,
                "unaccounted musical output during control wait",
            )?;
        }
        Ok(())
    }
    fn until(
        &mut self,
        control: usize,
        seconds: u64,
        reason: &str,
        condition: impl Fn(&Self) -> bool,
    ) -> Check {
        let deadline = (Instant::now() + Duration::from_secs(seconds)).min(self.outer);
        loop {
            let health = self.healthy()?;
            if health.admits_condition(condition(self)) {
                return Ok(());
            }
            require(
                Instant::now() < deadline,
                &format!("bounded wait failed: {reason}"),
            )?;
            self.control_tick(control, reason)?;
        }
    }
    fn exact_paused(&self) -> Option<(u64, u64)> {
        let app = self.app();
        let pair = app.audio.as_ref()?.confirmed_timeline_identity()?;
        let s = self.snapshot();
        (app.timeline_audio_matches_desired()
            && pair.0 == app.timeline_desired_generation
            && s.generation == Some(pair.0)
            && s.active_epoch == Some(pair.1)
            && s.epoch == pair.1
            && !s.playing
            && s.start_frame == 0
            && !app.playing)
            .then_some(pair)
    }
    fn permit(&mut self, phase: &str) -> Check {
        let phases = ["initial_source", "success_clone", "undo_source"];
        let ordinal = phases
            .iter()
            .position(|p| *p == phase)
            .ok_or("launch phase")?;
        let permit = json!({"schema":1,"phase":phase,"ordinal":ordinal,"owner_pid":self.launch["owner_pid"],"owner_pgid":self.launch["owner_pgid"],"expected_helper_path":self.launch["real_helper"]["path"],"expected_helper_sha256":self.launch["real_helper"]["sha256"]});
        let destination = self.binding.run.join(format!("launch-permit-{phase}.json"));
        let temporary = self
            .binding
            .run
            .join(format!(".permit-{phase}-{}.tmp", std::process::id()));
        private_write(&temporary, &json_bytes(&permit)?)?;
        fs::hard_link(&temporary, &destination).map_err(error)?;
        fs::remove_file(&temporary).map_err(error)?;
        self.journal
            .emit("phase", phase_event_payload(&self.binding.run, phase)?)
    }
}
impl GenuineFixture {
    fn persist_new_observations(&mut self) -> Check {
        let records = self.observer.copy_records_since(self.dumped_records);
        let private = self.binding.run.join("private");
        let journal = &mut self.journal;
        for record in &records {
            let mut private_blob: Option<std::borrow::Cow<'_, [u8]>> = None;
            let value = match &record.kind {
                RecordKind::Call {
                    operation,
                    boundary,
                    frames,
                    transport,
                    outcome,
                } => {
                    json!({"type":"call","operation":format!("{operation:?}"),"boundary":format!("{boundary:?}"),"frames":frames,"transport":transport.map(|t|format!("{t:?}")),"outcome":format!("{outcome:?}")})
                }
                RecordKind::StateLoad { bytes, outcome } => {
                    private_blob = Some(std::borrow::Cow::Borrowed(bytes));
                    json!({"type":"state_load","bytes":bytes.len(),"sha256":oracles::sha256_hex(bytes),"outcome":format!("{outcome:?}")})
                }
                RecordKind::BaseRead { site, id, raw_bits } => {
                    json!({"type":"base_read","site":site,"id":id,"raw_f64_bits":format!("{raw_bits:?}")})
                }
                RecordKind::Metadata { site, result } => {
                    json!({"type":"metadata","site":site,"metadata":result.as_ref().ok().map(metadata_json),"error":result.as_ref().err()})
                }
                RecordKind::AudioDeclaration {
                    input_buses,
                    output_buses,
                } => {
                    json!({"type":"audio_declaration","inputs":format!("{input_buses:?}"),"outputs":format!("{output_buses:?}")})
                }
                RecordKind::EventDeclaration { inputs, outputs } => {
                    json!({"type":"event_declaration","inputs":inputs,"outputs":outputs})
                }
                RecordKind::StrictPcm {
                    frames,
                    transport,
                    channels,
                    transport_applied,
                    output_events,
                    output_events_lost,
                } => {
                    let bytes = channels
                        .iter()
                        .flatten()
                        .flat_map(|x| x.to_bits().to_le_bytes())
                        .collect::<Vec<_>>();
                    let sha = oracles::sha256_hex(&bytes);
                    private_blob = Some(std::borrow::Cow::Owned(bytes));
                    json!({"type":"strict_pcm","frames":frames,"transport":format!("{transport:?}"),"channel_lengths":channels.iter().map(Vec::len).collect::<Vec<_>>(),"planar_ieee_sha256":sha,"transport_applied":transport_applied,"output_events":output_events,"output_events_lost":output_events_lost})
                }
                RecordKind::Midi { data, offset } => {
                    json!({"type":"midi","data":data,"offset":offset})
                }
                RecordKind::Capture {
                    label,
                    identity,
                    receipt_identity,
                    state,
                } => {
                    private_blob = Some(std::borrow::Cow::Borrowed(&state.opaque_state));
                    json!({"type":"capture","label":format!("{label:?}"),"identity":format!("{identity:?}"),"receipt_identity":receipt_identity,"metadata":metadata_json(&state.metadata),"state_bytes":state.opaque_state.len(),"state_sha256":oracles::sha256_hex(&state.opaque_state),"base_bits":state.parameter_bases.iter().map(|(id,v)|(*id,v.to_bits())).collect::<Vec<_>>(),"native_generation":state.native_generation,"native_dirty_revision":state.native_dirty_revision})
                }
                RecordKind::RuntimeEvent {
                    endpoint_id,
                    project_session,
                    instance_id,
                    event,
                } => {
                    json!({"type":"runtime_event","endpoint":endpoint_id,"session":project_session,"instance":instance_id,"event":format!("{event:?}")})
                }
                RecordKind::AppAudioAssetEvent(event) => {
                    json!({"type":"app_asset_event","event":format!("{event:?}")})
                }
                RecordKind::AppGeneratorEndpointEvent(event) => {
                    json!({"type":"app_generator_event","event":topology::lifecycle_event_evidence(*event)})
                }
                RecordKind::AppTimelineRuntimeEvent {
                    event,
                    before,
                    after,
                } => {
                    json!({"type":"app_timeline_runtime_event","event":topology::timeline_event_evidence(*event),"before":topology::timeline_state_evidence(*before),"after":topology::timeline_state_evidence(*after)})
                }
                RecordKind::WorkerJoin { worker_id, outcome } => {
                    json!({"type":"worker_join","worker_id":worker_id,"outcome":format!("{outcome:?}")})
                }
                RecordKind::OrdinaryState {
                    endpoint_id,
                    project_session,
                    instance_id,
                    slot,
                    request_id,
                    epoch,
                    byte_len,
                } => {
                    json!({"type":"ordinary_state","endpoint":endpoint_id,"session":project_session,"instance":instance_id,"slot":slot,"request":request_id,"epoch":epoch,"bytes":byte_len})
                }
                RecordKind::ResetReport {
                    frames,
                    discarded_prior_midi_events,
                    discarded_midi_events,
                    discarded_prior_parameter_points,
                    discarded_parameter_points,
                    output_events_lost,
                    parameter_output_fault,
                    process_error,
                } => {
                    json!({"type":"reset_report","frames":frames,"discarded_prior_midi_events":discarded_prior_midi_events,"discarded_midi_events":discarded_midi_events,"discarded_prior_parameter_points":discarded_prior_parameter_points,"discarded_parameter_points":discarded_parameter_points,"output_events_lost":output_events_lost,"parameter_output_fault":parameter_output_fault,"process_error":process_error})
                }
                RecordKind::StrictCaptureIdentity(identity) => {
                    json!({"type":"strict_capture_identity","identity":format!("{identity:?}")})
                }
                RecordKind::RenderReport(report) => {
                    json!({"type":"render_report","report":format!("{report:?}")})
                }
                RecordKind::Disposal {
                    terminated,
                    error,
                    diagnostics,
                } => {
                    json!({"type":"disposal","terminated":terminated,"error":format!("{error:?}"),"diagnostics":diagnostics})
                }
                RecordKind::Worker {
                    site,
                    worker_id,
                    manifest,
                    epoch,
                    sequence,
                    stats,
                } => {
                    json!({"type":"worker","site":site,"worker":worker_id,"manifest":format!("{manifest:?}"),"epoch":epoch,"sequence":sequence,"stats":format!("{stats:?}")})
                }
                RecordKind::WorkerCaptureGuard {
                    worker_id,
                    operands,
                } => {
                    json!({
                        "type": "worker_capture_guard", "worker": worker_id,
                        "expected_epoch": operands.expected_epoch,
                        "active_epoch": operands.active_epoch,
                        "requested_epoch": operands.requested_epoch,
                        "input_empty": operands.input_empty,
                        "parameter_admission": operands.parameter_admission,
                        "accepted": operands.accepted,
                    })
                }
                RecordKind::HookInventory { site, inactive } => {
                    json!({"type":"test_hooks","site":site,"inactive":inactive})
                }
                RecordKind::Lifecycle(site) => json!({"type":"lifecycle","site":site}),
            };
            if let Some(bytes) = private_blob {
                private_write(
                    &private.join(format!("observer-{:05}.bin", record.ordinal)),
                    &bytes,
                )?;
            }
            journal.emit("raw_owner_observation",json!({"ordinal":record.ordinal,"stage":format!("{:?}",record.stage),"instance":format!("{:?}",record.instance),"observation":value}))?;
        }
        self.dumped_records += records.len();
        Ok(())
    }
    fn preflight(&mut self) -> Check {
        self.journal.emit("declaration",json!({"stages":STAGES,"selector":SELECTOR,"binding_sha256":self.binding.binding_hash,"ui_boundary":"headless real review plus exact shared begin_clip_render_stop acceptance method; no pointer/native-window claim","control_ceilings":CONTROL_CEILINGS,"playing_segments":PLAY_SEGMENTS,"stopped_segments":[32,32,32],"cleanup_reserve":CLEANUP_PUMPS,"outer_seconds":120,"original_imported_oracle_sha256":"1bff180934231ca40cf92c079038f45b4dddfdd2ad29a6df9216174b12a0d8b2","adapted_oracle_source_sha256":"89ca0f7eb4defed60eac65b7c8d0330a9990a3a3deb4b1792339b030cea4550b","oracle_adaptation":"removed standalone test-only Gate module registration; mechanical Clippy style adaptations only; numerical constants, operation order, type, finite and padding checks unchanged"}))?;
        self.stage(0, Stage::Preflight)?;
        self.hooks()?;
        let descriptor_pin = self.binding.raw["harness_sources"]
            .as_array()
            .ok_or("harness source inventory missing")?
            .iter()
            .find(|pin| {
                pin["path"]
                    .as_str()
                    .is_some_and(|s| s.ends_with("/fixture-descriptor.json"))
            })
            .ok_or("pinned descriptor file absent")?;
        let descriptor_path = Path::new(
            descriptor_pin["path"]
                .as_str()
                .ok_or("descriptor pin path")?,
        );
        require(
            fs::metadata(descriptor_path).map_err(error)?.len() <= 16 * 1024,
            "descriptor file bound",
        )?;
        let pinned: PluginDescriptor =
            serde_json::from_slice(&fs::read(descriptor_path).map_err(error)?).map_err(error)?;
        require(
            json_bytes(&pinned)? == json_bytes(&descriptor())?,
            "pinned descriptor differs from declared initialized fixture identity",
        )?;
        let app = self.app();
        require(
            app.autosave_path.is_none()
                && app.plugin_cache_path.is_none()
                && app.recordings_dir.is_none()
                && app.saved_path.is_none()
                && app.audio_device_catalog_pending_generation.is_none()
                && app.scan_receiver.is_none()
                && !app.recovery_available,
            "external profile/device/cache services unexpectedly present",
        )?;
        let path = self.binding.run.join("launch-config.json");
        let deadline = Instant::now() + Duration::from_secs(2);
        while !path.exists() {
            require(
                Instant::now() < deadline,
                "outer launcher configuration unavailable",
            )?;
            std::thread::sleep(Duration::from_millis(10));
        }
        self.launch = serde_json::from_slice(&fs::read(&path).map_err(error)?).map_err(error)?;
        let proc = fs::read_to_string("/proc/self/stat").map_err(error)?;
        let fields = proc
            .rsplit_once(") ")
            .ok_or("proc stat")?
            .1
            .split_whitespace()
            .collect::<Vec<_>>();
        let pgid = fields
            .get(2)
            .ok_or("proc pgid")?
            .parse::<u64>()
            .map_err(error)?;
        let ticks = fields
            .get(19)
            .ok_or("proc ticks")?
            .parse::<u64>()
            .map_err(error)?;
        require(
            self.launch["schema"] == 1
                && self.launch["owner_pid"] == std::process::id()
                && self.launch["owner_pgid"] == pgid
                && pgid == u64::from(std::process::id())
                && self.launch["owner_start_ticks"] == ticks
                && self.launch["run_dir"] == self.binding.run.to_string_lossy().as_ref()
                && self.launch["phases"]
                    == json!(["initial_source", "success_clone", "undo_source"]),
            "launcher owner/start-ticks/group/phase binding",
        )?;
        require(
            self.launch["shim"]["path"]
                == self
                    .binding
                    .run
                    .join("bin/vst3-host-helper")
                    .to_string_lossy()
                    .as_ref()
                && self.launch["real_helper"]["path"]
                    == self
                        .binding
                        .run
                        .join("private/vst3-host-helper.real")
                        .to_string_lossy()
                        .as_ref()
                && self.launch["real_helper"]["sha256"]
                    == self.binding.raw["ordinary_helper"]["sha256"],
            "launcher executable binding",
        )?;
        let watchdog = self.launch["watchdog_deadline_monotonic_ns"]
            .as_u64()
            .ok_or("actual watchdog deadline missing")?;
        let remaining = watchdog
            .checked_sub(monotonic_ns()?)
            .ok_or("actual watchdog deadline elapsed during preflight")?;
        self.outer = self
            .outer
            .min(Instant::now() + Duration::from_nanos(remaining));
        private_write(
            &self.binding.output_directory.join("sibling-sentinel.txt"),
            SENTINEL,
        )?;
        self.passed()
    }
    fn setup_source(&mut self) -> Check {
        self.stage(1, Stage::InitialSource)?;
        let app = self.app_mut();
        app.project = fixture_project();
        app.transport_mode = TransportMode::Song;
        app.selected_clip = Some(app.project.clips[0].id);
        app.playlist_selection_ids = HashSet::from([app.project.clips[0].id]);
        require(
            app.audio
                .as_mut()
                .unwrap()
                .set_project_session(app.project_session),
            "initial actual project-session command refused",
        )?;
        app.rebuild_tempo_map();
        app.request_immediate_timeline_compile();
        self.setup_mixer_controls()?;
        // Picker opt-in is the existing production admission. No manually built
        // RunningGeneratorChain, mock source or injected readiness is involved.
        self.app_mut().plugin_picker_fresh_bounce = true;
        self.permit("initial_source")?;
        require(
            self.app_mut().load_plugin_into_target(
                &descriptor(),
                PluginPickerTarget::ChannelDevice { channel: 0 },
            ),
            "actual fresh-source load refused",
        )?;
        // Bind the only allowed ordinary startup State to the actual owner before
        // any App event poll. Undo's dedicated restoration path never requests it.
        let channel = self.app().project.channels[0].id;
        self.initial_state_identity = Some(
            self.app()
                .pending_generator_chains
                .get(&channel)
                .map(|source| {
                    (
                        source.endpoint_id,
                        source.project_session,
                        source.instance_id,
                    )
                })
                .or_else(|| {
                    self.app()
                        .running_generator_chains
                        .get(&channel)
                        .map(|source| {
                            (
                                source.endpoint_id,
                                source.project_session,
                                source.instance_id,
                            )
                        })
                })
                .ok_or("initial source owner missing before runtime event poll")?,
        );
        self.wait_initial_source_ready()?;
        self.require_mixer_readback("initial_source_ready")?;
        let s = self.snapshot();
        require(
            s.endpoints.len() == 1
                && s.timing.sample_rate == 48_000
                && s.timing.callback_budget_frames == 2048
                && s.timing.guard_quanta == 2
                && s.timing.lookahead_quanta == 18
                && s.timing.bridge_latency_frames == 2432,
            "actual conservative source timing mismatch",
        )?;
        require(
            s.loop_enabled && s.loop_start_frame == 0 && s.loop_end_frame == 96_000,
            "actual Song loop differs",
        )?;
        self.require_source_owner_binding(&s, InstancePhase::InitialSource)?;
        self.original_endpoint = Some(s.endpoints[0].endpoint_id);
        self.original_worker = Some(s.endpoints[0].fixed.bridge.worker_id);
        require(
            s.endpoints[0].plugin_instance_id.is_some()
                && self.initial_state_identity
                    == Some((
                        s.endpoints[0].endpoint_id,
                        s.endpoints[0].project_session,
                        s.endpoints[0].plugin_instance_id.unwrap(),
                    )),
            "initial startup State owner differs from actual Installed endpoint",
        )?;
        self.healthy()?;
        require(
            self.observer.with_snapshot(|snapshot| {
                snapshot
                    .records
                    .iter()
                    .filter(|r| matches!(&r.kind, RecordKind::OrdinaryState { .. }))
                    .count()
            }) == 1,
            "exact initial request0 State missing after startup readiness",
        )?;
        // Setup-only intentionally stale Project; no later direct model mutation.
        let app = self.app_mut();
        let instance = app.project.channels[0]
            .instrument_plugin_instance_id
            .ok_or("App did not commit source")?;
        let plugin = app
            .project
            .plugin_instances
            .iter_mut()
            .find(|p| p.id == instance)
            .ok_or("source model missing")?;
        plugin.parameters.insert(GLOBAL_VOLUME, 0.25);
        plugin.opaque_state.clear();
        app.history_snapshot = app.project.clone();
        app.history_fingerprint = project_fingerprint(&app.project);
        app.project_fingerprint = app.history_fingerprint;
        app.undo_stack.clear();
        app.redo_stack.clear();
        app.dirty = false;
        app.refresh_timeline_fingerprint(Instant::now(), true);
        app.request_immediate_timeline_compile();
        self.until(0, 15, "stale model baseline Timeline", |f| {
            f.exact_paused().is_some()
                && f.app()
                    .clip_render_observe(f.app().project.channels[0].id)
                    .is_ok()
        })?;
        self.initial_project = Some(self.app().project.clone());
        self.journal.emit(
            "helper_ready",
            json!({"phase":"initial_source","evidence":"slot_ready_and_installed"}),
        )?;
        self.passed()
    }
    fn require_initial_mixer_source_absent(&self) -> Check {
        let app = self.app();
        require(
            app.project.plugin_instances.is_empty()
                && app.running_generator_chains.is_empty()
                && app.pending_generator_chains.is_empty()
                && app.pending_generator_removals.is_empty()
                && app.pending_generator_routes.is_empty()
                && app.deferred_generator_rebuilds.is_empty()
                && app.running_insert_chains.is_empty()
                && app.pending_insert_chains.is_empty()
                && app.pending_insert_removals.is_empty()
                && app.deferred_insert_rebuilds.is_empty()
                && app.pending_clip_restorations.is_empty()
                && app.retired_clip_restorations.is_empty()
                && self.snapshot().endpoints.is_empty()
                && self.observer.with_snapshot(|s| {
                    s.source_positive_attempts == 0
                        && s.clone_replies == 0
                        && s.records.iter().all(|r| {
                            r.instance.is_none()
                                && !matches!(
                                    &r.kind,
                                    RecordKind::Call { .. }
                                        | RecordKind::Worker { .. }
                                        | RecordKind::StateLoad { .. }
                                        | RecordKind::OrdinaryState { .. }
                                        | RecordKind::Lifecycle("instance construction observed")
                                )
                        })
                }),
            "initial mixer setup observed a source owner/endpoint/plugin operation",
        )
    }
    fn drain_initial_mixer_commands(&mut self, before_batch: usize) -> Check {
        self.require_initial_mixer_source_absent()?;
        let pending = self.snapshot().pending_audio_commands;
        self.journal.emit(
            "initial_mixer_drain",
            json!({"before_batch":before_batch,
            "pending_audio_commands":pending,"controls_used":self.budget.controls[0],
            "command_queue_full":self.app().audio.as_ref().unwrap().snapshot().command_queue_full}),
        )?;
        while self.snapshot().pending_audio_commands != 0 {
            // All callbacks use the existing pending-work decision, pacer and
            // nontransferable control0/total/outer budgets. No hidden warmup.
            self.control_tick(0, "initial full-schema mixer command drain")?;
            self.require_initial_mixer_source_absent()?;
        }
        require(
            self.app()
                .audio
                .as_ref()
                .unwrap()
                .snapshot()
                .command_queue_full
                == 0,
            "initial mixer setup audio command queue overflowed",
        )
    }
    fn setup_mixer_controls(&mut self) -> Check {
        self.require_initial_mixer_source_absent()?;
        self.record_mixer_evidence("initial_setup_before")?;
        require(
            self.app()
                .audio
                .as_ref()
                .unwrap()
                .snapshot()
                .command_queue_full
                == 0,
            "initial mixer setup began with a command queue overflow",
        )?;
        let commands = mixer_control_commands(&self.app().project);
        require(
            commands.len() == 130,
            "initial full-schema mixer command count differs",
        )?;
        let mut admitted = 0;
        let mut batches = 0;
        for (index, batch) in review::mixer_command_batches(commands).enumerate() {
            self.drain_initial_mixer_commands(index)?;
            let count = batch.len();
            let result = review::admit_mixer_batch(batch, |command| {
                self.app_mut().audio.as_mut().unwrap().command(command)
            });
            self.journal.emit("initial_mixer_batch", json!({"batch":index,
                "command_count":count,"admitted":result.is_ok(),"error":result.as_ref().err(),
                "pending_audio_commands":self.snapshot().pending_audio_commands,
                "command_queue_full":self.app().audio.as_ref().unwrap().snapshot().command_queue_full}))?;
            result?;
            admitted += count;
            batches += 1;
        }
        self.drain_initial_mixer_commands(batches)?;
        let snapshot = self.snapshot();
        self.record_mixer_snapshot("initial_setup_after", &snapshot)?;
        require(
            admitted == 130 && batches == 5,
            "initial mixer batch extent differs",
        )?;
        review::require_mixer_controls(&self.app().project, &snapshot)
    }
    fn record_mixer_evidence(&mut self, boundary: &str) -> Check {
        let snapshot = self.snapshot();
        self.record_mixer_snapshot(boundary, &snapshot)
    }
    fn record_mixer_snapshot(
        &mut self,
        boundary: &str,
        snapshot: &GenuineCallbackSnapshot,
    ) -> Check {
        self.journal.emit("mixer_readback", json!({"boundary":boundary,
            "command_queue_full":self.app().audio.as_ref().unwrap().snapshot().command_queue_full,
            "dispatch_claim":"initial batches require individual admission; later repeated App dispatch admission is not inferred from readback",
            "actual":review::mixer_evidence(snapshot)}))
    }
    fn require_mixer_readback(&mut self, boundary: &str) -> Check {
        let snapshot = self.snapshot();
        self.record_mixer_snapshot(boundary, &snapshot)?;
        review::require_fixture_graph(&snapshot.nodes, &snapshot.routes)?;
        review::require_mixer_controls(&self.app().project, &snapshot)
    }
    fn wait_initial_source_ready(&mut self) -> Check {
        let deadline = (Instant::now() + Duration::from_secs(30)).min(self.outer);
        let mut previous_evidence = None;
        loop {
            let health = self.healthy()?;
            let (ready, evidence, complete) = self.initial_source_readiness()?;
            if previous_evidence.as_ref() != Some(&evidence) {
                self.journal
                    .emit("initial_source_readiness", evidence.clone())?;
                previous_evidence = Some(evidence);
            }
            require(
                complete,
                "initial source readiness diagnostic inventory exceeded its bound",
            )?;
            if health.admits_condition(ready) {
                return Ok(());
            }
            require(
                Instant::now() < deadline,
                "bounded wait failed: source SlotReady/install/current Timeline",
            )?;
            // The original control path, cadence and source-load ceiling remain
            // authoritative. Observational pending bits never grant a pump.
            self.control_tick(0, "source SlotReady/install/current Timeline")?;
        }
    }
    fn initial_source_readiness(&self) -> Check<(bool, Value, bool)> {
        let app = self.app();
        let (endpoint, session, instance) = self
            .initial_state_identity
            .ok_or("initial startup receipt identity missing")?;
        let channel = app
            .project
            .channels
            .first()
            .ok_or("initial channel missing")?;
        let mixer = app
            .project
            .mixer_runtime_slot(channel.mixer_track)
            .ok_or("initial source mixer route missing")?;
        let expected = startup::StartupIdentity {
            endpoint,
            session,
            instance,
            channel: channel.id,
            mixer,
        };
        let receipts = self.observer.with_snapshot(|s| {
            let mut counts = startup::ReceiptCounts::default();
            for record in &s.records {
                counts.observe(record, expected);
            }
            counts
        });
        // Sample once per operand. The production live-binding check retains its
        // own coherent endpoint/native barrier; never call it again for logging.
        let source = app.running_generator_chains.get(&channel.id);
        let native = source.and_then(|source| source.control.native_editor_snapshot(0));
        let live = app.clip_render_observe(channel.id);
        let snapshot = self.snapshot();
        let audio = app.audio.as_ref().ok_or("initial source audio missing")?;
        let confirmed = audio.confirmed_timeline_identity();
        let matches_desired = app.timeline_audio_matches_desired();
        let exact_paused = confirmed.is_some_and(|pair| {
            matches_desired
                && pair.0 == app.timeline_desired_generation
                && snapshot.generation == Some(pair.0)
                && snapshot.active_epoch == Some(pair.1)
                && snapshot.epoch == pair.1
                && !snapshot.playing
                && snapshot.start_frame == 0
                && !app.playing
        });
        let running_owner_exact = source.is_some_and(|source| {
            source.endpoint_id == endpoint
                && source.project_session == session
                && source.instance_id == instance
                && source.mixer_track == mixer
        }) && app.running_generator_chains.len() == 1
            && app.project_session == session
            && audio.project_session() == session
            && channel.instrument_plugin_instance_id == Some(instance);
        let complete = startup::diagnostic_inventory_complete(&snapshot)
            && app.running_generator_chains.len() <= 16;
        let operands = startup::ReadyOperands {
            exact_paused,
            running_owner_exact,
            coherent_live_binding: live.is_ok(),
            diagnostics_complete: complete,
        };
        let source_ready_without_topology = startup::ready(operands, receipts, native.as_ref());
        let ready = self.topology.clear() && source_ready_without_topology;
        let live_evidence = match live {
            Ok(binding) => json!({"ok":true,"stamp":format!("{:?}",binding.stamp),
                "sample_rate":binding.sample_rate,"control_revision":binding.control_revision,
                "asset_generation":binding.asset_generation,"engine_session":binding.stamp.engine_session,
                "endpoint_manifest":format!("{:?}",binding.endpoint.manifest())}),
            Err(reason) => json!({"ok":false,"error":startup::text_evidence(&reason)}),
        };
        let evidence = json!({"ready":ready,"source_ready_without_topology":source_ready_without_topology,"owned_topology_settled":self.topology.clear(),"operands":operands,"receipts":receipts,
            "expected":{"endpoint":endpoint,"session":session,"instance":instance,"channel":channel.id,"mixer_track":mixer},
            "engine_session":audio.engine_session(),"audio_project_session":audio.project_session(),
            "app_project_session":app.project_session,"project_instance":channel.instrument_plugin_instance_id,
            "running_count":app.running_generator_chains.len(),
            "running":source.map(|s|json!({"endpoint":s.endpoint_id,"session":s.project_session,
                "instance":s.instance_id,"mixer_track":s.mixer_track})),
            "native":startup::native_evidence(native.as_ref()),"live_binding":live_evidence,
            "timeline":{"matches_desired":matches_desired,"confirmed":confirmed,
                "desired_generation":app.timeline_desired_generation,"compiled_generation":app.timeline_compiled_generation,
                "compiled_fingerprint":app.timeline_compiled_fingerprint,"observed_fingerprint":app.timeline_observed_fingerprint,
                "source":app.timeline_audio_source.as_ref().map(|s|json!({"generation":s.generation,
                    "fingerprint":s.fingerprint,"sample_rate":s.sample_rate})),
                "app_playing":app.playing,"callback_generation":snapshot.generation,
                "callback_active_epoch":snapshot.active_epoch,"callback_epoch":snapshot.epoch,
                "callback_playing":snapshot.playing,"callback_frame":snapshot.start_frame},
            "callback_work":startup::callback_evidence(&snapshot,audio.test_transport_request_id())});
        Ok((ready, evidence, complete))
    }
    fn require_source_owner_binding(
        &self,
        s: &GenuineCallbackSnapshot,
        phase: InstancePhase,
    ) -> Check {
        require(s.endpoints.len() == 1, "one exact source endpoint required")?;
        let endpoint = &s.endpoints[0];
        let identities = self.observer.with_snapshot(|snapshot| {
            snapshot
                .records
                .iter()
                .filter_map(|r| r.instance)
                .filter(|i| i.phase == phase)
                .collect::<Vec<_>>()
        });
        require(
            !identities.is_empty()
                && identities.iter().all(|i| {
                    i.worker_id == Some(endpoint.fixed.bridge.worker_id)
                        && i.source_instance_id == endpoint.plugin_instance_id
                }),
            "callback endpoint does not bind exact observed owner worker",
        )?;
        let (ready,installed)=self.observer.with_snapshot(|snapshot|{
            let ready=snapshot.records.iter().filter(|r|matches!(&r.kind,RecordKind::RuntimeEvent{project_session,instance_id,event:RuntimeEvent::SlotReady{slot:0,..},..} if *project_session==endpoint.project_session && Some(*instance_id)==endpoint.plugin_instance_id)).count();
            let installed=snapshot.records.iter().filter(|r|matches!(&r.kind,RecordKind::AppGeneratorEndpointEvent(audio::GeneratorEndpointEvent::Installed{endpoint_id,plugin_instance_id,mixer_track:1,replaced_endpoint_id:None,replaced_plugin_instance_id:None,success:true,..}) if *endpoint_id==endpoint.endpoint_id && Some(*plugin_instance_id)==endpoint.plugin_instance_id)).count();
            (ready,installed)
        });
        // The two source lifetimes intentionally retain the same Project instance.
        require(
            ready
                == if phase == InstancePhase::InitialSource {
                    1
                } else {
                    2
                }
                && installed == 1,
            "actual SlotReady and exact new Installed receipts missing or duplicated",
        )?;
        let metadata = self.observer.with_snapshot(|snapshot| {
            snapshot
                .records
                .iter()
                .filter(|r| r.instance.is_some_and(|i| i.phase == phase))
                .filter_map(|r| match &r.kind {
                    RecordKind::Metadata {
                        result: Ok(metadata),
                        ..
                    } => Some(metadata.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        });
        require(
            !metadata.is_empty(),
            "actual source full metadata observation missing",
        )?;
        expected_metadata(metadata.last().ok_or("ready source metadata absent")?)?;
        Ok(())
    }
    fn observed_event(&self, request: u64, accept: impl Fn(&RuntimeEvent) -> bool) -> bool {
        let Some(source) = self.app().running_generator_chains.values().next() else {
            return false;
        };
        let (endpoint, session, instance) = (
            source.endpoint_id,
            source.project_session,
            source.instance_id,
        );
        self.observer.with_snapshot(|s|s.records.iter().any(|r|matches!(&r.kind,RecordKind::RuntimeEvent{endpoint_id,project_session,instance_id,event} if *endpoint_id==endpoint && *project_session==session && *instance_id==instance && match event {RuntimeEvent::ParameterCatalogPage{request_id,..}|RuntimeEvent::ParameterSetAck{request_id,..}|RuntimeEvent::ParameterValue{request_id,..}=>*request_id==request&&accept(event),_=>false})))
    }
    fn set_current_base(&mut self) -> Check {
        let mut cursor = 0u32;
        let mut found = false;
        for _ in 0..64 {
            let source = self
                .app()
                .running_generator_chains
                .values()
                .next()
                .ok_or("source owner missing")?;
            require(
                source
                    .control
                    .request_parameter_page(0, CATALOG_REQUEST, cursor, 64),
                "actual descriptor request refused",
            )?;
            self.until(1, 5, "actual Global Volume descriptor", |f| {
                f.observed_event(
                    CATALOG_REQUEST,
                    |e| matches!(e,RuntimeEvent::ParameterCatalogPage{cursor:c,..} if *c==cursor),
                )
            })?;
            let (next, descriptor) = self
                .observer
                .with_snapshot(|s| {
                    s.records.iter().rev().find_map(|r| match &r.kind {
                        RecordKind::RuntimeEvent {
                            event:
                                RuntimeEvent::ParameterCatalogPage {
                                    request_id,
                                    cursor: c,
                                    next_cursor,
                                    items,
                                    ..
                                },
                            ..
                        } if *request_id == CATALOG_REQUEST && *c == cursor => Some((
                            *next_cursor,
                            items.iter().find(|p| p.id == GLOBAL_VOLUME).cloned(),
                        )),
                        _ => None,
                    })
                })
                .ok_or("catalog receipt disappeared")?;
            if let Some(d) = descriptor {
                require(
                    d.name == "Global Volume"
                        && !d.read_only
                        && d.current_normalized.is_finite()
                        && (0.0..=1.0).contains(&d.current_normalized)
                        && d.default_normalized
                            .is_none_or(|v| v.is_finite() && (0.0..=1.0).contains(&v)),
                    "Global Volume descriptor/range admission failed",
                )?;
                found = true;
                break;
            }
            let Some(next) = next else { break };
            require(next > cursor, "catalog cursor failed progress")?;
            cursor = next;
        }
        require(
            found,
            "Global Volume descriptor absent from bounded actual catalog",
        )?;
        require(
            self.app()
                .running_generator_chains
                .values()
                .next()
                .unwrap()
                .control
                .set_parameter_tagged(0, GLOBAL_VOLUME, 0.75, SET_REQUEST),
            "actual Global Volume set refused",
        )?;
        self.until(1,5,"actual tagged Global Volume set",|f|f.observed_event(SET_REQUEST,|e|matches!(e,RuntimeEvent::ParameterSetAck{id,value,..} if *id==GLOBAL_VOLUME && value.to_bits()==0.75_f32.to_bits())))?;
        require(
            self.app()
                .running_generator_chains
                .values()
                .next()
                .unwrap()
                .control
                .query_parameter_tagged(0, GLOBAL_VOLUME, QUERY_REQUEST),
            "actual Global Volume getter refused",
        )?;
        self.until(1,5,"actual Global Volume readback",|f|f.observed_event(QUERY_REQUEST,|e|matches!(e,RuntimeEvent::ParameterValue{id,value,..} if *id==GLOBAL_VOLUME && value.to_bits()==0.75_f32.to_bits())))?;
        require(
            self.app().project.plugin_instances[0].parameters[&GLOBAL_VOLUME].to_bits()
                == 0.25_f32.to_bits(),
            "actual base setup rewrote stale Project",
        )?;
        Ok(())
    }
}
impl GenuineFixture {
    fn no_source_audio_shape(&self) -> Check {
        let app = self.app();
        let s = self.snapshot();
        require(
            app.running_generator_chains.is_empty()
                && app.pending_generator_chains.is_empty()
                && app.pending_generator_removals.is_empty()
                && app.pending_clip_restorations.is_empty()
                && app.retired_clip_restorations.is_empty()
                && app.running_insert_chains.is_empty()
                && app.pending_insert_chains.is_empty(),
            "live/pending/retired plugin source remains",
        )?;
        require(
            s.endpoints.is_empty()
                && s.native_voices.is_empty()
                && s.generator_notes == 0
                && s.zero_pdc
                && s.compiled_plugin_routes == 0
                && s.compiled_midi_routes == 0
                && s.plugin_control_bindings == 0
                && s.compiled_audio_clips == 1,
            "audio-only exact endpoint/PDC/compiled source shape mismatch",
        )?;
        review::require_fixture_graph(&s.nodes, &s.routes)?;
        review::require_mixer_controls(&app.project, &s)?;
        require(
            s.master_gain.to_bits() == 1.0_f32.to_bits()
                && s.master_pan == 0.0
                && s.track_gains[1].to_bits() == 1.0_f32.to_bits()
                && s.track_pans[1] == 0.0
                && !s.track_muted[0]
                && !s.track_muted[1]
                && !s.track_solo.iter().any(|x| *x)
                && s.click_envelope == 0.0
                && !s.midi_input_present
                && !s.midi_recording_present
                && !s.master_capture_present
                && s.driven_automation_targets == 0,
            "actual mixer/gain/pan or extra contribution differs",
        )?;
        let pcm = self
            .verified_pcm
            .as_ref()
            .ok_or("verified PCM allocation missing")?;
        let asset = self.asset_id.ok_or("verified asset id missing")?;
        require(
            s.assets.len() == 1
                && s.assets[0].id == asset
                && s.assets[0].sample_rate == 48_000
                && s.assets[0].channels == 2
                && s.assets[0].frames == RETAINED
                && Arc::ptr_eq(&s.assets[0].samples, pcm),
            "registered callback PCM allocation/extent identity mismatch",
        )?;
        Ok(())
    }
    fn play_pass(&mut self, pass: usize, control: usize, pcm: Option<&[f32]>) -> Check<Vec<f32>> {
        require(
            self.healthy()?.class == HealthClass::Clear,
            "Play requested while App health remains pending",
        )?;
        self.require_mixer_readback("before_fixed_playback")?;
        let before = self
            .exact_paused()
            .ok_or("Play lacks confirmed frame-zero paused identity")?;
        if pass > 0 {
            self.no_source_audio_shape()?;
        }
        let count = PLAY_SEGMENTS[pass];
        let mut output = Vec::with_capacity(count * 256);
        let mut active = None;
        let mut pending_activation_ack = None;
        let settled_topology = self.topology.settled_baseline()?;
        let baseline = self.snapshot();
        let loop_count = baseline.loop_count;
        self.app_mut().toggle_play();
        require(self.app().playing, "actual App Play refused")?;
        let deadline = (Instant::now() + Duration::from_secs(30)).min(self.outer);
        while self.budget.playing[pass] < count {
            require(Instant::now() < deadline, "fixed playing pass deadline")?;
            let expected_ack = pending_activation_ack.take();
            if let Some((phase, _)) = expected_ack {
                require(
                    self.app().timeline_audio_sync.phase == phase,
                    "pending Play activation identity changed before acknowledgment",
                )?;
            }
            let health = self.poll("Play activation / fixed musical output")?;
            if let Some((phase, actual_epoch)) = expected_ack {
                require(
                    health.confirms_activation(phase, actual_epoch),
                    "next scheduled poll did not confirm exact Play activation epoch",
                )?;
            }
            if active.is_some() {
                require(
                    health.class == HealthClass::Clear,
                    "unexpected pending App health within fixed musical window",
                )?;
            }
            if active.is_none() {
                require(
                    self.budget.controls[control] < CONTROL_CEILINGS[control],
                    "Play activation control ceiling",
                )?;
            }
            let legacy_barrier = self
                .app()
                .audio
                .as_ref()
                .unwrap()
                .test_transport_request_id();
            let (segment, samples) = self.pump(
                if active.is_some() {
                    Cadence::Measured
                } else {
                    Cadence::Control
                },
                PumpPurpose::Playing {
                    pass,
                    control,
                    already_active: active.is_some(),
                },
            )?;
            if !segment.playing {
                require(active.is_none(), "playing pass unexpectedly paused")?;
                continue;
            }
            if active.is_none() {
                self.journal.emit("first_activation_identity",json!({"pass":pass,"playing":true,
                    "expected_timeline_phase":format!("{:?}",self.app().timeline_audio_sync.phase),
                    "legacy_barrier_before_pump":legacy_barrier,"segment_legacy_barrier":segment.request_id,
                    "actual_generation":segment.generation,"actual_epoch":segment.epoch,"actual_frame":segment.start_frame}))?;
            }
            require(
                health.admits_first_activation(
                    segment.generation.ok_or("activation generation missing")?,
                    segment.epoch,
                    segment.request_id,
                    legacy_barrier,
                ),
                "first musical activation has non-activation pending work or wrong acknowledgment identity",
            )?;
            // The activation segment is first measured output and is charged
            // once here, never as both a control and musical segment.
            let index = self.budget.playing[pass] - 1;
            require(
                segment.timeline_ready
                    && segment.timeline_render_active
                    && segment.playing_after_render
                    && segment.start_frame == (index * 128) as u64
                    && segment.generation == Some(before.0)
                    && segment.active_epoch == Some(segment.epoch)
                    && segment.epoch > before.1
                    && segment.loop_enabled
                    && segment.loop_start_frame == 0
                    && segment.loop_end_frame == 96_000
                    && segment.loop_count == loop_count,
                "playing segment discontinuity/loop/readiness mismatch",
            )?;
            if let Some(identity) = active {
                require(
                    identity == (segment.generation, segment.epoch),
                    "musical pass identity changed",
                )?;
            } else {
                active = Some((segment.generation, segment.epoch));
                pending_activation_ack = health
                    .pending_activation()
                    .map(|phase| (phase, segment.epoch));
            }
            for state in [segment.before, segment.after] {
                require(
                    state.fixed_quantum_bridge_gaps == baseline.fixed_quantum_bridge_gaps
                        && state.fixed_quantum_output_underflow_frames
                            == baseline.fixed_quantum_output_underflow_frames
                        && state.timeline_execution_failures == settled_topology.failures
                        && state.plugin_topology_revision == settled_topology.revision
                        && !state.plugin_topology_replan_pending
                        && !state.metronome_enabled
                        && !state.recording
                        && !state.midi_input_present
                        && !state.midi_recording_present
                        && !state.master_capture_present
                        && state.driven_automation_targets == 0,
                    "measured segment underrun or unapproved contribution",
                )?;
                if pass > 0 {
                    require(
                        state.generator_endpoints == 0
                            && state.insert_endpoints == 0
                            && state.native_voices == 0
                            && state.generator_notes == 0
                            && state.audio_voices <= 1
                            && state.zero_pdc,
                        "duplicate source/native contribution or nonzero PDC",
                    )?;
                }
            }
            if let Some(pcm) = pcm {
                for (sample_index, &actual) in samples.iter().enumerate() {
                    let frame = segment.start_frame + (sample_index / 2) as u64;
                    let expected = playback_sample(pcm, frame, sample_index % 2)?;
                    require(
                        actual.is_finite()
                            && actual.abs() <= 1.0
                            && signed_zero_equal(actual, expected),
                        &format!(
                            "exact playback IEEE mismatch at frame {frame}, channel {}: {:08x} != {:08x}",
                            sample_index % 2,
                            actual.to_bits(),
                            expected.to_bits()
                        ),
                    )?;
                }
            }
            output.extend_from_slice(&samples);
        }
        // No further musical callback is permitted after the fixed final block.
        require(
            self.healthy()?.class == HealthClass::Clear,
            "Stop requested while App health remains pending",
        )?;
        self.app_mut().stop();
        let metrics = if pass == 0 {
            require_audible_windows(
                &output,
                &[(34_800, 36_000), (36_000, 37_200), (37_200, 38_400)],
            )?
        } else {
            require_audible_windows(
                &output,
                &[(34_800, 37_200), (37_200, 39_600), (39_600, 42_000)],
            )?
        };
        self.journal.emit("fixed_playback_pass",json!({"pass":pass,"segments":count,"frames":output.len()/2,"identity":active,"loop_count":loop_count,"windows":metrics,"exact_oracle":pcm.is_some()}))?;
        self.stop_window(
            pass,
            if pass == 0 { 1 } else { control },
            active.ok_or("no musical activation")?.1,
        )?;
        Ok(output)
    }
    fn stop_window(&mut self, pass: usize, control: usize, previous_epoch: u64) -> Check {
        let settled_topology = self.topology.settled_baseline()?;
        let stopped_baseline = self.snapshot();
        let mut identity = None;
        let mut pending_activation_ack = None;
        let mut loop_count = None;
        let deadline = (Instant::now() + Duration::from_secs(15)).min(self.outer);
        while self.budget.stopped[pass] < 32 {
            require(
                Instant::now() < deadline,
                "actual Stop activation / fixed silence deadline",
            )?;
            let expected_ack = pending_activation_ack.take();
            if let Some((phase, _)) = expected_ack {
                require(
                    self.app().timeline_audio_sync.phase == phase,
                    "pending Stop activation identity changed before acknowledgment",
                )?;
            }
            let health = self.poll("Stop activation / fixed32 stopped output")?;
            if let Some((phase, actual_epoch)) = expected_ack {
                require(
                    health.confirms_activation(phase, actual_epoch),
                    "next scheduled poll did not confirm exact Stop activation epoch",
                )?;
            }
            if identity.is_some() {
                require(
                    health.class == HealthClass::Clear,
                    "unexpected pending App health within fixed stopped window",
                )?;
            }
            if identity.is_none() {
                require(
                    self.budget.controls[control] < CONTROL_CEILINGS[control],
                    "Stop activation control ceiling",
                )?;
            }
            let legacy_barrier = self
                .app()
                .audio
                .as_ref()
                .unwrap()
                .test_transport_request_id();
            let (segment, samples) = self.pump(
                if identity.is_some() {
                    Cadence::Measured
                } else {
                    Cadence::Control
                },
                PumpPurpose::Stopped {
                    pass,
                    control,
                    previous_epoch,
                    generation: self.app().timeline_desired_generation,
                    already_active: identity.is_some(),
                },
            )?;
            require(
                [segment.before, segment.after].iter().all(|state| {
                    state.plugin_topology_revision == settled_topology.revision
                        && state.timeline_execution_failures == settled_topology.failures
                        && state.fixed_quantum_bridge_gaps
                            == stopped_baseline.fixed_quantum_bridge_gaps
                        && state.fixed_quantum_output_underflow_frames
                            == stopped_baseline.fixed_quantum_output_underflow_frames
                        && !state.plugin_topology_replan_pending
                }),
                "fixed stopped topology/counter baseline changed",
            )?;
            require(
                !segment.playing && !segment.playing_after_render,
                "unaccounted playing segment after actual Stop",
            )?;
            let exact = segment.start_frame == 0
                && segment.generation == Some(self.app().timeline_desired_generation)
                && segment.active_epoch == Some(segment.epoch)
                && segment.epoch > previous_epoch;
            if !exact {
                require(identity.is_none(), "stopped identity regressed")?;
                continue;
            }
            if identity.is_none() {
                self.journal.emit("first_activation_identity",json!({"pass":pass,"playing":false,
                    "expected_timeline_phase":format!("{:?}",self.app().timeline_audio_sync.phase),
                    "legacy_barrier_before_pump":legacy_barrier,"segment_legacy_barrier":segment.request_id,
                    "actual_generation":segment.generation,"actual_epoch":segment.epoch,"actual_frame":segment.start_frame}))?;
            }
            require(
                health.admits_first_activation(
                    segment
                        .generation
                        .ok_or("stopped activation generation missing")?,
                    segment.epoch,
                    segment.request_id,
                    legacy_barrier,
                ),
                "first stopped activation has non-activation pending work or wrong acknowledgment identity",
            )?;
            if let Some(pair) = identity {
                require(
                    pair == (segment.generation, segment.epoch),
                    "stopped window identity changed",
                )?;
            } else {
                identity = Some((segment.generation, segment.epoch));
                pending_activation_ack = health
                    .pending_activation()
                    .map(|phase| (phase, segment.epoch));
                loop_count = Some(segment.loop_count);
            }
            require(
                loop_count == Some(segment.loop_count)
                    && segment.loop_enabled
                    && segment.loop_start_frame == 0
                    && segment.loop_end_frame == 96_000,
                "stopped loop identity changed",
            )?;
            require(
                samples.iter().all(|x| x.is_finite() && x.abs() <= 1e-8),
                "fixed stopped window residual output",
            )?;
        }
        let pair = identity.ok_or("stopped activation never observed")?;
        let s = self.snapshot();
        // This is the fixed end, not a polling extension to rescue pending work.
        let observed_quiescent =
            s.paused_work_quiescent(pair.0.ok_or("paused generation")?, pair.1);
        self.journal.emit("fixed_stop_window",json!({"pass":pass,"segments":32,"frames":4096,"generation":pair.0,"epoch":pair.1,"snapshot":snapshot_summary(&s),"observed_quiescent":observed_quiescent,"label":"observation_only"}))?;
        require(
            observed_quiescent,
            "source work remains at end of fixed32 stop window",
        )?;
        require(
            self.poll("consume exact Stop acknowledgment")?.class == HealthClass::Clear,
            "settled Stop acknowledgment still has pending App health",
        )?;
        require(
            self.exact_paused() == Some((pair.0.unwrap(), pair.1)),
            "actual exact Stop acknowledgment missing",
        )?;
        self.require_mixer_readback("after_fixed_stop")
    }
    fn audition_source_and_base(&mut self) -> Check {
        self.stage(2, Stage::SourceAudibility)?;
        self.play_pass(0, 0, None)?;
        self.observer.set_stage(Stage::SourceStop).map_err(error)?;
        self.set_current_base()?;
        self.until(1, 10, "native observations settle before review", |f| {
            f.exact_paused().is_some()
                && f.app()
                    .clip_render_observe(f.app().project.channels[0].id)
                    .is_ok()
        })?;
        self.passed()
    }
    fn convert(&mut self) -> Check {
        self.stage(3, Stage::Conversion)?;
        self.hooks()?;
        let review_before = review::observe(self.app(), None);
        self.journal
            .emit("selected_review_before", review_before.evidence.clone())?;
        self.app_mut().open_selected_clip_render_review();
        let review_after = review::observe(self.app(), Some(&review_before));
        self.journal
            .emit("selected_review_after", review_after.evidence.clone())?;
        let directory = self.binding.output_directory.to_string_lossy().into_owned();
        let job = self
            .app_mut()
            .clip_render_job
            .as_mut()
            .ok_or_else(|| review_after.missing_job_error())?;
        require(
            job.phase == ClipRenderPhase::Review && job.render_starter.is_none(),
            "review/factory mismatch",
        )?;
        job.review.directory = directory;
        job.review.tail_seconds = 0.25;
        job.review.tail_acknowledged = true;
        require(
            checked_fixed_tail_frames(job.review.tail_seconds, job.review.initial.sample_rate)?
                == 12_000,
            "actual rounded review tail",
        )?;
        let ctx = self.ctx.clone();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            self.app_mut().clip_render_review_ui(ui.ctx())
        });
        self.accepting_job = self.app_mut().clip_render_job.take();
        // Job stays in the outer retaining owner even if the shared method panics.
        let result = catch_unwind(AssertUnwindSafe(|| {
            self.app
                .as_mut()
                .unwrap()
                .begin_clip_render_stop(self.accepting_job.as_mut().unwrap())
        }));
        self.app.as_mut().unwrap().clip_render_job = self.accepting_job.take();
        result.map_err(|_| "shared review acceptance panicked")??;
        self.until(
            2,
            20,
            "actual preparatory stop and first capture request",
            |f| {
                f.app()
                    .clip_render_job
                    .as_ref()
                    .is_some_and(|j| j.phase == ClipRenderPhase::Capturing)
            },
        )?;
        let job = self.app().clip_render_job.as_ref().unwrap();
        let request = job
            .plan
            .as_ref()
            .ok_or("actual occurrence plan absent")?
            .request();
        require(
            request.start_frame() == 30_000
                && request.end_frame() == 54_000
                && request.sample_rate() == 48_000
                && request.tail_frames() == 12_000
                && request.tempo() == 120.0,
            "actual occurrence geometry differs before clone",
        )?;
        let events = request.local_events();
        require(
            events.len() == 2 && events[0].frame == 0 && events[1].frame == 12_000,
            "actual local note schedule differs before clone",
        )?;
        require(
            matches!(
                events[0].kind,
                crate::timeline::TimelineEventKind::NoteOn { note: 60, .. }
            ) && matches!(
                events[1].kind,
                crate::timeline::TimelineEventKind::NoteOff { note: 60, .. }
            ),
            "actual note identity differs",
        )?;
        self.journal.emit("preclone_actual_request",json!({"start":request.start_frame(),"end":request.end_frame(),"rate":request.sample_rate(),"tail":request.tail_frames(),"events":format!("{events:?}"),"stamp":format!("{:?}",job.binding)}))?;
        self.hooks()?;
        self.permit("success_clone")?;
        self.until(2,45,"two actual captures through strict render, verifier, removal, join, media and Timeline",|f|f.app().clip_render_job.as_ref().is_some_and(|j|j.phase==ClipRenderPhase::Complete))?;
        self.require_mixer_readback("conversion_complete")?;
        require(
            self.observer.with_snapshot(|s| {
                s.records.iter().any(|r| {
                    r.instance
                        .is_some_and(|i| i.phase == InstancePhase::StrictClone)
                        && matches!(
                            r.kind,
                            RecordKind::Call {
                                operation: Operation::StrictPrepare,
                                boundary: Boundary::Returned,
                                outcome: Some(CallOutcome::Ok),
                                ..
                            }
                        )
                })
            }),
            "actual strict prepare success missing",
        )?;
        self.journal.emit(
            "helper_ready",
            json!({"phase":"success_clone","evidence":"strict_prepare_succeeded"}),
        )?;
        let job = self.app().clip_render_job.as_ref().unwrap();
        require(
            job.source_removed
                && job.source_joined
                && job.retired_source.is_none()
                && job.asset_queued
                && job.asset_ack == Some(true)
                && job.asset_operation.is_some()
                && !job.owns_pending_resource(),
            "Complete lacks actual source/asset terminal receipts",
        )?;
        require(
            job.expected_timeline_generation == Some(self.app().timeline_desired_generation)
                && self.exact_paused().is_some(),
            "Complete lacks exact new Timeline readiness",
        )?;
        let expected_asset_operation = job.asset_operation;
        require(
            job.expected_timeline_generation
                .is_some_and(|generation| generation > job.review.initial.stamp.timeline_revision)
                && self
                    .exact_paused()
                    .is_some_and(|pair| pair.1 > job.review.initial.stamp.epoch),
            "conversion Timeline identity is not genuinely newer",
        )?;
        self.generated_path = Some(job.output_path()?);
        self.initial_asset_operation = expected_asset_operation;
        self.committed_project =
            Some(clip_render::project_snapshot(&self.app().project).map_err(error)?);
        self.authoritative_before = Some(
            clip_render::project_snapshot(
                self.app()
                    .undo_stack
                    .last()
                    .ok_or("authoritative before image absent")?,
            )
            .map_err(error)?,
        );
        self.passed()
    }
}
fn snapshot_summary(s: &GenuineCallbackSnapshot) -> Value {
    // Keep the original flat evidence schema, but bound each json! expansion.
    // The three fixed inventories are disjoint; reject a duplicate before it
    // can overwrite evidence rather than relying on Map::extend semantics.
    let parts = [
        json!({
            "engine":s.engine_session,
            "project":s.project_session,
            "plugin_topology_revision":s.plugin_topology_revision,
            "plugin_topology_replan_pending":s.plugin_topology_replan_pending,
            "timeline_execution_failures":s.timeline_execution_failures,
            "needs_resync":s.timeline_stats.is_some_and(|stats| stats.ownership_needs_resync),
            "suspended":s.suspended,
            "graph_was_activated":s.graph_was_activated,
            "graph_binding_exact":s.graph_binding_exact,
            "channel_binding_revision":s.channel_binding_revision,
            "channel_binding_epoch":s.channel_binding_epoch,
            "active_plugin_automation_bindings":s.active_plugin_automation_bindings,
            "candidate_plugin_automation_bindings":s.candidate_plugin_automation_bindings,
            "compiled_midi_routes":s.compiled_midi_routes
        }),
        json!({
            "lifecycle_observed_count":s.lifecycle_observed_count,
            "lifecycle_overflowed":s.lifecycle_overflowed,
            "lifecycle_inconsistent_pair":s.lifecycle_inconsistent_pair,
            "generation":s.generation,
            "active_epoch":s.active_epoch,
            "epoch":s.epoch,
            "frame":s.start_frame,
            "playing":s.playing,
            "loop_count":s.loop_count,
            "pending_audio":s.pending_audio_commands,
            "pending_timeline":s.pending_timeline_commands,
            "pending_activation":s.pending_timeline_activation,
            "endpoints":s.endpoints.iter().map(|e|json!({"id":e.endpoint_id,"instance":e.plugin_instance_id,"mixer_track":e.mixer_track,"stopped_epoch_safety":startup::stopped_epoch_safety_evidence(e.stopped_epoch_safety_epoch,e.stopped_epoch_safety_remaining_frames,e.stopped_epoch_safety_failed),"fixed":format!("{:?}",e.fixed),"exact":format!("{:?}",e.exact_snapshot),"latency":format!("{:?}",e.coherent_latency)})).collect::<Vec<_>>(),
            "assets":s.assets.iter().map(|a|json!({"id":a.id,"rate":a.sample_rate,"channels":a.channels,"frames":a.frames,"arc_address":Arc::as_ptr(&a.samples) as *const f32 as usize})).collect::<Vec<_>>()
        }),
        json!({
            "native_voices":s.native_voices.len(),
            "audio_voices":s.audio_voices.len(),
            "zero_pdc":s.zero_pdc,
            "pdc_revision":s.pdc_revision,
            "graph_identity":s.graph_identity,
            "graph_pdc":format!("{:?}",s.graph_pdc),
            "raw_delays":format!("{:?}",s.raw_delays),
            "raw_delay_active":s.raw_delay_active,
            "legacy_pdc_active":s.legacy_pdc_active,
            "route_delays":format!("{:?}",s.route_delays),
            "master_gain":s.master_gain,
            "master_pan":s.master_pan,
            "track1_gain":s.track_gains[1],
            "track1_pan":s.track_pans[1]
        }),
    ];
    let mut summary = serde_json::Map::new();
    for part in parts {
        let Value::Object(part) = part else {
            unreachable!("snapshot summary parts are fixed object literals");
        };
        for (key, value) in part {
            match summary.entry(key) {
                serde_json::map::Entry::Vacant(entry) => {
                    entry.insert(value);
                }
                serde_json::map::Entry::Occupied(entry) => {
                    panic!("duplicate snapshot summary key: {}", entry.key());
                }
            }
        }
    }
    Value::Object(summary)
}

#[test]
fn snapshot_summary_preserves_flat_keys_values_and_explicit_nulls() {
    // Device-free read only: no callback pump, worker, source load or plugin.
    let (_engine, callback) = AudioEngine::test_engine_with_callback();
    let sampled = callback.genuine_snapshot();
    let mut nondefault = sampled.clone();
    nondefault.plugin_topology_revision = 4;
    nondefault.plugin_topology_replan_pending = true;
    nondefault.timeline_execution_failures = 4;
    nondefault.timeline_stats = Some(Default::default());
    nondefault
        .timeline_stats
        .as_mut()
        .unwrap()
        .ownership_needs_resync = true;
    nondefault.suspended = true;
    nondefault.graph_was_activated = true;
    nondefault.graph_binding_exact = false;
    nondefault.channel_binding_revision = Some(37);
    nondefault.channel_binding_epoch = Some(41);
    nondefault.active_plugin_automation_bindings = 2;
    nondefault.candidate_plugin_automation_bindings = 3;
    nondefault.compiled_midi_routes = 1;
    nondefault.lifecycle_observed_count = 16;
    nondefault.lifecycle_overflowed = true;
    nondefault.lifecycle_inconsistent_pair = true;
    nondefault.generation = Some(37);
    nondefault.active_epoch = Some(41);
    nondefault.epoch = 41;
    nondefault.start_frame = 43;
    nondefault.playing = true;
    nondefault.loop_count = 47;
    nondefault.pending_audio_commands = 1;
    nondefault.pending_timeline_commands = 2;
    nondefault.pending_timeline_activation = true;
    nondefault.graph_identity = Some((37, 41, 53));
    nondefault.pdc_revision = 59;
    nondefault.master_gain = 0.75;
    nondefault.master_pan = -0.25;
    nondefault.track_gains[1] = 0.5;
    nondefault.track_pans[1] = 0.25;
    nondefault
        .assets
        .push(crate::audio::genuine_observation::GenuineAssetObservation {
            slot: 0,
            id: 61,
            samples: Arc::from([0.25_f32, -0.25, 0.5, -0.5]),
            sample_rate: 48_000,
            channels: 2,
            frames: 2,
        });
    let mut nulls = nondefault.clone();
    nulls.channel_binding_revision = None;
    nulls.channel_binding_epoch = None;
    nulls.generation = None;
    nulls.active_epoch = None;
    nulls.graph_identity = None;
    nulls.timeline_stats = None;
    let keys = [
        "engine",
        "project",
        "plugin_topology_revision",
        "plugin_topology_replan_pending",
        "timeline_execution_failures",
        "needs_resync",
        "suspended",
        "graph_was_activated",
        "graph_binding_exact",
        "channel_binding_revision",
        "channel_binding_epoch",
        "active_plugin_automation_bindings",
        "candidate_plugin_automation_bindings",
        "compiled_midi_routes",
        "lifecycle_observed_count",
        "lifecycle_overflowed",
        "lifecycle_inconsistent_pair",
        "generation",
        "active_epoch",
        "epoch",
        "frame",
        "playing",
        "loop_count",
        "pending_audio",
        "pending_timeline",
        "pending_activation",
        "endpoints",
        "assets",
        "native_voices",
        "audio_voices",
        "zero_pdc",
        "pdc_revision",
        "graph_identity",
        "graph_pdc",
        "raw_delays",
        "raw_delay_active",
        "legacy_pdc_active",
        "route_delays",
        "master_gain",
        "master_pan",
        "track1_gain",
        "track1_pan",
    ];
    let expected_keys = keys.into_iter().collect::<std::collections::BTreeSet<_>>();
    assert_eq!(expected_keys.len(), 42);
    for s in [&sampled, &nondefault, &nulls] {
        // Independent flat construction: one bounded json! per original value,
        // no production parts, merge, key inventory or duplicate guard reused.
        let entries = [
            ("engine",json!(s.engine_session)),
            ("project",json!(s.project_session)),
            ("plugin_topology_revision",json!(s.plugin_topology_revision)),
            ("plugin_topology_replan_pending",json!(s.plugin_topology_replan_pending)),
            ("timeline_execution_failures",json!(s.timeline_execution_failures)),
            ("needs_resync",json!(s.timeline_stats.is_some_and(|stats| stats.ownership_needs_resync))),
            ("suspended",json!(s.suspended)),
            ("graph_was_activated",json!(s.graph_was_activated)),
            ("graph_binding_exact",json!(s.graph_binding_exact)),
            ("channel_binding_revision",json!(s.channel_binding_revision)),
            ("channel_binding_epoch",json!(s.channel_binding_epoch)),
            ("active_plugin_automation_bindings",json!(s.active_plugin_automation_bindings)),
            ("candidate_plugin_automation_bindings",json!(s.candidate_plugin_automation_bindings)),
            ("compiled_midi_routes",json!(s.compiled_midi_routes)),
            ("lifecycle_observed_count",json!(s.lifecycle_observed_count)),
            ("lifecycle_overflowed",json!(s.lifecycle_overflowed)),
            ("lifecycle_inconsistent_pair",json!(s.lifecycle_inconsistent_pair)),
            ("generation",json!(s.generation)),
            ("active_epoch",json!(s.active_epoch)),
            ("epoch",json!(s.epoch)),
            ("frame",json!(s.start_frame)),
            ("playing",json!(s.playing)),
            ("loop_count",json!(s.loop_count)),
            ("pending_audio",json!(s.pending_audio_commands)),
            ("pending_timeline",json!(s.pending_timeline_commands)),
            ("pending_activation",json!(s.pending_timeline_activation)),
            ("endpoints",json!(s.endpoints.iter().map(|e|json!({"id":e.endpoint_id,"instance":e.plugin_instance_id,"mixer_track":e.mixer_track,"stopped_epoch_safety":startup::stopped_epoch_safety_evidence(e.stopped_epoch_safety_epoch,e.stopped_epoch_safety_remaining_frames,e.stopped_epoch_safety_failed),"fixed":format!("{:?}",e.fixed),"exact":format!("{:?}",e.exact_snapshot),"latency":format!("{:?}",e.coherent_latency)})).collect::<Vec<_>>())),
            ("assets",json!(s.assets.iter().map(|a|json!({"id":a.id,"rate":a.sample_rate,"channels":a.channels,"frames":a.frames,"arc_address":Arc::as_ptr(&a.samples) as *const f32 as usize})).collect::<Vec<_>>())),
            ("native_voices",json!(s.native_voices.len())),
            ("audio_voices",json!(s.audio_voices.len())),
            ("zero_pdc",json!(s.zero_pdc)),
            ("pdc_revision",json!(s.pdc_revision)),
            ("graph_identity",json!(s.graph_identity)),
            ("graph_pdc",json!(format!("{:?}",s.graph_pdc))),
            ("raw_delays",json!(format!("{:?}",s.raw_delays))),
            ("raw_delay_active",json!(s.raw_delay_active)),
            ("legacy_pdc_active",json!(s.legacy_pdc_active)),
            ("route_delays",json!(format!("{:?}",s.route_delays))),
            ("master_gain",json!(s.master_gain)),
            ("master_pan",json!(s.master_pan)),
            ("track1_gain",json!(s.track_gains[1])),
            ("track1_pan",json!(s.track_pans[1]))
        ];
        let expected = Value::Object(
            entries
                .into_iter()
                .map(|(key, value)| (key.to_owned(), value))
                .collect(),
        );
        let actual = snapshot_summary(s);
        assert_eq!(actual, expected);
        let object = actual.as_object().unwrap();
        assert_eq!(object.len(), 42);
        assert_eq!(
            object
                .keys()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>(),
            expected_keys
        );
    }
    let explicit_nulls = snapshot_summary(&nulls);
    for key in [
        "channel_binding_revision",
        "channel_binding_epoch",
        "generation",
        "active_epoch",
        "graph_identity",
    ] {
        assert_eq!(
            explicit_nulls.as_object().unwrap().get(key),
            Some(&Value::Null)
        );
    }
    let changed = snapshot_summary(&nondefault);
    assert_eq!(changed["plugin_topology_revision"], 4);
    assert_eq!(changed["timeline_execution_failures"], 4);
    assert_eq!(changed["lifecycle_observed_count"], 16);
    assert_eq!(changed["needs_resync"], true);
    assert_eq!(changed["assets"].as_array().unwrap().len(), 1);
}

impl GenuineFixture {
    fn audit_media_and_history(&mut self) -> Check {
        self.stage(4, Stage::Conversion)?;
        let records = self.all_observations();
        let captures = records
            .iter()
            .filter_map(|r| match &r.kind {
                RecordKind::Capture {
                    label,
                    identity,
                    receipt_identity,
                    state,
                } => Some((*label, *identity, *receipt_identity, state)),
                _ => None,
            })
            .collect::<Vec<_>>();
        require(
            captures.len() == 2
                && captures[0].0 == CaptureLabel::Initial
                && captures[1].0 == CaptureLabel::Final
                && captures[0].1 == captures[1].1
                && captures[0].2 != captures[1].2,
            "dedicated capture receipt cardinality/identity differs",
        )?;
        let captured = captures[0].3;
        let recaptured = captures[1].3;
        expected_metadata(&captured.metadata)?;
        expected_metadata(&recaptured.metadata)?;
        require(
            captured.metadata == recaptured.metadata
                && captured.opaque_state == recaptured.opaque_state
                && !captured.opaque_state.is_empty()
                && captured.native_generation == recaptured.native_generation
                && captured.native_dirty_revision == recaptured.native_dirty_revision,
            "dedicated source recapture authority changed",
        )?;
        let bases = |s: &crate::instrument_bounce::BouncePluginState| {
            s.parameter_bases
                .iter()
                .map(|(id, v)| (*id, v.to_bits()))
                .collect::<Vec<_>>()
        };
        require(
            bases(captured) == bases(recaptured)
                && captured
                    .parameter_bases
                    .get(&GLOBAL_VOLUME)
                    .map(|v| v.to_bits())
                    == Some(0.75_f32.to_bits()),
            "two actual captures failed current0.75 exact base equality",
        )?;
        let mut generated = Vec::with_capacity(36_096 * 2);
        let mut block = 0usize;
        let mut midi = Vec::new();
        let mut process_attempts = 0;
        for r in &records {
            if r.instance
                .is_some_and(|i| i.phase == InstancePhase::StrictClone)
            {
                match &r.kind {
                    RecordKind::Call {
                        operation: Operation::StrictProcess,
                        boundary: Boundary::Attempt,
                        frames: Some(n),
                        ..
                    } => {
                        require(*n == 128, "strict clone positive call is not Q128")?;
                        process_attempts += 1;
                    }
                    RecordKind::StrictPcm {
                        frames,
                        transport,
                        channels,
                        transport_applied,
                        output_events,
                        output_events_lost,
                    } => {
                        require(
                            *frames == 128
                                && channels.len() == 2
                                && channels.iter().all(|c| c.len() == 128)
                                && *transport_applied
                                && *output_events == 0
                                && !*output_events_lost,
                            "actual strict Process reply shape/transport/events differs",
                        )?;
                        let frame = 30_000 + block as i64 * 128;
                        require(
                            transport.sample_position == frame
                                && transport.quarter_note_position == (frame as f64 / 24_000.0)
                                && transport.tempo == 120.0
                                && transport.playing
                                && transport.time_sig_numerator == 4
                                && transport.time_sig_denominator == 4,
                            "strict clone first/absolute transport differs",
                        )?;
                        for (&left, &right) in channels[0].iter().zip(&channels[1]) {
                            generated.push(left);
                            generated.push(right);
                        }
                        block += 1;
                    }
                    RecordKind::Midi { data, offset } => midi.push((block, *data, *offset)),
                    RecordKind::Metadata { result, .. } => {
                        expected_metadata(result.as_ref().map_err(error)?)?
                    }
                    _ => {}
                }
            }
        }
        require(
            block == 282 && process_attempts == 282,
            "strict clone had extra/missing/warmup Process calls",
        )?;
        require(
            midi == [(0, [0x90, 60, 100], 0), (93, [0x80, 60, 0], 96)],
            "strict clone MIDI ledger differs from frozen note schedule",
        )?;
        let generated_metrics = oracles::validate_generated_audio(&generated)?;
        let retained = oracles::trim_q128_stereo(&generated, 24_000, 12_000, 0)?;
        let path = self
            .generated_path
            .as_ref()
            .ok_or("generated path absent")?;
        let wav = fs::read(path).map_err(error)?;
        let wav_metrics = oracles::validate_float32_wav(&wav, &retained)?;
        let reports = records
            .iter()
            .filter_map(|r| match &r.kind {
                RecordKind::RenderReport(report) => Some(report),
                _ => None,
            })
            .collect::<Vec<_>>();
        require(
            reports.len() == 1,
            "actual Complete render report absent/duplicated",
        )?;
        let report = reports[0];
        require(
            report.identity == captures[0].1
                && report.origin_frame == 30_000
                && report.musical_frames == 24_000
                && report.frames == 36_000
                && report.fixed_tail == 12_000
                && report.native_latency == 0
                && report.reported_tail == 96_000
                && report.generated_frames == 36_096,
            "actual Complete report geometry/identity differs",
        )?;
        let position = |site: &str| {
            records.iter().find_map(|r| {
                matches!(&r.kind,RecordKind::Lifecycle(s) if *s==site).then_some(r.ordinal)
            })
        };
        let reap =
            position("strict Client Reaped success").ok_or("actual strict Client Reaped absent")?;
        let destroyed = position("renderer backend destruction returned")
            .ok_or("strict backend destructor acknowledgment absent")?;
        let publish = position("occurrence writer publication attempt")
            .ok_or("actual writer publication boundary absent")?;
        require(
            reap < destroyed
                && destroyed < publish
                && records
                    .iter()
                    .filter(|r| {
                        matches!(
                            &r.kind,
                            RecordKind::Disposal {
                                terminated: true,
                                error: None,
                                ..
                            }
                        )
                    })
                    .count()
                    == 1,
            "strict reap/disposal/destruction must precede publication",
        )?;
        let exact_removal = self
            .app()
            .clip_render_job
            .as_ref()
            .and_then(|job| job.removal)
            .ok_or("actual source removal stamp missing")?;
        let removals=records.iter().filter(|r|matches!(&r.kind,RecordKind::AppGeneratorEndpointEvent(audio::GeneratorEndpointEvent::StampedRemoved{stamp,result:Ok(())}) if *stamp==exact_removal)).collect::<Vec<_>>();
        require(
            removals.len() == 1 && Some(exact_removal.endpoint_id) == self.original_endpoint,
            "source exact stamped removal receipt missing/duplicate",
        )?;
        let joins=records.iter().filter(|r|matches!(&r.kind,RecordKind::WorkerJoin{worker_id,outcome:observe::WorkerJoinOutcome::Joined} if Some(*worker_id)==self.original_worker)).collect::<Vec<_>>();
        require(
            joins.len() == 1 && removals[0].ordinal < joins[0].ordinal,
            "original source exact Rust Joined did not follow removal",
        )?;
        let registrations=records.iter().filter(|r|matches!(&r.kind,RecordKind::AppAudioAssetEvent(AudioAssetEvent::Registered{operation,success:true,..}) if Some(*operation)==self.initial_asset_operation)).collect::<Vec<_>>();
        require(
            registrations.len() == 1 && joins[0].ordinal < registrations[0].ordinal,
            "conversion exact asset registration ack did not follow source Join",
        )?;
        let app = self.app();
        require(
            app.project.audio_assets.len() == 1
                && app.undo_stack.len() == 1
                && app.redo_stack.is_empty(),
            "conversion asset/history extent differs",
        )?;
        let asset = &app.project.audio_assets[0];
        require(
            asset.sample_rate == 48_000
                && asset.channels == 2
                && asset.bits_per_sample == 32
                && asset.frames == 36_000
                && asset.path == *path,
            "committed asset metadata/path differs",
        )?;
        let cached = app
            .cached_audio_assets
            .get(&asset.id)
            .ok_or("actual verified cache missing")?;
        require(
            cached.project_session == app.project_session
                && cached.project_asset_id == asset.id
                && cached.path == *path
                && cached.sample_rate == 48_000
                && cached.channels == 2
                && cached.samples.len() == retained.len()
                && cached
                    .samples
                    .iter()
                    .zip(&retained)
                    .all(|(a, b)| a.to_bits() == b.to_bits()),
            "verified cache PCM differs from every raw WAV bit",
        )?;
        let audio_clips = app
            .project
            .clips
            .iter()
            .filter(|c| c.kind == ClipKind::Audio)
            .collect::<Vec<_>>();
        require(audio_clips.len() == 1, "not exactly one new Audio Clip")?;
        let clip = audio_clips[0];
        require(
            clip.start.to_bits() == 1.25_f32.to_bits()
                && clip.length.to_bits() == 1.5_f32.to_bits()
                && clip.audio_asset_id == Some(asset.id)
                && clip.audio_source_offset_frame == Some(0)
                && clip.gain.to_bits() == 0.75_f32.to_bits()
                && clip.fade_in == 0.0
                && clip.fade_out == 0.0
                && !clip.muted,
            "Audio Clip placement/gain/offset/fades differs",
        )?;
        require(
            app.project.audio_clip_mixer_destinations.len() == 1
                && app.project.audio_clip_mixer_destinations[0].clip_id == clip.id
                && app.project.audio_clip_mixer_destinations[0].mixer_track_id == 1,
            "Audio Clip stable mixer destination differs",
        )?;
        let mut expected_before = self
            .initial_project
            .as_ref()
            .ok_or("initial Project freeze missing")?
            .clone();
        expected_before.plugin_instances[0].parameters = captured.parameter_bases.clone();
        expected_before.plugin_instances[0].opaque_state = captured.opaque_state.clone();
        require(
            clip_render::project_matches_snapshot(
                &expected_before,
                self.authoritative_before.as_ref().unwrap(),
            )
            .map_err(error)?,
            "authoritative before image changed unrelated Project fields",
        )?;
        let mut projected = app.project.clone();
        projected.audio_assets.clear();
        projected.audio_clip_mixer_destinations.clear();
        projected.clips.retain(|c| c.kind != ClipKind::Audio);
        projected.clips[0].muted = false;
        projected.plugin_instances[0].enabled = true;
        require(
            clip_render::project_matches_snapshot(
                &projected,
                self.authoritative_before.as_ref().unwrap(),
            )
            .map_err(error)?,
            "committed image changed unrelated notes/steps/source placement/channel/routes/history fields",
        )?;
        require(
            app.project
                .clips
                .iter()
                .find(|c| c.kind == ClipKind::Pattern)
                .is_some_and(|c| c.muted)
                && !app.project.plugin_instances[0].enabled,
            "source was not muted/disabled",
        )?;
        let asset_id = asset.id;
        let verified_pcm = Arc::clone(&cached.samples);
        self.asset_id = Some(asset_id);
        self.verified_pcm = Some(verified_pcm);
        self.original_wav = Some(wav);
        self.assert_compiled(true)?;
        self.no_source_audio_shape()?;
        self.journal.emit("independent_pcm_audit",json!({"generated":generated_metrics,"wav":wav_metrics,"capture_receipts":[captures[0].2,captures[1].2],"raw_native_generation":captured.native_generation,"raw_native_dirty_revision":captured.native_dirty_revision,"authoritative_state_sha256":oracles::sha256_hex(&captured.opaque_state),"before_project_sha256":oracles::sha256_hex(self.authoritative_before.as_ref().unwrap()),"after_project_sha256":oracles::sha256_hex(self.committed_project.as_ref().unwrap()),"snapshot":snapshot_summary(&self.snapshot())}))?;
        self.assert_launches(2, false)?;
        self.passed()
    }
    fn assert_compiled(&self, audio: bool) -> Check {
        let (_, timeline) = self
            .app()
            .latest_compiled_timeline()
            .ok_or("current compiled Timeline absent")?;
        let notes = timeline
            .events()
            .iter()
            .filter(|e| matches!(e.kind, crate::timeline::TimelineEventKind::NoteOn { .. }))
            .count();
        let clips = timeline.audio_clips();
        if audio {
            require(
                notes == 0
                    && clips.len() == 1
                    && timeline.plugin_routes().is_empty()
                    && timeline.midi_port_routes().is_empty(),
                "compiled Audio-only source is not exclusive",
            )?;
            let c = clips[0];
            require(
                c.start_frame == 30_000
                    && c.clip_end_frame == 66_000
                    && c.stop_frame == 66_000
                    && c.source_offset_frame == 0
                    && c.source_elapsed_frames == 0
                    && c.timeline_sample_rate == 48_000
                    && c.source_sample_rate == 48_000
                    && c.gain.to_bits() == 0.75_f32.to_bits()
                    && c.mixer_track == 1,
                "compiled exact audio occurrence differs",
            )?;
        } else {
            require(
                notes == 1 && clips.is_empty() && timeline.plugin_routes().len() == 1,
                "compiled restored MIDI source differs",
            )?;
        }
        Ok(())
    }
    fn audio_pass(&mut self, pass: usize) -> Check {
        let (stage, observation, control) = if pass == 1 {
            (5, Stage::FirstPlayback, 3)
        } else {
            (8, Stage::SecondPlayback, 6)
        };
        self.stage(stage, observation)?;
        let pcm = Arc::clone(self.verified_pcm.as_ref().ok_or("verified PCM absent")?);
        let output = self.play_pass(pass, control, Some(&pcm))?;
        if pass == 1 {
            self.first_playback = Some(output);
        } else {
            let first = self
                .first_playback
                .as_ref()
                .ok_or("first playback evidence absent")?;
            require(
                first.len() == output.len()
                    && first
                        .iter()
                        .zip(&output)
                        .all(|(a, b)| signed_zero_equal(*a, *b)),
                "Redo differs from first conversion at the same absolute output frame",
            )?;
        }
        self.verify_private_files()?;
        self.passed()
    }
    fn undo_source(&mut self) -> Check {
        self.stage(6, Stage::Undo)?;
        self.hooks()?;
        let previous = self.exact_paused().ok_or("Undo missing exact stop")?;
        self.record_mixer_evidence("undo_before_App_call")?;
        self.permit("undo_source")?;
        self.app_mut().undo();
        self.record_mixer_evidence("undo_after_App_call")?;
        require(
            self.app().clip_restoration_playback_pending
                && self.app().running_generator_chains.is_empty(),
            "Undo failed to gate playback on genuinely new source",
        )?;
        self.until(
            4,
            30,
            "Undo fresh source prepare/install/new Timeline",
            |f| {
                !f.app().clip_restoration_playback_pending
                    && !f.app().clip_restoration_failed
                    && f.exact_paused()
                        .is_some_and(|pair| pair.0 > previous.0 && pair.1 > previous.1)
                    && f.app().running_generator_chains.len() == 1
            },
        )?;
        let pair = self.exact_paused().unwrap();
        self.until(4, 5, "Undo fixed source settlement", |f| {
            f.snapshot().paused_work_quiescent(pair.0, pair.1)
        })?;
        self.require_mixer_readback("undo_ready")?;
        require(
            clip_render::project_matches_snapshot(
                &self.app().project,
                self.authoritative_before.as_ref().unwrap(),
            )
            .map_err(error)?,
            "Undo is not exact authoritative before image",
        )?;
        let s = self.snapshot();
        require(
            s.endpoints.len() == 1
                && Some(s.endpoints[0].endpoint_id) != self.original_endpoint
                && s.endpoints[0].mixer_track == 1
                && s.timing.callback_budget_frames == 2048
                && s.timing.guard_quanta == 2
                && s.timing.lookahead_quanta == 18
                && s.timing.bridge_latency_frames == 2432,
            "Undo exact new endpoint/timing mismatch",
        )?;
        self.require_source_owner_binding(&s, InstancePhase::UndoSource)?;
        self.undo_endpoint = Some(s.endpoints[0].endpoint_id);
        self.undo_worker = Some(s.endpoints[0].fixed.bridge.worker_id);
        let records = self.all_observations();
        let captured = records
            .iter()
            .find_map(|r| match &r.kind {
                RecordKind::Capture {
                    label: CaptureLabel::Initial,
                    state,
                    ..
                } => Some(state),
                _ => None,
            })
            .ok_or("first authoritative capture missing")?;
        let undo = records
            .iter()
            .filter(|r| {
                r.instance
                    .is_some_and(|i| i.phase == InstancePhase::UndoSource)
            })
            .collect::<Vec<_>>();
        require(undo.iter().filter(|r|matches!(&r.kind,RecordKind::StateLoad{bytes,outcome:CallOutcome::Ok} if bytes==&captured.opaque_state)).count()==1,"Undo did not load authoritative captured bytes exactly once")?;
        require(undo.iter().any(|r|matches!(&r.kind,RecordKind::BaseRead{id,raw_bits:Ok(bits),..} if *id==GLOBAL_VOLUME && f64::from_bits(*bits)==0.75)),"Undo actual base0.75 getter missing")?;
        // Fresh preflight metadata before Start is retained verbatim, but only
        // post-Start metadata inside the successful preparation fence is ready.
        let prepared = undo
            .iter()
            .filter(|r| {
                matches!(
                    &r.kind,
                    RecordKind::Call {
                        operation: Operation::SourcePrepare,
                        boundary: Boundary::Returned,
                        outcome: Some(CallOutcome::Ok),
                        ..
                    }
                )
            })
            .collect::<Vec<_>>();
        require(
            prepared.len() == 1,
            "Undo successful SourcePrepare return missing or duplicated",
        )?;
        let prepare_return = prepared[0].ordinal;
        let preparing = undo
            .iter()
            .filter(|r| {
                matches!(
                    &r.kind,
                    RecordKind::Call {
                        operation: Operation::SourcePrepare,
                        boundary: Boundary::Attempt,
                        ..
                    }
                )
            })
            .collect::<Vec<_>>();
        require(
            preparing.len() == 1 && preparing[0].ordinal < prepare_return,
            "Undo exact SourcePrepare attempt/return fence missing",
        )?;
        let prepare_attempt = preparing[0].ordinal;
        let started = undo
            .iter()
            .filter(|r| {
                prepare_attempt < r.ordinal
                    && r.ordinal < prepare_return
                    && matches!(
                        &r.kind,
                        RecordKind::Call {
                            operation: Operation::StartProcessing,
                            boundary: Boundary::Returned,
                            outcome: Some(CallOutcome::Ok),
                            ..
                        }
                    )
            })
            .collect::<Vec<_>>();
        require(
            started.len() == 1,
            "Undo preparation successful Start return missing or duplicated",
        )?;
        let start_return = started[0].ordinal;
        let ready_metadata = undo
            .iter()
            .filter(|r| start_return < r.ordinal && r.ordinal < prepare_return)
            .filter_map(|r| match &r.kind {
                RecordKind::Metadata { result, .. } => Some(result),
                _ => None,
            })
            .collect::<Vec<_>>();
        require(
            !ready_metadata.is_empty(),
            "Undo complete post-Start readiness metadata missing",
        )?;
        for metadata in ready_metadata {
            expected_metadata(metadata.as_ref().map_err(error)?)?;
        }
        self.journal.emit(
            "helper_ready",
            json!({"phase":"undo_source","evidence":"slot_ready_and_installed"}),
        )?;
        self.assert_compiled(false)?;
        self.assert_launches(3, false)?;
        self.verify_private_files()?;
        self.journal.emit("undo_actual_restoration",json!({"new_endpoint":self.undo_endpoint,"prior_endpoint":self.original_endpoint,"identity":pair,"snapshot":snapshot_summary(&s),"state_load_sha256":oracles::sha256_hex(&captured.opaque_state),"base_readback_bits":0.75_f64.to_bits(),"opaque_save_back_claimed":false}))?;
        self.passed()
    }
    fn redo_retirement(&mut self) -> Check {
        self.stage(7, Stage::Redo)?;
        let previous = self
            .exact_paused()
            .ok_or("Redo missing exact paused source")?;
        let before_counts = self.observer.with_snapshot(|s| {
            (
                s.clone_replies,
                s.records
                    .iter()
                    .filter(|r| matches!(r.kind, RecordKind::Capture { .. }))
                    .count(),
                s.records
                    .iter()
                    .filter(|r| {
                        matches!(
                            r.kind,
                            RecordKind::Lifecycle("instance construction observed")
                        )
                    })
                    .count(),
            )
        });
        self.record_mixer_evidence("redo_before_App_call")?;
        self.app_mut().redo();
        self.record_mixer_evidence("redo_after_App_call")?;
        self.until(
            5,
            20,
            "Redo exact source removal/join/retained media/new Timeline",
            |f| {
                !f.app().clip_restoration_playback_pending
                    && f.exact_paused()
                        .is_some_and(|pair| pair.0 > previous.0 && pair.1 > previous.1)
                    && f.app().running_generator_chains.is_empty()
                    && f.app().pending_generator_chains.is_empty()
                    && f.app().pending_generator_removals.is_empty()
                    && f.app().retired_clip_restorations.is_empty()
            },
        )?;
        self.require_mixer_readback("redo_ready")?;
        require(
            clip_render::project_matches_snapshot(
                &self.app().project,
                self.committed_project.as_ref().unwrap(),
            )
            .map_err(error)?,
            "Redo changed authoritative committed Project",
        )?;
        let after_counts = self.observer.with_snapshot(|s| {
            (
                s.clone_replies,
                s.records
                    .iter()
                    .filter(|r| matches!(r.kind, RecordKind::Capture { .. }))
                    .count(),
                s.records
                    .iter()
                    .filter(|r| {
                        matches!(
                            r.kind,
                            RecordKind::Lifecycle("instance construction observed")
                        )
                    })
                    .count(),
            )
        });
        require(
            before_counts == after_counts,
            "Redo created render/capture/source work",
        )?;
        require(
            self.app().undo_stack.len() == 1 && self.app().redo_stack.is_empty(),
            "Redo history identity differs",
        )?;
        let pcm = self.verified_pcm.as_ref().unwrap();
        let cached = &self.app().cached_audio_assets[&self.asset_id.unwrap()];
        require(
            Arc::ptr_eq(pcm, &cached.samples),
            "Redo did not reuse exact retained verified PCM allocation",
        )?;
        let records = self.all_observations();
        let removed=records.iter().filter(|r|r.stage==Stage::Redo).filter(|r|matches!(&r.kind,RecordKind::AppGeneratorEndpointEvent(audio::GeneratorEndpointEvent::Removed{endpoint_id:Some(id),..}) if Some(*id)==self.undo_endpoint)).collect::<Vec<_>>();
        let joined=records.iter().filter(|r|r.stage==Stage::Redo).filter(|r|matches!(&r.kind,RecordKind::WorkerJoin{worker_id,outcome:observe::WorkerJoinOutcome::Joined} if Some(*worker_id)==self.undo_worker)).collect::<Vec<_>>();
        let registered = records
            .iter()
            .filter(|r| r.stage == Stage::Redo)
            .filter_map(|r| match &r.kind {
                RecordKind::AppAudioAssetEvent(AudioAssetEvent::Registered {
                    operation,
                    id,
                    success: true,
                }) if Some(*id) == self.asset_id => Some((r.ordinal, *operation)),
                _ => None,
            })
            .collect::<Vec<_>>();
        require(
            removed.len() == 1 && joined.len() == 1 && removed[0].ordinal < joined[0].ordinal,
            "Redo exact Undo-source removal/Joined missing or duplicated",
        )?;
        require(
            registered.len() == 1
                && Some(registered[0].1) != self.initial_asset_operation
                && registered[0].1.engine_session == self.app().audio_asset_engine_session
                && registered[0].1.generation == self.app().audio_asset_generation,
            "Redo new exact asset operation/session registration ack missing",
        )?;
        self.no_source_audio_shape()?;
        self.assert_compiled(true)?;
        self.assert_launches(3, true)?;
        self.verify_private_files()?;
        self.journal.emit("redo_actual_retirement",json!({"retired_endpoint":self.undo_endpoint,"new_identity":self.exact_paused(),"reused_arc":true,"snapshot":snapshot_summary(&self.snapshot())}))?;
        self.passed()
    }
}
impl GenuineFixture {
    fn verify_private_files(&self) -> Check {
        let path = self.generated_path.as_ref().ok_or("render path missing")?;
        require(
            fs::read(path).map_err(error)?.as_slice()
                == self
                    .original_wav
                    .as_ref()
                    .ok_or("raw WAV inventory missing")?
                    .as_slice(),
            "generated WAV changed during Undo/Redo",
        )?;
        require(
            fs::read(self.binding.output_directory.join("sibling-sentinel.txt")).map_err(error)?
                == SENTINEL,
            "sibling sentinel changed",
        )?;
        let mut entries = fs::read_dir(&self.binding.output_directory)
            .map_err(error)?
            .map(|entry| entry.map(|e| e.path()).map_err(error))
            .collect::<Check<Vec<_>>>()?;
        entries.sort();
        let mut expected = vec![
            path.clone(),
            self.binding.output_directory.join("sibling-sentinel.txt"),
        ];
        expected.sort();
        require(
            entries == expected,
            "private output directory gained/lost unrelated files",
        )
    }
    fn launch_records(&self) -> Check<Vec<Value>> {
        let path = self.binding.run.join("launches.jsonl");
        let metadata = fs::symlink_metadata(&path).map_err(error)?;
        require(
            metadata.file_type().is_file() && metadata.len() <= 262_144,
            "launch ledger must be bounded regular file before reading",
        )?;
        let bytes = fs::read(&path).map_err(error)?;
        require(
            bytes.len() <= 262_144 && (bytes.is_empty() || bytes.ends_with(b"\n")),
            "launch ledger oversized/truncated",
        )?;
        bytes
            .split(|b| *b == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice(line).map_err(error))
            .collect()
    }
    fn assert_launches(&mut self, count: usize, all_exited: bool) -> Check {
        require(
            !self.binding.run.join("launch-overflow.json").exists(),
            "launch ledger overflow",
        )?;
        let records = self.launch_records()?;
        require(
            records
                .iter()
                .all(|r| !matches!(r["kind"].as_str(), Some("refusal" | "exec_error"))),
            "launcher refusal/exec error",
        )?;
        let attempts = records
            .iter()
            .filter(|r| r["kind"] == "attempt")
            .collect::<Vec<_>>();
        let intents = records
            .iter()
            .filter(|r| r["kind"] == "pre_exec_intent")
            .collect::<Vec<_>>();
        require(
            attempts.len() == count && intents.len() == count && count <= 3,
            "unexpected helper attempt/exec-intent cardinality",
        )?;
        for (index, intent) in intents.iter().enumerate() {
            require(
                intent["phase"] == ["initial_source", "success_clone", "undo_source"][index]
                    && intent["attempt"] == (index + 1)
                    && intent["pid"] == attempts[index]["pid"]
                    && intent["start_ticks"] == attempts[index]["start_ticks"]
                    && intent["ppid"] == std::process::id()
                    && intent["pgid"] == self.launch["owner_pgid"],
                "launch phase/PID ownership differs",
            )?;
            // Initial source and strict clone must already have exited at the
            // raw audit; Undo source is allowed alive only before Redo.
            if all_exited || index < 2 {
                let pid = intent["pid"].as_u64().ok_or("helper PID missing")?;
                let stat = PathBuf::from(format!("/proc/{pid}/stat"));
                let observed = read_proc_stat_bounded(&stat);
                let expected = intent["start_ticks"]
                    .as_u64()
                    .ok_or("helper original start ticks absent")?;
                let disposition = match &observed {
                    Ok(text) => match proc_start_ticks(text) {
                        Ok(ticks) if ticks == expected => "same_identity_still_present",
                        Ok(_) => "original_identity_gone_changed_start_ticks",
                        Err(_) => "process_identity_unknown_malformed_stat",
                    },
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        "original_identity_gone_not_found"
                    }
                    Err(_) => "process_identity_unknown_read_failure",
                };
                self.journal.emit("helper_process_identity",json!({"phase":intent["phase"],"pid":pid,"expected_start_ticks":expected,"disposition":disposition}))?;
                require(
                    original_helper_gone(
                        observed.as_deref().map_err(std::io::Error::kind),
                        expected,
                    )?,
                    "exact source helper identity remains live (Joined alone is narrower)",
                )?;
            }
        }
        Ok(())
    }
    fn final_audit(&mut self) -> Check {
        require(
            self.topology.complete(),
            "all four exact owned topology slots must settle",
        )?;
        self.stage(9, Stage::Cleanup)?;
        self.hooks()?;
        self.no_source_audio_shape()?;
        require(self.budget.complete(), "fixed pump inventory incomplete")?;
        let pair = self
            .exact_paused()
            .ok_or("final exact paused identity absent")?;
        require(
            self.snapshot().paused_work_quiescent(pair.0, pair.1),
            "final callback/owner work not terminal",
        )?;
        self.verify_private_files()?;
        self.assert_launches(3, true)?;
        self.persist_all_observations()?;
        let records = self.all_observations();
        require(
            records
                .iter()
                .filter(|r| {
                    matches!(
                        r.kind,
                        RecordKind::Lifecycle("instance construction observed")
                    )
                })
                .count()
                == 3
                && records
                    .iter()
                    .filter(|r| matches!(r.kind, RecordKind::Capture { .. }))
                    .count()
                    == 2,
            "final exactly-three instances/two captures audit",
        )?;
        require(
            records.iter().all(|r| {
                !matches!(
                    &r.kind,
                    RecordKind::HookInventory {
                        inactive: false,
                        ..
                    }
                )
            }),
            "reachable test hook was active",
        )?;
        self.observer.with_snapshot(|s| {
            require(
                s.failure.is_none() && s.source_positive_attempts <= 4096 && s.clone_replies == 282,
                "final independent source/clone call ceiling",
            )
        })?;
        self.journal.emit("final_owner_inventory",json!({"snapshot":snapshot_summary(&self.snapshot()),"budgets":self.budget,"source_positive_attempts":self.observer.with_snapshot(|s|s.source_positive_attempts),"worker_calls":"ordinaryQ128, reset-origin256 and SaveState zero-sample separately retained in raw records","source_shutdown_claim":"Rust Joined and original helper process identity gone, with NotFound versus changed start ticks recorded separately; strict checked reap claimed only for clone"}))?;
        // Success teardown is intentionally empty-source only. No retained source
        // session-departure behavior or fourth instance is exercised.
        self.callback.take();
        self.app.take();
        self.persist_all_observations()?;
        self.binding.verify_pins()?;
        self.observer.set_stage(Stage::Finished).map_err(error)?;
        self.passed()
    }
    fn run(&mut self) -> Check {
        self.preflight()?;
        self.setup_source()?;
        self.audition_source_and_base()?;
        self.convert()?;
        self.audit_media_and_history()?;
        self.audio_pass(1)?;
        self.undo_source()?;
        self.redo_retirement()?;
        self.audio_pass(2)?;
        self.final_audit()
    }
    fn all_observations(&self) -> Vec<Arc<observe::Record>> {
        // Only Arc handles are copied; payloads remain singly owned by their
        // immutable records. Each collector lock copies at most64 handles.
        let count = self.observer.with_snapshot(|s| s.records.len());
        let mut records = Vec::with_capacity(count);
        let mut index = 0;
        while index < count {
            let batch = self.observer.copy_records_since(index);
            if batch.is_empty() {
                break;
            }
            index += batch.len();
            records.extend(batch);
        }
        records
    }
    fn persist_all_observations(&mut self) -> Check {
        let count = self.observer.with_snapshot(|s| s.records.len());
        while self.dumped_records < count {
            let before = self.dumped_records;
            self.persist_new_observations()?;
            require(
                self.dumped_records > before,
                "observer persistence cursor did not advance",
            )?;
        }
        Ok(())
    }
    fn cleanup_failure(&mut self) -> Check {
        self.observer.set_stage(Stage::Cleanup).map_err(error)?;
        let deadline = (Instant::now() + Duration::from_secs(8)).min(self.outer);
        if self.app.is_none() {
            return Ok(());
        }
        if self.accepting_job.is_some() {
            require(
                self.app().clip_render_job.is_none(),
                "two conflicting retained review owners",
            )?;
            self.app.as_mut().unwrap().clip_render_job = self.accepting_job.take();
        }
        self.app_mut().cancel_clip_render_job();
        loop {
            require(Instant::now() < deadline, "failed cleanup owner deadline")?;
            // Cleanup preserves all actual terminal evidence and never applies
            // the success predicate to suppress the original failure.
            self.pace(Cadence::Poll, deadline)?;
            self.budget.poll()?;
            let ctx = self.ctx.clone();
            let app = self.app_mut();
            require(
                app.deferred_generator_rebuilds.is_empty(),
                "cleanup retained deferred rebuild intent; cannot risk another source construction",
            )?;
            app.drive_clip_session_retirement();
            app.poll_clip_restorations();
            app.poll_generator_endpoint_events();
            app.poll_plugin_runtime_events();
            app.poll_plugin_processing_health();
            app.poll_timeline_compile(&ctx);
            app.poll_audio_asset_events();
            app.poll_timeline_audio_runtime(&ctx);
            app.drive_clip_render_job();
            self.persist_all_observations()?;
            let pending = self
                .app()
                .clip_render_job
                .as_ref()
                .is_some_and(ClipRenderJob::owns_pending_resource)
                || !self.app().pending_generator_removals.is_empty();
            if !pending {
                break;
            }
            let snapshot = self.snapshot();
            if snapshot.callback_work_pending()
                || self
                    .app()
                    .audio
                    .as_ref()
                    .is_some_and(|a| snapshot.request_id != a.test_transport_request_id())
            {
                require(
                    self.budget.cleanup < CLEANUP_PUMPS && self.budget.total < MAX_PUMPS,
                    "reserved failed-cleanup pump ceiling",
                )?;
                self.pace(Cadence::Control, deadline)?;
                let app = self.app.as_ref().unwrap();
                self.callback
                    .as_mut()
                    .unwrap()
                    .pump(app.audio.as_ref().unwrap(), 128);
                self.budget.charge(Charge::FailureCleanup)?;
                let segments = self.callback.as_mut().unwrap().take_genuine_segments();
                for segment in segments {
                    self.journal.emit(
                        "failure_cleanup_segment",
                        json!({"segment":format!("{segment:?}"),"before":topology::segment_state_evidence(segment.before),"after":topology::segment_state_evidence(segment.after),"qualification":"failed_cleanup_only"}),
                    )?;
                }
                for receipt in self
                    .callback
                    .as_mut()
                    .unwrap()
                    .take_genuine_lifecycle_receipts()
                {
                    self.journal.emit("failure_cleanup_lifecycle_receipt",json!({"qualification":"failed_cleanup_only","receipt":topology::receipt_evidence(receipt)}))?;
                }
            }
        }
        // No ambiguous queued callback removal is discarded. After that gate,
        // releasing the callback disconnects endpoint ownership before shutdown.
        for receipt in self
            .callback
            .as_mut()
            .unwrap()
            .take_genuine_lifecycle_receipts()
        {
            self.journal.emit("failure_cleanup_lifecycle_receipt",json!({"qualification":"failed_cleanup_only","receipt":topology::receipt_evidence(receipt)}))?;
        }
        self.callback.take();
        loop {
            require(
                Instant::now() < deadline,
                "exact legacy source JoinHandle remains Pending",
            )?;
            let mut pending = false;
            let app = self.app_mut();
            for source in app.running_generator_chains.values_mut() {
                pending |= poll_guard(&mut source.guard)?;
            }
            for source in app.pending_generator_chains.values_mut() {
                pending |= poll_guard(&mut source.guard)?;
            }
            if let Some(source) = app
                .clip_render_job
                .as_mut()
                .and_then(|j| j.retired_source.as_mut())
            {
                pending |= poll_guard(&mut source.guard)?;
            }
            for guard in &mut app.retired_clip_restorations {
                pending |= poll_guard(guard)?;
            }
            for restoration in app.pending_clip_restorations.values_mut() {
                pending |= !restoration.poll_test_shutdown_retained();
            }
            require(
                app.running_insert_chains.is_empty()
                    && app.pending_insert_chains.is_empty()
                    && app.clip_session_retirement.is_none(),
                "unexpected cleanup owner outside declared source fixture",
            )?;
            if !pending {
                break;
            }
            self.pace(Cadence::Poll, deadline)?;
        }
        self.persist_all_observations()?;
        self.app.take();
        Ok(())
    }
    fn summary(&mut self, result: &Check, cleanup: &Check) -> Check {
        if let Err(reason) = result
            && let Some(index) = self.active_stage.take()
        {
            self.stage_results[index] = "failed";
            self.journal.emit(
                "stage_failed",
                json!({"stage":STAGES[index],"error":reason}),
            )?;
        }
        self.journal.emit("cleanup_result",json!({"terminal":cleanup.is_ok(),"error":cleanup.as_ref().err(),"quarantined":cleanup.is_err()}))?;
        self.journal.emit("summary",json!({"success":result.is_ok()&&cleanup.is_ok()&&self.stage_results.iter().all(|s|*s=="completed"),"stages":STAGES.iter().zip(self.stage_results).map(|(stage,status)|json!({"stage":stage,"status":status})).collect::<Vec<_>>(),"error":result.as_ref().err().or_else(||cleanup.as_ref().err()),"budgets":self.budget}))
    }
}
fn poll_guard(guard: &mut PluginWorkerGuard) -> Check<bool> {
    match guard.poll_shutdown_retained() {
        RetainedShutdownPoll::Pending => Ok(true),
        RetainedShutdownPoll::Joined => Ok(false),
        RetainedShutdownPoll::WorkerPanicked => Err("exact source worker joined with panic".into()),
    }
}
thread_local! {
    static QUARANTINED: std::cell::RefCell<Option<std::mem::ManuallyDrop<GenuineFixture>>> = const {std::cell::RefCell::new(None)};
}

#[test]
#[ignore = "manual genuine App qualification: exact run binding, final root once-only approval and owned-group watchdog required"]
fn genuine_selected_clip_app_conversion_undo_redo() {
    // All guards precede App source construction. A failed invocation cannot be
    // retried in this process, and ordinary discovery never executes this body.
    let entered = Instant::now();
    let (binding, token) = RunBinding::preflight().unwrap_or_else(|reason| {
        panic!("genuine App admission refused before any source load: {reason}")
    });
    assert!(
        !ADMITTED.swap(true, Ordering::AcqRel),
        "once-only genuine invocation already admitted"
    );
    let mut owner =
        std::mem::ManuallyDrop::new(GenuineFixture::new(binding, token, entered).unwrap_or_else(
            |reason| panic!("genuine App pre-source owner construction failed: {reason}"),
        ));
    let result = catch_unwind(AssertUnwindSafe(|| owner.run()))
        .unwrap_or_else(|_| Err("genuine App body unwound; owners retained outside catch".into()));
    let cleanup = if result.is_err() {
        catch_unwind(AssertUnwindSafe(|| owner.cleanup_failure()))
            .unwrap_or_else(|_| Err("failed cleanup unwound; exact owners quarantined".into()))
    } else {
        Ok(())
    };
    let summary = catch_unwind(AssertUnwindSafe(|| owner.summary(&result, &cleanup)))
        .unwrap_or_else(|_| Err("summary serialization unwound; original owners retained".into()));
    let failure = result
        .as_ref()
        .err()
        .or_else(|| cleanup.as_ref().err())
        .or_else(|| summary.as_ref().err())
        .cloned();
    if cleanup.is_err() {
        QUARANTINED.with(|slot| {
            let mut slot = slot.borrow_mut();
            assert!(slot.is_none(), "once-only quarantine occupied");
            *slot = Some(owner);
        });
        // Keep exact owners retained, with no detached Pending guard Drop. The
        // already verified external group watchdog owns failed-process removal.
        loop {
            std::thread::park_timeout(Duration::from_secs(60));
        }
    }
    drop(std::mem::ManuallyDrop::into_inner(owner));
    if let Some(reason) = failure {
        panic!("genuine App qualification failed: {reason}");
    }
}
