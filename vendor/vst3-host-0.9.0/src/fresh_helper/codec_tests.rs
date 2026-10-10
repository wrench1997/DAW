//! Plugin-free source fixtures. These tests must not be compiled or executed
//! before the source archive, independent review and explicit execution gate.
use super::super::{budget_test_hooks, pump, Cancellation, MAX_PAYLOAD};
use super::*;
use crate::audio::AudioBusConfig;
use crate::internal::native_edit_transport::tests::allocation_free;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn new_budget() -> Budget {
    Budget {
        deadline: Instant::now() + Duration::from_secs(180),
        cancel: Arc::new(Cancellation {
            cancelled: AtomicBool::new(false),
            wake: pump::new_wake().expect("test cancellation descriptor"),
            skip_wake: AtomicBool::new(false),
        }),
        live: 0,
        peak: 0,
        diagnostic_copy: false,
    }
}

// Most fixtures inspect ordinary HostResponse values; dedicated failure-marker
// tests below inspect the richer codec result without this convenience adapter.
fn decode(bytes: &[u8], shape: &ReplyShape, budget: &mut Budget) -> Result<HostResponse> {
    match super::decode(bytes, shape, budget)? {
        DecodedReply::Response(response) => Ok(response),
        DecodedReply::KnownFailure {
            reported: Some(response),
            ..
        } => Ok(response),
        DecodedReply::KnownFailure {
            diagnostic_unavailable: Some(error),
            ..
        } => Err(error),
        _ => panic!("known failure without diagnostic status"),
    }
}

fn error(bytes: &[u8], shape: &ReplyShape, expected: ErrorKind) {
    let mut budget = new_budget();
    let baseline = 17;
    budget.reserve(baseline).unwrap();
    assert_eq!(decode(bytes, shape, &mut budget).unwrap_err(), expected);
    assert_eq!(
        budget.live, baseline,
        "all failed codec allocations are dropped"
    );
}

fn load(path: String) -> HostCommand {
    HostCommand::LoadPlugin {
        path,
        sample_rate: 48000.0,
        block_size: 4096,
        tempo: 120.0,
        time_sig_numerator: 4,
        time_sig_denominator: 4,
        class_id: Some("0123456789ABCDEF".into()),
    }
}

fn info(compatibility: Vec<ClassCompatibility>) -> HostResponse {
    HostResponse::PluginInfo {
        vendor: "Vendor".into(),
        name: "Plugin".into(),
        version: "1.0".into(),
        category: "Fx".into(),
        uid: "id".into(),
        has_gui: false,
        audio_inputs: 8,
        audio_outputs: 8,
        output_channels: 32,
        has_midi_input: false,
        has_midi_output: true,
        compatibility,
    }
}

fn compat(old_count: usize) -> ClassCompatibility {
    ClassCompatibility {
        new_class_id: "N".repeat(MAX_ID),
        old_class_ids: vec!["O".repeat(MAX_ID); old_count],
    }
}

fn wire(response: &HostResponse) -> Vec<u8> {
    serde_json::to_vec(response).unwrap()
}

fn audio_wire(channels: Vec<Vec<f32>>) -> Vec<u8> {
    wire(&HostResponse::AudioOutput {
        outputs: channels,
        output_events: Vec::new(),
        output_events_lost: false,
        transport_applied: true,
    })
}

fn assert_command(command: &HostCommand, shape: &ReplyShape) {
    let mut budget = new_budget();
    let encoded = encode(command, shape, &mut budget).unwrap();
    assert_eq!(encoded.last(), Some(&b'\n'));
    assert_eq!(budget.live, encoded.capacity());
    let decoded: HostCommand = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(
        serde_json::to_vec(command).unwrap(),
        serde_json::to_vec(&decoded).unwrap()
    );
    let capacity = encoded.capacity();
    drop(encoded);
    budget.release(capacity);
    assert_eq!(budget.live, 0);
}

#[test]
fn every_supported_request_matches_the_existing_wire() {
    assert_command(&load("x/\"\\\n\t🦀.vst3".into()), &ReplyShape::PluginInfo);
    for context in [
        StateContext::Project,
        StateContext::Preset { path: None },
        StateContext::Preset {
            path: Some("a\\b.vstpreset".into()),
        },
    ] {
        assert_command(
            &HostCommand::LoadState {
                data: vec![0, 1, 2, 253, 254, 255],
                context,
            },
            &ReplyShape::Success,
        );
    }
    assert_command(&HostCommand::SaveState, &ReplyShape::State);
    assert_command(
        &HostCommand::GetParameter { id: u32::MAX },
        &ReplyShape::ParameterValue,
    );
    assert_command(&HostCommand::LatencySamples, &ReplyShape::LatencySamples);
    assert_command(&HostCommand::TailSamples, &ReplyShape::TailSamples);
    let transport = ProcessTransport {
        sample_position: -900,
        quarter_note_position: -0.0,
        tempo: 99.5,
        playing: true,
        time_sig_numerator: 7,
        time_sig_denominator: 8,
    };
    assert_command(
        &HostCommand::Process {
            inputs: vec![vec![0.0, -0.0, f32::INFINITY, f32::from_bits(0x7fc0_1234)]],
            frames: 4,
            transport: Some(transport),
        },
        &ReplyShape::Audio {
            channels: 2,
            frames: 4,
        },
    );
    assert_command(
        &HostCommand::Process {
            inputs: Vec::new(),
            frames: 0,
            transport: None,
        },
        &ReplyShape::Audio {
            channels: 0,
            frames: 0,
        },
    );
    assert_command(
        &HostCommand::ProcessBuses {
            inputs: vec![AudioBusBuffer {
                active: false,
                channels: vec![vec![1.0; 3]],
            }],
            outputs: vec![
                AudioBusConfig {
                    active: true,
                    channel_count: 2,
                },
                AudioBusConfig {
                    active: false,
                    channel_count: 1,
                },
            ],
            frames: 3,
            transport: Some(transport),
        },
        &ReplyShape::Buses {
            buses: vec![(true, 2), (false, 1)],
            frames: 3,
        },
    );
}

