//! Experimental Linux-only, owner-thread fresh-child transport foundation.
//!
//! This crate-private module has no production caller. It does not load a plugin by
//! itself, retry a command, alter SIGPIPE disposition, or create an I/O thread.
//! The process-wide one-child reservation is deliberately restrictive: caller
//! integration and simultaneous fresh candidate replacement need a later design.

mod codec;
mod pump;
mod quarantine;
#[cfg(test)]
mod tests;

use crate::process_isolation::{HostCommand, HostResponse};
use std::marker::PhantomData;
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

const MAX_LINE: usize = 96 * 1024 * 1024;
const MAX_PAYLOAD: usize = 256 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ErrorKind {
    UnsupportedRequest,
    UnsupportedReply,
    InvalidRequest,
    MalformedReply,
    WrongReply,
    Limit,
    Shape,
    Deadline,
    Cancelled,
    Allocation,
    Io(i32),
    Eof,
    ChildExited,
    ZeroWrite,
    Unsolicited,
    Terminal,
    Busy,
    Sigpipe,
    Spawn,
    OperationRejected,
    ReportedCrash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Delivery {
    NotSent,
    MayHaveApplied,
}

#[derive(Debug)]
pub(crate) struct ExchangeFailure {
    pub(crate) cause: ErrorKind,
    pub(crate) delivery: Delivery,
    /// A valid helper operation error is retained; it never proves rollback.
    pub(crate) reported: Option<Box<HostResponse>>,
    pub(crate) diagnostic_unavailable: Option<ErrorKind>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReplyShape {
    PluginInfo,
    Success,
    State,
    ParameterValue,
    LatencySamples,
    TailSamples,
    Audio {
        channels: usize,
        frames: usize,
    },
    Buses {
        buses: Vec<(bool, usize)>,
        frames: usize,
    },
}

/// One shared FD owns its identity until the final cancellation handle is dropped.
/// A late cancel therefore cannot write to a recycled raw descriptor.
struct Cancellation {
    cancelled: AtomicBool,
    wake: std::os::fd::OwnedFd,
    #[cfg(test)]
    skip_wake: AtomicBool,
}

#[derive(Clone)]
pub(crate) struct CancelHandle(Arc<Cancellation>);

impl CancelHandle {
    pub(crate) fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Release);
        #[cfg(test)]
        if self.0.skip_wake.swap(false, Ordering::AcqRel) {
            return;
        }
        pump::wake(&self.0.wake);
    }
}

/// Counts foundation-owned variable payload, excluding allocator overhead and
/// caller buffers. Reservations are cooperative; allocation/scheduling are not
/// universal hard deadlines. Codec and pump check before returning success.
struct Budget {
    deadline: Instant,
    cancel: Arc<Cancellation>,
    live: usize,
    peak: usize,
    /// Only a fully validated known helper failure may use this bounded tail.
    diagnostic_copy: bool,
}

impl Budget {
    fn check(&self) -> Result<(), ErrorKind> {
        if self.diagnostic_copy {
            return Ok(());
        }
        #[cfg(test)]
        if budget_test_hooks::should_cancel() {
            self.cancel.cancelled.store(true, Ordering::Release);
        }
        #[cfg(test)]
        if budget_test_hooks::deadline_should_expire() {
            return Err(ErrorKind::Deadline);
        }
        if self.cancel.cancelled.load(Ordering::Acquire) {
            return Err(ErrorKind::Cancelled);
        }
        if Instant::now() >= self.deadline {
            return Err(ErrorKind::Deadline);
        }
        Ok(())
    }

    fn reserve(&mut self, bytes: usize) -> Result<(), ErrorKind> {
        self.check()?;
        let live = self.live.checked_add(bytes).ok_or(ErrorKind::Limit)?;
        if live > MAX_PAYLOAD {
            return Err(ErrorKind::Limit);
        }
        self.live = live;
        self.peak = self.peak.max(live);
        Ok(())
    }

