from stochas_pattern import *

def note_entries(evts,kind):
 return [(e['block']*256+e['event']['sample_offset'],e['event']['data'][kind]['pitch']) for e in evts if kind in e['event']['data']]
reports={}
for bpm in [60,120]:
 h=Helper('stochas-tempo-'+str(bpm))
 try:
  info=h.load('Stochas.vst3');raw=(ROOT/'receipts/stochas-qa-pattern.state').read_bytes();assert 'Success' in h.call({'LoadState':{'data':base64.b64encode(raw).decode()}})
  assert 'Success' in h.call({'SetTempo':{'bpm':float(bpm)}});assert 'Success' in h.call({'SetPlaying':{'playing':False}});assert 'Success' in h.call('StartProcessing')
  before=events(h,8);assert not before
  assert 'Success' in h.call({'SetPlaying':{'playing':True}});evts=events(h,410)
  assert 'Success' in h.call({'SetPlaying':{'playing':False}});stopped=events(h,32)
  on=note_entries(evts,'NoteOn');off=note_entries(evts,'NoteOff');stopoff=note_entries(stopped,'NoteOff');assert len(on)>=8
  assert [n for _,n in on]==([60,64,67,60]*((len(on)+3)//4))[:len(on)],on
  intervals=[b[0]-a[0] for a,b in zip(on[1:],on[2:])];expect=48000*60/bpm/4;assert all(abs(x-expect)<=1 for x in intervals),(intervals,expect)
  assert not note_entries(stopped,'NoteOn');assert all(e['block']==0 for e in stopped),stopped
  balance={}
  for e in evts+stopped:
   d=e['event']['data']
   if 'NoteOn' in d: p=d['NoteOn']['pitch'];balance[p]=balance.get(p,0)+1
   if 'NoteOff' in d:p=d['NoteOff']['pitch'];balance[p]=balance.get(p,0)-1;assert balance[p]>=0
  assert all(n==0 for n in balance.values()),balance
  reports[str(bpm)]={'note_on_count':len(on),'note_off_count':len(off),'stop_note_off_count':len(stopoff),'onsets':on,'steady_intervals_samples':intervals,'expected_interval_samples':expect,'balanced_after_stop':balance,'no_note_on_after_stop':True,'events':evts,'stop_events':stopped}
  print(bpm,{k:v for k,v in reports[str(bpm)].items() if k not in ['events','stop_events','onsets']},flush=True)
 finally:h.close()
(ROOT/'receipts/stochas-tempo-check.json').write_text(json.dumps(reports,indent=2));print('PASS: real Stochas BPM timing, pitch sequence, start/stop and balanced note release')
