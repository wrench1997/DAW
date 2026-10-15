use super::*;

fn snapshot(revision: u64, active: [u32; 3], fault: bool) -> vst3_host::AsyncStateSnapshot {
    vst3_host::AsyncStateSnapshot {
        contract_version: 1,
        async_started_revision: revision,
        active_async: active[0],
        active_ui: active[1],
        active_other: active[2],
        readiness_faulted: fault,
    }
}

fn run_and_clone() -> (ObservationRun, InstanceObserver) {
    // Private run: no process-wide activation, SDK call, stream or PCM storage.
    let mut state = ObservationSnapshot::new(0, 0).unwrap();
    state.stage(Stage::InitialSource).unwrap();
    state.bind_instance(InstancePhase::InitialSource, None);
    state.stage(Stage::WholeExport).unwrap();
    let identity = state.bind_instance(
        InstancePhase::StrictClone,
        Some(WorkerIdentity {
            id: 17,
            manifest: PluginEndpointManifest::identified(&[23]).unwrap(),
        }),
    );
    assert_eq!(state.failure, None);
    let run = ObservationRun {
        state: Arc::new(Mutex::new(state)),
    };
    let observer = InstanceObserver {
        run: Some(Arc::downgrade(&run.state)),
        identity: Some(identity),
    };
    (run, observer)
}

#[test]
fn inactive_tap_does_not_attach_to_an_existing_private_run() {
    let (run, _) = run_and_clone();
    let before = run.async_observation_summary();
    InstanceObserver::default().strict_async_snapshot(
        AsyncObservationPhase::PrepareWait,
        snapshot(1, [1, 2, 3], true),
    );
    assert_eq!(run.async_observation_summary(), before);
    run.with_snapshot(|state| {
        assert!(state.records.is_empty());
        assert_eq!(state.private_bytes, 0);
    });
}

#[test]
fn phase_first_last_checked_count_maxima_and_fault_remain_actual_and_independent() {
    let (run, observer) = run_and_clone();
    let prepare_first = snapshot(7, [3, 1, 2], false);
    let prepare_middle = snapshot(8, [1, 6, 0], true);
    let prepare_last = snapshot(9, [0, 2, 4], false);
    let armed_first = snapshot(9, [0, 1, 0], false);
    let armed_last = snapshot(10, [1, 0, 0], true);
    for value in [prepare_first, prepare_middle, prepare_last] {
        observer.strict_async_snapshot(AsyncObservationPhase::PrepareWait, value);
    }
    for value in [armed_first, armed_last] {
        observer.strict_async_snapshot(AsyncObservationPhase::ArmedVerify, value);
    }
    let summary = run.async_observation_summary();
    assert_eq!(summary.failure, None);
    let clone = summary.strict_clone.unwrap();
    assert_eq!(clone.identity, observer.identity.unwrap());
    assert_eq!(clone.identity.worker_id, Some(17));
    assert_eq!(clone.identity.source_instance_id, Some(23));
    assert_eq!(
        clone.prepare_wait,
        Some(AsyncPhaseObservation {
            first: prepare_first,
            last: prepare_last,
            checked_count: 3,
            max_active_async: 3,
            max_active_ui: 6,
            max_active_other: 4,
            saw_fault: true,
        })
    );
    assert_eq!(
        clone.armed_verify,
        Some(AsyncPhaseObservation {
            first: armed_first,
            last: armed_last,
            checked_count: 2,
            max_active_async: 1,
            max_active_ui: 1,
            max_active_other: 0,
            saw_fault: true,
        })
    );
    assert_eq!(summary.evidence()["complete"], true);
    run.with_snapshot(|state| {
        assert!(state.records.is_empty());
        assert_eq!(state.private_bytes, 0);
        assert_eq!(state.clone_replies, 0);
        assert!(state.stream.is_none());
    });
}

#[test]
fn missing_owner_and_phase_stay_null_and_zero_is_only_an_observation() {
    let empty = AsyncObservationSummary::default().evidence();
    assert!(empty["strict_clone"].is_null());
    assert_eq!(empty["complete"], false);
    let (run, observer) = run_and_clone();
    let bound = run.async_observation_summary().evidence();
    assert!(bound["strict_clone"]["prepare_wait"].is_null());
    assert!(bound["strict_clone"]["armed_verify"].is_null());
    let zero = snapshot(11, [0, 0, 0], false);
    observer.strict_async_snapshot(AsyncObservationPhase::PrepareWait, zero);
    let evidence = run.async_observation_summary().evidence();
    assert_eq!(evidence["complete"], false);
    let phase = &evidence["strict_clone"]["prepare_wait"];
    assert_eq!(phase["checked_count"], 1);
    assert_eq!(phase["first"], serde_json::to_value(zero).unwrap());
    assert_eq!(phase["last"], serde_json::to_value(zero).unwrap());
    assert_eq!(phase["first"].as_object().unwrap().len(), 6);
    assert_eq!(evidence["strict_clone"]["identity"]["ordinal"], 2);
    assert_eq!(evidence["strict_clone"]["identity"]["phase"], "StrictClone");
    assert_eq!(evidence["strict_clone"]["identity"]["worker_id"], 17);
    assert_eq!(
        evidence["strict_clone"]["identity"]["source_instance_id"],
        23
    );
    assert!(
        evidence["interpretation"]
            .as_str()
            .unwrap()
            .contains("zero active counts do not prove absence")
    );
}