    fn grow<T>(&mut self, vec: &mut Vec<T>, new_len: usize) -> Result<(), ErrorKind> {
        if new_len <= vec.capacity() {
            return self.check();
        }
        let size = std::mem::size_of::<T>();
        let old = vec.capacity();
        let minimum = new_len
            .checked_sub(old)
            .and_then(|n| n.checked_mul(size))
            .ok_or(ErrorKind::Limit)?;
        self.reserve(minimum)?;
        #[cfg(test)]
        if budget_test_hooks::allocation_should_fail() {
            self.release(minimum);
            return Err(ErrorKind::Allocation);
        }
        if vec.try_reserve_exact(new_len - vec.len()).is_err() {
            self.release(minimum);
            return Err(ErrorKind::Allocation);
        }
        let actual = vec
            .capacity()
            .checked_sub(old)
            .and_then(|n| n.checked_mul(size))
            .ok_or(ErrorKind::Limit)?;
        // try_reserve_exact may reserve more than requested. Account that actual
        // capacity too before admitting the result; allocator internals are not
        // controlled by this logical payload budget.
        self.reserve(actual.checked_sub(minimum).ok_or(ErrorKind::Limit)?)?;
        Ok(())
    }

    fn release(&mut self, bytes: usize) {
        self.live = self
            .live
            .checked_sub(bytes)
            .expect("balanced payload accounting");
    }
}

struct ChildOwner {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: Option<ChildStdout>,
    /// A kill attempt is evidence, not proof of exit or reap.
    kill_requested: bool,
    /// First pipe-close grace, never renewed by repeated short cleanup budgets.
    graceful_until: Option<Instant>,
    observed_exit: Option<ExitStatus>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ready,
    InFlight,
    Terminal,
    Reaped,
}

/// The only owner of one admitted child. No COM object is present here.
/// Its cleanup permit remains reserved until concrete reap, including on Drop.
#[must_use = "finish checked disposal; dropping retains the child in quarantine"]
pub(crate) struct FreshHelperSession {
    owner: Option<ChildOwner>,
    permit: Option<quarantine::Permit>,
    cancel: Arc<Cancellation>,
    state: State,
    terminal: Option<ErrorKind>,
    _owner_thread: PhantomData<Rc<()>>,
}

pub(crate) struct SpawnFailure {
    pub(crate) cause: ErrorKind,
    /// FD setup or post-spawn expiry leaves the real child in guarded ownership.
    pub(crate) cleanup: Option<Box<FreshHelperSession>>,
}

#[derive(Debug)]
pub(crate) enum CleanupState {
    Reaped(ExitStatus),
    Pending,
    /// No abandoned child currently exists; a live session can still hold the slot.
    Empty,
    Busy,
}

#[derive(Debug)]
#[must_use = "Pending or Busy still blocks a new strict child"]
pub(crate) struct CleanupReport {
    pub(crate) state: CleanupState,
    pub(crate) shutdown_error: Option<ErrorKind>,
    pub(crate) kill_error: Option<i32>,
    pub(crate) wait_error: Option<i32>,
    pub(crate) kill_requested: bool,
}

impl CleanupReport {
    fn pending() -> Self {
        Self {
            state: CleanupState::Pending,
            shutdown_error: None,
            kill_error: None,
            wait_error: None,
            kill_requested: false,
        }
    }
}

