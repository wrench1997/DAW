from render_probe import *
import hashlib,datetime,platform
receipt={'time_utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'sample_rate':48000,'block_frames':256,'platform':platform.platform(),'scope':'Real third-party Linux VST3 DSP via production helper; not hardware, GUI, or MIDI-output routing validation','checks':{}}

def ok(r):
 assert 'Success' in r,r
 return r

def param(h,id,v):
 ok(h.call({'SetParameter':{'id':id,'value':v}}));read=h.call({'GetParameter':{'id':id}});assert abs(read['ParameterValue']['value']-v)<1e-6,read
 return {'id':id,'normalized':v,'readback':read,'display':h.call({'FormatParameter':{'id':id,'normalized':v}})}

def inp_block(index):
 return [[.15*math.sin(2*math.pi*f*(index*256+j)/48000) for j in range(256)] for f in [220,330]]

# Empty helper is explicitly an error, never a fabricated synth output.
h=Helper('validate-empty')
try:
 receipt['checks']['empty_helper_process']=h.call({'Process':{'inputs':[enc([0.]*256)]*2,'frames':256}})
 assert receipt['checks']['empty_helper_process']=={'Error':{'message':'No plugin loaded'}}
finally:h.close()

# Full instrument lifecycle, parameters, state and actual note release.
h=Helper('validate-instrument')
try:
 info=h.load('Surge XT.vst3')['PluginInfo'];r={'info':info,'buses':h.call('AudioBusLayout'),'units':h.call('GetUnits')}
 assert info['has_midi_input'] and info['name']=='Surge XT'
 r['parameter_count']=len(h.call('GetAllParameters')['Parameters']['params'])
 r['program_select']=h.call({'SelectProgram':{'unit_id':0,'program_index':0}})
 r['volume_parameter']=param(h,1336600346,.75)
 state=h.call('SaveState')['State']['data'];r['saved_state_bytes']=len(base64.b64decode(state))
 param(h,1336600346,.2);r['state_restore']=ok(h.call({'LoadState':{'data':state}}))
 r['restored_volume']=h.call({'GetParameter':{'id':1336600346}});assert abs(r['restored_volume']['ParameterValue']['value']-.75)<1e-6
 ok(h.call('StartProcessing'))
 pre=cat([process(h) for _ in range(20)]);r['pre_note']=stats(pre);assert r['pre_note']['peak']==0
 nid=h.call({'NoteOn':{'channel':0,'note':60,'velocity':100,'sample_offset':0}})['NoteStarted']['note_id']
 on=cat([process(h) for _ in range(188)]);r['note_on']=stats(on);assert r['note_on']['finite'] and r['note_on']['rms']>.0001
 ok(h.call({'NoteOff':{'note_id':nid,'sample_offset':0}}))
 off=cat([process(h) for _ in range(375)]);r['note_off']=stats(off);r['late_release']=stats([c[-48000:] for c in off]);assert r['late_release']['peak']<1e-6
 ok(h.call('StopProcessing'));ok(h.call('UnloadPlugin'));receipt['checks']['instrument']=r
finally:h.close()

# Identical generated stereo signal through dry and explicit delay settings.
outputs={};source=cat([inp_block(i) if i<94 else [[0.]*256]*2 for i in range(282)])
for label,mix,bypass in [('dry',0.,0.),('delay',1.,0.),('bypass',1.,1.)]:
 h=Helper('validate-fx-'+label)
 try:
  info=h.load('Surge XT Effects.vst3')['PluginInfo'];assert info['category']=='Fx';r={'info':info,'buses':h.call('AudioBusLayout')}
  r['program_select']=h.call({'SelectProgram':{'unit_id':0,'program_index':0}})
  r['parameters']=[param(h,id,v) for id,v in [(887087884,0.),(720135338,.5),(720135339,.4),(720135340,.4),(720135341,.25),(720135342,0.),(720135343,0.),(720135344,1.),(720135347,.5),(849359077,mix),(1652125811,bypass)]]
  ok(h.call('StartProcessing'))
  outs=cat([process(h,inp_block(i) if i<94 else [[0.]*256]*2) for i in range(282)])
  outputs[label]=outs;r['output']=stats(outs);r['delta_from_input']=stats([[a-b for a,b in zip(c,d)] for c,d in zip(outs,source)])
  r['tail_after_input_stops']=stats([c[94*256:] for c in outs]);assert r['output']['finite']
  if label in ['dry','bypass']: assert r['delta_from_input']['peak']<1e-5,r
  else:assert r['delta_from_input']['rms']>.001 and r['tail_after_input_stops']['rms']>.0001,r
  ok(h.call('StopProcessing'));ok(h.call('UnloadPlugin'));receipt['checks']['effects_'+label]=r
 finally:h.close()
wav('known-stereo-source-dry.wav',source);wav('surge-effects-delay.wav',outputs['delay'])
receipt['helper_sha256']=hashlib.sha256((ROOT/'bin/vst3-host-helper').read_bytes()).hexdigest()
receipt['plugin_package_sha256']=hashlib.sha256((ROOT/'downloads/surge-xt-linux-1.3.4-pluginsonly.tar.gz').read_bytes()).hexdigest()
receipt['all_assertions_passed']=True
(ROOT/'receipts'/'validation.json').write_text(json.dumps(receipt,indent=2))
print(json.dumps({k:{x:v for x,v in r.items() if x in ['parameter_count','pre_note','note_on','late_release','saved_state_bytes','output','delta_from_input','tail_after_input_stops']} if isinstance(r,dict) else r for k,r in receipt['checks'].items()},indent=2))
