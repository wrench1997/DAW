//! Test-profile observation only. No observer changes a DSP decision or services work.
//!
//! The serial test driver enables and drains this collector. Callback writes copy only
//! bounded values into already allocated storage. Detailed snapshots allocate/clone Arcs
//! on the driver thread, never from render_transport_chunk.

use super::*;
use crate::mixer_graph::{CompiledMixerNode, CompiledMixerRoute, MIXER_GRAPH_MAX_EDGES};

pub(crate) const GENUINE_SEGMENT_CAPACITY: usize = 8192;
pub(crate) const GENUINE_LIFECYCLE_CAPACITY: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GenuineLifecycleKind {
    Install,
    StampedRemove,
    Remove,
    Clear,
    RouteSet,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GenuineLifecycleEndpoint {
    pub(crate) channel_id: u32,
    pub(crate) endpoint_id: u64,
    pub(crate) plugin_instance_id: u64,
    pub(crate) mixer_track: usize,
    pub(crate) project_session: u64,
    pub(crate) worker_id: usize,
    pub(crate) epoch: u64,
}

/// Literal scalar state at the mutation boundary, not the preceding pump's state.
#[derive(Clone, Copy, Debug)]
pub(crate) struct GenuineLifecycleState {
    pub(crate) engine_session: u64,
    pub(crate) project_session: u64,
    pub(crate) transport_epoch: u64,
    pub(crate) legacy_request_barrier: u64,
    pub(crate) playing: bool,
    pub(crate) generation: Option<u64>,
    pub(crate) active_epoch: Option<u64>,
    pub(crate) plugin_topology_revision: u64,
    pub(crate) plugin_topology_replan_pending: bool,
    pub(crate) timeline_execution_failures: u64,
    pub(crate) fault: Option<PluginProcessingFault>,
    pub(crate) needs_resync: bool,
    pub(crate) suspended: bool,
    pub(crate) graph_was_activated: bool,
    pub(crate) graph_binding_exact: bool,
    pub(crate) graph_identity: Option<(u64, u64, u64)>,
    pub(crate) channel_binding_revision: Option<u64>,
    pub(crate) channel_binding_epoch: Option<u64>,
    pub(crate) active_plugin_automation_bindings: usize,
    pub(crate) candidate_plugin_automation_bindings: usize,
    pub(crate) compiled_midi_routes: usize,
    pub(crate) generator_endpoints: usize,
    // Retained in every test profile; the Linux genuine ledger consumes this count.
    #[cfg_attr(
        not(all(target_os = "linux", feature = "experimental-fresh-helper")),
        allow(dead_code)
    )]
    pub(crate) insert_endpoints: usize,
    // Retained in every test profile; the Linux genuine ledger binds this revision.
    #[cfg_attr(
        not(all(target_os = "linux", feature = "experimental-fresh-helper")),
        allow(dead_code)
    )]
    pub(crate) pdc_revision: u64,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct GenuineLifecycleObservation {
    pub(crate) run_binding: [u8; 32],
    pub(crate) pump_ordinal: u64,
    pub(crate) record_ordinal: u64,
    pub(crate) kind: GenuineLifecycleKind,
    pub(crate) event: GeneratorEndpointEvent,
    pub(crate) producer_present: bool,
    pub(crate) enqueue_success: bool,
    pub(crate) affected: Option<GenuineLifecycleEndpoint>,
    pub(crate) replaced: Option<GenuineLifecycleEndpoint>,
    /// Only accepted install/stamped removal/physical removal have a pair.
    pub(crate) pre: Option<GenuineLifecycleState>,
    pub(crate) post: GenuineLifecycleState,
}

#[derive(Clone, Copy)]
struct GenuineLifecycleMutation {
    kind: GenuineLifecycleKind,
    affected: GenuineLifecycleEndpoint,
    replaced: Option<GenuineLifecycleEndpoint>,
    pre: GenuineLifecycleState,
    stamp: Option<GeneratorRemovalStamp>,
    pump_ordinal: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GenuineSegmentDisposition {
    Rendered,
    FaultSilence,
    SuspendedSilence,
    AdmissionRejected,
}

#[derive(Clone, Copy, Debug)]
#[allow(dead_code)] // Bounded raw evidence retains fields beyond each profile\'s selected oracle.
pub(crate) struct GenuineSegmentObservation {
    pub(crate) pump_ordinal: u64,
    pub(crate) segment_ordinal: u64,
    pub(crate) start_frame: u64,
    pub(crate) frames: usize,
    /// Offset into this pump's output; not a post-pump reconstructed Timeline stamp.
    pub(crate) device_offset: usize,
    pub(crate) device_frame: u64,
    pub(crate) playing: bool,
    pub(crate) playing_after_render: bool,
    /// Exact prepare_timeline_render result. False while correctly paused.
    pub(crate) timeline_ready: bool,
    /// Actual plan-active flag immediately after prepare_timeline_render,
    /// before render_block can consume and clear the completed plan.
    pub(crate) timeline_render_active: bool,
    pub(crate) timeline_render_active_after: bool,
    pub(crate) generation: Option<u64>,
    pub(crate) active_epoch: Option<u64>,
    pub(crate) epoch: u64,
    pub(crate) request_id: u64,
    pub(crate) discontinuity_id: u64,
    pub(crate) loop_count: u64,
    pub(crate) loop_enabled: bool,
    pub(crate) loop_start_frame: u64,
    pub(crate) loop_end_frame: u64,
    pub(crate) loop_token: Option<u64>,
    pub(crate) disposition: GenuineSegmentDisposition,
    pub(crate) before: GenuineSegmentState,
    pub(crate) after: GenuineSegmentState,
}

#[derive(Clone, Copy, Debug)]
#[allow(dead_code)] // Bounded raw evidence retains fields beyond each profile\'s selected oracle.
pub(crate) struct GenuineSegmentState {
    pub(crate) generator_endpoints: usize,
    pub(crate) insert_endpoints: usize,
    pub(crate) native_voices: usize,
    pub(crate) audio_voices: usize,
    pub(crate) generator_notes: usize,
    pub(crate) fault: Option<PluginProcessingFault>,
    pub(crate) suspended: bool,
    pub(crate) needs_resync: bool,
    pub(crate) graph_identity: Option<(u64, u64, u64)>,
    pub(crate) graph_was_activated: bool,
    pub(crate) graph_binding_exact: bool,
    pub(crate) channel_binding_revision: Option<u64>,
    pub(crate) channel_binding_epoch: Option<u64>,
    pub(crate) plugin_topology_revision: u64,
    pub(crate) plugin_topology_replan_pending: bool,
    pub(crate) active_plugin_automation_bindings: usize,
    pub(crate) candidate_plugin_automation_bindings: usize,
    pub(crate) compiled_midi_routes: usize,
    pub(crate) pdc_revision: u64,
    pub(crate) zero_pdc: bool,
    pub(crate) paused_safety_services: usize,
    pub(crate) admitted_live_edit_endpoints: usize,
    pub(crate) parameter_edit_admission: u32,
    pub(crate) metronome_enabled: bool,
    pub(crate) recording: bool,
    pub(crate) midi_input_present: bool,
    pub(crate) midi_recording_present: bool,
    pub(crate) master_capture_present: bool,
    pub(crate) driven_automation_targets: usize,
    pub(crate) timeline_execution_failures: u64,
    pub(crate) fixed_quantum_bridge_gaps: u64,
    pub(crate) fixed_quantum_output_underflow_frames: u64,
    pub(crate) hooks_inactive: bool,
}

/// Owned by DspState; None is the default, including ordinary test execution.
/// There is a single writer, with no mutex, reservation, retry, or user callback.
pub(super) struct GenuineObservationBuffer {
    run_binding: [u8; 32],
    records: Box<[Option<GenuineSegmentObservation>]>,
    len: usize,
    overflowed: bool,
    pump_ordinal: u64,
    segment_ordinal: u64,
    lifecycle_records: [Option<GenuineLifecycleObservation>; GENUINE_LIFECYCLE_CAPACITY],
    lifecycle_len: usize,
    lifecycle_drained: usize,
    lifecycle_observed_count: u64,
    lifecycle_overflowed: bool,
    lifecycle_inconsistent_pair: bool,
    lifecycle_mutation: Option<GenuineLifecycleMutation>,
    lifecycle_operand: Option<(GenuineLifecycleKind, Option<GenuineLifecycleEndpoint>)>,
}

impl GenuineObservationBuffer {
    fn new(run_binding: [u8; 32]) -> Self {
        Self {
            run_binding,
            records: vec![None; GENUINE_SEGMENT_CAPACITY].into_boxed_slice(),
            len: 0,
            overflowed: false,
            pump_ordinal: 0,
            segment_ordinal: 0,
            lifecycle_records: [None; GENUINE_LIFECYCLE_CAPACITY],
            lifecycle_len: 0,
            lifecycle_drained: 0,
            lifecycle_observed_count: 0,
            lifecycle_overflowed: false,
            lifecycle_inconsistent_pair: false,
            lifecycle_mutation: None,
            lifecycle_operand: None,
        }
    }