impl FreshHelperSession {
    pub(crate) fn spawn(command: &mut Command, deadline: Instant) -> Result<Self, SpawnFailure> {
        let failure = |cause| SpawnFailure {
            cause,
            cleanup: None,
        };
        pump::require_ignored_sigpipe().map_err(failure)?;
        if Instant::now() >= deadline {
            return Err(failure(ErrorKind::Deadline));
        }
        let cancel = Arc::new(Cancellation {
            cancelled: AtomicBool::new(false),
            wake: pump::new_wake().map_err(failure)?,
            #[cfg(test)]
            skip_wake: AtomicBool::new(false),
        });
        let permit = quarantine::reserve().map_err(failure)?;
        // This path owns every captured pipe. No stderr reader thread is needed.
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                drop(permit); // No Child was created; release the reservation.
                return Err(failure(pump::io_error(error)));
            }
        };
        // From this statement onward unwind always reaches guarded child ownership.
        // No fallible FD operation precedes construction of this owner.
        let mut session = Self {
            owner: Some(ChildOwner {
                child,
                stdin: None,
                stdout: None,
                kill_requested: false,
                graceful_until: None,
                observed_exit: None,
            }),
            permit: Some(permit),
            cancel,
            state: State::Terminal,
            terminal: None,
            _owner_thread: PhantomData,
        };
        let result = (|| {
            let owner = session.owner.as_mut().expect("new child owner");
            owner.stdin = owner.child.stdin.take();
            owner.stdout = owner.child.stdout.take();
            let stdin = owner.stdin.as_ref().ok_or(ErrorKind::Spawn)?;
            let stdout = owner.stdout.as_ref().ok_or(ErrorKind::Spawn)?;
            pump::set_nonblocking(stdin)?;
            pump::set_nonblocking(stdout)?;
            if Instant::now() >= deadline {
                return Err(ErrorKind::Deadline);
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                session.state = State::Ready;
                Ok(session)
            }
            Err(cause) => {
                session.terminal = Some(cause);
                Err(SpawnFailure {
                    cause,
                    cleanup: Some(Box::new(session)),
                })
            }
        }
    }

    pub(crate) fn cancellation(&self) -> CancelHandle {
        CancelHandle(self.cancel.clone())
    }

    pub(crate) fn exchange(
        &mut self,
        command: &HostCommand,
        expected: ReplyShape,
        deadline: Instant,
    ) -> Result<HostResponse, ExchangeFailure> {
        if self.state != State::Ready {
            return Err(ExchangeFailure {
                cause: self.terminal.unwrap_or(ErrorKind::Terminal),
                delivery: Delivery::NotSent,
                reported: None,
                diagnostic_unavailable: None,
            });
        }
        // InFlight is installed before any fallible/allocating codec work. A Rust
        // unwind cannot make a partially issued exchange reusable.
        self.state = State::InFlight;
        let mut delivery = Delivery::NotSent;
        let mut budget = Budget {
            deadline,
            cancel: self.cancel.clone(),
            live: 0,
            peak: 0,
            diagnostic_copy: false,
        };
        let result = (|| {
            budget.check()?;
            let request = codec::encode(command, &expected, &mut budget)?;
            let request_charge = request.capacity();
            let owner = self.owner.as_mut().ok_or(ErrorKind::Terminal)?;
            pump::ensure_no_reply(owner, &budget)?;
            pump::write_request(owner, &request, &budget, &mut delivery)?;
            // Never retain the encoded request during response/decode allocations.
            drop(request);
            budget.release(request_charge);
            let response = pump::read_reply(owner, &mut budget)?;
            let reply = codec::decode(&response.bytes, &expected, &mut budget)?;
            let child_exited = if matches!(reply, codec::DecodedReply::Response(_)) {
                let exited = pump::observe_child(owner)?;
                budget.check()?;
                exited
            } else {
                false
            };
            Ok((reply, response.peer_closed || child_exited, child_exited))
        })();
        match result {
            Ok((
                codec::DecodedReply::KnownFailure {
                    crashed,
                    reported,
                    diagnostic_unavailable,
                },
                _,
                _,
            )) => {
                let cause = if crashed {
                    ErrorKind::ReportedCrash
                } else {
                    ErrorKind::OperationRejected
                };
                self.state = State::Terminal;
                self.terminal = Some(cause);
                Err(ExchangeFailure {
                    cause,
                    delivery,
                    reported: reported.map(Box::new),
                    diagnostic_unavailable,
                })
            }
            Ok((codec::DecodedReply::Response(reply), true, child_exited)) => {
                let cause = if child_exited {
                    ErrorKind::ChildExited
                } else {
                    ErrorKind::Eof
                };
                self.state = State::Terminal;
                self.terminal = Some(cause);
                Err(ExchangeFailure {
                    cause,
                    delivery,
                    reported: Some(Box::new(reply)),
                    diagnostic_unavailable: None,
                })
            }
            Ok((codec::DecodedReply::Response(reply), false, _)) => {
                self.state = State::Ready;
                Ok(reply)
            }
            Err(cause) => {
                self.state = State::Terminal;
                self.terminal = Some(cause);
                Err(ExchangeFailure {
                    cause,
                    delivery,
                    reported: None,
                    diagnostic_unavailable: None,
                })
            }
        }
    }

    pub(crate) fn dispose_checked(&mut self, deadline: Instant) -> CleanupReport {
        let healthy = self.state == State::Ready;
        self.state = State::Terminal;
        let Some(owner) = self.owner.as_mut() else {
            let mut report = CleanupReport::pending();
            report.state = CleanupState::Empty;
            return report;
        };
        let mut report = CleanupReport::pending();
        if healthy {
            let budget = Budget {
                deadline,
                cancel: self.cancel.clone(),
                live: 0,
                peak: 0,
                diagnostic_copy: false,
            };
            let mut delivery = Delivery::NotSent;
            report.shutdown_error =
                pump::write_request(owner, b"\"Shutdown\"\n", &budget, &mut delivery).err();
        }
        // No protocol writes after this point, including later disposal attempts.
        owner.stdin.take();
        owner.stdout.take();
        // Pipe closure gets up to 100 ms within the original deadline for normal
        // main-thread teardown (healthy Shutdown or faulted EOF). A faulted
        // session never sends Shutdown. This grace budget is not a reap promise.
        let graceful_until = *owner
            .graceful_until
            .get_or_insert_with(|| Instant::now() + std::time::Duration::from_millis(100));
        pump::reap(owner, deadline, Some(graceful_until), &mut report);
        if matches!(report.state, CleanupState::Reaped(_)) {
            self.owner.take();
            self.permit.take(); // Concrete try_wait success is the release boundary.
            self.state = State::Reaped;
        }
        report
    }

    pub(crate) fn cleanup_abandoned(deadline: Instant) -> CleanupReport {
        quarantine::cleanup(deadline)
    }
}

