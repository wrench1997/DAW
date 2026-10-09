// Included as a child of audio::tests so these integration cases share the existing
// project/Timeline fixtures. Workers are real PluginChain workers. The barriers
// are test-driver waits between device callbacks, never waits in the audio path;
// these tests establish deterministic semantics, not real-time scheduling claims.
use super::*;
fn route_bridge_frames() -> u32 {
    PreparedPluginTimingPlan::conservative(48000)
        .unwrap()
        .bridge_latency_frames
}
use crate::plugins::plugin_runtime::{MAX_PLUGIN_OUTPUT_EVENTS, PluginMidiBatch, PluginTransport};
use std::sync::Mutex;

const SOURCE_CHANNEL: u32 = 11;
const SINK_CHANNEL: u32 = 22;
const SOURCE_INSTANCE: u64 = 1_001;
const SINK_INSTANCE: u64 = 2_002;
const INSERT_INSTANCE: u64 = 3_003;
const MIDI_TEST_MAX_DELAY: u32 = 8_192;
const SOURCE_AUDIO: f32 = 0.125;
const SINK_NOTE_AUDIO: f32 = 0.25;
const INSERT_GAIN: f32 = 0.5;

#[derive(Clone, Copy, Debug, PartialEq)]
struct ObservedMidi {
    transport: PluginTransport,
    message: MidiMessage,
}

impl ObservedMidi {
    fn content_frame(self) -> i64 {
        self.transport.sample_position + i64::from(self.message.sample_offset)
    }
}

#[derive(Default)]
struct MidiRouteProbe {
    transports: Mutex<Vec<PluginTransport>>,
    notes: Mutex<Vec<ObservedMidi>>,
    active_notes: AtomicU32,
    resets: AtomicU32,
    failure: AtomicU32,
    latency_frames: AtomicU32,
}

#[derive(Clone, Copy)]
enum MidiTestRole {
    Source,
    Sink,
    Insert,
}

struct MidiRouteBackend {
    role: MidiTestRole,
    capabilities: (bool, bool),
    probe: Arc<MidiRouteProbe>,
    transport: PluginTransport,
    generated: Vec<(i64, [u8; 3])>,
    pending: Vec<MidiMessage>,
    output: PluginMidiBatch,
    active: [bool; 128],
}

impl PluginBackend for MidiRouteBackend {
    fn name(&self) -> &str {
        "production MIDI-route integration fixture"
    }

    fn prepare(&mut self, _config: PluginPrepareConfig) -> Result<(), String> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while self.probe.failure.load(Ordering::Acquire) == 5 {
            if std::time::Instant::now() >= deadline { return Err("fixture preparation release timed out".into()); }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        if self.probe.failure.load(Ordering::Acquire) == 4 { return Err("deliberate candidate prepare/restore failure".into()); }
        Ok(())
    }

    fn midi_capabilities(&self) -> (bool, bool) {
        self.capabilities
    }

    fn set_transport(&mut self, transport: PluginTransport) -> Result<(), String> {
        self.transport = transport;
        self.probe.transports.lock().unwrap().push(transport);
        Ok(())
    }

    fn send_midi(&mut self, message: MidiMessage) -> Result<(), String> {
        match message.data[0] & 0xf0 {
            0x80 | 0x90 => self.pending.push(message),
            0xb0 if matches!(message.data[1], 120 | 123) => {
                // Resets are observable even when no more blocks are submitted.
                self.active.fill(false);
                self.pending.clear();
                self.probe.active_notes.store(0, Ordering::Release);
            }
            _ => {}
        }
        Ok(())
    }

    fn process(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        frames: usize,
    ) -> Result<(), String> {
        if matches!(self.role, MidiTestRole::Source) {
            match self.probe.failure.load(Ordering::Acquire) {
                2 => return Err("deliberate source process failure".into()),
                3 => panic!("deliberate source process panic"),
                _ => {}
            }
        }
        self.pending.sort_by_key(|message| message.sample_offset);
        {
            let mut observed = self.probe.notes.lock().unwrap();
            observed.extend(self.pending.iter().copied().map(|message| ObservedMidi {
                transport: self.transport,
                message,
            }));
        }
        match self.role {
            MidiTestRole::Source => {
                self.output = PluginMidiBatch::default();
                for mut message in self.pending.drain(..) {
                    // A distinct emitted pitch proves the sink receives plug-in output,
                    // rather than a second copy of the source's Timeline input.
                    message.data[1] = message.data[1].saturating_add(12).min(127);
                    self.output.push(0, message);
                }
                if self.transport.playing {
                    for &(frame, data) in &self.generated {
                        let offset = frame - self.transport.sample_position;
                        if (0..frames as i64).contains(&offset) {
                            self.output.push(0, MidiMessage::new(data, offset as usize));
                        }
                    }
                }
                self.output.events[..self.output.len]
                    .sort_by_key(|event| event.message.sample_offset);
                left[..frames].fill(SOURCE_AUDIO);
                right[..frames].fill(SOURCE_AUDIO);
            }
            MidiTestRole::Sink => {
                let mut next = 0;
                for frame in 0..frames {
                    while next < self.pending.len()
                        && usize::from(self.pending[next].sample_offset) == frame
                    {
                        let message = self.pending[next];
                        self.active[usize::from(message.data[1])] =
                            message.data[0] & 0xf0 == 0x90 && message.data[2] != 0;
                        next += 1;
                    }
                    let count = self.active.iter().filter(|active| **active).count();
                    left[frame] = count as f32 * SINK_NOTE_AUDIO;
                    right[frame] = left[frame];
                }
                self.pending.clear();
                self.probe.active_notes.store(
                    self.active.iter().filter(|active| **active).count() as u32,
                    Ordering::Release,
                );
            }
            MidiTestRole::Insert => {
                for sample in left[..frames].iter_mut().chain(&mut right[..frames]) {
                    *sample *= INSERT_GAIN;
                }
            }
        }
        Ok(())
    }

    fn drain_midi_output(&mut self, batch: &mut PluginMidiBatch, slot: u8, _frames: usize) {
        if !matches!(self.role, MidiTestRole::Source) {
            // An instrument/FX may emit unrepresentable events on an unused output.
            // They must not invalidate successfully delivered audio or input MIDI.
            if self.probe.failure.load(Ordering::Acquire) == 4 {
                batch.lost = true;
            }
            return;
        }
        for event in &self.output.events[..self.output.len] {
            batch.push(slot, event.message);
        }
        if self.probe.failure.load(Ordering::Acquire) == 1 {
            // Exercise the real fixed-capacity batch overflow rather than injecting
            // an already-lost callback result into the adapter.
            for _ in 0..=MAX_PLUGIN_OUTPUT_EVENTS {
                batch.push(slot, MidiMessage::new([0x90, 99, 100], 0));
            }
        }
        self.output = PluginMidiBatch::default();
    }

    fn reset_processing(&mut self) -> Result<(), String> {
        self.pending.clear();
        self.output = PluginMidiBatch::default();
        self.active.fill(false);
        self.probe.active_notes.store(0, Ordering::Release);
        self.probe.resets.fetch_add(1, Ordering::Release);
        Ok(())
    }

    fn set_parameter(&mut self, _id: u32, _normalized: f32) -> Result<(), String> {
        Ok(())
    }
    fn get_parameter(&mut self, _id: u32) -> Result<f32, String> {
        Ok(0.0)
    }
    fn save_state(&mut self) -> Result<Vec<u8>, String> {
        Ok(Vec::new())
    }
    fn load_state(&mut self, _state: &[u8]) -> Result<(), String> {
        Ok(())
    }
    fn latency_samples(&self) -> u32 {
        self.probe.latency_frames.load(Ordering::Acquire)
    }
    fn tail_samples(&self) -> u32 {
        0
    }
}

fn midi_route_chain(
    instance: u64,
    role: MidiTestRole,
    capabilities: (bool, bool),
    probe: &Arc<MidiRouteProbe>,
    generated: &[(i64, [u8; 3])],
) -> PluginChain {
    let probe = Arc::clone(probe);
    let generated = generated.to_vec();
    let chain = PluginChain::spawn_identified_with_backend_factory(
        &[instance],
        move || {
            vec![BackendSlot::new(Box::new(MidiRouteBackend {
                role,
                capabilities,
                probe,
                transport: PluginTransport::default(),
                generated,
                pending: Vec::new(),
                output: PluginMidiBatch::default(),
                active: [false; 128],
            }))]
        },
        PluginPrepareConfig {
            sample_rate: 48_000.0,
            max_block_frames: DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES,
        },
    )
    .unwrap();
    wait_until(|| {
        chain
            .control
            .plugin_latency_snapshot()
            .is_some_and(|snapshot| {
                snapshot.slot_is_active(0) && snapshot.total_plugin_latency_samples == 0
            })
    });
    chain
}

fn midi_route_multi_insert(instances: &[u64], probe: &Arc<MidiRouteProbe>) -> PluginChain {
    let owned = instances.to_vec();
    let probe = Arc::clone(probe);
    let count = instances.len();
    let chain = PluginChain::spawn_identified_with_backend_factory(
        instances,
        move || {
            owned
                .iter()
                .map(|_| {
                    BackendSlot::new(Box::new(MidiRouteBackend {
                        role: MidiTestRole::Insert,
                        capabilities: (false, false),
                        probe: Arc::clone(&probe),
                        transport: PluginTransport::default(),
                        generated: Vec::new(),
                        pending: Vec::new(),
                        output: PluginMidiBatch::default(),
                        active: [false; 128],
                    }))
                })
                .collect()
        },
        PluginPrepareConfig {
            sample_rate: 48000.0,
            max_block_frames: 128,
        },
    )
    .unwrap();
    wait_until(|| {
        chain
            .control
            .plugin_latency_snapshot()
            .is_some_and(|snapshot| snapshot.active_mask.count_ones() as usize == count)
    });
    chain
}