    fn push(&mut self, mut record: GenuineSegmentObservation) {
        record.pump_ordinal = self.pump_ordinal;
        record.segment_ordinal = self.segment_ordinal;
        self.segment_ordinal = self.segment_ordinal.saturating_add(1);
        if self.len == self.records.len() {
            self.overflowed = true;
            return;
        }
        self.records[self.len] = Some(record);
        self.len += 1;
    }

    fn push_lifecycle(&mut self, mut record: GenuineLifecycleObservation) {
        record.run_binding = self.run_binding;
        record.pump_ordinal = self.pump_ordinal;
        record.record_ordinal = self.lifecycle_observed_count;
        self.lifecycle_observed_count = self.lifecycle_observed_count.saturating_add(1);
        if self.lifecycle_len == GENUINE_LIFECYCLE_CAPACITY {
            self.lifecycle_overflowed = true;
            return;
        }
        self.lifecycle_records[self.lifecycle_len] = Some(record);
        self.lifecycle_len += 1;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // Bounded raw evidence retains fields beyond each profile\'s selected oracle.
pub(crate) struct GenuineDelayObservation {
    pub(crate) current: u32,
    pub(crate) target: u32,
    pub(crate) frames_since_reset: Option<u64>,
}

impl GenuineDelayObservation {
    fn from_line(line: &StereoDelayLine) -> Self {
        Self {
            current: line.current_delay_samples(),
            target: line.target_delay_samples(),
            frames_since_reset: (line.maximum_delay_samples() > 0)
                .then(|| line.test_history_at_tap(1).0),
        }
    }
}

#[derive(Clone, Debug)]
#[allow(dead_code)] // Bounded raw evidence retains fields beyond each profile\'s selected oracle.
pub(crate) struct GenuineEndpointObservation {
    pub(crate) endpoint_id: u64,
    pub(crate) channel_id: Option<u32>,
    pub(crate) plugin_instance_id: Option<u64>,
    pub(crate) mixer_track: usize,
    pub(crate) project_session: u64,
    pub(crate) manifest: PluginEndpointManifest,
    pub(crate) coherent_latency: Option<PluginLatencySnapshot>,
    pub(crate) exact_snapshot: Option<PluginEndpointSnapshot>,
    pub(crate) fixed: crate::fixed_quantum::GenuineFixedQuantumSnapshot,
    pub(crate) staged_system_events: usize,
    pub(crate) staged_timeline_events: usize,
    pub(crate) staged_live_events: usize,
    pub(crate) partial_system_events: usize,
    pub(crate) partial_timeline_events: usize,
    pub(crate) partial_live_events: usize,
    pub(crate) partial_total_events: usize,
    pub(crate) admitted_live_edit_markers: u8,
    pub(crate) stopped_epoch_safety_epoch: Option<u64>,
    pub(crate) stopped_epoch_safety_remaining_frames: usize,
    pub(crate) stopped_epoch_safety_failed: bool,
    pub(crate) live_lane_faulted: bool,
    pub(crate) suppress_output_frames: usize,
    pub(crate) pdc_delay: Option<GenuineDelayObservation>,
    pub(crate) hooks_inactive: bool,
}

#[derive(Clone, Debug)]
#[allow(dead_code)] // Bounded raw evidence retains fields beyond each profile\'s selected oracle.
pub(crate) struct GenuineAssetObservation {
    pub(crate) slot: usize,
    pub(crate) id: u64,
    /// Actual registered allocation; Arc::ptr_eq is meaningful across conversion/Redo.
    /// Holding this clone does not pump, decode or replace the registered asset.
    pub(crate) samples: Arc<[f32]>,
    pub(crate) sample_rate: u32,
    pub(crate) channels: u16,
    pub(crate) frames: usize,
}

#[derive(Clone, Copy, Debug)]
#[allow(dead_code)] // Bounded raw evidence retains fields beyond each profile\'s selected oracle.
pub(crate) struct GenuineAudioVoiceObservation {
    pub(crate) slot: usize,
    pub(crate) clip_id: u64,
    pub(crate) asset_slot: usize,
    pub(crate) source_position: f64,
    pub(crate) gain: f32,
    pub(crate) mixer_track: usize,
    pub(crate) timeline_asset_id: Option<u64>,
    pub(crate) timeline_start_frame: u64,
    pub(crate) timeline_clip_end_frame: u64,
    pub(crate) timeline_stop_frame: u64,
    pub(crate) timeline_frame: u64,
    pub(crate) fades: CompiledClipFades,
    pub(crate) timeline_source_root_frame: u64,
    pub(crate) timeline_source_elapsed_frames: i64,
}

#[derive(Clone, Copy, Debug)]
#[allow(dead_code)] // Bounded raw evidence retains fields beyond each profile\'s selected oracle.
pub(crate) struct GenuineNativeVoiceObservation {
    pub(crate) slot: usize,
    pub(crate) mixer_track: usize,
    pub(crate) timeline_note_id: Option<u64>,
    pub(crate) timeline_channel_id: Option<u32>,
}

#[derive(Clone, Copy, Debug)]
#[allow(dead_code)] // Bounded raw evidence retains fields beyond each profile\'s selected oracle.
pub(crate) struct GenuineRouteDelayObservation {
    pub(crate) runtime_slot: usize,
    pub(crate) route_id: u64,
    pub(crate) current: u32,
    pub(crate) target: u32,
    pub(crate) frames_since_reset: Option<u64>,
}

#[derive(Clone, Debug)]
#[allow(dead_code)] // Bounded raw evidence retains fields beyond each profile\'s selected oracle.
pub(crate) struct GenuineCallbackSnapshot {
    pub(crate) engine_session: u64,
    pub(crate) project_session: u64,
    pub(crate) run_binding: Option<[u8; 32]>,
    pub(crate) observation_overflowed: bool,
    pub(crate) lifecycle_observed_count: u64,
    pub(crate) lifecycle_overflowed: bool,
    pub(crate) lifecycle_inconsistent_pair: bool,
    pub(crate) pump_ordinal: u64,
    pub(crate) start_frame: u64,
    pub(crate) device_frame: u64,
    pub(crate) playing: bool,
    pub(crate) generation: Option<u64>,
    pub(crate) active_epoch: Option<u64>,
    pub(crate) epoch: u64,
    pub(crate) request_id: u64,
    pub(crate) loop_enabled: bool,
    pub(crate) loop_start_frame: u64,
    pub(crate) loop_end_frame: u64,
    pub(crate) loop_count: u64,
    pub(crate) loop_token: Option<u64>,
    pub(crate) pending_audio_commands: usize,
    pub(crate) pending_timeline_commands: usize,
    pub(crate) pending_timeline_activation: bool,
    pub(crate) timeline_stats: Option<TimelineRuntimeStats>,
    pub(crate) fault: Option<PluginProcessingFault>,
    pub(crate) suspended: bool,
    pub(crate) timing: PreparedPluginTimingPlan,
    pub(crate) timing_activation: Option<PreparedPluginTimingPlan>,
    pub(crate) endpoints: Vec<GenuineEndpointObservation>,
    pub(crate) assets: Vec<GenuineAssetObservation>,
    pub(crate) native_voices: Vec<GenuineNativeVoiceObservation>,
    pub(crate) audio_voices: Vec<GenuineAudioVoiceObservation>,
    pub(crate) generator_notes: usize,
    pub(crate) sample_rate: f32,
    pub(crate) master_gain: f32,
    pub(crate) master_pan: f32,
    pub(crate) track_gains: [f32; TRACK_COUNT],
    pub(crate) track_pans: [f32; TRACK_COUNT],
    pub(crate) track_muted: [bool; TRACK_COUNT],
    pub(crate) track_solo: [bool; TRACK_COUNT],
    pub(crate) click_envelope: f32,
    pub(crate) graph_identity: Option<(u64, u64, u64)>,
    pub(crate) graph_was_activated: bool,
    pub(crate) graph_binding_exact: bool,
    pub(crate) channel_binding_revision: Option<u64>,
    pub(crate) channel_binding_epoch: Option<u64>,
    pub(crate) plugin_topology_revision: u64,
    pub(crate) plugin_topology_replan_pending: bool,
    pub(crate) active_plugin_automation_bindings: usize,
    pub(crate) candidate_plugin_automation_bindings: usize,
    pub(crate) graph_fingerprint: u64,
    pub(crate) nodes: Vec<CompiledMixerNode>,
    pub(crate) routes: Vec<CompiledMixerRoute>,
    pub(crate) graph_topological_runtime_slots: Vec<u8>,
    pub(crate) pdc_revision: u64,
    /// Dormant legacy metadata is retained verbatim, even after graph activation.
    pub(crate) legacy_pdc: PdcPlan,
    pub(crate) legacy_pdc_active: bool,
    /// Entire plan, including every node/route/generator and diagnostics.
    pub(crate) graph_pdc: Option<GraphPdcPlan>,
    /// Every allocated raw line remains visible; only marked slots feed the
    /// currently selected renderer. Dormant targets need not be zero.
    pub(crate) raw_delays: Vec<GenuineDelayObservation>,
    pub(crate) raw_delay_active: [bool; TRACK_COUNT],
    pub(crate) route_delay_bank_fingerprint: Option<u64>,
    pub(crate) route_delay_bank_route_count: Option<usize>,
    pub(crate) route_delays: Vec<GenuineRouteDelayObservation>,
    pub(crate) zero_pdc: bool,
    pub(crate) paused_safety_services: usize,
    pub(crate) paused_safety_remaining_frames: usize,
    pub(crate) admitted_live_edit_endpoints: usize,
    pub(crate) parameter_edit_admission: u32,
    pub(crate) midi_input_present: bool,
    pub(crate) midi_recording_present: bool,
    pub(crate) master_capture_present: bool,
    pub(crate) driven_automation_targets: usize,
    pub(crate) compiled_plugin_routes: usize,
    pub(crate) compiled_audio_clips: usize,
    pub(crate) compiled_midi_routes: usize,
    pub(crate) plugin_control_bindings: usize,
    pub(crate) timeline_execution_failures: u64,
    pub(crate) timeline_missing_assets: u64,
    pub(crate) fixed_quantum_bridge_gaps: u64,
    pub(crate) fixed_quantum_output_underflow_frames: u64,
    pub(crate) hooks_inactive: bool,
}

impl PreparedFixedEndpoint {
    pub(super) fn genuine_lifecycle_endpoint(
        &self,
        channel_id: u32,
        endpoint_id: u64,
        plugin_instance_id: u64,
        mixer_track: usize,
    ) -> GenuineLifecycleEndpoint {
        GenuineLifecycleEndpoint {
            channel_id,
            endpoint_id,
            plugin_instance_id,
            mixer_track,
            project_session: self.project_session,
            worker_id: self.adapter.genuine_snapshot().bridge.worker_id,
            epoch: self.epoch(),
        }
    }

    fn genuine_hooks_inactive(&self) -> bool {
        self.fresh_snapshot_script_len == 0
            && self.fresh_snapshot_script_cursor == 0
            && self.fresh_snapshot_script.iter().all(Option::is_none)
    }

    fn genuine_endpoint_observation(
        &self,
        endpoint_id: u64,
        channel_id: Option<u32>,
        plugin_instance_id: Option<u64>,
        mixer_track: usize,
        suppress_output_frames: usize,
        pdc_delay: Option<&StereoDelayLine>,
    ) -> GenuineEndpointObservation {
        GenuineEndpointObservation {
            endpoint_id,
            channel_id,
            plugin_instance_id,
            mixer_track,
            project_session: self.project_session,
            manifest: self.manifest,
            coherent_latency: self.coherent_latency,
            exact_snapshot: self.cached_exact_endpoint_snapshot(),
            fixed: self.adapter.genuine_snapshot(),
            staged_system_events: self.events.scratch.system.pending_len,
            staged_timeline_events: self.events.scratch.timeline.pending_len,
            staged_live_events: self.events.scratch.live.pending_len,
            partial_system_events: self.partial_quantum_usage.system,
            partial_timeline_events: self.partial_quantum_usage.timeline,
            partial_live_events: self.partial_quantum_usage.live,
            partial_total_events: self.partial_quantum_usage.total,
            admitted_live_edit_markers: self.admitted_live_edit_markers,
            stopped_epoch_safety_epoch: self.stopped_epoch_safety.map(|service| service.epoch),
            stopped_epoch_safety_remaining_frames: self
                .stopped_epoch_safety
                .map_or(0, |service| service.remaining_frames),
            stopped_epoch_safety_failed: self
                .stopped_epoch_safety
                .is_some_and(|service| service.failed),
            live_lane_faulted: self.events.scratch.live_faulted,
            suppress_output_frames,
            pdc_delay: pdc_delay.map(GenuineDelayObservation::from_line),
            hooks_inactive: self.genuine_hooks_inactive(),
        }
    }
}

impl DspState {
    pub(super) fn genuine_generator_identity(
        &self,
        channel_id: u32,
    ) -> Option<GenuineLifecycleEndpoint> {
        let slot = self.generator_endpoints[self.find_generator_slot(channel_id)?].as_ref()?;
        Some(slot.endpoint.genuine_lifecycle_endpoint(
            slot.channel_id,
            slot.endpoint_id,
            slot.plugin_instance_id,
            slot.mixer_track,
        ))
    }

    fn genuine_lifecycle_state(&self) -> GenuineLifecycleState {
        let runtime = self.timeline_runtime.as_ref();
        GenuineLifecycleState {
            engine_session: self.engine_session,
            project_session: self.project_session.load(Ordering::Acquire),
            transport_epoch: self.transport_epoch,
            legacy_request_barrier: self.callback_transport_request_id,
            playing: self.callback_transport_playing,
            generation: runtime.and_then(RealtimeTimelineRuntime::active_revision),
            active_epoch: runtime.and_then(RealtimeTimelineRuntime::active_epoch),
            plugin_topology_revision: self.plugin_topology_revision,
            plugin_topology_replan_pending: self.plugin_topology_replan_pending,
            timeline_execution_failures: self.timeline_execution_failures,
            fault: self.plugin_fault,
            needs_resync: runtime.is_some_and(|runtime| runtime.stats().ownership_needs_resync),
            suspended: self.plugin_processing_suspended(),
            graph_was_activated: self.mixer_graph_was_activated,
            graph_binding_exact: self.mixer_graph_binding_is_exact(),
            graph_identity: self
                .mixer_graph_identity
                .map(|identity| (identity.revision, identity.epoch, identity.fingerprint)),
            channel_binding_revision: self.timeline_channel_revision,
            channel_binding_epoch: self.timeline_channel_epoch,
            active_plugin_automation_bindings: self.timeline_plugin_automation_bindings.len,
            candidate_plugin_automation_bindings: self
                .timeline_activation_plugin_automation_bindings
                .len,
            compiled_midi_routes: runtime
                .and_then(RealtimeTimelineRuntime::active_timeline)
                .map_or(0, |timeline| timeline.midi_port_routes().len()),
            generator_endpoints: self.generator_endpoints.iter().flatten().count(),
            insert_endpoints: self.insert_endpoints.iter().flatten().count(),
            pdc_revision: self.pdc_plan_revision,
        }
    }

    pub(super) fn genuine_lifecycle_operand(
        &mut self,
        kind: GenuineLifecycleKind,
        affected: Option<GenuineLifecycleEndpoint>,
    ) {
        if let Some(observer) = self.test_genuine_observation.as_mut() {
            if observer.lifecycle_operand.is_some() || observer.lifecycle_mutation.is_some() {
                observer.lifecycle_inconsistent_pair = true;
            }
            observer.lifecycle_operand = Some((kind, affected));
        }
    }

    /// Called immediately before the actual accepted kernel's first topology mutation.
    pub(super) fn genuine_begin_lifecycle_mutation(
        &mut self,
        kind: GenuineLifecycleKind,
        affected: Option<GenuineLifecycleEndpoint>,
        replaced: Option<GenuineLifecycleEndpoint>,
        stamp: Option<GeneratorRemovalStamp>,
    ) {
        if self.test_genuine_observation.is_none() {
            return;
        }
        let pre = self.genuine_lifecycle_state();
        let observer = self.test_genuine_observation.as_mut().unwrap();
        let Some(affected) = affected else {
            observer.lifecycle_inconsistent_pair = true;
            return;
        };
        if observer.lifecycle_mutation.is_some() {
            observer.lifecycle_inconsistent_pair = true;
        }
        observer.lifecycle_mutation = Some(GenuineLifecycleMutation {
            kind,
            affected,
            replaced,
            pre,
            stamp,
            pump_ordinal: observer.pump_ordinal,
        });
    }

    /// Taps the immutable event and the outcome of its sole original push.
    pub(super) fn genuine_finish_lifecycle(
        &mut self,
        event: GeneratorEndpointEvent,
        producer_present: bool,
        enqueue_success: bool,
    ) {
        if self.test_genuine_observation.is_none() {
            return;
        }
        let (kind, event_channel) = match event {
            GeneratorEndpointEvent::Installed { channel_id, .. } => {
                (GenuineLifecycleKind::Install, Some(channel_id))
            }
            GeneratorEndpointEvent::StampedRemoved { stamp, .. } => {
                (GenuineLifecycleKind::StampedRemove, Some(stamp.channel_id))
            }
            GeneratorEndpointEvent::Removed { channel_id, .. } => {
                (GenuineLifecycleKind::Remove, Some(channel_id))
            }
            GeneratorEndpointEvent::Cleared { .. } => (GenuineLifecycleKind::Clear, None),
            GeneratorEndpointEvent::RouteSet { channel_id, .. } => {
                (GenuineLifecycleKind::RouteSet, Some(channel_id))
            }
        };
        let current_affected =
            event_channel.and_then(|channel| self.genuine_generator_identity(channel));
        let post = self.genuine_lifecycle_state();
        let observer = self.test_genuine_observation.as_mut().unwrap();
        let operand = observer.lifecycle_operand.take();
        let mutation = observer.lifecycle_mutation.take();
        let accepted_event = matches!(
            event,
            GeneratorEndpointEvent::Installed { success: true, .. }
                | GeneratorEndpointEvent::StampedRemoved { result: Ok(()), .. }
                | GeneratorEndpointEvent::Removed {
                    endpoint_id: Some(_),
                    plugin_instance_id: Some(_),
                    ..
                }
        );
        let pair_matches = mutation.is_some_and(|pair| {
            pair.kind == kind
                && pair.pump_ordinal == observer.pump_ordinal
                && match event {
                    GeneratorEndpointEvent::Installed {
                        channel_id,
                        endpoint_id,
                        plugin_instance_id,
                        mixer_track,
                        replaced_endpoint_id,
                        replaced_plugin_instance_id,
                        success,
                    } => {
                        success
                            && pair.stamp.is_none()
                            && pair.affected.channel_id == channel_id
                            && pair.affected.endpoint_id == endpoint_id
                            && pair.affected.plugin_instance_id == plugin_instance_id
                            && pair.affected.mixer_track == mixer_track
                            && pair.replaced.map(|value| value.endpoint_id) == replaced_endpoint_id
                            && pair.replaced.map(|value| value.plugin_instance_id)
                                == replaced_plugin_instance_id
                    }
                    GeneratorEndpointEvent::StampedRemoved { stamp, result } => {
                        result.is_ok()
                            && pair.stamp == Some(stamp)
                            && pair.replaced.is_none()
                            && pair.affected.channel_id == stamp.channel_id
                            && pair.affected.endpoint_id == stamp.endpoint_id
                            && pair.affected.plugin_instance_id == stamp.plugin_instance_id
                            && pair.affected.project_session == stamp.project_session
                            && pair.affected.epoch == stamp.stopped_epoch
                    }
                    GeneratorEndpointEvent::Removed {
                        channel_id,
                        endpoint_id,
                        plugin_instance_id,
                    } => {
                        pair.stamp.is_none()
                            && pair.replaced.is_none()
                            && pair.affected.channel_id == channel_id
                            && Some(pair.affected.endpoint_id) == endpoint_id
                            && Some(pair.affected.plugin_instance_id) == plugin_instance_id
                    }
                    _ => false,
                }
        });
        if accepted_event != pair_matches
            || mutation.is_some() != pair_matches
            || operand.is_some_and(|(operand_kind, _)| operand_kind != kind)
        {
            observer.lifecycle_inconsistent_pair = true;
        }
        observer.push_lifecycle(GenuineLifecycleObservation {
            run_binding: observer.run_binding,
            pump_ordinal: observer.pump_ordinal,
            record_ordinal: 0,
            kind,
            event,
            producer_present,
            enqueue_success,
            affected: mutation
                .map(|pair| pair.affected)
                .or_else(|| operand.and_then(|(_, affected)| affected))
                .or(current_affected),
            replaced: mutation.and_then(|pair| pair.replaced),
            pre: mutation.filter(|_| pair_matches).map(|pair| pair.pre),
            post,
        });
    }

    fn genuine_stopped_epoch_safety(&self) -> impl Iterator<Item = StoppedEpochSafetyService> + '_ {
        self.generator_endpoints
            .iter()
            .flatten()
            .filter_map(|slot| slot.endpoint.stopped_epoch_safety)
            .chain(
                self.insert_endpoints
                    .iter()
                    .flatten()
                    .filter_map(|slot| slot.endpoint.stopped_epoch_safety),
            )
    }

    fn genuine_hooks_inactive(&self) -> bool {
        crate::realtime_test_alloc::genuine_guard_inactive()
            && self.test_restoration_reset_arm.is_none()
            && self.test_restoration_reset_observation.is_none()
            && self
                .insert_endpoints
                .iter()
                .flatten()
                .all(|slot| slot.endpoint.genuine_hooks_inactive())
            && self
                .generator_endpoints
                .iter()
                .flatten()
                .all(|slot| slot.endpoint.genuine_hooks_inactive())
    }

    /// Includes actual taps, not only desired plan delays. A current nonzero tap
    /// crossfading toward zero is still nonzero evidence. Missing graph/bank fails.
    /// Only contributing graph paths qualify here; dormant legacy metadata and
    /// inactive raw-line targets are retained separately in the full snapshot.
    fn genuine_zero_pdc(&self) -> bool {
        let Some(plan) = self.graph_pdc_plan.as_ref().as_ref() else {
            return false;
        };
        let Some(bank) = self
            .timeline_runtime
            .as_ref()
            .and_then(RealtimeTimelineRuntime::active_mixer_delay_bank)
        else {
            return false;
        };
        self.mixer_graph_was_activated
            && self.mixer_graph_binding_is_exact()
            && self.mixer_graph_identity.is_some_and(|identity| {
                self.timeline_runtime
                    .as_ref()
                    .and_then(RealtimeTimelineRuntime::active_revision)
                    == Some(identity.revision)
                    && self
                        .timeline_runtime
                        .as_ref()
                        .and_then(RealtimeTimelineRuntime::active_epoch)
                        == Some(identity.epoch)
                    && identity.fingerprint == self.mixer_graph_plan.fingerprint()
                    && Some(identity.fingerprint) == plan.graph_fingerprint()
                    && identity.fingerprint == bank.graph_fingerprint()
            })
            && plan.node_count() == self.mixer_graph_plan.node_count()
            && plan.main_input_count() == self.mixer_graph_plan.route_count()
            && plan.master_output_latency_samples() == 0
            && !plan.diagnostics().has_arithmetic_overflow()
            && !plan.has_clamped_delays()
            && plan.nodes().all(|node| {
                node.stage_latency_samples == 0
                    && node.join_latency_samples == 0
                    && node.output_latency_samples == 0
                    && !node.output_latency_overflowed
                    && node.raw_source_delay.requested_samples() == 0
                    && node.raw_source_delay.applied_samples() == 0
            })
            && plan.main_inputs().all(|route| {
                route.source_latency_samples == 0
                    && route.destination_join_latency_samples == 0
                    && route.delay.requested_samples() == 0
                    && route.delay.applied_samples() == 0
            })
            && plan.generators().all(|generator| {
                generator.path_latency_samples == 0
                    && generator.destination_join_latency_samples == 0
                    && generator.delay.requested_samples() == 0
                    && generator.delay.applied_samples() == 0
            })
            && plan.nodes().all(|node| {
                self.pdc_raw_track_delays
                    .get(node.runtime_slot)
                    .is_some_and(|line| {
                        line.current_delay_samples() == 0 && line.target_delay_samples() == 0
                    })
            })
            && self.generator_endpoints.iter().flatten().all(|slot| {
                slot.pdc_delay.current_delay_samples() == 0
                    && slot.pdc_delay.target_delay_samples() == 0
            })
            && bank.route_count() == self.mixer_graph_plan.route_count()
            && (0..MIXER_GRAPH_MAX_EDGES).all(|slot| match bank.route_id_for_runtime_slot(slot) {
                Some(id) => {
                    self.mixer_graph_plan
                        .route_at_runtime_slot(slot as u8)
                        .is_some_and(|route| route.id == id)
                        && bank.current_delay_samples(slot) == Some(0)
                        && bank.target_delay_samples(slot) == Some(0)
                }
                None => self
                    .mixer_graph_plan
                    .route_at_runtime_slot(slot as u8)
                    .is_none(),
            })
    }

    fn genuine_segment_state(&self, status: &AudioStatus) -> GenuineSegmentState {
        let runtime = self.timeline_runtime.as_ref();
        let compiled = runtime.and_then(RealtimeTimelineRuntime::active_timeline);
        GenuineSegmentState {
            generator_endpoints: self.generator_endpoints.iter().flatten().count(),
            insert_endpoints: self.insert_endpoints.iter().flatten().count(),
            native_voices: self.voices.iter().filter(|voice| voice.active).count(),
            audio_voices: self
                .audio_voices
                .iter()
                .filter(|voice| voice.active)
                .count(),
            generator_notes: self.timeline_generator_notes.iter().flatten().count(),
            fault: self.plugin_fault,
            suspended: self.plugin_processing_suspended(),
            needs_resync: runtime.is_some_and(|runtime| runtime.stats().ownership_needs_resync),
            graph_identity: self
                .mixer_graph_identity
                .map(|identity| (identity.revision, identity.epoch, identity.fingerprint)),
            graph_was_activated: self.mixer_graph_was_activated,
            graph_binding_exact: self.mixer_graph_binding_is_exact(),
            channel_binding_revision: self.timeline_channel_revision,
            channel_binding_epoch: self.timeline_channel_epoch,
            plugin_topology_revision: self.plugin_topology_revision,
            plugin_topology_replan_pending: self.plugin_topology_replan_pending,
            active_plugin_automation_bindings: self.timeline_plugin_automation_bindings.len,
            candidate_plugin_automation_bindings: self
                .timeline_activation_plugin_automation_bindings
                .len,
            compiled_midi_routes: compiled.map_or(0, |timeline| timeline.midi_port_routes().len()),
            pdc_revision: self.pdc_plan_revision,
            zero_pdc: self.genuine_zero_pdc(),
            paused_safety_services: self.paused_midi_safety.iter().flatten().count()
                + self.genuine_stopped_epoch_safety().count(),
            admitted_live_edit_endpoints: self.admitted_live_edit_endpoint_count,
            parameter_edit_admission: self
                .parameter_edit_callback_admission
                .load(Ordering::Acquire),
            metronome_enabled: status.metronome_enabled.load(Ordering::Acquire),
            recording: status.recording.load(Ordering::Acquire),
            midi_input_present: self.midi_input.is_some(),
            midi_recording_present: self.midi_recording.is_some()
                || self.pending_midi_recording_start.is_some(),
            master_capture_present: self.master_capture.is_some(),
            driven_automation_targets: compiled
                .map_or(0, |timeline| timeline.driven_automation_targets().len()),
            timeline_execution_failures: self.timeline_execution_failures,
            fixed_quantum_bridge_gaps: self.fixed_quantum_bridge_gaps,
            fixed_quantum_output_underflow_frames: self.fixed_quantum_output_underflow_frames,
            hooks_inactive: self.genuine_hooks_inactive(),
        }
    }

    pub(super) fn genuine_begin_pump(&mut self) {
        if let Some(observer) = self.test_genuine_observation.as_mut() {
            if observer.lifecycle_mutation.is_some() || observer.lifecycle_operand.is_some() {
                observer.lifecycle_inconsistent_pair = true;
            }
            observer.pump_ordinal = observer.pump_ordinal.saturating_add(1);
        }
    }

    pub(super) fn genuine_segment_start(
        &self,
        status: &AudioStatus,
        transport: &RealtimeTransport,
        frames: usize,
        device_offset: usize,
    ) -> Option<GenuineSegmentObservation> {
        self.test_genuine_observation.as_ref()?;
        let runtime = self.timeline_runtime.as_ref();
        let before = self.genuine_segment_state(status);
        Some(GenuineSegmentObservation {
            pump_ordinal: 0,
            segment_ordinal: 0,
            start_frame: transport.timeline_frame,
            frames,
            device_offset,
            device_frame: transport.device_frame,
            playing: transport.request.playing,
            playing_after_render: transport.request.playing,
            timeline_ready: false,
            timeline_render_active: false,
            timeline_render_active_after: false,
            generation: runtime.and_then(RealtimeTimelineRuntime::active_revision),
            active_epoch: runtime.and_then(RealtimeTimelineRuntime::active_epoch),
            epoch: transport.epoch,
            request_id: transport.request.request_id,
            discontinuity_id: transport.request.discontinuity_id,
            loop_count: transport.loop_count,
            loop_enabled: transport.request.loop_enabled,
            loop_start_frame: transport.request.loop_start_frame,
            loop_end_frame: transport.request.loop_end_frame,
            loop_token: runtime.and_then(RealtimeTimelineRuntime::installed_loop_token),
            disposition: GenuineSegmentDisposition::Rendered,
            before,
            after: before,
        })
    }

    pub(super) fn genuine_finish_segment(
        &mut self,
        status: &AudioStatus,
        transport: &RealtimeTransport,
        record: Option<GenuineSegmentObservation>,
        timeline_ready: bool,
        disposition: GenuineSegmentDisposition,
    ) {
        let Some(mut record) = record else {
            return;
        };
        record.timeline_ready = timeline_ready;
        record.timeline_render_active_after = self.timeline_plan_render_active;
        record.playing_after_render = transport.request.playing;
        record.disposition = disposition;
        record.after = self.genuine_segment_state(status);
        if let Some(observer) = self.test_genuine_observation.as_mut() {
            observer.push(record);
        }
    }
}

impl TestAudioCallback {
    /// Call once on the driver thread before the first pump. The caller validates
    /// the actual source/run pins; this identity binds the resulting records only.
    pub(crate) fn enable_genuine_observation(
        &mut self,
        run_binding: [u8; 32],
    ) -> Result<(), &'static str> {
        if run_binding == [0; 32] {
            return Err("genuine observation requires a nonzero run binding");
        }
        if self.dsp.test_genuine_observation.is_some() {
            return Err("genuine observation is already enabled");
        }
        if self.last_output_frames != 0 {
            return Err("genuine observation must precede the first pump");
        }
        if !self.genuine_hooks_inactive() {
            return Err("behavior-changing callback test hooks are active");
        }
        self.dsp.test_genuine_observation =
            Some(Box::new(GenuineObservationBuffer::new(run_binding)));
        Ok(())
    }

