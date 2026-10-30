//! The render worker's sandbox: `klipolo --render-worker` checks, compiles and
//! measures agent-written WGSL and scenes, so it closes the descriptors it inherited and then sandboxes itself before
//! it reads a request.
//! - macOS: a fixed Seatbelt profile (`PROFILE`) whose only variables are its parameters, passed through
//!   `sandbox_init_with_parameters` (the symbol Chromium also uses), or, where that symbol is missing, `sandbox_init`
//!   with the values written into the text as escaped string literals (`escaped_profile`);
//! - Linux: `Unsupported` until L (Landlock);
//! - Windows: `Unsupported` until W (a restricted job object).
//!
//! The profile names only the worker's own session folder and fixed app or system paths, never a path a project
//! controls: every input is copied or passed as bytes into the session's `in/`.

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// What the worker may read and write. Every path is absolute and canonical (Seatbelt matches the resolved path:
/// `/tmp/x` never matches a rule for `/private/tmp/x`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxProfile {
    /// The worker's own folder: it reads all of it and writes only its `tmp/` and `out/`.
    pub session: PathBuf,
    /// The app's bundle (its executable, frameworks and bundled fonts), read only.
    pub bundle: PathBuf,
    /// At most `FONT_SLOTS` font folders, read only: the user's fonts folder (`~/Library/Fonts`), the managed fonts
    /// store (`<app data>/fonts`) and a spare. A folder that doesn't exist (yet) names nothing.
    pub font_dirs: Vec<PathBuf>,
}

/// Whether `sandbox_self` sandboxed the process. `Unsupported`: this OS has no worker sandbox yet (Linux until L,
/// Windows until W), so the app sends that worker no agent code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Applied {
    Sandboxed,
    Unsupported,
}

/// The profile's font parameters, `FONTS_0` to `FONTS_2`: always all set, an empty slot to the session folder,
/// since an unset `(param …)` breaks its `subpath`.
pub const FONT_SLOTS: usize = 3;

/// The macOS profile, the smallest with which the worker draws with Metal (the spike's report has
/// the evidence). Its only variables are `(param "SESSION")`, `(param "BUNDLE")`, `(param "FONTS_0")` to
/// `(param "FONTS_2")` and `(param "METAL_CACHE")`.
/// - `system.sb` brings dyld and the shared cache, reads of `/System`, `/usr/lib` and `/usr/share`, and the basic
///   system services (logging, preferences, notifications);
/// - reads: the bundle, the system-wide and the three font folders, and the session;
/// - Metal's shader cache, read only (Step 0): `METAL_CACHE` is this user's cache folder
///   (`$DARWIN_USER_CACHE_DIR`), of which the worker may stat the folder itself and read `com.apple.metal` and
///   `com.apple.metalfe` (compiled shaders, nothing personal). A warm cache cuts the worker's device from about 1.2 s
///   to 0.13 s and a new pipeline from 0.5 s to 0.03 s. Never written: the app (whose executable the worker is) loads
///   the same cache, so a compromised worker could otherwise poison the app's shaders;
/// - writes: only the session's `tmp/` and `out/`, and no mode, BSD flag or ACL changes there
///   `chflags uchg` or a deny-delete ACL would let it lock the parent out of what it wrote; Seatbelt
///   has `file-write-mode`, `file-write-flags` and `file-write-acl`, each probed on 15.7.5). A mode given at creation
///   (`mkdir(path, 0)`) is still possible: the parent's walk fails closed and its removal undoes it
///   (`unlock_for_removal`);
/// - the GPU's user clients. A class rule also matches its subclasses: `IOAccelDevice2`, `IOAccelSharedUserClient2`
///   and `IOAccelCommandQueue` are Intel's `IGAccelDevice`, `IGAccelSharedUserClient` and `IGAccelCommandQueue` (and
///   AMD's), `AGXDeviceUserClient` and `IOGPUDeviceUserClient` Apple silicon's;
/// - one XPC service, Metal's compiler: at `(version 1)` `system.sb` allows looking up any (framework services that
///   reach the network among them), and trustd, which fetches certificate URLs for its callers, is closed too;
/// - no network. Seatbelt tries a rule with a filter before one without, and of two with filters the later, so
///   `system.sb`'s syslog socket needs its own closing rule after `(deny network*)`;
/// - no DTrace helper: `system.sb` opens `/dev/dtracehelper` (the kernel's DOF parser, a known
///   attack surface) to every process; nothing the worker does registers probes. Signals to and task ports of other
///   processes stay refused by `(deny default)` (the Mac test asserts both for the parent).
pub const PROFILE: &str = r#"(version 1)
(deny default)
(import "system.sb")
(allow file-read* (subpath (param "BUNDLE")) (subpath "/Library/Fonts")
       (subpath (param "FONTS_0")) (subpath (param "FONTS_1")) (subpath (param "FONTS_2")) (subpath (param "SESSION")))
