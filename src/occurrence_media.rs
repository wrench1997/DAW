//! Dormant producer-owned occurrence publication and exact, unclamped validation.
//!
//! Publication never deletes its destination, even after cancellation or failed
//! validation. This module does not perform ordinary import geometry/resampling.
//! The file worker owns raw bytes plus decoded PCM only; its caller must release
//! generated render PCM before admitting verification.

use crate::{
    export::{
        CreateNewFailure, MAX_OCCURRENCE_MEDIA_FRAMES, PublishedFileIdentity, PublishedFloat32,
        StereoFrame, write_stereo_float32_create_new_checked,
    },
    export_job::ExportControl,
    instrument_bounce::OccurrenceRenderProvenance,
    wav::{WavAsset, WavMetadata, WavSampleEncoding},
};
use std::{
    fs::File,
    io::Read,
    panic::{AssertUnwindSafe, catch_unwind},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};

const FLOAT32_HEADER_BYTES: usize = 58;
const CHECK_SAMPLES: usize = 8192;
const READ_CHUNK_BYTES: usize = 64 * 1024;
const MAX_DIAGNOSTIC_BYTES: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MediaFailureKind {
    Cancelled,
    Invalid,
    Io,
    Identity,
    Format,
    Capacity,
    Panic,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MediaProblem {
    pub(crate) kind: MediaFailureKind,
    pub(crate) io_kind: Option<std::io::ErrorKind>,
    pub(crate) raw_os_error: Option<i32>,
    diagnostic: String,
}

impl MediaProblem {
    fn new(kind: MediaFailureKind, message: &str) -> Self {
        let mut end = message.len().min(MAX_DIAGNOSTIC_BYTES);
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        Self {
            kind,
            io_kind: None,
            raw_os_error: None,
            diagnostic: message[..end].to_owned(),
        }
    }

    fn io(error: std::io::Error) -> Self {
        let mut problem = Self::new(MediaFailureKind::Io, "Occurrence WAV file I/O failed");
        problem.io_kind = Some(error.kind());
        problem.raw_os_error = error.raw_os_error();
        problem
    }

    pub(crate) fn diagnostic(&self) -> &str {
        &self.diagnostic
    }
}

impl std::fmt::Display for MediaProblem {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.diagnostic)
    }
}

/// No field is caller writable and no public constructor can combine arbitrary
/// publication IDs with source metadata. Only publish_occurrence_render mints it.
#[derive(Debug)]
pub(crate) struct PublishedOccurrenceRender {
    provenance: OccurrenceRenderProvenance,
    publication: PublishedFloat32,
}

impl PublishedOccurrenceRender {
    pub(crate) fn provenance(&self) -> &OccurrenceRenderProvenance {
        &self.provenance
    }
    pub(crate) fn path(&self) -> &Path {
        self.publication.path()
    }
    pub(crate) fn file_identity(&self) -> PublishedFileIdentity {
        self.publication.identity()
    }
    pub(crate) fn sample_rate(&self) -> u32 {
        self.publication.sample_rate()
    }
    pub(crate) fn frames(&self) -> usize {
        self.publication.frames()
    }
    pub(crate) fn prewrite_peak(&self) -> f32 {
        self.publication.prewrite_peak()
    }
}

#[derive(Debug)]
pub(crate) struct OccurrencePublicationFailure {
    pub(crate) provenance: OccurrenceRenderProvenance,
    pub(crate) problem: Option<MediaProblem>,
    pub(crate) writer: Option<Box<CreateNewFailure>>,
}

/// Must be called on the renderer owner only after checked backend destruction.
/// Unity Float32 preserves headroom; >1 is retained on disk but cannot pass the
/// current lossless-reload validation and therefore cannot authorize insertion.
pub(crate) fn publish_occurrence_render(
    provenance: OccurrenceRenderProvenance,
    path: &Path,
    sample_rate: u32,
    stereo: &[StereoFrame],
    control: &ExportControl,
    deadline: Instant,
) -> Result<PublishedOccurrenceRender, OccurrencePublicationFailure> {
    let request = provenance.request();
    let expected = request
        .song_frames()
        .checked_add(u64::from(request.tail_frames()));
    if sample_rate != request.sample_rate()
        || expected.and_then(|frames| usize::try_from(frames).ok()) != Some(stereo.len())
        || stereo.is_empty()
        || stereo.len() > MAX_OCCURRENCE_MEDIA_FRAMES
        || provenance.identity().channel_id != request.channel_id()
        || provenance.identity().instance_id != request.instance_id()
    {
        return Err(OccurrencePublicationFailure {
            provenance,
            problem: Some(MediaProblem::new(
                MediaFailureKind::Invalid,
                "Rendered PCM does not match the captured occurrence, sample rate, or N+T extent",
            )),
            writer: None,
        });
    }
    match write_stereo_float32_create_new_checked(path, sample_rate, stereo, control, deadline) {
        Ok(publication) => Ok(PublishedOccurrenceRender {
            provenance,
            publication,
        }),
        Err(writer) => Err(OccurrencePublicationFailure {
            provenance,
            problem: None,
            writer: Some(writer),
        }),
    }
}

/// Insertion-capable media. It owns precisely one decoded PCM allocation and
/// the genuine producer receipt. Deliberately non-Clone; consumers borrow it or
/// move its PCM after preparing the transaction, never manufacture replacement IDs.
pub(crate) struct VerifiedOccurrenceMedia {
    published: PublishedOccurrenceRender,
    asset: WavAsset,
}

impl std::fmt::Debug for VerifiedOccurrenceMedia {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VerifiedOccurrenceMedia")
            .field("published", &self.published)
            .field("metadata", &self.asset.metadata)
            .field("samples", &"redacted")
            .finish()
    }
}

impl VerifiedOccurrenceMedia {
    pub(crate) fn provenance(&self) -> &OccurrenceRenderProvenance {
        self.published.provenance()
    }
    pub(crate) fn path(&self) -> &Path {
        self.published.path()
    }
    pub(crate) fn asset(&self) -> &WavAsset {
        &self.asset
    }
    pub(crate) fn prewrite_peak(&self) -> f32 {
        self.published.prewrite_peak()
    }
    pub(crate) fn file_identity(&self) -> PublishedFileIdentity {
        self.published.file_identity()
    }
    pub(crate) fn into_parts(self) -> (PublishedOccurrenceRender, WavAsset) {
        (self.published, self.asset)
    }
}

#[derive(Debug)]
pub(crate) struct MediaVerificationFailure {
    pub(crate) published: PublishedOccurrenceRender,
    pub(crate) problem: MediaProblem,
}

/// Retained path/provenance evidence for spawn or joined-worker panic failure.
/// This deliberately cannot authorize insertion or reconstruct a publication seal.
#[derive(Debug)]
pub(crate) struct RetainedMediaEvidence {
    pub(crate) provenance: OccurrenceRenderProvenance,
    pub(crate) path: std::path::PathBuf,
    pub(crate) identity: PublishedFileIdentity,
}

impl RetainedMediaEvidence {
    fn prepare(published: &PublishedOccurrenceRender) -> Result<Self, MediaProblem> {
        let path = published.path().to_str().ok_or_else(|| {
            MediaProblem::new(
                MediaFailureKind::Invalid,
                "Published occurrence path is not UTF-8",
            )
        })?;
        let mut owned = String::new();
        owned.try_reserve_exact(path.len()).map_err(|_| {
            MediaProblem::new(
                MediaFailureKind::Capacity,
                "Unable to reserve retained verification path",
            )
        })?;
        owned.push_str(path);
        Ok(Self {
            provenance: published.provenance().clone(),
            path: owned.into(),
            identity: published.file_identity(),
        })
    }
}

#[derive(Debug)]
pub(crate) enum VerificationStartFailure {
    /// The real producer seal is returned unchanged when admission is busy.
    Busy(PublishedOccurrenceRender),
    Refused {
        published: PublishedOccurrenceRender,
        problem: MediaProblem,
    },
    Spawn {
        retained: RetainedMediaEvidence,
        problem: MediaProblem,
    },
}