#[test]
fn every_supported_response_matches_the_existing_canonical_wire() {
    let fixtures = [
        (
            HostResponse::Success {
                message: "ok".into(),
            },
            ReplyShape::Success,
        ),
        (
            HostResponse::Error {
                message: "failed".into(),
            },
            ReplyShape::State,
        ),
        (
            HostResponse::Crashed {
                message: "crashed".into(),
            },
            ReplyShape::Audio {
                channels: 0,
                frames: 0,
            },
        ),
        (
            HostResponse::ParameterValue { value: -0.0 },
            ReplyShape::ParameterValue,
        ),
        (
            HostResponse::LatencySamples { samples: u32::MAX },
            ReplyShape::LatencySamples,
        ),
        (
            HostResponse::TailSamples { samples: 512 },
            ReplyShape::TailSamples,
        ),
        (
            HostResponse::State {
                data: vec![0, 1, 2, 3, 128, 255],
            },
            ReplyShape::State,
        ),
        (info(vec![compat(2)]), ReplyShape::PluginInfo),
        (
            HostResponse::AudioOutput {
                outputs: vec![vec![1.0, -0.0]],
                output_events: Vec::new(),
                output_events_lost: true,
                transport_applied: false,
            },
            ReplyShape::Audio {
                channels: 1,
                frames: 2,
            },
        ),
        (
            HostResponse::BusAudioOutput {
                outputs: vec![AudioBusBuffer {
                    active: false,
                    channels: vec![vec![0.5]],
                }],
                output_events: Vec::new(),
                output_events_lost: false,
                transport_applied: true,
            },
            ReplyShape::Buses {
                buses: vec![(false, 1)],
                frames: 1,
            },
        ),
    ];
    for (response, shape) in fixtures {
        let original = wire(&response);
        let decoded = decode(&original, &shape, &mut new_budget()).unwrap();
        assert_eq!(original, wire(&decoded));
    }
}

#[test]
fn unsupported_requests_and_incompatible_shapes_allocate_nothing() {
    for command in [
        HostCommand::Shutdown,
        HostCommand::UnloadPlugin,
        HostCommand::StartProcessing,
        HostCommand::GetAllParameters,
    ] {
        let mut budget = new_budget();
        assert_eq!(
            allocation_free(|| encode(&command, &ReplyShape::State, &mut budget)).unwrap_err(),
            ErrorKind::UnsupportedRequest
        );
        assert_eq!((budget.live, budget.peak), (0, 0));
    }
    for shape in [
        ReplyShape::Success,
        ReplyShape::ParameterValue,
        ReplyShape::Audio {
            channels: 0,
            frames: 0,
        },
    ] {
        let mut budget = new_budget();
        assert_eq!(
            allocation_free(|| encode(&HostCommand::SaveState, &shape, &mut budget)).unwrap_err(),
            ErrorKind::Shape
        );
        assert_eq!((budget.live, budget.peak), (0, 0));
    }
    let command = HostCommand::ProcessBuses {
        inputs: Vec::new(),
        outputs: vec![AudioBusConfig {
            active: true,
            channel_count: 2,
        }],
        frames: 2,
        transport: None,
    };
    for buses in [vec![(false, 2)], vec![(true, 1)], Vec::new()] {
        assert_eq!(
            encode(
                &command,
                &ReplyShape::Buses { buses, frames: 2 },
                &mut new_budget()
            )
            .unwrap_err(),
            ErrorKind::Shape
        );
    }
}

#[test]
fn request_shape_and_fixed_count_boundaries() {
    let mut command = HostCommand::Process {
        inputs: vec![vec![0.0; MAX_FRAMES]; MAX_CHANNELS],
        frames: MAX_FRAMES as u32,
        transport: None,
    };
    let shape = ReplyShape::Audio {
        channels: MAX_CHANNELS,
        frames: MAX_FRAMES,
    };
    assert_command(&command, &shape);
    if let HostCommand::Process { inputs, .. } = &mut command {
        inputs.push(vec![0.0; MAX_FRAMES]);
    }
    assert_eq!(
        encode(&command, &shape, &mut new_budget()).unwrap_err(),
        ErrorKind::Limit
    );
    for frames in [MAX_FRAMES - 1, MAX_FRAMES + 1] {
        let command = HostCommand::Process {
            inputs: vec![vec![0.0; frames]],
            frames: MAX_FRAMES as u32,
            transport: None,
        };
        assert!(matches!(
            encode(&command, &shape, &mut new_budget()),
            Err(ErrorKind::Shape | ErrorKind::Limit)
        ));
    }
    assert_eq!(
        encode(
            &HostCommand::Process {
                inputs: Vec::new(),
                frames: 1,
                transport: None
            },
            &ReplyShape::Audio {
                channels: 0,
                frames: 0
            },
            &mut new_budget()
        )
        .unwrap_err(),
        ErrorKind::Shape
    );
    for shape in [
        ReplyShape::Audio {
            channels: MAX_CHANNELS + 1,
            frames: 0,
        },
        ReplyShape::Audio {
            channels: 0,
            frames: MAX_FRAMES + 1,
        },
        ReplyShape::Buses {
            buses: vec![(true, 0); MAX_BUSES + 1],
            frames: 0,
        },
    ] {
        assert_eq!(check_shape(&shape).unwrap_err(), ErrorKind::Limit);
    }
    let command = HostCommand::ProcessBuses {
        inputs: vec![
            AudioBusBuffer {
                active: true,
                channels: vec![Vec::new(); 4]
            };
            MAX_BUSES
        ],
        outputs: vec![
            AudioBusConfig {
                active: false,
                channel_count: 4
            };
            MAX_BUSES
        ],
        frames: 0,
        transport: None,
    };
    assert_command(
        &command,
        &ReplyShape::Buses {
            buses: vec![(false, 4); MAX_BUSES],
            frames: 0,
        },
    );
    let invalid = HostCommand::ProcessBuses {
        inputs: vec![
            AudioBusBuffer {
                active: false,
                channels: Vec::new()
            };
            MAX_BUSES + 1
        ],
        outputs: Vec::new(),
        frames: 0,
        transport: None,
    };
    assert_eq!(
        encode(
            &invalid,
            &ReplyShape::Buses {
                buses: Vec::new(),
                frames: 0
            },
            &mut new_budget()
        )
        .unwrap_err(),
        ErrorKind::Limit
    );
    let invalid = HostCommand::ProcessBuses {
        inputs: vec![
            AudioBusBuffer {
                active: true,
                channels: vec![Vec::new(); 17]
            };
            2
        ],
        outputs: Vec::new(),
        frames: 0,
        transport: None,
    };
    assert_eq!(
        encode(
            &invalid,
            &ReplyShape::Buses {
                buses: Vec::new(),
                frames: 0
            },
            &mut new_budget()
        )
        .unwrap_err(),
        ErrorKind::Limit
    );
}

