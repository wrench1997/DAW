//! Additive, count-only reset-origin wire-contract regressions. No plugin or device needed.
#![cfg(feature = "vst3")]

use vst3_host::process_isolation::{HostCommand, HostResponse};
use vst3_host::{ProcessTransport, ResetOriginReport, ResetOriginSupport};

fn transport() -> ProcessTransport {
    ProcessTransport {
        sample_position: -96_000,
        quarter_note_position: 19.125,
        tempo: 135.5,
        playing: false,
        time_sig_numerator: 5,
        time_sig_denominator: 8,
    }
}

#[test]
fn reset_support_is_a_distinct_additive_capability_query() {
    assert_eq!(
        serde_json::to_string(&HostCommand::ResetOriginSupport).unwrap(),
        r#""ResetOriginSupport""#
    );
    assert!(matches!(
        serde_json::from_str::<HostCommand>(r#""ResetOriginSupport""#).unwrap(),
        HostCommand::ResetOriginSupport
    ));
    // The pre-existing state, MIDI, and processing command names remain unchanged.
    for (command, expected) in [
        (HostCommand::SaveState, r#""SaveState""#),
        (HostCommand::MidiPanic, r#""MidiPanic""#),
        (HostCommand::StartProcessing, r#""StartProcessing""#),
        (HostCommand::StopProcessing, r#""StopProcessing""#),
    ] {
        assert_eq!(serde_json::to_string(&command).unwrap(), expected);
    }
}

#[test]
fn reset_command_round_trips_small_standalone_blocks_and_explicit_transport() {
    for frames in [17, 47, 128] {
        let request = HostCommand::ProcessResetOrigin {
            frames,
            transport: transport(),
        };
        let value = serde_json::to_value(&request).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "ProcessResetOrigin": {
                    "frames": frames,
                    "transport": transport(),
                }
            })
        );
        match serde_json::from_value::<HostCommand>(value).unwrap() {
            HostCommand::ProcessResetOrigin {
                frames: actual_frames,
                transport: actual_transport,
            } => {
                assert_eq!(actual_frames, frames);
                assert_eq!(actual_transport, transport());
            }
            other => panic!("unexpected reset command: {other:?}"),
        }
    }
}

#[test]
fn reset_command_rejects_missing_transport_and_invalid_frame_types() {
    for value in [
        serde_json::json!({"ProcessResetOrigin": {"frames": 47}}),
        serde_json::json!({"ProcessResetOrigin": {"frames": 47, "transport": null}}),
        serde_json::json!({"ProcessResetOrigin": {"transport": transport()}}),
        serde_json::json!({"ProcessResetOrigin": {"frames": -1, "transport": transport()}}),
        serde_json::json!({"ProcessResetOrigin": {"frames": 47.5, "transport": transport()}}),
        serde_json::json!({"ProcessResetOrigin": {
            "frames": u64::from(u32::MAX) + 1, "transport": transport()
        }}),
    ] {
        assert!(
            serde_json::from_value::<HostCommand>(value.clone()).is_err(),
            "accepted malformed reset request: {value}"
        );
    }
}

#[test]
fn reset_support_reports_the_contract_limit_and_current_processing_state() {
    for (max_block_frames, processing) in [(17, false), (47, true), (128, true)] {
        let response = HostResponse::ResetOriginSupport {
            support: ResetOriginSupport {
                contract_version: 1,
                max_block_frames,
                processing,
            },
        };
        let value = serde_json::to_value(&response).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "ResetOriginSupport": {
                    "support": {
                        "contract_version": 1,
                        "max_block_frames": max_block_frames,
                        "processing": processing,
                    }
                }
            })
        );
        let HostResponse::ResetOriginSupport { support } = serde_json::from_value(value).unwrap()
        else {
            panic!("support response changed variant");
        };
        assert_eq!(support.contract_version, 1);
        assert_eq!(support.max_block_frames, max_block_frames);
        assert_eq!(support.processing, processing);
    }
}

#[test]
fn reset_report_round_trips_counts_and_attempt_errors_without_output_values() {
    for process_error in [
        None,
        Some("processor rejected reset-origin block".to_owned()),
    ] {
        let response = HostResponse::ResetOriginReport {
            report: ResetOriginReport {
                frames: 47,
                discarded_prior_midi_events: 3,
                discarded_midi_events: u64::MAX,
                discarded_prior_parameter_points: 5,
                discarded_parameter_points: u64::from(u32::MAX) + 1,
                output_events_lost: true,
                parameter_output_fault: true,
                process_error: process_error.clone(),
            },
        };
        let value = serde_json::to_value(&response).unwrap();
        // Exact schema: no audio buffers, MIDI events, parameter values, or opaque state.
        assert_eq!(
            value,
            serde_json::json!({
                "ResetOriginReport": {
                    "report": {
                        "frames": 47,
                        "discarded_prior_midi_events": 3,
                        "discarded_midi_events": u64::MAX,
                        "discarded_prior_parameter_points": 5,
                        "discarded_parameter_points": u64::from(u32::MAX) + 1,
                        "output_events_lost": true,
                        "parameter_output_fault": true,
                        "process_error": process_error,
                    }
                }
            })
        );
        let HostResponse::ResetOriginReport { report } =
            serde_json::from_str(&serde_json::to_string(&response).unwrap()).unwrap()
        else {
            panic!("attempted reset changed response variant");
        };
        assert_eq!(report.frames, 47);
        assert_eq!(report.discarded_prior_midi_events, 3);
        assert_eq!(report.discarded_midi_events, u64::MAX);
        assert_eq!(report.discarded_prior_parameter_points, 5);
        assert_eq!(report.discarded_parameter_points, u64::from(u32::MAX) + 1);
        assert!(report.output_events_lost);
        assert!(report.parameter_output_fault);
        assert_eq!(report.process_error, process_error);
    }
}

#[test]
fn incomplete_reset_support_and_report_cannot_claim_the_contract() {
    for value in [
        serde_json::json!({"ResetOriginSupport": {"support": {
            "max_block_frames": 47, "processing": true
        }}}),
        serde_json::json!({"ResetOriginSupport": {"support": {
            "contract_version": 1, "processing": true
        }}}),
        serde_json::json!({"ResetOriginSupport": {"support": {
            "contract_version": 1, "max_block_frames": 47
        }}}),
        serde_json::json!({"ResetOriginReport": {"report": {"frames": 47}}}),
        serde_json::json!({"ResetOriginReport": {"report": {
            "frames": 47,
            "discarded_prior_midi_events": 0,
            "discarded_midi_events": 0,
            "discarded_prior_parameter_points": 0,
            "discarded_parameter_points": 0,
            "output_events_lost": false,
            "process_error": null,
        }}}),
    ] {
        assert!(
            serde_json::from_value::<HostResponse>(value.clone()).is_err(),
            "accepted incomplete reset capability/report: {value}"
        );
    }
}
