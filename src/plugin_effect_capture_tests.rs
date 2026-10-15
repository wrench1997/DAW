//! Real worker/queue proofs for the private stopped Effect source path.

use super::*;
use crate::instrument_bounce::{
    BOUNCE_PREPARED_MAXIMUM, BounceAudioBus,
    effect::{EffectSourceIdentity, EffectSourceMetadata},
};
use std::{collections::BTreeMap, mem::ManuallyDrop};

const WAIT: Duration = Duration::from_secs(5);
const EPOCH: u64 = INITIAL_TRANSPORT_EPOCH + 1;
static CLEANUP_FAILED: AtomicBool = AtomicBool::new(false);
thread_local! {
    static QUARANTINE: std::cell::RefCell<Vec<ManuallyDrop<PluginChain>>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

struct CaptureProbe {
    instance_id: u64,
    marker: u8,
    entered: AtomicBool,
    held: AtomicBool,
    release: AtomicBool,
    revision: AtomicU64,
    parameter_bits: AtomicU64,
    latency: AtomicU32,
    failure: AtomicU32,
    drift_target: Mutex<Option<Arc<CaptureProbe>>>,
    captures: AtomicU64,
    verifies: AtomicU64,
    prepared: AtomicU64,
    resets: AtomicU64,
    processes: AtomicU64,
    saves: AtomicU64,
    loads: AtomicU64,
    closes: AtomicU64,
    midi: AtomicU64,
}

impl CaptureProbe {
    fn new(instance_id: u64, marker: u8) -> Self {
        Self {
            instance_id,
            marker,
            entered: AtomicBool::new(false),
            held: AtomicBool::new(false),
            release: AtomicBool::new(false),
            revision: AtomicU64::new(1),
            parameter_bits: AtomicU64::new(
                (0.123_456_789_012_345 + f64::from(marker) / 100.0).to_bits(),
            ),
            latency: AtomicU32::new(0),
            failure: AtomicU32::new(0),
            drift_target: Mutex::new(None),
            captures: AtomicU64::new(0),
            verifies: AtomicU64::new(0),
            prepared: AtomicU64::new(0),
            resets: AtomicU64::new(0),
            processes: AtomicU64::new(0),
            saves: AtomicU64::new(0),
            loads: AtomicU64::new(0),
            closes: AtomicU64::new(0),
            midi: AtomicU64::new(0),
        }
    }

    fn request(&self, config: SlotConfig) -> EffectCaptureSlotRequest {
        EffectCaptureSlotRequest {
            instance_id: self.instance_id,
            config,
            path: PathBuf::from(format!("/preloaded/effect{}.vst3", self.instance_id)),
            // Match the App's actual default-load provenance: stored UID is not a selector.
            requested_class_uid: None,
            parameter_ids: vec![3],
        }
    }

    fn state(&self) -> EffectPluginState {
        let base = f64::from_bits(self.parameter_bits.load(Ordering::Acquire));
        let request = self.request(SlotConfig::default());
        let mut metadata = EffectSourceMetadata {
            identity: EffectSourceIdentity {
                class_uid: format!("{:032X}", self.instance_id),
                vendor: "Worker fixture".into(),
                name: format!("Effect {}", self.instance_id),
                version: "1.0".into(),
                category: "Fx|Delay".into(),
            },
            path: request.path,
            requested_class_uid: request.requested_class_uid,
            sample_rate: 48_000.0,
            max_block_frames: BOUNCE_PREPARED_MAXIMUM,
            tempo: 128.0,
            time_sig_numerator: 4,
            time_sig_denominator: 4,
            input_buses: vec![BounceAudioBus::new(2, true, Some(true))],
            output_buses: vec![BounceAudioBus::new(2, true, Some(true))],
            event_input_buses: Vec::new(),
            event_output_buses: Vec::new(),
            latency_samples: self.latency.load(Ordering::Acquire),
            tail_samples: u32::from(self.marker),
            async_started_revision: self.revision.load(Ordering::Acquire),
        };
        match self.failure.load(Ordering::Acquire) {
            1 => metadata.identity.category = "Instrument|Synth".into(),
            2 => metadata
                .input_buses
                .push(BounceAudioBus::new(2, false, Some(false))),
            _ => {}
        }
        EffectPluginState {
            metadata,
            opaque_state: vec![self.marker, 0xE1, 0xFF],
            catalog_revision: 17,
            catalog: vec![
                PluginParameterDescriptor {
                    id: 3,
                    name: "Mix".into(),
                    unit: "%".into(),
                    current_normalized: base as f32,
                    default_normalized: Some(0.5),
                    step_count: Some(0),
                    automatable: true,
                    read_only: false,
                    bypass: false,
                },
                PluginParameterDescriptor {
                    id: 8,
                    name: "Meter".into(),
                    unit: String::new(),
                    current_normalized: 0.0,
                    default_normalized: None,
                    step_count: None,
                    automatable: false,
                    read_only: true,
                    bypass: false,
                },
            ],
            parameter_bases: BTreeMap::from([(3, base)]),
            native_generation: 11,
            native_dirty_revision: self.revision.load(Ordering::Acquire),
        }
    }

    fn activity(&self) -> [u64; 6] {
        [
            &self.resets,
            &self.processes,
            &self.saves,
            &self.loads,
            &self.closes,
            &self.midi,
        ]
        .map(|counter| counter.load(Ordering::Acquire))
    }
}

#[derive(Clone)]
pub(crate) struct EffectCaptureHandle {
    probes: Vec<Arc<CaptureProbe>>,
    configs: Vec<SlotConfig>,
}

impl EffectCaptureHandle {
    pub(crate) fn requests(&self) -> Vec<EffectCaptureSlotRequest> {
        self.probes
            .iter()
            .zip(&self.configs)
            .map(|(probe, config)| probe.request(*config))
            .collect()
    }
    pub(crate) fn state_for_slot(&self, slot: usize) -> EffectPluginState {
        self.probes[slot].state()
    }
    pub(crate) fn hold_slot(&self, slot: usize) {
        let probe = &self.probes[slot];
        probe.entered.store(false, Ordering::Release);
        probe.release.store(false, Ordering::Release);
        probe.held.store(true, Ordering::Release);
    }
    pub(crate) fn wait_entered(&self, slot: usize) {
        wait_until(|| self.probes[slot].entered.load(Ordering::Acquire));
    }
    pub(crate) fn release_slot(&self, slot: usize) {
        self.probes[slot].release.store(true, Ordering::Release);
    }
    pub(crate) fn release_all(&self) {
        for probe in &self.probes {
            probe.release.store(true, Ordering::Release);
        }
    }
    pub(crate) fn capture_count(&self, slot: usize) -> u64 {
        self.probes[slot].captures.load(Ordering::Acquire)
    }
    pub(crate) fn set_revision(&self, slot: usize, revision: u64) {
        self.probes[slot]
            .revision
            .store(revision, Ordering::Release);
    }
    pub(crate) fn set_parameter_base(&self, slot: usize, base: f64) {
        self.probes[slot]
            .parameter_bits
            .store(base.to_bits(), Ordering::Release);
    }
    pub(crate) fn set_latency(&self, slot: usize, latency: u32) {
        self.probes[slot].latency.store(latency, Ordering::Release);
    }
}

struct CaptureBackend(Arc<CaptureProbe>);

impl PluginBackend for CaptureBackend {
    fn name(&self) -> &str {
        "private Effect capture worker fixture"
    }
    fn prepare(&mut self, _: PluginPrepareConfig) -> Result<(), String> {
        self.0.prepared.fetch_add(1, Ordering::Release);
        Ok(())
    }
    fn process(&mut self, _: &mut [f32], _: &mut [f32], frames: usize) -> Result<(), String> {
        if frames > 0 {
            self.0.processes.fetch_add(1, Ordering::Release);
        }
        Ok(())
    }
    fn send_midi(&mut self, _: MidiMessage) -> Result<(), String> {
        self.0.midi.fetch_add(1, Ordering::Release);
        Ok(())
    }
    fn set_parameter(&mut self, _: u32, value: f32) -> Result<(), String> {
        self.0
            .parameter_bits
            .store(f64::from(value).to_bits(), Ordering::Release);
        Ok(())
    }
    fn get_parameter(&mut self, _: u32) -> Result<f32, String> {
        Ok(f64::from_bits(self.0.parameter_bits.load(Ordering::Acquire)) as f32)
    }
    fn save_state(&mut self) -> Result<Vec<u8>, String> {
        self.0.saves.fetch_add(1, Ordering::Release);
        Ok(vec![0x51])
    }
    fn load_state(&mut self, _: &[u8]) -> Result<(), String> {
        self.0.loads.fetch_add(1, Ordering::Release);
        Ok(())
    }
    fn native_editor(&mut self, command: NativeEditorCommand) -> Result<NativeEditorState, String> {
        if command == NativeEditorCommand::Close {
            self.0.closes.fetch_add(1, Ordering::Release);
        }
        Ok(NativeEditorState {
            supported: true,
            has_editor: true,
            open: command != NativeEditorCommand::Close,
            generation: 11,
            ..NativeEditorState::default()
        })
    }
    fn reset_processing(&mut self) -> Result<(), String> {
        self.0.resets.fetch_add(1, Ordering::Release);
        Ok(())
    }
    fn latency_samples(&self) -> u32 {
        self.0.latency.load(Ordering::Acquire)
    }
    fn tail_samples(&self) -> u32 {
        u32::from(self.0.marker)
    }
    fn capture_effect_bounce_state(
        &mut self,
        ids: &[u32],
        fence: &BounceCaptureFence,
    ) -> Result<EffectPluginState, BounceCaptureError> {
        assert_eq!(ids, &[3]);
        self.0.captures.fetch_add(1, Ordering::Release);
        self.0.entered.store(true, Ordering::Release);
        // Deliberately model an already-dispatched call: cancellation/deadline does not make
        // it terminate. The bounded release must return before its real checkpoint/ACK.
        let deadline = Instant::now() + WAIT;
        while self.0.held.load(Ordering::Acquire) && !self.0.release.load(Ordering::Acquire) {
            if Instant::now() >= deadline {
                return Err(BounceCaptureError::BackendFailed(
                    "fixture hold expired".into(),
                ));
            }
            thread::sleep(Duration::from_millis(1));
        }
        if self.0.failure.load(Ordering::Acquire) == 3 {
            return Err(BounceCaptureError::BackendFailed(
                "actual capture transport failure".into(),
            ));
        }
        fence.checkpoint()?;
        if let Some(target) = self.0.drift_target.lock().unwrap().take() {
            target.revision.fetch_add(1, Ordering::Release);
        }
        Ok(self.0.state())
    }
    fn verify_effect_bounce_state(
        &mut self,
        state: &EffectPluginState,
        fence: &BounceCaptureFence,
    ) -> Result<(), BounceCaptureError> {
        self.0.verifies.fetch_add(1, Ordering::Release);
        fence.checkpoint()?;
        let current = self.0.state();
        if state != &current {
            return Err(BounceCaptureError::Rejected(
                "source revision/parameter/metadata drifted".into(),
            ));
        }
        Ok(())
    }
}

fn identified_effect_chain(
    configs: Vec<SlotConfig>,
) -> Result<(PluginChain, EffectCaptureHandle), String> {
    let ids: Vec<_> = [91, 7, 42, 83].into_iter().take(configs.len()).collect();
    let probes: Vec<_> = ids
        .iter()
        .enumerate()
        .map(|(slot, id)| Arc::new(CaptureProbe::new(*id, slot as u8 + 1)))
        .collect();
    let worker_probes = probes.clone();
    let worker_configs = configs.clone();
    let chain = PluginChain::spawn_identified_with_backend_factory(
        &ids,
        move || {
            worker_probes
                .into_iter()
                .zip(worker_configs)
                .map(|(probe, config)| {
                    let mut slot = BackendSlot::new(Box::new(CaptureBackend(probe)));
                    slot.config = config;
                    slot
                })
                .collect()
        },
        PluginPrepareConfig {
            sample_rate: 48_000.0,
            max_block_frames: BOUNCE_PREPARED_MAXIMUM,
        },
    )?;
    Ok((chain, EffectCaptureHandle { probes, configs }))
}

/// App tests own retained shutdown of this returned real worker; releasing the handle is not ACK.
pub(crate) fn identified_two_slot_effect_chain()
-> Result<(PluginChain, EffectCaptureHandle), String> {
    identified_effect_chain(vec![SlotConfig::default(); 2])
}

struct Fixture {
    chain: Option<PluginChain>,
    handle: EffectCaptureHandle,
    cleanup: Option<RetainedShutdownPoll>,
}

impl Fixture {
    fn new(configs: Vec<SlotConfig>) -> Self {
        assert!(
            !CLEANUP_FAILED.load(Ordering::Acquire),
            "previous exact worker remains quarantined"
        );
        let (chain, handle) = identified_effect_chain(configs).unwrap();
        let mut fixture = Self {
            chain: Some(chain),
            handle,
            cleanup: None,
        };
        wait_until(|| fixture.chain().control.plugin_endpoint_snapshot().is_some());
        assert!(fixture.chain_mut().audio.set_epoch(EPOCH));
        wait_until(|| fixture.settled());
        // Use the real callback endpoint/input queue, and deliberately retain its output.
        assert!(matches!(
            fixture
                .chain_mut()
                .audio
                .try_submit(&[0.0; 128], &[0.0; 128]),
            SubmitStatus::Submitted { .. }
        ));
        wait_until(|| fixture.settled());
        while fixture.chain_mut().control.try_next_event().is_some() {}
        fixture
    }
    fn chain(&self) -> &PluginChain {
        self.chain.as_ref().unwrap()
    }
    fn chain_mut(&mut self) -> &mut PluginChain {
        self.chain.as_mut().unwrap()
    }
    fn settled(&self) -> bool {
        self.settled_at(EPOCH)
    }
    fn settled_at(&self, epoch: u64) -> bool {
        let metrics = &self.chain().control.metrics;
        metrics
            .worker_settlement
            .reset_completed_epoch
            .load(Ordering::Acquire)
            == epoch
            && metrics.worker_settlement.terminal.load(Ordering::Acquire)
                == metrics.submitted.load(Ordering::Acquire)
            && metrics.worker_settlement.busy.load(Ordering::Acquire) == 0
            && !self.chain().control.has_outstanding_admin_work()
    }
    fn request_until(&self, deadline: Instant) -> effect_capture::EffectChainStateReceiver {
        self.chain()
            .control
            .request_effect_chain_state(
                EPOCH,
                self.chain().control.plugin_endpoint_snapshot().unwrap(),
                self.handle.requests(),
                deadline,
            )
            .unwrap()
    }
    fn request(&self) -> effect_capture::EffectChainStateReceiver {
        self.request_until(Instant::now() + WAIT)
    }
    fn finish(&mut self) -> RetainedShutdownPoll {
        if let Some(outcome) = self.cleanup {
            return outcome;
        }
        self.handle.release_all();
        let deadline = Instant::now() + WAIT;
        let outcome = loop {
            let outcome = self.chain_mut().guard.poll_shutdown_retained();
            if outcome != RetainedShutdownPoll::Pending || Instant::now() >= deadline {
                break outcome;
            }
            thread::sleep(Duration::from_millis(1));
        };
        self.cleanup = Some(outcome);
        if outcome != RetainedShutdownPoll::Joined {
            CLEANUP_FAILED.store(true, Ordering::Release);
            QUARANTINE.with(|slot| {
                slot.borrow_mut()
                    .push(ManuallyDrop::new(self.chain.take().unwrap()))
            });
        }
        outcome
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let outcome = self.finish();
        if outcome != RetainedShutdownPoll::Joined && !thread::panicking() {
            panic!("Effect fixture cleanup failed: {outcome:?}; exact owner quarantined");
        }
    }
}

fn wait_until(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !predicate() {
        assert!(
            Instant::now() < deadline,
            "real Effect worker did not settle"
        );
        thread::sleep(Duration::from_millis(1));
    }
}

fn receive(
    receiver: &effect_capture::EffectChainStateReceiver,
) -> Result<EffectChainStateReceipt, BounceCaptureError> {
    let deadline = Instant::now() + WAIT;
    loop {
        match receiver.try_recv() {
            Ok(result) => {
                assert!(receiver.termination_acknowledged());
                return result;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => panic!("Effect worker lost its private reply"),
        }
        assert!(
            Instant::now() < deadline,
            "Effect capture did not terminate"
        );
        thread::sleep(Duration::from_millis(1));
    }
}

fn rejection(result: Result<EffectChainStateReceipt, BounceCaptureError>) -> String {
    match result {
        Err(BounceCaptureError::Rejected(message)) => message,
        other => panic!("expected healthy Effect refusal, got {other:?}"),
    }
}

#[test]
fn private_effect_capture_preserves_order_inactive_configs_and_exact_bases_without_save_or_dsp() {
    let configs = vec![
        SlotConfig {
            wet: 0.35,
            ..SlotConfig::default()
        },
        SlotConfig {
            enabled: false,
            wet: 0.75,
            ..SlotConfig::default()
        },
        SlotConfig {
            bypassed: true,
            wet: 0.625,
            ..SlotConfig::default()
        },
    ];
    let fixture = Fixture::new(configs);
    let before: Vec<_> = fixture
        .handle
        .probes
        .iter()
        .map(|probe| probe.activity())
        .collect();
    assert_eq!(
        before[0][0], 1,
        "bootstrap must perform a real worker epoch reset"
    );
    assert!(
        before[0][1] > 0,
        "bootstrap must process the submitted input"
    );
    assert!(before.iter().all(|activity| activity[2..5] == [0, 0, 0]));
    let receiver = fixture.request();
    let receipt = receive(&receiver).unwrap();
    wait_until(|| {
        fixture
            .chain()
            .control
            .effect_chain_receipt_current(&receipt)
    });
    assert_eq!(receipt.epoch, EPOCH);
    assert_eq!(
        receipt
            .slots
            .iter()
            .map(|slot| slot.request.clone())
            .collect::<Vec<_>>(),
        fixture.handle.requests()
    );
    assert_eq!(
        receipt
            .slots
            .iter()
            .map(|slot| slot.request.instance_id)
            .collect::<Vec<_>>(),
        vec![91, 7, 42]
    );
    for (slot, captured) in receipt.slots.iter().enumerate() {
        assert_eq!(captured.state, fixture.handle.state_for_slot(slot));
        assert_ne!(
            captured.state.parameter_bases[&3],
            f64::from(captured.state.catalog[0].current_normalized)
        );
        assert_eq!(fixture.handle.capture_count(slot), 1);
        assert_eq!(
            fixture.handle.probes[slot].verifies.load(Ordering::Acquire),
            1
        );
        assert_eq!(
            fixture.handle.probes[slot].prepared.load(Ordering::Acquire),
            1
        );
        assert_eq!(fixture.handle.probes[slot].activity(), before[slot]);
        let native = fixture
            .chain()
            .control
            .native_editor_snapshot(slot)
            .unwrap();
        assert_eq!(
            (native.capture_serial, native.parameter_capture_serial),
            (0, 0)
        );
        assert!(native.captured_state.is_none());
        assert!(native.captured_parameters.is_none());
        assert!(!native.state.open);
    }
    assert_eq!(fixture.chain().control.stats().faults, 0);
}

#[test]
fn cancelled_or_dropped_held_effect_capture_retains_worker_lease_until_actual_return() {
    for drop_receiver in [false, true] {
        let fixture = Fixture::new(vec![SlotConfig::default()]);
        fixture.handle.hold_slot(0);
        let mut receiver = Some(fixture.request());
        fixture.handle.wait_entered(0);
        if drop_receiver {
            drop(receiver.take());
        } else {
            receiver.as_ref().unwrap().cancel();
        }
        assert!(
            fixture
                .chain()
                .control
                .metrics
                .bounce_capture_busy
                .load(Ordering::Acquire)
        );
        assert!(fixture.chain().control.has_outstanding_admin_work());
        if let Some(receiver) = &receiver {
            assert!(!receiver.termination_acknowledged());
            assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));
        }
        assert!(
            fixture
                .chain()
                .control
                .request_effect_chain_state(
                    EPOCH,
                    fixture.chain().control.plugin_endpoint_snapshot().unwrap(),
                    fixture.handle.requests(),
                    Instant::now() + WAIT
                )
                .is_err()
        );
        fixture.handle.release_all();
        if let Some(receiver) = &receiver {
            assert!(rejection(receive(receiver)).contains("cancelled"));
        }
        wait_until(|| {
            !fixture
                .chain()
                .control
                .metrics
                .bounce_capture_busy
                .load(Ordering::Acquire)
                && !fixture.chain().control.has_outstanding_admin_work()
        });
        assert_eq!(fixture.handle.probes[0].verifies.load(Ordering::Acquire), 0);
        let retry = fixture.request();
        assert!(receive(&retry).is_ok());
        assert_eq!(fixture.handle.capture_count(0), 2);
        assert_eq!(fixture.chain().control.stats().faults, 0);
    }
}

