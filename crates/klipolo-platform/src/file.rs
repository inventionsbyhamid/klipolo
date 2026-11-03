//! Positioned reads and writes: the PCM cache writes each chunk where it belongs while the mixer reads others. macOS
//! and Linux use `pread`/`pwrite` (the file's cursor doesn't move); Windows uses `seek_read`/`seek_write`, which move
//! it, so callers never mix these with cursor-based reads and writes on the same file.
//!
//! Also the read-ahead hint for a mapped file (`will_need`), a file's identity (`identity`), and a small secret only
//! this user can read (`write_private`), read back only as a file of its own (`read_private`).

use std::fs::File;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Creates a small file (a model server's API key or pidfile) that only this user can read. It never replaces anything:
/// a file or a symbolic link already at `path` is `AlreadyExists`, and nothing is written through it (`O_CREAT |
/// O_EXCL | O_NOFOLLOW`).
/// - macOS and Linux: mode 0600;
/// - Windows: the file is created inside the per-user app-data folder, whose ACL already excludes other users (an
///   explicit DACL is still to come).
pub fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    io::Write::write_all(&mut options.open(path)?, bytes)
}

/// Reads at most `most` bytes of a small file `write_private` made, only as a regular file of its own: never through a
/// symbolic link, and never waiting on a FIFO or a device put at its name. Anything but a regular file is
/// `InvalidInput`; a file that isn't this user's alone is `PermissionDenied`.
/// - macOS and Linux: `O_NOFOLLOW | O_NONBLOCK` (a FIFO opens at once instead of waiting for a writer), then the open
///   file must be a regular file, owned by this process's effective user, that no one else may read or write (mode
///   0600 or narrower, as `write_private` makes it): one another user put in a shared folder is never read;
/// - Windows: the open file must be a regular file.
pub fn read_private(path: &Path, most: u64) -> io::Result<Vec<u8>> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no preconditions and can't fail.
        let me = unsafe { libc::geteuid() };
        if meta.uid() != me || meta.mode() & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "not a file of this user's alone",
            ));
        }
    }
    let mut bytes = Vec::new();
    io::Read::read_to_end(&mut io::Read::take(file, most), &mut bytes)?;
    Ok(bytes)
}

/// Tells the OS that `bytes`, part of a memory-mapped file, will be read soon, so it pages them in ahead of the reader
/// (the PCM cache after a seek). Only a hint: nothing changes if it can't.
/// - macOS and Linux: `madvise(MADV_WILLNEED)`;
/// - Windows (later: `PrefetchVirtualMemory`): nothing yet.
pub fn will_need(bytes: &[u8]) {
    crate::imp::will_need(bytes);
}

/// What identifies a file itself rather than its name: its volume and its number there. A model store keeps it beside
/// the size and modification time, so a model whose size and time match but which was replaced is checked again.
///
/// Its limits:
/// - **The same identity doesn't prove the same bytes.** A file rewritten in place keeps it (the size and time catch
///   that), and ext4 and XFS give a deleted file's number to a new one.
/// - **The same file can get another identity**, which only costs a needless check:
///   - volume numbers are handed out at mount, so a removable, network, LVM or btrfs volume may get another after a
///     reboot or a remount;
///   - FAT, exFAT, SMB and some FUSE file systems make file numbers up per mount;
///   - on Windows, FAT's file index changes when the file moves, and ReFS's 128-bit ids don't fit the 64-bit index
///     (later: `GetFileInformationByHandleEx(FileIdInfo)`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileIdentity {
    /// macOS and Linux: `st_dev`; Windows: the volume serial number.
    pub volume: u64,
    /// macOS and Linux: the inode (`st_ino`); Windows: the file index.
    pub file: u64,
}

/// `file`'s identity, or None when the OS won't say. It takes the open file, because std can't read the Windows
/// volume serial and file index from a `Metadata` yet (`MetadataExt::volume_serial_number` and `file_index` are
/// unstable); read the size and time from the same file (`file.metadata()`), so all three describe one file.
/// - macOS and Linux: `fstat`'s `st_dev` and `st_ino`;
/// - Windows: `GetFileInformationByHandle`'s `dwVolumeSerialNumber` and `nFileIndexHigh`/`Low`.
pub fn identity(file: &File) -> Option<FileIdentity> {
    crate::imp::file_identity(file)
}

/// What identifies a folder (or file) wherever its volume is mounted: its volume, by the UUID the volume keeps where
/// the OS gives one, and its number there. A drive plugged in again, or in another order, keeps it, where `st_dev`
/// doesn't (macOS numbers devices per mount). Where no UUID can be read, the device number stands in, so the identity
/// may change at a remount: callers must treat a mismatch as "maybe not the same", never the other way.
/// - macOS: `getattrlist`'s `ATTR_VOL_UUID` on the volume's mount point (`statfs`'s `f_mntonname`);
/// - Linux: `statvfs`'s `f_fsid`, which ext4, XFS and btrfs derive from the file system's UUID;
/// - Windows: none yet (the volume serial and file index need a handle opened with backup semantics).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PlacedIdentity {
    /// `uuid:<32 hex digits>` (macOS), `fsid:<hex>` (Linux), or `dev:<n>` where neither reads.
    pub volume: String,
    /// The inode (`st_ino`).
    pub file: u64,
}

/// `path`'s [`PlacedIdentity`], or None when the OS won't say (it is gone, or on Windows for now). Two stats, and on
/// macOS one `getattrlist`: call it when a folder is opened or compared, not per frame.
pub fn placed_identity(path: &Path) -> Option<PlacedIdentity> {
    crate::imp::placed_identity(path)
}

