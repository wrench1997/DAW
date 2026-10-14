//! Plugin-free unit fixtures for the dormant bounce checkpoint. No vendor module is loaded.

use super::*;
use crate::instrument_bounce::BounceAudioBus;
use crate::instrument_bounce::{
    BOUNCE_CAPTURE_TIMEOUT, BouncePluginMetadata, BounceSourceIdentity, BounceSourceStatus,
    InstrumentBounceCapture, InstrumentBouncePlan,
};
use std::collections::BTreeMap;

fn state() -> BouncePluginState {
    BouncePluginState {
        metadata: BouncePluginMetadata {
            class_uid: "ABCDEF019182FAEB566D624153675854".into(),
            version: "1.3.4".into(),
            name: "Fixture".into(),
            category: "Instrument|Synth".into(),
            event_input_buses: vec![(16, true, Some(true))],
            event_output_buses: vec![],
            input_buses: vec![],
            output_buses: vec![BounceAudioBus::new(2, true, Some(true))],
            latency_samples: 0,
            tail_samples: 0,
        },
        opaque_state: vec![1, 2, 3],
        parameter_bases: BTreeMap::from([(17, 0.75)]),
        native_generation: 2,
        native_dirty_revision: 9,
    }
}

struct CaptureBackend {
    result: Result<BouncePluginState, BounceCaptureError>,
    captures: Arc<AtomicU64>,
    changed_epoch: Option<Arc<AtomicU64>>,
    capture_hook: Option<fn(&BounceCaptureFence)>,
}

impl PluginBackend for CaptureBackend {
    fn name(&self) -> &str {
        "Capture fixture"
    }
    fn prepare(&mut self, _: PluginPrepareConfig) -> Result<(), String> {
        panic!("unexpected prepare")
    }
    fn process(&mut self, _: &mut [f32], _: &mut [f32], _: usize) -> Result<(), String> {
        panic!("unexpected Process")
    }
    fn send_midi(&mut self, _: MidiMessage) -> Result<(), String> {
        panic!("unexpected MIDI")
    }
    fn set_parameter(&mut self, _: u32, _: f32) -> Result<(), String> {
        panic!("unexpected setParameter")
    }
    fn get_parameter(&mut self, _: u32) -> Result<f32, String> {
        panic!("unexpected getter")
    }
    fn native_editor(&mut self, _: NativeEditorCommand) -> Result<NativeEditorState, String> {
        panic!("unexpected editor mutation")
    }
    fn save_state(&mut self) -> Result<Vec<u8>, String> {
        panic!("unexpected ordinary SaveState")
    }
    fn load_state(&mut self, _: &[u8]) -> Result<(), String> {
        panic!("unexpected source LoadState")
    }
    fn reset_processing(&mut self) -> Result<(), String> {
        panic!("unexpected source reset")
    }
    fn latency_samples(&self) -> u32 {
        0
    }
    fn tail_samples(&self) -> u32 {
        0
    }
    fn midi_capabilities(&self) -> (bool, bool) {
        (true, false)
    }
    fn capture_instrument_bounce_state(
        &mut self,
        ids: &[u32],
        fence: &BounceCaptureFence,
    ) -> Result<BouncePluginState, BounceCaptureError> {
        fence.checkpoint()?;
        assert_eq!(ids, &[17]);
        self.captures.fetch_add(1, Ordering::SeqCst);
        if let Some(epoch) = &self.changed_epoch {
            epoch.store(8, Ordering::Release);
        }
        if let Some(hook) = self.capture_hook {
            hook(fence);
        }
        self.result.clone()
    }
}

fn slot(
    result: Result<BouncePluginState, BounceCaptureError>,
    captures: Arc<AtomicU64>,
    changed_epoch: Option<Arc<AtomicU64>>,
) -> WorkerSlot {
    WorkerSlot {
        backend: Some(Box::new(CaptureBackend {
            result,
            captures,
            changed_epoch,
            capture_hook: None,
        })),
        config: SlotConfig::default(),
        fault: None,
        parameter_catalog_cache: None,
        native_base_ids: vec![17],
    }
}

fn capture_fence() -> BounceCaptureFence {
    BounceCaptureFence::new(Instant::now() + BOUNCE_CAPTURE_TIMEOUT)
}

#[test]
fn bounce_worker_capture_has_no_normal_state_or_native_publication() {
    let captures = Arc::new(AtomicU64::new(0));
    let mut slots = vec![slot(Ok(state()), Arc::clone(&captures), None)];
    let metrics = BridgeMetrics::default();
    let before = metrics.native_editors.lock().unwrap()[0].clone();
    let (mut events, mut rx) = RingBuffer::new(4);
    let receipt = capture_instrument_bounce_on_worker(
        &capture_fence(),
        7,
        7,
        &AtomicU64::new(7),
        true,
        &[17],
        &AtomicU32::new(0),
        &mut slots,
        &mut events,
        &metrics,
    )
    .unwrap();
    assert_eq!(receipt.state, state());
    assert_eq!(receipt.epoch, 7);
    assert_eq!(captures.load(Ordering::SeqCst), 1);
    assert!(rx.pop().is_err());
    let after = metrics.native_editors.lock().unwrap()[0].clone();
    assert_eq!(before.capture_serial, after.capture_serial);
    assert_eq!(
        before.parameter_capture_serial,
        after.parameter_capture_serial
    );
    assert_eq!(before.captured_state, after.captured_state);
    assert_eq!(before.dirty_revision, after.dirty_revision);
    assert!(slots[0].fault.is_none());
}

#[test]
fn bounce_worker_rejects_stale_nonquiescent_editor_and_pending_edit_without_capture() {
    for case in 0..7 {
        let captures = Arc::new(AtomicU64::new(0));
        let mut slots = vec![slot(Ok(state()), Arc::clone(&captures), None)];
        let metrics = BridgeMetrics::default();
        if case == 3 {
            metrics.native_editors.lock().unwrap()[0].state.open = true;
        }
        if case == 4 {
            metrics.native_editors.lock().unwrap()[0].pending_request = Some(55);
        }
        if case == 6 {
            slots[0].fault = Some("existing fault".into());
        }
        let (mut events, mut rx) = RingBuffer::new(4);
        let result = capture_instrument_bounce_on_worker(
            &capture_fence(),
            7,
            if case == 0 { 6 } else { 7 },
            &AtomicU64::new(if case == 1 { 8 } else { 7 }),
            case != 2,
            &[17],
            &AtomicU32::new(u32::from(case == 5)),
            &mut slots,
            &mut events,
            &metrics,
        );
        assert!(
            matches!(result, Err(BounceCaptureError::Rejected(_))),
            "case {case}"
        );
        assert_eq!(captures.load(Ordering::SeqCst), 0);
        assert!(rx.pop().is_err());
    }
}

