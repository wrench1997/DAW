//! Actual private-file parser/ordering regressions, without a native SDK.
use super::*;
use crate::{
    audio::{AudioAssetEvent, AudioAssetOperation, GeneratorEndpointEvent},
    genuine_app_observer::{CallOutcome, TimelineAppState, TimelinePhase, WorkerCaptureGuard},
    instrument_bounce::{
        BounceAudioBus, BouncePluginMetadata, BouncePluginState, render::RenderReport,
    },
    plugins::plugin_runtime::{BridgeStats, PluginEndpointManifest, PluginTransport, RuntimeEvent},
    timeline_runtime::TimelineRuntimeEvent,
};
use std::{
    collections::BTreeMap,
    fs::{self, FileTimes, OpenOptions},
    io::{Seek, SeekFrom, Write},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant, SystemTime},
};

static NEXT_PATH: AtomicU64 = AtomicU64::new(1);
struct TestPath(PathBuf);
impl TestPath {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
            "daw-stream-replay-{}-{}.bin",
            std::process::id(),
            NEXT_PATH.fetch_add(1, Ordering::Relaxed)
        )))
    }
}
impl Drop for TestPath {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
fn metadata() -> BouncePluginMetadata {
    BouncePluginMetadata {
        class_uid: "fixture class".into(),
        version: "1".into(),
        name: "fixture".into(),
        category: "Instrument".into(),
        input_buses: vec![],
        output_buses: vec![BounceAudioBus::new(2, true, Some(true))],
        event_input_buses: vec![(16, true, None)],
        event_output_buses: vec![],
        latency_samples: 3,
        tail_samples: 5,
    }
}
fn dictionary() -> native::BaselineDictionary {
    native::BaselineDictionary::new(
        7,
        native::CanonicalMetadata {
            class_uid: "fixture class".into(),
            version: "1".into(),
            name: "fixture".into(),
            category: "Instrument".into(),
            audio: native::AudioLayout {
                inputs: vec![],
                outputs: vec![native::AudioBus {
                    channel_count: 2,
                    is_main: true,
                    active: Some(true),
                }],
            },
            events: native::EventLayout {
                inputs: vec![native::EventBus {
                    channel_count: 16,
                    is_main: true,
                    active: None,
                }],
                outputs: vec![],
            },
            latency_samples: 3,
            tail_samples: 5,
        },
    )
    .unwrap()
}
fn header(sequence: u64, kind: u16, lane: outer::Lane) -> outer::Header {
    outer::Header {
        sequence,
        monotonic_ns: sequence * 3,
        stage: Stage::WholeExport,
        instance: None,
        kind,
        lane,
    }
}
struct Fixture {
    path: TestPath,
    dictionary: native::BaselineDictionary,
    actor: sink::StreamSink,
    sequence: u64,
    stage: Stage,
}
impl Fixture {
    fn new() -> Self {
        let path = TestPath::new();
        let dictionary = dictionary();
        let actor = sink::StreamSink::create_new(
            dictionary.clone(),
            &path.0,
            Instant::now() + Duration::from_secs(30),
        )
        .unwrap();
        Self {
            path,
            dictionary,
            actor,
            sequence: 0,
            stage: Stage::WholeExport,
        }
    }
    fn next(
        &mut self,
        kind: u16,
        lane: outer::Lane,
        instance: Option<InstanceIdentity>,
    ) -> outer::Header {
        self.sequence += 1;
        outer::Header {
            stage: self.stage,
            instance,
            ..header(self.sequence, kind, lane)
        }
    }
    fn control(
        &mut self,
        view: control::View<'_>,
        instance: Option<InstanceIdentity>,
    ) -> outer::Header {
        let header = self.next(view.kind(), outer::Lane::Control, instance);
        self.actor.emit_control(header, view).unwrap();
        header
    }
    fn native(
        &mut self,
        view: native::NativeView<'_>,
        instance: Option<InstanceIdentity>,
    ) -> outer::Header {
        let header = self.next(view.kind(), outer::Lane::Native(view.tag()), instance);
        self.actor.emit_native(header, view).unwrap();
        header
    }
    fn app(&mut self, view: app::AppView<'_>) {
        let header = self.next(view.kind(), outer::Lane::App, None);
        self.actor.emit_app(header, view).unwrap();
    }
    fn seal(&mut self) {
        self.actor.seal_after_taps_retired(true).unwrap();
        let bound = Instant::now() + Duration::from_secs(10);
        loop {
            match self.actor.poll_finish() {
                sink::FinishPoll::Joined(terminal) => {
                    assert!(terminal.complete());
                    return;
                }
                sink::FinishPoll::Pending => {
                    assert!(Instant::now() < bound);
                    thread::yield_now();
                }
            }
        }
    }
    fn reader(&self) -> ReplayReader<'_> {
        ReplayReader::open_joined(
            &self.path.0,
            &self.actor.terminal().unwrap(),
            &self.dictionary,
        )
        .unwrap()
    }
}
fn one_record() -> Fixture {
    let mut fixture = Fixture::new();
    fixture.control(control::View::Lifecycle("private fixture"), None);
    fixture.seal();
    fixture
}
fn ignore(_: RecordView<'_>) -> Result<(), VisitError> {
    Ok(())
}
fn mutate(path: &Path, offset: u64, bytes: &[u8]) {
    let mut file = OpenOptions::new().write(true).open(path).unwrap();
    file.seek(SeekFrom::Start(offset)).unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}
