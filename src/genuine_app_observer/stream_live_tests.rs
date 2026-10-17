//! Pure live-observer fixtures. No SDK, helper, audio device or real source is used.
use super::*;
use crate::genuine_app_observer::{
    Boundary, CallOutcome, InstanceObserver, MAX_CLONE_REPLIES, MAX_PRIVATE_BYTES, MAX_RECORDS,
    ObservationRun, ObservationSnapshot, Operation, TimelineAppState, TimelinePhase,
    WorkerCaptureGuard, WorkerJoinOutcome,
};
use crate::instrument_bounce::{
    BounceAudioBus,
    render::{BlockReply, DisposalObservation, RenderReport},
};
use crate::plugins::plugin_runtime::{
    BridgeStats, PluginEndpointManifest, PluginTransport, RuntimeEvent,
};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

static DIRECTORY_ID: AtomicU64 = AtomicU64::new(1);
struct Fixture {
    directory: PathBuf,
    transcript: PathBuf,
    sidecar: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "citrus-live-stream-{}-{}",
            std::process::id(),
            DIRECTORY_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).unwrap();
        Self {
            transcript: directory.join("transcript.bin"),
            sidecar: directory.join("states.bin"),
            directory,
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.transcript);
        let _ = fs::remove_file(&self.sidecar);
        let _ = fs::remove_dir(&self.directory);
    }
}
fn source() -> InstanceIdentity {
    InstanceIdentity {
        ordinal: 1,
        phase: InstancePhase::InitialSource,
        worker_id: Some(7),
        source_instance_id: Some(11),
    }
}
fn source_stamp() -> BounceSourceIdentity {
    BounceSourceIdentity {
        project_session: 17,
        project_fingerprint: 19,
        timeline_revision: 23,
        epoch: 29,
        endpoint_id: 31,
        channel_id: 37,
        instance_id: 11,
    }
}
fn metadata() -> BouncePluginMetadata {
    BouncePluginMetadata {
        class_uid: "00112233445566778899aabbccddeeff".into(),
        version: "model-only".into(),
        name: "borrowed observer fixture".into(),
        category: "Instrument".into(),
        input_buses: vec![],
        output_buses: vec![BounceAudioBus {
            channel_count: 2,
            is_main: true,
            active: Some(true),
        }],
        event_input_buses: vec![(16, true, Some(true))],
        event_output_buses: vec![],
        latency_samples: 0,
        tail_samples: 0,
    }
}
fn run() -> (ObservationRun, InstanceObserver) {
    let mut state = ObservationSnapshot::new(MAX_RECORDS, MAX_PRIVATE_BYTES).unwrap();
    state.stage = Stage::InitialSource;
    state.push(Some(source()), 0, || {
        RecordKind::Lifecycle("instance construction observed")
    });
    let metadata = metadata();
    state.push(
        Some(source()),
        super::super::metadata_size(&metadata).unwrap(),
        || RecordKind::Metadata {
            site: "actual supplied fixture metadata",
            result: Ok(metadata),
        },
    );
    state.push(Some(source()), 3, || RecordKind::StateLoad {
        bytes: vec![0, 0x80, 0xff],
        outcome: CallOutcome::Ok,
    });
    let run = ObservationRun {
        state: Arc::new(Mutex::new(state)),
    };
    let observer = InstanceObserver {
        run: Some(Arc::downgrade(&run.state)),
        identity: Some(source()),
    };
    (run, observer)
}
fn enable(run: &ObservationRun, fixture: &Fixture) {
    run.enable_stream(
        &fixture.transcript,
        &fixture.sidecar,
        source(),
        Instant::now() + Duration::from_secs(60),
    )
    .unwrap();
}
fn joined(owner: &mut StreamRetirement) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match owner.poll_finish() {
            StreamFinishPoll::Pending => {
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
            StreamFinishPoll::Joined { complete } => return complete,
        }
    }
}
fn emit_record(run: &ObservationRun, instance: Option<InstanceIdentity>, kind: RecordKind) {
    super::super::with_run(&run.state, |state| {
        assert!(state.try_stream(instance, from_record(&kind).unwrap()));
    });
}
fn timeline() -> TimelineAppState {
    TimelineAppState {
        phase: TimelinePhase::LoopReady,
        project_session: 17,
        engine_session: Some(19),
        sync_session: 23,
        desired_generation: 29,
        observed_fingerprint: 31,
        compiled_generation: None,
        compiled_fingerprint: None,
        source_generation: None,
        source_fingerprint: None,
        sync_source_generation: None,
        desired_seek_serial: 37,
        desired_seek_frame: 0,
        loop_token: None,
        loop_disable_request: None,
        retry_count: 0,
        stale_confirmations: 0,
        confirmed_identity: None,
        retry_required: false,
        retry_pending: None,
    }
}

