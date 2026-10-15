//! Explicit default-class fresh qualification through the real backend and mock child.
//! Factory declarations never choose the actual class; no plug-in binary/device is used.
use super::backend_protocol_test_helper::{ResetHelper, kind, reset_config};
use super::*;

const DEFAULT_UID: &str = "00000000000000000000000000000001";

fn catalog_default_spec(helper: &ResetHelper) -> PluginLoadSpec {
    let mut spec = helper.spec();
    spec.descriptor.category = "Unknown".into();
    spec.descriptor.is_instrument = false;
    let mut catalog = crate::plugins::catalog_tests::catalog();
    catalog.classes[1].class_id = DEFAULT_UID.into();
    spec.descriptor.vst3_catalog = Some(catalog);
    let classes = &spec.descriptor.vst3_catalog.as_ref().unwrap().classes;
    assert!(classes.len() > 1 && classes[0].class_id != DEFAULT_UID);
    assert!(spec.descriptor.is_catalog_only() && !spec.descriptor.verified);
    assert!(spec.class_uid.is_none());
    spec
}

fn publication(control: &mut PluginChainControl) -> Option<RuntimeEvent> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        while let Some(event) = control.try_next_event() {
            if matches!(
                event,
                RuntimeEvent::SlotReady { .. } | RuntimeEvent::SlotFault { .. }
            ) {
                return Some(event);
            }
        }
        thread::sleep(Duration::from_millis(1));
    }
    None
}

fn retire(guard: &mut PluginWorkerGuard) -> RetainedShutdownPoll {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let state = guard.poll_shutdown_retained();
        if state != RetainedShutdownPoll::Pending {
            return state;
        }
        assert!(
            Instant::now() < deadline,
            "original fresh worker remains retained"
        );
        thread::sleep(Duration::from_millis(1));
    }
}

fn assert_load_and_no_execution(commands: &[serde_json::Value], requested: Option<&str>) {
    let loads = commands
        .iter()
        .filter(|command| kind(command) == "LoadPlugin")
        .collect::<Vec<_>>();
    assert_eq!(
        loads.len(),
        1,
        "qualification never retries or chooses another class"
    );
    assert_eq!(
        loads[0]["LoadPlugin"]["class_id"],
        serde_json::json!(requested)
    );
    assert!(!commands.iter().any(|command| matches!(
        kind(command),
        "ProbeFactory"
            | "Process"
            | "ProcessBuses"
            | "ProcessResetOrigin"
            | "SendMidi"
            | "SendMidiAt"
            | "MidiPanic"
            | "SetTransport"
            | "CreateView"
            | "ShowEditor"
            | "CreateGui"
            | "ShowGui"
    )));
    for command in commands.iter().filter(|command| kind(command) == "Editor") {
        assert!(
            matches!(
                command["Editor"]["command"].as_str(),
                Some("Query" | "Close")
            ),
            "only existing closed-editor housekeeping is permitted: {command}"
        );
    }
    assert_eq!(
        commands
            .iter()
            .filter(|command| kind(command) == "Shutdown")
            .count(),
        1
    );
    assert_eq!(commands.last().map(kind), Some("Shutdown"));
}

fn fresh_setup_ledger() -> Vec<&'static str> {
    vec![
        "LoadPlugin",
        "AudioBusLayout",
        "ResetOriginSupport",
        "LoadState",
        "CheckedAudioBusLayout",
        "EventBusLayout",
        "LatencySamples",
        "TailSamples",
        "AudioBusLayout",
        "CheckedAudioBusLayout",
        "EventBusLayout",
        "LatencySamples",
        "TailSamples",
        "StartProcessing",
        "NativeDirtyRevision",
        "CheckedAudioBusLayout",
        "EventBusLayout",
        "LatencySamples",
        "TailSamples",
    ]
}

