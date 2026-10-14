//! Windows file-handle identity and checked same-volume publication.
//! No replacement, cross-volume copy, directory-fsync emulation or UI calls.

use super::PublishedFileIdentity;
use std::{
    fs::{File, OpenOptions},
    io,
    mem::{MaybeUninit, size_of},
    os::windows::{ffi::OsStrExt, fs::OpenOptionsExt, io::AsRawHandle},
    path::Path,
};
use winapi::um::{
    fileapi::{FILE_BASIC_INFO, FILE_ID_INFO, FILE_STANDARD_INFO},
    minwinbase::{FILE_INFO_BY_HANDLE_CLASS, FileBasicInfo, FileIdInfo, FileStandardInfo},
    winbase::{
        FILE_FLAG_OPEN_REPARSE_POINT, GetFileInformationByHandleEx, MOVEFILE_REPLACE_EXISTING,
        MOVEFILE_WRITE_THROUGH, MoveFileExW,
    },
    winnt::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT},
};

pub(crate) struct Snapshot {
    pub(crate) identity: PublishedFileIdentity,
    pub(crate) changed_seconds: i64,
    pub(crate) changed_nanoseconds: i64,
}

fn information<T>(file: &File, class: FILE_INFO_BY_HANDLE_CLASS) -> io::Result<T> {
    let mut info = MaybeUninit::<T>::zeroed();
    // The three private callers pair the OS class with its exact C-layout type.
    // The File owns a live handle, and synchronous query borrows this buffer.
    let success = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle().cast(),
            class,
            info.as_mut_ptr().cast(),
            size_of::<T>() as u32,
        )
    };
    if success == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { info.assume_init() })
}

fn timestamp(ticks: i64) -> (i64, i64) {
    // Keep all 100ns precision without interpreting a Windows epoch as Unix.
    (
        ticks.div_euclid(10_000_000),
        ticks.rem_euclid(10_000_000) * 100,
    )
}

pub(crate) fn inspect(file: &File) -> io::Result<Snapshot> {
    let basic: FILE_BASIC_INFO = information(file, FileBasicInfo)?;
    if basic.FileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_DIRECTORY) != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Occurrence output must be a regular file, without a reparse point",
        ));
    }
    let id: FILE_ID_INFO = information(file, FileIdInfo)?;
    let standard: FILE_STANDARD_INFO = information(file, FileStandardInfo)?;
    let bytes = unsafe { *standard.EndOfFile.QuadPart() };
    if standard.Directory != 0 || bytes < 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid occurrence file extent",
        ));
    }
    let (modified_seconds, modified_nanoseconds) =
        timestamp(unsafe { *basic.LastWriteTime.QuadPart() });
    let (changed_seconds, changed_nanoseconds) = timestamp(unsafe { *basic.ChangeTime.QuadPart() });
    Ok(Snapshot {
        identity: PublishedFileIdentity {
            device: id.VolumeSerialNumber,
            inode: u128::from_le_bytes(id.FileId.Identifier),
            bytes: bytes as u64,
            modified_seconds,
            modified_nanoseconds,
        },
        changed_seconds,
        changed_nanoseconds,
    })
}

pub(crate) fn open_unfollowed(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

fn wide_path(path: &Path) -> io::Result<Vec<u16>> {
    let mut text = path.as_os_str().encode_wide().collect::<Vec<_>>();
    if text.contains(&0) || !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid absolute publication path",
        ));
    }
    // Explicit extended paths support the same long absolute names that Rust's
    // file API supports, without changing any machine-wide path/security setting.
    for unit in &mut text {
        if *unit == b'/' as u16 {
            *unit = b'\\' as u16;
        }
    }
    let extended = if text.starts_with(&[92, 92, 63, 92]) {
        text
    } else if text.starts_with(&[92, 92]) {
        [
            "\\\\?\\UNC\\".encode_utf16().collect::<Vec<_>>(),
            text[2..].to_vec(),
        ]
        .concat()
    } else {
        ["\\\\?\\".encode_utf16().collect::<Vec<_>>(), text].concat()
    };
    Ok(extended.into_iter().chain(std::iter::once(0)).collect())
}

pub(super) fn move_file(source: &Path, destination: &Path, replace: bool) -> io::Result<()> {
    let source = wide_path(source)?;
    let destination = wide_path(destination)?;
    let flags = MOVEFILE_WRITE_THROUGH
        | if replace {
            MOVEFILE_REPLACE_EXISTING
        } else {
            0
        };
    // COPY_ALLOWED is deliberately absent: the private stage shares the target
    // directory. No-replace publication refuses any existing target atomically.
    // Data was already flushed on the producer's write handle before this move.
    let success = unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), flags) };
    if success == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
