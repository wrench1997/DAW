//! Owned overlapped host pipes; synchronous ends are inherited by the helper.
//! No I/O thread, blocking flush, retry/replay, or global OS-policy change.
use super::{Budget, ChildOwner, Delivery, ErrorKind, MAX_LINE};
use std::fs::File;
use std::io;
use std::marker::PhantomPinned;
use std::mem::zeroed;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::os::windows::process::CommandExt;
use std::pin::Pin;
use std::process::{Command, Stdio};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use winapi::shared::winerror::{
    ERROR_BROKEN_PIPE, ERROR_GEN_FAILURE, ERROR_IO_INCOMPLETE, ERROR_IO_PENDING, ERROR_NOT_FOUND,
    ERROR_NO_DATA, ERROR_OPERATION_ABORTED, ERROR_PIPE_NOT_CONNECTED,
};
use winapi::um::fileapi::{CreateFileW, ReadFile, WriteFile, OPEN_EXISTING};
use winapi::um::handleapi::INVALID_HANDLE_VALUE;
use winapi::um::ioapiset::{CancelIoEx, GetOverlappedResult};
use winapi::um::minwinbase::OVERLAPPED;
use winapi::um::namedpipeapi::{CreateNamedPipeW, PeekNamedPipe};
use winapi::um::synchapi::{
    CreateEventW, ResetEvent, SetEvent, WaitForMultipleObjects, WaitForSingleObject,
};
use winapi::um::winbase::{
    GetNamedPipeClientProcessId, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED,
    PIPE_ACCESS_INBOUND, PIPE_ACCESS_OUTBOUND, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_WAIT, WAIT_FAILED, WAIT_OBJECT_0,
};
use winapi::um::winnt::{GENERIC_READ, GENERIC_WRITE, HANDLE};

pub(super) use super::lifecycle::{observe_child, reap, reap_once};
#[cfg(test)]
#[path = "pump_test_hooks.rs"]
pub(super) mod test_hooks;

const CHUNK: usize = 4096;
static NEXT_PIPE: AtomicU64 = AtomicU64::new(0);

pub(super) fn io_error(error: io::Error) -> ErrorKind {
    ErrorKind::Io(error.raw_os_error().unwrap_or(ERROR_GEN_FAILURE as i32))
}
fn last_error() -> ErrorKind {
    io_error(io::Error::last_os_error())
}
fn native_code() -> u32 {
    io::Error::last_os_error()
        .raw_os_error()
        .unwrap_or(ERROR_GEN_FAILURE as i32) as u32
}
fn pipe_error(code: u32) -> ErrorKind {
    match code {
        ERROR_BROKEN_PIPE | ERROR_NO_DATA | ERROR_PIPE_NOT_CONNECTED => ErrorKind::Eof,
        _ => ErrorKind::Io(code as i32),
    }
}
fn event() -> Result<OwnedHandle, ErrorKind> {
    // SAFETY: null attributes/name create a private, non-inherited manual-reset event.
    let handle = unsafe { CreateEventW(null_mut(), 1, 0, null()) };
    if handle.is_null() {
        return Err(last_error());
    }
    // SAFETY: fresh successful kernel handle is transferred exactly once.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle.cast()) })
}
pub(super) fn require_ignored_sigpipe() -> Result<(), ErrorKind> {
    // Windows pipe failure is reported by the I/O operation; no Unix signal exists.
    Ok(())
}
pub(super) fn new_wake() -> Result<OwnedHandle, ErrorKind> {
    event()
}
pub(super) fn wake(handle: &OwnedHandle) {
    #[cfg(test)]
    for _ in 0..4 {
        if test_hooks::interrupted_wake() {
            continue;
        }
        // SAFETY: Arc ownership keeps the event identity live for this signal.
        unsafe { SetEvent(handle.as_raw_handle().cast()) };
        return;
    }
    #[cfg(not(test))]
    // Atomic sticky cancellation and a <=25 ms wait slice also cover signal failure.
    unsafe {
        SetEvent(handle.as_raw_handle().cast())
    };
}

