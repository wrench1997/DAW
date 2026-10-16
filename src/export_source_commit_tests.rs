use super::*;
use std::{panic::AssertUnwindSafe, thread, time::Duration};

fn wait_requested(owner: &SourceCommitOwner) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !owner.requested() {
        assert!(
            Instant::now() < deadline,
            "Writer never requested final source CAS"
        );
        thread::yield_now();
    }
}

#[test]
fn ungated_legacy_checked_and_ordinary_commit_contracts_are_unchanged() {
    let checked = ExportControl::default();
    checked
        .begin_commit_before(Instant::now() + Duration::from_secs(2))
        .unwrap();
    assert!(!checked.cancel());
    let ordinary = ExportControl::default();
    ordinary.begin_commit().unwrap();
    assert!(!ordinary.cancel());
}

#[test]
fn source_checked_writer_requires_host_actual_cas_and_cannot_use_legacy_fallback() {
    let control = ExportControl::default();
    let owner = control.require_source_checked_commit().unwrap();
    assert!(control.require_source_checked_commit().is_err());
    assert!(control.begin_commit().is_err());
    assert_eq!(control.0.state.load(Ordering::Acquire), RUNNING);
    let worker = control.clone();
    let deadline = Instant::now() + Duration::from_secs(2);
    let writer = thread::spawn(move || worker.begin_commit_before(deadline));
    wait_requested(&owner);
    assert_eq!(control.0.state.load(Ordering::Acquire), RUNNING);
    assert!(control.progress().can_cancel);
    assert!(owner.commit_before(deadline).unwrap());
    assert!(!owner.commit_before(deadline).unwrap());
    writer.join().expect("Actual writer joined").unwrap();
    assert_eq!(control.0.state.load(Ordering::Acquire), COMMITTING);
    assert!(!control.cancel());
    assert!(
        control.begin_commit_before(deadline).is_err(),
        "One-use request was reused"
    );
}

#[test]
fn cancellation_retires_pending_request_without_another_host_tick() {
    let control = ExportControl::default();
    let owner = control.require_source_checked_commit().unwrap();
    let worker = control.clone();
    let deadline = Instant::now() + Duration::from_secs(2);
    let writer = thread::spawn(move || worker.begin_commit_before(deadline));
    wait_requested(&owner);
    assert!(control.cancel());
    let error = writer
        .join()
        .expect("Actual cancelled writer joined")
        .unwrap_err();
    assert!(error.is::<ExportCancelled>());
    assert!(!owner.commit_before(deadline).unwrap());
    assert_eq!(control.0.state.load(Ordering::Acquire), CANCELLED);
}

#[test]
fn host_owner_drop_cancels_waiting_writer_without_another_tick() {
    let control = ExportControl::default();
    let owner = control.require_source_checked_commit().unwrap();
    let worker = control.clone();
    let writer =
        thread::spawn(move || worker.begin_commit_before(Instant::now() + Duration::from_secs(2)));
    wait_requested(&owner);
    drop(owner);
    let error = writer
        .join()
        .expect("Actual orphaned writer joined")
        .unwrap_err();
    assert!(error.is::<ExportCancelled>());
    assert_eq!(control.0.state.load(Ordering::Acquire), CANCELLED);
}

#[test]
fn timed_out_writer_atomically_retires_request_before_late_host_cas() {
    let control = ExportControl::default();
    let owner = control.require_source_checked_commit().unwrap();
    let worker = control.clone();
    let writer =
        thread::spawn(move || worker.begin_commit_before(Instant::now() + Duration::from_secs(2)));
    wait_requested(&owner);
    control.deadline_at(9900);
    wake(&control);
    let error = writer
        .join()
        .expect("Actual timed-out writer joined")
        .unwrap_err();
    assert!(error.is::<ExportDeadlineExceeded>());
    control.deadline_at(0);
    assert!(
        !owner
            .commit_before(Instant::now() + Duration::from_secs(2))
            .unwrap()
    );
    assert_eq!(control.0.state.load(Ordering::Acquire), RUNNING);
}

#[test]
fn unwound_writer_atomically_retires_request_before_late_host_cas() {
    let control = ExportControl::default();
    let owner = control.require_source_checked_commit().unwrap();
    owner
        .fence
        .panic_after_request
        .store(true, Ordering::Release);
    let worker = control.clone();
    let writer = thread::spawn(move || {
        std::panic::catch_unwind(AssertUnwindSafe(|| {
            worker.begin_commit_before(Instant::now() + Duration::from_secs(2))
        }))
    });
    assert!(
        writer
            .join()
            .expect("Actual unwound writer joined")
            .is_err()
    );
    assert!(!owner.requested());
    assert!(
        !owner
            .commit_before(Instant::now() + Duration::from_secs(2))
            .unwrap()
    );
    assert_eq!(control.0.state.load(Ordering::Acquire), RUNNING);
}

#[test]
fn actual_cas_wins_over_host_unwind_missing_notify_and_later_deadline_or_drop() {
    let control = ExportControl::default();
    let owner = control.require_source_checked_commit().unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let expire_after_cas = control.clone();
    *owner.fence.after_cas.lock().unwrap() = Some(Box::new(move || {
        // Expire the writer's actual deadline check under the request mutex,
        // then unwind before retiring/notifying. The global CAS already won.
        expire_after_cas.deadline_at(9900);
        panic!("injected host unwind after actual CAS and before notification");
    }));
    let worker = control.clone();
    let writer = thread::spawn(move || worker.begin_commit_before(deadline));
    wait_requested(&owner);
    assert!(std::panic::catch_unwind(AssertUnwindSafe(|| owner.commit_before(deadline))).is_err());
    drop(owner);
    writer
        .join()
        .expect("Actual committed writer joined")
        .unwrap();
    assert_eq!(control.0.state.load(Ordering::Acquire), COMMITTING);
    assert!(!control.cancel());
}
