//! The disk tests' cursor streams and what a take writes for one per hour: smooth synthetic paths (`continuous`, a
//! circle; `typical_mix`, straight runs) and real pointer motion (`real_motion`, `real_mix`: accelerating flicks on
//! whole and half points, as a mouse reports them).

use klipolo_capture::cursor::CursorEvent;
use klipolo_capture::preflight::cursor::CursorPlan;
use klipolo_capture::session::CURSOR;

use super::super::Harness;

/// Moves at `hz` for `secs` around a circle (each event's `host_ns` from 0: `cursor_bytes_per_hour` places them).
pub fn continuous(hz: f64, secs: f64) -> Vec<(f64, CursorEvent)> {
    let n = (hz * secs) as u64;
    (0..n)
        .map(|k| {
            let t = k as f64 / hz;
            let turn = (t / 4.0).fract() * std::f64::consts::TAU;
            let (x, y) = (
                (0.5 + 0.3 * turn.cos()) as f32,
                (0.5 + 0.3 * turn.sin()) as f32,
            );
            (t, CursorEvent::Move { host_ns: 0, x, y })
        })
        .collect()
}

/// A typical tutorial's cursor (`continuous`'s timing): moving 30 % of the time at 120 Hz (in straight runs), a click
/// every 4 s, a shape change every 10 s, and a beat every 500 ms while still.
pub fn typical_mix(secs: f64) -> Vec<(f64, CursorEvent)> {
    mix(secs, |t| {
        let x = 0.2 + 0.6 * (t / 7.0).fract();
        (x as f32, (0.3 + 0.4 * t.fract()) as f32)
    })
}

/// Real pointer motion at `hz` for `secs`, never still: one flick after another (`Flicks`).
pub fn real_motion(hz: f64, secs: f64) -> Vec<(f64, CursorEvent)> {
    let mut flicks = Flicks::default();
    let n = (hz * secs) as u64;
    (0..n)
        .map(|k| {
            let t = k as f64 / hz;
            let (x, y) = flicks.at(t);
            (t, CursorEvent::Move { host_ns: 0, x, y })
        })
        .collect()
}

/// `typical_mix` with real pointer motion while moving (`Flicks`).
pub fn real_mix(secs: f64) -> Vec<(f64, CursorEvent)> {
    let mut flicks = Flicks::default();
    mix(secs, move |t| flicks.at(t))
}

/// Flicks across a 1440 × 900 point display, each to a pseudo-random target in 150-450 ms on a minimum-jerk profile
/// (it speeds up, then slows down) along a slight arc, its places on whole and half points, as a mouse reports them.
#[derive(Default)]
struct Flicks {
    seed: u64,
    from: (f64, f64),
    to: (f64, f64),
    start: f64,
    len: f64,
}

impl Flicks {
    const AREA: (f64, f64) = (1440.0, 900.0);

    fn at(&mut self, t: f64) -> (f32, f32) {
        while self.len == 0.0 || t >= self.start + self.len {
            self.seed = self
                .seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let r = |shift: u32| (self.seed >> shift & 0xffff) as f64 / 65_535.0;
            self.from = if self.len == 0.0 {
                (720.0, 450.0)
            } else {
                self.to
            };
            self.to = (60.0 + r(16) * 1320.0, 60.0 + r(32) * 780.0);
            self.start = if self.len == 0.0 {
                0.0
            } else {
                self.start + self.len
            };
            self.len = 0.15 + r(48) * 0.3;
        }
        let u = ((t - self.start) / self.len).clamp(0.0, 1.0);
        let s = u * u * u * (10.0 - 15.0 * u + 6.0 * u * u);
        let (dx, dy) = (self.to.0 - self.from.0, self.to.1 - self.from.1);
        let arc = 0.08 * (u * std::f64::consts::PI).sin();
        let x = self.from.0 + dx * s - dy * arc;
        let y = self.from.1 + dy * s + dx * arc;
        let half = |v: f64| (v * 2.0).round() / 2.0;
        (
            (half(x) / Self::AREA.0) as f32,
            (half(y) / Self::AREA.1) as f32,
        )
    }
}

/// Moving 30 % of each second at 120 Hz along `place_at`, a beat every 500 ms while still, a click every 4 s and a
/// shape change every 10 s.
fn mix(secs: f64, mut place_at: impl FnMut(f64) -> (f32, f32)) -> Vec<(f64, CursorEvent)> {
    let mut out = Vec::new();
    let (mut t, mut place) = (0.0_f64, (0.5_f32, 0.5_f32));
    let mut shapes_sent = [false; 2];
    while t < secs {
        if t.fract() < 0.3 {
            place = place_at(t);
            let (x, y) = place;
            out.push((t, CursorEvent::Move { host_ns: 0, x, y }));
            t += 1.0 / 120.0;
        } else {
            let (x, y) = place;
            out.push((t, CursorEvent::Beat { host_ns: 0, x, y }));
            t = (t + 0.5).min(t.floor() + 1.0);
        }
    }
    for k in 0..(secs / 4.0) as u64 {
        let at = k as f64 * 4.0 + 0.1;
        for (dt, down) in [(0.0, true), (0.08, false)] {
            let (x, y) = (0.5, 0.5);
            let ev = CursorEvent::Button {
                host_ns: 0,
                btn: 0,
                down,
                x,
                y,
            };
            out.push((at + dt, ev));
        }
    }
    for k in 0..(secs / 10.0) as u64 {
        let which = (k % 2) as usize;
        let (at, key) = (k as f64 * 10.0 + 0.2, which as u64 + 1);
        let ev = if std::mem::replace(&mut shapes_sent[which], true) {
            CursorEvent::Shape { host_ns: 0, key }
        } else {
            let image = crate::common::shape(32, 32);
            CursorEvent::NewShape {
                host_ns: 0,
                key,
                image,
            }
        };
        out.push((at, ev));
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out
}

/// Runs a separate take on `script` and gives what its cursor wrote (the file and its shapes) per hour.
pub fn cursor_bytes_per_hour(script: Vec<(f64, CursorEvent)>) -> f64 {
    let secs = script.last().map_or(1.0, |(t, _)| *t).max(1.0);
    let mut h = Harness::recording_cursor(CursorPlan::Separate);
    for (t, mut ev) in script {
        let at = h.at(t);
        match &mut ev {
            CursorEvent::Move { host_ns, .. }
            | CursorEvent::Button { host_ns, .. }
            | CursorEvent::Shape { host_ns, .. }
            | CursorEvent::NewShape { host_ns, .. }
            | CursorEvent::Visible { host_ns, .. }
            | CursorEvent::Area { host_ns, .. }
            | CursorEvent::Beat { host_ns, .. }
            | CursorEvent::ClicksSampled { host_ns }
            | CursorEvent::ShapesFull { host_ns } => *host_ns = at,
            CursorEvent::Failed => {}
        }
        h.cursor(t, ev);
    }
    h.run_to(secs + 0.2);
    assert!(
        !h.core.cursor_watch().decided(),
        "the script kept the cursor alive"
    );
    let files = h.stopped_files();
    let bytes: usize = (files.iter())
        .filter(|(k, _)| *k == CURSOR || k.starts_with("cursors/"))
        .map(|(_, v)| v.len())
        .sum();
    bytes as f64 * 3_600.0 / secs
}
