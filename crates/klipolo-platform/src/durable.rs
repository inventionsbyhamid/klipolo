//! Files that survive a crash or a power cut, and file errors in terms the user can act on.
//!
//! `write_atomic` writes a temporary file beside the target, pushes it to the disk, renames it over the target, then
//! makes the rename itself durable, so a crash leaves the old file or the new one, never half:
//! - macOS: `F_BARRIERFSYNC` (ordered after every earlier write, about 3 ms) or `F_FULLFSYNC`; the barrier orders the
//!   rename after the data, so the folder needs no sync;
//! - Linux: `fdatasync` or `fsync`, then an `fsync` of the folder (the rename isn't durable until then);
//! - Windows: `FlushFileBuffers` either way; NTFS journals the rename (retries for antivirus locks are still to come).
//!
//! The steps go through `Steps`, so tests record them or inject a full disk (`fake::RecordingDurable`).

use std::fs::File;
use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};

/// How hard a write is pushed to the disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Durability {
    /// Ordered after every earlier write: a crash leaves the old or the new file, never half. Cheap on macOS
    /// (`F_BARRIERFSYNC`); a full flush elsewhere (`fdatasync`, `FlushFileBuffers`).
    Ordered,
    /// On the disk itself, even across a power cut (`F_FULLFSYNC`, `fsync`, `FlushFileBuffers`).
    Full,
}

/// Pushes `file`'s writes to the disk. `file` must be open for writing (Windows' `FlushFileBuffers` needs
/// `GENERIC_WRITE`).
pub fn sync(file: &File, durability: Durability) -> io::Result<()> {
    crate::imp::sync(file, durability)
}

/// Moves `file`'s data to the drive without draining the drive's cache: macOS `fsync` (Rust's `sync_data` is
/// F_FULLFSYNC there), Linux `fdatasync`, Windows `FlushFileBuffers`. On Linux and Windows that also reaches the disk
/// itself (both flush the drive's cache); on macOS a later `sync(.., Full)` of any file on the drive does (the recorder's
/// one flush a second, `klipolo-capture`'s durable writer).
pub fn push(file: &File) -> io::Result<()> {
    crate::imp::push(file)
}

/// Orders every write `push`ed before it ahead of every write after it, on the whole drive, without draining the
/// drive's cache: macOS `F_BARRIERFSYNC` (a plain `fsync` where the file system has no barrier). So a record written
/// after it (the recorder's `Durable` line) never reaches the disk before the data it vouches for, even if the power
/// goes before the next full flush. Linux and Windows have nothing to order: their `push` already reached the disk.
pub fn order(file: &File) -> io::Result<()> {
    crate::imp::order(file)
}

/// Makes the entries of `dir` (a rename into it) durable: Linux syncs the folder; macOS and Windows need nothing.
pub fn fsync_dir(dir: &Path) -> io::Result<()> {
    crate::imp::fsync_dir(dir)
}

/// Why a file operation failed, as far as the user can act on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiskReason {
    /// No space left (a quota or a file-size limit stops the same way).
    Full,
    /// The disk is read-only or write-protected.
    ReadOnly,
    /// Not allowed to write there.
    Denied,
    /// Another program holds the file (Windows sharing violations).
    Locked,
    Other,
}

/// Classifies an OS error (errno on macOS and Linux, a Win32 error code on Windows). An error without an OS code
/// (from `std` or a fake) is classified by its kind.
pub fn classify(e: &io::Error) -> DiskReason {
    match e.raw_os_error() {
        Some(code) => crate::imp::classify_os(code),
        None => match e.kind() {
            ErrorKind::StorageFull => DiskReason::Full,
            ErrorKind::ReadOnlyFilesystem => DiskReason::ReadOnly,
            ErrorKind::PermissionDenied => DiskReason::Denied,
            ErrorKind::ResourceBusy => DiskReason::Locked,
            _ => DiskReason::Other,
        },
    }
}

/// Classifies a C runtime `errno` (what FFmpeg's `AVERROR` carries; on Windows these aren't Win32 codes).
pub fn classify_errno(errno: i32) -> DiskReason {
    crate::imp::classify_errno(errno)
}

/// `a` and `b` (or their nearest existing folders) are on one volume, so a full disk fills both (None: unknown).
/// - macOS and Linux: the device number (`st_dev`);
/// - Windows (later: `GetVolumeInformationByHandleW`'s serial number): unknown until then.
pub fn same_volume(a: &Path, b: &Path) -> Option<bool> {
    let volume = |path: &Path| {
        let existing = path.ancestors().find(|p| p.exists())?;
        crate::imp::volume_id(existing)
    };
    Some(volume(a)? == volume(b)?)
}

/// Bytes this user may still write on the volume holding `path` (None: unknown).
pub fn free_space(path: &Path) -> Option<u64> {
    crate::imp::free_space(path)
}

/// The steps of an atomic write. `Os` does them for real; `fake::RecordingDurable` records them.
pub trait Steps {
    /// Creates `path` (replacing it) and writes `bytes`.
    fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<File>;
    fn sync(&self, file: &File, durability: Durability) -> io::Result<()>;
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()>;
    fn fsync_dir(&self, dir: &Path) -> io::Result<()>;
}