fn original_header_offset() -> usize {
    13
} // NOBS v1: len4/magic4/version1/lane1/tag1/kind2.

#[test]
fn actual_joined_file_reads_one_operand_and_proves_exact_eof() {
    let fixture = one_record();
    let mut reader = fixture.reader();
    let pointer = reader.retained_operand().as_ptr();
    assert_eq!(reader.operand_capacity(), outer::MAX_OPERAND);
    assert_eq!(
        reader
            .read_next(|record| {
                assert_eq!(record.header, header(1, 23, outer::Lane::Control));
                let Payload::Control(control::Control::Lifecycle(text)) = record.payload else {
                    panic!("wrong typed body")
                };
                assert_eq!(text, "private fixture");
                Ok(())
            })
            .unwrap(),
        ReadProgress::Record
    );
    assert_eq!(reader.retained_operand().as_ptr(), pointer);
    assert!(!reader.summary().complete);
    assert_eq!(reader.read_next(ignore).unwrap(), ReadProgress::Finished);
    assert!(
        reader.summary().actual_eof
            && reader.summary().file_stamp_matches
            && reader.summary().complete
    );
    assert_eq!(
        reader.summary().accepted_bytes,
        reader.summary().expected_bytes
    );
    assert_eq!(
        reader.summary().physical_bytes,
        reader.summary().expected_bytes
    );
    assert!(reader.failure().is_none());
}

#[test]
fn thousands_of_actual_records_reuse_one_reader_allocation() {
    let mut fixture = Fixture::new();
    for _ in 0..4000 {
        fixture.control(control::View::Lifecycle("bounded"), None);
    }
    fixture.seal();
    let mut reader = fixture.reader();
    let pointer = reader.retained_operand().as_ptr();
    let mut seen = 0;
    while reader
        .read_next(|record| {
            seen += 1;
            assert_eq!(record.header.sequence, seen);
            Ok(())
        })
        .unwrap()
        == ReadProgress::Record
    {
        assert_eq!(reader.retained_operand().as_ptr(), pointer);
        assert_eq!(reader.operand_capacity(), outer::MAX_OPERAND);
    }
    assert_eq!(seen, 4000);
    assert_eq!(reader.summary().by_kind[23], 4000);
    assert!(reader.summary().complete);
}

#[test]
fn independently_corrupted_global_sequence_fails_sticky_without_draining() {
    let fixture = one_record();
    mutate(
        &fixture.path.0,
        original_header_offset() as u64,
        &2_u64.to_le_bytes(),
    );
    let mut reader = fixture.reader();
    let first = reader.read_next(ignore).unwrap_err();
    assert_eq!(first.kind, FailureKind::Sequence);
    let physical = reader.summary().physical_bytes;
    let partial = reader.retained_operand().to_vec();
    assert_eq!(reader.read_next(ignore).unwrap_err(), first);
    assert_eq!(reader.summary().physical_bytes, physical);
    assert_eq!(reader.retained_operand(), partial);
    assert!(!reader.summary().complete);
}

#[test]
fn caller_time_and_stage_regressions_are_rejected_from_independent_wire() {
    for timestamp in [true, false] {
        let mut fixture = Fixture::new();
        fixture.control(control::View::Lifecycle("first"), None);
        fixture.control(control::View::Lifecycle("second"), None);
        fixture.seal();
        let bytes = fs::read(&fixture.path.0).unwrap();
        let second = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize + 4;
        if timestamp {
            mutate(&fixture.path.0, (second + 21) as u64, &2_u64.to_le_bytes());
        } else {
            mutate(&fixture.path.0, (second + 29) as u64, &[8]);
        }
        let mut reader = fixture.reader();
        assert_eq!(reader.read_next(ignore).unwrap(), ReadProgress::Record);
        assert_eq!(
            reader.read_next(ignore).unwrap_err().kind,
            if timestamp {
                FailureKind::Timestamp
            } else {
                FailureKind::Stage
            }
        );
        assert!(!reader.summary().complete);
    }
}

#[test]
fn independently_invalid_length_magic_lane_and_kind_never_fall_back_to_opaque() {
    for (offset, replacement) in [
        (0, vec![0, 0, 0, 0]),
        (0, u32::MAX.to_le_bytes().to_vec()),
        (4, b"BAD!".to_vec()),
        (9, vec![255]),
        (11, 24_u16.to_le_bytes().to_vec()),
        (13, 0_u64.to_le_bytes().to_vec()),
    ] {
        let fixture = one_record();
        mutate(&fixture.path.0, offset, &replacement);
        let mut reader = fixture.reader();
        assert!(reader.read_next(ignore).is_err());
        assert_eq!(reader.summary().accepted_records, 0);
        assert!(!reader.summary().complete);
    }
}

