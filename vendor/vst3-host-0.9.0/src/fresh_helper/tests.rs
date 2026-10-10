use super::*;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

// Every test that reserves the process-global slot holds this independent lock.
// Codec tests and cancellation-FD-only tests need no child reservation.
static CHILD_TESTS: Mutex<()> = Mutex::new(());
struct TestScope {
    _serial: MutexGuard<'static, ()>,
}
fn serial() -> TestScope {
    TestScope {
        _serial: CHILD_TESTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    }
}
impl Drop for TestScope {
    fn drop(&mut self) {
        pump::test_hooks::clear();
        let report = FreshHelperSession::cleanup_abandoned(deadline());
        if !matches!(report.state, CleanupState::Empty | CleanupState::Reaped(_)) {
            if std::thread::panicking() {
                eprintln!("cleanup retained unresolved mock child: {report:?}");
            } else {
                panic!("cleanup retained unresolved mock child: {report:?}");
            }
        }
    }
}
fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(3)
}
fn python(script: &str) -> Command {
    let mut command = Command::new("python3");
    command.args(["-u", "-c", script]);
    command
}
fn spawn(script: &str) -> FreshHelperSession {
    match FreshHelperSession::spawn(&mut python(script), deadline()) {
        Ok(session) => session,
        Err(mut failure) => {
            if let Some(session) = failure.cleanup.as_mut() {
                let report = session.dispose_checked(deadline());
                assert!(
                    matches!(report.state, CleanupState::Reaped(_)),
                    "setup cleanup: {report:?}"
                );
            }
            panic!("mock spawn: {:?}", failure.cause);
        }
    }
}
fn finish_forced(session: &mut FreshHelperSession) {
    let report = session.dispose_checked(deadline());
    assert!(
        matches!(report.state, CleanupState::Reaped(_)),
        "checked cleanup: {report:?}"
    );
}
fn finish(session: &mut FreshHelperSession) {
    let report = session.dispose_checked(deadline());
    assert!(
        matches!(report.state, CleanupState::Reaped(status) if status.success()),
        "cooperative child failed: {report:?}"
    );
    assert!(
        !report.kill_requested,
        "child assertion cannot be bypassed by force kill"
    );
}
fn failure_kind(result: Result<HostResponse, ExchangeFailure>) -> ExchangeFailure {
    match result {
        Err(failure) => failure,
        Ok(_) => panic!("expected failed exchange"),
    }
}
fn scalar(session: &mut FreshHelperSession) -> Result<HostResponse, ExchangeFailure> {
    session.exchange(
        &HostCommand::LatencySamples,
        ReplyShape::LatencySamples,
        deadline(),
    )
}
fn new_budget(deadline: Instant) -> Budget {
    Budget {
        deadline,
        cancel: Arc::new(Cancellation {
            cancelled: AtomicBool::new(false),
            wake: pump::new_wake().unwrap(),
            skip_wake: AtomicBool::new(false),
        }),
        live: 0,
        peak: 0,
        diagnostic_copy: false,
    }
}

#[test]
fn scalar_roundtrip_then_exact_reply_free_shutdown() {
    let _serial = serial();
    let mut session = spawn(
        r#"
import sys,json
assert json.loads(sys.stdin.readline()) == 'LatencySamples'
print('{"LatencySamples":{"samples":0}}',flush=True)
assert json.loads(sys.stdin.readline()) == 'Shutdown'
assert sys.stdin.read() == ''
"#,
    );
    assert!(matches!(
        scalar(&mut session),
        Ok(HostResponse::LatencySamples { samples: 0 })
    ));
    finish(&mut session);
}

#[test]
fn reported_error_seals_without_replay_or_second_shutdown() {
    let _serial = serial();
    let mut session = spawn(
        r#"
import sys,json
assert json.loads(sys.stdin.readline()) == 'SaveState'
print('{"Error":{"message":"operation may have partially applied"}}',flush=True)
assert sys.stdin.read() == ''
"#,
    );
    let failure =
        failure_kind(session.exchange(&HostCommand::SaveState, ReplyShape::State, deadline()));
    assert_eq!(failure.delivery, Delivery::MayHaveApplied);
    assert!(matches!(
        failure.reported.as_deref(),
        Some(HostResponse::Error { .. })
    ));
    assert_eq!(
        failure_kind(scalar(&mut session)).delivery,
        Delivery::NotSent
    );
    finish(&mut session);
}

