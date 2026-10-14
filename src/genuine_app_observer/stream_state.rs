//! Bounded state sidecar foundation. Supplied identities are retained exactly, not SDK authenticated.
//! This module neither registers an observer nor changes any existing collector/deadline.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

pub const MAX_BLOB_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_BLOB_COUNT: usize = 4;
pub const MAX_OWNED_CAPACITY_BYTES: usize = MAX_BLOB_BYTES * MAX_BLOB_COUNT;
pub const MAX_ARCHIVE_BYTES: u64 = MAX_OWNED_CAPACITY_BYTES as u64 + 1024;
pub const CHUNK_BYTES: usize = 32 * 1024;
pub const MAX_PATH_BYTES: usize = 32 * 1024;
const HEADER_BYTES: u64 = 32;
const REFERENCE_BYTES: u64 = 96;
const MAX_DIAGNOSTIC_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum SuppliedPhase {
    InitialSource = 1,
    StrictClone = 2,
    RestoredSource = 3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SuppliedIdentity {
    pub phase: SuppliedPhase,
    pub worker_id: Option<u64>,
    pub source_id: Option<u64>,
    pub instance_ordinal: u64,
    pub record_ordinal: u64,
    pub monotonic_elapsed_ns: u64,
}

impl SuppliedIdentity {
    fn same_instance(self, other: Self) -> bool {
        self.phase == other.phase
            && self.worker_id == other.worker_id
            && self.source_id == other.source_id
            && self.instance_ordinal == other.instance_ordinal
    }
}

/// Original owned bytes, including their actual Vec capacity, survive failure.
/// No private byte payload is included by Debug implementations or diagnostics.
pub struct OpaqueBlob {
    pub identity: SuppliedIdentity,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct SidecarConfig {
    pub absolute_path: PathBuf,
    pub max_blob_bytes: usize,
    pub max_blobs: usize,
    pub max_owned_capacity_bytes: usize,
    pub max_archive_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckedRangeReference {
    pub identity: SuppliedIdentity,
    pub unique_blob_ordinal: u64,
    pub offset: u64,
    pub length: u64,
    pub original_owned_capacity: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FailureKind {
    Admission,
    Create,
    Spawn,
    Cancelled,
    Write,
    Flush,
    Sync,
    Length,
    Seek,
    Read,
    Compare,
    WriterPanic,
    ArchiveDropPanic,
    JoinPanic,
}

#[derive(Debug, PartialEq, Eq)]
pub struct SidecarFailure {
    pub kind: FailureKind,
    pub diagnostic: String,
}

fn bounded_prefix(text: &str, limit: usize) -> &str {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

impl SidecarFailure {
    pub fn new(kind: FailureKind, text: impl AsRef<str>) -> Self {
        // Borrow the original input. Never clone its arbitrary capacity or whole large length.
        let diagnostic = bounded_prefix(text.as_ref(), MAX_DIAGNOSTIC_BYTES)
            .to_owned()
            .into_boxed_str()
            .into_string();
        Self { kind, diagnostic }
    }
}

impl Clone for SidecarFailure {
    fn clone(&self) -> Self {
        // Public fields permit externally fabricated unbounded diagnostics; clones re-admit them.
        Self::new(self.kind.clone(), &self.diagnostic)
    }
}

fn failure(kind: FailureKind, text: impl AsRef<str>) -> SidecarFailure {
    SidecarFailure::new(kind, text)
}

struct BoundedDiagnostic {
    text: String,
    truncated: bool,
}

impl BoundedDiagnostic {
    fn new() -> Self {
        Self {
            text: String::with_capacity(MAX_DIAGNOSTIC_BYTES),
            truncated: false,
        }
    }

    fn finish(self, kind: FailureKind) -> SidecarFailure {
        // Discard the builder's spare capacity; only the <=4096-byte prefix survives.
        SidecarFailure {
            kind,
            diagnostic: self.text.into_boxed_str().into_string(),
        }
    }
}

impl std::fmt::Write for BoundedDiagnostic {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        if self.truncated {
            return Ok(());
        }
        let remaining = MAX_DIAGNOSTIC_BYTES - self.text.len();
        let prefix = bounded_prefix(text, remaining);
        self.text.push_str(prefix);
        self.truncated = prefix.len() != text.len();
        Ok(())
    }
}

fn io_failure(kind: FailureKind, operation: &str, error: io::Error) -> SidecarFailure {
    // Error Display may supply an arbitrarily large borrowed string. A bounded formatter avoids
    // constructing a temporary unbounded format! String before truncating the final diagnostic.
    let mut diagnostic = BoundedDiagnostic::new();
    let formatted = std::fmt::write(&mut diagnostic, format_args!("{operation}: {error}"));
    if formatted.is_err() {
        let _ = std::fmt::Write::write_str(&mut diagnostic, " [error display failed]");
    }
    diagnostic.finish(kind)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DurableCounts {
    pub written_bytes: u64,
    pub physically_read_bytes: u64,
    pub compared_equal_bytes: u64,
    pub original_payload_compared_bytes: u64,
    pub flush_succeeded: bool,
    pub sync_succeeded: bool,
    pub length_before_readback: Option<u64>,
    pub length_after_readback: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SidecarReceipt {
    pub absolute_path: PathBuf,
    pub references: Vec<CheckedRangeReference>,
    pub unique_blobs: usize,
    pub archive_bytes: u64,
    pub original_payload_bytes: u64,
    pub actual_writer_joined: bool,
    pub counts: DurableCounts,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Terminal {
    pub actual_writer_joined: bool,
    pub receipt: Option<SidecarReceipt>,
    pub failure: Option<SidecarFailure>,
    /// Preserve the actual worker's first cause even if caller cancellation was observed first.
    pub writer_failure: Option<SidecarFailure>,
    pub counts: DurableCounts,
}

pub enum JoinPoll<'a> {
    Pending,
    Joined(&'a Terminal),
}

/// Admission rejection is outside the admitted resource budget and returns the exact owners.
#[must_use]
pub struct StartFailure {
    pub failure: SidecarFailure,
    pub owners: Arc<Vec<OpaqueBlob>>,
}

impl std::fmt::Debug for StartFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StartFailure")
            .field("failure", &self.failure)
            .field("owner_count", &self.owners.len())
            .finish()
    }
}

struct Plan {
    references: Vec<CheckedRangeReference>,
    unique_owner_indices: Vec<usize>,
    archive_bytes: u64,
    original_payload_bytes: u64,
}

fn prepare(config: &SidecarConfig, blobs: &Vec<OpaqueBlob>) -> Result<Plan, SidecarFailure> {
    if !config.absolute_path.is_absolute()
        || config.absolute_path.as_os_str().as_encoded_bytes().len() > MAX_PATH_BYTES
        || config.absolute_path.capacity() > MAX_PATH_BYTES
        || config.max_blob_bytes == 0
        || config.max_blob_bytes > MAX_BLOB_BYTES
        || config.max_blobs == 0
        || config.max_blobs > MAX_BLOB_COUNT
        || config.max_owned_capacity_bytes == 0
        || config.max_owned_capacity_bytes > MAX_OWNED_CAPACITY_BYTES
        || config.max_archive_bytes < HEADER_BYTES
        || config.max_archive_bytes > MAX_ARCHIVE_BYTES
    {
        return Err(failure(
            FailureKind::Admission,
            "invalid finite sidecar configuration",
        ));
    }
    if blobs.is_empty() || blobs.len() > config.max_blobs || blobs.capacity() > config.max_blobs {
        return Err(failure(
            FailureKind::Admission,
            "blob list length/capacity exceeds admitted bound",
        ));
    }
    let mut total_capacity = 0usize;
    let mut previous: Option<SuppliedIdentity> = None;
    for blob in blobs {
        if blob.bytes.is_empty()
            || blob.bytes.len() > config.max_blob_bytes
            || blob.bytes.capacity() > config.max_blob_bytes
            || blob.identity.instance_ordinal == 0
            || blob.identity.record_ordinal == 0
        {
            return Err(failure(
                FailureKind::Admission,
                "blob length/capacity or supplied ordinal is invalid",
            ));
        }
        total_capacity = total_capacity
            .checked_add(blob.bytes.capacity())
            .ok_or_else(|| failure(FailureKind::Admission, "owned capacity overflow"))?;
        if total_capacity > config.max_owned_capacity_bytes {
            return Err(failure(
                FailureKind::Admission,
                "total original owned capacity exceeds declared quota",
            ));
        }
        if let Some(previous) = previous
            && (blob.identity.record_ordinal <= previous.record_ordinal
                || blob.identity.monotonic_elapsed_ns < previous.monotonic_elapsed_ns)
        {
            return Err(failure(
                FailureKind::Admission,
                "supplied records are unordered or timestamps regress",
            ));
        }
        previous = Some(blob.identity);
    }
    let mut references: Vec<CheckedRangeReference> = Vec::new();
    let mut unique_owner_indices: Vec<usize> = Vec::new();
    references
        .try_reserve_exact(config.max_blobs)
        .map_err(|_| failure(FailureKind::Admission, "reference reservation failed"))?;
    unique_owner_indices
        .try_reserve_exact(config.max_blobs)
        .map_err(|_| failure(FailureKind::Admission, "unique owner reservation failed"))?;
    let table_bytes = (blobs.len() as u64)
        .checked_mul(REFERENCE_BYTES)
        .ok_or_else(|| failure(FailureKind::Admission, "reference table overflow"))?;
    let mut next_offset = HEADER_BYTES
        .checked_add(table_bytes)
        .ok_or_else(|| failure(FailureKind::Admission, "payload offset overflow"))?;
    for (owner_index, blob) in blobs.iter().enumerate() {
        let duplicate =
            unique_owner_indices
                .iter()
                .enumerate()
                .find_map(|(unique_index, &old_index)| {
                    let old: &OpaqueBlob = &blobs[old_index];
                    (blob.identity.same_instance(old.identity) && blob.bytes == old.bytes)
                        .then_some((unique_index, old_index))
                });
        let (unique_index, offset) = if let Some((unique_index, old_index)) = duplicate {
            let old_ref = references.get(old_index).ok_or_else(|| {
                failure(
                    FailureKind::Admission,
                    "missing prior full identity binding",
                )
            })?;
            let old_ref: &CheckedRangeReference = old_ref;
            (unique_index, old_ref.offset)
        } else {
            let unique_index = unique_owner_indices.len();
            let offset = next_offset;
            next_offset = next_offset
                .checked_add(blob.bytes.len() as u64)
                .ok_or_else(|| failure(FailureKind::Admission, "opaque range overflow"))?;
            unique_owner_indices.push(owner_index);
            (unique_index, offset)
        };
        if next_offset > config.max_archive_bytes {
            return Err(failure(
                FailureKind::Admission,
                "declared physical sidecar quota exceeded",
            ));
        }
        references.push(CheckedRangeReference {
            identity: blob.identity,
            unique_blob_ordinal: unique_index as u64 + 1,
            offset,
            length: blob.bytes.len() as u64,
            original_owned_capacity: blob.bytes.capacity() as u64,
        });
    }
    let original_payload_bytes = blobs.iter().try_fold(0u64, |total, blob| {
        total.checked_add(blob.bytes.len() as u64).ok_or_else(|| {
            failure(
                FailureKind::Admission,
                "original payload byte count overflow",
            )
        })
    })?;
    Ok(Plan {
        references,
        unique_owner_indices,
        archive_bytes: next_offset,
        original_payload_bytes,
    })
}

fn header(plan: &Plan) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    bytes[..8].copy_from_slice(b"NSTATE01");
    bytes[8..12].copy_from_slice(&1u32.to_le_bytes());
    bytes[12..16].copy_from_slice(&(plan.references.len() as u32).to_le_bytes());
    bytes[16..20].copy_from_slice(&(plan.unique_owner_indices.len() as u32).to_le_bytes());
    bytes[20..24].copy_from_slice(&(REFERENCE_BYTES as u32).to_le_bytes());
    bytes[24..32].copy_from_slice(&plan.archive_bytes.to_le_bytes());
    bytes
}

fn reference_bytes(reference: &CheckedRangeReference) -> [u8; 96] {
    let mut bytes = [0u8; 96];
    bytes[..4].copy_from_slice(&(reference.identity.phase as u32).to_le_bytes());
    let fields = [
        u64::from(reference.identity.worker_id.is_some()),
        reference.identity.worker_id.unwrap_or(0),
        u64::from(reference.identity.source_id.is_some()),
        reference.identity.source_id.unwrap_or(0),
        reference.identity.instance_ordinal,
        reference.identity.record_ordinal,
        reference.identity.monotonic_elapsed_ns,
        reference.unique_blob_ordinal,
        reference.offset,
        reference.length,
        reference.original_owned_capacity,
    ];
    for (index, value) in fields.into_iter().enumerate() {
        let start = 8 + index * 8;
        bytes[start..start + 8].copy_from_slice(&value.to_le_bytes());
    }
    bytes
}

/// Tests may inject bounded sinks. Arbitrary third-party implementations have no memory contract.
pub(crate) trait Archive: Send {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize>;
    fn flush(&mut self) -> io::Result<()>;
    fn sync(&mut self) -> io::Result<()>;
    fn length(&mut self) -> io::Result<u64>;
    fn rewind(&mut self) -> io::Result<()>;
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize>;
}

struct FileArchive(File);

impl FileArchive {
    fn create_new(path: &Path) -> io::Result<Self> {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path)
            .map(Self)
    }
}

impl Archive for FileArchive {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
    fn sync(&mut self) -> io::Result<()> {
        self.0.sync_all()
    }
    fn length(&mut self) -> io::Result<u64> {
        self.0.metadata().map(|metadata| metadata.len())
    }
    fn rewind(&mut self) -> io::Result<()> {
        self.0.seek(SeekFrom::Start(0)).map(|_| ())
    }
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.0.read(bytes)
    }
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), SidecarFailure> {
    if cancel.load(Ordering::Acquire) {
        Err(failure(
            FailureKind::Cancelled,
            "state sidecar cancelled; original owners retained",
        ))
    } else {
        Ok(())
    }
}

fn write_exact(
    archive: &mut dyn Archive,
    bytes: &[u8],
    cancel: &AtomicBool,
    counts: &mut DurableCounts,
) -> Result<(), SidecarFailure> {
    let mut offset = 0;
    while offset < bytes.len() {
        check_cancel(cancel)?;
        let end = (offset + CHUNK_BYTES).min(bytes.len());
        match archive.write(&bytes[offset..end]) {
            Ok(0) => {
                return Err(failure(
                    FailureKind::Write,
                    "physical writer accepted zero bytes",
                ));
            }
            Ok(written) if written <= end - offset => {
                offset += written;
                counts.written_bytes = counts
                    .written_bytes
                    .checked_add(written as u64)
                    .ok_or_else(|| {
                        failure(FailureKind::Write, "actual written byte count overflow")
                    })?;
            }
            Ok(_) => {
                return Err(failure(
                    FailureKind::Write,
                    "writer returned more bytes than supplied",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => (),
            Err(error) => return Err(io_failure(FailureKind::Write, "physical write", error)),
        }
    }
    Ok(())
}

fn compare_exact(
    archive: &mut dyn Archive,
    expected: &[u8],
    scratch: &mut [u8; CHUNK_BYTES],
    cancel: &AtomicBool,
    counts: &mut DurableCounts,
    aliases: Option<(&[OpaqueBlob], &[CheckedRangeReference], u64)>,
) -> Result<(), SidecarFailure> {
    let mut expected_offset = 0usize;
    for expected_chunk in expected.chunks(CHUNK_BYTES) {
        let mut read = 0;
        while read < expected_chunk.len() {
            check_cancel(cancel)?;
            let target = &mut scratch[read..expected_chunk.len()];
            match archive.read(target) {
                Ok(0) => {
                    return Err(failure(
                        FailureKind::Read,
                        "sidecar readback ended before original bytes",
                    ));
                }
                Ok(n) if n <= expected_chunk.len() - read => {
                    read += n;
                    counts.physically_read_bytes = counts
                        .physically_read_bytes
                        .checked_add(n as u64)
                        .ok_or_else(|| failure(FailureKind::Read, "actual read count overflow"))?;
                }
                Ok(_) => {
                    return Err(failure(
                        FailureKind::Read,
                        "reader returned more bytes than requested",
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => (),
                Err(error) => {
                    return Err(io_failure(FailureKind::Read, "physical readback", error));
                }
            }
        }
        if &scratch[..read] != expected_chunk {
            return Err(failure(
                FailureKind::Compare,
                format!(
                    "actual readback differs from original at or after archive byte {}",
                    counts.compared_equal_bytes
                ),
            ));
        }
        if let Some((owners, references, unique_ordinal)) = aliases {
            // Compare this actual physical read against every original duplicate allocation too.
            // No original vector can mutate while the writer/recovery shares the immutable Arc.
            for (owner_index, reference) in references.iter().enumerate() {
                if reference.unique_blob_ordinal == unique_ordinal {
                    let original = owners[owner_index]
                        .bytes
                        .get(expected_offset..expected_offset + read)
                        .ok_or_else(|| {
                            failure(FailureKind::Compare, "aliased original range is incomplete")
                        })?;
                    if &scratch[..read] != original {
                        return Err(failure(
                            FailureKind::Compare,
                            "readback differs from an original aliased allocation",
                        ));
                    }
                    counts.original_payload_compared_bytes = counts
                        .original_payload_compared_bytes
                        .checked_add(read as u64)
                        .ok_or_else(|| {
                            failure(
                                FailureKind::Compare,
                                "original payload compare count overflow",
                            )
                        })?;
                }
            }
        }
        counts.compared_equal_bytes = counts
            .compared_equal_bytes
            .checked_add(read as u64)
            .ok_or_else(|| failure(FailureKind::Compare, "equal byte count overflow"))?;
        expected_offset += read;
    }
    Ok(())
}

fn write_and_compare(
    archive: &mut dyn Archive,
    owners: &[OpaqueBlob],
    plan: &Plan,
    cancel: &AtomicBool,
    counts: &mut DurableCounts,
) -> Result<(), SidecarFailure> {
    check_cancel(cancel)?;
    write_exact(archive, &header(plan), cancel, counts)?;
    for reference in &plan.references {
        write_exact(archive, &reference_bytes(reference), cancel, counts)?;
    }
    for &owner_index in &plan.unique_owner_indices {
        write_exact(archive, &owners[owner_index].bytes, cancel, counts)?;
    }
    if counts.written_bytes != plan.archive_bytes {
        return Err(failure(
            FailureKind::Length,
            "written count does not match declared complete ranges",
        ));
    }
    check_cancel(cancel)?;
    archive
        .flush()
        .map_err(|error| io_failure(FailureKind::Flush, "flush", error))?;
    counts.flush_succeeded = true;
    check_cancel(cancel)?;
    archive
        .sync()
        .map_err(|error| io_failure(FailureKind::Sync, "sync_all", error))?;
    counts.sync_succeeded = true;
    check_cancel(cancel)?;
    let before = archive
        .length()
        .map_err(|error| io_failure(FailureKind::Length, "length before readback", error))?;
    counts.length_before_readback = Some(before);
    if before != plan.archive_bytes {
        return Err(failure(
            FailureKind::Length,
            "actual file length differs before readback",
        ));
    }
    archive
        .rewind()
        .map_err(|error| io_failure(FailureKind::Seek, "rewind", error))?;
    let mut scratch = [0u8; CHUNK_BYTES];
    compare_exact(archive, &header(plan), &mut scratch, cancel, counts, None)?;
    for reference in &plan.references {
        compare_exact(
            archive,
            &reference_bytes(reference),
            &mut scratch,
            cancel,
            counts,
            None,
        )?;
    }
    for (unique_index, &owner_index) in plan.unique_owner_indices.iter().enumerate() {
        compare_exact(
            archive,
            &owners[owner_index].bytes,
            &mut scratch,
            cancel,
            counts,
            Some((owners, &plan.references, unique_index as u64 + 1)),
        )?;
    }
    check_cancel(cancel)?;
    let mut extra = [0u8; 1];
    loop {
        match archive.read(&mut extra) {
            Ok(0) => break,
            Ok(1) => {
                counts.physically_read_bytes += 1;
                return Err(failure(
                    FailureKind::Length,
                    "extra sidecar bytes follow complete ranges",
                ));
            }
            Ok(_) => {
                return Err(failure(
                    FailureKind::Read,
                    "reader returned an invalid EOF count",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => check_cancel(cancel)?,
            Err(error) => return Err(io_failure(FailureKind::Read, "readback EOF", error)),
        }
    }
    let after = archive
        .length()
        .map_err(|error| io_failure(FailureKind::Length, "length after readback", error))?;
    counts.length_after_readback = Some(after);
    check_cancel(cancel)?;
    if after != plan.archive_bytes
        || counts.physically_read_bytes != plan.archive_bytes
        || counts.compared_equal_bytes != plan.archive_bytes
        || counts.original_payload_compared_bytes != plan.original_payload_bytes
    {
        return Err(failure(
            FailureKind::Length,
            "complete actual readback counts do not match declared file",
        ));
    }
    Ok(())
}

struct WriterReport {
    failure: Option<SidecarFailure>,
    counts: DurableCounts,
}

struct Inner {
    config: SidecarConfig,
    owners: Arc<Vec<OpaqueBlob>>,
    plan: Plan,
    cancel: Arc<AtomicBool>,
    handle: Option<JoinHandle<WriterReport>>,
    terminal: Option<Terminal>,
    sticky_failure: Option<SidecarFailure>,
}

impl Inner {
    fn cancel(&mut self) {
        self.cancel.store(true, Ordering::Release);
        let first = self
            .sticky_failure
            .get_or_insert_with(|| {
                failure(
                    FailureKind::Cancelled,
                    "state sidecar cancelled; original owners retained",
                )
            })
            .clone();
        if let Some(terminal) = &mut self.terminal {
            terminal.receipt = None;
            if terminal.failure.is_none() {
                terminal.failure = Some(first);
            }
        }
    }

    fn poll_join(&mut self) -> JoinPoll<'_> {
        if self.terminal.is_none() {
            let finished = self.handle.as_ref().is_some_and(JoinHandle::is_finished);
            if !finished {
                return JoinPoll::Pending;
            }
            let handle = self
                .handle
                .take()
                .expect("finished actual writer handle is owned");
            let report = match handle.join() {
                Ok(report) => report,
                Err(_) => WriterReport {
                    failure: Some(failure(
                        FailureKind::JoinPanic,
                        "actual writer join returned panic; counts unproven",
                    )),
                    counts: DurableCounts::default(),
                },
            };
            if self.sticky_failure.is_none() {
                self.sticky_failure = report.failure.clone();
            }
            // A completed thread alone cannot certify a cancelled attempt or mismatched counts.
            if self.sticky_failure.is_none()
                && (self.cancel.load(Ordering::Acquire)
                    || !report.counts.flush_succeeded
                    || !report.counts.sync_succeeded
                    || report.counts.written_bytes != self.plan.archive_bytes
                    || report.counts.compared_equal_bytes != self.plan.archive_bytes
                    || report.counts.physically_read_bytes != self.plan.archive_bytes
                    || report.counts.original_payload_compared_bytes
                        != self.plan.original_payload_bytes
                    || report.counts.length_before_readback != Some(self.plan.archive_bytes)
                    || report.counts.length_after_readback != Some(self.plan.archive_bytes))
            {
                self.sticky_failure = Some(failure(
                    FailureKind::Length,
                    "joined writer lacks complete durable counts",
                ));
            }
            let receipt = self.sticky_failure.is_none().then(|| SidecarReceipt {
                absolute_path: self.config.absolute_path.clone(),
                references: self.plan.references.clone(),
                unique_blobs: self.plan.unique_owner_indices.len(),
                archive_bytes: self.plan.archive_bytes,
                original_payload_bytes: self.plan.original_payload_bytes,
                actual_writer_joined: true,
                counts: report.counts.clone(),
            });
            self.terminal = Some(Terminal {
                actual_writer_joined: true,
                receipt,
                failure: self.sticky_failure.clone(),
                writer_failure: report.failure,
                counts: report.counts,
            });
        }
        JoinPoll::Joined(self.terminal.as_ref().expect("actual join stored terminal"))
    }
}

pub struct StateSidecar {
    inner: Option<Inner>,
}

impl StateSidecar {
    pub fn start(config: SidecarConfig, blobs: Vec<OpaqueBlob>) -> Result<Self, StartFailure> {
        let owners = Arc::new(blobs);
        let plan = prepare(&config, &owners).map_err(|failure| StartFailure {
            failure,
            owners: Arc::clone(&owners),
        })?;
        let archive =
            FileArchive::create_new(&config.absolute_path).map_err(|error| StartFailure {
                failure: io_failure(
                    FailureKind::Create,
                    "exclusive state sidecar creation",
                    error,
                ),
                owners: Arc::clone(&owners),
            })?;
        Self::start_prepared(config, owners, plan, Box::new(archive))
    }

    #[cfg(test)]
    pub(crate) fn start_with_archive(
        config: SidecarConfig,
        blobs: Vec<OpaqueBlob>,
        archive: Box<dyn Archive>,
    ) -> Result<Self, StartFailure> {
        let owners = Arc::new(blobs);
        let plan = prepare(&config, &owners).map_err(|failure| StartFailure {
            failure,
            owners: Arc::clone(&owners),
        })?;
        Self::start_prepared(config, owners, plan, archive)
    }

    fn start_prepared(
        config: SidecarConfig,
        owners: Arc<Vec<OpaqueBlob>>,
        plan: Plan,
        archive: Box<dyn Archive>,
    ) -> Result<Self, StartFailure> {
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let worker_owners = Arc::clone(&owners);
        let worker_plan = Plan {
            references: plan.references.clone(),
            unique_owner_indices: plan.unique_owner_indices.clone(),
            archive_bytes: plan.archive_bytes,
            original_payload_bytes: plan.original_payload_bytes,
        };
        let handle = thread::Builder::new()
            .name("opaque-state-sidecar-writer".into())
            .spawn(move || {
                let mut archive = archive;
                let mut counts = DurableCounts::default();
                // Original bytes also remain owned by the producer/recovery outside this unwind.
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    write_and_compare(
                        &mut *archive,
                        &worker_owners,
                        &worker_plan,
                        &worker_cancel,
                        &mut counts,
                    )
                }));
                let mut first = match result {
                    Ok(Ok(())) => None,
                    Ok(Err(error)) => Some(error),
                    Err(_) => Some(failure(
                        FailureKind::WriterPanic,
                        "writer operation panicked; partial counts and originals retained",
                    )),
                };
                let dropped =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(archive)));
                if dropped.is_err() && first.is_none() {
                    first = Some(failure(
                        FailureKind::ArchiveDropPanic,
                        "archive destruction panicked after writer operation",
                    ));
                }
                WriterReport {
                    failure: first,
                    counts,
                }
            })
            .map_err(|error| StartFailure {
                failure: io_failure(FailureKind::Spawn, "spawn state writer", error),
                owners: Arc::clone(&owners),
            })?;
        Ok(Self {
            inner: Some(Inner {
                config,
                owners,
                plan,
                cancel,
                handle: Some(handle),
                terminal: None,
                sticky_failure: None,
            }),
        })
    }

    pub fn owners(&self) -> &[OpaqueBlob] {
        &self
            .inner
            .as_ref()
            .expect("live sidecar owns originals")
            .owners
    }

    pub fn cancel(&mut self) {
        self.inner
            .as_mut()
            .expect("live sidecar owns writer")
            .cancel();
    }

    pub fn poll_join(&mut self) -> JoinPoll<'_> {
        self.inner
            .as_mut()
            .expect("live sidecar owns writer")
            .poll_join()
    }

    pub fn into_recovery(mut self) -> SidecarRecovery {
        SidecarRecovery {
            inner: self.inner.take(),
        }
    }
}

impl Drop for StateSidecar {
    fn drop(&mut self) {
        if let Some(mut inner) = self.inner.take()
            && inner.handle.is_some()
        {
            inner.cancel();
            // No detached JoinHandle or lost opaque owner. This is quarantine, not cleanup.
            std::mem::forget(inner);
        }
    }
}

/// Explicit retained owner for cancellation/partial I/O and actual writer retirement.
pub struct SidecarRecovery {
    inner: Option<Inner>,
}

impl SidecarRecovery {
    pub fn owners(&self) -> &[OpaqueBlob] {
        &self.inner.as_ref().expect("recovery owns originals").owners
    }
    pub fn cancel(&mut self) {
        self.inner.as_mut().expect("recovery owns writer").cancel();
    }
    pub fn poll_join(&mut self) -> JoinPoll<'_> {
        self.inner
            .as_mut()
            .expect("recovery owns writer")
            .poll_join()
    }
}

impl Drop for SidecarRecovery {
    fn drop(&mut self) {
        if let Some(mut inner) = self.inner.take()
            && inner.handle.is_some()
        {
            inner.cancel();
            std::mem::forget(inner);
        }
    }
}

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
pub(super) use live::*;

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
mod live {
    use super::*;
    use crate::genuine_app_observer::{
        CaptureLabel, InstanceIdentity, InstancePhase, stream_codec as native,
        stream_control as control, stream_envelope as outer,
    };
    use crate::instrument_bounce::{BouncePluginMetadata, BouncePluginState, BounceSourceIdentity};
    use std::collections::BTreeMap;

    const MAX_BASES: usize = 4096;
    const STATE_VERSION: u16 = 1;
    const METADATA_CARRIER: &str = "Capture.state.metadata carrier";

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(in crate::genuine_app_observer) enum SlotError {
        Header,
        MissingOwner,
        OwnerMismatch,
        Bound,
        Allocation,
        Empty,
        Order,
        Incomplete,
        Linkage,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(in crate::genuine_app_observer) struct RetentionLimits {
        pub max_blob_bytes: usize,
        pub max_blobs: usize,
        pub max_owned_capacity_bytes: usize,
    }
    impl Default for RetentionLimits {
        fn default() -> Self {
            Self {
                max_blob_bytes: MAX_BLOB_BYTES,
                max_blobs: MAX_BLOB_COUNT,
                max_owned_capacity_bytes: MAX_OWNED_CAPACITY_BYTES,
            }
        }
    }
    impl RetentionLimits {
        fn validate(self) -> Result<Self, SlotError> {
            if self.max_blob_bytes == 0
                || self.max_blob_bytes > MAX_BLOB_BYTES
                || self.max_blobs == 0
                || self.max_blobs > MAX_BLOB_COUNT
                || self.max_owned_capacity_bytes == 0
                || self.max_owned_capacity_bytes > MAX_OWNED_CAPACITY_BYTES
            {
                return Err(SlotError::Bound);
            }
            Ok(self)
        }
    }

    /// The original NOBS header is immutable. owner is a *separate supplied*
    /// attribution, never a replacement for Capture's original instance=None.
    /// A matching source ID is a binding check, not SDK/registry authentication.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(in crate::genuine_app_observer) struct SlotLink {
        slot: usize,
        original_header: outer::Header,
        owner: InstanceIdentity,
        byte_len: usize,
        owned_capacity: usize,
    }
    impl SlotLink {
        pub(in crate::genuine_app_observer) fn slot(self) -> usize {
            self.slot
        }
        pub(in crate::genuine_app_observer) fn original_header(self) -> outer::Header {
            self.original_header
        }
        pub(in crate::genuine_app_observer) fn supplied_owner(self) -> InstanceIdentity {
            self.owner
        }
        pub(in crate::genuine_app_observer) fn byte_len(self) -> usize {
            self.byte_len
        }
        pub(in crate::genuine_app_observer) fn owned_capacity(self) -> usize {
            self.owned_capacity
        }
        fn supplied_identity(self) -> Result<SuppliedIdentity, SlotError> {
            Ok(SuppliedIdentity {
                phase: match self.owner.phase {
                    InstancePhase::InitialSource => SuppliedPhase::InitialSource,
                    InstancePhase::StrictClone => SuppliedPhase::StrictClone,
                    InstancePhase::UndoSource => SuppliedPhase::RestoredSource,
                },
                worker_id: self
                    .owner
                    .worker_id
                    .map(u64::try_from)
                    .transpose()
                    .map_err(|_| SlotError::Bound)?,
                source_id: self.owner.source_instance_id,
                instance_ordinal: self.owner.ordinal,
                record_ordinal: self.original_header.sequence,
                monotonic_elapsed_ns: self.original_header.monotonic_ns,
            })
        }
    }

    fn supplied_owner(
        header: outer::Header,
        attribution: Option<InstanceIdentity>,
    ) -> Result<InstanceIdentity, SlotError> {
        if header.lane != outer::Lane::State
            || !matches!(header.kind, 2 | 9)
            || header.sequence == 0
            || (header.kind == 9 && header.instance.is_some())
        {
            return Err(SlotError::Header);
        }
        let owner = match (header.instance, attribution) {
            (Some(original), None) => original,
            (Some(original), Some(supplied)) if original == supplied => original,
            (Some(_), Some(_)) => return Err(SlotError::OwnerMismatch),
            (None, Some(supplied)) => supplied,
            (None, None) => return Err(SlotError::MissingOwner),
        };
        if owner.ordinal == 0 {
            return Err(SlotError::MissingOwner);
        }
        Ok(owner)
    }

    struct RetainedSlot {
        link: SlotLink,
        blob: OpaqueBlob,
    }
    /// One-way, fixed four-slot admission. No replacement, release/retry loop,
    /// hash, full-byte deduplication or I/O occurs in a tap. The old observer's
    /// lifetime copy/record limits remain the live owner's separate obligation.
    pub(in crate::genuine_app_observer) struct LiveStateSlots {
        limits: RetentionLimits,
        slots: [Option<RetainedSlot>; MAX_BLOB_COUNT],
        count: usize,
        owned_capacity: usize,
        admitted_bytes: usize,
    }
    impl LiveStateSlots {
        pub(in crate::genuine_app_observer) fn new(
            limits: RetentionLimits,
        ) -> Result<Self, SlotError> {
            Ok(Self {
                limits: limits.validate()?,
                slots: std::array::from_fn(|_| None),
                count: 0,
                owned_capacity: 0,
                admitted_bytes: 0,
            })
        }
        pub(in crate::genuine_app_observer) fn len(&self) -> usize {
            self.count
        }
        pub(in crate::genuine_app_observer) fn is_empty(&self) -> bool {
            self.count == 0
        }
        pub(in crate::genuine_app_observer) fn owned_capacity(&self) -> usize {
            self.owned_capacity
        }
        pub(in crate::genuine_app_observer) fn admitted_bytes(&self) -> usize {
            self.admitted_bytes
        }
        pub(in crate::genuine_app_observer) fn opaque(
            &self,
            link: SlotLink,
        ) -> Result<&[u8], SlotError> {
            let slot = self
                .slots
                .get(link.slot)
                .and_then(Option::as_ref)
                .ok_or(SlotError::Linkage)?;
            if slot.link != link {
                return Err(SlotError::Linkage);
            }
            Ok(&slot.blob.bytes)
        }
        fn admit(
            &self,
            header: outer::Header,
            attribution: Option<InstanceIdentity>,
            len: usize,
            cap: usize,
        ) -> Result<SlotLink, SlotError> {
            let owner = supplied_owner(header, attribution)?;
            if self.count >= self.limits.max_blobs
                || len == 0
                || len > cap
                || cap > self.limits.max_blob_bytes
                || self
                    .owned_capacity
                    .checked_add(cap)
                    .ok_or(SlotError::Bound)?
                    > self.limits.max_owned_capacity_bytes
            {
                return Err(SlotError::Bound);
            }
            if let Some(previous) = self
                .count
                .checked_sub(1)
                .and_then(|index| self.slots[index].as_ref())
                && (header.sequence <= previous.link.original_header.sequence
                    || header.monotonic_ns < previous.link.original_header.monotonic_ns)
            {
                return Err(SlotError::Order);
            }
            let link = SlotLink {
                slot: self.count,
                original_header: header,
                owner,
                byte_len: len,
                owned_capacity: cap,
            };
            link.supplied_identity()?;
            Ok(link)
        }
        fn store(&mut self, link: SlotLink, bytes: Vec<u8>) {
            self.owned_capacity += bytes.capacity();
            self.admitted_bytes += bytes.len();
            self.slots[self.count] = Some(RetainedSlot {
                link,
                blob: OpaqueBlob {
                    identity: link
                        .supplied_identity()
                        .expect("admitted fixed-width identity"),
                    bytes,
                },
            });
            self.count += 1;
        }
        /// Consumes the original allocation exactly; a rejection returns its
        /// unchanged Vec, including its pointer, length, capacity and raw bits.
        pub(in crate::genuine_app_observer) fn retain_owned(
            &mut self,
            header: outer::Header,
            attribution: Option<InstanceIdentity>,
            bytes: Vec<u8>,
        ) -> Result<SlotLink, OwnedAdmissionFailure> {
            let link = match self.admit(header, attribution, bytes.len(), bytes.capacity()) {
                Ok(link) => link,
                Err(error) => return Err(OwnedAdmissionFailure { error, bytes }),
            };
            self.store(link, bytes);
            Ok(link)
        }
        /// A borrowed slice has no capacity evidence for the caller's external
        /// allocation. Clone its actual bytes once, then admit and retain this
        /// new Vec's actual capacity. No opaque normalization or hash is used.
        pub(in crate::genuine_app_observer) fn retain_borrowed(
            &mut self,
            header: outer::Header,
            attribution: Option<InstanceIdentity>,
            bytes: &[u8],
        ) -> Result<SlotLink, SlotError> {
            self.admit(header, attribution, bytes.len(), bytes.len())?;
            let mut owned = Vec::new();
            owned
                .try_reserve_exact(bytes.len())
                .map_err(|_| SlotError::Allocation)?;
            let link = self.admit(header, attribution, bytes.len(), owned.capacity())?;
            owned.extend_from_slice(bytes);
            self.store(link, owned);
            Ok(link)
        }
        pub(in crate::genuine_app_observer) fn retain_state_load<'a>(
            &mut self,
            header: outer::Header,
            attribution: Option<InstanceIdentity>,
            bytes: &[u8],
            outcome: control::OutcomeRef<'a>,
        ) -> Result<StateView<'a>, SlotError> {
            if header.kind != 2 {
                return Err(SlotError::Header);
            }
            let link = self.retain_borrowed(header, attribution, bytes)?;
            Ok(StateView::StateLoad { link, outcome })
        }
        pub(in crate::genuine_app_observer) fn retain_capture<'a>(
            &mut self,
            header: outer::Header,
            attribution: InstanceIdentity,
            capture: CaptureOperands<'a>,
        ) -> Result<StateView<'a>, SlotError> {
            if header.kind != 9
                || attribution.source_instance_id != Some(capture.identity.instance_id)
            {
                return Err(SlotError::OwnerMismatch);
            }
            let link =
                self.retain_borrowed(header, Some(attribution), &capture.state.opaque_state)?;
            capture_view(
                link,
                capture.label,
                capture.identity,
                capture.receipt_identity,
                capture.state,
            )
        }
        /// Call only after the live App has actually retired its source/clone
        /// owners. This transfer proves retention only, never native cleanup.
        /// Reservation failure returns all four original slots unchanged.
        #[allow(clippy::result_large_err)] // Bounded fixed owners must survive allocation failure without another allocation.
        pub(in crate::genuine_app_observer) fn into_batch(
            mut self,
        ) -> Result<LiveStateBatch, SealFailure> {
            if self.count == 0 {
                return Err(SealFailure {
                    error: SlotError::Empty,
                    slots: self,
                });
            }
            let mut blobs = Vec::new();
            if blobs.try_reserve_exact(self.count).is_err() || blobs.capacity() > MAX_BLOB_COUNT {
                return Err(SealFailure {
                    error: SlotError::Allocation,
                    slots: self,
                });
            }
            let mut links = [None; MAX_BLOB_COUNT];
            for (index, slot) in self.slots.iter_mut().enumerate().take(self.count) {
                let retained = slot.take().expect("contiguous admitted slots");
                links[index] = Some(retained.link);
                blobs.push(retained.blob);
            }
            Ok(LiveStateBatch {
                blobs,
                links,
                count: self.count,
            })
        }
    }

    pub(in crate::genuine_app_observer) struct OwnedAdmissionFailure {
        pub error: SlotError,
        pub bytes: Vec<u8>,
    }
    impl std::fmt::Debug for OwnedAdmissionFailure {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("OwnedAdmissionFailure")
                .field("error", &self.error)
                .field("length", &self.bytes.len())
                .field("capacity", &self.bytes.capacity())
                .finish()
        }
    }
    pub(in crate::genuine_app_observer) struct SealFailure {
        pub error: SlotError,
        pub slots: LiveStateSlots,
    }
    impl std::fmt::Debug for SealFailure {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("SealFailure")
                .field("error", &self.error)
                .field("slots", &self.slots.len())
                .finish()
        }
    }
    pub(in crate::genuine_app_observer) struct LiveStateBatch {
        blobs: Vec<OpaqueBlob>,
        links: [Option<SlotLink>; MAX_BLOB_COUNT],
        count: usize,
    }
    impl LiveStateBatch {
        #[allow(clippy::result_large_err)] // A failed spawn/create retains the four original immutable links without boxing.
        pub(in crate::genuine_app_observer) fn start(
            self,
            config: SidecarConfig,
        ) -> Result<LiveStateSidecar, LiveStartFailure> {
            let Self {
                blobs,
                links,
                count,
            } = self;
            match StateSidecar::start(config, blobs) {
                Ok(sidecar) => Ok(LiveStateSidecar {
                    sidecar,
                    links,
                    count,
                }),
                Err(failure) => Err(LiveStartFailure {
                    failure,
                    links,
                    count,
                }),
            }
        }
    }
    pub(in crate::genuine_app_observer) struct LiveStartFailure {
        pub failure: StartFailure,
        pub links: [Option<SlotLink>; MAX_BLOB_COUNT],
        pub count: usize,
    }
    impl std::fmt::Debug for LiveStartFailure {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("LiveStartFailure")
                .field("failure", &self.failure)
                .field("count", &self.count)
                .finish()
        }
    }
    pub(in crate::genuine_app_observer) struct LinkedRanges<'a> {
        pub links: &'a [Option<SlotLink>; MAX_BLOB_COUNT],
        pub ranges: &'a [CheckedRangeReference],
    }
    fn link_receipt<'a>(
        links: &'a [Option<SlotLink>; MAX_BLOB_COUNT],
        count: usize,
        terminal: &'a Terminal,
    ) -> Result<LinkedRanges<'a>, SlotError> {
        if !terminal.actual_writer_joined
            || terminal.failure.is_some()
            || terminal.writer_failure.is_some()
        {
            return Err(SlotError::Incomplete);
        }
        let receipt = terminal.receipt.as_ref().ok_or(SlotError::Incomplete)?;
        if !receipt.actual_writer_joined
            || count == 0
            || count > MAX_BLOB_COUNT
            || receipt.references.len() != count
        {
            return Err(SlotError::Linkage);
        }
        for (index, reference) in receipt.references.iter().enumerate() {
            let link = links[index].ok_or(SlotError::Linkage)?;
            if link.slot != index
                || reference.identity != link.supplied_identity()?
                || reference.length != link.byte_len as u64
                || reference.original_owned_capacity != link.owned_capacity as u64
            {
                return Err(SlotError::Linkage);
            }
        }
        Ok(LinkedRanges {
            links,
            ranges: &receipt.references,
        })
    }
    pub(in crate::genuine_app_observer) enum LiveJoinPoll<'a> {
        Pending,
        Joined {
            terminal: &'a Terminal,
            linkage: Result<LinkedRanges<'a>, SlotError>,
        },
    }
    pub(in crate::genuine_app_observer) struct LiveStateSidecar {
        sidecar: StateSidecar,
        links: [Option<SlotLink>; MAX_BLOB_COUNT],
        count: usize,
    }
    impl LiveStateSidecar {
        pub(in crate::genuine_app_observer) fn owners(&self) -> &[OpaqueBlob] {
            self.sidecar.owners()
        }
        pub(in crate::genuine_app_observer) fn cancel(&mut self) {
            self.sidecar.cancel();
        }
        pub(in crate::genuine_app_observer) fn poll_join(&mut self) -> LiveJoinPoll<'_> {
            match self.sidecar.poll_join() {
                JoinPoll::Pending => LiveJoinPoll::Pending,
                JoinPoll::Joined(terminal) => LiveJoinPoll::Joined {
                    terminal,
                    linkage: link_receipt(&self.links, self.count, terminal),
                },
            }
        }
        pub(in crate::genuine_app_observer) fn into_recovery(self) -> LiveRecovery {
            LiveRecovery {
                sidecar: self.sidecar.into_recovery(),
                links: self.links,
                count: self.count,
            }
        }
    }
    pub(in crate::genuine_app_observer) struct LiveRecovery {
        sidecar: SidecarRecovery,
        links: [Option<SlotLink>; MAX_BLOB_COUNT],
        count: usize,
    }
    impl LiveRecovery {
        pub(in crate::genuine_app_observer) fn owners(&self) -> &[OpaqueBlob] {
            self.sidecar.owners()
        }
        pub(in crate::genuine_app_observer) fn cancel(&mut self) {
            self.sidecar.cancel();
        }
        pub(in crate::genuine_app_observer) fn poll_join(&mut self) -> LiveJoinPoll<'_> {
            match self.sidecar.poll_join() {
                JoinPoll::Pending => LiveJoinPoll::Pending,
                JoinPoll::Joined(terminal) => LiveJoinPoll::Joined {
                    terminal,
                    linkage: link_receipt(&self.links, self.count, terminal),
                },
            }
        }
    }

    /// Complete native references, including the actual outcome and all capture
    /// fields. Opaque bytes are linked to one retained slot, never put in 64KiB
    /// operands. Passing this view does not authenticate a source or receipt.
    pub(in crate::genuine_app_observer) enum StateView<'a> {
        StateLoad {
            link: SlotLink,
            outcome: control::OutcomeRef<'a>,
        },
        Capture {
            link: SlotLink,
            label: CaptureLabel,
            identity: BounceSourceIdentity,
            receipt_identity: usize,
            metadata: &'a BouncePluginMetadata,
            parameter_bases: &'a BTreeMap<u32, f32>,
            native_generation: u64,
            native_dirty_revision: u64,
        },
    }
    pub(in crate::genuine_app_observer) struct CaptureOperands<'a> {
        pub label: CaptureLabel,
        pub identity: BounceSourceIdentity,
        pub receipt_identity: usize,
        pub state: &'a BouncePluginState,
    }
    impl StateView<'_> {
        fn link(&self) -> SlotLink {
            match self {
                Self::StateLoad { link, .. } | Self::Capture { link, .. } => *link,
            }
        }
    }
    pub(in crate::genuine_app_observer) fn capture_view<'a>(
        link: SlotLink,
        label: CaptureLabel,
        identity: BounceSourceIdentity,
        receipt_identity: usize,
        state: &'a BouncePluginState,
    ) -> Result<StateView<'a>, SlotError> {
        if link.original_header.kind != 9
            || link.original_header.instance.is_some()
            || link.owner.source_instance_id != Some(identity.instance_id)
            || link.byte_len != state.opaque_state.len()
        {
            return Err(SlotError::OwnerMismatch);
        }
        Ok(StateView::Capture {
            link,
            label,
            identity,
            receipt_identity,
            metadata: &state.metadata,
            parameter_bases: &state.parameter_bases,
            native_generation: state.native_generation,
            native_dirty_revision: state.native_dirty_revision,
        })
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub(in crate::genuine_app_observer) enum BodyError {
        Slot(SlotError),
        Bound,
        Capacity,
        Occupied,
        Header,
        Truncated,
        MissingDictionary,
        UnknownCode,
        Trailing,
        Control(control::Error),
        Native(native::CodecError),
        Envelope(outer::Error),
    }
    struct Prefix {
        bytes: [u8; 256],
        len: usize,
    }
    impl Prefix {
        fn new() -> Self {
            Self {
                bytes: [0; 256],
                len: 0,
            }
        }
        fn put(&mut self, bytes: &[u8]) -> Result<(), BodyError> {
            let end = self.len.checked_add(bytes.len()).ok_or(BodyError::Bound)?;
            self.bytes
                .get_mut(self.len..end)
                .ok_or(BodyError::Bound)?
                .copy_from_slice(bytes);
            self.len = end;
            Ok(())
        }
        fn u8(&mut self, value: u8) -> Result<(), BodyError> {
            self.put(&[value])
        }
        fn u16(&mut self, value: u16) -> Result<(), BodyError> {
            self.put(&value.to_le_bytes())
        }
        fn u32(&mut self, value: u32) -> Result<(), BodyError> {
            self.put(&value.to_le_bytes())
        }
        fn u64(&mut self, value: u64) -> Result<(), BodyError> {
            self.put(&value.to_le_bytes())
        }
        fn option(&mut self, value: Option<u64>) -> Result<(), BodyError> {
            self.u8(u8::from(value.is_some()))?;
            if let Some(value) = value {
                self.u64(value)?;
            }
            Ok(())
        }
        fn link(&mut self, link: SlotLink) -> Result<(), BodyError> {
            self.put(b"NSTA")?;
            self.u16(STATE_VERSION)?;
            self.u16(link.original_header.kind)?;
            self.u8(link.slot as u8)?;
            self.u64(link.original_header.sequence)?;
            self.u64(link.original_header.monotonic_ns)?;
            self.u64(link.byte_len as u64)?;
            self.u64(link.owned_capacity as u64)?;
            self.u64(link.owner.ordinal)?;
            self.u8(match link.owner.phase {
                InstancePhase::InitialSource => 1,
                InstancePhase::StrictClone => 2,
                InstancePhase::UndoSource => 3,
            })?;
            self.option(
                link.owner
                    .worker_id
                    .map(u64::try_from)
                    .transpose()
                    .map_err(|_| BodyError::Bound)?,
            )?;
            self.option(link.owner.source_instance_id)
        }
        fn prepend(&self, destination: &mut Vec<u8>, budget: usize) -> Result<(), BodyError> {
            let total = destination
                .len()
                .checked_add(self.len)
                .ok_or(BodyError::Bound)?;
            if total > budget {
                return Err(BodyError::Bound);
            }
            if total > destination.capacity() {
                return Err(BodyError::Capacity);
            }
            let old = destination.len();
            destination.resize(total, 0);
            destination.copy_within(..old, self.len);
            destination[..self.len].copy_from_slice(&self.bytes[..self.len]);
            Ok(())
        }
    }
    pub(in crate::genuine_app_observer) fn encode_body(
        view: StateView<'_>,
        dictionary: Option<&native::BaselineDictionary>,
        destination: &mut Vec<u8>,
        total_body_budget: usize,
    ) -> Result<(), BodyError> {
        if !destination.is_empty() {
            return Err(BodyError::Occupied);
        }
        if total_body_budget > outer::MAX_BODY {
            return Err(BodyError::Bound);
        }
        if destination.capacity() > outer::MAX_OPERAND {
            return Err(BodyError::Capacity);
        }
        let result = (|| {
            let link = view.link();
            supplied_owner(link.original_header, Some(link.owner)).map_err(BodyError::Slot)?;
            let mut prefix = Prefix::new();
            prefix.link(link)?;
            match view {
                StateView::StateLoad { outcome, .. } => {
                    if link.original_header.kind != 2 {
                        return Err(BodyError::Header);
                    }
                    control::encode_outcome_only(
                        outcome,
                        destination,
                        total_body_budget
                            .checked_sub(prefix.len)
                            .ok_or(BodyError::Bound)?,
                    )
                    .map_err(BodyError::Control)?;
                    prefix.prepend(destination, total_body_budget)
                }
                StateView::Capture {
                    label,
                    identity,
                    receipt_identity,
                    metadata,
                    parameter_bases,
                    native_generation,
                    native_dirty_revision,
                    ..
                } => {
                    if link.original_header.kind != 9
                        || link.owner.source_instance_id != Some(identity.instance_id)
                    {
                        return Err(BodyError::Header);
                    }
                    if parameter_bases.len() > MAX_BASES {
                        return Err(BodyError::Bound);
                    }
                    prefix.u8(match label {
                        CaptureLabel::Initial => 1,
                        CaptureLabel::Final => 2,
                    })?;
                    for field in [
                        identity.project_session,
                        identity.project_fingerprint,
                        identity.timeline_revision,
                        identity.epoch,
                        identity.endpoint_id,
                        u64::from(identity.channel_id),
                        identity.instance_id,
                    ] {
                        prefix.u64(field)?;
                    }
                    prefix.u64(u64::try_from(receipt_identity).map_err(|_| BodyError::Bound)?)?;
                    prefix.u64(native_generation)?;
                    prefix.u64(native_dirty_revision)?;
                    prefix.u32(parameter_bases.len() as u32)?;
                    let metadata_length_offset = prefix.len;
                    prefix.u32(0)?;
                    let bases_bytes = parameter_bases
                        .len()
                        .checked_mul(8)
                        .ok_or(BodyError::Bound)?;
                    let limit = total_body_budget
                        .checked_sub(prefix.len)
                        .and_then(|budget| budget.checked_sub(bases_bytes))
                        .ok_or(BodyError::Bound)?;
                    let tag = native::encode_native_body_with_limit(
                        native::NativeView::Metadata {
                            site: METADATA_CARRIER,
                            result: Ok(metadata),
                        },
                        dictionary.ok_or(BodyError::MissingDictionary)?,
                        destination,
                        limit,
                    )
                    .map_err(BodyError::Native)?;
                    if tag != 5 {
                        return Err(BodyError::Header);
                    }
                    let length = u32::try_from(destination.len()).map_err(|_| BodyError::Bound)?;
                    prefix.bytes[metadata_length_offset..metadata_length_offset + 4]
                        .copy_from_slice(&length.to_le_bytes());
                    prefix.prepend(destination, total_body_budget)?;
                    let total = destination
                        .len()
                        .checked_add(bases_bytes)
                        .ok_or(BodyError::Bound)?;
                    if total > total_body_budget {
                        return Err(BodyError::Bound);
                    }
                    if total > destination.capacity() {
                        return Err(BodyError::Capacity);
                    }
                    for (&id, &value) in parameter_bases {
                        destination.extend_from_slice(&id.to_le_bytes());
                        destination.extend_from_slice(&value.to_bits().to_le_bytes());
                    }
                    Ok(())
                }
            }
        })();
        if result.is_err() {
            destination.clear();
        }
        result
    }
    pub(in crate::genuine_app_observer) fn encode_enveloped(
        view: StateView<'_>,
        dictionary: Option<&native::BaselineDictionary>,
        destination: &mut Vec<u8>,
    ) -> Result<(), BodyError> {
        let header = view.link().original_header;
        encode_body(view, dictionary, destination, outer::MAX_BODY)?;
        if let Err(error) = outer::wrap(header, destination) {
            destination.clear();
            return Err(BodyError::Envelope(error));
        }
        Ok(())
    }

    struct Reader<'a> {
        bytes: &'a [u8],
        offset: usize,
    }
    impl<'a> Reader<'a> {
        fn take(&mut self, len: usize) -> Result<&'a [u8], BodyError> {
            let end = self.offset.checked_add(len).ok_or(BodyError::Bound)?;
            let bytes = self
                .bytes
                .get(self.offset..end)
                .ok_or(BodyError::Truncated)?;
            self.offset = end;
            Ok(bytes)
        }
        fn u8(&mut self) -> Result<u8, BodyError> {
            Ok(self.take(1)?[0])
        }
        fn u16(&mut self) -> Result<u16, BodyError> {
            Ok(u16::from_le_bytes(
                self.take(2)?.try_into().map_err(|_| BodyError::Truncated)?,
            ))
        }
        fn u32(&mut self) -> Result<u32, BodyError> {
            Ok(u32::from_le_bytes(
                self.take(4)?.try_into().map_err(|_| BodyError::Truncated)?,
            ))
        }
        fn u64(&mut self) -> Result<u64, BodyError> {
            Ok(u64::from_le_bytes(
                self.take(8)?.try_into().map_err(|_| BodyError::Truncated)?,
            ))
        }
        fn usize(&mut self) -> Result<usize, BodyError> {
            usize::try_from(self.u64()?).map_err(|_| BodyError::Bound)
        }
        fn option(&mut self) -> Result<Option<u64>, BodyError> {
            match self.u8()? {
                0 => Ok(None),
                1 => self.u64().map(Some),
                _ => Err(BodyError::UnknownCode),
            }
        }
        fn link(&mut self, header: outer::Header) -> Result<SlotLink, BodyError> {
            if self.take(4)? != b"NSTA"
                || self.u16()? != STATE_VERSION
                || self.u16()? != header.kind
            {
                return Err(BodyError::Header);
            }
            let slot = self.u8()? as usize;
            let sequence = self.u64()?;
            let monotonic_ns = self.u64()?;
            let byte_len = self.usize()?;
            let owned_capacity = self.usize()?;
            let ordinal = self.u64()?;
            let phase = match self.u8()? {
                1 => InstancePhase::InitialSource,
                2 => InstancePhase::StrictClone,
                3 => InstancePhase::UndoSource,
                _ => return Err(BodyError::UnknownCode),
            };
            let worker_id = self
                .option()?
                .map(usize::try_from)
                .transpose()
                .map_err(|_| BodyError::Bound)?;
            let source_instance_id = self.option()?;
            let owner = InstanceIdentity {
                ordinal,
                phase,
                worker_id,
                source_instance_id,
            };
            supplied_owner(header, Some(owner)).map_err(BodyError::Slot)?;
            if slot >= MAX_BLOB_COUNT
                || sequence != header.sequence
                || monotonic_ns != header.monotonic_ns
                || byte_len == 0
                || byte_len > owned_capacity
                || owned_capacity > MAX_BLOB_BYTES
            {
                return Err(BodyError::Header);
            }
            Ok(SlotLink {
                slot,
                original_header: header,
                owner,
                byte_len,
                owned_capacity,
            })
        }
    }
    #[derive(Clone, Copy, Debug)]
    pub(in crate::genuine_app_observer) struct ParameterBases<'a> {
        bytes: &'a [u8],
        count: usize,
    }
    impl<'a> ParameterBases<'a> {
        pub(in crate::genuine_app_observer) fn len(self) -> usize {
            self.count
        }
        pub(in crate::genuine_app_observer) fn is_empty(self) -> bool {
            self.count == 0
        }
        pub(in crate::genuine_app_observer) fn items(
            self,
        ) -> impl Iterator<Item = (u32, u32)> + 'a {
            self.bytes.as_chunks::<8>().0.iter().map(|pair| {
                (
                    u32::from_le_bytes(pair[..4].try_into().expect("validated pair")),
                    u32::from_le_bytes(pair[4..].try_into().expect("validated pair")),
                )
            })
        }
    }
    pub(in crate::genuine_app_observer) enum StateBody<'a> {
        StateLoad {
            link: SlotLink,
            outcome: control::Outcome<'a>,
        },
        Capture {
            link: SlotLink,
            label: CaptureLabel,
            identity: BounceSourceIdentity,
            receipt_identity: usize,
            metadata: native::BorrowedMetadata<'a>,
            parameter_bases: ParameterBases<'a>,
            native_generation: u64,
            native_dirty_revision: u64,
        },
    }
    pub(in crate::genuine_app_observer) fn decode_body<'a>(
        header: outer::Header,
        bytes: &'a [u8],
        dictionary: Option<&'a native::BaselineDictionary>,
    ) -> Result<StateBody<'a>, BodyError> {
        if bytes.len() > outer::MAX_BODY {
            return Err(BodyError::Bound);
        }
        let mut r = Reader { bytes, offset: 0 };
        let link = r.link(header)?;
        let body = match header.kind {
            2 => {
                let outcome = control::decode_outcome_only(r.take(bytes.len() - r.offset)?)
                    .map_err(BodyError::Control)?;
                StateBody::StateLoad { link, outcome }
            }
            9 => {
                let label = match r.u8()? {
                    1 => CaptureLabel::Initial,
                    2 => CaptureLabel::Final,
                    _ => return Err(BodyError::UnknownCode),
                };
                let identity = BounceSourceIdentity {
                    project_session: r.u64()?,
                    project_fingerprint: r.u64()?,
                    timeline_revision: r.u64()?,
                    epoch: r.u64()?,
                    endpoint_id: r.u64()?,
                    channel_id: u32::try_from(r.u64()?).map_err(|_| BodyError::Bound)?,
                    instance_id: r.u64()?,
                };
                if link.owner.source_instance_id != Some(identity.instance_id) {
                    return Err(BodyError::Header);
                }
                let receipt_identity = r.usize()?;
                let native_generation = r.u64()?;
                let native_dirty_revision = r.u64()?;
                let count = r.u32()? as usize;
                if count > MAX_BASES {
                    return Err(BodyError::Bound);
                }
                let metadata_length = r.u32()? as usize;
                let native::NativeBody::Metadata {
                    site,
                    result: Ok(metadata),
                    ..
                } = native::decode_native_body_with_limit(
                    5,
                    r.take(metadata_length)?,
                    dictionary.ok_or(BodyError::MissingDictionary)?,
                    outer::MAX_BODY,
                )
                .map_err(BodyError::Native)?
                else {
                    return Err(BodyError::Header);
                };
                if site != METADATA_CARRIER {
                    return Err(BodyError::Header);
                }
                let parameter_bases = ParameterBases {
                    bytes: r.take(count.checked_mul(8).ok_or(BodyError::Bound)?)?,
                    count,
                };
                let mut previous = None;
                for (id, _) in parameter_bases.items() {
                    if previous.is_some_and(|old| id <= old) {
                        return Err(BodyError::Header);
                    }
                    previous = Some(id);
                }
                StateBody::Capture {
                    link,
                    label,
                    identity,
                    receipt_identity,
                    metadata,
                    parameter_bases,
                    native_generation,
                    native_dirty_revision,
                }
            }
            _ => return Err(BodyError::Header),
        };
        if r.offset != bytes.len() {
            return Err(BodyError::Trailing);
        }
        Ok(body)
    }
    pub(in crate::genuine_app_observer) fn decode_enveloped<'a>(
        bytes: &'a [u8],
        dictionary: Option<&'a native::BaselineDictionary>,
    ) -> Result<(outer::Header, StateBody<'a>), BodyError> {
        let (header, body) = outer::decode(bytes).map_err(BodyError::Envelope)?;
        Ok((header, decode_body(header, body, dictionary)?))
    }
}

#[cfg(test)]
#[path = "stream_state_tests.rs"]
mod tests;
