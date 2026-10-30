//! Where Klipolo runs from on Linux (until L): an AppImage (`$APPIMAGE`), judged by `statvfs` and `access`; a Flatpak
//! (`$FLATPAK_ID`) is managed. L: deb and rpm installs (managed too), and AppImage's own updater.

use std::ffi::{CString, OsString};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use crate::install::{InstallPlace, MoveError, PlaceFacts, classify};

fn set(name: &str) -> Option<OsString> {
    std::env::var_os(name).filter(|value| !value.is_empty())
}

/// `$APPIMAGE`; inside a Flatpak the running executable (its place is `Managed`); else None (a dev run).
pub(crate) fn bundle() -> Option<PathBuf> {
    if let Some(image) = set("APPIMAGE") {
        return Some(image.into());
    }
    set("FLATPAK_ID").and_then(|_| std::env::current_exe().ok())
}

/// `statvfs`'s `ST_RDONLY` and `access(W_OK)` on the bundle and its folder. One file system's `rename(2)` replaces a
/// file atomically, so the swap is atomic wherever the copy is writable (L: FUSE and exFAT mounts). Unknown when the
/// volume can't be read, unless the copy is managed.
pub(crate) fn install_place(bundle: &Path) -> InstallPlace {
    let managed = set("FLATPAK_ID").is_some();
    let Some(read_only) = read_only(bundle) else {
        return if managed {
            InstallPlace::Managed
        } else {
            InstallPlace::Unknown
        };
    };
    let facts = PlaceFacts {
        parent_writable: bundle.parent().is_some_and(writable),
        bundle_writable: writable(bundle),
        read_only_volume: read_only,
        managed,
        atomic_swap: true,
        // L: `chattr +i` and network mounts.
        locked: false,
        network: false,
    };
    classify(bundle, facts)
}

fn c_path(path: &Path) -> Option<CString> {
    CString::new(path.as_os_str().as_bytes()).ok()
}

fn read_only(path: &Path) -> Option<bool> {
    let path = c_path(path)?;
    // SAFETY: a plain C struct, all zeroes.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: a NUL-terminated path, and the struct `statvfs` fills.
    (unsafe { libc::statvfs(path.as_ptr(), &mut st) } == 0)
        .then_some(st.f_flag & libc::ST_RDONLY != 0)
}

fn writable(path: &Path) -> bool {
    // SAFETY: a NUL-terminated path.
    c_path(path).is_some_and(|p| unsafe { libc::access(p.as_ptr(), libc::W_OK) } == 0)
}

/// None on Linux: a package manager, Flatpak or the AppImage's own folder places the app.
pub(crate) fn move_to_applications(
    _bundle: &Path,
    _than: &dyn Fn(&str) -> std::cmp::Ordering,
) -> Result<PathBuf, MoveError> {
    Err(MoveError::Io(io::ErrorKind::Unsupported.into()))
}
