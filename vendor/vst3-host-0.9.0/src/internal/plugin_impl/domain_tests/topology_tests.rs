//! Actual owner/SDK evidence for dry topology. No plugin binary or native UI.
use super::*;

fn dry_state() -> Arc<MockState> {
    let state = Arc::new(MockState::default());
    state.dry_topology.store(true, Ordering::Relaxed);
    state.output_buses.store(3, Ordering::Relaxed);
    state.channels.store(2, Ordering::Relaxed);
    state.event_inputs.store(1, Ordering::Relaxed);
    state
}

fn restore_blob() -> Vec<u8> {
    encode_state_snapshot(&StateSnapshot {
        component: vec![1],
        controller: None,
    })
    .unwrap()
}

fn all_unknown(plugin: &PluginImpl) {
    let state = &plugin.runtime.bus_activation;
    for side in [
        &state.audio_input_acks,
        &state.audio_output_acks,
        &state.event_input_acks,
        &state.event_output_acks,
    ] {
        assert!(side.iter().all(Option::is_none));
    }
}

#[test]
fn dry_defaults_keep_inactive_unknown_until_actual_false_acknowledgment() {
    let trace = Trace::default();
    let state = dry_state();
    let mut plugin = plugin_fixture(&trace, &state);
    plugin.setup_processing().unwrap();
    let initial = plugin.checked_audio_bus_layout().unwrap();
    assert_eq!(initial.inputs[0].active, Some(true));
    assert_eq!(
        initial.outputs.iter().map(|b| b.active).collect::<Vec<_>>(),
        [Some(true), None, None]
    );
    assert_eq!(
        state.activation_calls.lock().unwrap().as_slice(),
        &[
            (kAudio as i32, kInput as i32, 0, true),
            (kAudio as i32, kOutput as i32, 0, true),
            (kEvent as i32, kInput as i32, 0, true)
        ]
    );
    trace.clear();
    // Observations never install evidence or invoke activation.
    plugin.checked_audio_bus_layout().unwrap();
    plugin.event_bus_layout().unwrap();
    assert!(!trace.methods().iter().any(|(_, m)| *m == "activateBus"));
    for index in [1, 2] {
        plugin
            .set_bus_active(
                crate::MediaType::Audio,
                crate::BusDirection::Output,
                index,
                false,
            )
            .unwrap();
    }
    let acknowledged = plugin.checked_audio_bus_layout().unwrap();
    assert_eq!(
        acknowledged
            .outputs
            .iter()
            .map(|b| b.active)
            .collect::<Vec<_>>(),
        [Some(true), Some(false), Some(false)]
    );
    assert!(acknowledged.outputs[0].is_main);
    assert!(acknowledged.outputs[1..].iter().all(|b| !b.is_main));
    let before = plugin.runtime.bus_activation.clone();
    plugin.reconfigure(44100.0, 128).unwrap();
    assert_eq!(
        plugin.runtime.bus_activation, before,
        "successful unchanged descriptors may retain the original acknowledgment"
    );
    drop(plugin);
    trace.assert_thread(thread::current().id());
}

#[test]
fn failed_activation_and_descriptor_change_during_success_never_reuse_old_ack() {
    for descriptor_changes in [false, true] {
        let trace = Trace::default();
        let state = Arc::new(MockState::default());
        let mut plugin = plugin_fixture(&trace, &state);
        plugin.setup_processing().unwrap();
        assert_eq!(
            plugin.checked_audio_bus_layout().unwrap().outputs[0].active,
            Some(true)
        );
        if descriptor_changes {
            let weak = Arc::downgrade(&state);
            *state.activation_callback.lock().unwrap() = Some(Box::new(move || {
                if let Some(state) = weak.upgrade() {
                    state.audio_bus_aux.store(true, Ordering::Relaxed);
                }
            }));
        } else {
            state.fail_activation.store(true, Ordering::Relaxed);
        }
        let result = plugin.set_bus_active(
            crate::MediaType::Audio,
            crate::BusDirection::Output,
            0,
            true,
        );
        // Explicit host intent commits only after the complete binding is
        // rechecked. An SDK success on a bus whose role changed is an error for
        // this transaction, as well as invalid activation evidence.
        let error = result.unwrap_err().to_string();
        if descriptor_changes {
            assert!(
                error.contains("descriptor/count changed during activation"),
                "{error}"
            );
        } else {
            assert!(error.contains("activateBus failed"), "{error}");
        }
        assert_eq!(
            plugin.checked_audio_bus_layout().unwrap().outputs[0].active,
            None
        );
        // Returning to an earlier descriptor cannot resurrect discarded evidence.
        state.audio_bus_aux.store(false, Ordering::Relaxed);
        state.fail_activation.store(false, Ordering::Relaxed);
        *state.activation_callback.lock().unwrap() = None;
        plugin.reconfigure(48000.0, 16).unwrap();
        assert_eq!(
            plugin.checked_audio_bus_layout().unwrap().outputs[0].active,
            None
        );
    }
}

