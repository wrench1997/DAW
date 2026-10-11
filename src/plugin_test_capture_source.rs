//! Shared, plugin-free controller fixture with a real identified worker and capture commands.
//!
//! The returned chain owns its real JoinHandle. Tests moving it into an App must keep that App
//! in a scoped cleanup owner (including assertion/unwind paths): release the gate, recover the
//! exact running or retired guard, and poll its retained shutdown to terminal before dropping it.
//! Dropping this handle only releases its mock gate; it cannot join a guard owned by the App.

use super::*;
use std::sync::Condvar;

const CAPTURE_GATE_BUDGET: Duration = Duration::from_secs(5);

struct Shared {
    state: Mutex<BouncePluginState>,
    feedback: Mutex<NativeEditorFeedback>,
    capture_count: AtomicU64,
    capture_paused: Mutex<bool>,
    capture_resume: Condvar,
    capture_failure_after_gate: Mutex<Option<String>>,
    panic_on_backend_drop: AtomicBool,
    prepare_paused: Mutex<bool>,
    prepare_resume: Condvar,
    reset: ProcessingGate,
    process: ProcessingGate,
    fail_process_after_gate: AtomicBool,
}

/// Mock-only backend blocking; production publications still come from the
/// real worker scopes. Default inactive, bounded, and released by the retained
/// Fixture before any unwind cleanup poll or exact JoinHandle shutdown.
#[derive(Default)]
struct ProcessingGate {
    entries: AtomicU64,
    paused: Mutex<bool>,
    resume: Condvar,
}
impl ProcessingGate {
    fn enter(&self) -> Result<(), String> {
        self.entries.fetch_add(1, Ordering::AcqRel);
        let paused = self.paused.lock().unwrap();
        let (paused, timeout) = self
            .resume
            .wait_timeout_while(paused, CAPTURE_GATE_BUDGET, |paused| *paused)
            .unwrap();
        if timeout.timed_out() && *paused {
            return Err("Capture fixture processing gate exceeded its bounded test budget".into());
        }
        Ok(())
    }
    fn pause(&self) {
        *self.paused.lock().unwrap() = true;
    }
    fn release(&self) {
        *self
            .paused
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = false;
        self.resume.notify_all();
    }
}

/// Control handle for a deterministic backend used by the actual worker/capture machinery.
#[must_use]
pub(crate) struct TestCaptureSource {
    shared: Arc<Shared>,
}

impl TestCaptureSource {
    pub(crate) fn new(
        instance_id: u64,
        state: BouncePluginState,
        config: PluginPrepareConfig,
    ) -> Result<(PluginChain, Self), String> {
        Self::new_with_prepare_gate(instance_id, state, config, false)
    }

    /// Explicit plugin-free seam: keep the real worker's preparation pending
    /// until released. No callback state, receipt or readiness is fabricated.
    #[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
    pub(crate) fn new_with_held_prepare(
        instance_id: u64,
        state: BouncePluginState,
        config: PluginPrepareConfig,
    ) -> Result<(PluginChain, Self), String> {
        Self::new_with_prepare_gate(instance_id, state, config, true)
    }

    fn new_with_prepare_gate(
        instance_id: u64,
        state: BouncePluginState,
        config: PluginPrepareConfig,
        prepare_paused: bool,
    ) -> Result<(PluginChain, Self), String> {
        let name = state.metadata.name.clone();
        let feedback = NativeEditorFeedback {
            state: NativeEditorState {
                generation: state.native_generation,
                ..Default::default()
            },
            dirty_revision: state.native_dirty_revision,
            catalog_invalidated: false,
        };
        let shared = Arc::new(Shared {
            state: Mutex::new(state),
            feedback: Mutex::new(feedback),
            capture_count: AtomicU64::new(0),
            capture_paused: Mutex::new(false),
            capture_resume: Condvar::new(),
            capture_failure_after_gate: Mutex::new(None),
            panic_on_backend_drop: AtomicBool::new(false),
            prepare_paused: Mutex::new(prepare_paused),
            prepare_resume: Condvar::new(),
            reset: ProcessingGate::default(),
            process: ProcessingGate::default(),
            fail_process_after_gate: AtomicBool::new(false),
        });
        let worker_shared = Arc::clone(&shared);
        let chain = PluginChain::spawn_identified_with_backend_factory(
            &[instance_id],
            move || {
                vec![BackendSlot::new(Box::new(CaptureBackend {
                    name,
                    shared: worker_shared,
                }))]
            },
            config,
        )?;
        Ok((chain, Self { shared }))
    }

