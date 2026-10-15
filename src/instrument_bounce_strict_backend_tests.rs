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
use crate::instrument_bounce::BounceAudioBus;
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
#[cfg(target_os = "linux")]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
    thread,
};
use vst3_host::strict_offline as strict;
use vst3_host::{MetadataQueryRejection, strict_offline::FailureKind as StrictKind};

static NEXT: AtomicU64 = AtomicU64::new(1);
const UID: &str = "ABCDEF019182FAEB566D624153675854";
const SENTINEL: &[u8] = b"unchanged destination";

fn serial() -> MutexGuard<'static, ()> {
    crate::plugins::helper_transport_test_lock()
}

// No shell, environment overrides, network, real bundle, or external assets.
// The helper validates all request scalars and logs complete commands including
// PID and per-command occurrence before responding. JSON lines remain atomic
// enough for the one writer; Rust owner observations use a separate audit file.
const MOCK: &str = r#"#!/usr/bin/env python3
import atexit, base64, json, os, pathlib, select, struct, sys, time
root = pathlib.Path(__file__).parent
cfg = json.loads((root / 'config.json').read_text())
if cfg['fault'] == 'delay_start':
    # A real child may be cancelled before Python reaches any startup ledger.
    # This test never creates the dedicated release marker: even a long parent
    # scheduling pause cannot let Python advance into the ledger. Cancellation
    # remains immediate and the unchanged EOF cleanup grace reaps this child.
    while not (root / 'preledger-release').exists():
        time.sleep(.01)
ledger = open(root / 'ledger.jsonl', 'a', buffering=1)
counts = {}
processed = 0
sets = []
reads = []
audio_activation = {}

if os.name == 'nt':
    import ctypes, msvcrt
    from ctypes import wintypes
    peek_pipe = ctypes.WinDLL('kernel32', use_last_error=True).PeekNamedPipe
    peek_pipe.argtypes = [wintypes.HANDLE, wintypes.LPVOID, wintypes.DWORD,
                          ctypes.POINTER(wintypes.DWORD), ctypes.POINTER(wintypes.DWORD),
                          ctypes.POINTER(wintypes.DWORD)]
    peek_pipe.restype = wintypes.BOOL

def stdin_ready():
    if os.name != 'nt':
        return bool(select.select([sys.stdin], [], [], .01)[0])
    available = wintypes.DWORD()
    if not peek_pipe(msvcrt.get_osfhandle(0), None, 0, None, ctypes.byref(available), None):
        error = ctypes.get_last_error()
        if error in (109, 232):
            return True
        raise OSError(error, 'PeekNamedPipe failed in mock stdin readiness')
    if not available.value:
        time.sleep(.01)
    return bool(available.value)
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
        if stdin_ready():
            extra = os.read(0, 4096)
            if not extra:
                eof()
                return False
            log('unexpected_after_block', data=extra.decode('utf-8', 'replace'))
            raise AssertionError('protocol continued after blocked terminal exchange')
    return True
def wait_bundle_getter_until_terminal_eof(stage):
    # Unlike the ordinary blocking fault, EOF is a witness of actual host
    # terminal cleanup, not permission to discard the delivered getter.
    log('blocked', stage=stage)
    while True:
        if stdin_ready():
            extra = os.read(0, 4096)
            assert not extra, 'protocol continued while bundle getter was held'
            log('late_getter_terminal_eof', stage=stage)
            return
        if (root / 'release').exists():
            # Unwind cleanup can release this fixture before a test assertion.
            log('late_getter_guard_release', stage=stage)
            return

def parameter(ident):
    return dict(id=ident, name='base '+str(ident), value=.5, min=0., max=1.,
                default=.25, unit='', step_count=0, can_automate=True,
                is_read_only=False, is_bypass=False, flags=0)
