
// Independent acceptance harness only. Every backend below is a genuine installed
// Stochas/Surge binary through PluginChain::spawn_identified. No mock factory is used.
fn native_root() -> PathBuf { std::env::var_os("VST3_VALIDATION_ROOT").map(PathBuf::from).unwrap_or_else(||PathBuf::from("<VALIDATION_ROOT>")) }
fn native_chain(instance:u64,bundle:&str,state:Vec<u8>) -> PluginChain {
    let root=native_root();
    let descriptor=crate::plugins::PluginDescriptor {
        id:format!("native-validation:{bundle}"), name:bundle.into(),vendor:"Surge Synth Team".into(),
        path:root.join("plugins").join(bundle),format:crate::plugins::PluginFormat::Vst3,
        category:String::new(),is_instrument:bundle!="Surge XT Effects.vst3",verified:false,
        vst3_metadata:None,scan_error:None,
    };
    let mut spec=crate::plugins::plugin_runtime::PluginLoadSpec::from_descriptor(descriptor);
    spec.vst3_helper_path=Some(root.join("route-acceptance/bin/vst3-host-helper"));spec.initial_state=state;
    let mut chain=PluginChain::spawn_identified(vec![(instance,spec)],PluginPrepareConfig{sample_rate:48000.0,max_block_frames:128}).unwrap();
    let deadline=Instant::now()+Duration::from_secs(30);
    loop {
        if let Some(event)=chain.control.try_next_event(){match event {
            crate::plugins::plugin_runtime::RuntimeEvent::SlotReady{..}=>{println!("native ready {event:?}");break},
            crate::plugins::plugin_runtime::RuntimeEvent::SlotFault{..}=>panic!("native plugin load failed: {event:?}"),_=>{}
        }}
        assert!(Instant::now()<deadline,"native plugin load deadline");thread::sleep(Duration::from_millis(1));
    }
    while !chain.control.plugin_latency_snapshot().is_some_and(|s|s.slot_is_active(0)) {
        assert!(Instant::now()<deadline,"native active latency snapshot deadline");thread::sleep(Duration::from_millis(1));
    }
    chain
}
fn native_project(bpm:f32,with_fx:bool,routed:bool)->Project {
    let mut project=midi_route_project(true,false);project.tempo=bpm;project.song_length_beats=32.;
    if !with_fx {project.mixer_insert_slots.clear();project.plugin_instances.retain(|p|p.id!=INSERT_INSTANCE);}
    if !routed {for p in &mut project.plugin_instances{p.midi_ports=crate::plugin_midi_routing::PluginMidiPorts::default();}}
    for p in &mut project.plugin_instances {
        let (bundle,uid)=match p.id {
            SOURCE_INSTANCE=>("Stochas.vst3","ABCDEF019182FAEB70726F6A53746F63"),
            SINK_INSTANCE=>("Surge XT.vst3","ABCDEF019182FAEB566D624153675854"),
            _=>("Surge XT Effects.vst3","ABCDEF019182FAEB566D624153465854"),
        };p.path=native_root().join("plugins").join(bundle);p.uid=uid.into();p.name=bundle.into();p.vendor="Surge Synth Team".into();
    }
    project
}
fn native_wav(name:&str,audio:&[[f32;2]]) {
    use std::io::Write;
    let mut f=std::fs::File::create(native_root().join("route-acceptance/receipts").join(name)).unwrap();
    let bytes=(audio.len()*4) as u32;f.write_all(b"RIFF").unwrap();f.write_all(&(36+bytes).to_le_bytes()).unwrap();f.write_all(b"WAVEfmt ").unwrap();f.write_all(&16u32.to_le_bytes()).unwrap();f.write_all(&1u16.to_le_bytes()).unwrap();f.write_all(&2u16.to_le_bytes()).unwrap();f.write_all(&48000u32.to_le_bytes()).unwrap();f.write_all(&192000u32.to_le_bytes()).unwrap();f.write_all(&4u16.to_le_bytes()).unwrap();f.write_all(&16u16.to_le_bytes()).unwrap();f.write_all(b"data").unwrap();f.write_all(&bytes.to_le_bytes()).unwrap();
    for frame in audio {for sample in frame {f.write_all(&((sample.clamp(-1.,1.)*32767.)as i16).to_le_bytes()).unwrap();}}
}
impl NativeGraphFixture {
    // No waits inside or between sub-quanta of this callback; only the caller paces
    // the device-callback boundary. PluginChain worker threads remain asynchronous.
    fn render_native_callback(&mut self,frames:usize)->(Vec<[f32;2]>,Vec<(u64,[u8;3])>) {
        let base=self.transport.device_frame;let mut output=Vec::with_capacity(frames);
        self.dsp.set_device_frame(base);
        self.dsp.set_callback_transport_boundary(MidiRecordClockAnchor{device_frame:base,timeline_frame:self.transport.timeline_frame,transport_epoch:self.transport.epoch,loop_count:self.transport.loop_count},self.transport.request.playing);
        let callback_started=Instant::now();
        render_transport_chunk(&mut self.dsp,&self.status,&self.mailbox,&mut self.transport,frames,|_,block|output.extend_from_slice(block));
        self.callback_max_runtime_us=self.callback_max_runtime_us.max(callback_started.elapsed().as_micros());
        assert!(frames<=MAX_MIXER_BLOCK_FRAMES);
        if self.dsp.meter_graph_rendered { self.sink_audio.extend_from_slice(&self.dsp.track_block[2*MAX_MIXER_BLOCK_FRAMES..2*MAX_MIXER_BLOCK_FRAMES+frames]); }
        else {self.sink_audio.extend(std::iter::repeat_n([0.;2],frames));}
        let source=self.dsp.find_generator_slot(SOURCE_CHANNEL).unwrap();let batch=self.dsp.generator_endpoints[source].as_ref().unwrap().endpoint.adapter.midi_output();
        if self.check_source_output && batch.lost {
            let report=serde_json::json!({"device_frame":base,"frames":frames,"epoch":self.transport.epoch,"source_midi_len":batch.len,"source_audio_lost":batch.audio_lost,"route_fault":self.dsp.midi_route_faulted,"published_faulted_destinations":self.status.plugin_midi_faulted_destinations.load(Ordering::Relaxed),"worker_stats":self.controls.iter().map(|c|format!("{:?}",c.stats())).collect::<Vec<_>>(),"adapter_stats":self.dsp.generator_endpoints.iter().flatten().map(|s|format!("{:?}",s.endpoint.stats())).collect::<Vec<_>>()});
            println!("NATIVE_UNEXPECTED_MIDI_LOSS {report}");std::fs::write(native_root().join("route-acceptance/receipts/unexpected-midi-loss.json"),serde_json::to_vec_pretty(&report).unwrap()).unwrap();
            panic!("source MIDI batch lost");
        }
        let events=batch.events[..batch.len].iter().map(|e|(base+u64::from(e.message.sample_offset),e.message.data)).collect();
        (output,events)
    }
    fn render_native_paced(&mut self,frames:usize,partition:usize)->(Vec<[f32;2]>,Vec<(u64,[u8;3])>,u128) {
        let mut output=Vec::with_capacity(frames);let mut events=Vec::new();let mut deadline=Instant::now();let mut max_late=0;let mut callbacks=0;
        while output.len()<frames {
            let n=partition.min(frames-output.len());let (a,e)=self.render_native_callback(n);output.extend(a);events.extend(e);callbacks+=1;
            deadline+=Duration::from_secs_f64(n as f64/48000.0);
            // Bounded positive jitter once per seven callbacks, with no worker polling.
            let target=deadline+if callbacks%7==0 {Duration::from_micros(300)}else{Duration::ZERO};
            let now=Instant::now();if target>now {thread::sleep(target-now)}else{max_late=max_late.max((now-target).as_micros());}
        }
        self.wait_workers();(output,events,max_late)
    }
}
#[test]
fn native_vst3_routed_graph_acceptance() {
    let mut reports=Vec::new();
    for (bpm,partition,with_fx,routed) in [(120.,128,false,true),(120.,256,false,true),(120.,512,false,true),(120.,2048,false,true),(60.,512,false,true),(120.,128,false,false),(120.,512,true,true)] {
        if with_fx && std::env::var_os("NATIVE_SKIP_FX").is_some(){continue;}
        let label=format!("bpm{}_cb{}_fx{}_route{}",bpm as u32,partition,with_fx,routed);
        if let Ok(filter)=std::env::var("NATIVE_ROUTE_CASE") {if !label.contains(&filter){continue;}}
        let mut f=NativeGraphFixture::new_native(native_project(bpm,with_fx,routed),true,with_fx);f.activate(2,0);
        let (audio,events,max_late)=f.render_native_paced(96000,partition);println!("NATIVE_CASE_FINISHED {label} routefault={} executionfail={} stats={:?}",f.dsp.midi_route_faulted,f.dsp.timeline_execution_failures,f.controls.iter().map(|c|c.stats()).collect::<Vec<_>>());
        if f.dsp.midi_route_faulted!=0 || f.dsp.timeline_execution_failures!=0 {
            native_wav(&format!("faulted-{label}.wav"),&audio);
            let failure=serde_json::json!({"case":label,"route_fault":f.dsp.midi_route_faulted,"timeline_execution_failures":f.dsp.timeline_execution_failures,"worker_stats":f.controls.iter().map(|c|format!("{:?}",c.stats())).collect::<Vec<_>>(),"adapter_stats":f.dsp.generator_endpoints.iter().flatten().map(|s|format!("{:?}",s.endpoint.stats())).chain(f.dsp.insert_endpoints.iter().flatten().map(|s|format!("{:?}",s.endpoint.stats()))).collect::<Vec<_>>()});
            std::fs::write(native_root().join("route-acceptance/receipts").join(format!("failure-{label}.json")),serde_json::to_vec_pretty(&failure).unwrap()).unwrap();
        }
        if routed {f.assert_clean();}else{
            assert_eq!(f.dsp.midi_route_faulted,0);assert_eq!(f.dsp.timeline_execution_failures,0);
            for c in &f.controls {let a=c.stats();assert_eq!(a.faults,0);assert_eq!(a.latency_drift_blocks,0);assert_eq!(a.input_overflows,0);assert_eq!(a.output_overflows,0);}
        }
        let peak=audio.iter().flatten().fold(0f32,|p,x|{assert!(x.is_finite());p.max(x.abs())});let energy:f64=audio.iter().flatten().map(|x|f64::from(*x)*f64::from(*x)).sum();let rms=(energy/(audio.len()*2)as f64).sqrt();
        let first=audio.iter().position(|a|a[0].abs().max(a[1].abs())>1e-6);
        let onsets:Vec<_>=events.iter().filter(|(_,d)|d[0]&0xf0==0x90&&d[2]>0).copied().collect();
        if routed {
            assert!(rms>0.001,"no genuine routed instrument audio {label}: {rms}");assert!(onsets.len()>=6,"no real source notes {label}");
            assert_eq!(onsets.iter().take(4).map(|(_,d)|d[1]).collect::<Vec<_>>(),vec![60,64,67,60]);
            let period=48000.*60./f64::from(bpm)/4.;for pair in onsets.windows(2).skip(1) {assert!(((pair[1].0-pair[0].0)as f64-period).abs()<=1.0,"wrong MIDI tempo {label}: {pair:?}");}
            let minimum=2176*(if with_fx{3}else{2});assert!(first.unwrap()>=minimum,"audio appeared before routed bridge");assert!(first.unwrap()<minimum+512,"unexpected route onset {label}: {first:?}");
        } else {assert!(f.sink_audio.iter().flatten().all(|x|*x==0.0),"MIDI port Off sink baseline should be silent; master includes metronome");}
        let mut adapter_receipts=Vec::new();
        for (name,endpoint) in f.dsp.generator_endpoints.iter().flatten().map(|s|(format!("generator-{}",s.channel_id),&s.endpoint)).chain(f.dsp.insert_endpoints.iter().enumerate().filter_map(|(i,s)|s.as_ref().map(|s|(format!("insert-{i}"),&s.endpoint)))) {
            let a=endpoint.stats();let expected_startup=if routed{16}else{1};
            let legacy_misses=if routed{0}else{f.controls[adapter_receipts.len()].stats().deadline_misses};
            assert_eq!(a.completed_quanta,96000/128,"{label} {name} quantum count");
            assert_eq!(a.submitted_quanta,a.completed_quanta,"{label} {name} submissions");
            assert_eq!(a.delayed_dry_quanta,expected_startup+legacy_misses,"{label} {name} nonstartup fallback");
            assert_eq!(a.plugin_output_quanta,a.completed_quanta-expected_startup-legacy_misses,"{label} {name} exact plugin output count");
            assert_eq!(a.bridge_gaps,0);assert_eq!(a.latency_drift_quanta,0);assert_eq!(a.output_underflow_frames,0);assert_eq!(a.frame_event_overflows,0);assert_eq!(a.endpoint_event_rejections,0);
            adapter_receipts.push(serde_json::json!({"endpoint":name,"completed_quanta":a.completed_quanta,"submitted_quanta":a.submitted_quanta,"exact_plugin_quanta":a.plugin_output_quanta,"delayed_dry_quanta":a.delayed_dry_quanta,"nonstartup_legacy_fallback_quanta":legacy_misses,"expected_lookahead_quanta":expected_startup,"all_stats":format!("{a:?}")}));
        }
        let first_note_off=events.iter().find(|(_,d)|d[0]&0xf0==0x80).map(|(frame,_)|*frame as usize+2176);
        let range_rms=|start:usize,end:usize|->Option<f64>{audio.get(start..end).map(|s|(s.iter().flatten().map(|x|f64::from(*x).powi(2)).sum::<f64>()/(s.len()*2)as f64).sqrt())};
        let release=first_note_off.map(|p|serde_json::json!({"audio_note_off_position":p,"early_release_rms":range_rms(p+128,p+512),"late_release_rms":range_rms(p+2300,p+2800),"next_note_audio_position":events.iter().find(|(frame,d)|*frame as usize+2176>p&&d[0]&0xf0==0x90).map(|(frame,_)|*frame+2176)}));
        let stats:Vec<_>=f.controls.iter().map(|c|format!("{:?}",c.stats())).collect();
        let rep=serde_json::json!({"case":label,"bpm":bpm,"callback_frames":partition,"fx":with_fx,"routed":routed,"strict_realtime_provenance_gate":routed,"off_baseline_scope":"sink silence and full accounting; legacy1Q deadline misses remain explicitly reported","frames":audio.len(),"master_peak":peak,"master_rms":rms,"sink_peak":f.sink_audio.iter().flatten().fold(0f32,|p,x|p.max(x.abs())),"first_nonzero":first,"source_events":events,"max_callback_lateness_us":max_late,"max_callback_render_runtime_us":f.callback_max_runtime_us,"worker_stats":stats,"adapter_provenance":adapter_receipts,"first_note_release":release});
        println!("NATIVE_GRAPH {}",rep);reports.push(rep);
        std::fs::write(native_root().join("route-acceptance/receipts/native-production-graph-partial.json"),serde_json::to_vec_pretty(&reports).unwrap()).unwrap();
        if bpm==120.&&partition==512&&routed&&!with_fx {native_wav("stochas-production-midi-route-surge.wav",&audio);native_wav("stochas-production-route-isolated-surge.wav",&f.sink_audio);}
        f.mailbox.publish(TransportMutation::SetPlaying(false));let(stopped,_,_)=f.render_native_paced(24000,partition);assert!(stopped.iter().flatten().all(|x|*x==0.));assert!(!f.transport.request.playing);
        if bpm==120.&&partition==512&&routed&&!with_fx {
            let device_before=f.transport.device_frame;f.activate(3,24000);
            let (resumed,notes,late)=f.render_native_paced(48000,512);f.assert_clean();
            let on:Vec<_>=notes.iter().filter(|(_,d)|d[0]&0xf0==0x90&&d[2]>0).collect();
            assert!(on.len()>=6);assert_eq!(on.iter().take(4).map(|(_,d)|d[1]).collect::<Vec<_>>(),vec![60,64,67,60]);
            assert!(notes.iter().all(|(frame,_)|*frame>=device_before+2176));assert_eq!(on[0].0,device_before+2176,"fresh generated source onset after seek");
            let first=resumed.iter().position(|a|a[0].abs().max(a[1].abs())>1e-6).unwrap();
            native_wav("stochas-production-route-after-seek.wav",&resumed);
            println!("NATIVE_SEEK device_before={device_before} target_content_frame=24000 first_audio={first} source_notes={notes:?} stats={:?}",f.controls.iter().map(|c|c.stats()).collect::<Vec<_>>());
            // A real VST instrument may retain a natural note-off release through a
            // paused interval. Record early audio instead of confusing it with new MIDI.
            assert!(first<4864,"late new-epoch audio onset {first}");
            reports.push(serde_json::json!({"case":"production_stop_then_seek_epoch3","target_content_frame":24000,"first_audio_after_epoch_samples":first,"source_events":notes,"max_callback_lateness_us":late}));
            f.mailbox.publish(TransportMutation::SetPlaying(false));f.render_native_paced(2048,512);
            let source_index=f.dsp.find_generator_slot(SOURCE_CHANNEL).unwrap();
            assert!(f.controls[source_index].load_state(0,std::fs::read(native_root().join("receipts/stochas-initial-state.bin")).unwrap()));
            assert!(f.controls[source_index].request_state_tagged(0,888));
            let deadline=Instant::now()+Duration::from_secs(5);loop {
                if let Some(crate::plugins::plugin_runtime::RuntimeEvent::State{request_id:888,..})=f.controls[source_index].try_next_event(){break;}
                assert!(Instant::now()<deadline,"blank Stochas state acknowledgement");thread::sleep(Duration::from_millis(1));
            }
            f.activate(4,0);let sink_start=f.sink_audio.len();let (release_only,quiet_events,_)=f.render_native_paced(96000,512);f.assert_clean();
            assert!(quiet_events.is_empty(),"blank Stochas emitted notes after new epoch {quiet_events:?}");
            let sink_only=&f.sink_audio[sink_start..];
            native_wav("blank-source-master.wav",&release_only);native_wav("blank-source-isolated-sink.wav",sink_only);
            let windows:Vec<_>=release_only.chunks(12000).zip(sink_only.chunks(12000)).enumerate().map(|(i,(master,sink))|{
                let stats=|a:&[[f32;2]]|serde_json::json!({"peak":a.iter().flatten().fold(0f32,|p,x|p.max(x.abs())),"rms":(a.iter().flatten().map(|x|f64::from(*x).powi(2)).sum::<f64>()/(a.len()*2)as f64).sqrt()});
                serde_json::json!({"quarter_second":i,"master":stats(master),"isolated_sink_track2":stats(sink)})
            }).collect();
            let early=sink_only[..4800].iter().flatten().fold(0f32,|p,v|p.max(v.abs()));let late=sink_only[48000..].iter().flatten().fold(0f32,|p,v|p.max(v.abs()));
            let release_report=serde_json::json!({"case":"production_resume_blank_source_natural_release","source_event_count":quiet_events.len(),"early_sink_release_peak":early,"final_second_sink_peak":late,"quarter_second_windows":windows,"master_contains_host_metronome":true});
            println!("NATIVE_GRAPH_RELEASE {}",release_report);
            std::fs::write(native_root().join("route-acceptance/receipts/native-release-isolation.json"),serde_json::to_vec_pretty(&release_report).unwrap()).unwrap();
            assert!(late<1e-6,"native sink release did not settle after no-MIDI restart: {late}");
            reports.push(release_report);

        }
        f.finish();
    }
    std::fs::write(native_root().join("route-acceptance/receipts/native-production-graph.json"),serde_json::to_vec_pretty(&reports).unwrap()).unwrap();
}