#[test]
fn transport_rejects_invalid_scalars_before_allocation() {
    let valid = ProcessTransport {
        sample_position: 0,
        quarter_note_position: 0.0,
        tempo: 120.0,
        playing: false,
        time_sig_numerator: 4,
        time_sig_denominator: 4,
    };
    for transport in [
        ProcessTransport {
            tempo: f64::NAN,
            ..valid
        },
        ProcessTransport {
            quarter_note_position: f64::INFINITY,
            ..valid
        },
        ProcessTransport {
            tempo: 0.0,
            ..valid
        },
        ProcessTransport {
            time_sig_denominator: 3,
            ..valid
        },
    ] {
        let mut budget = new_budget();
        assert_eq!(
            encode(
                &HostCommand::Process {
                    inputs: Vec::new(),
                    frames: 0,
                    transport: Some(transport)
                },
                &ReplyShape::Audio {
                    channels: 0,
                    frames: 0
                },
                &mut budget
            )
            .unwrap_err(),
            ErrorKind::InvalidRequest
        );
        assert_eq!(budget.peak, 0);
    }
}

#[test]
fn all_audio_ieee_bits_including_nan_payloads_survive() {
    let bits = [
        0,
        0x8000_0000,
        1,
        0x007f_ffff,
        0x0080_0000,
        0x3f80_0000,
        0x7f7f_ffff,
        0x7f80_0000,
        0xff80_0000,
        0x7fc0_1234,
        0x7f80_0001,
        0xffc0_5678,
    ];
    let samples: Vec<f32> = bits.iter().copied().map(f32::from_bits).collect();
    let encoded = audio_wire(vec![samples.clone()]);
    let decoded = decode(
        &encoded,
        &ReplyShape::Audio {
            channels: 1,
            frames: bits.len(),
        },
        &mut new_budget(),
    )
    .unwrap();
    let HostResponse::AudioOutput { outputs, .. } = decoded else {
        panic!("audio expected")
    };
    assert_eq!(
        outputs[0]
            .iter()
            .map(|sample| sample.to_bits())
            .collect::<Vec<_>>(),
        bits
    );
    assert_command(
        &HostCommand::Process {
            inputs: vec![samples],
            frames: bits.len() as u32,
            transport: None,
        },
        &ReplyShape::Audio {
            channels: 1,
            frames: bits.len(),
        },
    );
}

#[test]
fn parameter_wire_preserves_nonfinite_class_and_finite_bits() {
    for value in [
        0.0,
        -0.0,
        f64::MIN_POSITIVE,
        f64::from_bits(1),
        f64::MAX,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NAN,
    ] {
        let decoded = decode(
            &wire(&HostResponse::ParameterValue { value }),
            &ReplyShape::ParameterValue,
            &mut new_budget(),
        )
        .unwrap();
        let HostResponse::ParameterValue { value: actual } = decoded else {
            panic!("parameter expected")
        };
        if value.is_nan() {
            assert!(actual.is_nan());
        } else {
            assert_eq!(actual.to_bits(), value.to_bits());
        }
    }
    // The strict token length cap runs before spelling validation.
    error(
        br#"{"ParameterValue":{"value":"Infinity"}}"#,
        &ReplyShape::ParameterValue,
        ErrorKind::Limit,
    );
    for value in [
        "null", "\"1.0\"", "1e999", "01", "+1", "1.", ".5", "--1", "1e", "1e+",
    ] {
        error(
            format!("{{\"ParameterValue\":{{\"value\":{value}}}}}").as_bytes(),
            &ReplyShape::ParameterValue,
            ErrorKind::MalformedReply,
        );
    }
}

#[test]
fn malformed_base64_padding_and_partial_sample_are_never_published() {
    for value in [
        "A", "AAA", "====", "A===", "AA=A", "AA==AAAA", "AAA=AAAA", "AA?=", "AB==", "AAB=",
        "////=", "\"", "éAAA",
    ] {
        let response = format!(
            "{{\"State\":{{\"data\":{}}}}}",
            serde_json::to_string(value).unwrap()
        );
        error(
            response.as_bytes(),
            &ReplyShape::State,
            ErrorKind::MalformedReply,
        );
    }
    for value in ["AA==", "AAA=", "AAAA"] {
        error(
            format!("{{\"AudioOutput\":{{\"outputs\":[\"{value}\"],\"output_events\":[]}}}}")
                .as_bytes(),
            &ReplyShape::Audio {
                channels: 1,
                frames: 1,
            },
            ErrorKind::MalformedReply,
        );
    }
    let escaped = br#"{"State":{"data":"\u0041Q\u003d\u003d"}}"#;
    let HostResponse::State { data } =
        decode(escaped, &ReplyShape::State, &mut new_budget()).unwrap()
    else {
        panic!("state expected")
    };
    assert_eq!(data, [1]);
}

#[test]
fn exact_audio_shapes_reject_short_extra_channels_frames_buses_and_activation() {
    let shape = ReplyShape::Audio {
        channels: 2,
        frames: 2,
    };
    for channels in [
        vec![vec![0.0; 2]],
        vec![vec![0.0; 2]; 3],
        vec![vec![0.0; 1]; 2],
        vec![vec![0.0; 3]; 2],
    ] {
        let mut budget = new_budget();
        assert_eq!(
            decode(&audio_wire(channels), &shape, &mut budget).unwrap_err(),
            ErrorKind::Shape
        );
        assert_eq!(
            budget.peak, 0,
            "shape preflight precedes all final payload allocation"
        );
    }
    let bus_shape = ReplyShape::Buses {
        buses: vec![(false, 2)],
        frames: 2,
    };
    for outputs in [
        Vec::new(),
        vec![
            AudioBusBuffer {
                active: false,
                channels: vec![vec![0.0; 2]; 2]
            };
            2
        ],
        vec![AudioBusBuffer {
            active: true,
            channels: vec![vec![0.0; 2]; 2],
        }],
        vec![AudioBusBuffer {
            active: false,
            channels: vec![vec![0.0; 2]],
        }],
    ] {
        let response = HostResponse::BusAudioOutput {
            outputs,
            output_events: Vec::new(),
            output_events_lost: false,
            transport_applied: false,
        };
        error(&wire(&response), &bus_shape, ErrorKind::Shape);
    }
    let exact = audio_wire(vec![vec![0.0; MAX_FRAMES]; MAX_CHANNELS]);
    assert!(decode(
        &exact,
        &ReplyShape::Audio {
            channels: MAX_CHANNELS,
            frames: MAX_FRAMES
        },
        &mut new_budget()
    )
    .is_ok());
    let too_many_frames = audio_wire(vec![vec![0.0; MAX_FRAMES + 1]]);
    error(
        &too_many_frames,
        &ReplyShape::Audio {
            channels: 1,
            frames: MAX_FRAMES,
        },
        ErrorKind::Limit,
    );
    let exact_buses = HostResponse::BusAudioOutput {
        outputs: vec![
            AudioBusBuffer {
                active: false,
                channels: vec![Vec::new(); 4]
            };
            MAX_BUSES
        ],
        output_events: Vec::new(),
        output_events_lost: false,
        transport_applied: false,
    };
    assert!(decode(
        &wire(&exact_buses),
        &ReplyShape::Buses {
            buses: vec![(false, 4); MAX_BUSES],
            frames: 0
        },
        &mut new_budget()
    )
    .is_ok());
}

