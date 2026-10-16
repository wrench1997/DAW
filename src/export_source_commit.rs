//! One App-owned source check at the existing checked writer's final CAS.
//! The writer requests only after staging/sync; no cached Ready grant is sent.
use super::*;
use std::sync::{Condvar, MutexGuard};

#[cfg(test)]
type AfterCas = Box<dyn FnOnce() + Send>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Request {
    Idle,
    Requested,
    Retired,
}

pub(super) struct SourceCommitFence {
    request: Mutex<Request>,
    wake: Condvar,
    #[cfg(test)]
    panic_after_request: std::sync::atomic::AtomicBool,
    #[cfg(test)]
    after_cas: Mutex<Option<AfterCas>>,
}

/// The sole host-side owner. Dropping it cancels an uncommitted writer without
/// requiring another App tick. It owns no source, callback endpoint or sender.
pub(crate) struct SourceCommitOwner {
    control: ExportControl,
    fence: Arc<SourceCommitFence>,
}

impl ExportControl {
    /// Installed only by serial Whole's App owner, before its renderer starts.
    /// Direct SDK/Instrument/Occurrence callers keep the original None path.
    #[cfg(any(
        test,
        all(
            any(target_os = "linux", windows),
            feature = "experimental-fresh-helper"
        )
    ))]
    pub(crate) fn require_source_checked_commit(&self) -> anyhow::Result<SourceCommitOwner> {
        self.check_cancelled()?;
        anyhow::ensure!(
            self.0.state.load(Ordering::Acquire) == RUNNING,
            "Source verification cannot be installed after publication"
        );
        let mut installed = self
            .0
            .source_commit
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        anyhow::ensure!(
            installed.is_none(),
            "Source commit owner is already installed"
        );
        let fence = Arc::new(SourceCommitFence {
            request: Mutex::new(Request::Idle),
            wake: Condvar::new(),
            #[cfg(test)]
            panic_after_request: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            after_cas: Mutex::new(None),
        });
        *installed = Some(Arc::clone(&fence));
        Ok(SourceCommitOwner {
            control: self.clone(),
            fence,
        })
    }
}

impl SourceCommitOwner {
    pub(crate) fn requested(&self) -> bool {
        *self.fence.request.lock().unwrap_or_else(|p| p.into_inner()) == Request::Requested
    }

    /// The App calls this immediately after all original source/graph checks
    /// are Ready. The original global CAS, not this request state, is truth.
    pub(crate) fn commit_before(&self, deadline: Instant) -> anyhow::Result<bool> {
        let mut request = self.fence.request.lock().unwrap_or_else(|p| p.into_inner());
        if *request != Request::Requested {
            return Ok(false);
        }
        let result = self.control.begin_commit_before_unfenced(deadline);
        #[cfg(test)]
        if result.is_ok()
            && let Some(after_cas) = self.fence.after_cas.lock().unwrap().take()
        {
            after_cas();
        }
        *request = Request::Retired;
        self.fence.wake.notify_all();
        result.map(|()| true)
    }
}

impl Drop for SourceCommitOwner {
    fn drop(&mut self) {
        // CAS already won => cancel returns false. In particular, a host unwind
        // after CAS cannot turn the writer's actual COMMITTING into cancellation.
        self.control.cancel();
        self.fence.retire();
    }
}

impl SourceCommitFence {
    fn retire(&self) {
        *self.request.lock().unwrap_or_else(|p| p.into_inner()) = Request::Retired;
        self.wake.notify_all();
    }

    fn request_and_wait(&self, control: &ExportControl, deadline: Instant) -> anyhow::Result<()> {
        let mut request = self.request.lock().unwrap_or_else(|p| p.into_inner());
        anyhow::ensure!(
            *request == Request::Idle,
            "Source commit request is retired or reused"
        );
        control.check_deadline(deadline)?;
        *request = Request::Requested;
        // This owner retires BEFORE unlocking on success, failure and unwind.
        // A Drop that unlocked and reacquired would allow a late App CAS in
        // that gap after the writer had already abandoned publication.
        let mut retirement = RetireRequest {
            fence: self,
            guard: Some(request),
        };
        #[cfg(test)]
        assert!(
            !self.panic_after_request.load(Ordering::Acquire),
            "injected writer unwind after source commit request"
        );
        loop {
            // This read precedes cancellation/deadline/request retirement. If
            // the App won CAS but dropped/unwound before notify, finish rename.
            if matches!(
                control.0.state.load(Ordering::Acquire),
                COMMITTING | COMPLETE
            ) {
                return Ok(());
            }
            control.check_deadline(deadline)?;
            anyhow::ensure!(
                retirement.current() == Request::Requested,
                "Source commit owner was retired"
            );
            // Notifications accelerate cancellation/approval. A bounded reread
            // also observes the authoritative CAS if the host unwinds before
            // notify. This is not a source-readiness timer or a renewed budget.
            let wait = deadline
                .saturating_duration_since(Instant::now())
                .min(std::time::Duration::from_millis(20));
            retirement.wait(wait);
        }
    }
}

struct RetireRequest<'a> {
    fence: &'a SourceCommitFence,
    guard: Option<MutexGuard<'a, Request>>,
}
impl RetireRequest<'_> {
    fn current(&self) -> Request {
        **self.guard.as_ref().expect("Request mutex remains owned")
    }
    fn wait(&mut self, timeout: std::time::Duration) {
        // This Condvar is used with exactly this one mutex, and the bounded
        // timeout cannot overflow. Recover a poisoned guard rather than unwind
        // between the standard library's unlock and reacquire.
        let guard = self.guard.take().expect("Request mutex remains owned");
        let (guard, _) = self
            .fence
            .wake
            .wait_timeout(guard, timeout)
            .unwrap_or_else(|p| p.into_inner());
        self.guard = Some(guard);
    }
}
impl Drop for RetireRequest<'_> {
    fn drop(&mut self) {
        let request = self
            .guard
            .as_mut()
            .expect("Request mutex remains owned through retirement");
        **request = Request::Retired;
        self.fence.wake.notify_all();
    }
}

pub(super) fn wake(control: &ExportControl) {
    let fence = control
        .0
        .source_commit
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone();
    if let Some(fence) = fence {
        fence.wake.notify_all();
    }
}

pub(super) fn begin_or_defer(control: &ExportControl, deadline: Instant) -> anyhow::Result<()> {
    let fence = control
        .0
        .source_commit
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone();
    match fence {
        Some(fence) => {
            control.checkpoint(9900)?;
            fence.request_and_wait(control, deadline)
        }
        None => control.begin_commit_before_unfenced(deadline),
    }
}

#[cfg(test)]
#[path = "export_source_commit_tests.rs"]
mod tests;
