//! Actual child/pipe protocol regressions for the ordinary default-class API.
//! No plugin SDK, native editor, hardware audio or Kontakt is involved.
use super::*;
use crate::discovery::{FactoryClassMetadata, FactoryInfo, FactoryMetadata};
use crate::fresh_helper::tests::{arm_probe_setup_failure, arm_probe_wait_failure, python, serial};
use crate::process_isolation::{HostCommand, HostResponse};

fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(3)
}
fn probe(script: &str) -> Probe {
    match Probe::spawn(&mut python(script), deadline()) {
        Ok(probe) => probe,
        Err(mut failure) => {
            if let Some(owner) = failure.cleanup.as_mut() {
                let _ = owner.dispose(Instant::now(), deadline());
            }
            panic!("actual mock probe spawn failed: {:?}", failure.failure);
        }
    }
}
fn info() -> HostResponse {
    HostResponse::PluginInfo {
        vendor: "Fixture vendor".into(),
        name: "Only the default fixture class".into(),
        version: "1.2.3".into(),
        category: "Fx|Fixture".into(),
        uid: "ABCDEF019182FAEB566D624153465854".into(),
        has_gui: true,
        audio_inputs: 3,
        audio_outputs: 32,
        output_channels: 64,
        has_midi_input: false,
        has_midi_output: true,
        compatibility: Vec::new(),
    }
}
fn default_load() -> HostCommand {
    HostCommand::LoadPlugin {
        path: "fixture-default.vst3".into(),
        sample_rate: 48_000.0,
        block_size: 128,
        tempo: 120.0,
        time_sig_numerator: 4,
        time_sig_denominator: 4,
        class_id: None,
    }
}
fn ledger(reply: HostResponse, ending: &str) -> String {
    let expected = serde_json::to_string(&default_load()).unwrap();
    let response = serde_json::to_string(&reply).unwrap();
    format!(
        "import json,sys,time\nassert json.loads(sys.stdin.readline())==json.loads({expected:?})\nprint({response:?},flush=True)\n{ending}\n"
    )
}
fn clean_finish(probe: &mut Probe, protocol_deadline: Instant) {
    let result = probe.dispose(protocol_deadline, deadline());
    assert!(matches!(result.state, CleanupState::Reaped(status) if status.success()));
    assert!(!result.kill_requested);
    assert!(result.shutdown_failure.is_none());
    assert!(result.kill_error.is_none());
    assert!(result.wait_error.is_none());
    assert!(result.pipe_error.is_none());
}
fn failure(result: Result<LoadedPlugin, Failure>) -> Failure {
    match result {
        Err(failure) => failure,
        Ok(_) => panic!("expected an actual failed default probe exchange"),
    }
}

fn factory_metadata() -> FactoryMetadata {
    let class = FactoryClassMetadata {
        class_id: "0123456789ABCDEF0123456789ABCDEF".into(),
        name: "Declared instrument".into(),
        category: "Audio Module Class".into(),
        cardinality: i32::MAX,
        vendor: None,
        version: None,
        sdk_version: None,
        sub_categories: None,
        class_flags: None,
    };
    let mut controller = class.clone();
    controller.class_id = "FEDCBA9876543210FEDCBA9876543210".into();
    controller.name = "Declared controller".into();
    controller.category = "Component Controller Class".into();
    controller.vendor = Some(String::new());
    controller.sub_categories = Some(Vec::new());
    controller.class_flags = Some(0);
    FactoryMetadata {
        path: "fixture-factory.vst3".into(),
        factory: FactoryInfo {
            vendor: "Factory only".into(),
            url: String::new(),
            email: String::new(),
            flags: -1,
        },
        classes: vec![class, controller],
    }
}
fn factory_ledger(reply: HostResponse, ending: &str) -> String {
    let expected = serde_json::to_string(&HostCommand::ProbeFactory {
        path: "fixture-factory.vst3".into(),
    })
    .unwrap();
    let response = serde_json::to_string(&reply).unwrap();
    format!(
        "import json,sys,time\nassert json.loads(sys.stdin.readline())==json.loads({expected:?})\nprint({response:?},flush=True)\n{ending}\n"
    )
}
fn factory_failure(result: Result<FactoryMetadata, Failure>) -> Failure {
    match result {
        Err(failure) => failure,
        Ok(_) => panic!("expected a failed factory-only exchange"),
    }
}

