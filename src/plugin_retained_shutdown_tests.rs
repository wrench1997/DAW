//! Source-retirement seam fixtures only; no plug-in, helper or audio-device invocation.

use super::*;

const FIXTURE_WORKER_BUDGET: Duration = Duration::from_secs(5);

/// RAII-scoped lifetime for a real JoinHandle with deterministic admin-queue inspection.
///
/// The test owns the receiver so it can model a stalled admin consumer, drain precisely one
/// command, or disconnect it without scheduling races. A separate release channel controls
/// actual thread completion; an empty/disconnected admin queue never manufactures completion.
/// Both normal teardown and assertion/unwind paths release and join the worker before guard
/// Drop can run. The worker also has its own finite wait if test coordination fails.
struct RetainedFixture {
    guard: PluginWorkerGuard,
    admin: Option<Receiver<QueuedAdminCommand>>,
    release: Option<mpsc::Sender<()>>,
    cleanup_joined: Arc<AtomicBool>,
}

impl RetainedFixture {
    fn new(worker_panics: bool) -> Self {
        let (admin_tx, admin_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::channel();
        let metrics = Arc::new(BridgeMetrics::default());
        let admin_tx = AdminCommandSender::new(admin_tx, &metrics);
        let join = thread::spawn(move || {
            let _ = release_rx.recv_timeout(FIXTURE_WORKER_BUDGET);
            assert!(!worker_panics, "retained source fixture worker panic");
        });
        Self {
            guard: PluginWorkerGuard {
                admin: admin_tx,
                join: Some(join),
                metrics,
                retained_shutdown: RetainedShutdownState::NotRequested,
                fresh_retained_shutdown: false,
            },
            admin: Some(admin_rx),
            release: Some(release_tx),
            cleanup_joined: Arc::new(AtomicBool::new(false)),
        }
    }

    fn receiver(&self) -> &Receiver<QueuedAdminCommand> {
        self.admin.as_ref().expect("fixture admin is connected")
    }

    fn receive_command(&self) -> Result<AdminCommand, TryRecvError> {
        // This fixture simulates command completion at inspection; no backend runs.
        self.receiver()
            .try_recv()
            .map(|mut queued| queued.take_command())
    }

    fn fill_admin(&self) {
        assert!(
            self.guard
                .admin
                .try_send(AdminCommand::SetSlotConfig {
                    slot: 0,
                    config: SlotConfig::default(),
                })
                .is_ok()
        );
    }

    fn worker_id(&self) -> thread::ThreadId {
        self.guard
            .join
            .as_ref()
            .expect("fixture still owns the worker")
            .thread()
            .id()
    }

    fn assert_pending_same_worker(&mut self, id: thread::ThreadId) {
        assert_eq!(
            self.guard.poll_shutdown_retained(),
            RetainedShutdownPoll::Pending
        );
        assert_eq!(self.worker_id(), id);
        self.assert_no_detach();
    }

    fn assert_no_detach(&self) {
        assert_eq!(self.guard.metrics.snapshot().detached_workers, 0);
        assert_eq!(self.guard.metrics.snapshot().shutdown_timeouts, 0);
    }

    fn release_worker(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
    }

    fn wait_until_finished(&self) {
        let deadline = Instant::now() + FIXTURE_WORKER_BUDGET;
        while !self.guard.join.as_ref().unwrap().is_finished() {
            assert!(Instant::now() < deadline, "fixture worker failed to finish");
            thread::sleep(Duration::from_millis(1));
        }
    }

    fn finish_and_assert(&mut self, expected: RetainedShutdownPoll) {
        self.release_worker();
        self.wait_until_finished();
        assert_eq!(self.guard.poll_shutdown_retained(), expected);
        assert!(
            self.guard.join.is_none(),
            "terminal poll must consume the join"
        );
        for _ in 0..8 {
            assert_eq!(self.guard.poll_shutdown_retained(), expected);
            assert!(self.guard.join.is_none());
        }
        self.assert_no_detach();
    }
}

impl Drop for RetainedFixture {
    fn drop(&mut self) {
        self.release_worker();
        // Fixture cleanup is permitted to wait for this bounded mock, including on a test panic.
        // Production poll never calls join until is_finished; these are distinct contracts.
        if let Some(join) = self.guard.join.take() {
            let _ = join.join();
            self.cleanup_joined.store(true, Ordering::Release);
        }
    }
}

#[test]
fn retained_full_admin_queue_retries_without_replacing_or_detaching_worker() {
    let mut fixture = RetainedFixture::new(false);
    let id = fixture.worker_id();
    fixture.fill_admin();
    for _ in 0..8 {
        fixture.assert_pending_same_worker(id);
        assert!(matches!(
            fixture.guard.retained_shutdown,
            RetainedShutdownState::NotRequested
        ));
    }
    assert!(matches!(
        fixture.receive_command(),
        Ok(AdminCommand::SetSlotConfig { slot: 0, .. })
    ));
    assert!(matches!(
        fixture.receive_command(),
        Err(TryRecvError::Empty)
    ));

    fixture.assert_pending_same_worker(id);
    assert!(matches!(
        fixture.guard.retained_shutdown,
        RetainedShutdownState::WaitingForJoin
    ));
    assert!(matches!(
        fixture.receive_command(),
        Ok(AdminCommand::Shutdown)
    ));
    fixture.finish_and_assert(RetainedShutdownPoll::Joined);
    assert!(matches!(
        fixture.receive_command(),
        Err(TryRecvError::Empty)
    ));
}

#[test]
fn retained_admitted_shutdown_is_never_replayed_when_queue_has_space() {
    let mut fixture = RetainedFixture::new(false);
    let id = fixture.worker_id();
    fixture.assert_pending_same_worker(id);
    assert!(matches!(
        fixture.receive_command(),
        Ok(AdminCommand::Shutdown)
    ));
    for _ in 0..32 {
        fixture.assert_pending_same_worker(id);
        assert!(matches!(
            fixture.receive_command(),
            Err(TryRecvError::Empty)
        ));
    }
    fixture.finish_and_assert(RetainedShutdownPoll::Joined);
    assert!(matches!(
        fixture.receive_command(),
        Err(TryRecvError::Empty)
    ));
}

#[test]
fn retained_disconnected_admin_still_waits_for_exact_thread_join() {
    let mut fixture = RetainedFixture::new(false);
    let id = fixture.worker_id();
    drop(fixture.admin.take());
    for _ in 0..8 {
        fixture.assert_pending_same_worker(id);
        assert!(matches!(
            fixture.guard.retained_shutdown,
            RetainedShutdownState::WaitingForJoin
        ));
    }
    fixture.finish_and_assert(RetainedShutdownPoll::Joined);
}

#[test]
fn retained_stalled_worker_keeps_guard_and_join_after_shutdown_admission() {
    let mut fixture = RetainedFixture::new(false);
    let id = fixture.worker_id();
    fixture.assert_pending_same_worker(id);
    for _ in 0..8 {
        fixture.assert_pending_same_worker(id);
    }
    // The original command remains queued; observing admission is not observing teardown.
    assert!(matches!(
        fixture.receive_command(),
        Ok(AdminCommand::Shutdown)
    ));
    assert!(matches!(
        fixture.receive_command(),
        Err(TryRecvError::Empty)
    ));
    fixture.finish_and_assert(RetainedShutdownPoll::Joined);
}

#[test]
fn retained_worker_panic_is_joined_and_cached_as_failure() {
    let mut fixture = RetainedFixture::new(true);
    let id = fixture.worker_id();
    fixture.assert_pending_same_worker(id);
    assert!(matches!(
        fixture.receive_command(),
        Ok(AdminCommand::Shutdown)
    ));
    fixture.finish_and_assert(RetainedShutdownPoll::WorkerPanicked);
    assert!(matches!(
        fixture.guard.retained_shutdown,
        RetainedShutdownState::WorkerPanicked
    ));
    assert!(matches!(
        fixture.receive_command(),
        Err(TryRecvError::Empty)
    ));
}

#[test]
fn retained_already_finished_worker_joins_even_when_shutdown_queue_is_full() {
    let mut fixture = RetainedFixture::new(false);
    fixture.fill_admin();
    fixture.finish_and_assert(RetainedShutdownPoll::Joined);
    assert!(matches!(
        fixture.receive_command(),
        Ok(AdminCommand::SetSlotConfig { slot: 0, .. })
    ));
    assert!(matches!(
        fixture.receive_command(),
        Err(TryRecvError::Empty)
    ));
}

#[test]
fn retained_fixture_unwind_releases_and_joins_without_legacy_guard_detach() {
    let mut fixture = RetainedFixture::new(false);
    let metrics = Arc::clone(&fixture.guard.metrics);
    let cleanup_joined = Arc::clone(&fixture.cleanup_joined);
    let result = panic::catch_unwind(AssertUnwindSafe(move || {
        let id = fixture.worker_id();
        fixture.fill_admin();
        fixture.assert_pending_same_worker(id);
        panic!("fixture owner failure while the worker is still pending");
    }));
    assert!(result.is_err());
    assert!(cleanup_joined.load(Ordering::Acquire));
    assert_eq!(metrics.snapshot().detached_workers, 0);
    assert_eq!(metrics.snapshot().shutdown_timeouts, 0);
}