#[test]
fn body_kind_mismatch_and_trailing_typed_body_byte_are_rejected() {
    for mismatch in [true, false] {
        let fixture = one_record();
        // A no-instance header is 35 bytes; NCTL kind is at body offset6,
        // Lifecycle text length at8. No production encoder builds this bad wire.
        if mismatch {
            mutate(&fixture.path.0, 35 + 6, &22_u16.to_le_bytes());
        } else {
            mutate(
                &fixture.path.0,
                35 + 8,
                &("private fixture".len() as u32 - 1).to_le_bytes(),
            );
        }
        let mut reader = fixture.reader();
        assert!(matches!(
            reader.read_next(ignore).unwrap_err().kind,
            FailureKind::Body(_)
        ));
        assert!(!reader.summary().complete);
    }
}

#[test]
fn truncated_prefix_or_body_keeps_only_actual_received_bytes() {
    for keep in [2, 13] {
        let fixture = one_record();
        let mut reader = fixture.reader();
        OpenOptions::new()
            .write(true)
            .open(&fixture.path.0)
            .unwrap()
            .set_len(keep)
            .unwrap();
        assert_eq!(
            reader.read_next(ignore).unwrap_err().kind,
            FailureKind::Truncated
        );
        assert_eq!(reader.retained_operand().len() as u64, keep);
        assert_eq!(reader.summary().physical_bytes, keep);
        assert!(!reader.summary().complete);
    }
}

#[test]
fn appended_actual_byte_after_expected_records_is_not_an_eof() {
    let fixture = one_record();
    let mut reader = fixture.reader();
    assert_eq!(reader.read_next(ignore).unwrap(), ReadProgress::Record);
    OpenOptions::new()
        .append(true)
        .open(&fixture.path.0)
        .unwrap()
        .write_all(&[0x91])
        .unwrap();
    assert_eq!(
        reader.read_next(ignore).unwrap_err().kind,
        FailureKind::TrailingBytes
    );
    assert_eq!(
        reader.summary().physical_bytes,
        reader.summary().expected_bytes + 1
    );
    assert!(!reader.summary().actual_eof);
}

#[test]
fn actual_modified_time_change_at_same_length_prevents_finish() {
    let fixture = one_record();
    let mut reader = fixture.reader();
    reader.read_next(ignore).unwrap();
    OpenOptions::new()
        .write(true)
        .open(&fixture.path.0)
        .unwrap()
        .set_times(FileTimes::new().set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(123)))
        .unwrap();
    assert_eq!(
        reader.read_next(ignore).unwrap_err().kind,
        FailureKind::FileChanged
    );
    assert!(reader.summary().actual_eof);
    assert!(!reader.summary().file_stamp_matches);
}

#[test]
fn replacing_the_path_does_not_substitute_a_new_file_for_the_read_handle() {
    let fixture = one_record();
    let moved = TestPath::new();
    let mut reader = fixture.reader();
    reader.read_next(ignore).unwrap();
    let bytes = fs::read(&fixture.path.0).unwrap();
    fs::rename(&fixture.path.0, &moved.0).unwrap();
    fs::write(&fixture.path.0, bytes).unwrap();
    assert_eq!(
        reader.read_next(ignore).unwrap_err().kind,
        FailureKind::FileChanged
    );
    assert!(!reader.summary().complete);
}

#[test]
fn visitor_refusal_is_sticky_and_preserves_the_original_full_operand() {
    let fixture = one_record();
    let mut reader = fixture.reader();
    let failure = reader
        .read_next(|_| {
            Err(VisitError {
                code: "actual raw writer rejected",
            })
        })
        .unwrap_err();
    assert_eq!(
        failure.kind,
        FailureKind::Visitor("actual raw writer rejected")
    );
    assert_eq!(reader.summary().accepted_records, 0);
    assert_eq!(
        reader.retained_operand(),
        fs::read(&fixture.path.0).unwrap()
    );
    assert_eq!(reader.read_next(ignore).unwrap_err(), failure);
}

#[test]
fn unjoined_or_failed_writer_and_wrong_length_are_refused_at_open() {
    let mut fixture = Fixture::new();
    fixture.control(control::View::Lifecycle("private fixture"), None);
    fixture.actor.cancel();
    let bound = Instant::now() + Duration::from_secs(10);
    while matches!(fixture.actor.poll_finish(), sink::FinishPoll::Pending) {
        assert!(Instant::now() < bound);
        thread::yield_now();
    }
    assert!(matches!(
        ReplayReader::open_joined(
            &fixture.path.0,
            &fixture.actor.terminal().unwrap(),
            &fixture.dictionary
        )
        .err()
        .unwrap()
        .kind,
        FailureKind::WriterNotJoined
    ));
    let fixture = one_record();
    OpenOptions::new()
        .append(true)
        .open(&fixture.path.0)
        .unwrap()
        .write_all(&[0])
        .unwrap();
    assert_eq!(
        ReplayReader::open_joined(
            &fixture.path.0,
            &fixture.actor.terminal().unwrap(),
            &fixture.dictionary
        )
        .err()
        .unwrap()
        .kind,
        FailureKind::Counts
    );
}

