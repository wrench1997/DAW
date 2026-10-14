//! Explicit, device-free Windows qualification of the actual App bounce path.
//! Register as a child of clip_render_job, gated by test/windows/fresh-helper.
//! Run a private copy of the test executable with the real helper beside it.
//! This fixture never opens a native window or claims physical monitoring.

use super::*;
use crate::{
    genuine_app_observer::{
        self as observe, Boundary, CaptureLabel, InstancePhase, ObservationRun, Operation,
        RecordKind, Stage, WorkerJoinOutcome,
    },
    model::{PianoNote, PluginFormat as ModelPluginFormat, PluginRole, PluginRuntimeStatus},
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    mem::ManuallyDrop,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, atomic::AtomicBool},
};

// Reuse the independent SHA256, RIFF parser and dynamic Q128 trim oracle.
// Its fixed Surge geometry validators are intentionally not used for NOIRE.
#[allow(dead_code)]
#[path = "clip_render_genuine_oracles.rs"]
mod oracles;

#[path = "windows_noire_capture_probe.rs"]
mod windows_noire_capture_probe;

#[path = "windows_noire_whole_song_qa.rs"]
mod windows_noire_whole_song_qa;

// Dormant complete input foundation; no source/render owner is constructed.
#[allow(dead_code)]
#[path = "windows_noire_full_inputs.rs"]
mod windows_noire_full_inputs;

// Complete-project owner foundation; the native stream driver remains separate.
#[allow(dead_code)]
#[path = "windows_noire_full_owner.rs"]
mod windows_noire_full_owner;

type Check<T = ()> = Result<T, String>;
const SELECTOR: &str = "app::clip_render_job::windows_noire_bounce_qa::windows_noire_pure_source_capture_strict_clone_conversion_undo_redo";
const OPT_IN: &str = "actual-noire-pure-source-capture-strict-clone-app-v1";
const SOURCE_SHA256: &str = "90923c6384ff5bb3df7df2a6d31b0d7eea016e496a2588aaf2ed06e947cc3c23";
const KONTAKT_UID: &str = "5653544E694B386B6F6E74616B742038";
const RATE: usize = 48_000;
const TAIL_SECONDS: f64 = 3.0;
const CLONE_REPLY_LIMIT: usize = 2_048;
const MAX_PUMPS: usize = 12_000;
const RESTORATION_DIAGNOSTIC_LIMIT: usize = 4;
const CAPTURE_DIAGNOSTIC_LIMIT: usize = 4;
const METADATA_DIAGNOSTIC_BUS_LIMIT: usize = 32;
const RAW_CAPTURE_WRITE_LIMIT: usize = observe::MAX_PRIVATE_BYTES;
const OPAQUE_DIFF_RANGE_LIMIT: usize = 4;
const SOURCE_PROCESS_TRACE_LIMIT: usize = 64;
static ADMITTED: AtomicBool = AtomicBool::new(false);

/// Measured PCM facts are deliberately separate from the pass predicate: a
/// truncated/quiet render still leaves its complete WAV and measured receipt.
struct PcmMetrics {
    frames: usize,
    peak: f32,
    rms: f64,
    nonzero_frames: usize,
    clipping_samples: usize,
    first_nonzero_frame: Option<usize>,
    last_nonzero_frame: Option<usize>,
    maximum_internal_zero_run: usize,
    maximum_adjacent_channel_delta: f32,
    tail_nonzero_frames: usize,
    last_100ms_peak: f32,
    tail_end_threshold: f32,
}
impl PcmMetrics {
    fn measure(retained: &[f32], musical_frames: usize, tail_frames: usize) -> Check<Self> {
        let frames = musical_frames
            .checked_add(tail_frames)
            .ok_or("PCM frame count overflow")?;
        require(
            musical_frames > 0
                && tail_frames >= RATE * 3
                && frames.checked_mul(2) == Some(retained.len()),
            "PCM audit requires complete stereo music plus at least three seconds of retained tail",
        )?;
        require(
            retained.iter().all(|sample| sample.is_finite()),
            "retained PCM contains nonfinite samples",
        )?;
        let peak = retained
            .iter()
            .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
        let rms = (retained
            .iter()
            .map(|sample| f64::from(*sample).powi(2))
            .sum::<f64>()
            / retained.len() as f64)
            .sqrt();
        let mut nonzero_frames = 0;
        let mut first_nonzero_frame = None;
        let mut last_nonzero_frame = None;
        let mut maximum_internal_zero_run = 0;
        let mut zero_run = 0;
        let mut maximum_adjacent_channel_delta = 0.0_f32;
        for (frame_index, frame) in retained.as_chunks::<2>().0.iter().enumerate() {
            if frame.iter().any(|sample| *sample != 0.0) {
                nonzero_frames += 1;
                if last_nonzero_frame.is_some() {
                    maximum_internal_zero_run = maximum_internal_zero_run.max(zero_run);
                }
                first_nonzero_frame.get_or_insert(frame_index);
                last_nonzero_frame = Some(frame_index);
                zero_run = 0;
            } else {
                zero_run += 1;
            }
            if frame_index > 0 {
                for (channel, sample) in frame.iter().enumerate() {
                    maximum_adjacent_channel_delta = maximum_adjacent_channel_delta
                        .max((*sample - retained[(frame_index - 1) * 2 + channel]).abs());
                }
            }
        }
        let tail = &retained[musical_frames * 2..];
        let last = &retained[retained.len() - RATE / 10 * 2..];
        Ok(Self {
            frames,
            peak,
            rms,
            nonzero_frames,
            clipping_samples: retained.iter().filter(|sample| sample.abs() >= 1.0).count(),
            first_nonzero_frame,
            last_nonzero_frame,
            maximum_internal_zero_run,
            maximum_adjacent_channel_delta,
            tail_nonzero_frames: tail
                .as_chunks::<2>()
                .0
                .iter()
                .filter(|frame| frame.iter().any(|sample| *sample != 0.0))
                .count(),
            last_100ms_peak: last
                .iter()
                .fold(0.0_f32, |peak, sample| peak.max(sample.abs())),
            tail_end_threshold: 1.0e-6_f32.max(peak * 1.0e-4),
        })
    }
    fn qualify(&self) -> Check {
        require(
            self.peak > 1.0e-7 && self.rms > 1.0e-8 && self.nonzero_frames > 50,
            "actual NOIRE clone PCM is silent or insufficiently audible",
        )?;
        require(
            self.clipping_samples == 0,
            "actual NOIRE clone PCM reaches or exceeds full scale",
        )?;
        require(
            self.tail_nonzero_frames > 0,
            "fixed tail contains no actual instrument release PCM",
        )?;
        require(
            self.last_100ms_peak <= self.tail_end_threshold,
            "actual NOIRE release remains at the fixed tail limit; audio is retained but qualification fails as truncated",
        )
    }
    fn evidence(&self) -> Value {
        json!({"frames":self.frames,"nonzero_frames":self.nonzero_frames,"sample_peak":self.peak,"rms":self.rms,
            "clipping_samples":self.clipping_samples,"first_nonzero_frame":self.first_nonzero_frame,
            "last_nonzero_frame":self.last_nonzero_frame,"maximum_internal_zero_run_frames":self.maximum_internal_zero_run,
            "maximum_adjacent_channel_delta":self.maximum_adjacent_channel_delta,
            "tail_nonzero_frames":self.tail_nonzero_frames,"last_100ms_peak":self.last_100ms_peak,
            "tail_end_threshold":self.tail_end_threshold,
            "fixed_tail_limit_reached_pcm":self.last_100ms_peak > self.tail_end_threshold,
            "physical_listening":false,"full_natural_tail_guarantee":false,
            "adjacent_delta_is_measurement_not_listening_or_click_free_guarantee":true})
    }
}

#[derive(Default)]
struct TerminalFacts {
    workers: BTreeSet<usize>,
    joins: BTreeMap<usize, Vec<WorkerJoinOutcome>>,
    source_instances: Vec<(InstancePhase, Option<usize>, Option<u64>)>,
    clone_instances: usize,
    clone_reaps: Vec<(u64, bool)>,
    clone_terminal_disposals: Vec<u64>,
    clone_destructors: Vec<u64>,
    render_destructions: Vec<u64>,
    hooks_inactive: bool,
}
impl TerminalFacts {
    fn from_records(records: &[Arc<observe::Record>]) -> Self {
        let mut facts = Self {
            hooks_inactive: true,
            ..Self::default()
        };
        for record in records {
            match &record.kind {
                RecordKind::Lifecycle("instance construction observed") => {
                    if let Some(instance) = record.instance {
                        if instance.phase == InstancePhase::StrictClone {
                            facts.clone_instances += 1;
                        } else {
                            facts.source_instances.push((
                                instance.phase,
                                instance.worker_id,
                                instance.source_instance_id,
                            ));
                            if let Some(worker) = instance.worker_id {
                                facts.workers.insert(worker);
                            }
                        }
                    }
                }
                RecordKind::Worker { worker_id, .. } => {
                    facts.workers.insert(*worker_id);
                }
                RecordKind::WorkerJoin { worker_id, outcome } => {
                    facts.joins.entry(*worker_id).or_default().push(*outcome);
                }
                RecordKind::HookInventory { inactive, .. } => {
                    facts.hooks_inactive &= *inactive;
                }
                RecordKind::Lifecycle("renderer backend destruction returned") => {
                    facts.render_destructions.push(record.ordinal)
                }
                _ => {}
            }
            if record
                .instance
                .is_some_and(|instance| instance.phase == InstancePhase::StrictClone)
            {
                match &record.kind {
                    RecordKind::Lifecycle("strict Client Reaped success") => {
                        facts.clone_reaps.push((record.ordinal, true))
                    }
                    RecordKind::Lifecycle("strict Client Reaped abnormal") => {
                        facts.clone_reaps.push((record.ordinal, false))
                    }
                    RecordKind::Disposal {
                        terminated: true, ..
                    } => facts.clone_terminal_disposals.push(record.ordinal),
                    RecordKind::Lifecycle(
                        "StrictBackend destructor entered; fields still owned",
                    ) => facts.clone_destructors.push(record.ordinal),
                    _ => {}
                }
            }
        }
        facts
    }
    fn qualify(&self, require_success: bool) -> Check {
        require(
            self.hooks_inactive,
            "actual qualification enabled a behavior-changing test hook",
        )?;
        require(
            self.source_instances.len() <= 2
                && self.source_instances.iter().all(|(_, worker, instance)| {
                    worker.is_some_and(|worker| worker != 0)
                        && instance.is_some_and(|instance| instance != 0)
                }),
            "source construction lacks its actual worker or source instance identity",
        )?;
        require(
            self.workers.iter().all(|worker| {
                self.joins
                    .get(worker)
                    .is_some_and(|outcomes| outcomes.as_slice() == [WorkerJoinOutcome::Joined])
            }),
            "an actual source worker lacks exactly one successful JoinHandle completion",
        )?;
        require(
            self.joins
                .keys()
                .all(|worker| self.workers.contains(worker)),
            "worker join evidence is not bound to an observed source worker",
        )?;
        require(
            self.clone_instances <= 1,
            "strict clone owner was constructed more than once",
        )?;
        if self.clone_instances == 1 {
            require(
                self.clone_reaps.len() == 1
                    && self.clone_terminal_disposals.len() == 1
                    && self.clone_destructors.len() == 1
                    && self.render_destructions.len() == 1,
                "actual strict clone cleanup lacks one concrete Client Reaped, terminal disposal, and renderer destruction",
            )?;
            let (reap, successful) = self.clone_reaps[0];
            require(
                reap < self.clone_terminal_disposals[0]
                    && self.clone_terminal_disposals[0] < self.clone_destructors[0]
                    && self.clone_destructors[0] < self.render_destructions[0],
                "strict clone destruction preceded its actual concrete Reaped acknowledgment",
            )?;
            require(
                !require_success || successful,
                "strict clone was concretely reaped with abnormal exit status",
            )?;
        }
        if require_success {
            require(
                self.clone_instances == 1
                    && self.source_instances.len() == 2
                    && self.workers.len() == 2
                    && self.source_instances[0].0 == InstancePhase::InitialSource
                    && self.source_instances[1].0 == InstancePhase::UndoSource
                    && self.source_instances[0].1 != self.source_instances[1].1,
                "successful conversion/Undo lacks distinct actual initial-source and restoration workers",
            )?;
        }
        Ok(())
    }
    fn evidence(&self) -> Value {
        json!({"actual_source_workers":self.workers,
            "source_instances":self.source_instances.iter().map(|(phase,worker,instance)|json!({"phase":format!("{phase:?}"),"worker":worker,"source_instance":instance})).collect::<Vec<_>>(),
            "joins":self.joins.iter().map(|(worker,outcomes)|json!({"worker":worker,"outcomes":outcomes.iter().map(|outcome|format!("{outcome:?}")).collect::<Vec<_>>()})).collect::<Vec<_>>(),
            "strict_clone_instances":self.clone_instances,"strict_clone_reaped":self.clone_reaps,
            "strict_terminal_disposal_ordinals":self.clone_terminal_disposals,
            "strict_destructor_ordinals":self.clone_destructors,"renderer_destruction_ordinals":self.render_destructions,
            "behavior_changing_hooks_inactive":self.hooks_inactive,
            "source_worker_join_is_not_independent_source_helper_reap_evidence":true})
    }
}

fn error(value: impl std::fmt::Display) -> String {
    value.to_string().chars().take(2_048).collect()
}
fn panic_error(payload: Box<dyn std::any::Any + Send>, stage: &str) -> String {
    let detail = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("non-text panic payload");
    error(format!(
        "{stage} unwound: {detail}; retained plugin owners are not dropped by the catch boundary"
    ))
}
fn require(ok: bool, reason: &str) -> Check {
    if ok { Ok(()) } else { Err(error(reason)) }
}
fn new_write(path: &Path, bytes: &[u8]) -> Check {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(error)?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(error)
}
fn new_json(path: &Path, value: &Value) -> Check {
    let bytes = serde_json::to_vec_pretty(value).map_err(error)?;
    require(
        bytes.len() <= 1024 * 1024,
        "bounded JSON receipt exceeded 1 MiB",
    )?;
    new_write(path, &bytes)
}
fn read_bounded(path: &Path, maximum: u64) -> Check<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).map_err(error)?;
    require(
        metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() <= maximum,
        "input is not a bounded regular file",
    )?;
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(error)?
        .take(maximum + 1)
        .read_to_end(&mut bytes)
        .map_err(error)?;
    require(
        bytes.len() as u64 <= maximum,
        "bounded input grew during read",
    )?;
    Ok(bytes)
}
fn absolute_env(name: &str) -> Check<PathBuf> {
    let path = PathBuf::from(std::env::var_os(name).ok_or_else(|| format!("set {name}"))?);
    require(path.is_absolute(), "QA inputs must use absolute paths")?;
    Ok(path)
}
fn state_envelope(state: &[u8]) -> Check<Value> {
    require(
        state.len() >= 28 && state.starts_with(b"VST3HOST_STATE\0\0"),
        "saved Project state is not a complete VST3HOST_STATE envelope",
    )?;
    let word = |offset| u32::from_le_bytes(state[offset..offset + 4].try_into().unwrap());
    let version = word(16);
    let component = word(20);
    let controller = word(24);
    let controller_bytes = if controller == u32::MAX {
        0
    } else {
        controller
    };
    let bytes = u64::from(component) + u64::from(controller_bytes);
    require(
        version == 1
            && component > 0
            && bytes <= 64 * 1024 * 1024
            && bytes + 28 == state.len() as u64,
        "saved state envelope geometry is invalid",
    )?;
    Ok(json!({"version":version,"component_bytes":component,
        "controller_bytes":if controller == u32::MAX { None } else { Some(controller) },
        "bytes":state.len(),"sha256":oracles::sha256_hex(state)}))
}