(allow file-read-metadata (literal (param "METAL_CACHE")))
(allow file-read* (subpath (string-append (param "METAL_CACHE") "/com.apple.metal"))
       (subpath (string-append (param "METAL_CACHE") "/com.apple.metalfe")))
(allow file-write* (subpath (string-append (param "SESSION") "/tmp")) (subpath (string-append (param "SESSION") "/out")))
(deny file-write-mode file-write-flags file-write-acl (subpath (string-append (param "SESSION") "/tmp")) (subpath (string-append (param "SESSION") "/out")))
(allow iokit-open-user-client (iokit-user-client-class "IOAccelDevice2" "IOAccelSharedUserClient2" "IOAccelCommandQueue"
       "AGXDeviceUserClient" "IOGPUDeviceUserClient"))
(deny mach-lookup (xpc-service-name-prefix "") (global-name "com.apple.trustd" "com.apple.trustd.agent"))
(allow mach-lookup (xpc-service-name "com.apple.MTLCompilerService"))
(deny network*)
(deny network-outbound (literal "/private/var/run/syslog"))
(deny file-read* file-test-existence file-write* file-ioctl (literal "/dev/dtracehelper"))
"#;

/// The names `PROFILE` reads, each set by `parameters`.
pub const PARAMETERS: [&str; 3 + FONT_SLOTS] = [
    "SESSION",
    "BUNDLE",
    "FONTS_0",
    "FONTS_1",
    "FONTS_2",
    "METAL_CACHE",
];

/// Sandboxes this process for good, as `profile` allows (macOS), or answers `Unsupported` (Linux until L, Windows
/// until W). Call `close_inherited` first. An invalid profile is an error on every OS; on macOS so is a profile the
/// kernel refused, and then the worker must exit before it reads a request.
pub fn sandbox_self(profile: &SandboxProfile) -> Result<Applied, String> {
    crate::imp::sandbox::sandbox_self(profile)
}

/// `sandbox_self` through `sandbox_init` and `escaped_profile`, the path taken where `sandbox_init_with_parameters` is
/// missing, for the Mac test that proves both (`klipolo-engine/tests/make_sandbox_mac.rs`).
#[cfg(target_os = "macos")]
#[doc(hidden)]
pub fn sandbox_self_escaped(profile: &SandboxProfile) -> Result<Applied, String> {
    crate::imp::sandbox::sandbox_self_escaped(profile)
}

/// Caps the size of any file this process writes at `bytes` (`RLIMIT_FSIZE`, soft and hard, never above the hard limit
/// it has): the render worker sets it before its sandbox, so one file can't pass the parent's disk cap
/// between two of its polls. A write past it ends the process (`SIGXFSZ`). Windows: nothing (W: the job object).
pub fn limit_file_size(bytes: u64) -> std::io::Result<()> {
    crate::imp::sandbox::limit_file_size(bytes)
}

/// The id of this process's parent (`getppid`), for the render worker's watchdog: it changes when
/// the parent exits and the worker is adopted (by launchd, init or a subreaper), so a worker that outlives the app
/// notices and exits. None where the OS doesn't say (Windows until W, whose job object ends the worker with the app).
pub fn parent_id() -> Option<u32> {
    crate::imp::sandbox::parent_id()
}

/// The size cap `limit_file_size` set (`RLIMIT_FSIZE`'s soft limit), None when unlimited or unknown (Windows).
pub fn file_size_limit() -> Option<u64> {
    crate::imp::sandbox::file_size_limit()
}

/// Gives this user back what a sandboxed worker may have locked on `path`, never following a link: its BSD flags and
/// ACL (macOS: `lchflags(path, 0)`, an empty extended ACL), and on a folder the owner's read, write and search. Only
/// once that worker is gone (after its exit, or a leftover whose recorded worker isn't running), so
/// nothing changes the tree meanwhile. Errors are ignored: the removal that follows reports what still stands.
pub fn unlock_for_removal(path: &Path) {
    crate::imp::sandbox::unlock(path);
}

