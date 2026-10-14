//! Bounded, UI-independent state for a single cancellable offline export.
//!
//! Only the worker performs file I/O. Progress is a coalescing atomic snapshot,
//! and the result channel has one slot. A cancelled/stale job keeps ownership
//! until its worker exits, so repeated clicks cannot accumulate render threads.

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU8, AtomicU32, Ordering},
        mpsc::{self, Receiver},
    },
    time::Instant,
};

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
use std::sync::{Mutex, Weak};
#[cfg(all(
    test,
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
pub(crate) use strict_cancel_tests::StrictCancelPause;
#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
use vst3_host::strict_offline::CancelHandle;

const RUNNING: u8 = 0;
const CANCELLED: u8 = 1;
const COMMITTING: u8 = 2;
const COMPLETE: u8 = 3;

#[derive(Debug, thiserror::Error)]
#[error("WAV export cancelled; the destination was not changed")]
pub struct ExportCancelled;

#[derive(Debug, thiserror::Error)]
#[error("WAV export deadline elapsed before publication; the destination was not changed")]
pub(crate) struct ExportDeadlineExceeded;

#[derive(Default)]
struct ControlState {
    state: AtomicU8,
    progress: AtomicU32,
    #[cfg(all(
        any(target_os = "linux", windows),
        feature = "experimental-fresh-helper"
    ))]
    strict_cancel: Mutex<Option<RegisteredStrictCancel>>,
    #[cfg(all(
        test,
        any(target_os = "linux", windows),
        feature = "experimental-fresh-helper"
    ))]
    strict_cancel_pause: Mutex<Option<Arc<strict_cancel_tests::CancelPauseState>>>,
    #[cfg(test)]
    cancel_at: AtomicU32,
    #[cfg(test)]
    deadline_at: AtomicU32,
}

#[derive(Clone, Default)]
pub struct ExportControl(Arc<ControlState>);

/// Exactly one closed cancellation target, with no user callback or raw sender.
/// The probe exists only in unit tests; production stores the helper's opaque
/// cancellation-only authority.
#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
#[derive(Clone)]
enum StrictCancelWake {
    Helper(CancelHandle),
    #[cfg(test)]
    Probe(Arc<strict_cancel_tests::CancelProbe>),
}

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
impl StrictCancelWake {
    fn cancel(&self) {
        match self {
            Self::Helper(handle) => handle.cancel(),
            #[cfg(test)]
            Self::Probe(probe) => probe.cancel(),
        }
    }

    #[cfg(test)]
    fn after_install(&self) {
        if let Self::Probe(probe) = self {
            probe.after_install();
        }
    }
}

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
struct RegisteredStrictCancel {
    // Live guards retain this allocation, so its identity cannot be reused while
    // an old guard can still run. There is no wrapping integer generation.
    generation: Arc<()>,
    wake: StrictCancelWake,
}

/// Held by the strict backend through disposal. Replacing a registration is
/// allowed, but dropping an old guard cannot clear the replacement. This weak
/// owner reference also prevents a detached guard from retaining an export job.
#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
#[must_use = "keep the registration alive until the strict helper is disposed"]
pub(crate) struct StrictCancelRegistration {
    owner: Weak<ControlState>,
    generation: Arc<()>,
}

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
impl Drop for StrictCancelRegistration {
    fn drop(&mut self) {
        let Some(owner) = self.owner.upgrade() else {
            return;
        };
        let removed = {
            let mut registered = owner
                .strict_cancel
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if registered
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(&current.generation, &self.generation))
            {
                registered.take()
            } else {
                None
            }
        };
        // A final handle drop may close its wake FD. Do not do that under the
        // registration lock, just as cancellation never holds it across wake.
        drop(removed);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExportProgress {
    pub basis_points: u32,
    pub cancelling: bool,
    pub can_cancel: bool,
}

impl ExportProgress {
    pub fn phase(self) -> &'static str {
        if self.cancelling {
            return "Cancelling WAV export…";
        }
        match self.basis_points {
            0..1500 => "Preparing media",
            1500..5000 => "Rendering instruments",
            5000..6500 => "Mixing audio clips",
            6500..7500 => "Checking audio",
            7500..9800 => "Encoding WAV",
            9800..10000 => "Finalizing WAV",
            _ => "Export complete",
        }
    }
}