#[test]
fn bounce_epoch_changes_after_capture_never_publish_success() {
    let epoch = Arc::new(AtomicU64::new(7));
    let mut slots = vec![slot(
        Ok(state()),
        Arc::new(AtomicU64::new(0)),
        Some(Arc::clone(&epoch)),
    )];
    let (mut events, mut rx) = RingBuffer::new(4);
    let result = capture_instrument_bounce_on_worker(
        &capture_fence(),
        7,
        7,
        &epoch,
        true,
        &[17],
        &AtomicU32::new(0),
        &mut slots,
        &mut events,
        &BridgeMetrics::default(),
    );
    assert!(matches!(result, Err(BounceCaptureError::Rejected(_))));
    assert!(rx.pop().is_err());
    assert!(slots[0].fault.is_none());
}

#[test]
fn bounce_backend_failure_stays_visible_but_capture_rejection_does_not_fault() {
    for backend_failed in [false, true] {
        let error = if backend_failed {
            BounceCaptureError::BackendFailed("helper failed".into())
        } else {
            BounceCaptureError::Rejected("unsupported state".into())
        };
        let mut slots = vec![slot(Err(error), Arc::new(AtomicU64::new(0)), None)];
        let (mut events, mut rx) = RingBuffer::new(4);
        assert!(
            capture_instrument_bounce_on_worker(
                &capture_fence(),
                7,
                7,
                &AtomicU64::new(7),
                true,
                &[17],
                &AtomicU32::new(0),
                &mut slots,
                &mut events,
                &BridgeMetrics::default()
            )
            .is_err()
        );
        assert_eq!(slots[0].fault.is_some(), backend_failed);
        assert_eq!(
            matches!(rx.pop(), Ok(RuntimeEvent::SlotFault { .. })),
            backend_failed
        );
    }
}

#[test]
fn bounce_capture_failure_republishes_inactive_identity_when_fault_event_is_lost() {
    let mut slots = vec![slot(
        Err(BounceCaptureError::BackendFailed(
            "helper transport failed".into(),
        )),
        Arc::new(AtomicU64::new(0)),
        None,
    )];
    let metrics = BridgeMetrics::default();
    let (mut events, mut rx) = RingBuffer::new(1);
    update_latency_and_tail(&mut slots, &mut events, &metrics);
    let before = metrics.plugin_latency_snapshot().unwrap();
    assert_eq!(before.active_mask, 1);
    assert_eq!(metrics.midi_capabilities.load(Ordering::Acquire), 1);
    events.push(RuntimeEvent::ShutdownComplete).unwrap();
    assert!(
        capture_instrument_bounce_on_worker(
            &capture_fence(),
            7,
            7,
            &AtomicU64::new(7),
            true,
            &[17],
            &AtomicU32::new(0),
            &mut slots,
            &mut events,
            &metrics
        )
        .is_err()
    );
    let after = metrics.plugin_latency_snapshot().unwrap();
    assert_eq!(after.active_mask, 0);
    assert!(after.revision > before.revision);
    assert_eq!(metrics.midi_capabilities.load(Ordering::Acquire), 0);
    assert_eq!(metrics.event_overflows.load(Ordering::Acquire), 1);
    assert!(matches!(rx.pop(), Ok(RuntimeEvent::ShutdownComplete)));
    assert!(rx.pop().is_err());
}

fn control(capacity: usize) -> (PluginChainControl, Receiver<QueuedAdminCommand>) {
    let (admin, rx) = mpsc::sync_channel(capacity);
    let metrics = Arc::new(BridgeMetrics::default());
    let admin = AdminCommandSender::new(admin, &metrics);
    let (_, events) = RingBuffer::new(4);
    let (_, receipts) = RingBuffer::new(4);
    (
        PluginChainControl {
            admin,
            events,
            metrics,
            manifest: PluginEndpointManifest::identified(&[41]).unwrap(),
            parameter_edit_receipts: receipts,
            parameter_edit_admission: Arc::new(AtomicU32::new(0)),
        },
        rx,
    )
}

#[test]
fn bounce_receiver_drop_keeps_admission_until_queued_worker_lease_terminates() {
    let (control, commands) = control(1);
    let receipt = control
        .request_instrument_bounce_state(41, 7, vec![17])
        .unwrap();
    let fence = Arc::clone(&receipt.fence);
    drop(receipt);
    assert!(control.metrics.bounce_capture_busy.load(Ordering::Acquire));
    assert!(
        control
            .request_instrument_bounce_state(41, 7, vec![17])
            .err()
            .unwrap()
            .contains("already pending")
    );
    let mut queued = commands.try_recv().unwrap();
    let AdminCommand::CaptureInstrumentBounce { lease, reply, .. } = queued.take_command() else {
        panic!("wrong command")
    };
    assert!(fence.cancelled.load(Ordering::Acquire));
    assert!(!fence.terminated.load(Ordering::Acquire));
    let captures = Arc::new(AtomicU64::new(0));
    let mut slots = vec![slot(Ok(state()), Arc::clone(&captures), None)];
    let (mut events, _) = RingBuffer::new(4);
    let result = capture_instrument_bounce_on_worker(
        &lease.fence,
        7,
        7,
        &AtomicU64::new(7),
        true,
        &[17],
        &AtomicU32::new(0),
        &mut slots,
        &mut events,
        &control.metrics,
    );
    assert!(
        matches!(result, Err(BounceCaptureError::Rejected(ref reason)) if reason.contains("cancelled"))
    );
    assert_eq!(captures.load(Ordering::Acquire), 0);
    assert!(matches!(
        reply.try_send(result),
        Err(TrySendError::Disconnected(_))
    ));
    assert!(control.metrics.bounce_capture_busy.load(Ordering::Acquire));
    drop(lease);
    assert!(fence.terminated.load(Ordering::Acquire));
    assert!(!control.metrics.bounce_capture_busy.load(Ordering::Acquire));
    let next = control
        .request_instrument_bounce_state(41, 7, vec![17])
        .unwrap();
    drop(commands); // Real std channel teardown drops its queued lease even while sender lives.
    assert!(next.termination_acknowledged());
    assert!(matches!(next.try_recv(), Err(TryRecvError::Disconnected)));
    assert!(!control.metrics.bounce_capture_busy.load(Ordering::Acquire));
}

