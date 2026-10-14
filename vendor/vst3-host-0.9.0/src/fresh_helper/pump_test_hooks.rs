use std::cell::Cell;
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fault {
    Setup,
    ProbeInterrupted,
    WriteZero,
    WriteZeroAfterPrefix,
    WriteShortThenZero,
    ExpireBlockedWrite,
    WriteInterrupted,
    ReapPanic,
    Wait,
    Kill,
    PollAfterKill,
    ReadWaitForHup,
}
thread_local! { static NEXT: Cell<Option<Fault>> = const { Cell::new(None) }; }
pub(crate) fn arm(fault: Fault) {
    NEXT.with(|next| {
        assert!(next.get().is_none());
        next.set(Some(fault));
    });
}
pub(crate) fn take(fault: Fault) -> bool {
    NEXT.with(|next| {
        if next.get() == Some(fault) {
            next.set(None);
            true
        } else {
            false
        }
    })
}
thread_local! {
    static WAKE_LEFT: Cell<usize> = const { Cell::new(0) };
    static WAKE_COUNT: Cell<usize> = const { Cell::new(0) };
    static BLOCKED_WRITE: Cell<bool> = const { Cell::new(false) };
    static PIPE_COMPLETION_HOLD: Cell<bool> = const { Cell::new(false) };
}
pub(crate) fn hold_pipe_completion(hold: bool) {
    PIPE_COMPLETION_HOLD.with(|value| value.set(hold));
}
pub(crate) fn pipe_completion_held() -> bool {
    PIPE_COMPLETION_HOLD.with(Cell::get)
}
pub(crate) fn record_blocked_write() {
    BLOCKED_WRITE.with(|b| b.set(true));
}
pub(crate) fn blocked_write_observed() -> bool {
    BLOCKED_WRITE.with(Cell::get)
}
pub(crate) fn arm_wake_interrupts(count: usize) {
    WAKE_LEFT.with(|v| v.set(count));
    WAKE_COUNT.with(|v| v.set(0));
}
pub(crate) fn wake_attempts() -> usize {
    WAKE_COUNT.with(Cell::get)
}
pub(crate) fn interrupted_wake() -> bool {
    WAKE_COUNT.with(|v| v.set(v.get() + 1));
    WAKE_LEFT.with(|v| {
        if v.get() > 0 {
            v.set(v.get() - 1);
            true
        } else {
            false
        }
    })
}
pub(crate) fn clear() {
    NEXT.with(|next| next.set(None));
    WAKE_LEFT.with(|v| v.set(0));
    BLOCKED_WRITE.with(|b| b.set(false));
    PIPE_COMPLETION_HOLD.with(|value| value.set(false));
}
