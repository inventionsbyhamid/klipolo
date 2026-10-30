//! Test doubles (this crate's tests, or the `fake` feature): a settable clock, an atomic write that records its steps
//! and can fail one (the hook a power-loss simulation needs), capabilities to order, and a thermal state to order;
//! and (`seams`) a network path, Low Power Mode, a wake and an install place to order.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use crate::caps::{OsFamily, OsInfo, PlatformCaps};
use crate::clock::Clock;
use crate::durable::{Durability, Os, Steps};
use crate::power::Thermal;

mod seams;

pub use seams::{
    install_place_override, low_power_override, net_path_override, set_install_place,
    set_low_power, set_net_path, wake,
};

/// A clock that only moves when told to.
#[derive(Debug, Default)]
pub struct FakeClock(AtomicU64);

impl FakeClock {
    pub fn new(ns: u64) -> Self {
        Self(AtomicU64::new(ns))
    }

    pub fn set(&self, ns: u64) {
        self.0.store(ns, Ordering::SeqCst);
    }

    pub fn advance(&self, ns: u64) {
        self.0.fetch_add(ns, Ordering::SeqCst);
    }
}

impl Clock for FakeClock {
    fn now_ns(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// One step of an atomic write, as `RecordingDurable` saw it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Call {
    Write(PathBuf),
    Sync(Durability),
    Rename(PathBuf, PathBuf),
    FsyncDir(PathBuf),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Write,
    Sync,
    Rename,
    FsyncDir,
}

/// The error an injected failure gives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// The OS's "disk full" (ENOSPC; `ERROR_DISK_FULL` on Windows).
    Full,
    /// Another program holds the file (a Windows sharing violation, an antivirus scan).
    Locked,
}

impl Fault {
    fn error(self) -> io::Error {
        match self {
            Fault::Full => io::Error::from_raw_os_error(crate::imp::DISK_FULL),
            Fault::Locked => io::ErrorKind::ResourceBusy.into(),
        }
    }
}

/// Does each step for real (`durable::Os`) and records it; `failing` makes one step fail instead.
#[derive(Debug, Default)]
pub struct RecordingDurable {
    calls: Mutex<Vec<Call>>,
    fail: Option<(Step, Fault)>,
}

impl RecordingDurable {
    pub fn new() -> Self {
        Self::default()
    }

    /// `step` fails with `fault` (recorded, but not done).
    pub fn failing(step: Step, fault: Fault) -> Self {
        Self {
            calls: Mutex::default(),
            fail: Some((step, fault)),
        }
    }

    pub fn calls(&self) -> Vec<Call> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn record(&self, call: Call, step: Step) -> io::Result<()> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(call);
        match self.fail {
            Some((failing, fault)) if failing == step => Err(fault.error()),
            _ => Ok(()),
        }
    }
}

impl Steps for RecordingDurable {
    fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<File> {
        self.record(Call::Write(path.into()), Step::Write)?;
        Os.write(path, bytes)
    }

    fn sync(&self, file: &File, durability: Durability) -> io::Result<()> {
        self.record(Call::Sync(durability), Step::Sync)?;
        Os.sync(file, durability)
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.record(Call::Rename(from.into(), to.into()), Step::Rename)?;
        Os.rename(from, to)
    }

    fn fsync_dir(&self, dir: &Path) -> io::Result<()> {
        self.record(Call::FsyncDir(dir.into()), Step::FsyncDir)?;
        Os.fsync_dir(dir)
    }
}

/// Capabilities of another OS, for tests of what reads them.
pub fn fake_caps(family: OsFamily, presenter_native: bool) -> PlatformCaps {
    PlatformCaps {
        os: OsInfo {
            family,
            version: "test".into(),
            arch: "x86_64".into(),
            cpu_arch: "x86_64".into(),
        },
        presenter_native,
        durability_ordered: family == OsFamily::Macos,
        memory_bytes: 16 << 30,
        physical_cores: 4,
        worker_sandbox: family == OsFamily::Macos,
    }
}

/// The thermal state `power::thermal` reports while set (0: the OS's own; 1–4: Nominal to Critical). Process-wide.
static THERMAL: AtomicU8 = AtomicU8::new(0);

/// Makes `power::thermal` report `state` (None: the OS's own again).
pub fn set_thermal(state: Option<Thermal>) {
    let code = match state {
        None => 0,
        Some(Thermal::Nominal) => 1,
        Some(Thermal::Fair) => 2,
        Some(Thermal::Serious) => 3,
        Some(Thermal::Critical) => 4,
    };
    THERMAL.store(code, Ordering::SeqCst);
}

/// The state `set_thermal` set, if any.
pub fn thermal_override() -> Option<Thermal> {
    match THERMAL.load(Ordering::SeqCst) {
        1 => Some(Thermal::Nominal),
        2 => Some(Thermal::Fair),
        3 => Some(Thermal::Serious),
        4 => Some(Thermal::Critical),
        _ => None,
    }
}

/// The fonts seam's Linux answers, whatever OS this build is for (tests of shared code that name the emoji face or the
/// bundled folder), and an activated-font list to order.
pub mod fonts {
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, PoisonError};

    static ACTIVATED: Mutex<Option<Vec<PathBuf>>> = Mutex::new(None);

    pub fn emoji_family() -> &'static str {
        crate::fonts::emoji_family_of(crate::caps::OsFamily::Linux)
    }

    pub fn bundled_fonts_dir(exe_dir: &Path) -> PathBuf {
        exe_dir.join("fonts")
    }

    /// What `fonts::activated_font_files` answers from now on (None: the OS's own answer again).
    pub fn set_activated(files: Option<Vec<PathBuf>>) {
        *ACTIVATED.lock().unwrap_or_else(PoisonError::into_inner) = files;
    }

    /// The list `set_activated` set, if any.
    pub fn activated_override() -> Option<Vec<PathBuf>> {
        ACTIVATED
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// The worker sandbox's double: checks a profile as `process::sandbox::sandbox_self` does, applies nothing, and
/// answers `Sandboxed` unless a test set `Unsupported` (process-wide).
pub mod sandbox {
    use std::sync::atomic::{AtomicBool, Ordering};

    use crate::process::sandbox::{Applied, SandboxProfile, parameters};

    static UNSUPPORTED: AtomicBool = AtomicBool::new(false);

    /// What `sandbox_self` answers from now on.
    pub fn set_applied(applied: Applied) {
        UNSUPPORTED.store(applied == Applied::Unsupported, Ordering::SeqCst);
    }

    pub fn sandbox_self(profile: &SandboxProfile) -> Result<Applied, String> {
        parameters(profile)?;
        Ok(if UNSUPPORTED.load(Ordering::SeqCst) {
            Applied::Unsupported
        } else {
            Applied::Sandboxed
        })
    }
}