    pub(crate) fn genuine_hooks_inactive(&self) -> bool {
        self.dsp.genuine_hooks_inactive()
    }

    pub(crate) fn genuine_observation_overflowed(&self) -> bool {
        self.dsp
            .test_genuine_observation
            .as_ref()
            .is_some_and(|observer| {
                observer.overflowed
                    || observer.lifecycle_overflowed
                    || observer.lifecycle_inconsistent_pair
            })
    }

    /// Driver-only drain cursor. Stored records and the lifetime count never reset.
    pub(crate) fn take_genuine_lifecycle_receipts(&mut self) -> Vec<GenuineLifecycleObservation> {
        let Some(observer) = self.dsp.test_genuine_observation.as_mut() else {
            return Vec::new();
        };
        let records = observer.lifecycle_records
            [observer.lifecycle_drained..observer.lifecycle_len]
            .iter()
            .flatten()
            .copied()
            .collect();
        observer.lifecycle_drained = observer.lifecycle_len;
        records
    }

    /// Driver-thread allocation only. Draining never clears the sticky overflow flag.
    pub(crate) fn take_genuine_segments(&mut self) -> Vec<GenuineSegmentObservation> {
        let Some(observer) = self.dsp.test_genuine_observation.as_mut() else {
            return Vec::new();
        };
        let mut records = Vec::with_capacity(observer.len);
        for index in 0..observer.len {
            if let Some(record) = observer.records[index].take() {
                records.push(record);
            }
        }
        observer.len = 0;
        records
    }

