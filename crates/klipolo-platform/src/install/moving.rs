//! Move to Applications' steps, the same wherever an OS has the move (macOS): what is at
//! the target decides first, the copy is made beside it under a hidden name, what is at the target is read again, an
//! older Klipolo goes to the Trash, and the copy is renamed into place last. The OS's own steps are a `Mover` (macOS:
//! `ditto` and `xattr`, NSFileManager's Trash, `renamex_np`); the tests use plain folders.
//!
//! What a failure halfway leaves, so there is always one working Klipolo where the person can find it and never two
//! that fight (failure first):
//! - before the copy: nothing changed (a newer Klipolo or another app with the name stays, a disk without room for the
//!   copy says so, and nothing is copied);
//! - the copy fails: it is deleted (or swept by the next attempt after a kill), and the older Klipolo was never touched;
//! - something else was put at the target during the copy: it is judged again as it is now;
//! - the Trash refuses the older Klipolo: the copy is deleted, and the older one stays;
//! - the rename fails: the older Klipolo is put back from the Trash. If that fails too it stays in the Trash, where Put
//!   Back still works: it is logged and said (`LeftInTrash`).
//!
//! The running Klipolo (on the disk image, or translocated) is only read, never moved: until the copy is in place it is
//! the working one.

use std::cmp::Ordering;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use super::MoveError;

/// The name the copy always takes: "Klipolo 2.app" from Downloads still lands as Klipolo.app, so a second
/// Klipolo never appears beside the first.
pub(crate) const APP_NAME: &str = "Klipolo.app";

/// What an app's Info.plist says about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BundleInfo {
    /// `CFBundleIdentifier`.
    pub id: String,
    /// `CFBundleShortVersionString`; None when it has none.
    pub version: Option<String>,
}

/// The OS's steps.
pub(crate) trait Mover {
    /// `app`'s Info.plist; None when it has none (not an app).
    fn info(&self, app: &Path) -> Option<BundleInfo>;
    /// Bytes free on `dir`'s volume; None when the OS can't say (the copy is then tried).
    fn free_space(&self, dir: &Path) -> Option<u64>;
    /// `app` still carries the quarantine of a download (macOS: `com.apple.quarantine`), so it would run translocated.
    fn quarantined(&self, app: &Path) -> bool;
    /// Copies the bundle `from` to the new path `to`, ready to open (macOS: `ditto`, then the quarantine cleared).
    fn copy(&self, from: &Path, to: &Path) -> io::Result<()>;
    /// Moves `path` to the Trash, where the person can put it back; where it went, when the OS says.
    fn trash(&self, path: &Path) -> io::Result<Option<PathBuf>>;
    /// Puts `trashed` back at `to`, never over something there.
    fn put_back(&self, trashed: &Path, to: &Path) -> io::Result<()>;
    /// Renames `from` to `to`, never over something that appeared at `to` meanwhile.
    fn rename_new(&self, from: &Path, to: &Path) -> io::Result<()>;
}

/// Which Applications folder the copy goes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Folder {
    /// /Applications.
    System,
    /// ~/Applications, for an account that may not write /Applications.
    Home,
}

/// Where the move goes: /Applications when this user may write there. Otherwise ~/Applications, unless
/// a Klipolo (`id`) is already in /Applications (`found`, its Info.plist): an administrator's copy. A newer one says
/// so, and any other says to open that one, so a standard account never gets a second Klipolo beside it. `than`
/// compares a version found with this Klipolo's.
pub(crate) fn folder_for(
    system_writable: bool,
    found: Option<BundleInfo>,
    id: &str,
    than: &dyn Fn(&str) -> Ordering,
) -> Result<Folder, MoveError> {
    if system_writable {
        return Ok(Folder::System);
    }
    match found {
        Some(info) if info.id == id => match info.version {
            Some(v) if than(&v) == Ordering::Greater => Err(MoveError::NewerInstalled(v)),
            _ => Err(MoveError::InApplications),
        },
        _ => Ok(Folder::Home),
    }
}

/// What is at the target, judged.
enum Judged {
    /// Nothing: the copy takes the name.
    Empty,
    /// This very version of Klipolo, ready to open: already moved, so nothing is copied or trashed. A copy still
    /// quarantined (put there by `cp` or an unarchiver) would run translocated, so it counts as Older.
    Same,
    /// An older Klipolo, or one whose version can't be read: it goes to the Trash.
    Older,
}

