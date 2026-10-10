#!/usr/bin/env python3
"""Compile external contracts for the OFF-BY-DEFAULT experimental scoped worker.

Python 3.11+, Linux and a reviewed/archived source checkpoint are required. This
runner never executes a Rust fixture, loads a plug-in, or starts a helper. Cargo
check compiles the real vendor dependency; it never source-copies a substitute API
or changes production visibility. Dependency build scripts may run normally.

Supply an approved standalone --manifest and its exact pre-generated --lockfile.
The manifest format is intentionally narrow (the vendor path must be absolute):

    [package]
    name = "vst3-scoped-worker-contracts"
    version = "0.0.0"
    edition = "2021"
    publish = false
    build = false
    [lib]
    path = "active_fixture.rs"
    [features]
    default = []
    experimental-scoped-worker = ["vst3-host/experimental-scoped-worker"]
    [dependencies.vst3-host]
    path = "/absolute/reviewed/repo/vendor/vst3-host-0.9.0"
    default-features = false
    features = ["cpal-backend", "process-isolation"]
    [workspace]

Use --target-coordinated to attest that the parent has reserved --target-dir
against other build workers. Advisory locks prevent concurrent instances of this
runner, but cannot reserve the target against unrelated commands. No unknown
compiler flags, wrappers, target overrides or Cargo config includes are accepted.
A 2 GiB free-space floor is enforced before every command. Inputs, source and
configuration snapshots, metadata feature evidence, raw output, interruptions
and all run histories are retained in --qa-dir. Named reruns always start with
both positive controls. An expected negative counts only when each error has an
intended code/message and a primary span on its marked contract expression.
"""

import argparse
from contextlib import ExitStack, contextmanager
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import time
import tomllib


FEATURE = 'experimental-scoped-worker'
PACKAGE = 'vst3-scoped-worker-contracts'
TARGET_NAME = PACKAGE.replace('-', '_')
BASE_FEATURES = {'cpal-backend', 'process-isolation'}
DISK_FLOOR = 2 * 1024 ** 3
POSITIVE_CASES = {'positive_feature_on_public_session', 'positive_feature_off_owner'}

OWNER_COMMON = '''#![allow(dead_code, unused_imports, unused_variables)]
#![deny(unsafe_code)]
use vst3_host::{MainThreadPlugin, AudioBuffers, AudioBusConfig, AudioBusLayout,
    BusAudioBuffers, MidiChannel, ProcessTransport, Result};

fn transport() -> ProcessTransport {
    ProcessTransport {
        sample_position: 0, quarter_note_position: 0.0, tempo: 120.0,
        playing: false, time_sig_numerator: 4, time_sig_denominator: 4,
    }
}

// Never called. These real public operations must resolve in both feature modes.
fn owner_control(owner: &mut MainThreadPlugin) -> Result<()> {
    let _ = owner.info();
    let state = owner.save_state()?;
    owner.load_state(&state)?;
    owner.stop_processing()?;
    owner.reconfigure(48_000.0, 128)?;
    owner.start_processing()?;
    let _ = owner.reset_origin_support()?;
    let _ = owner.process_reset_origin(128, transport())?;
    Ok(())
}
'''

FEATURE_COMMON = OWNER_COMMON + '''
use vst3_host::scoped_worker::{AttachedUiBridge, AttachedUiRequest, AttachedUiReply,
    WorkerIo, WorkerCommand, WorkerReply, OrdinaryMidiEvent, ScopedWorkerResult};
'''

