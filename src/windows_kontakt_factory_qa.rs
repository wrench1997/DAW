//! Explicit factory-only experiment. A declared class is not a loaded instrument.

use std::{
    fs,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use vst3_host::default_class_probe::{CleanupObservation, CleanupState, Probe};

use super::{absolute_env, scan_binary_snapshot, scan_new_json};

const SELECTOR: &str =
    "windows_kontakt_qa::runtime_qa::factory_qa::windows_kontakt_factory_metadata_and_reap";
const OPT_IN: &str = "actual-kontakt-factory-only-metadata-and-reap-v1";
static ADMITTED: AtomicBool = AtomicBool::new(false);
const CATALOG_SELECTOR: &str =
    "windows_kontakt_qa::runtime_qa::factory_qa::windows_kontakt_production_catalog_and_reap";
const CATALOG_OPT_IN: &str = "actual-kontakt-production-catalog-and-reap-v1";

fn cleanup_evidence(pid: Option<u32>, cleanup: &CleanupObservation) -> Value {
    let (reaped, exit_success, exit_code) = match cleanup.state {
        CleanupState::Reaped(status) => (true, status.success(), status.code()),
        _ => (false, false, None),
    };
    let healthy = pid.is_some_and(|pid| pid != 0)
        && reaped
        && exit_success
        && !cleanup.kill_requested
        && cleanup.shutdown_failure.is_none()
        && cleanup.kill_error.is_none()
        && cleanup.wait_error.is_none()
        && cleanup.pipe_error.is_none();
    json!({
        "helper_pid":pid,"state":format!("{:?}",cleanup.state),
        "reaped":reaped,"exit_success":exit_success,"exit_code":exit_code,
        "kill_requested":cleanup.kill_requested,"kill_error":cleanup.kill_error,
        "wait_error":cleanup.wait_error,"pipe_error":cleanup.pipe_error,
        "shutdown_failure":cleanup.shutdown_failure.as_ref().map(|error|format!("{error:?}")),
        "healthy_retirement":healthy,
        "process_absence_used_as_reap_evidence":false,
    })
}

#[test]
fn factory_qa_cleanup_requires_actual_clean_retirement() {
    use std::os::windows::process::ExitStatusExt;
    use vst3_host::default_class_probe::{Delivery, Failure, FailureKind};

    fn clean() -> CleanupObservation {
        CleanupObservation {
            state: CleanupState::Reaped(std::process::ExitStatus::from_raw(0)),
            shutdown_failure: None,
            kill_error: None,
            wait_error: None,
            pipe_error: None,
            kill_requested: false,
        }
    }
    assert_eq!(
        cleanup_evidence(Some(1), &clean())["healthy_retirement"],
        true
    );
    for pid in [None, Some(0)] {
        assert_eq!(cleanup_evidence(pid, &clean())["healthy_retirement"], false);
    }
    for case in 0..7 {
        let mut report = clean();
        match case {
            0 => report.state = CleanupState::Reaped(std::process::ExitStatus::from_raw(1)),
            1 => report.state = CleanupState::Pending,
            2 => report.kill_requested = true,
            3 => report.kill_error = Some(5),
            4 => report.wait_error = Some(5),
            5 => report.pipe_error = Some(5),
            _ => {
                report.shutdown_failure = Some(Failure {
                    kind: FailureKind::Deadline,
                    delivery: Delivery::NotSent,
                    diagnostic: None,
                    diagnostic_unavailable: None,
                    request_context: None,
                    metadata_getter: None,
                    limit_evidence: None,
                });
            }
        }
        assert_eq!(
            cleanup_evidence(Some(1), &report)["healthy_retirement"],
            false
        );
    }
}

fn retire(owner: &mut Probe, output: &Path) -> (CleanupObservation, Option<String>) {
    let now = Instant::now();
    let mut cleanup = owner.dispose(
        now + Duration::from_millis(250),
        now + Duration::from_millis(2250),
    );
    let mut first_observation_error = None;
    if !matches!(cleanup.state, CleanupState::Reaped(_)) {
        first_observation_error = scan_new_json(
            &output.join("cleanup-pending.json"),
            &json!({
                "helper_pid":owner.helper_pid(),"cleanup":format!("{cleanup:?}"),
                "same_test_process_retains_owner":true,"original_deadlines_renewed":false,
            }),
        )
        .err();
    }
    // An observation expiry must not end this process with a live retained owner.
    // Continue only the same disposed owner; no request, spawn or grace renewal.
    // The outer observer must retain this exact test process if it remains Pending.
    while !matches!(cleanup.state, CleanupState::Reaped(_)) {
        match owner.continue_cleanup() {
            Ok(next) => cleanup = next,
            Err(error) => {
                first_observation_error.get_or_insert_with(|| format!("{error:?}"));
            }
        }
        if !matches!(cleanup.state, CleanupState::Reaped(_)) {
            thread::sleep(Duration::from_millis(10));
        }
    }
    (cleanup, first_observation_error)
}

#[test]
#[ignore = "actual installed Kontakt factory-only experiment; exact once-only opt-in and outer binary bindings required"]
fn windows_kontakt_factory_metadata_and_reap() {
    assert_eq!(
        std::env::var("CITRUS_KONTAKT_FACTORY_QA_OPT_IN").as_deref(),
        Ok(OPT_IN)
    );
    assert_eq!(
        std::env::args().skip(1).collect::<Vec<_>>(),
        [
            SELECTOR,
            "--ignored",
            "--exact",
            "--test-threads=1",
            "--nocapture"
        ]
    );
    assert!(
        !ADMITTED.swap(true, Ordering::AcqRel),
        "one attempt per process"
    );
    assert!(
        std::env::var_os("CITRUS_VST3_HELPER_SHUTDOWN_MESSAGE_PASS").is_none(),
        "do not combine the separate message-pass experiment with this one"
    );
    let _reservation_test_lock = crate::plugins::helper_transport_test_lock();
    let output = absolute_env("CITRUS_KONTAKT_FACTORY_QA_DIR");
    fs::create_dir(&output).expect("preserve all previous QA and user files");
    let executable = std::env::current_exe().unwrap().canonicalize().unwrap();
    let helper = executable.parent().unwrap().join("vst3-host-helper.exe");
    let helper = helper.canonicalize().unwrap();
    let executable_sha = std::env::var("CITRUS_KONTAKT_FACTORY_QA_EXECUTABLE_SHA256").unwrap();
    let helper_sha = std::env::var("CITRUS_KONTAKT_FACTORY_QA_HELPER_SHA256").unwrap();
    for supplied in [&executable_sha, &helper_sha] {
        assert!(supplied.len() == 64 && supplied.bytes().all(|b| b.is_ascii_hexdigit()));
    }
    let executable_before = scan_binary_snapshot(&executable, 256 * 1024 * 1024).unwrap();
    let helper_before = scan_binary_snapshot(&helper, 64 * 1024 * 1024).unwrap();
    let plugin = PathBuf::from(r"C:\Program Files\Common Files\VST3\Kontakt 8.vst3");
    scan_new_json(
        &output.join("preflight.json"),
        &json!({
            "schema":1,"selector":SELECTOR,"test_pid":std::process::id(),
            "test_executable":executable,"helper_executable":helper,"plugin":plugin,
            "supplied_executable_sha256":executable_sha,"supplied_helper_sha256":helper_sha,
            "supplied_hashes_verified_in_this_test":false,"outer_byte_binding_required":true,
            "factory_only":true,"runtime_default_class_claimed":false,
            "component_controller_view_state_MIDI_Process_device_requests":0,
        }),
    )
    .unwrap();

    let mut command = Command::new(&helper);
    command.creation_flags(0x0800_0000);
    let started = Instant::now();
    let mut metadata = None;
    let mut error = None;
    let mut cleanup_json = Value::Null;
    let mut cleanup_observation_error = None;
    match Probe::spawn(&mut command, Instant::now() + Duration::from_secs(5)) {
        Ok(mut owner) => {
            let pid = owner.helper_pid();
            let attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                owner.probe_factory(
                    &plugin.to_string_lossy(),
                    Instant::now() + Duration::from_secs(5),
                )
            }));
            let (cleanup, observation_error) = retire(&mut owner, &output);
            cleanup_json = cleanup_evidence(pid, &cleanup);
            cleanup_observation_error = observation_error;
            match attempt {
                Ok(Ok(actual)) => metadata = Some(actual),
                Ok(Err(failure)) => error = Some(format!("{failure:?}")),
                Err(_) => {
                    error =
                        Some("factory-only exchange unwound; cleanup completed afterward".into())
                }
            }
        }
        Err(mut failure) => {
            error = Some(format!("factory-only spawn failed: {:?}", failure.failure));
            if let Some(mut owner) = failure.cleanup.take() {
                let pid = owner.helper_pid();
                let (cleanup, observation_error) = retire(&mut owner, &output);
                cleanup_json = cleanup_evidence(pid, &cleanup);
                cleanup_observation_error = observation_error;
            }
        }
    }
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    let unchanged = scan_binary_snapshot(&executable, 256 * 1024 * 1024)
        .and_then(|actual| {
            if actual == executable_before {
                Ok(())
            } else {
                Err("test binary changed".into())
            }
        })
        .and_then(|()| scan_binary_snapshot(&helper, 64 * 1024 * 1024))
        .and_then(|actual| {
            if actual == helper_before {
                Ok(())
            } else {
                Err("helper binary changed".into())
            }
        });
    let declared_class_found = metadata.as_ref().is_some_and(|actual| {
        actual.path == plugin
            && actual.classes.iter().any(|class| {
                class.class_id == "5653544E694B386B6F6E74616B742038"
                    && class.category == "Audio Module Class"
            })
    });
    let passed = error.is_none()
        && unchanged.is_ok()
        && declared_class_found
        && cleanup_observation_error.is_none()
        && cleanup_json["healthy_retirement"] == true;
    scan_new_json(&output.join("result.json"), &json!({
        "schema":1,"passed":passed,"actual_error":error,"factory_metadata":metadata,
        "expected_declared_audio_class_found":declared_class_found,"cleanup":cleanup_json,
        "cleanup_observation_error":cleanup_observation_error,
        "actual_binary_bytes_unchanged":unchanged.is_ok(),"actual_binary_pin_error":unchanged.err(),
        "single_attempt_elapsed_ms":elapsed_ms,"performance_acceptance":false,
        "factory_declared_class_is_not_runtime_default_class":true,
        "component_controller_view_state_MIDI_Process_device_requests":0,
        "NOIRE_sound_state_restore_or_export_qualified":false,
        "production_scan_route_changed":false,
    })).unwrap();
    assert!(
        passed,
        "factory-only Kontakt experiment failed; preserve {}",
        output.display()
    );
}

