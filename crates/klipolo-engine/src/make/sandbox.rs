//! The worker's own sandbox: the paths come from `<session>/in/profile.json`, which the parent wrote before
//! the launch; the worker checks that it is its own session's and applies it with the platform's `sandbox_self`, whose
//! fixed profile (`klipolo_platform::process::sandbox::PROFILE`) names only the session, the bundle, the font folders and
//! this user's Metal cache, and validates every path again (absolute, canonical, no control characters).

use std::io::Read;
use std::path::Path;

use klipolo_platform::process::sandbox::{Applied, SandboxProfile, sandbox_self};

use super::protocol::PROFILE_NAME;
use super::session::IN;

/// `profile.json` is a few paths.
const MAX_PROFILE_BYTES: u64 = 16 * 1024;

/// The profile in `session`'s `in/`, if it is that session's.
pub fn read_profile(session: &Path) -> Result<SandboxProfile, String> {
    let file = std::fs::File::open(session.join(IN).join(PROFILE_NAME))
        .map_err(|e| format!("no profile: {e}"))?;
    let mut text = Vec::new();
    file.take(MAX_PROFILE_BYTES + 1)
        .read_to_end(&mut text)
        .map_err(|e| format!("the profile can't be read: {e}"))?;
    if text.len() as u64 > MAX_PROFILE_BYTES {
        return Err("the profile is too large".into());
    }
    let profile: SandboxProfile =
        serde_json::from_slice(&text).map_err(|e| format!("the profile isn't valid: {e}"))?;
    if profile.session.as_os_str() != session.as_os_str() {
        return Err("the profile is another session's".into());
    }
    Ok(profile)
}

/// Sandboxes this process as `session`'s profile says (macOS), or answers `Unsupported` (Linux until L, Windows until
/// W). An error means the worker must exit before it reads a request.
pub fn apply(session: &Path) -> Result<Applied, String> {
    sandbox_self(&read_profile(session)?)
}
