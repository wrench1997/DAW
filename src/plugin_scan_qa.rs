//! Typed interpretation of an actual scan, shared by explicit native QA.
//! A cleanup receipt is not source readiness, inner SDK teardown or sound.

use super::scan_lifecycle::ProbeLifecycle;
use super::{PluginDescriptor, PluginFormat, ScanOutcome, ScanReport, scan_checked};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub(crate) struct CheckedScan {
    requested: PathBuf,
    outcome: ScanOutcome,
}

impl CheckedScan {
    pub(crate) fn perform(path: &Path) -> Self {
        Self {
            requested: path.to_path_buf(),
            outcome: match scan_checked(std::slice::from_ref(&path.to_path_buf())) {
                Ok(report) => ScanOutcome::Completed(report),
                Err(stopped) => ScanOutcome::Stopped(stopped),
            },
        }
    }

    fn receipt(&self) -> Option<&ProbeLifecycle> {
        let receipts = match &self.outcome {
            ScanOutcome::Completed(report) => &report.probe_lifecycle,
            ScanOutcome::Stopped(stopped) => &stopped.probe_lifecycle,
        };
        match receipts.as_slice() {
            [receipt] if receipt.path == self.requested => Some(receipt),
            _ => None,
        }
    }

    pub(crate) fn helper_reaped(&self) -> bool {
        self.receipt()
            .is_some_and(|receipt| receipt.helper_pid.is_some_and(|pid| pid != 0) && receipt.reaped)
    }

    fn healthy_exit(&self) -> bool {
        self.helper_reaped()
            && self
                .receipt()
                .is_some_and(ProbeLifecycle::healthy_retirement)
    }

    fn descriptor(&self) -> Option<&PluginDescriptor> {
        match &self.outcome {
            ScanOutcome::Completed(report) => match report.plugins.as_slice() {
                [descriptor] if descriptor.path == self.requested => Some(descriptor),
                _ => None,
            },
            ScanOutcome::Stopped(_) => None,
        }
    }

    fn metadata_available(&self) -> bool {
        self.healthy_exit()
            && self
                .receipt()
                .is_some_and(|receipt| receipt.load_failure.is_none())
            && self.descriptor().is_some_and(|descriptor| {
                descriptor.format == PluginFormat::Vst3
                    && descriptor.verified
                    && descriptor.scan_error.is_none()
                    && descriptor.vst3_metadata.is_some()
            })
    }

    pub(crate) fn evidence(&self) -> Value {
        json!({
            "source":"explicit loaded-default verification via plugins::scan_checked; not ordinary factory catalog scanning",
            "requested_path":self.requested,
            "outcome":self.outcome,
            "scan_helper_strict_reap_proven":self.helper_reaped(),
            "scan_helper_healthy_exit_proven":self.healthy_exit(),
            "metadata_available_before_expected_uid_admission":self.metadata_available(),
            "expected_uid_admission_not_yet_evaluated":true,
            "default_class_id":Value::Null,
            "source_worker_identity":Value::Null,
            "scope":"separate empty default-class scan instance; never source initialization or sound",
            "SDK_version_not_in_descriptor":true,
            "inner_SDK_terminate_and_FreeLibrary_results_not_proven":true,
            "source_helper_reap_and_source_worker_join_not_proven_by_scan":true,
            "no_editor_open_state_MIDI_Process_or_audio_device_requested_by_QA":true,
            "SDK_load_may_probe_createView_capability":true,
        })
    }

    pub(crate) fn admit(self, expected_uid: &str) -> Result<SingleModuleScan, String> {
        if let ScanOutcome::Stopped(stopped) = &self.outcome {
            return Err(stopped.description());
        }
        if !self.metadata_available() {
            return Err("single-module scan lacks exact descriptor, successful metadata or healthy concrete helper/pipe retirement; inspect the retained scan receipt".into());
        }
        if expected_uid.len() != 32 || !expected_uid.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("saved source class UID is not an exact 32-hex identifier".into());
        }
        if self
            .descriptor()
            .and_then(|descriptor| descriptor.vst3_metadata.as_ref())
            .is_none_or(|metadata| metadata.class_uid != expected_uid)
        {
            return Err(
                "actual default-class scan UID differs from the requested saved source".into(),
            );
        }
        Ok(SingleModuleScan {
            checked: self,
            expected_uid: expected_uid.to_owned(),
        })
    }
}

