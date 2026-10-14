//! Plugin-free source fixtures. These tests must not be compiled or executed
//! before the source archive, independent review and explicit execution gate.
use super::super::{budget_test_hooks, pump, Cancellation, LimitEvidence, LimitSite, MAX_PAYLOAD};
use super::*;
use crate::audio::AudioBusConfig;
use crate::audio::{BusDirection, MediaType};
use crate::internal::native_edit_transport::tests::allocation_free;
use crate::midi::MidiChannel;
use crate::plugin::StateContext;
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
        limit_evidence: None,
    }
}

// Most fixtures inspect ordinary HostResponse values; dedicated failure-marker
// tests below inspect the richer codec result without this convenience adapter.
fn decode(bytes: &[u8], shape: &ReplyShape, budget: &mut Budget) -> Result<HostResponse> {
    match super::decode(bytes, shape, budget)? {
        DecodedReply::Response(response) => Ok(response),
        DecodedReply::MetadataBundleRejected { getter, reason } => {
            Ok(HostResponse::StrictMetadataRejected { getter, reason })
        }
        DecodedReply::MetadataRejected(reason) => {
            Ok(HostResponse::MetadataQueryRejected { reason })
        }
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

fn factory_reply() -> HostResponse {
    let unknown = FactoryClassMetadata {
        class_id: "0123456789ABCDEF0123456789ABCDEF".into(),
        name: "F1 declaration".into(),
        category: "Audio Module Class".into(),
        cardinality: i32::MAX,
        vendor: None,
        version: None,
        sdk_version: None,
        sub_categories: None,
        class_flags: None,
    };
    let mut empty = unknown.clone();
    empty.class_id = "FEDCBA9876543210FEDCBA9876543210".into();
    empty.name = "Successful empty getters 🦀".into();
    empty.vendor = Some(String::new());
    empty.version = Some(String::new());
    empty.sdk_version = Some(String::new());
    empty.sub_categories = Some(Vec::new());
    empty.class_flags = Some(u32::MAX);
    HostResponse::FactoryMetadata {
        metadata: FactoryMetadata {
            path: "factory/\"\\\n🦀.vst3".into(),
            factory: FactoryInfo {
                vendor: "Factory vendor".into(),
                url: String::new(),
                email: "factory@example.invalid".into(),
                flags: i32::MIN,
            },
            classes: vec![unknown, empty],
        },
    }
}

#[test]
fn factory_request_and_typed_reply_preserve_serde_wire_order_and_unknowns() {
    assert_command(
        &HostCommand::ProbeFactory {
            path: "factory/\"\\\n🦀.vst3".into(),
        },
        &ReplyShape::FactoryMetadata,
    );
    let expected = factory_reply();
    let bytes = wire(&expected);
    let mut budget = new_budget();
    assert!(
        allocation_free(|| preflight(&bytes, &ReplyShape::FactoryMetadata, &mut budget))
            .unwrap()
            .is_none()
    );
    assert_eq!(budget.live, 0);
    let actual = decode(&bytes, &ReplyShape::FactoryMetadata, &mut budget).unwrap();
    assert_eq!(wire(&actual), bytes);
    let HostResponse::FactoryMetadata { metadata } = actual else {
        panic!("closed factory shape");
    };
    assert_eq!(metadata.classes[0].vendor, None);
    assert_eq!(metadata.classes[0].sub_categories, None);
    assert_eq!(metadata.classes[1].vendor.as_deref(), Some(""));
    assert_eq!(metadata.classes[1].sub_categories, Some(Vec::new()));
    assert_eq!(metadata.classes[1].class_flags, Some(u32::MAX));
    assert!(
        budget.live >= metadata.classes.capacity() * std::mem::size_of::<FactoryClassMetadata>()
    );
}

#[test]
fn factory_wire_rejects_unknown_missing_duplicate_and_wrong_typed_fields() {
    let value = serde_json::to_value(factory_reply()).unwrap();
    for field in ["path", "factory", "classes"] {
        let mut bad = value.clone();
        bad["FactoryMetadata"]["metadata"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        error(
            &serde_json::to_vec(&bad).unwrap(),
            &ReplyShape::FactoryMetadata,
            ErrorKind::MalformedReply,
        );
    }
    for field in ["vendor", "url", "email", "flags"] {
        let mut bad = value.clone();
        bad["FactoryMetadata"]["metadata"]["factory"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        error(
            &serde_json::to_vec(&bad).unwrap(),
            &ReplyShape::FactoryMetadata,
            ErrorKind::MalformedReply,
        );
    }
    for field in [
        "class_id",
        "name",
        "category",
        "cardinality",
        "vendor",
        "version",
        "sdk_version",
        "sub_categories",
        "class_flags",
    ] {
        let mut bad = value.clone();
        bad["FactoryMetadata"]["metadata"]["classes"][0]
            .as_object_mut()
            .unwrap()
            .remove(field);
        error(
            &serde_json::to_vec(&bad).unwrap(),
            &ReplyShape::FactoryMetadata,
            ErrorKind::MalformedReply,
        );
    }
    for (field, replacement) in [
        ("vendor", serde_json::json!(42)),
        ("cardinality", serde_json::json!(2147483648_u64)),
        ("class_flags", serde_json::json!(-1)),
        ("sub_categories", serde_json::json!(false)),
        ("has_gui", serde_json::json!(true)),
    ] {
        let mut bad = value.clone();
        bad["FactoryMetadata"]["metadata"]["classes"][0][field] = replacement;
        error(
            &serde_json::to_vec(&bad).unwrap(),
            &ReplyShape::FactoryMetadata,
            ErrorKind::MalformedReply,
        );
    }
    let bytes = String::from_utf8(wire(&factory_reply())).unwrap();
    for bad in [
        bytes.replacen("\"metadata\":{", "\"metadata\":{},\"metadata\":{", 1),
        bytes.replacen("\"classes\":[", "\"classes\":[],\"classes\":[", 1),
        bytes.replacen("\"vendor\":null", "\"vendor\":null,\"vendor\":null", 1),
    ] {
        error(
            bad.as_bytes(),
            &ReplyShape::FactoryMetadata,
            ErrorKind::MalformedReply,
        );
    }
    error(
        &wire(&factory_reply()),
        &ReplyShape::PluginInfo,
        ErrorKind::WrongReply,
    );
    error(
        &wire(&info(Vec::new())),
        &ReplyShape::FactoryMetadata,
        ErrorKind::WrongReply,
    );
}

#[test]
fn factory_path_class_id_text_classes_and_subcategories_have_closed_limits() {
    assert_command(
        &HostCommand::ProbeFactory {
            path: "x".repeat(MAX_TEXT),
        },
        &ReplyShape::FactoryMetadata,
    );
    assert_eq!(
        encode(
            &HostCommand::ProbeFactory {
                path: "x".repeat(MAX_TEXT + 1)
            },
            &ReplyShape::FactoryMetadata,
            &mut new_budget()
        )
        .unwrap_err(),
        ErrorKind::Limit
    );
    assert_eq!(
        encode(
            &HostCommand::ProbeFactory { path: "x".into() },
            &ReplyShape::PluginInfo,
            &mut new_budget()
        )
        .unwrap_err(),
        ErrorKind::Shape
    );
    let HostResponse::FactoryMetadata { mut metadata } = factory_reply() else {
        unreachable!()
    };
    let class = metadata.classes.remove(0);
    metadata.classes = vec![class.clone(); MAX_FACTORY_CLASSES];
    let accepted = HostResponse::FactoryMetadata {
        metadata: metadata.clone(),
    };
    assert!(decode(
        &wire(&accepted),
        &ReplyShape::FactoryMetadata,
        &mut new_budget()
    )
    .is_ok());
    metadata.classes.push(class.clone());
    error(
        &wire(&HostResponse::FactoryMetadata { metadata }),
        &ReplyShape::FactoryMetadata,
        ErrorKind::Limit,
    );
    for (field, accepted, rejected) in [
        (
            "path",
            serde_json::json!("x".repeat(MAX_TEXT)),
            serde_json::json!("x".repeat(MAX_TEXT + 1)),
        ),
        (
            "name",
            serde_json::json!("x".repeat(MAX_TEXT)),
            serde_json::json!("x".repeat(MAX_TEXT + 1)),
        ),
        (
            "vendor",
            serde_json::json!("x".repeat(MAX_TEXT)),
            serde_json::json!("x".repeat(MAX_TEXT + 1)),
        ),
        (
            "sub_categories",
            serde_json::json!(vec!["x"; MAX_FACTORY_SUB_CATEGORIES]),
            serde_json::json!(vec!["x"; MAX_FACTORY_SUB_CATEGORIES + 1]),
        ),
    ] {
        let mut value = serde_json::to_value(factory_reply()).unwrap();
        if field == "path" {
            value["FactoryMetadata"]["metadata"][field] = accepted;
        } else {
            value["FactoryMetadata"]["metadata"]["classes"][0][field] = accepted;
        }
        assert!(decode(
            &serde_json::to_vec(&value).unwrap(),
            &ReplyShape::FactoryMetadata,
            &mut new_budget()
        )
        .is_ok());
        if field == "path" {
            value["FactoryMetadata"]["metadata"][field] = rejected;
        } else {
            value["FactoryMetadata"]["metadata"]["classes"][0][field] = rejected;
        }
        error(
            &serde_json::to_vec(&value).unwrap(),
            &ReplyShape::FactoryMetadata,
            ErrorKind::Limit,
        );
    }
}

#[test]
fn factory_class_id_must_be_nonzero_canonical_uppercase_hex() {
    for id in [
        String::new(),
        "0".repeat(32),
        "a".repeat(32),
        "G".repeat(32),
        "1".repeat(31),
        "1".repeat(33),
        "1".repeat(MAX_ID),
    ] {
        let mut value = serde_json::to_value(factory_reply()).unwrap();
        value["FactoryMetadata"]["metadata"]["classes"][0]["class_id"] = serde_json::json!(id);
        let bytes = serde_json::to_vec(&value).unwrap();
        let mut budget = new_budget();
        assert!(matches!(
            allocation_free(|| preflight(&bytes, &ReplyShape::FactoryMetadata, &mut budget)),
            Err(ErrorKind::MalformedReply)
        ));
        assert_eq!(budget.live, 0);
    }
    let mut value = serde_json::to_value(factory_reply()).unwrap();
    value["FactoryMetadata"]["metadata"]["classes"][0]["class_id"] =
        serde_json::json!("1".repeat(MAX_ID + 1));
    error(
        &serde_json::to_vec(&value).unwrap(),
        &ReplyShape::FactoryMetadata,
        ErrorKind::Limit,
    );
}

#[test]
fn factory_codec_checks_cancel_deadline_allocation_and_payload_budget() {
    let command = HostCommand::ProbeFactory { path: "x".into() };
    let bytes = wire(&factory_reply());
    for cancelled in [false, true] {
        let mut budget = new_budget();
        if cancelled {
            budget.cancel.cancelled.store(true, Ordering::Release);
        } else {
            budget.deadline = Instant::now();
        }
        let expected = if cancelled {
            ErrorKind::Cancelled
        } else {
            ErrorKind::Deadline
        };
        assert_eq!(
            allocation_free(|| encode(&command, &ReplyShape::FactoryMetadata, &mut budget))
                .unwrap_err(),
            expected
        );
        assert_eq!(
            allocation_free(|| decode(&bytes, &ReplyShape::FactoryMetadata, &mut budget))
                .unwrap_err(),
            expected
        );
        assert_eq!(budget.live, 0);
    }
    let mut budget = new_budget();
    budget.reserve(MAX_PAYLOAD).unwrap();
    assert_eq!(
        decode(&bytes, &ReplyShape::FactoryMetadata, &mut budget).unwrap_err(),
        ErrorKind::Limit
    );
    assert_eq!(budget.live, MAX_PAYLOAD);
    let mut budget = new_budget();
    let _hook = budget_test_hooks::fail_next_allocation();
    assert_eq!(
        decode(&bytes, &ReplyShape::FactoryMetadata, &mut budget).unwrap_err(),
        ErrorKind::Allocation
    );
    assert_eq!(budget.live, 0);
}

#[test]
fn factory_empty_list_and_nonempty_subcategories_preserve_exact_order() {
    let HostResponse::FactoryMetadata { mut metadata } = factory_reply() else {
        unreachable!()
    };
    metadata.classes.clear();
    let empty = HostResponse::FactoryMetadata {
        metadata: metadata.clone(),
    };
    assert_eq!(
        wire(
            &decode(
                &wire(&empty),
                &ReplyShape::FactoryMetadata,
                &mut new_budget()
            )
            .unwrap()
        ),
        wire(&empty)
    );
    let HostResponse::FactoryMetadata { mut metadata } = factory_reply() else {
        unreachable!()
    };
    metadata.classes[0].sub_categories = Some(vec!["Instrument".into(), "Synth".into(), "".into()]);
    let expected = HostResponse::FactoryMetadata { metadata };
    assert_eq!(
        wire(
            &decode(
                &wire(&expected),
                &ReplyShape::FactoryMetadata,
                &mut new_budget()
            )
            .unwrap()
        ),
        wire(&expected)
    );
}

#[test]
fn factory_large_reply_cooperatively_cancels_and_expires_in_both_decode_passes() {
    let HostResponse::FactoryMetadata { mut metadata } = factory_reply() else {
        unreachable!()
    };
    metadata.classes[0].name = "long 🦀 text".repeat(2048);
    metadata.classes[0].sub_categories = Some(vec!["Instrument".into(); 128]);
    let bytes = wire(&HostResponse::FactoryMetadata { metadata });
    let validation_checks = {
        let _hook = budget_test_hooks::cancel_at_check(usize::MAX);
        assert!(
            preflight(&bytes, &ReplyShape::FactoryMetadata, &mut new_budget())
                .unwrap()
                .is_none()
        );
        budget_test_hooks::checks()
    };
    let all_checks = {
        let _hook = budget_test_hooks::cancel_at_check(usize::MAX);
        decode(&bytes, &ReplyShape::FactoryMetadata, &mut new_budget()).unwrap();
        budget_test_hooks::checks()
    };
    assert!(all_checks > validation_checks + 2);
    for at in [2, validation_checks, validation_checks + 2, all_checks] {
        for expired in [false, true] {
            let mut budget = new_budget();
            let _hook = if expired {
                budget_test_hooks::expire_at_check(at)
            } else {
                budget_test_hooks::cancel_at_check(at)
            };
            assert_eq!(
                decode(&bytes, &ReplyShape::FactoryMetadata, &mut budget).unwrap_err(),
                if expired {
                    ErrorKind::Deadline
                } else {
                    ErrorKind::Cancelled
                }
            );
            assert_eq!(budget.live, 0);
        }
    }
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
        HostCommand::CreateGui,
        HostCommand::MidiPanic,
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
        ErrorKind::WrongReply,
    );
    error(
        br#"{"ParameterString":{"value":"still unsupported"}}"#,
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
        ("\"audio_inputs\":8", "\"audio_inputs\":33"),
        ("\"audio_outputs\":8", "\"audio_outputs\":-1"),
        ("\"audio_outputs\":8", "\"audio_outputs\":33"),
        ("\"output_channels\":32", "\"output_channels\":65"),
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
fn declared_32_stereo_outputs_do_not_expand_audio_exchange_limits() {
    let mut declared = info(Vec::new());
    if let HostResponse::PluginInfo {
        audio_inputs,
        audio_outputs,
        output_channels,
        ..
    } = &mut declared
    {
        *audio_inputs = 32;
        *audio_outputs = 32;
        *output_channels = 64;
    }
    let decoded = decode(&wire(&declared), &ReplyShape::PluginInfo, &mut new_budget()).unwrap();
    assert_eq!(wire(&decoded), wire(&declared));
    let command = HostCommand::Process {
        inputs: Vec::new(),
        frames: 128,
        transport: None,
    };
    assert_eq!(
        encode(
            &command,
            &ReplyShape::Audio {
                channels: 64,
                frames: 128
            },
            &mut new_budget()
        )
        .unwrap_err(),
        ErrorKind::Limit,
    );
    assert_eq!(
        encode(
            &HostCommand::ProcessBuses {
                inputs: Vec::new(),
                outputs: vec![
                    AudioBusConfig {
                        channel_count: 2,
                        active: false
                    };
                    32
                ],
                frames: 128,
                transport: None,
            },
            &ReplyShape::Buses {
                buses: vec![(false, 2); 32],
                frames: 128
            },
            &mut new_budget(),
        )
        .unwrap_err(),
        ErrorKind::Limit,
    );
}

#[test]
fn selected_audio_bus_request_preserves_wire_and_rejects_event_or_invalid_index() {
    for direction in [BusDirection::Input, BusDirection::Output] {
        for bus_index in [0, 31] {
            for active in [false, true] {
                assert_command(
                    &HostCommand::SetBusActive {
                        media_type: MediaType::Audio,
                        direction,
                        bus_index,
                        active,
                    },
                    &ReplyShape::Success,
                );
            }
        }
    }
    for (media_type, bus_index) in [
        (MediaType::Audio, -1),
        (MediaType::Audio, 32),
        (MediaType::Audio, i32::MAX),
        (MediaType::Event, 0),
    ] {
        let mut budget = new_budget();
        budget.reserve(17).unwrap();
        assert_eq!(
            encode(
                &HostCommand::SetBusActive {
                    media_type,
                    direction: BusDirection::Output,
                    bus_index,
                    active: false,
                },
                &ReplyShape::Success,
                &mut budget,
            )
            .unwrap_err(),
            ErrorKind::InvalidRequest
        );
        assert_eq!(budget.live, 17);
    }
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
            metadata_getter: None,
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
                diagnostic_unavailable: Some(ErrorKind::Allocation),
                metadata_getter: None,
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
                diagnostic_unavailable: Some(ErrorKind::Limit),
                metadata_getter: None,
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
        let view = RequestView::try_from(&command).unwrap();
        budget.check().unwrap();
        check_request(&view, &ReplyShape::Success).unwrap();
        let mut writer = Writer {
            bytes: None,
            len: 0,
            checked: 0,
            budget: &mut budget,
        };
        writer.command(&view).unwrap();
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

#[test]
fn offline_additions_emit_the_existing_wire_and_borrowed_path_matches() {
    for (command, shape) in [
        (
            HostCommand::SetProcessMode {
                mode: ProcessMode::Offline,
            },
            ReplyShape::Success,
        ),
        (
            HostCommand::ResetOriginSupport,
            ReplyShape::ResetOriginSupport,
        ),
        (HostCommand::StartProcessing, ReplyShape::Success),
        (HostCommand::StopProcessing, ReplyShape::Success),
        (
            HostCommand::NativeDirtyRevision,
            ReplyShape::NativeDirtyRevision,
        ),
        (HostCommand::GetAllParameters, ReplyShape::Parameters),
        (
            HostCommand::CheckedAudioBusLayout,
            ReplyShape::CheckedAudioBusLayout,
        ),
        (HostCommand::EventBusLayout, ReplyShape::EventBusLayout),
    ] {
        assert_command(&command, &shape);
        let view = allocation_free(|| RequestView::try_from(&command)).unwrap();
        assert_eq!(
            encode(&command, &shape, &mut new_budget()).unwrap(),
            encode_view(&view, &shape, &mut new_budget()).unwrap(),
        );
        let mut budget = new_budget();
        assert_eq!(
            allocation_free(|| encode(&command, &ReplyShape::State, &mut budget)).unwrap_err(),
            ErrorKind::Shape,
        );
        assert_eq!((budget.live, budget.peak), (0, 0));
    }
    for value in [-0.0, 0.0, f64::from_bits(1), 0.5, 1.0] {
        assert_command(
            &HostCommand::SetParameter {
                id: u32::MAX,
                value,
            },
            &ReplyShape::Success,
        );
    }
    for channel_index in 0..16 {
        let channel = MidiChannel::from_index(channel_index).unwrap();
        for note in [0, 127] {
            for velocity in [0, 127] {
                for event in [
                    MidiEvent::NoteOn {
                        channel,
                        note,
                        velocity,
                    },
                    MidiEvent::NoteOff {
                        channel,
                        note,
                        velocity,
                    },
                ] {
                    for sample_offset in [0, 127] {
                        assert_command(
                            &HostCommand::SendMidiAt {
                                event,
                                sample_offset,
                            },
                            &ReplyShape::Success,
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn offline_setter_mode_and_note_admission_reject_before_allocating() {
    let reject = |command: HostCommand| {
        let mut budget = new_budget();
        assert_eq!(
            allocation_free(|| encode(&command, &ReplyShape::Success, &mut budget)).unwrap_err(),
            ErrorKind::InvalidRequest,
        );
        assert_eq!((budget.live, budget.peak), (0, 0));
    };
    for mode in [ProcessMode::Realtime, ProcessMode::Prefetch] {
        reject(HostCommand::SetProcessMode { mode });
    }
    for value in [
        -f64::from_bits(1),
        1.0 + f64::EPSILON,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ] {
        reject(HostCommand::SetParameter { id: 0, value });
    }
    let channel = MidiChannel::Ch1;
    for event in [
        MidiEvent::NoteOn {
            channel,
            note: 128,
            velocity: 127,
        },
        MidiEvent::NoteOn {
            channel,
            note: 127,
            velocity: 128,
        },
        MidiEvent::NoteOff {
            channel,
            note: 255,
            velocity: 0,
        },
        MidiEvent::NoteOff {
            channel,
            note: 0,
            velocity: 255,
        },
        MidiEvent::ControlChange {
            channel,
            controller: 1,
            value: 1,
        },
        MidiEvent::ProgramChange {
            channel,
            program: 0,
        },
        MidiEvent::PitchBend {
            channel,
            value: 8192,
        },
        MidiEvent::ChannelAftertouch {
            channel,
            pressure: 0,
        },
        MidiEvent::PolyAftertouch {
            channel,
            note: 0,
            pressure: 0,
        },
    ] {
        reject(HostCommand::SendMidiAt {
            event,
            sample_offset: 0,
        });
    }
    for sample_offset in [-1, 128, i32::MIN, i32::MAX] {
        reject(HostCommand::SendMidiAt {
            event: MidiEvent::NoteOn {
                channel,
                note: 60,
                velocity: 100,
            },
            sample_offset,
        });
    }
}

#[test]
fn borrowed_large_state_preexpired_and_early_cancel_have_no_copy_or_mutation() {
    let state = vec![0xa5; 64 * 1024 * 1024];
    let pointer = state.as_ptr();
    let capacity = state.capacity();
    let request = RequestView::LoadState {
        data: &state,
        context: StateContextView::Project,
    };
    let mut budget = new_budget();
    budget.deadline = Instant::now();
    assert_eq!(
        allocation_free(|| encode_view(&request, &ReplyShape::Success, &mut budget)).unwrap_err(),
        ErrorKind::Deadline,
    );
    assert_eq!((budget.live, budget.peak), (0, 0));
    for check in [1, 2, 3, 4] {
        let mut budget = new_budget();
        let _hook = budget_test_hooks::cancel_at_check(check);
        assert_eq!(
            allocation_free(|| encode_view(&request, &ReplyShape::Success, &mut budget))
                .unwrap_err(),
            ErrorKind::Cancelled,
        );
        assert_eq!((budget.live, budget.peak), (0, 0));
    }
    assert_eq!(state.as_ptr(), pointer);
    assert_eq!(state.capacity(), capacity);
    assert!(state.iter().all(|&byte| byte == 0xa5));
}

#[test]
fn request_view_preserves_text_state_and_audio_storage_identity() {
    let command = load("borrowed/🦀.vst3".into());
    let view = allocation_free(|| RequestView::try_from(&command)).unwrap();
    let (
        HostCommand::LoadPlugin { path, class_id, .. },
        RequestView::LoadPlugin {
            path: borrowed,
            class_id: id,
            ..
        },
    ) = (&command, view)
    else {
        panic!("load view")
    };
    assert_eq!(path.as_ptr(), borrowed.as_ptr());
    assert_eq!(class_id.as_ref().unwrap().as_ptr(), id.unwrap().as_ptr());
    let command = HostCommand::LoadState {
        data: vec![0, 1, 255],
        context: StateContext::Preset {
            path: Some("preset".into()),
        },
    };
    let view = allocation_free(|| RequestView::try_from(&command)).unwrap();
    let (
        HostCommand::LoadState {
            data,
            context: StateContext::Preset { path },
        },
        RequestView::LoadState {
            data: borrowed,
            context: StateContextView::Preset {
                path: borrowed_path,
            },
        },
    ) = (&command, view)
    else {
        panic!("state view")
    };
    assert_eq!(data.as_ptr(), borrowed.as_ptr());
    assert_eq!(
        path.as_ref().unwrap().as_ptr(),
        borrowed_path.unwrap().as_ptr()
    );
    let samples = vec![vec![f32::from_bits(0x7fc0_1234), -0.0]];
    let view = RequestView::Process {
        inputs: FlatInputView::Borrowed(&samples),
        frames: 2,
        transport: None,
    };
    let encoded = encode_view(
        &view,
        &ReplyShape::Audio {
            channels: 1,
            frames: 2,
        },
        &mut new_budget(),
    )
    .unwrap();
    let HostCommand::Process { inputs, .. } = serde_json::from_slice(&encoded).unwrap() else {
        panic!("process wire")
    };
    assert_eq!(inputs[0][0].to_bits(), samples[0][0].to_bits());
    assert_eq!(inputs[0][1].to_bits(), samples[0][1].to_bits());
    assert_eq!(samples[0][0].to_bits(), 0x7fc0_1234);
    assert_eq!(samples[0][1].to_bits(), (-0.0f32).to_bits());
}

fn catalog_parameter(id: u32) -> Parameter {
    Parameter {
        id,
        name: "Gain\n🦀".into(),
        value: 0.25,
        min: -0.0,
        max: 1.0,
        default: 0.75,
        unit: "dB\t".into(),
        step_count: i32::MAX,
        can_automate: true,
        is_read_only: false,
        is_bypass: true,
        flags: u32::MAX,
    }
}

#[test]
fn catalog_preserves_every_field_and_charges_actual_payload_capacities() {
    let mut second = catalog_parameter(u32::MAX);
    second.value = f64::from_bits(1);
    second.default = -0.0;
    second.step_count = i32::MIN;
    second.can_automate = false;
    second.is_read_only = true;
    second.is_bypass = false;
    second.flags = 0;
    let source = HostResponse::Parameters {
        params: vec![catalog_parameter(0), second],
    };
    let bytes = wire(&source);
    let mut budget = new_budget();
    assert!(
        allocation_free(|| preflight(&bytes, &ReplyShape::Parameters, &mut budget))
            .unwrap()
            .is_none()
    );
    assert_eq!(budget.limit_evidence, None);
    assert_eq!((budget.live, budget.peak), (0, 0));
    let decoded = decode(&bytes, &ReplyShape::Parameters, &mut budget).unwrap();
    assert_eq!(wire(&decoded), bytes);
    let HostResponse::Parameters { params } = decoded else {
        panic!("catalog")
    };
    assert_eq!(params[0].min.to_bits(), (-0.0f64).to_bits());
    assert_eq!(params[1].default.to_bits(), (-0.0f64).to_bits());
    let charge = params.capacity() * std::mem::size_of::<Parameter>()
        + params
            .iter()
            .map(|p| p.name.capacity() + p.unit.capacity())
            .sum::<usize>();
    assert_eq!(budget.live, charge);
    assert_eq!(budget.peak, charge);
}

#[test]
fn catalog_exact_entry_limit_and_plus_one_and_duplicate_ids_are_preflight_only() {
    let mut params: Vec<_> = (0..MAX_PARAMETERS)
        .map(|id| {
            let mut parameter = catalog_parameter(id as u32);
            parameter.name.clear();
            parameter.unit.clear();
            parameter
        })
        .collect();
    let bytes = wire(&HostResponse::Parameters {
        params: params.clone(),
    });
    let mut budget = new_budget();
    assert!(
        allocation_free(|| preflight(&bytes, &ReplyShape::Parameters, &mut budget))
            .unwrap()
            .is_none()
    );
    assert_eq!(budget.limit_evidence, None);
    let decoded = decode(&bytes, &ReplyShape::Parameters, &mut new_budget()).unwrap();
    assert!(
        matches!(decoded, HostResponse::Parameters { params } if params.len() == MAX_PARAMETERS)
    );
    params.push(catalog_parameter(MAX_PARAMETERS as u32));
    let bytes = wire(&HostResponse::Parameters {
        params: params.clone(),
    });
    let mut budget = new_budget();
    assert_eq!(
        allocation_free(|| decode(&bytes, &ReplyShape::Parameters, &mut budget)).unwrap_err(),
        ErrorKind::Limit
    );
    assert_eq!(budget.peak, 0);
    assert_eq!(
        budget.limit_evidence,
        Some(LimitEvidence {
            site: LimitSite::ParameterCatalogEntries,
            observed_at_least: MAX_PARAMETERS + 1,
            maximum: MAX_PARAMETERS,
        })
    );
    params.pop();
    // A nonadjacent duplicate near the end is caught before any earlier titles
    // or descriptors are materialized. No partial catalog escapes to setters.
    params[MAX_PARAMETERS - 1].id = params[0].id;
    let bytes = wire(&HostResponse::Parameters { params });
    let mut budget = new_budget();
    assert_eq!(
        allocation_free(|| decode(&bytes, &ReplyShape::Parameters, &mut budget)).unwrap_err(),
        ErrorKind::MalformedReply
    );
    assert_eq!((budget.live, budget.peak), (0, 0));
    assert_eq!(
        budget.limit_evidence, None,
        "duplicate IDs do not fabricate a count limit"
    );
}

#[test]
fn catalog_utf8_text_exact_limits_and_aggregate_boundary() {
    let mut parameter = catalog_parameter(0);
    parameter.name = "🦀".repeat(MAX_PARAMETER_TEXT / 4);
    parameter.unit = "u".repeat(MAX_PARAMETER_TEXT);
    let bytes = wire(&HostResponse::Parameters {
        params: vec![parameter.clone()],
    });
    let mut exact_budget = new_budget();
    assert!(decode(&bytes, &ReplyShape::Parameters, &mut exact_budget).is_ok());
    assert_eq!(exact_budget.limit_evidence, None);
    for field in ["name", "unit"] {
        let mut invalid = parameter.clone();
        if field == "name" {
            invalid.name.push('x');
        } else {
            invalid.unit.push('x');
        }
        let bytes = wire(&HostResponse::Parameters {
            params: vec![invalid],
        });
        let mut budget = new_budget();
        assert_eq!(
            allocation_free(|| decode(&bytes, &ReplyShape::Parameters, &mut budget)).unwrap_err(),
            ErrorKind::Limit
        );
        assert_eq!(budget.peak, 0);
        assert_eq!(
            budget.limit_evidence,
            Some(LimitEvidence {
                site: LimitSite::ParameterTextBytes,
                observed_at_least: MAX_PARAMETER_TEXT + 1,
                maximum: MAX_PARAMETER_TEXT,
            })
        );
    }
    let params: Vec<_> = (0..MAX_PARAMETERS)
        .map(|id| {
            let mut p = parameter.clone();
            p.id = id as u32;
            p
        })
        .collect();
    assert_eq!(
        params
            .iter()
            .map(|p| p.name.len() + p.unit.len())
            .sum::<usize>(),
        MAX_CATALOG_TEXT
    );
    let bytes = wire(&HostResponse::Parameters { params });
    let mut budget = new_budget();
    assert!(
        allocation_free(|| preflight(&bytes, &ReplyShape::Parameters, &mut budget))
            .unwrap()
            .is_none()
    );
    let decoded = decode(&bytes, &ReplyShape::Parameters, &mut budget).unwrap();
    assert_eq!(wire(&decoded), bytes);
    // With 4096 entries and two 1024-byte strings, aggregate+1 on a wire also
    // violates an earlier limit. Exercise the aggregate guard independently.
    let mut fresh = new_budget();
    let mut parser = Parser {
        bytes: br#""x""#,
        position: 0,
        checked: 0,
        depth: 0,
        materialize: true,
        failure_message: None,
        budget: &mut fresh,
    };
    let mut total = MAX_CATALOG_TEXT;
    assert_eq!(
        allocation_free(|| parser.catalog_text(&mut total)).unwrap_err(),
        ErrorKind::Limit
    );
    assert_eq!(fresh.peak, 0);
    assert_eq!(
        fresh.limit_evidence,
        Some(LimitEvidence {
            site: LimitSite::ParameterCatalogTextBytes,
            observed_at_least: MAX_CATALOG_TEXT + 1,
            maximum: MAX_CATALOG_TEXT,
        })
    );
}

#[test]
fn catalog_closed_schema_rejects_missing_unknown_duplicate_and_invalid_fields() {
    let parameter = catalog_parameter(7);
    let source = serde_json::to_value(&parameter).unwrap();
    let fields = [
        "id",
        "name",
        "value",
        "min",
        "max",
        "default",
        "unit",
        "step_count",
        "can_automate",
        "is_read_only",
        "is_bypass",
        "flags",
    ];
    let reject = |parameter: serde_json::Value| {
        let line =
            serde_json::to_vec(&serde_json::json!({"Parameters":{"params":[parameter]}})).unwrap();
        let mut budget = new_budget();
        assert_eq!(
            allocation_free(|| decode(&line, &ReplyShape::Parameters, &mut budget)).unwrap_err(),
            ErrorKind::MalformedReply
        );
        assert_eq!((budget.live, budget.peak), (0, 0));
    };
    for field in fields {
        let mut missing = source.clone();
        missing.as_object_mut().unwrap().remove(field);
        reject(missing);
        let field_json = serde_json::to_string(&source[field]).unwrap();
        let object = serde_json::to_string(&source).unwrap();
        let duplicate = format!("{{\"{field}\":{field_json},{}", &object[1..]);
        let line = format!("{{\"Parameters\":{{\"params\":[{duplicate}]}}}}");
        let mut budget = new_budget();
        assert_eq!(
            allocation_free(|| decode(line.as_bytes(), &ReplyShape::Parameters, &mut budget))
                .unwrap_err(),
            ErrorKind::MalformedReply
        );
        assert_eq!(budget.peak, 0);
    }
    let mut unknown = source.clone();
    unknown["unknown"] = serde_json::json!({"nested":[[[]]]});
    reject(unknown);
    for field in ["value", "min", "max", "default"] {
        for value in [
            serde_json::Value::Null,
            serde_json::json!("NaN"),
            serde_json::json!("inf"),
            serde_json::json!("0.5"),
            serde_json::json!(false),
        ] {
            let mut invalid = source.clone();
            invalid[field] = value;
            reject(invalid);
        }
        let original = serde_json::to_string(&source).unwrap();
        let needle = format!(
            "\"{field}\":{}",
            serde_json::to_string(&source[field]).unwrap()
        );
        let invalid = original.replacen(&needle, &format!("\"{field}\":1e309"), 1);
        let line = format!("{{\"Parameters\":{{\"params\":[{invalid}]}}}}");
        error(
            line.as_bytes(),
            &ReplyShape::Parameters,
            ErrorKind::MalformedReply,
        );
    }
    for (field, value) in [
        ("id", serde_json::json!(-1)),
        ("id", serde_json::json!(4294967296u64)),
        ("id", serde_json::json!(1.5)),
        ("flags", serde_json::json!(4294967296u64)),
        ("flags", serde_json::json!(-1)),
        ("step_count", serde_json::json!(2147483648u64)),
        ("step_count", serde_json::json!(-2147483649i64)),
        ("step_count", serde_json::json!(1.5)),
        ("name", serde_json::json!(0)),
        ("unit", serde_json::Value::Null),
        ("can_automate", serde_json::json!(1)),
        ("is_read_only", serde_json::json!("false")),
        ("is_bypass", serde_json::Value::Null),
    ] {
        let mut invalid = source.clone();
        invalid[field] = value;
        reject(invalid);
    }
    for line in [
        br#"{"Parameters":{"params":null}}"#.as_slice(),
        br#"{"Parameters":{"params":{}}}"#.as_slice(),
        br#"{"Parameters":{"params":[[]]}}"#.as_slice(),
        br#"{"Parameters":{"params":[],"params":[]}}"#.as_slice(),
        br#"{"Parameters":{"params":[],"unknown":0}}"#.as_slice(),
        br#"{"Parameters":{}}"#.as_slice(),
    ] {
        error(line, &ReplyShape::Parameters, ErrorKind::MalformedReply);
    }
}

#[test]
fn new_scalar_metadata_responses_preserve_exact_values_and_schema() {
    for source in [
        HostResponse::ResetOriginSupport {
            support: ResetOriginSupport {
                contract_version: u32::MAX,
                max_block_frames: u32::MAX,
                processing: true,
            },
        },
        HostResponse::ResetOriginSupport {
            support: ResetOriginSupport {
                contract_version: 1,
                max_block_frames: 2048,
                processing: false,
            },
        },
        HostResponse::NativeDirtyRevision { revision: u64::MAX },
        HostResponse::NativeDirtyRevision { revision: 0 },
    ] {
        let shape = if matches!(source, HostResponse::NativeDirtyRevision { .. }) {
            ReplyShape::NativeDirtyRevision
        } else {
            ReplyShape::ResetOriginSupport
        };
        let bytes = wire(&source);
        let mut budget = new_budget();
        let response = allocation_free(|| decode(&bytes, &shape, &mut budget)).unwrap();
        assert_eq!(wire(&response), bytes);
        assert_eq!((budget.live, budget.peak), (0, 0));
        error(&bytes, &ReplyShape::Success, ErrorKind::WrongReply);
    }
    for bytes in [
        br#"{"NativeDirtyRevision":{"revision":18446744073709551616}}"#.as_slice(),
        br#"{"NativeDirtyRevision":{"revision":-1}}"#.as_slice(),
        br#"{"NativeDirtyRevision":{"revision":"1"}}"#.as_slice(),
        br#"{"NativeDirtyRevision":{"revision":1.0}}"#.as_slice(),
        br#"{"NativeDirtyRevision":{"revision":1,"revision":2}}"#.as_slice(),
        br#"{"NativeDirtyRevision":{"unknown":0}}"#.as_slice(),
        br#"{"NativeDirtyRevision":{}}"#.as_slice(),
    ] {
        error(
            bytes,
            &ReplyShape::NativeDirtyRevision,
            ErrorKind::MalformedReply,
        );
    }
    for bytes in [
        br#"{"ResetOriginSupport":{"support":{"contract_version":1,"max_block_frames":2048}}}"#.as_slice(),
        br#"{"ResetOriginSupport":{"support":{"contract_version":1,"max_block_frames":2048,"processing":null}}}"#.as_slice(),
        br#"{"ResetOriginSupport":{"support":{"contract_version":-1,"max_block_frames":2048,"processing":false}}}"#.as_slice(),
        br#"{"ResetOriginSupport":{"support":{"contract_version":1,"max_block_frames":4294967296,"processing":false}}}"#.as_slice(),
        br#"{"ResetOriginSupport":{"support":{"contract_version":1,"max_block_frames":2048,"processing":false,"unknown":0}}}"#.as_slice(),
        br#"{"ResetOriginSupport":{"support":{"contract_version":1,"contract_version":1,"max_block_frames":2048,"processing":false}}}"#.as_slice(),
        br#"{"ResetOriginSupport":{"support":null}}"#.as_slice(),
    ] { error(bytes, &ReplyShape::ResetOriginSupport, ErrorKind::MalformedReply); }
}

fn layout_wire(audio: bool, input_count: usize, output_count: usize, channels: usize) -> Vec<u8> {
    let buses = |count: usize| -> Vec<serde_json::Value> {
        (0..count)
            .map(|index| {
                serde_json::json!({
                    "channel_count": channels, "is_main": index % 2 == 0,
                    "active": match index % 3 { 0 => Some(true), 1 => Some(false), _ => None },
                })
            })
            .collect()
    };
    let name = if audio {
        "CheckedAudioBusLayout"
    } else {
        "EventBusLayout"
    };
    serde_json::to_vec(&serde_json::json!({name: {"layout": {"inputs": buses(input_count), "outputs": buses(output_count)}}})).unwrap()
}

#[test]
fn complete_layouts_preserve_order_channels_main_aux_and_unknown_activation() {
    for audio in [false, true] {
        let shape = if audio {
            ReplyShape::CheckedAudioBusLayout
        } else {
            ReplyShape::EventBusLayout
        };
        let maximum = if audio { 256 } else { 16 };
        for channels in [1, maximum] {
            let bytes = layout_wire(audio, MAX_LAYOUT_BUSES, MAX_LAYOUT_BUSES, channels);
            let mut budget = new_budget();
            assert!(allocation_free(|| preflight(&bytes, &shape, &mut budget))
                .unwrap()
                .is_none());
            assert_eq!(budget.peak, 0);
            let decoded = decode(&bytes, &shape, &mut budget).unwrap();
            let expected: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(serde_json::to_value(&decoded).unwrap(), expected);
            match decoded {
                HostResponse::CheckedAudioBusLayout { layout } => {
                    assert_eq!(layout.inputs.len(), 32);
                    assert_eq!(layout.outputs.len(), 32);
                    assert!(layout.inputs[0].is_main);
                    assert!(!layout.inputs[1].is_main);
                    assert_eq!(layout.inputs[0].active, Some(true));
                    assert_eq!(layout.inputs[1].active, Some(false));
                    assert_eq!(layout.inputs[2].active, None);
                    assert_eq!(
                        budget.live,
                        (layout.inputs.capacity() + layout.outputs.capacity())
                            * std::mem::size_of::<CheckedAudioBusConfig>()
                    );
                }
                HostResponse::EventBusLayout { layout } => {
                    assert_eq!(layout.inputs.len(), 32);
                    assert_eq!(layout.outputs.len(), 32);
                    assert!(layout.inputs[0].is_main);
                    assert!(!layout.inputs[1].is_main);
                    assert_eq!(layout.inputs[0].active, Some(true));
                    assert_eq!(layout.inputs[1].active, Some(false));
                    assert_eq!(layout.inputs[2].active, None);
                    assert_eq!(
                        budget.live,
                        (layout.inputs.capacity() + layout.outputs.capacity())
                            * std::mem::size_of::<EventBusConfig>()
                    );
                }
                _ => panic!("raw typed layout"),
            }
        }
        for (inputs, outputs, channels) in [(33, 0, 1), (0, 33, 1), (1, 0, maximum + 1), (0, 1, 0)]
        {
            let bytes = layout_wire(audio, inputs, outputs, channels);
            let mut budget = new_budget();
            assert_eq!(
                allocation_free(|| decode(&bytes, &shape, &mut budget)).unwrap_err(),
                ErrorKind::Limit
            );
            assert_eq!((budget.live, budget.peak), (0, 0));
        }
        assert!(decode(&layout_wire(audio, 0, 0, 1), &shape, &mut new_budget()).is_ok());
        error(
            &layout_wire(audio, 0, 0, 1),
            &ReplyShape::Parameters,
            ErrorKind::WrongReply,
        );
    }
}

#[test]
fn layout_raw_distinctions_survive_before_any_lossy_projection() {
    let bytes = br#"{"CheckedAudioBusLayout":{"layout":{"inputs":[],"outputs":[{"channel_count":2,"is_main":true,"active":true}]}}}"#;
    let HostResponse::CheckedAudioBusLayout { layout: original } =
        decode(bytes, &ReplyShape::CheckedAudioBusLayout, &mut new_budget()).unwrap()
    else {
        panic!("layout")
    };
    for changed in [
        std::str::from_utf8(bytes)
            .unwrap()
            .replace("\"is_main\":true", "\"is_main\":false"),
        std::str::from_utf8(bytes)
            .unwrap()
            .replace("\"active\":true", "\"active\":null"),
        std::str::from_utf8(bytes)
            .unwrap()
            .replace("\"active\":true", "\"active\":false"),
    ] {
        let HostResponse::CheckedAudioBusLayout { layout } = decode(
            changed.as_bytes(),
            &ReplyShape::CheckedAudioBusLayout,
            &mut new_budget(),
        )
        .unwrap() else {
            panic!("layout")
        };
        assert_eq!(
            original.outputs[0].channel_count,
            layout.outputs[0].channel_count
        );
        assert_ne!(
            original, layout,
            "main/aux and unknown activation cannot be discarded"
        );
    }
}

#[test]
fn layouts_reject_all_malformed_or_missing_raw_fields_before_materialization() {
    for variant in ["CheckedAudioBusLayout", "EventBusLayout"] {
        let shape = if variant == "CheckedAudioBusLayout" {
            ReplyShape::CheckedAudioBusLayout
        } else {
            ReplyShape::EventBusLayout
        };
        for bus in [
            r#"{"channel_count":2,"is_main":true}"#,
            r#"{"channel_count":2,"active":true}"#,
            r#"{"is_main":true,"active":true}"#,
            r#"{"channel_count":2,"is_main":true,"active":1}"#,
            r#"{"channel_count":2,"is_main":null,"active":true}"#,
            r#"{"channel_count":2,"is_main":"true","active":true}"#,
            r#"{"channel_count":-1,"is_main":true,"active":true}"#,
            r#"{"channel_count":1.5,"is_main":true,"active":true}"#,
            r#"{"channel_count":"2","is_main":true,"active":true}"#,
            r#"{"channel_count":2,"is_main":true,"active":true,"active":false}"#,
            r#"{"channel_count":2,"channel_count":2,"is_main":true,"active":true}"#,
            r#"{"channel_count":2,"is_main":true,"is_main":false,"active":true}"#,
            r#"{"channel_count":2,"is_main":true,"active":true,"extra":[]}"#,
            "null",
            "[]",
        ] {
            let line =
                format!("{{\"{variant}\":{{\"layout\":{{\"inputs\":[{bus}],\"outputs\":[]}}}}}}");
            let mut budget = new_budget();
            assert_eq!(
                allocation_free(|| decode(line.as_bytes(), &shape, &mut budget)).unwrap_err(),
                ErrorKind::MalformedReply
            );
            assert_eq!(budget.peak, 0);
        }
        for layout in [
            r#"{"inputs":[]}"#,
            r#"{"outputs":[]}"#,
            r#"{"inputs":[],"outputs":[],"extra":0}"#,
            r#"{"inputs":[],"inputs":[],"outputs":[]}"#,
            r#"{"inputs":[],"outputs":[],"outputs":[]}"#,
            r#"{"inputs":null,"outputs":[]}"#,
        ] {
            let line = format!("{{\"{variant}\":{{\"layout\":{layout}}}}}");
            error(line.as_bytes(), &shape, ErrorKind::MalformedReply);
        }
    }
}

#[test]
fn typed_metadata_refusals_preserve_every_reason_and_late_stop_precedence() {
    for reason in [
        MetadataQueryRejection::Unsupported,
        MetadataQueryRejection::PendingTopology,
        MetadataQueryRejection::InconsistentTopology,
    ] {
        let bytes = wire(&HostResponse::MetadataQueryRejected { reason });
        for shape in [
            ReplyShape::CheckedAudioBusLayout,
            ReplyShape::EventBusLayout,
        ] {
            let mut budget = new_budget();
            let marker = allocation_free(|| preflight(&bytes, &shape, &mut budget))
                .unwrap()
                .expect("known refusal");
            assert_eq!((budget.live, budget.peak), (0, 0));
            budget.cancel.cancelled.store(true, Ordering::Release);
            budget.deadline = Instant::now();
            let decoded = allocation_free(|| marker.materialize(&mut budget));
            assert!(matches!(decoded, DecodedReply::MetadataRejected(actual) if actual == reason));
            assert!(!budget.diagnostic_copy);
            assert_eq!((budget.live, budget.peak), (0, 0));

            // A typed finite refusal needs no diagnostic allocation even when
            // the charged scope is full or the next allocation would fail.
            let mut budget = new_budget();
            budget.reserve(MAX_PAYLOAD).unwrap();
            let _hook = budget_test_hooks::fail_next_allocation();
            assert!(matches!(
                allocation_free(|| super::decode(&bytes, &shape, &mut budget)).unwrap(),
                DecodedReply::MetadataRejected(actual) if actual == reason
            ));
            assert_eq!(budget.live, MAX_PAYLOAD);
        }
        for shape in [
            ReplyShape::Success,
            ReplyShape::Parameters,
            ReplyShape::ResetOriginSupport,
            ReplyShape::NativeDirtyRevision,
        ] {
            error(&bytes, &shape, ErrorKind::WrongReply);
        }
    }
}

#[test]
fn metadata_refusal_cannot_latch_until_the_complete_bounded_reply_is_validated() {
    let good = br#"{"MetadataQueryRejected":{"reason":"PendingTopology"}}"#;
    let shape = ReplyShape::CheckedAudioBusLayout;
    let checks = {
        let _hook = budget_test_hooks::cancel_at_check(usize::MAX);
        assert!(preflight(good, &shape, &mut new_budget())
            .unwrap()
            .is_some());
        budget_test_hooks::checks()
    };
    for check in [1, checks] {
        let mut budget = new_budget();
        let _hook = budget_test_hooks::cancel_at_check(check);
        assert_eq!(
            allocation_free(|| super::decode(good, &shape, &mut budget)).unwrap_err(),
            ErrorKind::Cancelled
        );
        assert_eq!(budget.peak, 0);
    }
    {
        let mut budget = new_budget();
        let _hook = budget_test_hooks::expire_at_check(checks);
        assert_eq!(
            allocation_free(|| super::decode(good, &shape, &mut budget)).unwrap_err(),
            ErrorKind::Deadline
        );
    }
    for malformed in [
        br#"{"MetadataQueryRejected":{"reason":"FutureReason"}}"#.as_slice(),
        br#"{"MetadataQueryRejected":{"reason":null}}"#.as_slice(),
        br#"{"MetadataQueryRejected":{"reason":{"Unsupported":null}}}"#.as_slice(),
        br#"{"MetadataQueryRejected":{"reason":"PendingTopology","reason":"Unsupported"}}"#
            .as_slice(),
        br#"{"MetadataQueryRejected":{"reason":"PendingTopology","other":0}}"#.as_slice(),
        br#"{"MetadataQueryRejected":{}}"#.as_slice(),
        br#"{"MetadataQueryRejected":{"reason":"PendingTopology"},"Error":{"message":"no"}}"#
            .as_slice(),
        br#"{"MetadataQueryRejected":{"reason":"PendingTopology"}}x"#.as_slice(),
        br#"{"MetadataQueryRejected":{"reason":"PendingTopology"}"#.as_slice(),
    ] {
        let mut budget = new_budget();
        assert_eq!(
            allocation_free(|| super::decode(malformed, &shape, &mut budget)).unwrap_err(),
            ErrorKind::MalformedReply
        );
        assert_eq!((budget.live, budget.peak), (0, 0));
    }
    let escaped = br#"{"MetadataQueryRejected":{"reason":"\u0055nsupported"}}"#;
    assert!(matches!(
        super::decode(escaped, &shape, &mut new_budget()).unwrap(),
        DecodedReply::MetadataRejected(MetadataQueryRejection::Unsupported)
    ));
}

#[test]
fn ordinary_error_and_crash_remain_distinct_from_metadata_refusal() {
    for shape in [
        ReplyShape::Parameters,
        ReplyShape::CheckedAudioBusLayout,
        ReplyShape::EventBusLayout,
    ] {
        for crashed in [false, true] {
            let source = if crashed {
                HostResponse::Crashed {
                    message: "remote crash".into(),
                }
            } else {
                HostResponse::Error {
                    message: "operation error".into(),
                }
            };
            let line = wire(&source);
            let mut budget = new_budget();
            let marker = preflight(&line, &shape, &mut budget).unwrap().unwrap();
            budget.cancel.cancelled.store(true, Ordering::Release);
            let reply = marker.materialize(&mut budget);
            let DecodedReply::KnownFailure {
                crashed: actual,
                reported: Some(reported),
                diagnostic_unavailable: None,
                metadata_getter: None,
            } = reply
            else {
                panic!("ordinary known failure")
            };
            assert_eq!(actual, crashed);
            assert_eq!(wire(&reported), line);
        }
    }
}

#[test]
fn catalog_and_layout_stop_checks_cover_preflight_materialization_and_final_return() {
    let params = (0..64)
        .map(|id| {
            let mut p = catalog_parameter(id);
            p.name = "🦀".repeat(256);
            p
        })
        .collect();
    for (line, shape) in [
        (
            wire(&HostResponse::Parameters { params }),
            ReplyShape::Parameters,
        ),
        (
            layout_wire(true, 32, 32, 256),
            ReplyShape::CheckedAudioBusLayout,
        ),
        (layout_wire(false, 32, 32, 16), ReplyShape::EventBusLayout),
    ] {
        let preflight_checks = {
            let _hook = budget_test_hooks::cancel_at_check(usize::MAX);
            let mut budget = new_budget();
            assert!(allocation_free(|| preflight(&line, &shape, &mut budget))
                .unwrap()
                .is_none());
            budget_test_hooks::checks()
        };
        let total_checks = {
            let _hook = budget_test_hooks::cancel_at_check(usize::MAX);
            decode(&line, &shape, &mut new_budget()).unwrap();
            budget_test_hooks::checks()
        };
        for check in [
            1,
            preflight_checks / 2,
            preflight_checks - 1,
            preflight_checks,
            preflight_checks + 1,
            total_checks - 1,
            total_checks,
        ] {
            for deadline in [false, true] {
                let mut budget = new_budget();
                let _hook = if deadline {
                    budget_test_hooks::expire_at_check(check)
                } else {
                    budget_test_hooks::cancel_at_check(check)
                };
                let expected = if deadline {
                    ErrorKind::Deadline
                } else {
                    ErrorKind::Cancelled
                };
                assert_eq!(decode(&line, &shape, &mut budget).unwrap_err(), expected);
                assert_eq!(budget.live, 0);
                if check <= preflight_checks {
                    assert_eq!(budget.peak, 0);
                }
            }
        }
        let mut budget = new_budget();
        let _hook = budget_test_hooks::fail_next_allocation();
        assert_eq!(
            decode(&line, &shape, &mut budget).unwrap_err(),
            ErrorKind::Allocation
        );
        assert_eq!(budget.live, 0);
    }
}

#[test]
fn scalar_readback_retains_raw_f64_at_captured_f32_boundaries() {
    let captured = 0.5f32;
    let next = f32::from_bits(captured.to_bits() + 1);
    let midpoint = (f64::from(captured) + f64::from(next)) / 2.0;
    for raw in [
        -0.0,
        f64::from_bits(1),
        f64::from(captured),
        f64::from_bits(midpoint.to_bits() - 1),
        midpoint,
        f64::from_bits(midpoint.to_bits() + 1),
        f64::from(next),
        1.0,
        -f64::from_bits(1),
        1.0 + f64::EPSILON,
    ] {
        let bytes = wire(&HostResponse::ParameterValue { value: raw });
        let HostResponse::ParameterValue { value } =
            decode(&bytes, &ReplyShape::ParameterValue, &mut new_budget()).unwrap()
        else {
            panic!("raw readback")
        };
        assert_eq!(
            value.to_bits(),
            raw.to_bits(),
            "codec must not clamp or project raw readback"
        );
    }
    assert_eq!(
        (f64::from_bits(midpoint.to_bits() - 1) as f32).to_bits(),
        captured.to_bits()
    );
    assert_eq!((midpoint as f32).to_bits(), captured.to_bits());
    assert_eq!(
        (f64::from_bits(midpoint.to_bits() + 1) as f32).to_bits(),
        next.to_bits()
    );
    assert_eq!(
        -0.0f32, 0.0f32,
        "caller base matching uses numeric equality"
    );
    // A negative raw value can project to signed zero but still fails the
    // caller's required finite [0,1] guard before its deliberate f32 projection.
    let below_zero = -f64::from_bits(1);
    assert_eq!(below_zero as f32, 0.0f32);
    assert!(!(0.0..=1.0).contains(&below_zero));
}

fn silent_transport() -> ProcessTransport {
    ProcessTransport {
        sample_position: -128,
        quarter_note_position: -0.25,
        tempo: 123.5,
        playing: true,
        time_sig_numerator: 7,
        time_sig_denominator: 8,
    }
}

#[test]
fn silent_generator_view_is_allocation_free_and_repeats_one_immutable_positive_zero_channel() {
    let mut first_pointer = None;
    for count in 0..=2 {
        allocation_free(|| {
            let RequestView::Process { inputs, frames, .. } =
                RequestView::silent_generator(count, silent_transport())
            else {
                panic!("flat Process view")
            };
            assert_eq!(frames, 128);
            assert_eq!(inputs.channel_count(), count);
            assert_eq!(inputs.iter().len(), count);
            for channel in inputs.iter() {
                assert_eq!(channel.len(), 128);
                assert!(channel.iter().all(|sample| sample.to_bits() == 0));
                assert_eq!(
                    *first_pointer.get_or_insert(channel.as_ptr()),
                    channel.as_ptr()
                );
            }
        });
    }
    let samples = vec![vec![0.25; 128], vec![-0.5; 128]];
    allocation_free(|| {
        let borrowed = FlatInputView::Borrowed(&samples);
        assert_eq!(borrowed.channel_count(), 2);
        for (channel, original) in borrowed.iter().zip(&samples) {
            assert_eq!(channel.as_ptr(), original.as_ptr());
        }
    });
}

#[test]
fn silent_generator_wire_is_exact_q128_zero_and_matches_existing_flat_encoding() {
    use crate::internal::native_edit_transport::tests::measure_allocations;
    for input_channels in 0..=2 {
        for output_channels in 1..=2 {
            let request = RequestView::silent_generator(input_channels, silent_transport());
            let shape = ReplyShape::Audio {
                channels: output_channels,
                frames: 128,
            };
            let mut budget = new_budget();
            let (encoded, allocations) =
                measure_allocations(|| encode_view(&request, &shape, &mut budget));
            let encoded = encoded.unwrap();
            assert_eq!(
                allocations.allocations, 1,
                "only the charged wire Vec may allocate"
            );
            assert_eq!(allocations.allocated_bytes, encoded.capacity());
            assert_eq!(allocations.deallocations, 0);
            assert_eq!(budget.live, encoded.capacity());
            assert_eq!(budget.peak, encoded.capacity());
            let legacy = HostCommand::Process {
                inputs: vec![vec![0.0; 128]; input_channels],
                frames: 128,
                transport: Some(silent_transport()),
            };
            assert_eq!(encoded, encode(&legacy, &shape, &mut new_budget()).unwrap());
            let HostCommand::Process {
                inputs,
                frames,
                transport,
            } = serde_json::from_slice(&encoded).unwrap()
            else {
                panic!("unchanged Process wire")
            };
            assert_eq!(inputs.len(), input_channels);
            assert_eq!(frames, 128);
            assert_eq!(transport, Some(silent_transport()));
            for channel in inputs {
                assert_eq!(channel.len(), 128);
                assert!(channel.iter().all(|sample| sample.to_bits() == 0));
            }
            let charge = encoded.capacity();
            drop(encoded);
            budget.release(charge);
            assert_eq!(budget.live, 0);
        }
    }
}

#[test]
fn silent_generator_view_rejects_bounds_after_budget_check_without_allocation() {
    for (input_channels, frames) in [(3, 128), (usize::MAX, 128), (0, 127), (2, 129)] {
        let request = RequestView::Process {
            inputs: FlatInputView::SilentQ128(input_channels),
            frames,
            transport: Some(silent_transport()),
        };
        let shape = ReplyShape::Audio {
            channels: 2,
            frames: frames as usize,
        };
        for stop in 0..4 {
            let mut budget = new_budget();
            if stop == 1 || stop == 3 {
                budget.deadline = Instant::now();
            }
            if stop == 2 || stop == 3 {
                budget.cancel.cancelled.store(true, Ordering::Release);
            }
            let expected = match stop {
                1 => ErrorKind::Deadline,
                2 | 3 => ErrorKind::Cancelled,
                _ => ErrorKind::Shape,
            };
            assert_eq!(
                allocation_free(|| encode_view(&request, &shape, &mut budget)).unwrap_err(),
                expected,
            );
            assert_eq!((budget.live, budget.peak), (0, 0));
        }
    }
}

#[test]
fn silent_generator_preserves_every_codec_checkpoint_and_payload_accounting() {
    let request = RequestView::silent_generator(2, silent_transport());
    let shape = ReplyShape::Audio {
        channels: 2,
        frames: 128,
    };
    let checks = {
        let _hook = budget_test_hooks::cancel_at_check(usize::MAX);
        encode_view(&request, &shape, &mut new_budget()).unwrap();
        budget_test_hooks::checks()
    };
    // Two channels cross the existing Writer checkpoint in both count and emit,
    // in addition to initial, allocation and final checks.
    assert!(checks >= 6);
    for at in 1..=checks {
        for expired in [false, true] {
            let mut budget = new_budget();
            let _hook = if expired {
                budget_test_hooks::expire_at_check(at)
            } else {
                budget_test_hooks::cancel_at_check(at)
            };
            assert_eq!(
                encode_view(&request, &shape, &mut budget).unwrap_err(),
                if expired {
                    ErrorKind::Deadline
                } else {
                    ErrorKind::Cancelled
                },
            );
            assert_eq!(budget.live, 0);
        }
    }
    let mut budget = new_budget();
    budget.live = MAX_PAYLOAD;
    budget.peak = MAX_PAYLOAD;
    assert_eq!(
        allocation_free(|| encode_view(&request, &shape, &mut budget)).unwrap_err(),
        ErrorKind::Limit,
    );
    assert_eq!((budget.live, budget.peak), (MAX_PAYLOAD, MAX_PAYLOAD));
    let mut budget = new_budget();
    let _hook = budget_test_hooks::fail_next_allocation();
    assert_eq!(
        allocation_free(|| encode_view(&request, &shape, &mut budget)).unwrap_err(),
        ErrorKind::Allocation,
    );
    assert_eq!(budget.live, 0);
}

#[test]
fn selected_bundle_codec_complete_fields_nullable_activation_and_u32_bounds() {
    use crate::process_isolation::{SelectedMetadataPolicy, StrictMetadataSnapshot};
    assert_command(
        &HostCommand::StrictMetadataSnapshot(crate::process_isolation::SelectedMetadataRequest {
            policy: SelectedMetadataPolicy::Kontakt841SelectedStereo,
        }),
        &ReplyShape::StrictMetadataSnapshot,
    );
    let source = HostResponse::StrictMetadataSnapshot {
        snapshot: StrictMetadataSnapshot {
            audio: CheckedAudioBusLayout {
                inputs: vec![
                    CheckedAudioBusConfig {
                        channel_count: 256,
                        is_main: false,
                        active: None
                    };
                    32
                ],
                outputs: vec![
                    CheckedAudioBusConfig {
                        channel_count: 2,
                        is_main: true,
                        active: Some(false)
                    };
                    32
                ],
            },
            events: EventBusLayout {
                inputs: vec![
                    EventBusConfig {
                        channel_count: 16,
                        is_main: true,
                        active: Some(true)
                    };
                    32
                ],
                outputs: vec![
                    EventBusConfig {
                        channel_count: 1,
                        is_main: false,
                        active: None
                    };
                    32
                ],
            },
            latency_samples: 0,
            tail_samples: u32::MAX,
        },
    };
    let bytes = wire(&source);
    assert!(bytes.len() < 8192);
    assert_eq!(
        wire(
            &decode(
                &bytes,
                &ReplyShape::StrictMetadataSnapshot,
                &mut new_budget()
            )
            .unwrap()
        ),
        bytes
    );
}

#[test]
fn selected_bundle_codec_closed_schema_limits_and_wrong_reply() {
    let valid = br#"{"StrictMetadataSnapshot":{"snapshot":{"audio":{"inputs":[],"outputs":[]},"events":{"inputs":[],"outputs":[]},"latency_samples":0,"tail_samples":4294967295}}}"#;
    assert!(decode(
        valid,
        &ReplyShape::StrictMetadataSnapshot,
        &mut new_budget()
    )
    .is_ok());
    for (find, replacement) in [
        ("\"tail_samples\":4294967295", "\"tail_samples\":4294967296"),
        ("\"latency_samples\":0,", ""),
        (
            "\"latency_samples\":0",
            "\"latency_samples\":0,\"latency_samples\":1",
        ),
        (
            "\"latency_samples\":0",
            "\"latency_samples\":0,\"cached\":true",
        ),
        (
            "\"inputs\":[]",
            "\"inputs\":[{\"channel_count\":0,\"is_main\":true,\"active\":true}]",
        ),
    ] {
        let changed = String::from_utf8(valid.to_vec())
            .unwrap()
            .replacen(find, replacement, 1);
        assert!(
            decode(
                changed.as_bytes(),
                &ReplyShape::StrictMetadataSnapshot,
                &mut new_budget()
            )
            .is_err(),
            "{changed}"
        );
    }
    error(
        br#"{"StrictMetadataAdmissionRejected":{"stage":"unknown"}}"#,
        &ReplyShape::StrictMetadataSnapshot,
        ErrorKind::MalformedReply,
    );
    error(
        br#"{"StrictMetadataRejected":{"getter":"TailSamples","reason":"unknown"}}"#,
        &ReplyShape::StrictMetadataSnapshot,
        ErrorKind::MalformedReply,
    );
    error(
        br#"{"StrictMetadataFailed":{"getter":"unknown","message":"e"}}"#,
        &ReplyShape::StrictMetadataSnapshot,
        ErrorKind::MalformedReply,
    );
    error(
        br#"{"StrictMetadataFailed":{"getter":"TailSamples","message":"e","extra":0}}"#,
        &ReplyShape::StrictMetadataSnapshot,
        ErrorKind::MalformedReply,
    );
    error(
        br#"{"LatencySamples":{"samples":0}}"#,
        &ReplyShape::StrictMetadataSnapshot,
        ErrorKind::WrongReply,
    );
    error(valid, &ReplyShape::EventBusLayout, ErrorKind::WrongReply);
    let buses = vec![
        CheckedAudioBusConfig {
            channel_count: 2,
            is_main: true,
            active: Some(true)
        };
        33
    ];
    let source = HostResponse::StrictMetadataSnapshot {
        snapshot: crate::process_isolation::StrictMetadataSnapshot {
            audio: CheckedAudioBusLayout {
                inputs: Vec::new(),
                outputs: buses,
            },
            events: EventBusLayout {
                inputs: Vec::new(),
                outputs: Vec::new(),
            },
            latency_samples: 0,
            tail_samples: 0,
        },
    };
    error(
        &wire(&source),
        &ReplyShape::StrictMetadataSnapshot,
        ErrorKind::Limit,
    );
}

#[test]
fn selected_bundle_codec_known_failure_keeps_actual_getter_when_copy_fails() {
    for getter in [
        MetadataGetter::CheckedAudioBusLayout,
        MetadataGetter::EventBusLayout,
        MetadataGetter::LatencySamples,
        MetadataGetter::TailSamples,
    ] {
        let source = HostResponse::StrictMetadataFailed {
            getter,
            message: "actual failure".into(),
        };
        let bytes = wire(&source);
        let mut budget = new_budget();
        let marker =
            allocation_free(|| preflight(&bytes, &ReplyShape::StrictMetadataSnapshot, &mut budget))
                .unwrap()
                .unwrap();
        budget.cancel.cancelled.store(true, Ordering::Release);
        let _hook = budget_test_hooks::fail_next_allocation();
        assert!(
            matches!(marker.materialize(&mut budget), DecodedReply::KnownFailure { crashed: false, reported: None, diagnostic_unavailable: Some(ErrorKind::Allocation), metadata_getter: Some(actual) } if actual == getter)
        );
    }
}

#[test]
fn selected_bundle_codec_snapshot_stop_fences_cover_both_passes() {
    let bytes = wire(&HostResponse::StrictMetadataSnapshot {
        snapshot: crate::process_isolation::StrictMetadataSnapshot {
            audio: CheckedAudioBusLayout {
                inputs: Vec::new(),
                outputs: vec![
                    CheckedAudioBusConfig {
                        channel_count: 2,
                        is_main: true,
                        active: Some(true)
                    };
                    32
                ],
            },
            events: EventBusLayout {
                inputs: vec![EventBusConfig {
                    channel_count: 16,
                    is_main: true,
                    active: Some(true),
                }],
                outputs: Vec::new(),
            },
            latency_samples: 0,
            tail_samples: u32::MAX,
        },
    });
    let checks = {
        let _hook = budget_test_hooks::cancel_at_check(usize::MAX);
        decode(
            &bytes,
            &ReplyShape::StrictMetadataSnapshot,
            &mut new_budget(),
        )
        .unwrap();
        budget_test_hooks::checks()
    };
    for at in [2, checks / 2, checks] {
        let _hook = budget_test_hooks::cancel_at_check(at);
        assert_eq!(
            decode(
                &bytes,
                &ReplyShape::StrictMetadataSnapshot,
                &mut new_budget()
            )
            .unwrap_err(),
            ErrorKind::Cancelled
        );
    }
    let _hook = budget_test_hooks::fail_next_allocation();
    assert_eq!(
        decode(
            &bytes,
            &ReplyShape::StrictMetadataSnapshot,
            &mut new_budget()
        )
        .unwrap_err(),
        ErrorKind::Allocation
    );
}

// Construct raw JSON rather than serde maps so duplicate and escaped keys
// reach the actual closed parser without being coalesced by a fixture.
fn selected_bundle_invalid_failure_schema_matrix() -> Vec<String> {
    fn object(variant: &str, fields: &[(&str, &str)]) -> String {
        let body = fields
            .iter()
            .map(|(key, value)| format!(r#""{key}":{value}"#))
            .collect::<Vec<_>>()
            .join(",");
        format!(r#"{{"{variant}":{{{body}}}}}"#)
    }
    let mut wires = Vec::new();
    for (variant, key, escaped, value, sibling) in [
        (
            "StrictMetadataFailed",
            "getter",
            r"\u0067etter",
            r#""TailSamples""#,
            Some(("message", r#""actual failure""#)),
        ),
        (
            "StrictMetadataFailed",
            "message",
            r"\u006dessage",
            r#""actual failure""#,
            Some(("getter", r#""TailSamples""#)),
        ),
        (
            "StrictMetadataRejected",
            "getter",
            r"\u0067etter",
            r#""EventBusLayout""#,
            Some(("reason", r#""Unsupported""#)),
        ),
        (
            "StrictMetadataRejected",
            "reason",
            r"\u0072eason",
            r#""Unsupported""#,
            Some(("getter", r#""EventBusLayout""#)),
        ),
        (
            "StrictMetadataAdmissionRejected",
            "stage",
            r"\u0073tage",
            r#""EventInput""#,
            None,
        ),
    ] {
        let prefix: Vec<_> = sibling.into_iter().collect();
        wires.push(object(variant, &prefix)); // required field missing
        for (first, second) in [
            (key, key),
            (key, escaped),
            (escaped, key),
            (escaped, escaped),
        ] {
            let mut fields = prefix.clone();
            fields.extend([(first, value), (second, value)]);
            wires.push(object(variant, &fields));
        }
        for wrong in ["null", "0", "true", "{}", "[]"] {
            let mut fields = prefix.clone();
            fields.push((key, wrong));
            wires.push(object(variant, &fields));
        }
        let mut fields = prefix;
        fields.extend([(key, value), ("unexpected", "0")]);
        wires.push(object(variant, &fields));
    }
    wires.extend([
        r#"{"StrictMetadataFailed":{"getter":"unknown","message":"actual failure"}}"#.to_owned(),
        r#"{"StrictMetadataRejected":{"getter":"unknown","reason":"Unsupported"}}"#.to_owned(),
        r#"{"StrictMetadataRejected":{"getter":"EventBusLayout","reason":"unknown"}}"#.to_owned(),
        r#"{"StrictMetadataAdmissionRejected":{"stage":"unknown"}}"#.to_owned(),
    ]);
    assert_eq!(wires.len(), 59);
    wires
}

#[test]
fn selected_bundle_codec_failure_field_matrix_has_no_validated_inner_attribution() {
    for wire in selected_bundle_invalid_failure_schema_matrix() {
        let mut budget = new_budget();
        budget.reserve(17).unwrap();
        // A partially parsed getter is never a KnownOperationFailure marker.
        assert!(
            matches!(
                preflight(
                    wire.as_bytes(),
                    &ReplyShape::StrictMetadataSnapshot,
                    &mut budget
                ),
                Err(ErrorKind::MalformedReply)
            ),
            "{wire}"
        );
        assert_eq!(budget.live, 17);
        match super::decode(
            wire.as_bytes(),
            &ReplyShape::StrictMetadataSnapshot,
            &mut budget,
        ) {
            Err(cause) => assert_eq!(cause, ErrorKind::MalformedReply, "{wire}"),
            Ok(_) => panic!("malformed failure produced response or attribution: {wire}"),
        }
        assert_eq!(budget.live, 17);
    }
}

#[test]
fn selected_bundle_codec_unique_escaped_failure_fields_remain_valid() {
    for wire in [
        r#"{"StrictMetadataFailed":{"\u0067etter":"Tail\u0053amples","\u006dessage":"actual failure"}}"#,
        r#"{"StrictMetadataRejected":{"\u0067etter":"EventBusLayout","\u0072eason":"Unsupported"}}"#,
        r#"{"StrictMetadataAdmissionRejected":{"\u0073tage":"EventInput"}}"#,
    ] {
        assert!(
            decode(
                wire.as_bytes(),
                &ReplyShape::StrictMetadataSnapshot,
                &mut new_budget()
            )
            .is_ok(),
            "{wire}"
        );
    }
}
