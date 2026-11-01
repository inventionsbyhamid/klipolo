//! The macOS backend: Klipolo's Mac code, kept out of the engine and the app.

mod activated;
mod audio_route;
mod dock;
mod folder;
pub(crate) mod fonts;
mod install;
mod network;
mod power;
mod pressure;
mod process;
pub(crate) mod sandbox;
mod screen_color;
mod share;

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

pub use audio_route::{RouteWatch, hardware_latency};
pub(crate) use dock::install_dock_menu;
pub(crate) use folder::{
    Held, hold_folder, is_trash_name, locate_folder, move_to_trash, put_back, trash_roots,
    volume_name,
};
pub(crate) use install::{bundle, install_place, move_to_applications};
pub(crate) use network::watch as watch_net_path;
pub(crate) use power::watch_wake;
pub use power::{Activity, battery, begin as begin_activity, begin_capture, low_power, thermal};
pub(crate) use pressure::start as watch_pressure;
pub(crate) use process::{
    boot_id, exe_path, memory_bytes, own_anonymous_memory, process_memory, running_from,
    set_background, status,
};
pub(crate) use screen_color::{sample_screen_color, screen_color_supported};
pub(crate) use share::{share_files, share_supported};

use crate::durable::Durability;
use crate::qos::Qos;

#[cfg(any(test, feature = "fake"))]
pub(crate) const DISK_FULL: i32 = libc::ENOSPC;

/// `mach_absolute_time` converted to ns (CLOCK_UPTIME_RAW).
pub(crate) fn host_ns() -> u64 {
    unsafe extern "C" {
        fn clock_gettime_nsec_np(clock: libc::clockid_t) -> u64;
    }
    // SAFETY: reads a clock; no memory is touched.
    unsafe { clock_gettime_nsec_np(libc::CLOCK_UPTIME_RAW) }
}

fn qos_class(qos: Qos) -> libc::qos_class_t {
    use libc::qos_class_t::{
        QOS_CLASS_USER_INITIATED, QOS_CLASS_USER_INTERACTIVE, QOS_CLASS_UTILITY,
    };
    match qos {
        Qos::UserInteractive => QOS_CLASS_USER_INTERACTIVE,
        Qos::UserInitiated => QOS_CLASS_USER_INITIATED,
        Qos::Utility => QOS_CLASS_UTILITY,
    }
}

pub(crate) fn set_qos(qos: Qos) {
    // SAFETY: only changes the calling thread's own scheduling class.
    let _ = unsafe { libc::pthread_set_qos_class_self_np(qos_class(qos), 0) };
}

// <pthread/qos.h> (macOS 10.10+): a QoS override of another thread, for a thread waiting on it.
unsafe extern "C" {
    fn pthread_override_qos_class_start_np(
        thread: libc::pthread_t,
        class: libc::qos_class_t,
        relative_priority: libc::c_int,
    ) -> *mut std::ffi::c_void;
    fn pthread_override_qos_class_end_np(over: *mut std::ffi::c_void) -> libc::c_int;
}

/// A QoS override this process started (null: it couldn't start, so there is nothing to end).
pub(crate) struct Lent(*mut std::ffi::c_void);

pub(crate) fn lend<T>(thread: &std::thread::JoinHandle<T>, qos: Qos) -> Lent {
    use std::os::unix::thread::JoinHandleExt;
    // SAFETY: the borrowed handle keeps its thread's pthread valid (it can't be joined or detached meanwhile), even if
    // the thread has ended; the override holds its own reference to the kernel thread, so it may end after the thread.
    Lent(unsafe { pthread_override_qos_class_start_np(thread.as_pthread_t(), qos_class(qos), 0) })
}

impl Drop for Lent {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: an override `lend` started, ended once.
            let _ = unsafe { pthread_override_qos_class_end_np(self.0) };
        }
    }
}

/// `F_FULLFSYNC` (`Full`: the drive flushes its cache too) or `F_BARRIERFSYNC` (`Ordered`). A file system that doesn't
/// support one (a network share, some external formats: ENOTSUP, EINVAL or ENOTTY) gets the next weaker one, down to a
/// plain `fsync` (the data reaches the drive or the server, without the cache flush). std's `sync_all` and `sync_data`
/// are both `F_FULLFSYNC` on macOS, so neither is a fallback.
pub(crate) fn sync(file: &File, durability: Durability) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let fd = file.as_raw_fd();
    // SAFETY: a valid open descriptor; neither command takes an argument.
    let fcntl = |cmd| retrying(|| unsafe { libc::fcntl(fd, cmd) });
    // SAFETY: a valid open descriptor.
    sync_by(durability, fcntl, || {
        retrying(|| unsafe { libc::fsync(fd) })
    })
}

/// A plain `fsync`: the data reaches the drive, without the cache flush.
pub(crate) fn push(file: &File) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let fd = file.as_raw_fd();
    // SAFETY: a valid open descriptor.
    retrying(|| unsafe { libc::fsync(fd) })
}

/// `F_BARRIERFSYNC`, down to a plain `fsync` where the file system has none (`sync`'s ladder).
pub(crate) fn order(file: &File) -> io::Result<()> {
    sync(file, Durability::Ordered)
}

