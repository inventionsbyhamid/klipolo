//! Folders read through handles: a folder is opened from the handle of the one above it
//! (`openat`), never by its whole path again, and never through a link at any level (`O_NOFOLLOW`, with `O_DIRECTORY`
//! for folders), so a folder swapped for a link after it was listed is refused rather than followed. Files open the same
//! way (`O_NONBLOCK` too, so a FIFO doesn't hang the open). Windows (until W) checks each step's metadata first and
//! opens the reparse point itself, as the library's reader does.

use std::io;

/// What a link, a FIFO, a device or another non-plain entry opens as: the caller refuses it as a link.
#[derive(Debug)]
pub struct NotPlain;

impl std::fmt::Display for NotPlain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("not a plain folder or file")
    }
}

impl std::error::Error for NotPlain {}

fn not_plain() -> io::Error {
    io::Error::other(NotPlain)
}

#[cfg(unix)]
pub use unix::Dir;

#[cfg(unix)]
mod unix {
    use std::ffi::{CStr, CString, OsStr, OsString};
    use std::fs::File;
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::path::Path;

    const DIR: libc::c_int =
        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    const FILE: libc::c_int =
        libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC;

    /// An open folder.
    pub struct Dir(OwnedFd);

    /// `fd` owned, or the OS's error: a link (`ELOOP`) or a file where a folder was asked (`ENOTDIR`) as `NotPlain`.
    fn owned(fd: libc::c_int) -> io::Result<OwnedFd> {
        if fd >= 0 {
            // SAFETY: a descriptor the call just opened, owned by nothing else.
            return Ok(unsafe { OwnedFd::from_raw_fd(fd) });
        }
        let e = io::Error::last_os_error();
        match e.raw_os_error() {
            Some(libc::ELOOP | libc::ENOTDIR) => Err(super::not_plain()),
            _ => Err(e),
        }
    }

    fn c_name(name: &OsStr) -> io::Result<CString> {
        CString::new(name.as_bytes()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))
    }

    /// A name with no `/` and not `.` or `..`, as a C string.
    fn plain(name: &str) -> io::Result<CString> {
        if name.is_empty() || name.contains('/') || name == "." || name == ".." {
            return Err(io::Error::from(io::ErrorKind::InvalidInput));
        }
        c_name(OsStr::new(name))
    }

    impl Dir {
        /// The folder at `path`, which itself mustn't be a link.
        pub fn open(path: &Path) -> io::Result<Self> {
            let path = c_name(path.as_os_str())?;
            // SAFETY: a NUL-terminated path.
            owned(unsafe { libc::open(path.as_ptr(), DIR) }).map(Self)
        }

        /// The folder `name` in this one (one plain name, never a path).
        pub fn sub(&self, name: &str) -> io::Result<Self> {
            let name = plain(name)?;
            // SAFETY: an open folder's descriptor and a NUL-terminated name.
            owned(unsafe { libc::openat(self.0.as_raw_fd(), name.as_ptr(), DIR) }).map(Self)
        }

        /// The file `name` in this one, to read.
        pub fn file(&self, name: &str) -> io::Result<File> {
            let name = plain(name)?;
            // SAFETY: as in `sub`.
            owned(unsafe { libc::openat(self.0.as_raw_fd(), name.as_ptr(), FILE) }).map(File::from)
        }

        /// The names it holds (`.` and `..` left out), at most `max`; true when it holds more.
        pub fn names(&self, max: usize) -> io::Result<(Vec<OsString>, bool)> {
            // SAFETY: duplicating an open descriptor.
            let fd = owned(unsafe { libc::dup(self.0.as_raw_fd()) })?;
            // SAFETY: `fdopendir` takes the duplicate over when it succeeds.
            let stream = unsafe { libc::fdopendir(fd.as_raw_fd()) };
            if stream.is_null() {
                return Err(io::Error::last_os_error());
            }
            std::mem::forget(fd);
            let mut names = Vec::new();
            let mut more = false;
            // SAFETY: `stream` is open until `closedir` below; each entry is copied before the next `readdir`.
            unsafe {
                libc::rewinddir(stream);
                loop {
                    let entry = libc::readdir(stream);
                    if entry.is_null() {
                        break;
                    }
                    let name = CStr::from_ptr((*entry).d_name.as_ptr()).to_bytes();
                    if name == b"." || name == b".." {
                        continue;
                    }
                    if names.len() == max {
                        more = true;
                        break;
                    }
                    names.push(OsString::from_vec(name.to_vec()));
                }
                libc::closedir(stream);
            }
            Ok((names, more))
        }
    }
}

#[cfg(not(unix))]
pub use other::Dir;

#[cfg(not(unix))]
mod other {
    use std::ffi::OsString;
    use std::fs::File;
    use std::io;
    use std::path::{Path, PathBuf};

    use super::super::super::read::{is_link, open_no_follow};

    /// A folder, by its path, checked at each step.
    pub struct Dir(PathBuf);

    impl Dir {
        pub fn open(path: &Path) -> io::Result<Self> {
            let meta = std::fs::symlink_metadata(path)?;
            if is_link(&meta) || !meta.is_dir() {
                return Err(super::not_plain());
            }
            Ok(Self(path.to_path_buf()))
        }

        pub fn sub(&self, name: &str) -> io::Result<Self> {
            Self::open(&self.0.join(name))
        }

        pub fn file(&self, name: &str) -> io::Result<File> {
            let path = self.0.join(name);
            let meta = std::fs::symlink_metadata(&path)?;
            if is_link(&meta) || !meta.is_file() {
                return Err(super::not_plain());
            }
            let file = open_no_follow(&path)?;
            if is_link(&file.metadata()?) {
                return Err(super::not_plain());
            }
            Ok(file)
        }

        pub fn names(&self, max: usize) -> io::Result<(Vec<OsString>, bool)> {
            let mut names = Vec::new();
            for entry in std::fs::read_dir(&self.0)? {
                if names.len() == max {
                    return Ok((names, true));
                }
                names.push(entry?.file_name());
            }
            Ok((names, false))
        }
    }
}
