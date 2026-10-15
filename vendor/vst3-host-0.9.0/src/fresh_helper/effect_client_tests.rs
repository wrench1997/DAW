//! Real public Effect clients against ledger-checking Python protocol children.
//! These exercise transport and admission, not a VST3 SDK DSP implementation.
use super::*;
use crate::audio::{BusDirection, MediaType};
use crate::strict_offline::{Delivery, EffectIdentity, Failure};

type Pair = (HostCommand, HostResponse);
const CLASS: &str = "0123456789abcdef0123456789abcdef";
const STATE: &[u8] = &[0, 255, 17, 3];

fn identity() -> EffectIdentity {
    EffectIdentity {
        class_id: CLASS.into(),
        vendor: "ledger vendor".into(),
        name: "ledger effect".into(),
        version: "1.2.3".into(),
        category: "Fx|Dynamics".into(),
    }
}
fn spec() -> strict_offline::LoadSpec<'static> {
    strict_offline::LoadSpec {
        path: "ledger-effect.vst3",
        sample_rate: 48000.0,
        tempo: 120.0,
        time_sig_numerator: 4,
        time_sig_denominator: 4,
        class_id: CLASS,
    }
}
fn load() -> Pair {
    (
        HostCommand::LoadPlugin {
            path: spec().path.into(),
            sample_rate: 48000.0,
            block_size: 2048,
            tempo: 120.0,
            time_sig_numerator: 4,
            time_sig_denominator: 4,
            class_id: Some(CLASS.into()),
        },
        HostResponse::PluginInfo {
            vendor: identity().vendor,
            name: identity().name,
            version: identity().version,
            category: identity().category,
            uid: CLASS.into(),
            has_gui: false,
            audio_inputs: 1,
            audio_outputs: 1,
            output_channels: 2,
            has_midi_input: false,
            has_midi_output: false,
            compatibility: vec![],
        },
    )
}
fn audio(active: Option<bool>) -> CheckedAudioBusLayout {
    let bus = CheckedAudioBusConfig {
        channel_count: 2,
        is_main: true,
        active,
    };
    CheckedAudioBusLayout {
        inputs: vec![bus.clone()],
        outputs: vec![bus],
    }
}
fn query(layout: CheckedAudioBusLayout) -> Pair {
    (
        HostCommand::CheckedAudioBusLayout,
        HostResponse::CheckedAudioBusLayout { layout },
    )
}
fn support() -> Pair {
    (
        HostCommand::ResetOriginSupport,
        HostResponse::ResetOriginSupport {
            support: ResetOriginSupport {
                contract_version: 1,
                max_block_frames: 2048,
                processing: false,
            },
        },
    )
}
fn snapshot() -> crate::AsyncStateSnapshot {
    crate::AsyncStateSnapshot {
        contract_version: 1,
        async_started_revision: 9_007_199_254_740_993,
        active_async: 0,
        active_ui: 1,
        active_other: 0,
        readiness_faulted: false,
    }
}
fn metadata() -> Vec<Pair> {
    vec![
        (
            HostCommand::AsyncStateSnapshot,
            HostResponse::AsyncStateSnapshot {
                snapshot: snapshot(),
            },
        ),
        (
            HostCommand::NativeDirtyRevision,
            HostResponse::NativeDirtyRevision { revision: 17 },
        ),
        query(audio(Some(true))),
        (
            HostCommand::EventBusLayout,
            HostResponse::EventBusLayout {
                layout: EventBusLayout {
                    inputs: vec![],
                    outputs: vec![],
                },
            },
        ),
        (
            HostCommand::LatencySamples,
            HostResponse::LatencySamples { samples: 32 },
        ),
        (
            HostCommand::TailSamples,
            HostResponse::TailSamples { samples: 160 },
        ),
        (
            HostCommand::NativeDirtyRevision,
            HostResponse::NativeDirtyRevision { revision: 17 },
        ),
        (
            HostCommand::AsyncStateSnapshot,
            HostResponse::AsyncStateSnapshot {
                snapshot: snapshot(),
            },
        ),
    ]
}
fn selection() -> Vec<Pair> {
    vec![
        support(),
        query(audio(None)),
        (
            HostCommand::SetBusActive {
                media_type: MediaType::Audio,
                direction: BusDirection::Input,
                bus_index: 0,
                active: true,
            },
            success(),
        ),
        (
            HostCommand::SetBusActive {
                media_type: MediaType::Audio,
                direction: BusDirection::Output,
                bus_index: 0,
                active: true,
            },
            success(),
        ),
        query(audio(Some(true))),
    ]
}
fn prepared() -> Vec<Pair> {
    let mut pairs = vec![
        load(),
        (
            HostCommand::SetProcessMode {
                mode: ProcessMode::Offline,
            },
            success(),
        ),
        (
            HostCommand::LoadState {
                data: STATE.to_vec(),
                context: StateContext::Project,
            },
            success(),
        ),
        (
            HostCommand::GetAllParameters,
            HostResponse::Parameters {
                params: vec![parameter()],
            },
        ),
        (HostCommand::SetParameter { id: 17, value: 0.5 }, success()),
        (
            HostCommand::GetParameter { id: 17 },
            HostResponse::ParameterValue { value: 0.5 },
        ),
        query(audio(None)),
    ];
    pairs.extend(selection());
    pairs
}
fn admission() -> Vec<Pair> {
    let mut pairs = vec![support()];
    pairs.extend(metadata());
    pairs.push((
        HostCommand::ArmOfflineStateReadiness {
            expected_revision: snapshot().async_started_revision,
        },
        success(),
    ));
    pairs.extend(metadata());
    pairs
}
fn start() -> Vec<Pair> {
    let mut pairs = metadata();
    pairs.extend([
        query(audio(Some(true))),
        (HostCommand::StartProcessing, success()),
        query(audio(Some(true))),
    ]);
    pairs.extend(metadata());
    pairs
}
fn stop() -> Vec<Pair> {
    let mut pairs = vec![
        (HostCommand::StopProcessing, success()),
        query(audio(Some(true))),
    ];
    pairs.extend(metadata());
    pairs
}
fn ready_pairs() -> Vec<Pair> {
    let mut pairs = prepared();
    pairs.extend(admission());
    pairs.extend(start());
    pairs
}
fn prepare(c: &mut Client) {
    let loaded = c
        .load_stereo_effect(spec(), &identity(), deadline())
        .unwrap();
    assert_eq!(loaded.uid, CLASS);
    c.set_offline(deadline()).unwrap();
    c.load_state(STATE, deadline()).unwrap();
    let params = c.parameters(deadline()).unwrap();
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].id, 17);
    c.set_parameter(17, 0.5, deadline()).unwrap();
    assert!(strict_offline::captured_base_matches(
        c.parameter(17, deadline()).unwrap(),
        0.5
    ));
    let before = c.checked_audio_layout(deadline()).unwrap();
    assert_eq!(before, audio(None));
    assert_eq!(
        c.select_main_stereo_output(&before, deadline()).unwrap(),
        audio(Some(true))
    );
}
fn ready(c: &mut Client) {
    prepare(c);
    let receipt = c.admit_stereo_effect(deadline()).unwrap();
    assert_eq!(receipt.identity, identity());
    assert_eq!(receipt.path, spec().path);
    assert_eq!(receipt.sample_rate, 48000.0);
    assert_eq!(receipt.max_block_frames, 2048);
    assert_eq!(receipt.tempo, 120.0);
    assert_eq!(
        (receipt.time_sig_numerator, receipt.time_sig_denominator),
        (4, 4)
    );
    assert_eq!(receipt.state_bytes, STATE.len());
    assert_eq!(receipt.audio, audio(Some(true)));
    assert_eq!(
        receipt.events.inputs.len() + receipt.events.outputs.len(),
        0
    );
    assert_eq!((receipt.latency_samples, receipt.tail_samples), (32, 160));
    assert_eq!(
        receipt.async_started_revision,
        snapshot().async_started_revision
    );
    assert_eq!(receipt.native_dirty_revision, 17);
    c.start_processing(deadline()).unwrap();
}
fn input() -> Vec<Vec<f32>> {
    let bits = [
        0,
        0x8000_0000,
        1,
        0x8000_0001,
        0x3e80_0000,
        0xbf00_0000,
        0x4070_0000,
        0xc010_0000,
    ];
    (0..2)
        .map(|channel| {
            (0..128)
                .map(|frame| f32::from_bits(bits[(frame + channel) % bits.len()]))
                .collect()
        })
        .collect()
}
fn bits(input: &[Vec<f32>]) -> Vec<Vec<u32>> {
    input
        .iter()
        .map(|channel| channel.iter().map(|v| v.to_bits()).collect())
        .collect()
}
fn process(inputs: &[Vec<f32>], outputs: Vec<Vec<f32>>) -> Pair {
    (
        HostCommand::Process {
            inputs: inputs.to_vec(),
            frames: 128,
            transport: Some(transport()),
        },
        HostResponse::AudioOutput {
            outputs,
            output_events: vec![],
            output_events_lost: false,
            transport_applied: true,
        },
    )
}
fn processing_pairs(inputs: &[Vec<f32>], reply: HostResponse, post: bool) -> Vec<Pair> {
    let mut pairs = ready_pairs();
    pairs.extend(metadata());
    pairs.push((process(inputs, vec![]).0, reply));
    if post {
        pairs.extend(metadata());
    }
    pairs
}
// The child checks exact base64 bytes against the ledger, decodes each input
// and computes its result from those samples. No canned audio reply is used.
fn transform_ledger(pairs: &[Pair], half_gain: bool) -> String {
    let json = serde_json::to_string(pairs).unwrap();
    format!(
        r#"import base64,json,math,struct,sys
pairs=json.loads({json:?})
for expected,reply in pairs:
 actual=json.loads(sys.stdin.readline())
 assert actual==expected,(actual,expected)
 if isinstance(actual,dict) and 'Process' in actual:
  request=actual['Process']
  assert request['frames']==128 and len(request['inputs'])==2,request
  raw=[base64.b64decode(c,validate=True) for c in request['inputs']]
  assert all(len(c)==512 for c in raw),raw
  values=[struct.unpack('<128f',c) for c in raw]
  assert all(math.isfinite(v) for c in values for v in c),values
  gain=0.5 if {half_gain} else 1.0
  output=[struct.pack('<128f',*(v*gain for v in c)) for c in values]
  reply={{'AudioOutput':{{'outputs':[base64.b64encode(c).decode() for c in output],'output_events':[],'output_events_lost':False,'transport_applied':True}}}}
 print(json.dumps(reply),flush=True)
assert json.loads(sys.stdin.readline())=='Shutdown'
assert sys.stdin.read()==''
"#,
        half_gain = if half_gain { "True" } else { "False" }
    )
}
fn unpublished_failure(c: &mut Client, input: &[Vec<f32>], end: Instant) -> Failure {
    let before = bits(input);
    let pointers: Vec<_> = input.iter().map(Vec::as_ptr).collect();
    let mut destination = vec![vec![-19.25; 128]; 2];
    let sentinel = bits(&destination);
    let result = c.process_stereo_effect(input, transport(), end);
    assert_eq!(bits(input), before);
    assert_eq!(input.iter().map(Vec::as_ptr).collect::<Vec<_>>(), pointers);
    let failure = match result {
        Err(failure) => failure,
        Ok(block) => {
            destination = block.outputs;
            panic!("failed Effect published audio: {:?}", bits(&destination));
        }
    };
    assert_eq!(bits(&destination), sentinel);
    failure
}