fn catalog_scan_evidence(requested: &Path, outcome: &crate::plugins::ScanOutcome) -> (Value, bool) {
    use crate::plugins::{PluginFormat, ScanOutcome};

    let receipts = match outcome {
        ScanOutcome::Completed(report) => &report.probe_lifecycle,
        ScanOutcome::Stopped(stopped) => &stopped.probe_lifecycle,
    };
    let receipt = match receipts.as_slice() {
        [receipt] if receipt.path == requested => Some(receipt),
        _ => None,
    };
    let reaped = receipt
        .is_some_and(|receipt| receipt.helper_pid.is_some_and(|pid| pid != 0) && receipt.reaped);
    let healthy = reaped
        && receipt
            .is_some_and(|receipt| receipt.healthy_retirement() && receipt.exit_code == Some(0));
    let descriptor = match outcome {
        ScanOutcome::Completed(report) => match report.plugins.as_slice() {
            [descriptor] if descriptor.path == requested => Some(descriptor),
            _ => None,
        },
        ScanOutcome::Stopped(_) => None,
    };
    let catalog = descriptor.and_then(|descriptor| descriptor.vst3_catalog.as_ref());
    let declaration_only = descriptor.is_some_and(|descriptor| {
        descriptor.format == PluginFormat::Vst3
            && !descriptor.verified
            && descriptor.vst3_metadata.is_none()
            && descriptor.category == "Unknown"
            && !descriptor.is_instrument
            && descriptor.scan_error.is_none()
            && catalog.is_some()
    });
    let declared_class_found = catalog.is_some_and(|catalog| {
        catalog.classes.iter().any(|class| {
            class.class_id == "5653544E694B386B6F6E74616B742038"
                && class.category == "Audio Module Class"
        })
    });
    let passed = declaration_only
        && declared_class_found
        && healthy
        && receipt.is_some_and(|receipt| receipt.load_failure.is_none());
    (
        json!({
            "source":"production plugins::scan_catalog_checked; one fixed module catalog attempt",
            "requested_path":requested,"outcome":outcome,
            "exact_single_descriptor":descriptor.is_some(),"exact_single_lifecycle":receipt.is_some(),
            "scan_helper_strict_reap_proven":reaped,"scan_helper_healthy_exit_proven":healthy,
            "catalog_only_descriptor_invariants_hold":declaration_only,
            "expected_declared_audio_class_found":declared_class_found,
            "factory_catalog":catalog,"catalog_declarations_and_retirement_accepted":passed,
            "runtime_default_class_or_capabilities_verified":false,
            "OS_process_absence_used_as_reap_evidence":false,
        }),
        passed,
    )
}

