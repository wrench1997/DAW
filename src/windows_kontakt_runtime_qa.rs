//! Explicit, device-free acceptance of the actual Windows VST3 worker lifecycle.
//! Neither silence nor successful state commands qualify a NOIRE instrument.
//!
//! The separate ignored `windows_noire_actual_first_sequence_callback_cadence`
//! probe accepts only a saved, locally verified NOIRE opaque state. Set absolute
//! CITRUS_NOIRE_FIRST_BLOCK_QA_{DIR,HELPER,STATE} paths; DIR must not exist. Stop
//! the native DAW and its helper first, then run this one test with --ignored
//! --exact --test-threads=1. No SaveState barrier precedes any reset phase. Its
//! simulated 480/960-frame cadence does not attest a native audio device, heard
//! sound, cold App restoration, clip conversion or an offline export.

use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

use crate::{
    fixed_quantum::{FixedQuantumAdapter, FixedQuantumProcessStatus, FrameEvent},
    genuine_app_observer::{
        Boundary, CallOutcome, ObservationRun, Operation, RecordKind, Stage, WorkerJoinOutcome,
    },
    plugin_timing::PreparedPluginTimingPlan,
    plugins::{
        PluginDescriptor, PluginFormat, Vst3ScanMetadata,
        plugin_runtime::{
            MidiMessage, PluginChain, PluginChainControl, PluginLoadSpec, PluginPrepareConfig,
            PluginTransport, ReceiveStatus, RetainedShutdownPoll, RuntimeEvent, SubmitStatus,
        },
    },
};

const BLOCKS: usize = 64;
const FRAMES: usize = 128;

const SCAN_SELECTOR: &str =
    "windows_kontakt_qa::runtime_qa::windows_kontakt_default_class_scan_and_reap";
const SCAN_OPT_IN: &str = "actual-kontakt-default-class-scan-and-reap-v1";
static SCAN_ADMITTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn scan_binary_snapshot(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let file = fs::File::open(path).map_err(|error| error.to_string())?;
    let bytes = file.metadata().map_err(|error| error.to_string())?.len();
    if bytes == 0 || bytes > limit {
        return Err(
            "scanner-only binary is empty or exceeds the original bounded pin input".into(),
        );
    }
    let mut snapshot = Vec::new();
    snapshot
        .try_reserve_exact(bytes as usize)
        .map_err(|error| error.to_string())?;
    file.take(limit + 1)
        .read_to_end(&mut snapshot)
        .map_err(|error| error.to_string())?;
    if snapshot.len() as u64 != bytes {
        return Err("scanner-only binary size changed during the snapshot".into());
    }
    Ok(snapshot)
}

fn scan_new_json(path: &Path, value: &serde_json::Value) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| error.to_string())?;
    file.write_all(&bytes).map_err(|error| error.to_string())
}

