use super::super::{
    AdminCommand, AdminCommandSender, AudioThreadEndpoint, BackendSlot, BridgeMetrics,
    EndpointScratch, LatencyAttestation, MAX_OUTSTANDING_PARAMETER_EDITS, MidiMessage,
    NativeEditorCommand, NativeEditorState, PluginBackend, PluginChain, PluginEndpointManifest,
    PluginPrepareConfig, RealtimeOutputSource, RealtimeProcessStatus, RetainedShutdownPoll,
    RuntimeEvent, ShutdownOutcome, StereoBlock,
};
use super::*;
use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::{Arc, atomic::AtomicU32, mpsc};
use std::thread;
use std::time::Duration;

fn endpoint(
    enabled: bool,
) -> (
    AudioThreadEndpoint,
    Consumer<StereoBlock>,
    Producer<StereoBlock>,
) {
    let (input, input_consumer) = RingBuffer::new(48);
    let (output_producer, output) = RingBuffer::new(48);
    let metrics = Arc::new(BridgeMetrics {
        diagnostics: RuntimeDiagnostics::new(enabled),
        ..Default::default()
    });
    metrics.max_block_frames.store(128, Ordering::Relaxed);
    metrics.current_epoch.store(1, Ordering::Relaxed);
    metrics.plugin_latency_revision.store(1, Ordering::Relaxed);
    let requested_epoch = Arc::clone(&metrics.requested_epoch);
    requested_epoch.store(1, Ordering::Relaxed);
    let (parameter_edit_failures, _failure_rx) =
        RingBuffer::new(MAX_OUTSTANDING_PARAMETER_EDITS as usize);
    let mut audio = AudioThreadEndpoint {
        input,
        output,
        epoch: 1,
        next_sequence: 1,
        requested_epoch,
        scratch: Box::new(EndpointScratch::new()),
        metrics,
        manifest: PluginEndpointManifest::default(),
        parameter_edit_admission: Arc::new(AtomicU32::new(0)),
        parameter_edit_failures,
    };
    assert!(audio.configure_timing_plan(
        crate::plugin_timing::PreparedPluginTimingPlan::new(1, 48_000, 2048).unwrap()
    ));
    (audio, input_consumer, output_producer)
}

#[derive(Debug, PartialEq, Eq)]
struct RunOutcome {
    pcm_checksum: u64,
    midi_checksum: u64,
    output_quanta: u64,
    fault: Option<(PluginProcessingFaultReason, u64)>,
    submitted: u64,
    completed: u64,
    deadline_misses: u64,
    lookahead: usize,
    epoch: u64,
}

fn advance_exact(enabled: bool, withheld: Option<u64>) -> (RunOutcome, AudioThreadEndpoint) {
    let (mut audio, mut input, mut output) = endpoint(enabled);
    let mut left = [0.0; 128];
    let mut right = [0.0; 128];
    let mut pcm_checksum = 0_u64;
    let mut midi_checksum = 0_u64;
    let mut output_quanta = 0;
    // 18 priming quanta plus 7000 exact musical outputs. This has no scheduler race.
    for sequence in 1..=7018_u64 {
        let value = sequence as f32 / 10_000.0;
        let samples = [value; 128];
        let status = audio.process_realtime_with_expected_latency_revision(
            &samples, &samples, &mut left, &mut right, 1,
        );
        if audio.processing_fault().is_some() {
            assert_eq!(left, [0.0; 128]);
            assert_eq!(right, [0.0; 128]);
            assert_eq!(audio.take_midi_output().len, 0);
            break;
        }
        let expected = sequence.saturating_sub(18);
        let batch = audio.take_midi_output();
        if expected == 0 {
            assert_eq!(left, [0.0; 128]);
            assert_eq!(batch.len, 0);
        } else {
            assert!(matches!(status, RealtimeProcessStatus::Processed {
                sequence: actual,
                source: RealtimeOutputSource::Plugin,
                ..
            } if actual == expected));
            assert_eq!(left, [(expected as f32 / 10_000.0) * 2.0; 128]);
            assert_eq!(right, left);
            assert_eq!(batch.len, 1);
            assert_eq!(
                batch.events[0].message.data,
                [0x90, (expected % 128) as u8, 97]
            );
            pcm_checksum = pcm_checksum
                .wrapping_mul(31)
                .wrapping_add(u64::from(left[0].to_bits()));
            midi_checksum = midi_checksum
                .wrapping_mul(31)
                .wrapping_add(u64::from(batch.events[0].message.data[1]));
            output_quanta += 1;
        }
        let mut block = input.pop().unwrap();
        assert_eq!(block.sequence, sequence);
        if withheld == Some(sequence) {
            continue;
        }
        block.left[..128].fill(value * 2.0);
        block.right[..128].fill(value * 2.0);
        block.latency_attestation = LatencyAttestation::Valid;
        block
            .midi_output
            .push(0, MidiMessage::new([0x90, (sequence % 128) as u8, 97], 0));
        output.push(block).unwrap();
        audio.metrics.completed.fetch_add(1, Ordering::Relaxed);
        audio.metrics.diagnostics.count_output_frames(128);
    }
    let stats = audio.stats();
    (
        RunOutcome {
            pcm_checksum,
            midi_checksum,
            output_quanta,
            fault: audio.processing_fault(),
            submitted: stats.submitted,
            completed: stats.completed,
            deadline_misses: stats.deadline_misses,
            lookahead: audio.bridge_lookahead_quanta(),
            epoch: audio.epoch,
        },
        audio,
    )
}

