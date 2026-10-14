use super::{Budget, ChildOwner, Delivery, ErrorKind, MAX_LINE};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::Instant;

pub(super) fn io_error(error: io::Error) -> ErrorKind {
    ErrorKind::Io(error.raw_os_error().unwrap_or(libc::EIO))
}
fn last_error() -> ErrorKind {
    io_error(io::Error::last_os_error())
}
fn interrupted() -> bool {
    io::Error::last_os_error().kind() == io::ErrorKind::Interrupted
}
fn would_block() -> bool {
    io::Error::last_os_error().kind() == io::ErrorKind::WouldBlock
}

pub(super) fn require_ignored_sigpipe() -> Result<(), ErrorKind> {
    let mut disposition = std::mem::MaybeUninit::<libc::sigaction>::uninit();
    // SAFETY: a null new-action pointer makes this read-only; old-action points
    // to correctly sized writable storage and is read only on syscall success.
    if unsafe { libc::sigaction(libc::SIGPIPE, std::ptr::null(), disposition.as_mut_ptr()) } != 0 {
        return Err(last_error());
    }
    // SAFETY: successful sigaction initialized the full old-action structure.
    if unsafe { disposition.assume_init() }.sa_sigaction != libc::SIG_IGN {
        return Err(ErrorKind::Sigpipe);
    }
    Ok(())
}

pub(super) fn new_wake() -> Result<OwnedFd, ErrorKind> {
    // SAFETY: eventfd has no pointer arguments. Atomic flags prevent descriptor
    // inheritance and blocking wake writes from its first observable instant.
    let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
    if fd < 0 {
        return Err(last_error());
    }
    // SAFETY: this call returned a new exclusively owned descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

pub(super) fn wake(fd: &OwnedFd) {
    let one = 1u64.to_ne_bytes();
    // EAGAIN means the counter is already readable. EINTR retries preserve the
    // sticky cancellation; poll also has a 25 ms fallback recheck interval.
    for _ in 0..4 {
        #[cfg(test)]
        if test_hooks::interrupted_wake() {
            continue;
        }
        // SAFETY: borrowed descriptor is live and the byte slice is valid.
        let result = unsafe { libc::write(fd.as_raw_fd(), one.as_ptr().cast(), one.len()) };
        if result >= 0 || !interrupted() {
            return;
        }
    }
}

pub(super) fn set_nonblocking(fd: &impl AsRawFd) -> Result<(), ErrorKind> {
    #[cfg(test)]
    if test_hooks::take(test_hooks::Fault::Setup) {
        return Err(ErrorKind::Io(libc::EIO));
    }
    // SAFETY: descriptor remains owned by the session; F_GETFL has no third arg.
    let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
    if flags < 0 {
        return Err(last_error());
    }
    // SAFETY: preserves all existing flags and only adds nonblocking behavior.
    if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(last_error());
    }
    Ok(())
}

fn timeout(deadline: Instant) -> i32 {
    let remaining = deadline.saturating_duration_since(Instant::now());
    remaining
        .as_millis()
        .saturating_add(u128::from(remaining.subsec_nanos() % 1_000_000 != 0))
        .min(i32::MAX as u128) as i32
}

fn poll_fds(
    fds: &mut [libc::pollfd],
    budget: &Budget,
    owner: &mut ChildOwner,
) -> Result<(), ErrorKind> {
    loop {
        budget.check()?;
        // SAFETY: slice supplies valid pollfd storage for the exact count;
        // descriptors remain borrowed from their owners for this synchronous call.
        let result = unsafe {
            libc::poll(
                fds.as_mut_ptr(),
                fds.len() as libc::nfds_t,
                timeout(budget.deadline).min(25),
            )
        };
        if result < 0 {
            if interrupted() {
                continue;
            }
            return Err(last_error());
        }
        if result == 0 {
            if observe_child(owner)? {
                return Err(ErrorKind::ChildExited);
            }
            budget.check()?;
            continue;
        }
        // Do not preempt observed descriptor errors with a later cancellation;
        // caller inspects readiness/error bits, then checks the budget again.
        return Ok(());
    }
}