/// Closes every descriptor above 2 this process inherited: `close_range` where the OS has it (Linux 5.9+), else
/// each one `/dev/fd` lists (macOS). Windows (W: the launcher passes no handles; a job object at W): nothing. Call it
/// first in the worker's `main`, while only one thread runs, before `sandbox_self`.
pub fn close_inherited() {
    crate::imp::sandbox::close_inherited();
}

/// `profile` as the parameters `PROFILE` reads, in `PARAMETERS`' order, each validated: absolute, canonical, UTF-8,
/// no control characters (`"` and `\` are fine: a parameter is a value, never profile text, and `escaped_profile`
/// escapes them). At most `FONT_SLOTS` font folders; a slot with no folder, or whose folder doesn't exist, is the
/// session folder. `METAL_CACHE` is this process's own lookup of this user's cache folder (macOS), never a value from
/// `profile`, else the session folder.
pub fn parameters(profile: &SandboxProfile) -> Result<Vec<(&'static str, String)>, String> {
    if profile.font_dirs.len() > FONT_SLOTS {
        return Err(format!(
            "at most {FONT_SLOTS} font folders, not {}",
            profile.font_dirs.len()
        ));
    }
    let session = existing("session", &profile.session)?;
    let bundle = existing("bundle", &profile.bundle)?;
    let mut values = vec![("SESSION", session.clone()), ("BUNDLE", bundle)];
    for (slot, name) in PARAMETERS[2..2 + FONT_SLOTS].iter().enumerate() {
        let folder = match profile.font_dirs.get(slot) {
            Some(dir) => {
                let text = plain("font folder", dir)?;
                if dir.exists() {
                    canonical("font folder", dir)?;
                    text
                } else {
                    session.clone()
                }
            }
            None => session.clone(),
        };
        values.push((name, folder));
    }
    let metal = crate::imp::sandbox::metal_cache()
        .and_then(|dir| existing("Metal cache", &dir).ok())
        .unwrap_or(session);
    values.push(("METAL_CACHE", metal));
    Ok(values)
}

/// `PROFILE` with each `(param "NAME")` replaced by its value as an escaped string literal (`\` and `"` escaped;
/// `parameters` already refused control characters): for `sandbox_init`, where `sandbox_init_with_parameters` is
/// missing.
pub fn escaped_profile(parameters: &[(&str, String)]) -> Result<String, String> {
    let mut text = PROFILE.to_owned();
    for (name, value) in parameters {
        if value.chars().any(char::is_control) {
            return Err(format!("{name} has a control character"));
        }
        let literal = value.replace('\\', "\\\\").replace('"', "\\\"");
        text = text.replace(&format!("(param \"{name}\")"), &format!("\"{literal}\""));
    }
    if text.contains("(param ") {
        return Err("a profile parameter has no value".into());
    }
    Ok(text)
}

/// `path` as text: absolute, only plain names after the root (no `.` or `..`), UTF-8, no control characters.
fn plain(what: &str, path: &Path) -> Result<String, String> {
    if !path.is_absolute() {
        return Err(format!("the {what} isn't an absolute path"));
    }
    let only_names = path.components().all(|c| {
        matches!(
            c,
            Component::RootDir | Component::Prefix(_) | Component::Normal(_)
        )
    });
    if !only_names {
        return Err(format!("the {what} has a . or .. in it"));
    }
    let text = path
        .to_str()
        .ok_or_else(|| format!("the {what} isn't UTF-8"))?;
    if text.chars().any(char::is_control) {
        return Err(format!("the {what} has a control character"));
    }
    Ok(text.to_owned())
}

/// `path` resolves to itself, byte for byte: no link on the way (Seatbelt matches the resolved path), no `/./` or
/// trailing `/` (which `Path`'s own comparison ignores, but the profile's text wouldn't).
fn canonical(what: &str, path: &Path) -> Result<(), String> {
    match std::fs::canonicalize(path) {
        Ok(resolved) if resolved.as_os_str() == path.as_os_str() => Ok(()),
        Ok(_) => Err(format!("the {what} isn't canonical (a link on its way)")),
        Err(e) => Err(format!("the {what} can't be resolved: {e}")),
    }
}

/// A folder that must exist: `plain` and `canonical`.
fn existing(what: &str, path: &Path) -> Result<String, String> {
    let text = plain(what, path)?;
    canonical(what, path)?;
    Ok(text)
}

#[cfg(test)]
mod tests;
