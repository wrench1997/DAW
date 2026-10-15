//! Actual App stop/Timeline notifications, callback graph witness and identified Effect workers.
//! The Clip job carries only the production stop notifications; no Clip capture, renderer,
//! media writer, helper, native editor window or audio device is started by these tests.
use super::*;
use crate::{
    app::stopped_bounce_capture::CapturedStoppedEffectChain,
    instrument_bounce::{BounceCaptureError, effect::EffectCapturePollError as CapturePollError},
    model::PluginFormat,
    plugins::plugin_runtime::effect_capture_tests::{
        EffectCaptureHandle, identified_two_slot_effect_chain,
    },
};

const EFFECT_WAIT: Duration = Duration::from_secs(4);
const EFFECT_TRACK: usize = 1;

struct EffectAppFixture {
    fixture: Option<Fixture>,
    effects: EffectCaptureHandle,
    actor: Option<StoppedBounceCapture>,
}

struct RetainedEffectAppFixture {
    _fixture: Fixture,
    _effects: EffectCaptureHandle,
    _actor: Option<StoppedBounceCapture>,
}

thread_local! {
    // A failed join retains the original App/worker, actor and serial admission together.
    static RETAINED_EFFECT_APP: std::cell::RefCell<Option<std::mem::ManuallyDrop<RetainedEffectAppFixture>>>
        = const { std::cell::RefCell::new(None) };
}

impl EffectAppFixture {
    fn new() -> Self {
        let mut fixture = Fixture::new();
        // The shared generator fixture intentionally begins with an uncaptured native
        // dirty revision. Topology changes require its real ordinary SaveState receipt
        // and App reconciliation first; a private bounce capture cannot clear that guard.
        let channel_id = fixture.app.project.channels[0].id;
        let capture_serial = fixture.app.running_generator_chains[&channel_id]
            .control
            .native_editor_snapshot(0)
            .unwrap()
            .capture_serial;
        assert!(
            fixture.app.running_generator_chains[&channel_id]
                .control
                .request_state(0)
        );
        fixture.until(|fixture| {
            fixture.app.running_generator_chains[&channel_id]
                .control
                .native_editor_snapshot(0)
                .is_some_and(|snapshot| snapshot.capture_serial > capture_serial)
                && fixture.app.native_editor_topology_guard().is_ok()
        });
        let (chain, effects) = identified_two_slot_effect_chain().unwrap();
        let mut this = Self {
            fixture: Some(fixture),
            effects,
            actor: None,
        };
        let requests = this.effects.requests();
        let fixture = this.fixture.as_mut().unwrap();
        let mut candidate = fixture.app.project.clone();
        candidate.tempo = 128.0;
        let track_id = candidate
            .mixer_track_id_at_runtime_slot(EFFECT_TRACK)
            .unwrap();
        // Physical worker order follows Project slot order, not numeric instance ID or
        // the storage order of these deliberately reversed placement references.
        for (physical_slot, request) in requests.iter().enumerate() {
            candidate.plugin_instances.push(PluginInstance {
                id: request.instance_id,
                format: PluginFormat::Vst3,
                role: PluginRole::Effect,
                path: request.path.clone(),
                // Persisted descriptor IDs do not select an SDK factory class.
                uid: format!("fixture-effect-descriptor-{}", request.instance_id),
                vendor: "Worker fixture".into(),
                name: format!("Effect {}", request.instance_id),
                enabled: request.config.enabled,
                bypass: request.config.bypassed,
                wet: request.config.wet,
                parameters: BTreeMap::from([(3, 0.25)]),
                opaque_state: vec![0x51],
                runtime_status: PluginRuntimeStatus::Loaded,
                midi_ports: Default::default(),
            });
            candidate.mixer_insert_slots.insert(
                0,
                MixerInsertSlotRef {
                    track: track_id,
                    slot: [2, 9][physical_slot],
                    plugin_instance_id: request.instance_id,
                },
            );
        }
        fixture
            .app
            .queue_mixer_track_candidate_prepared(EFFECT_TRACK, candidate, None, Some(chain))
            .unwrap();
        fixture.app.request_immediate_timeline_compile();
        this.control_until(|this| {
            let app = &this.fixture.as_ref().unwrap().app;
            app.running_insert_chains.contains_key(&EFFECT_TRACK)
                && !app.plugin_initializations_pending()
                && app.timeline_audio_matches_desired()
                && app
                    .stopped_bounce_observe(app.project.channels[0].id)
                    .is_ok()
                && app
                    .audio
                    .as_ref()
                    .unwrap()
                    .prepared_playback_token()
                    .is_some()
        });
        // Simulate unnotified authoritative vendor values after ordinary loaded-state
        // initialization. Project/cache f32 bases and opaque bytes deliberately stay stale.
        this.effects.set_parameter_base(0, 0.133_456_789_012_345);
        this.effects.set_parameter_base(1, 0.143_456_789_012_345);
        let fixture = this.fixture.as_mut().unwrap();
        fixture.app.history_snapshot = fixture.app.project.clone();
        fixture.app.history_fingerprint = project_fingerprint(&fixture.app.project);
        fixture.app.project_fingerprint = fixture.app.history_fingerprint;
        fixture.app.undo_stack.clear();
        fixture.app.redo_stack.clear();
        fixture.app.dirty = false;
        this
    }

