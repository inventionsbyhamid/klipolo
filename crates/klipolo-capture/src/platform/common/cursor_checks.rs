//! The macOS cursor source's runtime self-checks (the open findings are
//! handled both ways, the take checking for itself). Pure, built on every OS, so every OS's tests run them.
//! - `Gating`: does the global monitor deliver? A `pressedMouseButtons` down edge with no
//!   monitor down near it, twice within a minute, or a pointer that moved more than 2 points while the monitor sent
//!   nothing for 1 s, switches the take to sampled clicks. It watches the whole take (a monitor can stop mid-take), and
//!   misses decay: two unrelated ones far apart never switch. A miss over one of Klipolo's own windows (the HUD, the menu-bar item: Apple
//!   gives a global monitor no event for its own app) never counts, and a pause or a resume drops what isn't judged.
//!   The monitor's handler runs on the main thread, which a busy Mac can stall: nothing is judged until the main queue
//!   has run since (a stalled one is never a monitor that misses), and an edge it never got to is dropped unjudged.
//! - `ClockOffset`: the event clock against the host clock, from the events' lags (host ns at delivery minus
//!   the event's time): once 20 came, the smallest lag yet, since a stall only adds lag. Past 1 s, every event's time is
//!   corrected by it; a time is never after its arrival.
//!
//! The shape looks' own decisions (no current cursor, the cursor seed, the shape limit, animations) are
//! `cursor_looks`'.

/// A sampled edge is matched by a monitor event of the same button and direction this close to it (the edge happened
/// between its sample and the one before, so the window starts that much earlier).
pub const MATCH_NS: u64 = 100_000_000;
/// An edge is judged this long after its sample: the monitor's event may still be on its way (its delivery lag).
pub const JUDGE_AFTER_NS: u64 = MATCH_NS + 50_000_000;
/// A second missed down this soon after the last one switches the take to sampled clicks.
pub const MISS_WINDOW_NS: u64 = 60_000_000_000;
/// An edge is judged only once the main queue ran this long after its sample (the monitor could have answered by then).
pub const MAIN_AFTER_NS: u64 = 50_000_000;
/// An edge the main queue didn't get to within this is dropped unjudged.
pub const GIVE_UP_NS: u64 = 2_000_000_000;
/// The pointer moved more than this (points) with no monitor event for `QUIET_NS`: the monitor delivers no moves.
pub const MOVED_PTS: f64 = 2.0;
pub const QUIET_NS: u64 = 1_000_000_000;
/// Events before the clock offset is decided.
pub const OFFSET_EVENTS: usize = 20;
/// An offset beyond this is corrected.
pub const OFFSET_LIMIT_NS: i64 = 1_000_000_000;
/// Before the offset is decided, an event lagging more than this (time asleep: another clock base) takes its arrival.
pub const SNAP_LIMIT_NS: i64 = 10_000_000_000;
/// The monitor's button events and the sampled edges kept for matching.
const KEEP: usize = 16;

/// Why a take's clicks are sampled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// The monitor couldn't be installed.
    NoMonitor,
    /// Two down edges within `MISS_WINDOW_NS` had no monitor down near them.
    MissedButtons,
    /// The pointer moved while the monitor said nothing for a second.
    NoMoves,
}

/// A sampled edge: the pass before it, its own pass, its button, and where the pointer was (global points).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Edge {
    from_ns: u64,
    at_ns: u64,
    btn: u8,
    place: (f64, f64),
}

/// See the module notes. O(KEEP) per pass, no allocation once made.
#[derive(Clone, Debug, Default)]
pub struct Gating {
    /// The monitor's downs (their own times, by button), newest last.
    downs: Vec<(u64, u8)>,
    pending: Vec<Edge>,
    /// Missed downs in the take (for its log), and the last one's sample.
    misses: u32,
    last_miss: Option<u64>,
    switch: bool,
    /// The monitor's last event seen, and where the pointer was then (the move check's start).
    quiet_from: Option<(u64, (f64, f64))>,
}

impl Gating {
    pub fn new() -> Self {
        Self {
            downs: Vec::with_capacity(KEEP),
            pending: Vec::with_capacity(KEEP),
            ..Self::default()
        }
    }

    /// A down the monitor delivered, at its own time.
    pub fn monitor_down(&mut self, at_ns: u64, btn: u8) {
        if self.downs.len() == KEEP {
            self.downs.remove(0);
        }
        self.downs.push((at_ns, btn));
    }

    /// `btn` went down between the pass at `from_ns` and this one at `at_ns`, the pointer at `place`. Ups aren't judged:
    /// one click over a window that takes its own events (a menu's) is one miss, not two.
    pub fn down_edge(&mut self, from_ns: u64, at_ns: u64, btn: u8, place: (f64, f64)) {
        if self.pending.len() == KEEP {
            self.pending.remove(0);
        }
        self.pending.push(Edge {
            from_ns,
            at_ns,
            btn,
            place,
        });
    }

    /// A pause or its end at `now_ns`: what isn't judged is dropped (the click that paused, a button changed while
    /// paused), and the move check starts again.
    pub fn reset(&mut self, now_ns: u64, place: (f64, f64)) {
        self.pending.clear();
        self.quiet_from = Some((now_ns, place));
    }

