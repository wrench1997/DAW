//! Test-only line-protocol stand-in. No VST binary, production helper or native UI is loaded.
use super::*;
use std::{
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);
const UID: &str = "ABCDEF019182FAEB566D624153675854";

/// Compile-checked complete replies, round-tripped through the real decoder before any helper
/// is launched. Failure modes mutate one intentional field/stage of these valid templates.
fn response_templates() -> String {
    use vst3_host::{
        plugin::{CheckedAudioBusConfig, CheckedAudioBusLayout, EventBusConfig, EventBusLayout},
        process_isolation::HostResponse as R,
    };
    let responses = BTreeMap::from([
        (
            "LoadPlugin",
            R::PluginInfo {
                vendor: "Fixture".into(),
                name: "Fixture".into(),
                version: "1.3.4".into(),
                category: "Instrument|Synth".into(),
                uid: UID.into(),
                has_gui: true,
                audio_inputs: 0,
                audio_outputs: 1,
                output_channels: 2,
                has_midi_input: true,
                has_midi_output: false,
                compatibility: vec![],
            },
        ),
        (
            "AudioBusLayout",
            R::AudioBusLayout {
                layout: vst3_host::audio::AudioBusLayout {
                    inputs: vec![],
                    outputs: vec![vst3_host::audio::AudioBusConfig {
                        channel_count: 2,
                        active: true,
                    }],
                },
            },
        ),
        (
            "CheckedAudioBusLayout",
            R::CheckedAudioBusLayout {
                layout: CheckedAudioBusLayout {
                    inputs: vec![],
                    outputs: vec![CheckedAudioBusConfig {
                        channel_count: 2,
                        is_main: true,
                        active: Some(true),
                    }],
                },
            },
        ),
        (
            "EventBusLayout",
            R::EventBusLayout {
                layout: EventBusLayout {
                    inputs: vec![EventBusConfig {
                        channel_count: 16,
                        is_main: true,
                        active: Some(true),
                    }],
                    outputs: vec![],
                },
            },
        ),
        (
            "ResetOriginSupport",
            R::ResetOriginSupport {
                support: vst3_host::ResetOriginSupport {
                    contract_version: 1,
                    max_block_frames: 2048,
                    processing: false,
                },
            },
        ),
        (
            "Editor",
            R::EditorState {
                state: vst3_host::IsolatedEditorState {
                    supported: true,
                    has_editor: true,
                    open: false,
                    width: 0,
                    height: 0,
                    generation: 2,
                },
            },
        ),
        (
            "NativeDirtyRevision",
            R::NativeDirtyRevision { revision: 9 },
        ),
        ("LatencySamples", R::LatencySamples { samples: 0 }),
        ("TailSamples", R::TailSamples { samples: 0 }),
        (
            "GetAllParameters",
            R::Parameters {
                params: vec![vst3_host::Parameter {
                    id: 17,
                    name: "Gain".into(),
                    value: 0.75,
                    min: 0.0,
                    max: 1.0,
                    default: 0.5,
                    unit: String::new(),
                    step_count: 0,
                    can_automate: true,
                    is_read_only: false,
                    is_bypass: false,
                    flags: 0,
                }],
            },
        ),
        ("GetParameter", R::ParameterValue { value: 0.75 }),
        (
            "SaveState",
            R::State {
                data: vec![1, 2, 3],
            },
        ),
        ("EmptyState", R::State { data: vec![] }),
        (
            "Success",
            R::Success {
                message: "ok".into(),
            },
        ),
        (
            "OldHelper",
            R::Error {
                message: "unsupported command".into(),
            },
        ),
        (
            "SdkError",
            R::Error {
                message: "getBusInfo failed".into(),
            },
        ),
        (
            "Unexpected",
            R::Error {
                message: "forbidden/unimplemented command".into(),
            },
        ),
        (
            "CaptureError",
            R::Error {
                message: "injected capture IPC failure".into(),
            },
        ),
        (
            "Unsupported",
            R::MetadataQueryRejected {
                reason: vst3_host::MetadataQueryRejection::Unsupported,
            },
        ),
        (
            "PendingTopology",
            R::MetadataQueryRejected {
                reason: vst3_host::MetadataQueryRejection::PendingTopology,
            },
        ),
        (
            "InconsistentTopology",
            R::MetadataQueryRejected {
                reason: vst3_host::MetadataQueryRejection::InconsistentTopology,
            },
        ),
    ]);
    let encoded = serde_json::to_string(&responses).unwrap();
    let decoded: BTreeMap<String, R> = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded.len(), responses.len());
    encoded
}

