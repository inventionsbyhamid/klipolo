#![allow(dead_code)]
//! Test inputs shared by the cursor crate's tests: cursor files built from lines, seeded paths and presses,
//! seeded camera focus, and the follow-crop's takes, the same on every machine.

pub mod activity;
pub mod camera;
pub mod follow;

#[allow(unused_imports)]
pub use camera::{
    check, click, cursor_through, in_span, input, on_screen, random_fixed, random_focus,
    random_focus_counts, walk_to_edge,
};

use klipolo_cursor::format::{Capture, Line, write_line};
use klipolo_cursor::{ClickSource, CursorTrack, Limits, Us};

/// A recording's first line: separate cursor, exact clicks, a 1440 × 900 point screen.
pub fn head() -> Line {
    Line::Header {
        v: 2,
        capture: Capture::Separate,
        clicks: ClickSource::Events,
        w: 1440.0,
        h: 900.0,
    }
}

/// A cursor file of `lines`.
pub fn file(lines: &[Line]) -> Vec<u8> {
    let mut out = Vec::new();
    for l in lines {
        write_line(l, &mut out);
    }
    out
}

/// `bytes` read as a take's file of that length is.
pub fn read(bytes: &[u8]) -> CursorTrack {
    CursorTrack::read(bytes, Limits::for_file_len(bytes.len() as u64))
}

/// SplitMix64: a seed gives the same numbers everywhere.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// In [0, 1).
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// In [lo, hi).
    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }
}

/// Positions every 4 ms (250 Hz, the most a recorder writes).
pub const STEP_US: Us = 4_000;

/// The file of a seeded random path over `[0, span_us)`: eased glides, sudden jumps, quick reversals and stops (no
/// lines while still but a heartbeat every 500 ms), inside the frame.
pub fn path_file(seed: u64, span_us: Us) -> Vec<u8> {
    let mut rng = Rng::new(seed);
    let mut out = Vec::with_capacity((span_us / STEP_US) as usize * 44);
    write_line(&head(), &mut out);
    let (mut x, mut y) = (rng.range(0.1, 0.9), rng.range(0.1, 0.9));
    let mut t: Us = 0;
    let put = |t: Us, x: f64, y: f64, out: &mut Vec<u8>| {
        let (x, y) = (x as f32, y as f32);
        write_line(&Line::Move { t, x, y }, out);
    };
    put(t, x, y, &mut out);
    while t < span_us {
        let pick = rng.unit();
        if pick < 0.2 {
            // Still: a heartbeat every 500 ms.
            let until = t + (rng.range(0.3, 2.0) * 1e6) as Us;
            while t + 500_000 < until.min(span_us) {
                t += 500_000;
                put(t, x, y, &mut out);
            }
            t = until;
            continue;
        }
        if pick < 0.3 {
            (x, y) = (rng.range(0.05, 0.95), rng.range(0.05, 0.95));
            t += STEP_US;
            put(t, x, y, &mut out);
            continue;
        }
        let (fx, fy) = (x, y);
        let (tx, ty) = (rng.range(0.05, 0.95), rng.range(0.05, 0.95));
        let back = pick < 0.4;
        let steps = (rng.range(0.15, 1.5) * 1e6) as Us / STEP_US;
        for i in 1..=steps.max(1) {
            let mut f = i as f64 / steps.max(1) as f64;
            if back {
                f = 1.0 - (2.0 * f - 1.0).abs();
            }
            let e = f * f * (3.0 - 2.0 * f);
            (x, y) = (fx + (tx - fx) * e, fy + (ty - fy) * e);
            t += STEP_US;
            if t >= span_us {
                break;
            }
            put(t, x, y, &mut out);
        }
    }
    out
}

/// A seeded random path over `[0, span_us)` (`path_file`).
pub fn random_path(seed: u64, span_us: Us) -> CursorTrack {
    read(&path_file(seed, span_us))
}

/// A steady movement from `x0` to `x1` at height `y` over `span_us`, at 250 Hz.
pub fn line(x0: f32, x1: f32, y: f32, span_us: Us) -> CursorTrack {
    let mut lines = vec![head()];
    for t in (0..=span_us).step_by(STEP_US as usize) {
        let x = x0 + (x1 - x0) * (t as f32 / span_us as f32);
        lines.push(Line::Move { t, x, y });
    }
    read(&file(&lines))
}