#[test]
#[ignore = "actual ordinary Kontakt default-class scan only; explicit once-only opt-in, private adjacent helper and outer binary pins required"]
fn windows_kontakt_default_class_scan_and_reap() {
    use crate::plugins::scan_qa::CheckedScan;
    use serde_json::{Value, json};
    use std::sync::atomic::Ordering;

    assert_eq!(
        std::env::var("CITRUS_KONTAKT_SCAN_QA_OPT_IN").as_deref(),
        Ok(SCAN_OPT_IN)
    );
    assert_eq!(
        std::env::args().skip(1).collect::<Vec<_>>(),
        [
            SCAN_SELECTOR,
            "--ignored",
            "--exact",
            "--test-threads=1",
            "--nocapture",
        ]
    );
    assert!(
        !SCAN_ADMITTED.swap(true, Ordering::AcqRel),
        "scanner-only fixture is one-shot"
    );
    let output = absolute_env("CITRUS_KONTAKT_SCAN_QA_DIR");
    fs::create_dir(&output).expect("scanner-only output must be new; preserve every prior receipt");
    let executable = std::env::current_exe().unwrap().canonicalize().unwrap();
    let helper = executable
        .parent()
        .unwrap()
        .join("vst3-host-helper.exe")
        .canonicalize()
        .unwrap();
    // The production scan resolves only this adjacent helper. No helper override,
    // instrument state, source worker, editor, MIDI, Process or device is requested.
    let supplied_executable_sha =
        std::env::var("CITRUS_KONTAKT_SCAN_QA_EXECUTABLE_SHA256").unwrap();
    let supplied_helper_sha = std::env::var("CITRUS_KONTAKT_SCAN_QA_HELPER_SHA256").unwrap();
    for supplied in [&supplied_executable_sha, &supplied_helper_sha] {
        assert!(
            supplied.len() == 64
                && supplied
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );
    }
    let executable_before = scan_binary_snapshot(&executable, 256 * 1024 * 1024).unwrap();
    let helper_before = scan_binary_snapshot(&helper, 64 * 1024 * 1024).unwrap();
    let plugin = PathBuf::from(r"C:\Program Files\Common Files\VST3\Kontakt 8.vst3");
    assert!(plugin.exists());
    scan_new_json(&output.join("preflight.json"), &json!({
        "schema":1,"selector":SCAN_SELECTOR,"plugin":plugin,
        "test_executable":{"path":executable,"bytes":executable_before.len(),"outer_runner_sha256":supplied_executable_sha},
        "adjacent_production_helper":{"path":helper,"bytes":helper_before.len(),"outer_runner_sha256":supplied_helper_sha},
        "sha256_fields_supplied_by_outer_runner":true,
        "sha256_fields_verified_in_this_test":false,
        "outer_runner_verification_required_for_binary_binding":true,
        "this_test_compares_actual_binary_bytes_before_and_after":true,
        "scope":"one ordinary default-class scan; no NOIRE state or audio acceptance",
        "SDK_load_may_probe_createView_capability":true,
    })).unwrap();
    let started = Instant::now();
    let attempted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        CheckedScan::perform(&plugin)
    }));
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    let mut scan = Value::Null;
    let mut reaped = None;
    let result = (|| -> Result<(), String> {
        let checked = attempted.map_err(|_| {
            "ordinary production scanner unwound; preserve retained cleanup ownership".to_owned()
        })?;
        reaped = Some(checked.helper_reaped());
        scan = checked.evidence();
        scan_new_json(&output.join("scan-preflight.json"), &scan)?;
        let accepted = checked.admit("5653544E694B386B6F6E74616B742038")?;
        scan = accepted.evidence();
        if !accepted.descriptor().is_instrument {
            return Err("actual Kontakt default descriptor is not an instrument".into());
        }
        Ok(())
    })();
    let unchanged = scan_binary_snapshot(&executable, 256 * 1024 * 1024)
        .and_then(|bytes| {
            if bytes == executable_before {
                Ok(())
            } else {
                Err("test binary changed during scan".into())
            }
        })
        .and_then(|()| scan_binary_snapshot(&helper, 64 * 1024 * 1024))
        .and_then(|bytes| {
            if bytes == helper_before {
                Ok(())
            } else {
                Err("helper binary changed during scan".into())
            }
        });
    let passed = result.is_ok() && unchanged.is_ok() && reaped == Some(true);
    scan_new_json(&output.join("result.json"), &json!({
        "schema":1,"passed":passed,"actual_error":result.as_ref().err(),
        "actual_binary_bytes_unchanged":unchanged.is_ok(),"actual_binary_pin_error":unchanged.as_ref().err(),
        "scan_helper_strict_reap_proven":reaped,"scan":scan,
        "single_call_elapsed_ms_including_spawn_load_and_cleanup":elapsed_ms,
        "elapsed_value_not_a_performance_or_audio_deadline_acceptance":true,
        "source_worker_started":false,"source_helper_reap_proven":false,
        "state_MIDI_Process_editor_open_and_audio_device_requests":0,
        "Kontakt_version_NOIRE_instrument_and_audio_not_proven_by_descriptor":true,
        "OS_process_absence_not_used_as_reap_evidence":true,
    })).unwrap();
    assert!(
        passed,
        "ordinary Kontakt scan failed; preserve {}",
        output.display()
    );
}

fn absolute_env(name: &str) -> PathBuf {
    let path = PathBuf::from(std::env::var_os(name).unwrap_or_else(|| panic!("set {name}")));
    assert!(
        path.is_absolute(),
        "{name} must be an absolute private path"
    );
    path
}

fn state_barrier(
    chain: &mut PluginChain,
    request_id: u64,
    epoch: Option<u64>,
) -> Result<Vec<u8>, String> {
    let admitted = match epoch {
        Some(epoch) => chain
            .control
            .request_state_tagged_for_epoch(0, request_id, epoch),
        None => chain.control.request_state_tagged(0, request_id),
    };
    if !admitted {
        return Err("state barrier could not be queued".into());
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        while let Some(event) = chain.control.try_next_event() {
            match event {
                RuntimeEvent::SlotFault { message, .. } => return Err(message),
                RuntimeEvent::State {
                    slot: 0,
                    request_id: actual,
                    epoch: observed,
                    bytes,
                } if actual == request_id && observed == epoch => {
                    if bytes.is_empty() {
                        return Err("state barrier returned empty state".into());
                    }
                    return Ok(bytes);
                }
                RuntimeEvent::StateRejected {
                    request_id: actual,
                    reason,
                    ..
                } if actual == request_id => {
                    return Err(format!("state barrier rejected: {reason:?}"));
                }
                _ => {}
            }
        }
        thread::sleep(Duration::from_millis(1));
    }
    Err("actual worker state barrier timed out".into())
}

