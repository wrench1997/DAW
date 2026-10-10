//! Thread/lifetime proof using allocating, locking COM mocks, not native plug-ins or RT tests.
use super::super::borrowed_processor_fixture::{borrow_for_test, BorrowedProcessor};
use super::*;
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    sync::mpsc,
    time::{Duration, Instant},
};

// These objects are constructed on the worker's own stack and never cross a thread boundary.
// The mock's prepared shape is no inputs, one mono output, f32, maximum 16 samples.
fn invoke_worker_local(token: &mut BorrowedProcessor<'_>) -> (tresult, [f32; 16]) {
    let mut samples = [0.0; 16];
    let mut channels = [samples.as_mut_ptr()];
    let mut bus: AudioBusBuffers = unsafe { std::mem::zeroed() };
    bus.numChannels = 1;
    bus.__field0.channelBuffers32 = channels.as_mut_ptr();
    let mut data: ProcessData = unsafe { std::mem::zeroed() };
    data.processMode = ProcessModes_::kRealtime as i32;
    data.symbolicSampleSize = SymbolicSampleSizes_::kSample32 as i32;
    data.numSamples = 16;
    data.numOutputs = 1;
    data.outputs = &mut bus;
    // SAFETY: all nested writable pointers refer to the exclusive, live worker stack above;
    // absent input/parameter/event/context pointers have matching zero counts. The fixture
    // is already prepared/active/processing for exactly this format and bus shape. Nothing
    // owns/releases COM queues on the worker. Only the synchronous call can access pointers.
    let result = unsafe { token.process(&mut data) };
    (result, samples)
}

fn assert_main_owned_calls(trace: &Trace, main: ThreadId, workers: &[ThreadId]) {
    for call in trace.calls() {
        if call.method == "process" {
            assert!(workers.contains(&call.thread), "{call:?}");
            assert_ne!(call.thread, main);
        } else {
            assert_eq!(call.thread, main, "owning/control/lifecycle call: {call:?}");
        }
    }
}

#[test]
fn borrowed_processor_same_thread_baseline_cannot_dispatch_during_main_hold() {
    let trace = Trace::default();
    let state = Arc::new(MockState::default());
    let mut owner = plugin_fixture(&trace, &state);
    owner.start_processing().unwrap();
    let mut queued_audio = AudioBuffers::new(0, 1, 16, 48000.0);
    let start = Instant::now();
    // The same-thread dispatcher cannot execute its pending Process until this hold ends.
    thread::sleep(Duration::from_millis(350));
    assert!(start.elapsed() >= Duration::from_millis(350));
    assert!(!trace
        .methods()
        .iter()
        .any(|(_, method)| *method == "process"));
    owner.process(&mut queued_audio).unwrap();
    assert!(trace.methods().contains(&("component", "process")));
    trace.assert_thread(thread::current().id());
}

