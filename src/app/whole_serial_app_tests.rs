//! Real Whole stop actor and two real source endpoints; no clone or device is needed.
use super::*;
use crate::instrument_bounce::{
    effect::{CapturedEffectSlot, EffectSourceMetadata},
    render::serial::EffectRenderBackend,
};
use crate::plugins::plugin_runtime::effect_capture_tests::{
    EffectCaptureHandle, identified_single_effect_chain,
};
use std::collections::BTreeMap;

struct AppEffectClone {
    disposed: bool,
}
impl EffectRenderBackend for AppEffectClone {
    fn prepare(
        &mut self,
        slot: &CapturedEffectSlot,
        _: &RenderControl,
    ) -> Result<EffectSourceMetadata, BackendFailure> {
        assert_eq!(slot.state.opaque_state, vec![1, 0xE1, 0xFF]);
        assert_eq!(slot.state.parameter_bases[&3].to_bits(), 0.25_f64.to_bits());
        Ok(slot.state.metadata.clone())
    }
    fn process(
        &mut self,
        input: &[Vec<f32>],
        _: PluginTransport,
    ) -> Result<BlockReply, BackendFailure> {
        Ok(BlockReply {
            channels: input.to_vec(),
            transport_applied: true,
            output_events: 0,
            output_events_lost: false,
        })
    }
    fn dispose(&mut self, _: Instant) -> DisposalObservation {
        self.disposed = true;
        DisposalObservation {
            terminated: true,
            error: None,
            diagnostics: Vec::new(),
        }
    }
}
impl Drop for AppEffectClone {
    fn drop(&mut self) {
        assert!(self.disposed);
    }
}