struct FakeHelper {
    dir: PathBuf,
    script: PathBuf,
    log: PathBuf,
    plugin: PathBuf,
}

impl FakeHelper {
    fn new(mode: &str) -> Self {
        let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "citrus-bounce-protocol-{}-{id}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("fake-helper");
        let log = dir.join("requests.jsonl");
        let plugin = dir.join("no-vendor-code.vst3");
        std::fs::write(&plugin, b"protocol fixture only").unwrap();
        let program = r#"#!/usr/bin/env python3
import copy, json, sys, time
from pathlib import Path
mode = MODE
log = LOG
fixture_dir = Path(FIXTURE_DIR)
capture_phase = False
capture_step = 0
templates = json.loads(TEMPLATES)
saved = False
loaded = False
processing = False
parameter_applied = False
for line in sys.stdin:
    request = json.loads(line)
    with open(log, 'a') as stream:
        stream.write(json.dumps(request) + '\n')
    name = request if isinstance(request, str) else next(iter(request))
    data = {} if isinstance(request, str) else request[name]
    result = copy.deepcopy(templates.get(name, templates['Unexpected']))
    if name == 'CheckedAudioBusLayout':
        if mode == 'old_audio_helper': result = templates['OldHelper']
        elif mode == 'audio_wrong_reply': result = templates['Success']
        elif mode == 'audio_sdk_error': result = templates['SdkError']
        elif mode == 'audio_unsupported': result = templates['Unsupported']
        elif mode == 'audio_pending': result = templates['PendingTopology']
        elif (mode == 'audio_changed_on_start' and processing) or (mode == 'audio_changed_on_parameter' and parameter_applied):
            result = templates['InconsistentTopology']
    elif name == 'EventBusLayout':
        if mode == 'old_helper': result = templates['OldHelper']
        else:
            layout = result['EventBusLayout']['layout']
            bus = layout['inputs'][0]
            if (mode == 'outputs_after_state' and loaded) or (mode == 'metadata_change' and saved): layout['outputs'] = [copy.deepcopy(bus)]
            if mode == 'extra_input': layout['inputs'].append(copy.deepcopy(bus))
            if mode == 'unknown_activation': bus['active'] = None
    elif name == 'ResetOriginSupport':
        result['ResetOriginSupport']['support']['processing'] = processing
    elif name == 'Editor':
        if data['command'] not in ('Query', 'Close'): result = templates['Unexpected']
        elif data['command'] == 'Query':
            state = result['EditorState']['state']
            state['generation'] = 3 if mode == 'generation_change' and saved else 2
            if mode == 'open': state.update(open=True, width=320, height=240)
    elif name == 'NativeDirtyRevision':
        if mode == 'revision_change' and saved: result['NativeDirtyRevision']['revision'] = 10
    elif name == 'GetAllParameters':
        if mode == 'unknown_base': result['Parameters']['params'] = []
        if mode.startswith('block_'):
            second = copy.deepcopy(result['Parameters']['params'][0])
            second['id'] = 18
            result['Parameters']['params'].append(second)
    elif name == 'SaveState':
        saved = True
        if mode == 'empty_state': result = templates['EmptyState']
    elif name in ('SetProcessMode','LoadState','StartProcessing','StopProcessing','SetParameter','Shutdown','UnloadPlugin'):
        if name == 'SetParameter': parameter_applied = True
        if name == 'LoadState': loaded = True
        if name == 'StartProcessing': processing = True
        if name == 'StopProcessing': processing = False
        result = templates['Success']
    if name == 'Editor' and data['command'] == 'Query': capture_phase = True
    if mode.startswith('block_') and capture_phase:
        capture_step += 1
        target = int(mode.split('_')[1])
        if capture_step == target:
            (fixture_dir / 'entered').write_text(name)
            wait_until = time.monotonic() + 4
            while not (fixture_dir / 'release').exists() and time.monotonic() < wait_until:
                time.sleep(0.001)
            if not (fixture_dir / 'release').exists() or mode.endswith('_fault'):
                result = templates['CaptureError']
    print(json.dumps(result), flush=True)
    if name == 'Shutdown': break
"#;
        let program = program
            .replace(
                "TEMPLATES",
                &serde_json::to_string(&response_templates()).unwrap(),
            )
            .replace("FIXTURE_DIR", &serde_json::to_string(&dir).unwrap())
            .replace("MODE", &serde_json::to_string(mode).unwrap())
            .replace("LOG", &serde_json::to_string(&log).unwrap());
        std::fs::write(&script, program).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self {
            dir,
            script,
            log,
            plugin,
        }
    }