// Keep phase evidence off the protocol pipe. Spawn returning only proves that
// the child and owned pipes exist, not that Python has entered this script.
fn attempt_seal_ledger(request: HostCommand, reply: HostResponse, ending: &str) -> String {
    let expected = serde_json::to_string(&request).unwrap();
    let response = serde_json::to_string(&reply).unwrap();
    let operation = match request {
        HostCommand::ProbeFactory { .. } => "factory",
        HostCommand::LoadPlugin { .. } => "default-load",
        _ => panic!("attempt-seal fixture only permits its two original requests"),
    };
    format!(
        "import sys,time\n_trace_started=time.monotonic()\ndef _trace(stage): print('attempt-seal {operation} '+stage+' child_elapsed='+str(time.monotonic()-_trace_started),file=sys.stderr,flush=True)\n_trace('script-entered')\nimport json\n_trace('json-ready')\n_request=sys.stdin.readline()\n_trace('request-read')\nassert json.loads(_request)==json.loads({expected:?})\n_trace('request-validated')\nprint({response:?},flush=True)\n_trace('reply-flushed')\n{ending}\n"
    )
}

fn attempt_seal_exchange<T>(
    result: Result<T, Failure>,
    probe: &mut Probe,
    operation: &str,
    spawn_elapsed: Duration,
    exchange_started: Instant,
    exchange_deadline: Instant,
) -> T {
    match result {
        Ok(value) => value,
        Err(failure) => {
            let pid = probe.helper_pid();
            let exchange_elapsed = exchange_started.elapsed();
            // Preserve the original failure, deadline and child owner. This is
            // checked disposal, never a second exchange or a fresh child.
            let cleanup = probe.dispose(exchange_deadline, deadline());
            panic!(
                "attempt-seal {operation} failed: pid={pid:?}, spawn_elapsed={spawn_elapsed:?}, exchange_elapsed={exchange_elapsed:?}, original_budget={:?}, failure={failure:?}, cleanup={cleanup:?}",
                exchange_deadline.saturating_duration_since(exchange_started),
            );
        }
    }
}

#[test]
fn factory_probe_sends_only_factory_request_preserves_unknowns_and_reaps_shutdown() {
    let _serial = serial();
    let expected = factory_metadata();
    let mut probe = probe(&factory_ledger(
        HostResponse::FactoryMetadata {
            metadata: expected.clone(),
        },
        "assert json.loads(sys.stdin.readline())=='Shutdown'\nassert sys.stdin.read()==''",
    ));
    let actual_pid = probe.helper_pid().unwrap();
    let exchange_deadline = deadline();
    let actual = probe
        .probe_factory("fixture-factory.vst3", exchange_deadline)
        .unwrap();
    assert_eq!(actual, expected);
    assert_eq!(actual.classes[0].vendor, None);
    assert_eq!(actual.classes[1].vendor.as_deref(), Some(""));
    assert_eq!(actual.classes[1].sub_categories, Some(Vec::new()));
    assert_eq!(probe.helper_pid(), Some(actual_pid));
    clean_finish(&mut probe, exchange_deadline);
    assert_eq!(probe.helper_pid(), None);
    assert!(
        matches!(probe.continue_cleanup().unwrap().state, CleanupState::Reaped(status) if status.success())
    );
}