#[test]
fn production_catalog_qa_requires_declarations_unknown_capabilities_and_original_retirement() {
    use crate::plugins::{PluginDescriptor, ScanOutcome, ScanReport, ScanStopKind, ScanStopped};

    let plugin = PathBuf::from("synthetic-catalog.vst3");
    let descriptor: PluginDescriptor = serde_json::from_value(json!({
        "id":"synthetic-one-module","name":"Synthetic catalog","vendor":"Factory vendor",
        "path":plugin,"format":"Vst3","category":"Unknown","is_instrument":false,
        "verified":false,"vst3_metadata":null,"scan_error":null,
        "vst3_catalog":{
            "factory":{"vendor":"Factory vendor","url":"","email":"","flags":0},
            "classes":[{
                "class_id":"5653544E694B386B6F6E74616B742038","name":"Declared audio class",
                "category":"Audio Module Class","cardinality":2147483647,
                "vendor":null,"version":null,"sdk_version":null,"sub_categories":null,"class_flags":null
            },{
                "class_id":"00000000000000000000000000000001","name":"Declared controller",
                "category":"Component Controller Class","cardinality":1,
                "vendor":"","version":"","sdk_version":"","sub_categories":[],"class_flags":0
            }]
        }
    })).unwrap();
    let completed = |plugins| {
        ScanOutcome::Completed(ScanReport {
            plugins,
            probe_lifecycle: Vec::new(),
        })
    };
    let (evidence, accepted) = catalog_scan_evidence(&plugin, &completed(vec![descriptor.clone()]));
    assert!(
        !accepted,
        "declarations alone cannot prove actual helper retirement"
    );
    assert_eq!(evidence["catalog_only_descriptor_invariants_hold"], true);
    assert_eq!(evidence["expected_declared_audio_class_found"], true);
    assert_eq!(evidence["scan_helper_healthy_exit_proven"], false);
    assert_eq!(
        evidence["factory_catalog"]["classes"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(evidence["factory_catalog"]["classes"][0]["vendor"].is_null());
    assert_eq!(evidence["factory_catalog"]["classes"][1]["vendor"], "");
    for case in 0..7 {
        let mut changed = descriptor.clone();
        match case {
            0 => changed.verified = true,
            1 => changed.category = "Instrument".into(),
            2 => changed.is_instrument = true,
            3 => changed.scan_error = Some("synthetic failure".into()),
            4 => {
                changed.vst3_metadata = Some(crate::plugins::Vst3ScanMetadata {
                    class_uid: "5653544E694B386B6F6E74616B742038".into(),
                    category: "Instrument".into(),
                    has_midi_input: true,
                    has_midi_output: false,
                })
            }
            5 => changed.path = "different.vst3".into(),
            _ => changed.vst3_catalog = None,
        }
        let (evidence, accepted) = catalog_scan_evidence(&plugin, &completed(vec![changed]));
        assert!(!accepted);
        assert_eq!(evidence["catalog_only_descriptor_invariants_hold"], false);
    }
    let (evidence, accepted) =
        catalog_scan_evidence(&plugin, &completed(vec![descriptor.clone(), descriptor]));
    assert!(!accepted);
    assert_eq!(evidence["exact_single_descriptor"], false);
    let stopped = ScanOutcome::Stopped(ScanStopped::new(
        ScanStopKind::Cancelled,
        "synthetic stopped catalog",
    ));
    let (evidence, accepted) = catalog_scan_evidence(&plugin, &stopped);
    assert!(!accepted);
    assert!(evidence["factory_catalog"].is_null());
}

fn retire_catalog_quarantine(output: &Path) -> (CleanupObservation, Option<String>) {
    // The production scanner already disposed its owner using its original
    // deadlines. If it transferred a Pending owner, only advance that same
    // quarantine slot: never scan again or renew its natural-exit interval.
    let mut cleanup = Probe::cleanup_abandoned(Instant::now() + Duration::from_millis(250));
    let mut observation_error = None;
    if !matches!(cleanup.state, CleanupState::Empty | CleanupState::Reaped(_)) {
        observation_error = scan_new_json(
            &output.join("catalog-cleanup-pending.json"),
            &json!({
                "cleanup":format!("{cleanup:?}"),"same_test_process_retains_quarantine":true,
                "original_deadlines_renewed":false,"scan_request_replayed":false,
            }),
        )
        .err();
    }
    while !matches!(cleanup.state, CleanupState::Empty | CleanupState::Reaped(_)) {
        thread::sleep(Duration::from_millis(10));
        cleanup = Probe::cleanup_abandoned(Instant::now() + Duration::from_millis(250));
    }
    (cleanup, observation_error)
}

#[test]
#[ignore = "actual production Kontakt catalog scan; independent exact opt-in, private adjacent helper and outer binary pins required"]
fn windows_kontakt_production_catalog_and_reap() {
    assert_eq!(
        std::env::var("CITRUS_KONTAKT_CATALOG_QA_OPT_IN").as_deref(),
        Ok(CATALOG_OPT_IN)
    );
    assert_eq!(
        std::env::args().skip(1).collect::<Vec<_>>(),
        [
            CATALOG_SELECTOR,
            "--ignored",
            "--exact",
            "--test-threads=1",
            "--nocapture"
        ]
    );
    assert!(
        !ADMITTED.swap(true, Ordering::AcqRel),
        "one attempt per process"
    );
    assert!(std::env::var_os("CITRUS_VST3_HELPER_SHUTDOWN_MESSAGE_PASS").is_none());
    let _reservation_test_lock = crate::plugins::helper_transport_test_lock();
    let output = absolute_env("CITRUS_KONTAKT_CATALOG_QA_DIR");
    fs::create_dir(&output).expect("catalog output must be new; preserve all previous files");
    let executable = std::env::current_exe().unwrap().canonicalize().unwrap();
    let helper = executable
        .parent()
        .unwrap()
        .join("vst3-host-helper.exe")
        .canonicalize()
        .unwrap();
    let executable_sha = std::env::var("CITRUS_KONTAKT_CATALOG_QA_EXECUTABLE_SHA256").unwrap();
    let helper_sha = std::env::var("CITRUS_KONTAKT_CATALOG_QA_HELPER_SHA256").unwrap();
    for supplied in [&executable_sha, &helper_sha] {
        assert!(supplied.len() == 64 && supplied.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }
    let executable_before = scan_binary_snapshot(&executable, 256 * 1024 * 1024).unwrap();
    let helper_before = scan_binary_snapshot(&helper, 64 * 1024 * 1024).unwrap();
    let plugin = PathBuf::from(r"C:\Program Files\Common Files\VST3\Kontakt 8.vst3");
    scan_new_json(
        &output.join("catalog-preflight.json"),
        &json!({
            "schema":1,"selector":CATALOG_SELECTOR,"test_pid":std::process::id(),
            "test_executable":executable,"helper_executable":helper,"plugin":plugin,
            "supplied_executable_sha256":executable_sha,"supplied_helper_sha256":helper_sha,
            "supplied_hashes_verified_in_this_test":false,"outer_byte_binding_required":true,
            "production_entry":"plugins::scan_catalog_checked","factory_catalog_only":true,
            "component_controller_view_state_MIDI_Process_device_requests":0,
            "user_project_or_preset_accessed":false,
        }),
    )
    .unwrap();
    let started = Instant::now();
    let attempted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::plugins::scan_catalog_checked(std::slice::from_ref(&plugin))
    }));
    let scan_elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    let (cleanup, cleanup_observation_error) = retire_catalog_quarantine(&output);
    let (scan, declarations_and_retirement_accepted, error) = match attempted {
        Ok(outcome) => {
            let outcome = match outcome {
                Ok(report) => crate::plugins::ScanOutcome::Completed(report),
                Err(stopped) => crate::plugins::ScanOutcome::Stopped(stopped),
            };
            let (scan, accepted) = catalog_scan_evidence(&plugin, &outcome);
            let error = match &outcome {
                crate::plugins::ScanOutcome::Stopped(stopped) => Some(stopped.description()),
                _ if !accepted => Some("catalog lacks one exact declaration-only entry, expected declaration or healthy original lifecycle".into()),
                _ => None,
            };
            (scan, accepted, error)
        }
        Err(_) => (
            Value::Null,
            false,
            Some("production catalog scan unwound; retained cleanup was observed afterward".into()),
        ),
    };
    let unchanged = scan_binary_snapshot(&executable, 256 * 1024 * 1024)
        .and_then(|actual| {
            if actual == executable_before {
                Ok(())
            } else {
                Err("test binary changed during catalog scan".into())
            }
        })
        .and_then(|()| scan_binary_snapshot(&helper, 64 * 1024 * 1024))
        .and_then(|actual| {
            if actual == helper_before {
                Ok(())
            } else {
                Err("helper binary changed during catalog scan".into())
            }
        });
    // Later quarantine completion cannot turn a stopped scan or its original
    // unhealthy/missing receipt into an accepted catalog publication.
    let passed = declarations_and_retirement_accepted
        && error.is_none()
        && unchanged.is_ok()
        && cleanup_observation_error.is_none();
    scan_new_json(&output.join("catalog-result.json"), &json!({
        "schema":1,"passed":passed,"actual_error":error,"scan":scan,
        "actual_binary_bytes_unchanged":unchanged.is_ok(),"actual_binary_pin_error":unchanged.err(),
        "retained_cleanup_observation":format!("{cleanup:?}"),
        "retained_cleanup_observation_is_not_original_scan_healthy_reap_proof":true,
        "cleanup_observation_error":cleanup_observation_error,
        "single_production_scan_elapsed_ms":scan_elapsed_ms,"performance_acceptance":false,
        "production_entry":"plugins::scan_catalog_checked","runtime_default_class_claimed":false,
        "component_controller_view_state_MIDI_Process_device_requests":0,
        "NOIRE_sound_state_restore_or_export_qualified":false,"user_project_or_preset_accessed":false,
    })).unwrap();
    assert!(
        passed,
        "production Kontakt catalog scan failed; preserve {}",
        output.display()
    );
}
