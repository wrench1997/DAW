use super::*;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::{Duration, Instant};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct FixtureDirectory {
    directory: PathBuf,
    file: PathBuf,
}

impl FixtureDirectory {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "citrus-state-sidecar-fixture-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).expect("exclusive new fixture directory");
        let file = directory.join("opaque-state.bin");
        Self { directory, file }
    }
}

impl Drop for FixtureDirectory {
    fn drop(&mut self) {
        // These exact paths were exclusively created by this fixture; no recursive deletion.
        let _ = fs::remove_file(&self.file);
        let _ = fs::remove_dir(&self.directory);
    }
}

fn config(path: PathBuf) -> SidecarConfig {
    SidecarConfig {
        absolute_path: path,
        max_blob_bytes: MAX_BLOB_BYTES,
        max_blobs: MAX_BLOB_COUNT,
        max_owned_capacity_bytes: MAX_OWNED_CAPACITY_BYTES,
        max_archive_bytes: MAX_ARCHIVE_BYTES,
    }
}

fn identity(record_ordinal: u64) -> SuppliedIdentity {
    SuppliedIdentity {
        phase: SuppliedPhase::InitialSource,
        worker_id: Some(123),
        source_id: Some(0),
        instance_ordinal: 7,
        record_ordinal,
        monotonic_elapsed_ns: record_ordinal * 17,
    }
}

fn blob(record_ordinal: u64, bytes: Vec<u8>) -> OpaqueBlob {
    OpaqueBlob {
        identity: identity(record_ordinal),
        bytes,
    }
}

fn joined(session: &mut StateSidecar) -> Terminal {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match session.poll_join() {
            JoinPoll::Joined(terminal) => return terminal.clone(),
            JoinPoll::Pending => {
                assert!(
                    Instant::now() < deadline,
                    "actual writer JoinHandle did not finish"
                );
                std::thread::yield_now();
            }
        }
    }
}

fn recovery_joined(recovery: &mut SidecarRecovery) -> Terminal {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match recovery.poll_join() {
            JoinPoll::Joined(terminal) => return terminal.clone(),
            JoinPoll::Pending => {
                assert!(
                    Instant::now() < deadline,
                    "actual recovered writer JoinHandle did not finish"
                );
                std::thread::yield_now();
            }
        }
    }
}

fn read_range_exact(path: &PathBuf, offset: u64, original: &[u8]) {
    let mut file = File::open(path).unwrap();
    file.seek(SeekFrom::Start(offset)).unwrap();
    let mut scratch = [0u8; CHUNK_BYTES];
    for expected in original.chunks(CHUNK_BYTES) {
        file.read_exact(&mut scratch[..expected.len()]).unwrap();
        assert_eq!(&scratch[..expected.len()], expected);
    }
}

#[test]
fn saved13_sized_actual_file_is_chunked_lossless_and_really_joined() {
    let fixture = FixtureDirectory::new();
    let bytes: Vec<u8> = (0..591_772)
        .map(|index| ((index * 31 + index / 257) % 256) as u8)
        .collect();
    let mut session =
        StateSidecar::start(config(fixture.file.clone()), vec![blob(2, bytes)]).unwrap();
    let terminal = joined(&mut session);
    assert!(terminal.actual_writer_joined);
    assert_eq!(terminal.failure, None);
    let receipt = terminal.receipt.unwrap();
    assert_eq!(receipt.references.len(), 1);
    assert_eq!(receipt.archive_bytes, 32 + 96 + 591_772);
    assert_eq!(receipt.counts.written_bytes, receipt.archive_bytes);
    assert_eq!(receipt.counts.compared_equal_bytes, receipt.archive_bytes);
    assert_eq!(receipt.counts.physically_read_bytes, receipt.archive_bytes);
    assert_eq!(receipt.original_payload_bytes, 591_772);
    assert_eq!(receipt.counts.original_payload_compared_bytes, 591_772);
    assert!(receipt.counts.flush_succeeded && receipt.counts.sync_succeeded);
    assert_eq!(
        receipt.counts.length_after_readback,
        Some(receipt.archive_bytes)
    );
    assert_eq!(receipt.references[0].identity, identity(2));
    read_range_exact(
        &fixture.file,
        receipt.references[0].offset,
        &session.owners()[0].bytes,
    );
}

#[test]
fn ieee_signed_zero_nan_and_infinity_are_archived_as_opaque_original_bits() {
    let fixture = FixtureDirectory::new();
    let bits = [
        0u32,
        0x8000_0000,
        0x7f80_0000,
        0xff80_0000,
        0x7fc0_1234,
        0x7f80_4321,
    ];
    let bytes: Vec<u8> = bits.into_iter().flat_map(u32::to_le_bytes).collect();
    let mut session =
        StateSidecar::start(config(fixture.file.clone()), vec![blob(1, bytes)]).unwrap();
    let receipt = joined(&mut session).receipt.unwrap();
    read_range_exact(
        &fixture.file,
        receipt.references[0].offset,
        &session.owners()[0].bytes,
    );
}

#[test]
fn duplicate_range_requires_all_original_bytes_and_full_instance_binding() {
    let fixture = FixtureDirectory::new();
    let original = vec![9, 3, 0, 0xff, 17];
    let mut changed = original.clone();
    changed[3] ^= 1;
    let mut different_instance = blob(3, original.clone());
    different_instance.identity.worker_id = None;
    let mut session = StateSidecar::start(
        config(fixture.file.clone()),
        vec![
            blob(1, original.clone()),
            blob(2, original),
            different_instance,
            blob(4, changed),
        ],
    )
    .unwrap();
    let receipt = joined(&mut session).receipt.unwrap();
    assert_eq!(receipt.unique_blobs, 3);
    assert_eq!(receipt.original_payload_bytes, 20);
    assert_eq!(receipt.counts.original_payload_compared_bytes, 20);
    assert_eq!(receipt.references[0].offset, receipt.references[1].offset);
    assert_ne!(
        receipt.references[0].identity,
        receipt.references[1].identity
    );
    assert_eq!(receipt.references[1].identity.record_ordinal, 2);
    assert_ne!(receipt.references[0].offset, receipt.references[2].offset);
    assert_ne!(receipt.references[0].offset, receipt.references[3].offset);
    for (reference, owner) in receipt.references.iter().zip(session.owners()) {
        read_range_exact(&fixture.file, reference.offset, &owner.bytes);
        assert_eq!(reference.identity, owner.identity);
        assert!(reference.offset.checked_add(reference.length).unwrap() <= receipt.archive_bytes);
    }
}

#[test]
fn exclusive_create_does_not_overwrite_existing_evidence() {
    let fixture = FixtureDirectory::new();
    let mut existing = File::create(&fixture.file).unwrap();
    existing.write_all(b"existing evidence").unwrap();
    existing.sync_all().unwrap();
    drop(existing);
    let rejected = StateSidecar::start(config(fixture.file.clone()), vec![blob(1, vec![8, 9])])
        .err()
        .expect("CreateNew must reject collision");
    assert_eq!(rejected.failure.kind, FailureKind::Create);
    assert_eq!(rejected.owners[0].bytes, [8, 9]);
    read_range_exact(&fixture.file, 0, b"existing evidence");
}

#[test]
fn relative_path_is_rejected_with_the_original_owner() {
    let bytes = vec![0, 1, 2, 3];
    let pointer = bytes.as_ptr();
    let rejected = StateSidecar::start(config(PathBuf::from("relative.bin")), vec![blob(1, bytes)])
        .err()
        .expect("relative path rejected");
    assert_eq!(rejected.failure.kind, FailureKind::Admission);
    assert_eq!(rejected.owners[0].bytes.as_ptr(), pointer);
}

