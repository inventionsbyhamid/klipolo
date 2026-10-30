//! What the macOS and Linux backends share.

use std::fs::File;
use std::io;

use crate::durable::DiskReason;

/// errno → reason: a quota or a file-size limit stops a write like a full disk does.
pub(crate) fn classify_errno(errno: i32) -> DiskReason {
    match errno {
        libc::ENOSPC | libc::EDQUOT | libc::EFBIG => DiskReason::Full,
        libc::EROFS => DiskReason::ReadOnly,
        libc::EACCES | libc::EPERM => DiskReason::Denied,
        _ => DiskReason::Other,
    }
}

pub(crate) fn write_all_at(file: &File, buf: &[u8], offset: u64) -> io::Result<()> {
    std::os::unix::fs::FileExt::write_all_at(file, buf, offset)
}

pub(crate) fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<()> {
    std::os::unix::fs::FileExt::read_exact_at(file, buf, offset)
}

/// `madvise(MADV_WILLNEED)` over the pages `bytes` spans (from the start of its first page, as madvise wants).
pub(crate) fn will_need(bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    // SAFETY: reads a system constant.
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(1) as usize;
    let start = bytes.as_ptr() as usize;
    let lead = start % page;
    // SAFETY: a hint over the pages `bytes` lies in, which are mapped while it is borrowed; it changes no data. An
    // error (memory that isn't a file's) only means no read-ahead.
    let _ = unsafe {
        libc::madvise(
            (start - lead) as *mut libc::c_void,
            bytes.len() + lead,
            libc::MADV_WILLNEED,
        )
    };
}

/// The device `path` is on (`st_dev`).
pub(crate) fn volume_id(path: &std::path::Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|meta| meta.dev())
}

/// Renames `from` to `to`, never over something already there (AlreadyExists): a folder put back from the Trash
/// (`folder::put_back`). The check and the rename are two steps; the only caller puts back a take whose name is its
/// unique id, so nothing else makes that name between them.
pub(crate) fn rename_new(from: &std::path::Path, to: &std::path::Path) -> io::Result<()> {
    if std::fs::symlink_metadata(to).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "something is already there",
        ));
    }
    std::fs::rename(from, to)
}

pub(crate) fn stdin_is_pipe() -> bool {
    // SAFETY: fstat on descriptor 0 with a zeroed out-struct.
    unsafe {
        let mut st: libc::stat = std::mem::zeroed();
        libc::fstat(0, &mut st) == 0 && (st.st_mode & libc::S_IFMT) == libc::S_IFIFO
    }
}

pub(crate) fn is_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // SAFETY: signal 0 sends nothing; it only checks that the process exists.
    let found = unsafe { libc::kill(pid, 0) } == 0;
    // EPERM: it exists but belongs to another user.
    found || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// `pid` as one process's `pid_t`: 0 and ids beyond `pid_t` (negative to `kill`: a process group, or every process)
/// are never passed to a signal or a priority call.
pub(crate) fn one_pid(pid: u32) -> io::Result<libc::pid_t> {
    match libc::pid_t::try_from(pid) {
        Ok(id) if id > 0 => Ok(id),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not one process's id",
        )),
    }
}

/// What the backend's `status` says about a process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Status {
    /// When it started, in µs since 1970.
    pub started: u64,
    /// It has exited, but its parent hasn't reaped it yet: it keeps its id, and no signal reaches it.
    pub zombie: bool,
}

pub(crate) fn start_time(pid: u32) -> Option<u64> {
    crate::imp::status(pid).map(|s| s.started)
}

/// `pid` still names the process that started at `started`, and it hasn't exited.
pub(crate) fn still_running(pid: u32, started: u64) -> bool {
    one_pid(pid).is_ok()
        && crate::imp::status(pid).is_some_and(|s| s.started == started && !s.zombie)
}

/// `pid` as a `pid_t` while it still names the running process that started at `started`. `NotFound` when it has
/// exited or the id names another start; `PermissionDenied` when it exists but its status can't be read.
pub(crate) fn verify(pid: u32, started: u64) -> io::Result<libc::pid_t> {
    let id = one_pid(pid)?;
    match crate::imp::status(pid) {
        Some(s) if s.started == started && !s.zombie => Ok(id),
        None if is_alive(pid) => Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the process's start can't be read, so it can't be told apart from another",
        )),
        _ => Err(io::Error::new(
            io::ErrorKind::NotFound,
            "the process has exited, or its id now names another",
        )),
    }
}

/// How long `terminate` waits after `SIGTERM`, and how often it looks.
const GRACE: std::time::Duration = std::time::Duration::from_secs(2);
const LOOK_EVERY: std::time::Duration = std::time::Duration::from_millis(100);