#[test]
fn every_identity_domain_and_missing_identity_reject_without_overwriting_evidence() {
    for domain in 0..5 {
        let (run, observer) = run_and_clone();
        observer.strict_async_snapshot(
            AsyncObservationPhase::PrepareWait,
            snapshot(1, [1, 0, 0], false),
        );
        let before = run.async_observation_summary().strict_clone;
        let mut identity = observer.identity.unwrap();
        match domain {
            0 => identity.ordinal += 1,
            1 => identity.phase = InstancePhase::InitialSource,
            2 => identity.worker_id = Some(18),
            3 => identity.source_instance_id = Some(24),
            4 => {}
            _ => unreachable!(),
        }
        let wrong = InstanceObserver {
            run: observer.run.clone(),
            identity: (domain != 4).then_some(identity),
        };
        wrong.strict_async_snapshot(
            AsyncObservationPhase::PrepareWait,
            snapshot(2, [0, 1, 0], true),
        );
        observer.strict_async_snapshot(
            AsyncObservationPhase::ArmedVerify,
            snapshot(3, [0, 0, 0], false),
        );
        let after = run.async_observation_summary();
        assert_eq!(after.strict_clone, before);
        assert_eq!(
            after.failure,
            Some("async observation instance identity mismatch")
        );
        assert_eq!(after.evidence()["complete"], false);
    }
    let mut unbound = AsyncObservationSummary::default();
    unbound.observe(
        None,
        AsyncObservationPhase::PrepareWait,
        snapshot(0, [0, 0, 0], false),
    );
    assert_eq!(unbound.strict_clone, None);
    assert_eq!(
        unbound.failure,
        Some("async observation has no bound strict clone")
    );
}

#[test]
fn duplicate_and_invalid_clone_binding_freeze_the_original_owner() {
    let (run, observer) = run_and_clone();
    observer.strict_async_snapshot(
        AsyncObservationPhase::PrepareWait,
        snapshot(1, [0, 1, 0], false),
    );
    let before = run.async_observation_summary().strict_clone;
    with_run(&run.state, |state| {
        state.bind_instance(InstancePhase::StrictClone, None);
    });
    observer.strict_async_snapshot(
        AsyncObservationPhase::ArmedVerify,
        snapshot(2, [0, 0, 0], false),
    );
    let after = run.async_observation_summary();
    assert_eq!(after.strict_clone, before);
    assert_eq!(
        after.failure,
        Some("async observation invalid or duplicate strict clone binding")
    );
    let mut state = ObservationSnapshot::new(0, 0).unwrap();
    state.bind_instance(InstancePhase::StrictClone, None);
    assert_eq!(state.async_observation.strict_clone, None);
    assert!(state.async_observation.failure.is_some());
}

#[test]
fn count_overflow_is_checked_before_any_phase_mutation_and_is_sticky() {
    let (run, observer) = run_and_clone();
    observer.strict_async_snapshot(
        AsyncObservationPhase::PrepareWait,
        snapshot(1, [1, 0, 0], false),
    );
    with_run(&run.state, |state| {
        state
            .async_observation
            .strict_clone
            .as_mut()
            .unwrap()
            .prepare_wait
            .as_mut()
            .unwrap()
            .checked_count = u64::MAX;
    });
    let before = run.async_observation_summary().strict_clone;
    observer.strict_async_snapshot(
        AsyncObservationPhase::PrepareWait,
        snapshot(2, [0, 8, 0], true),
    );
    observer.strict_async_snapshot(
        AsyncObservationPhase::ArmedVerify,
        snapshot(3, [0, 0, 0], false),
    );
    let after = run.async_observation_summary();
    assert_eq!(after.strict_clone, before);
    assert_eq!(after.failure, Some("async observation count overflow"));
    assert_eq!(after.evidence()["complete"], false);
}

#[test]
fn poison_marks_incomplete_without_replacing_prior_observations() {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    for tap_after_poison in [false, true] {
        let (run, observer) = run_and_clone();
        observer.strict_async_snapshot(
            AsyncObservationPhase::PrepareWait,
            snapshot(1, [0, 0, 0], false),
        );
        let before = run.async_observation_summary().strict_clone;
        assert!(
            catch_unwind(AssertUnwindSafe(
                || run.with_snapshot(|_| panic!("intentional async summary poison"))
            ))
            .is_err()
        );
        if tap_after_poison {
            observer.strict_async_snapshot(
                AsyncObservationPhase::ArmedVerify,
                snapshot(2, [0, 0, 0], false),
            );
        }
        let after = run.async_observation_summary();
        assert_eq!(after.strict_clone, before);
        assert_eq!(after.failure, Some("observer mutex poisoned"));
        assert_eq!(after.evidence()["complete"], false);
        let finished = run.finish();
        assert_eq!(finished.async_observation, after);
    }
}

#[test]
fn finish_and_drop_retire_held_taps_without_rebuilding_summary_ownership() {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    for poisoned in [false, true] {
        for finish in [false, true] {
            let (run, observer) = run_and_clone();
            observer.strict_async_snapshot(
                AsyncObservationPhase::PrepareWait,
                snapshot(1, [0, 1, 0], false),
            );
            let mut before = run.async_observation_summary();
            let held = Arc::clone(&run.state);
            if poisoned {
                assert!(
                    catch_unwind(AssertUnwindSafe(|| {
                        run.with_snapshot(|_| panic!("intentional retirement poison"));
                    }))
                    .is_err()
                );
                before.fail("observer mutex poisoned");
            }
            if finish {
                assert_eq!(run.finish().async_observation, before);
            } else {
                drop(run);
            }
            observer.strict_async_snapshot(
                AsyncObservationPhase::ArmedVerify,
                snapshot(2, [0, 0, 0], false),
            );
            let retired = held.lock().unwrap_or_else(|p| p.into_inner());
            assert!(retired.retired);
            assert_eq!(
                retired.async_observation,
                AsyncObservationSummary::default()
            );
            assert!(retired.records.is_empty());
            assert_eq!(retired.failure, None);
        }
    }
}
