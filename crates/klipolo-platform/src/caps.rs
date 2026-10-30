//! What this computer's platform offers (capabilities are data, not `cfg`). This crate reports static facts only;
//! the engine adds the GPU and encoders it probes. Logged once when the engine starts.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum OsFamily {
    Macos,
    Windows,
    Linux,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct OsInfo {
    pub family: OsFamily,
    /// The OS release: "15.6.1" (macOS), "10.0.22631" (Windows), the kernel release (Linux); empty when unknown.
    pub version: String,
    /// The CPU architecture this build runs as: "x86_64" or "aarch64".
    pub arch: String,
    /// The CPU's own architecture, also when this build runs translated: an x86_64 build under Rosetta (macOS) or on
    /// Windows on Arm reports "aarch64". Export estimates go by it (Apple silicon has a media engine).
    pub cpu_arch: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PlatformCaps {
    pub os: OsInfo,
    /// The preview can present natively under the webview (macOS's CAMetalLayer); elsewhere frames go over the
    /// socket.
    pub presenter_native: bool,
    /// Writes can be ordered after earlier ones without a full flush (`F_BARRIERFSYNC`), so saves are cheap.
    pub durability_ordered: bool,
    /// This computer's memory in bytes (0 when the OS won't say): hardware classes and model budgets go by it.
    /// - macOS: `hw.memsize`, what is installed;
    /// - Linux: `/proc/meminfo`'s `MemTotal`; Windows: `GlobalMemoryStatusEx`'s `ullTotalPhys`. Both are the memory
    ///   the OS can use, which leaves out what firmware and the kernel reserve, so a 16 GB PC reads about 15.6 to 15.8
    ///   GiB.
    ///
    /// So a class threshold sits between two tiers (12 GiB between 8 and 16 GB), never at one.
    pub memory_bytes: u64,
    /// CPU cores (at least 1): model thread counts go by them, since a core's second hardware thread adds little to
    /// matrix work.
    /// - macOS: `hw.physicalcpu`; Windows: `GetLogicalProcessorInformation`'s cores;
    /// - Linux: `/proc/cpuinfo`'s `physical id` and `cpu cores`. Arm Linux lists neither, so there it counts logical
    ///   CPUs (what this process may use).
    pub physical_cores: u32,
    /// The render worker can sandbox itself here (`process::sandbox_self`: macOS's Seatbelt). Windows and Linux get
    /// theirs at W and L; until then agent-made code is never checked there, so the Make tools are off.
    pub worker_sandbox: bool,
}

/// Whether this OS has a sandbox for the render worker (see `PlatformCaps::worker_sandbox`).
pub const WORKER_SANDBOX: bool = cfg!(target_os = "macos");

/// This computer's capabilities (cheap: nothing is probed).
pub fn current() -> PlatformCaps {
    PlatformCaps {
        os: OsInfo {
            family: FAMILY,
            version: crate::imp::os_version(),
            arch: std::env::consts::ARCH.into(),
            cpu_arch: crate::imp::cpu_arch(),
        },
        presenter_native: cfg!(target_os = "macos"),
        durability_ordered: cfg!(target_os = "macos"),
        memory_bytes: crate::imp::memory_bytes(),
        physical_cores: (num_cpus::get_physical() as u32).max(1),
        worker_sandbox: WORKER_SANDBOX,
    }
}

#[cfg(target_os = "macos")]
pub(crate) const FAMILY: OsFamily = OsFamily::Macos;
#[cfg(windows)]
pub(crate) const FAMILY: OsFamily = OsFamily::Windows;
#[cfg(target_os = "linux")]
pub(crate) const FAMILY: OsFamily = OsFamily::Linux;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_static_report_describes_this_build() {
        let caps = current();
        assert_eq!(caps.os.family, FAMILY);
        assert_eq!(caps.os.arch, std::env::consts::ARCH);
        assert!(["x86_64", "aarch64"].contains(&caps.os.cpu_arch.as_str()));
        if caps.os.arch == "aarch64" {
            assert_eq!(
                caps.os.cpu_arch, "aarch64",
                "an Arm build runs on an Arm CPU"
            );
        }
        assert!(!caps.os.version.is_empty());
        assert_eq!(caps.presenter_native, cfg!(target_os = "macos"));
        assert!(caps.memory_bytes >= 1 << 30, "at least 1 GiB");
        assert!(caps.physical_cores >= 1);
        assert_eq!(
            caps.worker_sandbox,
            cfg!(target_os = "macos"),
            "macOS only until W and L"
        );
        let fake = crate::fake::fake_caps(OsFamily::Windows, false);
        assert_eq!(fake.os.family, OsFamily::Windows);
        assert!(!fake.presenter_native && !fake.durability_ordered && !fake.worker_sandbox);
        assert!(crate::fake::fake_caps(OsFamily::Macos, true).worker_sandbox);
    }
}
