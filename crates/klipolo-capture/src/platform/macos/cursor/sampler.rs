//! The macOS cursor's sampler thread, at utility QoS, each pass in an autorelease pool,
//! on `cursor_pace`'s schedule (60 Hz while the cursor moved in the last second or a down waits to be judged, 10 Hz
//! while still):
//! - **shapes and visibility** (`shapes`): when Core Graphics' cursor seed moves (at most 30 times a second, 3 while
//!   the cursor animates), plus the scheduled safety looks;
//! - **the area**, every 250 ms (`bounds::area_of`), with an `Area` event when its size changed;
//! - **a beat**, 500 ms after anything was last sent: where `mouseLocation` is (flipped to Core Graphics' top-left);
//! - **the self-check** (`check`). A miss switches the take to the poll (`poll`:
//!   `ClicksSampled`, then 240 Hz samples), logged with why. A monitor that couldn't be installed switches at the
//!   start.
//!
//! While paused it sends nothing and only looks at the pause once every 100 ms; its first pass after a resume starts
//! the self-check again (no edge from a press during the pause).

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{Receiver, SyncSender};
use std::thread::JoinHandle;
use std::time::Duration;

use klipolo_platform::clock::host_ns;
use klipolo_platform::qos::{self, Qos};
use objc2::rc::autoreleasepool;

use super::check::SelfCheck;
use super::shapes::Shapes;
use super::{Down, Link, bounds, poll, resident_mb};
use crate::backend::Target;
use crate::cursor::CursorEvent;
use crate::platform::common::cursor_checks::Why;
use crate::platform::common::cursor_map::to_area;
use crate::platform::common::cursor_pace::{Pace, beat_due};

const PAUSED_LOOK: Duration = Duration::from_millis(100);
/// A monitor event this recent means the cursor is moving (or clicked).
const MONITOR_FRESH_NS: u64 = 20_000_000;
/// The resident size is read this often (its largest and the first minute's go in the stop line).
const RSS_EVERY_NS: u64 = 60_000_000_000;

/// See the module notes.
pub(super) struct Sampler {
    out: SyncSender<CursorEvent>,
    target: Target,
    link: Arc<Link>,
    clicks: Arc<AtomicU8>,
    check: SelfCheck,
    pace: Pace,
    poll: Option<JoinHandle<()>>,
    area: (f64, f64),
    /// The rect last written to the area (written only when it changed: a display take never writes again).
    rect: (f64, f64, f64, f64),
    /// The pointer at the last pass (global points, top-left).
    place: Option<(f64, f64)>,
    seed_look_at: u64,
}

impl Sampler {
    pub(super) fn new(
        out: SyncSender<CursorEvent>,
        target: Target,
        (link, downs, clicks): (Arc<Link>, Receiver<Down>, Arc<AtomicU8>),
    ) -> Self {
        let area = link.area.get();
        Self {
            out,
            target,
            check: SelfCheck::new(link.clone(), downs),
            link,
            clicks,
            pace: Pace::default(),
            poll: None,
            area: (area.2, area.3),
            rect: area,
            place: None,
            seed_look_at: 0,
        }
    }

    /// Runs on its own thread; `why`: the take's clicks are sampled from the start.
    pub(super) fn spawn(self, why: Option<Why>) -> std::io::Result<JoinHandle<()>> {
        std::thread::Builder::new()
            .name("klipolo-cursor-sample".into())
            .stack_size(512 * 1024)
            .spawn(move || self.run(why))
    }