#[test]
fn factory_and_default_load_share_one_attempt_seal_in_both_directions() {
    let _serial = serial();
    let ending = "assert json.loads(sys.stdin.readline())=='Shutdown'\nassert sys.stdin.read()==''";
    let factory_script = attempt_seal_ledger(
        HostCommand::ProbeFactory {
            path: "fixture-factory.vst3".into(),
        },
        HostResponse::FactoryMetadata {
            metadata: factory_metadata(),
        },
        ending,
    );
    let spawn_started = Instant::now();
    let mut factory = probe(&factory_script);
    let spawn_elapsed = spawn_started.elapsed();
    let exchange_started = Instant::now();
    let exchange_deadline = deadline();
    let first = factory.probe_factory("fixture-factory.vst3", exchange_deadline);
    attempt_seal_exchange(
        first,
        &mut factory,
        "factory",
        spawn_elapsed,
        exchange_started,
        exchange_deadline,
    );
    for rejected in [
        factory_failure(factory.probe_factory("never-probed.vst3", deadline())),
        failure(factory.load_default_class("never-loaded.vst3", deadline())),
    ] {
        assert_eq!(rejected.kind, FailureKind::Terminal);
        assert_eq!(rejected.delivery, Delivery::NotSent);
        assert!(rejected.request_context.is_none());
    }
    clean_finish(&mut factory, exchange_deadline);
    let loaded_script = attempt_seal_ledger(default_load(), info(), ending);
    let spawn_started = Instant::now();
    let mut loaded = probe(&loaded_script);
    let spawn_elapsed = spawn_started.elapsed();
    let exchange_started = Instant::now();
    let exchange_deadline = deadline();
    let first = loaded.load_default_class("fixture-default.vst3", exchange_deadline);
    attempt_seal_exchange(
        first,
        &mut loaded,
        "default-load",
        spawn_elapsed,
        exchange_started,
        exchange_deadline,
    );
    let rejected = factory_failure(loaded.probe_factory("never-probed.vst3", deadline()));
    assert_eq!(rejected.kind, FailureKind::Terminal);
    assert_eq!(rejected.delivery, Delivery::NotSent);
    clean_finish(&mut loaded, exchange_deadline);
}

#[test]
fn factory_probe_reported_error_and_crash_are_typed_not_retried_or_reloaded() {
    let _serial = serial();
    for (reply, kind, message) in [
        (
            HostResponse::Error {
                message: "factory getter failed".into(),
            },
            FailureKind::OperationRejected,
            "factory getter failed",
        ),
        (
            HostResponse::Crashed {
                message: "factory-only crash".into(),
            },
            FailureKind::ReportedCrash,
            "factory-only crash",
        ),
    ] {
        let mut probe = probe(&factory_ledger(reply, "assert sys.stdin.read()==''"));
        let actual_pid = probe.helper_pid().unwrap();
        let error = factory_failure(probe.probe_factory("fixture-factory.vst3", deadline()));
        assert_eq!(error.kind, kind);
        assert_eq!(error.delivery, Delivery::MayHaveApplied);
        assert_eq!(error.diagnostic.as_deref(), Some(message));
        assert_eq!(error.request_context.unwrap().operation, "ProbeFactory");
        assert_eq!(
            failure(probe.load_default_class("never-loaded.vst3", deadline())).kind,
            FailureKind::Terminal
        );
        assert_eq!(probe.helper_pid(), Some(actual_pid));
        clean_finish(&mut probe, Instant::now());
    }
}

#[test]
fn factory_probe_rejects_wrong_shape_and_wrong_requested_path() {
    let _serial = serial();
    let mut wrong_path = factory_metadata();
    wrong_path.path = "another-module.vst3".into();
    for reply in [
        info(),
        HostResponse::FactoryMetadata {
            metadata: wrong_path,
        },
    ] {
        let mut probe = probe(&factory_ledger(reply, "assert sys.stdin.read()==''"));
        let error = factory_failure(probe.probe_factory("fixture-factory.vst3", deadline()));
        assert_eq!(error.kind, FailureKind::WrongReply);
        assert_eq!(error.delivery, Delivery::MayHaveApplied);
        assert_eq!(error.request_context.unwrap().operation, "ProbeFactory");
        assert!(error.diagnostic.is_none());
        assert_eq!(
            factory_failure(probe.probe_factory("fixture-factory.vst3", deadline())).kind,
            FailureKind::Terminal
        );
        clean_finish(&mut probe, Instant::now());
    }
}

