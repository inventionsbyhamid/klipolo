//! How long a frame stays the same, conservatively: never later than the next change. A track changes from its
//! start through its last key (staggered members and repeats through their spread or last cycle), except across a
//! segment that holds (a `hold` ease, or equal values neither of which is read from the tracks before); modifiers, a
//! typewriter, scramble or counter running, live or coming particles, a backdrop (what is under the layer changes)
//! and a fill whose generator animates change all the time.

use super::super::instance::SceneInstance;
use super::super::timeline::Track;
use super::tracks::{secs, us};
use crate::motion::Ease;

/// The next time any value changes after `t`: `f64::INFINITY` when nothing does again, `t` itself when something is
/// changing at `t` (always with a backdrop, an animating generator fill or particles alive at `t`).
pub fn static_until(inst: &SceneInstance, t: f64) -> f64 {
    if inst.has_backdrop || inst.fills_animate || !t.is_finite() {
        return t;
    }
    if inst.still.is_some() {
        return f64::INFINITY;
    }
    let mut next = f64::INFINITY;
    for &e in &inst.emitters {
        let Some(em) = inst.nodes.get(e).and_then(|n| n.emitter.as_deref()) else {
            continue;
        };
        if t < em.at {
            next = next.min(em.at);
        } else if em.rate > 0.0 || (em.burst > 0 && t < em.at + em.life[1]) {
            return t;
        }
    }
    let cs = inst.compiled();
    for (list, &c) in inst.lists.iter().zip(&inst.list_cnode) {
        // A node's own stagger spreads at most over the units the check counted.
        let most_units = cs.nodes.get(c).map_or(0, |n| n.max_units);
        for tr in list.iter().filter_map(|&i| inst.pool.get(i as usize)) {
            let spread = match tr.tween {
                Some(ti) if tr.stagger.is_some() => inst.groups.get(ti).map_or(0.0, |g| g.total),
                None if tr.unit => tr.stagger.map_or(0.0, |st| st.total(most_units)),
                _ => 0.0,
            };
            next = next.min(track_next(tr, t, spread));
            if next <= t {
                return t;
            }
        }
    }
    let used = |c: usize| inst.first.get(c).is_some_and(|&f| f != u32::MAX);
    if inst
        .mods
        .iter()
        .enumerate()
        .any(|(c, m)| !m.is_empty() && used(c))
    {
        return t;
    }
    for n in &inst.nodes {
        if let Some(a) = n.anim.as_deref() {
            let windows = [
                a.count.as_ref().map(|c| (c.at, c.dur)),
                a.reveal.map(|r| (r.at, r.dur)),
                a.scramble.as_ref().map(|(s, _, _)| (s.at, s.dur)),
            ];
            for (at, dur) in windows.into_iter().flatten() {
                if t < at {
                    next = next.min(at);
                } else if t < at + dur {
                    return t;
                }
            }
            if let Some(&(at, _)) = a.holds.iter().find(|(at, _)| *at > t) {
                next = next.min(at);
            }
        }
    }
    next
}

/// When `tr` next changes after `t`.
fn track_next(tr: &Track, t: f64, spread: f64) -> f64 {
    let (Some(first), Some(last)) = (tr.keys.first(), tr.keys.last()) else {
        return f64::INFINITY;
    };
    let start = secs(first.t_us);
    let mut end = secs(last.t_us);
    if let Some(r) = tr.repeat {
        end = if r.count < 0 {
            f64::INFINITY
        } else {
            let runs = f64::from(r.count) + 1.0;
            start + runs * (end - start) + (runs - 1.0) * r.delay
        };
    }
    if t < start {
        return start;
    }
    if t >= end + spread {
        return f64::INFINITY;
    }
    if spread > 0.0 || tr.repeat.is_some() {
        return t;
    }
    let i = tr.keys.partition_point(|k| k.t_us <= us(t));
    let read_before = |k: usize| tr.chain == Some(k);
    match (
        i.checked_sub(1).and_then(|a| tr.keys.get(a)),
        tr.keys.get(i),
    ) {
        (Some(_), Some(b)) if b.ease == Some(Ease::Hold) => secs(b.t_us),
        (Some(a), Some(b)) if a.v == b.v && !read_before(i - 1) && !read_before(i) => secs(b.t_us),
        _ => t,
    }
}
