//! Four finite, receipt-owned topology transitions. Test-profile observation only.
//! No method here services a callback, polls a queue, issues a request, or retries.
use super::*;
use crate::audio::genuine_observation::{
    GenuineLifecycleKind, GenuineLifecycleObservation, GenuineLifecycleState, GenuineSegmentState,
};
use crate::genuine_app_observer::{TimelineAppState, TimelinePhase};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub(crate) enum OwnedTopologyKind {
    InitialInstall,
    ConversionRemove,
    UndoInstall,
    RedoRemove,
}
impl OwnedTopologyKind {
    pub(crate) const fn index(self) -> usize {
        match self {
            Self::InitialInstall => 0,
            Self::ConversionRemove => 1,
            Self::UndoInstall => 2,
            Self::RedoRemove => 3,
        }
    }
    pub(crate) const fn role(self) -> (usize, usize) {
        match self {
            Self::InitialInstall => (1, 0),
            Self::ConversionRemove => (3, 2),
            Self::UndoInstall => (6, 4),
            Self::RedoRemove => (7, 5),
        }
    }
    const fn installs(self) -> bool {
        matches!(self, Self::InitialInstall | Self::UndoInstall)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TopologyPumpRole {
    Strict,
    Owned(OwnedTopologyKind),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub(crate) enum OwnedTopologyPhase {
    Unarmed,
    Claimed,
    CallbackAcceptedAwaitingAppPoll,
    AppAcceptedAwaitingOwnedTimeline,
    AwaitingExactTimeline,
    Settled,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub(crate) struct TopologyBaseline {
    pub revision: u64,
    pub failures: u64,
}
impl TopologyBaseline {
    fn successor(self) -> Check<Self> {
        require(
            self.revision != u64::MAX,
            "topology qualification refuses revision wrap",
        )?;
        Ok(Self {
            revision: self.revision + 1,
            failures: self
                .failures
                .checked_add(1)
                .ok_or("topology failure counter overflow")?,
        })
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OwnedTopologyClaim {
    pub kind: OwnedTopologyKind,
    pub stage: usize,
    pub control: usize,
    pub pump: u64,
    pub next_poll: usize,
    pub engine: u64,
    pub project: u64,
    pub channel: u32,
    pub endpoint: u64,
    pub instance: u64,
    pub mixer: usize,
    pub worker: usize,
    pub previous_endpoint: Option<u64>,
    pub previous_worker: Option<usize>,
    pub stamp: Option<audio::GeneratorRemovalStamp>,
    pub old_generation: u64,
    pub old_epoch: u64,
    pub legacy_barrier: u64,
    pub old_graph: (u64, u64, u64),
    pub desired_generation_at_claim: u64,
    pub observed_fingerprint_at_claim: u64,
    pub project_fingerprint_at_claim: u64,
    pub baseline: TopologyBaseline,
    pub bridge_gaps: u64,
    pub underflow_frames: u64,
    pub retry_count: u64,
    pub stale_confirmations: u64,
    pub retired_sources_before: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub(crate) struct OwnedTimeline {
    pub generation: u64,
    pub fingerprint: u64,
    pub project_fingerprint: Option<u64>,
    pub engine: u64,
    pub project: u64,
}
#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
struct TimelineReceipts {
    install_request: Option<u64>,
    loop_request: Option<u64>,
    seek_request: Option<u64>,
    seek_serial: Option<u64>,
    minimum_epoch: Option<u64>,
    activation_request: Option<u64>,
    loop_token: Option<u64>,
    installed: bool,
    loop_installed: bool,
    chase_installed: bool,
    activation_applied: bool,
    committed_epoch: Option<u64>,
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct OwnedTopologySlot {
    pub phase: OwnedTopologyPhase,
    pub claim: Option<OwnedTopologyClaim>,
    pub receipt: Option<GenuineLifecycleObservation>,
    pub timeline: Option<OwnedTimeline>,
    pub consumed_poll: Option<usize>,
    requests: TimelineReceipts,
}
impl Default for OwnedTopologySlot {
    fn default() -> Self {
        Self {
            phase: OwnedTopologyPhase::Unarmed,
            claim: None,
            receipt: None,
            timeline: None,
            consumed_poll: None,
            requests: TimelineReceipts::default(),
        }
    }
}
#[derive(Debug)]
pub(crate) struct OwnedTopologyLedger {
    run: [u8; 32],
    failed: std::cell::Cell<bool>,
    pub slots: [OwnedTopologySlot; 4],
    settled: usize,
    observed_receipts: u64,
    baseline: TopologyBaseline,
}
#[derive(Clone, Copy, Debug, serde::Serialize)]
pub(crate) struct TopologySettlement {
    pub generation: u64,
    pub epoch: u64,
    pub exact_paused: bool,
    pub source_current: bool,
    pub owner_complete: bool,
    pub worker_joined: bool,
    pub asset_receipt_exact: bool,
    pub history_exact: bool,
    pub paused_work_quiescent: bool,
    pub ordinary_health_clear: bool,
}
fn bind(slot: &mut Option<u64>, value: u64, label: &str) -> Check {
    require(value != 0 && value != u64::MAX, label)?;
    if let Some(old) = slot {
        require(*old == value, label)
    } else {
        *slot = Some(value);
        Ok(())
    }
}
fn snapshot_resync(snapshot: &GenuineCallbackSnapshot) -> bool {
    snapshot
        .timeline_stats
        .is_some_and(|stats| stats.ownership_needs_resync)
}
fn terminal_phase(phase: TimelinePhase) -> bool {
    matches!(
        phase,
        TimelinePhase::Faulted
            | TimelinePhase::FaultClearReady { .. }
            | TimelinePhase::FaultClearQueued { .. }
    )
}
impl OwnedTopologyLedger {
    pub(crate) fn new(run: [u8; 32]) -> Self {
        Self {
            run,
            failed: std::cell::Cell::new(false),
            slots: [OwnedTopologySlot::default(); 4],
            settled: 0,
            observed_receipts: 0,
            baseline: TopologyBaseline {
                revision: 0,
                failures: 0,
            },
        }
    }
    pub(crate) fn open(&self) -> Option<&OwnedTopologySlot> {
        self.slots
            .get(self.settled)
            .filter(|s| s.phase != OwnedTopologyPhase::Unarmed)
    }
    fn open_mut(&mut self) -> Check<&mut OwnedTopologySlot> {
        self.slots
            .get_mut(self.settled)
            .filter(|s| s.phase != OwnedTopologyPhase::Unarmed)
            .ok_or_else(|| "no exact owned topology operation".into())
    }
    pub(crate) fn clear(&self) -> bool {
        !self.failed.get() && self.open().is_none()
    }
    pub(crate) fn complete(&self) -> bool {
        self.settled == 4 && self.clear()
    }
    pub(crate) fn settled_baseline(&self) -> Check<TopologyBaseline> {
        require(
            self.clear(),
            "fixed window/readiness has unresolved topology",
        )?;
        Ok(self.baseline)
    }
    fn expected(&self) -> Check<TopologyBaseline> {
        if self.open().is_some_and(|s| s.receipt.is_some()) {
            self.baseline.successor()
        } else {
            Ok(self.baseline)
        }
    }
    fn claim_inner(&mut self, c: OwnedTopologyClaim) -> Check {
        if let Some(slot) = self.open() {
            require(
                slot.phase == OwnedTopologyPhase::Claimed,
                "accepted topology operation cannot be rearmed",
            )?;
            let mut previous = slot.claim.ok_or("deferred claim absent")?;
            require(
                c.pump > previous.pump && c.next_poll > previous.next_poll,
                "deferred claim did not advance existing pump/poll",
            )?;
            previous.pump = c.pump;
            previous.next_poll = c.next_poll;
            // Initial request0 State may update opaque/source model data while
            // the real queued install waits. It cannot change callback/owner
            // authority; the exact ordinary startup receipt is checked separately.
            if c.kind == OwnedTopologyKind::InitialInstall {
                previous.project_fingerprint_at_claim = c.project_fingerprint_at_claim;
                previous.observed_fingerprint_at_claim = c.observed_fingerprint_at_claim;
                previous.desired_generation_at_claim = c.desired_generation_at_claim;
            }
            require(
                previous == c,
                "deferred command owner, exact old callback or desired obligation changed",
            )?;
            self.open_mut()?.claim = Some(c);
            return Ok(());
        }
        require(
            c.kind.index() == self.settled && self.settled < 4,
            "crossed, duplicate or fifth topology operation",
        )?;
        require(
            (c.stage, c.control) == c.kind.role(),
            "topology claim wrong stage/control role",
        )?;
        require(
            c.engine != 0
                && c.project != 0
                && c.endpoint != 0
                && c.instance != 0
                && c.worker != 0
                && c.mixer == 1
                && c.old_generation != 0
                && c.old_generation != u64::MAX
                && c.old_epoch != 0
                && c.old_epoch != u64::MAX
                && c.pump != 0
                && c.next_poll != 0,
            "topology claim invalid identity domain",
        )?;
        require(
            c.baseline == self.baseline,
            "unaccounted pre-operation counter change",
        )?;
        require(
            c.retired_sources_before == 0,
            "unattributable preexisting retired source",
        )?;
        require(
            c.retry_count == 0 && c.stale_confirmations == 0,
            "preexisting Timeline rejection/retry/stale evidence",
        )?;
        c.baseline.successor()?;
        require(
            c.old_graph.0 == c.old_generation && c.old_graph.1 == c.old_epoch,
            "claim old callback graph is not exact paused identity",
        )?;
        require(
            c.kind != OwnedTopologyKind::UndoInstall
                || (c.previous_endpoint.is_some_and(|p| p != c.endpoint)
                    && c.previous_worker.is_some_and(|p| p != c.worker)),
            "Undo owner is not fresh",
        )?;
        require(
            c.kind != OwnedTopologyKind::RedoRemove
                || c.previous_endpoint == Some(c.endpoint) && c.previous_worker == Some(c.worker),
            "Redo must retain the exact Undo endpoint and worker",
        )?;
        if c.kind == OwnedTopologyKind::ConversionRemove {
            require(
                c.stamp.is_some_and(|s| {
                    s.operation_id != 0
                        && s.engine_session == c.engine
                        && s.project_session == c.project
                        && s.channel_id == c.channel
                        && s.endpoint_id == c.endpoint
                        && s.plugin_instance_id == c.instance
                        && s.stopped_epoch == c.old_epoch
                }),
                "exact sealed and queued conversion stamp absent",
            )?;
        } else {
            require(
                c.stamp.is_none(),
                "ordinary lifecycle cannot borrow removal stamp",
            )?;
        }
        self.slots[self.settled] = OwnedTopologySlot {
            phase: OwnedTopologyPhase::Claimed,
            claim: Some(c),
            ..OwnedTopologySlot::default()
        };
        Ok(())
    }
    fn callback_receipts_inner(
        &mut self,
        receipts: &[GenuineLifecycleObservation],
        pump: u64,
        next_poll: usize,
    ) -> Check {
        if receipts.is_empty() {
            return Ok(());
        }
        require(
            receipts.len() == 1,
            "unexpected lifecycle cardinality in callback",
        )?;
        let r = receipts[0];
        require(
            r.run_binding == self.run
                && r.record_ordinal == self.observed_receipts
                && r.pump_ordinal == pump
                && r.producer_present
                && r.enqueue_success,
            "lifecycle receipt run/ordinal/pump or actual enqueue failed",
        )?;
        let s = self.open().ok_or("unsolicited lifecycle receipt")?;
        require(
            s.phase == OwnedTopologyPhase::Claimed,
            "duplicate or second topology mutation",
        )?;
        let c = s.claim.ok_or("claimed topology owner missing")?;
        require(
            c.pump == pump && c.next_poll == next_poll,
            "receipt crossed actual pump/next existing poll",
        )?;
        let pre = r
            .pre
            .ok_or("receipt lacks immediate accepted pre-mutation state")?;
        let post = r.post;
        require(
            r.replaced.is_none()
                && r.affected.is_some_and(|e| {
                    e.channel_id == c.channel
                        && e.endpoint_id == c.endpoint
                        && e.plugin_instance_id == c.instance
                        && e.mixer_track == c.mixer
                        && e.project_session == c.project
                        && e.worker_id == c.worker
                        && e.epoch == c.old_epoch
                }),
            "lifecycle affected/replacement identity mismatch",
        )?;
        require(
            match (c.kind, r.kind, r.event) {
                (
                    OwnedTopologyKind::InitialInstall | OwnedTopologyKind::UndoInstall,
                    GenuineLifecycleKind::Install,
                    audio::GeneratorEndpointEvent::Installed {
                        channel_id,
                        endpoint_id,
                        plugin_instance_id,
                        mixer_track,
                        replaced_endpoint_id: None,
                        replaced_plugin_instance_id: None,
                        success: true,
                    },
                ) => {
                    channel_id == c.channel
                        && endpoint_id == c.endpoint
                        && plugin_instance_id == c.instance
                        && mixer_track == c.mixer
                }
                (
                    OwnedTopologyKind::ConversionRemove,
                    GenuineLifecycleKind::StampedRemove,
                    audio::GeneratorEndpointEvent::StampedRemoved {
                        stamp,
                        result: Ok(()),
                    },
                ) => Some(stamp) == c.stamp,
                (
                    OwnedTopologyKind::RedoRemove,
                    GenuineLifecycleKind::Remove,
                    audio::GeneratorEndpointEvent::Removed {
                        channel_id,
                        endpoint_id: Some(endpoint),
                        plugin_instance_id: Some(instance),
                    },
                ) => channel_id == c.channel && endpoint == c.endpoint && instance == c.instance,
                _ => false,
            },
            "wrong, rejected, missing-target or unowned lifecycle event",
        )?;
        for p in [pre, post] {
            require(
                p.engine_session == c.engine
                    && p.project_session == c.project
                    && p.transport_epoch == c.old_epoch
                    && p.legacy_request_barrier == c.legacy_barrier
                    && !p.playing
                    && p.generation == Some(c.old_generation)
                    && p.active_epoch == Some(c.old_epoch)
                    && p.graph_identity == Some(c.old_graph)
                    && p.graph_was_activated
                    && p.fault.is_none()
                    && p.insert_endpoints == 0
                    && p.active_plugin_automation_bindings == 0
                    && p.candidate_plugin_automation_bindings == 0
                    && p.compiled_midi_routes == 0,
                "same-operation callback identity/fault/zero-route premise failed",
            )?;
        }
        require(
            pre.plugin_topology_revision == c.baseline.revision
                && pre.timeline_execution_failures == c.baseline.failures
                && !pre.plugin_topology_replan_pending
                && !pre.needs_resync
                && !pre.suspended
                && pre.graph_binding_exact
                && pre.channel_binding_revision == Some(c.old_generation)
                && pre.channel_binding_epoch == Some(c.old_epoch),
            "pre-mutation callback is not exact settled paused baseline",
        )?;
        let after = c.baseline.successor()?;
        require(
            post.plugin_topology_revision == after.revision
                && post.timeline_execution_failures == after.failures
                && post.plugin_topology_replan_pending
                && post.needs_resync
                && post.suspended
                && post.channel_binding_revision.is_none()
                && post.channel_binding_epoch.is_none()
                && post.pdc_revision == pre.pdc_revision
                && pre.generator_endpoints == usize::from(!c.kind.installs())
                && post.generator_endpoints == usize::from(c.kind.installs()),
            "owned topology mutation not exact +1 revision/+1 failure/endpoint change",
        )?;
        self.observed_receipts += 1;
        let slot = self.open_mut()?;
        slot.receipt = Some(r);
        slot.phase = OwnedTopologyPhase::CallbackAcceptedAwaitingAppPoll;
        Ok(())
    }
    fn consume_app_poll_inner(
        &mut self,
        poll: usize,
        events: &[audio::GeneratorEndpointEvent],
        owner_transfer_exact: bool,
    ) -> Check {
        let Some(slot) = self.open() else {
            return require(events.is_empty(), "unowned App lifecycle event");
        };
        if slot.phase != OwnedTopologyPhase::CallbackAcceptedAwaitingAppPoll {
            return require(events.is_empty(), "extra App lifecycle event");
        }
        let claim = slot.claim.ok_or("poll claim missing")?;
        require(
            poll == claim.next_poll
                && events.len() == 1
                && Some(events[0]) == slot.receipt.map(|r| r.event)
                && owner_transfer_exact,
            "exact lifecycle was not consumed once with owner transfer at next scheduled poll",
        )?;
        let slot = self.open_mut()?;
        slot.consumed_poll = Some(poll);
        slot.phase = OwnedTopologyPhase::AppAcceptedAwaitingOwnedTimeline;
        Ok(())
    }
    fn observe_timeline_owner_inner(
        &mut self,
        issued: Option<OwnedTimeline>,
        owner_live: bool,
    ) -> Check {
        let Some(slot) = self.open() else {
            return Ok(());
        };
        if matches!(
            slot.phase,
            OwnedTopologyPhase::Claimed | OwnedTopologyPhase::CallbackAcceptedAwaitingAppPoll
        ) {
            return Ok(());
        }
        require(
            owner_live,
            "accepted lifecycle lost its exact live successor obligation",
        )?;
        let claim = slot.claim.ok_or("Timeline claim missing")?;
        if let Some(bound) = slot.timeline {
            require(
                issued == Some(bound),
                "owned successor generation/fingerprint missing, replaced or superseded",
            )?;
        }
        if let Some(issued) = issued {
            require(
                issued.engine == claim.engine
                    && issued.project == claim.project
                    && issued.generation > claim.old_generation
                    && issued.generation != u64::MAX,
                "successor not actual newer owner-issued generation",
            )?;
            let slot = self.open_mut()?;
            slot.timeline = Some(issued);
            slot.phase = OwnedTopologyPhase::AwaitingExactTimeline;
        }
        Ok(())
    }
    fn observe_issued_inner(&mut self, state: TimelineAppState) -> Check {
        require(
            !terminal_phase(state.phase)
                && !state.retry_required
                && state.retry_pending.is_none()
                && state.retry_count == 0
                && state.stale_confirmations == 0,
            "terminal App Timeline fault or retry",
        )?;
        let Some(slot) = self.open_mut().ok() else {
            return Ok(());
        };
        let c = slot.claim.ok_or("issued request claim missing")?;
        require(
            state.engine_session == Some(c.engine)
                && state.project_session == c.project
                && state.retry_count == c.retry_count
                && state.stale_confirmations == c.stale_confirmations,
            "Timeline session, rejected/stale/retry drift",
        )?;
        let Some(timeline) = slot.timeline else {
            return Ok(());
        };
        require(
            state.desired_generation == timeline.generation
                && state.observed_fingerprint == timeline.fingerprint,
            "issued request lost exact desired source fingerprint",
        )?;
        if matches!(
            state.phase,
            TimelinePhase::InstallQueued { .. }
                | TimelinePhase::LoopQueued { .. }
                | TimelinePhase::SeekQueued { .. }
                | TimelinePhase::AwaitingActivation { .. }
        ) {
            require(
                state.source_generation == Some(timeline.generation)
                    && state.source_fingerprint == Some(timeline.fingerprint)
                    && state.compiled_generation == Some(timeline.generation)
                    && state.compiled_fingerprint == Some(timeline.fingerprint),
                "issued request lacks exact actual compiled/source obligation",
            )?;
        }
        let r = &mut slot.requests;
        match state.phase {
            TimelinePhase::InstallQueued {
                request_id,
                generation,
            } => {
                require(
                    generation == timeline.generation,
                    "foreign Timeline install generation",
                )?;
                bind(
                    &mut r.install_request,
                    request_id,
                    "changed/zero Timeline install request",
                )?;
            }
            TimelinePhase::LoopQueued {
                request_id,
                generation,
                frame,
            } => {
                require(
                    generation == timeline.generation
                        && frame == 0
                        && r.installed
                        && r.install_request
                            .is_some_and(|previous| request_id > previous),
                    "crossed loop request",
                )?;
                bind(
                    &mut r.loop_request,
                    request_id,
                    "changed/zero Timeline loop request",
                )?;
            }
            TimelinePhase::SeekQueued {
                request_id,
                generation,
                seek_serial,
                epoch,
                frame,
            } => {
                require(
                    generation == timeline.generation
                        && frame == 0
                        && r.loop_installed
                        && epoch > c.old_epoch
                        && r.loop_request.is_some_and(|previous| request_id > previous),
                    "crossed seek request",
                )?;
                bind(
                    &mut r.seek_request,
                    request_id,
                    "changed/zero Timeline seek request",
                )?;
                bind(&mut r.seek_serial, seek_serial, "changed/zero seek serial")?;
                bind(
                    &mut r.minimum_epoch,
                    epoch,
                    "changed/zero issued minimum epoch",
                )?;
            }
            TimelinePhase::AwaitingActivation {
                generation,
                seek_serial,
                epoch,
                frame,
                activation_request_id,
            } => {
                require(
                    generation == timeline.generation && frame == 0 && r.chase_installed,
                    "crossed activation request",
                )?;
                bind(
                    &mut r.seek_serial,
                    seek_serial,
                    "activation seek serial changed",
                )?;
                bind(
                    &mut r.minimum_epoch,
                    epoch,
                    "activation issued minimum epoch changed",
                )?;
                if activation_request_id != 0 {
                    require(
                        r.seek_request
                            .is_some_and(|previous| activation_request_id > previous),
                        "activation request wrapped or crossed seek request",
                    )?;
                    bind(
                        &mut r.activation_request,
                        activation_request_id,
                        "activation request changed",
                    )?;
                }
            }
            TimelinePhase::Active { generation, epoch } => {
                require(
                    generation == c.old_generation && epoch == c.old_epoch
                        || r.activation_applied
                            && generation == timeline.generation
                            && Some(epoch) == r.committed_epoch,
                    "unobserved or foreign Active state",
                )?;
            }
            _ => {}
        }
        Ok(())
    }
    fn timeline_event_inner(
        &mut self,
        event: TimelineRuntimeEvent,
        before: TimelineAppState,
        after: TimelineAppState,
    ) -> Check {
        require(
            !matches!(
                event,
                TimelineRuntimeEvent::Rejected { .. }
                    | TimelineRuntimeEvent::TransportActivationRejected { .. }
                    | TimelineRuntimeEvent::ShutdownComplete { .. }
            ),
            "actual Timeline rejection/shutdown is terminal",
        )?;
        require(
            !terminal_phase(before.phase)
                && !terminal_phase(after.phase)
                && before.retry_count == after.retry_count
                && before.stale_confirmations == after.stale_confirmations
                && !after.retry_required
                && before.retry_pending.is_none()
                && after.retry_pending.is_none(),
            "actual Timeline event caused fault/retry/stale confirmation",
        )?;
        if self.clear() {
            return Ok(());
        }
        self.observe_issued(before)?;
        let slot = self.open_mut()?;
        let c = slot.claim.ok_or("Timeline receipt claim missing")?;
        let target = slot
            .timeline
            .ok_or("Timeline event arrived before exact owner-issued successor")?;
        require(
            event.revision() == target.generation && event.request_id() != 0,
            "foreign Timeline event generation/request",
        )?;
        let r = &mut slot.requests;
        match event {
            TimelineRuntimeEvent::Installed { request_id, .. } => {
                require(
                    !r.installed
                        && r.install_request == Some(request_id)
                        && matches!(before.phase,TimelinePhase::InstallQueued {request_id:id,generation} if id == request_id && generation == target.generation),
                    "install receipt not actual exact issued request",
                )?;
                r.installed = true;
            }
            TimelineRuntimeEvent::LoopChaseInstalled {
                request_id,
                frame,
                token,
                ..
            } => {
                require(
                    r.installed
                        && !r.loop_installed
                        && r.loop_request == Some(request_id)
                        && frame == 0
                        && after.loop_token == Some(token),
                    "loop receipt/request/token mismatch",
                )?;
                bind(&mut r.loop_token, token, "zero/changed loop token")?;
                r.loop_installed = true;
            }
            TimelineRuntimeEvent::ChaseInstalled {
                request_id,
                epoch,
                frame,
                ..
            } => {
                require(
                    r.loop_installed
                        && !r.chase_installed
                        && r.seek_request == Some(request_id)
                        && r.minimum_epoch == Some(epoch)
                        && frame == 0,
                    "chase receipt/request/minimum mismatch",
                )?;
                r.chase_installed = true;
            }
            TimelineRuntimeEvent::TransportActivationApplied {
                request_id,
                epoch,
                frame,
                ..
            } => {
                require(
                    r.chase_installed
                        && !r.activation_applied
                        && r.activation_request == Some(request_id)
                        && frame == 0
                        && epoch > c.old_epoch
                        && epoch != u64::MAX
                        && r.minimum_epoch.is_some_and(|minimum| {
                            timeline_epoch_has_reached(epoch, minimum)
                                && epoch >= minimum
                                && epoch - minimum < (1u64 << 63)
                        })
                        && after.phase
                            == (TimelinePhase::Active {
                                generation: target.generation,
                                epoch,
                            })
                        && after.confirmed_identity == Some((target.generation, epoch))
                        && after.loop_token == r.loop_token,
                    "activation receipt/request/committed epoch mismatch",
                )?;
                r.activation_applied = true;
                r.committed_epoch = Some(epoch);
            }
            _ => return Err("unexpected Timeline lifecycle within topology barrier".into()),
        }
        self.observe_issued(after)
    }
    fn snapshot_inner(&self, snapshot: &GenuineCallbackSnapshot) -> Check {
        let expected = self.expected()?;
        require(
            snapshot.run_binding == Some(self.run)
                && snapshot.plugin_topology_revision == expected.revision
                && snapshot.timeline_execution_failures == expected.failures
                && !snapshot.lifecycle_overflowed
                && !snapshot.lifecycle_inconsistent_pair
                && snapshot.lifecycle_observed_count == self.observed_receipts
                && snapshot.fault.is_none()
                && snapshot.timeline_missing_assets == 0,
            "callback counter/topology/lifecycle drift or independent fault",
        )?;
        if let Some(stats) = snapshot.timeline_stats {
            require(
                stats.command_queue_full == 0
                    && stats.event_backpressure == 0
                    && stats.retire_backpressure == 0
                    && stats.rejected_requests == 0
                    && stats.unexpected_realtime_drops == 0,
                "independent Timeline queue/rejection/ownership fault",
            )?;
        }
        if let Some(slot) = self.open() {
            let c = slot.claim.ok_or("snapshot claim missing")?;
            require(
                snapshot.engine_session == c.engine
                    && snapshot.project_session == c.project
                    && !snapshot.playing
                    && snapshot.request_id == c.legacy_barrier
                    && snapshot.fixed_quantum_bridge_gaps == c.bridge_gaps
                    && snapshot.fixed_quantum_output_underflow_frames == c.underflow_frames,
                "owned barrier session/transport/bridge drift",
            )?;
            let has_endpoint = if slot.receipt.is_some() {
                c.kind.installs()
            } else {
                !c.kind.installs()
            };
            require(
                snapshot.endpoints.len() == usize::from(has_endpoint)
                    && snapshot.endpoints.iter().all(|e| {
                        e.endpoint_id == c.endpoint
                            && e.plugin_instance_id == Some(c.instance)
                            && e.channel_id == Some(c.channel)
                            && e.mixer_track == c.mixer
                            && e.project_session == c.project
                            && e.fixed.bridge.worker_id == c.worker
                            && e.fixed.epoch == snapshot.epoch
                    }),
                "deferred/accepted exact endpoint ownership changed",
            )?;
            if snapshot.plugin_topology_replan_pending
                || snapshot.suspended
                || snapshot_resync(snapshot)
            {
                require(
                    slot.receipt.is_some()
                        && snapshot.plugin_topology_replan_pending
                        && snapshot.suspended
                        && snapshot_resync(snapshot),
                    "pending command or unrelated suspension cannot grant barrier",
                )?;
            }
        } else {
            require(
                !snapshot.plugin_topology_replan_pending
                    && !snapshot.suspended
                    && !snapshot_resync(snapshot),
                "unowned callback suspension/resync",
            )?;
        }
        Ok(())
    }
    fn segment_inner(
        &self,
        s: &GenuineSegmentObservation,
        output: &[f32],
        role: TopologyPumpRole,
    ) -> Check {
        let expected = self.expected()?;
        for state in [s.before, s.after] {
            require(
                state.plugin_topology_revision == expected.revision
                    && state.timeline_execution_failures == expected.failures
                    && state.fault.is_none(),
                "segment independent fault or unaccounted counters",
            )?;
        }
        if s.disposition == GenuineSegmentDisposition::Rendered {
            require(
                [s.before, s.after]
                    .iter()
                    .all(|p| !p.suspended && !p.needs_resync && !p.plugin_topology_replan_pending),
                "rendered segment retains fault/resync/topology pending",
            )?;
            if let Some(slot) = self.open() {
                require(
                    role == TopologyPumpRole::Owned(slot.claim.ok_or("segment claim absent")?.kind)
                        && (slot.receipt.is_some() || slot.phase == OwnedTopologyPhase::Claimed),
                    "owned barrier leaked into strict callback role",
                )?;
            }
            return Ok(());
        }
        let slot = self.open().ok_or("unowned silent callback")?;
        let c = slot.claim.ok_or("silent callback claim absent")?;
        require(
            role == TopologyPumpRole::Owned(c.kind)
                && slot.receipt.is_some()
                && s.disposition == GenuineSegmentDisposition::SuspendedSilence
                && !s.playing
                && !s.playing_after_render
                && s.start_frame == 0
                && s.generation == Some(c.old_generation)
                && s.active_epoch == Some(c.old_epoch)
                && s.epoch == c.old_epoch
                && s.request_id == c.legacy_barrier
                && output.len() == 256
                && output.iter().all(|v| v.is_finite() && *v == 0.0),
            "silent topology exception has wrong role/disposition/identity/output",
        )?;
        for state in [s.before, s.after] {
            require(
                state.plugin_topology_replan_pending
                    && state.suspended
                    && state.needs_resync
                    && state.graph_identity == Some(c.old_graph)
                    && state.active_plugin_automation_bindings == 0
                    && state.candidate_plugin_automation_bindings == 0
                    && state.compiled_midi_routes == 0
                    && state.native_voices == 0
                    && state.audio_voices == 0
                    && state.generator_notes == 0
                    && state.fixed_quantum_bridge_gaps == c.bridge_gaps
                    && state.fixed_quantum_output_underflow_frames == c.underflow_frames,
                "silent segment changed independent state",
            )?;
        }
        Ok(())
    }
    fn settle_inner(
        &mut self,
        evidence: TopologySettlement,
        snapshot: &GenuineCallbackSnapshot,
    ) -> Check<bool> {
        self.snapshot(snapshot)?;
        let Some(slot) = self.open() else {
            return Ok(false);
        };
        if slot.phase != OwnedTopologyPhase::AwaitingExactTimeline {
            return Ok(false);
        }
        let t = slot.timeline.ok_or("settlement target absent")?;
        let r = slot.requests;
        if !(r.activation_applied
            && evidence.exact_paused
            && evidence.source_current
            && evidence.owner_complete
            && evidence.worker_joined
            && evidence.asset_receipt_exact
            && evidence.history_exact
            && evidence.paused_work_quiescent
            && evidence.ordinary_health_clear)
        {
            return Ok(false);
        }
        require(
            slot.consumed_poll.is_some()
                && evidence.generation == t.generation
                && r.committed_epoch == Some(evidence.epoch)
                && snapshot.generation == Some(evidence.generation)
                && snapshot.active_epoch == Some(evidence.epoch)
                && snapshot.epoch == evidence.epoch
                && snapshot.graph_binding_exact
                && snapshot.channel_binding_revision == Some(evidence.generation)
                && snapshot.channel_binding_epoch == Some(evidence.epoch)
                && snapshot.loop_token == r.loop_token
                && !snapshot.plugin_topology_replan_pending
                && !snapshot.suspended
                && !snapshot_resync(snapshot)
                && !snapshot.playing
                && snapshot.start_frame == 0,
            "settlement not exact activation/callback/channel identity",
        )?;
        self.baseline = self.baseline.successor()?;
        self.slots[self.settled].phase = OwnedTopologyPhase::Settled;
        self.settled += 1;
        Ok(true)
    }
    pub(crate) fn evidence(&self) -> Value {
        json!({"terminal_failure":self.failed.get(),"settled_count":self.settled,"settled_baseline":self.baseline,"lifecycle_receipts":self.observed_receipts,"slots":self.slots.iter().map(|s| json!({"phase":s.phase,"kind":s.claim.map(|c|c.kind),"callback_receipt_ordinal":s.receipt.map(|r|r.record_ordinal),"callback_pump":s.receipt.map(|r|r.pump_ordinal),"expected_next_poll":s.claim.map(|c|c.next_poll),"consumed_poll":s.consumed_poll,"owner_issued_timeline":s.timeline,"issued_and_consumed":s.requests})).collect::<Vec<_>>()})
    }
}

pub(crate) fn claim_from_app(
    app: &CitrusApp,
    snapshot: &GenuineCallbackSnapshot,
    kind: OwnedTopologyKind,
    pump: u64,
    next_poll: usize,
    worker: usize,
    previous_endpoint: Option<u64>,
    previous_worker: Option<usize>,
) -> Check<Option<OwnedTopologyClaim>> {
    let channel = app
        .project
        .channels
        .first()
        .ok_or("topology source channel absent")?
        .id;
    let (endpoint, instance, mixer, project, stamp) = if kind.installs() {
        let Some(owner) = app.pending_generator_chains.get(&channel) else {
            return Ok(None);
        };
        require(
            owner.control.genuine_worker_id() == worker
                && owner.guard.genuine_worker_id() == worker,
            "pending control/guard does not own exact claimed worker",
        )?;
        require(
            app.pending_generator_chains.len() == 1 && app.running_generator_chains.is_empty(),
            "install claim not sole pending owner",
        )?;
        if kind == OwnedTopologyKind::UndoInstall {
            require(
                app.clip_restoration_playback_pending
                    && !app.clip_restoration_failed
                    && app.clip_restoration_installs.get(&owner.instance_id)
                        == Some(&(owner.endpoint_id, false)),
                "Undo not actual queued restoration installation",
            )?;
        }
        (
            owner.endpoint_id,
            owner.instance_id,
            owner.mixer_track,
            owner.project_session,
            None,
        )
    } else {
        let stamp = if kind == OwnedTopologyKind::ConversionRemove {
            let Some(job) = app
                .clip_render_job
                .as_ref()
                .filter(|job| job.phase == ClipRenderPhase::Removing && job.removal_queued)
            else {
                return Ok(None);
            };
            let stamp = job.removal.ok_or("queued conversion has no exact stamp")?;
            let binding = job
                .binding
                .ok_or("queued conversion has no sealed live binding")?;
            require(
                binding.stamp.engine_session == stamp.engine_session
                    && binding.stamp.project_session == stamp.project_session
                    && binding.stamp.channel_id == stamp.channel_id
                    && binding.stamp.endpoint_id == stamp.endpoint_id
                    && binding.stamp.instance_id == stamp.plugin_instance_id
                    && job.history.is_some()
                    && job.commit.is_none()
                    && !job.source_removed,
                "queued conversion sealed binding changed",
            )?;
            Some(stamp)
        } else {
            let Some(pending) = app.pending_generator_removals.get(&channel) else {
                return Ok(None);
            };
            require(
                pending.project_session == app.project_session
                    && pending.endpoint_id == previous_endpoint
                    && pending.plugin_instance_id.is_some()
                    && app.clip_restoration_playback_pending,
                "Redo lacks actual exact Undo removal owner",
            )?;
            None
        };
        let owner = app
            .running_generator_chains
            .get(&channel)
            .ok_or("queued removal lost running owner")?;
        require(
            owner.control.genuine_worker_id() == worker
                && owner.guard.genuine_worker_id() == worker,
            "running control/guard does not own exact claimed worker",
        )?;
        require(
            app.running_generator_chains.len() == 1 && app.pending_generator_chains.is_empty(),
            "removal owner cardinality",
        )?;
        if kind == OwnedTopologyKind::RedoRemove {
            let pending = &app.pending_generator_removals[&channel];
            require(
                pending.endpoint_id == Some(owner.endpoint_id)
                    && pending.plugin_instance_id == Some(owner.instance_id)
                    && previous_worker == Some(worker),
                "Redo pending command does not own restored worker",
            )?;
        }
        (
            owner.endpoint_id,
            owner.instance_id,
            owner.mixer_track,
            owner.project_session,
            stamp,
        )
    };
    let audio = app
        .audio
        .as_ref()
        .ok_or("topology claim audio unavailable")?;
    // Current desired model can already differ. The old exact callback binding is
    // the mutation baseline; desired/source equality belongs to settlement only.
    let generation = snapshot
        .generation
        .ok_or("pre-mutation old generation absent")?;
    let epoch = snapshot
        .active_epoch
        .ok_or("pre-mutation old active epoch absent")?;
    require(
        snapshot.engine_session == audio.engine_session()
            && snapshot.project_session == app.project_session
            && audio.project_session() == project
            && project == app.project_session
            && snapshot.epoch == epoch
            && audio.confirmed_timeline_identity() == Some((generation, epoch))
            && !snapshot.playing
            && !app.playing
            && snapshot.start_frame == 0
            && snapshot.graph_was_activated
            && snapshot.graph_binding_exact
            && snapshot.channel_binding_revision == Some(generation)
            && snapshot.channel_binding_epoch == Some(epoch)
            && !snapshot.plugin_topology_replan_pending
            && !snapshot.suspended
            && !snapshot_resync(snapshot)
            && snapshot.fault.is_none()
            && snapshot.active_plugin_automation_bindings == 0
            && snapshot.candidate_plugin_automation_bindings == 0
            && snapshot.compiled_midi_routes == 0
            && snapshot.request_id == audio.test_transport_request_id(),
        "actual pre-pump callback is not exact paused old graph",
    )?;
    let (stage, control) = kind.role();
    Ok(Some(OwnedTopologyClaim {
        kind,
        stage,
        control,
        pump,
        next_poll,
        engine: audio.engine_session(),
        project,
        channel,
        endpoint,
        instance,
        mixer,
        worker,
        previous_endpoint,
        previous_worker,
        stamp,
        old_generation: generation,
        old_epoch: epoch,
        legacy_barrier: snapshot.request_id,
        old_graph: snapshot
            .graph_identity
            .ok_or("pre-mutation graph identity missing")?,
        desired_generation_at_claim: app.timeline_desired_generation,
        observed_fingerprint_at_claim: app.timeline_observed_fingerprint,
        project_fingerprint_at_claim: project_fingerprint(&app.project),
        baseline: TopologyBaseline {
            revision: snapshot.plugin_topology_revision,
            failures: snapshot.timeline_execution_failures,
        },
        bridge_gaps: snapshot.fixed_quantum_bridge_gaps,
        underflow_frames: snapshot.fixed_quantum_output_underflow_frames,
        retry_count: app.timeline_audio_sync.retry_count,
        stale_confirmations: app.timeline_audio_sync.stale_confirmations,
        retired_sources_before: app.retired_clip_restorations.len(),
    }))
}
pub(crate) fn backend_workers_exact(
    c: OwnedTopologyClaim,
    records: &[Arc<observe::Record>],
) -> bool {
    let phase = if matches!(
        c.kind,
        OwnedTopologyKind::InitialInstall | OwnedTopologyKind::ConversionRemove
    ) {
        InstancePhase::InitialSource
    } else {
        InstancePhase::UndoSource
    };
    records
        .iter()
        .filter_map(|record| record.instance)
        .filter(|identity| {
            identity.phase == phase && identity.source_instance_id == Some(c.instance)
        })
        .all(|identity| identity.worker_id.is_none_or(|worker| worker == c.worker))
}
fn joined(records: &[Arc<observe::Record>], worker: usize) -> bool {
    records.iter().filter(|r| matches!(r.kind,RecordKind::WorkerJoin {worker_id,outcome:observe::WorkerJoinOutcome::Joined} if worker_id == worker)).count() == 1
}
fn exact_running(app: &CitrusApp, c: OwnedTopologyClaim) -> bool {
    app.running_generator_chains.len() == 1
        && app
            .running_generator_chains
            .get(&c.channel)
            .is_some_and(|s| {
                s.project_session == c.project
                    && s.endpoint_id == c.endpoint
                    && s.instance_id == c.instance
                    && s.mixer_track == c.mixer
                    && s.control.genuine_worker_id() == c.worker
                    && s.guard.genuine_worker_id() == c.worker
            })
        && app.pending_generator_chains.is_empty()
        && app
            .project
            .channels
            .iter()
            .find(|ch| ch.id == c.channel)
            .is_some_and(|ch| ch.instrument_plugin_instance_id == Some(c.instance))
}
pub(crate) fn owner_transfer_exact(
    app: &CitrusApp,
    c: OwnedTopologyClaim,
    records: &[Arc<observe::Record>],
) -> bool {
    if !backend_workers_exact(c, records)
        || app.project_session != c.project
        || app
            .audio
            .as_ref()
            .is_none_or(|a| a.engine_session() != c.engine || a.project_session() != c.project)
    {
        return false;
    }
    match c.kind {
        OwnedTopologyKind::InitialInstall => exact_running(app, c),
        OwnedTopologyKind::UndoInstall => {
            exact_running(app, c)
                && app.clip_restoration_installs.get(&c.instance) == Some(&(c.endpoint, true))
        }
        OwnedTopologyKind::ConversionRemove => {
            app.running_generator_chains.is_empty()
                && app.clip_render_job.as_ref().is_some_and(|job| {
                    job.removal == c.stamp
                        && !job.removal_queued
                        && job.source_removed
                        && job.model_commit_started
                        && job.commit.is_some()
                        && matches!(
                            job.phase,
                            ClipRenderPhase::CommittedSourceJoining
                                | ClipRenderPhase::CommittedPlaybackPending
                                | ClipRenderPhase::Complete
                        )
                        && (job.retired_source.as_ref().is_some_and(|s| {
                            s.project_session == c.project
                                && s.endpoint_id == c.endpoint
                                && s.instance_id == c.instance
                                && s.control.genuine_worker_id() == c.worker
                                && s.guard.genuine_worker_id() == c.worker
                        }) || job.source_joined && joined(records, c.worker))
                })
        }
        OwnedTopologyKind::RedoRemove => {
            app.running_generator_chains.is_empty()
                && app.pending_generator_removals.is_empty()
                && app.clip_restoration_playback_pending
                && c.retired_sources_before == 0
                && c.previous_endpoint == Some(c.endpoint)
                && c.previous_worker == Some(c.worker)
                && ((app.retired_clip_restorations.len() == 1
                    && app.retired_clip_restorations[0].genuine_worker_id() == c.worker)
                    || app.retired_clip_restorations.is_empty() && joined(records, c.worker))
        }
    }
}
pub(crate) fn owned_timeline(
    app: &CitrusApp,
    c: OwnedTopologyClaim,
) -> Check<(Option<OwnedTimeline>, bool)> {
    require(
        app.project_session == c.project
            && app.audio.as_ref().is_some_and(|a| {
                a.engine_session() == c.engine && a.project_session() == c.project
            }),
        "owner-issued Timeline session changed",
    )?;
    let (generation, live) = match c.kind {
        OwnedTopologyKind::InitialInstall => (
            app.timeline_audio_source
                .as_ref()
                .filter(|source| {
                    source.generation > c.old_generation
                        && source.generation == app.timeline_desired_generation
                        && source.fingerprint == app.timeline_observed_fingerprint
                        && app.timeline_compiled_generation == Some(source.generation)
                        && app.timeline_compiled_fingerprint == Some(source.fingerprint)
                })
                .map(|source| source.generation),
            exact_running(app, c),
        ),
        OwnedTopologyKind::ConversionRemove => {
            let job = app
                .clip_render_job
                .as_ref()
                .ok_or("conversion successor owner disappeared")?;
            (
                job.expected_timeline_generation,
                job.removal == c.stamp
                    && job.source_removed
                    && (job.retired_source.as_ref().is_some_and(|source| {
                        source.endpoint_id == c.endpoint
                            && source.instance_id == c.instance
                            && source.project_session == c.project
                            && source.control.genuine_worker_id() == c.worker
                            && source.guard.genuine_worker_id() == c.worker
                    }) || job.source_joined && job.retired_source.is_none())
                    && job.commit.is_some()
                    && job.failure.is_none()
                    && job.secondary_failure.is_none()
                    && matches!(
                        job.phase,
                        ClipRenderPhase::CommittedSourceJoining
                            | ClipRenderPhase::CommittedPlaybackPending
                            | ClipRenderPhase::Complete
                    )
                    && job.committed_fingerprint == Some(project_fingerprint(&app.project)),
            )
        }
        OwnedTopologyKind::UndoInstall | OwnedTopologyKind::RedoRemove => {
            if let Some((engine, _)) = app.clip_restoration_timeline {
                require(engine == c.engine, "restoration Timeline engine changed")?;
            }
            (
                app.clip_restoration_timeline
                    .map(|(_, generation)| generation),
                !app.clip_restoration_failed
                    && (app.clip_restoration_playback_pending
                        || app.clip_restoration_timeline.is_some())
                    && if c.kind.installs() {
                        exact_running(app, c)
                            && app.clip_restoration_installs.get(&c.instance)
                                == Some(&(c.endpoint, true))
                    } else {
                        app.running_generator_chains.is_empty()
                            && app.pending_generator_removals.is_empty()
                            && app.retired_clip_restorations.len() <= 1
                            && app
                                .retired_clip_restorations
                                .iter()
                                .all(|guard| guard.genuine_worker_id() == c.worker)
                    },
            )
        }
    };
    let issued = generation.map(|generation| OwnedTimeline {
        generation,
        fingerprint: app.timeline_observed_fingerprint,
        project_fingerprint: (c.kind != OwnedTopologyKind::InitialInstall)
            .then(|| project_fingerprint(&app.project)),
        engine: c.engine,
        project: c.project,
    });
    if let Some(t) = issued {
        require(
            t.generation == app.timeline_desired_generation,
            "actual owner-issued generation was superseded",
        )?;
    }
    Ok((issued, live))
}
pub(crate) fn settlement_from_app(
    app: &CitrusApp,
    c: OwnedTopologyClaim,
    records: &[Arc<observe::Record>],
    s: &GenuineCallbackSnapshot,
    ordinary_health_clear: bool,
) -> TopologySettlement {
    let confirmed = app
        .audio
        .as_ref()
        .and_then(AudioEngine::confirmed_timeline_identity);
    let pair = confirmed.unwrap_or((0, 0));
    let source_current = app.timeline_audio_source.as_ref().is_some_and(|source| {
        source.generation == app.timeline_desired_generation
            && timeline_source_identity_is_current(
                source.generation,
                source.fingerprint,
                source.sample_rate,
                app.timeline_compiled_generation,
                app.timeline_compiled_fingerprint,
                app.timeline_observed_fingerprint,
                app.timeline_sample_rate(),
            )
            && source.timeline.midi_port_routes().is_empty()
            && if c.kind.installs() {
                source.timeline.audio_clips().is_empty()
                    && source.timeline.plugin_routes().len() == 1
            } else {
                source.timeline.plugin_routes().is_empty()
                    && source.timeline.audio_clips().len() == 1
            }
    });
    let worker_joined = c.kind.installs() || joined(records, c.worker);
    let asset_receipt_exact = match c.kind {
        OwnedTopologyKind::ConversionRemove => app.clip_render_job.as_ref().is_some_and(|job| job.source_joined && job.asset_ack == Some(true) && job.asset_operation.is_some_and(|wanted| records.iter().filter(|r| matches!(r.kind,RecordKind::AppAudioAssetEvent(AudioAssetEvent::Registered {operation,success:true,..}) if operation == wanted)).count() == 1)),
        OwnedTopologyKind::RedoRemove => app.project.audio_assets.len() == 1 && records.iter().filter(|r| r.stage == Stage::Redo && matches!(r.kind,RecordKind::AppAudioAssetEvent(AudioAssetEvent::Registered {operation,id,success:true}) if id == app.project.audio_assets[0].id && operation.engine_session == app.audio_asset_engine_session && operation.generation == app.audio_asset_generation)).count() == 1,
        _ => true,
    };
    let owner_complete = if c.kind.installs() {
        exact_running(app, c)
            && (c.kind != OwnedTopologyKind::UndoInstall
                || !app.clip_restoration_playback_pending
                    && app.clip_restoration_installs.get(&c.instance) == Some(&(c.endpoint, true)))
    } else {
        app.running_generator_chains.is_empty()
            && app.pending_generator_chains.is_empty()
            && app.pending_generator_removals.is_empty()
            && app.pending_clip_restorations.is_empty()
            && app.retired_clip_restorations.is_empty()
            && (c.kind != OwnedTopologyKind::RedoRemove || !app.clip_restoration_playback_pending)
    };
    TopologySettlement {
        generation: pair.0,
        epoch: pair.1,
        exact_paused: app.timeline_audio_matches_desired()
            && pair.0 == app.timeline_desired_generation
            && s.generation == Some(pair.0)
            && s.active_epoch == Some(pair.1)
            && s.epoch == pair.1
            && !app.playing
            && !s.playing
            && s.start_frame == 0,
        source_current,
        owner_complete,
        worker_joined,
        asset_receipt_exact,
        history_exact: if c.kind == OwnedTopologyKind::InitialInstall {
            true
        } else {
            project_fingerprint(&app.project) == project_fingerprint(&app.history_snapshot)
        },
        paused_work_quiescent: pair.0 != 0
            && pair.1 != 0
            && s.paused_work_quiescent(pair.0, pair.1),
        ordinary_health_clear,
    }
}
pub(crate) fn claim_evidence(c: OwnedTopologyClaim) -> Value {
    json!({"kind":c.kind,"stage":c.stage,"control":c.control,"pump":c.pump,"expected_next_poll":c.next_poll,"engine_session":c.engine,"project_session":c.project,"channel":c.channel,"endpoint":c.endpoint,"instance":c.instance,"mixer":c.mixer,"worker":c.worker,"previous_endpoint":c.previous_endpoint,"previous_worker":c.previous_worker,"stamp":c.stamp.map(stamp_evidence),"old_generation":c.old_generation,"old_epoch":c.old_epoch,"legacy_request_barrier":c.legacy_barrier,"old_graph_generation":c.old_graph.0,"old_graph_epoch":c.old_graph.1,"old_graph_fingerprint":c.old_graph.2,"desired_generation_at_claim":c.desired_generation_at_claim,"observed_fingerprint_at_claim":c.observed_fingerprint_at_claim,"project_fingerprint_at_claim":c.project_fingerprint_at_claim,"baseline":c.baseline,"bridge_gaps":c.bridge_gaps,"underflow_frames":c.underflow_frames,"retry_count":c.retry_count,"stale_confirmations":c.stale_confirmations,"retired_sources_before":c.retired_sources_before})
}
fn stamp_evidence(s: audio::GeneratorRemovalStamp) -> Value {
    json!({"operation_id":s.operation_id,"engine_session":s.engine_session,"project_session":s.project_session,"channel_id":s.channel_id,"endpoint_id":s.endpoint_id,"plugin_instance_id":s.plugin_instance_id,"stopped_epoch":s.stopped_epoch})
}
pub(crate) fn lifecycle_event_evidence(e: audio::GeneratorEndpointEvent) -> Value {
    match e {
        audio::GeneratorEndpointEvent::Installed {
            channel_id,
            endpoint_id,
            plugin_instance_id,
            mixer_track,
            replaced_endpoint_id,
            replaced_plugin_instance_id,
            success,
        } => {
            json!({"variant":"Installed","channel":channel_id,"endpoint":endpoint_id,"instance":plugin_instance_id,"mixer":mixer_track,"replaced_endpoint":replaced_endpoint_id,"replaced_instance":replaced_plugin_instance_id,"success":success})
        }
        audio::GeneratorEndpointEvent::StampedRemoved { stamp, result } => {
            json!({"variant":"StampedRemoved","stamp":stamp_evidence(stamp),"success":result.is_ok(),"rejection":result.err().map(|r|format!("{r:?}"))})
        }
        audio::GeneratorEndpointEvent::Removed {
            channel_id,
            endpoint_id,
            plugin_instance_id,
        } => {
            json!({"variant":"Removed","channel":channel_id,"endpoint":endpoint_id,"instance":plugin_instance_id})
        }
        audio::GeneratorEndpointEvent::Cleared {
            request_id,
            removed,
        } => json!({"variant":"Cleared","request_id":request_id,"removed":removed}),
        audio::GeneratorEndpointEvent::RouteSet {
            channel_id,
            endpoint_id,
            plugin_instance_id,
            mixer_track,
            success,
        } => {
            json!({"variant":"RouteSet","channel":channel_id,"endpoint":endpoint_id,"instance":plugin_instance_id,"mixer":mixer_track,"success":success})
        }
    }
}
fn mutation_state_evidence(s: GenuineLifecycleState) -> Value {
    json!({"engine_session":s.engine_session,"project_session":s.project_session,"transport_epoch":s.transport_epoch,"legacy_request_barrier":s.legacy_request_barrier,"playing":s.playing,"generation":s.generation,"active_epoch":s.active_epoch,"plugin_topology_revision":s.plugin_topology_revision,"plugin_topology_replan_pending":s.plugin_topology_replan_pending,"timeline_execution_failures":s.timeline_execution_failures,"fault":s.fault.map(|f|format!("{f:?}")),"needs_resync":s.needs_resync,"suspended":s.suspended,"graph_was_activated":s.graph_was_activated,"graph_binding_exact":s.graph_binding_exact,"graph_identity":s.graph_identity,"channel_binding_revision":s.channel_binding_revision,"channel_binding_epoch":s.channel_binding_epoch,"active_plugin_automation_bindings":s.active_plugin_automation_bindings,"candidate_plugin_automation_bindings":s.candidate_plugin_automation_bindings,"compiled_midi_routes":s.compiled_midi_routes,"generator_endpoints":s.generator_endpoints,"insert_endpoints":s.insert_endpoints,"pdc_revision":s.pdc_revision})
}
pub(crate) fn receipt_evidence(r: GenuineLifecycleObservation) -> Value {
    let endpoint = |e: crate::audio::genuine_observation::GenuineLifecycleEndpoint| json!({"channel":e.channel_id,"endpoint":e.endpoint_id,"instance":e.plugin_instance_id,"mixer":e.mixer_track,"project_session":e.project_session,"worker":e.worker_id,"epoch":e.epoch});
    json!({"run_binding":r.run_binding,"pump":r.pump_ordinal,"record_ordinal":r.record_ordinal,"kernel_kind":format!("{:?}",r.kind),"event":lifecycle_event_evidence(r.event),"producer_present":r.producer_present,"enqueue_success":r.enqueue_success,"affected":r.affected.map(endpoint),"replaced":r.replaced.map(endpoint),"pre":r.pre.map(mutation_state_evidence),"post":mutation_state_evidence(r.post)})
}
pub(crate) fn segment_state_evidence(s: GenuineSegmentState) -> Value {
    json!({"plugin_topology_revision":s.plugin_topology_revision,"plugin_topology_replan_pending":s.plugin_topology_replan_pending,"timeline_execution_failures":s.timeline_execution_failures,"fault":s.fault.map(|f|format!("{f:?}")),"suspended":s.suspended,"needs_resync":s.needs_resync,"graph_identity":s.graph_identity,"graph_was_activated":s.graph_was_activated,"graph_binding_exact":s.graph_binding_exact,"channel_binding_revision":s.channel_binding_revision,"channel_binding_epoch":s.channel_binding_epoch,"active_plugin_automation_bindings":s.active_plugin_automation_bindings,"candidate_plugin_automation_bindings":s.candidate_plugin_automation_bindings,"compiled_midi_routes":s.compiled_midi_routes,"generator_endpoints":s.generator_endpoints,"insert_endpoints":s.insert_endpoints,"native_voices":s.native_voices,"audio_voices":s.audio_voices,"generator_notes":s.generator_notes,"pdc_revision":s.pdc_revision,"zero_pdc":s.zero_pdc,"paused_safety_services":s.paused_safety_services,"admitted_live_edit_endpoints":s.admitted_live_edit_endpoints,"parameter_edit_admission":s.parameter_edit_admission,"metronome_enabled":s.metronome_enabled,"recording":s.recording,"midi_input_present":s.midi_input_present,"midi_recording_present":s.midi_recording_present,"master_capture_present":s.master_capture_present,"driven_automation_targets":s.driven_automation_targets,"fixed_quantum_bridge_gaps":s.fixed_quantum_bridge_gaps,"fixed_quantum_output_underflow_frames":s.fixed_quantum_output_underflow_frames,"hooks_inactive":s.hooks_inactive})
}
pub(crate) fn timeline_state_evidence(s: TimelineAppState) -> Value {
    json!({"phase":timeline_phase_evidence(s.phase),"project_session":s.project_session,"engine_session":s.engine_session,"sync_session":s.sync_session,"desired_generation":s.desired_generation,"observed_fingerprint":s.observed_fingerprint,"compiled_generation":s.compiled_generation,"compiled_fingerprint":s.compiled_fingerprint,"source_generation":s.source_generation,"source_fingerprint":s.source_fingerprint,"sync_source_generation":s.sync_source_generation,"desired_seek_serial":s.desired_seek_serial,"desired_seek_frame":s.desired_seek_frame,"loop_token":s.loop_token,"loop_disable_request":s.loop_disable_request,"retry_count":s.retry_count,"stale_confirmations":s.stale_confirmations,"confirmed_identity":s.confirmed_identity,"retry_required":s.retry_required,"retry_pending":s.retry_pending})
}
fn timeline_phase_evidence(p: TimelinePhase) -> Value {
    match p {
        TimelinePhase::InstallQueued {
            request_id,
            generation,
        } => {
            json!({"variant":"InstallQueued","timeline_request_id":request_id,"generation":generation})
        }
        TimelinePhase::LoopQueued {
            request_id,
            generation,
            frame,
        } => {
            json!({"variant":"LoopQueued","timeline_request_id":request_id,"generation":generation,"frame":frame})
        }
        TimelinePhase::SeekQueued {
            request_id,
            generation,
            seek_serial,
            epoch,
            frame,
        } => {
            json!({"variant":"SeekQueued","timeline_request_id":request_id,"generation":generation,"seek_serial":seek_serial,"minimum_epoch":epoch,"frame":frame})
        }
        TimelinePhase::AwaitingActivation {
            generation,
            seek_serial,
            epoch,
            frame,
            activation_request_id,
        } => {
            json!({"variant":"AwaitingActivation","activation_request_id":activation_request_id,"generation":generation,"seek_serial":seek_serial,"minimum_epoch":epoch,"frame":frame})
        }
        TimelinePhase::Active { generation, epoch } => {
            json!({"variant":"Active","generation":generation,"committed_epoch":epoch})
        }
        TimelinePhase::AssetHandoffClearReady { revision } => {
            json!({"variant":"AssetHandoffClearReady","revision":revision})
        }
        TimelinePhase::AssetHandoffClearQueued {
            request_id,
            revision,
        } => {
            json!({"variant":"AssetHandoffClearQueued","timeline_request_id":request_id,"revision":revision})
        }
        TimelinePhase::FaultClearReady { revision } => {
            json!({"variant":"FaultClearReady","revision":revision})
        }
        TimelinePhase::FaultClearQueued {
            request_id,
            revision,
        } => {
            json!({"variant":"FaultClearQueued","timeline_request_id":request_id,"revision":revision})
        }
        _ => json!({"variant":format!("{p:?}")}),
    }
}
pub(crate) fn timeline_event_evidence(e: TimelineRuntimeEvent) -> Value {
    let extra = match e {
        TimelineRuntimeEvent::ChaseInstalled { epoch, frame, .. } => {
            json!({"variant":"ChaseInstalled","minimum_epoch":epoch,"frame":frame})
        }
        TimelineRuntimeEvent::LoopChaseInstalled { frame, token, .. } => {
            json!({"variant":"LoopChaseInstalled","frame":frame,"loop_token":token})
        }
        TimelineRuntimeEvent::TransportActivationApplied { epoch, frame, .. } => {
            json!({"variant":"TransportActivationApplied","committed_epoch":epoch,"frame":frame})
        }
        TimelineRuntimeEvent::Rejected { reason, .. } => {
            json!({"variant":"Rejected","reason":format!("{reason:?}")})
        }
        TimelineRuntimeEvent::TransportActivationRejected { reason, .. } => {
            json!({"variant":"TransportActivationRejected","reason":format!("{reason:?}")})
        }
        TimelineRuntimeEvent::Installed { .. } => json!({"variant":"Installed"}),
        TimelineRuntimeEvent::Cleared { .. } => json!({"variant":"Cleared"}),
        TimelineRuntimeEvent::ShutdownComplete { .. } => json!({"variant":"ShutdownComplete"}),
    };
    json!({"timeline_request_id":e.request_id(),"generation":e.revision(),"event":extra})
}

#[derive(Clone, Copy, Debug)]
pub(super) enum PumpPurpose {
    Control(usize),
    Playing {
        pass: usize,
        control: usize,
        already_active: bool,
    },
    Stopped {
        pass: usize,
        control: usize,
        previous_epoch: u64,
        generation: u64,
        already_active: bool,
    },
}
impl PumpPurpose {
    pub(super) fn charge(self, segment: Option<&GenuineSegmentObservation>) -> Charge {
        match self {
            Self::Control(control) => Charge::Control(control),
            Self::Playing {
                pass,
                control,
                already_active,
            } => {
                if already_active || segment.is_some_and(|s| s.playing) {
                    Charge::Playing(pass)
                } else {
                    Charge::Control(control)
                }
            }
            Self::Stopped {
                pass,
                control,
                previous_epoch,
                generation,
                already_active,
            } => {
                if already_active
                    || segment.is_some_and(|s| {
                        s.start_frame == 0
                            && s.generation == Some(generation)
                            && s.active_epoch == Some(s.epoch)
                            && s.epoch > previous_epoch
                    })
                {
                    Charge::Stopped(pass)
                } else {
                    Charge::Control(control)
                }
            }
        }
    }
}
pub(crate) fn ordinary_health_clear_from_app(app: &CitrusApp) -> Check<bool> {
    let observed = health::observe(app);
    if observed.status.class == HealthClass::Failed {
        // Preserve this exact already-sampled classification and operand set.
        // Raw build016 discarded it and retained only the generic prefix, so
        // its specific failed operand and restoration cause remain unknown.
        // Everything below is lazy driver-only evidence, never new authority.
        const TOAST_BYTES: usize = 4 * 1024;
        fn inventory<I: ExactSizeIterator>(items: I, item: impl Fn(I::Item) -> Value) -> Value {
            let count = items.len();
            let entries = items
                .take(observe::MAX_RECORD_BATCH)
                .map(item)
                .collect::<Vec<_>>();
            json!({"count":count,"truncated":count > observe::MAX_RECORD_BATCH,"entries":entries})
        }
        let observed_at = Instant::now();
        let pending_restorations = inventory(
            app.pending_clip_restorations.iter(),
            |(channel, pending)| {
                let started_at = pending.genuine_diagnostic_started_at();
                let deadline = started_at.checked_add(Duration::from_secs(35));
                let chain_workers = pending.genuine_diagnostic_chain_workers();
                let (transferred_control, transferred_guard) =
                    pending.genuine_diagnostic_transferred_workers();
                json!({"channel":channel,"ready":pending.genuine_diagnostic_ready(),
                "chain_present":chain_workers.is_some(),
                "chain_control_worker":chain_workers.map(|workers|workers.0),
                "chain_guard_worker":chain_workers.map(|workers|workers.1),
                "transferred_control_worker":transferred_control,"transferred_guard_worker":transferred_guard,
                "started_at":format!("{started_at:?}"),"deadline":deadline.map(|value|format!("{value:?}")),
                "elapsed_ms_at_observation":observed_at.saturating_duration_since(started_at).as_millis(),
                "deadline_elapsed_at_observation":deadline.map(|value|observed_at > value)})
            },
        );
        let pending_generators = inventory(
            app.pending_generator_chains.iter(),
            |(channel, source)| {
                json!({"channel":channel,"project_session":source.project_session,
                "endpoint":source.endpoint_id,"instance":source.instance_id,"mixer_track":source.mixer_track,
                "control_worker":source.control.genuine_worker_id(),"guard_worker":source.guard.genuine_worker_id()})
            },
        );
        let running_generators = inventory(
            app.running_generator_chains.iter(),
            |(channel, source)| {
                json!({"channel":channel,"project_session":source.project_session,
                "endpoint":source.endpoint_id,"instance":source.instance_id,"mixer_track":source.mixer_track,
                "control_worker":source.control.genuine_worker_id(),"guard_worker":source.guard.genuine_worker_id()})
            },
        );
        let retired_restorations = inventory(
            app.retired_clip_restorations.iter(),
            |guard| json!({"guard_worker":guard.genuine_worker_id()}),
        );
        let install_acknowledgments = inventory(
            app.clip_restoration_installs.iter(),
            |(instance, (endpoint, ready))| json!({"instance":instance,"endpoint":endpoint,"ready":ready}),
        );
        let current_toast_observation = app.toast.as_ref().map(|(message, _)| {
            let mut end = message.len().min(TOAST_BYTES);
            while !message.is_char_boundary(end) { end -= 1; }
            json!({"label":"current_toast_observation","text":&message[..end],
                "original_bytes":message.len(),"retained_bytes":end,"truncated":end != message.len()})
        });
        let restoration_observation = json!({
            "clip_restoration_failed":app.clip_restoration_failed,
            "clip_restoration_playback_pending":app.clip_restoration_playback_pending,
            "app_playing":app.playing,"clip_restoration_timeline":app.clip_restoration_timeline,
            "pending_restorations":pending_restorations,"pending_generators":pending_generators,
            "running_generators":running_generators,"retired_restorations":retired_restorations,
            "install_acknowledgments":install_acknowledgments,
            "current_toast_observation":current_toast_observation,
            "bounds":{"inventory_items":observe::MAX_RECORD_BATCH,"toast_bytes":TOAST_BYTES}
        });
        return Err(format!(
            "ordinary App health failed during owned topology: {}",
            json!({"health":observed.evidence,"restoration_observation":restoration_observation})
        ));
    }
    Ok(observed.status.class == HealthClass::Clear)
}
impl GenuineFixture {
    pub(super) fn prepare_owned_topology_pump(
        &mut self,
        purpose: PumpPurpose,
    ) -> Check<TopologyPumpRole> {
        let kind = match (self.active_stage, purpose) {
            (Some(1), PumpPurpose::Control(0)) => Some(OwnedTopologyKind::InitialInstall),
            (Some(3), PumpPurpose::Control(2)) => Some(OwnedTopologyKind::ConversionRemove),
            (Some(6), PumpPurpose::Control(4)) => Some(OwnedTopologyKind::UndoInstall),
            (Some(7), PumpPurpose::Control(5)) => Some(OwnedTopologyKind::RedoRemove),
            _ => None,
        };
        if let Some(slot) = self.topology.open() {
            let c = slot.claim.ok_or("open topology claim absent")?;
            require(
                kind == Some(c.kind),
                "owned topology barrier escaped declared stage/control role",
            )?;
            if slot.phase != OwnedTopologyPhase::Claimed {
                return Ok(TopologyPumpRole::Owned(c.kind));
            }
        }
        let Some(kind) = kind else {
            return Ok(TopologyPumpRole::Strict);
        };
        let channel = self.app().project.channels.first().map(|c| c.id);
        let actual_owner = channel.and_then(|channel| {
            if kind.installs() {
                self.app().pending_generator_chains.get(&channel).map(|s| {
                    (
                        s.instance_id,
                        s.control.genuine_worker_id(),
                        s.guard.genuine_worker_id(),
                    )
                })
            } else {
                self.app().running_generator_chains.get(&channel).map(|s| {
                    (
                        s.instance_id,
                        s.control.genuine_worker_id(),
                        s.guard.genuine_worker_id(),
                    )
                })
            }
        });
        let Some((instance, worker, guard_worker)) = actual_owner else {
            require(
                self.topology.clear(),
                "deferred queued topology owner disappeared",
            )?;
            return Ok(TopologyPumpRole::Strict);
        };
        require(
            worker != 0 && worker == guard_worker,
            "pre-pump control and retained guard worker identity differ",
        )?;
        let phase = if matches!(
            kind,
            OwnedTopologyKind::InitialInstall | OwnedTopologyKind::ConversionRemove
        ) {
            InstancePhase::InitialSource
        } else {
            InstancePhase::UndoSource
        };
        let records = self.all_observations();
        for identity in records
            .iter()
            .filter_map(|r| r.instance)
            .filter(|i| i.phase == phase && i.source_instance_id == Some(instance))
        {
            if let Some(id) = identity.worker_id {
                require(worker == id, "pre-pump source worker identity changed")?;
            }
        }
        // No source lifecycle is queued during initial mixer, capture, or pure
        // controller waits; those retain the strict ordinary callback role.
        let candidate = claim_from_app(
            self.app(),
            &self.snapshot(),
            kind,
            self.budget.total as u64 + 1,
            self.budget.polls + 1,
            worker,
            if kind == OwnedTopologyKind::RedoRemove {
                self.undo_endpoint
            } else {
                self.original_endpoint
            },
            if kind == OwnedTopologyKind::RedoRemove {
                self.undo_worker
            } else {
                self.original_worker
            },
        )?;
        if let Some(c) = candidate {
            self.journal
                .emit("owned_topology_pre_pump_claim", claim_evidence(c))?;
            self.topology.claim(c)?;
            Ok(TopologyPumpRole::Owned(kind))
        } else {
            require(
                self.topology.clear(),
                "deferred queued lifecycle command disappeared",
            )?;
            Ok(TopologyPumpRole::Strict)
        }
    }
    pub(super) fn consume_owned_topology_poll(&mut self) -> Check {
        let records = self.all_observations();
        while self.dumped_records < records.len() {
            self.persist_new_observations()?;
        }
        let events = records[self.topology_records..]
            .iter()
            .filter_map(|r| {
                if let RecordKind::AppGeneratorEndpointEvent(e) = r.kind {
                    Some(e)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        let claim = self.topology.open().and_then(|s| s.claim);
        let transfer = claim.is_some_and(|c| owner_transfer_exact(self.app(), c, &records));
        if !events.is_empty()
            || self
                .topology
                .open()
                .is_some_and(|s| s.phase == OwnedTopologyPhase::CallbackAcceptedAwaitingAppPoll)
        {
            self.journal.emit("owned_topology_app_poll",json!({"poll":self.budget.polls,"events":events.iter().copied().map(lifecycle_event_evidence).collect::<Vec<_>>(),"owner_transfer_exact":transfer,"ledger":self.topology.evidence()}))?;
        }
        self.topology
            .consume_app_poll(self.budget.polls, &events, transfer)?;
        if let Some(c) = claim {
            let (issued, live) = owned_timeline(self.app(), c)?;
            if self.topology.open().is_some_and(|s| s.timeline != issued) || !live {
                self.journal.emit("owned_topology_issuance",json!({"poll":self.budget.polls,"issued":issued,"owner_live":live,"app":timeline_state_evidence(self.app().genuine_timeline_app_state())}))?;
            }
            self.topology.observe_timeline_owner(issued, live)?;
        }
        for record in &records[self.topology_records..] {
            if let RecordKind::AppTimelineRuntimeEvent {
                event,
                before,
                after,
            } = record.kind
            {
                self.topology.timeline_event(event, before, after)?;
            }
        }
        self.topology_records = records.len();
        self.topology
            .observe_issued(self.app().genuine_timeline_app_state())?;
        Ok(())
    }
    pub(super) fn refresh_owned_topology(&mut self, ordinary_health_clear: bool) -> Check {
        let records = self.all_observations();
        for claim in self.topology.slots.iter().filter_map(|slot| slot.claim) {
            require(
                backend_workers_exact(claim, &records),
                "later backend observation differs from retained App worker",
            )?;
        }
        let Some(slot) = self.topology.open() else {
            return Ok(());
        };
        let claim = slot.claim.ok_or("health topology claim absent")?;
        if matches!(
            slot.phase,
            OwnedTopologyPhase::Claimed | OwnedTopologyPhase::CallbackAcceptedAwaitingAppPoll
        ) {
            return Ok(());
        }
        let snapshot = self.snapshot();
        let (issued, live) = owned_timeline(self.app(), claim)?;
        let mut evidence = settlement_from_app(
            self.app(),
            claim,
            &records,
            &snapshot,
            ordinary_health_clear,
        );
        if claim.kind == OwnedTopologyKind::InitialInstall {
            let (_, initial, complete) = self.initial_source_readiness()?;
            evidence.owner_complete &= complete && initial["source_ready_without_topology"] == true;
        }
        evidence.history_exact &= match claim.kind {
            OwnedTopologyKind::UndoInstall => {
                self.authoritative_before.as_ref().is_some_and(|image| {
                    clip_render::project_matches_snapshot(&self.app().project, image)
                        .unwrap_or(false)
                })
            }
            OwnedTopologyKind::RedoRemove => self.committed_project.as_ref().is_some_and(|image| {
                clip_render::project_matches_snapshot(&self.app().project, image).unwrap_or(false)
            }),
            _ => true,
        };
        if claim.kind == OwnedTopologyKind::RedoRemove {
            evidence.asset_receipt_exact &= self
                .asset_id
                .and_then(|id| self.app().cached_audio_assets.get(&id))
                .zip(self.verified_pcm.as_ref())
                .is_some_and(|(cached, pcm)| Arc::ptr_eq(&cached.samples, pcm));
            let before = self.topology.slots[OwnedTopologyKind::RedoRemove.index()]
                .claim
                .ok_or("Redo immutable owner claim absent")?;
            evidence.owner_complete &= before.endpoint == self.undo_endpoint.unwrap_or(0)
                && before.worker == self.undo_worker.unwrap_or(0);
        }
        let health_evidence = json!({"owner_live":live,"issued":issued,"settlement":evidence,"callback":startup::callback_evidence(&snapshot,self.app().audio.as_ref().unwrap().test_transport_request_id()),"ledger":self.topology.evidence()});
        if self.last_topology_health_evidence.as_ref() != Some(&health_evidence) {
            self.journal
                .emit("owned_topology_health", health_evidence.clone())?;
            self.last_topology_health_evidence = Some(health_evidence);
        }
        self.topology.observe_timeline_owner(issued, live)?;
        self.topology
            .observe_issued(self.app().genuine_timeline_app_state())?;
        if self.topology.settle(evidence, &snapshot)? {
            self.journal
                .emit("owned_topology_settled", self.topology.evidence())?;
        }
        Ok(())
    }
}

impl OwnedTopologyLedger {
    pub(crate) fn claim(&mut self, c: OwnedTopologyClaim) -> Check {
        require(!self.failed.get(), "owned topology ledger already terminal")?;
        let result = self.claim_inner(c);
        if result.is_err() {
            self.failed.set(true);
        }
        result
    }
    pub(crate) fn callback_receipts(
        &mut self,
        receipts: &[GenuineLifecycleObservation],
        pump: u64,
        next_poll: usize,
    ) -> Check {
        require(!self.failed.get(), "owned topology ledger already terminal")?;
        let result = self.callback_receipts_inner(receipts, pump, next_poll);
        if result.is_err() {
            self.failed.set(true);
        }
        result
    }
    pub(crate) fn consume_app_poll(
        &mut self,
        poll: usize,
        events: &[audio::GeneratorEndpointEvent],
        owner_transfer_exact: bool,
    ) -> Check {
        require(!self.failed.get(), "owned topology ledger already terminal")?;
        let result = self.consume_app_poll_inner(poll, events, owner_transfer_exact);
        if result.is_err() {
            self.failed.set(true);
        }
        result
    }
    pub(crate) fn observe_timeline_owner(
        &mut self,
        issued: Option<OwnedTimeline>,
        owner_live: bool,
    ) -> Check {
        require(!self.failed.get(), "owned topology ledger already terminal")?;
        let result = self.observe_timeline_owner_inner(issued, owner_live);
        if result.is_err() {
            self.failed.set(true);
        }
        result
    }
    pub(crate) fn observe_issued(&mut self, state: TimelineAppState) -> Check {
        require(!self.failed.get(), "owned topology ledger already terminal")?;
        let result = self.observe_issued_inner(state);
        if result.is_err() {
            self.failed.set(true);
        }
        result
    }
    pub(crate) fn timeline_event(
        &mut self,
        event: TimelineRuntimeEvent,
        before: TimelineAppState,
        after: TimelineAppState,
    ) -> Check {
        require(!self.failed.get(), "owned topology ledger already terminal")?;
        let result = self.timeline_event_inner(event, before, after);
        if result.is_err() {
            self.failed.set(true);
        }
        result
    }
    pub(crate) fn snapshot(&self, snapshot: &GenuineCallbackSnapshot) -> Check {
        require(!self.failed.get(), "owned topology ledger already terminal")?;
        let result = self.snapshot_inner(snapshot);
        if result.is_err() {
            self.failed.set(true);
        }
        result
    }
    pub(crate) fn segment(
        &self,
        s: &GenuineSegmentObservation,
        output: &[f32],
        role: TopologyPumpRole,
    ) -> Check {
        require(!self.failed.get(), "owned topology ledger already terminal")?;
        let result = self.segment_inner(s, output, role);
        if result.is_err() {
            self.failed.set(true);
        }
        result
    }
    pub(crate) fn settle(
        &mut self,
        evidence: TopologySettlement,
        snapshot: &GenuineCallbackSnapshot,
    ) -> Check<bool> {
        require(!self.failed.get(), "owned topology ledger already terminal")?;
        let result = self.settle_inner(evidence, snapshot);
        if result.is_err() {
            self.failed.set(true);
        }
        result
    }
}

#[cfg(test)]
#[path = "clip_render_genuine_topology_tests.rs"]
mod tests;