#[test]
fn original_length_and_reserved_capacity_are_independently_bounded() {
    let fixture = FixtureDirectory::new();
    let mut bounded = config(fixture.file.clone());
    bounded.max_blob_bytes = 32;
    let mut bytes = Vec::with_capacity(33);
    bytes.push(7);
    let capacity = bytes.capacity();
    let pointer = bytes.as_ptr();
    let rejected = StateSidecar::start(bounded.clone(), vec![blob(1, bytes)])
        .err()
        .unwrap();
    assert_eq!(rejected.failure.kind, FailureKind::Admission);
    assert_eq!(rejected.owners[0].bytes.capacity(), capacity);
    assert_eq!(rejected.owners[0].bytes.as_ptr(), pointer);
    let rejected = StateSidecar::start(bounded, vec![blob(1, vec![1; 33])])
        .err()
        .unwrap();
    assert_eq!(rejected.owners[0].bytes.len(), 33);
    assert!(!fixture.file.exists());
}

#[test]
fn count_total_capacity_archive_quota_and_actual_record_order_reject_before_creation() {
    let fixture = FixtureDirectory::new();
    let mut bounded = config(fixture.file.clone());
    bounded.max_blobs = 1;
    let rejected = StateSidecar::start(bounded, vec![blob(1, vec![1]), blob(2, vec![2])])
        .err()
        .unwrap();
    assert_eq!(rejected.owners.len(), 2);
    let mut bounded = config(fixture.file.clone());
    bounded.max_owned_capacity_bytes = 3;
    let rejected = StateSidecar::start(bounded, vec![blob(1, vec![1; 4])])
        .err()
        .unwrap();
    assert_eq!(rejected.owners[0].bytes, [1; 4]);
    let mut bounded = config(fixture.file.clone());
    bounded.max_archive_bytes = 128;
    let rejected = StateSidecar::start(bounded, vec![blob(1, vec![8])])
        .err()
        .unwrap();
    assert_eq!(rejected.failure.kind, FailureKind::Admission);
    let rejected = StateSidecar::start(
        config(fixture.file.clone()),
        vec![blob(2, vec![1]), blob(1, vec![1])],
    )
    .err()
    .unwrap();
    assert_eq!(rejected.failure.kind, FailureKind::Admission);
    let mut regressed = blob(3, vec![1]);
    regressed.identity.monotonic_elapsed_ns = 0;
    let rejected = StateSidecar::start(
        config(fixture.file.clone()),
        vec![blob(2, vec![1]), regressed],
    )
    .err()
    .unwrap();
    assert_eq!(rejected.failure.kind, FailureKind::Admission);
    assert!(!fixture.file.exists());
}

#[derive(Clone, Copy, Debug)]
enum Fault {
    None,
    WriteAt(usize),
    PanicAt(usize),
    Flush,
    Sync,
    Length,
    Rewind,
    ReadAt(usize),
    TruncatedAt(usize),
    CorruptAt(usize),
    Extra,
    DropPanic,
    InterruptedOnce,
}

/// Fixed 1024-byte synthetic storage; no unlimited memory sink is admitted.
struct BoundedArchive {
    bytes: [u8; 1024],
    length: usize,
    position: usize,
    max_write: usize,
    fault: Fault,
    interrupted_write: bool,
    interrupted_read: bool,
    extra_returned: bool,
}

impl BoundedArchive {
    fn new(fault: Fault) -> Self {
        Self {
            bytes: [0; 1024],
            length: 0,
            position: 0,
            max_write: 3,
            fault,
            interrupted_write: false,
            interrupted_read: false,
            extra_returned: false,
        }
    }
}

fn synthetic_error() -> io::Error {
    io::Error::other("synthetic bounded archive fault")
}

impl Archive for BoundedArchive {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if matches!(self.fault, Fault::InterruptedOnce) && !self.interrupted_write {
            self.interrupted_write = true;
            return Err(io::ErrorKind::Interrupted.into());
        }
        let mut count = bytes.len().min(self.max_write);
        match self.fault {
            Fault::WriteAt(at) if self.length >= at => return Err(synthetic_error()),
            Fault::PanicAt(at) if self.length >= at => {
                panic!("synthetic actual writer operation panic")
            }
            Fault::WriteAt(at) | Fault::PanicAt(at) => count = count.min(at - self.length),
            _ => (),
        }
        if self.length + count > self.bytes.len() {
            return Err(synthetic_error());
        }
        self.bytes[self.length..self.length + count].copy_from_slice(&bytes[..count]);
        self.length += count;
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        if matches!(self.fault, Fault::Flush) {
            Err(synthetic_error())
        } else {
            Ok(())
        }
    }
    fn sync(&mut self) -> io::Result<()> {
        if matches!(self.fault, Fault::Sync) {
            Err(synthetic_error())
        } else {
            Ok(())
        }
    }
    fn length(&mut self) -> io::Result<u64> {
        Ok(self.length as u64 + u64::from(matches!(self.fault, Fault::Length)))
    }
    fn rewind(&mut self) -> io::Result<()> {
        if matches!(self.fault, Fault::Rewind) {
            return Err(synthetic_error());
        }
        self.position = 0;
        Ok(())
    }
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if matches!(self.fault, Fault::InterruptedOnce) && !self.interrupted_read {
            self.interrupted_read = true;
            return Err(io::ErrorKind::Interrupted.into());
        }
        match self.fault {
            Fault::ReadAt(at) if self.position >= at => return Err(synthetic_error()),
            Fault::TruncatedAt(at) if self.position >= at => return Ok(0),
            _ => (),
        }
        if self.position == self.length {
            if matches!(self.fault, Fault::Extra) && !self.extra_returned {
                self.extra_returned = true;
                bytes[0] = 0xaa;
                return Ok(1);
            }
            return Ok(0);
        }
        let mut count = bytes.len().min(5).min(self.length - self.position);
        if let Fault::ReadAt(at) | Fault::TruncatedAt(at) = self.fault {
            count = count.min(at - self.position);
        }
        bytes[..count].copy_from_slice(&self.bytes[self.position..self.position + count]);
        if let Fault::CorruptAt(at) = self.fault
            && at >= self.position
            && at < self.position + count
        {
            bytes[at - self.position] ^= 1;
        }
        self.position += count;
        Ok(count)
    }
}

impl Drop for BoundedArchive {
    fn drop(&mut self) {
        if matches!(self.fault, Fault::DropPanic) {
            panic!("synthetic archive Drop panic");
        }
    }
}

fn synthetic_session(fault: Fault) -> (FixtureDirectory, StateSidecar) {
    let fixture = FixtureDirectory::new();
    let session = StateSidecar::start_with_archive(
        config(fixture.file.clone()),
        vec![blob(1, vec![19, 20, 0, 255])],
        Box::new(BoundedArchive::new(fault)),
    )
    .unwrap();
    (fixture, session)
}

#[test]
fn short_writes_and_interrupted_reads_count_only_actual_accepted_bytes() {
    for fault in [Fault::None, Fault::InterruptedOnce] {
        let (_fixture, mut session) = synthetic_session(fault);
        let terminal = joined(&mut session);
        let receipt = terminal.receipt.unwrap();
        assert_eq!(receipt.archive_bytes, 132);
        assert_eq!(receipt.counts.written_bytes, 132);
        assert_eq!(receipt.counts.physically_read_bytes, 132);
        assert_eq!(receipt.counts.compared_equal_bytes, 132);
    }
}

