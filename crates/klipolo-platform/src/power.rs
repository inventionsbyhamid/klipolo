//! Keeps the OS from napping or idle-sleeping the process while an export runs (`begin`) or a take records
//! (`begin_capture`), and says how hot the computer runs, how much battery it has left, whether Low Power Mode is on
//! and when the computer wakes (update downloads and uploads wait in Low Power Mode, and a wake is a moment to check).
//! macOS: an NSProcessInfo activity (an App-Napped process measured ~10× slower). Windows (later: `PowerSetRequest`)
//! and Linux (later: a logind or portal inhibit) hold nothing yet.

use std::sync::{Arc, Once};

pub use crate::imp::Activity;

use crate::listeners::Listeners;

/// Holds the system awake (and the process un-napped) for `reason` until the guard drops.
pub fn begin(reason: &str) -> Activity {
    crate::imp::begin_activity(reason)
}

/// The recorder's activity for the length of a take: no App Nap, no idle system or display sleep, no sudden or
/// automatic termination, latency-critical.
/// - macOS: NSActivityOptions UserInitiated | LatencyCritical | IdleSystemSleepDisabled |
///   IdleDisplaySleepDisabled | SuddenTerminationDisabled | AutomaticTerminationDisabled (`capture_options`).
/// - Windows (later): PowerSetRequest System, Display, ExecutionRequired; until then, `begin`'s.
/// - Linux (later): a logind sleep:idle inhibitor (the portal inside Flatpak); until then, `begin`'s.
///
/// It opens no device and starts no capture: it only keeps the computer awake while a take the person started records
/// (klipolo-capture's `shell_view::TakeActivity`, its only caller, holds it from the gate until Stop's finishing ends).
pub fn begin_capture(reason: &str) -> Activity {
    crate::imp::begin_capture(reason)
}

/// How hot the computer runs. Background work (editing copies) pauses at `Serious` or worse.
/// - macOS: `NSProcessInfo.thermalState`.
/// - Windows (later): no public thermal state; the effective power mode
///   (`PowerRegisterForEffectivePowerModeNotifications`) maps battery saver to `Fair`. Stub: `Nominal`.
/// - Linux (later): power-profiles-daemon's `PerformanceDegraded`, else the thermal zones' trip points. Stub:
///   `Nominal`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Thermal {
    Nominal,
    Fair,
    Serious,
    Critical,
}

/// The computer's thermal state now (tests can set it: `fake::set_thermal`).
pub fn thermal() -> Thermal {
    #[cfg(any(test, feature = "fake"))]
    if let Some(t) = crate::fake::thermal_override() {
        return t;
    }
    crate::imp::thermal()
}

/// The computer runs at its usual speed now: not in Low Power Mode, and not warm (`Fair` or worse). Only a speed
/// measured while it is quiet says anything about the device (transcription's speed check).
pub fn quiet() -> bool {
    !low_power() && thermal() < Thermal::Fair
}

/// A battery's charge (the recorder's preflight warns below 20 % while it drains).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Battery {
    /// 0–100.
    pub percent: u8,
    /// On the charger, charging or full: it isn't draining.
    pub charging: bool,
}

/// The battery now; None on a computer without one, or where it can't be read.
/// - Linux: `/sys/class/power_supply/BAT*/{capacity,status}`, every battery together.
/// - macOS: IOPowerSources' internal battery (a UPS doesn't count).
/// - Windows (later: `GetSystemPowerStatus`): None until then.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn battery() -> Option<Battery> {
    crate::imp::battery()
}

/// The battery now: None until Windows reads `GetSystemPowerStatus`.
#[cfg(windows)]
pub fn battery() -> Option<Battery> {
    None
}

/// Low Power Mode (macOS 12+: `NSProcessInfo.isLowPowerModeEnabled`); later Windows battery saver and
/// power-profiles-daemon's `power-saver` on Linux: the OS slows the computer down to save its battery. False when
/// unknown (tests can set it: `fake::set_low_power`).
pub fn low_power() -> bool {
    #[cfg(any(test, feature = "fake"))]
    if let Some(on) = crate::fake::low_power_override() {
        return on;
    }
    crate::imp::low_power()
}

static WAKE: Listeners<dyn Fn() + Send + Sync> = Listeners::new();

/// `f` runs after the computer wakes from sleep (macOS `NSWorkspaceDidWakeNotification`, on the main thread where
/// AppKit posts it: keep it short; later Windows `RegisterSuspendResumeNotification` and Linux logind
/// `PrepareForSleep(false)`). Stubs never call it (tests can: `fake::wake`).
pub fn on_wake(f: impl Fn() + Send + Sync + 'static) {
    WAKE.add(Arc::new(f));
    static WATCHING: Once = Once::new();
    WATCHING.call_once(crate::imp::watch_wake);
}

/// The backend's (or the fake's) wake: every `on_wake` listener runs, outside any lock.
#[cfg_attr(
    not(any(target_os = "macos", test, feature = "fake")),
    allow(dead_code)
)]
pub(crate) fn woke() {
    for listener in WAKE.all() {
        listener();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn battery_reads_without_panicking() {
        // None on a desktop or a computer whose battery can't be read; else a charge from 0 to 100.
        if let Some(b) = battery() {
            assert!(b.percent <= 100, "{b:?}");
        }
    }

    #[test]
    fn low_power_mode_reads_without_panicking() {
        // Off on Windows and Linux until they read it; the person's setting on a Mac.
        if !cfg!(target_os = "macos") {
            assert!(!low_power());
        }
    }

    #[test]
    fn a_set_thermal_state_wins_until_cleared() {
        crate::fake::set_thermal(Some(Thermal::Serious));
        assert_eq!(thermal(), Thermal::Serious);
        crate::fake::set_thermal(None);
        // The OS's own answer again (the Windows and Linux stubs say Nominal).
        if !cfg!(target_os = "macos") {
            assert_eq!(thermal(), Thermal::Nominal);
        }
    }
}
