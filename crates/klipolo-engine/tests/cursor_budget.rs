//! A cursor take's budget at real scale, asserted in release: an hour of a tutorial take and
//! an hour of continuous 250 Hz motion, each loaded and smoothed, with their times and bytes printed for the report.
//!
//! `cargo test --release -p klipolo-engine --test cursor_budget -- --ignored --nocapture`

use std::time::{Duration, Instant};

use klipolo_engine::media::cursor::{BUDGET_BYTES, load_take};
use klipolo_model::cursor_style::Smoothing;

/// SplitMix64, the same numbers everywhere.
struct Rng(u64);

impl Rng {
    fn unit(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        ((z ^ (z >> 31)) >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// An hour of cursor: flicks to random places on a 1440 × 900 point screen, each 150–450 ms of minimum-jerk motion on
/// whole and half points (as the real-motion fixture is). `typical`: moving a quarter of the time at 120 Hz, a beat
/// every 500 ms at rest and a click on about every other rest (one every 2.4 s); else 250 Hz of continuous motion.
fn hour(typical: bool) -> String {
    let mut rng = Rng(7);
    let (step, mut t, end) = (
        if typical { 8_333 } else { 4_000 },
        0_i64,
        3_600_000_000_i64,
    );
    let (mut x, mut y) = (720.0, 450.0);
    let mut out = String::with_capacity(48 << 20);
    out += "{\"k\":\"h\",\"v\":2,\"capture\":\"separate\",\"clicks\":\"events\",\"w\":1440,\"h\":900}\n";
    let mut line = |t: i64, x: f64, y: f64| {
        let (x, y) = (
            (x * 2.0).round() / 2.0 / 1440.0,
            (y * 2.0).round() / 2.0 / 900.0,
        );
        out += &format!("{{\"k\":\"m\",\"t\":{t},\"x\":{x:.5},\"y\":{y:.5}}}\n");
    };
    let mut clicks = Vec::new();
    while t < end {
        let (tx, ty) = (rng.unit() * 1440.0, rng.unit() * 900.0);
        let span = 150_000 + (rng.unit() * 300_000.0) as i64;
        let (fx, fy) = (x, y);
        let mut k = step;
        while k <= span {
            let u = k as f64 / span as f64;
            let s = u * u * u * (10.0 - 15.0 * u + 6.0 * u * u);
            line(t + k, fx + (tx - fx) * s, fy + (ty - fy) * s);
            k += step;
        }
        (x, y, t) = (tx, ty, t + span + step);
        if typical {
            // At rest three times as long as it moved, a beat every 500 ms; a click on every other rest.
            let rest = 3 * span;
            if rng.unit() < 0.5 {
                clicks.push(t);
            }
            let mut b = 500_000;
            while b < rest {
                line(t + b, x, y);
                b += 500_000;
            }
            t += rest;
        }
    }
    for c in clicks {
        out += &format!("{{\"k\":\"b\",\"t\":{c},\"btn\":0,\"d\":1,\"x\":0.5,\"y\":0.5}}\n");
        out += &format!(
            "{{\"k\":\"b\",\"t\":{},\"btn\":0,\"d\":0,\"x\":0.5,\"y\":0.5}}\n",
            c + 80_000
        );
    }
    out
}

/// The fastest of three times `f` measures.
fn timed(mut f: impl FnMut() -> Duration) -> Duration {
    (0..3).map(|_| f()).min().unwrap()
}

/// The fastest of `n` runs of `f`.
fn best(n: usize, mut f: impl FnMut()) -> Duration {
    (0..n)
        .map(|_| {
            let start = Instant::now();
            f();
            start.elapsed()
        })
        .min()
        .unwrap()
}

#[test]
#[ignore = "real scale; the budget holds in release (see the module notes)"]
fn an_hours_take_loads_and_smooths_within_the_budget() {
    let dir = tempfile::tempdir().unwrap();
    for typical in [true, false] {
        let path = dir.path().join(if typical {
            "typical.jsonl"
        } else {
            "worst.jsonl"
        });
        let text = hour(typical);
        std::fs::write(&path, &text).unwrap();
        let load = best(3, || drop(load_take(&path).unwrap()));
        let take = load_take(&path).unwrap();
        take.prepare(&[Smoothing::Medium]);
        // Each run on a take just read: its first smoothing (with the rest scan), then one more (sharing it).
        let first = timed(|| {
            let t = load_take(&path).unwrap();
            let start = Instant::now();
            t.prepare(&[Smoothing::Medium]);
            start.elapsed()
        });
        let second = timed(|| {
            let t = load_take(&path).unwrap();
            t.prepare(&[Smoothing::Medium]);
            let start = Instant::now();
            t.prepare(&[Smoothing::Medium, Smoothing::Strong]);
            start.elapsed()
        });
        let medium = take.smoothed(Smoothing::Medium).unwrap();
        // Hours of motion: the steps the path stores (one every 8,333 µs).
        let motion_h = medium.len() as f64 * 8_333.0 / 3.6e9;
        let samples = take.track.len();
        let per_motion_h = first.as_secs_f64() * 1e3 / motion_h;
        eprintln!(
            "{}: {:.1} MB file, {samples} positions, {} presses; load {load:?} (track {} B = {:.2} B a position, \
             {} still runs, {} dwells); Medium {:?} with the rest scan = {per_motion_h:.1} ms an hour of motion \
             ({motion_h:.2} h, {} B = {:.2} MB an hour of motion); one more smoothing ≤ {second:?}; the take {} B, \
             {} such takes in {BUDGET_BYTES} B",
            if typical {
                "a tutorial hour"
            } else {
                "an hour of 250 Hz motion"
            },
            text.len() as f64 / 1e6,
            take.track.presses().len(),
            take.track.bytes(),
            take.track.bytes() as f64 / samples as f64,
            take.still.runs().len(),
            take.dwells().len(),
            first,
            medium.bytes(),
            medium.bytes() as f64 / 1e6 / motion_h,
            take.bytes(),
            BUDGET_BYTES / take.bytes(),
        );
        // Memory (any build): 8 B a position; the path 4 B a step (1.73 MB an hour of motion, measured by its steps)
        // plus its segment table (16 B a segment, each at least 4/ω of steps, so at most a fifth more); a tutorial hour
        // within 3 MB with its path; four of the largest within the budget.
        assert!(take.track.bytes() <= samples * 8 + take.track.presses().len() * 64 + 4_096);
        assert!(medium.bytes() as f64 <= 1.73e6 * motion_h * 1.2);
        assert!(!typical || take.bytes() <= 3_000_000, "{}", take.bytes());
        assert!(take.bytes() * 4 <= BUDGET_BYTES);
        if cfg!(debug_assertions) {
            continue;
        }
        // Time (release, the target on an Intel i5): load ≤ 150 ms an hour of cursor data, asserted where it is
        // typical; an hour of continuous 250 Hz motion parses 900,000 lines, which a 2013 Xeon server does in about
        // 0.42 s, bounded here at 0.6 s. Smoothing ≤ 30 ms an hour of motion, with the take's rest scan.
        let load_ms = if typical { 150 } else { 600 };
        assert!(load <= Duration::from_millis(load_ms), "load {load:?}");
        assert!(per_motion_h <= 30.0, "{per_motion_h} ms an hour of motion");
    }
}