#[test]
fn partial_io_failure_keeps_exact_original_owner_and_actual_written_prefix() {
    let (_fixture, mut session) = synthetic_session(Fault::WriteAt(7));
    let pointer = session.owners()[0].bytes.as_ptr();
    let terminal = joined(&mut session);
    assert_eq!(terminal.failure.unwrap().kind, FailureKind::Write);
    assert!(terminal.actual_writer_joined && terminal.receipt.is_none());
    assert_eq!(terminal.counts.written_bytes, 7);
    assert_eq!(terminal.counts.physically_read_bytes, 0);
    assert!(!terminal.counts.flush_succeeded);
    assert_eq!(session.owners()[0].bytes, [19, 20, 0, 255]);
    assert_eq!(session.owners()[0].bytes.as_ptr(), pointer);
}

struct PartialFileArchive {
    file: File,
    accepted: u64,
    fail_after: u64,
}

impl Archive for PartialFileArchive {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.accepted >= self.fail_after {
            return Err(synthetic_error());
        }
        let limit = bytes
            .len()
            .min((self.fail_after - self.accepted) as usize)
            .min(7);
        let actual = self.file.write(&bytes[..limit])?;
        self.accepted += actual as u64;
        Ok(actual)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
    fn sync(&mut self) -> io::Result<()> {
        self.file.sync_all()
    }
    fn length(&mut self) -> io::Result<u64> {
        self.file.metadata().map(|metadata| metadata.len())
    }
    fn rewind(&mut self) -> io::Result<()> {
        self.file.seek(SeekFrom::Start(0)).map(|_| ())
    }
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.file.read(bytes)
    }
}

#[test]
fn actual_partial_disk_file_remains_incomplete_and_original_owner_is_preserved() {
    let fixture = FixtureDirectory::new();
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&fixture.file)
        .unwrap();
    let archive = PartialFileArchive {
        file,
        accepted: 0,
        fail_after: 137,
    };
    let mut session = StateSidecar::start_with_archive(
        config(fixture.file.clone()),
        vec![blob(1, vec![0x37; 64])],
        Box::new(archive),
    )
    .unwrap();
    let pointer = session.owners()[0].bytes.as_ptr();
    let terminal = joined(&mut session);
    assert!(terminal.receipt.is_none() && terminal.actual_writer_joined);
    assert_eq!(terminal.failure.unwrap().kind, FailureKind::Write);
    assert_eq!(terminal.counts.written_bytes, 137);
    assert_eq!(fs::metadata(&fixture.file).unwrap().len(), 137);
    read_range_exact(&fixture.file, 128, &[0x37; 9]);
    assert_eq!(session.owners()[0].bytes.as_ptr(), pointer);
    assert_eq!(session.owners()[0].bytes, [0x37; 64]);
    assert!(!terminal.counts.flush_succeeded && !terminal.counts.sync_succeeded);
}

#[test]
fn writer_unwind_keeps_actual_partial_count_and_original_capacity() {
    let (_fixture, mut session) = synthetic_session(Fault::PanicAt(7));
    let capacity = session.owners()[0].bytes.capacity();
    let terminal = joined(&mut session);
    assert_eq!(terminal.failure.unwrap().kind, FailureKind::WriterPanic);
    assert!(terminal.actual_writer_joined && terminal.receipt.is_none());
    assert_eq!(terminal.counts.written_bytes, 7);
    assert_eq!(session.owners()[0].bytes.capacity(), capacity);
    assert_eq!(session.owners()[0].bytes, [19, 20, 0, 255]);
}

#[test]
fn flush_sync_length_seek_and_read_errors_cannot_publish_a_complete_reference() {
    for (fault, expected) in [
        (Fault::Flush, FailureKind::Flush),
        (Fault::Sync, FailureKind::Sync),
        (Fault::Length, FailureKind::Length),
        (Fault::Rewind, FailureKind::Seek),
        (Fault::ReadAt(37), FailureKind::Read),
        (Fault::TruncatedAt(37), FailureKind::Read),
        (Fault::CorruptAt(17), FailureKind::Compare),
        (Fault::Extra, FailureKind::Length),
    ] {
        let (_fixture, mut session) = synthetic_session(fault);
        let terminal = joined(&mut session);
        assert_eq!(terminal.failure.unwrap().kind, expected, "{fault:?}");
        assert!(terminal.actual_writer_joined && terminal.receipt.is_none());
        assert_eq!(terminal.counts.written_bytes, 132);
        assert_eq!(session.owners()[0].bytes, [19, 20, 0, 255]);
    }
}

#[test]
fn archive_drop_panic_after_full_readback_still_invalidates_success() {
    let (_fixture, mut session) = synthetic_session(Fault::DropPanic);
    let terminal = joined(&mut session);
    assert_eq!(
        terminal.failure.unwrap().kind,
        FailureKind::ArchiveDropPanic
    );
    assert!(terminal.receipt.is_none() && terminal.actual_writer_joined);
    assert_eq!(terminal.counts.compared_equal_bytes, 132);
    assert_eq!(session.owners()[0].bytes, [19, 20, 0, 255]);
}

struct Gate {
    open: Mutex<bool>,
    changed: Condvar,
}
impl Gate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            open: Mutex::new(false),
            changed: Condvar::new(),
        })
    }
    fn wait(&self) {
        let mut open = self.open.lock().unwrap();
        while !*open {
            open = self.changed.wait(open).unwrap();
        }
    }
    fn release(&self) {
        *self.open.lock().unwrap() = true;
        self.changed.notify_all();
    }
}
struct GateRelease(Arc<Gate>);
impl Drop for GateRelease {
    fn drop(&mut self) {
        self.0.release();
    }
}

struct HeldArchive {
    inner: BoundedArchive,
    gate: Arc<Gate>,
    entered: Option<mpsc::SyncSender<()>>,
    hold_drop: bool,
}
impl Archive for HeldArchive {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if !self.hold_drop
            && let Some(entered) = self.entered.take()
        {
            let _ = entered.send(());
            self.gate.wait();
        }
        self.inner.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
    fn sync(&mut self) -> io::Result<()> {
        self.inner.sync()
    }
    fn length(&mut self) -> io::Result<u64> {
        self.inner.length()
    }
    fn rewind(&mut self) -> io::Result<()> {
        self.inner.rewind()
    }
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.inner.read(bytes)
    }
}
impl Drop for HeldArchive {
    fn drop(&mut self) {
        if self.hold_drop
            && let Some(entered) = self.entered.take()
        {
            let _ = entered.send(());
            self.gate.wait();
        }
    }
}

fn held_session(
    hold_drop: bool,
) -> (
    FixtureDirectory,
    StateSidecar,
    Arc<Gate>,
    mpsc::Receiver<()>,
    GateRelease,
) {
    let fixture = FixtureDirectory::new();
    let gate = Gate::new();
    let (entered, receiver) = mpsc::sync_channel(1);
    let archive = HeldArchive {
        inner: BoundedArchive::new(Fault::None),
        gate: Arc::clone(&gate),
        entered: Some(entered),
        hold_drop,
    };
    let session = StateSidecar::start_with_archive(
        config(fixture.file.clone()),
        vec![blob(1, vec![1, 2, 3])],
        Box::new(archive),
    )
    .unwrap();
    let fallback = GateRelease(Arc::clone(&gate));
    (fixture, session, gate, receiver, fallback)
}