#[test]
fn malformed_wrong_and_extra_frames_are_terminal() {
    let _serial = serial();
    for (wire, expected) in [
        ("not-json\\n", ErrorKind::MalformedReply),
        (
            "{\"TailSamples\":{\"samples\":7}}\\n",
            ErrorKind::WrongReply,
        ),
        (
            "{\"LatencySamples\":{\"samples\":7}}\\n{}\\n",
            ErrorKind::Unsolicited,
        ),
    ] {
        let script = format!("import sys\nsys.stdin.readline()\nsys.stdout.write({wire:?}.replace('\\\\n','\\n'));sys.stdout.flush()\nsys.stdin.read()\n");
        let mut session = spawn(&script);
        let failure = failure_kind(scalar(&mut session));
        assert_eq!(failure.cause, expected);
        assert_eq!(failure.delivery, Delivery::MayHaveApplied);
        assert!(scalar(&mut session).is_err());
        finish(&mut session);
    }
}

#[test]
fn final_reply_bytes_are_preserved_as_diagnostics_when_peer_closure_is_observed() {
    let _serial = serial();
    let mut session = spawn("import sys\nsys.stdin.readline()\nprint('{\"LatencySamples\":{\"samples\":4294967295}}',flush=True)\n");
    // Closure can become observable after the immediate post-frame read. Both
    // orders preserve the value, but any observed EOF seals the session.
    match scalar(&mut session) {
        Ok(HostResponse::LatencySamples { samples: u32::MAX }) => {}
        Err(failure) => {
            assert!(matches!(
                failure.cause,
                ErrorKind::Eof | ErrorKind::ChildExited
            ));
            assert!(matches!(
                failure.reported.as_deref(),
                Some(HostResponse::LatencySamples { samples: u32::MAX })
            ));
            assert_eq!(session.state, State::Terminal);
        }
        _ => panic!("complete final frame lost"),
    }
    finish(&mut session);
}

#[test]
fn incomplete_reply_eof_is_not_success() {
    let _serial = serial();
    let mut session = spawn("import sys\nsys.stdin.readline()\nsys.stdout.write('{\"LatencySamples\":');sys.stdout.flush()\n");
    let failure = failure_kind(scalar(&mut session));
    assert!(matches!(
        failure.cause,
        ErrorKind::Eof | ErrorKind::ChildExited
    ));
    assert_eq!(failure.delivery, Delivery::MayHaveApplied);
    finish(&mut session);
}

#[test]
fn response_timeout_is_terminal_and_not_retried() {
    let _serial = serial();
    let mut session = spawn("import sys,time\nsys.stdin.readline()\ntime.sleep(10)\n");
    let failure = failure_kind(session.exchange(
        &HostCommand::LatencySamples,
        ReplyShape::LatencySamples,
        Instant::now() + Duration::from_millis(80),
    ));
    assert_eq!(failure.cause, ErrorKind::Deadline);
    assert_eq!(failure.delivery, Delivery::MayHaveApplied);
    assert_eq!(
        failure_kind(scalar(&mut session)).cause,
        ErrorKind::Deadline
    );
    finish_forced(&mut session);
}

#[test]
fn cancel_wakes_owner_poll_without_session_lock() {
    let _serial = serial();
    let mut session = spawn("import sys,time\nsys.stdin.readline()\ntime.sleep(10)\n");
    let cancel = session.cancellation();
    let failure = std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(Duration::from_millis(50));
            cancel.cancel();
        });
        failure_kind(scalar(&mut session))
    });
    assert_eq!(failure.cause, ErrorKind::Cancelled);
    assert_eq!(failure.delivery, Delivery::MayHaveApplied);
    finish_forced(&mut session);
    // Still owns its eventfd, even after child disposal. No recycled fd target.
    cancel.cancel();
}

#[test]
fn expired_and_unsupported_requests_send_nothing() {
    let _serial = serial();
    for unsupported in [false, true] {
        let mut session = spawn("import sys\nassert sys.stdin.read() == ''\n");
        let result = if unsupported {
            session.exchange(
                &HostCommand::CreateGui,
                ReplyShape::ParameterValue,
                deadline(),
            )
        } else {
            session.exchange(
                &HostCommand::LatencySamples,
                ReplyShape::LatencySamples,
                Instant::now(),
            )
        };
        let failure = failure_kind(result);
        assert_eq!(failure.delivery, Delivery::NotSent);
        assert_eq!(
            failure.cause,
            if unsupported {
                ErrorKind::UnsupportedRequest
            } else {
                ErrorKind::Deadline
            }
        );
        finish(&mut session);
    }
}