fn process_phase(chain: &mut PluginChain, phase: &str) -> Result<serde_json::Value, String> {
    let input = [0.0_f32; FRAMES];
    let mut left = [0.0_f32; FRAMES];
    let mut right = [0.0_f32; FRAMES];
    let mut elapsed_ms = Vec::with_capacity(BLOCKS);
    let mut peak = 0.0_f32;
    let mut square_sum = 0.0_f64;
    for block in 0..BLOCKS {
        chain.audio.set_transport(PluginTransport {
            sample_position: (block * FRAMES) as i64,
            quarter_note_position: (block * FRAMES) as f64 / 48_000.0 * 80.0 / 60.0,
            tempo: 80.0,
            playing: true,
            ..PluginTransport::default()
        });
        if block == 0 || block == BLOCKS / 2 {
            let data = if block == 0 {
                [0x90, 60, 80]
            } else {
                [0x80, 60, 0]
            };
            if !chain
                .audio
                .try_send_midi(Some(0), MidiMessage::new(data, 0))
            {
                return Err(format!("{phase}: MIDI admission failed"));
            }
        }
        let started = Instant::now();
        let expected = match chain.audio.try_submit(&input, &input) {
            SubmitStatus::Submitted { sequence } => sequence,
            other => return Err(format!("{phase}: submit {other:?}")),
        };
        let deadline = started + Duration::from_secs(5);
        loop {
            while let Some(event) = chain.control.try_next_event() {
                if let RuntimeEvent::SlotFault { message, .. } = event {
                    return Err(message);
                }
            }
            match chain.audio.try_receive(&mut left, &mut right) {
                ReceiveStatus::Processed { sequence, frames }
                    if sequence == expected && frames == FRAMES =>
                {
                    break;
                }
                ReceiveStatus::Empty if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(1))
                }
                other => return Err(format!("{phase}: receive {other:?}")),
            }
        }
        elapsed_ms.push(started.elapsed().as_secs_f64() * 1_000.0);
        for sample in left.iter().chain(&right) {
            if !sample.is_finite() {
                return Err(format!("{phase}: nonfinite PCM"));
            }
            peak = peak.max(sample.abs());
            square_sum += f64::from(*sample).powi(2);
        }
    }
    let stats = chain.control.stats();
    if stats.faults != 0
        || stats.reset_faults != 0
        || stats.input_gaps != 0
        || stats.output_overflows != 0
    {
        return Err(format!("{phase}: actual bridge fault/loss {stats:?}"));
    }
    Ok(serde_json::json!({
        "phase": phase, "blocks": BLOCKS, "frames_per_block": FRAMES,
        "epoch": chain.audio.epoch(), "finite_pcm": true,
        "peak": peak, "rms": (square_sum / (BLOCKS * FRAMES * 2) as f64).sqrt(),
        "serial_submit_to_receive_ms": elapsed_ms,
        "timing_scope": "actual worker, metadata RPCs and polling; serial device-free, not callback deadline acceptance",
        "completed_cumulative": stats.completed, "worker_epoch_resets": stats.worker_epoch_resets,
    }))
}