fn midi_route_project(monitor_muted: bool, timeline_notes: bool) -> Project {
    let mut project = timeline_test_project(4.0);
    let mut source = timeline_test_channel(SOURCE_CHANNEL, 1);
    source.instrument_plugin_instance_id = Some(SOURCE_INSTANCE);
    let mut sink = timeline_test_channel(SINK_CHANNEL, 2);
    sink.instrument_plugin_instance_id = Some(SINK_INSTANCE);
    project.channels.extend([source, sink]);
    let mut source = timeline_test_plugin(SOURCE_INSTANCE);
    source.midi_ports.output = Some(0);
    source.midi_ports.audio_monitor_muted = monitor_muted;
    let mut sink = timeline_test_plugin(SINK_INSTANCE);
    sink.midi_ports.input = Some(0);
    let mut insert = timeline_test_plugin(INSERT_INSTANCE);
    insert.role = PluginRole::Effect;
    project.plugin_instances.extend([source, sink, insert]);
    project.mixer_insert_slots.push(MixerInsertSlotRef {
        track: 2,
        slot: 0,
        plugin_instance_id: INSERT_INSTANCE,
    });
    if timeline_notes {
        let note = |id, channel_id, pitch, start, length| PianoNote {
            id,
            channel_id: Some(channel_id),
            group_id: None,
            note: pitch,
            start: frame_as_beat(start),
            length: frame_as_beat(length),
            velocity: 1.0,
            selected: false,
            muted: false,
        };
        project.patterns.push(Pattern {
            id: 1,
            name: "MIDI source and intentionally excluded sink note".into(),
            length_steps: 16,
            channel_steps: vec![[false; 16]; 2],
            notes: vec![
                note(1, SOURCE_CHANNEL, 60, 13, 208),
                note(2, SINK_CHANNEL, 45, 59, 400),
            ],
        });
        project.clips.push(timeline_pattern_clip(1, 1.0, 1));
    }
    project
}

struct MidiGraphFixture {
    dsp: Box<DspState>,
    controller: TimelineRuntimeController,
    timeline: Arc<CompiledTimeline>,
    transport: RealtimeTransport,
    status: AudioStatus,
    mailbox: TransportMailbox,
    controls: Vec<PluginChainControl>,
    guards: Vec<PluginWorkerGuard>,
    retired: Consumer<RetiredEndpointResource>,
    source: Arc<MidiRouteProbe>,
    sink: Arc<MidiRouteProbe>,
    insert: Arc<MidiRouteProbe>,
    loop_token: u64,
    timing: PreparedPluginTimingPlan,
    timeline_revision: u64,
    activation_playing: bool,
}

impl MidiGraphFixture {
    fn new(
        project: Project,
        reverse_install: bool,
        source_output: bool,
        sink_input: bool,
        generated: &[(i64, [u8; 3])],
    ) -> Self {
        let timeline = compile_timeline_test_project(&project);
        let (mut controller, realtime) = create_timeline_runtime();
        let bank = Box::new(
            PreparedMixerGraphDelayBank::new(timeline.mixer_graph(), MIDI_TEST_MAX_DELAY).unwrap(),
        );
        controller
            .install_with_mixer_resources(71, Arc::clone(&timeline), bank)
            .unwrap();
        let loop_chase = controller
            .prepare_loop_chase(&timeline, 71, 0, TimelineChaseOptions::default())
            .unwrap();
        let loop_token = controller.install_loop_chase(loop_chase).unwrap();
        let (retired_tx, retired) = RingBuffer::new(16);
        let (insert_events, _insert_rx) = RingBuffer::new(16);
        let (generator_events, _generator_rx) = RingBuffer::new(16);
        let mut dsp = Box::write(
            Box::new_uninit(),
            DspState::try_new_inner(
                48_000.0,
                Some(retired_tx),
                Some(insert_events),
                Some(generator_events),
                None,
                MIDI_TEST_MAX_DELAY,
                Some(realtime),
            )
            .unwrap(),
        );
        dsp.master = 1.0;
        dsp.track_gains.fill(1.0);
        let source = Arc::new(MidiRouteProbe::default());
        let sink = Arc::new(MidiRouteProbe::default());
        let insert = Arc::new(MidiRouteProbe::default());
        let source_chain = midi_route_chain(
            SOURCE_INSTANCE,
            MidiTestRole::Source,
            (true, source_output),
            &source,
            generated,
        );
        let sink_chain = midi_route_chain(
            SINK_INSTANCE,
            MidiTestRole::Sink,
            (sink_input, false),
            &sink,
            &[],
        );
        let insert_instances: Vec<_> = project
            .mixer_insert_slots
            .iter()
            .filter(|slot| slot.track == 2)
            .map(|slot| slot.plugin_instance_id)
            .collect();
        let insert_chain = midi_route_multi_insert(&insert_instances, &insert);
        let mut controls = Vec::new();
        let mut guards = Vec::new();
        let mut generators = vec![
            (SOURCE_CHANNEL, SOURCE_INSTANCE, 1, source_chain),
            (SINK_CHANNEL, SINK_INSTANCE, 2, sink_chain),
        ];
        if reverse_install {
            generators.reverse();
        }
        for (channel, instance, track, chain) in generators {
            let PluginChain {
                audio,
                control,
                guard,
            } = chain;
            dsp.install_generator_endpoint(
                channel,
                instance + 10_000,
                instance,
                track,
                fixed_adapter(audio),
                StereoDelayLine::new(MIDI_TEST_MAX_DELAY).unwrap(),
            );
            controls.push(control);
            guards.push(guard);
        }
        let PluginChain {
            audio,
            control,
            guard,
        } = insert_chain;
        dsp.install_insert_endpoint(2, INSERT_INSTANCE + 10_000, fixed_adapter(audio));
        controls.push(control);
        guards.push(guard);
        let master_instances: Vec<_> = project
            .mixer_insert_slots
            .iter()
            .filter(|slot| project.mixer_runtime_slot(slot.track) == Some(0))
            .map(|slot| slot.plugin_instance_id)
            .collect();
        if !master_instances.is_empty() {
            let PluginChain {
                audio,
                control,
                guard,
            } = midi_route_multi_insert(&master_instances, &insert);
            dsp.install_insert_endpoint(0, 50_000, fixed_adapter(audio));
            controls.push(control);
            guards.push(guard);
        }
        assert_eq!(dsp.apply_pending_timeline_commands(), 2);
        Self {
            dsp,
            controller,
            timeline,
            transport: RealtimeTransport::default(),
            status: AudioStatus::default(),
            mailbox: TransportMailbox::default(),
            controls,
            guards,
            retired,
            source,
            sink,
            insert,
            loop_token,
            timing: PreparedPluginTimingPlan::conservative(48000).unwrap(),
            timeline_revision: 71,
            activation_playing: true,
        }
    }

    fn enqueue_activation(&mut self, epoch: u64, frame: u64) {
        let mut chase = self
            .controller
            .prepare_chase(
                &self.timeline,
                self.timeline_revision,
                epoch,
                frame,
                TimelineChaseOptions::default(),
            )
            .unwrap();
        chase.plugin_timing = self.timing;
        chase.plugin_topology_revision = self.dsp.plugin_topology_revision;
        self.controller.install_chase(chase).unwrap();
        self.controller
            .activate_transport(
                TimelineTransportActivationSpec {
                    revision: self.timeline_revision,
                    target_epoch: epoch,
                    minimum_epoch: epoch,
                    frame,
                    beat_q32: ((frame as f64 / 24_000.0) * BEAT_Q32_ONE as f64) as u64,
                    loop_start_frame: 0,
                    loop_end_frame: 0,
                    loop_start_q32: 0,
                    loop_end_q32: 0,
                    loop_token: self.loop_token,
                    loop_enabled: false,
                    playing: self.activation_playing,
                    mixer_pan_release: TimelineMixerPanRelease::EMPTY,
                },
                self.mailbox.try_load().unwrap().request_id,
            )
            .unwrap();
    }

    fn queue_activation(&mut self, epoch: u64, frame: u64) -> TimelineTransportActivationTicket {
        self.enqueue_activation(epoch, frame);
        crate::realtime_test_alloc::assert_no_alloc_or_drop(|| {
            assert_eq!(self.dsp.apply_pending_timeline_commands(), 2);
        });
        self.dsp.pending_timeline_transport_activation().unwrap()
    }

    fn replace_timeline(&mut self, project: &Project) {
        self.timeline_revision += 1;
        self.timeline = compile_timeline_test_project(project);
        let bank = Box::new(PreparedMixerGraphDelayBank::new(self.timeline.mixer_graph(), MIDI_TEST_MAX_DELAY).unwrap());
        self.controller.install_with_mixer_resources(self.timeline_revision, Arc::clone(&self.timeline), bank).unwrap();
        let chase = self.controller.prepare_loop_chase(&self.timeline, self.timeline_revision, 0, TimelineChaseOptions::default()).unwrap();
        self.loop_token = self.controller.install_loop_chase(chase).unwrap();
        crate::realtime_test_alloc::assert_no_alloc_or_drop(|| { assert_eq!(self.dsp.apply_pending_timeline_commands(), 2); });
    }

    fn select_timing_profile(&mut self, budget: u32) {
        self.timing =
            PreparedPluginTimingPlan::new(self.timing.revision + 1, 48000, budget).unwrap();
        self.dsp.requested_plugin_timing_revision = self.timing.revision;
        self.status
            .plugin_processing
            .requested_revision
            .store(self.timing.revision, Ordering::Release);
        self.status
            .plugin_processing
            .requested_budget
            .store(budget, Ordering::Release);
        self.dsp.raw_callback_frames = 0;
    }

    fn activate(&mut self, epoch: u64, frame: u64) {
        self.queue_activation(epoch, frame);
        // The production callback owns the single preflight/commit transaction.
        // Preflighting here as well would consume the staged chase twice.
        crate::realtime_test_alloc::assert_no_alloc_or_drop(|| {
            self.dsp.refresh_pdc_plan(&self.status, 128);
            self.transport.apply_pending_timeline_activation(&self.status, &mut self.dsp);
        });
        assert_eq!(self.transport.epoch, epoch);
        assert_eq!(self.transport.timeline_frame, frame);
        assert_eq!(self.dsp.timeline_channel_revision, Some(self.timeline_revision));
        assert!(self.dsp.mixer_graph_binding_is_exact());
        self.wait_workers();
    }

    fn wait_workers(&self) {
        // Completion is acknowledged only after the worker has published output.
        // A new epoch may instead account for a submitted block by discarding it.
        for control in &self.controls {
            let submitted = control.stats().submitted;
            wait_until(|| {
                let stats = control.stats();
                stats.completed + stats.dropped_old_epoch_inputs >= submitted || stats.stopped
            });
        }
    }

