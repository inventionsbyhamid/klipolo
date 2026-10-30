//! The network path, so update downloads and telemetry uploads wait while the connection costs the person money or
//! data:
//! - macOS: `NWPathMonitor` (Low Data Mode is `isConstrained`; cellular and a personal hotspot are `isExpensive`);
//! - Windows (W): `GetNetworkConnectivityHint`'s cost (a fixed or variable cost is constrained, roaming or over the data
//!   limit expensive) and `NotifyNetworkConnectivityHintChange`. Stub: the default;
//! - Linux (L): NetworkManager's `Metered` (yes or guess-yes is constrained) and its `PropertiesChanged` signal. Stub:
//!   the default.
//!
//! Nothing polls: the OS's monitor reports each change, and `path` reads the latest report.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Once};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::listeners::Listeners;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NetPath {
    pub online: bool,
    /// Low Data Mode (macOS `isConstrained`), a metered connection elsewhere.
    pub constrained: bool,
    /// A cellular or personal-hotspot path (macOS `isExpensive`).
    pub expensive: bool,
}

impl Default for NetPath {
    /// Online, neither constrained nor expensive: what an OS without a signal says, so nothing pauses for nothing.
    fn default() -> Self {
        Self {
            online: true,
            constrained: false,
            expensive: false,
        }
    }
}

const ONLINE: u8 = 1;
const CONSTRAINED: u8 = 2;
const EXPENSIVE: u8 = 4;

impl NetPath {
    /// The three flags in three bits (`PATH` and the fake keep them in one atomic byte).
    pub(crate) fn bits(self) -> u8 {
        [
            (self.online, ONLINE),
            (self.constrained, CONSTRAINED),
            (self.expensive, EXPENSIVE),
        ]
        .into_iter()
        .filter(|&(on, _)| on)
        .fold(0, |bits, (_, bit)| bits | bit)
    }

    pub(crate) fn from_bits(bits: u8) -> Self {
        Self {
            online: bits & ONLINE != 0,
            constrained: bits & CONSTRAINED != 0,
            expensive: bits & EXPENSIVE != 0,
        }
    }
}

/// The latest path the OS reported (the default until it does).
static PATH: AtomicU8 = AtomicU8::new(ONLINE);

static LISTENERS: Listeners<dyn Fn(NetPath) + Send + Sync> = Listeners::new();

/// The path now. The first call starts the OS's monitor on a utility queue; until it answers, and where the OS has no
/// signal, the default. Never blocks.
pub fn path() -> NetPath {
    #[cfg(any(test, feature = "fake"))]
    if let Some(path) = crate::fake::net_path_override() {
        return path;
    }
    watch();
    NetPath::from_bits(PATH.load(Ordering::Relaxed))
}

/// `f` hears every change on the monitor's queue (keep it short). Stubs never call it.
pub fn on_change(f: impl Fn(NetPath) + Send + Sync + 'static) {
    LISTENERS.add(Arc::new(f));
    watch();
}

/// The backend's report: stored, and when it differs from the last one, heard by every listener, outside any lock.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn report(path: NetPath) {
    if PATH.swap(path.bits(), Ordering::Relaxed) != path.bits() {
        notify(path);
    }
}

/// Every `on_change` listener hears `path` (the backend's change, or the fake's).
#[cfg_attr(
    not(any(target_os = "macos", test, feature = "fake")),
    allow(dead_code)
)]
pub(crate) fn notify(path: NetPath) {
    for listener in LISTENERS.all() {
        listener(path);
    }
}

/// Starts the OS's monitor (once per process; later calls do nothing).
fn watch() {
    static STARTED: Once = Once::new();
    STARTED.call_once(crate::imp::watch_net_path);
}
