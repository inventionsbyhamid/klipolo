//! The session clock: one timeline for every source of a take.
//!
//! Session time is host time (`klipolo_platform::clock::host_ns`) since the start gate, with every paused stretch left
//! out. So a pause leaves no gap: the first sample after Resume continues where the last one before Pause stopped, and
//! a source keeps one continuous run across pauses.
//!
//! - **Video** carries its own host times, so each packet maps through `session_us`.
//! - **Audio** PCM has no times; a run logs anchors (`drift::Anchor`) on this clock once a second, and `drift::fit`
//!   gives the run's offset and drift.
//! - A sample stamped before the start, or inside a pause, has no session time and is dropped. A sample stamped before
//!   a pause but delivered after it began keeps its time.
//!
//! Calls are lenient, because the worker's inputs race: a second `start`, a `pause` while paused or before the start,
//! and a `resume` while running change nothing. Host times that run backwards are clamped, so pauses never overlap and
//! session time never goes backwards.
//!
//! Cost: `session_us` and `elapsed_us` are O(log p) for p pauses (a binary search over the closed pauses, which keep
//! their running total), with no allocation; memory is 24 bytes a pause, and a pause is one press or one auto-pause.

use klipolo_model::time::Us;

/// A closed pause `[from, to)` in host ns, with the paused ns of every pause up to and including it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Pause {
    from: u64,
    to: u64,
    total_ns: u64,
}

/// Where a host time falls on the session clock.
enum At {
    /// Before the start, or the clock hasn't started.
    Before,
    /// Inside a pause; the session time the pause froze at, in ns.
    Paused(u64),
    /// Running; the session time in ns.
    Running(u64),
}

/// Host time since the start gate, with paused stretches left out. All times are host ns.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionClock {
    start: Option<u64>,
    /// Closed pauses, in order and disjoint.
    pauses: Vec<Pause>,
    open_pause: Option<u64>,
}

impl SessionClock {
    pub fn new() -> Self {
        Self::default()
    }

    /// The start gate opened at `host_ns`: session time 0. The first start wins.
    pub fn start(&mut self, host_ns: u64) {
        if self.start.is_none() {
            self.start = Some(host_ns);
        }
    }

    pub fn started(&self) -> bool {
        self.start.is_some()
    }

    /// Session time stops at `host_ns`. Ignored before the start and while paused.
    pub fn pause(&mut self, host_ns: u64) {
        let Some(start) = self.start else { return };
        if self.open_pause.is_some() {
            return;
        }
        let floor = self.pauses.last().map_or(start, |p| p.to);
        self.open_pause = Some(host_ns.max(floor));
    }

    /// Session time runs again from `host_ns`. Ignored unless paused.
    pub fn resume(&mut self, host_ns: u64) {
        let Some(from) = self.open_pause.take() else {
            return;
        };
        let to = host_ns.max(from);
        if to > from {
            let total_ns = self.paused_ns_before(self.pauses.len()) + (to - from);
            self.pauses.push(Pause { from, to, total_ns });
        }
    }

    pub fn paused(&self) -> bool {
        self.open_pause.is_some()
    }

    /// The session time of a sample stamped `host_ns`; None before the start or inside a pause (the sample is dropped).
    pub fn session_us(&self, host_ns: u64) -> Option<Us> {
        match self.at(host_ns) {
            At::Running(ns) => Some(us(ns)),
            At::Before | At::Paused(_) => None,
        }
    }

    /// The first instant at or after `host_ns` that the clock runs: `host_ns` itself while running, the start when
    /// before it, the resume when inside a closed pause; None before the clock started or inside the open pause. A sound
    /// block is cut there, at its own frames, so pauses leave no step between its frames and their times.
    pub fn next_running(&self, host_ns: u64) -> Option<u64> {
        let start = self.start?;
        if host_ns < start {
            return Some(start);
        }
        if self.open_pause.is_some_and(|from| host_ns >= from) {
            return None;
        }
        let i = self.pauses.partition_point(|p| p.to <= host_ns);
        match self.pauses.get(i) {
            Some(p) if p.from <= host_ns => Some(p.to),
            _ => Some(host_ns),
        }
    }

    /// Where the first pause that begins after `host_ns` begins (closed or open); None when none does.
    pub fn next_pause(&self, host_ns: u64) -> Option<u64> {
        let i = self.pauses.partition_point(|p| p.from <= host_ns);
        self.pauses
            .get(i)
            .map(|p| p.from)
            .or(self.open_pause.filter(|&from| from > host_ns))
    }

    /// Elapsed session time at `host_ns` (frozen while paused). 0 before the start.
    pub fn elapsed_us(&self, host_ns: u64) -> Us {
        match self.at(host_ns) {
            At::Running(ns) | At::Paused(ns) => us(ns),
            At::Before => 0,
        }
    }

    fn at(&self, host_ns: u64) -> At {
        let Some(start) = self.start else {
            return At::Before;
        };
        if host_ns < start {
            return At::Before;
        }
        if let Some(from) = self.open_pause.filter(|&from| host_ns >= from) {
            return At::Paused(from - start - self.paused_ns_before(self.pauses.len()));
        }
        // The first pause that hadn't ended by `host_ns`; every earlier one is left out of the session time.
        let i = self.pauses.partition_point(|p| p.to <= host_ns);
        let before = self.paused_ns_before(i);
        match self.pauses.get(i) {
            Some(p) if p.from <= host_ns => At::Paused(p.from - start - before),
            _ => At::Running(host_ns - start - before),
        }
    }

    /// The paused ns of the first `i` closed pauses.
    fn paused_ns_before(&self, i: usize) -> u64 {
        i.checked_sub(1)
            .map_or(0, |last| self.pauses[last].total_ns)
    }
}

/// ns → µs, truncated. Any u64 of ns fits an `Us` once divided by 1,000.
fn us(ns: u64) -> Us {
    (ns / 1_000) as Us
}
