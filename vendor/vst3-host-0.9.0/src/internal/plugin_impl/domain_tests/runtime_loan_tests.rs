//! Full-kernel scoped-runtime evidence with allocating, locking COM mocks.
//! Completion during a held UI call is not an audio deadline or native-plug-in guarantee.
//! Rust unwind tests never panic through a COM vtable.
use super::super::domain_session::{with_runtime_loan_for_test, RuntimeLoan};
use super::*;
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    sync::mpsc,
    time::{Duration, Instant},
};

const WATCHDOG: Duration = Duration::from_secs(3);

fn process_on_worker(owner: &mut PluginImpl, buffers: &mut AudioBuffers) -> Result<()> {
    with_runtime_loan_for_test(owner, |_, mut loan| {
        thread::scope(|scope| scope.spawn(move || loan.process(buffers)).join().unwrap())
    })
}

fn assert_owner_ledger(trace: &Trace, main: ThreadId, workers: &[ThreadId]) {
    for call in trace.calls() {
        if call.method == "process" {
            assert!(workers.contains(&call.thread), "{call:?}");
            assert_ne!(call.thread, main);
        } else {
            assert_eq!(call.thread, main, "owning/control/lifecycle call: {call:?}");
        }
    }
}

fn assert_final_teardown(trace: &Trace) {
    let calls = trace.methods();
    let at = |target| calls.iter().position(|call| *call == target).unwrap();
    for target in [
        ("component", "terminate"),
        ("component", "drop"),
        ("factory", "drop"),
    ] {
        assert_eq!(calls.iter().filter(|call| **call == target).count(), 1);
        assert!(at(target) < at(("module", "drop")), "{target:?}");
    }
}

#[test]
fn runtime_loan_full_process_completes_before_350ms_ui_getter_releases() {
    let main = thread::current().id();
    let trace = Trace::default();
    let state = Arc::new(MockState::default());
    let mut owner = plugin_fixture(&trace, &state);
    owner.start_processing().unwrap();
    let host = owner.control._host_app.clone();
    let (ui_entered_tx, ui_entered_rx) = mpsc::sync_channel(1);
    let (completed_tx, completed_rx) = mpsc::sync_channel(1);
    let completed_rx = Mutex::new(completed_rx);
    let held = Arc::new(AtomicBool::new(false));
    let held_in_ui = held.clone();
    let held_in_worker = held.clone();
    let completed_before_release = Arc::new(AtomicBool::new(false));
    let completed_in_ui = completed_before_release.clone();
    let long_enough = Arc::new(AtomicBool::new(false));
    let long_enough_in_ui = long_enough.clone();
    *state.control_get_callback.lock().unwrap() = Some(Box::new(move || {
        // Do not panic in this COM callback, including on a watchdog failure.
        held_in_ui.store(true, Ordering::Release);
        let start = Instant::now();
        let _ = ui_entered_tx.send(());
        thread::sleep(Duration::from_millis(350));
        let completed = completed_rx.lock().unwrap().recv_timeout(WATCHDOG).ok();
        completed_in_ui.store(completed == Some(true), Ordering::Release);
        long_enough_in_ui.store(
            start.elapsed() >= Duration::from_millis(350),
            Ordering::Release,
        );
        held_in_ui.store(false, Ordering::Release);
    }));
    let gate_seen = Arc::new(AtomicBool::new(false));
    let gate_in_sdk = gate_seen.clone();
    let host_in_sdk = host.clone();
    *state.process_callback.lock().unwrap() = Some(Box::new(move || {
        gate_in_sdk.store(
            host_in_sdk.is_data_exchange_in_process_for_test(),
            Ordering::Release,
        );
    }));
    let before = trace.calls().len();
    let mut audio = AudioBuffers::new(0, 1, 16, 48000.0);
    let worker = with_runtime_loan_for_test(&mut owner, |control, mut loan| {
        thread::scope(|scope| {
            let worker = scope.spawn(move || {
                ui_entered_rx.recv_timeout(WATCHDOG).unwrap();
                let result = loan.process(&mut audio);
                completed_tx
                    .send(result.is_ok() && held_in_worker.load(Ordering::Acquire))
                    .unwrap();
                (thread::current().id(), result, audio)
            });
            assert_eq!(control.get_parameter(7)?, 0.0);
            let (id, result, audio) = worker.join().unwrap();
            result?;
            assert_eq!(audio.outputs[0], [1.0; 16]);
            Ok(id)
        })
    })
    .unwrap();
    assert!(long_enough.load(Ordering::Acquire));
    assert!(completed_before_release.load(Ordering::Acquire));
    assert!(!held.load(Ordering::Acquire));
    assert!(gate_seen.load(Ordering::Acquire));
    assert!(!host.is_data_exchange_in_process_for_test());
    assert!(owner.runtime.has_processed_audio);
    assert_eq!(
        &trace.methods()[before..],
        &[
            ("component", "getParamNormalized"),
            ("component", "process")
        ]
    );
    *state.control_get_callback.lock().unwrap() = None;
    *state.process_callback.lock().unwrap() = None;
    drop(host);
    owner.stop_processing().unwrap();
    drop(owner);
    assert_owner_ledger(&trace, main, &[worker]);
    assert_final_teardown(&trace);
}