POSITIVE_SESSION = '''
// Exhaustive matching is intentional: every admitted operation must remain explicit.
fn command_shape(command: WorkerCommand) {
    match command {
        WorkerCommand::Process { buffers, transport } => {},
        WorkerCommand::ProcessBuses { buffers, transport } => {},
        WorkerCommand::SendMidi { event, sample_offset } => match event {
            OrdinaryMidiEvent::NoteOn { channel, note, velocity } => {},
            OrdinaryMidiEvent::NoteOff { channel, note, velocity } => {},
            OrdinaryMidiEvent::PolyAftertouch { channel, note, pressure } => {},
        },
        WorkerCommand::TakeOutputEvents => {},
    }
}
fn ui_shape(request: AttachedUiRequest, reply: AttachedUiReply) {
    match request {
        AttachedUiRequest::AttachedSize | AttachedUiRequest::CanResize |
        AttachedUiRequest::TakeResizeRequest | AttachedUiRequest::ServiceRunLoop => {},
        AttachedUiRequest::Resize { width, height } => {},
        AttachedUiRequest::SetScale { factor } => {},
    }
    match reply {
        AttachedUiReply::AttachedSize(size) | AttachedUiReply::Resized(size) => {},
        AttachedUiReply::CanResize(value) | AttachedUiReply::ScaleAccepted(value) => {},
        AttachedUiReply::ResizeRequest(size) => {},
        AttachedUiReply::RunLoopServiced => {},
    }
}
fn reply_shape(reply: WorkerReply) {
    match reply {
        WorkerReply::Processed { buffers, result } => {},
        WorkerReply::ProcessedBuses { buffers, result } => {},
        WorkerReply::MidiAdmission { result } => {},
        WorkerReply::OutputEvents { events, lost } => {},
    }
}
fn public_session(owner: &mut MainThreadPlugin) -> ScopedWorkerResult {
    fn require_send<T: Send>() {}
    require_send::<WorkerIo<'_>>();
    require_send::<&mut WorkerIo<'_>>();
    require_send::<WorkerCommand>();
    require_send::<WorkerReply>();
    let mut owned_reply: Option<WorkerReply> = None;
    owner.with_scoped_worker(&mut |ui, io| {
        std::thread::scope(|scope| {
            // A caller-owned scope INSIDE the callback can borrow the producer port.
            // The library-owned processing worker is neither returned nor extracted.
            let producer = scope.spawn(move || {
                let audio = AudioBuffers::new(2, 2, 128, 48_000.0);
                io.try_submit(WorkerCommand::Process {
                    buffers: audio, transport: Some(transport()),
                }).expect("compile-only submit");
                let _ = io.has_pending();
                let _ = io.is_closed();
                let _ = io.is_terminal();
                let first = match io.try_reply().expect("compile-only poll") {
                    Some(reply) => reply,
                    None => io.recv_reply().expect("compile-only receive"),
                };
                reply_shape(first);
                let layout = AudioBusLayout {
                    inputs: vec![AudioBusConfig { channel_count: 2, active: true }],
                    outputs: vec![AudioBusConfig { channel_count: 2, active: true }],
                };
                io.try_submit(WorkerCommand::ProcessBuses {
                    buffers: BusAudioBuffers::new(&layout, 128, 48_000.0), transport: None,
                }).expect("compile-only buses");
                reply_shape(io.recv_reply().expect("compile-only buses reply"));
                io.try_submit(WorkerCommand::SendMidi {
                    event: OrdinaryMidiEvent::NoteOn {
                        channel: MidiChannel::Ch1, note: 60, velocity: 100,
                    }, sample_offset: 0,
                }).expect("compile-only MIDI");
                reply_shape(io.recv_reply().expect("compile-only MIDI reply"));
                io.try_submit(WorkerCommand::TakeOutputEvents).expect("compile-only drain");
                io.recv_reply().expect("compile-only owned reply")
            });
            // All editor authority stays on the calling/loading thread.
            for request in [AttachedUiRequest::AttachedSize, AttachedUiRequest::CanResize,
                AttachedUiRequest::Resize { width: 320, height: 240 },
                AttachedUiRequest::SetScale { factor: 1.0 },
                AttachedUiRequest::TakeResizeRequest, AttachedUiRequest::ServiceRunLoop] {
                let reply = ui.request(request)?;
                ui_shape(request, reply);
            }
            owned_reply = Some(producer.join().expect("compile-only joined producer"));
            Ok(())
        })
    })?;
    // Owned data can escape. The owner becomes available only after callback AND join.
    if let Some(reply) = owned_reply { reply_shape(reply); }
    owner_control(owner).expect("compile-only post-join administration");
    owner.with_scoped_worker(&mut |ui, io| {
        let _ = ui.request(AttachedUiRequest::ServiceRunLoop)?;
        let _ = io.is_closed();
        Ok(())
    })
}
'''


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def write_json(path, value):
    temporary = path.with_suffix(path.suffix + '.tmp')
    temporary.write_text(json.dumps(value, indent=2) + '\n', encoding='utf-8')
    temporary.replace(path)