    fn render(&mut self, frames: usize) -> Vec<[f32; 2]> {
        let mut output = Vec::with_capacity(frames);
        self.dsp.set_device_frame(self.transport.device_frame);
        self.dsp.set_callback_transport_boundary(
            MidiRecordClockAnchor {
                device_frame: self.transport.device_frame,
                timeline_frame: self.transport.timeline_frame,
                transport_epoch: self.transport.epoch,
                loop_count: self.transport.loop_count,
            },
            self.transport.request.playing,
        );
        if !self.dsp.admit_plugin_callback(frames) {
            self.transport.request.playing = false;
            self.transport.request.loop_enabled = false;
            self.dsp.publish_plugin_epoch_status(&self.status);
            output.resize(frames, [0.0; 2]);
            self.wait_workers();
            return output;
        }
        crate::realtime_test_alloc::assert_no_alloc_or_drop(|| {
            render_transport_chunk(
                &mut self.dsp,
                &self.status,
                &self.mailbox,
                &mut self.transport,
                frames,
                |_, block| output.extend_from_slice(block),
            )
        });
        self.wait_workers();
        output
    }

    fn render_frames(&mut self, frames: usize, partition: usize) -> Vec<[f32; 2]> {
        let mut output = Vec::with_capacity(frames);
        while output.len() < frames {
            output.extend(self.render(partition.min(frames - output.len())));
        }
        output
    }

    fn assert_clean(&self) {
        assert_eq!(self.dsp.midi_route_faulted, 0);
        assert_eq!(self.dsp.timeline_execution_failures, 0);
        for control in &self.controls {
            let stats = control.stats();
            assert_eq!(stats.faults, 0);
            assert_eq!(stats.deadline_misses, 0);
            assert_eq!(stats.input_overflows, 0);
            assert_eq!(stats.output_overflows, 0);
            assert_eq!(stats.latency_drift_blocks, 0);
        }
    }

    fn finish(mut self) {
        self.wait_workers();
        for endpoint in &mut self.dsp.generator_endpoints {
            drop(endpoint.take());
        }
        for endpoint in &mut self.dsp.insert_endpoints {
            drop(endpoint.take());
        }
        while let Ok(retired) = self.retired.pop() {
            drop(retired);
        }
        for guard in self.guards.drain(..) {
            guard.shutdown();
        }
    }
}

fn observed_notes(probe: &MidiRouteProbe) -> Vec<(i64, [u8; 3])> {
    probe
        .notes
        .lock()
        .unwrap()
        .iter()
        .map(|note| (note.content_frame(), note.message.data))
        .collect()
}

#[test]
fn midi_port_graph_routes_generated_and_transformed_notes_through_real_workers_and_fx() {
    // Reversed physical slots and callbacks spanning many Q128 blocks must not
    // change the dependency order or the delayed callback-offset conversion.
    for (reverse, partition) in [(false, 128), (true, 31), (true, 255), (true, 2_048)] {
        let mut fixture = MidiGraphFixture::new(
            midi_route_project(false, true),
            reverse,
            true,
            true,
            &[(37, [0x90, 73, 100]), (311, [0x80, 73, 0])],
        );
        fixture.activate(2, 0);
        let source_index = fixture.dsp.find_generator_slot(SOURCE_CHANNEL).unwrap();
        let sink_index = fixture.dsp.find_generator_slot(SINK_CHANNEL).unwrap();
        assert_eq!(source_index > sink_index, reverse);
        let rendered = fixture.render_frames(8_192, partition);
        fixture.assert_clean();
        assert_eq!(
            observed_notes(&fixture.sink),
            vec![
                (13, [0x90, 72, 127]),
                (37, [0x90, 73, 100]),
                (221, [0x80, 72, 0]),
                (311, [0x80, 73, 0]),
            ],
            "generated output, transformed input, and exact offsets; partition {partition}"
        );
        assert_eq!(fixture.sink.active_notes.load(Ordering::Acquire), 0);
        let delay = route_bridge_frames() as usize * 3;
        for (frame, actual) in rendered.iter().enumerate() {
            let mut expected = if frame >= delay { SOURCE_AUDIO } else { 0.0 };
            if (delay + 13..delay + 221).contains(&frame) {
                expected += SINK_NOTE_AUDIO * INSERT_GAIN;
            }
            if (delay + 37..delay + 311).contains(&frame) {
                expected += SINK_NOTE_AUDIO * INSERT_GAIN;
            }
            assert_eq!(
                *actual,
                [expected.tanh(); 2],
                "frame {frame}, partition {partition}"
            );
        }
        assert!(!fixture.insert.transports.lock().unwrap().is_empty());
        fixture.finish();
    }
}

#[test]
fn midi_port_graph_monitor_mute_preserves_events_and_exact_source_sink_transport() {
    let mut fixture = MidiGraphFixture::new(
        midi_route_project(true, false),
        true,
        true,
        true,
        &[(37, [0x90, 73, 100]), (311, [0x80, 73, 0])],
    );
    fixture.activate(2, 0);
    let rendered = fixture.render_frames(8_192, 127);
    fixture.assert_clean();
    assert_eq!(
        observed_notes(&fixture.sink),
        vec![(37, [0x90, 73, 100]), (311, [0x80, 73, 0])]
    );
    let delay = route_bridge_frames() as usize * 3;
    for (frame, actual) in rendered.iter().enumerate() {
        let expected = if (delay + 37..delay + 311).contains(&frame) {
            (SINK_NOTE_AUDIO * INSERT_GAIN).tanh()
        } else {
            0.0
        };
        assert_eq!(*actual, [expected; 2], "muted source, frame {frame}");
    }
    {
        let source = fixture.source.transports.lock().unwrap();
        let sink = fixture.sink.transports.lock().unwrap();
        assert_eq!(source.len(), sink.len());
        assert_eq!(source.len(), 8_192 / 128);
        for (quantum, (source, sink)) in source.iter().zip(sink.iter()).enumerate() {
            assert_eq!(source.sample_position, quantum as i64 * 128);
            assert_eq!(
                sink.sample_position,
                source.sample_position - i64::from(route_bridge_frames())
            );
            for transport in [source, sink] {
                assert!(transport.playing);
                assert_eq!(transport.tempo, 120.0);
                assert_eq!(
                    (transport.time_sig_numerator, transport.time_sig_denominator),
                    (4, 4)
                );
                assert!(
                    (transport.quarter_note_position - transport.sample_position as f64 / 24_000.0)
                        .abs()
                        < 1e-12
                );
            }
        }
    }
    fixture.finish();
}

#[test]
fn midi_port_graph_sink_rejects_direct_live_midi_while_source_output_is_delivered() {
    let mut fixture = MidiGraphFixture::new(
        midi_route_project(true, true),
        true,
        true,
        true,
        &[(37, [0x90, 73, 100])],
    );
    fixture.activate(2, 0);
    let sink = fixture.dsp.find_generator_slot(SINK_CHANNEL).unwrap();
    assert!(
        fixture.dsp.generator_endpoints[sink]
            .as_ref()
            .unwrap()
            .endpoint
            .midi_port_input
    );
    assert!(
        !fixture.dsp.generator_endpoints[sink]
            .as_mut()
            .unwrap()
            .endpoint
            .stage(FrameEvent::midi(11, Some(0), [0x90, 45, 127]),)
    );
    fixture.render_frames(4_096, 64);
    fixture.assert_clean();
    let notes = observed_notes(&fixture.sink);
    assert!(
        notes
            .iter()
            .any(|(frame, data)| *frame == 37 && *data == [0x90, 73, 100])
    );
    assert!(notes.iter().all(|(_, data)| data[1] != 45));
    fixture.finish();
}

#[test]
fn midi_port_graph_output_overflow_process_failure_and_panic_silence_held_notes() {
    for failure in [1, 2, 3] {
        let mut fixture = MidiGraphFixture::new(
            midi_route_project(true, false),
            true,
            true,
            true,
            &[
                (37, [0x90, 73, 100]),
                (2_945, [0x90, 74, 100]),
                (6_017, [0x90, 75, 100]),
            ],
        );
        fixture.activate(2, 0);
        fixture.render_frames(2_560, 128);
        assert_eq!(fixture.sink.active_notes.load(Ordering::Acquire), 1);
        fixture.source.failure.store(failure, Ordering::Release);
        fixture.render_frames(4_096, 128);
        wait_until(|| fixture.sink.active_notes.load(Ordering::Acquire) == 0);
        let sink = fixture.dsp.find_generator_slot(SINK_CHANNEL).unwrap();
        assert_ne!(
            fixture.dsp.midi_route_faulted & (1_u64 << sink),
            0,
            "failure mode {failure}"
        );
        let notes_after_panic = observed_notes(&fixture.sink);
        let later = fixture.render_frames(4_096, 128);
        assert_eq!(
            observed_notes(&fixture.sink),
            notes_after_panic,
            "old queued note resurrection, failure {failure}"
        );
        assert_eq!(fixture.sink.active_notes.load(Ordering::Acquire), 0);
        // Failure is injected at input frame 2560. It can take one producer bridge
        // to reach the router; existing zero-tail FX can retain at most one further
        // bridge. The sink is already cleared; no new notes were admitted above.
        let last_allowed_audio_frame = 2_560 + 2 * route_bridge_frames() as usize;
        let later_start = 2_560 + 4_096;
        let drain = last_allowed_audio_frame.saturating_sub(later_start);
        assert!(
            later[drain..].iter().all(|frame| *frame == [0.0; 2]),
            "failure {failure}, bounded drain {drain}"
        );
        assert!(
            notes_after_panic
                .iter()
                .all(|(_, data)| !matches!(data[1], 74 | 75 | 99))
        );
        fixture.finish();
    }
}

#[test]
fn midi_port_graph_stop_and_epoch_seek_discard_pending_notes_without_resurrection() {
    for stop_first in [false, true] {
        let mut fixture = MidiGraphFixture::new(
            midi_route_project(true, false),
            true,
            true,
            true,
            &[(37, [0x90, 73, 100]), (2_945, [0x90, 74, 100])],
        );
        fixture.activate(2, 0);
        fixture.render_frames(3_072, 128);
        assert_eq!(fixture.sink.active_notes.load(Ordering::Acquire), 1);
        let observed_before = observed_notes(&fixture.sink);
        assert_eq!(observed_before, vec![(37, [0x90, 73, 100])]);
        if stop_first {
            fixture
                .mailbox
                .publish(TransportMutation::SetPlaying(false));
            assert!(
                fixture
                    .render_frames(512, 128)
                    .iter()
                    .all(|frame| *frame == [0.0; 2])
            );
            wait_until(|| fixture.sink.active_notes.load(Ordering::Acquire) == 0);
            assert!(!fixture.transport.request.playing);
            assert_eq!(fixture.transport.timeline_frame, 3_072);
        }
        // Seek beyond all generated notes. Old epoch source outputs still contain
        // pitch 74, but neither it nor the held pitch may reappear after activation.
        fixture.activate(3, 12_288);
        wait_until(|| fixture.sink.active_notes.load(Ordering::Acquire) == 0);
        let rendered = fixture.render_frames(8_192, 255);
        fixture.assert_clean();
        assert_eq!(observed_notes(&fixture.sink), observed_before);
        assert_eq!(fixture.sink.active_notes.load(Ordering::Acquire), 0);
        assert!(rendered.iter().all(|frame| *frame == [0.0; 2]));
        fixture.finish();
    }
}

