//! Plugin-free proofs for the test-only allocation witness and run-owned collector.
//! Global-observer cases share App fixture admission for their complete lifetime.

use super::*;
use crate::genuine_app_observer::{
    self as observe, MAX_RECORD_BATCH, MAX_RECORDS, MAX_WORKER_IDENTITIES, ObservationRun,
    RecordKind, Stage, WorkerJoinOutcome,
};
use std::{cell::Cell, mem::ManuallyDrop, sync::MutexGuard};

const WAIT: Duration = Duration::from_secs(5);
static ADMISSION: Mutex<()> = Mutex::new(());
static CLEANUP_FAILED: AtomicBool = AtomicBool::new(false);

struct FailedOwner {
    _chain: PluginChain,
    _observer: Option<ObservationRun>,
}
thread_local! {
    static QUARANTINE: std::cell::RefCell<Option<ManuallyDrop<FailedOwner>>> =
        const { std::cell::RefCell::new(None) };
}

fn admission() -> MutexGuard<'static, ()> {
    let deadline = Instant::now() + WAIT;
    loop {
        assert!(
            !CLEANUP_FAILED.load(Ordering::Acquire),
            "exact worker remains quarantined"
        );
        assert!(
            Instant::now() < deadline,
            "identity fixture admission timed out"
        );
        match ADMISSION.try_lock() {
            Ok(guard) => return guard,
            Err(std::sync::TryLockError::Poisoned(error)) => return error.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => thread::sleep(Duration::from_millis(1)),
        }
    }
}

#[derive(Default)]
struct BackendEvidence {
    loaded: AtomicU64,
    prepared: AtomicU64,
    destroyed: AtomicU64,
}
struct FiniteBackend(Arc<BackendEvidence>);
impl Drop for FiniteBackend {
    fn drop(&mut self) {
        self.0.destroyed.fetch_add(1, Ordering::Release);
    }
}
impl PluginBackend for FiniteBackend {
    fn name(&self) -> &str {
        "identity witness fixture"
    }
    fn prepare(&mut self, _: PluginPrepareConfig) -> Result<(), String> {
        self.0.prepared.fetch_add(1, Ordering::Release);
        Ok(())
    }
    fn process(&mut self, _: &mut [f32], _: &mut [f32], _: usize) -> Result<(), String> {
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
        0
    }
    fn tail_samples(&self) -> u32 {
        0
    }
}

