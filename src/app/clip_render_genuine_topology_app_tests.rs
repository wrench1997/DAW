//! Plugin-free App sequencing gates for the same owned-topology classifier as
//! the ignored genuine route. These are source-prepared tests, not run evidence.
//!
//! Initial preparation substitutes TestCaptureSource only at the existing
//! Fixture's pending-generator handoff. It does not exercise the SDK picker/load
//! method. All callback receipts, subsequent polls, compilation, conversion,
//! history operations, restoration installation and joins are production paths.
#[path = "clip_render_current_source_dispatch_tests.rs"]
mod current_source_dispatch_tests;
use super::*;
use crate::app::clip_render_job::genuine::topology::{
    self, OwnedTopologyKind, OwnedTopologyLedger, OwnedTopologyPhase, TopologyPumpRole,
};
use crate::audio::genuine_observation::{
    GenuineCallbackSnapshot, GenuineLifecycleObservation, GenuineSegmentDisposition,
};
use crate::genuine_app_observer::{
    self as observe, ObservationRun, RecordKind, TimelinePhase, WorkerJoinOutcome,
};
use crate::plugins::plugin_runtime::{BridgeStats, PluginChain};
use std::sync::Arc;

const RUN: [u8; 32] = [0x64; 32];
const MOCK_WAIT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy)]
#[allow(clippy::large_enum_variant)] // Four bounded Copy slots retain exact in-place worker evidence.
enum SourceSlot {
    Reserved,
    Owned { worker: usize, stats: BridgeStats },
}
thread_local! {
    // Reserved before spawn. No unbounded diagnostic Vec or replacement owner
    // is admitted after a cleanup failure. The actual guards stay in Fixture's
    // exact App, which is retained whole if any bounded drain cannot prove join.
    static SOURCE_SLOTS: std::cell::RefCell<[Option<SourceSlot>; 4]> = const {
        std::cell::RefCell::new([None; 4])
    };
}
fn reserve_source_slot() -> Result<usize, String> {
    if FIXTURE_POISONED.load(Ordering::Acquire) {
        return Err("prior exact mock fixture remains quarantined".into());
    }
    SOURCE_SLOTS.with(|slots| {
        let mut slots = slots.borrow_mut();
        let index = slots
            .iter()
            .position(Option::is_none)
            .ok_or("four retained mock source slots exhausted")?;
        slots[index] = Some(SourceSlot::Reserved);
        Ok(index)
    })
}
fn bind_source_slot(index: usize, chain: &PluginChain) -> usize {
    let worker = chain.audio.genuine_snapshot().worker_id;
    let stats = chain.control.stats();
    SOURCE_SLOTS
        .with(|slots| slots.borrow_mut()[index] = Some(SourceSlot::Owned { worker, stats }));
    worker
}
fn latest_worker() -> usize {
    SOURCE_SLOTS.with(|slots| {
        slots
            .borrow()
            .iter()
            .rev()
            .find_map(|slot| match slot {
                Some(SourceSlot::Owned { worker, .. }) => Some(*worker),
                _ => None,
            })
            .expect("a real mock source was admitted")
    })
}
fn source_count() -> usize {
    SOURCE_SLOTS.with(|slots| {
        slots
            .borrow()
            .iter()
            .filter(|slot| matches!(slot, Some(SourceSlot::Owned { .. })))
            .count()
    })
}

fn restoration_factory(
    candidate: &Project,
    instance_id: u64,
    spec: PluginLoadSpec,
    config: PluginPrepareConfig,
    metadata: &BouncePluginMetadata,
) -> Result<PluginChain, String> {
    let slot = reserve_source_slot()?;
    let chain = crate::app::clip_render_glue::tests::authoritative_mock_restoration_factory(
        candidate,
        instance_id,
        spec,
        config,
        metadata,
    )?;
    // No assertion, fallible preparation or worker wait after successful spawn.
    bind_source_slot(slot, &chain);
    Ok(chain)
}

struct TopologyFixture {
    // Deliberate drop order: all exact App owners drain before observation ends.
    fixture: Fixture,
    observer: ObservationRun,
    ledger: OwnedTopologyLedger,
    poll: usize,
    record_cursor: usize,
    initial_worker: usize,
    initial_endpoint: u64,
    callback_receipts: [Option<GenuineLifecycleObservation>; 4],
    callback_receipt_count: usize,
    last_disposition: Option<GenuineSegmentDisposition>,
}

