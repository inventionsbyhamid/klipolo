//! Where Klipolo is installed, whether an update can replace it there, and Move to Applications.
//! Each backend reads the facts from its OS (`PlaceFacts`); `classify` turns them into an `InstallPlace` the same way
//! everywhere:
//! - macOS: `statfs` (a read-only volume; APFS, whose `RENAME_SWAP` swaps a bundle atomically) and `access(W_OK)` on
//!   the bundle and its folder; a translocated copy by its path;
//! - Windows (later): the per-user install folder's writability, NTFS rename semantics. Until then `place` answers
//!   `Unknown`: the `process::running_from` stub can't see processes, so an install would otherwise take "nothing
//!   runs" for an answer;
//! - Linux: `$APPIMAGE`'s writability; a Flatpak (`$FLATPAK_ID`) is managed (later: deb and rpm too).

use std::cmp::Ordering;
use std::io;
use std::path::{Component, Path, PathBuf};

// Move to Applications' steps: only macOS has the move (on Windows and Linux the installer or package places the app).
#[cfg(any(target_os = "macos", test))]
pub(crate) mod moving;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum InstallPlace {
    /// Updates install at quit.
    Writable,
    /// Only "Restart now" installs, behind the OS's password prompt.
    NeedsAdmin,
    /// Writable, but the volume can't swap atomically (not APFS): a kill between two renames would leave no app,
    /// so only "Restart now" installs, with the person present.
    NonAtomic,
    /// macOS runs a quarantined copy from a random read-only folder: no updates until it's moved.
    Translocated,
    /// Running from the mounted disk image.
    DiskImage,
    /// The bundle is locked in Finder (Get Info → Locked: `UF_IMMUTABLE`, or `SF_IMMUTABLE`): nothing can replace it,
    /// not even with a password, until the person unlocks it.
    Locked,
    /// On a network volume this user can't write: the password prompt's administrator can't write a network share
    /// either, so no install is tried there. A writable network copy is `NonAtomic`.
    Network,
    /// A package manager or store owns the copy (Flatpak, deb, rpm): the self-updater is off.
    Managed,
    /// Not a bundle (`cargo test`, a dev run).
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlaceFacts {
    pub parent_writable: bool,
    pub bundle_writable: bool,
    pub read_only_volume: bool,
    pub managed: bool,
    /// The volume swaps two names atomically (APFS `RENAME_SWAP`).
    pub atomic_swap: bool,
    /// The bundle is locked (macOS `UF_IMMUTABLE` or `SF_IMMUTABLE`).
    pub locked: bool,
    /// The volume isn't on this computer (macOS: no `MNT_LOCAL`).
    pub network: bool,
}

/// The rule every backend applies to what it read (pure), in this order: a managed copy; a translocated one (a path
/// component `AppTranslocation`); a read-only volume (the disk image); a locked bundle; writable with an atomic swap;
/// writable without one; a network volume this user can't write; else it needs an administrator.
pub fn classify(bundle: &Path, facts: PlaceFacts) -> InstallPlace {
    let translocated = bundle
        .components()
        .any(|part| part == Component::Normal("AppTranslocation".as_ref()));
    let writable = facts.parent_writable && facts.bundle_writable;
    if facts.managed {
        InstallPlace::Managed
    } else if translocated {
        InstallPlace::Translocated
    } else if facts.read_only_volume {
        InstallPlace::DiskImage
    } else if facts.locked {
        InstallPlace::Locked
    } else if writable && facts.atomic_swap {
        InstallPlace::Writable
    } else if writable {
        InstallPlace::NonAtomic
    } else if facts.network {
        InstallPlace::Network
    } else {
        InstallPlace::NeedsAdmin
    }
}

/// The running app's bundle: macOS the `.app` above `current_exe`; W the install folder; L `$APPIMAGE` (inside a
/// Flatpak, the running executable, whose place is `Managed`). None in tests and dev runs.
pub fn bundle() -> Option<PathBuf> {
    crate::imp::bundle()
}

/// Where `bundle` sits, read from the OS, then `classify`. `Unknown` when the OS can't describe its volume (it is
/// gone), and on Windows for now (tests can set it: `fake::set_install_place`).
pub fn place(bundle: &Path) -> InstallPlace {
    #[cfg(any(test, feature = "fake"))]
    if let Some(place) = crate::fake::install_place_override() {
        return place;
    }
    crate::imp::install_place(bundle)
}

/// Why Move to Applications didn't move (`move_to_applications`).
#[derive(Debug)]
pub enum MoveError {
    /// A newer Klipolo (this version) is already there: it stays, and nothing was copied.
    NewerInstalled(String),
    /// Another app named Klipolo.app is there: it stays, and nothing was copied.
    NameTaken,
    /// This account may not write /Applications, where a Klipolo already is (an administrator's copy): nothing is
    /// copied to ~/Applications beside it, so there are never two. The person opens that one.
    InApplications,
    /// The copy couldn't take its place, and the older Klipolo it replaced couldn't be put back: that one is in the
    /// Trash, where Put Back works (logged).
    LeftInTrash(io::Error),
    /// Copying, the Trash or the rename failed; nothing else changed (an older Klipolo is where it was). Windows and
    /// Linux: `Unsupported`.
    Io(io::Error),
}

impl From<io::Error> for MoveError {
    fn from(e: io::Error) -> Self {
        MoveError::Io(e)
    }
}

/// macOS: copies `bundle` into /Applications (else ~/Applications when that isn't writable and no Klipolo is in
/// /Applications already: `InApplications`) as Klipolo.app, whatever the bundle's own name, with `ditto`, clears
/// `com.apple.quarantine` on the copy, and returns the copy's path. `than` compares a Klipolo already there (the same
/// bundle id; its `CFBundleShortVersionString`) with this one: an older one goes to the Trash once the copy is
/// complete, this very version is simply opened (nothing is copied), and a newer one stays (an older disk image never
/// replaces a newer install). Anything else with that name stays. A disk without room for the copy says so before
/// anything is written. Blocking: `ditto` reads the whole app, so call it off the main thread. Windows and Linux:
/// `Unsupported`.
///
/// What a failure halfway leaves (`install::moving`): the copy is made under a hidden temporary name beside the target
/// and renamed into place last, so a failed copy changes nothing (it is deleted, or swept by the next attempt after a
/// kill); what is at the target is read again before it goes; a failed rename puts the older Klipolo back from the
/// Trash, and one that can't be put back stays there (`LeftInTrash`). The running copy is only read.
pub fn move_to_applications(
    bundle: &Path,
    than: &dyn Fn(&str) -> Ordering,
) -> Result<PathBuf, MoveError> {
    crate::imp::move_to_applications(bundle, than)
}