impl ExportControl {
    /// Returns false once publication has begun. The CAS is the cancellation
    /// boundary: an accepted cancellation can never replace the destination.
    pub fn cancel(&self) -> bool {
        let accepted = match self.0.state.compare_exchange(
            RUNNING,
            CANCELLED,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) | Err(CANCELLED) => true,
            Err(_) => false,
        };
        #[cfg(all(
            test,
            any(target_os = "linux", windows),
            feature = "experimental-fresh-helper"
        ))]
        if accepted {
            let pause = {
                self.0
                    .strict_cancel_pause
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .take()
            };
            if let Some(pause) = pause {
                pause.after_publication();
            }
        }
        #[cfg(all(
            any(target_os = "linux", windows),
            feature = "experimental-fresh-helper"
        ))]
        if accepted {
            let wake = {
                let registered = self
                    .0
                    .strict_cancel
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                registered.as_ref().map(|current| current.wake.clone())
            };
            // Sticky export cancellation is already published, and the lock has
            // been released before the helper's bounded nonblocking wake.
            if let Some(wake) = wake {
                wake.cancel();
            }
        }
        accepted
    }

    /// Test-only, one-shot pause of the first accepted cancellation after its
    /// canonical state publication and before taking the wake registration lock.
    /// Keep the returned guard inside the test's thread scope so unwinding
    /// releases the cancelling worker before the scope joins it.
    #[cfg(all(
        test,
        any(target_os = "linux", windows),
        feature = "experimental-fresh-helper"
    ))]
    pub(crate) fn pause_strict_cancel_before_wake(&self, deadline: Instant) -> StrictCancelPause {
        StrictCancelPause::arm(self, deadline)
    }

    /// Install the strict helper's wake before its first exchange. The one-slot
    /// registration replaces an older target; each guard unregisters only its
    /// own generation. Cancellation racing before installation is observed by
    /// the post-install reread, while later cancellation sees the installed wake.
    #[cfg(all(
        any(target_os = "linux", windows),
        feature = "experimental-fresh-helper"
    ))]
    pub(crate) fn register_strict_cancel(&self, handle: CancelHandle) -> StrictCancelRegistration {
        self.register_strict_cancel_wake(StrictCancelWake::Helper(handle))
    }

    #[cfg(all(
        any(target_os = "linux", windows),
        feature = "experimental-fresh-helper"
    ))]
    fn register_strict_cancel_wake(&self, wake: StrictCancelWake) -> StrictCancelRegistration {
        let generation = Arc::new(());
        let previous = {
            let mut registered = self
                .0
                .strict_cancel
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            registered.replace(RegisteredStrictCancel {
                generation: generation.clone(),
                wake: wake.clone(),
            })
        };
        // Release replaced wake ownership outside the registration lock too.
        drop(previous);
        let registration = StrictCancelRegistration {
            owner: Arc::downgrade(&self.0),
            generation,
        };
        #[cfg(test)]
        wake.after_install();
        if self.0.state.load(Ordering::Acquire) == CANCELLED {
            wake.cancel();
        }
        registration
    }

    pub fn check_cancelled(&self) -> Result<(), ExportCancelled> {
        if self.0.state.load(Ordering::Acquire) == CANCELLED {
            Err(ExportCancelled)
        } else {
            Ok(())
        }
    }

    pub fn progress(&self) -> ExportProgress {
        let state = self.0.state.load(Ordering::Acquire);
        ExportProgress {
            basis_points: self.0.progress.load(Ordering::Relaxed),
            cancelling: state == CANCELLED,
            can_cancel: state == RUNNING,
        }
    }

    /// Actual completed work only; never advances from elapsed wall time.
    pub(crate) fn checkpoint(&self, basis_points: u32) -> Result<(), ExportCancelled> {
        self.0
            .progress
            .fetch_max(basis_points.min(9999), Ordering::Relaxed);
        #[cfg(test)]
        {
            let cancel_at = self.0.cancel_at.load(Ordering::Relaxed);
            if cancel_at != 0 && basis_points >= cancel_at {
                self.cancel();
            }
        }
        self.check_cancelled()
    }

    pub(crate) fn work_progress(
        &self,
        completed: usize,
        total: usize,
        start: u32,
        end: u32,
    ) -> Result<(), ExportCancelled> {
        let fraction = if total == 0 {
            end - start
        } else {
            ((completed.min(total) as u128 * u128::from(end - start)) / total as u128) as u32
        };
        self.checkpoint(start + fraction)
    }

    pub(crate) fn begin_commit(&self) -> Result<(), ExportCancelled> {
        self.checkpoint(9900)?;
        self.0
            .state
            .compare_exchange(RUNNING, COMMITTING, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| ExportCancelled)?;
        Ok(())
    }

    /// Cooperative deadline for the checked publication path only. Filesystem
    /// operations can block; this does not interrupt an in-progress OS call.
    pub(crate) fn check_deadline(&self, deadline: Instant) -> anyhow::Result<()> {
        self.check_cancelled()?;
        let expired = Instant::now() >= deadline;
        #[cfg(test)]
        let expired = expired || {
            let at = self.0.deadline_at.load(Ordering::Relaxed);
            at != 0 && self.0.progress.load(Ordering::Relaxed) >= at
        };
        if expired {
            return Err(ExportDeadlineExceeded.into());
        }
        Ok(())
    }

    /// The deadline reaches the final cancellation/commit boundary. Once this
    /// CAS wins, publication may finish even if the OS rename outlasts the
    /// deadline; reporting a deadline error after publication would be false.
    pub(crate) fn begin_commit_before(&self, deadline: Instant) -> anyhow::Result<()> {
        self.checkpoint(9900)?;
        self.check_deadline(deadline)?;
        self.0
            .state
            .compare_exchange(RUNNING, COMMITTING, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| ExportCancelled)?;
        Ok(())
    }

    pub(crate) fn complete(&self) {
        self.0.progress.store(10000, Ordering::Relaxed);
        self.0.state.store(COMPLETE, Ordering::Release);
    }

    /// Deterministic clock-expiry seam for checked-writer integration fixtures.
    /// It changes only deadline checks, never the cancel/commit CAS itself.
    #[cfg(test)]
    pub(crate) fn deadline_at(&self, basis_points: u32) {
        self.0.deadline_at.store(basis_points, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub(crate) fn cancel_at(&self, basis_points: u32) {
        self.0.cancel_at.store(basis_points, Ordering::Relaxed);
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ExportOutcome {
    Complete {
        path: PathBuf,
        level_summary: String,
    },
    Cancelled,
    Failed(String),
}

struct RunningExport {
    project_session: u64,
    path: PathBuf,
    control: ExportControl,
    receiver: Receiver<ExportOutcome>,
}

#[derive(Default)]
pub struct ExportJob {
    running: Option<RunningExport>,
}

impl ExportJob {
    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    pub fn start(
        &mut self,
        project_session: u64,
        path: PathBuf,
        worker: impl FnOnce(ExportControl) -> ExportOutcome + Send + 'static,
    ) -> std::io::Result<bool> {
        if self.is_running() {
            return Ok(false);
        }
        let control = ExportControl::default();
        let worker_control = control.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("wav-export".into())
            .spawn(move || {
                let outcome = worker(worker_control);
                let _ = sender.send(outcome);
            })?;
        self.running = Some(RunningExport {
            project_session,
            path,
            control,
            receiver,
        });
        Ok(true)
    }

    pub fn cancel(&self) -> bool {
        self.running
            .as_ref()
            .is_some_and(|job| job.control.cancel())
    }

    pub fn status(&self) -> Option<(&std::path::Path, ExportProgress)> {
        self.running
            .as_ref()
            .map(|job| (job.path.as_path(), job.control.progress()))
    }

    /// Retire every result exactly once, but never notify the new project of
    /// an old project's completion or error.
    pub fn poll(&mut self, current_project_session: u64) -> Option<ExportOutcome> {
        let job = self.running.as_ref()?;
        let outcome = match job.receiver.try_recv() {
            Ok(outcome) => outcome,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => {
                ExportOutcome::Failed("Export worker stopped unexpectedly".into())
            }
        };
        let is_current = job.project_session == current_project_session;
        self.running = None;
        is_current.then_some(outcome)
    }
}

impl Drop for ExportJob {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(all(
    test,
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
mod strict_cancel_tests {
    use super::*;
    use std::{
        sync::Condvar,
        time::{Duration, Instant},
    };

    #[derive(Default)]
    struct Latch {
        released: Mutex<bool>,
        changed: Condvar,
    }

    impl Latch {
        fn release(&self) {
            *self
                .released
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
            self.changed.notify_all();
        }

        fn wait_until(&self, deadline: Instant) -> bool {
            let mut released = self
                .released
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            while !*released {
                let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                    return false;
                };
                let (value, _) = self
                    .changed
                    .wait_timeout(released, remaining)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                released = value;
            }
            true
        }
    }

    /// Declared inside the thread scope: release a paused worker before the
    /// scope joins it, including when a main-thread assertion unwinds.
    struct ReleaseOnDrop<'a>(&'a Latch);

    impl Drop for ReleaseOnDrop<'_> {
        fn drop(&mut self) {
            self.0.release();
        }
    }

    pub(super) struct CancelPauseState {
        published: Latch,
        resume: Latch,
        deadline: Instant,
    }

    impl CancelPauseState {
        pub(super) fn after_publication(&self) {
            self.published.release();
            assert!(
                self.resume.wait_until(self.deadline),
                "post-cancellation wake pause deadline elapsed"
            );
        }
    }

    /// Closed bounded fixture gate; no callback, sender, or cancellation write
    /// escapes. Its guard releases an active pause or removes an unused one.
    #[must_use = "keep this guard inside the scope owning the cancelling worker"]
    pub(crate) struct StrictCancelPause {
        owner: Weak<ControlState>,
        state: Arc<CancelPauseState>,
    }

    impl StrictCancelPause {
        pub(super) fn arm(control: &ExportControl, deadline: Instant) -> Self {
            let state = Arc::new(CancelPauseState {
                published: Latch::default(),
                resume: Latch::default(),
                deadline,
            });
            let previous = {
                control
                    .0
                    .strict_cancel_pause
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .replace(state.clone())
            };
            if let Some(previous) = previous {
                previous.resume.release();
            }
            Self {
                owner: Arc::downgrade(&control.0),
                state,
            }
        }

        pub(crate) fn wait_until_paused(&self) -> bool {
            self.state.published.wait_until(self.state.deadline)
        }

        pub(crate) fn release(&self) {
            self.state.resume.release();
        }
    }

    impl Drop for StrictCancelPause {
        fn drop(&mut self) {
            self.release();
            if let Some(owner) = self.owner.upgrade() {
                let mut current = owner
                    .strict_cancel_pause
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if current
                    .as_ref()
                    .is_some_and(|state| Arc::ptr_eq(state, &self.state))
                {
                    current.take();
                }
            }
        }
    }

    /// Closed, synchronous test target: checks ordering and lock release without
    /// constructing a child, exposing a callback, or requiring a real plugin.
    pub(super) struct CancelProbe {
        owner: Weak<ControlState>,
        calls: AtomicU32,
        installation_gate: Option<Arc<InstallationGate>>,
        check_wake_lock: bool,
    }

    struct InstallationGate {
        installed: Latch,
        resume: Latch,
        deadline: Instant,
    }

    impl CancelProbe {
        fn new(control: &ExportControl) -> Arc<Self> {
            Self::with_gate(control, None)
        }

        fn with_gate(
            control: &ExportControl,
            installation_gate: Option<Arc<InstallationGate>>,
        ) -> Arc<Self> {
            Arc::new(Self {
                owner: Arc::downgrade(&control.0),
                calls: AtomicU32::new(0),
                installation_gate,
                check_wake_lock: true,
            })
        }

        pub(super) fn cancel(&self) {
            let owner = self.owner.upgrade().expect("cancelling a live export");
            assert_eq!(owner.state.load(Ordering::Acquire), CANCELLED);
            if self.check_wake_lock {
                assert!(
                    owner.strict_cancel.try_lock().is_ok(),
                    "wake must run after releasing the registration lock"
                );
            }
            self.calls.fetch_add(1, Ordering::AcqRel);
        }

        pub(super) fn after_install(&self) {
            if let Some(gate) = &self.installation_gate {
                gate.installed.release();
                assert!(
                    gate.resume.wait_until(gate.deadline),
                    "registration resume deadline elapsed"
                );
            }
        }

        fn calls(&self) -> u32 {
            self.calls.load(Ordering::Acquire)
        }
    }

    impl Drop for CancelProbe {
        fn drop(&mut self) {
            if let Some(owner) = self.owner.upgrade() {
                assert!(
                    owner.strict_cancel.try_lock().is_ok(),
                    "wake ownership must be released outside the registration lock"
                );
            }
        }
    }

    fn register(control: &ExportControl, probe: &Arc<CancelProbe>) -> StrictCancelRegistration {
        control.register_strict_cancel_wake(StrictCancelWake::Probe(probe.clone()))
    }

    #[test]
    fn strict_registration_is_shared_by_all_control_clones() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ExportControl>();
        assert_send_sync::<CancelHandle>();

        let control = ExportControl::default();
        let before = control.clone();
        let registrar = before.clone();
        let probe = CancelProbe::new(&control);
        let _registration = register(&registrar, &probe);
        let after = control.clone();
        assert_eq!(probe.calls(), 0);
        for (index, clone) in [control, before, registrar, after].iter().enumerate() {
            assert!(clone.cancel(), "repeated cancellation remains accepted");
            assert!(clone.check_cancelled().is_err());
            assert_eq!(probe.calls(), index as u32 + 1);
        }
    }

    #[test]
    fn cancellation_before_registration_is_rechecked_before_returning() {
        let control = ExportControl::default();
        assert!(control.clone().cancel());
        let probe = CancelProbe::new(&control);
        let _registration = register(&control, &probe);
        assert_eq!(probe.calls(), 1);
        assert!(control.begin_commit().is_err());
    }

    #[test]
    fn real_cancel_can_pause_with_its_flag_visible_before_the_registered_wake() {
        let control = ExportControl::default();
        let probe = CancelProbe::new(&control);
        let _registration = register(&control, &probe);
        std::thread::scope(|scope| {
            let pause =
                control.pause_strict_cancel_before_wake(Instant::now() + Duration::from_secs(5));
            let worker = scope.spawn(|| control.cancel());
            assert!(
                pause.wait_until_paused(),
                "cancel did not reach its post-CAS pause"
            );
            assert!(control.check_cancelled().is_err());
            assert!(control.progress().cancelling);
            assert_eq!(probe.calls(), 0, "registered client wake is still delayed");
            assert!(control.begin_commit().is_err());
            pause.release();
            assert!(worker.join().unwrap());
            assert_eq!(probe.calls(), 1);
        });
    }

    #[test]
    fn dropping_an_unused_post_cancel_pause_disarms_it() {
        let control = ExportControl::default();
        let probe = CancelProbe::new(&control);
        let _registration = register(&control, &probe);
        let pause =
            control.pause_strict_cancel_before_wake(Instant::now() + Duration::from_secs(5));
        drop(pause);
        assert!(control.0.strict_cancel_pause.lock().unwrap().is_none());
        assert!(control.cancel());
        assert_eq!(probe.calls(), 1);
    }

    #[test]
    fn post_cancel_pause_releases_before_scoped_join_during_unwind() {
        let control = ExportControl::default();
        let probe = CancelProbe::new(&control);
        let _registration = register(&control, &probe);
        let result = std::panic::catch_unwind(|| {
            std::thread::scope(|scope| {
                let pause = control
                    .pause_strict_cancel_before_wake(Instant::now() + Duration::from_secs(5));
                scope.spawn(|| assert!(control.cancel()));
                assert!(pause.wait_until_paused());
                panic!("fixture failed before explicitly releasing the cancelling worker");
            });
        });
        assert!(result.is_err());
        assert_eq!(
            probe.calls(),
            1,
            "the released worker was joined during unwind"
        );
    }

    #[test]
    fn cancellation_between_install_and_reread_cannot_lose_the_wake() {
        let control = ExportControl::default();
        let gate = Arc::new(InstallationGate {
            installed: Latch::default(),
            resume: Latch::default(),
            deadline: Instant::now() + Duration::from_secs(5),
        });
        let probe = CancelProbe::with_gate(&control, Some(gate.clone()));
        std::thread::scope(|scope| {
            let _release_on_failure = ReleaseOnDrop(&gate.resume);
            let worker = scope.spawn(|| register(&control, &probe));

            // The actual registration path is paused after unlocking the
            // installed slot but before its reread. Both waits use the same
            // absolute deadline; panic before the resume releases it via Drop.
            assert!(
                gate.installed.wait_until(gate.deadline),
                "registration installation deadline elapsed"
            );
            assert!(control.cancel());
            assert_eq!(probe.calls(), 1);
            gate.resume.release();
            let _registration = worker.join().unwrap();
            assert_eq!(probe.calls(), 2, "both racers observe sticky cancellation");
        });
    }

    #[test]
    fn cancellation_is_published_while_registration_waits_for_the_slot() {
        let control = ExportControl::default();
        // These two contenders may legitimately hold the slot lock while the
        // other is waking. Other, phased tests check lock release without that
        // unrelated contention making a try_lock assertion scheduling-sensitive.
        let probe = Arc::new(CancelProbe {
            owner: Arc::downgrade(&control.0),
            calls: AtomicU32::new(0),
            installation_gate: None,
            check_wake_lock: false,
        });
        let entered = Latch::default();
        let deadline = Instant::now() + Duration::from_secs(5);
        std::thread::scope(|scope| {
            // This guard lives inside the scope, so an assertion or spawn
            // failure unlocks the slot before the scope unconditionally joins.
            let slot = control.0.strict_cancel.lock().unwrap();
            let registration_worker = scope.spawn(|| {
                entered.release();
                register(&control, &probe)
            });
            assert!(
                entered.wait_until(deadline),
                "registration worker did not enter before the deadline"
            );
            let cancellation_worker = scope.spawn(|| control.cancel());
            while control.check_cancelled().is_ok() && Instant::now() < deadline {
                std::thread::yield_now();
            }
            let published_before_unlock = control.check_cancelled().is_err();
            // Either contender may take the lock first. Release on timeout too,
            // then join both workers before asserting their observed outcomes.
            drop(slot);
            let registration_result = registration_worker.join();
            let cancellation_result = cancellation_worker.join();
            assert!(
                published_before_unlock,
                "cancel did not publish before locking"
            );
            let _registration = registration_result.unwrap();
            assert!(cancellation_result.unwrap());
        });
        assert!(probe.calls() >= 1);
        assert!(control.begin_commit().is_err());
    }

    #[test]
    fn stale_registration_guard_cannot_remove_a_replacement() {
        let control = ExportControl::default();
        let old_probe = CancelProbe::new(&control);
        let old_guard = register(&control, &old_probe);
        let new_probe = CancelProbe::new(&control);
        let new_guard = register(&control.clone(), &new_probe);
        drop(old_guard);
        assert!(control.cancel());
        assert_eq!(old_probe.calls(), 0);
        assert_eq!(new_probe.calls(), 1);
        drop(new_guard);
        assert!(control.0.strict_cancel.lock().unwrap().is_none());
        assert!(control.cancel());
        assert_eq!(
            new_probe.calls(),
            1,
            "unregistered targets are not awakened"
        );
    }

    #[test]
    fn replacement_on_an_already_cancelled_job_is_also_awakened() {
        let control = ExportControl::default();
        let old_probe = CancelProbe::new(&control);
        let old_guard = register(&control, &old_probe);
        assert!(control.cancel());
        let new_probe = CancelProbe::new(&control);
        let _new_guard = register(&control, &new_probe);
        assert_eq!(new_probe.calls(), 1);
        drop(old_guard);
        assert!(control.cancel());
        assert_eq!(old_probe.calls(), 1);
        assert_eq!(new_probe.calls(), 2);
    }

    #[test]
    fn guards_are_job_bound_and_do_not_retain_the_control_or_wake() {
        let old_control = ExportControl::default();
        let old_probe = CancelProbe::new(&old_control);
        let old_weak = Arc::downgrade(&old_probe);
        let old_guard = register(&old_control, &old_probe);
        let old_state = Arc::downgrade(&old_control.0);
        drop(old_probe);
        drop(old_control);
        assert!(old_state.upgrade().is_none());
        assert!(old_weak.upgrade().is_none());

        let new_control = ExportControl::default();
        let new_probe = CancelProbe::new(&new_control);
        let _new_guard = register(&new_control, &new_probe);
        drop(old_guard);
        assert!(new_control.cancel());
        assert_eq!(new_probe.calls(), 1);
    }

    #[test]
    fn unregistration_and_replacement_release_wake_ownership_outside_the_lock() {
        let control = ExportControl::default();
        let old_probe = CancelProbe::new(&control);
        let old_weak = Arc::downgrade(&old_probe);
        let old_guard = register(&control, &old_probe);
        drop(old_probe);
        let new_probe = CancelProbe::new(&control);
        let new_weak = Arc::downgrade(&new_probe);
        let new_guard = register(&control, &new_probe);
        assert!(
            old_weak.upgrade().is_none(),
            "replacement released the old wake"
        );
        drop(old_guard);
        drop(new_probe);
        assert!(new_weak.upgrade().is_some());
        drop(new_guard);
        assert!(
            new_weak.upgrade().is_none(),
            "guard released the registered wake"
        );
        assert!(control.0.strict_cancel.lock().unwrap().is_none());
    }

    #[test]
    fn unwinding_the_owner_unregisters_its_wake() {
        let control = ExportControl::default();
        let probe = CancelProbe::new(&control);
        let result = std::panic::catch_unwind(|| {
            let _guard = register(&control, &probe);
            panic!("strict owner unwound");
        });
        assert!(result.is_err());
        assert!(control.0.strict_cancel.lock().unwrap().is_none());
        assert!(control.cancel());
        assert_eq!(probe.calls(), 0);
    }

    fn assert_job_route_wakes(drop_job: bool) {
        // These routes need the actual job cancellation methods, but no worker
        // execution. A private running record avoids a detached production
        // thread and leaves nothing waiting if any assertion below unwinds.
        let control = ExportControl::default();
        let (_sender, receiver) = mpsc::sync_channel(1);
        let job = ExportJob {
            running: Some(RunningExport {
                project_session: 1,
                path: "strict.wav".into(),
                control: control.clone(),
                receiver,
            }),
        };
        let probe = CancelProbe::new(&control);
        let _registration = register(&control, &probe);
        if drop_job {
            drop(job);
        } else {
            assert!(job.cancel());
            assert_eq!(probe.calls(), 1);
            drop(job);
        }
        assert_eq!(probe.calls(), if drop_job { 1 } else { 2 });
        assert!(control.check_cancelled().is_err());
    }

    #[test]
    fn explicit_export_job_cancel_wakes_the_shared_registration() {
        assert_job_route_wakes(false);
    }

    #[test]
    fn dropping_export_job_wakes_the_shared_registration() {
        assert_job_route_wakes(true);
    }

    #[test]
    fn checkpoint_cancellation_uses_the_same_registration() {
        let control = ExportControl::default();
        let probe = CancelProbe::new(&control);
        let _registration = register(&control, &probe);
        control.cancel_at(8000);
        assert!(control.clone().checkpoint(8000).is_err());
        assert_eq!(probe.calls(), 1);
    }

    #[test]
    fn accepted_registered_cancel_closes_both_publication_fences() {
        let control = ExportControl::default();
        let probe = CancelProbe::new(&control);
        let _registration = register(&control, &probe);
        assert!(control.clone().cancel());
        assert!(control.begin_commit().is_err());
        assert!(
            control
                .begin_commit_before(Instant::now() + Duration::from_secs(60))
                .unwrap_err()
                .is::<ExportCancelled>()
        );
        assert_eq!(probe.calls(), 1);
        assert!(control.progress().cancelling);
        assert_ne!(control.progress().basis_points, 10000);
    }

    #[test]
    fn cancellation_rejected_after_commit_never_wakes_the_helper() {
        let control = ExportControl::default();
        let probe = CancelProbe::new(&control);
        let _registration = register(&control, &probe);
        control.begin_commit().unwrap();
        assert!(!control.clone().cancel());
        control.complete();
        assert!(!control.cancel());
        assert_eq!(probe.calls(), 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn poll_until_finished(job: &mut ExportJob, session: u64) -> Option<ExportOutcome> {
        let deadline = Instant::now() + Duration::from_secs(5);
        while job.is_running() {
            if let Some(result) = job.poll(session) {
                return Some(result);
            }
            assert!(Instant::now() < deadline, "worker did not finish");
            std::thread::yield_now();
        }
        None
    }

    #[test]
    fn progress_is_monotonic_bounded_and_complete_only_after_commit() {
        let control = ExportControl::default();
        control.checkpoint(2000).unwrap();
        control.checkpoint(1000).unwrap();
        assert_eq!(control.progress().basis_points, 2000);
        control
            .work_progress(usize::MAX, usize::MAX, 2000, 5000)
            .unwrap();
        assert_eq!(control.progress().basis_points, 5000);
        control.checkpoint(u32::MAX).unwrap();
        assert_eq!(control.progress().basis_points, 9999);
        control.begin_commit().unwrap();
        assert!(!control.cancel());
        control.complete();
        assert_eq!(control.progress().basis_points, 10000);
        assert!(!control.progress().can_cancel);
    }

    #[test]
    fn accepted_cancellation_prevents_commit_and_is_idempotent() {
        let control = ExportControl::default();
        assert!(control.cancel());
        assert!(control.cancel());
        assert!(control.begin_commit().is_err());
        assert!(control.progress().cancelling);
        assert!(!control.progress().can_cancel);
    }

    #[test]
    fn checked_commit_has_success_expiry_and_cancellation_controls() {
        let deadline = Instant::now() + Duration::from_secs(60);
        let success = ExportControl::default();
        success.begin_commit_before(deadline).unwrap();
        assert_eq!(success.progress().basis_points, 9900);
        assert!(!success.cancel());
        success.complete();
        assert_eq!(success.progress().basis_points, 10000);

        let expired = ExportControl::default();
        let error = expired.begin_commit_before(Instant::now()).unwrap_err();
        assert!(error.is::<ExportDeadlineExceeded>());
        assert_eq!(expired.progress().basis_points, 9900);
        assert!(expired.cancel(), "expiry must not enter COMMITTING");

        let cancelled = ExportControl::default();
        cancelled.cancel_at(9900);
        let error = cancelled.begin_commit_before(deadline).unwrap_err();
        assert!(error.is::<ExportCancelled>());
        assert!(cancelled.progress().cancelling);
    }

    #[test]
    fn repeated_clicks_and_cancellation_keep_one_worker_until_it_finishes() {
        let mut job = ExportJob::default();
        let (release, wait) = mpsc::sync_channel(1);
        assert!(
            job.start(1, "first.wav".into(), move |control| {
                wait.recv().unwrap();
                assert!(control.check_cancelled().is_err());
                ExportOutcome::Cancelled
            })
            .unwrap()
        );
        assert!(
            !job.start(1, "second.wav".into(), |_| panic!("duplicate worker"))
                .unwrap()
        );
        assert!(job.cancel());
        assert!(job.is_running());
        assert!(
            !job.start(2, "new.wav".into(), |_| panic!(
                "cancelled worker still owns slot"
            ))
            .unwrap()
        );
        release.send(()).unwrap();
        assert_eq!(
            poll_until_finished(&mut job, 1),
            Some(ExportOutcome::Cancelled)
        );
        assert_eq!(job.poll(1), None);
        assert!(!job.is_running());
    }

    #[test]
    fn stale_completion_after_new_project_does_not_leak_or_clear_new_job() {
        let mut job = ExportJob::default();
        job.start(7, "old.wav".into(), |_| ExportOutcome::Complete {
            path: "old.wav".into(),
            level_summary: String::new(),
        })
        .unwrap();
        assert_eq!(poll_until_finished(&mut job, 8), None);
        job.start(8, "new.wav".into(), |_| ExportOutcome::Complete {
            path: "new.wav".into(),
            level_summary: String::new(),
        })
        .unwrap();
        assert_eq!(
            poll_until_finished(&mut job, 8),
            Some(ExportOutcome::Complete {
                path: "new.wav".into(),
                level_summary: String::new()
            })
        );
    }

    #[test]
    fn commit_failure_retires_job_without_claiming_cancellation_or_success() {
        let mut job = ExportJob::default();
        let (committed, gate) = mpsc::sync_channel(1);
        let (release, wait) = mpsc::sync_channel(1);
        job.start(1, "failed-commit.wav".into(), move |control| {
            control.begin_commit().unwrap();
            committed.send(()).unwrap();
            wait.recv().unwrap();
            ExportOutcome::Failed("Unable to commit staged WAV".into())
        })
        .unwrap();
        gate.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(!job.cancel());
        assert_eq!(job.status().unwrap().1.basis_points, 9900);
        release.send(()).unwrap();
        assert!(matches!(
            poll_until_finished(&mut job, 1),
            Some(ExportOutcome::Failed(_))
        ));
        assert!(!job.is_running());
        assert!(
            job.start(1, "retry.wav".into(), |_| ExportOutcome::Complete {
                path: "retry.wav".into(),
                level_summary: String::new()
            })
            .unwrap()
        );
        assert_eq!(
            poll_until_finished(&mut job, 1),
            Some(ExportOutcome::Complete {
                path: "retry.wav".into(),
                level_summary: String::new()
            })
        );
    }

    #[test]
    fn stale_error_and_worker_disconnect_release_the_slot() {
        let mut job = ExportJob::default();
        job.start(1, "failed.wav".into(), |_| {
            ExportOutcome::Failed("old failure".into())
        })
        .unwrap();
        assert_eq!(poll_until_finished(&mut job, 2), None);
        job.start(2, "panic.wav".into(), |_| panic!("test worker panic"))
            .unwrap();
        assert!(matches!(
            poll_until_finished(&mut job, 2),
            Some(ExportOutcome::Failed(_))
        ));
        assert!(!job.is_running());
    }
}