#[test]
fn default_memory_mode_retains_original_pcm_limit_private_bytes_and_ordinals() {
    let (run, observer) = run();
    let reply = BlockReply {
        channels: vec![vec![0.25; 1], vec![-0.25; 1]],
        transport_applied: true,
        output_events: 0,
        output_events_lost: false,
    };
    let before = run.with_snapshot(|s| (s.records.len(), s.private_bytes));
    for _ in 0..MAX_CLONE_REPLIES {
        observer.pcm(1, PluginTransport::default(), &reply);
    }
    observer.pcm(1, PluginTransport::default(), &reply);
    run.with_snapshot(|s| {
        assert!(s.stream.is_none());
        assert_eq!(s.clone_replies, MAX_CLONE_REPLIES);
        assert_eq!(s.records.len(), before.0 + MAX_CLONE_REPLIES);
        assert_eq!(s.private_bytes, before.1 + MAX_CLONE_REPLIES * 8);
        assert_eq!(s.failure, Some("observer strict PCM cap exceeded"));
        for (index, record) in s.records.iter().enumerate() {
            assert_eq!(record.ordinal, index as u64 + 1);
        }
    });
}

#[test]
fn default_memory_source_attempt_limit_remains_4096_and_is_never_reset() {
    let (run, observer) = run();
    for _ in 0..super::super::MAX_SOURCE_POSITIVE_ATTEMPTS + 1 {
        observer.call(
            Operation::OrdinaryProcess,
            Boundary::Attempt,
            Some(128),
            Some(PluginTransport::default()),
            None,
        );
    }
    run.with_snapshot(|snapshot| {
        assert!(snapshot.stream.is_none());
        assert_eq!(
            snapshot.source_positive_attempts,
            super::super::MAX_SOURCE_POSITIVE_ATTEMPTS + 1
        );
        assert_eq!(
            snapshot.failure,
            Some("source positive processing attempt cap exceeded")
        );
        assert_eq!(
            snapshot.records.len(),
            3 + super::super::MAX_SOURCE_POSITIVE_ATTEMPTS + 1
        );
    });
}

#[test]
fn enable_preserves_every_prelude_arc_header_and_opaque_allocation() {
    let fixture = Fixture::new();
    let (run, observer) = run();
    let original = run.copy_records_since(0);
    let private = run.with_snapshot(|s| s.private_bytes);
    let opaque = match &original[2].kind {
        RecordKind::StateLoad { bytes, .. } => bytes.as_ptr(),
        _ => panic!("state missing"),
    };
    enable(&run, &fixture);
    observer.midi([0x90, 60, 100], 0);
    run.with_snapshot(|s| {
        assert_eq!(s.private_bytes, private);
        assert_eq!(s.records.len(), original.len());
        for (current, prior) in s.records.iter().zip(&original) {
            assert!(Arc::ptr_eq(current, prior));
            assert_eq!(current.ordinal, prior.ordinal);
            assert_eq!(current.monotonic_elapsed_ns, prior.monotonic_elapsed_ns);
            assert_eq!(current.stage, prior.stage);
            assert_eq!(current.instance, prior.instance);
        }
        let RecordKind::StateLoad { bytes, .. } = &s.records[2].kind else {
            panic!("state missing")
        };
        assert_eq!(bytes.as_ptr(), opaque);
    });
    assert!(run.take_stream_after_taps_retired(false).is_err());
    let mut owner = run.take_stream_after_taps_retired(true).unwrap();
    assert!(joined(&mut owner));
    let inspection = owner.inspect();
    assert_eq!(inspection.actor.unwrap().expected.records, 4);
    assert_eq!(inspection.states.unwrap().ranges[0].length, 3);
    assert!(run.take_stream_after_taps_retired(true).is_err());
}