#[test]
fn factory_probe_expired_or_precancelled_attempt_delivers_no_bytes_and_cannot_load() {
    let _serial = serial();
    for cancel in [false, true] {
        let mut probe = probe("import sys\nassert sys.stdin.read()==''\n");
        let actual_pid = probe.helper_pid().unwrap();
        if cancel {
            probe.cancellation().cancel();
        }
        let error = factory_failure(probe.probe_factory(
            "fixture-factory.vst3",
            if cancel { deadline() } else { Instant::now() },
        ));
        assert_eq!(
            error.kind,
            if cancel {
                FailureKind::Cancelled
            } else {
                FailureKind::Deadline
            }
        );
        assert_eq!(error.delivery, Delivery::NotSent);
        assert_eq!(error.request_context.unwrap().operation, "ProbeFactory");
        assert_eq!(
            failure(probe.load_default_class("never-loaded.vst3", deadline())).kind,
            FailureKind::Terminal
        );
        assert_eq!(probe.helper_pid(), Some(actual_pid));
        clean_finish(&mut probe, Instant::now());
    }
}

#[test]
fn factory_probe_timeout_retains_same_owner_until_forced_reap() {
    let _serial = serial();
    let mut probe = probe("import sys,time\nsys.stdin.readline()\ntime.sleep(30)\n");
    let actual_pid = probe.helper_pid().unwrap();
    let exchange_deadline = Instant::now() + Duration::from_millis(120);
    let error = factory_failure(probe.probe_factory("fixture-factory.vst3", exchange_deadline));
    assert_eq!(error.kind, FailureKind::Deadline);
    assert_eq!(error.delivery, Delivery::MayHaveApplied);
    assert_eq!(error.request_context.unwrap().operation, "ProbeFactory");
    assert_eq!(probe.helper_pid(), Some(actual_pid));
    let result = probe.dispose(exchange_deadline, deadline());
    assert!(matches!(result.state, CleanupState::Reaped(_)));
    assert!(result.kill_requested);
    assert_eq!(probe.helper_pid(), None);
}

#[test]
fn factory_probe_cross_thread_cancel_wakes_same_inflight_owner_without_load_retry() {
    let _serial = serial();
    let mut probe = probe("import sys,time\nsys.stdin.readline()\ntime.sleep(30)\n");
    let cancel = probe.cancellation();
    let join = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        cancel.cancel();
    });
    let error = factory_failure(probe.probe_factory("fixture-factory.vst3", deadline()));
    join.join().unwrap();
    assert_eq!(error.kind, FailureKind::Cancelled);
    assert_eq!(error.delivery, Delivery::MayHaveApplied);
    assert_eq!(error.request_context.unwrap().operation, "ProbeFactory");
    assert_eq!(
        failure(probe.load_default_class("never-loaded.vst3", deadline())).kind,
        FailureKind::Terminal
    );
    let result = probe.dispose(Instant::now(), deadline());
    assert!(matches!(result.state, CleanupState::Reaped(_)));
    assert!(result.kill_requested);
}

#[test]
fn factory_probe_oversized_request_is_unsent_but_consumes_single_attempt() {
    let _serial = serial();
    let mut probe = probe("import sys\nassert sys.stdin.read()==''\n");
    let error = factory_failure(probe.probe_factory(&"x".repeat(64 * 1024 + 1), deadline()));
    assert_eq!(error.kind, FailureKind::Limit);
    assert_eq!(error.delivery, Delivery::NotSent);
    assert_eq!(error.request_context.unwrap().operation, "ProbeFactory");
    assert_eq!(
        failure(probe.load_default_class("never-loaded.vst3", deadline())).kind,
        FailureKind::Terminal
    );
    clean_finish(&mut probe, Instant::now());
}

#[test]
fn factory_declarations_cannot_substitute_failed_healthy_retirement() {
    let _serial = serial();
    let mut probe = probe(&factory_ledger(
        HostResponse::FactoryMetadata { metadata: factory_metadata() },
        "assert json.loads(sys.stdin.readline())=='Shutdown'\nassert sys.stdin.read()==''\ntime.sleep(30)",
    ));
    let exchange_deadline = deadline();
    assert_eq!(
        probe
            .probe_factory("fixture-factory.vst3", exchange_deadline)
            .unwrap(),
        factory_metadata()
    );
    let result = probe.dispose(exchange_deadline, Instant::now() + Duration::from_secs(1));
    assert!(matches!(result.state, CleanupState::Reaped(_)));
    assert!(result.kill_requested);
}

