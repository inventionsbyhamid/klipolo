//! The Linux backend, for now: the clock, durable files, process plumbing and the install place are real; QoS, power,
//! memory pressure, the audio route and the network path are stubs that change nothing (each names its later
//! counterpart).

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::durable::Durability;
use crate::qos::Qos;

mod battery;
pub(crate) mod fonts;
mod install;
mod process;
pub(crate) mod sandbox;
mod trash;
pub(crate) use battery::battery;
pub(crate) use install::{bundle, install_place, move_to_applications};

#[cfg(any(test, feature = "fake"))]
pub(crate) const DISK_FULL: i32 = libc::ENOSPC;

/// `CLOCK_MONOTONIC` in ns.
pub(crate) fn host_ns() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: writes the time into a local.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

/// Later: `nice` and `ioprio_set` for utility work, `SCHED_IDLE` for background work, rtkit for audio.
pub(crate) fn set_qos(_qos: Qos) {}

/// Later: an ioprio and nice boost of the waited-on thread while the caller waits.
pub(crate) struct Lent;

pub(crate) fn lend<T>(_thread: &std::thread::JoinHandle<T>, _qos: Qos) -> Lent {
    Lent
}

/// Later: a logind (or portal) `Inhibit("idle:sleep")`.
pub struct Activity;

pub(crate) fn begin_activity(_reason: &str) -> Activity {
    Activity
}

/// Later: a logind `sleep:idle` inhibitor for the take (the Inhibit portal inside Flatpak). Until then,
/// `begin_activity`'s.
pub(crate) fn begin_capture(reason: &str) -> Activity {
    begin_activity(reason)
}

/// Later: power-profiles-daemon's `PerformanceDegraded`, else the thermal zones' trip points (`power::Thermal`).
pub(crate) fn thermal() -> crate::power::Thermal {
    crate::power::Thermal::Nominal
}

/// Later: PSI triggers on `/proc/pressure/memory`. The level stays Normal.
pub(crate) fn watch_pressure() {}

/// Later: NetworkManager's `Metered` property and its `PropertiesChanged` signal (D-Bus). Nothing to watch yet, so
/// `network::path` stays online, neither constrained nor expensive.
pub(crate) fn watch_net_path() {}

/// Later: power-profiles-daemon's `ActiveProfile` is "power-saver". False until then.
pub(crate) fn low_power() -> bool {
    false
}

/// Later: logind's `PrepareForSleep(false)` signal. Nothing to watch yet: `power::on_wake` listeners never run.
pub(crate) fn watch_wake() {}

/// Later: the PipeWire registry (or PulseAudio `subscribe`). Reports no changes.
#[derive(Default)]
pub struct RouteWatch;

impl RouteWatch {
    pub fn new() -> Self {
        Self
    }

    pub fn follow(&mut self) {}
}

/// Later: `pw_stream_get_time_n` or `snd_pcm_delay`.
pub(crate) fn hardware_latency() -> Duration {
    Duration::ZERO
}

/// No eyedropper on Linux: the UI shows none.
pub(crate) fn screen_color_supported() -> bool {
    false
}

pub(crate) fn sample_screen_color(done: Box<dyn FnOnce(Option<String>) + Send>) {
    done(None)
}

/// `fdatasync` or `fsync`.
pub(crate) fn sync(file: &File, durability: Durability) -> io::Result<()> {
    match durability {
        Durability::Ordered => file.sync_data(),
        Durability::Full => file.sync_all(),
    }
}

/// `fdatasync`, which flushes the drive's cache too.
pub(crate) fn push(file: &File) -> io::Result<()> {
    file.sync_data()
}

/// Nothing: `push` already reached the disk.
pub(crate) fn order(_file: &File) -> io::Result<()> {
    Ok(())
}

/// `fsync` of the folder, so a rename into it survives a power cut. File systems that can't sync a folder (EINVAL)
/// are left as they are.
pub(crate) fn fsync_dir(dir: &Path) -> io::Result<()> {
    match File::open(dir)?.sync_all() {
        Err(e) if e.raw_os_error() == Some(libc::EINVAL) => Ok(()),
        other => other,
    }
}

/// `$XDG_DATA_HOME/Trash` (the home Trash, XDG Trash spec).
pub(crate) fn trash_roots() -> Vec<PathBuf> {
    dirs::data_dir()
        .map(|data| data.join("Trash"))
        .into_iter()
        .collect()
}

/// A volume's Trash: `.Trash` (with a folder per user) or `.Trash-<uid>`.
pub(crate) fn is_trash_name(name: &std::ffi::OsStr) -> bool {
    name == ".Trash"
        || name.to_str().is_some_and(|name| {
            name.strip_prefix(".Trash-")
                .is_some_and(|uid| !uid.is_empty() && uid.bytes().all(|b| b.is_ascii_digit()))
        })
}