#[test]
fn blocked_stdin_deadline_marks_partial_delivery() {
    let _serial = serial();
    let mut session = spawn("import time\ntime.sleep(180)\n");
    // Larger than the OS pipe and bounded by the strict state limit.
    let command = HostCommand::LoadState {
        data: vec![7; 1024 * 1024],
        context: crate::plugin::StateContext::Project,
    };
    // Expire only after an actual EAGAIN on the non-reading pipe. Counting
    // and encoding have a generous watchdog, never a MayHaveApplied assumption.
    let expiry = budget_test_hooks::expire_at_check(usize::MAX);
    pump::test_hooks::arm(pump::test_hooks::Fault::ExpireBlockedWrite);
    let failure = failure_kind(session.exchange(
        &command,
        ReplyShape::Success,
        Instant::now() + Duration::from_secs(180),
    ));
    assert!(pump::test_hooks::blocked_write_observed());
    drop(expiry);
    assert_eq!(failure.cause, ErrorKind::Deadline);
    assert_eq!(failure.delivery, Delivery::MayHaveApplied);
    finish_forced(&mut session);
}

#[test]
fn unresolved_live_and_abandoned_owner_prevent_second_spawn() {
    let _serial = serial();
    let session = spawn("import time\ntime.sleep(10)\n");
    let first_pid = session.owner.as_ref().unwrap().child.id();
    let refusal = FreshHelperSession::spawn(
        &mut python("raise AssertionError('must not spawn')"),
        deadline(),
    );
    assert!(matches!(
        refusal,
        Err(SpawnFailure {
            cause: ErrorKind::Busy,
            cleanup: None
        })
    ));
    drop(session);
    let refusal = FreshHelperSession::spawn(
        &mut python("raise AssertionError('must not spawn')"),
        deadline(),
    );
    assert!(matches!(
        refusal,
        Err(SpawnFailure {
            cause: ErrorKind::Busy,
            cleanup: None
        })
    ));
    let pending = FreshHelperSession::cleanup_abandoned(Instant::now());
    assert!(matches!(pending.state, CleanupState::Pending));
    // The same actual child remains owned; cleanup did not evict/overwrite it.
    assert_eq!(quarantine::abandoned_pid(), Some(first_pid));
    let reaped = FreshHelperSession::cleanup_abandoned(deadline());
    assert!(
        matches!(reaped.state, CleanupState::Reaped(_)),
        "{reaped:?}"
    );
    assert!(matches!(
        FreshHelperSession::cleanup_abandoned(deadline()).state,
        CleanupState::Empty
    ));
    let mut next = spawn("import sys\nsys.stdin.readline()\n");
    finish(&mut next);
}

#[test]
fn owner_unwind_quarantines_child_without_deadlock() {
    let _serial = serial();
    let result = std::panic::catch_unwind(|| {
        let _session = spawn("import time\ntime.sleep(10)\n");
        panic!("Rust owner callback failure");
    });
    assert!(result.is_err());
    let reaped = FreshHelperSession::cleanup_abandoned(deadline());
    assert!(
        matches!(reaped.state, CleanupState::Reaped(_)),
        "{reaped:?}"
    );
}

#[test]
fn direct_pending_cleanup_retains_owner_until_reap() {
    let _serial = serial();
    let mut session = spawn("import time\ntime.sleep(10)\n");
    let pid = session.owner.as_ref().unwrap().child.id();
    let report = session.dispose_checked(Instant::now());
    assert!(matches!(report.state, CleanupState::Pending));
    assert_eq!(session.owner.as_ref().unwrap().child.id(), pid);
    assert_eq!(session.state, State::Terminal);
    finish_forced(&mut session);
    assert!(session.owner.is_none());
}

#[test]
fn codec_budget_limits_and_final_expiry_are_explicit() {
    let mut budget = new_budget(deadline());
    budget.reserve(MAX_PAYLOAD).unwrap();
    assert_eq!(budget.reserve(1), Err(ErrorKind::Limit));
    budget.release(MAX_PAYLOAD);
    budget.deadline = Instant::now();
    assert_eq!(budget.check(), Err(ErrorKind::Deadline));
    budget.cancel.cancelled.store(true, Ordering::Release);
    assert_eq!(budget.check(), Err(ErrorKind::Cancelled));
}

#[test]
fn sticky_cancel_is_observed_even_if_wake_write_is_missed() {
    let _serial = serial();
    let mut session = spawn("import sys,time\nsys.stdin.readline()\ntime.sleep(10)\n");
    let cancel = session.cancellation();
    cancel.0.skip_wake.store(true, Ordering::Release);
    let start = Instant::now();
    let failure = std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(Duration::from_millis(50));
            cancel.cancel();
        });
        failure_kind(session.exchange(
            &HostCommand::LatencySamples,
            ReplyShape::LatencySamples,
            Instant::now() + Duration::from_secs(5),
        ))
    });
    assert_eq!(failure.cause, ErrorKind::Cancelled);
    assert!(
        start.elapsed() < Duration::from_secs(3),
        "missed wake slept until full deadline"
    );
    finish_forced(&mut session);
}