impl TopologyFixture {
    fn queued_initial() -> Self {
        Self::queued_initial_with_held_prepare(false)
    }
    fn queued_initial_with_held_prepare(held: bool) -> Self {
        let admission_deadline = Instant::now() + MOCK_WAIT;
        let admission = loop {
            assert!(
                Instant::now() < admission_deadline,
                "serial mock admission timed out before spawn"
            );
            assert!(
                !FIXTURE_POISONED.load(Ordering::Acquire),
                "exact failed fixture is retained"
            );
            match FIXTURE_ADMISSION.try_lock() {
                Ok(guard) => break guard,
                Err(std::sync::TryLockError::Poisoned(error)) => break error.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) => {
                    std::thread::sleep(Duration::from_millis(1))
                }
            }
        };
        SOURCE_SLOTS.with(|slots| {
            assert!(
                slots.borrow().iter().all(Option::is_none),
                "prior retained mock slot cannot be reused"
            );
        });
        let observer = ObservationRun::install().unwrap();
        let verifier_guard = crate::occurrence_media::verification_test_guard();
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = CitrusApp::new_boxed_with_services(&cc, false);
        let mut desired = source_project();
        desired.channels[0].mixer_track = 1;
        app.project = desired.clone();
        app.project.channels[0].instrument_plugin_instance_id = None;
        app.project.plugin_instances.clear();
        app.saved_path = None;
        app.autosave_path = None;
        app.transport_mode = TransportMode::Song;
        app.selected_clip = Some(app.project.clips[0].id);
        app.playlist_selection_ids = HashSet::from([app.project.clips[0].id]);
        let (mut audio, mut callback) = AudioEngine::test_engine_with_callback();
        callback.enable_genuine_observation(RUN).unwrap();
        assert!(audio.set_project_session(app.project_session));
        app.audio = Some(audio);
        app.rebuild_tempo_map();
        app.request_immediate_timeline_compile();
        let directory = std::env::temp_dir().join(format!(
            "citrus-app-owned-topology-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir(&directory).unwrap();
        let fixture = Fixture {
            app: OwnedApp(Some(app)),
            ctx,
            callback: Some(callback),
            source: OwnedSource(None),
            directory,
            verifier_guard: Some(verifier_guard),
            admission_guard: Some(admission),
            cleanup_budget: MOCK_WAIT,
        };
        let mut result = Self {
            fixture,
            observer,
            ledger: OwnedTopologyLedger::new(RUN),
            poll: 0,
            record_cursor: 0,
            initial_worker: 0,
            initial_endpoint: 0,
            callback_receipts: [None; 4],
            callback_receipt_count: 0,
            last_disposition: None,
        };
        result.wait(None, |test| {
            test.exact_paused() && test.snapshot().graph_was_activated
        });
        let baseline = result.snapshot();
        assert_eq!(baseline.plugin_topology_revision, 0);
        assert_eq!(baseline.timeline_execution_failures, 0);
        assert!(baseline.endpoints.is_empty());
        assert!(baseline.graph_binding_exact);
        assert!(!baseline.plugin_topology_replan_pending);
        let channel = desired.channels[0].id;
        let instance = desired.channels[0].instrument_plugin_instance_id.unwrap();
        let mixer = desired
            .mixer_runtime_slot(desired.channels[0].mixer_track)
            .unwrap();
        let candidate_state = snapshot_channel_generator(&desired, channel).unwrap();
        let endpoint_id = result.fixture.app.allocate_generator_endpoint_id();
        result
            .fixture
            .app
            .pending_generator_chains
            .try_reserve(1)
            .unwrap();
        let slot = reserve_source_slot().unwrap();
        let config = PluginPrepareConfig {
            sample_rate: 48_000.0,
            max_block_frames: MAX_MIXER_BLOCK_FRAMES,
        };
        let (chain, source) = if held {
            TestCaptureSource::new_with_held_prepare(instance, source_state(), config).unwrap()
        } else {
            TestCaptureSource::new(instance, source_state(), config).unwrap()
        };
        let worker = bind_source_slot(slot, &chain);
        let PluginChain {
            audio: endpoint,
            control,
            guard,
        } = chain;
        // Exact initial mock seam: successful spawn transfers immediately into
        // the same retained PendingGeneratorChain used by the existing fixture.
        // No successful Installed or running owner is manufactured here.
        let project_session = result.fixture.app.project_session;
        result.fixture.app.pending_generator_chains.insert(
            channel,
            PendingGeneratorChain {
                project_session,
                endpoint_id,
                instance_id: instance,
                mixer_track: mixer,
                control,
                guard,
                previous_state: None,
                candidate_state,
            },
        );
        result.fixture.source = OwnedSource(Some(source));
        result.fixture.app.project = desired;
        result.initial_worker = worker;
        result.initial_endpoint = endpoint_id;
        assert!(
            result
                .fixture
                .app
                .audio
                .as_mut()
                .unwrap()
                .install_generator_endpoint_for_project(
                    project_session,
                    channel,
                    endpoint_id,
                    instance,
                    mixer,
                    endpoint,
                )
        );
        // No manual fingerprint refresh, forced generation or activation here.
        // The next unchanged ordinary poll owns the normal debounce/compile path.
        result
    }

    fn snapshot(&self) -> GenuineCallbackSnapshot {
        self.fixture.callback().genuine_snapshot()
    }
    fn exact_paused(&self) -> bool {
        let app = self.fixture.app();
        let snapshot = self.snapshot();
        app.timeline_audio_matches_desired()
            && app
                .audio
                .as_ref()
                .and_then(AudioEngine::confirmed_timeline_identity)
                == snapshot.generation.zip(snapshot.active_epoch)
            && snapshot.generation == Some(app.timeline_desired_generation)
            && snapshot.active_epoch == Some(snapshot.epoch)
            && !app.playing
            && !snapshot.playing
            && snapshot.start_frame == 0
            && !snapshot.suspended
            && !snapshot
                .timeline_stats
                .is_some_and(|stats| stats.ownership_needs_resync)
            && !snapshot.plugin_topology_replan_pending
    }

    fn records(&self) -> Vec<Arc<observe::Record>> {
        self.observer.with_snapshot(|snapshot| {
            assert!(
                snapshot.failure.is_none(),
                "bounded App observer failed: {:?}",
                snapshot.failure
            );
            snapshot.records.clone()
        })
    }

    fn ordinary_poll(&mut self) {
        self.poll += 1;
        let ctx = self.fixture.ctx();
        let app = self.fixture.app_mut();
        // Byte-for-byte operation ordering of GenuineFixture::poll; no extra
        // endpoint, restoration, Timeline or worker poll between these calls.
        app.drive_clip_session_retirement();
        app.poll_clip_restorations();
        app.poll_generator_endpoint_events();
        app.poll_plugin_runtime_events();
        app.poll_plugin_parameter_worker_receipts();
        app.poll_plugin_processing_health();
        app.poll_timeline_compile(&ctx);
        app.poll_audio_asset_events();
        app.poll_timeline_audio_runtime(&ctx);
        app.drive_clip_render_job();
    }

    fn checked_poll(&mut self) -> Result<(), String> {
        let result = self.checked_poll_inner();
        if let Err(error) = &result {
            self.report_failure("ordinary_poll", error);
        }
        result
    }

    fn checked_poll_inner(&mut self) -> Result<(), String> {
        self.ordinary_poll();
        // Independent App failures dominate a perfect lifecycle receipt.
        let ordinary_clear = topology::ordinary_health_clear_from_app(self.fixture.app())?;
        let records = self.records();
        let new = &records[self.record_cursor..];
        let events = new
            .iter()
            .filter_map(|record| match record.kind {
                RecordKind::AppGeneratorEndpointEvent(event) => Some(event),
                _ => None,
            })
            .collect::<Vec<_>>();
        let claim = self.ledger.open().and_then(|slot| slot.claim);
        let transfer = claim.is_some_and(|claim| {
            topology::owner_transfer_exact(self.fixture.app(), claim, &records)
        });
        self.ledger.consume_app_poll(self.poll, &events, transfer)?;
        if let Some(claim) = claim {
            let (issued, live) = topology::owned_timeline(self.fixture.app(), claim)?;
            self.ledger.observe_timeline_owner(issued, live)?;
        }
        for record in new {
            if let RecordKind::AppTimelineRuntimeEvent {
                event,
                before,
                after,
            } = record.kind
            {
                self.ledger.timeline_event(event, before, after)?;
            }
        }
        self.ledger
            .observe_issued(self.fixture.app().genuine_timeline_app_state())?;
        self.ledger.snapshot(&self.snapshot())?;
        if let Some(claim) = claim {
            let snapshot = self.snapshot();
            let evidence = topology::settlement_from_app(
                self.fixture.app(),
                claim,
                &records,
                &snapshot,
                ordinary_clear,
            );
            self.ledger.settle(evidence, &snapshot)?;
        }
        self.record_cursor = records.len();
        Ok(())
    }

    fn checked_pump(&mut self, kind: Option<OwnedTopologyKind>) -> Result<(), String> {
        let result = self.checked_pump_inner(kind);
        if let Err(error) = &result {
            self.report_failure("existing_control_pump", error);
        }
        result
    }

    fn checked_pump_inner(&mut self, kind: Option<OwnedTopologyKind>) -> Result<(), String> {
        let before = self.snapshot();
        // This is exactly the existing genuine control service predicate.
        if !before.callback_work_pending()
            && before.request_id
                == self
                    .fixture
                    .app
                    .audio
                    .as_ref()
                    .unwrap()
                    .test_transport_request_id()
        {
            return Ok(());
        }
        if (self.ledger.clear()
            || self
                .ledger
                .open()
                .is_some_and(|slot| slot.phase == OwnedTopologyPhase::Claimed))
            && let Some(kind) = kind
        {
            let worker = if matches!(
                kind,
                OwnedTopologyKind::InitialInstall | OwnedTopologyKind::ConversionRemove
            ) {
                self.initial_worker
            } else {
                latest_worker()
            };
            let (previous_endpoint, previous_worker) = if kind == OwnedTopologyKind::RedoRemove {
                (
                    self.fixture
                        .app
                        .running_generator_chains
                        .values()
                        .next()
                        .map(|source| source.endpoint_id),
                    Some(worker),
                )
            } else {
                (Some(self.initial_endpoint), Some(self.initial_worker))
            };
            if let Some(claim) = topology::claim_from_app(
                self.fixture.app(),
                &before,
                kind,
                before.pump_ordinal + 1,
                self.poll + 1,
                worker,
                previous_endpoint,
                previous_worker,
            )? {
                self.ledger.claim(claim)?;
            }
        }
        self.fixture.pump_callback_only();
        let after = self.snapshot();
        let receipts = self
            .fixture
            .callback_mut()
            .take_genuine_lifecycle_receipts();
        self.ledger
            .callback_receipts(&receipts, after.pump_ordinal, self.poll + 1)?;
        for receipt in &receipts {
            let Some(slot) = self.callback_receipts.get_mut(self.callback_receipt_count) else {
                return Err("more than four actual lifecycle receipts".into());
            };
            *slot = Some(*receipt);
            self.callback_receipt_count += 1;
        }
        let segments = self.fixture.callback_mut().take_genuine_segments();
        let output = self
            .fixture
            .callback()
            .last_output()
            .iter()
            .flat_map(|frame| frame.iter().copied())
            .collect::<Vec<_>>();
        assert_eq!(segments.len(), 1, "one exact Q128 callback segment");
        self.last_disposition = Some(segments[0].disposition);
        let role = self
            .ledger
            .open()
            .and_then(|slot| slot.claim)
            .map_or(TopologyPumpRole::Strict, |claim| {
                TopologyPumpRole::Owned(claim.kind)
            });
        self.ledger.segment(&segments[0], &output, role)?;
        self.ledger.snapshot(&after)?;
        Ok(())
    }

    fn report_failure(&self, boundary: &'static str, error: &str) {
        let snapshot = self.snapshot();
        let mut latest = self.observer.with_snapshot(|observed| {
            observed
                .records
                .iter()
                .rev()
                .filter_map(|record| {
                    let payload = match record.kind {
                        RecordKind::AppTimelineRuntimeEvent {
                            event,
                            before,
                            after,
                        } => {
                            serde_json::json!({
                                "type": "AppTimelineRuntimeEvent",
                                "event": topology::timeline_event_evidence(event),
                                "before": topology::timeline_state_evidence(before),
                                "after": topology::timeline_state_evidence(after),
                            })
                        }
                        RecordKind::AppGeneratorEndpointEvent(event) => serde_json::json!({
                            "type": "AppGeneratorEndpointEvent",
                            "event": topology::lifecycle_event_evidence(event),
                        }),
                        _ => return None,
                    };
                    Some(serde_json::json!({
                        "record_ordinal": record.ordinal,
                        "stage": format!("{:?}", record.stage),
                        "payload": payload,
                    }))
                })
                .take(16)
                .collect::<Vec<_>>()
        });
        latest.reverse();
        // Failure-only, bounded driver evidence. These wrappers do not consume
        // any queue, service an owner, retry a result, poll, or pump. No opaque
        // state, Project serialization, PCM or asset allocation is printed.
        eprintln!(
            "owned topology App qualification failure: {}",
            serde_json::json!({
                "boundary": boundary,
                "reason": error.chars().take(512).collect::<String>(),
                "ordinary_poll_ordinal": self.poll,
                "app_timeline": topology::timeline_state_evidence(self.fixture.app().genuine_timeline_app_state()),
                "callback": {
                    "pump_ordinal": snapshot.pump_ordinal,
                    "engine_session": snapshot.engine_session,
                    "project_session": snapshot.project_session,
                    "generation": snapshot.generation,
                    "active_epoch": snapshot.active_epoch,
                    "transport_epoch": snapshot.epoch,
                    "legacy_request_barrier": snapshot.request_id,
                    "start_frame": snapshot.start_frame,
                    "playing": snapshot.playing,
                    "plugin_topology_revision": snapshot.plugin_topology_revision,
                    "plugin_topology_replan_pending": snapshot.plugin_topology_replan_pending,
                    "timeline_execution_failures": snapshot.timeline_execution_failures,
                    "needs_resync": snapshot.timeline_stats.is_some_and(|stats| stats.ownership_needs_resync),
                    "suspended": snapshot.suspended,
                    "fault": snapshot.fault.map(|fault| format!("{fault:?}")),
                    "graph_identity": snapshot.graph_identity,
                    "graph_was_activated": snapshot.graph_was_activated,
                    "graph_binding_exact": snapshot.graph_binding_exact,
                    "channel_binding_revision": snapshot.channel_binding_revision,
                    "channel_binding_epoch": snapshot.channel_binding_epoch,
                    "pending_audio_commands": snapshot.pending_audio_commands,
                    "pending_timeline_commands": snapshot.pending_timeline_commands,
                    "pending_timeline_activation": snapshot.pending_timeline_activation,
                    "lifecycle_observed_count": snapshot.lifecycle_observed_count,
                    "lifecycle_overflowed": snapshot.lifecycle_overflowed,
                    "lifecycle_inconsistent_pair": snapshot.lifecycle_inconsistent_pair,
                    "segment_observation_overflowed": snapshot.observation_overflowed,
                    "timeline_missing_assets": snapshot.timeline_missing_assets,
                    "bridge_gaps": snapshot.fixed_quantum_bridge_gaps,
                    "bridge_underflow_frames": snapshot.fixed_quantum_output_underflow_frames,
                },
                "ledger": self.ledger.evidence(),
                "latest_app_events": latest,
            })
        );
    }

    fn step(&mut self, kind: Option<OwnedTopologyKind>) -> Result<(), String> {
        self.checked_poll()?;
        self.checked_pump(kind)
    }
    fn wait(&mut self, kind: Option<OwnedTopologyKind>, condition: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + MOCK_WAIT;
        while !condition(self) {
            assert!(
                Instant::now() < deadline,
                "unchanged App sequencing failed bounded recovery: {}",
                self.fixture.timeout_metadata()
            );
            if let Err(error) = self.step(kind) {
                panic!(
                    "unchanged App sequence is a qualification blocker: {error}; {}",
                    self.fixture.timeout_metadata()
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn wait_for_queued(&mut self, kind: OwnedTopologyKind) {
        let deadline = Instant::now() + MOCK_WAIT;
        loop {
            self.checked_poll()
                .unwrap_or_else(|error| panic!("pre-mutation App ordering: {error}"));
            let app = self.fixture.app();
            let queued = match kind {
                OwnedTopologyKind::InitialInstall | OwnedTopologyKind::UndoInstall => {
                    !app.pending_generator_chains.is_empty()
                }
                OwnedTopologyKind::ConversionRemove => {
                    app.clip_render_job.as_ref().is_some_and(|job| {
                        job.phase == ClipRenderPhase::Removing && job.removal_queued
                    })
                }
                OwnedTopologyKind::RedoRemove => !app.pending_generator_removals.is_empty(),
            };
            if queued {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "actual lifecycle operation never queued: {}",
                self.fixture.timeout_metadata()
            );
            self.checked_pump(None).unwrap();
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn mutation_gap(&mut self, kind: OwnedTopologyKind) -> topology::OwnedTopologyClaim {
        self.wait_for_queued(kind);
        let consumed_before = self
            .records()
            .iter()
            .filter(|record| matches!(record.kind, RecordKind::AppGeneratorEndpointEvent(_)))
            .count();
        let deadline = Instant::now() + MOCK_WAIT;
        loop {
            self.checked_pump(Some(kind)).unwrap();
            if self.ledger.open().is_some_and(|slot| {
                slot.phase == OwnedTopologyPhase::CallbackAcceptedAwaitingAppPoll
            }) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "queued operation never produced accepted mutation receipt"
            );
            self.checked_poll().unwrap();
            std::thread::sleep(Duration::from_millis(10));
        }
        let slot = self
            .ledger
            .open()
            .expect("actual receipt retains open barrier");
        assert_eq!(
            slot.phase,
            OwnedTopologyPhase::CallbackAcceptedAwaitingAppPoll
        );
        let claim = slot.claim.unwrap();
        assert_eq!(claim.next_poll, self.poll + 1);
        assert_eq!(
            self.records()
                .iter()
                .filter(|record| matches!(record.kind, RecordKind::AppGeneratorEndpointEvent(_)))
                .count(),
            consumed_before
        );
        assert!(
            self.ledger.settled_baseline().is_err(),
            "fixed-window entry must refuse the receipt-to-poll gap"
        );
        assert!(
            !self.exact_paused(),
            "an old Active phase cannot settle accepted topology"
        );
        let snapshot = self.snapshot();
        assert_eq!(snapshot.plugin_topology_revision, kind.index() as u64 + 1);
        assert_eq!(
            snapshot.timeline_execution_failures,
            kind.index() as u64 + 1
        );
        assert!(
            snapshot.plugin_topology_replan_pending
                && snapshot.suspended
                && snapshot
                    .timeline_stats
                    .is_some_and(|stats| stats.ownership_needs_resync)
        );
        assert!(!snapshot.playing && snapshot.fault.is_none());
        claim
    }

    fn settled(&self, kind: OwnedTopologyKind) -> bool {
        self.ledger.slots[kind.index()].phase == OwnedTopologyPhase::Settled && self.ledger.clear()
    }
    fn complete_initial(&mut self) {
        let claim = self.mutation_gap(OwnedTopologyKind::InitialInstall);
        assert!(
            self.fixture
                .app
                .pending_generator_chains
                .contains_key(&claim.channel)
        );
        self.checked_poll().unwrap();
        assert!(self.fixture.app.pending_generator_chains.is_empty());
        assert_eq!(
            self.fixture.app.running_generator_chains[&claim.channel].endpoint_id,
            claim.endpoint
        );
        assert_eq!(self.ledger.slots[0].consumed_poll, Some(claim.next_poll));
        self.wait(Some(OwnedTopologyKind::InitialInstall), |test| {
            test.settled(OwnedTopologyKind::InitialInstall)
                && test.fixture.app.clip_render_observe(claim.channel).is_ok()
        });
        // Existing fixture's stale Project/live-source preparation follows the
        // completed actual startup receipt. It creates no Timeline receipt.
        self.fixture.source.set_capture_state(source_state());
        self.fixture.app.project.plugin_instances[0]
            .parameters
            .insert(17, 0.25);
        self.fixture.app.project.plugin_instances[0].opaque_state = vec![1, 2, 3];
        self.fixture.app.history_snapshot = self.fixture.app.project.clone();
        self.fixture.app.history_fingerprint = project_fingerprint(&self.fixture.app.project);
        self.fixture.app.project_fingerprint = self.fixture.app.history_fingerprint;
        self.fixture.app.undo_stack.clear();
        self.fixture.app.redo_stack.clear();
        self.fixture.app.dirty = false;
    }
    fn complete_conversion(&mut self) {
        self.fixture.begin();
        let claim = self.mutation_gap(OwnedTopologyKind::ConversionRemove);
        let job = self.fixture.app.clip_render_job.as_ref().unwrap();
        assert_eq!(job.removal, claim.stamp);
        assert!(job.expected_timeline_generation.is_none());
        assert!(!job.source_removed && !job.source_joined);
        self.checked_poll().unwrap();
        let job = self.fixture.app.clip_render_job.as_ref().unwrap();
        assert!(job.source_removed && job.model_commit_started && job.commit.is_some());
        assert!(
            job.expected_timeline_generation.is_none(),
            "media acknowledgment must precede numeric successor issuance"
        );
        assert_eq!(self.ledger.slots[1].consumed_poll, Some(claim.next_poll));
        assert!(self.ledger.slots[1].timeline.is_none());
        self.wait(Some(OwnedTopologyKind::ConversionRemove), |test| {
            test.settled(OwnedTopologyKind::ConversionRemove)
                && test
                    .fixture
                    .app
                    .clip_render_job
                    .as_ref()
                    .is_some_and(|job| job.phase == ClipRenderPhase::Complete)
        });
        let job = self.fixture.app.clip_render_job.as_ref().unwrap();
        assert!(job.source_joined && job.retired_source.is_none());
        assert_eq!(job.asset_ack, Some(true));
        assert_eq!(
            job.expected_timeline_generation,
            self.ledger.slots[1]
                .timeline
                .map(|timeline| timeline.generation)
        );
        assert!(self.joined_once(self.initial_worker));
        assert_eq!(self.fixture.source.capture_count(), 2);
    }
    fn complete_undo(&mut self) -> (u64, usize) {
        self.fixture.app.clip_restoration_test_factory = Some(restoration_factory);
        self.fixture.app.undo();
        assert!(self.fixture.app.clip_restoration_playback_pending);
        let claim = self.mutation_gap(OwnedTopologyKind::UndoInstall);
        assert_ne!(claim.endpoint, self.initial_endpoint);
        assert_ne!(claim.worker, self.initial_worker);
        assert_eq!(
            self.fixture
                .app
                .clip_restoration_installs
                .get(&claim.instance),
            Some(&(claim.endpoint, false))
        );
        self.checked_poll().unwrap();
        assert_eq!(
            self.fixture
                .app
                .clip_restoration_installs
                .get(&claim.instance),
            Some(&(claim.endpoint, true))
        );
        assert!(
            self.fixture.app.clip_restoration_timeline.is_none(),
            "restoration poll precedes this exact install consumption"
        );
        assert_eq!(self.ledger.slots[2].consumed_poll, Some(claim.next_poll));
        assert!(self.ledger.slots[2].timeline.is_none());
        assert!(self.ledger.settled_baseline().is_err());
        self.wait(Some(OwnedTopologyKind::UndoInstall), |test| {
            test.settled(OwnedTopologyKind::UndoInstall)
        });
        assert!(
            self.poll > claim.next_poll,
            "generation issuance needs the next ordinary restoration poll"
        );
        assert_eq!(
            self.fixture
                .app
                .clip_restoration_timeline
                .map(|(_, generation)| generation),
            self.ledger.slots[2]
                .timeline
                .map(|timeline| timeline.generation)
        );
        assert!(!self.fixture.app.clip_restoration_playback_pending);
        assert_eq!(source_count(), 2);
        (claim.endpoint, claim.worker)
    }
    fn joined_once(&self, worker: usize) -> bool {
        self.records().iter().filter(|record| matches!(record.kind, RecordKind::WorkerJoin { worker_id, outcome: WorkerJoinOutcome::Joined } if worker_id == worker)).count() == 1
    }
}

impl Drop for TopologyFixture {
    fn drop(&mut self) {
        let was_panicking = std::thread::panicking();
        // One five-second budget, including unwind. Fixture's fallback receives
        // no second wait budget; a Pending owner is immediately quarantined.
        self.fixture.cleanup_budget = Duration::ZERO;
        let cleanup = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.fixture.drain_retained(Instant::now() + MOCK_WAIT)
        }));
        let joined = matches!(cleanup, Ok(Ok(()))) && self.observer.with_snapshot(|records| {
            records.failure.is_none() && SOURCE_SLOTS.with(|slots| {
                slots.borrow().iter().flatten().all(|slot| match slot {
                    SourceSlot::Reserved => false,
                    SourceSlot::Owned { worker, stats } => stats.detached_workers == 0 && stats.shutdown_timeouts == 0
                        && records.records.iter().filter(|record| matches!(record.kind, RecordKind::WorkerJoin { worker_id, outcome: WorkerJoinOutcome::Joined } if worker_id == *worker)).count() == 1,
                })
            })
        });
        if joined {
            SOURCE_SLOTS.with(|slots| *slots.borrow_mut() = [None; 4]);
        } else {
            FIXTURE_POISONED.store(true, Ordering::Release);
            if !was_panicking {
                panic!(
                    "exact mock cleanup not proven; pending App/guards and reserved source slots remain retained"
                );
            }
        }
    }
}

#[test]
fn owned_topology_actual_app_initial_install_from_activated_empty_graph() {
    let mut test = TopologyFixture::queued_initial();
    test.complete_initial();
    assert_eq!(test.ledger.settled_baseline().unwrap().failures, 1);
    assert_eq!(test.callback_receipt_count, 1);
}

#[test]
fn owned_topology_actual_app_pending_prepare_is_rendered_without_receipt_authority() {
    let mut test = TopologyFixture::queued_initial_with_held_prepare(true);
    test.wait_for_queued(OwnedTopologyKind::InitialInstall);
    test.checked_pump(Some(OwnedTopologyKind::InitialInstall))
        .unwrap();
    assert_eq!(
        test.last_disposition,
        Some(GenuineSegmentDisposition::Rendered)
    );
    assert_eq!(
        test.ledger.open().unwrap().phase,
        OwnedTopologyPhase::Claimed
    );
    assert!(test.ledger.open().unwrap().receipt.is_none());
    assert_eq!(test.callback_receipt_count, 0);
    assert_eq!(test.snapshot().plugin_topology_revision, 0);
    assert_eq!(test.snapshot().timeline_execution_failures, 0);
    assert!(test.fixture.app.running_generator_chains.is_empty());
    assert_eq!(test.fixture.app.pending_generator_chains.len(), 1);
    let pending = test
        .fixture
        .app
        .pending_generator_chains
        .values()
        .next()
        .unwrap();
    assert_eq!(pending.control.genuine_worker_id(), test.initial_worker);
    assert_eq!(pending.guard.genuine_worker_id(), test.initial_worker);
    let pending_snapshot = test.snapshot();
    assert!(
        topology::claim_from_app(
            test.fixture.app(),
            &pending_snapshot,
            OwnedTopologyKind::InitialInstall,
            pending_snapshot.pump_ordinal + 1,
            test.poll + 1,
            test.initial_worker.wrapping_add(1),
            Some(test.initial_endpoint),
            Some(test.initial_worker),
        )
        .is_err(),
        "a foreign supplied worker cannot override the actual pending control/guard identity"
    );
    assert!(
        test.observer.with_snapshot(|observed| observed
            .records
            .iter()
            .all(|record| record.instance.is_none())),
        "the held real pending owner must bind without a backend InstanceObserver record"
    );
    let mut unowned_suspension = test.snapshot();
    unowned_suspension.suspended = true;
    let mut negative = OwnedTopologyLedger::new(RUN);
    negative
        .claim(test.ledger.open().unwrap().claim.unwrap())
        .unwrap();
    assert!(
        negative.snapshot(&unowned_suspension).is_err(),
        "pending preparation cannot authorize suspension"
    );
    assert!(test.ledger.settled_baseline().is_err());
    test.fixture.source.release_prepare();
    test.complete_initial();
    assert_eq!(test.callback_receipt_count, 1);
}

#[test]
fn owned_topology_actual_receipt_rejects_unowned_wrong_worker_extra_count_and_late_poll() {
    let mut test = TopologyFixture::queued_initial();
    let claim = test.mutation_gap(OwnedTopologyKind::InitialInstall);
    let receipt = test.callback_receipts[0].unwrap();
    let mut unowned = OwnedTopologyLedger::new(RUN);
    assert!(
        unowned
            .callback_receipts(&[receipt], receipt.pump_ordinal, claim.next_poll)
            .is_err()
    );
    for changed in [
        {
            let mut changed = receipt;
            changed.affected.as_mut().unwrap().worker_id += 1;
            changed
        },
        {
            let mut changed = receipt;
            changed.post.timeline_execution_failures += 1;
            changed
        },
        {
            let mut changed = receipt;
            changed.enqueue_success = false;
            changed
        },
    ] {
        let mut ledger = OwnedTopologyLedger::new(RUN);
        ledger.claim(claim).unwrap();
        assert!(
            ledger
                .callback_receipts(&[changed], receipt.pump_ordinal, claim.next_poll)
                .is_err()
        );
    }
    let mut duplicate = OwnedTopologyLedger::new(RUN);
    duplicate.claim(claim).unwrap();
    assert!(
        duplicate
            .callback_receipts(&[receipt, receipt], receipt.pump_ordinal, claim.next_poll)
            .is_err()
    );
    // Consume only through the same ordinary App poll, then derive transfer
    // evidence from its actual resulting retained owner. The negative ledger
    // replay changes the claimed poll number, never the production receipt.
    test.ordinary_poll();
    let records = test.records();
    let transferred = topology::owner_transfer_exact(test.fixture.app(), claim, &records);
    assert!(transferred);
    let mut late = OwnedTopologyLedger::new(RUN);
    late.claim(claim).unwrap();
    late.callback_receipts(&[receipt], receipt.pump_ordinal, claim.next_poll)
        .unwrap();
    assert!(
        late.consume_app_poll(claim.next_poll + 1, &[receipt.event], transferred)
            .is_err()
    );
}

#[test]
fn owned_topology_actual_pending_receipt_never_launders_app_fault_or_retry() {
    let mut test = TopologyFixture::queued_initial();
    let claim = test.mutation_gap(OwnedTopologyKind::InitialInstall);
    let receipt = test.callback_receipts[0].unwrap();
    let state = test.fixture.app.genuine_timeline_app_state();
    for phase in [
        TimelinePhase::Faulted,
        TimelinePhase::FaultClearReady {
            revision: claim.old_generation,
        },
        TimelinePhase::FaultClearQueued {
            request_id: 1,
            revision: claim.old_generation,
        },
    ] {
        let mut ledger = OwnedTopologyLedger::new(RUN);
        ledger.claim(claim).unwrap();
        ledger
            .callback_receipts(&[receipt], receipt.pump_ordinal, claim.next_poll)
            .unwrap();
        assert!(
            ledger
                .observe_issued(observe::TimelineAppState { phase, ..state })
                .is_err()
        );
    }
    assert!(!test.fixture.app.plugin_processing_retry_required);
    test.fixture.app.plugin_processing_retry_pending =
        Some((claim.old_generation, claim.old_epoch));
    // Ordinary health uses the independent required flag. The actual pending
    // owner still makes it Wait; the topology ledger rejects pending retry.
    assert_eq!(
        topology::ordinary_health_clear_from_app(test.fixture.app()),
        Ok(false)
    );
    let assert_retry_rejected = |retry_state: observe::TimelineAppState| {
        let mut ledger = OwnedTopologyLedger::new(RUN);
        ledger.claim(claim).unwrap();
        ledger
            .callback_receipts(&[receipt], receipt.pump_ordinal, claim.next_poll)
            .unwrap();
        assert_eq!(
            ledger.observe_issued(retry_state).unwrap_err(),
            "terminal App Timeline fault or retry"
        );
        assert_eq!(ledger.evidence()["terminal_failure"], true);
        assert!(ledger.settled_baseline().is_err());
        assert_eq!(
            ledger.observe_issued(state).unwrap_err(),
            "owned topology ledger already terminal"
        );
    };
    let pending_state = test.fixture.app.genuine_timeline_app_state();
    assert!(!pending_state.retry_required);
    assert_eq!(
        pending_state.retry_pending,
        Some((claim.old_generation, claim.old_epoch))
    );
    assert_retry_rejected(pending_state);

    test.fixture.app.plugin_processing_retry_required = true;
    let error = topology::ordinary_health_clear_from_app(test.fixture.app())
        .expect_err("required retry must fail ordinary health independently");
    let evidence: serde_json::Value = serde_json::from_str(
        error
            .strip_prefix("ordinary App health failed during owned topology: ")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(evidence["health"]["classification"], "Failed");
    assert_eq!(
        evidence["health"]["failed_operands"],
        serde_json::json!(["plugin_processing_retry_required"])
    );
    // This separate negative ledger removes pending only from the copied
    // observation, proving required alone is fatal without clearing App state.
    let required_state = test.fixture.app.genuine_timeline_app_state();
    assert!(required_state.retry_required && required_state.retry_pending.is_some());
    assert_retry_rejected(observe::TimelineAppState {
        retry_pending: None,
        ..required_state
    });
    // Both actual retry fields remain set through bounded cleanup; no fault or
    // runtime resync is cleared to manufacture later success.
}

#[test]
fn owned_topology_actual_rejected_timeline_receipt_preserves_handler_early_return() {
    let mut test = TopologyFixture::queued_initial();
    let claim = test.mutation_gap(OwnedTopologyKind::InitialInstall);
    let receipt = test.callback_receipts[0].unwrap();
    // This is deliberately a failing fixture. Consume the lifecycle only via
    // the next existing poll, then queue a real wrong-generation Clear through
    // AudioEngine. No successful runtime receipt is injected or fabricated.
    test.ordinary_poll();
    let records = test.records();
    let transferred = topology::owner_transfer_exact(test.fixture.app(), claim, &records);
    assert!(transferred);
    let wrong_generation = claim.old_generation.checked_add(1_000).unwrap();
    let request = test
        .fixture
        .app
        .audio
        .as_mut()
        .unwrap()
        .clear_compiled_timeline(wrong_generation)
        .unwrap();
    assert!(test.snapshot().callback_work_pending());
    test.fixture.pump_callback_only();
    test.ordinary_poll();
    let records = test.records();
    let consumed = records
        .iter()
        .find_map(|record| match record.kind {
            RecordKind::AppTimelineRuntimeEvent {
                event:
                    event @ TimelineRuntimeEvent::Rejected {
                        request_id,
                        revision,
                        ..
                    },
                before,
                after,
            } if request_id == request && revision == wrong_generation => {
                Some((event, before, after))
            }
            _ => None,
        })
        .expect("actual rejected callback event must be observed at App consumption");
    assert_eq!(
        consumed.2.stale_confirmations,
        consumed.1.stale_confirmations + 1,
        "post-state must survive the original handler's stale-event early return"
    );
    let mut ledger = OwnedTopologyLedger::new(RUN);
    ledger.claim(claim).unwrap();
    ledger
        .callback_receipts(&[receipt], receipt.pump_ordinal, claim.next_poll)
        .unwrap();
    ledger
        .consume_app_poll(claim.next_poll, &[receipt.event], transferred)
        .unwrap();
    assert!(
        ledger
            .timeline_event(consumed.0, consumed.1, consumed.2)
            .is_err()
    );
    assert!(ledger.settled_baseline().is_err());
    // A later recovery is deliberately not attempted. Retained cleanup cannot
    // change the failed receipt or turn this barrier into success.
}

#[test]
fn owned_topology_actual_app_conversion_requires_join_asset_and_new_timeline() {
    let mut test = TopologyFixture::queued_initial();
    test.complete_initial();
    test.complete_conversion();
    assert_eq!(test.ledger.settled_baseline().unwrap().failures, 2);
    assert_eq!(test.callback_receipt_count, 2);
}

#[test]
fn owned_topology_actual_app_undo_fresh_install_issues_generation_after_receipt_poll() {
    let mut test = TopologyFixture::queued_initial();
    test.complete_initial();
    test.complete_conversion();
    test.complete_undo();
    assert_eq!(test.ledger.settled_baseline().unwrap().failures, 3);
    assert_eq!(test.callback_receipt_count, 3);
}

#[test]
fn owned_topology_actual_app_redo_physically_removes_undo_worker_and_reuses_pcm() {
    let mut test = TopologyFixture::queued_initial();
    test.complete_initial();
    test.complete_conversion();
    let committed = serde_json::to_vec(&test.fixture.app.project).unwrap();
    let asset = test.fixture.app.project.audio_assets[0].id;
    let pcm = Arc::clone(&test.fixture.app.cached_audio_assets[&asset].samples);
    let render_reports = test
        .records()
        .iter()
        .filter(|record| matches!(record.kind, RecordKind::RenderReport(_)))
        .count();
    let (undo_endpoint, undo_worker) = test.complete_undo();
    test.observer.set_stage(observe::Stage::Redo).unwrap();
    test.fixture.app.redo();
    let claim = test.mutation_gap(OwnedTopologyKind::RedoRemove);
    assert_eq!((claim.endpoint, claim.worker), (undo_endpoint, undo_worker));
    assert!(
        matches!(test.callback_receipts[3].unwrap().event, GeneratorEndpointEvent::Removed {
        endpoint_id: Some(endpoint), plugin_instance_id: Some(41), ..
    } if endpoint == undo_endpoint)
    );
    test.checked_poll().unwrap();
    assert!(test.fixture.app.running_generator_chains.is_empty());
    assert!(test.fixture.app.pending_generator_removals.is_empty());
    if let Some(guard) = test.fixture.app.retired_clip_restorations.first() {
        assert_eq!(test.fixture.app.retired_clip_restorations.len(), 1);
        assert_eq!(guard.genuine_worker_id(), undo_worker);
    } else {
        assert!(test.joined_once(undo_worker));
    }
    let mut foreign_claim = claim;
    foreign_claim.worker = test.initial_worker;
    assert!(!topology::owner_transfer_exact(
        test.fixture.app(),
        foreign_claim,
        &test.records()
    ));
    assert!(test.fixture.app.clip_restoration_timeline.is_none());
    assert_eq!(test.ledger.slots[3].consumed_poll, Some(claim.next_poll));
    test.wait(Some(OwnedTopologyKind::RedoRemove), |test| {
        test.ledger.complete()
    });
    assert!(test.joined_once(undo_worker));
    assert_eq!(
        serde_json::to_vec(&test.fixture.app.project).unwrap(),
        committed
    );
    assert!(Arc::ptr_eq(
        &pcm,
        &test.fixture.app.cached_audio_assets[&asset].samples
    ));
    assert!(test.fixture.app.retired_clip_restorations.is_empty());
    assert!(test.fixture.app.pending_clip_restorations.is_empty());
    assert_eq!(source_count(), 2);
    assert_eq!(test.fixture.source.capture_count(), 2);
    assert_eq!(
        test.records()
            .iter()
            .filter(|record| matches!(record.kind, RecordKind::RenderReport(_)))
            .count(),
        render_reports
    );
    assert_eq!(test.ledger.settled_baseline().unwrap().failures, 4);
}