fn scratch(saved: &Project) -> Check<(Project, PluginDescriptor)> {
    let sources: Vec<_> = saved
        .plugin_instances
        .iter()
        .filter(|instance| instance.uid == KONTAKT_UID)
        .collect();
    require(
        sources.len() == 1,
        "saved Project must identify exactly one Kontakt class",
    )?;
    let mut instance = sources[0].clone();
    require(
        instance.format == ModelPluginFormat::Vst3 && instance.role == PluginRole::Instrument,
        "saved source is not the actual Kontakt VST3 instrument",
    )?;
    state_envelope(&instance.opaque_state)?;
    require(
        instance.parameters.len() <= 4096
            && instance
                .parameters
                .values()
                .all(|value| value.is_finite() && (0.0..=1.0).contains(value)),
        "saved parameter bases are invalid",
    )?;
    let channels: Vec<_> = saved
        .channels
        .iter()
        .filter(|channel| channel.instrument_plugin_instance_id == Some(instance.id))
        .collect();
    require(channels.len() == 1, "saved Kontakt assignment is ambiguous")?;
    let mut project = Project::blank();
    project.name = "Private Windows NOIRE Pure strict bounce qualification".into();
    project.tempo = saved.tempo;
    project.swing = 0.0;
    project.song_length_beats = 16.0;
    project.channels[0].volume = channels[0].volume;
    project.channels[0].pan = 0.0;
    project.channels[0].muted = false;
    project.channels[0].solo = false;
    project.channels[0].instrument_plugin_instance_id = Some(instance.id);
    project.channels[0].mixer_track = project
        .mixer_track_id_at_runtime_slot(1)
        .ok_or("scratch insert is absent")?;
    for track in &mut project.mixer_tracks {
        track.volume = 1.0;
        track.pan = 0.0;
        track.muted = false;
        track.solo = false;
    }
    instance.enabled = true;
    instance.bypass = false;
    instance.wet = 1.0;
    instance.midi_ports = Default::default();
    instance.runtime_status = PluginRuntimeStatus::Unloaded;
    // The fresh intent and later candidate load must share the production
    // persistable descriptor identity. A QA label is not that identity; keep
    // the saved historical UID/path byte-for-byte and bind factory metadata
    // separately instead of bypassing the fresh-source guard.
    let mut descriptor = plugins::plugin_load_identity_from_instance(&instance).descriptor;
    descriptor.vst3_metadata = Some(plugins::Vst3ScanMetadata {
        class_uid: KONTAKT_UID.into(),
        category: "Instrument".into(),
        has_midi_input: true,
        has_midi_output: true,
    });
    project.plugin_instances.push(instance);
    project.patterns[0].length_steps = 16;
    project.patterns[0].channel_steps = vec![[false; 16]];
    project.patterns[0].notes = vec![PianoNote {
        id: 1,
        channel_id: Some(project.channels[0].id),
        group_id: None,
        note: 60,
        start: 0.0,
        length: 1.0,
        velocity: 0.2,
        selected: false,
        muted: false,
    }];
    let mut clip = saved
        .clips
        .iter()
        .find(|clip| clip.kind == ClipKind::Pattern)
        .cloned()
        .ok_or("saved pattern placement is absent")?;
    clip.id = 1;
    clip.track = 0;
    clip.start = 2.0;
    clip.length = 2.0;
    clip.source_offset = 0.0;
    clip.pattern_id = project.patterns[0].id;
    clip.name = "NOIRE Pure C4 actual dry bounce".into();
    clip.group_id = None;
    clip.audio_asset_id = None;
    clip.automation_id = None;
    clip.audio_source_offset_frame = None;
    clip.audio_source_reference = None;
    clip.audio_length_reference = None;
    clip.fade_in_reference = None;
    clip.fade_out_reference = None;
    clip.fade_in = 0.0;
    clip.fade_out = 0.0;
    clip.gain = 1.0;
    clip.muted = false;
    project.clips = vec![clip];
    project.automation_lanes.clear();
    project.piano_notes.clear();
    project.mixer_insert_slots.clear();
    project.audio_assets.clear();
    project.audio_clip_mixer_destinations.clear();
    require(
        project.plugin_instances[0].opaque_state == sources[0].opaque_state
            && project.plugin_instances[0].parameters == sources[0].parameters,
        "scratch preparation changed saved state or parameter bases",
    )?;
    Ok((project, descriptor))
}

struct Inputs {
    directory: PathBuf,
    project: PathBuf,
    helper: PathBuf,
    executable: PathBuf,
    project_hash: String,
    helper_hash: String,
    executable_hash: String,
}
impl Inputs {
    fn load(directory: PathBuf) -> Check<(Self, Project, PluginDescriptor, Value)> {
        let project = absolute_env("CITRUS_NOIRE_BOUNCE_QA_PROJECT")?
            .canonicalize()
            .map_err(error)?;
        let helper = absolute_env("CITRUS_NOIRE_BOUNCE_QA_HELPER")?
            .canonicalize()
            .map_err(error)?;
        let executable = std::env::current_exe()
            .map_err(error)?
            .canonicalize()
            .map_err(error)?;
        require(
            helper
                == executable
                    .parent()
                    .ok_or("test executable parent missing")?
                    .join("vst3-host-helper.exe")
                    .canonicalize()
                    .map_err(error)?,
            "HELPER must be the actual production helper beside the private test executable",
        )?;
        let project_bytes = read_bounded(&project, 2 * 1024 * 1024)?;
        let project_hash = oracles::sha256_hex(&project_bytes);
        require(
            project_hash == SOURCE_SHA256,
            "PROJECT must be the previously cold-verified native13 normal Save",
        )?;
        let saved = Project::load(&project).map_err(error)?;
        let (prepared, descriptor) = scratch(&saved)?;
        require(descriptor.path.exists(), "actual Kontakt bundle is absent")?;
        let helper_hash = oracles::sha256_hex(&read_bounded(&helper, 64 * 1024 * 1024)?);
        let executable_hash = oracles::sha256_hex(&read_bounded(&executable, 256 * 1024 * 1024)?);
        let evidence = json!({"schema":1,"fixture":"actual-noire-pure-source-capture-strict-clone-app-v1",
            "project":{"path":project,"bytes":project_bytes.len(),"sha256":project_hash,"encoding":"Project hex via production Project::load"},
            "helper":{"path":helper,"sha256":helper_hash},"test_executable":{"path":executable,"sha256":executable_hash},
            "instrument":{"class_uid":KONTAKT_UID,"expected_version":"8.4.1","external_preset_identity":"NOIRE Pure previously viewed in native13/14; opaque bytes alone cannot name a preset", "saved_state":state_envelope(&prepared.plugin_instances[0].opaque_state)?,
                "persisted_generic_parameter_count":prepared.plugin_instances[0].parameters.len(),
                "parameter_qualification_scope":"Only the captured writable generic base set; this does not prove every NI-internal parameter is restored"},
            "observer_limits":{"clone_replies":CLONE_REPLY_LIMIT,"records":observe::MAX_RECORDS,"private_bytes":observe::MAX_PRIVATE_BYTES},
            "scope":"actual plugin source/capture/strict clone and headless production App conversion; no audio device/native pointer/listening claim"});
        Ok((
            Self {
                directory,
                project,
                helper,
                executable,
                project_hash,
                helper_hash,
                executable_hash,
            },
            prepared,
            descriptor,
            evidence,
        ))
    }
    fn unchanged(&self) -> Check {
        for (path, maximum, expected) in [
            (&self.project, 2 * 1024 * 1024, &self.project_hash),
            (&self.helper, 64 * 1024 * 1024, &self.helper_hash),
            (&self.executable, 256 * 1024 * 1024, &self.executable_hash),
        ] {
            require(
                oracles::sha256_hex(&read_bounded(path, maximum)?) == *expected,
                "an original Project or pinned executable changed during qualification",
            )?;
        }
        Ok(())
    }
}

struct Owner {
    app: Option<Box<CitrusApp>>,
    callback: Option<audio::TestAudioCallback>,
    accepting_job: Option<ClipRenderJob>,
    ctx: egui::Context,
    observer: ObservationRun,
    inputs: Inputs,
    evidence: Value,
    deadline: Instant,
    pumps: usize,
    callback_segments: usize,
    lifecycle_receipts: usize,
    attempted_output_path: Option<PathBuf>,
}
impl Owner {
    fn new(inputs: Inputs, evidence: Value) -> Check<Self> {
        let observer =
            ObservationRun::install_with_clone_reply_limit(CLONE_REPLY_LIMIT).map_err(error)?;
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = CitrusApp::new_boxed_with_services(&cc, false);
        let (engine, mut callback) = AudioEngine::test_engine_with_callback();
        let token = [0x4e; 32];
        callback.enable_genuine_observation(token).map_err(error)?;
        app.audio = Some(engine);
        Ok(Self {
            app: Some(app),
            callback: Some(callback),
            accepting_job: None,
            ctx,
            observer,
            inputs,
            evidence,
            deadline: Instant::now() + Duration::from_secs(280),
            pumps: 0,
            callback_segments: 0,
            lifecycle_receipts: 0,
            attempted_output_path: None,
        })
    }
    fn app(&self) -> &CitrusApp {
        self.app.as_deref().expect("retained App")
    }
    fn app_mut(&mut self) -> &mut CitrusApp {
        self.app.as_deref_mut().expect("retained App")
    }
    /// Borrow each still-retained restoration once. A missing candidate/chain
    /// after transfer remains None; these diagnostics never revive an owner,
    /// query a plugin, pump a callback, or restart the restoration deadline.
    fn restoration_diagnostics(&self, moment: &str) -> Value {
        let app = self.app();
        let now = Instant::now();
        let mut rows = app
            .pending_clip_restorations
            .iter()
            .take(RESTORATION_DIAGNOSTIC_LIMIT)
            .map(|(channel, restoration)| {
                let candidate_fingerprint = restoration
                    .genuine_test_candidate()
                    .map(project_fingerprint);
                let audio_worker = restoration.genuine_test_chain_audio_worker();
                let ready = restoration.genuine_diagnostic_ready();
                let age_seconds = now
                    .checked_duration_since(restoration.genuine_diagnostic_started_at())
                    .map(|duration| duration.as_secs_f64());
                let chain_workers = restoration.genuine_diagnostic_chain_workers();
                let transferred_workers = restoration.genuine_diagnostic_transferred_workers();
                (
                    *channel,
                    json!({"channel":channel,"candidate_fingerprint":candidate_fingerprint,
                    "chain_audio_worker":audio_worker,"ready":ready,"age_seconds":age_seconds,
                    "chain_control_guard_workers":chain_workers,
                    "transferred_control_guard_workers":transferred_workers}),
                )
            })
            .collect::<Vec<_>>();
        rows.sort_unstable_by_key(|(channel, _)| *channel);
        json!({"moment":moment,"read_only":true,"pending_count":app.pending_clip_restorations.len(),
            "row_limit":RESTORATION_DIAGNOSTIC_LIMIT,"truncated":app.pending_clip_restorations.len() > rows.len(),
            "pending":rows.into_iter().map(|(_,row)|row).collect::<Vec<_>>(),
            "current_project_fingerprint":project_fingerprint(&app.project),
            "desired_timeline_generation":app.timeline_desired_generation,
            "restoration_playback_pending":app.clip_restoration_playback_pending,
            "restoration_failed":app.clip_restoration_failed,
            "pending_generator_count":app.pending_generator_chains.len(),
            "running_generator_count":app.running_generator_chains.len(),
            "pending_removal_count":app.pending_generator_removals.len(),
            "retired_restoration_guard_count":app.retired_clip_restorations.len()})
    }
    fn tick(&mut self, deadline: Instant, qualify: bool) -> Check {
        require(
            Instant::now() < deadline && self.pumps < MAX_PUMPS,
            "qualification poll/pump deadline or bound exceeded",
        )?;
        if !qualify {
            require(
                self.app().deferred_generator_rebuilds.is_empty(),
                "cleanup retained deferred source construction intent; another source must not be constructed",
            )?;
        }
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
        if qualify {
            if let Some(reason) = self
                .app()
                .clip_render_job
                .as_ref()
                .and_then(|job| job.failure.as_ref())
            {
                return Err(error(reason));
            }
            require(
                !self.app().clip_restoration_failed,
                "actual App restoration failed",
            )?;
            self.observer.with_snapshot(|snapshot| {
                require(
                    snapshot.failure.is_none(),
                    snapshot.failure.unwrap_or("observer failure"),
                )
            })?;
        }
        let callback = self.callback.as_mut().ok_or("callback owner absent")?;
        let app = self.app.as_ref().unwrap();
        let snapshot = callback.genuine_snapshot();
        let engine = app.audio.as_ref().ok_or("audio engine owner absent")?;
        if snapshot.callback_work_pending()
            || snapshot.request_id != engine.test_transport_request_id()
        {
            callback.pump(engine, 128);
            self.pumps += 1;
            self.callback_segments += callback.take_genuine_segments().len();
            self.lifecycle_receipts += callback.take_genuine_lifecycle_receipts().len();
        }
        std::thread::sleep(Duration::from_millis(8));
        Ok(())
    }
    fn wait(&mut self, seconds: u64, predicate: impl Fn(&Self) -> bool) -> Check {
        let deadline = self
            .deadline
            .min(Instant::now() + Duration::from_secs(seconds));
        loop {
            self.observer.with_snapshot(|snapshot| {
                require(
                    snapshot.failure.is_none(),
                    snapshot.failure.unwrap_or("observer failure"),
                )
            })?;
            if predicate(self) {
                return Ok(());
            }
            self.tick(deadline, true)?;
        }
    }
    fn paused(&self) -> bool {
        let app = self.app();
        let Some((generation, epoch)) = app
            .audio
            .as_ref()
            .and_then(AudioEngine::confirmed_timeline_identity)
        else {
            return false;
        };
        let snapshot = self.callback.as_ref().unwrap().genuine_snapshot();
        app.timeline_audio_matches_desired()
            && generation == app.timeline_desired_generation
            && !app.playing
            && !snapshot.playing
            && snapshot.generation == Some(generation)
            && snapshot.active_epoch == Some(epoch)
            && snapshot.epoch == epoch
            && snapshot.paused_work_quiescent(generation, epoch)
    }
    fn initialize_source(&mut self, candidate: Project, descriptor: PluginDescriptor) -> Check {
        let channel = self.begin_source_initialization(candidate, descriptor)?;
        self.wait(40, |owner| {
            owner.app().pending_generator_chains.is_empty()
                && owner.app().running_generator_chains.len() == 1
                && owner.paused()
                && owner.app().clip_render_observe(channel).is_ok()
        })?;
        self.finish_source_initialization()
    }

    // Shared only by the explicit QA fixtures. The original occurrence wrapper
    // retains its exact admission predicate and operation order above.
    fn begin_source_initialization(
        &mut self,
        candidate: Project,
        descriptor: PluginDescriptor,
    ) -> Check<u32> {
        self.observer
            .set_stage(Stage::InitialSource)
            .map_err(error)?;
        new_write(
            &self.inputs.directory.join("scratch-input.citrus"),
            &serde_json::to_vec(&candidate).map_err(error)?,
        )?;
        let channel = candidate.channels[0].id;
        let instance = candidate.plugin_instances[0].id;
        let mut empty = candidate.clone();
        empty.channels[0].instrument_plugin_instance_id = None;
        empty.plugin_instances.clear();
        let app = self.app_mut();
        app.project = empty;
        app.transport_mode = TransportMode::Song;
        app.selected_clip = Some(1);
        app.playlist_selection_ids = HashSet::from([1]);
        app.history_snapshot = app.project.clone();
        app.history_fingerprint = project_fingerprint(&app.project);
        app.project_fingerprint = app.history_fingerprint;
        app.undo_stack.clear();
        app.redo_stack.clear();
        require(
            app.audio
                .as_mut()
                .unwrap()
                .set_project_session(app.project_session),
            "actual project-session command rejected",
        )?;
        app.rebuild_tempo_map();
        app.request_immediate_timeline_compile();
        // Every mixer command is admitted individually and serviced by the real
        // test callback. No queue-full flag or endpoint acknowledgment is forged.
        let commands = mixer_control_commands(&app.project);
        for command in commands {
            require(
                self.app_mut().audio.as_mut().unwrap().command(command),
                "individual production mixer command admission failed",
            )?;
            self.tick(self.deadline, true)?;
        }
        let previous = snapshot_channel_generator(&self.app().project, channel);
        self.app_mut()
            .record_fresh_clip_source_for_new_load(instance, &descriptor)?;
        self.app_mut()
            .queue_channel_generator_candidate(channel, candidate, previous)?;
        Ok(channel)
    }