#[test]
fn effect_client_known_pcm_transform_is_computed_by_child_with_exact_receipt() {
    let _serial = serial();
    let input = input();
    let original = bits(&input);
    let mut pairs = ready_pairs();
    pairs.extend(metadata());
    pairs.push(process(&input, vec![]));
    pairs.extend(metadata());
    pairs.extend(stop());
    let mut c = client(&transform_ledger(&pairs, true));
    ready(&mut c);
    let output = c
        .process_stereo_effect(&input, transport(), deadline())
        .unwrap();
    assert_eq!(bits(&input), original);
    assert_eq!(
        bits(&output.outputs),
        bits(
            &input
                .iter()
                .map(|channel| channel.iter().map(|v| v * 0.5).collect())
                .collect::<Vec<_>>()
        )
    );
    assert_ne!(bits(&output.outputs), original);
    assert!(output.outputs.iter().flatten().any(|v| v.abs() > 1.0));
    assert_eq!(output.output_event_count, 0);
    assert!(!output.output_events_lost && output.transport_applied);
    c.stop_processing(deadline()).unwrap();
    finish_client(&mut c);
}

#[test]
fn effect_client_finite_pcm_bits_and_explicit_zero_tail_input_are_not_substituted() {
    let _serial = serial();
    let mut input = input();
    input[0][120] = f32::MAX;
    input[1][120] = f32::MIN;
    let zeros = vec![vec![0.0; 128]; 2];
    let mut pairs = ready_pairs();
    for samples in [&input, &zeros] {
        pairs.extend(metadata());
        pairs.push(process(samples, vec![]));
        pairs.extend(metadata());
    }
    pairs.extend(stop());
    let mut c = client(&transform_ledger(&pairs, false));
    ready(&mut c);
    for samples in [&input, &zeros] {
        let before = bits(samples);
        let out = c
            .process_stereo_effect(samples, transport(), deadline())
            .unwrap();
        assert_eq!(bits(&out.outputs), before);
        assert_eq!(bits(samples), before);
    }
    c.stop_processing(deadline()).unwrap();
    finish_client(&mut c);
}

