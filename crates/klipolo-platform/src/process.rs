//! Process plumbing for the out-of-process export worker: whether stdin is the parent's pipe, the one-worker-per-job
//! lock, the worker's command, and whether another process (named by its id in a temporary file) still runs.
//!
//! And for model servers: a child's memory, its start time (a pidfile keeps it, so a sweep never stops a process
//! that reused the id), stopping it, and background priority while it warms up. `Forward`: what of a worker's stderr
//! reaches the app's log.
//!
//! And for updates: the processes running from the app's bundle, with their mode arguments (`running_from`).
//! And for the render worker: its sandbox (`sandbox`).

mod forward;
mod recorder;
mod running;
pub mod sandbox;

pub use forward::Forward;
pub use recorder::{recorder_command, recorder_command_in_job, stop_child};
pub use running::{Running, mode_of, running_from};

use std::fs::File;
use std::io;
use std::path::Path;
use std::time::{Duration, SystemTime};

/// A temporary file or folder whose name carries no process id is left over once it hasn't changed for this long.
const UNOWNED_AFTER: Duration = Duration::from_secs(10 * 60);

/// True when stdin is a pipe (the app started this process); a closed pipe then means the parent is gone.
pub fn stdin_is_pipe() -> bool {
    crate::imp::stdin_is_pipe()
}

/// Whether a read of `fd` would return at once: bytes wait in it, or its writer closed it (or it isn't open). `wait`:
/// blocks until it would (`poll`), so a reader waits for bytes without reading them.
#[cfg(unix)]
pub fn readable(fd: std::os::fd::RawFd, wait: bool) -> bool {
    let mut probe = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        // SAFETY: one pollfd, as the count says; it only reads the descriptor's state.
        let n = unsafe { libc::poll(&raw mut probe, 1, if wait { -1 } else { 0 }) };
        if n >= 0 {
            return n > 0 && probe.revents != 0;
        }
        if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            // A descriptor poll can't read: a read would fail at once.
            return true;
        }
    }
}

/// Takes an exclusive lock on `file` without waiting (std's `File::try_lock`: `flock` on macOS and Linux,
/// `LockFileEx` on Windows). It is released when the file closes, on any exit. `WouldBlock`: another process holds
/// it.
pub fn lock_exclusive(file: &File) -> io::Result<()> {
    file.try_lock().map_err(|e| match e {
        std::fs::TryLockError::WouldBlock => io::ErrorKind::WouldBlock.into(),
        std::fs::TryLockError::Error(e) => e,
    })
}

/// True while the process `pid` exists (macOS and Linux: `kill(pid, 0)`, also when it belongs to another user;
/// Windows: `OpenProcess` and its exit code). A file named after a live process is still being written.
pub fn is_alive(pid: u32) -> bool {
    pid != 0 && crate::imp::is_alive(pid)
}

/// Whether a temporary file or folder named after process `pid` (None: its name has no id) is left over from work a
/// crash or a force quit cut off: its process is gone, or, with no id, it hasn't changed for 10 minutes. A live
/// process's files never are.
pub fn is_leftover(pid: Option<u32>, modified: Option<SystemTime>) -> bool {
    match pid {
        Some(pid) => !is_alive(pid),
        None => modified
            .and_then(|at| at.elapsed().ok())
            .is_some_and(|age| age >= UNOWNED_AFTER),
    }
}

/// Raises this process's soft open-files limit to `target`, never past its hard limit, and answers the soft limit it
/// has now (launchd gives an app 256). Windows: nothing (0).
pub fn raise_open_files(target: u64) -> io::Result<u64> {
    crate::imp::sandbox::raise_open_files(target)
}

/// A command that runs `exe` as a background worker (Windows: without a console window).
pub fn worker_command(exe: &Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(exe);
    crate::imp::worker_flags(&mut command);
    command
}

/// A process's memory: `footprint` is what Activity Monitor shows on macOS (`phys_footprint`, which leaves out clean
/// memory-mapped model weights); `resident` is its resident set.
/// - macOS: `proc_pid_rusage(RUSAGE_INFO_V2)`'s `ri_phys_footprint` and `ri_resident_size`;
/// - Linux: `/proc/<pid>/status`'s `VmRSS` for both (not yet: the footprint from `smaps_rollup`'s anonymous PSS);
/// - Windows: `GetProcessMemoryInfo`'s `PrivateUsage` (the private commit: Task Manager's "Commit size", not its
///   "Memory" column, which is the private working set) and `WorkingSetSize`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessMemory {
    pub footprint: u64,
    pub resident: u64,
}

/// `pid`'s memory, or None when it is gone, has exited (also before its parent reaps it: never its last figures), or
/// the OS won't say (another user's process).
pub fn memory(pid: u32) -> Option<ProcessMemory> {
    crate::imp::process_memory(pid)
}

/// This process's anonymous memory (bytes): its own heap and stacks, never the file pages it maps (sound caches,
/// prints, model weights), so a bench's figure is what it allocates:
/// - Linux: `/proc/self/status`'s `RssAnon`;
/// - macOS: `phys_footprint` (`memory`'s footprint), which already leaves clean mapped pages out;
/// - Windows: the private commit (`memory`'s footprint).
///
/// None when the OS won't say.
pub fn own_anonymous_memory() -> Option<u64> {
    crate::imp::own_anonymous_memory()
}