    /// Detailed read-only driver-thread inspection. Never call from an audio callback.
    pub(crate) fn genuine_snapshot(&self) -> GenuineCallbackSnapshot {
        let dsp = &self.dsp;
        let runtime = dsp.timeline_runtime.as_ref();
        let compiled = runtime.and_then(RealtimeTimelineRuntime::active_timeline);
        let bank = runtime.and_then(RealtimeTimelineRuntime::active_mixer_delay_bank);
        let endpoints = dsp
            .generator_endpoints
            .iter()
            .flatten()
            .map(|slot| {
                slot.endpoint.genuine_endpoint_observation(
                    slot.endpoint_id,
                    Some(slot.channel_id),
                    Some(slot.plugin_instance_id),
                    slot.mixer_track,
                    slot.suppress_output_frames,
                    Some(&slot.pdc_delay),
                )
            })
            .chain(
                dsp.insert_endpoints
                    .iter()
                    .enumerate()
                    .filter_map(|(track, slot)| {
                        slot.as_ref().map(|slot| {
                            slot.endpoint.genuine_endpoint_observation(
                                slot.endpoint_id,
                                None,
                                None,
                                track,
                                slot.suppress_output_frames,
                                None,
                            )
                        })
                    }),
            )
            .collect();
        GenuineCallbackSnapshot {
            engine_session: dsp.engine_session,
            project_session: dsp.project_session.load(Ordering::Acquire),
            run_binding: dsp
                .test_genuine_observation
                .as_ref()
                .map(|observer| observer.run_binding),
            observation_overflowed: self.genuine_observation_overflowed(),
            lifecycle_observed_count: dsp
                .test_genuine_observation
                .as_ref()
                .map_or(0, |observer| observer.lifecycle_observed_count),
            lifecycle_overflowed: dsp
                .test_genuine_observation
                .as_ref()
                .is_some_and(|observer| observer.lifecycle_overflowed),
            lifecycle_inconsistent_pair: dsp
                .test_genuine_observation
                .as_ref()
                .is_some_and(|observer| observer.lifecycle_inconsistent_pair),
            pump_ordinal: dsp
                .test_genuine_observation
                .as_ref()
                .map_or(0, |observer| observer.pump_ordinal),
            start_frame: self.transport.timeline_frame,
            device_frame: self.transport.device_frame,
            playing: self.transport.request.playing,
            generation: runtime.and_then(RealtimeTimelineRuntime::active_revision),
            active_epoch: runtime.and_then(RealtimeTimelineRuntime::active_epoch),
            epoch: self.transport.epoch,
            request_id: self.transport.request.request_id,
            loop_enabled: self.transport.request.loop_enabled,
            loop_start_frame: self.transport.request.loop_start_frame,
            loop_end_frame: self.transport.request.loop_end_frame,
            loop_count: self.transport.loop_count,
            loop_token: runtime.and_then(RealtimeTimelineRuntime::installed_loop_token),
            pending_audio_commands: self.commands.slots(),
            pending_timeline_commands: runtime.map_or(0, RealtimeTimelineRuntime::pending_commands),
            pending_timeline_activation: dsp.pending_timeline_transport_activation().is_some(),
            timeline_stats: runtime.map(RealtimeTimelineRuntime::stats),
            fault: dsp.plugin_fault,
            suspended: dsp.plugin_processing_suspended(),
            timing: dsp.plugin_timing,
            timing_activation: dsp.plugin_timing_activation,
            endpoints,
            assets: dsp
                .audio_assets
                .iter()
                .enumerate()
                .filter_map(|(slot, asset)| {
                    asset
                        .samples
                        .as_ref()
                        .map(|samples| GenuineAssetObservation {
                            slot,
                            id: asset.id,
                            samples: Arc::clone(samples),
                            sample_rate: asset.sample_rate,
                            channels: asset.channels,
                            frames: asset.frames,
                        })
                })
                .collect(),
            native_voices: dsp
                .voices
                .iter()
                .enumerate()
                .filter(|(_, voice)| voice.active)
                .map(|(slot, voice)| GenuineNativeVoiceObservation {
                    slot,
                    mixer_track: voice.mixer_track,
                    timeline_note_id: voice.timeline_note_id,
                    timeline_channel_id: voice.timeline_channel_id,
                })
                .collect(),
            audio_voices: dsp
                .audio_voices
                .iter()
                .enumerate()
                .filter(|(_, voice)| voice.active)
                .map(|(slot, voice)| GenuineAudioVoiceObservation {
                    slot,
                    clip_id: voice.clip_id,
                    asset_slot: voice.asset_slot,
                    source_position: voice.source_position,
                    gain: voice.gain,
                    mixer_track: voice.mixer_track,
                    timeline_asset_id: voice.timeline_asset_id,
                    timeline_start_frame: voice.timeline_start_frame,
                    timeline_clip_end_frame: voice.timeline_clip_end_frame,
                    timeline_stop_frame: voice.timeline_stop_frame,
                    timeline_frame: voice.timeline_frame,
                    fades: voice.fades,
                    timeline_source_root_frame: voice.timeline_source_root_frame,
                    timeline_source_elapsed_frames: voice.timeline_source_elapsed_frames,
                })
                .collect(),
            generator_notes: dsp.timeline_generator_notes.iter().flatten().count(),
            sample_rate: dsp.sample_rate,
            master_gain: dsp.master,
            master_pan: dsp.master_pan,
            track_gains: dsp.track_gains,
            track_pans: dsp.track_pans,
            track_muted: dsp.track_muted,
            track_solo: dsp.track_solo,
            click_envelope: dsp.click_envelope,
            graph_identity: dsp
                .mixer_graph_identity
                .map(|identity| (identity.revision, identity.epoch, identity.fingerprint)),
            graph_was_activated: dsp.mixer_graph_was_activated,
            graph_binding_exact: dsp.mixer_graph_binding_is_exact(),
            channel_binding_revision: dsp.timeline_channel_revision,
            channel_binding_epoch: dsp.timeline_channel_epoch,
            plugin_topology_revision: dsp.plugin_topology_revision,
            plugin_topology_replan_pending: dsp.plugin_topology_replan_pending,
            active_plugin_automation_bindings: dsp.timeline_plugin_automation_bindings.len,
            candidate_plugin_automation_bindings: dsp
                .timeline_activation_plugin_automation_bindings
                .len,
            graph_fingerprint: dsp.mixer_graph_plan.fingerprint(),
            nodes: (0..MIXER_GRAPH_MAX_NODES)
                .filter_map(|slot| dsp.mixer_graph_plan.node_at_runtime_slot(slot as u8))
                .collect(),
            routes: (0..MIXER_GRAPH_MAX_EDGES)
                .filter_map(|slot| dsp.mixer_graph_plan.route_at_runtime_slot(slot as u8))
                .collect(),
            graph_topological_runtime_slots: dsp
                .mixer_graph_plan
                .topological_runtime_slots()
                .to_vec(),
            pdc_revision: dsp.pdc_plan_revision,
            legacy_pdc: dsp.pdc_plan.clone(),
            legacy_pdc_active: !dsp.mixer_graph_was_activated,
            graph_pdc: dsp.graph_pdc_plan.as_ref().clone(),
            raw_delays: dsp
                .pdc_raw_track_delays
                .iter()
                .map(GenuineDelayObservation::from_line)
                .collect(),
            raw_delay_active: std::array::from_fn(|slot| {
                !dsp.mixer_graph_was_activated
                    || dsp
                        .mixer_graph_plan
                        .node_at_runtime_slot(slot as u8)
                        .is_some()
            }),
            route_delay_bank_fingerprint: bank.map(PreparedMixerGraphDelayBank::graph_fingerprint),
            route_delay_bank_route_count: bank.map(PreparedMixerGraphDelayBank::route_count),
            route_delays: bank.map_or_else(Vec::new, |bank| {
                (0..MIXER_GRAPH_MAX_EDGES)
                    .filter_map(|runtime_slot| {
                        Some(GenuineRouteDelayObservation {
                            runtime_slot,
                            route_id: bank.route_id_for_runtime_slot(runtime_slot)?,
                            current: bank.current_delay_samples(runtime_slot)?,
                            target: bank.target_delay_samples(runtime_slot)?,
                            frames_since_reset: (bank.maximum_delay_samples() > 0)
                                .then(|| {
                                    bank.test_history_at_tap(runtime_slot, 1)
                                        .map(|value| value.0)
                                })
                                .flatten(),
                        })
                    })
                    .collect()
            }),
            zero_pdc: dsp.genuine_zero_pdc(),
            paused_safety_services: dsp.paused_midi_safety.iter().flatten().count()
                + dsp.genuine_stopped_epoch_safety().count(),
            paused_safety_remaining_frames: dsp
                .paused_midi_safety
                .iter()
                .flatten()
                .map(|service| service.remaining_frames)
                .chain(
                    dsp.genuine_stopped_epoch_safety()
                        .map(|service| service.remaining_frames),
                )
                .sum(),
            admitted_live_edit_endpoints: dsp.admitted_live_edit_endpoint_count,
            parameter_edit_admission: dsp
                .parameter_edit_callback_admission
                .load(Ordering::Acquire),
            midi_input_present: dsp.midi_input.is_some(),
            midi_recording_present: dsp.midi_recording.is_some()
                || dsp.pending_midi_recording_start.is_some(),
            master_capture_present: dsp.master_capture.is_some(),
            driven_automation_targets: compiled
                .map_or(0, |timeline| timeline.driven_automation_targets().len()),
            compiled_plugin_routes: compiled.map_or(0, |timeline| timeline.plugin_routes().len()),
            compiled_audio_clips: compiled.map_or(0, |timeline| timeline.audio_clips().len()),
            compiled_midi_routes: compiled.map_or(0, |timeline| timeline.midi_port_routes().len()),
            plugin_control_bindings: dsp.timeline_plugin_automation_bindings.len,
            timeline_execution_failures: dsp.timeline_execution_failures,
            timeline_missing_assets: dsp.timeline_missing_assets,
            fixed_quantum_bridge_gaps: dsp.fixed_quantum_bridge_gaps,
            fixed_quantum_output_underflow_frames: dsp.fixed_quantum_output_underflow_frames,
            hooks_inactive: self.genuine_hooks_inactive(),
        }
    }
}

impl GenuineEndpointObservation {
    /// Only callback-owned, unsubmitted work. An asynchronously processing block
    /// or completed output buffer is deliberately not a request for another pump.
    pub(crate) fn callback_work_pending(&self) -> bool {
        self.staged_system_events != 0
            || self.staged_timeline_events != 0
            || self.staged_live_events != 0
            || self.partial_total_events != 0
            || self.admitted_live_edit_markers != 0
            || self.stopped_epoch_safety_epoch.is_some()
            || self.fixed.input_phase_frames != 0
            || self.fixed.pending_event_count != 0
            || self.fixed.bridge.pending_inline_events != 0
    }

