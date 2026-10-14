//! Full COM lifecycle regressions for explicit owner choices across dynamic I/O.

use super::*;

fn selected_stereo_fixture(trace: &Trace, state: &Arc<MockState>) -> PluginImpl {
    state.output_buses.store(32, Ordering::Relaxed);
    state.channels.store(2, Ordering::Relaxed);
    state.event_inputs.store(1, Ordering::Relaxed);
    state.event_outputs.store(1, Ordering::Relaxed);
    let mut plugin = plugin_fixture(trace, state);
    plugin.setup_processing().unwrap();
    for index in 0..32 {
        plugin
            .set_bus_active(
                crate::audio::MediaType::Audio,
                crate::audio::BusDirection::Output,
                index,
                index == 0,
            )
            .unwrap();
    }
    plugin
        .set_bus_active(
            crate::audio::MediaType::Event,
            crate::audio::BusDirection::Input,
            0,
            true,
        )
        .unwrap();
    plugin
        .set_bus_active(
            crate::audio::MediaType::Event,
            crate::audio::BusDirection::Output,
            0,
            false,
        )
        .unwrap();
    plugin
}

fn request_io_change(plugin: &PluginImpl) {
    let handler = plugin
        .control
        .component_handler
        .as_ref()
        .unwrap()
        .to_com_ptr::<IComponentHandler>()
        .unwrap();
    assert_eq!(
        unsafe { handler.restartComponent(RestartFlags_::kIoChanged) },
        kResultOk
    );
}

fn assert_one_stereo_wire_with_full_declared_layout(plugin: &PluginImpl) {
    let layout = plugin.checked_audio_bus_layout().unwrap();
    assert!(layout.inputs.is_empty());
    assert_eq!(layout.outputs.len(), 32);
    for (index, bus) in layout.outputs.iter().enumerate() {
        assert_eq!(bus.channel_count, 2);
        assert!(bus.is_main);
        assert_eq!(bus.active, Some(index == 0));
    }
    assert_eq!(plugin.output_channel_count(), 2);
    let data = plugin.runtime.process_data.as_ref().unwrap();
    assert_eq!(data.process_data.numOutputs, 32);
    assert_eq!(data.output_bus_buffers.len(), 32);
    assert_eq!(data.sample_buffers.output_channel_count(), 2);
    let HostSampleBuffers::F32(samples) = &data.sample_buffers else {
        panic!("fixture must retain its F32 allocation");
    };
    assert_eq!(samples.outputs.len(), 2);
    assert_eq!(samples.output_channel_ptrs.0.len(), 32);
    for (index, bus) in data.output_bus_buffers.iter().enumerate() {
        assert_eq!(bus.numChannels, 2);
        assert!(!unsafe { bus.__field0.channelBuffers32 }.is_null());
        assert_eq!(samples.output_channel_ptrs.0[index].len(), 2);
        assert!(samples.output_channel_ptrs.0[index]
            .iter()
            .all(|pointer| pointer.is_null() == (index != 0)));
    }
    let events = plugin.event_bus_layout().unwrap();
    assert_eq!(events.inputs.len(), 1);
    assert_eq!(events.inputs[0].channel_count, 16);
    assert_eq!(events.inputs[0].active, Some(true));
    assert_eq!(events.outputs.len(), 1);
    assert_eq!(events.outputs[0].active, Some(false));
}

fn assert_failed_io_is_quiescent(plugin: &mut PluginImpl, trace: &Trace) {
    assert!(!plugin.runtime.is_processing);
    assert!(!plugin.control.is_active);
    assert!(plugin.runtime.process_data.is_none());
    assert!(plugin.control.applied_setup.is_none());
    assert!(plugin
        .runtime
        .bus_activation
        .audio_output_acks
        .iter()
        .all(Option::is_none));
    assert!(trace.methods().iter().all(|(_, method)| {
        !matches!(
            *method,
            "setActive(true)" | "setProcessing(true)" | "process"
        )
    }));
    trace.clear();
    let mut buffers = AudioBuffers::new(0, 2, 16, 48000.0);
    assert!(plugin.process(&mut buffers).is_err());
    assert!(trace
        .methods()
        .iter()
        .all(|(_, method)| *method != "process"));
}