def build_cases():
    cases = []

    def case(name, body='', codes=(), fragments=(), reason='', feature=True,
             required_codes=None):
        source = (FEATURE_COMMON if feature else OWNER_COMMON) + '\n' + body + '\n'
        markers = [n for n, line in enumerate(source.splitlines(), 1) if '// CONTRACT' in line]
        if codes and not markers:
            raise ValueError(f'Negative fixture lacks a marker: {name}')
        cases.append(dict(name=name, feature=feature, source=source,
                          expect='fail' if codes else 'pass', allowed_codes=list(codes),
                          required_codes=list(codes if required_codes is None else required_codes),
                          fragments=list(fragments), contract_lines=markers, reason=reason))

    case('positive_feature_on_public_session', POSITIVE_SESSION, reason=(
        'The actual external feature-on API admits same-thread attached UI and an inner '
        'scoped Send producer, owned replies, and post-join owner reuse; closed enums stay explicit.'))
    case('positive_feature_off_owner', feature=False, reason=(
        'The real external owner remains available without the experimental feature.'))

    for name, action, codes, fragments in [
        ('save', 'let _ = owner.save_state()?;', ['E0502'], ['immutable', 'mutable']),
        ('reset', 'let _ = owner.process_reset_origin(128, transport())?;',
         ['E0500', 'E0501'], ['closure', 'borrow']),
        ('reconfigure', 'owner.reconfigure(48_000.0, 128)?;',
         ['E0500', 'E0501'], ['closure', 'borrow']),
        ('reborrow', 'let _ = owner.with_scoped_worker(&mut |_, _| Ok(()));',
         ['E0500', 'E0501'], ['closure', 'borrow']),
    ]:
        case(f'owner_cannot_{name}_during_session', f'''fn contract(owner: &mut MainThreadPlugin) {{
    let _ = owner.with_scoped_worker(&mut |_, io| {{ // CONTRACT
        {action} // CONTRACT
        let _ = io.is_closed();
        Ok(())
    }});
}}''', codes, fragments, f'The owner stays exclusively borrowed during {name}.')
    case('owner_cannot_drop_during_session', '''fn contract(mut owner: MainThreadPlugin) {
    let _ = owner.with_scoped_worker(&mut |_, io| { // CONTRACT
        drop(owner); // CONTRACT
        let _ = io.is_closed();
        Ok(())
    });
}''', ['E0505', 'E0507'], ['cannot move out', 'owner', 'borrowed', 'FnMut'],
         'The owner and its retained module cannot be dropped while the session exists.')

    for name, ty, argument in [('worker', 'WorkerIo', 'io'), ('ui', 'AttachedUiBridge', 'ui')]:
        case(f'{name}_cannot_escape_outer_storage', f'''fn contract(owner: &mut MainThreadPlugin) {{
    let mut escaped: Option<&mut {ty}<'_>> = None;
    let _ = owner.with_scoped_worker(&mut |ui, io| {{
        escaped = Some({argument}); // CONTRACT
        Ok(())
    }});
    std::hint::black_box(escaped);
}}''', ['E0521'], ['escapes', argument],
             f'Neither the callback borrow nor the branded {ty} can escape into outer storage.')
    case('worker_cannot_detach_thread', '''fn contract(owner: &mut MainThreadPlugin) {
    let _ = owner.with_scoped_worker(&mut |_, io| {
        std::thread::spawn(move || { io.has_pending() }); // CONTRACT
        Ok(())
    });
}''', ['E0521', None], ["'static", 'io'],
         'Send cannot extend either borrowed WorkerIo lifetime into a detached static worker.',
         required_codes=[])
    case('worker_cannot_enter_outer_scope', '''fn contract(owner: &mut MainThreadPlugin) {
    std::thread::scope(|scope| {
        let _ = owner.with_scoped_worker(&mut |_, io| {
            scope.spawn(move || { io.has_pending() }); // CONTRACT
            Ok(())
        });
    });
}''', ['E0521'], ['escapes', 'io'],
         'A thread scope outside the callback cannot extend the producer borrow lifetime.')

    for ty, label, traits in [('MainThreadPlugin', 'owner', ('Send', 'Sync')),
                             ("AttachedUiBridge<'static>", 'ui', ('Send', 'Sync')),
                             ("WorkerIo<'static>", 'worker', ('Sync', 'Clone'))]:
        for trait in traits:
            case(f'{label}_not_{trait.lower()}', f'''fn contract() {{
    fn require<T: {trait}>() {{}}
    require::<{ty}>(); // CONTRACT
}}''', ['E0277'], [trait, ty.split('<')[0]],
                 f'The actual external {ty} cannot satisfy {trait}.')

    for name, ty, expression, method, category in [
        ('worker_no_metadata', 'WorkerIo', 'port.info()', 'info', 'metadata'),
        ('worker_no_lifecycle', 'WorkerIo', 'port.stop_processing()', 'stop_processing', 'lifecycle'),
        ('worker_no_controller', 'WorkerIo', 'port.get_parameter(0)', 'get_parameter', 'controller'),
        ('ui_no_controller', 'AttachedUiBridge', 'port.get_parameter(0)', 'get_parameter', 'controller'),
        ('ui_no_lifecycle', 'AttachedUiBridge', 'port.start_processing()', 'start_processing', 'lifecycle'),
        ('worker_no_raw_channels', 'WorkerIo', 'port.into_channels()', 'into_channels', 'channel extraction'),
        ('worker_no_raw_sender', 'WorkerIo', 'port.sender()', 'sender', 'channel access'),
        ('worker_no_com', 'WorkerIo', 'port.query_interface()', 'query_interface', 'COM'),
        ('ui_no_raw_pointer', 'AttachedUiBridge', 'port.as_raw()', 'as_raw', 'raw editor extraction'),
        ('worker_no_extractor', 'WorkerIo', 'port.into_inner()', 'into_inner', 'worker/owner extraction'),
    ]:
        case(name, f'''fn contract(port: &mut {ty}<'_>) {{
    let _ = {expression}; // CONTRACT
}}''', ['E0599'], [f'`{method}`', ty], f'{ty} exposes no {category} authority.')
    case('ports_keep_backing_fields_private', '''fn contract(io: &mut WorkerIo<'_>, ui: &mut AttachedUiBridge<'_>) {
    let _ = &io.command; // CONTRACT
    let _ = &io.reply; // CONTRACT
    let _ = &ui.dispatch; // CONTRACT
}''', ['E0616'], ['`command`', '`reply`', '`dispatch`', 'private'],
         'Both channel endpoints and the attached-editor dispatcher remain private.')
    case('worker_no_deref', '''fn contract() {
    fn require<T: std::ops::Deref>() {}
    require::<WorkerIo<'static>>(); // CONTRACT
}''', ['E0277'], ['Deref', 'WorkerIo'], 'No Deref route to raw channels or a runtime/owner.')
    case('owner_no_extractor', '''fn contract(owner: MainThreadPlugin) {
    let _ = owner.into_inner(); // CONTRACT
}''', ['E0599'], ['`into_inner`', 'MainThreadPlugin'],
         'The main-thread wrapper cannot yield a movable legacy plugin owner.')
    case('worker_requires_exclusive_receiver', '''fn contract(io: &WorkerIo<'_>) {
    let _ = io.try_submit(WorkerCommand::TakeOutputEvents); // CONTRACT
}''', ['E0596'], ['io', 'mutable'], 'Submitting work requires exclusive access to the single-credit port.')
    case('private_runtime_and_test_hook_inaccessible', '''use vst3_host::internal; // CONTRACT
''', ['E0603'], ['`internal`', 'private'],
         'The private domain runtime, processing loan and cfg(test) run_for_test hook are not external API.')

    case('feature_off_module_absent', '''use vst3_host::scoped_worker; // CONTRACT
''', ['E0432'], ['scoped_worker', 'configured out'],
         'Without opt-in the public scoped_worker module does not exist.', feature=False)
    case('feature_off_method_absent', '''fn contract(owner: &mut MainThreadPlugin) {
    let _ = owner.with_scoped_worker(&mut |_, _| Ok(())); // CONTRACT
}''', ['E0599'], ['`with_scoped_worker`', 'MainThreadPlugin'],
         'Without opt-in the owner has no with_scoped_worker entry point.', feature=False)
    if len({case['name'] for case in cases}) != len(cases):
        raise ValueError('Duplicate contract names')
    return cases