#[test]
fn midi_port_graph_activation_rejects_missing_capabilities_without_partial_commit() {
    for (source_output, sink_input) in [(false, true), (true, false)] {
        let mut fixture = MidiGraphFixture::new(
            midi_route_project(false, false),
            true,
            source_output,
            sink_input,
            &[],
        );
        let ticket = fixture.queue_activation(2, 0);
        assert_eq!(
            fixture
                .dsp
                .preflight_timeline_transport_activation(ticket, 2),
            Err(TimelineTransportActivationRejectReason::GeneratorRouteBinding)
        );
        assert!(!fixture.dsp.mixer_graph_was_activated);
        assert_eq!(fixture.dsp.transport_epoch, 1);
        assert_eq!(fixture.dsp.timeline_channel_revision, None);
        assert!(
            fixture
                .dsp
                .generator_endpoints
                .iter()
                .flatten()
                .all(|slot| !slot.endpoint.midi_port_input)
        );
        assert!(fixture.source.transports.lock().unwrap().is_empty());
        assert!(fixture.sink.transports.lock().unwrap().is_empty());
        fixture.finish();
    }
}

#[test]
fn midi_port_graph_activation_rejects_source_instance_replacement() {
    let mut fixture =
        MidiGraphFixture::new(midi_route_project(false, false), false, true, true, &[]);
    let ticket = fixture.queue_activation(2, 0);
    assert!(
        fixture
            .dsp
            .preflight_timeline_transport_activation(ticket, 2)
            .is_ok()
    );
    let replacement = midi_route_chain(
        9_999,
        MidiTestRole::Source,
        (true, true),
        &Arc::new(MidiRouteProbe::default()),
        &[],
    );
    let PluginChain {
        audio,
        control,
        guard,
    } = replacement;
    fixture.dsp.install_generator_endpoint(
        SOURCE_CHANNEL,
        19_999,
        9_999,
        1,
        fixed_adapter(audio),
        StereoDelayLine::new(MIDI_TEST_MAX_DELAY).unwrap(),
    );
    fixture.controls.push(control);
    fixture.guards.push(guard);
    assert_eq!(
        fixture
            .dsp
            .preflight_timeline_transport_activation(ticket, 2),
        Err(TimelineTransportActivationRejectReason::GeneratorRouteBinding)
    );
    assert_eq!(fixture.dsp.transport_epoch, 1);
    assert!(!fixture.dsp.mixer_graph_was_activated);
    assert!(fixture.sink.transports.lock().unwrap().is_empty());
    fixture.finish();
}

#[test]
fn ordinary_plugin_graph_uses_the_same_admitted_timing_as_routed_graphs() {
    let mut project = midi_route_project(false, false);
    for plugin in &mut project.plugin_instances {
        plugin.midi_ports = crate::plugin_midi_routing::PluginMidiPorts::default();
    }
    let mut fixture = MidiGraphFixture::new(project, false, true, true, &[]);
    assert!(fixture.timeline.midi_port_routes().is_empty());
    fixture.activate(2, 0);
    let rendered = fixture.render_frames(2 * route_bridge_frames() as usize + 512, 128);
    fixture.assert_clean();
    // Ordinary Generators and the insert share the same operating plan as MIDI routes.
    let graph_delay = 2 * route_bridge_frames() as usize;
    for (frame, actual) in rendered.iter().enumerate() {
        let expected = if frame >= graph_delay {
            SOURCE_AUDIO.tanh()
        } else {
            0.0
        };
        assert_eq!(*actual, [expected; 2], "normal-project frame {frame}");
    }
    for control in &fixture.controls {
        assert_eq!(control.stats().latency_samples, route_bridge_frames() - 128);
    }
    fixture.finish();
}

#[test]
fn midi_port_graph_unused_sink_and_fx_midi_output_does_not_fault_audio() {
    let mut fixture = MidiGraphFixture::new(
        midi_route_project(true, false),
        true,
        true,
        true,
        &[(37, [0x90, 73, 100]), (12_000, [0x80, 73, 0])],
    );
    fixture.sink.failure.store(4, Ordering::Release);
    fixture.insert.failure.store(4, Ordering::Release);
    fixture.activate(2, 0);
    let output = fixture.render_frames(10_240, 512);
    assert!(output.iter().any(|frame| frame[0] != 0.0));
    fixture.assert_clean();
    fixture.finish();
}

#[test]
fn midi_port_graph_bypassed_fx_keeps_audio_and_midi_route_working() {
    let mut fixture = MidiGraphFixture::new(
        midi_route_project(true, false),
        true,
        true,
        true,
        &[(37, [0x90, 73, 100]), (12_000, [0x80, 73, 0])],
    );
    // Prepare the bypassed chain before callback ownership transfer. A direct admin mutation
    // of an already-installed latency identity is intentionally a different (faulting) path.
    let PluginChain { audio, control, guard } = midi_route_multi_insert(&[INSERT_INSTANCE], &fixture.insert);
    assert!(control.set_slot_config(0, crate::plugins::plugin_runtime::SlotConfig { bypassed: true, ..Default::default() }));
    wait_until(|| control.plugin_latency_snapshot().is_some_and(|snapshot| snapshot.active_mask == 0));
    let endpoint = fixed_adapter(audio);
    fixture.controls.push(control);
    fixture.guards.push(guard);
    crate::realtime_test_alloc::assert_no_alloc_or_drop(|| fixture.dsp.install_insert_endpoint(2, 55_004, endpoint));
    fixture.activate(2, 0);
    let output = fixture.render_frames(10_240, 512);
    assert!(output.iter().any(|frame| frame[0] != 0.0));
    fixture.assert_clean();
    fixture.finish();
}

#[test]
fn midi_port_graph_oversized_whole_callback_latches_sink_before_internal_chunks() {
    let mut fixture = MidiGraphFixture::new(
        midi_route_project(true, false),
        true,
        true,
        true,
        &[(37, [0x90, 73, 100])],
    );
    fixture.activate(2, 0);
    fixture.render_frames(3_072, 512);
    assert_eq!(fixture.sink.active_notes.load(Ordering::Acquire), 1);
    assert!(!fixture.dsp.admit_plugin_callback(2_049));
    wait_until(|| fixture.sink.active_notes.load(Ordering::Acquire) == 0);
    assert!(fixture.dsp.plugin_fault.is_some());
    fixture.finish();
}

#[test]
fn midi_port_graph_fanout_delivers_identical_notes_to_independent_sinks() {
    let mut project = midi_route_project(true, false);
    let mut channel = project
        .channels
        .iter()
        .find(|channel| channel.id == SINK_CHANNEL)
        .unwrap()
        .clone();
    channel.id = 33;
    channel.instrument_plugin_instance_id = Some(4_004);
    project.channels.push(channel);
    let mut instance = project
        .plugin_instances
        .iter()
        .find(|plugin| plugin.id == SINK_INSTANCE)
        .unwrap()
        .clone();
    instance.id = 4_004;
    project.plugin_instances.push(instance);
    let mut fixture = MidiGraphFixture::new(
        project,
        true,
        true,
        true,
        &[(37, [0x90, 73, 100]), (1_000, [0x80, 73, 0])],
    );
    let other_sink = Arc::new(MidiRouteProbe::default());
    let PluginChain {
        audio,
        control,
        guard,
    } = midi_route_chain(4_004, MidiTestRole::Sink, (true, false), &other_sink, &[]);
    fixture.dsp.install_generator_endpoint(
        33,
        14_004,
        4_004,
        2,
        fixed_adapter(audio),
        StereoDelayLine::new(MIDI_TEST_MAX_DELAY).unwrap(),
    );
    fixture.controls.push(control);
    fixture.guards.push(guard);
    fixture.activate(2, 0);
    fixture.render_frames(10_240, 512);
    assert_eq!(observed_notes(&fixture.sink), observed_notes(&other_sink));
    assert_eq!(observed_notes(&other_sink).len(), 2);
    assert_eq!(other_sink.active_notes.load(Ordering::Acquire), 0);
    fixture.assert_clean();
    fixture.finish();
}

#[test]
fn midi_port_graph_new_epoch_clears_old_failure_suppression_and_recovers() {
    let mut fixture = MidiGraphFixture::new(
        midi_route_project(true, false),
        true,
        true,
        true,
        &[(37, [0x90, 73, 100]), (8_000, [0x80, 73, 0])],
    );
    fixture.activate(2, 0);
    fixture.render_frames(3_072, 512);
    fixture.dsp.fail_timeline_block();
    assert!(
        fixture
            .dsp
            .generator_endpoints
            .iter()
            .flatten()
            .all(|slot| slot.suppress_output_frames > 0)
    );
    let fault_mask = fixture.dsp.midi_route_faulted;
    fixture.dsp.apply_transport_epoch(&fixture.status, 2, 0);
    assert_eq!(
        fixture.dsp.midi_route_faulted, fault_mask,
        "same epoch cannot clear fault ownership"
    );
    for slot in fixture.dsp.generator_endpoints.iter_mut().flatten() {
        let suppression = slot.suppress_output_frames;
        assert!(suppression > 0, "AlreadyCurrent retains suppression");
        assert!(matches!(
            synchronize_endpoint_epoch(&mut slot.endpoint, 0),
            EndpointEpochSync::Failed
        ));
        assert_eq!(slot.endpoint.epoch(), 2);
        assert_eq!(slot.suppress_output_frames, suppression);
    }
    fixture
        .mailbox
        .publish(TransportMutation::SetPlaying(false));
    fixture.render(128);
    let failures = fixture.dsp.timeline_execution_failures;
    fixture.select_timing_profile(2048);
    fixture.activate(3, 0);
    assert!(
        fixture
            .dsp
            .generator_endpoints
            .iter()
            .flatten()
            .all(|slot| slot.suppress_output_frames == 0)
    );
    assert!(
        fixture
            .dsp
            .insert_endpoints
            .iter()
            .flatten()
            .all(|slot| slot.suppress_output_frames == 0)
    );
    let output = fixture.render_frames(10_240, 512);
    assert_eq!(fixture.dsp.midi_route_faulted, 0);
    assert_eq!(fixture.dsp.timeline_execution_failures, failures);
    assert!(output.iter().any(|frame| frame[0] != 0.0));
    fixture.finish();
}