#[derive(Debug)]
pub(crate) enum VerificationOutcome {
    Verified(VerifiedOccurrenceMedia),
    Failed(MediaVerificationFailure),
    /// An actual finished-handle join observed a panic. No successful receipt
    /// can escape this path, including a panic after file validation completed.
    WorkerPanicked {
        retained: RetainedMediaEvidence,
        problem: MediaProblem,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VerificationStatus {
    Running,
    Joining,
    Terminated,
}

/// Cancellation-only capability. It cannot read PCM/results, release admission,
/// substitute a worker, or revoke/erase the immutable published file.
#[derive(Clone, Debug)]
pub(crate) struct VerificationCancellation(std::sync::Arc<AtomicBool>);
impl VerificationCancellation {
    pub(crate) fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
}

/// Exactly ONE verifier is admitted process-wide, including abandoned work.
/// The slot is reserved before spawn and owns the handle from spawn return until
/// actual finished-handle join. This is a narrow Stop A constraint, not a pool.
static VERIFICATION_SLOT: std::sync::Mutex<Option<VerificationSlot>> = std::sync::Mutex::new(None);

// All source/mock tests using the one global owner enter through the drain
// guard. Its serial lock stays private so a fixture cannot accidentally unlock
// while a verifier, result, open File, or PCM owner is still pending.
#[cfg(test)]
static VERIFICATION_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
#[cfg(test)]
static VERIFICATION_FIXTURE_CLEANUP_FAILURE: std::sync::atomic::AtomicU8 =
    std::sync::atomic::AtomicU8::new(0);

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FixtureDrainFailure {
    TimedOut,
    Panicked,
}

#[cfg(test)]
struct FixtureJoinedOwner {
    ticket: std::sync::Arc<()>,
    thread: std::thread::ThreadId,
}

/// Declare this FIRST in an owner fixture. Later gate-release, receipt, admission
/// and directory-retention guards then drop before its bounded final drain.
/// Directories are retained on unwind and whenever termination remains pending.
#[cfg(test)]
pub(crate) struct VerificationTestGuard {
    _serial: std::sync::MutexGuard<'static, ()>,
    joined: std::sync::Arc<std::sync::Mutex<Option<FixtureJoinedOwner>>>,
}

#[cfg(test)]
pub(crate) fn verification_test_guard() -> VerificationTestGuard {
    let serial = VERIFICATION_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    assert_eq!(
        VERIFICATION_FIXTURE_CLEANUP_FAILURE.load(Ordering::Acquire),
        0,
        "Prior verifier fixture cleanup failed; any pending slot retained and subsequent fixture admission refused"
    );
    VerificationTestGuard {
        _serial: serial,
        joined: std::sync::Arc::new(std::sync::Mutex::new(None)),
    }
}

#[cfg(test)]
pub(crate) fn verification_test_has_pending() -> bool {
    VERIFICATION_SLOT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .is_some()
}

#[cfg(test)]
impl VerificationTestGuard {
    fn drain(&self) -> Result<(), FixtureDrainFailure> {
        {
            let mut slot = VERIFICATION_SLOT
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(job) = slot.as_mut() {
                job.abandoned = true;
                job.cancel.cancel();
            }
        }
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        loop {
            // Snapshot only identity handles, never File/PCM/result ownership.
            let observed = {
                let slot = VERIFICATION_SLOT
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let Some(job) = slot.as_ref() else {
                    return Ok(());
                };
                job.handle.as_ref().map(|handle| FixtureJoinedOwner {
                    ticket: std::sync::Arc::clone(&job.ticket),
                    thread: handle.thread().id(),
                })
            };
            if let Some(outcome) = VerificationAdmission::poll_abandoned() {
                // A terminal poll has joined the exact observed handle. Result
                // ownership is destroyed here, outside both admission locks.
                drop(outcome);
                *self
                    .joined
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = observed;
            }
            let empty = VERIFICATION_SLOT
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_none();
            if empty {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(FixtureDrainFailure::TimedOut);
            }
            // No lock is held across this test-only bounded watchdog wait.
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
}

#[cfg(test)]
impl Drop for VerificationTestGuard {
    fn drop(&mut self) {
        let already_unwinding = std::thread::panicking();
        // Never replace the original assertion panic, even if cleanup itself
        // panics. No pending slot is reset and no JoinHandle is detached.
        let failure = match catch_unwind(AssertUnwindSafe(|| self.drain())) {
            Ok(Ok(())) => None,
            Ok(Err(failure)) => Some(failure),
            Err(_) => Some(FixtureDrainFailure::Panicked),
        };
        if let Some(failure) = failure {
            let code = match failure {
                FixtureDrainFailure::TimedOut => 1,
                FixtureDrainFailure::Panicked => 2,
            };
            let _ = VERIFICATION_FIXTURE_CLEANUP_FAILURE.compare_exchange(
                0,
                code,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
            // Fixed diagnostics only. Even a stderr failure is caught so Drop
            // remains non-panicking during the original fixture unwind.
            let _ = catch_unwind(AssertUnwindSafe(|| {
                eprintln!(
                    "VERIFIER_FIXTURE_CLEANUP_FAILED: {failure:?}; any pending slot retained; subsequent fixture admission refused"
                );
            }));
            if !already_unwinding {
                // Fail this fixture even if it is the final test in the binary.
                // Inner gates/owners have already dropped; there is no original
                // panic to replace on this normal-exit failure path.
                panic!(
                    "Verifier fixture cleanup failed; any pending slot retained and subsequent fixture admission refused"
                );
            }
        }
        // The private serial guard is released only after this drain/report.
    }
}

struct VerificationSlot {
    owner: std::sync::Arc<()>,
    ticket: std::sync::Arc<()>,
    cancel: VerificationCancellation,
    retained: Option<RetainedMediaEvidence>,
    handle:
        Option<std::thread::JoinHandle<Result<VerifiedOccurrenceMedia, MediaVerificationFailure>>>,
    joining: bool,
    abandoned: bool,
}

/// Retaining admission owner. Dropping it never joins, detaches, removes the
/// reserved slot, or opens another admission; it requests cancellation only.
pub(crate) struct VerificationAdmission {
    owner: std::sync::Arc<()>,
}
impl Default for VerificationAdmission {
    fn default() -> Self {
        Self {
            owner: std::sync::Arc::new(()),
        }
    }
}

#[derive(Debug)]
pub(crate) struct VerificationReceipt {
    ticket: std::sync::Arc<()>,
    cancel: VerificationCancellation,
}

impl VerificationAdmission {
    pub(crate) fn is_busy(&self) -> bool {
        VERIFICATION_SLOT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some()
    }

    #[allow(clippy::result_large_err)] // Return the exact unstarted producer owner on refusal.
    pub(crate) fn start(
        &self,
        published: PublishedOccurrenceRender,
    ) -> Result<VerificationReceipt, VerificationStartFailure> {
        self.start_with_policy(published, VerificationPolicy::default())
    }

    #[allow(clippy::result_large_err)] // Same owned refusal envelope as start.
    fn start_with_policy(
        &self,
        published: PublishedOccurrenceRender,
        policy: VerificationPolicy,
    ) -> Result<VerificationReceipt, VerificationStartFailure> {
        #[cfg(test)]
        if VERIFICATION_FIXTURE_CLEANUP_FAILURE.load(Ordering::Acquire) != 0 {
            return Err(VerificationStartFailure::Refused {
                published,
                problem: MediaProblem::new(
                    MediaFailureKind::Invalid,
                    "Prior verifier fixture cleanup failed; subsequent fixture admission refused",
                ),
            });
        }
        // Only small bounded path/proof handles are copied. No file, raw audio,
        // PCM, or captured state is cloned. Nothing here performs filesystem I/O.
        let retained = match RetainedMediaEvidence::prepare(&published) {
            Ok(retained) => retained,
            Err(problem) => return Err(VerificationStartFailure::Refused { published, problem }),
        };
        let ticket = std::sync::Arc::new(());
        let cancel = VerificationCancellation(std::sync::Arc::new(AtomicBool::new(false)));
        {
            let mut slot = VERIFICATION_SLOT
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if slot.is_some() {
                return Err(VerificationStartFailure::Busy(published));
            }
            *slot = Some(VerificationSlot {
                owner: std::sync::Arc::clone(&self.owner),
                ticket: std::sync::Arc::clone(&ticket),
                cancel: cancel.clone(),
                retained: Some(retained),
                handle: None,
                joining: false,
                abandoned: false,
            });
        }
        let worker_cancel = cancel.clone();
        #[cfg(test)]
        let fail_spawn = policy.fixture.fail_spawn;
        let work = move || {
            #[cfg(test)]
            {
                if let Some(gate) = &policy.fixture.worker_gate {
                    gate.wait();
                }
                assert!(
                    !policy.fixture.panic_worker,
                    "injected verifier worker panic"
                );
            }
            let outcome = verify_published_with_policy(published, &worker_cancel.0, &policy);
            #[cfg(test)]
            assert!(
                !policy.fixture.panic_after_outcome,
                "injected verifier panic after file destruction"
            );
            outcome
        };
        #[cfg(test)]
        let spawn = if fail_spawn {
            drop(work);
            Err(std::io::Error::other("injected verifier spawn failure"))
        } else {
            std::thread::Builder::new()
                .name("occurrence-media-verifier".into())
                .spawn(work)
        };
        #[cfg(not(test))]
        let spawn = std::thread::Builder::new()
            .name("occurrence-media-verifier".into())
            .spawn(work);
        // Reservation cannot be removed while its handle is absent. A worker
        // may already have finished; retaining its handle still precedes any ack.
        let mut slot = VERIFICATION_SLOT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match spawn {
            Ok(handle) => {
                slot.as_mut().expect("reserved verifier slot").handle = Some(handle);
                Ok(VerificationReceipt { ticket, cancel })
            }
            Err(error) => {
                let reserved = slot.take().expect("reserved verifier slot");
                let retained = reserved.retained.expect("reserved path evidence");
                drop(slot);
                Err(VerificationStartFailure::Spawn {
                    retained,
                    problem: MediaProblem::io(error),
                })
            }
        }
    }

    /// Explicit nonblocking drain for unexpected owner Drop. It only joins an
    /// already-finished handle, returns retained evidence, and never spawns a
    /// cleanup thread. While an OS read stalls, the same reservation stays busy.
    pub(crate) fn poll_abandoned() -> Option<VerificationOutcome> {
        poll_verifier(None)
    }
}

impl Drop for VerificationAdmission {
    fn drop(&mut self) {
        let mut slot = VERIFICATION_SLOT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(job) = slot
            .as_mut()
            .filter(|job| std::sync::Arc::ptr_eq(&job.owner, &self.owner))
        {
            job.abandoned = true;
            job.cancel.cancel();
        }
    }
}

impl VerificationReceipt {
    pub(crate) fn cancellation(&self) -> VerificationCancellation {
        self.cancel.clone()
    }
    pub(crate) fn cancel(&self) {
        self.cancel.cancel();
    }
    pub(crate) fn status(&self) -> VerificationStatus {
        let slot = VERIFICATION_SLOT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match slot
            .as_ref()
            .filter(|job| std::sync::Arc::ptr_eq(&job.ticket, &self.ticket))
        {
            Some(job) if job.joining => VerificationStatus::Joining,
            Some(_) => VerificationStatus::Running,
            None => VerificationStatus::Terminated,
        }
    }
    pub(crate) fn termination_acknowledged(&self) -> bool {
        self.status() == VerificationStatus::Terminated
    }
    pub(crate) fn poll(&mut self) -> Option<VerificationOutcome> {
        poll_verifier(Some(&self.ticket))
    }
}

impl Drop for VerificationReceipt {
    fn drop(&mut self) {
        let mut slot = VERIFICATION_SLOT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(job) = slot
            .as_mut()
            .filter(|job| std::sync::Arc::ptr_eq(&job.ticket, &self.ticket))
        {
            job.abandoned = true;
            job.cancel.cancel();
        }
    }
}

fn poll_verifier(ticket: Option<&std::sync::Arc<()>>) -> Option<VerificationOutcome> {
    let (handle, cancel, retained) = {
        let mut slot = VERIFICATION_SLOT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let job = slot.as_mut()?;
        if ticket.map_or(!job.abandoned, |ticket| {
            !std::sync::Arc::ptr_eq(ticket, &job.ticket)
        }) || job.joining
            || !job
                .handle
                .as_ref()
                .is_some_and(std::thread::JoinHandle::is_finished)
        {
            return None;
        }
        job.joining = true;
        (
            job.handle.take().expect("finished retained handle"),
            job.cancel.clone(),
            job.retained.take().expect("retained path evidence"),
        )
    };
    // No filesystem operation, PCM destruction, blocking wait, or worker join
    // occurs under the admission lock. is_finished was observed before taking
    // the handle, and the SAME reserved slot remains visible as Joining.
    let joined = handle.join();
    let outcome = match joined {
        Ok(Ok(verified)) if cancel.0.load(Ordering::Acquire) => {
            let (published, asset) = verified.into_parts();
            drop(asset);
            VerificationOutcome::Failed(MediaVerificationFailure {
                published,
                problem: MediaProblem::new(
                    MediaFailureKind::Cancelled,
                    "Occurrence insertion cancelled; joined verification retained the render",
                ),
            })
        }
        Ok(Ok(verified)) => VerificationOutcome::Verified(verified),
        Ok(Err(failure)) => VerificationOutcome::Failed(failure),
        Err(_) => VerificationOutcome::WorkerPanicked {
            retained,
            problem: MediaProblem::new(
                MediaFailureKind::Panic,
                "Occurrence verification worker panicked; exact handle joined and render retained",
            ),
        },
    };
    // Only an actual joined result releases global admission. Neither Drop,
    // cancellation, a result becoming available, nor elapsed time can do so.
    let mut slot = VERIFICATION_SLOT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let completed = slot.take().expect("joining verifier reservation");
    debug_assert!(completed.joining && completed.handle.is_none());
    drop(slot);
    drop(completed);
    Some(outcome)
}

fn checkpoint(cancel: &AtomicBool) -> Result<(), MediaProblem> {
    if cancel.load(Ordering::Acquire) {
        Err(MediaProblem::new(
            MediaFailureKind::Cancelled,
            "Occurrence insertion cancelled; published render retained",
        ))
    } else {
        Ok(())
    }
}

/// Test-only synchronous access to the same validation body for pure fixtures.
/// Production callers must use VerificationAdmission and observe finished join;
/// they cannot bypass retained worker ownership through this wrapper.
#[cfg(test)]
#[allow(clippy::result_large_err)] // Pure fixtures only; production must retain/join the worker owner.
pub(crate) fn verify_published_occurrence(
    published: PublishedOccurrenceRender,
    cancel: &AtomicBool,
) -> Result<VerifiedOccurrenceMedia, MediaVerificationFailure> {
    verify_published_with_policy(published, cancel, &VerificationPolicy::default())
}

#[allow(clippy::result_large_err)] // Failure retains the producer seal by value.
fn verify_published_with_policy(
    published: PublishedOccurrenceRender,
    cancel: &AtomicBool,
    policy: &VerificationPolicy,
) -> Result<VerifiedOccurrenceMedia, MediaVerificationFailure> {
    let checked = catch_unwind(AssertUnwindSafe(|| {
        verify_file(&published.publication, cancel, policy)
    }))
    .unwrap_or_else(|_| {
        Err(MediaProblem::new(
            MediaFailureKind::Panic,
            "Occurrence file verification panicked; published render retained",
        ))
    });
    match checked {
        Ok(asset) => Ok(VerifiedOccurrenceMedia { published, asset }),
        Err(problem) => Err(MediaVerificationFailure { published, problem }),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ReadIdentity {
    published: PublishedFileIdentity,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

impl ReadIdentity {
    fn inspect(metadata: &std::fs::Metadata) -> Result<Self, MediaProblem> {
        let published = PublishedFileIdentity::from_metadata(metadata).map_err(MediaProblem::io)?;
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::MetadataExt;
            Ok(Self {
                published,
                changed_seconds: metadata.ctime(),
                changed_nanoseconds: metadata.ctime_nsec(),
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = published;
            Err(MediaProblem::new(
                MediaFailureKind::Identity,
                "Linux file identity is unavailable",
            ))
        }
    }
}

fn same_path_identity(path: &Path, expected: ReadIdentity) -> Result<(), MediaProblem> {
    // symlink_metadata refuses a replacement symlink even if it points to the
    // original inode. Inode/stamps detect drift; they do not prevent a hostile
    // unrelated writer from racing or rewriting an already-open inode.
    let actual =
        ReadIdentity::inspect(&std::fs::symlink_metadata(path).map_err(MediaProblem::io)?)?;
    if actual != expected {
        return Err(MediaProblem::new(
            MediaFailureKind::Identity,
            "Published occurrence path or file identity changed",
        ));
    }
    Ok(())
}

#[derive(Default)]
struct VerificationPolicy {
    #[cfg(test)]
    fixture: VerificationFixture,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VerificationStep {
    RawReserve,
    Read,
    PcmReserve,
    Decode,
    AfterDecode,
}

#[cfg(test)]
#[derive(Default)]
struct VerificationFixture {
    fail: Option<VerificationStep>,
    panic_at: Option<VerificationStep>,
    cancel_at: Option<VerificationStep>,
    corrupt_read_sample: Option<u32>,
    replace_after_decode: bool,
    worker_gate: Option<std::sync::Arc<VerificationGate>>,
    panic_worker: bool,
    panic_after_outcome: bool,
    fail_spawn: bool,
    pause_step: Option<(VerificationStep, std::sync::Arc<VerificationGate>)>,
}

#[cfg(test)]
#[derive(Default)]
struct VerificationGate {
    entered: AtomicBool,
    released: std::sync::Mutex<bool>,
    changed: std::sync::Condvar,
}
#[cfg(test)]
impl VerificationGate {
    fn wait(&self) {
        self.entered.store(true, Ordering::Release);
        let mut released = self
            .released
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let deadline = Instant::now() + std::time::Duration::from_secs(10);
        while !*released {
            let (next, timeout) = self
                .changed
                .wait_timeout(released, deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            released = next;
            assert!(
                !timeout.timed_out() || *released,
                "verifier fixture release watchdog elapsed"
            );
        }
    }
    fn release(&self) {
        *self
            .released
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        self.changed.notify_all();
    }
}

impl VerificationPolicy {
    #[cfg(test)]
    fn step(&self, step: VerificationStep, cancel: &AtomicBool) -> Result<(), MediaProblem> {
        if let Some((at, gate)) = &self.fixture.pause_step
            && *at == step
        {
            gate.wait();
        }
        assert_ne!(
            self.fixture.panic_at,
            Some(step),
            "injected occurrence verification panic"
        );
        if self.fixture.cancel_at == Some(step) {
            cancel.store(true, Ordering::Release);
        }
        checkpoint(cancel)?;
        if self.fixture.fail == Some(step) {
            return Err(MediaProblem::new(
                match step {
                    VerificationStep::RawReserve | VerificationStep::PcmReserve => {
                        MediaFailureKind::Capacity
                    }
                    VerificationStep::Read => MediaFailureKind::Io,
                    _ => MediaFailureKind::Format,
                },
                "Injected occurrence verification failure",
            ));
        }
        Ok(())
    }
}

fn verify_file(
    published: &PublishedFloat32,
    cancel: &AtomicBool,
    _policy: &VerificationPolicy,
) -> Result<WavAsset, MediaProblem> {
    checkpoint(cancel)?;
    let frames = published.frames();
    let samples = frames.checked_mul(2).ok_or_else(|| {
        MediaProblem::new(MediaFailureKind::Capacity, "PCM sample extent overflow")
    })?;
    let bytes_len = frames
        .checked_mul(8)
        .and_then(|bytes| bytes.checked_add(FLOAT32_HEADER_BYTES))
        .ok_or_else(|| MediaProblem::new(MediaFailureKind::Capacity, "WAV byte extent overflow"))?;
    if frames == 0
        || frames > MAX_OCCURRENCE_MEDIA_FRAMES
        || published.identity().bytes != bytes_len as u64
        || !published.prewrite_peak().is_finite()
        || !(0.0..=1.0).contains(&published.prewrite_peak())
    {
        return Err(MediaProblem::new(
            MediaFailureKind::Invalid,
            "Published occurrence extent or prewrite headroom is outside the lossless reload subset",
        ));
    }
    let mut file = File::open(published.path()).map_err(MediaProblem::io)?;
    let before = ReadIdentity::inspect(&file.metadata().map_err(MediaProblem::io)?)?;
    if before.published != published.identity() {
        return Err(MediaProblem::new(
            MediaFailureKind::Identity,
            "Opened occurrence WAV differs from its producer-created identity",
        ));
    }
    same_path_identity(published.path(), before)?;
    // Ownership ledger: one bounded raw Vec and one decoded Vec. There is no
    // ordinary decoder or intermediate sample copy, and reservations are fallible.
    #[cfg(test)]
    _policy.step(VerificationStep::RawReserve, cancel)?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(bytes_len).map_err(|_| {
        MediaProblem::new(
            MediaFailureKind::Capacity,
            "Unable to reserve raw occurrence WAV bytes",
        )
    })?;
    bytes.resize(bytes_len, 0);
    for chunk in bytes.chunks_mut(READ_CHUNK_BYTES) {
        checkpoint(cancel)?;
        #[cfg(test)]
        _policy.step(VerificationStep::Read, cancel)?;
        file.read_exact(chunk).map_err(MediaProblem::io)?;
    }
    checkpoint(cancel)?;
    let mut extra = [0_u8; 1];
    match file.read(&mut extra) {
        Ok(0) => {}
        Ok(_) => {
            return Err(MediaProblem::new(
                MediaFailureKind::Format,
                "Occurrence WAV has excess bytes",
            ));
        }
        Err(error) => return Err(MediaProblem::io(error)),
    }
    validate_header(&bytes, published.sample_rate(), frames)?;
    #[cfg(test)]
    if let Some(bits) = _policy.fixture.corrupt_read_sample {
        bytes[FLOAT32_HEADER_BYTES..FLOAT32_HEADER_BYTES + 4].copy_from_slice(&bits.to_le_bytes());
    }
    #[cfg(test)]
    _policy.step(VerificationStep::PcmReserve, cancel)?;
    let mut pcm = Vec::new();
    pcm.try_reserve_exact(samples).map_err(|_| {
        MediaProblem::new(
            MediaFailureKind::Capacity,
            "Unable to reserve decoded occurrence PCM",
        )
    })?;
    let mut peak = 0.0_f32;
    let (encoded_samples, remainder) = bytes[FLOAT32_HEADER_BYTES..].as_chunks::<4>();
    if !remainder.is_empty() {
        return Err(MediaProblem::new(
            MediaFailureKind::Format,
            "Occurrence Float32 data ends in a partial sample",
        ));
    }
    for (index, sample) in encoded_samples.iter().enumerate() {
        if index % CHECK_SAMPLES == 0 {
            checkpoint(cancel)?;
            #[cfg(test)]
            _policy.step(VerificationStep::Decode, cancel)?;
        }
        let value = f32::from_le_bytes(*sample);
        if !value.is_finite() || value.abs() > 1.0 {
            return Err(MediaProblem::new(
                MediaFailureKind::Format,
                "Occurrence Float32 PCM is nonfinite or would clip on ordinary reload",
            ));
        }
        peak = peak.max(value.abs());
        pcm.push(value);
    }
    if pcm.len() != samples || peak != published.prewrite_peak() {
        return Err(MediaProblem::new(
            MediaFailureKind::Format,
            "Occurrence decoded extent or peak differs from its prewrite receipt",
        ));
    }
    #[cfg(test)]
    {
        _policy.step(VerificationStep::AfterDecode, cancel)?;
        if _policy.fixture.replace_after_decode {
            let alternate = published.path().with_extension("replacement");
            std::fs::write(&alternate, &bytes).unwrap();
            std::fs::rename(alternate, published.path()).unwrap();
        }
    }
    let after = ReadIdentity::inspect(&file.metadata().map_err(MediaProblem::io)?)?;
    if after != before {
        return Err(MediaProblem::new(
            MediaFailureKind::Identity,
            "Occurrence WAV changed during full-byte validation",
        ));
    }
    same_path_identity(published.path(), after)?;
    checkpoint(cancel)?;
    drop(file);
    drop(bytes);
    let rate = published.sample_rate();
    Ok(WavAsset {
        metadata: WavMetadata {
            sample_rate: rate,
            channels: 2,
            encoding: WavSampleEncoding::IeeeFloat,
            bits_per_sample: 32,
            valid_bits_per_sample: 32,
            channel_mask: None,
            block_align: 8,
            byte_rate: rate * 8,
            frames: frames as u64,
            duration_seconds: frames as f64 / f64::from(rate),
            data_bytes: (frames as u64) * 8,
        },
        samples: pcm,
    })
}

fn validate_header(bytes: &[u8], rate: u32, frames: usize) -> Result<(), MediaProblem> {
    let invalid = || {
        MediaProblem::new(
            MediaFailureKind::Format,
            "Occurrence WAV must have the exact producer Float32 RIFF/fmt/fact/data structure",
        )
    };
    if bytes.len() < FLOAT32_HEADER_BYTES {
        return Err(invalid());
    }
    let u16_at = |offset| u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
    let u32_at = |offset| {
        u32::from_le_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ])
    };
    let data_bytes = u32::try_from(frames)
        .ok()
        .and_then(|frames| frames.checked_mul(8))
        .ok_or_else(invalid)?;
    if &bytes[0..4] != b"RIFF"
        || u32_at(4) != data_bytes + 50
        || &bytes[8..16] != b"WAVEfmt "
        || u32_at(16) != 18
        || u16_at(20) != 3
        || u16_at(22) != 2
        || u32_at(24) != rate
        || u32_at(28) != rate * 8
        || u16_at(32) != 8
        || u16_at(34) != 32
        || u16_at(36) != 0
        || &bytes[38..42] != b"fact"
        || u32_at(42) != 4
        || u32_at(46) != frames as u32
        || &bytes[50..54] != b"data"
        || u32_at(54) != data_bytes
        || bytes.len() != FLOAT32_HEADER_BYTES + data_bytes as usize
    {
        return Err(invalid());
    }
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::{sync::atomic::AtomicU64, time::Duration};
    static DIRECTORY_ID: AtomicU64 = AtomicU64::new(1);

    fn published(name: &str, frames: &[StereoFrame]) -> (std::path::PathBuf, PublishedFloat32) {
        let sequence = DIRECTORY_ID.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "citrus-verify-{name}-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).unwrap();
        let output = write_stereo_float32_create_new_checked(
            &directory.join("output.wav"),
            8000,
            frames,
            &ExportControl::default(),
            Instant::now() + Duration::from_secs(60),
        )
        .unwrap();
        (directory, output)
    }

    #[test]
    fn actual_owned_writer_full_byte_validation_preserves_exact_pcm() {
        let frames = [StereoFrame {
            left: 0.25,
            right: -0.5,
        }; 8193];
        let (directory, output) = published("success", &frames);
        let asset = verify_file(
            &output,
            &AtomicBool::new(false),
            &VerificationPolicy::default(),
        )
        .unwrap();
        assert_eq!(asset.metadata.sample_rate, 8000);
        assert_eq!(asset.metadata.frames, 8193);
        assert_eq!(asset.metadata.channels, 2);
        assert_eq!(asset.metadata.encoding, WavSampleEncoding::IeeeFloat);
        assert_eq!(asset.samples.len(), 8193 * 2);
        let (stereo, remainder) = asset.samples.as_chunks::<2>();
        assert!(remainder.is_empty());
        for sample in stereo {
            assert_eq!(sample, &[0.25, -0.5]);
        }
        assert!(output.path().is_file());
        drop(asset);
        // PCM/cache eviction does not delete the producer's immutable media.
        assert!(output.path().is_file());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn actual_owned_writer_headroom_is_retained_but_never_insertable() {
        let (directory, output) = published(
            "headroom",
            &[StereoFrame {
                left: 1.25,
                right: -0.5,
            }],
        );
        let problem = verify_file(
            &output,
            &AtomicBool::new(false),
            &VerificationPolicy::default(),
        )
        .unwrap_err();
        assert_eq!(problem.kind, MediaFailureKind::Invalid);
        let bytes = std::fs::read(output.path()).unwrap();
        assert_eq!(f32::from_le_bytes(bytes[58..62].try_into().unwrap()), 1.25);
        assert!(output.path().is_file());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn actual_owned_identity_refuses_replacement_and_truncation() {
        for replacement in [false, true] {
            let (directory, output) = published(
                "identity",
                &[StereoFrame {
                    left: 0.25,
                    right: -0.5,
                }],
            );
            if replacement {
                let alternate = directory.join("alternate.wav");
                std::fs::write(&alternate, std::fs::read(output.path()).unwrap()).unwrap();
                std::fs::rename(alternate, output.path()).unwrap();
            } else {
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(output.path())
                    .unwrap()
                    .set_len(58)
                    .unwrap();
            }
            let problem = verify_file(
                &output,
                &AtomicBool::new(false),
                &VerificationPolicy::default(),
            )
            .unwrap_err();
            assert_eq!(problem.kind, MediaFailureKind::Identity);
            assert!(output.path().is_file());
            std::fs::remove_dir_all(directory).unwrap();
        }
    }

    #[test]
    fn canonical_parser_refuses_each_wrong_header_field_and_extent() {
        let (directory, output) = published(
            "header",
            &[StereoFrame {
                left: 0.25,
                right: -0.5,
            }],
        );
        let bytes = std::fs::read(output.path()).unwrap();
        validate_header(&bytes, 8000, 1).unwrap();
        for offset in [
            0, 4, 8, 12, 16, 20, 22, 24, 28, 32, 34, 36, 38, 42, 46, 50, 54,
        ] {
            let mut invalid = bytes.clone();
            invalid[offset] ^= 1;
            assert_eq!(
                validate_header(&invalid, 8000, 1).unwrap_err().kind,
                MediaFailureKind::Format
            );
        }
        assert!(validate_header(&bytes[..57], 8000, 1).is_err());
        assert!(validate_header(&bytes[..bytes.len() - 1], 8000, 1).is_err());
        let mut long = bytes.clone();
        long.push(0);
        assert!(validate_header(&long, 8000, 1).is_err());
        assert!(validate_header(&bytes, 16000, 1).is_err());
        assert!(validate_header(&bytes, 8000, 2).is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn raw_pcm_reserve_read_and_decode_failures_retain_published_file() {
        for step in [
            VerificationStep::RawReserve,
            VerificationStep::Read,
            VerificationStep::PcmReserve,
            VerificationStep::Decode,
            VerificationStep::AfterDecode,
        ] {
            let (directory, output) = published(
                "injected",
                &[StereoFrame {
                    left: 0.25,
                    right: -0.5,
                }; 129],
            );
            let policy = VerificationPolicy {
                fixture: VerificationFixture {
                    fail: Some(step),
                    ..VerificationFixture::default()
                },
            };
            assert!(verify_file(&output, &AtomicBool::new(false), &policy).is_err());
            assert!(output.path().is_file());
            assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
            std::fs::remove_dir_all(directory).unwrap();
        }
    }

    #[test]
    fn verification_cancellation_at_each_owned_phase_retains_file() {
        for step in [
            VerificationStep::RawReserve,
            VerificationStep::Read,
            VerificationStep::PcmReserve,
            VerificationStep::Decode,
            VerificationStep::AfterDecode,
        ] {
            let (directory, output) = published(
                "cancel",
                &[StereoFrame {
                    left: 0.25,
                    right: -0.5,
                }; 129],
            );
            let policy = VerificationPolicy {
                fixture: VerificationFixture {
                    cancel_at: Some(step),
                    ..VerificationFixture::default()
                },
            };
            let problem = verify_file(&output, &AtomicBool::new(false), &policy).unwrap_err();
            assert_eq!(problem.kind, MediaFailureKind::Cancelled);
            assert!(output.path().is_file());
            std::fs::remove_dir_all(directory).unwrap();
        }
    }

    #[test]
    fn verification_panics_destroy_local_buffers_without_deleting_file() {
        for step in [
            VerificationStep::RawReserve,
            VerificationStep::Read,
            VerificationStep::PcmReserve,
            VerificationStep::Decode,
            VerificationStep::AfterDecode,
        ] {
            let (directory, output) = published(
                "panic",
                &[StereoFrame {
                    left: 0.25,
                    right: -0.5,
                }; 129],
            );
            let policy = VerificationPolicy {
                fixture: VerificationFixture {
                    panic_at: Some(step),
                    ..VerificationFixture::default()
                },
            };
            assert!(
                catch_unwind(AssertUnwindSafe(|| verify_file(
                    &output,
                    &AtomicBool::new(false),
                    &policy
                )))
                .is_err()
            );
            assert!(output.path().is_file());
            std::fs::remove_dir_all(directory).unwrap();
        }
    }
    #[test]
    fn raw_float_decoder_rejects_nonfinite_headroom_and_changed_peak_without_clamping() {
        for sample in [
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            1.25,
            -1.25,
            0.75,
        ] {
            let (directory, output) = published(
                "bad-pcm",
                &[StereoFrame {
                    left: 0.25,
                    right: -0.5,
                }],
            );
            let policy = VerificationPolicy {
                fixture: VerificationFixture {
                    corrupt_read_sample: Some(sample.to_bits()),
                    ..VerificationFixture::default()
                },
            };
            assert_eq!(
                verify_file(&output, &AtomicBool::new(false), &policy)
                    .unwrap_err()
                    .kind,
                MediaFailureKind::Format
            );
            assert!(output.path().is_file());
            std::fs::remove_dir_all(directory).unwrap();
        }
    }

    #[test]
    fn opened_file_replacement_during_decode_refuses_sealed_output() {
        let (directory, output) = published(
            "replace-during-read",
            &[StereoFrame {
                left: 0.25,
                right: -0.5,
            }; 129],
        );
        let policy = VerificationPolicy {
            fixture: VerificationFixture {
                replace_after_decode: true,
                ..VerificationFixture::default()
            },
        };
        assert_eq!(
            verify_file(&output, &AtomicBool::new(false), &policy)
                .unwrap_err()
                .kind,
            MediaFailureKind::Identity
        );
        assert!(output.path().is_file());
        std::fs::remove_dir_all(directory).unwrap();
    }
    fn owned_occurrence(name: &str) -> (std::path::PathBuf, PublishedOccurrenceRender) {
        use crate::{
            app::clip_render::test_prepare_occurrence_request,
            instrument_bounce::occurrence::test_capture,
            model::{
                ClipKind, PluginFormat, PluginInstance, PluginRole, PluginRuntimeStatus, Project,
            },
            occurrence_request::OccurrenceStamp,
        };
        let mut project = Project {
            tempo: 120.0,
            song_length_beats: 20.0,
            ..Project::default()
        };
        project.channels.truncate(1);
        project.channels[0].instrument_plugin_instance_id = Some(41);
        project.channels[0].pan = 0.0;
        project.channels[0].volume = 0.75;
        project.plugin_instances = vec![PluginInstance {
            id: 41,
            format: PluginFormat::Vst3,
            role: PluginRole::Instrument,
            path: "fake-only.vst3".into(),
            uid: "fixture".into(),
            vendor: "Fixture".into(),
            name: "Fixture".into(),
            enabled: true,
            bypass: false,
            wet: 1.0,
            parameters: Default::default(),
            opaque_state: vec![1, 2, 3],
            runtime_status: PluginRuntimeStatus::Loaded,
            midi_ports: Default::default(),
        }];
        project.automation_lanes.clear();
        project.patterns.truncate(1);
        project.patterns[0].length_steps = 4;
        project.patterns[0].notes.clear();
        project.patterns[0].channel_steps = vec![[false; 16]];
        project.patterns[0].channel_steps[0][0] = true;
        project.clips.truncate(1);
        project.clips[0].kind = ClipKind::Pattern;
        project.clips[0].muted = false;
        project.clips[0].start = 1.0;
        project.clips[0].length = 1.0;
        project.clips[0].source_offset = 0.0;
        project.clips[0].pattern_id = project.patterns[0].id;
        let stamp = OccurrenceStamp {
            project_session: 1,
            project_revision: 2,
            timeline_revision: 3,
            epoch: 4,
            engine_session: 5,
            endpoint_id: 6,
            channel_id: project.channels[0].id,
            instance_id: 41,
            mixer_track_id: project.channels[0].mixer_track,
            native_generation: 0,
            app_native_dirty_revision: 0,
            native_capture_serial: 0,
            parameter_capture_serial: 0,
            native_control_request: 0,
            parameter_control_request: 0,
        };
        let request =
            test_prepare_occurrence_request(&project, stamp, project.clips[0].id, 8000, 0).unwrap();
        let captured = test_capture(
            &project,
            std::sync::Arc::clone(&request),
            vec![1, 2, 3],
            vec![],
        );
        let provenance = captured.occurrence_provenance().unwrap();
        let sequence = DIRECTORY_ID.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "citrus-owner-{name}-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).unwrap();
        let frames = vec![
            StereoFrame {
                left: 0.25,
                right: -0.5
            };
            request.song_frames() as usize
        ];
        let published = publish_occurrence_render(
            provenance,
            &directory.join("output.wav"),
            8000,
            &frames,
            &ExportControl::default(),
            Instant::now() + Duration::from_secs(60),
        )
        .unwrap();
        drop(frames);
        drop(captured);
        (directory, published)
    }

    struct GateRelease(std::sync::Arc<VerificationGate>);
    impl Drop for GateRelease {
        fn drop(&mut self) {
            self.0.release();
        }
    }
    fn gate() -> (std::sync::Arc<VerificationGate>, GateRelease) {
        let gate = std::sync::Arc::new(VerificationGate::default());
        (std::sync::Arc::clone(&gate), GateRelease(gate))
    }
    fn await_condition(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !condition() {
            assert!(
                Instant::now() < deadline,
                "verifier fixture did not reach its expected checkpoint"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    fn await_outcome(receipt: &mut VerificationReceipt) -> VerificationOutcome {
        let mut result = None;
        await_condition(|| {
            result = receipt.poll();
            result.is_some()
        });
        result.unwrap()
    }
    fn await_abandoned() -> VerificationOutcome {
        let mut result = None;
        await_condition(|| {
            result = VerificationAdmission::poll_abandoned();
            result.is_some()
        });
        result.unwrap()
    }
    fn finished_but_unjoined() -> bool {
        VERIFICATION_SLOT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .is_some_and(|job| {
                job.handle
                    .as_ref()
                    .is_some_and(std::thread::JoinHandle::is_finished)
            })
    }

    #[test]
    fn verifier_busy_refusal_returns_the_exact_unstarted_publication() {
        let _serial = verification_test_guard();
        let (directory, published) = owned_occurrence("busy-active");
        let (other_directory, other) = owned_occurrence("busy-refused");
        let expected_path = other.path().to_path_buf();
        let expected_provenance = other.provenance().clone();
        let owner = VerificationAdmission::default();
        let (gate, release) = gate();
        let mut receipt = owner
            .start_with_policy(
                published,
                VerificationPolicy {
                    fixture: VerificationFixture {
                        worker_gate: Some(std::sync::Arc::clone(&gate)),
                        ..VerificationFixture::default()
                    },
                },
            )
            .unwrap();
        await_condition(|| gate.entered.load(Ordering::Acquire));
        let VerificationStartFailure::Busy(returned) = owner.start(other).unwrap_err() else {
            panic!("busy admission changed category")
        };
        assert_eq!(returned.path(), expected_path);
        assert!(returned.provenance().same_capture(&expected_provenance));
        assert!(owner.is_busy());
        receipt.cancel();
        drop(release);
        assert!(
            matches!(await_outcome(&mut receipt), VerificationOutcome::Failed(failure) if failure.problem.kind == MediaFailureKind::Cancelled)
        );
        assert!(!owner.is_busy());
        assert!(returned.path().is_file());
        std::fs::remove_dir_all(directory).unwrap();
        std::fs::remove_dir_all(other_directory).unwrap();
    }

    #[test]
    fn verifier_pending_polls_retain_same_handle_ticket_and_slot() {
        let _serial = verification_test_guard();
        let (directory, published) = owned_occurrence("same-owner");
        let owner = VerificationAdmission::default();
        let (gate, release) = gate();
        let mut receipt = owner
            .start_with_policy(
                published,
                VerificationPolicy {
                    fixture: VerificationFixture {
                        worker_gate: Some(std::sync::Arc::clone(&gate)),
                        ..VerificationFixture::default()
                    },
                },
            )
            .unwrap();
        await_condition(|| gate.entered.load(Ordering::Acquire));
        let thread_id = VERIFICATION_SLOT
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .handle
            .as_ref()
            .unwrap()
            .thread()
            .id();
        let ticket = std::sync::Arc::clone(&receipt.ticket);
        for _ in 0..3 {
            assert!(receipt.poll().is_none());
            assert!(VerificationAdmission::poll_abandoned().is_none());
            assert!(!receipt.termination_acknowledged());
            let slot = VERIFICATION_SLOT.lock().unwrap();
            let job = slot.as_ref().unwrap();
            assert!(std::sync::Arc::ptr_eq(&job.ticket, &ticket));
            assert_eq!(job.handle.as_ref().unwrap().thread().id(), thread_id);
        }
        drop(release);
        assert!(matches!(
            await_outcome(&mut receipt),
            VerificationOutcome::Verified(_)
        ));
        assert!(receipt.termination_acknowledged());
        assert!(!owner.is_busy());
        assert!(receipt.poll().is_none());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn verifier_receipt_drop_keeps_exact_slot_until_abandoned_finished_join() {
        let _serial = verification_test_guard();
        let (directory, published) = owned_occurrence("receipt-drop");
        let expected_path = published.path().to_path_buf();
        let owner = VerificationAdmission::default();
        let (gate, release) = gate();
        let receipt = owner
            .start_with_policy(
                published,
                VerificationPolicy {
                    fixture: VerificationFixture {
                        worker_gate: Some(std::sync::Arc::clone(&gate)),
                        ..VerificationFixture::default()
                    },
                },
            )
            .unwrap();
        await_condition(|| gate.entered.load(Ordering::Acquire));
        let ticket = std::sync::Arc::clone(&receipt.ticket);
        let thread_id = VERIFICATION_SLOT
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .handle
            .as_ref()
            .unwrap()
            .thread()
            .id();
        drop(receipt);
        assert!(owner.is_busy());
        assert!(VerificationAdmission::poll_abandoned().is_none());
        {
            let slot = VERIFICATION_SLOT.lock().unwrap();
            let job = slot.as_ref().unwrap();
            assert!(job.abandoned && job.cancel.0.load(Ordering::Acquire));
            assert!(std::sync::Arc::ptr_eq(&job.ticket, &ticket));
            assert_eq!(job.handle.as_ref().unwrap().thread().id(), thread_id);
        }
        drop(release);
        let VerificationOutcome::Failed(failure) = await_abandoned() else {
            panic!("dropped receiver was not cancelled")
        };
        assert_eq!(failure.problem.kind, MediaFailureKind::Cancelled);
        assert_eq!(failure.published.path(), expected_path);
        assert!(!owner.is_busy());
        assert!(expected_path.is_file());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn verifier_admission_drop_cancels_but_live_receipt_can_drain_same_worker() {
        let _serial = verification_test_guard();
        let (directory, published) = owned_occurrence("admission-drop");
        let owner = VerificationAdmission::default();
        let (gate, release) = gate();
        let mut receipt = owner
            .start_with_policy(
                published,
                VerificationPolicy {
                    fixture: VerificationFixture {
                        worker_gate: Some(std::sync::Arc::clone(&gate)),
                        ..VerificationFixture::default()
                    },
                },
            )
            .unwrap();
        await_condition(|| gate.entered.load(Ordering::Acquire));
        drop(owner);
        let next_owner = VerificationAdmission::default();
        assert!(next_owner.is_busy());
        assert!(receipt.poll().is_none());
        drop(release);
        assert!(
            matches!(await_outcome(&mut receipt), VerificationOutcome::Failed(failure) if failure.problem.kind == MediaFailureKind::Cancelled)
        );
        assert!(!next_owner.is_busy());
        assert!(receipt.termination_acknowledged());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn verifier_cancel_capability_cannot_release_admission_or_erase_media() {
        let _serial = verification_test_guard();
        let (directory, published) = owned_occurrence("cancel-capability");
        let path = published.path().to_path_buf();
        let owner = VerificationAdmission::default();
        let (gate, release) = gate();
        let mut receipt = owner
            .start_with_policy(
                published,
                VerificationPolicy {
                    fixture: VerificationFixture {
                        worker_gate: Some(std::sync::Arc::clone(&gate)),
                        ..VerificationFixture::default()
                    },
                },
            )
            .unwrap();
        await_condition(|| gate.entered.load(Ordering::Acquire));
        let cancel = receipt.cancellation();
        cancel.cancel();
        drop(cancel);
        assert!(owner.is_busy());
        assert!(!receipt.termination_acknowledged());
        assert!(path.is_file());
        drop(release);
        assert!(
            matches!(await_outcome(&mut receipt), VerificationOutcome::Failed(failure) if failure.problem.kind == MediaFailureKind::Cancelled)
        );
        assert!(path.is_file());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn verifier_completion_before_poll_retains_result_and_admission_until_join() {
        let _serial = verification_test_guard();
        let (directory, published) = owned_occurrence("completed-unjoined");
        let owner = VerificationAdmission::default();
        let mut receipt = owner.start(published).unwrap();
        await_condition(finished_but_unjoined);
        assert!(owner.is_busy());
        assert_eq!(receipt.status(), VerificationStatus::Running);
        assert!(!receipt.termination_acknowledged());
        let VerificationOutcome::Verified(media) = receipt.poll().unwrap() else {
            panic!("completed verified media was lost")
        };
        assert_eq!(media.asset().samples.len(), 8000);
        assert!(receipt.termination_acknowledged());
        assert!(!owner.is_busy());
        assert!(receipt.poll().is_none());
        assert!(media.path().is_file());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn verifier_worker_panic_and_post_result_panic_join_with_retained_path() {
        let _serial = verification_test_guard();
        for after_result in [false, true] {
            let (directory, published) = owned_occurrence("worker-panic");
            let path = published.path().to_path_buf();
            let identity = published.file_identity();
            let provenance = published.provenance().clone();
            let owner = VerificationAdmission::default();
            let mut receipt = owner
                .start_with_policy(
                    published,
                    VerificationPolicy {
                        fixture: VerificationFixture {
                            panic_worker: !after_result,
                            panic_after_outcome: after_result,
                            ..VerificationFixture::default()
                        },
                    },
                )
                .unwrap();
            let VerificationOutcome::WorkerPanicked { retained, problem } =
                await_outcome(&mut receipt)
            else {
                panic!("worker panic minted a successful receipt")
            };
            assert_eq!(problem.kind, MediaFailureKind::Panic);
            assert_eq!(retained.path, path);
            assert_eq!(retained.identity, identity);
            assert!(retained.provenance.same_capture(&provenance));
            assert!(path.is_file());
            assert!(receipt.termination_acknowledged());
            assert!(!owner.is_busy());
            std::fs::remove_dir_all(directory).unwrap();
        }
    }

    #[test]
    fn verifier_completed_owner_drop_drains_then_reuses_the_single_slot() {
        let _serial = verification_test_guard();
        let (directory, published) = owned_occurrence("completed-drop");
        let owner = VerificationAdmission::default();
        let receipt = owner.start(published).unwrap();
        await_condition(finished_but_unjoined);
        drop(receipt);
        assert!(owner.is_busy());
        assert!(
            matches!(await_abandoned(), VerificationOutcome::Failed(failure) if failure.problem.kind == MediaFailureKind::Cancelled)
        );
        assert!(!owner.is_busy());
        let (next_directory, next) = owned_occurrence("slot-reused");
        let mut next_receipt = owner.start(next).unwrap();
        assert!(matches!(
            await_outcome(&mut next_receipt),
            VerificationOutcome::Verified(_)
        ));
        assert!(!owner.is_busy());
        std::fs::remove_dir_all(directory).unwrap();
        std::fs::remove_dir_all(next_directory).unwrap();
    }

    #[test]
    fn verifier_spawn_failure_keeps_path_and_releases_only_unspawned_reservation() {
        let _serial = verification_test_guard();
        let (directory, published) = owned_occurrence("spawn-failed");
        let path = published.path().to_path_buf();
        let owner = VerificationAdmission::default();
        let VerificationStartFailure::Spawn { retained, problem } = owner
            .start_with_policy(
                published,
                VerificationPolicy {
                    fixture: VerificationFixture {
                        fail_spawn: true,
                        ..VerificationFixture::default()
                    },
                },
            )
            .unwrap_err()
        else {
            panic!("spawn failure lost its category")
        };
        assert_eq!(retained.path, path);
        assert_eq!(problem.kind, MediaFailureKind::Io);
        assert!(path.is_file());
        assert!(!owner.is_busy());
        assert!(VerificationAdmission::poll_abandoned().is_none());
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn verifier_cancel_while_open_file_and_pcm_are_owned_waits_for_finished_join() {
        let _serial = verification_test_guard();
        let (directory, published) = owned_occurrence("cancel-owned-pcm");
        let path = published.path().to_path_buf();
        let owner = VerificationAdmission::default();
        let (gate, release) = gate();
        let mut receipt = owner
            .start_with_policy(
                published,
                VerificationPolicy {
                    fixture: VerificationFixture {
                        pause_step: Some((
                            VerificationStep::AfterDecode,
                            std::sync::Arc::clone(&gate),
                        )),
                        ..VerificationFixture::default()
                    },
                },
            )
            .unwrap();
        // At this checkpoint the worker owns its open File, full raw WAV Vec,
        // and completed PCM Vec. None can be dropped by UI cancellation.
        await_condition(|| gate.entered.load(Ordering::Acquire));
        let thread_id = VERIFICATION_SLOT
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .handle
            .as_ref()
            .unwrap()
            .thread()
            .id();
        receipt.cancel();
        assert!(owner.is_busy());
        assert!(receipt.poll().is_none());
        assert!(!receipt.termination_acknowledged());
        assert!(path.is_file());
        assert_eq!(
            VERIFICATION_SLOT
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .handle
                .as_ref()
                .unwrap()
                .thread()
                .id(),
            thread_id
        );
        drop(release);
        let VerificationOutcome::Failed(failure) = await_outcome(&mut receipt) else {
            panic!("cancelled decode minted a receipt")
        };
        assert_eq!(failure.problem.kind, MediaFailureKind::Cancelled);
        assert_eq!(failure.published.path(), path);
        assert!(receipt.termination_acknowledged());
        assert!(!owner.is_busy());
        assert!(path.is_file());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn verifier_caught_validation_panic_retains_producer_seal_until_join() {
        let _serial = verification_test_guard();
        let (directory, published) = owned_occurrence("caught-validation-panic");
        let path = published.path().to_path_buf();
        let owner = VerificationAdmission::default();
        let mut receipt = owner
            .start_with_policy(
                published,
                VerificationPolicy {
                    fixture: VerificationFixture {
                        panic_at: Some(VerificationStep::AfterDecode),
                        ..VerificationFixture::default()
                    },
                },
            )
            .unwrap();
        let VerificationOutcome::Failed(failure) = await_outcome(&mut receipt) else {
            panic!("caught validation panic changed worker outcome")
        };
        assert_eq!(failure.problem.kind, MediaFailureKind::Panic);
        assert_eq!(failure.published.path(), path);
        assert!(receipt.termination_acknowledged());
        assert!(!owner.is_busy());
        assert!(path.is_file());
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn verifier_assertion_unwind_releases_gate_drains_same_handle_and_preserves_panic() {
        let directory = std::sync::Mutex::new(None);
        let gate_seen = std::sync::Mutex::new(None);
        let owner_seen = std::sync::Mutex::new(None);
        let drain_seen = std::sync::Mutex::new(None);
        let assertion = catch_unwind(AssertUnwindSafe(|| {
            // First local guard, deliberately outside every inner owner/release.
            let serial = verification_test_guard();
            *drain_seen.lock().unwrap() = Some(std::sync::Arc::clone(&serial.joined));
            let (path, published) = owned_occurrence("assertion-unwind");
            *directory.lock().unwrap() = Some(path);
            let owner = VerificationAdmission::default();
            let (gate, _release) = gate();
            *gate_seen.lock().unwrap() = Some(std::sync::Arc::clone(&gate));
            let receipt = owner
                .start_with_policy(
                    published,
                    VerificationPolicy {
                        fixture: VerificationFixture {
                            pause_step: Some((
                                VerificationStep::AfterDecode,
                                std::sync::Arc::clone(&gate),
                            )),
                            ..VerificationFixture::default()
                        },
                    },
                )
                .unwrap();
            await_condition(|| gate.entered.load(Ordering::Acquire));
            let thread = VERIFICATION_SLOT
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .handle
                .as_ref()
                .unwrap()
                .thread()
                .id();
            *owner_seen.lock().unwrap() = Some((std::sync::Arc::clone(&receipt.ticket), thread));
            assert!(
                receipt.termination_acknowledged(),
                "intentional original verifier fixture assertion"
            );
        }))
        .unwrap_err();
        let original = assertion
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| assertion.downcast_ref::<String>().map(String::as_str))
            .unwrap_or("");
        assert!(original.contains("intentional original verifier fixture assertion"));
        // A new guarded admission can proceed only after the original guard's
        // drain finished. Interleaved independent fixtures remain serialized.
        let _serial = verification_test_guard();
        assert_eq!(
            VERIFICATION_FIXTURE_CLEANUP_FAILURE.load(Ordering::Acquire),
            0
        );
        let gate = gate_seen.into_inner().unwrap().unwrap();
        assert!(
            *gate
                .released
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        );
        let (ticket, thread) = owner_seen.into_inner().unwrap().unwrap();
        let drain = drain_seen.into_inner().unwrap().unwrap();
        let joined = drain.lock().unwrap();
        let joined = joined
            .as_ref()
            .expect("unwind guard joined the original verifier");
        assert!(std::sync::Arc::ptr_eq(&joined.ticket, &ticket));
        assert_eq!(joined.thread, thread);
        assert!(VERIFICATION_SLOT.lock().unwrap().is_none());
        let old_directory = directory.into_inner().unwrap().unwrap();
        assert!(old_directory.join("output.wav").is_file());
        let (next_directory, published) = owned_occurrence("after-assertion-unwind");
        let owner = VerificationAdmission::default();
        let mut receipt = owner.start(published).unwrap();
        assert!(matches!(
            await_outcome(&mut receipt),
            VerificationOutcome::Verified(_)
        ));
        assert!(receipt.termination_acknowledged());
        // Neither directory is removed until actual joined termination.
        std::fs::remove_dir_all(old_directory).unwrap();
        std::fs::remove_dir_all(next_directory).unwrap();
    }
}