    fn finish_source_initialization(&mut self) -> Check {
        let app = self.app_mut();
        app.undo_stack.clear();
        app.redo_stack.clear();
        app.history_snapshot = app.project.clone();
        app.history_fingerprint = project_fingerprint(&app.project);
        app.project_fingerprint = app.history_fingerprint;
        app.dirty = false;

        Ok(())
    }

    fn exercise(&mut self, candidate: Project, descriptor: PluginDescriptor) -> Check {
        self.initialize_source(candidate, descriptor)?;
        self.observer.set_stage(Stage::Conversion).map_err(error)?;
        self.app_mut().open_selected_clip_render_review();
        let mut job = self
            .app_mut()
            .clip_render_job
            .take()
            .ok_or("production occurrence review was refused")?;
        job.review.directory = self
            .inputs
            .directory
            .join("render")
            .to_string_lossy()
            .into_owned();
        job.review.tail_seconds = TAIL_SECONDS;
        job.review.tail_acknowledged = true;
        fs::create_dir(Path::new(&job.review.directory)).map_err(error)?;
        self.attempted_output_path = Some(job.output_path()?);
        self.accepting_job = Some(job);
        // Render the actual review surface. Acceptance invokes the identical
        // production method used by its button; no pointer/UI activation claim.
        self.app_mut().clip_render_job = self.accepting_job.take();
        let ctx = self.ctx.clone();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            self.app_mut().clip_render_review_ui(ui.ctx())
        });
        self.accepting_job = self.app_mut().clip_render_job.take();
        let mut job = self.accepting_job.take().ok_or("review owner was lost")?;
        let result = self.app_mut().begin_clip_render_stop(&mut job);
        self.app_mut().clip_render_job = Some(job);
        result?;
        self.wait(150, |owner| {
            owner
                .app()
                .clip_render_job
                .as_ref()
                .is_some_and(|job| job.phase == ClipRenderPhase::Complete)
        })?;
        let job = self.app().clip_render_job.as_ref().unwrap();
        require(
            job.source_removed
                && job.source_joined
                && job.retired_source.is_none()
                && job.asset_ack == Some(true)
                && !job.owns_pending_resource()
                && self.paused(),
            "conversion Complete lacks source join, asset, or exact timeline acknowledgment",
        )?;
        require(
            self.app().undo_stack.len() == 1 && self.app().redo_stack.is_empty(),
            "conversion is not one undoable transaction",
        )?;
        require(
            self.app()
                .project
                .clips
                .iter()
                .any(|clip| clip.kind == ClipKind::Pattern && clip.muted)
                && self
                    .app()
                    .project
                    .clips
                    .iter()
                    .any(|clip| clip.kind == ClipKind::Audio)
                && !self.app().project.plugin_instances[0].enabled,
            "conversion did not retain MIDI and disable its generator",
        )?;
        let before =
            clip_render::project_snapshot(self.app().undo_stack.last().unwrap()).map_err(error)?;
        let converted = clip_render::project_snapshot(&self.app().project).map_err(error)?;
        new_write(
            &self.inputs.directory.join("authoritative-before.citrus"),
            &before,
        )?;
        new_write(&self.inputs.directory.join("converted.citrus"), &converted)?;
        self.audit_pcm()?;

        self.observer.set_stage(Stage::Undo).map_err(error)?;
        let previous_endpoint = self
            .observer
            .with_snapshot(|snapshot| {
                snapshot
                    .records
                    .iter()
                    .find_map(|record| match &record.kind {
                        RecordKind::Capture { identity, .. } => Some(identity.endpoint_id),
                        _ => None,
                    })
            })
            .ok_or("actual source capture identity absent")?;
        self.app_mut().undo();
        self.wait(45, |owner| {
            !owner.app().clip_restoration_playback_pending
                && owner.app().pending_clip_restorations.is_empty()
                && owner.app().running_generator_chains.len() == 1
                && owner.paused()
        })?;
        require(
            clip_render::project_matches_snapshot(&self.app().project, &before).map_err(error)?,
            "actual Undo differs from authoritative captured source",
        )?;
        require(
            self.app()
                .running_generator_chains
                .values()
                .next()
                .unwrap()
                .endpoint_id
                != previous_endpoint,
            "Undo reused the old removed endpoint",
        )?;
        let wav_hash = oracles::sha256_hex(&read_bounded(
            self.attempted_output_path.as_ref().unwrap(),
            16 * 1024 * 1024,
        )?);
        self.observer.set_stage(Stage::Redo).map_err(error)?;
        self.app_mut().redo();
        self.wait(35, |owner| {
            !owner.app().clip_restoration_playback_pending
                && owner.app().running_generator_chains.is_empty()
                && owner.app().pending_generator_chains.is_empty()
                && owner.app().pending_generator_removals.is_empty()
                && owner.app().retired_clip_restorations.is_empty()
                && owner.paused()
        })?;
        require(
            clip_render::project_matches_snapshot(&self.app().project, &converted)
                .map_err(error)?,
            "actual Redo differs from the converted Project",
        )?;
        require(
            oracles::sha256_hex(&read_bounded(
                self.attempted_output_path.as_ref().unwrap(),
                16 * 1024 * 1024,
            )?) == wav_hash,
            "Undo/Redo changed the retained rendered WAV",
        )?;
        self.evidence["conversion"] = json!({"actual_production_entry":true,"actual_capture_required":true,
            "single_history_transaction":true,"muted_midi_retained":true,"new_undo_endpoint":true,
            "undo_authoritative_project_exact":true,"redo_converted_project_exact":true,
            "wav_preserved_after_undo_redo":true,"wav_sha256":wav_hash});
        self.inputs.unchanged()
    }

    fn audit_pcm(&mut self) -> Check {
        self.observer.with_snapshot(|snapshot| {
            require(
                snapshot.failure.is_none(),
                snapshot.failure.unwrap_or("observer failure"),
            )
        })?;
        let records = self
            .observer
            .with_snapshot(|snapshot| snapshot.records.clone());
        let captures: Vec<_> = records
            .iter()
            .filter_map(|record| match &record.kind {
                RecordKind::Capture {
                    label,
                    identity,
                    receipt_identity,
                    state,
                } => Some((*label, *identity, *receipt_identity, state)),
                _ => None,
            })
            .collect();
        require(
            captures.len() == 2
                && captures[0].0 == CaptureLabel::Initial
                && captures[1].0 == CaptureLabel::Final
                && captures[0].1 == captures[1].1
                && captures[0].2 != captures[1].2,
            "two real source capture receipts are absent or conflated",
        )?;
        let metadata = &captures[0].3.metadata;
        require(
            metadata.class_uid == KONTAKT_UID
                && metadata.version == "8.4.1"
                && metadata.uses_selected_stereo_bounce()
                && metadata.output_buses.len() == 32,
            "actual captured source class/version or complete selected-32-bus topology differs",
        )?;
        metadata.validate(RATE as u32)?;
        require(
            captures[0].3.metadata == captures[1].3.metadata
                && captures[0].3.opaque_state == captures[1].3.opaque_state
                && captures[0].3.parameter_bases == captures[1].3.parameter_bases
                && captures[0].3.native_generation == captures[1].3.native_generation
                && captures[0].3.native_dirty_revision == captures[1].3.native_dirty_revision,
            "source recapture differs from rendered state",
        )?;
        let initial_saved_hash = self.evidence["instrument"]["saved_state"]["sha256"]
            .as_str()
            .ok_or("saved source state hash absent")?;
        require(records.iter().any(|record| record.instance.is_some_and(|instance| instance.phase == InstancePhase::InitialSource)
            && matches!(&record.kind, RecordKind::StateLoad { bytes, outcome:observe::CallOutcome::Ok }
                if oracles::sha256_hex(bytes) == initial_saved_hash)),
            "actual fresh source did not acknowledge the pinned cold-verified Project state input")?;
        let clone_loads: Vec<_> = records
            .iter()
            .filter_map(|record| {
                if record
                    .instance
                    .is_some_and(|instance| instance.phase == InstancePhase::StrictClone)
                    && let RecordKind::StateLoad { bytes, outcome } = &record.kind
                {
                    Some((bytes, outcome))
                } else {
                    None
                }
            })
            .collect();
        require(
            clone_loads.len() == 1
                && matches!(clone_loads[0].1, observe::CallOutcome::Ok)
                && clone_loads[0].0 == &captures[0].3.opaque_state,
            "actual clone state-load acknowledgment is not the authoritative captured source bytes",
        )?;
        let clone_base_reads: BTreeMap<_, _> = records
            .iter()
            .filter_map(|record| {
                if record
                    .instance
                    .is_some_and(|instance| instance.phase == InstancePhase::StrictClone)
                    && let RecordKind::BaseRead {
                        site: "StrictBackend prepare readback",
                        id,
                        raw_bits: Ok(bits),
                    } = &record.kind
                {
                    Some((*id, f64::from_bits(*bits) as f32))
                } else {
                    None
                }
            })
            .collect();
        require(
            clone_base_reads == captures[0].3.parameter_bases,
            "actual clone parameter readbacks do not cover the complete captured writable base set",
        )?;
        let reports: Vec<_> = records
            .iter()
            .filter_map(|record| match &record.kind {
                RecordKind::RenderReport(report) => Some(report),
                _ => None,
            })
            .collect();
        require(
            reports.len() == 1,
            "actual render report absent or duplicated",
        )?;
        let report = reports[0];
        require(
            report.identity == captures[0].1
                && report.native_latency == metadata.latency_samples
                && report.fixed_tail == (TAIL_SECONDS * RATE as f64) as u32,
            "actual latency/tail/capture report binding differs",
        )?;
        let map = crate::tempo_map::TempoMap::from_project(
            self.app()
                .undo_stack
                .last()
                .ok_or("authoritative Project history absent")?,
            RATE as u32,
        )
        .map_err(error)?;
        let expected_origin = map.beat_to_frame(2.0).map_err(error)?;
        let expected_end = map.beat_to_frame(4.0).map_err(error)?;
        require(
            report.origin_frame == expected_origin
                && report.musical_frames == expected_end - expected_origin,
            "actual occurrence origin or full two-beat musical length differs from the authoritative placement",
        )?;
        let shape = oracles::q128_frame_oracle(
            report.musical_frames as usize,
            report.fixed_tail as usize,
            report.native_latency as usize,
        )?;
        require(
            shape.generated_frames / 128 <= CLONE_REPLY_LIMIT,
            "actual latency/tail exceeds explicit QA-only observation bound",
        )?;
        require(
            report.frames == shape.retained_frames
                && report.generated_frames == shape.generated_frames as u64
                && (1.0..=2.0).contains(&(report.musical_frames as f64 / RATE as f64)),
            "actual render duration/generated geometry differs from bounded 1-2 second music and 3 second tail",
        )?;
        let mut generated = Vec::with_capacity(shape.generated_frames * 2);
        let mut process_calls = 0;
        let mut note_on = Vec::new();
        let mut note_off = Vec::new();
        for record in &records {
            if !record
                .instance
                .is_some_and(|instance| instance.phase == InstancePhase::StrictClone)
            {
                continue;
            }
            match &record.kind {
                RecordKind::Call {
                    operation: Operation::StrictProcess,
                    boundary: Boundary::Attempt,
                    frames: Some(frames),
                    ..
                } => {
                    require(*frames == 128, "actual strict process is not Q128")?;
                    process_calls += 1;
                }
                RecordKind::StrictPcm {
                    frames,
                    channels,
                    transport_applied,
                    output_events,
                    output_events_lost,
                    ..
                } => {
                    require(
                        *frames == 128
                            && channels.len() == 2
                            && channels.iter().all(|channel| channel.len() == 128)
                            && *transport_applied
                            && *output_events == 0
                            && !*output_events_lost,
                        "strict reply shape, transport acknowledgment or MIDI loss differs",
                    )?;
                    for (left, right) in channels[0].iter().zip(&channels[1]) {
                        generated.push(*left);
                        generated.push(*right);
                    }
                }
                RecordKind::Midi { data, offset } => {
                    require(
                        *offset < 128 && data[1] == 60 && data[0] & 0x0f == 0,
                        "actual clone received an unexpected channel, pitch or Q128 MIDI offset",
                    )?;
                    match data[0] & 0xf0 {
                        0x90 if data[2] > 0 => note_on.push((process_calls, *data, *offset)),
                        0x80 | 0x90 => note_off.push((process_calls, *data, *offset)),
                        _ => return Err("actual clone received a non-note MIDI command".into()),
                    }
                }
                _ => {}
            }
        }
        require(
            process_calls == shape.generated_frames / 128
                && generated.len() == shape.generated_frames * 2,
            "observed clone processing includes missing, extra, or truncated blocks",
        )?;
        let expected_off = map.beat_to_frame(3.0).map_err(error)? - expected_origin;
        require(
            note_on == [(0, [0x90, 60, 25], 0)]
                && note_off
                    == [(
                        expected_off as usize / 128,
                        [0x80, 60, 0],
                        (expected_off % 128) as u16,
                    )],
            "actual clone MIDI ledger differs from the complete scratch C4 schedule; no new notes may enter its fixed tail",
        )?;
        require(
            generated.iter().all(|sample| sample.is_finite()),
            "generated PCM contains nonfinite samples",
        )?;
        let retained = oracles::trim_q128_stereo(
            &generated,
            report.musical_frames as usize,
            report.fixed_tail as usize,
            report.native_latency as usize,
        )?;
        let path = publication_claim(self.app().clip_render_job.as_ref(), &records)
            .map(|(path, _, _)| path.to_path_buf())
            .ok_or("actual producer publication evidence is absent")?;
        require(
            self.attempted_output_path.as_deref() == Some(path.as_path()),
            "actual producer publication path differs from the reviewed attempted output path",
        )?;
        let wav = read_bounded(&path, 16 * 1024 * 1024)?;
        let parsed = oracles::parse_float32_stereo_48k_wav(&wav)?;
        require(
            parsed.frames == report.frames
                && parsed.sample_bits.len() == retained.len()
                && parsed
                    .sample_bits
                    .iter()
                    .zip(&retained)
                    .all(|(bits, sample)| *bits == sample.to_bits()),
            "published WAV does not bitwise match actual latency-trimmed strict clone PCM",
        )?;
        let metrics = PcmMetrics::measure(
            &retained,
            report.musical_frames as usize,
            report.fixed_tail as usize,
        )?;
        let reap = records
            .iter()
            .find(|record| {
                matches!(
                    &record.kind,
                    RecordKind::Lifecycle("strict Client Reaped success")
                )
            })
            .map(|record| record.ordinal);
        let destroyed = records
            .iter()
            .find(|record| {
                matches!(
                    &record.kind,
                    RecordKind::Lifecycle("renderer backend destruction returned")
                )
            })
            .map(|record| record.ordinal);
        let publication = records
            .iter()
            .find(|record| {
                matches!(
                    &record.kind,
                    RecordKind::Lifecycle("occurrence writer publication attempt")
                )
            })
            .map(|record| record.ordinal);
        self.evidence["pcm"] = json!({"path":path,"wav_sha256":oracles::sha256_hex(&wav),
            "float32_stereo_48000":true,"actual_q128_blocks":process_calls,"generated_frames":shape.generated_frames,
            "native_latency_frames":report.native_latency,"music_frames":report.musical_frames,"fixed_tail_frames":report.fixed_tail,
            "frames":parsed.frames,"duration_seconds":parsed.frames as f64 / RATE as f64,
            "wav_equals_actual_trimmed_clone_pcm":true,"measurements":metrics.evidence(),
            "exact_c4_note_on":note_on,"exact_c4_note_off":note_off,
            "source_loaded_pinned_saved_state":true,"clone_loaded_actual_capture_state":true,
            "complete_clone_parameter_base_readbacks":clone_base_reads.len(),
            "all_ni_internal_parameters_restoration_proven":false,"source_metadata":metadata_json(metadata),
            "reaped_ordinal":reap,"destroyed_ordinal":destroyed,"publication_ordinal":publication,
            "physical_listening":false,"full_natural_tail_guarantee":false});
        new_json(
            &self.inputs.directory.join("pcm-audit.json"),
            &self.evidence["pcm"],
        )?;
        metrics.qualify()?;
        require(
            reap.zip(destroyed)
                .zip(publication)
                .is_some_and(|((reap, destroyed), publish)| {
                    reap < destroyed && destroyed < publish
                }),
            "actual strict Reaped/destruction evidence does not precede publication",
        )?;
        self.observer
            .with_snapshot(|snapshot| TerminalFacts::from_records(&snapshot.records).qualify(false))
    }

    fn cleanup(&mut self) -> Check {
        self.observer.set_stage(Stage::Cleanup).map_err(error)?;
        if let Some(job) = self.accepting_job.take() {
            self.app_mut().clip_render_job = Some(job);
        }
        self.app_mut().cancel_clip_render_job();
        let deadline = Instant::now() + Duration::from_secs(35);
        while self
            .app()
            .clip_render_job
            .as_ref()
            .is_some_and(ClipRenderJob::owns_pending_resource)
            || !self.app().pending_generator_removals.is_empty()
        {
            self.tick(deadline, false)?;
        }
        require(
            self.app().deferred_generator_rebuilds.is_empty(),
            "cleanup cannot discard a deferred source construction intent",
        )?;
        self.callback.take();
        loop {
            require(
                Instant::now() < deadline,
                "actual source/restoration worker join remains pending",
            )?;
            let app = self.app_mut();
            let mut pending = false;
            for source in app.running_generator_chains.values_mut() {
                pending |= poll_guard(&mut source.guard)?;
            }
            for source in app.pending_generator_chains.values_mut() {
                pending |= poll_guard(&mut source.guard)?;
            }
            if let Some(source) = app
                .clip_render_job
                .as_mut()
                .and_then(|job| job.retired_source.as_mut())
            {
                pending |= poll_guard(&mut source.guard)?;
            }
            for guard in &mut app.retired_clip_restorations {
                pending |= poll_guard(guard)?;
            }
            for guard in &mut app.retired_fresh_source_candidates {
                pending |= poll_guard(guard)?;
            }
            for restoration in app.pending_clip_restorations.values_mut() {
                pending |= !restoration.poll_test_shutdown_retained();
            }
            require(
                app.running_insert_chains.is_empty()
                    && app.pending_insert_chains.is_empty()
                    && app.clip_session_retirement.is_none(),
                "unexpected cleanup owner outside the scratch fixture",
            )?;
            if !pending {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        self.observer.with_snapshot(|snapshot| {
            require(
                snapshot.failure.is_none(),
                snapshot.failure.unwrap_or("observer failure"),
            )?;
            TerminalFacts::from_records(&snapshot.records).qualify(false)
        })
    }
    fn final_receipt(&mut self, body: &Check, cleanup: &Check) -> Check {
        let restoration = self.restoration_diagnostics("final_receipt_after_cleanup_attempt");
        self.evidence["retained_restoration_final"] = restoration;
        let records = self
            .observer
            .with_snapshot(|snapshot| snapshot.records.clone());
        self.evidence["actual_source_captures"] = capture_diagnostics(&records);
        // Diagnostic persistence reads the already retained capture records.
        // Failure cannot replace the production byte refusal or body predicate.
        let observer_complete = cleanup.is_ok()
            && self
                .observer
                .with_snapshot(|snapshot| snapshot.failure.is_none());
        attach_capture_diagnostics(
            &mut self.evidence,
            &self.inputs.directory,
            &records,
            observer_complete,
        );
        let output = output_diagnostics(
            self.app().clip_render_job.as_ref(),
            &records,
            self.attempted_output_path.as_deref(),
        );
        let failures: Vec<_> = records.iter().filter_map(|record| match &record.kind {
            RecordKind::Call { operation, outcome:Some(observe::CallOutcome::Error(reason)), .. } => Some(json!({"ordinal":record.ordinal,"operation":format!("{operation:?}"),"error":error(reason)})),
            RecordKind::Call { operation, outcome:Some(observe::CallOutcome::StrictError(failure)), .. } => Some(json!({"ordinal":record.ordinal,"operation":format!("{operation:?}"),"typed":backend_failure_json(failure)})),
            RecordKind::Disposal { terminated, error:Some(failure), diagnostics } => Some(json!({"ordinal":record.ordinal,"terminated":terminated,"typed":backend_failure_json(failure),"diagnostics":diagnostics.iter().take(16).map(error).collect::<Vec<_>>()})),
            _ => None,
        }).take(64).collect();
        let job = self.app().clip_render_job.as_ref();
        let typed_job = job.and_then(|job| job.failure_evidence.as_ref()).map(|failure| match failure {
            JobFailureEvidence::Render(failure) => json!({"kind":format!("{:?}",failure.kind),"primary":error(&failure.primary),
                "backend":failure.backend.as_deref().map(backend_failure_json),
                "cleanup_backend":failure.cleanup_backend.as_deref().map(backend_failure_json),
                "cleanup_grace_elapsed":failure.cleanup_grace_elapsed,
                "cleanup":failure.cleanup.iter().take(16).map(error).collect::<Vec<_>>()}),
            _ => json!({"category":"capture/publication/verification","primary":job.and_then(|job|job.failure.as_ref()).map(error)}),
        });
        let summary = self.observer.with_snapshot(|snapshot| json!({"records":snapshot.records.len(),
            "private_bytes":snapshot.private_bytes,"clone_replies":snapshot.clone_replies,
            "clone_reply_limit":snapshot.clone_reply_cap,
            "source_positive_attempts":snapshot.source_positive_attempts,"failure":snapshot.failure}));
        let joined = records
            .iter()
            .filter(|record| {
                matches!(
                    &record.kind,
                    RecordKind::WorkerJoin {
                        outcome: WorkerJoinOutcome::Joined,
                        ..
                    }
                )
            })
            .count();
        let terminal = TerminalFacts::from_records(&records);
        let terminal_result = terminal.qualify(body.is_ok());
        let restored_state = records.iter().find_map(|record| match &record.kind {
            RecordKind::Capture {
                label: CaptureLabel::Initial,
                state,
                ..
            } => Some(&state.opaque_state),
            _ => None,
        });
        let restoration_result = if body.is_ok() {
            require(restored_state.is_some_and(|expected| records.iter().any(|record|
                record.instance.is_some_and(|instance| instance.phase == InstancePhase::UndoSource)
                && matches!(&record.kind, RecordKind::StateLoad { bytes, outcome:observe::CallOutcome::Ok } if bytes == expected))),
                "actual Undo restoration did not acknowledge the authoritative captured state input")
        } else {
            Ok(())
        };
        let observer_result = self.observer.with_snapshot(|snapshot| {
            require(
                snapshot.failure.is_none(),
                snapshot.failure.unwrap_or("observer failure"),
            )
        });
        let unchanged = self.inputs.unchanged();
        self.evidence["result"] = json!({"body_passed":body.is_ok(),"body_error":body.as_ref().err(),
            "cleanup_terminal":cleanup.is_ok(),"cleanup_error":cleanup.as_ref().err(),
            "original_input_and_binaries_unchanged":unchanged.is_ok(),"pin_error":unchanged.as_ref().err(),
            "typed_job_failure":typed_job,"actual_host_failures":failures,"observer":summary,
            "terminal_ownership":terminal.evidence(),"terminal_evidence_passed":terminal_result.is_ok(),
            "terminal_evidence_error":terminal_result.as_ref().err(),
            "undo_captured_state_load_acknowledged":body.is_ok() && restoration_result.is_ok(),
            "undo_captured_state_load_error":restoration_result.as_ref().err(),
            "undo_restored_sound_not_auditioned":true,
            "actual_source_worker_join_records":joined,"pump_count":self.pumps,
            "callback_segments":self.callback_segments,"lifecycle_receipts":self.lifecycle_receipts,
            "attempted_output_path":self.attempted_output_path,"output_publication":output,
            "published_path":output["published_path"],
            "partial_files_preserved":true,
            "success":body.is_ok() && cleanup.is_ok() && unchanged.is_ok()
                && terminal_result.is_ok() && restoration_result.is_ok() && observer_result.is_ok()});
        new_json(&self.inputs.directory.join("result.json"), &self.evidence)?;
        unchanged?;
        terminal_result?;
        restoration_result?;
        observer_result
    }
}

fn poll_guard(guard: &mut PluginWorkerGuard) -> Check<bool> {
    match guard.poll_shutdown_retained() {
        RetainedShutdownPoll::Pending => Ok(true),
        RetainedShutdownPoll::Joined => Ok(false),
        RetainedShutdownPoll::WorkerPanicked => Err("actual worker joined after panic".into()),
    }
}
fn metadata_json(metadata: &crate::instrument_bounce::BouncePluginMetadata) -> Value {
    json!({"uid":error(&metadata.class_uid),"version":error(&metadata.version),"name":error(&metadata.name),"category":error(&metadata.category),
        "audio_input_bus_count":metadata.input_buses.len(),"audio_output_bus_count":metadata.output_buses.len(),
        "event_input_bus_count":metadata.event_input_buses.len(),"event_output_bus_count":metadata.event_output_buses.len(),
        "declared_audio_input_channels":metadata.input_buses.iter().try_fold(0_usize,|sum,bus|sum.checked_add(bus.channel_count)),
        "declared_audio_output_channels":metadata.output_buses.iter().try_fold(0_usize,|sum,bus|sum.checked_add(bus.channel_count)),
        "audio_inputs":metadata.input_buses.iter().take(METADATA_DIAGNOSTIC_BUS_LIMIT).map(|bus|json!([bus.channel_count,bus.is_main,bus.active])).collect::<Vec<_>>(),
        "audio_outputs":metadata.output_buses.iter().take(METADATA_DIAGNOSTIC_BUS_LIMIT).map(|bus|json!([bus.channel_count,bus.is_main,bus.active])).collect::<Vec<_>>(),
        "event_inputs":metadata.event_input_buses.iter().take(METADATA_DIAGNOSTIC_BUS_LIMIT).collect::<Vec<_>>(),
        "event_outputs":metadata.event_output_buses.iter().take(METADATA_DIAGNOSTIC_BUS_LIMIT).collect::<Vec<_>>(),
        "diagnostic_bus_row_limit":METADATA_DIAGNOSTIC_BUS_LIMIT,
        "diagnostic_lists_truncated":{"audio_inputs":metadata.input_buses.len() > METADATA_DIAGNOSTIC_BUS_LIMIT,
            "audio_outputs":metadata.output_buses.len() > METADATA_DIAGNOSTIC_BUS_LIMIT,
            "event_inputs":metadata.event_input_buses.len() > METADATA_DIAGNOSTIC_BUS_LIMIT,
            "event_outputs":metadata.event_output_buses.len() > METADATA_DIAGNOSTIC_BUS_LIMIT},
        "diagnostic_lists_do_not_authorize_plugin_or_publication":true,
        "native_latency":metadata.latency_samples,"reported_tail":metadata.tail_samples})
}

/// Hash the retained capture payload in place, outside the observer mutex.
/// JSON contains bounded metadata/scalars; it never copies opaque state bytes.
fn capture_diagnostics(records: &[Arc<observe::Record>]) -> Value {
    let total = records
        .iter()
        .filter(|record| matches!(&record.kind, RecordKind::Capture { .. }))
        .count();
    let captures = records.iter().filter_map(|record| match &record.kind {
        RecordKind::Capture { label,identity,receipt_identity,state } => Some(json!({
            "ordinal":record.ordinal,"stage":format!("{:?}",record.stage),"label":format!("{label:?}"),
            "receipt_identity":receipt_identity,"source_identity":{
                "project_session":identity.project_session,"project_fingerprint":identity.project_fingerprint,
                "timeline_revision":identity.timeline_revision,"epoch":identity.epoch,"endpoint_id":identity.endpoint_id,
                "channel_id":identity.channel_id,"instance_id":identity.instance_id},
            "metadata":metadata_json(&state.metadata),"state_bytes":state.opaque_state.len(),
            "state_sha256":oracles::sha256_hex(&state.opaque_state),"writable_generic_base_count":state.parameter_bases.len(),
            "native_generation":state.native_generation,"native_dirty_revision":state.native_dirty_revision,
            "all_ni_internal_parameters_restoration_proven":false})),
        _ => None,
    }).take(CAPTURE_DIAGNOSTIC_LIMIT).collect::<Vec<_>>();
    json!({"capture_count":total,"row_limit":CAPTURE_DIAGNOSTIC_LIMIT,"truncated":total > captures.len(),
        "captures":captures,"opaque_state_bytes_copied_to_json":false})
}

/// Only the documented host envelope is interpreted. Vendor payloads remain
/// exact opaque slices; no field is named volatile or changed for comparison.
fn capture_envelope_diagnostics(state: &[u8]) -> Check<Value> {
    state_envelope(state)?;
    let word = |offset| u32::from_le_bytes(state[offset..offset + 4].try_into().unwrap());
    let component_len = usize::try_from(word(20)).map_err(error)?;
    let component_end = 28_usize
        .checked_add(component_len)
        .ok_or("component offset overflow")?;
    let controller_len = word(24);
    let controller = if controller_len == u32::MAX {
        Value::Null
    } else {
        let controller_len = usize::try_from(controller_len).map_err(error)?;
        let end = component_end
            .checked_add(controller_len)
            .ok_or("controller offset overflow")?;
        json!({"offset":component_end,"end_exclusive":end,"bytes":controller_len,
            "sha256":oracles::sha256_hex(&state[component_end..end])})
    };
    Ok(
        json!({"known_host_envelope":true,"version":word(16),"header_bytes":28,
        "component":{"offset":28,"end_exclusive":component_end,"bytes":component_len,
            "sha256":oracles::sha256_hex(&state[28..component_end])},
        "controller":controller,"controller_absent":controller_len == u32::MAX,
        "vendor_payload_interpreted_or_canonicalized":false}),
    )
}

/// Count every differing byte, including an unmatched length suffix. Ranges
/// contain offsets only, never data excerpts, and truncation is explicit.
fn opaque_byte_diff(before: &[u8], after: &[u8]) -> Check<Value> {
    require(
        before.len() <= observe::MAX_STATE_BYTES && after.len() <= observe::MAX_STATE_BYTES,
        "opaque diff exceeds the existing observer state byte bound",
    )?;
    let mut first = None;
    let mut differing_bytes = 0_usize;
    let mut range_count = 0_usize;
    let mut ranges = Vec::new();
    let mut range_start = None;
    for offset in 0..before.len().max(after.len()) {
        let different = before.get(offset) != after.get(offset);
        if different {
            first.get_or_insert(offset);
            differing_bytes += 1;
            range_start.get_or_insert(offset);
        } else if let Some(start) = range_start.take() {
            range_count += 1;
            if ranges.len() < OPAQUE_DIFF_RANGE_LIMIT {
                ranges.push(json!({"offset":start,"end_exclusive":offset,"bytes":offset-start}));
            }
        }
    }
    if let Some(start) = range_start {
        let end = before.len().max(after.len());
        range_count += 1;
        if ranges.len() < OPAQUE_DIFF_RANGE_LIMIT {
            ranges.push(json!({"offset":start,"end_exclusive":end,"bytes":end-start}));
        }
    }
    Ok(
        json!({"before_bytes":before.len(),"after_bytes":after.len(),
        "first_differing_offset":first,"differing_byte_count":differing_bytes,
        "range_count":range_count,"range_row_limit":OPAQUE_DIFF_RANGE_LIMIT,
        "ranges_truncated":range_count > ranges.len(),"ranges":ranges,
        "comparison":"exact bytes; unmatched suffix bytes also differ",
        "opaque_bytes_copied_to_json":false,"comparison_authorizes_insert_or_restore":false}),
    )
}

struct CaptureRef<'a> {
    record: &'a observe::Record,
    label: CaptureLabel,
    identity: crate::instrument_bounce::BounceSourceIdentity,
    receipt_identity: usize,
    state: &'a crate::instrument_bounce::BouncePluginState,
}
fn capture_refs(records: &[Arc<observe::Record>]) -> impl Iterator<Item = CaptureRef<'_>> {
    records.iter().filter_map(|record| match &record.kind {
        RecordKind::Capture {
            label,
            identity,
            receipt_identity,
            state,
        } => Some(CaptureRef {
            record,
            label: *label,
            identity: *identity,
            receipt_identity: *receipt_identity,
            state,
        }),
        _ => None,
    })
}
fn capture_identity_json(identity: crate::instrument_bounce::BounceSourceIdentity) -> Value {
    json!({"project_session":identity.project_session,"project_fingerprint":identity.project_fingerprint,
        "timeline_revision":identity.timeline_revision,"epoch":identity.epoch,
        "endpoint_id":identity.endpoint_id,"channel_id":identity.channel_id,"instance_id":identity.instance_id})
}