#[test]
fn io_restart_reapplies_explicit_stereo_selection_with_full_descriptors_and_fresh_sdk_ack() {
    let trace = Trace::default();
    let state = Arc::new(MockState::default());
    let mut plugin = selected_stereo_fixture(&trace, &state);
    plugin.start_processing().unwrap();
    let descriptors = unsafe { plugin.read_explicit_bus_bindings().unwrap() };
    assert_one_stereo_wire_with_full_declared_layout(&plugin);
    request_io_change(&plugin);
    // Taking flags separately erases ACKs, not the independent host choices.
    assert!(plugin.take_restart_flags().io_changed());
    assert!(plugin
        .runtime
        .bus_activation
        .audio_output_acks
        .iter()
        .all(Option::is_none));
    request_io_change(&plugin);
    state.activation_calls.lock().unwrap().clear();
    trace.clear();
    assert!(plugin.service_host_requests().unwrap().io_changed());
    trace.assert_thread(thread::current().id());
    let methods = trace.methods();
    let stopped = methods
        .iter()
        .position(|(_, m)| *m == "setProcessing(false)")
        .unwrap();
    let inactive = methods
        .iter()
        .position(|(_, m)| *m == "setActive(false)")
        .unwrap();
    let activated = methods
        .iter()
        .position(|(_, m)| *m == "activateBus")
        .unwrap();
    assert!(stopped < inactive && inactive < activated);
    let mut calls = (0..32)
        .map(|index| (kAudio as i32, kOutput as i32, index, index == 0))
        .collect::<Vec<_>>();
    calls.extend([
        (kEvent as i32, kInput as i32, 0, true),
        (kEvent as i32, kOutput as i32, 0, false),
    ]);
    assert_eq!(*state.activation_calls.lock().unwrap(), calls);
    assert_eq!(
        unsafe { plugin.read_explicit_bus_bindings().unwrap() },
        descriptors
    );
    assert_one_stereo_wire_with_full_declared_layout(&plugin);
    plugin
        .send_midi_event(MidiEvent::NoteOn {
            channel: MidiChannel::Ch1,
            note: 60,
            velocity: 100,
        })
        .unwrap();
    let mut buffers = AudioBuffers::new(0, 2, 16, 48000.0);
    plugin.process(&mut buffers).unwrap();
    assert_eq!(buffers.outputs, [vec![1.0; 16], vec![2.0; 16]]);
    assert_eq!(state.processed_event_count.load(Ordering::Relaxed), 1);
    assert_eq!(*state.processed_layouts.lock().unwrap(), [vec![2; 32]]);
}

#[test]
fn io_restart_rejects_explicit_descriptor_count_and_query_drift_before_any_activation() {
    for mutation in 0..12 {
        let trace = Trace::default();
        let state = Arc::new(MockState::default());
        let mut plugin = selected_stereo_fixture(&trace, &state);
        plugin.start_processing().unwrap();
        match mutation {
            0 => state.channels.store(1, Ordering::Relaxed),
            1 => state.audio_names_reversed.store(true, Ordering::Relaxed),
            2 => state.audio_bus_aux.store(true, Ordering::Relaxed),
            3 => state.audio_flags_override.store(0, Ordering::Relaxed),
            4 => state.audio_name_tail.store(7, Ordering::Relaxed),
            5 => state.output_buses.store(31, Ordering::Relaxed),
            6 => state.output_buses.store(33, Ordering::Relaxed),
            7 => state.output_buses.store(-1, Ordering::Relaxed),
            8 => state.channels.store(0, Ordering::Relaxed),
            9 => state.audio_bus_info_error.store(true, Ordering::Relaxed),
            10 => state.event_bus_info_error.store(true, Ordering::Relaxed),
            _ => state.event_inputs.store(2, Ordering::Relaxed),
        }
        state.activation_calls.lock().unwrap().clear();
        request_io_change(&plugin);
        trace.clear();
        let error = plugin.service_host_requests().unwrap_err();
        assert!(!error.to_string().is_empty(), "mutation {mutation}");
        assert!(
            state.activation_calls.lock().unwrap().is_empty(),
            "mutation {mutation}"
        );
        assert_failed_io_is_quiescent(&mut plugin, &trace);
    }
}