fn source_owner() -> InstanceIdentity {
    InstanceIdentity {
        ordinal: 1,
        phase: InstancePhase::InitialSource,
        worker_id: Some(0),
        source_instance_id: Some(99),
    }
}
fn clone_owner() -> InstanceIdentity {
    InstanceIdentity {
        ordinal: 2,
        phase: InstancePhase::StrictClone,
        worker_id: None,
        source_instance_id: None,
    }
}
fn source_identity() -> BounceSourceIdentity {
    BounceSourceIdentity {
        project_session: 11,
        project_fingerprint: 0,
        timeline_revision: 13,
        epoch: 17,
        endpoint_id: 19,
        channel_id: 23,
        instance_id: 99,
    }
}
fn instance_header(value: outer::Header) -> instances::RecordHeader {
    instances::RecordHeader {
        global_sequence: value.sequence,
        monotonic_elapsed_ns: value.monotonic_ns,
    }
}
fn timeline_state() -> TimelineAppState {
    TimelineAppState {
        phase: TimelinePhase::InstallReady,
        project_session: 11,
        engine_session: Some(0),
        sync_session: 13,
        desired_generation: 17,
        observed_fingerprint: 19,
        compiled_generation: Some(23),
        compiled_fingerprint: None,
        source_generation: Some(29),
        source_fingerprint: Some(31),
        sync_source_generation: Some(37),
        desired_seek_serial: 41,
        desired_seek_frame: 43,
        loop_token: Some(0),
        loop_disable_request: Some(47),
        retry_count: 53,
        stale_confirmations: 59,
        confirmed_identity: Some((61, 0)),
        retry_required: true,
        retry_pending: Some((0, 67)),
    }
}
fn transport() -> PluginTransport {
    PluginTransport {
        sample_position: -19,
        quarter_note_position: -0.0,
        tempo: f64::from_bits(0x7ff8_0000_0000_1234),
        playing: true,
        time_sig_numerator: 7,
        time_sig_denominator: 8,
    }
}
struct CompleteFixture {
    transcript: Fixture,
    sidecar_path: TestPath,
    sidecar: state::LiveStateSidecar,
    instances: instances::Dictionary,
    pcm_bits: [u32; 5],
}
fn complete_fixture() -> CompleteFixture {
    let mut fixture = Fixture::new();
    let mut identities = instances::Builder::new();
    fixture.stage = Stage::InitialSource;
    let birth = fixture.control(
        control::View::Lifecycle("instance construction observed"),
        Some(source_owner()),
    );
    identities
        .add_entry(instance_header(birth), source_owner())
        .unwrap();
    let mut slots = state::LiveStateSlots::new(state::RetentionLimits::default()).unwrap();
    let load = fixture.next(2, outer::Lane::State, Some(source_owner()));
    let load_view = slots
        .retain_state_load(load, None, &[0x91, 0x82, 0x73], control::OutcomeRef::Ok)
        .unwrap();
    fixture.actor.emit_state(load, load_view).unwrap();
    fixture.control(
        control::View::BaseRead {
            site: "raw fixture",
            id: 7,
            raw_bits: Ok(0x7ff8_0000_0000_1337),
        },
        Some(source_owner()),
    );
    let metadata = metadata();
    fixture.native(
        native::NativeView::Metadata {
            site: "actual supplied metadata",
            result: Ok(&metadata),
        },
        Some(source_owner()),
    );
    fixture.native(
        native::NativeView::AudioDeclaration {
            inputs: &metadata.input_buses,
            outputs: &metadata.output_buses,
        },
        Some(source_owner()),
    );
    fixture.native(
        native::NativeView::EventDeclaration {
            inputs: &metadata.event_input_buses,
            outputs: &metadata.event_output_buses,
        },
        Some(source_owner()),
    );
    fixture.stage = Stage::SourceStop;
    fixture.app(app::AppView::AudioAsset(&AudioAssetEvent::Registered {
        operation: AudioAssetOperation {
            engine_session: 0,
            generation: 3,
            operation_id: 5,
        },
        id: 7,
        success: true,
    }));
    fixture.app(app::AppView::GeneratorEndpoint(
        &GeneratorEndpointEvent::Removed {
            channel_id: 23,
            endpoint_id: Some(0),
            plugin_instance_id: None,
        },
    ));
    fixture.app(app::AppView::Timeline {
        event: &TimelineRuntimeEvent::Installed {
            request_id: 0,
            revision: 41,
        },
        before: &timeline_state(),
        after: &timeline_state(),
    });
    fixture.control(
        control::View::WorkerCaptureGuard {
            worker_id: Some(0),
            operands: &WorkerCaptureGuard {
                expected_epoch: 11,
                active_epoch: 13,
                requested_epoch: 17,
                input_empty: false,
                parameter_admission: 19,
                accepted: false,
            },
        },
        None,
    );
    fixture.app(app::AppView::Runtime {
        endpoint_id: 19,
        project_session: 11,
        instance_id: 99,
        event: &RuntimeEvent::SlotReady {
            slot: 0,
            name: "real field".into(),
            latency_samples: 3,
            tail_samples: 5,
            loaded_vst3_identity: None,
            loaded_vst3_identity_error: None,
        },
    });
    fixture.control(
        control::View::OrdinaryState {
            endpoint_id: 19,
            project_session: 11,
            instance_id: 99,
            slot: 0,
            request_id: 7,
            epoch: Some(0),
            byte_len: 99,
        },
        None,
    );
    fixture.control(
        control::View::ResetReport {
            frames: 128,
            discarded_prior_midi_events: 61,
            discarded_midi_events: 67,
            discarded_prior_parameter_points: 71,
            discarded_parameter_points: 73,
            output_events_lost: true,
            parameter_output_fault: false,
            process_error: Some("retained actual field"),
        },
        Some(source_owner()),
    );
    let captured = BouncePluginState {
        metadata,
        opaque_state: vec![0x11, 0x22, 0x33, 0x44],
        parameter_bases: BTreeMap::from([(0, -0.0), (7, f32::from_bits(0x7fc0_1337))]),
        native_generation: 29,
        native_dirty_revision: 31,
    };
    let capture = fixture.next(9, outer::Lane::State, None);
    let capture_view = slots
        .retain_capture(
            capture,
            source_owner(),
            state::CaptureOperands {
                label: CaptureLabel::Initial,
                identity: source_identity(),
                receipt_identity: 37,
                state: &captured,
            },
        )
        .unwrap();
    fixture.actor.emit_state(capture, capture_view).unwrap();
    identities
        .record_initial_capture(
            instance_header(capture),
            CaptureLabel::Initial,
            None,
            1,
            37,
            source_identity(),
        )
        .unwrap();
    fixture.stage = Stage::WholeExport;
    let birth = fixture.control(
        control::View::Lifecycle("instance construction observed"),
        Some(clone_owner()),
    );
    identities
        .add_entry(instance_header(birth), clone_owner())
        .unwrap();
    let consumption = fixture.control(
        control::View::StrictCaptureIdentity(&source_identity()),
        Some(clone_owner()),
    );
    identities
        .record_strict_consumption(
            instance_header(consumption),
            Some(clone_owner()),
            1,
            capture.sequence,
            source_identity(),
        )
        .unwrap();
    fixture.native(
        native::NativeView::Midi {
            data: [0x90, 60, 100],
            offset: 4,
        },
        Some(clone_owner()),
    );
    fixture.native(
        native::NativeView::Call {
            operation: native::NativeOperation::Midi,
            boundary: native::Boundary::Attempt,
            frames: None,
            transport: None,
            outcome: None,
        },
        Some(clone_owner()),
    );
    fixture.native(
        native::NativeView::Call {
            operation: native::NativeOperation::Midi,
            boundary: native::Boundary::Returned,
            frames: None,
            transport: None,
            outcome: Some(&CallOutcome::Ok),
        },
        Some(clone_owner()),
    );
    let transport = transport();
    fixture.native(
        native::NativeView::Call {
            operation: native::NativeOperation::Process,
            boundary: native::Boundary::Attempt,
            frames: Some(5),
            transport: Some(&transport),
            outcome: None,
        },
        Some(clone_owner()),
    );
    fixture.native(
        native::NativeView::Call {
            operation: native::NativeOperation::Process,
            boundary: native::Boundary::Returned,
            frames: Some(5),
            transport: Some(&transport),
            outcome: Some(&CallOutcome::Ok),
        },
        Some(clone_owner()),
    );
    let pcm_bits = [0, 0x8000_0000, 0x7fc0_1234, 0x7f80_0000, 0xff80_0000];
    let channels = [
        pcm_bits.map(f32::from_bits).to_vec(),
        pcm_bits.map(f32::from_bits).to_vec(),
    ];
    fixture.native(
        native::NativeView::StrictPcm {
            frames: 5,
            transport: &transport,
            channels: &channels,
            transport_applied: true,
            output_events: 43,
            output_events_lost: true,
        },
        Some(clone_owner()),
    );
    fixture.control(
        control::View::Call {
            operation: Operation::StrictDispose,
            boundary: Boundary::Attempt,
            frames: None,
            transport: None,
            outcome: None,
        },
        Some(clone_owner()),
    );
    fixture.control(
        control::View::Disposal {
            terminated: true,
            error: None,
            diagnostics: &[],
        },
        Some(clone_owner()),
    );
    fixture.control(
        control::View::RenderReport(&RenderReport {
            identity: source_identity(),
            origin_frame: 19,
            musical_frames: 23,
            frames: 29,
            native_latency: 31,
            reported_tail: 37,
            fixed_tail: 41,
            generated_frames: 43,
            sample_peak: f32::from_bits(0x7fc0_0137),
        }),
        None,
    );
    fixture.control(
        control::View::Worker {
            site: "worker fields",
            worker_id: 0,
            manifest: &PluginEndpointManifest::identified(&[99]).unwrap(),
            epoch: 17,
            sequence: Some(0),
            stats: &BridgeStats::default(),
        },
        None,
    );
    fixture.stage = Stage::Cleanup;
    fixture.control(
        control::View::WorkerJoin {
            worker_id: 0,
            outcome: WorkerJoinOutcome::Joined,
        },
        None,
    );
    fixture.control(
        control::View::HookInventory {
            site: "inactive supplied field",
            inactive: true,
        },
        None,
    );
    fixture.stage = Stage::Finished;
    fixture.control(
        control::View::Lifecycle("renderer backend destruction returned"),
        None,
    );
    fixture.seal();
    let sidecar_path = TestPath::new();
    let sidecar = slots
        .into_batch()
        .unwrap()
        .start(state::SidecarConfig {
            absolute_path: sidecar_path.0.clone(),
            max_blob_bytes: state::MAX_BLOB_BYTES,
            max_blobs: state::MAX_BLOB_COUNT,
            max_owned_capacity_bytes: state::MAX_OWNED_CAPACITY_BYTES,
            max_archive_bytes: state::MAX_ARCHIVE_BYTES,
        })
        .unwrap();
    CompleteFixture {
        transcript: fixture,
        sidecar_path,
        sidecar,
        instances: identities.freeze().unwrap(),
        pcm_bits,
    }
}
fn bind_states(reader: &mut ReplayReader<'_>, sidecar: &mut state::LiveStateSidecar) {
    let bound = Instant::now() + Duration::from_secs(10);
    loop {
        match sidecar.poll_join() {
            state::LiveJoinPoll::Pending => {
                assert!(Instant::now() < bound);
                thread::yield_now();
            }
            state::LiveJoinPoll::Joined { terminal, linkage } => {
                assert!(terminal.actual_writer_joined && terminal.failure.is_none());
                reader.bind_state_ranges(linkage.unwrap()).unwrap();
                return;
            }
        }
    }
}

