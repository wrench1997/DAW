//! Real same-thread capability tests. Rust callback unwind is not foreign COM unwind recovery.
use super::*;

fn main_thread_owner(internal: PluginImpl) -> crate::plugin::MainThreadPlugin {
    let plugin = crate::plugin::Plugin {
        info: internal.info.clone(),
        compatibility: Vec::new(),
        is_processing: internal.runtime.is_processing,
        sample_rate: internal.runtime.sample_rate,
        block_size: internal.runtime.block_size,
        audio_levels: Arc::new(Mutex::new(crate::audio::AudioLevels::new(1))),
        parameter_change_callback: None,
        audio_callback: None,
        internal: Some(Box::new(internal)),
    };
    crate::plugin::MainThreadPlugin::from_in_process(plugin)
}

#[test]
fn scoped_domains_use_borrowed_com_on_loading_thread_and_rejoin_before_teardown() {
    let trace = Trace::default();
    let state = Arc::new(MockState::default());
    let mut plugin = plugin_fixture(&trace, &state);
    plugin.start_processing().unwrap();
    plugin.set_parameter(1, 0.25).unwrap();
    let handler = plugin.control.component_handler.as_ref().unwrap().clone();
    assert_eq!(unsafe { handler.performEdit(3, 0.75) }, kResultOk);
    *plugin.control.editor_resize.lock().unwrap() = Some((640, 480));
    let host = plugin.control._host_app.clone();
    let saw_gate = Arc::new(AtomicBool::new(false));
    let saw = saw_gate.clone();
    let callback_host = host.clone();
    *state.process_callback.lock().unwrap() = Some(Box::new(move || {
        saw.store(
            callback_host.is_data_exchange_in_process_for_test(),
            Ordering::Release,
        );
    }));
    let mut bus = BusAudioBuffers::new(&plugin.audio_bus_layout().unwrap(), 16, 48000.0);
    let mut flat = AudioBuffers::new(0, 1, 16, 48000.0);
    let mut owner = main_thread_owner(plugin);
    trace.clear();
    owner
        .with_domain_session(&mut |control, processor| {
            assert_eq!(control.get_parameter(1)?, 0.25);
            assert_eq!(control.take_editor_resize_request(), Some((640, 480)));
            #[cfg(target_os = "linux")]
            control.service_run_loop();
            processor.queue_parameter_at(2, 0.5, 0)?;
            assert_eq!(control.get_parameter(2)?, 0.25); // queue admission does not claim mirroring
            let id = processor.note_on(MidiChannel::Ch1, 60, 100, 0)?;
            processor.send_note_expression(id, crate::midi::NoteExpressionType::Volume, 0.5, 0)?;
            processor.set_process_transport(crate::plugin::ProcessTransport {
                sample_position: 1024,
                quarter_note_position: 4.0,
                tempo: 120.0,
                playing: true,
                time_sig_numerator: 4,
                time_sig_denominator: 4,
            })?;
            processor.process(&mut flat)?;
            processor.note_off(id, 0)?;
            processor.process_buses(&mut bus)?;
            assert_eq!(
                processor.take_output_events_with_loss(),
                (Vec::new(), false)
            );
            Ok(())
        })
        .unwrap();
    assert!(saw_gate.load(Ordering::Acquire));
    assert!(!host.is_data_exchange_in_process_for_test());
    assert_eq!(state.processed_event_count.load(Ordering::Acquire), 3);
    assert_eq!(
        state.processed_parameters.lock().unwrap()[0],
        [(1, 0, 0.25), (2, 0, 0.5), (3, 0, 0.75)]
    );
    assert!(flat.outputs[0].iter().all(|v| *v == 1.0));
    assert!(bus.outputs[0].channels[0].iter().all(|v| *v == 1.0));
    assert_eq!(handler.native_edits.snapshot().applied, 1);
    assert!(trace
        .methods()
        .iter()
        .all(|(_, method)| !matches!(*method, "addRef" | "release" | "terminate" | "module_drop")));
    trace.assert_thread(thread::current().id());
    // State/lifecycle become available only after the synchronous visit has returned.
    assert!(!owner.save_state().unwrap().is_empty());
    owner.stop_processing().unwrap();
    drop(owner);
    let calls = trace.methods();
    let at = |target| calls.iter().position(|call| *call == target).unwrap();
    assert!(at(("component", "terminate")) < at(("module", "drop")));
    assert_eq!(
        calls
            .iter()
            .filter(|call| **call == ("component", "terminate"))
            .count(),
        1
    );
    assert_eq!(
        calls
            .iter()
            .filter(|call| **call == ("component", "drop"))
            .count(),
        1
    );
}

