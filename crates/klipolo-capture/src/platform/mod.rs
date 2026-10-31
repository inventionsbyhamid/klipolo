//! The OS capture backends, behind the `real-capture` feature, which only the app turns on (capture
//! capture consent). Each OS's backend starts as one that can't record and fills in over its tasks: macOS 20-24,
//! Windows 26-29, Linux 30-32. Their pure helpers (`common`) are built on every OS, without `real-capture`, so every
//! OS's tests run them; the FFmpeg encoder for Windows and Linux (`ffmpeg_encode`) joins this module later.

pub mod common;

#[cfg(feature = "real-capture")]
use crate::backend::Backend;
#[cfg(feature = "real-capture")]
use crate::shell::{Shell, ShellIo, ShellOutput};

/// The macOS backend; its pure helpers are public for the Mac's tests (`tests/macos_*.rs`).
#[cfg(all(feature = "real-capture", target_os = "macos"))]
pub mod macos;
#[cfg(all(feature = "real-capture", target_os = "macos"))]
use self::macos as imp;

// `self::`: once the `windows` crate is a dependency, a bare `windows` is ambiguous (E0659).
#[cfg(all(feature = "real-capture", windows))]
mod windows;
#[cfg(all(feature = "real-capture", windows))]
use self::windows as imp;

#[cfg(all(feature = "real-capture", target_os = "linux"))]
mod linux;
#[cfg(all(feature = "real-capture", target_os = "linux"))]
use self::linux as imp;

#[cfg(feature = "real-capture")]
mod probe_only;

/// The backend for the probe alone, without the consent guard: only `caps()` and `probe()` work on it (the others
/// return Unsupported or nothing), and the probe encodes generated frames only. `worker --probe` and the ignored probe
/// test use it; every OS has one.
#[cfg(feature = "real-capture")]
pub fn backend_for_probe() -> Box<dyn Backend> {
    Box::new(probe_only::ProbeOnly(imp::OsBackend::new()))
}

/// This OS's capture backend. The consent guard runs first, so a test or CI build with `real-capture` panics here
/// before any OS call.
#[cfg(feature = "real-capture")]
pub fn backend() -> Box<dyn Backend> {
    crate::guard::assert_real_capture_allowed("the OS capture backend");
    Box::new(imp::OsBackend::new())
}

/// The shell of a backend that has none yet: no item, no hotkeys; it waits for `Quit`.
#[cfg(feature = "real-capture")]
struct NoShell;

#[cfg(feature = "real-capture")]
impl Shell for NoShell {
    fn run(self: Box<Self>, io: ShellIo) {
        while let Ok(out) = io.output.recv() {
            if out == ShellOutput::Quit {
                break;
            }
        }
    }
}
