// Register inside audio.rs's existing `#[cfg(test)] mod tests` with:
// mod master_automation_tests {
//     use super::*;
//     include!("audio_master_automation_tests.rs");
// }
//
// These tests exercise the production command consumer, Timeline preparation,
// transport renderer, source mixer and final master output. All fixtures are
// The first five fixtures are 48 kHz with no plug-ins and zero PDC. The later
// fixtures use real mock PluginChain workers, physical generator/insert latency
// and the admitted bridge profile. Their waits are test-driver barriers between
// callbacks; these establish PCM semantics, not a real-time scheduling claim.

const MASTER_FADE_FRAMES: usize = 512;
const MASTER_FADE_START: usize = 64;
const MASTER_FADE_END: usize = 448;
const MASTER_TEST_SOURCE: [f32; 2] = [0.375, -0.25];

#[derive(Clone, Copy, Debug)]
enum MasterRenderPath {
    Legacy,
    MixerGraph,
}

#[derive(Clone, Copy, Debug)]
enum MasterSource {
    Asset,
    Native,
    None,
}

#[derive(Clone, Copy, Debug)]
enum MasterUiCadence {
    None,
    Sparse,
    Dense,
}

fn master_fade_project(source: MasterSource, automated: bool) -> Project {
    let length = frame_as_beat(MASTER_FADE_FRAMES as u64);
    let mut project = timeline_test_project(length);
    if matches!(source, MasterSource::Native) {
        project.channels.push(timeline_test_channel(1, 1));
        project.patterns.push(Pattern {
            id: 1,
            name: "Master fade native source".into(),
            length_steps: 16,
            channel_steps: vec![[false; 16]],
            notes: vec![PianoNote {
                id: 1,
                channel_id: Some(1),
                group_id: None,
                note: 72,
                start: 0.0,
                length,
                velocity: 1.0,
                selected: false,
                muted: false,
            }],
        });
        project.clips.push(timeline_pattern_clip(1, length, 1));
    }
    if automated {
        push_timeline_automation(
            &mut project,
            901,
            AutomationTarget::MasterVolume,
            AutomationCurve::Linear,
            [
                AutomationPoint::new(0.0, 1.0),
                AutomationPoint::new(frame_as_beat_f64(MASTER_FADE_START as u64), 1.0),
                AutomationPoint::new(frame_as_beat_f64(MASTER_FADE_END as u64), 0.0),
                AutomationPoint::new(frame_as_beat_f64(MASTER_FADE_FRAMES as u64), 0.0),
            ],
        );
    }
    project
}

fn master_callback_fixture(
    path: MasterRenderPath,
    source: MasterSource,
    automated: bool,
) -> (
    TimelineRuntimeController,
    Box<DspState>,
    AudioStatus,
    RealtimeTransport,
) {
    master_callback_fixture_for_project(
        path,
        source,
        automated,
        master_fade_project(source, automated),
    )
}

fn master_callback_fixture_for_project(
    path: MasterRenderPath,
    source: MasterSource,
    automated: bool,
    project: Project,
) -> (
    TimelineRuntimeController,
    Box<DspState>,
    AudioStatus,
    RealtimeTransport,
) {
    let timeline = compile_timeline_test_project(&project);
    let (controller, mut dsp, status) = match path {
        MasterRenderPath::Legacy => {
            // Keep the large debug DSP fixture on the heap instead of returning
            // it by value through another fixture stack frame on Windows.
            let (mut controller, realtime) = create_timeline_runtime();
            let chase = controller
                .prepare_chase(&timeline, 901, 2, 0, TimelineChaseOptions::default())
                .unwrap();
            install_test_timeline(&mut controller, 901, timeline);
            controller.install_chase(chase).unwrap();
            let mut dsp = Box::write(Box::new_uninit(), timeline_test_dsp(realtime));
            assert_eq!(dsp.apply_pending_timeline_commands(), 2);
            let status = AudioStatus::default();
            status.playing.store(true, Ordering::Release);
            dsp.apply_transport_discontinuity(
                &status,
                2,
                0,
                0,
                TransportDiscontinuity::OneShot,
                true,
            );
            (controller, dsp, status)
        }
        MasterRenderPath::MixerGraph => install_and_activate_mixer_graph_timeline(timeline, 901, 2),
    };
    // Deliberately differs from every point on most of the fade. A generated
    // matrix that is never consumed must fail even when the UI sends no command.
    dsp.master = if automated { 0.37 } else { 1.0 };
    dsp.master_pan = 0.0;
    dsp.track_gains[1] = 1.0;
    dsp.track_pans[1] = 0.0;
    if matches!(source, MasterSource::Asset) {
        let samples: Vec<f32> = (0..MASTER_FADE_FRAMES)
            .flat_map(|_| MASTER_TEST_SOURCE)
            .collect();
        let (mut retired, _reclaimed) = test_reclaimer();
        register_test_asset(&mut dsp, &mut retired, 901, &samples, 48_000, 2);
        dsp.handle(
            AudioCommand::PlayClip {
                clip_id: 901,
                asset_id: 901,
                source_frame: 0.0,
                gain: 1.0,
                mixer_track: 1,
            },
            &mut retired,
        );
    }
    if matches!(path, MasterRenderPath::MixerGraph) {
        assert_eq!(
            dsp.graph_pdc_plan
                .as_ref()
                .as_ref()
                .unwrap()
                .master_output_latency_samples(),
            0,
            "this regression intentionally covers the zero-latency domain",
        );
    }
    let transport = RealtimeTransport {
        epoch: 2,
        request: TransportRequest {
            playing: true,
            loop_enabled: false,
            ..TransportRequest::default()
        },
        ..RealtimeTransport::default()
    };
    (controller, dsp, status, transport)
}

fn render_master_callback_pcm(
    path: MasterRenderPath,
    source: MasterSource,
    automated: bool,
    partitions: &[usize],
    cadence: MasterUiCadence,
) -> Vec<[f32; 2]> {
    assert!(!partitions.is_empty());
    let (_controller, mut dsp, status, mut transport) =
        master_callback_fixture(path, source, automated);
    let mailbox = TransportMailbox::default();
    let (mut commands, mut command_reader) = RingBuffer::new(32);
    let (mut retired, _reclaimed) = test_reclaimer();
    let (mut asset_events, _asset_event_reader) = RingBuffer::new(8);
    let mut pcm = Vec::with_capacity(MASTER_FADE_FRAMES);
    let mut callback = 0;
    while pcm.len() < MASTER_FADE_FRAMES {
        let update = match cadence {
            MasterUiCadence::None => false,
            MasterUiCadence::Sparse => callback % 13 == 0,
            MasterUiCadence::Dense => true,
        };
        if update {
            // Simulate conflicting old UI-cadence writes at the real callback
            // command boundary, including multiple writes in one dense tick.
            let writes = if matches!(cadence, MasterUiCadence::Dense) {
                3
            } else {
                1
            };
            for write in 0..writes {
                let value = ((callback * 7 + write * 3) % 11) as f32 / 10.0;
                commands.push(AudioCommand::SetMaster(value)).unwrap();
            }
        }
        process_commands(
            &mut dsp,
            &mut command_reader,
            &mut retired,
            &mut asset_events,
        );
        let frames = partitions[callback % partitions.len()].min(MASTER_FADE_FRAMES - pcm.len());
        assert!(frames > 0 && frames <= MAX_MIXER_BLOCK_FRAMES);
        render_transport_chunk(
            &mut dsp,
            &status,
            &mailbox,
            &mut transport,
            frames,
            |_, block| {
                pcm.extend_from_slice(block);
            },
        );
        callback += 1;
    }
    assert_eq!(transport.timeline_frame, MASTER_FADE_FRAMES as u64);
    assert_eq!(transport.device_frame, MASTER_FADE_FRAMES as u64);
    assert_eq!(
        dsp.timeline_runtime.as_ref().unwrap().next_frame(),
        Some(MASTER_FADE_FRAMES as u64)
    );
    assert_eq!(dsp.timeline_execution_failures, 0);
    assert_eq!(dsp.timeline_missing_assets, 0);
    assert!(dsp.plugin_fault.is_none());
    pcm
}

fn master_expected_gain(frame: usize) -> f32 {
    if frame < MASTER_FADE_START {
        1.0
    } else if frame < MASTER_FADE_END {
        (1.0 - (frame - MASTER_FADE_START) as f64 / (MASTER_FADE_END - MASTER_FADE_START) as f64)
            as f32
    } else {
        0.0
    }
}

#[track_caller]
fn assert_master_pcm_close(actual: &[[f32; 2]], expected: &[[f32; 2]], context: &str) {
    assert_eq!(actual.len(), expected.len());
    for (frame, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        for channel in 0..2 {
            // Less than 1/30 of one signed 16-bit PCM unit, while allowing the
            // automation kernel's f32 endpoint interpolation across partitions.
            assert!(
                (actual[channel] - expected[channel]).abs() <= 1.0e-6,
                "{context}: frame {frame}, channel {channel}: actual {}, expected {}",
                actual[channel],
                expected[channel]
            );
        }
    }
}

#[test]
fn master_volume_callback_fades_real_asset_samples_without_ui_updates() {
    let expected: Vec<[f32; 2]> = (0..MASTER_FADE_FRAMES)
        .map(|frame| MASTER_TEST_SOURCE.map(|sample| (sample * master_expected_gain(frame)).tanh()))
        .collect();
    for path in [MasterRenderPath::Legacy, MasterRenderPath::MixerGraph] {
        let pcm = render_master_callback_pcm(
            path,
            MasterSource::Asset,
            true,
            &[512],
            MasterUiCadence::None,
        );
        assert_master_pcm_close(&pcm, &expected, &format!("{path:?}, no UI"));
        assert!(pcm[MASTER_FADE_START][0] > pcm[MASTER_FADE_END - 1][0]);
        assert!(
            pcm[MASTER_FADE_END..]
                .iter()
                .all(|frame| *frame == [0.0; 2])
        );
    }
}

