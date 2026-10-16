//! An actual metadata call holds the original source worker, not a fabricated
//! status. Callback/App ticks continue throughout the wait for final publication.
use super::*;
use std::sync::{Condvar, Mutex};

struct CloneHoldState {
    state: Mutex<(bool, bool)>, // entered, released
    wake: Condvar,
}
struct CloneHold(Arc<CloneHoldState>);
impl CloneHold {
    fn new() -> Self {
        Self(Arc::new(CloneHoldState {
            state: Mutex::new((false, false)),
            wake: Condvar::new(),
        }))
    }
    fn entered(&self) -> bool {
        self.0.state.lock().unwrap().0
    }
    fn release(&self) {
        self.0.state.lock().unwrap().1 = true;
        self.0.wake.notify_all();
    }
}
impl Drop for CloneHold {
    fn drop(&mut self) {
        self.release();
    }
}
impl CloneHoldState {
    fn wait(&self) {
        let mut state = self.state.lock().unwrap();
        state.0 = true;
        let deadline = Instant::now() + Duration::from_secs(5);
        while !state.1 {
            assert!(
                Instant::now() < deadline,
                "Closed clone factory was not released"
            );
            (state, _) = self
                .wake
                .wait_timeout(state, deadline.saturating_duration_since(Instant::now()))
                .unwrap();
        }
    }
}

/// Returns only after the real checked writer has requested its final CAS,
/// while the actual source metadata call and callback token are still Waiting.
fn enter_source_commit_wait(this: &mut SerialApp, deadline: Option<Instant>) -> PathBuf {
    enter_source_commit_wait_internal(this, deadline, false)
}

fn enter_source_commit_wait_internal(
    this: &mut SerialApp,
    deadline: Option<Instant>,
    generator: bool,
) -> PathBuf {
    let path = this.begin_held_second();
    std::fs::write(&path, b"original destination").unwrap();
    if let Some(deadline) = deadline {
        this.fixture
            .app_mut()
            .whole_song_export_job
            .as_mut()
            .unwrap()
            .render_deadline = Some(deadline);
    }
    let clone_hold = CloneHold::new();
    let worker_hold = Arc::clone(&clone_hold.0);
    install_app_serial_renderer(this, Arc::new(AtomicBool::new(false)), move || {
        worker_hold.wait()
    });
    this.effects[1].release_all();
    this.until(|_| clone_hold.entered());
    if generator {
        this.fixture.source_handle().pause_metadata();
        this.until(|this| this.fixture.source_handle().metadata_waiting());
    } else {
        this.effects[0].hold_next_metadata(0);
        this.until(|this| this.effects[0].metadata_entered(0));
    }
    this.tick();
    let job = this.fixture.app().whole_song_export_job.as_ref().unwrap();
    assert_eq!(job.phase, WholeExportPhase::RenderingDisposing);
    assert!(
        job.failure.is_none(),
        "Metadata in flight is Waiting, not drift: {:?}",
        job.failure
    );
    assert_eq!(job.effect_leases.len(), 2);
    if generator {
        let source =
            &this.fixture.app().running_generator_chains[&job.actor.initial.stamp.channel_id];
        assert_eq!(
            source
                .control
                .stopped_source_work_settlement(job.generator_ticket.unwrap()),
            BounceSourceSettlement::Waiting
        );
    }
    assert!(!job.effect_leases[0].recheck(this.fixture.app()).unwrap());
    assert!(
        this.fixture
            .app()
            .audio
            .as_ref()
            .unwrap()
            .prepared_playback_token()
            .is_none()
    );
    clone_hold.release();
    this.until(|this| {
        this.fixture
            .app()
            .whole_song_export_job
            .as_ref()
            .unwrap()
            .source_commit
            .as_ref()
            .unwrap()
            .requested()
    });
    let job = this.fixture.app().whole_song_export_job.as_ref().unwrap();
    assert!(job.pending() && job.failure.is_none());
    assert!(
        job.export.progress().can_cancel,
        "App must not CAS while source is Waiting"
    );
    assert!(!job.render.as_ref().unwrap().termination_acknowledged());
    assert_eq!(std::fs::read(&path).unwrap(), b"original destination");
    this.fixture
        .assert_whole_exact_source_guards_unpolled_for_test();
    path
}

