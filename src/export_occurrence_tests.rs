//! Source/mock-only no-clobber publication coverage. Never invokes a plug-in.
use super::*;
use std::time::Duration;

fn directory(name: &str) -> PathBuf {
    let id = STAGED_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let directory =
        std::env::temp_dir().join(format!("citrus-owned-{name}-{}-{id}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    directory
}

fn publish(
    path: &Path,
    fixture: WavWriteFixture,
) -> std::result::Result<PublishedFloat32, Box<CreateNewFailure>> {
    let mut policy = WavWritePolicy::checked(Instant::now() + Duration::from_secs(60));
    policy.fixture = fixture;
    write_float32_create_new_with_policy(
        path,
        8000,
        &[StereoFrame {
            left: 0.25,
            right: -0.5,
        }; 129],
        &ExportControl::default(),
        policy,
    )
}

#[cfg(target_os = "linux")]
#[test]
fn owned_float32_success_is_real_no_clobber_and_matches_ordinary_bytes() {
    let dir = directory("success");
    let path = dir.join("owned.wav");
    let published = publish(&path, WavWriteFixture::default()).unwrap();
    assert_eq!(published.path(), path);
    assert_eq!(published.frames(), 129);
    assert_eq!(published.sample_rate(), 8000);
    assert_eq!(published.prewrite_peak(), 0.5);
    assert_eq!(
        published.identity(),
        PublishedFileIdentity::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap()
    );
    let legacy = dir.join("ordinary.wav");
    write_stereo_float32_checked(
        &legacy,
        8000,
        &[StereoFrame {
            left: 0.25,
            right: -0.5,
        }; 129],
        &ExportControl::default(),
        Instant::now() + Duration::from_secs(60),
    )
    .unwrap();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        std::fs::read(&legacy).unwrap()
    );
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn owned_float32_collision_never_replaces_an_existing_destination() {
    let dir = directory("collision");
    let path = dir.join("existing.wav");
    std::fs::write(&path, b"original").unwrap();
    let failure = publish(&path, WavWriteFixture::default()).unwrap_err();
    assert_eq!(failure.evidence.phase, PublicationPhase::LinkStarted);
    assert!(failure.evidence.retained_destination.is_none());
    assert!(failure.evidence.retained_stage.is_none());
    assert_eq!(
        failure.primary.io_kind,
        Some(std::io::ErrorKind::AlreadyExists)
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"original");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn owned_float32_prelink_faults_and_panics_have_no_destination() {
    for operation in [
        StagedOperation::Write,
        StagedOperation::Flush,
        StagedOperation::Sync,
        StagedOperation::Link,
    ] {
        for panic in [false, true] {
            let dir = directory("prelink");
            let path = dir.join("output.wav");
            let failure = publish(
                &path,
                WavWriteFixture {
                    fail: (!panic).then_some(operation),
                    panic_at: panic.then_some(operation),
                    ..WavWriteFixture::default()
                },
            )
            .unwrap_err();
            assert!(!path.exists());
            assert!(failure.evidence.retained_destination.is_none());
            assert_eq!(
                failure.evidence.phase,
                if operation == StagedOperation::Link {
                    PublicationPhase::LinkStarted
                } else {
                    PublicationPhase::PreLink
                }
            );
            assert_eq!(
                failure.primary.kind,
                if panic {
                    PublicationErrorKind::Panic
                } else {
                    PublicationErrorKind::Io
                }
            );
            assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
            std::fs::remove_dir(dir).unwrap();
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn owned_float32_postlink_fault_and_panic_always_retain_destination() {
    for panic in [false, true] {
        let dir = directory("postlink");
        let path = dir.join("output.wav");
        let failure = publish(
            &path,
            WavWriteFixture {
                fail: (!panic).then_some(StagedOperation::AfterLink),
                panic_at: panic.then_some(StagedOperation::AfterLink),
                ..WavWriteFixture::default()
            },
        )
        .unwrap_err();
        assert_eq!(failure.evidence.phase, PublicationPhase::FinalizationFailed);
        assert_eq!(
            failure.evidence.retained_destination.as_deref(),
            Some(path.as_path())
        );
        assert!(failure.evidence.identity.is_some());
        assert!(failure.evidence.retained_stage.is_none());
        assert!(failure.stage_unlink.is_none());
        assert!(failure.parent_sync.is_none());
        assert_eq!(
            failure.primary.kind,
            if panic {
                PublicationErrorKind::Panic
            } else {
                PublicationErrorKind::Io
            }
        );
        assert_eq!(std::fs::read(&path).unwrap().len(), 58 + 129 * 8);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(target_os = "linux")]
#[test]
fn owned_float32_unlink_failure_still_attempts_sync_and_retains_both_errors() {
    let dir = directory("dual-finalization");
    let path = dir.join("output.wav");
    let failure = publish(
        &path,
        WavWriteFixture {
            fail_unlink: true,
            fail_parent_sync: true,
            ..WavWriteFixture::default()
        },
    )
    .unwrap_err();
    assert_eq!(failure.evidence.phase, PublicationPhase::FinalizationFailed);
    assert_eq!(
        failure.evidence.retained_destination.as_deref(),
        Some(path.as_path())
    );
    let stage = failure.evidence.retained_stage.as_ref().unwrap();
    assert!(stage.is_file());
    assert_eq!(std::fs::read(stage).unwrap(), std::fs::read(&path).unwrap());
    assert_eq!(
        failure.stage_unlink.as_ref().unwrap().operation,
        PublicationOperation::StageUnlink
    );
    assert_eq!(
        failure.parent_sync.as_ref().unwrap().operation,
        PublicationOperation::ParentSync
    );
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn owned_float32_unlink_panic_still_attempts_sync() {
    let dir = directory("unlink-panic");
    let path = dir.join("output.wav");
    let failure = publish(
        &path,
        WavWriteFixture {
            panic_at: Some(StagedOperation::Unlink),
            fail_parent_sync: true,
            ..WavWriteFixture::default()
        },
    )
    .unwrap_err();
    assert_eq!(
        failure.stage_unlink.as_ref().unwrap().kind,
        PublicationErrorKind::Panic
    );
    assert!(failure.parent_sync.is_some());
    assert!(failure.evidence.retained_stage.as_ref().unwrap().is_file());
    assert!(path.is_file());
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn owned_float32_sync_error_and_panic_retain_only_linked_destination() {
    for panic in [false, true] {
        let dir = directory("sync-failure");
        let path = dir.join("output.wav");
        let failure = publish(
            &path,
            WavWriteFixture {
                fail_parent_sync: !panic,
                panic_at: panic.then_some(StagedOperation::ParentSync),
                ..WavWriteFixture::default()
            },
        )
        .unwrap_err();
        assert!(failure.evidence.retained_stage.is_none());
        assert_eq!(
            failure.evidence.retained_destination.as_deref(),
            Some(path.as_path())
        );
        assert_eq!(
            failure.parent_sync.as_ref().unwrap().kind,
            if panic {
                PublicationErrorKind::Panic
            } else {
                PublicationErrorKind::Io
            }
        );
        assert!(path.is_file());
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(target_os = "linux")]
#[test]
fn owned_float32_prelink_abort_failure_retains_only_owned_stage() {
    for panic in [false, true] {
        let dir = directory("abort-failure");
        let path = dir.join("output.wav");
        let failure = publish(
            &path,
            WavWriteFixture {
                fail: Some(StagedOperation::Write),
                fail_abort: !panic,
                panic_at: panic.then_some(StagedOperation::Abort),
                ..WavWriteFixture::default()
            },
        )
        .unwrap_err();
        assert_eq!(failure.evidence.phase, PublicationPhase::PreLink);
        assert!(failure.evidence.retained_destination.is_none());
        assert!(failure.evidence.retained_stage.as_ref().unwrap().is_file());
        assert!(failure.abort.is_some());
        assert!(!path.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(target_os = "linux")]
#[test]
fn owned_float32_cancel_or_deadline_before_cas_cannot_publish() {
    for expiry in [false, true] {
        let dir = directory("cancel-cas");
        let path = dir.join("output.wav");
        let failure = publish(
            &path,
            WavWriteFixture {
                cancel_before_commit: !expiry,
                expire_before_commit: expiry,
                ..WavWriteFixture::default()
            },
        )
        .unwrap_err();
        assert_eq!(failure.evidence.phase, PublicationPhase::PreLink);
        assert_eq!(
            failure.primary.kind,
            if expiry {
                PublicationErrorKind::Deadline
            } else {
                PublicationErrorKind::Cancelled
            }
        );
        assert!(failure.evidence.retained_destination.is_none());
        assert!(!path.exists());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        std::fs::remove_dir(dir).unwrap();
    }
}

#[cfg(target_os = "linux")]
#[test]
fn immutable_file_cancellation_stays_closed_after_cas_with_or_without_a_link() {
    for fail_link in [false, true] {
        let dir = directory("cancel-after-cas");
        let path = dir.join("output.wav");
        let result = publish(
            &path,
            WavWriteFixture {
                fail: fail_link.then_some(StagedOperation::Link),
                cancel_after_cas: true,
                cancel_after_link: !fail_link,
                ..WavWriteFixture::default()
            },
        );
        if fail_link {
            let failure = result.unwrap_err();
            assert_eq!(failure.evidence.phase, PublicationPhase::LinkStarted);
            assert!(failure.evidence.retained_destination.is_none());
            assert!(!path.exists());
        } else {
            assert!(result.unwrap().path().is_file());
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(target_os = "linux")]
#[test]
fn immediate_postlink_panic_preserves_primary_and_both_finalization_errors() {
    let dir = directory("panic-dual-finalization");
    let path = dir.join("output.wav");
    let failure = publish(
        &path,
        WavWriteFixture {
            panic_at: Some(StagedOperation::AfterLink),
            fail_unlink: true,
            fail_parent_sync: true,
            ..WavWriteFixture::default()
        },
    )
    .unwrap_err();
    assert_eq!(failure.primary.kind, PublicationErrorKind::Panic);
    assert!(failure.stage_unlink.is_some());
    assert!(failure.parent_sync.is_some());
    assert_eq!(
        failure.evidence.retained_destination.as_deref(),
        Some(path.as_path())
    );
    assert!(failure.evidence.retained_stage.as_ref().unwrap().is_file());
    assert!(path.is_file());
    std::fs::remove_dir_all(dir).unwrap();
}