#[derive(Clone, Copy, Debug)]
enum LateWork {
    Input,
    Admin,
    Parameter,
    Epoch,
    EpochAba,
    Editor,
}

fn admit_late_work(fixture: &mut Fixture, work: LateWork) {
    match work {
        LateWork::Input => assert!(matches!(
            fixture
                .chain_mut()
                .audio
                .try_submit(&[0.0; 128], &[0.0; 128]),
            SubmitStatus::Submitted { .. }
        )),
        LateWork::Admin => assert!(fixture.chain().control.query_parameter(0, 3)),
        LateWork::Parameter => assert!(fixture.chain_mut().audio.try_set_parameter_tagged(
            0,
            3,
            0.75,
            ParameterEditId::new(700).unwrap()
        )),
        LateWork::Epoch => assert!(fixture.chain_mut().audio.set_epoch(EPOCH + 1)),
        LateWork::EpochAba => {
            assert!(fixture.chain_mut().audio.set_epoch(EPOCH + 1));
            assert!(fixture.chain_mut().audio.set_epoch(EPOCH));
        }
        LateWork::Editor => assert!(fixture.chain().control.request_native_editor(
            0,
            701,
            NativeEditorCommand::Open { owner: None },
            &[3]
        )),
    }
}

#[test]
fn effect_capture_rejects_new_input_admin_parameter_epoch_or_editor_intent_while_held() {
    for work in [
        LateWork::Input,
        LateWork::Admin,
        LateWork::Parameter,
        LateWork::Epoch,
        LateWork::EpochAba,
        LateWork::Editor,
    ] {
        let mut fixture = Fixture::new(vec![SlotConfig::default()]);
        fixture.handle.hold_slot(0);
        let receiver = fixture.request();
        fixture.handle.wait_entered(0);
        admit_late_work(&mut fixture, work);
        fixture.handle.release_all();
        let message = rejection(receive(&receiver));
        assert!(message.contains("changed"), "{work:?}: {message}");
        assert_eq!(fixture.handle.probes[0].verifies.load(Ordering::Acquire), 0);
        assert_eq!(fixture.chain().control.stats().faults, 0, "{work:?}");
    }
}