    /// Observed atomics, not a coherent ownership acknowledgment or promise
    /// against future submission. The unchanged capture input/epoch fence is
    /// authoritative. No extra worker query, SDK call, retry, or barrier is issued.
    pub(crate) fn input_work_quiescent(&self, epoch: u64) -> bool {
        let bridge = &self.fixed.bridge;
        !self.callback_work_pending()
            && self.hooks_inactive
            && !self.live_lane_faulted
            && self.fixed.epoch == epoch
            && bridge.stats.current_epoch == epoch
            && bridge.worker_epoch == epoch
            && bridge.input_pending == 0
            && bridge.worker_busy == 0
            && bridge.input_consumed == bridge.stats.submitted
            && bridge.operations_completed == bridge.stats.submitted
            && bridge.parameter_edits_admitted == 0
            && !bridge.capture_busy
    }
}

impl GenuineCallbackSnapshot {
    /// Does not include worker-only asynchronous settlement or output ownership.
    /// The driver still enforces its phase permission and exact identity before
    /// servicing this work. Fault/resync never grants an automatic retry.
    pub(crate) fn callback_work_pending(&self) -> bool {
        self.pending_audio_commands != 0
            || self.pending_timeline_commands != 0
            || self.pending_timeline_activation
            || self.paused_safety_services != 0
            || self.admitted_live_edit_endpoints != 0
            || self
                .endpoints
                .iter()
                .any(GenuineEndpointObservation::callback_work_pending)
    }