#[test]
#[ignore = "requires explicit private CITRUS_KONTAKT_RUNTIME_QA_{DIR,HELPER,STATE} and local Kontakt 8.4.1"]
fn windows_kontakt_actual_worker_load_state_reset_and_retained_shutdown() {
    let output = absolute_env("CITRUS_KONTAKT_RUNTIME_QA_DIR");
    let helper = absolute_env("CITRUS_KONTAKT_RUNTIME_QA_HELPER");
    let state_path = absolute_env("CITRUS_KONTAKT_RUNTIME_QA_STATE");
    assert!(helper.is_file() && state_path.is_file());
    let initial_state = fs::read(&state_path).unwrap();
    assert!(!initial_state.is_empty());
    fs::create_dir(&output).expect("refuse to overwrite any previous QA run or user project");
    let uid = "5653544E694B386B6F6E74616B742038";
    let plugin_path = PathBuf::from(r"C:\Program Files\Common Files\VST3\Kontakt 8.vst3");
    assert!(plugin_path.exists());
    let mut spec = PluginLoadSpec::from_descriptor(PluginDescriptor {
        id: "explicit-local-kontakt-runtime-qa".into(),
        name: "Kontakt 8".into(),
        vendor: "Native Instruments".into(),
        path: plugin_path.clone(),
        format: PluginFormat::Vst3,
        category: "Instrument".into(),
        is_instrument: true,
        verified: true,
        scan_error: None,
        vst3_metadata: Some(Vst3ScanMetadata {
            class_uid: uid.into(),
            category: "Instrument".into(),
            has_midi_input: true,
            has_midi_output: true,
        }),
    });
    spec.class_uid = Some(uid.into());
    spec.vst3_helper_path = Some(helper.clone());
    spec.initial_state = initial_state.clone();
    let observation = ObservationRun::install().unwrap();
    observation.set_stage(Stage::InitialSource).unwrap();
    // No factory seam or fake backend: this loads the ordinary production Vst3Backend on its worker.
    let mut chain = PluginChain::spawn_identified(
        vec![(1, spec)],
        PluginPrepareConfig {
            sample_rate: 48_000.0,
            max_block_frames: 2_048,
        },
    )
    .unwrap();
    let worker_id = chain.guard.genuine_worker_id();
    let mut ready_receipt = None;
    let result = (|| -> Result<Vec<serde_json::Value>, String> {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            match chain.control.try_next_event() {
                Some(RuntimeEvent::SlotReady {
                    slot: 0,
                    name,
                    latency_samples,
                    tail_samples,
                }) => {
                    // Production adds the actual vendor to its ready display name.
                    // The loader uses the explicit class UID; keep the observed full name.
                    if name.split(" — ").next() != Some("Kontakt 8") {
                        return Err(format!("unexpected actual ready identity: {name:?}"));
                    }
                    ready_receipt = Some(serde_json::json!({
                        "name": name, "latency_samples": latency_samples,
                        "tail_samples": tail_samples,
                    }));
                    break;
                }
                Some(RuntimeEvent::SlotFault { message, .. }) => return Err(message),
                _ if Instant::now() < deadline => thread::sleep(Duration::from_millis(1)),
                other => return Err(format!("actual Kontakt readiness timed out: {other:?}")),
            }
        }
        let mut phases = vec![process_phase(&mut chain, "initial_state_prepare")?];
        if !chain.control.load_state(0, initial_state) {
            return Err("prepared LoadState could not be queued".into());
        }
        // Same admin FIFO: receipt proves LoadState completed; enqueue alone is not an acknowledgment.
        let resaved = state_barrier(&mut chain, 41, None)?;
        fs::write(output.join("prepared-reloaded.state.bin"), resaved)
            .map_err(|e| e.to_string())?;
        phases.push(process_phase(&mut chain, "prepared_load_state")?);
        let epoch = chain.audio.reset_epoch();
        // Epoch-bound capture forces actual worker reset completion before any new-epoch MIDI/process.
        let reset_saved = state_barrier(&mut chain, 42, Some(epoch))?;
        fs::write(output.join("epoch-reset.state.bin"), reset_saved).map_err(|e| e.to_string())?;
        if chain.control.stats().worker_epoch_resets == 0 {
            return Err("actual worker did not apply reset".into());
        }
        phases.push(process_phase(&mut chain, "explicit_epoch_reset")?);
        Ok(phases)
    })();
    observation.set_stage(Stage::Cleanup).unwrap();
    let shutdown_started = Instant::now();
    let shutdown_deadline = shutdown_started + Duration::from_secs(30);
    let shutdown = loop {
        let progress = chain.guard.poll_shutdown_retained();
        if progress != RetainedShutdownPoll::Pending {
            break progress;
        }
        if Instant::now() >= shutdown_deadline {
            // Keep the guard owned; the test process will fail explicitly rather than label a timeout joined.
            break RetainedShutdownPoll::Pending;
        }
        thread::sleep(Duration::from_millis(1));
    };
    let records = observation.finish();
    let successful_calls = |operation| {
        records.records.iter().filter(|record| matches!(record.kind,
        RecordKind::Call { operation: actual, boundary: Boundary::Returned, outcome: Some(CallOutcome::Ok), .. } if actual == operation)).count()
    };
    let real_process_calls = successful_calls(Operation::OrdinaryProcess);
    let real_prepare_calls = successful_calls(Operation::SourcePrepare);
    let real_reset_calls = successful_calls(Operation::Reset);
    let state_loads = records
        .records
        .iter()
        .filter(|record| {
            matches!(
                record.kind,
                RecordKind::StateLoad {
                    outcome: CallOutcome::Ok,
                    ..
                }
            )
        })
        .count();
    let joined_observed = records.records.iter().any(|record| matches!(record.kind,
        RecordKind::WorkerJoin { worker_id: actual, outcome: WorkerJoinOutcome::Joined } if actual == worker_id));
    let passed = result.is_ok()
        && shutdown == RetainedShutdownPoll::Joined
        && joined_observed
        && real_prepare_calls == 1
        && real_process_calls == BLOCKS * 3
        && real_reset_calls >= 1
        && state_loads >= 2
        && records.failure.is_none();
    let receipt = serde_json::json!({
        "status": if passed { "passed" } else { "failed" },
        "scope": "actual ordinary Windows Vst3Backend and worker; MIDI/process, prepared LoadState, epoch reset, retained worker join",
        "plugin": plugin_path, "class_uid": uid, "helper": helper, "initial_state": state_path,
        "ready": ready_receipt,
        "device_free": true, "noire_qualified": false, "native_editor": "not tested",
        "physical_monitoring": "not tested", "cold_app_restore": "not tested", "offline_export": "not tested",
        "phases": result.as_ref().ok(), "error": result.as_ref().err(),
        "observer_failure": records.failure, "real_prepare_successes": real_prepare_calls,
        "real_process_successes": real_process_calls, "real_reset_successes": real_reset_calls,
        "real_state_load_successes": state_loads, "retained_shutdown": format!("{shutdown:?}"),
        "worker_join_observed": joined_observed, "shutdown_elapsed_seconds": shutdown_started.elapsed().as_secs_f64(),
        "helper_reaped": "worker join alone does not attest subprocess reap",
    });
    fs::write(
        output.join("result.json"),
        serde_json::to_vec_pretty(&receipt).unwrap(),
    )
    .unwrap();
    println!("{}", serde_json::to_string_pretty(&receipt).unwrap());
    assert!(
        passed,
        "actual worker probe failed; inspect {}",
        output.join("result.json").display()
    );
}

const NOIRE_RATE: usize = 48_000;
const NOIRE_FRAMES: usize = NOIRE_RATE * 2;
const NOIRE_NOTE_OFF: usize = NOIRE_RATE / 2;

#[derive(Default, serde::Serialize)]
struct NoirePcmMetrics {
    frames: usize,
    non_finite_samples: usize,
    clipped_samples: usize,
    nonzero_samples: usize,
    peak: f32,
    rms: f64,
    release_peak: f32,
    max_adjacent_delta: f32,
}

fn inspect_noire_pcm(samples: &[f32], release_start_frame: usize) -> NoirePcmMetrics {
    let mut metrics = NoirePcmMetrics {
        frames: samples.len() / 2,
        ..NoirePcmMetrics::default()
    };
    let mut sum = 0.0;
    for (index, &sample) in samples.iter().enumerate() {
        if !sample.is_finite() {
            metrics.non_finite_samples += 1;
            continue;
        }
        let absolute = sample.abs();
        metrics.peak = metrics.peak.max(absolute);
        metrics.clipped_samples += usize::from(absolute >= 1.0);
        metrics.nonzero_samples += usize::from(absolute > 1e-7);
        sum += f64::from(sample).powi(2);
        if index / 2 >= release_start_frame {
            metrics.release_peak = metrics.release_peak.max(absolute);
        }
        if index >= 2 && samples[index - 2].is_finite() {
            metrics.max_adjacent_delta = metrics
                .max_adjacent_delta
                .max((sample - samples[index - 2]).abs());
        }
    }
    if !samples.is_empty() {
        metrics.rms = (sum / samples.len() as f64).sqrt();
    }
    metrics
}