#[test]
fn io_restart_sdk_rejection_blocks_failed_explicit_choice_without_resuming_or_fallback() {
    for rejection in [kResultFalse, vst3::Steinberg::kInternalError] {
        let trace = Trace::default();
        let state = Arc::new(MockState::default());
        let mut plugin = selected_stereo_fixture(&trace, &state);
        plugin.start_processing().unwrap();
        state.activation_result.store(rejection, Ordering::Relaxed);
        state.activation_calls.lock().unwrap().clear();
        request_io_change(&plugin);
        trace.clear();
        assert!(plugin.service_host_requests().is_err());
        assert_eq!(
            *state.activation_calls.lock().unwrap(),
            [(kAudio as i32, kOutput as i32, 0, true)]
        );
        assert_failed_io_is_quiescent(&mut plugin, &trace);
        state.activation_result.store(kResultOk, Ordering::Relaxed);
        state.activation_calls.lock().unwrap().clear();
        request_io_change(&plugin);
        trace.clear();
        assert!(
            plugin.service_host_requests().is_err(),
            "a failed choice must stay blocked"
        );
        assert!(state.activation_calls.lock().unwrap().is_empty());
        assert_failed_io_is_quiescent(&mut plugin, &trace);
        plugin
            .set_bus_active(
                crate::audio::MediaType::Audio,
                crate::audio::BusDirection::Output,
                0,
                true,
            )
            .unwrap();
        request_io_change(&plugin);
        plugin.service_host_requests().unwrap();
        assert_one_stereo_wire_with_full_declared_layout(&plugin);
    }

    // A rejection of an explicit false after a successful first bus must not
    // publish the partial ACK set or resume any of the old process pointers.
    let trace = Trace::default();
    let state = Arc::new(MockState::default());
    let mut plugin = selected_stereo_fixture(&trace, &state);
    plugin.start_processing().unwrap();
    state.activation_fail_index.store(1, Ordering::Relaxed);
    state.activation_calls.lock().unwrap().clear();
    request_io_change(&plugin);
    trace.clear();
    assert!(plugin.service_host_requests().is_err());
    assert_eq!(
        *state.activation_calls.lock().unwrap(),
        [
            (kAudio as i32, kOutput as i32, 0, true),
            (kAudio as i32, kOutput as i32, 1, false)
        ]
    );
    assert_failed_io_is_quiescent(&mut plugin, &trace);
}

#[test]
fn io_restart_without_explicit_choices_keeps_all_declared_defaults() {
    let trace = Trace::default();
    let state = Arc::new(MockState::default());
    state.output_buses.store(32, Ordering::Relaxed);
    state.channels.store(2, Ordering::Relaxed);
    let mut plugin = plugin_fixture(&trace, &state);
    plugin.start_processing().unwrap();
    assert!(plugin.control.bus_activation_preferences.is_empty());
    request_io_change(&plugin);
    state.activation_calls.lock().unwrap().clear();
    plugin.service_host_requests().unwrap();
    assert_eq!(plugin.output_channel_count(), 64);
    assert_prepared_layout(&plugin, 32, 2);
    assert_eq!(
        *state.activation_calls.lock().unwrap(),
        (0..32)
            .map(|index| (kAudio as i32, kOutput as i32, index, true))
            .collect::<Vec<_>>()
    );
}

#[test]
fn io_restart_rejects_old_explicit_count_after_legacy_load_state_expands_side() {
    let trace = Trace::default();
    let state = Arc::new(MockState::default());
    let mut plugin = plugin_fixture(&trace, &state);
    plugin.setup_processing().unwrap();
    plugin
        .set_bus_active(
            crate::audio::MediaType::Audio,
            crate::audio::BusDirection::Output,
            0,
            false,
        )
        .unwrap();
    plugin.start_processing().unwrap();
    state.expand_on_state.store(true, Ordering::Relaxed);
    let snapshot = encode_state_snapshot(&StateSnapshot {
        component: vec![1],
        controller: None,
    })
    .unwrap();
    // Preserve the generic legacy state-load contract. Its fresh SDK ACKs must
    // not replace the explicit intent's original descriptor and side count.
    plugin
        .load_state_with_context(&snapshot, &StateContext::Project)
        .unwrap();
    assert_eq!(plugin.runtime.bus_activation.audio_outputs, [false, true]);
    assert!(plugin
        .runtime
        .bus_activation
        .audio_output_acks
        .iter()
        .all(Option::is_some));
    state.activation_calls.lock().unwrap().clear();
    request_io_change(&plugin);
    trace.clear();
    assert!(plugin.service_host_requests().is_err());
    assert!(state.activation_calls.lock().unwrap().is_empty());
    assert_failed_io_is_quiescent(&mut plugin, &trace);
}