#[test]
fn setup_failure_returns_actual_child_for_checked_cleanup() {
    let _serial = serial();
    pump::test_hooks::arm(pump::test_hooks::Fault::Setup);
    let mut failure =
        match FreshHelperSession::spawn(&mut python("import time\ntime.sleep(10)\n"), deadline()) {
            Err(failure) => failure,
            Ok(_) => panic!("injected setup failure missing"),
        };
    assert_eq!(failure.cause, ErrorKind::Io(libc::EIO));
    let session = failure
        .cleanup
        .as_mut()
        .expect("child cannot disappear on setup failure");
    assert!(session.owner.is_some());
    finish_forced(session);
}

#[test]
fn wait_and_kill_errors_retain_same_child_for_retry_cleanup_only() {
    let _serial = serial();
    for (fault, expected_wait, expected_kill) in [
        (pump::test_hooks::Fault::Wait, Some(libc::EIO), None),
        (pump::test_hooks::Fault::Kill, None, Some(libc::EPERM)),
    ] {
        let mut session = spawn("import time\ntime.sleep(10)\n");
        let pid = session.owner.as_ref().unwrap().child.id();
        pump::test_hooks::arm(fault);
        let report = session.dispose_checked(deadline());
        assert!(matches!(report.state, CleanupState::Pending));
        assert_eq!(report.wait_error, expected_wait);
        assert_eq!(report.kill_error, expected_kill);
        assert_eq!(session.owner.as_ref().unwrap().child.id(), pid);
        assert!(scalar(&mut session).is_err());
        finish_forced(&mut session);
    }
}

#[test]
fn kill_evidence_survives_a_later_observation_error() {
    let _serial = serial();
    let mut session = spawn("import time\ntime.sleep(10)\n");
    pump::test_hooks::arm(pump::test_hooks::Fault::PollAfterKill);
    let report = session.dispose_checked(deadline());
    assert_eq!(report.wait_error, Some(libc::EIO));
    assert!(report.kill_requested);
    assert!(session.owner.is_some());
    finish_forced(&mut session);
}

#[test]
fn quarantine_cleanup_unwind_restores_exact_owner() {
    let _serial = serial();
    let session = spawn("import time\ntime.sleep(10)\n");
    let pid = session.owner.as_ref().unwrap().child.id();
    drop(session);
    pump::test_hooks::arm(pump::test_hooks::Fault::ReapPanic);
    let result = std::panic::catch_unwind(|| {
        let _ = FreshHelperSession::cleanup_abandoned(deadline());
    });
    assert!(result.is_err());
    assert_eq!(quarantine::abandoned_pid(), Some(pid));
    let report = FreshHelperSession::cleanup_abandoned(deadline());
    assert!(matches!(report.state, CleanupState::Reaped(_)));
}

#[test]
fn zero_write_never_claims_delivery_and_interrupted_operations_retry() {
    let _serial = serial();
    let mut session = spawn("import sys\nassert sys.stdin.read() == ''\n");
    pump::test_hooks::arm(pump::test_hooks::Fault::WriteZero);
    let failure = failure_kind(scalar(&mut session));
    assert_eq!(failure.cause, ErrorKind::ZeroWrite);
    assert_eq!(failure.delivery, Delivery::NotSent);
    finish(&mut session);
    for fault in [
        pump::test_hooks::Fault::WriteInterrupted,
        pump::test_hooks::Fault::ProbeInterrupted,
    ] {
        let mut session = spawn("import sys,json\nassert json.loads(sys.stdin.readline()) == 'LatencySamples'\nprint('{\"LatencySamples\":{\"samples\":9}}',flush=True)\nassert json.loads(sys.stdin.readline()) == 'Shutdown'\n");
        pump::test_hooks::arm(fault);
        assert!(matches!(
            scalar(&mut session),
            Ok(HostResponse::LatencySamples { samples: 9 })
        ));
        finish(&mut session);
    }
}

