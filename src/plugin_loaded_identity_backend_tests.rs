//! Cross-platform identity through the actual VST3 backend and child protocol.
use super::backend_protocol_test_helper::{ResetHelper, kind, reset_config};
use super::*;

fn ordinary_catalog_spec(helper: &ResetHelper) -> PluginLoadSpec {
    let mut spec = helper.spec();
    spec.descriptor.category = "Unknown".into();
    spec.descriptor.is_instrument = false;
    spec.descriptor.vst3_catalog = Some(crate::plugins::catalog_tests::catalog());
    assert!(spec.descriptor.is_catalog_only());
    assert!(spec.class_uid.is_none());
    spec
}

fn assert_default_startup_ledger(commands: &[serde_json::Value]) {
    let first_event_layout = commands
        .iter()
        .position(|command| kind(command) == "EventBusLayout")
        .expect("actual post-prepare event bus request");
    assert_eq!(
        commands[..=first_event_layout]
            .iter()
            .map(kind)
            .collect::<Vec<_>>(),
        [
            "LoadPlugin",
            "AudioBusLayout",
            "ResetOriginSupport",
            "LoadState",
            "StartProcessing",
            "NativeDirtyRevision",
            "LatencySamples",
            "TailSamples",
            "EventBusLayout",
        ]
    );
    let loads = commands
        .iter()
        .filter(|command| kind(command) == "LoadPlugin")
        .collect::<Vec<_>>();
    assert_eq!(
        loads.len(),
        1,
        "identity qualification never retries or picks another class"
    );
    assert!(loads[0]["LoadPlugin"]["class_id"].is_null());
    assert_eq!(
        commands
            .iter()
            .filter(|command| kind(command) == "EventBusLayout")
            .count(),
        1
    );
    assert_eq!(
        commands
            .iter()
            .filter(|command| kind(command) == "Shutdown")
            .count(),
        1
    );
    assert_eq!(commands.last().map(kind), Some("Shutdown"));
    assert!(!commands.iter().any(|command| matches!(
        kind(command),
        "ProbeFactory" | "Process" | "SendMidi" | "Midi" | "SetTransport" | "ProcessResetOrigin"
    )));
}

#[test]
fn actual_default_ready_identity_uses_loaded_uid_version_and_current_event_bus_not_name_or_catalog()
{
    for active in [false, true] {
        let helper = ResetHelper::new();
        if !active {
            helper.select("inactive-event");
        }
        let spec = ordinary_catalog_spec(&helper);
        let path = spec.descriptor.path.clone();
        let descriptor_id = spec.descriptor.id.clone();
        let mut chain = PluginChain::spawn_identified(vec![(42, spec)], reset_config()).unwrap();
        let mut ready = None;
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while ready.is_none() && std::time::Instant::now() < deadline {
            if let Some(RuntimeEvent::SlotReady {
                loaded_vst3_identity,
                ..
            }) = chain.control.try_next_event()
            {
                ready = loaded_vst3_identity;
            }
            thread::sleep(Duration::from_millis(1));
        }
        let child = helper.observe_child();
        let joined = chain.guard.shutdown_blocking(Duration::from_secs(5));
        assert_eq!(joined, ShutdownOutcome::Joined);
        child
            .expect("observe exact child before shutdown")
            .assert_clean_shutdown();
        let identity = ready.expect("actual prepared default class identity required");
        assert_eq!(identity.path(), path);
        assert!(identity.requested_class_uid().is_none());
        assert_eq!(identity.class_uid(), "00000000000000000000000000000001");
        assert_ne!(identity.class_uid(), descriptor_id);
        assert_eq!(identity.version(), "1");
        assert_eq!(identity.category(), "Instrument");
        assert_eq!(identity.is_instrument_with_midi_input(), active);
        let commands = helper.commands();
        assert_default_startup_ledger(&commands);
        let loads = commands
            .iter()
            .filter(|value| kind(value) == "LoadPlugin")
            .collect::<Vec<_>>();
        assert_eq!(loads.len(), 1);
        assert!(loads[0]["LoadPlugin"]["class_id"].is_null());
        let position = |name| {
            commands
                .iter()
                .position(|value| kind(value) == name)
                .unwrap()
        };
        assert!(position("StartProcessing") < position("EventBusLayout"));
        assert!(position("LatencySamples") < position("EventBusLayout"));
        assert!(position("TailSamples") < position("EventBusLayout"));
    }
}

