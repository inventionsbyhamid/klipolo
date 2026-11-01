//! The seams updates and uploads ask about, on Windows, for now: where Klipolo runs from, the network path, battery
//! saver and the wake. Each stub degrades honestly and names its later counterpart.

use std::io;
use std::path::{Path, PathBuf};

use crate::install::{InstallPlace, MoveError};

/// Later: the per-user install folder (`%LOCALAPPDATA%\Programs\Klipolo`, the folder above `current_exe` when it holds
/// the NSIS uninstaller). None until then.
pub(crate) fn bundle() -> Option<PathBuf> {
    None
}

/// Later: the install folder's writability, and NTFS's rename semantics. `Unknown` until then: the
/// `process::running_from` stub can't see processes, so any other answer would let an install take "nothing runs"
/// for the truth.
pub(crate) fn install_place(_bundle: &Path) -> InstallPlace {
    InstallPlace::Unknown
}

/// None on Windows: the installer places the app.
pub(crate) fn move_to_applications(
    _bundle: &Path,
    _than: &dyn Fn(&str) -> std::cmp::Ordering,
) -> Result<PathBuf, MoveError> {
    Err(MoveError::Io(io::ErrorKind::Unsupported.into()))
}

/// Later: `GetNetworkConnectivityHint` (its `ConnectivityCost`) and `NotifyNetworkConnectivityHintChange`. Nothing to
/// watch yet, so `network::path` stays online, neither constrained nor expensive.
pub(crate) fn watch_net_path() {}

/// Later: battery saver (`GetSystemPowerStatus`'s `SystemStatusFlag`). False until then.
pub(crate) fn low_power() -> bool {
    false
}

/// Later: `RegisterSuspendResumeNotification` (`PBT_APMRESUMEAUTOMATIC`). Nothing to watch yet: `power::on_wake`
/// listeners never run.
pub(crate) fn watch_wake() {}
