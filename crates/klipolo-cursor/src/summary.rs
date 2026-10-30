//! The cursor in plain words: the planner's cursor line in its context, nearest the playhead, and
//! `describeCursor`'s answer for a range. Places are region words from a 3 × 3 grid, never coordinates; times are
//! `m:ss` (`h:mm:ss` past an hour, `klipolo_model::time::clock`).
//!
//! **Cost:** a binary search per kind, then the nearest facts one at a time, the line rendered again after each (a
//! few dozen facts fit 400 bytes): O(kinds × log n + k²) for k listed.

use std::fmt::Write;

use klipolo_model::time::clock;

use crate::Us;
use crate::timeline::{ClickAt, TimelineFacts};

mod playhead;

pub use playhead::{NEAR_US, playhead_note};

const MINUTE: Us = 60_000_000;
/// Clicks this close together in one region print as one run ("1:04 and 1:05 (top right)").
const RUN_GAP_US: Us = 3_000_000;
/// `describe` lists this many of each kind, then says how many more.
const DESCRIBE_MAX: usize = 12;
/// The kinds, in the order they print: clicks, pauses (dwells), typing, idle stretches.
const KINDS: usize = 4;
const NAMES: [(&str, &str); KINDS] = [
    ("click", "clicks"),
    ("pause", "pauses"),
    ("typing stretch", "typing stretches"),
    ("idle stretch", "idle stretches"),
];

/// "top left" … "centre" … "bottom right" (thirds).
pub fn region_word(x: f32, y: f32) -> &'static str {
    const WORDS: [[&str; 3]; 3] = [
        ["top left", "top centre", "top right"],
        ["middle left", "centre", "middle right"],
        ["bottom left", "bottom centre", "bottom right"],
    ];
    let third = |v: f32| (v >= 1.0 / 3.0) as usize + (v >= 2.0 / 3.0) as usize;
    WORDS[third(y)][third(x)]
}

/// The planner's cursor line: counts, then the facts nearest the playhead, taken outward in time order
/// until `max_bytes`; the printed window is the earliest to the latest fact listed, rounded out to whole minutes.
/// The idle count reads "typing and idle not measured yet" until every take's screen is analysed, then "idle not
/// measured yet" until its sound is measured. None when there are no facts (or the counts alone don't fit).
pub fn summary_line(f: &TimelineFacts, playhead_us: Us, max_bytes: usize) -> Option<String> {
    listed_near(f, playhead_us, max_bytes).map(|(_, line)| line)
}

/// Each kind's facts `lo..hi` that `summary_line` lists for `playhead_us` within `max_bytes`, and the line.
fn listed_near(f: &TimelineFacts, playhead_us: Us, max_bytes: usize) -> Option<(Listed, String)> {
    if (0..KINDS).all(|k| count(f, k) == 0) {
        return None;
    }
    // Each kind's listed facts are `lo..hi`, grown outward from the playhead one nearest fact at a time.
    let all: Listed = std::array::from_fn(|k| (0, count(f, k)));
    let mut listed: Listed = std::array::from_fn(|k| {
        let split = first_not(count(f, k), |i| times(f, k, i).0 <= playhead_us);
        (split, split)
    });
    let mut line = summary(f, &listed);
    if line.len() > max_bytes {
        return None;
    }
    while let Some((k, earlier)) = nearest(f, &listed, &all, playhead_us) {
        let mut more = listed;
        if earlier {
            more[k].0 -= 1;
        } else {
            more[k].1 += 1;
        }
        let longer = summary(f, &more);
        if longer.len() > max_bytes {
            break;
        }
        (listed, line) = (more, longer);
    }
    Some((listed, line))
}

/// Each kind's facts `lo..hi` (indices into its list).
type Listed = [(usize, usize); KINDS];

/// `describeCursor`'s answer for [from, to), in plain words.
pub fn describe(f: &TimelineFacts, from: Us, to: Us) -> String {
    let mut s = format!("Between {} and {}: ", clock(from), clock(to));
    let mut any = false;
    for (k, &name) in NAMES.iter().enumerate() {
        // A click inside [from, to), or a span reaching into it.
        let before = |i| match times(f, k, i) {
            (at, _) if k == 0 => at < from,
            (_, end) => end <= from,
        };
        let lo = first_not(count(f, k), before);
        let hi = first_not(count(f, k), |i| times(f, k, i).0 < to).max(lo);
        if lo == hi {
            continue;
        }
        if any {
            s.push_str("; ");
        }
        any = true;
        let _ = write!(s, "{}: ", counted(hi - lo, name));
        items(&mut s, f, k, (lo, hi.min(lo + DESCRIBE_MAX)));
        if hi - lo > DESCRIBE_MAX {
            let _ = write!(s, ", and {} more", hi - lo - DESCRIBE_MAX);
        }
    }
    if !any {
        s.push_str("no clicks, pauses, typing or idle stretches");
    }
    s.push('.');
    if !f.activity_known {
        s.push_str(
            " Typing and idle stretches aren't counted yet: the screen is still being analysed.",
        );
    } else if !f.sound_known {
        s.push_str(" Idle stretches aren't counted yet: the sound is still being measured.");
    }
    s
}

fn count(f: &TimelineFacts, k: usize) -> usize {
    [f.clicks.len(), f.dwells.len(), f.typing.len(), f.idle.len()][k]
}

/// Kind `k`'s fact `i` as [from, to] (a click's both ends are its time).
fn times(f: &TimelineFacts, k: usize, i: usize) -> (Us, Us) {
    match k {
        0 => (f.clicks[i].timeline_us, f.clicks[i].timeline_us),
        1 => (f.dwells[i].from_us, f.dwells[i].to_us),
        2 => (f.typing[i].from_us, f.typing[i].to_us),
        _ => f.idle[i],
    }
}