    /// Exact stopped identity plus safety/edit/input/submission settlement. A
    /// paused fallback or suspended/resync state cannot pass. Output FIFO/pre-roll
    /// ownership is reported but intentionally excluded from this predicate.
    pub(crate) fn paused_work_quiescent(&self, generation: u64, epoch: u64) -> bool {
        generation != 0
            && epoch != 0
            && !self.playing
            && self.start_frame == 0
            && self.generation == Some(generation)
            && self.active_epoch == Some(epoch)
            && self.epoch == epoch
            && self.fault.is_none()
            && !self.suspended
            && self.graph_binding_exact
            && self
                .timeline_stats
                .is_some_and(|stats| !stats.ownership_needs_resync)
            && !self.callback_work_pending()
            && self.parameter_edit_admission == 0
            && self.native_voices.is_empty()
            && self.audio_voices.is_empty()
            && self.generator_notes == 0
            && !self.midi_input_present
            && !self.midi_recording_present
            && !self.master_capture_present
            && self.hooks_inactive
            && !self.observation_overflowed
            && self
                .endpoints
                .iter()
                .all(|endpoint| endpoint.input_work_quiescent(epoch))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn genuine_callback_observation_is_inactive_by_default_and_run_bound_once() {
        let (engine, mut callback) = TestAudioCallback::new();
        assert!(callback.genuine_snapshot().run_binding.is_none());
        assert!(callback.enable_genuine_observation([0; 32]).is_err());
        callback.enable_genuine_observation([7; 32]).unwrap();
        assert!(callback.enable_genuine_observation([8; 32]).is_err());
        assert_eq!(callback.genuine_snapshot().run_binding, Some([7; 32]));
        assert!(callback.genuine_hooks_inactive());
        callback.pump(&engine, 128);
        assert_eq!(callback.take_genuine_segments().len(), 1);
        let (engine, mut callback) = TestAudioCallback::new();
        callback.pump(&engine, 128);
        assert!(callback.take_genuine_segments().is_empty());
        assert!(callback.enable_genuine_observation([9; 32]).is_err());
    }

    #[test]
    fn genuine_callback_records_actual_request_start_before_advance_without_allocation() {
        let (engine, mut callback) = TestAudioCallback::new();
        callback.enable_genuine_observation([1; 32]).unwrap();
        engine.seek_transport_to_frame(1.25, 30_000);
        engine.set_playing(true);
        crate::realtime_test_alloc::assert_no_alloc_or_drop(|| callback.pump(&engine, 128));
        let records = callback.take_genuine_segments();
        assert_eq!(records.len(), 1);
        let record = records[0];
        assert_eq!(
            (record.start_frame, record.frames, record.device_offset),
            (30_000, 128, 0)
        );
        assert_eq!(record.device_frame, 0);
        assert_eq!(record.pump_ordinal, 1);
        assert_eq!(record.segment_ordinal, 0);
        assert!(record.playing && record.playing_after_render && record.timeline_ready);
        assert_eq!(record.request_id, engine.test_transport_request_id());
        assert_eq!(record.disposition, GenuineSegmentDisposition::Rendered);
        assert_eq!(callback.genuine_snapshot().start_frame, 30_128);
        crate::realtime_test_alloc::assert_no_alloc_or_drop(|| callback.pump(&engine, 128));
        assert_eq!(callback.take_genuine_segments()[0].start_frame, 30_128);
    }

    #[test]
    fn genuine_callback_records_each_real_loop_segment_and_device_offset() {
        let (engine, mut callback) = TestAudioCallback::new();
        callback.enable_genuine_observation([2; 32]).unwrap();
        engine.set_transport_loop_frames(0.0, 0, 1.0, 160, true);
        engine.seek_transport_to_frame(0.0, 120);
        engine.set_playing(true);
        callback.pump(&engine, 128);
        let records = callback.take_genuine_segments();
        assert_eq!(records.len(), 2);
        assert_eq!(
            (
                records[0].start_frame,
                records[0].frames,
                records[0].device_offset
            ),
            (120, 40, 0)
        );
        assert_eq!(
            (
                records[1].start_frame,
                records[1].frames,
                records[1].device_offset
            ),
            (0, 88, 40)
        );
        assert_eq!(records[1].device_frame, records[0].device_frame + 40);
        assert_eq!(records[1].loop_count, records[0].loop_count + 1);
        assert!(records[1].epoch > records[0].epoch);
    }

    #[test]
    fn genuine_callback_overflow_is_sticky_after_driver_drain_and_never_allocates() {
        let (engine, mut callback) = TestAudioCallback::new();
        callback.enable_genuine_observation([3; 32]).unwrap();
        callback.pump(&engine, 128);
        let record = callback.take_genuine_segments()[0];
        // Collector capacity test copies a real observed value; it does not
        // replay DSP, manufacture a runtime receipt, or enter the genuine run.
        let observer = callback.dsp.test_genuine_observation.as_mut().unwrap();
        crate::realtime_test_alloc::assert_no_alloc_or_drop(|| {
            for _ in 0..=GENUINE_SEGMENT_CAPACITY {
                observer.push(record);
            }
        });
        assert!(callback.genuine_observation_overflowed());
        assert_eq!(
            callback.take_genuine_segments().len(),
            GENUINE_SEGMENT_CAPACITY
        );
        assert!(callback.genuine_observation_overflowed());
        callback.pump(&engine, 128);
        assert_eq!(callback.take_genuine_segments().len(), 1);
        assert!(callback.genuine_observation_overflowed());
    }

    #[test]
    fn genuine_callback_admission_rejection_preserves_actual_silent_extent() {
        let (engine, mut callback) = TestAudioCallback::new();
        callback.enable_genuine_observation([4; 32]).unwrap();
        // The real guard records this unsupported raw device extent. The next
        // supported pump must take its existing admission-rejection path.
        // This is a plugin-free unit test, not genuine-driver work or fake health.
        assert!(!callback.dsp.admit_plugin_callback(2049));
        callback.pump(&engine, 128);
        let records = callback.take_genuine_segments();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].frames, 128);
        assert_eq!(
            records[0].disposition,
            GenuineSegmentDisposition::AdmissionRejected
        );
        assert!(records[0].after.fault.is_some());
        assert!(
            callback
                .last_output()
                .iter()
                .all(|frame| *frame == [0.0; 2])
        );
        assert_eq!(callback.genuine_snapshot().device_frame, 128);
    }