#[test]
fn all_twenty_three_kinds_visit_complete_fields_and_bind_real_sidecar_ranges() {
    let mut fixture = complete_fixture();
    let mut reader = fixture.transcript.reader();
    reader.bind_instances(&fixture.instances).unwrap();
    bind_states(&mut reader, &mut fixture.sidecar);
    let mut kinds = [false; 24];
    let mut capture_seen = false;
    let mut pcm_seen = false;
    while reader
        .read_next(|record| {
            kinds[record.header.kind as usize] = true;
            match record.payload {
                Payload::State(state::StateBody::Capture {
                    link,
                    receipt_identity,
                    metadata,
                    parameter_bases,
                    native_generation,
                    native_dirty_revision,
                    ..
                }) => {
                    assert!(record.header.instance.is_none());
                    assert_eq!(link.supplied_owner(), source_owner());
                    assert_eq!(receipt_identity, 37);
                    assert_eq!(metadata.class_uid, "fixture class");
                    assert_eq!(
                        parameter_bases.items().collect::<Vec<_>>(),
                        [(0, 0x8000_0000), (7, 0x7fc0_1337)]
                    );
                    assert_eq!((native_generation, native_dirty_revision), (29, 31));
                    capture_seen = true;
                }
                Payload::Native(native::NativeBody::StrictPcm(pcm)) => {
                    assert_eq!(record.header.instance, Some(clone_owner()));
                    assert_eq!(pcm.channel_count(), 2);
                    for channel in pcm.channels() {
                        assert_eq!(channel.sample_bits().collect::<Vec<_>>(), fixture.pcm_bits);
                    }
                    assert_eq!(pcm.transport.quarter_note_bits, (-0.0_f64).to_bits());
                    assert_eq!(pcm.transport.tempo_bits, 0x7ff8_0000_0000_1234);
                    assert_eq!(pcm.output_events, 43);
                    assert!(pcm.output_events_lost);
                    pcm_seen = true;
                }
                Payload::App(app::AppControl::Timeline { before, after, .. }) => {
                    assert_eq!(before, timeline_state());
                    assert_eq!(after, timeline_state());
                }
                Payload::App(app::AppControl::GeneratorEndpoint(
                    GeneratorEndpointEvent::Removed {
                        endpoint_id,
                        plugin_instance_id,
                        ..
                    },
                )) => {
                    assert_eq!(endpoint_id, Some(0));
                    assert_eq!(plugin_instance_id, None);
                }
                Payload::Control(control::Control::Worker {
                    sequence, manifest, ..
                }) => {
                    assert_eq!(sequence, Some(0));
                    assert_eq!(manifest.instance_ids[0], Some(99));
                }
                _ => {}
            }
            Ok(())
        })
        .unwrap()
        == ReadProgress::Record
    {}
    assert!(kinds[1..].iter().all(|seen| *seen));
    assert!(capture_seen && pcm_seen && reader.summary().complete);
    assert!(fixture.sidecar_path.0.exists());
    assert_eq!(
        reader.summary().births[0].unwrap().identity.worker_id,
        Some(0)
    );
    assert_eq!(reader.summary().births[1].unwrap().identity.worker_id, None);
    assert_eq!(reader.summary().births[0].unwrap().worker_joined_records, 1);
    assert_eq!(
        reader.summary().births[1]
            .unwrap()
            .terminated_without_error_records,
        1
    );
    assert_eq!(reader.summary().strict_midi_records, 1);
    assert_eq!(reader.summary().strict_process_attempts, 1);
    assert_eq!(reader.summary().strict_process_ok_returns, 1);
    assert_eq!(reader.summary().strict_pcm_records, 1);
    assert_eq!(
        (
            reader.summary().strict_pcm_frames,
            reader.summary().strict_pcm_samples
        ),
        (5, 10)
    );
    let report = reader.summary().reports[0].unwrap();
    assert_eq!(report.header.kind, 19);
    assert_eq!(report.identity, source_identity());
    assert_eq!(
        (report.origin_frame, report.musical_frames, report.frames),
        (19, 23, 29)
    );
    assert_eq!(
        (
            report.native_latency,
            report.reported_tail,
            report.fixed_tail
        ),
        (31, 37, 41)
    );
    assert_eq!(report.generated_frames, 43);
    assert_eq!(report.sample_peak_bits, 0x7fc0_0137);
}

