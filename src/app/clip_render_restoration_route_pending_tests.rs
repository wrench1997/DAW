//! Deterministic real-compiler-before-Ready Undo regression. The production
//! classifier is intentionally unchanged in the RED source snapshot. No test
//! result, Ready event, callback receipt, phase, generation or retry is injected.
use super::*;
use std::io::Write as _;

#[derive(Default)]
struct HeldPrepareSlot {
    reserved: bool,
    released: bool,
    source: Option<TestCaptureSource>,
}
thread_local! {
    // One gate is reserved before Undo can spawn. Its source handle must outlive
    // the factory return: dropping TestCaptureSource would release preparation.
    static HELD_PREPARE: std::cell::RefCell<HeldPrepareSlot> = const {
        std::cell::RefCell::new(HeldPrepareSlot {
            reserved: false,
            released: false,
            source: None,
        })
    };
}

struct HeldPrepareOwner;
impl HeldPrepareOwner {
    fn reserve() -> Self {
        HELD_PREPARE.with(|slot| {
            let mut slot = slot.borrow_mut();
            assert!(!slot.reserved && slot.source.is_none());
            slot.reserved = true;
        });
        Self
    }
    fn is_held(&self) -> bool {
        HELD_PREPARE.with(|slot| {
            let slot = slot.borrow();
            slot.reserved && slot.source.is_some() && !slot.released
        })
    }
    fn release(&self) {
        HELD_PREPARE.with(|slot| {
            let mut slot = slot.borrow_mut();
            if let Some(source) = &slot.source {
                source.release_prepare();
            }
            slot.released = true;
        });
    }
}
impl Drop for HeldPrepareOwner {
    fn drop(&mut self) {
        self.release();
        HELD_PREPARE.with(|slot| *slot.borrow_mut() = HeldPrepareSlot::default());
    }
}

fn held_restoration_factory(
    candidate: &Project,
    instance_id: u64,
    spec: PluginLoadSpec,
    config: PluginPrepareConfig,
    metadata: &BouncePluginMetadata,
) -> Result<PluginChain, String> {
    // Identical authoritative input checks to authoritative_mock_restoration_factory.
    let instance = candidate
        .plugin_instances
        .iter()
        .find(|item| item.id == instance_id)
        .ok_or("mock restoration candidate lost its exact instance")?;
    if instance_id != 41
        || !instance.enabled
        || instance.bypass
        || instance.wet.to_bits() != 1.0_f32.to_bits()
        || instance.parameters.get(&17).map(|value| value.to_bits()) != Some(0.75_f32.to_bits())
        || instance.opaque_state != [7, 8, 9]
        || spec.initial_state != instance.opaque_state
        || spec.descriptor.id != instance.uid
        || spec.descriptor.path != instance.path
        || spec.class_uid.as_deref() != Some(metadata.class_uid.as_str())
        || spec.descriptor.id == metadata.class_uid
        || config.sample_rate != 48_000.0
        || config.max_block_frames != MAX_MIXER_BLOCK_FRAMES
    {
        return Err(
            "mock factory did not receive exact authoritative source state and identity".into(),
        );
    }
    let state = BouncePluginState {
        metadata: metadata.clone(),
        opaque_state: instance.opaque_state.clone(),
        parameter_bases: instance.parameters.clone(),
        native_generation: 0,
        native_dirty_revision: 0,
    };
    HELD_PREPARE.with(|slot| {
        let mut owner = slot.borrow_mut();
        if !owner.reserved || owner.released || owner.source.is_some() {
            return Err("exact held preparation owner was not reserved before spawn".into());
        }
        let source_slot = reserve_source_slot()?;
        let (chain, source) = TestCaptureSource::new_with_held_prepare(instance_id, state, config)?;
        // No assertion, worker wait or fallible preparation after spawn. Retain
        // the gate immediately; the returned unsplit chain goes directly into
        // the existing production PendingClipRestoration owner.
        owner.source = Some(source);
        bind_source_slot(source_slot, &chain);
        Ok(chain)
    })
}

