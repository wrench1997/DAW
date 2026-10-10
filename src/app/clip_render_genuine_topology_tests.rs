//! Pure tables exercise the same finite classifier used by the real driver.
//! These manufacture evidence deliberately; actual callback/App tests are separate.
use super::*;
use crate::audio::genuine_observation::GenuineLifecycleEndpoint;
const RUN: [u8; 32] = [71; 32];
const KINDS: [OwnedTopologyKind; 4] = [
    OwnedTopologyKind::InitialInstall,
    OwnedTopologyKind::ConversionRemove,
    OwnedTopologyKind::UndoInstall,
    OwnedTopologyKind::RedoRemove,
];
fn claim(kind: OwnedTopologyKind) -> OwnedTopologyClaim {
    let (stage, control) = kind.role();
    OwnedTopologyClaim {
        kind,
        stage,
        control,
        pump: 21,
        next_poll: 31,
        engine: 7,
        project: 9,
        channel: 2,
        endpoint: 17,
        instance: 19,
        mixer: 1,
        worker: 23,
        previous_endpoint: Some(if kind == OwnedTopologyKind::RedoRemove {
            17
        } else {
            13
        }),
        previous_worker: Some(if kind == OwnedTopologyKind::RedoRemove {
            23
        } else {
            15
        }),
        stamp: (kind == OwnedTopologyKind::ConversionRemove).then_some(
            audio::GeneratorRemovalStamp {
                operation_id: 29,
                engine_session: 7,
                project_session: 9,
                channel_id: 2,
                endpoint_id: 17,
                plugin_instance_id: 19,
                stopped_epoch: 5,
            },
        ),
        old_generation: 3,
        old_epoch: 5,
        legacy_barrier: 73,
        old_graph: (3, 5, 101),
        desired_generation_at_claim: 4,
        observed_fingerprint_at_claim: 103,
        project_fingerprint_at_claim: 107,
        baseline: TopologyBaseline {
            revision: kind.index() as u64,
            failures: kind.index() as u64,
        },
        bridge_gaps: 0,
        underflow_frames: 0,
        retry_count: 0,
        stale_confirmations: 0,
        retired_sources_before: 0,
    }
}
fn ledger(kind: OwnedTopologyKind) -> OwnedTopologyLedger {
    let mut ledger = OwnedTopologyLedger::new(RUN);
    // A pure table can seed earlier accounted slots; it does not represent an
    // actual callback or substitute for real App/mock sequencing qualification.
    ledger.settled = kind.index();
    ledger.baseline = claim(kind).baseline;
    ledger.observed_receipts = kind.index() as u64;
    for slot in &mut ledger.slots[..kind.index()] {
        slot.phase = OwnedTopologyPhase::Settled;
    }
    ledger
}
fn state(c: OwnedTopologyClaim) -> GenuineLifecycleState {
    GenuineLifecycleState {
        engine_session: c.engine,
        project_session: c.project,
        transport_epoch: c.old_epoch,
        legacy_request_barrier: c.legacy_barrier,
        playing: false,
        generation: Some(c.old_generation),
        active_epoch: Some(c.old_epoch),
        plugin_topology_revision: c.baseline.revision,
        plugin_topology_replan_pending: false,
        timeline_execution_failures: c.baseline.failures,
        fault: None,
        needs_resync: false,
        suspended: false,
        graph_was_activated: true,
        graph_binding_exact: true,
        graph_identity: Some(c.old_graph),
        channel_binding_revision: Some(c.old_generation),
        channel_binding_epoch: Some(c.old_epoch),
        active_plugin_automation_bindings: 0,
        candidate_plugin_automation_bindings: 0,
        compiled_midi_routes: 0,
        generator_endpoints: usize::from(!c.kind.installs()),
        insert_endpoints: 0,
        pdc_revision: 11,
    }
}
fn receipt(c: OwnedTopologyClaim) -> GenuineLifecycleObservation {
    let pre = state(c);
    let mut post = pre;
    post.plugin_topology_revision += 1;
    post.timeline_execution_failures += 1;
    post.plugin_topology_replan_pending = true;
    post.needs_resync = true;
    post.suspended = true;
    post.graph_binding_exact = false;
    post.channel_binding_revision = None;
    post.channel_binding_epoch = None;
    post.generator_endpoints = usize::from(c.kind.installs());
    let (kind, event) = match c.kind {
        OwnedTopologyKind::InitialInstall | OwnedTopologyKind::UndoInstall => (
            GenuineLifecycleKind::Install,
            audio::GeneratorEndpointEvent::Installed {
                channel_id: c.channel,
                endpoint_id: c.endpoint,
                plugin_instance_id: c.instance,
                mixer_track: c.mixer,
                replaced_endpoint_id: None,
                replaced_plugin_instance_id: None,
                success: true,
            },
        ),
        OwnedTopologyKind::ConversionRemove => (
            GenuineLifecycleKind::StampedRemove,
            audio::GeneratorEndpointEvent::StampedRemoved {
                stamp: c.stamp.unwrap(),
                result: Ok(()),
            },
        ),
        OwnedTopologyKind::RedoRemove => (
            GenuineLifecycleKind::Remove,
            audio::GeneratorEndpointEvent::Removed {
                channel_id: c.channel,
                endpoint_id: Some(c.endpoint),
                plugin_instance_id: Some(c.instance),
            },
        ),
    };
    GenuineLifecycleObservation {
        run_binding: RUN,
        pump_ordinal: c.pump,
        record_ordinal: c.kind.index() as u64,
        kind,
        event,
        producer_present: true,
        enqueue_success: true,
        affected: Some(GenuineLifecycleEndpoint {
            channel_id: c.channel,
            endpoint_id: c.endpoint,
            plugin_instance_id: c.instance,
            mixer_track: c.mixer,
            project_session: c.project,
            worker_id: c.worker,
            epoch: c.old_epoch,
        }),
        replaced: None,
        pre: Some(pre),
        post,
    }
}
fn accepted(
    kind: OwnedTopologyKind,
) -> (
    OwnedTopologyLedger,
    OwnedTopologyClaim,
    GenuineLifecycleObservation,
) {
    let c = claim(kind);
    let r = receipt(c);
    let mut l = ledger(kind);
    l.claim(c).unwrap();
    l.callback_receipts(&[r], c.pump, c.next_poll).unwrap();
    (l, c, r)
}
fn target(c: OwnedTopologyClaim) -> OwnedTimeline {
    OwnedTimeline {
        generation: 4,
        fingerprint: 103,
        project_fingerprint: (c.kind != OwnedTopologyKind::InitialInstall).then_some(107),
        engine: c.engine,
        project: c.project,
    }
}
fn app_state(c: OwnedTopologyClaim, phase: TimelinePhase) -> TimelineAppState {
    TimelineAppState {
        phase,
        project_session: c.project,
        engine_session: Some(c.engine),
        sync_session: 1,
        desired_generation: 4,
        observed_fingerprint: 103,
        compiled_generation: Some(4),
        compiled_fingerprint: Some(103),
        source_generation: Some(4),
        source_fingerprint: Some(103),
        sync_source_generation: Some(4),
        desired_seek_serial: 11,
        desired_seek_frame: 0,
        loop_token: Some(127),
        loop_disable_request: None,
        retry_count: 0,
        stale_confirmations: 0,
        confirmed_identity: Some((c.old_generation, c.old_epoch)),
        retry_required: false,
        retry_pending: None,
    }
}
fn drive_receipts(
    l: &mut OwnedTopologyLedger,
    c: OwnedTopologyClaim,
    committed_epoch: u64,
) -> Check {
    let pairs = [
        (
            TimelineRuntimeEvent::Installed {
                request_id: 101,
                revision: 4,
            },
            TimelinePhase::InstallQueued {
                request_id: 101,
                generation: 4,
            },
            TimelinePhase::LoopReady,
        ),
        (
            TimelineRuntimeEvent::LoopChaseInstalled {
                request_id: 103,
                revision: 4,
                frame: 0,
                token: 127,
            },
            TimelinePhase::LoopQueued {
                request_id: 103,
                generation: 4,
                frame: 0,
            },
            TimelinePhase::SeekReady,
        ),
        (
            TimelineRuntimeEvent::ChaseInstalled {
                request_id: 107,
                revision: 4,
                epoch: 7,
                frame: 0,
            },
            TimelinePhase::SeekQueued {
                request_id: 107,
                generation: 4,
                seek_serial: 11,
                epoch: 7,
                frame: 0,
            },
            TimelinePhase::AwaitingActivation {
                generation: 4,
                seek_serial: 11,
                epoch: 7,
                frame: 0,
                activation_request_id: 109,
            },
        ),
        (
            TimelineRuntimeEvent::TransportActivationApplied {
                request_id: 109,
                revision: 4,
                epoch: committed_epoch,
                frame: 0,
            },
            TimelinePhase::AwaitingActivation {
                generation: 4,
                seek_serial: 11,
                epoch: 7,
                frame: 0,
                activation_request_id: 109,
            },
            TimelinePhase::Active {
                generation: 4,
                epoch: committed_epoch,
            },
        ),
    ];
    for (event, before, after) in pairs {
        let before = app_state(c, before);
        let mut after = app_state(c, after);
        if matches!(
            event,
            TimelineRuntimeEvent::TransportActivationApplied { .. }
        ) {
            after.confirmed_identity = Some((4, committed_epoch));
        }
        l.timeline_event(event, before, after)?;
    }
    Ok(())
}
fn segment_state(c: OwnedTopologyClaim, pending: bool) -> GenuineSegmentState {
    GenuineSegmentState {
        generator_endpoints: usize::from(if pending {
            c.kind.installs()
        } else {
            !c.kind.installs()
        }),
        insert_endpoints: 0,
        native_voices: 0,
        audio_voices: 0,
        generator_notes: 0,
        fault: None,
        suspended: pending,
        needs_resync: pending,
        graph_identity: Some(c.old_graph),
        graph_was_activated: true,
        graph_binding_exact: !pending,
        channel_binding_revision: (!pending).then_some(c.old_generation),
        channel_binding_epoch: (!pending).then_some(c.old_epoch),
        plugin_topology_revision: c.baseline.revision + u64::from(pending),
        plugin_topology_replan_pending: pending,
        active_plugin_automation_bindings: 0,
        candidate_plugin_automation_bindings: 0,
        compiled_midi_routes: 0,
        pdc_revision: 11,
        zero_pdc: true,
        paused_safety_services: 0,
        admitted_live_edit_endpoints: 0,
        parameter_edit_admission: 0,
        metronome_enabled: false,
        recording: false,
        midi_input_present: false,
        midi_recording_present: false,
        master_capture_present: false,
        driven_automation_targets: 0,
        timeline_execution_failures: c.baseline.failures + u64::from(pending),
        fixed_quantum_bridge_gaps: 0,
        fixed_quantum_output_underflow_frames: 0,
        hooks_inactive: true,
    }
}
fn segment(c: OwnedTopologyClaim, pending: bool) -> GenuineSegmentObservation {
    GenuineSegmentObservation {
        pump_ordinal: c.pump,
        segment_ordinal: 0,
        start_frame: 0,
        frames: 128,
        device_offset: 0,
        device_frame: 0,
        playing: false,
        playing_after_render: false,
        timeline_ready: false,
        timeline_render_active: false,
        timeline_render_active_after: false,
        generation: Some(c.old_generation),
        active_epoch: Some(c.old_epoch),
        epoch: c.old_epoch,
        request_id: c.legacy_barrier,
        discontinuity_id: 1,
        loop_count: 0,
        loop_enabled: true,
        loop_start_frame: 0,
        loop_end_frame: 96000,
        loop_token: Some(127),
        disposition: if pending {
            GenuineSegmentDisposition::SuspendedSilence
        } else {
            GenuineSegmentDisposition::Rendered
        },
        before: segment_state(c, pending),
        after: segment_state(c, pending),
    }
}
#[test]
fn four_kinds_accept_only_same_operation_receipt_then_exact_next_poll() {
    for kind in KINDS {
        let (mut l, c, r) = accepted(kind);
        assert!(!l.clear());
        assert!(l.settled_baseline().is_err());
        assert_eq!(
            l.open().unwrap().phase,
            OwnedTopologyPhase::CallbackAcceptedAwaitingAppPoll
        );
        l.segment(
            &segment(c, true),
            &[0.0; 256],
            TopologyPumpRole::Owned(kind),
        )
        .unwrap();
        l.consume_app_poll(c.next_poll, &[r.event], true).unwrap();
        l.observe_timeline_owner(None, true).unwrap();
        assert_eq!(
            l.open().unwrap().phase,
            OwnedTopologyPhase::AppAcceptedAwaitingOwnedTimeline
        );
        assert!(!l.clear());
        l.observe_timeline_owner(Some(target(c)), true).unwrap();
        drive_receipts(&mut l, c, 9).unwrap(); // actual epoch may exceed issued minimum7.
        assert_eq!(l.open().unwrap().requests.committed_epoch, Some(9));
        assert!(!l.clear());
    }
}
#[test]
fn claims_and_capacity_deferral_never_authorize_suspension_or_readiness() {
    for kind in KINDS {
        let c = claim(kind);
        let mut l = ledger(kind);
        l.claim(c).unwrap();
        l.callback_receipts(&[], c.pump, c.next_poll).unwrap();
        l.segment(
            &segment(c, false),
            &[0.0; 256],
            TopologyPumpRole::Owned(kind),
        )
        .unwrap();
        l.consume_app_poll(c.next_poll, &[], false).unwrap();
        let later = OwnedTopologyClaim {
            pump: c.pump + 1,
            next_poll: c.next_poll + 1,
            ..c
        };
        l.claim(later).unwrap();
        l.callback_receipts(&[receipt(later)], later.pump, later.next_poll)
            .unwrap();
        assert!(!l.clear());
        let mut pending_only = ledger(kind);
        pending_only.claim(c).unwrap();
        assert!(
            pending_only
                .segment(
                    &segment(c, true),
                    &[0.0; 256],
                    TopologyPumpRole::Owned(kind)
                )
                .is_err()
        );
        assert!(
            pending_only
                .callback_receipts(&[receipt(c)], c.pump, c.next_poll)
                .is_err()
        );
        let mut unclaimed = ledger(kind);
        assert!(
            unclaimed
                .callback_receipts(&[receipt(c)], c.pump, c.next_poll)
                .is_err()
        );
    }
}
#[test]
fn every_receipt_identity_delivery_pair_and_counter_operand_is_exact() {
    for kind in KINDS {
        let c = claim(kind);
        for case in 0..35 {
            let mut r = receipt(c);
            match case {
                0 => r.run_binding[0] ^= 1,
                1 => r.pump_ordinal += 1,
                2 => r.record_ordinal += 1,
                3 => r.producer_present = false,
                4 => r.enqueue_success = false,
                5 => r.pre = None,
                6 => r.affected = None,
                7 => r.replaced = r.affected,
                8 => r.affected.as_mut().unwrap().channel_id += 1,
                9 => r.affected.as_mut().unwrap().endpoint_id += 1,
                10 => r.affected.as_mut().unwrap().plugin_instance_id += 1,
                11 => r.affected.as_mut().unwrap().mixer_track += 1,
                12 => r.affected.as_mut().unwrap().project_session += 1,
                13 => r.affected.as_mut().unwrap().worker_id += 1,
                14 => r.affected.as_mut().unwrap().epoch += 1,
                15 => r.pre.as_mut().unwrap().engine_session += 1,
                16 => r.pre.as_mut().unwrap().project_session += 1,
                17 => r.pre.as_mut().unwrap().transport_epoch += 1,
                18 => r.pre.as_mut().unwrap().legacy_request_barrier += 1,
                19 => r.pre.as_mut().unwrap().playing = true,
                20 => r.pre.as_mut().unwrap().active_plugin_automation_bindings = 1,
                21 => r.pre.as_mut().unwrap().candidate_plugin_automation_bindings = 1,
                22 => r.pre.as_mut().unwrap().compiled_midi_routes = 1,
                23 => r.pre.as_mut().unwrap().timeline_execution_failures += 1,
                24 => r.pre.as_mut().unwrap().plugin_topology_revision += 1,
                25 => r.post.timeline_execution_failures -= 1,
                26 => r.post.timeline_execution_failures += 1,
                27 => r.post.plugin_topology_revision -= 1,
                28 => r.post.plugin_topology_revision += 1,
                29 => r.post.plugin_topology_replan_pending = false,
                30 => r.post.needs_resync = false,
                31 => r.post.suspended = false,
                32 => r.post.generator_endpoints += 1,
                33 => r.kind = GenuineLifecycleKind::Clear,
                34 => {
                    r.event = audio::GeneratorEndpointEvent::Cleared {
                        request_id: 31,
                        removed: 1,
                    }
                }
                _ => unreachable!(),
            }
            let mut l = ledger(kind);
            l.claim(c).unwrap();
            assert!(
                l.callback_receipts(&[r], c.pump, c.next_poll).is_err(),
                "{kind:?} case{case}"
            );
            assert!(
                l.callback_receipts(&[receipt(c)], c.pump, c.next_poll)
                    .is_err(),
                "failure must stay terminal"
            );
        }
    }
}
#[test]
fn next_poll_cardinality_owner_transfer_and_settled_reopening_are_terminal() {
    for kind in KINDS {
        for case in 0..6 {
            let (mut l, c, r) = accepted(kind);
            let events = match case {
                0 => vec![],
                1 => vec![r.event, r.event],
                2 => vec![audio::GeneratorEndpointEvent::Cleared {
                    request_id: 1,
                    removed: 0,
                }],
                _ => vec![r.event],
            };
            let poll = if case == 3 {
                c.next_poll + 1
            } else if case == 4 {
                c.next_poll - 1
            } else {
                c.next_poll
            };
            assert!(l.consume_app_poll(poll, &events, case != 5).is_err());
            assert!(l.consume_app_poll(c.next_poll, &[r.event], true).is_err());
        }
        let (mut l, c, r) = accepted(kind);
        assert!(l.callback_receipts(&[r], c.pump, c.next_poll).is_err());
        let mut extra = ledger(kind);
        extra.claim(c).unwrap();
        assert!(
            extra
                .callback_receipts(&[r, r], c.pump, c.next_poll)
                .is_err()
        );
    }
    let mut l = ledger(OwnedTopologyKind::RedoRemove);
    l.settled = 4;
    l.baseline = TopologyBaseline {
        revision: 4,
        failures: 4,
    };
    l.slots[3].phase = OwnedTopologyPhase::Settled;
    assert!(l.claim(claim(OwnedTopologyKind::InitialInstall)).is_err());
}
#[test]
fn owned_successor_cannot_be_missing_foreign_or_superseded_once_issued() {
    for kind in KINDS {
        for case in 0..7 {
            let (mut l, c, r) = accepted(kind);
            l.consume_app_poll(c.next_poll, &[r.event], true).unwrap();
            l.observe_timeline_owner(None, true).unwrap();
            l.observe_timeline_owner(Some(target(c)), true).unwrap();
            let mut changed = target(c);
            match case {
                0 => changed.generation += 1,
                1 => changed.fingerprint += 1,
                2 => changed.project += 1,
                3 => changed.engine += 1,
                4 => changed.project_fingerprint = Some(109),
                _ => {}
            }
            assert!(
                l.observe_timeline_owner(if case == 5 { None } else { Some(changed) }, case != 6)
                    .is_err(),
                "case{case}"
            );
        }
    }
}
#[test]
fn exact_epoch_minimum_request_and_legacy_domains_do_not_alias() {
    for epoch in [0, 5, 6, u64::MAX, 7 + (1u64 << 63)] {
        let (mut l, c, r) = accepted(OwnedTopologyKind::RedoRemove);
        l.consume_app_poll(c.next_poll, &[r.event], true).unwrap();
        l.observe_timeline_owner(Some(target(c)), true).unwrap();
        assert!(drive_receipts(&mut l, c, epoch).is_err());
    }
    for case in 0..7 {
        let (mut l, c, r) = accepted(OwnedTopologyKind::RedoRemove);
        l.consume_app_poll(c.next_poll, &[r.event], true).unwrap();
        l.observe_timeline_owner(Some(target(c)), true).unwrap();
        let mut before = app_state(
            c,
            TimelinePhase::InstallQueued {
                request_id: 101,
                generation: 4,
            },
        );
        match case {
            0 => before.retry_count = 1,
            1 => before.stale_confirmations = 1,
            2 => before.retry_required = true,
            3 => before.retry_pending = Some((7, 9)),
            4 => before.engine_session = Some(99),
            5 => before.phase = TimelinePhase::Faulted,
            6 => before.phase = TimelinePhase::FaultClearReady { revision: 3 },
            _ => unreachable!(),
        }
        assert!(
            l.timeline_event(
                TimelineRuntimeEvent::Installed {
                    request_id: 101,
                    revision: 4
                },
                before,
                app_state(c, TimelinePhase::LoopReady)
            )
            .is_err()
        );
    }
    for request in [0, 73, 102, u64::MAX] {
        let (mut l, c, r) = accepted(OwnedTopologyKind::RedoRemove);
        l.consume_app_poll(c.next_poll, &[r.event], true).unwrap();
        l.observe_timeline_owner(Some(target(c)), true).unwrap();
        l.observe_issued(app_state(
            c,
            TimelinePhase::InstallQueued {
                request_id: 101,
                generation: 4,
            },
        ))
        .unwrap();
        assert!(
            l.timeline_event(
                TimelineRuntimeEvent::Installed {
                    request_id: request,
                    revision: 4
                },
                app_state(
                    c,
                    TimelinePhase::InstallQueued {
                        request_id: 101,
                        generation: 4
                    }
                ),
                app_state(c, TimelinePhase::LoopReady)
            )
            .is_err()
        );
    }
}
#[test]
fn arithmetic_wrap_saturation_and_claim_identity_domains_refuse_qualification() {
    // The audio callback tests exercise the actual production next_nonzero_id
    // wrap mapping; this driver table separately refuses qualification at wrap.
    for baseline in [
        TopologyBaseline {
            revision: u64::MAX,
            failures: 0,
        },
        TopologyBaseline {
            revision: 0,
            failures: u64::MAX,
        },
    ] {
        assert!(baseline.successor().is_err());
    }
    for case in 0..16 {
        let mut c = claim(OwnedTopologyKind::InitialInstall);
        match case {
            0 => c.stage = 2,
            1 => c.control = 1,
            2 => c.engine = 0,
            3 => c.project = 0,
            4 => c.endpoint = 0,
            5 => c.instance = 0,
            6 => c.worker = 0,
            7 => c.mixer = 0,
            8 => c.old_epoch = 0,
            9 => c.old_generation = 0,
            10 => c.baseline.failures = 1,
            11 => c.baseline.revision = 1,
            12 => c.retired_sources_before = 1,
            13 => c.retry_count = 1,
            14 => c.stale_confirmations = 1,
            15 => c.old_graph.1 += 1,
            _ => unreachable!(),
        }
        assert!(ledger(c.kind).claim(c).is_err(), "case{case}");
    }
    let mut c = claim(OwnedTopologyKind::UndoInstall);
    c.previous_endpoint = Some(c.endpoint);
    assert!(ledger(c.kind).claim(c).is_err());
    let mut c = claim(OwnedTopologyKind::ConversionRemove);
    c.stamp.as_mut().unwrap().operation_id = 0;
    assert!(ledger(c.kind).claim(c).is_err());
}
#[test]
fn silent_barrier_has_exact_role_numerical_zero_and_independent_failure_precedence() {
    for case in 0..15 {
        let (l, c, _) = accepted(OwnedTopologyKind::RedoRemove);
        let mut s = segment(c, true);
        let mut output = [0.0; 256];
        let mut role = TopologyPumpRole::Owned(c.kind);
        match case {
            0 => role = TopologyPumpRole::Strict,
            1 => role = TopologyPumpRole::Owned(OwnedTopologyKind::UndoInstall),
            2 => s.playing = true,
            3 => s.playing_after_render = true,
            4 => s.request_id += 1,
            5 => s.epoch += 1,
            6 => s.disposition = GenuineSegmentDisposition::FaultSilence,
            7 => s.disposition = GenuineSegmentDisposition::AdmissionRejected,
            8 => s.after.timeline_execution_failures += 1,
            9 => s.after.plugin_topology_revision += 1,
            10 => s.before.fixed_quantum_bridge_gaps = 1,
            11 => s.after.fixed_quantum_output_underflow_frames = 1,
            12 => output[0] = f32::MIN_POSITIVE,
            13 => output[0] = f32::NAN,
            14 => s.after.native_voices = 1,
            _ => unreachable!(),
        }
        assert!(l.segment(&s, &output, role).is_err(), "case{case}");
    }
    let (l, c, _) = accepted(OwnedTopologyKind::RedoRemove);
    l.segment(
        &segment(c, true),
        &[-0.0; 256],
        TopologyPumpRole::Owned(c.kind),
    )
    .unwrap();
}
#[test]
fn fixed_playing_and_stopped_samples_preserve_accounted_baselines_one_two_four() {
    for count in [1, 2, 4] {
        for playing in [false, true] {
            for bad in 0..6 {
                let mut l = OwnedTopologyLedger::new(RUN);
                l.baseline = TopologyBaseline {
                    revision: count,
                    failures: count,
                };
                l.settled = count as usize;
                let mut c = claim(OwnedTopologyKind::InitialInstall);
                c.baseline = l.baseline;
                let mut s = segment(c, false);
                s.playing = playing;
                s.playing_after_render = playing;
                match bad {
                    1 => s.after.timeline_execution_failures += 1,
                    2 => s.before.plugin_topology_revision += 1,
                    3 => s.after.plugin_topology_replan_pending = true,
                    4 => s.after.suspended = true,
                    5 => s.after.needs_resync = true,
                    _ => {}
                }
                assert_eq!(
                    l.segment(&s, &[0.0; 256], TopologyPumpRole::Strict).is_ok(),
                    bad == 0
                );
            }
        }
    }
}
#[test]
fn first_activation_and_failed_attempt_are_charged_once_without_extending_windows() {
    let c = claim(OwnedTopologyKind::InitialInstall);
    let mut s = segment(c, false);
    s.playing = true;
    let mut budget = Budget::default();
    budget
        .charge(
            PumpPurpose::Playing {
                pass: 0,
                control: 1,
                already_active: false,
            }
            .charge(Some(&s)),
        )
        .unwrap();
    assert_eq!(budget.playing, [1, 0, 0]);
    assert_eq!(budget.total, 1);
    assert_eq!(budget.controls, [0; 8]);
    s.playing = false;
    s.epoch = 8;
    s.active_epoch = Some(8);
    budget
        .charge(
            PumpPurpose::Stopped {
                pass: 0,
                control: 1,
                previous_epoch: 5,
                generation: c.old_generation,
                already_active: false,
            }
            .charge(Some(&s)),
        )
        .unwrap();
    assert_eq!(budget.stopped, [1, 0, 0]);
    assert_eq!(budget.total, 2);
    assert_eq!(budget.controls, [0; 8]);
    budget.charge(PumpPurpose::Control(2).charge(None)).unwrap();
    assert_eq!(budget.total, 3);
    assert_eq!(budget.controls[2], 1);
    budget.stopped[0] = 32;
    assert!(
        budget
            .charge(
                PumpPurpose::Stopped {
                    pass: 0,
                    control: 1,
                    previous_epoch: 5,
                    generation: c.old_generation,
                    already_active: true
                }
                .charge(Some(&s))
            )
            .is_err()
    );
}