#[test]
fn explicit_bus_validation_only_rejections_preserve_previous_choice_and_ack() {
    let trace = Trace::default();
    let state = Arc::new(MockState::default());
    let mut plugin = selected_stereo_fixture(&trace, &state);
    let before = plugin.runtime.bus_activation.clone();
    state.activation_calls.lock().unwrap().clear();
    for index in [-1, 32] {
        assert!(matches!(
            plugin.set_bus_active(
                crate::audio::MediaType::Audio,
                crate::audio::BusDirection::Output,
                index,
                true
            ),
            Err(Error::InvalidParameter(_))
        ));
        assert_eq!(plugin.runtime.bus_activation, before);
        assert!(state.activation_calls.lock().unwrap().is_empty());
    }
    plugin.start_processing().unwrap();
    request_io_change(&plugin);
    plugin.service_host_requests().unwrap();
    assert_one_stereo_wire_with_full_declared_layout(&plugin);
}

#[test]
fn failed_explicit_requests_cannot_reuse_old_intent_or_sdk_success_before_commit() {
    for failure in 0..4 {
        let trace = Trace::default();
        let state = Arc::new(MockState::default());
        let mut plugin = selected_stereo_fixture(&trace, &state);
        plugin.activate().unwrap();
        match failure {
            0 => state.fail_activation.store(true, Ordering::Relaxed),
            1 => state.audio_bus_info_error.store(true, Ordering::Relaxed),
            2 => state.fail_reactivate.store(true, Ordering::Relaxed),
            _ => {
                let changed = state.clone();
                *state.activation_callback.lock().unwrap() = Some(Box::new(move || {
                    changed.channels.store(0, Ordering::Relaxed);
                }));
            }
        }
        assert!(plugin
            .set_bus_active(
                crate::audio::MediaType::Audio,
                crate::audio::BusDirection::Output,
                1,
                true
            )
            .is_err());
        assert!(plugin.runtime.bus_activation.audio_output_acks[1].is_none());
        state.fail_activation.store(false, Ordering::Relaxed);
        state.audio_bus_info_error.store(false, Ordering::Relaxed);
        state.fail_reactivate.store(false, Ordering::Relaxed);
        state.channels.store(2, Ordering::Relaxed);
        *state.activation_callback.lock().unwrap() = None;
        state.activation_calls.lock().unwrap().clear();
        request_io_change(&plugin);
        trace.clear();
        assert!(plugin.service_host_requests().is_err(), "failure {failure}");
        assert!(state.activation_calls.lock().unwrap().is_empty());
        assert_failed_io_is_quiescent(&mut plugin, &trace);
        plugin
            .set_bus_active(
                crate::audio::MediaType::Audio,
                crate::audio::BusDirection::Output,
                1,
                false,
            )
            .unwrap();
        request_io_change(&plugin);
        plugin.service_host_requests().unwrap();
        assert_one_stereo_wire_with_full_declared_layout(&plugin);
    }
}

#[test]
fn io_restart_does_not_apply_later_choices_after_sdk_synchronously_changes_descriptor() {
    let trace = Trace::default();
    let state = Arc::new(MockState::default());
    let mut plugin = selected_stereo_fixture(&trace, &state);
    plugin.start_processing().unwrap();
    let changed = state.clone();
    *state.activation_callback.lock().unwrap() = Some(Box::new(move || {
        changed.audio_name_tail.store(9, Ordering::Relaxed);
    }));
    request_io_change(&plugin);
    state.activation_calls.lock().unwrap().clear();
    trace.clear();
    assert!(plugin.service_host_requests().is_err());
    assert_eq!(
        *state.activation_calls.lock().unwrap(),
        [(kAudio as i32, kOutput as i32, 0, true)]
    );
    assert_failed_io_is_quiescent(&mut plugin, &trace);
    *state.activation_callback.lock().unwrap() = None;
}