/// Installed immediately after successful spawn, before any assertion or wait.
/// As in the queued-worker fixture, only an actual terminal retained poll proves
/// cleanup. Pending/panic outcomes retain the exact chain and observer together;
/// no legacy guard Drop, unconditional join, or fabricated cleanup success runs.
struct SpawnFixture {
    chain: Option<PluginChain>,
    observer: Option<ObservationRun>,
    release: Option<mpsc::Sender<()>>,
    evidence: Arc<BackendEvidence>,
    cleanup: Option<RetainedShutdownPoll>,
}
impl SpawnFixture {
    fn new(observer: ObservationRun) -> Result<Self, String> {
        assert!(!CLEANUP_FAILED.load(Ordering::Acquire));
        QUARANTINE.with(|slot| assert!(slot.borrow().is_none()));
        let evidence = Arc::new(BackendEvidence::default());
        let worker_evidence = Arc::clone(&evidence);
        let (release, receive) = mpsc::channel();
        let chain = PluginChain::spawn_identified_with_backend_factory(
            &[17],
            move || {
                worker_evidence.loaded.fetch_add(1, Ordering::Release);
                // No unbounded backend/loader blocking, even if test coordination unwinds.
                let _ = receive.recv_timeout(WAIT);
                vec![BackendSlot::new(Box::new(FiniteBackend(worker_evidence)))]
            },
            PluginPrepareConfig {
                sample_rate: 48_000.0,
                max_block_frames: 128,
            },
        )?;
        Ok(Self {
            chain: Some(chain),
            observer: Some(observer),
            release: Some(release),
            evidence,
            cleanup: None,
        })
    }
    fn wait_loaded(&self) {
        let deadline = Instant::now() + WAIT;
        while self.evidence.loaded.load(Ordering::Acquire) == 0 {
            assert!(Instant::now() < deadline, "actual loader did not enter");
            thread::sleep(Duration::from_millis(1));
        }
    }
    fn finish(&mut self) -> RetainedShutdownPoll {
        if let Some(outcome) = self.cleanup {
            return outcome;
        }
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
        let chain = self
            .chain
            .as_mut()
            .expect("exact spawned chain remains owned");
        let deadline = Instant::now() + WAIT;
        let outcome = loop {
            let outcome = chain.guard.poll_shutdown_retained();
            if outcome != RetainedShutdownPoll::Pending || Instant::now() >= deadline {
                break outcome;
            }
            thread::sleep(Duration::from_millis(1));
        };
        self.cleanup = Some(outcome);
        if outcome != RetainedShutdownPoll::Joined {
            CLEANUP_FAILED.store(true, Ordering::Release);
            crate::app::CitrusApp::mark_fixture_cleanup_failed_for_test();
            let failed = FailedOwner {
                _chain: self.chain.take().expect("failed exact chain remains owned"),
                _observer: self.observer.take(),
            };
            QUARANTINE.with(|slot| *slot.borrow_mut() = Some(ManuallyDrop::new(failed)));
        }
        outcome
    }
    fn assert_joined_once(&mut self, worker_id: usize) {
        assert_eq!(self.finish(), RetainedShutdownPoll::Joined);
        let chain = self.chain.as_mut().unwrap();
        for _ in 0..8 {
            assert_eq!(
                chain.guard.poll_shutdown_retained(),
                RetainedShutdownPoll::Joined
            );
            assert!(chain.guard.join.is_none());
            assert_eq!(chain.guard.genuine_worker_id(), worker_id);
        }
        assert_eq!(chain.control.stats().detached_workers, 0);
        assert_eq!(chain.control.stats().shutdown_timeouts, 0);
        assert_eq!(self.evidence.loaded.load(Ordering::Acquire), 1);
        assert_eq!(self.evidence.prepared.load(Ordering::Acquire), 1);
        assert_eq!(self.evidence.destroyed.load(Ordering::Acquire), 1);
        self.observer.as_ref().unwrap().with_snapshot(|snapshot| {
            let mut joins = snapshot
                .records
                .iter()
                .filter_map(|record| match record.kind {
                    RecordKind::WorkerJoin { worker_id, outcome } => Some((worker_id, outcome)),
                    _ => None,
                });
            assert_eq!(joins.next(), Some((worker_id, WorkerJoinOutcome::Joined)));
            assert_eq!(joins.next(), None);
        });
    }
}
impl Drop for SpawnFixture {
    fn drop(&mut self) {
        let outcome = self.finish();
        if outcome != RetainedShutdownPoll::Joined && !thread::panicking() {
            panic!("identity fixture cleanup failed: {outcome:?}; exact owner quarantined");
        }
    }
}

#[test]
fn worker_identity_witness_preserves_allocation_not_metrics_or_worker_lifetime() {
    crate::app::CitrusApp::with_observer_fixture_admission_for_test(
        worker_identity_witness_preserves_allocation_under_admission,
    );
}