#[test]
fn effect_receipt_remembers_later_input_and_drained_admin_not_just_current_busy_flags() {
    for work in [
        LateWork::Input,
        LateWork::Admin,
        LateWork::Parameter,
        LateWork::Epoch,
        LateWork::EpochAba,
        LateWork::Editor,
    ] {
        let mut fixture = Fixture::new(vec![SlotConfig::default()]);
        let receiver = fixture.request();
        let receipt = receive(&receiver).unwrap();
        wait_until(|| {
            fixture
                .chain()
                .control
                .effect_chain_receipt_current(&receipt)
        });
        admit_late_work(&mut fixture, work);
        assert!(
            !fixture
                .chain()
                .control
                .effect_chain_receipt_current(&receipt),
            "{work:?}"
        );
        if matches!(work, LateWork::Input | LateWork::Admin) {
            wait_until(|| fixture.settled());
            assert!(
                !fixture
                    .chain()
                    .control
                    .effect_chain_receipt_current(&receipt),
                "drained {work:?}"
            );
        }
    }
}

#[test]
fn effect_receipt_rejects_fully_settled_epoch_aba_and_new_capture_still_succeeds() {
    let mut fixture = Fixture::new(vec![SlotConfig::default()]);
    let receiver = fixture.request();
    let receipt = receive(&receiver).unwrap();
    wait_until(|| {
        fixture
            .chain()
            .control
            .effect_chain_receipt_current(&receipt)
    });
    let endpoint = receipt.endpoint;
    let state = receipt.slots[0].state.clone();
    let resets = fixture.handle.probes[0].resets.load(Ordering::Acquire);

    for (index, epoch) in [EPOCH + 1, EPOCH].into_iter().enumerate() {
        assert!(fixture.chain_mut().audio.set_epoch(epoch));
        wait_until(|| fixture.settled_at(epoch));
        assert_eq!(
            fixture.handle.probes[0].resets.load(Ordering::Acquire),
            resets + index as u64 + 1,
            "each distinct epoch must reach the real worker reset"
        );
    }

    assert_eq!(fixture.chain().audio.epoch(), EPOCH);
    assert_eq!(
        fixture.chain().control.plugin_endpoint_snapshot(),
        Some(endpoint)
    );
    assert_eq!(fixture.handle.state_for_slot(0), state);
    assert_eq!(
        fixture
            .chain()
            .control
            .metrics
            .epoch_resets
            .load(Ordering::Acquire),
        receipt.epoch_resets + 2
    );
    assert_eq!(
        fixture
            .chain()
            .control
            .effect_chain_receipt_settlement(&receipt),
        BounceSourceSettlement::Invalid,
        "matching current values cannot revive a pre-reset receipt"
    );

    let retry = fixture.request();
    let current = receive(&retry).unwrap();
    wait_until(|| {
        fixture
            .chain()
            .control
            .effect_chain_receipt_current(&current)
    });
    assert_eq!(current.epoch_resets, receipt.epoch_resets + 2);
    assert_eq!(current.slots[0].state, state);
    assert_eq!(fixture.handle.capture_count(0), 2);
    assert_eq!(fixture.chain().control.stats().faults, 0);
}

