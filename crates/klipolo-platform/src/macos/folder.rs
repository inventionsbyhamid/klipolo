//! Folders on macOS: the Trash (its names, and moving a folder there), a folder held by descriptor (it follows renames
//! and moves), and a volume's UUID.

use std::io;
use std::path::{Path, PathBuf};

use objc2::rc::Retained;
use objc2_foundation::{NSFileManager, NSURL};

/// No Trash folder by path: every Trash is known by its name (`is_trash_name`).
pub(crate) fn trash_roots() -> Vec<PathBuf> {
    Vec::new()
}

/// `.Trash` (the home folder's, iCloud Drive's) and `.Trashes` (other volumes'): Finder keeps these names.
pub(crate) fn is_trash_name(name: &std::ffi::OsStr) -> bool {
    name == ".Trash" || name == ".Trashes"
}

/// Moves `path` to the Trash as Finder does (`NSFileManager.trashItemAtURL`, so Put Back works): a recording's Delete,
/// which asks first, and Start over's take. `folder::move_to_trash` calls it. Where it went in the Trash when
/// the OS says.
pub(crate) fn move_to_trash(path: &Path) -> io::Result<Option<PathBuf>> {
    let url = NSURL::from_file_path(path)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not an absolute path"))?;
    let mut went: Option<Retained<NSURL>> = None;
    NSFileManager::defaultManager()
        .trashItemAtURL_resultingItemURL_error(&url, Some(&mut went))
        .map_err(|e| {
            // NSFileNoSuchFileError and NSFileReadNoSuchFileError: the folder is already gone. The kinds the person can
            // act on keep their meaning: NSFileWriteNoPermissionError, NSFileWriteOutOfSpaceError.
            let kind = match e.code() {
                4 | 260 => io::ErrorKind::NotFound,
                513 => io::ErrorKind::PermissionDenied,
                640 => io::ErrorKind::StorageFull,
                _ => io::ErrorKind::Other,
            };
            io::Error::new(kind, e.localizedDescription().to_string())
        })?;
    Ok(went.and_then(|url| url.to_file_path()))
}

/// Puts `trashed` back at `to` (Start over's Undo): a rename on its volume, as Finder's Put Back, never over something
/// already there.
pub(crate) fn put_back(trashed: &Path, to: &Path) -> io::Result<()> {
    crate::unix::rename_new(trashed, to)
}

/// A folder held by descriptor (`folder::HeldFolder`).
pub(crate) struct Held(std::os::fd::OwnedFd);

/// `open(O_EVTONLY | O_DIRECTORY)`: a descriptor that follows the folder and never stops its volume from being ejected.
pub(crate) fn hold_folder(path: &Path) -> io::Result<Option<Held>> {
    use std::os::fd::FromRawFd;
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    loop {
        // SAFETY: a NUL-terminated path; the descriptor returned is owned by `Held` from here on.
        let fd = unsafe {
            libc::open(
                path.as_ptr(),
                libc::O_EVTONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        };
        if fd != -1 {
            // SAFETY: a descriptor just opened, owned by nothing else.
            return Ok(Some(Held(unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) })));
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

/// `F_GETPATH`, kept only while the folder at that path is the held one: a deleted folder may still answer its last
/// place, where a new folder may stand now.
pub(crate) fn locate_folder(held: &Held) -> Option<PathBuf> {
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    let fd = held.0.as_raw_fd();
    let mut buf = [0u8; libc::PATH_MAX as usize];
    // SAFETY: F_GETPATH writes a NUL-terminated path of at most MAXPATHLEN (PATH_MAX) bytes into the buffer.
    if unsafe { libc::fcntl(fd, libc::F_GETPATH, buf.as_mut_ptr()) } == -1 {
        return None;
    }
    let path = std::ffi::CStr::from_bytes_until_nul(&buf).ok()?;
    let path = PathBuf::from(std::ffi::OsStr::from_bytes(path.to_bytes()));
    // SAFETY: a valid open descriptor and a zeroed out-struct.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd, &mut st) } != 0 {
        return None;
    }
    let there = std::fs::metadata(&path).ok()?;
    let same = there.is_dir() && there.dev() == st.st_dev as u64 && there.ino() == st.st_ino;
    same.then_some(path)
}

/// The UUID of the volume `path` is on (`uuid:<32 hex digits>`), which the volume keeps across mounts:
/// `getattrlist(ATTR_VOL_UUID)` on its mount point (`statfs`). None when either call fails or the volume has none.
pub(crate) fn volume_name(path: &Path) -> Option<String> {
    use std::os::unix::ffi::OsStrExt;
    /// What `getattrlist` writes for `ATTR_VOL_UUID` alone: the length, then the UUID.
    #[repr(C)]
    struct VolumeUuid {
        length: u32,
        uuid: [u8; 16],
    }
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: a plain C struct, all zeroes.
    let mut fs: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: a NUL-terminated path, and the struct `statfs` fills.
    if unsafe { libc::statfs(path.as_ptr(), &mut fs) } != 0 {
        return None;
    }
    let mut list = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: 0,
        volattr: libc::ATTR_VOL_INFO | libc::ATTR_VOL_UUID,
        dirattr: 0,
        fileattr: 0,
        forkattr: 0,
    };
    let mut out = VolumeUuid {
        length: 0,
        uuid: [0; 16],
    };
    // SAFETY: the mount point `statfs` wrote (NUL-terminated within its array), the attributes asked for, and a buffer
    // whose size goes with it (the call writes at most that).
    let read = unsafe {
        libc::getattrlist(
            fs.f_mntonname.as_ptr(),
            (&mut list as *mut libc::attrlist).cast(),
            (&mut out as *mut VolumeUuid).cast(),
            std::mem::size_of::<VolumeUuid>(),
            0,
        )
    } == 0;
    let whole = out.length as usize >= std::mem::size_of::<VolumeUuid>();
    (read && whole && out.uuid != [0; 16]).then(|| {
        let digits: String = out.uuid.iter().map(|b| format!("{b:02x}")).collect();
        format!("uuid:{digits}")
    })
}

#[cfg(test)]
mod tests {
    /// It moves a temp folder into this Mac's own Trash (and deletes it there at once), so the ordinary suite never
    /// runs it: the user's Trash is left alone. Run it by hand on a Mac:
    /// `cargo test -p klipolo-platform --lib -- --ignored a_folder_goes_to_the_trash`.
    #[test]
    #[ignore = "touches this Mac's real Trash: run by hand with --ignored"]
    fn a_folder_goes_to_the_trash() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("take.klipolorec");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("journal.jsonl"), b"{}\n").unwrap();
        let went = super::move_to_trash(&folder).unwrap();
        assert!(!folder.exists(), "gone from its place");
        // It went to the Trash; the test leaves nothing there.
        let went = went.expect("the Trash says where it went");
        assert!(went.join("journal.jsonl").exists(), "{}", went.display());
        std::fs::remove_dir_all(went).unwrap();
    }
}