fn worker_identity_witness_preserves_allocation_under_admission() {
    let _admission = admission();
    let metrics = Arc::new(BridgeMetrics::default());
    let contents = Arc::clone(&metrics.requested_epoch);
    let original_id = Arc::as_ptr(&metrics) as usize;
    let strong = Arc::strong_count(&metrics);
    let witness = WorkerIdentityWitness::new(&metrics);
    assert_eq!(Arc::strong_count(&metrics), strong);
    assert_eq!(Arc::weak_count(&metrics), 1);
    assert_eq!(witness.0.as_ptr() as usize, original_id);
    assert_eq!(Arc::strong_count(&contents), 2);
    drop(metrics);
    assert!(witness.0.upgrade().is_none());
    assert_eq!(
        Arc::strong_count(&contents),
        1,
        "metrics contents have been destroyed"
    );
    assert_eq!(witness.0.as_ptr() as usize, original_id);
    drop(witness);

    let observer = ObservationRun::install().unwrap();
    let mut fixture = SpawnFixture::new(observer).unwrap();
    fixture.wait_loaded();
    let chain = fixture.chain.as_mut().unwrap();
    let original_id = Arc::as_ptr(&chain.guard.metrics) as usize;
    let original_thread = chain.guard.join.as_ref().unwrap().thread().id();
    assert_eq!(chain.audio.genuine_snapshot().worker_id, original_id);
    assert_eq!(chain.control.genuine_worker_id(), original_id);
    assert_eq!(chain.guard.genuine_worker_id(), original_id);
    assert_eq!(
        Arc::weak_count(&chain.guard.metrics),
        1,
        "synchronous collector witness"
    );
    let strong = Arc::strong_count(&chain.guard.metrics);
    let witness = WorkerIdentityWitness::new(&chain.guard.metrics);
    assert_eq!(Arc::strong_count(&chain.guard.metrics), strong);
    assert_eq!(witness.0.as_ptr() as usize, original_id);
    let contents = Arc::clone(&chain.guard.metrics.requested_epoch);
    assert_eq!(
        chain.guard.poll_shutdown_retained(),
        RetainedShutdownPoll::Pending
    );
    assert_eq!(
        chain.guard.join.as_ref().unwrap().thread().id(),
        original_thread
    );
    assert_eq!(chain.guard.genuine_worker_id(), original_id);
    fixture.assert_joined_once(original_id);
    assert_eq!(witness.0.as_ptr() as usize, original_id);
    fixture
        .observer
        .as_ref()
        .unwrap()
        .with_snapshot(|snapshot| assert_eq!(snapshot.failure, None));
    drop(fixture.chain.take());
    assert!(witness.0.upgrade().is_none());
    assert_eq!(
        Arc::strong_count(&contents),
        1,
        "no metrics contents or worker retained"
    );
    let snapshot = fixture.observer.take().unwrap().finish();
    assert_eq!(witness.0.as_ptr() as usize, original_id);
    assert_eq!(fixture.evidence.destroyed.load(Ordering::Acquire), 1);
    drop(snapshot);
    drop(witness);
}

#[test]
fn worker_identity_observer_retention_is_bounded_lazy_and_run_local() {
    crate::app::CitrusApp::with_observer_fixture_admission_for_test(
        worker_identity_observer_retention_under_admission,
    );
}