#[test]
fn default_probe_sends_original_configuration_and_reaps_actual_shutdown() {
    let _serial = serial();
    let mut probe = probe(&ledger(
        info(),
        "assert json.loads(sys.stdin.readline())=='Shutdown'\nassert sys.stdin.read()==''",
    ));
    let actual_pid = probe.helper_pid().unwrap();
    assert_ne!(actual_pid, 0);
    let exchange_deadline = deadline();
    let metadata = probe
        .load_default_class("fixture-default.vst3", exchange_deadline)
        .unwrap();
    assert_eq!(metadata.vendor, "Fixture vendor");
    assert_eq!(metadata.name, "Only the default fixture class");
    assert_eq!(metadata.version, "1.2.3");
    assert_eq!(metadata.category, "Fx|Fixture");
    assert_eq!(metadata.uid, "ABCDEF019182FAEB566D624153465854");
    assert!(metadata.has_gui);
    assert_eq!(metadata.audio_inputs, 3);
    assert_eq!(metadata.audio_outputs, 32);
    assert_eq!(metadata.output_channels, 64);
    assert!(!metadata.has_midi_input);
    assert!(metadata.has_midi_output);
    assert!(metadata.compatibility.is_empty());
    assert_eq!(probe.helper_pid(), Some(actual_pid));
    clean_finish(&mut probe, exchange_deadline);
    assert_eq!(probe.helper_pid(), None);
    let repeated = probe.continue_cleanup().unwrap();
    assert!(matches!(repeated.state, CleanupState::Reaped(status) if status.success()));
    assert!(!repeated.kill_requested);
}

#[test]
fn default_probe_rejects_a_second_load_without_request_or_respawn() {
    let _serial = serial();
    let mut probe = probe(&ledger(
        info(),
        "assert json.loads(sys.stdin.readline())=='Shutdown'\nassert sys.stdin.read()==''",
    ));
    let actual_pid = probe.helper_pid().unwrap();
    let exchange_deadline = deadline();
    probe
        .load_default_class("fixture-default.vst3", exchange_deadline)
        .unwrap();
    let second = failure(probe.load_default_class("never-loaded.vst3", exchange_deadline));
    assert_eq!(second.kind, FailureKind::Terminal);
    assert_eq!(second.delivery, Delivery::NotSent);
    assert!(second.request_context.is_none());
    assert_eq!(probe.helper_pid(), Some(actual_pid));
    clean_finish(&mut probe, exchange_deadline);
}

#[test]
fn default_probe_reported_crash_is_not_retried_and_retains_original_error() {
    let _serial = serial();
    let mut probe = probe(&ledger(
        HostResponse::Crashed {
            message: "actual fixture LoadPlugin crash".into(),
        },
        "assert sys.stdin.read()==''",
    ));
    let actual_pid = probe.helper_pid().unwrap();
    let first = failure(probe.load_default_class("fixture-default.vst3", deadline()));
    assert_eq!(first.kind, FailureKind::ReportedCrash);
    assert_eq!(first.delivery, Delivery::MayHaveApplied);
    assert_eq!(
        first.diagnostic.as_deref(),
        Some("actual fixture LoadPlugin crash")
    );
    assert_eq!(first.request_context.unwrap().operation, "LoadPlugin");
    let second = failure(probe.load_default_class("fixture-default.vst3", deadline()));
    assert_eq!(second.kind, FailureKind::Terminal);
    assert_eq!(second.delivery, Delivery::NotSent);
    assert_eq!(probe.helper_pid(), Some(actual_pid));
    clean_finish(&mut probe, Instant::now());
}