#[test]
fn master_volume_callback_ownership_ignores_sparse_and_dense_legacy_ui_commands() {
    for path in [MasterRenderPath::Legacy, MasterRenderPath::MixerGraph] {
        for source in [MasterSource::Asset, MasterSource::Native] {
            let reference = render_master_callback_pcm(
                path,
                source,
                true,
                &[17, 31, 7, 63],
                MasterUiCadence::None,
            );
            assert!(reference.iter().any(|frame| frame[0] != 0.0));
            for cadence in [MasterUiCadence::Sparse, MasterUiCadence::Dense] {
                let pcm = render_master_callback_pcm(path, source, true, &[17, 31, 7, 63], cadence);
                assert_eq!(reference, pcm, "{path:?}, {source:?}, {cadence:?}");
            }
        }
    }
}

#[test]
fn master_volume_callback_fade_is_independent_of_callback_partition() {
    for path in [MasterRenderPath::Legacy, MasterRenderPath::MixerGraph] {
        for source in [MasterSource::Asset, MasterSource::Native] {
            let reference =
                render_master_callback_pcm(path, source, true, &[512], MasterUiCadence::None);
            for partitions in [&[1, 7, 63, 257][..], &[17, 128, 31, 509][..]] {
                let pcm = render_master_callback_pcm(
                    path,
                    source,
                    true,
                    partitions,
                    MasterUiCadence::None,
                );
                assert_master_pcm_close(
                    &pcm,
                    &reference,
                    &format!("{path:?}, {source:?}, {partitions:?}"),
                );
            }
        }
    }
}

#[test]
fn master_volume_callback_fades_compiled_native_notes_after_source_mix() {
    for path in [MasterRenderPath::Legacy, MasterRenderPath::MixerGraph] {
        let unity = render_master_callback_pcm(
            path,
            MasterSource::Native,
            false,
            &[512],
            MasterUiCadence::None,
        );
        let expected: Vec<[f32; 2]> = unity
            .iter()
            .enumerate()
            .map(|(frame, source)| {
                // Independent full-gain production render supplies the oscillator
                // source. Undo only the final soft clip, then apply the authored
                // master envelope; no duplicated oscillator or matrix helper.
                source.map(|sample| {
                    (f64::from(sample).atanh() * f64::from(master_expected_gain(frame))).tanh()
                        as f32
                })
            })
            .collect();
        let pcm = render_master_callback_pcm(
            path,
            MasterSource::Native,
            true,
            &[17, 31, 7, 63],
            MasterUiCadence::None,
        );
        assert!(
            unity[MASTER_FADE_END..]
                .iter()
                .any(|frame| frame[0].abs() > 0.01)
        );
        assert_master_pcm_close(&pcm, &expected, &format!("{path:?}, compiled native note"));
        assert!(
            pcm[MASTER_FADE_END..]
                .iter()
                .all(|frame| *frame == [0.0; 2])
        );
    }
}

fn render_manual_master_preview(path: Option<MasterRenderPath>, gain: f32) -> Vec<[f32; 2]> {
    let (mut dsp, status, _controller) = if let Some(path) = path {
        let (controller, mut dsp, status, mut transport) =
            master_callback_fixture(path, MasterSource::None, true);
        render_transport_chunk(
            &mut dsp,
            &status,
            &TransportMailbox::default(),
            &mut transport,
            32,
            |_, _| {},
        );
        dsp.pause_timeline();
        status.playing.store(false, Ordering::Release);
        assert!(!dsp.prepare_timeline_render(2, 32, transport.beat_q32, 64, false));
        (dsp, status, Some(controller))
    } else {
        (
            Box::new(DspState::new(48_000.0)),
            AudioStatus::default(),
            None,
        )
    };
    let (mut retired, _reclaimed) = test_reclaimer();
    dsp.handle(AudioCommand::SetMaster(gain), &mut retired);
    dsp.handle(
        AudioCommand::NoteOn {
            note: 69,
            velocity: 1.0,
            mixer_track: 0,
        },
        &mut retired,
    );
    // Paused native audition still uses the production downstream mixer. An old
    // finished Matrix must not override a manual master after playback stops.
    dsp.render_block(&status, 64);
    assert_eq!(dsp.master, gain);
    if path.is_some() {
        assert_eq!(
            dsp.timeline_runtime.as_ref().unwrap().next_frame(),
            Some(32)
        );
    }
    dsp.master_block[..64].to_vec()
}

#[test]
fn master_volume_manual_control_survives_stop_and_missing_callback_binding() {
    for path in [
        None,
        Some(MasterRenderPath::Legacy),
        Some(MasterRenderPath::MixerGraph),
    ] {
        let unity = render_manual_master_preview(path, 1.0);
        let manual = render_manual_master_preview(path, 0.25);
        let expected: Vec<[f32; 2]> = unity
            .iter()
            .map(|source| source.map(|sample| (f64::from(sample).atanh() * 0.25).tanh() as f32))
            .collect();
        assert!(unity.iter().any(|frame| frame[0].abs() > 0.01));
        assert_master_pcm_close(
            &manual,
            &expected,
            &format!("{path:?}, stopped manual preview"),
        );
    }
}

const MASTER_PDC_SOURCE_INSTANCE: u64 = 20_001;
const MASTER_PDC_FIRST_INSTANCE: u64 = 20_002;
const MASTER_PDC_SECOND_INSTANCE: u64 = 20_003;
const MASTER_PDC_SOURCE_LATENCY: u32 = 23;
const MASTER_PDC_FIRST_LATENCY: u32 = 11;
const MASTER_PDC_SECOND_LATENCY: u32 = 7;
const MASTER_PDC_SOURCE: f32 = 0.125;
const MASTER_PDC_PROJECT_FRAMES: usize = 8_192;

// Reuse the existing physical delay backend instead of substituting a matrix
// or delay-line oracle for the production endpoint path. The wrapper also clears
// its actual plug-in delay memory on reset, as a well-behaved real plug-in must.
struct MasterRecordingBackend {
    inner: MockInsertBackend,
    processed_frames: Arc<AtomicU64>,
}

impl PluginBackend for MasterRecordingBackend {
    fn name(&self) -> &str {
        "master automation PCM recording fixture"
    }

    fn prepare(&mut self, config: PluginPrepareConfig) -> Result<(), String> {
        self.inner.prepare(config)
    }

    fn process(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        frames: usize,
    ) -> Result<(), String> {
        self.inner.process(left, right, frames)?;
        self.processed_frames
            .fetch_add(frames as u64, Ordering::Release);
        Ok(())
    }

    fn reset_processing(&mut self) -> Result<(), String> {
        self.inner.delay_left.fill(0.0);
        self.inner.delay_right.fill(0.0);
        self.inner.delay_index = 0;
        Ok(())
    }

    fn send_midi(&mut self, message: MidiMessage) -> Result<(), String> {
        self.inner.send_midi(message)
    }

    fn set_parameter(&mut self, id: u32, normalized: f32) -> Result<(), String> {
        self.inner.set_parameter(id, normalized)
    }

    fn get_parameter(&mut self, id: u32) -> Result<f32, String> {
        self.inner.get_parameter(id)
    }

    fn save_state(&mut self) -> Result<Vec<u8>, String> {
        self.inner.save_state()
    }

    fn load_state(&mut self, state: &[u8]) -> Result<(), String> {
        self.inner.load_state(state)
    }

    fn latency_samples(&self) -> u32 {
        self.inner.latency_samples()
    }

    fn tail_samples(&self) -> u32 {
        0
    }
}

fn master_recording_slot(add: f32, latency: u32, recorded: &Arc<AtomicU64>) -> BackendSlot {
    BackendSlot::new(Box::new(MasterRecordingBackend {
        inner: MockInsertBackend {
            add,
            gain: 1.0,
            last_midi: Arc::new(AtomicU32::new(0)),
            parameter: Arc::new(AtomicU32::new(0)),
            latency: Arc::new(AtomicU32::new(latency)),
            process_delay: Duration::ZERO,
            delay_left: [0.0; 512],
            delay_right: [0.0; 512],
            delay_index: 0,
        },
        processed_frames: Arc::clone(recorded),
    }))
}

fn master_pdc_project() -> Project {
    let mut project = master_fade_project(MasterSource::None, true);
    project.song_length_beats = frame_as_beat(MASTER_PDC_PROJECT_FRAMES as u64);
    let mut channel = timeline_test_channel(1, 1);
    channel.instrument_plugin_instance_id = Some(MASTER_PDC_SOURCE_INSTANCE);
    project.channels.push(channel);
    project
        .plugin_instances
        .push(timeline_test_plugin(MASTER_PDC_SOURCE_INSTANCE));
    for (slot, instance) in [
        (0, MASTER_PDC_FIRST_INSTANCE),
        (1, MASTER_PDC_SECOND_INSTANCE),
    ] {
        let mut plugin = timeline_test_plugin(instance);
        plugin.role = PluginRole::Effect;
        project.plugin_instances.push(plugin);
        project.mixer_insert_slots.push(MixerInsertSlotRef {
            track: crate::mixer_graph::MASTER_MIXER_TRACK_ID,
            slot,
            plugin_instance_id: instance,
        });
    }
    project
}

struct MasterPdcFixture {
    dsp: Box<DspState>,
    controller: TimelineRuntimeController,
    timeline: Arc<CompiledTimeline>,
    transport: RealtimeTransport,
    status: AudioStatus,
    mailbox: TransportMailbox,
    timing: PreparedPluginTimingPlan,
    loop_token: u64,
    controls: Vec<PluginChainControl>,
    guards: Vec<PluginWorkerGuard>,
    retired: Consumer<RetiredEndpointResource>,
    recorded: Arc<AtomicU64>,
}

impl MasterPdcFixture {
    fn new(budget: u32) -> Self {
        Self::with_master_capacity(budget, 8_192)
    }

    fn with_master_capacity(budget: u32, master_capacity: u32) -> Self {
        Self::with_project_and_capacities(master_pdc_project(), budget, master_capacity, 8_192)
    }

    fn with_project_and_capacities(
        project: Project,
        budget: u32,
        master_capacity: u32,
        pan_capacity: u32,
    ) -> Self {
        Self::with_project_and_three_capacities(
            project,
            budget,
            master_capacity,
            pan_capacity,
            8_192,
        )
    }

