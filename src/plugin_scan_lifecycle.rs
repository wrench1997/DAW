//! Scan control and actual helper retirement; plugin owners stay on the worker.

use super::PluginDescriptor;
#[cfg(all(feature = "vst3", any(windows, target_os = "linux")))]
use super::{Vst3CatalogProbeInfo, Vst3ProbeInfo};
use serde::Serialize;
#[cfg(all(feature = "vst3", any(windows, target_os = "linux")))]
use std::path::Path;
use std::path::PathBuf;
#[cfg(all(feature = "vst3", any(windows, target_os = "linux")))]
use std::sync::Mutex;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Debug, Serialize)]
pub enum ScanStopKind {
    Cancelled,
    #[cfg(any(test, feature = "vst3"))]
    HelperUnavailable,
    #[cfg(any(test, all(feature = "vst3", any(windows, target_os = "linux"))))]
    ResourceBusy,
    #[cfg(any(test, all(feature = "vst3", any(windows, target_os = "linux"))))]
    CleanupPending,
    #[cfg(any(test, all(feature = "vst3", any(windows, target_os = "linux"))))]
    UncleanRetirement,
    WorkerExited,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProbeLifecycle {
    pub path: PathBuf,
    pub helper_pid: Option<u32>,
    pub load_failure: Option<String>,
    pub reaped: bool,
    pub exit_success: bool,
    pub exit_code: Option<i32>,
    pub kill_requested: bool,
    pub shutdown_failure: Option<String>,
    pub kill_error: Option<i32>,
    pub wait_error: Option<i32>,
    pub pipe_error: Option<i32>,
}

impl ProbeLifecycle {
    #[cfg(all(feature = "vst3", any(windows, target_os = "linux")))]
    pub fn healthy_retirement(&self) -> bool {
        self.helper_pid.is_some()
            && self.reaped
            && self.exit_success
            && !self.kill_requested
            && self.shutdown_failure.is_none()
            && self.kill_error.is_none()
            && self.wait_error.is_none()
            && self.pipe_error.is_none()
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ScanStopped {
    pub kind: ScanStopKind,
    pub detail: String,
    pub probe_lifecycle: Vec<ProbeLifecycle>,
}

impl ScanStopped {
    pub fn new(kind: ScanStopKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
            probe_lifecycle: Vec::new(),
        }
    }

    pub fn description(&self) -> String {
        format!(
            "Plugin scan stopped: {}; previous index retained",
            self.detail
        )
    }

    pub(super) fn cancelled() -> Self {
        Self::new(ScanStopKind::Cancelled, "cancelled")
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ScanReport {
    pub plugins: Vec<PluginDescriptor>,
    pub probe_lifecycle: Vec<ProbeLifecycle>,
}

#[derive(Clone, Debug, Serialize)]
pub enum ScanOutcome {
    Completed(ScanReport),
    Stopped(ScanStopped),
}

pub(super) struct ProbeFailure {
    pub message: String,
    pub stopped: Option<ScanStopped>,
    pub lifecycle: Option<Box<ProbeLifecycle>>,
}

impl ProbeFailure {
    pub fn metadata(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            stopped: None,
            lifecycle: None,
        }
    }

    #[cfg(feature = "vst3")]
    pub fn stop(kind: ScanStopKind, detail: impl Into<String>) -> Self {
        let detail = detail.into();
        Self {
            message: detail.clone(),
            stopped: Some(ScanStopped::new(kind, detail)),
            lifecycle: None,
        }
    }
}

pub(super) struct ScanControl {
    flag: AtomicBool,
    #[cfg(all(feature = "vst3", any(windows, target_os = "linux")))]
    current: Mutex<Option<vst3_host::default_class_probe::CancelHandle>>,
}

impl ScanControl {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            flag: AtomicBool::new(false),
            #[cfg(all(feature = "vst3", any(windows, target_os = "linux")))]
            current: Mutex::new(None),
        })
    }

    #[cfg(any(test, all(feature = "vst3", any(windows, target_os = "linux"))))]
    pub fn cancelled(&self) -> bool {
        self.flag.load(Ordering::Acquire)
    }

    pub fn flag(&self) -> &AtomicBool {
        &self.flag
    }

    pub fn cancel(&self) {
        self.flag.store(true, Ordering::Release);
        #[cfg(all(feature = "vst3", any(windows, target_os = "linux")))]
        {
            let handle = self
                .current
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone();
            if let Some(handle) = handle {
                handle.cancel();
            }
        }
    }

    #[cfg(all(feature = "vst3", any(windows, target_os = "linux")))]
    fn publish(&self, handle: vst3_host::default_class_probe::CancelHandle) {
        // Publish and inspect cancellation in the same short critical section.
        // Cancellation itself never runs while holding this slot mutex.
        let cancelled = {
            let mut slot = self
                .current
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            *slot = Some(handle.clone());
            self.cancelled()
        };
        if cancelled {
            handle.cancel();
        }
    }

