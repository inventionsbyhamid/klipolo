//! The watchdog: never a video without a cursor. For a take whose frames don't show the cursor
//! (`CursorPlan::Separate`), it decides to burn the cursor in once its source has said nothing for 1.5 s while the
//! screen is live (its pipe running, the take not paused, its display awake), or at once when the source
//! fails. It decides once and never switches back. Pure: every time is an argument (host ns).
//!
//! The silence clock starts when the take records (`start`), and starts again whenever the screen isn't live (a pause,
//! a restart, a sleeping display), so only live silence counts. It also starts again at a poll that came late (the core
//! or the whole process stalled): the source's beats may be queued right behind that poll, so time nobody
//! watched never counts as silence. A source that fails before the take records decides at the first tick after it
//! starts, so the decision always happens while the take can act on the pipe's answer.

use crate::preflight::cursor::CursorPlan;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BurnReason {
    Silent,
    SourceFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchAction {
    None,
    BurnIn(BurnReason),
}

/// See the module notes.
#[derive(Clone, Debug)]
pub struct Watch {
    plan: CursorPlan,
    /// When the source last said something (its arrival, host ns).
    last_ns: Option<u64>,
    /// Where the silence clock starts: the take's start, the screen's last moment not live, or a late poll.
    since_ns: Option<u64>,
    /// The last tick's time (a tick later than `LATE_NS` after it restarts the clock).
    last_tick_ns: Option<u64>,
    failed: bool,
    decided: bool,
    burned: bool,
    paused: bool,
}

impl Watch {
    pub const SILENT_NS: u64 = 1_500_000_000;
    /// A tick this long after the last one came late: the core polls every 100 ms while it watches.
    pub const LATE_NS: u64 = 300_000_000;

    pub fn new(plan: CursorPlan) -> Self {
        Self {
            plan,
            last_ns: None,
            since_ns: None,
            last_tick_ns: None,
            failed: false,
            decided: false,
            burned: false,
            paused: false,
        }
    }

    /// A resumed take whose journal says it burned in (or decided to) starts burned.
    pub fn resumed_burned() -> Self {
        Self {
            decided: true,
            burned: true,
            ..Self::new(CursorPlan::Burned)
        }
    }

    /// The take records from `now_ns`: the silence clock starts.
    pub fn start(&mut self, now_ns: u64) {
        (self.since_ns, self.last_tick_ns) = (Some(now_ns), Some(now_ns));
    }

    /// The source said something, arriving at `host_ns`.
    pub fn on_event(&mut self, host_ns: u64) {
        self.last_ns = Some(self.last_ns.map_or(host_ns, |l| l.max(host_ns)));
    }

    /// The source can't go on: burn in now, or at the first tick once the take records (again).
    pub fn on_failed(&mut self) -> WatchAction {
        if !self.watching() {
            return WatchAction::None;
        }
        self.failed = true;
        if self.paused || self.since_ns.is_none() {
            return WatchAction::None;
        }
        self.decided = true;
        WatchAction::BurnIn(BurnReason::SourceFailed)
    }

    /// A pause restarts the silence clock.
    pub fn set_paused(&mut self, paused: bool, now_ns: u64) {
        self.paused = paused;
        if !paused && self.since_ns.is_some() {
            self.since_ns = Some(now_ns);
        }
    }

    /// `screen_live`: the screen pipe is running, not paused, and its display is awake.
    pub fn tick(&mut self, now_ns: u64, screen_live: bool) -> WatchAction {
        let late = (self.last_tick_ns).is_some_and(|l| now_ns.saturating_sub(l) > Self::LATE_NS);
        self.last_tick_ns = self.last_tick_ns.map(|_| now_ns);
        if !self.watching() || self.paused || self.since_ns.is_none() {
            return WatchAction::None;
        }
        if !screen_live {
            self.since_ns = Some(now_ns);
            return WatchAction::None;
        }
        if self.failed {
            self.decided = true;
            return WatchAction::BurnIn(BurnReason::SourceFailed);
        }
        if late {
            self.since_ns = Some(now_ns);
            return WatchAction::None;
        }
        let quiet_from = self.since_ns.max(self.last_ns).unwrap_or(now_ns);
        if now_ns >= quiet_from.saturating_add(Self::SILENT_NS) {
            self.decided = true;
            return WatchAction::BurnIn(BurnReason::Silent);
        }
        WatchAction::None
    }

    /// The frames show the cursor now (the stream confirmed it, or the healed run's first frame came).
    pub fn set_burned(&mut self) {
        (self.decided, self.burned) = (true, true);
    }

    /// The watchdog decided to burn in (the frames may not show it yet).
    pub fn decided(&self) -> bool {
        self.decided
    }

    pub fn burned(&self) -> bool {
        self.burned
    }

    fn watching(&self) -> bool {
        self.plan == CursorPlan::Separate && !self.decided
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: u64 = 1_000_000_000;

    fn started() -> Watch {
        let mut w = Watch::new(CursorPlan::Separate);
        w.start(10 * S);
        w
    }

    /// Polls every 100 ms after `from` up to `to` (both on the host clock), the screen `live`; the first burn-in.
    fn polled(w: &mut Watch, from: u64, to: u64, live: bool) -> Option<(u64, BurnReason)> {
        let mut now = from;
        while now + S / 10 <= to {
            now += S / 10;
            if let WatchAction::BurnIn(why) = w.tick(now, live) {
                return Some((now, why));
            }
        }
        None
    }

    #[test]
    fn silence_for_1_5_s_while_the_screen_is_live_burns_in_once() {
        let mut w = started();
        assert_eq!(polled(&mut w, 10 * S, 11 * S, true), None);
        w.on_event(11 * S);
        let burned = polled(&mut w, 11 * S, 20 * S, true);
        assert_eq!(burned, Some((12 * S + S / 2, BurnReason::Silent)));
        assert!(w.decided() && !w.burned());
        assert_eq!(polled(&mut w, 12 * S + S / 2, 20 * S, true), None, "once");
        w.set_burned();
        assert!(w.burned());
        // A source that never said anything burns in 1.5 s after the take started recording.
        let mut quiet = started();
        let burned = polled(&mut quiet, 10 * S, 20 * S, true);
        assert_eq!(burned, Some((11 * S + S / 2, BurnReason::Silent)));
    }

    #[test]
    fn a_still_screen_with_beats_never_burns_in() {
        let mut w = started();
        for i in 0..100 {
            let now = 10 * S + i * S / 2;
            w.on_event(now);
            assert_eq!(polled(&mut w, now, now + S / 2, true), None);
        }
        // Burned takes and games have nothing to burn in.
        let mut burned = Watch::new(CursorPlan::Burned);
        burned.start(0);
        assert_eq!(polled(&mut burned, 0, 60 * S, true), None);
        assert_eq!(burned.on_failed(), WatchAction::None);
    }

    #[test]
    fn a_failed_source_burns_in_at_once() {
        let mut w = started();
        w.on_event(10 * S);
        assert_eq!(w.on_failed(), WatchAction::BurnIn(BurnReason::SourceFailed));
        assert_eq!(w.on_failed(), WatchAction::None);
        // Failed while paused: as the take records again.
        let mut paused = started();
        paused.set_paused(true, 11 * S);
        assert_eq!(paused.on_failed(), WatchAction::None);
        assert_eq!(polled(&mut paused, 11 * S, 20 * S, true), None);
        paused.set_paused(false, 21 * S);
        let burned = polled(&mut paused, 21 * S, 22 * S, true);
        assert_eq!(burned, Some((21 * S + S / 10, BurnReason::SourceFailed)));
    }

    #[test]
    fn a_paused_or_sleeping_screen_is_not_live() {
        let mut w = started();
        w.on_event(10 * S);
        w.set_paused(true, 10 * S);
        assert_eq!(polled(&mut w, 10 * S, 30 * S, true), None, "paused");
        w.set_paused(false, 30 * S);
        assert_eq!(
            polled(&mut w, 30 * S, 31 * S, true),
            None,
            "the clock restarted"
        );
        // A sleeping display (or a restarting pipe) isn't live, and its silence doesn't count afterwards.
        assert_eq!(polled(&mut w, 31 * S, 40 * S, false), None);
        let burned = polled(&mut w, 40 * S, 45 * S, true);
        assert_eq!(burned, Some((41 * S + S / 2, BurnReason::Silent)));
    }

    #[test]
    fn a_late_poll_restarts_the_silence_clock() {
        let mut w = started();
        w.on_event(10 * S);
        assert_eq!(w.tick(10 * S + S / 10, true), WatchAction::None);
        // The core (or the process) stalled for 2 s: the poll that comes late restarts the clock, as the source's beats
        // may be queued right behind it.
        assert_eq!(w.tick(12 * S + S / 10, true), WatchAction::None);
        // On-time polls with nothing arriving: 1.5 s after the late poll, it burns in.
        let mut now = 12 * S + S / 10;
        while now + S / 10 < 13 * S + 6 * S / 10 {
            now += S / 10;
            assert_eq!(w.tick(now, true), WatchAction::None, "at {now}");
        }
        assert_eq!(
            w.tick(13 * S + 6 * S / 10, true),
            WatchAction::BurnIn(BurnReason::Silent)
        );
    }

    #[test]
    fn a_source_that_fails_before_the_take_records_decides_at_its_first_tick() {
        let mut w = Watch::new(CursorPlan::Separate);
        assert_eq!(w.on_failed(), WatchAction::None, "not recording yet");
        assert!(!w.decided());
        w.start(10 * S);
        assert_eq!(
            w.tick(10 * S + S / 10, true),
            WatchAction::BurnIn(BurnReason::SourceFailed)
        );
    }
}
