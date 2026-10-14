//! Actual closed public client against cooperative, ledger-checking mock children.
#[path = "selected_bus_tests.rs"]
mod selected_bus_tests;
use super::*;
use crate::plugin::{CheckedAudioBusConfig, CheckedAudioBusLayout, EventBusConfig, EventBusLayout};
use crate::strict_offline::{self, Client, FailureKind};
use crate::{
    MidiChannel, MidiEvent, Parameter, ProcessMode, ProcessTransport, ResetOriginSupport,
    StateContext,
};

fn client(script: &str) -> Client {
    match Client::spawn(&mut python(script), deadline()) {
        Ok(client) => client,
        Err(mut error) => {
            if let Some(owner) = error.cleanup.as_mut() {
                let _ = owner.dispose(Instant::now(), deadline());
            }
            panic!("client spawn: {:?}", error.failure);
        }
    }
}
fn ledger(pairs: &[(HostCommand, HostResponse)], shutdown: bool) -> String {
    let json = serde_json::to_string(pairs).unwrap();
    format!("import json,sys\npairs=json.loads({json:?})\nfor expected,reply in pairs:\n actual=json.loads(sys.stdin.readline())\n assert actual==expected,(actual,expected)\n print(json.dumps(reply),flush=True)\n{}assert sys.stdin.read()==''\n",if shutdown { "assert json.loads(sys.stdin.readline())=='Shutdown'\n" }else{""})
}
fn success() -> HostResponse {
    HostResponse::Success {
        message: "ok".into(),
    }
}
fn finish_client(client: &mut Client) {
    let report = client.dispose(deadline(), deadline());
    assert!(
        matches!(report.state,strict_offline::CleanupState::Reaped(s) if s.success()),
        "{report:?}"
    );
    assert!(
        report.shutdown_failure.is_none()
            && report.kill_error.is_none()
            && report.wait_error.is_none()
            && !report.kill_requested,
        "{report:?}"
    );
}
fn transport() -> ProcessTransport {
    ProcessTransport {
        sample_position: 0,
        quarter_note_position: 0.0,
        tempo: 120.0,
        playing: true,
        time_sig_numerator: 4,
        time_sig_denominator: 4,
    }
}
fn parameter() -> Parameter {
    Parameter {
        id: 17,
        name: "base".into(),
        value: 0.5,
        min: 0.0,
        max: 1.0,
        default: 0.25,
        unit: "u".into(),
        step_count: 0,
        can_automate: true,
        is_read_only: false,
        is_bypass: false,
        flags: 0xA5,
    }
}