#[test]
fn actual_generator_metadata_wait_requires_ready_before_host_final_cas() {
    let mut this = SerialApp::new();
    let before = this.fixture.bytes();
    let ids = this.ids();
    let path = enter_source_commit_wait_internal(&mut this, None, true);
    this.fixture.source_handle().release_metadata();
    this.until(|this| !this.fixture.app().whole_song_export_fence_active());
    let job = this.fixture.app().whole_song_export_job.as_ref().unwrap();
    assert_eq!(job.phase, WholeExportPhase::Complete, "{:?}", job.failure);
    assert!(matches!(
        job.outcome,
        Some(MasterSongRenderOutcome::Complete(_))
    ));
    assert!(
        crate::wav::read_wav(&path)
            .unwrap()
            .samples
            .iter()
            .any(|v| *v != 0.0)
    );
    assert_healthy_original_sources(&this, &ids, &before);
}

fn assert_healthy_original_sources(this: &SerialApp, ids: &[usize], before: &[u8]) {
    assert_eq!(this.ids(), ids);
    assert_eq!(this.fixture.bytes(), before);
    this.fixture
        .assert_whole_exact_source_guards_unpolled_for_test();
    assert_eq!(this.fixture.whole_exact_worker_join_count_for_test(ids), 0);
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
}

#[test]
fn actual_metadata_wait_requires_fresh_complete_source_ready_before_final_cas() {
    let mut this = SerialApp::new();
    let before = this.fixture.bytes();
    let ids = this.ids();
    let path = enter_source_commit_wait(&mut this, None);
    let operations: Vec<_> = this
        .effects
        .iter()
        .map(|e| e.source_operations(0))
        .collect();
    this.effects[0].release_metadata(0);
    this.until(|this| !this.fixture.app().whole_song_export_fence_active());
    let job = this.fixture.app().whole_song_export_job.as_ref().unwrap();
    assert_eq!(job.phase, WholeExportPhase::Complete, "{:?}", job.failure);
    assert!(matches!(
        job.outcome,
        Some(MasterSongRenderOutcome::Complete(_))
    ));
    let wav = crate::wav::read_wav(&path).unwrap();
    assert!(wav.samples.iter().any(|v| *v != 0.0));
    assert_healthy_original_sources(&this, &ids, &before);
    assert_eq!(
        this.effects
            .iter()
            .map(|e| e.source_operations(0))
            .collect::<Vec<_>>(),
        operations
    );
}

#[test]
fn actual_metadata_latency_drift_cannot_use_waiting_commit_request() {
    let mut this = SerialApp::new();
    let before = this.fixture.bytes();
    let ids = this.ids();
    let path = enter_source_commit_wait(&mut this, None);
    this.effects[0].set_latency(0, 17);
    this.effects[0].release_metadata(0);
    this.until(|this| !this.fixture.app().whole_song_export_fence_active());
    let job = this.fixture.app().whole_song_export_job.as_ref().unwrap();
    assert_eq!(job.phase, WholeExportPhase::Failed);
    assert!(job.failure.is_some());
    assert!(matches!(
        job.outcome,
        Some(MasterSongRenderOutcome::Failed(_))
    ));
    assert_eq!(std::fs::read(&path).unwrap(), b"original destination");
    assert_healthy_original_sources(&this, &ids, &before);
}

#[test]
fn actual_new_source_admin_intent_invalidates_waiting_commit_before_processing() {
    let mut this = SerialApp::new();
    let before = this.fixture.bytes();
    let ids = this.ids();
    let path = enter_source_commit_wait(&mut this, None);
    let track = this
        .fixture
        .app()
        .whole_song_export_job
        .as_ref()
        .unwrap()
        .insert_sources[0]
        .track;
    assert!(
        this.fixture.app().running_insert_chains[&track]
            .control
            .set_parameter(0, 3, 0.5)
    );
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
    // Metadata is still held: the monotonic admitted intent must reject without
    // waiting for that later parameter command to run on the source worker.
    this.until(|this| !this.fixture.app().whole_song_export_fence_active());
    assert_eq!(
        this.fixture
            .app()
            .whole_song_export_job
            .as_ref()
            .unwrap()
            .phase,
        WholeExportPhase::Failed
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"original destination");
    assert_healthy_original_sources(&this, &ids, &before);
    this.effects[0].release_metadata(0);
}