fn noire_pcm_failures(metrics: &NoirePcmMetrics) -> Vec<String> {
    let mut errors = Vec::new();
    if metrics.frames != NOIRE_FRAMES {
        errors.push(format!(
            "incomplete callback PCM: {} frames, expected {NOIRE_FRAMES}",
            metrics.frames
        ));
    }
    if metrics.non_finite_samples != 0 {
        errors.push(format!(
            "nonfinite PCM samples: {}",
            metrics.non_finite_samples
        ));
    }
    if metrics.peak <= 1e-6 || metrics.rms <= 1e-8 {
        errors.push("silent/below-threshold PCM is not NOIRE success".into());
    }
    if metrics.release_peak <= 1e-7 {
        errors.push("no observed release after note-off and bridge latency".into());
    }
    if metrics.clipped_samples != 0 {
        errors.push(format!(
            "unclamped PCM reaches clipping: {} samples",
            metrics.clipped_samples
        ));
    }
    if metrics.max_adjacent_delta > 0.25 {
        errors.push(format!(
            "possible discontinuity: adjacent delta {} exceeds probe threshold 0.25",
            metrics.max_adjacent_delta
        ));
    }
    errors
}

/// Persist only after the simulated callbacks have ended. No encoding/clamping
/// can turn invalid or silent plug-in output into a successful receipt.
fn write_probe_float_wav(path: &Path, samples: &[f32]) -> Result<(), String> {
    let data_bytes = u32::try_from(samples.len() * 4).map_err(|e| e.to_string())?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    let mut header = Vec::with_capacity(44);
    header.extend_from_slice(b"RIFF");
    header.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    header.extend_from_slice(b"WAVEfmt ");
    header.extend_from_slice(&16_u32.to_le_bytes());
    header.extend_from_slice(&3_u16.to_le_bytes()); // Unclamped IEEE float PCM.
    header.extend_from_slice(&2_u16.to_le_bytes());
    header.extend_from_slice(&(NOIRE_RATE as u32).to_le_bytes());
    header.extend_from_slice(&(NOIRE_RATE as u32 * 8).to_le_bytes());
    header.extend_from_slice(&8_u16.to_le_bytes());
    header.extend_from_slice(&32_u16.to_le_bytes());
    header.extend_from_slice(b"data");
    header.extend_from_slice(&data_bytes.to_le_bytes());
    file.write_all(&header).map_err(|e| e.to_string())?;
    for sample in samples {
        file.write_all(&sample.to_le_bytes())
            .map_err(|e| e.to_string())?;
    }
    file.flush().map_err(|e| e.to_string())
}

fn drain_probe_faults(control: &mut PluginChainControl) -> Vec<String> {
    let mut faults = Vec::new();
    while let Some(event) = control.try_next_event() {
        if let RuntimeEvent::SlotFault { message, .. } = event {
            faults.push(message);
        }
    }
    faults
}

fn await_noire_reset_settlement(
    adapter: &FixedQuantumAdapter,
    control: &mut PluginChainControl,
    epoch: u64,
) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let faults = drain_probe_faults(control);
        if !faults.is_empty() {
            return Err(format!("worker fault while waiting for reset: {faults:?}"));
        }
        if adapter.stopped_playback_settled(epoch) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            let snapshot = adapter.genuine_snapshot().bridge;
            return Err(format!(
                "reset settlement timed out: requested {epoch}, worker epoch {}, consumed {}, terminal {}, busy {}",
                snapshot.worker_epoch,
                snapshot.input_consumed,
                snapshot.operations_completed,
                snapshot.worker_busy,
            ));
        }
        thread::sleep(Duration::from_millis(1));
    }
}

#[derive(Clone, Copy)]
struct NoirePhase {
    name: &'static str,
    callback_frames: usize,
    await_reset: bool,
}