fn history_bytes(app: &CitrusApp) -> Vec<u8> {
    serde_json::to_vec(&(
        &app.history_snapshot,
        app.history_fingerprint,
        &app.undo_stack,
        &app.redo_stack,
        app.dirty,
    ))
    .unwrap()
}

fn assert_no_retry_or_fault(app: &CitrusApp, before: observe::TimelineAppState) {
    let state = app.genuine_timeline_app_state();
    assert!(!matches!(
        state.phase,
        TimelinePhase::Faulted
            | TimelinePhase::FaultClearReady { .. }
            | TimelinePhase::FaultClearQueued { .. }
    ));
    assert!(!state.retry_required && state.retry_pending.is_none());
    assert_eq!(state.retry_count, before.retry_count);
    assert_eq!(state.stale_confirmations, before.stale_confirmations);
    assert_eq!((state.retry_count, state.stale_confirmations), (0, 0));
    assert!(!app.clip_restoration_failed);
}

fn actual_undo_compile_is_current(test: &TopologyFixture, generation: u64) -> bool {
    let app = test.fixture.app();
    let Some(source) = &app.timeline_audio_source else {
        return false;
    };
    app.timeline_audio_source_has_current_authority()
        && app.timeline_desired_generation == generation
        && app.timeline_compiled_generation == Some(generation)
        && source.generation == generation
        && app
            .compiled_timeline
            .as_ref()
            .is_some_and(|compiled| Arc::ptr_eq(compiled, &source.timeline))
}

fn failed_held_poll(
    test: &TopologyFixture,
    gate: &HeldPrepareOwner,
    requirement: GeneratorRouteRequirement,
    generation: u64,
    error: &str,
) -> ! {
    let app = test.fixture.app();
    let pending = app.pending_clip_restorations.get(&requirement.channel_id);
    let source = app.timeline_audio_source.as_ref();
    let started_at = pending.map(|pending| pending.genuine_diagnostic_started_at());
    let first_rejected = test.ledger.first_rejected_issued_state();
    // One bounded first-failure artifact before unwind releases the held gate.
    // Retained ledger input is labeled separately from current App state; no
    // second ledger call, event drain, worker poll or callback pump is performed.
    let evidence = serde_json::json!({
        "boundary": "held_undo_ordinary_poll",
        "primary_error": error.chars().take(1024).collect::<String>(),
        "ordinary_poll": test.poll,
        "actual_undo_compile_consumed": actual_undo_compile_is_current(test, generation),
        "expected_undo_generation": generation,
        "first_rejected_issued_state": first_rejected.map(topology::timeline_state_evidence),
        "current_app_timeline": topology::timeline_state_evidence(app.genuine_timeline_app_state()),
        "timeline_error": app.timeline_audio_sync.last_error.as_ref().map(|s| s.chars().take(1024).collect::<String>()),
        "required_route": {
            "channel": requirement.channel_id,
            "instance": requirement.plugin_instance_id,
            "mixer": requirement.mixer_track,
        },
        "actual_required_routes": source.map(|source| format!("{:?}", timeline_generator_route_requirements(&source.timeline))),
        "actual_route_state": source.map(|source| format!("{:?}", app.timeline_generator_routes_state(&source.timeline))),
        "source": source.map(|source| serde_json::json!({
            "generation": source.generation,
            "fingerprint": source.fingerprint,
            "sample_rate": source.sample_rate,
            "live_fingerprint": app.clip_timeline_fingerprint(source.sample_rate),
        })),
        "gate_held": gate.is_held(),
        "pending_count": app.pending_clip_restorations.len(),
        "ready": pending.map(|pending| pending.genuine_diagnostic_ready()),
        "chain_workers": pending.and_then(|pending| pending.genuine_diagnostic_chain_workers()),
        "chain_audio_worker": pending.and_then(|pending| pending.genuine_test_chain_audio_worker()),
        "candidate_retained": pending.is_some_and(|pending| pending.genuine_test_candidate().is_some()),
        "transferred_workers": pending.map(|pending| pending.genuine_diagnostic_transferred_workers()),
        "started_at": started_at.map(|value| format!("{value:?}")),
        "original_deadline": started_at.and_then(|value| value.checked_add(Duration::from_secs(35))).map(|value| format!("{value:?}")),
        "elapsed_ms": started_at.map(|value| value.elapsed().as_millis()),
        "running_generators": app.running_generator_chains.len(),
        "pending_generators": app.pending_generator_chains.len(),
    });
    let diagnostic_write_error = writeln!(
        std::io::stderr().lock(),
        "held Undo restoration first failure: {evidence}"
    )
    .err();
    panic!(
        "held Undo must remain Pending after actual MIDI compile acceptance: {error}; diagnostic_write_error={diagnostic_write_error:?}"
    );
}