#[test]
fn completed_write_readback_is_not_success_until_actual_writer_join() {
    let (_fixture, mut session, gate, entered, _fallback) = held_session(true);
    entered
        .recv_timeout(Duration::from_secs(10))
        .expect("actual writer reached post-readback Drop gate");
    assert!(matches!(session.poll_join(), JoinPoll::Pending));
    assert_eq!(session.owners()[0].bytes, [1, 2, 3]);
    gate.release();
    let terminal = joined(&mut session);
    assert!(terminal.receipt.unwrap().actual_writer_joined);
}

#[test]
fn cancellation_during_actual_held_write_retains_owner_and_requires_real_join() {
    let (_fixture, mut session, gate, entered, _fallback) = held_session(false);
    entered
        .recv_timeout(Duration::from_secs(10))
        .expect("actual writer entered write gate");
    session.cancel();
    assert!(matches!(session.poll_join(), JoinPoll::Pending));
    gate.release();
    let terminal = joined(&mut session);
    assert_eq!(terminal.failure.unwrap().kind, FailureKind::Cancelled);
    assert!(terminal.actual_writer_joined && terminal.receipt.is_none());
    assert_eq!(terminal.counts.written_bytes, 3);
    assert_eq!(session.owners()[0].bytes, [1, 2, 3]);
}

#[test]
fn recovery_moves_the_exact_live_owner_and_writer_handle_without_detaching() {
    let (_fixture, session, gate, entered, _fallback) = held_session(false);
    entered
        .recv_timeout(Duration::from_secs(10))
        .expect("actual held writer entry");
    let pointer = session.owners()[0].bytes.as_ptr();
    let mut recovery = session.into_recovery();
    recovery.cancel();
    assert!(matches!(recovery.poll_join(), JoinPoll::Pending));
    assert_eq!(recovery.owners()[0].bytes.as_ptr(), pointer);
    gate.release();
    let terminal = recovery_joined(&mut recovery);
    assert!(terminal.actual_writer_joined && terminal.receipt.is_none());
    assert_eq!(terminal.failure.unwrap().kind, FailureKind::Cancelled);
    assert_eq!(recovery.owners()[0].bytes.as_ptr(), pointer);
}

#[test]
fn cancel_after_actual_success_invalidates_cached_receipt_without_faking_a_second_join() {
    let (_fixture, mut session) = synthetic_session(Fault::None);
    let first = joined(&mut session);
    assert!(first.receipt.is_some());
    session.cancel();
    let second = joined(&mut session);
    assert!(second.actual_writer_joined && second.receipt.is_none());
    assert_eq!(second.failure.unwrap().kind, FailureKind::Cancelled);
    assert_eq!(second.counts, first.counts);
}

#[test]
fn each_optional_identity_field_is_preserved_in_the_actual_file_table() {
    let fixture = FixtureDirectory::new();
    let mut first = blob(1, vec![11]);
    first.identity.worker_id = None;
    first.identity.source_id = Some(0);
    first.identity.phase = SuppliedPhase::StrictClone;
    let mut second = blob(2, vec![11]);
    second.identity.worker_id = Some(0);
    second.identity.source_id = None;
    second.identity.phase = SuppliedPhase::RestoredSource;
    let mut session =
        StateSidecar::start(config(fixture.file.clone()), vec![first, second]).unwrap();
    let receipt = joined(&mut session).receipt.unwrap();
    assert_eq!(receipt.unique_blobs, 2);
    let mut file = File::open(&fixture.file).unwrap();
    let mut row = [0u8; 96];
    file.seek(SeekFrom::Start(32)).unwrap();
    file.read_exact(&mut row).unwrap();
    let u64_at =
        |bytes: &[u8], at: usize| u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
    assert_eq!(u32::from_le_bytes(row[..4].try_into().unwrap()), 2);
    assert_eq!(u64_at(&row, 8), 0);
    assert_eq!(u64_at(&row, 16), 0);
    assert_eq!(u64_at(&row, 24), 1);
    assert_eq!(u64_at(&row, 32), 0);
    assert_eq!(u64_at(&row, 40), 7);
    assert_eq!(u64_at(&row, 48), 1);
    assert_eq!(u64_at(&row, 56), 17);
    file.read_exact(&mut row).unwrap();
    assert_eq!(u32::from_le_bytes(row[..4].try_into().unwrap()), 3);
    assert_eq!(u64_at(&row, 8), 1);
    assert_eq!(u64_at(&row, 16), 0);
    assert_eq!(u64_at(&row, 24), 0);
    assert_eq!(u64_at(&row, 32), 0);
    assert_eq!(u64_at(&row, 48), 2);
    assert_eq!(u64_at(&row, 56), 34);
}

#[test]
fn diagnostic_large_length_is_rebuilt_into_a_bounded_actual_allocation() {
    let original = "a".repeat(2 * 1024 * 1024);
    let original_pointer = original.as_ptr();
    let failure = SidecarFailure::new(FailureKind::Write, &original);
    assert_eq!(failure.diagnostic.len(), 4096);
    assert_eq!(failure.diagnostic.capacity(), failure.diagnostic.len());
    assert_ne!(failure.diagnostic.as_ptr(), original_pointer);
    assert_eq!(original.len(), 2 * 1024 * 1024);
    assert_eq!(failure.diagnostic.as_str(), &original[..4096]);
}

#[test]
fn diagnostic_small_length_does_not_keep_the_inputs_large_spare_capacity() {
    let mut original = String::with_capacity(2 * 1024 * 1024);
    original.push_str("bad");
    let original_capacity = original.capacity();
    let failure = SidecarFailure::new(FailureKind::Write, &original);
    assert_eq!(failure.diagnostic, "bad");
    assert_eq!(failure.diagnostic.capacity(), 3);
    assert!(original_capacity > 4096);
    assert_ne!(failure.diagnostic.as_ptr(), original.as_ptr());
    let original_pointer = original.as_ptr();
    let unchecked = SidecarFailure {
        kind: FailureKind::Write,
        diagnostic: original,
    };
    let cloned = unchecked.clone();
    assert_eq!(cloned.diagnostic, "bad");
    assert_eq!(cloned.diagnostic.capacity(), 3);
    assert_ne!(cloned.diagnostic.as_ptr(), original_pointer);
}

#[test]
fn unchecked_public_diagnostic_utf8_clone_and_terminal_clone_reapply_the_bound() {
    let mut oversized = String::with_capacity(2 * 1024 * 1024);
    for _ in 0..10_000 {
        oversized.push('钢');
    }
    let unchecked = SidecarFailure {
        kind: FailureKind::Read,
        diagnostic: oversized,
    };
    let cloned = unchecked.clone();
    assert_eq!(cloned.kind, FailureKind::Read);
    assert_eq!(cloned.diagnostic.len(), 4095);
    assert_eq!(cloned.diagnostic.capacity(), 4095);
    assert!(unchecked.diagnostic.starts_with(&cloned.diagnostic));
    assert_eq!(
        std::str::from_utf8(cloned.diagnostic.as_bytes()).unwrap(),
        cloned.diagnostic.as_str()
    );
    let terminal = Terminal {
        actual_writer_joined: false,
        receipt: None,
        failure: Some(unchecked),
        writer_failure: Some(cloned),
        counts: DurableCounts::default(),
    };
    let copy = terminal.clone();
    for failure in [copy.failure.unwrap(), copy.writer_failure.unwrap()] {
        assert!(failure.diagnostic.len() <= 4096);
        assert!(failure.diagnostic.capacity() <= 4096);
    }
}

struct HugeError {
    text: String,
    released: Arc<AtomicBool>,
}