#[test]
fn explicit_fresh_default_catalog_source_qualifies_actual_none_loaded_class_before_ready() {
    let helper = ResetHelper::new();
    let spec = catalog_default_spec(&helper);
    let path = spec.descriptor.path.clone();
    let gate = helper.hold_child_start();
    let mut chain =
        PluginChain::spawn_fresh_default_bounce_source(42, spec, reset_config()).unwrap();
    let child = gate.observe_and_release(&helper);
    let retained = chain.guard.requires_retained_fresh_shutdown();
    let event = publication(&mut chain.control);
    let end = retire(&mut chain.guard);
    assert_eq!(end, RetainedShutdownPoll::Joined);
    assert!(retained);
    child
        .expect("observe exact Python before first request")
        .assert_clean_shutdown();
    let Some(RuntimeEvent::SlotReady {
        slot: 0,
        latency_samples: 37,
        tail_samples: 511,
        loaded_vst3_identity: Some(identity),
        loaded_vst3_identity_error: None,
        ..
    }) = event
    else {
        panic!("complete actual fresh default qualification required: {event:?}");
    };
    assert_eq!(identity.path(), path);
    assert_eq!(identity.class_uid(), DEFAULT_UID);
    assert_eq!(identity.version(), "1");
    assert!(identity.requested_class_uid().is_none() && identity.is_instrument_with_midi_input());
    assert_eq!(chain.control.stats().faults, 0);
    assert_eq!(chain.control.stats().detached_workers, 0);
    assert_eq!(
        chain.control.plugin_latency_snapshot().unwrap().active_mask,
        1
    );
    while let Some(event) = chain.control.try_next_event() {
        assert!(
            !matches!(
                event,
                RuntimeEvent::SlotReady { .. } | RuntimeEvent::SlotFault { .. }
            ),
            "one candidate has one successful publication: {event:?}"
        );
    }
    let commands = helper.commands();
    let mut expected = fresh_setup_ledger();
    // Ready metadata, loaded identity and initial timing publication are real,
    // separate fences after the complete first post-Start metadata receipt.
    expected.extend([
        "LatencySamples",
        "TailSamples",
        "EventBusLayout",
        "LatencySamples",
        "TailSamples",
    ]);
    assert_eq!(
        commands[..expected.len()]
            .iter()
            .map(kind)
            .collect::<Vec<_>>(),
        expected
    );
    assert_load_and_no_execution(&commands, None);
    assert!(
        !commands
            .iter()
            .any(|command| kind(command) == "SetBusActive"),
        "the fully attested zero-input/stereo topology needs no fabricated activation"
    );
}

#[test]
fn explicit_fresh_default_prepare_is_one_shot_after_success_and_after_metadata_failure() {
    for fail_metadata in [false, true] {
        let helper = ResetHelper::new();
        if fail_metadata {
            helper.select("CheckedAudioBusLayout");
        }
        let mut source = Vst3Backend::load_fresh_default_bounce_source(
            catalog_default_spec(&helper),
            reset_config(),
        )
        .unwrap();
        let child = helper.observe_child();
        assert!(matches!(
            source.fresh_bounce_setup,
            Some(FreshBounceSourceSetup::Pending)
        ));
        let result = source.prepare(reset_config());
        if fail_metadata {
            assert!(result.is_err() && !source.prepared);
            assert!(matches!(
                source.fresh_bounce_setup,
                Some(FreshBounceSourceSetup::Consumed)
            ));
        } else {
            assert!(result.is_ok() && source.prepared);
            assert!(matches!(
                source.fresh_bounce_setup,
                Some(FreshBounceSourceSetup::Pinned {
                    latency_samples: 37,
                    tail_samples: 511,
                })
            ));
            let commands = helper.commands();
            assert_eq!(
                commands.iter().map(kind).collect::<Vec<_>>(),
                fresh_setup_ledger()
            );
        }
        let before = helper.commands();
        assert!(
            source
                .prepare(reset_config())
                .unwrap_err()
                .contains("one-shot")
        );
        assert_eq!(
            helper.commands(),
            before,
            "consumed admission cannot issue another setup call"
        );
        drop(source);
        child
            .expect("observe exact direct backend Python")
            .assert_clean_shutdown();
        assert_load_and_no_execution(&helper.commands(), None);
    }
}

#[test]
fn fresh_default_opt_in_preserves_exact_none_rejection_and_ordinary_none_load_contract() {
    let helper = ResetHelper::new();
    assert!(
        PluginChain::spawn_fresh_bounce_source(42, catalog_default_spec(&helper), reset_config())
            .is_err()
    );
    assert!(
        Vst3Backend::load_fresh_bounce_source(catalog_default_spec(&helper), reset_config())
            .is_err()
    );
    let mut explicit = catalog_default_spec(&helper);
    explicit.class_uid = Some(DEFAULT_UID.into());
    assert!(PluginChain::spawn_fresh_default_bounce_source(42, explicit, reset_config()).is_err());
    let mut explicit = catalog_default_spec(&helper);
    explicit.class_uid = Some(DEFAULT_UID.into());
    assert!(Vst3Backend::load_fresh_default_bounce_source(explicit, reset_config()).is_err());
    assert!(helper.commands().is_empty() && !helper.root.join("child.json").exists());
    let mut ordinary = Vst3Backend::load(catalog_default_spec(&helper), reset_config()).unwrap();
    let child = helper.observe_child();
    assert!(ordinary.fresh_bounce_setup.is_none());
    ordinary.prepare(reset_config()).unwrap();
    assert!(ordinary.fresh_bounce_setup.is_none());
    assert!(
        !helper
            .commands()
            .iter()
            .any(|command| matches!(kind(command), "CheckedAudioBusLayout" | "SetBusActive"))
    );
    drop(ordinary);
    child
        .expect("observe exact ordinary Python")
        .assert_clean_shutdown();
    assert_load_and_no_execution(&helper.commands(), None);
}