/// This is called once at the final receipt of each newly created opt-in run.
/// The full attempted length reserves budget before I/O, including failures.
/// A path is claimed only after the actual CreateNew open succeeds.
fn retain_capture_new(
    path: &Path,
    bytes: &[u8],
    reserved: &mut usize,
    write_limit: usize,
) -> Value {
    let Some(next) = reserved.checked_add(bytes.len()) else {
        return json!({"created_path":null,"complete":false,"error":"raw write budget overflow"});
    };
    if bytes.len() > observe::MAX_STATE_BYTES || next > write_limit {
        return json!({"created_path":null,"complete":false,"error":"raw capture retention byte bound exceeded"});
    }
    *reserved = next;
    let mut file = match OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(file) => file,
        Err(reason) => {
            return json!({"created_path":null,"complete":false,"error":error(reason),
            "existing_path_not_claimed_or_replaced":true});
        }
    };
    let written = (|| -> Check {
        file.write_all(bytes).map_err(error)?;
        file.sync_all().map_err(error)?;
        file.seek(SeekFrom::Start(0)).map_err(error)?;
        // Compare the actual created file against the borrowed observer slice
        // with one bounded buffer. No second opaque-state allocation is made.
        let mut buffer = [0_u8; 4096];
        for expected in bytes.chunks(buffer.len()) {
            file.read_exact(&mut buffer[..expected.len()])
                .map_err(error)?;
            require(
                &buffer[..expected.len()] == expected,
                "created raw capture file differs from observer bytes",
            )?;
        }
        require(
            file.read(&mut buffer[..1]).map_err(error)? == 0,
            "created raw capture file has an unexpected suffix",
        )?;
        Ok(())
    })();
    let file_len = file.metadata().ok().map(|metadata| metadata.len());
    json!({"created_path":path,"complete":written.is_ok(),"created_file_bytes":file_len,
        "created_file_sha256":written.is_ok().then(||oracles::sha256_hex(bytes)),
        "sha256_follows_exact_streamed_file_byte_verification":written.is_ok(),
        "verification_buffer_bytes":4096,
        "error":written.err(),"partial_created_file_preserved":true})
}