    #[cfg(all(feature = "vst3", any(windows, target_os = "linux")))]
    fn clear(&self) {
        self.current
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
    }
}

#[cfg(all(feature = "vst3", any(windows, target_os = "linux")))]
pub(super) fn probe(
    path: &Path,
    helper: &Path,
    timeout: std::time::Duration,
    control: &ScanControl,
) -> Result<Vst3ProbeInfo, ProbeFailure> {
    match probe_request(path, helper, timeout, control, ScanRequest::LoadedDefault)? {
        ScanReply::Loaded(info) => Ok(info),
        ScanReply::Catalog(_) => unreachable!("closed loaded-default request/reply pair"),
    }
}

#[cfg(all(feature = "vst3", any(windows, target_os = "linux")))]
pub(super) fn probe_catalog(
    path: &Path,
    helper: &Path,
    timeout: std::time::Duration,
    control: &ScanControl,
) -> Result<Vst3CatalogProbeInfo, ProbeFailure> {
    match probe_request(path, helper, timeout, control, ScanRequest::FactoryCatalog)? {
        ScanReply::Catalog(info) => Ok(info),
        ScanReply::Loaded(_) => unreachable!("closed factory request/reply pair"),
    }
}

#[cfg(all(feature = "vst3", any(windows, target_os = "linux")))]
#[derive(Clone, Copy)]
enum ScanRequest {
    LoadedDefault,
    FactoryCatalog,
}

#[cfg(all(feature = "vst3", any(windows, target_os = "linux")))]
enum ScanReply {
    Loaded(Vst3ProbeInfo),
    Catalog(Vst3CatalogProbeInfo),
}

#[cfg(all(feature = "vst3", any(windows, target_os = "linux")))]
fn probe_request(
    path: &Path,
    helper: &Path,
    timeout: std::time::Duration,
    control: &ScanControl,
    request: ScanRequest,
) -> Result<ScanReply, ProbeFailure> {
    #[cfg(windows)]
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    let mut command = Command::new(helper);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // Same hidden production helper policy.
    probe_command(path, &mut command, timeout, control, request)
}