def feature_closure(manifest, roots):
    features = manifest.get('features', {})
    seen, todo = set(), list(roots)
    while todo:
        feature = todo.pop()
        if feature in seen:
            continue
        seen.add(feature)
        todo.extend(features.get(feature, []))
    return seen


def check_defaults(repo):
    evidence = {}
    for relative in ('Cargo.toml', 'vendor/vst3-host-0.9.0/Cargo.toml',
                     'vendor/vst3-host-0.9.0/Cargo.toml.orig'):
        path = repo / relative
        data = tomllib.loads(path.read_text(encoding='utf-8'))
        if FEATURE not in data.get('features', {}):
            raise ValueError(f'Missing explicit feature in {relative}')
        closure = feature_closure(data, ['default'])
        if any(item == FEATURE or item.split('/')[-1] == FEATURE for item in closure):
            raise ValueError(f'Experimental feature reachable from defaults: {relative}')
        dependency_tables = [data.get('dependencies', {})]
        dependency_tables += [value.get('dependencies', {}) for value in data.get('target', {}).values()]
        for dependencies in dependency_tables:
            dep = dependencies.get('vst3-host', {})
            if isinstance(dep, dict) and FEATURE in dep.get('features', []):
                raise ValueError(f'Experimental vendor feature enabled by dependency defaults: {relative}')
        evidence[relative] = dict(sha256=sha256(path), default=data['features'].get('default', []),
                                  default_closure=sorted(closure))
    return evidence


def validate_inputs(repo, manifest_path, lock_path):
    data = tomllib.loads(manifest_path.read_text(encoding='utf-8'))
    if set(data) != {'package', 'lib', 'features', 'dependencies', 'workspace'}:
        raise ValueError('Manifest must have only package/lib/features/dependencies/workspace tables')
    package = data['package']
    required = dict(name=PACKAGE, version='0.0.0', edition='2021', publish=False, build=False)
    optional = {'autolib', 'autobins', 'autoexamples', 'autotests', 'autobenches'}
    if any(package.get(k) != v for k, v in required.items()) or set(package) - set(required) - optional:
        raise ValueError('Unexpected harness package; use the manifest format in --help')
    if any(package[key] is not False for key in set(package) & optional):
        raise ValueError('Automatic target options, when present, must be false')
    lib = data['lib']
    if lib.get('path') != 'active_fixture.rs' or set(lib) - {'path', 'name', 'test', 'doctest'}:
        raise ValueError('Harness must compile active_fixture.rs as its sole library target')
    if lib.get('name', TARGET_NAME) != TARGET_NAME:
        raise ValueError('Unexpected library name')
    if any(lib[key] is not False for key in ('test', 'doctest') if key in lib):
        raise ValueError('Tests/doctests must not be enabled in the harness manifest')
    if data['features'] != {'default': [], FEATURE: [f'vst3-host/{FEATURE}']}:
        raise ValueError('Unexpected harness feature table')
    vendor = (repo / 'vendor/vst3-host-0.9.0').resolve()
    expected_dependency = {'path': str(vendor), 'default-features': False,
                           'features': sorted(BASE_FEATURES)}
    dependency = data['dependencies'].get('vst3-host', {})
    normalized = dict(dependency) if isinstance(dependency, dict) else {}
    if isinstance(normalized.get('features'), list):
        normalized['features'] = sorted(normalized['features'])
    if set(data['dependencies']) != {'vst3-host'} or normalized != expected_dependency:
        raise ValueError('Dependency must be exactly the selected real vendor with baseline features')
    if data['workspace'] != {}:
        raise ValueError('The harness must be an empty standalone workspace')
    lock = tomllib.loads(lock_path.read_text(encoding='utf-8'))
    if lock.get('version') not in (3, 4):
        raise ValueError('Unsupported lockfile version')
    for name, version in ((PACKAGE, '0.0.0'), ('vst3-host', '0.9.0')):
        packages = [p for p in lock.get('package', []) if p.get('name') == name]
        if len(packages) != 1 or packages[0].get('version') != version or 'source' in packages[0]:
            raise ValueError(f'Lockfile must contain the exact local {name} package')
    return check_defaults(repo)


def input_paths(repo, manifest, lock):
    source = repo / 'vendor/vst3-host-0.9.0'
    paths = {f'vendor_source/{p.relative_to(source)}': p for p in sorted((source / 'src').rglob('*.rs'))}
    paths.update({'input/Cargo.toml': manifest, 'input/Cargo.lock': lock,
                  'repository/Cargo.toml': repo / 'Cargo.toml',
                  'repository/Cargo.lock': repo / 'Cargo.lock',
                  'vendor/Cargo.toml': source / 'Cargo.toml',
                  'vendor/Cargo.toml.orig': source / 'Cargo.toml.orig',
                  'runner/check_vst3_scoped_worker_contracts.py': Path(__file__).resolve()})
    return paths


