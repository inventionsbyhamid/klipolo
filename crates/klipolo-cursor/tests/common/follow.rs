//! Follow-crop inputs for its tests: crops of the 1440 × 900 test picture, cursor files built from glides, rests
//! and clicks, the speed check, and seeded tutorials with presses, dwells, typing (and its activity) and playing media.

use klipolo_cursor::activity::{ActivityTrack, TICK_US, Tick};
use klipolo_cursor::facts::{Dwell, StillRuns, dwells};
use klipolo_cursor::follow::{Crop, FollowInput, FollowPath, FollowRules};
use klipolo_cursor::format::Line;
use klipolo_cursor::{CursorTrack, Rect01, RegionSpan, Us};

use super::activity::{activity_of, cells, tick_of};
use super::{Rng, file, head, random_path, read, with_presses};

pub const S: Us = 1_000_000;
pub const STEP: Us = FollowPath::STEP_US;
/// The test picture's width over its height (the header's 1440 × 900 points).
pub const ASPECT: f64 = 1.6;

/// A 9:16 crop of the test picture, `closer` times closer.
pub fn vertical(closer: f64) -> Crop {
    Crop {
        w: 900.0 * 9.0 / 16.0 / 1440.0 / closer,
        h: 1.0 / closer,
    }
}

/// What `track` alone gives (no dwells, typing, media or activity), with the default rules.
pub fn alone(track: &CursorTrack, crop: Crop) -> FollowInput<'_> {
    FollowInput {
        track,
        dwells: &[],
        typing: &[],
        media: &[],
        activity: &[],
        aspect: ASPECT,
        crop,
        rules: FollowRules::default(),
    }
}

/// The cursor gliding from `a` to `b` over `(from, to]`, at 250 Hz.
pub fn glide(lines: &mut Vec<Line>, (from, to): (Us, Us), a: (f32, f32), b: (f32, f32)) {
    for t in (from + 4_000..=to).step_by(4_000) {
        let f = (t - from) as f32 / (to - from) as f32;
        let (x, y) = (a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f);
        lines.push(Line::Move { t, x, y });
    }
}

/// The cursor resting at (x, y) over `[from, to]`: a heartbeat every 500 ms.
pub fn rest(lines: &mut Vec<Line>, (from, to): (Us, Us), (x, y): (f32, f32)) {
    lines.extend((from..=to).step_by(500_000).map(|t| Line::Move { t, x, y }));
}

/// A primary click at `t` on (x, y), held 80 ms.
pub fn click(lines: &mut Vec<Line>, t: Us, (x, y): (f32, f32)) {
    for (t, d) in [(t, 1), (t + 80_000, 0)] {
        lines.push(Line::Button { t, btn: 0, d, x, y });
    }
}

/// The track of the test header and `lines`, put in time order.
pub fn cursor_of(mut lines: Vec<Line>) -> CursorTrack {
    lines.sort_by_key(|l| match l {
        Line::Move { t, .. }
        | Line::Button { t, .. }
        | Line::Visible { t, .. }
        | Line::Burned { t } => *t,
        _ => 0,
    });
    lines.insert(0, head());
    read(&file(&lines))
}

/// A box of the picture over `[from, to)`.
pub fn span(from: Us, to: Us, (x, y, w, h): (f32, f32, f32, f32)) -> RegionSpan {
    RegionSpan {
        from,
        to,
        rect: Rect01 { x, y, w, h },
    }
}

/// The centre at `t`, which must be known.
pub fn at(p: &FollowPath, t: Us) -> (f64, f64) {
    p.centre_at(t).unwrap_or_else(|| panic!("no centre at {t}"))
}

/// Every step of `p` over `[from, to)` keeps the crop inside the picture and moves at most twice `rules.speed` crop
/// widths a second, measured on a picture `aspect` wide; and at most once that where it can't have sped up (`doubled`).
/// Gives how far it moved, in picture widths.
pub fn check_speed(
    p: &FollowPath,
    input: &FollowInput,
    rules: FollowRules,
    aspect: f64,
    (from, to): (Us, Us),
) -> Result<f64, String> {
    let (crop, once) = (input.crop, rules.speed * input.crop.w * STEP as f64 / 1e6);
    let (hx, hy) = (crop.w / 2.0, crop.h / 2.0);
    let (mut last, mut travelled): (Option<(f64, f64)>, f64) = (None, 0.0);
    for t in (from..to).step_by(STEP as usize) {
        let (x, y) = at(p, t);
        if x < hx - 1e-12 || x > 1.0 - hx + 1e-12 || y < hy - 1e-12 || y > 1.0 - hy + 1e-12 {
            return Err(format!(
                "the crop shows past the picture at {t}: centre ({x}, {y})"
            ));
        }
        if let Some(c) = last {
            let moved = (x - c.0).hypot((y - c.1) / aspect);
            let fast = doubled(input, rules, t, c);
            let most = if fast {
                2.0 * once
            } else {
                once * (1.0 + 1e-3)
            } + 1e-12;
            if moved > most {
                return Err(format!(
                    "{moved} in the step to {t}, over {most} (doubled: {fast})"
                ));
            }
            travelled += moved;
        }
        last = Some((x, y));
    }
    Ok(travelled)
}