#[test]
fn common_timing_profiles_align_routed_midi_audio_and_one_physical_fx_bridge() {
    for budget in crate::plugin_timing::PLUGIN_CALLBACK_PROFILES {
        for partition in [1, 31, 64, 127, 128, 129, 255, 256, 512, 2048] {
            if partition > budget as usize {
                continue;
            }
            let mut fixture = MidiGraphFixture::new(
                midi_route_project(true, false),
                true,
                true,
                true,
                &[
                    (0, [0x90, 60, 100]),
                    (1, [0x80, 60, 0]),
                    (127, [0x90, 64, 100]),
                    (128, [0x80, 64, 0]),
                ],
            );
            fixture.select_timing_profile(budget);
            fixture.activate(2, 0);
            let latency = fixture.timing.bridge_latency_frames as usize * 3;
            assert_eq!(
                fixture
                    .dsp
                    .graph_pdc_plan
                    .as_ref()
                    .as_ref()
                    .unwrap()
                    .master_output_latency_samples(),
                latency as u64
            );
            let output = fixture.render_frames(latency + 512, partition);
            fixture.assert_clean();
            assert_eq!(
                observed_notes(&fixture.sink),
                vec![
                    (0, [0x90, 60, 100]),
                    (1, [0x80, 60, 0]),
                    (127, [0x90, 64, 100]),
                    (128, [0x80, 64, 0])
                ]
            );
            for (frame, stereo) in output.iter().enumerate() {
                let expected = if frame == latency || frame == latency + 127 {
                    (SINK_NOTE_AUDIO * INSERT_GAIN).tanh()
                } else {
                    0.0
                };
                assert!(
                    (stereo[0] - expected).abs() < 1e-6,
                    "B={budget} partition={partition} frame={frame}: {stereo:?} vs {expected}"
                );
            }
            fixture.finish();
        }
    }
}

#[test]
fn every_profile_rejects_budget_plus_one_before_submission_and_retry_is_fresh_stopped() {
    for budget in crate::plugin_timing::PLUGIN_CALLBACK_PROFILES {
        let mut fixture =
            MidiGraphFixture::new(midi_route_project(true, false), false, true, true, &[]);
        fixture.select_timing_profile(budget);
        fixture.activate(2, 0);
        let before: Vec<_> = fixture
            .controls
            .iter()
            .map(|control| control.stats().submitted)
            .collect();
        crate::realtime_test_alloc::assert_no_alloc_or_drop(|| {
            assert!(!fixture.dsp.admit_plugin_callback(budget as usize + 1));
            fixture.dsp.publish_plugin_epoch_status(&fixture.status);
        });
        assert_eq!(
            fixture
                .controls
                .iter()
                .map(|control| control.stats().submitted)
                .collect::<Vec<_>>(),
            before
        );
        let fault = fixture.dsp.plugin_fault.unwrap();
        assert_eq!(fault.raw_callback_frames, budget + 1);
        assert_eq!(fault.callback_budget_frames, budget);
        let old_revision = fixture.timing.revision;
        fixture.queue_activation(3, 0);
        fixture
            .transport
            .apply_pending_timeline_activation(&fixture.status, &mut fixture.dsp);
        assert_eq!(
            fixture.transport.epoch, 2,
            "same timing revision must not clear a fault"
        );
        fixture.select_timing_profile(budget);
        fixture.activation_playing = false;
        assert!(fixture.timing.revision > old_revision);
        fixture.queue_activation(4, 0);
        // The explicit retry is a stopped replan; no automatic playing or loop replay.
        crate::realtime_test_alloc::assert_no_alloc_or_drop(|| {
            fixture.dsp.refresh_pdc_plan(&fixture.status, 128);
            fixture.transport.apply_pending_timeline_activation(&fixture.status, &mut fixture.dsp);
        });
        assert_eq!(fixture.transport.epoch, 4);
        assert!(!fixture.transport.request.playing);
        assert!(!fixture.transport.request.loop_enabled);
        assert!(fixture.dsp.plugin_fault.is_none());
        assert_eq!(fixture.dsp.plugin_fault_count, 1);
        fixture.finish();
    }
}

#[test]
fn serial_fx_slots_share_one_bridge_and_master_adds_one_under_changing_callbacks() {
    for budget in crate::plugin_timing::PLUGIN_CALLBACK_PROFILES {
        let mut project = midi_route_project(true, false);
        for (instance, track, slot) in [(3004, 2, 1), (4001, 0, 0)] {
            let mut plugin = timeline_test_plugin(instance);
            plugin.role = PluginRole::Effect;
            project.plugin_instances.push(plugin);
            project.mixer_insert_slots.push(MixerInsertSlotRef {
                track: crate::model::mixer_track_id_for_runtime_slot(track),
                slot,
                plugin_instance_id: instance,
            });
        }
        let mut fixture = MidiGraphFixture::new(
            project,
            true,
            true,
            true,
            &[
                (0, [0x90, 60, 100]),
                (1, [0x80, 60, 0]),
                (127, [0x90, 64, 100]),
                (128, [0x80, 64, 0]),
            ],
        );
        fixture.select_timing_profile(budget);
        fixture.activate(2, 0);
        let expected_latency = 4 * fixture.timing.bridge_latency_frames as usize;
        assert_eq!(
            fixture
                .dsp
                .graph_pdc_plan
                .as_ref()
                .as_ref()
                .unwrap()
                .master_output_latency_samples(),
            expected_latency as u64
        );
        let partitions: Vec<usize> = [1, 31, 64, 127, 128, 129, 255, 256, 512, 2048]
            .into_iter()
            .filter(|frames| *frames <= budget as usize)
            .collect();
        let mut output = Vec::new();
        let total = expected_latency + 512;
        let mut index = 0;
        while output.len() < total {
            output.extend(
                fixture.render(partitions[index % partitions.len()].min(total - output.len())),
            );
            index += 1;
        }
        fixture.assert_clean();
        for (frame, stereo) in output.iter().enumerate() {
            let expected = if frame == expected_latency || frame == expected_latency + 127 {
                (SINK_NOTE_AUDIO * INSERT_GAIN.powi(3)).tanh()
            } else {
                0.0
            };
            assert!(
                (stereo[0] - expected).abs() < 1e-6,
                "B={budget}, frame={frame}: {stereo:?} expected {expected}"
            );
        }
        fixture.finish();
    }
}

#[test]
fn timing_candidate_is_stale_after_profile_request_and_commits_partial_quanta_atomically() {
    let mut fixture =
        MidiGraphFixture::new(midi_route_project(true, false), false, true, true, &[]);
    let stale = fixture.queue_activation(2, 0);
    let old_plan = fixture.dsp.plugin_timing;
    fixture.select_timing_profile(128);
    assert!(
        fixture
            .dsp
            .preflight_timeline_transport_activation(stale, 2)
            .is_err()
    );
    fixture.dsp.reject_timeline_transport_activation(
        stale,
        TimelineTransportActivationRejectReason::GraphPdcPlan,
    );
    assert_eq!(fixture.dsp.plugin_timing, old_plan);
    assert_eq!(fixture.transport.epoch, 1);
    let mut left = [0.0; 31];
    let mut right = [0.0; 31];
    fixture.dsp.generator_endpoints[0]
        .as_mut()
        .unwrap()
        .endpoint
        .process_generator(1, 31, &mut left, &mut right);
    assert_eq!(
        fixture.dsp.generator_endpoints[0]
            .as_ref()
            .unwrap()
            .endpoint
            .input_phase_frames(),
        31
    );
    let next = fixture.queue_activation(3, 0);
    fixture
        .dsp
        .preflight_timeline_transport_activation(next, 3)
        .unwrap();
    assert_eq!(fixture.dsp.plugin_timing, old_plan);
    assert_eq!(
        fixture.dsp.generator_endpoints[0]
            .as_ref()
            .unwrap()
            .endpoint
            .input_phase_frames(),
        31
    );
    // Reset already-staged preflight scratch before using the production single-preflight driver.
    fixture.dsp.timeline_executor.abort_staged_reset();
    fixture.dsp.timeline_automation.abort_staged_reset();
    fixture
        .transport
        .apply_pending_timeline_activation(&fixture.status, &mut fixture.dsp);
    assert_eq!(fixture.transport.epoch, 3);
    assert_eq!(fixture.dsp.plugin_timing, fixture.timing);
    for slot in fixture.dsp.generator_endpoints.iter().flatten() {
        assert_eq!(slot.endpoint.input_phase_frames(), 0);
        assert_eq!(
            slot.endpoint.adapter.latency().total_frames,
            fixture.timing.bridge_latency_frames as usize
        );
    }
    assert_eq!(
        fixture
            .dsp
            .graph_pdc_plan
            .as_ref()
            .as_ref()
            .unwrap()
            .master_output_latency_samples(),
        3 * u64::from(fixture.timing.bridge_latency_frames)
    );
    fixture.finish();
}

