//! Worker-owned atomic private capture, sharing the established cancellation/termination lease.
use super::*;
use crate::instrument_bounce::effect::CapturedEffectSlot;

/// Read-only stopped worker history for the serial Whole source lease. It does
/// not change admission or receipt rules for existing Instrument callers.
#[derive(Clone, Copy, Debug)]
pub(crate) struct StoppedSourceWorkTicket {
    epoch: u64,
    submitted: u64,
    admin_revision: u64,
    epoch_resets: u64,
}
impl StoppedSourceWorkTicket {
    pub(crate) fn from_effect_receipt(receipt: &EffectChainStateReceipt) -> Self {
        Self {
            epoch: receipt.epoch,
            submitted: receipt.submitted,
            admin_revision: receipt.admin_revision,
            epoch_resets: receipt.epoch_resets,
        }
    }
    pub(crate) fn following_private_capture(mut self) -> Option<Self> {
        self.admin_revision = self.admin_revision.checked_add(1)?;
        Some(self)
    }
}

pub(crate) struct EffectChainStateReceiver {
    receiver: Receiver<Result<EffectChainStateReceipt, BounceCaptureError>>,
    fence: Arc<BounceCaptureFence>,
}
impl EffectChainStateReceiver {
    pub(crate) fn cancel(&self) {
        self.fence.cancel();
    }
    pub(crate) fn termination_acknowledged(&self) -> bool {
        self.fence.terminated.load(Ordering::Acquire)
    }
    pub(crate) fn try_recv(
        &self,
    ) -> Result<Result<EffectChainStateReceipt, BounceCaptureError>, TryRecvError> {
        if !self.termination_acknowledged() {
            return Err(TryRecvError::Empty);
        }
        self.receiver.try_recv()
    }
}
impl Drop for EffectChainStateReceiver {
    fn drop(&mut self) {
        self.cancel();
    }
}

impl PluginChainControl {
    pub(crate) fn stopped_source_work_ticket(&self, epoch: u64) -> Option<StoppedSourceWorkTicket> {
        let ticket = StoppedSourceWorkTicket {
            epoch,
            submitted: self.metrics.submitted.load(Ordering::Acquire),
            admin_revision: self.metrics.effect_admin_revision.load(Ordering::Acquire),
            epoch_resets: self.metrics.epoch_resets.load(Ordering::Acquire),
        };
        (self.stopped_source_work_settlement(ticket) == BounceSourceSettlement::Ready)
            .then_some(ticket)
    }

    pub(crate) fn stopped_source_work_settlement(
        &self,
        ticket: StoppedSourceWorkTicket,
    ) -> BounceSourceSettlement {
        if !effect_tickets_unchanged(
            &self.metrics,
            ticket.epoch,
            ticket.submitted,
            ticket.admin_revision,
            ticket.epoch_resets,
            &self.parameter_edit_admission,
        ) {
            return BounceSourceSettlement::Invalid;
        }
        if self.has_outstanding_admin_work()
            || self.metrics.bounce_capture_busy.load(Ordering::Acquire)
            || self.metrics.worker_settlement.busy.load(Ordering::Acquire) != 0
        {
            return BounceSourceSettlement::Waiting;
        }
        BounceSourceSettlement::Ready
    }

