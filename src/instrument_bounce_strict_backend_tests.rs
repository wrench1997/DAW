//! Source-only acceptance fixtures for the opt-in strict clone.
//!
//! These are mock processes, never plugins. Each test owns private paths and a
//! single serialized strict reservation. RAII drains both renderer and direct
//! owners on assertion unwind; deleting the fixture never abandons its child.
//! Run only in the separately approved mock acceptance gate, with Python 3.
//!
//! Partial Shutdown writes and injected spawn/kill/wait syscalls are deliberately
//! not claimed here: the closed adapter has no public fault-injection surface.
//! Those remain separate foundation evidence, not adapter acceptance results.
//! Pump-level deadline-after-EAGAIN proof also remains Stop A evidence: a fixed
//! deadline may legitimately expire during large-state encoding before dispatch.

use super::*;
use crate::{
    export_job::ExportControl,
    instrument_bounce::render::{
        BackendDelivery, BackendFailure, BackendFailureKind, BlockReply, DisposalObservation,
        FailureKind, RenderAdmission, RenderBackend, RenderControl, RenderFailure, RenderOutcome,
        RenderReceipt, RenderStatus,
    },
    model::{PluginInstance, PluginRole, PluginRuntimeStatus},
    plugins::{
        plugin_runtime::PluginTransport,
        strict_backend::{StrictBackend, TestAction},
    },
};
use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{
        Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
    thread,
};
use vst3_host::{MetadataQueryRejection, strict_offline::FailureKind as StrictKind};

static SERIAL: Mutex<()> = Mutex::new(());
static NEXT: AtomicU64 = AtomicU64::new(1);
const UID: &str = "ABCDEF019182FAEB566D624153675854";
const SENTINEL: &[u8] = b"unchanged destination";

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|p| p.into_inner())
}

// No shell, environment overrides, network, real bundle, or external assets.
// The helper validates all request scalars and logs complete commands including
// PID and per-command occurrence before responding. JSON lines remain atomic
// enough for the one writer; Rust owner observations use a separate audit file.
const MOCK: &str = r#"#!/usr/bin/env python3
import atexit, base64, json, os, pathlib, select, struct, sys, time
root = pathlib.Path(__file__).parent
cfg = json.loads((root / 'config.json').read_text())
ledger = open(root / 'ledger.jsonl', 'a', buffering=1)
counts = {}
processed = 0
sets = []
reads = []
def log(kind, **fields):
    ledger.write(json.dumps(dict(pid=os.getpid(), kind=kind, **fields)) + '\n')
def unchanged():
    return (root / 'render.wav').read_bytes() == b'unchanged destination'
def exiting():
    log('exit', destination_unchanged=unchanged())
atexit.register(exiting)
log('spawn', destination_unchanged=unchanged())
def reply(value):
    print(json.dumps(value, separators=(',', ':')), flush=True)
def eof():
    log('eof', destination_unchanged=unchanged())
    if cfg['fault'] == 'hold_exit':
        log('holding_exit')
        while not (root / 'release').exists():
            time.sleep(.01)
def wait_block(stage):
    log('blocked', stage=stage)
    while not (root / 'release').exists():
        ready, _, _ = select.select([sys.stdin], [], [], .01)
        if ready:
            extra = os.read(0, 4096)
            if not extra:
                eof()
                return False
            log('unexpected_after_block', data=extra.decode('utf-8', 'replace'))
            raise AssertionError('protocol continued after blocked terminal exchange')
    return True
def parameter(ident):
    return dict(id=ident, name='base '+str(ident), value=.5, min=0., max=1.,
                default=.25, unit='', step_count=0, can_automate=True,
                is_read_only=False, is_bypass=False, flags=0)
