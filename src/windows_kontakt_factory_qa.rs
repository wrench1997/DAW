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