    pub(crate) fn request_effect_chain_state(
        &self,
        expected_epoch: u64,
        endpoint: PluginEndpointSnapshot,
        requests: Vec<EffectCaptureSlotRequest>,
        deadline: Instant,
    ) -> Result<EffectChainStateReceiver, String> {
        if expected_epoch == 0
            || requests.is_empty()
            || requests.len() > MAX_CHAIN_SLOTS
            || self.plugin_endpoint_snapshot() != Some(endpoint)
            || endpoint.slot_count() != requests.len()
            || requests.iter().enumerate().any(|(slot, request)| {
                self.manifest.instance_id(slot) != Some(request.instance_id)
                    || !request.config.wet.is_finite()
                    || !(0.0..=1.0).contains(&request.config.wet)
                    || request.parameter_ids.len() > MAX_PLUGIN_PARAMETER_CATALOG_ITEMS
                    || request.parameter_ids.windows(2).any(|p| p[0] >= p[1])
            })
            || self.has_outstanding_admin_work()
            || self.parameter_edit_admission.load(Ordering::Acquire) != 0
            || self.metrics.effect_admin_exhausted.load(Ordering::Acquire)
            || self.metrics.effect_epoch_exhausted.load(Ordering::Acquire)
        {
            return Err("Effect capture requires the exact ordered endpoint and drained parameter/admin work".into());
        }
        let native = self
            .metrics
            .native_editors
            .lock()
            .map_err(|_| "Native snapshot lock poisoned")?;
        if native[..requests.len()].iter().any(|n| !editor_closed(n)) {
            return Err("Effect capture source editor is open, pending or faulted".into());
        }
        if self
            .metrics
            .bounce_capture_busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err("A private capture is already pending for this endpoint".into());
        }
        let fence = Arc::new(BounceCaptureFence::new_effect(deadline));
        let lease = BounceCaptureLease {
            fence: Arc::clone(&fence),
            metrics: Arc::clone(&self.metrics),
        };
        let submitted = self.metrics.submitted.load(Ordering::Acquire);
        let epoch_resets = self.metrics.epoch_resets.load(Ordering::Acquire);
        let admin_revision = self
            .metrics
            .effect_admin_revision
            .load(Ordering::Acquire)
            .checked_add(1)
            .ok_or("Effect capture admin identity exhausted")?;
        let (reply, receiver) = mpsc::sync_channel(1);
        if !self.try_admin(AdminCommand::CaptureEffectChain {
            expected_epoch,
            endpoint: Box::new(endpoint),
            requests,
            submitted,
            admin_revision,
            epoch_resets,
            parameter_admission: Arc::clone(&self.parameter_edit_admission),
            lease,
            reply,
        }) {
            return Err("Effect capture command queue is full or disconnected".into());
        }
        Ok(EffectChainStateReceiver { receiver, fence })
    }

    /// Recheck after actual termination and around the App's callback witness read. Neither a
    /// cached callback token nor a consumed reply can erase intervening input/admin intent.
    pub(crate) fn effect_chain_receipt_current(&self, receipt: &EffectChainStateReceipt) -> bool {
        self.effect_chain_receipt_settlement(receipt) == BounceSourceSettlement::Ready
    }

    pub(crate) fn effect_chain_receipt_settlement(
        &self,
        receipt: &EffectChainStateReceipt,
    ) -> BounceSourceSettlement {
        if !effect_tickets_unchanged(
            &self.metrics,
            receipt.epoch,
            receipt.submitted,
            receipt.admin_revision,
            receipt.epoch_resets,
            &self.parameter_edit_admission,
        ) || self.plugin_endpoint_snapshot() != Some(receipt.endpoint)
            || !self
                .metrics
                .native_editors
                .lock()
                .is_ok_and(|n| n[..receipt.slots.len()].iter().all(editor_closed))
        {
            return BounceSourceSettlement::Invalid;
        }
        if self.has_outstanding_admin_work()
            || self.metrics.bounce_capture_busy.load(Ordering::Acquire)
            || self.metrics.worker_settlement.busy.load(Ordering::Acquire) != 0
        {
            return BounceSourceSettlement::Waiting;
        }
        BounceSourceSettlement::Ready
    }
}

fn editor_closed(n: &NativeEditorSnapshot) -> bool {
    !n.state.open && n.pending_request.is_none() && !n.capture_in_progress && n.error.is_none()
}

fn effect_tickets_current(
    metrics: &BridgeMetrics,
    epoch: u64,
    submitted: u64,
    admin_revision: u64,
    epoch_resets: u64,
    parameter_admission: &AtomicU32,
    own_admin: u32,
) -> bool {
    effect_tickets_unchanged(
        metrics,
        epoch,
        submitted,
        admin_revision,
        epoch_resets,
        parameter_admission,
    ) && metrics.outstanding_admin_work.load(Ordering::Acquire) == own_admin
        && metrics.worker_settlement.busy.load(Ordering::Acquire) == own_admin.min(1)
}