impl std::fmt::Debug for HugeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HugeError")
            .field("text_len", &self.text.len())
            .finish()
    }
}
impl std::fmt::Display for HugeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // One actual borrowed large Display chunk; the module must not format! it in full.
        formatter.write_str(&self.text)
    }
}
impl std::error::Error for HugeError {}
impl Drop for HugeError {
    fn drop(&mut self) {
        self.released.store(true, Ordering::Release);
    }
}

struct HugeErrorArchive {
    error: Option<HugeError>,
}

impl Archive for HugeErrorArchive {
    fn write(&mut self, _bytes: &[u8]) -> io::Result<usize> {
        Err(io::Error::other(
            self.error.take().expect("one supplied error"),
        ))
    }
    fn flush(&mut self) -> io::Result<()> {
        panic!("write failure must stop before flush")
    }
    fn sync(&mut self) -> io::Result<()> {
        panic!("write failure must stop before sync")
    }
    fn length(&mut self) -> io::Result<u64> {
        panic!("write failure must stop before length")
    }
    fn rewind(&mut self) -> io::Result<()> {
        panic!("write failure must stop before rewind")
    }
    fn read(&mut self, _bytes: &mut [u8]) -> io::Result<usize> {
        panic!("write failure must stop before readback")
    }
}

#[test]
fn actual_io_error_display_is_streamed_into_a_bounded_resident_diagnostic() {
    let fixture = FixtureDirectory::new();
    let released = Arc::new(AtomicBool::new(false));
    let mut text = String::with_capacity(2 * 1024 * 1024);
    for _ in 0..10_000 {
        text.push_str("钢琴");
    }
    let archive = HugeErrorArchive {
        error: Some(HugeError {
            text,
            released: Arc::clone(&released),
        }),
    };
    let mut session = StateSidecar::start_with_archive(
        config(fixture.file.clone()),
        vec![blob(1, vec![0x31, 0x32])],
        Box::new(archive),
    )
    .unwrap();
    let terminal = joined(&mut session);
    assert!(terminal.actual_writer_joined && terminal.receipt.is_none());
    assert!(
        released.load(Ordering::Acquire),
        "supplied unbounded error allocation was not retained"
    );
    assert_eq!(terminal.counts.written_bytes, 0);
    for failure in [terminal.failure.unwrap(), terminal.writer_failure.unwrap()] {
        assert_eq!(failure.kind, FailureKind::Write);
        assert!(failure.diagnostic.starts_with("physical write: "));
        assert!(failure.diagnostic.len() <= 4096);
        assert!(failure.diagnostic.capacity() <= 4096);
        assert_eq!(
            std::str::from_utf8(failure.diagnostic.as_bytes()).unwrap(),
            failure.diagnostic.as_str()
        );
    }
    assert_eq!(session.owners()[0].bytes, [0x31, 0x32]);
}

#[test]
fn absolute_path_large_encoded_length_is_rejected_before_file_or_writer_creation() {
    let fixture = FixtureDirectory::new();
    let mut oversized_path = fixture.directory.clone();
    oversized_path.push("钢".repeat(MAX_PATH_BYTES));
    assert!(oversized_path.is_absolute());
    assert!(oversized_path.as_os_str().as_encoded_bytes().len() > MAX_PATH_BYTES);
    let bytes = vec![8, 0, 9];
    let pointer = bytes.as_ptr();
    let rejected = StateSidecar::start(config(oversized_path), vec![blob(1, bytes)])
        .err()
        .unwrap();
    assert_eq!(rejected.failure.kind, FailureKind::Admission);
    assert!(rejected.failure.diagnostic.capacity() <= 4096);
    assert_eq!(rejected.owners[0].bytes.as_ptr(), pointer);
    assert!(!fixture.file.exists());
    assert_eq!(fs::read_dir(&fixture.directory).unwrap().count(), 0);
}

#[test]
fn absolute_path_small_length_with_large_reserved_capacity_is_rejected_before_creation() {
    let fixture = FixtureDirectory::new();
    let mut oversized_path = PathBuf::with_capacity(MAX_PATH_BYTES + 1);
    oversized_path.push(&fixture.file);
    assert!(oversized_path.is_absolute());
    assert!(oversized_path.as_os_str().as_encoded_bytes().len() < MAX_PATH_BYTES);
    assert!(oversized_path.capacity() > MAX_PATH_BYTES);
    let bytes = vec![0, 3];
    let pointer = bytes.as_ptr();
    let rejected = StateSidecar::start(config(oversized_path), vec![blob(1, bytes)])
        .err()
        .unwrap();
    assert_eq!(rejected.failure.kind, FailureKind::Admission);
    assert_eq!(rejected.owners[0].bytes.as_ptr(), pointer);
    assert!(!fixture.file.exists());
    assert_eq!(fs::read_dir(&fixture.directory).unwrap().count(), 0);
}

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
mod live_tests {
    use super::*;
    use crate::genuine_app_observer::{
        CaptureLabel, InstanceIdentity, InstancePhase, Stage, stream_codec as native,
        stream_control as control, stream_envelope as outer,
    };
    use crate::instrument_bounce::{
        BounceAudioBus, BouncePluginMetadata, BouncePluginState, BounceSourceIdentity,
        render::{BackendDelivery, BackendFailure, BackendFailureKind},
    };
    use std::collections::BTreeMap;
    use vst3_host::strict_offline::{
        FailureKind as StrictKind, LimitEvidence, LimitSite, MetadataGetter, RequestContext,
    };