/// This OS's file steps.
#[derive(Clone, Copy, Debug, Default)]
pub struct Os;

impl Steps for Os {
    fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<File> {
        let mut file = File::create(path)?;
        file.write_all(bytes)?;
        Ok(file)
    }

    fn sync(&self, file: &File, durability: Durability) -> io::Result<()> {
        sync(file, durability)
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        std::fs::rename(from, to)
    }

    fn fsync_dir(&self, dir: &Path) -> io::Result<()> {
        fsync_dir(dir)
    }
}

/// The temporary file `write_atomic` writes first: `<name>.<pid>.tmp` beside the target, so two processes writing
/// the same file (an export worker and one left over from before a quit) never share one.
pub fn temp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.tmp", std::process::id()));
    path.with_file_name(name)
}

/// The process writing the temporary file named `name` (`temp_path`'s `<name>.<pid>.tmp`); None for any other name.
pub fn temp_owner(name: &str) -> Option<u32> {
    name.strip_suffix(".tmp")?.rsplit('.').next()?.parse().ok()
}

/// Replaces `path` with `bytes` so a crash or power cut leaves the old file or the new one, never half. A failed
/// write removes the temporary file.
pub fn write_atomic(path: &Path, bytes: &[u8], durability: Durability) -> io::Result<()> {
    write_atomic_with(&Os, path, bytes, durability)
}

/// `write_atomic` through `steps`.
pub fn write_atomic_with(
    steps: &dyn Steps,
    path: &Path,
    bytes: &[u8],
    durability: Durability,
) -> io::Result<()> {
    let tmp = temp_path(path);
    let written = steps.write(&tmp, bytes).and_then(|file| {
        steps.sync(&file, durability)?;
        drop(file);
        steps.rename(&tmp, path)
    });
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => steps.fsync_dir(dir),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{Call, Fault, RecordingDurable, Step};

    /// Two folders of one temporary folder share a volume; one not made yet counts by its nearest existing folder.
    #[test]
    fn folders_on_one_volume_are_known_as_such() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        std::fs::create_dir(&a).unwrap();
        let expected = if cfg!(unix) { Some(true) } else { None };
        assert_eq!(same_volume(&a, &dir.path().join("b").join("c")), expected);
    }

    /// The recorder's tick (klipolo-capture's durable writer): push, order, then one full flush, on a file open to append.
    #[test]
    fn a_push_an_order_and_a_full_sync_work_on_an_appended_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(dir.path().join("run-01.caf"))
            .unwrap();
        file.write_all(b"caff").unwrap();
        push(&file).unwrap();
        order(&file).unwrap();
        sync(&file, Durability::Full).unwrap();
    }

    #[test]
    fn an_atomic_write_replaces_the_file_in_order_and_leaves_no_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("project.klipolo");
        std::fs::write(&file, b"old").unwrap();
        write_atomic(&file, b"new", Durability::Ordered).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"new");
        assert!(!temp_path(&file).exists());
        assert_eq!(
            temp_path(&file),
            dir.path()
                .join(format!("project.klipolo.{}.tmp", std::process::id()))
        );
        assert_eq!(
            temp_owner(&format!("project.klipolo.{}.tmp", std::process::id())),
            Some(std::process::id())
        );
        assert_eq!(temp_owner("project.klipolo.tmp"), None);
        assert!(free_space(dir.path()).is_some_and(|bytes| bytes > 0));

        let steps = RecordingDurable::new();
        write_atomic_with(&steps, &file, b"newer", Durability::Full).unwrap();
        let tmp = temp_path(&file);
        assert_eq!(
            steps.calls(),
            [
                Call::Write(tmp.clone()),
                Call::Sync(Durability::Full),
                Call::Rename(tmp, file.clone()),
                Call::FsyncDir(dir.path().to_path_buf()),
            ]
        );
        assert_eq!(std::fs::read(&file).unwrap(), b"newer");
    }

    #[test]
    fn a_full_disk_keeps_the_old_file_and_reads_as_full() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("job.json");
        std::fs::write(&file, b"old").unwrap();
        let steps = RecordingDurable::failing(Step::Sync, Fault::Full);
        let e = write_atomic_with(&steps, &file, b"new", Durability::Ordered).unwrap_err();
        assert_eq!(classify(&e), DiskReason::Full);
        assert_eq!(std::fs::read(&file).unwrap(), b"old");
        assert!(!temp_path(&file).exists(), "the temporary file is removed");
        assert_eq!(steps.calls().len(), 2, "no rename after a failed sync");

        let locked = RecordingDurable::failing(Step::Rename, Fault::Locked);
        let e = write_atomic_with(&locked, &file, b"new", Durability::Ordered).unwrap_err();
        assert_eq!(classify(&e), DiskReason::Locked);

        #[cfg(unix)]
        {
            let enospc = io::Error::from_raw_os_error(libc::ENOSPC);
            assert_eq!(classify(&enospc), DiskReason::Full);
            assert_eq!(classify_errno(libc::EDQUOT), DiskReason::Full);
            assert_eq!(classify_errno(libc::EROFS), DiskReason::ReadOnly);
            assert_eq!(classify_errno(libc::EACCES), DiskReason::Denied);
            assert_eq!(classify_errno(libc::EINVAL), DiskReason::Other);
        }
    }
}
