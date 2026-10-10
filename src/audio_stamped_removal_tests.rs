use super::*;
use crate::plugins::plugin_runtime::{BridgeStats, RetainedShutdownPoll};
use std::sync::{Mutex, MutexGuard};

const MOCK_DRAIN_BUDGET: Duration = Duration::from_secs(5);
static MOCK_FIXTURE_SERIAL: Mutex<()> = Mutex::new(());
static MOCK_FIXTURE_BLOCKED: AtomicBool = AtomicBool::new(false);
static RETAINED_MOCK_FAILURES: Mutex<Vec<RetainedMockFailure>> = Mutex::new(Vec::new());

/// Never destroyed on timeout: the exact guard/control and last diagnostics
/// remain here, and all later admissions in this slice fail before spawning.
struct RetainedMockFailure {
    _guard: PluginWorkerGuard,
    _control: PluginChainControl,
    _stats: BridgeStats,
}

struct RetainedMockWorker {
    audio: Option<AudioThreadEndpoint>,
    control: Option<PluginChainControl>,
    guard: Option<PluginWorkerGuard>,
    joined: Arc<AtomicU32>,
}

impl RetainedMockWorker {
    fn instrument(instance_id: u64, amplitude: f32) -> Self {
        assert!(!MOCK_FIXTURE_BLOCKED.load(Ordering::Acquire));
        let joined = Arc::new(AtomicU32::new(0));
        let chain = PluginChain::spawn_identified_with_backend_factory(
            &[instance_id],
            move || {
                vec![BackendSlot::new(Box::new(MockInstrumentBackend {
                    active: false,
                    amplitude,
                    process_delay: Duration::ZERO,
                    last_midi: Arc::new(AtomicU32::new(0)),
                    parameter: Arc::new(AtomicU32::new(0)),
                }))]
            },
            PluginPrepareConfig {
                sample_rate: 48_000.0,
                max_block_frames: 128,
            },
        )
        .expect("identified mock spawn");
        // No assertion/wait between successful spawn and retained ownership.
        Self {
            audio: Some(chain.audio),
            control: Some(chain.control),
            guard: Some(chain.guard),
            joined,
        }
    }

    fn empty() -> Self {
        assert!(!MOCK_FIXTURE_BLOCKED.load(Ordering::Acquire));
        let joined = Arc::new(AtomicU32::new(0));
        let chain = PluginChain::spawn_with_backend_factory(
            Vec::<BackendSlot>::new,
            PluginPrepareConfig {
                sample_rate: 48_000.0,
                max_block_frames: 128,
            },
        )
        .expect("empty mock spawn");
        Self {
            audio: Some(chain.audio),
            control: Some(chain.control),
            guard: Some(chain.guard),
            joined,
        }
    }

    fn timing(instance_id: u64, latency: Arc<AtomicU32>, tail: Arc<AtomicU32>) -> Self {
        assert!(!MOCK_FIXTURE_BLOCKED.load(Ordering::Acquire));
        let joined = Arc::new(AtomicU32::new(0));
        let chain = PluginChain::spawn_identified_with_backend_factory(
            &[instance_id],
            move || {
                vec![BackendSlot::new(Box::new(InstallTimingBackend {
                    latency,
                    tail,
                }))]
            },
            PluginPrepareConfig {
                sample_rate: 48_000.0,
                max_block_frames: 128,
            },
        )
        .expect("timing mock spawn");
        Self {
            audio: Some(chain.audio),
            control: Some(chain.control),
            guard: Some(chain.guard),
            joined,
        }
    }

    fn control(&self) -> &PluginChainControl {
        self.control.as_ref().unwrap()
    }

    fn drain(&mut self) -> Option<RetainedShutdownPoll> {
        drop(self.audio.take());
        if self.guard.is_none() {
            return match self.joined.load(Ordering::Acquire) {
                1 => Some(RetainedShutdownPoll::Joined),
                2 => Some(RetainedShutdownPoll::WorkerPanicked),
                _ => None,
            };
        }
        let deadline = Instant::now() + MOCK_DRAIN_BUDGET;
        loop {
            let state = self.guard.as_mut().unwrap().poll_shutdown_retained();
            if state != RetainedShutdownPoll::Pending {
                let stats = self.control().stats();
                self.joined.store(
                    if state == RetainedShutdownPoll::Joined {
                        1
                    } else {
                        2
                    },
                    Ordering::Release,
                );
                // Only a terminal retained poll permits ordinary guard Drop.
                drop(self.guard.take());
                if stats.detached_workers != 0 || stats.shutdown_timeouts != 0 {
                    MOCK_FIXTURE_BLOCKED.store(true, Ordering::Release);
                    return None;
                }
                return Some(state);
            }
            if Instant::now() >= deadline {
                MOCK_FIXTURE_BLOCKED.store(true, Ordering::Release);
                let stats = self.control().stats();
                RETAINED_MOCK_FAILURES
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push(RetainedMockFailure {
                        _guard: self.guard.take().unwrap(),
                        _control: self.control.take().unwrap(),
                        _stats: stats,
                    });
                return None;
            }
            thread::sleep(Duration::from_millis(1));
        }
    }
}

impl Drop for RetainedMockWorker {
    fn drop(&mut self) {
        let result = self.drain();
        if !thread::panicking() {
            assert_eq!(
                result,
                Some(RetainedShutdownPoll::Joined),
                "mock worker retained after nonterminal cleanup or joined failure"
            );
        }
    }
}

/// Restores even on assertion/unwind, before fixture teardown may read the mailbox.
struct MailboxSequenceRestore {
    mailbox: Arc<TransportMailbox>,
    sequence: u64,
}
impl Drop for MailboxSequenceRestore {
    fn drop(&mut self) {
        self.mailbox
            .sequence
            .store(self.sequence, Ordering::Release);
    }
}

type RemovalObservation = (
    u64,
    bool,
    u64,
    Option<(u64, u64, u64, u64)>,
    [f32; 2],
    [f32; 2],
    f32,
);

struct RemovalFixture {
    callback: Option<TestAudioCallback>,
    engine: Option<AudioEngine>,
    workers: Vec<RetainedMockWorker>,
    _admission: MutexGuard<'static, ()>,
}