#[test]
fn bounce_admission_releases_on_queue_rejection_and_old_receiver_cannot_release_new_job() {
    let (control, commands) = control(1);
    assert!(control.try_admin(AdminCommand::Shutdown));
    assert!(
        control
            .request_instrument_bounce_state(41, 7, vec![17])
            .err()
            .unwrap()
            .contains("queue is full or disconnected")
    );
    assert!(!control.metrics.bounce_capture_busy.load(Ordering::Acquire));
    assert!(matches!(
        commands.try_recv().map(|mut queued| queued.take_command()),
        Ok(AdminCommand::Shutdown)
    ));
    let first = control
        .request_instrument_bounce_state(41, 7, vec![17])
        .unwrap();
    drop(commands.try_recv().unwrap());
    assert!(first.termination_acknowledged());
    let second = control
        .request_instrument_bounce_state(41, 7, vec![17])
        .unwrap();
    drop(first);
    assert!(control.metrics.bounce_capture_busy.load(Ordering::Acquire));
    assert!(!second.termination_acknowledged());
    drop(commands.try_recv().unwrap());
    assert!(second.termination_acknowledged());
    assert!(!control.metrics.bounce_capture_busy.load(Ordering::Acquire));
    drop(commands);
    assert!(
        control
            .request_instrument_bounce_state(41, 7, vec![17])
            .err()
            .unwrap()
            .contains("disconnected")
    );
    assert!(!control.metrics.bounce_capture_busy.load(Ordering::Acquire));
}

#[test]
fn bounce_observed_termination_ack_guarantees_admission_is_already_released() {
    let (control, commands) = control(1);
    let first = control
        .request_instrument_bounce_state(41, 7, vec![17])
        .unwrap();
    let mut queued = commands.try_recv().unwrap();
    let AdminCommand::CaptureInstrumentBounce { lease, .. } = queued.take_command() else {
        panic!("wrong command")
    };
    let reached = Arc::new(std::sync::Barrier::new(2));
    let resume = Arc::new(std::sync::Barrier::new(2));
    *first.fence.before_ack.lock().unwrap() = Some((Arc::clone(&reached), Arc::clone(&resume)));
    let worker = std::thread::spawn(move || drop(lease));
    reached.wait(); // Worker is deterministically between admission release and final ack.
    let released_before_ack = !control.metrics.bounce_capture_busy.load(Ordering::Acquire);
    let not_yet_acknowledged = !first.termination_acknowledged();
    resume.wait(); // Release even if the assertions below discover a regression.
    assert!(released_before_ack);
    assert!(not_yet_acknowledged);
    while !first.termination_acknowledged() {
        std::thread::yield_now();
    }
    // Do not join the worker before the retry: the observed ack itself is the guarantee.
    let second = control
        .request_instrument_bounce_state(41, 7, vec![17])
        .expect("acknowledged old lease must no longer hold admission");
    worker.join().unwrap();
    drop(first);
    assert!(control.metrics.bounce_capture_busy.load(Ordering::Acquire));
    assert!(!second.termination_acknowledged());
    drop(commands.try_recv().unwrap());
    assert!(second.termination_acknowledged());
    assert!(!control.metrics.bounce_capture_busy.load(Ordering::Acquire));
}

#[test]
fn bounce_expired_queued_request_has_no_backend_calls_and_panic_unwinds_lease() {
    let (control, commands) = control(1);
    let receipt = control
        .request_instrument_bounce_state_until(41, 7, vec![17], Instant::now())
        .unwrap();
    let mut queued = commands.try_recv().unwrap();
    let AdminCommand::CaptureInstrumentBounce { lease, reply, .. } = queued.take_command() else {
        panic!("wrong command")
    };
    let captures = Arc::new(AtomicU64::new(0));
    let mut slots = vec![slot(Ok(state()), Arc::clone(&captures), None)];
    let (mut events, _) = RingBuffer::new(4);
    let result = capture_instrument_bounce_on_worker(
        &lease.fence,
        7,
        7,
        &AtomicU64::new(7),
        true,
        &[17],
        &AtomicU32::new(0),
        &mut slots,
        &mut events,
        &control.metrics,
    );
    assert!(
        matches!(result, Err(BounceCaptureError::Rejected(ref reason)) if reason.contains("timed out"))
    );
    assert_eq!(captures.load(Ordering::Acquire), 0);
    reply.try_send(result).unwrap();
    assert!(matches!(receipt.try_recv(), Err(TryRecvError::Empty)));
    let unwind = panic::catch_unwind(AssertUnwindSafe(move || {
        let _lease = lease;
        panic!("synthetic worker teardown");
    }));
    assert!(unwind.is_err());
    assert!(receipt.termination_acknowledged());
    assert!(!control.metrics.bounce_capture_busy.load(Ordering::Acquire));
    assert!(
        matches!(receipt.try_recv(), Ok(Err(BounceCaptureError::Rejected(reason))) if reason.contains("timed out"))
    );
}

#[test]
fn bounce_cancel_during_capture_preserves_real_failure_and_panic_as_reliable_fault() {
    for case in 0..3 {
        let captures = Arc::new(AtomicU64::new(0));
        let mut slots = vec![slot(Ok(state()), Arc::clone(&captures), None)];
        slots[0].backend = Some(Box::new(CaptureBackend {
            result: if case == 1 {
                Err(BounceCaptureError::BackendFailed(
                    "getter IPC failure".into(),
                ))
            } else {
                Ok(state())
            },
            captures,
            changed_epoch: None,
            capture_hook: Some(if case == 2 {
                |fence| {
                    fence.cancel();
                    panic!("capture panic");
                }
            } else {
                |fence| fence.cancel()
            }),
        }));
        let metrics = BridgeMetrics::default();
        let (mut events, mut rx) = RingBuffer::new(4);
        update_latency_and_tail(&mut slots, &mut events, &metrics);
        let fence = capture_fence();
        let result = capture_instrument_bounce_on_worker(
            &fence,
            7,
            7,
            &AtomicU64::new(7),
            true,
            &[17],
            &AtomicU32::new(0),
            &mut slots,
            &mut events,
            &metrics,
        );
        if case == 0 {
            assert!(
                matches!(result, Err(BounceCaptureError::Rejected(reason)) if reason.contains("cancelled"))
            );
            assert!(slots[0].fault.is_none());
            assert_eq!(metrics.plugin_latency_snapshot().unwrap().active_mask, 1);
            assert!(rx.pop().is_err());
        } else {
            let message = if case == 1 {
                "getter IPC failure"
            } else {
                "capture panic"
            };
            assert!(
                matches!(result, Err(BounceCaptureError::BackendFailed(reason)) if reason.contains(message))
            );
            assert!(slots[0].fault.as_ref().unwrap().contains(message));
            assert_eq!(metrics.plugin_latency_snapshot().unwrap().active_mask, 0);
            assert!(matches!(rx.pop(), Ok(RuntimeEvent::SlotFault { .. })));
        }
    }
}

