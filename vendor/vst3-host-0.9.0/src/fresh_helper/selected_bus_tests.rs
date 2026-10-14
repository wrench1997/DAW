//! Disposable selected-output clients against actual cooperative mock children.
use super::*;
use crate::audio::{BusDirection, MediaType};

fn layout(output_count: usize, input_channels: usize, selected: bool) -> CheckedAudioBusLayout {
    CheckedAudioBusLayout {
        inputs: if input_channels == 0 {
            Vec::new()
        } else {
            vec![CheckedAudioBusConfig {
                channel_count: input_channels,
                is_main: true,
                active: selected.then_some(true),
            }]
        },
        outputs: (0..output_count)
            .map(|index| CheckedAudioBusConfig {
                channel_count: 2,
                is_main: index == 0,
                active: selected.then_some(index == 0),
            })
            .collect(),
    }
}
fn query(layout: &CheckedAudioBusLayout) -> (HostCommand, HostResponse) {
    (
        HostCommand::CheckedAudioBusLayout,
        HostResponse::CheckedAudioBusLayout {
            layout: layout.clone(),
        },
    )
}
fn support(processing: bool) -> (HostCommand, HostResponse) {
    (
        HostCommand::ResetOriginSupport,
        HostResponse::ResetOriginSupport {
            support: ResetOriginSupport {
                contract_version: 1,
                max_block_frames: 2048,
                processing,
            },
        },
    )
}
fn activation(
    direction: BusDirection,
    bus_index: usize,
    active: bool,
) -> (HostCommand, HostResponse) {
    (
        HostCommand::SetBusActive {
            media_type: MediaType::Audio,
            direction,
            bus_index: bus_index as i32,
            active,
        },
        success(),
    )
}
fn selection(
    before: &CheckedAudioBusLayout,
    after: &CheckedAudioBusLayout,
) -> Vec<(HostCommand, HostResponse)> {
    let mut pairs = vec![support(false), query(before)];
    for index in 0..before.inputs.len() {
        pairs.push(activation(BusDirection::Input, index, true));
    }
    for index in 0..before.outputs.len() {
        pairs.push(activation(BusDirection::Output, index, index == 0));
    }
    pairs.push(query(after));
    pairs
}
fn process(input_channels: usize) -> (HostCommand, HostResponse) {
    (
        HostCommand::Process {
            inputs: vec![vec![0.0; 128]; input_channels],
            frames: 128,
            transport: Some(transport()),
        },
        HostResponse::AudioOutput {
            outputs: vec![vec![0.25; 128]; 2],
            output_events: Vec::new(),
            output_events_lost: false,
            transport_applied: true,
        },
    )
}

#[test]
fn selected_32_output_generator_acks_every_bus_and_retains_complete_layout() {
    let _serial = serial();
    for inputs in [0, 1, 2] {
        let before = layout(32, inputs, false);
        let after = layout(32, inputs, true);
        let mut pairs = selection(&before, &after);
        pairs.extend([
            query(&after),
            (HostCommand::StartProcessing, success()),
            query(&after),
            query(&after),
            process(inputs),
            query(&after),
            (HostCommand::StopProcessing, success()),
            query(&after),
        ]);
        let mut c = client(&ledger(&pairs, true));
        assert_eq!(
            c.select_main_stereo_output(&before, deadline()).unwrap(),
            after
        );
        c.start_processing(deadline()).unwrap();
        let block = c
            .process_silent_generator(inputs, 2, transport(), deadline())
            .unwrap();
        assert_eq!(block.outputs, vec![vec![0.25; 128]; 2]);
        assert_eq!(block.output_event_count, 0);
        c.stop_processing(deadline()).unwrap();
        finish_client(&mut c);
    }
}

#[test]
fn selected_invalid_shape_sends_no_sdk_mutation_and_processing_is_rejected() {
    let _serial = serial();
    for change in 0..7 {
        let mut before = layout(2, 0, false);
        match change {
            0 => before.outputs.clear(),
            1 => before.outputs[0].is_main = false,
            2 => before.outputs[0].channel_count = 1,
            3 => before.outputs = layout(33, 0, false).outputs,
            4 => before.outputs[1].channel_count = 63,
            5 => before.inputs = layout(1, 3, false).inputs,
            _ => {
                before.inputs = vec![
                    CheckedAudioBusConfig {
                        channel_count: 1,
                        is_main: true,
                        active: None
                    };
                    2
                ]
            }
        }
        let mut c = client(&ledger(&[], false));
        let failure = c
            .select_main_stereo_output(&before, deadline())
            .unwrap_err();
        assert_eq!(failure.kind, FailureKind::Shape);
        assert_eq!(failure.delivery, strict_offline::Delivery::NotSent);
        assert_eq!(failure.request_context, None);
        assert!(c.start_processing(deadline()).is_err());
        finish_client(&mut c);
    }
    let before = layout(2, 0, false);
    let mut c = client(&ledger(&[support(true)], false));
    let failure = c
        .select_main_stereo_output(&before, deadline())
        .unwrap_err();
    assert_eq!(failure.kind, FailureKind::InvalidRequest);
    assert_eq!(
        failure.request_context, None,
        "successful support query is not the failed request"
    );
    finish_client(&mut c);
}

