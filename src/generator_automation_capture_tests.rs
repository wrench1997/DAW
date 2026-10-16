//! Actual source worker/queue/typed-ACK tests, with no plugin/device loaded.
use super::*;
use crate::instrument_bounce::GeneratorAutomationCatalog;
use std::sync::atomic::AtomicBool;

const WAIT: Duration = Duration::from_secs(5);
const EPOCH: u64 = INITIAL_TRANSPORT_EPOCH + 1;
#[derive(Default)]
struct Probe {
    entered: AtomicBool,
    held: AtomicBool,
    release: AtomicBool,
    automated: AtomicU64,
    legacy: AtomicU64,
    mode: AtomicU32,
}
fn catalog() -> GeneratorAutomationCatalog {
    GeneratorAutomationCatalog {
        revision: 7,
        parameters: vec![PluginParameterDescriptor {
            id: 17,
            name: "Source gain".into(),
            unit: String::new(),
            current_normalized: 0.75,
            default_normalized: Some(0.5),
            step_count: Some(0),
            automatable: true,
            read_only: false,
            bypass: false,
        }],
    }
}
struct Backend(Arc<Probe>);
impl PluginBackend for Backend {
    fn name(&self) -> &str {
        "Actual automation capture worker fixture"
    }
    fn prepare(&mut self, _: PluginPrepareConfig) -> Result<(), String> {
        Ok(())
    }
    fn process(&mut self, _: &mut [f32], _: &mut [f32], _: usize) -> Result<(), String> {
        panic!("capture cannot advance source DSP")
    }
    fn send_midi(&mut self, _: MidiMessage) -> Result<(), String> {
        Ok(())
    }
    fn set_parameter(&mut self, _: u32, _: f32) -> Result<(), String> {
        Ok(())
    }
    fn get_parameter(&mut self, _: u32) -> Result<f32, String> {
        Ok(0.75)
    }
    fn save_state(&mut self) -> Result<Vec<u8>, String> {
        panic!("no shared SaveState")
    }
    fn load_state(&mut self, _: &[u8]) -> Result<(), String> {
        panic!("no source LoadState")
    }
    fn reset_processing(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn latency_samples(&self) -> u32 {
        0
    }
    fn tail_samples(&self) -> u32 {
        0
    }
    fn capture_instrument_bounce_state(
        &mut self,
        ids: &[u32],
        fence: &BounceCaptureFence,
    ) -> Result<BouncePluginState, BounceCaptureError> {
        assert_eq!(ids, [17]);
        fence.checkpoint()?;
        self.0.legacy.fetch_add(1, Ordering::Release);
        Ok(state())
    }
    fn capture_automated_instrument_bounce_state(
        &mut self,
        ids: &[u32],
        driven: &[u32],
        fence: &BounceCaptureFence,
    ) -> Result<(BouncePluginState, GeneratorAutomationCatalog), BounceCaptureError> {
        assert_eq!(ids, [17]);
        assert_eq!(driven, [17]);
        self.0.automated.fetch_add(1, Ordering::Release);
        self.0.entered.store(true, Ordering::Release);
        let deadline = Instant::now() + WAIT;
        while self.0.held.load(Ordering::Acquire) && !self.0.release.load(Ordering::Acquire) {
            assert!(Instant::now() < deadline, "held source was not released");
            thread::sleep(Duration::from_millis(1));
        }
        fence.checkpoint()?;
        let mut catalog = catalog();
        match self.0.mode.load(Ordering::Acquire) {
            1 => catalog.parameters[0].read_only = true,
            2 => catalog.parameters[0].automatable = false,
            3 => {
                return Err(BounceCaptureError::BackendFailed(
                    "real fixture capture failure".into(),
                ));
            }
            _ => {}
        }
        Ok((state(), catalog))
    }
}
fn until(mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !f() {
        assert!(
            Instant::now() < deadline,
            "actual automation source worker did not settle"
        );
        thread::sleep(Duration::from_millis(1));
    }
}
struct Fixture {
    chain: Option<PluginChain>,
    probe: Arc<Probe>,
}
impl Fixture {
    fn new() -> Self {
        let probe = Arc::new(Probe::default());
        let worker = Arc::clone(&probe);
        let mut chain = PluginChain::spawn_identified_with_backend_factory(
            &[41],
            move || vec![BackendSlot::new(Box::new(Backend(worker)))],
            PluginPrepareConfig {
                sample_rate: 48_000.0,
                max_block_frames: crate::instrument_bounce::BOUNCE_PREPARED_MAXIMUM,
            },
        )
        .unwrap();
        assert!(chain.audio.set_epoch(EPOCH));
        until(|| {
            chain.audio.stopped_playback_settled(EPOCH)
                && chain.control.plugin_endpoint_snapshot().is_some()
        });
        Self {
            chain: Some(chain),
            probe,
        }
    }
    fn chain(&self) -> &PluginChain {
        self.chain.as_ref().unwrap()
    }
    fn request(&self) -> BounceStateReceiver {
        self.chain()
            .control
            .request_whole_instrument_bounce_state(41, EPOCH, vec![17], vec![17])
            .unwrap()
    }
    fn receive(
        &self,
        receiver: &BounceStateReceiver,
    ) -> Result<BounceStateReceipt, BounceCaptureError> {
        let deadline = Instant::now() + WAIT;
        loop {
            match receiver.try_recv() {
                Ok(result) => {
                    assert!(receiver.termination_acknowledged());
                    return result;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => panic!("actual worker reply lost"),
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.probe.release.store(true, Ordering::Release);
        let mut chain = self.chain.take().unwrap();
        let deadline = Instant::now() + WAIT;
        let outcome = loop {
            let outcome = chain.guard.poll_shutdown_retained();
            if outcome != RetainedShutdownPoll::Pending || Instant::now() >= deadline {
                break outcome;
            }
            thread::sleep(Duration::from_millis(1));
        };
        if outcome != RetainedShutdownPoll::Joined {
            // Preserve the exact failed owner on assertion unwind; never ordinary
            // guard Drop/timeout-detach and call it a completed source shutdown.
            std::mem::forget(chain);
            if !thread::panicking() {
                panic!("actual source Join unproven: {outcome:?}");
            }
        }
    }
}

#[test]
fn whole_generator_capture_actual_worker_transfers_catalog_but_legacy_never_requires_it() {
    let f = Fixture::new();
    let before = f.chain().control.native_editor_snapshot(0).unwrap();
    let receipt = f.receive(&f.request()).unwrap();
    assert_eq!(receipt.automation_catalog, Some(catalog()));
    assert_eq!(receipt.state, state());
    until(|| !f.chain().control.has_outstanding_admin_work());
    let old = f
        .chain()
        .control
        .request_instrument_bounce_state(41, EPOCH, vec![17])
        .unwrap();
    assert!(f.receive(&old).unwrap().automation_catalog.is_none());
    assert_eq!(f.probe.automated.load(Ordering::Acquire), 1);
    assert_eq!(f.probe.legacy.load(Ordering::Acquire), 1);
    let after = f.chain().control.native_editor_snapshot(0).unwrap();
    assert_eq!(before.captured_state, after.captured_state);
    assert_eq!(before.capture_serial, after.capture_serial);
    assert_eq!(f.chain().control.stats().faults, 0);
}

#[test]
fn whole_generator_capture_cancel_holds_original_worker_lease_until_actual_typed_ack() {
    let f = Fixture::new();
    f.probe.held.store(true, Ordering::Release);
    let receiver = f.request();
    until(|| f.probe.entered.load(Ordering::Acquire));
    receiver.cancel();
    assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));
    assert!(!receiver.termination_acknowledged());
    assert!(
        f.chain()
            .control
            .metrics
            .bounce_capture_busy
            .load(Ordering::Acquire)
    );
    assert!(
        f.chain()
            .control
            .request_whole_instrument_bounce_state(41, EPOCH, vec![17], vec![17])
            .is_err()
    );
    f.probe.release.store(true, Ordering::Release);
    assert!(
        matches!(f.receive(&receiver),Err(BounceCaptureError::Rejected(ref s)) if s.contains("cancelled"))
    );
    until(|| !f.chain().control.has_outstanding_admin_work());
    assert!(
        !f.chain()
            .control
            .metrics
            .bounce_capture_busy
            .load(Ordering::Acquire)
    );
    assert_eq!(f.chain().control.stats().faults, 0);
}

#[test]
fn whole_generator_capture_catalog_refusal_is_healthy_but_genuine_backend_fault_is_not() {
    for mode in [1, 2, 3] {
        let f = Fixture::new();
        f.probe.mode.store(mode, Ordering::Release);
        let result = f.receive(&f.request());
        if mode == 3 {
            assert!(matches!(result, Err(BounceCaptureError::BackendFailed(_))));
        } else {
            assert!(matches!(result, Err(BounceCaptureError::Rejected(_))));
        }
        assert_eq!(f.chain().control.stats().faults, u64::from(mode == 3));
    }
}