#[test]
fn public_client_closed_sequence_retains_every_result_and_reaps_cooperatively() {
    let _serial = serial();
    let support = ResetOriginSupport {
        contract_version: 1,
        max_block_frames: 2048,
        processing: false,
    };
    let audio = CheckedAudioBusLayout {
        inputs: vec![],
        outputs: vec![CheckedAudioBusConfig {
            channel_count: 2,
            is_main: true,
            active: Some(true),
        }],
    };
    let events = EventBusLayout {
        inputs: vec![EventBusConfig {
            channel_count: 16,
            is_main: true,
            active: Some(true),
        }],
        outputs: vec![],
    };
    let note = MidiEvent::NoteOn {
        channel: MidiChannel::Ch1,
        note: 60,
        velocity: 100,
    };
    let pairs = vec![
        (
            HostCommand::LoadPlugin {
                path: "mock.vst3".into(),
                sample_rate: 48000.0,
                block_size: 2048,
                tempo: 120.0,
                time_sig_numerator: 4,
                time_sig_denominator: 4,
                class_id: Some("0123456789abcdef0123456789abcdef".into()),
            },
            HostResponse::PluginInfo {
                vendor: "vendor".into(),
                name: "mock".into(),
                version: "1".into(),
                category: "Instrument".into(),
                uid: "0123456789abcdef0123456789abcdef".into(),
                has_gui: true,
                audio_inputs: 0,
                audio_outputs: 1,
                output_channels: 2,
                has_midi_input: true,
                has_midi_output: false,
                compatibility: vec![],
            },
        ),
        (
            HostCommand::SetProcessMode {
                mode: ProcessMode::Offline,
            },
            success(),
        ),
        (
            HostCommand::ResetOriginSupport,
            HostResponse::ResetOriginSupport { support },
        ),
        (
            HostCommand::LoadState {
                data: vec![1, 2, 3],
                context: StateContext::Project,
            },
            success(),
        ),
        (HostCommand::StartProcessing, success()),
        (
            HostCommand::NativeDirtyRevision,
            HostResponse::NativeDirtyRevision { revision: u64::MAX },
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
        (
            HostCommand::CheckedAudioBusLayout,
            HostResponse::CheckedAudioBusLayout {
                layout: audio.clone(),
            },
        ),
        (
            HostCommand::EventBusLayout,
            HostResponse::EventBusLayout {
                layout: events.clone(),
            },
        ),
        (
            HostCommand::LatencySamples,
            HostResponse::LatencySamples { samples: 0 },
        ),
        (
            HostCommand::TailSamples,
            HostResponse::TailSamples { samples: u32::MAX },
        ),
        (
            HostCommand::SendMidiAt {
                event: note,
                sample_offset: 127,
            },
            success(),
        ),
        (
            HostCommand::Process {
                inputs: vec![],
                frames: 128,
                transport: Some(transport()),
            },
            HostResponse::AudioOutput {
                outputs: vec![vec![0.25; 128], vec![-0.5; 128]],
                output_events: vec![],
                output_events_lost: false,
                transport_applied: true,
            },
        ),
        (HostCommand::StopProcessing, success()),
    ];
    let mut c = client(&ledger(&pairs, true));
    let info = c
        .load_plugin(
            strict_offline::LoadSpec {
                path: "mock.vst3",
                sample_rate: 48000.0,
                tempo: 120.0,
                time_sig_numerator: 4,
                time_sig_denominator: 4,
                class_id: "0123456789abcdef0123456789abcdef",
            },
            deadline(),
        )
        .unwrap();
    assert_eq!(
        (
            info.name.as_str(),
            info.uid.as_str(),
            info.version.as_str(),
            info.category.as_str()
        ),
        (
            "mock",
            "0123456789abcdef0123456789abcdef",
            "1",
            "Instrument"
        )
    );
    assert_eq!(
        (info.audio_inputs, info.audio_outputs, info.output_channels),
        (0, 1, 2)
    );
    c.set_offline(deadline()).unwrap();
    assert_eq!(c.reset_origin_support(deadline()).unwrap(), support);
    let bytes = vec![1, 2, 3];
    let ptr = bytes.as_ptr();
    c.load_state(&bytes, deadline()).unwrap();
    assert_eq!(bytes.as_ptr(), ptr);
    assert_eq!(bytes, [1, 2, 3]);
    c.start_processing(deadline()).unwrap();
    assert_eq!(c.native_dirty_revision(deadline()).unwrap(), u64::MAX);
    assert_eq!(
        serde_json::to_string(&c.parameters(deadline()).unwrap()).unwrap(),
        serde_json::to_string(&vec![parameter()]).unwrap()
    );
    c.set_parameter(17, 0.5, deadline()).unwrap();
    assert!(strict_offline::captured_base_matches(
        c.parameter(17, deadline()).unwrap(),
        0.5
    ));
    assert_eq!(c.checked_audio_layout(deadline()).unwrap(), audio);
    assert_eq!(c.event_layout(deadline()).unwrap(), events);
    assert_eq!(c.latency_samples(deadline()).unwrap(), 0);
    assert_eq!(c.tail_samples(deadline()).unwrap(), u32::MAX);
    c.send_note(note, 127, deadline()).unwrap();
    let out = c.process(2, transport(), deadline()).unwrap();
    assert_eq!(out.outputs, vec![vec![0.25; 128], vec![-0.5; 128]]);
    assert_eq!(out.output_event_count, 0);
    assert!(!out.output_events_lost);
    assert!(out.transport_applied);
    c.stop_processing(deadline()).unwrap();
    finish_client(&mut c);
    assert!(
        matches!(c.continue_cleanup().unwrap().state,strict_offline::CleanupState::Reaped(s) if s.success())
    );
}

#[test]
fn raw_layout_distinctions_and_typed_refusals_are_never_projected_away() {
    let _serial = serial();
    for (main, active) in [
        (true, Some(true)),
        (false, Some(true)),
        (true, None),
        (true, Some(false)),
    ] {
        let expected = CheckedAudioBusLayout {
            inputs: vec![],
            outputs: vec![CheckedAudioBusConfig {
                channel_count: 2,
                is_main: main,
                active,
            }],
        };
        let mut c = client(&ledger(
            &[(
                HostCommand::CheckedAudioBusLayout,
                HostResponse::CheckedAudioBusLayout {
                    layout: expected.clone(),
                },
            )],
            true,
        ));
        assert_eq!(c.checked_audio_layout(deadline()).unwrap(), expected);
        finish_client(&mut c);
    }
    for reason in [
        crate::MetadataQueryRejection::Unsupported,
        crate::MetadataQueryRejection::PendingTopology,
        crate::MetadataQueryRejection::InconsistentTopology,
    ] {
        let mut c = client(&ledger(
            &[(
                HostCommand::EventBusLayout,
                HostResponse::MetadataQueryRejected { reason },
            )],
            false,
        ));
        let error = c.event_layout(deadline()).unwrap_err();
        assert_eq!(error.kind, FailureKind::MetadataRejected(reason));
        assert_eq!(error.delivery, strict_offline::Delivery::MayHaveApplied);
        assert!(c.start_processing(deadline()).is_err());
        finish_client(&mut c);
    }
}

#[test]
fn captured_base_projection_is_exact_f32_numeric_equality_without_clamp() {
    for value in [0.0, -0.0, 0.5, 1.0, f32::from_bits(1), 0.12345679] {
        assert!(strict_offline::captured_base_matches(
            f64::from(value),
            value
        ));
    }
    assert!(strict_offline::captured_base_matches(-0.0, 0.0));
    assert!(strict_offline::captured_base_matches(
        0.5 + f64::EPSILON,
        0.5
    )); // Same explicit f32 projection.
    for value in [
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        -f64::EPSILON,
        1.0 + f64::EPSILON,
        0.500001,
    ] {
        assert!(!strict_offline::captured_base_matches(value, 0.5));
    }
    assert!(!strict_offline::captured_base_matches(0.5, f32::NAN));
}

#[test]
fn closed_client_invalid_notes_and_shapes_send_nothing_and_seal() {
    let _serial = serial();
    for (event, offset) in [
        (
            MidiEvent::ControlChange {
                channel: MidiChannel::Ch1,
                controller: 1,
                value: 2,
            },
            0,
        ),
        (
            MidiEvent::NoteOn {
                channel: MidiChannel::Ch1,
                note: 128,
                velocity: 1,
            },
            0,
        ),
        (
            MidiEvent::NoteOff {
                channel: MidiChannel::Ch1,
                note: 60,
                velocity: 128,
            },
            0,
        ),
        (
            MidiEvent::NoteOn {
                channel: MidiChannel::Ch1,
                note: 60,
                velocity: 1,
            },
            128,
        ),
    ] {
        let mut c = client(&ledger(&[], false));
        let failure = c.send_note(event, offset, deadline()).unwrap_err();
        assert_eq!(failure.delivery, strict_offline::Delivery::NotSent);
        assert!(c.latency_samples(deadline()).is_err());
        finish_client(&mut c);
    }
    for channels in [0, 3, usize::MAX] {
        let mut c = client(&ledger(&[], false));
        let failure = c.process(channels, transport(), deadline()).unwrap_err();
        assert_eq!(failure.kind, FailureKind::Shape);
        assert_eq!(failure.delivery, strict_offline::Delivery::NotSent);
        finish_client(&mut c);
    }
}

#[test]
fn public_borrowed_large_state_expiry_and_cancellation_allocate_no_payload_copy() {
    let _serial = serial();
    use crate::internal::native_edit_transport::tests::measure_allocations;
    let data = vec![0xA5; 64 * 1024 * 1024];
    let pointer = data.as_ptr();
    for cancelled in [false, true] {
        let mut c = client(&ledger(&[], false));
        if cancelled {
            c.cancellation().cancel();
        }
        let end = if cancelled {
            deadline()
        } else {
            Instant::now()
        };
        let (result, allocations) = measure_allocations(|| c.load_state(&data, end));
        assert_eq!(
            allocations.allocations, 0,
            "preflight must not clone opaque caller payload"
        );
        let failure = result.unwrap_err();
        assert_eq!(failure.delivery, strict_offline::Delivery::NotSent);
        assert_eq!(
            failure.kind,
            if cancelled {
                FailureKind::Cancelled
            } else {
                FailureKind::Deadline
            }
        );
        assert_eq!(data.as_ptr(), pointer);
        assert!(data.iter().all(|&b| b == 0xA5));
        finish_client(&mut c);
    }
}

#[test]
fn cancelled_stop_and_shutdown_keep_categories_and_still_reap() {
    let _serial = serial();
    // Cancellation before Stop seals without sending Stop or Shutdown.
    let mut c = client(&ledger(&[], false));
    c.cancellation().cancel();
    let failure = c.stop_processing(deadline()).unwrap_err();
    assert_eq!(failure.kind, FailureKind::Cancelled);
    assert_eq!(failure.delivery, strict_offline::Delivery::NotSent);
    finish_client(&mut c);
    // Cancellation after successful Stop makes the single Shutdown attempt NotSent.
    let mut c = client(&ledger(&[(HostCommand::StopProcessing, success())], false));
    c.stop_processing(deadline()).unwrap();
    c.cancellation().cancel();
    let report = c.dispose(deadline(), deadline());
    let failure = report.shutdown_failure.unwrap();
    assert_eq!(failure.kind, FailureKind::Cancelled);
    assert_eq!(failure.delivery, strict_offline::Delivery::NotSent);
    assert!(matches!(report.state,strict_offline::CleanupState::Reaped(s) if s.success()));
    assert!(!report.kill_requested);
    let cached = c.continue_cleanup().unwrap();
    assert_eq!(
        cached.shutdown_failure.unwrap().kind,
        FailureKind::Cancelled
    );
}

#[test]
fn public_spawn_setup_failure_and_abandonment_keep_reserved_owner() {
    let _serial = serial();
    pump::test_hooks::arm(pump::test_hooks::Fault::Setup);
    let mut failed = match Client::spawn(
        &mut python("import sys\nassert sys.stdin.read()==''\n"),
        deadline(),
    ) {
        Err(f) => f,
        Ok(_) => panic!("setup hook ignored"),
    };
    assert_eq!(failed.failure.kind, FailureKind::Io(libc::EIO));
    let owner = failed.cleanup.take().expect("still-owned child");
    assert!(
        matches!(Client::spawn(&mut python("raise AssertionError('not admitted')"),deadline()),Err(e) if e.failure.kind==FailureKind::Busy && e.cleanup.is_none())
    );
    drop(owner);
    assert!(
        matches!(Client::spawn(&mut python("raise AssertionError('not admitted')"),deadline()),Err(e) if e.failure.kind==FailureKind::Busy)
    );
    let report = Client::cleanup_abandoned(deadline());
    assert!(matches!(
        report.state,
        strict_offline::CleanupState::Reaped(_)
    ));
    let mut next = client(&ledger(&[], true));
    finish_client(&mut next);
}

#[test]
fn class_id_is_exact_hex_and_rejection_preserves_budget_precedence() {
    let _serial = serial();
    for id in [
        "",
        "123",
        "0123456789abcdef0123456789abcdef0",
        "0123456789abcdef0123456789abcdef00",
        "0123456789abcdef0123456789abcdeg",
    ] {
        for stop in [0, 1, 2] {
            let mut c = client(&ledger(&[], false));
            if stop == 1 {
                c.cancellation().cancel();
            }
            let end = if stop == 2 {
                Instant::now()
            } else {
                deadline()
            };
            let failure = c
                .load_plugin(
                    strict_offline::LoadSpec {
                        path: "mock.vst3",
                        sample_rate: 48000.0,
                        tempo: 120.0,
                        time_sig_numerator: 4,
                        time_sig_denominator: 4,
                        class_id: id,
                    },
                    end,
                )
                .unwrap_err();
            assert_eq!(
                failure.kind,
                match stop {
                    1 => FailureKind::Cancelled,
                    2 => FailureKind::Deadline,
                    _ => FailureKind::InvalidRequest,
                }
            );
            assert_eq!(failure.delivery, strict_offline::Delivery::NotSent);
            finish_client(&mut c);
        }
    }
}

#[test]
fn partial_shutdown_stays_may_have_applied_with_no_second_protocol() {
    let _serial = serial();
    let mut c = client("import sys\nassert sys.stdin.buffer.read()==b'\"S'\n");
    pump::test_hooks::arm(pump::test_hooks::Fault::WriteShortThenZero);
    let report = c.dispose(deadline(), deadline());
    let failure = report.shutdown_failure.as_ref().unwrap();
    assert_eq!(failure.kind, FailureKind::ZeroWrite);
    assert_eq!(failure.delivery, strict_offline::Delivery::MayHaveApplied);
    assert!(matches!(report.state,strict_offline::CleanupState::Reaped(s) if s.success()));
    assert!(!report.kill_requested);
    let repeated = c.continue_cleanup().unwrap();
    assert_eq!(
        repeated.shutdown_failure.unwrap().delivery,
        strict_offline::Delivery::MayHaveApplied
    );
}

#[test]
fn known_stop_failure_is_not_relabelled_by_later_cancellation() {
    let _serial = serial();
    let mut c = client(&ledger(
        &[(
            HostCommand::StopProcessing,
            HostResponse::Error {
                message: "stop rejected".into(),
            },
        )],
        false,
    ));
    let failure = c.stop_processing(deadline()).unwrap_err();
    c.cancellation().cancel();
    assert_eq!(failure.kind, FailureKind::OperationRejected);
    assert_eq!(failure.delivery, strict_offline::Delivery::MayHaveApplied);
    assert_eq!(failure.diagnostic.as_deref(), Some("stop rejected"));
    finish_client(&mut c);
}

#[test]
fn cancellation_during_stop_preserves_sent_evidence_and_exact_eof_ledger() {
    let _serial = serial();
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "strict-stop-latch-{}-{}",
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
    let mut c=client(&format!("import json,sys\nassert json.loads(sys.stdin.readline())=='StopProcessing'\nwith open({path_json},'x') as out: out.write('accepted Stop')\nassert sys.stdin.read()==''\n"));
    let cancellation = c.cancellation();
    let wait_path = path.clone();
    let end = deadline();
    let result = std::thread::scope(|scope| {
        let worker = scope.spawn(move || {
            while !wait_path.exists() {
                assert!(Instant::now() < end, "stop admission latch watchdog");
                std::thread::sleep(Duration::from_millis(1));
            }
            cancellation.cancel();
        });
        let result = c.stop_processing(deadline());
        worker.join().unwrap();
        result
    });
    let failure = result.unwrap_err();
    assert_eq!(failure.kind, FailureKind::Cancelled);
    assert_eq!(failure.delivery, strict_offline::Delivery::MayHaveApplied);
    assert_eq!(std::fs::read_to_string(path).unwrap(), "accepted Stop");
    finish_client(&mut c);
}

#[test]
fn public_pending_cleanup_keeps_errors_and_blocks_new_candidate_until_reap() {
    let _serial = serial();
    let mut c = client("import time\ntime.sleep(30)\n");
    let report = c.dispose(Instant::now(), Instant::now());
    assert!(matches!(
        report.state,
        strict_offline::CleanupState::Pending
    ));
    assert_eq!(report.shutdown_failure.unwrap().kind, FailureKind::Deadline);
    // Allow the single recorded grace to pass; no response-time threshold claim.
    std::thread::sleep(Duration::from_millis(110));
    pump::test_hooks::arm(pump::test_hooks::Fault::Kill);
    let report = c.continue_cleanup().unwrap();
    assert_eq!(report.kill_error, Some(libc::EPERM));
    assert!(matches!(
        report.state,
        strict_offline::CleanupState::Pending
    ));
    assert!(
        matches!(Client::spawn(&mut python("raise AssertionError('blocked')"),deadline()),Err(e) if e.failure.kind==FailureKind::Busy)
    );
    let report = c.continue_cleanup().unwrap();
    assert!(report.kill_requested);
    assert!(matches!(
        report.state,
        strict_offline::CleanupState::Pending
    ));
    let end = deadline();
    loop {
        let report = c.continue_cleanup().unwrap();
        if matches!(report.state, strict_offline::CleanupState::Reaped(_)) {
            assert_eq!(report.kill_error, Some(libc::EPERM));
            assert_eq!(report.shutdown_failure.unwrap().kind, FailureKind::Deadline);
            break;
        }
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(1));
    }
    let mut next = client(&ledger(&[], true));
    finish_client(&mut next);
}