def hash_inputs(paths):
    return {name: sha256(path) for name, path in paths.items()}


def config_paths(cwd, cargo_home):
    paths = set()
    for parent in (cwd, *cwd.parents):
        for name in ('config', 'config.toml'):
            path = parent / '.cargo' / name
            if path.exists():
                paths.add(path.resolve())
    for name in ('config', 'config.toml'):
        path = cargo_home / name
        if path.exists():
            paths.add(path.resolve())
    return sorted(paths)


def check_environment(target, cwd):
    env = os.environ.copy()
    allowed_cargo = {'CARGO_HOME', 'CARGO_TARGET_DIR', 'CARGO_BUILD_JOBS', 'CARGO_INCREMENTAL',
                     'CARGO_PROFILE_DEV_DEBUG', 'CARGO_PROFILE_TEST_DEBUG', 'CARGO_TERM_COLOR',
                     'CARGO_NET_OFFLINE'}
    allowed_rust = {'RUSTUP_HOME', 'RUSTUP_TOOLCHAIN', 'RUSTUP_AUTO_INSTALL'}
    unknown = [key for key, value in env.items() if value and (
        (key.startswith('CARGO_') and key not in allowed_cargo) or
        (key.startswith('RUST') and key not in allowed_rust))]
    if unknown:
        raise ValueError(f'Unknown Cargo/Rust flags or wrappers in environment: {sorted(unknown)}')
    inherited = env.get('CARGO_TARGET_DIR')
    if inherited and Path(inherited).resolve() != target:
        raise ValueError('--target-dir conflicts with inherited CARGO_TARGET_DIR')
    configs = config_paths(cwd, Path(env.get('CARGO_HOME', str(Path.home() / '.cargo'))).resolve())
    evidence = []
    # Deliberately narrow: inspect configuration without silently deleting user flags.
    allowed_top = {'net', 'http', 'registries', 'registry', 'source', 'term', 'build'}
    allowed_build = {'jobs', 'target-dir', 'incremental'}
    for path in configs:
        data = tomllib.loads(path.read_text(encoding='utf-8'))
        if set(data) - allowed_top or set(data.get('build', {})) - allowed_build:
            raise ValueError(f'Unknown compiler-affecting Cargo configuration: {path}')
        # Registry redirection can replace dependencies. Preserve ordinary offline
        # settings only; source replacement needs its own reviewed harness format.
        if any(key in data for key in ('source', 'registries', 'registry')):
            raise ValueError(f'Registry/source replacement configuration is unsupported: {path}')
        evidence.append(dict(path=str(path), sha256=sha256(path)))
    env.update(CARGO_TARGET_DIR=str(target), CARGO_ENCODED_RUSTFLAGS='',
               RUSTC_WRAPPER='', RUSTC_WORKSPACE_WRAPPER='', CARGO_NET_OFFLINE='true')
    return env, evidence


def disk_evidence(paths):
    result = {}
    for path in paths:
        free = shutil.disk_usage(path).free
        result[str(path)] = free
        if free < DISK_FLOOR:
            raise RuntimeError(f'Free space below 2 GiB floor: {path} ({free} bytes)')
    return result


@contextmanager
def exclusive_lock(path):
    with path.open('a+', encoding='utf-8') as stream:
        try:
            fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as exc:
            raise RuntimeError(f'Another runner owns lock: {path}') from exc
        stream.seek(0)
        stream.truncate()
        stream.write(json.dumps({'pid': os.getpid(), 'started_utc': time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())}) + '\n')
        stream.flush()
        try:
            yield
        finally:
            # Never unlink: another waiter could otherwise lock a different inode.
            fcntl.flock(stream, fcntl.LOCK_UN)


def read_json_lines(path):
    messages, invalid = [], []
    for line in path.read_text(encoding='utf-8', errors='replace').splitlines():
        try:
            item = json.loads(line)
        except json.JSONDecodeError:
            invalid.append(line)
            continue
        if isinstance(item, dict):
            messages.append(item)
        else:
            invalid.append(line)
    return messages, invalid


def check_feature_evidence(metadata, repo, enabled):
    vendor_manifest = (repo / 'vendor/vst3-host-0.9.0/Cargo.toml').resolve()
    packages = [p for p in metadata.get('packages', []) if p.get('name') == 'vst3-host']
    if len(packages) != 1 or Path(packages[0].get('manifest_path', '')).resolve() != vendor_manifest:
        raise ValueError('Metadata did not resolve exactly the selected real vendor manifest')
    vendor = packages[0]
    roots = [p for p in metadata['packages'] if p.get('name') == PACKAGE]
    if len(roots) != 1 or metadata.get('resolve', {}).get('root') != roots[0]['id']:
        raise ValueError('Metadata root is not the external contract harness')
    nodes = {node['id']: node for node in metadata['resolve']['nodes']}
    actual = set(nodes[vendor['id']]['features'])
    expected = BASE_FEATURES | {'cpal'} | ({FEATURE} if enabled else set())
    if actual != expected:
        raise ValueError(f'Unexpected vendor features: {sorted(actual)}, wanted {sorted(expected)}')
    root_features = set(nodes[roots[0]['id']]['features'])
    if root_features != ({FEATURE} if enabled else set()):
        raise ValueError(f'Unexpected harness features: {sorted(root_features)}')
    targets = [t for t in vendor['targets'] if 'lib' in t['kind']]
    source = vendor_manifest.parent / 'src/lib.rs'
    if len(targets) != 1 or Path(targets[0]['src_path']).resolve() != source.resolve():
        raise ValueError('Vendor does not compile its actual production library source')
    return dict(package_id=vendor['id'], root_package_id=roots[0]['id'],
                features=sorted(actual), source_path=str(source))