#[test]
fn callback_error_and_rust_unwind_release_owner_without_fake_native_ack() {
    for unwind in [false, true] {
        let state = Arc::new(MockState::default());
        let mut plugin = plugin_fixture(&Trace::default(), &state);
        plugin.start_processing().unwrap();
        let handler = plugin.control.component_handler.as_ref().unwrap().clone();
        let host = plugin.control._host_app.clone();
        let before = handler.native_edits.snapshot();
        let mut owner = main_thread_owner(plugin);
        if unwind {
            let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _ = owner.with_domain_session(&mut |_control, _processor| {
                    panic!("controlled Rust visitor unwind before processor invocation");
                });
            }));
            assert!(caught.is_err());
        } else {
            assert!(matches!(
                owner.with_domain_session(&mut |_, _| Err(Error::EventInputRejected)),
                Err(Error::EventInputRejected)
            ));
        }
        assert_eq!(handler.native_edits.snapshot(), before);
        assert!(!host.is_data_exchange_in_process_for_test());
        owner
            .with_domain_session(&mut |_, processor| {
                processor.process(&mut AudioBuffers::new(0, 1, 0, 48000.0))
            })
            .unwrap();
        assert!(!owner.save_state().unwrap().is_empty());
        owner.stop_processing().unwrap();
    }
}

#[test]
fn scoped_failed_sdk_and_output_fault_keep_exact_ack_and_capture_semantics() {
    for (sdk_fails, output_overflow) in [(true, false), (false, true), (true, true)] {
        let state = Arc::new(MockState::default());
        state.no_alloc_process.store(true, Ordering::Release);
        state.fail_process.store(sdk_fails, Ordering::Release);
        state
            .output_points
            .store(if output_overflow { 4097 } else { 0 }, Ordering::Release);
        let mut plugin = plugin_fixture(&Trace::default(), &state);
        plugin.start_processing().unwrap();
        let handler = plugin.control.component_handler.as_ref().unwrap().clone();
        unsafe {
            handler.performEdit(7, 0.5);
        }
        let host = plugin.control._host_app.clone();
        let mut owner = main_thread_owner(plugin);
        let result = owner.with_domain_session(&mut |_, processor| {
            processor.process(&mut AudioBuffers::new(0, 1, 0, 48000.0))
        });
        if output_overflow {
            assert!(matches!(result, Err(Error::ParameterOutputRejected)));
        } else {
            assert!(matches!(result, Err(Error::ProcessFailed(result)) if result == kResultFalse));
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
        let result = owner.with_domain_session(&mut |_, processor| {
            processor.process(&mut AudioBuffers::new(0, 1, 0, 48000.0))
        });
        if output_overflow {
            assert!(matches!(result, Err(Error::ParameterOutputRejected)));
        } else {
            result.unwrap();
        }
        assert!(owner.save_state().is_err());
    }
}

#[test]
fn scoped_zero_sample_flush_and_positive_attempt_preserve_surge_history() {
    for fail_positive in [false, true] {
        let state = Arc::new(MockState::default());
        let mut plugin = plugin_fixture(&Trace::default(), &state);
        identify_surge_fixture(&mut plugin);
        plugin.start_processing().unwrap();
        let handler = plugin.control.component_handler.as_ref().unwrap().clone();
        unsafe {
            handler.performEdit(7, 0.5);
        }
        plugin
            .with_domain_session(&mut |_, processor| {
                processor.process(&mut AudioBuffers::new(0, 1, 0, 48000.0))
            })
            .unwrap();
        assert_eq!(handler.native_edits.snapshot().applied, 1);
        assert!(plugin.preflight_state_restore().is_ok());
        let saved = plugin.save_state().unwrap();
        plugin.load_state(&saved).unwrap();
        state.fail_process.store(fail_positive, Ordering::Release);
        let result = plugin.with_domain_session(&mut |_, processor| {
            processor.process(&mut AudioBuffers::new(0, 1, 1, 48000.0))
        });
        assert_eq!(result.is_err(), fail_positive);
        assert!(plugin.preflight_state_restore().is_err());
        let before = handler.native_edits.snapshot();
        assert!(plugin.load_state(&saved).is_err());
        assert_eq!(handler.native_edits.snapshot(), before);
    }
}

#[test]
fn scoped_admission_rejection_keeps_checked_note_and_parameter_obligations() {
    let mut plugin = plugin_fixture(&Trace::default(), &Arc::new(MockState::default()));
    fill_admission_queue(&mut plugin, MAX_QUEUED_EVENTS);
    let next = plugin.runtime.next_note_id;
    plugin
        .with_domain_session(&mut |_, processor| {
            assert!(matches!(
                processor.note_on(MidiChannel::Ch1, 60, 100, 0),
                Err(Error::EventInputRejected)
            ));
            assert!(matches!(
                processor.send_plugin_event(admission_scalar_event()),
                Err(Error::EventInputRejected)
            ));
            assert!(processor.note_on(MidiChannel::Ch1, 128, 100, 0).is_err());
            assert!(processor
                .send_note_expression(
                    crate::midi::NoteId(1),
                    crate::midi::NoteExpressionType::Volume,
                    f64::NAN,
                    0
                )
                .is_err());
            assert!(processor.queue_parameter_at(1, f64::NAN, 0).is_err());
            for id in 0..MAX_PENDING_PARAM_CHANGES {
                processor.queue_parameter_at(id as u32, 0.5, 0)?;
            }
            assert!(matches!(
                processor.queue_parameter_at(9999, 0.5, 0),
                Err(Error::ParameterInputRejected)
            ));
            assert!(matches!(
                processor.process(&mut AudioBuffers::new(0, 1, 0, 48000.0)),
                Err(Error::NotProcessing)
            ));
            Ok(())
        })
        .unwrap();
    assert_eq!(plugin.runtime.next_note_id, next);
    assert!(plugin.runtime.active_notes.is_empty());
    assert_eq!(
        plugin.runtime.pending_param_changes.len(),
        MAX_PENDING_PARAM_CHANGES
    );
    assert_eq!(
        plugin.runtime.input_events.events.lock().unwrap().len(),
        MAX_QUEUED_EVENTS
    );
}

#[test]
fn explicit_session_rejects_non_control_thread_identity_before_visiting() {
    let mut plugin = plugin_fixture(&Trace::default(), &Arc::new(MockState::default()));
    let original = plugin.control.control_thread;
    plugin.control.control_thread = thread::spawn(|| thread::current().id()).join().unwrap();
    let mut called = false;
    assert!(plugin
        .with_domain_session(&mut |_, _| {
            called = true;
            Ok(())
        })
        .is_err());
    assert!(!called);
    plugin.control.control_thread = original;
}

#[test]
fn visitor_failure_after_successful_sdk_call_does_not_revoke_native_application() {
    for unwind in [false, true] {
        let state = Arc::new(MockState::default());
        let mut plugin = plugin_fixture(&Trace::default(), &state);
        plugin.start_processing().unwrap();
        let handler = plugin.control.component_handler.as_ref().unwrap().clone();
        unsafe {
            handler.performEdit(7, 0.5);
        }
        let host = plugin.control._host_app.clone();
        let mut owner = main_thread_owner(plugin);
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            owner.with_domain_session(&mut |_, processor| {
                processor.process(&mut AudioBuffers::new(0, 1, 0, 48000.0))?;
                if unwind {
                    panic!("controlled Rust visitor unwind after successful SDK call");
                }
                Err(Error::EventInputRejected)
            })
        }));
        if unwind {
            assert!(outcome.is_err());
        } else {
            assert!(matches!(outcome.unwrap(), Err(Error::EventInputRejected)));
        }
        let native = handler.native_edits.snapshot();
        assert_eq!(native.applied, 1);
        assert!(!native.lost);
        assert!(!host.is_data_exchange_in_process_for_test());
        assert!(!owner.save_state().unwrap().is_empty());
        owner.stop_processing().unwrap();
    }
}