#[test]
fn state_failure_and_failed_post_restore_negotiation_clear_all_acknowledgments() {
    for failure in 0..3 {
        let trace = Trace::default();
        let state = dry_state();
        let mut plugin = plugin_fixture(&trace, &state);
        plugin.setup_processing().unwrap();
        match failure {
            0 => state.fail_state.store(true, Ordering::Relaxed),
            1 => state.fail_arrangement.store(true, Ordering::Relaxed),
            _ => state.fail_setup.store(true, Ordering::Relaxed),
        }
        assert!(plugin
            .load_state_with_context(&restore_blob(), &StateContext::Project)
            .is_err());
        all_unknown(&plugin);
    }
}

#[test]
fn successful_restore_attests_retained_false_without_extra_activation_policy() {
    let trace = Trace::default();
    let state = dry_state();
    let mut plugin = plugin_fixture(&trace, &state);
    plugin.setup_processing().unwrap();
    state.activation_calls.lock().unwrap().clear();
    plugin
        .load_state_with_context(&restore_blob(), &StateContext::Project)
        .unwrap();
    assert_eq!(
        state.activation_calls.lock().unwrap().as_slice(),
        &[
            (kAudio as i32, kInput as i32, 0, true),
            (kAudio as i32, kOutput as i32, 0, true),
            (kAudio as i32, kOutput as i32, 1, false),
            (kAudio as i32, kOutput as i32, 2, false),
            (kEvent as i32, kInput as i32, 0, true)
        ]
    );
    assert_eq!(
        plugin
            .checked_audio_bus_layout()
            .unwrap()
            .outputs
            .iter()
            .map(|b| b.active)
            .collect::<Vec<_>>(),
        [Some(true), Some(false), Some(false)]
    );
}

#[test]
fn io_restart_service_failure_before_refresh_invalidates_evidence() {
    let trace = Trace::default();
    let state = dry_state();
    let mut plugin = plugin_fixture(&trace, &state);
    plugin.start_processing().unwrap();
    let callback = plugin
        .control
        .component_handler
        .as_ref()
        .unwrap()
        .to_com_ptr::<IComponentHandler>()
        .unwrap();
    state.fail_deactivate.store(true, Ordering::Relaxed);
    assert_eq!(
        unsafe { callback.restartComponent(RestartFlags_::kIoChanged) },
        kResultOk
    );
    state.activation_calls.lock().unwrap().clear();
    assert!(plugin.service_host_requests().is_err());
    assert!(state.activation_calls.lock().unwrap().is_empty());
    all_unknown(&plugin);
    state.fail_deactivate.store(false, Ordering::Relaxed);
}

#[test]
fn event_rebuild_count_reorder_and_read_failure_cannot_resurrect_acknowledgments() {
    for changed in 0..3 {
        let trace = Trace::default();
        let state = Arc::new(MockState::default());
        state.event_inputs.store(2, Ordering::Relaxed);
        let mut plugin = plugin_fixture(&trace, &state);
        plugin.setup_processing().unwrap();
        assert!(plugin
            .event_bus_layout()
            .unwrap()
            .inputs
            .iter()
            .all(|b| b.active == Some(true)));
        match changed {
            0 => state.event_inputs.store(1, Ordering::Relaxed),
            1 => state.event_names_reversed.store(true, Ordering::Relaxed),
            _ => state.event_bus_info_error.store(true, Ordering::Relaxed),
        }
        plugin.reconfigure(48000.0, 32).unwrap();
        assert!(plugin
            .runtime
            .bus_activation
            .event_input_acks
            .iter()
            .all(Option::is_none));
        state.event_inputs.store(2, Ordering::Relaxed);
        state.event_names_reversed.store(false, Ordering::Relaxed);
        state.event_bus_info_error.store(false, Ordering::Relaxed);
        plugin.reconfigure(48000.0, 16).unwrap();
        assert!(plugin
            .event_bus_layout()
            .unwrap()
            .inputs
            .iter()
            .all(|b| b.active.is_none()));
    }
}

/// Called only by the real COM mock Process vtable, with production prepared buffers.
/// Every nested pointer used below is checked for the expected shape/non-nullness first.
/// It returns SDK failure on an assertion mismatch rather than unwinding through COM.
pub(super) unsafe fn check_dry_sdk_buffers(data: &mut ProcessData) -> bool {
    if data.numSamples != 128
        || data.numInputs != 1
        || data.numOutputs != 3
        || data.inputs.is_null()
        || data.outputs.is_null()
    {
        return false;
    }
    let input = &mut *data.inputs;
    if input.numChannels != 2 {
        return false;
    }
    for bus_index in 0..3 {
        if (*data.outputs.add(bus_index)).numChannels != 2 {
            return false;
        }
    }
    macro_rules! check {
        ($field:ident, $sample:ty) => {{
            if input.__field0.$field.is_null() {
                return false;
            }
            for channel in 0..2 {
                let samples = *input.__field0.$field.add(channel);
                if samples.is_null() {
                    return false;
                }
                if !std::slice::from_raw_parts(samples, 128)
                    .iter()
                    .all(|x| x.to_bits() == 0)
                {
                    return false;
                }
                // Deliberately poison host input storage after observation. The next call
                // must overwrite it, whether explicit dry zeros or omitted flat inputs.
                std::slice::from_raw_parts_mut(samples, 128).fill(0.75 as $sample);
            }
            for bus_index in 0..3 {
                let bus = &mut *data.outputs.add(bus_index);
                if bus.__field0.$field.is_null() {
                    return false;
                }
                for channel in 0..2 {
                    let samples = *bus.__field0.$field.add(channel);
                    if bus_index == 0 {
                        if samples.is_null() {
                            return false;
                        }
                        std::slice::from_raw_parts_mut(samples, 128).fill((channel + 1) as $sample);
                    } else if !samples.is_null() {
                        return false;
                    }
                }
                bus.silenceFlags = 0;
            }
        }};
    }
    if data.symbolicSampleSize == SymbolicSampleSizes_::kSample32 as i32 {
        check!(channelBuffers32, f32);
    } else if data.symbolicSampleSize == SymbolicSampleSizes_::kSample64 as i32 {
        check!(channelBuffers64, f64);
    } else {
        return false;
    }
    true
}