#[test]
fn diagnostics_on_off_preserve_7000_exact_pcm_midi_and_k18() {
    let (off, off_audio) = advance_exact(false, None);
    let (on, on_audio) = advance_exact(true, None);
    assert_eq!(off, on);
    assert_eq!(on.output_quanta, 7000);
    assert_eq!(on.lookahead, 18);
    assert_eq!(on.fault, None);
    assert_eq!(on.deadline_misses, 0);
    assert!(!off_audio.runtime_diagnostics_snapshot().enabled);
    assert!(on_audio.runtime_diagnostics_snapshot().enabled);
    assert_eq!(on_audio.runtime_diagnostics_snapshot().first_fault, None);
}

#[test]
fn withheld_6924_faults_only_at_original_boundary_and_reset_preserves_first_evidence() {
    let (off, _) = advance_exact(false, Some(6924));
    let (on, mut audio) = advance_exact(true, Some(6924));
    assert_eq!(off, on);
    assert_eq!(on.output_quanta, 6923);
    assert_eq!(
        on.fault,
        Some((PluginProcessingFaultReason::DeadlineMiss, 6924))
    );
    assert_eq!(on.deadline_misses, 1);
    let snapshot = audio.runtime_diagnostics_snapshot();
    let original = snapshot.first_fault.unwrap();
    assert!(snapshot.first_fault_coherent);
    assert_eq!(original.epoch, 1);
    assert_eq!(original.fault_sequence, 6924);
    assert_eq!(original.next_sequence, 6942);
    assert_eq!(original.expected_sequence, 6924);
    assert_eq!(original.receive_branch, PluginReceiveBranch::Empty);
    assert_eq!(original.requested_frames, 128);
    assert_eq!(original.future_output_owned, 17);
    assert_eq!(original.input_frames, original.submitted * 128);
    assert_eq!(original.output_frames, original.completed * 128);
    assert!(audio.set_epoch(2));
    assert_eq!(audio.processing_fault(), None);
    assert_eq!(
        audio.runtime_diagnostics_snapshot().first_fault,
        Some(original)
    );
    let samples = [0.0; 128];
    let mut left = samples;
    let mut right = samples;
    for _ in 0..19 {
        audio.process_realtime_with_expected_latency_revision(
            &samples, &samples, &mut left, &mut right, 1,
        );
    }
    assert_eq!(
        audio.processing_fault(),
        Some((PluginProcessingFaultReason::DeadlineMiss, 1))
    );
    assert_eq!(
        audio.runtime_diagnostics_snapshot().first_fault,
        Some(original)
    );
}