impl RemovalFixture {
    fn new() -> Self {
        let admission = MOCK_FIXTURE_SERIAL
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert!(
            !MOCK_FIXTURE_BLOCKED.load(Ordering::Acquire),
            "prior mock worker still retained; later admission blocked"
        );
        let (engine, callback) = AudioEngine::test_engine_with_callback();
        let mut fixture = Self {
            callback: Some(callback),
            engine: Some(engine),
            workers: Vec::with_capacity(4),
            _admission: admission,
        };
        assert!(fixture.engine.as_mut().unwrap().set_project_session(7));
        let worker = fixture.add_instrument(700, 0.75);
        let audio = fixture.workers[worker].audio.take().unwrap();
        assert!(
            fixture
                .engine
                .as_mut()
                .unwrap()
                .install_generator_endpoint_for_project(7, 9, 70, 700, 1, audio)
        );
        fixture.process_commands();
        assert!(matches!(
            fixture
                .engine
                .as_mut()
                .unwrap()
                .next_generator_endpoint_event(),
            Some(GeneratorEndpointEvent::Installed {
                success: true,
                endpoint_id: 70,
                ..
            })
        ));
        fixture
    }

    fn add_instrument(&mut self, instance_id: u64, amplitude: f32) -> usize {
        let index = self.workers.len();
        self.workers
            .push(RetainedMockWorker::instrument(instance_id, amplitude));
        wait_until(|| {
            self.workers[index]
                .control()
                .plugin_latency_snapshot()
                .is_some_and(|snapshot| snapshot.revision != 0 && snapshot.slot_is_active(0))
        });
        index
    }

    fn add_empty(&mut self) -> usize {
        let index = self.workers.len();
        self.workers.push(RetainedMockWorker::empty());
        index
    }

    fn add_timing(
        &mut self,
        instance_id: u64,
        latency: Arc<AtomicU32>,
        tail: Arc<AtomicU32>,
    ) -> usize {
        let index = self.workers.len();
        self.workers
            .push(RetainedMockWorker::timing(instance_id, latency, tail));
        wait_until(|| {
            self.workers[index]
                .control()
                .plugin_latency_snapshot()
                .is_some_and(|snapshot| snapshot.revision != 0 && snapshot.slot_is_active(0))
        });
        index
    }

    fn stamp(&self) -> GeneratorRemovalStamp {
        GeneratorRemovalStamp {
            operation_id: 101,
            engine_session: self.engine.as_ref().unwrap().engine_session(),
            project_session: 7,
            channel_id: 9,
            endpoint_id: 70,
            plugin_instance_id: 700,
            stopped_epoch: self.callback.as_ref().unwrap().dsp.transport_epoch,
        }
    }

    fn process_commands(&mut self) {
        let callback = self.callback.as_mut().unwrap();
        process_commands(
            &mut callback.dsp,
            &mut callback.commands,
            &mut callback.retired_assets,
            &mut callback.asset_events,
        );
    }

    fn observe(&self) -> RemovalObservation {
        let dsp = &self.callback.as_ref().unwrap().dsp;
        (
            dsp.plugin_topology_revision,
            dsp.plugin_topology_replan_pending,
            dsp.timeline_execution_failures,
            dsp.generator_endpoints[0].as_ref().map(|slot| {
                (
                    slot.endpoint_id,
                    slot.plugin_instance_id,
                    slot.endpoint.project_session,
                    slot.endpoint.epoch(),
                )
            }),
            dsp.track_block[0],
            dsp.master_block[0],
            dsp.plugin_output_left[0],
        )
    }

    fn assert_rejected(
        &mut self,
        stamp: GeneratorRemovalStamp,
        reason: GeneratorRemovalRejectReason,
    ) {
        self.callback.as_mut().unwrap().dsp.track_block[0] = [0.125, -0.25];
        self.callback.as_mut().unwrap().dsp.master_block[0] = [0.375, -0.5];
        self.callback.as_mut().unwrap().dsp.plugin_output_left[0] = 0.625;
        let before = self.observe();
        assert!(
            self.engine
                .as_mut()
                .unwrap()
                .remove_generator_endpoint_stamped(stamp)
        );
        self.process_commands();
        assert_eq!(
            self.observe(),
            before,
            "rejected removal must not mutate DSP state"
        );
        assert_eq!(
            self.engine
                .as_mut()
                .unwrap()
                .next_generator_endpoint_event(),
            Some(GeneratorEndpointEvent::StampedRemoved {
                stamp,
                result: Err(reason)
            })
        );
        assert!(
            self.engine
                .as_mut()
                .unwrap()
                .next_generator_endpoint_event()
                .is_none()
        );
        assert!(
            self.engine
                .as_mut()
                .unwrap()
                .retired_insert_endpoints
                .pop()
                .is_err()
        );
    }

    fn finish(self) {
        drop(self);
    }
}

impl Drop for RemovalFixture {
    fn drop(&mut self) {
        // Release every command/endpoint/retirement ring before attempting worker joins.
        drop(self.callback.take());
        drop(self.engine.take());
        // Explicitly drain while the serial admission lease is still held.
        for worker in &mut self.workers {
            let _ = worker.drain();
        }
    }
}

#[test]
fn stamped_removal_engine_binding_is_actual_unique_and_project_zero_refuses() {
    let (mut first, first_callback) = AudioEngine::test_engine_with_callback();
    let (second, second_callback) = AudioEngine::test_engine_with_callback();
    assert_ne!(first.engine_session(), 0);
    assert_ne!(first.engine_session(), second.engine_session());
    assert_eq!(first_callback.dsp.engine_session, first.engine_session());
    assert_eq!(first.project_session(), 0);
    assert!(!first.set_project_session(0));
    assert!(first.set_project_session(7));
    assert_eq!(
        first_callback.dsp.project_session.load(Ordering::Acquire),
        7
    );
    assert!(!first.set_project_session(0));
    assert_eq!(first.project_session(), 7);
    drop((first_callback, second_callback, first, second));
}

#[test]
fn stamped_removal_rejects_every_zero_or_stale_identity_without_mutation() {
    use GeneratorRemovalRejectReason as Reject;
    let mut fixture = RemovalFixture::new();
    let valid = fixture.stamp();
    for field in 0..6 {
        let mut zero = valid;
        match field {
            0 => zero.operation_id = 0,
            1 => zero.engine_session = 0,
            2 => zero.project_session = 0,
            3 => zero.endpoint_id = 0,
            4 => zero.plugin_instance_id = 0,
            _ => zero.stopped_epoch = 0,
        }
        fixture.assert_rejected(zero, Reject::InvalidStamp);
    }
    for (changed, reason) in [
        (
            GeneratorRemovalStamp {
                engine_session: valid.engine_session + 1,
                ..valid
            },
            Reject::EngineSessionChanged,
        ),
        (
            GeneratorRemovalStamp {
                project_session: 8,
                ..valid
            },
            Reject::ProjectSessionChanged,
        ),
        (
            GeneratorRemovalStamp {
                channel_id: 10,
                ..valid
            },
            Reject::MissingEndpoint,
        ),
        (
            GeneratorRemovalStamp {
                endpoint_id: 71,
                ..valid
            },
            Reject::EndpointChanged,
        ),
        (
            GeneratorRemovalStamp {
                plugin_instance_id: 701,
                ..valid
            },
            Reject::PluginInstanceChanged,
        ),
        (
            GeneratorRemovalStamp {
                stopped_epoch: valid.stopped_epoch + 1,
                ..valid
            },
            Reject::StoppedEpochChanged,
        ),
    ] {
        fixture.assert_rejected(changed, reason);
    }
    fixture.finish();
}