struct SerialApp {
    fixture: OwnedSerialFixture,
    effects: Vec<EffectCaptureHandle>,
    dropped: bool,
}
struct OwnedSerialFixture(Option<Fixture>);
impl std::ops::Deref for OwnedSerialFixture {
    type Target = Fixture;
    fn deref(&self) -> &Fixture {
        self.0.as_ref().expect("Original serial fixture retained")
    }
}
impl std::ops::DerefMut for OwnedSerialFixture {
    fn deref_mut(&mut self) -> &mut Fixture {
        self.0.as_mut().expect("Original serial fixture retained")
    }
}
thread_local! {
    static RETAINED_SERIAL_APP: std::cell::RefCell<Vec<std::mem::ManuallyDrop<RetainedSerialApp>>> = const { std::cell::RefCell::new(Vec::new()) };
}
struct RetainedSerialApp {
    _fixture: Fixture,
    _effects: Vec<EffectCaptureHandle>,
}
impl SerialApp {
    fn new() -> Self {
        // These source-only lifecycle checks retain the exact controls below.
        // They do not install the process-global full genuine-App collector:
        // unrelated parallel worker/hook traffic belongs to another test.
        let mut fixture = Fixture::new();
        settle_initial_source(&mut fixture);
        let channel_id = fixture.app().project.channels[0].id;
        let serial = fixture.app().running_generator_chains[&channel_id]
            .control
            .native_editor_snapshot(0)
            .unwrap()
            .capture_serial;
        assert!(
            fixture.app().running_generator_chains[&channel_id]
                .control
                .request_state(0)
        );
        until(&mut fixture, |fixture| {
            fixture.app().running_generator_chains[&channel_id]
                .control
                .native_editor_snapshot(0)
                .unwrap()
                .capture_serial
                > serial
                && fixture.app().native_editor_topology_guard().is_ok()
        });
        let source_track = fixture
            .app()
            .project
            .mixer_runtime_slot(fixture.app().project.channels[0].mixer_track)
            .unwrap();
        assert_ne!(source_track, 0);
        let mut effects = Vec::new();
        for (runtime_track, id) in [(source_track, 91), (0, 7)] {
            let (chain, handle) = identified_single_effect_chain(id).unwrap();
            let request = handle.requests().remove(0);
            let mut candidate = fixture.app().project.clone();
            candidate.tempo = 128.0;
            let track = candidate
                .mixer_track_id_at_runtime_slot(runtime_track)
                .unwrap();
            candidate.plugin_instances.push(PluginInstance {
                id,
                format: crate::model::PluginFormat::Vst3,
                role: PluginRole::Effect,
                path: request.path,
                uid: format!("effect-{id}"),
                vendor: "Worker fixture".into(),
                name: format!("Effect {id}"),
                enabled: true,
                bypass: false,
                wet: 1.0,
                parameters: BTreeMap::from([(3, 0.25)]),
                opaque_state: vec![0x51],
                runtime_status: PluginRuntimeStatus::Loaded,
                midi_ports: Default::default(),
            });
            candidate.mixer_insert_slots.push(MixerInsertSlotRef {
                track,
                slot: 2,
                plugin_instance_id: id,
            });
            fixture
                .app_mut()
                .queue_mixer_track_candidate_prepared(runtime_track, candidate, None, Some(chain))
                .unwrap();
            fixture.app_mut().request_immediate_timeline_compile();
            let deadline = Instant::now() + Duration::from_secs(6);
            loop {
                fixture.pump_callback_only();
                fixture.app_mut().poll_insert_endpoint_events();
                fixture.tick();
                if fixture
                    .app()
                    .running_insert_chains
                    .contains_key(&runtime_track)
                    && !fixture.app().plugin_initializations_pending()
                    && fixture.app().timeline_audio_matches_desired()
                    && fixture
                        .app()
                        .audio
                        .as_ref()
                        .unwrap()
                        .prepared_playback_token()
                        .is_some()
                {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "Two real endpoints did not settle"
                );
            }
            effects.push(handle);
        }
        let app = fixture.app_mut();
        app.history_snapshot = app.project.clone();
        app.history_fingerprint = project_fingerprint(&app.project);
        app.project_fingerprint = app.history_fingerprint;
        app.undo_stack.clear();
        app.redo_stack.clear();
        app.dirty = false;
        fixture.retain_whole_exact_join_sources_for_test();
        Self {
            fixture: OwnedSerialFixture(Some(fixture)),
            effects,
            dropped: false,
        }
    }
    fn tick(&mut self) {
        self.fixture.pump_callback_only();
        self.fixture.app_mut().poll_insert_endpoint_events();
        self.fixture.tick();
    }
    fn until(&mut self, mut condition: impl FnMut(&Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(8);
        while !condition(self) {
            assert!(
                Instant::now() < deadline,
                "{}",
                whole_fixture_diagnostic(&self.fixture)
            );
            self.tick();
        }
    }
    fn begin_held_second(&mut self) -> PathBuf {
        self.effects[1].hold_slot(0);
        let selected = options(&self.fixture);
        let review = self
            .fixture
            .app()
            .prepare_whole_song_export_review(selected, true)
            .unwrap();
        let path = self.fixture.directory().join("serial-held.wav");
        self.fixture
            .app_mut()
            .begin_whole_song_export(review, path.clone())
            .unwrap();
        self.until(|this| this.effects[1].capture_count(0) == 1);
        self.effects[1].wait_entered(0);
        let job = self.fixture.app().whole_song_export_job.as_ref().unwrap();
        assert_eq!(job.phase, WholeExportPhase::CapturingEffects);
        assert_eq!(job.captured_effects.len(), 1);
        assert!(job.captured_generator.is_some() && job.actor.owns_pending_capture());
        path
    }
    fn ids(&self) -> Vec<usize> {
        self.fixture
            .app()
            .running_generator_chains
            .values()
            .map(|c| c.control.genuine_worker_id())
            .chain(
                self.fixture
                    .app()
                    .running_insert_chains
                    .values()
                    .map(|c| c.control.genuine_worker_id()),
            )
            .collect()
    }
}
impl Drop for SerialApp {
    fn drop(&mut self) {
        let was_panicking = std::thread::panicking();
        let cleanup = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.cleanup()));
        if !matches!(cleanup, Ok(Ok(()))) {
            crate::app::clip_render_job::tests::FIXTURE_POISONED.store(true, Ordering::Release);
            if let Some(fixture) = self.fixture.0.take() {
                RETAINED_SERIAL_APP.with(|slot| {
                    slot.borrow_mut()
                        .push(std::mem::ManuallyDrop::new(RetainedSerialApp {
                            _fixture: fixture,
                            _effects: self.effects.clone(),
                        }))
                });
            }
            if !was_panicking {
                panic!("Serial App source cleanup incomplete; exact owners and admission retained");
            }
        }
    }
}
impl SerialApp {
    fn cleanup(&mut self) -> Result<(), &'static str> {
        for effect in &self.effects {
            effect.release_all();
        }
        if self.dropped {
            return Ok(());
        }
        self.fixture.app_mut().cancel_whole_song_export();
        self.until(|this| !this.fixture.app().whole_song_export_fence_active());
        self.fixture.detach_callback_and_audio_for_whole_test();
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let mut pending = false;
            for source in self.fixture.app_mut().running_insert_chains.values_mut() {
                match source.guard.poll_shutdown_retained() {
                    RetainedShutdownPoll::Pending => pending = true,
                    RetainedShutdownPoll::Joined => {}
                    RetainedShutdownPoll::WorkerPanicked => {
                        return Err("Actual Effect source panicked");
                    }
                }
            }
            if !pending {
                break;
            }
            if Instant::now() >= deadline {
                return Err("Actual Effect source did not join");
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    }
}