#[test]
fn stage_begin_is_readable_before_end_and_all_stage_names_remain_distinct() {
    let diagnostics = RuntimeDiagnostics::new(true);
    diagnostics.context(4, 6924);
    for stage in [
        PluginWorkerStage::Reset,
        PluginWorkerStage::NativeFeedback,
        PluginWorkerStage::Admin,
        PluginWorkerStage::Midi,
        PluginWorkerStage::Transport,
        PluginWorkerStage::BackendProcess,
        PluginWorkerStage::Metadata,
        PluginWorkerStage::OutputPush,
        PluginWorkerStage::IdlePark,
    ] {
        let scope = diagnostics.stage(stage, Some(0));
        let active = diagnostics.snapshot().worker;
        assert!(active.coherent && active.active);
        assert_eq!(
            (active.epoch, active.sequence, active.slot),
            (4, 6924, Some(0))
        );
        assert_eq!(active.stage, stage);
        assert_eq!(active.end_ns, 0);
        drop(scope);
        let completed = diagnostics.snapshot().worker;
        assert!(completed.coherent && !completed.active);
        assert_eq!(completed.begin_ns, active.begin_ns);
        assert!(completed.end_ns >= completed.begin_ns);
        assert!(completed.publication_id > active.publication_id);
    }
}

#[test]
fn publication_collision_and_version_exhaustion_are_bounded_and_explicit() {
    let diagnostics = RuntimeDiagnostics::new(true);
    diagnostics.worker.version.store(1, Ordering::Release);
    let collided = diagnostics.snapshot();
    assert!(!collided.worker.coherent);
    diagnostics
        .worker
        .version
        .store(u64::MAX - 1, Ordering::Release);
    let scope = diagnostics.stage(PluginWorkerStage::BackendProcess, Some(0));
    assert!(diagnostics.snapshot().worker.exhausted);
    drop(scope);
    diagnostics.fault.version.store(1, Ordering::Release);
    assert!(!diagnostics.snapshot().first_fault_coherent);
    assert_eq!(diagnostics.snapshot().first_fault, None);
    diagnostics
        .fault
        .version
        .store(u64::MAX - 1, Ordering::Release);
    diagnostics.fault.publish([0; FAULT_FIELDS]);
    assert!(diagnostics.snapshot().first_fault_exhausted);
}

#[test]
fn disabled_diagnostics_never_publish_or_read_clock() {
    let diagnostics = RuntimeDiagnostics::new(false);
    assert!(diagnostics.origin.is_none());
    diagnostics.context(4, 6924);
    let scope = diagnostics.stage(PluginWorkerStage::BackendProcess, Some(0));
    let admin = diagnostics.admin(
        PluginWorkerAdminCommand::SaveState,
        Some(0),
        72,
        PluginWorkerNativeEditorCommand::None,
    );
    diagnostics.count_input_frames(128);
    diagnostics.count_output_frames(128);
    drop(scope);
    drop(admin);
    let snapshot = diagnostics.snapshot();
    assert!(!snapshot.enabled);
    assert_eq!(diagnostics.worker.version.load(Ordering::Relaxed), 0);
    assert_eq!(diagnostics.context_epoch.load(Ordering::Relaxed), 0);
    assert_eq!(diagnostics.context_admin_command.load(Ordering::Relaxed), 0);
    assert_eq!(
        diagnostics.context_admin_request_id.load(Ordering::Relaxed),
        0
    );
    assert_eq!(diagnostics.input_frames.load(Ordering::Relaxed), 0);
    assert!(snapshot.first_fault_coherent);
}

#[test]
fn concurrent_publication_never_combines_epoch_sequence_and_phase() {
    let diagnostics = Arc::new(RuntimeDiagnostics::new(true));
    let writer_diagnostics = Arc::clone(&diagnostics);
    let writer = thread::spawn(move || {
        for sequence in 1..=10_000 {
            writer_diagnostics.context(sequence + 100, sequence);
            drop(writer_diagnostics.stage(PluginWorkerStage::BackendProcess, Some(1)));
        }
    });
    while !writer.is_finished() {
        let snapshot = diagnostics.snapshot().worker;
        if snapshot.coherent && snapshot.publication_id != 0 {
            assert_eq!(snapshot.epoch, snapshot.sequence + 100);
            assert_eq!(snapshot.slot, Some(1));
            assert_eq!(snapshot.stage, PluginWorkerStage::BackendProcess);
            assert!(snapshot.active || snapshot.end_ns >= snapshot.begin_ns);
        }
    }
    writer.join().unwrap();
}

struct Gate {
    entered: AtomicBool,
    released: AtomicBool,
}

struct GatedBackend(Arc<Gate>);

struct ReleaseGate(Arc<Gate>);
impl Drop for ReleaseGate {
    fn drop(&mut self) {
        self.0.released.store(true, Ordering::Release);
    }
}

