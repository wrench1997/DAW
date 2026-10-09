from render_probe import *
h=Helper('stochas-probe')
try:
 rec={'load':h.load('Stochas.vst3')};print('load',rec['load'],flush=True)
 for cmd in ['AudioBusLayout','GetUnits','GetAllParameters','LatencySamples','TailSamples']:
  rec[cmd]=h.call(cmd);print(cmd,str(rec[cmd])[:1000],flush=True)
 print(h.call('StartProcessing'));print(h.call({'SetPlaying':{'playing':True}}))
 rec['default_pattern_events']=[]
 for i in range(400):
  r=h.call({'Process':{'inputs':[enc([0.]*256)]*2,'frames':256}});rec['default_pattern_events']+=r.get('AudioOutput',{}).get('output_events',[])
 print('default pattern events',len(rec['default_pattern_events']))
 rec['note_on']=h.call({'NoteOn':{'channel':0,'note':60,'velocity':100,'sample_offset':17}});print(rec['note_on'])
 rec['note_on_output']=h.call({'Process':{'inputs':[enc([0.]*256)]*2,'frames':256}})['AudioOutput']['output_events'];print('note on output',rec['note_on_output'])
 nid=rec['note_on']['NoteStarted']['note_id'];rec['note_off']=h.call({'NoteOff':{'note_id':nid,'sample_offset':49}})
 rec['note_off_output']=h.call({'Process':{'inputs':[enc([0.]*256)]*2,'frames':256}})['AudioOutput']['output_events'];print('note off output',rec['note_off_output'])
 (ROOT/'receipts/stochas-probe.json').write_text(json.dumps(rec,indent=2))
finally:h.close()
