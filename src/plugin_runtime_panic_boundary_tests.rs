// Included in the existing private runtime tests so the isolated regression
// uses the same MockBackend and unchanged two-second receive helpers.
mod panic_boundary_regression {
    use super::*;
    use std::process::{Child, Command, ExitStatus, Stdio};

    const SELECTOR: &str = "plugins::plugin_runtime::tests::panic_boundary_regression::ordinary_panic_hook_gate_isolated_child";
    const CHILD_ENV: &str = "CITRUS_RUNTIME_PANIC_BOUNDARY_CHILD";
    const CHILD_VALUE: &str = "single-test-ordinary-panic-hook-gate-v1";
    const RETENTION_SELECTOR: &str = "plugins::plugin_runtime::tests::panic_boundary_regression::pending_child_cleanup_retains_actual_owner_without_replacement";
    const RETENTION_CHILD_VALUE: &str = "pending-owner-actual-child-fixture-v1";

    struct OwnedChild {
        child: Child,
        retained_failure: Option<String>,
    }

    struct ChildOwnerSlot(Option<OwnedChild>);

    impl ChildOwnerSlot {
        const fn new() -> Self {
            Self(None)
        }

        fn spawn(&mut self, command: &mut Command) -> Result<(), String> {
            if let Some(owner) = &self.0 {
                return Err(format!(
                    "isolated test child already owned: pid={}; retained_failure={:?}",
                    owner.child.id(),
                    owner.retained_failure,
                ));
            }
            let child = command.spawn().map_err(|error| error.to_string())?;
            self.0 = Some(OwnedChild {
                child,
                retained_failure: None,
            });
            Ok(())
        }

        fn identity(&self) -> Option<(u32, Option<String>)> {
            self.0
                .as_ref()
                .map(|owner| (owner.child.id(), owner.retained_failure.clone()))
        }

        fn poll(&mut self) -> Result<Option<ExitStatus>, String> {
            let owner = self.0.as_mut().ok_or("isolated child owner is absent")?;
            match owner.child.try_wait().map_err(|error| error.to_string())? {
                Some(status) => {
                    // Only a concrete OS reap releases the actual Child owner.
                    self.0.take();
                    Ok(Some(status))
                }
                None => Ok(None),
            }
        }

        fn record_cleanup(&mut self, context: &str, observation: &CleanupObservation) -> String {
            let owner = self.0.as_mut().expect("cleanup borrows the retained owner");
            let detail = format!(
                "{context}; pid={}; cleanup={observation:?}",
                owner.child.id(),
            );
            if matches!(observation.reap, ReapObservation::Reaped(_)) {
                self.0.take();
            } else {
                owner.retained_failure.get_or_insert_with(|| detail.clone());
            }
            detail
        }
    }

    // These two slots belong only to this fixture's test process. A test panic
    // cannot drop a pending Child, and another invocation cannot replace it.
    static HOOK_CHILD_OWNER: Mutex<ChildOwnerSlot> = Mutex::new(ChildOwnerSlot::new());
    static RETENTION_TEST_OWNER: Mutex<ChildOwnerSlot> = Mutex::new(ChildOwnerSlot::new());

    fn lock_owner(slot: &Mutex<ChildOwnerSlot>) -> std::sync::MutexGuard<'_, ChildOwnerSlot> {
        // A previous panic remains a test failure; poison must not discard its
        // actual OS owner or make the retained identity inaccessible.
        slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[derive(Debug)]
    enum ReapObservation {
        Reaped(ExitStatus),
        WaitError(String),
        Pending,
    }

    #[derive(Debug)]
    struct CleanupObservation {
        kill: Result<(), String>,
        reap: ReapObservation,
    }

    struct HookState {
        entered: AtomicBool,
        released: AtomicBool,
    }