#[test]
fn latency_change_requires_explicit_stopped_retry_and_new_exact_attestations() {
    let mut fixture =
        MidiGraphFixture::new(midi_route_project(true, false), false, true, true, &[]);
    fixture.activate(2, 0);
    fixture.render_frames(256, 128);
    let original_pdc = fixture
        .dsp
        .graph_pdc_plan
        .as_ref()
        .as_ref()
        .unwrap()
        .master_output_latency_samples();
    let original_revision = fixture.controls[2]
        .plugin_latency_snapshot()
        .unwrap()
        .revision;
    fixture.insert.latency_frames.store(32, Ordering::Release);
    fixture.controls[2].set_slot_config(0, SlotConfig::default());
    wait_until(|| {
        fixture.controls[2]
            .plugin_latency_snapshot()
            .is_some_and(|snapshot| snapshot.revision != original_revision)
    });
    fixture.dsp.refresh_pdc_plan(&fixture.status, 128);
    assert!(fixture.dsp.plugin_fault.is_some());
    assert_eq!(
        fixture
            .dsp
            .graph_pdc_plan
            .as_ref()
            .as_ref()
            .unwrap()
            .master_output_latency_samples(),
        original_pdc
    );
    let submitted: Vec<_> = fixture
        .controls
        .iter()
        .map(|control| control.stats().submitted)
        .collect();
    assert!(fixture.render(128).iter().all(|frame| *frame == [0.0; 2]));
    assert_eq!(
        fixture
            .controls
            .iter()
            .map(|control| control.stats().submitted)
            .collect::<Vec<_>>(),
        submitted
    );
    fixture.select_timing_profile(2048);
    fixture.activation_playing = false;
    fixture.activate(3, 0);
    assert!(fixture.dsp.plugin_fault.is_none());
    assert!(!fixture.transport.request.playing);
    assert_eq!(fixture.dsp.plugin_fault_count, 1);
    assert_eq!(
        fixture
            .dsp
            .graph_pdc_plan
            .as_ref()
            .as_ref()
            .unwrap()
            .master_output_latency_samples(),
        original_pdc + 32
    );
    let endpoint = &fixture.dsp.insert_endpoints[2].as_ref().unwrap().endpoint;
    assert_eq!(
        endpoint.expected_latency_revision(),
        fixture.controls[2]
            .plugin_latency_snapshot()
            .unwrap()
            .revision
    );
    fixture.finish();
}


#[test]
fn topology_replacement_paused_gap_stale_chase_and_retirement_backpressure_are_allocation_free() {
    let mut fixture = MidiGraphFixture::new(midi_route_project(false, false), false, true, true, &[]);
    fixture.activation_playing = false;
    fixture.activate(2, 0);
    let old_endpoint = fixture.dsp.insert_endpoints[2].as_ref().unwrap().endpoint_id;
    let old_revision = fixture.dsp.plugin_topology_revision;
    let PluginChain { audio, control, guard } = midi_route_multi_insert(&[INSERT_INSTANCE], &fixture.insert);
    fixture.controls.push(control);
    fixture.guards.push(guard);
    let replacement = fixed_adapter(audio);
    let (mut commands, mut callback_commands) = RingBuffer::new(8);
    let (mut retired_assets, _assets) = RingBuffer::new(8);
    let (mut events, _events) = RingBuffer::new(8);
    let (retired, mut reclaim) = RingBuffer::new(1);
    fixture.dsp.retired_insert_endpoints = Some(retired);
    // A full retirement ring must retain ownership in the command ring without mutation.
    let PluginChain { audio, control, guard } = midi_route_multi_insert(&[INSERT_INSTANCE], &fixture.insert);
    fixture.controls.push(control);
    fixture.guards.push(guard);
    fixture.dsp.retire_insert_endpoint(fixed_adapter(audio));
    commands.push(AudioCommand::InstallInsertEndpoint { insert: 2, endpoint_id: 55_001, endpoint: replacement }).unwrap();
    crate::realtime_test_alloc::assert_no_alloc_or_drop(|| {
        process_commands(&mut fixture.dsp, &mut callback_commands, &mut retired_assets, &mut events);
        fixture.dsp.refresh_pdc_plan(&fixture.status, 128);
    });
    assert!(callback_commands.peek().is_ok());
    assert_eq!(fixture.dsp.plugin_topology_revision, old_revision);
    assert_eq!(fixture.dsp.insert_endpoints[2].as_ref().unwrap().endpoint_id, old_endpoint);
    drop(reclaim.pop().unwrap()); // Only the control/test thread destroys retired resources.
    // Prepared before callback acceptance, then installed in production command order.
    fixture.enqueue_activation(3, 0);
    // Also preserve an edit already admitted to the surviving generator's partial quantum.
    // The topology gap must not silently service it using fail_timeline_block's revision 0.
    let source_index = fixture.dsp.find_generator_slot(SOURCE_CHANNEL).unwrap();
    let mut edit_batch = TimelineEndpointBatchPlan::new_boxed();
    let endpoint_id = fixture.dsp.generator_endpoints[source_index].as_ref().unwrap().endpoint_id;
    let edit = ParameterEditSubmission {
        edit_id: crate::plugin_parameter_edit::ParameterEditId(701),
        route: ParameterEditRoute {
            project_session: 0,
            endpoint: crate::plugin_parameter_edit::ParameterEndpoint { kind: ParameterEndpointKind::Generator, id: endpoint_id },
            instance_id: SOURCE_INSTANCE,
            slot: 0,
            parameter_id: 9,
        },
        normalized: 0.7,
    };
    crate::realtime_test_alloc::assert_no_alloc_or_drop(|| {
        assert_eq!(DspState::try_admit_plugin_parameter_edit(
            &mut fixture.dsp.generator_endpoints[source_index].as_mut().unwrap().endpoint,
            TimelineEndpointKey::new(SOURCE_CHANNEL, endpoint_id, SOURCE_INSTANCE),
            false, edit, edit_batch.as_mut()), Ok(true));
    });
    fixture.dsp.admitted_live_edit_endpoint_count = 1;
    let before: Vec<_> = fixture.controls.iter().map(|c| c.stats().submitted).collect();
    // The production boundary accepts replacement, refreshes old PDC, rejects the stale chase,
    // rejects paused parameter/MIDI commands, and renders silence without any worker submission.
    commands.push(AudioCommand::SetGeneratorParameter { channel_id: SOURCE_CHANNEL, slot: 0, id: 1, normalized: 0.4 }).unwrap();
    commands.push(AudioCommand::SendGeneratorMidi { channel_id: SOURCE_CHANNEL, slot: None, data: [0x90, 60, 100], sample_offset: 0 }).unwrap();
    let mut rendered = [[1.0; 2]; 128];
    crate::realtime_test_alloc::assert_no_alloc_or_drop(|| {
        process_commands(&mut fixture.dsp, &mut callback_commands, &mut retired_assets, &mut events);
        assert_eq!(fixture.dsp.apply_pending_timeline_commands(), 2);
        fixture.dsp.refresh_pdc_plan(&fixture.status, 128);
        fixture.transport.apply_pending_timeline_activation(&fixture.status, &mut fixture.dsp);
        assert!(fixture.dsp.admit_plugin_callback(128));
        render_transport_chunk(&mut fixture.dsp, &fixture.status, &fixture.mailbox, &mut fixture.transport, 128,
            |offset, block| rendered[offset..offset + block.len()].copy_from_slice(block));
    });
    assert!(rendered.iter().all(|frame| *frame == [0.0; 2]));
    assert_eq!(fixture.transport.epoch, 2);
    assert!(fixture.dsp.plugin_topology_replan_pending);
    assert!(fixture.dsp.plugin_fault.is_none());
    assert_eq!(fixture.controls.iter().map(|c| c.stats().submitted).collect::<Vec<_>>(), before);
    assert!(fixture.dsp.generator_endpoints.iter().flatten().all(|e| e.endpoint.adapter.processing_fault().is_none()));
    assert_eq!(fixture.dsp.plugin_processing_snapshot().health, PluginProcessingHealth::Priming);
    // No explicit retry or timing revision change: a newly prepared exact candidate suffices.
    let timing = fixture.timing;
    fixture.activate(4, 0);
    assert_eq!(fixture.dsp.plugin_timing, timing);
    assert!(!fixture.dsp.plugin_topology_replan_pending);
    assert!(fixture.dsp.plugin_fault.is_none());
    assert_eq!(fixture.dsp.plugin_fault_count, 0);
    drop(reclaim.pop().unwrap());
    fixture.finish();
}

#[test]
fn authorized_topology_remove_add_and_generator_replace_require_exact_manifests() {
    let mut project = midi_route_project(false, false);
    let mut fixture = MidiGraphFixture::new(project.clone(), false, true, true, &[]);
    fixture.activate(2, 0);
    // Removal suspends first; the old manifest is rejected despite its fresh topology stamp.
    crate::realtime_test_alloc::assert_no_alloc_or_drop(|| fixture.dsp.remove_insert_endpoint(2));
    fixture.queue_activation(3, 0);
    crate::realtime_test_alloc::assert_no_alloc_or_drop(|| {
        fixture.dsp.refresh_pdc_plan(&fixture.status, 128);
        fixture.transport.apply_pending_timeline_activation(&fixture.status, &mut fixture.dsp);
    });
    assert_eq!(fixture.transport.epoch, 2);
    assert!(fixture.dsp.plugin_fault.is_none());
    project.mixer_insert_slots.clear();
    fixture.replace_timeline(&project);
    fixture.activate(4, 0);
    assert!(!fixture.dsp.plugin_topology_replan_pending);
    project.mixer_insert_slots.push(MixerInsertSlotRef { track: 2, slot: 0, plugin_instance_id: INSERT_INSTANCE });
    let PluginChain { audio, control, guard } = midi_route_multi_insert(&[INSERT_INSTANCE], &fixture.insert);
    let endpoint = fixed_adapter(audio);
    fixture.controls.push(control); fixture.guards.push(guard);
    crate::realtime_test_alloc::assert_no_alloc_or_drop(|| fixture.dsp.install_insert_endpoint(2, 55_002, endpoint));
    fixture.replace_timeline(&project);
    fixture.activate(5, 0);
    let PluginChain { audio, control, guard } = midi_route_chain(SOURCE_INSTANCE, MidiTestRole::Source, (true, true), &fixture.source, &[]);
    let endpoint = fixed_adapter(audio);
    let pdc = StereoDelayLine::new(MIDI_TEST_MAX_DELAY).unwrap();
    fixture.controls.push(control); fixture.guards.push(guard);
    crate::realtime_test_alloc::assert_no_alloc_or_drop(|| fixture.dsp.install_generator_endpoint(SOURCE_CHANNEL, 55_003, SOURCE_INSTANCE, 1, endpoint, pdc));
    fixture.activate(6, 0);
    assert_eq!(fixture.dsp.plugin_fault_count, 0);
    // An identity mutation without an accepted lifecycle command is still a real fault.
    fixture.dsp.insert_endpoints[2].as_mut().unwrap().endpoint_id = 999_999;
    crate::realtime_test_alloc::assert_no_alloc_or_drop(|| fixture.dsp.refresh_pdc_plan(&fixture.status, 128));
    assert_eq!(fixture.dsp.plugin_fault.unwrap().reason, PluginProcessingFaultReason::EndpointChanged);
    fixture.finish();
}