#[test]
fn stamped_removal_rechecks_current_project_binding_after_enqueue() {
    let mut fixture = RemovalFixture::new();
    let stamp = fixture.stamp();
    assert!(
        fixture
            .engine
            .as_mut()
            .unwrap()
            .remove_generator_endpoint_stamped(stamp)
    );
    assert!(fixture.engine.as_mut().unwrap().set_project_session(8));
    let before = fixture.observe();
    fixture.process_commands();
    assert_eq!(fixture.observe(), before);
    assert_eq!(
        fixture
            .engine
            .as_mut()
            .unwrap()
            .next_generator_endpoint_event(),
        Some(GeneratorEndpointEvent::StampedRemoved {
            stamp,
            result: Err(GeneratorRemovalRejectReason::ProjectSessionChanged),
        })
    );
    // Updating the command to the new project cannot launder an old endpoint.
    fixture.assert_rejected(
        GeneratorRemovalStamp {
            project_session: 8,
            ..stamp
        },
        GeneratorRemovalRejectReason::EndpointProjectChanged,
    );
    fixture.finish();
}

#[test]
fn stamped_removal_requires_observed_requested_and_endpoint_stopped_epoch() {
    use GeneratorRemovalRejectReason as Reject;
    let mut fixture = RemovalFixture::new();
    let stamp = fixture.stamp();
    fixture
        .callback
        .as_mut()
        .unwrap()
        .dsp
        .callback_transport_playing = true;
    fixture.assert_rejected(stamp, Reject::TransportRunning);
    fixture
        .callback
        .as_mut()
        .unwrap()
        .dsp
        .callback_transport_playing = false;
    fixture
        .callback
        .as_mut()
        .unwrap()
        .dsp
        .callback_transport_anchor
        .transport_epoch += 1;
    fixture.assert_rejected(stamp, Reject::StoppedEpochChanged);
    fixture
        .callback
        .as_mut()
        .unwrap()
        .dsp
        .callback_transport_anchor
        .transport_epoch = stamp.stopped_epoch;
    assert!(
        fixture.callback.as_mut().unwrap().dsp.generator_endpoints[0]
            .as_mut()
            .unwrap()
            .endpoint
            .set_epoch(stamp.stopped_epoch + 1)
    );
    fixture.assert_rejected(stamp, Reject::EndpointEpochChanged);
    fixture.finish();
}

#[test]
fn stamped_removal_rejects_queued_play_seek_and_unreadable_mailbox() {
    use GeneratorRemovalRejectReason as Reject;
    for change in 0..4 {
        let mut fixture = RemovalFixture::new();
        let stamp = fixture.stamp();
        let sequence_restore = MailboxSequenceRestore {
            mailbox: Arc::clone(&fixture.engine.as_ref().unwrap().transport_mailbox),
            sequence: fixture
                .engine
                .as_ref()
                .unwrap()
                .transport_mailbox
                .sequence
                .load(Ordering::Acquire),
        };
        let reason = match change {
            0 => {
                fixture.engine.as_mut().unwrap().set_playing(true);
                Reject::TransportRunning
            }
            1 => {
                fixture
                    .engine
                    .as_mut()
                    .unwrap()
                    .seek_transport_to_frame(1.0, 24_000);
                Reject::TransportRequestPending
            }
            2 => {
                fixture.engine.as_mut().unwrap().set_playing(false);
                Reject::TransportRequestPending
            }
            _ => {
                fixture
                    .engine
                    .as_mut()
                    .unwrap()
                    .transport_mailbox
                    .sequence
                    .store(1, Ordering::Release);
                Reject::TransportRequestPending
            }
        };
        fixture.assert_rejected(stamp, reason);
        drop(sequence_restore);
        fixture.finish();
    }
}

#[test]
fn stamped_removal_rejects_pending_timeline_transition_and_native_manifest_mismatch() {
    let mut fixture = RemovalFixture::new();
    let stamp = fixture.stamp();
    fixture
        .engine
        .as_mut()
        .unwrap()
        .install_compiled_timeline(2, compiled_test_timeline())
        .unwrap();
    fixture.assert_rejected(
        stamp,
        GeneratorRemovalRejectReason::TimelineTransitionPending,
    );
    fixture
        .callback
        .as_mut()
        .unwrap()
        .dsp
        .apply_pending_timeline_commands();
    fixture.callback.as_mut().unwrap().dsp.generator_endpoints[0]
        .as_mut()
        .unwrap()
        .plugin_instance_id = 701;
    fixture.assert_rejected(
        GeneratorRemovalStamp {
            plugin_instance_id: 701,
            ..stamp
        },
        GeneratorRemovalRejectReason::EndpointManifestChanged,
    );
    fixture.finish();
}

#[test]
fn stamped_removal_event_ring_full_keeps_command_and_all_state() {
    let mut fixture = RemovalFixture::new();
    let stamp = fixture.stamp();
    let (mut events, receiver) = RingBuffer::new(1);
    let blocker = GeneratorEndpointEvent::Cleared {
        request_id: 909,
        removed: 0,
    };
    events.push(blocker).unwrap();
    fixture
        .callback
        .as_mut()
        .unwrap()
        .dsp
        .generator_endpoint_events = Some(events);
    fixture.engine.as_mut().unwrap().generator_endpoint_events = receiver;
    assert!(
        fixture
            .engine
            .as_mut()
            .unwrap()
            .remove_generator_endpoint_stamped(stamp)
    );
    let before = fixture.observe();
    fixture.process_commands();
    assert_eq!(fixture.observe(), before);
    assert!(matches!(fixture.callback.as_mut().unwrap().commands.peek(),
        Ok(AudioCommand::RemoveGeneratorEndpointStamped(queued)) if *queued == stamp));
    assert_eq!(
        fixture
            .engine
            .as_mut()
            .unwrap()
            .next_generator_endpoint_event(),
        Some(blocker)
    );
    fixture.process_commands();
    assert_eq!(
        fixture
            .engine
            .as_mut()
            .unwrap()
            .next_generator_endpoint_event(),
        Some(GeneratorEndpointEvent::StampedRemoved {
            stamp,
            result: Ok(())
        })
    );
    assert!(
        fixture
            .engine
            .as_mut()
            .unwrap()
            .next_generator_endpoint_event()
            .is_none()
    );
    fixture.finish();
}

