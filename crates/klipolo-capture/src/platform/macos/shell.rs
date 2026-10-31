//! The worker shell on macOS (`MacShell`): the worker's main thread runs AppKit as an accessory
//! app (no Dock icon, no menu bar of its own) with the recorder's menu-bar item, the HUD, the global shortcuts while
//! armed, and the power, focus and parent watches.
//! - **Out:** a forwarding thread takes the session's outputs and posts each to the main queue, in order, where the
//!   item, the HUD, the shortcuts (`KeysController`) and the take's activity (`TakeActivity`) follow them.
//! - **In:** clicks and shortcuts come through `muda`'s and `global-hotkey`'s handlers, which AppKit calls on the main
//!   thread, as `ShellInput`s; a click is paired with its action by the item's id.
//! - **The item** is made once AppKit's loop runs (`delegate`), and a take goes on without it if it can't be.
//! - **The end:** `Quit` (or the session's end of the channel) stops AppKit's loop, so `run` returns and the worker
//!   exits with the session's own code. AppKit's `terminate:` would exit the process at once, before the session's
//!   last events and its exit code (3 for a take that never started); a system quit goes through the delegate, which
//!   stops and saves the take first and ends with the session's code.
//!
//! Nothing here captures or asks: the item, the HUD and the shortcuts touch no device.

use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::Arc;
use std::time::Instant;

use cidre::dispatch;
use klipolo_platform::caps::OsFamily;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

use super::bubble::{Bubble, BubbleFeed};
use super::delegate::Delegate;
use super::hotkeys::MacHotkeys;
use super::hud::Hud;
use super::item::Item;
use super::observe::Observers;
use super::shell_loop::{forward, handle_clicks_and_keys, stop};
use super::{focus, parent, power};
use crate::backend::IndicatorPlace;
use crate::shell::{HotkeyAction, Shell, ShellIo, ShellOutput, StatusView};
use crate::shell_view::{
    KeysController, TakeActivity, Words, gate_answer, menu_items, ready_line, shortcut_text,
    status_title,
};

thread_local! {
    /// The shell's main-thread state while `MacShell::run` runs (AppKit's objects live on the main thread only).
    static FACE: RefCell<Option<Face>> = const { RefCell::new(None) };
}

pub(super) struct MacShell {
    /// The camera bubble's live layer, which the take's camera pipe feeds.
    pub(super) bubble: Arc<BubbleFeed>,
}

impl Shell for MacShell {
    fn run(self: Box<Self>, io: ShellIo) {
        let Some(mtm) = MainThreadMarker::new() else {
            tracing::error!(
                "shell: not on the main thread; the take runs without its menu-bar item"
            );
            return Box::new(crate::platform::NoShell).run(io);
        };
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        // Before the item exists, so no event of its own is ever queued without bound.
        handle_clicks_and_keys(&io.input);
        let ShellIo {
            input,
            output,
            keys,
            words,
            hud_corner,
            hud_display,
            end,
            camera_bubble,
        } = io;
        // Made now, before any capture filter is built, so a display's filter knows its window (`bubble`).
        let bubble = camera_bubble.then(|| Bubble::new(mtm, hud_display.clone()));
        // AppKit keeps its delegate weakly: the face holds it for the shell's life.
        let delegate = Delegate::new(mtm, input.clone(), end);
        app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        let words = Words::from(words);
        let mut hud = Hud::new(mtm, hud_corner, hud_display);
        // The countdown's question, answered from the game: the start key, or the item's Cancel.
        let start = shortcut_text(&keys.start_stop, OsFamily::Macos);
        hud.answer(gate_answer(&words, &start, IndicatorPlace::MenuBar));
        let face = Face {
            mtm,
            item: None,
            delegate,
            hud,
            bubble,
            feed: self.bubble,
            keys: KeysController::new(MacHotkeys::new(), keys),
            activity: TakeActivity::default(),
            words,
            view: None,
            hud_timer: None,
        };
        FACE.with(|f| *f.borrow_mut() = Some(face));
        let mut observers = Observers::default();
        let screen_lock = power::watch(&input, &mut observers);
        focus::watch(&input, &mut observers);
        parent::watch(input);
        forward(output);
        app.run();
        drop((observers, screen_lock));
        if let Some(face) = FACE.with(|f| f.borrow_mut().take()) {
            face.close();
        }
    }
}