#[test]
fn recovered_health_belongs_to_current_epoch_only() {
    let mut fixture = MidiGraphFixture::new(midi_route_project(false, false), false, true, true, &[]);
    fixture.select_timing_profile(128);
    fixture.activate(2, 0);
    assert!(!fixture.dsp.admit_plugin_callback(129));
    fixture.select_timing_profile(128);
    fixture.activation_playing = false;
    fixture.activate(3, 0);
    assert_eq!(fixture.dsp.plugin_processing_snapshot().health, PluginProcessingHealth::Priming);
    fixture.activation_playing = true;
    // Explicit Play gets another fresh epoch. A pending recovery follows that epoch until
    // actual output is primed, but must not permanently label subsequent normal epochs.
    fixture.activate(4, 0);
    fixture.render_frames(4096, 128);
    assert_eq!(fixture.dsp.plugin_processing_snapshot().health, PluginProcessingHealth::Recovered);
    assert_eq!(fixture.dsp.plugin_recovered_count, 1);
    fixture.activate(5, 0);
    fixture.render_frames(4096, 128);
    assert_eq!(fixture.dsp.plugin_processing_snapshot().health, PluginProcessingHealth::Running);
    assert_eq!(fixture.dsp.plugin_recovered_count, 1);
    fixture.finish();
}


#[test]
fn production_endpoint_admission_waits_for_worker_prepare_and_failed_candidate_retains_old_chain() {
    for fail in [true, false] {
        let mut fixture = MidiGraphFixture::new(midi_route_project(false, false), false, true, true, &[]);
        fixture.activation_playing = false;
        fixture.activate(2, 0);
        let old_id = fixture.dsp.insert_endpoints[2].as_ref().unwrap().endpoint_id;
        let old_topology = fixture.dsp.plugin_topology_revision;
        let probe = Arc::new(MidiRouteProbe::default());
        probe.failure.store(5, Ordering::Release);
        let backend_probe = Arc::clone(&probe);
        let PluginChain { audio, control, guard } = PluginChain::spawn_identified_with_backend_factory(
            &[INSERT_INSTANCE], move || vec![BackendSlot::new(Box::new(MidiRouteBackend {
                role: MidiTestRole::Insert, capabilities: (false, false), probe: backend_probe,
                transport: PluginTransport::default(), generated: Vec::new(), pending: Vec::new(),
                output: PluginMidiBatch::default(), active: [false; 128],
            }))], PluginPrepareConfig { sample_rate: 48000.0, max_block_frames: 128 }).unwrap();
        let mut endpoint = fixed_adapter(audio);
        endpoint.project_session = 7; // The actual production admission contract.
        fixture.controls.push(control); fixture.guards.push(guard);
        let candidate_control = fixture.controls.len() - 1;
        let (mut commands, mut callback_commands) = RingBuffer::new(4);
        let (mut retired_assets, _assets) = RingBuffer::new(4);
        let (mut asset_events, _events) = RingBuffer::new(4);
        let (events, mut confirmations) = RingBuffer::new(4);
        fixture.dsp.insert_endpoint_events = Some(events);
        commands.push(AudioCommand::InstallInsertEndpoint { insert: 2, endpoint_id: 55_010, endpoint }).unwrap();
        crate::realtime_test_alloc::assert_no_alloc_or_drop(|| {
            process_commands(&mut fixture.dsp, &mut callback_commands, &mut retired_assets, &mut asset_events);
            fixture.dsp.refresh_pdc_plan(&fixture.status, 128);
        });
        assert!(callback_commands.peek().is_ok());
        assert_eq!(fixture.dsp.insert_endpoints[2].as_ref().unwrap().endpoint_id, old_id);
        assert_eq!(fixture.dsp.plugin_topology_revision, old_topology);
        assert!(confirmations.pop().is_err());
        probe.failure.store(if fail { 4 } else { 0 }, Ordering::Release);
        wait_until(|| fixture.controls[candidate_control].plugin_latency_snapshot().is_some());
        crate::realtime_test_alloc::assert_no_alloc_or_drop(|| {
            process_commands(&mut fixture.dsp, &mut callback_commands, &mut retired_assets, &mut asset_events);
            fixture.dsp.refresh_pdc_plan(&fixture.status, 128);
        });
        assert!(callback_commands.peek().is_err());
        assert!(matches!(confirmations.pop().unwrap(), InsertEndpointEvent::Installed { success, .. } if success != fail));
        if fail {
            assert_eq!(fixture.dsp.insert_endpoints[2].as_ref().unwrap().endpoint_id, old_id);
            assert_eq!(fixture.dsp.plugin_topology_revision, old_topology);
            assert!(!fixture.dsp.plugin_topology_replan_pending);
        } else {
            assert_eq!(fixture.dsp.insert_endpoints[2].as_ref().unwrap().endpoint_id, 55_010);
            assert!(fixture.dsp.plugin_topology_replan_pending);
            fixture.activate(3, 0);
        }
        assert!(fixture.dsp.plugin_fault.is_none());
        fixture.finish();
    }
}

#[test]
fn authorized_timeline_clear_and_non_plugin_resync_do_not_latch_endpoint_fault() {
    for with_plugins in [true, false] {
        let project = if with_plugins { midi_route_project(false, false) } else { timeline_test_project(4.0) };
        let mut fixture = MidiGraphFixture::new(midi_route_project(false, false), false, true, true, &[]);
        if !with_plugins {
            fixture.dsp.clear_insert_endpoints(1);
            fixture.dsp.clear_generator_endpoints(1);
            fixture.replace_timeline(&project);
        }
        fixture.activate(2, 0);
        fixture.render(63);
        fixture.controller.clear(fixture.timeline_revision).unwrap();
        let before: Vec<_> = fixture.controls.iter().map(|control| control.stats().submitted).collect();
        let mut output = [[1.0; 2]; 128];
        crate::realtime_test_alloc::assert_no_alloc_or_drop(|| {
            assert_eq!(fixture.dsp.apply_pending_timeline_commands(), 1);
            fixture.dsp.refresh_pdc_plan(&fixture.status, 128);
            fixture.transport.apply_pending_timeline_activation(&fixture.status, &mut fixture.dsp);
            assert!(fixture.dsp.admit_plugin_callback(128));
            render_transport_chunk(&mut fixture.dsp, &fixture.status, &fixture.mailbox, &mut fixture.transport, 128,
                |offset, block| output[offset..offset + block.len()].copy_from_slice(block));
        });
        assert!(output.iter().all(|frame| *frame == [0.0; 2]));
        assert!(fixture.dsp.plugin_fault.is_none());
        assert_eq!(fixture.controls.iter().map(|control| control.stats().submitted).collect::<Vec<_>>(), before);
        fixture.replace_timeline(&project);
        fixture.activate(3, 0);
        assert_eq!(fixture.dsp.plugin_fault_count, 0);
        // A non-plugin Timeline failure has the same known suspended binding, while actual
        // MIDI routing loss uses its separate non-droppable cause latch.
        crate::realtime_test_alloc::assert_no_alloc_or_drop(|| {
            fixture.dsp.timeline_runtime.as_mut().unwrap().require_resync();
            fixture.dsp.fail_timeline_block();
            fixture.dsp.refresh_pdc_plan(&fixture.status, 128);
            assert!(fixture.dsp.admit_plugin_callback(128));
        });
        assert!(fixture.dsp.plugin_fault.is_none());
        fixture.activate(4, 0);
        assert_eq!(fixture.dsp.plugin_fault_count, 0);
        fixture.finish();
    }
}

#[test]
fn empty_endpoint_destination_still_reserves_retirement_for_late_worker_rejection() {
    let mut fixture = MidiGraphFixture::new(midi_route_project(false, false), false, true, true, &[]);
    fixture.activation_playing = false;
    fixture.activate(2, 0);
    let (retired, mut reclaim) = RingBuffer::new(1);
    fixture.dsp.retired_insert_endpoints = Some(retired);
    let PluginChain { audio, control, guard } = midi_route_multi_insert(&[INSERT_INSTANCE], &fixture.insert);
    fixture.dsp.retire_insert_endpoint(fixed_adapter(audio));
    fixture.controls.push(control); fixture.guards.push(guard);
    let PluginChain { audio, control, guard } = midi_route_multi_insert(&[55_020], &fixture.insert);
    let mut endpoint = fixed_adapter(audio);
    endpoint.project_session = 7;
    let (mut commands, mut callback_commands) = RingBuffer::new(2);
    let (mut retired_assets, _assets) = RingBuffer::new(2);
    let (mut asset_events, _events) = RingBuffer::new(2);
    commands.push(AudioCommand::InstallInsertEndpoint { insert: 3, endpoint_id: 55_020, endpoint }).unwrap();
    crate::realtime_test_alloc::assert_no_alloc_or_drop(|| {
        process_commands(&mut fixture.dsp, &mut callback_commands, &mut retired_assets, &mut asset_events);
    });
    assert!(callback_commands.peek().is_ok());
    assert!(fixture.dsp.insert_endpoints[3].is_none());
    // The candidate can stop after a readiness observation; it must still have a guaranteed
    // off-thread retirement destination when the command is eventually popped and rejected.
    assert_eq!(guard.shutdown_blocking(std::time::Duration::from_secs(2)),
        crate::plugins::plugin_runtime::ShutdownOutcome::Joined);
    assert!(control.stats().stopped);
    drop(reclaim.pop().unwrap());
    crate::realtime_test_alloc::assert_no_alloc_or_drop(|| {
        process_commands(&mut fixture.dsp, &mut callback_commands, &mut retired_assets, &mut asset_events);
    });
    assert!(callback_commands.peek().is_err());
    assert!(fixture.dsp.insert_endpoints[3].is_none());
    drop(reclaim.pop().expect("rejected candidate was retired, not leaked or callback-dropped"));
    fixture.finish();
}