    fn owner() -> InstanceIdentity {
        InstanceIdentity {
            ordinal: 3,
            phase: InstancePhase::InitialSource,
            worker_id: None,
            source_instance_id: Some(0),
        }
    }
    fn header(kind: u16, sequence: u64) -> outer::Header {
        outer::Header {
            sequence,
            monotonic_ns: sequence * 11,
            stage: Stage::InitialSource,
            instance: if kind == 2 { Some(owner()) } else { None },
            kind,
            lane: outer::Lane::State,
        }
    }
    fn slots() -> LiveStateSlots {
        LiveStateSlots::new(RetentionLimits::default()).unwrap()
    }
    /// Pure codec fixture only. This metadata is never scanned or submitted to
    /// a plugin and supplies no SDK, NOIRE, source-ready or acoustic evidence.
    fn metadata() -> BouncePluginMetadata {
        BouncePluginMetadata {
            class_uid: "00112233445566778899aabbccddeeff".into(),
            version: "model-only".into(),
            name: "pure codec fixture".into(),
            category: "Instrument|Model".into(),
            input_buses: vec![BounceAudioBus {
                channel_count: 0,
                is_main: true,
                active: None,
            }],
            output_buses: vec![
                BounceAudioBus {
                    channel_count: 2,
                    is_main: true,
                    active: Some(true),
                },
                BounceAudioBus {
                    channel_count: 1,
                    is_main: false,
                    active: Some(false),
                },
            ],
            event_input_buses: vec![(16, true, Some(true))],
            event_output_buses: vec![(1, false, None)],
            latency_samples: 127,
            tail_samples: u32::MAX,
        }
    }
    fn dictionary(metadata: &BouncePluginMetadata) -> native::BaselineDictionary {
        native::BaselineDictionary::new(
            7,
            native::CanonicalMetadata {
                class_uid: metadata.class_uid.clone(),
                version: metadata.version.clone(),
                name: metadata.name.clone(),
                category: metadata.category.clone(),
                audio: native::AudioLayout {
                    inputs: metadata
                        .input_buses
                        .iter()
                        .map(|bus| native::AudioBus {
                            channel_count: bus.channel_count as u32,
                            is_main: bus.is_main,
                            active: bus.active,
                        })
                        .collect(),
                    outputs: metadata
                        .output_buses
                        .iter()
                        .map(|bus| native::AudioBus {
                            channel_count: bus.channel_count as u32,
                            is_main: bus.is_main,
                            active: bus.active,
                        })
                        .collect(),
                },
                events: native::EventLayout {
                    inputs: metadata
                        .event_input_buses
                        .iter()
                        .map(|&(channel_count, is_main, active)| native::EventBus {
                            channel_count,
                            is_main,
                            active,
                        })
                        .collect(),
                    outputs: metadata
                        .event_output_buses
                        .iter()
                        .map(|&(channel_count, is_main, active)| native::EventBus {
                            channel_count,
                            is_main,
                            active,
                        })
                        .collect(),
                },
                latency_samples: metadata.latency_samples,
                tail_samples: metadata.tail_samples,
            },
        )
        .unwrap()
    }
    fn source() -> BounceSourceIdentity {
        BounceSourceIdentity {
            project_session: 5,
            project_fingerprint: 7,
            timeline_revision: 11,
            epoch: 13,
            endpoint_id: 17,
            channel_id: 19,
            instance_id: 0,
        }
    }
    fn state() -> BouncePluginState {
        BouncePluginState {
            metadata: metadata(),
            opaque_state: vec![0, 0x80, 0xff, 3],
            parameter_bases: BTreeMap::from([
                (0, f32::from_bits(0x8000_0000)),
                (7, f32::from_bits(0x7fc0_0137)),
                (11, f32::INFINITY),
            ]),
            native_generation: 23,
            native_dirty_revision: 29,
        }
    }
    fn failure() -> BackendFailure {
        let mut failure =
            BackendFailure::new(BackendFailureKind::Deadline, "actual outcome field fixture");
        failure.delivery = Some(BackendDelivery::MayHaveApplied);
        failure.strict_kind = Some(StrictKind::Limit);
        failure.strict_diagnostic_unavailable = Some(StrictKind::Io(-123));
        failure.strict_stage = Some("state.load.actual-result");
        failure.last_wire_request = Some(RequestContext {
            operation: "StrictMetadataSnapshot",
            media_type: None,
            direction: None,
            bus_index: None,
        });
        failure.strict_metadata_getter = Some(MetadataGetter::TailSamples);
        failure.limit_evidence = Some(LimitEvidence {
            site: LimitSite::ParameterCatalogTextBytes,
            observed_at_least: 65537,
            maximum: 65536,
        });
        failure
    }

    #[test]
    fn live_owned_admission_keeps_exact_pointer_capacity_and_rejected_originals() {
        let mut slots = slots();
        assert!(slots.is_empty());
        let mut bytes = Vec::with_capacity(17);
        bytes.extend_from_slice(&[0, 0x80, 0xff, 3]);
        let pointer = bytes.as_ptr();
        let capacity = bytes.capacity();
        let link = slots.retain_owned(header(2, 1), None, bytes).unwrap();
        assert_eq!(slots.opaque(link).unwrap().as_ptr(), pointer);
        assert_eq!(slots.opaque(link).unwrap(), [0, 0x80, 0xff, 3]);
        assert_eq!(link.owned_capacity(), capacity);
        assert_eq!(slots.owned_capacity(), capacity);
        assert_eq!(slots.admitted_bytes(), 4);
        let mut bytes = Vec::with_capacity(17);
        bytes.push(1);
        let pointer = bytes.as_ptr();
        let capacity = bytes.capacity();
        let mut invalid = header(9, 2);
        invalid.instance = Some(owner());
        let rejected = slots
            .retain_owned(invalid, Some(owner()), bytes)
            .unwrap_err();
        assert_eq!(rejected.error, SlotError::Header);
        assert_eq!(rejected.bytes.as_ptr(), pointer);
        assert_eq!(rejected.bytes.capacity(), capacity);
        assert_eq!(rejected.bytes, [1]);
        assert_eq!(slots.len(), 1);
    }

    #[test]
    fn live_slots_enforce_count_capacity_total_order_and_explicit_owner() {
        let mut slots = LiveStateSlots::new(RetentionLimits {
            max_blob_bytes: 8,
            max_blobs: 2,
            max_owned_capacity_bytes: 12,
        })
        .unwrap();
        let first = slots.retain_borrowed(header(2, 1), None, &[1; 8]).unwrap();
        assert_eq!(
            slots.retain_borrowed(header(2, 2), None, &[2; 5]),
            Err(SlotError::Bound)
        );
        assert_eq!(
            slots.retain_borrowed(header(2, 1), None, &[2]),
            Err(SlotError::Order)
        );
        let mut wrong = owner();
        wrong.worker_id = Some(0);
        assert_eq!(
            slots.retain_borrowed(header(2, 2), Some(wrong), &[2]),
            Err(SlotError::OwnerMismatch)
        );
        assert_eq!(
            slots.retain_borrowed(header(9, 2), None, &[2]),
            Err(SlotError::MissingOwner)
        );
        let second = slots
            .retain_borrowed(header(9, 2), Some(owner()), &[2; 4])
            .unwrap();
        assert_eq!(
            slots.retain_borrowed(header(2, 3), None, &[3]),
            Err(SlotError::Bound)
        );
        assert_eq!(
            (slots.len(), slots.admitted_bytes(), slots.owned_capacity()),
            (2, 12, 12)
        );
        assert_eq!(slots.opaque(first).unwrap(), [1; 8]);
        assert_eq!(slots.opaque(second).unwrap(), [2; 4]);
        let mut oversized = Vec::with_capacity(9);
        oversized.push(0xff);
        let pointer = oversized.as_ptr();
        let mut fresh = LiveStateSlots::new(RetentionLimits {
            max_blob_bytes: 8,
            max_blobs: 2,
            max_owned_capacity_bytes: 12,
        })
        .unwrap();
        let rejected = fresh
            .retain_owned(header(2, 1), None, oversized)
            .unwrap_err();
        assert_eq!(rejected.error, SlotError::Bound);
        assert_eq!(rejected.bytes.as_ptr(), pointer);
        assert!(fresh.is_empty());
    }

    #[test]
    fn default_live_slots_keep_exactly_four_states_and_return_the_fifth_owner() {
        let mut slots = slots();
        let mut links = [None; MAX_BLOB_COUNT];
        for (index, link) in links.iter_mut().enumerate() {
            *link = Some(
                slots
                    .retain_owned(header(2, index as u64 + 1), None, vec![index as u8])
                    .unwrap(),
            );
        }
        let mut fifth = Vec::with_capacity(7);
        fifth.extend_from_slice(&[0xff, 0x80]);
        let pointer = fifth.as_ptr();
        let capacity = fifth.capacity();
        let rejected = slots
            .retain_owned(header(2, MAX_BLOB_COUNT as u64 + 1), None, fifth)
            .unwrap_err();
        assert_eq!(rejected.error, SlotError::Bound);
        assert_eq!(rejected.bytes.as_ptr(), pointer);
        assert_eq!(rejected.bytes.capacity(), capacity);
        assert_eq!(rejected.bytes, [0xff, 0x80]);
        assert_eq!((slots.len(), slots.admitted_bytes()), (MAX_BLOB_COUNT, 4));
        for (index, link) in links.into_iter().enumerate() {
            assert_eq!(slots.opaque(link.unwrap()).unwrap(), [index as u8]);
        }
    }