/// `SIGTERM`, then `SIGKILL` if the same process still runs after `GRACE`; each only after `verify`.
pub(crate) fn terminate(pid: u32, started: u64) -> io::Result<()> {
    let id = match verify(pid, started) {
        Ok(id) => id,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    signal(id, libc::SIGTERM)?;
    let mut waited = std::time::Duration::ZERO;
    while waited < GRACE {
        if !still_running(pid, started) {
            return Ok(());
        }
        std::thread::sleep(LOOK_EVERY);
        waited += LOOK_EVERY;
    }
    if still_running(pid, started) {
        signal(id, libc::SIGKILL)?;
    }
    Ok(())
}

/// `SIGKILL` at once, only after `verify`: nothing waits.
pub(crate) fn kill(pid: u32, started: u64) -> io::Result<()> {
    match verify(pid, started) {
        Ok(id) => signal(id, libc::SIGKILL),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// `path`'s volume, by the UUID the OS gives for it (`volume_name`), else `dev:<st_dev>`, and its inode.
pub(crate) fn placed_identity(path: &std::path::Path) -> Option<crate::file::PlacedIdentity> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).ok()?;
    let volume = crate::imp::volume_name(path).unwrap_or_else(|| format!("dev:{}", meta.dev()));
    Some(crate::file::PlacedIdentity {
        volume,
        file: meta.ino(),
    })
}

/// The volume (`st_dev`) and the file's number there (`st_ino`), from the open file (`fstat`).
pub(crate) fn file_identity(file: &File) -> Option<crate::file::FileIdentity> {
    use std::os::unix::fs::MetadataExt;
    let meta = file.metadata().ok()?;
    Some(crate::file::FileIdentity {
        volume: meta.dev(),
        file: meta.ino(),
    })
}

/// `kill(id, signal)`; a process that is already gone (ESRCH) counts as done.
fn signal(id: libc::pid_t, signal: libc::c_int) -> io::Result<()> {
    // SAFETY: a signal to one process (`one_pid` made `id` positive).
    if unsafe { libc::kill(id, signal) } == 0 {
        return Ok(());
    }
    let e = io::Error::last_os_error();
    if e.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(e)
    }
}

/// `process::recorder_command`'s rule on macOS and Linux: a session and process group of its own.
pub(crate) fn recorder_flags(command: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: `setsid` is async-signal-safe and only changes the child's own session; nothing is allocated.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
}

/// No jobs on macOS or Linux: the same session of its own.
pub(crate) fn recorder_flags_in_job(command: &mut std::process::Command) {
    recorder_flags(command);
}

/// `SIGTERM` to a child the caller hasn't reaped (`process::stop_child`); one that has exited is `Ok`.
pub(crate) fn ask_to_stop(child: &std::process::Child) -> io::Result<()> {
    let pid = libc::pid_t::try_from(child.id())
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    // SAFETY: a plain signal to our own unreaped child, whose id can't have been reused.
    if unsafe { libc::kill(pid, libc::SIGTERM) } == -1 {
        let e = io::Error::last_os_error();
        if e.raw_os_error() != Some(libc::ESRCH) {
            return Err(e);
        }
    }
    Ok(())
}

/// Closes every descriptor above 2 (`process::sandbox::close_inherited`): each one `listing` (`/dev/fd`, or Linux's
/// `/proc/self/fd`) names, or, when it can't be read, every number up to the descriptor table's size (at most 65 536).
/// Only while one thread runs: the listing's own descriptor is closed before the others, and a number another thread
/// reused meanwhile would be closed under it.
pub(crate) fn close_above_stderr(listing: &str) {
    let listed: Option<Vec<libc::c_int>> = std::fs::read_dir(listing).ok().map(|entries| {
        entries
            .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse().ok())
            .filter(|&fd| fd > 2)
            .collect()
    });
    // The listing is closed by now, so its own number comes back EBADF, harmlessly.
    let fds = listed.unwrap_or_else(|| {
        // SAFETY: reads a limit.
        let size = unsafe { libc::getdtablesize() }.clamp(3, 1 << 16);
        (3..size).collect()
    });
    for fd in fds {
        // SAFETY: closes a descriptor of this process nothing in it still uses (the caller's rule); EBADF for one that
        // isn't open.
        unsafe { libc::close(fd) };
    }
}

/// `RLIMIT_FSIZE` at `bytes`, soft and hard (`process::sandbox::limit_file_size`), never above the hard limit already
/// set, which only a privileged process could raise.
pub(crate) fn limit_file_size(bytes: u64) -> io::Result<()> {
    let mut now = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: reads this process's own limit into a value on the stack.
    if unsafe { libc::getrlimit(libc::RLIMIT_FSIZE, &mut now) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let cap = (bytes as libc::rlim_t).min(now.rlim_max);
    let limit = libc::rlimit {
        rlim_cur: cap,
        rlim_max: cap,
    };
    // SAFETY: sets this process's own limit from a value on the stack.
    if unsafe { libc::setrlimit(libc::RLIMIT_FSIZE, &limit) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// `RLIMIT_NOFILE`'s soft limit raised to `target`, never past the hard limit (`process::raise_open_files`); the soft
/// limit it is now.
pub(crate) fn raise_open_files(target: u64) -> io::Result<u64> {
    let mut now = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: reads this process's own limit into a value on the stack.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut now) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let want = target.min(now.rlim_max);
    if now.rlim_cur >= want {
        return Ok(now.rlim_cur);
    }
    let raised = libc::rlimit {
        rlim_cur: want,
        rlim_max: now.rlim_max,
    };
    // SAFETY: sets this process's own limit from a value on the stack.
    if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &raised) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(want)
}

/// The owner's read, write and search on `path` if it is a folder and not a link (`process::sandbox::unlock_for_removal`;
/// the caller's rule: nothing changes the tree meanwhile).
pub(crate) fn owner_all(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = path.symlink_metadata()
        && meta.is_dir()
    {
        let mode = meta.permissions().mode() | 0o700;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
    }
}