#[test]
fn dry_generator_actual_sdk_f32_and_f64_preserves_all_slots_and_silences_every_input_call() {
    for sample_size in [ProcessingSampleSize::F32, ProcessingSampleSize::F64] {
        let trace = Trace::default();
        let state = dry_state();
        let mut plugin = plugin_fixture(&trace, &state);
        plugin.runtime.sample_size = sample_size;
        plugin.runtime.block_size = 128;
        plugin.setup_processing().unwrap();
        plugin
            .load_state_with_context(&restore_blob(), &StateContext::Project)
            .unwrap();
        plugin.start_processing().unwrap();
        let expected = plugin.checked_audio_bus_layout().unwrap();
        assert_eq!(plugin.output_channel_count(), 2);
        for inputs in [2, 2, 0, 0] {
            let mut buffers = AudioBuffers::new(inputs, 2, 128, 48000.0);
            assert!(buffers.inputs.iter().flatten().all(|x| x.to_bits() == 0));
            plugin.process(&mut buffers).unwrap();
            assert_eq!(buffers.outputs, [vec![1.0; 128], vec![2.0; 128]]);
            assert_eq!(plugin.checked_audio_bus_layout().unwrap(), expected);
        }
        assert_eq!(
            *state.dry_process_checks.lock().unwrap(),
            vec![(sample_size.symbolic(), true); 4]
        );
        plugin.stop_processing().unwrap();
        drop(plugin);
        trace.assert_thread(thread::current().id());
    }
}

#[test]
fn partial_activation_refresh_failure_does_not_publish_earlier_successes() {
    let trace = Trace::default();
    let state = dry_state();
    let mut plugin = plugin_fixture(&trace, &state);
    plugin.setup_processing().unwrap();
    state.activation_calls.lock().unwrap().clear();
    let entered = Arc::new(AtomicUsize::new(0));
    let observed = entered.clone();
    let weak = Arc::downgrade(&state);
    *state.activation_callback.lock().unwrap() = Some(Box::new(move || {
        if observed.fetch_add(1, Ordering::Relaxed) == 2 {
            if let Some(state) = weak.upgrade() {
                state.fail_activation.store(true, Ordering::Relaxed);
            }
        }
    }));
    assert!(plugin
        .load_state_with_context(&restore_blob(), &StateContext::Project)
        .is_err());
    assert_eq!(entered.load(Ordering::Relaxed), 3);
    assert_eq!(
        state.activation_calls.lock().unwrap().as_slice(),
        &[
            (kAudio as i32, kInput as i32, 0, true),
            (kAudio as i32, kOutput as i32, 0, true),
            (kAudio as i32, kOutput as i32, 1, false)
        ]
    );
    all_unknown(&plugin);
}

#[test]
fn audio_count_and_reorder_rebuild_discard_old_index_evidence() {
    for reorder in [false, true] {
        let trace = Trace::default();
        let state = Arc::new(MockState::default());
        state.output_buses.store(2, Ordering::Relaxed);
        let mut plugin = plugin_fixture(&trace, &state);
        plugin.setup_processing().unwrap();
        assert!(plugin
            .checked_audio_bus_layout()
            .unwrap()
            .outputs
            .iter()
            .all(|b| b.active == Some(true)));
        if reorder {
            state.audio_names_reversed.store(true, Ordering::Relaxed);
        } else {
            state.output_buses.store(1, Ordering::Relaxed);
        }
        plugin.reconfigure(48000.0, 32).unwrap();
        assert!(plugin
            .runtime
            .bus_activation
            .audio_output_acks
            .iter()
            .all(Option::is_none));
        state.output_buses.store(2, Ordering::Relaxed);
        state.audio_names_reversed.store(false, Ordering::Relaxed);
        plugin.reconfigure(48000.0, 16).unwrap();
        assert!(plugin
            .checked_audio_bus_layout()
            .unwrap()
            .outputs
            .iter()
            .all(|b| b.active.is_none()));
    }
}