/// Whether the crop at `c` may move faster than its limit in the step to `t`: no click is due within the pre-roll (a
/// click never speeds it up), and typing goes on or the raw cursor, shown in the picture, is outside the crop's safe
/// room along a side the crop can move.
fn doubled(input: &FollowInput, rules: FollowRules, t: Us, c: (f64, f64)) -> bool {
    let inside = |x: f32, y: f32| (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y);
    let track = input.track;
    let click = track
        .clicks()
        .any(|k| inside(k.x, k.y) && k.down >= t && k.down - t <= rules.pre_roll_us);
    let typing = input.typing.iter().any(|s| s.from <= t && t < s.to);
    let out = |p: f32, c: f64, size: f64| {
        size < 1.0 && (f64::from(p) - c).abs() > (0.5 - rules.safe) * size
    };
    let margin = track
        .position(t)
        .filter(|&(x, y)| track.visible_at(t) && inside(x, y))
        .is_some_and(|(x, y)| out(x, c.0, input.crop.w) || out(y, c.1, input.crop.h));
    !click && (typing || margin)
}

/// Each step's length over `[from, to)`, in picture widths, on a picture `ASPECT` wide.
pub fn step_lengths(p: &FollowPath, (from, to): (Us, Us)) -> Vec<f64> {
    let c: Vec<(f64, f64)> = (from..to)
        .step_by(STEP as usize)
        .map(|t| at(p, t))
        .collect();
    c.windows(2)
        .map(|w| (w[1].0 - w[0].0).hypot((w[1].1 - w[0].1) / ASPECT))
        .collect()
}

/// The most a crop of width `w` moves in a step at its limit (the default rules).
pub fn once(w: f64) -> f64 {
    FollowRules::default().speed * w * STEP as f64 / 1e6
}

/// A seeded take: a wandering cursor with clicks and drags, its dwells, typing spans (a line typed through each, in
/// its activity ticks) and playing media, each list sorted and disjoint in time.
pub struct Tutorial {
    pub track: CursorTrack,
    pub dwells: Vec<Dwell>,
    pub typing: Vec<RegionSpan>,
    pub media: Vec<RegionSpan>,
    pub activity: ActivityTrack,
}

impl Tutorial {
    pub fn new(seed: u64, span_us: Us) -> Self {
        let track = with_presses(random_path(seed, span_us));
        let dwells = dwells(&track, &StillRuns::of(&track));
        let mut rng = Rng::new(seed ^ 0x7E57);
        let mut ticks = vec![Tick::default(); (span_us / TICK_US) as usize];
        let mut typing = Vec::new();
        let mut t = (rng.range(1.0, 20.0) * 1e6) as Us;
        while t < span_us - 10 * S {
            // 2–40 cells across (some wider than any crop) and 2–6 down; a 1 × 2 region every other tick walks it.
            let (wc, hc) = (rng.range(2.0, 40.0) as u8, rng.range(2.0, 6.0) as u8);
            let (x0, y0) = (
                rng.range(0.0, f64::from(64 - wc)) as u8,
                rng.range(0.0, f64::from(36 - hc)) as u8,
            );
            let to = t + (rng.range(1.5, 8.0) * 1e6) as Us;
            for (k, tick) in ticks[(t / TICK_US) as usize..(to / TICK_US) as usize]
                .iter_mut()
                .enumerate()
            {
                let j = (k / 2) as u8;
                if k % 2 == 0 {
                    *tick = tick_of(&[cells(x0 + j % wc, y0 + (j / wc * 2) % (hc - 1), 1, 2)]);
                }
            }
            let rect = (
                f32::from(x0) / 64.0,
                f32::from(y0) / 36.0,
                f32::from(wc) / 64.0,
                f32::from(hc) / 36.0,
            );
            typing.push(span(t, to, rect));
            t = to + (rng.range(5.0, 40.0) * 1e6) as Us;
        }
        let mut media = Vec::new();
        let mut t = (rng.range(1.0, 30.0) * 1e6) as Us;
        while t < span_us {
            let (w, h) = (rng.range(0.1, 0.4), rng.range(0.1, 0.4));
            let (x, y) = (rng.range(0.0, 1.0 - w), rng.range(0.0, 1.0 - h));
            let to = t + (rng.range(10.0, 20.0) * 1e6) as Us;
            media.push(span(t, to, (x as f32, y as f32, w as f32, h as f32)));
            t = to + (rng.range(10.0, 60.0) * 1e6) as Us;
        }
        let activity = activity_of(&ticks);
        Self {
            track,
            dwells,
            typing,
            media,
            activity,
        }
    }

    /// The take's input for `crop`, with its activity as `runs` gives it (`self.runs()`).
    pub fn input<'a>(&'a self, runs: &'a [(Us, &'a ActivityTrack)], crop: Crop) -> FollowInput<'a> {
        FollowInput {
            track: &self.track,
            dwells: &self.dwells,
            typing: &self.typing,
            media: &self.media,
            activity: runs,
            aspect: ASPECT,
            crop,
            rules: FollowRules::default(),
        }
    }

    /// One screen file, from the take's start.
    pub fn runs(&self) -> [(Us, &ActivityTrack); 1] {
        [(0, &self.activity)]
    }

    /// The take's span.
    pub fn span(&self) -> (Us, Us) {
        self.track.span().expect("a path")
    }
}