/// `sync`'s ladder over `fcntl` and `fsync` (tests fake them): each command the file system doesn't support falls to
/// the next; any other failure is the answer.
fn sync_by(
    durability: Durability,
    fcntl: impl Fn(libc::c_int) -> io::Result<()>,
    fsync: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    let commands: &[libc::c_int] = match durability {
        Durability::Full => &[libc::F_FULLFSYNC, libc::F_BARRIERFSYNC],
        Durability::Ordered => &[libc::F_BARRIERFSYNC],
    };
    for &command in commands {
        match fcntl(command) {
            Err(e) if unsupported(&e) => {}
            done => return done,
        }
    }
    fsync()
}

/// The file system doesn't do that kind of sync.
fn unsupported(e: &io::Error) -> bool {
    matches!(
        e.raw_os_error(),
        Some(libc::ENOTSUP | libc::EOPNOTSUPP | libc::EINVAL | libc::ENOTTY)
    )
}

/// A call answering -1 with errno, again after an interruption.
fn retrying(call: impl Fn() -> libc::c_int) -> io::Result<()> {
    loop {
        if call() != -1 {
            return Ok(());
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

/// Nothing to do: the barrier orders the rename after the file's data.
pub(crate) fn fsync_dir(_dir: &Path) -> io::Result<()> {
    Ok(())
}

pub(crate) use crate::unix::{
    classify_errno, classify_errno as classify_os, file_identity, is_alive, kill, placed_identity,
    read_exact_at, start_time, stdin_is_pipe, still_running, terminate, volume_id, will_need,
    write_all_at,
};

/// `statfs`: blocks available to this user × block size.
pub(crate) fn free_space(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: a NUL-terminated path and a zeroed out-struct.
    unsafe {
        let mut st: libc::statfs = std::mem::zeroed();
        (libc::statfs(path.as_ptr(), &mut st) == 0).then(|| st.f_bavail * u64::from(st.f_bsize))
    }
}

pub(crate) fn worker_flags(_command: &mut tokio::process::Command) {}

/// `~/Movies`.
pub(crate) fn videos_dir() -> Option<PathBuf> {
    dirs::video_dir()
}

/// `~/Library/Logs/Klipolo`.
pub(crate) fn logs_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join("Library").join("Logs").join("Klipolo"))
}

/// "aarch64" on Apple silicon, also when this Intel build runs under Rosetta (`hw.optional.arm64` exists only on
/// Apple silicon); else "x86_64".
pub(crate) fn cpu_arch() -> String {
    let mut value: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>();
    // SAFETY: a NUL-terminated name, and an int-sized output buffer whose size is passed with it.
    let found = unsafe {
        libc::sysctlbyname(
            c"hw.optional.arm64".as_ptr(),
            (&raw mut value).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } == 0;
    if found && value == 1 {
        "aarch64".into()
    } else {
        std::env::consts::ARCH.into()
    }
}

pub(crate) fn os_version() -> String {
    let v = objc2_foundation::NSProcessInfo::processInfo().operatingSystemVersion();
    format!("{}.{}.{}", v.majorVersion, v.minorVersion, v.patchVersion)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Runs `sync_by` with `fcntl` answering `answer(command)`; returns the result and the calls made.
    fn ladder(
        durability: Durability,
        answer: impl Fn(libc::c_int) -> Option<i32>,
    ) -> (io::Result<()>, Vec<&'static str>) {
        let calls = RefCell::new(Vec::new());
        let name = |command| match command {
            libc::F_FULLFSYNC => "full",
            libc::F_BARRIERFSYNC => "barrier",
            _ => "other",
        };
        let result = sync_by(
            durability,
            |command| {
                calls.borrow_mut().push(name(command));
                answer(command).map_or(Ok(()), |code| Err(io::Error::from_raw_os_error(code)))
            },
            || {
                calls.borrow_mut().push("fsync");
                Ok(())
            },
        );
        (result, calls.into_inner())
    }

    #[test]
    fn a_sync_the_file_system_doesnt_support_falls_to_the_next_one() {
        // A network share without F_FULLFSYNC: the barrier.
        let (done, calls) = ladder(Durability::Full, |c| {
            (c == libc::F_FULLFSYNC).then_some(libc::ENOTSUP)
        });
        assert!(done.is_ok());
        assert_eq!(calls, ["full", "barrier"]);
        // Neither command: a plain fsync, for both durabilities.
        let (done, calls) = ladder(Durability::Full, |c| {
            Some(if c == libc::F_FULLFSYNC {
                libc::ENOTTY
            } else {
                libc::EINVAL
            })
        });
        assert!(done.is_ok());
        assert_eq!(calls, ["full", "barrier", "fsync"]);
        let (done, calls) = ladder(Durability::Ordered, |_| Some(libc::EINVAL));
        assert!(done.is_ok());
        assert_eq!(calls, ["barrier", "fsync"]);
        // A real failure (a disk error) is the answer: nothing weaker is tried in its place.
        let (done, calls) = ladder(Durability::Full, |_| Some(libc::EIO));
        assert_eq!(done.unwrap_err().raw_os_error(), Some(libc::EIO));
        assert_eq!(calls, ["full"]);
        // A supporting file system: one call.
        assert_eq!(ladder(Durability::Full, |_| None).1, ["full"]);
        // And for real, on this volume.
        let dir = tempfile::tempdir().unwrap();
        let file = File::create(dir.path().join("f")).unwrap();
        sync(&file, Durability::Full).unwrap();
        sync(&file, Durability::Ordered).unwrap();
    }
}
