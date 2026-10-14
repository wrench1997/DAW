//! Shared retained-child teardown: pipe completion and concrete reap are both required.

use super::{pump, ChildOwner, CleanupState, ErrorKind};
use std::time::Instant;

/// Preserve the direct-child status even if a descendant keeps its pipe open.
pub(super) fn observe_child(owner: &mut ChildOwner) -> Result<bool, ErrorKind> {
    if owner.observed_exit.is_some() {
        return Ok(true);
    }
    match owner.child.try_wait() {
        Ok(Some(status)) => {
            owner.observed_exit = Some(status);
            Ok(true)
        }
        Ok(None) => Ok(false),
        Err(error) => Err(pump::io_error(error)),
    }
}

/// Returns whether observation may continue. All evidence is kept on the same
/// child even if a subsequent Rust test hook unwinds or Drop quarantines it.
fn cleanup_observation(
    owner: &mut ChildOwner,
    graceful_until: Option<Instant>,
    kill_deadline: Option<Instant>,
) -> bool {
    let pipes_closed = pump::close_pipes(owner);
    owner.cleanup_report.kill_requested = owner.kill_requested;
    if let Some(status) = owner.observed_exit {
        if pipes_closed {
            owner.cleanup_report.state = CleanupState::Reaped(status);
            return false;
        }
        return true;
    }
    #[cfg(test)]
    if pump::test_hooks::take(pump::test_hooks::Fault::ReapPanic) {
        panic!("Rust cleanup observation hook");
    }
    #[cfg(test)]
    if pump::test_hooks::take(pump::test_hooks::Fault::Wait) {
        owner.cleanup_report.wait_error.get_or_insert(libc::EIO);
        return false;
    }
    match owner.child.try_wait() {
        Ok(Some(status)) => {
            owner.observed_exit = Some(status);
            if pipes_closed {
                owner.cleanup_report.state = CleanupState::Reaped(status);
                return false;
            }
            return true;
        }
        Ok(None) => {}
        Err(error) => {
            owner
                .cleanup_report
                .wait_error
                .get_or_insert(error.raw_os_error().unwrap_or(libc::EIO));
            return false;
        }
    }
    if !owner.kill_requested
        && kill_deadline.is_none_or(|limit| Instant::now() < limit)
        && graceful_until.is_none_or(|until| Instant::now() >= until)
    {
        #[cfg(test)]
        if pump::test_hooks::take(pump::test_hooks::Fault::Kill) {
            owner.cleanup_report.kill_error.get_or_insert(libc::EPERM);
            return false;
        }
        match owner.child.kill() {
            Ok(()) => {
                owner.kill_requested = true;
                owner.cleanup_report.kill_requested = true;
            }
            Err(error) => {
                owner
                    .cleanup_report
                    .kill_error
                    .get_or_insert(error.raw_os_error().unwrap_or(libc::EIO));
                return false;
            }
        }
    }
    true
}

/// One observation and at most one eligible kill. A kill is never a reap result.
pub(super) fn reap_once(owner: &mut ChildOwner, graceful_until: Option<Instant>) {
    cleanup_observation(owner, graceful_until, None);
}

pub(super) fn reap(owner: &mut ChildOwner, deadline: Instant, graceful_until: Option<Instant>) {
    loop {
        if !cleanup_observation(owner, graceful_until, Some(deadline)) || Instant::now() >= deadline
        {
            return;
        }
        // Original deadline only; no background reaper or blocking Child::wait.
        #[cfg(test)]
        if owner.kill_requested && pump::test_hooks::take(pump::test_hooks::Fault::PollAfterKill) {
            owner.cleanup_report.wait_error.get_or_insert(libc::EIO);
            return;
        }
        if let Err(ErrorKind::Io(code)) = pump::wait_cleanup(owner, deadline) {
            owner.cleanup_report.wait_error.get_or_insert(code);
            return;
        }
    }
}