#[test]
fn io_lifecycle_failures_invalidate_ack_and_storage_without_replaying_processing() {
    // suspend, deactivate, reactivate, resume, and failed resume-cleanup deactivation.
    for failure in 0..5 {
        let trace = Trace::default();
        let state = Arc::new(MockState::default());
        let mut plugin = selected_stereo_fixture(&trace, &state);
        plugin.start_processing().unwrap();
        match failure {
            0 => state.fail_processing_stop.store(true, Ordering::Relaxed),
            1 => state.fail_deactivate.store(true, Ordering::Relaxed),
            2 => state.fail_reactivate.store(true, Ordering::Relaxed),
            _ => state.fail_processing_start.store(true, Ordering::Relaxed),
        }
        if failure == 4 {
            let cleanup_state = state.clone();
            *state.activation_callback.lock().unwrap() = Some(Box::new(move || {
                // The initial deactivation already succeeded before the SDK bus calls.
                cleanup_state.fail_deactivate.store(true, Ordering::Relaxed);
            }));
        }
        request_io_change(&plugin);
        trace.clear();
        let error = plugin.service_host_requests().unwrap_err().to_string();
        assert!(!error.is_empty());
        assert!(
            plugin.runtime.process_data.is_none(),
            "case {failure}: stale processing storage"
        );
        assert!(
            plugin.control.applied_setup.is_none(),
            "case {failure}: stale successful setup"
        );
        assert!(plugin
            .runtime
            .bus_activation
            .audio_output_acks
            .iter()
            .all(Option::is_none));
        assert!(plugin
            .runtime
            .bus_activation
            .event_input_acks
            .iter()
            .all(Option::is_none));
        assert!(plugin
            .runtime
            .bus_activation
            .event_output_acks
            .iter()
            .all(Option::is_none));
        assert_eq!(plugin.runtime.is_processing, failure == 0);
        assert_eq!(plugin.control.is_active, matches!(failure, 0 | 1 | 4));
        let methods = trace.methods();
        assert_eq!(
            methods
                .iter()
                .filter(|(_, method)| *method == "setProcessing(true)")
                .count(),
            usize::from(failure >= 3),
            "case {failure}: no automatic replay after failure"
        );
        if failure == 4 {
            assert!(error.contains("resume processing"), "{error}");
            assert!(error.contains("cleanup"), "{error}");
        }
        trace.clear();
        let mut buffers = AudioBuffers::new(0, 2, 16, 48_000.0);
        assert!(
            plugin.process(&mut buffers).is_err(),
            "case {failure}: failed I/O cannot process"
        );
        assert!(trace
            .methods()
            .iter()
            .all(|(_, method)| *method != "process"));
        // Avoid leaving the test mock in its deliberate refusal state during teardown.
        state.fail_processing_stop.store(false, Ordering::Relaxed);
        state.fail_processing_start.store(false, Ordering::Relaxed);
        state.fail_deactivate.store(false, Ordering::Relaxed);
        state.fail_reactivate.store(false, Ordering::Relaxed);
        *state.activation_callback.lock().unwrap() = None;
        state.activation_calls.lock().unwrap().clear();
        request_io_change(&plugin);
        plugin.service_host_requests().unwrap();
        assert_eq!(state.activation_calls.lock().unwrap().len(), 34);
        assert_one_stereo_wire_with_full_declared_layout(&plugin);
    }
}

#[test]
fn latency_only_deactivation_refusal_retains_legacy_resume_behavior() {
    let trace = Trace::default();
    let state = Arc::new(MockState::default());
    let mut plugin = selected_stereo_fixture(&trace, &state);
    plugin.start_processing().unwrap();
    state.fail_deactivate.store(true, Ordering::Relaxed);
    let handler = plugin
        .control
        .component_handler
        .as_ref()
        .unwrap()
        .to_com_ptr::<IComponentHandler>()
        .unwrap();
    assert_eq!(
        unsafe { handler.restartComponent(RestartFlags_::kLatencyChanged) },
        kResultOk
    );
    trace.clear();
    assert!(plugin.service_host_requests().is_err());
    assert!(plugin.runtime.is_processing);
    assert!(plugin.control.is_active);
    assert_one_stereo_wire_with_full_declared_layout(&plugin);
    assert_eq!(
        trace
            .methods()
            .iter()
            .filter(|(_, method)| *method == "setProcessing(true)")
            .count(),
        1
    );
    state.fail_deactivate.store(false, Ordering::Relaxed);
}