def check_diagnostics(spec, returncode, messages, fixture, feature_evidence):
    diagnostics = [item['message'] for item in messages if item.get('reason') == 'compiler-message'
                   and item.get('message', {}).get('level') == 'error']
    rendered = '\n'.join(d.get('rendered') or d.get('message', '') for d in diagnostics)
    codes = {(d.get('code') or {}).get('code') for d in diagnostics}
    problems = []
    artifacts = [item for item in messages if item.get('reason') == 'compiler-artifact']
    vendor_artifacts = [item for item in artifacts if item.get('package_id') == feature_evidence['package_id']
                        and item.get('target', {}).get('name') == 'vst3_host']
    if not vendor_artifacts or any(set(item.get('features', [])) != set(feature_evidence['features'])
                                   for item in vendor_artifacts):
        problems.append('Missing or incorrect real-vendor artifact feature evidence')
    if any(item.get('profile', {}).get('test') is True for item in artifacts):
        problems.append('An unexpected test target was compiled')
    if any('bin' in item.get('target', {}).get('kind', []) for item in artifacts):
        problems.append('An unexpected binary/helper target was compiled')
    finished = [item for item in messages if item.get('reason') == 'build-finished']
    if len(finished) != 1 or finished[0].get('success') != (returncode == 0):
        problems.append('Missing or inconsistent Cargo build-finished evidence')
    if spec['expect'] == 'pass':
        if returncode != 0 or diagnostics:
            problems.append('Positive real-external-API control did not compile')
        if not any(item.get('package_id') == feature_evidence['root_package_id']
                   and item.get('target', {}).get('name') == TARGET_NAME for item in artifacts):
            problems.append('Positive control has no harness library artifact')
    else:
        if returncode != 101 or not diagnostics:
            problems.append('Negative needs Cargo rustc-error exit 101 and actual error diagnostics')
        unexpected = codes - set(spec['allowed_codes'])
        if unexpected:
            problems.append(f'Unexpected error codes: {sorted(map(str, unexpected))}')
        missing = set(spec['required_codes']) - codes
        if missing:
            problems.append(f'Missing required error codes: {sorted(map(str, missing))}')
        for diagnostic in diagnostics:
            code = (diagnostic.get('code') or {}).get('code')
            if code is None and diagnostic.get('message') != 'lifetime may not live long enough':
                problems.append('Unexpected unnumbered error; only the exact lifetime diagnostic is accepted')
            if code == 'E0521' and 'borrowed data escapes' not in diagnostic.get('message', ''):
                problems.append('Unexpected E0521 message')
            marked = False
            for span in diagnostic.get('spans', []):
                path = Path(span.get('file_name', ''))
                if not path.is_absolute():
                    path = fixture.parent / path
                if span.get('is_primary') and path.resolve() == fixture.resolve():
                    marked |= any(span.get('line_start', 0) <= n <= span.get('line_end', 0)
                                  for n in spec['contract_lines'])
            if not marked:
                problems.append('Error primary span misses its marked contract expression')
        for fragment in spec['fragments']:
            if fragment not in rendered:
                problems.append(f'Missing intended diagnostic fragment: {fragment!r}')
    return problems, codes, rendered


def execute(command, env, cwd, log_prefix, report, report_path, disk_paths):
    report['free_space_before_command'] = disk_evidence(disk_paths)
    report['active_command'] = command
    report['active_log_prefix'] = str(log_prefix)
    write_json(report_path, report)
    write_json(log_prefix.with_suffix('.command.json'), command)
    with log_prefix.with_suffix('.stdout').open('w', encoding='utf-8') as stdout, \
            log_prefix.with_suffix('.stderr').open('w', encoding='utf-8') as stderr:
        # A private process group lets cancellation stop Cargo and its children before
        # the target lock is released. No shell, Rust executable, or plugin is launched.
        process = subprocess.Popen(command, env=env, cwd=cwd, stdout=stdout, stderr=stderr,
                                   text=True, start_new_session=True)
        try:
            returncode = process.wait()
        except BaseException:
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                process.wait()
            raise
    write_json(log_prefix.with_suffix('.exit.json'), {'returncode': returncode})
    report.pop('active_command', None)
    report.pop('active_log_prefix', None)
    write_json(report_path, report)
    return returncode


def interrupted(signum, _frame):
    raise InterruptedError(f'Received signal {signum}')