/// Copies `bundle` into `dir` as Klipolo.app. A Klipolo already there (the bundle id `id`) is replaced when `than` says
/// its version is older than this one (or it can't be read: it goes to the Trash, so nothing is lost), kept when it
/// is newer (an older disk image never replaces a newer install), and simply opened when it is this very version.
/// Anything else with the name stays. A disk without room for the copy is refused before anything is written. Moving a
/// copy onto itself changes nothing.
pub(crate) fn move_into(
    m: &dyn Mover,
    bundle: &Path,
    dir: &Path,
    id: &str,
    than: &dyn Fn(&str) -> Ordering,
) -> Result<PathBuf, MoveError> {
    let target = dir.join(APP_NAME);
    let same = |a: &Path, b: &Path| matches!((a.canonicalize(), b.canonicalize()), (Ok(a), Ok(b)) if a == b);
    if same(bundle, &target) {
        return Ok(target);
    }
    // Decided before anything is copied.
    let before = judge(m, &target, id, than)?;
    if let Judged::Same = before {
        return Ok(target);
    }
    // A killed attempt's leftovers go first, so they never count against the space.
    sweep(dir);
    // The OS's copy would fail halfway and say only that it failed: a full disk says so first.
    if let (Some(free), Some(size)) = (m.free_space(dir), size_of(bundle))
        && free < size
    {
        return Err(io::Error::from(io::ErrorKind::StorageFull).into());
    }
    let temp = dir.join(temp_name(std::process::id()));
    let _ = std::fs::remove_dir_all(&temp);
    let gone = |e: MoveError| {
        let _ = std::fs::remove_dir_all(&temp);
        Err(e)
    };
    if let Err(e) = m.copy(bundle, &temp) {
        return gone(e.into());
    }
    // Read again just before anything there changes: a Klipolo or another app put there during the copy is judged
    // as it is now, never trashed on the strength of what was there before.
    let replaces = match judge(m, &target, id, than) {
        Ok(Judged::Empty) => false,
        Ok(Judged::Older) => true,
        Ok(Judged::Same) => {
            let _ = std::fs::remove_dir_all(&temp);
            return Ok(target);
        }
        Err(e) => return gone(e),
    };
    let trashed = match if replaces { m.trash(&target) } else { Ok(None) } {
        Ok(trashed) => trashed,
        Err(e) => return gone(e.into()),
    };
    if let Err(e) = m.rename_new(&temp, &target) {
        let _ = std::fs::remove_dir_all(&temp);
        if replaces && !put_back(m, trashed.as_deref(), &target) {
            return Err(MoveError::LeftInTrash(e));
        }
        return Err(e.into());
    }
    Ok(target)
}

/// What is at `target` now: nothing, a Klipolo (`id`) older, the same or newer than this one (`than`), or another app.
fn judge(
    m: &dyn Mover,
    target: &Path,
    id: &str,
    than: &dyn Fn(&str) -> Ordering,
) -> Result<Judged, MoveError> {
    match std::fs::symlink_metadata(target) {
        Ok(_) => match m.info(target) {
            Some(info) if info.id == id => match info.version {
                Some(v) => match than(&v) {
                    Ordering::Greater => Err(MoveError::NewerInstalled(v)),
                    Ordering::Equal if m.quarantined(target) => Ok(Judged::Older),
                    Ordering::Equal => Ok(Judged::Same),
                    Ordering::Less => Ok(Judged::Older),
                },
                None => Ok(Judged::Older),
            },
            _ => Err(MoveError::NameTaken),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Judged::Empty),
        Err(e) => Err(e.into()),
    }
}

/// The bytes a copy of `bundle` needs: its files' sizes, links not followed. None when it can't be read whole.
fn size_of(bundle: &Path) -> Option<u64> {
    let meta = std::fs::symlink_metadata(bundle).ok()?;
    if !meta.is_dir() {
        return Some(meta.len());
    }
    std::fs::read_dir(bundle)
        .ok()?
        .map(|entry| size_of(&entry.ok()?.path()))
        .try_fold(0u64, |sum, size| Some(sum.saturating_add(size?)))
}

/// After a failed rename: the older Klipolo back from the Trash. False (logged, never with a path) when it
/// couldn't be put back, so it is still in the Trash.
fn put_back(m: &dyn Mover, trashed: Option<&Path>, target: &Path) -> bool {
    let Some(older) = trashed else {
        tracing::warn!(
            "move to Applications: the older Klipolo is in the Trash, which didn't say where to put it back from"
        );
        return false;
    };
    match m.put_back(older, target) {
        Ok(()) => true,
        Err(error) => {
            tracing::warn!(%error, "move to Applications: the older Klipolo couldn't be put back from the Trash");
            false
        }
    }
}

/// `.Klipolo.app.moving-`, then the id of the process making the copy: hidden, and swept once that process is gone.
const TEMP_PREFIX: &str = ".Klipolo.app.moving-";

fn temp_name(pid: u32) -> OsString {
    format!("{TEMP_PREFIX}{pid}").into()
}

/// Deletes copies an earlier attempt left when it was killed halfway (its process is gone).
fn sweep(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let pid: Option<u32> = name
            .to_str()
            .and_then(|n| n.strip_prefix(TEMP_PREFIX))
            .and_then(|pid| pid.parse().ok());
        if pid.is_some_and(|pid| !crate::process::is_alive(pid)) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
mod tests;