fn probe_reply(owner: &ChildOwner, budget: &Budget) -> Result<bool, ErrorKind> {
    let fd = owner
        .stdout
        .as_ref()
        .ok_or(ErrorKind::Terminal)?
        .as_raw_fd();
    let mut byte = [0u8; 1];
    loop {
        budget.check()?;
        #[cfg(test)]
        if test_hooks::take(test_hooks::Fault::ProbeInterrupted) {
            continue;
        }
        // SAFETY: valid owned nonblocking descriptor and one-byte storage.
        let n = unsafe { libc::read(fd, byte.as_mut_ptr().cast(), byte.len()) };
        if n > 0 {
            return Err(ErrorKind::Unsolicited);
        }
        if n == 0 {
            return Ok(true);
        }
        if interrupted() {
            continue;
        }
        if would_block() {
            return Ok(false);
        }
        return Err(last_error());
    }
}

pub(super) fn ensure_no_reply(owner: &ChildOwner, budget: &Budget) -> Result<(), ErrorKind> {
    if probe_reply(owner, budget)? {
        return Err(ErrorKind::Eof);
    }
    Ok(())
}

pub(super) fn write_request(
    owner: &mut ChildOwner,
    bytes: &[u8],
    budget: &Budget,
    delivery: &mut Delivery,
) -> Result<(), ErrorKind> {
    require_ignored_sigpipe()?;
    let stdin = owner.stdin.as_ref().ok_or(ErrorKind::Terminal)?.as_raw_fd();
    let stdout = owner
        .stdout
        .as_ref()
        .ok_or(ErrorKind::Terminal)?
        .as_raw_fd();
    let mut sent = 0;
    while sent < bytes.len() {
        if observe_child(owner)? {
            return Err(ErrorKind::ChildExited);
        }
        budget.check()?;
        #[cfg(test)]
        if test_hooks::take(test_hooks::Fault::WriteZero) {
            return Err(ErrorKind::ZeroWrite);
        }
        #[cfg(test)]
        if test_hooks::take(test_hooks::Fault::WriteInterrupted) {
            continue;
        }
        #[cfg(test)]
        if sent > 0 && test_hooks::take(test_hooks::Fault::WriteZeroAfterPrefix) {
            return Err(ErrorKind::ZeroWrite);
        }
        let end = sent.saturating_add(4096).min(bytes.len());
        #[cfg(test)]
        let end = if sent == 0 && test_hooks::take(test_hooks::Fault::WriteShortThenZero) {
            test_hooks::arm(test_hooks::Fault::WriteZeroAfterPrefix);
            end.min(2)
        } else {
            end
        };
        // SAFETY: stdin is live/nonblocking; bytes is valid for this slice length.
        // Supported embedding has SIGPIPE ignored and must not change it live.
        let n = unsafe { libc::write(stdin, bytes[sent..end].as_ptr().cast(), end - sent) };
        if n > 0 {
            *delivery = Delivery::MayHaveApplied;
            sent += n as usize;
            continue;
        }
        if n == 0 {
            return Err(ErrorKind::ZeroWrite);
        }
        if interrupted() {
            continue;
        }
        if !would_block() {
            return Err(last_error());
        }
        #[cfg(test)]
        if test_hooks::take(test_hooks::Fault::ExpireBlockedWrite) {
            test_hooks::record_blocked_write();
            super::budget_test_hooks::force_expired();
        }
        let mut fds = [
            libc::pollfd {
                fd: stdin,
                events: libc::POLLOUT,
                revents: 0,
            },
            libc::pollfd {
                fd: stdout,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: budget.cancel.wake.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        poll_fds(&mut fds, budget, owner)?;
        if fds[0].revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            return Err(ErrorKind::Io(libc::EPIPE));
        }
        if fds[1].revents & libc::POLLNVAL != 0 {
            return Err(ErrorKind::Io(libc::EBADF));
        }
        // A reply before the complete request is protocol corruption. This also
        // prevents a malicious helper filling stdout from deadlocking a write.
        if fds[1].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
            ensure_no_reply(owner, budget)?;
        }
        budget.check()?;
    }
    budget.check()
}