    fn with_project_and_three_capacities(
        project: Project,
        budget: u32,
        master_capacity: u32,
        pan_capacity: u32,
        mute_capacity: u32,
    ) -> Self {
        let manual_master_muted = project.mixer_tracks[0].muted;
        let timeline = compile_timeline_test_project(&project);
        let (mut controller, realtime) = create_timeline_runtime();
        install_test_timeline(&mut controller, 990, Arc::clone(&timeline));
        let loop_chase = controller
            .prepare_loop_chase(&timeline, 990, 0, TimelineChaseOptions::default())
            .unwrap();
        let loop_token = controller.install_loop_chase(loop_chase).unwrap();
        let (retired_tx, retired) = RingBuffer::new(8);
        let (insert_events, _insert_reader) = RingBuffer::new(8);
        let (generator_events, _generator_reader) = RingBuffer::new(8);
        let dsp = Box::write(
            Box::new_uninit(),
            DspState::try_new_inner(
                48_000.0,
                Some(retired_tx),
                Some(insert_events),
                Some(generator_events),
                None,
                8_192,
                Some(realtime),
            )
            .unwrap(),
        );
        let mut fixture = Self {
            dsp,
            controller,
            timeline,
            transport: RealtimeTransport::default(),
            status: AudioStatus::default(),
            mailbox: TransportMailbox::default(),
            timing: PreparedPluginTimingPlan::new(2, 48_000, budget).unwrap(),
            loop_token,
            controls: Vec::with_capacity(2),
            guards: Vec::with_capacity(2),
            retired,
            recorded: Arc::new(AtomicU64::new(0)),
        };
        fixture.dsp.master = 0.37;
        fixture.dsp.timeline_master_volume =
            MasterVolumeAutomation::new(master_capacity, MAX_MIXER_BLOCK_FRAMES).unwrap();
        fixture.dsp.timeline_master_pan =
            MasterOutputAutomation::new_pan(pan_capacity, MAX_MIXER_BLOCK_FRAMES).unwrap();
        fixture.dsp.timeline_master_mute =
            MasterOutputAutomation::new_mute(mute_capacity, MAX_MIXER_BLOCK_FRAMES).unwrap();
        let (mut retired, _reclaimed) = test_reclaimer();
        fixture.dsp.handle(
            AudioCommand::SetTrackMuted {
                track: 0,
                muted: manual_master_muted,
            },
            &mut retired,
        );
        fixture.dsp.track_gains.fill(1.0);
        fixture.dsp.master_pan = 0.0;
        fixture.dsp.requested_plugin_timing_revision = fixture.timing.revision;
        fixture
            .status
            .plugin_processing
            .requested_revision
            .store(fixture.timing.revision, Ordering::Release);
        fixture
            .status
            .plugin_processing
            .requested_budget
            .store(budget, Ordering::Release);

        for master in [false, true] {
            let instances = if master {
                vec![MASTER_PDC_FIRST_INSTANCE, MASTER_PDC_SECOND_INSTANCE]
            } else {
                vec![MASTER_PDC_SOURCE_INSTANCE]
            };
            let recorded = Arc::clone(&fixture.recorded);
            let chain = PluginChain::spawn_identified_with_backend_factory(
                &instances,
                move || {
                    if master {
                        vec![
                            master_recording_slot(0.0, MASTER_PDC_FIRST_LATENCY, &recorded),
                            master_recording_slot(0.0, MASTER_PDC_SECOND_LATENCY, &recorded),
                        ]
                    } else {
                        vec![master_recording_slot(
                            MASTER_PDC_SOURCE,
                            MASTER_PDC_SOURCE_LATENCY,
                            &recorded,
                        )]
                    }
                },
                PluginPrepareConfig {
                    sample_rate: 48_000.0,
                    max_block_frames: 128,
                },
            )
            .expect("identified recording backend preparation");
            let PluginChain {
                audio,
                control,
                guard,
            } = chain;
            // Retain worker ownership before any assertion or readiness wait.
            fixture.controls.push(control);
            fixture.guards.push(guard);
            let control = fixture.controls.last().unwrap();
            wait_until(|| {
                control.plugin_latency_snapshot().is_some_and(|snapshot| {
                    snapshot.revision != 0
                        && snapshot.total_plugin_latency_samples
                            == if master {
                                MASTER_PDC_FIRST_LATENCY + MASTER_PDC_SECOND_LATENCY
                            } else {
                                MASTER_PDC_SOURCE_LATENCY
                            }
                })
            });
            if master {
                fixture
                    .dsp
                    .install_insert_endpoint(0, 29_002, fixed_adapter(audio));
            } else {
                fixture.dsp.install_generator_endpoint(
                    1,
                    29_001,
                    MASTER_PDC_SOURCE_INSTANCE,
                    1,
                    fixed_adapter(audio),
                    test_pdc_delay(),
                );
            }
        }
        assert_eq!(fixture.dsp.apply_pending_timeline_commands(), 2);
        fixture.activate(2, 0);
        assert_eq!(
            fixture.delay(),
            2 * u64::from(fixture.timing.bridge_latency_frames)
                + u64::from(
                    MASTER_PDC_SOURCE_LATENCY
                        + MASTER_PDC_FIRST_LATENCY
                        + MASTER_PDC_SECOND_LATENCY
                )
        );
        fixture
    }

    fn delay(&self) -> u64 {
        self.dsp
            .graph_pdc_plan
            .as_ref()
            .as_ref()
            .unwrap()
            .master_output_latency_samples()
    }