    #[test]
    fn state_load_body_preserves_full_strict_outcome_without_a_fabricated_operation() {
        let failure = failure();
        let mut slots = slots();
        let view = slots
            .retain_state_load(
                header(2, 1),
                None,
                &[0x80, 0xff],
                control::OutcomeRef::StrictError(&failure),
            )
            .unwrap();
        let StateView::StateLoad { link, .. } = &view else {
            panic!("state-load view lost")
        };
        let link = *link;
        let mut bytes = Vec::with_capacity(outer::MAX_OPERAND);
        let pointer = bytes.as_ptr();
        encode_enveloped(view, None, &mut bytes).unwrap();
        assert_eq!(bytes.as_ptr(), pointer);
        let (original, body) = decode_enveloped(&bytes, None).unwrap();
        assert_eq!(original, header(2, 1));
        let StateBody::StateLoad {
            link: decoded,
            outcome: control::Outcome::StrictError(observed),
        } = body
        else {
            panic!("actual outcome lost")
        };
        assert_eq!(decoded, link);
        assert_eq!(observed.kind, failure.kind);
        assert_eq!(observed.delivery, failure.delivery);
        assert_eq!(observed.diagnostic, failure.diagnostic());
        assert_eq!(observed.strict_kind, failure.strict_kind);
        assert_eq!(
            observed.strict_diagnostic_unavailable,
            failure.strict_diagnostic_unavailable
        );
        assert_eq!(observed.strict_stage, failure.strict_stage);
        assert_eq!(observed.last_wire_request, failure.last_wire_request);
        assert_eq!(
            observed.strict_metadata_getter,
            failure.strict_metadata_getter
        );
        assert_eq!(observed.limit_evidence, failure.limit_evidence);
    }

    #[test]
    fn capture_body_preserves_none_header_full_metadata_bases_receipt_and_native_revisions() {
        let state = state();
        let mut baseline = state.metadata.clone();
        baseline.name = "different baseline".into();
        let dictionary = dictionary(&baseline);
        let mut slots = slots();
        let view = slots
            .retain_capture(
                header(9, 1),
                owner(),
                CaptureOperands {
                    label: CaptureLabel::Final,
                    identity: source(),
                    receipt_identity: usize::MAX,
                    state: &state,
                },
            )
            .unwrap();
        let StateView::Capture { link, .. } = &view else {
            panic!("capture view lost")
        };
        let link = *link;
        let mut bytes = Vec::with_capacity(outer::MAX_OPERAND);
        encode_enveloped(view, Some(&dictionary), &mut bytes).unwrap();
        let (original, body) = decode_enveloped(&bytes, Some(&dictionary)).unwrap();
        assert_eq!(original.instance, None);
        let StateBody::Capture {
            link: decoded,
            label,
            identity,
            receipt_identity,
            metadata,
            parameter_bases,
            native_generation,
            native_dirty_revision,
        } = body
        else {
            panic!("capture lost")
        };
        assert_eq!(decoded, link);
        assert_eq!(decoded.supplied_owner().source_instance_id, Some(0));
        assert_eq!(label, CaptureLabel::Final);
        assert_eq!(identity, source());
        assert_eq!(receipt_identity, usize::MAX);
        assert_eq!((native_generation, native_dirty_revision), (23, 29));
        assert_eq!(
            (
                metadata.class_uid,
                metadata.version,
                metadata.name,
                metadata.category
            ),
            (
                state.metadata.class_uid.as_str(),
                state.metadata.version.as_str(),
                state.metadata.name.as_str(),
                state.metadata.category.as_str()
            )
        );
        assert_eq!(
            (metadata.latency_samples, metadata.tail_samples),
            (127, u32::MAX)
        );
        for (retained, actual) in [
            (metadata.audio_inputs, &state.metadata.input_buses),
            (metadata.audio_outputs, &state.metadata.output_buses),
        ] {
            assert_eq!(retained.len(), actual.len());
            for (retained, actual) in retained.iter().zip(actual) {
                assert_eq!(
                    (
                        retained.channel_count as usize,
                        retained.is_main,
                        retained.active
                    ),
                    (actual.channel_count, actual.is_main, actual.active)
                );
            }
        }
        for (retained, actual) in [
            (metadata.event_inputs, &state.metadata.event_input_buses),
            (metadata.event_outputs, &state.metadata.event_output_buses),
        ] {
            assert_eq!(retained.len(), actual.len());
            assert_eq!(
                retained
                    .iter()
                    .map(|bus| (bus.channel_count, bus.is_main, bus.active))
                    .collect::<Vec<_>>(),
                *actual
            );
        }
        assert_eq!(parameter_bases.len(), 3);
        assert!(!parameter_bases.is_empty());
        assert_eq!(
            parameter_bases.items().collect::<Vec<_>>(),
            [
                (0, 0x8000_0000),
                (7, 0x7fc0_0137),
                (11, f32::INFINITY.to_bits())
            ]
        );
        let mut wrong_source = source();
        wrong_source.instance_id = 1;
        assert!(matches!(
            capture_view(link, CaptureLabel::Initial, wrong_source, 0, &state),
            Err(SlotError::OwnerMismatch)
        ));
    }

    #[test]
    fn capture_channel_id_round_trips_u32_max_and_refuses_a_wider_wire_value() {
        let state = state();
        let dictionary = dictionary(&state.metadata);
        let mut identity = source();
        identity.channel_id = u32::MAX;
        let mut slots = slots();
        let view = slots
            .retain_capture(
                header(9, 1),
                owner(),
                CaptureOperands {
                    label: CaptureLabel::Initial,
                    identity,
                    receipt_identity: 37,
                    state: &state,
                },
            )
            .unwrap();
        let mut bytes = Vec::with_capacity(outer::MAX_OPERAND);
        encode_body(view, Some(&dictionary), &mut bytes, outer::MAX_BODY).unwrap();
        let StateBody::Capture {
            identity: observed, ..
        } = decode_body(header(9, 1), &bytes, Some(&dictionary)).unwrap()
        else {
            panic!("capture body lost")
        };
        assert_eq!(observed, identity);
        let channel_wire = u64::from(u32::MAX).to_le_bytes();
        let offsets: Vec<_> = bytes
            .windows(channel_wire.len())
            .enumerate()
            .filter_map(|(offset, value)| (value == channel_wire).then_some(offset))
            .collect();
        assert_eq!(
            offsets.len(),
            1,
            "fixture channel field must be unambiguous"
        );
        bytes[offsets[0]..offsets[0] + 8].copy_from_slice(&(u64::from(u32::MAX) + 1).to_le_bytes());
        assert!(matches!(
            decode_body(header(9, 1), &bytes, Some(&dictionary)),
            Err(BodyError::Bound)
        ));
    }

    #[test]
    fn state_body_budget_exact_fit_or_whole_refusal_never_reallocates() {
        let dictionary = dictionary(&metadata());
        let mut slots = slots();
        let link = slots.retain_borrowed(header(2, 1), None, &[1, 2]).unwrap();
        let mut full = Vec::with_capacity(outer::MAX_OPERAND);
        encode_body(
            StateView::StateLoad {
                link,
                outcome: control::OutcomeRef::Error("actual error"),
            },
            Some(&dictionary),
            &mut full,
            outer::MAX_BODY,
        )
        .unwrap();
        let mut limited = Vec::with_capacity(outer::MAX_OPERAND);
        let pointer = limited.as_ptr();
        let capacity = limited.capacity();
        encode_body(
            StateView::StateLoad {
                link,
                outcome: control::OutcomeRef::Error("actual error"),
            },
            Some(&dictionary),
            &mut limited,
            full.len(),
        )
        .unwrap();
        assert_eq!(limited, full);
        limited.clear();
        assert!(
            encode_body(
                StateView::StateLoad {
                    link,
                    outcome: control::OutcomeRef::Error("actual error")
                },
                Some(&dictionary),
                &mut limited,
                full.len() - 1
            )
            .is_err()
        );
        assert!(limited.is_empty());
        assert_eq!(limited.as_ptr(), pointer);
        assert_eq!(limited.capacity(), capacity);
        limited.push(0xff);
        assert_eq!(
            encode_body(
                StateView::StateLoad {
                    link,
                    outcome: control::OutcomeRef::Ok
                },
                Some(&dictionary),
                &mut limited,
                0
            ),
            Err(BodyError::Occupied)
        );
        assert_eq!(limited, [0xff]);
    }