fn project() -> crate::model::Project {
    let mut project = crate::model::Project::default();
    project.automation_lanes.clear();
    project.channels[0].instrument_plugin_instance_id = Some(41);
    project.plugin_instances.push(crate::model::PluginInstance {
        id: 41,
        format: crate::model::PluginFormat::Vst3,
        role: crate::model::PluginRole::Instrument,
        path: "fixture.vst3".into(),
        uid: "historical-path-id".into(),
        vendor: "Fixture".into(),
        name: "Fixture".into(),
        enabled: true,
        bypass: false,
        wet: 1.0,
        parameters: BTreeMap::from([(17, 0.1)]),
        opaque_state: vec![9],
        runtime_status: crate::model::PluginRuntimeStatus::Loaded,
        midi_ports: Default::default(),
    });
    project
}

fn status() -> BounceSourceStatus {
    BounceSourceStatus {
        confirmed_stopped: true,
        exact_timeline: true,
        source_ready: true,
        ..Default::default()
    }
}

fn identity(plan: &InstrumentBouncePlan) -> BounceSourceIdentity {
    BounceSourceIdentity {
        project_session: 2,
        project_fingerprint: plan.project_fingerprint(),
        timeline_revision: 4,
        epoch: 7,
        endpoint_id: 19,
        channel_id: plan.channel_id(),
        instance_id: plan.instance_id(),
    }
}

#[test]
fn bounce_coordinator_captures_current_bases_without_changing_project() {
    let project = project();
    let before = serde_json::to_vec(&project).unwrap();
    let plan =
        InstrumentBouncePlan::prepare(&project, project.channels[0].id, 48_000, 48_000).unwrap();
    let stamp = identity(&plan);
    let (control, commands) = control(1);
    let now = Instant::now();
    let mut capture = InstrumentBounceCapture::begin(plan, stamp, status(), &control, now).unwrap();
    assert!(capture.poll(stamp, status(), now).unwrap().is_none());
    let mut queued = commands.try_recv().unwrap();
    let AdminCommand::CaptureInstrumentBounce { reply, lease, .. } = queued.take_command() else {
        panic!("wrong command")
    };
    reply
        .send(Ok(BounceStateReceipt {
            epoch: 7,
            state: state(),
        }))
        .unwrap();
    assert!(capture.poll(stamp, status(), now).unwrap().is_none());
    assert!(!capture.termination_acknowledged());
    assert!(control.metrics.bounce_capture_busy.load(Ordering::Acquire));
    drop(lease);
    let captured = capture.poll(stamp, status(), now).unwrap().unwrap();
    assert_eq!(captured.identity(), stamp);
    assert_eq!(captured.state.parameter_bases[&17], 0.75);
    assert_eq!(captured.render_spec().initial_state, vec![1, 2, 3]);
    assert_eq!(
        captured.render_spec().class_uid.as_deref(),
        Some("ABCDEF019182FAEB566D624153675854")
    );
    assert_eq!(serde_json::to_vec(&project).unwrap(), before);
    assert!(!control.metrics.bounce_capture_busy.load(Ordering::Acquire));
    assert!(capture.poll(stamp, status(), now).is_err());
}

#[test]
fn bounce_coordinator_retains_cancel_timeout_stale_and_disconnect_until_worker_ack() {
    for case in 0..6 {
        let project = project();
        let original = serde_json::to_vec(&project).unwrap();
        let plan =
            InstrumentBouncePlan::prepare(&project, project.channels[0].id, 48_000, 0).unwrap();
        let stamp = identity(&plan);
        let (control, commands) = control(1);
        let now = Instant::now();
        let mut capture =
            InstrumentBounceCapture::begin(plan, stamp, status(), &control, now).unwrap();
        let mut queued = commands.try_recv().unwrap();
        let AdminCommand::CaptureInstrumentBounce { reply, lease, .. } = queued.take_command()
        else {
            panic!("wrong command")
        };
        if case == 5 {
            drop(reply);
        } else {
            reply
                .send(Ok(BounceStateReceipt {
                    epoch: if case == 2 { 8 } else { 7 },
                    state: state(),
                }))
                .unwrap();
        }
        let mut observed = stamp;
        if case == 0 {
            observed.project_session += 1;
        }
        if case == 1 {
            observed.endpoint_id += 1;
        }
        if case == 4 {
            capture.cancel();
        }
        let polled = if case == 3 {
            now + BOUNCE_CAPTURE_TIMEOUT
        } else {
            now
        };
        assert!(
            capture.poll(observed, status(), polled).unwrap().is_none(),
            "case {case}"
        );
        assert!(!capture.termination_acknowledged());
        assert_eq!(capture.is_cancelling(), matches!(case, 0 | 1 | 3 | 4));
        assert!(control.metrics.bounce_capture_busy.load(Ordering::Acquire));
        assert!(
            control
                .request_instrument_bounce_state(41, 7, vec![17])
                .err()
                .unwrap()
                .contains("already pending")
        );
        drop(lease);
        assert!(capture.termination_acknowledged());
        let error = capture.poll(observed, status(), polled).err().unwrap();
        let expected = [
            "source changed",
            "source changed",
            "stale epoch",
            "timed out",
            "cancelled",
            "disconnected",
        ][case];
        assert!(error.contains(expected), "case {case}: {error}");
        assert_eq!(serde_json::to_vec(&project).unwrap(), original);
        assert!(!control.metrics.bounce_capture_busy.load(Ordering::Acquire));
    }
}

#[test]
fn bounce_coordinator_cancel_does_not_replace_a_real_worker_failure() {
    let project = project();
    let plan = InstrumentBouncePlan::prepare(&project, project.channels[0].id, 48_000, 0).unwrap();
    let stamp = identity(&plan);
    let (control, commands) = control(1);
    let now = Instant::now();
    let mut capture = InstrumentBounceCapture::begin(plan, stamp, status(), &control, now).unwrap();
    let mut queued = commands.try_recv().unwrap();
    let AdminCommand::CaptureInstrumentBounce { reply, lease, .. } = queued.take_command() else {
        panic!("wrong command")
    };
    capture.cancel();
    reply
        .send(Err(BounceCaptureError::BackendFailed(
            "real IPC fault wins".into(),
        )))
        .unwrap();
    assert!(capture.poll(stamp, status(), now).unwrap().is_none());
    drop(lease);
    assert_eq!(
        capture.poll(stamp, status(), now).err().unwrap(),
        "real IPC fault wins"
    );
    assert!(!capture.is_cancelling());
}