#[test]
fn runtime_loan_preserves_full_split_parameter_event_transport_and_output_pipeline() {
    let main = thread::current().id();
    let trace = Trace::default();
    let state = Arc::new(MockState::default());
    state.capture_origin.store(true, Ordering::Release);
    let mut owner = plugin_fixture(&trace, &state);
    owner.start_processing().unwrap();
    owner.set_parameter(1, 0.25).unwrap();
    let handler = owner.control.component_handler.as_ref().unwrap().clone();
    assert_eq!(unsafe { handler.performEdit(3, 0.75) }, kResultOk);
    *owner.control.editor_resize.lock().unwrap() = Some((640, 480));
    let output_parameters = owner
        .runtime
        .process_data
        .as_ref()
        .unwrap()
        .output_param_changes
        .clone();
    let output_events = owner.runtime.output_events.clone();
    *state.process_callback.lock().unwrap() = Some(Box::new(move || unsafe {
        let queue =
            vst3::ComRef::from_raw(output_parameters.addParameterData(&77, ptr::null_mut()))
                .unwrap();
        let _ = queue.addPoint(2, 0.625, ptr::null_mut());
        let mut event: Event = std::mem::zeroed();
        event.r#type = kNoteOffEvent as u16;
        event.sampleOffset = 2;
        event.__field0.noteOff = NoteOffEvent {
            channel: 0,
            pitch: 60,
            velocity: 0.0,
            noteId: 99,
            tuning: 0.0,
        };
        let _ = output_events.addEvent(&mut event);
    }));
    let mut buses = BusAudioBuffers::new(&owner.audio_bus_layout().unwrap(), 16, 48000.0);
    let mut flat = AudioBuffers::new(0, 1, 40, 48000.0);
    let before = trace.calls().len();
    let worker = with_runtime_loan_for_test(&mut owner, |mut control, mut loan| {
        thread::scope(|scope| {
            let worker = scope.spawn(move || -> Result<_> {
                loan.queue_parameter_at(2, 0.5, 17)?;
                let note = loan.note_on(MidiChannel::Ch1, 60, 100, 0)?;
                loan.send_note_expression(note, crate::midi::NoteExpressionType::Volume, 0.5, 20)?;
                loan.note_off(note, 33)?;
                loan.send_plugin_event(admission_scalar_event())?;
                loan.set_process_transport(crate::plugin::ProcessTransport {
                    sample_position: 1024,
                    quarter_note_position: 4.0,
                    tempo: 120.0,
                    playing: true,
                    time_sig_numerator: 4,
                    time_sig_denominator: 4,
                })?;
                loan.process(&mut flat)?;
                loan.process_buses(&mut buses)?;
                let (events, lost) = loan.take_output_events_with_loss();
                assert!(!lost);
                assert_eq!(events.len(), 4);
                assert!(events.iter().all(|event| matches!(
                    event.data,
                    crate::midi::PluginEventData::NoteOff { note_id: 99, .. }
                )));
                assert_eq!(loan.take_output_events_with_loss(), (Vec::new(), false));
                assert_eq!(flat.outputs[0], [1.0; 40]);
                assert_eq!(buses.outputs[0].channels[0], [1.0; 16]);
                Ok(thread::current().id())
            });
            assert_eq!(control.get_parameter(1)?, 0.25);
            assert!(control.format_parameter(1, 0.25).is_err());
            assert_eq!(control.take_editor_resize_request(), Some((640, 480)));
            assert!(control.resize_editor(640, 480).is_err());
            #[cfg(target_os = "linux")]
            control.service_run_loop();
            worker.join().unwrap()
        })
    })
    .unwrap();
    assert_eq!(
        *state.processed_parameters.lock().unwrap(),
        [
            vec![(1, 0, 0.25), (3, 0, 0.75)],
            vec![(2, 1, 0.5)],
            vec![],
            vec![]
        ]
    );
    let calls = state.origin_calls.lock().unwrap();
    assert_eq!(
        calls.iter().map(|call| call.frames).collect::<Vec<_>>(),
        [16, 16, 8, 16]
    );
    assert_eq!(
        calls
            .iter()
            .map(|call| call.transport.unwrap().1)
            .collect::<Vec<_>>(),
        [1024, 1040, 1056, 1064]
    );
    assert_eq!(
        calls
            .iter()
            .map(|call| call.events.len())
            .collect::<Vec<_>>(),
        [2, 1, 1, 0]
    );
    assert_eq!(calls[1].events[0].1, 4);
    assert_eq!(calls[2].events[0].1, 1);
    drop(calls);
    assert_eq!(state.processed_event_count.load(Ordering::Acquire), 4);
    assert_eq!(handler.native_edits.snapshot().applied, 1);
    assert_eq!(
        owner.runtime.process_transport.unwrap().sample_position,
        1080
    );
    assert_eq!(
        (0..4)
            .map(|_| owner.runtime.output_param_feedback.pop().unwrap())
            .collect::<Vec<_>>(),
        [(77, 0.625); 4]
    );
    assert!(trace.methods()[before..]
        .iter()
        .all(|(_, method)| !matches!(*method, "addRef" | "release" | "terminate" | "module_drop")));
    assert!(!owner.save_state().unwrap().is_empty());
    *state.process_callback.lock().unwrap() = None;
    drop(handler);
    owner.stop_processing().unwrap();
    drop(owner);
    assert_owner_ledger(&trace, main, &[worker]);
    assert_final_teardown(&trace);
}

