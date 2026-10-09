#[path = "production-src/plugins.rs"]
#[allow(dead_code)]
mod plugins;
use plugins::plugin_runtime::*;
use std::{path::PathBuf,time::{Duration,Instant},thread};
fn block(chain:&mut PluginChain,l:&[f32],r:&[f32])->(Vec<f32>,Vec<f32>){
 assert!(matches!(chain.audio.try_submit(l,r),SubmitStatus::Submitted{..}));
 let mut ol=vec![0.;l.len()];let mut or=vec![0.;r.len()];let deadline=Instant::now()+Duration::from_secs(5);
 loop {let s=chain.audio.try_receive(&mut ol,&mut or);if !matches!(s,ReceiveStatus::Empty){println!("block: {:?}",s);return (ol,or)};assert!(Instant::now()<deadline);thread::sleep(Duration::from_millis(1));}
}
fn main(){
 let root=std::env::var_os("VALIDATION_ROOT").map(PathBuf::from).unwrap_or_else(||std::env::current_dir().unwrap());
 let scan=plugins::scan(&[root.join("plugins")]);
 println!("Production scan: {:?}",scan);
 let config=PluginPrepareConfig{sample_rate:48000.0,max_block_frames:256};
 let mut dry=PluginChain::spawn(vec![],config).unwrap();
 let l:Vec<f32>=(0..256).map(|i|(i as f32*0.057).sin()*0.1).collect();let r:Vec<f32>=l.iter().map(|v|-v).collect();let (ol,or)=block(&mut dry,&l,&r);assert_eq!(l,ol);assert_eq!(r,or);println!("No-plugin production chain dry baseline: bit-exact");assert_eq!(dry.guard.shutdown_blocking(Duration::from_secs(5)),ShutdownOutcome::Joined);
 for (label,names) in [("instrument",vec!["Surge XT"]),("effects",vec!["Surge XT Effects"]),("instrument-fx-chain",vec!["Surge XT","Surge XT Effects"]) ] {
 let specs=names.iter().map(|name|{let mut s=PluginLoadSpec::from_descriptor(scan.iter().find(|d|d.name==*name).unwrap().clone());s.vst3_helper_path=Some(root.join("bin/vst3-host-helper"));s}).collect();
 let mut chain=PluginChain::spawn(specs,config).unwrap();let mut ready=0;let deadline=Instant::now()+Duration::from_secs(30);
 while ready<names.len(){if let Some(ev)=chain.control.try_next_event(){println!("{} {:?}",label,ev);match ev{RuntimeEvent::SlotReady{..}=>ready+=1,RuntimeEvent::SlotFault{..}=>panic!("slot fault"),_=>()}};assert!(Instant::now()<deadline);thread::sleep(Duration::from_millis(1));}
 if label!="effects" {assert!(chain.audio.try_send_midi(Some(0),MidiMessage::new([0x90,60,100],0)));}
 let zeros=vec![0.;256];let mut energy=0.;let mut delta=0.;let mut count=0;
 for _ in 0..30 {let input=if label=="effects"{&l}else{&zeros};let (ol,or)=block(&mut chain,input,input);for (&x,&y) in ol.iter().chain(or.iter()).zip(input.iter().chain(input.iter())){assert!(x.is_finite());energy+=f64::from(x*x);delta+=f64::from((x-y)*(x-y));count+=1;}}
 println!("{} PCM rms={} delta_rms={} samples={}",label,(energy/count as f64).sqrt(),(delta/count as f64).sqrt(),count);assert!(energy>0.00001);assert!(delta>0.00001);
 if label!="effects"{assert!(chain.audio.try_send_midi(Some(0),MidiMessage::new([0x80,60,0],0)));block(&mut chain,&zeros,&zeros);}
 println!("{} bridge stats {:?}",label,chain.audio.stats());assert_eq!(chain.guard.shutdown_blocking(Duration::from_secs(5)),ShutdownOutcome::Joined);
 }
 println!("PASS: real production scanner, no-plugin dry, instrument, FX, instrument-to-FX chain and clean shutdown");
}
