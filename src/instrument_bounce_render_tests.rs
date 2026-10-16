use super::*;
use crate::instrument_bounce::BounceAudioBus;
use crate::{
    instrument_bounce::{BouncePluginState, InstrumentBouncePlan},
    model::*,
};
use std::{
    collections::BTreeMap,
    sync::{Mutex, atomic::AtomicU64},
    thread,
};

#[path = "generator_q128_automation_tests.rs"]
mod generator_automation;

static NEXT: AtomicU64 = AtomicU64::new(1);
fn control() -> RenderControl {
    RenderControl {
        export: ExportControl::default(),
        deadline: Instant::now() + Duration::from_secs(20),
    }
}
fn metadata(latency: u32, channels: usize) -> BouncePluginMetadata {
    BouncePluginMetadata {
        class_uid: "ABCDEF019182FAEB566D624153675854".into(),
        version: "1.3.4".into(),
        name: "Fake only".into(),
        category: "Instrument|Synth".into(),
        event_input_buses: vec![(16, true, Some(true))],
        event_output_buses: vec![],
        input_buses: vec![],
        output_buses: vec![BounceAudioBus::new(channels, true, Some(true))],
        latency_samples: latency,
        tail_samples: u32::MAX,
    }
}
fn fixture_project(song_frames: u64) -> Project {
    let mut project = Project {
        tempo: 120.0,
        song_length_beats: song_frames as f32 / 4000.0,
        ..Project::default()
    };
    project.automation_lanes.clear();
    project.channels.truncate(1);
    project.channels[0].instrument_plugin_instance_id = Some(41);
    project.plugin_instances = vec![PluginInstance {
        id: 41,
        format: PluginFormat::Vst3,
        role: PluginRole::Instrument,
        path: "fake-only.vst3".into(),
        uid: "path-id".into(),
        vendor: "Fixture".into(),
        name: "Fixture".into(),
        enabled: true,
        bypass: false,
        wet: 1.0,
        parameters: BTreeMap::new(),
        opaque_state: vec![1],
        runtime_status: PluginRuntimeStatus::Loaded,
        midi_ports: Default::default(),
    }];
    project.patterns.truncate(1);
    project.patterns[0].channel_steps = vec![[false; 16]];
    project.patterns[0].channel_steps[0][0] = true;
    project.patterns[0].notes.clear();
    project.clips.truncate(1);
    project.clips[0].start = 0.0;
    project.clips[0].length = project.song_length_beats;
    project.clips[0].source_offset = 0.0;
    project.clips[0].pattern_id = project.patterns[0].id;
    project
}
fn capture(song_frames: u64, tail: u32, latency: u32, channels: usize) -> CapturedInstrumentBounce {
    capture_project(
        &fixture_project(song_frames),
        song_frames,
        tail,
        latency,
        channels,
    )
}
fn capture_project(
    project: &Project,
    song_frames: u64,
    tail: u32,
    latency: u32,
    channels: usize,
) -> CapturedInstrumentBounce {
    let plan = InstrumentBouncePlan::prepare(project, project.channels[0].id, 8000, tail).unwrap();
    assert_eq!(plan.timeline().duration_frames(), song_frames);
    let identity = BounceSourceIdentity {
        project_session: 11,
        project_fingerprint: plan.project_fingerprint(),
        timeline_revision: 12,
        epoch: 13,
        endpoint_id: 14,
        channel_id: plan.channel_id(),
        instance_id: 41,
    };
    CapturedInstrumentBounce {
        receipt_token: Arc::new(()),
        identity,
        plan,
        state: BouncePluginState {
            metadata: metadata(latency, channels),
            opaque_state: vec![1],
            parameter_bases: BTreeMap::new(),
            native_generation: 2,
            native_dirty_revision: 9,
        },
    }
}
struct Files(PathBuf);
impl Files {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "bounce-engine-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn output(&self) -> PathBuf {
        self.0.join("render.wav")
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn wait_until(mut condition: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(8);
    while !condition() {
        assert!(Instant::now() < end, "fixture deadline");
        thread::sleep(Duration::from_millis(1));
    }
}
fn outcome(receipt: &RenderReceipt) -> RenderOutcome {
    wait_until(|| receipt.termination_acknowledged());
    receipt.try_recv().unwrap()
}

#[derive(Clone, Copy, Debug)]
enum Fault {
    None,
    Short,
    Extra,
    Missing,
    BadLatencySample,
    BadPaddingSample,
    MissingTransport,
    Midi,
    Loss,
    Drift,
    CancelPrepare,
    CancelMetadata,
    CancelMidi,
    DeadlineProcess,
    CancelProcess,
    FailAndCancel,
    InvalidAndCancel,
    PanicProcess,
    CleanupFailure,
    CleanupPanic,
}
struct Fake {
    meta: BouncePluginMetadata,
    ledger: Arc<Mutex<Vec<String>>>,
    control: RenderControl,
    fault: Fault,
    next: u64,
    retained_end: u64,
    query_count: usize,
    cleanup_gate: Arc<AtomicBool>,
    disposed: Arc<AtomicBool>,
    cleanup_panicked: bool,
    cleanup_failure: Option<BackendFailure>,
    terminal_grace: Arc<Mutex<Option<Arc<JobProgress>>>>,
}
impl Fake {
    fn new(meta: BouncePluginMetadata, control: RenderControl, fault: Fault, end: u64) -> Self {
        Self {
            meta,
            ledger: Arc::new(Mutex::new(vec![])),
            control,
            fault,
            next: 0,
            retained_end: end,
            query_count: 0,
            cleanup_gate: Arc::new(AtomicBool::new(true)),
            disposed: Arc::new(AtomicBool::new(false)),
            cleanup_panicked: false,
            cleanup_failure: None,
            terminal_grace: Arc::new(Mutex::new(None)),
        }
    }
    fn log(&self, s: impl Into<String>) {
        self.ledger.lock().unwrap().push(s.into());
    }
}
impl RenderBackend for Fake {
    fn prepare(
        &mut self,
        capture: &CapturedInstrumentBounce,
        control: &RenderControl,
    ) -> Result<BouncePluginMetadata, BackendFailure> {
        control.checkpoint().map_err(|e| e.primary)?;
        assert_eq!(capture.state.opaque_state, vec![1]);
        self.log("Prepare");
        if matches!(self.fault, Fault::CancelPrepare) {
            self.control.export.cancel();
        }
        Ok(self.meta.clone())
    }
    fn metadata(&mut self) -> Result<BouncePluginMetadata, BackendFailure> {
        self.query_count += 1;
        self.log("Metadata");
        if matches!(self.fault, Fault::CancelMetadata) {
            self.control.export.cancel();
        }
        let mut meta = self.meta.clone();
        if matches!(self.fault, Fault::Drift) && self.query_count == 2 {
            meta.latency_samples += 1;
        }
        Ok(meta)
    }
    fn send_midi(&mut self, midi: [u8; 3], offset: u16) -> Result<(), BackendFailure> {
        self.log(format!("Midi:{}:{midi:?}", self.next + u64::from(offset)));
        if matches!(self.fault, Fault::CancelMidi) {
            self.control.export.cancel();
        }
        Ok(())
    }
    fn process(
        &mut self,
        transport: PluginTransport,
        frames: usize,
    ) -> Result<BlockReply, BackendFailure> {
        assert_eq!(frames, 128);
        assert_eq!(transport.sample_position, self.next as i64);
        assert!(transport.playing);
        assert_eq!(
            (transport.time_sig_numerator, transport.time_sig_denominator),
            (4, 4)
        );
        assert!((transport.quarter_note_position - self.next as f64 / 4000.0).abs() < 1e-12);
        self.log(format!("Process:{}", self.next));
        if matches!(self.fault, Fault::DeadlineProcess) {
            thread::sleep(
                self.control
                    .deadline
                    .saturating_duration_since(Instant::now())
                    + Duration::from_millis(2),
            );
        }
        if matches!(
            self.fault,
            Fault::CancelProcess
                | Fault::FailAndCancel
                | Fault::InvalidAndCancel
                | Fault::PanicProcess
        ) {
            self.control.export.cancel();
        }
        if matches!(self.fault, Fault::FailAndCancel) {
            return Err("injected Process failure".into());
        }
        if matches!(self.fault, Fault::PanicProcess) {
            panic!("injected Process panic");
        }
        let mut channels = vec![vec![0.0; frames]; self.meta.output_buses[0].channel_count];
        for (ch, channel) in channels.iter_mut().enumerate() {
            for (i, sample) in channel.iter_mut().enumerate() {
                let frame = self.next + i as u64;
                if frame == u64::from(self.meta.latency_samples) {
                    *sample = if ch == 0 { 0.75 } else { -0.25 };
                }
                if (matches!(self.fault, Fault::BadLatencySample) && frame == 0)
                    || (matches!(self.fault, Fault::BadPaddingSample) && frame == self.retained_end)
                {
                    *sample = f32::NAN;
                }
            }
        }
        if matches!(self.fault, Fault::Short | Fault::InvalidAndCancel) {
            channels[0].pop();
        }
        if matches!(self.fault, Fault::Missing) {
            channels.pop();
        }
        if matches!(self.fault, Fault::Extra) {
            channels.push(vec![0.0; frames]);
        }
        self.next += frames as u64;
        Ok(BlockReply {
            channels,
            transport_applied: !matches!(self.fault, Fault::MissingTransport),
            output_events: usize::from(matches!(self.fault, Fault::Midi)),
            output_events_lost: matches!(self.fault, Fault::Loss),
        })
    }
    fn dispose(&mut self, _deadline: Instant) -> DisposalObservation {
        if matches!(self.fault, Fault::CleanupPanic) && !self.cleanup_panicked {
            self.cleanup_panicked = true;
            self.log("DisposePanic");
            panic!("injected disposal panic");
        }
        if !self.cleanup_gate.load(Ordering::Acquire) {
            self.log("DisposePending");
            return DisposalObservation {
                terminated: false,
                error: self.cleanup_failure.clone().or_else(|| {
                    matches!(self.fault, Fault::CleanupFailure)
                        .then(|| "injected unproven disposal".into())
                }),
                diagnostics: Vec::new(),
            };
        }
        if let Some(progress) = self.terminal_grace.lock().unwrap().as_ref() {
            progress.expire_cleanup_grace.store(true, Ordering::Release);
        }
        self.log("Disposed");
        self.disposed.store(true, Ordering::Release);
        DisposalObservation {
            terminated: true,
            error: self.cleanup_failure.clone().or_else(|| {
                matches!(self.fault, Fault::CleanupFailure)
                    .then(|| "injected cleanup failure".into())
            }),
            diagnostics: Vec::new(),
        }
    }
}
impl Drop for Fake {
    fn drop(&mut self) {
        assert!(
            self.disposed.load(Ordering::Acquire),
            "owner dropped before checked disposal"
        );
        self.log("Dropped");
    }
}

#[test]
fn fake_render_exact_native_latency_tail_padding_and_mono_stereo_unity() {
    for latency in [0, 1, 127, 128, 129] {
        for n in [127, 128, 129, 257] {
            for tail in [0, 7] {
                for channels in [1, 2] {
                    let c = capture(n, tail, latency, channels);
                    let stamp = c.identity();
                    let ctl = control();
                    let files = Files::new();
                    let fake = Fake::new(
                        c.metadata().clone(),
                        ctl.clone(),
                        Fault::None,
                        n + u64::from(tail) + u64::from(latency),
                    );
                    let ledger = Arc::clone(&fake.ledger);
                    let admission = RenderAdmission::default();
                    let receipt = admission
                        .start(c, files.output(), true, ctl, move || fake)
                        .unwrap();
                    let RenderOutcome::Complete(report) = outcome(&receipt) else {
                        panic!("successful control failed")
                    };
                    assert_eq!(report.identity, stamp);
                    assert_eq!(report.frames, n as usize + tail as usize);
                    assert_eq!(report.native_latency, latency);
                    assert_eq!(report.fixed_tail, tail);
                    assert_eq!(report.reported_tail, u32::MAX);
                    assert_eq!(report.sample_peak, 0.75);
                    assert_eq!(report.generated_frames % 128, 0);
                    assert!(report.generated_frames > n);
                    assert!(!admission.is_busy());
                    let wav = crate::wav::read_wav(files.output()).unwrap();
                    assert_eq!(wav.frame_count(), report.frames);
                    assert_eq!(wav.samples[0], 0.75);
                    assert_eq!(wav.samples[1], if channels == 1 { 0.75 } else { -0.25 });
                    assert!(wav.samples[2..].iter().all(|s| *s == 0.0));
                    let ledger = ledger.lock().unwrap();
                    assert!(
                        ledger
                            .iter()
                            .any(|s| s == &format!("Midi:{n}:[128, 36, 0]"))
                    );
                    assert_eq!(&ledger[ledger.len() - 2..], ["Disposed", "Dropped"]);
                }
            }
        }
    }
}

#[test]
fn fake_render_rejects_entire_bad_reply_including_trimmed_and_padding_samples() {
    // Explicit successful control precedes the failure matrix.
    let ctl = control();
    let c = capture(129, 0, 1, 2);
    let prepared = PreparedRender::from_capture(&c, true, &ctl).unwrap();
    let mut good = Fake::new(c.metadata().clone(), ctl.clone(), Fault::None, 130);
    assert_eq!(
        render_audio(&c, &prepared, &mut good, &ctl).unwrap().len(),
        129
    );
    good.dispose(ctl.deadline);
    for fault in [
        Fault::Short,
        Fault::Extra,
        Fault::Missing,
        Fault::BadLatencySample,
        Fault::BadPaddingSample,
        Fault::MissingTransport,
        Fault::Midi,
        Fault::Loss,
        Fault::Drift,
    ] {
        let c = capture(129, 0, 1, 2);
        let ctl = control();
        let files = Files::new();
        std::fs::write(files.output(), b"original").unwrap();
        let fake = Fake::new(c.metadata().clone(), ctl.clone(), fault, 130);
        let ledger = Arc::clone(&fake.ledger);
        let receipt = RenderAdmission::default()
            .start(c, files.output(), true, ctl, move || fake)
            .unwrap();
        let RenderOutcome::Failed(error) = outcome(&receipt) else {
            panic!("fault {fault:?} accepted")
        };
        assert_eq!(error.kind, FailureKind::Invalid);
        assert!(error.primary.contains(if matches!(fault, Fault::Drift) {
            "metadata changed after"
        } else {
            "Invalid exact Process"
        }));
        assert_eq!(std::fs::read(files.output()).unwrap(), b"original");
        assert_eq!(std::fs::read_dir(&files.0).unwrap().count(), 1);
        let ledger = ledger.lock().unwrap();
        assert_eq!(ledger.last().unwrap(), "Dropped");
        let processes = ledger.iter().filter(|s| s.starts_with("Process:")).count();
        assert_eq!(
            processes,
            if matches!(fault, Fault::BadPaddingSample) {
                2
            } else {
                1
            },
            "{fault:?}"
        );
    }
}

#[test]
fn cancellation_fault_panic_and_cleanup_errors_never_publish() {
    let good_capture = capture(128, 0, 0, 2);
    let good_control = control();
    let good_files = Files::new();
    let good = Fake::new(
        good_capture.metadata().clone(),
        good_control.clone(),
        Fault::None,
        128,
    );
    let good_receipt = RenderAdmission::default()
        .start(
            good_capture,
            good_files.output(),
            true,
            good_control,
            move || good,
        )
        .unwrap();
    assert!(matches!(outcome(&good_receipt), RenderOutcome::Complete(_)));
    assert!(good_files.output().exists());
    for fault in [
        Fault::CancelPrepare,
        Fault::CancelMetadata,
        Fault::CancelMidi,
        Fault::DeadlineProcess,
        Fault::CancelProcess,
        Fault::FailAndCancel,
        Fault::InvalidAndCancel,
        Fault::PanicProcess,
        Fault::CleanupFailure,
        Fault::CleanupPanic,
    ] {
        let c = capture(128, 0, 0, 2);
        let mut ctl = control();
        if matches!(fault, Fault::DeadlineProcess) {
            ctl.deadline = Instant::now() + Duration::from_secs(2);
        }
        let files = Files::new();
        let fake = Fake::new(c.metadata().clone(), ctl.clone(), fault, 128);
        let ledger = Arc::clone(&fake.ledger);
        let admission = RenderAdmission::default();
        let receipt = admission
            .start(c, files.output(), true, ctl, move || fake)
            .unwrap();
        let RenderOutcome::Failed(error) = outcome(&receipt) else {
            panic!("fault {fault:?} accepted")
        };
        let expected = match fault {
            Fault::CancelPrepare
            | Fault::CancelMetadata
            | Fault::CancelMidi
            | Fault::CancelProcess => FailureKind::Cancelled,
            Fault::DeadlineProcess => FailureKind::Deadline,
            Fault::InvalidAndCancel => FailureKind::Invalid,
            Fault::PanicProcess | Fault::CleanupPanic => FailureKind::Panic,
            _ => FailureKind::Backend,
        };
        assert_eq!(error.kind, expected, "{fault:?}: {error:?}");
        let reason = match fault {
            Fault::FailAndCancel => "injected Process failure",
            Fault::InvalidAndCancel => "Invalid exact Process",
            Fault::DeadlineProcess => "deadline expired",
            Fault::PanicProcess => "backend panicked",
            Fault::CleanupFailure => "injected cleanup failure",
            Fault::CleanupPanic => "disposal panicked",
            _ => "cancel",
        };
        assert!(error.primary.contains(reason), "{fault:?}: {error:?}");
        if matches!(fault, Fault::CleanupFailure) {
            assert_eq!(
                error.backend.as_ref().unwrap().kind,
                BackendFailureKind::Backend
            );
        }
        assert!(!files.output().exists());
        assert_eq!(std::fs::read_dir(&files.0).unwrap().count(), 0);
        assert!(!admission.is_busy());
        let ledger = ledger.lock().unwrap();
        assert_eq!(ledger.last().unwrap(), "Dropped");
        let before_cleanup = ledger
            .iter()
            .position(|s| s == "Disposed" || s == "DisposePanic")
            .unwrap();
        if matches!(
            fault,
            Fault::CancelPrepare | Fault::CancelMetadata | Fault::CancelMidi
        ) {
            let expected = match fault {
                Fault::CancelPrepare => 1,
                Fault::CancelMetadata => 2,
                _ => 3,
            };
            assert_eq!(before_cleanup, expected, "{fault:?}: {ledger:?}");
            assert!(!ledger.iter().any(|s| s.starts_with("Process:")));
        }
        if matches!(
            fault,
            Fault::CancelProcess
                | Fault::DeadlineProcess
                | Fault::FailAndCancel
                | Fault::InvalidAndCancel
                | Fault::PanicProcess
        ) {
            assert_eq!(
                ledger.iter().filter(|s| s.starts_with("Process:")).count(),
                1
            );
        }
    }
}

#[test]
fn dropped_receiver_and_pending_disposal_retain_owner_and_job_until_ack() {
    let c = capture(128, 0, 0, 2);
    let ctl = control();
    let files = Files::new();
    let fake = Fake::new(c.metadata().clone(), ctl.clone(), Fault::None, 128);
    fake.cleanup_gate.store(false, Ordering::Release);
    let gate = Arc::clone(&fake.cleanup_gate);
    let ledger = Arc::clone(&fake.ledger);
    let disposed = Arc::clone(&fake.disposed);
    let admission = RenderAdmission::default();
    let receipt = admission
        .start(c, files.output(), true, ctl, move || fake)
        .unwrap();
    wait_until(|| ledger.lock().unwrap().iter().any(|s| s == "DisposePending"));
    assert!(!receipt.termination_acknowledged());
    assert!(matches!(receipt.try_recv(), Err(TryRecvError::Empty)));
    receipt.expire_cleanup_grace();
    wait_until(|| matches!(receipt.status(), RenderStatus::CleanupPending { .. }));
    drop(receipt);
    assert!(matches!(
        admission.status(),
        Some(RenderStatus::CleanupPending { .. })
    ));
    assert!(admission.is_busy());
    assert!(!disposed.load(Ordering::Acquire));
    assert!(!files.output().exists());
    let rejected = admission.start(
        capture(128, 0, 0, 2),
        files.output(),
        true,
        control(),
        || -> Fake { panic!("busy factory must not run") },
    );
    assert!(rejected.err().unwrap().contains("still owns"));
    gate.store(true, Ordering::Release);
    wait_until(|| !admission.is_busy());
    assert!(disposed.load(Ordering::Acquire));
    assert_eq!(ledger.lock().unwrap().last().unwrap(), "Dropped");
    assert!(!files.output().exists());
}

fn source() -> NoteSourceDescriptor {
    NoteSourceDescriptor::ChannelStep {
        clip_id: 1,
        pattern_id: 1,
        step: 0,
        repetition: 0,
    }
}
fn on(id: u64, frame: u64, pitch: u8) -> TimelineEvent {
    TimelineEvent {
        frame,
        kind: TimelineEventKind::NoteOn {
            note_id: id,
            channel_id: 7,
            note: pitch,
            velocity: 0.5,
            gain: 0.75,
            mixer_track: 1,
            source: source(),
        },
    }
}
fn off(id: u64, frame: u64, pitch: u8) -> TimelineEvent {
    TimelineEvent {
        frame,
        kind: TimelineEventKind::NoteOff {
            note_id: id,
            channel_id: 7,
            note: pitch,
            mixer_track: 1,
            source: source(),
        },
    }
}
#[test]
fn mapper_rejects_boundaries_and_full_off_identity_mismatches_without_partial_result() {
    assert_eq!(
        map_selected_notes(&[on(1, 0, 60), off(1, 128, 60)], 7, 128, &control()).unwrap(),
        vec![
            ScheduledMidi {
                frame: 0,
                data: [0x90, 60, 48]
            },
            ScheduledMidi {
                frame: 128,
                data: [0x80, 60, 0]
            }
        ]
    );
    for case in 0..9 {
        let mut events = vec![on(1, 0, 60), off(1, 128, 60)];
        match case {
            0 => events[0].frame = 128,
            1 => events[1].frame = 129,
            2 => events[1].frame = 0,
            3 => {
                if let TimelineEventKind::NoteOff { ref mut source, .. } = events[1].kind {
                    *source = NoteSourceDescriptor::LegacyPiano {
                        clip_id: 1,
                        pattern_id: 1,
                        persistent_note_id: 1,
                        note_index: 0,
                        repetition: 0,
                    }
                }
            }
            4 => {
                if let TimelineEventKind::NoteOff {
                    ref mut mixer_track,
                    ..
                } = events[1].kind
                {
                    *mixer_track = 2
                }
            }
            5 => events[1] = off(1, 128, 61),
            6 => events[1] = off(2, 128, 60),
            7 => events.insert(1, on(1, 1, 60)),
            _ => {
                if let TimelineEventKind::NoteOn {
                    ref mut velocity, ..
                } = events[0].kind
                {
                    *velocity = f32::NAN
                }
            }
        }
        let error = map_selected_notes(&events, 7, 128, &control()).unwrap_err();
        assert_eq!(error.kind, FailureKind::Invalid, "case {case}");
        let reason = match case {
            0 | 7 | 8 => "Selected NoteOn has an invalid frame",
            6 => "Selected NoteOff has no active identity",
            _ => "Selected NoteOff does not match its full NoteOn identity/range",
        };
        assert!(error.primary.contains(reason), "case {case}: {error:?}");
    }
}
#[test]
fn mapper_capacity_limits_and_preflight_cancellation_are_explicit() {
    for count in [96, 97] {
        let mut events: Vec<_> = (0..count).map(|n| on(n + 1, 0, (n % 128) as u8)).collect();
        events.extend((0..count).map(|n| off(n + 1, 256, (n % 128) as u8)));
        let result = map_selected_notes(&events, 7, 256, &control());
        if count == 96 {
            assert_eq!(result.unwrap().len(), 192)
        } else {
            assert!(result.unwrap_err().primary.contains("96 Timeline"));
        }
    }
    for count in [512, 513] {
        let mut events: Vec<_> = (0..count).map(|n| on(n + 1, n * 128, 60)).collect();
        events.extend((0..count).map(|n| off(n + 1, 100_000 + n, 60)));
        let result = map_selected_notes(&events, 7, 200_000, &control());
        if count == 512 {
            assert_eq!(result.unwrap().len(), 513)
        } else {
            assert!(result.unwrap_err().primary.contains("512 active"));
        }
    }
    let ctl = control();
    ctl.export.cancel();
    assert_eq!(
        map_selected_notes(&[on(1, 0, 60), off(1, 1, 60)], 7, 2, &ctl)
            .unwrap_err()
            .kind,
        FailureKind::Cancelled
    );
    let expired = RenderControl {
        export: ExportControl::default(),
        deadline: Instant::now(),
    };
    assert_eq!(
        PreparedRender::from_capture(&capture(128, 0, 0, 2), true, &expired)
            .err()
            .unwrap()
            .kind,
        FailureKind::Deadline
    );
    assert!(
        PreparedRender::from_capture(&capture(128, 0, 0, 2), false, &control())
            .err()
            .unwrap()
            .primary
            .contains("acknowledgment")
    );
}

#[test]
fn rounded_processing_bounds_are_checked_without_allocating_the_song() {
    assert_eq!(render_frame_bounds(127, 0, 0).unwrap(), (127, 128));
    assert_eq!(render_frame_bounds(128, 0, 0).unwrap(), (128, 256));
    assert_eq!(render_frame_bounds(129, 7, 129).unwrap(), (136, 384));
    assert_eq!(
        render_frame_bounds(MAX_BOUNCE_FRAMES - 1, 0, 0).unwrap().1,
        MAX_BOUNCE_FRAMES
    );
    assert!(
        render_frame_bounds(MAX_BOUNCE_FRAMES, 0, 0)
            .unwrap_err()
            .primary
            .contains("Rounded render")
    );
    for (n, t, l) in [(0, 0, 0), (u64::MAX, 1, 0), (u64::MAX, 0, 1)] {
        assert_eq!(
            render_frame_bounds(n, t, l).unwrap_err().kind,
            FailureKind::Invalid
        );
    }
}

#[test]
fn full_schedule_preflight_observes_mid_scan_cancellation_and_never_constructs_backend() {
    let ctl = control();
    ctl.export.cancel_at(500);
    let events: Vec<_> = (0..1000)
        .map(|n| {
            let mut event = on(n + 1, n, 60);
            if let TimelineEventKind::NoteOn {
                ref mut channel_id, ..
            } = event.kind
            {
                *channel_id = 99;
            }
            event
        })
        .collect();
    let error = map_selected_notes(&events, 7, 2000, &ctl).unwrap_err();
    assert_eq!(error.kind, FailureKind::Cancelled);
    assert_eq!(ctl.export.progress().basis_points, 500);
    let admission = RenderAdmission::default();
    let files = Files::new();
    let calls = Arc::new(AtomicU64::new(0));
    let worker_calls = Arc::clone(&calls);
    let receipt = admission
        .start(
            capture(128, 0, 0, 2),
            files.output(),
            false,
            control(),
            move || -> Fake {
                worker_calls.fetch_add(1, Ordering::Relaxed);
                panic!("invalid preflight must not construct backend")
            },
        )
        .unwrap();
    let RenderOutcome::Failed(error) = outcome(&receipt) else {
        panic!("unacknowledged tail accepted")
    };
    assert_eq!(error.kind, FailureKind::Invalid);
    assert!(error.primary.contains("acknowledgment"));
    assert_eq!(calls.load(Ordering::Acquire), 0);
    assert!(!files.output().exists());
    assert!(!admission.is_busy());
}

#[test]
fn failed_disposal_keeps_receipt_pending_and_preserves_error_after_ack() {
    let c = capture(128, 0, 0, 2);
    let ctl = control();
    let files = Files::new();
    let fake = Fake::new(
        c.metadata().clone(),
        ctl.clone(),
        Fault::CleanupFailure,
        128,
    );
    fake.cleanup_gate.store(false, Ordering::Release);
    let gate = Arc::clone(&fake.cleanup_gate);
    let ledger = Arc::clone(&fake.ledger);
    let admission = RenderAdmission::default();
    let receipt = admission
        .start(c, files.output(), true, ctl, move || fake)
        .unwrap();
    wait_until(|| ledger.lock().unwrap().iter().any(|s| s == "DisposePending"));
    assert!(admission.is_busy());
    assert!(!receipt.termination_acknowledged());
    assert!(!files.output().exists());
    receipt.cancel();
    gate.store(true, Ordering::Release);
    let RenderOutcome::Failed(error) = outcome(&receipt) else {
        panic!("failed disposal published")
    };
    assert_eq!(error.kind, FailureKind::Backend);
    assert_eq!(error.primary, "injected unproven disposal");
    assert_eq!(error.backend.as_ref().unwrap().diagnostic(), error.primary);
    assert!(error.cleanup.iter().any(|s| s.contains("cleanup failure")));
    assert!(!files.output().exists());
    assert!(!admission.is_busy());
}

#[test]
fn compiled_arrangement_keeps_swing_step_and_piano_repeats_source_offsets_and_clip_gain() {
    let mut swung = fixture_project(2000);
    swung.swing = 1.0;
    swung.patterns[0].channel_steps[0][1] = true;

    let mut steps = fixture_project(13000);
    steps.patterns[0].length_steps = 4;
    steps.clips[0].start = 1.0;
    steps.clips[0].length = 2.25;
    steps.clips[0].source_offset = 0.25;

    let mut piano = fixture_project(73500);
    piano.patterns[0].channel_steps[0] = [false; 16];
    piano.patterns[0].notes = vec![PianoNote {
        id: 1,
        channel_id: Some(piano.channels[0].id),
        group_id: None,
        note: 64,
        start: 0.5,
        length: 2.0,
        velocity: 0.8,
        selected: false,
        muted: false,
    }];
    piano.channels[0].volume = 0.01; // Not part of instrument MIDI velocity.
    piano.clips[0].gain = 0.5;
    piano.clips[0].start = 1.0;
    piano.clips[0].length = 17.375;
    piano.clips[0].source_offset = 0.25;

    for (project, frames, ons, offs, pitch, velocity) in [
        (swung, 2000, vec![0, 1750], vec![1000, 2000], 36, 127),
        (steps, 13000, vec![7000, 11000], vec![8000, 12000], 36, 127),
        (piano, 73500, vec![5000, 69000], vec![13000, 73500], 64, 51),
    ] {
        let before = serde_json::to_vec(&project).unwrap();
        let capture = capture_project(&project, frames, 0, 0, 2);
        let prepared = PreparedRender::from_capture(&capture, true, &control()).unwrap();
        assert_eq!(
            prepared
                .midi
                .iter()
                .filter(|e| e.data[0] == 0x90)
                .map(|e| e.frame)
                .collect::<Vec<_>>(),
            ons
        );
        assert_eq!(
            prepared
                .midi
                .iter()
                .filter(|e| e.data[0] == 0x80)
                .map(|e| e.frame)
                .collect::<Vec<_>>(),
            offs
        );
        assert!(prepared.midi.iter().all(|e| e.data[1] == pitch));
        assert!(
            prepared
                .midi
                .iter()
                .filter(|e| e.data[0] == 0x90)
                .all(|e| e.data[2] == velocity)
        );
        assert_eq!(serde_json::to_vec(&project).unwrap(), before);
    }
}

#[test]
fn empty_fixture_constructor_panic_releases_only_the_empty_job() {
    let admission = RenderAdmission::default();
    let files = Files::new();
    let receipt = admission
        .start(
            capture(128, 0, 0, 2),
            files.output(),
            true,
            control(),
            || -> Fake { panic!("empty fixture constructor panic") },
        )
        .unwrap();
    let RenderOutcome::Failed(error) = outcome(&receipt) else {
        panic!("constructor panic accepted")
    };
    assert_eq!(error.kind, FailureKind::Panic);
    assert_eq!(error.primary, "Empty fixture owner construction panicked");
    assert!(error.cleanup.is_empty());
    assert!(!admission.is_busy());
    assert!(!files.output().exists());
}

#[test]
fn renderer_writer_cancel_and_deadline_keep_category_after_backend_termination() {
    // A full successful job is the control for each publication-stage injection.
    for (cancel_at, deadline_at) in [
        (0, 0),
        (7000, 0),
        (7800, 0),
        (9800, 0),
        (9900, 0),
        (0, 9900),
    ] {
        let c = capture(16384, 0, 0, 2);
        let ctl = control();
        if cancel_at != 0 {
            ctl.export.cancel_at(cancel_at);
        }
        if deadline_at != 0 {
            ctl.export.deadline_at(deadline_at);
        }
        let observed_control = ctl.clone();
        let fake = Fake::new(c.metadata().clone(), ctl.clone(), Fault::None, 16384);
        let ledger = Arc::clone(&fake.ledger);
        let files = Files::new();
        std::fs::write(files.output(), b"original destination").unwrap();
        let admission = RenderAdmission::default();
        let receipt = admission
            .start(c, files.output(), true, ctl, move || fake)
            .unwrap();
        let result = outcome(&receipt);
        if cancel_at == 0 && deadline_at == 0 {
            assert!(matches!(result, RenderOutcome::Complete(_)));
            assert_eq!(
                crate::wav::read_wav(files.output()).unwrap().frame_count(),
                16384
            );
        } else {
            let RenderOutcome::Failed(error) = result else {
                panic!("writer injection published")
            };
            assert_eq!(
                error.kind,
                if cancel_at != 0 {
                    FailureKind::Cancelled
                } else {
                    FailureKind::Deadline
                }
            );
            assert!(error.primary.contains(if cancel_at != 0 {
                "WAV export cancelled"
            } else {
                "WAV export deadline elapsed"
            }));
            assert_eq!(
                std::fs::read(files.output()).unwrap(),
                b"original destination"
            );
            let progress = observed_control.export.progress().basis_points;
            assert!(progress >= cancel_at.max(deadline_at) && progress < 10000);
        }
        assert_eq!(std::fs::read_dir(&files.0).unwrap().count(), 1);
        assert!(!admission.is_busy());
        let log = ledger.lock().unwrap();
        assert_eq!(&log[log.len() - 2..], ["Disposed", "Dropped"]);
        assert_eq!(
            log.iter().filter(|s| s.starts_with("Process:")).count(),
            129
        );
    }
}

#[test]
fn renderer_writer_categories_preserve_checked_abort_error_and_retained_path() {
    for deadline in [false, true] {
        let original: anyhow::Error = if deadline {
            crate::export_job::ExportDeadlineExceeded.into()
        } else {
            crate::export_job::ExportCancelled.into()
        };
        let error = original.context(crate::export::StagedOutputAbortFailure {
            retained_path: PathBuf::from("retained-stage.wav"),
            removal_error: std::io::Error::other("injected abort failure"),
        });
        let failure = writer_failure(error);
        assert_eq!(
            failure.kind,
            if deadline {
                FailureKind::Deadline
            } else {
                FailureKind::Cancelled
            }
        );
        assert!(failure.primary.contains("retained-stage.wav"));
        assert!(failure.primary.contains("injected abort failure"));
        assert!(failure.primary.contains(if deadline {
            "deadline elapsed"
        } else {
            "export cancelled"
        }));
    }
}

#[test]
fn bounded_cleanup_grace_becomes_visible_pending_without_ack_or_owner_release() {
    for fault in [Fault::None, Fault::CleanupFailure, Fault::CleanupPanic] {
        let c = capture(128, 0, 0, 2);
        let ctl = control();
        let fake = Fake::new(c.metadata().clone(), ctl.clone(), fault, 128);
        fake.cleanup_gate.store(false, Ordering::Release);
        let gate = Arc::clone(&fake.cleanup_gate);
        let ledger = Arc::clone(&fake.ledger);
        let disposed = Arc::clone(&fake.disposed);
        let admission = RenderAdmission::default();
        let files = Files::new();
        let receipt = admission
            .start(c, files.output(), true, ctl, move || fake)
            .unwrap();
        wait_until(|| ledger.lock().unwrap().iter().any(|s| s == "DisposePending"));
        receipt.expire_cleanup_grace();
        wait_until(|| matches!(receipt.status(), RenderStatus::CleanupPending { .. }));
        let RenderStatus::CleanupPending { diagnostics } = receipt.status() else {
            unreachable!()
        };
        assert!(diagnostics.iter().any(|s| s.contains("grace elapsed")));
        if matches!(fault, Fault::CleanupFailure) {
            assert!(diagnostics.iter().any(|s| s.contains("unproven disposal")));
        }
        if matches!(fault, Fault::CleanupPanic) {
            assert!(diagnostics.iter().any(|s| s.contains("disposal panicked")));
        }
        assert!(admission.is_busy());
        assert!(!receipt.termination_acknowledged());
        assert!(matches!(receipt.try_recv(), Err(TryRecvError::Empty)));
        assert!(!disposed.load(Ordering::Acquire));
        assert!(!files.output().exists());
        receipt.cancel();
        // Visibility is retained by the admission owner, not only this receiver.
        assert!(matches!(
            admission.status(),
            Some(RenderStatus::CleanupPending { .. })
        ));
        let retry = admission.start(
            capture(128, 0, 0, 2),
            files.output(),
            true,
            control(),
            || -> Fake { panic!("pending cleanup cannot admit a replacement") },
        );
        assert_eq!(
            retry.err().unwrap(),
            "An instrument renderer still owns this job"
        );
        gate.store(true, Ordering::Release);
        let RenderOutcome::Failed(error) = outcome(&receipt) else {
            panic!("late cleanup published")
        };
        assert_eq!(
            error.kind,
            match fault {
                Fault::None => FailureKind::Cancelled,
                Fault::CleanupPanic => FailureKind::Panic,
                _ => FailureKind::Backend,
            }
        );
        assert!(error.cleanup_grace_elapsed);
        assert!(error.cleanup.iter().any(|s| s.contains("grace elapsed")));
        assert_eq!(receipt.status(), RenderStatus::Terminated);
        assert!(!admission.is_busy());
        assert!(!files.output().exists());
        assert!(disposed.load(Ordering::Acquire));
        // Acquire acknowledgment allows immediate readmission. The old receipt
        // remains bound to its old progress/control and cannot cancel this job.
        let next_capture = capture(128, 0, 0, 2);
        let next_control = control();
        let next_fake = Fake::new(
            next_capture.metadata().clone(),
            next_control.clone(),
            Fault::None,
            128,
        );
        next_fake.cleanup_gate.store(false, Ordering::Release);
        let next_gate = Arc::clone(&next_fake.cleanup_gate);
        let next_ledger = Arc::clone(&next_fake.ledger);
        let next = admission
            .start(
                next_capture,
                files.output(),
                true,
                next_control.clone(),
                move || next_fake,
            )
            .unwrap();
        wait_until(|| {
            next_ledger
                .lock()
                .unwrap()
                .iter()
                .any(|s| s == "DisposePending")
        });
        drop(receipt);
        assert!(!next_control.export.progress().cancelling);
        assert_eq!(next.status(), RenderStatus::Disposing);
        next_gate.store(true, Ordering::Release);
        assert!(matches!(outcome(&next), RenderOutcome::Complete(_)));
        assert!(!admission.is_busy());
    }
}

#[test]
fn first_terminal_disposal_poll_cannot_skip_post_poll_grace_expiry() {
    for expire in [false, true] {
        let c = capture(128, 0, 0, 2);
        let ctl = control();
        let fake = Fake::new(c.metadata().clone(), ctl.clone(), Fault::None, 128);
        fake.cleanup_gate.store(false, Ordering::Release);
        let gate = Arc::clone(&fake.cleanup_gate);
        let terminal_grace = Arc::clone(&fake.terminal_grace);
        let ledger = Arc::clone(&fake.ledger);
        let files = Files::new();
        let admission = RenderAdmission::default();
        let receipt = admission
            .start(c, files.output(), true, ctl, move || fake)
            .unwrap();
        wait_until(|| ledger.lock().unwrap().iter().any(|s| s == "DisposePending"));
        assert_eq!(receipt.status(), RenderStatus::Disposing);
        if expire {
            // Only the first terminal poll trips the synthetic clock boundary.
            // Prior nonterminal polls remain within the grace.
            *terminal_grace.lock().unwrap() = Some(Arc::clone(&receipt.progress));
        }
        gate.store(true, Ordering::Release);
        let result = outcome(&receipt);
        if expire {
            let RenderOutcome::Failed(error) = result else {
                panic!("late terminal poll published")
            };
            assert_eq!(error.kind, FailureKind::Backend);
            assert!(error.cleanup_grace_elapsed);
            assert!(error.cleanup.iter().any(|s| s.contains("grace elapsed")));
            assert!(!files.output().exists());
        } else {
            assert!(matches!(result, RenderOutcome::Complete(_)));
            assert!(files.output().exists());
        }
        let log = ledger.lock().unwrap();
        assert_eq!(log.iter().filter(|s| s.as_str() == "Disposed").count(), 1);
        assert_eq!(log.last().unwrap(), "Dropped");
        assert_eq!(receipt.status(), RenderStatus::Terminated);
        assert!(!admission.is_busy());
    }
}

// These fixtures pin renderer precedence, not the strict adapter's protocol. Its
// separate mock-child acceptance must prove actual Stop/Shutdown/reap ordering.
struct CleanupStep {
    label: &'static str,
    terminated: bool,
    error: Option<BackendFailure>,
    diagnostics: Vec<String>,
    cancel: bool,
    expire_grace: bool,
}
impl CleanupStep {
    fn new(label: &'static str, terminated: bool, error: Option<BackendFailure>) -> Self {
        Self {
            label,
            terminated,
            error,
            diagnostics: Vec::new(),
            cancel: false,
            expire_grace: false,
        }
    }
}
struct TypedFake {
    fake: Fake,
    process_error: Option<BackendFailure>,
    cancel_final_process: bool,
    cleanup: std::collections::VecDeque<CleanupStep>,
    progress: Arc<JobProgress>,
    cancel_on_drop: bool,
    panic_on_drop: bool,
}
impl RenderBackend for TypedFake {
    fn prepare(
        &mut self,
        capture: &CapturedInstrumentBounce,
        control: &RenderControl,
    ) -> Result<BouncePluginMetadata, BackendFailure> {
        self.fake.prepare(capture, control)
    }
    fn metadata(&mut self) -> Result<BouncePluginMetadata, BackendFailure> {
        self.fake.metadata()
    }
    fn send_midi(&mut self, midi: [u8; 3], offset: u16) -> Result<(), BackendFailure> {
        self.fake.send_midi(midi, offset)
    }
    fn process(
        &mut self,
        transport: PluginTransport,
        frames: usize,
    ) -> Result<BlockReply, BackendFailure> {
        let reply = self.fake.process(transport, frames)?;
        if self.cancel_final_process && self.fake.next == 256 {
            self.fake.control.export.cancel();
        }
        match self.process_error.take() {
            Some(error) => Err(error),
            None => Ok(reply),
        }
    }
    fn dispose(&mut self, _deadline: Instant) -> DisposalObservation {
        let step = self.cleanup.pop_front().expect("unexpected cleanup poll");
        self.fake.log(step.label);
        if step.cancel {
            self.fake.control.export.cancel();
        }
        if step.expire_grace {
            self.progress
                .expire_cleanup_grace
                .store(true, Ordering::Release);
        }
        if step.terminated {
            self.fake.disposed.store(true, Ordering::Release);
        }
        DisposalObservation {
            terminated: step.terminated,
            error: step.error,
            diagnostics: step.diagnostics,
        }
    }
}
impl Drop for TypedFake {
    fn drop(&mut self) {
        assert!(
            self.cleanup.is_empty(),
            "cleanup finished before scripted reap"
        );
        if self.cancel_on_drop {
            self.fake.control.export.cancel();
        }
        assert!(!self.panic_on_drop, "injected destructor panic");
    }
}
fn run_typed_case(
    process_error: Option<BackendFailure>,
    cancel_final_process: bool,
    cleanup: Vec<CleanupStep>,
    cancel_on_drop: bool,
    panic_on_drop: bool,
) -> (RenderOutcome, Vec<String>) {
    let c = capture(128, 0, 0, 2);
    let ctl = control();
    let progress = Arc::new(JobProgress::new());
    let files = Files::new();
    std::fs::write(files.output(), b"unchanged destination").unwrap();
    let fake = Fake::new(c.metadata().clone(), ctl.clone(), Fault::None, 128);
    let ledger = Arc::clone(&fake.ledger);
    let typed = TypedFake {
        fake,
        process_error,
        cancel_final_process,
        cleanup: cleanup.into(),
        progress: Arc::clone(&progress),
        cancel_on_drop,
        panic_on_drop,
    };
    let result = run_owned(c, files.output(), true, &ctl, &progress, move || typed);
    if matches!(result, RenderOutcome::Failed(_)) {
        assert_eq!(
            std::fs::read(files.output()).unwrap(),
            b"unchanged destination"
        );
    } else {
        assert_eq!(
            crate::wav::read_wav(files.output()).unwrap().frame_count(),
            128
        );
    }
    assert_eq!(std::fs::read_dir(&files.0).unwrap().count(), 1);
    let ledger = ledger.lock().unwrap().clone();
    assert_eq!(ledger.last().unwrap(), "Dropped");
    (result, ledger)
}
fn typed_error(kind: BackendFailureKind, delivery: BackendDelivery, text: &str) -> BackendFailure {
    BackendFailure::new(kind, text).with_delivery(delivery)
}

#[test]
fn typed_backend_failure_category_delivery_and_bounded_text_survive_checked_calls() {
    // Ordinary fake diagnostics cannot imply cancellation by their text.
    for text in [
        "Cancelled: fake backend fault",
        "Deadline: fake backend fault",
    ] {
        let failure: BackendFailure = text.into();
        assert_eq!(failure.kind, BackendFailureKind::Backend);
        assert_eq!(failure.delivery, None);
        let rendered = control().call::<()>(|| Err(failure.clone())).unwrap_err();
        assert_eq!(rendered.kind, FailureKind::Backend);
        assert_eq!(rendered.backend.as_deref(), Some(&failure));
    }
    for (kind, expected) in [
        (BackendFailureKind::Cancelled, FailureKind::Cancelled),
        (BackendFailureKind::Deadline, FailureKind::Deadline),
        (BackendFailureKind::Backend, FailureKind::Backend),
    ] {
        for delivery in [BackendDelivery::NotSent, BackendDelivery::MayHaveApplied] {
            let ctl = control();
            let error = typed_error(kind, delivery, &"界".repeat(4000));
            assert!(error.diagnostic().len() <= MAX_BACKEND_DIAGNOSTIC_BYTES);
            assert_eq!(error.diagnostic().len(), 2046);
            let result = ctl
                .call_checked::<()>(
                    || {
                        // A real operation failure already established in flight
                        // keeps its kind even if cancellation is now also sticky.
                        ctl.export.cancel();
                        Err(error.clone())
                    },
                    |_| panic!("failure cannot be validated as a successful reply"),
                )
                .unwrap_err();
            assert_eq!(result.kind, expected);
            assert_eq!(result.primary, error.diagnostic());
            assert_eq!(result.backend.as_deref(), Some(&error));
            assert!(result.cleanup.is_empty());
        }
    }
}

#[test]
fn renderer_cleanup_boundary_matrix_preserves_exact_typed_primary_and_destination() {
    let (success, ledger) = run_typed_case(
        None,
        false,
        vec![CleanupStep::new("HealthyStopShutdownReaped", true, None)],
        false,
        false,
    );
    assert!(matches!(success, RenderOutcome::Complete(_)));
    assert_eq!(
        &ledger[ledger.len() - 2..],
        ["HealthyStopShutdownReaped", "Dropped"]
    );

    for (label, delivery) in [
        ("BeforeStop", BackendDelivery::NotSent),
        ("DuringStop", BackendDelivery::MayHaveApplied),
        ("AfterStopBeforeShutdown", BackendDelivery::NotSent),
        ("PartialShutdown", BackendDelivery::MayHaveApplied),
    ] {
        for (kind, expected) in [
            (BackendFailureKind::Cancelled, FailureKind::Cancelled),
            (BackendFailureKind::Deadline, FailureKind::Deadline),
        ] {
            let error = typed_error(kind, delivery, label);
            let mut pending = CleanupStep::new(label, false, Some(error.clone()));
            pending.cancel = kind == BackendFailureKind::Cancelled;
            let mut reaped = CleanupStep::new("SameOwnerReaped", true, Some(error.clone()));
            reaped
                .diagnostics
                .push("forced termination after sealed exchange".into());
            let (result, ledger) = run_typed_case(None, false, vec![pending, reaped], false, false);
            let RenderOutcome::Failed(failure) = result else {
                panic!("{label} published")
            };
            assert_eq!(failure.kind, expected, "{label}");
            assert_eq!(failure.primary, label);
            assert_eq!(failure.backend.as_deref(), Some(&error));
            assert_eq!(
                failure.cleanup,
                ["forced termination after sealed exchange"]
            );
            assert_eq!(
                &ledger[ledger.len() - 3..],
                [label, "SameOwnerReaped", "Dropped"]
            );
        }
    }

    // Cancellation observed after final Process remains the earlier rendering
    // failure even when disposal subsequently has typed transport evidence.
    let cleanup_cancel = typed_error(
        BackendFailureKind::Cancelled,
        BackendDelivery::NotSent,
        "Stop rejected after final Process cancellation",
    );
    let (result, ledger) = run_typed_case(
        None,
        true,
        vec![CleanupStep::new(
            "CancelledRenderReaped",
            true,
            Some(cleanup_cancel),
        )],
        false,
        false,
    );
    let RenderOutcome::Failed(failure) = result else {
        panic!("final Process cancellation published")
    };
    assert_eq!(failure.kind, FailureKind::Cancelled);
    assert_eq!(failure.backend, None);
    assert!(failure.primary.contains("cancel"));
    assert_eq!(
        failure.cleanup,
        ["Stop rejected after final Process cancellation"]
    );
    assert_eq!(
        ledger.iter().filter(|s| s.starts_with("Process:")).count(),
        2
    );

    // The final publication fence must run after actual backend destruction.
    let (result, _) = run_typed_case(
        None,
        false,
        vec![CleanupStep::new("HealthyReapBeforeLateCancel", true, None)],
        true,
        false,
    );
    let RenderOutcome::Failed(failure) = result else {
        panic!("post-reap cancellation published")
    };
    assert_eq!(failure.kind, FailureKind::Cancelled);
    assert_eq!(failure.backend, None);
    assert!(failure.cleanup.is_empty());
}

#[test]
fn earlier_render_or_cleanup_fault_wins_over_later_cancel_deadline_and_reap_details() {
    for earlier_render in [false, true] {
        for later in [BackendFailureKind::Cancelled, BackendFailureKind::Deadline] {
            let first = typed_error(
                BackendFailureKind::Backend,
                BackendDelivery::MayHaveApplied,
                if earlier_render {
                    "known Process failure"
                } else {
                    "known Stop failure"
                },
            );
            let late = typed_error(
                later,
                BackendDelivery::NotSent,
                "later cleanup interruption",
            );
            let mut first_cleanup = CleanupStep::new(
                "StopObservation",
                false,
                (!earlier_render).then(|| first.clone()),
            );
            first_cleanup.cancel = true;
            let mut final_cleanup = CleanupStep::new("Reaped", true, Some(late.clone()));
            final_cleanup.diagnostics = vec!["kill error".into(), "nonzero child exit".into()];
            let (result, _) = run_typed_case(
                earlier_render.then(|| first.clone()),
                false,
                vec![first_cleanup, final_cleanup],
                true,
                false,
            );
            let RenderOutcome::Failed(failure) = result else {
                panic!("preceding fault published")
            };
            assert_eq!(failure.kind, FailureKind::Backend);
            assert_eq!(failure.primary, first.diagnostic());
            assert_eq!(failure.backend.as_deref(), Some(&first));
            assert_eq!(
                failure.cleanup_backend.as_deref(),
                Some(if earlier_render { &late } else { &first })
            );
            assert_eq!(
                failure.cleanup,
                [
                    "later cleanup interruption",
                    "kill error",
                    "nonzero child exit"
                ]
            );
        }
    }
}

#[test]
fn cleanup_grace_is_secondary_to_later_typed_cancel_or_deadline() {
    for kind in [BackendFailureKind::Cancelled, BackendFailureKind::Deadline] {
        let mut grace = CleanupStep::new("PendingPastGrace", false, None);
        grace.expire_grace = true;
        let primary = typed_error(
            kind,
            BackendDelivery::NotSent,
            "original protocol budget ended",
        );
        let (result, ledger) = run_typed_case(
            None,
            false,
            vec![
                grace,
                CleanupStep::new("SameOwnerReaped", true, Some(primary.clone())),
            ],
            false,
            false,
        );
        let RenderOutcome::Failed(failure) = result else {
            panic!("elapsed grace published")
        };
        assert_eq!(failure.backend.as_deref(), Some(&primary));
        assert_eq!(
            failure.kind,
            if kind == BackendFailureKind::Cancelled {
                FailureKind::Cancelled
            } else {
                FailureKind::Deadline
            }
        );
        assert!(failure.cleanup_grace_elapsed);
        assert_eq!(
            failure.cleanup,
            ["Cleanup grace elapsed before termination was acknowledged"]
        );
        assert_eq!(
            &ledger[ledger.len() - 3..],
            ["PendingPastGrace", "SameOwnerReaped", "Dropped"]
        );
    }
}

#[test]
fn bounded_secondary_evidence_keeps_original_render_fault_and_no_output() {
    let original: BackendFailure = "original Process fault".into();
    let mut cleanup = CleanupStep::new(
        "ReapedWithDetails",
        true,
        Some("first cleanup fault".into()),
    );
    cleanup.diagnostics = (0..32)
        .map(|n| format!("detail {n}: {}", "界".repeat(4000)))
        .collect();
    let (result, _) = run_typed_case(Some(original.clone()), false, vec![cleanup], false, false);
    let RenderOutcome::Failed(failure) = result else {
        panic!("cleanup faults published")
    };
    assert_eq!(failure.backend.as_deref(), Some(&original));
    assert_eq!(failure.cleanup[0], "first cleanup fault");
    assert_eq!(failure.cleanup.len(), MAX_CLEANUP_DIAGNOSTICS + 1);
    assert!(
        failure
            .cleanup
            .iter()
            .all(|s| s.len() <= MAX_BACKEND_DIAGNOSTIC_BYTES)
    );
    assert_eq!(
        failure.cleanup.last().unwrap(),
        "Additional cleanup diagnostics omitted after the first eight"
    );
    let mut duplicates = Vec::new();
    for _ in 0..32 {
        record_cleanup_error(&mut duplicates, "same detail".into());
    }
    assert_eq!(duplicates, ["same detail"]);
}

#[test]
fn destructor_panic_is_real_fault_but_cannot_replace_prior_render_or_cleanup_failure() {
    for earlier in [None, Some("render"), Some("cleanup")] {
        let first = typed_error(
            BackendFailureKind::Backend,
            BackendDelivery::MayHaveApplied,
            "earlier fault",
        );
        let (result, _) = run_typed_case(
            (earlier == Some("render")).then(|| first.clone()),
            false,
            vec![CleanupStep::new(
                "ReapedBeforePanic",
                true,
                (earlier == Some("cleanup")).then(|| first.clone()),
            )],
            true,
            true,
        );
        let RenderOutcome::Failed(failure) = result else {
            panic!("destructor panic published")
        };
        if earlier.is_some() {
            assert_eq!(failure.kind, FailureKind::Backend);
            assert_eq!(failure.backend.as_deref(), Some(&first));
            assert_eq!(failure.cleanup, ["Disposed backend destructor panicked"]);
        } else {
            assert_eq!(failure.kind, FailureKind::Panic);
            assert_eq!(failure.backend, None);
            assert_eq!(failure.primary, "Disposed backend destructor panicked");
        }
    }
}

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
#[test]
fn strict_failure_conversion_preserves_every_exact_category_and_metadata_refusal() {
    use vst3_host::{MetadataQueryRejection, strict_offline as strict};
    let kinds = [
        strict::FailureKind::UnsupportedRequest,
        strict::FailureKind::UnsupportedReply,
        strict::FailureKind::InvalidRequest,
        strict::FailureKind::MalformedReply,
        strict::FailureKind::WrongReply,
        strict::FailureKind::Limit,
        strict::FailureKind::Shape,
        strict::FailureKind::Deadline,
        strict::FailureKind::Cancelled,
        strict::FailureKind::Allocation,
        strict::FailureKind::Io(5),
        strict::FailureKind::Eof,
        strict::FailureKind::ChildExited,
        strict::FailureKind::ZeroWrite,
        strict::FailureKind::Unsolicited,
        strict::FailureKind::Terminal,
        strict::FailureKind::Busy,
        strict::FailureKind::Sigpipe,
        strict::FailureKind::Spawn,
        strict::FailureKind::OperationRejected,
        strict::FailureKind::ReportedCrash,
        strict::FailureKind::MetadataRejected(MetadataQueryRejection::Unsupported),
        strict::FailureKind::MetadataRejected(MetadataQueryRejection::PendingTopology),
        strict::FailureKind::MetadataRejected(MetadataQueryRejection::InconsistentTopology),
        strict::FailureKind::CleanupNotBegun,
    ];
    for kind in kinds {
        for delivery in [strict::Delivery::NotSent, strict::Delivery::MayHaveApplied] {
            for diagnostic in [None, Some("界".repeat(4000))] {
                let failure = BackendFailure::from(strict::Failure {
                    kind,
                    delivery,
                    diagnostic,
                    diagnostic_unavailable: Some(strict::FailureKind::Allocation),
                    request_context: None,
                    limit_evidence: None,
                });
                let ctl = control();
                let rendered = ctl
                    .call::<()>(|| {
                        ctl.export.cancel();
                        Err(failure.clone())
                    })
                    .unwrap_err();
                let retained = rendered.backend.as_deref().unwrap();
                assert_eq!(retained, &failure);
                assert_eq!(retained.strict_kind, Some(kind));
                assert_eq!(
                    retained.strict_diagnostic_unavailable,
                    Some(strict::FailureKind::Allocation)
                );
                assert_eq!(
                    retained.delivery,
                    Some(match delivery {
                        strict::Delivery::NotSent => BackendDelivery::NotSent,
                        strict::Delivery::MayHaveApplied => BackendDelivery::MayHaveApplied,
                    })
                );
                assert_eq!(
                    rendered.kind,
                    match kind {
                        strict::FailureKind::Cancelled => FailureKind::Cancelled,
                        strict::FailureKind::Deadline => FailureKind::Deadline,
                        _ => FailureKind::Backend,
                    }
                );
                assert!(rendered.primary.len() <= MAX_BACKEND_DIAGNOSTIC_BYTES);
            }
        }
    }
}

#[test]
fn typed_cancel_or_deadline_pending_retains_same_backend_admission_and_no_destination_change() {
    for (kind, expected) in [
        (BackendFailureKind::Cancelled, FailureKind::Cancelled),
        (BackendFailureKind::Deadline, FailureKind::Deadline),
    ] {
        let c = capture(128, 0, 0, 2);
        let ctl = control();
        let mut fake = Fake::new(c.metadata().clone(), ctl.clone(), Fault::None, 128);
        let error = typed_error(
            kind,
            BackendDelivery::MayHaveApplied,
            "interrupted cleanup exchange",
        );
        fake.cleanup_failure = Some(error.clone());
        fake.cleanup_gate.store(false, Ordering::Release);
        let gate = Arc::clone(&fake.cleanup_gate);
        let disposed = Arc::clone(&fake.disposed);
        let ledger = Arc::clone(&fake.ledger);
        let files = Files::new();
        std::fs::write(files.output(), b"original destination").unwrap();
        let admission = RenderAdmission::default();
        let receipt = admission
            .start(c, files.output(), true, ctl, move || fake)
            .unwrap();
        wait_until(|| ledger.lock().unwrap().iter().any(|s| s == "DisposePending"));
        receipt.expire_cleanup_grace();
        wait_until(|| matches!(receipt.status(), RenderStatus::CleanupPending { .. }));
        assert!(!receipt.termination_acknowledged());
        assert!(matches!(receipt.try_recv(), Err(TryRecvError::Empty)));
        assert!(admission.is_busy());
        assert!(!disposed.load(Ordering::Acquire));
        assert_eq!(
            std::fs::read(files.output()).unwrap(),
            b"original destination"
        );
        assert_eq!(std::fs::read_dir(&files.0).unwrap().count(), 1);
        let rejected = admission.start(
            capture(128, 0, 0, 2),
            files.output(),
            true,
            control(),
            || -> Fake {
                panic!("typed cancellation cannot release an unreaped owner's admission")
            },
        );
        assert!(rejected.err().unwrap().contains("still owns"));
        receipt.cancel();
        gate.store(true, Ordering::Release);
        let RenderOutcome::Failed(failure) = outcome(&receipt) else {
            panic!("typed cleanup failure published")
        };
        assert_eq!(failure.kind, expected);
        assert_eq!(failure.backend.as_deref(), Some(&error));
        assert!(failure.cleanup_grace_elapsed);
        assert_eq!(
            failure.cleanup,
            ["Cleanup grace elapsed before termination was acknowledged"]
        );
        assert!(!admission.is_busy());
        assert!(disposed.load(Ordering::Acquire));
        let ledger = ledger.lock().unwrap();
        assert_eq!(ledger.iter().filter(|s| s.as_str() == "Prepare").count(), 1);
        assert_eq!(&ledger[ledger.len() - 2..], ["Disposed", "Dropped"]);
        assert_eq!(
            std::fs::read(files.output()).unwrap(),
            b"original destination"
        );
        assert_eq!(std::fs::read_dir(&files.0).unwrap().count(), 1);
    }
}

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
#[test]
fn preceding_render_fault_keeps_first_exact_strict_cleanup_error_across_later_observations() {
    use vst3_host::strict_offline as strict;
    for kind in [
        strict::FailureKind::Cancelled,
        strict::FailureKind::Deadline,
    ] {
        for delivery in [strict::Delivery::NotSent, strict::Delivery::MayHaveApplied] {
            let first: BackendFailure = "earlier renderer backend fault".into();
            let cleanup = BackendFailure::from(strict::Failure {
                kind,
                delivery,
                diagnostic: Some("interrupted strict cleanup".into()),
                diagnostic_unavailable: None,
                request_context: None,
                limit_evidence: None,
            });
            let mut pending =
                CleanupStep::new("PendingStrictCleanup", false, Some(cleanup.clone()));
            pending.cancel = true;
            let reap = CleanupStep::new("Reaped", true, Some("later OS fault".into()));
            let (result, _) = run_typed_case(
                Some(first.clone()),
                false,
                vec![pending, reap],
                false,
                false,
            );
            let RenderOutcome::Failed(failure) = result else {
                panic!("strict cleanup interruption published")
            };
            assert_eq!(failure.kind, FailureKind::Backend);
            assert_eq!(failure.backend.as_deref(), Some(&first));
            assert_eq!(failure.cleanup_backend.as_deref(), Some(&cleanup));
            assert_eq!(
                failure.cleanup,
                ["interrupted strict cleanup", "later OS fault"]
            );
        }
    }
}