struct BackendWithoutDomains;
impl PluginInternal for BackendWithoutDomains {
    fn set_parameter(&mut self, _: u32, _: f64) -> Result<()> {
        unreachable!()
    }
    fn get_parameter(&self, _: u32) -> Result<f64> {
        unreachable!()
    }
    fn get_all_parameters(&self) -> Result<Vec<Parameter>> {
        unreachable!()
    }
    fn format_parameter(&self, _: u32, _: f64) -> Result<String> {
        unreachable!()
    }
    fn process(&mut self, _: &mut AudioBuffers) -> Result<()> {
        unreachable!()
    }
    fn send_midi_event(&mut self, _: MidiEvent) -> Result<()> {
        unreachable!()
    }
    fn start_processing(&mut self) -> Result<()> {
        unreachable!()
    }
    fn stop_processing(&mut self) -> Result<()> {
        unreachable!()
    }
    fn has_editor(&self) -> bool {
        unreachable!()
    }
    fn open_editor(&mut self, _: *mut c_void) -> Result<()> {
        unreachable!()
    }
    fn close_editor(&mut self) -> Result<()> {
        unreachable!()
    }
    fn get_editor_size(&self) -> Result<(i32, i32)> {
        unreachable!()
    }
    fn get_parameter_changes(&self) -> Vec<(u32, f64)> {
        unreachable!()
    }
}

#[test]
fn unsupported_backend_uses_object_safe_default_without_running_callback() {
    let mut backend: Box<dyn PluginInternal> = Box::new(BackendWithoutDomains);
    let mut invoked = false;
    let result = backend.with_domain_session(&mut |_, _| {
        invoked = true;
        Ok(())
    });
    assert!(result.unwrap_err().to_string().contains("unsupported"));
    assert!(!invoked);
}