#[test]
fn state_range_binding_can_arrive_after_reading_without_rewriting_capture_identity() {
    let mut fixture = complete_fixture();
    let mut reader = fixture.transcript.reader();
    reader.bind_instances(&fixture.instances).unwrap();
    for _ in 0..fixture.transcript.sequence {
        assert_eq!(reader.read_next(ignore).unwrap(), ReadProgress::Record);
    }
    assert!(!reader.summary().complete);
    bind_states(&mut reader, &mut fixture.sidecar);
    assert_eq!(reader.read_next(ignore).unwrap(), ReadProgress::Finished);
    assert!(reader.summary().state_links_match && reader.summary().instance_links_match);
}

#[test]
fn missing_joined_state_ranges_prevent_successful_eof_seal() {
    let mut fixture = complete_fixture();
    let mut reader = fixture.transcript.reader();
    reader.bind_instances(&fixture.instances).unwrap();
    for _ in 0..fixture.transcript.sequence {
        reader.read_next(ignore).unwrap();
    }
    assert_eq!(
        reader.read_next(ignore).unwrap_err().kind,
        FailureKind::StateLinkage
    );
    // Retire this synthetic sidecar even though the transcript rejection is sticky.
    let bound = Instant::now() + Duration::from_secs(10);
    while matches!(fixture.sidecar.poll_join(), state::LiveJoinPoll::Pending) {
        assert!(Instant::now() < bound);
        thread::yield_now();
    }
    assert!(!reader.summary().complete);
}

