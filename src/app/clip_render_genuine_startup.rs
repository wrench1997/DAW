//! Receipt-based initial startup evidence. Native capture serial is an observed
//! editor-publication counter, never an acknowledgment of ordinary request0 State.
use super::*;
use crate::plugins::plugin_runtime::NativeEditorSnapshot;
const TEXT_BYTES: usize = 1024;
const ENDPOINT_LIMIT: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct StartupIdentity {
    pub endpoint: u64,
    pub session: u64,
    pub instance: u64,
    pub channel: u32,
    pub mixer: usize,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub(super) struct ReceiptCounts {
    states: usize,
    matching_states: usize,
    ready: usize,
    matching_ready: usize,
    installed: usize,
    matching_installed: usize,
}
impl ReceiptCounts {
    pub fn observe(&mut self, record: &observe::Record, expected: StartupIdentity) {
        let stage = record.stage == Stage::InitialSource;
        match &record.kind {
            RecordKind::OrdinaryState {
                endpoint_id,
                project_session,
                instance_id,
                slot,
                request_id,
                epoch,
                ..
            } => {
                self.states += 1;
                self.matching_states += usize::from(
                    stage
                        && *endpoint_id == expected.endpoint
                        && *project_session == expected.session
                        && *instance_id == expected.instance
                        && *slot == 0
                        && *request_id == 0
                        && epoch.is_none(),
                );
            }
            RecordKind::RuntimeEvent {
                endpoint_id,
                project_session,
                instance_id,
                event: RuntimeEvent::SlotReady { slot, .. },
            } => {
                self.ready += 1;
                self.matching_ready += usize::from(
                    stage
                        && *endpoint_id == expected.endpoint
                        && *project_session == expected.session
                        && *instance_id == expected.instance
                        && *slot == 0,
                );
            }
            RecordKind::AppGeneratorEndpointEvent(audio::GeneratorEndpointEvent::Installed {
                channel_id,
                endpoint_id,
                plugin_instance_id,
                mixer_track,
                replaced_endpoint_id,
                replaced_plugin_instance_id,
                success,
            }) => {
                self.installed += 1;
                self.matching_installed += usize::from(
                    stage
                        && *channel_id == expected.channel
                        && *endpoint_id == expected.endpoint
                        && *plugin_instance_id == expected.instance
                        && *mixer_track == expected.mixer
                        && replaced_endpoint_id.is_none()
                        && replaced_plugin_instance_id.is_none()
                        && *success,
                );
            }
            _ => {}
        }
    }
    fn complete(self) -> bool {
        self.states == 1
            && self.matching_states == 1
            && self.ready == 1
            && self.matching_ready == 1
            && self.installed == 1
            && self.matching_installed == 1
    }
}
#[derive(Clone, Copy, Debug, serde::Serialize)]
pub(super) struct ReadyOperands {
    pub exact_paused: bool,
    pub running_owner_exact: bool,
    pub coherent_live_binding: bool,
    pub diagnostics_complete: bool,
}
fn native_idle(native: Option<&NativeEditorSnapshot>) -> bool {
    native.is_some_and(|native| {
        !native.state.open
            && native.pending_request.is_none()
            && !native.capture_in_progress
            && native.error.is_none()
    })
}
pub(super) fn ready(
    operands: ReadyOperands,
    receipts: ReceiptCounts,
    native: Option<&NativeEditorSnapshot>,
) -> bool {
    operands.exact_paused
        && operands.running_owner_exact
        && operands.coherent_live_binding
        && operands.diagnostics_complete
        && receipts.complete()
        && native_idle(native)
}
pub(super) fn text_evidence(text: &str) -> Value {
    let mut end = text.len().min(TEXT_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    json!({"text":&text[..end],"original_bytes":text.len(),"truncated":end != text.len()})
}
pub(super) fn native_evidence(native: Option<&NativeEditorSnapshot>) -> Value {
    native.map(|n|json!({"present":true,"idle":native_idle(Some(n)),
        "supported":n.state.supported,"has_editor":n.state.has_editor,"open":n.state.open,
        "generation":n.state.generation,"native_used":n.native_used,"dirty_revision":n.dirty_revision,
        "captured_generation":n.captured_generation,"captured_dirty_revision":n.captured_dirty_revision,
        "capture_serial":n.capture_serial,"parameter_capture_serial":n.parameter_capture_serial,
        "pending_request":n.pending_request,"capture_in_progress":n.capture_in_progress,
        "completed_request":n.completed_request,"error":n.error.as_deref().map(text_evidence)}))
        .unwrap_or_else(||json!({"present":false,"idle":false}))
}
pub(super) fn stopped_epoch_safety_evidence(
    epoch: Option<u64>,
    remaining_frames: usize,
    failed: bool,
) -> Value {
    json!({"epoch":epoch,"remaining_frames":remaining_frames,"failed":failed})
}

// This is the unchanged control_tick decision, exposed only for attribution.
// No callback work is serviced here, and no pending bit grants a new pump.
pub(super) fn pump_decision(
    callback_work_pending: bool,
    callback_barrier: u64,
    mailbox_barrier: u64,
) -> bool {
    callback_work_pending || callback_barrier != mailbox_barrier
}
pub(super) fn callback_evidence(snapshot: &GenuineCallbackSnapshot, mailbox_barrier: u64) -> Value {
    let endpoints = snapshot.endpoints.iter().take(ENDPOINT_LIMIT).map(|e| {
        let b = &e.fixed.bridge;
        json!({"endpoint":e.endpoint_id,"project_session":e.project_session,"channel":e.channel_id,
            "instance":e.plugin_instance_id,"mixer_track":e.mixer_track,"worker":b.worker_id,
            "callback_work_pending":e.callback_work_pending(),"live_lane_faulted":e.live_lane_faulted,
            "stopped_epoch_safety":stopped_epoch_safety_evidence(e.stopped_epoch_safety_epoch,
                e.stopped_epoch_safety_remaining_frames,e.stopped_epoch_safety_failed),
            "staged_system":e.staged_system_events,"staged_timeline":e.staged_timeline_events,
            "staged_live":e.staged_live_events,"partial_system":e.partial_system_events,
            "partial_timeline":e.partial_timeline_events,"partial_live":e.partial_live_events,
            "partial_total":e.partial_total_events,"admitted_live_edit_markers":e.admitted_live_edit_markers,
            "input_phase_frames":e.fixed.input_phase_frames,"pending_event_count":e.fixed.pending_event_count,
            "pending_inline_events":b.pending_inline_events,"input_pending":b.input_pending,
            "worker_busy":b.worker_busy,"submitted":b.stats.submitted,"input_consumed":b.input_consumed,
            "operations_completed":b.operations_completed,"worker_epoch":b.worker_epoch,
            "callback_epoch":e.fixed.epoch,"bridge_epoch":b.stats.current_epoch,
            "capture_busy":b.capture_busy,"parameter_edits_admitted":b.parameter_edits_admitted})
    }).collect::<Vec<_>>();
    json!({"engine_session":snapshot.engine_session,"project_session":snapshot.project_session,
        "callback_work_pending":snapshot.callback_work_pending(),
        "would_control_pump":pump_decision(snapshot.callback_work_pending(),snapshot.request_id,mailbox_barrier),
        "callback_legacy_barrier":snapshot.request_id,"mailbox_legacy_barrier":mailbox_barrier,
        "pending_audio_commands":snapshot.pending_audio_commands,
        "pending_timeline_commands":snapshot.pending_timeline_commands,
        "pending_activation":snapshot.pending_timeline_activation,
        "paused_safety_services":snapshot.paused_safety_services,
        "admitted_live_edit_endpoints":snapshot.admitted_live_edit_endpoints,
        "parameter_edit_admission":snapshot.parameter_edit_admission,
        "generation":snapshot.generation,"active_epoch":snapshot.active_epoch,"epoch":snapshot.epoch,
        "plugin_topology_revision":snapshot.plugin_topology_revision,"plugin_topology_replan_pending":snapshot.plugin_topology_replan_pending,
        "timeline_execution_failures":snapshot.timeline_execution_failures,"needs_resync":snapshot.timeline_stats.is_some_and(|stats| stats.ownership_needs_resync),"suspended":snapshot.suspended,
        "graph_was_activated":snapshot.graph_was_activated,"graph_binding_exact":snapshot.graph_binding_exact,
        "channel_binding_revision":snapshot.channel_binding_revision,"channel_binding_epoch":snapshot.channel_binding_epoch,
        "active_plugin_automation_bindings":snapshot.active_plugin_automation_bindings,"candidate_plugin_automation_bindings":snapshot.candidate_plugin_automation_bindings,
        "compiled_midi_routes":snapshot.compiled_midi_routes,"lifecycle_observed_count":snapshot.lifecycle_observed_count,
        "lifecycle_overflowed":snapshot.lifecycle_overflowed,"lifecycle_inconsistent_pair":snapshot.lifecycle_inconsistent_pair,
        "frame":snapshot.start_frame,"playing":snapshot.playing,
        "endpoint_count":snapshot.endpoints.len(),"endpoint_limit":ENDPOINT_LIMIT,
        "endpoints_truncated":snapshot.endpoints.len()>ENDPOINT_LIMIT,"endpoints":endpoints})
}
fn inventory_count_complete(count: usize) -> bool {
    count <= ENDPOINT_LIMIT
}
pub(super) fn diagnostic_inventory_complete(snapshot: &GenuineCallbackSnapshot) -> bool {
    inventory_count_complete(snapshot.endpoints.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    const ID: StartupIdentity = StartupIdentity {
        endpoint: 11,
        session: 7,
        instance: 13,
        channel: 2,
        mixer: 1,
    };
    fn record(kind: RecordKind) -> observe::Record {
        observe::Record {
            ordinal: 1,
            monotonic_elapsed_ns: 0, // Synthetic oracle input, never native timing evidence.
            stage: Stage::InitialSource,
            instance: None,
            kind,
        }
    }
    fn state(
        identity: StartupIdentity,
        slot: usize,
        request: u64,
        epoch: Option<u64>,
    ) -> observe::Record {
        record(RecordKind::OrdinaryState {
            endpoint_id: identity.endpoint,
            project_session: identity.session,
            instance_id: identity.instance,
            slot,
            request_id: request,
            epoch,
            byte_len: 51927,
        })
    }
    fn slot_ready(identity: StartupIdentity) -> observe::Record {
        record(RecordKind::RuntimeEvent {
            endpoint_id: identity.endpoint,
            project_session: identity.session,
            instance_id: identity.instance,
            event: RuntimeEvent::SlotReady {
                slot: 0,
                name: "Project-only receipt".into(),
                latency_samples: 0,
                tail_samples: 96000,
            },
        })
    }
    fn installed(identity: StartupIdentity) -> observe::Record {
        record(RecordKind::AppGeneratorEndpointEvent(
            audio::GeneratorEndpointEvent::Installed {
                channel_id: identity.channel,
                endpoint_id: identity.endpoint,
                plugin_instance_id: identity.instance,
                mixer_track: identity.mixer,
                replaced_endpoint_id: None,
                replaced_plugin_instance_id: None,
                success: true,
            },
        ))
    }
    fn counts() -> ReceiptCounts {
        let mut counts = ReceiptCounts::default();
        for event in [state(ID, 0, 0, None), slot_ready(ID), installed(ID)] {
            counts.observe(&event, ID);
        }
        counts
    }
    fn operands() -> ReadyOperands {
        ReadyOperands {
            exact_paused: true,
            running_owner_exact: true,
            coherent_live_binding: true,
            diagnostics_complete: true,
        }
    }
    #[test]
    fn stopped_epoch_safety_evidence_preserves_exact_observed_scalars() {
        for (epoch, remaining_frames, failed, expected) in [
            (
                None,
                0,
                false,
                json!({"epoch":null,"remaining_frames":0,"failed":false}),
            ),
            (
                Some(7),
                128,
                false,
                json!({"epoch":7,"remaining_frames":128,"failed":false}),
            ),
            (
                Some(7),
                128,
                true,
                json!({"epoch":7,"remaining_frames":128,"failed":true}),
            ),
            (
                Some(7),
                32,
                false,
                json!({"epoch":7,"remaining_frames":32,"failed":false}),
            ),
            // Synthetic inconsistent observations must not be normalized.
            (
                None,
                32,
                false,
                json!({"epoch":null,"remaining_frames":32,"failed":false}),
            ),
            (
                None,
                0,
                true,
                json!({"epoch":null,"remaining_frames":0,"failed":true}),
            ),
            (
                None,
                32,
                true,
                json!({"epoch":null,"remaining_frames":32,"failed":true}),
            ),
            (
                Some(7),
                0,
                false,
                json!({"epoch":7,"remaining_frames":0,"failed":false}),
            ),
            (
                Some(0),
                0,
                true,
                json!({"epoch":0,"remaining_frames":0,"failed":true}),
            ),
            (
                Some(u64::MAX),
                usize::MAX,
                true,
                json!({"epoch":u64::MAX,"remaining_frames":usize::MAX,"failed":true}),
            ),
        ] {
            assert_eq!(
                stopped_epoch_safety_evidence(epoch, remaining_frames, failed),
                expected
            );
        }
    }
    #[test]
    fn clean_native_zero_serial_accepts_exact_real_receipt_shapes() {
        let native = NativeEditorSnapshot::default();
        assert!(!native.native_used && native.dirty_revision == 0 && native.capture_serial == 0);
        assert!(ready(operands(), counts(), Some(&native)));
        assert_eq!(native_evidence(Some(&native))["capture_serial"], 0);
    }
    #[test]
    fn receipt_owner_request_epoch_stage_and_cardinality_are_exact() {
        for identity in [
            StartupIdentity { endpoint: 12, ..ID },
            StartupIdentity { session: 8, ..ID },
            StartupIdentity { instance: 14, ..ID },
        ] {
            for which in 0..3 {
                let mut c = ReceiptCounts::default();
                for event in [
                    state(if which == 0 { identity } else { ID }, 0, 0, None),
                    slot_ready(if which == 1 { identity } else { ID }),
                    installed(if which == 2 { identity } else { ID }),
                ] {
                    c.observe(&event, ID);
                }
                // Installed has no session field; exact session belongs to the
                // bound live owner plus SlotReady/State, not an invented receipt.
                if which != 2 || identity.session == ID.session {
                    assert!(!c.complete());
                } else {
                    assert!(c.complete());
                }
            }
        }
        for identity in [
            StartupIdentity { channel: 3, ..ID },
            StartupIdentity { mixer: 2, ..ID },
        ] {
            let mut c = ReceiptCounts::default();
            for event in [state(ID, 0, 0, None), slot_ready(ID), installed(identity)] {
                c.observe(&event, ID);
            }
            assert!(!c.complete());
        }
        let mut rejected = installed(ID);
        if let RecordKind::AppGeneratorEndpointEvent(audio::GeneratorEndpointEvent::Installed {
            success,
            ..
        }) = &mut rejected.kind
        {
            *success = false;
        }
        let mut replaced = installed(ID);
        if let RecordKind::AppGeneratorEndpointEvent(audio::GeneratorEndpointEvent::Installed {
            replaced_endpoint_id,
            ..
        }) = &mut replaced.kind
        {
            *replaced_endpoint_id = Some(99);
        }
        for event in [rejected, replaced] {
            let mut c = ReceiptCounts::default();
            c.observe(&state(ID, 0, 0, None), ID);
            c.observe(&slot_ready(ID), ID);
            c.observe(&event, ID);
            assert!(!c.complete());
        }
        let mut wrong_slot = slot_ready(ID);
        if let RecordKind::RuntimeEvent {
            event: RuntimeEvent::SlotReady { slot, .. },
            ..
        } = &mut wrong_slot.kind
        {
            *slot = 1;
        }
        let mut c = ReceiptCounts::default();
        c.observe(&state(ID, 0, 0, None), ID);
        c.observe(&wrong_slot, ID);
        c.observe(&installed(ID), ID);
        assert!(!c.complete());
        for event in [
            state(ID, 1, 0, None),
            state(ID, 0, 1, None),
            state(ID, 0, 0, Some(9)),
        ] {
            let mut c = ReceiptCounts::default();
            c.observe(&event, ID);
            c.observe(&slot_ready(ID), ID);
            c.observe(&installed(ID), ID);
            assert!(!c.complete());
        }
        for which in 0..3 {
            let events = [state(ID, 0, 0, None), slot_ready(ID), installed(ID)];
            let mut missing = ReceiptCounts::default();
            for (index, event) in events.iter().enumerate() {
                if index != which {
                    missing.observe(event, ID);
                }
            }
            assert!(!missing.complete());
            let mut duplicate = counts();
            duplicate.observe(&events[which], ID);
            assert!(!duplicate.complete());
            let mut wrong_stage = ReceiptCounts::default();
            for (index, mut event) in events.into_iter().enumerate() {
                if index == which {
                    event.stage = Stage::Undo;
                }
                wrong_stage.observe(&event, ID);
            }
            assert!(!wrong_stage.complete());
        }
    }
    #[test]
    fn every_non_native_ready_operand_is_required() {
        let native = NativeEditorSnapshot::default();
        for o in [
            ReadyOperands {
                exact_paused: false,
                ..operands()
            },
            ReadyOperands {
                running_owner_exact: false,
                ..operands()
            },
            ReadyOperands {
                coherent_live_binding: false,
                ..operands()
            },
            ReadyOperands {
                diagnostics_complete: false,
                ..operands()
            },
        ] {
            assert!(!ready(o, counts(), Some(&native)));
        }
        assert!(!ready(operands(), counts(), None));
    }
    #[test]
    fn native_busy_open_pending_or_error_never_passes() {
        let mut native = NativeEditorSnapshot {
            capture_serial: u64::MAX,
            capture_in_progress: true,
            ..Default::default()
        };
        assert!(!ready(operands(), counts(), Some(&native)));
        native.capture_in_progress = false;
        native.pending_request = Some(19);
        assert!(!ready(operands(), counts(), Some(&native)));
        native.pending_request = None;
        native.state.open = true;
        assert!(!ready(operands(), counts(), Some(&native)));
        native.state.open = false;
        native.error = Some("native failure".into());
        assert!(!ready(operands(), counts(), Some(&native)));
        native.error = None;
        assert!(ready(operands(), counts(), Some(&native)));
    }
    #[test]
    fn serial_error_and_utf8_bounds_are_preserved() {
        let native = NativeEditorSnapshot {
            capture_serial: 17,
            parameter_capture_serial: 19,
            native_used: true,
            dirty_revision: 23,
            pending_request: Some(29),
            capture_in_progress: true,
            error: Some("a".repeat(TEXT_BYTES - 1) + "🦀end"),
            ..Default::default()
        };
        let evidence = native_evidence(Some(&native));
        assert_eq!(evidence["capture_serial"], 17);
        assert_eq!(evidence["parameter_capture_serial"], 19);
        assert_eq!(evidence["native_used"], true);
        assert_eq!(evidence["dirty_revision"], 23);
        assert_eq!(evidence["pending_request"], 29);
        assert_eq!(evidence["capture_in_progress"], true);
        assert_eq!(
            evidence["error"]["text"].as_str().unwrap().len(),
            TEXT_BYTES - 1
        );
        assert_eq!(evidence["error"]["truncated"], true);
        assert!(inventory_count_complete(ENDPOINT_LIMIT));
        assert!(!inventory_count_complete(ENDPOINT_LIMIT + 1));
        let all = serde_json::to_value(operands()).unwrap();
        for field in [
            "exact_paused",
            "running_owner_exact",
            "coherent_live_binding",
            "diagnostics_complete",
        ] {
            assert_eq!(all[field], true);
        }
    }
    #[test]
    fn callback_pending_and_mailbox_mismatch_are_independent_evidence_only() {
        for (pending, callback, mailbox, expected) in [
            (false, 73, 73, false),
            (true, 73, 73, true),
            (false, 73, 74, true),
            (true, 73, 74, true),
        ] {
            assert_eq!(pump_decision(pending, callback, mailbox), expected);
        }
        let (engine, callback) = AudioEngine::test_engine_with_callback();
        let snapshot = callback.genuine_snapshot();
        let evidence = callback_evidence(&snapshot, engine.test_transport_request_id());
        assert_eq!(
            evidence["callback_work_pending"],
            snapshot.callback_work_pending()
        );
        assert_eq!(
            evidence["pending_audio_commands"],
            snapshot.pending_audio_commands
        );
        assert_eq!(
            evidence["pending_timeline_commands"],
            snapshot.pending_timeline_commands
        );
        assert_eq!(
            evidence["pending_activation"],
            snapshot.pending_timeline_activation
        );
        assert!(diagnostic_inventory_complete(&snapshot));
        assert!(evidence["endpoints"].as_array().unwrap().len() <= ENDPOINT_LIMIT);
        for field in 0..5 {
            let mut pending = callback.genuine_snapshot();
            match field {
                0 => pending.pending_audio_commands = 1,
                1 => pending.pending_timeline_commands = 1,
                2 => pending.pending_timeline_activation = true,
                3 => pending.paused_safety_services = 1,
                4 => pending.admitted_live_edit_endpoints = 1,
                _ => unreachable!(),
            }
            let evidence = callback_evidence(&pending, pending.request_id);
            assert_eq!(evidence["callback_work_pending"], true);
            assert_eq!(evidence["would_control_pump"], true);
        }
        // No pump, command, plugin owner, observer installation or service run.
    }
}