#[test]
fn bounce_metadata_and_state_policy_rejects_unsupported_inputs_and_empty_state() {
    state().validate(&[17], 48_000).unwrap();
    for case in 0..8 {
        let mut state = state();
        match case {
            0 => state.opaque_state.clear(),
            1 => state
                .metadata
                .input_buses
                .push(BounceAudioBus::new(3, false, Some(false))),
            2 => state
                .metadata
                .output_buses
                .push(BounceAudioBus::new(2, true, Some(true))),
            3 => state
                .metadata
                .event_output_buses
                .push((16, true, Some(false))),
            4 => state.metadata.class_uid = "historical-path-id".into(),
            5 => state.metadata.latency_samples = 48_001,
            6 => {
                state.parameter_bases.insert(17, f32::NAN);
            }
            _ => {
                state.parameter_bases.insert(18, 0.4);
            }
        }
        assert!(state.validate(&[17], 48_000).is_err(), "case {case}");
    }
}

#[test]
fn bounce_eligibility_does_not_silently_remove_unsupported_project_features() {
    for case in 0..6 {
        let mut project = project();
        match case {
            0 => project.plugin_instances[0].format = crate::model::PluginFormat::Vst2,
            1 => project.plugin_instances[0].bypass = true,
            2 => project.plugin_instances[0].wet = 0.5,
            3 => project.plugin_instances[0].midi_ports.output = Some(1),
            4 => project.channels[0].muted = true,
            _ => project.clips[0].pattern_id = u32::MAX,
        }
        assert!(
            InstrumentBouncePlan::prepare(&project, project.channels[0].id, 48_000, 0).is_err(),
            "case {case}"
        );
    }
}

#[test]
fn selected_stereo_metadata_requires_exact_kontakt_identity_complete_activation_and_preserved_event_output()
 {
    let mut metadata = state().metadata;
    metadata.class_uid = "5653544E694B386B6F6E74616B742038".into();
    metadata.version = "8.4.1".into();
    metadata.output_buses = (0..32)
        .map(|index| BounceAudioBus::new(2, true, Some(index == 0)))
        .collect();
    metadata.event_output_buses = vec![(16, true, Some(true))];
    let original = metadata.clone();
    metadata.validate(48_000).unwrap();
    assert_eq!(metadata, original);
    for invalid in 0..8 {
        let mut changed = original.clone();
        match invalid {
            0 => changed.version = "8.4.2".into(),
            1 => changed.class_uid = "00000000000000000000000000000000".into(),
            2 => changed.output_buses[31].active = None,
            3 => changed.output_buses[31].active = Some(true),
            4 => changed.output_buses[0].channel_count = 1,
            5 => changed
                .output_buses
                .push(BounceAudioBus::new(1, true, Some(false))),
            6 => changed.output_buses[31].channel_count = 3,
            _ => changed.event_output_buses[0].2 = None,
        }
        assert!(
            changed.validate(48_000).is_err(),
            "invalid selected bus case {invalid}"
        );
    }
}