impl PluginBackend for GatedBackend {
    fn name(&self) -> &str {
        "diagnostic gated backend"
    }
    fn prepare(&mut self, _config: PluginPrepareConfig) -> Result<(), String> {
        Ok(())
    }
    fn process(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        frames: usize,
    ) -> Result<(), String> {
        self.0.entered.store(true, Ordering::Release);
        while !self.0.released.load(Ordering::Acquire) {
            thread::yield_now();
        }
        left[..frames].fill(0.25);
        right[..frames].fill(0.25);
        Ok(())
    }
    fn send_midi(&mut self, _message: MidiMessage) -> Result<(), String> {
        Ok(())
    }
    fn set_parameter(&mut self, _id: u32, _normalized: f32) -> Result<(), String> {
        Ok(())
    }
    fn get_parameter(&mut self, _id: u32) -> Result<f32, String> {
        Ok(0.0)
    }
    fn save_state(&mut self) -> Result<Vec<u8>, String> {
        Ok(Vec::new())
    }
    fn load_state(&mut self, _state: &[u8]) -> Result<(), String> {
        Ok(())
    }
    fn latency_samples(&self) -> u32 {
        0
    }
    fn tail_samples(&self) -> u32 {
        0
    }
}

fn wait_for(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !predicate() {
        assert!(
            Instant::now() < deadline,
            "diagnostic worker wait timed out"
        );
        thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn real_worker_backend_begin_is_captured_before_fault_midi_safety_reset() {
    let gate = Arc::new(Gate {
        entered: AtomicBool::new(false),
        released: AtomicBool::new(false),
    });
    let backend_gate = Arc::clone(&gate);
    let _release_on_failure = ReleaseGate(Arc::clone(&gate));
    let mut chain = PluginChain::spawn_loader_with_diagnostics(
        PluginPrepareConfig {
            sample_rate: 48_000.0,
            max_block_frames: 128,
        },
        PluginEndpointManifest::unknown(1),
        move || vec![Ok(BackendSlot::new(Box::new(GatedBackend(backend_gate))))],
        RuntimeDiagnostics::new(true),
    )
    .unwrap();
    wait_for(|| chain.control.plugin_latency_snapshot().is_some());
    let revision = chain.control.plugin_latency_snapshot().unwrap().revision;
    assert!(chain.audio.configure_timing_plan(
        crate::plugin_timing::PreparedPluginTimingPlan::new(1, 48_000, 2048).unwrap()
    ));
    let samples = [0.0; 128];
    let mut left = samples;
    let mut right = samples;
    chain.audio.process_realtime_with_expected_latency_revision(
        &samples, &samples, &mut left, &mut right, revision,
    );
    wait_for(|| gate.entered.load(Ordering::Acquire));
    for _ in 0..18 {
        chain.audio.process_realtime_with_expected_latency_revision(
            &samples, &samples, &mut left, &mut right, revision,
        );
    }
    let snapshot = chain.control.runtime_diagnostics_snapshot();
    let decision = snapshot.first_fault;
    gate.released.store(true, Ordering::Release);
    assert_eq!(
        chain.guard.shutdown_blocking(Duration::from_secs(3)),
        ShutdownOutcome::Joined
    );
    let decision = decision.unwrap();
    assert_eq!(decision.reason, PluginProcessingFaultReason::DeadlineMiss);
    assert_eq!(
        (
            decision.epoch,
            decision.fault_sequence,
            decision.next_sequence
        ),
        (1, 1, 19)
    );
    assert!(decision.worker_at_fault.coherent && decision.worker_at_fault.active);
    assert_eq!(
        decision.worker_at_fault.stage,
        PluginWorkerStage::BackendProcess
    );
    assert_eq!(decision.worker_at_fault.sequence, 1);
    assert_eq!(decision.worker_at_fault.slot, Some(0));
    assert_eq!(decision.worker_at_fault.end_ns, 0);
    assert_ne!(decision.worker_at_fault.publication_id, 0);
    assert_eq!(left, [0.0; 128]);
    // The JSON representation is produced only on this control/test thread.
    let json = serde_json::to_value(decision).unwrap();
    assert_eq!(json["reason"], "DeadlineMiss");
    assert_eq!(json["worker_at_fault"]["stage"], "BackendProcess");
}

#[test]
fn nested_capture_keeps_admin_identity_and_idle_clears_command_context() {
    let diagnostics = RuntimeDiagnostics::new(true);
    diagnostics.context(4, 1397);
    let admin = diagnostics.admin(
        PluginWorkerAdminCommand::SaveState,
        Some(0),
        72,
        PluginWorkerNativeEditorCommand::None,
    );
    {
        let capture = diagnostics.stage(PluginWorkerStage::NativeStateCapture, Some(0));
        let snapshot = diagnostics.snapshot().worker;
        assert_eq!(snapshot.admin_command, PluginWorkerAdminCommand::SaveState);
        assert_eq!(snapshot.admin_request_id, 72);
        assert_eq!(snapshot.stage, PluginWorkerStage::NativeStateCapture);
        assert_eq!(snapshot.sequence, 1397);
        drop(capture);
    }
    drop(admin);
    let finished = diagnostics.snapshot().worker;
    assert!(!finished.active);
    assert_eq!(finished.stage, PluginWorkerStage::Admin);
    assert_eq!(finished.admin_command, PluginWorkerAdminCommand::SaveState);
    let idle = diagnostics.stage(PluginWorkerStage::IdlePark, None);
    let snapshot = diagnostics.snapshot().worker;
    assert_eq!(snapshot.admin_command, PluginWorkerAdminCommand::None);
    assert_eq!(snapshot.admin_request_id, 0);
    assert_eq!(
        snapshot.native_editor_command,
        PluginWorkerNativeEditorCommand::None
    );
    drop(idle);
}

struct GatedAdminBackend {
    gate: Arc<Gate>,
    command: PluginWorkerAdminCommand,
}

impl GatedAdminBackend {
    fn block(&self, command: PluginWorkerAdminCommand) {
        if self.command == command {
            self.gate.entered.store(true, Ordering::Release);
            while !self.gate.released.load(Ordering::Acquire) {
                thread::yield_now();
            }
        }
    }
}

impl PluginBackend for GatedAdminBackend {
    fn name(&self) -> &str {
        "diagnostic gated admin backend"
    }
    fn prepare(&mut self, _config: PluginPrepareConfig) -> Result<(), String> {
        Ok(())
    }
    fn process(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        frames: usize,
    ) -> Result<(), String> {
        left[..frames].fill(0.25);
        right[..frames].fill(0.25);
        Ok(())
    }
    fn send_midi(&mut self, _message: MidiMessage) -> Result<(), String> {
        Ok(())
    }
    fn set_parameter(&mut self, _id: u32, _normalized: f32) -> Result<(), String> {
        Ok(())
    }
    fn get_parameter(&mut self, _id: u32) -> Result<f32, String> {
        self.block(PluginWorkerAdminCommand::QueryParameter);
        Ok(0.5)
    }
    fn save_state(&mut self) -> Result<Vec<u8>, String> {
        self.block(PluginWorkerAdminCommand::SaveState);
        Ok(Vec::new())
    }
    fn load_state(&mut self, _state: &[u8]) -> Result<(), String> {
        Ok(())
    }
    fn native_editor(&mut self, command: NativeEditorCommand) -> Result<NativeEditorState, String> {
        if command == NativeEditorCommand::Close {
            self.block(PluginWorkerAdminCommand::NativeEditor);
        }
        Ok(NativeEditorState::default())
    }
    fn latency_samples(&self) -> u32 {
        0
    }
    fn tail_samples(&self) -> u32 {
        0
    }
}

fn gated_admin_fault(
    command: PluginWorkerAdminCommand,
    request_id: u64,
) -> PluginFaultDecisionSnapshot {
    let gate = Arc::new(Gate {
        entered: AtomicBool::new(false),
        released: AtomicBool::new(false),
    });
    let backend_gate = Arc::clone(&gate);
    let _release_on_failure = ReleaseGate(Arc::clone(&gate));
    let mut chain = PluginChain::spawn_loader_with_diagnostics(
        PluginPrepareConfig {
            sample_rate: 48_000.0,
            max_block_frames: 128,
        },
        PluginEndpointManifest::unknown(1),
        move || {
            vec![Ok(BackendSlot::new(Box::new(GatedAdminBackend {
                gate: backend_gate,
                command,
            })))]
        },
        RuntimeDiagnostics::new(true),
    )
    .unwrap();
    wait_for(|| chain.control.plugin_latency_snapshot().is_some());
    let revision = chain.control.plugin_latency_snapshot().unwrap().revision;
    assert!(chain.audio.configure_timing_plan(
        crate::plugin_timing::PreparedPluginTimingPlan::new(1, 48_000, 2048).unwrap()
    ));
    let samples = [0.0; 128];
    let mut left = samples;
    let mut right = samples;
    chain.audio.process_realtime_with_expected_latency_revision(
        &samples, &samples, &mut left, &mut right, revision,
    );
    wait_for(|| chain.control.stats().completed == 1);
    let queued = match command {
        PluginWorkerAdminCommand::SaveState => chain
            .control
            .request_state_tagged_for_epoch(0, request_id, 1),
        PluginWorkerAdminCommand::QueryParameter => {
            chain.control.query_parameter_tagged(0, 17, request_id)
        }
        PluginWorkerAdminCommand::NativeEditor => {
            chain
                .control
                .request_native_editor(0, request_id, NativeEditorCommand::Close, &[])
        }
        _ => panic!("unsupported gated admin test command"),
    };
    assert!(queued);
    wait_for(|| gate.entered.load(Ordering::Acquire));
    for _ in 2..=20 {
        chain.audio.process_realtime_with_expected_latency_revision(
            &samples, &samples, &mut left, &mut right, revision,
        );
    }
    let decision = chain.control.runtime_diagnostics_snapshot().first_fault;
    gate.released.store(true, Ordering::Release);
    let decision = decision.unwrap();
    assert!(chain.audio.set_epoch(2));
    assert_eq!(
        chain.control.runtime_diagnostics_snapshot().first_fault,
        Some(decision)
    );
    assert_eq!(
        chain.guard.shutdown_blocking(Duration::from_secs(3)),
        ShutdownOutcome::Joined
    );
    assert_eq!(decision.reason, PluginProcessingFaultReason::DeadlineMiss);
    assert_eq!(
        (
            decision.fault_sequence,
            decision.expected_sequence,
            decision.next_sequence
        ),
        (2, 2, 20)
    );
    assert_eq!(decision.input_pending, 18);
    assert_eq!(decision.worker_at_fault.sequence, 1);
    assert!(decision.worker_at_fault.coherent && decision.worker_at_fault.active);
    assert_eq!(decision.worker_at_fault.slot, Some(0));
    assert_eq!(decision.worker_at_fault.admin_command, command);
    assert_eq!(decision.worker_at_fault.admin_request_id, request_id);
    decision
}

#[test]
fn real_worker_save_state_gate_preserves_command_and_request_in_first_fault() {
    let decision = gated_admin_fault(PluginWorkerAdminCommand::SaveState, 72);
    assert_eq!(
        decision.worker_at_fault.stage,
        PluginWorkerStage::NativeStateCapture
    );
    assert_eq!(
        decision.worker_at_fault.native_editor_command,
        PluginWorkerNativeEditorCommand::None
    );
    let json = serde_json::to_value(decision).unwrap();
    assert_eq!(json["worker_at_fault"]["admin_command"], "SaveState");
    assert_eq!(json["worker_at_fault"]["admin_request_id"], 72);
}

#[test]
fn real_worker_query_parameter_gate_is_distinct_from_save_state() {
    let decision = gated_admin_fault(PluginWorkerAdminCommand::QueryParameter, 71);
    assert_eq!(decision.worker_at_fault.stage, PluginWorkerStage::Admin);
    assert_eq!(
        decision.worker_at_fault.native_editor_command,
        PluginWorkerNativeEditorCommand::None
    );
}

#[test]
fn real_worker_native_editor_close_gate_is_distinct_from_query_and_save_state() {
    let decision = gated_admin_fault(PluginWorkerAdminCommand::NativeEditor, 73);
    assert_eq!(decision.worker_at_fault.stage, PluginWorkerStage::Admin);
    assert_eq!(
        decision.worker_at_fault.native_editor_command,
        PluginWorkerNativeEditorCommand::Close
    );
}

fn save_admin(request_id: u64) -> AdminCommand {
    AdminCommand::SaveState {
        slot: 0,
        request_id,
        expected_epoch: None,
    }
}

#[test]
fn production_admin_lease_covers_queued_work_before_worker_busy_and_queue_failure() {
    let (audio, _input, _output) = endpoint(false);
    audio
        .metrics
        .worker_settlement
        .valid
        .store(true, Ordering::Relaxed);
    audio
        .metrics
        .worker_settlement
        .reset_completed_epoch
        .store(1, Ordering::Relaxed);
    assert!(audio.stopped_playback_settled(1));
    let (sender, receiver) = mpsc::sync_channel(1);
    let sender = AdminCommandSender::new(sender, &audio.metrics);
    assert!(sender.try_send(save_admin(1)).is_ok());
    assert_eq!(
        audio.metrics.worker_settlement.busy.load(Ordering::Acquire),
        0
    );
    assert_eq!(
        audio.metrics.outstanding_admin_work.load(Ordering::Acquire),
        1
    );
    assert!(!audio.stopped_playback_settled(1));
    assert!(matches!(
        sender.try_send(save_admin(2)),
        Err(mpsc::TrySendError::Full(_))
    ));
    assert_eq!(
        audio.metrics.outstanding_admin_work.load(Ordering::Acquire),
        1
    );
    let mut queued = receiver.try_recv().unwrap();
    assert!(matches!(
        queued.take_command(),
        AdminCommand::SaveState { request_id: 1, .. }
    ));
    assert_eq!(
        audio.metrics.outstanding_admin_work.load(Ordering::Acquire),
        1
    );
    assert!(!audio.stopped_playback_settled(1));
    drop(queued);
    assert_eq!(
        audio.metrics.outstanding_admin_work.load(Ordering::Acquire),
        0
    );
    assert!(audio.stopped_playback_settled(1));
    drop(receiver);
    assert!(matches!(
        sender.try_send(save_admin(3)),
        Err(mpsc::TrySendError::Disconnected(_))
    ));
    assert_eq!(
        audio.metrics.outstanding_admin_work.load(Ordering::Acquire),
        0
    );
}

#[test]
fn production_admin_receiver_reclamation_and_unwind_release_each_lease_once() {
    let metrics = Arc::new(BridgeMetrics::default());
    let (sender, receiver) = mpsc::sync_channel(2);
    let sender = AdminCommandSender::new(sender, &metrics);
    assert!(sender.try_send(save_admin(1)).is_ok());
    assert!(sender.try_send(AdminCommand::Shutdown).is_ok());
    assert_eq!(metrics.outstanding_admin_work.load(Ordering::Acquire), 2);
    drop(receiver);
    assert_eq!(metrics.outstanding_admin_work.load(Ordering::Acquire), 0);
    assert!(matches!(
        sender.try_send(save_admin(2)),
        Err(mpsc::TrySendError::Disconnected(_))
    ));
    assert_eq!(metrics.outstanding_admin_work.load(Ordering::Acquire), 0);
    let (sender, receiver) = mpsc::sync_channel(1);
    let sender = AdminCommandSender::new(sender, &metrics);
    assert!(sender.try_send(save_admin(3)).is_ok());
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut queued = receiver.try_recv().unwrap();
        let _command = queued.take_command();
        assert_eq!(metrics.outstanding_admin_work.load(Ordering::Acquire), 1);
        panic!("deliberate admin execution unwind");
    }));
    assert!(result.is_err());
    assert_eq!(metrics.outstanding_admin_work.load(Ordering::Acquire), 0);
}

