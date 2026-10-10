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

fn control(capacity: usize) -> (PluginChainControl, Receiver<AdminCommand>) {
    let (admin, rx) = mpsc::sync_channel(capacity);
    let (_, events) = RingBuffer::new(4);
    let (_, receipts) = RingBuffer::new(4);
    (
        PluginChainControl {
            admin,
            events,
            metrics: Arc::new(BridgeMetrics::default()),
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
    let AdminCommand::CaptureInstrumentBounce { lease, reply, .. } = commands.try_recv().unwrap()
    else {
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
    assert!(matches!(commands.try_recv(), Ok(AdminCommand::Shutdown)));
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
    let AdminCommand::CaptureInstrumentBounce { lease, .. } = commands.try_recv().unwrap() else {
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
    let AdminCommand::CaptureInstrumentBounce { lease, reply, .. } = commands.try_recv().unwrap()
    else {
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
    let AdminCommand::CaptureInstrumentBounce { reply, lease, .. } = commands.try_recv().unwrap()
    else {
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
        let AdminCommand::CaptureInstrumentBounce { reply, lease, .. } =
            commands.try_recv().unwrap()
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
    let AdminCommand::CaptureInstrumentBounce { reply, lease, .. } = commands.try_recv().unwrap()
    else {
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
                .push(BounceAudioBus::new(2, false, Some(false))),
            2 => state
                .metadata
                .output_buses
                .push(BounceAudioBus::new(2, true, Some(false))),
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
fn bounce_audio_policy_keeps_every_declared_bus_and_known_activation() {
    for input_channels in [0, 1, 2] {
        for input_active in [false, true] {
            for output_channels in [1, 2] {
                for auxiliaries in 0..=2 {
                    let mut metadata = state().metadata;
                    metadata.input_buses = if input_channels == 0 {
                        vec![]
                    } else {
                        vec![BounceAudioBus::new(
                            input_channels,
                            true,
                            Some(input_active),
                        )]
                    };
                    metadata.output_buses =
                        vec![BounceAudioBus::new(output_channels, true, Some(true))];
                    metadata.output_buses.extend(
                        (0..auxiliaries)
                            .map(|index| BounceAudioBus::new(index + 1, false, Some(false))),
                    );
                    let exact = metadata.clone();
                    metadata.validate(48_000).unwrap();
                    assert_eq!(metadata, exact);
                    assert_eq!(metadata.output_buses.len(), 1 + auxiliaries);
                    assert_eq!(
                        metadata.participating_input_channels(),
                        if input_active { input_channels } else { 0 }
                    );
                }
            }
        }
    }
}

#[test]
fn bounce_audio_policy_refuses_every_out_of_bound_raw_descriptor_without_projection() {
    let mut valid = state().metadata;
    valid.input_buses = vec![BounceAudioBus::new(2, true, Some(true))];
    valid.output_buses.extend([
        BounceAudioBus::new(1, false, Some(false)),
        BounceAudioBus::new(2, false, Some(false)),
    ]);
    for case in 0..19 {
        let mut metadata = valid.clone();
        match case {
            0 => metadata.input_buses.push(metadata.input_buses[0]),
            1 => metadata.input_buses[0].is_main = false,
            2 => metadata.input_buses[0].channel_count = 0,
            3 => metadata.input_buses[0].channel_count = 3,
            4 => metadata.input_buses[0].active = None,
            5 => metadata.output_buses.clear(),
            6 => metadata
                .output_buses
                .push(BounceAudioBus::new(2, false, Some(false))),
            7 => metadata.output_buses[0].is_main = false,
            8 => metadata.output_buses[0].active = Some(false),
            9 => metadata.output_buses[0].active = None,
            10 => metadata.output_buses[0].channel_count = 0,
            11 => metadata.output_buses[0].channel_count = 3,
            12 => metadata.output_buses[1].is_main = true,
            13 => metadata.output_buses[2].is_main = true,
            14 => metadata.output_buses[1].active = Some(true),
            15 => metadata.output_buses[2].active = None,
            16 => metadata.output_buses[2].channel_count = 0,
            17 => metadata.output_buses[2].channel_count = 3,
            _ => metadata.output_buses.swap(0, 2),
        }
        let exact = metadata.clone();
        assert!(metadata.validate(48_000).is_err(), "case {case}");
        assert_eq!(metadata, exact, "admission must be observational");
    }
    let mut changed = valid.clone();
    changed.output_buses[1].channel_count = 2;
    changed.validate(48_000).unwrap();
    assert_ne!(
        changed, valid,
        "even supported inactive-bus drift changes the receipt"
    );
    changed = valid.clone();
    changed.output_buses.swap(1, 2);
    changed.validate(48_000).unwrap();
    assert_ne!(changed, valid, "inactive order survives metadata equality");
}