pub(crate) struct SingleModuleScan {
    checked: CheckedScan,
    expected_uid: String,
}

impl SingleModuleScan {
    pub(crate) fn descriptor(&self) -> &PluginDescriptor {
        self.checked
            .descriptor()
            .expect("typed single-module admission")
    }

    pub(crate) fn helper_reaped(&self) -> bool {
        self.checked.helper_reaped()
    }

    pub(crate) fn metadata_admitted(&self) -> bool {
        self.checked.metadata_available()
            && self
                .descriptor()
                .vst3_metadata
                .as_ref()
                .is_some_and(|metadata| metadata.class_uid == self.expected_uid)
    }

    pub(crate) fn evidence(&self) -> Value {
        let mut evidence = self.checked.evidence();
        evidence["expected_uid_admission_not_yet_evaluated"] = json!(false);
        evidence["expected_uid"] = json!(self.expected_uid);
        evidence["metadata_admitted_for_expected_uid"] = json!(self.metadata_admitted());
        evidence
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::{ScanStopKind, ScanStopped, Vst3ScanMetadata};

    const UID: &str = "5653544E694B386B6F6E74616B742038";

    fn synthetic() -> CheckedScan {
        let path = PathBuf::from("synthetic-empty-Kontakt.vst3");
        CheckedScan {
            requested: path.clone(),
            outcome: ScanOutcome::Completed(ScanReport {
                plugins: vec![PluginDescriptor {
                    id: "synthetic metadata only".into(),
                    name: "Synthetic Kontakt".into(),
                    vendor: "Synthetic vendor".into(),
                    path: path.clone(),
                    format: PluginFormat::Vst3,
                    category: "Instrument".into(),
                    is_instrument: true,
                    verified: true,
                    vst3_catalog: None,
                    scan_error: None,
                    vst3_metadata: Some(Vst3ScanMetadata {
                        class_uid: UID.into(),
                        category: "Instrument".into(),
                        has_midi_input: true,
                        has_midi_output: true,
                    }),
                }],
                probe_lifecycle: vec![ProbeLifecycle {
                    path,
                    helper_pid: Some(17),
                    load_failure: None,
                    reaped: true,
                    exit_success: true,
                    exit_code: Some(0),
                    kill_requested: false,
                    shutdown_failure: None,
                    kill_error: None,
                    wait_error: None,
                    pipe_error: None,
                }],
            }),
        }
    }

    fn report(scan: &mut CheckedScan) -> &mut ScanReport {
        match &mut scan.outcome {
            ScanOutcome::Completed(report) => report,
            _ => unreachable!(),
        }
    }

    #[test]
    fn missing_ambiguous_or_unbound_reap_evidence_cannot_admit_metadata() {
        for scenario in 0..5 {
            let mut scan = synthetic();
            let report = report(&mut scan);
            match scenario {
                0 => report.probe_lifecycle.clear(),
                1 => report
                    .probe_lifecycle
                    .push(report.probe_lifecycle[0].clone()),
                2 => report.probe_lifecycle[0].path = PathBuf::from("different.vst3"),
                3 => report.probe_lifecycle[0].helper_pid = None,
                4 => report.probe_lifecycle[0].helper_pid = Some(0),
                _ => unreachable!(),
            }
            assert!(!scan.helper_reaped());
            assert!(scan.admit(UID).is_err());
        }
    }

    #[test]
    fn concrete_reap_alone_cannot_substitute_healthy_metadata_retirement() {
        for scenario in 0..7 {
            let mut scan = synthetic();
            let receipt = &mut report(&mut scan).probe_lifecycle[0];
            match scenario {
                0 => receipt.reaped = false,
                1 => receipt.kill_requested = true,
                2 => {
                    receipt.exit_success = false;
                    receipt.exit_code = Some(9);
                }
                3 => receipt.shutdown_failure = Some("synthetic rejection".into()),
                4 => receipt.kill_error = Some(5),
                5 => receipt.wait_error = Some(5),
                6 => receipt.pipe_error = Some(5),
                _ => unreachable!(),
            }
            assert_eq!(scan.helper_reaped(), scenario != 0);
            assert!(scan.admit(UID).is_err());
        }
    }

    #[test]
    fn load_failure_retains_actual_healthy_cleanup_but_never_source_admission() {
        let mut scan = synthetic();
        let report = report(&mut scan);
        report.probe_lifecycle[0].load_failure = Some("synthetic LoadErr".into());
        report.plugins[0].verified = false;
        report.plugins[0].scan_error = Some("synthetic LoadErr".into());
        report.plugins[0].vst3_metadata = None;
        let evidence = scan.evidence();
        assert_eq!(evidence["scan_helper_strict_reap_proven"], true);
        assert_eq!(evidence["scan_helper_healthy_exit_proven"], true);
        assert_eq!(
            evidence["metadata_available_before_expected_uid_admission"],
            false
        );
        assert!(scan.admit(UID).is_err());
    }

    #[test]
    fn factory_catalog_cannot_substitute_loaded_default_admission() {
        let mut scan = synthetic();
        let plugin = &mut report(&mut scan).plugins[0];
        plugin.verified = false;
        plugin.vst3_metadata = None;
        plugin.category = "Unknown".into();
        plugin.is_instrument = false;
        plugin.vst3_catalog = Some(super::super::Vst3FactoryCatalog {
            factory: super::super::Vst3FactoryInfo {
                vendor: "Declared vendor".into(),
                url: String::new(),
                email: String::new(),
                flags: 0,
            },
            classes: vec![super::super::Vst3FactoryClass {
                class_id: UID.into(),
                name: "Expected declared UID is not selected default".into(),
                category: "Audio Module Class".into(),
                cardinality: i32::MAX,
                vendor: None,
                version: None,
                sdk_version: None,
                sub_categories: Some(vec!["Instrument".into()]),
                class_flags: None,
            }],
        });
        assert!(plugin.is_catalog_only());
        assert!(scan.helper_reaped() && scan.healthy_exit());
        assert!(!scan.metadata_available());
        assert!(scan.admit(UID).is_err());
    }

    #[test]
    fn stopped_scan_keeps_typed_failure_and_receipts_instead_of_empty_completion() {
        let mut scan = synthetic();
        let receipts = report(&mut scan).probe_lifecycle.clone();
        scan.outcome = ScanOutcome::Stopped(ScanStopped {
            kind: ScanStopKind::Cancelled,
            detail: "synthetic post-metadata cancellation".into(),
            probe_lifecycle: receipts.clone(),
        });
        let evidence = scan.evidence();
        assert!(scan.helper_reaped());
        assert_eq!(evidence["outcome"]["Stopped"]["kind"], "Cancelled");
        assert_eq!(
            evidence["outcome"]["Stopped"]["probe_lifecycle"],
            json!(receipts)
        );
        assert_eq!(
            evidence["metadata_available_before_expected_uid_admission"],
            false
        );
        assert!(scan.admit(UID).is_err());
    }

    #[test]
    fn descriptor_or_class_binding_failure_never_becomes_saved_source_metadata() {
        for scenario in 0..5 {
            let mut scan = synthetic();
            let report = report(&mut scan);
            match scenario {
                0 => report.plugins.clear(),
                1 => report.plugins.push(report.plugins[0].clone()),
                2 => report.plugins[0].path = PathBuf::from("different.vst3"),
                3 => {
                    report.plugins[0].vst3_metadata.as_mut().unwrap().class_uid =
                        "00112233445566778899AABBCCDDEEFF".into()
                }
                4 => report.plugins[0].format = PluginFormat::Vst2,
                _ => unreachable!(),
            }
            assert!(scan.helper_reaped());
            assert!(scan.admit(UID).is_err());
        }
    }

    #[test]
    fn accepted_scan_retains_original_outcome_and_claims_no_source_or_audio_identity() {
        let scan = synthetic();
        let before = json!(scan.outcome);
        let accepted = scan.admit(UID).unwrap();
        assert!(accepted.helper_reaped() && accepted.metadata_admitted());
        let evidence = accepted.evidence();
        assert_eq!(evidence["outcome"], before);
        assert_eq!(evidence["source_worker_identity"], Value::Null);
        assert_eq!(
            evidence["source_helper_reap_and_source_worker_join_not_proven_by_scan"],
            true
        );
        assert_eq!(evidence["metadata_admitted_for_expected_uid"], true);
    }
}
