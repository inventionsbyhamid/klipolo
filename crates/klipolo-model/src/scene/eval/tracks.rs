//! Reading shared tracks for one member (an instance node, or one of its units) at a moment, without allocating. One
//! property's tracks, in timeline order (where the timeline placed each tween, then list order), give the value of
//! the last one in that order that has started for this member, each start moved by the member's stagger offset (a
//! GSAP seek); a repeat maps the moment into its cycle (a yoyo's odd cycles run backwards); keys are
//! found by binary search, as `motion::value_at` does. A key that takes the value the property had is read from the
//! tracks before it at that member's own moment, at most [`CHAIN_DEPTH`] deep. Every track looked
//! at is counted; an instance that could read past `MAX_TRACKS` a frame draws its settled still instead (`instance`).

use std::cell::Cell;

use super::super::instance::SceneInstance;
use super::super::timeline::Track;
use super::state::ClipState;
use crate::motion::Key;

/// How many tracks back a chained key reads (`from` after `from` after …); past it, the resting value.
pub(crate) const CHAIN_DEPTH: u8 = 8;

/// Seconds to microseconds, held to the scene's longest.
pub(crate) fn us(t: f64) -> i64 {
    if t.is_finite() {
        (t.clamp(-1e4, 1e4) * 1e6).round() as i64
    } else {
        0
    }
}

pub(crate) fn secs(us: i64) -> f64 {
    us as f64 / 1e6
}

/// When `tr`'s keys are read at `t` (its offset already taken away): inside a repeat's cycle, held at a cycle's end
/// through its delay and after the last; an odd cycle of a yoyo runs backwards, so an odd repeat ends on the start.
pub(crate) fn local(tr: &Track, t: f64) -> f64 {
    let (Some(r), Some(a), Some(b)) = (tr.repeat, tr.keys.first(), tr.keys.last()) else {
        return t;
    };
    let start = secs(a.t_us);
    let dur = secs(b.t_us - a.t_us);
    let tau = t - start;
    if tau <= 0.0 || dur <= 0.0 {
        return t;
    }
    let period = dur + r.delay.max(0.0);
    let cycles = if r.count < 0 {
        f64::INFINITY
    } else {
        f64::from(r.count) + 1.0
    };
    let mut k = (tau / period).floor();
    let mut phase = (tau - k * period).min(dur);
    if k >= cycles {
        k = cycles - 1.0;
        phase = dur;
    }
    if r.yoyo && (k as i64) % 2 == 1 {
        phase = dur - phase;
    }
    start + phase
}

/// The keys either side of `t_us` and the eased progress between them, as `value_at` reads them.
pub(crate) fn segment(keys: &[Key], t_us: i64) -> Option<(usize, usize, f64)> {
    let first = keys.first()?;
    if t_us <= first.t_us {
        return Some((0, 0, 0.0));
    }
    let next = keys.partition_point(|k| k.t_us <= t_us);
    if next >= keys.len() {
        let last = keys.len() - 1;
        return Some((last, last, 1.0));
    }
    let a = next - 1;
    let span = (keys[next].t_us - keys[a].t_us).max(1) as f64;
    let p = keys[next]
        .ease
        .unwrap_or_default()
        .apply((t_us - keys[a].t_us) as f64 / span);
    Some((a, next, p))
}

/// Reads one member's tracks.
pub(crate) struct Reader<'a> {
    pub inst: &'a SceneInstance,
    pub node: usize,
    /// The unit, for a split text's unit tracks.
    pub unit: Option<usize>,
    /// Tracks looked at this frame, over every member.
    pub reads: &'a Cell<usize>,
}

