"""Summarize retained run receipts without changing measurements or failed outcomes."""
from pathlib import Path
import json, hashlib
root = Path(__file__).resolve().parent
runs=[]
for run in sorted((root/'runs').iterdir()):
    if not run.is_dir(): continue
    binding=json.loads((run/'run-binding.json').read_text())
    cases=[]
    for path in sorted(run.glob('b*.json')):
        value=json.loads(path.read_text())
        if 'outcome' not in value: continue
        final=value.get('final',value.get('diagnostic',{}))
        cases.append({'case':value['case'],'outcome':value['outcome'],'phase':value.get('phase','paced measurement'),'errors':value.get('errors',[]),'fault':final.get('fault'),'first_sink_sample':value.get('first_sink_sample'),'declared_pdc_samples':value.get('declared_pdc_samples'),'source_note_on_count':value.get('source_note_on_count'),'source_note_off_count':value.get('source_note_off_count'),'callback_runtime_max_us':final.get('callback_runtime_max_us'),'processing_suspended':final.get('processing_suspended'),'timeline_execution_failures':final.get('timeline_execution_failures')})
    runs.append({'run':run.name,'binding':binding,'exit_status':int((run/'exit-status.txt').read_text()) if (run/'exit-status.txt').exists() else None,'raw_log_sha256':hashlib.sha256((run/'raw.log').read_bytes()).hexdigest(),'matrix_pass':sum(c['outcome']=='PASS' for c in cases),'matrix_fail':sum(c['outcome']=='FAIL' for c in cases),'matrix_cases':cases,'fresh_surge_proof':json.loads((run/'fresh-restored-surge.json').read_text()) if (run/'fresh-restored-surge.json').exists() else None})
(root/'receipts/run-index.json').write_text(json.dumps({'scope':'All retained observed runs; setup failures, source defects and endpoint deadlines remain distinct. A later success does not supersede an earlier failure.','runs':runs},indent=2))
print(json.dumps([{k:r[k] for k in ['run','exit_status','matrix_pass','matrix_fail']} for r in runs],indent=2))