    #[test]
    fn state_decoder_refuses_changed_header_unknown_flags_and_trailing_outcome() {
        let dictionary = dictionary(&metadata());
        let mut slots = slots();
        let link = slots.retain_borrowed(header(2, 1), None, &[1]).unwrap();
        let mut bytes = Vec::with_capacity(outer::MAX_OPERAND);
        encode_body(
            StateView::StateLoad {
                link,
                outcome: control::OutcomeRef::Ok,
            },
            Some(&dictionary),
            &mut bytes,
            outer::MAX_BODY,
        )
        .unwrap();
        assert!(decode_body(header(2, 2), &bytes, Some(&dictionary)).is_err());
        let mut unknown = bytes.clone();
        unknown[50] = 2;
        assert!(matches!(
            decode_body(header(2, 1), &unknown, Some(&dictionary)),
            Err(BodyError::UnknownCode)
        ));
        let mut unknown = bytes.clone();
        unknown[4] = 2;
        assert!(decode_body(header(2, 1), &unknown, Some(&dictionary)).is_err());
        let mut extra = bytes.clone();
        extra.push(0);
        assert!(decode_body(header(2, 1), &extra, Some(&dictionary)).is_err());
        for length in 0..bytes.len() {
            assert!(decode_body(header(2, 1), &bytes[..length], Some(&dictionary)).is_err());
        }
    }

    #[test]
    fn live_handoff_actual_writer_ranges_keep_original_headers_and_duplicate_allocations() {
        let fixture = FixtureDirectory::new();
        let mut slots = slots();
        let first = slots
            .retain_borrowed(header(2, 1), None, &[1, 2, 3, 4])
            .unwrap();
        let second = slots
            .retain_borrowed(header(9, 2), Some(owner()), &[1, 2, 3, 4])
            .unwrap();
        let first_pointer = slots.opaque(first).unwrap().as_ptr();
        let second_pointer = slots.opaque(second).unwrap().as_ptr();
        assert_ne!(first_pointer, second_pointer);
        let mut sidecar = slots
            .into_batch()
            .unwrap()
            .start(config(fixture.file.clone()))
            .unwrap();
        assert_eq!(sidecar.owners()[0].bytes.as_ptr(), first_pointer);
        assert_eq!(sidecar.owners()[1].bytes.as_ptr(), second_pointer);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match sidecar.poll_join() {
                LiveJoinPoll::Pending => {
                    assert!(Instant::now() < deadline);
                    std::thread::yield_now();
                }
                LiveJoinPoll::Joined { terminal, linkage } => {
                    let linkage = linkage.unwrap();
                    assert!(terminal.actual_writer_joined);
                    assert_eq!(linkage.links[..2], [Some(first), Some(second)]);
                    assert_eq!(linkage.links[1].unwrap().original_header().instance, None);
                    assert_eq!(linkage.ranges.len(), 2);
                    assert_eq!(linkage.ranges[0].offset, linkage.ranges[1].offset);
                    assert_eq!(terminal.counts.original_payload_compared_bytes, 8);
                    break;
                }
            }
        }
    }

    #[test]
    fn cancelled_live_sidecar_recovery_retains_slots_and_requires_actual_join_without_receipt() {
        let fixture = FixtureDirectory::new();
        let payloads: [&[u8]; 2] = [&[0, 0x80, 0xff, 3], &[9, 0, 0xff]];
        let mut first = Vec::with_capacity(17);
        first.extend_from_slice(payloads[0]);
        let mut second = Vec::with_capacity(31);
        second.extend_from_slice(payloads[1]);
        let pointers = [first.as_ptr(), second.as_ptr()];
        let capacities = [first.capacity(), second.capacity()];
        let mut slots = slots();
        let links = [
            slots.retain_owned(header(2, 1), None, first).unwrap(),
            slots
                .retain_owned(header(9, 2), Some(owner()), second)
                .unwrap(),
        ];
        for (index, link) in links.iter().copied().enumerate() {
            assert_eq!(link.slot(), index);
            assert_eq!(link.byte_len(), payloads[index].len());
            assert_eq!(slots.opaque(link).unwrap().as_ptr(), pointers[index]);
        }
        assert_eq!(links[0].original_header().instance, Some(owner()));
        assert_eq!(links[1].original_header().instance, None);
        let mut sidecar = slots
            .into_batch()
            .unwrap()
            .start(config(fixture.file.clone()))
            .unwrap();
        sidecar.cancel();
        let mut recovery = sidecar.into_recovery();
        recovery.cancel();
        let deadline = Instant::now() + Duration::from_secs(10);
        let first_terminal = loop {
            match recovery.poll_join() {
                LiveJoinPoll::Pending => {
                    assert!(Instant::now() < deadline, "same writer did not retire");
                    std::thread::yield_now();
                }
                LiveJoinPoll::Joined { terminal, linkage } => {
                    assert!(terminal.actual_writer_joined);
                    assert_eq!(
                        terminal.failure.as_ref().unwrap().kind,
                        FailureKind::Cancelled
                    );
                    assert!(terminal.receipt.is_none());
                    assert!(matches!(linkage, Err(SlotError::Incomplete)));
                    break terminal.clone();
                }
            }
        };
        for (index, retained) in recovery.owners().iter().enumerate() {
            assert_eq!(retained.bytes.as_ptr(), pointers[index]);
            assert_eq!(retained.bytes.capacity(), capacities[index]);
            assert_eq!(retained.bytes, payloads[index]);
            assert_eq!(
                retained.identity.record_ordinal,
                links[index].original_header().sequence
            );
            assert_eq!(
                retained.identity.source_id,
                links[index].supplied_owner().source_instance_id
            );
        }
        assert_eq!(recovery.owners().len(), 2);
        let LiveJoinPoll::Joined { terminal, linkage } = recovery.poll_join() else {
            panic!("joined recovery cannot become pending again")
        };
        assert_eq!(terminal, &first_terminal);
        assert!(matches!(linkage, Err(SlotError::Incomplete)));
    }

    #[test]
    fn live_start_failure_retains_all_linked_original_owners_without_a_fake_join() {
        let fixture = FixtureDirectory::new();
        fs::write(&fixture.file, b"preserve existing fixture").unwrap();
        let mut slots = slots();
        let link = slots
            .retain_borrowed(header(9, 1), Some(owner()), &[0xff, 0x80])
            .unwrap();
        let pointer = slots.opaque(link).unwrap().as_ptr();
        let rejected = slots
            .into_batch()
            .unwrap()
            .start(config(fixture.file.clone()))
            .err()
            .unwrap();
        assert_eq!(rejected.failure.failure.kind, FailureKind::Create);
        assert_eq!(rejected.failure.owners[0].bytes.as_ptr(), pointer);
        assert_eq!(rejected.links[0], Some(link));
        assert_eq!(rejected.count, 1);
        assert_eq!(
            fs::read(&fixture.file).unwrap(),
            b"preserve existing fixture"
        );
    }
}