    /// Change capture authority without publishing a native dirty notification.
    pub(crate) fn set_capture_state(&self, state: BouncePluginState) {
        *self.shared.state.lock().unwrap() = state;
    }

    /// The production worker's normal feedback poll publishes this value asynchronously.
    pub(crate) fn set_native_feedback(&self, feedback: NativeEditorFeedback) {
        *self.shared.feedback.lock().unwrap() = feedback;
    }

    /// Number of actual backend capture entries, including any currently paused capture.
    pub(crate) fn capture_count(&self) -> u64 {
        self.shared.capture_count.load(Ordering::Acquire)
    }

    pub(crate) fn pause_capture(&self) {
        *self.shared.capture_paused.lock().unwrap() = true;
    }

    /// Model an already-returned backend failure before the post-call cancellation checkpoint.
    pub(crate) fn fail_capture_after_gate(&self, message: String) {
        *self.shared.capture_failure_after_gate.lock().unwrap() = Some(message);
    }

    /// Arm one actual backend teardown panic; the retained JoinHandle reports the failure.
    pub(crate) fn panic_on_shutdown(&self) {
        self.shared
            .panic_on_backend_drop
            .store(true, Ordering::Release);
    }

    pub(crate) fn pause_reset(&self) {
        self.shared.reset.pause();
    }
    pub(crate) fn release_reset(&self) {
        self.shared.reset.release();
    }
    pub(crate) fn reset_count(&self) -> u64 {
        self.shared.reset.entries.load(Ordering::Acquire)
    }
    pub(crate) fn pause_process(&self) {
        self.shared.process.pause();
    }
    pub(crate) fn release_process(&self) {
        self.shared.process.release();
    }
    pub(crate) fn process_count(&self) -> u64 {
        self.shared.process.entries.load(Ordering::Acquire)
    }
    pub(crate) fn fail_process_after_gate(&self) {
        self.shared
            .fail_process_after_gate
            .store(true, Ordering::Release);
    }

    pub(crate) fn release_gates(&self) {
        self.release_reset();
        self.release_process();
        self.release_prepare();
        *self
            .shared
            .capture_paused
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = false;
        self.shared.capture_resume.notify_all();
    }

    pub(crate) fn release_prepare(&self) {
        *self
            .shared
            .prepare_paused
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = false;
        self.shared.prepare_resume.notify_all();
    }
}

impl Drop for TestCaptureSource {
    fn drop(&mut self) {
        self.release_gates();
    }
}

struct CaptureBackend {
    name: String,
    shared: Arc<Shared>,
}

impl Drop for CaptureBackend {
    fn drop(&mut self) {
        let armed = self
            .shared
            .panic_on_backend_drop
            .swap(false, Ordering::AcqRel);
        // An existing worker unwind already produces WorkerPanicked; never turn it into abort.
        if armed && !thread::panicking() {
            panic!("capture fixture backend shutdown panic");
        }
    }
}

impl PluginBackend for CaptureBackend {
    fn name(&self) -> &str {
        &self.name
    }