#[test]
fn borrowed_processor_completes_during_350ms_main_hold_and_rejoins_before_alias_teardown() {
    let main = thread::current().id();
    let trace = Trace::default();
    let state = Arc::new(MockState::default());
    let mut owner = plugin_fixture(&trace, &state);
    owner.start_processing().unwrap();
    let before = trace.calls().len();
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let (go_tx, go_rx) = mpsc::sync_channel(1);
    let (done_tx, done_rx) = mpsc::sync_channel(1);
    let held = Arc::new(AtomicBool::new(false));
    let worker_held = held.clone();
    let mut token = borrow_for_test(&mut owner).unwrap();
    let worker_id = thread::scope(|scope| {
        let worker = scope.spawn(move || {
            ready_tx.send(thread::current().id()).unwrap();
            go_rx.recv().unwrap();
            let output = invoke_worker_local(&mut token);
            done_tx
                .send((output, worker_held.load(Ordering::Acquire)))
                .unwrap();
        });
        let worker_id = ready_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let start = Instant::now();
        held.store(true, Ordering::Release);
        go_tx.send(()).unwrap();
        // Simulated GUI-side hold after initialization. No timing threshold for DSP speed.
        thread::sleep(Duration::from_millis(350));
        let (((result, samples), completed_while_held), elapsed) = (
            done_rx.recv_timeout(Duration::from_secs(3)).unwrap(),
            start.elapsed(),
        );
        assert!(elapsed >= Duration::from_millis(350));
        assert!(completed_while_held);
        assert!(held.load(Ordering::Acquire));
        assert_eq!(result, kResultOk);
        assert_eq!(samples, [1.0; 16]);
        held.store(false, Ordering::Release);
        worker.join().unwrap();
        worker_id
    });
    assert_eq!(&trace.methods()[before..], &[("component", "process")]);
    assert!(!owner
        .control
        ._host_app
        .is_data_exchange_in_process_for_test());
    // The test-only path intentionally leaves runtime orchestration untouched. It cannot
    // be substituted for production processing or used to claim native acknowledgment.
    assert!(!owner.runtime.has_processed_audio);
    owner.stop_processing().unwrap();
    drop(owner);
    assert_eq!(
        trace.lifecycle(),
        [("component", "initialize"), ("component", "terminate")]
    );
    let methods = trace.methods();
    let module_drop = methods
        .iter()
        .position(|x| *x == ("module", "drop"))
        .unwrap();
    for event in [
        ("component", "terminate"),
        ("component", "drop"),
        ("factory", "drop"),
    ] {
        assert!(
            methods.iter().position(|x| *x == event).unwrap() < module_drop,
            "{event:?}"
        );
    }
    assert_main_owned_calls(&trace, main, &[worker_id]);
}

#[test]
fn borrowed_processor_gate_spans_sdk_call_and_clears_on_success_and_tresult_error() {
    for fail in [false, true] {
        let main = thread::current().id();
        let trace = Trace::default();
        let state = Arc::new(MockState::default());
        state.fail_process.store(fail, Ordering::Release);
        let mut owner = plugin_fixture(&trace, &state);
        owner.start_processing().unwrap();
        // This owning host reference remains on main and is released there after join.
        let host = owner.control._host_app.clone();
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let release_rx = Mutex::new(release_rx);
        *state.process_entry_callback.lock().unwrap() = Some(Box::new(move || {
            let _ = entered_tx.send(());
            let _ = release_rx.lock().unwrap().recv();
        }));
        let mut token = borrow_for_test(&mut owner).unwrap();
        let (id, result) = thread::scope(|scope| {
            let worker =
                scope.spawn(move || (thread::current().id(), invoke_worker_local(&mut token).0));
            let release_on_error = ReleaseOnDrop(Some(release_tx));
            entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
            assert!(host.is_data_exchange_in_process_for_test());
            drop(release_on_error);
            worker.join().unwrap()
        });
        assert_eq!(result, if fail { kResultFalse } else { kResultOk });
        assert!(!host.is_data_exchange_in_process_for_test());
        drop(host);
        owner.stop_processing().unwrap();
        drop(owner);
        assert_main_owned_calls(&trace, main, &[id]);
    }
}

#[test]
fn borrowed_processor_worker_rust_panic_joins_before_owner_is_reused() {
    let main = thread::current().id();
    let trace = Trace::default();
    let state = Arc::new(MockState::default());
    let mut owner = plugin_fixture(&trace, &state);
    owner.start_processing().unwrap();
    let mut token = borrow_for_test(&mut owner).unwrap();
    let (id_tx, id_rx) = mpsc::sync_channel(1);
    let result = thread::scope(|scope| {
        scope
            .spawn(move || {
                id_tx.send(thread::current().id()).unwrap();
                assert_eq!(invoke_worker_local(&mut token).0, kResultOk);
                panic!("Rust worker panic after COM returned");
            })
            .join()
    });
    assert!(result.is_err());
    assert!(!owner
        .control
        ._host_app
        .is_data_exchange_in_process_for_test());
    owner.stop_processing().unwrap();
    drop(owner);
    assert_main_owned_calls(&trace, main, &[id_rx.recv().unwrap()]);
}

struct ReleaseOnDrop(Option<mpsc::SyncSender<()>>);
impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}