#[test]
fn runtime_loan_zero_flush_and_failed_positive_attempt_preserve_restore_history() {
    for fail_positive in [false, true] {
        let state = Arc::new(MockState::default());
        state.capture_origin.store(true, Ordering::Release);
        let mut owner = plugin_fixture(&Trace::default(), &state);
        identify_surge_fixture(&mut owner);
        owner.start_processing().unwrap();
        let handler = owner.control.component_handler.as_ref().unwrap().clone();
        unsafe {
            handler.performEdit(7, 0.5);
        }
        process_on_worker(&mut owner, &mut AudioBuffers::new(0, 1, 0, 48000.0)).unwrap();
        assert_eq!(handler.native_edits.snapshot().applied, 1);
        assert!(!owner.runtime.has_processed_audio);
        assert_eq!(state.origin_calls.lock().unwrap()[0].frames, 0);
        assert!(state.processed_layouts.lock().unwrap()[0].is_empty());
        assert!(owner.preflight_state_restore().is_ok());
        let saved = owner.save_state().unwrap();
        owner.load_state(&saved).unwrap();
        state.fail_process.store(fail_positive, Ordering::Release);
        let result = process_on_worker(&mut owner, &mut AudioBuffers::new(0, 1, 1, 48000.0));
        assert_eq!(result.is_err(), fail_positive);
        assert!(owner.runtime.has_processed_audio);
        assert!(owner.preflight_state_restore().is_err());
        let before = handler.native_edits.snapshot();
        assert!(owner.load_state(&saved).is_err());
        assert_eq!(handler.native_edits.snapshot(), before);
    }
}

#[test]
fn runtime_loan_sdk_error_and_parameter_output_fault_keep_exact_ack_precedence() {
    for (sdk_fails, output_overflow) in [(true, false), (false, true), (true, true)] {
        let state = Arc::new(MockState::default());
        state.no_alloc_process.store(true, Ordering::Release);
        state.fail_process.store(sdk_fails, Ordering::Release);
        state
            .output_points
            .store(if output_overflow { 4097 } else { 0 }, Ordering::Release);
        let mut owner = plugin_fixture(&Trace::default(), &state);
        owner.start_processing().unwrap();
        let handler = owner.control.component_handler.as_ref().unwrap().clone();
        unsafe {
            handler.performEdit(7, 0.5);
        }
        let host = owner.control._host_app.clone();
        let result = process_on_worker(&mut owner, &mut AudioBuffers::new(0, 1, 0, 48000.0));
        if output_overflow {
            assert!(matches!(result, Err(Error::ParameterOutputRejected)));
        } else {
            assert!(matches!(result, Err(Error::ProcessFailed(code)) if code == kResultFalse));
        }
        let status = handler.native_edits.snapshot();
        assert_eq!(status.applied, u64::from(!sdk_fails));
        assert_eq!(status.lost, sdk_fails);
        assert!(!host.is_data_exchange_in_process_for_test());
        assert!(owner.save_state().is_err());
        state.fail_process.store(false, Ordering::Release);
        state.output_points.store(0, Ordering::Release);
        owner.stop_processing().unwrap();
        owner.start_processing().unwrap();
        let result = process_on_worker(&mut owner, &mut AudioBuffers::new(0, 1, 0, 48000.0));
        assert_eq!(result.is_err(), output_overflow);
        assert!(owner.save_state().is_err());
    }
}

#[test]
fn runtime_loan_output_event_loss_is_drainable_without_false_native_failure() {
    let state = Arc::new(MockState::default());
    state.no_alloc_process.store(true, Ordering::Release);
    state.output_event_attempts.store(4097, Ordering::Release);
    let mut owner = plugin_fixture(&Trace::default(), &state);
    owner.runtime.output_events_owned = Arc::new(ArrayQueue::new(4096));
    owner.start_processing().unwrap();
    let handler = owner.control.component_handler.as_ref().unwrap().clone();
    unsafe {
        handler.performEdit(42, 0.25);
    }
    with_runtime_loan_for_test(&mut owner, |_, mut loan| thread::scope(|scope| {
        scope.spawn(move || -> Result<()> {
            loan.process(&mut AudioBuffers::new(0, 1, 1, 48000.0))?;
            let (events, lost) = loan.take_output_events_with_loss();
            assert!(lost);
            assert_eq!(events.len(), 4096);
            for (index, event) in events.iter().enumerate() {
                assert!(matches!(event.data, crate::midi::PluginEventData::NoteOff { note_id, .. } if note_id == index as i32));
            }
            assert_eq!(loan.take_output_events_with_loss(), (Vec::new(), false));
            Ok(())
        }).join().unwrap()
    })).unwrap();
    assert_eq!(state.rejected_output_events.load(Ordering::Acquire), 1);
    let status = handler.native_edits.snapshot();
    assert_eq!(status.applied, 1);
    assert!(!status.lost);
    assert!(owner.save_state().is_ok());
}