#[test]
fn unsupported_sigpipe_and_epipe_are_confined_to_disposable_hosts() {
    let _serial = serial();
    for mode in ["sigpipe-default", "epipe"] {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "fresh_helper::tests::disposable_signal_host",
                "--nocapture",
            ])
            .env("CITRUS_STRICT_DISPOSABLE_SIGNAL_TEST", mode);
        let mut session = match FreshHelperSession::spawn(&mut command, deadline()) {
            Ok(session) => session,
            Err(_) => panic!("disposable host spawn"),
        };
        // This executable is a bounded unit-test host, not a protocol helper.
        // Do not write Shutdown into its test runner's stdin.
        session.state = State::Terminal;
        // Wait directly for the disposable test to finish, under a separate
        // deadline; Drop still retains it if an assertion unwinds here.
        let end = deadline();
        loop {
            match session.owner.as_mut().unwrap().child.try_wait().unwrap() {
                Some(status) => {
                    assert!(status.success(), "disposable {mode} failed");
                    break;
                }
                None if Instant::now() < end => std::thread::sleep(Duration::from_millis(5)),
                None => panic!("disposable {mode} exceeded its watchdog"),
            }
        }
        use std::io::Read;
        let mut captured = String::new();
        session
            .owner
            .as_mut()
            .unwrap()
            .stdout
            .as_mut()
            .unwrap()
            .take(64 * 1024)
            .read_to_string(&mut captured)
            .unwrap();
        let marker = format!("STRICT_SIGNAL_COMPLETE:{mode}");
        assert_eq!(
            captured.lines().filter(|line| *line == marker).count(),
            1,
            "disposable filter must actually execute its mode-specific assertions: {captured}"
        );
        finish(&mut session);
    }
}

#[test]
fn disposable_signal_host() {
    let Ok(mode) = std::env::var("CITRUS_STRICT_DISPOSABLE_SIGNAL_TEST") else {
        return;
    };
    let _serial = serial();
    if mode == "sigpipe-default" {
        // This process exists solely for this test. Production never changes
        // signal disposition; the ordinary test runner is unaffected.
        // SAFETY: SIG_DFL is a valid handler value, no Rust callback is installed.
        unsafe {
            libc::signal(libc::SIGPIPE, libc::SIG_DFL);
        }
        let result = FreshHelperSession::spawn(
            &mut python("raise AssertionError('must not spawn')"),
            deadline(),
        );
        assert!(matches!(
            result,
            Err(SpawnFailure {
                cause: ErrorKind::Sigpipe,
                cleanup: None
            })
        ));
        println!("STRICT_SIGNAL_COMPLETE:{mode}");
        return;
    }
    assert_eq!(mode, "epipe");
    let mut session = spawn("import os,time\nos.close(0)\ntime.sleep(10)\n");
    // A nonblocking POLLERR is the actual pipe-reader-closed condition, so no
    // scheduling sleep is treated as proof that the child closed its descriptor.
    use std::os::fd::AsRawFd;
    let fd = session
        .owner
        .as_ref()
        .unwrap()
        .stdin
        .as_ref()
        .unwrap()
        .as_raw_fd();
    let end = deadline();
    loop {
        let mut descriptor = libc::pollfd {
            fd,
            events: libc::POLLOUT,
            revents: 0,
        };
        // SAFETY: exactly one valid pollfd and a live borrowed descriptor.
        let result = unsafe { libc::poll(&mut descriptor, 1, 10) };
        assert!(result >= 0);
        if descriptor.revents & libc::POLLERR != 0 {
            break;
        }
        assert!(Instant::now() < end, "pipe did not report reader closure");
    }
    let failure = failure_kind(scalar(&mut session));
    assert_eq!(failure.cause, ErrorKind::Io(libc::EPIPE));
    assert_eq!(failure.delivery, Delivery::NotSent);
    finish_forced(&mut session);
    println!("STRICT_SIGNAL_COMPLETE:{mode}");
}

#[test]
fn cancellation_wake_retries_are_finite_under_interrupt_storm() {
    let budget = new_budget(deadline());
    let cancel = CancelHandle(budget.cancel.clone());
    pump::test_hooks::arm_wake_interrupts(1000);
    cancel.cancel();
    assert_eq!(pump::test_hooks::wake_attempts(), 4);
    assert_eq!(budget.check(), Err(ErrorKind::Cancelled));
    pump::test_hooks::clear();
}

#[test]
fn already_closed_final_reply_is_terminal_and_retained() {
    let _serial = serial();
    let mut session = spawn("import sys\nsys.stdin.readline()\nprint('{\"LatencySamples\":{\"samples\":23}}',flush=True)\n");
    pump::test_hooks::arm(pump::test_hooks::Fault::ReadWaitForHup);
    let failure = failure_kind(scalar(&mut session));
    assert!(matches!(
        failure.cause,
        ErrorKind::Eof | ErrorKind::ChildExited
    ));
    assert_eq!(failure.delivery, Delivery::MayHaveApplied);
    assert!(matches!(
        failure.reported.as_deref(),
        Some(HostResponse::LatencySamples { samples: 23 })
    ));
    assert_eq!(session.state, State::Terminal);
    assert!(scalar(&mut session).is_err());
    finish(&mut session);
}