struct Operation {
    overlapped: OVERLAPPED,
    bytes: [u8; CHUNK],
    _pinned: PhantomPinned,
}
struct Resources {
    handle: OwnedHandle,
    event: OwnedHandle,
    operation: Pin<Box<Operation>>,
}
pub(super) struct Pipe {
    resources: Option<Resources>,
    pending: bool,
    closing: bool,
    cancellation_requested: bool,
    writing: bool,
}
// SAFETY: only exclusive ownership crosses into the retained-child quarantine.
// Kernel handles are thread-independent; the pinned buffer/OVERLAPPED addresses
// never move and no Rust reference or concurrent I/O observer crosses with them.
// FreshHelperSession itself remains !Send/!Sync through its owner-thread marker.
unsafe impl Send for Pipe {}

impl Pipe {
    fn pair(writing: bool) -> Result<(Self, File), ErrorKind> {
        let id = NEXT_PIPE
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| ErrorKind::Limit)?;
        let name = format!(
            r"\\.\pipe\citrus-strict-{}-{}-{}",
            std::process::id(),
            id,
            if writing { "in" } else { "out" }
        );
        let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        // SAFETY: bounded nul-terminated local name and null security attributes.
        // FIRST_PIPE_INSTANCE refuses preexisting names; remote clients are refused.
        let server = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                (if writing {
                    PIPE_ACCESS_OUTBOUND
                } else {
                    PIPE_ACCESS_INBOUND
                }) | FILE_FLAG_OVERLAPPED
                    | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                CHUNK as u32,
                CHUNK as u32,
                0,
                null_mut(),
            )
        };
        if server == INVALID_HANDLE_VALUE {
            return Err(last_error());
        }
        // SAFETY: successful new server handle has a single owner.
        let handle = unsafe { OwnedHandle::from_raw_handle(server.cast()) };
        // The new server is already available: client connection needs no wait or
        // pending ConnectNamedPipe storage before guarded child ownership exists.
        // A client connecting first establishes a valid named-pipe connection.
        let client = unsafe {
            CreateFileW(
                name.as_ptr(),
                if writing { GENERIC_READ } else { GENERIC_WRITE },
                0,
                null_mut(),
                OPEN_EXISTING,
                0,
                null_mut(),
            )
        };
        if client == INVALID_HANDLE_VALUE {
            return Err(last_error());
        }
        // SAFETY: this fresh synchronous client end is owned by File, then Stdio.
        let client = File::from(unsafe { OwnedHandle::from_raw_handle(client.cast()) });
        let mut client_pid = 0;
        // SAFETY: connected live pipe and writable DWORD. Refuse a racing local client.
        if unsafe { GetNamedPipeClientProcessId(server, &mut client_pid) } == 0 {
            return Err(last_error());
        }
        if client_pid != std::process::id() {
            return Err(ErrorKind::Spawn);
        }
        let event = event()?;
        let operation = Box::pin(Operation {
            // SAFETY: the SDK structure accepts a zero-initialized OVERLAPPED.
            overlapped: unsafe { zeroed() },
            bytes: [0; CHUNK],
            _pinned: PhantomPinned,
        });
        Ok((
            Self {
                resources: Some(Resources {
                    handle,
                    event,
                    operation,
                }),
                pending: false,
                closing: false,
                cancellation_requested: false,
                writing,
            },
            client,
        ))
    }
    fn handle(&self) -> HANDLE {
        self.resources
            .as_ref()
            .expect("live pipe resources")
            .handle
            .as_raw_handle()
            .cast()
    }
    fn event_handle(&self) -> HANDLE {
        self.resources
            .as_ref()
            .expect("live pipe resources")
            .event
            .as_raw_handle()
            .cast()
    }
    fn operation(&mut self) -> &mut Operation {
        // SAFETY: callers only modify fields after concrete completion, never move
        // the pinned Operation or reuse its addresses while an operation is pending.
        unsafe {
            self.resources
                .as_mut()
                .expect("live pipe resources")
                .operation
                .as_mut()
                .get_unchecked_mut()
        }
    }
    fn begin(&mut self, input: Option<&[u8]>) -> Result<Option<usize>, ErrorKind> {
        if self.pending || self.closing {
            return Err(ErrorKind::Terminal);
        }
        let handle = self.handle();
        let event = self.event_handle();
        // SAFETY: live owned event. Each pipe has at most one outstanding operation.
        if unsafe { ResetEvent(event) } == 0 {
            return Err(last_error());
        }
        let operation = self.operation();
        operation.overlapped = unsafe { zeroed() };
        operation.overlapped.hEvent = event;
        let len = match input {
            Some(bytes) if bytes.len() <= CHUNK => {
                operation.bytes[..bytes.len()].copy_from_slice(bytes);
                bytes.len()
            }
            Some(_) => return Err(ErrorKind::Limit),
            None => CHUNK,
        };
        let mut transferred = 0;
        // SAFETY: pinned owned storage and live overlapped server handle. No caller
        // buffer is borrowed by the kernel. Storage survives errors/cancellation.
        let ready = unsafe {
            if input.is_some() {
                WriteFile(
                    handle,
                    operation.bytes.as_ptr().cast(),
                    len as u32,
                    &mut transferred,
                    &mut operation.overlapped,
                )
            } else {
                ReadFile(
                    handle,
                    operation.bytes.as_mut_ptr().cast(),
                    len as u32,
                    &mut transferred,
                    &mut operation.overlapped,
                )
            }
        };
        if ready != 0 {
            return Ok(Some(transferred as usize));
        }
        let code = native_code();
        if code == ERROR_IO_PENDING {
            self.pending = true;
            Ok(None)
        } else {
            Err(if self.writing {
                ErrorKind::Io(code as i32)
            } else {
                pipe_error(code)
            })
        }
    }
    fn poll(&mut self) -> Option<Result<usize, ErrorKind>> {
        if !self.pending {
            return Some(Err(ErrorKind::Terminal));
        }
        #[cfg(test)]
        if self.closing && test_hooks::pipe_completion_held() {
            return None;
        }
        let handle = self.handle();
        let operation = self.operation();
        let mut transferred = 0;
        // SAFETY: same live operation/handle; FALSE never blocks for completion.
        if unsafe { GetOverlappedResult(handle, &mut operation.overlapped, &mut transferred, 0) }
            != 0
        {
            self.pending = false;
            return Some(Ok(transferred as usize));
        }
        let code = native_code();
        if code == ERROR_IO_INCOMPLETE {
            return None;
        }
        self.pending = false;
        Some(Err(if self.writing {
            ErrorKind::Io(code as i32)
        } else {
            pipe_error(code)
        }))
    }
    fn close(&mut self) -> (bool, Option<i32>) {
        self.closing = true;
        let mut error = None;
        if self.pending {
            if !self.cancellation_requested {
                let handle = self.handle();
                let operation = self.operation();
                // SAFETY: cancellation marks the same operation; its resources are
                // retained until GetOverlappedResult confirms actual completion.
                if unsafe { CancelIoEx(handle, &mut operation.overlapped) } == 0 {
                    let code = native_code();
                    if code != ERROR_NOT_FOUND {
                        error = Some(code as i32);
                    } else {
                        self.cancellation_requested = true;
                    }
                } else {
                    self.cancellation_requested = true;
                }
            }
            match self.poll() {
                None => return (false, error),
                Some(Err(ErrorKind::Io(code))) if code != ERROR_OPERATION_ABORTED as i32 => {
                    error.get_or_insert(code);
                }
                Some(_) => {}
            }
        }
        (true, error)
    }
}
impl Drop for Pipe {
    fn drop(&mut self) {
        if self.pending {
            // Defensive fail-closed boundary for an internal invariant violation.
            // Normal disposal/quarantine never drops pending resources. Even a
            // future erroneous early drop must not free memory still used by Windows.
            if let Some(resources) = self.resources.take() {
                std::mem::forget(resources);
            }
        }
    }
}