#[test]
fn duplicate_missing_unknown_and_trailing_fields_fail_closed() {
    for response in [
        br#"{"State":{}}"#.as_slice(),
        br#"{"State":{"data":"","data":""}}"#,
        br#"{"State":{"data":"","unknown":{"huge":"ignored?"}}}"#,
        br#"{"State":{"data":""},"State":{"data":""}}"#,
        br#"{"State":{"data":"",}}"#,
        br#"{"State":{"data":""},}"#,
        br#"{"State":{"data":""}}{}"#,
        br#"{"State":{"data":false}}"#,
        br#"{"State":{"data":""}"#,
        b"{}",
    ] {
        error(response, &ReplyShape::State, ErrorKind::MalformedReply);
    }
    for response in [
        br#"{"AudioOutput":{"outputs":[]}}"#.as_slice(),
        br#"{"AudioOutput":{"output_events":[]}}"#,
        br#"{"AudioOutput":{"outputs":[],"output_events":[],"output_events_lost":false,"output_events_lost":true}}"#,
        br#"{"AudioOutput":{"outputs":[],"output_events":[],"transport_applied":null}}"#,
        br#"{"AudioOutput":{"outputs":[],"output_events":[],"unknown":[]}}"#,
    ] { error(response, &ReplyShape::Audio { channels: 0, frames: 0 }, ErrorKind::MalformedReply); }
    error(
        br#"{"TailSamples":{"samples":1}}"#,
        &ReplyShape::LatencySamples,
        ErrorKind::WrongReply,
    );
    error(
        br#"{"Parameters":{"params":[]}}"#,
        &ReplyShape::State,
        ErrorKind::UnsupportedReply,
    );
    error(
        br#"{"LatencySamples":{"samples":4294967296}}"#,
        &ReplyShape::LatencySamples,
        ErrorKind::MalformedReply,
    );
    error(
        br#"{"LatencySamples":{"samples":-1}}"#,
        &ReplyShape::LatencySamples,
        ErrorKind::MalformedReply,
    );
}

#[test]
fn reject_events_at_first_element_and_legacy_state_at_array_open() {
    // The rest is deliberately not even valid JSON, so a parse-first/drop-events
    // implementation produces MalformedReply instead of the required boundary.
    for first in [
        "{\"data\":[[[[[[[[[[[[[[[[",
        "null",
        "\"unterminated",
        "false",
        "@",
    ] {
        let mut budget = new_budget();
        let response = format!("{{\"AudioOutput\":{{\"output_events\":[{first}");
        assert_eq!(
            decode(
                response.as_bytes(),
                &ReplyShape::Audio {
                    channels: 0,
                    frames: 0
                },
                &mut budget
            )
            .unwrap_err(),
            ErrorKind::UnsupportedReply
        );
        assert_eq!(budget.peak, 0);
    }
    for response in [
        br#"{"State":{"data":["#.as_slice(),
        br#"{"State":{"data":[[[[[[[[[["#,
        br#"{"State":{"data":[]}}"#,
    ] {
        error(response, &ReplyShape::State, ErrorKind::UnsupportedReply);
    }
    // Unknown fields are also rejected before parsing their nested payload.
    error(
        br#"{"State":{"unknown":[[[[[[["#,
        &ReplyShape::State,
        ErrorKind::MalformedReply,
    );
}

#[test]
fn empty_events_preserve_both_flags_and_explicit_legacy_defaults() {
    for lost in [false, true] {
        for applied in [false, true] {
            for buses in [false, true] {
                let variant = if buses {
                    "BusAudioOutput"
                } else {
                    "AudioOutput"
                };
                let shape = if buses {
                    ReplyShape::Buses {
                        buses: Vec::new(),
                        frames: 0,
                    }
                } else {
                    ReplyShape::Audio {
                        channels: 0,
                        frames: 0,
                    }
                };
                let response = format!("{{\"{variant}\":{{\"output_events_lost\":{lost},\"transport_applied\":{applied},\"output_events\":[],\"outputs\":[]}}}}");
                let decoded = decode(response.as_bytes(), &shape, &mut new_budget()).unwrap();
                match decoded {
                    HostResponse::AudioOutput {
                        output_events,
                        output_events_lost,
                        transport_applied,
                        ..
                    }
                    | HostResponse::BusAudioOutput {
                        output_events,
                        output_events_lost,
                        transport_applied,
                        ..
                    } => {
                        assert!(output_events.is_empty());
                        assert_eq!(output_events_lost, lost);
                        assert_eq!(transport_applied, applied);
                    }
                    _ => panic!("audio expected"),
                }
            }
        }
    }
    let response = decode(
        br#"{"AudioOutput":{"output_events":[],"outputs":[]}}"#,
        &ReplyShape::Audio {
            channels: 0,
            frames: 0,
        },
        &mut new_budget(),
    )
    .unwrap();
    assert!(matches!(
        response,
        HostResponse::AudioOutput {
            output_events_lost: false,
            transport_applied: false,
            ..
        }
    ));
}

#[test]
fn text_and_class_id_limits_are_decoded_byte_limits_with_unicode_validation() {
    for message in ["x".repeat(MAX_TEXT), "é".repeat(MAX_TEXT / 2)] {
        let encoded = wire(&HostResponse::Success {
            message: message.clone(),
        });
        let HostResponse::Success { message: actual } =
            decode(&encoded, &ReplyShape::Success, &mut new_budget()).unwrap()
        else {
            panic!("success expected")
        };
        assert_eq!(actual, message);
    }
    error(
        &wire(&HostResponse::Success {
            message: "x".repeat(MAX_TEXT + 1),
        }),
        &ReplyShape::Success,
        ErrorKind::Limit,
    );
    let escaped = format!(
        "{{\"Success\":{{\"message\":\"{}\"}}}}",
        "\\u0061".repeat(MAX_TEXT)
    );
    assert!(decode(escaped.as_bytes(), &ReplyShape::Success, &mut new_budget()).is_ok());
    let escaped = format!(
        "{{\"Success\":{{\"message\":\"{}\"}}}}",
        "\\u0061".repeat(MAX_TEXT + 1)
    );
    error(escaped.as_bytes(), &ReplyShape::Success, ErrorKind::Limit);
    let valid = br#"{"Succ\u0065ss":{"message":"\uD83E\uDD80\n\t\b\f\r\/\\\""}}"#;
    let HostResponse::Success { message } =
        decode(valid, &ReplyShape::Success, &mut new_budget()).unwrap()
    else {
        panic!("success expected")
    };
    assert_eq!(message, "🦀\n\t\u{8}\u{c}\r/\\\"");
    for value in [
        "\\uD800",
        "\\uDC00",
        "\\uD800\\u0041",
        "\\uZZZZ",
        "\\x00",
        "\n",
    ] {
        error(
            format!("{{\"Success\":{{\"message\":\"{value}\"}}}}").as_bytes(),
            &ReplyShape::Success,
            ErrorKind::MalformedReply,
        );
    }
    error(
        b"{\"Success\":{\"message\":\"\xff\"}}",
        &ReplyShape::Success,
        ErrorKind::MalformedReply,
    );
    assert_command(&load("x".repeat(MAX_TEXT)), &ReplyShape::PluginInfo);
    assert_eq!(
        encode(
            &load("x".repeat(MAX_TEXT + 1)),
            &ReplyShape::PluginInfo,
            &mut new_budget()
        )
        .unwrap_err(),
        ErrorKind::Limit
    );
    let mut command = load("x".into());
    if let HostCommand::LoadPlugin { class_id, .. } = &mut command {
        *class_id = Some("x".repeat(MAX_ID));
    }
    assert_command(&command, &ReplyShape::PluginInfo);
    if let HostCommand::LoadPlugin { class_id, .. } = &mut command {
        *class_id = Some("x".repeat(MAX_ID + 1));
    }
    assert_eq!(
        encode(&command, &ReplyShape::PluginInfo, &mut new_budget()).unwrap_err(),
        ErrorKind::Limit
    );
}