#[test]
fn zero_write_after_prefix_keeps_may_have_applied_and_no_shutdown() {
    let _serial = serial();
    let mut session = spawn("import sys\ndata=sys.stdin.buffer.read()\nassert len(data)==4096\nassert b'Shutdown' not in data\n");
    pump::test_hooks::arm(pump::test_hooks::Fault::WriteZeroAfterPrefix);
    let command = HostCommand::LoadState {
        data: vec![1; 8192],
        context: crate::plugin::StateContext::Project,
    };
    let failure = failure_kind(session.exchange(&command, ReplyShape::Success, deadline()));
    assert_eq!(failure.cause, ErrorKind::ZeroWrite);
    assert_eq!(failure.delivery, Delivery::MayHaveApplied);
    finish(&mut session);
}

#[test]
fn oversized_unterminated_pipe_reply_is_rejected_at_the_line_limit() {
    let _serial = serial();
    let mut session = spawn("import sys\nsys.stdin.readline()\nchunk=b'x'*4096\nfor _ in range(24577): sys.stdout.buffer.write(chunk)\nsys.stdout.buffer.flush()\nsys.stdin.read()\n");
    let failure = failure_kind(session.exchange(
        &HostCommand::LatencySamples,
        ReplyShape::LatencySamples,
        Instant::now() + Duration::from_secs(20),
    ));
    assert_eq!(failure.cause, ErrorKind::Limit);
    assert_eq!(failure.delivery, Delivery::MayHaveApplied);
    finish_forced(&mut session);
}

#[test]
fn observed_fault_is_sticky_when_cancel_is_published_later() {
    let _serial = serial();
    let mut session = spawn(
        "import sys\nsys.stdin.readline()\nprint('malformed',flush=True)\nsys.stdin.read()\n",
    );
    let failure = failure_kind(scalar(&mut session));
    assert_eq!(failure.cause, ErrorKind::MalformedReply);
    session.cancellation().cancel();
    assert_eq!(
        failure_kind(scalar(&mut session)).cause,
        ErrorKind::MalformedReply
    );
    finish(&mut session);
}

#[test]
fn cold_load_crash_is_one_attempt_and_no_implicit_replay() {
    let _serial = serial();
    let mut session = spawn("import sys,json\nrequest=json.loads(sys.stdin.readline())\nassert list(request)==['LoadPlugin']\nprint('{\"Crashed\":{\"message\":\"mock cold-load failure\"}}',flush=True)\nassert sys.stdin.read()==''\n");
    let command = HostCommand::LoadPlugin {
        path: "mock-only.vst3".into(),
        sample_rate: 48000.0,
        block_size: 128,
        tempo: 120.0,
        time_sig_numerator: 4,
        time_sig_denominator: 4,
        class_id: None,
    };
    let failure = failure_kind(session.exchange(&command, ReplyShape::PluginInfo, deadline()));
    assert_eq!(failure.delivery, Delivery::MayHaveApplied);
    assert!(matches!(
        failure.reported.as_deref(),
        Some(HostResponse::Crashed { .. })
    ));
    assert_eq!(
        failure_kind(session.exchange(&command, ReplyShape::PluginInfo, deadline())).delivery,
        Delivery::NotSent
    );
    finish(&mut session);
}

#[test]
fn repeated_short_cleanup_does_not_renew_first_grace() {
    let _serial = serial();
    let mut session = spawn("import time\ntime.sleep(10)\n");
    let pid = session.owner.as_ref().unwrap().child.id();
    let first = session.dispose_checked(Instant::now() + Duration::from_millis(5));
    assert!(matches!(first.state, CleanupState::Pending));
    let grace = session.owner.as_ref().unwrap().graceful_until.unwrap();
    let second = session.dispose_checked(Instant::now() + Duration::from_millis(5));
    assert!(matches!(second.state, CleanupState::Pending));
    assert_eq!(session.owner.as_ref().unwrap().child.id(), pid);
    assert_eq!(session.owner.as_ref().unwrap().graceful_until, Some(grace));
    assert!(session.owner.as_ref().unwrap().stdin.is_none());
    std::thread::sleep(grace.saturating_duration_since(Instant::now()) + Duration::from_millis(1));
    let final_report = session.dispose_checked(deadline());
    assert!(matches!(final_report.state, CleanupState::Reaped(_)));
    assert!(final_report.kill_requested);
}

#[test]
fn cancellation_handle_has_only_independent_send_sync_authority() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<CancelHandle>();
    // Negative auto-trait checks use ambiguous inference if the forbidden trait
    // is implemented; they exercise the real private type rather than imports.
    trait AmbiguousIfSend<A> {
        fn marker() {}
    }
    impl<T: ?Sized> AmbiguousIfSend<()> for T {}
    impl<T: ?Sized + Send> AmbiguousIfSend<u8> for T {}
    let _ = <FreshHelperSession as AmbiguousIfSend<_>>::marker;
    trait AmbiguousIfSync<A> {
        fn marker() {}
    }
    impl<T: ?Sized> AmbiguousIfSync<()> for T {}
    impl<T: ?Sized + Sync> AmbiguousIfSync<u8> for T {}
    let _ = <FreshHelperSession as AmbiguousIfSync<_>>::marker;
}

