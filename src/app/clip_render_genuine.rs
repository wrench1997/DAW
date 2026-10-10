//! Ignored, explicit-run-only qualification of the actual selected-clip App path.
//!
//! SOURCE PREPARATION ONLY. Ordinary builds/discovery cannot run this test. The
//! independent IEEE/WAV oracle retains every imported body and its22 tests; only
//! the obsolete standalone Gate test-module registration is removed. Original
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
#[path = "clip_render_genuine_oracles.rs"]
mod oracles;
#[path = "clip_render_genuine_support.rs"]
mod support;
use support::*;
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
            bytes.len() <= 256 * 1024 && self.bytes + bytes.len() + 1 <= 16 * 1024 * 1024,
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
    p.mixer_tracks.truncate(2);
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
    for (i, track) in p.mixer_tracks.iter_mut().enumerate() {
        track.id = if i == 0 { MASTER_MIXER_TRACK_ID } else { 1 };
        track.runtime_slot = i as u8;
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
    committed_project: Option<Vec<u8>>,
    authoritative_before: Option<Vec<u8>>,
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
        self.active_stage = Some(index);
        self.stage_results[index] = "incomplete";
        self.observer.set_stage(observer).map_err(error)?;
        self.journal
            .emit("stage_begin", json!({"index":index,"stage":STAGES[index]}))
    }
    fn passed(&mut self) -> Check {
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
    fn healthy(&self) -> Check {
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
        let app = self.app();
        require(
            !app.plugin_processing_retry_required
                && !app.clip_restoration_failed
                && app.timeline_compile_error.is_none()
                && app.timeline_audio_sync.last_error.is_none(),
            "actual App processing/restoration/Timeline fault",
        )?;
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
        if let Some(job) = &app.clip_render_job {
            if matches!(
                job.phase,
                ClipRenderPhase::Capturing
                    | ClipRenderPhase::RenderingDisposing
                    | ClipRenderPhase::VerifyingMedia
                    | ClipRenderPhase::RecapturingSource
            ) {
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
        }
        let snapshot = self.snapshot();
        require(
            snapshot.fault.is_none()
                && !snapshot.suspended
                && snapshot.timeline_execution_failures == 0
                && snapshot.timeline_missing_assets == 0,
            "actual callback fault",
        )?;
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
        self.observer.with_snapshot(|s| {
            let mut ordinary_states = 0usize;
            for r in &s.records { match &r.kind {
                RecordKind::Call{boundary:Boundary::Returned,outcome:Some(CallOutcome::Error(_) | CallOutcome::StrictError(_)),..} => return Err("actual host operation failed".into()),
                RecordKind::RuntimeEvent{event,..} if matches!(event,RuntimeEvent::SlotFault{..}|RuntimeEvent::StateRejected{..}|RuntimeEvent::ParameterCommandFailed{..}) => return Err("actual runtime event reports failure".into()),
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
        })
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
    fn poll(&mut self, reason: &str) -> Check {
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
        self.persist_new_observations()?;
        self.healthy()
    }
    fn pump(&mut self, cadence: Cadence) -> Check<(GenuineSegmentObservation, Vec<f32>)> {
        require(
            self.budget.total < MAX_PUMPS,
            "total pump ceiling before callback",
        )?;
        self.pace(cadence, self.outer)?;
        let app = self.app.as_ref().unwrap();
        let callback = self.callback.as_mut().unwrap();
        callback.pump(app.audio.as_ref().unwrap(), 128);
        let segments = callback.take_genuine_segments();
        let output = callback
            .last_output()
            .iter()
            .flat_map(|x| x.iter().copied())
            .collect::<Vec<_>>();
        // Persist actual raw segment stamps before interpreting acceptance.
        for s in &segments {
            self.journal.emit("callback_segment",json!({"pump":s.pump_ordinal,"segment":s.segment_ordinal,"stage":self.active_stage,"start":s.start_frame,"frames":s.frames,"device_offset":s.device_offset,"device_frame":s.device_frame,"playing":s.playing,"timeline_ready":s.timeline_ready,"generation":s.generation,"active_epoch":s.active_epoch,"epoch":s.epoch,"request_id":s.request_id,"loop_count":s.loop_count,"loop_enabled":s.loop_enabled,"loop_start":s.loop_start_frame,"loop_end":s.loop_end_frame,"disposition":format!("{:?}",s.disposition),"before":format!("{:?}",s.before),"after":format!("{:?}",s.after)}))?;
        }
        require(
            segments.len() == 1,
            "pump did not produce exactly one accounted Q128 segment",
        )?;
        let segment = segments[0];
        require(
            segment.frames == 128 && segment.device_offset == 0 && output.len() == 256,
            "Q128 output extent mismatch",
        )?;
        require(
            segment.disposition == GenuineSegmentDisposition::Rendered
                && segment.before.fault.is_none()
                && segment.after.fault.is_none()
                && !segment.before.suspended
                && !segment.after.suspended
                && !segment.before.needs_resync
                && !segment.after.needs_resync,
            "callback segment fault/resync",
        )?;
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
            let (segment, _) = self.pump(Cadence::Control)?;
            self.budget.charge(Charge::Control(control))?;
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
        while !condition(self) {
            require(
                Instant::now() < deadline,
                &format!("bounded wait failed: {reason}"),
            )?;
            self.control_tick(control, reason)?;
        }
        self.healthy()
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
                    json!({"type":"app_generator_event","event":format!("{event:?}")})
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
        self.journal.emit("declaration",json!({"stages":STAGES,"selector":SELECTOR,"binding_sha256":self.binding.binding_hash,"ui_boundary":"headless real review plus exact shared begin_clip_render_stop acceptance method; no pointer/native-window claim","control_ceilings":CONTROL_CEILINGS,"playing_segments":PLAY_SEGMENTS,"stopped_segments":[32,32,32],"cleanup_reserve":CLEANUP_PUMPS,"outer_seconds":120,"original_imported_oracle_sha256":"1bff180934231ca40cf92c079038f45b4dddfdd2ad29a6df9216174b12a0d8b2","adapted_oracle_source_sha256":"a02d3b59af625bdc68bc992753bbfd6683b4913b2f44bd8413240b7e722d4ca2","oracle_adaptation":"removed standalone test-only Gate module registration; all22 numerical/WAV/SHA test bodies unchanged"}))?;
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
        app.sync_mixer_to_audio();
        app.request_immediate_timeline_compile();
        // Picker opt-in is the existing production admission. No manually built
        // RunningGeneratorChain, mock source or injected readiness is involved.
        app.plugin_picker_fresh_bounce = true;
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
        self.until(0, 30, "source SlotReady/install/current Timeline", |f| {
            let app = f.app();
            let channel = app.project.channels[0].id;
            f.exact_paused().is_some()
                && app
                    .running_generator_chains
                    .get(&channel)
                    .is_some_and(|source| {
                        source.control.native_editor_snapshot(0).is_some_and(|s| {
                            s.capture_serial > 0
                                && !s.capture_in_progress
                                && s.pending_request.is_none()
                        })
                    })
                && app.clip_render_observe(channel).is_ok()
        })?;
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
            "exact initial request0 State missing after native startup capture settled",
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
        require(
            s.nodes.len() == 2
                && s.nodes
                    .iter()
                    .any(|n| n.id == MASTER_MIXER_TRACK_ID && n.runtime_slot == 0)
                && s.nodes.iter().any(|n| n.id == 1 && n.runtime_slot == 1)
                && s.routes.len() == 1,
            "actual two-node route shape mismatch",
        )?;
        let route = &s.routes[0];
        require(
            route.source_id == 1
                && route.destination_id == MASTER_MIXER_TRACK_ID
                && route.source_runtime_slot == 1
                && route.destination_runtime_slot == 0
                && route.tap == MixerRouteTap::PostFader
                && route.gain.to_bits() == 1.0_f32.to_bits(),
            "actual unity source-to-master route mismatch",
        )?;
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
        let before = self
            .exact_paused()
            .ok_or("Play lacks confirmed frame-zero paused identity")?;
        if pass > 0 {
            self.no_source_audio_shape()?;
        }
        let count = PLAY_SEGMENTS[pass];
        let mut output = Vec::with_capacity(count * 256);
        let mut active = None;
        let baseline = self.snapshot();
        let loop_count = baseline.loop_count;
        self.app_mut().toggle_play();
        require(self.app().playing, "actual App Play refused")?;
        let deadline = (Instant::now() + Duration::from_secs(30)).min(self.outer);
        while self.budget.playing[pass] < count {
            require(Instant::now() < deadline, "fixed playing pass deadline")?;
            self.poll("Play activation / fixed musical output")?;
            if active.is_none() {
                require(
                    self.budget.controls[control] < CONTROL_CEILINGS[control],
                    "Play activation control ceiling",
                )?;
            }
            let (segment, samples) = self.pump(if active.is_some() {
                Cadence::Measured
            } else {
                Cadence::Control
            })?;
            if !segment.playing {
                require(active.is_none(), "playing pass unexpectedly paused")?;
                self.budget.charge(Charge::Control(control))?;
                continue;
            }
            // The activation segment is first measured output and is charged
            // once here, never as both a control and musical segment.
            let index = self.budget.playing[pass];
            self.budget.charge(Charge::Playing(pass))?;
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
            }
            for state in [segment.before, segment.after] {
                require(
                    state.fixed_quantum_bridge_gaps == baseline.fixed_quantum_bridge_gaps
                        && state.fixed_quantum_output_underflow_frames
                            == baseline.fixed_quantum_output_underflow_frames
                        && state.timeline_execution_failures == 0
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
        let mut identity = None;
        let mut loop_count = None;
        let deadline = (Instant::now() + Duration::from_secs(15)).min(self.outer);
        while self.budget.stopped[pass] < 32 {
            require(
                Instant::now() < deadline,
                "actual Stop activation / fixed silence deadline",
            )?;
            self.poll("Stop activation / fixed32 stopped output")?;
            if identity.is_none() {
                require(
                    self.budget.controls[control] < CONTROL_CEILINGS[control],
                    "Stop activation control ceiling",
                )?;
            }
            let (segment, samples) = self.pump(if identity.is_some() {
                Cadence::Measured
            } else {
                Cadence::Control
            })?;
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
                self.budget.charge(Charge::Control(control))?;
                continue;
            }
            self.budget.charge(Charge::Stopped(pass))?;
            if let Some(pair) = identity {
                require(
                    pair == (segment.generation, segment.epoch),
                    "stopped window identity changed",
                )?;
            } else {
                identity = Some((segment.generation, segment.epoch));
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
        require(
            s.paused_work_quiescent(pair.0.ok_or("paused generation")?, pair.1),
            "source work remains at end of fixed32 stop window",
        )?;
        self.journal.emit("fixed_stop_window",json!({"pass":pass,"segments":32,"frames":4096,"generation":pair.0,"epoch":pair.1,"snapshot":snapshot_summary(&s)}))?;
        self.poll("consume exact Stop acknowledgment")?;
        require(
            self.exact_paused() == Some((pair.0.unwrap(), pair.1)),
            "actual exact Stop acknowledgment missing",
        )
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
        self.app_mut().open_selected_clip_render_review();
        let directory = self.binding.output_directory.to_string_lossy().into_owned();
        let job = self
            .app_mut()
            .clip_render_job
            .as_mut()
            .ok_or("actual selected review refused")?;
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
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            self.app_mut().clip_render_review_ui(ctx)
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
    json!({"engine":s.engine_session,"project":s.project_session,"generation":s.generation,"active_epoch":s.active_epoch,"epoch":s.epoch,"frame":s.start_frame,"playing":s.playing,"loop_count":s.loop_count,"pending_audio":s.pending_audio_commands,"pending_timeline":s.pending_timeline_commands,"pending_activation":s.pending_timeline_activation,"endpoints":s.endpoints.iter().map(|e|json!({"id":e.endpoint_id,"instance":e.plugin_instance_id,"mixer_track":e.mixer_track,"fixed":format!("{:?}",e.fixed),"exact":format!("{:?}",e.exact_snapshot),"latency":format!("{:?}",e.coherent_latency)})).collect::<Vec<_>>(),"assets":s.assets.iter().map(|a|json!({"id":a.id,"rate":a.sample_rate,"channels":a.channels,"frames":a.frames,"arc_address":Arc::as_ptr(&a.samples) as *const f32 as usize})).collect::<Vec<_>>(),"native_voices":s.native_voices.len(),"audio_voices":s.audio_voices.len(),"zero_pdc":s.zero_pdc,"pdc_revision":s.pdc_revision,"graph_identity":s.graph_identity,"graph_pdc":format!("{:?}",s.graph_pdc),"raw_delays":format!("{:?}",s.raw_delays),"raw_delay_active":s.raw_delay_active,"legacy_pdc_active":s.legacy_pdc_active,"route_delays":format!("{:?}",s.route_delays),"master_gain":s.master_gain,"master_pan":s.master_pan,"track1_gain":s.track_gains[1],"track1_pan":s.track_pans[1]})
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
                        for index in 0..128 {
                            generated.push(channels[0][index]);
                            generated.push(channels[1][index]);
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
        self.permit("undo_source")?;
        self.app_mut().undo();
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
        self.app_mut().redo();
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
                        json!({"segment":format!("{segment:?}")}),
                    )?;
                }
            }
        }
        // No ambiguous queued callback removal is discarded. After that gate,
        // releasing the callback disconnects endpoint ownership before shutdown.
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
        if let Err(reason) = result {
            if let Some(index) = self.active_stage.take() {
                self.stage_results[index] = "failed";
                self.journal.emit(
                    "stage_failed",
                    json!({"stage":STAGES[index],"error":reason}),
                )?;
            }
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