#[test]
fn process_wrong_shape_is_error_while_finite_policy_evidence_is_preserved() {
    let _serial = serial();
    for outputs in [
        vec![vec![0.0; 127]],
        vec![vec![0.0; 129]],
        vec![vec![0.0; 128]; 2],
    ] {
        let mut c = client(&ledger(
            &[(
                HostCommand::Process {
                    inputs: vec![],
                    frames: 128,
                    transport: Some(transport()),
                },
                HostResponse::AudioOutput {
                    outputs,
                    output_events: vec![],
                    output_events_lost: false,
                    transport_applied: true,
                },
            )],
            false,
        ));
        let error = c.process(1, transport(), deadline()).unwrap_err();
        assert_eq!(error.kind, FailureKind::Shape);
        assert_eq!(error.delivery, strict_offline::Delivery::MayHaveApplied);
        finish_client(&mut c);
    }
    let sample = f32::from_bits(0x7fc00123);
    let mut c = client(&ledger(
        &[(
            HostCommand::Process {
                inputs: vec![],
                frames: 128,
                transport: Some(transport()),
            },
            HostResponse::AudioOutput {
                outputs: vec![vec![sample; 128]],
                output_events: vec![],
                output_events_lost: true,
                transport_applied: false,
            },
        )],
        true,
    ));
    let out = c.process(1, transport(), deadline()).unwrap();
    assert!(out.outputs[0]
        .iter()
        .all(|s| s.to_bits() == sample.to_bits()));
    assert!(out.output_events_lost);
    assert!(!out.transport_applied);
    finish_client(&mut c);
}