#[test]
fn production_admin_admission_counter_never_wraps_to_false_idle() {
    let metrics = Arc::new(BridgeMetrics::default());
    let (sender, receiver) = mpsc::sync_channel(1);
    let sender = AdminCommandSender::new(sender, &metrics);
    metrics
        .outstanding_admin_work
        .store(u32::MAX, Ordering::Release);
    assert!(matches!(
        sender.try_send(save_admin(1)),
        Err(mpsc::TrySendError::Full(_))
    ));
    assert_eq!(
        metrics.outstanding_admin_work.load(Ordering::Acquire),
        u32::MAX
    );
    assert!(matches!(
        receiver.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
}

fn production_admin_chain(command: PluginWorkerAdminCommand) -> (PluginChain, Arc<Gate>) {
    let gate = Arc::new(Gate {
        entered: AtomicBool::new(false),
        released: AtomicBool::new(false),
    });
    let backend_gate = Arc::clone(&gate);
    let chain = PluginChain::spawn_loader_with_diagnostics(
        PluginPrepareConfig {
            sample_rate: 48_000.0,
            max_block_frames: 128,
        },
        PluginEndpointManifest::unknown(1),
        move || {
            vec![Ok(BackendSlot::new(Box::new(GatedAdminBackend {
                gate: backend_gate,
                command,
            })))]
        },
        RuntimeDiagnostics::new(false),
    )
    .unwrap();
    (chain, gate)
}

#[test]
fn production_admin_actual_save_remains_owned_until_backend_returns_with_diagnostics_off() {
    let (mut chain, gate) = production_admin_chain(PluginWorkerAdminCommand::SaveState);
    let _release_on_failure = ReleaseGate(Arc::clone(&gate));
    wait_for(|| chain.audio.stopped_playback_settled(1));
    assert!(!chain.control.has_outstanding_admin_work());
    assert!(!chain.control.runtime_diagnostics_snapshot().enabled);
    assert!(chain.control.request_state_tagged_for_epoch(0, 81, 1));
    assert!(chain.control.has_outstanding_admin_work());
    wait_for(|| gate.entered.load(Ordering::Acquire));
    assert!(chain.control.has_outstanding_admin_work());
    assert!(!chain.audio.stopped_playback_settled(1));
    // A UI timeout is not an owned worker operation and cannot clear this count.
    assert_eq!(
        chain
            .control
            .metrics
            .outstanding_admin_work
            .load(Ordering::Acquire),
        1
    );
    gate.released.store(true, Ordering::Release);
    let mut received_state = false;
    wait_for(|| {
        while let Some(event) = chain.control.try_next_event() {
            received_state |= matches!(event, RuntimeEvent::State { request_id: 81, .. });
        }
        received_state
            && !chain.control.has_outstanding_admin_work()
            && chain.audio.stopped_playback_settled(1)
    });
    assert_eq!(
        chain.guard.shutdown_blocking(Duration::from_secs(3)),
        ShutdownOutcome::Joined
    );
    assert!(!chain.control.has_outstanding_admin_work());
}

#[test]
fn production_admin_shutdown_retains_ownership_through_real_window_close_and_reclaims_queue() {
    let (mut chain, gate) = production_admin_chain(PluginWorkerAdminCommand::NativeEditor);
    let _release_on_failure = ReleaseGate(Arc::clone(&gate));
    wait_for(|| chain.audio.stopped_playback_settled(1));
    assert_eq!(
        chain.guard.poll_shutdown_retained(),
        RetainedShutdownPoll::Pending
    );
    wait_for(|| gate.entered.load(Ordering::Acquire));
    // Shutdown's actual backend close is still blocked although its queue entry was consumed.
    assert!(chain.control.has_outstanding_admin_work());
    assert!(!chain.audio.stopped_playback_settled(1));
    assert!(chain.control.request_state_tagged(0, 82));
    assert_eq!(
        chain
            .control
            .metrics
            .outstanding_admin_work
            .load(Ordering::Acquire),
        2
    );
    gate.released.store(true, Ordering::Release);
    wait_for(|| chain.guard.poll_shutdown_retained() == RetainedShutdownPoll::Joined);
    assert!(!chain.control.has_outstanding_admin_work());
    assert!(chain.control.stats().stopped);
    while let Some(event) = chain.control.try_next_event() {
        assert!(!matches!(event, RuntimeEvent::State { request_id: 82, .. }));
    }
}

#[test]
fn production_admin_epoch_rejection_releases_without_entering_state_capture() {
    let (mut chain, gate) = production_admin_chain(PluginWorkerAdminCommand::SaveState);
    let _release_on_failure = ReleaseGate(Arc::clone(&gate));
    wait_for(|| chain.audio.stopped_playback_settled(1));
    assert!(chain.control.request_state_tagged_for_epoch(0, 90, 7));
    let mut rejected = false;
    wait_for(|| {
        while let Some(event) = chain.control.try_next_event() {
            rejected |= matches!(event, RuntimeEvent::StateRejected { request_id: 90, .. });
        }
        rejected && !chain.control.has_outstanding_admin_work()
    });
    assert!(!gate.entered.load(Ordering::Acquire));
    assert_eq!(
        chain.guard.shutdown_blocking(Duration::from_secs(3)),
        ShutdownOutcome::Joined
    );
    assert!(!chain.control.has_outstanding_admin_work());
}
