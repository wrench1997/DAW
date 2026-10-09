"""Build an independent source snapshot and copy exact Cargo compiler artifacts.
Caller owns the Cargo-target reservation. Needs DAW_SOURCE and toolchain/linker env.
"""
import pathlib,os,subprocess,json,shutil,hashlib
r=pathlib.Path(__file__).resolve().parent
subprocess.run(['python3',str(r/'prepare_snapshot.py')],check=True)
(r/'bin').mkdir(exist_ok=True);out={}
(r/'receipts/build-environment.json').write_text(json.dumps({'rustc_verbose_version':subprocess.check_output(['rustc','-vV'],text=True),'selected_environment':{n:os.environ.get(n) for n in ['CARGO_PROFILE_DEV_DEBUG','CARGO_PROFILE_TEST_DEBUG','CARGO_INCREMENTAL','CARGO_TARGET_DIR','PKG_CONFIG_PATH','LIBRARY_PATH','TMPDIR']},'cargo_flags':['--offline','--locked','--all-features'],'scope':'source-linked test binary plus actual production helper'},indent=2))
for command,log,name,target in [(['test','--no-run'],'native-build','native-route-tests','citrus-studio'),(['build'],'helper-build','vst3-host-helper','vst3-host-helper')]:
 with (r/'receipts'/f'{log}.jsonl').open('w') as stdout,(r/'receipts'/f'{log}.stderr').open('w') as stderr:
  subprocess.run(['cargo',*command,'--offline','--locked','--all-features','--bin',target,'--message-format=json'],cwd=r/'source',stdout=stdout,stderr=stderr,check=True)
 artifacts=[json.loads(l) for l in (r/'receipts'/f'{log}.jsonl').read_text().splitlines()]
 a=[a for a in artifacts if a.get('reason')=='compiler-artifact' and a.get('executable') and a['target']['name']==target][-1]
 p=r/'bin'/name;shutil.copy2(a['executable'],p)
 digest=hashlib.sha256(p.read_bytes()).hexdigest()
 archive=r/'compiled'/digest;archive.mkdir(parents=True,exist_ok=True)
 if not (archive/name).exists():shutil.copy2(p,archive/name)
 out[name]={'sha256':digest,'compiler_artifact':a}
(r/'receipts/executable-attribution.json').write_text(json.dumps(out,indent=2))
(r/'receipts/build-binding.json').write_text(json.dumps({'source_manifest_sha256':hashlib.sha256((r/'receipts/source-snapshot.json').read_bytes()).hexdigest(),'build_script_sha256':hashlib.sha256(pathlib.Path(__file__).read_bytes()).hexdigest(),'binaries':{n:v['sha256'] for n,v in out.items()}},indent=2))
print(json.dumps({n:v['sha256'] for n,v in out.items()},indent=2))