/// Retain only existing observer-owned bytes, outside its mutex. No SDK call,
/// additional capture, Project edit, publication or restore is performed here.
fn retain_capture_bytes(directory: &Path, records: &[Arc<observe::Record>]) -> Value {
    let total = capture_refs(records).count();
    let captures = capture_refs(records)
        .take(CAPTURE_DIAGNOSTIC_LIMIT)
        .collect::<Vec<_>>();
    let mut reserved = 0_usize;
    let mut rows = Vec::new();
    let mut comparisons = Vec::new();
    for (index, capture) in captures.iter().enumerate() {
        let bytes = &capture.state.opaque_state;
        let name = format!(
            "opaque-capture-{:020}-{:?}.bin",
            capture.record.ordinal, capture.label
        );
        let retained = retain_capture_new(
            &directory.join(name),
            bytes,
            &mut reserved,
            RAW_CAPTURE_WRITE_LIMIT,
        );
        let geometry = capture_envelope_diagnostics(bytes);
        rows.push(json!({"ordinal":capture.record.ordinal,"monotonic_elapsed_ns":capture.record.monotonic_elapsed_ns,
            "label":format!("{:?}",capture.label),"receipt_identity":capture.receipt_identity,
            "source_identity":capture_identity_json(capture.identity),"state_bytes":bytes.len(),
            "state_sha256":oracles::sha256_hex(bytes),"retention":retained,
            "envelope":geometry.as_ref().ok(),"envelope_error":geometry.as_ref().err()}));
        if index > 0 {
            let first = &captures[0];
            let diff = opaque_byte_diff(&first.state.opaque_state, bytes);
            comparisons.push(json!({"before_ordinal":first.record.ordinal,"after_ordinal":capture.record.ordinal,
                "source_identity_equal":first.identity == capture.identity,
                "metadata_equal":first.state.metadata == capture.state.metadata,
                "parameter_bases_equal":first.state.parameter_bases == capture.state.parameter_bases,
                "native_revisions_equal":first.state.native_generation == capture.state.native_generation
                    && first.state.native_dirty_revision == capture.state.native_dirty_revision,
                "diff":diff.as_ref().ok(),"diff_error":diff.as_ref().err()}));
        }
    }
    let complete = total > 0
        && total == captures.len()
        && rows
            .iter()
            .all(|row| row["retention"]["complete"] == true && row["envelope_error"].is_null())
        && comparisons.iter().all(|row| row["diff_error"].is_null());
    json!({"diagnostic_complete":complete,"capture_count":total,"capture_row_limit":CAPTURE_DIAGNOSTIC_LIMIT,
        "captures_truncated":total > captures.len(),"per_state_byte_limit":observe::MAX_STATE_BYTES,
        "total_attempted_write_byte_limit":RAW_CAPTURE_WRITE_LIMIT,"attempted_write_bytes_reserved":reserved,
        "observer_private_byte_limit_unchanged":observe::MAX_PRIVATE_BYTES,"captures":rows,
        "comparisons_to_first_retained_capture":comparisons,"opaque_bytes_copied_to_json":false,
        "raw_file_comparison_does_not_authorize_insert_or_restore":true,
        "main_body_and_success_predicates_unchanged":true})
}

/// Optional stricter diagnostic policy for the separate opt-in capture probe.
/// The bounce QA uses the Value directly and keeps its original pass predicate.
/// Call this once: retrying CreateNew would intentionally reject prior files.
fn retain_capture_bytes_checked(
    directory: &Path,
    records: &[Arc<observe::Record>],
) -> Check<Value> {
    let value = retain_capture_bytes(directory, records);
    require(
        value["diagnostic_complete"] == true,
        "raw capture diagnostics incomplete; preserve created files and independent diagnostic evidence",
    )?;
    Ok(value)
}

fn attach_capture_diagnostics(
    evidence: &mut Value,
    directory: &Path,
    records: &[Arc<observe::Record>],
    observer_complete: bool,
) {
    evidence["actual_capture_raw_retention"] = catch_unwind(AssertUnwindSafe(|| {
        retain_capture_bytes(directory, records)
    }))
    .unwrap_or_else(|payload| {
        json!({"diagnostic_complete":false,
        "diagnostic_error":panic_error(payload,"capture raw retention"),
        "main_body_and_success_predicates_unchanged":true})
    });
    evidence["actual_original_source_positive_calls"] = catch_unwind(AssertUnwindSafe(|| {
        source_positive_diagnostics(records, observer_complete)
    })).unwrap_or_else(|payload| json!({"capture_window_counts_known":false,
        "between_captures_attempt_count":null,"diagnostic_error":panic_error(payload,"source positive call diagnostics")}));
}

/// Process attempts are source-specific facts, not successful DSP receipts.
/// A single original InitialSource incarnation must be bound before a count
/// between captures is reported. No epoch is invented from the capture stamp:
/// the actual Call tap's PluginTransport does not carry one.
fn source_positive_diagnostics(records: &[Arc<observe::Record>], observer_complete: bool) -> Value {
    let initial = capture_refs(records)
        .filter(|row| row.label == CaptureLabel::Initial)
        .collect::<Vec<_>>();
    let final_captures = capture_refs(records)
        .filter(|row| row.label == CaptureLabel::Final)
        .collect::<Vec<_>>();
    let capture_pair = if initial.len() == 1
        && final_captures.len() == 1
        && initial[0].record.ordinal < final_captures[0].record.ordinal
        && initial[0].identity == final_captures[0].identity
    {
        Some((initial[0].record.ordinal, final_captures[0].record.ordinal))
    } else {
        None
    };
    let original_id = if initial.len() == 1 {
        Some(initial[0].identity.instance_id)
    } else {
        None
    };
    let mut incarnations = BTreeMap::new();
    for record in records {
        if let Some(instance) = record.instance
            && instance.phase == InstancePhase::InitialSource
            && original_id.is_some()
            && instance.source_instance_id == original_id
        {
            incarnations.insert(instance.ordinal, instance);
        }
    }
    let original = if incarnations.len() == 1 {
        incarnations.values().next().copied()
    } else {
        None
    };
    let mut attempt_counts = [0_usize; 4];
    let mut returned_ok_counts = [0_usize; 4];
    let mut ordinary_attempts = 0;
    let mut reset_attempts = 0;
    let mut unbound_initial_attempts = 0;
    let mut unidentified_initial_attempts = 0;
    let mut traced_attempts = 0;
    let mut rows = Vec::new();
    for record in records {
        let Some(instance) = record
            .instance
            .filter(|instance| instance.phase == InstancePhase::InitialSource)
        else {
            continue;
        };
        let RecordKind::Call {
            operation,
            boundary,
            frames: Some(frames),
            transport,
            outcome,
        } = &record.kind
        else {
            continue;
        };
        if *frames == 0
            || !matches!(
                operation,
                Operation::OrdinaryProcess | Operation::ResetOrigin
            )
        {
            continue;
        }
        if Some(instance) != original {
            if *boundary == Boundary::Attempt {
                unbound_initial_attempts += 1;
                if instance.source_instance_id.is_none() {
                    unidentified_initial_attempts += 1;
                }
                // Keep unknown original candidates visible without adding
                // them to exact original-source counts. Distinct known IDs
                // are excluded; Clone and Undo were excluded above.
                if original_id.is_none()
                    || instance.source_instance_id.is_none()
                    || instance.source_instance_id == original_id
                {
                    traced_attempts += 1;
                    if rows.len() < SOURCE_PROCESS_TRACE_LIMIT {
                        rows.push(source_positive_trace_row(
                            record,
                            instance,
                            *operation,
                            *frames,
                            *transport,
                            "unknown_capture_boundary",
                            false,
                        ));
                    }
                }
            }
            continue;
        }
        let region = match capture_pair {
            Some((first, _)) if record.ordinal < first => 0,
            Some((first, last)) if record.ordinal > first && record.ordinal < last => 1,
            Some((_, last)) if record.ordinal > last => 2,
            _ => 3,
        };
        if *boundary == Boundary::Returned && matches!(outcome, Some(observe::CallOutcome::Ok)) {
            returned_ok_counts[region] += 1;
        }
        if *boundary != Boundary::Attempt {
            continue;
        }
        attempt_counts[region] += 1;
        traced_attempts += 1;
        if *operation == Operation::OrdinaryProcess {
            ordinary_attempts += 1;
        } else {
            reset_attempts += 1;
        }
        if rows.len() < SOURCE_PROCESS_TRACE_LIMIT {
            rows.push(source_positive_trace_row(
                record,
                instance,
                *operation,
                *frames,
                *transport,
                [
                    "before_initial",
                    "between_captures",
                    "after_final",
                    "unknown_capture_boundary",
                ][region],
                true,
            ));
        }
    }
    let attempts = attempt_counts.iter().sum::<usize>();
    let resolved = original.is_some()
        && capture_pair.is_some()
        && observer_complete
        && unidentified_initial_attempts == 0;
    json!({"initial_capture_count":initial.len(),"final_capture_count":final_captures.len(),
        "initial_capture_ordinal":(initial.len() == 1).then(||initial[0].record.ordinal),
        "final_capture_ordinal":(final_captures.len() == 1).then(||final_captures[0].record.ordinal),
        "initial_capture_identity":(initial.len() == 1).then(||capture_identity_json(initial[0].identity)),
        "final_capture_identity":(final_captures.len() == 1).then(||capture_identity_json(final_captures[0].identity)),
        "original_source_incarnations_matching_capture":incarnations.len(),"observer_records_complete":observer_complete,
        "capture_window_counts_known":resolved,"original_source_observed_attempt_count":original.map(|_|attempts),
        "ordinary_observed_attempts":ordinary_attempts,"reset_origin_observed_attempts":reset_attempts,
        "unbound_initial_source_attempts_excluded":unbound_initial_attempts,
        "unidentified_initial_source_attempts":unidentified_initial_attempts,
        "observed_attempt_counts":{"before_initial":attempt_counts[0],"between_captures":attempt_counts[1],
            "after_final":attempt_counts[2],"unknown_capture_boundary":attempt_counts[3]},
        "between_captures_attempt_count":resolved.then_some(attempt_counts[1]),
        "capture_window_attempt_counts":resolved.then_some(json!({"before_initial":attempt_counts[0],
            "between_captures":attempt_counts[1],"after_final":attempt_counts[2]})),
        "returned_ok_observed_counts":{"before_initial":returned_ok_counts[0],"between_captures":returned_ok_counts[1],
            "after_final":returned_ok_counts[2],"unknown_capture_boundary":returned_ok_counts[3]},
        "attempts_are_not_successful_process_receipts":true,"trace_row_limit":SOURCE_PROCESS_TRACE_LIMIT,
        "trace_observed_eligible_attempts":traced_attempts,
        "trace_truncated":traced_attempts > rows.len(),"trace":rows,
        "strict_clone_and_undo_source_excluded":true,"call_epoch_unobserved_not_assumed_zero":true,
        "adds_process_or_capture_requests":false})
}

fn source_positive_trace_row(
    record: &observe::Record,
    instance: observe::InstanceIdentity,
    operation: Operation,
    frames: usize,
    transport: Option<crate::plugins::plugin_runtime::PluginTransport>,
    relation: &str,
    source_bound: bool,
) -> Value {
    json!({"ordinal":record.ordinal,"monotonic_elapsed_ns":record.monotonic_elapsed_ns,
        "site":format!("{operation:?}"),"boundary":"Attempt","frames":frames,
        "source":{"phase":"InitialSource","instance_ordinal":instance.ordinal,
            "worker_id":instance.worker_id,"source_instance_id":instance.source_instance_id},
        "source_bound_to_single_original_incarnation":source_bound,
        "epoch":null,"epoch_observed_at_call_tap":false,
        "playing":transport.map(|value|value.playing),
        "sample_position":transport.map(|value|value.sample_position),"capture_relation":relation})
}