fn native_sync_block(chain:&mut PluginChain)->[f32;128] {
    let zeros=[0.0;128];assert!(matches!(chain.audio.try_submit(&zeros,&zeros),crate::plugins::plugin_runtime::SubmitStatus::Submitted{..}));
    let mut l=[0.;128];let mut r=[0.;128];let deadline=Instant::now()+Duration::from_secs(5);
    loop {match chain.audio.try_receive(&mut l,&mut r) {
        crate::plugins::plugin_runtime::ReceiveStatus::Empty=>{assert!(Instant::now()<deadline);thread::sleep(Duration::from_millis(1));},
        crate::plugins::plugin_runtime::ReceiveStatus::Processed{..}=>return l,
        x=>panic!("unexpected real native render status {x:?}")
    }}
}
#[test]
fn native_vst3_surges_safety_latch_releases_held_note() {
    let mut chain=native_chain(909,"Surge XT.vst3",Vec::new());
    chain.audio.set_transport(PluginTransport{playing:true,tempo:120.,..Default::default()});
    assert!(chain.audio.try_send_midi(Some(0),MidiMessage::new([0x90,60,100],0)));
    let held:Vec<f32>=(0..100).flat_map(|_|native_sync_block(&mut chain)).collect();let peak=held.iter().fold(0f32,|p,v|p.max(v.abs()));assert!(peak>0.01);
    chain.audio.block_midi_until_epoch();let before=chain.control.stats().submitted;
    // No audio submitted during this independent worker-reset opportunity.
    thread::sleep(Duration::from_millis(100));assert_eq!(chain.control.stats().submitted,before);
    chain.audio.set_epoch(2);chain.audio.set_transport(PluginTransport{playing:false,tempo:120.,..Default::default()});
    let after:Vec<f32>=(0..750).flat_map(|_|native_sync_block(&mut chain)).collect();assert!(after.iter().all(|s|s.is_finite()));
    let tail_peak=after[after.len()-48000..].iter().fold(0f32,|p,v|p.max(v.abs()));assert!(tail_peak<1e-6,"held note resurrected after latch/epoch: {tail_peak}");
    assert_eq!(chain.control.stats().faults,0);assert_eq!(chain.control.stats().reset_faults,0);
    println!("NATIVE_LATCH held_peak={peak} final_second_peak={tail_peak} stats={:?}",chain.control.stats());
    let receipt=serde_json::json!({"held_note_peak":peak,"silent_wait_without_submissions_ms":100,"last_second_peak":tail_peak,"no_explicit_note_off_sent_by_test":true,"scope":"combined real VST3 worker safety latch then new-epoch no-resurrection, allowing release tail"});
    std::fs::write(native_root().join("route-acceptance/receipts/native-safety-latch.json"),serde_json::to_vec_pretty(&receipt).unwrap()).unwrap();
    assert_eq!(chain.guard.shutdown_blocking(Duration::from_secs(10)),crate::plugins::plugin_runtime::ShutdownOutcome::Joined);
}

