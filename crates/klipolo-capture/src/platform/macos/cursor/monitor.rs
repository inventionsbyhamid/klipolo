//! The global mouse monitor: `NSEvent.addGlobalMonitorForEventsMatchingMask:handler:` with
//! the mouse-only mask (`mouse_mask`: buttons, drags and moves, never a key or a scroll), installed and removed on the
//! main queue, which the worker shell's AppKit loop runs and where AppKit calls the handler.
//!
//! **The handler** never blocks and allocates nothing: it returns at once while the take is paused, the source stopped
//! or its clicks sampled (the poll's then); else it reads the event's type, time, button and place
//! (`CGEventGetLocation`: global points, top-left), maps the place into the captured area as it is now, stamps the
//! time on the host clock (`ClockOffset`: as the event says, unless the event clock turns out more than 1 s off; its
//! smallest lag goes in the take's stop line), and
//! `try_send`s one `CursorEvent`. A down also goes to the sampler's self-check.
//!
//! Apple gives a global monitor no event aimed at Klipolo's own windows (the HUD, the menu-bar item): those clicks
//! aren't in the captured picture either, and the self-check doesn't count them as misses.

use std::cell::RefCell;
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{SyncSender, sync_channel};
use std::time::Duration;

use block2::RcBlock;
use cidre::dispatch;
use klipolo_platform::clock::host_ns;
use objc2::MainThreadMarker;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSEvent, NSEventMask, NSEventType};
use objc2_core_graphics::CGEvent;

use super::{Down, Link};
use crate::cursor::CursorEvent;
use crate::pipe::PipeError;
use crate::platform::common::cursor_checks::ClockOffset;
use crate::platform::common::cursor_map::{ns_seconds_to_host_ns, to_area};
use crate::platform::common::mouse_mask::mouse_mask;

/// How long `install` waits for the main queue to run it.
const INSTALL_WAIT: Duration = Duration::from_secs(2);

/// The installed monitor: removed when dropped, on the main queue.
pub(super) struct Token(Option<Held>);

/// The monitor object AppKit gave.
struct Held(Retained<AnyObject>);

// SAFETY: an opaque token, only ever passed back to `removeMonitor:` (on the main thread) and released there.
unsafe impl Send for Held {}

impl Drop for Token {
    fn drop(&mut self) {
        let mut held = self.0.take();
        let mut remove = move || {
            if let Some(Held(monitor)) = held.take() {
                // SAFETY: the object `addGlobalMonitorForEventsMatchingMask:handler:` returned, removed once.
                unsafe { NSEvent::removeMonitor(&monitor) };
            }
        };
        if MainThreadMarker::new().is_some() {
            remove();
        } else {
            dispatch::Queue::main().async_mut(remove);
        }
    }
}

/// Installs the monitor on the main queue, waiting up to 2 s for it to run (Err: the main queue isn't running, or
/// AppKit gave no monitor). One the queue installs after the wait is removed at once.
pub(super) fn install(
    out: SyncSender<CursorEvent>,
    link: Arc<Link>,
    downs: SyncSender<Down>,
) -> Result<Token, PipeError> {
    let (tx, rx) = sync_channel(1);
    let mut job = Some(move || {
        // A late one: the wait is over, so the answer (and its token) is dropped here, on the main thread.
        let _ = tx.try_send(add(out, link, downs));
    });
    let mut run = move || {
        if let Some(job) = job.take() {
            job();
        }
    };
    if MainThreadMarker::new().is_some() {
        run();
    } else {
        dispatch::Queue::main().async_mut(run);
    }
    rx.recv_timeout(INSTALL_WAIT)
        .map_err(|_| PipeError::Os("the main queue didn't install the mouse monitor".into()))?
}

/// On the main thread: the handler and its monitor.
fn add(
    out: SyncSender<CursorEvent>,
    link: Arc<Link>,
    downs: SyncSender<Down>,
) -> Result<Token, PipeError> {
    // Only the main thread touches it (AppKit calls the handler there).
    let clock = RefCell::new(ClockOffset::default());
    let handler = RcBlock::new(move |event: NonNull<NSEvent>| {
        let off = |flag: &std::sync::atomic::AtomicBool| flag.load(Ordering::Acquire);
        if off(&link.stop) || off(&link.paused) || off(&link.sampled) {
            return;
        }
        let now = host_ns();
        // SAFETY: AppKit passes a live event for the handler's call.
        let event = unsafe { event.as_ref() };
        autoreleasepool(|_| handle(event, now, &link, &out, &downs, &clock));
    });
    let mask = NSEventMask(mouse_mask());
    NSEvent::addGlobalMonitorForEventsMatchingMask_handler(mask, &handler)
        .map(|monitor| Token(Some(Held(monitor))))
        .ok_or_else(|| PipeError::Os("AppKit gave no mouse monitor".into()))
}

/// One event, arrived at `now`.
fn handle(
    event: &NSEvent,
    now: u64,
    link: &Link,
    out: &SyncSender<CursorEvent>,
    downs: &SyncSender<Down>,
    clock: &RefCell<ClockOffset>,
) {
    let other = || u8::try_from(event.buttonNumber()).unwrap_or(u8::MAX);
    let button = match event.r#type() {
        NSEventType::LeftMouseDown => Some((0, true)),
        NSEventType::LeftMouseUp => Some((0, false)),
        NSEventType::RightMouseDown => Some((1, true)),
        NSEventType::RightMouseUp => Some((1, false)),
        NSEventType::OtherMouseDown => Some((other(), true)),
        NSEventType::OtherMouseUp => Some((other(), false)),
        NSEventType::MouseMoved
        | NSEventType::LeftMouseDragged
        | NSEventType::RightMouseDragged
        | NSEventType::OtherMouseDragged => None,
        _ => return,
    };
    link.last_monitor.store(now, Ordering::Release);
    let host_ns = {
        let mut clock = clock.borrow_mut();
        let at = clock.stamp(ns_seconds_to_host_ns(event.timestamp()), now);
        let least = clock.least_lag().unwrap_or(0);
        link.least_lag.store(least, Ordering::Relaxed);
        if let Some(offset_ns) = clock.take_decided() {
            let corrected = offset_ns != 0;
            tracing::info!(
                offset_ns,
                corrected,
                "cursor: the event clock against the host clock"
            );
        }
        at
    };
    let place = match event.CGEvent() {
        Some(cg) => {
            let p = CGEvent::location(Some(&*cg));
            (p.x, p.y)
        }
        None => link.pointer(),
    };
    let (x, y) = to_area(place, link.area.get());
    let ev = match button {
        Some((btn, down)) => {
            if down {
                let _ = downs.try_send((host_ns, btn));
                link.down_seen(ns_seconds_to_host_ns(event.timestamp()), host_ns);
            }
            CursorEvent::Button {
                host_ns,
                btn,
                down,
                x,
                y,
            }
        }
        None => CursorEvent::Move { host_ns, x, y },
    };
    link.send(out, ev);
}