while True:
    line = sys.stdin.buffer.readline()
    if not line:
        eof()
        break
    command = json.loads(line)
    name = command if isinstance(command, str) else next(iter(command))
    args = {} if isinstance(command, str) else command[name]
    counts[name] = counts.get(name, 0) + 1
    stage = name + ':' + str(counts[name])
    log('command', stage=stage, command=command, destination_unchanged=unchanged())
    assert unchanged(), 'destination published before child exit'
    if name == 'Shutdown':
        assert cfg['stage'] != stage, 'Shutdown is write-only, not a reply stage'
        assert sys.stdin.buffer.read() == b'', 'protocol after Shutdown'
        eof()
        if cfg['fault'] == 'nonzero_exit':
            sys.exit(17)
        break
    selected = cfg['stage'] == stage
    fault = cfg['fault'] if selected else ''
    if fault == 'death':
        log('injected_death', stage=stage)
        os._exit(17)
    if fault == 'block' and not wait_block(stage):
        break
    if fault in ('error', 'wrong', 'malformed'):
        log('injected_fault', stage=stage, fault=fault)
        if fault == 'error':
            reply({'Error': {'message': 'known mock failure '+stage}})
        elif fault == 'wrong':
            reply({'PluginInfo': {}} if name != 'LoadPlugin' else {'Success': {'message': 'wrong'}})
        else:
            print('{malformed', flush=True)
        continue
    success = {'Success': {'message': 'ok'}}
    if name == 'LoadPlugin':
        assert args == dict(path=str(root / 'mock-only.vst3'), sample_rate=8000.,
                           block_size=2048, tempo=120., time_sig_numerator=4,
                           time_sig_denominator=4, class_id=cfg['uid']), args
        info = dict(vendor='Mock', name='Mock only', version='1.3.4',
                    category='Instrument|Synth', uid=cfg['uid'], has_gui=False,
                    audio_inputs=0, audio_outputs=1, output_channels=cfg['channels'],
                    has_midi_input=True, has_midi_output=False, compatibility=[])
        if fault == 'identity': info['uid'] = '0'*32
        if fault == 'version': info['version'] = 'wrong'
        if fault == 'category': info['category'] = 'Fx'
        if fault == 'name': info['name'] = 'different'
        response = {'PluginInfo': info}
    elif name == 'SetProcessMode':
        assert args == {'mode': 'Offline'}, args
        response = success
    elif name == 'ResetOriginSupport':
        support = dict(contract_version=1, max_block_frames=2048, processing=False)
        if fault == 'support_version': support['contract_version'] = 2
        if fault == 'support_maximum': support['max_block_frames'] = 128
        if fault == 'support_processing': support['processing'] = True
        response = {'ResetOriginSupport': {'support': support}}
    elif name == 'LoadState':
        assert args['context'] == 'Project', args
        assert base64.b64decode(args['data'], validate=True) == bytes([1, 2, 3]), args
        response = success
    elif name in ('StartProcessing', 'StopProcessing'):
        response = success
    elif name == 'NativeDirtyRevision':
        response = {'NativeDirtyRevision': {'revision': 77}}
    elif name == 'GetAllParameters':
        params = [parameter(17), parameter(23)]
        if fault == 'catalog_missing': params.pop()
        if fault == 'catalog_readonly': params[1]['is_read_only'] = True
        if fault == 'catalog_duplicate': params.append(parameter(17))
        response = {'Parameters': {'params': params}}
    elif name == 'SetParameter':
        assert [args['id'], args['value']] == cfg['bases'][len(sets)], args
        assert not reads, 'setter after readback'
        sets.append(args['id'])
        response = success
    elif name == 'GetParameter':
        assert sets == [17, 23], sets
        assert args['id'] == [17, 23][len(reads)], args
        reads.append(args['id'])
        # Deliberately different raw f64 that projects to the same captured f32.
        value = .1 if args['id'] == 17 else .75
        if fault == 'base_mismatch': value = .100001
        if fault == 'base_negative': value = -.001
        if fault == 'base_above_one': value = 1.001
        if fault == 'base_nan': value = 'NaN'
        if fault == 'base_inf': value = 'inf'
        response = {'ParameterValue': {'value': value}}
    elif name == 'CheckedAudioBusLayout':
        layout = dict(inputs=[], outputs=[dict(channel_count=cfg['channels'], is_main=True, active=True)])
        if fault == 'audio_aux': layout['outputs'][0]['is_main'] = False
        if fault == 'audio_unknown': layout['outputs'][0]['active'] = None
        if fault == 'audio_inactive': layout['outputs'][0]['active'] = False
        if fault == 'audio_input': layout['inputs'] = [dict(channel_count=1, is_main=True, active=True)]
        if fault == 'audio_extra_bus': layout['outputs'].append(dict(channel_count=1, is_main=False, active=False))
        if fault == 'audio_channels': layout['outputs'][0]['channel_count'] = 3
        response = {'CheckedAudioBusLayout': {'layout': layout}}
    elif name == 'EventBusLayout':
        layout = dict(inputs=[dict(channel_count=16, is_main=True, active=True)], outputs=[])
        if fault == 'event_aux': layout['inputs'][0]['is_main'] = False
        if fault == 'event_unknown': layout['inputs'][0]['active'] = None
        if fault == 'event_output': layout['outputs'] = [dict(channel_count=1, is_main=True, active=True)]
        response = {'EventBusLayout': {'layout': layout}}
    elif name == 'LatencySamples':
        response = {'LatencySamples': {'samples': cfg['latency'] + (1 if fault == 'latency_drift' else 0)}}
    elif name == 'TailSamples':
        response = {'TailSamples': {'samples': 0 if fault == 'tail_drift' else 4294967295}}
    elif name == 'SendMidiAt':
        assert 0 <= args['sample_offset'] < 128, args
        event = args['event']
        assert len(event) == 1 and next(iter(event)) in ('NoteOn', 'NoteOff'), event
        response = success
    elif name == 'Process':
        assert reads == [17, 23], reads
        assert args['inputs'] == [] and args['frames'] == 128, args
        transport = args['transport']
        assert transport == dict(sample_position=processed, quarter_note_position=processed/4000.,
                                 tempo=120., playing=True, time_sig_numerator=4,
                                 time_sig_denominator=4), transport
        processed += 128
        channels = cfg['channels'] + (1 if fault == 'extra_channels' else -1 if fault == 'short_channels' else 0)
        frames = 128 + (1 if fault == 'extra_frames' else -1 if fault == 'short_frames' else 0)
        outputs = []
        for channel in range(channels):
            samples = [.25 if channel == 0 else -.5] * frames
            if fault == 'nonfinite_first': samples[0] = float('nan')
            if fault == 'nonfinite_last': samples[-1] = float('inf')
            outputs.append(base64.b64encode(struct.pack('<'+'f'*frames, *samples)).decode('ascii'))
        response = {'AudioOutput': dict(outputs=outputs, output_events=[{}] if fault == 'output_event' else [],
                    output_events_lost=fault == 'output_loss', transport_applied=fault != 'missing_ack')}
    else:
        raise AssertionError('forbidden command '+name)
    if fault.startswith('metadata_'):
        response = {'MetadataQueryRejected': {'reason': fault[len('metadata_'):]}}
    reply(response)
    log('reply', stage=stage)
    if cfg['fault'] == 'block_stdin' and stage == 'ResetOriginSupport:1':
        # Witness a prefix of the next large LoadState, then stop draining stdin.
        # This proves dispatch without needing private write/syscall hooks.
        prefix = os.read(0, 64)
        assert prefix.startswith(b'{"LoadState":'), prefix
        log('blocked_input', stage='LoadState:1', prefix=prefix.hex())
        while not (root / 'release').exists():
            time.sleep(.01)
        break
"#;

