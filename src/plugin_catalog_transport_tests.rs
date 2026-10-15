//! Actual repository-owned child/pipe fixtures. No plug-in DLL or device is used.
use super::*;
use std::process::Command;
use std::time::{Duration, Instant};

// These mock children own no plug-in state. Once their actual request/EOF
// assertions and stream flushes finish, interpreter finalization is not part of
// the protocol contract being tested under the terminal 100 ms exit window.
const PROTOCOL_COMPLETE_EXIT: &str = "sys.stdout.flush()\nsys.stderr.flush()\nos._exit(0)";

fn protocol_child(script: &str) -> Command {
    let executable = std::env::var_os("CITRUS_STRICT_TEST_PYTHON").unwrap_or_else(|| {
        if cfg!(windows) {
            "python".into()
        } else {
            "python3".into()
        }
    });
    let mut command = Command::new(executable);
    command.args(["-u", "-c", script]);
    command
}

fn factory_script(response: serde_json::Value, expect_shutdown: bool, ending: &str) -> String {
    let response = serde_json::to_string(&response).unwrap();
    let shutdown = if expect_shutdown {
        "assert json.loads(sys.stdin.readline())=='Shutdown'\n"
    } else {
        ""
    };
    format!(
        "import json,os,sys,time\nassert json.loads(sys.stdin.readline())=={{'ProbeFactory':{{'path':'fixture-catalog.vst3'}}}}\nprint({response:?},flush=True)\n{shutdown}assert sys.stdin.read()==''\n{ending}\n"
    )
}

fn factory_response() -> serde_json::Value {
    let catalog = super::super::catalog_tests::catalog();
    serde_json::json!({"FactoryMetadata":{"metadata":{
        "path":"fixture-catalog.vst3", "factory":catalog.factory, "classes":catalog.classes,
    }}})
}

#[test]
fn production_catalog_sends_only_factory_request_then_actual_healthy_shutdown() {
    let _guard = super::super::helper_transport_test_lock();
    let mut command = protocol_child(&factory_script(factory_response(), true, ""));
    let result = probe_command(
        Path::new("fixture-catalog.vst3"),
        &mut command,
        Duration::from_secs(3),
        &ScanControl::new(),
        ScanRequest::FactoryCatalog,
    );
    let info = match result {
        Ok(ScanReply::Catalog(info)) => info,
        Ok(_) => panic!("factory request became loaded-default metadata"),
        Err(error) => panic!("{}", error.message),
    };
    assert_eq!(info.catalog, super::super::catalog_tests::catalog());
    let receipt = info.lifecycle.unwrap();
    assert_eq!(receipt.path, Path::new("fixture-catalog.vst3"));
    assert!(receipt.healthy_retirement());
    assert!(receipt.load_failure.is_none());
}

#[test]
fn production_catalog_rejects_wrong_reply_failure_exit_and_forced_kill_without_load_retry() {
    let _guard = super::super::helper_transport_test_lock();
    for (scenario, label) in [
        (0, "reported-factory-error"),
        (1, "wrong-reply-shape"),
        (2, "nonzero-exit-after-shutdown"),
        (3, "forced-kill-after-shutdown"),
        (4, "wrong-requested-path"),
    ] {
        let mut response = factory_response();
        let ending = match scenario {
            0 => {
                response = serde_json::json!({"Error":{"message":"factory refused"}});
                PROTOCOL_COMPLETE_EXIT
            }
            1 => {
                response = serde_json::json!("Success");
                PROTOCOL_COMPLETE_EXIT
            }
            2 => "sys.exit(9)",
            3 => "time.sleep(30)",
            4 => {
                response["FactoryMetadata"]["metadata"]["path"] = "wrong.vst3".into();
                PROTOCOL_COMPLETE_EXIT
            }
            _ => unreachable!(),
        };
        let mut command = protocol_child(&factory_script(
            response,
            scenario == 2 || scenario == 3,
            ending,
        ));
        let started = Instant::now();
        let failure = match probe_command(
            Path::new("fixture-catalog.vst3"),
            &mut command,
            Duration::from_secs(3),
            &ScanControl::new(),
            ScanRequest::FactoryCatalog,
        ) {
            Err(error) => error,
            Ok(_) => panic!("invalid catalog helper admitted scenario {scenario} ({label})"),
        };
        // Keep the actual primary rejection and every independent cleanup field
        // beside the failing scenario. A reap alone does not identify which
        // healthy-retirement predicate failed on a loaded CI runner.
        let evidence = serde_json::json!({
            "scenario": scenario,
            "label": label,
            "elapsed_ms": started.elapsed().as_millis(),
            "failure": failure.message,
            "stopped": failure.stopped,
            "lifecycle": failure.lifecycle,
        });
        eprintln!("catalog-retirement-attempt {evidence}");
        let receipt = failure
            .lifecycle
            .as_ref()
            .unwrap_or_else(|| panic!("catalog failure lost its actual lifecycle: {evidence}"));
        assert!(receipt.reaped, "{evidence}");
        if scenario == 2 || scenario == 3 {
            assert!(
                matches!(
                    failure.stopped.as_ref().map(|stopped| &stopped.kind),
                    Some(ScanStopKind::UncleanRetirement)
                ),
                "{evidence}"
            );
            assert!(!receipt.healthy_retirement(), "{evidence}");
        } else {
            assert!(receipt.healthy_retirement(), "{evidence}");
            assert!(receipt.load_failure.is_some(), "{evidence}");
        }
        if scenario == 3 {
            assert!(receipt.kill_requested, "{evidence}");
        }
    }
}

