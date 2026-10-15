//! Optional, fixed-storage production bridge evidence. This is observation only:
//! no timestamp is read on the device callback and no diagnostic decides timing.
use std::sync::atomic::{self, AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use crate::plugin_timing::PluginProcessingFaultReason;

const READ_ATTEMPTS: usize = 3;
const STAGE_FIELDS: usize = 13;
const FAULT_FIELDS: usize = 31 + STAGE_FIELDS;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[repr(u64)]
pub enum PluginWorkerStage {
    #[default]
    NotStarted,
    Startup,
    Reset,
    NativeFeedback,
    Admin,
    Midi,
    Transport,
    /// Includes backend wrapper work and helper IPC; this is not SDK DSP time.
    BackendProcess,
    Metadata,
    OutputPush,
    IdlePark,
    Shutdown,
    /// State/revision/parameter capture, retaining the owning admin command.
    NativeStateCapture,
}

impl PluginWorkerStage {
    fn from_raw(raw: u64) -> Self {
        match raw {
            1 => Self::Startup,
            2 => Self::Reset,
            3 => Self::NativeFeedback,
            4 => Self::Admin,
            5 => Self::Midi,
            6 => Self::Transport,
            7 => Self::BackendProcess,
            8 => Self::Metadata,
            9 => Self::OutputPush,
            10 => Self::IdlePark,
            11 => Self::Shutdown,
            12 => Self::NativeStateCapture,
            _ => Self::NotStarted,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[repr(u64)]
pub enum PluginWorkerAdminCommand {
    #[default]
    None,
    Shutdown,
    CaptureInstrumentBounce,
    SaveState,
    LoadState,
    NativeEditor,
    SetSlotConfig,
    SetParameter,
    QueryParameter,
    RequestParameterPage,
    CaptureEffectChain,
}

impl PluginWorkerAdminCommand {
    fn from_raw(raw: u64) -> Self {
        match raw {
            1 => Self::Shutdown,
            2 => Self::CaptureInstrumentBounce,
            3 => Self::SaveState,
            4 => Self::LoadState,
            5 => Self::NativeEditor,
            6 => Self::SetSlotConfig,
            7 => Self::SetParameter,
            8 => Self::QueryParameter,
            9 => Self::RequestParameterPage,
            10 => Self::CaptureEffectChain,
            _ => Self::None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[repr(u64)]
pub enum PluginWorkerNativeEditorCommand {
    #[default]
    None,
    Open,
    Focus,
    Close,
}

impl PluginWorkerNativeEditorCommand {
    fn from_raw(raw: u64) -> Self {
        match raw {
            1 => Self::Open,
            2 => Self::Focus,
            3 => Self::Close,
            _ => Self::None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct PluginWorkerStageSnapshot {
    /// False means the bounded reader could not obtain a complete publication.
    pub coherent: bool,
    /// A version or relative clock overflow makes this publication non-authoritative.
    pub exhausted: bool,
    pub publication_id: u64,
    pub epoch: u64,
    /// Admin/feedback stages retain the previous input quantum's context.
    /// This is not a claim that the admin command consumed this sequence.
    pub sequence: u64,
    pub slot: Option<u8>,
    pub stage: PluginWorkerStage,
    pub active: bool,
    /// Nanoseconds relative to diagnostic construction on the control thread.
    pub begin_ns: u64,
    /// Zero while active; a blocked call publishes its begin before entering it.
    pub end_ns: u64,
    /// Retained across nested capture/metadata scopes until the command returns.
    pub admin_command: PluginWorkerAdminCommand,
    /// Zero denotes an untagged command, or a command without request identity.
    pub admin_request_id: u64,
    pub native_editor_command: PluginWorkerNativeEditorCommand,
}

impl PluginWorkerStageSnapshot {
    fn values(self) -> [u64; STAGE_FIELDS] {
        [
            self.epoch,
            self.sequence,
            self.slot.map_or(0, |slot| u64::from(slot) + 1),
            self.stage as u64,
            u64::from(self.active),
            self.begin_ns,
            self.end_ns,
            u64::from(self.coherent),
            u64::from(self.exhausted),
            self.publication_id,
            self.admin_command as u64,
            self.admin_request_id,
            self.native_editor_command as u64,
        ]
    }

    fn from_values(values: [u64; STAGE_FIELDS], publication_id: u64) -> Self {
        Self {
            epoch: values[0],
            sequence: values[1],
            slot: values[2]
                .checked_sub(1)
                .and_then(|slot| u8::try_from(slot).ok()),
            stage: PluginWorkerStage::from_raw(values[3]),
            active: values[4] != 0,
            begin_ns: values[5],
            end_ns: values[6],
            coherent: values[7] != 0,
            exhausted: values[8] != 0,
            publication_id: if publication_id == 0 {
                values[9]
            } else {
                publication_id
            },
            admin_command: PluginWorkerAdminCommand::from_raw(values[10]),
            admin_request_id: values[11],
            native_editor_command: PluginWorkerNativeEditorCommand::from_raw(values[12]),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[repr(u64)]
pub enum PluginReceiveBranch {
    #[default]
    NotAttempted,
    Empty,
    Processed,
    LatencyDrift,
    OutputTooSmall,
    FutureOutputOverflow,
}

impl PluginReceiveBranch {
    fn from_raw(raw: u64) -> Self {
        match raw {
            1 => Self::Empty,
            2 => Self::Processed,
            3 => Self::LatencyDrift,
            4 => Self::OutputTooSmall,
            5 => Self::FutureOutputOverflow,
            _ => Self::NotAttempted,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct ReceiveDecision {
    pub branch: PluginReceiveBranch,
    pub received_sequence: u64,
    pub received_frames: usize,
    pub requested_frames: usize,
    pub expected_latency_revision: u64,
}

/// First callback fault for the endpoint lifetime. Reset never erases this history.
/// Occupancy and counters are sampled at the decision, before its MIDI safety latch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct PluginFaultDecisionSnapshot {
    #[serde(serialize_with = "serialize_fault_reason")]
    pub reason: PluginProcessingFaultReason,
    pub epoch: u64,
    pub fault_sequence: u64,
    pub next_sequence: u64,
    /// Endpoint receive expectation at the fault decision. InputLoss occurs
    /// after submit advances next_sequence, so this can be the next receive
    /// target rather than the target of the preceding successful receive.
    pub expected_sequence: u64,
    pub receive_branch: PluginReceiveBranch,
    pub received_sequence: u64,
    pub received_frames: usize,
    pub requested_frames: usize,
    pub expected_latency_revision: u64,
    pub lookahead_quanta: usize,
    pub input_pending: usize,
    pub output_owned: usize,
    pub future_output_owned: usize,
    pub delayed_dry_frames: usize,
    pub delayed_dry_submitted: bool,
    pub pending_inline_events: usize,
    pub callback_sequences: u64,
    pub submitted: u64,
    pub completed: u64,
    pub input_gaps: u64,
    pub input_overflows: u64,
    pub deadline_misses: u64,
    pub output_overflows: u64,
    pub output_loss_epoch: u64,
    pub frame_mismatches: u64,
    pub stale_outputs: u64,
    pub dropped_old_epoch_outputs: u64,
    pub input_frames: u64,
    pub output_frames: u64,
    pub frame_counter_overflow: bool,
    /// A bounded worker publication read before the MIDI safety latch. Queue
    /// occupancy and counters are sampled separately, not globally atomically.
    pub worker_at_fault: PluginWorkerStageSnapshot,
}

fn serialize_fault_reason<S: serde::Serializer>(
    reason: &PluginProcessingFaultReason,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(match reason {
        PluginProcessingFaultReason::CallbackBudgetExceeded => "CallbackBudgetExceeded",
        PluginProcessingFaultReason::UnsupportedCallback => "UnsupportedCallback",
        PluginProcessingFaultReason::DeadlineMiss => "DeadlineMiss",
        PluginProcessingFaultReason::InputLoss => "InputLoss",
        PluginProcessingFaultReason::OutputLoss => "OutputLoss",
        PluginProcessingFaultReason::LatencyDrift => "LatencyDrift",
        PluginProcessingFaultReason::EventLoss => "EventLoss",
        PluginProcessingFaultReason::EndpointChanged => "EndpointChanged",
        PluginProcessingFaultReason::WorkerFault => "WorkerFault",
    })
}

impl PluginFaultDecisionSnapshot {
    fn values(self) -> [u64; FAULT_FIELDS] {
        let mut values = [0; FAULT_FIELDS];
        values[..31].copy_from_slice(&[
            self.reason as u64,
            self.epoch,
            self.fault_sequence,
            self.next_sequence,
            self.expected_sequence,
            self.receive_branch as u64,
            self.received_sequence,
            self.received_frames as u64,
            self.requested_frames as u64,
            self.expected_latency_revision,
            self.lookahead_quanta as u64,
            self.input_pending as u64,
            self.output_owned as u64,
            self.future_output_owned as u64,
            self.delayed_dry_frames as u64,
            u64::from(self.delayed_dry_submitted),
            self.pending_inline_events as u64,
            self.callback_sequences,
            self.submitted,
            self.completed,
            self.input_gaps,
            self.input_overflows,
            self.deadline_misses,
            self.output_overflows,
            self.output_loss_epoch,
            self.frame_mismatches,
            self.stale_outputs,
            self.dropped_old_epoch_outputs,
            self.input_frames,
            self.output_frames,
            u64::from(self.frame_counter_overflow),
        ]);
        values[31..].copy_from_slice(&self.worker_at_fault.values());
        values
    }

    fn from_values(values: [u64; FAULT_FIELDS]) -> Self {
        let reason = match values[0] {
            2 => PluginProcessingFaultReason::UnsupportedCallback,
            3 => PluginProcessingFaultReason::DeadlineMiss,
            4 => PluginProcessingFaultReason::InputLoss,
            5 => PluginProcessingFaultReason::OutputLoss,
            6 => PluginProcessingFaultReason::LatencyDrift,
            7 => PluginProcessingFaultReason::EventLoss,
            8 => PluginProcessingFaultReason::EndpointChanged,
            9 => PluginProcessingFaultReason::WorkerFault,
            _ => PluginProcessingFaultReason::CallbackBudgetExceeded,
        };
        let mut worker = [0; STAGE_FIELDS];
        worker.copy_from_slice(&values[31..]);
        Self {
            reason,
            epoch: values[1],
            fault_sequence: values[2],
            next_sequence: values[3],
            expected_sequence: values[4],
            receive_branch: PluginReceiveBranch::from_raw(values[5]),
            received_sequence: values[6],
            received_frames: values[7] as usize,
            requested_frames: values[8] as usize,
            expected_latency_revision: values[9],
            lookahead_quanta: values[10] as usize,
            input_pending: values[11] as usize,
            output_owned: values[12] as usize,
            future_output_owned: values[13] as usize,
            delayed_dry_frames: values[14] as usize,
            delayed_dry_submitted: values[15] != 0,
            pending_inline_events: values[16] as usize,
            callback_sequences: values[17],
            submitted: values[18],
            completed: values[19],
            input_gaps: values[20],
            input_overflows: values[21],
            deadline_misses: values[22],
            output_overflows: values[23],
            output_loss_epoch: values[24],
            frame_mismatches: values[25],
            stale_outputs: values[26],
            dropped_old_epoch_outputs: values[27],
            input_frames: values[28],
            output_frames: values[29],
            frame_counter_overflow: values[30] != 0,
            worker_at_fault: PluginWorkerStageSnapshot::from_values(worker, 0),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct PluginRuntimeDiagnosticsSnapshot {
    pub enabled: bool,
    pub worker: PluginWorkerStageSnapshot,
    pub first_fault: Option<PluginFaultDecisionSnapshot>,
    /// False is a bounded collision, not evidence that no fault occurred.
    pub first_fault_coherent: bool,
    pub first_fault_exhausted: bool,
}

struct Publication<const N: usize> {
    version: AtomicU64,
    exhausted: AtomicBool,
    values: [AtomicU64; N],
}

impl<const N: usize> Default for Publication<N> {
    fn default() -> Self {
        Self {
            version: AtomicU64::new(0),
            exhausted: AtomicBool::new(false),
            values: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
}

impl<const N: usize> Publication<N> {
    /// Exactly one writer per publication: worker stages or callback first fault.
    fn publish(&self, values: [u64; N]) {
        if self.exhausted.load(Ordering::Acquire) {
            return;
        }
        let current = self.version.load(Ordering::Relaxed);
        let Some(next) = current.checked_add(2).filter(|_| current & 1 == 0) else {
            self.exhausted.store(true, Ordering::Release);
            return;
        };
        self.version.swap(current + 1, Ordering::AcqRel);
        atomic::fence(Ordering::Release);
        for (field, value) in self.values.iter().zip(values) {
            field.store(value, Ordering::Relaxed);
        }
        self.version.store(next, Ordering::Release);
    }

    fn read(&self) -> Option<([u64; N], u64)> {
        for _ in 0..READ_ATTEMPTS {
            let before = self.version.load(Ordering::Acquire);
            if before & 1 != 0 {
                continue;
            }
            let values = std::array::from_fn(|index| self.values[index].load(Ordering::Relaxed));
            atomic::fence(Ordering::Acquire);
            if self.version.load(Ordering::Relaxed) == before {
                return Some((values, before));
            }
        }
        None
    }
}

pub(super) struct RuntimeDiagnostics {
    enabled: bool,
    origin: Option<Instant>,
    worker: Publication<STAGE_FIELDS>,
    fault: Publication<FAULT_FIELDS>,
    fault_claimed: AtomicBool,
    context_epoch: AtomicU64,
    context_sequence: AtomicU64,
    // Worker-only context. Readers receive these only through the stage publication.
    context_admin_command: AtomicU64,
    context_admin_request_id: AtomicU64,
    context_native_editor_command: AtomicU64,
    input_frames: AtomicU64,
    output_frames: AtomicU64,
    frame_counter_overflow: AtomicBool,
}

impl Default for RuntimeDiagnostics {
    fn default() -> Self {
        Self::new(false)
    }
}

impl RuntimeDiagnostics {
    pub(super) fn from_environment() -> Self {
        Self::new(
            std::env::var_os("CITRUS_STUDIO_PLUGIN_DIAGNOSTICS").is_some_and(|value| value == "1"),
        )
    }

    pub(super) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            origin: enabled.then(Instant::now),
            worker: Publication::default(),
            fault: Publication::default(),
            fault_claimed: AtomicBool::new(false),
            context_epoch: AtomicU64::new(0),
            context_sequence: AtomicU64::new(0),
            context_admin_command: AtomicU64::new(0),
            context_admin_request_id: AtomicU64::new(0),
            context_native_editor_command: AtomicU64::new(0),
            input_frames: AtomicU64::new(0),
            output_frames: AtomicU64::new(0),
            frame_counter_overflow: AtomicBool::new(false),
        }
    }

    pub(super) fn enabled(&self) -> bool {
        self.enabled
    }

    pub(super) fn context(&self, epoch: u64, sequence: u64) {
        if self.enabled {
            self.context_epoch.store(epoch, Ordering::Relaxed);
            self.context_sequence.store(sequence, Ordering::Relaxed);
        }
    }

    fn relative_ns(&self) -> u64 {
        let elapsed = self.origin.map_or(0, |origin| origin.elapsed().as_nanos());
        match u64::try_from(elapsed) {
            Ok(value) => value,
            Err(_) => {
                self.worker.exhausted.store(true, Ordering::Release);
                u64::MAX
            }
        }
    }

    pub(super) fn stage(&self, stage: PluginWorkerStage, slot: Option<usize>) -> StageScope<'_> {
        let snapshot = if self.enabled {
            let snapshot = PluginWorkerStageSnapshot {
                coherent: true,
                epoch: self.context_epoch.load(Ordering::Relaxed),
                sequence: self.context_sequence.load(Ordering::Relaxed),
                slot: slot.and_then(|slot| u8::try_from(slot).ok()),
                stage,
                active: true,
                begin_ns: self.relative_ns(),
                admin_command: PluginWorkerAdminCommand::from_raw(
                    self.context_admin_command.load(Ordering::Relaxed),
                ),
                admin_request_id: self.context_admin_request_id.load(Ordering::Relaxed),
                native_editor_command: PluginWorkerNativeEditorCommand::from_raw(
                    self.context_native_editor_command.load(Ordering::Relaxed),
                ),
                ..Default::default()
            };
            self.worker.publish(snapshot.values());
            snapshot
        } else {
            PluginWorkerStageSnapshot::default()
        };
        StageScope {
            diagnostics: self,
            snapshot,
        }
    }

    pub(super) fn admin(
        &self,
        command: PluginWorkerAdminCommand,
        slot: Option<usize>,
        request_id: u64,
        native_command: PluginWorkerNativeEditorCommand,
    ) -> AdminScope<'_> {
        let previous = if self.enabled {
            [
                self.context_admin_command
                    .swap(command as u64, Ordering::Relaxed),
                self.context_admin_request_id
                    .swap(request_id, Ordering::Relaxed),
                self.context_native_editor_command
                    .swap(native_command as u64, Ordering::Relaxed),
            ]
        } else {
            [0; 3]
        };
        AdminScope {
            diagnostics: self,
            previous,
            stage: Some(self.stage(PluginWorkerStage::Admin, slot)),
        }
    }

    fn worker_snapshot(&self) -> PluginWorkerStageSnapshot {
        match self.worker.read() {
            Some((values, version)) => {
                let mut snapshot = PluginWorkerStageSnapshot::from_values(values, version);
                snapshot.coherent = true;
                snapshot.exhausted |= self.worker.exhausted.load(Ordering::Acquire);
                snapshot
            }
            None => PluginWorkerStageSnapshot {
                exhausted: self.worker.exhausted.load(Ordering::Acquire),
                ..Default::default()
            },
        }
    }

    pub(super) fn first_fault(&self, mut decision: PluginFaultDecisionSnapshot) {
        if !self.enabled || self.fault_claimed.swap(true, Ordering::AcqRel) {
            return;
        }
        decision.worker_at_fault = self.worker_snapshot();
        decision.input_frames = self.input_frames.load(Ordering::Relaxed);
        decision.output_frames = self.output_frames.load(Ordering::Relaxed);
        decision.frame_counter_overflow = self.frame_counter_overflow.load(Ordering::Acquire);
        self.fault.publish(decision.values());
    }

    pub(super) fn count_input_frames(&self, frames: usize) {
        self.count_frames(&self.input_frames, frames);
    }

    pub(super) fn count_output_frames(&self, frames: usize) {
        self.count_frames(&self.output_frames, frames);
    }

    fn count_frames(&self, counter: &AtomicU64, frames: usize) {
        if !self.enabled {
            return;
        }
        // One writer owns each counter. Never wrap a diagnostic frame identity.
        if let Some(next) = counter.load(Ordering::Relaxed).checked_add(frames as u64) {
            counter.store(next, Ordering::Relaxed);
        } else {
            self.frame_counter_overflow.store(true, Ordering::Release);
        }
    }

    pub(super) fn snapshot(&self) -> PluginRuntimeDiagnosticsSnapshot {
        if !self.enabled {
            return PluginRuntimeDiagnosticsSnapshot {
                first_fault_coherent: true,
                ..Default::default()
            };
        }
        let fault = self.fault.read();
        let first_fault_coherent = fault.as_ref().is_some_and(|(_, version)| {
            *version != 0 || !self.fault_claimed.load(Ordering::Acquire)
        });
        let first_fault = fault.and_then(|(values, version)| {
            (version != 0).then(|| PluginFaultDecisionSnapshot::from_values(values))
        });
        PluginRuntimeDiagnosticsSnapshot {
            enabled: true,
            worker: self.worker_snapshot(),
            first_fault,
            first_fault_coherent,
            first_fault_exhausted: self.fault.exhausted.load(Ordering::Acquire),
        }
    }
}

pub(super) struct StageScope<'a> {
    diagnostics: &'a RuntimeDiagnostics,
    snapshot: PluginWorkerStageSnapshot,
}

pub(super) struct AdminScope<'a> {
    diagnostics: &'a RuntimeDiagnostics,
    previous: [u64; 3],
    stage: Option<StageScope<'a>>,
}

impl Drop for AdminScope<'_> {
    fn drop(&mut self) {
        // Publish this command's end before restoring the outer worker context.
        drop(self.stage.take());
        if self.diagnostics.enabled {
            self.diagnostics
                .context_admin_command
                .store(self.previous[0], Ordering::Relaxed);
            self.diagnostics
                .context_admin_request_id
                .store(self.previous[1], Ordering::Relaxed);
            self.diagnostics
                .context_native_editor_command
                .store(self.previous[2], Ordering::Relaxed);
        }
    }
}

impl Drop for StageScope<'_> {
    fn drop(&mut self) {
        if self.diagnostics.enabled {
            self.snapshot.active = false;
            self.snapshot.end_ns = self.diagnostics.relative_ns();
            self.diagnostics.worker.publish(self.snapshot.values());
        }
    }
}

#[cfg(test)]
#[path = "plugin_runtime_diagnostics_tests.rs"]
mod tests;