#[test]
fn stamped_removal_retirement_ring_full_keeps_command_and_all_state() {
    let mut fixture = RemovalFixture::new();
    let stamp = fixture.stamp();
    let worker = fixture.add_empty();
    let audio = fixture.workers[worker].audio.take().unwrap();
    let (mut retired, receiver) = RingBuffer::new(1);
    fixture.engine.as_mut().unwrap().retired_insert_endpoints = receiver;
    assert!(
        fixture
            .engine
            .as_mut()
            .unwrap()
            .remove_generator_endpoint_stamped(stamp)
    );
    assert!(
        retired
            .push(RetiredEndpointResource {
                _endpoint: fixed_adapter(audio),
                _pdc_delay: None,
            })
            .is_ok()
    );
    fixture
        .callback
        .as_mut()
        .unwrap()
        .dsp
        .retired_insert_endpoints = Some(retired);
    let before = fixture.observe();
    fixture.process_commands();
    assert_eq!(fixture.observe(), before);
    assert!(matches!(fixture.callback.as_mut().unwrap().commands.peek(),
        Ok(AudioCommand::RemoveGeneratorEndpointStamped(queued)) if *queued == stamp));
    assert!(
        fixture
            .engine
            .as_mut()
            .unwrap()
            .generator_endpoint_events
            .pop()
            .is_err()
    );
    drop(
        fixture
            .engine
            .as_mut()
            .unwrap()
            .retired_insert_endpoints
            .pop()
            .unwrap(),
    );
    fixture.process_commands();
    assert_eq!(
        fixture
            .engine
            .as_mut()
            .unwrap()
            .generator_endpoint_events
            .pop()
            .unwrap(),
        GeneratorEndpointEvent::StampedRemoved {
            stamp,
            result: Ok(())
        }
    );
    let removed = fixture
        .engine
        .as_mut()
        .unwrap()
        .retired_insert_endpoints
        .pop()
        .unwrap();
    assert_eq!(removed._endpoint.project_session, stamp.project_session);
    assert!(removed._pdc_delay.is_some());
    drop(removed);
    fixture.finish();
}

#[test]
fn stamped_removal_exact_ack_is_distinct_and_replay_missing_target_refuses() {
    let mut fixture = RemovalFixture::new();
    // Any nonzero operation ID is correlation, not mutable DSP authority. The
    // coordinator separately requires exact equality with its issued operation.
    let stamp = GeneratorRemovalStamp {
        operation_id: 702,
        ..fixture.stamp()
    };
    assert!(
        fixture
            .engine
            .as_mut()
            .unwrap()
            .remove_generator_endpoint_stamped(stamp)
    );
    fixture.process_commands();
    assert!(
        fixture
            .callback
            .as_mut()
            .unwrap()
            .dsp
            .generator_endpoints
            .iter()
            .all(Option::is_none)
    );
    assert_eq!(
        fixture
            .engine
            .as_mut()
            .unwrap()
            .generator_endpoint_events
            .pop()
            .unwrap(),
        GeneratorEndpointEvent::StampedRemoved {
            stamp,
            result: Ok(())
        }
    );
    assert!(
        fixture
            .engine
            .as_mut()
            .unwrap()
            .generator_endpoint_events
            .pop()
            .is_err()
    );
    let removed = fixture
        .engine
        .as_mut()
        .unwrap()
        .retired_insert_endpoints
        .pop()
        .unwrap();
    assert_eq!(removed._endpoint.manifest.instance_id(0), Some(700));
    drop(removed);
    fixture.assert_rejected(stamp, GeneratorRemovalRejectReason::MissingEndpoint);
    fixture.finish();
}

#[test]
fn stamped_removal_never_touches_replacement_and_legacy_removal_is_unchanged() {
    let mut fixture = RemovalFixture::new();
    let stamp = fixture.stamp();
    let worker = fixture.add_instrument(701, 0.25);
    let audio = fixture.workers[worker].audio.take().unwrap();
    assert!(
        fixture
            .engine
            .as_mut()
            .unwrap()
            .install_generator_endpoint_for_project(7, 9, 71, 701, 1, audio)
    );
    fixture.process_commands();
    assert!(matches!(
        fixture
            .engine
            .as_mut()
            .unwrap()
            .next_generator_endpoint_event(),
        Some(GeneratorEndpointEvent::Installed {
            endpoint_id: 71,
            success: true,
            ..
        })
    ));
    fixture.assert_rejected(stamp, GeneratorRemovalRejectReason::EndpointChanged);
    fixture
        .callback
        .as_mut()
        .unwrap()
        .dsp
        .callback_transport_playing = true;
    assert!(
        fixture
            .engine
            .as_mut()
            .unwrap()
            .remove_generator_endpoint(9)
    );
    fixture.process_commands();
    assert_eq!(
        fixture
            .engine
            .as_mut()
            .unwrap()
            .next_generator_endpoint_event(),
        Some(GeneratorEndpointEvent::Removed {
            channel_id: 9,
            endpoint_id: Some(71),
            plugin_instance_id: Some(701),
        })
    );
    fixture.finish();
}

