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
};

const RUNNING: u8 = 0;
const CANCELLED: u8 = 1;
const COMMITTING: u8 = 2;
const COMPLETE: u8 = 3;

#[derive(Debug, thiserror::Error)]
#[error("WAV export cancelled; the destination was not changed")]
pub struct ExportCancelled;

#[derive(Default)]
struct ControlState {
    state: AtomicU8,
    progress: AtomicU32,
    #[cfg(test)]
    cancel_at: AtomicU32,
}

#[derive(Clone, Default)]
pub struct ExportControl(Arc<ControlState>);

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
            7500..9800 => "Encoding 24-bit WAV",
            9800..10000 => "Finalizing WAV",
            _ => "Export complete",
        }
    }
}

impl ExportControl {
    /// Returns false once publication has begun. The CAS is the cancellation
    /// boundary: an accepted cancellation can never replace the destination.
    pub fn cancel(&self) -> bool {
        match self
            .0
            .state
            .compare_exchange(RUNNING, CANCELLED, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) | Err(CANCELLED) => true,
            Err(_) => false,
        }
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

    pub(crate) fn complete(&self) {
        self.0.progress.store(10000, Ordering::Relaxed);
        self.0.state.store(COMPLETE, Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn cancel_at(&self, basis_points: u32) {
        self.0.cancel_at.store(basis_points, Ordering::Relaxed);
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ExportOutcome {
    Complete(PathBuf),
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
        job.start(7, "old.wav".into(), |_| {
            ExportOutcome::Complete("old.wav".into())
        })
        .unwrap();
        assert_eq!(poll_until_finished(&mut job, 8), None);
        job.start(8, "new.wav".into(), |_| {
            ExportOutcome::Complete("new.wav".into())
        })
        .unwrap();
        assert_eq!(
            poll_until_finished(&mut job, 8),
            Some(ExportOutcome::Complete("new.wav".into()))
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
            job.start(1, "retry.wav".into(), |_| ExportOutcome::Complete(
                "retry.wav".into()
            ))
            .unwrap()
        );
        assert_eq!(
            poll_until_finished(&mut job, 1),
            Some(ExportOutcome::Complete("retry.wav".into()))
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
