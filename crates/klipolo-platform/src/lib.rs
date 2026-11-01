//! Klipolo's OS services. Each module exposes one API; the backend for the OS this build targets implements it:
//! - `macos`: the full backend;
//! - `windows` and `linux`: compiling stubs that degrade honestly (no QoS, no activity, normal memory pressure, no
//!   route changes), plus the simple real clock, durable-file and process code;
//! - `fake`: test doubles (a settable clock, recorded durable writes, capabilities).
//!
//! No wgpu, FFmpeg or Tauri here: the GPU, presenter, decode and encode seams live in `klipolo-engine::platform`.

pub mod audio_route;
pub mod caps;
pub mod clock;
pub mod dock;
pub mod durable;
pub mod file;
pub mod folder;
pub mod fonts;
pub mod install;
mod listeners;
pub mod network;
pub mod paths;
pub mod power;
pub mod pressure;
pub mod process;
pub mod qos;
pub mod screen_color;
#[cfg(any(windows, test))]
mod trash_rule;

#[cfg(any(test, feature = "fake"))]
pub mod fake;

#[cfg(unix)]
mod unix;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as imp;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as imp;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as imp;

#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
compile_error!("Klipolo builds for macOS, Windows and Linux only.");
