//! Test-only line-protocol stand-in. No VST binary, production helper or native UI is loaded.
use super::*;
use crate::instrument_bounce::BounceAudioBus;
use std::{
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
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
        ("TakeParameterEdits", R::ParameterEdits { edits: vec![] }),
        (
            "TakeHostNotifications",
            R::HostNotifications {
                notifications: vec![],
            },
        ),
        ("ServiceHostRequests", R::RestartFlags { bits: 0 }),
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
    preserve_evidence: AtomicBool,
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
import atexit, copy, json, sys, time
from pathlib import Path
mode = MODE
log = LOG
fixture_dir = Path(FIXTURE_DIR)
def publish_entered(value):
    # Publish existence only after the complete exact stage has been written/closed.
    marker = fixture_dir / 'entered'
    temporary = fixture_dir / 'entered.tmp'
    assert not marker.exists(), 'duplicate held-stage marker'
    with temporary.open('x') as stream:
        stream.write(value)
    temporary.rename(marker)
capture_phase = False
capture_step = 0
templates = json.loads(TEMPLATES)
saved = False
loaded = False
processing = False
parameter_applied = False
acknowledged = set()
activation_calls = 0
post_start_fence_held = False
timing = json.loads((fixture_dir / 'timing.json').read_text()) if (fixture_dir / 'timing.json').exists() else None
timing_reads = dict(LatencySamples=0, TailSamples=0)
metadata_reads = 0
metadata_stage = None
timing_held = False
if timing is not None:
    atexit.register(lambda: (fixture_dir / 'helper-exit.txt').write_text('normal interpreter exit\n'))
dry_shape = mode.startswith('dry_') or mode.startswith('fresh_')
attested = mode.startswith('dry_attested') or mode.startswith('dry_drift')
observed_roles = 'observed_roles' in mode or mode == 'fresh_role_diagnostic'
input_aux = observed_roles or mode in ('fresh_input_aux', 'dry_attested_input_aux')
extra_main = observed_roles or mode in ('fresh_extra_main', 'fresh_main_active_last', 'dry_attested_extra_main')
for line in sys.stdin:
    request = json.loads(line)
    with open(log, 'a') as stream:
        stream.write(json.dumps(request) + '\n')
    name = request if isinstance(request, str) else next(iter(request))
    data = {} if isinstance(request, str) else request[name]
    timing_stage = None
    if timing is not None and processing:
        if name == 'CheckedAudioBusLayout':
            metadata_reads += 1
            metadata_stage = ('post_start' if metadata_reads == 1 else
                              'capture_first' if metadata_reads == 2 else 'capture_final')
        if name in timing_reads:
            timing_reads[name] += 1
            timing_stage = metadata_stage or ('ready' if timing_reads[name] == 2 else
                                              'snapshot' if timing_reads[name] == 3 else 'poll')
    result = copy.deepcopy(templates.get(name, templates['Unexpected']))
    if dry_shape and name == 'LoadPlugin' and mode == 'fresh_identity':
        result['PluginInfo']['category'] = 'Fx'
    if dry_shape and name in ('AudioBusLayout', 'CheckedAudioBusLayout'):
        checked = name == 'CheckedAudioBusLayout'
        def bus(channels, main, active, index, direction):
            if checked:
                if not active and not attested and (direction, index) not in acknowledged:
                    active = None
                return dict(channel_count=channels, is_main=main, active=active)
            return dict(channel_count=channels, active=active)
        inputs = [bus(2, not input_aux, mode != 'fresh_input_inactive', 0, 'Input')]
        outputs = [bus(2, mode != 'fresh_output0_aux', True, 0, 'Output'),
                   bus(2 if observed_roles else 1, extra_main, False, 1, 'Output'),
                   bus(3 if mode in ('fresh_unsupported_last', 'fresh_role_diagnostic') else 2,
                       extra_main, mode in ('fresh_aux_active_last', 'fresh_main_active_last'), 2, 'Output')]
        if mode == 'fresh_raw_invalid_after_start' and checked and processing:
            outputs[0]['channel_count'] = 4
        if mode == 'fresh_unknown_after_start' and checked and processing:
            outputs[1]['active'] = None
        if mode == 'fresh_extra_output': outputs.append(bus(2, False, False, 3, 'Output'))
        if mode == 'fresh_prepared_mismatch' and not checked: outputs[-1]['channel_count'] = 1
        if mode == 'fresh_prepared_count' and not checked: outputs.pop()
        if mode == 'fresh_prepared_activation' and not checked: outputs[0]['active'] = False
        if mode == 'fresh_active_unknown' and checked: inputs[0]['active'] = None
        if mode == 'fresh_drift' and activation_calls: outputs[-1]['channel_count'] = 1
        if mode == 'dry_drift' and saved: outputs[-1]['channel_count'] = 1
        if checked:
            if ((mode == 'fresh_input_role_drift' and activation_calls)
                    or (mode in ('fresh_input_role_on_start', 'fresh_combined_on_start') and processing)
                    or (mode == 'dry_drift_input_role' and saved)):
                inputs[0]['is_main'] = False
            if ((mode == 'fresh_extra_role_drift' and activation_calls)
                    or (mode in ('fresh_extra_role_on_start', 'fresh_combined_on_start') and processing)
                    or (mode == 'dry_drift_extra_role' and saved)):
                outputs[-1]['is_main'] = True
        result = {name: {'layout': dict(inputs=inputs, outputs=outputs)}}
    if name == 'CheckedAudioBusLayout':
        if mode == 'old_audio_helper': result = templates['OldHelper']
        elif mode == 'audio_wrong_reply': result = templates['Success']
        elif mode == 'audio_sdk_error': result = templates['SdkError']
        elif mode == 'audio_unsupported': result = templates['Unsupported']
        elif mode == 'audio_pending': result = templates['PendingTopology']
        elif (mode in ('audio_changed_on_start', 'fresh_changed_on_start') and processing) or (mode == 'audio_changed_on_parameter' and parameter_applied):
            result = templates['InconsistentTopology']
    elif name == 'EventBusLayout':
        if mode == 'old_helper': result = templates['OldHelper']
        else:
            layout = result['EventBusLayout']['layout']
            bus = layout['inputs'][0]
            if (mode == 'outputs_after_state' and loaded) or (mode == 'metadata_change' and saved): layout['outputs'] = [copy.deepcopy(bus)]
            if mode == 'extra_input': layout['inputs'].append(copy.deepcopy(bus))
            if mode in ('unknown_activation', 'fresh_event_unknown'): bus['active'] = None
            if mode in ('fresh_event_channels_on_start', 'fresh_combined_on_start') and processing:
                bus['channel_count'] = 8
    elif name == 'LatencySamples':
        if mode in ('fresh_latency_on_start', 'fresh_combined_on_start') and processing:
            result['LatencySamples']['samples'] = 127
    elif name == 'TailSamples':
        if mode in ('fresh_tail_on_start', 'fresh_combined_on_start') and processing:
            result['TailSamples']['samples'] = 4294967295
        if mode == 'fresh_tail_during_ack' and activation_calls:
            result['TailSamples']['samples'] = 96000
    elif name == 'SetBusActive':
        assert dry_shape and not processing and not saved, 'activation outside fresh unpublished setup'
        assert data['media_type'] == 'Audio' and data['active'] is False, data
        activation_calls += 1
        if mode == 'fresh_set_fail2' and activation_calls == 2:
            result = templates['SdkError']
        else:
            if mode != 'fresh_ack_missing': acknowledged.add((data['direction'], data['bus_index']))
            result = templates['Success']
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
        result = templates['SdkError'] if name == 'LoadState' and mode == 'fresh_restore_fail' else templates['Success']
    if name == 'Editor' and data['command'] == 'Query': capture_phase = True
    if mode.startswith('block_') and capture_phase:
        capture_step += 1
        target = int(mode.split('_')[1])
        if capture_step == target:
            publish_entered(name)
            wait_until = time.monotonic() + 4
            while not (fixture_dir / 'release').exists() and time.monotonic() < wait_until:
                time.sleep(0.001)
            if not (fixture_dir / 'release').exists() or mode.endswith('_fault'):
                result = templates['CaptureError']
    if name == 'CheckedAudioBusLayout' and 'CheckedAudioBusLayout' in result:
        with (fixture_dir / 'checked-layouts.jsonl').open('a') as stream:
            stream.write(json.dumps(result['CheckedAudioBusLayout']['layout']) + '\n')
    if mode == 'fresh_observed_roles_block_post_start' and name == 'TailSamples' and processing and not post_start_fence_held:
        post_start_fence_held = True
        publish_entered(name)
        wait_until = time.monotonic() + 4
        while not (fixture_dir / 'release').exists() and time.monotonic() < wait_until:
            time.sleep(0.001)
        if not (fixture_dir / 'release').exists(): result = templates['CaptureError']
    if timing is not None and name in timing_reads:
        samples = 0 if name == 'LatencySamples' else timing['after_tail' if processing else 'before_tail']
        if timing_stage is not None and timing_stage == timing.get('change_stage') and name == timing.get('field'):
            samples = timing['observed']
        result[name]['samples'] = samples
        if timing_stage is not None and timing_stage == timing.get('error_stage') and name == timing.get('field'):
            result = templates['CaptureError']
        if timing_stage is not None and timing_stage == timing.get('hold_stage') and name == timing.get('field') and not timing_held:
            timing_held = True
            publish_entered(timing_stage + ':' + name)
            wait_until = time.monotonic() + 4
            while not (fixture_dir / 'release').exists() and time.monotonic() < wait_until:
                time.sleep(0.001)
            if not (fixture_dir / 'release').exists(): result = templates['CaptureError']
        with (fixture_dir / 'timing-replies.jsonl').open('a') as stream:
            stream.write(json.dumps(dict(stage=timing_stage, name=name, result=result)) + '\n')
        if name == 'TailSamples': metadata_stage = None
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
            preserve_evidence: AtomicBool::new(false),
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

    fn checked_layouts(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(self.dir.join("checked-layouts.jsonl"))
            .unwrap()
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
        if std::thread::panicking() || self.preserve_evidence.load(Ordering::Relaxed) {
            eprintln!(
                "Retained offline protocol fixture evidence: {}",
                self.dir.display()
            );
            return;
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The worker owns the helper, so fixture files outlive its bounded shutdown even
/// when an assertion unwinds. The production legacy shutdown may still time out
/// and detach. Retaining evidence does not prove worker join or helper reap.
#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
struct FreshSourceWorkerGuard<'a> {
    worker: Option<super::super::PluginWorkerGuard>,
    outcome: Option<super::super::ShutdownOutcome>,
    fixture: &'a FakeHelper,
}

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
impl<'a> FreshSourceWorkerGuard<'a> {
    fn new(fixture: &'a FakeHelper, worker: super::super::PluginWorkerGuard) -> Self {
        Self {
            worker: Some(worker),
            outcome: None,
            fixture,
        }
    }

    fn join(&mut self) -> super::super::ShutdownOutcome {
        use super::super::ShutdownOutcome;
        if let Some(outcome) = self.outcome {
            return outcome;
        }
        let worker = self
            .worker
            .take()
            .expect("unjoined fixture owns its worker");
        // Also release any test-owned wait before asking its owner to exit.
        if let Err(error) =
            std::fs::write(self.fixture.dir.join("release"), b"fixture owner shutdown")
        {
            self.fixture
                .preserve_evidence
                .store(true, Ordering::Relaxed);
            eprintln!("Unable to release offline protocol fixture: {error}");
        }
        let outcome = worker.shutdown_blocking(std::time::Duration::from_secs(5));
        self.outcome = Some(outcome);
        if outcome != ShutdownOutcome::Joined {
            self.fixture
                .preserve_evidence
                .store(true, Ordering::Relaxed);
        }
        if let Err(error) = std::fs::write(
            self.fixture.dir.join("worker-shutdown.txt"),
            format!("{outcome:?}\n"),
        ) {
            self.fixture
                .preserve_evidence
                .store(true, Ordering::Relaxed);
            eprintln!("Unable to save offline protocol shutdown evidence: {error}");
        }
        outcome
    }
}

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
impl Drop for FreshSourceWorkerGuard<'_> {
    fn drop(&mut self) {
        if self.worker.is_some() {
            let outcome = self.join();
            if std::thread::panicking() || outcome != super::super::ShutdownOutcome::Joined {
                eprintln!(
                    "Offline protocol worker shutdown: {outcome:?}; evidence: {}",
                    self.fixture.dir.display()
                );
            }
        }
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
            output_buses: vec![BounceAudioBus::new(2, true, Some(true))],
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

fn dry_metadata() -> BouncePluginMetadata {
    let mut metadata = capture_state().metadata;
    metadata.input_buses = vec![BounceAudioBus::new(2, true, Some(true))];
    metadata.output_buses.extend([
        BounceAudioBus::new(1, false, Some(false)),
        BounceAudioBus::new(2, false, Some(false)),
    ]);
    metadata
}

fn role_metadata(input_main: bool, extras_main: bool, stereo_extras: bool) -> BouncePluginMetadata {
    let mut metadata = dry_metadata();
    metadata.input_buses[0].is_main = input_main;
    for bus in &mut metadata.output_buses[1..] {
        bus.is_main = extras_main;
        if stereo_extras {
            bus.channel_count = 2;
        }
    }
    metadata
}

#[test]
fn capture_and_factory_preserve_attested_inactive_audio_declarations() {
    for (mode, expected) in [
        ("dry_attested", dry_metadata()),
        ("dry_attested_input_aux", role_metadata(false, false, false)),
        ("dry_attested_extra_main", role_metadata(true, true, false)),
        (
            "dry_attested_observed_roles",
            role_metadata(false, true, true),
        ),
    ] {
        let fake = FakeHelper::new(mode);
        let mut source = fake.source();
        let capture = source
            .capture_bounce_state(&[17], &capture_fence())
            .unwrap();
        assert_eq!(capture.metadata, expected, "{mode}");
        assert_eq!(fake.names(), CAPTURE_SEQUENCE, "{mode}");
        let clone_fixture = FakeHelper::new(mode);
        let clone = OfflineInstrument::load_snapshot(
            clone_fixture.spec(capture.opaque_state.clone()),
            48_000,
            &capture,
        )
        .unwrap();
        assert_eq!(clone.metadata(), &capture.metadata, "{mode}");
        assert_eq!(clone._backend.input_channels, 2);
        assert_eq!(clone._backend.output_channels, 2);
        assert!(!clone_fixture.names().iter().any(|name| matches!(
            name.as_str(),
            "SetBusActive" | "Process" | "ProcessBuses" | "ProcessResetOrigin" | "ResetOrigin"
        )));
    }
}

#[test]
fn capture_unknown_audio_activation_requires_fresh_load_without_mutation() {
    for mode in ["dry_unknown", "dry_unknown_observed_roles"] {
        let fake = FakeHelper::new(mode);
        let mut source = fake.source();
        let error = source
            .capture_bounce_state(&[17], &capture_fence())
            .unwrap_err();
        assert!(matches!(error, BounceCaptureError::Rejected(_)));
        assert!(error.to_string().contains("fresh load"), "{mode}: {error}");
        assert_eq!(fake.names(), &CAPTURE_SEQUENCE[..6]);
        let layouts = fake.checked_layouts();
        assert_eq!(layouts.len(), 1);
        assert_eq!(layouts[0]["inputs"][0]["active"], true);
        for index in [1, 2] {
            assert!(layouts[0]["outputs"][index]["active"].is_null());
        }
        if mode == "dry_unknown_observed_roles" {
            assert_eq!(layouts[0]["inputs"][0]["is_main"], false);
            assert_eq!(layouts[0]["inputs"][0]["channel_count"], 2);
            let outputs = layouts[0]["outputs"].as_array().unwrap();
            assert_eq!(outputs.len(), 3);
            for output in outputs {
                assert_eq!(output["is_main"], true);
                assert_eq!(output["channel_count"], 2);
            }
        }
        assert!(source.prepared && source.plugin.is_processing());
    }
}

#[test]
fn capture_fence_detects_even_supported_inactive_audio_bus_drift() {
    for mode in ["dry_drift", "dry_drift_input_role", "dry_drift_extra_role"] {
        let fake = FakeHelper::new(mode);
        let mut source = fake.source();
        let error = source
            .capture_bounce_state(&[17], &capture_fence())
            .unwrap_err();
        assert!(matches!(error, BounceCaptureError::Rejected(_)));
        assert!(
            error.to_string().contains("metadata changed"),
            "{mode}: {error}"
        );
        assert_eq!(fake.names(), CAPTURE_SEQUENCE);
        assert!(source.prepared && source.plugin.is_processing());
    }
}

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
fn fresh_config() -> PluginPrepareConfig {
    PluginPrepareConfig {
        sample_rate: 48_000.0,
        max_block_frames: 2048,
    }
}

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
fn fresh_timing_fixture(before: u32, after: u32, mut options: serde_json::Value) -> FakeHelper {
    options["before_tail"] = before.into();
    options["after_tail"] = after.into();
    let fake = FakeHelper::new("fresh_timing");
    std::fs::write(
        fake.dir.join("timing.json"),
        serde_json::to_vec(&options).unwrap(),
    )
    .unwrap();
    fake
}

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
const FRESH_NATIVE_POLL: &[&str] = &[
    "Editor",
    "TakeParameterEdits",
    "TakeHostNotifications",
    "ServiceHostRequests",
    "NativeDirtyRevision",
    "LatencySamples",
    "TailSamples",
];

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
const FRESH_WORKER_TEARDOWN: &[&str] = &["Editor", "Editor", "StopProcessing", "Shutdown"];

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
fn fresh_timing_error(field: &str, expected: u32, observed: u32) -> String {
    let field = match field {
        "LatencySamples" => "latency_samples",
        "TailSamples" => "tail_samples",
        _ => panic!("not a timing field: {field}"),
    };
    format!(
        "Fresh bounce source {field} changed after initial Start: expected {expected}, observed {observed}"
    )
}

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
fn wait_fresh_timing_hold(fake: &FakeHelper, stage: &str, field: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while !fake.dir.join("entered").exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_eq!(
        std::fs::read_to_string(fake.dir.join("entered")).unwrap(),
        format!("{stage}:{field}"),
        "exact in-flight timing request: {:?}",
        fake.names()
    );
}

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
fn wait_fresh_timing_ready(control: &mut super::super::PluginChainControl, tail: u32) {
    use super::super::RuntimeEvent;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut ready = 0;
    while std::time::Instant::now() < deadline {
        while let Some(event) = control.try_next_event() {
            match event {
                RuntimeEvent::SlotReady {
                    slot,
                    latency_samples,
                    tail_samples,
                    ..
                } => {
                    assert_eq!((slot, latency_samples, tail_samples), (0, 0, tail));
                    ready += 1;
                }
                RuntimeEvent::SlotFault { message, .. } => panic!("unexpected fault: {message}"),
                _ => panic!("unexpected admission event: {event:?}"),
            }
        }
        if ready != 0
            && control
                .plugin_latency_snapshot()
                .is_some_and(|snapshot| snapshot.active_mask == 1)
            && control
                .native_editor_snapshot(0)
                .is_some_and(|snapshot| snapshot.state.generation == 2 && snapshot.error.is_none())
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_eq!(ready, 1);
    let snapshot = control.plugin_latency_snapshot().unwrap();
    assert_eq!(snapshot.active_mask, 1);
    assert_eq!(snapshot.slot_latency_samples[0], 0);
    assert_eq!(snapshot.total_plugin_latency_samples, 0);
    assert_eq!(snapshot.tail_samples, tail);
    assert_eq!(control.stats().latency_samples, 0);
    assert_eq!(control.stats().tail_samples, tail);
    assert_eq!(control.stats().submitted, 0);
    assert_eq!(control.stats().completed, 0);
    assert_eq!(control.stats().faults, 0);
}

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
fn wait_fresh_capture_reply(
    receiver: &super::super::BounceStateReceiver,
) -> Result<crate::instrument_bounce::BounceStateReceipt, BounceCaptureError> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match receiver.try_recv() {
            Ok(result) => {
                assert!(receiver.termination_acknowledged());
                return result;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(error) => panic!("capture reply disconnected: {error}"),
        }
        assert!(
            std::time::Instant::now() < deadline,
            "capture reply deadline"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// Accept complete existing native polls around exactly one dedicated capture, never an
/// added metadata probe, capture retry, parameter/state mutation, or processing command.
#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
fn assert_fresh_capture_worker_ledger(fake: &FakeHelper, capture: &[&str]) {
    let names = fake.names();
    let mut expected = fresh_setup_commands(true);
    expected.extend([
        "LatencySamples",
        "TailSamples",
        "LatencySamples",
        "TailSamples",
    ]);
    assert!(names.len() >= expected.len() + FRESH_WORKER_TEARDOWN.len());
    assert_eq!(names[..expected.len()], expected);
    let polling_end = names.len() - FRESH_WORKER_TEARDOWN.len();
    let mut index = expected.len();
    let mut captures = 0;
    let mut polls = 0;
    while index < polling_end {
        let remaining = &names[index..polling_end];
        if !capture.is_empty()
            && captures == 0
            && remaining.len() >= capture.len()
            && remaining[..capture.len()]
                .iter()
                .map(String::as_str)
                .eq(capture.iter().copied())
        {
            expected.extend_from_slice(capture);
            index += capture.len();
            captures += 1;
        } else {
            assert!(
                remaining.len() >= FRESH_NATIVE_POLL.len()
                    && remaining[..FRESH_NATIVE_POLL.len()]
                        .iter()
                        .map(String::as_str)
                        .eq(FRESH_NATIVE_POLL.iter().copied()),
                "unexpected or partial native/capture sequence: {names:?}"
            );
            expected.extend_from_slice(FRESH_NATIVE_POLL);
            index += FRESH_NATIVE_POLL.len();
            polls += 1;
        }
    }
    assert!(polls >= 1, "normal initial native poll remains required");
    assert_eq!(captures, usize::from(!capture.is_empty()));
    expected.extend_from_slice(FRESH_WORKER_TEARDOWN);
    assert_eq!(names, expected);
    for (index, request) in fake.requests().iter().enumerate() {
        if let Some(editor) = request.get("Editor") {
            assert_eq!(
                editor["command"],
                if index < polling_end {
                    "Query"
                } else {
                    "Close"
                }
            );
        }
    }
    assert_eq!(
        std::fs::read_to_string(fake.dir.join("helper-exit.txt")).unwrap(),
        "normal interpreter exit\n",
        "fixture child interpreter exit evidence, independent of legacy Joined"
    );
}

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
fn assert_no_dedicated_capture_publication(control: &super::super::PluginChainControl) {
    let native = control.native_editor_snapshot(0).unwrap();
    assert_eq!(native.capture_serial, 0);
    assert_eq!(native.parameter_capture_serial, 0);
    assert_eq!(native.captured_generation, 0);
    assert_eq!(native.captured_dirty_revision, 0);
    assert!(native.captured_state.is_none());
    assert!(native.captured_parameters.is_none());
    assert!(!native.capture_in_progress);
    assert!(native.pending_request.is_none());
    assert_eq!(native.completed_request, 0);
}

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
#[test]
fn fresh_source_acknowledges_only_unknown_prepared_inactive_slots_once_before_start() {
    for (mode, mut expected) in [
        ("fresh_normal", dry_metadata()),
        ("fresh_input_inactive", dry_metadata()),
        ("dry_attested", dry_metadata()),
        ("fresh_input_aux", role_metadata(false, false, false)),
        ("fresh_extra_main", role_metadata(true, true, false)),
        ("fresh_observed_roles", role_metadata(false, true, true)),
    ] {
        let fake = FakeHelper::new(mode);
        let mut source =
            Vst3Backend::load_fresh_bounce_source(fake.spec(vec![1, 2, 3]), fresh_config())
                .unwrap();
        source.prepare(fresh_config()).unwrap();
        if mode == "fresh_input_inactive" {
            expected.input_buses[0].active = Some(false);
        }
        assert_eq!(source.bounce_metadata(None).unwrap(), expected);
        assert_eq!(
            source.input_channels,
            if mode == "fresh_input_inactive" { 0 } else { 2 }
        );
        let requests = fake.requests();
        let names = fake.names();
        let restore = names.iter().position(|name| name == "LoadState").unwrap();
        let start = names
            .iter()
            .position(|name| name == "StartProcessing")
            .unwrap();
        let activations: Vec<_> = requests
            .iter()
            .enumerate()
            .filter_map(|(index, request)| request.get("SetBusActive").map(|data| (index, data)))
            .collect();
        assert_eq!(
            activations.len(),
            if mode == "dry_attested" {
                0
            } else if mode == "fresh_input_inactive" {
                3
            } else {
                2
            }
        );
        for (index, data) in &activations {
            assert!(restore < *index && *index < start);
            assert_eq!(data["active"], false);
            assert_eq!(data["media_type"], "Audio");
        }
        let expected_targets = if mode == "dry_attested" {
            vec![]
        } else if mode == "fresh_input_inactive" {
            vec![("Input", 0), ("Output", 1), ("Output", 2)]
        } else {
            vec![("Output", 1), ("Output", 2)]
        };
        for ((_, data), (direction, index)) in activations.iter().zip(expected_targets) {
            assert_eq!(data["direction"], direction);
            assert_eq!(data["bus_index"], index);
        }
        if mode != "dry_attested" {
            let first = activations[0].0;
            assert!(names[restore + 1..first].contains(&"CheckedAudioBusLayout".into()));
            assert!(names[restore + 1..first].contains(&"AudioBusLayout".into()));
            assert!(names[restore + 1..first].contains(&"EventBusLayout".into()));
            assert!(names[restore + 1..first].contains(&"TailSamples".into()));
        }
        assert!(
            !names
                .iter()
                .any(|name| matches!(name.as_str(), "Process" | "ResetOrigin" | "StopProcessing"))
        );
        assert!(
            source
                .prepare(fresh_config())
                .unwrap_err()
                .contains("one-shot")
        );
        assert_eq!(
            fake.requests(),
            requests,
            "published source reprepare must issue no command"
        );
        assert!(source.prepared && source.plugin.is_processing());
    }
}

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
#[test]
fn fresh_source_complete_preflight_refuses_without_any_activation_or_start() {
    for mode in [
        "fresh_aux_active_last",
        "fresh_main_active_last",
        "fresh_unsupported_last",
        "fresh_extra_output",
        "fresh_output0_aux",
        "fresh_prepared_mismatch",
        "fresh_prepared_count",
        "fresh_prepared_activation",
        "fresh_active_unknown",
        "fresh_identity",
        "fresh_event_unknown",
    ] {
        let fake = FakeHelper::new(mode);
        let mut source =
            Vst3Backend::load_fresh_bounce_source(fake.spec(vec![1, 2, 3]), fresh_config())
                .unwrap();
        assert!(source.prepare(fresh_config()).is_err(), "{mode}");
        let requests = fake.requests();
        assert!(
            !fake.names().iter().any(|name| matches!(
                name.as_str(),
                "SetBusActive" | "StartProcessing" | "Process"
            )),
            "{mode}"
        );
        assert!(!source.prepared && !source.plugin.is_processing());
        assert!(
            source
                .prepare(fresh_config())
                .unwrap_err()
                .contains("one-shot")
        );
        assert_eq!(
            fake.requests(),
            requests,
            "failed candidate cannot retry: {mode}"
        );
    }
}

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
#[test]
fn fresh_source_partial_ack_failure_missing_ack_and_drift_never_start_or_retry() {
    for (mode, calls) in [
        ("fresh_set_fail2", 2),
        ("fresh_ack_missing", 1),
        ("fresh_drift", 1),
        ("fresh_input_role_drift", 1),
        ("fresh_extra_role_drift", 1),
        ("fresh_tail_during_ack", 1),
    ] {
        let fake = FakeHelper::new(mode);
        let mut source =
            Vst3Backend::load_fresh_bounce_source(fake.spec(vec![1, 2, 3]), fresh_config())
                .unwrap();
        let error = source.prepare(fresh_config()).unwrap_err();
        if matches!(
            mode,
            "fresh_input_role_drift" | "fresh_extra_role_drift" | "fresh_tail_during_ack"
        ) {
            assert_eq!(
                error, "Fresh bounce source topology changed during inactive-bus acknowledgment",
                "{mode}"
            );
        }
        let requests = fake.requests();
        assert_eq!(
            fake.names()
                .iter()
                .filter(|name| *name == "SetBusActive")
                .count(),
            calls
        );
        assert!(
            !fake
                .names()
                .iter()
                .any(|name| matches!(name.as_str(), "StartProcessing" | "Process"))
        );
        assert!(!source.prepared && !source.plugin.is_processing());
        assert!(
            source
                .prepare(fresh_config())
                .unwrap_err()
                .contains("one-shot")
        );
        assert_eq!(fake.requests(), requests);
    }
}

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
fn fresh_metadata_mismatch_case(
    mode: &str,
) -> (
    BouncePluginMetadata,
    BouncePluginMetadata,
    &'static [&'static str],
) {
    let before = dry_metadata();
    let mut current = before.clone();
    let different: &'static [&'static str] = match mode {
        "fresh_input_role_on_start" => {
            current.input_buses[0].is_main = false;
            &["input_buses"]
        }
        "fresh_extra_role_on_start" => {
            current.output_buses[2].is_main = true;
            &["output_buses"]
        }
        "fresh_event_channels_on_start" => {
            current.event_input_buses[0].0 = 8;
            &["event_input_buses"]
        }
        "fresh_latency_on_start" => {
            current.latency_samples = 127;
            &["latency_samples"]
        }
        "fresh_combined_on_start" => {
            current.input_buses[0].is_main = false;
            current.output_buses[2].is_main = true;
            current.event_input_buses[0].0 = 8;
            current.latency_samples = 127;
            current.tail_samples = u32::MAX;
            &[
                "input_buses",
                "output_buses",
                "event_input_buses",
                "latency_samples",
                "tail_samples",
            ]
        }
        _ => panic!("not a Start metadata-mismatch fixture: {mode}"),
    };
    // Both complete operands must pass the unmodified validator independently.
    // These failures therefore exercise equality after validation, not an earlier refusal.
    before.validate(48_000).unwrap();
    current.validate(48_000).unwrap();
    assert_ne!(before, current, "{mode}");
    (before, current, different)
}

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
fn fresh_metadata_json(metadata: &BouncePluginMetadata) -> serde_json::Value {
    let text = |value: &str| {
        assert!(value.len() <= 256, "fixture identities are complete views");
        serde_json::json!({
            "byte_length": value.len(),
            "excerpt": value,
            "truncated": false,
        })
    };
    let buses = |entries: Vec<serde_json::Value>| {
        assert!(entries.len() <= 16, "fixture bus lists are complete views");
        serde_json::json!({
            "total_count": entries.len(),
            "truncated": false,
            "entries": entries,
        })
    };
    let audio = |entries: &[BounceAudioBus]| {
        buses(
            entries
                .iter()
                .map(|bus| {
                    serde_json::json!({
                        "channel_count": bus.channel_count,
                        "is_main": bus.is_main,
                        "active": bus.active,
                    })
                })
                .collect(),
        )
    };
    let events = |entries: &[(u32, bool, Option<bool>)]| {
        buses(
            entries
                .iter()
                .map(|&(channel_count, is_main, active)| {
                    serde_json::json!({
                        "channel_count": channel_count,
                        "is_main": is_main,
                        "active": active,
                    })
                })
                .collect(),
        )
    };
    serde_json::json!({
        "class_uid": text(&metadata.class_uid),
        "version": text(&metadata.version),
        "name": text(&metadata.name),
        "category": text(&metadata.category),
        "input_buses": audio(&metadata.input_buses),
        "output_buses": audio(&metadata.output_buses),
        "event_input_buses": events(&metadata.event_input_buses),
        "event_output_buses": events(&metadata.event_output_buses),
        "latency_samples": metadata.latency_samples,
        "tail_samples": metadata.tail_samples,
    })
}

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
fn assert_fresh_metadata_mismatch(mode: &str, error: &str) {
    const MARKER: &str = "\nFreshBounceMetadataMismatchV1=";
    let (prefix, data) = error.split_once(MARKER).expect("typed mismatch suffix");
    assert_eq!(
        prefix, "Fresh bounce source topology changed at Start before admission",
        "{mode}"
    );
    assert_eq!(error.matches(MARKER).count(), 1, "{mode}");
    assert!(!error.contains("FreshBounceTopologyRefusalV1="), "{mode}");
    let record: serde_json::Value = serde_json::from_str(data).unwrap();
    let (before, current, changed_fields) = fresh_metadata_mismatch_case(mode);
    let different: serde_json::Map<String, serde_json::Value> = [
        "class_uid",
        "version",
        "name",
        "category",
        "input_buses",
        "output_buses",
        "event_input_buses",
        "event_output_buses",
        "latency_samples",
        "tail_samples",
    ]
    .into_iter()
    .map(|field| (field.to_owned(), changed_fields.contains(&field).into()))
    .collect();
    assert_eq!(
        record,
        serde_json::json!({
            "schema": 1,
            "kind": "fresh_bounce_metadata_mismatch",
            "phase": "post_start",
            "before_start": fresh_metadata_json(&before),
            "current_after_start": fresh_metadata_json(&current),
            "different": different,
        }),
        "exact complete operands and all ten difference flags: {mode}"
    );
}

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
fn assert_fresh_post_start_refusal_ledger(fake: &FakeHelper, restored: bool) {
    let mut expected = fresh_setup_commands(restored);
    // A failed prepare drops its unpublished backend before normal native polling.
    expected.extend(["Editor", "StopProcessing", "Shutdown"]);
    assert_eq!(
        fake.names(),
        expected,
        "only setup, the complete post-Start metadata fence, and teardown are permitted"
    );
    let requests = fake.requests();
    let activations: Vec<_> = requests
        .iter()
        .filter_map(|request| request.get("SetBusActive").cloned())
        .collect();
    assert_eq!(
        activations,
        [1, 2].map(|index| serde_json::json!({
            "media_type": "Audio",
            "direction": "Output",
            "bus_index": index,
            "active": false,
        }))
    );
    assert_eq!(
        requests[requests.len() - 3],
        serde_json::json!({"Editor": {"command": "Close"}})
    );
}

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
fn assert_fresh_post_start_validation_refusal(mode: &str, error: &str) {
    let before = dry_metadata();
    let mut current = before.clone();
    let expected_prefix = match mode {
        "fresh_raw_invalid_after_start" => {
            current.output_buses[0].channel_count = 4;
            "Instrument bounce requires at most one known mono/stereo input for explicit silence, one known-active main mono/stereo output at index zero, and at most two known-inactive mono/stereo extra outputs"
        }
        "fresh_unknown_after_start" => {
            current.output_buses[1].active = None;
            "Instrument bounce activation is unknown; a fresh load with explicit inactive-bus acknowledgment is required"
        }
        _ => panic!("not a post-Start validation-refusal fixture: {mode}"),
    };
    before.validate(48_000).unwrap();
    assert_eq!(current.validate(48_000).unwrap_err(), expected_prefix);
    assert!(!error.contains("FreshBounceMetadataMismatchV1="), "{mode}");
    const MARKER: &str = "\nFreshBounceTopologyRefusalV1=";
    let (prefix, data) = error.split_once(MARKER).expect("original validator suffix");
    assert_eq!(prefix, expected_prefix, "{mode}");
    assert_eq!(error.matches(MARKER).count(), 1, "{mode}");
    let record: serde_json::Value = serde_json::from_str(data).unwrap();
    let checked = |metadata| {
        let all = fresh_metadata_json(metadata);
        serde_json::json!({"inputs": all["input_buses"], "outputs": all["output_buses"]})
    };
    assert_eq!(
        record,
        serde_json::json!({
            "schema": 1,
            "kind": "fresh_bounce_topology_refusal",
            "validation_phase": "post_start",
            "validator_error_excerpt": expected_prefix,
            "validator_error_truncated": false,
            "raw_checked": checked(&current),
            "prepared_observation_phase": null,
            "prepared": null,
            "proposal": null,
            "expected_before_start": checked(&before),
        }),
        "earlier validator keeps its original schema and exact attribution: {mode}"
    );
}

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
#[test]
fn failed_fresh_source_worker_publishes_no_ready_or_active_slot_and_leaves_old_source() {
    use super::super::{PluginChain, RuntimeEvent};
    use std::time::{Duration, Instant};
    let old = FakeHelper::new("normal");
    let old_source = old.source();
    for mode in [
        "fresh_aux_active_last",
        "fresh_set_fail2",
        "fresh_ack_missing",
        "fresh_changed_on_start",
        "fresh_raw_invalid_after_start",
        "fresh_unknown_after_start",
        "fresh_input_role_on_start",
        "fresh_extra_role_on_start",
    ] {
        let fake = FakeHelper::new(mode);
        let PluginChain {
            audio: _audio,
            mut control,
            guard,
        } = PluginChain::spawn_fresh_bounce_source(41, fake.spec(vec![1, 2, 3]), fresh_config())
            .unwrap();
        let mut worker = FreshSourceWorkerGuard::new(&fake, guard);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut fault = false;
        while Instant::now() < deadline {
            while let Some(event) = control.try_next_event() {
                assert!(!matches!(event, RuntimeEvent::SlotReady { .. }), "{mode}");
                if let RuntimeEvent::SlotFault { slot: 0, message } = event {
                    if matches!(
                        mode,
                        "fresh_input_role_on_start" | "fresh_extra_role_on_start"
                    ) {
                        assert_fresh_metadata_mismatch(mode, &message);
                    } else if matches!(
                        mode,
                        "fresh_raw_invalid_after_start" | "fresh_unknown_after_start"
                    ) {
                        assert_fresh_post_start_validation_refusal(mode, &message);
                    } else {
                        assert!(
                            !message.contains("FreshBounceMetadataMismatchV1="),
                            "{mode}"
                        );
                    }
                    fault = true;
                }
            }
            if fault && control.plugin_latency_snapshot().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(fault, "candidate must report its failure: {mode}");
        assert_eq!(control.stats().faults, 1, "{mode}");
        assert_eq!(control.plugin_latency_snapshot().unwrap().active_mask, 0);
        assert_eq!(worker.join(), super::super::ShutdownOutcome::Joined);
        while let Some(event) = control.try_next_event() {
            assert!(!matches!(event, RuntimeEvent::SlotReady { .. }), "{mode}");
        }
        assert!(!fake.names().iter().any(|name| matches!(
            name.as_str(),
            "Process" | "ProcessBuses" | "ProcessResetOrigin" | "ResetOrigin"
        )));
        if matches!(
            mode,
            "fresh_input_role_on_start"
                | "fresh_extra_role_on_start"
                | "fresh_raw_invalid_after_start"
                | "fresh_unknown_after_start"
        ) {
            assert_fresh_post_start_refusal_ledger(&fake, true);
        }
        assert_eq!(control.stats().faults, 1, "{mode}");
        assert_eq!(control.plugin_latency_snapshot().unwrap().active_mask, 0);
        assert!(old_source.prepared && old_source.plugin.is_processing());
        assert!(
            old.names().is_empty(),
            "candidate failure must not touch the existing source"
        );
    }
}

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
#[test]
fn fresh_source_valid_start_metadata_drifts_report_exact_operands_without_admission() {
    use super::super::{PluginChain, RuntimeEvent, ShutdownOutcome};
    use std::time::{Duration, Instant};
    let old = FakeHelper::new("normal");
    let old_source = old.source();
    let old_requests = old.requests();
    let old_state = (
        old_source.prepared,
        old_source.plugin.is_processing(),
        old_source.input_channels,
        old_source.output_channels,
        old_source.native_revision_base,
    );
    for mode in [
        "fresh_input_role_on_start",
        "fresh_extra_role_on_start",
        "fresh_event_channels_on_start",
        "fresh_latency_on_start",
        "fresh_combined_on_start",
    ] {
        let fake = FakeHelper::new(mode);
        let PluginChain {
            audio: _audio,
            mut control,
            guard,
        } = PluginChain::spawn_fresh_bounce_source(41, fake.spec(vec![]), fresh_config()).unwrap();
        let mut worker = FreshSourceWorkerGuard::new(&fake, guard);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut faults = 0;
        while Instant::now() < deadline {
            while let Some(event) = control.try_next_event() {
                match event {
                    RuntimeEvent::SlotReady { .. } => {
                        panic!("refused source published Ready: {mode}")
                    }
                    RuntimeEvent::SlotFault { slot, message } => {
                        assert_eq!(slot, 0, "{mode}");
                        assert_fresh_metadata_mismatch(mode, &message);
                        faults += 1;
                    }
                    _ => {}
                }
            }
            if faults != 0 && control.plugin_latency_snapshot().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(faults, 1, "exactly one pre-join refusal: {mode}");
        assert_eq!(control.stats().faults, 1, "{mode}");
        assert_eq!(control.plugin_latency_snapshot().unwrap().active_mask, 0);
        assert_eq!(worker.join(), ShutdownOutcome::Joined, "{mode}");
        while let Some(event) = control.try_next_event() {
            match event {
                RuntimeEvent::SlotReady { .. } => {
                    panic!("refused source published Ready after join: {mode}")
                }
                RuntimeEvent::SlotFault { .. } => faults += 1,
                _ => {}
            }
        }
        assert_eq!(faults, 1, "no second fault after bounded shutdown: {mode}");
        assert_eq!(control.stats().faults, 1, "{mode}");
        assert_eq!(control.stats().submitted, 0, "{mode}");
        assert_eq!(control.stats().completed, 0, "{mode}");
        assert_eq!(control.plugin_latency_snapshot().unwrap().active_mask, 0);
        // This exact ledger excludes state/capture/process calls, native polling,
        // additional metadata reads, another activation pass, and a second candidate.
        assert_fresh_post_start_refusal_ledger(&fake, false);
        assert_eq!(
            old.requests(),
            old_requests,
            "old-source ledger must remain untouched: {mode}"
        );
        assert_eq!(
            (
                old_source.prepared,
                old_source.plugin.is_processing(),
                old_source.input_channels,
                old_source.output_channels,
                old_source.native_revision_base
            ),
            old_state,
            "old-source lifecycle and cached state must remain untouched: {mode}"
        );
    }
}

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
#[test]
fn default_source_loader_never_acknowledges_inactive_defaults() {
    let mut names = Vec::new();
    for mode in ["normal", "fresh_normal", "fresh_observed_roles"] {
        let fake = FakeHelper::new(mode);
        let mut source = Vst3Backend::load(fake.spec(vec![]), fresh_config()).unwrap();
        source.prepare(fresh_config()).unwrap();
        assert!(!fake.names().iter().any(|name| name == "SetBusActive"));
        names.push(fake.names());
    }
    for candidate in &names[1..] {
        assert_eq!(
            &names[0], candidate,
            "default setup call ledger is unchanged for additional inactive buses and roles"
        );
    }
}

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
fn fresh_setup_commands(restored: bool) -> Vec<&'static str> {
    let mut setup = vec!["LoadPlugin", "AudioBusLayout", "ResetOriginSupport"];
    if restored {
        setup.push("LoadState");
    }
    setup.extend([
        "CheckedAudioBusLayout",
        "EventBusLayout",
        "LatencySamples",
        "TailSamples",
        "AudioBusLayout",
    ]);
    for _ in 0..2 {
        setup.extend([
            "SetBusActive",
            "CheckedAudioBusLayout",
            "EventBusLayout",
            "LatencySamples",
            "TailSamples",
            "AudioBusLayout",
        ]);
    }
    setup.extend([
        "CheckedAudioBusLayout",
        "EventBusLayout",
        "LatencySamples",
        "TailSamples",
        "StartProcessing",
        "NativeDirtyRevision",
        "CheckedAudioBusLayout",
        "EventBusLayout",
        "LatencySamples",
        "TailSamples",
    ]);
    setup
}

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
fn assert_fresh_setup_fences(fake: &FakeHelper, expected: &BouncePluginMetadata) {
    let setup = fresh_setup_commands(true);
    let names = fake.names();
    assert!(names.len() >= setup.len(), "incomplete setup: {names:?}");
    assert!(
        names[..setup.len()].iter().map(String::as_str).eq(setup),
        "full preflight, each acknowledgment reread, and post-Start fence precede Ready: {names:?}"
    );
    let activations: Vec<_> = fake
        .requests()
        .into_iter()
        .filter_map(|request| request.get("SetBusActive").cloned())
        .collect();
    assert_eq!(
        activations,
        [1, 2].map(|index| serde_json::json!({
            "media_type": "Audio",
            "direction": "Output",
            "bus_index": index,
            "active": false,
        }))
    );
    let buses = |buses: &[BounceAudioBus]| {
        buses
            .iter()
            .map(|bus| {
                serde_json::json!({
                    "channel_count": bus.channel_count,
                    "is_main": bus.is_main,
                    "active": bus.active,
                })
            })
            .collect::<Vec<_>>()
    };
    let known = serde_json::json!({
        "inputs": buses(&expected.input_buses),
        "outputs": buses(&expected.output_buses),
    });
    let mut raw = known.clone();
    raw["outputs"][1]["active"] = serde_json::Value::Null;
    raw["outputs"][2]["active"] = serde_json::Value::Null;
    let mut first_ack = raw.clone();
    first_ack["outputs"][1]["active"] = false.into();
    assert_eq!(
        fake.checked_layouts(),
        [raw, first_ack, known.clone(), known.clone(), known],
        "raw unknowns become false only after each actual mock acknowledgment; all descriptors survive"
    );
}

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
fn assert_fresh_success_ledger(fake: &FakeHelper) {
    let mut expected = fresh_setup_commands(true);
    // Ready's checked metadata and the initial latency publication are distinct reads.
    expected.extend([
        "LatencySamples",
        "TailSamples",
        "LatencySamples",
        "TailSamples",
    ]);
    const POLL: &[&str] = &[
        "Editor",
        "TakeParameterEdits",
        "TakeHostNotifications",
        "ServiceHostRequests",
        "NativeDirtyRevision",
        "LatencySamples",
        "TailSamples",
    ];
    const TEARDOWN: &[&str] = &["Editor", "Editor", "StopProcessing", "Shutdown"];
    let names = fake.names();
    assert!(
        names.len() >= expected.len() + POLL.len() + TEARDOWN.len(),
        "complete native poll required: {names:?}"
    );
    let polling_end = names.len() - TEARDOWN.len();
    let polling = &names[expected.len()..polling_end];
    assert_eq!(
        polling.len() % POLL.len(),
        0,
        "partial or extra native calls: {names:?}"
    );
    for _ in 0..polling.len() / POLL.len() {
        expected.extend(POLL);
    }
    expected.extend(TEARDOWN);
    assert_eq!(
        names, expected,
        "exact setup, Ready reads, native polls, and teardown; no diagnostic-only calls"
    );
    let requests = fake.requests();
    for (index, request) in requests.iter().enumerate() {
        if let Some(editor) = request.get("Editor") {
            assert_eq!(
                editor["command"],
                if index < polling_end {
                    "Query"
                } else {
                    "Close"
                }
            );
        }
    }
}

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
#[test]
fn fresh_source_worker_admits_ready_only_after_verified_setup_and_start() {
    use super::super::{PluginChain, RuntimeEvent};
    use std::time::{Duration, Instant};
    for (mode, expected) in [
        ("fresh_normal", dry_metadata()),
        (
            "fresh_tail_on_start",
            BouncePluginMetadata {
                tail_samples: u32::MAX,
                ..dry_metadata()
            },
        ),
        (
            "fresh_observed_roles_block_post_start",
            role_metadata(false, true, true),
        ),
    ] {
        let fake = FakeHelper::new(mode);
        let PluginChain {
            audio: _audio,
            mut control,
            guard,
        } = PluginChain::spawn_fresh_bounce_source(41, fake.spec(vec![1, 2, 3]), fresh_config())
            .unwrap();
        let mut worker = FreshSourceWorkerGuard::new(&fake, guard);
        let deadline = Instant::now() + Duration::from_secs(5);
        if mode == "fresh_observed_roles_block_post_start" {
            // Hold the final post-Start metadata reply, so command issue alone cannot
            // masquerade as a completed admission fence. The guard releases on unwind.
            while !fake.dir.join("entered").exists() && Instant::now() < deadline {
                while let Some(event) = control.try_next_event() {
                    assert!(
                        !matches!(
                            event,
                            RuntimeEvent::SlotReady { .. } | RuntimeEvent::SlotFault { .. }
                        ),
                        "publication before the held post-Start fence: {event:?}"
                    );
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            assert_eq!(
                std::fs::read_to_string(fake.dir.join("entered")).unwrap(),
                "TailSamples"
            );
            assert_fresh_setup_fences(&fake, &expected);
            while let Some(event) = control.try_next_event() {
                assert!(
                    !matches!(
                        event,
                        RuntimeEvent::SlotReady { .. } | RuntimeEvent::SlotFault { .. }
                    ),
                    "publication with final post-Start reply still withheld: {event:?}"
                );
            }
            assert!(
                control
                    .plugin_latency_snapshot()
                    .is_none_or(|snapshot| snapshot.active_mask == 0)
            );
            std::fs::write(fake.dir.join("release"), b"allow post-Start fence reply").unwrap();
        }
        let mut ready = false;
        while Instant::now() < deadline {
            while let Some(event) = control.try_next_event() {
                if let RuntimeEvent::SlotFault { slot, message } = &event {
                    panic!(
                        "unexpected slot {slot} fault: {message}; request ledger: {:#?}",
                        fake.requests()
                    );
                }
                if let RuntimeEvent::SlotReady {
                    slot: 0,
                    latency_samples,
                    tail_samples,
                    ..
                } = event
                {
                    assert_eq!(latency_samples, expected.latency_samples);
                    assert_eq!(tail_samples, expected.tail_samples);
                    assert_fresh_setup_fences(&fake, &expected);
                    assert!(!ready, "one candidate must publish Ready only once: {mode}");
                    ready = true;
                }
            }
            let feedback_published = control
                .native_editor_snapshot(0)
                .is_some_and(|snapshot| snapshot.state.generation == 2 && snapshot.error.is_none());
            if ready && feedback_published && control.plugin_latency_snapshot().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(
            ready,
            "no SlotReady; request ledger: {:#?}",
            fake.requests()
        );
        let feedback = control
            .native_editor_snapshot(0)
            .expect("identified slot snapshot");
        assert_eq!(
            feedback.state.generation, 2,
            "normal worker feedback must complete"
        );
        assert!(feedback.state.supported && feedback.state.has_editor && !feedback.state.open);
        assert!(
            feedback.error.is_none(),
            "native feedback failed: {:?}; request ledger: {:#?}",
            feedback.error,
            fake.requests()
        );
        assert_eq!(feedback.dirty_revision, 0);
        let names = fake.names();
        assert!(
            names
                .windows(5)
                .any(|window| window.iter().map(String::as_str).eq([
                    "Editor",
                    "TakeParameterEdits",
                    "TakeHostNotifications",
                    "ServiceHostRequests",
                    "NativeDirtyRevision",
                ])),
            "complete normal polling sequence missing: {names:?}"
        );
        assert_eq!(control.plugin_latency_snapshot().unwrap().active_mask, 1);
        assert_eq!(
            control.plugin_latency_snapshot().unwrap().tail_samples,
            expected.tail_samples
        );
        assert_eq!(control.stats().tail_samples, expected.tail_samples);
        assert!(!fake.names().iter().any(|name| matches!(
            name.as_str(),
            "Process" | "ProcessBuses" | "ProcessResetOrigin" | "ResetOrigin"
        )));
        assert_eq!(control.stats().submitted, 0);
        assert_eq!(control.stats().completed, 0);
        let joined = worker.join();
        assert_eq!(joined, super::super::ShutdownOutcome::Joined);
        assert_eq!(
            worker.join(),
            joined,
            "repeated join preserves the original observation"
        );
        assert_fresh_success_ledger(&fake);
        assert_eq!(control.stats().faults, 0, "{mode}");
        while let Some(event) = control.try_next_event() {
            assert!(
                !matches!(
                    event,
                    RuntimeEvent::SlotReady { .. } | RuntimeEvent::SlotFault { .. }
                ),
                "no later Ready or fault after successful admission: {event:?}"
            );
        }
    }
}

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
#[test]
fn fresh_source_initial_tail_settlement_reaches_ready_snapshot_and_capture_unchanged() {
    use super::super::{PluginChain, RuntimeEvent, ShutdownOutcome};
    for (before, after) in [
        (88_200, 96_000),
        (0, 17),
        (17, 0),
        (0, u32::MAX),
        (u32::MAX, 0),
        (17, u32::MAX),
        (u32::MAX, 17),
    ] {
        let fake = fresh_timing_fixture(before, after, serde_json::json!({}));
        let PluginChain {
            audio,
            mut control,
            guard,
        } = PluginChain::spawn_fresh_bounce_source(41, fake.spec(vec![1, 2, 3]), fresh_config())
            .unwrap();
        let mut worker = FreshSourceWorkerGuard::new(&fake, guard);
        wait_fresh_timing_ready(&mut control, after);
        assert_eq!(audio.stats().tail_samples, after);
        let receiver = control
            .request_instrument_bounce_state(41, audio.epoch(), vec![17])
            .unwrap();
        let receipt = wait_fresh_capture_reply(&receiver).unwrap();
        assert_eq!(receipt.epoch, audio.epoch());
        let mut expected = capture_state();
        expected.metadata = dry_metadata();
        expected.metadata.tail_samples = after;
        assert_eq!(
            receipt.state, expected,
            "exact opaque state and post-Start metadata: {before}->{after}"
        );
        assert_no_dedicated_capture_publication(&control);
        assert_eq!(control.stats().faults, 0);
        assert_eq!(control.stats().submitted, 0);
        assert_eq!(control.stats().completed, 0);
        assert_eq!(worker.join(), ShutdownOutcome::Joined);
        while let Some(event) = control.try_next_event() {
            assert!(
                matches!(event, RuntimeEvent::ShutdownComplete),
                "unexpected publication: {event:?}"
            );
        }
        assert_fresh_capture_worker_ledger(&fake, CAPTURE_SEQUENCE);
        let replies: Vec<serde_json::Value> =
            std::fs::read_to_string(fake.dir.join("timing-replies.jsonl"))
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
        let tails: Vec<_> = replies
            .iter()
            .filter(|reply| reply["name"] == "TailSamples")
            .collect();
        assert!(
            tails.len() >= 10,
            "setup, Ready, snapshot, native poll and both capture reads stay live"
        );
        for reply in tails {
            assert_eq!(
                reply["result"]["TailSamples"]["samples"],
                if reply["stage"].is_null() {
                    before
                } else {
                    after
                }
            );
        }
    }
}

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
#[test]
fn fresh_source_tail_settlement_waits_for_post_start_and_ready_reply_fences() {
    use super::super::{PluginChain, RuntimeEvent, ShutdownOutcome};
    for stage in ["post_start", "ready"] {
        let fake = fresh_timing_fixture(
            88_200,
            96_000,
            serde_json::json!({
                "hold_stage": stage, "field": "TailSamples",
            }),
        );
        let PluginChain {
            audio,
            mut control,
            guard,
        } = PluginChain::spawn_fresh_bounce_source(41, fake.spec(vec![1, 2, 3]), fresh_config())
            .unwrap();
        let mut worker = FreshSourceWorkerGuard::new(&fake, guard);
        wait_fresh_timing_hold(&fake, stage, "TailSamples");
        assert!(
            control.try_next_event().is_none(),
            "no Ready before the held reply"
        );
        assert!(
            control
                .plugin_latency_snapshot()
                .is_none_or(|snapshot| snapshot.active_mask == 0)
        );
        assert_eq!(control.stats().submitted, 0);
        assert_eq!(control.stats().completed, 0);
        let mut held = fresh_setup_commands(true);
        if stage == "ready" {
            held.extend(["LatencySamples", "TailSamples"]);
        }
        assert_eq!(
            fake.names(),
            held,
            "no Process, MIDI, reset, capture, poll or replay before release"
        );
        std::fs::write(fake.dir.join("release"), b"release exact admission reply").unwrap();
        wait_fresh_timing_ready(&mut control, 96_000);
        assert_eq!(audio.stats().tail_samples, 96_000);
        assert_eq!(worker.join(), ShutdownOutcome::Joined);
        assert_fresh_capture_worker_ledger(&fake, &[]);
        while let Some(event) = control.try_next_event() {
            assert!(
                matches!(event, RuntimeEvent::ShutdownComplete),
                "unexpected later event: {event:?}"
            );
        }
    }
}

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
#[test]
fn fresh_source_timing_drift_faults_at_each_existing_worker_fence_without_rebaseline() {
    use super::super::{PluginChain, RuntimeEvent, ShutdownOutcome};
    use std::time::{Duration, Instant};
    let old = FakeHelper::new("normal");
    let old_source = old.source();
    let old_requests = old.requests();
    let old_state = (
        old_source.prepared,
        old_source.plugin.is_processing(),
        old_source.input_channels,
        old_source.output_channels,
        old_source.native_revision_base,
    );
    for stage in ["ready", "snapshot", "poll"] {
        for field in ["LatencySamples", "TailSamples"] {
            for query_error in [false, true] {
                let expected_value = if field == "LatencySamples" { 0 } else { 96_000 };
                let observed = expected_value + 1;
                let fake = fresh_timing_fixture(
                    88_200,
                    96_000,
                    serde_json::json!({
                        "field": field, "observed": observed, "hold_stage": stage,
                        "change_stage": if query_error { "never" } else { stage },
                        "error_stage": if query_error { stage } else { "never" },
                    }),
                );
                let PluginChain {
                    audio: _audio,
                    mut control,
                    guard,
                } = PluginChain::spawn_fresh_bounce_source(
                    41,
                    fake.spec(vec![1, 2, 3]),
                    fresh_config(),
                )
                .unwrap();
                let mut worker = FreshSourceWorkerGuard::new(&fake, guard);
                wait_fresh_timing_hold(&fake, stage, field);
                let mut ready = 0;
                while let Some(event) = control.try_next_event() {
                    match event {
                        RuntimeEvent::SlotReady {
                            slot,
                            latency_samples,
                            tail_samples,
                            ..
                        } => {
                            assert_eq!((slot, latency_samples, tail_samples), (0, 0, 96_000));
                            ready += 1;
                        }
                        _ => panic!("unexpected event before reply: {event:?}"),
                    }
                }
                assert_eq!(
                    ready,
                    usize::from(stage != "ready"),
                    "initial-snapshot drift may follow an already queued Ready"
                );
                if stage == "poll" {
                    assert_eq!(control.plugin_latency_snapshot().unwrap().active_mask, 1);
                    assert_eq!(control.stats().tail_samples, 96_000);
                } else {
                    assert!(
                        control
                            .plugin_latency_snapshot()
                            .is_none_or(|snapshot| snapshot.active_mask == 0)
                    );
                }
                std::fs::write(
                    fake.dir.join("release"),
                    b"return changed timing or helper error",
                )
                .unwrap();
                let deadline = Instant::now() + Duration::from_secs(5);
                let mut faults = Vec::new();
                while Instant::now() < deadline {
                    while let Some(event) = control.try_next_event() {
                        match event {
                            RuntimeEvent::SlotFault { slot, message } => {
                                assert_eq!(slot, 0);
                                faults.push(message);
                            }
                            _ => panic!("unexpected post-fence event: {event:?}"),
                        }
                    }
                    if !faults.is_empty()
                        && control
                            .plugin_latency_snapshot()
                            .is_some_and(|snapshot| snapshot.active_mask == 0)
                    {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                assert_eq!(faults.len(), 1);
                if query_error {
                    assert_eq!(faults[0], format!("{field}: injected capture IPC failure"));
                } else {
                    assert_eq!(
                        faults[0],
                        fresh_timing_error(field, expected_value, observed)
                    );
                }
                assert_eq!(control.stats().faults, 1);
                assert_eq!(control.plugin_latency_snapshot().unwrap().active_mask, 0);
                assert_eq!(control.stats().tail_samples, 0);
                assert_eq!(control.stats().submitted, 0);
                assert_eq!(control.stats().completed, 0);
                assert_eq!(worker.join(), ShutdownOutcome::Joined);
                while let Some(event) = control.try_next_event() {
                    assert!(
                        matches!(event, RuntimeEvent::ShutdownComplete),
                        "no retry/rebaseline after fault: {event:?}"
                    );
                }
                let mut expected = fresh_setup_commands(true);
                if stage != "ready" {
                    expected.extend(["LatencySamples", "TailSamples"]);
                }
                if stage == "poll" {
                    expected.extend(["LatencySamples", "TailSamples"]);
                    expected.extend_from_slice(&FRESH_NATIVE_POLL[..5]);
                }
                expected.push("LatencySamples");
                if field == "TailSamples" {
                    expected.push("TailSamples");
                }
                expected.extend_from_slice(FRESH_WORKER_TEARDOWN);
                assert_eq!(
                    fake.names(),
                    expected,
                    "exact drift ledger: {stage}/{field}/{query_error}"
                );
                assert_eq!(
                    std::fs::read_to_string(fake.dir.join("helper-exit.txt")).unwrap(),
                    "normal interpreter exit\n"
                );
                assert_eq!(old.requests(), old_requests);
                assert_eq!(
                    (
                        old_source.prepared,
                        old_source.plugin.is_processing(),
                        old_source.input_channels,
                        old_source.output_channels,
                        old_source.native_revision_base
                    ),
                    old_state
                );
            }
        }
    }
}

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
#[test]
fn fresh_source_capture_pin_detects_first_and_final_drift_before_cancellation_checkpoint() {
    use super::super::{PluginChain, RuntimeEvent, ShutdownOutcome};
    for stage in ["capture_first", "capture_final"] {
        for field in ["LatencySamples", "TailSamples"] {
            for outcome in ["drift", "cancel_drift", "cancel_error", "cancel_match"] {
                let expected_value = if field == "LatencySamples" { 0 } else { 96_000 };
                let observed = expected_value + 1;
                let fake = fresh_timing_fixture(
                    88_200,
                    96_000,
                    serde_json::json!({
                        "field": field, "observed": observed, "hold_stage": stage,
                        "change_stage": if outcome.ends_with("drift") { stage } else { "never" },
                        "error_stage": if outcome == "cancel_error" { stage } else { "never" },
                    }),
                );
                let PluginChain {
                    audio,
                    mut control,
                    guard,
                } = PluginChain::spawn_fresh_bounce_source(
                    41,
                    fake.spec(vec![1, 2, 3]),
                    fresh_config(),
                )
                .unwrap();
                let mut worker = FreshSourceWorkerGuard::new(&fake, guard);
                wait_fresh_timing_ready(&mut control, 96_000);
                let receiver = control
                    .request_instrument_bounce_state(41, audio.epoch(), vec![17])
                    .unwrap();
                wait_fresh_timing_hold(&fake, stage, field);
                assert!(!receiver.termination_acknowledged());
                assert!(matches!(
                    receiver.try_recv(),
                    Err(std::sync::mpsc::TryRecvError::Empty)
                ));
                assert_eq!(control.plugin_latency_snapshot().unwrap().active_mask, 1);
                if outcome.starts_with("cancel_") {
                    receiver.cancel();
                }
                // Cancellation is published after dispatch, while the actual scalar/helper
                // reply is still withheld. A returned mismatch must win inside capture_call.
                std::fs::write(
                    fake.dir.join("release"),
                    b"return exact in-flight capture reply",
                )
                .unwrap();
                let error = wait_fresh_capture_reply(&receiver).unwrap_err();
                let faulted = outcome != "cancel_match";
                let message = match error {
                    BounceCaptureError::BackendFailed(message) => {
                        assert!(faulted);
                        if outcome == "cancel_error" {
                            assert_eq!(message, format!("{field}: injected capture IPC failure"));
                        } else {
                            assert_eq!(
                                message,
                                fresh_timing_error(field, expected_value, observed)
                            );
                        }
                        message
                    }
                    BounceCaptureError::Rejected(message) => {
                        assert!(
                            !faulted,
                            "observed backend fault hidden by rejection: {message}"
                        );
                        assert_eq!(message, "Instrument bounce capture was cancelled");
                        message
                    }
                };
                assert_eq!(control.stats().faults, u64::from(faulted));
                assert_eq!(
                    control.plugin_latency_snapshot().unwrap().active_mask,
                    u16::from(!faulted)
                );
                assert_eq!(
                    control.stats().tail_samples,
                    if faulted { 0 } else { 96_000 }
                );
                assert_no_dedicated_capture_publication(&control);
                assert_eq!(control.stats().submitted, 0);
                assert_eq!(control.stats().completed, 0);
                assert_eq!(worker.join(), ShutdownOutcome::Joined);
                let mut faults = 0;
                while let Some(event) = control.try_next_event() {
                    match event {
                        RuntimeEvent::SlotFault {
                            slot,
                            message: event_message,
                        } => {
                            assert_eq!(slot, 0);
                            assert_eq!(event_message, message);
                            faults += 1;
                        }
                        RuntimeEvent::ShutdownComplete => {}
                        _ => panic!(
                            "dedicated capture published an ordinary/native event: {event:?}"
                        ),
                    }
                }
                assert_eq!(faults, usize::from(faulted));
                let prefix = match (stage, field) {
                    ("capture_first", "LatencySamples") => 5,
                    ("capture_first", "TailSamples") => 6,
                    ("capture_final", "LatencySamples") => 14,
                    ("capture_final", "TailSamples") => 15,
                    _ => unreachable!(),
                };
                assert_fresh_capture_worker_ledger(&fake, &CAPTURE_SEQUENCE[..prefix]);
                assert_eq!(
                    fake.names()
                        .iter()
                        .filter(|name| name.as_str() == "SaveState")
                        .count(),
                    usize::from(stage == "capture_final")
                );
            }
        }
    }
}

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
#[test]
fn fresh_source_predispatch_capture_cancellation_sends_no_capture_request() {
    use super::super::{PluginChain, RuntimeEvent, ShutdownOutcome};
    let fake = fresh_timing_fixture(
        88_200,
        96_000,
        serde_json::json!({
            "hold_stage": "poll", "field": "TailSamples",
        }),
    );
    let PluginChain {
        audio,
        mut control,
        guard,
    } = PluginChain::spawn_fresh_bounce_source(41, fake.spec(vec![1, 2, 3]), fresh_config())
        .unwrap();
    let mut worker = FreshSourceWorkerGuard::new(&fake, guard);
    wait_fresh_timing_hold(&fake, "poll", "TailSamples");
    wait_fresh_timing_ready(&mut control, 96_000);
    let receiver = control
        .request_instrument_bounce_state(41, audio.epoch(), vec![17])
        .unwrap();
    receiver.cancel();
    assert!(!receiver.termination_acknowledged());
    std::fs::write(
        fake.dir.join("release"),
        b"finish prior poll after queued capture cancellation",
    )
    .unwrap();
    assert!(
        matches!(wait_fresh_capture_reply(&receiver), Err(BounceCaptureError::Rejected(message)) if message == "Instrument bounce capture was cancelled")
    );
    assert_eq!(control.stats().faults, 0);
    assert_eq!(control.plugin_latency_snapshot().unwrap().active_mask, 1);
    assert_no_dedicated_capture_publication(&control);
    assert_eq!(worker.join(), ShutdownOutcome::Joined);
    assert_fresh_capture_worker_ledger(&fake, &[]);
    while let Some(event) = control.try_next_event() {
        assert!(
            matches!(event, RuntimeEvent::ShutdownComplete),
            "unexpected publication: {event:?}"
        );
    }
}

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
#[test]
fn fresh_source_success_pin_and_failed_consumption_both_forbid_reprepare() {
    use super::super::FreshBounceSourceSetup;
    for success in [false, true] {
        let fake = if success {
            fresh_timing_fixture(88_200, 96_000, serde_json::json!({}))
        } else {
            FakeHelper::new("fresh_latency_on_start")
        };
        let mut source =
            Vst3Backend::load_fresh_bounce_source(fake.spec(vec![1, 2, 3]), fresh_config())
                .unwrap();
        assert!(matches!(
            source.fresh_bounce_setup,
            Some(FreshBounceSourceSetup::Pending)
        ));
        let result = source.prepare(fresh_config());
        if success {
            result.unwrap();
            assert!(matches!(
                source.fresh_bounce_setup,
                Some(FreshBounceSourceSetup::Pinned {
                    latency_samples: 0,
                    tail_samples: 96_000
                })
            ));
        } else {
            assert_fresh_metadata_mismatch("fresh_latency_on_start", &result.unwrap_err());
            assert!(matches!(
                source.fresh_bounce_setup,
                Some(FreshBounceSourceSetup::Consumed)
            ));
        }
        let requests = fake.requests();
        assert_eq!(
            source.prepare(fresh_config()).unwrap_err(),
            "Fresh bounce source preparation is one-shot; load a new candidate"
        );
        assert_eq!(
            fake.requests(),
            requests,
            "reprepare emits no SDK/protocol command"
        );
        assert_eq!(source.prepared, success);
        assert!(source.plugin.is_processing());
        drop(source);
        assert_fresh_post_start_refusal_ledger(&fake, true);
    }
}

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
#[test]
fn default_source_timing_remains_live_and_unpinned_after_start() {
    for field in ["LatencySamples", "TailSamples"] {
        let observed = if field == "LatencySamples" { 1 } else { 96_001 };
        let fake = fresh_timing_fixture(
            88_200,
            96_000,
            serde_json::json!({
                "field": field, "change_stage": "ready", "observed": observed,
            }),
        );
        let mut source = Vst3Backend::load(fake.spec(vec![1, 2, 3]), fresh_config()).unwrap();
        source.prepare(fresh_config()).unwrap();
        assert!(source.fresh_bounce_setup.is_none());
        let setup = fake.names();
        assert_eq!(
            setup,
            [
                "LoadPlugin",
                "AudioBusLayout",
                "ResetOriginSupport",
                "LoadState",
                "StartProcessing",
                "NativeDirtyRevision"
            ]
        );
        assert_eq!(source.processing_metadata().unwrap(), (0, 96_000));
        assert_eq!(
            source.processing_metadata().unwrap(),
            if field == "LatencySamples" {
                (1, 96_000)
            } else {
                (0, 96_001)
            }
        );
        assert_eq!(source.processing_metadata().unwrap(), (0, 96_000));
        assert!(source.fresh_bounce_setup.is_none());
        let mut expected = setup;
        for _ in 0..3 {
            expected.extend(["LatencySamples".into(), "TailSamples".into()]);
        }
        assert_eq!(
            fake.names(),
            expected,
            "matching and changed ordinary reads use the unchanged live pair"
        );
        drop(source);
        expected.extend(["Editor".into(), "StopProcessing".into(), "Shutdown".into()]);
        assert_eq!(fake.names(), expected);
    }
}

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
#[test]
fn fresh_source_first_prepare_consumes_setup_even_when_restore_or_initial_guard_fails() {
    for mode in ["fresh_restore_fail", "fresh_normal"] {
        let fake = FakeHelper::new(mode);
        let mut source =
            Vst3Backend::load_fresh_bounce_source(fake.spec(vec![1, 2, 3]), fresh_config())
                .unwrap();
        let config = if mode == "fresh_normal" {
            PluginPrepareConfig {
                max_block_frames: 128,
                ..fresh_config()
            }
        } else {
            fresh_config()
        };
        assert!(source.prepare(config).is_err());
        let requests = fake.requests();
        assert!(!fake.names().iter().any(|name| matches!(
            name.as_str(),
            "SetBusActive" | "StartProcessing" | "Process"
        )));
        assert!(
            source
                .prepare(fresh_config())
                .unwrap_err()
                .contains("one-shot")
        );
        assert_eq!(fake.requests(), requests);
    }
}

#[test]
fn fresh_offline_factory_rejects_supported_inactive_bus_change_against_capture() {
    for changed in ["inactive_channels", "input_role", "extra_role"] {
        let fake = FakeHelper::new("dry_attested");
        let mut capture = capture_state();
        capture.metadata = dry_metadata();
        match changed {
            "inactive_channels" => capture.metadata.output_buses[1].channel_count = 2,
            "input_role" => capture.metadata.input_buses[0].is_main = false,
            "extra_role" => capture.metadata.output_buses[1].is_main = true,
            _ => unreachable!(),
        }
        capture.metadata.validate(48_000).unwrap();
        let result = OfflineInstrument::load_snapshot(
            fake.spec(capture.opaque_state.clone()),
            48_000,
            &capture,
        );
        let error = result
            .err()
            .expect("valid but changed descriptor must refuse");
        assert_eq!(
            error, "Fresh bounce instance metadata differs from the authoritative captured source",
            "{changed}"
        );
        assert!(fake.names().iter().any(|name| name == "TailSamples"));
        assert!(!fake.names().iter().any(|name| matches!(
            name.as_str(),
            "SetBusActive" | "Process" | "ProcessBuses" | "ProcessResetOrigin" | "ResetOrigin"
        )));
    }
}

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
#[test]
fn fresh_source_keeps_original_zero_input_topology_without_activation_calls() {
    let fake = FakeHelper::new("normal");
    let mut source =
        Vst3Backend::load_fresh_bounce_source(fake.spec(vec![]), fresh_config()).unwrap();
    source.prepare(fresh_config()).unwrap();
    assert_eq!(
        source.bounce_metadata(None).unwrap(),
        capture_state().metadata
    );
    assert_eq!(source.input_channels, 0);
    assert!(!fake.names().iter().any(|name| name == "SetBusActive"));
}

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
#[test]
fn fresh_source_fixture_unwind_joins_worker_and_retains_terminal_evidence() {
    use super::super::{PluginChain, RuntimeEvent};
    use std::time::{Duration, Instant};
    const INTENTIONAL_PANIC: &str = "intentional Rust panic to exercise fixture owner unwind";
    let mut retained_directory = None;
    let unwound: std::thread::Result<()> = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
        || {
            let fake = FakeHelper::new("fresh_normal");
            retained_directory = Some(fake.dir.clone());
            let PluginChain {
                audio: _audio,
                mut control,
                guard,
            } = PluginChain::spawn_fresh_bounce_source(
                41,
                fake.spec(vec![1, 2, 3]),
                fresh_config(),
            )
            .unwrap();
            // No explicit join: this exact guard and the fixture must drop while unwinding.
            let _worker = FreshSourceWorkerGuard::new(&fake, guard);
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut ready = false;
            let mut feedback_published = false;
            while Instant::now() < deadline {
                while let Some(event) = control.try_next_event() {
                    if let RuntimeEvent::SlotFault { slot, message } = &event {
                        panic!(
                            "unexpected slot {slot} fault before unwind: {message}; request ledger: {:#?}",
                            fake.requests()
                        );
                    }
                    ready |= matches!(event, RuntimeEvent::SlotReady { slot: 0, .. });
                }
                feedback_published = control.native_editor_snapshot(0).is_some_and(|snapshot| {
                    snapshot.state.generation == 2 && snapshot.error.is_none()
                });
                if ready && feedback_published {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            assert!(
                ready && feedback_published,
                "worker setup must succeed before intentional unwind; request ledger: {:#?}",
                fake.requests()
            );
            assert_eq!(control.plugin_latency_snapshot().unwrap().active_mask, 1);
            // Deliberate test-thread Rust panic, outside every helper/COM operation.
            // RAII cleanup must not replace this payload with a secondary cleanup panic.
            panic!("{INTENTIONAL_PANIC}");
        },
    ));
    let panic = unwound.expect_err("fixture scope must unwind intentionally");
    let message = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&str>().copied());
    assert_eq!(
        message,
        Some(INTENTIONAL_PANIC),
        "original panic payload must survive bounded cleanup"
    );
    let directory = retained_directory.expect("fixture path retained outside unwinding scope");
    assert!(
        directory.is_dir(),
        "panic must preserve the fixture directory"
    );
    assert_eq!(
        std::fs::read_to_string(directory.join("worker-shutdown.txt")).unwrap(),
        "Joined\n"
    );
    assert_eq!(
        std::fs::read(directory.join("release")).unwrap(),
        b"fixture owner shutdown"
    );
    let raw = std::fs::read_to_string(directory.join("requests.jsonl")).unwrap();
    let requests: Vec<serde_json::Value> = raw
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        requests.last().and_then(serde_json::Value::as_str),
        Some("Shutdown"),
        "terminal helper request must be retained: {raw}"
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.as_str() == Some("Shutdown"))
            .count(),
        1
    );
    assert!(
        requests
            .iter()
            .any(|request| request.as_str() == Some("StopProcessing"))
    );
    assert!(!requests.iter().any(|request| request.get("Process").is_some() || request.get("ResetOrigin").is_some()));
    // Inspect the actual bounded-join observation and terminal request before cleanup.
    // Any failed assertion above leaves the retained evidence untouched; this does not
    // claim the legacy shutdown mechanism has gained strict helper-reap guarantees.
    std::fs::remove_dir_all(&directory).unwrap();
    assert!(!directory.exists());
}

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
fn parse_fresh_topology_refusal(error: &str) -> serde_json::Value {
    let (prefix, data) = error.split_once("\nFreshBounceTopologyRefusalV1=").unwrap();
    assert_eq!(
        prefix,
        "Instrument bounce requires at most one known mono/stereo input for explicit silence, one known-active main mono/stereo output at index zero, and at most two known-inactive mono/stereo extra outputs"
    );
    serde_json::from_str(data).unwrap()
}

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
#[test]
fn fresh_source_diagnostic_worker_keeps_exact_refusal_ledger_and_raw_roles() {
    use super::super::{PluginChain, RuntimeEvent, ShutdownOutcome};
    use std::time::{Duration, Instant};
    let fake = FakeHelper::new("fresh_role_diagnostic");
    let PluginChain {
        audio: _audio,
        mut control,
        guard,
    } = PluginChain::spawn_fresh_bounce_source(41, fake.spec(vec![]), fresh_config()).unwrap();
    let mut worker = FreshSourceWorkerGuard::new(&fake, guard);
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut fault = None;
    while Instant::now() < deadline && fault.is_none() {
        while let Some(event) = control.try_next_event() {
            match event {
                RuntimeEvent::SlotReady { .. } => panic!("refused source published Ready"),
                RuntimeEvent::SlotFault { slot: 0, message } => fault = Some(message),
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let fault = fault.expect("actual worker refusal required");
    let json = parse_fresh_topology_refusal(&fault);
    assert_eq!(json["schema"], 1);
    assert_eq!(json["kind"], "fresh_bounce_topology_refusal");
    assert_eq!(json["validation_phase"], "proposed_preflight");
    assert_eq!(json["prepared_observation_phase"], "initial_preflight_read");
    assert!(json["expected_before_start"].is_null());
    assert_eq!(json["raw_checked"]["inputs"]["total_count"], 1);
    assert_eq!(
        json["raw_checked"]["inputs"]["entries"][0]["is_main"],
        false
    );
    assert_eq!(json["raw_checked"]["outputs"]["total_count"], 3);
    for index in 0..3 {
        assert_eq!(
            json["raw_checked"]["outputs"]["entries"][index]["is_main"],
            true
        );
    }
    for index in [1, 2] {
        assert!(json["raw_checked"]["outputs"]["entries"][index]["active"].is_null());
        assert_eq!(
            json["prepared"]["outputs"]["entries"][index]["active"],
            false
        );
        assert_eq!(
            json["proposal"]["outputs"]["entries"][index]["active"],
            false
        );
    }
    for layer in ["raw_checked", "prepared", "proposal"] {
        assert_eq!(json[layer]["inputs"]["total_count"], 1);
        assert_eq!(json[layer]["outputs"]["total_count"], 3);
        assert_eq!(json[layer]["inputs"]["truncated"], false);
        assert_eq!(json[layer]["outputs"]["truncated"], false);
        assert_eq!(json[layer]["inputs"]["entries"][0]["channel_count"], 2);
        assert_eq!(json[layer]["inputs"]["entries"][0]["active"], true);
        assert_eq!(json[layer]["outputs"]["entries"][0]["channel_count"], 2);
        assert_eq!(json[layer]["outputs"]["entries"][0]["active"], true);
        assert_eq!(json[layer]["outputs"]["entries"][1]["channel_count"], 2);
        assert_eq!(
            json[layer]["outputs"]["entries"][2]["channel_count"], 3,
            "the otherwise-admissible role fixture refuses an explicit unsupported channel count"
        );
    }
    assert_eq!(json["proposal"]["inputs"]["entries"][0]["is_main"], false);
    for index in 0..3 {
        assert_eq!(
            json["proposal"]["outputs"]["entries"][index]["is_main"],
            true
        );
    }
    assert_eq!(worker.join(), ShutdownOutcome::Joined);
    assert_eq!(control.plugin_latency_snapshot().unwrap().active_mask, 0);
    assert_eq!(
        fake.names(),
        [
            "LoadPlugin",
            "AudioBusLayout",
            "ResetOriginSupport",
            "CheckedAudioBusLayout",
            "EventBusLayout",
            "LatencySamples",
            "TailSamples",
            "AudioBusLayout",
            "Editor",
            "Shutdown",
        ]
    );
    while let Some(event) = control.try_next_event() {
        assert!(!matches!(event, RuntimeEvent::SlotReady { .. }));
    }
    let requests = fake.requests();
    assert_eq!(requests[8]["Editor"]["command"], "Close");
    assert_eq!(control.stats().submitted, 0);
    assert_eq!(control.stats().completed, 0);
}

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
#[test]
fn fresh_source_diagnostic_success_path_adds_no_owner_calls() {
    let fake = FakeHelper::new("normal");
    let mut source =
        Vst3Backend::load_fresh_bounce_source(fake.spec(vec![]), fresh_config()).unwrap();
    source.prepare(fresh_config()).unwrap();
    assert_eq!(
        fake.names(),
        [
            "LoadPlugin",
            "AudioBusLayout",
            "ResetOriginSupport",
            "CheckedAudioBusLayout",
            "EventBusLayout",
            "LatencySamples",
            "TailSamples",
            "AudioBusLayout",
            "CheckedAudioBusLayout",
            "EventBusLayout",
            "LatencySamples",
            "TailSamples",
            "StartProcessing",
            "NativeDirtyRevision",
            "CheckedAudioBusLayout",
            "EventBusLayout",
            "LatencySamples",
            "TailSamples",
        ]
    );
    assert!(source.prepared);
}

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
#[test]
fn fresh_source_diagnostic_post_start_failure_does_not_query_prepared_again() {
    for mode in ["fresh_raw_invalid_after_start", "fresh_unknown_after_start"] {
        let fake = FakeHelper::new(mode);
        let mut source =
            Vst3Backend::load_fresh_bounce_source(fake.spec(vec![]), fresh_config()).unwrap();
        let error = source.prepare(fresh_config()).unwrap_err();
        assert_fresh_post_start_validation_refusal(mode, &error);
        assert_eq!(fake.names(), fresh_setup_commands(false), "{mode}");
        assert!(!source.prepared && source.plugin.is_processing());
        let requests = fake.requests();
        assert!(
            source
                .prepare(fresh_config())
                .unwrap_err()
                .contains("one-shot")
        );
        assert_eq!(
            fake.requests(),
            requests,
            "failed candidate may not prepare again: {mode}"
        );
    }
}
