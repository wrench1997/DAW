//! Real coordinator + dedicated worker capture kernel, with a PluginBackend
//! stand-in, then the actual occurrence renderer/writer/verifier/candidate join.
//! No VST binary, SDK, source worker thread, device or UI window is invoked.
use super::*;
use crate::{
    app::clip_render::{test_commit_verified_occurrence, test_prepare_occurrence_request},
    export_job::ExportControl,
    instrument_bounce::{
        CapturedInstrumentBounce,
        render::{
            BackendFailure, BlockReply, DisposalObservation, OccurrenceRenderOutcome,
            RenderAdmission, RenderBackend, RenderControl,
        },
    },
    occurrence_request::{OccurrenceRenderRequest, OccurrenceStamp},
};
use std::{path::PathBuf, sync::Mutex};

static NEXT_JOIN: AtomicU64 = AtomicU64::new(1);
struct Files(PathBuf);
impl Files {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "citrus-occurrence-join-{}-{}",
            std::process::id(),
            NEXT_JOIN.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> PathBuf {
        self.0.join("render.wav")
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        if !std::thread::panicking() && !crate::occurrence_media::verification_test_has_pending() {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
fn fixture(
    a: u64,
    n: u64,
    latency: u32,
) -> (
    crate::model::Project,
    Arc<OccurrenceRenderRequest>,
    BouncePluginState,
) {
    let mut p = project();
    p.tempo = 120.0;
    p.song_length_beats = 4.0;
    p.channels.truncate(1);
    p.channels[0].muted = false;
    p.channels[0].pan = 0.0;
    p.channels[0].volume = 0.75;
    p.plugin_instances[0].parameters.insert(17, 0.25);
    p.patterns.truncate(1);
    p.patterns[0].length_steps = 4;
    p.patterns[0].notes.clear();
    p.patterns[0].channel_steps = vec![[false; 16]];
    p.patterns[0].channel_steps[0][0] = true;
    p.clips.truncate(1);
    p.clips[0].kind = crate::model::ClipKind::Pattern;
    p.clips[0].muted = false;
    p.clips[0].start = a as f32 / 4000.0;
    p.clips[0].length = n as f32 / 4000.0;
    p.clips[0].source_offset = 0.0;
    p.clips[0].pattern_id = p.patterns[0].id;
    let stamp = OccurrenceStamp {
        project_session: 2,
        project_revision: 3,
        timeline_revision: 4,
        epoch: 7,
        engine_session: 5,
        endpoint_id: 19,
        channel_id: p.channels[0].id,
        instance_id: 41,
        mixer_track_id: p.channels[0].mixer_track,
        native_generation: 2,
        app_native_dirty_revision: 1,
        native_capture_serial: 0,
        parameter_capture_serial: 0,
        native_control_request: 10,
        parameter_control_request: 11,
    };
    let request = test_prepare_occurrence_request(&p, stamp, p.clips[0].id, 8000, 7).unwrap();
    let mut state = state();
    state.metadata.latency_samples = latency;
    assert_ne!(state.native_dirty_revision, stamp.app_native_dirty_revision);
    (p, request, state)
}

fn capture_actual(
    project: &crate::model::Project,
    request: Arc<OccurrenceRenderRequest>,
    state: BouncePluginState,
    control: &PluginChainControl,
    commands: &Receiver<QueuedAdminCommand>,
    calls: Arc<AtomicU64>,
) -> CapturedInstrumentBounce {
    let plan = InstrumentBouncePlan::prepare_occurrence(project, request).unwrap();
    assert_eq!(plan.retained_project_state_bytes(), 0);
    let identity = identity(&plan);
    let now = Instant::now();
    let mut coordinator =
        InstrumentBounceCapture::begin(plan, identity, status(), control, now).unwrap();
    assert!(coordinator.poll(identity, status(), now).unwrap().is_none());
    let mut queued = commands.try_recv().unwrap();
    let AdminCommand::CaptureInstrumentBounce {
        expected_epoch,
        parameter_ids,
        parameter_admission,
        lease,
        reply,
    } = queued.take_command()
    else {
        panic!("not the dedicated capture request")
    };
    let before = control.metrics.native_editors.lock().unwrap()[0].clone();
    let mut slots = vec![slot(Ok(state), calls, None)];
    let (mut events, mut rx) = RingBuffer::new(4);
    let result = capture_instrument_bounce_on_worker(
        &lease.fence,
        expected_epoch,
        identity.epoch,
        &AtomicU64::new(identity.epoch),
        true,
        &parameter_ids,
        &parameter_admission,
        &mut slots,
        &mut events,
        &control.metrics,
    );
    reply.send(result).unwrap();
    assert!(
        coordinator.poll(identity, status(), now).unwrap().is_none(),
        "reply is not termination"
    );
    assert!(!coordinator.termination_acknowledged());
    drop(lease);
    assert!(coordinator.termination_acknowledged());
    let captured = coordinator.poll(identity, status(), now).unwrap().unwrap();
    assert!(rx.pop().is_err());
    assert!(commands.try_recv().is_err());
    let after = control.metrics.native_editors.lock().unwrap()[0].clone();
    assert_eq!(before.captured_state, after.captured_state);
    assert_eq!(before.capture_serial, after.capture_serial);
    assert_eq!(
        before.parameter_capture_serial,
        after.parameter_capture_serial
    );
    assert_eq!(control.stats().faults, 0);
    captured
}

#[derive(Default)]
struct Ledger {
    transport: Vec<PluginTransport>,
    midi: Vec<(usize, [u8; 3], u16)>,
    metadata_calls: usize,
    disposed: bool,
    dropped: bool,
}
struct Backend {
    metadata: BouncePluginMetadata,
    ledger: Arc<Mutex<Ledger>>,
    frame: usize,
    cancel_after_first: Option<ExportControl>,
}
impl Drop for Backend {
    fn drop(&mut self) {
        let mut log = self.ledger.lock().unwrap();
        assert!(log.disposed);
        log.dropped = true;
    }
}
impl RenderBackend for Backend {
    fn prepare(
        &mut self,
        capture: &CapturedInstrumentBounce,
        _: &RenderControl,
    ) -> Result<BouncePluginMetadata, BackendFailure> {
        assert_eq!(capture.metadata(), &self.metadata);
        Ok(self.metadata.clone())
    }
    fn metadata(&mut self) -> Result<BouncePluginMetadata, BackendFailure> {
        self.ledger.lock().unwrap().metadata_calls += 1;
        Ok(self.metadata.clone())
    }
    fn send_midi(&mut self, midi: [u8; 3], offset: u16) -> Result<(), BackendFailure> {
        self.ledger
            .lock()
            .unwrap()
            .midi
            .push((self.frame, midi, offset));
        Ok(())
    }
    fn process(
        &mut self,
        transport: PluginTransport,
        frames: usize,
    ) -> Result<BlockReply, BackendFailure> {
        assert_eq!(frames, 128);
        self.ledger.lock().unwrap().transport.push(transport);
        let left = (self.frame..self.frame + frames)
            .map(|f| f as f32 / 1024.0)
            .collect::<Vec<_>>();
        let right = left.iter().map(|f| -*f).collect();
        self.frame += frames;
        if let Some(control) = self.cancel_after_first.take() {
            assert!(control.cancel());
        }
        Ok(BlockReply {
            channels: vec![left, right],
            transport_applied: true,
            output_events: 0,
            output_events_lost: false,
        })
    }
    fn dispose(&mut self, _: Instant) -> DisposalObservation {
        self.ledger.lock().unwrap().disposed = true;
        DisposalObservation {
            terminated: true,
            error: None,
            diagnostics: vec![],
        }
    }
}
fn render(
    capture: CapturedInstrumentBounce,
    files: &Files,
) -> (
    crate::occurrence_media::VerifiedOccurrenceMedia,
    crate::instrument_bounce::render::RenderReport,
    Arc<Mutex<Ledger>>,
) {
    let request = Arc::clone(capture.plan().occurrence_request().unwrap());
    let metadata = capture.metadata().clone();
    let log = Arc::new(Mutex::new(Ledger::default()));
    let worker_log = Arc::clone(&log);
    let admission = RenderAdmission::default();
    let control = RenderControl {
        export: ExportControl::default(),
        deadline: Instant::now() + Duration::from_secs(8),
    };
    let receipt = admission
        .start_occurrence(capture, files.path(), control, move || Backend {
            metadata,
            ledger: worker_log,
            frame: 0,
            cancel_after_first: None,
        })
        .unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    while !receipt.termination_acknowledged() {
        assert!(Instant::now() < until, "mock occurrence renderer deadline");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(!admission.is_busy());
    let OccurrenceRenderOutcome::Complete { report, published } = receipt.try_recv().unwrap()
    else {
        panic!("occurrence did not publish")
    };
    let log_guard = log.lock().unwrap();
    assert!(log_guard.disposed && log_guard.dropped);
    for (index, transport) in log_guard.transport.iter().enumerate() {
        let absolute = request.start_frame() + index as u64 * 128;
        assert_eq!(transport.sample_position, absolute as i64);
        assert_eq!(
            transport.quarter_note_position,
            absolute as f64 * 120.0 / (8000.0 * 60.0)
        );
        assert!(transport.playing);
    }
    assert_eq!(log_guard.midi.len(), 2);
    assert_eq!(log_guard.midi[0].0 + usize::from(log_guard.midi[0].2), 0);
    assert_eq!(
        log_guard.midi[1].0 + usize::from(log_guard.midi[1].2),
        request.song_frames() as usize
    );
    drop(log_guard);
    let verification = crate::occurrence_media::VerificationAdmission::default();
    let mut verification_receipt = verification.start(published).unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    let outcome = loop {
        if let Some(outcome) = verification_receipt.poll() {
            break outcome;
        }
        assert!(
            Instant::now() < until,
            "occurrence verification join watchdog"
        );
        std::thread::sleep(Duration::from_millis(1));
    };
    assert!(verification_receipt.termination_acknowledged());
    assert!(!verification.is_busy());
    let crate::occurrence_media::VerificationOutcome::Verified(media) = outcome else {
        panic!("published occurrence verification failed");
    };
    assert_eq!(
        media.asset().samples[0].to_bits(),
        (report.native_latency as f32 / 1024.0).to_bits()
    );
    (media, report, log)
}

#[test]
fn occurrence_actual_capture_renderer_file_and_authoritative_history_join() {
    let _serial = crate::occurrence_media::verification_test_guard();
    for (a, n, latency) in [
        (0, 1, 0),
        (1, 127, 1),
        (127, 128, 127),
        (128, 129, 128),
        (129, 257, 129),
    ] {
        let files = Files::new();
        let (mut project, request, state) = fixture(a, n, latency);
        let original = serde_json::to_vec(&project).unwrap();
        let (control, commands) = control(2);
        let calls = Arc::new(AtomicU64::new(0));
        let first = capture_actual(
            &project,
            Arc::clone(&request),
            state.clone(),
            &control,
            &commands,
            Arc::clone(&calls),
        );
        let attestation = first.retain_occurrence_attestation().unwrap();
        assert_eq!(attestation.provenance().native_dirty_revision(), 9);
        assert_eq!(request.stamp().app_native_dirty_revision, 1);
        let (media, report, _) = render(first, &files);
        assert_eq!(report.origin_frame, a);
        assert_eq!(report.musical_frames, n);
        assert_eq!(report.frames, n as usize + 7);
        let second = capture_actual(
            &project,
            request,
            state,
            &control,
            &commands,
            Arc::clone(&calls),
        );
        let matched = attestation.match_recapture(second).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(matched.provenance().same_capture(media.provenance()));
        assert_eq!(serde_json::to_vec(&project).unwrap(), original);
        let published = test_commit_verified_occurrence(&mut project, &media, &matched).unwrap();
        assert_eq!(published, files.path());
        assert!(published.exists());
    }
}

#[test]
fn occurrence_selected_actual_capture_renderer_preserves_all_declarations_through_history_join() {
    let _serial = crate::occurrence_media::verification_test_guard();
    let files = Files::new();
    let (mut project, request, mut state) = fixture(129, 129, 1);
    state.metadata.class_uid = "5653544E694B386B6F6E74616B742038".into();
    state.metadata.version = "8.4.1".into();
    state.metadata.output_buses = (0..32)
        .map(|index| BounceAudioBus::new(2, index % 3 == 0, Some(index == 0)))
        .collect();
    state.metadata.event_output_buses = vec![(16, true, Some(true))];
    state.metadata.validate(8000).unwrap();
    let metadata = state.metadata.clone();
    let before = serde_json::to_vec(&project).unwrap();
    let (control, commands) = control(2);
    let calls = Arc::new(AtomicU64::new(0));
    let first = capture_actual(
        &project,
        Arc::clone(&request),
        state.clone(),
        &control,
        &commands,
        Arc::clone(&calls),
    );
    let attestation = first.retain_occurrence_attestation().unwrap();
    assert_eq!(attestation.provenance().metadata(), &metadata);
    let (media, _, _) = render(first, &files);
    assert_eq!(media.provenance().metadata(), &metadata);
    let second = capture_actual(
        &project,
        request,
        state,
        &control,
        &commands,
        Arc::clone(&calls),
    );
    let matched = attestation.match_recapture(second).unwrap();
    assert_eq!(matched.metadata(), &metadata);
    assert_eq!(matched.metadata().output_buses.len(), 32);
    assert_eq!(
        matched.metadata().event_output_buses,
        [(16, true, Some(true))]
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(serde_json::to_vec(&project).unwrap(), before);
    let published = test_commit_verified_occurrence(&mut project, &media, &matched).unwrap();
    assert_eq!(published, files.path());
    assert!(published.exists());
}

#[test]
fn occurrence_second_actual_capture_drift_keeps_source_and_published_media() {
    let _serial = crate::occurrence_media::verification_test_guard();
    for changed_field in 0..5 {
        let files = Files::new();
        let (project, request, state) = fixture(129, 129, 1);
        let before = serde_json::to_vec(&project).unwrap();
        let (control, commands) = control(2);
        let calls = Arc::new(AtomicU64::new(0));
        let first = capture_actual(
            &project,
            Arc::clone(&request),
            state.clone(),
            &control,
            &commands,
            Arc::clone(&calls),
        );
        let attestation = first.retain_occurrence_attestation().unwrap();
        let (media, _, _) = render(first, &files);
        let mut changed = state;
        match changed_field {
            0 => changed.opaque_state.push(4),
            1 => {
                changed.parameter_bases.insert(17, 0.5);
            }
            2 => changed.metadata.tail_samples = 1,
            3 => changed.native_dirty_revision += 1,
            4 => changed.native_generation += 1,
            _ => unreachable!(),
        }
        let second = capture_actual(
            &project,
            request,
            changed,
            &control,
            &commands,
            Arc::clone(&calls),
        );
        let error = attestation.match_recapture(second).unwrap_err();
        let expected = if changed_field == 4 {
            "Actual occurrence capture differs from its reviewed source/runtime stamp"
        } else {
            "The second actual source capture differs from the consumed render capture"
        };
        assert_eq!(error, expected, "changed field {changed_field}");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(serde_json::to_vec(&project).unwrap(), before);
        assert!(media.path().exists());
    }
}

#[test]
fn occurrence_capture_attestation_refuses_original_receipt_and_other_request() {
    for reuse_original in [true, false] {
        let (project, request, state) = fixture(128, 129, 0);
        let (control, commands) = control(2);
        let calls = Arc::new(AtomicU64::new(0));
        let first = capture_actual(
            &project,
            Arc::clone(&request),
            state.clone(),
            &control,
            &commands,
            Arc::clone(&calls),
        );
        let attestation = first.retain_occurrence_attestation().unwrap();
        let second = if reuse_original {
            first
        } else {
            drop(first);
            let separate_request = test_prepare_occurrence_request(
                &project,
                request.stamp(),
                request.clip_id(),
                8000,
                7,
            )
            .unwrap();
            assert!(!Arc::ptr_eq(&request, &separate_request));
            capture_actual(
                &project,
                separate_request,
                state,
                &control,
                &commands,
                Arc::clone(&calls),
            )
        };
        assert_eq!(
            attestation.match_recapture(second).unwrap_err(),
            "The second actual source capture differs from the consumed render capture"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            if reuse_original { 1 } else { 2 }
        );
    }
}

#[test]
fn occurrence_request_refuses_changed_snapshot_and_changed_compiled_schedule() {
    let (mut project, request, _) = fixture(128, 129, 0);
    let snapshot = serde_json::to_vec(&project).unwrap();
    project.name.push('x');
    assert_eq!(
        InstrumentBouncePlan::prepare_occurrence(&project, Arc::clone(&request))
            .err()
            .unwrap(),
        "Project differs from the reviewed occurrence snapshot"
    );
    project = serde_json::from_slice(&snapshot).unwrap();
    let plan = InstrumentBouncePlan::prepare_occurrence(&project, Arc::clone(&request)).unwrap();
    let mut events = request.local_events().to_vec();
    assert_eq!(events[0].frame, 0);
    events[0].frame = 1;
    let changed = OccurrenceRenderRequest::new(
        Arc::clone(request.consent()),
        request.pattern_id(),
        request.channel_id(),
        request.instance_id(),
        request.start_frame(),
        request.end_frame(),
        plan.timeline().duration_frames(),
        request.tempo(),
        events.into(),
    )
    .unwrap();
    assert_eq!(
        InstrumentBouncePlan::prepare_occurrence(&project, changed)
            .err()
            .unwrap(),
        "Occurrence schedule omits or changes an original compiled event"
    );
    assert_eq!(serde_json::to_vec(&project).unwrap(), snapshot);
}

#[test]
fn occurrence_cancel_before_factory_or_after_first_reply_retains_source_and_destination() {
    for before_factory in [true, false] {
        let files = Files::new();
        std::fs::write(files.path(), b"existing destination").unwrap();
        let (project, request, state) = fixture(129, 129, 1);
        let source_before = serde_json::to_vec(&project).unwrap();
        let (control, commands) = control(2);
        let capture = capture_actual(
            &project,
            request,
            state,
            &control,
            &commands,
            Arc::new(AtomicU64::new(0)),
        );
        let metadata = capture.metadata().clone();
        let log = Arc::new(Mutex::new(Ledger::default()));
        let worker_log = Arc::clone(&log);
        let constructed = Arc::new(AtomicBool::new(false));
        let worker_constructed = Arc::clone(&constructed);
        let export = ExportControl::default();
        if before_factory {
            assert!(export.cancel());
        }
        let worker_export = export.clone();
        let admission = RenderAdmission::default();
        let receipt = admission
            .start_occurrence(
                capture,
                files.path(),
                RenderControl {
                    export,
                    deadline: Instant::now() + Duration::from_secs(8),
                },
                move || {
                    worker_constructed.store(true, Ordering::Release);
                    Backend {
                        metadata,
                        ledger: worker_log,
                        frame: 0,
                        cancel_after_first: Some(worker_export),
                    }
                },
            )
            .unwrap();
        let until = Instant::now() + Duration::from_secs(10);
        while !receipt.termination_acknowledged() {
            assert!(Instant::now() < until, "occurrence cancellation watchdog");
            std::thread::sleep(Duration::from_millis(1));
        }
        let OccurrenceRenderOutcome::RenderFailed(failure) = receipt.try_recv().unwrap() else {
            panic!("cancelled occurrence did not preserve render failure category");
        };
        assert_eq!(
            failure.kind,
            crate::instrument_bounce::render::FailureKind::Cancelled
        );
        assert!(!admission.is_busy());
        assert_eq!(constructed.load(Ordering::Acquire), !before_factory);
        let log = log.lock().unwrap();
        if before_factory {
            assert!(log.transport.is_empty() && log.midi.is_empty());
            assert!(!log.disposed && !log.dropped);
        } else {
            assert_eq!(log.transport.len(), 1);
            assert_eq!(
                log.metadata_calls, 1,
                "no post-Process metadata after cancellation"
            );
            assert_eq!(log.midi.len(), 1);
            assert!(log.disposed && log.dropped);
        }
        assert_eq!(serde_json::to_vec(&project).unwrap(), source_before);
        assert_eq!(
            std::fs::read(files.path()).unwrap(),
            b"existing destination"
        );
        assert_eq!(std::fs::read_dir(&files.0).unwrap().count(), 1);
    }
}

#[test]
fn occurrence_second_capture_cancel_timeout_and_fault_are_drained_without_insertion() {
    let _serial = crate::occurrence_media::verification_test_guard();
    for case in 0..3 {
        let files = Files::new();
        let (project, request, state) = fixture(129, 129, 1);
        let original = serde_json::to_vec(&project).unwrap();
        let (control, commands) = control(2);
        let calls = Arc::new(AtomicU64::new(0));
        let first = capture_actual(
            &project,
            Arc::clone(&request),
            state.clone(),
            &control,
            &commands,
            Arc::clone(&calls),
        );
        let attestation = first.retain_occurrence_attestation().unwrap();
        let (media, _, _) = render(first, &files);
        let plan = InstrumentBouncePlan::prepare_occurrence(&project, request).unwrap();
        let stamp = identity(&plan);
        let now = Instant::now();
        let mut second =
            InstrumentBounceCapture::begin(plan, stamp, status(), &control, now).unwrap();
        let mut queued = commands.try_recv().unwrap();
        let AdminCommand::CaptureInstrumentBounce {
            expected_epoch,
            parameter_ids,
            parameter_admission,
            lease,
            reply,
        } = queued.take_command()
        else {
            panic!("expected the second dedicated capture request");
        };
        let observed_time = if case == 1 {
            now + BOUNCE_CAPTURE_TIMEOUT
        } else {
            now
        };
        if case == 0 {
            second.cancel();
        }
        if case < 2 {
            assert!(
                second
                    .poll(stamp, status(), observed_time)
                    .unwrap()
                    .is_none()
            );
        }
        let mut slots = vec![slot(
            if case == 2 {
                Err(BounceCaptureError::BackendFailed(
                    "second capture helper fault".into(),
                ))
            } else {
                Ok(state)
            },
            Arc::clone(&calls),
            None,
        )];
        let (mut events, mut event_reader) = RingBuffer::new(4);
        let result = capture_instrument_bounce_on_worker(
            &lease.fence,
            expected_epoch,
            stamp.epoch,
            &AtomicU64::new(stamp.epoch),
            true,
            &parameter_ids,
            &parameter_admission,
            &mut slots,
            &mut events,
            &control.metrics,
        );
        reply.send(result).unwrap();
        // A cancellation observed after a returned helper fault cannot replace it.
        if case == 2 {
            second.cancel();
        }
        assert!(
            second
                .poll(stamp, status(), observed_time)
                .unwrap()
                .is_none()
        );
        assert!(!second.termination_acknowledged());
        assert!(control.metrics.bounce_capture_busy.load(Ordering::Acquire));
        drop(lease);
        assert!(second.termination_acknowledged());
        let error = second.poll(stamp, status(), observed_time).err().unwrap();
        assert_eq!(
            error,
            [
                "Instrument bounce capture was cancelled",
                "Instrument bounce state capture timed out",
                "second capture helper fault"
            ][case]
        );
        assert_eq!(calls.load(Ordering::SeqCst), if case == 2 { 2 } else { 1 });
        assert_eq!(slots[0].fault.is_some(), case == 2);
        assert_eq!(
            matches!(event_reader.pop(), Ok(RuntimeEvent::SlotFault { .. })),
            case == 2
        );
        assert!(event_reader.pop().is_err());
        assert!(!control.metrics.bounce_capture_busy.load(Ordering::Acquire));
        assert_eq!(serde_json::to_vec(&project).unwrap(), original);
        assert!(media.path().exists());
        // No matched source seal exists, so candidate/history preparation is not entered.
        drop(attestation);
    }
}
