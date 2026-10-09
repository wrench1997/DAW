"""Build an independent source snapshot and copy exact Cargo compiler artifacts.
Caller owns the Cargo-target reservation. Needs DAW_SOURCE and toolchain/linker env.
"""
import pathlib,os,subprocess,json,shutil,hashlib
r=pathlib.Path(__file__).resolve().parent
subprocess.run(['python3',str(r/'prepare_snapshot.py')],check=True)
(r/'bin').mkdir(exist_ok=True);out={}
for command,log,name,target in [(['test','--no-run'],'native-build','native-route-tests','citrus-studio'),(['build'],'helper-build','vst3-host-helper','vst3-host-helper')]:
 with (r/'receipts'/f'{log}.jsonl').open('w') as stdout,(r/'receipts'/f'{log}.stderr').open('w') as stderr:
  subprocess.run(['cargo',*command,'--offline','--locked','--all-features','--bin',target,'--message-format=json'],cwd=r/'source',stdout=stdout,stderr=stderr,check=True)
 artifacts=[json.loads(l) for l in (r/'receipts'/f'{log}.jsonl').read_text().splitlines()]
 a=[a for a in artifacts if a.get('reason')=='compiler-artifact' and a.get('executable') and a['target']['name']==target][-1]
 p=r/'bin'/name;shutil.copy2(a['executable'],p)
 out[name]={'sha256':hashlib.sha256(p.read_bytes()).hexdigest(),'compiler_artifact':a}
(r/'receipts/executable-attribution.json').write_text(json.dumps(out,indent=2))
print(json.dumps({n:v['sha256'] for n,v in out.items()},indent=2))