struct Mock {
    root: PathBuf,
    channels: usize,
    latency: u32,
    blocked_stdin: bool,
}
impl Mock {
    fn new(fault: &str, stage: &str) -> Self {
        Self::with_shape(fault, stage, 2, 0)
    }
    fn with_shape(fault: &str, stage: &str, channels: usize, latency: u32) -> Self {
        let root = std::env::temp_dir().join(format!(
            "strict-bounce-mock-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let this = Self {
            root,
            channels,
            latency,
            blocked_stdin: fault == "block_stdin",
        };
        this.write(
            "config.json",
            serde_json::to_vec(&json!({
                "fault": fault, "stage": stage, "channels": channels, "latency": latency,
                "uid": UID, "bases": [[17, f64::from(0.1_f32)], [23, 0.75]],
            }))
            .unwrap()
            .as_slice(),
        );
        this.write("helper.py", MOCK.as_bytes());
        fs::set_permissions(
            this.root.join("helper.py"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        this.write(
            "mock-only.vst3",
            b"not a plugin; Python never opens this file",
        );
        this.write("render.wav", SENTINEL);
        this
    }
    fn write(&self, name: &str, bytes: &[u8]) {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(self.root.join(name))
            .unwrap();
        file.write_all(bytes).unwrap();
    }
    fn output(&self) -> PathBuf {
        self.root.join("render.wav")
    }
    fn ledger(&self) -> Vec<Value> {
        read_lines(&self.root.join("ledger.jsonl"))
    }
    fn audit(&self) -> Vec<Value> {
        read_lines(&self.root.join("audit.jsonl"))
    }
    fn commands(&self) -> Vec<String> {
        self.ledger()
            .into_iter()
            .filter(|v| v["kind"] == "command")
            .map(|v| v["stage"].as_str().unwrap().to_owned())
            .collect()
    }
    fn wait_stage(&self, kind: &str, stage: &str) {
        wait_until(|| {
            self.ledger()
                .iter()
                .any(|v| v["kind"] == kind && v["stage"] == stage)
        });
    }
    fn assert_unchanged(&self) {
        assert_eq!(fs::read(self.output()).unwrap(), SENTINEL);
        assert!(
            !fs::read_dir(&self.root).unwrap().any(|entry| {
                let name = entry.unwrap().file_name();
                let name = name.to_string_lossy();
                name.starts_with(".render.wav.") && name.ends_with(".tmp")
            }),
            "failed render leaked a staged WAV"
        );
    }
    fn assert_reaped(&self) {
        let ledger = self.ledger();
        let pids: Vec<u64> = ledger.iter().map(|v| v["pid"].as_u64().unwrap()).collect();
        assert!(!pids.is_empty(), "mock never started");
        assert!(
            pids.iter().all(|pid| *pid == pids[0]),
            "candidate replayed or replaced"
        );
        assert!(
            !Path::new(&format!("/proc/{}", pids[0])).exists(),
            "child not reaped"
        );
        assert!(
            ledger
                .iter()
                .filter_map(|v| v["destination_unchanged"].as_bool())
                .all(|v| v)
        );
        assert!(!ledger.iter().any(|v| v["kind"] == "unexpected_after_block"));
    }
    fn assert_terminal_at(&self, stage: &str) {
        let commands = self.commands();
        assert_eq!(
            commands.last().map(String::as_str),
            Some(stage),
            "later exchange/replay: {commands:?}"
        );
        assert_eq!(commands.iter().filter(|s| s.as_str() == stage).count(), 1);
        self.assert_unchanged();
        self.assert_reaped();
    }
    fn assert_cooperative(&self, stopped: bool) {
        let commands = self.commands();
        assert_eq!(commands.last().map(String::as_str), Some("Shutdown:1"));
        assert_eq!(
            commands
                .iter()
                .filter(|s| s.starts_with("StopProcessing:"))
                .count(),
            usize::from(stopped)
        );
        if stopped {
            assert_eq!(commands[commands.len() - 2], "StopProcessing:1");
        }
        assert!(self.ledger().iter().any(|v| v["kind"] == "exit"));
        self.assert_reaped();
    }
    fn capture(&self) -> (Project, CapturedInstrumentBounce) {
        let mut project = Project {
            tempo: 120.0,
            song_length_beats: 128.0 / 4000.0,
            ..Project::default()
        };
        project.automation_lanes.clear();
        project.channels.truncate(1);
        project.channels[0].instrument_plugin_instance_id = Some(41);
        project.plugin_instances = vec![PluginInstance {
            id: 41,
            format: PluginFormat::Vst3,
            role: PluginRole::Instrument,
            path: self.root.join("mock-only.vst3"),
            uid: "project-path-id-is-not-factory-class".into(),
            vendor: "Mock".into(),
            name: "Mock".into(),
            enabled: true,
            bypass: false,
            wet: 1.0,
            parameters: [(17, 0.25), (23, 0.5)].into(),
            opaque_state: vec![9, 9],
            runtime_status: PluginRuntimeStatus::Loaded,
            midi_ports: Default::default(),
        }];
        project.patterns.truncate(1);
        project.patterns[0].channel_steps = vec![[false; 16]];
        project.patterns[0].channel_steps[0][0] = true;
        project.patterns[0].notes.clear();
        project.clips.truncate(1);
        project.clips[0].start = 0.0;
        project.clips[0].length = project.song_length_beats;
        project.clips[0].source_offset = 0.0;
        project.clips[0].pattern_id = project.patterns[0].id;
        let mut plan =
            InstrumentBouncePlan::prepare(&project, project.channels[0].id, 8000, 0).unwrap();
        assert_eq!(plan.timeline().duration_frames(), 128);
        plan.spec.vst3_helper_path = Some(self.root.join("helper.py"));
        let identity = BounceSourceIdentity {
            project_session: 11,
            project_fingerprint: plan.project_fingerprint(),
            timeline_revision: 12,
            epoch: 13,
            endpoint_id: 14,
            channel_id: plan.channel_id(),
            instance_id: 41,
        };
        let state = BouncePluginState {
            metadata: BouncePluginMetadata {
                class_uid: UID.into(),
                version: "1.3.4".into(),
                name: "Mock only".into(),
                category: "Instrument|Synth".into(),
                input_buses: vec![],
                output_buses: vec![(self.channels, true)],
                event_input_buses: vec![(16, true, Some(true))],
                event_output_buses: vec![],
                latency_samples: self.latency,
                tail_samples: u32::MAX,
            },
            opaque_state: if self.blocked_stdin {
                vec![1; 1024 * 1024]
            } else {
                vec![1, 2, 3]
            },
            parameter_bases: [(17, 0.1), (23, 0.75)].into(),
            native_generation: 2,
            native_dirty_revision: 9,
        };
        (
            project,
            CapturedInstrumentBounce {
                identity,
                plan,
                state,
            },
        )
    }
}
impl Drop for Mock {
    fn drop(&mut self) {
        if thread::panicking() {
            // Job/DirectOwner declarations follow this fixture and drain first.
            // Preserve raw failure evidence for the separately approved run.
            eprintln!(
                "Retained strict bounce mock evidence: {}",
                self.root.display()
            );
            return;
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn read_lines(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}
fn audit(root: &Path, value: Value) {
    let mut file = OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(root.join("audit.jsonl"))
        .unwrap();
    writeln!(file, "{value}").unwrap();
}
fn wait_until(mut predicate: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(15);
    while !predicate() {
        assert!(Instant::now() < end, "mock fixture wait expired");
        thread::sleep(Duration::from_millis(1));
    }
}
fn control(duration: Duration) -> RenderControl {
    RenderControl {
        export: ExportControl::default(),
        deadline: Instant::now() + duration,
    }
}

#[derive(Clone, Copy)]
enum Hook {
    None,
    CancelAfterProcess,
    CancelBeforeDispose,
    FaultThenCancel,
    PanicAfterProcess,
    CancelOnDrop,
    CancelAfterSpawn,
    CancelAfterStop,
    PanicAfterDisposal,
}

// This wrapper never implements protocol. It records actual adapter disposal and
// provides deterministic renderer-boundary actions outside the adapter/client.
struct Audited {
    backend: StrictBackend,
    root: PathBuf,
    hook: Hook,
    control: RenderControl,
    processed: usize,
    disposal_started: bool,
}
impl Audited {
    fn new(root: PathBuf, hook: Hook, control: RenderControl) -> Self {
        let backend = match hook {
            Hook::CancelAfterSpawn => StrictBackend::with_test_action(TestAction::CancelAfterSpawn),
            Hook::CancelAfterStop => StrictBackend::with_test_action(TestAction::CancelAfterStop),
            Hook::PanicAfterDisposal => {
                StrictBackend::with_test_action(TestAction::PanicAfterDisposalOnce)
            }
            _ => StrictBackend::new(),
        };
        Self {
            backend,
            root,
            hook,
            control,
            processed: 0,
            disposal_started: false,
        }
    }
}
impl RenderBackend for Audited {
    fn prepare(
        &mut self,
        capture: &CapturedInstrumentBounce,
        control: &RenderControl,
    ) -> Result<BouncePluginMetadata, BackendFailure> {
        let before = capture.state.clone();
        let identity = capture.identity();
        let result = self.backend.prepare(capture, control);
        assert_eq!(
            capture.state, before,
            "preparation changed authoritative capture"
        );
        assert_eq!(capture.identity(), identity);
        result
    }
    fn metadata(&mut self) -> Result<BouncePluginMetadata, BackendFailure> {
        self.backend.metadata()
    }
    fn send_midi(&mut self, midi: [u8; 3], offset: u16) -> Result<(), BackendFailure> {
        self.backend.send_midi(midi, offset)
    }
    fn process(
        &mut self,
        transport: PluginTransport,
        frames: usize,
    ) -> Result<BlockReply, BackendFailure> {
        let result = self.backend.process(transport, frames);
        self.processed += 1;
        if (matches!(self.hook, Hook::CancelAfterProcess) && self.processed == 2 && result.is_ok())
            || (matches!(self.hook, Hook::FaultThenCancel) && result.is_err())
        {
            self.control.export.cancel();
        }
        if matches!(self.hook, Hook::PanicAfterProcess) {
            panic!("mock owner panic after actual Process");
        }
        result
    }
    fn dispose(&mut self, deadline: Instant) -> DisposalObservation {
        if !self.disposal_started {
            self.disposal_started = true;
            if matches!(self.hook, Hook::CancelBeforeDispose) {
                self.control.export.cancel();
            }
        }
        let observation = self.backend.dispose(deadline);
        audit(
            &self.root,
            json!({ "kind": "disposal", "terminated": observation.terminated,
            "destination_unchanged": fs::read(self.root.join("render.wav")).unwrap() == SENTINEL,
            "error": observation.error.as_ref().map(|e| format!("{:?}", e.kind)),
            "diagnostics": observation.diagnostics }),
        );
        observation
    }
}
impl Drop for Audited {
    fn drop(&mut self) {
        if matches!(self.hook, Hook::CancelOnDrop) {
            self.control.export.cancel();
        }
        audit(
            &self.root,
            json!({"kind": "drop", "destination_unchanged": fs::read(self.root.join("render.wav")).unwrap() == SENTINEL}),
        );
    }
}

// Receipt Drop alone cancels but is not a join. This guard always drains the
// admission, including when an assertion panics, before Mock can delete paths.
struct Job<'a> {
    mock: &'a Mock,
    admission: RenderAdmission,
    receipt: Option<RenderReceipt>,
    control: RenderControl,
}
impl<'a> Job<'a> {
    fn start(mock: &'a Mock, hook: Hook, duration: Duration) -> Self {
        Self::with_control(mock, hook, control(duration))
    }
    fn with_control(mock: &'a Mock, hook: Hook, ctl: RenderControl) -> Self {
        let (project, capture) = mock.capture();
        let source_before = serde_json::to_vec(&project).unwrap();
        let admission = RenderAdmission::default();
        let root = mock.root.clone();
        let backend_control = ctl.clone();
        let receipt = admission
            .start(capture, mock.output(), true, ctl.clone(), move || {
                Audited::new(root, hook, backend_control)
            })
            .unwrap();
        let job = Self {
            mock,
            admission,
            receipt: Some(receipt),
            control: ctl,
        };
        assert_eq!(serde_json::to_vec(&project).unwrap(), source_before);
        job
    }
    fn finish(&self) -> RenderOutcome {
        let receipt = self.receipt.as_ref().unwrap();
        wait_until(|| receipt.termination_acknowledged());
        let result = receipt.try_recv().unwrap();
        assert_eq!(receipt.status(), RenderStatus::Terminated);
        assert!(!self.admission.is_busy());
        for entry in self.mock.audit() {
            assert_eq!(entry["destination_unchanged"], true);
        }
        result
    }
}
impl Drop for Job<'_> {
    fn drop(&mut self) {
        self.control.export.cancel();
        self.receipt.take();
        let _ = fs::write(self.mock.root.join("release"), b"release on guard drop");
        while self.admission.is_busy() {
            thread::sleep(Duration::from_millis(1));
        }
    }
}
fn failed(outcome: RenderOutcome) -> RenderFailure {
    match outcome {
        RenderOutcome::Failed(failure) => failure,
        success => panic!("unexpected publication: {success:?}"),
    }
}
fn normal_job(mock: &Mock) -> Job<'_> {
    Job::start(mock, Hook::None, Duration::from_secs(20))
}
fn assert_typed(failure: &RenderFailure, kind: StrictKind, delivery: BackendDelivery) {
    let backend = failure
        .backend
        .as_ref()
        .expect("typed strict backend evidence");
    assert_eq!(backend.strict_kind, Some(kind), "{failure:?}");
    assert_eq!(backend.delivery, Some(delivery));
    assert_eq!(
        failure.kind,
        match kind {
            StrictKind::Cancelled => FailureKind::Cancelled,
            StrictKind::Deadline => FailureKind::Deadline,
            _ => FailureKind::Backend,
        }
    );
}

#[test]
fn strict_clone_success_exact_order_source_identity_and_reap_before_wav() {
    let _serial = serial();
    for channels in [1, 2] {
        let mock = Mock::with_shape("", "", channels, 0);
        let (_, capture) = mock.capture();
        let identity = capture.identity();
        let scheduled =
            render::PreparedRender::from_capture(&capture, true, &control(Duration::from_secs(20)))
                .unwrap()
                .midi;
        let job = normal_job(&mock);
        let RenderOutcome::Complete(report) = job.finish() else {
            panic!("healthy mock failed")
        };
        assert_eq!(report.identity, identity);
        assert_eq!(
            (
                report.frames,
                report.generated_frames,
                report.native_latency
            ),
            (128, 256, 0)
        );
        assert_eq!(report.reported_tail, u32::MAX);
        let wav = crate::wav::read_wav(mock.output()).unwrap();
        assert_eq!(wav.frame_count(), 128);
        for frame in wav.samples.chunks_exact(2) {
            assert_eq!(frame, &[0.25, if channels == 1 { 0.25 } else { -0.5 }]);
        }
        let mut expected: Vec<String> = [
            "LoadPlugin:1",
            "SetProcessMode:1",
            "ResetOriginSupport:1",
            "LoadState:1",
            "StartProcessing:1",
            "NativeDirtyRevision:1",
            "GetAllParameters:1",
            "SetParameter:1",
            "SetParameter:2",
            "GetParameter:1",
            "GetParameter:2",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        for n in 1..=5 {
            for name in [
                "CheckedAudioBusLayout",
                "EventBusLayout",
                "LatencySamples",
                "TailSamples",
            ] {
                expected.push(format!("{name}:{n}"));
            }
            if n == 2 || n == 4 {
                expected.push(format!("SendMidiAt:{}", if n == 2 { 1 } else { 2 }));
                expected.push(format!("Process:{}", if n == 2 { 1 } else { 2 }));
            }
        }
        expected.extend(["StopProcessing:1".into(), "Shutdown:1".into()]);
        assert_eq!(mock.commands(), expected);
        let actual_notes: Vec<Value> = mock
            .ledger()
            .into_iter()
            .filter(|entry| {
                entry["kind"] == "command" && entry["command"].get("SendMidiAt").is_some()
            })
            .map(|entry| entry["command"]["SendMidiAt"].clone())
            .collect();
        let expected_notes: Vec<Value> = scheduled.into_iter().map(|note| {
            let kind = if note.data[0] & 0xf0 == 0x90 && note.data[2] > 0 { "NoteOn" } else { "NoteOff" };
            json!({ "event": { (kind): { "channel": format!("Ch{}", (note.data[0] & 15) + 1),
                "note": note.data[1], "velocity": note.data[2] } }, "sample_offset": note.frame % 128 })
        }).collect();
        assert_eq!(actual_notes, expected_notes);
        mock.assert_cooperative(true);
        let audit = mock.audit();
        assert!(
            audit.iter().any(|v| v["kind"] == "disposal"
                && v["terminated"] == true
                && v["error"].is_null())
        );
        assert_eq!(audit.last().unwrap()["kind"], "drop");
    }
}

#[test]
fn strict_clone_every_reply_stage_error_wrong_malformed_or_death_seals_without_replay() {
    let _serial = serial();
    let stages = [
        "LoadPlugin:1",
        "SetProcessMode:1",
        "ResetOriginSupport:1",
        "LoadState:1",
        "StartProcessing:1",
        "NativeDirtyRevision:1",
        "GetAllParameters:1",
        "SetParameter:1",
        "SetParameter:2",
        "GetParameter:1",
        "GetParameter:2",
        "CheckedAudioBusLayout:1",
        "EventBusLayout:1",
        "LatencySamples:1",
        "TailSamples:1",
        "CheckedAudioBusLayout:2",
        "EventBusLayout:2",
        "LatencySamples:2",
        "TailSamples:2",
        "SendMidiAt:1",
        "Process:1",
        "CheckedAudioBusLayout:3",
        "EventBusLayout:3",
        "LatencySamples:3",
        "TailSamples:3",
        "StopProcessing:1",
    ];
    for stage in stages {
        for fault in ["error", "wrong", "malformed", "death"] {
            let mock = Mock::new(fault, stage);
            let failure = failed(normal_job(&mock).finish());
            match fault {
                "error" => {
                    assert_typed(
                        &failure,
                        StrictKind::OperationRejected,
                        BackendDelivery::MayHaveApplied,
                    );
                    assert_eq!(failure.primary, format!("known mock failure {stage}"));
                }
                "wrong" => assert_typed(
                    &failure,
                    StrictKind::WrongReply,
                    BackendDelivery::MayHaveApplied,
                ),
                "malformed" => assert_typed(
                    &failure,
                    StrictKind::MalformedReply,
                    BackendDelivery::MayHaveApplied,
                ),
                "death" => {
                    let backend = failure.backend.as_ref().unwrap();
                    assert!(
                        matches!(
                            backend.strict_kind,
                            Some(StrictKind::ChildExited | StrictKind::Eof)
                        ),
                        "{failure:?}"
                    );
                    assert_eq!(backend.delivery, Some(BackendDelivery::MayHaveApplied));
                }
                _ => unreachable!(),
            }
            mock.assert_terminal_at(stage);
        }
    }
}

#[test]
fn strict_clone_semantic_preparation_rejections_keep_healthy_cleanup_and_all_bases_before_setters()
{
    let _serial = serial();
    for (fault, stage, stopped, no_setter) in [
        ("identity", "LoadPlugin:1", false, true),
        ("version", "LoadPlugin:1", false, true),
        ("support_version", "ResetOriginSupport:1", false, true),
        ("support_maximum", "ResetOriginSupport:1", false, true),
        ("support_processing", "ResetOriginSupport:1", false, true),
        ("catalog_missing", "GetAllParameters:1", true, true),
        ("catalog_readonly", "GetAllParameters:1", true, true),
        ("base_mismatch", "GetParameter:1", true, false),
        ("base_negative", "GetParameter:1", true, false),
        ("base_above_one", "GetParameter:1", true, false),
        ("base_nan", "GetParameter:1", true, false),
        ("base_inf", "GetParameter:1", true, false),
        ("category", "LoadPlugin:1", true, false),
        ("name", "LoadPlugin:1", true, false),
    ] {
        let mock = Mock::new(fault, stage);
        let failure = failed(normal_job(&mock).finish());
        assert_eq!(failure.kind, FailureKind::Backend, "{fault}: {failure:?}");
        assert_eq!(failure.backend.as_ref().unwrap().strict_kind, None);
        assert!(failure.cleanup.is_empty(), "{fault}: {failure:?}");
        assert!(!mock.commands().iter().any(|s| s.starts_with("Process:")));
        if no_setter {
            assert!(
                !mock
                    .commands()
                    .iter()
                    .any(|s| s.starts_with("SetParameter:"))
            );
        }
        if fault.starts_with("base_") {
            assert!(
                failure.primary.contains("raw=")
                    && failure.primary.contains("projected=")
                    && failure.primary.contains("captured=")
            );
        }
        mock.assert_unchanged();
        mock.assert_cooperative(stopped);
    }
    let mock = Mock::new("catalog_duplicate", "GetAllParameters:1");
    let failure = failed(normal_job(&mock).finish());
    assert_typed(
        &failure,
        StrictKind::MalformedReply,
        BackendDelivery::MayHaveApplied,
    );
    mock.assert_terminal_at("GetAllParameters:1");
}

#[test]
fn strict_clone_raw_metadata_transition_and_typed_refusal_reject_before_projection() {
    let _serial = serial();
    for occurrence in [1, 2, 3] {
        for (fault, name) in [
            ("audio_aux", "CheckedAudioBusLayout"),
            ("audio_unknown", "CheckedAudioBusLayout"),
            ("audio_inactive", "CheckedAudioBusLayout"),
            ("audio_input", "CheckedAudioBusLayout"),
            ("audio_extra_bus", "CheckedAudioBusLayout"),
            ("audio_channels", "CheckedAudioBusLayout"),
            ("event_aux", "EventBusLayout"),
            ("event_unknown", "EventBusLayout"),
            ("event_output", "EventBusLayout"),
            ("latency_drift", "LatencySamples"),
            ("tail_drift", "TailSamples"),
        ] {
            let mock = Mock::new(fault, &format!("{name}:{occurrence}"));
            let failure = failed(normal_job(&mock).finish());
            assert!(matches!(
                failure.kind,
                FailureKind::Backend | FailureKind::Invalid
            ));
            assert!(
                failure
                    .backend
                    .as_ref()
                    .is_none_or(|e| e.strict_kind.is_none())
            );
            assert!(failure.cleanup.is_empty(), "{fault}: {failure:?}");
            assert_eq!(
                mock.commands()
                    .iter()
                    .filter(|s| s.starts_with("Process:"))
                    .count(),
                usize::from(occurrence == 3)
            );
            mock.assert_unchanged();
            mock.assert_cooperative(true);
        }
        for name in ["CheckedAudioBusLayout", "EventBusLayout"] {
            for (reason, typed) in [
                ("Unsupported", MetadataQueryRejection::Unsupported),
                ("PendingTopology", MetadataQueryRejection::PendingTopology),
                (
                    "InconsistentTopology",
                    MetadataQueryRejection::InconsistentTopology,
                ),
            ] {
                let stage = format!("{name}:{occurrence}");
                let mock = Mock::new(&format!("metadata_{reason}"), &stage);
                let failure = failed(normal_job(&mock).finish());
                assert_typed(
                    &failure,
                    StrictKind::MetadataRejected(typed),
                    BackendDelivery::MayHaveApplied,
                );
                mock.assert_terminal_at(&stage);
            }
        }
    }
}

#[test]
fn strict_clone_exact_audio_shape_all_samples_transport_and_event_loss_gate_publication() {
    let _serial = serial();
    for occurrence in [1, 2] {
        for fault in [
            "short_channels",
            "extra_channels",
            "short_frames",
            "extra_frames",
            "nonfinite_first",
            "nonfinite_last",
            "missing_ack",
            "output_event",
            "output_loss",
        ] {
            // Nonfinite first sample is discarded latency, last sample of the
            // closing block is discarded padding. Both must still be checked.
            let mock = Mock::with_shape(fault, &format!("Process:{occurrence}"), 2, 1);
            let failure = failed(normal_job(&mock).finish());
            match fault {
                "short_channels" | "extra_channels" | "short_frames" | "extra_frames" => {
                    assert_typed(&failure, StrictKind::Shape, BackendDelivery::MayHaveApplied);
                    mock.assert_terminal_at(&format!("Process:{occurrence}"));
                }
                "output_event" => {
                    assert_typed(
                        &failure,
                        StrictKind::UnsupportedReply,
                        BackendDelivery::MayHaveApplied,
                    );
                    mock.assert_terminal_at(&format!("Process:{occurrence}"));
                }
                _ => {
                    assert_eq!(failure.kind, FailureKind::Invalid);
                    assert_eq!(failure.backend, None);
                    mock.assert_unchanged();
                    mock.assert_cooperative(true);
                }
            }
            assert_eq!(
                mock.commands()
                    .iter()
                    .filter(|s| s.starts_with("Process:"))
                    .count(),
                occurrence
            );
        }
    }
}

#[test]
fn strict_clone_cancel_and_original_deadline_interrupt_blocked_stages_without_later_io() {
    let _serial = serial();
    for stage in [
        "LoadPlugin:1",
        "LoadState:1",
        "GetAllParameters:1",
        "SetParameter:2",
        "GetParameter:2",
        "CheckedAudioBusLayout:1",
        "Process:1",
        "StopProcessing:1",
    ] {
        for cancel in [true, false] {
            let mock = Mock::new("block", stage);
            let job = Job::start(
                &mock,
                Hook::None,
                if cancel {
                    Duration::from_secs(20)
                } else {
                    Duration::from_secs(2)
                },
            );
            mock.wait_stage("blocked", stage);
            if cancel {
                assert!(job.control.export.clone().cancel());
            }
            let failure = failed(job.finish());
            assert_typed(
                &failure,
                if cancel {
                    StrictKind::Cancelled
                } else {
                    StrictKind::Deadline
                },
                BackendDelivery::MayHaveApplied,
            );
            mock.assert_terminal_at(stage);
        }
    }
}

#[test]
fn strict_clone_precancelled_and_expired_admission_never_spawns_or_loads() {
    let _serial = serial();
    for cancel in [true, false] {
        let mock = Mock::new("", "");
        let ctl = control(if cancel {
            Duration::from_secs(20)
        } else {
            Duration::ZERO
        });
        if cancel {
            ctl.export.cancel();
        }
        let job = Job::with_control(&mock, Hook::None, ctl);
        let failure = failed(job.finish());
        assert_eq!(
            failure.kind,
            if cancel {
                FailureKind::Cancelled
            } else {
                FailureKind::Deadline
            }
        );
        assert!(mock.ledger().is_empty());
        mock.assert_unchanged();
    }
}

#[test]
fn strict_clone_large_state_cancel_after_observed_stdin_prefix_is_partial_and_never_replayed() {
    let _serial = serial();
    let mock = Mock::new("block_stdin", "LoadState:1");
    let job = Job::start(&mock, Hook::None, Duration::from_secs(20));
    mock.wait_stage("blocked_input", "LoadState:1");
    assert!(job.control.export.clone().cancel());
    let failure = failed(job.finish());
    assert_typed(
        &failure,
        StrictKind::Cancelled,
        BackendDelivery::MayHaveApplied,
    );
    assert_eq!(
        mock.commands(),
        ["LoadPlugin:1", "SetProcessMode:1", "ResetOriginSupport:1"]
    );
    assert_eq!(
        mock.ledger()
            .iter()
            .filter(|entry| entry["kind"] == "blocked_input")
            .count(),
        1
    );
    assert!(
        failure
            .cleanup
            .iter()
            .any(|s| s.contains("forced termination")),
        "{failure:?}"
    );
    mock.assert_unchanged();
    mock.assert_reaped();
}

#[test]
fn strict_clone_published_cancel_closes_next_internal_admission_before_delayed_wake() {
    let _serial = serial();
    for stage in [
        "LoadPlugin:1",
        "CheckedAudioBusLayout:1",
        "CheckedAudioBusLayout:2",
    ] {
        let mock = Mock::new("block", stage);
        let job = normal_job(&mock);
        mock.wait_stage("blocked", stage);
        thread::scope(|scope| {
            // Declared inside the scope so unwind releases this bounded pause
            // before the scope joins the thread paused after the real CAS.
            let pause = job
                .control
                .export
                .pause_strict_cancel_before_wake(Instant::now() + Duration::from_secs(10));
            let cancelling = job.control.export.clone();
            let worker = scope.spawn(move || cancelling.cancel());
            assert!(pause.wait_until_paused());
            assert!(job.control.export.check_cancelled().is_err());
            // The strict client's wake remains paused. A valid in-flight reply
            // succeeds; the owner's sticky-state sync must reject its NEXT call.
            fs::write(mock.root.join("release"), b"return valid blocked reply").unwrap();
            let failure = failed(job.finish());
            assert_typed(&failure, StrictKind::Cancelled, BackendDelivery::NotSent);
            mock.assert_terminal_at(stage);
            pause.release();
            assert!(worker.join().unwrap());
        });
    }
}

#[test]
fn strict_clone_cancellation_boundary_matrix_preserves_typed_cleanup_and_final_fence() {
    let _serial = serial();
    for hook in [
        Hook::CancelAfterSpawn,
        Hook::CancelAfterProcess,
        Hook::CancelBeforeDispose,
        Hook::CancelAfterStop,
        Hook::CancelOnDrop,
    ] {
        let mock = Mock::new("", "");
        let failure = failed(Job::start(&mock, hook, Duration::from_secs(20)).finish());
        assert_eq!(failure.kind, FailureKind::Cancelled, "{failure:?}");
        match hook {
            Hook::CancelAfterSpawn => {
                assert_typed(&failure, StrictKind::Cancelled, BackendDelivery::NotSent);
                assert!(mock.commands().is_empty());
            }
            Hook::CancelBeforeDispose => {
                assert_typed(&failure, StrictKind::Cancelled, BackendDelivery::NotSent);
                assert_eq!(mock.commands().last().unwrap(), "TailSamples:5");
            }
            Hook::CancelAfterStop => {
                assert_typed(&failure, StrictKind::Cancelled, BackendDelivery::NotSent);
                assert_eq!(mock.commands().last().unwrap(), "StopProcessing:1");
            }
            Hook::CancelAfterProcess => {
                assert_eq!(failure.backend, None);
                assert_eq!(mock.commands().last().unwrap(), "Process:2");
                assert!(!failure.cleanup.is_empty());
            }
            Hook::CancelOnDrop => {
                assert_eq!(failure.backend, None);
                mock.assert_cooperative(true);
            }
            _ => unreachable!(),
        }
        mock.assert_unchanged();
        mock.assert_reaped();
    }
    let mock = Mock::new("error", "Process:1");
    let failure =
        failed(Job::start(&mock, Hook::FaultThenCancel, Duration::from_secs(20)).finish());
    assert_typed(
        &failure,
        StrictKind::OperationRejected,
        BackendDelivery::MayHaveApplied,
    );
    assert_eq!(failure.primary, "known mock failure Process:1");
    mock.assert_terminal_at("Process:1");
}

#[test]
fn strict_clone_receipt_drop_and_assertion_unwind_drain_same_child_and_allow_next_candidate() {
    let _serial = serial();
    for unwind in [false, true] {
        let mock = Mock::new("block", "Process:1");
        let mut job = normal_job(&mock);
        mock.wait_stage("blocked", "Process:1");
        assert!(job.admission.is_busy());
        if unwind {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _owned_guard = job;
                panic!("intentional fixture assertion unwind");
            }));
            assert!(result.is_err());
        } else {
            drop(job.receipt.take());
            wait_until(|| !job.admission.is_busy());
            assert_eq!(job.admission.status(), Some(RenderStatus::Terminated));
        }
        mock.assert_terminal_at("Process:1");
        let next = Mock::new("", "");
        assert!(matches!(
            normal_job(&next).finish(),
            RenderOutcome::Complete(_)
        ));
        next.assert_cooperative(true);
    }
}

#[test]
fn strict_clone_owner_panic_disposal_panic_and_nonzero_or_forced_exit_never_publish() {
    let _serial = serial();
    for hook in [Hook::PanicAfterProcess, Hook::PanicAfterDisposal] {
        let mock = Mock::new("", "");
        let failure = failed(Job::start(&mock, hook, Duration::from_secs(20)).finish());
        assert_eq!(failure.kind, FailureKind::Panic);
        mock.assert_unchanged();
        mock.assert_cooperative(true);
        assert_eq!(
            mock.commands()
                .iter()
                .filter(|s| s.starts_with("StopProcessing:"))
                .count(),
            1
        );
        assert_eq!(
            mock.commands()
                .iter()
                .filter(|s| s.starts_with("Shutdown:"))
                .count(),
            1
        );
    }
    for fault in ["nonzero_exit", "hold_exit"] {
        let mock = Mock::new(fault, "");
        let failure = failed(normal_job(&mock).finish());
        assert_eq!(failure.kind, FailureKind::Backend);
        assert!(
            failure
                .cleanup
                .iter()
                .any(|s| s.contains("abnormal status")),
            "{failure:?}"
        );
        if fault == "hold_exit" {
            assert!(
                failure
                    .cleanup
                    .iter()
                    .any(|s| s.contains("forced termination"))
            );
        }
        assert_eq!(mock.commands().last().unwrap(), "Shutdown:1");
        mock.assert_unchanged();
        mock.assert_reaped();
    }
}

struct DirectOwner {
    backend: StrictBackend,
    cleanup_deadline: Option<Instant>,
    terminated: bool,
}
impl DirectOwner {
    fn new() -> Self {
        Self {
            backend: StrictBackend::new(),
            cleanup_deadline: None,
            terminated: false,
        }
    }
    fn dispose(&mut self, deadline: Instant) -> DisposalObservation {
        let deadline = *self.cleanup_deadline.get_or_insert(deadline);
        let result = self.backend.dispose(deadline);
        self.terminated = result.terminated;
        result
    }
}
impl Drop for DirectOwner {
    fn drop(&mut self) {
        // Never abandon a strict owner, even if the test assertion unwinds.
        while !self.terminated {
            self.dispose(Instant::now() + Duration::from_secs(5));
            if !self.terminated {
                thread::sleep(Duration::from_millis(1));
            }
        }
    }
}

#[test]
fn strict_clone_pending_retains_same_reservation_until_force_and_concrete_reap() {
    let _serial = serial();
    let mock = Mock::new("hold_exit", "");
    let (_, capture) = mock.capture();
    let mut owner = DirectOwner::new();
    let ctl = control(Duration::from_secs(20));
    owner.backend.prepare(&capture, &ctl).unwrap();
    // An expired cleanup budget cannot be promoted into a new protocol budget.
    // The live helper deliberately holds on EOF, guaranteeing this first Pending.
    let first = owner.dispose(Instant::now());
    assert!(!first.terminated);
    let first_error = first.error.as_ref().unwrap();
    assert_eq!(first_error.kind, BackendFailureKind::Deadline);
    assert_eq!(first_error.strict_kind, Some(StrictKind::Deadline));
    assert_eq!(first_error.delivery, Some(BackendDelivery::NotSent));
    let mut second = DirectOwner::new();
    let blocked = second.backend.prepare(&capture, &ctl).unwrap_err();
    assert_eq!(blocked.strict_kind, Some(StrictKind::Busy));
    assert_eq!(blocked.delivery, Some(BackendDelivery::NotSent));
    assert!(second.dispose(Instant::now()).terminated);
    let end = Instant::now() + Duration::from_secs(10);
    let last = loop {
        let observation = owner.dispose(Instant::now() + Duration::from_secs(30));
        if observation.terminated {
            break observation;
        }
        assert!(Instant::now() < end, "same owner failed to reap");
        thread::sleep(Duration::from_millis(1));
    };
    assert_eq!(last.error.as_ref(), Some(first_error));
    assert!(
        last.diagnostics
            .iter()
            .any(|s| s.contains("forced termination"))
    );
    mock.assert_terminal_at("TailSamples:1");
    let next = Mock::new("", "");
    assert!(matches!(
        normal_job(&next).finish(),
        RenderOutcome::Complete(_)
    ));
    next.assert_cooperative(true);
}