fn noire_cadence_phase(
    adapter: &mut FixedQuantumAdapter,
    control: &mut PluginChainControl,
    observation: &ObservationRun,
    phase: NoirePhase,
    output: &Path,
) -> serde_json::Value {
    let epoch = adapter.epoch() + 1;
    let requested_ns = observation.monotonic_elapsed_ns();
    assert!(adapter.set_epoch(epoch));
    let reset_requested_ns = observation.monotonic_elapsed_ns();
    let mut errors = Vec::new();
    if phase.await_reset
        && let Err(error) = await_noire_reset_settlement(adapter, control, epoch)
    {
        errors.push(error);
    }
    let settled_before_first_callback = adapter.stopped_playback_settled(epoch);
    let settled_ns = observation.monotonic_elapsed_ns();
    let revision = match adapter.plugin_latency_snapshot() {
        Some(snapshot) => snapshot.revision,
        None => {
            errors.push("no coherent latency snapshot before first callback".into());
            0
        }
    };
    adapter.set_expected_latency_revision(revision);
    let before = adapter.bridge_stats();
    let mut left = [0.0_f32; 960];
    let mut right = [0.0_f32; 960];
    let mut pcm = Vec::with_capacity(NOIRE_FRAMES * 2);
    let mut callback_receipts = Vec::with_capacity(NOIRE_FRAMES / phase.callback_frames);
    let mut cadence_overruns = 0;
    let mut maximum_lateness_ms = 0.0_f64;
    let started = Instant::now();
    let first_callback_ns = observation.monotonic_elapsed_ns();
    let mut content_frame = 0;
    if errors.is_empty() {
        while content_frame < NOIRE_FRAMES {
            // Absolute time avoids cumulative sleep drift. This is a driver
            // cadence, not a hardware callback acceptance or a timing guarantee.
            let due = started + Duration::from_secs_f64(content_frame as f64 / NOIRE_RATE as f64);
            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                thread::sleep(wait);
            }
            let call_started = Instant::now();
            let late_ms = call_started.saturating_duration_since(due).as_secs_f64() * 1000.0;
            maximum_lateness_ms = maximum_lateness_ms.max(late_ms);
            cadence_overruns +=
                usize::from(late_ms > phase.callback_frames as f64 * 1000.0 / NOIRE_RATE as f64);
            let mut event = None;
            if content_frame == 0 {
                event = Some(FrameEvent::midi(0, Some(0), [0x90, 60, 25]));
            } else if content_frame <= NOIRE_NOTE_OFF
                && NOIRE_NOTE_OFF < content_frame + phase.callback_frames
            {
                event = Some(FrameEvent::midi(
                    (NOIRE_NOTE_OFF - content_frame) as u16,
                    Some(0),
                    [0x80, 60, 0],
                ));
            }
            adapter.set_transport(
                PluginTransport {
                    sample_position: content_frame as i64,
                    quarter_note_position: content_frame as f64 / NOIRE_RATE as f64 * 80.0 / 60.0,
                    tempo: 80.0,
                    playing: true,
                    ..PluginTransport::default()
                },
                NOIRE_RATE as f64,
            );
            let status = adapter.process_generator(
                epoch,
                phase.callback_frames,
                &mut left,
                &mut right,
                event.as_slice(),
            );
            let duration_ms = call_started.elapsed().as_secs_f64() * 1000.0;
            callback_receipts.push(serde_json::json!({
                "content_frame": content_frame, "lateness_ms": late_ms,
                "driver_and_adapter_duration_ms": duration_ms,
                "expected_sequence": adapter.expected_sequence(),
            }));
            for (&l, &r) in left[..phase.callback_frames]
                .iter()
                .zip(&right[..phase.callback_frames])
            {
                pcm.extend_from_slice(&[l, r]);
            }
            content_frame += phase.callback_frames;
            if !matches!(status, FixedQuantumProcessStatus::Processed { .. }) {
                errors.push(format!("adapter callback rejected: {status:?}"));
                break;
            }
            errors.extend(drain_probe_faults(control));
            if let Some((reason, sequence)) = adapter.processing_fault() {
                errors.push(format!(
                    "strict processing fault: {reason:?}, expected sequence {sequence}"
                ));
                break;
            }
            if !errors.is_empty() {
                break;
            }
        }
    }
    let after = adapter.bridge_stats();
    let snapshot = adapter.genuine_snapshot();
    let metrics = inspect_noire_pcm(
        &pcm,
        NOIRE_NOTE_OFF + adapter.latency().total_frames + FRAMES,
    );
    errors.extend(noire_pcm_failures(&metrics));
    if cadence_overruns != 0 {
        errors.push(format!(
            "simulated cadence exceeded a whole callback interval {cadence_overruns} times; deadline comparison is inconclusive"
        ));
    }
    for (name, delta) in [
        (
            "deadline misses",
            after.deadline_misses.saturating_sub(before.deadline_misses),
        ),
        (
            "input gaps",
            after.input_gaps.saturating_sub(before.input_gaps),
        ),
        (
            "output overflows",
            after
                .output_overflows
                .saturating_sub(before.output_overflows),
        ),
        ("worker faults", after.faults.saturating_sub(before.faults)),
        (
            "reset faults",
            after.reset_faults.saturating_sub(before.reset_faults),
        ),
        (
            "latency drift",
            after
                .latency_drift_outputs
                .saturating_sub(before.latency_drift_outputs),
        ),
    ] {
        if delta != 0 {
            errors.push(format!("{name}: {delta}"));
        }
    }
    let pcm_path = output.join(format!("{}.wav", phase.name));
    if let Err(error) = write_probe_float_wav(&pcm_path, &pcm) {
        errors.push(format!("PCM persistence failed: {error}"));
    }
    serde_json::json!({
        "phase": phase.name, "status": if errors.is_empty() { "passed" } else { "failed" },
        "errors": errors, "epoch": epoch, "callback_frames": phase.callback_frames,
        "reset_wait_requested": phase.await_reset,
        "reset_settled_before_first_callback": settled_before_first_callback,
        "reset_request_begin_ns": requested_ns, "reset_request_returned_ns": reset_requested_ns,
        "reset_observed_ns": settled_ns, "first_callback_ns": first_callback_ns,
        "expected_latency_revision": revision, "adapter_latency_frames": adapter.latency().total_frames,
        "note": { "key": 60, "velocity": 25, "note_on_frame": 0, "note_off_frame": NOIRE_NOTE_OFF },
        "pcm": metrics, "pcm_path": pcm_path, "callbacks": callback_receipts,
        "cadence_overruns": cadence_overruns, "maximum_driver_lateness_ms": maximum_lateness_ms,
        "release_scope": "1.5 seconds observed after note-off, excluding bridge delay; complete tail not qualified",
        "terminal_worker_snapshot": {
            "worker_id": snapshot.bridge.worker_id, "worker_epoch": snapshot.bridge.worker_epoch,
            "input_consumed": snapshot.bridge.input_consumed,
            "operations_completed": snapshot.bridge.operations_completed,
            "worker_busy": snapshot.bridge.worker_busy,
            "strict_timing": snapshot.bridge.strict_timing,
            "submitted_cumulative": after.submitted, "completed_cumulative": after.completed,
        },
    })
}