pub(super) struct ReplyFrame {
    pub(super) bytes: Vec<u8>,
    pub(super) peer_closed: bool,
}

pub(super) fn read_reply(
    owner: &mut ChildOwner,
    budget: &mut Budget,
) -> Result<ReplyFrame, ErrorKind> {
    let stdout = owner
        .stdout
        .as_ref()
        .ok_or(ErrorKind::Terminal)?
        .as_raw_fd();
    #[cfg(test)]
    if test_hooks::take(test_hooks::Fault::ReadWaitForHup) {
        loop {
            budget.check()?;
            let mut fd = libc::pollfd {
                fd: stdout,
                events: 0,
                revents: 0,
            };
            // SAFETY: test-only readiness barrier on the same live pipe.
            let result = unsafe { libc::poll(&mut fd, 1, 1) };
            if result < 0 && !interrupted() {
                return Err(last_error());
            }
            if fd.revents & libc::POLLHUP != 0 {
                break;
            }
        }
    }
    let mut reply = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        // Read readiness/error before checking a simultaneously raised cancel.
        // SAFETY: live nonblocking descriptor and writable stack buffer.
        let n = unsafe { libc::read(stdout, chunk.as_mut_ptr().cast(), chunk.len()) };
        if n > 0 {
            let bytes = &chunk[..n as usize];
            let newline = bytes.iter().position(|byte| *byte == b'\n');
            let used = newline.unwrap_or(bytes.len());
            let new_len = reply.len().checked_add(used).ok_or(ErrorKind::Limit)?;
            if new_len > MAX_LINE {
                return Err(ErrorKind::Limit);
            }
            if new_len > reply.capacity() {
                let target = new_len
                    .max(reply.capacity().saturating_mul(2))
                    .clamp(4096, MAX_LINE);
                budget.grow(&mut reply, target)?;
            }
            reply.extend_from_slice(&bytes[..used]);
            if let Some(at) = newline {
                if at + 1 != bytes.len() {
                    return Err(ErrorKind::Unsolicited);
                }
                // Detect already-buffered extra frames, while accepting final
                // bytes followed by HUP/EOF as a complete response.
                let peer_closed = probe_reply(owner, budget)?;
                budget.check()?;
                return Ok(ReplyFrame {
                    bytes: reply,
                    peer_closed,
                });
            }
            budget.check()?;
            continue;
        }
        if n == 0 {
            return Err(ErrorKind::Eof);
        }
        if interrupted() {
            budget.check()?;
            continue;
        }
        if !would_block() {
            return Err(last_error());
        }
        let mut fds = [
            libc::pollfd {
                fd: stdout,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: budget.cancel.wake.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        poll_fds(&mut fds, budget, owner)?;
        if fds[0].revents & libc::POLLNVAL != 0 {
            return Err(ErrorKind::Io(libc::EBADF));
        }
        // POLLIN+HUP is deliberately handled by the next read, preserving final
        // bytes. Cancellation alone cannot keep us spinning on readable eventfd.
        if fds[0].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) == 0 {
            budget.check()?;
        }
    }
}

pub(super) use super::lifecycle::{observe_child, reap, reap_once};

pub(super) fn close_pipes(owner: &mut ChildOwner) -> bool {
    owner.stdin.take();
    owner.stdout.take();
    true
}

pub(super) fn wait_cleanup(_owner: &ChildOwner, deadline: Instant) -> Result<(), ErrorKind> {
    let millis = timeout(deadline).min(1);
    // SAFETY: zero pollfds allows null and only waits under the original deadline.
    if unsafe { libc::poll(std::ptr::null_mut(), 0, millis) } < 0 && !interrupted() {
        return Err(last_error());
    }
    Ok(())
}

#[cfg(test)]
#[path = "pump_test_hooks.rs"]
pub(super) mod test_hooks;