    #[test]
    fn genuine_callback_observer_preserves_pcm_and_reports_actual_asset_arc() {
        let samples: Arc<[f32]> = (0..512)
            .flat_map(|_| [0.125_f32, -0.25])
            .collect::<Vec<_>>()
            .into();
        let (mut observed_engine, mut observed) = TestAudioCallback::new();
        let (mut ordinary_engine, mut ordinary) = TestAudioCallback::new();
        observed.enable_genuine_observation([11; 32]).unwrap();
        // Admit and observe the same real playing request before the measured
        // clip on both engines. PlayClip alone deliberately does not start transport.
        observed_engine.set_playing(true);
        ordinary_engine.set_playing(true);
        observed.pump(&observed_engine, 128);
        ordinary.pump(&ordinary_engine, 128);
        assert!(observed.genuine_snapshot().playing);
        assert!(ordinary.genuine_snapshot().playing);
        assert_eq!(observed.last_output(), ordinary.last_output());
        assert!(
            observed
                .last_output()
                .iter()
                .all(|frame| *frame == [0.0; 2])
        );
        let admitted = observed.take_genuine_segments();
        assert_eq!(admitted.len(), 1);
        assert!(admitted[0].playing && admitted[0].playing_after_render);
        assert_eq!(
            admitted[0].request_id,
            observed_engine.test_transport_request_id()
        );
        for engine in [&mut observed_engine, &mut ordinary_engine] {
            let operation = AudioAssetOperation {
                engine_session: engine.engine_session,
                generation: 1,
                operation_id: 1,
            };
            assert!(engine.register_asset(operation, 77, Arc::clone(&samples), 48_000, 2));
            assert!(engine.play_clip(88, 77, 0.0, 0.75, 1));
        }
        observed.pump(&observed_engine, 128);
        ordinary.pump(&ordinary_engine, 128);
        assert_eq!(observed.last_output(), ordinary.last_output());
        assert!(
            observed
                .last_output()
                .iter()
                .any(|frame| *frame != [0.0; 2])
        );
        let snapshot = observed.genuine_snapshot();
        assert_eq!(snapshot.assets.len(), 1);
        assert!(Arc::ptr_eq(&snapshot.assets[0].samples, &samples));
        assert_eq!(snapshot.assets[0].id, 77);
        assert_eq!(snapshot.assets[0].frames, 512);
        assert_eq!(snapshot.audio_voices.len(), 1);
        assert_eq!(snapshot.audio_voices[0].clip_id, 88);
        assert_eq!(snapshot.audio_voices[0].gain.to_bits(), 0.75_f32.to_bits());
        assert_eq!(snapshot.audio_voices[0].mixer_track, 1);
        assert!(snapshot.native_voices.is_empty());
        assert!(ordinary.take_genuine_segments().is_empty());
    }

