import pathlib,shutil,re,subprocess,json,hashlib,os
root=pathlib.Path(__file__).resolve().parent;source=pathlib.Path(os.environ.get('DAW_SOURCE','<DAW_SOURCE>'));out=root/'source'
if out.exists():shutil.rmtree(out)
shutil.copytree(source,out,ignore=shutil.ignore_patterns('.git','target','.codex','.agents'))
s=(source/'src/audio_midi_routing_tests.rs').read_text();start=s.index('struct MidiGraphFixture {');end=s.index('\nfn observed_notes',start);code=s[start:end].replace('MidiGraphFixture','NativeGraphFixture')
a=code.index('    fn new(');b=code.index('        let timeline =',a)
code=code.replace('    loop_token: u64,','    loop_token: u64,\n    sink_audio: Vec<[f32;2]>,\n    check_source_output: bool,\n    callback_max_runtime_us: u128,')
code=code.replace('            loop_token,','            loop_token,\n            sink_audio: Vec::new(),\n            check_source_output: project.plugin_instances.iter().any(|p|p.midi_ports.output.is_some()),\n            callback_max_runtime_us: 0,')
a=code.index('    fn new(');b=code.index('        let timeline =',a)
code=code[:a]+'    fn new_native(project: Project, reverse_install: bool, with_fx:bool) -> Self {\n'+code[b:]
a=code.index('        let source_chain =');b=code.index('        let mut controls',a)
code=code[:a]+'''        let source_chain = native_chain(SOURCE_INSTANCE,"Stochas.vst3",std::fs::read(native_root().join("receipts/stochas-qa-pattern.state")).unwrap());
        let sink_chain = native_chain(SINK_INSTANCE,"Surge XT.vst3",Vec::new());
        let insert_chain = if with_fx {Some(native_chain(INSERT_INSTANCE,"Surge XT Effects.vst3",Vec::new()))}else{None};
'''+code[b:]
a=code.index('        let PluginChain {\n            audio,\n            control,\n            guard,\n        } = insert_chain;');b=code.index('        assert_eq!(dsp.apply_pending',a)
code=code[:a]+'''        if let Some(PluginChain{audio,control,guard})=insert_chain {
            dsp.install_insert_endpoint(2, INSERT_INSTANCE + 10_000, fixed_adapter(audio));controls.push(control);guards.push(guard);
        }
'''+code[b:]
# Reuse only fixture control-plane setup/render helpers, never its mock backends.
assert 'midi_route_chain(' not in code
# Production activation owns its single preflight; a second manual preflight would stage reset twice.
code=code.replace('        self.dsp\n            .preflight_timeline_transport_activation(ticket, epoch)\n            .unwrap();', '        let _ = ticket;')
code=code.replace('frame as f64 / 24_000.0', 'frame as f64 * self.timeline.plugin_transport_tempo() / (48_000.0 * 60.0)')
# Shutdown must be conclusively joined rather than merely requested.
code=code.replace('            guard.shutdown();','            assert_eq!(guard.shutdown_blocking(Duration::from_secs(10)), crate::plugins::plugin_runtime::ShutdownOutcome::Joined);')
(out/'src/audio_midi_routing_tests.rs').write_text(s+'\n'+code+'\n'+(root/'native_suffix.rs').read_text())
manifest={str(p.relative_to(source)):hashlib.sha256(p.read_bytes()).hexdigest() for p in source.rglob('*') if p.is_file() and '.git' not in p.parts and ('src' in p.parts or 'vendor' in p.parts or p.name in ('Cargo.toml','Cargo.lock','build.rs'))}
(root/'receipts/source-snapshot.json').write_text(json.dumps({'base':subprocess.check_output(['git','-C',str(source),'rev-parse','HEAD'],text=True).strip(),'files':manifest},indent=2))
print(out)
