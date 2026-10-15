//! Insert capture is attached to the existing actual stopped actor, never a forged Generator.
use super::*;
use crate::instrument_bounce::effect::EffectCapturePollError as CapturePollError;
use crate::model::PluginFormat;
use crate::{
    audio::PreparedPlaybackToken,
    instrument_bounce::effect::{EffectCaptureSlotRequest, EffectChainStateReceipt},
    plugins::plugin_runtime::{EffectChainStateReceiver, PluginControlIdentity, SlotConfig},
};
use std::sync::mpsc::TryRecvError;

#[derive(Clone, Debug, PartialEq, Eq)]
struct NativeBinding {
    generation: u64,
    dirty_revision: u64,
    capture_serial: u64,
    parameter_capture_serial: u64,
    completed_request: u64,
}

struct EffectBinding {
    project_session: u64,
    track: usize,
    endpoint_id: u64,
    owner: PluginControlIdentity,
    endpoint: PluginEndpointSnapshot,
    placements: Vec<MixerInsertSlotRef>,
    requests: Vec<EffectCaptureSlotRequest>,
    native: Vec<NativeBinding>,
    token: PreparedPlaybackToken,
    sample_rate: u32,
}

/// A single ordered endpoint receipt. The caller receives all slots together, after actual
/// termination and exact source rechecks; it cannot construct one from Project/cache bytes.
pub(in crate::app) struct CapturedStoppedEffectChain {
    pub(in crate::app) project_fingerprint: u64,
    pub(in crate::app) track: usize,
    pub(in crate::app) endpoint_id: u64,
    pub(in crate::app) placements: Vec<MixerInsertSlotRef>,
    pub(in crate::app) token: PreparedPlaybackToken,
    pub(in crate::app) receipt: EffectChainStateReceipt,
}

pub(in crate::app) struct StoppedEffectChainCapture {
    binding: EffectBinding,
    receiver: Option<EffectChainStateReceiver>,
    receipt: Option<EffectChainStateReceipt>,
    abort_reason: Option<CapturePollError>,
    started_at: Instant,
}
impl StoppedEffectChainCapture {
    pub(super) fn cancel(&mut self) {
        self.abort_reason.get_or_insert(CapturePollError::Cancelled);
        if let Some(receiver) = &self.receiver {
            receiver.cancel();
        }
    }

    pub(in crate::app) fn termination_acknowledged(&self) -> bool {
        self.receiver
            .as_ref()
            .is_none_or(EffectChainStateReceiver::termination_acknowledged)
    }

    /// The shutdown owner needs no surviving App/audio engine to drain its cancelled command.
    /// A real backend fault keeps precedence over cancellation and remains in worker health.
    pub(in crate::app) fn poll_cancelled(&mut self) -> Result<bool, CapturePollError> {
        self.cancel();
        let Some(receiver) = &self.receiver else {
            return Ok(true);
        };
        match receiver.try_recv() {
            Err(TryRecvError::Empty) => Ok(false),
            Err(TryRecvError::Disconnected) => Err(CapturePollError::Disconnected),
            Ok(Err(crate::instrument_bounce::BounceCaptureError::BackendFailed(message))) => {
                Err(CapturePollError::Backend(
                    crate::instrument_bounce::BounceCaptureError::BackendFailed(message),
                ))
            }
            Ok(_) => {
                self.receiver = None;
                self.receipt = None;
                Ok(true)
            }
        }
    }

    fn abort(&mut self, reason: CapturePollError) {
        self.abort_reason.get_or_insert(reason);
        if let Some(receiver) = &self.receiver {
            receiver.cancel();
        }
    }