#[test]
fn actual_ready_identity_bus_failure_keeps_legacy_ready_but_refuses_new_qualification_without_retry()
 {
    let helper = ResetHelper::new();
    helper.select("EventBusLayout");
    let mut chain =
        PluginChain::spawn_identified(vec![(42, ordinary_catalog_spec(&helper))], reset_config())
            .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut ready = None;
    let mut failure = None;
    while ready.is_none() && failure.is_none() && std::time::Instant::now() < deadline {
        match chain.control.try_next_event() {
            Some(RuntimeEvent::SlotReady {
                loaded_vst3_identity,
                loaded_vst3_identity_error,
                ..
            }) => {
                ready = Some((loaded_vst3_identity, loaded_vst3_identity_error));
            }
            Some(RuntimeEvent::SlotFault { message, .. }) => failure = Some(message),
            _ => (),
        }
        thread::sleep(Duration::from_millis(1));
    }
    let child = helper.observe_child();
    assert_eq!(
        chain.guard.shutdown_blocking(Duration::from_secs(5)),
        ShutdownOutcome::Joined
    );
    child
        .expect("observe exact child before shutdown")
        .assert_clean_shutdown();
    assert!(failure.is_none());
    let (identity, reason) = ready.expect("ordinary VST3 still becomes ready");
    assert!(identity.is_none());
    assert!(
        reason
            .unwrap()
            .contains("Prepared VST3 event identity unavailable")
    );
    assert_eq!(chain.control.stats().faults, 0);
    assert_default_startup_ledger(&helper.commands());
    assert_eq!(
        helper
            .commands()
            .iter()
            .filter(|command| kind(command) == "LoadPlugin")
            .count(),
        1
    );
}

#[test]
fn actual_ready_identity_validation_failure_keeps_ordinary_default_load_and_ready_usable() {
    for (uid, version) in [
        ("not-a-class-id", "1"),
        ("00000000000000000000000000000001", ""),
    ] {
        let helper = ResetHelper::new();
        helper.set_identity(uid, version);
        let mut chain = PluginChain::spawn_identified(
            vec![(42, ordinary_catalog_spec(&helper))],
            reset_config(),
        )
        .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut ready = None;
        while ready.is_none() && std::time::Instant::now() < deadline {
            if let Some(event) = chain.control.try_next_event() {
                ready = Some(event);
            }
            thread::sleep(Duration::from_millis(1));
        }
        let faults = chain.control.stats().faults;
        let child = helper.observe_child();
        assert_eq!(
            chain.guard.shutdown_blocking(Duration::from_secs(5)),
            ShutdownOutcome::Joined
        );
        child
            .expect("observe exact child before shutdown")
            .assert_clean_shutdown();
        let Some(RuntimeEvent::SlotReady {
            loaded_vst3_identity: None,
            loaded_vst3_identity_error: Some(reason),
            ..
        }) = ready
        else {
            panic!("ordinary default load must remain Ready: {ready:?}");
        };
        assert!(reason.contains("actual class UID and version"));
        assert_eq!(faults, 0);
        let commands = helper.commands();
        assert_default_startup_ledger(&commands);
        let loads = commands
            .iter()
            .filter(|command| kind(command) == "LoadPlugin")
            .collect::<Vec<_>>();
        assert_eq!(loads.len(), 1);
        assert!(loads[0]["LoadPlugin"]["class_id"].is_null());
    }
}