/// Where kind `k`'s fact `i` is, in words (idle has no place).
fn place(f: &TimelineFacts, k: usize, i: usize) -> Option<&'static str> {
    match k {
        1 => Some(region_word(f.dwells[i].x, f.dwells[i].y)),
        2 => {
            let r = f.typing[i].rect;
            Some(region_word(r.x + r.w / 2.0, r.y + r.h / 2.0))
        }
        _ => None,
    }
}

/// The first index in 0..n where `before` turns false (true, then false, as for `partition_point`).
fn first_not(n: usize, before: impl Fn(usize) -> bool) -> usize {
    let (mut lo, mut hi) = (0, n);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if before(mid) {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    lo
}

/// The fact just outside the listed ones nearest `t`, among each kind's `within`: its kind, and whether it is the
/// earlier side. Ties go to the earlier fact.
fn nearest(f: &TimelineFacts, listed: &Listed, within: &Listed, t: Us) -> Option<(usize, bool)> {
    let away = |(from, to): (Us, Us)| from.saturating_sub(t).max(t.saturating_sub(to)).max(0);
    let sides = (0..KINDS).flat_map(|k| {
        let (lo, hi) = listed[k];
        let earlier = (lo > within[k].0).then(|| (k, true, times(f, k, lo - 1)));
        let later = (hi < within[k].1).then(|| (k, false, times(f, k, hi)));
        earlier.into_iter().chain(later)
    });
    sides
        .min_by_key(|&(k, _, span)| (away(span), span.0, k))
        .map(|(k, earlier, _)| (k, earlier))
}

/// "Cursor: 3 clicks, 1 pause, 1 idle stretch. Near 1:00–3:00: clicks 1:04 and 1:05 (top right); …".
fn summary(f: &TimelineFacts, listed: &Listed) -> String {
    let mut s = String::with_capacity(400);
    // Until every take's screen and sound are measured, "0 idle stretches" is a guess the planner would repeat; and
    // until the screen is, saying nothing of typing reads as "no typing".
    let idle = if !f.activity_known {
        "typing and idle not measured yet".to_owned()
    } else if f.sound_known {
        counted(f.idle.len(), NAMES[3])
    } else {
        "idle not measured yet".to_owned()
    };
    let _ = write!(
        s,
        "Cursor: {}, {}, {idle}.",
        counted(f.clicks.len(), NAMES[0]),
        counted(f.dwells.len(), NAMES[1]),
    );
    let spans = (0..KINDS).flat_map(|k| (listed[k].0..listed[k].1).map(move |i| times(f, k, i)));
    let Some((first, last)) = spans.fold(None, |w: Option<(Us, Us)>, (a, b)| {
        Some(w.map_or((a, b), |(lo, hi)| (lo.min(a), hi.max(b))))
    }) else {
        return s;
    };
    let from = first.div_euclid(MINUTE) * MINUTE;
    let to = (last.saturating_add(MINUTE - 1).div_euclid(MINUTE) * MINUTE).max(from + MINUTE);
    let _ = write!(s, " Near {}–{}: ", clock(from), clock(to));
    by_kind(&mut s, f, listed);
    s.push('.');
    s
}

/// The `listed` facts kind by kind: "clicks 1:04 and 1:05 (top right); paused 1:12–1:15 (middle left)".
fn by_kind(s: &mut String, f: &TimelineFacts, listed: &Listed) {
    let labels = ["clicks ", "paused ", "typing ", "idle "];
    let mut first_kind = true;
    for (k, &(lo, hi)) in listed.iter().enumerate().filter(|(_, l)| l.0 < l.1) {
        if !first_kind {
            s.push_str("; ");
        }
        first_kind = false;
        s.push_str(if k == 0 && hi - lo == 1 {
            "click "
        } else {
            labels[k]
        });
        items(s, f, k, (lo, hi));
    }
}

/// Kind `k`'s facts `lo..hi` in time order: clicks in runs ("1:04 and 1:05 (top right), 1:31 (centre)"), the others
/// as spans ("1:12–1:15 (middle left)").
fn items(s: &mut String, f: &TimelineFacts, k: usize, (lo, hi): (usize, usize)) {
    if k == 0 {
        return click_runs(s, &f.clicks[lo..hi]);
    }
    for i in lo..hi {
        if i > lo {
            s.push_str(", ");
        }
        let (from, to) = times(f, k, i);
        let _ = write!(s, "{}–{}", clock(from), clock(to));
        if let Some(word) = place(f, k, i) {
            let _ = write!(s, " ({word})");
        }
    }
}

/// Consecutive clicks within `RUN_GAP_US` in one region print as one run.
fn click_runs(s: &mut String, clicks: &[ClickAt]) {
    let mut rest = clicks;
    while let Some(c) = rest.first() {
        let word = region_word(c.x, c.y);
        let same = |pair: &[ClickAt]| {
            pair[1].timeline_us - pair[0].timeline_us <= RUN_GAP_US
                && region_word(pair[1].x, pair[1].y) == word
        };
        let n = 1 + rest.windows(2).take_while(|pair| same(pair)).count();
        if rest.len() < clicks.len() {
            s.push_str(", ");
        }
        for (j, c) in rest[..n].iter().enumerate() {
            if j > 0 {
                s.push_str(if j + 1 == n { " and " } else { ", " });
            }
            s.push_str(&clock(c.timeline_us));
        }
        let _ = write!(s, " ({word})");
        rest = &rest[n..];
    }
}

/// "1 click", "3 clicks".
fn counted(n: usize, (one, many): (&str, &str)) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}
