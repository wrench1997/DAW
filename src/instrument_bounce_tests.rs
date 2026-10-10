//! Plugin-free unit fixtures for the dormant bounce checkpoint. No vendor module is loaded.

use super::*;
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
            output_buses: vec![(2, true)],
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
    ) -> Result<BouncePluginState, BounceCaptureError> {
        assert_eq!(ids, &[17]);
        self.captures.fetch_add(1, Ordering::SeqCst);
        if let Some(epoch) = &self.changed_epoch {
            epoch.store(8, Ordering::Release);
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
        })),
        config: SlotConfig::default(),
        fault: None,
        parameter_catalog_cache: None,
        native_base_ids: vec![17],
    }
}

#[test]
fn bounce_worker_capture_has_no_normal_state_or_native_publication() {
    let captures = Arc::new(AtomicU64::new(0));
    let mut slots = vec![slot(Ok(state()), Arc::clone(&captures), None)];
    let metrics = BridgeMetrics::default();
    let before = metrics.native_editors.lock().unwrap()[0].clone();
    let (mut events, mut rx) = RingBuffer::new(4);
    let receipt = capture_instrument_bounce_on_worker(
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
fn bounce_capture_admission_is_bounded_and_cancel_releases_it() {
    let (control, commands) = control(1);
    let receipt = control
        .request_instrument_bounce_state(41, 7, vec![17])
        .unwrap();
    assert!(
        control
            .request_instrument_bounce_state(41, 7, vec![17])
            .is_err()
    );
    drop(receipt);
    assert!(!control.metrics.bounce_capture_busy.load(Ordering::Acquire));
    // The abandoned command still fills the bounded queue; never accumulate render/capture jobs.
    assert!(
        control
            .request_instrument_bounce_state(41, 7, vec![17])
            .is_err()
    );
    let AdminCommand::CaptureInstrumentBounce { cancelled, .. } = commands.try_recv().unwrap()
    else {
        panic!("wrong command")
    };
    assert!(cancelled.load(Ordering::Acquire));
    let next = control
        .request_instrument_bounce_state(41, 7, vec![17])
        .unwrap();
    drop(next);
    drop(commands);
    assert!(
        control
            .request_instrument_bounce_state(41, 7, vec![17])
            .is_err()
    );
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
    let AdminCommand::CaptureInstrumentBounce { reply, .. } = commands.try_recv().unwrap() else {
        panic!("wrong command")
    };
    reply
        .send(Ok(BounceStateReceipt {
            epoch: 7,
            state: state(),
        }))
        .unwrap();
    let captured = capture.poll(stamp, status(), now).unwrap().unwrap();
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
fn bounce_coordinator_rejects_stale_session_epoch_timeout_cancel_and_worker_disconnect() {
    for case in 0..6 {
        let project = project();
        let plan =
            InstrumentBouncePlan::prepare(&project, project.channels[0].id, 48_000, 0).unwrap();
        let stamp = identity(&plan);
        let (control, commands) = control(1);
        let now = Instant::now();
        let mut capture =
            InstrumentBounceCapture::begin(plan, stamp, status(), &control, now).unwrap();
        let AdminCommand::CaptureInstrumentBounce { reply, .. } = commands.try_recv().unwrap()
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
            capture.poll(observed, status(), polled).is_err(),
            "case {case}"
        );
        assert!(!control.metrics.bounce_capture_busy.load(Ordering::Acquire));
    }
}

#[test]
fn bounce_metadata_and_state_policy_rejects_unsupported_inputs_and_empty_state() {
    state().validate(&[17], 48_000).unwrap();
    for case in 0..8 {
        let mut state = state();
        match case {
            0 => state.opaque_state.clear(),
            1 => state.metadata.input_buses.push((2, false)),
            2 => state.metadata.output_buses.push((2, false)),
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