#[test]
fn plugin_counts_are_rejected_rather_than_clamped() {
    let original = String::from_utf8(wire(&info(Vec::new()))).unwrap();
    for (from, to) in [
        ("\"audio_inputs\":8", "\"audio_inputs\":9"),
        ("\"audio_outputs\":8", "\"audio_outputs\":-1"),
        ("\"output_channels\":32", "\"output_channels\":33"),
    ] {
        error(
            original.replace(from, to).as_bytes(),
            &ReplyShape::PluginInfo,
            ErrorKind::Limit,
        );
    }
    error(
        original.replace("\"has_gui\":false,", "").as_bytes(),
        &ReplyShape::PluginInfo,
        ErrorKind::MalformedReply,
    );
    error(
        original
            .replace("\"has_gui\":false", "\"has_gui\":false,\"has_gui\":true")
            .as_bytes(),
        &ReplyShape::PluginInfo,
        ErrorKind::MalformedReply,
    );
}

#[test]
fn compatibility_entry_per_entry_aggregate_and_id_boundaries() {
    for entries in [
        vec![compat(0); MAX_COMPAT],
        vec![compat(MAX_COMPAT)],
        vec![compat(1); MAX_COMPAT],
    ] {
        assert!(decode(
            &wire(&info(entries)),
            &ReplyShape::PluginInfo,
            &mut new_budget()
        )
        .is_ok());
    }
    for entries in [
        vec![compat(0); MAX_COMPAT + 1],
        vec![compat(MAX_COMPAT + 1)],
        vec![compat(128), compat(129)],
    ] {
        let mut budget = new_budget();
        assert_eq!(
            decode(&wire(&info(entries)), &ReplyShape::PluginInfo, &mut budget).unwrap_err(),
            ErrorKind::Limit
        );
        assert_eq!(budget.peak, 0);
    }
    for entry in [
        ClassCompatibility {
            new_class_id: "x".repeat(MAX_ID + 1),
            old_class_ids: Vec::new(),
        },
        ClassCompatibility {
            new_class_id: "x".into(),
            old_class_ids: vec!["y".repeat(MAX_ID + 1)],
        },
    ] {
        error(
            &wire(&info(vec![entry])),
            &ReplyShape::PluginInfo,
            ErrorKind::Limit,
        );
    }
    let original = String::from_utf8(wire(&info(vec![compat(0)]))).unwrap();
    for changed in [
        original.replace(
            "\"old_class_ids\":[]",
            "\"old_class_ids\":[],\"old_class_ids\":[]",
        ),
        original.replace(",\"old_class_ids\":[]", ""),
        original.replace("\"old_class_ids\":[]", "\"unknown\":[]"),
    ] {
        error(
            changed.as_bytes(),
            &ReplyShape::PluginInfo,
            ErrorKind::MalformedReply,
        );
    }
}

#[test]
fn scalar_number_and_depth_limits_are_checked_without_generic_recursion() {
    let at_limit = format!(
        "{{\"ParameterValue\":{{\"value\":0.{}1}}}}",
        "0".repeat(MAX_NUMBER - 3)
    );
    assert!(decode(
        at_limit.as_bytes(),
        &ReplyShape::ParameterValue,
        &mut new_budget()
    )
    .is_ok());
    let over_limit = format!(
        "{{\"ParameterValue\":{{\"value\":0.{}1}}}}",
        "0".repeat(MAX_NUMBER - 2)
    );
    error(
        over_limit.as_bytes(),
        &ReplyShape::ParameterValue,
        ErrorKind::Limit,
    );
    let bytes = [b'['; MAX_DEPTH + 1];
    let mut budget = new_budget();
    let mut parser = Parser {
        bytes: &bytes,
        position: 0,
        checked: 0,
        depth: 0,
        materialize: false,
        failure_message: None,
        budget: &mut budget,
    };
    for _ in 0..MAX_DEPTH {
        parser.open(b'[').unwrap();
    }
    assert_eq!(parser.open(b'[').unwrap_err(), ErrorKind::Limit);
    assert_eq!(parser.budget.peak, 0);
}