    fn poll(
        &mut self,
        app: &CitrusApp,
        snapshot: &[u8],
        source_valid: Result<(), String>,
    ) -> Result<Option<CapturedStoppedEffectChain>, CapturePollError> {
        if let Err(reason) = source_valid.and_then(|()| self.binding.recheck(app)) {
            self.abort(CapturePollError::Stale(reason));
        }
        if self.started_at.elapsed() >= crate::instrument_bounce::BOUNCE_CAPTURE_TIMEOUT {
            self.abort(CapturePollError::Deadline);
        }
        if self.receipt.is_none() {
            let result = self
                .receiver
                .as_ref()
                .ok_or_else(|| {
                    CapturePollError::Invalid("Effect capture ownership was lost".into())
                })?
                .try_recv();
            match result {
                Err(TryRecvError::Empty) => return Ok(None),
                Err(TryRecvError::Disconnected) => return Err(CapturePollError::Disconnected),
                Ok(Err(crate::instrument_bounce::BounceCaptureError::BackendFailed(message))) => {
                    return Err(CapturePollError::Backend(
                        crate::instrument_bounce::BounceCaptureError::BackendFailed(message),
                    ));
                }
                Ok(result) => {
                    if let Some(reason) = &self.abort_reason {
                        return Err(reason.clone());
                    }
                    self.receipt = Some(result.map_err(CapturePollError::Backend)?);
                    self.receiver = None;
                }
            }
        }
        if let Some(reason) = &self.abort_reason {
            return Err(reason.clone());
        }
        let receipt = self.receipt.as_ref().expect("received complete chain");
        if receipt
            .slots
            .iter()
            .any(|slot| slot.state.metadata.sample_rate != f64::from(self.binding.sample_rate))
        {
            return Err(CapturePollError::Invalid(
                "The actual Effect source rate differs from the frozen audio graph".into(),
            ));
        }
        let chain = app
            .running_insert_chains
            .get(&self.binding.track)
            .ok_or_else(|| {
                CapturePollError::Stale("The captured Insert endpoint disappeared".into())
            })?;
        match chain.control.effect_chain_receipt_settlement(receipt) {
            BounceSourceSettlement::Waiting => return Ok(None),
            BounceSourceSettlement::Invalid => {
                return Err(CapturePollError::Stale(
                    "Effect input/admin/parameter work or endpoint changed after capture".into(),
                ));
            }
            BounceSourceSettlement::Ready => {}
        }
        // This is a current callback witness, not a claim that a new callback ran. If capture
        // completed between callbacks, unchanged monotonic worker tickets preserve the witness.
        let token = app
            .audio
            .as_ref()
            .and_then(AudioEngine::prepared_playback_token);
        if token.is_some_and(|token| token != self.binding.token) {
            return Err(CapturePollError::Stale(
                "The prepared graph/timing/PDC identity changed".into(),
            ));
        }
        if token.is_none() {
            return Ok(None);
        }
        self.binding.recheck(app).map_err(CapturePollError::Stale)?;
        match chain.control.effect_chain_receipt_settlement(receipt) {
            BounceSourceSettlement::Waiting => return Ok(None),
            BounceSourceSettlement::Invalid => {
                return Err(CapturePollError::Stale(
                    "Effect source changed around callback validation".into(),
                ));
            }
            BounceSourceSettlement::Ready => {}
        }
        Ok(Some(CapturedStoppedEffectChain {
            project_fingerprint: project_snapshot_fingerprint(snapshot),
            track: self.binding.track,
            endpoint_id: self.binding.endpoint_id,
            placements: self.binding.placements.clone(),
            token: self.binding.token,
            receipt: self.receipt.take().expect("checked complete chain"),
        }))
    }
}

impl EffectBinding {
    fn observe(
        app: &CitrusApp,
        track: usize,
        token: PreparedPlaybackToken,
    ) -> Result<Self, String> {
        let chain = app
            .running_insert_chains
            .get(&track)
            .ok_or("No callback-confirmed Insert endpoint")?;
        if chain.project_session != app.project_session || app.plugin_topology_is_settling() {
            return Err("The Insert endpoint/session is changing".into());
        }
        let state = snapshot_mixer_plugin_track(&app.project, track);
        if state.slots.is_empty()
            || state.slots.len() != state.instances.len()
            || state
                .instances
                .iter()
                .map(|p| p.id)
                .ne(chain.instance_ids.iter().copied())
        {
            return Err(
                "The complete ordered Insert placement differs from the actual endpoint".into(),
            );
        }
        let endpoint = chain
            .control
            .plugin_endpoint_snapshot()
            .ok_or("Insert endpoint metadata is unavailable")?;
        let stats = chain.control.stats();
        if endpoint.slot_count() != state.slots.len()
            || stats.stopped
            || stats.faults != 0
            || chain.control.outstanding_parameter_edits() != 0
        {
            return Err("The Insert endpoint is unavailable, faulted or has parameter work".into());
        }
        let mut requests = Vec::with_capacity(state.instances.len());
        let mut native = Vec::with_capacity(state.instances.len());
        for (slot, instance) in state.instances.iter().enumerate() {
            if instance.format != PluginFormat::Vst3
                || instance.runtime_status != PluginRuntimeStatus::Loaded
                || endpoint.manifest().instance_id(slot) != Some(instance.id)
                || !instance.wet.is_finite()
                || !(0.0..=1.0).contains(&instance.wet)
                || instance.midi_ports.input.is_some()
                || instance.midi_ports.output.is_some()
                || endpoint
                    .slot(slot)
                    .is_none_or(|s| s.is_active() != (instance.enabled && !instance.bypass))
            {
                return Err("Insert capture requires exact loaded VST3 slots/configurations without MIDI routing".into());
            }
            let spec = plugins::plugin_load_identity_from_instance(instance);
            requests.push(EffectCaptureSlotRequest {
                instance_id: instance.id,
                config: SlotConfig {
                    enabled: instance.enabled,
                    bypassed: instance.bypass,
                    wet: instance.wet,
                },
                path: spec.descriptor.path,
                requested_class_uid: spec.class_uid,
                parameter_ids: instance.parameters.keys().copied().collect(),
            });
            let observed = chain
                .control
                .native_editor_snapshot(slot)
                .ok_or("Insert native observation is unavailable")?;
            if observed.state.open
                || observed.pending_request.is_some()
                || observed.capture_in_progress
                || observed.error.is_some()
            {
                return Err(
                    "Close every source Effect editor and finish native work before capture".into(),
                );
            }
            native.push(NativeBinding {
                generation: observed.state.generation,
                dirty_revision: observed.dirty_revision,
                capture_serial: observed.capture_serial,
                parameter_capture_serial: observed.parameter_capture_serial,
                completed_request: observed.completed_request,
            });
        }
        Ok(Self {
            project_session: app.project_session,
            track,
            endpoint_id: chain.endpoint_id,
            owner: chain.control.owner_identity(),
            endpoint,
            placements: state.slots,
            requests,
            native,
            token,
            sample_rate: app
                .audio
                .as_ref()
                .ok_or("Audio is unavailable")?
                .snapshot()
                .sample_rate,
        })
    }