#[test]
fn actual_whole_serial_original_deadline_cancels_held_source_and_prevents_later_calls_before_ack() {
    let mut this = SerialApp::new();
    let before = this.fixture.bytes();
    let ids = this.ids();
    this.effects[0].hold_slot(0);
    let selected = options(&this.fixture);
    let review = this
        .fixture
        .app()
        .prepare_whole_song_export_review(selected, true)
        .unwrap();
    let path = this
        .fixture
        .directory()
        .join("serial-original-deadline.wav");
    std::fs::write(&path, b"original destination").unwrap();
    this.fixture
        .app_mut()
        .begin_whole_song_export(review, path.clone())
        .unwrap();
    this.until(|this| {
        this.fixture
            .app()
            .whole_song_export_job
            .as_ref()
            .is_some_and(|job| {
                job.phase == WholeExportPhase::CapturingEffects && !job.actor.owns_pending_capture()
            })
    });
    let deadline = Instant::now() + Duration::from_millis(100);
    this.fixture
        .app_mut()
        .whole_song_export_job
        .as_mut()
        .unwrap()
        .render_deadline = Some(deadline);
    this.until(|this| this.effects[0].capture_count(0) == 1);
    this.effects[0].wait_entered(0);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
    this.tick();
    let job = this.fixture.app().whole_song_export_job.as_ref().unwrap();
    assert!(job.pending() && job.actor.owns_pending_capture());
    assert!(
        job.failure
            .as_deref()
            .unwrap()
            .contains("original render deadline")
    );
    assert!(
        !job.cancel_requested,
        "Deadline is a failed Whole job, not a user Cancel"
    );
    assert_eq!(this.effects[0].capture_completion_counts(0), (0, 0));
    assert_eq!(this.effects[1].capture_count(0), 0);
    assert_eq!(this.fixture.whole_exact_worker_join_count_for_test(&ids), 0);
    this.effects[0].release_all();
    this.until(|this| !this.fixture.app().whole_song_export_fence_active());
    let job = this.fixture.app().whole_song_export_job.as_ref().unwrap();
    assert_eq!(job.phase, WholeExportPhase::Failed);
    assert_eq!(
        job.effect_capture_failure,
        Some(EffectCapturePollError::Deadline)
    );
    assert_eq!(this.effects[0].capture_completion_counts(0), (0, 0));
    assert_eq!(this.effects[1].capture_count(0), 0);
    assert_eq!(this.fixture.whole_exact_worker_join_count_for_test(&ids), 0);
    assert_eq!(std::fs::read(&path).unwrap(), b"original destination");
    assert_eq!(this.fixture.bytes(), before);
}

