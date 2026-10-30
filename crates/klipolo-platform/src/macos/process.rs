//! Processes on macOS: a process's start and state, its memory, its CPU band and executable, and this boot's id and
//! the computer's memory; the processes running from a folder, with their mode arguments.

use std::io;
use std::path::{Path, PathBuf};

/// `proc_pidinfo(PROC_PIDTBSDINFO)`: the start in µs since 1970, and whether it is a zombie. A non-zero argument also
/// finds a process that has exited but isn't reaped yet (with 0 a zombie doesn't answer). None when the process is
/// gone or isn't this user's.
pub(crate) fn status(pid: u32) -> Option<crate::unix::Status> {
    const ZOMBIES_TOO: u64 = 1;
    let id = crate::unix::one_pid(pid).ok()?;
    // SAFETY: plain integers; all zeroes is a valid value.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: the flavor's out-struct, with its size passed; the call writes at most that many bytes.
    let written = unsafe {
        libc::proc_pidinfo(
            id,
            libc::PROC_PIDTBSDINFO,
            ZOMBIES_TOO,
            (&raw mut info).cast(),
            size,
        )
    };
    (written == size && info.pbi_pid == pid).then(|| crate::unix::Status {
        started: info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec,
        zombie: info.pbi_status == libc::SZOMB,
    })
}

/// This process's footprint (`process_memory`): the anonymous memory a bench counts.
pub(crate) fn own_anonymous_memory() -> Option<u64> {
    process_memory(std::process::id()).map(|m| m.footprint)
}

/// `proc_pid_rusage(RUSAGE_INFO_V2)`: `ri_phys_footprint` (Activity Monitor's "Memory") and `ri_resident_size`. None
/// once the process has exited: a zombie still answers, with its last resident size and no footprint.
pub(crate) fn process_memory(pid: u32) -> Option<crate::process::ProcessMemory> {
    let id = crate::unix::one_pid(pid).ok()?;
    // SAFETY: plain integers; all zeroes is a valid value.
    let mut info: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
    // SAFETY: the flavor's out-struct (libc types the buffer as `rusage_info_t *`, after the C prototype); the call
    // writes one `rusage_info_v2`.
    let ok =
        unsafe { libc::proc_pid_rusage(id, libc::RUSAGE_INFO_V2, (&raw mut info).cast()) } == 0;
    // Asked after the figures, so a process that exited before them never passes.
    let running = ok && status(pid).is_some_and(|s| !s.zombie);
    running.then_some(crate::process::ProcessMemory {
        footprint: info.ri_phys_footprint,
        resident: info.ri_resident_size,
    })
}

/// `setpriority(PRIO_DARWIN_PROCESS)`: into the darwin background band (lowest CPU priority, throttled disk and
/// network I/O) or out of it, once `verify` has found the process that started at `started`.
pub(crate) fn set_background(pid: u32, started: u64, on: bool) -> io::Result<()> {
    let id = crate::unix::verify(pid, started)?;
    let value = if on { libc::PRIO_DARWIN_BG } else { 0 };
    // SAFETY: changes one process's band (`one_pid` refused 0, which would mean this process).
    if unsafe { libc::setpriority(libc::PRIO_DARWIN_PROCESS, id as libc::id_t, value) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// `proc_pidpath`: the executable's path as the kernel resolved it at exec. None when the process is gone or isn't
/// this user's.
pub(crate) fn exe_path(pid: u32) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let id = crate::unix::one_pid(pid).ok()?;
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: the buffer and its size; the call writes at most that many bytes and returns the length.
    let len = unsafe { libc::proc_pidpath(id, buf.as_mut_ptr().cast(), buf.len() as u32) };
    let len = usize::try_from(len)
        .ok()
        .filter(|&n| n > 0 && n <= buf.len())?;
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(&buf[..len])))
}

