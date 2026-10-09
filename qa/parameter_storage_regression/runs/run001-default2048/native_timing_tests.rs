// External genuine-plugin QA; no production source is modified.
fn plugin_root() -> PathBuf {
    std::env::var_os("VST3_VALIDATION_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("${VST3_VALIDATION_ROOT}"))
}
fn receipt_root() -> PathBuf {
    std::env::var_os("NATIVE_TIMING_RECEIPT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| qa_root().join("receipts"))
}
fn qa_root() -> PathBuf {
    std::env::var_os("NATIVE_TIMING_QA_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| plugin_root().join("callback-safe-optimized"))
}
fn native_chain(instance: u64, bundle: &str, state: Vec<u8>) -> PluginChain {
    let root = plugin_root();
    let descriptor = crate::plugins::PluginDescriptor {
        id: format!("native-validation:{bundle}"),
        name: bundle.into(),
        vendor: "Surge Synth Team".into(),
        path: root.join("plugins").join(bundle),
        format: crate::plugins::PluginFormat::Vst3,
        category: String::new(),
        is_instrument: bundle != "Surge XT Effects.vst3",
        verified: false,
        vst3_metadata: None,
        scan_error: None,
    };
    let mut spec = crate::plugins::plugin_runtime::PluginLoadSpec::from_descriptor(descriptor);
    spec.vst3_helper_path = Some(qa_root().join("bin/vst3-host-helper"));
    spec.initial_state = state;
    let mut chain = PluginChain::spawn_identified(
        vec![(instance, spec)],
        PluginPrepareConfig {
            sample_rate: 48000.0,
            max_block_frames: 128,
        },
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(event) = chain.control.try_next_event() {
            match event {
                crate::plugins::plugin_runtime::RuntimeEvent::SlotReady { .. } => {
                    println!("native ready {event:?}");
                    break;
                }
                crate::plugins::plugin_runtime::RuntimeEvent::SlotFault { .. } => {
                    panic!("native plugin load failed: {event:?}")
                }
                _ => {}
            }
        }
        assert!(Instant::now() < deadline, "native plugin load deadline");
        thread::sleep(Duration::from_millis(1));
    }
    while !chain
        .control
        .plugin_latency_snapshot()
        .is_some_and(|s| s.slot_is_active(0))
    {
        assert!(
            Instant::now() < deadline,
            "native active latency snapshot deadline"
        );
        thread::sleep(Duration::from_millis(1));
    }
    chain
}
fn timing_project(routed: bool) -> Project {
    let mut p = midi_route_project(true, false);
    p.tempo = 120.;
    p.song_length_beats = 32.;
    if !routed {
        p.channels.retain(|c| c.id != SOURCE_CHANNEL);
        p.plugin_instances.retain(|i| i.id != SOURCE_INSTANCE);
        for i in &mut p.plugin_instances {
            i.midi_ports = crate::plugin_midi_routing::PluginMidiPorts::default();
        }
        p.patterns.push(Pattern {
            id: 1,
            name: "Own generated C E G test notes".into(),
            length_steps: 16,
            channel_steps: vec![[false; 16]],
            notes: [(60, 6000), (64, 18000), (67, 30000)]
                .into_iter()
                .enumerate()
                .map(|(id, (note, frame))| PianoNote {
                    id: id as u64 + 1,
                    channel_id: Some(SINK_CHANNEL),
                    group_id: None,
                    note,
                    start: frame_as_beat(frame),
                    length: frame_as_beat(6000),
                    velocity: 100.0 / 127.,
                    selected: false,
                    muted: false,
                })
                .collect(),
        });
        p.clips.push(timeline_pattern_clip(1, 2., 1));
    }
    for i in &mut p.plugin_instances {
        let (bundle, uid) = match i.id {
            SOURCE_INSTANCE => ("Stochas.vst3", "ABCDEF019182FAEB70726F6A53746F63"),
            SINK_INSTANCE => ("Surge XT.vst3", "ABCDEF019182FAEB566D624153675854"),
            _ => ("Surge XT Effects.vst3", "ABCDEF019182FAEB566D624153465854"),
        };
        i.path = plugin_root().join("plugins").join(bundle);
        i.uid = uid.into();
        i.name = bundle.into();
        i.vendor = "Surge Synth Team".into();
    }
    p
}
fn timing_write(name: &str, value: &serde_json::Value) {
    std::fs::write(
        receipt_root().join(name),
        serde_json::to_vec_pretty(value).unwrap(),
    )
    .unwrap();
}
fn timing_wav(name: &str, a: &[[f32; 2]]) {
    use std::io::Write;
    let mut f = std::fs::File::create(receipt_root().join(name)).unwrap();
    let bytes = (a.len() * 4) as u32;
    f.write_all(b"RIFF").unwrap();
    f.write_all(&(36 + bytes).to_le_bytes()).unwrap();
    f.write_all(b"WAVEfmt ").unwrap();
    f.write_all(&16u32.to_le_bytes()).unwrap();
    f.write_all(&1u16.to_le_bytes()).unwrap();
    f.write_all(&2u16.to_le_bytes()).unwrap();
    f.write_all(&48000u32.to_le_bytes()).unwrap();
    f.write_all(&192000u32.to_le_bytes()).unwrap();
    f.write_all(&4u16.to_le_bytes()).unwrap();
    f.write_all(&16u16.to_le_bytes()).unwrap();
    f.write_all(b"data").unwrap();
    f.write_all(&bytes.to_le_bytes()).unwrap();
    for frame in a {
        for v in frame {
            f.write_all(&((v.clamp(-1., 1.) * 32767.) as i16).to_le_bytes())
                .unwrap();
        }
    }
}
impl NativeTimingFixture {
    fn diagnostic(&self) -> serde_json::Value {
        let p = self.dsp.plugin_processing_snapshot();
        let mut diagnostic = serde_json::json!({"prepared_plan":{"revision":p.plan.revision,"sample_rate":p.plan.sample_rate,"budget":p.plan.callback_budget_frames,"guard_quanta":p.plan.guard_quanta,"lookahead_quanta":p.plan.lookahead_quanta,"bridge_latency_frames":p.plan.bridge_latency_frames},"health":format!("{:?}",p.health),"fault":p.fault.map(|v|serde_json::json!({"endpoint_id":v.endpoint_id,"epoch":v.epoch,"expected_sequence":v.expected_sequence,"raw_callback_frames":v.raw_callback_frames,"callback_budget_frames":v.callback_budget_frames,"lookahead_quanta":v.lookahead_quanta,"timing_revision":v.timing_revision,"reason":format!("{:?}",v.reason)})),"fault_count":p.fault_count,"recovered_count":p.recovered_count,"deadline_misses":p.deadline_misses,"input_losses":p.input_losses,"output_losses":p.output_losses,"processing":format!("{:?}",self.dsp.plugin_processing_snapshot()),"published_processing":format!("{:?}",self.status.plugin_processing.snapshot(48000)),"playing":self.transport.request.playing,"epoch":self.transport.epoch,"timing":format!("{:?}",self.timing),"pdc_samples":self.dsp.graph_pdc_plan.as_ref().as_ref().map(|p|p.master_output_latency_samples()),"processing_suspended":self.dsp.plugin_processing_suspended(),"binding_exact":self.dsp.mixer_graph_binding_is_exact(),"timeline_execution_failures":self.dsp.timeline_execution_failures,"timeline_channel_revision":self.dsp.timeline_channel_revision,"timeline_channel_epoch":self.dsp.timeline_channel_epoch,"transport_epoch":self.dsp.transport_epoch,"timeline_runtime_revision":self.dsp.timeline_runtime.as_ref().and_then(|r|r.active_revision()),"timeline_runtime_epoch":self.dsp.timeline_runtime.as_ref().and_then(|r|r.active_epoch()),"plugin_topology_replan_pending":self.dsp.plugin_topology_replan_pending,"plugin_topology_revision":self.dsp.plugin_topology_revision,"mixer_graph_identity":format!("{:?}",self.dsp.mixer_graph_identity),"requested_timing_revision":self.dsp.requested_plugin_timing_revision,"captured_insert_identities":self.dsp.mixer_graph_endpoint_identities.inserts.iter().flatten().map(|s|format!("{s:?}")).collect::<Vec<_>>(),"captured_generator_identities":self.dsp.mixer_graph_endpoint_identities.generators.iter().flatten().map(|s|format!("{s:?}")).collect::<Vec<_>>(),"worker_latency_snapshots":self.controls.iter().map(|c|format!("{:?}",c.plugin_latency_snapshot())).collect::<Vec<_>>(),"worker_stats":self.controls.iter().map(|c|format!("{:?}",c.stats())).collect::<Vec<_>>(),"adapter_stats":self.adapters().iter().map(|s|format!("{s:?}")).collect::<Vec<_>>()});
        let timing = serde_json::json!({"callback_runtime_max_us":self.callback_runtime_max_us,"paced_start_lateness_max_us":self.paced_start_lateness_max_us,"paced_callbacks":self.paced_callbacks,"callback_core_interval_overruns":self.callback_core_interval_overruns,"callback_outer_interval_overruns":self.callback_outer_interval_overruns,"callback_core_headroom_min_ns":if self.callback_core_headroom_min_ns==i64::MAX{None}else{Some(self.callback_core_headroom_min_ns)},"callback_render_max_us":self.callback_render_max_us,"callback_outer_max_us":self.callback_outer_max_us,"callback_capture_max_us":self.callback_capture_max_us,"source_midi_loss_batches":self.source_midi_loss_batches,"capture_overflow":self.capture_overflow,"capture_overflow_frames":self.capture_overflow_frames,"capture_overflow_events":self.capture_overflow_events});
        diagnostic
            .as_object_mut()
            .unwrap()
            .extend(timing.as_object().unwrap().clone());
        diagnostic
    }
    fn adapters(&self) -> Vec<crate::fixed_quantum::FixedQuantumStats> {
        self.dsp
            .generator_endpoints
            .iter()
            .flatten()
            .map(|s| s.endpoint.stats())
            .chain(
                self.dsp
                    .insert_endpoints
                    .iter()
                    .flatten()
                    .map(|s| s.endpoint.stats()),
            )
            .collect()
    }
    fn callback(&mut self, frames: usize) -> Vec<[f32; 2]> {
        let outer_start = Instant::now();
        assert!(frames <= self.callback_output.len());
        let base = self.transport.device_frame;
        let core_start = Instant::now();
        self.callback_output[..frames].fill([0.; 2]);
        self.dsp.set_device_frame(base);
        self.dsp.set_callback_transport_boundary(
            MidiRecordClockAnchor {
                device_frame: base,
                timeline_frame: self.transport.timeline_frame,
                transport_epoch: self.transport.epoch,
                loop_count: self.transport.loop_count,
            },
            self.transport.request.playing,
        );
        self.dsp.raw_callback_frames = frames;
        self.dsp.requested_plugin_timing_revision = self
            .status
            .plugin_processing
            .requested_revision
            .load(Ordering::Acquire);
        self.dsp.apply_pending_timeline_commands();
        self.dsp
            .refresh_pdc_plan(&self.status, frames.min(MAX_MIXER_BLOCK_FRAMES));
        self.transport
            .apply_pending_timeline_activation(&self.status, &mut self.dsp);
        self.transport
            .apply_latest_request(&self.mailbox, &self.status, &mut self.dsp);
        let admitted = self.dsp.admit_plugin_callback(frames);
        if !admitted {
            self.transport.request.playing = false;
            self.transport.request.loop_enabled = false;
            self.transport.device_frame += frames as u64;
            self.transport.publish(&self.status);
            self.dsp.publish_plugin_epoch_status(&self.status);
        } else {
            self.dsp.refresh_pdc_plan(&self.status, frames);
            self.transport
                .apply_pending_timeline_activation(&self.status, &mut self.dsp);
            let render_start = Instant::now();
            let mut written = 0;
            let output = &mut self.callback_output;
            render_transport_chunk(
                &mut self.dsp,
                &self.status,
                &self.mailbox,
                &mut self.transport,
                frames,
                |_, block| {
                    output[written..written + block.len()].copy_from_slice(block);
                    written += block.len();
                },
            );
            assert_eq!(written, frames);
            self.callback_render_max_us = self
                .callback_render_max_us
                .max(render_start.elapsed().as_micros());
        }
        let core_elapsed = core_start.elapsed();
        let interval_ns = Duration::from_secs_f64(frames as f64 / 48000.).as_nanos() as i64;
        let headroom = interval_ns - core_elapsed.as_nanos().min(i64::MAX as u128) as i64;
        self.callback_core_headroom_min_ns = self.callback_core_headroom_min_ns.min(headroom);
        self.callback_core_interval_overruns += u64::from(headroom < 0);
        self.callback_runtime_max_us = self.callback_runtime_max_us.max(core_elapsed.as_micros());
        // Everything below is outside the measured callback core. Capture storage was
        // allocated before activation, and overflow is explicit rather than truncating.
        let capture_start = Instant::now();
        if self.captured_sink.len() + frames > self.captured_sink.capacity() {
            self.capture_overflow = true;
            self.capture_overflow_frames += frames as u64;
        } else if admitted && self.dsp.meter_graph_rendered {
            self.captured_sink.extend_from_slice(
                &self.dsp.track_block
                    [2 * MAX_MIXER_BLOCK_FRAMES..2 * MAX_MIXER_BLOCK_FRAMES + frames],
            );
        } else {
            self.captured_sink
                .extend(std::iter::repeat_n([0.; 2], frames));
        }
        if admitted {
            if let Some(source) = self.dsp.find_generator_slot(SOURCE_CHANNEL) {
                let b = self.dsp.generator_endpoints[source]
                    .as_ref()
                    .unwrap()
                    .endpoint
                    .adapter
                    .midi_output();
                if self.captured_events.len() + b.len > self.captured_events.capacity() {
                    self.capture_overflow = true;
                    self.capture_overflow_events += b.len as u64;
                } else {
                    self.captured_events.extend(
                        b.events[..b.len]
                            .iter()
                            .map(|e| (base + u64::from(e.message.sample_offset), e.message.data)),
                    );
                }
                if b.lost {
                    self.source_midi_loss_batches += 1;
                }
            }
        }
        self.callback_capture_max_us = self
            .callback_capture_max_us
            .max(capture_start.elapsed().as_micros());
        // The convenience return copy belongs to the outer test scheduler, not CPAL.
        let delivered = self.callback_output[..frames].to_vec();
        let outer_elapsed = outer_start.elapsed();
        self.callback_outer_interval_overruns +=
            u64::from(outer_elapsed.as_nanos() > interval_ns as u128);
        self.callback_outer_max_us = self.callback_outer_max_us.max(outer_elapsed.as_micros());
        delivered
    }
    fn paced(&mut self, frames: usize, parts: &[usize]) -> Vec<[f32; 2]> {
        let mut out = Vec::with_capacity(frames);
        let mut deadline = Instant::now();
        let mut i = 0;
        while out.len() < frames {
            self.paced_start_lateness_max_us = self.paced_start_lateness_max_us.max(
                Instant::now()
                    .saturating_duration_since(deadline)
                    .as_micros(),
            );
            self.paced_callbacks += 1;
            let n = parts[i % parts.len()].min(frames - out.len());
            out.extend(self.callback(n));
            deadline += Duration::from_secs_f64(n as f64 / 48000.);
            let target = deadline
                + if i % 7 == 6 {
                    Duration::from_micros(300)
                } else {
                    Duration::ZERO
                };
            if let Some(d) = target.checked_duration_since(Instant::now()) {
                thread::sleep(d);
            }
            i += 1;
        }
        self.wait_workers();
        out
    }
    fn load_own_pattern(&mut self) {
        let i = self.dsp.find_generator_slot(SOURCE_CHANNEL).unwrap();
        assert!(self.controls[i].load_state(
            0,
            std::fs::read(plugin_root().join("receipts/stochas-qa-pattern.state")).unwrap()
        ));
        assert!(self.controls[i].request_state_tagged(0, 888));
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(crate::plugins::plugin_runtime::RuntimeEvent::State {
                request_id: 888,
                ..
            }) = self.controls[i].try_next_event()
            {
                break;
            }
            assert!(Instant::now() < deadline, "Stochas pattern state ack");
            thread::sleep(Duration::from_millis(1));
        }
    }
    fn load_blank_pattern(&mut self) {
        let i = self.dsp.find_generator_slot(SOURCE_CHANNEL).unwrap();
        assert!(self.controls[i].load_state(
            0,
            std::fs::read(plugin_root().join("receipts/stochas-initial-state.bin")).unwrap()
        ));
        assert!(self.controls[i].request_state_tagged(0, 888));
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(crate::plugins::plugin_runtime::RuntimeEvent::State {
                request_id: 888,
                ..
            }) = self.controls[i].try_next_event()
            {
                break;
            }
            assert!(Instant::now() < deadline, "Stochas pattern state ack");
            thread::sleep(Duration::from_millis(1));
        }
    }
    fn stopped_retry(&mut self, budget: u32, epoch: u64) {
        self.mailbox.publish(TransportMutation::SetPlaying(false));
        self.transport
            .apply_latest_request(&self.mailbox, &self.status, &mut self.dsp);
        self.select_timing_profile(budget);
        self.activation_playing = false;
        self.activate(epoch, 0);
        assert!(!self.transport.request.playing, "Retry must remain stopped");
        assert!(self.dsp.plugin_fault.is_none());
        let silent = self.paced(128, &[128.min(budget as usize)]);
        assert!(silent.iter().flatten().all(|v| *v == 0.));
        assert!(!self.transport.request.playing);
    }
}
fn run_timing_profile(budget: u32, routed: bool, changing: bool) -> bool {
    let label = format!("b{budget}_routed{routed}_changing{changing}");
    let mut f = NativeTimingFixture::new_native(timing_project(routed), routed);
    f.select_timing_profile(budget);
    let mut cold_trace =
        vec![serde_json::json!({"stage":"ready_before_activation","diagnostic":f.diagnostic()})];
    f.activate(2, 48000);
    cold_trace.push(serde_json::json!({"stage":"after_activation","diagnostic":f.diagnostic()}));
    let parts = if changing {
        vec![budget as usize, 31, (budget as usize / 2).max(1), 127]
    } else {
        vec![budget as usize]
    };
    // The actual FX lazily publishes32 samples: observe its fence, never mask it.
    for callback in 0..64 {
        f.paced(budget as usize, &[budget as usize]);
        cold_trace.push(serde_json::json!({"stage":"after_cold_callback","callback":callback,"diagnostic":f.diagnostic()}));
        if f.dsp.plugin_fault.is_some() || f.dsp.plugin_processing_suspended() {
            break;
        }
    }
    timing_write(
        &format!("{label}-cold-trace.json"),
        &serde_json::json!({"case":label,"trace":cold_trace}),
    );
    let cold = f.diagnostic();
    let cold_fault = f.dsp.plugin_fault;
    if !cold_fault.is_some_and(|v| {
        matches!(
            v.reason,
            PluginProcessingFaultReason::LatencyDrift
                | PluginProcessingFaultReason::EndpointChanged
        )
    }) || !f
        .controls
        .last()
        .unwrap()
        .plugin_latency_snapshot()
        .is_some_and(|s| s.total_plugin_latency_samples == 32)
    {
        let report = serde_json::json!({"case":label,"outcome":"FAIL","phase":"cold FX must show genuine latency fence","diagnostic":cold});
        timing_write(&format!("{label}.json"), &report);
        println!("NATIVE_TIMING_CASE {report}");
        f.finish();
        return false;
    }
    f.stopped_retry(budget, 3);
    let stopped = f.diagnostic();
    if routed {
        f.load_own_pattern();
    }
    f.activation_playing = true;
    f.activate(4, 0);
    let workers_before: Vec<_> = f.controls.iter().map(|c| c.stats()).collect();
    let adapters_before = f.adapters();
    let start = f.captured_sink.len();
    let events_start = f.captured_events.len();
    let base = f.transport.device_frame;
    let declared = f
        .dsp
        .graph_pdc_plan
        .as_ref()
        .as_ref()
        .unwrap()
        .master_output_latency_samples();
    let cpu_threads = std::env::var("NATIVE_CPU_LOAD_THREADS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0)
        .min(8);
    let load_threads: Vec<_> = (0..cpu_threads)
        .map(|_| {
            thread::spawn(|| {
                let end = Instant::now() + Duration::from_millis(1100);
                let mut x = 0.314159f64;
                while Instant::now() < end {
                    for _ in 0..1024 {
                        x = std::hint::black_box((x + 0.1).sin().cos());
                    }
                }
                std::hint::black_box(x)
            })
        })
        .collect();
    f.callback_runtime_max_us = 0;
    f.source_midi_loss_batches = 0;
    f.callback_outer_max_us = 0;
    f.callback_render_max_us = 0;
    f.callback_core_interval_overruns = 0;
    f.callback_outer_interval_overruns = 0;
    f.callback_core_headroom_min_ns = i64::MAX;
    f.callback_capture_max_us = 0;
    f.paced_start_lateness_max_us = 0;
    f.paced_callbacks = 0;
    let audio = f.paced(48000, &parts);
    for t in load_threads {
        t.join().unwrap();
    }
    let sink = &f.captured_sink[start..];
    let events = &f.captured_events[events_start..];
    let first = sink.iter().position(|f| f[0].abs().max(f[1].abs()) > 1e-6);
    let finite = sink.iter().flatten().all(|v| v.is_finite());
    let peak = sink.iter().flatten().fold(0f32, |p, v| p.max(v.abs()));
    let workers_after: Vec<_> = f.controls.iter().map(|c| c.stats()).collect();
    let adapters_after = f.adapters();
    let k = u64::from(f.timing.lookahead_quanta);
    let l = u64::from(f.timing.bridge_latency_frames);
    let expected_pdc = l * if routed { 3 } else { 2 } + 32;
    let mut errors = Vec::new();
    if f.source_midi_loss_batches != 0 {
        errors.push("source MIDI output loss in measured segment".into());
    }
    if f.capture_overflow {
        errors.push("bounded capture overflow; no acceptance claim".into());
    }
    if f.dsp.plugin_fault.is_some() {
        errors.push("processing fault during paced run".to_string());
    }
    if !finite || peak <= 0.001 {
        errors.push("nonfinite or silent actual sink".into());
    }
    if declared != expected_pdc {
        errors.push(format!("PDC{declared} != expected{expected_pdc}"));
    }
    let first_input_note = if routed { 0 } else { 6000 };
    if !first.is_some_and(|n| {
        n as u64 >= declared + first_input_note && (n as u64) < declared + first_input_note + 512
    }) {
        errors.push(format!("first sink sample{first:?}, declaredPDC{declared}"));
    }
    for (i, (a, b)) in workers_before.iter().zip(&workers_after).enumerate() {
        if b.completed - a.completed != 375
            || b.deadline_misses != a.deadline_misses
            || b.input_overflows != a.input_overflows
            || b.output_overflows != a.output_overflows
            || b.faults != a.faults
            || b.latency_drift_blocks != a.latency_drift_blocks
        {
            errors.push(format!("worker{i} delivery mismatch"));
        }
    }
    for (i, (a, b)) in adapters_before.iter().zip(&adapters_after).enumerate() {
        if b.plugin_output_quanta - a.plugin_output_quanta != 375 - k
            || b.delayed_dry_quanta - a.delayed_dry_quanta != k
            || b.bridge_gaps != a.bridge_gaps
            || b.latency_drift_quanta != a.latency_drift_quanta
        {
            errors.push(format!("adapter{i} provenance mismatch"));
        }
    }
    let ons: Vec<_> = events
        .iter()
        .filter(|(_, d)| d[0] & 0xf0 == 0x90 && d[2] > 0)
        .collect();
    let offs = events.iter().filter(|(_, d)| d[0] & 0xf0 == 0x80).count();
    if routed {
        if ons.len() != 8
            || offs != 8
            || ons.first().map(|v| v.0) != Some(base + l)
            || ons.iter().take(4).map(|(_, d)| d[1]).collect::<Vec<_>>() != vec![60, 64, 67, 60]
        {
            errors.push("source event count/onset/pitch mismatch".into());
        }
        for p in ons.windows(2) {
            if (p[1].0 as i64 - p[0].0 as i64 - 6000).abs() > 1 {
                errors.push("source tempo spacing mismatch".into());
            }
        }
    }
    let expected_sink_events = if routed { 32 } else { 22 };
    if adapters_after[0].frame_events_staged - adapters_before[0].frame_events_staged
        != expected_sink_events
    {
        errors.push(format!(
            "sink staged event count {}, expected{}",
            adapters_after[0].frame_events_staged - adapters_before[0].frame_events_staged,
            expected_sink_events
        ));
    }
    let report = serde_json::json!({"case":label,"outcome":if errors.is_empty(){"PASS"}else{"FAIL"},"errors":errors,"budget":budget,"callback_core_interval_budget_pass":f.callback_core_interval_overruns==0,"cpu_load_threads":cpu_threads,"routed":routed,"changing":changing,"partitions":parts,"cold_fence":cold,"stopped_retry":stopped,"final":f.diagnostic(),"timing_bridge_samples":l,"expected_pdc_samples":expected_pdc,"declared_pdc_samples":declared,"first_input_note_frame":first_input_note,"first_sink_sample":first,"sink_peak":peak,"source_events":events,"source_note_on_count":ons.len(),"source_note_off_count":offs,"workers_before":workers_before.iter().map(|x|format!("{x:?}")).collect::<Vec<_>>(),"workers_after":workers_after.iter().map(|x|format!("{x:?}")).collect::<Vec<_>>(),"adapters_before":adapters_before.iter().map(|x|format!("{x:?}")).collect::<Vec<_>>(),"adapters_after":adapters_after.iter().map(|x|format!("{x:?}")).collect::<Vec<_>>()});
    timing_write(&format!("{label}.json"), &report);
    println!("NATIVE_TIMING_CASE {report}");
    timing_wav(&format!("{label}-sink.wav"), sink);
    timing_wav(&format!("{label}-master.wav"), &audio);
    let passed = errors.is_empty();
    f.finish();
    passed
}
#[test]
fn native_timing_all_exposed_profiles() {
    let mut failures = Vec::new();
    for b in [128, 256, 512, 2048] {
        if let Ok(filter) = std::env::var("NATIVE_TIMING_BUDGETS") {
            let requested: Vec<u32> = filter
                .split(',')
                .map(|v| v.parse().expect("numeric budgets"))
                .collect();
            assert!(
                !requested.is_empty()
                    && requested.iter().all(|b| [128, 256, 512, 2048].contains(b))
            );
            if !requested.contains(&b) {
                continue;
            }
        }
        for routed in [true, false] {
            for changing in [false, true] {
                if let Ok(filter) = std::env::var("NATIVE_TIMING_CASE") {
                    if !format!("b{b}_routed{routed}_changing{changing}").contains(&filter) {
                        continue;
                    }
                }
                if !run_timing_profile(b, routed, changing) {
                    failures.push((b, routed, changing));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "native timing profiles failed {failures:?}"
    );
}
fn warm_native_timing(budget: u32) -> NativeTimingFixture {
    let mut f = NativeTimingFixture::new_native(timing_project(true), true);
    f.select_timing_profile(budget);
    f.activate(2, 48000);
    for _ in 0..64 {
        f.paced(budget as usize, &[budget as usize]);
        if f.dsp.plugin_fault.is_some() {
            break;
        }
    }
    let cold = f.diagnostic();
    timing_write(&format!("admission-b{budget}-cold.json"), &cold);
    assert!(
        f.dsp.plugin_fault.is_some_and(|v| matches!(
            v.reason,
            PluginProcessingFaultReason::LatencyDrift
                | PluginProcessingFaultReason::EndpointChanged
        )) && f
            .controls
            .last()
            .unwrap()
            .plugin_latency_snapshot()
            .is_some_and(|s| s.total_plugin_latency_samples == 32),
        "unexpected cold fault {cold}"
    );
    f.stopped_retry(budget, 3);
    f.load_own_pattern();
    f.activation_playing = true;
    f.activate(4, 0);
    f
}
#[test]
fn native_timing_budget_admission_stopped_retry() {
    let mut failures = Vec::new();
    for budget in [128, 256, 512, 2048] {
        let mut f = warm_native_timing(budget);
        let before: Vec<_> = f.controls.iter().map(|c| c.stats().submitted).collect();
        let rejected = f.callback(budget as usize + 1);
        let fault = f.dsp.plugin_fault.unwrap();
        let rejected_diag = f.diagnostic();
        assert!(rejected.iter().flatten().all(|v| *v == 0.));
        assert!(!f.transport.request.playing);
        assert_eq!(
            f.controls
                .iter()
                .map(|c| c.stats().submitted)
                .collect::<Vec<_>>(),
            before
        );
        assert_eq!(fault.raw_callback_frames, budget + 1);
        assert_eq!(fault.callback_budget_frames, budget);
        assert_eq!(
            fault.reason,
            if budget == 2048 {
                PluginProcessingFaultReason::UnsupportedCallback
            } else {
                PluginProcessingFaultReason::CallbackBudgetExceeded
            }
        );
        let previous_revision = f.timing.revision;
        f.stopped_retry(budget, 5);
        assert!(f.timing.revision > previous_revision);
        let stopped = f.diagnostic();
        f.activation_playing = true;
        f.activate(6, 0);
        let worker_before: Vec<_> = f.controls.iter().map(|c| c.stats()).collect();
        let start = f.captured_sink.len();
        let _ = f.paced(24064, &[budget as usize]);
        let peak = f.captured_sink[start..]
            .iter()
            .flatten()
            .fold(0f32, |p, v| p.max(v.abs()));
        let result = serde_json::json!({"case":format!("raw-budget-{budget}-plus1"),"rejected_before_any_submission":true,"rejected":rejected_diag,"stopped_retry":stopped,"restarted":f.diagnostic(),"sink_peak_after_explicit_restart":peak,"worker_before":worker_before.iter().map(|v|format!("{v:?}")).collect::<Vec<_>>()});
        timing_write(&format!("admission-b{budget}.json"), &result);
        println!("NATIVE_TIMING_ADMISSION {result}");
        if f.capture_overflow || f.dsp.plugin_fault.is_some() || peak <= 0.01 {
            failures.push(budget);
        }
        f.finish();
    }
    assert!(
        failures.is_empty(),
        "budget admission succeeded but explicit restart failed for {failures:?}"
    );
}
#[test]
fn native_timing_unpaced_overload_cleanup_and_retry() {
    let budget = 512;
    let mut f = warm_native_timing(budget);
    let warm = f.paced(8960, &[512]);
    assert!(warm.iter().flatten().any(|v| v.abs() > 0.01));
    let mut count = 0;
    while f.dsp.plugin_fault.is_none() && count < 128 {
        f.callback(budget as usize);
        count += 1;
    }
    let overloaded = f.diagnostic();
    timing_write("controlled-overload-before-retry.json", &overloaded);
    assert!(f.dsp.plugin_fault.is_some());
    let stopped_output = f.paced(2048, &[512]);
    assert!(stopped_output.iter().flatten().all(|v| *v == 0.));
    assert!(!f.transport.request.playing);
    f.stopped_retry(budget, 5);
    let stopped = f.diagnostic();
    f.load_blank_pattern();
    f.activation_playing = true;
    f.activate(6, 0);
    let quiet_start = f.captured_sink.len();
    let events_start = f.captured_events.len();
    let blank_master = f.paced(192000, &[512]);
    let blank_sink = &f.captured_sink[quiet_start..];
    let quiet_late_peak = blank_sink[144000..]
        .iter()
        .flatten()
        .fold(0f32, |p, v| p.max(v.abs()));
    let quiet_source_events = f.captured_events.len() - events_start;
    let quarter_stats:Vec<_>=blank_sink.chunks(12000).enumerate().map(|(i,a)|serde_json::json!({"quarter_second":i,"peak":a.iter().flatten().fold(0f32,|p,v|p.max(v.abs())),"rms":(a.iter().flatten().map(|v|f64::from(*v).powi(2)).sum::<f64>()/(a.len()*2)as f64).sqrt()})).collect();
    timing_wav("controlled-overload-blank-sink.wav", blank_sink);
    timing_wav("controlled-overload-blank-master.wav", &blank_master);
    let cleanup = serde_json::json!({"case":"overload cleanup with genuinely blank source","observation_seconds":4,"last_second_sink_peak":quiet_late_peak,"quarter_second_sink_stats":quarter_stats,"source_event_count":quiet_source_events,"diagnostic":f.diagnostic()});
    timing_write("controlled-overload-blank-cleanup.json", &cleanup);
    let cleanup_passed = !f.capture_overflow
        && quiet_source_events == 0
        && quiet_late_peak < 1e-6
        && f.dsp.plugin_fault.is_none();
    let cleanup_had_fault = f.dsp.plugin_fault.is_some();
    f.stopped_retry(budget, 7);
    f.load_own_pattern();
    f.activation_playing = true;
    f.activate(8, 0);
    let before: Vec<_> = f.controls.iter().map(|c| c.stats()).collect();
    let a_before = f.adapters();
    let start = f.captured_sink.len();
    f.paced(48000, &[512]);
    let peak = f.captured_sink[start..]
        .iter()
        .flatten()
        .fold(0f32, |p, v| p.max(v.abs()));
    let after: Vec<_> = f.controls.iter().map(|c| c.stats()).collect();
    let a_after = f.adapters();
    let result = serde_json::json!({"case":"deliberate unpaced raw callback burst","callbacks_until_fault":count,"cleanup_had_additional_deadline":cleanup_had_fault,"cleanup_passed":cleanup_passed,"overload":overloaded,"stopped_retry":stopped,"recovery":f.diagnostic(),"sink_peak":peak,"worker_before":before.iter().map(|v|format!("{v:?}")).collect::<Vec<_>>(),"worker_after":after.iter().map(|v|format!("{v:?}")).collect::<Vec<_>>(),"adapter_before":a_before.iter().map(|v|format!("{v:?}")).collect::<Vec<_>>(),"adapter_after":a_after.iter().map(|v|format!("{v:?}")).collect::<Vec<_>>()});
    timing_write("controlled-overload-recovery.json", &result);
    println!("NATIVE_TIMING_OVERLOAD {result}");
    assert!(!f.capture_overflow);
    assert!(f.dsp.plugin_fault.is_none());
    assert!(peak > 0.01);
    for (a, b) in before.iter().zip(&after) {
        assert_eq!(b.deadline_misses, a.deadline_misses);
        assert_eq!(b.completed - a.completed, 375);
    }
    for (a, b) in a_before.iter().zip(&a_after) {
        assert_eq!(
            b.plugin_output_quanta - a.plugin_output_quanta,
            375 - u64::from(f.timing.lookahead_quanta)
        );
    }
    f.finish();
    assert!(
        cleanup_passed,
        "four-second cleanup had a retained fault or nonzero source MIDI/tail"
    );
}
fn fresh_parameter(chain: &mut PluginChainControl, id: u32, tag: u64) -> f32 {
    assert!(chain.query_parameter_tagged(0, id, tag));
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(crate::plugins::plugin_runtime::RuntimeEvent::ParameterValue {
            request_id,
            value,
            ..
        }) = chain.try_next_event()
        {
            if request_id == tag {
                return value;
            }
        }
        assert!(Instant::now() < until);
        thread::sleep(Duration::from_millis(1));
    }
}
fn fresh_state(chain: &mut PluginChainControl, tag: u64) -> Vec<u8> {
    assert!(chain.request_state_tagged(0, tag));
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(crate::plugins::plugin_runtime::RuntimeEvent::State {
            request_id, bytes, ..
        }) = chain.try_next_event()
        {
            if request_id == tag {
                return bytes;
            }
        }
        assert!(Instant::now() < until);
        thread::sleep(Duration::from_millis(1));
    }
}
fn fresh_volume_tag(state: &[u8]) -> String {
    let text = String::from_utf8_lossy(state);
    let a = text
        .find("<volume ")
        .expect("Surge component XML volume field");
    let b = text[a..].find("/>").unwrap() + a + 2;
    text[a..b].to_owned()
}
fn fresh_volume_db(tag: &str) -> f64 {
    let start = tag.find("value=\"").unwrap() + 7;
    let end = tag[start..].find('"').unwrap() + start;
    tag[start..end].parse().unwrap()
}
fn fresh_sync_block(chain: &mut PluginChain) -> [f32; 128] {
    let zero = [0.; 128];
    assert!(matches!(
        chain.audio.try_submit(&zero, &zero),
        crate::plugins::plugin_runtime::SubmitStatus::Submitted { .. }
    ));
    let mut l = [0.; 128];
    let mut r = [0.; 128];
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        match chain.audio.try_receive(&mut l, &mut r) {
            crate::plugins::plugin_runtime::ReceiveStatus::Empty => {
                assert!(Instant::now() < until);
                thread::sleep(Duration::from_millis(1));
            }
            crate::plugins::plugin_runtime::ReceiveStatus::Processed { .. } => return l,
            x => panic!("fresh-state render status{x:?}"),
        }
    }
}
#[test]
fn native_timing_fresh_restored_surge_first_note_and_state() {
    let input = std::fs::read(qa_root().join("inputs/surge-native-edited.state")).unwrap();
    let input_volume = fresh_volume_tag(&input);
    assert!((fresh_volume_db(&input_volume) + 6.27905654907227).abs() < 1e-5);
    // initial_state is supplied at construction, before this instance's first Process.
    let mut chain = native_chain(99001, "Surge XT.vst3", input);
    let before_controller = fresh_parameter(&mut chain.control, 1336600346, 9001);
    let before = fresh_state(&mut chain.control, 9002);
    let before_volume = fresh_volume_tag(&before);
    chain.audio.set_transport(PluginTransport {
        playing: true,
        tempo: 120.,
        ..Default::default()
    });
    assert!(
        chain
            .audio
            .try_send_midi(Some(0), MidiMessage::new([0x90, 60, 100], 0))
    );
    let first: Vec<f32> = (0..8).flat_map(|_| fresh_sync_block(&mut chain)).collect();
    let peak = first.iter().fold(0f32, |p, v| p.max(v.abs()));
    let first_nonzero = first.iter().position(|v| v.abs() > 1e-6);
    let after_controller = fresh_parameter(&mut chain.control, 1336600346, 9003);
    let after = fresh_state(&mut chain.control, 9004);
    let after_volume = fresh_volume_tag(&after);
    let result = serde_json::json!({"case":"fresh Surge loaded with captured UI state before first Process","scope":"actual production PluginChain/backend state and first-note proof, not a paced graph or GUI test","input_state_sha256":"104bc8ab0945eda2b2873ac43da12dfff67f1e4cbb4863b6827a220740d35a42","parameter_id":1336600346u32,"expected_normalized":0.8691863417625427f64,"controller_before":before_controller,"controller_after":after_controller,"input_component_volume":input_volume,"before_component_volume":before_volume,"after_component_volume":after_volume,"one_note_on_only":[144,60,100],"first_note_frames":1024,"first_note_peak":peak,"first_nonzero":first_nonzero,"worker_stats":format!("{:?}",chain.control.stats())});
    timing_write("fresh-restored-surge.json", &result);
    println!("NATIVE_FRESH_SURGE {result}");
    assert!(first.iter().all(|v| v.is_finite()));
    assert!(peak > 0.001, "first genuine NoteOn produced no audio");
    assert!((before_controller - 0.86918634).abs() < 1e-6);
    assert!((after_controller - 0.86918634).abs() < 1e-6);
    assert!((fresh_volume_db(&before_volume) + 6.27905654907227).abs() < 1e-5);
    assert!((fresh_volume_db(&after_volume) + 6.27905654907227).abs() < 1e-5);
    assert!(
        chain
            .audio
            .try_send_midi(Some(0), MidiMessage::new([0x80, 60, 0], 0))
    );
    for _ in 0..64 {
        fresh_sync_block(&mut chain);
    }
    assert_eq!(
        chain.guard.shutdown_blocking(Duration::from_secs(10)),
        crate::plugins::plugin_runtime::ShutdownOutcome::Joined
    );
}