    fn queue_activation(&mut self, epoch: u64, frame: u64) {
        let mut chase = self
            .controller
            .prepare_chase(
                &self.timeline,
                990,
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
                    revision: 990,
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
    }

    fn activate(&mut self, epoch: u64, frame: u64) {
        self.queue_activation(epoch, frame);
        self.dsp.refresh_pdc_plan(&self.status, 128);
        self.transport
            .apply_pending_timeline_activation(&self.status, &mut self.dsp);
        assert_eq!(self.transport.epoch, epoch);
        assert_eq!(self.transport.timeline_frame, frame);
        assert!(self.dsp.mixer_graph_binding_is_exact());
        self.wait_workers();
    }

    fn wait_workers(&self) {
        for control in &self.controls {
            let submitted = control.stats().submitted;
            wait_until(|| {
                let stats = control.stats();
                stats.completed + stats.dropped_old_epoch_inputs >= submitted || stats.stopped
            });
        }
    }

    fn render(&mut self, frames: usize) -> Vec<[f32; 2]> {
        let mut pcm = Vec::with_capacity(frames);
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
        assert!(self.dsp.admit_plugin_callback(frames));
        render_transport_chunk(
            &mut self.dsp,
            &self.status,
            &self.mailbox,
            &mut self.transport,
            frames,
            |_, block| pcm.extend_from_slice(block),
        );
        self.wait_workers();
        assert!(self.dsp.plugin_fault.is_none());
        assert_eq!(self.dsp.timeline_execution_failures, 0);
        pcm
    }

    fn render_frames(&mut self, frames: usize, partitions: &[usize]) -> Vec<[f32; 2]> {
        let mut pcm = Vec::with_capacity(frames);
        let mut callback = 0;
        while pcm.len() < frames {
            pcm.extend(
                self.render(partitions[callback % partitions.len()].min(frames - pcm.len())),
            );
            callback += 1;
        }
        pcm
    }
}

impl Drop for MasterPdcFixture {
    fn drop(&mut self) {
        // Test-driver cleanup; no endpoint or worker is destroyed in a callback.
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

fn expected_master_pdc_pcm(frames: usize, delay: usize, seek_frame: usize) -> Vec<[f32; 2]> {
    (0..frames)
        .map(|frame| {
            let gain = if frame < delay {
                0.0
            } else {
                master_expected_gain(seek_frame + frame - delay)
            };
            [(MASTER_PDC_SOURCE * gain).tanh(); 2]
        })
        .collect()
}

#[test]
fn master_volume_pcm_tracks_source_frame_through_generator_bridge_and_master_prefix() {
    for budget in [128, 512] {
        let mut fixture = MasterPdcFixture::new(budget);
        let delay = fixture.delay() as usize;
        let total = delay + MASTER_FADE_FRAMES;
        let pcm = fixture.render_frames(total, &[1, 31, 64, 127, 128]);
        let expected = expected_master_pdc_pcm(total, delay, 0);
        assert_master_pcm_close(
            &pcm,
            &expected,
            &format!("real generator/master prefix, budget {budget}"),
        );
        assert!(fixture.recorded.load(Ordering::Acquire) > 0);
        assert!(pcm[..delay].iter().all(|frame| *frame == [0.0; 2]));
        assert!(
            pcm[delay + MASTER_FADE_END..]
                .iter()
                .all(|frame| *frame == [0.0; 2])
        );
        assert!(pcm[delay + MASTER_FADE_START][0] > 0.1);
        for control in &fixture.controls {
            let stats = control.stats();
            assert_eq!(stats.deadline_misses, 0);
            assert_eq!(stats.input_overflows, 0);
            assert_eq!(stats.output_overflows, 0);
        }
    }
}

#[test]
fn master_volume_nonzero_pdc_seek_epoch_rebind_discards_old_gain_history_in_final_pcm() {
    let mut warm = MasterPdcFixture::new(128);
    let delay = warm.delay() as usize;
    // Fill the old master history with the completed fade's zero value.
    warm.render_frames(delay + MASTER_FADE_FRAMES + 128, &[128]);
    warm.activate(3, 256);
    let mut fresh = MasterPdcFixture::new(128);
    fresh.activate(3, 256);
    let total = delay + 320;
    let actual = warm.render_frames(total, &[31, 64, 127, 128]);
    let reference = fresh.render_frames(total, &[31, 64, 127, 128]);
    let expected = expected_master_pdc_pcm(total, delay, 256);
    assert_master_pcm_close(&reference, &expected, "fresh nonzero PDC seek");
    assert_master_pcm_close(
        &actual,
        &reference,
        "warm history must not leak across epoch seek",
    );
    assert!(actual[delay][0] > 0.05);
    assert!(
        actual[delay + MASTER_FADE_END - 256..]
            .iter()
            .all(|frame| *frame == [0.0; 2])
    );
}

#[test]
fn master_volume_failed_activation_preserves_active_delayed_gain_pcm() {
    let mut candidate = MasterPdcFixture::new(128);
    let mut reference = MasterPdcFixture::new(128);
    assert_master_pcm_close(
        &candidate.render_frames(384, &[128]),
        &reference.render_frames(384, &[128]),
        "before rejected candidate",
    );
    let transport_before = candidate.transport;
    let master_before = candidate.dsp.timeline_master_volume.identity();
    let master_cursor_before = candidate.dsp.timeline_master_volume.next_source_frame();
    let pdc_revision_before = candidate.dsp.pdc_plan_revision;
    let executor_frame_before = candidate.dsp.timeline_executor.next_frame();
    assert_eq!(
        candidate.dsp.requested_plugin_timing_revision,
        candidate.timing.revision
    );
    assert_eq!(
        candidate
            .status
            .plugin_processing
            .requested_revision
            .load(Ordering::Acquire),
        candidate.timing.revision
    );
    while candidate.controller.poll_event().is_some() {}
    candidate.queue_activation(3, 0);
    while candidate.controller.poll_event().is_some() {}
    let original = candidate.controls[1].plugin_latency_snapshot().unwrap();
    let changed = crate::plugins::plugin_runtime::PluginLatencySnapshot {
        revision: next_nonzero_id(original.revision),
        ..original
    };
    // A same-revision candidate can reuse a cached graph generation and perform
    // only its final fresh read. Start with the changed publication so that path
    // must reject as well; a full capture observes a change back to the actual
    // worker's original generation on the second read and must also reject.
    candidate.dsp.insert_endpoints[0]
        .as_mut()
        .unwrap()
        .endpoint
        .script_fresh_endpoint_snapshots([Some(changed), Some(original)]);
    candidate
        .transport
        .apply_pending_timeline_activation(&candidate.status, &mut candidate.dsp);
    let fresh_reads = candidate.dsp.insert_endpoints[0]
        .as_ref()
        .unwrap()
        .endpoint
        .fresh_snapshot_script_cursor;
    assert!(
        (1..=2).contains(&fresh_reads),
        "negative fixture must actually observe the scripted endpoint drift"
    );
    assert!(
        matches!(
            candidate.controller.poll_event(),
            Some(TimelineRuntimeEvent::TransportActivationRejected {
                revision: 990,
                reason: TimelineTransportActivationRejectReason::GraphPdcPlan,
                ..
            })
        ),
        "the candidate must produce an actual rejection receipt"
    );
    assert_eq!(candidate.transport, transport_before);
    assert_eq!(
        candidate
            .dsp
            .timeline_runtime
            .as_ref()
            .unwrap()
            .active_epoch(),
        Some(2)
    );
    assert!(
        candidate
            .dsp
            .pending_timeline_transport_activation()
            .is_none()
    );
    assert_eq!(
        candidate.dsp.timeline_master_volume.identity(),
        master_before
    );
    assert_eq!(
        candidate.dsp.timeline_master_volume.next_source_frame(),
        master_cursor_before
    );
    assert_eq!(candidate.dsp.pdc_plan_revision, pdc_revision_before);
    assert_eq!(
        candidate.dsp.timeline_executor.next_frame(),
        executor_frame_before
    );
    let total = candidate.delay() as usize + MASTER_FADE_FRAMES;
    let actual = candidate.render_frames(total, &[31, 64, 127, 128]);
    let expected = reference.render_frames(total, &[31, 64, 127, 128]);
    assert!(expected.iter().any(|frame| frame[0] > 0.1));
    assert_master_pcm_close(
        &actual,
        &expected,
        "late endpoint precommit rejection must preserve active history",
    );
}

#[test]
fn master_volume_nonzero_pdc_pause_freezes_history_and_resume_matches_continuous_pcm() {
    let mut paused = MasterPdcFixture::new(128);
    let mut continuous = MasterPdcFixture::new(128);
    let before = paused.render_frames(256, &[128]);
    assert_master_pcm_close(
        &before,
        &continuous.render_frames(256, &[128]),
        "before pause",
    );
    paused.mailbox.publish(TransportMutation::SetPlaying(false));
    paused
        .transport
        .apply_latest_request(&paused.mailbox, &paused.status, &mut paused.dsp);
    let source_frame = paused.transport.timeline_frame;
    let device_frame = paused.transport.device_frame;
    let silent = paused.render_frames(768, &[31, 64, 127, 128]);
    assert!(silent.iter().all(|frame| *frame == [0.0; 2]));
    assert_eq!(paused.transport.timeline_frame, source_frame);
    assert_eq!(paused.transport.device_frame, device_frame + 768);
    assert_eq!(
        paused.dsp.timeline_runtime.as_ref().unwrap().next_frame(),
        Some(source_frame)
    );
    paused.mailbox.publish(TransportMutation::SetPlaying(true));
    paused
        .transport
        .apply_latest_request(&paused.mailbox, &paused.status, &mut paused.dsp);
    assert_eq!(paused.transport.epoch, 2);
    let total = paused.delay() as usize + MASTER_FADE_FRAMES;
    let actual = paused.render_frames(total, &[31, 64, 127, 128]);
    let expected = continuous.render_frames(total, &[31, 64, 127, 128]);
    assert!(expected.iter().any(|frame| frame[0] > 0.1));
    assert_master_pcm_close(
        &actual,
        &expected,
        "paused device time must not advance source gain history",
    );
}

#[test]
fn master_volume_over_capacity_activation_rejects_without_changing_active_pcm() {
    let mut candidate = MasterPdcFixture::with_master_capacity(128, 1_200);
    let mut reference = MasterPdcFixture::with_master_capacity(128, 1_200);
    assert_master_pcm_close(
        &candidate.render_frames(384, &[128]),
        &reference.render_frames(384, &[128]),
        "before capacity rejection",
    );
    let transport_before = candidate.transport;
    let master_before = candidate.dsp.timeline_master_volume.identity();
    let cursor_before = candidate.dsp.timeline_master_volume.next_source_frame();
    let current_timing = candidate.timing;
    while candidate.controller.poll_event().is_some() {}
    candidate.timing = PreparedPluginTimingPlan::new(3, 48_000, 512).unwrap();
    candidate.dsp.requested_plugin_timing_revision = candidate.timing.revision;
    candidate
        .status
        .plugin_processing
        .requested_revision
        .store(candidate.timing.revision, Ordering::Release);
    candidate
        .status
        .plugin_processing
        .requested_budget
        .store(512, Ordering::Release);
    candidate.queue_activation(3, 0);
    while candidate.controller.poll_event().is_some() {}
    candidate
        .transport
        .apply_pending_timeline_activation(&candidate.status, &mut candidate.dsp);
    assert_eq!(candidate.transport, transport_before);
    assert_eq!(candidate.dsp.plugin_timing, current_timing);
    assert_eq!(
        candidate.dsp.timeline_master_volume.identity(),
        master_before
    );
    assert_eq!(
        candidate.dsp.timeline_master_volume.next_source_frame(),
        cursor_before
    );
    assert!(
        candidate
            .dsp
            .pending_timeline_transport_activation()
            .is_none()
    );
    assert_eq!(
        candidate
            .dsp
            .timeline_runtime
            .as_ref()
            .unwrap()
            .active_epoch(),
        Some(2)
    );
    assert!(matches!(
        candidate.controller.poll_event(),
        Some(TimelineRuntimeEvent::TransportActivationRejected {
            revision: 990,
            reason: TimelineTransportActivationRejectReason::GraphPdcPlan,
            ..
        })
    ));
    candidate.timing = current_timing;
    candidate.dsp.requested_plugin_timing_revision = current_timing.revision;
    candidate
        .status
        .plugin_processing
        .requested_revision
        .store(current_timing.revision, Ordering::Release);
    candidate
        .status
        .plugin_processing
        .requested_budget
        .store(current_timing.callback_budget_frames, Ordering::Release);
    let total = candidate.delay() as usize + MASTER_FADE_FRAMES;
    assert_master_pcm_close(
        &candidate.render_frames(total, &[31, 64, 127, 128]),
        &reference.render_frames(total, &[31, 64, 127, 128]),
        "rejected output delay must preserve the prior delayed gain history",
    );
}

fn master_expected_pan(frame: usize) -> f32 {
    if frame < MASTER_FADE_START {
        -1.0
    } else if frame < MASTER_FADE_END {
        (-1.0
            + 2.0 * (frame - MASTER_FADE_START) as f64
                / (MASTER_FADE_END - MASTER_FADE_START) as f64) as f32
    } else {
        1.0
    }
}

fn master_pan_project(source: MasterSource) -> Project {
    let mut project = master_fade_project(source, false);
    push_timeline_automation(
        &mut project,
        902,
        AutomationTarget::MasterPan,
        AutomationCurve::Linear,
        [
            AutomationPoint::new(0.0, -1.0),
            AutomationPoint::new(frame_as_beat_f64(MASTER_FADE_START as u64), -1.0),
            AutomationPoint::new(frame_as_beat_f64(MASTER_FADE_END as u64), 1.0),
            AutomationPoint::new(frame_as_beat_f64(MASTER_FADE_FRAMES as u64), 1.0),
        ],
    );
    project
}

fn render_master_pan_pcm(
    path: MasterRenderPath,
    source: MasterSource,
    project: Project,
    partitions: &[usize],
    cadence: MasterUiCadence,
) -> Vec<[f32; 2]> {
    let (_controller, mut dsp, status, mut transport) =
        master_callback_fixture_for_project(path, source, false, project);
    dsp.master_pan = 0.63;
    let mut pcm = Vec::with_capacity(MASTER_FADE_FRAMES);
    let (mut commands, mut reader) = RingBuffer::new(16);
    let (mut retired, _reclaimed) = test_reclaimer();
    let (mut events, _event_reader) = RingBuffer::new(8);
    let mut callback = 0;
    while pcm.len() < MASTER_FADE_FRAMES {
        if matches!(cadence, MasterUiCadence::Dense)
            || (matches!(cadence, MasterUiCadence::Sparse) && callback % 13 == 0)
        {
            commands
                .push(AudioCommand::SetMasterPan(
                    (callback % 11) as f32 / 5.0 - 1.0,
                ))
                .unwrap();
        }
        process_commands(&mut dsp, &mut reader, &mut retired, &mut events);
        let frames = partitions[callback % partitions.len()].min(MASTER_FADE_FRAMES - pcm.len());
        render_transport_chunk(
            &mut dsp,
            &status,
            &TransportMailbox::default(),
            &mut transport,
            frames,
            |_, block| pcm.extend_from_slice(block),
        );
        callback += 1;
    }
    assert_eq!(dsp.timeline_execution_failures, 0);
    assert_eq!(dsp.timeline_automation_pending, 0);
    pcm
}

#[test]
fn master_pan_callback_applies_signed_balance_in_final_asset_and_native_pcm() {
    for path in [MasterRenderPath::Legacy, MasterRenderPath::MixerGraph] {
        for source in [MasterSource::Asset, MasterSource::Native] {
            let unity =
                render_master_callback_pcm(path, source, false, &[512], MasterUiCadence::None);
            let actual = render_master_pan_pcm(
                path,
                source,
                master_pan_project(source),
                &[31, 127, 17],
                MasterUiCadence::None,
            );
            let expected: Vec<[f32; 2]> = unity
                .iter()
                .enumerate()
                .map(|(frame, sample)| {
                    let pan = master_expected_pan(frame);
                    [
                        (f64::from(sample[0]).atanh() * f64::from(1.0 - pan.max(0.0))).tanh()
                            as f32,
                        (f64::from(sample[1]).atanh() * f64::from(1.0 + pan.min(0.0))).tanh()
                            as f32,
                    ]
                })
                .collect();
            assert_master_pcm_close(
                &actual,
                &expected,
                "sample-owned signed master pan after source mix",
            );
            assert!(
                actual[..MASTER_FADE_START]
                    .iter()
                    .all(|frame| frame[1] == 0.0)
            );
            assert!(
                actual[MASTER_FADE_END..]
                    .iter()
                    .all(|frame| frame[0] == 0.0)
            );
        }
    }
}

#[test]
fn master_pan_callback_pcm_ignores_ui_cadence_and_block_partition() {
    for path in [MasterRenderPath::Legacy, MasterRenderPath::MixerGraph] {
        let reference = render_master_pan_pcm(
            path,
            MasterSource::Asset,
            master_pan_project(MasterSource::Asset),
            &[512],
            MasterUiCadence::None,
        );
        for cadence in [
            MasterUiCadence::None,
            MasterUiCadence::Sparse,
            MasterUiCadence::Dense,
        ] {
            for partitions in [&[1, 31, 127][..], &[17, 257, 63][..]] {
                let actual = render_master_pan_pcm(
                    path,
                    MasterSource::Asset,
                    master_pan_project(MasterSource::Asset),
                    partitions,
                    cadence,
                );
                assert_master_pcm_close(
                    &actual,
                    &reference,
                    "UI cadence/partition must not change final pan PCM",
                );
            }
        }
    }
}

#[test]
fn master_pan_placed_hold_returns_to_zero_base_without_gain_division() {
    for path in [MasterRenderPath::Legacy, MasterRenderPath::MixerGraph] {
        let mut project = master_fade_project(MasterSource::Asset, false);
        project.mixer_tracks[0].pan = 0.0;
        push_timeline_automation(
            &mut project,
            903,
            AutomationTarget::MasterPan,
            AutomationCurve::Hold,
            [AutomationPoint::new(0.0, -1.0)],
        );
        let mut clip = timeline_pattern_clip(903, frame_as_beat(128), 0);
        clip.kind = ClipKind::Automation;
        clip.automation_id = Some(903);
        clip.start = frame_as_beat(64);
        project.clips.push(clip);
        let actual = render_master_pan_pcm(
            path,
            MasterSource::Asset,
            project,
            &[31, 127, 17],
            MasterUiCadence::Dense,
        );
        let expected: Vec<[f32; 2]> = (0..MASTER_FADE_FRAMES)
            .map(|frame| {
                [
                    MASTER_TEST_SOURCE[0].tanh(),
                    if (64..192).contains(&frame) {
                        0.0
                    } else {
                        MASTER_TEST_SOURCE[1].tanh()
                    },
                ]
            })
            .collect();
        assert_master_pcm_close(
            &actual,
            &expected,
            "placed pan uses true zero base before/after its half-open range",
        );
    }
}

fn master_pan_pdc_project() -> Project {
    let mut project = master_pdc_project();
    project.automation_lanes.clear();
    push_timeline_automation(
        &mut project,
        904,
        AutomationTarget::MasterVolume,
        AutomationCurve::Hold,
        [AutomationPoint::new(0.0, 1.0)],
    );
    push_timeline_automation(
        &mut project,
        905,
        AutomationTarget::MasterPan,
        AutomationCurve::Linear,
        [
            AutomationPoint::new(0.0, -1.0),
            AutomationPoint::new(frame_as_beat_f64(MASTER_FADE_START as u64), -1.0),
            AutomationPoint::new(frame_as_beat_f64(MASTER_FADE_END as u64), 1.0),
            AutomationPoint::new(frame_as_beat_f64(MASTER_PDC_PROJECT_FRAMES as u64), 1.0),
        ],
    );
    project
}

fn pan_pdc_fixture(budget: u32, pan_capacity: u32) -> MasterPdcFixture {
    let mut fixture = MasterPdcFixture::with_project_and_capacities(
        master_pan_pdc_project(),
        budget,
        8_192,
        pan_capacity,
    );
    fixture.dsp.master_pan = 0.63;
    fixture
}

fn expected_pan_pdc_pcm(frames: usize, delay: usize, seek: usize) -> Vec<[f32; 2]> {
    (0..frames)
        .map(|frame| {
            if frame < delay {
                [0.0; 2]
            } else {
                let pan = master_expected_pan(seek + frame - delay);
                [
                    (MASTER_PDC_SOURCE * (1.0 - pan.max(0.0))).tanh(),
                    (MASTER_PDC_SOURCE * (1.0 + pan.min(0.0))).tanh(),
                ]
            }
        })
        .collect()
}

#[test]
fn master_pan_nonzero_pdc_pcm_matches_output_source_domain_with_both_controls_owned() {
    for budget in [128, 512] {
        let mut fixture = pan_pdc_fixture(budget, 8_192);
        let delay = fixture.delay() as usize;
        let total = delay + MASTER_FADE_FRAMES;
        let actual = fixture.render_frames(total, &[1, 31, 64, 127, 128]);
        assert_eq!(fixture.dsp.timeline_automation_pending, 0);
        assert_master_pcm_close(
            &actual,
            &expected_pan_pdc_pcm(total, delay, 0),
            "master pan T-output_delay through generator and master prefix",
        );
        assert!(
            actual[delay + MASTER_FADE_END..]
                .iter()
                .all(|sample| sample[0] == 0.0)
        );
    }
}

#[test]
fn master_pan_pdc_seek_and_installed_loop_discontinuity_reseed_discard_old_pan() {
    let mut warm = pan_pdc_fixture(128, 8_192);
    let delay = warm.delay() as usize;
    warm.render_frames(delay + MASTER_FADE_FRAMES, &[128]);
    warm.activate(3, 128);
    let mut fresh = pan_pdc_fixture(128, 8_192);
    fresh.activate(3, 128);
    let total = delay + 256;
    assert_master_pcm_close(
        &warm.render_frames(total, &[31, 127, 128]),
        &fresh.render_frames(total, &[31, 127, 128]),
        "pan seek must discard previous right-only history",
    );
    // Exercise the installed Loop chase production path, rather than pretending
    // an ordinary seek is evidence of loop-discontinuity history reset.
    warm.dsp.apply_transport_discontinuity(
        &warm.status,
        4,
        0,
        0,
        TransportDiscontinuity::Loop,
        true,
    );
    assert_eq!(warm.dsp.timeline_channel_epoch, Some(4));
    warm.transport.epoch = 4;
    warm.transport.timeline_frame = 0;
    warm.transport.beat_q32 = 0;
    warm.transport.request.target_timeline_frame = 0;
    warm.transport.request.target_beat_q32 = 0;
    warm.transport.publish(&warm.status);
    warm.wait_workers();
    assert_master_pcm_close(
        &warm.render_frames(delay + 128, &[31, 127, 128]),
        &expected_pan_pdc_pcm(delay + 128, delay, 0),
        "installed loop chase must reseed left pan",
    );
}

#[test]
fn master_pan_pdc_pause_freezes_control_history_and_manual_preview_uses_manual_pan() {
    let mut paused = pan_pdc_fixture(128, 8_192);
    let mut reference = pan_pdc_fixture(128, 8_192);
    assert_master_pcm_close(
        &paused.render_frames(384, &[128]),
        &reference.render_frames(384, &[128]),
        "before pan pause",
    );
    paused.mailbox.publish(TransportMutation::SetPlaying(false));
    paused
        .transport
        .apply_latest_request(&paused.mailbox, &paused.status, &mut paused.dsp);
    let next = paused.dsp.timeline_master_pan.next_source_frame();
    assert!(
        paused
            .render_frames(768, &[31, 127, 128])
            .iter()
            .all(|frame| *frame == [0.0; 2])
    );
    assert_eq!(paused.dsp.timeline_master_pan.next_source_frame(), next);
    paused.mailbox.publish(TransportMutation::SetPlaying(true));
    paused
        .transport
        .apply_latest_request(&paused.mailbox, &paused.status, &mut paused.dsp);
    let total = paused.delay() as usize + 256;
    assert_master_pcm_close(
        &paused.render_frames(total, &[31, 127, 128]),
        &reference.render_frames(total, &[31, 127, 128]),
        "paused device frames must not advance pan history",
    );
    for path in [MasterRenderPath::Legacy, MasterRenderPath::MixerGraph] {
        let (_controller, mut dsp, status, mut transport) = master_callback_fixture_for_project(
            path,
            MasterSource::None,
            false,
            master_pan_project(MasterSource::None),
        );
        render_transport_chunk(
            &mut dsp,
            &status,
            &TransportMailbox::default(),
            &mut transport,
            32,
            |_, _| {},
        );
        dsp.pause_timeline();
        status.playing.store(false, Ordering::Release);
        assert!(!dsp.prepare_timeline_render(2, 32, transport.beat_q32, 64, false));
        let (mut retired, _reclaimed) = test_reclaimer();
        dsp.handle(AudioCommand::SetMasterPan(-1.0), &mut retired);
        dsp.handle(
            AudioCommand::NoteOn {
                note: 69,
                velocity: 1.0,
                mixer_track: 0,
            },
            &mut retired,
        );
        dsp.render_block(&status, 64);
        assert!(
            dsp.master_block[..64]
                .iter()
                .any(|frame| frame[0].abs() > 0.01)
        );
        assert!(dsp.master_block[..64].iter().all(|frame| frame[1] == 0.0));
    }
}

#[test]
fn master_pan_pdc_rejected_activation_and_capacity_preserve_prior_final_pcm() {
    for capacity_rejection in [false, true] {
        let dual_fixture = || {
            let mut project = master_pan_pdc_project();
            project.automation_lanes[0]
                .lane
                .set_curve(AutomationCurve::Linear);
            project.automation_lanes[0].lane.replace_points([
                AutomationPoint::new(0.0, 0.8),
                AutomationPoint::new(frame_as_beat_f64(MASTER_PDC_PROJECT_FRAMES as u64), 0.2),
            ]);
            MasterPdcFixture::with_project_and_capacities(project, 128, 8_192, 1_200)
        };
        let mut candidate = dual_fixture();
        let mut reference = dual_fixture();
        assert_master_pcm_close(
            &candidate.render_frames(384, &[128]),
            &reference.render_frames(384, &[128]),
            "before pan candidate",
        );
        let transport = candidate.transport;
        let pan_identity = candidate.dsp.timeline_master_pan.identity();
        let pan_cursor = candidate.dsp.timeline_master_pan.next_source_frame();
        let volume_identity = candidate.dsp.timeline_master_volume.identity();
        let volume_cursor = candidate.dsp.timeline_master_volume.next_source_frame();
        let current_timing = candidate.timing;
        if capacity_rejection {
            candidate.timing = PreparedPluginTimingPlan::new(3, 48_000, 512).unwrap();
            candidate.dsp.requested_plugin_timing_revision = candidate.timing.revision;
            candidate
                .status
                .plugin_processing
                .requested_revision
                .store(candidate.timing.revision, Ordering::Release);
            candidate
                .status
                .plugin_processing
                .requested_budget
                .store(512, Ordering::Release);
        }
        while candidate.controller.poll_event().is_some() {}
        candidate.queue_activation(3, 0);
        while candidate.controller.poll_event().is_some() {}
        if !capacity_rejection {
            let original = candidate.controls[1].plugin_latency_snapshot().unwrap();
            let changed = PluginLatencySnapshot {
                revision: next_nonzero_id(original.revision),
                ..original
            };
            candidate.dsp.insert_endpoints[0]
                .as_mut()
                .unwrap()
                .endpoint
                .script_fresh_endpoint_snapshots([Some(changed), Some(original)]);
        }
        candidate
            .transport
            .apply_pending_timeline_activation(&candidate.status, &mut candidate.dsp);
        if !capacity_rejection {
            assert!(
                (1..=2).contains(
                    &candidate.dsp.insert_endpoints[0]
                        .as_ref()
                        .unwrap()
                        .endpoint
                        .fresh_snapshot_script_cursor
                ),
                "rejection fixture must consume the scripted endpoint drift"
            );
        }
        assert!(matches!(
            candidate.controller.poll_event(),
            Some(TimelineRuntimeEvent::TransportActivationRejected {
                revision: 990,
                reason: TimelineTransportActivationRejectReason::GraphPdcPlan,
                ..
            })
        ));
        assert_eq!(candidate.transport, transport);
        assert_eq!(candidate.dsp.timeline_master_pan.identity(), pan_identity);
        assert_eq!(
            candidate.dsp.timeline_master_pan.next_source_frame(),
            pan_cursor
        );
        assert_eq!(
            candidate.dsp.timeline_master_volume.identity(),
            volume_identity
        );
        assert_eq!(
            candidate.dsp.timeline_master_volume.next_source_frame(),
            volume_cursor
        );
        assert_eq!(candidate.dsp.plugin_timing, current_timing);
        candidate.timing = current_timing;
        candidate.dsp.requested_plugin_timing_revision = current_timing.revision;
        candidate
            .status
            .plugin_processing
            .requested_revision
            .store(current_timing.revision, Ordering::Release);
        candidate
            .status
            .plugin_processing
            .requested_budget
            .store(128, Ordering::Release);
        let total = candidate.delay() as usize + MASTER_FADE_FRAMES;
        assert_master_pcm_close(
            &candidate.render_frames(total, &[31, 127, 128]),
            &reference.render_frames(total, &[31, 127, 128]),
            "rejected binding must preserve both changing Volume/Pan histories and final PCM",
        );
    }
}

fn master_mute_project(source: MasterSource, automated: bool) -> Project {
    let mut project = master_fade_project(source, false);
    project.mixer_tracks[0].muted = true;
    if automated {
        push_timeline_automation(
            &mut project,
            906,
            AutomationTarget::MixerMute {
                track: crate::mixer_graph::MASTER_MIXER_TRACK_ID,
            },
            AutomationCurve::Hold,
            [
                AutomationPoint::new(0.0, 1.0),
                AutomationPoint::new(frame_as_beat_f64(64), 0.0),
                AutomationPoint::new(frame_as_beat_f64(448), 1.0),
                AutomationPoint::new(frame_as_beat_f64(512), 1.0),
            ],
        );
    }
    project
}

fn render_master_mute_pcm(
    path: MasterRenderPath,
    automated: bool,
    partitions: &[usize],
    cadence: MasterUiCadence,
    source_muted: bool,
    source_excluded_by_solo: bool,
    master_solo: bool,
) -> Vec<[f32; 2]> {
    let (_controller, mut dsp, status, mut transport) = master_callback_fixture_for_project(
        path,
        MasterSource::Asset,
        false,
        master_mute_project(MasterSource::Asset, automated),
    );
    let (mut retired, _reclaimed) = test_reclaimer();
    // The generic fixtures do not synchronize persisted mixer mute into DSP.
    // Exercise the actual manual=true case through its production command.
    dsp.handle(
        AudioCommand::SetTrackMuted {
            track: 0,
            muted: true,
        },
        &mut retired,
    );
    dsp.handle(
        AudioCommand::SetTrackMuted {
            track: 1,
            muted: source_muted,
        },
        &mut retired,
    );
    dsp.handle(
        AudioCommand::SetTrackSolo {
            track: 2,
            solo: source_excluded_by_solo,
        },
        &mut retired,
    );
    dsp.handle(
        AudioCommand::SetTrackSolo {
            track: 0,
            solo: master_solo,
        },
        &mut retired,
    );
    assert!(dsp.track_muted[0]);
    if automated {
        assert_eq!(
            dsp.timeline_runtime
                .as_ref()
                .unwrap()
                .active_timeline()
                .unwrap()
                .automation_bases()
                .iter()
                .find(|base| base.target == (CompiledAutomationTarget::MixerMute { track: 0 }))
                .unwrap()
                .value,
            1.0,
        );
    }
    let (mut commands, mut reader) = RingBuffer::new(16);
    let (mut events, _event_reader) = RingBuffer::new(8);
    let mut pcm = Vec::with_capacity(MASTER_FADE_FRAMES);
    let mut callback = 0;
    while pcm.len() < MASTER_FADE_FRAMES {
        if matches!(cadence, MasterUiCadence::Dense)
            || (matches!(cadence, MasterUiCadence::Sparse) && callback % 13 == 0)
        {
            commands
                .push(AudioCommand::SetTrackMuted {
                    track: 0,
                    muted: callback % 2 == 0,
                })
                .unwrap();
        }
        process_commands(&mut dsp, &mut reader, &mut retired, &mut events);
        let frames = partitions[callback % partitions.len()].min(MASTER_FADE_FRAMES - pcm.len());
        render_transport_chunk(
            &mut dsp,
            &status,
            &TransportMailbox::default(),
            &mut transport,
            frames,
            |_, block| pcm.extend_from_slice(block),
        );
        callback += 1;
    }
    assert_eq!(dsp.timeline_execution_failures, 0);
    assert_eq!(dsp.timeline_automation_pending, 0);
    pcm
}

#[test]
fn master_mute_true_base_can_unmute_actual_manual_muted_graph_and_legacy_pcm() {
    let expected: Vec<[f32; 2]> = (0..MASTER_FADE_FRAMES)
        .map(|frame| {
            if (64..448).contains(&frame) {
                MASTER_TEST_SOURCE.map(f32::tanh)
            } else {
                [0.0; 2]
            }
        })
        .collect();
    for path in [MasterRenderPath::Legacy, MasterRenderPath::MixerGraph] {
        let actual = render_master_mute_pcm(
            path,
            true,
            &[512],
            MasterUiCadence::None,
            false,
            false,
            false,
        );
        assert_master_pcm_close(
            &actual,
            &expected,
            "manual=true must allow sample-owned unmute",
        );
        assert!(actual[64][0] != 0.0);
        let manual = render_master_mute_pcm(
            path,
            false,
            &[31, 127],
            MasterUiCadence::None,
            false,
            false,
            false,
        );
        assert!(manual.iter().all(|frame| *frame == [0.0; 2]));
        for (source_muted, source_excluded, master_solo) in [
            (true, false, false),
            (false, true, false),
            (false, false, true),
        ] {
            let actual = render_master_mute_pcm(
                path,
                true,
                &[17, 127, 31],
                MasterUiCadence::None,
                source_muted,
                source_excluded,
                master_solo,
            );
            if master_solo {
                assert_master_pcm_close(
                    &actual,
                    &expected,
                    "master solo must not exclude all inserts",
                );
            } else {
                assert!(actual.iter().all(|frame| *frame == [0.0; 2]));
            }
        }
        for cadence in [
            MasterUiCadence::None,
            MasterUiCadence::Sparse,
            MasterUiCadence::Dense,
        ] {
            for partitions in [&[1, 31, 127][..], &[17, 257, 63][..]] {
                assert_master_pcm_close(
                    &render_master_mute_pcm(path, true, partitions, cadence, false, false, false),
                    &expected,
                    "mute PCM must be independent of UI commands and block partition",
                );
            }
        }
    }
}

fn master_mute_pdc_project() -> Project {
    let mut project = master_pan_pdc_project();
    project.mixer_tracks[0].muted = true;
    project.automation_lanes[0]
        .lane
        .set_curve(AutomationCurve::Linear);
    project.automation_lanes[0].lane.replace_points([
        AutomationPoint::new(0.0, 0.8),
        AutomationPoint::new(frame_as_beat_f64(MASTER_PDC_PROJECT_FRAMES as u64), 0.2),
    ]);
    push_timeline_automation(
        &mut project,
        907,
        AutomationTarget::MixerMute {
            track: crate::mixer_graph::MASTER_MIXER_TRACK_ID,
        },
        AutomationCurve::Hold,
        [
            AutomationPoint::new(0.0, 1.0),
            AutomationPoint::new(frame_as_beat_f64(64), 0.499),
            AutomationPoint::new(frame_as_beat_f64(192), 0.5),
            AutomationPoint::new(frame_as_beat_f64(256), 0.501),
            AutomationPoint::new(frame_as_beat_f64(448), 0.0),
            AutomationPoint::new(frame_as_beat_f64(512), 1.0),
            AutomationPoint::new(frame_as_beat_f64(640), 0.0),
            AutomationPoint::new(frame_as_beat_f64(896), 1.0),
            AutomationPoint::new(frame_as_beat_f64(MASTER_PDC_PROJECT_FRAMES as u64), 1.0),
        ],
    );
    project
}

fn mute_pdc_fixture(budget: u32, mute_capacity: u32) -> MasterPdcFixture {
    MasterPdcFixture::with_project_and_three_capacities(
        master_mute_pdc_project(),
        budget,
        8_192,
        8_192,
        mute_capacity,
    )
}

fn linear_mute_pdc_fixture(budget: u32) -> MasterPdcFixture {
    let mut project = master_pan_pdc_project();
    project.mixer_tracks[0].muted = true;
    project.automation_lanes[1]
        .lane
        .set_curve(AutomationCurve::Hold);
    project.automation_lanes[1]
        .lane
        .replace_points([AutomationPoint::new(0.0, 0.0)]);
    push_timeline_automation(
        &mut project,
        908,
        AutomationTarget::MixerMute {
            track: crate::mixer_graph::MASTER_MIXER_TRACK_ID,
        },
        AutomationCurve::Linear,
        [
            AutomationPoint::new(0.0, 0.0),
            AutomationPoint::new(frame_as_beat_f64(4_000), 1.0),
        ],
    );
    MasterPdcFixture::with_project_and_three_capacities(project, budget, 8_192, 8_192, 8_192)
}

fn expected_linear_mute_pdc_pcm(frames: usize, delay: usize, seek: usize) -> Vec<[f32; 2]> {
    (0..frames)
        .map(|frame| {
            if frame < delay || seek + frame - delay >= 2_000 {
                [0.0; 2]
            } else {
                [MASTER_PDC_SOURCE.tanh(); 2]
            }
        })
        .collect()
}

#[test]
fn master_mute_linear_nonzero_pdc_crosses_threshold_at_frame_2000_for_every_partition() {
    for budget in [128, 512] {
        for partitions in [&[128][..], &[1, 31, 64, 127, 128][..], &[17, 63, 97][..]] {
            let mut fixture = linear_mute_pdc_fixture(budget);
            assert!(fixture.dsp.track_muted[0]);
            let delay = fixture.delay() as usize;
            let total = delay + 2_200;
            let actual = fixture.render_frames(total, partitions);
            assert_master_pcm_close(
                &actual,
                &expected_linear_mute_pdc_pcm(total, delay, 0),
                "Linear Mute must interpolate raw control and threshold once at source frame 2000",
            );
            assert_ne!(actual[delay + 1_999], [0.0; 2]);
            assert_eq!(actual[delay + 2_000], [0.0; 2]);
            assert_eq!(fixture.dsp.timeline_automation_pending, 0);
        }
    }
}

#[test]
fn master_mute_linear_nonzero_pdc_seek_retains_raw_chase_and_exact_threshold_pcm() {
    let mut warm = linear_mute_pdc_fixture(128);
    let delay = warm.delay() as usize;
    warm.render_frames(delay + 4_096, &[128]);
    for (epoch, seek) in [(3, 1_500), (4, 1_999), (5, 2_000)] {
        warm.activate(epoch, seek);
        let mut fresh = linear_mute_pdc_fixture(128);
        fresh.activate(epoch, seek);
        let total = delay + 768;
        let expected = expected_linear_mute_pdc_pcm(total, delay, seek as usize);
        let actual = warm.render_frames(total, &[31, 127, 128]);
        let reference = fresh.render_frames(total, &[17, 63, 97]);
        assert_master_pcm_close(
            &reference,
            &expected,
            "fresh Linear Mute seek and output delay",
        );
        assert_master_pcm_close(
            &actual,
            &reference,
            "warm Linear Mute history must reseed from raw chase",
        );
        if seek < 2_000 {
            let edge = delay + 2_000 - seek as usize;
            assert_ne!(actual[edge - 1], [0.0; 2]);
            assert_eq!(actual[edge], [0.0; 2]);
        } else {
            assert!(actual.iter().all(|frame| *frame == [0.0; 2]));
        }
    }
}

fn master_expected_mute(frame: usize) -> bool {
    !((64..192).contains(&frame) || (448..512).contains(&frame) || (640..896).contains(&frame))
}

#[test]
fn master_mute_compiled_hold_edges_preserve_authored_threshold_values() {
    let timeline = compile_timeline_test_project(&master_mute_pdc_project());
    for (frame, expected) in [(64, 0.499_f32), (192, 0.5_f32), (256, 0.501_f32)] {
        let ramp = timeline
            .events()
            .iter()
            .find_map(|event| match event.kind {
                TimelineEventKind::AutomationRamp(ramp)
                    if event.frame == frame
                        && ramp.target == (CompiledAutomationTarget::MixerMute { track: 0 }) =>
                {
                    Some(ramp)
                }
                _ => None,
            })
            .expect("each authored hold point emits its exact frame transition");
        assert_eq!(ramp.shape, AutomationRampShape::Hold);
        assert_eq!(
            ramp.start_value, expected,
            "compiled hold edge at frame {frame}"
        );
        assert_eq!(ramp.end_value, expected, "hold left limit at frame {frame}");
    }
}

fn expected_mute_pdc_pcm(frames: usize, delay: usize, seek: usize) -> Vec<[f32; 2]> {
    (0..frames)
        .map(|frame| {
            if frame < delay || master_expected_mute(seek + frame.saturating_sub(delay)) {
                [0.0; 2]
            } else {
                let source_frame = seek + frame - delay;
                let gain =
                    (0.8 - 0.6 * source_frame as f64 / MASTER_PDC_PROJECT_FRAMES as f64) as f32;
                let pan = master_expected_pan(source_frame);
                [
                    (MASTER_PDC_SOURCE * gain * (1.0 - pan.max(0.0))).tanh(),
                    (MASTER_PDC_SOURCE * gain * (1.0 + pan.min(0.0))).tanh(),
                ]
            }
        })
        .collect()
}

#[test]
fn master_mute_threshold_and_nonzero_pdc_edges_are_exact_in_final_pcm() {
    for budget in [128, 512] {
        let mut reference = mute_pdc_fixture(budget, 8_192);
        let delay = reference.delay() as usize;
        let total = delay + 1_024;
        let expected = expected_mute_pdc_pcm(total, delay, 0);
        let actual = reference.render_frames(total, &[128]);
        assert_master_pcm_close(&actual, &expected, "mute threshold at T plus output delay");
        assert!(actual[delay + 63] == [0.0; 2] && actual[delay + 64][0] != 0.0);
        assert!(actual[delay + 191].iter().any(|sample| *sample != 0.0));
        assert_eq!(actual[delay + 192], [0.0; 2]);
        assert_eq!(actual[delay + 256], [0.0; 2]);
        for cadence in [
            MasterUiCadence::None,
            MasterUiCadence::Sparse,
            MasterUiCadence::Dense,
        ] {
            let mut fixture = mute_pdc_fixture(budget, 8_192);
            let (mut commands, mut reader) = RingBuffer::new(16);
            let (mut retired, _reclaimed) = test_reclaimer();
            let (mut events, _event_reader) = RingBuffer::new(8);
            let mut actual = Vec::with_capacity(total);
            let mut callback = 0;
            while actual.len() < total {
                if matches!(cadence, MasterUiCadence::Dense)
                    || (matches!(cadence, MasterUiCadence::Sparse) && callback % 13 == 0)
                {
                    commands
                        .push(AudioCommand::SetTrackMuted {
                            track: 0,
                            muted: callback % 2 == 0,
                        })
                        .unwrap();
                }
                process_commands(&mut fixture.dsp, &mut reader, &mut retired, &mut events);
                actual.extend(
                    fixture.render([1, 31, 64, 127, 128][callback % 5].min(total - actual.len())),
                );
                callback += 1;
            }
            assert_master_pcm_close(
                &actual,
                &expected,
                "three delayed controls must ignore UI mute cadence and partition",
            );
            assert_eq!(fixture.dsp.timeline_automation_pending, 0);
        }
    }
}

#[test]
fn master_mute_seek_and_real_installed_loop_discard_old_history_in_final_pcm() {
    let mut warm = mute_pdc_fixture(128, 8_192);
    let delay = warm.delay() as usize;
    warm.render_frames(delay + 1_024, &[128]);
    warm.activate(3, 128);
    let mut fresh = mute_pdc_fixture(128, 8_192);
    fresh.activate(3, 128);
    let total = delay + 384;
    let reference = fresh.render_frames(total, &[31, 127, 128]);
    assert_master_pcm_close(
        &reference,
        &expected_mute_pdc_pcm(total, delay, 128),
        "fresh mute seek",
    );
    assert_master_pcm_close(
        &warm.render_frames(total, &[31, 127, 128]),
        &reference,
        "mute seek must discard old muted history",
    );
    assert!(reference[delay].iter().any(|sample| *sample != 0.0));
    warm.dsp.apply_transport_discontinuity(
        &warm.status,
        4,
        0,
        0,
        TransportDiscontinuity::Loop,
        true,
    );
    assert_eq!(warm.dsp.timeline_channel_epoch, Some(4));
    warm.transport.epoch = 4;
    warm.transport.timeline_frame = 0;
    warm.transport.beat_q32 = 0;
    warm.transport.request.target_timeline_frame = 0;
    warm.transport.request.target_beat_q32 = 0;
    warm.transport.publish(&warm.status);
    warm.wait_workers();
    let actual = warm.render_frames(delay + 384, &[31, 127, 128]);
    assert_master_pcm_close(
        &actual,
        &expected_mute_pdc_pcm(delay + 384, delay, 0),
        "installed loop chase must reseed mute and all output controls",
    );
    assert!(actual[delay + 64].iter().any(|sample| *sample != 0.0));
}

#[test]
fn master_mute_pause_freezes_history_manual_preview_and_resume_preserve_pcm() {
    let mut paused = mute_pdc_fixture(128, 8_192);
    let mut reference = mute_pdc_fixture(128, 8_192);
    assert_master_pcm_close(
        &paused.render_frames(384, &[128]),
        &reference.render_frames(384, &[128]),
        "before mute pause",
    );
    paused.mailbox.publish(TransportMutation::SetPlaying(false));
    paused
        .transport
        .apply_latest_request(&paused.mailbox, &paused.status, &mut paused.dsp);
    let cursors = [
        paused.dsp.timeline_master_volume.next_source_frame(),
        paused.dsp.timeline_master_pan.next_source_frame(),
        paused.dsp.timeline_master_mute.next_source_frame(),
    ];
    assert!(
        paused
            .render_frames(768, &[31, 127, 128])
            .iter()
            .all(|frame| *frame == [0.0; 2])
    );
    assert_eq!(
        [
            paused.dsp.timeline_master_volume.next_source_frame(),
            paused.dsp.timeline_master_pan.next_source_frame(),
            paused.dsp.timeline_master_mute.next_source_frame()
        ],
        cursors
    );
    paused.mailbox.publish(TransportMutation::SetPlaying(true));
    paused
        .transport
        .apply_latest_request(&paused.mailbox, &paused.status, &mut paused.dsp);
    let total = paused.delay() as usize + 512;
    let expected = reference.render_frames(total, &[31, 127, 128]);
    assert!(expected.iter().any(|frame| *frame != [0.0; 2]));
    assert_master_pcm_close(
        &paused.render_frames(total, &[31, 127, 128]),
        &expected,
        "paused device frames must not advance any control history",
    );
    for path in [MasterRenderPath::Legacy, MasterRenderPath::MixerGraph] {
        for manual_muted in [false, true] {
            let (_controller, mut dsp, status, mut transport) = master_callback_fixture_for_project(
                path,
                MasterSource::None,
                false,
                master_mute_project(MasterSource::None, true),
            );
            render_transport_chunk(
                &mut dsp,
                &status,
                &TransportMailbox::default(),
                &mut transport,
                32,
                |_, _| {},
            );
            dsp.pause_timeline();
            status.playing.store(false, Ordering::Release);
            let next = dsp.timeline_master_mute.next_source_frame();
            assert!(!dsp.prepare_timeline_render(2, 32, transport.beat_q32, 64, false));
            let (mut retired, _reclaimed) = test_reclaimer();
            dsp.handle(
                AudioCommand::SetTrackMuted {
                    track: 0,
                    muted: manual_muted,
                },
                &mut retired,
            );
            dsp.handle(
                AudioCommand::NoteOn {
                    note: 69,
                    velocity: 1.0,
                    mixer_track: 0,
                },
                &mut retired,
            );
            dsp.render_block(&status, 64);
            assert_eq!(dsp.timeline_master_mute.next_source_frame(), next);
            if manual_muted {
                assert!(
                    dsp.master_block[..64]
                        .iter()
                        .all(|frame| *frame == [0.0; 2])
                );
            } else {
                assert!(
                    dsp.master_block[..64]
                        .iter()
                        .any(|frame| frame[0].abs() > 0.01)
                );
            }
        }
    }
}

#[test]
fn master_mute_pdc_rejected_activation_and_capacity_preserve_all_three_histories_and_pcm() {
    for capacity_rejection in [false, true] {
        let mut candidate = mute_pdc_fixture(128, 1_200);
        let mut reference = mute_pdc_fixture(128, 1_200);
        assert_master_pcm_close(
            &candidate.render_frames(384, &[128]),
            &reference.render_frames(384, &[128]),
            "before mute candidate",
        );
        let transport_before = candidate.transport;
        let volume_identity = candidate.dsp.timeline_master_volume.identity();
        let volume_cursor = candidate.dsp.timeline_master_volume.next_source_frame();
        let pan_identity = candidate.dsp.timeline_master_pan.identity();
        let pan_cursor = candidate.dsp.timeline_master_pan.next_source_frame();
        let mute_identity = candidate.dsp.timeline_master_mute.identity();
        let mute_cursor = candidate.dsp.timeline_master_mute.next_source_frame();
        let pdc_revision_before = candidate.dsp.pdc_plan_revision;
        let executor_frame_before = candidate.dsp.timeline_executor.next_frame();
        let active_epoch_before = candidate
            .dsp
            .timeline_runtime
            .as_ref()
            .unwrap()
            .active_epoch();
        let current_timing = candidate.dsp.plugin_timing;
        assert_eq!(active_epoch_before, Some(2));
        assert_eq!(current_timing, candidate.timing);
        if capacity_rejection {
            candidate.timing = PreparedPluginTimingPlan::new(3, 48_000, 512).unwrap();
            candidate.dsp.requested_plugin_timing_revision = candidate.timing.revision;
            candidate
                .status
                .plugin_processing
                .requested_revision
                .store(candidate.timing.revision, Ordering::Release);
            candidate
                .status
                .plugin_processing
                .requested_budget
                .store(512, Ordering::Release);
        }
        while candidate.controller.poll_event().is_some() {}
        candidate.queue_activation(3, 0);
        while candidate.controller.poll_event().is_some() {}
        if !capacity_rejection {
            let original = candidate.controls[1].plugin_latency_snapshot().unwrap();
            let changed = PluginLatencySnapshot {
                revision: next_nonzero_id(original.revision),
                ..original
            };
            candidate.dsp.insert_endpoints[0]
                .as_mut()
                .unwrap()
                .endpoint
                .script_fresh_endpoint_snapshots([Some(changed), Some(original)]);
        }
        candidate
            .transport
            .apply_pending_timeline_activation(&candidate.status, &mut candidate.dsp);
        if !capacity_rejection {
            let fresh_reads = candidate.dsp.insert_endpoints[0]
                .as_ref()
                .unwrap()
                .endpoint
                .fresh_snapshot_script_cursor;
            assert!(
                (1..=2).contains(&fresh_reads),
                "mute rejection fixture must consume the scripted endpoint drift"
            );
        }
        assert!(
            matches!(
                candidate.controller.poll_event(),
                Some(TimelineRuntimeEvent::TransportActivationRejected {
                    revision: 990,
                    reason: TimelineTransportActivationRejectReason::GraphPdcPlan,
                    ..
                })
            ),
            "mute candidate must produce an actual rejection receipt"
        );
        assert_eq!(candidate.transport, transport_before);
        assert_eq!(
            candidate.dsp.timeline_master_volume.identity(),
            volume_identity
        );
        assert_eq!(
            candidate.dsp.timeline_master_volume.next_source_frame(),
            volume_cursor
        );
        assert_eq!(candidate.dsp.timeline_master_pan.identity(), pan_identity);
        assert_eq!(
            candidate.dsp.timeline_master_pan.next_source_frame(),
            pan_cursor
        );
        assert_eq!(candidate.dsp.timeline_master_mute.identity(), mute_identity);
        assert_eq!(
            candidate.dsp.timeline_master_mute.next_source_frame(),
            mute_cursor
        );
        assert_eq!(candidate.dsp.pdc_plan_revision, pdc_revision_before);
        assert_eq!(
            candidate.dsp.timeline_executor.next_frame(),
            executor_frame_before
        );
        assert_eq!(
            candidate
                .dsp
                .timeline_runtime
                .as_ref()
                .unwrap()
                .active_epoch(),
            active_epoch_before
        );
        assert!(
            candidate
                .dsp
                .pending_timeline_transport_activation()
                .is_none()
        );
        assert_eq!(candidate.dsp.plugin_timing, current_timing);
        candidate.timing = current_timing;
        candidate.dsp.requested_plugin_timing_revision = current_timing.revision;
        candidate
            .status
            .plugin_processing
            .requested_revision
            .store(current_timing.revision, Ordering::Release);
        candidate
            .status
            .plugin_processing
            .requested_budget
            .store(128, Ordering::Release);
        // This budget has a longer physical delay than the warmup. Verify an
        // authored mute edge after audible output, as well as the startup zeros.
        let total = candidate.delay() as usize + 512;
        let actual = candidate.render_frames(total, &[31, 127, 128]);
        let expected = reference.render_frames(total, &[31, 127, 128]);
        let mute_edge = candidate.delay() as usize + 192 - 384;
        assert!(actual[mute_edge - 1].iter().any(|sample| *sample != 0.0));
        assert_eq!(actual[mute_edge], [0.0; 2]);
        for pcm in [&actual, &expected] {
            assert!(
                pcm.iter()
                    .any(|frame| frame[0].abs() > 0.01 || frame[1].abs() > 0.01),
                "preserved output must include audible unmuted PCM"
            );
            assert!(
                pcm.contains(&[0.0; 2]),
                "preserved output must include exactly silent muted PCM"
            );
        }
        assert_master_pcm_close(
            &actual,
            &expected,
            "rejected binding must preserve changing Volume/Pan/Mute histories and final PCM",
        );
    }
}