/// `kern.bootsessionuuid`: a UUID made at each boot.
pub(crate) fn boot_id() -> Option<String> {
    let mut buf = [0u8; 64];
    let mut size = buf.len();
    // SAFETY: a NUL-terminated name, and a byte buffer whose size is passed with it (the call writes at most that).
    let ok = unsafe {
        libc::sysctlbyname(
            c"kern.bootsessionuuid".as_ptr(),
            buf.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } == 0;
    if !ok {
        return None;
    }
    let id = std::ffi::CStr::from_bytes_until_nul(&buf[..size.min(buf.len())]).ok()?;
    let id = id.to_str().ok()?;
    (!id.is_empty()).then(|| id.to_owned())
}

/// `hw.memsize` (0 when it can't be read).
pub(crate) fn memory_bytes() -> u64 {
    let mut value: u64 = 0;
    let mut size = std::mem::size_of::<u64>();
    // SAFETY: a NUL-terminated name, and a u64 output buffer whose size is passed with it.
    let ok = unsafe {
        libc::sysctlbyname(
            c"hw.memsize".as_ptr(),
            (&raw mut value).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } == 0;
    if ok { value } else { 0 }
}

/// Every process's id (`proc_listallpids`) whose executable (`process::exe_path`) lies under `bundle`, but our own,
/// with its mode from `KERN_PROCARGS2`. `Err` when the list can't be read.
pub(crate) fn running_from(bundle: &Path) -> io::Result<Vec<crate::process::Running>> {
    let own = std::process::id();
    let mut args = Vec::new();
    Ok(all_pids()?
        .into_iter()
        .filter(|&pid| pid != own)
        .filter(|&pid| crate::process::exe_path(pid).is_some_and(|exe| exe.starts_with(bundle)))
        .map(|pid| crate::process::Running {
            pid,
            mode: mode(pid, &mut args),
        })
        .collect())
}

/// `proc_listallpids`, into a buffer with room for processes started since it was sized; a full buffer may have
/// missed some, so it grows and asks again. A failed call, or a list that keeps growing, is an error: missing a process
/// would read as "nothing runs".
fn all_pids() -> io::Result<Vec<u32>> {
    // SAFETY: a null buffer asks only for the count.
    let mut room = usize::try_from(unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) })
        .unwrap_or(0)
        + 64;
    for _ in 0..4 {
        let mut pids: Vec<libc::pid_t> = vec![0; room];
        let bytes = libc::c_int::try_from(room * std::mem::size_of::<libc::pid_t>())
            .unwrap_or(libc::c_int::MAX);
        // SAFETY: the buffer and its size in bytes; the call writes at most that and returns how many ids it wrote.
        let count = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
        let Ok(count) = usize::try_from(count) else {
            return Err(io::Error::last_os_error());
        };
        if count < room {
            pids.truncate(count);
            return Ok(pids
                .into_iter()
                .filter_map(|pid| u32::try_from(pid).ok())
                .filter(|&pid| pid != 0)
                .collect());
        }
        room *= 2;
    }
    Err(io::Error::other("the process list kept growing"))
}

/// `pid`'s mode argument from `sysctl(KERN_PROCARGS2)`, read into `buf` (sized once to `KERN_ARGMAX`: a smaller
/// buffer gets the end of the argument area, not its start). None when it can't be read (another user's process, one
/// that has exited) or has none.
fn mode(pid: u32, buf: &mut Vec<u8>) -> Option<String> {
    if buf.is_empty() {
        *buf = vec![0; args_max()];
    }
    let mut name = [
        libc::CTL_KERN,
        libc::KERN_PROCARGS2,
        crate::unix::one_pid(pid).ok()?,
    ];
    let mut size = buf.len();
    // SAFETY: a three-int name, and the buffer with its size; the call writes at most that many bytes into it.
    let read = unsafe {
        libc::sysctl(
            name.as_mut_ptr(),
            3,
            buf.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } == 0;
    if !read {
        return None;
    }
    crate::process::mode_of(&first_args(&buf[..size.min(buf.len())])?)
}

/// `KERN_ARGMAX`, the most an argument area holds (1 MiB when it can't be read).
fn args_max() -> usize {
    let mut name = [libc::CTL_KERN, libc::KERN_ARGMAX];
    let mut max: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>();
    // SAFETY: a two-int name, and an int-sized output with its size.
    let read = unsafe {
        libc::sysctl(
            name.as_mut_ptr(),
            2,
            (&raw mut max).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } == 0;
    usize::try_from(max)
        .ok()
        .filter(|&max| read && max > 0)
        .unwrap_or(1 << 20)
}

/// The first two arguments in a `KERN_PROCARGS2` buffer: `argc` (an int), the executable's path, NUL padding, then the
/// arguments, each ending in a NUL. Fewer when `argc` is smaller; None when the buffer is cut short.
fn first_args(buf: &[u8]) -> Option<Vec<String>> {
    let argc = i32::from_ne_bytes(buf.get(..4)?.try_into().ok()?);
    let rest = buf.get(4..)?;
    let rest = &rest[rest.iter().position(|&b| b == 0)?..];
    let rest = &rest[rest.iter().position(|&b| b != 0)?..];
    let want = usize::try_from(argc).ok()?.min(2);
    let args: Vec<String> = rest
        .split(|&b| b == 0)
        .take(want)
        .map(|arg| String::from_utf8_lossy(arg).into_owned())
        .collect();
    (args.len() == want).then_some(args)
}
