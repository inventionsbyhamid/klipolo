//! Seeded camera inputs for the camera tests: a tutorial's mix of clicks, pauses and typing, and a cursor walking
//! to the frame's edge.

use klipolo_cursor::camera::{Fixed, Focus, Input, Plan, Rules, View, Weight, check_plan};
use klipolo_cursor::{Rect01, Us};

use super::Rng;

const S: f64 = 1e6;

/// A click at `at` on (x, y).
pub fn click(at: Us, x: f32, y: f32) -> Focus {
    point(at, at, x as f64, y as f64, Weight::Click)
}

/// What to plan over `[0, span)`.
pub fn input<'a>(span: Us, focus: &'a [Focus], fixed: &'a [Fixed], rules: Rules) -> Input<'a> {
    Input {
        span: (0, span),
        focus,
        fixed,
        rules,
    }
}

/// `check_plan` over `[0, span)`, failing with `label` and the reason.
pub fn check(p: &Plan, r: Rules, span: Us, label: &str) {
    check_plan(p, r, span).unwrap_or_else(|e| panic!("{label}: {e}"));
}

/// How many of `focus` start in `[0, span)`: each is either a kept event or counted as dropped.
pub fn in_span(focus: &[Focus], span: Us) -> u32 {
    focus.iter().filter(|f| f.at >= 0 && f.at < span).count() as u32
}

/// The view at `t` shows (x, y), anywhere in it: worked out from its rectangle, apart from the plan's own lists.
pub fn on_screen(p: &Plan, t: Us, x: f32, y: f32) -> bool {
    let (r, e) = (p.view_at(t).rect(), 1e-6);
    x >= r.x - e && x <= r.x + r.w + e && y >= r.y - e && y <= r.y + r.h + e
}

/// Seeded person's zooms over `[0, span_us)`: 1.25–3×, 1.5–4 s long, each view inside the frame, starting 3 s or more
/// from the span's ends. Most lie 6–30 s apart; one in three follows the one before 1–3 s later at nearly its view
/// so the move between them is short and never has to be squeezed.
pub fn random_fixed(seed: u64, span_us: Us) -> Vec<Fixed> {
    let mut rng = Rng::new(seed ^ 0xF1_3ED);
    let mut out: Vec<Fixed> = Vec::new();
    let mut t = (rng.range(3.0, 20.0) * S) as Us;
    loop {
        let to = t + (rng.range(1.5, 4.0) * S) as Us;
        if to > span_us - 3_000_000 {
            return out;
        }
        let close = out.last().filter(|p| p.to + 3_000_000 >= t).map(|p| p.view);
        let scale = close.map_or_else(|| rng.range(1.25, 3.0), |v| v.scale);
        let half = 0.5 / scale;
        let mut near = |c: Option<f64>| match c {
            Some(c) => (c + rng.range(-0.03, 0.03)).clamp(half, 1.0 - half),
            None => rng.range(half, 1.0 - half),
        };
        let (cx, cy) = (near(close.map(|v| v.cx)), near(close.map(|v| v.cy)));
        out.push(Fixed {
            from: t,
            to,
            view: View { cx, cy, scale },
        });
        let gap = if rng.unit() < 1.0 / 3.0 {
            rng.range(1.0, 3.0)
        } else {
            rng.range(6.0, 30.0)
        };
        t = to + (gap * S) as Us;
    }
}

fn point(at: Us, until: Us, x: f64, y: f64, weight: Weight) -> Focus {
    let rect = Rect01 {
        x: x as f32,
        y: y as f32,
        w: 0.0,
        h: 0.0,
    };
    Focus {
        at,
        until,
        rect,
        weight,
    }
}

/// A typed box of `w` × `h` around (x, y), kept inside the frame.
fn typed(at: Us, until: Us, x: f64, y: f64, w: f64, h: f64) -> Focus {
    let rect = Rect01 {
        x: (x - w / 2.0).clamp(0.0, 1.0 - w) as f32,
        y: (y - h / 2.0).clamp(0.0, 1.0 - h) as f32,
        w: w as f32,
        h: h as f32,
    };
    Focus {
        at,
        until,
        rect,
        weight: Weight::Typing,
    }
}

/// One focus of a seeded kind at `at` and (x, y): a click (60 %), a pause of 0.6–3 s (25 %) or a typed box of
/// 1.5–12 s (15 %), now and then wider than any view can frame.
fn one(rng: &mut Rng, at: Us, x: f64, y: f64) -> Focus {
    let kind = rng.unit();
    if kind < 0.6 {
        return point(at, at, x, y, Weight::Click);
    }
    if kind < 0.85 {
        let until = at + (rng.range(0.6, 3.0) * S) as Us;
        return point(at, until, x, y, Weight::Dwell);
    }
    let until = at + (rng.range(1.5, 12.0) * S) as Us;
    let (w, h) = (rng.range(0.02, 0.65), rng.range(0.02, 0.25));
    typed(at, until, x, y, w, h)
}