impl Drop for FreshHelperSession {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.take() {
            let permit = self
                .permit
                .take()
                .expect("live child has reserved cleanup slot");
            quarantine::deposit(permit, owner);
        }
    }
}

#[cfg(test)]
mod budget_test_hooks {
    use std::cell::Cell;
    thread_local! {
        static REMAINING: Cell<Option<usize>> = const { Cell::new(None) };
        static CHECKS: Cell<usize> = const { Cell::new(0) };
        static FAIL_ALLOCATION: Cell<bool> = const { Cell::new(false) };
        static EXPIRE_REMAINING: Cell<Option<usize>> = const { Cell::new(None) };
        static EXPIRED: Cell<bool> = const { Cell::new(false) };
    }
    pub(super) struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            REMAINING.with(|r| r.set(None));
            FAIL_ALLOCATION.with(|f| f.set(false));
            EXPIRE_REMAINING.with(|r| r.set(None));
            EXPIRED.with(|e| e.set(false));
        }
    }
    pub(super) fn cancel_at_check(n: usize) -> Guard {
        assert!(n > 0);
        REMAINING.with(|r| {
            assert!(r.get().is_none());
            r.set(Some(n));
        });
        CHECKS.with(|n| n.set(0));
        Guard
    }
    pub(super) fn expire_at_check(n: usize) -> Guard {
        assert!(n > 0);
        EXPIRE_REMAINING.with(|r| {
            assert!(r.get().is_none());
            r.set(Some(n));
        });
        CHECKS.with(|n| n.set(0));
        Guard
    }
    pub(super) fn force_expired() {
        EXPIRED.with(|e| e.set(true));
    }
    pub(super) fn deadline_should_expire() -> bool {
        if EXPIRED.with(Cell::get) {
            return true;
        }
        EXPIRE_REMAINING.with(|r| match r.get() {
            Some(1) => {
                r.set(None);
                EXPIRED.with(|e| e.set(true));
                true
            }
            Some(n) => {
                r.set(Some(n - 1));
                false
            }
            None => false,
        })
    }
    pub(super) fn fail_next_allocation() -> Guard {
        FAIL_ALLOCATION.with(|f| {
            assert!(!f.get());
            f.set(true);
        });
        Guard
    }
    pub(super) fn allocation_should_fail() -> bool {
        FAIL_ALLOCATION.with(|f| f.replace(false))
    }
    pub(super) fn checks() -> usize {
        CHECKS.with(Cell::get)
    }
    pub(super) fn should_cancel() -> bool {
        CHECKS.with(|n| n.set(n.get() + 1));
        REMAINING.with(|r| match r.get() {
            Some(1) => {
                r.set(None);
                true
            }
            Some(n) => {
                r.set(Some(n - 1));
                false
            }
            None => false,
        })
    }
}
