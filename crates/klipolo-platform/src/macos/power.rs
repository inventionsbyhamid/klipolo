//! An NSProcessInfo activity: no App Nap and no idle system sleep while it lives (moved from
//! `klipolo-engine/src/activity.rs`), and the recorder's stronger one for a take (`begin_capture`); the thermal state;
//! the battery (IOPowerSources); Low Power Mode; and the wake from sleep (NSWorkspace).

use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject};
use objc2_foundation::{
    NSActivityOptions, NSArray, NSDictionary, NSNotification, NSNumber, NSProcessInfo,
    NSProcessInfoThermalState, NSString, ns_string,
};

use crate::power::{Battery, Thermal};

// IOKit's power sources. Their CF objects are toll-free bridged: the info blob, the list (an array) and each source's
// description (a dictionary).
#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    /// +1; null when there is nothing to report.
    fn IOPSCopyPowerSourcesInfo() -> *mut AnyObject;
    /// +1.
    fn IOPSCopyPowerSourcesList(blob: *const AnyObject) -> *mut NSArray<AnyObject>;
    /// +0, owned by `blob`.
    fn IOPSGetPowerSourceDescription(
        blob: *const AnyObject,
        source: *const AnyObject,
    ) -> *const NSDictionary<NSString, AnyObject>;
}

/// A user-initiated activity with idle system sleep disabled; it ends when dropped.
pub struct Activity(Retained<ProtocolObject<dyn NSObjectProtocol>>);

pub fn begin(reason: &str) -> Activity {
    let info = NSProcessInfo::processInfo();
    let options = NSActivityOptions::UserInitiated | NSActivityOptions::IdleSystemSleepDisabled;
    Activity(info.beginActivityWithOptions_reason(options, &NSString::from_str(reason)))
}

/// The take's activity (`power::begin_capture`), with the recorder's options (`capture_options`).
pub fn begin_capture(reason: &str) -> Activity {
    let info = NSProcessInfo::processInfo();
    Activity(info.beginActivityWithOptions_reason(capture_options(), &NSString::from_str(reason)))
}

/// A take's activity options: user-initiated and latency-critical (no App Nap, no timer coalescing on the
/// capture threads), with neither the system nor the display idle-sleeping (a static talking-head take never sees
/// input, and a sleeping display stops ScreenCaptureKit), and macOS never quitting the recorder on its own (sudden or
/// automatic termination).
pub fn capture_options() -> NSActivityOptions {
    NSActivityOptions::UserInitiated
        | NSActivityOptions::LatencyCritical
        | NSActivityOptions::IdleSystemSleepDisabled
        | NSActivityOptions::IdleDisplaySleepDisabled
        | NSActivityOptions::SuddenTerminationDisabled
        | NSActivityOptions::AutomaticTerminationDisabled
}

impl Drop for Activity {
    fn drop(&mut self) {
        // SAFETY: ends the activity this guard began, exactly once.
        unsafe { NSProcessInfo::processInfo().endActivity(&self.0) };
    }
}

/// `NSProcessInfo.thermalState` (0 nominal to 3 critical, in order); an unknown newer value counts as critical.
pub fn thermal() -> Thermal {
    match NSProcessInfo::processInfo().thermalState() {
        NSProcessInfoThermalState::Nominal => Thermal::Nominal,
        NSProcessInfoThermalState::Fair => Thermal::Fair,
        NSProcessInfoThermalState::Serious => Thermal::Serious,
        _ => Thermal::Critical,
    }
}

/// `NSProcessInfo.isLowPowerModeEnabled` (macOS 12+), read at each call: a property, cheap.
pub fn low_power() -> bool {
    NSProcessInfo::processInfo().isLowPowerModeEnabled()
}

/// One observer of `NSWorkspaceDidWakeNotification` on the shared workspace's notification centre, kept for the
/// process's life. With no queue, the block runs where AppKit posts the notification: the main thread.
pub(crate) fn watch_wake() {
    use objc2_app_kit::{NSWorkspace, NSWorkspaceDidWakeNotification};
    let block = block2::RcBlock::new(|_: NonNull<NSNotification>| crate::power::woke());
    let center = NSWorkspace::sharedWorkspace().notificationCenter();
    // SAFETY: AppKit's own notification name (a static string), no sender filter and no queue; the block is sendable
    // (it only calls `power::woke`, which any thread may).
    let observer = unsafe {
        center.addObserverForName_object_queue_usingBlock(
            Some(NSWorkspaceDidWakeNotification),
            None,
            None,
            &block,
        )
    };
    // The centre holds the observer until it is removed, which is never.
    std::mem::forget(observer);
}

/// The internal battery from IOPowerSources (`kIOPSInternalBatteryType`): its charge as a percent of its capacity, and
/// whether it's on the charger (`kIOPSPowerSourceStateKey` "AC Power") or charging. None on a Mac without one (a UPS
/// doesn't count).
pub fn battery() -> Option<Battery> {
    objc2::rc::autoreleasepool(|_| {
        // SAFETY: a +1 object, or null (None); `Retained` releases it.
        let info = unsafe { Retained::from_raw(IOPSCopyPowerSourcesInfo()) }?;
        // SAFETY: the blob just read; a +1 array, or null.
        let list = unsafe { Retained::from_raw(IOPSCopyPowerSourcesList(&*info)) }?;
        list.iter().find_map(|source| {
            // SAFETY: the blob and one of its sources; the description lives as long as `info`, which outlives it.
            let described = unsafe { IOPSGetPowerSourceDescription(&*info, &*source) };
            // SAFETY: null, or a dictionary owned by `info`.
            let description = unsafe { described.as_ref() }?;
            internal_battery(description)
        })
    })
}

fn internal_battery(d: &NSDictionary<NSString, AnyObject>) -> Option<Battery> {
    let text =
        |key: &NSString| (d.objectForKey(key)?.downcast_ref::<NSString>()).map(|s| s.to_string());
    let number = |key: &NSString| {
        (d.objectForKey(key)?.downcast_ref::<NSNumber>()).map(|n| n.integerValue())
    };
    if text(ns_string!("Type")).as_deref() != Some("InternalBattery") {
        return None;
    }
    let now = number(ns_string!("Current Capacity"))?;
    let most = number(ns_string!("Max Capacity"))
        .filter(|&m| m > 0)
        .unwrap_or(100);
    let on_power = text(ns_string!("Power Source State")).as_deref() == Some("AC Power");
    let charging = number(ns_string!("Is Charging")).is_some_and(|c| c != 0);
    Some(Battery {
        percent: u8::try_from((now * 100 / most).clamp(0, 100)).unwrap_or(100),
        charging: on_power || charging,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_capture_activity_keeps_the_display_awake() {
        let options = capture_options();
        assert!(options.contains(NSActivityOptions::IdleDisplaySleepDisabled));
        assert!(options.contains(NSActivityOptions::LatencyCritical));
    }
}