#[test]
fn default_probe_timeout_requires_same_owner_reap_and_stays_failed() {
    let _serial = serial();
    let mut probe = probe("import sys,time\nsys.stdin.readline()\ntime.sleep(30)\n");
    let actual_pid = probe.helper_pid().unwrap();
    let exchange_deadline = Instant::now() + Duration::from_millis(120);
    let error = failure(probe.load_default_class("fixture-default.vst3", exchange_deadline));
    assert_eq!(error.kind, FailureKind::Deadline);
    assert_eq!(error.delivery, Delivery::MayHaveApplied);
    assert_eq!(error.request_context.unwrap().operation, "LoadPlugin");
    assert_eq!(probe.helper_pid(), Some(actual_pid));
    let result = probe.dispose(exchange_deadline, deadline());
    assert!(matches!(result.state, CleanupState::Reaped(_)));
    assert!(result.kill_requested);
    assert_eq!(probe.helper_pid(), None);
    let second = failure(probe.load_default_class("fixture-default.vst3", deadline()));
    assert_eq!(second.kind, FailureKind::Terminal);
    assert_eq!(second.delivery, Delivery::NotSent);
}

#[test]
fn default_probe_pre_cancel_delivers_no_load_bytes_and_reaps_same_child() {
    let _serial = serial();
    let mut probe = probe("import sys\nassert sys.stdin.read()==''\n");
    let actual_pid = probe.helper_pid().unwrap();
    probe.cancellation().cancel();
    let error = failure(probe.load_default_class("fixture-default.vst3", deadline()));
    assert_eq!(error.kind, FailureKind::Cancelled);
    assert_eq!(error.delivery, Delivery::NotSent);
    assert_eq!(probe.helper_pid(), Some(actual_pid));
    clean_finish(&mut probe, Instant::now());
}

#[test]
fn default_probe_cross_thread_cancel_wakes_same_inflight_owner() {
    let _serial = serial();
    let mut probe = probe("import sys,time\nsys.stdin.readline()\ntime.sleep(30)\n");
    let actual_pid = probe.helper_pid().unwrap();
    let cancel = probe.cancellation();
    let join = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        cancel.cancel();
    });
    let error = failure(probe.load_default_class("fixture-default.vst3", deadline()));
    join.join().unwrap();
    assert_eq!(error.kind, FailureKind::Cancelled);
    assert_eq!(error.delivery, Delivery::MayHaveApplied);
    assert_eq!(probe.helper_pid(), Some(actual_pid));
    let result = probe.dispose(Instant::now(), deadline());
    assert!(matches!(result.state, CleanupState::Reaped(_)));
    assert!(result.kill_requested);
}

#[test]
fn default_probe_failed_spawn_keeps_real_cleanup_owner_and_blocks_another_child() {
    let _serial = serial();
    arm_probe_setup_failure();
    let mut failed = match Probe::spawn(
        &mut python("import sys\nassert sys.stdin.read()==''\n"),
        deadline(),
    ) {
        Ok(_) => panic!("actual post-spawn setup fault unexpectedly succeeded"),
        Err(failure) => failure,
    };
    assert_eq!(failed.failure.kind, FailureKind::Io(libc::EIO));
    assert_eq!(failed.failure.delivery, Delivery::NotSent);
    let owner = failed.cleanup.as_mut().expect("actual child remains owned");
    assert_ne!(owner.helper_pid().unwrap(), 0);
    let blocked = match Probe::spawn(
        &mut Command::new("must-not-spawn-while-reserved"),
        deadline(),
    ) {
        Ok(_) => panic!("another child escaped the occupied reservation"),
        Err(failure) => failure,
    };
    assert_eq!(blocked.failure.kind, FailureKind::Busy);
    assert_eq!(blocked.failure.delivery, Delivery::NotSent);
    assert!(blocked.cleanup.is_none());
    clean_finish(owner, Instant::now());
}