/// Later: an `O_PATH` descriptor, read back through `/proc/self/fd` (`folder::HeldFolder`). None yet: folders are
/// followed by their path.
pub(crate) enum Held {}

pub(crate) fn hold_folder(_path: &Path) -> io::Result<Option<Held>> {
    Ok(None)
}

pub(crate) fn locate_folder(held: &Held) -> Option<PathBuf> {
    match *held {}
}

/// The volume `path` is on, by `statvfs`'s `f_fsid` (`fsid:<hex>`), which ext4, XFS and btrfs derive from the file
/// system's UUID, so it survives a remount. None when it fails or is zero.
pub(crate) fn volume_name(path: &Path) -> Option<String> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: a plain C struct, all zeroes.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: a NUL-terminated path, and the struct `statvfs` fills.
    let read = unsafe { libc::statvfs(path.as_ptr(), &mut st) } == 0;
    (read && st.f_fsid != 0).then(|| format!("fsid:{:x}", st.f_fsid))
}

use process::kib;
#[cfg(test)]
use process::parse_stat;
pub(crate) use process::{
    boot_id, exe_path, own_anonymous_memory, process_memory, running_from, set_background, status,
};
pub(crate) use trash::{move_to_trash, put_back};

pub(crate) use crate::unix::{
    classify_errno, classify_errno as classify_os, file_identity, is_alive, kill, placed_identity,
    read_exact_at, start_time, stdin_is_pipe, still_running, terminate, volume_id, will_need,
    write_all_at,
};

/// `/proc/meminfo`'s `MemTotal` (0 when it can't be read).
pub(crate) fn memory_bytes() -> u64 {
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|info| kib(&info, "MemTotal"))
        .map_or(0, |kib| kib * 1024)
}

/// `statvfs`: blocks available to this user × fragment size.
pub(crate) fn free_space(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: a NUL-terminated path and a zeroed out-struct.
    unsafe {
        let mut st: libc::statvfs = std::mem::zeroed();
        (libc::statvfs(path.as_ptr(), &mut st) == 0).then(|| st.f_bavail * st.f_frsize)
    }
}

pub(crate) fn worker_flags(_command: &mut tokio::process::Command) {}

/// `XDG_VIDEOS_DIR` (`user-dirs.dirs`), else `~/Videos`.
pub(crate) fn videos_dir() -> Option<PathBuf> {
    dirs::video_dir().or_else(|| dirs::home_dir().map(|home| home.join("Videos")))
}

/// `$XDG_STATE_HOME/klipolo/logs`, else `~/.local/state/klipolo/logs` (the XDG default).
pub(crate) fn logs_dir() -> Option<PathBuf> {
    dirs::state_dir().map(|state| state.join("klipolo").join("logs"))
}

/// The build's architecture: Linux desktops run Klipolo natively.
pub(crate) fn cpu_arch() -> String {
    std::env::consts::ARCH.into()
}

/// The kernel release (`uname -r`).
pub(crate) fn os_version() -> String {
    // SAFETY: a zeroed out-struct; `release` is NUL-terminated on success.
    unsafe {
        let mut name: libc::utsname = std::mem::zeroed();
        if libc::uname(&mut name) != 0 {
            return String::new();
        }
        std::ffi::CStr::from_ptr(name.release.as_ptr())
            .to_string_lossy()
            .into_owned()
    }
}

/// The Dock menu's counterpart isn't built yet (`dock`'s module doc), so there is nothing to install.
pub(crate) fn install_dock_menu(_items: crate::dock::DockItems, _click: crate::dock::DockClick) {
    tracing::debug!("dock: no app icon menu on this OS yet");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_lines_are_read_by_field() {
        // A command name with a space and parentheses, then fields 3 (the state) to 22 (the start time) and on.
        let stat = |state: &str| {
            let middle: Vec<String> = (4..22).map(|field| field.to_string()).collect();
            format!(
                "4242 (llama (x) 1) {state} {} 987654 0 0\n",
                middle.join(" ")
            )
        };
        assert_eq!(parse_stat(&stat("S")), Some((987_654, false)));
        assert_eq!(parse_stat(&stat("Z")), Some((987_654, true)));
        assert_eq!(parse_stat("4242 (cut"), None);
        let status = "VmRSSx:\t1 kB\nVmPeak:\t   20 kB\nVmRSS:\t    2048 kB\n";
        assert_eq!(kib(status, "VmRSS"), Some(2048));
        assert_eq!(kib(status, "VmSwap"), None);
    }
}
