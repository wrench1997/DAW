#!/usr/bin/env python3
"""Compile real private VST3 domain-session contracts without editing production code.

Requires Python 3.11+, an explicitly supplied source-linked vendor manifest and its
matching lockfile, and an exclusive/coordinated Cargo-target slot. The caller owns
PATH, CARGO_HOME, RUSTUP_HOME, platform library settings, and build profiles.
All generated fixtures and evidence are written to --qa-dir outside the repository.
No downloads, dependency resolution, public exports, or production test hooks.
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
FORBIDDEN_CODES = {'E0432', 'E0433', 'E0412', 'E0425', 'E0603', 'E0624', 'E0583'}


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def source_hashes(repo, source):
    return {str(p.relative_to(repo)): sha256(p) for p in sorted(source.rglob('*.rs'))}


def prepare(here, repo, manifest_input, lock_input):
    HERE = here
    SOURCE = repo / 'vendor/vst3-host-0.9.0/src'
    FIXTURES = HERE / 'fixtures'
    FIXTURES.mkdir(exist_ok=True)
    COMMON = '''#![allow(dead_code, unused_imports, unused_variables)]
    use crate::audio::{AudioBuffers, BusAudioBuffers};
    use crate::internal::plugin_impl::domain_session::{ControlOps, ProcessorOps, DomainSessionCallback};
    use crate::plugin::{MainThreadPlugin, PluginInternal, ProcessTransport};
    use crate::midi::{MidiChannel, NoteExpressionType, PluginEvent};
    use crate::Result;

    fn stopped_reset_transport() -> ProcessTransport {
        ProcessTransport { sample_position: 0, quarter_note_position: 0.0, tempo: 120.0,
            playing: false, time_sig_numerator: 4, time_sig_denominator: 4 }
    }

    // Every case resolves the exact production types and exercises the real private entry points.
    // This body is type-checked, never run and never substitutes a mock implementation.
    fn positive_same_private_api(owner: &mut MainThreadPlugin, backend: &mut dyn PluginInternal,
        audio: &mut AudioBuffers, buses: &mut BusAudioBuffers, event: &PluginEvent) -> Result<()> {
        let mut observed = 0.0;
        let mut formatted = String::new();
        owner.with_domain_session(&mut |control: &mut ControlOps<'_>, processor: &mut ProcessorOps<'_>| {
            observed = control.get_parameter(0)?;
            formatted = control.format_parameter(0, observed)?;
            let _ = control.resize_editor(320, 240);
            let _ = control.take_editor_resize_request();
            #[cfg(target_os = "linux")]
            control.service_run_loop();
            processor.queue_parameter_at(0, 0.5, 0)?;
            processor.process(audio)?;
            processor.process_buses(buses)?;
            processor.send_plugin_event(event.clone())?;
            let id = processor.note_on(MidiChannel::Ch1, 60, 100, 0)?;
            processor.send_note_expression(id, NoteExpressionType::Volume, 0.5, 0)?;
            processor.note_off(id, 0)?;
            processor.set_process_transport(ProcessTransport { sample_position: 0, quarter_note_position: 0.0, tempo: 120.0, playing: false, time_sig_numerator: 4, time_sig_denominator: 4 })?;
            let _ = processor.take_output_events_with_loss();
            Ok(())
        })?;
        // Owned results survive; exclusive owner/admin authority resumes after callback return.
        let _ = (observed, formatted);
        let state = owner.save_state()?;
        owner.load_state(&state)?;
        owner.reconfigure(48_000.0, 128)?;
        let _ = owner.info();
        // Reset is administrative: available only after the scoped borrow has rejoined.
        let _ = owner.reset_origin_support()?;
        let _ = owner.process_reset_origin(128, stopped_reset_transport())?;
        owner.with_domain_session(&mut |control, processor| {
            processor.queue_parameter_at(0, control.get_parameter(0)?, 0)
        })?;
        let callback: &mut DomainSessionCallback<'_> = &mut |control, processor| {
            processor.queue_parameter_at(0, control.get_parameter(0)?, 0)
        };
        backend.with_domain_session(callback)?;
        Ok(())
    }
    '''

    cases = []
    def case(name, body, codes=(), fragments=(), reason='', required_codes=()):
        (FIXTURES / (name + '.rs')).write_text(COMMON + '\n' + body + '\n')
        cases.append(dict(name=name, expect='fail' if codes else 'pass', allowed_codes=list(codes),
                          required_codes=list(required_codes or codes), fragments=list(fragments), reason=reason))

    case('positive_private_api', '', reason='The real private types, HRTB callback alias, both entry points, every allowed facade operation, and owner operations after return are accessible and type-check.')
    for ty, label in [('ControlOps', 'control'), ('ProcessorOps', 'processor')]:
        for trait, phrase in [('Send', 'sent'), ('Sync', 'shared')]:
            case(f'{label}_not_{trait.lower()}', f'''fn contract() {{
        fn require<T: {trait}>() {{}}
        require::<{ty}<'static>>(); // CONTRACT
    }}''', ['E0277'], ['Rc<()>', phrase, ty], f'{ty} cannot satisfy {trait}; its Rc phantom marker independently blocks the auto trait.')
        case(f'{label}_cannot_escape', f'''fn contract(owner: &mut MainThreadPlugin) -> Result<()> {{
        let mut escaped: Option<&mut {ty}<'_>> = None;
        owner.with_domain_session(&mut |control, processor| {{
            escaped = Some({label}); // CONTRACT
            Ok(())
        }})?;
        std::hint::black_box(escaped);
        Ok(())
    }}''', ['E0521'], ['escapes', label], f'The callback-provided {ty} reference/lifetime cannot escape into captured outer storage.')
        case(f'{label}_cannot_move_out', f'''fn contract(owner: &mut MainThreadPlugin) -> Result<()> {{
        owner.with_domain_session(&mut |control, processor| {{
            let stolen = *{label}; // CONTRACT
            std::hint::black_box(stolen);
            Ok(())
        }})
    }}''', ['E0507'], ['cannot move out', label], f'The caller borrows {ty}; it does not own a movable facade.')
        case(f'{label}_no_double_mut_borrow', f'''fn contract(owner: &mut MainThreadPlugin) -> Result<()> {{
        owner.with_domain_session(&mut |control, processor| {{
            let first = &mut *{label};
            let second = &mut *{label}; // CONTRACT
            std::hint::black_box(first);
            std::hint::black_box(second);
            Ok(())
        }})
    }}''', ['E0499'], ['more than once', label], f'The {ty} borrow remains exclusive within the callback.')
        for method, expression, category in [
            ('save_state', f'{label}.save_state()', 'state save'),
            ('load_state', f'{label}.load_state(&[])', 'state restore'),
            ('reconfigure', f'{label}.reconfigure(48_000.0, 128)', 'runtime reconfiguration'),
            ('reset_origin_support', f'{label}.reset_origin_support()', 'reset preflight'),
            ('process_reset_origin', f'{label}.process_reset_origin(128, stopped_reset_transport())', 'reset transaction'),
            ('start_processing', f'{label}.start_processing()', 'activation'),
            ('stop_processing', f'{label}.stop_processing()', 'deactivation'),
            ('set_bus_arrangements', f'{label}.set_bus_arrangements(&[], &[])', 'bus reconfiguration'),
            ('info', f'{label}.info()', 'metadata'),
            ('get_parameters', f'{label}.get_parameters()', 'metadata enumeration'),
            ('as_ptr', f'{label}.as_ptr()', 'raw COM extraction'),
            ('into_inner', f'{label}.into_inner()', 'owner extraction'),
            ('with_domain_session', f'{label}.with_domain_session(&mut |_, _| Ok(()))', 'nested domain authority'),
        ]:
            case(f'{label}_no_{method}', f'''fn contract(owner: &mut MainThreadPlugin) -> Result<()> {{
        owner.with_domain_session(&mut |control, processor| {{
            let _ = {expression}; // CONTRACT
            Ok(())
        }})
    }}''', ['E0599'], [f'`{method}`', ty], f'{ty} exposes no {category} API ({method}).')
        for trait in ['Deref', 'DerefMut']:
            case(f'{label}_no_{trait.lower()}', f'''fn contract() {{
        fn require<T: std::ops::{trait}>() {{}}
        require::<{ty}<'static>>(); // CONTRACT
    }}''', ['E0277'], [trait, ty], f'{ty} has no {trait} owner escape route.')

    for name, action, codes, fragments in [
        ('save_state', 'let _ = owner.save_state()?;', ['E0502'], ['immutable', 'mutable']),
        ('load_state', 'owner.load_state(&[])?;', ['E0500', 'E0501'], ['closure', 'borrow']),
        ('reconfigure', 'owner.reconfigure(48_000.0, 128)?;', ['E0500', 'E0501'], ['closure', 'borrow']),
        ('metadata', 'let _ = owner.info();', ['E0502'], ['immutable', 'mutable']),
        ('reset_origin_support', 'let _ = owner.reset_origin_support()?;', ['E0502'], ['immutable', 'mutable']),
        ('process_reset_origin', 'let _ = owner.process_reset_origin(128, stopped_reset_transport())?;', ['E0500', 'E0501'], ['closure', 'borrow']),
        ('nested_session', 'owner.with_domain_session(&mut |_, _| Ok(()))?;', ['E0500', 'E0501'], ['closure', 'borrow']),
    ]:
        case(f'owner_cannot_{name}_during_session', f'''fn contract(owner: &mut MainThreadPlugin) -> Result<()> {{
        owner.with_domain_session(&mut |control, processor| {{ // CONTRACT
            {action}
            processor.queue_parameter_at(0, control.get_parameter(0)?, 0)?;
            Ok(())
        }})
    }}''', codes, fragments, f'The owner remains exclusively borrowed: {name} is unavailable while either session capability is live.')
    case('owner_cannot_drop_during_session', '''fn contract(mut owner: MainThreadPlugin) -> Result<()> {
        owner.with_domain_session(&mut |control, processor| { // CONTRACT
            drop(owner);
            processor.queue_parameter_at(0, control.get_parameter(0)?, 0)?;
            Ok(())
        })
    }''', ['E0505', 'E0507'], ['cannot move out', 'owner'], 'An owned MainThreadPlugin cannot be dropped while its domain-session receiver borrow and callback are active.')
    for label, ty, field in [('control', 'ControlOps', 'controller'), ('control', 'ControlOps', 'view'), ('processor', 'ProcessorOps', 'processor'), ('processor', 'ProcessorOps', 'runtime')]:
        case(f'{label}_private_{field}', f'''fn contract(owner: &mut MainThreadPlugin) -> Result<()> {{
        owner.with_domain_session(&mut |control, processor| {{
            let _ = &{label}.{field}; // CONTRACT
            Ok(())
        }})
    }}''', ['E0616'], [f'`{field}`', ty, 'private'], f'The actual {ty} {field} field is private and cannot expose raw COM/runtime state.')

    (HERE / 'cases.json').write_text(json.dumps(cases, indent=2) + '\n')
    manifest = manifest_input.read_text()
    parsed = tomllib.loads(manifest)
    production_manifest = tomllib.loads((SOURCE.parent / 'Cargo.toml').read_text())
    actual_lib = (manifest_input.parent / parsed['lib']['path']).resolve()
    if actual_lib != (SOURCE / 'lib.rs').resolve():
        raise ValueError('The input manifest must source-link the selected production lib.rs')
    for key in ('features', 'dependencies', 'target'):
        if parsed.get(key, {}) != production_manifest.get(key, {}):
            raise ValueError(f'The input manifest differs from production {key}')
    if any(key in parsed for key in ('dev-dependencies', 'bin', 'test', 'bench', 'example')):
        raise ValueError('Supply the approved runtime-only source-linked vendor manifest')
    # Change only the selected library path; preserve dependency versions and features.
    def replace_lib(match):
        body, count = re.subn(r'(?m)^path = .*$', 'path = "lib.rs"', match.group(2))
        if count != 1:
            raise ValueError('Expected one library source path')
        return match.group(1) + body
    manifest, count = re.subn(r'(?ms)(^\[lib\]\n)(.*?)(?=^\[|\Z)', replace_lib, manifest)
    if count != 1:
        raise ValueError('Expected one library target')
    (HERE / 'Cargo.toml').write_text(manifest)
    shutil.copyfile(lock_input, HERE / 'Cargo.lock')
    lib = (SOURCE / 'lib.rs').read_text()
    modules = []
    def rebase(match):
        visibility, name = match.group(1) or '', match.group(2)
        matches = [p for p in [SOURCE / f'{name}.rs', SOURCE / name / 'mod.rs'] if p.is_file()]
        assert len(matches) == 1, (name, matches)
        modules.append(str(matches[0]))
        return f'#[path = {json.dumps(str(matches[0]))}]\n{visibility}mod {name};'
    shim, count = re.subn(r'(?m)^(pub )?mod (\w+);$', rebase, lib)
    assert count == 17, f'Unexpected top-level module layout: {count}'
    (HERE / 'lib.rs.template').write_text(shim + '\n\n// Crate-internal QA fixture, never exported.\n#[path = "active_fixture.rs"]\nmod private_domain_contract;\n')
    (HERE / 'source-map.json').write_text(json.dumps({'production_root': str(SOURCE), 'modules': modules}, indent=2) + '\n')


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
    parser.add_argument('--case', action='append', help='Named diagnostic rerun; positive always runs first')
    args = parser.parse_args()
    if args.target_dir is None:
        parser.error('Supply the coordinated --target-dir or CARGO_TARGET_DIR')
    repo, here = args.repo.resolve(), args.qa_dir.resolve()
    source = repo / 'vendor/vst3-host-0.9.0/src'
    manifest_input, lock_input = args.manifest.resolve(), args.lockfile.resolve()
    target = args.target_dir.resolve()
    if here == repo or here.is_relative_to(repo):
        parser.error('--qa-dir must be outside the repository')
    if manifest_input == here / 'Cargo.toml' or lock_input == here / 'Cargo.lock':
        parser.error('Input manifest/lockfile must be separate from generated output')
    inherited_target = os.environ.get('CARGO_TARGET_DIR')
    if inherited_target and Path(inherited_target).resolve() != target:
        parser.error('--target-dir conflicts with inherited CARGO_TARGET_DIR')
    here.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    env['CARGO_TARGET_DIR'] = str(target)
    inputs = {'manifest': sha256(manifest_input), 'lockfile': sha256(lock_input),
              'production_manifest': sha256(source.parent / 'Cargo.toml')}
    before = source_hashes(repo, source)
    prepare(here, repo, manifest_input, lock_input)
    cases = json.loads((here / 'cases.json').read_text())
    if args.case:
        unknown = set(args.case) - {c['name'] for c in cases}
        if unknown:
            parser.error(f'Unknown cases: {sorted(unknown)}')
        cases = [c for c in cases if c['name'] == 'positive_private_api' or c['name'] in args.case]
    runs = here / 'runs'
    runs.mkdir(exist_ok=True)
    run_id = 1 + max([int(p.name) for p in runs.iterdir() if p.is_dir() and p.name.isdigit()] or [0])
    run_dir = runs / f'{run_id:04d}'
    run_dir.mkdir()
    logs = run_dir / 'logs'
    logs.mkdir()
    for artifact in ('Cargo.toml', 'Cargo.lock', 'lib.rs.template', 'cases.json', 'source-map.json'):
        shutil.copyfile(here / artifact, run_dir / artifact)
    fixture_hashes = {str(p.relative_to(here)): sha256(p) for p in sorted((here / 'fixtures').glob('*.rs'))}
    shutil.copyfile(here / 'lib.rs.template', here / 'lib.rs')
    command = ['cargo', 'check', '--locked', '--offline', '--manifest-path', str(here / 'Cargo.toml'),
               '--no-default-features', '--features', FEATURES, '--lib', '--message-format=json']
    report = dict(command=command, target=str(target), run_directory=str(run_dir),
                  source_hashes_before=before, fixture_hashes=fixture_hashes,
                  shim_sha256=sha256(here / 'lib.rs'), script_sha256=sha256(Path(__file__).resolve()),
                  input_paths={'manifest': str(manifest_input), 'lockfile': str(lock_input)},
                  input_hashes_before=inputs,
                  inherited_build_environment={k: v for k, v in env.items() if k in (
                      'CARGO_HOME', 'RUSTUP_HOME', 'RUSTUP_TOOLCHAIN', 'RUSTC', 'RUSTC_WRAPPER',
                      'RUSTC_WORKSPACE_WRAPPER', 'CARGO_PROFILE_DEV_DEBUG', 'CARGO_PROFILE_TEST_DEBUG',
                      'CARGO_INCREMENTAL', 'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'CARGO_BUILD_TARGET',
                      'PKG_CONFIG_PATH', 'LIBRARY_PATH')},
                  compiler=subprocess.check_output([env.get('RUSTC', 'rustc'), '--version', '--verbose'],
                                                   env=env, text=True),
                  repository_head=subprocess.check_output(['git', '-C', str(repo), 'rev-parse', 'HEAD'],
                                                          text=True).strip(), cases=[])
    for spec in cases:
        name = spec['name']
        shutil.copyfile(here / 'fixtures' / (name + '.rs'), here / 'active_fixture.rs')
        shutil.copyfile(here / 'active_fixture.rs', logs / (name + '.rs'))
        started = time.monotonic()
        result = subprocess.run(command, env=env, cwd=repo, text=True,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        (logs / (name + '.jsonl')).write_text(result.stdout)
        (logs / (name + '.stderr')).write_text(result.stderr)
        diagnostics = []
        for line in result.stdout.splitlines():
            try:
                item = json.loads(line)
            except json.JSONDecodeError:
                continue
            if item.get('reason') == 'compiler-message' and item['message']['level'] == 'error':
                diagnostics.append(item['message'])
        rendered = '\n'.join(d.get('rendered') or d.get('message', '') for d in diagnostics)
        (logs / (name + '.diagnostics.txt')).write_text(rendered)
        codes = {(d.get('code') or {}).get('code') for d in diagnostics}
        problems = []
        if spec['expect'] == 'pass':
            if result.returncode != 0 or diagnostics:
                problems.append('positive production-private API fixture did not compile')
        else:
            if result.returncode == 0:
                problems.append('negative fixture unexpectedly compiled')
            if not diagnostics:
                problems.append('failure contained no rustc error diagnostics')
            if codes & FORBIDDEN_CODES:
                problems.append(f'import/privacy-of-entry failure: {sorted(codes & FORBIDDEN_CODES)}')
            if codes - set(spec['allowed_codes']):
                problems.append(f'unexpected codes: {sorted(str(c) for c in codes - set(spec["allowed_codes"]))}')
            if set(spec['required_codes']) - codes:
                problems.append(f'missing required codes: {sorted(set(spec["required_codes"]) - codes)}')
            if any(not any(s.get('is_primary') and Path(s.get('file_name', '')).name == 'active_fixture.rs'
                           for s in d.get('spans', [])) for d in diagnostics):
                problems.append('error primary span is not in the injected contract fixture')
            for fragment in spec['fragments']:
                if fragment not in rendered:
                    problems.append(f'missing diagnostic text: {fragment!r}')
        entry = dict(name=name, passed=not problems, returncode=result.returncode,
                     codes=sorted(str(c) for c in codes), reason=spec['reason'], problems=problems,
                     seconds=round(time.monotonic() - started, 3))
        report['cases'].append(entry)
        print(f'{"PASS" if not problems else "FAIL"} {name} {entry["codes"]}', flush=True)
        if problems:
            print('\n'.join(problems), flush=True)
            print(rendered[-10000:] or result.stderr[-10000:], flush=True)
            if spec['expect'] == 'pass':
                break  # No negative result counts if the actual private API is unreachable.
    after = source_hashes(repo, source)
    inputs_after = {'manifest': sha256(manifest_input), 'lockfile': sha256(lock_input),
                    'production_manifest': sha256(source.parent / 'Cargo.toml')}
    report.update(source_hashes_after=after, stable_production_sources=before == after,
                  input_hashes_after=inputs_after, stable_inputs=inputs == inputs_after,
                  changed_production_sources=sorted(k for k in set(before) | set(after) if before.get(k) != after.get(k)),
                  complete=len(report['cases']) == len(cases),
                  passed=sum(x['passed'] for x in report['cases']), total=len(cases))
    (here / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    shutil.copyfile(here / 'report.json', run_dir / 'report.json')
    summary = (f'{report["passed"]}/{report["total"]} cases passed; '
               f'production source stable: {report["stable_production_sources"]}; '
               f'manifest/lock stable: {report["stable_inputs"]}')
    print(summary, flush=True)
    (here / 'summary.txt').write_text(summary + '\n')
    shutil.copyfile(here / 'summary.txt', run_dir / 'summary.txt')
    return 0 if (report['passed'] == report['total'] and report['stable_production_sources'] and report['stable_inputs']) else 1


if __name__ == '__main__':
    sys.exit(main())
