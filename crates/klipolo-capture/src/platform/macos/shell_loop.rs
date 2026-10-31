//! The macOS shell's AppKit loop plumbing (`shell`): clicks and shortcuts in as the session's inputs, the session's
//! outputs onto the main queue in order, and the loop's stop.

use std::sync::mpsc::{Receiver, SyncSender};

use cidre::dispatch;
use global_hotkey::{GlobalHotKeyEvent, HotKeyState};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSEvent, NSEventModifierFlags, NSEventType};
use objc2_foundation::NSPoint;
use tray_icon::TrayIconEvent;
use tray_icon::menu::MenuEvent;

use super::shell::with_face;
use crate::shell::{MenuAction, ShellInput, ShellOutput};

/// Clicks on the menu's actions and pressed shortcuts become the session's inputs; the item's own mouse events aren't
/// needed and are dropped rather than queued.
pub(super) fn handle_clicks_and_keys(input: &SyncSender<ShellInput>) {
    let to = input.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        if let Some(action) = MenuAction::from_id(&event.id.0) {
            let _ = to.send(ShellInput::Menu(action));
        }
    }));
    TrayIconEvent::set_event_handler(Some(|_| {}));
    let to = input.clone();
    GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
        if event.state != HotKeyState::Pressed {
            return;
        }
        if let Some(action) = with_face(|face| face.pressed(event.id)).flatten() {
            let _ = to.send(ShellInput::Hotkey(action));
        }
    }));
}

/// The session's outputs, each posted to the main queue in order; the channel's end stops the shell.
pub(super) fn forward(output: Receiver<ShellOutput>) {
    let spawned = std::thread::Builder::new()
        .name("klipolo-shell-out".into())
        .spawn(move || {
            for out in output {
                let quit = out == ShellOutput::Quit;
                let mut out = Some(out);
                dispatch::Queue::main().async_mut(move || {
                    if let Some(out) = out.take() {
                        with_face(|face| face.show(out));
                    }
                });
                if quit {
                    return;
                }
            }
            dispatch::Queue::main().async_mut(|| {
                if let Some(mtm) = MainThreadMarker::new() {
                    stop(mtm);
                }
            });
        });
    if let Err(error) = spawned {
        tracing::error!(%error, "shell: the session's outputs can't reach the menu-bar item");
    }
}

/// Ends AppKit's loop: `stop:` takes effect after the event in hand, so one of our own is posted to make sure there is
/// one.
pub(super) fn stop(mtm: MainThreadMarker) {
    let app = NSApplication::sharedApplication(mtm);
    app.stop(None);
    let wake = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
        NSEventType::ApplicationDefined,
        NSPoint::new(0.0, 0.0),
        NSEventModifierFlags::empty(),
        0.0,
        0,
        None,
        0,
        0,
        0,
    );
    if let Some(wake) = wake {
        app.postEvent_atStart(&wake, false);
    }
}