#[test]
#[ignore = "requires stopped native DAW and explicit private CITRUS_NOIRE_FIRST_BLOCK_QA_{DIR,HELPER,STATE}; actual locally verified NOIRE state"]
fn windows_noire_actual_first_sequence_callback_cadence() {
    let output = absolute_env("CITRUS_NOIRE_FIRST_BLOCK_QA_DIR");
    let helper = absolute_env("CITRUS_NOIRE_FIRST_BLOCK_QA_HELPER");
    let state_path = absolute_env("CITRUS_NOIRE_FIRST_BLOCK_QA_STATE");
    assert!(helper.is_file() && state_path.is_file());
    let initial_state = fs::read(&state_path).unwrap();
    assert!(!initial_state.is_empty());
    let initial_state_byte_len = initial_state.len();
    fs::create_dir(&output)
        .expect("use a new private probe directory; never overwrite evidence or user work");
    let uid = "5653544E694B386B6F6E74616B742038";
    let path = PathBuf::from(r"C:\Program Files\Common Files\VST3\Kontakt 8.vst3");
    assert!(path.exists());
    let mut spec = PluginLoadSpec::from_descriptor(PluginDescriptor {
        id: "explicit-local-noire-first-sequence-qa".into(),
        name: "Kontakt 8".into(),
        vendor: "Native Instruments".into(),
        path: path.clone(),
        format: PluginFormat::Vst3,
        category: "Instrument".into(),
        is_instrument: true,
        verified: true,
        scan_error: None,
        vst3_metadata: Some(Vst3ScanMetadata {
            class_uid: uid.into(),
            category: "Instrument".into(),
            has_midi_input: true,
            has_midi_output: true,
        }),
    });
    spec.class_uid = Some(uid.into());
    spec.vst3_helper_path = Some(helper.clone());
    spec.initial_state = initial_state;
    let observation = ObservationRun::install().unwrap();
    observation.set_stage(Stage::InitialSource).unwrap();
    let chain = PluginChain::spawn_identified(
        vec![(1, spec)],
        PluginPrepareConfig {
            sample_rate: NOIRE_RATE as f64,
            max_block_frames: 2048,
        },
    )
    .unwrap();
    let worker_id = chain.guard.genuine_worker_id();
    let PluginChain {
        audio,
        mut control,
        mut guard,
    } = chain;
    let mut adapter = FixedQuantumAdapter::new(audio, FRAMES).unwrap();
    let plan = PreparedPluginTimingPlan::conservative(NOIRE_RATE as u32).unwrap();
    assert_eq!(plan.lookahead_quanta, 18);
    assert!(adapter.configure_timing_plan(plan));
    let mut ready = None;
    let mut setup_error = None;
    let readiness_deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match control.try_next_event() {
            Some(RuntimeEvent::SlotReady {
                slot: 0,
                name,
                latency_samples,
                tail_samples,
            }) => {
                if name.split(" — ").next() != Some("Kontakt 8") {
                    setup_error = Some(format!("unexpected ready identity: {name:?}"));
                }
                ready = Some(
                    serde_json::json!({"name": name, "latency_samples": latency_samples, "tail_samples": tail_samples}),
                );
                break;
            }
            Some(RuntimeEvent::SlotFault { message, .. }) => {
                setup_error = Some(message);
                break;
            }
            _ if Instant::now() < readiness_deadline => thread::sleep(Duration::from_millis(1)),
            _ => {
                setup_error = Some("actual NOIRE-state worker readiness timed out".into());
                break;
            }
        }
    }
    let mut phases = Vec::new();
    if setup_error.is_none() {
        for (index, phase) in [
            NoirePhase {
                name: "480-immediate-reset",
                callback_frames: 480,
                await_reset: false,
            },
            NoirePhase {
                name: "480-settled-reset",
                callback_frames: 480,
                await_reset: true,
            },
            NoirePhase {
                name: "960-immediate-reset",
                callback_frames: 960,
                await_reset: false,
            },
            NoirePhase {
                name: "960-settled-reset",
                callback_frames: 960,
                await_reset: true,
            },
        ]
        .into_iter()
        .enumerate()
        {
            if index != 0 {
                // Clear a previous phase's MIDI safety latch, then establish an
                // idle baseline without saving state or submitting silent DSP.
                let baseline_epoch = adapter.epoch() + 1;
                assert!(adapter.set_epoch(baseline_epoch));
                if let Err(error) =
                    await_noire_reset_settlement(&adapter, &mut control, baseline_epoch)
                {
                    setup_error = Some(error);
                    break;
                }
            }
            phases.push(noire_cadence_phase(
                &mut adapter,
                &mut control,
                &observation,
                phase,
                &output,
            ));
        }
    }
    observation.set_stage(Stage::Cleanup).unwrap();
    let shutdown_deadline = Instant::now() + Duration::from_secs(30);
    let shutdown = loop {
        let progress = guard.poll_shutdown_retained();
        if progress != RetainedShutdownPoll::Pending || Instant::now() >= shutdown_deadline {
            break progress;
        }
        thread::sleep(Duration::from_millis(1));
    };
    let records = observation.finish();
    let mut worker_trace = Vec::new();
    for record in &records.records {
        if let RecordKind::Worker {
            site,
            worker_id,
            manifest,
            epoch,
            sequence,
            stats,
        } = record.kind
            && (sequence == Some(1) || sequence.is_none())
        {
            worker_trace.push(serde_json::json!({
                "ordinal": record.ordinal, "monotonic_elapsed_ns": record.monotonic_elapsed_ns,
                "site": site, "worker_id": worker_id, "instance_id": manifest.instance_id(0),
                "epoch": epoch, "sequence": sequence, "submitted": stats.submitted, "completed": stats.completed,
            }));
        }
    }
    let save_attempts = records
        .records
        .iter()
        .filter(|r| {
            matches!(
                r.kind,
                RecordKind::Call {
                    operation: Operation::SaveState,
                    boundary: Boundary::Attempt,
                    ..
                }
            )
        })
        .count();
    let joined = records.records.iter().any(|r| matches!(r.kind,
        RecordKind::WorkerJoin { worker_id: actual, outcome: WorkerJoinOutcome::Joined } if actual == worker_id));
    let passed = setup_error.is_none()
        && phases.len() == 4
        && phases.iter().all(|phase| phase["status"] == "passed")
        && records.failure.is_none()
        && save_attempts == 0
        && shutdown == RetainedShutdownPoll::Joined
        && joined;
    let receipt = serde_json::json!({
        "status": if passed { "passed" } else { "failed" },
        "scope": "actual production Vst3Backend/worker and strict Q128 adapter under simulated callback cadence; diagnostic only",
        "plugin": path, "class_uid": uid, "helper": helper, "initial_state": state_path,
        "initial_state_byte_len": initial_state_byte_len,
        "input_state_identity": "external native NOIRE receipt required; this probe cannot identify a preset from opaque bytes",
        "ready": ready, "setup_error": setup_error, "phases": phases,
        "plan": { "B": plan.callback_budget_frames, "Q": FRAMES, "K": plan.lookahead_quanta,
            "sample_rate": plan.sample_rate, "bridge_latency_frames": plan.bridge_latency_frames },
        "worker_trace": worker_trace, "observer_failure": records.failure, "save_state_attempts": save_attempts,
        "retained_shutdown": format!("{shutdown:?}"), "worker_join_observed": joined,
        "helper_reaped": "worker join does not independently attest subprocess reap",
        "device_free": true, "native_callback_deadline_acceptance": false, "noire_qualified": false,
        "physical_monitoring": "not tested", "cold_app_restore": "not tested",
        "complete_tail": "not tested", "clip_conversion": "not tested", "offline_export": "not tested",
        "timing_limit": "observer locks/timestamps and Windows driver sleep scheduling affect measured latency; no callback collector or SDK calls added",
    });
    fs::write(
        output.join("result.json"),
        serde_json::to_vec_pretty(&receipt).unwrap(),
    )
    .unwrap();
    println!(
        "first-sequence diagnostic: {}; inspect {}",
        receipt["status"],
        output.join("result.json").display()
    );
    assert!(
        passed,
        "NOIRE first-sequence probe failed; preserve {}",
        output.join("result.json").display()
    );
}