#[test]
fn late_plugin_latency_observation_cannot_become_an_unfaulted_suspended_graph() {
    // The worker publishes after the production pre-admission refresh. Cover the later
    // automation-batch read, normal render's third refresh, and paused monitor refresh.
    for budget in [128, 256, 512, 2048] {
        for routed in [false, true] {
            for phase in 0..4 {
                let mut project = midi_route_project(true, true);
                if !routed {
                    for plugin in &mut project.plugin_instances {
                        plugin.midi_ports = crate::plugin_midi_routing::PluginMidiPorts::default();
                    }
                }
                if phase == 0 || phase == 3 {
                    push_timeline_automation(&mut project, 1,
                        AutomationTarget::PluginParameter { instance: INSERT_INSTANCE, parameter: 9 },
                        AutomationCurve::Linear,
                        [AutomationPoint::new(0.0, 0.25), AutomationPoint::new(1.0, 0.75)]);
                }
                let mut fixture = MidiGraphFixture::new(project, false, true, true, &[]);
                fixture.select_timing_profile(budget);
                fixture.activation_playing = phase != 2;
                fixture.activate(2, 0);
                if phase == 0 || phase == 3 {
                    assert_eq!(fixture.dsp.timeline_plugin_automation_bindings.iter().count(), 1);
                }
                fixture.dsp.raw_callback_frames = budget as usize;
                crate::realtime_test_alloc::assert_no_alloc_or_drop(|| {
                    fixture.dsp.refresh_pdc_plan(&fixture.status, budget as usize);
                    assert!(fixture.dsp.admit_plugin_callback(budget as usize));
                });
                let original = fixture.controls[2].plugin_latency_snapshot().unwrap();
                let old_revision = original.revision;
                fixture.insert.latency_frames.store(32, Ordering::Release);
                assert!(fixture.controls[2].set_slot_config(0, SlotConfig::default()));
                wait_until(|| fixture.controls[2].plugin_latency_snapshot()
                    .is_some_and(|snapshot| snapshot.revision != old_revision));
                if phase == 3 {
                    // The first batch identity read still matches; drift becomes visible only
                    // in precommit's second read, after matrix/event preparation has finished.
                    let newer = fixture.controls[2].plugin_latency_snapshot().unwrap();
                    fixture.dsp.insert_endpoints[2].as_mut().unwrap().endpoint
                        .script_fresh_endpoint_snapshots([Some(original), Some(newer)]);
                }
                let submissions: Vec<_> = fixture.controls.iter().map(|c| c.stats().submitted).collect();
                let mut output = vec![[1.0; 2]; budget as usize];
                crate::realtime_test_alloc::assert_no_alloc_or_drop(|| {
                    if phase == 2 {
                        let monitor = PausedMidiMonitorRoute {
                            generator_index: fixture.dsp.find_generator_slot(SINK_CHANNEL).unwrap(),
                            mixer_track: 2,
                        };
                        fixture.dsp.render_paused_midi_monitor_graph(&fixture.status, budget as usize, monitor);
                        output.copy_from_slice(&fixture.dsp.master_block[..budget as usize]);
                    } else {
                        render_transport_chunk(&mut fixture.dsp, &fixture.status, &fixture.mailbox,
                            &mut fixture.transport, budget as usize,
                            |offset, block| output[offset..offset + block.len()].copy_from_slice(block));
                    }
                });
                assert!(output.iter().all(|frame| *frame == [0.0; 2]));
                let fault = fixture.dsp.plugin_fault.expect("late observed drift must not stay Priming");
                assert_eq!(fault.reason, PluginProcessingFaultReason::LatencyDrift);
                assert_eq!(fault.endpoint_id, INSERT_INSTANCE + 10_000);
                assert_eq!(fault.epoch, 2);
                assert_eq!(fault.timing_revision, fixture.timing.revision);
                assert_eq!(fault.raw_callback_frames, budget);
                assert_eq!(fault.expected_sequence, 0);
                assert_eq!(fixture.controls.iter().map(|c| c.stats().submitted).collect::<Vec<_>>(), submissions);
                // Further raw callbacks cannot service paused MIDI/parameters or silently
                // recover. A same-plan chase is rejected; explicit stopped Retry is required.
                assert!(fixture.render(budget as usize).iter().all(|frame| *frame == [0.0; 2]));
                fixture.queue_activation(3, 0);
                crate::realtime_test_alloc::assert_no_alloc_or_drop(|| {
                    fixture.transport.apply_pending_timeline_activation(&fixture.status, &mut fixture.dsp);
                });
                assert_eq!(fixture.transport.epoch, 2);
                assert_eq!(fixture.dsp.plugin_fault, Some(fault));
                fixture.select_timing_profile(budget);
                fixture.activation_playing = false;
                fixture.activate(4, 0);
                assert!(fixture.dsp.plugin_fault.is_none());
                assert!(!fixture.transport.request.playing);
                assert_eq!(fixture.dsp.plugin_fault_count, 1);
                fixture.finish();
            }
        }
    }
}

#[test]
fn generic_timeline_failure_retains_observed_drift_without_an_extra_shared_read() {
    for observed_drift in [false, true] {
        let mut fixture = MidiGraphFixture::new(midi_route_project(false, false), false, true, true, &[]);
        fixture.activate(2, 0);
        let endpoint = &mut fixture.dsp.insert_endpoints[2].as_mut().unwrap().endpoint;
        let original = endpoint.coherent_latency_snapshot().unwrap();
        let newer = PluginLatencySnapshot { revision: next_nonzero_id(original.revision), ..original };
        endpoint.script_fresh_endpoint_snapshots([if observed_drift { Some(newer) } else { None }, None]);
        crate::realtime_test_alloc::assert_no_alloc_or_drop(|| {
            // Represents the identity-bearing batch read that failed before cleanup.
            let _ = fixture.dsp.insert_endpoints[2].as_mut().unwrap().endpoint.exact_endpoint_snapshot();
            fixture.dsp.fail_timeline_block();
        });
        assert_eq!(fixture.dsp.insert_endpoints[2].as_ref().unwrap().endpoint.fresh_snapshot_script_cursor, 1,
            "generic failure classification must not re-read shared metadata");
        if observed_drift {
            let fault = fixture.dsp.plugin_fault.unwrap();
            assert_eq!(fault.endpoint_id, INSERT_INSTANCE + 10_000);
            assert_eq!(fault.reason, PluginProcessingFaultReason::LatencyDrift);
        } else {
            assert!(fixture.dsp.plugin_fault.is_none(), "packet failure plus an unavailable read is not positive drift evidence");
        }
        fixture.finish();
    }
}

#[test]
fn late_paused_monitor_fault_fences_same_callback_safety_and_parameter_services() {
    for frames in [31, 128, 2048] {
        let mut project = midi_route_project(false, false);
        for plugin in &mut project.plugin_instances {
            plugin.midi_ports = crate::plugin_midi_routing::PluginMidiPorts::default();
        }
        let mut fixture = MidiGraphFixture::new(project, false, true, true, &[]);
        for endpoint in fixture.dsp.generator_endpoints.iter_mut().flatten() {
            endpoint.endpoint.project_session = 7;
        }
        fixture.activation_playing = false;
        fixture.activate(2, 0);
        let source_index = fixture.dsp.find_generator_slot(SOURCE_CHANNEL).unwrap();
        let source_id = fixture.dsp.generator_endpoints[source_index].as_ref().unwrap().endpoint_id;
        let sink_id = fixture.dsp.generator_endpoints[fixture.dsp.find_generator_slot(SINK_CHANNEL).unwrap()]
            .as_ref().unwrap().endpoint_id;
        let (retired_midi_tx, _retired_midi_rx) = RingBuffer::new(4);
        let (midi_event_tx, _midi_event_rx) = RingBuffer::new(4);
        fixture.dsp.retired_midi_inputs = Some(retired_midi_tx);
        fixture.dsp.midi_input_route_events = Some(midi_event_tx);
        let (_sender, receiver) = crate::midi_device::test_input_mailbox(8, 33);
        fixture.dsp.install_midi_input(5, PreparedMidiInputRoute::new(receiver, MidiGeneratorRouteStamp {
            project_session: 7, channel_id: SINK_CHANNEL, endpoint_id: sink_id,
            plugin_instance_id: SINK_INSTANCE, slot: None,
        }).unwrap());
        let mut batch = TimelineEndpointBatchPlan::new_boxed();
        let edit = ParameterEditSubmission {
            edit_id: crate::plugin_parameter_edit::ParameterEditId(702),
            route: ParameterEditRoute {
                project_session: 7,
                endpoint: crate::plugin_parameter_edit::ParameterEndpoint {
                    kind: ParameterEndpointKind::Generator, id: source_id,
                },
                instance_id: SOURCE_INSTANCE, slot: 0, parameter_id: 9,
            }, normalized: 0.7,
        };
        assert_eq!(DspState::try_admit_plugin_parameter_edit(
            &mut fixture.dsp.generator_endpoints[source_index].as_mut().unwrap().endpoint,
            TimelineEndpointKey::new(SOURCE_CHANNEL, source_id, SOURCE_INSTANCE),
            false, edit, batch.as_mut()), Ok(true));
        fixture.dsp.admitted_live_edit_endpoint_count = 1;
        fixture.dsp.paused_midi_safety[source_index] = Some(PausedMidiSafetyService {
            stamp: MidiGeneratorRouteStamp { project_session: 7, channel_id: SOURCE_CHANNEL,
                endpoint_id: source_id, plugin_instance_id: SOURCE_INSTANCE, slot: None },
            remaining_frames: 128,
        });
        fixture.dsp.raw_callback_frames = frames;
        fixture.dsp.refresh_pdc_plan(&fixture.status, frames);
        assert!(fixture.dsp.admit_plugin_callback(frames));
        let old = fixture.controls[2].plugin_latency_snapshot().unwrap().revision;
        fixture.insert.latency_frames.store(32, Ordering::Release);
        assert!(fixture.controls[2].set_slot_config(0, SlotConfig::default()));
        wait_until(|| fixture.controls[2].plugin_latency_snapshot().is_some_and(|s| s.revision != old));
        let callbacks = fixture.dsp.generator_endpoints[source_index].as_ref().unwrap().endpoint.stats().callbacks;
        let mut output = vec![[1.0; 2]; frames];
        crate::realtime_test_alloc::assert_no_alloc_or_drop(|| {
            render_transport_chunk(&mut fixture.dsp, &fixture.status, &fixture.mailbox,
                &mut fixture.transport, frames,
                |offset, block| output[offset..offset + block.len()].copy_from_slice(block));
        });
        assert_eq!(fixture.dsp.plugin_fault.unwrap().reason, PluginProcessingFaultReason::LatencyDrift);
        assert!(output.iter().all(|frame| *frame == [0.0; 2]));
        assert_eq!(fixture.dsp.generator_endpoints[source_index].as_ref().unwrap().endpoint.stats().callbacks, callbacks);
        assert_eq!(fixture.dsp.paused_midi_safety[source_index].unwrap().remaining_frames, 128);
        assert!(fixture.controls.iter().all(|c| c.stats().submitted == 0));
        fixture.finish();
    }
}