#[test]
fn effect_client_wrong_actual_identity_role_and_load_shape_seal_without_fallback() {
    let _serial = serial();
    for change in 0..11 {
        let mut pair = load();
        if let HostResponse::PluginInfo {
            uid,
            vendor,
            name,
            version,
            category,
            has_midi_input,
            has_midi_output,
            audio_inputs,
            audio_outputs,
            output_channels,
            ..
        } = &mut pair.1
        {
            match change {
                0 => *uid = "fedcba9876543210fedcba9876543210".into(),
                1 => vendor.push('!'),
                2 => name.push('!'),
                3 => version.push('!'),
                4 => *category = "Instrument".into(),
                5 => *category = "Fx|Instrument".into(),
                6 => *has_midi_input = true,
                7 => *has_midi_output = true,
                8 => *audio_inputs = 0,
                9 => *audio_outputs = 2,
                _ => *output_channels = 1,
            }
        }
        let mut c = client(&ledger(&[pair], false));
        let failure = c
            .load_stereo_effect(spec(), &identity(), deadline())
            .unwrap_err();
        assert_eq!(
            (failure.kind, failure.delivery),
            (FailureKind::Shape, Delivery::NotSent)
        );
        assert_eq!(failure.request_context, None);
        let repeated = unpublished_failure(&mut c, &input(), deadline());
        assert_eq!(
            (repeated.kind, repeated.delivery),
            (FailureKind::Shape, Delivery::NotSent)
        );
        finish_client(&mut c);
    }
}

#[test]
fn effect_client_invalid_expected_role_is_unsent_and_cancellation_precedes_validation() {
    let _serial = serial();
    for category in ["Effect", "NotFx", "Fx|Instrument", "FxExtra", ""] {
        for cancelled in [false, true] {
            let mut expected = identity();
            expected.category = category.into();
            let mut c = client(&ledger(&[], false));
            if cancelled {
                c.cancellation().cancel();
            }
            let failure = c
                .load_stereo_effect(spec(), &expected, deadline())
                .unwrap_err();
            assert_eq!(failure.delivery, Delivery::NotSent);
            assert_eq!(
                failure.kind,
                if cancelled {
                    FailureKind::Cancelled
                } else {
                    FailureKind::InvalidRequest
                }
            );
            finish_client(&mut c);
        }
    }
    for field in 0..4 {
        for oversized in [false, true] {
            let mut expected = identity();
            let value = if oversized {
                if field == 3 {
                    format!("Fx|{}", "x".repeat(65_534))
                } else {
                    "x".repeat(65_537)
                }
            } else {
                String::new()
            };
            assert_eq!(value.len(), if oversized { 65_537 } else { 0 });
            match field {
                0 => expected.vendor = value,
                1 => expected.name = value,
                2 => expected.version = value,
                _ => expected.category = value,
            }
            let mut c = client(&ledger(&[], false));
            let failure = c
                .load_stereo_effect(spec(), &expected, deadline())
                .unwrap_err();
            assert_eq!(
                (failure.kind, failure.delivery),
                (FailureKind::InvalidRequest, Delivery::NotSent)
            );
            assert_eq!(failure.request_context, None);
            finish_client(&mut c);
        }
    }
}

#[test]
fn effect_client_unprepared_and_unstarted_processing_cannot_publish() {
    let _serial = serial();
    for state in 0..3 {
        let pairs = if state == 0 { vec![] } else { prepared() };
        let mut c = client(&ledger(&pairs, false));
        if state > 0 {
            prepare(&mut c);
        }
        let failure = if state == 2 {
            c.start_processing(deadline()).unwrap_err()
        } else {
            unpublished_failure(&mut c, &input(), deadline())
        };
        assert_eq!(
            (failure.kind, failure.delivery),
            (FailureKind::InvalidRequest, Delivery::NotSent)
        );
        finish_client(&mut c);
    }
}

#[test]
fn effect_client_admission_needs_explicit_offline_state_and_selection() {
    let _serial = serial();
    for missing in 0..3 {
        let mut pairs = vec![load()];
        if missing != 0 {
            pairs.push((
                HostCommand::SetProcessMode {
                    mode: ProcessMode::Offline,
                },
                success(),
            ));
        }
        if missing != 1 {
            pairs.push((
                HostCommand::LoadState {
                    data: STATE.to_vec(),
                    context: StateContext::Project,
                },
                success(),
            ));
        }
        if missing != 2 {
            pairs.extend(selection());
        }
        let mut c = client(&ledger(&pairs, false));
        c.load_stereo_effect(spec(), &identity(), deadline())
            .unwrap();
        if missing != 0 {
            c.set_offline(deadline()).unwrap();
        }
        if missing != 1 {
            c.load_state(STATE, deadline()).unwrap();
        }
        if missing != 2 {
            c.select_main_stereo_output(&audio(None), deadline())
                .unwrap();
        }
        let failure = c.admit_stereo_effect(deadline()).unwrap_err();
        assert_eq!(
            (failure.kind, failure.delivery),
            (FailureKind::InvalidRequest, Delivery::NotSent)
        );
        finish_client(&mut c);
    }
}