#[test]
fn default_probe_wait_error_remains_after_actual_reap_and_pending_blocks_spawn() {
    let _serial = serial();
    let mut probe = probe("import sys\nassert sys.stdin.read()==''\n");
    arm_probe_wait_failure();
    let first = probe.dispose(Instant::now(), deadline());
    assert!(matches!(first.state, CleanupState::Pending));
    assert_eq!(first.wait_error, Some(libc::EIO));
    assert_eq!(
        first.shutdown_failure.as_ref().unwrap().kind,
        FailureKind::Deadline
    );
    let blocked = match Probe::spawn(
        &mut Command::new("must-not-spawn-while-pending"),
        deadline(),
    ) {
        Ok(_) => panic!("another child escaped pending cleanup"),
        Err(failure) => failure,
    };
    assert_eq!(blocked.failure.kind, FailureKind::Busy);
    let cleanup_deadline = deadline();
    let joined = loop {
        let observed = probe.continue_cleanup().unwrap();
        if matches!(observed.state, CleanupState::Reaped(_)) {
            break observed;
        }
        assert!(
            Instant::now() < cleanup_deadline,
            "same child still pending: {observed:?}"
        );
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(joined.wait_error, first.wait_error);
    assert_eq!(
        joined.shutdown_failure.as_ref().unwrap().kind,
        FailureKind::Deadline
    );
    assert_eq!(
        joined.shutdown_failure.as_ref().unwrap().delivery,
        Delivery::NotSent
    );
    assert!(matches!(joined.state, CleanupState::Reaped(_)));
}

#[test]
fn default_probe_drop_quarantines_same_child_without_claiming_reap() {
    let _serial = serial();
    let probe = probe("import sys\nassert sys.stdin.read()==''\n");
    let actual_pid = probe.helper_pid().unwrap();
    drop(probe);
    let blocked = match Probe::spawn(
        &mut Command::new("must-not-spawn-while-abandoned"),
        deadline(),
    ) {
        Ok(_) => panic!("abandoned child lost its reservation"),
        Err(failure) => failure,
    };
    assert_eq!(blocked.failure.kind, FailureKind::Busy);
    assert_ne!(actual_pid, 0);
    let result = Probe::cleanup_abandoned(deadline());
    assert!(matches!(result.state, CleanupState::Reaped(_)));
    assert!(matches!(
        Probe::cleanup_abandoned(Instant::now()).state,
        CleanupState::Empty
    ));
}

#[test]
fn default_probe_metadata_cannot_substitute_a_failed_healthy_disposal() {
    let _serial = serial();
    let mut probe = probe(&ledger(
        info(),
        "assert json.loads(sys.stdin.readline())=='Shutdown'\nassert sys.stdin.read()==''\ntime.sleep(30)",
    ));
    let exchange_deadline = deadline();
    let metadata = probe
        .load_default_class("fixture-default.vst3", exchange_deadline)
        .unwrap();
    assert_eq!(metadata.uid, "ABCDEF019182FAEB566D624153465854");
    let cleanup_deadline = Instant::now() + Duration::from_secs(1);
    let result = probe.dispose(exchange_deadline, cleanup_deadline);
    assert!(matches!(result.state, CleanupState::Reaped(_)));
    assert!(result.kill_requested);
    // Metadata still proves only the actual returned class, while this
    // cleanup receipt explicitly prevents a healthy natural-exit claim.
}

#[test]
fn default_probe_expired_exchange_never_delivers_load_or_extends_deadline() {
    let _serial = serial();
    let mut probe = probe("import sys\nassert sys.stdin.read()==''\n");
    let error = failure(probe.load_default_class("fixture-default.vst3", Instant::now()));
    assert_eq!(error.kind, FailureKind::Deadline);
    assert_eq!(error.delivery, Delivery::NotSent);
    assert_eq!(error.request_context.unwrap().operation, "LoadPlugin");
    clean_finish(&mut probe, Instant::now());
}

#[test]
fn default_probe_wrong_reply_cannot_become_metadata_or_request_replay() {
    let _serial = serial();
    let mut probe = probe(&ledger(
        HostResponse::Success {
            message: "not PluginInfo".into(),
        },
        "assert sys.stdin.read()==''",
    ));
    let error = failure(probe.load_default_class("fixture-default.vst3", deadline()));
    assert_eq!(error.kind, FailureKind::WrongReply);
    assert_eq!(error.delivery, Delivery::MayHaveApplied);
    assert_eq!(error.request_context.unwrap().operation, "LoadPlugin");
    assert!(error.diagnostic.is_none());
    clean_finish(&mut probe, Instant::now());
}