#[test]
fn runtime_loan_readiness_refusals_do_not_visit_or_touch_com() {
    for mode in [
        "inactive",
        "output_fault",
        "wrong_thread",
        "missing_setup",
        "missing_data",
        "active_call",
    ] {
        let trace = Trace::default();
        let mut owner = plugin_fixture(&trace, &Arc::new(MockState::default()));
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
        let host = owner.control._host_app.clone();
        let mut gate = host.data_exchange_process_gate();
        let active = if mode == "active_call" {
            Some(gate.enter())
        } else {
            None
        };
        let before = trace.calls().len();
        let mut visited = false;
        assert!(
            with_runtime_loan_for_test(&mut owner, |_, _| {
                visited = true;
                Ok(())
            })
            .is_err(),
            "{mode}"
        );
        assert!(!visited, "{mode}");
        assert_eq!(trace.calls().len(), before, "{mode}");
        drop(active);
        owner.control.control_thread = main;
    }
}

#[test]
fn runtime_loan_bus_validation_rejects_before_sdk_or_native_ack() {
    let trace = Trace::default();
    let mut owner = plugin_fixture(&trace, &Arc::new(MockState::default()));
    owner.start_processing().unwrap();
    let handler = owner.control.component_handler.as_ref().unwrap().clone();
    unsafe {
        handler.performEdit(7, 0.5);
    }
    let before = handler.native_edits.snapshot();
    let mut buses = BusAudioBuffers::new(&owner.audio_bus_layout().unwrap(), 16, 48000.0);
    buses.outputs[0].active = false;
    trace.clear();
    with_runtime_loan_for_test(&mut owner, |_, mut loan| {
        thread::scope(|scope| {
            scope
                .spawn(move || {
                    assert!(loan.process_buses(&mut buses).is_err());
                    Ok(())
                })
                .join()
                .unwrap()
        })
    })
    .unwrap();
    assert_eq!(handler.native_edits.snapshot(), before);
    assert!(trace.calls().is_empty());
    process_on_worker(&mut owner, &mut AudioBuffers::new(0, 1, 16, 48000.0)).unwrap();
    assert_eq!(handler.native_edits.snapshot().applied, 1);
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
fn runtime_loan_gate_spans_the_sdk_and_clears_on_success_and_error() {
    for fail in [false, true] {
        let state = Arc::new(MockState::default());
        state.fail_process.store(fail, Ordering::Release);
        let mut owner = plugin_fixture(&Trace::default(), &state);
        owner.start_processing().unwrap();
        let host = owner.control._host_app.clone();
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let release_rx = Mutex::new(release_rx);
        *state.process_entry_callback.lock().unwrap() = Some(Box::new(move || {
            let _ = entered_tx.send(());
            let _ = release_rx.lock().unwrap().recv_timeout(WATCHDOG);
        }));
        let result = with_runtime_loan_for_test(&mut owner, |_, mut loan| {
            thread::scope(|scope| {
                let worker =
                    scope.spawn(move || loan.process(&mut AudioBuffers::new(0, 1, 16, 48000.0)));
                let release = ReleaseOnDrop(Some(release_tx));
                entered_rx.recv_timeout(WATCHDOG).unwrap();
                assert!(host.is_data_exchange_in_process_for_test());
                drop(release);
                worker.join().unwrap()
            })
        });
        assert_eq!(result.is_err(), fail);
        assert!(!host.is_data_exchange_in_process_for_test());
    }
}

#[test]
fn runtime_loan_worker_rust_panic_rejoins_without_revoking_completed_ack() {
    let main = thread::current().id();
    let trace = Trace::default();
    let mut owner = plugin_fixture(&trace, &Arc::new(MockState::default()));
    owner.start_processing().unwrap();
    let handler = owner.control.component_handler.as_ref().unwrap().clone();
    unsafe {
        handler.performEdit(7, 0.5);
    }
    let (id_tx, id_rx) = mpsc::sync_channel(1);
    with_runtime_loan_for_test(&mut owner, |_, mut loan| {
        let result = thread::scope(|scope| {
            scope
                .spawn(move || {
                    id_tx.send(thread::current().id()).unwrap();
                    loan.process(&mut AudioBuffers::new(0, 1, 0, 48000.0))
                        .unwrap();
                    panic!("controlled Rust worker panic after COM returned");
                })
                .join()
        });
        assert!(result.is_err());
        Ok(())
    })
    .unwrap();
    assert_eq!(handler.native_edits.snapshot().applied, 1);
    assert!(!handler.native_edits.snapshot().lost);
    assert!(!owner
        .control
        ._host_app
        .is_data_exchange_in_process_for_test());
    assert!(owner.save_state().is_ok());
    drop(handler);
    owner.stop_processing().unwrap();
    drop(owner);
    assert_owner_ledger(&trace, main, &[id_rx.recv_timeout(WATCHDOG).unwrap()]);
    assert_final_teardown(&trace);
}

#[test]
fn runtime_loan_main_rust_unwind_joins_before_owner_or_module_reuse() {
    let main = thread::current().id();
    let trace = Trace::default();
    let mut owner = plugin_fixture(&trace, &Arc::new(MockState::default()));
    owner.start_processing().unwrap();
    let completed = Arc::new(AtomicBool::new(false));
    let worker_completed = completed.clone();
    let (id_tx, id_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let result = catch_unwind(AssertUnwindSafe(|| {
        let _: Result<()> = with_runtime_loan_for_test(&mut owner, |_, mut loan| {
            thread::scope(|scope| {
                scope.spawn(move || {
                    id_tx.send(thread::current().id()).unwrap();
                    release_rx.recv_timeout(WATCHDOG).unwrap();
                    loan.process(&mut AudioBuffers::new(0, 1, 16, 48000.0))
                        .unwrap();
                    worker_completed.store(true, Ordering::Release);
                });
                let _release = ReleaseOnDrop(Some(release_tx));
                panic!("controlled main Rust panic outside COM");
            })
        });
    }));
    assert!(result.is_err());
    assert!(completed.load(Ordering::Acquire));
    assert!(!trace
        .methods()
        .iter()
        .any(|call| call.1 == "terminate" || *call == ("module", "drop")));
    assert!(!owner
        .control
        ._host_app
        .is_data_exchange_in_process_for_test());
    owner.stop_processing().unwrap();
    drop(owner);
    assert_owner_ledger(&trace, main, &[id_rx.recv_timeout(WATCHDOG).unwrap()]);
    assert_final_teardown(&trace);
}

#[test]
fn runtime_loan_blocked_worker_pins_scope_and_module_until_join() {
    let trace = Trace::default();
    let mut owner = plugin_fixture(&trace, &Arc::new(MockState::default()));
    owner.start_processing().unwrap();
    let returned = Arc::new(AtomicBool::new(false));
    let observer_returned = returned.clone();
    let observer_trace = trace.clone();
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let observer = thread::spawn(move || {
        let _release = ReleaseOnDrop(Some(release_tx));
        entered_rx.recv_timeout(WATCHDOG).unwrap();
        thread::sleep(Duration::from_millis(100));
        assert!(!observer_returned.load(Ordering::Acquire));
        assert!(!observer_trace
            .methods()
            .iter()
            .any(|call| call.1 == "terminate" || *call == ("module", "drop")));
    });
    with_runtime_loan_for_test(&mut owner, |_, mut loan| {
        thread::scope(|scope| {
            scope.spawn(move || {
                entered_tx.send(()).unwrap();
                release_rx.recv_timeout(WATCHDOG).unwrap();
                loan.process(&mut AudioBuffers::new(0, 1, 16, 48000.0))
                    .unwrap();
            }); // Discarding the handle still cannot detach this scoped worker.
        });
        Ok(())
    })
    .unwrap();
    returned.store(true, Ordering::Release);
    observer.join().unwrap();
    owner.stop_processing().unwrap();
    drop(owner);
    assert_final_teardown(&trace);
}

#[test]
fn runtime_loan_current_and_retired_host_queue_release_counts_are_distinct() {
    // A plug-in may legally retain a host queue. Its own reference is distinct from
    // the loan's borrowed processor authority, which never clones/releases COM.
    for retire_before_release in [false, true] {
        let main = thread::current().id();
        let trace = Trace::default();
        let state = Arc::new(MockState::default());
        let mut owner = plugin_fixture(&trace, &state);
        owner.start_processing().unwrap();
        let input = &owner
            .runtime
            .process_data
            .as_ref()
            .unwrap()
            .input_param_changes;
        let retained = unsafe {
            let raw = input.addParameterData(&7, ptr::null_mut());
            assert!(!raw.is_null());
            assert_eq!(IParamValueQueue::add_ref(raw), 2);
            ComPtr::from_raw(raw).unwrap()
        };
        if retire_before_release {
            owner.stop_processing().unwrap();
            owner.reconfigure(44100.0, 32).unwrap();
            owner.start_processing().unwrap();
        }
        let retained = Mutex::new(Some(retained));
        let releases = Arc::new(Mutex::new(Vec::new()));
        let ledger = releases.clone();
        *state.process_callback.lock().unwrap() = Some(Box::new(move || {
            let retained = retained.lock().unwrap().take().unwrap();
            // SAFETY: the mock obtained this exact owned reference by AddRef above.
            // into_raw transfers it without Release, then release consumes it once.
            // No pointer access follows a final release. IParamValueQueue has the
            // SDK's existing Send + Sync bounds; no new unsafe auto-trait is added.
            // This runs inside the mocked SDK call, as a retaining plug-in may do.
            let remaining = unsafe { IParamValueQueue::release(retained.into_raw()) };
            ledger
                .lock()
                .unwrap()
                .push((thread::current().id(), remaining));
        }));
        let before = trace.calls().len();
        let worker = with_runtime_loan_for_test(&mut owner, |_, mut loan| {
            thread::scope(|scope| {
                scope
                    .spawn(move || -> Result<_> {
                        loan.process(&mut AudioBuffers::new(
                            0,
                            1,
                            0,
                            if retire_before_release {
                                44100.0
                            } else {
                                48000.0
                            },
                        ))?;
                        Ok(thread::current().id())
                    })
                    .join()
                    .unwrap()
            })
        })
        .unwrap();
        assert_ne!(worker, main);
        assert_eq!(
            *releases.lock().unwrap(),
            [(worker, if retire_before_release { 0 } else { 1 })]
        );
        assert_eq!(&trace.methods()[before..], &[("component", "process")]);
        *state.process_callback.lock().unwrap() = None;
        owner.stop_processing().unwrap();
        drop(owner);
        assert_owner_ledger(&trace, main, &[worker]);
        assert_final_teardown(&trace);
    }
}

fn public_plugin(internal: PluginImpl) -> crate::plugin::Plugin {
    crate::plugin::Plugin {
        info: internal.info.clone(),
        compatibility: Vec::new(),
        is_processing: internal.runtime.is_processing,
        sample_rate: internal.runtime.sample_rate,
        block_size: internal.runtime.block_size,
        audio_levels: Arc::new(Mutex::new(crate::audio::AudioLevels::new(1))),
        parameter_change_callback: None,
        audio_callback: None,
        internal: Some(Box::new(internal)),
    }
}

#[test]
fn caller_envelope_calls_back_once_per_successful_caller_block_including_zero() {
    for bus_aware in [false, true] {
        let state = Arc::new(MockState::default());
        let mut internal = plugin_fixture(&Trace::default(), &state);
        internal.start_processing().unwrap();
        let layout = internal.audio_bus_layout().unwrap();
        let mut plugin = public_plugin(internal);
        let callbacks = Arc::new(Mutex::new(Vec::new()));
        let observations = callbacks.clone();
        let meters = plugin.audio_levels.clone();
        let locked_callbacks = Arc::new(AtomicUsize::new(0));
        let lock_observations = locked_callbacks.clone();
        plugin.audio_callback = Some(Box::new(move |levels| {
            if matches!(meters.try_lock(), Err(std::sync::TryLockError::WouldBlock)) {
                lock_observations.fetch_add(1, Ordering::Relaxed);
            }
            observations.lock().unwrap().push((
                thread::current().id(),
                levels.channels[0].peak,
                levels.channels[0].rms,
            ));
        }));
        for frames in [40, 0] {
            if bus_aware {
                plugin
                    .process_bus_audio(&mut BusAudioBuffers::new(&layout, frames, 48000.0))
                    .unwrap();
            } else {
                plugin
                    .process_audio(&mut AudioBuffers::new(0, 1, frames, 48000.0))
                    .unwrap();
            }
        }
        assert_eq!(state.processed_layouts.lock().unwrap().len(), 4);
        assert_eq!(locked_callbacks.load(Ordering::Acquire), 2);
        assert_eq!(
            *callbacks.lock().unwrap(),
            [
                (thread::current().id(), 1.0, 1.0),
                (thread::current().id(), 0.0, 0.0)
            ]
        );
    }
}

#[test]
fn caller_envelope_skips_metering_and_callback_after_partial_sdk_or_output_failure() {
    for bus_aware in [false, true] {
        for failure in ["sdk", "partial", "output"] {
            let state = Arc::new(MockState::default());
            let mut internal = plugin_fixture(&Trace::default(), &state);
            internal.start_processing().unwrap();
            let layout = internal.audio_bus_layout().unwrap();
            if failure == "sdk" {
                state.fail_process.store(true, Ordering::Release);
            }
            if failure == "output" {
                state.no_alloc_process.store(true, Ordering::Release);
                state.output_points.store(4097, Ordering::Release);
            }
            if failure == "partial" {
                let weak_state = Arc::downgrade(&state);
                *state.process_callback.lock().unwrap() = Some(Box::new(move || {
                    if let Some(state) = weak_state.upgrade() {
                        state.fail_process.store(true, Ordering::Release);
                    }
                }));
            }
            let mut plugin = public_plugin(internal);
            let callbacks = Arc::new(AtomicUsize::new(0));
            let observed = callbacks.clone();
            plugin.audio_callback = Some(Box::new(move |_| {
                observed.fetch_add(1, Ordering::Relaxed);
            }));
            let result = if bus_aware {
                let mut audio = BusAudioBuffers::new(&layout, 32, 48000.0);
                audio.outputs[0].channels[0].fill(-0.25);
                let result = plugin.process_bus_audio(&mut audio);
                if failure == "partial" {
                    assert_eq!(audio.outputs[0].channels[0][..16], [1.0; 16]);
                    assert_eq!(audio.outputs[0].channels[0][16..], [-0.25; 16]);
                }
                result
            } else {
                let mut audio = AudioBuffers::new(0, 1, 32, 48000.0);
                audio.outputs[0].fill(-0.25);
                let result = plugin.process_audio(&mut audio);
                if failure == "partial" {
                    assert_eq!(audio.outputs[0][..16], [1.0; 16]);
                    assert_eq!(audio.outputs[0][16..], [-0.25; 16]);
                }
                result
            };
            assert!(result.is_err(), "{failure}");
            assert_eq!(callbacks.load(Ordering::Acquire), 0, "{failure}");
            assert_eq!(
                plugin.get_output_levels().channels[0].peak,
                0.0,
                "{failure}"
            );
        }
    }
}

#[test]
fn caller_envelope_preserves_poisoned_meter_mutex_skip_and_read_recovery() {
    for bus_aware in [false, true] {
        let state = Arc::new(MockState::default());
        let mut internal = plugin_fixture(&Trace::default(), &state);
        internal.start_processing().unwrap();
        let layout = internal.audio_bus_layout().unwrap();
        let mut plugin = public_plugin(internal);
        let levels = plugin.audio_levels.clone();
        let poison = catch_unwind(AssertUnwindSafe(|| {
            let mut levels = levels.lock().unwrap();
            levels.channels[0].peak = 0.375;
            panic!("controlled Rust meter mutex poison outside COM");
        }));
        assert!(poison.is_err());
        let callbacks = Arc::new(AtomicUsize::new(0));
        let observed = callbacks.clone();
        plugin.audio_callback = Some(Box::new(move |_| {
            observed.fetch_add(1, Ordering::Relaxed);
        }));
        if bus_aware {
            plugin
                .process_bus_audio(&mut BusAudioBuffers::new(&layout, 16, 48000.0))
                .unwrap();
        } else {
            plugin
                .process_audio(&mut AudioBuffers::new(0, 1, 16, 48000.0))
                .unwrap();
        }
        assert_eq!(state.processed_layouts.lock().unwrap().len(), 1);
        assert_eq!(callbacks.load(Ordering::Acquire), 0);
        assert_eq!(plugin.get_output_levels().channels[0].peak, 0.375);
        assert!(plugin.audio_levels.is_poisoned());
    }
}

// Positive auto-trait assertion supplements the compile-fail !Sync/!Clone/escape cases.
#[test]
fn runtime_loan_is_send_without_moving_aggregate_ownership() {
    fn requires_send<T: Send>() {}
    requires_send::<RuntimeLoan<'static>>();
}

#[test]
fn runtime_loan_split_native_ack_only_advances_after_each_successful_sdk_call() {
    for fail_second in [false, true] {
        let main = thread::current().id();
        let state = Arc::new(MockState::default());
        let mut owner = plugin_fixture(&Trace::default(), &state);
        owner.start_processing().unwrap();
        let handler = owner.control.component_handler.as_ref().unwrap().clone();
        unsafe {
            handler.performEdit(7, 0.25);
        }
        let observed = Arc::new(Mutex::new(Vec::new()));
        let snapshots = observed.clone();
        // This clone only observes the bounded transport's atomic status. The COM
        // component handler remains on main and is never called by the worker.
        let status_observer = handler.native_edits.clone();
        let (first_sdk_tx, first_sdk_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let release_rx = Mutex::new(release_rx);
        let sdk_calls = AtomicUsize::new(0);
        let latch_ok = Arc::new(AtomicBool::new(true));
        let callback_latch_ok = latch_ok.clone();
        *state.process_callback.lock().unwrap() = Some(Box::new(move || {
            let status = status_observer.snapshot();
            if let Ok(mut snapshots) = snapshots.lock() {
                snapshots.push((status.submitted, status.applied));
            } else {
                callback_latch_ok.store(false, Ordering::Release);
            }
            if sdk_calls.fetch_add(1, Ordering::Relaxed) == 0 {
                let sent = first_sdk_tx.send(()).is_ok();
                let released = release_rx
                    .lock()
                    .ok()
                    .is_some_and(|rx| rx.recv_timeout(WATCHDOG).is_ok());
                if !sent || !released {
                    callback_latch_ok.store(false, Ordering::Release);
                }
            }
        }));
        let result = with_runtime_loan_for_test(&mut owner, |control, mut loan| {
            thread::scope(|scope| {
                let worker =
                    scope.spawn(move || loan.process(&mut AudioBuffers::new(0, 1, 32, 48000.0)));
                // Even a main-side assertion failure releases the parked SDK call
                // before scope joins. Watchdog failures never panic through COM.
                let release = ReleaseOnDrop(Some(release_tx));
                first_sdk_rx.recv_timeout(WATCHDOG).unwrap();
                assert_eq!(thread::current().id(), main);
                assert_eq!(control.get_parameter(7)?, 0.0);
                assert_eq!(handler.native_edits.snapshot().applied, 0);
                assert_eq!(unsafe { handler.performEdit(7, 0.75) }, kResultOk);
                state.fail_process.store(fail_second, Ordering::Release);
                drop(release);
                worker.join().unwrap()
            })
        });
        assert!(latch_ok.load(Ordering::Acquire));
        assert_eq!(result.is_err(), fail_second);
        let status = handler.native_edits.snapshot();
        assert_eq!(status.submitted, 2);
        assert_eq!(status.applied, if fail_second { 1 } else { 2 });
        assert_eq!(status.lost, fail_second);
        if fail_second {
            assert_eq!(*observed.lock().unwrap(), [(1, 0)]);
        } else {
            assert_eq!(*observed.lock().unwrap(), [(1, 0), (2, 1)]);
            assert_eq!(
                *state.processed_parameters.lock().unwrap(),
                [vec![(7, 0, 0.25)], vec![(7, 0, 0.75)]]
            );
        }
        assert!(owner.runtime.pending_param_changes.is_empty());
        assert!(owner.runtime.input_events.is_empty());
        assert!(owner.runtime.chunk_events.is_empty());
        *state.process_callback.lock().unwrap() = None;
    }
}

#[test]
fn runtime_loan_checked_note_and_parameter_admission_preserve_obligations() {
    let mut owner = plugin_fixture(&Trace::default(), &Arc::new(MockState::default()));
    owner.start_processing().unwrap();
    fill_admission_queue(&mut owner, MAX_QUEUED_EVENTS);
    let next_note = owner.runtime.next_note_id;
    with_runtime_loan_for_test(&mut owner, |_, mut loan| {
        thread::scope(|scope| {
            scope
                .spawn(move || -> Result<()> {
                    assert!(matches!(
                        loan.note_on(MidiChannel::Ch1, 60, 100, 0),
                        Err(Error::EventInputRejected)
                    ));
                    assert!(matches!(
                        loan.send_plugin_event(admission_scalar_event()),
                        Err(Error::EventInputRejected)
                    ));
                    assert!(loan.note_on(MidiChannel::Ch1, 128, 100, 0).is_err());
                    assert!(loan
                        .send_note_expression(
                            crate::midi::NoteId(1),
                            crate::midi::NoteExpressionType::Volume,
                            f64::NAN,
                            0
                        )
                        .is_err());
                    assert!(loan.queue_parameter_at(1, f64::NAN, 0).is_err());
                    for id in 0..MAX_PENDING_PARAM_CHANGES {
                        loan.queue_parameter_at(id as u32, 0.5, 0)?;
                    }
                    assert!(matches!(
                        loan.queue_parameter_at(9999, 0.5, 0),
                        Err(Error::ParameterInputRejected)
                    ));
                    Ok(())
                })
                .join()
                .unwrap()
        })
    })
    .unwrap();
    assert_eq!(owner.runtime.next_note_id, next_note);
    assert!(owner.runtime.active_notes.is_empty());
    assert_eq!(
        owner.runtime.pending_param_changes.len(),
        MAX_PENDING_PARAM_CHANGES
    );
    assert_eq!(
        owner.runtime.input_events.events.lock().unwrap().len(),
        MAX_QUEUED_EVENTS
    );
}

#[test]
fn runtime_loan_exact_host_capacity_failure_cleans_staging_without_sdk_or_native_change() {
    let trace = Trace::default();
    let state = Arc::new(MockState::default());
    let mut owner = plugin_fixture(&trace, &state);
    owner.start_processing().unwrap();
    let data = owner.runtime.process_data.as_mut().unwrap();
    data.input_param_changes = ComWrapper::new(ParameterChanges::new(ParameterQueueLimits {
        max_queues: 1,
        max_points: 1,
    }));
    data.process_data.inputParameterChanges = data
        .input_param_changes
        .as_com_ref::<IParameterChanges>()
        .unwrap()
        .as_ptr();
    let handler = owner.control.component_handler.as_ref().unwrap().clone();
    unsafe {
        handler.performEdit(3, 0.5);
    }
    let before = handler.native_edits.snapshot();
    trace.clear();
    let result = with_runtime_loan_for_test(&mut owner, |_, mut loan| {
        thread::scope(|scope| {
            scope
                .spawn(move || -> Result<()> {
                    loan.queue_parameter_at(1, 0.25, 0)?;
                    loan.queue_parameter_at(2, 0.75, 0)?;
                    loan.note_on(MidiChannel::Ch1, 60, 100, 0)?;
                    loan.process(&mut AudioBuffers::new(0, 1, 0, 48000.0))
                })
                .join()
                .unwrap()
        })
    });
    assert!(matches!(result, Err(Error::ParameterInputRejected)));
    assert_eq!(handler.native_edits.snapshot(), before);
    assert!(trace.calls().is_empty());
    assert!(owner.runtime.pending_param_changes.is_empty());
    assert!(owner.runtime.input_events.is_empty());
    assert!(owner.runtime.chunk_events.is_empty());
    assert!(!owner.runtime.has_processed_audio);
    assert!(!owner
        .control
        ._host_app
        .is_data_exchange_in_process_for_test());
    let result = process_on_worker(&mut owner, &mut AudioBuffers::new(0, 1, 0, 48000.0));
    assert!(matches!(result, Err(Error::ParameterInputRejected)));
    assert_eq!(handler.native_edits.snapshot(), before);
    assert!(trace.calls().is_empty());
}

#[test]
fn caller_envelope_parameter_callback_asymmetry_remains_unchanged() {
    let state = Arc::new(MockState::default());
    let mut internal = plugin_fixture(&Trace::default(), &state);
    internal.start_processing().unwrap();
    let mut plugin = public_plugin(internal);
    let callbacks = Arc::new(Mutex::new(Vec::new()));
    let observed = callbacks.clone();
    plugin.parameter_change_callback = Some(Box::new(move |id, value| {
        observed
            .lock()
            .unwrap()
            .push((thread::current().id(), id, value));
    }));
    plugin.set_parameter(1, 0.25).unwrap();
    plugin.set_parameter_at(2, 0.5, 3).unwrap();
    plugin.queue_processor_parameter_at(3, 0.75, 7).unwrap();
    assert!(plugin.set_parameter(1, f64::NAN).is_err());
    assert!(plugin.set_parameter_at(2, f64::NAN, 3).is_err());
    assert!(plugin.queue_processor_parameter_at(3, f64::NAN, 7).is_err());
    plugin
        .process_audio(&mut AudioBuffers::new(0, 1, 16, 48000.0))
        .unwrap();
    assert_eq!(
        *callbacks.lock().unwrap(),
        [(thread::current().id(), 1, 0.25)]
    );
    assert_eq!(
        *state.processed_parameters.lock().unwrap(),
        [vec![(1, 0, 0.25), (2, 3, 0.5), (3, 7, 0.75)]]
    );
    // Preserve the legacy public queue method's main-thread mirroring behavior.
    // Internal RuntimeLoan/ProcessorOps admission is separately processor-only.
    assert_eq!(
        f64::from_bits(state.last_mirrored.load(Ordering::Acquire)),
        0.75
    );
}
