import subprocess,json,os,select,struct,base64,math,pathlib,time
ROOT=pathlib.Path(__file__).resolve().parent
class Helper:
 def __init__(self,name):
  (ROOT/'receipts').mkdir(exist_ok=True); (ROOT/'home').mkdir(exist_ok=True); (ROOT/'bin').mkdir(exist_ok=True)
  self.name=name; self.log=open(ROOT/'receipts'/f'{name}.stderr.log','w'); self.buf=b''
  env=dict(os.environ,HOME=str(ROOT/'home'),XDG_CONFIG_HOME=str(ROOT/'home/config'),XDG_DATA_HOME=str(ROOT/'home/data')); env.pop('DISPLAY',None);env.pop('WAYLAND_DISPLAY',None)
  self.p=subprocess.Popen([str(ROOT/'bin/vst3-host-helper')],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=self.log,env=env)
 def call(self,cmd,timeout=30):
  self.p.stdin.write(json.dumps(cmd).encode()+b'\n');self.p.stdin.flush();deadline=time.monotonic()+timeout
  while b'\n' not in self.buf:
   t=deadline-time.monotonic()
   if t<=0 or not select.select([self.p.stdout],[],[],t)[0]:raise TimeoutError(f'{self.name}: {cmd}')
   chunk=os.read(self.p.stdout.fileno(),65536)
   if not chunk:raise RuntimeError(f'helper exited {self.p.poll()}: {cmd}; inspect stderr')
   self.buf+=chunk
   assert len(self.buf)<20000000
  line,self.buf=self.buf.split(b'\n',1)
  return json.loads(line)
 def load(self,plugin):
  return self.call({'LoadPlugin':{'path':str(ROOT/'plugins'/plugin),'sample_rate':48000.0,'block_size':256,'tempo':120.0,'time_sig_numerator':4,'time_sig_denominator':4}})
 def close(self):
  if self.p.poll() is None:
   self.p.stdin.write(b'"Shutdown"\n');self.p.stdin.flush()
   try:self.p.wait(10)
   except subprocess.TimeoutExpired:self.p.kill();self.p.wait();raise
  self.log.close()
if __name__=='__main__':
 for label,plugin in [('instrument','Surge XT.vst3'),('effects','Surge XT Effects.vst3')]:
  h=Helper(label)
  try:
   rec={'plugin':plugin,'load':h.load(plugin)}
   print(label,rec['load'],flush=True)
   for cmd in ['AudioBusLayout','GetUnits','GetAllParameters','LatencySamples','TailSamples']:
    rec[cmd]=h.call(cmd)
    print(label,cmd,str(rec[cmd])[:1000],flush=True)
   (ROOT/'receipts'/f'{label}-probe.json').write_text(json.dumps(rec,indent=2))
  finally:h.close()