#[test]
fn effect_client_admission_requires_stopped_v1_exact_2048_support() {
    let _serial = serial();
    for change in 0..4 {
        let mut pair = support();
        if let HostResponse::ResetOriginSupport { support } = &mut pair.1 {
            match change {
                0 => support.processing = true,
                1 => support.contract_version = 2,
                2 => support.max_block_frames = 127,
                _ => support.max_block_frames = 128,
            }
        }
        let mut pairs = prepared();
        pairs.push(pair);
        let mut c = client(&ledger(&pairs, false));
        prepare(&mut c);
        let failure = c.admit_stereo_effect(deadline()).unwrap_err();
        assert_eq!(
            (failure.kind, failure.delivery),
            (FailureKind::Shape, Delivery::NotSent)
        );
        assert!(c.start_processing(deadline()).is_err());
        finish_client(&mut c);
    }
}

fn changed_metadata(change: usize) -> Vec<Pair> {
    let mut pairs = metadata();
    match change {
        0..=3 => {
            if let HostResponse::AsyncStateSnapshot { snapshot } = &mut pairs[0].1 {
                match change {
                    0 => snapshot.active_async = 1,
                    1 => snapshot.active_other = 1,
                    2 => snapshot.readiness_faulted = true,
                    _ => snapshot.contract_version = 2,
                }
            }
        }
        4 => {
            if let HostResponse::AsyncStateSnapshot { snapshot } = &mut pairs[7].1 {
                snapshot.async_started_revision += 1;
            }
        }
        5 => pairs[6].1 = HostResponse::NativeDirtyRevision { revision: 18 },
        6..=9 => {
            if let HostResponse::CheckedAudioBusLayout { layout } = &mut pairs[2].1 {
                match change {
                    6 => layout.inputs[0].channel_count = 1,
                    7 => layout.outputs[0].active = None,
                    8 => layout.inputs.push(layout.inputs[0].clone()),
                    _ => layout.outputs[0].is_main = false,
                }
            }
        }
        10 | 11 => {
            if let HostResponse::EventBusLayout { layout } = &mut pairs[3].1 {
                let bus = EventBusConfig {
                    channel_count: 16,
                    is_main: true,
                    active: Some(true),
                };
                if change == 10 {
                    layout.inputs.push(bus);
                } else {
                    layout.outputs.push(bus);
                }
            }
        }
        12 => pairs[4].1 = HostResponse::LatencySamples { samples: 33 },
        _ => pairs[5].1 = HostResponse::TailSamples { samples: 161 },
    }
    pairs
}

#[test]
fn effect_client_admission_refuses_busy_drifting_or_nonstereo_metadata_before_arm() {
    let _serial = serial();
    for change in 0..12 {
        let mut pairs = prepared();
        pairs.push(support());
        let mut changed = changed_metadata(change);
        // Invalid wire contract is rejected by the first getter's codec;
        // readiness and descriptor refusals consume the full fresh bundle.
        if change == 3 {
            changed.truncate(1);
        }
        pairs.extend(changed);
        let mut c = client(&ledger(&pairs, false));
        prepare(&mut c);
        let failure = c.admit_stereo_effect(deadline()).unwrap_err();
        assert_eq!(
            (failure.kind, failure.delivery),
            if change == 3 {
                (FailureKind::MalformedReply, Delivery::MayHaveApplied)
            } else {
                (FailureKind::Shape, Delivery::NotSent)
            },
            "change {change}"
        );
        finish_client(&mut c);
    }
}

#[test]
fn effect_client_arm_brackets_metadata_and_keeps_exact_latency_tail() {
    let _serial = serial();
    for change in [4, 5, 6, 10, 12, 13] {
        let mut pairs = prepared();
        pairs.push(support());
        pairs.extend(metadata());
        pairs.push((
            HostCommand::ArmOfflineStateReadiness {
                expected_revision: snapshot().async_started_revision,
            },
            success(),
        ));
        pairs.extend(changed_metadata(change));
        let mut c = client(&ledger(&pairs, false));
        prepare(&mut c);
        let failure = c.admit_stereo_effect(deadline()).unwrap_err();
        assert_eq!(
            (failure.kind, failure.delivery),
            (FailureKind::Shape, Delivery::NotSent)
        );
        finish_client(&mut c);
    }
}

#[test]
fn effect_client_start_and_process_recheck_metadata_on_both_sides() {
    let _serial = serial();
    for boundary in 0..4 {
        let mut pairs = prepared();
        pairs.extend(admission());
        if boundary > 1 {
            pairs.extend(start());
        }
        if boundary % 2 == 1 {
            pairs.extend(metadata());
            if boundary == 1 {
                pairs.extend([
                    query(audio(Some(true))),
                    (HostCommand::StartProcessing, success()),
                    query(audio(Some(true))),
                ]);
            } else {
                pairs.push(process(&input(), vec![vec![0.25; 128]; 2]));
            }
        }
        pairs.extend(changed_metadata(12));
        let mut c = client(&ledger(&pairs, false));
        prepare(&mut c);
        c.admit_stereo_effect(deadline()).unwrap();
        if boundary > 1 {
            c.start_processing(deadline()).unwrap();
        }
        let failure = if boundary < 2 {
            c.start_processing(deadline()).unwrap_err()
        } else {
            unpublished_failure(&mut c, &input(), deadline())
        };
        assert_eq!(
            (failure.kind, failure.delivery),
            (FailureKind::Shape, Delivery::NotSent),
            "boundary {boundary}"
        );
        finish_client(&mut c);
    }
}

