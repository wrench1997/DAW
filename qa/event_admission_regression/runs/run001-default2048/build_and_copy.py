"""Build an independent source snapshot and copy exact Cargo compiler artifacts.
Caller owns the Cargo-target reservation. Needs DAW_SOURCE and toolchain/linker env.
"""
import pathlib,os,subprocess,json,shutil,hashlib,time,signal
r=pathlib.Path(__file__).resolve().parent
profile=os.environ.get('NATIVE_HARNESS_PROFILE','release')
assert profile in ['debug','release']
profile_args=['--release'] if profile=='release' else []
assert os.environ.get('CARGO_BUILD_JOBS')=='2','This bounded cold build requires CARGO_BUILD_JOBS=2'
def bounded_build(command,stdout,stderr):
 process=subprocess.Popen(command,cwd=r/'source',stdout=stdout,stderr=stderr,start_new_session=True)
 with (r/'receipts'/f'{profile}-resource-watch.jsonl').open('a') as watch:
  while process.poll() is None:
   free=shutil.disk_usage('${WORKSPACE_ROOT}').free
   memory={k:int(v.strip().split()[0])*1024 for k,v in (line.split(':',1) for line in pathlib.Path('/proc/meminfo').read_text().splitlines()) if k in ['MemAvailable','MemTotal']}
   watch.write(json.dumps({'profile':profile,'command':command,'unix_seconds':time.time(),'free_disk_bytes':free,'memory':memory})+'\n');watch.flush()
   if free<2*1024**3 or memory.get('MemAvailable',2**63)<512*1024**2:
    os.killpg(process.pid,signal.SIGTERM)
    try:process.wait(timeout=10)
    except subprocess.TimeoutExpired:os.killpg(process.pid,signal.SIGKILL);process.wait()
    raise RuntimeError('Stopped own release build: disk<2GiB or available memory<512MiB; no caches deleted')
   time.sleep(2)
 if process.returncode:raise subprocess.CalledProcessError(process.returncode,command)

subprocess.run(['python3',str(r/'prepare_snapshot.py')],check=True)
(r/'bin').mkdir(exist_ok=True);out={}
(r/'receipts/build-environment.json').write_text(json.dumps({'rustc_verbose_version':subprocess.check_output(['rustc','-vV'],text=True),'selected_environment':{n:os.environ.get(n) for n in ['CARGO_BUILD_JOBS','CARGO_PROFILE_DEV_DEBUG','CARGO_PROFILE_TEST_DEBUG','CARGO_INCREMENTAL','CARGO_TARGET_DIR','PKG_CONFIG_PATH','LIBRARY_PATH','TMPDIR']},'profile':profile,'cargo_flags':['--offline','--locked','--all-features',*profile_args],'scope':'source-linked test binary plus actual production helper'},indent=2))
for command,log,name,target in [(['test','--no-run'],'native-build','native-route-tests','citrus-studio'),(['build'],'helper-build','vst3-host-helper','vst3-host-helper')]:
 with (r/'receipts'/f'{log}.jsonl').open('w') as stdout,(r/'receipts'/f'{log}.stderr').open('w') as stderr:
  bounded_build(['cargo',*command,'--offline','--locked','--all-features',*profile_args,'--bin',target,'--message-format=json'],stdout,stderr)
 artifacts=[json.loads(l) for l in (r/'receipts'/f'{log}.jsonl').read_text().splitlines()]
 a=[a for a in artifacts if a.get('reason')=='compiler-artifact' and a.get('executable') and a['target']['name']==target][-1]
 p=r/'bin'/name;shutil.copy2(a['executable'],p)
 digest=hashlib.sha256(p.read_bytes()).hexdigest()
 archive=r/'compiled'/digest;archive.mkdir(parents=True,exist_ok=True)
 if not (archive/name).exists():shutil.copy2(p,archive/name)
 out[name]={'sha256':digest,'compiler_artifact':a}
(r/'receipts/executable-attribution.json').write_text(json.dumps(out,indent=2))
(r/'receipts/build-binding.json').write_text(json.dumps({'profile':profile,'source_manifest_sha256':hashlib.sha256((r/'receipts/source-snapshot.json').read_bytes()).hexdigest(),'build_script_sha256':hashlib.sha256(pathlib.Path(__file__).read_bytes()).hexdigest(),'binaries':{n:v['sha256'] for n,v in out.items()}},indent=2))
print(json.dumps({n:v['sha256'] for n,v in out.items()},indent=2))