#[test]
fn direct_child_exit_is_detected_while_a_separate_writer_holds_stdout_open() {
    let _serial = serial();
    use std::os::fd::{FromRawFd, OwnedFd};
    pump::require_ignored_sigpipe().unwrap();
    let permit = quarantine::reserve().unwrap();
    let cancel = Arc::new(Cancellation {
        cancelled: AtomicBool::new(false),
        wake: pump::new_wake().unwrap(),
        skip_wake: AtomicBool::new(false),
    });
    let mut fds = [-1; 2];
    // SAFETY: pipe2 writes exactly two new descriptors; atomic CLOEXEC prevents
    // unintended inheritance. Each descriptor is immediately given one owner.
    assert_eq!(
        unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) },
        0
    );
    // SAFETY: fresh successful pipe2 descriptors, transferred exactly once.
    let read = unsafe { OwnedFd::from_raw_fd(fds[0]) };
    let extra_writer = unsafe { OwnedFd::from_raw_fd(fds[1]) };
    let mut command = python("import sys\nsys.stdin.readline()\n");
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::from(extra_writer.try_clone().unwrap()))
        .stderr(Stdio::inherit());
    let child = command.spawn().unwrap();
    // Install the returned child in guarded ownership before any fallible setup.
    let mut session = FreshHelperSession {
        owner: Some(ChildOwner {
            child,
            stdin: None,
            stdout: Some(ChildStdout::from(read)),
            kill_requested: false,
            graceful_until: None,
            observed_exit: None,
            cleanup_report: CleanupReport::pending(),
        }),
        permit: Some(permit),
        cancel,
        state: State::Ready,
        terminal: None,
        completed_cleanup: None,
        _owner_thread: PhantomData,
    };
    let owner = session.owner.as_mut().unwrap();
    owner.stdin = owner.child.stdin.take();
    pump::set_nonblocking(owner.stdin.as_ref().unwrap()).unwrap();
    let start = Instant::now();
    let failure = failure_kind(scalar(&mut session));
    assert_eq!(failure.cause, ErrorKind::ChildExited);
    assert_eq!(failure.delivery, Delivery::MayHaveApplied);
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(session
        .owner
        .as_ref()
        .unwrap()
        .observed_exit
        .unwrap()
        .success());
    // The independently held write FD is still alive: EOF was not the evidence.
    // Even a subsequent valid-looking reply cannot resurrect a dead candidate.
    use std::io::Write;
    let mut writer = std::fs::File::from(extra_writer);
    writer
        .write_all(b"{\"LatencySamples\":{\"samples\":17}}\n")
        .unwrap();
    let later = failure_kind(scalar(&mut session));
    assert_eq!(later.cause, ErrorKind::ChildExited);
    assert_eq!(later.delivery, Delivery::NotSent);
    assert_eq!(session.state, State::Terminal);
    drop(writer);
    finish(&mut session);
}

#[test]
fn expiry_during_encoding_is_not_sent_and_child_observes_no_bytes() {
    let _serial = serial();
    let mut session = spawn("import sys\nassert sys.stdin.read()==''\n");
    let command = HostCommand::LoadState {
        data: vec![1; 64 * 1024],
        context: crate::plugin::StateContext::Project,
    };
    let expiry = budget_test_hooks::expire_at_check(8);
    let failure = failure_kind(session.exchange(&command, ReplyShape::Success, deadline()));
    drop(expiry);
    assert_eq!(failure.cause, ErrorKind::Deadline);
    assert_eq!(failure.delivery, Delivery::NotSent);
    finish(&mut session);
}

#[test]
fn separate_protocol_expiry_closes_without_shutdown_then_proves_reap() {
    let _serial = serial();
    let mut session = spawn("import sys\nassert sys.stdin.read()==''\n");
    let report = session.dispose_with_deadlines(Instant::now(), deadline());
    assert_eq!(report.shutdown_error, Some(ErrorKind::Deadline));
    assert_eq!(report.shutdown_delivery, Some(Delivery::NotSent));
    assert!(matches!(report.state, CleanupState::Reaped(s) if s.success()));
    assert!(!report.kill_requested);
    let repeated = session.continue_cleanup().unwrap();
    assert_eq!(repeated.shutdown_error, report.shutdown_error);
    assert_eq!(repeated.shutdown_delivery, report.shutdown_delivery);
    assert!(matches!(repeated.state, CleanupState::Reaped(s) if s.success()));
}