def run(args):
    repo, here, target = args.repo.resolve(), args.qa_dir.resolve(), args.target_dir.resolve()
    manifest, lock = args.manifest.resolve(), args.lockfile.resolve()
    if sys.platform != 'linux':
        raise ValueError('This compiler-contract runner requires the approved Linux implementation')
    if here == repo or here.is_relative_to(repo) or target == repo or target.is_relative_to(repo):
        raise ValueError('QA and target directories must be outside the repository')
    if here == target or here.is_relative_to(target) or target.is_relative_to(here):
        raise ValueError('QA and target directories must not overlap')
    if any(path == here or path.is_relative_to(here) for path in (manifest, lock)):
        raise ValueError('Approved input manifest/lock must be separate from generated QA output')
    here.mkdir(parents=True, exist_ok=True)
    target.mkdir(parents=True, exist_ok=True)
    with ExitStack() as stack:
        stack.enter_context(exclusive_lock(here / '.runner.lock'))
        stack.enter_context(exclusive_lock(target / '.scoped-worker-contracts.lock'))
        runs = here / 'runs'
        runs.mkdir(exist_ok=True)
        number = 1 + max([int(p.name) for p in runs.iterdir() if p.is_dir() and p.name.isdigit()] or [0])
        run_dir = runs / f'{number:04d}'
        run_dir.mkdir()
        logs, harness = run_dir / 'logs', run_dir / 'harness'
        logs.mkdir()
        harness.mkdir()
        report_path = run_dir / 'report.json'
        report = dict(schema=1, run_directory=str(run_dir), started_utc=time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime()),
                      repository=str(repo), target=str(target), target_coordinated=True,
                      disk_floor_bytes=DISK_FLOOR, rust_fixtures_executed=False, plugins_loaded=False,
                      compiler_only=True, complete=False, success=False, cases=[])
        write_json(report_path, report)
        paths, before, config_before, selected = {}, {}, {}, []
        try:
            disk_paths = [here, target, Path(os.environ.get('TMPDIR', '/tmp')).resolve()]
            report['initial_free_space'] = disk_evidence(disk_paths)
            report['default_feature_evidence'] = validate_inputs(repo, manifest, lock)
            paths = input_paths(repo, manifest, lock)
            before = hash_inputs(paths)
            report['input_paths'] = {name: str(path) for name, path in paths.items()}
            report['input_hashes_before'] = before
            for name, source in paths.items():
                destination = run_dir / 'snapshot' / name
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(source, destination)
                if sha256(destination) != before[name]:
                    raise RuntimeError(f'Input changed during archival: {source}')
            env, configs = check_environment(target, harness)
            report['cargo_configs'] = configs
            for index, config in enumerate(configs):
                path = Path(config['path'])
                config_before[str(path)] = config['sha256']
                shutil.copyfile(path, run_dir / f'cargo-config-{index:02d}.toml')
            report['build_environment'] = {key: value for key, value in env.items()
                if key.startswith(('CARGO_', 'RUST')) or key in ('PATH', 'PKG_CONFIG_PATH', 'LIBRARY_PATH', 'TMPDIR')}
            cargo, rustc = shutil.which('cargo', path=env.get('PATH')), shutil.which('rustc', path=env.get('PATH'))
            if not cargo or not rustc:
                raise RuntimeError('The approved toolchain must provide cargo and rustc on PATH')
            report['tool_paths'] = {'cargo': cargo, 'rustc': rustc}
            # Record invoked paths AND resolved executable bytes (rustup proxies may share a binary).
            report['tool_sha256'] = {name: sha256(Path(path).resolve()) for name, path in report['tool_paths'].items()}
            shutil.copyfile(manifest, harness / 'Cargo.toml')
            shutil.copyfile(lock, harness / 'Cargo.lock')
            cases = build_cases()
            unknown = set(args.case or []) - {case['name'] for case in cases}
            if unknown:
                raise ValueError(f'Unknown cases: {sorted(unknown)}')
            selected = [case for case in cases if not args.case or case['name'] in POSITIVE_CASES or case['name'] in args.case]
            report['selected_cases'] = [case['name'] for case in selected]
            fixtures = run_dir / 'fixtures'
            fixtures.mkdir()
            for case in cases:
                (fixtures / (case['name'] + '.rs')).write_text(case['source'], encoding='utf-8')
            write_json(run_dir / 'cases.json', [{key: value for key, value in case.items() if key != 'source'} for case in cases])
            report['fixture_hashes'] = {p.name: sha256(p) for p in sorted(fixtures.iterdir())}
            (harness / 'active_fixture.rs').write_text(OWNER_COMMON, encoding='utf-8')
            for name, command in [('rustc-version', [rustc, '--version', '--verbose']),
                                  ('cargo-version', [cargo, '--version', '--verbose']),
                                  ('repository-head', ['git', '-C', str(repo), 'rev-parse', 'HEAD'])]:
                if execute(command, env, harness, logs / name, report, report_path, disk_paths):
                    raise RuntimeError(f'Failed to record {name}')
            compiler = (logs / 'rustc-version.stdout').read_text(encoding='utf-8')
            host = re.search(r'^host: (\S+)$', compiler, re.MULTILINE)
            if not host or 'linux' not in host.group(1):
                raise ValueError('The selected rustc host is not Linux')
            feature_evidence = {}
            for enabled in (True, False):
                name = 'feature-on' if enabled else 'feature-off'
                flags = ['--no-default-features'] + (['--features', FEATURE] if enabled else [])
                command = [cargo, 'metadata', '--format-version=1', '--locked', '--offline',
                           '--manifest-path', str(harness / 'Cargo.toml'), '--filter-platform', host.group(1)] + flags
                if execute(command, env, harness, logs / (name + '-metadata'), report, report_path, disk_paths):
                    raise RuntimeError(f'Failed real dependency metadata: {name}')
                metadata = json.loads((logs / (name + '-metadata.stdout')).read_text(encoding='utf-8'))
                feature_evidence[enabled] = check_feature_evidence(metadata, repo, enabled)
            report['feature_evidence'] = {str(key): value for key, value in feature_evidence.items()}
            for spec in selected:
                if hash_inputs(input_paths(repo, manifest, lock)) != before:
                    raise RuntimeError('Reviewed source/input changed; refusing further compilation')
                _, current_configs = check_environment(target, harness)
                if {item['path']: item['sha256'] for item in current_configs} != config_before:
                    raise RuntimeError('Cargo configuration changed during this run')
                fixture = harness / 'active_fixture.rs'
                shutil.copyfile(fixtures / (spec['name'] + '.rs'), fixture)
                flags = ['--no-default-features'] + (['--features', FEATURE] if spec['feature'] else [])
                command = [cargo, 'check', '--locked', '--offline', '--manifest-path', str(harness / 'Cargo.toml'),
                           '--lib', '--message-format=json'] + flags
                report['active_case'] = spec['name']
                prefix = logs / spec['name']
                started = time.monotonic()
                returncode = execute(command, env, harness, prefix, report, report_path, disk_paths)
                messages, invalid = read_json_lines(prefix.with_suffix('.stdout'))
                problems, codes, rendered = check_diagnostics(spec, returncode, messages, fixture,
                                                               feature_evidence[spec['feature']])
                if invalid:
                    problems.append('Non-JSON output on Cargo compiler-message stream')
                prefix.with_suffix('.diagnostics.txt').write_text(rendered, encoding='utf-8')
                entry = dict(name=spec['name'], feature=spec['feature'], passed=not problems,
                             returncode=returncode, command=command, problems=problems,
                             codes=sorted(map(str, codes)), reason=spec['reason'],
                             fixture_sha256=sha256(fixture), seconds=round(time.monotonic() - started, 3))
                report['cases'].append(entry)
                report.pop('active_case', None)
                write_json(report_path, report)
                print(f'{"PASS" if not problems else "FAIL"} {spec["name"]} {entry["codes"]}', flush=True)
                if problems:
                    print('\n'.join(problems), flush=True)
                    print(rendered[-10000:] or prefix.with_suffix('.stderr').read_text(encoding='utf-8')[-10000:], flush=True)
                    if spec['expect'] == 'pass':
                        break  # No negative may count after a failed real-source positive.
            report['complete'] = len(report['cases']) == len(selected)
        except BaseException as exc:
            report['interruption'] = {'type': type(exc).__name__, 'message': str(exc)}
            print(f'Runner stopped: {type(exc).__name__}: {exc}', file=sys.stderr, flush=True)
        finally:
            try:
                after = hash_inputs(input_paths(repo, manifest, lock))
                report['input_hashes_after'] = after
                report['stable_inputs'] = bool(before) and before == after
                report['changed_inputs'] = sorted(key for key in set(before) | set(after) if before.get(key) != after.get(key))
                report['stable_generated_manifest'] = sha256(harness / 'Cargo.toml') == before.get('input/Cargo.toml')
                report['stable_generated_lock'] = sha256(harness / 'Cargo.lock') == before.get('input/Cargo.lock')
                _, current_configs = check_environment(target, harness)
                report['stable_configs'] = {item['path']: item['sha256'] for item in current_configs} == config_before
            except BaseException as exc:
                report['final_verification_error'] = f'{type(exc).__name__}: {exc}'
            report['passed'] = sum(item['passed'] for item in report['cases'])
            report['total'] = len(selected)
            report['success'] = bool(report['complete'] and selected and report['passed'] == len(selected)
                and all(report.get(key) for key in ('stable_inputs', 'stable_generated_manifest',
                                                   'stable_generated_lock', 'stable_configs'))
                and 'interruption' not in report and 'final_verification_error' not in report)
            report['finished_utc'] = time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())
            summary = (f'{report["passed"]}/{report["total"]} cases passed; complete: {report["complete"]}; '
                       f'stable source/inputs: {report.get("stable_inputs", False)}; success: {report["success"]}; '
                       'no Rust fixtures executed or plugins loaded')
            (run_dir / 'summary.txt').write_text(summary + '\n', encoding='utf-8')
            write_json(report_path, report)
            write_json(here / 'report.json', report)
            (here / 'summary.txt').write_text(summary + '\n', encoding='utf-8')
            with (here / 'history.jsonl').open('a', encoding='utf-8') as history:
                history.write(json.dumps({'run_directory': str(run_dir), 'success': report['success'],
                                          'passed': report['passed'], 'total': report['total'],
                                          'finished_utc': report['finished_utc']}) + '\n')
            print(summary, flush=True)
        return 0 if report['success'] else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter,
                                     allow_abbrev=False)
    parser.add_argument('--repo', type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument('--qa-dir', type=Path, required=True)
    parser.add_argument('--manifest', type=Path, required=True)
    parser.add_argument('--lockfile', type=Path, required=True)
    parser.add_argument('--target-dir', type=Path, required=True)
    parser.add_argument('--target-coordinated', action='store_true', required=True,
                        help='Attest that the parent reserved this Cargo target against unrelated build workers')
    parser.add_argument('--case', action='append', help='Named rerun; both positive controls always run first')
    args = parser.parse_args()
    for sig in (signal.SIGTERM, signal.SIGHUP):
        signal.signal(sig, interrupted)
    try:
        return run(args)
    except (OSError, ValueError, RuntimeError, InterruptedError) as exc:
        parser.exit(2, f'{type(exc).__name__}: {exc}\n')


if __name__ == '__main__':
    sys.exit(main())