fn activate_removal_fixture_timeline(
    fixture: &mut RemovalFixture,
    timeline: Arc<CompiledTimeline>,
    revision: u64,
    epoch: u64,
    playing: bool,
) {
    let chase = fixture
        .engine
        .as_mut()
        .unwrap()
        .prepare_timeline_chase(
            &timeline,
            revision,
            epoch,
            0,
            TimelineChaseOptions::default(),
        )
        .unwrap();
    let loop_chase = fixture
        .engine
        .as_mut()
        .unwrap()
        .prepare_timeline_loop_chase(&timeline, revision, 0, TimelineChaseOptions::default())
        .unwrap();
    fixture
        .engine
        .as_mut()
        .unwrap()
        .install_compiled_timeline(revision, timeline)
        .unwrap();
    let loop_token = fixture
        .engine
        .as_mut()
        .unwrap()
        .install_timeline_loop_chase(loop_chase)
        .unwrap();
    fixture
        .engine
        .as_mut()
        .unwrap()
        .install_timeline_chase(chase)
        .unwrap();
    fixture
        .engine
        .as_mut()
        .unwrap()
        .activate_timeline_transport(TimelineTransportActivationSpec {
            revision,
            target_epoch: epoch,
            minimum_epoch: epoch,
            frame: 0,
            beat_q32: 0,
            loop_start_frame: 0,
            loop_end_frame: 0,
            loop_start_q32: 0,
            loop_end_q32: 0,
            loop_token,
            loop_enabled: false,
            playing,
            mixer_pan_release: TimelineMixerPanRelease::EMPTY,
        })
        .unwrap();
    fixture
        .callback
        .as_mut()
        .unwrap()
        .pump(fixture.engine.as_ref().unwrap(), 128);
    let mut accepted = false;
    while let Some(event) = fixture
        .engine
        .as_mut()
        .unwrap()
        .next_timeline_runtime_event()
    {
        if let TimelineRuntimeEvent::TransportActivationApplied { .. } = event {
            accepted = true;
        }
    }
    assert!(
        accepted,
        "test activation must use the actual callback receipt"
    );
    assert_eq!(
        fixture
            .engine
            .as_mut()
            .unwrap()
            .confirmed_timeline_identity(),
        Some((revision, epoch))
    );
    assert_eq!(
        fixture
            .engine
            .as_mut()
            .unwrap()
            .snapshot()
            .transport_playing,
        playing
    );
}

fn poison_removal_residues(dsp: &mut DspState, graph_route_slot: usize) {
    for delay in dsp.pdc_raw_track_delays.iter_mut() {
        delay.request_delay(7, 0).unwrap();
        for _ in 0..16 {
            let _ = delay.process_sample([0.75, -0.75]);
        }
    }
    let slot = dsp.generator_endpoints[0].as_mut().unwrap();
    slot.pdc_delay.request_delay(5, 0).unwrap();
    for _ in 0..16 {
        let _ = slot.pdc_delay.process_sample([0.75, -0.75]);
    }
    let bank = dsp
        .timeline_runtime
        .as_mut()
        .unwrap()
        .active_mixer_delay_bank_mut()
        .unwrap();
    for _ in 0..8193 {
        let _ = bank
            .process_sample(graph_route_slot, [0.75, -0.75])
            .unwrap();
    }
    dsp.track_block.fill([0.75, -0.75]);
    dsp.master_block.fill([0.75, -0.75]);
    dsp.plugin_input_left.fill(0.75);
    dsp.plugin_input_right.fill(-0.75);
    dsp.plugin_output_left.fill(0.75);
    dsp.plugin_output_right.fill(-0.75);
    install_constant_timeline_voice(dsp, 9, 1);
}

