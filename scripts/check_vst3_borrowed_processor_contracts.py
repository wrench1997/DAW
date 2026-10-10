#!/usr/bin/env python3
"""Compile the real cfg(test) borrowed-processor capability contracts, never run them.

Requires Python 3.11+, an approved runtime-only source-linked vendor manifest and
its matching lockfile, and an exclusive/coordinated Cargo target. The caller owns
PATH, CARGO_HOME, RUSTUP_HOME, platform libraries, and build profiles. Run only
after the exact unsafe source has been reviewed and durably archived.

The generated crate root source-links the actual private vendor modules. Cargo
check --lib --tests enables their existing cfg(test) items without a new feature,
global --cfg test, dependency changes, or production-file edits. Separate --lib
checks establish production reachability and the test module's absence. No Rust
fixture is a runnable test, and this runner never loads a plugin or calls Process.
All inputs, copied fixtures, raw output, and hash evidence stay in --qa-dir.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time
import tomllib


FEATURES = 'cpal-backend,process-isolation'
POSITIVE_CASES = {'positive_scoped_private_api', 'positive_production_owner_api'}
FORBIDDEN_CODES = {'E0432', 'E0433', 'E0412', 'E0425', 'E0603', 'E0624', 'E0583'}

COMMON = '''#![allow(dead_code, unused_imports, unused_variables)]
use crate::internal::plugin_impl::PluginImpl;
use crate::internal::plugin_impl::borrowed_processor_fixture::{
    borrow_for_test, BorrowedProcessor,
};
use crate::plugin::PluginInternal;
use crate::Result;
use vst3::Steinberg::{tresult, Vst::{IAudioProcessor, ProcessData}};

// Type-checking input only. It is constructed on the worker, never transferred
// from the loading thread. A real call needs owner-compatible data and valid,
// exclusive nested buffers/COM queues; this literal does not establish that.
fn compile_only_worker_data() -> ProcessData {
    ProcessData {
        processMode: 0, symbolicSampleSize: 0, numSamples: 0,
        numInputs: 0, numOutputs: 0,
        inputs: std::ptr::null_mut(), outputs: std::ptr::null_mut(),
        inputParameterChanges: std::ptr::null_mut(),
        outputParameterChanges: std::ptr::null_mut(),
        inputEvents: std::ptr::null_mut(), outputEvents: std::ptr::null_mut(),
        processContext: std::ptr::null_mut(),
    }
}

// This is an ordinary, never-called function, not a #[test]. Every test-cfg
// case resolves the same actual private entry point and process signature.
fn positive_same_private_api(owner: &mut PluginImpl) -> Result<()> {
    fn require_send<T: Send>() {}
    require_send::<BorrowedProcessor<'_>>();
    let mut token = borrow_for_test(owner)?;
    std::thread::scope(|scope| {
        scope.spawn(move || {
            let mut data = compile_only_worker_data();
            // Compile only. Executing this would additionally require every
            // SDK precondition, including nested-pointer validity, to hold.
            let result: tresult = unsafe { token.process(&mut data) };
            std::hint::black_box(result);
        }).join().expect("compile-only worker");
    });
    // The scope has rejoined and the token is gone: owner administration and
    // a second, sequential capability loan must both type-check again.
    owner.stop_processing()?;
    owner.start_processing()?;
    let _ = owner.save_state()?;
    let next = borrow_for_test(owner)?;
    drop(next);
    owner.reconfigure(48_000.0, 128)?;
    Ok(())
}
'''

PRODUCTION_COMMON = '''#![allow(dead_code, unused_imports, unused_variables)]
use crate::internal::plugin_impl::PluginImpl;
use crate::plugin::PluginInternal;
use crate::Result;

fn real_production_owner_is_reachable(owner: &mut PluginImpl) -> Result<()> {
    owner.stop_processing()?;
    owner.start_processing()?;
    let _ = owner.save_state()?;
    Ok(())
}
'''


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def source_hashes(repo, source):
    return {str(p.relative_to(repo)): sha256(p) for p in sorted(source.rglob('*.rs'))}


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n', encoding='utf-8')


def prepare(here, repo, manifest_input, lock_input):
    source = repo / 'vendor/vst3-host-0.9.0/src'
    fixtures = here / 'fixtures'
    fixtures.mkdir(exist_ok=True)
    cases = []

    def case(name, body='', codes=(), fragments=(), reason='', mode='test',
             required_codes=None):
        text = (COMMON if mode == 'test' else PRODUCTION_COMMON) + '\n' + body + '\n'
        (fixtures / (name + '.rs')).write_text(text, encoding='utf-8')
        contract_lines = [i for i, line in enumerate(text.splitlines(), 1)
                          if '// CONTRACT' in line]
        if codes and not contract_lines:
            raise ValueError(f'Negative fixture has no contract line: {name}')
        cases.append(dict(name=name, mode=mode, expect='fail' if codes else 'pass',
                          allowed_codes=list(codes), required_codes=list(
                              codes if required_codes is None else required_codes),
                          fragments=list(fragments), contract_lines=contract_lines,
                          reason=reason))

    case('positive_scoped_private_api', reason=(
        'Actual private capability is Send, moves into a joined scoped worker with '
        'worker-local SDK data, and releases the owner for later administration/loans.'))
    case('positive_production_owner_api', mode='production', reason=(
        'Actual private PluginImpl and its owner API compile without cfg(test).'))
    case('production_fixture_module_absent', '''
use crate::internal::plugin_impl::borrowed_processor_fixture; // CONTRACT
''', ['E0432'], ['borrowed_processor_fixture', 'configured out'], mode='production',
         reason='Only this separate visibility case expects the cfg(test) module to be absent.')

    for trait, fragments in [('Sync', ['Sync', 'BorrowedProcessor', 'shared']),
                             ('Clone', ['Clone', 'BorrowedProcessor'])]:
        case('token_not_' + trait.lower(), f'''fn contract() {{
    fn require<T: {trait}>() {{}}
    require::<BorrowedProcessor<'static>>(); // CONTRACT
}}''', ['E0277'], fragments, f'The real token does not implement {trait}.')

    methods = [
        ('clone', 'token.clone()', 'cloning'),
        ('info', 'token.info()', 'metadata'),
        ('get_parameters', 'token.get_parameters()', 'parameter metadata'),
        ('get_latency_samples', 'token.get_latency_samples()', 'latency metadata'),
        ('get_tail_samples', 'token.get_tail_samples()', 'tail metadata'),
        ('audio_bus_layout', 'token.audio_bus_layout()', 'bus metadata'),
        ('save_state', 'token.save_state()', 'state save'),
        ('load_state', 'token.load_state(&[])', 'state restore'),
        ('reconfigure', 'token.reconfigure(48_000.0, 128)', 'reconfiguration'),
        ('start_processing', 'token.start_processing()', 'activation'),
        ('stop_processing', 'token.stop_processing()', 'deactivation'),
        ('set_bus_arrangements', 'token.set_bus_arrangements(&[], &[])', 'bus changes'),
        ('set_active', 'token.set_active(false)', 'component lifecycle'),
        ('terminate', 'token.terminate()', 'component teardown'),
        ('reset_origin_support', 'token.reset_origin_support()', 'reset preflight'),
        ('process_reset_origin', 'token.process_reset_origin()', 'reset transaction'),
        ('as_ptr', 'token.as_ptr()', 'raw pointer extraction'),
        ('as_raw', 'token.as_raw()', 'raw pointer extraction'),
        ('into_raw', 'token.into_raw()', 'raw pointer extraction'),
        ('into_inner', 'token.into_inner()', 'owner/COM extraction'),
        ('cast', 'token.cast::<IAudioProcessor>()', 'COM cast'),
        ('query_interface', 'token.query_interface()', 'COM query'),
        ('add_ref', 'token.add_ref()', 'COM reference counting'),
        ('release', 'token.release()', 'COM reference counting'),
        ('processor', 'token.processor()', 'processor COM getter'),
        ('controller', 'token.controller()', 'controller COM getter'),
        ('component', 'token.component()', 'component COM getter'),
    ]
    for method, expression, category in methods:
        case('token_no_' + method, f'''fn contract(owner: &mut PluginImpl) -> Result<()> {{
    let mut token = borrow_for_test(owner)?;
    let _ = {expression}; // CONTRACT
    Ok(())
}}''', ['E0599'], [f'`{method}`', 'BorrowedProcessor'],
             f'The real token exposes no {category} method ({method}).')

    for trait in ('Deref', 'DerefMut'):
        case('token_no_' + trait.lower(), f'''fn contract() {{
    fn require<T: std::ops::{trait}>() {{}}
    require::<BorrowedProcessor<'static>>(); // CONTRACT
}}''', ['E0277'], [trait, 'BorrowedProcessor'], f'No {trait} escape to owner or COM.')
    for trait in ('AsRef', 'Borrow'):
        path = 'std::convert::AsRef' if trait == 'AsRef' else 'std::borrow::Borrow'
        for ty in ('IAudioProcessor', 'PluginImpl'):
            case(f'token_no_{trait.lower()}_{ty.lower()}', f'''fn contract() {{
    fn require<T: {path}<{ty}>>() {{}}
    require::<BorrowedProcessor<'static>>(); // CONTRACT
}}''', ['E0277'], [trait, ty, 'BorrowedProcessor'],
                 f'No {trait} escape to {ty}.')
    for field in ('processor', 'in_process', '_owner_and_module', '_not_sync'):
        case('token_private_' + field.lstrip('_'), f'''fn contract(owner: &mut PluginImpl) -> Result<()> {{
    let token = borrow_for_test(owner)?;
    let _ = &token.{field}; // CONTRACT
    Ok(())
}}''', ['E0616'], [f'`{field}`', 'BorrowedProcessor', 'private'],
             f'The real {field} field cannot expose pointer, gate, or ownership markers.')

    case('owner_no_mutable_use_while_token_live', '''fn contract(owner: &mut PluginImpl) -> Result<()> {
    let mut token = borrow_for_test(owner)?;
    owner.stop_processing()?; // CONTRACT
    std::hint::black_box(&mut token);
    Ok(())
}''', ['E0499'], ['owner', 'more than once'],
         'Owner mutable/lifecycle authority remains borrowed until the last token use.')
    case('owner_no_shared_use_while_token_live', '''fn contract(owner: &mut PluginImpl) -> Result<()> {
    let mut token = borrow_for_test(owner)?;
    let _ = owner.save_state()?; // CONTRACT
    std::hint::black_box(&mut token);
    Ok(())
}''', ['E0502'], ['owner', 'immutable', 'mutable'],
         'Even a shared owner operation cannot overlap the exclusive token loan.')
    case('owner_cannot_drop_while_token_live', '''fn contract(mut owner: PluginImpl) -> Result<()> {
    let mut token = borrow_for_test(&mut owner)?;
    drop(owner); // CONTRACT
    std::hint::black_box(&mut token);
    Ok(())
}''', ['E0505'], ['owner', 'cannot move out', 'borrowed'],
         'The owning aggregate and its retained module cannot be dropped while the token lives.')
    case('token_cannot_outlive_owner_and_module', '''fn contract(owner: PluginImpl) -> Result<()> {
    let mut token;
    {
        let mut local_owner = owner;
        token = borrow_for_test(&mut local_owner)?; // CONTRACT
    }
    std::hint::black_box(&mut token);
    Ok(())
}''', ['E0597'], ['local_owner', 'does not live long enough'],
         'Token cannot survive the lexical owner/module lifetime, even within one thread.')
    case('token_cannot_return_from_local_owner', '''fn contract(mut owner: PluginImpl) -> Result<BorrowedProcessor<'static>> {
    borrow_for_test(&mut owner) // CONTRACT
}''', ['E0515'], ['owner', 'returns a value referencing'],
         'No token can be returned after the local aggregate/module owner is destroyed.')
    case('token_cannot_detach_thread', '''fn contract(owner: &mut PluginImpl) -> Result<()> { // CONTRACT
    let mut token = borrow_for_test(owner)?; // CONTRACT
    std::thread::spawn(move || { // CONTRACT
        let mut data = compile_only_worker_data();
        // Compile only; a real SDK call requires valid exclusive nested data.
        let result: tresult = unsafe { token.process(&mut data) };
        std::hint::black_box(result);
    });
    Ok(())
}''', ['E0521'], ['owner', 'escapes', "'static"],
         'A token borrowed from a nonstatic owner cannot enter a detached thread::spawn.')
    case('owner_no_second_simultaneous_token', '''fn contract(owner: &mut PluginImpl) -> Result<()> {
    let mut first = borrow_for_test(owner)?;
    let second = borrow_for_test(owner)?; // CONTRACT
    std::hint::black_box(&mut first);
    std::hint::black_box(second);
    Ok(())
}''', ['E0499'], ['owner', 'more than once'],
         'A second capability cannot be acquired while the first exclusive loan is live.')
    case('token_shared_reference_cannot_cross_thread', '''fn contract(owner: &mut PluginImpl) -> Result<()> {
    let token = borrow_for_test(owner)?;
    std::thread::scope(|scope| {
        let shared = &token;
        scope.spawn(move || { // CONTRACT
            std::hint::black_box(shared);
        });
    });
    Ok(())
}''', ['E0277'], ['BorrowedProcessor', 'Sync', 'shared'],
         'Send permits token ownership transfer, not cross-thread transfer of shared token references.')
    case('token_process_requires_unsafe', '''fn contract(token: &mut BorrowedProcessor<'_>, data: &mut ProcessData) {
    let _ = token.process(data); // CONTRACT
}''', ['E0133'], ['process', 'unsafe'],
         'A mutable SDK ProcessData reference alone does not make the SDK call safe.')
    case('token_process_requires_mutable_token', '''unsafe fn contract(token: &BorrowedProcessor<'_>, data: &mut ProcessData) {
    let _ = unsafe { token.process(data) }; // CONTRACT
}''', ['E0596'], ['token', 'mutable'],
         'Only an exclusive token receiver can make a process call.')

    write_json(here / 'cases.json', cases)
    manifest = manifest_input.read_text(encoding='utf-8')
    parsed = tomllib.loads(manifest)
    production = tomllib.loads((source.parent / 'Cargo.toml').read_text(encoding='utf-8'))
    actual_lib = (manifest_input.parent / parsed['lib']['path']).resolve()
    if actual_lib != (source / 'lib.rs').resolve():
        raise ValueError('The input manifest must source-link the selected real vendor lib.rs')
    for key in ('features', 'dependencies', 'target'):
        if parsed.get(key, {}) != production.get(key, {}):
            raise ValueError(f'The input manifest differs from production {key}')
    if any(key in parsed for key in ('dev-dependencies', 'build-dependencies', 'bin', 'test', 'bench', 'example')):
        raise ValueError('Supply the approved runtime-only source-linked vendor manifest')
    if parsed.get('package', {}).get('build') is not False:
        raise ValueError('The approved runtime-only vendor manifest must retain build = false')
    if parsed.get('lib', {}).get('test') is False:
        raise ValueError('The supplied library target disables cfg(test) compilation')

    # Change only the library source path. No dependency, feature, or target is added.
    def replace_lib(match):
        body, count = re.subn(r'(?m)^path\s*=.*$', 'path = "lib.rs"', match.group(2))
        if count != 1:
            raise ValueError('Expected one library source path')
        return match.group(1) + body

    manifest, count = re.subn(r'(?ms)(^\[lib\]\n)(.*?)(?=^\[|\Z)', replace_lib, manifest)
    if count != 1:
        raise ValueError('Expected one library target')
    (here / 'Cargo.toml').write_text(manifest, encoding='utf-8')
    shutil.copyfile(lock_input, here / 'Cargo.lock')
    lib = (source / 'lib.rs').read_text(encoding='utf-8')
    modules = []

    def rebase(match):
        visibility, name = match.group(1) or '', match.group(2)
        matches = [p for p in (source / f'{name}.rs', source / name / 'mod.rs') if p.is_file()]
        if len(matches) != 1:
            raise ValueError(f'Unexpected source layout for {name}: {matches}')
        modules.append(str(matches[0]))
        return f'#[path = {json.dumps(str(matches[0]))}]\n{visibility}mod {name};'

    shim, count = re.subn(r'(?m)^(pub )?mod (\w+);$', rebase, lib)
    if count != 19:
        raise ValueError(f'Unexpected top-level module layout: {count}')
    for mode, gate in [('test', '#[cfg(test)]\n'), ('production', '')]:
        text = shim + '\n\n// Crate-internal compile-only QA fixture; never exported.\n'
        text += gate + '#[path = "active_fixture.rs"]\nmod private_borrowed_processor_contract;\n'
        (here / f'lib.{mode}.rs.template').write_text(text, encoding='utf-8')
    write_json(here / 'source-map.json', dict(production_root=str(source), modules=modules,
               borrowed_fixture=str(source / 'internal/plugin_impl/borrowed_processor_fixture.rs'),
               mechanism='Source-linked crate root; existing cfg(test) through cargo check --lib --tests'))
    return cases


def sanitized_environment(target):
    env = os.environ.copy()
    suspicious = {'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'RUSTC_WRAPPER',
                  'RUSTC_WORKSPACE_WRAPPER', 'CARGO_BUILD_RUSTFLAGS',
                  'CARGO_BUILD_RUSTC_WRAPPER', 'CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER',
                  'RUSTC_BOOTSTRAP'}
    suspicious.update(k for k in env if k.startswith('CARGO_TARGET_') and k.endswith('_RUSTFLAGS'))
    removed = {k: env.pop(k) for k in sorted(suspicious) if k in env}
    # Empty encoded flags have priority over target/config rustflags. Empty wrapper
    # variables also disable wrappers supplied by a Cargo config file.
    overrides = {'CARGO_ENCODED_RUSTFLAGS': '', 'RUSTC_WRAPPER': '',
                 'RUSTC_WORKSPACE_WRAPPER': '', 'CARGO_TARGET_DIR': str(target)}
    env.update(overrides)
    return env, removed, overrides


def check_diagnostics(spec, resultcode, messages):
    diagnostics = [item['message'] for item in messages
                   if item.get('reason') == 'compiler-message' and item['message']['level'] == 'error']
    rendered = '\n'.join(d.get('rendered') or d.get('message', '') for d in diagnostics)
    codes = {(d.get('code') or {}).get('code') for d in diagnostics}
    problems = []
    if spec['expect'] == 'pass':
        if resultcode != 0 or diagnostics:
            problems.append('positive actual-private-source fixture did not compile')
        test_artifact = any(item.get('reason') == 'compiler-artifact'
                            and item.get('target', {}).get('name') == 'vst3_host'
                            and item.get('profile', {}).get('test') is True for item in messages)
        if spec['mode'] == 'test' and not test_artifact:
            problems.append('Cargo did not emit evidence of the cfg(test) library target')
    else:
        if resultcode == 0:
            problems.append('negative fixture unexpectedly compiled')
        if not diagnostics:
            problems.append('failure contained no rustc error diagnostics')
        forbidden = FORBIDDEN_CODES - ({'E0432'} if spec['name'] == 'production_fixture_module_absent' else set())
        if codes & forbidden:
            problems.append(f'import/privacy-of-entry failure: {sorted(codes & forbidden)}')
        unexpected = codes - set(spec['allowed_codes'])
        if unexpected:
            problems.append(f'unexpected codes: {sorted(str(code) for code in unexpected)}')
        missing = set(spec['required_codes']) - codes
        if missing:
            problems.append(f'missing required codes: {sorted(missing)}')
        for diagnostic in diagnostics:
            if not any(span.get('is_primary')
                       and Path(span.get('file_name', '')).name == 'active_fixture.rs'
                       and any(span.get('line_start', 0) <= line <= span.get('line_end', 0)
                               for line in spec['contract_lines'])
                       for span in diagnostic.get('spans', [])):
                problems.append('error primary span misses the marked contract expression')
        for fragment in spec['fragments']:
            if fragment not in rendered:
                problems.append(f'missing diagnostic text: {fragment!r}')
    return problems, codes, rendered


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repo', type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument('--qa-dir', required=True, type=Path,
                        help='Generated inputs/evidence directory outside the repository')
    parser.add_argument('--manifest', required=True, type=Path,
                        help='Approved runtime-only source-linked vendor Cargo.toml')
    parser.add_argument('--lockfile', required=True, type=Path,
                        help='Exact matching Cargo.lock; never substituted or regenerated')
    parser.add_argument('--target-dir', type=Path, default=os.environ.get('CARGO_TARGET_DIR'),
                        help='Already coordinated Cargo target (or CARGO_TARGET_DIR)')
    parser.add_argument('--case', action='append', help='Named rerun; both positive controls always run first')
    args = parser.parse_args()
    if args.target_dir is None:
        parser.error('Supply the coordinated --target-dir or CARGO_TARGET_DIR')
    repo, here = args.repo.resolve(), args.qa_dir.resolve()
    source = repo / 'vendor/vst3-host-0.9.0/src'
    manifest_input, lock_input, target = args.manifest.resolve(), args.lockfile.resolve(), args.target_dir.resolve()
    if here == repo or here.is_relative_to(repo):
        parser.error('--qa-dir must be outside the repository')
    if manifest_input == here / 'Cargo.toml' or lock_input == here / 'Cargo.lock':
        parser.error('Input manifest/lockfile must be separate from generated output')
    inherited_target = os.environ.get('CARGO_TARGET_DIR')
    if inherited_target and Path(inherited_target).resolve() != target:
        parser.error('--target-dir conflicts with inherited CARGO_TARGET_DIR')
    here.mkdir(parents=True, exist_ok=True)
    env, removed_env, overrides = sanitized_environment(target)
    inputs = {'manifest': sha256(manifest_input), 'lockfile': sha256(lock_input),
              'production_manifest': sha256(source.parent / 'Cargo.toml')}
    before = source_hashes(repo, source)
    cases = prepare(here, repo, manifest_input, lock_input)
    if args.case:
        unknown = set(args.case) - {case['name'] for case in cases}
        if unknown:
            parser.error(f'Unknown cases: {sorted(unknown)}')
        cases = [case for case in cases if case['name'] in POSITIVE_CASES or case['name'] in args.case]
    runs = here / 'runs'
    runs.mkdir(exist_ok=True)
    run_id = 1 + max([int(p.name) for p in runs.iterdir() if p.is_dir() and p.name.isdigit()] or [0])
    run_dir = runs / f'{run_id:04d}'
    run_dir.mkdir()
    logs = run_dir / 'logs'
    logs.mkdir()
    for artifact in ('Cargo.toml', 'Cargo.lock', 'lib.test.rs.template',
                     'lib.production.rs.template', 'cases.json', 'source-map.json'):
        shutil.copyfile(here / artifact, run_dir / artifact)
    shutil.copyfile(manifest_input, run_dir / 'input.Cargo.toml')
    shutil.copyfile(source.parent / 'Cargo.toml', run_dir / 'production.Cargo.toml')
    shutil.copyfile(Path(__file__).resolve(), run_dir / Path(__file__).name)
    shutil.copytree(here / 'fixtures', run_dir / 'fixtures')
    write_json(run_dir / 'source-hashes.before.json', before)
    for relative, expected in before.items():
        snapshot = run_dir / 'source-snapshot' / relative
        snapshot.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(repo / relative, snapshot)
        if sha256(snapshot) != expected:
            raise RuntimeError(f'Source changed while archiving: {relative}')
    fixture_hashes = {str(p.relative_to(here)): sha256(p) for p in sorted((here / 'fixtures').glob('*.rs'))}
    command = ['cargo', 'check', '--locked', '--offline', '--manifest-path', str(here / 'Cargo.toml'),
               '--no-default-features', '--features', FEATURES, '--lib', '--message-format=json']
    report = dict(command_base=command, test_target_argument='--tests', target=str(target),
                  run_directory=str(run_dir), source_hashes_before=before, fixture_hashes=fixture_hashes,
                  script_sha256=sha256(Path(__file__).resolve()), selected_cases=[case['name'] for case in cases],
                  input_paths={'manifest': str(manifest_input), 'lockfile': str(lock_input)},
                  input_hashes_before=inputs, removed_build_environment=removed_env,
                  explicit_environment_overrides=overrides,
                  inherited_build_environment={k: v for k, v in env.items() if k in (
                      'CARGO_HOME', 'RUSTUP_HOME', 'RUSTUP_TOOLCHAIN', 'RUSTC',
                      'CARGO_PROFILE_DEV_DEBUG', 'CARGO_PROFILE_TEST_DEBUG', 'CARGO_INCREMENTAL',
                      'CARGO_BUILD_TARGET', 'PKG_CONFIG_PATH', 'LIBRARY_PATH')},
                  compiler=subprocess.check_output([env.get('RUSTC', 'rustc'), '--version', '--verbose'],
                                                   env=env, text=True),
                  cargo=subprocess.check_output(['cargo', '--version', '--verbose'], env=env, text=True),
                  repository_head=subprocess.check_output(['git', '-C', str(repo), 'rev-parse', 'HEAD'],
                                                          text=True).strip(),
                  rust_fixtures_executed=False, complete=False, cases=[])
    write_json(run_dir / 'report.json', report)
    try:
        for spec in cases:
            name = spec['name']
            shutil.copyfile(here / f'lib.{spec["mode"]}.rs.template', here / 'lib.rs')
            shutil.copyfile(here / 'fixtures' / (name + '.rs'), here / 'active_fixture.rs')
            shutil.copyfile(here / 'active_fixture.rs', logs / (name + '.rs'))
            shutil.copyfile(here / 'lib.rs', logs / (name + '.lib.rs'))
            run_command = command + (['--tests'] if spec['mode'] == 'test' else [])
            write_json(logs / (name + '.command.json'), run_command)
            started = time.monotonic()
            report['active_case'] = name
            write_json(run_dir / 'report.json', report)
            # Persist raw output while Cargo runs, including interrupted attempts.
            with (logs / (name + '.jsonl')).open('w', encoding='utf-8') as stdout, \
                    (logs / (name + '.stderr')).open('w', encoding='utf-8') as stderr:
                result = subprocess.run(run_command, env=env, cwd=repo, text=True,
                                        stdout=stdout, stderr=stderr)
            messages = []
            for line in (logs / (name + '.jsonl')).read_text(encoding='utf-8', errors='replace').splitlines():
                try:
                    item = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if isinstance(item, dict):
                    messages.append(item)
            problems, codes, rendered = check_diagnostics(spec, result.returncode, messages)
            (logs / (name + '.diagnostics.txt')).write_text(rendered, encoding='utf-8')
            entry = dict(name=name, mode=spec['mode'], passed=not problems, returncode=result.returncode,
                         command=run_command, shim_sha256=sha256(here / 'lib.rs'),
                         fixture_sha256=sha256(here / 'active_fixture.rs'),
                         codes=sorted(str(code) for code in codes), reason=spec['reason'], problems=problems,
                         seconds=round(time.monotonic() - started, 3))
            report['cases'].append(entry)
            report.pop('active_case', None)
            write_json(run_dir / 'report.json', report)
            print(f'{"PASS" if not problems else "FAIL"} {name} {entry["codes"]}', flush=True)
            if problems:
                print('\n'.join(problems), flush=True)
                print(rendered[-10000:] or (logs / (name + '.stderr')).read_text(
                    encoding='utf-8', errors='replace')[-10000:], flush=True)
                if spec['expect'] == 'pass':
                    break  # No negative result counts if either real-source control failed.
    except BaseException as exc:
        report['interruption'] = f'{type(exc).__name__}: {exc}'
        raise
    finally:
        after = source_hashes(repo, source)
        write_json(run_dir / 'source-hashes.after.json', after)
        inputs_after = {'manifest': sha256(manifest_input), 'lockfile': sha256(lock_input),
                        'production_manifest': sha256(source.parent / 'Cargo.toml')}
        report.update(source_hashes_after=after, stable_production_sources=before == after,
                      input_hashes_after=inputs_after, stable_inputs=inputs == inputs_after,
                      stable_generated_lock=sha256(here / 'Cargo.lock') == inputs['lockfile'],
                      changed_production_sources=sorted(k for k in set(before) | set(after)
                                                        if before.get(k) != after.get(k)),
                      complete=len(report['cases']) == len(cases),
                      passed=sum(case['passed'] for case in report['cases']), total=len(cases))
        write_json(here / 'report.json', report)
        write_json(run_dir / 'report.json', report)
        summary = (f'{report["passed"]}/{report["total"]} cases passed; '
                   f'complete: {report["complete"]}; '
                   f'production source stable: {report["stable_production_sources"]}; '
                   f'manifest/lock stable: {report["stable_inputs"]}; '
                   f'generated lock stable: {report["stable_generated_lock"]}; no Rust fixtures executed')
        print(summary, flush=True)
        (here / 'summary.txt').write_text(summary + '\n', encoding='utf-8')
        shutil.copyfile(here / 'summary.txt', run_dir / 'summary.txt')
    return 0 if (report['complete'] and report['passed'] == report['total']
                 and report['stable_production_sources'] and report['stable_inputs']
                 and report['stable_generated_lock']) else 1


if __name__ == '__main__':
    sys.exit(main())