// Both closed request modes use this one owner, cancellation slot and retirement
// state machine. Tests substitute a repository-owned protocol child, never a DLL.
#[cfg(all(feature = "vst3", any(windows, target_os = "linux")))]
fn probe_command(
    path: &Path,
    command: &mut std::process::Command,
    timeout: std::time::Duration,
    control: &ScanControl,
    request: ScanRequest,
) -> Result<ScanReply, ProbeFailure> {
    use std::time::{Duration, Instant};
    use vst3_host::default_class_probe::{
        CleanupObservation, CleanupState, Failure, FailureKind, Probe,
    };

    fn describe(failure: &Failure) -> String {
        match &failure.diagnostic {
            Some(message) => format!("{:?}: {message}", failure.kind),
            None => format!("{:?}", failure.kind),
        }
    }

    fn lifecycle(
        path: &Path,
        pid: Option<u32>,
        load_failure: Option<String>,
        cleanup: CleanupObservation,
    ) -> ProbeLifecycle {
        let (reaped, exit_success, exit_code) = match cleanup.state {
            CleanupState::Reaped(status) => (true, status.success(), status.code()),
            _ => (false, false, None),
        };
        ProbeLifecycle {
            path: path.to_path_buf(),
            helper_pid: pid,
            load_failure,
            reaped,
            exit_success,
            exit_code,
            kill_requested: cleanup.kill_requested,
            shutdown_failure: cleanup.shutdown_failure.as_ref().map(describe),
            kill_error: cleanup.kill_error,
            wait_error: cleanup.wait_error,
            pipe_error: cleanup.pipe_error,
        }
    }

    fn retire(probe: &mut Probe) -> CleanupObservation {
        let now = Instant::now();
        probe.dispose(
            now + Duration::from_millis(250),
            now + Duration::from_millis(2250),
        )
    }

    if control.cancelled() {
        return Err(ProbeFailure::stop(
            ScanStopKind::Cancelled,
            "cancelled before helper spawn",
        ));
    }
    // A prior failed scan may have transferred its unfinished owner into the
    // process-wide quarantine. Advance only that existing cleanup on this
    // worker; neither a busy owner nor pending pipe/reap permits a new child.
    recover_abandoned()?;
    if control.cancelled() {
        return Err(ProbeFailure::stop(
            ScanStopKind::Cancelled,
            "cancelled before helper spawn",
        ));
    }
    let mut owner = match Probe::spawn(command, Instant::now() + timeout) {
        Ok(owner) => owner,
        Err(mut failure) => {
            let kind = failure.failure.kind;
            let message = describe(&failure.failure);
            let mut error = ProbeFailure::stop(
                if matches!(kind, FailureKind::Busy) {
                    ScanStopKind::ResourceBusy
                } else {
                    ScanStopKind::HelperUnavailable
                },
                message,
            );
            if let Some(mut owner) = failure.cleanup.take() {
                let pid = owner.helper_pid();
                let cleanup = retire(&mut owner);
                error.lifecycle = Some(Box::new(lifecycle(
                    path,
                    pid,
                    Some(error.message.clone()),
                    cleanup,
                )));
                // Dropping an unfinished owner deposits that same owner into
                // the transport quarantine; it never detaches pipe resources.
            }
            if error
                .lifecycle
                .as_ref()
                .is_some_and(|receipt| !receipt.reaped)
            {
                error.stopped = Some(ScanStopped::new(
                    ScanStopKind::CleanupPending,
                    format!(
                        "{}; helper cleanup is still pending; rescan advances retained cleanup",
                        error.message
                    ),
                ));
            }
            if control.cancelled() {
                let detail = error
                    .stopped
                    .as_ref()
                    .map_or_else(|| error.message.clone(), |stopped| stopped.detail.clone());
                error.stopped = Some(ScanStopped::new(
                    ScanStopKind::Cancelled,
                    format!("cancelled; {detail}"),
                ));
            }
            return Err(error);
        }
    };
    let pid = owner.helper_pid();
    control.publish(owner.cancellation());
    // The original response window is sampled once after spawn, covers the
    // whole write/read/decode exchange, and is never extended or replayed.
    let deadline = Instant::now() + timeout;
    let factory_catalog = matches!(request, ScanRequest::FactoryCatalog);
    let loaded =
        match request {
            ScanRequest::LoadedDefault => owner
                .load_default_class(&path.to_string_lossy(), deadline)
                .map(|info| {
                    ScanReply::Loaded(Vst3ProbeInfo {
                        name: info.name,
                        vendor: info.vendor,
                        metadata: super::Vst3ScanMetadata {
                            class_uid: info.uid,
                            category: info.category,
                            has_midi_input: info.has_midi_input,
                            has_midi_output: info.has_midi_output,
                        },
                        lifecycle: None,
                    })
                }),
            ScanRequest::FactoryCatalog => owner
                .probe_factory(&path.to_string_lossy(), deadline)
                .map(|metadata| {
                    ScanReply::Catalog(Vst3CatalogProbeInfo {
                        catalog: super::Vst3FactoryCatalog {
                            factory: super::Vst3FactoryInfo {
                                vendor: metadata.factory.vendor,
                                url: metadata.factory.url,
                                email: metadata.factory.email,
                                flags: metadata.factory.flags,
                            },
                            classes: metadata
                                .classes
                                .into_iter()
                                .map(|class| super::Vst3FactoryClass {
                                    class_id: class.class_id,
                                    name: class.name,
                                    category: class.category,
                                    cardinality: class.cardinality,
                                    vendor: class.vendor,
                                    version: class.version,
                                    sdk_version: class.sdk_version,
                                    sub_categories: class.sub_categories,
                                    class_flags: class.class_flags,
                                })
                                .collect(),
                        },
                        lifecycle: None,
                    })
                }),
        };
    let cleanup = retire(&mut owner);
    let receipt = lifecycle(path, pid, loaded.as_ref().err().map(describe), cleanup);
    control.clear();

    if control.cancelled() {
        let detail = if receipt.reaped {
            "cancelled"
        } else {
            "cancelled; helper cleanup is still pending; rescan advances retained cleanup"
        };
        let mut error = ProbeFailure::stop(ScanStopKind::Cancelled, detail);
        error.lifecycle = Some(Box::new(receipt));
        return Err(error);
    }
    if !receipt.reaped {
        let mut error = ProbeFailure::stop(
            ScanStopKind::CleanupPending,
            "helper cleanup is still pending; rescan advances retained cleanup",
        );
        error.lifecycle = Some(Box::new(receipt));
        return Err(error);
    }
    match loaded {
        // A real reap is necessary but does not excuse abnormal exit, kill or
        // failed pipe cleanup, including when the metadata request also failed.
        Err(ref failure) if factory_catalog && !receipt.healthy_retirement() => {
            let mut error = ProbeFailure::stop(
                ScanStopKind::UncleanRetirement,
                format!("{}; helper did not retire cleanly", describe(failure)),
            );
            error.lifecycle = Some(Box::new(receipt));
            Err(error)
        }
        Err(failure) => {
            let mut error = ProbeFailure::metadata(describe(&failure));
            error.lifecycle = Some(Box::new(receipt));
            Err(error)
        }
        Ok(mut info) if receipt.healthy_retirement() => {
            match &mut info {
                ScanReply::Loaded(info) => info.lifecycle = Some(receipt),
                ScanReply::Catalog(info) => info.lifecycle = Some(receipt),
            }
            Ok(info)
        }
        Ok(_) => {
            let mut error = ProbeFailure::stop(
                ScanStopKind::UncleanRetirement,
                "metadata returned but helper did not retire cleanly",
            );
            error.lifecycle = Some(Box::new(receipt));
            Err(error)
        }
    }
}

