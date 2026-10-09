// Included as a child of audio::tests so these integration cases share the existing
// project/Timeline fixtures. Workers are real PluginChain workers. The barriers
// are test-driver waits between device callbacks, never waits in the audio path;
// these tests establish deterministic semantics, not real-time scheduling claims.
use super::*;
use crate::plugin_midi_routing::MIDI_ROUTE_BRIDGE_FRAMES;
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
        0
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
        let insert_chain = midi_route_chain(
            INSERT_INSTANCE,
            MidiTestRole::Insert,
            (false, false),
            &insert,
            &[],
        );
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
        }
    }

    fn queue_activation(&mut self, epoch: u64, frame: u64) -> TimelineTransportActivationTicket {
        let chase = self
            .controller
            .prepare_chase(
                &self.timeline,
                71,
                epoch,
                frame,
                TimelineChaseOptions::default(),
            )
            .unwrap();
        self.controller.install_chase(chase).unwrap();
        self.controller
            .activate_transport(
                TimelineTransportActivationSpec {
                    revision: 71,
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
                    playing: true,
                    mixer_pan_release: TimelineMixerPanRelease::EMPTY,
                },
                self.mailbox.try_load().unwrap().request_id,
            )
            .unwrap();
        assert_eq!(self.dsp.apply_pending_timeline_commands(), 2);
        self.dsp.pending_timeline_transport_activation().unwrap()
    }

    fn activate(&mut self, epoch: u64, frame: u64) {
        self.queue_activation(epoch, frame);
        // The production callback owns the single preflight/commit transaction.
        // Preflighting here as well would consume the staged chase twice.
        self.transport
            .apply_pending_timeline_activation(&self.status, &mut self.dsp);
        assert_eq!(self.transport.epoch, epoch);
        assert_eq!(self.transport.timeline_frame, frame);
        assert_eq!(self.dsp.timeline_channel_revision, Some(71));
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
        render_transport_chunk(
            &mut self.dsp,
            &self.status,
            &self.mailbox,
            &mut self.transport,
            frames,
            |_, block| output.extend_from_slice(block),
        );
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
        let delay = MIDI_ROUTE_BRIDGE_FRAMES as usize * 3;
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
    let delay = MIDI_ROUTE_BRIDGE_FRAMES as usize * 3;
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
                source.sample_position - i64::from(MIDI_ROUTE_BRIDGE_FRAMES)
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
        let last_allowed_audio_frame = 2_560 + 2 * MIDI_ROUTE_BRIDGE_FRAMES as usize;
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
fn midi_port_graph_off_keeps_existing_two_quantum_worker_bridges() {
    let mut project = midi_route_project(false, false);
    for plugin in &mut project.plugin_instances {
        plugin.midi_ports = crate::plugin_midi_routing::PluginMidiPorts::default();
    }
    let mut fixture = MidiGraphFixture::new(project, false, true, true, &[]);
    assert!(fixture.timeline.midi_port_routes().is_empty());
    fixture.activate(2, 0);
    let rendered = fixture.render_frames(1_024, 128);
    fixture.assert_clean();
    // Both Generator and track insert retain the existing Q128 accumulation
    // plus one asynchronous Q128 turn, rather than acquiring MIDI-route latency.
    let graph_delay = 2 * (2 * DEFAULT_PLUGIN_FIXED_QUANTUM_FRAMES);
    for (frame, actual) in rendered.iter().enumerate() {
        let expected = if frame >= graph_delay {
            SOURCE_AUDIO.tanh()
        } else {
            0.0
        };
        assert_eq!(*actual, [expected; 2], "normal-project frame {frame}");
    }
    for control in &fixture.controls {
        assert_eq!(control.stats().latency_samples, 128);
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
    let insert_control = &fixture.controls[2];
    assert!(insert_control.set_slot_config(
        0,
        crate::plugins::plugin_runtime::SlotConfig {
            bypassed: true,
            ..Default::default()
        }
    ));
    wait_until(|| {
        insert_control
            .plugin_latency_snapshot()
            .is_some_and(|snapshot| snapshot.active_mask == 0)
    });
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
    fixture.dsp.reject_oversized_midi_callback(2_049);
    wait_until(|| fixture.sink.active_notes.load(Ordering::Acquire) == 0);
    assert_ne!(fixture.dsp.midi_route_faulted, 0);
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