pub(super) fn prepare_pipes(command: &mut Command) -> Result<(Pipe, Pipe), ErrorKind> {
    let (stdin, read_end) = Pipe::pair(true)?;
    let (stdout, write_end) = Pipe::pair(false)?;
    command
        .stdin(Stdio::from(read_end))
        .stdout(Stdio::from(write_end))
        .stderr(Stdio::inherit())
        .creation_flags(0x08000000); // CREATE_NO_WINDOW
    Ok((stdin, stdout))
}
#[cfg(test)]
pub(super) fn prepare_with_retained_writer(
    command: &mut Command,
) -> Result<(Pipe, Pipe, File), ErrorKind> {
    let (stdin, read_end) = Pipe::pair(true)?;
    let (stdout, write_end) = Pipe::pair(false)?;
    let held = write_end.try_clone().map_err(io_error)?;
    command
        .stdin(Stdio::from(read_end))
        .stdout(Stdio::from(write_end))
        .stderr(Stdio::inherit())
        .creation_flags(0x08000000);
    Ok((stdin, stdout, held))
}
#[cfg(test)]
pub(super) fn has_pending_io(owner: &ChildOwner) -> bool {
    owner.stdin.as_ref().is_some_and(|pipe| pipe.pending)
        || owner.stdout.as_ref().is_some_and(|pipe| pipe.pending)
}
pub(super) fn finish_setup(owner: &ChildOwner) -> Result<(), ErrorKind> {
    #[cfg(test)]
    if test_hooks::take(test_hooks::Fault::Setup) {
        return Err(ErrorKind::Io(libc::EIO));
    }
    if owner.stdin.is_none() || owner.stdout.is_none() {
        return Err(ErrorKind::Spawn);
    }
    Ok(())
}
pub(super) fn close_pipes(owner: &mut ChildOwner) -> bool {
    for slot in [&mut owner.stdin, &mut owner.stdout] {
        if let Some(pipe) = slot {
            let (closed, error) = pipe.close();
            if let Some(error) = error {
                owner.cleanup_report.pipe_error.get_or_insert(error);
            }
            if closed {
                slot.take();
            }
        }
    }
    owner.stdin.is_none() && owner.stdout.is_none()
}
fn millis(deadline: Instant, maximum: u32) -> u32 {
    let left = deadline.saturating_duration_since(Instant::now());
    left.as_millis()
        .saturating_add(u128::from(left.subsec_nanos() % 1_000_000 != 0))
        .min(maximum as u128) as u32
}
pub(super) fn wait_cleanup(owner: &ChildOwner, deadline: Instant) -> Result<(), ErrorKind> {
    // A reaped child's signaled handle cannot pace pending pipe cancellation.
    // Sleep with no I/O buffer borrow; cleanup remains under its original deadline.
    if owner.observed_exit.is_some() {
        std::thread::sleep(std::time::Duration::from_millis(millis(deadline, 1) as u64));
        return Ok(());
    }
    let child = owner.child.as_raw_handle().cast();
    // SAFETY: borrowed live Child process handle; at most one millisecond.
    if unsafe { WaitForSingleObject(child, millis(deadline, 1)) } == WAIT_FAILED {
        return Err(last_error());
    }
    Ok(())
}
fn probe_reply(owner: &ChildOwner) -> Result<bool, ErrorKind> {
    let pipe = owner.stdout.as_ref().ok_or(ErrorKind::Terminal)?;
    let mut available = 0;
    // SAFETY: owned overlapped byte pipe, no concurrent reads on this owner thread.
    // Peek copies no payload and never consumes a response byte.
    if unsafe {
        PeekNamedPipe(
            pipe.handle(),
            null_mut(),
            0,
            null_mut(),
            &mut available,
            null_mut(),
        )
    } == 0
    {
        return match pipe_error(native_code()) {
            ErrorKind::Eof => Ok(true),
            error => Err(error),
        };
    }
    if available != 0 {
        return Err(ErrorKind::Unsolicited);
    }
    Ok(false)
}
pub(super) fn ensure_no_reply(owner: &ChildOwner, budget: &Budget) -> Result<(), ErrorKind> {
    budget.check()?;
    #[cfg(test)]
    let _ = test_hooks::take(test_hooks::Fault::ProbeInterrupted);
    if probe_reply(owner)? {
        return Err(ErrorKind::Eof);
    }
    Ok(())
}
fn pipe_mut(owner: &mut ChildOwner, writing: bool) -> Result<&mut Pipe, ErrorKind> {
    if writing {
        owner.stdin.as_mut()
    } else {
        owner.stdout.as_mut()
    }
    .ok_or(ErrorKind::Terminal)
}
fn await_io(owner: &mut ChildOwner, writing: bool, budget: &Budget) -> Result<usize, ErrorKind> {
    loop {
        if let Some(result) = pipe_mut(owner, writing)?.poll() {
            return result;
        }
        if writing {
            if let Err(error) = ensure_no_reply(owner, budget) {
                // A final write can complete just as the helper produces its reply.
                // Prove completion again before classifying that reply as early.
                if let Some(result) = pipe_mut(owner, writing)?.poll() {
                    return result;
                }
                return Err(error);
            }
        }
        if observe_child(owner)? {
            // Preserve a completed final block instead of treating exit alone as EOF.
            if let Some(result) = pipe_mut(owner, writing)?.poll() {
                return result;
            }
            return Err(ErrorKind::ChildExited);
        }
        budget.check()?;
        let handles = [
            pipe_mut(owner, writing)?.event_handle(),
            budget.cancel.wake.as_raw_handle().cast(),
            owner.child.as_raw_handle().cast(),
        ];
        // SAFETY: three live owned handles; only this thread observes the operation.
        let result = unsafe {
            WaitForMultipleObjects(
                handles.len() as u32,
                handles.as_ptr(),
                0,
                millis(budget.deadline, 25),
            )
        };
        if result == WAIT_FAILED {
            return Err(last_error());
        }
        if result == WAIT_OBJECT_0 + 1 {
            // Inspect simultaneous completion/OS errors before sticky cancellation.
            if let Some(result) = pipe_mut(owner, writing)?.poll() {
                return result;
            }
            budget.check()?;
        }
    }
}
pub(super) fn write_request(
    owner: &mut ChildOwner,
    bytes: &[u8],
    budget: &Budget,
    delivery: &mut Delivery,
) -> Result<(), ErrorKind> {
    let mut sent = 0usize;
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
        ensure_no_reply(owner, budget)?;
        let end = sent.saturating_add(CHUNK).min(bytes.len());
        #[cfg(test)]
        let end = if sent == 0 && test_hooks::take(test_hooks::Fault::WriteShortThenZero) {
            test_hooks::arm(test_hooks::Fault::WriteZeroAfterPrefix);
            end.min(2)
        } else {
            end
        };
        let n = match pipe_mut(owner, true)?.begin(Some(&bytes[sent..end]))? {
            Some(n) => n,
            None => {
                // Submitted overlapped writes may apply before cancellation is observed.
                *delivery = Delivery::MayHaveApplied;
                #[cfg(test)]
                if test_hooks::take(test_hooks::Fault::ExpireBlockedWrite) {
                    test_hooks::record_blocked_write();
                    super::budget_test_hooks::force_expired();
                }
                await_io(owner, true, budget)?
            }
        };
        if n == 0 {
            return Err(ErrorKind::ZeroWrite);
        }
        if n > end - sent {
            return Err(ErrorKind::Shape);
        }
        *delivery = Delivery::MayHaveApplied;
        sent += n;
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
    #[cfg(test)]
    if test_hooks::take(test_hooks::Fault::ReadWaitForHup) {
        while !observe_child(owner)? {
            budget.check()?;
            wait_cleanup(owner, budget.deadline)?;
        }
    }
    let mut reply = Vec::new();
    loop {
        let n = match pipe_mut(owner, false)?.begin(None)? {
            Some(n) => n,
            None => await_io(owner, false, budget)?,
        };
        if n == 0 {
            return Err(ErrorKind::Eof);
        }
        if n > CHUNK {
            return Err(ErrorKind::Shape);
        }
        let pipe = pipe_mut(owner, false)?;
        let bytes = &pipe.operation().bytes[..n];
        let newline = bytes.iter().position(|byte| *byte == b'\n');
        let used = newline.unwrap_or(bytes.len());
        let new_len = reply.len().checked_add(used).ok_or(ErrorKind::Limit)?;
        if new_len > MAX_LINE {
            return Err(ErrorKind::Limit);
        }
        if new_len > reply.capacity() {
            let target = new_len
                .max(reply.capacity().saturating_mul(2))
                .clamp(CHUNK, MAX_LINE);
            budget.grow(&mut reply, target)?;
        }
        reply.extend_from_slice(&bytes[..used]);
        if let Some(at) = newline {
            if at + 1 != bytes.len() {
                return Err(ErrorKind::Unsolicited);
            }
            let peer_closed = probe_reply(owner)?;
            budget.check()?;
            return Ok(ReplyFrame {
                bytes: reply,
                peer_closed,
            });
        }
        budget.check()?;
    }
}