#[test]
fn every_state_reference_field_is_bound_exactly_and_none_is_not_some_zero() {
    let mut fixture = complete_fixture();
    let bound = Instant::now() + Duration::from_secs(10);
    let (links, original_ranges) = loop {
        match fixture.sidecar.poll_join() {
            state::LiveJoinPoll::Pending => {
                assert!(Instant::now() < bound);
                thread::yield_now();
            }
            state::LiveJoinPoll::Joined { terminal, linkage } => {
                assert!(terminal.actual_writer_joined);
                let linkage = linkage.unwrap();
                break (*linkage.links, linkage.ranges.to_vec());
            }
        }
    };
    for changed in 0..8 {
        let mut ranges = original_ranges.clone();
        match changed {
            0 => ranges[0].identity.worker_id = None,
            1 => ranges[0].identity.source_id = Some(0),
            2 => ranges[0].identity.instance_ordinal += 1,
            3 => ranges[0].identity.record_ordinal += 1,
            4 => ranges[0].identity.monotonic_elapsed_ns += 1,
            5 => ranges[0].identity.phase = state::SuppliedPhase::StrictClone,
            6 => ranges[0].length += 1,
            7 => ranges[0].original_owned_capacity += 1,
            _ => unreachable!(),
        }
        let mut reader = fixture.transcript.reader();
        assert_eq!(
            reader
                .bind_state_ranges(state::LinkedRanges {
                    links: &links,
                    ranges: &ranges
                })
                .unwrap_err()
                .kind,
            FailureKind::StateLinkage
        );
        assert!(
            reader
                .bind_state_ranges(state::LinkedRanges {
                    links: &links,
                    ranges: &original_ranges
                })
                .is_err()
        );
        assert!(!reader.summary().complete);
    }
}

