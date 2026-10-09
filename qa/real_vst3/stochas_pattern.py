"""Create an owned deterministic test pattern via the tagged Stochas XML schema.
Never guesses opaque bytes: start from genuine SaveState, parse the source-defined
host/JUCE envelopes, preserve unchanged private trailer, and verify plugin re-export.
Sources: Stochas v1.3.13 src/Persist.cpp, SequenceData.h, Constants.h;
JUCE 4f43011b96eb0636104cb3e433894cda98243626 AudioProcessor.cpp;
Citrus b57076a vendor/vst3-host-0.9.0/src/plugin.rs state envelope.
"""
from render_probe import *
import xml.etree.ElementTree as ET,hashlib
MAGIC=b'VST3HOST_STATE\0\0'; assert len(MAGIC)==16

def unpack(raw):
 assert raw[:16]==MAGIC
 version,n,ctl=struct.unpack('<III',raw[16:28]);assert version==1 and ctl==0xffffffff and len(raw)==28+n
 comp=raw[28:];magic,nxml=struct.unpack('<II',comp[:8]);assert magic==0x21324356
 xml=comp[8:8+nxml];assert len(xml)==nxml
 root=ET.fromstring(xml);assert root.tag=='stochas' and root.get('version')=='1'
 trailer=comp[8+nxml:];assert trailer.startswith(b'\0') and trailer.endswith(b'JUCEPrivateData')
 return root,trailer

def pack(root,trailer):
 xml=ET.tostring(root,encoding='utf-8',xml_declaration=True)
 comp=struct.pack('<II',0x21324356,len(xml))+xml+trailer
 return MAGIC+struct.pack('<III',1,len(comp),0xffffffff)+comp

def pattern(root):
 layers=root.find('layer').findall('l');assert len(layers)==4
 for lay in layers:
  lay.find('mute').set('val','0' if lay.get('idx')=='0' else '1')
  lay.find('pats').clear()
 lay=layers[0];assert lay.get('idx')=='0'
 lay.set('name','Citrus QA own notes')
 for key,val in [('numsteps','4'),('numrows','4'),('curpat','0'),('mono','1'),('mchan','1'),('notecust','1'),('dcycle','50'),('stppm','16'),('clkdiv','16'),('humpos','0'),('humvel','0'),('humlen','0')]:lay.find(key).set('val',val)
 rownotes={125:60,126:64,127:67}
 for n in lay.find('notes').findall('n'):
  if int(n.get('idx')) in rownotes:n.set('cust',str(rownotes[int(n.get('idx'))]))
 p=ET.SubElement(lay.find('pats'),'p',idx='0',name='QA C E G C four steps');rows=ET.SubElement(p,'rows')
 for row,steps in [(125,[0,3]),(126,[1]),(127,[2])]:
  cells=ET.SubElement(ET.SubElement(rows,'r',idx=str(row)),'cells')
  for step in steps:ET.SubElement(cells,'c',idx=str(step),prob='100',velo='100',len='0',offs='0')
 root.find('autoplay').set('val','0');root.find('seed').set('val','12345');root.find('midipass').set('val','1')
 return root

def extract_pattern(root):
 l=root.find("./layer/l[@idx='0']")
 return {'notes':{int(x.get('idx')):int(x.get('cust')) for x in l.findall('./notes/n') if int(x.get('idx')) in [125,126,127]},'cells':sorted((int(r.get('idx')),int(c.get('idx')),int(c.get('prob')),int(c.get('velo')),int(c.get('len'))) for r in l.findall('./pats/p[@idx="0"]/rows/r') for c in r.findall('./cells/c')),'steps':l.find('numsteps').get('val'),'autoplay':root.find('autoplay').get('val')}

def events(h,blocks):
 out=[]
 for i in range(blocks):
  r=h.call({'Process':{'inputs':[enc([0.]*256)]*2,'frames':256}})['AudioOutput']
  assert all(x==0 for c in r['outputs'] for x in dec(c)), 'Stochas must not produce audio'
  for e in r['output_events']:out.append({'block':i,'event':e})
 return out

if __name__=='__main__':
 h=Helper('stochas-pattern')
 try:
  info=h.load('Stochas.vst3');assert info['PluginInfo']['version']=='1.3.13' and info['PluginInfo']['name']=='Stochas';raw=base64.b64decode(h.call('SaveState')['State']['data']);root,tail=unpack(raw);custom=pattern(root);expected=extract_pattern(custom);built=pack(custom,tail)
  result=h.call({'LoadState':{'data':base64.b64encode(built).decode()}});assert 'Success' in result,result
  exported=base64.b64decode(h.call('SaveState')['State']['data']);check,tail2=unpack(exported);assert extract_pattern(check)==expected,(extract_pattern(check),expected)
  (ROOT/'receipts/stochas-qa-pattern.state').write_bytes(exported);(ROOT/'receipts/stochas-qa-pattern.xml').write_bytes(ET.tostring(check,encoding='utf-8',xml_declaration=True))
  print('State source-schema build/load/re-export passed',expected,flush=True)
  print(h.call('StartProcessing'),flush=True);print(h.call({'SetTempo':{'bpm':120.}}),flush=True)
  print(h.call({'SetPlaying':{'playing':False}}),flush=True)
  pre=events(h,8);assert not pre,pre
  print(h.call({'SetPlaying':{'playing':True}}),flush=True);playing=events(h,400)
  print('PLAY',len(playing),playing[:12],flush=True)
  print(h.call({'SetPlaying':{'playing':False}}),flush=True);stopped=events(h,32);print('STOP',stopped,flush=True)
  out={'plugin':info,'source_schema_roundtrip_verified':True,'pattern':expected,'playing_at_bpm':120,'sample_rate':48000,'block_size':256,'preplay_output':pre,'playing_output':playing,'stop_output':stopped,'state_sha256':hashlib.sha256(exported).hexdigest()}
  (ROOT/'receipts/stochas-pattern-output.json').write_text(json.dumps(out,indent=2));assert playing,'No genuine MIDI output produced'
  ons=[e for e in playing if 'NoteOn' in e['event']['data']];offs=[e for e in playing+stopped if 'NoteOff' in e['event']['data']]
  assert len(ons)==len(offs) and not any('NoteOn' in e['event']['data'] for e in stopped)
  assert any('NoteOff' in e['event']['data'] for e in stopped),'This test must stop during an active note'
 finally:h.close()
