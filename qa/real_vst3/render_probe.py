from probe import *
import wave,array

def enc(ch):return base64.b64encode(struct.pack('<%df'%len(ch),*ch)).decode()
def dec(ch):
 b=base64.b64decode(ch);return list(struct.unpack('<%df'%(len(b)//4),b))
def process(h,inputs=None,n=256):
 r=h.call({'Process':{'inputs':[enc(c) for c in (inputs or [[0.0]*n,[0.0]*n])],'frames':n}})
 if 'AudioOutput' not in r:raise RuntimeError(r)
 return [dec(c) for c in r['AudioOutput']['outputs']]
def stats(channels):
 a=[x for c in channels for x in c];return {'samples':len(a),'finite':all(math.isfinite(x) for x in a),'peak':max(map(abs,a),default=0),'rms':math.sqrt(sum(x*x for x in a)/max(len(a),1))}
def cat(blocks):return [sum((b[c] for b in blocks),[]) for c in range(len(blocks[0]))]
def wav(name,ch):
 assert all(math.isfinite(x) for c in ch for x in c)
 with wave.open(str(ROOT/'receipts'/name),'wb') as w:
  w.setnchannels(len(ch));w.setsampwidth(2);w.setframerate(48000)
  w.writeframes(b''.join(struct.pack('<h',int(max(-1,min(1,s))*32767)) for samples in zip(*ch) for s in samples))
if __name__=='__main__':
 h=Helper('instrument-render')
 try:
  print(h.load('Surge XT.vst3'),flush=True); print(h.call('StartProcessing'),flush=True)
  pre=cat([process(h) for _ in range(20)]); print('pre',stats(pre),flush=True)
  r=h.call({'NoteOn':{'channel':0,'note':60,'velocity':100,'sample_offset':0}});print(r,flush=True);nid=r['NoteStarted']['note_id']
  on=cat([process(h) for _ in range(188)]);print('on',stats(on),flush=True)
  print(h.call({'NoteOff':{'note_id':nid,'sample_offset':0}}),flush=True)
  off=cat([process(h) for _ in range(375)]);print('off',stats(off),'last',stats([c[-48000:] for c in off]),flush=True)
  wav('surge-instrument-note.wav',[pre[i]+on[i]+off[i] for i in range(2)])
 finally:h.close()
 h=Helper('effects-render')
 try:
  print(h.load('Surge XT Effects.vst3'),flush=True)
  for id in [887087884,849359077,720135339,720135338]:
   print('param',id,[(v,h.call({'FormatParameter':{'id':id,'normalized':v}})) for v in [0,.025,.05,.1,.2,.3,.4,.5,.6,.7,.8,.9,1]],flush=True)
  print(h.call('StartProcessing'),flush=True)
  inp=[[.1*math.sin(2*math.pi*440*j/48000) for j in range(48000)]]*2
  out=cat([process(h,[c[j:j+256]+[0.]*(256-len(c[j:j+256])) for c in inp]) for j in range(0,48000,256)])
  print('fx',stats(out),'input',stats(inp),'delta',stats([[a-b for a,b in zip(out[i],inp[i])] for i in range(2)]),flush=True)
  wav('surge-effects-default.wav',out)
 finally:h.close()
