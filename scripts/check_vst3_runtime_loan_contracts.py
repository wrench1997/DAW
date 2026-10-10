#!/usr/bin/env python3
"""Compile the real cfg(test) full-runtime loan capability contracts, never run them.

Requires Python 3.11+, an approved runtime-only source-linked vendor manifest and
its matching lockfile, and an exclusive/coordinated Cargo target. The caller owns
PATH, CARGO_HOME, RUSTUP_HOME, platform libraries, and build profiles. Run only
after the exact unsafe source has been reviewed and durably archived.

The generated crate root source-links the actual private vendor modules. Cargo
check --lib --tests enables their existing cfg(test) items without a new feature,
global --cfg test, dependency changes, or production-file edits. Separate --lib
checks establish production reachability and the test-only API's absence. No Rust
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
POSITIVE_CASES = {'positive_scoped_runtime_api', 'positive_production_domain_api',
                  'positive_process_only_capability'}
ABSENCE_CASES = {'production_runtime_loan_absent', 'production_runtime_entry_absent'}
FORBIDDEN_CODES = {'E0432', 'E0433', 'E0412', 'E0425', 'E0603', 'E0624', 'E0583'}

PRODUCTION_COMMON = '''#![allow(dead_code, unused_imports, unused_variables)]
#![deny(unsafe_code)]
use crate::audio::{AudioBuffers, AudioBusConfig, AudioBusLayout, BusAudioBuffers};
use crate::internal::plugin_impl::PluginImpl;
use crate::internal::plugin_impl::domain_session::{ControlOps, ProcessorOps};
use crate::internal::processor_lease::{ProcessorCalls, ProcessorProcessCall};
use crate::plugin::{PluginInternal, ProcessMode, ProcessTransport};
use crate::midi::{MidiChannel, NoteExpressionType, PluginEvent};
use crate::Result;
use vst3::Steinberg::{kResultOk, tresult, Vst::{IAudioProcessor, ProcessData}};

struct ProcessOnly;

// The sole exception to this fixture's unsafe-code lint is the trait's required
// method declaration. This mock never dereferences data or calls a COM method.
#[allow(unsafe_code)]
impl ProcessorProcessCall for ProcessOnly {
    unsafe fn process(&mut self, _data: &mut ProcessData) -> tresult { kResultOk }
}

fn stopped_transport() -> ProcessTransport {
    ProcessTransport {
        sample_position: 0, quarter_note_position: 0.0, tempo: 120.0,
        playing: false, time_sig_numerator: 4, time_sig_denominator: 4,
    }
}

// Ordinary, never-called functions. Source-linked production types and every
// allowed domain operation must resolve before any negative result can count.
fn real_production_domains_are_reachable(owner: &mut PluginImpl,
    audio: &mut AudioBuffers, buses: &mut BusAudioBuffers) -> Result<()> {
    owner.with_domain_session(&mut |control, processor| {
        let observed = control.get_parameter(0)?;
        let _ = control.format_parameter(0, observed)?;
        let _ = control.resize_editor(320, 240);
        let _ = control.take_editor_resize_request();
        #[cfg(target_os = "linux")]
        control.service_run_loop();
        processor.queue_parameter_at(0, 0.5, 0)?;
        processor.send_plugin_event(PluginEvent::sysex(vec![0xf0, 0xf7]))?;
        let id = processor.note_on(MidiChannel::Ch1, 60, 100, 0)?;
        processor.send_note_expression(id, NoteExpressionType::Volume, 0.5, 0)?;
        processor.note_off(id, 0)?;
        processor.set_process_transport(stopped_transport())?;
        processor.process(audio)?;
        processor.process_buses(buses)?;
        let _: (Vec<PluginEvent>, bool) = processor.take_output_events_with_loss();
        Ok(())
    })?;
    owner.stop_processing()?;
    owner.start_processing()?;
    let state = owner.save_state()?;
    owner.load_state(&state)?;
    owner.reconfigure(48_000.0, 128)?;
    let _ = owner.reset_origin_support()?;
    let _ = owner.process_reset_origin(128, stopped_transport())?;
    Ok(())
}
'''

COMMON = PRODUCTION_COMMON + '''
use crate::internal::plugin_impl::domain_session::{
    with_runtime_loan_for_test, RuntimeLoan,
};

// Only safe, fully owned high-level audio/event inputs. The fixture is compiled
// but never executed; it neither loads a plugin nor invokes the processing SDK.
fn positive_same_private_api(owner: &mut PluginImpl) -> Result<()> {
    fn require_send<T: Send>() {}
    require_send::<RuntimeLoan<'_>>();
    let output = with_runtime_loan_for_test(owner,
        |mut control: ControlOps<'_>, mut loan: RuntimeLoan<'_>| {
        std::thread::scope(|scope| {
            let worker = scope.spawn(move || {
                let mut audio = AudioBuffers::new(2, 2, 128, 48_000.0);
                let layout = AudioBusLayout {
                    inputs: vec![AudioBusConfig { channel_count: 2, active: true }],
                    outputs: vec![AudioBusConfig { channel_count: 2, active: true }],
                };
                let mut buses = BusAudioBuffers::new(&layout, 128, 48_000.0);
                loan.queue_parameter_at(0, 0.5, 0).expect("compile-only queue");
                loan.send_plugin_event(PluginEvent::sysex(vec![0xf0, 0xf7]))
                    .expect("compile-only event");
                let id = loan.note_on(MidiChannel::Ch1, 60, 100, 0)
                    .expect("compile-only note");
                loan.send_note_expression(id, NoteExpressionType::Volume, 0.5, 0)
                    .expect("compile-only expression");
                loan.note_off(id, 0).expect("compile-only release");
                loan.set_process_transport(stopped_transport()).expect("compile-only transport");
                loan.process(&mut audio).expect("compile-only flat processing");
                loan.process_buses(&mut buses).expect("compile-only bus processing");
                loan.take_output_events_with_loss()
            });
            // Control stays on the calling thread while the worker owns the loan.
            let value = control.get_parameter(0)?;
            let _ = control.format_parameter(0, value)?;
            let _ = control.resize_editor(320, 240);
            let _ = control.take_editor_resize_request();
            #[cfg(target_os = "linux")]
            control.service_run_loop();
            Ok(worker.join().expect("compile-only worker"))
        })
    })?;
    // Only owned results escape. After join AND callback return the owner can be
    // reused, reconfigured, reset and lent again, still with no unsafe call site.
    let _: (Vec<PluginEvent>, bool) = output;
    owner.stop_processing()?;
    owner.start_processing()?;
    let state = owner.save_state()?;
    owner.load_state(&state)?;
    owner.reconfigure(48_000.0, 128)?;
    let _ = owner.reset_origin_support()?;
    let _ = owner.process_reset_origin(128, stopped_transport())?;
    with_runtime_loan_for_test(owner, |control, mut loan| {
        loan.queue_parameter_at(0, control.get_parameter(0)?, 0)
    })?;
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

    case('positive_scoped_runtime_api', reason=(
        'Real RuntimeLoan is Send and processes safe owned audio/event inputs on a scoped '
        'worker while ControlOps remains on the caller; owned results and later owner '
        'administration, reset and sequential loans compile after join.'))
    case('positive_production_domain_api', mode='production', reason=(
        'Actual private owner and every existing domain operation compile without cfg(test).'))
    case('positive_process_only_capability', '''fn contract() {
    fn require<T: ProcessorProcessCall>() {}
    require::<ProcessOnly>();
}''', mode='production', reason=(
        'An implementation of the actual process-only trait satisfies the kernel '
        'capability without implementing set_processing or any lifecycle trait.'))
    for name, symbol in [('production_runtime_loan_absent', 'RuntimeLoan'),
                         ('production_runtime_entry_absent', 'with_runtime_loan_for_test')]:
        case(name, f'''use crate::internal::plugin_impl::domain_session::{symbol}; // CONTRACT
''', ['E0432'], [symbol, 'configured out'], mode='production',
             reason=f'The test-only {symbol} is absent from a normal library build.')

    case('process_capability_has_no_lifecycle', '''fn contract<P: ProcessorProcessCall>(processor: &mut P) {
    let _ = processor.set_processing(0); // CONTRACT
}''', ['E0599'], ['`set_processing`', 'ProcessorCalls'], mode='production',
         reason='The real process-only trait bound grants no set_processing lifecycle operation.')
    case('lifecycle_capability_has_no_process', '''fn contract<P: ProcessorCalls>(processor: &mut P, data: &mut ProcessData) {
    let _ = processor.process(data); // CONTRACT
}''', ['E0599'], ['`process`', 'ProcessorProcessCall'], mode='production',
         reason='The real lifecycle-only trait bound grants no SDK process operation.')

    for ty, label, traits, mode in [
        ('RuntimeLoan', 'loan', ('Sync', 'Clone'), 'test'),
        ('ControlOps', 'control', ('Send', 'Sync'), 'test'),
        ('ProcessorOps', 'production_processor', ('Send', 'Sync'), 'production'),
        ('ControlOps', 'production_control', ('Send', 'Sync'), 'production'),
    ]:
        for trait in traits:
            case(f'{label}_not_{trait.lower()}', f'''fn contract() {{
    fn require<T: {trait}>() {{}}
    require::<{ty}<'static>>(); // CONTRACT
}}''', ['E0277'], [trait, ty], f'The real {ty} does not implement {trait}.', mode=mode)

    methods = [
        ('clone', 'loan.clone()', 'cloning'),
        ('info', 'loan.info()', 'metadata'),
        ('get_parameters', 'loan.get_parameters()', 'parameter metadata'),
        ('get_parameter', 'loan.get_parameter(0)', 'controller reads'),
        ('format_parameter', 'loan.format_parameter(0, 0.5)', 'controller formatting'),
        ('set_parameter', 'loan.set_parameter(0, 0.5)', 'mixed controller/DSP writes'),
        ('resize_editor', 'loan.resize_editor(320, 240)', 'editor access'),
        ('get_latency_samples', 'loan.get_latency_samples()', 'latency metadata'),
        ('get_tail_samples', 'loan.get_tail_samples()', 'tail metadata'),
        ('audio_bus_layout', 'loan.audio_bus_layout()', 'bus metadata'),
        ('save_state', 'loan.save_state()', 'state save'),
        ('load_state', 'loan.load_state(&[])', 'state restore'),
        ('reconfigure', 'loan.reconfigure(48_000.0, 128)', 'reconfiguration'),
        ('set_process_mode', 'loan.set_process_mode(ProcessMode::Realtime)', 'processing setup'),
        ('setup_processing', 'loan.setup_processing(48_000.0, 128)', 'processing setup'),
        ('start_processing', 'loan.start_processing()', 'activation'),
        ('stop_processing', 'loan.stop_processing()', 'deactivation'),
        ('set_bus_arrangements', 'loan.set_bus_arrangements(&[], &[])', 'bus changes'),
        ('set_bus_active', 'loan.set_bus_active(true, 0, false)', 'bus activation'),
        ('set_active', 'loan.set_active(false)', 'component lifecycle'),
        ('terminate', 'loan.terminate()', 'component teardown'),
        ('reset_origin_support', 'loan.reset_origin_support()', 'reset preflight'),
        ('process_reset_origin', 'loan.process_reset_origin(128, stopped_transport())', 'reset transaction'),
        ('with_domain_session', 'loan.with_domain_session(&mut |_, _| Ok(()))', 'nested owner authority'),
        ('as_ptr', 'loan.as_ptr()', 'raw pointer extraction'),
        ('as_raw', 'loan.as_raw()', 'raw pointer extraction'),
        ('into_raw', 'loan.into_raw()', 'raw pointer extraction'),
        ('into_inner', 'loan.into_inner()', 'runtime/COM extraction'),
        ('inner', 'loan.inner()', 'runtime capability extraction'),
        ('runtime', 'loan.runtime()', 'runtime extraction'),
        ('cast', 'loan.cast::<IAudioProcessor>()', 'COM cast'),
        ('query_interface', 'loan.query_interface()', 'COM query'),
        ('add_ref', 'loan.add_ref()', 'COM reference counting'),
        ('release', 'loan.release()', 'COM reference counting'),
        ('processor', 'loan.processor()', 'processor COM getter'),
        ('controller', 'loan.controller()', 'controller COM getter'),
        ('component', 'loan.component()', 'component COM getter'),
    ]
    for method, expression, category in methods:
        case('loan_no_' + method, f'''fn contract(owner: &mut PluginImpl) -> Result<()> {{
    with_runtime_loan_for_test(owner, |_, mut loan| {{
        let _ = {expression}; // CONTRACT
        Ok(())
    }})
}}''', ['E0599'], [f'`{method}`', 'RuntimeLoan'],
             f'The actual loan exposes no {category} method ({method}).')

    for trait in ('Deref', 'DerefMut'):
        case('loan_no_' + trait.lower(), f'''fn contract() {{
    fn require<T: std::ops::{trait}>() {{}}
    require::<RuntimeLoan<'static>>(); // CONTRACT
}}''', ['E0277'], [trait, 'RuntimeLoan'], f'No {trait} escape to runtime, owner or COM.')
    for trait, path in [('AsRef', 'std::convert::AsRef'), ('Borrow', 'std::borrow::Borrow')]:
        for ty, label in [('IAudioProcessor', 'processor_com'), ('PluginImpl', 'owner'),
                          ("ProcessorOps<'static>", 'processor_ops')]:
            case(f'loan_no_{trait.lower()}_{label}', f'''fn contract() {{
    fn require<T: {path}<{ty}>>() {{}}
    require::<RuntimeLoan<'static>>(); // CONTRACT
}}''', ['E0277'], [trait, ty.split('<')[0], 'RuntimeLoan'],
                 f'No {trait} extraction of {ty}.')
    case('loan_private_inner', '''fn contract(owner: &mut PluginImpl) -> Result<()> {
    with_runtime_loan_for_test(owner, |_, loan| {
        let _ = &loan.inner; // CONTRACT
        Ok(())
    })
}''', ['E0616'], ['`inner`', 'RuntimeLoan', 'private'],
         'The actual inner field cannot expose runtime, gate or raw processor capability.')
    case('production_processor_private_core', '''fn contract(processor: &mut ProcessorOps<'_>) {
    let _ = &processor.core; // CONTRACT
}''', ['E0616'], ['`core`', 'ProcessorOps', 'private'], mode='production',
         reason='The actual nongeneric production facade keeps its prepared kernel core private.')
    for field in ('runtime', 'processor', 'gate'):
        case('production_processor_no_' + field + '_field', f'''fn contract(processor: &mut ProcessorOps<'_>) {{
    let _ = &processor.{field}; // CONTRACT
}}''', ['E0609'], [f'`{field}`', 'ProcessorOps', 'no field'], mode='production',
             reason=f'The former {field} backing route is absent from the nongeneric production facade.')
    case('loan_no_constructor', '''fn contract() {
    let _ = RuntimeLoan::new(); // CONTRACT
}''', ['E0599'], ['`new`', 'RuntimeLoan'],
         'No standalone constructor bypasses the preflighted callback entry.')

    # These are actual owner operations at an overlapping call, rather than
    # an inaccessible entry point or a hand-written substitute lifetime type.
    for name, action, codes, fragments in [
        ('save_state', 'let _ = owner.save_state()?;', ['E0502'], ['immutable', 'mutable']),
        ('load_state', 'owner.load_state(&[])?;', ['E0500', 'E0501'], ['closure', 'borrow']),
        ('stop_processing', 'owner.stop_processing()?;', ['E0500', 'E0501'], ['closure', 'borrow']),
        ('start_processing', 'owner.start_processing()?;', ['E0500', 'E0501'], ['closure', 'borrow']),
        ('reconfigure', 'owner.reconfigure(48_000.0, 128)?;', ['E0500', 'E0501'], ['closure', 'borrow']),
        ('reset_origin_support', 'let _ = owner.reset_origin_support()?;', ['E0502'], ['immutable', 'mutable']),
        ('process_reset_origin', 'let _ = owner.process_reset_origin(128, stopped_transport())?;', ['E0500', 'E0501'], ['closure', 'borrow']),
        ('second_loan', 'with_runtime_loan_for_test(owner, |_, _| Ok(()))?;', ['E0500', 'E0501'], ['closure', 'borrow']),
        ('production_session', 'owner.with_domain_session(&mut |_, _| Ok(()))?;', ['E0500', 'E0501'], ['closure', 'borrow']),
    ]:
        case('owner_cannot_' + name + '_during_loan', f'''fn contract(owner: &mut PluginImpl) -> Result<()> {{
    with_runtime_loan_for_test(owner, |control, mut loan| {{ // CONTRACT
        {action} // CONTRACT
        loan.queue_parameter_at(0, control.get_parameter(0)?, 0)?;
        Ok(())
    }})
}}''', codes, fragments,
             f'The aggregate remains exclusively borrowed: {name} cannot overlap a runtime loan.')
    case('owner_cannot_drop_during_loan', '''fn contract(mut owner: PluginImpl) -> Result<()> {
    with_runtime_loan_for_test(&mut owner, |_, mut loan| { // CONTRACT
        drop(owner); // CONTRACT
        loan.queue_parameter_at(0, 0.5, 0)?;
        Ok(())
    })
}''', ['E0505'], ['cannot move out', 'owner', 'borrowed'],
         'The owner and its retained module cannot be dropped during the callback loan.')

    for label, ty in [('loan', 'RuntimeLoan'), ('control', 'ControlOps')]:
        case(label + '_cannot_escape_outer_storage', f'''fn contract(owner: &mut PluginImpl) -> Result<()> {{
    let mut escaped: Option<{ty}<'_>> = None;
    with_runtime_loan_for_test(owner, |control, loan| {{
        escaped = Some({label}); // CONTRACT
        Ok(())
    }})?;
    std::hint::black_box(escaped);
    Ok(())
}}''', ['E0521'], ['escapes', label],
             f'Higher-ranked callback lifetime prevents {ty} escaping into captured outer storage.')
        case(label + '_cannot_return_from_callback', f'''fn contract(owner: &mut PluginImpl) -> Result<{ty}<'_>> {{
    with_runtime_loan_for_test(owner, |control, loan| Ok({label})) // CONTRACT
}}''', [None], ['lifetime may not live long enough'],
             f'The callback cannot return {ty}; R is independent of its fresh loan lifetime.')
    case('loan_cannot_detach_thread', '''fn contract(owner: &mut PluginImpl) -> Result<()> {
    with_runtime_loan_for_test(owner, |_, mut loan| {
        std::thread::spawn(move || { // CONTRACT
            let mut audio = AudioBuffers::new(2, 2, 128, 48_000.0);
            let _ = loan.process(&mut audio);
        });
        Ok(())
    })
}''', ['E0521'], ['escapes', "'static", 'loan'],
         'Send does not allow a borrowed runtime to enter a detached static worker.')
    case('loan_cannot_enter_outer_thread_scope', '''fn contract(owner: &mut PluginImpl) -> Result<()> {
    std::thread::scope(|scope| {
        with_runtime_loan_for_test(owner, |_, loan| {
            scope.spawn(move || { std::hint::black_box(loan); }); // CONTRACT
            Ok(())
        })
    })
}''', ['E0521'], ['escapes', 'loan'],
         'A worker scope outside the callback cannot extend the runtime loan lifetime.')
    case('loan_shared_reference_cannot_cross_thread', '''fn contract(owner: &mut PluginImpl) -> Result<()> {
    with_runtime_loan_for_test(owner, |_, loan| {
        std::thread::scope(|scope| {
            let shared = &loan;
            scope.spawn(move || { std::hint::black_box(shared); }); // CONTRACT
        });
        Ok(())
    })
}''', ['E0277'], ['RuntimeLoan', 'Sync', 'shared'],
         'Send permits ownership transfer, but !Sync prevents transferring shared loan references.')
    case('control_cannot_move_to_scoped_worker', '''fn contract(owner: &mut PluginImpl) -> Result<()> {
    with_runtime_loan_for_test(owner, |control, _| {
        std::thread::scope(|scope| {
            scope.spawn(move || { std::hint::black_box(control); }); // CONTRACT
        });
        Ok(())
    })
}''', ['E0277'], ['ControlOps', 'Send', 'sent'],
         'The real control capability cannot move to even a joined worker.')
    case('control_shared_reference_cannot_cross_thread', '''fn contract(owner: &mut PluginImpl) -> Result<()> {
    with_runtime_loan_for_test(owner, |control, _| {
        std::thread::scope(|scope| {
            let shared = &control;
            scope.spawn(move || { std::hint::black_box(shared); }); // CONTRACT
        });
        Ok(())
    })
}''', ['E0277'], ['ControlOps', 'Sync', 'shared'],
         'A shared reference cannot transfer controller/editor authority across threads.')
    for method, ty in [('process', 'AudioBuffers'), ('process_buses', 'BusAudioBuffers')]:
        case('loan_' + method + '_requires_mutable_receiver', f'''fn contract(loan: &RuntimeLoan<'_>, audio: &mut {ty}) {{
    let _ = loan.{method}(audio); // CONTRACT
}}''', ['E0596'], ['loan', 'mutable'],
             f'{method} requires an exclusive receiver even though the method itself is safe.')
    case('loan_no_double_mutable_use', '''fn contract(owner: &mut PluginImpl) -> Result<()> {
    with_runtime_loan_for_test(owner, |_, mut loan| {
        let first = &mut loan;
        let second = &mut loan; // CONTRACT
        std::hint::black_box(first);
        std::hint::black_box(second);
        Ok(())
    })
}''', ['E0499'], ['loan', 'more than once'],
         'A loan cannot yield two simultaneous exclusive processing receivers.')

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
        if name == 'internal':
            return '__PRIVATE_RUNTIME_CONTRACT_INTERNAL__'
        return f'#[path = {json.dumps(str(matches[0]))}]\n{visibility}mod {name};'

    shim, count = re.subn(r'(?m)^(pub )?mod (\w+);$', rebase, lib)
    if count != 17:
        raise ValueError(f'Unexpected top-level module layout: {count}')
    # Anchor an inline internal module at the original directory and preserve
    # its declarations verbatim. An absolute #[path] on plugin_impl.rs would
    # change its children's lookup directory and cause E0583 before contracts
    # could type-check. The inline anchor retains ordinary nested resolution.
    # This also places the two pub(super) kernel traits in the fixture's real
    # visibility scope, without public exports or an accepted E0603 shortcut.
    internal_root = source / 'internal/mod.rs'
    internal_text = internal_root.read_text(encoding='utf-8')
    internal_modules = []
    declarations = re.findall(r'(?m)^(pub(?:\([^)]*\))? )?mod (\w+);$', internal_text)
    if len(declarations) != 10:
        raise ValueError(f'Unexpected internal module layout: {len(declarations)}')
    for _, name in declarations:
        matches = [p for p in (source / 'internal' / f'{name}.rs',
                              source / 'internal' / name / 'mod.rs') if p.is_file()]
        if len(matches) != 1:
            raise ValueError(f'Unexpected internal source layout for {name}: {matches}')
        internal_modules.append(str(matches[0]))
    if shim.count('__PRIVATE_RUNTIME_CONTRACT_INTERNAL__') != 1:
        raise ValueError('Expected exactly one internal module anchor')
    for mode, gate in [('test', '#[cfg(test)]\n'), ('production', '')]:
        text = f'#[path = {json.dumps(str(source / "internal"))}]\nmod internal {{\n'
        text += internal_text + '\n// Internal-sibling compile-only fixture; never exported.\n'
        text += gate + f'#[path = {json.dumps(str(here / "active_fixture.rs"))}]\n'
        text += 'mod private_runtime_loan_contract;\n}\n'
        (here / f'lib.{mode}.rs.template').write_text(
            shim.replace('__PRIVATE_RUNTIME_CONTRACT_INTERNAL__', text), encoding='utf-8')
    write_json(here / 'source-map.json', dict(production_root=str(source), modules=modules,
               internal_root=str(internal_root), internal_modules=internal_modules,
               runtime_loan_source=str(source / 'internal/plugin_impl/domain_session.rs'),
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
        production_artifact = any(item.get('reason') == 'compiler-artifact'
                                  and item.get('target', {}).get('name') == 'vst3_host'
                                  and item.get('profile', {}).get('test') is False
                                  for item in messages)
        if spec['mode'] == 'production' and not production_artifact:
            problems.append('Cargo did not emit evidence of the production library target')
        if spec['mode'] == 'production' and test_artifact:
            problems.append('A production-only control unexpectedly compiled a test target')
    else:
        if resultcode == 0:
            problems.append('negative fixture unexpectedly compiled')
        if not diagnostics:
            problems.append('failure contained no rustc error diagnostics')
        forbidden = FORBIDDEN_CODES - ({'E0432'} if spec['name'] in ABSENCE_CASES else set())
        if codes & forbidden:
            problems.append(f'import/privacy-of-entry failure: {sorted(codes & forbidden)}')
        unexpected = codes - set(spec['allowed_codes'])
        if unexpected:
            problems.append(f'unexpected codes: {sorted(str(code) for code in unexpected)}')
        missing = set(spec['required_codes']) - codes
        if missing:
            problems.append(f'missing required codes: {sorted(str(code) for code in missing)}')
        for diagnostic in diagnostics:
            # rustc does not number this HRTB return-lifetime diagnostic. Do not
            # let an unrelated unnumbered error satisfy an escape contract.
            if ((diagnostic.get('code') or {}).get('code') is None
                    and diagnostic.get('message') != 'lifetime may not live long enough'):
                problems.append('unexpected unnumbered diagnostic')
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
    parser.add_argument('--case', action='append', help='Named rerun; all three positive controls always run first')
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
                    break  # No negative counts if any real-source positive control failed.
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