#[test]
fn joined_source_only_stream_keeps_writer_evidence_but_refuses_complete_song_replay() {
    let fixture = Fixture::new();
    let (run, _observer) = run();
    enable(&run, &fixture);
    let mut owner = run.take_stream_after_taps_retired(true).unwrap();
    assert!(joined(&mut owner));

    let evidence = owner.writer_evidence();
    assert_eq!(evidence["transcript"], fixture.transcript.to_str().unwrap());
    assert_eq!(evidence["states"], fixture.sidecar.to_str().unwrap());
    assert_eq!(evidence["transcript_actual_writer_joined"], true);
    assert_eq!(evidence["transcript_complete"], true);
    assert_eq!(evidence["transcript_records"], 3);
    assert_eq!(evidence["sidecar_actual_writer_joined"], true);
    assert_eq!(evidence["state_range_linkage"], true);
    assert_eq!(evidence["instance_dictionary_complete"], true);
    assert_eq!(evidence["SDK_or_source_helper_reap_claim"], false);

    // Actual joined writers do not establish a missing clone or renderer run.
    // Refusal must happen before creating an apparently valid raw output file.
    let raw = fixture.directory.join("incomplete-song.f32le");
    let error = owner.replay_to_raw(&raw).unwrap_err();
    assert_eq!(error.code, "raw replay actual clone birth is absent");
    assert!(!raw.exists());
    assert_eq!(owner.writer_evidence(), evidence);
}

#[test]
fn streaming_exceeds_both_memory_caps_without_cloning_pcm_or_draining_prelude() {
    let fixture = Fixture::new();
    let (run, observer) = run();
    enable(&run, &fixture);
    let reply = BlockReply {
        channels: vec![vec![0.25; 128], vec![-0.25; 128]],
        transport_applied: true,
        output_events: 0,
        output_events_lost: false,
    };
    let original = run.copy_records_since(0);
    let private = run.with_snapshot(|s| s.private_bytes);
    for _ in 0..4097 {
        observer.call(
            Operation::OrdinaryProcess,
            Boundary::Attempt,
            Some(128),
            Some(PluginTransport::default()),
            None,
        );
        observer.string_result(
            Operation::OrdinaryProcess,
            Some(128),
            Some(PluginTransport::default()),
            &Ok::<(), String>(()),
        );
    }
    for _ in 0..4097 {
        observer.pcm(128, PluginTransport::default(), &reply);
    }
    for _ in 0..MAX_RECORDS + 1 {
        observer.midi([0x90, 64, 100], 127);
    }
    run.with_snapshot(|s| {
        assert!(s.failure.is_none());
        assert_eq!(s.records.len(), original.len());
        assert_eq!(s.private_bytes, private);
        assert_eq!(s.clone_replies, 0);
        assert_eq!(s.stream.as_ref().unwrap().pcm_replies, 4097);
        assert_eq!(s.stream.as_ref().unwrap().source_positive_attempts, 4097);
        assert_eq!(s.source_positive_attempts, 0);
        assert_eq!(
            s.stream.as_ref().unwrap().emitted_records,
            original.len() as u64 + 3 * 4097 + MAX_RECORDS as u64 + 1
        );
        for (a, b) in s.records.iter().zip(&original) {
            assert!(Arc::ptr_eq(a, b));
        }
    });
    let mut owner = run.take_stream_after_taps_retired(true).unwrap();
    assert!(joined(&mut owner));
    let actor = owner.inspect().actor.unwrap();
    assert!(actor.terminal.unwrap().complete());
    assert_eq!(
        actor.expected.records,
        3 + 3 * 4097 + MAX_RECORDS as u64 + 1
    );
}