#[test]
fn bounce_audio_policy_keeps_every_declared_bus_and_known_activation() {
    for input_channels in [0, 1, 2] {
        for input_active in [false, true] {
            for input_main in [false, true] {
                for output_channels in [1, 2] {
                    for extras in 0..=2 {
                        for role_mask in 0..(1 << extras) {
                            for channel_mask in 0..(1 << extras) {
                                let mut metadata = state().metadata;
                                metadata.input_buses = if input_channels == 0 {
                                    vec![]
                                } else {
                                    vec![BounceAudioBus::new(
                                        input_channels,
                                        input_main,
                                        Some(input_active),
                                    )]
                                };
                                metadata.output_buses =
                                    vec![BounceAudioBus::new(output_channels, true, Some(true))];
                                metadata.output_buses.extend((0..extras).map(|index| {
                                    BounceAudioBus::new(
                                        1 + ((channel_mask >> index) & 1),
                                        role_mask & (1 << index) != 0,
                                        Some(false),
                                    )
                                }));
                                let exact = metadata.clone();
                                metadata.validate(48_000).unwrap();
                                assert_eq!(metadata, exact);
                                assert_eq!(metadata.output_buses.len(), 1 + extras);
                                assert_eq!(
                                    metadata.participating_input_channels(),
                                    if input_active { input_channels } else { 0 }
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn bounce_audio_policy_refuses_every_out_of_bound_raw_descriptor_without_projection() {
    for input_main in [false, true] {
        for extra_one_main in [false, true] {
            for extra_two_main in [false, true] {
                let mut valid = state().metadata;
                valid.input_buses = vec![BounceAudioBus::new(2, input_main, Some(true))];
                valid.output_buses.extend([
                    BounceAudioBus::new(1, extra_one_main, Some(false)),
                    BounceAudioBus::new(2, extra_two_main, Some(false)),
                ]);
                valid.validate(48_000).unwrap();
                for case in 0..20 {
                    let mut metadata = valid.clone();
                    match case {
                        0 => metadata.input_buses.push(metadata.input_buses[0]),
                        1 => metadata.input_buses[0].channel_count = 0,
                        2 => metadata.input_buses[0].channel_count = 3,
                        3 => metadata.input_buses[0].active = None,
                        4 => metadata.output_buses.clear(),
                        5 => metadata.output_buses.push(BounceAudioBus::new(
                            2,
                            extra_one_main,
                            Some(false),
                        )),
                        6 => metadata.output_buses[0].is_main = false,
                        7 => metadata.output_buses[0].active = Some(false),
                        8 => metadata.output_buses[0].active = None,
                        9 => metadata.output_buses[0].channel_count = 0,
                        10 => metadata.output_buses[0].channel_count = 3,
                        11 => metadata.output_buses[1].active = Some(true),
                        12 => metadata.output_buses[2].active = Some(true),
                        13 => metadata.output_buses[1].active = None,
                        14 => metadata.output_buses[2].active = None,
                        15 => metadata.output_buses[1].channel_count = 0,
                        16 => metadata.output_buses[1].channel_count = 3,
                        17 => metadata.output_buses[2].channel_count = 0,
                        18 => metadata.output_buses[2].channel_count = 3,
                        _ => metadata.output_buses.swap(0, 2),
                    }
                    let exact = metadata.clone();
                    assert!(metadata.validate(48_000).is_err(), "case {case}");
                    assert_eq!(metadata, exact, "admission must be observational");
                }
            }
        }
    }
}

#[test]
fn bounce_audio_supported_role_channel_and_order_drift_still_changes_full_receipt() {
    let mut valid = state().metadata;
    valid.input_buses = vec![BounceAudioBus::new(2, false, Some(true))];
    valid.output_buses.extend([
        BounceAudioBus::new(1, true, Some(false)),
        BounceAudioBus::new(2, true, Some(false)),
    ]);
    valid.validate(48_000).unwrap();
    for case in 0..6 {
        let mut changed = valid.clone();
        match case {
            0 => changed.input_buses[0].is_main = true,
            1 => changed.output_buses[1].is_main = false,
            2 => changed.output_buses[2].is_main = false,
            3 => changed.output_buses[1].channel_count = 2,
            4 => changed.output_buses.swap(1, 2),
            _ => changed.input_buses[0].active = Some(false),
        }
        let exact = changed.clone();
        changed.validate(48_000).unwrap();
        assert_eq!(changed, exact, "admission must not normalize a descriptor");
        assert_ne!(
            changed, valid,
            "supported drift {case} changes the full receipt"
        );
    }
}

fn master_song_lane(
    id: u64,
    target: crate::automation::AutomationTarget,
    start: f64,
    end: f64,
) -> crate::model::ProjectAutomation {
    let mut lane = crate::automation::AutomationLane::new(target);
    lane.replace_points([
        crate::automation::AutomationPoint::new(0.0, start),
        crate::automation::AutomationPoint::new(32.0, end),
    ]);
    crate::model::ProjectAutomation {
        id,
        name: format!("Master song lane {id}"),
        lane,
    }
}

fn master_song_project() -> crate::model::Project {
    use crate::automation::AutomationTarget;
    let mut project = project();
    project.tempo = 120.0;
    project.swing = 0.5;
    project.song_length_beats = 32.0;
    project.piano_notes.clear();
    for channel in &mut project.channels {
        channel.steps = [false; 16];
    }
    let template = project.clips[0].clone();
    project.patterns.clear();
    project.clips.clear();
    for index in 0..5 {
        let id = index as u32 + 1;
        let mut steps = vec![[false; 16]; project.channels.len()];
        steps[0][1] = true;
        steps[0][9] = true;
        project.patterns.push(crate::model::Pattern {
            id,
            name: format!("Song pattern {id}"),
            length_steps: 16,
            channel_steps: steps,
            notes: vec![crate::model::PianoNote {
                id: 100 + u64::from(id),
                channel_id: Some(project.channels[0].id),
                group_id: None,
                note: 60 + index as u8,
                start: 1.0,
                length: 0.5,
                velocity: 0.8,
                selected: false,
                muted: false,
            }],
        });
        let mut clip = template.clone();
        clip.id = id;
        clip.start = index as f32 * 4.0;
        clip.length = 4.0;
        clip.kind = crate::model::ClipKind::Pattern;
        clip.pattern_id = id;
        clip.source_offset = if index == 1 { 0.5 } else { 0.0 };
        clip.gain = 0.5 + index as f32 * 0.1;
        clip.muted = false;
        project.clips.push(clip);
    }
    let mut repeated = project.clips[0].clone();
    repeated.id = 6;
    repeated.start = 20.0;
    repeated.length = 8.0;
    project.clips.push(repeated);
    let mut muted = project.clips[1].clone();
    muted.id = 7;
    muted.start = 28.0;
    muted.muted = true;
    project.clips.push(muted);
    project.active_pattern = 4;
    project.automation_lanes = vec![
        master_song_lane(81, AutomationTarget::MasterVolume, 1.0, 0.0),
        master_song_lane(82, AutomationTarget::MasterPan, -0.4, 0.4),
        master_song_lane(
            83,
            AutomationTarget::MixerMute {
                track: crate::mixer_graph::MASTER_MIXER_TRACK_ID,
            },
            0.0,
            1.0,
        ),
    ];
    project
}

fn master_song_timeline(project: &crate::model::Project) -> crate::timeline::CompiledTimeline {
    let tempo_map = crate::tempo_map::TempoMap::from_project(project, 48_000).unwrap();
    crate::timeline::CompiledTimeline::from_project(
        project,
        &tempo_map,
        crate::timeline::TimelineCompileOptions::default(),
    )
    .unwrap()
}

#[test]
fn master_song_prepare_retains_five_patterns_master_fade_and_exact_original_fingerprint() {
    use crate::timeline::{CompiledAutomationTarget, NoteSourceDescriptor, TimelineEventKind};
    use std::hash::{Hash, Hasher};
    let project = master_song_project();
    let original = serde_json::to_vec(&project).unwrap();
    let expected = master_song_timeline(&project);
    assert!(expected.diagnostics().is_empty());
    let plan =
        InstrumentBouncePlan::prepare_master_song(&project, project.channels[0].id, 48_000, 7)
            .unwrap();
    assert!(plan.is_master_song());
    assert!(plan.occurrence_request().is_none());
    assert_eq!(plan.timeline(), &expected);
    assert_eq!(
        plan.timeline().channel_bases().len(),
        project.channels.len()
    );
    assert_eq!(plan.timeline().plugin_transport_tempo(), 120.0);
    let mut hasher = std::hash::DefaultHasher::new();
    original.hash(&mut hasher);
    assert_eq!(plan.project_fingerprint(), hasher.finish());
    assert_eq!(serde_json::to_vec(&project).unwrap(), original);
    let patterns = plan
        .timeline()
        .events()
        .iter()
        .filter_map(|event| match event.kind {
            TimelineEventKind::NoteOn { source, .. } => match source {
                NoteSourceDescriptor::ChannelStep { pattern_id, .. }
                | NoteSourceDescriptor::LegacyPiano { pattern_id, .. } => Some(pattern_id),
            },
            _ => None,
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(patterns, std::collections::BTreeSet::from([1, 2, 3, 4, 5]));
    assert!(plan.timeline().stats().muted_clips_skipped > 0);
    assert_eq!(
        plan.timeline().driven_automation_targets(),
        &[
            CompiledAutomationTarget::MasterVolume,
            CompiledAutomationTarget::MasterPan,
            CompiledAutomationTarget::MixerMute { track: 0 },
        ]
    );
    for variant in 0..3 {
        let mut changed = project.clone();
        match variant {
            0 => changed.swing = 0.0,
            1 => changed.clips[1].source_offset = 0.0,
            _ => changed.clips[6].muted = false,
        }
        assert_ne!(
            master_song_timeline(&changed).events(),
            plan.timeline().events()
        );
    }
}

#[test]
fn master_song_prepare_refuses_legacy_piano_sources_absent_from_timeline_and_fingerprint() {
    let original = master_song_project();
    let mut legacy = original.clone();
    legacy
        .piano_notes
        .push(original.patterns[0].notes[0].clone());
    assert_eq!(
        master_song_timeline(&legacy),
        master_song_timeline(&original)
    );
    assert_eq!(
        serde_json::to_vec(&legacy).unwrap(),
        serde_json::to_vec(&original).unwrap()
    );
    let reason = InstrumentBouncePlan::prepare_master_song(&legacy, 1, 48_000, 0)
        .err()
        .expect("unresolved legacy Piano Roll source must not be omitted");
    assert!(reason.contains("unresolved legacy Piano Roll sources"));
    assert_eq!(legacy.piano_notes.len(), 1);
}

#[test]
fn master_song_prepare_preserves_static_mix_and_accepts_stable_master_aliases() {
    use crate::{automation::AutomationTarget, mixer_graph::MASTER_MIXER_TRACK_ID};
    let mut project = master_song_project();
    project.channels[0].muted = true;
    project.channels[0].volume = 0.42;
    project.channels[0].pan = -0.25;
    project.channels[0].mixer_track = 3;
    project.mixer_tracks[3].volume = 0.63;
    project.mixer_tracks[3].pan = 0.2;
    project.mixer_tracks.swap(0, 9);
    project.automation_lanes[0] = master_song_lane(
        81,
        AutomationTarget::MixerVolume {
            track: MASTER_MIXER_TRACK_ID,
        },
        1.0,
        0.0,
    );
    project.automation_lanes[1] = master_song_lane(
        82,
        AutomationTarget::MixerPan {
            track: MASTER_MIXER_TRACK_ID,
        },
        -0.4,
        0.4,
    );
    let mut disabled_tempo = master_song_lane(84, AutomationTarget::Tempo, 120.0, 180.0);
    disabled_tempo.lane.set_enabled(false);
    project.automation_lanes.push(disabled_tempo);
    project
        .automation_lanes
        .push(crate::model::ProjectAutomation {
            id: 85,
            name: "Empty source control".into(),
            lane: crate::automation::AutomationLane::new(AutomationTarget::ChannelVolume {
                channel: project.channels[0].id,
            }),
        });
    let plan =
        InstrumentBouncePlan::prepare_master_song(&project, project.channels[0].id, 48_000, 0)
            .unwrap();
    assert_eq!(plan.timeline(), &master_song_timeline(&project));
    let source = plan
        .timeline()
        .channel_bases()
        .iter()
        .find(|base| base.channel_id == 1)
        .unwrap();
    assert!(source.muted);
    assert_eq!(source.volume, 0.42);
    assert_eq!(source.pan, -0.25);
    assert_eq!(source.mixer_track, 3);
    assert_eq!(
        plan.timeline()
            .mixer_graph()
            .runtime_slot_for_id(MASTER_MIXER_TRACK_ID),
        Some(0),
    );
}

#[test]
fn master_song_prepare_refuses_every_active_source_mixer_plugin_and_transport_control() {
    use crate::automation::AutomationTarget;
    let targets = [
        AutomationTarget::Tempo,
        AutomationTarget::Swing,
        AutomationTarget::ChannelVolume { channel: 1 },
        AutomationTarget::ChannelPan { channel: 1 },
        AutomationTarget::ChannelMute { channel: 1 },
        AutomationTarget::MixerVolume { track: 1 },
        AutomationTarget::MixerPan { track: 1 },
        AutomationTarget::MixerMute { track: 1 },
        AutomationTarget::PluginParameter {
            instance: 41,
            parameter: 17,
        },
        AutomationTarget::MixerVolume { track: 0 },
    ];
    for target in targets {
        let mut project = master_song_project();
        project
            .automation_lanes
            .push(master_song_lane(84, target.clone(), 0.0, 1.0));
        let original = serde_json::to_vec(&project).unwrap();
        let reason = InstrumentBouncePlan::prepare_master_song(&project, 1, 48_000, 0)
            .err()
            .expect("unsupported automation must fail");
        // A runtime slot is not a persistent mixer identity. Model validation
        // rejects this dangling reference before the automation policy runs.
        let expected = if target == (AutomationTarget::MixerVolume { track: 0 }) {
            "automation 84 references missing mixer track 0"
        } else {
            "active automation"
        };
        assert!(reason.contains(expected), "target {target:?}: {reason}");
        assert_eq!(serde_json::to_vec(&project).unwrap(), original);
    }
    for tempo in [f32::NAN, f32::INFINITY, 19.0, 400.1] {
        let mut project = master_song_project();
        project.tempo = tempo;
        let reason = InstrumentBouncePlan::prepare_master_song(&project, 1, 48_000, 0)
            .err()
            .expect("strict transport tempo must fail");
        assert!(reason.contains("finite fixed tempo"));
    }
}

#[test]
fn master_song_prepare_refuses_other_music_and_audio_even_when_static_mix_would_silence_it() {
    for source in 0..3 {
        let mut project = master_song_project();
        match source {
            0 => project.patterns[0].channel_steps[1][0] = true,
            1 => {
                let mut note = project.patterns[0].notes[0].clone();
                note.id = 500;
                note.channel_id = Some(project.channels[1].id);
                project.patterns[0].notes.push(note);
            }
            _ => {
                let mut audio = project.clips[0].clone();
                audio.id = 80;
                audio.kind = crate::model::ClipKind::Audio;
                audio.audio_asset_id = Some(70);
                audio.audio_source_offset_frame = Some(0);
                audio.length = 1.0;
                project.clips.push(audio);
                project.audio_assets.push(crate::model::AudioAsset {
                    id: 70,
                    name: "Metadata-only audio source".into(),
                    path: "never-opened.wav".into(),
                    sample_rate: 48_000,
                    channels: 2,
                    bits_per_sample: 32,
                    frames: 48_000,
                    waveform_peaks: vec![],
                });
                project.audio_clip_mixer_destinations.push(
                    crate::model::AudioClipMixerDestination {
                        clip_id: 80,
                        mixer_track_id: crate::mixer_graph::MASTER_MIXER_TRACK_ID,
                    },
                );
                project.mixer_tracks[0].muted = true;
            }
        }
        project.channels[1].muted = true;
        project.channels[0].solo = true;
        assert!(master_song_timeline(&project).diagnostics().is_empty());
        let reason = InstrumentBouncePlan::prepare_master_song(&project, 1, 48_000, 0)
            .err()
            .expect("another source must not be omitted");
        assert!(
            reason.contains(if source == 2 {
                "audio sources"
            } else {
                "musical channel source"
            }),
            "source {source}: {reason}"
        );
    }
}

#[test]
fn master_song_prepare_refuses_active_generators_effects_and_ambiguous_source_bindings() {
    for case in 0..6 {
        let mut project = master_song_project();
        let mut other = project.plugin_instances[0].clone();
        other.id = 42;
        match case {
            0 => {
                other.wet = 0.0;
                project.plugin_instances.push(other);
                project.channels[1].instrument_plugin_instance_id = Some(42);
            }
            1 => {
                other.role = crate::model::PluginRole::Effect;
                project.plugin_instances.push(other);
                project
                    .mixer_insert_slots
                    .push(crate::model::MixerInsertSlotRef {
                        track: 1,
                        slot: 0,
                        plugin_instance_id: 42,
                    });
            }
            2 => project.channels[1].instrument_plugin_instance_id = Some(41),
            3 => project
                .plugin_instances
                .push(project.plugin_instances[0].clone()),
            4 => project.plugin_instances[0].midi_ports.output = Some(1),
            _ => project.clips[0].pattern_id = u32::MAX,
        }
        let reason = InstrumentBouncePlan::prepare_master_song(&project, 1, 48_000, 0)
            .err()
            .expect("unsupported or ambiguous route must fail");
        let expected = match case {
            0 => "active generator",
            1 => "active mixer effects",
            // These malformed identities are rejected by the existing model
            // guard before Timeline compilation; no source may be omitted.
            2 => "plug-in 41 is placed more than once",
            3 => "plug-in identity 41 is duplicated",
            4 => "MIDI-port routing",
            _ => "Timeline diagnostics",
        };
        assert!(reason.contains(expected), "case {case}: {reason}");
    }
}

#[test]
fn master_song_prepare_retains_inactive_plugin_routes_without_promoting_them_to_sources() {
    for bypass in [false, true] {
        let mut project = master_song_project();
        let mut other = project.plugin_instances[0].clone();
        other.id = 42;
        other.enabled = bypass;
        other.bypass = bypass;
        project.plugin_instances.push(other);
        project.channels[1].instrument_plugin_instance_id = Some(42);
        let mut effect = project.plugin_instances[0].clone();
        effect.id = 43;
        effect.enabled = bypass;
        effect.bypass = bypass;
        effect.role = crate::model::PluginRole::Effect;
        project.plugin_instances.push(effect);
        project
            .mixer_insert_slots
            .push(crate::model::MixerInsertSlotRef {
                track: 1,
                slot: 0,
                plugin_instance_id: 43,
            });
        let plan = InstrumentBouncePlan::prepare_master_song(&project, 1, 48_000, 0).unwrap();
        assert_eq!(plan.timeline(), &master_song_timeline(&project));
        assert_eq!(plan.timeline().plugin_routes().len(), 3);
    }
}

#[test]
fn master_song_prepare_refuses_active_sidechains_and_aliases_without_stable_master() {
    use crate::{automation::AutomationTarget, mixer_graph::MASTER_MIXER_TRACK_ID};
    for case in 0..2 {
        let mut project = master_song_project();
        if case == 0 {
            project.mixer_routes[0].destination =
                crate::mixer_graph::MixerRouteDestination::PluginSidechain {
                    mixer_track_id: MASTER_MIXER_TRACK_ID,
                    slot: 0,
                    input_bus: 1,
                };
        } else {
            project.mixer_tracks[0].id = 900;
            project.automation_lanes[0] = master_song_lane(
                81,
                AutomationTarget::MixerVolume {
                    track: MASTER_MIXER_TRACK_ID,
                },
                1.0,
                0.0,
            );
        }
        assert!(InstrumentBouncePlan::prepare_master_song(&project, 1, 48_000, 0).is_err());
    }
}

fn master_song_occurrence_request(
    project: &crate::model::Project,
) -> Arc<crate::occurrence_request::OccurrenceRenderRequest> {
    use crate::{
        occurrence_request::{FixedTailAcknowledgment, OccurrenceRenderRequest, OccurrenceStamp},
        timeline::{NoteSourceDescriptor, TimelineEventKind},
    };
    let timeline = master_song_timeline(project);
    let local_events = timeline
        .events()
        .iter()
        .copied()
        .filter(|event| {
            let source = match event.kind {
                TimelineEventKind::NoteOn { source, .. }
                | TimelineEventKind::NoteOff { source, .. } => source,
                _ => return false,
            };
            match source {
                NoteSourceDescriptor::ChannelStep { clip_id, .. }
                | NoteSourceDescriptor::LegacyPiano { clip_id, .. } => {
                    clip_id == project.clips[0].id
                }
            }
        })
        .collect::<Vec<_>>();
    let snapshot: Arc<[u8]> = serde_json::to_vec(project).unwrap().into();
    let stamp = OccurrenceStamp {
        project_session: 2,
        project_revision: 3,
        timeline_revision: 4,
        epoch: 7,
        engine_session: 5,
        endpoint_id: 19,
        channel_id: 1,
        instance_id: 41,
        mixer_track_id: project.channels[0].mixer_track,
        native_generation: 2,
        app_native_dirty_revision: 1,
        native_capture_serial: 0,
        parameter_capture_serial: 0,
        native_control_request: 10,
        parameter_control_request: 11,
    };
    let consent = Arc::new(FixedTailAcknowledgment::review(snapshot, stamp, 1, 48_000, 0).unwrap());
    let end = crate::tempo_map::TempoMap::from_project(project, 48_000)
        .unwrap()
        .beat_to_frame(4.0)
        .unwrap();
    OccurrenceRenderRequest::new(
        consent,
        1,
        1,
        41,
        0,
        end,
        timeline.duration_frames(),
        120.0,
        local_events.into(),
    )
    .unwrap()
}

#[test]
fn master_song_admission_leaves_legacy_and_occurrence_automation_rejection_unchanged() {
    let mut project = master_song_project();
    let request = master_song_occurrence_request(&project);
    for reason in [
        InstrumentBouncePlan::prepare(&project, 1, 48_000, 0)
            .err()
            .unwrap(),
        InstrumentBouncePlan::prepare_occurrence(&project, request)
            .err()
            .unwrap(),
    ] {
        assert!(reason.contains("does not support active automation"));
    }
    project.automation_lanes.clear();
    assert!(
        !InstrumentBouncePlan::prepare(&project, 1, 48_000, 0)
            .unwrap()
            .is_master_song()
    );
    assert!(
        !InstrumentBouncePlan::prepare_occurrence(
            &project,
            master_song_occurrence_request(&project)
        )
        .unwrap()
        .is_master_song()
    );
}

#[cfg(target_os = "linux")]
#[path = "instrument_bounce_occurrence_tests.rs"]
mod occurrence_join_tests;