#[test]
fn native_vst3_fx_latency_reactivation() {
    let mut f=NativeGraphFixture::new_native(native_project(120.,true,true),true,true);f.activate(2,0);
    let(_,_,_)=f.render_native_paced(4096,512);
    let before_recovery:Vec<_>=f.controls.iter().map(|c|c.stats()).collect();
    println!("NATIVE_FX_INITIAL_DRIFT routefault={} failures={} stats={before_recovery:?}",f.dsp.midi_route_faulted,f.dsp.timeline_execution_failures);
    assert_ne!(f.dsp.midi_route_faulted,0,"expected genuine lazy latency change to be fenced");
    assert_eq!(before_recovery[2].latency_samples,2080);
    f.mailbox.publish(TransportMutation::SetPlaying(false));f.render_native_paced(2048,512);
    f.activate(3,0);
    let worker_before:Vec<_>=f.controls.iter().map(|c|c.stats()).collect();
    let adapter_before:Vec<_>=f.dsp.generator_endpoints.iter().flatten().map(|s|s.endpoint.stats()).chain(f.dsp.insert_endpoints.iter().flatten().map(|s|s.endpoint.stats())).collect();
    let start=f.sink_audio.len();let base=f.transport.device_frame;let previous_failures=f.dsp.timeline_execution_failures;
    let(audio,events,late)=f.render_native_paced(96000,512);
    let sink=&f.sink_audio[start..];let peak=sink.iter().flatten().fold(0f32,|p,x|p.max(x.abs()));let first=sink.iter().position(|v|v[0].abs().max(v[1].abs())>1e-6);
    native_wav("stochas-surge-fx-replanned-master.wav",&audio);native_wav("stochas-surge-fx-replanned-isolated.wav",sink);
    let worker_after:Vec<_>=f.controls.iter().map(|c|c.stats()).collect();let adapter_after:Vec<_>=f.dsp.generator_endpoints.iter().flatten().map(|s|s.endpoint.stats()).chain(f.dsp.insert_endpoints.iter().flatten().map(|s|s.endpoint.stats())).collect();
    let report=serde_json::json!({"case":"native_fx_latency_stop_reactivate_epoch3","initial_worker_stats":before_recovery.iter().map(|s|format!("{s:?}")).collect::<Vec<_>>(),"before_worker_stats":worker_before.iter().map(|s|format!("{s:?}")).collect::<Vec<_>>(),"after_worker_stats":worker_after.iter().map(|s|format!("{s:?}")).collect::<Vec<_>>(),"adapter_before":adapter_before.iter().map(|s|format!("{s:?}")).collect::<Vec<_>>(),"adapter_after":adapter_after.iter().map(|s|format!("{s:?}")).collect::<Vec<_>>(),"sink_peak":peak,"first_sink_sample":first,"source_events":events,"device_base":base,"max_callback_lateness_us":late,"route_fault":f.dsp.midi_route_faulted,"new_execution_failures":f.dsp.timeline_execution_failures-previous_failures});
    println!("NATIVE_FX_RECOVERY {report}");std::fs::write(native_root().join("route-acceptance/receipts/native-fx-reactivation.json"),serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    assert_eq!(f.dsp.midi_route_faulted,0);assert_eq!(f.dsp.timeline_execution_failures,previous_failures);assert!(peak>0.01);assert!((6528..7040).contains(&first.unwrap()));
    let ons:Vec<_>=events.iter().filter(|(_,d)|d[0]&0xf0==0x90&&d[2]>0).collect();assert_eq!(ons[0].0,base+2176);assert_eq!(ons.iter().take(4).map(|(_,d)|d[1]).collect::<Vec<_>>(),vec![60,64,67,60]);
    for (a,b) in worker_before.iter().zip(&worker_after){assert_eq!(b.latency_drift_blocks,a.latency_drift_blocks);assert_eq!(b.deadline_misses,a.deadline_misses);assert_eq!(b.faults,a.faults);assert_eq!(b.completed-a.completed,750);}
    for (a,b) in adapter_before.iter().zip(&adapter_after){assert_eq!(b.plugin_output_quanta-a.plugin_output_quanta,734);assert_eq!(b.delayed_dry_quanta-a.delayed_dry_quanta,16);assert_eq!(b.bridge_gaps,a.bridge_gaps);}
    f.finish();
}

#[test]
fn native_vst3_retrigger_chord_latch_releases_all() {
    let mut chain=native_chain(909,"Surge XT.vst3",Vec::new());
    chain.audio.set_transport(PluginTransport{playing:true,tempo:120.,..Default::default()});
    assert!(chain.audio.try_send_midi(Some(0),MidiMessage::new([0x90,60,100],0)));
    let _first_note=native_sync_block(&mut chain);
    for pitch in [60,64,67] {assert!(chain.audio.try_send_midi(Some(0),MidiMessage::new([0x90,pitch,100],0)));}
    let held:Vec<f32>=(0..100).flat_map(|_|native_sync_block(&mut chain)).collect();let peak=held.iter().fold(0f32,|p,v|p.max(v.abs()));assert!(peak>0.01);
    chain.audio.block_midi_until_epoch();let before=chain.control.stats().submitted;
    // No audio submitted during this independent worker-reset opportunity.
    thread::sleep(Duration::from_millis(100));assert_eq!(chain.control.stats().submitted,before);
    chain.audio.set_epoch(2);chain.audio.set_transport(PluginTransport{playing:false,tempo:120.,..Default::default()});
    let after:Vec<f32>=(0..750).flat_map(|_|native_sync_block(&mut chain)).collect();assert!(after.iter().all(|s|s.is_finite()));
    let tail_peak=after[after.len()-48000..].iter().fold(0f32,|p,v|p.max(v.abs()));assert!(tail_peak<1e-6,"held note resurrected after latch/epoch: {tail_peak}");
    assert_eq!(chain.control.stats().faults,0);assert_eq!(chain.control.stats().reset_faults,0);
    println!("NATIVE_RETRIGGER_CHORD_LATCH held_peak={peak} final_second_peak={tail_peak} stats={:?}",chain.control.stats());
    let receipt=serde_json::json!({"held_note_peak":peak,"note_on_pitches":[60,60,64,67],"channel":1,"silent_wait_without_submissions_ms":100,"last_second_peak":tail_peak,"no_explicit_note_off_sent_by_test":true,"scope":"combined real VST3 worker safety latch then new-epoch no-resurrection, allowing release tail"});
    std::fs::write(native_root().join("route-acceptance/receipts/native-retrigger-chord-latch.json"),serde_json::to_vec_pretty(&receipt).unwrap()).unwrap();
    assert_eq!(chain.guard.shutdown_blocking(Duration::from_secs(10)),crate::plugins::plugin_runtime::ShutdownOutcome::Joined);
}

#[test]
fn native_vst3_overload_fences_and_recovers() {
    let mut f=NativeGraphFixture::new_native(native_project(120.,false,true),true,false);f.activate(2,0);
    // Deliberately violate the callback processing budget, with genuine backends.
    // This is a controlled overload test, not a normal playback pass.
    f.check_source_output=false;let mut callbacks=0;
    while f.dsp.midi_route_faulted==0 && callbacks<128 {f.render_native_callback(2048);callbacks+=1;}
    assert_ne!(f.dsp.midi_route_faulted,0,"unpaced native overload did not reach a fence");
    assert!(f.status.plugin_midi_faulted_destinations.load(Ordering::Relaxed)>0);
    let (fenced,_,_)=f.render_native_paced(4096,512);assert!(fenced.iter().flatten().all(|v|*v==0.));
    f.wait_workers();let overload_workers:Vec<_>=f.controls.iter().map(|c|c.stats()).collect();
    let overload_adapters:Vec<_>=f.dsp.generator_endpoints.iter().flatten().map(|s|s.endpoint.stats()).collect();
    let fault_count=f.status.plugin_midi_faulted_destinations.load(Ordering::Relaxed);
    println!("NATIVE_CONTROLLED_OVERLOAD callbacks={callbacks} published_faults={fault_count} workers={overload_workers:?} adapters={overload_adapters:?}");
    f.mailbox.publish(TransportMutation::SetPlaying(false));f.render_native_paced(2048,512);f.activate(3,0);f.check_source_output=true;
    let worker_before:Vec<_>=f.controls.iter().map(|c|c.stats()).collect();let adapter_before:Vec<_>=f.dsp.generator_endpoints.iter().flatten().map(|s|s.endpoint.stats()).collect();
    let start=f.sink_audio.len();let base=f.transport.device_frame;let failures=f.dsp.timeline_execution_failures;
    let(_,events,late)=f.render_native_paced(48000,512);let sink=&f.sink_audio[start..];let peak=sink.iter().flatten().fold(0f32,|p,x|p.max(x.abs()));
    let worker_after:Vec<_>=f.controls.iter().map(|c|c.stats()).collect();let adapter_after:Vec<_>=f.dsp.generator_endpoints.iter().flatten().map(|s|s.endpoint.stats()).collect();
    let report=serde_json::json!({"scope":"intentional unpaced real-plugin overload, failclosed status publication and genuine paced restart","unpaced_callbacks":callbacks,"published_faulted_destinations":fault_count,"overload_worker_stats":overload_workers.iter().map(|s|format!("{s:?}")).collect::<Vec<_>>(),"overload_adapter_stats":overload_adapters.iter().map(|s|format!("{s:?}")).collect::<Vec<_>>(),"recovery_worker_before":worker_before.iter().map(|s|format!("{s:?}")).collect::<Vec<_>>(),"recovery_worker_after":worker_after.iter().map(|s|format!("{s:?}")).collect::<Vec<_>>(),"recovery_adapter_before":adapter_before.iter().map(|s|format!("{s:?}")).collect::<Vec<_>>(),"recovery_adapter_after":adapter_after.iter().map(|s|format!("{s:?}")).collect::<Vec<_>>(),"sink_peak_after_restart":peak,"source_events":events,"base_device_frame":base,"max_callback_lateness_us":late,"published_faults_after_restart":f.status.plugin_midi_faulted_destinations.load(Ordering::Relaxed)});
    println!("NATIVE_OVERLOAD_RECOVERY {report}");std::fs::write(native_root().join("route-acceptance/receipts/native-overload-recovery.json"),serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    assert_eq!(f.dsp.midi_route_faulted,0);assert_eq!(f.status.plugin_midi_faulted_destinations.load(Ordering::Relaxed),0);assert_eq!(f.dsp.timeline_execution_failures,failures);assert!(peak>0.01);
    let ons:Vec<_>=events.iter().filter(|(_,d)|d[0]&0xf0==0x90&&d[2]>0).collect();assert_eq!(ons[0].0,base+2176);assert_eq!(ons.iter().take(4).map(|(_,d)|d[1]).collect::<Vec<_>>(),vec![60,64,67,60]);
    for(a,b)in worker_before.iter().zip(&worker_after){assert_eq!(a.deadline_misses,b.deadline_misses);assert_eq!(a.faults,b.faults);assert_eq!(b.completed-a.completed,375);}
    for(a,b)in adapter_before.iter().zip(&adapter_after){assert_eq!(b.plugin_output_quanta-a.plugin_output_quanta,359);assert_eq!(b.delayed_dry_quanta-a.delayed_dry_quanta,16);}
    f.finish();
}