#[test]
fn stamped_removal_stopped_epoch_and_acceptance_exclude_every_poisoned_residue() {
    let mut fixture = RemovalFixture::new();
    let mut source_project = timeline_test_project(4.0);
    let mut channel = timeline_test_channel(9, 1);
    channel.instrument_plugin_instance_id = Some(700);
    source_project.channels.push(channel);
    source_project
        .plugin_instances
        .push(timeline_test_plugin(700));
    let source_timeline = compile_timeline_test_project(&source_project);
    activate_removal_fixture_timeline(&mut fixture, Arc::clone(&source_timeline), 10, 2, false);

    // Stopped activation leaves its real All Notes Off safety lane pending. A paused
    // callback deliberately does not advance this source, so drain one actual Q128
    // quantum before admitting the fixture's single live Note On. Ring enqueue alone
    // would otherwise succeed even though endpoint live staging still refuses it.
    {
        let endpoint = &fixture.callback.as_ref().unwrap().dsp.generator_endpoints[0]
            .as_ref()
            .unwrap()
            .endpoint;
        assert_eq!(endpoint.adapter.input_phase_frames(), 0);
        assert!(endpoint.events.scratch.live_faulted);
        assert_eq!(
            endpoint.events.scratch.system.pending_len,
            TIMELINE_ENDPOINT_DISCONTINUITY_EVENTS
        );
        for channel in 0..16_u8 {
            assert_eq!(
                endpoint.events.scratch.system.pending[usize::from(channel)],
                FrameEvent::midi(0, None, [0xB0 | channel, 123, 0])
            );
        }
        assert_eq!(endpoint.events.scratch.live.pending_len, 0);
    }
    let submitted_before_safety = fixture.workers[0].control().stats().submitted;
    assert!(
        fixture
            .callback
            .as_mut()
            .unwrap()
            .dsp
            .process_generator_endpoints(128)
    );
    let submitted_safety = fixture.workers[0].control().stats().submitted;
    assert_eq!(submitted_safety, submitted_before_safety + 1);
    wait_until(|| fixture.workers[0].control().stats().completed >= submitted_safety);
    {
        let endpoint = &fixture.callback.as_ref().unwrap().dsp.generator_endpoints[0]
            .as_ref()
            .unwrap()
            .endpoint;
        assert_eq!(endpoint.adapter.input_phase_frames(), 0);
        assert!(!endpoint.events.scratch.live_faulted);
        assert_eq!(endpoint.events.scratch.system.pending_len, 0);
        assert_eq!(endpoint.events.scratch.live.pending_len, 0);
    }
    let live_overflows_before_note = fixture
        .callback
        .as_ref()
        .unwrap()
        .dsp
        .fixed_quantum_event_overflows;

    // Produce real nonzero mock worker output, including a partially consumed
    // fixed-quantum output FIFO. No SDK, helper process, or audio device exists.
    assert!(
        fixture
            .engine
            .as_mut()
            .unwrap()
            .send_generator_midi(9, Some(0), [0x90, 60, 127], 0)
    );
    fixture.process_commands();
    {
        let dsp = &fixture.callback.as_ref().unwrap().dsp;
        assert_eq!(
            dsp.fixed_quantum_event_overflows,
            live_overflows_before_note
        );
        let endpoint = &dsp.generator_endpoints[0].as_ref().unwrap().endpoint;
        assert!(!endpoint.events.scratch.live_faulted);
        assert_eq!(endpoint.events.scratch.system.pending_len, 0);
        assert_eq!(endpoint.events.scratch.live.pending_len, 1);
        assert_eq!(
            endpoint.events.scratch.live.pending[0],
            FrameEvent::midi(0, Some(0), [0x90, 60, 127])
        );
    }
    let mut saw_old_audio = false;
    for _ in 0..fixture
        .callback
        .as_mut()
        .unwrap()
        .dsp
        .plugin_timing
        .lookahead_quanta
        + 4
    {
        fixture
            .callback
            .as_mut()
            .unwrap()
            .dsp
            .track_block
            .fill([0.0; 2]);
        assert!(
            fixture
                .callback
                .as_mut()
                .unwrap()
                .dsp
                .process_generator_endpoints(128)
        );
        let submitted = fixture.workers[0].control().stats().submitted;
        wait_until(|| fixture.workers[0].control().stats().completed >= submitted);
        saw_old_audio |= fixture
            .callback
            .as_mut()
            .unwrap()
            .dsp
            .track_block
            .iter()
            .any(|frame| frame[0] != 0.0);
    }
    assert!(
        saw_old_audio,
        "the old source must actually produce poison audio"
    );
    assert!(
        fixture
            .callback
            .as_mut()
            .unwrap()
            .dsp
            .process_generator_endpoints(1)
    );
    let before_epoch = fixture.callback.as_mut().unwrap().dsp.generator_endpoints[0]
        .as_ref()
        .unwrap()
        .endpoint
        .stats();
    assert_eq!(
        fixture.callback.as_mut().unwrap().dsp.generator_endpoints[0]
            .as_ref()
            .unwrap()
            .endpoint
            .adapter
            .input_phase_frames(),
        1
    );
    let graph_route_slot = (0..crate::mixer_graph::MIXER_GRAPH_MAX_EDGES)
        .find(|slot| {
            fixture
                .callback
                .as_mut()
                .unwrap()
                .dsp
                .timeline_runtime
                .as_ref()
                .unwrap()
                .active_mixer_delay_bank()
                .unwrap()
                .target_delay_samples(*slot)
                .is_some_and(|delay| delay > 0)
        })
        .expect("the generator bridge produces an actual delayed graph route");
    poison_removal_residues(
        &mut fixture.callback.as_mut().unwrap().dsp,
        graph_route_slot,
    );

    // A real stopped Timeline activation, rather than editing a stamp, creates
    // the new epoch and runs the existing endpoint/bridge generation reset.
    activate_removal_fixture_timeline(&mut fixture, source_timeline, 11, 3, false);
    fixture
        .callback
        .as_mut()
        .unwrap()
        .pump(fixture.engine.as_ref().unwrap(), 128);
    let after_epoch = fixture.callback.as_mut().unwrap().dsp.generator_endpoints[0]
        .as_ref()
        .unwrap()
        .endpoint
        .stats();
    assert!(
        after_epoch.partial_input_frames_discarded > before_epoch.partial_input_frames_discarded
    );
    assert!(
        after_epoch.output_frames_discarded_on_reset
            > before_epoch.output_frames_discarded_on_reset
    );
    assert_eq!(
        fixture.callback.as_mut().unwrap().dsp.generator_endpoints[0]
            .as_ref()
            .unwrap()
            .endpoint
            .epoch(),
        3
    );
    for delay in fixture
        .callback
        .as_mut()
        .unwrap()
        .dsp
        .pdc_raw_track_delays
        .iter_mut()
    {
        for _ in 0..16 {
            assert_eq!(delay.process_sample([0.0; 2]), [0.0; 2]);
        }
    }
    {
        let bank = fixture
            .callback
            .as_mut()
            .unwrap()
            .dsp
            .timeline_runtime
            .as_mut()
            .unwrap()
            .active_mixer_delay_bank_mut()
            .unwrap();
        for _ in 0..8193 {
            assert_eq!(
                bank.process_sample(graph_route_slot, [0.0; 2]),
                Some([0.0; 2])
            );
        }
    }

    // Re-poison callback histories after the stopped reset to prove the exact
    // acceptance boundary clears every callback-side residual again.
    poison_removal_residues(
        &mut fixture.callback.as_mut().unwrap().dsp,
        graph_route_slot,
    );
    let stamp = fixture.stamp();
    assert_eq!(stamp.stopped_epoch, 3);
    assert!(
        fixture
            .engine
            .as_mut()
            .unwrap()
            .remove_generator_endpoint_stamped(stamp)
    );
    fixture.process_commands();
    assert_eq!(
        fixture
            .engine
            .as_mut()
            .unwrap()
            .generator_endpoint_events
            .pop()
            .unwrap(),
        GeneratorEndpointEvent::StampedRemoved {
            stamp,
            result: Ok(())
        }
    );
    let dsp = &mut fixture.callback.as_mut().unwrap().dsp;
    assert!(dsp.generator_endpoints.iter().all(Option::is_none));
    assert!(dsp.plugin_topology_replan_pending);
    assert!(
        dsp.track_block
            .iter()
            .chain(dsp.master_block.iter())
            .all(|frame| *frame == [0.0; 2])
    );
    for buffer in [
        &dsp.plugin_input_left,
        &dsp.plugin_input_right,
        &dsp.plugin_output_left,
        &dsp.plugin_output_right,
    ] {
        assert!(buffer.iter().all(|sample| *sample == 0.0));
    }
    assert!(dsp.voices.iter().all(|voice| !voice.active));
    for delay in dsp.pdc_raw_track_delays.iter_mut() {
        for _ in 0..16 {
            assert_eq!(delay.process_sample([0.0; 2]), [0.0; 2]);
        }
    }
    let bank = dsp
        .timeline_runtime
        .as_mut()
        .unwrap()
        .active_mixer_delay_bank_mut()
        .unwrap();
    for _ in 0..8193 {
        assert_eq!(
            bank.process_sample(graph_route_slot, [0.0; 2]),
            Some([0.0; 2])
        );
    }
    let mut retired = fixture
        .engine
        .as_mut()
        .unwrap()
        .retired_insert_endpoints
        .pop()
        .unwrap();
    for _ in 0..16 {
        assert_eq!(
            retired
                ._pdc_delay
                .as_mut()
                .unwrap()
                .process_sample([0.0; 2]),
            [0.0; 2]
        );
    }
    drop(retired);
    for _ in 0..65 {
        fixture
            .callback
            .as_mut()
            .unwrap()
            .pump(fixture.engine.as_ref().unwrap(), 128);
        assert!(
            fixture.callback.as_mut().unwrap().dsp.master_block[..128]
                .iter()
                .all(|frame| *frame == [0.0; 2])
        );
    }

    // Join the exact old mock source before admitting the new audio topology.
    assert_eq!(
        fixture.workers[0].drain(),
        Some(RetainedShutdownPoll::Joined)
    );
    assert!(fixture.workers[0].control().stats().stopped);
    let callback = fixture.callback.as_mut().unwrap();
    let engine = fixture.engine.as_mut().unwrap();
    let mut audio_project = timeline_test_project(4.0);
    audio_project.audio_assets.push(AudioAsset {
        id: 77,
        name: "new audio".into(),
        path: PathBuf::from("new-audio.wav"),
        sample_rate: 48_000,
        channels: 1,
        bits_per_sample: 32,
        frames: 128,
        waveform_peaks: Vec::new(),
    });
    let mut clip = timeline_pattern_clip(8, frame_as_beat(128), 0);
    clip.kind = ClipKind::Audio;
    clip.start = frame_as_beat(128);
    clip.audio_asset_id = Some(77);
    audio_project.clips.push(clip);
    let mut pcm = vec![0.0; 128];
    pcm[0] = 0.125;
    assert!(engine.register_asset(
        AudioAssetOperation {
            engine_session: engine.engine_session(),
            generation: 12,
            operation_id: 202,
        },
        77,
        Arc::from(pcm),
        48_000,
        1
    ));
    callback.pump(engine, 128);
    assert!(matches!(
        engine.next_asset_event(),
        Some(AudioAssetEvent::Registered { success: true, .. })
    ));
    let timeline = compile_timeline_test_project(&audio_project);
    let chase = engine
        .prepare_timeline_chase(&timeline, 12, 4, 0, TimelineChaseOptions::default())
        .unwrap();
    let loop_chase = engine
        .prepare_timeline_loop_chase(&timeline, 12, 0, TimelineChaseOptions::default())
        .unwrap();
    engine.install_compiled_timeline(12, timeline).unwrap();
    let loop_token = engine.install_timeline_loop_chase(loop_chase).unwrap();
    engine.install_timeline_chase(chase).unwrap();
    engine
        .activate_timeline_transport(TimelineTransportActivationSpec {
            revision: 12,
            target_epoch: 4,
            minimum_epoch: 4,
            frame: 0,
            beat_q32: 0,
            loop_start_frame: 0,
            loop_end_frame: 0,
            loop_start_q32: 0,
            loop_end_q32: 0,
            loop_token,
            loop_enabled: false,
            playing: true,
            mixer_pan_release: TimelineMixerPanRelease::EMPTY,
        })
        .unwrap();
    callback.pump(engine, 128);
    assert_eq!(engine.confirmed_timeline_identity(), Some((12, 4)));
    assert!(
        callback.dsp.master_block[..128]
            .iter()
            .all(|frame| *frame == [0.0; 2])
    );
    callback.pump(engine, 128);
    let expected = (0.125_f32 * 0.72).tanh();
    assert_eq!(callback.dsp.master_block[0], [expected; 2]);
    assert!(
        callback.dsp.master_block[1..128]
            .iter()
            .all(|frame| *frame == [0.0; 2])
    );
    for _ in 0..65 {
        callback.pump(engine, 128);
        assert!(
            callback.dsp.master_block[..128]
                .iter()
                .all(|frame| *frame == [0.0; 2])
        );
    }
    fixture.finish();
}