#[test]
fn actual_cancel_wakes_final_source_wait_without_another_app_tick_or_source_shutdown() {
    let mut this = SerialApp::new();
    let before = this.fixture.bytes();
    let ids = this.ids();
    let path = enter_source_commit_wait(&mut this, None);
    let operations: Vec<_> = this
        .effects
        .iter()
        .map(|e| e.source_operations(0))
        .collect();
    assert!(this.fixture.app_mut().cancel_whole_song_export());
    wait_without_app_ticks(|| {
        this.fixture
            .app()
            .whole_song_export_job
            .as_ref()
            .unwrap()
            .render
            .as_ref()
            .unwrap()
            .termination_acknowledged()
    });
    this.tick();
    assert_eq!(
        this.fixture
            .app()
            .whole_song_export_job
            .as_ref()
            .unwrap()
            .phase,
        WholeExportPhase::Cancelled
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"original destination");
    assert_healthy_original_sources(&this, &ids, &before);
    assert_eq!(
        this.effects
            .iter()
            .map(|e| e.source_operations(0))
            .collect::<Vec<_>>(),
        operations
    );
    this.effects[0].release_metadata(0);
}

fn wait_without_app_ticks(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "Owner did not terminate without an App tick"
        );
        std::thread::yield_now();
    }
}

#[test]
fn actual_original_deadline_retires_final_source_wait_without_another_app_tick() {
    let mut this = SerialApp::new();
    let before = this.fixture.bytes();
    let ids = this.ids();
    let path = enter_source_commit_wait(&mut this, Some(Instant::now() + Duration::from_secs(2)));
    wait_without_app_ticks(|| {
        this.fixture
            .app()
            .whole_song_export_job
            .as_ref()
            .unwrap()
            .render
            .as_ref()
            .unwrap()
            .termination_acknowledged()
    });
    this.tick();
    let job = this.fixture.app().whole_song_export_job.as_ref().unwrap();
    assert_eq!(job.phase, WholeExportPhase::Failed);
    assert!(!job.cancel_requested);
    let Some(MasterSongRenderOutcome::Failed(failure)) = &job.outcome else {
        panic!("No actual deadline failure")
    };
    assert_eq!(
        failure.kind,
        crate::instrument_bounce::render::FailureKind::Deadline
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"original destination");
    assert_healthy_original_sources(&this, &ids, &before);
    this.effects[0].release_metadata(0);
}

#[test]
fn actual_app_drop_wakes_final_source_wait_and_retains_real_guards_until_join() {
    let mut this = SerialApp::new();
    let ids = this.ids();
    let path = enter_source_commit_wait(&mut this, None);
    let admission = this
        .fixture
        .app()
        .whole_song_export_job
        .as_ref()
        .unwrap()
        .admission
        .clone();
    this.fixture.force_drop_app_for_whole_test();
    this.dropped = true;
    wait_without_app_ticks(|| !admission.is_busy());
    assert_ne!(
        this.fixture.whole_exact_worker_join_count_for_test(&ids),
        3,
        "Held actual source metadata has not returned, so all sources cannot have joined"
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"original destination");
    this.effects[0].release_metadata(0);
    wait_without_app_ticks(|| this.fixture.whole_exact_worker_join_count_for_test(&ids) == 3);
    assert_eq!(std::fs::read(&path).unwrap(), b"original destination");
}

#[test]
fn actual_engine_loss_wakes_final_source_wait_and_retains_real_guards_until_join() {
    let mut this = SerialApp::new();
    let before = this.fixture.bytes();
    let ids = this.ids();
    let path = enter_source_commit_wait(&mut this, None);
    let admission = this
        .fixture
        .app()
        .whole_song_export_job
        .as_ref()
        .unwrap()
        .admission
        .clone();
    this.fixture
        .app_mut()
        .retain_clip_source_before_engine_loss();
    this.fixture.detach_callback_and_audio_for_whole_test();
    // One loss transition is observed, but no further normal App tick is
    // needed to release the writer from its host-mediated final boundary.
    this.tick();
    wait_without_app_ticks(|| !admission.is_busy());
    assert_ne!(this.fixture.whole_exact_worker_join_count_for_test(&ids), 3);
    assert_eq!(std::fs::read(&path).unwrap(), b"original destination");
    this.effects[0].release_metadata(0);
    this.until(|this| !this.fixture.app().whole_song_export_fence_active());
    assert_eq!(this.fixture.whole_exact_worker_join_count_for_test(&ids), 3);
    assert_eq!(this.fixture.bytes(), before);
    assert_eq!(std::fs::read(&path).unwrap(), b"original destination");
}