#[test]
fn production_catalog_terminal_reply_distinguishes_protocol_exit_from_interpreter_teardown() {
    let _guard = super::super::helper_transport_test_lock();
    for direct_exit in [true, false] {
        // Register the same slow interpreter finalizer only after the fixture's
        // real request and EOF assertions. Direct exit deliberately omits that
        // unrelated work; normal Python exit must still hit actual kill/reap.
        let ending = format!(
            "import atexit\ndef delayed_finalizer():\n print('catalog-fixture interpreter-finalizer-entered after EOF',file=sys.stderr,flush=True)\n time.sleep(30)\natexit.register(delayed_finalizer)\n{}",
            if direct_exit {
                PROTOCOL_COMPLETE_EXIT
            } else {
                ""
            },
        );
        let mut command = protocol_child(&factory_script(
            serde_json::json!({"Error":{"message":"factory refused before interpreter exit"}}),
            false,
            &ending,
        ));
        let started = Instant::now();
        let failure = match probe_command(
            Path::new("fixture-catalog.vst3"),
            &mut command,
            Duration::from_secs(3),
            &ScanControl::new(),
            ScanRequest::FactoryCatalog,
        ) {
            Err(failure) => failure,
            Ok(_) => panic!("terminal reply admitted a catalog; direct_exit={direct_exit}"),
        };
        let evidence = serde_json::json!({
            "scenario": "reported-factory-error-interpreter-finalizer",
            "direct_exit": direct_exit,
            "elapsed_ms": started.elapsed().as_millis(),
            "failure": failure.message,
            "stopped": failure.stopped,
            "lifecycle": failure.lifecycle,
        });
        eprintln!("catalog-retirement-attempt {evidence}");
        if direct_exit {
            assert!(failure.stopped.is_none(), "{evidence}");
        } else {
            assert!(
                matches!(
                    failure.stopped.as_ref().map(|stopped| &stopped.kind),
                    Some(ScanStopKind::UncleanRetirement)
                ),
                "{evidence}"
            );
        }
        let receipt = failure
            .lifecycle
            .as_ref()
            .unwrap_or_else(|| panic!("terminal failure lost its actual lifecycle: {evidence}"));
        assert!(receipt.reaped, "{evidence}");
        assert_eq!(receipt.kill_requested, !direct_exit, "{evidence}");
        assert_eq!(receipt.healthy_retirement(), direct_exit, "{evidence}");
        assert_eq!(
            receipt.load_failure.as_deref(),
            Some("OperationRejected: factory refused before interpreter exit"),
            "{evidence}"
        );
    }
}

#[test]
fn production_catalog_deadline_and_sticky_cancellation_never_admit_metadata() {
    let _guard = super::super::helper_transport_test_lock();
    let script = "import json,sys,time\nassert json.loads(sys.stdin.readline())=={'ProbeFactory':{'path':'fixture-catalog.vst3'}}\ntime.sleep(30)";
    let mut command = protocol_child(script);
    let start = Instant::now();
    let failure = match probe_command(
        Path::new("fixture-catalog.vst3"),
        &mut command,
        Duration::from_millis(100),
        &ScanControl::new(),
        ScanRequest::FactoryCatalog,
    ) {
        Err(error) => error,
        Ok(_) => panic!("deadline returned metadata"),
    };
    assert!(failure.message.contains("Deadline"));
    assert!(start.elapsed() < Duration::from_secs(4));
    let receipt = failure.lifecycle.unwrap();
    assert!(receipt.reaped && receipt.kill_requested);

    let control = ScanControl::new();
    control.cancel();
    let mut never_spawned = Command::new("nonexistent-catalog-helper-must-not-start");
    let failure = match probe_command(
        Path::new("fixture-catalog.vst3"),
        &mut never_spawned,
        Duration::from_secs(3),
        &control,
        ScanRequest::FactoryCatalog,
    ) {
        Err(error) => error,
        Ok(_) => panic!("cancelled catalog was admitted"),
    };
    assert!(matches!(
        failure.stopped.unwrap().kind,
        ScanStopKind::Cancelled
    ));
    assert!(failure.lifecycle.is_none());
}

#[test]
fn production_catalog_in_flight_cancellation_retains_actual_cleanup_without_retry() {
    let _guard = super::super::helper_transport_test_lock();
    let directory = super::super::tests::TestDirectory::new();
    let marker = directory.0.join("factory-request-delivered");
    let script = format!(
        "import json,sys,time,pathlib\nassert json.loads(sys.stdin.readline())=={{'ProbeFactory':{{'path':'fixture-catalog.vst3'}}}}\npathlib.Path({:?}).write_text('delivered')\ntime.sleep(30)",
        marker.to_string_lossy(),
    );
    let control = ScanControl::new();
    let cancel = control.clone();
    let cancellation = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !marker.is_file() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        let delivered = marker.is_file();
        cancel.cancel();
        delivered
    });
    let result = probe_command(
        Path::new("fixture-catalog.vst3"),
        &mut protocol_child(&script),
        Duration::from_secs(3),
        &control,
        ScanRequest::FactoryCatalog,
    );
    let delivered = cancellation.join().unwrap();
    let failure = match result {
        Err(error) => error,
        Ok(_) => panic!("cancelled in-flight catalog returned metadata"),
    };
    assert!(
        delivered,
        "fixture must have received its one factory request"
    );
    assert!(matches!(
        failure.stopped.unwrap().kind,
        ScanStopKind::Cancelled
    ));
    assert!(failure.lifecycle.unwrap().reaped);
}
