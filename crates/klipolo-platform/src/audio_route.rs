//! Output route and latency changes, and how late the default output plays.
//!
//! macOS: CoreAudio property listeners on the default output device setting and on the current device's latency,
//! safety offset, buffer size and rate. cpal's default unit already follows a switch to AirPods; these tell the mixer
//! to re-read the latency so the audio clock (and the picture) re-anchors. A listener only bumps a counter: CoreAudio
//! is never called from inside it.
//!
//! Windows (W: `IMMNotificationClient`, `IAudioClient::GetStreamLatency`) and Linux (L: PipeWire, `snd_pcm_delay`)
//! report no changes and no latency yet, so lip sync there is off by the device latency.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

pub use crate::imp::RouteWatch;

static CHANGES: AtomicU64 = AtomicU64::new(0);

/// How many route or latency changes the OS has reported so far (any output's watch).
pub fn changes() -> u64 {
    CHANGES.load(Ordering::Acquire)
}

/// A backend's listener saw a change.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn changed() {
    CHANGES.fetch_add(1, Ordering::AcqRel);
}

/// How long after the callback's buffer is due the default output device actually plays it (zero where unknown).
pub fn hardware_latency() -> Duration {
    crate::imp::hardware_latency()
}
