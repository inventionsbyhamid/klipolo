//! The render worker's sandbox on Linux: `Unsupported` until L (Landlock: reads of the session folder, the bundle and
//! the font folders, writes to the session's `tmp/` and `out/`, and no network, from the same parameters), so the app
//! sends this worker no agent code.

use crate::process::sandbox::{Applied, SandboxProfile, parameters};

pub(crate) fn sandbox_self(profile: &SandboxProfile) -> Result<Applied, String> {
    parameters(profile)?;
    Ok(Applied::Unsupported)
}

pub(crate) fn limit_file_size(bytes: u64) -> std::io::Result<()> {
    crate::unix::limit_file_size(bytes)
}

pub(crate) fn raise_open_files(target: u64) -> std::io::Result<u64> {
    crate::unix::raise_open_files(target)
}

/// `getppid()`: the process that started this one, or what adopted it once that exited (launchd, init, a subreaper).
pub(crate) fn parent_id() -> Option<u32> {
    // SAFETY: getppid takes nothing and can't fail.
    Some(unsafe { libc::getppid() } as u32)
}

/// This process's file size limit (`RLIMIT_FSIZE`, soft), None when unlimited or unknown.
pub(crate) fn file_size_limit() -> Option<u64> {
    // SAFETY: a plain out-struct of two integers, written by getrlimit.
    let mut r: libc::rlimit = unsafe { std::mem::zeroed() };
    // SAFETY: a valid resource and a pointer to that struct.
    let ok = unsafe { libc::getrlimit(libc::RLIMIT_FSIZE, &raw mut r) } == 0;
    (ok && r.rlim_cur != libc::RLIM_INFINITY).then_some(r.rlim_cur)
}

/// The owner's access on a folder (Linux has no flags a user can set that stop a removal, and no ACL its sandbox would
/// allow at L).
pub(crate) fn unlock(path: &std::path::Path) {
    crate::unix::owner_all(path);
}

/// No Metal here: `METAL_CACHE` is the session folder.
pub(crate) fn metal_cache() -> Option<std::path::PathBuf> {
    None
}

/// `close_range(3, ~0, 0)` (Linux 5.9+), else each descriptor `/proc/self/fd` lists.
pub(crate) fn close_inherited() {
    // SAFETY: closes this process's descriptors above 2, which nothing in it uses yet (the caller's rule).
    let closed = unsafe { libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, 0u32) } == 0;
    if !closed {
        crate::unix::close_above_stderr("/proc/self/fd");
    }
}