#[test]
fn request_and_decoded_payload_account_actual_owned_capacities() {
    let command = HostCommand::LoadState {
        data: vec![7; 16384],
        context: StateContext::Project,
    };
    let mut budget = new_budget();
    budget.reserve(99).unwrap();
    let request = encode(&command, &ReplyShape::Success, &mut budget).unwrap();
    assert_eq!(budget.live, 99 + request.capacity());
    assert_eq!(budget.peak, budget.live);
    let request_capacity = request.capacity();
    drop(request);
    budget.release(request_capacity);
    assert_eq!(budget.live, 99);
    let line = wire(&HostResponse::AudioOutput {
        outputs: vec![vec![1.0; 2]; 2],
        output_events: Vec::new(),
        output_events_lost: false,
        transport_applied: true,
    });
    budget.reserve(line.capacity()).unwrap();
    let response = decode(
        &line,
        &ReplyShape::Audio {
            channels: 2,
            frames: 2,
        },
        &mut budget,
    )
    .unwrap();
    let HostResponse::AudioOutput {
        outputs,
        output_events,
        ..
    } = response
    else {
        panic!("audio expected")
    };
    let charge = outputs.capacity() * std::mem::size_of::<Vec<f32>>()
        + outputs
            .iter()
            .map(|c| c.capacity() * std::mem::size_of::<f32>())
            .sum::<usize>()
        + output_events.capacity() * std::mem::size_of::<crate::midi::PluginEvent>();
    assert_eq!(budget.live, 99 + line.capacity() + charge);
    drop(outputs);
    drop(output_events);
    budget.release(charge);
    assert_eq!(budget.live, 99 + line.capacity());
    let response = decode(
        &wire(&info(vec![compat(2)])),
        &ReplyShape::PluginInfo,
        &mut budget,
    )
    .unwrap();
    let HostResponse::PluginInfo {
        vendor,
        name,
        version,
        category,
        uid,
        compatibility,
        ..
    } = response
    else {
        panic!("info expected")
    };
    let charge = vendor.capacity()
        + name.capacity()
        + version.capacity()
        + category.capacity()
        + uid.capacity()
        + compatibility.capacity() * std::mem::size_of::<ClassCompatibility>()
        + compatibility
            .iter()
            .map(|entry| {
                entry.new_class_id.capacity()
                    + entry.old_class_ids.capacity() * std::mem::size_of::<String>()
                    + entry
                        .old_class_ids
                        .iter()
                        .map(String::capacity)
                        .sum::<usize>()
            })
            .sum::<usize>();
    assert_eq!(budget.live, 99 + line.capacity() + charge);
}

#[test]
fn payload_reservation_limit_rejects_and_failure_rolls_back_live_charge() {
    let mut budget = new_budget();
    budget.reserve(MAX_PAYLOAD).unwrap();
    assert_eq!(
        encode(&HostCommand::SaveState, &ReplyShape::State, &mut budget).unwrap_err(),
        ErrorKind::Limit
    );
    assert_eq!(budget.live, MAX_PAYLOAD);
    assert_eq!(
        decode(
            br#"{"Success":{"message":"x"}}"#,
            &ReplyShape::Success,
            &mut budget
        )
        .unwrap_err(),
        ErrorKind::Limit
    );
    assert_eq!(budget.live, MAX_PAYLOAD);
    // A malformed response never allocates even when valid earlier fields have
    // large final payloads, because the complete preflight must pass first.
    let bad = br#"{"Success":{"message":"looks valid","unknown":0}}"#;
    assert_eq!(
        decode(bad, &ReplyShape::Success, &mut budget).unwrap_err(),
        ErrorKind::MalformedReply
    );
    assert_eq!(budget.live, MAX_PAYLOAD);
}

#[test]
fn deadline_and_sticky_cancel_are_checked_before_codec_work() {
    let mut expired = new_budget();
    expired.deadline = Instant::now();
    assert_eq!(
        encode(&HostCommand::SaveState, &ReplyShape::State, &mut expired).unwrap_err(),
        ErrorKind::Deadline
    );
    assert_eq!(
        decode(
            br#"{"State":{"data":""}}"#,
            &ReplyShape::State,
            &mut expired
        )
        .unwrap_err(),
        ErrorKind::Deadline
    );
    assert_eq!(expired.peak, 0);
    let mut cancelled = new_budget();
    cancelled.cancel.cancelled.store(true, Ordering::Release);
    assert_eq!(
        encode(&HostCommand::SaveState, &ReplyShape::State, &mut cancelled).unwrap_err(),
        ErrorKind::Cancelled
    );
    assert_eq!(
        decode(
            br#"{"State":{"data":""}}"#,
            &ReplyShape::State,
            &mut cancelled
        )
        .unwrap_err(),
        ErrorKind::Cancelled
    );
    assert_eq!(cancelled.peak, 0);
}

#[test]
fn deterministic_cancellation_interrupts_counting_emission_preflight_and_materialization() {
    let command = HostCommand::LoadState {
        data: vec![3; 32768],
        context: StateContext::Project,
    };
    let checkpoints = {
        let _hook = budget_test_hooks::cancel_at_check(usize::MAX);
        encode(&command, &ReplyShape::Success, &mut new_budget()).unwrap();
        budget_test_hooks::checks()
    };
    for at in [2, checkpoints / 2, checkpoints - 1, checkpoints] {
        let _hook = budget_test_hooks::cancel_at_check(at);
        let mut budget = new_budget();
        assert_eq!(
            encode(&command, &ReplyShape::Success, &mut budget).unwrap_err(),
            ErrorKind::Cancelled
        );
        assert_eq!(budget.live, 0);
    }
    for line in [
        wire(&HostResponse::State {
            data: vec![4; 32768],
        }),
        wire(&HostResponse::Success {
            message: "\n🦀".repeat(4096),
        }),
    ] {
        let shape = if line.starts_with(b"{\"State\"") {
            ReplyShape::State
        } else {
            ReplyShape::Success
        };
        let checkpoints = {
            let _hook = budget_test_hooks::cancel_at_check(usize::MAX);
            decode(&line, &shape, &mut new_budget()).unwrap();
            budget_test_hooks::checks()
        };
        for at in [2, checkpoints / 2, checkpoints - 1, checkpoints] {
            let _hook = budget_test_hooks::cancel_at_check(at);
            let mut budget = new_budget();
            assert_eq!(
                decode(&line, &shape, &mut budget).unwrap_err(),
                ErrorKind::Cancelled
            );
            assert_eq!(budget.live, 0);
        }
    }
}

#[test]
fn bounded_writer_line_boundary_and_size_arithmetic_overflow() {
    let mut budget = new_budget();
    let mut writer = Writer {
        bytes: None,
        len: MAX_LINE - 1,
        checked: MAX_LINE - 1,
        budget: &mut budget,
    };
    writer.put(b"\n").unwrap();
    assert_eq!(writer.len, MAX_LINE);
    assert_eq!(writer.put(b"x").unwrap_err(), ErrorKind::Limit);
    assert_eq!(base64_size(usize::MAX).unwrap_err(), ErrorKind::Limit);
    assert_eq!(writer.budget.peak, 0);
}

