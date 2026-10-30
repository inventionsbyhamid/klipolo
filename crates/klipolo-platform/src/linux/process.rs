//! Processes on Linux, from `/proc`: another process's start and state, its memory and executable, this boot's id, and
//! background priority (L), and the processes running from a folder.

use std::io;
use std::path::{Path, PathBuf};

/// `/proc/<pid>/stat`: the start in µs since 1970 (the boot plus the ticks since it), and whether the process has
/// exited unreaped. None when it is gone.
pub(crate) fn status(pid: u32) -> Option<crate::unix::Status> {
    crate::unix::one_pid(pid).ok()?;
    let (ticks, zombie) = parse_stat(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)?;
    let (boot_us, hz) = boot_clock()?;
    Some(crate::unix::Status {
        started: boot_us + ticks * 1_000_000 / hz,
        zombie,
    })
}

/// When this boot began, in µs since 1970 (`/proc/stat`'s `btime`, whole seconds), and the clock ticks per second
/// (`sysconf(_SC_CLK_TCK)`). Read once per run, so every start time a run computes counts from the same boot even if
/// the clock is set meanwhile (the kernel moves `btime` with it).
fn boot_clock() -> Option<(u64, u64)> {
    static CLOCK: std::sync::OnceLock<Option<(u64, u64)>> = std::sync::OnceLock::new();
    *CLOCK.get_or_init(|| {
        let stat = std::fs::read_to_string("/proc/stat").ok()?;
        let btime: u64 = stat
            .lines()
            .find_map(|line| line.strip_prefix("btime "))?
            .trim()
            .parse()
            .ok()?;
        // SAFETY: reads a system constant.
        let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        (hz > 0).then(|| (btime * 1_000_000, hz as u64))
    })
}

/// Field 22 (the start, in clock ticks since boot) and whether field 3 (the state) is `Z` (a zombie) or `X` (dead).
/// Fields are counted after the command name's last closing parenthesis, since the name itself may hold spaces and
/// parentheses.
pub(super) fn parse_stat(stat: &str) -> Option<(u64, bool)> {
    let mut fields = stat[stat.rfind(')')? + 1..].split_ascii_whitespace();
    let state = fields.next()?;
    let ticks = fields.nth(22 - 4)?.parse().ok()?;
    Some((ticks, matches!(state, "Z" | "X" | "x")))
}

/// A `Name:   N kB` line's N.
pub(super) fn kib(text: &str, name: &str) -> Option<u64> {
    let value = text
        .lines()
        .find_map(|line| line.strip_prefix(name)?.strip_prefix(':'))?;
    value.trim().strip_suffix("kB")?.trim().parse().ok()
}

/// `/proc/<pid>/status`'s `VmRSS` for both figures (L: the footprint from `smaps_rollup`'s anonymous PSS). None for a
/// zombie, which has no `VmRSS` line.
pub(crate) fn process_memory(pid: u32) -> Option<crate::process::ProcessMemory> {
    crate::unix::one_pid(pid).ok()?;
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let resident = kib(&status, "VmRSS")? * 1024;
    Some(crate::process::ProcessMemory {
        footprint: resident,
        resident,
    })
}

/// `/proc/self/status`'s `RssAnon`: resident anonymous pages, without the file pages the process maps.
pub(crate) fn own_anonymous_memory() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    Some(kib(&status, "RssAnon")? * 1024)
}

/// The check (`verify`), then nothing. L: raising the priority back needs `CAP_SYS_NICE`, so
/// `ioprio_set(IOPRIO_CLASS_IDLE)` and a cgroup's `cpu.weight` instead.
pub(crate) fn set_background(pid: u32, started: u64, _on: bool) -> io::Result<()> {
    crate::unix::verify(pid, started).map(|_| ())
}

/// `/proc/<pid>/exe`: the executable's path. None when the process is gone, isn't this user's, or its file was
/// deleted (the kernel then adds " (deleted)", which never matches a path we expect).
pub(crate) fn exe_path(pid: u32) -> Option<PathBuf> {
    crate::unix::one_pid(pid).ok()?;
    std::fs::read_link(format!("/proc/{pid}/exe")).ok()
}

/// `/proc/sys/kernel/random/boot_id`: a UUID made at each boot.
pub(crate) fn boot_id() -> Option<String> {
    let id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
    let id = id.trim();
    (!id.is_empty()).then(|| id.to_owned())
}

/// Every `/proc/<pid>/exe` under `bundle` but our own (an unreadable link is skipped: another user's process, or one
/// that has exited), with its mode from `/proc/<pid>/cmdline` (None when that can't be read). `Err` when `/proc` can't
/// be listed.
pub(crate) fn running_from(bundle: &Path) -> io::Result<Vec<crate::process::Running>> {
    let own = std::process::id();
    Ok(std::fs::read_dir("/proc")?
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
        .filter(|&pid| pid != own)
        .filter(|&pid| crate::process::exe_path(pid).is_some_and(|exe| exe.starts_with(bundle)))
        .map(|pid| crate::process::Running {
            pid,
            mode: mode(pid),
        })
        .collect())
}

/// The mode in `/proc/<pid>/cmdline` (the arguments, each ending in a NUL).
fn mode(pid: u32) -> Option<String> {
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let args: Vec<_> = cmdline
        .split(|&b| b == 0)
        .take(2)
        .map(String::from_utf8_lossy)
        .collect();
    crate::process::mode_of(&args)
}
