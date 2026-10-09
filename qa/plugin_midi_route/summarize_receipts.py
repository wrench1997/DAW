import pathlib,json,re,hashlib,wave
r=pathlib.Path(__file__).resolve().parent;q=r/'receipts'
def parse(s):return {k:int(v) for k,v in re.findall(r'(\w+): (\d+)',s)}
def wav(n):
 p=q/n
 with wave.open(str(p),'rb') as f:d={'path':n,'sample_rate':f.getframerate(),'channels':f.getnchannels(),'frames':f.getnframes(),'duration_seconds':f.getnframes()/f.getframerate(),'sample_width_bytes':f.getsampwidth()}
 d['sha256']=hashlib.sha256(p.read_bytes()).hexdigest();return d
matrix=json.loads((q/'native-production-graph.json').read_text());fx=json.loads((q/'native-fx-reactivation.json').read_text())
fx['worker_delta']=[{k:b[k]-a[k] for k in ('submitted','completed','deadline_misses','faults','latency_drift_blocks','input_overflows','output_overflows')} for a,b in zip(map(parse,fx['before_worker_stats']),map(parse,fx['after_worker_stats']))]
fx['adapter_delta']=[{k:b[k]-a[k] for k in ('completed_quanta','submitted_quanta','plugin_output_quanta','delayed_dry_quanta','bridge_gaps')} for a,b in zip(map(parse,fx['adapter_before']),map(parse,fx['adapter_after']))]
out={'scope':'Real Linux VST3 through actual production DAW graph, paced offline callback harness; no physical audio-device assertion','source':json.loads((q/'source-snapshot.json').read_text()),'executables':json.loads((q/'executable-attribution.json').read_text()),'plugins':{'Stochas':'1.3.13','Surge XT':'1.3.4','Surge XT Effects':'1.3.4'},'matrix':matrix,'fx_recovery':fx,'single_note_latch':json.loads((q/'native-safety-latch.json').read_text()),'retrigger_chord_latch':json.loads((q/'native-retrigger-chord-latch.json').read_text()),'controlled_overload':json.loads((q/'native-overload-recovery.json').read_text()),'concurrent_load_observation':json.loads((q/'concurrent-load-outcome.json').read_text()),'quiet_same_binary_comparison':json.loads((q/'quiet-comparison-condition.json').read_text()),'audio':[wav(n) for n in ('stochas-production-route-isolated-surge.wav','stochas-production-midi-route-surge.wav','stochas-surge-fx-replanned-isolated.wav')],'limitations':['Two-endpoint MIDI bridge adds4352frames/90.667ms at48kHz plus attack; FX adds2176frames plus native32samples.','Ports-Off keeps legacy1Q timing; any observed misses are reported and are not a real-time performance pass.','Cold Surge Effects latency0→32 is intentionally fenced; recovery is tested through stop and fresh transport epoch.','Default Surge patch creates the sound; WAVs contain our generated notes, not factory demo music. The2second excerpt ends while the repeating pattern is active.','A concurrent-build run lost a source MIDI batch; its precise cause was not logged. Same-binary quiet comparison passed, so unconditional real-time robustness is not established.','No Windows, Harmony Blueprint, physical speaker/device, or native GUI validation is implied by these receipts.']}
(q/'summary.json').write_text(json.dumps(out,indent=2))
print('Summary:',q/'summary.json')
for a in out['audio']:print(a)
for a in matrix:
 if 'adapter_provenance' in a:print(a['case'],'first=',a['first_nonzero'],'endpoints=',a['adapter_provenance'],'workers=',list(map(parse,a['worker_stats'])))
print('FX delta:',fx['worker_delta'],fx['adapter_delta'],'first=',fx['first_sink_sample'])