struct InstallTimingBackend {
    latency: Arc<AtomicU32>,
    tail: Arc<AtomicU32>,
}

impl PluginBackend for InstallTimingBackend {
    fn name(&self) -> &str {
        "recreated timing mock"
    }
    fn prepare(&mut self, _: PluginPrepareConfig) -> Result<(), String> {
        Ok(())
    }
    fn process(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        frames: usize,
    ) -> Result<(), String> {
        left[..frames].fill(0.0);
        right[..frames].fill(0.0);
        Ok(())
    }
    fn send_midi(&mut self, _: MidiMessage) -> Result<(), String> {
        Ok(())
    }
    fn set_parameter(&mut self, _: u32, _: f32) -> Result<(), String> {
        Ok(())
    }
    fn get_parameter(&mut self, _: u32) -> Result<f32, String> {
        Ok(0.0)
    }
    fn save_state(&mut self) -> Result<Vec<u8>, String> {
        Ok(Vec::new())
    }
    fn load_state(&mut self, _: &[u8]) -> Result<(), String> {
        Ok(())
    }
    fn latency_samples(&self) -> u32 {
        self.latency.load(Ordering::Acquire)
    }
    fn tail_samples(&self) -> u32 {
        self.tail.load(Ordering::Acquire)
    }
}

fn assert_install_refused_without_replacement(fixture: &mut RemovalFixture, instance_id: u64) {
    let before = fixture.observe();
    fixture.process_commands();
    assert_eq!(fixture.observe(), before);
    assert_eq!(
        fixture
            .engine
            .as_mut()
            .unwrap()
            .generator_endpoint_events
            .pop()
            .unwrap(),
        GeneratorEndpointEvent::Installed {
            channel_id: 9,
            endpoint_id: 71,
            plugin_instance_id: instance_id,
            mixer_track: 1,
            replaced_endpoint_id: None,
            replaced_plugin_instance_id: None,
            success: false,
        }
    );
    let incoming = fixture
        .engine
        .as_mut()
        .unwrap()
        .retired_insert_endpoints
        .pop()
        .unwrap();
    assert_eq!(incoming._endpoint.manifest.instance_id(0), Some(701));
    assert!(incoming._pdc_delay.is_some());
    drop(incoming);
    assert!(
        fixture
            .engine
            .as_mut()
            .unwrap()
            .retired_insert_endpoints
            .pop()
            .is_err()
    );
    assert_eq!(
        fixture.callback.as_ref().unwrap().dsp.generator_endpoints[0]
            .as_ref()
            .unwrap()
            .endpoint_id,
        70
    );
}

#[test]
fn recreated_generator_install_refuses_actual_timing_drift_after_enqueue() {
    for change_tail in [false, true] {
        let mut fixture = RemovalFixture::new();
        let latency = Arc::new(AtomicU32::new(8));
        let tail = Arc::new(AtomicU32::new(11));
        let worker = fixture.add_timing(701, Arc::clone(&latency), Arc::clone(&tail));
        let audio = fixture.workers[worker].audio.take().unwrap();
        assert!(
            fixture
                .engine
                .as_mut()
                .unwrap()
                .install_generator_endpoint_for_project_with_timing(
                    7,
                    9,
                    71,
                    701,
                    1,
                    audio,
                    GeneratorInstallTimingExpectation {
                        latency_samples: 8,
                        tail_samples: 11
                    },
                )
        );
        if change_tail {
            tail.store(12, Ordering::Release);
        } else {
            latency.store(9, Ordering::Release);
        }
        assert!(
            fixture.workers[worker]
                .control()
                .set_slot_config(0, SlotConfig::default())
        );
        wait_until(|| {
            fixture.workers[worker]
                .control()
                .plugin_latency_snapshot()
                .is_some_and(|snapshot| {
                    snapshot.total_plugin_latency_samples == if change_tail { 8 } else { 9 }
                        && snapshot.tail_samples == if change_tail { 12 } else { 11 }
                })
        });
        assert_install_refused_without_replacement(&mut fixture, 701);
        fixture.finish();
    }
}

