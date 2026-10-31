//! Cap's MIT `scap-screencapturekit` (https://github.com/CapSoftware/Cap at d327051); `NOTICE` lists every Cap-derived
//! file.

/// Cap's MIT `scap-screencapturekit`: the ScreenCaptureKit delegate and stream settings over cidre (macOS).
#[cfg(target_os = "macos")]
pub(crate) mod sck;