#[test]
fn first_encoding_failure_keeps_complete_original_record_and_the_same_sampled_header_once() {
    let fixture = Fixture::new();
    let (run, observer) = run();
    enable(&run, &fixture);
    let failed_site: &'static str =
        Box::leak("x".repeat(super::super::MAX_TEXT + 1).into_boxed_str());
    let value = metadata();
    observer.metadata(failed_site, Ok(&value));
    observer.metadata("later failure must not clone", Ok(&value));
    run.with_snapshot(|s| {
        assert_eq!(s.records.len(), 4);
        let record = &s.records[3];
        let RecordKind::Metadata {
            site,
            result: Ok(actual),
        } = &record.kind
        else {
            panic!("complete failed original absent")
        };
        assert_eq!(*site, failed_site);
        assert_eq!(actual, &value);
        let attempted = s
            .stream
            .as_ref()
            .unwrap()
            .actor
            .as_ref()
            .unwrap()
            .failure_header()
            .unwrap();
        assert_eq!(record.ordinal, attempted.sequence);
        assert_eq!(record.monotonic_elapsed_ns, attempted.monotonic_ns);
        assert_eq!(record.instance, attempted.instance);
        assert_eq!(record.stage, attempted.stage);
        assert!(s.stream.as_ref().unwrap().failed_header_to_retain.is_none());
        assert!(
            s.stream
                .as_ref()
                .unwrap()
                .actor
                .as_ref()
                .unwrap()
                .inspect()
                .buffer
                .unwrap()
                .is_empty()
        );
    });
    let mut owner = run.take_stream_after_taps_retired(true).unwrap();
    assert!(!joined(&mut owner));
    assert!(owner.inspect().failure.is_some());
    assert!(owner.inspect().state_terminal.unwrap().actual_writer_joined);
}

#[test]
fn fifth_stream_state_preserves_the_failed_original_and_all_four_admitted_owners() {
    let fixture = Fixture::new();
    let (run, observer) = run();
    enable(&run, &fixture);
    for value in 1..=3 {
        observer.state_load(&[0x80, value], &Ok(()));
    }
    observer.state_load(&[0xff, 0x80, 4], &Ok(()));
    observer.state_load(&[99], &Ok(()));
    run.with_snapshot(|snapshot| {
        assert_eq!(snapshot.records.len(), 4);
        let RecordKind::StateLoad {
            bytes,
            outcome: CallOutcome::Ok,
        } = &snapshot.records[3].kind
        else {
            panic!("failed original state was not retained")
        };
        assert_eq!(bytes, &[0xff, 0x80, 4]);
        assert_eq!(
            snapshot
                .stream
                .as_ref()
                .unwrap()
                .states
                .as_ref()
                .unwrap()
                .len(),
            4
        );
    });
    let mut owner = run.take_stream_after_taps_retired(true).unwrap();
    assert!(!joined(&mut owner));
    let StateOwner::Running(sidecar) = &owner.state_owner else {
        panic!("four admitted state owners lost")
    };
    assert_eq!(sidecar.owners().len(), 4);
    assert_eq!(sidecar.owners()[0].bytes, [0, 0x80, 0xff]);
    for (index, admitted) in sidecar.owners()[1..].iter().enumerate() {
        assert_eq!(admitted.bytes, [0x80, index as u8 + 1]);
    }
    assert!(owner.inspect().state_terminal.unwrap().actual_writer_joined);
}

#[test]
fn stream_pcm_shape_and_timeline_lifetime_limit_keep_original_rejection_text() {
    let fixture = Fixture::new();
    let (run, observer) = run();
    enable(&run, &fixture);
    observer.pcm(
        128,
        PluginTransport::default(),
        &BlockReply {
            channels: vec![vec![0.0; 129]],
            transport_applied: true,
            output_events: 0,
            output_events_lost: false,
        },
    );
    run.with_snapshot(|s| {
        assert_eq!(s.failure, Some("observer strict PCM cap exceeded"));
        assert_eq!(s.records.len(), 3);
        assert_eq!(s.stream.as_ref().unwrap().pcm_replies, 0);
    });
    let mut owner = run.take_stream_after_taps_retired(true).unwrap();
    assert!(!joined(&mut owner));
    let fixture = Fixture::new();
    let (run, _) = self::run();
    enable(&run, &fixture);
    super::super::with_run(&run.state, |snapshot| {
        for _ in 0..=super::super::MAX_TIMELINE_EVENTS {
            snapshot.timeline_event(
                crate::timeline_runtime::TimelineRuntimeEvent::Installed {
                    request_id: 31,
                    revision: 37,
                },
                timeline(),
                timeline(),
            );
        }
        assert_eq!(snapshot.timeline_events, super::super::MAX_TIMELINE_EVENTS);
        assert_eq!(
            snapshot.failure,
            Some("observer Timeline event cap exceeded")
        );
        assert_eq!(snapshot.records.len(), 3);
    });
    let mut owner = run.take_stream_after_taps_retired(true).unwrap();
    assert!(!joined(&mut owner));
}