/// Return only a genuine producer/verification path. In the successful App
/// path, the publication seal has moved into retained PCM; the exact Complete
/// render record plus the job's receipt-derived retained path remains evidence.
/// A guessed review path or a filesystem Exists result is never a substitute.
fn publication_claim<'a>(
    job: Option<&'a ClipRenderJob>,
    records: &[Arc<observe::Record>],
) -> Option<(&'a Path, &'static str, bool)> {
    let job = job?;
    if let Some(verified) = job.verified.as_ref() {
        return Some((
            verified.path(),
            "retained VerifiedOccurrenceMedia receipt",
            true,
        ));
    }
    if let Some(published) = job.publication.as_ref() {
        return Some((
            published.path(),
            "retained PublishedOccurrenceRender receipt",
            false,
        ));
    }
    match job.failure_evidence.as_ref() {
        Some(JobFailureEvidence::Verification(failure)) => {
            return Some((
                failure.published.path(),
                "published producer receipt retained after verification failure",
                false,
            ));
        }
        Some(JobFailureEvidence::VerificationWorker(retained)) => {
            return Some((
                &retained.path,
                "producer-derived verification worker failure evidence; no insertion authority",
                false,
            ));
        }
        _ => {}
    }
    let initial_captures: Vec<_> = records
        .iter()
        .filter_map(|record| match &record.kind {
            RecordKind::Capture {
                label: CaptureLabel::Initial,
                identity,
                ..
            } => Some(*identity),
            _ => None,
        })
        .take(2)
        .collect();
    if initial_captures.len() != 1 {
        return None;
    }
    let matching_reports = records
        .iter()
        .filter(|record| {
            matches!(&record.kind,RecordKind::RenderReport(report)
        if report.identity == initial_captures[0])
        })
        .take(2)
        .count();
    if matching_reports != 1 {
        return None;
    }
    job.retained_path.as_deref().map(|path| {
        (
            path,
            "actual completed producer outcome and App receipt-derived retained path",
            job.phase == ClipRenderPhase::Complete,
        )
    })
}

fn output_diagnostics(
    job: Option<&ClipRenderJob>,
    records: &[Arc<observe::Record>],
    attempted: Option<&Path>,
) -> Value {
    let claim = publication_claim(job, records);
    let failed_link = job.and_then(|job| match job.failure_evidence.as_ref() {
        Some(JobFailureEvidence::Publication(failure)) => failure
            .writer
            .as_ref()
            .and_then(|writer| writer.evidence.retained_destination.as_deref()),
        _ => None,
    });
    json!({"attempted_output_path":attempted,"published_path":claim.map(|(path,_,_)|path),
        "publication_evidence":claim.map(|(_,evidence,_)|evidence),
        "app_media_verified":claim.is_some_and(|(_,_,verified)|verified),
        "retained_destination_from_failed_publication_evidence":failed_link,
        "filesystem_existence_is_not_ownership_evidence":true,
        "publication_or_app_verification_is_not_audibility_or_physical_listening_proof":true})
}
fn backend_failure_json(failure: &crate::instrument_bounce::render::BackendFailure) -> Value {
    json!({"kind":format!("{:?}",failure.kind),"delivery":format!("{:?}",failure.delivery),
        "strict_kind":format!("{:?}",failure.strict_kind),
        "strict_diagnostic_unavailable":format!("{:?}",failure.strict_diagnostic_unavailable),
        "strict_stage":failure.strict_stage,
        "last_wire_request":failure.last_wire_request.map(|request|format!("{request:?}")),
        "limit_evidence":failure.limit_evidence.map(|evidence|json!({
            "site":format!("{:?}",evidence.site),
            "observed_at_least":evidence.observed_at_least,"maximum":evidence.maximum,
            "complete_catalog_or_field_length_not_inferred":true})),
        "diagnostic":error(failure.diagnostic())})
}

thread_local! {
    static QUARANTINED: std::cell::RefCell<Option<ManuallyDrop<Owner>>> = const { std::cell::RefCell::new(None) };
}