#[test]
fn actual_whole_serial_complete_captures_flow_into_one_owned_renderer_and_one_final_wav() {
    let mut this = SerialApp::new();
    let before = this.fixture.bytes();
    let ids = this.ids();
    let path = this.begin_held_second();
    let consumed = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&consumed);
    this.fixture
        .app_mut()
        .whole_song_export_job
        .as_mut()
        .unwrap()
        .serial_render_starter = Some(Box::new(
        move |admission, project, captured, chains, options, path, control, reviewed| {
            assert!(reviewed && captured.plan().is_serial_master());
            assert_eq!(
                chains
                    .iter()
                    .flat_map(|chain| chain
                        .receipt
                        .slots
                        .iter()
                        .map(|slot| slot.request.instance_id))
                    .collect::<Vec<_>>(),
                [91, 7]
            );
            observed.store(true, Ordering::Release);
            let metadata = captured.metadata().clone();
            admission.start_serial_with_backends(
                project,
                captured,
                chains,
                options,
                path,
                control,
                reviewed,
                move || WholeMockBackend {
                    metadata,
                    disposed: false,
                    next: 0,
                    midi: Arc::new(std::sync::Mutex::new(Vec::new())),
                    prepare_failure: None,
                    disposal_failure: None,
                },
                || AppEffectClone { disposed: false },
            )
        },
    ));
    this.effects[1].release_all();
    this.until(|this| !this.fixture.app().whole_song_export_fence_active());
    let job = this.fixture.app().whole_song_export_job.as_ref().unwrap();
    assert_eq!(job.phase, WholeExportPhase::Complete, "{:?}", job.failure);
    assert!(consumed.load(Ordering::Acquire));
    assert!(matches!(
        job.outcome,
        Some(MasterSongRenderOutcome::Complete(_))
    ));
    let wav = crate::wav::read_wav(&path).unwrap();
    assert!(wav.samples.iter().any(|sample| *sample != 0.0));
    assert_eq!(this.fixture.bytes(), before);
    assert_eq!(this.ids(), ids);
    assert_eq!(this.fixture.whole_exact_worker_join_count_for_test(&ids), 0);
}

#[test]
fn actual_whole_serial_generator_worker_history_is_retained_after_generator_capture() {
    let mut this = SerialApp::new();
    let path = this.begin_held_second();
    let source = this
        .fixture
        .app()
        .running_generator_chains
        .values()
        .next()
        .unwrap();
    assert!(source.control.request_state(0));
    this.tick();
    assert!(
        this.fixture
            .app()
            .whole_song_export_job
            .as_ref()
            .unwrap()
            .failure
            .is_some()
    );
    assert!(this.fixture.app().whole_song_export_fence_active());
    this.effects[1].release_all();
    this.until(|this| !this.fixture.app().whole_song_export_fence_active());
    assert!(!path.exists());
}

#[test]
fn actual_whole_serial_earlier_endpoint_receipt_is_invalidated_while_later_capture_is_held() {
    let mut this = SerialApp::new();
    let before = this.fixture.bytes();
    let path = this.begin_held_second();
    let track = this
        .fixture
        .app()
        .whole_song_export_job
        .as_ref()
        .unwrap()
        .captured_effects[0]
        .track;
    assert!(
        this.fixture.app().running_insert_chains[&track]
            .control
            .request_state(0)
    );
    this.tick();
    assert!(
        this.fixture
            .app()
            .whole_song_export_job
            .as_ref()
            .unwrap()
            .failure
            .as_deref()
            .unwrap()
            .contains("Effect")
    );
    assert!(this.fixture.app().whole_song_export_fence_active());
    this.effects[1].release_all();
    this.until(|this| !this.fixture.app().whole_song_export_fence_active());
    assert_eq!(this.fixture.bytes(), before);
    assert!(!path.exists());
}

#[test]
fn actual_whole_serial_healthy_cancel_drains_effect_ack_without_destroying_any_source() {
    let mut this = SerialApp::new();
    let before = this.fixture.bytes();
    let ids = this.ids();
    let path = this.begin_held_second();
    let operations: Vec<_> = this
        .effects
        .iter()
        .map(|e| e.source_operations(0))
        .collect();
    let generator_resets = this.fixture.source_handle().reset_count();
    assert!(this.fixture.app_mut().cancel_whole_song_export());
    for _ in 0..3 {
        this.tick();
    }
    assert!(this.fixture.app().whole_song_export_fence_active());
    this.fixture
        .assert_whole_exact_source_guards_unpolled_for_test();
    assert_eq!(this.fixture.whole_exact_worker_join_count_for_test(&ids), 0);
    this.effects[1].release_all();
    this.until(|this| !this.fixture.app().whole_song_export_fence_active());
    assert_eq!(this.ids(), ids);
    assert_eq!(
        this.effects
            .iter()
            .map(|e| e.source_operations(0))
            .collect::<Vec<_>>(),
        operations
    );
    assert!(
        this.fixture
            .app()
            .running_insert_chains
            .values()
            .all(|c| !c.control.stats().stopped)
    );
    assert!(
        this.fixture
            .app()
            .running_generator_chains
            .values()
            .all(|c| !c.control.stats().stopped)
    );
    assert_eq!(this.fixture.source_handle().reset_count(), generator_resets);
    this.fixture
        .assert_whole_exact_source_guards_unpolled_for_test();
    assert_eq!(this.fixture.whole_exact_worker_join_count_for_test(&ids), 0);
    assert_eq!(this.fixture.bytes(), before);
    assert!(!path.exists());
}