#[test]
fn cancellation_and_repeated_terminal_read_keep_the_same_complete_retirement_owner() {
    let fixture = Fixture::new();
    let (run, observer) = run();
    enable(&run, &fixture);
    observer.midi([0x80, 60, 0], 0);
    let mut owner = run.take_stream_after_taps_retired(true).unwrap();
    owner.cancel();
    assert!(!joined(&mut owner));
    let first = owner.inspect().actor.unwrap().expected;
    assert!(
        owner
            .inspect()
            .actor
            .unwrap()
            .terminal
            .unwrap()
            .writer
            .writer_joined
    );
    assert!(owner.inspect().state_terminal.unwrap().actual_writer_joined);
    assert!(owner.inspect().states.is_none());
    assert!(!joined(&mut owner));
    assert_eq!(owner.inspect().actor.unwrap().expected, first);
    let StateOwner::Running(sidecar) = &owner.state_owner else {
        panic!("original states lost")
    };
    assert_eq!(sidecar.owners()[0].bytes, [0, 0x80, 0xff]);
    assert!(owner.inspect().startup_failure.is_none());
    assert!(owner.inspect().state_start_failure.is_none());
    assert!(owner.inspect().state_seal_failure.is_none());
}

#[test]
fn unsealed_drop_recovers_the_whole_original_prelude_failure_slots_and_writer_owner() {
    let fixture = Fixture::new();
    let (run, observer) = run();
    enable(&run, &fixture);
    let failed_site: &'static str =
        Box::leak("x".repeat(super::super::MAX_TEXT + 1).into_boxed_str());
    observer.metadata(failed_site, Ok(&metadata()));
    let handle = run.stream_recovery_handle().unwrap();
    let (source_record, failed_record) =
        run.with_snapshot(|s| (Arc::downgrade(&s.records[2]), Arc::downgrade(&s.records[3])));
    drop(run.finish());
    assert!(source_record.upgrade().is_some());
    assert!(failed_record.upgrade().is_some());
    assert!(handle.take_after_taps_retired(false).is_err());
    let owner = handle.take_after_taps_retired(true).unwrap();
    assert!(Arc::ptr_eq(
        owner.live.failed_original.as_ref().unwrap(),
        &failed_record.upgrade().unwrap()
    ));
    assert!(Arc::ptr_eq(
        &owner.live.prelude[2],
        &source_record.upgrade().unwrap()
    ));
    assert_eq!(owner.live.source, source());
    assert_eq!(owner.live.next_sequence, 4);
    let recovered = owner.recovery_handle();
    // No fake retirement is inferred from an already quiet or finished worker.
    // This sidecar's actual JoinHandle has not been consumed by poll_finish.
    drop(owner);
    let mut owner = recovered.take_after_taps_retired(true).unwrap();
    assert!(!joined(&mut owner));
    assert!(
        owner
            .inspect()
            .actor
            .unwrap()
            .terminal
            .unwrap()
            .writer
            .writer_joined
    );
    assert!(owner.inspect().state_terminal.unwrap().actual_writer_joined);
    let RecordKind::Metadata {
        site,
        result: Ok(value),
    } = &owner.live.failed_original.as_ref().unwrap().kind
    else {
        panic!("whole failed record lost")
    };
    assert_eq!(*site, failed_site);
    assert_eq!(value, &metadata());
    let StateOwner::Running(sidecar) = &owner.state_owner else {
        panic!("original opaque state slots lost")
    };
    assert_eq!(sidecar.owners()[0].bytes, [0, 0x80, 0xff]);
    assert!(recovered.take_after_taps_retired(true).is_err());
}

