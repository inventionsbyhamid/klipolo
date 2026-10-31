//! From Cap (https://github.com/CapSoftware/Cap) at d327051: its MIT `scap-*` crates and its other crates (used under
//! the AGPL-3.0), as `NOTICE` lists them.

/// Cap's MIT `scap-screencapturekit`: the ScreenCaptureKit delegate and stream settings over cidre (macOS).
#[cfg(target_os = "macos")]
pub(crate) mod sck;