#[test]
fn public_silent_generator_signature_accepts_counts_and_no_sample_buffer() {
    let _: fn(
        &mut Client,
        usize,
        usize,
        ProcessTransport,
        Instant,
    ) -> Result<strict_offline::AudioBlock, strict_offline::Failure> =
        Client::process_silent_generator;
}

#[test]
fn public_silent_generator_supplies_only_exact_positive_zero_q128_channels() {
    let _serial = serial();
    let mut c = client(
        r#"
import base64,json,struct,sys
for input_channels in (0,1,2,2,1,0):
 for output_channels in (1,2):
  command=json.loads(sys.stdin.readline())
  assert set(command)=={'Process'},command
  request=command['Process']
  assert set(request)=={'inputs','frames','transport'},request
  assert request['frames']==128,request
  assert len(request['inputs'])==input_channels,request
  for channel in request['inputs']:
   raw=base64.b64decode(channel,validate=True)
   assert len(raw)==128*4,len(raw)
   assert raw==bytes(128*4),raw
  assert request['transport']=={'sample_position':0,'quarter_note_position':0.0,'tempo':120.0,'playing':True,'time_sig_numerator':4,'time_sig_denominator':4},request
  outputs=[base64.b64encode(struct.pack('<f',0.25*(channel+1))*128).decode() for channel in range(output_channels)]
  print(json.dumps({'AudioOutput':{'outputs':outputs,'output_events':[],'output_events_lost':False,'transport_applied':True}}),flush=True)
assert json.loads(sys.stdin.readline())=='Shutdown'
assert sys.stdin.read()==''
"#,
    );
    for input_channels in [0, 1, 2, 2, 1, 0] {
        for output_channels in 1..=2 {
            let out = c
                .process_silent_generator(input_channels, output_channels, transport(), deadline())
                .unwrap();
            assert_eq!(out.outputs.len(), output_channels);
            for (channel, samples) in out.outputs.iter().enumerate() {
                assert_eq!(samples.len(), 128);
                assert!(samples
                    .iter()
                    .all(|sample| *sample == 0.25 * (channel + 1) as f32));
            }
            assert_eq!(out.output_event_count, 0);
            assert!(!out.output_events_lost);
            assert!(out.transport_applied);
        }
    }
    finish_client(&mut c);
}