#[test]
fn bad_enable_identity_or_path_cannot_replace_source_or_start_a_writer() {
    let fixture = Fixture::new();
    let (run, _) = run();
    let mut wrong = source();
    wrong.worker_id = Some(8);
    assert!(
        run.enable_stream(
            &fixture.transcript,
            &fixture.sidecar,
            wrong,
            Instant::now() + Duration::from_secs(10)
        )
        .is_err()
    );
    assert!(
        run.enable_stream(
            Path::new("relative.bin"),
            &fixture.sidecar,
            source(),
            Instant::now() + Duration::from_secs(10)
        )
        .is_err()
    );
    assert!(!fixture.transcript.exists());
    run.with_snapshot(|s| {
        assert_eq!(s.records.len(), 3);
        assert!(s.stream.is_none());
    });
}

#[test]
fn startup_collision_keeps_dictionary_prelude_and_failed_actor_owners() {
    let fixture = Fixture::new();
    fs::write(&fixture.transcript, b"preserve prior evidence").unwrap();
    let (run, _) = run();
    assert!(
        run.enable_stream(
            &fixture.transcript,
            &fixture.sidecar,
            source(),
            Instant::now() + Duration::from_secs(10)
        )
        .is_err()
    );
    run.with_snapshot(|s| {
        assert_eq!(s.records.len(), 3);
        assert!(s.stream.as_ref().unwrap().startup_failure.is_some());
    });
    let mut owner = run.take_stream_after_taps_retired(true).unwrap();
    assert!(!joined(&mut owner));
    assert_eq!(
        fs::read(&fixture.transcript).unwrap(),
        b"preserve prior evidence"
    );
    assert!(owner.inspect().startup_failure.is_some());
    assert!(owner.inspect().state_seal_failure.is_some());
}

