//! The render worker's sandbox on Windows: `Unsupported` for now (later, a restricted job object: no network, a
//! low-integrity token, writes only to the session's `tmp/` and `out/`), so the app sends this worker no agent code.

use crate::process::sandbox::{Applied, SandboxProfile};

/// Windows will validate the profile with the rules its job object needs (`canonicalize` gives `\\?\` paths here).
pub(crate) fn sandbox_self(_profile: &SandboxProfile) -> Result<Applied, String> {
    Ok(Applied::Unsupported)
}

/// Nothing yet (later: the job object's own limits).
pub(crate) fn limit_file_size(_bytes: u64) -> std::io::Result<()> {
    Ok(())
}

/// Nothing (Windows has no soft limit on open handles).
pub(crate) fn raise_open_files(_target: u64) -> std::io::Result<u64> {
    Ok(0)
}

/// Not known yet (later: the job object ends the worker with the app).
pub(crate) fn parent_id() -> Option<u32> {
    None
}

/// Not set yet.
pub(crate) fn file_size_limit() -> Option<u64> {
    None
}

/// Not read-only (later: the job object's own rules).
pub(crate) fn unlock(path: &std::path::Path) {
    if let Ok(meta) = path.symlink_metadata() {
        let mut permissions = meta.permissions();
        if permissions.readonly() && !meta.file_type().is_symlink() {
            #[allow(clippy::permissions_set_readonly_false)]
            permissions.set_readonly(false);
            let _ = std::fs::set_permissions(path, permissions);
        }
    }
}

/// No Metal here: `METAL_CACHE` is the session folder.
pub(crate) fn metal_cache() -> Option<std::path::PathBuf> {
    None
}

/// Nothing: the launcher passes the worker no handles but its pipes (later: the job object's own rules).
pub(crate) fn close_inherited() {}