pub trait PositionedIo {
    /// Writes all of `buf` at `offset`.
    fn write_all_at(&self, buf: &[u8], offset: u64) -> io::Result<()>;
    /// Fills `buf` from `offset` (UnexpectedEof when the file ends first).
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()>;
}

impl PositionedIo for File {
    fn write_all_at(&self, buf: &[u8], offset: u64) -> io::Result<()> {
        crate::imp::write_all_at(self, buf, offset)
    }

    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        crate::imp::read_exact_at(self, buf, offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder's identity names its volume by the UUID it keeps where the OS gives one (macOS),
    /// reads the same each time, and differs for another folder there. A path that isn't there has none.
    #[cfg(unix)]
    #[test]
    fn a_folder_is_known_by_its_volume_and_its_number_there() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();
        let first = placed_identity(&a).unwrap();
        assert_eq!(placed_identity(&a), Some(first.clone()));
        let other = placed_identity(&b).unwrap();
        assert_eq!(other.volume, first.volume);
        assert_ne!(other.file, first.file);
        #[cfg(target_os = "macos")]
        assert!(
            first.volume.starts_with("uuid:") && first.volume.len() == 37,
            "{}",
            first.volume
        );
        assert!(placed_identity(&dir.path().join("gone")).is_none());
    }

    #[test]
    fn positioned_writes_land_where_asked_and_reads_past_the_end_fail() {
        let dir = tempfile::tempdir().unwrap();
        let file = File::options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(dir.path().join("pcm"))
            .unwrap();
        file.write_all_at(b"world", 6).unwrap();
        file.write_all_at(b"hello ", 0).unwrap();
        let mut buf = [0u8; 11];
        file.read_exact_at(&mut buf, 0).unwrap();
        assert_eq!(&buf, b"hello world");
        let e = file.read_exact_at(&mut buf, 4).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[cfg(unix)]
    #[test]
    fn a_private_file_is_this_users_alone_and_never_replaces_or_follows_anything() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("key");
        write_private(&path, b"secret").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"secret");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let again = write_private(&path, b"other").unwrap_err();
        assert_eq!(again.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&path).unwrap(), b"secret");
        let target = dir.path().join("elsewhere");
        std::fs::write(&target, b"untouched").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(write_private(&link, b"secret").is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"untouched");
        // A link to nowhere isn't followed into a new file either.
        let dangling = dir.path().join("dangling");
        std::os::unix::fs::symlink(dir.path().join("created"), &dangling).unwrap();
        assert!(write_private(&dangling, b"secret").is_err());
        assert!(!dir.path().join("created").exists());
    }

    /// A private file reads back, at most `most` bytes of it. A link, a folder or a FIFO put in its place is refused,
    /// a FIFO at once rather than waiting for a writer that never comes.
    #[cfg(unix)]
    #[test]
    fn a_private_file_reads_only_as_a_file_of_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("key");
        write_private(&path, b"secret").unwrap();
        assert_eq!(read_private(&path, 64).unwrap(), b"secret");
        assert_eq!(read_private(&path, 3).unwrap(), b"sec");
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(read_private(&link, 64).is_err(), "a link");
        let folder = dir.path().join("folder");
        std::fs::create_dir(&folder).unwrap();
        assert!(read_private(&folder, 64).is_err(), "a folder");
        let fifo = dir.path().join("fifo");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        assert!(made.success());
        // On a thread, so a read that waits fails the test instead of hanging it.
        let (said, heard) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = said.send(read_private(&fifo, 64).is_err());
        });
        let refused = heard.recv_timeout(std::time::Duration::from_secs(2));
        assert_eq!(refused, Ok(true), "a FIFO, refused at once");
        assert!(read_private(&dir.path().join("gone"), 64).is_err());
    }

    /// A private file is this user's alone: one others may read or write (mode past 0600), or one another user owns,
    /// is refused, however it reads otherwise. Owners are tested where the test may give the file away (as root).
    #[cfg(unix)]
    #[test]
    fn a_private_file_is_refused_when_others_may_open_it_or_own_it() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("key");
        write_private(&path, b"secret").unwrap();
        for mode in [0o644, 0o640, 0o604, 0o620, 0o602, 0o610] {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            let refused = read_private(&path, 64).unwrap_err();
            assert_eq!(refused.kind(), io::ErrorKind::PermissionDenied, "{mode:o}");
        }
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).unwrap();
        assert_eq!(
            read_private(&path, 64).unwrap(),
            b"secret",
            "read only by its owner"
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        // SAFETY: geteuid has no preconditions and can't fail.
        if unsafe { libc::geteuid() } == 0 {
            let given = std::process::Command::new("chown")
                .arg("1")
                .arg(&path)
                .status()
                .unwrap();
            assert!(given.success());
            let refused = read_private(&path, 64).unwrap_err();
            assert_eq!(
                refused.kind(),
                io::ErrorKind::PermissionDenied,
                "another user's"
            );
        }
    }

    #[test]
    fn a_replaced_file_has_another_identity() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model");
        std::fs::write(&path, b"one").unwrap();
        // Held open, so its number can't be handed to the replacement.
        let first = File::open(&path).unwrap();
        let id = identity(&first).expect("an identity");
        assert_eq!(
            identity(&File::open(&path).unwrap()),
            Some(id),
            "the same file"
        );
        std::fs::write(dir.path().join("next"), b"two").unwrap();
        std::fs::rename(dir.path().join("next"), &path).unwrap();
        assert_ne!(
            identity(&File::open(&path).unwrap()),
            Some(id),
            "a replacement"
        );
    }
}
