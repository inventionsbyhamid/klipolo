//! The cursor files and frames the Auto zoom tests plan over: a file of lines, a walk through key points clicking on
//! the way, a tutorial of clicks; and what a window shows.

use std::io::Write;

use klipolo_cursor::CursorTrack;
use klipolo_engine::media::cursor::load_track;
use klipolo_model::layout::BoxN;

use super::support::{bx, secs};

/// A cursor file of `lines` (format 2 moves and button lines), read back.
pub fn track(lines: &[String]) -> CursorTrack {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    for l in lines {
        writeln!(f, "{l}").unwrap();
    }
    load_track(f.path()).unwrap()
}

/// A cursor at 30 Hz over 60 s through `keys` (seconds, x, y: straight between them, resting before the first),
/// clicking where it is at each of `clicks` (seconds). After the last key it keeps moving (back and forth, 0.1 of the
/// frame every half second), so it never rests into a pause there: a pause the planner must hold to its end.
pub fn walk(keys: &[(f64, f32, f32)], clicks: &[f64]) -> CursorTrack {
    let at = |s: f64| {
        let i = keys.partition_point(|k| k.0 <= s);
        match (i.checked_sub(1).map(|i| keys[i]), keys.get(i)) {
            (Some(a), Some(b)) => {
                let u = ((s - a.0) / (b.0 - a.0)) as f32;
                (a.1 + (b.1 - a.1) * u, a.2 + (b.2 - a.2) * u)
            }
            (Some(a), None) => {
                let phase = ((s - a.0) * 2.0).fract() as f32;
                let off = 0.1 * (1.0 - (2.0 * phase - 1.0).abs());
                (a.1 + off, a.2)
            }
            (None, Some(b)) => (b.1, b.2),
            (None, None) => (0.5, 0.5),
        }
    };
    let mut lines: Vec<String> = (0..1_800)
        .map(|i| {
            let (x, y) = at(i as f64 / 30.0);
            format!(r#"{{"k":"m","t":{},"x":{x},"y":{y}}}"#, i * 33_333)
        })
        .collect();
    for &c in clicks {
        let (x, y) = at(c);
        for (t, d) in [(secs(c), 1), (secs(c) + 50_000, 0)] {
            lines.push(format!(
                r#"{{"k":"b","t":{t},"btn":0,"d":{d},"x":{x},"y":{y}}}"#
            ));
        }
    }
    track(&lines)
}

/// A tutorial: the cursor reaches each click's point 0.4 s before it and leaves 0.1 s after, gliding on to the next
/// (so it never rests long enough for a pause).
pub fn tutorial(clicks: &[(f64, f32, f32)]) -> CursorTrack {
    let mut keys: Vec<(f64, f32, f32)> = Vec::new();
    for &(t, x, y) in clicks {
        keys.push((t - 0.4, x, y));
        keys.push((t + 0.1, x, y));
    }
    walk(&keys, &clicks.iter().map(|c| c.0).collect::<Vec<_>>())
}

/// (x, y) inside `w`'s safe zone: 15 % of its width in from each side, a side on the frame's edge reaching it.
pub fn in_safe_zone(w: BoxN, x: f64, y: f64) -> bool {
    let (m, e) = (0.15 * w.w, 1e-6);
    let lo = |p: f64| if p <= e { 0.0 } else { p + m };
    let hi = |p: f64, s: f64| if p + s >= 1.0 - e { 1.0 } else { p + s - m };
    x >= lo(w.x) - e && x <= hi(w.x, w.w) + e && y >= lo(w.y) - e && y <= hi(w.y, w.h) + e
}

pub fn shows(w: Option<BoxN>, x: f64, y: f64) -> bool {
    let w = w.unwrap_or(bx(0.0, 0.0, 1.0, 1.0));
    let e = 1e-6;
    (w.x - e..=w.x + w.w + e).contains(&x) && (w.y - e..=w.y + w.h + e).contains(&y)
}