getter_counts = {}
def metadata_reply(name, fault, stage):
    if name == 'CheckedAudioBusLayout':
        layout = dict(inputs=[dict(bus) for bus in cfg.get('inputs', [])],
                      outputs=[dict(channel_count=cfg['channels'], is_main=True, active=True)] +
                              [dict(bus) for bus in cfg.get('extra_outputs', [])])
        if cfg.get('selected_stereo', False):
            for direction, buses in [('Input', layout['inputs']), ('Output', layout['outputs'])]:
                for index, bus in enumerate(buses):
                    bus['active'] = audio_activation.get((direction, index), True)
        if fault == 'audio_aux': layout['outputs'][0]['is_main'] = False
        if fault == 'audio_unknown': layout['outputs'][0]['active'] = None
        if fault == 'audio_inactive': layout['outputs'][0]['active'] = False
        if fault == 'audio_input': layout['inputs'] = [dict(channel_count=1, is_main=True, active=True)]
        if fault == 'audio_extra_bus': layout['outputs'].append(dict(channel_count=1, is_main=False, active=False))
        if fault == 'audio_channels': layout['outputs'][0]['channel_count'] = 3
        if fault == 'audio_input_unknown': layout['inputs'][0]['active'] = None
        if fault == 'audio_input_aux': layout['inputs'][0]['is_main'] = False
        if fault == 'audio_input_main': layout['inputs'][0]['is_main'] = True
        if fault == 'audio_extra_input': layout['inputs'].append(dict(channel_count=1, is_main=layout['inputs'][0]['is_main'], active=False))
        if fault == 'audio_output_missing': layout['outputs'] = []
        if fault == 'audio_input_channels': layout['inputs'][0]['channel_count'] = 3
        if fault == 'audio_input_activation_drift': layout['inputs'][0]['active'] = False
        if fault == 'audio_active_aux': layout['outputs'][-1]['active'] = True
        if fault == 'audio_aux_unknown': layout['outputs'][-1]['active'] = None
        if fault == 'audio_aux_main': layout['outputs'][-1]['is_main'] = True
        if fault == 'audio_aux_aux': layout['outputs'][-1]['is_main'] = False
        if fault == 'audio_aux_channels': layout['outputs'][-1]['channel_count'] = 3
        if fault == 'audio_aux_drift': layout['outputs'][-1]['channel_count'] = 1
        if fault == 'audio_aux_order': layout['outputs'][1:] = reversed(layout['outputs'][1:])
        if fault == 'audio_aux_removed': layout['outputs'].pop()
        if fault == 'audio_extra_aux': layout['outputs'].append(dict(channel_count=2, is_main=False, active=False))
        if fault.startswith('audio_'): log('injected_audio_topology', stage=stage, fault=fault, layout=layout)
        log('audio_topology', stage=stage, layout=layout)
        response = {'CheckedAudioBusLayout': {'layout': layout}}
    elif name == 'EventBusLayout':
        layout = dict(inputs=[dict(channel_count=16, is_main=True, active=True)],
                      outputs=[dict(channel_count=16, is_main=True, active=True)] if cfg.get('selected_stereo', False) else [])
        if fault == 'event_aux': layout['inputs'][0]['is_main'] = False
        if fault == 'event_unknown': layout['inputs'][0]['active'] = None
        if fault == 'event_output': layout['outputs'] = [dict(channel_count=1, is_main=True, active=True)]
        response = {'EventBusLayout': {'layout': layout}}
    elif name == 'LatencySamples':
        response = {'LatencySamples': {'samples': cfg['latency'] + (1 if fault == 'latency_drift' else 0)}}
    elif name == 'TailSamples':
        response = {'TailSamples': {'samples': 0 if fault == 'tail_drift' else 4294967295}}
    return response
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
        if cfg['fault'] == 'delay_exit':
            log('shutdown_received_before_natural_delay', stage=stage)
            # A genuine child teardown taking longer than the old 100 ms grace.
            time.sleep(.35)
            log('natural_exit_ready', stage=stage)
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
    if fault in ('error', 'wrong', 'malformed', 'limit'):
        log('injected_fault', stage=stage, fault=fault)
        if fault == 'error':
            reply({'Error': {'message': 'known mock failure '+stage}})
        elif fault == 'wrong':
            reply({'PluginInfo': {}} if name != 'LoadPlugin' else {'Success': {'message': 'wrong'}})
        elif fault == 'malformed':
            print('{malformed', flush=True)
        else:
            # Exceed the existing text cap, without changing any transport budget.
            reply({'Error': {'message': 'x' * (64 * 1024 + 1)}})
        continue
    success = {'Success': {'message': 'ok'}}
    if name == 'LoadPlugin':
        assert args == dict(path=str(root / 'mock-only.vst3'), sample_rate=8000.,
                           block_size=2048, tempo=120., time_sig_numerator=4,
                           time_sig_denominator=4, class_id=cfg['uid']), args
        info = dict(vendor='Mock', name='Mock only', version=cfg.get('version', '1.3.4'),
                    category='Instrument|Synth', uid=cfg['uid'], has_gui=False,
                    audio_inputs=0, audio_outputs=1+len(cfg.get('extra_outputs', [])), output_channels=cfg['channels'],
                    has_midi_input=True, has_midi_output=cfg.get('selected_stereo', False), compatibility=[])
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
        audio_activation.clear()
        response = success
    elif name == 'SetBusActive':
        assert args['media_type'] == 'Audio', 'event outputs must not be disabled'
        assert args['direction'] in ('Input', 'Output')
        maximum = len(cfg.get('inputs', [])) if args['direction'] == 'Input' else 1+len(cfg.get('extra_outputs', []))
        assert 0 <= args['bus_index'] < maximum
        if fault != 'ack_without_participation':
            audio_activation[(args['direction'], args['bus_index'])] = args['active']
        log('audio_ack', direction=args['direction'], index=args['bus_index'], active=args['active'])
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
        if fault == 'catalog_limit': params = [parameter(index) for index in range(4097)]
        if fault == 'catalog_max': params = [parameter(index) for index in range(4096)]
        if fault == 'catalog_name_limit': params[0]['name'] = 'x' * 1025
        if fault == 'catalog_unit_limit': params[0]['unit'] = 'x' * 1025
        response = {'Parameters': {'params': params}}
    elif name == 'SetParameter':
        assert [args['id'], args['value']] == cfg['bases'][len(sets)], args
        assert not reads, 'setter after readback'
        sets.append(args['id'])
        response = success
    elif name == 'GetParameter':
        assert sets == [base[0] for base in cfg['bases']], sets
        assert args['id'] == cfg['bases'][len(reads)][0], args
        reads.append(args['id'])
        # Deliberately different raw f64 that projects to the same captured f32.
        value = .1 if args['id'] == 17 else .75
        if fault == 'base_mismatch': value = .100001
        if fault == 'base_negative': value = -.001
        if fault == 'base_above_one': value = 1.001
        if fault == 'base_nan': value = 'NaN'
        if fault == 'base_inf': value = 'inf'
        response = {'ParameterValue': {'value': value}}
    elif name in ('CheckedAudioBusLayout', 'EventBusLayout', 'LatencySamples', 'TailSamples'):
        getter_counts[name] = getter_counts.get(name, 0) + 1
        log('metadata_getter', getter=name, stage=name+':'+str(getter_counts[name]), wire_stage=stage)
        response = metadata_reply(name, fault, stage)
    elif name == 'StrictMetadataSnapshot':
        assert args == {'policy': 'Kontakt841SelectedStereo'}, args
        if cfg['uid'] != '5653544E694B386B6F6E74616B742038' or cfg.get('version') != '8.4.1':
            response = {'StrictMetadataAdmissionRejected': {'stage': 'FactoryIdentity'}}
        else:
            snapshot = {}
            response = None
            for getter, field in [('CheckedAudioBusLayout', 'audio'), ('EventBusLayout', 'events'), ('LatencySamples', 'latency_samples'), ('TailSamples', 'tail_samples')]:
                getter_counts[getter] = getter_counts.get(getter, 0) + 1
                inner_stage = getter+':'+str(getter_counts[getter])
                log('metadata_getter', getter=getter, stage=inner_stage, wire_stage=stage)
                inner_fault = cfg['fault'] if cfg['stage'] == inner_stage else ''
                if inner_fault == 'death': os._exit(17)
                if inner_fault == 'late_after_eof':
                    wait_bundle_getter_until_terminal_eof(inner_stage)
                    log('late_getter_completed', stage=inner_stage)
                if inner_fault == 'block' and not wait_block(inner_stage):
                    response = None
                    break
                if inner_fault == 'error':
                    response = {'StrictMetadataFailed': {'getter': getter, 'message': 'known mock failure '+inner_stage}}
                    break
                if inner_fault == 'limit':
                    response = {'StrictMetadataFailed': {'getter': getter, 'message': 'x' * (64 * 1024 + 1)}}
                    break
                if inner_fault == 'malformed':
                    print('{malformed', flush=True)
                    response = None
                    break
                if inner_fault == 'wrong':
                    response = {'PluginInfo': {}}
                    break
                value = metadata_reply(getter, inner_fault, inner_stage)[getter]
                snapshot[field] = value['layout'] if getter in ('CheckedAudioBusLayout', 'EventBusLayout') else value['samples']
                if getter == 'EventBusLayout':
                    inputs = snapshot['events']['inputs']
                    if len(inputs) != 1 or not inputs[0]['is_main'] or inputs[0]['active'] is not True or not 1 <= inputs[0]['channel_count'] <= 16:
                        response = {'StrictMetadataAdmissionRejected': {'stage': 'EventInput'}}
                        break
            else:
                response = {'StrictMetadataSnapshot': {'snapshot': snapshot}}
            if response is None:
                break
    elif name == 'SendMidiAt':
        assert 0 <= args['sample_offset'] < 128, args
        event = args['event']
        assert len(event) == 1 and next(iter(event)) in ('NoteOn', 'NoteOff'), event
        response = success
    elif name == 'Process':
        assert reads == [base[0] for base in cfg['bases']], reads
        assert args['frames'] == 128, args
        input_channels = sum(bus['channel_count'] for bus in cfg.get('inputs', []) if bus['active'])
        assert len(args['inputs']) == input_channels, args
        for encoded in args['inputs']:
            assert base64.b64decode(encoded, validate=True) == bytes(128 * 4), 'nonzero or malformed generator input'
        log('silent_inputs', channels=input_channels, frames=128, bits='positive_zero')
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
    if cfg['fault'] == 'late_after_eof' and name == 'StrictMetadataSnapshot':
        assert 'StrictMetadataSnapshot' in response, response
        # Complete all four fields after the real host stopped. Cleanup may
        # already have closed stdout; record the complete wire and attempted
        # write independently, never call that a delivered or accepted reply.
        encoded = json.dumps(response, separators=(',', ':'))
        log('late_complete_snapshot', stage=stage, snapshot=response,
            encoded_bytes=len(encoded.encode('utf-8')))
        try:
            print(encoded, flush=True)
        except OSError as error:
            # Windows may report EINVAL rather than BrokenPipeError when the
            # real host closes this pipe. Preserve the actual write outcome;
            # the error alone is not evidence that a reply was delivered.
            log('late_snapshot_write_failed', stage=stage,
                error_type=type(error).__name__, errno=error.errno,
                winerror=getattr(error, 'winerror', None),
                message=str(error))
            sys.stdout = open(os.devnull, 'w')
        else:
            log('late_snapshot_written', stage=stage)
        break
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
    // Channels, declared Main role, and known activation are independent.
    input: Option<(usize, bool, bool)>,
    // Extra outputs retain their declared role while remaining known inactive.
    extra_outputs: Vec<(usize, bool)>,
    selected_stereo: bool,
    empty_bases: bool,
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
        #[cfg(target_os = "linux")]
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let this = Self {
            root,
            channels,
            latency,
            blocked_stdin: fault == "block_stdin",
            input: None,
            extra_outputs: Vec::new(),
            selected_stereo: false,
            empty_bases: false,
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
        #[cfg(target_os = "linux")]
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
    fn with_topology(
        mut self,
        input: Option<(usize, bool, bool)>,
        extra_outputs: Vec<(usize, bool)>,
    ) -> Self {
        self.input = input;
        self.extra_outputs = extra_outputs;
        let path = self.root.join("config.json");
        let mut config: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        config["inputs"] = json!(input.into_iter().map(|(channels, is_main, active)|
            json!({"channel_count": channels, "is_main": is_main, "active": active})).collect::<Vec<_>>());
        config["extra_outputs"] = json!(self.extra_outputs.iter().map(|&(channels, is_main)|
            json!({"channel_count": channels, "is_main": is_main, "active": false})).collect::<Vec<_>>());
        fs::write(path, serde_json::to_vec(&config).unwrap()).unwrap();
        self
    }
    fn write(&self, name: &str, bytes: &[u8]) {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(target_os = "linux")]
        options.mode(0o600);
        let mut file = options.open(self.root.join(name)).unwrap();
        file.write_all(bytes).unwrap();
    }
    fn with_selected_stereo(mut self) -> Self {
        self = self.with_topology(None, vec![(2, true); 31]);
        self.selected_stereo = true;
        let path = self.root.join("config.json");
        let mut config: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        config["uid"] = json!("5653544E694B386B6F6E74616B742038");
        config["version"] = json!("8.4.1");
        config["selected_stereo"] = json!(true);
        fs::write(path, serde_json::to_vec(&config).unwrap()).unwrap();
        self
    }
    fn with_empty_bases(mut self) -> Self {
        self.empty_bases = true;
        let path = self.root.join("config.json");
        let mut config: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        config["bases"] = json!([]);
        fs::write(path, serde_json::to_vec(&config).unwrap()).unwrap();
        self
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
        #[cfg(target_os = "linux")]
        assert!(
            !Path::new(&format!("/proc/{}", pids[0])).exists(),
            "child not reaped"
        );
        #[cfg(windows)]
        assert_windows_pid_exited(pids[0]);
        assert!(
            ledger
                .iter()
                .filter_map(|v| v["destination_unchanged"].as_bool())
                .all(|v| v)
        );
        assert!(!ledger.iter().any(|v| v["kind"] == "unexpected_after_block"));
    }
    fn assert_pre_dispatch_cancel_reaped(&self) {
        let audit = self.audit();
        let disposal = audit
            .iter()
            .rev()
            .find(|entry| entry["kind"] == "disposal")
            .expect("pre-dispatch cancellation must have an owner disposal observation");
        assert!(
            concrete_child_reap_evidence(disposal),
            "one spawned OS child must be concretely reaped: {disposal:?}"
        );
        // CancelAfterSpawn has no startup handshake: Python may not yet have
        // opened its ledger when the same owned child is killed and reaped.
        // When startup did run, retain the independent PID exit check as well.
        if !self.ledger().is_empty() {
            self.assert_reaped();
        }
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
            if self.selected_stereo {
                let layouts = commands
                    .iter()
                    .filter(|stage| stage.starts_with("CheckedAudioBusLayout:"))
                    .count();
                assert_eq!(
                    commands[commands.len() - 3..],
                    [
                        "StopProcessing:1".to_owned(),
                        format!("CheckedAudioBusLayout:{layouts}"),
                        "Shutdown:1".to_owned(),
                    ],
                    "selected cleanup must verify its complete layout after Stop, before Shutdown"
                );
            } else {
                assert_eq!(commands[commands.len() - 2], "StopProcessing:1");
            }
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
            parameters: if self.empty_bases {
                Default::default()
            } else {
                [(17, 0.25), (23, 0.5)].into()
            },
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
                class_uid: if self.selected_stereo {
                    "5653544E694B386B6F6E74616B742038"
                } else {
                    UID
                }
                .into(),
                version: if self.selected_stereo {
                    "8.4.1"
                } else {
                    "1.3.4"
                }
                .into(),
                name: "Mock only".into(),
                category: "Instrument|Synth".into(),
                input_buses: self
                    .input
                    .into_iter()
                    .map(|(channels, is_main, active)| {
                        BounceAudioBus::new(channels, is_main, Some(active))
                    })
                    .collect(),
                output_buses: std::iter::once(BounceAudioBus::new(self.channels, true, Some(true)))
                    .chain(self.extra_outputs.iter().map(|&(channels, is_main)| {
                        BounceAudioBus::new(channels, is_main, Some(false))
                    }))
                    .collect(),
                event_input_buses: vec![(16, true, Some(true))],
                event_output_buses: if self.selected_stereo {
                    vec![(16, true, Some(true))]
                } else {
                    vec![]
                },
                latency_samples: self.latency,
                tail_samples: u32::MAX,
            },
            opaque_state: if self.blocked_stdin {
                vec![1; 1024 * 1024]
            } else {
                vec![1, 2, 3]
            },
            parameter_bases: if self.empty_bases {
                Default::default()
            } else {
                [(17, 0.1), (23, 0.75)].into()
            },
            native_generation: 2,
            native_dirty_revision: 9,
        };
        (
            project,
            CapturedInstrumentBounce {
                receipt_token: Arc::new(()),
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

fn concrete_child_reap_evidence(disposal: &Value) -> bool {
    disposal["kind"] == "disposal"
        && disposal["terminated"] == true
        && disposal["destination_unchanged"] == true
        && disposal["client_spawn_successes"].as_u64() == Some(1)
        && disposal["child_reaped"] == true
        && disposal["child_reap_status"]
            .as_str()
            .is_some_and(|status| !status.is_empty())
}

#[cfg(windows)]
fn assert_windows_pid_exited(pid: u64) {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use winapi::um::{
        processthreadsapi::OpenProcess,
        synchapi::WaitForSingleObject,
        winbase::WAIT_OBJECT_0,
        winnt::{PROCESS_QUERY_LIMITED_INFORMATION, SYNCHRONIZE},
    };
    let process = unsafe {
        OpenProcess(
            SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            u32::try_from(pid).unwrap(),
        )
    };
    if process.is_null() {
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(87),
            "PID absence must be authoritative, not an access error"
        );
        return;
    }
    let process = unsafe { OwnedHandle::from_raw_handle(process.cast()) };
    assert_eq!(
        unsafe { WaitForSingleObject(process.as_raw_handle().cast(), 0) },
        WAIT_OBJECT_0,
        "actual Windows child is still alive after reported disposal"
    );
}
fn audit(root: &Path, value: Value) {
    let mut options = OpenOptions::new();
    options.append(true).create(true);
    #[cfg(target_os = "linux")]
    options.mode(0o600);
    let mut file = options.open(root.join("audit.jsonl")).unwrap();
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
        #[cfg(windows)]
        let backend = backend.with_python_mock();
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
        let (client_spawn_successes, child_reap_status) = self.backend.test_child_completion();
        audit(
            &self.root,
            json!({ "kind": "disposal", "terminated": observation.terminated,
            "destination_unchanged": fs::read(self.root.join("render.wav")).unwrap() == SENTINEL,
            "client_spawn_successes": client_spawn_successes,
            "child_reaped": child_reap_status.is_some(),
            "child_reap_status": child_reap_status.map(|status| status.to_string()),
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
fn selected_source_project_snapshot(project: &Project) -> Value {
    serde_json::to_value(project).unwrap()
}

#[test]
fn strict_clone_selected_stereo_reselects_after_state_and_preserves_complete_declared_midi_output()
{
    let _serial = serial();
    let mock = Mock::new("", "").with_selected_stereo();
    let (project, capture) = mock.capture();
    let project_before = selected_source_project_snapshot(&project);
    let metadata = capture.metadata().clone();
    let state = capture.state.opaque_state.clone();
    let bases = capture.state.parameter_bases.clone();
    let RenderOutcome::Complete(report) = normal_job(&mock).finish() else {
        panic!("selected stereo mock failed");
    };
    assert_eq!(report.frames, 128);
    assert_eq!(metadata.output_buses.len(), 32);
    assert_eq!(metadata.event_output_buses, [(16, true, Some(true))]);
    let ledger = mock.ledger();
    let acknowledgments: Vec<_> = ledger
        .iter()
        .filter(|entry| entry["kind"] == "audio_ack")
        .collect();
    assert_eq!(acknowledgments.len(), 32);
    for (index, ack) in acknowledgments.iter().enumerate() {
        assert_eq!(ack["direction"], "Output");
        assert_eq!(ack["index"], index);
        assert_eq!(ack["active"], index == 0);
    }
    let commands = mock.commands();
    let load = commands
        .iter()
        .position(|stage| stage == "LoadState:1")
        .unwrap();
    let first_ack = commands
        .iter()
        .position(|stage| stage == "SetBusActive:1")
        .unwrap();
    let last_ack = commands
        .iter()
        .position(|stage| stage == "SetBusActive:32")
        .unwrap();
    let start = commands
        .iter()
        .position(|stage| stage == "StartProcessing:1")
        .unwrap();
    assert!(load < first_ack && last_ack < start);
    let mut expected: Vec<String> = [
        "LoadPlugin:1",
        "SetProcessMode:1",
        "ResetOriginSupport:1",
        "LoadState:1",
        "CheckedAudioBusLayout:1",
        "ResetOriginSupport:2",
        "CheckedAudioBusLayout:2",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    expected.extend((1..=32).map(|index| format!("SetBusActive:{index}")));
    expected.extend(
        [
            "CheckedAudioBusLayout:3",
            "CheckedAudioBusLayout:4",
            "StartProcessing:1",
            "CheckedAudioBusLayout:5",
            "NativeDirtyRevision:1",
            "GetAllParameters:1",
            "SetParameter:1",
            "SetParameter:2",
            "GetParameter:1",
            "GetParameter:2",
        ]
        .into_iter()
        .map(str::to_owned),
    );
    let mut next_layout = 6;
    for metadata in 1..=5 {
        expected.push(format!("StrictMetadataSnapshot:{metadata}"));
        if metadata == 2 || metadata == 4 {
            let process = if metadata == 2 { 1 } else { 2 };
            expected.extend([
                format!("SendMidiAt:{process}"),
                format!("CheckedAudioBusLayout:{next_layout}"),
                format!("Process:{process}"),
                format!("CheckedAudioBusLayout:{}", next_layout + 1),
            ]);
            next_layout += 2;
        }
    }
    expected.extend([
        "StopProcessing:1".to_owned(),
        format!("CheckedAudioBusLayout:{next_layout}"),
        "Shutdown:1".to_owned(),
    ]);
    assert_eq!(
        commands, expected,
        "selected clone must retain complete layout checks around Start, each Q128 Process, and after Stop before Shutdown"
    );
    assert!(
        ledger
            .iter()
            .filter(|entry| entry["kind"] == "command")
            .all(|entry| {
                entry["command"]
                    .get("SetBusActive")
                    .is_none_or(|args| args["media_type"] == "Audio")
            }),
        "declared MIDI output must survive without being disabled"
    );
    let wav = crate::wav::read_wav(mock.output()).unwrap();
    assert_eq!(wav.frame_count(), 128);
    assert!(
        wav.samples
            .as_chunks::<2>()
            .0
            .iter()
            .all(|frame| *frame == [0.25, -0.5])
    );
    assert_eq!(capture.state.opaque_state, state);
    assert_eq!(capture.state.parameter_bases, bases);
    assert_eq!(selected_source_project_snapshot(&project), project_before);
    mock.assert_cooperative(true);
}

#[test]
fn strict_selected_stereo_owner_ack_unknown_or_complete_descriptor_drift_cannot_publish() {
    let _serial = serial();
    for (fault, stage) in [
        ("error", "SetBusActive:9"),
        ("ack_without_participation", "SetBusActive:9"),
        ("audio_aux_unknown", "CheckedAudioBusLayout:3"),
        ("audio_aux_drift", "CheckedAudioBusLayout:3"),
        ("event_unknown", "EventBusLayout:1"),
    ] {
        let mock = Mock::new(fault, stage).with_selected_stereo();
        let (project, capture) = mock.capture();
        let original = selected_source_project_snapshot(&project);
        let state = capture.state.opaque_state.clone();
        let failure = failed(normal_job(&mock).finish());
        assert!(matches!(
            failure.kind,
            FailureKind::Backend | FailureKind::Invalid
        ));
        if fault == "ack_without_participation" {
            assert_typed(&failure, StrictKind::Shape, BackendDelivery::NotSent);
            let backend = failure.backend.as_ref().unwrap();
            assert_eq!(
                backend.strict_stage,
                Some("prepare.select_main_stereo_output")
            );
            assert_eq!(
                backend.last_wire_request, None,
                "successful final layout query is not a failed wire request"
            );
            mock.assert_terminal_at("CheckedAudioBusLayout:3");
        }
        assert!(
            !mock
                .commands()
                .iter()
                .any(|command| command.starts_with("Process:"))
        );
        mock.assert_unchanged();
        mock.assert_reaped();
        assert_eq!(selected_source_project_snapshot(&project), original);
        assert_eq!(capture.state.opaque_state, state);
    }
}

#[test]
fn strict_selected_declared_midi_output_actual_events_or_loss_reject_every_discarded_kept_and_padding_block()
 {
    let _serial = serial();
    for process in 1..=4 {
        for fault in ["output_event", "output_loss"] {
            let stage = format!("Process:{process}");
            let mock = Mock::with_shape(fault, &stage, 2, 257).with_selected_stereo();
            let failure = failed(normal_job(&mock).finish());
            assert!(matches!(
                failure.kind,
                FailureKind::Backend | FailureKind::Invalid
            ));
            assert_eq!(
                mock.commands()
                    .iter()
                    .filter(|command| command.starts_with("Process:"))
                    .count(),
                process
            );
            mock.assert_unchanged();
            mock.assert_reaped();
        }
    }
}

#[test]
fn strict_selected_stereo_cancel_during_sdk_ack_reaps_without_source_or_destination_changes() {
    let _serial = serial();
    let mock = Mock::new("block", "SetBusActive:9").with_selected_stereo();
    let (project, capture) = mock.capture();
    let original = selected_source_project_snapshot(&project);
    let state = capture.state.opaque_state.clone();
    let job = normal_job(&mock);
    mock.wait_stage("blocked", "SetBusActive:9");
    assert!(job.control.export.clone().cancel());
    let failure = failed(job.finish());
    assert_eq!(failure.kind, FailureKind::Cancelled);
    mock.assert_terminal_at("SetBusActive:9");
    assert_eq!(selected_source_project_snapshot(&project), original);
    assert_eq!(capture.state.opaque_state, state);
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

fn assert_exchange_context(
    failure: &RenderFailure,
    owner_stage: &'static str,
    operation: &'static str,
) {
    let backend = failure
        .backend
        .as_ref()
        .expect("typed strict backend evidence");
    assert_eq!(backend.strict_stage, Some(owner_stage), "{failure:?}");
    assert_eq!(
        backend.last_wire_request,
        Some(vst3_host::strict_offline::RequestContext {
            operation,
            media_type: None,
            direction: None,
            bus_index: None,
        }),
        "{failure:?}"
    );
}

fn ordinary_owner_stage(stage: &str) -> (&'static str, &'static str) {
    match stage.split_once(':').unwrap() {
        ("LoadPlugin", _) => ("prepare.load_plugin", "LoadPlugin"),
        ("SetProcessMode", _) => ("prepare.set_offline", "SetProcessMode"),
        ("ResetOriginSupport", _) => ("prepare.reset_origin_support", "ResetOriginSupport"),
        ("LoadState", _) => ("prepare.load_state", "LoadState"),
        ("StartProcessing", _) => ("prepare.start_processing", "StartProcessing"),
        ("NativeDirtyRevision", _) => ("prepare.native_dirty_revision", "NativeDirtyRevision"),
        ("GetAllParameters", _) => ("prepare.parameters", "GetAllParameters"),
        ("SetParameter", _) => ("prepare.set_parameter", "SetParameter"),
        ("GetParameter", _) => ("prepare.parameter_readback", "GetParameter"),
        ("CheckedAudioBusLayout", "1") => (
            "prepare.final_metadata.checked_audio_layout",
            "CheckedAudioBusLayout",
        ),
        ("EventBusLayout", "1") => ("prepare.final_metadata.event_layout", "EventBusLayout"),
        ("LatencySamples", "1") => ("prepare.final_metadata.latency_samples", "LatencySamples"),
        ("TailSamples", "1") => ("prepare.final_metadata.tail_samples", "TailSamples"),
        ("CheckedAudioBusLayout", _) => (
            "render.metadata.checked_audio_layout",
            "CheckedAudioBusLayout",
        ),
        ("EventBusLayout", _) => ("render.metadata.event_layout", "EventBusLayout"),
        ("LatencySamples", _) => ("render.metadata.latency_samples", "LatencySamples"),
        ("TailSamples", _) => ("render.metadata.tail_samples", "TailSamples"),
        ("SendMidiAt", _) => ("render.send_midi", "SendMidiAt"),
        ("Process", _) => ("render.process", "Process"),
        ("StopProcessing", _) => ("dispose.stop_processing", "StopProcessing"),
        other => panic!("missing fixed owner-stage oracle: {other:?}"),
    }
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
        let (frames, remainder) = wav.samples.as_chunks::<2>();
        assert!(remainder.is_empty());
        for frame in frames {
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
        for fault in ["error", "wrong", "malformed", "death", "limit"] {
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
                "limit" => {
                    assert_typed(&failure, StrictKind::Limit, BackendDelivery::MayHaveApplied);
                    assert_eq!(
                        failure.backend.as_ref().unwrap().limit_evidence,
                        None,
                        "generic Error message cap is not a catalog guard"
                    );
                }
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
            let (owner_stage, operation) = ordinary_owner_stage(stage);
            assert_exchange_context(&failure, owner_stage, operation);
            mock.assert_terminal_at(stage);
        }
    }
}

#[test]
fn strict_clone_parameter_catalog_limit_retains_actual_request_and_seals_without_setter() {
    let _serial = serial();
    let mock = Mock::new("catalog_limit", "GetAllParameters:1");
    assert_eq!(mock.capture().1.state.parameter_bases.len(), 2);
    let failure = failed(normal_job(&mock).finish());
    assert_typed(&failure, StrictKind::Limit, BackendDelivery::MayHaveApplied);
    assert_exchange_context(&failure, "prepare.parameters", "GetAllParameters");
    assert_eq!(
        failure.backend.as_ref().unwrap().limit_evidence,
        Some(vst3_host::strict_offline::LimitEvidence {
            site: vst3_host::strict_offline::LimitSite::ParameterCatalogEntries,
            observed_at_least: 4097,
            maximum: 4096,
        })
    );
    mock.assert_terminal_at("GetAllParameters:1");
}

#[test]
fn strict_clone_nonempty_catalog_boundary_replays_bases_after_full_catalog_and_text_limits_reap() {
    let _serial = serial();
    let exact = Mock::new("catalog_max", "GetAllParameters:1");
    let (_, captured) = exact.capture();
    assert_eq!(
        captured
            .state
            .parameter_bases
            .keys()
            .copied()
            .collect::<Vec<_>>(),
        [17, 23]
    );
    assert!(matches!(
        normal_job(&exact).finish(),
        RenderOutcome::Complete(_)
    ));
    let commands = exact.commands();
    let catalog = commands
        .iter()
        .position(|stage| stage == "GetAllParameters:1")
        .unwrap();
    let first_setter = commands
        .iter()
        .position(|stage| stage == "SetParameter:1")
        .unwrap();
    let final_setter = commands
        .iter()
        .position(|stage| stage == "SetParameter:2")
        .unwrap();
    let first_readback = commands
        .iter()
        .position(|stage| stage == "GetParameter:1")
        .unwrap();
    assert!(catalog < first_setter && first_setter < final_setter && final_setter < first_readback);
    assert_eq!(
        commands
            .iter()
            .filter(|stage| stage.starts_with("SetParameter:"))
            .count(),
        2
    );
    assert_eq!(
        commands
            .iter()
            .filter(|stage| stage.starts_with("GetParameter:"))
            .count(),
        2
    );
    exact.assert_cooperative(true);
    for fault in ["catalog_name_limit", "catalog_unit_limit"] {
        let mock = Mock::new(fault, "GetAllParameters:1");
        assert_eq!(mock.capture().1.state.parameter_bases.len(), 2);
        let failure = failed(normal_job(&mock).finish());
        assert_typed(&failure, StrictKind::Limit, BackendDelivery::MayHaveApplied);
        assert_exchange_context(&failure, "prepare.parameters", "GetAllParameters");
        assert_eq!(
            failure.backend.as_ref().unwrap().limit_evidence,
            Some(vst3_host::strict_offline::LimitEvidence {
                site: vst3_host::strict_offline::LimitSite::ParameterTextBytes,
                observed_at_least: 1025,
                maximum: 1024,
            })
        );
        mock.assert_terminal_at("GetAllParameters:1");
    }
}

#[test]
fn strict_clone_exact_empty_base_set_loads_state_and_renders_without_unused_catalog_or_override_requests()
 {
    let _serial = serial();
    for selected in [false, true] {
        for fault in ["catalog_limit", "error"] {
            let mock = Mock::new(fault, "GetAllParameters:1");
            let mock = if selected {
                mock.with_selected_stereo()
            } else {
                mock
            };
            let mock = mock.with_empty_bases();
            let (project, capture) = mock.capture();
            let project_before = selected_source_project_snapshot(&project);
            let state_before = capture.state.clone();
            assert!(project.plugin_instances[0].parameters.is_empty());
            assert!(capture.state.parameter_bases.is_empty());
            let (_, requested_ids) = capture.strict_load_inputs();
            assert!(requested_ids.is_empty());
            capture
                .state
                .validate(requested_ids, capture.plan().sample_rate())
                .unwrap();
            let RenderOutcome::Complete(report) = normal_job(&mock).finish() else {
                panic!("exact empty requested base set must not depend on an unused catalog");
            };
            let commands = mock.commands();
            assert!(commands.contains(&"LoadState:1".to_owned()));
            assert!(commands.contains(&"StartProcessing:1".to_owned()));
            assert!(commands.contains(&"NativeDirtyRevision:1".to_owned()));
            assert!(commands.contains(&if selected {
                "StrictMetadataSnapshot:1".to_owned()
            } else {
                "EventBusLayout:1".to_owned()
            }));
            for getter in ["EventBusLayout", "LatencySamples", "TailSamples"] {
                assert_eq!(
                    commands.contains(&format!("{getter}:1")),
                    !selected,
                    "selected metadata travels in the complete bundle; ordinary metadata remains separate"
                );
            }
            assert!(commands.contains(&"Process:1".to_owned()));
            assert!(commands.contains(&"StopProcessing:1".to_owned()));
            assert!(
                !commands
                    .iter()
                    .any(|stage| stage.starts_with("GetAllParameters:")
                        || stage.starts_with("SetParameter:")
                        || stage.starts_with("GetParameter:"))
            );
            let ledger = mock.ledger();
            if selected {
                let snapshots: Vec<_> = commands
                    .iter()
                    .filter(|stage| stage.starts_with("StrictMetadataSnapshot:"))
                    .collect();
                // Preparation and pre/post metadata for both actual Q128
                // blocks still invoke all four SDK getters in their order.
                assert_eq!(
                    snapshots,
                    (1..=5)
                        .map(|index| format!("StrictMetadataSnapshot:{index}"))
                        .collect::<Vec<_>>()
                        .iter()
                        .collect::<Vec<_>>()
                );
                for stage in snapshots {
                    let getters: Vec<_> = ledger
                        .iter()
                        .filter(|row| {
                            row["kind"] == "metadata_getter"
                                && row["wire_stage"].as_str() == Some(stage.as_str())
                        })
                        .map(|row| row["getter"].as_str().unwrap())
                        .collect();
                    assert_eq!(
                        getters,
                        [
                            "CheckedAudioBusLayout",
                            "EventBusLayout",
                            "LatencySamples",
                            "TailSamples"
                        ]
                    );
                }
            }
            let loads: Vec<_> = ledger
                .iter()
                .filter_map(|entry| entry["command"].get("LoadState"))
                .collect();
            assert_eq!(loads.len(), 1);
            assert_eq!(loads[0], &json!({"data": "AQID", "context": "Project"}));
            assert_eq!(
                ledger
                    .iter()
                    .filter(|entry| entry["kind"] == "audio_ack")
                    .count(),
                if selected { 32 } else { 0 }
            );
            assert_eq!(report.identity, capture.identity());
            assert_eq!(report.native_latency, mock.latency);
            assert_eq!(report.frames, 128);
            let wav = crate::wav::read_wav(mock.output()).unwrap();
            assert_eq!(wav.frame_count(), 128);
            assert!(
                wav.samples
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .all(|frame| *frame == [0.25, -0.5])
            );
            mock.assert_cooperative(true);
            assert!(mock.audit().iter().any(concrete_child_reap_evidence));
            assert_eq!(capture.state, state_before);
            assert_eq!(selected_source_project_snapshot(&project), project_before);
        }
    }
}

#[test]
fn strict_clone_healthy_delayed_natural_exit_reaps_successfully_before_wav_publication() {
    let _serial = serial();
    let mock = Mock::new("delay_exit", "")
        .with_selected_stereo()
        .with_empty_bases();
    let (project, capture) = mock.capture();
    let source_before = selected_source_project_snapshot(&project);
    let state_before = capture.state.clone();
    let RenderOutcome::Complete(report) = normal_job(&mock).finish() else {
        panic!("healthy clone must be allowed its bounded natural teardown before forced kill");
    };
    assert_eq!(report.frames, 128);
    let ledger = mock.ledger();
    assert!(
        ledger
            .iter()
            .any(|entry| entry["kind"] == "shutdown_received_before_natural_delay")
    );
    assert!(
        ledger
            .iter()
            .any(|entry| entry["kind"] == "natural_exit_ready")
    );
    mock.assert_cooperative(true);
    assert!(
        mock.audit()
            .iter()
            .any(|entry| concrete_child_reap_evidence(entry)
                && entry["error"].is_null()
                && entry["diagnostics"].as_array().is_some_and(Vec::is_empty))
    );
    assert_eq!(
        mock.commands()
            .iter()
            .filter(|stage| stage.starts_with("Shutdown:"))
            .count(),
        1
    );
    let wav = crate::wav::read_wav(mock.output()).unwrap();
    assert_eq!(wav.frame_count(), 128);
    assert!(
        wav.samples
            .as_chunks::<2>()
            .0
            .iter()
            .all(|frame| *frame == [0.25, -0.5])
    );
    assert_eq!(capture.state, state_before);
    assert_eq!(selected_source_project_snapshot(&project), source_before);
}

#[test]
fn strict_clone_empty_opaque_state_or_inexact_requested_base_set_rejects_before_spawn() {
    let _serial = serial();
    for defect in [
        "empty_opaque",
        "missing_requested",
        "partial_requested",
        "unexpected_base",
    ] {
        let mock = Mock::new("", "");
        let mock = if matches!(defect, "missing_requested" | "partial_requested") {
            mock
        } else {
            mock.with_empty_bases()
        };
        let (project, mut capture) = mock.capture();
        match defect {
            "empty_opaque" => capture.state.opaque_state.clear(),
            "missing_requested" => capture.state.parameter_bases.clear(),
            "partial_requested" => {
                capture.state.parameter_bases.remove(&23);
            }
            "unexpected_base" => {
                capture.state.parameter_bases.insert(17, 0.1);
            }
            _ => unreachable!(),
        }
        let project_before = selected_source_project_snapshot(&project);
        let state_before = capture.state.clone();
        let mut owner = DirectOwner::new();
        let failure = owner
            .backend
            .prepare(&capture, &control(Duration::from_secs(20)))
            .unwrap_err();
        assert_eq!(failure.kind, BackendFailureKind::Backend);
        assert_eq!(failure.strict_kind, None);
        let (spawns, reap) = owner.backend.test_child_completion();
        assert_eq!(spawns, 0);
        assert!(reap.is_none());
        assert!(
            owner
                .dispose(Instant::now() + Duration::from_secs(5))
                .terminated
        );
        assert!(mock.ledger().is_empty());
        mock.assert_unchanged();
        assert_eq!(capture.state, state_before);
        assert_eq!(selected_source_project_snapshot(&project), project_before);
    }
}

#[test]
fn strict_selected_preparation_limit_names_implicit_exchange_and_sdk_bus_index() {
    let _serial = serial();
    for (stage, owner_stage, operation) in [
        (
            "CheckedAudioBusLayout:1",
            "prepare.post_state_audio_layout",
            "CheckedAudioBusLayout",
        ),
        (
            "ResetOriginSupport:2",
            "prepare.select_main_stereo_output",
            "ResetOriginSupport",
        ),
        (
            "CheckedAudioBusLayout:2",
            "prepare.select_main_stereo_output",
            "CheckedAudioBusLayout",
        ),
        (
            "CheckedAudioBusLayout:3",
            "prepare.select_main_stereo_output",
            "CheckedAudioBusLayout",
        ),
        (
            "CheckedAudioBusLayout:4",
            "prepare.start_processing",
            "CheckedAudioBusLayout",
        ),
        (
            "StartProcessing:1",
            "prepare.start_processing",
            "StartProcessing",
        ),
        (
            "CheckedAudioBusLayout:5",
            "prepare.start_processing",
            "CheckedAudioBusLayout",
        ),
        (
            "CheckedAudioBusLayout:6",
            "prepare.final_metadata.bundle",
            "StrictMetadataSnapshot",
        ),
        (
            "EventBusLayout:1",
            "prepare.final_metadata.bundle",
            "StrictMetadataSnapshot",
        ),
        (
            "LatencySamples:1",
            "prepare.final_metadata.bundle",
            "StrictMetadataSnapshot",
        ),
        (
            "TailSamples:1",
            "prepare.final_metadata.bundle",
            "StrictMetadataSnapshot",
        ),
    ] {
        let mock = Mock::new("limit", stage).with_selected_stereo();
        let failure = failed(normal_job(&mock).finish());
        assert_typed(&failure, StrictKind::Limit, BackendDelivery::MayHaveApplied);
        assert_exchange_context(&failure, owner_stage, operation);
        mock.assert_terminal_at(if operation == "StrictMetadataSnapshot" {
            "StrictMetadataSnapshot:1"
        } else {
            stage
        });
        assert_eq!(
            failure.backend.as_ref().unwrap().strict_metadata_getter,
            None
        );
    }
    let mock = Mock::new("limit", "SetBusActive:9").with_selected_stereo();
    let failure = failed(normal_job(&mock).finish());
    assert_typed(&failure, StrictKind::Limit, BackendDelivery::MayHaveApplied);
    let backend = failure.backend.as_ref().unwrap();
    assert_eq!(
        backend.strict_stage,
        Some("prepare.select_main_stereo_output")
    );
    assert_eq!(
        backend.last_wire_request,
        Some(vst3_host::strict_offline::RequestContext {
            operation: "SetBusActive",
            media_type: Some(vst3_host::audio::MediaType::Audio),
            direction: Some(vst3_host::audio::BusDirection::Output),
            bus_index: Some(8),
        })
    );
    mock.assert_terminal_at("SetBusActive:9");
}

#[test]
fn strict_failure_context_preserves_utf8_diagnostic_and_all_typed_evidence() {
    use vst3_host::strict_offline as strict;
    let diagnostic = "界".repeat(4000);
    let request = strict::RequestContext {
        operation: "GetAllParameters",
        media_type: None,
        direction: None,
        bus_index: None,
    };
    for kind in [
        StrictKind::Limit,
        StrictKind::Cancelled,
        StrictKind::Deadline,
    ] {
        let failure = BackendFailure::from(strict::Failure {
            kind,
            delivery: strict::Delivery::MayHaveApplied,
            diagnostic: Some(diagnostic.clone()),
            diagnostic_unavailable: Some(StrictKind::Allocation),
            request_context: Some(request),
            metadata_getter: None,
            limit_evidence: (kind == StrictKind::Limit).then_some(strict::LimitEvidence {
                site: strict::LimitSite::ParameterTextBytes,
                observed_at_least: 1025,
                maximum: 1024,
            }),
        });
        let original = failure.clone();
        let contextual = failure.with_strict_context("prepare.parameters");
        assert_eq!(contextual.kind, original.kind);
        assert_eq!(contextual.delivery, original.delivery);
        assert_eq!(contextual.strict_kind, original.strict_kind);
        assert_eq!(
            contextual.strict_diagnostic_unavailable,
            original.strict_diagnostic_unavailable
        );
        assert_eq!(contextual.diagnostic(), original.diagnostic());
        assert_eq!(contextual.to_string(), original.to_string());
        assert_eq!(contextual.diagnostic(), "界".repeat(2048 / 3));
        assert_eq!(contextual.last_wire_request, Some(request));
        assert_eq!(contextual.limit_evidence, original.limit_evidence);
        assert_eq!(contextual.strict_stage, Some("prepare.parameters"));
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
        assert_eq!(failure.backend.as_ref().unwrap().strict_stage, None);
        assert_eq!(failure.backend.as_ref().unwrap().last_wire_request, None);
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
        if matches!(hook, Hook::CancelAfterSpawn) {
            mock.assert_pre_dispatch_cancel_reaped();
        } else {
            mock.assert_reaped();
        }
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
fn strict_clone_pre_dispatch_reap_evidence_rejects_no_spawn_pending_and_kill_only() {
    // Synthetic audit records qualify the evidence gate. The next test
    // separately exercises an actual OS child with deliberately delayed startup.
    let complete = json!({
        "kind": "disposal", "terminated": true, "destination_unchanged": true,
        "client_spawn_successes": 1, "child_reaped": true,
        "child_reap_status": "synthetic completed status",
    });
    assert!(concrete_child_reap_evidence(&complete));
    let mut no_spawn = complete.clone();
    no_spawn["client_spawn_successes"] = json!(0);
    let mut pending = complete.clone();
    pending["terminated"] = json!(false);
    pending["child_reaped"] = json!(false);
    pending["child_reap_status"] = Value::Null;
    let mut kill_only = complete.clone();
    kill_only["child_reaped"] = json!(false);
    kill_only["child_reap_status"] = Value::Null;
    kill_only["diagnostics"] = json!(["Strict clone required forced termination"]);
    let mut missing_status = complete.clone();
    missing_status["child_reap_status"] = Value::Null;
    let mut replayed = complete.clone();
    replayed["client_spawn_successes"] = json!(2);
    for incomplete in [no_spawn, pending, kill_only, missing_status, replayed] {
        assert!(
            !concrete_child_reap_evidence(&incomplete),
            "incomplete or replaced OS child evidence must be rejected: {incomplete:?}"
        );
    }
}

#[test]
fn strict_clone_cancel_before_mock_startup_reaps_real_child_without_dispatch_or_ledger() {
    let _serial = serial();
    let mock = Mock::new("delay_start", "");
    // The generic Job guard releases protocol blocks with `release`, never the
    // dedicated preledger marker. This case must be reaped before startup logs.
    let failure =
        failed(Job::start(&mock, Hook::CancelAfterSpawn, Duration::from_secs(20)).finish());
    assert_typed(&failure, StrictKind::Cancelled, BackendDelivery::NotSent);
    assert!(!mock.root.join("ledger.jsonl").exists());
    assert!(mock.commands().is_empty());
    mock.assert_unchanged();
    mock.assert_pre_dispatch_cancel_reaped();
    assert!(
        mock.audit().iter().any(|entry| {
            entry["diagnostics"].as_array().is_some_and(|diagnostics| {
                diagnostics.iter().any(|message| {
                    message.as_str() == Some("Strict clone required forced termination")
                })
            })
        }),
        "delayed startup must prove actual force-kill followed by concrete reap"
    );
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
            assert_eq!(
                failure.backend.as_deref().unwrap().strict_stage,
                Some("dispose.wait_for_exit"),
            );
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
        let backend = StrictBackend::new();
        #[cfg(windows)]
        let backend = backend.with_python_mock();
        Self {
            backend,
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

#[test]
fn strict_clone_dry_topology_preserves_descriptors_and_sends_exact_positive_zero_inputs() {
    let _serial = serial();
    let mut inputs = vec![None];
    let mut extra_bus_options = Vec::new();
    for channels in [1, 2] {
        for is_main in [false, true] {
            extra_bus_options.push((channels, is_main));
            for active in [false, true] {
                inputs.push(Some((channels, is_main, active)));
            }
        }
    }
    let mut extra_output_layouts = vec![vec![]];
    for &first in &extra_bus_options {
        extra_output_layouts.push(vec![first]);
        for &second in &extra_bus_options {
            extra_output_layouts.push(vec![first, second]);
        }
    }
    assert_eq!(inputs.len(), 9);
    assert_eq!(extra_output_layouts.len(), 21);
    let mut observed_shape_cases = 0;
    for input in inputs {
        for channels in [1, 2] {
            for extra_outputs in &extra_output_layouts {
                let mock = Mock::with_shape("", "", channels, 0)
                    .with_topology(input, extra_outputs.clone());
                let (_, capture) = mock.capture();
                let expected_inputs: Vec<_> = input
                    .into_iter()
                    .map(|(channels, is_main, active)| {
                        BounceAudioBus::new(channels, is_main, Some(active))
                    })
                    .collect();
                let expected_outputs: Vec<_> =
                    std::iter::once(BounceAudioBus::new(channels, true, Some(true)))
                        .chain(extra_outputs.iter().map(|&(channels, is_main)| {
                            BounceAudioBus::new(channels, is_main, Some(false))
                        }))
                        .collect();
                assert_eq!(capture.metadata().input_buses, expected_inputs);
                assert_eq!(capture.metadata().output_buses, expected_outputs);
                // The observed Aux stereo input and all-Main stereo outputs must
                // appear as exactly this bounded, known-activation fixture.
                if input == Some((2, false, true))
                    && channels == 2
                    && extra_outputs.as_slice() == [(2, true), (2, true)]
                {
                    observed_shape_cases += 1;
                    assert_eq!(
                        capture.metadata().input_buses,
                        vec![BounceAudioBus::new(2, false, Some(true))]
                    );
                    assert_eq!(
                        capture.metadata().output_buses,
                        vec![
                            BounceAudioBus::new(2, true, Some(true)),
                            BounceAudioBus::new(2, true, Some(false)),
                            BounceAudioBus::new(2, true, Some(false)),
                        ]
                    );
                }
                let RenderOutcome::Complete(_) = normal_job(&mock).finish() else {
                    panic!(
                        "dry topology mock failed: input={input:?}, channels={channels}, extras={extra_outputs:?}"
                    )
                };
                let expected_layout = json!({
                    "inputs": expected_inputs.iter().map(|bus| json!({
                        "channel_count": bus.channel_count,
                        "is_main": bus.is_main,
                        "active": bus.active,
                    })).collect::<Vec<_>>(),
                    "outputs": expected_outputs.iter().map(|bus| json!({
                        "channel_count": bus.channel_count,
                        "is_main": bus.is_main,
                        "active": bus.active,
                    })).collect::<Vec<_>>(),
                });
                let ledger = mock.ledger();
                let layouts: Vec<_> = ledger
                    .iter()
                    .filter(|entry| entry["kind"] == "audio_topology")
                    .collect();
                assert_eq!(layouts.len(), 5);
                for (index, entry) in layouts.into_iter().enumerate() {
                    assert_eq!(
                        entry["stage"],
                        format!("CheckedAudioBusLayout:{}", index + 1)
                    );
                    assert_eq!(entry["layout"], expected_layout);
                }
                let expected_input_channels = input
                    .filter(|(_, _, active)| *active)
                    .map_or(0, |(channels, _, _)| channels);
                let zeros: Vec<_> = ledger
                    .iter()
                    .filter(|entry| entry["kind"] == "silent_inputs")
                    .collect();
                assert_eq!(
                    zeros.len(),
                    2,
                    "every musical Process checks all wire samples"
                );
                for entry in zeros {
                    assert_eq!(entry["channels"], expected_input_channels);
                    assert_eq!(entry["frames"], 128);
                    assert_eq!(entry["bits"], "positive_zero");
                }
                let wav = crate::wav::read_wav(mock.output()).unwrap();
                assert_eq!(wav.frame_count(), 128);
                let (frames, remainder) = wav.samples.as_chunks::<2>();
                assert!(remainder.is_empty());
                for frame in frames {
                    assert_eq!(frame, &[0.25, if channels == 1 { 0.25 } else { -0.5 }]);
                }
                assert!(
                    !mock
                        .commands()
                        .iter()
                        .any(|name| name.starts_with("SetBusActive"))
                );
                mock.assert_cooperative(true);
            }
        }
    }
    assert_eq!(observed_shape_cases, 1);
}

#[test]
fn strict_clone_full_dry_topology_drift_refuses_before_and_after_each_process() {
    let _serial = serial();
    for occurrence in 1..=5 {
        for (input_main, extra_main) in [(true, false), (false, false), (true, true), (false, true)]
        {
            for fault in [
                "audio_input_unknown",
                "audio_input_aux",
                "audio_input_main",
                "audio_extra_input",
                "audio_input_channels",
                "audio_input_activation_drift",
                "audio_active_aux",
                "audio_aux_unknown",
                "audio_aux_main",
                "audio_aux_aux",
                "audio_aux_channels",
                "audio_aux_drift",
                "audio_aux_order",
                "audio_aux_removed",
                "audio_extra_aux",
                "audio_aux",
                "audio_unknown",
                "audio_inactive",
                "audio_channels",
                "audio_output_missing",
            ] {
                if matches!(fault, "audio_input_aux") && !input_main
                    || matches!(fault, "audio_input_main") && input_main
                    || matches!(fault, "audio_aux_main") && extra_main
                    || matches!(fault, "audio_aux_aux") && !extra_main
                {
                    continue;
                }
                let stage = format!("CheckedAudioBusLayout:{occurrence}");
                let mock = Mock::new(fault, &stage).with_topology(
                    Some((2, input_main, true)),
                    vec![(1, extra_main), (2, extra_main)],
                );
                let failure = failed(normal_job(&mock).finish());
                let supported_drift = matches!(
                    fault,
                    "audio_input_aux"
                        | "audio_input_main"
                        | "audio_aux_main"
                        | "audio_aux_aux"
                        | "audio_input_activation_drift"
                        | "audio_aux_drift"
                        | "audio_aux_order"
                        | "audio_aux_removed"
                );
                let (expected_kind, expected_message) = if supported_drift {
                    match occurrence {
                        1 => (
                            FailureKind::Backend,
                            "Fresh strict clone metadata differs from authoritative capture",
                        ),
                        2 | 4 => (
                            FailureKind::Invalid,
                            "Instrument metadata changed before Process",
                        ),
                        3 | 5 => (
                            FailureKind::Invalid,
                            "Instrument metadata changed after Process",
                        ),
                        _ => unreachable!(),
                    }
                } else if matches!(
                    fault,
                    "audio_input_unknown" | "audio_aux_unknown" | "audio_unknown"
                ) {
                    (
                        FailureKind::Backend,
                        "Instrument bounce activation is unknown; a fresh load with explicit inactive-bus acknowledgment is required",
                    )
                } else {
                    (
                        FailureKind::Backend,
                        "Instrument bounce requires at most one known mono/stereo input for explicit silence, one known-active main mono/stereo output at index zero, and at most two known-inactive mono/stereo extra outputs",
                    )
                };
                assert_eq!(
                    failure.kind, expected_kind,
                    "{fault} at {stage}: {failure:?}"
                );
                assert_eq!(failure.primary, expected_message, "{fault} at {stage}");
                if expected_kind == FailureKind::Backend {
                    let backend = failure
                        .backend
                        .as_ref()
                        .expect("exact local admission failure");
                    assert_eq!(backend.kind, BackendFailureKind::Backend);
                    assert_eq!(backend.diagnostic(), expected_message);
                    assert_eq!(
                        backend.strict_kind, None,
                        "not a transport or decoder failure"
                    );
                    assert_eq!(backend.delivery, None);
                } else {
                    assert!(
                        failure.backend.is_none(),
                        "renderer equality fence owns supported drift"
                    );
                }
                assert!(failure.cleanup.is_empty(), "{fault}: {failure:?}");
                assert!(failure.cleanup_backend.is_none());
                assert!(!failure.cleanup_grace_elapsed);
                let ledger = mock.ledger();
                let injections: Vec<_> = ledger
                    .iter()
                    .filter(|entry| entry["kind"] == "injected_audio_topology")
                    .collect();
                assert_eq!(
                    injections.len(),
                    1,
                    "the selected mutation must actually occur"
                );
                assert_eq!(injections[0]["stage"], stage);
                assert_eq!(injections[0]["fault"], fault);
                assert!(
                    ledger
                        .iter()
                        .any(|entry| entry["kind"] == "reply" && entry["stage"] == stage)
                );
                let commands = mock.commands();
                assert_eq!(
                    commands
                        .iter()
                        .filter(|name| name.starts_with("CheckedAudioBusLayout:"))
                        .count(),
                    occurrence
                );
                let injected_index = commands
                    .iter()
                    .position(|name| name == &stage)
                    .expect("injected request was dispatched");
                let mut exact_suffix = vec![stage];
                if supported_drift {
                    // Supported shapes reach complete metadata equality; policy failures stop
                    // at the raw audio policy before any later metadata request.
                    exact_suffix.extend([
                        format!("EventBusLayout:{occurrence}"),
                        format!("LatencySamples:{occurrence}"),
                        format!("TailSamples:{occurrence}"),
                    ]);
                }
                exact_suffix.extend(["StopProcessing:1".into(), "Shutdown:1".into()]);
                assert_eq!(
                    &commands[injected_index..],
                    exact_suffix.as_slice(),
                    "exact refusal and cleanup stage: {fault}"
                );
                assert_eq!(
                    commands
                        .iter()
                        .filter(|name| name.starts_with("Process:"))
                        .count(),
                    (occurrence - 1) / 2
                );
                mock.assert_unchanged();
                mock.assert_cooperative(true);
            }
        }
    }
}

#[test]
fn strict_selected_bundle_actual_getter_failures_keep_wire_and_inner_identity() {
    let _serial = serial();
    for (stage, getter, label) in [
        (
            "CheckedAudioBusLayout:6",
            strict::MetadataGetter::CheckedAudioBusLayout,
            "prepare.final_metadata.checked_audio_layout",
        ),
        (
            "EventBusLayout:1",
            strict::MetadataGetter::EventBusLayout,
            "prepare.final_metadata.event_layout",
        ),
        (
            "LatencySamples:1",
            strict::MetadataGetter::LatencySamples,
            "prepare.final_metadata.latency_samples",
        ),
        (
            "TailSamples:1",
            strict::MetadataGetter::TailSamples,
            "prepare.final_metadata.tail_samples",
        ),
    ] {
        let mock = Mock::new("error", stage).with_selected_stereo();
        let (project, capture) = mock.capture();
        let original = selected_source_project_snapshot(&project);
        let state = capture.state.clone();
        let failure = failed(normal_job(&mock).finish());
        assert_typed(
            &failure,
            StrictKind::OperationRejected,
            BackendDelivery::MayHaveApplied,
        );
        assert_exchange_context(&failure, label, "StrictMetadataSnapshot");
        assert_eq!(
            failure.backend.as_ref().unwrap().strict_metadata_getter,
            Some(getter)
        );
        mock.assert_terminal_at("StrictMetadataSnapshot:1");
        assert!(
            !mock
                .commands()
                .iter()
                .any(|row| row.starts_with("Process:"))
        );
        assert_eq!(selected_source_project_snapshot(&project), original);
        assert_eq!(capture.state, state);
    }
}

#[test]
fn strict_selected_bundle_actual_event_admission_calls_no_latency_or_tail() {
    let _serial = serial();
    let mock = Mock::new("event_unknown", "EventBusLayout:1").with_selected_stereo();
    let (project, capture) = mock.capture();
    let original = selected_source_project_snapshot(&project);
    let state = capture.state.clone();
    let failure = failed(normal_job(&mock).finish());
    assert_eq!(failure.kind, FailureKind::Backend);
    assert!(failure.primary.contains("known-active main event input"));
    let ledger = mock.ledger();
    assert!(ledger.iter().any(|row| row["kind"] == "metadata_getter"
        && row["getter"] == "EventBusLayout"
        && row["wire_stage"] == "StrictMetadataSnapshot:1"));
    assert!(!ledger.iter().any(|row| row["kind"] == "metadata_getter"
        && (row["getter"] == "LatencySamples" || row["getter"] == "TailSamples")));
    assert!(
        !mock
            .commands()
            .iter()
            .any(|row| row.starts_with("Process:"))
    );
    mock.assert_unchanged();
    mock.assert_reaped();
    assert_eq!(selected_source_project_snapshot(&project), original);
    assert_eq!(capture.state, state);
}

#[test]
fn strict_selected_bundle_actual_held_getter_completes_after_cancel_or_deadline_without_publish() {
    let _serial = serial();
    for cancel in [true, false] {
        let mock = Mock::new("late_after_eof", "LatencySamples:1").with_selected_stereo();
        let (project, capture) = mock.capture();
        let original = selected_source_project_snapshot(&project);
        let state = capture.state.clone();
        let job = Job::start(
            &mock,
            Hook::None,
            if cancel {
                Duration::from_secs(20)
            } else {
                Duration::from_secs(2)
            },
        );
        mock.wait_stage("blocked", "LatencySamples:1");
        assert!(job.admission.is_busy());
        if cancel {
            assert!(job.control.export.clone().cancel());
        }
        let failure = failed(job.finish());
        if !cancel {
            assert!(Instant::now() >= job.control.deadline);
        }
        assert_typed(
            &failure,
            if cancel {
                StrictKind::Cancelled
            } else {
                StrictKind::Deadline
            },
            BackendDelivery::MayHaveApplied,
        );
        assert_exchange_context(
            &failure,
            "prepare.final_metadata.bundle",
            "StrictMetadataSnapshot",
        );
        assert_eq!(
            failure.backend.as_ref().unwrap().strict_metadata_getter,
            None
        );
        let ledger = mock.ledger();
        let eof = ledger
            .iter()
            .position(|row| row["kind"] == "late_getter_terminal_eof")
            .unwrap();
        let held_done = ledger
            .iter()
            .position(|row| row["kind"] == "late_getter_completed")
            .unwrap();
        let tail = ledger
            .iter()
            .position(|row| row["kind"] == "metadata_getter" && row["getter"] == "TailSamples")
            .unwrap();
        let complete = ledger
            .iter()
            .position(|row| row["kind"] == "late_complete_snapshot")
            .unwrap();
        assert!(eof < held_done && held_done < tail && tail < complete);
        let snapshot = &ledger[complete]["snapshot"]["StrictMetadataSnapshot"]["snapshot"];
        assert!(snapshot["audio"].is_object() && snapshot["events"].is_object());
        assert_eq!(snapshot["latency_samples"], mock.latency);
        assert_eq!(snapshot["tail_samples"], u32::MAX);
        let write_outcomes: Vec<_> = ledger
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                row["kind"] == "late_snapshot_write_failed"
                    || row["kind"] == "late_snapshot_written"
            })
            .collect();
        assert_eq!(write_outcomes.len(), 1);
        let (write_position, write_outcome) = write_outcomes[0];
        assert_eq!(write_outcome["stage"], "StrictMetadataSnapshot:1");
        if write_outcome["kind"] == "late_snapshot_write_failed" {
            assert!(!write_outcome["error_type"].as_str().unwrap().is_empty());
            assert!(write_outcome["errno"].as_i64().is_some());
            assert!(!write_outcome["message"].as_str().unwrap().is_empty());
        }
        let exited = ledger.iter().position(|row| row["kind"] == "exit").unwrap();
        assert!(complete < write_position && write_position < exited);
        assert!(mock.audit().iter().any(concrete_child_reap_evidence));
        assert!(
            !mock
                .commands()
                .iter()
                .any(|row| row.starts_with("Process:") || row.starts_with("SendMidiAt:"))
        );
        mock.assert_terminal_at("StrictMetadataSnapshot:1");
        assert_eq!(selected_source_project_snapshot(&project), original);
        assert_eq!(capture.state, state);
    }
}