#[test]
#[ignore = "actual local NOIRE Pure Windows source/capture/strict clone/App qualification; explicit private env and root-only launch required"]
fn windows_noire_pure_source_capture_strict_clone_conversion_undo_redo() {
    require(
        std::env::var("CITRUS_NOIRE_BOUNCE_QA_OPT_IN").as_deref() == Ok(OPT_IN),
        "explicit NOIRE bounce opt-in is absent",
    )
    .unwrap();
    let args: Vec<_> = std::env::args().skip(1).collect();
    require(
        args == [
            SELECTOR,
            "--ignored",
            "--exact",
            "--test-threads=1",
            "--nocapture",
        ],
        "only the exact once-only ignored selector is admitted",
    )
    .unwrap();
    require(
        !ADMITTED.swap(true, Ordering::AcqRel),
        "actual NOIRE fixture already admitted in this process",
    )
    .unwrap();
    let directory = absolute_env("CITRUS_NOIRE_BOUNCE_QA_DIR").unwrap();
    fs::create_dir(&directory)
        .expect("QA_DIR must be a new private directory; never overwrite evidence");
    let loaded = catch_unwind(AssertUnwindSafe(|| Inputs::load(directory.clone())))
        .unwrap_or_else(|payload| Err(panic_error(payload, "preflight")));
    let (inputs, project, descriptor, evidence) = match loaded {
        Ok(loaded) => loaded,
        Err(reason) => {
            new_json(&directory.join("result.json"), &json!({"schema":1,"stage":"preflight","success":false,"error":reason,"plugin_started":false,"partial_files_preserved":true})).unwrap();
            panic!(
                "NOIRE bounce preflight failed; preserve {}",
                directory.display()
            );
        }
    };
    let initialized = catch_unwind(AssertUnwindSafe(|| Owner::new(inputs, evidence)))
        .unwrap_or_else(|payload| {
            Err(panic_error(
                payload,
                "owner construction before any plugin admission",
            ))
        });
    let mut owner = match initialized {
        Ok(owner) => ManuallyDrop::new(owner),
        Err(reason) => {
            new_json(&directory.join("result.json"), &json!({"schema":1,"stage":"owner-construction","success":false,"error":reason,"plugin_started":false})).unwrap();
            panic!(
                "NOIRE bounce owner construction failed; preserve {}",
                directory.display()
            );
        }
    };
    let body = catch_unwind(AssertUnwindSafe(|| owner.exercise(project, descriptor)))
        .unwrap_or_else(|payload| Err(panic_error(payload, "actual App qualification")));
    let restoration = owner.restoration_diagnostics("after_body_before_cleanup");
    owner.evidence["retained_restoration_before_cleanup"] = restoration;
    let cleanup = catch_unwind(AssertUnwindSafe(|| owner.cleanup())).unwrap_or_else(|payload| {
        Err(panic_error(
            payload,
            "actual cleanup; ownership quarantined",
        ))
    });
    let receipt = catch_unwind(AssertUnwindSafe(|| owner.final_receipt(&body, &cleanup)))
        .unwrap_or_else(|payload| {
            let reason = panic_error(payload,"final evidence writer; prior evidence files preserved");
            let _ = new_json(&directory.join("result-writer-failure.json"), &json!({"schema":1,"success":false,
                "stage":"final-receipt","error":reason,"body_error":body.as_ref().err(),
                "cleanup_error":cleanup.as_ref().err(),"owners_quarantined":cleanup.is_err(),"partial_files_preserved":true}));
            Err(reason)
        });
    let success = body.is_ok() && cleanup.is_ok() && receipt.is_ok();
    if cleanup.is_ok() {
        // Every actual owner has acknowledged termination before this Drop.
        unsafe {
            ManuallyDrop::drop(&mut owner);
        }
    } else {
        QUARANTINED.with(|quarantine| *quarantine.borrow_mut() = Some(owner));
    }
    assert!(
        success,
        "actual NOIRE bounce qualification failed; preserve {}",
        directory.display()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn state_envelope_rejects_raw_truncated_and_inconsistent_saved_input() {
        for bytes in [Vec::new(), vec![0; 28], b"VST3HOST_STATE\0\0".to_vec()] {
            assert!(state_envelope(&bytes).is_err());
        }
        let mut state = b"VST3HOST_STATE\0\0".to_vec();
        state.extend_from_slice(&1_u32.to_le_bytes());
        state.extend_from_slice(&1_u32.to_le_bytes());
        state.extend_from_slice(&u32::MAX.to_le_bytes());
        state.push(7);
        assert_eq!(state_envelope(&state).unwrap()["component_bytes"], 1);
        state.pop();
        assert!(state_envelope(&state).is_err());
    }

    #[test]
    fn scratch_fresh_intent_matches_production_saved_descriptor_identity_without_path_rewriting() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = CitrusApp::new_boxed_with_services(&cc, false);
        app.project = Project::blank();
        for (index, path) in [
            r"C:\QA fixture\Kontakt 8.vst3",
            r"\\?\C:\QA fixture\Kontakt 8.vst3",
            "C:/QA fixture/Kontakt 8.vst3",
        ]
        .into_iter()
        .enumerate()
        {
            // A synthetic state envelope exercises only fixture preparation
            // and real intent registration. No plugin/helper is constructed.
            let mut state = b"VST3HOST_STATE\0\0".to_vec();
            state.extend_from_slice(&1_u32.to_le_bytes());
            state.extend_from_slice(&1_u32.to_le_bytes());
            state.extend_from_slice(&u32::MAX.to_le_bytes());
            state.push(7);
            let instance = crate::model::PluginInstance {
                id: 601 + index as u64,
                format: ModelPluginFormat::Vst3,
                role: PluginRole::Instrument,
                path: PathBuf::from(path),
                uid: KONTAKT_UID.into(),
                vendor: "Native Instruments fixture".into(),
                name: "Kontakt fixture".into(),
                enabled: true,
                bypass: false,
                wet: 1.0,
                parameters: BTreeMap::new(),
                opaque_state: state,
                runtime_status: PluginRuntimeStatus::Loaded,
                midi_ports: Default::default(),
            };
            let mut saved = Project::default();
            for channel in &mut saved.channels {
                channel.instrument_plugin_instance_id = None;
            }
            saved.channels[0].instrument_plugin_instance_id = Some(instance.id);
            saved.plugin_instances = vec![instance];
            let original = serde_json::to_vec(&saved).unwrap();
            let (candidate, descriptor) = scratch(&saved).unwrap();
            let candidate_instance = &candidate.plugin_instances[0];
            let production_spec = plugins::plugin_load_spec_from_instance(candidate_instance);
            app.record_fresh_clip_source_for_new_load(candidate_instance.id, &descriptor)
                .unwrap();
            let (intent_descriptor, intent_path, intent_class) =
                &app.fresh_bounce_sources[&candidate_instance.id];
            assert_eq!(intent_descriptor, &production_spec.descriptor.id);
            assert_eq!(intent_path, &production_spec.descriptor.path);
            assert_eq!(intent_class.as_str(), KONTAKT_UID);
            assert_eq!(descriptor.id, saved.plugin_instances[0].uid);
            assert_eq!(descriptor.path, saved.plugin_instances[0].path);
            assert_eq!(descriptor.format, production_spec.descriptor.format);
            assert_eq!(descriptor.category, production_spec.descriptor.category);
            assert_eq!(
                descriptor.is_instrument,
                production_spec.descriptor.is_instrument
            );
            assert_eq!(
                production_spec.initial_state,
                saved.plugin_instances[0].opaque_state
            );
            assert_eq!(
                candidate_instance.parameters,
                saved.plugin_instances[0].parameters
            );
            assert_eq!(serde_json::to_vec(&saved).unwrap(), original);
            assert!(app.running_generator_chains.is_empty());
            assert!(app.pending_generator_chains.is_empty());
        }
    }

    fn diagnostic_metadata(outputs: usize) -> crate::instrument_bounce::BouncePluginMetadata {
        use crate::instrument_bounce::{BounceAudioBus, BouncePluginMetadata};
        BouncePluginMetadata {
            class_uid: KONTAKT_UID.into(),
            version: "8.4.1".into(),
            name: "metadata-only fixture".into(),
            category: "Instrument".into(),
            event_input_buses: vec![(16, true, Some(true))],
            event_output_buses: vec![(16, true, Some(true))],
            input_buses: vec![BounceAudioBus::new(2, true, Some(true))],
            output_buses: (0..outputs)
                .map(|index| BounceAudioBus::new(2, true, Some(index == 0)))
                .collect(),
            latency_samples: 257,
            tail_samples: u32::MAX,
        }
    }

    #[test]
    fn metadata_diagnostics_preserve_32_declared_outputs_and_one_midi_bus_without_old_bound_projection()
     {
        let metadata = diagnostic_metadata(32);
        metadata.validate(RATE as u32).unwrap();
        let value = metadata_json(&metadata);
        assert_eq!(value["audio_output_bus_count"], 32);
        assert_eq!(value["declared_audio_output_channels"], 64);
        assert_eq!(value["audio_outputs"].as_array().unwrap().len(), 32);
        assert_eq!(value["event_output_bus_count"], 1);
        assert_eq!(value["event_outputs"], json!([[16, true, true]]));
        assert_eq!(value["audio_outputs"][0], json!([2, true, true]));
        assert_eq!(value["audio_outputs"][31], json!([2, true, false]));
        assert_eq!(value["diagnostic_lists_truncated"]["audio_outputs"], false);
        let invalid = diagnostic_metadata(33);
        assert!(invalid.validate(RATE as u32).is_err());
        let value = metadata_json(&invalid);
        assert_eq!(value["audio_output_bus_count"], 33);
        assert_eq!(value["declared_audio_output_channels"], 66);
        assert_eq!(value["audio_outputs"].as_array().unwrap().len(), 32);
        assert_eq!(value["diagnostic_lists_truncated"]["audio_outputs"], true);
        assert_eq!(
            value["diagnostic_lists_do_not_authorize_plugin_or_publication"],
            true
        );
    }

    fn capture_record(
        ordinal: u64,
        metadata: crate::instrument_bounce::BouncePluginMetadata,
    ) -> Arc<observe::Record> {
        capture_bytes_record(ordinal, CaptureLabel::Initial, metadata, vec![1, 2, 3])
    }

    fn capture_bytes_record(
        ordinal: u64,
        label: CaptureLabel,
        metadata: crate::instrument_bounce::BouncePluginMetadata,
        opaque_state: Vec<u8>,
    ) -> Arc<observe::Record> {
        let identity = crate::instrument_bounce::BounceSourceIdentity {
            project_session: 1,
            project_fingerprint: 2,
            timeline_revision: 3,
            epoch: 4,
            endpoint_id: 5,
            channel_id: 6,
            instance_id: 7,
        };
        Arc::new(observe::Record {
            ordinal,
            monotonic_elapsed_ns: 0,
            stage: Stage::Conversion,
            instance: None,
            kind: RecordKind::Capture {
                label,
                identity,
                receipt_identity: ordinal as usize,
                state: crate::instrument_bounce::BouncePluginState {
                    metadata,
                    opaque_state,
                    parameter_bases: BTreeMap::new(),
                    native_generation: 11,
                    native_dirty_revision: 12,
                },
            },
        })
    }

    #[test]
    fn capture_failure_diagnostics_keep_hash_counts_native_revisions_and_bound_rows_without_state_copy()
     {
        let records = (1..=6)
            .map(|ordinal| capture_record(ordinal, diagnostic_metadata(32)))
            .collect::<Vec<_>>();
        let value = capture_diagnostics(&records);
        assert_eq!(value["capture_count"], 6);
        assert_eq!(value["captures"].as_array().unwrap().len(), 4);
        assert_eq!(value["truncated"], true);
        assert_eq!(value["opaque_state_bytes_copied_to_json"], false);
        let first = &value["captures"][0];
        assert_eq!(first["metadata"]["audio_output_bus_count"], 32);
        assert_eq!(first["metadata"]["event_output_bus_count"], 1);
        assert_eq!(first["state_bytes"], 3);
        assert_eq!(first["state_sha256"], oracles::sha256_hex(&[1, 2, 3]));
        assert_eq!(first["native_generation"], 11);
        assert_eq!(first["native_dirty_revision"], 12);
        assert!(first.get("opaque_state").is_none());
        assert_eq!(
            first["all_ni_internal_parameters_restoration_proven"],
            false
        );
    }

    fn diagnostic_envelope(component: &[u8], controller: Option<&[u8]>) -> Vec<u8> {
        let mut state = b"VST3HOST_STATE\0\0".to_vec();
        state.extend_from_slice(&1_u32.to_le_bytes());
        state.extend_from_slice(&(component.len() as u32).to_le_bytes());
        state.extend_from_slice(
            &controller
                .map_or(u32::MAX, |bytes| bytes.len() as u32)
                .to_le_bytes(),
        );
        state.extend_from_slice(component);
        if let Some(controller) = controller {
            state.extend_from_slice(controller);
        }
        state
    }

    fn diagnostic_directory(tag: &str) -> PathBuf {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "citrus-noire-{tag}-{}-{suffix}",
            std::process::id()
        ));
        fs::create_dir(&directory).unwrap();
        directory
    }

    fn remove_diagnostic_directory(directory: &Path) {
        // Only nonrecursive removal of files in this newly created unit-test
        // directory. Failed assertions intentionally retain their evidence.
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            assert!(entry.file_type().unwrap().is_file());
            fs::remove_file(entry.path()).unwrap();
        }
        fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn capture_raw_diagnostic_geometry_hashes_only_known_host_component_and_controller_slices() {
        let component = [1, 4, 7, 10];
        let controller = [3, 8, 13];
        let mut state = diagnostic_envelope(&component, Some(&controller));
        let value = capture_envelope_diagnostics(&state).unwrap();
        assert_eq!(value["header_bytes"], 28);
        assert_eq!(value["version"], 1);
        assert_eq!(
            value["component"],
            json!({"offset":28,"end_exclusive":32,"bytes":4,
            "sha256":oracles::sha256_hex(&component)})
        );
        assert_eq!(
            value["controller"],
            json!({"offset":32,"end_exclusive":35,"bytes":3,
            "sha256":oracles::sha256_hex(&controller)})
        );
        assert_eq!(value["vendor_payload_interpreted_or_canonicalized"], false);
        let absent = capture_envelope_diagnostics(&diagnostic_envelope(&component, None)).unwrap();
        assert_eq!(absent["controller_absent"], true);
        assert!(absent["controller"].is_null());
        state[20..24].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(capture_envelope_diagnostics(&state).is_err());
        assert!(capture_envelope_diagnostics(&state[..27]).is_err());
    }

    #[test]
    fn capture_raw_diagnostic_retains_nonidentical_actual_files_with_equal_metadata_and_exact_diff()
    {
        let directory = diagnostic_directory("raw-diff");
        let before = diagnostic_envelope(&[1, 2, 3, 4, 5, 6, 7, 8], Some(&[9, 10, 11, 12]));
        let mut after = before.clone();
        for offset in [30, 31, 36, 39] {
            after[offset] ^= 0x80;
        }
        let records = vec![
            capture_bytes_record(
                10,
                CaptureLabel::Initial,
                diagnostic_metadata(32),
                before.clone(),
            ),
            capture_bytes_record(
                20,
                CaptureLabel::Final,
                diagnostic_metadata(32),
                after.clone(),
            ),
        ];
        let value = retain_capture_bytes_checked(&directory, &records).unwrap();
        assert_eq!(value["diagnostic_complete"], true);
        assert_eq!(
            value["attempted_write_bytes_reserved"],
            before.len() + after.len()
        );
        for (index, expected) in [&before, &after].into_iter().enumerate() {
            let row = &value["captures"][index];
            let path = PathBuf::from(row["retention"]["created_path"].as_str().unwrap());
            let actual = read_bounded(&path, observe::MAX_STATE_BYTES as u64).unwrap();
            assert_eq!(&actual, expected);
            assert_eq!(row["state_sha256"], oracles::sha256_hex(&actual));
            assert_eq!(
                row["retention"]["created_file_sha256"],
                oracles::sha256_hex(&actual)
            );
            assert_eq!(
                row["retention"]["sha256_follows_exact_streamed_file_byte_verification"],
                true
            );
            assert!(row.get("opaque_state").is_none());
        }
        let comparison = &value["comparisons_to_first_retained_capture"][0];
        for name in [
            "source_identity_equal",
            "metadata_equal",
            "parameter_bases_equal",
            "native_revisions_equal",
        ] {
            assert_eq!(comparison[name], true);
        }
        assert_eq!(comparison["diff"]["first_differing_offset"], 30);
        assert_eq!(comparison["diff"]["differing_byte_count"], 4);
        assert_eq!(
            comparison["diff"]["ranges"],
            json!([
            {"offset":30,"end_exclusive":32,"bytes":2},
            {"offset":36,"end_exclusive":37,"bytes":1},
            {"offset":39,"end_exclusive":40,"bytes":1}])
        );
        assert_ne!(
            value["captures"][0]["envelope"]["component"]["sha256"],
            value["captures"][1]["envelope"]["component"]["sha256"]
        );
        assert_ne!(
            value["captures"][0]["envelope"]["controller"]["sha256"],
            value["captures"][1]["envelope"]["controller"]["sha256"]
        );
        assert_eq!(
            value["raw_file_comparison_does_not_authorize_insert_or_restore"],
            true
        );
        remove_diagnostic_directory(&directory);
    }

    #[test]
    fn capture_raw_diagnostic_create_new_failure_keeps_original_file_and_body_error() {
        let directory = diagnostic_directory("raw-existing");
        let marker = directory.join("opaque-capture-00000000000000000010-Initial.bin");
        new_write(&marker, b"prior independent evidence").unwrap();
        let records = vec![capture_bytes_record(
            10,
            CaptureLabel::Initial,
            diagnostic_metadata(32),
            diagnostic_envelope(&[9, 8, 7], None),
        )];
        let mut evidence = json!({"body_error":"The second actual source capture differs from the consumed render capture",
            "body_passed":false,"success":false,"independent_marker":"original"});
        attach_capture_diagnostics(&mut evidence, &directory, &records, false);
        assert_eq!(
            evidence["body_error"],
            "The second actual source capture differs from the consumed render capture"
        );
        assert_eq!(evidence["body_passed"], false);
        assert_eq!(evidence["success"], false);
        assert_eq!(evidence["independent_marker"], "original");
        let value = &evidence["actual_capture_raw_retention"];
        assert_eq!(value["diagnostic_complete"], false);
        assert!(value["captures"][0]["retention"]["created_path"].is_null());
        assert!(value["captures"][0]["retention"]["error"].is_string());
        assert_eq!(
            read_bounded(&marker, 128).unwrap(),
            b"prior independent evidence"
        );
        assert!(retain_capture_bytes_checked(&directory, &records).is_err());
        assert_eq!(
            read_bounded(&marker, 128).unwrap(),
            b"prior independent evidence"
        );
        remove_diagnostic_directory(&directory);
    }

    #[test]
    fn capture_raw_diagnostic_verifies_actual_file_across_multiple_bounded_readback_chunks() {
        let directory = diagnostic_directory("raw-streaming");
        let component = (0..8193)
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        let state = diagnostic_envelope(&component, Some(&[255, 0, 128]));
        let records = vec![capture_bytes_record(
            10,
            CaptureLabel::Initial,
            diagnostic_metadata(32),
            state.clone(),
        )];
        let value = retain_capture_bytes_checked(&directory, &records).unwrap();
        let retention = &value["captures"][0]["retention"];
        assert_eq!(retention["verification_buffer_bytes"], 4096);
        assert_eq!(retention["created_file_bytes"], state.len());
        let path = PathBuf::from(retention["created_path"].as_str().unwrap());
        let actual = read_bounded(&path, state.len() as u64).unwrap();
        assert_eq!(actual, state);
        assert_eq!(
            retention["created_file_sha256"],
            oracles::sha256_hex(&actual)
        );
        assert_eq!(
            value["captures"][0]["envelope"]["component"]["sha256"],
            oracles::sha256_hex(&component)
        );
        remove_diagnostic_directory(&directory);
    }

    #[test]
    fn capture_raw_diagnostic_reserves_failed_writes_and_stops_at_total_byte_budget() {
        let directory = diagnostic_directory("raw-budget");
        let prior = directory.join("existing.bin");
        new_write(&prior, b"old").unwrap();
        let mut reserved = 0;
        let failed = retain_capture_new(&prior, &[1, 2, 3, 4], &mut reserved, 6);
        assert_eq!(reserved, 4);
        assert!(failed["created_path"].is_null());
        let blocked = directory.join("over-budget.bin");
        let value = retain_capture_new(&blocked, &[5, 6, 7], &mut reserved, 6);
        assert_eq!(reserved, 4);
        assert_eq!(value["complete"], false);
        assert!(!blocked.exists());
        let admitted = directory.join("remaining-budget.bin");
        let value = retain_capture_new(&admitted, &[5, 6], &mut reserved, 6);
        assert_eq!(reserved, 6);
        assert_eq!(value["complete"], true);
        assert_eq!(read_bounded(&admitted, 2).unwrap(), [5, 6]);
        assert_eq!(read_bounded(&prior, 3).unwrap(), b"old");
        remove_diagnostic_directory(&directory);
    }

    #[test]
    fn capture_raw_diagnostic_row_bound_keeps_distinct_final_ordinals_without_extra_files() {
        let directory = diagnostic_directory("raw-row-bound");
        let records = (1..=6)
            .map(|ordinal| {
                capture_bytes_record(
                    ordinal,
                    if ordinal == 1 {
                        CaptureLabel::Initial
                    } else {
                        CaptureLabel::Final
                    },
                    diagnostic_metadata(32),
                    diagnostic_envelope(&[ordinal as u8], None),
                )
            })
            .collect::<Vec<_>>();
        let value = retain_capture_bytes(&directory, &records);
        assert_eq!(value["capture_count"], 6);
        assert_eq!(value["captures_truncated"], true);
        assert_eq!(value["diagnostic_complete"], false);
        assert_eq!(value["captures"].as_array().unwrap().len(), 4);
        assert_eq!(value["per_state_byte_limit"], observe::MAX_STATE_BYTES);
        assert_eq!(
            value["total_attempted_write_byte_limit"],
            observe::MAX_PRIVATE_BYTES
        );
        let files = fs::read_dir(&directory)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(files.len(), 4);
        let names = files
            .iter()
            .map(|entry| entry.file_name())
            .collect::<BTreeSet<_>>();
        assert_eq!(names.len(), 4);
        assert!(
            !directory
                .join("opaque-capture-00000000000000000005-Final.bin")
                .exists()
        );
        let trace = source_positive_diagnostics(&records, true);
        assert_eq!(trace["final_capture_count"], 5);
        assert_eq!(trace["capture_window_counts_known"], false);
        assert!(trace["between_captures_attempt_count"].is_null());
        remove_diagnostic_directory(&directory);
    }

    #[test]
    fn capture_raw_diagnostic_diff_counts_all_ranges_and_unmatched_suffix_without_payload_excerpts()
    {
        let before = [0_u8; 20];
        let mut after = before;
        for index in (0..20).step_by(2) {
            after[index] = 1;
        }
        let value = opaque_byte_diff(&before, &after).unwrap();
        assert_eq!(value["differing_byte_count"], 10);
        assert_eq!(value["range_count"], 10);
        assert_eq!(value["ranges"].as_array().unwrap().len(), 4);
        assert_eq!(value["ranges_truncated"], true);
        assert_eq!(value["first_differing_offset"], 0);
        assert_eq!(value["opaque_bytes_copied_to_json"], false);
        let suffix = opaque_byte_diff(&[10, 20], &[10, 21, 30]).unwrap();
        assert_eq!(suffix["first_differing_offset"], 1);
        assert_eq!(suffix["differing_byte_count"], 2);
        assert_eq!(
            suffix["ranges"],
            json!([{"offset":1,"end_exclusive":3,"bytes":2}])
        );
        let equal = opaque_byte_diff(&before, &before).unwrap();
        assert!(equal["first_differing_offset"].is_null());
        assert_eq!(equal["differing_byte_count"], 0);
    }

    fn positive_call_record(
        ordinal: u64,
        instance: observe::InstanceIdentity,
        operation: Operation,
        boundary: Boundary,
        frames: usize,
        outcome: Option<observe::CallOutcome>,
    ) -> Arc<observe::Record> {
        Arc::new(observe::Record {
            ordinal,
            monotonic_elapsed_ns: ordinal * 100,
            stage: Stage::Conversion,
            instance: Some(instance),
            kind: RecordKind::Call {
                operation,
                boundary,
                frames: Some(frames),
                outcome,
                transport: Some(crate::plugins::plugin_runtime::PluginTransport {
                    sample_position: ordinal as i64 * 128,
                    playing: operation == Operation::OrdinaryProcess,
                    ..Default::default()
                }),
            },
        })
    }

    fn diagnostic_source_instance() -> observe::InstanceIdentity {
        observe::InstanceIdentity {
            ordinal: 1,
            phase: InstancePhase::InitialSource,
            worker_id: Some(19),
            source_instance_id: Some(7),
        }
    }

    #[test]
    fn capture_source_trace_uses_actual_record_ordinals_and_excludes_clone_undo_and_foreign_source()
    {
        let original = diagnostic_source_instance();
        let clone = observe::InstanceIdentity {
            ordinal: 2,
            phase: InstancePhase::StrictClone,
            worker_id: None,
            source_instance_id: Some(7),
        };
        let undo = observe::InstanceIdentity {
            ordinal: 3,
            phase: InstancePhase::UndoSource,
            worker_id: Some(20),
            source_instance_id: Some(7),
        };
        let foreign = observe::InstanceIdentity {
            ordinal: 4,
            source_instance_id: Some(99),
            ..original
        };
        let records = vec![
            positive_call_record(
                5,
                original,
                Operation::OrdinaryProcess,
                Boundary::Attempt,
                128,
                None,
            ),
            positive_call_record(
                6,
                original,
                Operation::OrdinaryProcess,
                Boundary::Returned,
                128,
                Some(observe::CallOutcome::Ok),
            ),
            capture_bytes_record(
                10,
                CaptureLabel::Initial,
                diagnostic_metadata(32),
                diagnostic_envelope(&[1], None),
            ),
            positive_call_record(
                12,
                original,
                Operation::ResetOrigin,
                Boundary::Attempt,
                64,
                None,
            ),
            positive_call_record(
                13,
                original,
                Operation::ResetOrigin,
                Boundary::Returned,
                64,
                Some(observe::CallOutcome::Ok),
            ),
            positive_call_record(
                14,
                original,
                Operation::OrdinaryProcess,
                Boundary::Attempt,
                32,
                None,
            ),
            positive_call_record(
                15,
                original,
                Operation::OrdinaryProcess,
                Boundary::Returned,
                32,
                Some(observe::CallOutcome::Error(
                    "actual returned error fixture".into(),
                )),
            ),
            positive_call_record(
                16,
                clone,
                Operation::OrdinaryProcess,
                Boundary::Attempt,
                128,
                None,
            ),
            positive_call_record(
                17,
                foreign,
                Operation::OrdinaryProcess,
                Boundary::Attempt,
                128,
                None,
            ),
            positive_call_record(
                18,
                undo,
                Operation::OrdinaryProcess,
                Boundary::Attempt,
                128,
                None,
            ),
            positive_call_record(
                19,
                original,
                Operation::OrdinaryProcess,
                Boundary::Attempt,
                0,
                None,
            ),
            capture_bytes_record(
                20,
                CaptureLabel::Final,
                diagnostic_metadata(32),
                diagnostic_envelope(&[2], None),
            ),
            positive_call_record(
                25,
                original,
                Operation::OrdinaryProcess,
                Boundary::Attempt,
                128,
                None,
            ),
        ];
        let value = source_positive_diagnostics(&records, true);
        assert_eq!(value["initial_capture_ordinal"], 10);
        assert_eq!(value["final_capture_ordinal"], 20);
        assert_eq!(value["capture_window_counts_known"], true);
        assert_eq!(value["between_captures_attempt_count"], 2);
        assert_eq!(value["original_source_observed_attempt_count"], 4);
        assert_eq!(value["ordinary_observed_attempts"], 3);
        assert_eq!(value["reset_origin_observed_attempts"], 1);
        assert_eq!(value["returned_ok_observed_counts"]["between_captures"], 1);
        assert_eq!(value["unbound_initial_source_attempts_excluded"], 1);
        let rows = value["trace"].as_array().unwrap();
        assert_eq!(
            rows.iter()
                .map(|row| row["ordinal"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            [5, 12, 14, 25]
        );
        assert_eq!(rows[1]["monotonic_elapsed_ns"], 1200);
        assert_eq!(rows[1]["site"], "ResetOrigin");
        assert_eq!(rows[1]["frames"], 64);
        assert_eq!(rows[1]["playing"], false);
        assert_eq!(rows[1]["sample_position"], 1536);
        assert_eq!(rows[1]["source"]["worker_id"], 19);
        assert_eq!(rows[1]["capture_relation"], "between_captures");
        assert!(rows.iter().all(|row| row["epoch"].is_null()));
        assert_eq!(value["attempts_are_not_successful_process_receipts"], true);
    }

    #[test]
    fn capture_source_trace_exact_counts_survive_trace_truncation_and_unknown_boundary_is_not_zero()
    {
        let source = diagnostic_source_instance();
        let mut records = vec![capture_bytes_record(
            1,
            CaptureLabel::Initial,
            diagnostic_metadata(32),
            diagnostic_envelope(&[1], None),
        )];
        records.extend((10..80).map(|ordinal| {
            positive_call_record(
                ordinal,
                source,
                Operation::OrdinaryProcess,
                Boundary::Attempt,
                128,
                None,
            )
        }));
        records.push(capture_bytes_record(
            100,
            CaptureLabel::Final,
            diagnostic_metadata(32),
            diagnostic_envelope(&[2], None),
        ));
        let value = source_positive_diagnostics(&records, true);
        assert_eq!(value["between_captures_attempt_count"], 70);
        assert_eq!(value["original_source_observed_attempt_count"], 70);
        assert_eq!(value["trace"].as_array().unwrap().len(), 64);
        assert_eq!(value["trace_truncated"], true);
        let incomplete = source_positive_diagnostics(&records, false);
        assert_eq!(incomplete["capture_window_counts_known"], false);
        assert!(incomplete["between_captures_attempt_count"].is_null());
        assert!(incomplete["capture_window_attempt_counts"].is_null());
        assert_eq!(incomplete["original_source_observed_attempt_count"], 70);
        records.push(capture_bytes_record(
            101,
            CaptureLabel::Final,
            diagnostic_metadata(32),
            diagnostic_envelope(&[3], None),
        ));
        let ambiguous = source_positive_diagnostics(&records, true);
        assert_eq!(ambiguous["final_capture_count"], 2);
        assert!(ambiguous["final_capture_ordinal"].is_null());
        assert!(ambiguous["between_captures_attempt_count"].is_null());
        assert_eq!(
            ambiguous["observed_attempt_counts"]["unknown_capture_boundary"],
            70
        );
    }

    #[test]
    fn capture_source_trace_missing_source_binding_or_multiple_incarnations_does_not_claim_no_dsp()
    {
        let source = diagnostic_source_instance();
        let initial = capture_bytes_record(
            1,
            CaptureLabel::Initial,
            diagnostic_metadata(32),
            diagnostic_envelope(&[1], None),
        );
        let final_capture = capture_bytes_record(
            100,
            CaptureLabel::Final,
            diagnostic_metadata(32),
            diagnostic_envelope(&[2], None),
        );
        let unknown = observe::InstanceIdentity {
            source_instance_id: None,
            ordinal: 2,
            ..source
        };
        let records = vec![
            initial.clone(),
            positive_call_record(
                10,
                source,
                Operation::OrdinaryProcess,
                Boundary::Attempt,
                128,
                None,
            ),
            positive_call_record(
                11,
                unknown,
                Operation::OrdinaryProcess,
                Boundary::Attempt,
                128,
                None,
            ),
            final_capture.clone(),
        ];
        let value = source_positive_diagnostics(&records, true);
        assert_eq!(value["unidentified_initial_source_attempts"], 1);
        assert!(value["between_captures_attempt_count"].is_null());
        assert_eq!(value["trace"].as_array().unwrap().len(), 2);
        assert_eq!(
            value["trace"][1]["source_bound_to_single_original_incarnation"],
            false
        );
        assert_eq!(
            value["trace"][1]["capture_relation"],
            "unknown_capture_boundary"
        );
        let other_incarnation = observe::InstanceIdentity {
            ordinal: 3,
            worker_id: Some(22),
            ..source
        };
        let records = vec![
            initial,
            positive_call_record(
                10,
                source,
                Operation::OrdinaryProcess,
                Boundary::Attempt,
                128,
                None,
            ),
            positive_call_record(
                11,
                other_incarnation,
                Operation::OrdinaryProcess,
                Boundary::Attempt,
                128,
                None,
            ),
            final_capture,
        ];
        let value = source_positive_diagnostics(&records, true);
        assert_eq!(value["original_source_incarnations_matching_capture"], 2);
        assert!(value["original_source_observed_attempt_count"].is_null());
        assert!(value["between_captures_attempt_count"].is_null());
        assert_eq!(value["trace"].as_array().unwrap().len(), 2);
        assert!(
            value["trace"]
                .as_array()
                .unwrap()
                .iter()
                .all(|row| row["source_bound_to_single_original_incarnation"] == false)
        );
    }

    #[test]
    fn attempted_path_or_existing_foreign_file_without_actual_producer_receipt_is_not_published() {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "citrus-noire-path-label-unit-{}-{suffix}",
            std::process::id()
        ));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("foreign-file.wav");
        new_write(&path, b"unrelated preexisting data").unwrap();
        assert!(path.exists());
        let value = output_diagnostics(None, &[], Some(&path));
        assert_eq!(value["attempted_output_path"], json!(path));
        assert!(value["published_path"].is_null());
        assert!(!value["app_media_verified"].as_bool().unwrap());
        assert_eq!(
            value["filesystem_existence_is_not_ownership_evidence"],
            true
        );
        // Even actual render geometry without the App's retained producer path
        // cannot associate this arbitrary existing file with this invocation.
        let capture = capture_record(1, diagnostic_metadata(32));
        let identity = match &capture.kind {
            RecordKind::Capture { identity, .. } => *identity,
            _ => unreachable!(),
        };
        let report = Arc::new(observe::Record {
            ordinal: 2,
            monotonic_elapsed_ns: 1,
            stage: Stage::Conversion,
            instance: None,
            kind: RecordKind::RenderReport(crate::instrument_bounce::render::RenderReport {
                identity,
                origin_frame: 0,
                musical_frames: 1,
                frames: 1,
                native_latency: 257,
                reported_tail: u32::MAX,
                fixed_tail: 0,
                generated_frames: 384,
                sample_peak: 0.1,
            }),
        });
        let value = output_diagnostics(None, &[capture, report], Some(&path));
        assert!(value["published_path"].is_null());
        assert!(value["publication_evidence"].is_null());
        assert_eq!(
            read_bounded(&path, 128).unwrap(),
            b"unrelated preexisting data"
        );
        fs::remove_file(path).unwrap();
        fs::remove_dir(directory).unwrap();
    }

    fn release_pcm() -> (Vec<f32>, usize, usize) {
        let music = RATE;
        let tail = RATE * 3;
        let mut pcm = vec![0.0; (music + tail) * 2];
        for frame in 0..music {
            pcm[frame * 2] = 0.01;
            pcm[frame * 2 + 1] = -0.02;
        }
        for frame in music..music + 1000 {
            pcm[frame * 2] = 0.001;
            pcm[frame * 2 + 1] = -0.002;
        }
        (pcm, music, tail)
    }

    #[test]
    fn pcm_audit_keeps_complete_three_second_release_geometry_and_stereo_facts() {
        let (pcm, music, tail) = release_pcm();
        let metrics = PcmMetrics::measure(&pcm, music, tail).unwrap();
        metrics.qualify().unwrap();
        assert_eq!(metrics.frames, music + tail);
        assert_eq!(metrics.first_nonzero_frame, Some(0));
        assert_eq!(metrics.last_nonzero_frame, Some(music + 999));
        assert_eq!(metrics.tail_nonzero_frames, 1000);
        assert_eq!(metrics.clipping_samples, 0);
        assert_eq!(metrics.last_100ms_peak, 0.0);
        assert_eq!(metrics.evidence()["full_natural_tail_guarantee"], false);
        assert!(PcmMetrics::measure(&pcm, music, tail - 1).is_err());
    }

    #[test]
    fn pcm_audit_rejects_silence_missing_release_clipping_and_fixed_tail_truncation() {
        let (pcm, music, tail) = release_pcm();
        let mut silent = vec![0.0; pcm.len()];
        let measured = PcmMetrics::measure(&silent, music, tail).unwrap();
        assert!(measured.qualify().unwrap_err().contains("silent"));
        silent[..music * 2].copy_from_slice(&pcm[..music * 2]);
        assert!(
            PcmMetrics::measure(&silent, music, tail)
                .unwrap()
                .qualify()
                .unwrap_err()
                .contains("no actual instrument release")
        );
        let mut clipped = pcm.clone();
        clipped[0] = 1.0;
        let measured = PcmMetrics::measure(&clipped, music, tail).unwrap();
        assert_eq!(measured.clipping_samples, 1);
        assert!(measured.qualify().unwrap_err().contains("full scale"));
        let mut truncated = pcm;
        let end = truncated.len();
        truncated[end - 100..].fill(0.001);
        let measured = PcmMetrics::measure(&truncated, music, tail).unwrap();
        assert_eq!(measured.evidence()["fixed_tail_limit_reached_pcm"], true);
        assert!(measured.qualify().unwrap_err().contains("fixed tail limit"));
    }

    #[test]
    fn pcm_audit_measures_internal_zero_run_and_rejects_nonfinite_channels() {
        let (mut pcm, music, tail) = release_pcm();
        pcm[200..240].fill(0.0);
        let measured = PcmMetrics::measure(&pcm, music, tail).unwrap();
        assert_eq!(measured.maximum_internal_zero_run, 20);
        assert_eq!(measured.maximum_adjacent_channel_delta, 0.02);
        for sample in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            pcm[music * 2 + 1] = sample;
            assert!(PcmMetrics::measure(&pcm, music, tail).is_err());
        }
    }

    #[test]
    fn private_clone_reply_bound_retains_real_latency_and_three_seconds_tail() {
        // The pinned native13 source is 84 BPM. Neither latency nor retained
        // tail is reduced to fit the deliberately run-local observation cap.
        let music = (2.0_f64 * 60.0 / 84.0 * RATE as f64).round() as usize;
        for latency in [0, 257, RATE] {
            let oracle = oracles::q128_frame_oracle(music, RATE * 3, latency).unwrap();
            assert_eq!(oracle.retained_frames, music + RATE * 3);
            assert_eq!(oracle.retained_start_frame, latency);
            assert!(oracle.generated_frames / 128 <= CLONE_REPLY_LIMIT);
        }
        assert_eq!(observe::MAX_CLONE_REPLIES, 282);
    }

    fn terminal_facts() -> TerminalFacts {
        TerminalFacts {
            workers: BTreeSet::from([10, 20]),
            joins: BTreeMap::from([
                (10, vec![WorkerJoinOutcome::Joined]),
                (20, vec![WorkerJoinOutcome::Joined]),
            ]),
            source_instances: vec![
                (InstancePhase::InitialSource, Some(10), Some(100)),
                (InstancePhase::UndoSource, Some(20), Some(100)),
            ],
            clone_instances: 1,
            clone_reaps: vec![(2, true)],
            clone_terminal_disposals: vec![3],
            clone_destructors: vec![4],
            render_destructions: vec![5],
            hooks_inactive: true,
        }
    }

    #[test]
    fn terminal_audit_requires_concrete_reap_and_each_distinct_source_join() {
        let mut facts = terminal_facts();
        facts.qualify(true).unwrap();
        facts.clone_reaps.clear();
        assert!(
            facts
                .qualify(false)
                .unwrap_err()
                .contains("concrete Client Reaped")
        );
        facts.clone_reaps.push((2, true));
        facts.joins.get_mut(&20).unwrap().clear();
        assert!(facts.qualify(false).unwrap_err().contains("JoinHandle"));
        facts
            .joins
            .insert(20, vec![WorkerJoinOutcome::WorkerPanicked]);
        assert!(facts.qualify(false).is_err());
        facts.joins.insert(
            20,
            vec![WorkerJoinOutcome::Joined, WorkerJoinOutcome::Joined],
        );
        assert!(facts.qualify(false).is_err());
    }

    #[test]
    fn terminal_audit_distinguishes_reaped_failure_from_success_and_wrong_order() {
        let mut facts = terminal_facts();
        facts.clone_reaps[0].1 = false;
        facts.qualify(false).unwrap();
        assert!(facts.qualify(true).unwrap_err().contains("abnormal"));
        facts.clone_reaps[0].1 = true;
        facts.clone_destructors[0] = 1;
        assert!(
            facts
                .qualify(false)
                .unwrap_err()
                .contains("destruction preceded")
        );
        facts.clone_destructors[0] = 4;
        facts.hooks_inactive = false;
        assert!(
            facts
                .qualify(false)
                .unwrap_err()
                .contains("behavior-changing")
        );
    }

    #[test]
    fn terminal_record_reader_binds_instance_and_worker_receipt_without_payload_copy() {
        use observe::{InstanceIdentity, Record};
        let source = Some(InstanceIdentity {
            ordinal: 1,
            phase: InstancePhase::InitialSource,
            worker_id: Some(19),
            source_instance_id: Some(100),
        });
        let records = vec![
            Arc::new(Record {
                ordinal: 1,
                monotonic_elapsed_ns: 0,
                stage: Stage::InitialSource,
                instance: source,
                kind: RecordKind::Lifecycle("instance construction observed"),
            }),
            Arc::new(Record {
                ordinal: 2,
                monotonic_elapsed_ns: 1,
                stage: Stage::Cleanup,
                instance: None,
                kind: RecordKind::WorkerJoin {
                    worker_id: 19,
                    outcome: WorkerJoinOutcome::Joined,
                },
            }),
        ];
        let mut facts = TerminalFacts::from_records(&records);
        facts.qualify(false).unwrap();
        assert_eq!(facts.workers, BTreeSet::from([19]));
        assert_eq!(facts.source_instances.len(), 1);
        facts.joins.insert(99, vec![WorkerJoinOutcome::Joined]);
        assert!(facts.qualify(false).unwrap_err().contains("not bound"));
    }

    #[test]
    fn bounded_receipt_writer_is_exclusive_and_never_replaces_existing_evidence() {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "citrus-noire-receipt-unit-{}-{suffix}",
            std::process::id()
        ));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("result.json");
        let first = json!({"schema":1,"success":false,"error":"actual diagnostic retained"});
        new_json(&path, &first).unwrap();
        let original = read_bounded(&path, 1024 * 1024).unwrap();
        assert!(new_json(&path, &json!({"success":true})).is_err());
        assert_eq!(read_bounded(&path, 1024 * 1024).unwrap(), original);
        let overflow = directory.join("overflow.json");
        assert!(new_json(&overflow, &json!({"oversize":"x".repeat(1024 * 1024)})).is_err());
        assert!(!overflow.exists());
        fs::remove_file(path).unwrap();
        fs::remove_dir(directory).unwrap();
    }
}
