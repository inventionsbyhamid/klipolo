//! Klipolo's preview and export engine. No Tauri dependency: the app crate (`crates/klipolo-app`) adapts it.

pub mod api;
pub mod audio;
pub mod decode_wait;
pub mod export;
mod ffmpeg_info;
/// The generated test media's folder, for the unit tests (the integration tests' copy).
#[cfg(test)]
#[path = "../tests/support/fixture_set.rs"]
mod fixture_set;
pub mod frames;
pub mod headless;
pub mod make;
pub mod media;
pub mod plan;
pub mod platform;
pub mod present;
pub mod registry;
pub mod render;
pub mod session;
#[doc(hidden)]
pub mod test_support;
#[allow(dead_code, unused_imports, unused_variables, clippy::all)]
pub mod vendor;

pub use ffmpeg_info::{FfmpegInfo, ffmpeg_info};

/// A file's name for the log: the log names files, never their folders (which may name the user or a client).
pub(crate) fn log_name<P: AsRef<std::path::Path> + ?Sized>(path: &P) -> std::borrow::Cow<'_, str> {
    path.as_ref()
        .file_name()
        .map_or(std::borrow::Cow::Borrowed(""), |n| n.to_string_lossy())
}
// OS services moved to `klipolo-platform` (PS); their engine paths stay for the callers.
pub use klipolo_platform::power as activity;
pub use klipolo_platform::qos;
