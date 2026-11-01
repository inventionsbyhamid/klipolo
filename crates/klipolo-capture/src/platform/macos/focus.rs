//! The frontmost app and apps quitting and launching, from NSWorkspace's notices, as `ShellInput::Focus`, `AppQuit` and
//! `AppLaunched` with the app's id (`AppInfo::app_id`: its bundle id, else its executable's path). The session pauses a
//! game's take while the game isn't in front, and waits for a game that quit (detection only
//! suggests; these never start anything).

use std::sync::mpsc::SyncSender;

use objc2::rc::Retained;
use objc2_app_kit::{
    NSRunningApplication, NSWorkspace, NSWorkspaceApplicationKey,
    NSWorkspaceDidActivateApplicationNotification, NSWorkspaceDidLaunchApplicationNotification,
    NSWorkspaceDidTerminateApplicationNotification,
};
use objc2_foundation::NSNotification;

use super::apps::app_id;
use super::observe::Observers;
use crate::shell::ShellInput;

/// Adds the focus observers to `observers`.
pub(super) fn watch(input: &SyncSender<ShellInput>, observers: &mut Observers) {
    let workspace = NSWorkspace::sharedWorkspace().notificationCenter();
    let to = input.clone();
    // SAFETY (every `unsafe` below): reading AppKit's notification-name constants.
    observers.add(
        &workspace,
        unsafe { NSWorkspaceDidActivateApplicationNotification },
        move |note| {
            let _ = to.send(ShellInput::Focus {
                app_id: app_of(note),
            });
        },
    );
    let to = input.clone();
    observers.add(
        &workspace,
        unsafe { NSWorkspaceDidTerminateApplicationNotification },
        move |note| {
            if let Some(app_id) = app_of(note) {
                let _ = to.send(ShellInput::AppQuit { app_id });
            }
        },
    );
    let to = input.clone();
    observers.add(
        &workspace,
        unsafe { NSWorkspaceDidLaunchApplicationNotification },
        move |note| {
            if let Some(app_id) = app_of(note) {
                let _ = to.send(ShellInput::AppLaunched { app_id });
            }
        },
    );
}

/// The notice's app (`NSWorkspaceApplicationKey`), by its id.
fn app_of(note: &NSNotification) -> Option<String> {
    let info = note.userInfo()?;
    // SAFETY: reading AppKit's constant.
    let key = unsafe { NSWorkspaceApplicationKey };
    let app: Retained<NSRunningApplication> = info.objectForKey(key)?.downcast().ok()?;
    app_id(&app)
}