#[test]
fn effect_epoch_ticket_exhaustion_is_sticky_without_blocking_legacy_epoch_or_instrument_capture() {
    let mut fixture = Fixture::new(vec![SlotConfig::default()]);
    let receiver = fixture.request();
    let receipt = receive(&receiver).unwrap();
    wait_until(|| {
        fixture
            .chain()
            .control
            .effect_chain_receipt_current(&receipt)
    });
    let resets = fixture.handle.probes[0].resets.load(Ordering::Acquire);
    // Boundary injection changes only the callback's monotonic ticket, never worker settlement.
    fixture
        .chain()
        .control
        .metrics
        .epoch_resets
        .store(u64::MAX, Ordering::Release);
    assert!(fixture.chain_mut().audio.set_epoch(EPOCH + 1));
    wait_until(|| fixture.settled_at(EPOCH + 1));
    assert_eq!(fixture.chain().audio.epoch(), EPOCH + 1);
    assert_eq!(
        fixture.handle.probes[0].resets.load(Ordering::Acquire),
        resets + 1
    );
    assert!(
        fixture
            .chain()
            .control
            .metrics
            .effect_epoch_exhausted
            .load(Ordering::Acquire)
    );
    assert_eq!(
        fixture
            .chain()
            .control
            .metrics
            .epoch_resets
            .load(Ordering::Acquire),
        u64::MAX
    );
    assert!(
        fixture
            .chain()
            .control
            .request_effect_chain_state(
                EPOCH + 1,
                fixture.chain().control.plugin_endpoint_snapshot().unwrap(),
                fixture.handle.requests(),
                Instant::now() + WAIT,
            )
            .is_err()
    );

    // Exhaustion cannot break the original callback contract or wrap/reopen Effect admission.
    assert!(fixture.chain_mut().audio.set_epoch(EPOCH));
    wait_until(|| fixture.settled());
    assert_eq!(
        fixture.handle.probes[0].resets.load(Ordering::Acquire),
        resets + 2
    );
    assert!(
        fixture
            .chain()
            .control
            .metrics
            .effect_epoch_exhausted
            .load(Ordering::Acquire)
    );
    assert_eq!(
        fixture
            .chain()
            .control
            .metrics
            .epoch_resets
            .load(Ordering::Acquire),
        u64::MAX
    );
    assert!(
        !fixture
            .chain()
            .control
            .effect_chain_receipt_current(&receipt)
    );
    assert!(
        fixture
            .chain()
            .control
            .request_effect_chain_state(
                EPOCH,
                fixture.chain().control.plugin_endpoint_snapshot().unwrap(),
                fixture.handle.requests(),
                Instant::now() + WAIT,
            )
            .is_err()
    );

    let instrument = fixture
        .chain()
        .control
        .request_instrument_bounce_state(91, EPOCH, vec![3])
        .expect("Effect-only ticket exhaustion must not gate the original Instrument entry point");
    let deadline = Instant::now() + WAIT;
    let result = loop {
        match instrument.try_recv() {
            Ok(result) => break result,
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => panic!("legacy Instrument worker reply lost"),
        }
        assert!(
            Instant::now() < deadline,
            "legacy Instrument request did not terminate"
        );
        thread::sleep(Duration::from_millis(1));
    };
    assert!(instrument.termination_acknowledged());
    assert!(
        matches!(result, Err(BounceCaptureError::Rejected(ref message))
        if message == "This backend does not support instrument bounce capture")
    );
    assert_eq!(
        fixture.handle.capture_count(0),
        1,
        "refused Effect requests never call the backend"
    );
    assert_eq!(fixture.chain().control.stats().faults, 0);
}