#[test]
fn cleanup_continuation_rejects_before_disposal_without_mutation() {
    let _serial = serial();
    let mut session = spawn("import json,sys\nassert json.loads(sys.stdin.readline())=='LatencySamples'\nprint('{\"LatencySamples\":{\"samples\":7}}',flush=True)\nassert json.loads(sys.stdin.readline())=='Shutdown'\nassert sys.stdin.read()==''\n");
    let pid = session.owner.as_ref().unwrap().child.id();
    assert_eq!(
        session.continue_cleanup().unwrap_err(),
        ErrorKind::CleanupNotBegun
    );
    assert_eq!(session.state, State::Ready);
    assert_eq!(session.owner.as_ref().unwrap().child.id(), pid);
    assert!(matches!(
        scalar(&mut session),
        Ok(HostResponse::LatencySamples { samples: 7 })
    ));
    finish(&mut session);
}

#[test]
fn nonwaiting_cleanup_retries_kill_after_expiry_and_preserves_first_evidence() {
    let _serial = serial();
    let mut session = spawn("import time\ntime.sleep(30)\n");
    let pid = session.owner.as_ref().unwrap().child.id();
    let first = session.dispose_with_deadlines(Instant::now(), Instant::now());
    assert!(matches!(first.state, CleanupState::Pending));
    assert_eq!(first.shutdown_error, Some(ErrorKind::Deadline));
    // Test-only clock positioning; no scheduling threshold is an acceptance gate.
    session.owner.as_mut().unwrap().graceful_until = Some(Instant::now());
    let first_grace = session.owner.as_ref().unwrap().graceful_until;
    for fault in [
        pump::test_hooks::Fault::Kill,
        pump::test_hooks::Fault::Kill,
        pump::test_hooks::Fault::Wait,
    ] {
        pump::test_hooks::arm(fault);
        let report = session.continue_cleanup().unwrap();
        assert!(matches!(report.state, CleanupState::Pending));
        assert_eq!(report.kill_error, Some(libc::EPERM));
        assert_eq!(session.owner.as_ref().unwrap().child.id(), pid);
        assert_eq!(session.owner.as_ref().unwrap().graceful_until, first_grace);
    }
    let killed = session.continue_cleanup().unwrap();
    assert!(matches!(killed.state, CleanupState::Pending));
    assert!(killed.kill_requested); // One successful kill is not yet a reap result.
    assert_eq!(killed.kill_error, Some(libc::EPERM));
    assert_eq!(killed.wait_error, Some(libc::EIO));
    assert_eq!(killed.shutdown_error, Some(ErrorKind::Deadline));
    let end = deadline();
    let final_report = loop {
        let report = session.continue_cleanup().unwrap();
        if matches!(report.state, CleanupState::Reaped(_)) {
            break report;
        }
        assert!(Instant::now() < end, "mock reap watchdog");
        std::thread::sleep(Duration::from_millis(1));
    };
    assert!(final_report.kill_requested);
    assert_eq!(final_report.kill_error, Some(libc::EPERM));
    assert_eq!(final_report.wait_error, Some(libc::EIO));
    let cached = session.dispose_checked(Instant::now());
    assert!(matches!(cached.state, CleanupState::Reaped(_)));
    assert_eq!(cached.kill_error, final_report.kill_error);
    assert_eq!(cached.wait_error, final_report.wait_error);
    assert_eq!(cached.shutdown_error, final_report.shutdown_error);
}

#[test]
fn pending_cleanup_evidence_travels_with_abandoned_owner() {
    let _serial = serial();
    let mut session = spawn("import time\ntime.sleep(30)\n");
    let pid = session.owner.as_ref().unwrap().child.id();
    let _ = session.dispose_with_deadlines(Instant::now(), Instant::now());
    session.owner.as_mut().unwrap().graceful_until = Some(Instant::now());
    pump::test_hooks::arm(pump::test_hooks::Fault::Wait);
    let report = session.continue_cleanup().unwrap();
    assert_eq!(report.wait_error, Some(libc::EIO));
    drop(session);
    assert_eq!(quarantine::abandoned_pid(), Some(pid));
    pump::test_hooks::arm(pump::test_hooks::Fault::ReapPanic);
    assert!(
        std::panic::catch_unwind(|| FreshHelperSession::cleanup_abandoned(deadline())).is_err()
    );
    assert_eq!(quarantine::abandoned_pid(), Some(pid));
    let final_report = FreshHelperSession::cleanup_abandoned(deadline());
    assert!(matches!(final_report.state, CleanupState::Reaped(_)));
    assert_eq!(final_report.wait_error, Some(libc::EIO));
    assert_eq!(final_report.shutdown_error, Some(ErrorKind::Deadline));
}

#[path = "client_tests.rs"]
mod client_tests;