    /// The missed downs so far.
    pub fn misses(&self) -> u32 {
        self.misses
    }

    /// Whether a down waits to be judged (the sampler looks sooner).
    pub fn waiting(&self) -> bool {
        !self.pending.is_empty()
    }

    /// One pass at `now_ns`, the pointer at `place`, the monitor's last event at `monitor_ns` (0: none yet), the main
    /// queue's last run at `main_ns`. `own(p)` says whether one of Klipolo's own windows is at `p`; it is asked only about
    /// a miss. Some: switch, and why.
    pub fn check(
        &mut self,
        now_ns: u64,
        place: (f64, f64),
        (monitor_ns, main_ns): (u64, u64),
        own: &mut impl FnMut((f64, f64)) -> bool,
    ) -> Option<Why> {
        let downs = &self.downs;
        let (mut missed, mut last_miss, mut switch) = (0, self.last_miss, self.switch);
        self.pending.retain(|e| {
            if now_ns < e.at_ns.saturating_add(JUDGE_AFTER_NS) {
                return true;
            }
            if main_ns < e.at_ns.saturating_add(MAIN_AFTER_NS) {
                return now_ns < e.at_ns.saturating_add(GIVE_UP_NS);
            }
            let (lo, hi) = (e.from_ns.saturating_sub(MATCH_NS), e.at_ns + MATCH_NS);
            let seen = (downs.iter()).any(|&(t, b)| b == e.btn && (lo..=hi).contains(&t));
            if !seen && !own(e.place) {
                missed += 1;
                switch |= last_miss.is_some_and(|m| e.at_ns.saturating_sub(m) <= MISS_WINDOW_NS);
                last_miss = Some(e.at_ns);
            }
            false
        });
        (self.misses, self.last_miss, self.switch) = (self.misses + missed, last_miss, switch);
        if self.switch {
            return Some(Why::MissedButtons);
        }
        // The quiet starts at the monitor's newest event (or now, before its first), where the pointer is.
        let (since, from) = match self.quiet_from {
            Some((at, from)) if monitor_ns <= at => (at, from),
            Some(_) => {
                self.quiet_from = Some((monitor_ns, place));
                return None;
            }
            None => {
                self.quiet_from = Some((now_ns.max(monitor_ns), place));
                return None;
            }
        };
        let moved = (place.0 - from.0).hypot(place.1 - from.1) > MOVED_PTS;
        let main_ran = main_ns >= since.saturating_add(QUIET_NS / 2);
        if moved && main_ran && now_ns.saturating_sub(since) >= QUIET_NS {
            if own(place) {
                self.quiet_from = Some((now_ns, place));
                return None;
            }
            return Some(Why::NoMoves);
        }
        None
    }
}

/// Whether the sampler sends the main queue another block (it stores when it ran): only once the last one, sent at
/// `sent_ns` (0: none yet), has run (`main_ns`). A queued block runs in order anyway, so a stall never piles them up
///.
pub fn ping_due(sent_ns: u64, main_ns: u64) -> bool {
    sent_ns == 0 || main_ns >= sent_ns
}

/// See the module notes.
#[derive(Clone, Debug, Default)]
pub struct ClockOffset {
    /// Events seen (up to `OFFSET_EVENTS`), and the smallest lag yet.
    n: usize,
    least: Option<i64>,
    told: bool,
}

impl ClockOffset {
    /// An event stamped `ts_ns` that arrived at `now_ns`: its time on the host clock, never after its arrival.
    pub fn stamp(&mut self, ts_ns: u64, now_ns: u64) -> u64 {
        let lag = (now_ns as i64).saturating_sub(ts_ns as i64);
        self.least = Some(self.least.map_or(lag, |least| least.min(lag)));
        self.n = (self.n + 1).min(OFFSET_EVENTS);
        // Before the offset, only a sleep-sized lag (another clock base) takes the arrival; once decided, every event is
        // its own time plus the offset. A late one (a stalled main thread) keeps its own time, so a click's down stays
        // before its up, and the reorder window takes it. A stamp far from the clock's own lag (a stale
        // one) takes its arrival.
        let fixed = match self.offset() {
            Some(offset) if lag.saturating_sub(offset).abs() <= SNAP_LIMIT_NS => {
                (ts_ns as i64).saturating_add(offset)
            }
            None if lag.abs() <= SNAP_LIMIT_NS => ts_ns as i64,
            _ => return now_ns,
        };
        (fixed.max(0) as u64).min(now_ns)
    }

    /// The correction once `OFFSET_EVENTS` events came: the smallest lag seen, when it is past 1 s, else 0. A stall only
    /// adds lag, so a stall over the first events can't decide a false offset, and any prompt delivery since lowers it
    ///.
    pub fn offset(&self) -> Option<i64> {
        let least = self.least.filter(|_| self.n >= OFFSET_EVENTS)?;
        Some(if least.abs() > OFFSET_LIMIT_NS {
            least
        } else {
            0
        })
    }

    /// The smallest lag seen (for the take's log).
    pub fn least_lag(&self) -> Option<i64> {
        self.least
    }

    /// The correction once it is decided, the first time it is asked (for the take's log).
    pub fn take_decided(&mut self) -> Option<i64> {
        let d = self.offset().filter(|_| !self.told);
        self.told |= d.is_some();
        d
    }
}
