//! Headless process-context and event-loss wire-contract regressions.
#![cfg(feature = "vst3")]

use vst3_host::ProcessTransport;
use vst3_host::process_isolation::{HostCommand, HostResponse};

#[test]
fn process_audio_and_transport_are_the_same_wire_request() {
    let transport = ProcessTransport {
        sample_position: 480_000,
        quarter_note_position: 27.125,
        tempo: 93.5,
        playing: false,
        time_sig_numerator: 7,
        time_sig_denominator: 8,
    };
    let request = HostCommand::Process {
        inputs: vec![vec![0.0; 32]],
        frames: 32,
        transport: Some(transport),
    };
    let json = serde_json::to_string(&request).unwrap();
    match serde_json::from_str::<HostCommand>(&json).unwrap() {
        HostCommand::Process {
            inputs,
            frames,
            transport: actual,
        } => {
            assert_eq!(inputs, vec![vec![0.0; 32]]);
            assert_eq!(frames, 32);
            assert_eq!(actual, Some(transport));
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn legacy_process_requests_and_audio_replies_remain_readable() {
    assert!(matches!(
        serde_json::from_str::<HostCommand>(r#"{"Process":{"inputs":[],"frames":0}}"#).unwrap(),
        HostCommand::Process {
            transport: None,
            ..
        }
    ));
    assert!(matches!(
        serde_json::from_str::<HostResponse>(
            r#"{"AudioOutput":{"outputs":[],"output_events":[]}}"#
        )
        .unwrap(),
        HostResponse::AudioOutput {
            output_events_lost: false,
            ..
        }
    ));
    assert!(matches!(
        serde_json::from_str::<HostResponse>(
            r#"{"AudioOutput":{"outputs":[],"output_events":[],"output_events_lost":true}}"#
        )
        .unwrap(),
        HostResponse::AudioOutput {
            output_events_lost: true,
            ..
        }
    ));
}