#[test]
fn actual_whole_serial_exact_join_evidence_is_independent_of_foreign_collector_failure() {
    let mut this = SerialApp::new();
    // Other ordinary global-observer fixtures share this Fixture's admission.
    // The strict-clone summary test instead holds the helper lock. Take it in
    // Fixture -> helper order, and never publish a false hook to an unknown run.
    let _helper_observer_admission = crate::plugins::helper_transport_test_lock();
    let before = this.fixture.bytes();
    let ids = this.ids();
    let path = this.begin_held_second();
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                let foreign = crate::genuine_app_observer::ObservationRun::install()
                    .expect("source-only SerialApp must not own the global collector");
                crate::genuine_app_observer::record_hooks("foreign-test-only override", false);
                let failed_foreign = foreign.finish();
                // This run is deliberately invalid, not evidence for our sources.
                // Concurrent unrelated taps may already have latched another first
                // failure; neither that failure nor this hook can be cleared.
                assert!(failed_foreign.failure.is_some());
            })
            .join()
            .expect("foreign tap owner actually terminated");
    });
    this.fixture
        .assert_whole_exact_source_guards_unpolled_for_test();
    assert_eq!(this.fixture.whole_exact_worker_join_count_for_test(&ids), 0);
    assert_eq!(this.fixture.bytes(), before);
    this.fixture.force_drop_app_for_whole_test();
    this.dropped = true;
    assert_eq!(this.fixture.whole_exact_worker_join_count_for_test(&ids), 0);
    this.effects[1].release_all();
    let deadline = Instant::now() + Duration::from_secs(8);
    while this.fixture.whole_exact_worker_join_count_for_test(&ids) != 3 {
        assert!(
            Instant::now() < deadline,
            "Exact source joins did not return"
        );
        std::thread::yield_now();
    }
    assert!(!path.exists());
    // Retire the original Fixture before releasing the other observer's lock.
    drop(this);
}

#[test]
fn actual_whole_serial_engine_loss_retains_all_sources_until_effect_ack_and_actual_join() {
    let mut this = SerialApp::new();
    let ids = this.ids();
    let before = this.fixture.bytes();
    let path = this.begin_held_second();
    this.fixture
        .app_mut()
        .retain_clip_source_before_engine_loss();
    this.fixture.detach_callback_and_audio_for_whole_test();
    for _ in 0..3 {
        this.tick();
    }
    let job = this.fixture.app().whole_song_export_job.as_ref().unwrap();
    assert!(job.actor.owns_pending_capture() && job.retired_source.is_some());
    assert_eq!(job.retired_inserts.len(), 2);
    assert_eq!(this.fixture.whole_exact_worker_join_count_for_test(&ids), 0);
    this.effects[1].release_all();
    this.until(|this| !this.fixture.app().whole_song_export_fence_active());
    assert_eq!(this.fixture.whole_exact_worker_join_count_for_test(&ids), 3);
    assert_eq!(this.fixture.bytes(), before);
    assert!(!path.exists());
}

#[test]
fn actual_whole_serial_app_drop_keeps_all_source_guards_until_effect_ack_and_actual_join() {
    let mut this = SerialApp::new();
    let ids = this.ids();
    let path = this.begin_held_second();
    this.fixture.force_drop_app_for_whole_test();
    this.dropped = true;
    assert_eq!(this.fixture.whole_exact_worker_join_count_for_test(&ids), 0);
    assert!(!path.exists());
    this.effects[1].release_all();
    let deadline = Instant::now() + Duration::from_secs(8);
    while this.fixture.whole_exact_worker_join_count_for_test(&ids) != 3 {
        assert!(
            Instant::now() < deadline,
            "AppDrop did not join all three exact source workers"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(!path.exists());
}
