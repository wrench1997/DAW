import wave,struct,math,json,pathlib,hashlib
r=pathlib.Path(__file__).resolve().parent/'receipts'
def read(p):
 with wave.open(str(p),'rb') as f:return [v[0]/32767 for v in struct.iter_unpack('<hh',f.readframes(f.getnframes()))]
m=read(r/'blank-source-master.wav');s=read(r/'blank-source-isolated-sink.wav');inds=[i for i,v in enumerate(m) if i>=12000 and abs(v)>1/32767];clusters=[]
for i in inds:
 if not clusters or i-clusters[-1][-1]>1000:clusters.append([])
 clusters[-1].append(i)
rows=[]
for c in clusters:
 start=c[0]-1
 def model(n):
  env=.1*.992**n
  return math.tanh(env*math.sin(2*math.pi*1100*n/48000)) if env>.0001 else 0.
 diff=max(abs(m[i]-model(i-start)) for i in range(start,min(start+900,len(m))))
 assert diff<1/32767+1e-6
 rows.append({'first_sample':start,'last_nonzero':c[-1],'max_error_vs_host_1100Hz_exponential_click':diff})
x={'sink_peak_after_quarter_second':max(map(abs,s[12000:])),'master_click_clusters':rows,'comparison_tolerance_pcm16':1/32767+1e-6,'host_click_parameters':{'frequency_hz':1100,'initial_amplitude':0.1,'per_sample_decay':.992,'cutoff':.0001,'output':'tanh'},'input_sha256':{n:hashlib.sha256((r/n).read_bytes()).hexdigest() for n in ['blank-source-master.wav','blank-source-isolated-sink.wav']},'explanation':'Every residual click after the isolated Surge track is zero matches the source-defined host metronome within PCM16 quantization tolerance.'};(r/'metronome-confound-verification.json').write_text(json.dumps(x,indent=2));print(json.dumps(x,indent=2))
