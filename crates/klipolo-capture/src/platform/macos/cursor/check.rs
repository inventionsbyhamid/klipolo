//! The sampler's part of the monitor's self-check (`Gating` holds the rules): each pass it
//! takes the monitor's downs, `pressedMouseButtons`' down edges (left, right, middle) and the pointer's place, and asks
//! about a miss only over windows that aren't Klipolo's (`bounds::own_window_at`). The monitor's handler runs on the main
//! thread, which a busy Mac can stall: a tiny block sent to the main queue each pass says when it last ran, so a stall
//! is never judged a monitor that misses. Nothing runs once the take's clicks are sampled.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Receiver;

use cidre::dispatch;
use klipolo_platform::clock::host_ns;
use objc2_app_kit::NSEvent;

use super::{Down, Link, bounds};
use crate::platform::common::cursor_checks::{Gating, Why, ping_due};

/// The buttons the self-check follows (`pressedMouseButtons`' bits, the file's `btn`).
const CHECKED: u8 = 3;

/// See the module notes.
pub(super) struct SelfCheck {
    link: Arc<Link>,
    downs: Receiver<Down>,
    gating: Gating,
    /// The buttons at the last pass (None: the next pass only sets them: the first one, the first after a pause).
    buttons: Option<usize>,
    last_pass: u64,
    /// When the last main-queue block was sent (it stores when it ran in `Link::main_alive`).
    ping_at: u64,
}

impl SelfCheck {
    pub(super) fn new(link: Arc<Link>, downs: Receiver<Down>) -> Self {
        Self {
            link,
            downs,
            gating: Gating::new(),
            buttons: None,
            last_pass: 0,
            ping_at: 0,
        }
    }

    pub(super) fn misses(&self) -> u32 {
        self.gating.misses()
    }

    /// The take resumed at `now`: no edge from a press during the pause, nothing judged from before it.
    pub(super) fn resumed(&mut self, now: u64, place: (f64, f64)) {
        self.buttons = None;
        self.gating.reset(now, place);
    }

    /// One pass at `now`, the pointer at `place`, the monitor's last event at `monitor`: (whether the next pass should
    /// come sooner: a button down or a down to judge; a reason to switch to sampled clicks).
    pub(super) fn pass(
        &mut self,
        now: u64,
        place: (f64, f64),
        monitor: u64,
    ) -> (bool, Option<Why>) {
        if self.link.sampled.load(Ordering::Acquire) {
            return (false, None);
        }
        // The main queue's time first, then the downs: every down its handler sent before that block ran is seen
        //.
        let main = self.ping(now);
        for (at, btn) in self.downs.try_iter() {
            self.gating.monitor_down(at, btn);
        }
        let pressed = NSEvent::pressedMouseButtons();
        if let Some(before) = self.buttons {
            for btn in (0..CHECKED).filter(|b| (!before & pressed) >> b & 1 == 1) {
                self.gating.down_edge(self.last_pass, now, btn, place);
            }
        }
        (self.buttons, self.last_pass) = (Some(pressed), now);
        let why = (self.gating).check(now, place, (monitor, main), &mut bounds::own_window_at);
        (pressed & 0b111 != 0 || self.gating.waiting(), why)
    }

    /// When the main queue last ran; a block is sent to it again once the last one ran (`ping_due`).
    fn ping(&mut self, now: u64) -> u64 {
        let main = self.link.main_alive.load(Ordering::Acquire);
        if ping_due(self.ping_at, main) {
            self.ping_at = now;
            let link = self.link.clone();
            dispatch::Queue::main()
                .async_mut(move || link.main_alive.store(host_ns(), Ordering::Release));
        }
        main
    }
}