/// `track`'s lines: its kept positions and its presses (downs and ups), not yet in time order.
fn lines_of(track: &CursorTrack) -> Vec<Line> {
    let mut lines: Vec<Line> = Vec::new();
    for p in track.presses() {
        let (x, y) = (p.x, p.y);
        lines.push(Line::Button {
            t: p.down,
            btn: p.btn,
            d: 1,
            x,
            y,
        });
        if p.up != Us::MAX {
            let (x, y) = track.position(p.up).unwrap_or((x, y));
            lines.push(Line::Button {
                t: p.up,
                btn: p.btn,
                d: 0,
                x,
                y,
            });
        }
    }
    if let Some((from, to)) = track.span() {
        lines.extend(
            track
                .samples(from, to + 1)
                .map(|(t, x, y)| Line::Move { t, x, y }),
        );
    }
    lines
}

/// The file of `track`'s header and `lines`, in time order.
fn file_with(track: &CursorTrack, mut lines: Vec<Line>) -> Vec<u8> {
    lines.sort_by_key(|l| match l {
        Line::Move { t, .. } | Line::Button { t, .. } => *t,
        _ => 0,
    });
    let mut out = Vec::new();
    if let Some(h) = track.header() {
        let (w, h2) = h.area_pts;
        write_line(
            &Line::Header {
                v: 2,
                capture: h.capture,
                clicks: h.clicks,
                w,
                h: h2,
            },
            &mut out,
        );
    }
    for l in &lines {
        write_line(l, &mut out);
    }
    out
}

/// `track`'s file: its header, its kept positions and its presses, in time order.
pub fn bytes_of(track: &CursorTrack) -> Vec<u8> {
    file_with(track, lines_of(track))
}

/// `track` with presses of the primary button about every 1.3 s where the cursor was: clicks of 80–400 ms, and every
/// fourth a drag of 0.6–1.5 s.
pub fn with_presses(track: CursorTrack) -> CursorTrack {
    let Some((from, to)) = track.span() else {
        return track;
    };
    let mut rng = Rng::new(from as u64 ^ 0xC1C);
    let mut lines = lines_of(&track);
    let mut t = from + 300_000;
    let mut k = 0;
    while t < to {
        let long = k % 4 == 3;
        let held = (if long {
            rng.range(0.6, 1.5)
        } else {
            rng.range(0.08, 0.4)
        } * 1e6) as Us;
        let up = (t + held).min(to);
        let (x, y) = track.position(t).unwrap_or((0.5, 0.5));
        lines.push(Line::Button {
            t,
            btn: 0,
            d: 1,
            x,
            y,
        });
        let (x, y) = track.position(up).unwrap_or((x, y));
        lines.push(Line::Button {
            t: up,
            btn: 0,
            d: 0,
            x,
            y,
        });
        t = up + (rng.range(0.4, 1.6) * 1e6) as Us;
        k += 1;
    }
    read(&file_with(&track, lines))
}

/// The smallest and largest raw positions over `[from, to]`, as ((min x, min y), (max x, max y)).
pub fn range(track: &CursorTrack, from: Us, to: Us) -> ((f32, f32), (f32, f32)) {
    let ends = [track.position(from), track.position(to)];
    let inside = track
        .samples(from, to.saturating_add(1))
        .map(|(_, x, y)| (x, y));
    let mut lo = (f32::INFINITY, f32::INFINITY);
    let mut hi = (f32::NEG_INFINITY, f32::NEG_INFINITY);
    for (x, y) in ends.into_iter().flatten().chain(inside) {
        lo = (lo.0.min(x), lo.1.min(y));
        hi = (hi.0.max(x), hi.1.max(y));
    }
    (lo, hi)
}

/// The raw path didn't move over `[from, to]`.
pub fn still_between(track: &CursorTrack, from: Us, to: Us) -> bool {
    let (lo, hi) = range(track, from, to);
    lo == hi
}
