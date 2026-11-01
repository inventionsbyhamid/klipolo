//! Lossless thinning: a move that the straight line between its kept
//! neighbours reproduces, at its own time, within what the reader keeps is never written. The reader keeps a position
//! as 16-bit fixed point over −0.5 to 1.5, to the nearest (`klipolo_cursor::track::QUANTUM`, 2 / 65,535 of the picture), so
//! within half a step: a dropped move is off the chord by no more than the reader's own rounding. The reader
//! interpolates linearly between kept positions, so it reads the same path; smooth motion writes far fewer lines
//! (Beats Cap 12, and keeps the file quick to load), real pointer motion (whole and half points) a few. Every point
//! dropped since the last one written is checked against each new chord, so the error never builds up.
//!
//! Nothing waits long: the newest point is written at every gather, tick and flush (`flush`), so held-back points
//! never add to what a crash loses. Stills keep their points: the chord across a still runs through it in time, so the
//! rest point and the held place before a far move are never on it.
//!
//! Cost: O(dropped) per point, at most `MAX_DROPPED` (a gather's worth at 250 Hz is 25); no allocation once warm.

use klipolo_model::time::Us;

/// A position: its time and place (already rounded to the written precision).
pub(super) type Point = (Us, f32, f32);

/// The reader's kept precision (half its step): a dropped point is within this of the chord, on each axis.
const TOLERANCE: f64 = klipolo_cursor::track::QUANTUM / 2.0;
/// At most this many points dropped between two written ones.
const MAX_DROPPED: usize = 64;
/// What the reader's fixed point holds of a place (`klipolo_cursor::track`: −0.5 to 1.5 of the picture): the chord is
/// checked on what it reads, so a straight run across an edge keeps the point where it crosses.
const READ_RANGE: (f64, f64) = (-0.5, 1.5);

/// See the module notes.
#[derive(Clone, Debug, Default)]
pub(super) struct Straight {
    /// The last point written.
    anchor: Option<Point>,
    /// The newest point, not written yet.
    pending: Option<Point>,
    /// The points dropped since the anchor.
    dropped: Vec<Point>,
}

impl Straight {
    /// Going on after a point already in the file (a resumed take).
    pub(super) fn after(written: Point) -> Self {
        Self {
            anchor: Some(written),
            ..Self::default()
        }
    }

    /// The next point (times increase); `write` gets the points to write now, in order.
    pub(super) fn push(&mut self, p: Point, write: &mut impl FnMut(Point)) {
        let Some(anchor) = self.anchor else {
            self.anchor = Some(p);
            return write(p);
        };
        if let Some(pending) = self.pending {
            let fits = self.dropped.len() < MAX_DROPPED
                && on_chord(anchor, p, pending)
                && self.dropped.iter().all(|&d| on_chord(anchor, p, d));
            if fits {
                self.dropped.push(pending);
            } else {
                write(pending);
                self.anchor = Some(pending);
                self.dropped.clear();
            }
        }
        self.pending = Some(p);
    }

    /// The newest point written now (a gather, a tick, a flush).
    pub(super) fn flush(&mut self, write: &mut impl FnMut(Point)) {
        if let Some(p) = self.pending.take() {
            write(p);
            self.anchor = Some(p);
            self.dropped.clear();
        }
    }
}

/// `c` (between them in time) is within the reader's precision of the chord from `a` to `b`, at its own time, as the
/// reader reads all three.
fn on_chord(a: Point, b: Point, c: Point) -> bool {
    let span = (b.0 - a.0) as f64;
    if span <= 0.0 {
        return false;
    }
    let k = (c.0 - a.0) as f64 / span;
    let read = |v: f32| f64::from(v).clamp(READ_RANGE.0, READ_RANGE.1);
    let near = |va: f32, vb: f32, vc: f32| {
        let (va, vb) = (read(va), read(vb));
        (va + (vb - va) * k - read(vc)).abs() <= TOLERANCE + 1e-9
    };
    near(a.1, b.1, c.1) && near(a.2, b.2, c.2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(points: &[Point]) -> Vec<Point> {
        let (mut s, mut out) = (Straight::default(), Vec::new());
        for &p in points {
            s.push(p, &mut |q| out.push(q));
        }
        s.flush(&mut |q| out.push(q));
        out
    }

    #[test]
    fn a_straight_run_keeps_its_ends_and_a_turn_keeps_its_corner() {
        let line: Vec<Point> = (0..10)
            .map(|i| (i * 4_000, 0.1 + 0.001 * i as f32, 0.5))
            .collect();
        assert_eq!(run(&line), [line[0], line[9]]);
        let mut corner = line.clone();
        corner.extend((1..5).map(|i| (36_000 + i * 4_000, 0.109, 0.5 + 0.001 * i as f32)));
        let kept = run(&corner);
        assert!(kept.contains(&line[9]), "the corner: {kept:?}");
        // A still: the rest point stays (the chord across the still runs through it in time).
        let still = [
            (0, 0.1, 0.5),
            (4_000, 0.2, 0.5),
            (504_000, 0.2, 0.5),
            (508_000, 0.3, 0.5),
        ];
        assert_eq!(run(&still), still);
    }

    #[test]
    fn a_move_within_half_the_readers_step_of_the_chord_is_dropped_and_one_past_it_is_kept() {
        // The chord's middle is 0.100015; the reader's step is 2 / 65,535 ≈ 3.05e-5.
        let (a, b) = ((0, 0.1, 0.5), (8_000, 0.100_03, 0.5));
        let within = (4_000, 0.100_03, 0.5);
        assert_eq!(
            run(&[a, within, b]),
            [a, b],
            "1.5e-5 off: within half a step"
        );
        let past = (4_000, 0.100_04, 0.5);
        assert_eq!(run(&[a, past, b]), [a, past, b], "2.5e-5 off: past it");
    }

    #[test]
    fn a_straight_run_across_the_readers_edge_keeps_the_point_where_it_crosses() {
        // The reader clamps to −0.5..1.5, so past 1.5 a straight run reads as a corner.
        let (a, c, b) = ((0, 1.4, 0.5), (4_000, 1.6, 0.5), (8_000, 1.8, 0.5));
        assert_eq!(run(&[a, c, b]), [a, c, b]);
        let (a, c, b) = ((0, -0.4, 0.5), (4_000, -0.6, 0.5), (8_000, -0.8, 0.5));
        assert_eq!(run(&[a, c, b]), [a, c, b]);
    }
}