    fn fixture(&self) -> &Fixture {
        self.fixture.as_ref().unwrap()
    }

    fn control_tick(&mut self) {
        let fixture = self.fixture.as_mut().unwrap();
        fixture.pump_callback_only();
        fixture.app.poll_insert_endpoint_events();
        fixture.app.poll_generator_endpoint_events();
        fixture.app.poll_plugin_runtime_events();
        fixture.app.poll_plugin_processing_health();
        fixture.app.poll_timeline_compile(&fixture.ctx);
        fixture.app.poll_audio_asset_events();
        fixture.app.poll_timeline_audio_runtime(&fixture.ctx);
        // Deliberately do not drive Clip or Whole: this test owns a private source actor.
        std::thread::sleep(Duration::from_millis(1));
    }

    fn control_until(&mut self, mut condition: impl FnMut(&Self) -> bool) {
        let deadline = Instant::now() + EFFECT_WAIT;
        while !condition(self) {
            assert!(
                Instant::now() < deadline,
                "actual Effect App callback/worker did not settle; {}",
                self.fixture().timeout_metadata()
            );
            self.control_tick();
        }
    }

    fn ready_actor(&mut self) {
        let fixture = self.fixture.as_mut().unwrap();
        let mut job = fixture.app.prepare_clip_render_review().unwrap();
        job.review.directory = fixture.directory.to_str().unwrap().to_owned();
        job.review.tail_acknowledged = true;
        fixture.app.begin_clip_render_stop(&mut job).unwrap();
        fixture.app.clip_render_job = Some(job);
        let deadline = Instant::now() + EFFECT_WAIT;
        loop {
            let fixture = self.fixture.as_mut().unwrap();
            let mut job = fixture.app.clip_render_job.take().unwrap();
            let ready = job.stopped.poll_stop(&mut fixture.app).unwrap().is_some();
            let graph_ready = fixture
                .app
                .audio
                .as_ref()
                .unwrap()
                .prepared_playback_token()
                .is_some();
            fixture.app.clip_render_job = Some(job);
            if ready && graph_ready {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "actual stopped actor did not reach its acknowledged graph boundary; {}",
                fixture.timeout_metadata()
            );
            self.control_tick();
        }
        let job = self
            .fixture
            .as_mut()
            .unwrap()
            .app
            .clip_render_job
            .take()
            .unwrap();
        let preparation = job.stopped.stop_preparation.unwrap();
        assert!(preparation.activation.unwrap().committed_epoch.is_some());
        assert_eq!(
            job.stopped.stop_settlement.unwrap().settlement,
            BounceSourceSettlement::Ready
        );
        assert!(!job.stopped.owns_pending_capture());
        assert!(job.plan.is_none());
        self.actor = Some(job.stopped);
    }

    fn begin_capture(&mut self) {
        self.actor
            .as_mut()
            .unwrap()
            .begin_effect_chain_capture(&self.fixture.as_ref().unwrap().app, EFFECT_TRACK)
            .unwrap();
        assert!(self.actor.as_ref().unwrap().owns_pending_capture());
    }

    fn poll_capture(&mut self) -> Result<Option<CapturedStoppedEffectChain>, CapturePollError> {
        self.actor
            .as_mut()
            .unwrap()
            .poll_effect_chain_capture(&self.fixture.as_ref().unwrap().app)
    }