#[test]
fn effect_client_start_rechecks_selected_bus_acknowledgment_before_and_after_start() {
    let _serial = serial();
    for after_start in [false, true] {
        let mut pairs = prepared();
        pairs.extend(admission());
        pairs.extend(metadata());
        if after_start {
            pairs.extend([
                query(audio(Some(true))),
                (HostCommand::StartProcessing, success()),
            ]);
        }
        pairs.push(query(audio(Some(false))));
        let mut c = client(&ledger(&pairs, false));
        prepare(&mut c);
        c.admit_stereo_effect(deadline()).unwrap();
        let failure = c.start_processing(deadline()).unwrap_err();
        assert_eq!(
            (failure.kind, failure.delivery),
            (FailureKind::Shape, Delivery::NotSent)
        );
        finish_client(&mut c);
    }
}

#[test]
fn effect_client_state_parameter_and_selection_changes_invalidate_receipt() {
    let _serial = serial();
    for change in 0..6 {
        let mut pairs = prepared();
        pairs.extend(admission());
        match change {
            0 => pairs.push((
                HostCommand::LoadState {
                    data: STATE.to_vec(),
                    context: StateContext::Project,
                },
                success(),
            )),
            1 => pairs.push((HostCommand::SetParameter { id: 17, value: 0.5 }, success())),
            2 => {
                let mut selected_again = selection();
                selected_again[1] = query(audio(Some(true)));
                pairs.extend(selected_again);
            }
            3 => pairs.push((
                HostCommand::SetProcessMode {
                    mode: ProcessMode::Offline,
                },
                success(),
            )),
            4 => pairs.push((
                HostCommand::ArmOfflineStateReadiness {
                    expected_revision: snapshot().async_started_revision,
                },
                success(),
            )),
            _ => pairs.push((
                HostCommand::SetParameter {
                    id: 17,
                    value: f64::from_bits(0.75_f64.to_bits() + 1),
                },
                success(),
            )),
        }
        let mut c = client(&ledger(&pairs, false));
        prepare(&mut c);
        c.admit_stereo_effect(deadline()).unwrap();
        match change {
            0 => c.load_state(STATE, deadline()).unwrap(),
            1 => c.set_parameter(17, 0.5, deadline()).unwrap(),
            2 => {
                c.select_main_stereo_output(&audio(Some(true)), deadline())
                    .unwrap();
            }
            3 => c.set_offline(deadline()).unwrap(),
            4 => c
                .arm_offline_state_readiness(snapshot().async_started_revision, deadline())
                .unwrap(),
            _ => c
                .set_parameter_f64(17, f64::from_bits(0.75_f64.to_bits() + 1), deadline())
                .unwrap(),
        }
        let failure = c.start_processing(deadline()).unwrap_err();
        assert_eq!(
            (failure.kind, failure.delivery),
            (FailureKind::InvalidRequest, Delivery::NotSent)
        );
        finish_client(&mut c);
    }
}

#[test]
fn effect_client_readonly_layout_query_keeps_receipt_and_tail_max_is_not_zeroed() {
    let _serial = serial();
    let mut pairs = prepared();
    let mut admit = admission();
    for pair in &mut admit {
        if matches!(pair.0, HostCommand::TailSamples) {
            pair.1 = HostResponse::TailSamples { samples: u32::MAX };
        }
    }
    pairs.extend(admit);
    pairs.push(query(audio(Some(true))));
    let mut after_query = start();
    after_query.extend(stop());
    for pair in &mut after_query {
        if matches!(pair.0, HostCommand::TailSamples) {
            pair.1 = HostResponse::TailSamples { samples: u32::MAX };
        }
    }
    pairs.extend(after_query);
    let mut c = client(&ledger(&pairs, true));
    prepare(&mut c);
    let receipt = c.admit_stereo_effect(deadline()).unwrap();
    assert_eq!(
        (receipt.latency_samples, receipt.tail_samples),
        (32, u32::MAX)
    );
    assert_eq!(c.checked_audio_layout(deadline()).unwrap(), receipt.audio);
    c.start_processing(deadline()).unwrap();
    c.stop_processing(deadline()).unwrap();
    finish_client(&mut c);
}

#[test]
fn effect_client_rejects_empty_wrong_channel_and_frame_input_without_process_bytes() {
    let _serial = serial();
    for input in [
        vec![],
        vec![vec![0.0; 128]],
        vec![vec![0.0; 128]; 3],
        vec![vec![0.0; 127], vec![0.0; 128]],
        vec![vec![0.0; 128], vec![0.0; 129]],
    ] {
        let mut pairs = ready_pairs();
        pairs.extend(metadata());
        let mut c = client(&ledger(&pairs, false));
        ready(&mut c);
        let failure = unpublished_failure(&mut c, &input, deadline());
        assert_eq!(
            (failure.kind, failure.delivery),
            (FailureKind::Shape, Delivery::NotSent)
        );
        assert_eq!(failure.request_context.unwrap().operation, "Process");
        finish_client(&mut c);
    }
}

#[test]
fn effect_client_nonfinite_input_is_unsent_and_preserves_every_source_bit() {
    let _serial = serial();
    for sample in [
        f32::from_bits(0x7fc0_0123),
        f32::INFINITY,
        f32::NEG_INFINITY,
    ] {
        let mut input = input();
        input[1][127] = sample;
        let mut pairs = ready_pairs();
        pairs.extend(metadata());
        let mut c = client(&ledger(&pairs, false));
        ready(&mut c);
        let failure = unpublished_failure(&mut c, &input, deadline());
        assert_eq!(
            (failure.kind, failure.delivery),
            (FailureKind::InvalidRequest, Delivery::NotSent)
        );
        finish_client(&mut c);
    }
}

#[test]
fn effect_client_invalid_transport_is_not_sent_as_a_valid_audio_request() {
    let _serial = serial();
    for change in 0..4 {
        let mut t = transport();
        match change {
            0 => t.tempo = 0.0,
            1 => t.quarter_note_position = f64::NAN,
            2 => t.time_sig_numerator = 0,
            _ => t.time_sig_denominator = 3,
        }
        let mut pairs = ready_pairs();
        pairs.extend(metadata());
        let mut c = client(&ledger(&pairs, false));
        ready(&mut c);
        let input = input();
        let before = bits(&input);
        let failure = c.process_stereo_effect(&input, t, deadline()).unwrap_err();
        assert_eq!(bits(&input), before);
        assert_eq!(
            (failure.kind, failure.delivery),
            (FailureKind::InvalidRequest, Delivery::NotSent)
        );
        finish_client(&mut c);
    }
}