#[test]
fn selected_baseline_descriptor_drift_never_activates_a_bus() {
    let _serial = serial();
    let before = layout(2, 0, false);
    for change in 0..3 {
        let mut changed = before.clone();
        match change {
            0 => changed.outputs[1].channel_count = 1,
            1 => changed.outputs[1].is_main = true,
            _ => changed.outputs.push(changed.outputs[1].clone()),
        }
        let mut c = client(&ledger(&[support(false), query(&changed)], false));
        let failure = c
            .select_main_stereo_output(&before, deadline())
            .unwrap_err();
        assert_eq!(failure.kind, FailureKind::Shape);
        assert_eq!(
            failure.request_context, None,
            "successful layout query is not the failed request"
        );
        finish_client(&mut c);
    }
}

#[test]
fn selected_failed_setter_never_queries_final_layout_or_starts() {
    let _serial = serial();
    let before = layout(3, 0, false);
    let mut pairs = vec![
        support(false),
        query(&before),
        activation(BusDirection::Output, 0, true),
    ];
    let mut rejected = activation(BusDirection::Output, 1, false);
    rejected.1 = HostResponse::Error {
        message: "activateBus failed: SDK rejected output 1".into(),
    };
    pairs.push(rejected);
    let mut c = client(&ledger(&pairs, false));
    let failure = c
        .select_main_stereo_output(&before, deadline())
        .unwrap_err();
    assert_eq!(failure.kind, FailureKind::OperationRejected);
    assert_eq!(failure.delivery, strict_offline::Delivery::MayHaveApplied);
    assert_eq!(
        failure.request_context,
        Some(strict_offline::RequestContext {
            operation: "SetBusActive",
            media_type: Some(MediaType::Audio),
            direction: Some(BusDirection::Output),
            bus_index: Some(1),
        })
    );
    assert_eq!(
        failure.diagnostic.as_deref(),
        Some("activateBus failed: SDK rejected output 1")
    );
    assert!(c.start_processing(deadline()).is_err());
    finish_client(&mut c);
}

#[test]
fn selected_acknowledgment_and_prepared_storage_drift_seal_the_session() {
    let _serial = serial();
    let before = layout(3, 1, false);
    for change in 0..7 {
        let mut after = layout(3, 1, true);
        match change {
            0 => after.outputs[0].active = None,
            1 => after.outputs[1].active = None,
            2 => after.outputs[1].active = Some(true),
            3 => after.outputs[2].channel_count = 1,
            4 => after.outputs[2].is_main = true,
            5 => after.inputs[0].active = Some(false),
            _ => {
                after.outputs.pop();
            }
        }
        let mut c = client(&ledger(&selection(&before, &after), false));
        assert_eq!(
            c.select_main_stereo_output(&before, deadline())
                .unwrap_err()
                .kind,
            FailureKind::Shape
        );
        assert!(c.process(2, transport(), deadline()).is_err());
        finish_client(&mut c);
    }
}

#[test]
fn selected_start_and_process_recheck_layout_before_accepting_audio() {
    let _serial = serial();
    let before = layout(2, 0, false);
    let after = layout(2, 0, true);
    let mut changed = after.clone();
    changed.outputs[1].active = None; // complete SDK descriptor binding was invalidated
    for boundary in 0..5 {
        let mut pairs = selection(&before, &after);
        match boundary {
            0 => pairs.push(query(&changed)),
            1 => pairs.extend([
                query(&after),
                (HostCommand::StartProcessing, success()),
                query(&changed),
            ]),
            2 => pairs.push(query(&changed)),
            3 => pairs.extend([query(&after), process(0), query(&changed)]),
            _ => pairs.extend([(HostCommand::StopProcessing, success()), query(&changed)]),
        }
        let mut c = client(&ledger(&pairs, false));
        c.select_main_stereo_output(&before, deadline()).unwrap();
        let failure = match boundary {
            0 | 1 => c.start_processing(deadline()).unwrap_err(),
            2 | 3 => c.process(2, transport(), deadline()).unwrap_err(),
            _ => c.stop_processing(deadline()).unwrap_err(),
        };
        assert_eq!(failure.kind, FailureKind::Shape);
        assert!(c.start_processing(deadline()).is_err());
        finish_client(&mut c);
    }
}

#[test]
fn selected_input_shape_cannot_be_silently_omitted_or_padded() {
    let _serial = serial();
    let before = layout(2, 1, false);
    let after = layout(2, 1, true);
    for (inputs, outputs) in [(0, 2), (2, 2), (1, 1)] {
        let mut pairs = selection(&before, &after);
        pairs.push(query(&after));
        let mut c = client(&ledger(&pairs, false));
        c.select_main_stereo_output(&before, deadline()).unwrap();
        assert_eq!(
            c.process_silent_generator(inputs, outputs, transport(), deadline())
                .unwrap_err()
                .kind,
            FailureKind::Shape
        );
        finish_client(&mut c);
    }
}

#[test]
fn selected_state_reload_requires_a_new_explicit_selection_before_start() {
    let _serial = serial();
    let before = layout(2, 0, false);
    let after = layout(2, 0, true);
    let mut pairs = selection(&before, &after);
    pairs.push((
        HostCommand::LoadState {
            data: vec![1, 2],
            context: StateContext::Project,
        },
        success(),
    ));
    let mut c = client(&ledger(&pairs, false));
    c.select_main_stereo_output(&before, deadline()).unwrap();
    c.load_state(&[1, 2], deadline()).unwrap();
    assert_eq!(
        c.start_processing(deadline()).unwrap_err().kind,
        FailureKind::Shape
    );
    finish_client(&mut c);
}
