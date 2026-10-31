//! The reorder window: cursor events come from several threads (the monitor, the
//! heartbeat, the shape sampler), so a button the hardware stamped 15 ms ago can arrive after a later heartbeat. Lines
//! wait here until they are `WINDOW_US` older than the newest time seen (or the clock), then leave sorted by time,
//! equal times in arrival order. A line older than the last of its own kind already written (one later than the
//! window) is dropped and counted, never written: the reader would skip it (it checks order per kind).
//!
//! Cost: a gather sorts the lines in the window (a few dozen at 250 Hz) in place; no allocation once warm.

use klipolo_cursor::LastTimes;
use klipolo_cursor::format::{Line, write_line};
use klipolo_model::time::Us;

/// Lines wait here until they are older than the newest seen by `WINDOW_US`, then leave sorted by time (stable).
#[derive(Clone, Debug)]
pub struct Reorder {
    pending: Vec<(Us, u32, Line)>,
    arrival: u32,
    newest: Option<Us>,
    last: KindTimes,
    late: u64,
}

impl Default for Reorder {
    fn default() -> Self {
        Self::new()
    }
}

impl Reorder {
    /// Above the p99 delivery lag.
    pub const WINDOW_US: Us = 50_000;

    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
            arrival: 0,
            newest: None,
            last: KindTimes::default(),
            late: 0,
        }
    }

    /// A resumed take's: nothing older than each kind's last line in its file is written.
    pub fn resumed(last: &LastTimes) -> Self {
        let mut r = Self::new();
        let at = |t: Option<Us>| t.unwrap_or(Us::MIN);
        r.last.moves = at(last.moves);
        r.last.buttons.fill(at(last.buttons));
        r.last.shapes = at(last.shapes);
        r.last.visible = at(last.visible);
        r.last.areas = at(last.areas);
        r.last.clicks = at(last.clicks);
        r.last.burned = at(last.burned);
        r
    }

    pub fn push(&mut self, t: Us, line: Line) {
        if self.pending.is_empty() {
            self.arrival = 0;
        }
        self.newest = Some(self.newest.map_or(t, |n| n.max(t)));
        self.pending.push((t, self.arrival, line));
        self.arrival = self.arrival.wrapping_add(1);
    }

    /// The clock moved on to `now` (session µs): lines older than it by the window are ready even if nothing newer came.
    pub fn advance(&mut self, now: Us) {
        self.newest = Some(self.newest.map_or(now, |n| n.max(now)));
    }

    /// The lines now older than the window, in time order, appended to `out`.
    pub fn drain_ready(&mut self, out: &mut Vec<u8>) {
        if let Some(newest) = self.newest {
            self.drain_to(Some(newest.saturating_sub(Self::WINDOW_US)), out);
        }
    }

    /// Everything, in time order (a pause, Stop, a burn-in).
    pub fn drain_all(&mut self, out: &mut Vec<u8>) {
        self.drain_to(None, out);
    }

    /// Lines dropped for arriving after a later line of their kind was written.
    pub fn late(&self) -> u64 {
        self.late
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    fn drain_to(&mut self, until: Option<Us>, out: &mut Vec<u8>) {
        self.pending.sort_unstable_by_key(|p| (p.0, p.1));
        let n = until.map_or(self.pending.len(), |u| {
            self.pending.partition_point(|p| p.0 <= u)
        });
        for (t, _, line) in self.pending.drain(..n) {
            if self.last.admit(&line, t) {
                write_line(&line, out);
            } else {
                self.late += 1;
            }
        }
    }
}

/// Each kind's last time written, as the reader orders them (each button on its own).
#[derive(Clone, Debug)]
struct KindTimes {
    moves: Us,
    buttons: Box<[Us; 256]>,
    shapes: Us,
    visible: Us,
    areas: Us,
    clicks: Us,
    burned: Us,
}

impl Default for KindTimes {
    fn default() -> Self {
        Self {
            moves: Us::MIN,
            buttons: Box::new([Us::MIN; 256]),
            shapes: Us::MIN,
            visible: Us::MIN,
            areas: Us::MIN,
            clicks: Us::MIN,
            burned: Us::MIN,
        }
    }
}

impl KindTimes {
    /// Whether `line` at `t` is no older than its kind's last; if so it becomes the last. Untimed lines always are.
    fn admit(&mut self, line: &Line, t: Us) -> bool {
        let last = match line {
            Line::Move { .. } => &mut self.moves,
            Line::Button { btn, .. } => &mut self.buttons[usize::from(*btn)],
            Line::Shape { .. } => &mut self.shapes,
            Line::Visible { .. } => &mut self.visible,
            Line::Area { .. } => &mut self.areas,
            Line::Clicks { .. } => &mut self.clicks,
            Line::Burned { .. } => &mut self.burned,
            Line::Header { .. } | Line::ShapeDef { .. } => return true,
        };
        let ok = t >= *last;
        if ok {
            *last = t;
        }
        ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mv(t: Us) -> Line {
        Line::Move { t, x: 0.5, y: 0.5 }
    }

    fn btn(t: Us, d: u8) -> Line {
        Line::Button {
            t,
            btn: 0,
            d,
            x: 0.5,
            y: 0.5,
        }
    }

    fn times(out: &[u8]) -> Vec<Us> {
        let lines = out.split(|b| *b == b'\n').filter(|l| !l.is_empty());
        lines
            .map(|l| match klipolo_cursor::format::parse_line(l).unwrap() {
                Line::Move { t, .. } | Line::Button { t, .. } => t,
                other => panic!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn lines_leave_sorted_once_older_than_the_window() {
        let mut r = Reorder::new();
        let mut out = Vec::new();
        r.push(1_000_000, mv(1_000_000));
        // The button the hardware stamped 15 ms before the beat, delivered after it, and its release.
        r.push(985_000, btn(985_000, 1));
        r.push(990_000, btn(990_000, 0));
        r.drain_ready(&mut out);
        assert!(out.is_empty(), "all three within 50 ms of the newest");
        r.advance(1_050_000);
        r.drain_ready(&mut out);
        assert_eq!(times(&out), [985_000, 990_000, 1_000_000]);
        assert!(r.is_empty());
        // Later than the window: an older button is dropped and counted, an equal time is kept.
        r.push(980_000, btn(980_000, 1));
        r.push(1_000_000, mv(1_000_000));
        r.drain_all(&mut out);
        assert_eq!(times(&out), [985_000, 990_000, 1_000_000, 1_000_000]);
        assert_eq!(r.late(), 1);
    }

    #[test]
    fn drain_all_empties_in_order() {
        let mut r = Reorder::new();
        for t in [30, 10, 20, 10] {
            r.push(t, mv(t));
        }
        let mut out = Vec::new();
        r.drain_all(&mut out);
        assert_eq!(times(&out), [10, 10, 20, 30]);
        assert!(r.is_empty());
        let resumed = &mut Reorder::resumed(&LastTimes {
            moves: Some(25),
            ..LastTimes::default()
        });
        resumed.push(20, mv(20));
        resumed.push(30, mv(30));
        let mut out = Vec::new();
        resumed.drain_all(&mut out);
        assert_eq!(times(&out), [30], "nothing older than the file's last move");
    }
}