    fn spec(&self, state: Vec<u8>) -> super::super::PluginLoadSpec {
        super::super::PluginLoadSpec {
            descriptor: super::super::PluginDescriptor {
                id: "path-id".into(),
                name: "Fixture".into(),
                vendor: "Fixture".into(),
                path: self.plugin.clone(),
                format: super::super::PluginFormat::Vst3,
                category: "Instrument".into(),
                is_instrument: true,
                verified: true,
                vst3_metadata: None,
                scan_error: None,
            },
            class_uid: Some(UID.into()),
            vst3_helper_path: Some(self.script.clone()),
            initial_state: state,
            enabled: true,
            bypassed: false,
            wet: 1.0,
        }
    }

    fn source(&self) -> Vst3Backend {
        let config = PluginPrepareConfig {
            sample_rate: 48_000.0,
            max_block_frames: 2048,
        };
        let mut source = Vst3Backend::load(self.spec(vec![]), config).unwrap();
        source.prepare(config).unwrap();
        std::fs::write(&self.log, b"").unwrap();
        source
    }

    fn requests(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn names(&self) -> Vec<String> {
        self.requests()
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.as_object().unwrap().keys().next().unwrap().clone())
            })
            .collect()
    }
}

impl Drop for FakeHelper {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn capture_state() -> BouncePluginState {
    BouncePluginState {
        metadata: BouncePluginMetadata {
            class_uid: UID.into(),
            version: "1.3.4".into(),
            name: "Fixture".into(),
            category: "Instrument|Synth".into(),
            event_input_buses: vec![(16, true, Some(true))],
            event_output_buses: vec![],
            input_buses: vec![],
            output_buses: vec![(2, true)],
            latency_samples: 0,
            tail_samples: 0,
        },
        opaque_state: vec![1, 2, 3],
        parameter_bases: BTreeMap::from([(17, 0.75)]),
        native_generation: 2,
        native_dirty_revision: 9,
    }
}

const CAPTURE_SEQUENCE: &[&str] = &[
    "Editor",
    "NativeDirtyRevision",
    "CheckedAudioBusLayout",
    "EventBusLayout",
    "LatencySamples",
    "TailSamples",
    "SaveState",
    "GetAllParameters",
    "GetParameter",
    "Editor",
    "NativeDirtyRevision",
    "CheckedAudioBusLayout",
    "EventBusLayout",
    "LatencySamples",
    "TailSamples",
];

fn capture_fence() -> BounceCaptureFence {
    BounceCaptureFence::new(
        std::time::Instant::now() + crate::instrument_bounce::BOUNCE_CAPTURE_TIMEOUT,
    )
}

fn capture_control() {
    let fake = FakeHelper::new("normal");
    let mut source = fake.source();
    assert_eq!(
        source
            .capture_bounce_state(&[17], &capture_fence())
            .unwrap(),
        capture_state()
    );
    assert_eq!(fake.names(), CAPTURE_SEQUENCE);
}

fn factory_control() {
    let fake = FakeHelper::new("normal");
    let state = capture_state();
    let owner =
        OfflineInstrument::load_snapshot(fake.spec(state.opaque_state.clone()), 48_000, &state)
            .unwrap();
    assert_eq!(owner.metadata(), &state.metadata);
}

fn assert_capture_failure(
    fake: &FakeHelper,
    result: Result<BouncePluginState, BounceCaptureError>,
    harmless: bool,
    message: &str,
    prefix_len: usize,
) {
    let error = result.unwrap_err();
    assert_eq!(
        matches!(error, BounceCaptureError::Rejected(_)),
        harmless,
        "{error}"
    );
    assert!(error.to_string().contains(message), "{error}");
    assert!(
        !error.to_string().contains("timed out"),
        "unexpected protocol timeout: {error}"
    );
    assert_eq!(fake.names(), &CAPTURE_SEQUENCE[..prefix_len]);
    assert!(fake.requests().iter().all(|request| {
        request
            .get("Editor")
            .is_none_or(|data| data["command"] == "Query")
    }));
}

fn assert_factory_failure(fake: &FakeHelper, message: &str, last_stage: &str) {
    let state = capture_state();
    let error =
        OfflineInstrument::load_snapshot(fake.spec(state.opaque_state.clone()), 48_000, &state)
            .err()
            .expect("injection must reject");
    assert!(error.contains(message), "{error}");
    assert!(
        !error.contains("timed out"),
        "unexpected protocol timeout: {error}"
    );
    let names = fake.names();
    let cleanup = names
        .iter()
        .position(|name| name == "Editor")
        .expect("candidate Drop closes its editor");
    let setup = &names[..cleanup];
    assert_eq!(setup.last().map(String::as_str), Some(last_stage));
    let index = |wanted: &str| setup.iter().position(|name| name == wanted).unwrap();
    assert!(index("SetProcessMode") < index("LoadState"));
    assert!(index("LoadState") < index("StartProcessing"));
    assert!(index("StartProcessing") < index("SetParameter"));
    assert!(index("SetParameter") < index("CheckedAudioBusLayout"));
    assert!(!names.iter().any(|name| matches!(
        name.as_str(),
        "Process" | "ProcessBuses" | "ProcessResetOrigin" | "SaveState"
    )));
    assert!(names[cleanup..].iter().all(|name| matches!(
        name.as_str(),
        "Editor" | "StopProcessing" | "Shutdown" | "UnloadPlugin"
    )));
    assert!(fake.requests().iter().all(|request| {
        request
            .get("Editor")
            .is_none_or(|data| data["command"] == "Close")
    }));
}

#[test]
fn actual_vst3_capture_queries_closed_state_current_bases_and_all_fences_only() {
    let fake = FakeHelper::new("normal");
    let mut source = fake.source();
    let state = source
        .capture_bounce_state(&[17], &capture_fence())
        .unwrap();
    assert_eq!(state, capture_state());
    assert_eq!(
        fake.names(),
        [
            "Editor",
            "NativeDirtyRevision",
            "CheckedAudioBusLayout",
            "EventBusLayout",
            "LatencySamples",
            "TailSamples",
            "SaveState",
            "GetAllParameters",
            "GetParameter",
            "Editor",
            "NativeDirtyRevision",
            "CheckedAudioBusLayout",
            "EventBusLayout",
            "LatencySamples",
            "TailSamples"
        ]
    );
    assert!(source.prepared);
    assert!(!source.last_transport.playing);
    assert!(source.plugin.is_processing());
}

#[test]
fn actual_vst3_capture_rejects_open_editor_unknown_keys_empty_state_and_changed_fences() {
    capture_control();
    for (mode, harmless, message, prefix) in [
        ("open", true, "Close the source native editor", 1),
        (
            "unknown_base",
            true,
            "Stored parameter base 17 is missing or read-only",
            8,
        ),
        (
            "empty_state",
            true,
            "requires nonempty authoritative state",
            7,
        ),
        (
            "generation_change",
            true,
            "Source native generation, state revision or metadata changed",
            11,
        ),
        (
            "revision_change",
            true,
            "Source native generation, state revision or metadata changed",
            11,
        ),
        (
            "metadata_change",
            true,
            "Source native generation, state revision or metadata changed",
            15,
        ),
        (
            "old_helper",
            false,
            "EventBusLayout: unsupported command",
            4,
        ),
        ("extra_input", true, "requires an actual VST3 instrument", 6),
        (
            "unknown_activation",
            true,
            "requires an actual VST3 instrument",
            6,
        ),
    ] {
        let fake = FakeHelper::new(mode);
        let mut source = fake.source();
        assert_capture_failure(
            &fake,
            source.capture_bounce_state(&[17], &capture_fence()),
            harmless,
            message,
            prefix,
        );
    }
}

#[test]
fn actual_fresh_factory_selects_offline_before_restore_and_start_without_warmup_or_resave() {
    let fake = FakeHelper::new("normal");
    let state = capture_state();
    let owner =
        OfflineInstrument::load_snapshot(fake.spec(state.opaque_state.clone()), 48_000, &state)
            .unwrap();
    assert_eq!(owner.metadata(), &state.metadata);
    let names = fake.names();
    let index = |wanted: &str| names.iter().position(|name| name == wanted).unwrap();
    assert!(index("SetProcessMode") < index("LoadState"));
    assert!(index("LoadState") < index("StartProcessing"));
    assert!(index("StartProcessing") < index("SetParameter"));
    assert!(index("SetParameter") < index("EventBusLayout"));
    assert!(!names.iter().any(|name| matches!(
        name.as_str(),
        "Process" | "ProcessBuses" | "ProcessResetOrigin" | "SaveState" | "Editor"
    )));
    let requests = fake.requests();
    assert!(requests.iter().any(|request| {
        request
            .get("SetProcessMode")
            .is_some_and(|data| data["mode"] == "Offline")
    }));
    assert!(requests.iter().any(|request| {
        request
            .get("SetParameter")
            .is_some_and(|data| data["id"] == 17 && data["value"] == 0.75)
    }));
}

#[test]
fn actual_fresh_factory_rejects_post_restore_event_output_and_old_helper() {
    factory_control();
    for (mode, message, stage) in [
        (
            "outputs_after_state",
            "requires an actual VST3 instrument",
            "TailSamples",
        ),
        (
            "old_helper",
            "EventBusLayout: unsupported command",
            "EventBusLayout",
        ),
        (
            "extra_input",
            "requires an actual VST3 instrument",
            "TailSamples",
        ),
        (
            "unknown_activation",
            "requires an actual VST3 instrument",
            "TailSamples",
        ),
    ] {
        let fake = FakeHelper::new(mode);
        assert_factory_failure(&fake, message, stage);
    }
}

#[test]
fn capture_metadata_typed_refusal_is_harmless_but_unknown_remote_errors_fault() {
    capture_control();
    for (mode, harmless, message, prefix) in [
        (
            "audio_unsupported",
            true,
            "Metadata query rejected: Unsupported",
            3,
        ),
        (
            "audio_pending",
            true,
            "Metadata query rejected: PendingTopology",
            3,
        ),
        (
            "audio_changed_on_start",
            true,
            "Metadata query rejected: InconsistentTopology",
            3,
        ),
        (
            "old_audio_helper",
            false,
            "CheckedAudioBusLayout: unsupported command",
            3,
        ),
        (
            "old_helper",
            false,
            "EventBusLayout: unsupported command",
            4,
        ),
        (
            "audio_wrong_reply",
            false,
            "CheckedAudioBusLayout: unexpected response",
            3,
        ),
        (
            "audio_sdk_error",
            false,
            "CheckedAudioBusLayout: getBusInfo failed",
            3,
        ),
    ] {
        let fake = FakeHelper::new(mode);
        let mut source = fake.source();
        assert_capture_failure(
            &fake,
            source.capture_bounce_state(&[17], &capture_fence()),
            harmless,
            message,
            prefix,
        );
        assert!(source.prepared);
    }
}

#[test]
fn fresh_factory_rejects_audio_changes_at_start_or_parameter_application_and_sdk_errors() {
    factory_control();
    for (mode, message) in [
        (
            "audio_changed_on_start",
            "Metadata query rejected: InconsistentTopology",
        ),
        (
            "audio_changed_on_parameter",
            "Metadata query rejected: InconsistentTopology",
        ),
        (
            "audio_sdk_error",
            "CheckedAudioBusLayout: getBusInfo failed",
        ),
        (
            "audio_wrong_reply",
            "CheckedAudioBusLayout: unexpected response",
        ),
        (
            "old_audio_helper",
            "CheckedAudioBusLayout: unsupported command",
        ),
        ("audio_pending", "Metadata query rejected: PendingTopology"),
    ] {
        let fake = FakeHelper::new(mode);
        assert_factory_failure(&fake, message, "CheckedAudioBusLayout");
    }
}

#[derive(Clone, Copy)]
enum CaptureInterruption {
    Cancel,
    Deadline,
    CancelWithFault,
}

/// The fixture keeps its source owner alive until assertions finish, so the request ledger
/// proves capture termination without confusing source teardown with further capture calls.
fn interrupt_capture_at(stage: usize, interruption: CaptureInterruption, ids: &[u32]) {
    use std::{
        sync::{Arc, mpsc},
        thread,
        time::{Duration, Instant},
    };
    let fault = matches!(interruption, CaptureInterruption::CancelWithFault);
    let fake = FakeHelper::new(&format!(
        "block_{stage}_{}",
        if fault { "fault" } else { "ok" }
    ));
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let (result_tx, result_rx) = mpsc::sync_channel(1);
    let (finish_tx, finish_rx) = mpsc::sync_channel(1);
    thread::scope(|scope| {
        let source_fixture = &fake;
        let owner = scope.spawn(move || {
            let mut source = source_fixture.source();
            let deadline = Instant::now()
                + if matches!(interruption, CaptureInterruption::Deadline) {
                    Duration::from_secs(2)
                } else {
                    crate::instrument_bounce::BOUNCE_CAPTURE_TIMEOUT
                };
            let fence = Arc::new(BounceCaptureFence::new(deadline));
            ready_tx.send(Arc::clone(&fence)).unwrap();
            let result = source.capture_bounce_state(ids, &fence);
            result_tx
                .send((result, source.prepared, source.plugin.is_processing()))
                .unwrap();
            // A failed test cannot hold the scoped owner forever.
            let _ = finish_rx.recv_timeout(Duration::from_secs(8));
        });
        let fence = ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let wait_until = Instant::now() + Duration::from_secs(3);
        while !fake.dir.join("entered").exists() && Instant::now() < wait_until {
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(
            std::fs::read_to_string(fake.dir.join("entered")).unwrap(),
            CAPTURE_SEQUENCE[stage - 1]
        );
        if matches!(interruption, CaptureInterruption::Deadline) {
            thread::sleep(
                fence.deadline.saturating_duration_since(Instant::now()) + Duration::from_millis(5),
            );
        } else {
            fence.cancel();
        }
        std::fs::write(fake.dir.join("release"), b"release").unwrap();
        let (result, prepared, processing) =
            result_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(
            prepared && processing,
            "source lifecycle must remain untouched"
        );
        let reason = if fault {
            "injected capture IPC failure"
        } else if matches!(interruption, CaptureInterruption::Deadline) {
            "timed out"
        } else {
            "cancelled"
        };
        let error = result.unwrap_err();
        assert_eq!(
            matches!(error, BounceCaptureError::BackendFailed(_)),
            fault,
            "{error}"
        );
        assert!(error.to_string().contains(reason), "{error}");
        assert_eq!(fake.names(), &CAPTURE_SEQUENCE[..stage]);
        // No second parameter request or trailing fence is permitted after cancellation/deadline.
        assert!(
            fake.requests()
                .iter()
                .filter_map(|request| request.get("GetParameter"))
                .all(|parameter| parameter["id"] == 17)
        );
        assert!(fake.requests().iter().all(|request| {
            request
                .get("Editor")
                .is_none_or(|data| data["command"] == "Query")
        }));
        finish_tx.send(()).unwrap();
        owner.join().unwrap();
    });
}

#[test]
fn capture_cancellation_stops_after_each_inflight_state_metadata_and_parameter_stage() {
    capture_control();
    for stage in 1..=CAPTURE_SEQUENCE.len() {
        interrupt_capture_at(stage, CaptureInterruption::Cancel, &[17]);
    }
}

#[test]
fn slow_successful_base_getter_observes_deadline_before_any_next_request() {
    capture_control();
    interrupt_capture_at(9, CaptureInterruption::Deadline, &[17, 18]);
}

#[test]
fn inflight_state_and_getter_ipc_failures_win_over_concurrent_cancellation() {
    capture_control();
    for stage in [7, 9] {
        interrupt_capture_at(stage, CaptureInterruption::CancelWithFault, &[17, 18]);
    }
}

#[test]
fn already_cancelled_or_expired_backend_capture_sends_no_request() {
    capture_control();
    for cancelled in [false, true] {
        let fake = FakeHelper::new("normal");
        let mut source = fake.source();
        let fence = if cancelled {
            capture_fence()
        } else {
            BounceCaptureFence::new(std::time::Instant::now())
        };
        if cancelled {
            fence.cancel();
        }
        let result = source.capture_bounce_state(&[17], &fence);
        let expected = if cancelled { "cancelled" } else { "timed out" };
        assert!(
            matches!(result, Err(BounceCaptureError::Rejected(reason)) if reason.contains(expected))
        );
        assert!(fake.names().is_empty());
    }
}