#[test]
fn original_process_and_zero_input_generator_have_identical_wire_bytes() {
    let _serial = serial();
    let mut c = client(
        r#"
import base64,json,sys
for channels in (1,2):
 first=None
 for call in range(2):
  line=sys.stdin.buffer.readline()
  request=json.loads(line)['Process']
  assert request['inputs']==[],request
  assert request['frames']==128,request
  if first is None:
   first=line
  else:
   assert line==first,(line,first)
  print(json.dumps({'AudioOutput':{'outputs':[base64.b64encode(bytes(128*4)).decode()]*channels,'output_events':[],'output_events_lost':False,'transport_applied':True}}),flush=True)
assert json.loads(sys.stdin.buffer.readline())=='Shutdown'
assert sys.stdin.buffer.read()==b''
"#,
    );
    for channels in 1..=2 {
        let original = c.process(channels, transport(), deadline()).unwrap();
        let silent = c
            .process_silent_generator(0, channels, transport(), deadline())
            .unwrap();
        assert_eq!(original.outputs, silent.outputs);
    }
    finish_client(&mut c);
}

#[test]
fn silent_generator_invalid_counts_are_unsent_allocation_free_and_keep_budget_precedence() {
    let _serial = serial();
    use crate::internal::native_edit_transport::tests::measure_allocations;
    for (inputs, outputs) in [
        (3, 1),
        (usize::MAX, 2),
        (0, 0),
        (2, 3),
        (1, usize::MAX),
        (3, 3),
    ] {
        for stop in 0..4 {
            let mut c = client(&ledger(&[], false));
            if stop == 2 || stop == 3 {
                c.cancellation().cancel();
            }
            let end = if stop == 1 || stop == 3 {
                Instant::now()
            } else {
                deadline()
            };
            let (result, allocations) = measure_allocations(|| {
                c.process_silent_generator(inputs, outputs, transport(), end)
            });
            assert_eq!(allocations.allocations, 0);
            let failure = result.unwrap_err();
            assert_eq!(failure.delivery, strict_offline::Delivery::NotSent);
            assert_eq!(
                failure.kind,
                match stop {
                    1 => FailureKind::Deadline,
                    2 | 3 => FailureKind::Cancelled,
                    _ => FailureKind::Shape,
                }
            );
            // Rejection seals the session, retaining the original failure and
            // sending no follow-up Process or Shutdown bytes.
            let repeated = c
                .process_silent_generator(0, 1, transport(), deadline())
                .unwrap_err();
            assert_eq!(repeated.kind, failure.kind);
            assert_eq!(repeated.delivery, strict_offline::Delivery::NotSent);
            finish_client(&mut c);
        }
    }
}

#[test]
fn silent_generator_valid_counts_stop_before_encoding_or_send_when_budget_ends() {
    let _serial = serial();
    use crate::internal::native_edit_transport::tests::measure_allocations;
    for inputs in 0..=2 {
        for cancelled in [false, true] {
            let mut c = client(&ledger(&[], false));
            if cancelled {
                c.cancellation().cancel();
            }
            let end = if cancelled {
                deadline()
            } else {
                Instant::now()
            };
            let (result, allocations) =
                measure_allocations(|| c.process_silent_generator(inputs, 2, transport(), end));
            assert_eq!(allocations.allocations, 0);
            let failure = result.unwrap_err();
            assert_eq!(failure.delivery, strict_offline::Delivery::NotSent);
            assert_eq!(
                failure.kind,
                if cancelled {
                    FailureKind::Cancelled
                } else {
                    FailureKind::Deadline
                }
            );
            finish_client(&mut c);
        }
    }
}