#[test]
fn effect_client_wrong_output_shape_and_nonfinite_loss_transport_never_escape() {
    let _serial = serial();
    for change in 0..8 {
        let input = input();
        let mut reply = process(&input, vec![vec![0.25; 128]; 2]).1;
        if let HostResponse::AudioOutput {
            outputs,
            output_events_lost,
            transport_applied,
            ..
        } = &mut reply
        {
            match change {
                0 => {
                    outputs.pop();
                }
                1 => outputs.push(vec![0.0; 128]),
                2 => {
                    outputs[1].pop();
                }
                3 => outputs[0].push(0.0),
                4 => outputs[1][127] = f32::from_bits(0x7fc0_0123),
                5 => outputs[0][0] = f32::INFINITY,
                6 => *output_events_lost = true,
                _ => *transport_applied = false,
            }
        }
        let pairs = processing_pairs(&input, reply, change >= 4);
        let mut c = client(&ledger(&pairs, false));
        ready(&mut c);
        let failure = unpublished_failure(&mut c, &input, deadline());
        assert_eq!(failure.kind, FailureKind::Shape);
        assert_eq!(
            failure.delivery,
            if change < 4 {
                Delivery::MayHaveApplied
            } else {
                Delivery::NotSent
            }
        );
        assert_eq!(
            failure.request_context.map(|c| c.operation),
            if change < 4 { Some("Process") } else { None }
        );
        finish_client(&mut c);
    }
}

#[test]
fn effect_client_generator_and_midi_entry_points_are_never_sent_on_effect() {
    let _serial = serial();
    for operation in 0..3 {
        let mut c = client(&ledger(&[load()], false));
        c.load_stereo_effect(spec(), &identity(), deadline())
            .unwrap();
        let failure = match operation {
            0 => c.process(2, transport(), deadline()).unwrap_err(),
            1 => c
                .process_silent_generator(2, 2, transport(), deadline())
                .unwrap_err(),
            _ => c
                .send_note(
                    MidiEvent::NoteOn {
                        channel: MidiChannel::Ch1,
                        note: 60,
                        velocity: 100,
                    },
                    0,
                    deadline(),
                )
                .unwrap_err(),
        };
        assert_eq!(
            (failure.kind, failure.delivery),
            (FailureKind::UnsupportedRequest, Delivery::NotSent)
        );
        finish_client(&mut c);
    }
}

#[test]
fn effect_client_malformed_and_operation_error_keep_failed_process_and_reap() {
    let _serial = serial();
    for malformed in [true, false] {
        let input = input();
        let mut pairs = ready_pairs();
        pairs.extend(metadata());
        let json = serde_json::to_string(&pairs).unwrap();
        let expected = serde_json::to_string(&process(&input, vec![]).0).unwrap();
        let response = if malformed {
            "not-json"
        } else {
            "{\"Error\":{\"message\":\"effect process rejected after possible DSP mutation\"}}"
        };
        let script = format!("import json,sys\nfor expected,reply in json.loads({json:?}):\n actual=json.loads(sys.stdin.readline())\n assert actual==expected,(actual,expected)\n print(json.dumps(reply),flush=True)\nassert json.loads(sys.stdin.readline())==json.loads({expected:?})\nprint({response:?},flush=True)\nassert sys.stdin.read()==''\n");
        let mut c = client(&script);
        ready(&mut c);
        let failure = unpublished_failure(&mut c, &input, deadline());
        assert_eq!(
            failure.kind,
            if malformed {
                FailureKind::MalformedReply
            } else {
                FailureKind::OperationRejected
            }
        );
        assert_eq!(failure.delivery, Delivery::MayHaveApplied);
        assert_eq!(failure.request_context.unwrap().operation, "Process");
        if !malformed {
            assert_eq!(
                failure.diagnostic.as_deref(),
                Some("effect process rejected after possible DSP mutation")
            );
        }
        finish_client(&mut c);
    }
}

#[test]
fn effect_client_nonempty_output_events_are_unsupported_not_discarded() {
    let _serial = serial();
    let input = input();
    let mut pairs = ready_pairs();
    pairs.extend(metadata());
    let json = serde_json::to_string(&pairs).unwrap();
    let expected = serde_json::to_string(&process(&input, vec![]).0).unwrap();
    let mut reply = serde_json::to_value(process(&input, vec![vec![0.25; 128]; 2]).1).unwrap();
    reply["AudioOutput"]["output_events"] = serde_json::json!([{}]);
    let reply = serde_json::to_string(&reply).unwrap();
    let script = format!("import json,sys\nfor expected,reply in json.loads({json:?}):\n actual=json.loads(sys.stdin.readline())\n assert actual==expected,(actual,expected)\n print(json.dumps(reply),flush=True)\nassert json.loads(sys.stdin.readline())==json.loads({expected:?})\nprint({reply:?},flush=True)\nassert sys.stdin.read()==''\n");
    let mut c = client(&script);
    ready(&mut c);
    let failure = unpublished_failure(&mut c, &input, deadline());
    assert_eq!(
        (failure.kind, failure.delivery),
        (FailureKind::UnsupportedReply, Delivery::MayHaveApplied)
    );
    assert_eq!(failure.request_context.unwrap().operation, "Process");
    finish_client(&mut c);
}

#[test]
fn effect_client_deadline_and_sticky_cancel_preserve_pcm_without_followup() {
    let _serial = serial();
    for cancelled in [false, true] {
        let mut c = client(&ledger(&ready_pairs(), false));
        ready(&mut c);
        if cancelled {
            c.cancellation().cancel();
        }
        let end = if cancelled {
            deadline()
        } else {
            Instant::now()
        };
        let failure = unpublished_failure(&mut c, &input(), end);
        assert_eq!(
            (failure.kind, failure.delivery),
            (
                if cancelled {
                    FailureKind::Cancelled
                } else {
                    FailureKind::Deadline
                },
                Delivery::NotSent
            )
        );
        finish_client(&mut c);
    }
}