    fn finish_capture(&mut self) -> Result<CapturedStoppedEffectChain, CapturePollError> {
        let deadline = Instant::now() + EFFECT_WAIT;
        loop {
            match self.poll_capture() {
                Ok(Some(capture)) => return Ok(capture),
                Err(error) => return Err(error),
                Ok(None) => {}
            }
            assert!(
                Instant::now() < deadline,
                "private Effect capture did not acknowledge termination"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn rejection(&mut self) -> CapturePollError {
        match self.finish_capture() {
            Err(error) => error,
            Ok(_) => panic!("changed stopped Effect source was accepted"),
        }
    }

    fn cleanup(&mut self) -> Result<(), &'static str> {
        self.effects.release_all();
        if let Some(actor) = &mut self.actor {
            actor.cancel_capture();
        }
        let deadline = Instant::now() + EFFECT_WAIT;
        while self
            .actor
            .as_ref()
            .is_some_and(StoppedBounceCapture::owns_pending_capture)
        {
            let _ = self.poll_capture();
            if Instant::now() >= deadline {
                return Err("private Effect worker termination ACK is still pending");
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let fixture = self.fixture.as_mut().unwrap();
        fixture.callback = None;
        loop {
            let mut pending = false;
            for chain in fixture.app.running_insert_chains.values_mut() {
                match chain.guard.poll_shutdown_retained() {
                    RetainedShutdownPoll::Pending => pending = true,
                    RetainedShutdownPoll::Joined => {}
                    RetainedShutdownPoll::WorkerPanicked => {
                        return Err("exact Effect worker panicked");
                    }
                }
            }
            for chain in fixture.app.pending_insert_chains.values_mut() {
                match chain.guard.poll_shutdown_retained() {
                    RetainedShutdownPoll::Pending => pending = true,
                    RetainedShutdownPoll::Joined => {}
                    RetainedShutdownPoll::WorkerPanicked => {
                        return Err("pending Effect worker panicked");
                    }
                }
            }
            if !pending {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err("exact Effect JoinHandle is still pending");
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

impl Drop for EffectAppFixture {
    fn drop(&mut self) {
        let was_panicking = std::thread::panicking();
        self.effects.release_all();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.cleanup()));
        if !matches!(result, Ok(Ok(()))) {
            FIXTURE_POISONED.store(true, Ordering::Release);
            let retained = RetainedEffectAppFixture {
                _fixture: self.fixture.take().unwrap(),
                _effects: self.effects.clone(),
                _actor: self.actor.take(),
            };
            RETAINED_EFFECT_APP.with(|slot| {
                *slot.borrow_mut() = Some(std::mem::ManuallyDrop::new(retained));
            });
            if !was_panicking {
                panic!("Effect App fixture cleanup failed; exact owners and admission retained");
            }
        }
    }
}

#[test]
fn actual_stopped_effect_capture_returns_complete_project_slot_order_and_private_vendor_state() {
    let mut fixture = EffectAppFixture::new();
    let before = fixture.fixture().bytes();
    fixture.ready_actor();
    let expected_token = fixture
        .fixture()
        .app
        .audio
        .as_ref()
        .unwrap()
        .prepared_playback_token()
        .unwrap();
    let expected_endpoint = fixture.fixture().app.running_insert_chains[&EFFECT_TRACK].endpoint_id;
    fixture.begin_capture();
    let captured = fixture.finish_capture().unwrap();
    assert_eq!(captured.track, EFFECT_TRACK);
    assert_eq!(captured.endpoint_id, expected_endpoint);
    assert_eq!(captured.token, expected_token);
    assert_eq!(
        captured.project_fingerprint,
        project_snapshot_fingerprint(&fixture.actor.as_ref().unwrap().snapshot)
    );
    assert_eq!(
        captured
            .placements
            .iter()
            .map(|slot| (slot.slot, slot.plugin_instance_id))
            .collect::<Vec<_>>(),
        vec![(2, 91), (9, 7)]
    );
    assert_eq!(captured.receipt.epoch, expected_token.epoch);
    assert_eq!(
        captured
            .receipt
            .slots
            .iter()
            .map(|slot| slot.request.clone())
            .collect::<Vec<_>>(),
        fixture.effects.requests()
    );
    for (slot, receipt) in captured.receipt.slots.iter().enumerate() {
        assert_eq!(receipt.state, fixture.effects.state_for_slot(slot));
        assert_ne!(
            receipt.state.parameter_bases[&3],
            f64::from(receipt.state.catalog[0].current_normalized)
        );
        assert_eq!(fixture.effects.capture_count(slot), 1);
        let native = fixture.fixture().app.running_insert_chains[&EFFECT_TRACK]
            .control
            .native_editor_snapshot(slot)
            .unwrap();
        assert_eq!(
            (native.capture_serial, native.parameter_capture_serial),
            (0, 0)
        );
        assert!(native.captured_state.is_none());
        assert!(native.captured_parameters.is_none());
    }
    assert!(!fixture.actor.as_ref().unwrap().owns_pending_capture());
    assert_eq!(fixture.fixture().source.capture_count(), 0);
    assert!(fixture.fixture().app.clip_render_job.is_none());
    assert_eq!(fixture.fixture().bytes(), before);
    assert_eq!(
        std::fs::read_dir(&fixture.fixture().directory)
            .unwrap()
            .count(),
        0
    );
}

#[derive(Clone, Copy, Debug)]
enum AppDrift {
    Endpoint,
    ModelRevision,
    Frame,
    Timing,
    ParameterWork,
    Project,
}

#[test]
fn actual_stopped_effect_capture_rejects_endpoint_revision_frame_timing_parameter_and_project_drift()
 {
    for drift in [
        AppDrift::Endpoint,
        AppDrift::ModelRevision,
        AppDrift::Frame,
        AppDrift::Timing,
        AppDrift::ParameterWork,
        AppDrift::Project,
    ] {
        let mut fixture = EffectAppFixture::new();
        fixture.ready_actor();
        fixture.effects.hold_slot(1);
        fixture.begin_capture();
        fixture.effects.wait_entered(1);
        assert_eq!(fixture.effects.capture_count(0), 1);
        let mut target_frame = None;
        {
            let app = &mut fixture.fixture.as_mut().unwrap().app;
            match drift {
                AppDrift::Endpoint => {
                    // Change App endpoint intent while retaining the exact original worker.
                    let chain = app.running_insert_chains.get_mut(&EFFECT_TRACK).unwrap();
                    chain.endpoint_id = next_nonzero_app_id(chain.endpoint_id);
                }
                AppDrift::ModelRevision => {
                    app.clip_render_model_revision =
                        next_nonzero_app_id(app.clip_render_model_revision);
                }
                AppDrift::Frame => {
                    let frame = app.audio.as_ref().unwrap().snapshot().timeline_frame + 128;
                    let beat = app
                        .timeline_audio_source
                        .as_ref()
                        .unwrap()
                        .tempo_map
                        .frame_to_beat(frame)
                        .unwrap();
                    app.timeline_audio_sync.request_seek(beat, frame);
                    target_frame = Some(frame);
                }
                AppDrift::Timing => {
                    let audio = app.audio.as_mut().unwrap();
                    audio
                        .request_plugin_timing_profile(audio.plugin_timing_profile())
                        .unwrap();
                }
                AppDrift::ParameterWork => {
                    assert!(
                        app.running_insert_chains[&EFFECT_TRACK]
                            .control
                            .set_parameter_tagged(0, 3, 0.625, 700)
                    );
                }
                AppDrift::Project => app.project.channels[0].volume = 0.375,
            }
        }
        let after_drift = fixture.fixture().bytes();
        // A stale/cancelled source cannot return terminal while slot 1 still owns its
        // already-dispatched call. The original atomic chain receipt remains retained.
        assert!(matches!(fixture.poll_capture(), Ok(None)), "{drift:?}");
        assert!(
            fixture.actor.as_ref().unwrap().owns_pending_capture(),
            "{drift:?}"
        );
        fixture.effects.release_all();
        if let Some(frame) = target_frame {
            fixture.control_until(|fixture| {
                fixture
                    .fixture()
                    .app
                    .audio
                    .as_ref()
                    .unwrap()
                    .snapshot()
                    .timeline_frame
                    == frame
            });
        }
        let error = fixture.rejection();
        assert!(
            matches!(
                error,
                CapturePollError::Stale(_)
                    | CapturePollError::Backend(BounceCaptureError::Rejected(_))
            ),
            "{drift:?}: {error}"
        );
        assert!(!fixture.actor.as_ref().unwrap().owns_pending_capture());
        assert_eq!(fixture.effects.capture_count(0), 1);
        assert_eq!(fixture.effects.capture_count(1), 1);
        assert_eq!(fixture.fixture().source.capture_count(), 0);
        assert_eq!(fixture.fixture().bytes(), after_drift, "{drift:?}");
    }
}

#[test]
fn actual_stopped_effect_capture_rechecks_earlier_slot_revision_and_exact_parameter_bases() {
    for parameter_drift in [false, true] {
        let mut fixture = EffectAppFixture::new();
        let before = fixture.fixture().bytes();
        fixture.ready_actor();
        fixture.effects.hold_slot(1);
        fixture.begin_capture();
        fixture.effects.wait_entered(1);
        if parameter_drift {
            fixture.effects.set_parameter_base(0, 0.875_000_000_000_001);
        } else {
            fixture.effects.set_revision(0, 2);
        }
        fixture.effects.release_all();
        let error = fixture.rejection();
        assert!(
            matches!(error, CapturePollError::Backend(BounceCaptureError::Rejected(ref message))
            if message.contains("drifted")),
            "{error}"
        );
        assert_eq!(
            fixture.fixture().app.running_insert_chains[&EFFECT_TRACK]
                .control
                .stats()
                .faults,
            0
        );
        assert!(!fixture.actor.as_ref().unwrap().owns_pending_capture());
        assert_eq!(fixture.fixture().bytes(), before);
    }
}

#[test]
fn actual_stopped_effect_cancel_keeps_the_owner_until_held_worker_really_acknowledges() {
    let mut fixture = EffectAppFixture::new();
    let before = fixture.fixture().bytes();
    fixture.ready_actor();
    fixture.effects.hold_slot(0);
    fixture.begin_capture();
    fixture.effects.wait_entered(0);
    fixture.actor.as_mut().unwrap().cancel_capture();
    for _ in 0..3 {
        assert!(matches!(fixture.poll_capture(), Ok(None)));
        assert!(fixture.actor.as_ref().unwrap().owns_pending_capture());
    }
    assert!(
        fixture
            .actor
            .as_mut()
            .unwrap()
            .begin_effect_chain_capture(&fixture.fixture.as_ref().unwrap().app, EFFECT_TRACK)
            .is_err()
    );
    assert_eq!(fixture.effects.capture_count(0), 1);
    assert_eq!(fixture.effects.capture_count(1), 0);
    fixture.effects.release_all();
    assert!(matches!(fixture.rejection(), CapturePollError::Cancelled));
    assert!(!fixture.actor.as_ref().unwrap().owns_pending_capture());
    assert_eq!(fixture.fixture().bytes(), before);
}

#[test]
fn actual_stopped_effect_engine_loss_drains_the_original_private_receipt_before_releasing_owner() {
    let mut fixture = EffectAppFixture::new();
    let before = fixture.fixture().bytes();
    fixture.ready_actor();
    fixture.effects.hold_slot(0);
    fixture.begin_capture();
    fixture.effects.wait_entered(0);
    let endpoint_id = fixture.fixture().app.running_insert_chains[&EFFECT_TRACK].endpoint_id;
    {
        let original = fixture.fixture.as_mut().unwrap();
        original.callback = None;
        original.app.audio = None;
    }
    for _ in 0..3 {
        assert!(matches!(fixture.poll_capture(), Ok(None)));
        assert!(fixture.actor.as_ref().unwrap().owns_pending_capture());
        let retained = &fixture.fixture().app.running_insert_chains[&EFFECT_TRACK];
        assert_eq!(retained.endpoint_id, endpoint_id);
        assert_eq!(retained.instance_ids, vec![91, 7]);
        assert_eq!(retained.control.stats().detached_workers, 0);
    }
    fixture.effects.release_all();
    let error = fixture.rejection();
    assert!(
        matches!(error, CapturePollError::Stale(ref message) if message.contains("Audio")),
        "{error}"
    );
    assert!(!fixture.actor.as_ref().unwrap().owns_pending_capture());
    assert_eq!(fixture.effects.capture_count(0), 1);
    assert_eq!(fixture.effects.capture_count(1), 0);
    assert_eq!(fixture.fixture().bytes(), before);
}

#[test]
fn actual_stopped_effect_fixture_unwind_releases_the_gate_and_joins_the_exact_worker() {
    let mut directory = None;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut fixture = EffectAppFixture::new();
        directory = Some(fixture.fixture().directory.clone());
        fixture.ready_actor();
        fixture.effects.hold_slot(0);
        fixture.begin_capture();
        fixture.effects.wait_entered(0);
        panic!("intentional private Effect capture fixture unwind");
    }));
    let panic = result.expect_err("the actual held capture must reach the intentional unwind");
    let text = panic
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| panic.downcast_ref::<String>().map(String::as_str));
    assert_eq!(
        text,
        Some("intentional private Effect capture fixture unwind")
    );
    assert!(
        !FIXTURE_POISONED.load(Ordering::Acquire),
        "the original capture ACK and Effect/source worker joins must finish before admission retires"
    );
    let directory = directory.unwrap();
    assert!(
        directory.exists(),
        "unwind preserves the diagnostic directory after proven joins"
    );
    assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 0);
    std::fs::remove_dir(&directory).unwrap();
}