#[test]
fn borrowed_processor_main_rust_unwind_still_joins_and_pins_module() {
    let main = thread::current().id();
    let trace = Trace::default();
    let state = Arc::new(MockState::default());
    let mut owner = plugin_fixture(&trace, &state);
    owner.start_processing().unwrap();
    let completed = Arc::new(AtomicBool::new(false));
    let worker_completed = completed.clone();
    let (id_tx, id_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let result = catch_unwind(AssertUnwindSafe(|| {
        let mut token = borrow_for_test(&mut owner).unwrap();
        thread::scope(|scope| {
            scope.spawn(move || {
                id_tx.send(thread::current().id()).unwrap();
                release_rx.recv().unwrap();
                assert_eq!(invoke_worker_local(&mut token).0, kResultOk);
                worker_completed.store(true, Ordering::Release);
            });
            let _release_during_rust_unwind = ReleaseOnDrop(Some(release_tx));
            panic!("main Rust panic outside COM");
        });
    }));
    assert!(result.is_err());
    assert!(completed.load(Ordering::Acquire));
    assert!(!trace
        .methods()
        .iter()
        .any(|x| x.1 == "terminate" || *x == ("module", "drop")));
    assert!(!owner
        .control
        ._host_app
        .is_data_exchange_in_process_for_test());
    owner.stop_processing().unwrap();
    drop(owner);
    assert_main_owned_calls(&trace, main, &[id_rx.recv().unwrap()]);
}

#[test]
fn borrowed_processor_blocked_worker_prevents_scope_return_and_module_drop() {
    let trace = Trace::default();
    let state = Arc::new(MockState::default());
    let mut owner = plugin_fixture(&trace, &state);
    owner.start_processing().unwrap();
    let returned = Arc::new(AtomicBool::new(false));
    let observer_returned = returned.clone();
    let observer_trace = trace.clone();
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let observer = thread::spawn(move || {
        let _always_release = ReleaseOnDrop(Some(release_tx));
        entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        thread::sleep(Duration::from_millis(100));
        assert!(!observer_returned.load(Ordering::Acquire));
        assert!(!observer_trace
            .methods()
            .iter()
            .any(|x| x.1 == "terminate" || *x == ("module", "drop")));
    });
    let mut token = borrow_for_test(&mut owner).unwrap();
    thread::scope(|scope| {
        scope.spawn(move || {
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            assert_eq!(invoke_worker_local(&mut token).0, kResultOk);
        }); // No manual join: scope itself must join even an otherwise abandoned handle.
    });
    returned.store(true, Ordering::Release);
    observer.join().unwrap();
    owner.stop_processing().unwrap();
    drop(owner);
    assert!(trace.methods().contains(&("module", "drop")));
}

#[test]
fn borrowed_processor_readiness_refusals_do_not_call_or_retain_com() {
    for mode in [
        "inactive",
        "output_fault",
        "wrong_thread",
        "missing_setup",
        "missing_data",
        "active_call",
    ] {
        let trace = Trace::default();
        let state = Arc::new(MockState::default());
        let mut owner = plugin_fixture(&trace, &state);
        let main = owner.control.control_thread;
        if mode != "inactive" {
            owner.start_processing().unwrap();
        }
        if mode == "output_fault" {
            owner
                .runtime
                .parameter_output_fault
                .store(true, Ordering::Release);
        }
        if mode == "wrong_thread" {
            owner.control.control_thread = thread::spawn(|| thread::current().id()).join().unwrap();
        }
        if mode == "missing_setup" {
            owner.control.applied_setup = None;
        }
        if mode == "missing_data" {
            owner.runtime.process_data = None;
        }
        // An independently retained host observer stays on main. Its existing gate can
        // model an already-active SDK interval without ever moving host ownership.
        let host = owner.control._host_app.clone();
        let mut gate = host.data_exchange_process_gate();
        let active = if mode == "active_call" {
            Some(gate.enter())
        } else {
            None
        };
        let before = trace.calls().len();
        assert!(borrow_for_test(&mut owner).is_err());
        assert_eq!(trace.calls().len(), before);
        drop(active);
        // Restore only this synthetic thread-identity fixture before normal main teardown.
        // A latched output fault is never cleared, even in this test.
        owner.control.control_thread = main;
    }
}