#[test]
fn actual_app_undo_compile_before_held_prepare_waits_for_exact_install_and_new_timeline() {
    let mut test = TopologyFixture::queued_initial();
    test.complete_initial();
    test.complete_conversion();
    let app = test.fixture.app();
    let requirement = GeneratorRouteRequirement {
        channel_id: app.project.channels[0].id,
        plugin_instance_id: 41,
        mixer_track: app
            .project
            .mixer_runtime_slot(app.project.channels[0].mixer_track)
            .unwrap(),
    };
    let authoritative_before =
        clip_render::project_snapshot(app.undo_stack.last().unwrap()).unwrap();
    let converted = clip_render::project_snapshot(&app.project).unwrap();
    let asset = app.project.audio_assets[0].id;
    let pcm = Arc::clone(&app.cached_audio_assets[&asset].samples);
    let pcm_bytes = app.retained_clip_pcm_bytes;
    let converted_generation = app.timeline_desired_generation;
    let before = app.genuine_timeline_app_state();
    assert!(
        timeline_generator_route_requirements(
            &app.timeline_audio_source.as_ref().unwrap().timeline
        )
        .unwrap()
        .is_empty()
    );
    assert!(test.joined_once(test.initial_worker));

    // Declared after `test`, so assertion unwind releases this exact gate BEFORE
    // TopologyFixture's unchanged five-second retained same-worker cleanup.
    let gate = HeldPrepareOwner::reserve();
    test.fixture.app.clip_restoration_test_factory = Some(held_restoration_factory);
    test.observer.set_stage(observe::Stage::Undo).unwrap();
    test.fixture.app.undo();
    let generation = test.fixture.app.timeline_desired_generation;
    assert!(generation > converted_generation);
    assert_eq!(source_count(), 2);
    let worker = latest_worker();
    assert_ne!(worker, test.initial_worker);
    let started_at = test.fixture.app.pending_clip_restorations[&requirement.channel_id]
        .genuine_diagnostic_started_at();
    let history = history_bytes(test.fixture.app());
    let held_revision = test.fixture.app.clip_render_model_revision;
    let assert_held = |test: &TopologyFixture| {
        let app = test.fixture.app();
        let pending = &app.pending_clip_restorations[&requirement.channel_id];
        assert!(gate.is_held());
        assert_eq!(app.pending_clip_restorations.len(), 1);
        assert!(!pending.genuine_diagnostic_ready());
        assert_eq!(pending.genuine_diagnostic_started_at(), started_at);
        assert_eq!(
            pending.genuine_diagnostic_chain_workers(),
            Some((worker, worker))
        );
        assert_eq!(pending.genuine_test_chain_audio_worker(), Some(worker));
        assert!(
            clip_render::project_matches_snapshot(
                pending
                    .genuine_test_candidate()
                    .expect("exact candidate remains retained"),
                &authoritative_before,
            )
            .unwrap()
        );
        assert_eq!(
            pending.genuine_diagnostic_transferred_workers(),
            (None, None)
        );
        assert!(app.running_generator_chains.is_empty());
        assert!(app.pending_generator_chains.is_empty());
        assert!(app.pending_generator_removals.is_empty());
        assert!(app.pending_generator_routes.is_empty());
        assert!(app.clip_restoration_installs.is_empty());
        assert!(app.clip_restoration_timeline.is_none());
        assert!(app.clip_restoration_playback_pending);
        assert!(!app.playing && !test.snapshot().playing);
        assert_eq!(app.timeline_desired_generation, generation);
        assert_eq!(app.clip_render_model_revision, held_revision);
        assert_eq!(history_bytes(app), history);
        assert!(
            clip_render::project_matches_snapshot(&app.project, &authoritative_before).unwrap()
        );
        assert_eq!(app.retained_clip_pcm_bytes, pcm_bytes);
        assert!(app.retained_clip_media.contains_key(&asset));
        assert_eq!(test.callback_receipt_count, 2);
        assert_no_retry_or_fault(app, before);
    };
    assert_held(&test);
    let deadline = Instant::now() + MOCK_WAIT;
    while !actual_undo_compile_is_current(&test, generation) {
        assert!(
            Instant::now() < deadline,
            "actual Undo compiler did not finish while preparation remained held"
        );
        if let Err(error) = test.checked_poll_inner() {
            failed_held_poll(&test, &gate, requirement, generation, &error);
        }
        assert_held(&test);
        test.checked_pump(None).unwrap();
        std::thread::sleep(Duration::from_millis(10));
    }
    let app = test.fixture.app();
    let source = app.timeline_audio_source.as_ref().unwrap();
    assert_eq!(
        timeline_generator_route_requirements(&source.timeline).unwrap(),
        vec![requirement]
    );
    assert_eq!(
        source.fingerprint,
        app.clip_timeline_fingerprint(source.sample_rate)
    );
    assert_eq!(
        app.timeline_generator_routes_state(&source.timeline),
        RequiredGeneratorRouteState::Pending {
            channel_id: requirement.channel_id,
            plugin_instance_id: requirement.plugin_instance_id,
        }
    );
    assert_eq!(
        app.timeline_audio_sync.phase,
        TimelineAudioSyncPhase::InstallReady
    );
    assert!(app.timeline_pending_chase.is_none() && app.timeline_pending_loop_chase.is_none());
    let held_state = app.genuine_timeline_app_state();
    let held_callback = test.snapshot();
    assert_eq!(held_callback.pending_timeline_commands, 0);
    assert!(!held_callback.pending_timeline_activation);
    assert_eq!(held_callback.generation, Some(converted_generation));
    assert_eq!(held_callback.plugin_topology_revision, 2);
    assert_eq!(held_callback.timeline_execution_failures, 2);
    assert!(held_callback.endpoints.is_empty());
    // An entire further ordinary poll/pump opportunity must remain safely held.
    if let Err(error) = test.checked_poll_inner() {
        failed_held_poll(&test, &gate, requirement, generation, &error);
    }
    test.checked_pump(None).unwrap();
    assert_held(&test);
    assert_eq!(test.fixture.app.genuine_timeline_app_state(), held_state);
    assert_eq!(test.snapshot().pending_timeline_commands, 0);
    assert!(!test.snapshot().pending_timeline_activation);
    assert_eq!(test.snapshot().pump_ordinal, held_callback.pump_ordinal);

    gate.release();
    let claim = test.mutation_gap(OwnedTopologyKind::UndoInstall);
    assert_eq!(
        (claim.channel, claim.instance, claim.worker),
        (requirement.channel_id, 41, worker)
    );
    assert_ne!(claim.endpoint, test.initial_endpoint);
    let pending = &test.fixture.app.pending_generator_chains[&requirement.channel_id];
    assert_eq!(pending.control.genuine_worker_id(), worker);
    assert_eq!(pending.guard.genuine_worker_id(), worker);
    assert_eq!(pending.endpoint_id, claim.endpoint);
    assert!(test.fixture.app.pending_clip_restorations.is_empty());
    assert!(test.fixture.app.running_generator_chains.is_empty());
    assert_eq!(
        test.fixture.app.clip_restoration_installs.get(&41),
        Some(&(claim.endpoint, false))
    );
    assert!(test.fixture.app.clip_restoration_timeline.is_none());
    assert!(test.fixture.app.clip_restoration_playback_pending);
    assert_eq!(test.callback_receipt_count, 3);
    assert_eq!(test.snapshot().endpoints[0].fixed.bridge.worker_id, worker);
    assert_eq!(
        test.fixture.app.timeline_generator_routes_state(
            &test
                .fixture
                .app
                .timeline_audio_source
                .as_ref()
                .unwrap()
                .timeline
        ),
        RequiredGeneratorRouteState::Pending {
            channel_id: requirement.channel_id,
            plugin_instance_id: requirement.plugin_instance_id,
        }
    );
    test.checked_poll().unwrap();
    assert_eq!(test.poll, claim.next_poll);
    assert!(test.fixture.app.pending_generator_chains.is_empty());
    let running = &test.fixture.app.running_generator_chains[&requirement.channel_id];
    assert_eq!(
        (
            running.control.genuine_worker_id(),
            running.guard.genuine_worker_id()
        ),
        (worker, worker)
    );
    assert_eq!(running.endpoint_id, claim.endpoint);
    assert_eq!(
        test.fixture.app.clip_restoration_installs.get(&41),
        Some(&(claim.endpoint, true))
    );
    assert!(
        test.fixture.app.clip_restoration_timeline.is_none(),
        "restoration poll precedes the exact Installed receipt consumption"
    );
    assert!(test.fixture.app.clip_restoration_playback_pending);
    assert_eq!(test.ledger.slots[2].consumed_poll, Some(claim.next_poll));
    assert!(test.ledger.slots[2].timeline.is_none());
    test.wait(Some(OwnedTopologyKind::UndoInstall), |test| {
        test.settled(OwnedTopologyKind::UndoInstall)
    });
    let app = test.fixture.app();
    let restored = test.ledger.slots[2].timeline.unwrap();
    assert!(test.poll > claim.next_poll);
    assert!(restored.generation > generation);
    assert_eq!(
        app.clip_restoration_timeline,
        Some((restored.engine, restored.generation))
    );
    assert_eq!(app.timeline_desired_generation, restored.generation);
    assert_eq!(app.timeline_observed_fingerprint, restored.fingerprint);
    assert!(test.exact_paused());
    assert!(!app.clip_restoration_playback_pending);
    assert_no_retry_or_fault(app, before);
    assert!(clip_render::project_matches_snapshot(&app.project, &authoritative_before).unwrap());
    assert!(
        clip_render::project_matches_snapshot(app.redo_stack.last().unwrap(), &converted).unwrap()
    );
    assert_eq!(history_bytes(app), history);
    let timeline = &app.timeline_audio_source.as_ref().unwrap().timeline;
    assert_eq!(
        timeline_generator_route_requirements(timeline).unwrap(),
        vec![requirement]
    );
    assert!(timeline.events().iter().any(|event| matches!(
        event.kind,
        crate::timeline::TimelineEventKind::NoteOn { .. }
    )));
    assert!(!timeline.events().iter().any(|event| matches!(
        event.kind,
        crate::timeline::TimelineEventKind::AudioStart(_)
    )));
    assert!(app.project.audio_assets.is_empty());
    assert_eq!(app.project.plugin_instances[0].opaque_state, [7, 8, 9]);
    assert_eq!(
        app.project.plugin_instances[0].parameters[&17].to_bits(),
        0.75_f32.to_bits()
    );
    assert_eq!(app.retained_clip_pcm_bytes, pcm_bytes);
    assert!(app.retained_clip_media.contains_key(&asset));
    assert!(!pcm.is_empty());
    assert_eq!(source_count(), 2);
    assert_eq!(test.fixture.source.capture_count(), 2);
    assert_eq!(test.callback_receipt_count, 3);
    assert_eq!(test.ledger.settled_baseline().unwrap().failures, 3);
    // Drop verifies exactly one healthy join per retained original/fresh worker,
    // with the unchanged bounded drain and quarantine-on-unproven-join policy.
    drop(gate);
    drop(test);
    assert!(!FIXTURE_POISONED.load(Ordering::Acquire));
    SOURCE_SLOTS.with(|slots| assert!(slots.borrow().iter().all(Option::is_none)));
}
