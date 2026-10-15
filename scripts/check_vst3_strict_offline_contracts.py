#!/usr/bin/env python3
"""Compile external contracts for the OFF-BY-DEFAULT Linux strict offline client.

Python 3.11+, Linux and a reviewed/archived source checkpoint are required. This
runner never executes a Rust fixture, loads a plug-in, or starts a helper. Cargo
check compiles the real vendor dependency; no substitute API, source-linked module
shim, private-source import, or production visibility change is permitted.
Dependency build scripts may run normally after explicit execution approval.

Supply an approved standalone --manifest and its exact pre-generated --lockfile.
The manifest format is intentionally narrow (the vendor path must be absolute):

    [package]
    name = "vst3-strict-offline-contracts"
    version = "0.0.0"
    edition = "2021"
    publish = false
    build = false
    [lib]
    path = "active_fixture.rs"
    [features]
    default = []
    experimental-fresh-helper = ["vst3-host/experimental-fresh-helper"]
    [dependencies.vst3-host]
    path = "/absolute/reviewed/repo/vendor/vst3-host-0.9.0"
    default-features = false
    features = ["cpal-backend", "process-isolation"]
    [workspace]

--reviewed-archived attests that the integrated sources, this runner and its
manifest/lock have been frozen, durably archived and independently reviewed, and
that compiler-only execution was approved. It does not grant that approval.
Use --target-coordinated to attest that the parent has reserved --target-dir
against other build workers. Advisory locks cannot reserve the target against
unrelated commands. No unknown compiler flags, wrappers, target overrides or
Cargo config includes are accepted. A 2 GiB free-space floor is enforced before
every command. Inputs, source/config snapshots, actual dependency feature
metadata, command/output hashes, generated fixtures, interruptions and all run
histories are retained in --qa-dir. Named reruns always begin with both positive
controls. A negative counts only if EVERY compiler error originates in the
external harness, matches its exact expected code and message fragments, and
has a primary span on the marked contract expression. There is no fixed source
module count. Changes to sources, manifests, lockfiles or fixtures fail the run.

These are API compiler contracts, not runtime cleanup, deadline or codec tests.
Synchronous load/state borrows intentionally may use local data, and owned
replies may escape. The contracts do not invent a borrower-token lifetime or
claim that an observation is an unforgeable cleanup authorization token.
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


FEATURE = 'experimental-fresh-helper'
PACKAGE = 'vst3-strict-offline-contracts'
TARGET_NAME = PACKAGE.replace('-', '_')
BASE_FEATURES = {'cpal-backend', 'process-isolation'}
DISK_FLOOR = 2 * 1024 ** 3
POSITIVE_CASES = {'positive_feature_on_public_client', 'positive_feature_off_owner'}

OWNER_COMMON = '''#![allow(dead_code, unused_imports, unused_variables)]
#![deny(unsafe_code)]
use vst3_host::{MainThreadPlugin, MidiChannel, MidiEvent, ProcessTransport};
use std::process::{Command, Child, ChildStdin, ChildStdout};
use std::time::Instant;

fn transport() -> ProcessTransport {
    ProcessTransport {
        sample_position: 0, quarter_note_position: 0.0, tempo: 120.0,
        playing: false, time_sig_numerator: 4, time_sig_denominator: 4,
    }
}

// Never called. Ordinary public owner operations resolve with the feature off.
fn owner_control(owner: &mut MainThreadPlugin) -> vst3_host::Result<()> {
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
use vst3_host::strict_offline::{self, Client, CancelHandle, SpawnFailure, Failure,
    FailureKind, Delivery, CleanupObservation, CleanupState, LoadSpec, LoadedPlugin,
    AudioBlock, captured_base_matches, EffectIdentity, StereoEffectMetadata};
use vst3_host::process_isolation::HostCommand;
'''

POSITIVE_CLIENT = '''
fn require_send_sync_clone_static<T: Send + Sync + Clone + 'static>() {}
fn require_owned<T: 'static>() {}
fn public_traits() {
    require_send_sync_clone_static::<CancelHandle>();
    require_owned::<LoadedPlugin>();
    require_owned::<AudioBlock>();
    require_owned::<EffectIdentity>();
    require_owned::<StereoEffectMetadata>();
    require_owned::<Failure>();
    require_owned::<CleanupObservation>();
}

// Function bodies are type-checked only. No helper, plug-in or thread is started.
fn public_spawn(command: &mut Command, deadline: Instant) -> Result<Client, SpawnFailure> {
    Client::spawn(command, deadline)
}
fn share_only_cancellation(client: &Client) -> (CancelHandle, CancelHandle) {
    let handle = client.cancellation();
    let clone = handle.clone();
    handle.cancel();
    (handle, clone)
}

// Local borrowed load data is legal: the request completes synchronously, the
// result owns its data, and the next client operation needs no retained token.
fn local_load(client: &mut Client, deadline: Instant) -> Result<LoadedPlugin, Failure> {
    let loaded = {
        let path = String::from("/compile-only/Example.vst3");
        let class_id = String::from("00112233445566778899AABBCCDDEEFF");
        let spec = LoadSpec {
            path: &path, class_id: &class_id, sample_rate: 48_000.0, tempo: 120.0,
            time_sig_numerator: 4, time_sig_denominator: 4,
        };
        client.load_plugin(spec, deadline)?
    };
    client.set_offline(deadline)?;
    Ok(loaded)
}
fn local_state(client: &mut Client, deadline: Instant) -> Result<(), Failure> {
    let result: Result<(), Failure> = {
        let state = vec![1_u8, 2, 3];
        client.load_state(&state, deadline)
    };
    result?;
    client.start_processing(deadline)
}
fn public_operations(client: &mut Client, deadline: Instant) -> Result<AudioBlock, Failure> {
    client.set_offline(deadline)?;
    let _: vst3_host::ResetOriginSupport = client.reset_origin_support(deadline)?;
    let _: u64 = client.native_dirty_revision(deadline)?;
    let _: Vec<vst3_host::Parameter> = client.parameters(deadline)?;
    client.set_parameter(0, 0.5_f32, deadline)?;
    client.set_parameter_f64(0, f64::from_bits(0.5_f64.to_bits() + 1), deadline)?;
    let raw: f64 = client.parameter(0, deadline)?;
    let _: bool = captured_base_matches(raw, 0.5_f32);
    let _: vst3_host::plugin::CheckedAudioBusLayout = client.checked_audio_layout(deadline)?;
    let _: vst3_host::plugin::EventBusLayout = client.event_layout(deadline)?;
    let _: u32 = client.latency_samples(deadline)?;
    let _: u32 = client.tail_samples(deadline)?;
    client.send_note(MidiEvent::NoteOn {
        channel: MidiChannel::Ch1, note: 60, velocity: 100,
    }, 0, deadline)?;
    let block: AudioBlock = client.process(2, transport(), deadline)?;
    client.stop_processing(deadline)?;
    Ok(block)
}
fn owned_spawn_cleanup(failure: SpawnFailure) -> (Failure, Option<Box<Client>>) {
    let SpawnFailure { failure, cleanup } = failure;
    (failure, cleanup)
}
fn continue_owned_cleanup(mut client: Box<Client>, deadline: Instant)
    -> (Box<Client>, CleanupObservation, Result<CleanupObservation, Failure>) {
    let observation: CleanupObservation = client.dispose(deadline, deadline);
    let continued: Result<CleanupObservation, Failure> = client.continue_cleanup();
    (client, observation, continued)
}
fn public_cleanup(client: &mut Client, deadline: Instant) -> CleanupObservation {
    let _first: CleanupObservation = client.dispose(deadline, deadline);
    let _continued: Result<CleanupObservation, Failure> = client.continue_cleanup();
    Client::cleanup_abandoned(deadline)
}
fn effect_operations(client: &mut Client, deadline: Instant)
    -> Result<(StereoEffectMetadata, AudioBlock), Failure> {
    let identity = EffectIdentity {
        class_id: "00112233445566778899AABBCCDDEEFF".into(), vendor: "v".into(),
        name: "effect".into(), version: "1".into(), category: "Fx|Delay".into(),
    };
    let _loaded = client.load_stereo_effect(LoadSpec {
        path: "/compile-only/Effect.vst3", class_id: &identity.class_id,
        sample_rate: 48_000.0, tempo: 120.0,
        time_sig_numerator: 4, time_sig_denominator: 4,
    }, &identity, deadline)?;
    client.set_offline(deadline)?;
    client.load_state(&[1_u8, 2, 3], deadline)?;
    client.set_parameter(0, 0.5_f32, deadline)?;
    client.set_parameter_f64(0, f64::from_bits(0.75_f64.to_bits() + 1), deadline)?;
    let layout = client.checked_audio_layout(deadline)?;
    let _selected = client.select_main_stereo_output(&layout, deadline)?;
    let metadata = client.admit_stereo_effect(deadline)?;
    client.start_processing(deadline)?;
    let audio = {
        let local = vec![vec![0.0_f32; 128]; 2];
        client.process_stereo_effect(&local, transport(), deadline)?
    };
    client.stop_processing(deadline)?;
    Ok((metadata, audio))
}
fn observation_fields(observation: CleanupObservation) {
    let CleanupObservation {
        state, shutdown_failure, kill_error, wait_error, pipe_error, kill_requested,
    } = observation;
    let _: Option<Failure> = shutdown_failure;
    let _: Option<i32> = kill_error;
    let _: Option<i32> = wait_error;
    let _: Option<i32> = pipe_error;
    let _: bool = kill_requested;
    match state {
        CleanupState::Reaped(status) => { let _: std::process::ExitStatus = status; },
        CleanupState::Pending | CleanupState::Empty | CleanupState::Busy => {},
    }
}
fn failure_fields(failure: Failure) {
    let Failure { kind, delivery, diagnostic, diagnostic_unavailable,
        request_context, metadata_getter, limit_evidence } = failure;
    let _: Option<String> = diagnostic;
    let _: Option<FailureKind> = diagnostic_unavailable;
    let _: Option<strict_offline::RequestContext> = request_context;
    let _: Option<strict_offline::MetadataGetter> = metadata_getter;
    let _: Option<strict_offline::LimitEvidence> = limit_evidence;
    match delivery { Delivery::NotSent | Delivery::MayHaveApplied => {} }
    match kind {
        FailureKind::UnsupportedRequest | FailureKind::UnsupportedReply |
        FailureKind::InvalidRequest | FailureKind::MalformedReply | FailureKind::WrongReply |
        FailureKind::Limit | FailureKind::Shape | FailureKind::Deadline | FailureKind::Cancelled |
        FailureKind::Allocation | FailureKind::Eof | FailureKind::ChildExited |
        FailureKind::ZeroWrite | FailureKind::Unsolicited | FailureKind::Terminal |
        FailureKind::Busy | FailureKind::Sigpipe | FailureKind::Spawn |
        FailureKind::OperationRejected | FailureKind::ReportedCrash |
        FailureKind::CleanupNotBegun => {},
        FailureKind::Io(errno) => { let _: i32 = errno; },
        FailureKind::MetadataRejected(reason) => {
            let _: vst3_host::MetadataQueryRejection = reason;
        },
    }
}
fn owned_reply_fields(block: AudioBlock, loaded: LoadedPlugin) {
    let AudioBlock { outputs, output_event_count, output_events_lost, transport_applied } = block;
    let _: Vec<Vec<f32>> = outputs;
    let _: usize = output_event_count;
    let _: (bool, bool) = (output_events_lost, transport_applied);
    let LoadedPlugin { vendor, name, version, category, uid, has_gui, audio_inputs,
        audio_outputs, output_channels, has_midi_input, has_midi_output, compatibility } = loaded;
    let _: (String, String, String, String, String) = (vendor, name, version, category, uid);
    let _: (bool, bool, bool) = (has_gui, has_midi_input, has_midi_output);
    let _: (i32, i32, i32) = (audio_inputs, audio_outputs, output_channels);
    let _: Vec<vst3_host::discovery::ClassCompatibility> = compatibility;
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

    def case(name, body='', code=None, fragments=(), reason='', feature=True):
        source = (FEATURE_COMMON if feature else OWNER_COMMON) + '\n' + body + '\n'
        markers = [n for n, line in enumerate(source.splitlines(), 1) if '// CONTRACT' in line]
        if code is not None and (not markers or not fragments or not re.fullmatch(r'E\d{4}', code)):
            raise ValueError(f'Negative fixture needs exact numbered code, fragments and marker: {name}')
        if code is None and markers:
            raise ValueError(f'Positive fixture unexpectedly has a negative marker: {name}')
        cases.append(dict(name=name, feature=feature, source=source,
                          expect='fail' if code is not None else 'pass',
                          expected_code=code, diagnostic_fragments=list(fragments),
                          contract_lines=markers, reason=reason))

    case('positive_feature_on_public_client', POSITIVE_CLIENT, reason=(
        'Real public Client operations, owned spawn-cleanup/replies, cancellation traits, '
        'and local synchronous load/state borrows compile without exposing a session.'))
    case('positive_feature_off_owner', feature=False, reason=(
        'Ordinary public owner operations remain usable without the strict feature.'))

    for ty, label, traits in [('Client', 'client', ('Send', 'Sync', 'Clone')),
                             ('SpawnFailure', 'spawn_failure', ('Send',))]:
        for trait in traits:
            words = {'Send': 'cannot be sent between threads safely',
                     'Sync': 'cannot be shared between threads safely',
                     'Clone': 'is not satisfied'}
            case(f'{label}_not_{trait.lower()}', f'''fn contract() {{
    fn require<T: {trait}>() {{}}
    require::<{ty}>(); // CONTRACT
}}''', 'E0277', [ty, trait, words[trait]], f'{ty} cannot satisfy {trait}.')

    for ty, label in [('Client', 'client'), ('CancelHandle', 'cancel'),
                      ('CleanupObservation', 'cleanup_observation')]:
        for trait, suffix in [('std::ops::Deref', 'deref'),
                              ('std::os::fd::AsRawFd', 'as_raw_fd'),
                              ('std::os::fd::AsFd', 'as_fd'),
                              ('std::os::fd::IntoRawFd', 'into_raw_fd')]:
            short = trait.rsplit('::', 1)[1]
            case(f'{label}_no_{suffix}', f'''fn contract() {{
    fn require<T: {trait}>() {{}}
    require::<{ty}>(); // CONTRACT
}}''', 'E0277', [ty, short, 'is not satisfied'],
                 f'{ty} grants no {short} route to raw session, child or descriptor authority.')

    # One marked boundary per case; missing methods cannot be hidden by an
    # unrelated Rust failure or by private source substituted into the harness.
    for name, ty, expression, method, reason in [
        ('client_no_into_inner', 'Client', 'value.into_inner()', 'into_inner', 'owner extraction'),
        ('client_no_into_session', 'Client', 'value.into_session()', 'into_session', 'session extraction'),
        ('client_no_session', 'Client', 'value.session()', 'session', 'session access'),
        ('client_no_child', 'Client', 'value.child()', 'child', 'raw Child access'),
        ('client_no_into_child', 'Client', 'value.into_child()', 'into_child', 'raw Child extraction'),
        ('client_no_pipes', 'Client', 'value.into_pipes()', 'into_pipes', 'pipe extraction'),
        ('client_no_stdin', 'Client', 'value.stdin()', 'stdin', 'request pipe access'),
        ('client_no_stdout', 'Client', 'value.stdout()', 'stdout', 'reply pipe access'),
        ('client_no_raw_fd_method', 'Client', 'value.as_raw_fd()', 'as_raw_fd', 'inherent raw FD access'),
        ('client_no_send_command', 'Client', 'value.send_command(HostCommand::Shutdown)', 'send_command', 'generic HostCommand dispatch'),
        ('client_no_request', 'Client', 'value.request(HostCommand::Shutdown, deadline)', 'request', 'generic HostCommand dispatch'),
        ('client_no_queued_state', 'Client', 'value.try_submit(&[1_u8, 2, 3][..])', 'try_submit', 'queued borrowed state authority'),
        ('client_no_queued_load', 'Client', 'value.enqueue_load(spec)', 'enqueue_load', 'queued borrowed load authority'),
        ('cancel_no_exchange', 'CancelHandle', 'value.exchange(HostCommand::Shutdown, deadline)', 'exchange', 'exchange authority'),
        ('cancel_no_owner', 'CancelHandle', 'value.owner()', 'owner', 'owner authority'),
        ('cancel_no_session', 'CancelHandle', 'value.session()', 'session', 'session authority'),
        ('cancel_no_into_inner', 'CancelHandle', 'value.into_inner()', 'into_inner', 'owner extraction'),
        ('cancel_no_child', 'CancelHandle', 'value.child()', 'child', 'raw Child authority'),
        ('cancel_no_dispose', 'CancelHandle', 'value.dispose(deadline, deadline)', 'dispose', 'cleanup authority'),
        ('cancel_no_continue_cleanup', 'CancelHandle', 'value.continue_cleanup()', 'continue_cleanup', 'cleanup continuation authority'),
        ('observation_no_owner', 'CleanupObservation', 'value.owner()', 'owner', 'owner authority'),
        ('observation_no_into_inner', 'CleanupObservation', 'value.into_inner()', 'into_inner', 'owner extraction'),
        ('observation_no_continue_cleanup', 'CleanupObservation', 'value.continue_cleanup()', 'continue_cleanup', 'cleanup continuation authority'),
    ]:
        case(name, f'''fn contract(mut value: {ty}, spec: LoadSpec<'_>, deadline: Instant) {{
    let _ = {expression}; // CONTRACT
}}''', 'E0599', [f'`{method}`', ty, 'no method named'], f'{ty} exposes no {reason}.')

    case('client_session_field_private', '''fn contract(client: &Client) {
    let _ = &client.session; // CONTRACT
}''', 'E0616', ['`session`', 'Client', 'private'],
         'The real FreshHelperSession field is private to Client.')
    case('client_generic_exchange_private', '''fn contract() {
    let _ = Client::exchange; // CONTRACT
}''', 'E0624', ['`exchange`', 'private'], 'Internal exchange cannot be named externally.')
    case('client_success_exchange_private', '''fn contract() {
    let _ = Client::success; // CONTRACT
}''', 'E0624', ['`success`', 'private'], 'Internal success dispatch cannot be named externally.')
    case('cancel_backing_authority_private', '''fn contract(cancel: CancelHandle) {
    let _ = cancel.0; // CONTRACT
}''', 'E0616', ['`0`', 'CancelHandle', 'private'],
         'The cancellation wrapper never exposes its backing capability.')

    for ty, label, field in [('Client', 'client', 'child'), ('Client', 'client', 'stdin'),
                            ('Client', 'client', 'stdout'),
                            ('CleanupObservation', 'observation', 'client'),
                            ('CleanupObservation', 'observation', 'child'),
                            ('CleanupObservation', 'observation', 'session')]:
        case(f'{label}_no_{field}_field', f'''fn contract(value: {ty}) {{
    let _ = value.{field}; // CONTRACT
}}''', 'E0609', [f'`{field}`', ty, 'no field'], f'{ty} carries no public {field} authority.')

    case('foundation_module_private', '''use vst3_host::fresh_helper; // CONTRACT
''', 'E0603', ['`fresh_helper`', 'private'],
         'The full transport foundation stays private even with the feature enabled.')
    case('foundation_session_private', '''use vst3_host::fresh_helper::FreshHelperSession; // CONTRACT
''', 'E0603', ['`fresh_helper`', 'private'],
         'The raw foundation owner is not an external dependency API.')
    case('feature_off_module_absent', '''use vst3_host::strict_offline; // CONTRACT
''', 'E0432', ['vst3_host::strict_offline', 'configured out'],
         'Without the experimental feature strict_offline is absent.', feature=False)

    for target, label in [('Child', 'child'), ('ChildStdin', 'stdin'),
                          ('ChildStdout', 'stdout'), ('std::os::fd::OwnedFd', 'owned_fd')]:
        short = target.rsplit('::', 1)[-1]
        case(f'client_cannot_convert_{label}', f'''fn contract(client: Client) {{
    let _: {target} = client.into(); // CONTRACT
}}''', 'E0277', [short, 'Client', 'From', 'is not satisfied'],
             f'Client grants no Into/From extraction into {target}.')
    case('cancel_cannot_convert_client', '''fn contract(cancel: CancelHandle) {
    let _: Client = cancel.into(); // CONTRACT
}''', 'E0277', ['Client', 'CancelHandle', 'From', 'is not satisfied'],
         'Clonable cancellation authority cannot be converted into the owner.')
    case('cancel_no_abandoned_cleanup', '''fn contract(deadline: Instant) {
    let _ = CancelHandle::cleanup_abandoned(deadline); // CONTRACT
}''', 'E0599', ['`cleanup_abandoned`', 'CancelHandle', 'no associated function or constant named'],
         'Cancellation-only authority has no abandoned-owner cleanup entry point.')

    case('spawn_failure_cleanup_is_client', '''fn contract(failure: SpawnFailure) {
    let _: Option<Box<Child>> = failure.cleanup; // CONTRACT
}''', 'E0308', ['mismatched types', 'Child', 'Client'],
         'SpawnFailure cleanup keeps the real Client owner, never a transferable raw Child.')
    case('observation_not_client', '''fn contract(observation: CleanupObservation) {
    let _: Client = observation; // CONTRACT
}''', 'E0308', ['mismatched types', 'Client', 'CleanupObservation'],
         'Cleanup evidence is not the still-owning client.')
    case('observation_cannot_convert_client', '''fn contract(observation: CleanupObservation) {
    let _: Client = observation.into(); // CONTRACT
}''', 'E0277', ['Client', 'CleanupObservation', 'From', 'is not satisfied'],
         'Cleanup evidence grants no implicit conversion into a new owner.')
    case('observation_cannot_convert_child', '''fn contract(observation: CleanupObservation) {
    let _: Child = observation.into(); // CONTRACT
}''', 'E0277', ['Child', 'CleanupObservation', 'From', 'is not satisfied'],
         'Cleanup evidence grants no implicit conversion into a raw Child.')
    case('legacy_command_not_load_spec', '''fn contract(client: &mut Client, deadline: Instant) {
    let _ = client.load_plugin(HostCommand::Shutdown, deadline); // CONTRACT
}''', 'E0308', ['mismatched types', 'LoadSpec', 'HostCommand'],
         'The typed load entry point cannot carry an arbitrary HostCommand.')
    case('state_exchange_has_no_queue_token', '''fn contract(client: &mut Client, deadline: Instant) {
    let result = client.load_state(&[1_u8, 2, 3], deadline);
    let _ = result.recv_reply(); // CONTRACT
}''', 'E0599', ['`recv_reply`', 'Result', 'no method named'],
         'A synchronous Result<(), Failure> is not a queued borrower or receive token.')
    case('client_requires_exclusive_exchange', '''fn contract(client: &Client, deadline: Instant) {
    let _ = client.load_state(&[1_u8, 2, 3], deadline); // CONTRACT
}''', 'E0596', ['client', 'mutable'],
         'Exchange requires exclusive Client access.')
    case('client_requires_exclusive_cleanup', '''fn contract(client: &Client, deadline: Instant) {
    let _ = client.dispose(deadline, deadline); // CONTRACT
}''', 'E0596', ['client', 'mutable'],
         'Disposal requires the owning Client exclusive access.')
    case('client_effect_admission_private', '''fn contract(client: &Client) {
    let _ = &client.effect; // CONTRACT
}''', 'E0616', ['`effect`', 'Client', 'private'],
         'A caller cannot forge the session-private Effect role and admission.')
    case('effect_process_requires_exclusive_client', '''fn contract(client: &Client, deadline: Instant) {
    let input = vec![vec![0.0_f32; 128]; 2];
    let _ = client.process_stereo_effect(&input, transport(), deadline); // CONTRACT
}''', 'E0596', ['client', 'mutable'],
         'Borrowed Effect input still requires exclusive owner-thread exchange.')

    if len({spec['name'] for spec in cases}) != len(cases):
        raise ValueError('Duplicate contract names')
    if {spec['name'] for spec in cases if spec['expect'] == 'pass'} != POSITIVE_CASES:
        raise ValueError('The mandatory positive controls changed unexpectedly')
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
    vendor = repo / 'vendor/vst3-host-0.9.0'
    paths = {}
    # Inventory every source/resource file, including additions and removals.
    # The application is not compiled, but it must also stay at the source freeze.
    for label, root in [('vendor_source', vendor / 'src'), ('application_source', repo / 'src')]:
        if not root.is_dir():
            raise ValueError(f'Missing production source tree: {root}')
        for source in sorted(root.rglob('*')):
            if source.is_symlink():
                raise ValueError(f'Symlinked source/resource is unsupported: {source}')
            if source.is_file():
                paths[f'{label}/{source.relative_to(root)}'] = source
    paths.update({'input/Cargo.toml': manifest, 'input/Cargo.lock': lock,
                  'repository/Cargo.toml': repo / 'Cargo.toml',
                  'repository/Cargo.lock': repo / 'Cargo.lock',
                  'vendor/Cargo.toml': vendor / 'Cargo.toml',
                  'vendor/Cargo.toml.orig': vendor / 'Cargo.toml.orig',
                  'vendor/Cargo.lock': vendor / 'Cargo.lock',
                  'runner/check_vst3_strict_offline_contracts.py': Path(__file__).resolve()})
    for label, root in [('repository', repo), ('vendor', vendor)]:
        for name in ('build.rs', 'rust-toolchain', 'rust-toolchain.toml'):
            source = root / name
            if source.exists():
                paths[f'{label}/{name}'] = source
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
    records = [item for item in messages if item.get('reason') == 'compiler-message'
               and item.get('message', {}).get('level') == 'error']
    diagnostics = [item['message'] for item in records]
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
        if codes != {spec['expected_code']}:
            problems.append(f'Expected only {spec["expected_code"]}; received {sorted(map(str, codes))}')
        covered_lines = set()
        for record in records:
            diagnostic = record['message']
            if record.get('package_id') != feature_evidence['root_package_id']:
                problems.append('Error originated outside the external contract harness')
            if record.get('target', {}).get('name') != TARGET_NAME:
                problems.append('Error originated in an unexpected compilation target')
            diagnostic_text = diagnostic.get('rendered') or diagnostic.get('message', '')
            # Match each error independently, never the union of unrelated errors.
            for fragment in spec['diagnostic_fragments']:
                if fragment not in diagnostic_text:
                    problems.append(f'Error misses intended diagnostic fragment: {fragment!r}')
            marked = set()
            for span in diagnostic.get('spans', []):
                path = Path(span.get('file_name', ''))
                if not path.is_absolute():
                    path = fixture.parent / path
                if span.get('is_primary') and path.resolve() == fixture.resolve():
                    marked.update(n for n in spec['contract_lines']
                                  if span.get('line_start', 0) <= n <= span.get('line_end', 0))
            if not marked:
                problems.append('Error primary span misses its marked contract expression')
            covered_lines.update(marked)
        if covered_lines != set(spec['contract_lines']):
            problems.append('A marked negative expression lacks its intended primary-span error')
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
        # the target lock is released. No shell, fixture executable or plug-in is launched.
        # Dependency build scripts may execute normally through Cargo.
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
    if any(path == output or path.is_relative_to(output)
           for path in (manifest, lock) for output in (here, target)):
        raise ValueError('Approved input manifest/lock must be separate from generated QA/target output')
    here.mkdir(parents=True, exist_ok=True)
    target.mkdir(parents=True, exist_ok=True)
    with ExitStack() as stack:
        stack.enter_context(exclusive_lock(here / '.runner.lock'))
        stack.enter_context(exclusive_lock(target / '.strict-offline-contracts.lock'))
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
                      reviewed_archived=True, execution_approval_attested=True,
                      disk_floor_bytes=DISK_FLOOR, rust_fixtures_executed=False, plugins_loaded=False,
                      compiler_only=True, complete=False, success=False, cases=[])
        write_json(report_path, report)
        paths, before, config_before, selected = {}, {}, {}, []
        fixture_before = {}
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
            fixture_before = {p.name: sha256(p) for p in sorted(fixtures.iterdir())}
            report['fixture_hashes_before'] = fixture_before
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
                if {p.name: sha256(p) for p in sorted(fixtures.iterdir())} != fixture_before:
                    raise RuntimeError('Archived generated fixtures changed; refusing compilation')
                if sha256(harness / 'Cargo.toml') != before['input/Cargo.toml'] or \
                        sha256(harness / 'Cargo.lock') != before['input/Cargo.lock']:
                    raise RuntimeError('Generated manifest/lock changed; refusing compilation')
                fixture = harness / 'active_fixture.rs'
                shutil.copyfile(fixtures / (spec['name'] + '.rs'), fixture)
                expected_fixture_hash = fixture_before[spec['name'] + '.rs']
                if sha256(fixture) != expected_fixture_hash:
                    raise RuntimeError('The active source does not match the archived fixture')
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
                if sha256(fixture) != expected_fixture_hash:
                    problems.append('The active source changed during compiler execution')
                prefix.with_suffix('.diagnostics.txt').write_text(rendered, encoding='utf-8')
                entry = dict(name=spec['name'], feature=spec['feature'], passed=not problems,
                             returncode=returncode, command=command, problems=problems,
                             codes=sorted(map(str, codes)), reason=spec['reason'],
                             fixture_sha256_before=expected_fixture_hash, fixture_sha256_after=sha256(fixture),
                             seconds=round(time.monotonic() - started, 3))
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
                fixture_after = {p.name: sha256(p) for p in sorted((run_dir / 'fixtures').iterdir())}
                report['fixture_hashes_after'] = fixture_after
                report['stable_generated_fixtures'] = bool(fixture_before) and fixture_before == fixture_after
                _, current_configs = check_environment(target, harness)
                report['stable_configs'] = {item['path']: item['sha256'] for item in current_configs} == config_before
            except BaseException as exc:
                report['final_verification_error'] = f'{type(exc).__name__}: {exc}'
            report['passed'] = sum(item['passed'] for item in report['cases'])
            report['total'] = len(selected)
            report['success'] = bool(report['complete'] and selected and report['passed'] == len(selected)
                and all(report.get(key) for key in ('stable_inputs', 'stable_generated_manifest',
                                                   'stable_generated_lock', 'stable_configs', 'stable_generated_fixtures'))
                and 'interruption' not in report and 'final_verification_error' not in report)
            report['command_log_hashes'] = {str(p.relative_to(logs)): sha256(p)
                                          for p in sorted(logs.rglob('*')) if p.is_file()}
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
    parser.add_argument('--reviewed-archived', action='store_true', required=True,
                        help='Attest independent source/runner/input review, durable archive and execution approval')
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