/// A tutorial-like mix over `[0, span_us)`: bursts of actions near each other, ordinary gaps and quiet stretches,
/// mostly near the last action, sometimes anywhere, and now and then at the frame's very edge (the menu bar, the
/// dock, a sidebar). Pauses and typing may overlap later actions. Sorted by time.
pub fn random_focus(seed: u64, span_us: Us) -> Vec<Focus> {
    let mut rng = Rng::new(seed ^ 0xCA3E_7A00);
    let mut out = Vec::new();
    let (mut x, mut y) = (0.5, 0.5);
    let mut t = (rng.range(0.2, 3.0) * S) as Us;
    while t < span_us {
        let pick = rng.unit();
        if pick < 0.6 {
            x = (x + rng.range(-0.12, 0.12)).clamp(0.0, 1.0);
            y = (y + rng.range(-0.12, 0.12)).clamp(0.0, 1.0);
        } else if pick < 0.9 {
            (x, y) = (rng.unit(), rng.unit());
        } else {
            let edge = rng.range(0.0, 0.03);
            let side = |v: bool| if v { edge } else { 1.0 - edge };
            if rng.unit() < 0.5 {
                (x, y) = (rng.unit(), side(rng.unit() < 0.5));
            } else {
                (x, y) = (side(rng.unit() < 0.5), rng.unit());
            }
        }
        out.push(one(&mut rng, t, x, y));
        let gap = match rng.unit() {
            g if g < 0.35 => rng.range(0.15, 1.2),
            g if g < 0.85 => rng.range(1.2, 5.0),
            _ => rng.range(5.0, 25.0),
        };
        t += (gap * S) as Us;
    }
    out
}

/// Exactly `clicks` clicks, `dwells` pauses and `typing` typed boxes at seeded times over `[0, span_us)` and seeded
/// places anywhere (the real-scale input). Sorted by time.
pub fn random_focus_counts(
    seed: u64,
    span_us: Us,
    clicks: usize,
    dwells: usize,
    typing: usize,
) -> Vec<Focus> {
    let mut rng = Rng::new(seed ^ 0x2_4A11);
    let mut out = Vec::with_capacity(clicks + dwells + typing);
    let at = |rng: &mut Rng| (rng.unit() * span_us as f64) as Us;
    for _ in 0..clicks {
        let t = at(&mut rng);
        out.push(point(t, t, rng.unit(), rng.unit(), Weight::Click));
    }
    for _ in 0..dwells {
        let t = at(&mut rng);
        let until = t + (rng.range(0.6, 3.0) * S) as Us;
        out.push(point(t, until, rng.unit(), rng.unit(), Weight::Dwell));
    }
    for _ in 0..typing {
        let t = at(&mut rng);
        let until = t + (rng.range(1.5, 12.0) * S) as Us;
        let (w, h) = (rng.range(0.02, 0.3), rng.range(0.02, 0.15));
        out.push(typed(t, until, rng.unit(), rng.unit(), w, h));
    }
    out.sort_by_key(|f| f.at);
    out
}

/// A raw cursor that reaches each focus's point by its start (the cursor is where it clicks) and wanders widely in
/// between, a seeded way each time, so the glide must follow it and the events must still win.
pub fn cursor_through(seed: u64, focus: &[Focus]) -> impl Fn(Us) -> Option<(f32, f32)> {
    let mut rng = Rng::new(seed ^ 0x6_11DE);
    let centre = |f: &Focus| {
        (
            (f.rect.x + f.rect.w / 2.0) as f64,
            (f.rect.y + f.rect.h / 2.0) as f64,
        )
    };
    let pts: Vec<(Us, (f64, f64), f64)> = focus
        .iter()
        .map(|f| (f.at, centre(f), rng.range(0.0, std::f64::consts::TAU)))
        .collect();
    move |t| {
        let i = pts.partition_point(|p| p.0 <= t);
        let (t0, a) = i
            .checked_sub(1)
            .map_or((0, (0.5, 0.5)), |i| (pts[i].0, pts[i].1));
        let Some(&(t1, b, angle)) = pts.get(i) else {
            return Some((a.0 as f32, a.1 as f32));
        };
        let u = (t - t0) as f64 / (t1 - t0).max(1) as f64;
        let away = 0.35 * (std::f64::consts::PI * u).sin();
        let x = a.0 + (b.0 - a.0) * u + away * angle.cos();
        let y = a.1 + (b.1 - a.1) * u + away * angle.sin();
        Some((x.clamp(0.0, 1.0) as f32, y.clamp(0.0, 1.0) as f32))
    }
}

/// A raw cursor resting in the middle until `start_us`, then walking right to x = 0.95 over `secs`, and resting there.
pub fn walk_to_edge(start_us: Us, secs: f64) -> impl Fn(Us) -> Option<(f32, f32)> {
    move |t| {
        let f = ((t - start_us) as f64 / (secs * S)).clamp(0.0, 1.0);
        Some(((0.5 + 0.45 * f) as f32, 0.5))
    }
}