#[test]
fn fresh_exact_requested_class_mismatch_never_falls_back_to_actual_default() {
    let helper = ResetHelper::new();
    let mut spec = catalog_default_spec(&helper);
    let requested = spec.descriptor.vst3_catalog.as_ref().unwrap().classes[0]
        .class_id
        .clone();
    spec.class_uid = Some(requested.clone());
    let gate = helper.hold_child_start();
    let mut chain = PluginChain::spawn_fresh_bounce_source(42, spec, reset_config()).unwrap();
    let child = gate.observe_and_release(&helper);
    let event = publication(&mut chain.control);
    assert_eq!(retire(&mut chain.guard), RetainedShutdownPoll::Joined);
    child
        .expect("retain Python before exact mismatch disposal")
        .assert_clean_shutdown();
    let Some(RuntimeEvent::SlotFault { message, .. }) = event else {
        panic!("exact mismatch must fault: {event:?}");
    };
    assert!(message.contains("differs from the requested identity"));
    assert_eq!(chain.control.stats().detached_workers, 0);
    let commands = helper.commands();
    assert_load_and_no_execution(&commands, Some(&requested));
    assert!(!commands.iter().any(|command| matches!(
        kind(command),
        "LoadState" | "StartProcessing" | "CheckedAudioBusLayout"
    )));
}

#[test]
fn fresh_default_invalid_actual_identity_or_metadata_never_publishes_ready_or_retries() {
    for case in [
        "invalid-uid",
        "zero-uid",
        "empty-version",
        "effect",
        "no-midi",
        "CheckedAudioBusLayout",
        "post-start-tail",
    ] {
        let helper = ResetHelper::new();
        match case {
            "invalid-uid" => helper.set_identity("not-a-class-id", "1"),
            "zero-uid" => helper.set_identity("00000000000000000000000000000000", "1"),
            "empty-version" => helper.set_identity(DEFAULT_UID, ""),
            "effect" => helper.set_identity_traits("Fx", true, false),
            "no-midi" => helper.set_identity_traits("Instrument", false, false),
            _ => helper.select(case),
        }
        let gate = helper.hold_child_start();
        let mut chain = PluginChain::spawn_fresh_default_bounce_source(
            42,
            catalog_default_spec(&helper),
            reset_config(),
        )
        .unwrap();
        let child = gate.observe_and_release(&helper);
        let retained = chain.guard.requires_retained_fresh_shutdown();
        let event = publication(&mut chain.control);
        assert_eq!(retire(&mut chain.guard), RetainedShutdownPoll::Joined);
        assert!(retained);
        child
            .expect("retain Python before failed default qualification disposal")
            .assert_clean_shutdown();
        assert!(
            matches!(event, Some(RuntimeEvent::SlotFault { .. })),
            "{case}: {event:?}"
        );
        while let Some(event) = chain.control.try_next_event() {
            assert!(
                !matches!(event, RuntimeEvent::SlotReady { .. }),
                "{case}: {event:?}"
            );
        }
        assert!(chain.control.stats().faults > 0);
        assert_eq!(chain.control.stats().detached_workers, 0);
        assert_eq!(
            chain.control.plugin_latency_snapshot().unwrap().active_mask,
            0
        );
        let commands = helper.commands();
        assert_load_and_no_execution(&commands, None);
        let started = commands
            .iter()
            .any(|command| kind(command) == "StartProcessing");
        assert_eq!(
            started,
            case == "post-start-tail",
            "failed preflight cannot start: {case}"
        );
        if case == "post-start-tail" {
            let expected = fresh_setup_ledger();
            assert_eq!(
                commands[..expected.len()]
                    .iter()
                    .map(kind)
                    .collect::<Vec<_>>(),
                expected
            );
        }
    }
}