fn worker_identity_observer_retention_under_admission() {
    let _admission = admission();
    let built = Cell::new(0);
    let metrics: [Arc<BridgeMetrics>; MAX_WORKER_IDENTITIES] =
        std::array::from_fn(|_| Arc::new(BridgeMetrics::default()));
    let fifth = Arc::new(BridgeMetrics::default());
    observe::retain_worker_identity(|| {
        built.set(built.get() + 1);
        WorkerIdentityWitness::new(&fifth)
    });
    assert_eq!(built.get(), 0, "inactive builder must stay lazy");
    assert_eq!(Arc::weak_count(&fifth), 0);

    let observer = ObservationRun::install().unwrap();
    for owner in &metrics {
        observe::retain_worker_identity(|| {
            built.set(built.get() + 1);
            WorkerIdentityWitness::new(owner)
        });
        assert_eq!(Arc::strong_count(owner), 1);
        assert_eq!(Arc::weak_count(owner), 1);
    }
    assert_eq!(built.get(), MAX_WORKER_IDENTITIES);
    observe::retain_worker_identity(|| {
        built.set(built.get() + 1);
        WorkerIdentityWitness::new(&fifth)
    });
    assert_eq!(
        built.get(),
        MAX_WORKER_IDENTITIES,
        "full builder must stay lazy"
    );
    assert_eq!(Arc::weak_count(&fifth), 0);
    observer.with_snapshot(|snapshot| {
        assert_eq!(
            snapshot.failure,
            Some("observer worker identity cap exceeded")
        )
    });
    observer.set_stage(Stage::Finished).unwrap();
    assert!(observer.set_stage(Stage::Preflight).is_err());

    // Even full/failed observation must not replace the real spawn or loader result.
    let mut fixture = SpawnFixture::new(observer).unwrap();
    fixture.wait_loaded();
    let chain = fixture.chain.as_ref().unwrap();
    let worker_id = chain.guard.genuine_worker_id();
    assert_eq!(
        Arc::weak_count(&chain.guard.metrics),
        0,
        "fifth spawn is unretained"
    );
    fixture.assert_joined_once(worker_id);
    drop(fixture.chain.take());
    let observer = fixture.observer.take().unwrap();
    for _ in 0..=MAX_RECORD_BATCH {
        observe::record_lifecycle("bounded identity record");
    }
    let copies = observer.copy_records_since(0);
    assert_eq!(copies.len(), MAX_RECORD_BATCH);
    observer.with_snapshot(|snapshot| {
        assert_eq!(
            snapshot.failure,
            Some("observer worker identity cap exceeded")
        );
        assert!(Arc::ptr_eq(&copies[0], &snapshot.records[0]));
    });
    let mut snapshot = observer.finish();
    assert!(metrics.iter().all(|owner| Arc::weak_count(owner) == 1));
    snapshot.records.clear();
    assert!(
        metrics.iter().all(|owner| Arc::weak_count(owner) == 1),
        "record drain cannot recycle witnesses"
    );
    drop(snapshot);
    assert!(
        metrics.iter().all(|owner| Arc::weak_count(owner) == 0),
        "copied records cannot retain witnesses"
    );
    assert!(!copies.is_empty());
    drop(copies);

    // Independent record exhaustion does not consume or recycle identity slots.
    let observer = ObservationRun::install().unwrap();
    observer.with_snapshot(|snapshot| {
        assert!(snapshot.records.is_empty());
        assert_eq!(snapshot.failure, None);
    });
    for _ in 0..=MAX_RECORDS {
        observe::record_lifecycle("record cap before identity");
    }
    observer.with_snapshot(|snapshot| {
        assert_eq!(snapshot.records.len(), MAX_RECORDS);
        assert_eq!(snapshot.failure, Some("observer record cap exceeded"));
    });
    observe::retain_worker_identity(|| WorkerIdentityWitness::new(&metrics[0]));
    assert_eq!(Arc::weak_count(&metrics[0]), 1);
    let poisoned = panic::catch_unwind(AssertUnwindSafe(|| {
        observer.with_snapshot(|_| panic!("intentional observation mutex poison"));
    }));
    assert_eq!(
        poisoned.unwrap_err().downcast_ref::<&str>().copied(),
        Some("intentional observation mutex poison")
    );
    for owner in &metrics[1..] {
        observe::retain_worker_identity(|| WorkerIdentityWitness::new(owner));
    }
    observe::retain_worker_identity(|| panic!("poisoned/full builder must stay lazy"));
    observer.with_snapshot(|snapshot| {
        assert_eq!(snapshot.failure, Some("observer record cap exceeded"))
    });
    assert_eq!(observer.copy_records_since(0).len(), MAX_RECORD_BATCH);
    assert!(metrics.iter().all(|owner| Arc::weak_count(owner) == 1));
    drop(observer);
    assert!(
        metrics.iter().all(|owner| Arc::weak_count(owner) == 0),
        "run Drop releases witnesses"
    );

    // A fresh poison must be latched, yet its free slot still performs bounded retention.
    let observer = ObservationRun::install().unwrap();
    let poisoned = panic::catch_unwind(AssertUnwindSafe(|| {
        observer.with_snapshot(|_| panic!("intentional fresh observation mutex poison"));
    }));
    assert_eq!(
        poisoned.unwrap_err().downcast_ref::<&str>().copied(),
        Some("intentional fresh observation mutex poison")
    );
    observe::retain_worker_identity(|| WorkerIdentityWitness::new(&fifth));
    assert_eq!(Arc::strong_count(&fifth), 1);
    assert_eq!(Arc::weak_count(&fifth), 1);
    observer
        .with_snapshot(|snapshot| assert_eq!(snapshot.failure, Some("observer mutex poisoned")));
    let snapshot = observer.finish();
    assert_eq!(Arc::weak_count(&fifth), 1);
    drop(snapshot);
    assert_eq!(Arc::weak_count(&fifth), 0);

    let observer = ObservationRun::install().unwrap();
    observer.with_snapshot(|snapshot| {
        assert!(snapshot.records.is_empty());
        assert_eq!(snapshot.failure, None);
    });
    for owner in &metrics {
        observe::retain_worker_identity(|| WorkerIdentityWitness::new(owner));
    }
    observer.with_snapshot(|snapshot| assert_eq!(snapshot.failure, None));
    assert!(metrics.iter().all(|owner| Arc::weak_count(owner) == 1));
    drop(observer);
    assert!(metrics.iter().all(|owner| Arc::weak_count(owner) == 0));
    observe::retain_worker_identity(|| panic!("finished run must not retain a witness"));
}