    fn install_empty_timeline(engine: &mut AudioEngine, generation: u64, epoch: u64) {
        let mut project = crate::model::Project::blank();
        project.mixer_tracks.truncate(2);
        project
            .mixer_routes
            .retain(|route| route.source_mixer_track_id == 1);
        let tempo = crate::tempo_map::TempoMap::from_project(&project, 48_000).unwrap();
        let timeline = Arc::new(
            CompiledTimeline::from_project(
                &project,
                &tempo,
                crate::timeline::TimelineCompileOptions::default(),
            )
            .unwrap(),
        );
        let chase = engine
            .prepare_timeline_chase(
                &timeline,
                generation,
                epoch,
                0,
                TimelineChaseOptions::default(),
            )
            .unwrap();
        let loop_chase = engine
            .prepare_timeline_loop_chase(&timeline, generation, 0, TimelineChaseOptions::default())
            .unwrap();
        engine
            .install_compiled_timeline(generation, timeline)
            .unwrap();
        let loop_token = engine.install_timeline_loop_chase(loop_chase).unwrap();
        engine.install_timeline_chase(chase).unwrap();
        engine
            .activate_timeline_transport(TimelineTransportActivationSpec {
                revision: generation,
                target_epoch: epoch,
                minimum_epoch: epoch,
                frame: 0,
                beat_q32: 0,
                loop_start_frame: 0,
                loop_end_frame: 0,
                loop_start_q32: 0,
                loop_end_q32: 0,
                loop_token,
                loop_enabled: false,
                playing: false,
                mixer_pan_release: crate::timeline_runtime::TimelineMixerPanRelease::EMPTY,
            })
            .unwrap();
    }

    #[test]
    fn genuine_callback_qualified_paused_identity_requires_real_activation_and_exact_epoch() {
        let (mut engine, mut callback) = TestAudioCallback::new();
        callback.enable_genuine_observation([6; 32]).unwrap();
        install_empty_timeline(&mut engine, 17, 7);
        assert!(callback.genuine_snapshot().callback_work_pending());
        callback.pump(&engine, 128);
        let snapshot = callback.genuine_snapshot();
        assert!(snapshot.paused_work_quiescent(17, 7));
        assert!(!snapshot.paused_work_quiescent(16, 7));
        assert!(!snapshot.paused_work_quiescent(17, 6));
        assert!(snapshot.zero_pdc);
        assert_eq!(snapshot.nodes.len(), 2);
        assert_eq!(snapshot.routes.len(), 1);
        assert_eq!(snapshot.route_delays.len(), 1);
        assert_eq!(snapshot.raw_delays.len(), TRACK_COUNT);
        let record = callback.take_genuine_segments()[0];
        assert_eq!(record.generation, Some(17));
        assert_eq!(record.active_epoch, Some(7));
        assert_eq!(record.epoch, 7);
        assert!(!record.playing);
        assert!(
            !record.timeline_ready,
            "the exact kernel readiness result is false while paused"
        );
        engine.set_playing(true);
        callback.pump(&engine, 128);
        let playing = callback.take_genuine_segments()[0];
        assert!(playing.timeline_ready);
        assert!(
            playing.timeline_render_active,
            "observe the prepared plan before rendering consumes it"
        );
        assert!(
            !playing.timeline_render_active_after,
            "a consumed plan is reported separately"
        );
    }

    #[test]
    fn genuine_callback_zero_pdc_attributes_active_paths_and_reports_dormant_storage() {
        let (mut engine, mut callback) = TestAudioCallback::new();
        callback.enable_genuine_observation([12; 32]).unwrap();
        install_empty_timeline(&mut engine, 19, 9);
        callback.pump(&engine, 128);
        // Plugin-free predicate regression uses real delay-line requests.
        // No setter or fabricated snapshot is exposed to the genuine fixture.
        callback.dsp.pdc_raw_track_delays[31]
            .request_delay(7, 0)
            .unwrap();
        let snapshot = callback.genuine_snapshot();
        assert!(!snapshot.legacy_pdc_active);
        assert!(snapshot.raw_delay_active[0] && snapshot.raw_delay_active[1]);
        assert!(!snapshot.raw_delay_active[31]);
        assert_eq!(snapshot.raw_delays[31].target, 7);
        assert!(
            snapshot.zero_pdc,
            "dormant storage does not feed the exact two-node graph"
        );
        callback.dsp.pdc_raw_track_delays[1]
            .request_delay(1, 0)
            .unwrap();
        let snapshot = callback.genuine_snapshot();
        assert!(!snapshot.zero_pdc, "any nonzero contributing tap fails");
        assert_eq!(snapshot.raw_delays[1].current, 1);
        assert_eq!(snapshot.raw_delays[1].target, 1);
    }

    #[test]
    fn genuine_callback_empty_fallback_is_not_qualified_paused_identity() {
        let (engine, mut callback) = TestAudioCallback::new();
        callback.enable_genuine_observation([5; 32]).unwrap();
        callback.pump(&engine, 128);
        let snapshot = callback.genuine_snapshot();
        assert!(!snapshot.callback_work_pending());
        assert!(!snapshot.paused_work_quiescent(1, snapshot.epoch));
        assert!(
            !snapshot.zero_pdc,
            "an absent graph is not proof of a zero-delay graph"
        );
        assert_eq!(snapshot.timing.sample_rate, 48_000);
        assert_eq!(snapshot.timing.callback_budget_frames, 2048);
        assert_eq!(snapshot.timing.guard_quanta, 2);
        assert_eq!(snapshot.timing.lookahead_quanta, 18);
        assert_eq!(snapshot.timing.bridge_latency_frames, 2432);
    }
}