/// This boot's id, so a pidfile from before a reboot never matches a process of this one. None when the OS won't say.
/// - macOS: `kern.bootsessionuuid`;
/// - Linux: `/proc/sys/kernel/random/boot_id`;
/// - Windows: the boot time (`NtQuerySystemInformation(SystemTimeOfDayInformation)`, in 100 ns since 1601). Setting
///   the clock moves it, so a pidfile written before that no longer matches: the sweep misses that orphan (it frees
///   its weights after 15 idle minutes anyway) and never stops another process.
pub fn boot_id() -> Option<String> {
    crate::imp::boot_id()
}

/// When `pid` started, in µs since 1970: stable for the process's life, so a pidfile stores it (with `boot_id`) and a
/// later sweep never stops a different process that reused the id. None when gone or unknown (Windows, for now).
/// - macOS: `proc_pidinfo(PROC_PIDTBSDINFO)`'s start;
/// - Linux: `/proc/<pid>/stat` field 22 (clock ticks since boot) after `/proc/stat`'s `btime` (the boot, in s since
///   1970, read once per run), as psutil does: so 10 ms steps, and a clock set between two runs changes it, which
///   only makes a sweep miss;
/// - Windows (not yet: `GetProcessTimes`' creation time): None.
///
/// A process that has exited keeps its start time until its parent reaps it.
pub fn start_time(pid: u32) -> Option<u64> {
    crate::imp::start_time(pid)
}

/// Whether the process a pidfile recorded (its id, start time and boot) still runs: the same boot, the same start, and
/// not exited. A reused id, a pidfile from before a reboot, an exited process, or one the OS won't describe (Windows,
/// for now) never matches.
pub fn still_running(pid: u32, started: u64, boot: &str) -> bool {
    pid != 0 && boot_id().is_some_and(|now| now == boot) && crate::imp::still_running(pid, started)
}

/// Asks `pid` to exit, and forces it after 2 s, so it blocks up to 2 s (async callers use `spawn_blocking`). Both
/// signals go only to the process that started at `started` (`start_time`), checked right before each: one that has
/// exited (also one its parent hasn't reaped yet) or whose id now names another start is left alone, and that is
/// `Ok` (the process asked for is gone).
/// - `InvalidInput`: process 0, or an id beyond the OS's range (which `kill` would take for this process group,
///   another group or every process);
/// - `PermissionDenied`: the id names a process whose start can't be read (another user's), so nothing is sent.
///
/// The backends:
/// - macOS and Linux: `SIGTERM`, then `SIGKILL`;
/// - Windows (not yet: `OpenProcess`, `GetProcessTimes` and `TerminateProcess` on the same handle, so the check can't
///   race; sidecars live in a Job Object there): `Unsupported`.
pub fn terminate(pid: u32, started: u64) -> io::Result<()> {
    one_process(pid)?;
    crate::imp::terminate(pid, started)
}

/// Forces `pid` to exit at once, without waiting: for a model server that is still loading when the app stops it (quit,
/// critical memory pressure), so the load ends now rather than after it. Only the process that started at `started`,
/// checked right before the signal; one that has exited or whose id now names another start is left alone (`Ok`).
/// - macOS and Linux: `SIGKILL`;
/// - Windows (not yet: `TerminateProcess` on a handle whose `GetProcessTimes` matched): `Unsupported`.
pub fn kill(pid: u32, started: u64) -> io::Result<()> {
    one_process(pid)?;
    crate::imp::kill(pid, started)
}

/// The executable `pid` runs, as the OS resolved it when the process started, so a sweep can check that a pidfile
/// names a model server and not some other program. None when it is gone or the OS won't say (another user's).
/// - macOS: `proc_pidpath`;
/// - Linux: `/proc/<pid>/exe` (a deleted file reads with " (deleted)" added, so it matches nothing);
/// - Windows: `QueryFullProcessImageNameW`.
pub fn exe_path(pid: u32) -> Option<std::path::PathBuf> {
    if pid == 0 {
        return None;
    }
    crate::imp::exe_path(pid)
}

/// Background priority for a child while it does work nobody is waiting on (a model warming up): on macOS the
/// darwin background band (CPU and I/O throttled); off again before a person's request. A no-op on Linux and Windows
/// for now. Only for the process that started at `started`, checked first:
/// - `NotFound`: it has exited, or the id now names another process;
/// - `InvalidInput`: process 0 (which `setpriority` takes for this process) or an id beyond the OS's range;
/// - `PermissionDenied`: its start can't be read (another user's process).
///
/// The backends:
/// - macOS: `setpriority(PRIO_DARWIN_PROCESS, pid, PRIO_DARWIN_BG or 0)`;
/// - Linux (not yet; raising the priority back needs `CAP_SYS_NICE`, so `ioprio` and a cgroup's `cpu.weight` instead):
///   the check, then nothing;
/// - Windows (not yet: `SetPriorityClass(IDLE_PRIORITY_CLASS)` and EcoQoS through `SetProcessInformation`, on a handle
///   whose `GetProcessTimes` matched): nothing.
pub fn set_background(pid: u32, started: u64, on: bool) -> io::Result<()> {
    one_process(pid)?;
    crate::imp::set_background(pid, started, on)
}

/// Process 0 is never one process to signal or reprioritise: `kill` and `setpriority` take it for the caller.
fn one_process(pid: u32) -> io::Result<()> {
    if pid == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "process 0 names no one process",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