// Kept separate so the independently approved invocation can schedule these
// larger plugin-free boundary fixtures explicitly and record their memory cost.
#[test]
fn large_state_exact_limit_and_plus_one_have_no_eager_base64_scratch() {
    let mut command = HostCommand::LoadState {
        data: vec![0; MAX_STATE],
        context: StateContext::Project,
    };
    let mut budget = new_budget();
    let encoded = encode(&command, &ReplyShape::Success, &mut budget).unwrap();
    assert!(encoded.len() < MAX_LINE);
    assert_eq!(budget.live, encoded.capacity());
    assert_eq!(
        budget.peak,
        encoded.capacity(),
        "one accounted request buffer only"
    );
    let capacity = encoded.capacity();
    drop(encoded);
    budget.release(capacity);
    if let HostCommand::LoadState { data, .. } = &mut command {
        data.push(0);
    }
    assert_eq!(
        encode(&command, &ReplyShape::Success, &mut budget).unwrap_err(),
        ErrorKind::Limit
    );
    assert_eq!(budget.live, 0);
    drop(command);
    // The maximum has two final bytes. Construct the known all-zero canonical
    // fixture directly, so this test does not need the legacy state encoder.
    let mut line = Vec::with_capacity(base64_size(MAX_STATE).unwrap() + 64);
    line.extend_from_slice(b"{\"State\":{\"data\":\"");
    let start = line.len();
    line.resize(start + base64_size(MAX_STATE).unwrap(), b'A');
    let last = line.len() - 1;
    line[last] = b'=';
    line.extend_from_slice(b"\"}}");
    let mut budget = new_budget();
    budget.reserve(line.capacity()).unwrap();
    let decoded = decode(&line, &ReplyShape::State, &mut budget).unwrap();
    let HostResponse::State { data } = decoded else {
        panic!("state expected")
    };
    assert_eq!(data.len(), MAX_STATE);
    assert!(data.iter().all(|&byte| byte == 0));
    assert_eq!(budget.live, line.capacity() + data.capacity());
    assert_eq!(budget.peak, budget.live);
    drop(data);
    // Removing final padding adds exactly one decoded byte, without increasing
    // encoded length. The decoded length check must reject before allocation.
    line[last] = b'A';
    let mut fresh = new_budget();
    assert_eq!(
        decode(&line, &ReplyShape::State, &mut fresh).unwrap_err(),
        ErrorKind::Limit
    );
    assert_eq!(fresh.peak, 0);
}

#[test]
fn large_response_line_exact_limit_and_plus_one() {
    let suffix = br#"{"LatencySamples":{"samples":0}}"#;
    let mut line = vec![b' '; MAX_LINE - suffix.len()];
    line.extend_from_slice(suffix);
    assert!(decode(&line, &ReplyShape::LatencySamples, &mut new_budget()).is_ok());
    line.push(b' ');
    let mut budget = new_budget();
    assert_eq!(
        decode(&line, &ReplyShape::LatencySamples, &mut budget).unwrap_err(),
        ErrorKind::Limit
    );
    assert_eq!(budget.peak, 0);
}

#[test]
fn request_finite_extremes_use_bounded_lossless_scalar_spelling() {
    for value in [
        f64::from_bits(1),
        f64::MIN_POSITIVE,
        f64::MAX,
        -f64::MAX,
        -0.0,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NAN,
    ] {
        let mut command = load("x".into());
        if let HostCommand::LoadPlugin {
            sample_rate, tempo, ..
        } = &mut command
        {
            *sample_rate = value;
            *tempo = value;
        }
        assert_command(&command, &ReplyShape::PluginInfo);
        let encoded = encode(&command, &ReplyShape::PluginInfo, &mut new_budget()).unwrap();
        let json = std::str::from_utf8(&encoded).unwrap();
        let scalar = json
            .split("\"sample_rate\":")
            .nth(1)
            .unwrap()
            .split(',')
            .next()
            .unwrap();
        assert!(scalar.len() <= MAX_NUMBER);
    }
}

#[test]
fn complete_failure_preflight_latches_category_before_diagnostic_copy() {
    for crashed in [false, true] {
        let source = if crashed {
            HostResponse::Crashed {
                message: "detail\n🦀".repeat(4096),
            }
        } else {
            HostResponse::Error {
                message: "detail\n🦀".repeat(4096),
            }
        };
        let bytes = wire(&source);
        let mut budget = new_budget();
        let marker = preflight(&bytes, &ReplyShape::State, &mut budget)
            .unwrap()
            .expect("known failure marker");
        assert_eq!(budget.live, 0);
        // Both stop conditions arrive strictly after complete checked validation.
        budget.cancel.cancelled.store(true, Ordering::Release);
        budget.deadline = Instant::now();
        let reply = marker.materialize(&mut budget);
        let DecodedReply::KnownFailure {
            crashed: actual_crashed,
            reported: Some(reported),
            diagnostic_unavailable: None,
        } = reply
        else {
            panic!("failure category and diagnostic must survive")
        };
        assert_eq!(actual_crashed, crashed);
        assert_eq!(wire(&reported), bytes);
        assert!(
            !budget.diagnostic_copy,
            "the normal exchange budget stays strict"
        );
        assert_eq!(budget.check().unwrap_err(), ErrorKind::Cancelled);
    }
}

#[test]
fn cancellation_at_last_preflight_check_prevents_known_failure_marker() {
    let bytes = wire(&HostResponse::Error {
        message: "x".repeat(16384),
    });
    let checkpoints = {
        let _hook = budget_test_hooks::cancel_at_check(usize::MAX);
        assert!(preflight(&bytes, &ReplyShape::State, &mut new_budget())
            .unwrap()
            .is_some());
        budget_test_hooks::checks()
    };
    let _hook = budget_test_hooks::cancel_at_check(checkpoints);
    let mut budget = new_budget();
    assert!(matches!(
        super::decode(&bytes, &ReplyShape::State, &mut budget),
        Err(ErrorKind::Cancelled)
    ));
    assert_eq!(budget.live, 0);
}

#[test]
fn known_failure_survives_diagnostic_allocation_and_budget_failure() {
    let bytes = wire(&HostResponse::Error {
        message: "diagnostic".into(),
    });
    {
        let mut budget = new_budget();
        let _hook = budget_test_hooks::fail_next_allocation();
        let result = super::decode(&bytes, &ReplyShape::State, &mut budget).unwrap();
        assert!(matches!(
            result,
            DecodedReply::KnownFailure {
                crashed: false,
                reported: None,
                diagnostic_unavailable: Some(ErrorKind::Allocation)
            }
        ));
        assert_eq!(budget.live, 0);
        assert!(!budget.diagnostic_copy);
    }
    {
        let mut budget = new_budget();
        budget.reserve(MAX_PAYLOAD).unwrap();
        let result = super::decode(&bytes, &ReplyShape::State, &mut budget).unwrap();
        assert!(matches!(
            result,
            DecodedReply::KnownFailure {
                crashed: false,
                reported: None,
                diagnostic_unavailable: Some(ErrorKind::Limit)
            }
        ));
        assert_eq!(budget.live, MAX_PAYLOAD);
    }
}

#[test]
fn success_never_uses_the_known_failure_diagnostic_exception() {
    let bytes = wire(&HostResponse::Success {
        message: "ok".into(),
    });
    let mut budget = new_budget();
    assert!(preflight(&bytes, &ReplyShape::Success, &mut budget)
        .unwrap()
        .is_none());
    budget.cancel.cancelled.store(true, Ordering::Release);
    assert!(matches!(
        super::decode(&bytes, &ReplyShape::Success, &mut budget),
        Err(ErrorKind::Cancelled)
    ));
    assert_eq!(budget.live, 0);
}