    fn run(mut self, why: Option<Why>) {
        qos::set_current(Qos::Utility);
        let mut shapes = Shapes::new();
        let (table, ..) = shapes.counts();
        tracing::info!(table, seed = shapes.seeded(), "cursor: the sampler started");
        if let Some(why) = why {
            self.switch(host_ns(), why);
        }
        let mut was_paused = false;
        let (rss_start, mut rss_minute, mut rss_max) = (resident_mb(), None, 0.0_f64);
        let mut rss_at = host_ns();
        while !self.link.stop.load(Ordering::Acquire) {
            if self.link.paused.load(Ordering::Acquire) {
                was_paused = true;
                std::thread::sleep(PAUSED_LOOK);
                continue;
            }
            let now = host_ns();
            let place = autoreleasepool(|_| self.link.pointer());
            if std::mem::take(&mut was_paused) {
                self.check.resumed(now, place);
            }
            let monitor = self.link.last_monitor.load(Ordering::Acquire);
            if self.place.replace(place) != Some(place)
                || now.saturating_sub(monitor) < MONITOR_FRESH_NS
            {
                self.pace.moved(now);
            }
            let (busy, why) = autoreleasepool(|_| self.check.pass(now, place, monitor));
            if let Some(why) = why {
                self.switch(now, why);
            }
            self.shapes(now, &mut shapes);
            if self.pace.area_due(now) {
                self.area(now);
            }
            if beat_due(self.link.last_sent.load(Ordering::Acquire), now) {
                let ((x, y), host_ns) = (to_area(place, self.link.area.get()), now);
                self.link
                    .send(&self.out, CursorEvent::Beat { host_ns, x, y });
            }
            if now.saturating_sub(rss_at) >= RSS_EVERY_NS {
                let mb = resident_mb();
                (rss_at, rss_max) = (now, rss_max.max(mb));
                rss_minute.get_or_insert(mb);
            }
            let wait = self.pace.pass_ns(now, busy);
            std::thread::sleep(Duration::from_nanos(wait.saturating_sub(host_ns() - now)));
        }
        if let Some(poll) = self.poll.take()
            && poll.join().is_err()
        {
            tracing::error!("cursor: the poll panicked");
        }
        let (_, seed_misses, seed_hits) = shapes.counts();
        tracing::info!(
            looks = shapes.looks,
            seed = shapes.seeded(),
            seed_misses,
            seed_hits,
            animations = shapes.animation.started(),
            missed_downs = self.check.misses(),
            sampled = self.link.sampled.load(Ordering::Acquire),
            dropped = self.link.dropped.load(Ordering::Relaxed),
            "cursor: the sampler stopped"
        );
        let at =
            |a: &[std::sync::atomic::AtomicU64; 2]| a.each_ref().map(|v| v.load(Ordering::Acquire));
        let ([first_event_ns, first_ns], [last_event_ns, last_ns]) =
            (at(&self.link.first_down), at(&self.link.last_down));
        tracing::info!(
            first_event_ns,
            first_ns,
            last_event_ns,
            last_ns,
            rss_start_mb = rss_start,
            rss_minute_mb = rss_minute.unwrap_or(rss_start),
            rss_max_mb = rss_max.max(rss_start),
            rss_end_mb = resident_mb(),
            event_clock_least_lag_ns = self.link.least_lag.load(Ordering::Relaxed),
            "cursor: the take's first and last downs (event, host ns), the worker's resident size, the event clock's lag"
        );
    }

    /// A look when the seed moved (at most every `Animation::seed_look_ns`: 30 Hz, 3 Hz while the cursor animates), or
    /// when one is scheduled. A scheduled look reads the seed too: one that moved within the limit isn't a safety
    /// look's "key changed, same seed".
    fn shapes(&mut self, now: u64, shapes: &mut Shapes) {
        let seeded = shapes.seeded();
        let (out, link) = (&self.out, &self.link);
        let mut send = |ev| link.send(out, ev);
        let gap = shapes.animation.seed_look_ns();
        if seeded && now.saturating_sub(self.seed_look_at) >= gap && shapes.seed_moved() {
            self.seed_look_at = now;
            self.pace.looked(now);
            shapes.look(now, true, &mut send);
        } else if self.pace.look_due(now, seeded) {
            let moved = shapes.seed_moved();
            shapes.look(now, moved, &mut send);
        }
    }

    /// The area again (a window moved or resized; a display rearranged), and the main display's height.
    fn area(&mut self, now: u64) {
        self.link.set_main_h(bounds::main_height());
        let Some(rect) = bounds::area_of(&self.target) else {
            return;
        };
        if rect != self.rect {
            self.rect = rect;
            self.link.area.set(rect);
        }
        if (rect.2, rect.3) != self.area {
            self.area = (rect.2, rect.3);
            let (w_pts, h_pts, host_ns) = (rect.2 as f32, rect.3 as f32, now);
            let ev = CursorEvent::Area {
                host_ns,
                w_pts,
                h_pts,
            };
            self.link.send(&self.out, ev);
        }
    }

    /// From `now` the take's clicks are sampled: the monitor sends nothing more, the poll sends the positions and
    /// buttons, and the file gets its `c` line.
    fn switch(&mut self, now: u64, why: Why) {
        if self.link.sampled.swap(true, Ordering::AcqRel) {
            return;
        }
        self.clicks.store(1, Ordering::Release);
        tracing::warn!(
            ?why,
            clicks = "sampled",
            "cursor: the take's clicks are sampled from here"
        );
        let ev = CursorEvent::ClicksSampled { host_ns: now };
        self.link.send(&self.out, ev);
        match poll::spawn(self.out.clone(), self.link.clone()) {
            Ok(poll) => self.poll = Some(poll),
            Err(e) => {
                tracing::error!(error = %e, "cursor: the poll didn't start");
                let _ = self.out.try_send(CursorEvent::Failed);
            }
        }
    }
}