#[test]
fn effect_client_sent_process_deadline_and_cancel_keep_delivery_and_pcm() {
    let _serial = serial();
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    for cancelled in [false, true] {
        let path = std::env::temp_dir().join(format!(
            "strict-effect-process-latch-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        struct Remove(std::path::PathBuf);
        impl Drop for Remove {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        let _remove = Remove(path.clone());
        let path_json = serde_json::to_string(path.to_str().unwrap()).unwrap();
        let input = input();
        let mut pairs = ready_pairs();
        pairs.extend(metadata());
        let json = serde_json::to_string(&pairs).unwrap();
        let expected = serde_json::to_string(&process(&input, vec![]).0).unwrap();
        let script = format!("import json,sys\nfor expected,reply in json.loads({json:?}):\n actual=json.loads(sys.stdin.readline())\n assert actual==expected,(actual,expected)\n print(json.dumps(reply),flush=True)\nassert json.loads(sys.stdin.readline())==json.loads({expected:?})\nwith open({path_json},'x') as out: out.write('accepted Effect Process')\nassert sys.stdin.read()==''\n");
        let mut c = client(&script);
        ready(&mut c);
        let cancellation = c.cancellation();
        let end = deadline();
        let failure = std::thread::scope(|scope| {
            let worker = cancelled.then(|| {
                scope.spawn(|| {
                    while !path.exists() {
                        assert!(
                            Instant::now() < end,
                            "Effect Process admission latch deadline"
                        );
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    cancellation.cancel();
                })
            });
            let failure = unpublished_failure(&mut c, &input, end);
            if let Some(worker) = worker {
                worker.join().unwrap();
            }
            failure
        });
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "accepted Effect Process"
        );
        assert_eq!(
            (failure.kind, failure.delivery),
            (
                if cancelled {
                    FailureKind::Cancelled
                } else {
                    FailureKind::Deadline
                },
                Delivery::MayHaveApplied
            )
        );
        assert_eq!(failure.request_context.unwrap().operation, "Process");
        finish_client(&mut c);
    }
}

#[test]
fn effect_client_pending_cleanup_retains_reservation_until_actual_reap() {
    let _serial = serial();
    let pairs = ready_pairs();
    let json = serde_json::to_string(&pairs).unwrap();
    let script = format!("import json,sys,time\nfor expected,reply in json.loads({json:?}):\n actual=json.loads(sys.stdin.readline())\n assert actual==expected,(actual,expected)\n print(json.dumps(reply),flush=True)\nassert sys.stdin.read()==''\ntime.sleep(30)\n");
    let mut c = client(&script);
    ready(&mut c);
    let failure = unpublished_failure(&mut c, &input(), Instant::now());
    assert_eq!(
        (failure.kind, failure.delivery),
        (FailureKind::Deadline, Delivery::NotSent)
    );
    let report = c.dispose(Instant::now(), Instant::now());
    assert!(matches!(
        report.state,
        strict_offline::CleanupState::Pending
    ));
    std::thread::sleep(Duration::from_millis(110));
    pump::test_hooks::arm(pump::test_hooks::Fault::Kill);
    let report = c.continue_cleanup().unwrap();
    assert_eq!(report.kill_error, Some(libc::EPERM));
    assert!(matches!(
        report.state,
        strict_offline::CleanupState::Pending
    ));
    assert!(
        matches!(Client::spawn(&mut python("raise AssertionError('reserved until reap')"), deadline()), Err(f) if f.failure.kind == FailureKind::Busy && f.cleanup.is_none())
    );
    let end = deadline();
    loop {
        let report = c.continue_cleanup().unwrap();
        if matches!(report.state, strict_offline::CleanupState::Reaped(_)) {
            assert!(report.kill_requested);
            assert_eq!(report.kill_error, Some(libc::EPERM));
            break;
        }
        assert!(Instant::now() < end, "Effect child actual reap deadline");
        std::thread::sleep(Duration::from_millis(1));
    }
    let mut next = client(&ledger(&[], true));
    finish_client(&mut next);
}

#[test]
fn effect_client_bound_role_cannot_reload_or_reopen_generator_before_actual_reap() {
    let _serial = serial();
    for started in [false, true] {
        for effect_entry in [false, true] {
            let pairs = if started { ready_pairs() } else { vec![load()] };
            let json = serde_json::to_string(&pairs).unwrap();
            // There is deliberately no second LoadPlugin reply. Any attempted
            // reload or fallback makes this real child assertion fail.
            let script = format!("import json,sys,time\nfor expected,reply in json.loads({json:?}):\n actual=json.loads(sys.stdin.readline())\n assert actual==expected,(actual,expected)\n print(json.dumps(reply),flush=True)\nassert sys.stdin.read()==''\ntime.sleep(30)\n");
            let mut c = client(&script);
            if started {
                ready(&mut c);
            } else {
                c.load_stereo_effect(spec(), &identity(), deadline())
                    .unwrap();
            }
            let failure = if effect_entry {
                c.load_stereo_effect(spec(), &identity(), deadline())
                    .unwrap_err()
            } else {
                c.load_plugin(spec(), deadline()).unwrap_err()
            };
            assert_eq!(
                (failure.kind, failure.delivery),
                (FailureKind::UnsupportedRequest, Delivery::NotSent)
            );
            assert_eq!(failure.request_context, None);
            assert_eq!(
                c.process_silent_generator(2, 2, transport(), deadline())
                    .unwrap_err()
                    .kind,
                FailureKind::UnsupportedRequest
            );
            assert_eq!(
                c.send_note(
                    MidiEvent::NoteOn {
                        channel: MidiChannel::Ch1,
                        note: 60,
                        velocity: 100
                    },
                    0,
                    deadline()
                )
                .unwrap_err()
                .kind,
                FailureKind::UnsupportedRequest
            );
            assert_eq!(
                c.start_processing(deadline()).unwrap_err().kind,
                FailureKind::UnsupportedRequest
            );
            assert_eq!(
                unpublished_failure(&mut c, &input(), deadline()).kind,
                FailureKind::UnsupportedRequest
            );
            let report = c.dispose(Instant::now(), Instant::now());
            assert!(matches!(
                report.state,
                strict_offline::CleanupState::Pending
            ));
            assert!(
                matches!(Client::spawn(&mut python("raise AssertionError('role failure still owns child')"), deadline()), Err(f) if f.failure.kind == FailureKind::Busy && f.cleanup.is_none())
            );
            let end = deadline();
            loop {
                let report = c.continue_cleanup().unwrap();
                if matches!(report.state, strict_offline::CleanupState::Reaped(_)) {
                    assert!(report.kill_requested);
                    assert!(
                        report.shutdown_failure.is_none()
                            && report.kill_error.is_none()
                            && report.wait_error.is_none()
                            && report.pipe_error.is_none()
                    );
                    break;
                }
                assert!(
                    Instant::now() < end,
                    "bound Effect role child actual reap deadline"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
            let mut next = client(&ledger(&[], true));
            finish_client(&mut next);
        }
    }
    // The sticky-role precondition belongs only to the new Effect path. An old
    // unbound generic caller still reloads and uses its original silent input.
    let samples = vec![vec![0.0; 128]; 2];
    let mut legacy = client(&ledger(
        &[load(), load(), process(&samples, vec![vec![0.25; 128]; 2])],
        true,
    ));
    legacy.load_plugin(spec(), deadline()).unwrap();
    legacy.load_plugin(spec(), deadline()).unwrap();
    assert_eq!(
        legacy
            .process_silent_generator(2, 2, transport(), deadline())
            .unwrap()
            .outputs,
        vec![vec![0.25; 128]; 2]
    );
    finish_client(&mut legacy);
}

#[test]
fn effect_client_f64_parameter_preserves_one_ulp_and_old_f32_wire_semantics() {
    let _serial = serial();
    let precise = f64::from_bits(0.75_f64.to_bits() + 1);
    assert_ne!(precise.to_bits(), f64::from(precise as f32).to_bits());
    let values = [0.0, -0.0, f64::from_bits(1), precise, 1.0];
    let old_f32 = f32::from_bits(0.75_f32.to_bits() + 1);
    let mut pairs = Vec::new();
    for value in values.into_iter().chain([f64::from(old_f32)]) {
        pairs.extend([
            (HostCommand::SetParameter { id: 17, value }, success()),
            (
                HostCommand::GetParameter { id: 17 },
                HostResponse::ParameterValue { value },
            ),
        ]);
    }
    let mut c = client(&ledger(&pairs, true));
    for value in values {
        c.set_parameter_f64(17, value, deadline()).unwrap();
        assert_eq!(
            c.parameter(17, deadline()).unwrap().to_bits(),
            value.to_bits()
        );
    }
    c.set_parameter(17, old_f32, deadline()).unwrap();
    assert_eq!(
        c.parameter(17, deadline()).unwrap().to_bits(),
        f64::from(old_f32).to_bits()
    );
    finish_client(&mut c);
}

#[test]
fn effect_client_f64_parameter_bounds_cancel_delivery_and_retirement_are_strict() {
    let _serial = serial();
    for value in [
        -f64::from_bits(1),
        f64::from_bits(1.0_f64.to_bits() + 1),
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ] {
        for stop in 0..4 {
            let mut c = client(&ledger(&[], false));
            if stop >= 2 {
                c.cancellation().cancel();
            }
            let end = if stop == 1 || stop == 3 {
                Instant::now()
            } else {
                deadline()
            };
            let failure = c.set_parameter_f64(17, value, end).unwrap_err();
            let expected = match stop {
                1 => FailureKind::Deadline,
                2 | 3 => FailureKind::Cancelled,
                _ => FailureKind::InvalidRequest,
            };
            assert_eq!(
                (failure.kind, failure.delivery),
                (expected, Delivery::NotSent)
            );
            assert_eq!(failure.request_context.unwrap().operation, "SetParameter");
            let repeated = c.set_parameter_f64(17, 0.5, deadline()).unwrap_err();
            assert_eq!(
                (repeated.kind, repeated.delivery),
                (expected, Delivery::NotSent)
            );
            finish_client(&mut c);
        }
    }

    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    for cancelled in [false, true] {
        let path = std::env::temp_dir().join(format!(
            "strict-effect-f64-setter-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        struct Remove(std::path::PathBuf);
        impl Drop for Remove {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        let _remove = Remove(path.clone());
        let value = f64::from_bits(0.75_f64.to_bits() + 1);
        let mut pairs = prepared();
        pairs.extend(admission());
        let json = serde_json::to_string(&pairs).unwrap();
        let expected = serde_json::to_string(&HostCommand::SetParameter { id: 17, value }).unwrap();
        let path_json = serde_json::to_string(path.to_str().unwrap()).unwrap();
        let script = format!("import json,sys\nfor expected,reply in json.loads({json:?}):\n actual=json.loads(sys.stdin.readline())\n assert actual==expected,(actual,expected)\n print(json.dumps(reply),flush=True)\nassert json.loads(sys.stdin.readline())==json.loads({expected:?})\nwith open({path_json},'x') as out: out.write('accepted exact f64 parameter')\nassert sys.stdin.read()==''\n");
        let mut c = client(&script);
        prepare(&mut c);
        c.admit_stereo_effect(deadline()).unwrap();
        let cancellation = c.cancellation();
        let end = deadline();
        let failure = std::thread::scope(|scope| {
            let worker = cancelled.then(|| {
                scope.spawn(|| {
                    while !path.exists() {
                        assert!(
                            Instant::now() < end,
                            "f64 SetParameter admission latch deadline"
                        );
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    cancellation.cancel();
                })
            });
            let failure = c.set_parameter_f64(17, value, end).unwrap_err();
            if let Some(worker) = worker {
                worker.join().unwrap();
            }
            failure
        });
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "accepted exact f64 parameter"
        );
        assert_eq!(
            (failure.kind, failure.delivery),
            (
                if cancelled {
                    FailureKind::Cancelled
                } else {
                    FailureKind::Deadline
                },
                Delivery::MayHaveApplied
            )
        );
        assert_eq!(failure.request_context.unwrap().operation, "SetParameter");
        assert_eq!(
            unpublished_failure(&mut c, &input(), deadline()).kind,
            failure.kind
        );
        finish_client(&mut c);
    }
}