#[cfg(all(feature = "vst3", any(windows, target_os = "linux")))]
fn recover_abandoned() -> Result<(), ProbeFailure> {
    use std::time::{Duration, Instant};
    use vst3_host::default_class_probe::{CleanupState, Probe};

    // No protocol, new load, or renewed graceful-exit interval. This bounded
    // continuation may finish a prior owner's pipe cancellation and real reap.
    let cleanup = Probe::cleanup_abandoned(Instant::now() + Duration::from_millis(250));
    match cleanup.state {
        CleanupState::Empty | CleanupState::Reaped(_) => Ok(()),
        CleanupState::Busy => Err(ProbeFailure::stop(
            ScanStopKind::ResourceBusy,
            "another helper owner is still active",
        )),
        CleanupState::Pending => Err(ProbeFailure::stop(
            ScanStopKind::CleanupPending,
            format!(
                "retained helper cleanup is still pending: {cleanup:?}; previous index retained"
            ),
        )),
    }
}

#[cfg(all(test, feature = "vst3", any(windows, target_os = "linux")))]
#[path = "plugin_catalog_transport_tests.rs"]
mod catalog_transport_tests;

#[cfg(all(test, feature = "vst3", any(windows, target_os = "linux")))]
mod tests {
    use super::*;
    use std::process::Command;
    use std::time::{Duration, Instant};
    use vst3_host::default_class_probe::{CleanupState, Probe};

    fn fixture() -> Probe {
        let executable = std::env::var_os("CITRUS_STRICT_TEST_PYTHON").unwrap_or_else(|| {
            if cfg!(windows) {
                "python".into()
            } else {
                "python3".into()
            }
        });
        let mut command = Command::new(executable);
        command.args(["-u", "-c", "import time; time.sleep(30)"]);
        match Probe::spawn(&mut command, Instant::now() + Duration::from_secs(3)) {
            Ok(owner) => owner,
            Err(failure) => panic!("mock helper spawn failed: {:?}", failure.failure),
        }
    }

    #[test]
    fn scan_retry_reaps_abandoned_owner_before_admitting_another_helper() {
        let _guard = super::super::helper_transport_test_lock();
        assert!(recover_abandoned().is_ok());
        let owner = fixture();
        let first_pid = owner.helper_pid().unwrap();
        drop(owner);
        // No background reaper exists. The production scan retry itself must
        // release the actual retained owner before another child is admitted.
        assert!(recover_abandoned().is_ok());
        let mut next = fixture();
        assert_ne!(next.helper_pid().unwrap(), first_pid);
        let cleanup = next.dispose(Instant::now(), Instant::now() + Duration::from_secs(3));
        assert!(matches!(cleanup.state, CleanupState::Reaped(_)));
    }

    #[test]
    fn scan_retry_cannot_take_or_replace_an_active_helper_owner() {
        let _guard = super::super::helper_transport_test_lock();
        assert!(recover_abandoned().is_ok());
        let mut owner = fixture();
        let pid = owner.helper_pid();
        let failure = recover_abandoned().expect_err("live owner must remain busy");
        assert!(matches!(
            failure.stopped.unwrap().kind,
            ScanStopKind::ResourceBusy
        ));
        assert_eq!(owner.helper_pid(), pid);
        let cleanup = owner.dispose(Instant::now(), Instant::now() + Duration::from_secs(3));
        assert!(matches!(cleanup.state, CleanupState::Reaped(_)));
    }

    #[test]
    fn scan_cancel_slot_preserves_both_publication_orders_without_a_load() {
        use vst3_host::default_class_probe::{Delivery, FailureKind};
        let _guard = super::super::helper_transport_test_lock();
        for cancel_first in [true, false] {
            assert!(recover_abandoned().is_ok());
            let control = ScanControl::new();
            let mut owner = fixture();
            if cancel_first {
                control.cancel();
            }
            control.publish(owner.cancellation());
            if !cancel_first {
                control.cancel();
            }
            let failure = owner
                .load_default_class(
                    "no-sdk-needed.vst3",
                    Instant::now() + Duration::from_secs(1),
                )
                .unwrap_err();
            assert_eq!(failure.kind, FailureKind::Cancelled);
            assert_eq!(failure.delivery, Delivery::NotSent);
            let cleanup = owner.dispose(Instant::now(), Instant::now() + Duration::from_secs(3));
            assert!(matches!(cleanup.state, CleanupState::Reaped(_)));
            control.clear();
        }
    }
}