    fn prepare(&mut self, _: PluginPrepareConfig) -> Result<(), String> {
        let paused = self.shared.prepare_paused.lock().unwrap();
        let (paused, timeout) = self
            .shared
            .prepare_resume
            .wait_timeout_while(paused, CAPTURE_GATE_BUDGET, |paused| *paused)
            .unwrap();
        if timeout.timed_out() && *paused {
            return Err("Capture fixture preparation exceeded its bounded test budget".into());
        }
        Ok(())
    }

    fn process(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        frames: usize,
    ) -> Result<(), String> {
        self.shared.process.enter()?;
        if self
            .shared
            .fail_process_after_gate
            .swap(false, Ordering::AcqRel)
        {
            return Err("Capture fixture actual Process failure".into());
        }
        left[..frames].fill(0.0);
        right[..frames].fill(0.0);
        Ok(())
    }

    fn reset_processing(&mut self) -> Result<(), String> {
        self.shared.reset.enter()
    }

    fn send_midi(&mut self, _: MidiMessage) -> Result<(), String> {
        Ok(())
    }

    fn midi_capabilities(&self) -> (bool, bool) {
        (true, false)
    }

    fn set_parameter(&mut self, id: u32, normalized: f32) -> Result<(), String> {
        let mut state = self.shared.state.lock().unwrap();
        let value = state
            .parameter_bases
            .get_mut(&id)
            .ok_or_else(|| "Unknown capture-fixture parameter".to_owned())?;
        *value = normalized;
        Ok(())
    }

    fn get_parameter(&mut self, id: u32) -> Result<f32, String> {
        self.shared
            .state
            .lock()
            .unwrap()
            .parameter_bases
            .get(&id)
            .copied()
            .ok_or_else(|| "Unknown capture-fixture parameter".to_owned())
    }

    fn native_editor_feedback(&mut self) -> Result<NativeEditorFeedback, String> {
        Ok(self.shared.feedback.lock().unwrap().clone())
    }

    fn save_state(&mut self) -> Result<Vec<u8>, String> {
        Ok(self.shared.state.lock().unwrap().opaque_state.clone())
    }

    fn load_state(&mut self, _: &[u8]) -> Result<(), String> {
        Err("Capture fixture does not reload its source".to_owned())
    }

    fn capture_instrument_bounce_state(
        &mut self,
        ids: &[u32],
        fence: &BounceCaptureFence,
    ) -> Result<BouncePluginState, BounceCaptureError> {
        fence.checkpoint()?;
        self.shared.capture_count.fetch_add(1, Ordering::AcqRel);
        let paused = self.shared.capture_paused.lock().unwrap();
        let (paused, timeout) = self
            .shared
            .capture_resume
            .wait_timeout_while(paused, CAPTURE_GATE_BUDGET, |paused| *paused)
            .unwrap();
        if timeout.timed_out() && *paused {
            return Err(BounceCaptureError::Rejected(
                "Capture fixture pause exceeded its bounded test budget".to_owned(),
            ));
        }
        drop(paused);
        if let Some(message) = self
            .shared
            .capture_failure_after_gate
            .lock()
            .unwrap()
            .take()
        {
            return Err(BounceCaptureError::BackendFailed(message));
        }
        fence.checkpoint()?;
        let mut captured = self.shared.state.lock().unwrap().clone();
        if ids
            .iter()
            .any(|id| !captured.parameter_bases.contains_key(id))
        {
            return Err(BounceCaptureError::BackendFailed(
                "Capture fixture is missing a requested base parameter".to_owned(),
            ));
        }
        captured.parameter_bases.retain(|id, _| ids.contains(id));
        Ok(captured)
    }

    fn latency_samples(&self) -> u32 {
        self.shared.state.lock().unwrap().metadata.latency_samples
    }

    fn tail_samples(&self) -> u32 {
        self.shared.state.lock().unwrap().metadata.tail_samples
    }

    fn processing_metadata(&self) -> Result<(u32, u32), String> {
        let state = self.shared.state.lock().unwrap();
        Ok((state.metadata.latency_samples, state.metadata.tail_samples))
    }
}