#[test]
fn later_slot_revision_drift_refuses_the_entire_chain_at_backend_verification() {
    let fixture = Fixture::new(vec![SlotConfig::default(); 2]);
    *fixture.handle.probes[1].drift_target.lock().unwrap() =
        Some(Arc::clone(&fixture.handle.probes[0]));
    let receiver = fixture.request();
    assert!(rejection(receive(&receiver)).contains("drifted"));
    assert_eq!(fixture.handle.capture_count(0), 1);
    assert_eq!(fixture.handle.capture_count(1), 1);
    assert_eq!(fixture.handle.probes[0].verifies.load(Ordering::Acquire), 1);
    assert_eq!(fixture.handle.probes[1].verifies.load(Ordering::Acquire), 0);
    assert_eq!(fixture.chain().control.stats().faults, 0);
}

#[test]
fn effect_metadata_refusal_is_healthy_but_real_backend_failure_faults_the_slot() {
    for failure in [1, 2, 3] {
        let fixture = Fixture::new(vec![SlotConfig::default()]);
        fixture.handle.probes[0]
            .failure
            .store(failure, Ordering::Release);
        let receiver = fixture.request();
        let result = receive(&receiver);
        if failure == 3 {
            assert!(
                matches!(result, Err(BounceCaptureError::BackendFailed(ref message)) if message.contains("transport failure"))
            );
            assert_eq!(fixture.chain().control.stats().faults, 1);
            wait_until(|| {
                fixture
                    .chain()
                    .control
                    .plugin_endpoint_snapshot()
                    .is_some_and(|endpoint| !endpoint.slot(0).unwrap().active)
            });
        } else {
            let message = rejection(result);
            assert!(message.contains(if failure == 1 {
                "factory identity"
            } else {
                "stereo"
            }));
            assert_eq!(fixture.chain().control.stats().faults, 0);
            fixture.handle.probes[0].failure.store(0, Ordering::Release);
            wait_until(|| !fixture.chain().control.has_outstanding_admin_work());
            let retry = fixture.request();
            assert!(receive(&retry).is_ok());
        }
    }
}

#[test]
fn effect_capture_deadline_does_not_acknowledge_a_still_held_backend_call() {
    let fixture = Fixture::new(vec![SlotConfig::default()]);
    fixture.handle.hold_slot(0);
    let deadline = Instant::now() + Duration::from_millis(100);
    let receiver = fixture.request_until(deadline);
    fixture.handle.wait_entered(0);
    while Instant::now() <= deadline {
        thread::sleep(Duration::from_millis(1));
    }
    assert!(!receiver.termination_acknowledged());
    assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));
    assert!(
        fixture
            .chain()
            .control
            .metrics
            .bounce_capture_busy
            .load(Ordering::Acquire)
    );
    assert!(fixture.chain().control.has_outstanding_admin_work());
    fixture.handle.release_all();
    assert!(rejection(receive(&receiver)).contains("timed out"));
    assert_eq!(fixture.handle.probes[0].verifies.load(Ordering::Acquire), 0);
    assert_eq!(fixture.chain().control.stats().faults, 0);
}