fn effect_tickets_unchanged(
    metrics: &BridgeMetrics,
    epoch: u64,
    submitted: u64,
    admin_revision: u64,
    epoch_resets: u64,
    parameter_admission: &AtomicU32,
) -> bool {
    let worker = &metrics.worker_settlement;
    !metrics.effect_admin_exhausted.load(Ordering::Acquire)
        && !metrics.effect_epoch_exhausted.load(Ordering::Acquire)
        && metrics.epoch_resets.load(Ordering::Acquire) == epoch_resets
        && metrics.effect_admin_revision.load(Ordering::Acquire) == admin_revision
        && metrics.requested_epoch.load(Ordering::Acquire) == epoch
        && metrics.submitted.load(Ordering::Acquire) == submitted
        && worker.valid.load(Ordering::Acquire)
        && !worker.terminated.load(Ordering::Acquire)
        && !worker.publication.exhausted.load(Ordering::Acquire)
        && worker.reset_completed_epoch.load(Ordering::Acquire) == epoch
        && worker.consumed.load(Ordering::Acquire) == submitted
        && worker.terminal.load(Ordering::Acquire) == submitted
        && parameter_admission.load(Ordering::Acquire) == 0
        && !metrics.stopped.load(Ordering::Acquire)
        && !metrics.midi_blocked.load(Ordering::Acquire)
        && metrics.faults.load(Ordering::Acquire) == 0
        && !(metrics.output_overflows.load(Ordering::Acquire) != 0
            && metrics.output_loss_epoch.load(Ordering::Acquire) == epoch)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn capture_effect_chain_on_worker(
    fence: &BounceCaptureFence,
    expected_epoch: u64,
    active_epoch: u64,
    requested_epoch: &AtomicU64,
    input: &Consumer<StereoBlock>,
    endpoint: PluginEndpointSnapshot,
    requests: &[EffectCaptureSlotRequest],
    submitted: u64,
    admin_revision: u64,
    epoch_resets: u64,
    parameter_admission: &AtomicU32,
    slots: &mut [WorkerSlot],
    events: &mut Producer<RuntimeEvent>,
    metrics: &BridgeMetrics,
) -> Result<EffectChainStateReceipt, BounceCaptureError> {
    let reject = |s: &str| BounceCaptureError::Rejected(s.into());
    let current = || {
        expected_epoch != 0
            && active_epoch == expected_epoch
            && requested_epoch.load(Ordering::Acquire) == expected_epoch
            && input.is_empty()
            && effect_tickets_current(
                metrics,
                expected_epoch,
                submitted,
                admin_revision,
                epoch_resets,
                parameter_admission,
                1,
            )
            && metrics
                .plugin_latency_snapshot()
                .is_some_and(|latency| endpoint.latency() == latency)
            && metrics
                .native_editors
                .lock()
                .is_ok_and(|n| n[..requests.len()].iter().all(editor_closed))
    };
    fence.checkpoint()?;
    if requests.is_empty()
        || requests.len() != slots.len()
        || requests.len() != endpoint.slot_count()
        || requests.len() > MAX_CHAIN_SLOTS
        || !current()
    {
        return Err(reject(
            "Effect chain is not quiescent at the exact ordered stopped endpoint",
        ));
    }
    let mut captured = Vec::with_capacity(slots.len());
    let mut total_bytes = 0usize;
    for (index, request) in requests.iter().enumerate() {
        if !current() {
            return Err(reject("Effect source work changed during capture"));
        }
        let slot = &mut slots[index];
        if slot.fault.is_some()
            || slot.config != request.config
            || endpoint.manifest().instance_id(index) != Some(request.instance_id)
        {
            return Err(reject("Effect source slot identity/configuration changed"));
        }
        let backend = slot
            .backend
            .as_mut()
            .ok_or_else(|| reject("Effect source backend unavailable"))?;
        let result =
            catch_effect(|| backend.capture_effect_bounce_state(&request.parameter_ids, fence));
        let state = effect_result(result, index, slots, events, metrics)?;
        state
            .validate(&request.parameter_ids)
            .map_err(BounceCaptureError::Rejected)?;
        if endpoint.slot(index).is_some_and(|slot| {
            slot.active && slot.latency_samples != state.metadata.latency_samples
        }) {
            return Err(reject(
                "Effect source actual DSP latency differs from the frozen endpoint",
            ));
        }
        if state.metadata.path != request.path
            || state.metadata.requested_class_uid != request.requested_class_uid
        {
            return Err(reject(
                "Effect source actual load identity differs from the frozen slot",
            ));
        }
        total_bytes = total_bytes
            .checked_add(state.opaque_state.len())
            .ok_or_else(|| reject("Effect chain state size overflow"))?;
        if total_bytes > crate::instrument_bounce::MAX_BOUNCE_STATE_BYTES {
            return Err(reject("Effect chain state exceeds 64 MiB"));
        }
        captured.push(CapturedEffectSlot {
            request: request.clone(),
            state,
        });
    }
    // A later slot's capture must not silently invalidate an earlier source receipt.
    for (index, captured_slot) in captured.iter().enumerate() {
        fence.checkpoint()?;
        if !current() {
            return Err(reject("Effect source work changed before verification"));
        }
        let backend = slots[index]
            .backend
            .as_mut()
            .ok_or_else(|| reject("Effect source backend unavailable"))?;
        let result =
            catch_effect(|| backend.verify_effect_bounce_state(&captured_slot.state, fence));
        effect_result(result, index, slots, events, metrics)?;
    }
    fence.checkpoint()?;
    let captured_tail = captured
        .iter()
        .filter(|slot| slot.request.config.enabled && !slot.request.config.bypassed)
        .fold(0u32, |tail, slot| {
            tail.saturating_add(slot.state.metadata.tail_samples)
        });
    if captured_tail != endpoint.latency().tail_samples {
        return Err(reject(
            "Effect source actual DSP tail differs from the frozen endpoint",
        ));
    }
    if !current() {
        return Err(reject(
            "Effect source work changed before the atomic receipt",
        ));
    }
    Ok(EffectChainStateReceipt {
        epoch: expected_epoch,
        endpoint,
        slots: captured,
        submitted,
        admin_revision,
        epoch_resets,
    })
}

fn catch_effect<T>(
    operation: impl FnOnce() -> Result<T, BounceCaptureError>,
) -> Result<T, BounceCaptureError> {
    panic::catch_unwind(AssertUnwindSafe(operation)).unwrap_or_else(|payload| {
        Err(BounceCaptureError::BackendFailed(format!(
            "Effect source capture panicked: {}",
            panic_payload_message(payload.as_ref())
        )))
    })
}
fn effect_result<T>(
    result: Result<T, BounceCaptureError>,
    index: usize,
    slots: &mut [WorkerSlot],
    events: &mut Producer<RuntimeEvent>,
    metrics: &BridgeMetrics,
) -> Result<T, BounceCaptureError> {
    if let Err(BounceCaptureError::BackendFailed(message)) = &result {
        slots[index].fault = Some(message.clone());
        register_fault(index, message.clone(), events, metrics);
        update_latency_and_tail(slots, events, metrics);
    }
    result
}