#[test]
fn settlement_requires_every_same_sample_operand_and_cannot_reopen() {
    for missing in 0..9 {
        let (mut l, c, r) = accepted(OwnedTopologyKind::RedoRemove);
        l.consume_app_poll(c.next_poll, &[r.event], true).unwrap();
        l.observe_timeline_owner(Some(target(c)), true).unwrap();
        drive_receipts(&mut l, c, 9).unwrap();
        let (_engine, callback) = AudioEngine::test_engine_with_callback();
        // Synthetic pure-classifier snapshot, never claimed as callback proof.
        let mut s = callback.genuine_snapshot();
        s.run_binding = Some(RUN);
        s.engine_session = c.engine;
        s.project_session = c.project;
        s.lifecycle_observed_count = 4;
        s.plugin_topology_revision = 4;
        s.timeline_execution_failures = 4;
        s.generation = Some(4);
        s.active_epoch = Some(9);
        s.epoch = 9;
        s.request_id = c.legacy_barrier;
        s.start_frame = 0;
        s.playing = false;
        s.graph_was_activated = true;
        s.graph_binding_exact = true;
        s.graph_identity = Some((4, 9, 101));
        s.channel_binding_revision = Some(4);
        s.channel_binding_epoch = Some(9);
        s.loop_token = Some(127);
        s.timeline_stats = Some(Default::default());
        let mut e = TopologySettlement {
            generation: 4,
            epoch: 9,
            exact_paused: true,
            source_current: true,
            owner_complete: true,
            worker_joined: true,
            asset_receipt_exact: true,
            history_exact: true,
            paused_work_quiescent: true,
            ordinary_health_clear: true,
        };
        match missing {
            0 => e.exact_paused = false,
            1 => e.source_current = false,
            2 => e.owner_complete = false,
            3 => e.worker_joined = false,
            4 => e.asset_receipt_exact = false,
            5 => e.history_exact = false,
            6 => e.paused_work_quiescent = false,
            7 => e.ordinary_health_clear = false,
            8 => l.open_mut().unwrap().requests.activation_applied = false,
            _ => unreachable!(),
        }
        assert!(!l.settle(e, &s).unwrap());
        assert!(!l.clear());
        e.exact_paused = true;
        e.source_current = true;
        e.owner_complete = true;
        e.worker_joined = true;
        e.asset_receipt_exact = true;
        e.history_exact = true;
        e.paused_work_quiescent = true;
        e.ordinary_health_clear = true;
        l.open_mut().unwrap().requests.activation_applied = true;
        assert!(l.settle(e, &s).unwrap());
        assert!(l.complete());
        assert_eq!(
            l.settled_baseline().unwrap(),
            TopologyBaseline {
                revision: 4,
                failures: 4
            }
        );
        assert!(!l.settle(e, &s).unwrap());
        assert!(l.claim(c).is_err());
    }
}

#[test]
fn retained_app_worker_is_authoritative_before_backend_observer_arrives() {
    for kind in KINDS {
        let c = claim(kind);
        assert!(backend_workers_exact(c, &[]));
        let phase = if kind.index() < 2 {
            InstancePhase::InitialSource
        } else {
            InstancePhase::UndoSource
        };
        for (worker, expected) in [
            (None, true),
            (Some(c.worker), true),
            (Some(c.worker + 1), false),
        ] {
            let record = Arc::new(observe::Record {
                ordinal: 0,
                stage: Stage::InitialSource,
                instance: Some(observe::InstanceIdentity {
                    ordinal: 0,
                    phase,
                    worker_id: worker,
                    source_instance_id: Some(c.instance),
                }),
                kind: RecordKind::Lifecycle("pure backend identity evidence"),
            });
            assert_eq!(backend_workers_exact(c, &[record]), expected);
        }
    }
}