#[test]
fn noire_first_sequence_pcm_oracle_rejects_silence_nonfinite_clipping_and_missing_release() {
    let silence = vec![0.0; NOIRE_FRAMES * 2];
    assert!(
        noire_pcm_failures(&inspect_noire_pcm(&silence, NOIRE_NOTE_OFF))
            .iter()
            .any(|error| error.contains("silent"))
    );
    let mut audible = (0..NOIRE_FRAMES)
        .flat_map(|frame| {
            let sample =
                ((frame as f32 / NOIRE_RATE as f32) * 440.0 * std::f32::consts::TAU).sin() * 0.01;
            [sample, sample]
        })
        .collect::<Vec<_>>();
    assert!(noire_pcm_failures(&inspect_noire_pcm(&audible, NOIRE_NOTE_OFF)).is_empty());
    audible[0] = f32::NAN;
    assert!(
        noire_pcm_failures(&inspect_noire_pcm(&audible, NOIRE_NOTE_OFF))
            .iter()
            .any(|error| error.contains("nonfinite"))
    );
    audible[0] = 1.0;
    assert!(
        noire_pcm_failures(&inspect_noire_pcm(&audible, NOIRE_NOTE_OFF))
            .iter()
            .any(|error| error.contains("clipping"))
    );
    audible[0] = 0.0;
    audible[NOIRE_NOTE_OFF * 2..].fill(0.0);
    assert!(
        noire_pcm_failures(&inspect_noire_pcm(&audible, NOIRE_NOTE_OFF))
            .iter()
            .any(|error| error.contains("release"))
    );
}
