#!/usr/bin/env bash
set -euo pipefail
here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
run_name="${1:?Supply a unique run name, e.g. run001-quiet}"
shift
run="$here/runs/$run_name"
if [[ -e "$run" ]]; then echo "Refusing to overwrite $run" >&2; exit 2; fi
mkdir -p "$run" "$here/home/config" "$here/home/data" "$here/tmp"
export NATIVE_TIMING_QA_ROOT="$here" NATIVE_TIMING_RECEIPT_DIR="$run"
export VST3_VALIDATION_ROOT="${VST3_VALIDATION_ROOT:-$(dirname "$here")}" HOME="$here/home" XDG_CONFIG_HOME="$here/home/config" XDG_DATA_HOME="$here/home/data" TMPDIR="$here/tmp" RUST_BACKTRACE=1
python3 - "$here" "$run" <<'PY'
import pathlib,sys,json,hashlib,os,datetime,shutil
r=pathlib.Path(sys.argv[1]);out=pathlib.Path(sys.argv[2]);source=json.loads((r/'receipts/source-snapshot.json').read_text())
assert source.get('harness_sha256')==hashlib.sha256((r/'native_timing_tests.rs').read_bytes()).hexdigest(),'Harness changed or unbound: rebuild before running'
required=os.getenv('REQUIRED_TIMING_SOURCE_COMMIT')
assert required and source['commit']==required,'Set REQUIRED_TIMING_SOURCE_COMMIT to the approved frozen commit'
build=json.loads((r/'receipts/build-binding.json').read_text())
assert build['source_manifest_sha256']==hashlib.sha256((r/'receipts/source-snapshot.json').read_bytes()).hexdigest(),'Source snapshot is newer than the compiled binary'
for name,digest in build['binaries'].items():
 assert hashlib.sha256((r/'bin'/name).read_bytes()).hexdigest()==digest,'Compiled binary changed: '+name
for name in ['source-snapshot.json','executable-attribution.json','build-binding.json']:
 shutil.copy2(r/'receipts'/name,out/name)
for name in ['native_timing_tests.rs','prepare_snapshot.py','build_and_copy.py']:
 shutil.copy2(r/name,out/name)
resource_context={}
for path in ['/sys/fs/cgroup/cpu.max','/sys/fs/cgroup/cpu.stat','/sys/fs/cgroup/memory.max','/sys/fs/cgroup/memory.current','/proc/loadavg']:
 try: resource_context[path]=pathlib.Path(path).read_text().strip()
 except OSError as error: resource_context[path]=str(error)
(out/'host-resources-before.json').write_text(json.dumps(resource_context,indent=2))
record={'started_utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'test_sha256':hashlib.sha256((r/'bin/native-route-tests').read_bytes()).hexdigest(),'helper_sha256':hashlib.sha256((r/'bin/vst3-host-helper').read_bytes()).hexdigest(),'source_manifest_sha256':hashlib.sha256((r/'receipts/source-snapshot.json').read_bytes()).hexdigest(),'source_commit':json.loads((r/'receipts/source-snapshot.json').read_text())['commit'],'profile_filter':os.getenv('NATIVE_TIMING_CASE'),'cpu_load_threads':os.getenv('NATIVE_CPU_LOAD_THREADS','0'),'declared_environment':os.getenv('NATIVE_TIMING_RUN_CONDITION','unspecified')};(out/'run-binding.json').write_text(json.dumps(record,indent=2))
PY
set +e
"$here/bin/native-route-tests" "${1:-native_timing_}" --nocapture --test-threads=1 > "$run/raw.log" 2>&1
status=$?
printf '%s\n' "$status" > "$run/exit-status.txt"
python3 - "$run" <<'PY'
import pathlib,json,sys
p=pathlib.Path(sys.argv[1]);out={}
for name in ['/sys/fs/cgroup/cpu.stat','/sys/fs/cgroup/memory.current','/proc/loadavg']:
 try:out[name]=pathlib.Path(name).read_text().strip()
 except OSError as error:out[name]=str(error)
(p/'host-resources-after.json').write_text(json.dumps(out,indent=2))
PY
exit "$status"