    fn recheck(&self, app: &CitrusApp) -> Result<(), String> {
        let audio = app
            .audio
            .as_ref()
            .ok_or("Audio disappeared during Effect capture")?;
        if audio.engine_session() != self.token.engine_session
            || app.project_session != self.project_session
            || audio.project_session() != self.project_session
            || audio.transport_request_id() != self.token.transport_request
            || audio.requested_plugin_timing_revision() != self.token.timing_revision
            || audio.confirmed_timeline_identity()
                != Some((self.token.timeline_revision, self.token.epoch))
            || audio.snapshot().timeline_frame != self.token.timeline_frame
            || audio.snapshot().sample_rate != self.sample_rate
        {
            return Err(
                "The stopped Effect engine/session/Timeline/transport/timing binding changed"
                    .into(),
            );
        }
        let current = Self::observe(app, self.track, self.token)?;
        let chain = app
            .running_insert_chains
            .get(&self.track)
            .expect("observed endpoint");
        if current.endpoint_id != self.endpoint_id
            || !self.owner.matches(&chain.control)
            || current.endpoint != self.endpoint
            || current.placements != self.placements
            || current.requests != self.requests
            || current.native != self.native
        {
            return Err(
                "The exact Effect endpoint, slot order, configuration or native revision changed"
                    .into(),
            );
        }
        Ok(())
    }
}

impl StoppedBounceCapture {
    /// Call only after this actor's actual Stop/Timeline/worker-ready boundary. This adds no
    /// Whole admission and retains the same frozen Project/source/session owner.
    pub(in crate::app) fn begin_effect_chain_capture(
        &mut self,
        app: &CitrusApp,
        track: usize,
    ) -> Result<(), String> {
        let source = self.live_recheck(app)?;
        self.stop_intent_current(app)?;
        if self.owns_pending_capture() || self.ready_binding != Some(source) {
            return Err("The exact stopped actor is not ready for a private Effect capture".into());
        }
        let audio = app.audio.as_ref().ok_or("Audio is unavailable")?;
        let token = audio
            .prepared_playback_token()
            .ok_or("Wait for every stopped graph worker to settle")?;
        if token.engine_session != source.stamp.engine_session
            || token.project_session != source.stamp.project_session
            || token.timeline_revision != source.stamp.timeline_revision
            || token.epoch != source.stamp.epoch
            || token.transport_request
                != self
                    .stop_transport_request
                    .ok_or("Stopped request is missing")?
        {
            return Err("The callback graph witness differs from the actual stopped actor".into());
        }
        let binding = EffectBinding::observe(app, track, token)?;
        binding.recheck(app)?;
        let receiver = app.running_insert_chains[&track]
            .control
            .request_effect_chain_state(
                token.epoch,
                binding.endpoint,
                binding.requests.clone(),
                Instant::now() + crate::instrument_bounce::BOUNCE_CAPTURE_TIMEOUT,
            )?;
        self.effect_capture = Some(StoppedEffectChainCapture {
            binding,
            receiver: Some(receiver),
            receipt: None,
            abort_reason: None,
            started_at: Instant::now(),
        });
        self.stopping = false;
        Ok(())
    }

    pub(in crate::app) fn poll_effect_chain_capture(
        &mut self,
        app: &CitrusApp,
    ) -> Result<Option<CapturedStoppedEffectChain>, CapturePollError> {
        let valid = self
            .live_recheck(app)
            .map(|_| ())
            .and_then(|()| self.stop_intent_current(app));
        let capture = self.effect_capture.as_mut().ok_or_else(|| {
            CapturePollError::Invalid("No private Effect capture is pending".into())
        })?;
        let result = capture.poll(app, &self.snapshot, valid);
        if !matches!(result, Ok(None)) {
            self.effect_capture = None;
        }
        result
    }

    /// Shutdown adapters retain this owner and its source fence until poll reports terminal.
    pub(in crate::app) fn take_cancelled_effect_capture_for_shutdown(
        &mut self,
    ) -> Option<StoppedEffectChainCapture> {
        self.cancel_capture();
        self.effect_capture.take()
    }
}