    type Hook = dyn Fn(&panic::PanicHookInfo<'_>) + Send + Sync + 'static;

    struct HookGate {
        state: Arc<HookState>,
        previous: Arc<Hook>,
    }

    impl HookGate {
        fn install(expected_payload: &'static str) -> Self {
            let state = Arc::new(HookState {
                entered: AtomicBool::new(false),
                released: AtomicBool::new(false),
            });
            let previous: Arc<Hook> = Arc::from(panic::take_hook());
            let hook_previous = Arc::clone(&previous);
            let hook_state = Arc::clone(&state);
            panic::set_hook(Box::new(move |info| {
                if thread::current().name() == Some("citrus-plugin-chain")
                    && panic_payload_message(info.payload()) == expected_payload
                {
                    hook_state.entered.store(true, Ordering::Release);
                    while !hook_state.released.load(Ordering::Acquire) {
                        thread::park_timeout(Duration::from_millis(1));
                    }
                } else {
                    hook_previous(info);
                }
            }));
            Self { state, previous }
        }

        fn release(&self) {
            self.state.released.store(true, Ordering::Release);
        }
    }

    impl Drop for HookGate {
        fn drop(&mut self) {
            // Declare this guard after the chain: even a test assertion unwind
            // releases the blocked worker before its worker guard is dropped.
            self.release();
            // set_hook is forbidden while this thread is panicking. That error
            // case remains inside this one isolated test process, never the suite.
            if !thread::panicking() {
                let previous = Arc::clone(&self.previous);
                panic::set_hook(Box::new(move |info| previous(info)));
            }
        }
    }

    fn assert_hook_holds_before_unwind(trace: &panic_boundary::Trace, chain: &mut PluginChain) {
        let phase = trace.snapshot();
        assert!(phase.panic_requested_us.is_some(), "{phase:?}");
        assert!(phase.unwind_started_us.is_none(), "{phase:?}");
        assert!(phase.catch_returned_us.is_none(), "{phase:?}");
        assert!(phase.terminal_published_us.is_none(), "{phase:?}");
        assert!(!chain.guard.join.as_ref().unwrap().is_finished());
        let mut left = [0.0; 64];
        let mut right = [0.0; 64];
        assert!(matches!(
            chain.audio.try_receive(&mut left, &mut right),
            ReceiveStatus::Empty
        ));
        assert!(chain.control.try_next_parameter_edit_receipt().is_none());
        eprintln!(
            "ordinary-hook-held {}",
            trace.describe(
                &chain.control.metrics,
                &chain.guard,
                chain.control.outstanding_parameter_edits(),
            )
        );
    }

    fn assert_all_boundaries(trace: &panic_boundary::Trace) {
        wait_until(|| trace.snapshot().terminal_published_us.is_some());
        let phase = trace.snapshot();
        let requested = phase.panic_requested_us.unwrap();
        let unwind = phase.unwind_started_us.unwrap();
        let caught = phase.catch_returned_us.unwrap();
        let published = phase.terminal_published_us.unwrap();
        assert!(
            requested <= unwind
                && unwind <= caught
                && caught <= published
                && published <= phase.elapsed_us,
            "{phase:?}"
        );
        eprintln!("ordinary-hook-released {phase:?}");
    }

    fn exercise_isolated_hook_gate() {
        for terminal in [
            panic_boundary::Terminal::ProcessOutput {
                epoch: INITIAL_TRANSPORT_EPOCH,
                sequence: 1,
            },
            panic_boundary::Terminal::ParameterReceipt(303),
        ] {
            let trace = panic_boundary::Trace::new(terminal);
            let worker_trace = Arc::clone(&trace);
            let process = match terminal {
                panic_boundary::Terminal::ProcessOutput { .. } => MockProcess::Panic,
                panic_boundary::Terminal::ParameterReceipt(_) => MockProcess::Gain(1.0),
            };
            let mut chain = PluginChain::spawn_with_backend_factory(
                move || {
                    let mut backend = MockBackend::new("ordinary hook gate", process);
                    backend.panic_boundary = Some(worker_trace);
                    vec![BackendSlot::new(Box::new(backend))]
                },
                config(),
            )
            .unwrap();
            wait_until(|| chain.control.plugin_latency_snapshot().is_some());
            let gate = HookGate::install(match terminal {
                panic_boundary::Terminal::ProcessOutput { .. } => "mock exploded",
                panic_boundary::Terminal::ParameterReceipt(_) => "mock parameter setter exploded",
            });
            let input = [0.25; 8];
            if let panic_boundary::Terminal::ParameterReceipt(edit_id) = terminal {
                assert!(chain.audio.try_set_parameter_tagged(
                    0,
                    u32::MAX - 1,
                    0.5,
                    ParameterEditId::new(edit_id).unwrap(),
                ));
            }
            assert!(matches!(
                chain.audio.try_submit(&input, &input),
                SubmitStatus::Submitted { .. }
            ));
            wait_until(|| gate.state.entered.load(Ordering::Acquire));
            assert_hook_holds_before_unwind(&trace, &mut chain);
            gate.release();
            if let panic_boundary::Terminal::ParameterReceipt(_) = terminal {
                assert!(matches!(
                    wait_for_parameter_edit_receipt_with_timeout_context(&mut chain.control, || {
                        trace.describe(
                            &chain.audio.metrics,
                            &chain.guard,
                            chain.audio.parameter_edit_admission.load(Ordering::Acquire),
                        )
                    }),
                    ParameterEditReceipt::Failed {
                        edit_id,
                        reason: ParameterEditFailureReason::BackendPanicked,
                        ..
                    } if edit_id.get() == 303
                ));
            }
            let (left, right) = receive_with_timeout_context(&mut chain.audio, input.len(), || {
                trace.describe(
                    &chain.control.metrics,
                    &chain.guard,
                    chain.control.outstanding_parameter_edits(),
                )
            });
            assert_eq!(&left[..input.len()], &input);
            assert_eq!(&right[..input.len()], &input);
            assert_all_boundaries(&trace);
            assert!(chain.control.stats().faults >= 1);
            drop(gate);
            chain.guard.shutdown();
        }
    }

    fn terminate_and_reap(child: &mut Child) -> CleanupObservation {
        let kill = child.kill().map_err(|error| error.to_string());
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    return CleanupObservation {
                        kill,
                        reap: ReapObservation::Reaped(status),
                    };
                }
                Err(error) => {
                    return CleanupObservation {
                        kill,
                        reap: ReapObservation::WaitError(error.to_string()),
                    };
                }
                Ok(None) if Instant::now() >= deadline => {
                    return CleanupObservation {
                        kill,
                        reap: ReapObservation::Pending,
                    };
                }
                Ok(None) => thread::sleep(Duration::from_millis(1)),
            }
        }
    }

    fn cleanup_owned(slot: &Mutex<ChildOwnerSlot>, context: &str) -> (String, CleanupObservation) {
        let mut slot = lock_owner(slot);
        let observation = terminate_and_reap(&mut slot.0.as_mut().unwrap().child);
        let detail = slot.record_cleanup(context, &observation);
        (detail, observation)
    }

    fn isolated_command(selector: &str, marker: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([selector, "--exact", "--test-threads=1", "--nocapture"])
            .env(CHILD_ENV, marker);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        command
    }

    #[test]
    fn pending_child_cleanup_retains_actual_owner_without_replacement() {
        if std::env::var(CHILD_ENV).as_deref() == Ok(RETENTION_CHILD_VALUE) {
            let expected = [
                RETENTION_SELECTOR,
                "--exact",
                "--test-threads=1",
                "--nocapture",
            ];
            assert_eq!(std::env::args().skip(1).collect::<Vec<_>>(), expected);
            // The owned parent's piped stdin keeps this actual child alive;
            // no plug-in, device, hook or artificial successful exit is used.
            let mut input = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut input).unwrap();
            return;
        }
        let mut command = isolated_command(RETENTION_SELECTOR, RETENTION_CHILD_VALUE);
        command.stdin(Stdio::piped());
        let spawned = lock_owner(&RETENTION_TEST_OWNER).spawn(&mut command);
        spawned.expect("actual retention fixture child must start");
        let pid = lock_owner(&RETENTION_TEST_OWNER).identity().unwrap().0;
        let alive = lock_owner(&RETENTION_TEST_OWNER).poll();
        assert!(matches!(alive, Ok(None)), "{alive:?}");
        let mut original_failure = None;
        for reap in [
            ReapObservation::WaitError("injected cleanup wait failure".into()),
            ReapObservation::Pending,
        ] {
            // Inject only the cleanup observation, not an exit status. The
            // Child and its observed live OS identity are real throughout.
            let observation = CleanupObservation {
                kill: Err("injected kill failure; child deliberately remains live".into()),
                reap,
            };
            let detail = lock_owner(&RETENTION_TEST_OWNER)
                .record_cleanup("injected failure disposition", &observation);
            original_failure.get_or_insert_with(|| detail.clone());
            let identity = lock_owner(&RETENTION_TEST_OWNER).identity().unwrap();
            assert_eq!(identity.0, pid, "{detail}");
            assert_eq!(identity.1.as_deref(), original_failure.as_deref());
            let replacement = lock_owner(&RETENTION_TEST_OWNER)
                .spawn(&mut Command::new("must-not-spawn-over-a-pending-owner"));
            let refused = replacement.unwrap_err();
            assert!(refused.contains(&format!("already owned: pid={pid}")));
            let alive = lock_owner(&RETENTION_TEST_OWNER).poll();
            assert!(matches!(alive, Ok(None)), "{alive:?}; {detail}");
            assert_eq!(lock_owner(&RETENTION_TEST_OWNER).identity().unwrap().0, pid);
        }
        let (detail, observation) = cleanup_owned(&RETENTION_TEST_OWNER, "actual final cleanup");
        assert!(observation.kill.is_ok(), "{detail}");
        match observation.reap {
            ReapObservation::Reaped(status) => assert!(!status.success(), "{detail}"),
            ReapObservation::WaitError(error) => panic!("{detail}; wait_error={error}"),
            ReapObservation::Pending => panic!("{detail}; actual owner remains retained"),
        }
        assert!(lock_owner(&RETENTION_TEST_OWNER).identity().is_none());
    }

    #[test]
    fn ordinary_panic_hook_gate_isolated_child() {
        if std::env::var(CHILD_ENV).as_deref() == Ok(CHILD_VALUE) {
            let expected = [SELECTOR, "--exact", "--test-threads=1", "--nocapture"];
            assert_eq!(std::env::args().skip(1).collect::<Vec<_>>(), expected);
            exercise_isolated_hook_gate();
            return;
        }
        let mut command = isolated_command(SELECTOR, CHILD_VALUE);
        // This is only the new isolated regression's process envelope. The two
        // original pipeline waits retain their existing two-second deadlines.
        let spawned = lock_owner(&HOOK_CHILD_OWNER).spawn(&mut command);
        spawned.expect("isolated hook test must start without replacing an owner");
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let poll = lock_owner(&HOOK_CHILD_OWNER).poll();
            match poll {
                Ok(Some(status)) => {
                    assert!(status.success(), "isolated hook test failed: {status:?}");
                    break;
                }
                Err(error) => {
                    let (cleanup, _) = cleanup_owned(&HOOK_CHILD_OWNER, &error);
                    panic!("isolated hook test wait failed: {error}; {cleanup}");
                }
                Ok(None) if Instant::now() >= deadline => {
                    let (cleanup, _) =
                        cleanup_owned(&HOOK_CHILD_OWNER, "15 second envelope expired");
                    panic!("isolated hook test exceeded its 15 second envelope; {cleanup}");
                }
                Ok(None) => thread::sleep(Duration::from_millis(1)),
            }
        }
    }
}