impl Reader<'_> {
    fn track(&self, i: u32) -> Option<&Track> {
        self.inst.pool.get(i as usize)
    }

    /// When this member starts `tr`, after its first member: its stagger offset.
    fn offset(&self, tr: &Track) -> f64 {
        let j = self.unit.unwrap_or(0);
        match tr.tween {
            Some(ti) if tr.stagger.is_some() => {
                let base = self.inst.base_of(ti, self.node);
                self.inst
                    .groups
                    .get(ti)
                    .map_or(0.0, |g| g.offset(base + if tr.unit { j } else { 0 }))
            }
            None if tr.unit && tr.stagger.is_some() => self
                .inst
                .nodes
                .get(self.node)
                .and_then(|n| n.node_group)
                .and_then(|g| self.inst.groups.get(g))
                .map_or(0.0, |g| g.offset(j)),
            _ => 0.0,
        }
    }

    /// The track ruling at `t`: the last in timeline order that has started for this member (starts moved by its
    /// offsets), and its offset; else the one that starts first and its offset, with false.
    fn ruling(&self, list: &[u32], t: f64) -> Option<(usize, f64, bool)> {
        let mut best: Option<(usize, f64, f64)> = None;
        let mut first: Option<(usize, f64, f64)> = None;
        for (k, &i) in list.iter().enumerate() {
            let Some(tr) = self.track(i) else {
                continue;
            };
            self.reads.set(self.reads.get() + 1);
            let o = self.offset(tr);
            let s = secs(tr.start_us()) + o;
            // Timeline order (the run's order) decides among those started, as GSAP renders its children: the later
            // tween wins, whatever its stagger made it start for this member.
            if s <= t {
                best = Some((k, s, o));
            }
            if first.is_none_or(|(_, fs, _)| s < fs) {
                first = Some((k, s, o));
            }
        }
        match (best, first) {
            (Some((k, _, o)), _) => Some((k, o, true)),
            (None, Some((k, _, o))) => Some((k, o, false)),
            (None, None) => None,
        }
    }

    /// One property's value at `t`; `rest` without tracks.
    pub fn number(&self, list: &[u32], t: f64, rest: f64, depth: u8) -> f64 {
        if list.is_empty() {
            return rest;
        }
        let Some((k, o, started)) = self.ruling(list, t) else {
            return rest;
        };
        let Some(tr) = self.track(list[k]) else {
            return rest;
        };
        let at = if started {
            local(tr, t - o)
        } else {
            secs(tr.start_us())
        };
        let Some((a, b, p)) = segment(&tr.keys, us(at)) else {
            return rest;
        };
        let va = self.key(list, k, a, o, rest, depth);
        if a == b || p == 0.0 {
            return va;
        }
        let vb = self.key(list, k, b, o, rest, depth);
        va + (vb - va) * p
    }

    /// A key's value: its own, or for the chained key the value the tracks before it give at its moment.
    fn key(&self, list: &[u32], k: usize, ki: usize, o: f64, rest: f64, depth: u8) -> f64 {
        let Some(tr) = self.track(list[k]) else {
            return rest;
        };
        let Some(key) = tr.keys.get(ki) else {
            return rest;
        };
        if tr.chain != Some(ki) {
            return key.v;
        }
        if depth >= CHAIN_DEPTH {
            return rest;
        }
        self.number(&list[..k], secs(key.t_us) + o, rest, depth + 1)
    }

    /// A text index at `t` (0, the node's own, without tracks): texts switch at their keys.
    pub fn text(&self, list: &[u32], t: f64, depth: u8) -> f64 {
        if list.is_empty() {
            return 0.0;
        }
        let Some((k, o, started)) = self.ruling(list, t) else {
            return 0.0;
        };
        let Some(tr) = self.track(list[k]) else {
            return 0.0;
        };
        let at = if started {
            local(tr, t - o)
        } else {
            secs(tr.start_us())
        };
        let Some((a, b, p)) = segment(&tr.keys, us(at)) else {
            return 0.0;
        };
        let ki = if p >= 1.0 { b } else { a };
        match tr.keys.get(ki) {
            Some(key) if tr.chain == Some(ki) && depth < CHAIN_DEPTH => {
                self.text(&list[..k], secs(key.t_us) + o, depth + 1)
            }
            Some(key) if tr.chain != Some(ki) => key.v,
            _ => 0.0,
        }
    }

    /// The clip at `t`, `own` without tracks: clips written in different units ease into each other.
    pub fn clip(&self, list: &[u32], t: f64, own: &ClipState, depth: u8) -> ClipState {
        if list.is_empty() {
            return *own;
        }
        let Some((k, o, started)) = self.ruling(list, t) else {
            return *own;
        };
        let Some(tr) = self.track(list[k]) else {
            return *own;
        };
        let at = if started {
            local(tr, t - o)
        } else {
            secs(tr.start_us())
        };
        let Some((a, b, p)) = segment(&tr.keys, us(at)) else {
            return *own;
        };
        let clip_at = |ki: usize| -> ClipState {
            match tr.keys.get(ki) {
                Some(key) if tr.chain == Some(ki) => {
                    if depth < CHAIN_DEPTH {
                        self.clip(&list[..k], secs(key.t_us) + o, own, depth + 1)
                    } else {
                        *own
                    }
                }
                Some(key) => self.inst.clip_of(key.v, own),
                None => *own,
            }
        };
        let ca = clip_at(a);
        if a == b || p == 0.0 {
            return ca;
        }
        ClipState::mix(&ca, &clip_at(b), p)
    }
}