#[test]
fn all_twenty_three_original_kinds_have_lossless_borrowed_stream_wiring() {
    let fixture = Fixture::new();
    let (run, observer) = run();
    enable(&run, &fixture);
    observer.call(
        Operation::SourcePrepare,
        Boundary::Returned,
        None,
        None,
        Some(CallOutcome::Ok),
    );
    observer.state_load(&[1, 0x80, 0xff], &Ok(()));
    observer.base_read("base", 0, &Ok(-0.0));
    let metadata = metadata();
    observer.metadata("fresh metadata", Ok(&metadata));
    observer.audio_declaration(&metadata.input_buses, &metadata.output_buses);
    observer.event_declaration(
        metadata.event_input_buses.iter().copied(),
        metadata.event_output_buses.iter().copied(),
    );
    observer.pcm(
        128,
        PluginTransport::default(),
        &BlockReply {
            channels: vec![vec![0.0; 128], vec![0.0; 128]],
            transport_applied: true,
            output_events: 0,
            output_events_lost: false,
        },
    );
    observer.midi([0x90, 60, 100], 0);
    let captured = BouncePluginState {
        metadata: metadata.clone(),
        opaque_state: vec![3, 0x80, 0xff],
        parameter_bases: BTreeMap::from([(0, -0.0)]),
        native_generation: 41,
        native_dirty_revision: 43,
    };
    emit_record(
        &run,
        None,
        RecordKind::Capture {
            label: CaptureLabel::Initial,
            identity: source_stamp(),
            receipt_identity: 47,
            state: captured,
        },
    );
    emit_record(
        &run,
        None,
        RecordKind::AppAudioAssetEvent(crate::audio::AudioAssetEvent::Cleared {
            operation: crate::audio::AudioAssetOperation {
                engine_session: 17,
                generation: 19,
                operation_id: 23,
            },
            removed: 0,
        }),
    );
    emit_record(
        &run,
        None,
        RecordKind::AppGeneratorEndpointEvent(crate::audio::GeneratorEndpointEvent::Cleared {
            request_id: 29,
            removed: 0,
        }),
    );
    super::super::with_run(&run.state, |s| {
        s.timeline_event(
            crate::timeline_runtime::TimelineRuntimeEvent::Installed {
                request_id: 31,
                revision: 37,
            },
            timeline(),
            timeline(),
        )
    });
    super::super::with_run(&run.state, |s| {
        super::super::record_capture_guard_into(
            s,
            Some(7),
            WorkerCaptureGuard {
                expected_epoch: 29,
                active_epoch: 29,
                requested_epoch: 29,
                input_empty: true,
                parameter_admission: 0,
                accepted: true,
            },
        )
    });
    emit_record(
        &run,
        None,
        RecordKind::WorkerJoin {
            worker_id: 7,
            outcome: WorkerJoinOutcome::Joined,
        },
    );
    emit_record(
        &run,
        None,
        RecordKind::RuntimeEvent {
            endpoint_id: 31,
            project_session: 17,
            instance_id: 11,
            event: RuntimeEvent::ShutdownComplete,
        },
    );
    emit_record(
        &run,
        None,
        RecordKind::OrdinaryState {
            endpoint_id: 31,
            project_session: 17,
            instance_id: 11,
            slot: 0,
            request_id: 53,
            epoch: Some(29),
            byte_len: 591772,
        },
    );
    observer.reset_report(&vst3_host::ResetOriginReport {
        frames: 0,
        discarded_prior_midi_events: 0,
        discarded_midi_events: 0,
        discarded_prior_parameter_points: 0,
        discarded_parameter_points: 0,
        output_events_lost: false,
        parameter_output_fault: false,
        process_error: None,
    });
    let clone = InstanceIdentity {
        ordinal: 2,
        phase: InstancePhase::StrictClone,
        worker_id: None,
        source_instance_id: None,
    };
    run.set_stage(Stage::WholeExport).unwrap();
    emit_record(
        &run,
        Some(clone),
        RecordKind::Lifecycle("instance construction observed"),
    );
    let clone_observer = InstanceObserver {
        run: Some(Arc::downgrade(&run.state)),
        identity: Some(clone),
    };
    clone_observer.capture_identity(source_stamp());
    emit_record(
        &run,
        None,
        RecordKind::RenderReport(RenderReport {
            identity: source_stamp(),
            origin_frame: 0,
            musical_frames: 128,
            frames: 128,
            native_latency: 0,
            reported_tail: 0,
            fixed_tail: 0,
            generated_frames: 128,
            sample_peak: 0.0,
        }),
    );
    clone_observer.disposal(&DisposalObservation {
        terminated: true,
        error: None,
        diagnostics: vec![],
    });
    emit_record(
        &run,
        None,
        RecordKind::Worker {
            site: "actual fixture worker",
            worker_id: 7,
            manifest: PluginEndpointManifest::identified(&[11]).unwrap(),
            epoch: 29,
            sequence: Some(0),
            stats: BridgeStats::default(),
        },
    );
    emit_record(
        &run,
        None,
        RecordKind::HookInventory {
            site: "inactive fixture hook",
            inactive: true,
        },
    );
    observer.lifecycle("actual fixture end");
    let mut owner = run.take_stream_after_taps_retired(true).unwrap();
    assert!(joined(&mut owner));
    let inspection = owner.inspect();
    assert!(inspection.instances.unwrap().links_complete());
    let dictionary = inspection.actor.as_ref().unwrap().dictionary;
    let mut file = File::open(&fixture.transcript).unwrap();
    let mut operand = Vec::with_capacity(envelope::MAX_OPERAND);
    let mut seen = [false; 23];
    loop {
        let mut prefix = [0; 4];
        let n = file.read(&mut prefix).unwrap();
        if n == 0 {
            break;
        }
        assert_eq!(n, 4);
        let size = u32::from_le_bytes(prefix) as usize;
        assert!(size + 4 <= envelope::MAX_OPERAND);
        operand.clear();
        operand.extend_from_slice(&prefix);
        operand.resize(size + 4, 0);
        file.read_exact(&mut operand[4..]).unwrap();
        let (header, _) = envelope::decode(&operand).unwrap();
        seen[header.kind as usize - 1] = true;
        match header.lane {
            envelope::Lane::Control => {
                super::super::stream_body::decode_control(&operand).unwrap();
            }
            envelope::Lane::App => {
                super::super::stream_body::decode_app(&operand).unwrap();
            }
            envelope::Lane::Native(_) => {
                super::super::stream_body::decode_native(&operand, dictionary).unwrap();
            }
            envelope::Lane::State => {
                let (actual, _) = state::decode_enveloped(&operand, Some(dictionary)).unwrap();
                if actual.kind == 9 {
                    assert_eq!(actual.instance, None);
                }
            }
        }
    }
    assert!(seen.iter().all(|kind| *kind));
    assert_eq!(
        file.seek(SeekFrom::End(0)).unwrap(),
        inspection.actor.unwrap().expected.bytes
    );
}