#[test]
fn preset_path_uid_and_load_block_size_exact_boundaries() {
    let command = HostCommand::LoadState {
        data: Vec::new(),
        context: StateContext::Preset {
            path: Some("p".repeat(MAX_TEXT)),
        },
    };
    assert_command(&command, &ReplyShape::Success);
    let command = HostCommand::LoadState {
        data: Vec::new(),
        context: StateContext::Preset {
            path: Some("p".repeat(MAX_TEXT + 1)),
        },
    };
    assert_eq!(
        encode(&command, &ReplyShape::Success, &mut new_budget()).unwrap_err(),
        ErrorKind::Limit
    );
    let mut response = info(Vec::new());
    if let HostResponse::PluginInfo { uid, .. } = &mut response {
        *uid = "x".repeat(MAX_ID);
    }
    assert!(decode(&wire(&response), &ReplyShape::PluginInfo, &mut new_budget()).is_ok());
    if let HostResponse::PluginInfo { uid, .. } = &mut response {
        uid.push('x');
    }
    error(&wire(&response), &ReplyShape::PluginInfo, ErrorKind::Limit);
    let mut command = load("p".into());
    if let HostCommand::LoadPlugin { block_size, .. } = &mut command {
        *block_size = (MAX_FRAMES + 1) as u32;
    }
    assert_eq!(
        encode(&command, &ReplyShape::PluginInfo, &mut new_budget()).unwrap_err(),
        ErrorKind::Limit
    );
}

#[test]
fn accounting_covers_state_and_escaped_text_without_dynamic_scratch() {
    let line = br#"{"Success":{"message":"\uD83E\uDD80\u0061\n"}}"#;
    let mut budget = new_budget();
    let response = decode(line, &ReplyShape::Success, &mut budget).unwrap();
    let HostResponse::Success { message } = response else {
        panic!("success expected")
    };
    assert_eq!(budget.live, message.capacity());
    assert_eq!(budget.peak, message.capacity());
    let charge = message.capacity();
    drop(message);
    budget.release(charge);
    let response = decode(
        br#"{"State":{"data":"AQIDBA=="}}"#,
        &ReplyShape::State,
        &mut budget,
    )
    .unwrap();
    let HostResponse::State { data } = response else {
        panic!("state expected")
    };
    assert_eq!(budget.live, data.capacity());
    assert_eq!(data, [1, 2, 3, 4]);
}

#[test]
fn deterministic_deadline_interrupts_count_emit_preflight_materialize_and_final_check() {
    let command = HostCommand::LoadState {
        data: vec![6; 32768],
        context: StateContext::Project,
    };
    let count_checks = {
        let _hook = budget_test_hooks::expire_at_check(usize::MAX);
        let mut budget = new_budget();
        budget.check().unwrap();
        check_request(&command, &ReplyShape::Success).unwrap();
        let mut writer = Writer {
            bytes: None,
            len: 0,
            checked: 0,
            budget: &mut budget,
        };
        writer.command(&command).unwrap();
        budget_test_hooks::checks()
    };
    let encode_checks = {
        let _hook = budget_test_hooks::expire_at_check(usize::MAX);
        encode(&command, &ReplyShape::Success, &mut new_budget()).unwrap();
        budget_test_hooks::checks()
    };
    // The first phase counts without allocation; successful grow performs its
    // two reservation checks before any byte-emission checkpoint.
    for at in [2, count_checks + 3, encode_checks] {
        let _hook = budget_test_hooks::expire_at_check(at);
        let mut budget = new_budget();
        assert_eq!(
            encode(&command, &ReplyShape::Success, &mut budget).unwrap_err(),
            ErrorKind::Deadline
        );
        assert_eq!(budget.live, 0);
        assert_eq!(
            budget.check().unwrap_err(),
            ErrorKind::Deadline,
            "injected expiry remains sticky"
        );
    }
    for (response, shape) in [
        (
            HostResponse::State {
                data: vec![7; 32768],
            },
            ReplyShape::State,
        ),
        (
            HostResponse::Success {
                message: "\n🦀".repeat(4096),
            },
            ReplyShape::Success,
        ),
    ] {
        let line = wire(&response);
        let preflight_checks = {
            let _hook = budget_test_hooks::expire_at_check(usize::MAX);
            assert!(preflight(&line, &shape, &mut new_budget())
                .unwrap()
                .is_none());
            budget_test_hooks::checks()
        };
        let decode_checks = {
            let _hook = budget_test_hooks::expire_at_check(usize::MAX);
            decode(&line, &shape, &mut new_budget()).unwrap();
            budget_test_hooks::checks()
        };
        for at in [
            2,
            preflight_checks,
            preflight_checks + (decode_checks - preflight_checks) / 2,
            decode_checks,
        ] {
            let _hook = budget_test_hooks::expire_at_check(at);
            let mut budget = new_budget();
            assert_eq!(
                decode(&line, &shape, &mut budget).unwrap_err(),
                ErrorKind::Deadline
            );
            assert_eq!(budget.live, 0);
        }
    }
}

#[test]
fn preflight_and_representative_rejections_measured_without_heap_activity() {
    let good = audio_wire(vec![vec![0.25; 4096]; 2]);
    let shape = ReplyShape::Audio {
        channels: 2,
        frames: 4096,
    };
    let mut budget = new_budget();
    assert!(allocation_free(|| preflight(&good, &shape, &mut budget))
        .unwrap()
        .is_none());
    let failure = br#"{"Error":{"message":"valid failure detail"}}"#;
    let mut budget = new_budget();
    assert!(
        allocation_free(|| preflight(failure, &ReplyShape::State, &mut budget))
            .unwrap()
            .is_some()
    );
    let cases: Vec<(Vec<u8>, ReplyShape, ErrorKind)> = vec![
        (
            br#"{"AudioOutput":{"output_events":[{"unparsed": definitely-not-json"#.to_vec(),
            ReplyShape::Audio {
                channels: 0,
                frames: 0,
            },
            ErrorKind::UnsupportedReply,
        ),
        (
            audio_wire(vec![vec![0.0; 1]]),
            ReplyShape::Audio {
                channels: 1,
                frames: 2,
            },
            ErrorKind::Shape,
        ),
        (
            br#"{"Success":{"message":"bounded","unknown":0}}"#.to_vec(),
            ReplyShape::Success,
            ErrorKind::MalformedReply,
        ),
    ];
    for (line, shape, expected) in cases {
        let mut budget = new_budget();
        assert_eq!(
            allocation_free(|| super::decode(&line, &shape, &mut budget)).unwrap_err(),
            expected
        );
    }
}