/// The main thread's state: what the person sees, and the shortcuts and activity the take holds.
pub(super) struct Face {
    mtm: MainThreadMarker,
    /// Made once AppKit's loop runs (`launched`); None until then, or if it couldn't be.
    item: Option<Item>,
    delegate: Retained<Delegate>,
    hud: Hud,
    /// The take's camera bubble, when it shows one (`ShellIo::camera_bubble`), and the layer its camera feeds.
    bubble: Option<Bubble>,
    feed: Arc<BubbleFeed>,
    keys: KeysController<MacHotkeys>,
    activity: TakeActivity,
    words: Words,
    view: Option<StatusView>,
    /// When the HUD's next timer is due.
    hud_timer: Option<Instant>,
}

impl Face {
    /// AppKit's loop runs: the menu-bar item now, as tray-icon requires for fullscreen apps. Without it the take goes
    /// on: the app's Stop, the shortcuts, the HUD and the watches still work.
    pub(super) fn launched(&mut self) {
        match Item::new(self.mtm, self.keys.keys()) {
            Ok(item) => {
                self.item = Some(item);
                self.refresh_menu();
            }
            Err(error) => {
                tracing::error!(%error, "shell: no menu-bar item; the app's Stop still works")
            }
        }
    }

    pub(super) fn show(&mut self, out: ShellOutput) {
        match out {
            ShellOutput::Keys(armed) => {
                self.keys.apply(armed);
                self.refresh_menu();
            }
            ShellOutput::Status(view) => {
                self.delegate.seen(view.state);
                self.activity.apply(view.state);
                self.hud.status(view.clone());
                if let Some(bubble) = &mut self.bubble {
                    bubble.apply(view.bubble, &self.feed);
                }
                self.view = Some(view);
                self.refresh_menu();
                self.redraw_hud();
            }
            ShellOutput::Flash(kind) => {
                self.hud.flash(kind);
                self.redraw_hud();
            }
            // The system's quit is ending the process with the session's code (`delegate`), not AppKit's loop.
            ShellOutput::Quit if self.delegate.quitting() => {
                self.keys.apply(false);
                self.hud.hide();
                self.hide_bubble();
            }
            ShellOutput::Quit => stop(self.mtm),
        }
    }

    /// A shortcut was pressed: its action, and the HUD shows the countdown when it starts the take.
    pub(super) fn pressed(&mut self, id: u32) -> Option<HotkeyAction> {
        let action = self.keys.registry().action(id)?;
        if action == HotkeyAction::StartStop {
            self.hud.start_key();
        }
        Some(action)
    }

    fn hide_bubble(&self) {
        if let Some(bubble) = &self.bubble {
            bubble.hide();
        }
    }

    /// The title and the menu: the armed "Ready" line (only while the start key is ours), a taken shortcut, the
    /// session's lines, then the actions.
    fn refresh_menu(&mut self) {
        let Some(view) = &self.view else { return };
        let start = &self.keys.keys().start_stop;
        let start =
            (self.keys.registry().holds(start)).then(|| shortcut_text(start, OsFamily::Macos));
        let mut lines: Vec<String> = ready_line(view.state, &self.words, start.as_deref())
            .into_iter()
            .collect();
        lines.extend(self.keys.problem().map(|p| self.words.word(p)));
        let items = menu_items(view, &self.words, &lines);
        if let Some(item) = &mut self.item {
            item.show(&status_title(view), &items);
        }
    }

    /// Draws the HUD's change, and sets a timer for its next one (a flash's end, a change held back by the rate).
    fn redraw_hud(&mut self) {
        let Some(due) = self.hud.redraw(&self.words) else {
            return;
        };
        let now = Instant::now();
        if self.hud_timer.is_some_and(|at| at > now && at <= due) {
            return;
        }
        self.hud_timer = Some(due);
        let delay = dispatch::Time::with_delta(due.saturating_duration_since(now));
        dispatch::Queue::main().after_f(delay, std::ptr::null_mut::<c_void>(), hud_due);
    }

    /// The shell's end: no shortcut outlives it and the HUD and the bubble go; the item and the take's activity end as
    /// they drop.
    fn close(mut self) {
        self.keys.apply(false);
        self.hud.hide();
        self.hide_bubble();
    }
}

/// The HUD's timer.
extern "C-unwind" fn hud_due(_: *mut c_void) {
    with_face(|face| {
        face.hud_timer = None;
        face.redraw_hud();
    });
}

/// The main thread's state, if the shell runs on this thread (and isn't already in use).
pub(super) fn with_face<T>(f: impl FnOnce(&mut Face) -> T) -> Option<T> {
    FACE.with(|face| match face.try_borrow_mut() {
        Ok(mut face) => face.as_mut().map(f),
        Err(_) => {
            tracing::warn!("shell: an event came while the shell was busy; dropped");
            None
        }
    })
}