#[test]
fn instance_dictionary_cannot_rewrite_a_real_birth_worker_some_zero_as_none() {
    let mut fixture = complete_fixture();
    let original = &fixture.instances;
    let capture = original.capture().unwrap();
    let consumption = original.consumption().unwrap();
    let mut replacement = instances::Builder::new();
    let mut source = source_owner();
    source.worker_id = None;
    replacement
        .add_entry(original.entry(1).unwrap().construction, source)
        .unwrap();
    replacement
        .record_initial_capture(
            capture.header,
            CaptureLabel::Initial,
            None,
            capture.known_owner_source_ordinal,
            capture.receipt_identity,
            capture.source,
        )
        .unwrap();
    replacement
        .add_entry(original.entry(2).unwrap().construction, clone_owner())
        .unwrap();
    replacement
        .record_strict_consumption(
            consumption.header,
            Some(clone_owner()),
            consumption.known_owner_source_ordinal,
            consumption.captured_record_sequence,
            consumption.source,
        )
        .unwrap();
    let replacement = replacement.freeze().unwrap();
    let mut reader = fixture.transcript.reader();
    reader.bind_instances(&replacement).unwrap();
    bind_states(&mut reader, &mut fixture.sidecar);
    for _ in 0..fixture.transcript.sequence {
        reader.read_next(ignore).unwrap();
    }
    assert_eq!(
        reader.read_next(ignore).unwrap_err().kind,
        FailureKind::InstanceLinkage
    );
    assert!(!reader.summary().complete);
}

fn record_positions(bytes: &[u8]) -> Vec<usize> {
    let mut offset = 0;
    let mut result = Vec::new();
    while offset < bytes.len() {
        result.push(offset);
        offset += u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize + 4;
    }
    result
}

#[test]
fn independently_changed_strict_return_frames_cannot_match_process_attempt() {
    let mut fixture = complete_fixture();
    let bytes = fs::read(&fixture.transcript.path.0).unwrap();
    let offset = record_positions(&bytes)
        .into_iter()
        .find(|&offset| {
            let length =
                u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize + 4;
            let (header, body) = outer::decode(&bytes[offset..offset + length]).unwrap();
            header.lane == outer::Lane::Native(11) && body[0] == 4 && body[1] == 1
        })
        .unwrap();
    let length = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize + 4;
    let (_, body) = outer::decode(&bytes[offset..offset + length]).unwrap();
    let body_offset = offset + length - body.len();
    // Native call v1: operation/boundary/framesflag then exact u64 frames.
    mutate(
        &fixture.transcript.path.0,
        (body_offset + 3) as u64,
        &6_u64.to_le_bytes(),
    );
    let mut reader = fixture.transcript.reader();
    reader.bind_instances(&fixture.instances).unwrap();
    bind_states(&mut reader, &mut fixture.sidecar);
    let error = loop {
        match reader.read_next(ignore) {
            Ok(ReadProgress::Record) => {}
            Ok(ReadProgress::Finished) => panic!("mismatched return sealed"),
            Err(error) => break error,
        }
    };
    assert_eq!(error.kind, FailureKind::CallOrdering);
    assert!(!reader.summary().complete);
}

#[test]
fn successful_process_without_its_actual_pcm_record_cannot_finish() {
    let mut fixture = Fixture::new();
    let mut instances = instances::Builder::new();
    fixture.stage = Stage::InitialSource;
    let birth = fixture.control(
        control::View::Lifecycle("instance construction observed"),
        Some(source_owner()),
    );
    instances
        .add_entry(instance_header(birth), source_owner())
        .unwrap();
    fixture.stage = Stage::WholeExport;
    let birth = fixture.control(
        control::View::Lifecycle("instance construction observed"),
        Some(clone_owner()),
    );
    instances
        .add_entry(instance_header(birth), clone_owner())
        .unwrap();
    let transport = transport();
    fixture.native(
        native::NativeView::Call {
            operation: native::NativeOperation::Process,
            boundary: native::Boundary::Attempt,
            frames: Some(5),
            transport: Some(&transport),
            outcome: None,
        },
        Some(clone_owner()),
    );
    fixture.native(
        native::NativeView::Call {
            operation: native::NativeOperation::Process,
            boundary: native::Boundary::Returned,
            frames: Some(5),
            transport: Some(&transport),
            outcome: Some(&CallOutcome::Ok),
        },
        Some(clone_owner()),
    );
    fixture.seal();
    let mut reader = fixture.reader();
    reader.bind_instances(&instances.freeze().unwrap()).unwrap();
    for _ in 0..fixture.sequence {
        reader.read_next(ignore).unwrap();
    }
    assert_eq!(
        reader.read_next(ignore).unwrap_err().kind,
        FailureKind::CallOrdering
    );
    assert!(!reader.summary().complete);
}

#[test]
fn actual_birth_is_required_before_a_stamped_instance_can_supply_records() {
    let mut fixture = Fixture::new();
    fixture.control(
        control::View::Lifecycle("unbound field"),
        Some(source_owner()),
    );
    fixture.seal();
    let mut reader = fixture.reader();
    assert_eq!(
        reader.read_next(ignore).unwrap_err().kind,
        FailureKind::InstanceLinkage
    );
    assert!(!reader.summary().complete);
}
