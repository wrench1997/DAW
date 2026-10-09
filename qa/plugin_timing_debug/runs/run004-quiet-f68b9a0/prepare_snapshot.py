from pathlib import Path
import os,shutil,json,hashlib,subprocess
r=Path(__file__).resolve().parent;s=Path(os.environ.get('DAW_SOURCE','__DAW_SOURCE__'));out=r/'source'
assert s.resolve()!=out.resolve() and out.resolve() not in s.resolve().parents,'DAW_SOURCE must not be the disposable snapshot directory'
if out.exists():shutil.rmtree(out)
shutil.copytree(s,out,ignore=shutil.ignore_patterns('.git','target','.codex','.agents'))
t=(s/'src/audio_midi_routing_tests.rs').read_text();a=t.index('struct MidiGraphFixture {');b=t.index('\nfn observed_notes',a);c=t[a:b].replace('MidiGraphFixture','NativeTimingFixture')
c=c.replace('    activation_playing: bool,','    activation_playing: bool,\n    captured_sink: Vec<[f32;2]>,\n    captured_events: Vec<(u64,[u8;3])>,\n    callback_runtime_max_us: u128,\n    paced_start_lateness_max_us: u128,\n    paced_callbacks: u64,')
a=c.index('    fn new(');b=c.index('        let timeline =',a);c=c[:a]+'    fn new_native(project: Project, routed: bool) -> Self {\n'+c[b:]
a=c.index('        let source_chain =');b=c.index('        let mut controls',a)
c=c[:a]+'''        let source_chain=if routed {Some(native_chain(SOURCE_INSTANCE,"Stochas.vst3",std::fs::read(plugin_root().join("receipts/stochas-initial-state.bin")).unwrap()))}else{None};
        let sink_chain=native_chain(SINK_INSTANCE,"Surge XT.vst3",Vec::new());
        let insert_chain=native_chain(INSERT_INSTANCE,"Surge XT Effects.vst3",Vec::new());
'''+c[b:]
a=c.index('        let mut generators =');b=c.index('        for (channel, instance, track, chain)',a)
c=c[:a]+'''        let mut generators=vec![(SINK_CHANNEL,SINK_INSTANCE,2,sink_chain)];
        if let Some(source_chain)=source_chain {generators.push((SOURCE_CHANNEL,SOURCE_INSTANCE,1,source_chain));}
'''+c[b:]
a=c.index('        let master_instances:');b=c.index('        assert_eq!(dsp.apply_pending',a);c=c[:a]+c[b:]
c=c.replace('            activation_playing: true,','            activation_playing: true,\n            captured_sink:Vec::new(),\n            captured_events:Vec::new(),\n            callback_runtime_max_us:0,\n            paced_start_lateness_max_us:0,\n            paced_callbacks:0,')
c=c.replace('frame as f64 / 24_000.0','frame as f64 * self.timeline.plugin_transport_tempo() / (48_000.0 * 60.0)')
c=c.replace('            guard.shutdown();','            assert_eq!(guard.shutdown_blocking(Duration::from_secs(10)),crate::plugins::plugin_runtime::ShutdownOutcome::Joined);')
assert 'midi_route_chain(' not in c and 'midi_route_multi_insert(' not in c
(out/'src/audio_midi_routing_tests.rs').write_text(t+'\n'+c+'\n'+(r/'native_timing_tests.rs').read_text())
manifest={str(p.relative_to(s)):hashlib.sha256(p.read_bytes()).hexdigest() for p in s.rglob('*') if p.is_file() and '.git' not in p.parts and ('src' in p.parts or 'vendor' in p.parts or p.name in ['Cargo.toml','Cargo.lock','build.rs'])}
(r/'receipts/source-snapshot.json').write_text(json.dumps({'commit':os.environ.get('DAW_SOURCE_COMMIT') or subprocess.check_output(['git','-C',str(s),'rev-parse','HEAD'],text=True).strip(),'files':manifest,'harness_sha256':hashlib.sha256((r/'native_timing_tests.rs').read_bytes()).hexdigest(),'builder_sha256':hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),'prepared_test_module_sha256':hashlib.sha256((out/'src/audio_midi_routing_tests.rs').read_bytes()).hexdigest()},indent=2));print(out)
