//! Power for the take, as `ShellInput::Power`:
//! - NSWorkspace's will-sleep (WillSleep) and did-wake (`Woke`, with the sleep's wall-clock length from `SleepClock`:
//!   the host clock stops while a Mac sleeps), session resign and become active (Locked, Unlocked: fast user
//!   switching) and will-power-off (PowerOff);
//! - the distributed `com.apple.screenIsLocked` and `com.apple.screenIsUnlocked` (Locked, Unlocked);
//! - `NSProcessInfoThermalStateDidChangeNotification` (`Thermal`, the state now).
//!
//! The take's own activity (`begin_capture`: display and system kept awake) is the shell view's `TakeActivity`, which
//! the shell gives every status.

use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;

use objc2_app_kit::{
    NSWorkspace, NSWorkspaceDidWakeNotification, NSWorkspaceSessionDidBecomeActiveNotification,
    NSWorkspaceSessionDidResignActiveNotification, NSWorkspaceWillPowerOffNotification,
    NSWorkspaceWillSleepNotification,
};
use objc2_foundation::{NSNotificationCenter, NSProcessInfoThermalStateDidChangeNotification};

use super::observe::{Distributed, Observers};
use crate::platform::common::sleep::SleepClock;
use crate::shell::{PowerEvent, ShellInput};

/// Adds the power observers to `observers`; the returned screen-lock observers live as long as the caller keeps them.
pub(super) fn watch(input: &SyncSender<ShellInput>, observers: &mut Observers) -> Distributed {
    let workspace = NSWorkspace::sharedWorkspace().notificationCenter();
    let sleep = Arc::new(Mutex::new(SleepClock::default()));
    let sender = |input: &SyncSender<ShellInput>| {
        let input = input.clone();
        move |event: PowerEvent| {
            let _ = input.send(ShellInput::Power(event));
        }
    };
    // SAFETY (every `unsafe` below): reading AppKit's and Foundation's notification-name constants.
    let (send, clock) = (sender(input), sleep.clone());
    observers.add(
        &workspace,
        unsafe { NSWorkspaceWillSleepNotification },
        move |_| {
            let event = lock(&clock).will_sleep(SystemTime::now());
            send(event);
        },
    );
    let (send, clock) = (sender(input), sleep);
    observers.add(
        &workspace,
        unsafe { NSWorkspaceDidWakeNotification },
        move |_| {
            let event = lock(&clock).woke(SystemTime::now());
            send(event);
        },
    );
    for (name, event) in [
        (
            unsafe { NSWorkspaceSessionDidResignActiveNotification },
            PowerEvent::Locked,
        ),
        (
            unsafe { NSWorkspaceSessionDidBecomeActiveNotification },
            PowerEvent::Unlocked,
        ),
        (
            unsafe { NSWorkspaceWillPowerOffNotification },
            PowerEvent::PowerOff,
        ),
    ] {
        let send = sender(input);
        observers.add(&workspace, name, move |_| send(event));
    }
    let send = sender(input);
    observers.add(
        &NSNotificationCenter::defaultCenter(),
        unsafe { NSProcessInfoThermalStateDidChangeNotification },
        move |_| send(PowerEvent::Thermal(klipolo_platform::power::thermal())),
    );
    Distributed::new(
        input,
        &[
            (
                "com.apple.screenIsLocked",
                ShellInput::Power(PowerEvent::Locked),
            ),
            (
                "com.apple.screenIsUnlocked",
                ShellInput::Power(PowerEvent::Unlocked),
            ),
        ],
    )
}

fn lock(clock: &Mutex<SleepClock>) -> std::sync::MutexGuard<'_, SleepClock> {
    clock.lock().unwrap_or_else(PoisonError::into_inner)
}