#[test]
fn recreated_generator_install_refuses_read_failure_inactive_and_wrong_manifest() {
    for failure in 0..4 {
        let mut fixture = RemovalFixture::new();
        let worker = fixture.add_instrument(701, 0.25);
        let audio = fixture.workers[worker].audio.take().unwrap();
        let mut endpoint = fixed_adapter(audio);
        endpoint.project_session = 7;
        endpoint.generator_install_timing = Some(GeneratorInstallTimingExpectation {
            latency_samples: 0,
            tail_samples: 0,
        });
        let snapshot = fixture.workers[worker]
            .control()
            .plugin_latency_snapshot()
            .unwrap();
        assert!(
            endpoint.coherent_latency_snapshot().is_some(),
            "a cached valid pair must not rescue a failed fresh read"
        );
        let mut instance_id = 701;
        match failure {
            0 => endpoint.script_fresh_endpoint_snapshots([None, None]),
            1 => endpoint.script_fresh_endpoint_snapshots([
                Some(PluginLatencySnapshot {
                    revision: 0,
                    ..snapshot
                }),
                None,
            ]),
            2 => endpoint.script_fresh_endpoint_snapshots([
                Some(PluginLatencySnapshot {
                    active_mask: 0,
                    ..snapshot
                }),
                None,
            ]),
            _ => instance_id = 702,
        }
        assert!(
            fixture
                .engine
                .as_mut()
                .unwrap()
                .command(AudioCommand::InstallGeneratorEndpoint {
                    channel_id: 9,
                    endpoint_id: 71,
                    plugin_instance_id: instance_id,
                    mixer_track: 1,
                    endpoint,
                    pdc_delay: test_pdc_delay(),
                })
        );
        assert_install_refused_without_replacement(&mut fixture, instance_id);
        fixture.finish();
    }
}

#[test]
fn recreated_generator_install_matches_once_and_adds_no_lifetime_pin() {
    let mut fixture = RemovalFixture::new();
    let latency = Arc::new(AtomicU32::new(8));
    let tail = Arc::new(AtomicU32::new(11));
    let worker = fixture.add_timing(701, Arc::clone(&latency), Arc::clone(&tail));
    let audio = fixture.workers[worker].audio.take().unwrap();
    assert!(
        fixture
            .engine
            .as_mut()
            .unwrap()
            .install_generator_endpoint_for_project_with_timing(
                7,
                9,
                71,
                701,
                1,
                audio,
                GeneratorInstallTimingExpectation {
                    latency_samples: 8,
                    tail_samples: 11
                },
            )
    );
    fixture.process_commands();
    assert_eq!(
        fixture
            .engine
            .as_mut()
            .unwrap()
            .next_generator_endpoint_event(),
        Some(GeneratorEndpointEvent::Installed {
            channel_id: 9,
            endpoint_id: 71,
            plugin_instance_id: 701,
            mixer_track: 1,
            replaced_endpoint_id: Some(70),
            replaced_plugin_instance_id: Some(700),
            success: true,
        })
    );
    assert!(
        fixture.callback.as_ref().unwrap().dsp.generator_endpoints[0]
            .as_ref()
            .unwrap()
            .endpoint
            .generator_install_timing
            .is_none()
    );
    latency.store(9, Ordering::Release);
    assert!(
        fixture.workers[worker]
            .control()
            .set_slot_config(0, SlotConfig::default())
    );
    wait_until(|| {
        fixture.workers[worker]
            .control()
            .plugin_latency_snapshot()
            .is_some_and(|snapshot| snapshot.total_plugin_latency_samples == 9)
    });
    let endpoint = &mut fixture.callback.as_mut().unwrap().dsp.generator_endpoints[0]
        .as_mut()
        .unwrap()
        .endpoint;
    endpoint.refresh_latency_snapshot();
    assert_eq!(
        endpoint
            .coherent_latency_snapshot()
            .unwrap()
            .total_plugin_latency_samples,
        9
    );
    assert_eq!(fixture.workers[worker].control().stats().faults, 0);
    fixture.finish();
}

#[test]
fn ordinary_generator_install_none_keeps_existing_timing_admission() {
    let mut fixture = RemovalFixture::new();
    let latency = Arc::new(AtomicU32::new(8));
    let tail = Arc::new(AtomicU32::new(11));
    let worker = fixture.add_timing(701, Arc::clone(&latency), tail);
    let audio = fixture.workers[worker].audio.take().unwrap();
    assert!(
        fixture
            .engine
            .as_mut()
            .unwrap()
            .install_generator_endpoint_for_project(7, 9, 71, 701, 1, audio)
    );
    latency.store(9, Ordering::Release);
    assert!(
        fixture.workers[worker]
            .control()
            .set_slot_config(0, SlotConfig::default())
    );
    wait_until(|| {
        fixture.workers[worker]
            .control()
            .plugin_latency_snapshot()
            .is_some_and(|snapshot| snapshot.total_plugin_latency_samples == 9)
    });
    fixture.process_commands();
    assert!(matches!(
        fixture
            .engine
            .as_mut()
            .unwrap()
            .next_generator_endpoint_event(),
        Some(GeneratorEndpointEvent::Installed {
            endpoint_id: 71,
            replaced_endpoint_id: Some(70),
            success: true,
            ..
        })
    ));
    fixture.finish();
}

#[test]
fn audio_fixture_assertion_unwind_restores_mailbox_and_joins_every_mock() {
    let mut witnesses = Vec::new();
    let mut mailbox = None;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut fixture = RemovalFixture::new();
        fixture.add_instrument(701, 0.25);
        fixture.add_empty();
        witnesses.extend(
            fixture
                .workers
                .iter()
                .map(|worker| Arc::clone(&worker.joined)),
        );
        let shared = Arc::clone(&fixture.engine.as_ref().unwrap().transport_mailbox);
        mailbox = Some(Arc::clone(&shared));
        let restore = MailboxSequenceRestore {
            sequence: shared.sequence.load(Ordering::Acquire),
            mailbox: shared,
        };
        restore.mailbox.sequence.store(1, Ordering::Release);
        panic!("intentional assertion-path cleanup proof");
    }));
    assert!(result.is_err());
    assert_eq!(witnesses.len(), 3);
    assert!(
        witnesses
            .iter()
            .all(|joined| joined.load(Ordering::Acquire) == 1)
    );
    assert_eq!(mailbox.unwrap().sequence.load(Ordering::Acquire) & 1, 0);
}
