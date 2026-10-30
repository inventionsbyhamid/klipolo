//! Runs of speed changes in one batch ("speed up every clip"). Each changes the main track's timing, so applied one by
//! one every op re-lays the whole project out (`normalize`: O(clips + layers), with an index of every clip), and a run
//! of n costs O(n · (clips + layers)): 1,500 clips took about 200 ms.
//!
//! The layouts between the ops only matter if one of them would bump a layer onto another track (a collision in a
//! state the run passes through): the starts they derive are derived again from the anchors by the next pass, and the
//! speed changes read nothing a pass writes. So a run applies its ops and lays out once, when a check proves that no
//! state it passes through has a collision, which is exactly when that is what applying them one by one gives. The
//! check is O(clips + layers + run): on a track, the gap between neighbours is a sum of terms each depending on one
//! clip's speed (the lengths of the clips between them, and where each starts in its own clip), so its smallest value
//! over every mix of the speeds the clips pass through is the sum of each term's smallest.
//!
//! A run is planned once. When a pair of neighbours may collide, the ops before the first that changes a clip that pair
//! depends on are still laid out once (no state before it differs from the settled one for that pair), and the rest of
//! the run goes op by op, never planned again. Debug tests compare every deferred stretch with the op-by-op result
//! (`verify-normalize`).

use std::collections::HashMap;
use std::ops::RangeInclusive;

use super::main_ops::update_clip_at;
use super::places::Places;
use super::whole_indexed;
use crate::ops::Op;
use crate::project::{Project, RecordingSource};
use crate::query::pick;
use crate::time::{scaled, unscaled, Us, OVERLAP_TOLERANCE_US};
use crate::timeline::{Clip, ClipSource, Item, ItemContent};

/// The speeds the model accepts (`validate`): a run with any other goes op by op, and fails there.
const SPEEDS: RangeInclusive<f64> = 0.25..=4.0;

/// How many of `ops`, from the first, change a clip's speed.
pub(super) fn len(ops: &[Op]) -> usize {
    ops.iter()
        .take_while(|op| matches!(op, Op::UpdateClip { patch, .. } if patch.speed.is_some()))
        .count()
}

/// Plans the run `ops` (all speed changes, on a settled project) once and applies as much of it as it can without a
/// whole pass per op, each op with exactly its op-by-op result: all of it laid out once when no state it passes through
/// bumps a layer; otherwise played on lanes (`speed_lanes`), or, when the lanes can't hold the project, the ops before
/// the first that may bump laid out once. Returns how many ops it applied; the caller applies the rest one by one.
pub(super) fn apply(p: &mut Project, ops: &[Op], places: &mut Places) -> usize {
    let Some(plan) = plan(p, ops, places) else {
        return 0;
    };
    #[cfg(all(feature = "verify-normalize", debug_assertions))]
    let before = p.clone();
    let done = if plan.safe == ops.len() {
        laid_out_once(p, ops, &plan, places)
    } else {
        match super::speed_lanes::run(p, ops, &plan.clips, &plan.cameras, places) {
            Some(done) => done,
            None => laid_out_once(p, &ops[..plan.safe], &plan, places),
        }
    };
    #[cfg(all(feature = "verify-normalize", debug_assertions))]
    if done > 0 {
        let expected = one_by_one(&before, &ops[..done]);
        assert!(
            *p == expected || format!("{p:?}") == format!("{expected:?}"),
            "{done} speed changes applied as a run differ from one by one"
        );
    }
    done
}

/// The first `ops.len()` ops of the plan's run applied, then one whole pass.
fn laid_out_once(p: &mut Project, ops: &[Op], plan: &Plan, places: &mut Places) -> usize {
    if ops.is_empty() {
        return 0;
    }
    for (op, &i) in ops.iter().zip(&plan.clips) {
        if let Op::UpdateClip { patch, .. } = op {
            let cameras = plan.cameras.get(&i).map_or(&[][..], Vec::as_slice);
            update_clip_at(p, i, patch, Some(cameras));
        }
    }
    whole_indexed(p, places);
    places.layers_moved();
    ops.len()
}

/// Tests only: the run as `apply_part` applies it without deferring, a whole pass after each op.
#[cfg(all(feature = "verify-normalize", debug_assertions))]
fn one_by_one(p: &Project, ops: &[Op]) -> Project {
    let mut q = p.clone();
    for op in ops {
        super::apply_one(&mut q, op, &mut Places::default()).expect("the run's clips are there");
        // `whole`, uncounted: tests count the passes the batch itself runs.
        super::normalize(&mut q);
    }
    q
}

/// What applying a run needs: each op's clip, each clip's camera layers (the ones a new speed re-times), and how many
/// ops from the start can be laid out once.
struct Plan {
    clips: Vec<usize>,
    cameras: HashMap<usize, Vec<(usize, usize)>>,
    safe: usize,
}

/// A clip through the run: its speed and length before it, then after each op on it; the visits with a speed not seen
/// before (a term that depends on the speed alone needs only those); and the first op on it.
struct Visits {
    speeds: Vec<f64>,
    lengths: Vec<Us>,
    distinct: Vec<usize>,
    first: usize,
}

fn source_len(c: &Clip) -> Us {
    c.source_out_us.saturating_sub(c.source_in_us)
}

/// Where a layer is in every state of the run: free, or at a source moment of a clip (`offset` into it), with a length
/// that is fixed or (a separated camera's) changes with each visit of the clip it is attached to.
enum Pos<'a> {
    Free {
        start: Us,
        len: Us,
    },
    Anchored {
        clip: usize,
        offset: Us,
        len: Len<'a>,
    },
}

enum Len<'a> {
    Fixed(Us),
    /// The lengths at each visit of this clip.
    Visits(usize, &'a [Us]),
}

/// A term of one clip's share of a gap: where a layer starts in it (+ or −), or a camera layer's length (−).
enum Extra<'a> {
    Offset(Us, i64),
    Length(&'a [Us]),
}

struct Run<'a> {
    main: &'a [Clip],
    visits: HashMap<usize, Visits>,
    /// Σ over the clips before k of each clip's shortest (`lo`) and longest (`hi`) length in the run.
    lo: Vec<i64>,
    hi: Vec<i64>,
    /// The first op on each clip (`usize::MAX` for none) in ranges by powers of two, built on the first pair that may
    /// collide: the earliest op a pair depends on in O(1).
    firsts: Vec<Vec<usize>>,
}

impl Run<'_> {
    /// The visits of clip `k` a term needs: every one when a camera layer's length is in it (it follows the visits, not
    /// the speed alone), else one per speed.
    fn states(&self, k: usize, every: bool) -> Vec<usize> {
        match self.visits.get(&k) {
            None => vec![0],
            Some(v) if every => (0..v.speeds.len()).collect(),
            Some(v) => v.distinct.clone(),
        }
    }

    fn first(&self, k: usize) -> usize {
        self.visits.get(&k).map_or(usize::MAX, |v| v.first)
    }

    /// The first op on any clip in `[a, b)`.
    fn first_in(&mut self, a: usize, b: usize) -> usize {
        if a >= b {
            return usize::MAX;
        }
        if self.firsts.is_empty() {
            let base: Vec<usize> = (0..self.main.len()).map(|k| self.first(k)).collect();
            self.firsts.push(base);
            let mut width = 1;
            while 2 * width <= self.main.len() {
                let prev = &self.firsts[self.firsts.len() - 1];
                let next = (0..prev.len() - width)
                    .map(|i| prev[i].min(prev[i + width]))
                    .collect();
                self.firsts.push(next);
                width *= 2;
            }
        }
        let level = (usize::BITS - 1 - (b - a).leading_zeros()) as usize;
        let row = &self.firsts[level];
        row[a].min(row[b - (1 << level)])
    }

    /// The first op the gap between `u` and `v` depends on: one on a clip either sits in, or on a clip between them.
    fn first_for(&mut self, u: &Pos, v: &Pos) -> usize {
        let mut first = usize::MAX;
        for pos in [u, v] {
            if let Pos::Anchored { clip, len, .. } = pos {
                first = first.min(self.first(*clip));
                if let Len::Visits(x, _) = len {
                    first = first.min(self.first(*x));
                }
            }
        }
        let range = match (u, v) {
            (Pos::Anchored { clip: a, .. }, Pos::Anchored { clip: b, .. }) => {
                (*a.min(b), *a.max(b))
            }
            (Pos::Free { .. }, Pos::Anchored { clip, .. })
            | (Pos::Anchored { clip, .. }, Pos::Free { .. }) => (0, *clip),
            (Pos::Free { .. }, Pos::Free { .. }) => (0, 0),
        };
        first.min(self.first_in(range.0, range.1))
    }

    fn speed(&self, k: usize, j: usize) -> f64 {
        self.visits
            .get(&k)
            .map_or(self.main[k].speed, |v| v.speeds[j])
    }

    fn length(&self, k: usize, j: usize) -> i64 {
        self.visits
            .get(&k)
            .map_or(self.main[k].duration_us(), |v| v.lengths[j])
    }

    /// The smallest value over the run of `start(v) − start(u)`, less `u`'s length when `with_len`: a sum over the clips
    /// of a term each, the clips between the two (whole lengths, + or −) summed from `lo`/`hi`, and the clips the
    /// layers sit in (their offsets, and a camera layer's length) taken together state by state.
    fn least(&self, u: &Pos, v: &Pos, with_len: bool) -> i64 {
        let mut total: i64 = 0;
        // Lengths of whole clips: + for those before v's clip, − for those before u's.
        let (plus, minus) = match (u, v) {
            (Pos::Anchored { clip: a, .. }, Pos::Anchored { clip: b, .. }) if a < b => {
                (Some((*a, *b)), None)
            }
            (Pos::Anchored { clip: a, .. }, Pos::Anchored { clip: b, .. }) if b < a => {
                (None, Some((*b, *a)))
            }
            (Pos::Anchored { .. }, Pos::Anchored { .. }) => (None, None),
            (Pos::Free { .. }, Pos::Anchored { clip: b, .. }) => (Some((0, *b)), None),
            (Pos::Anchored { clip: a, .. }, Pos::Free { .. }) => (None, Some((0, *a))),
            (Pos::Free { .. }, Pos::Free { .. }) => (None, None),
        };
        if let Some((a, b)) = plus {
            total += self.lo[b] - self.lo[a];
        }
        if let Some((a, b)) = minus {
            total -= self.hi[b] - self.hi[a];
        }
        let coef = |k: usize| -> i64 {
            i64::from(plus.is_some_and(|(a, b)| a <= k && k < b))
                - i64::from(minus.is_some_and(|(a, b)| a <= k && k < b))
        };
        // The clips with terms of their own.
        let mut extras: Vec<(usize, Extra)> = Vec::with_capacity(3);
        match v {
            Pos::Free { start, .. } => total += start,
            Pos::Anchored { clip, offset, .. } => extras.push((*clip, Extra::Offset(*offset, 1))),
        }
        match u {
            Pos::Free { start, len } => {
                total -= start;
                if with_len {
                    total -= len;
                }
            }
            Pos::Anchored { clip, offset, len } => {
                extras.push((*clip, Extra::Offset(*offset, -1)));
                if with_len {
                    match len {
                        Len::Fixed(l) => total -= l,
                        Len::Visits(x, lens) => extras.push((*x, Extra::Length(lens))),
                    }
                }
            }
        }
        let mut done: Vec<usize> = Vec::with_capacity(3);
        for &(k, _) in &extras {
            if done.contains(&k) {
                continue;
            }
            done.push(k);
            // The clip's whole length was counted at its extreme in the sums above; here it is taken state by state.
            let c = coef(k);
            if c > 0 {
                total -= self.lo[k + 1] - self.lo[k];
            } else if c < 0 {
                total += self.hi[k + 1] - self.hi[k];
            }
            let every = extras
                .iter()
                .any(|(x, extra)| *x == k && matches!(extra, Extra::Length(_)));
            let least = self
                .states(k, every)
                .into_iter()
                .map(|j| {
                    let speed = self.speed(k, j);
                    let mut value = c * self.length(k, j);
                    for (_, extra) in extras.iter().filter(|(x, _)| *x == k) {
                        value += match extra {
                            Extra::Offset(offset, sign) => sign * scaled(*offset, speed),
                            Extra::Length(lens) => -lens[j],
                        };
                    }
                    value
                })
                .min()
                .unwrap_or(0);
            total += least;
        }
        total
    }
}

/// The run's plan, or None when none of it can be laid out once: a clip it names isn't there, or a speed is out of
/// range. `safe` is the whole run when no state it passes through has two neighbours on a track out of order (the whole
/// pass would bump one); otherwise the ops before the first one any such pair depends on.
fn plan(p: &Project, ops: &[Op], places: &mut Places) -> Option<Plan> {
    let main = &p.timeline.main;
    let mut clips = Vec::with_capacity(ops.len());
    let mut visits: HashMap<usize, Visits> = HashMap::new();
    for (n, op) in ops.iter().enumerate() {
        let Op::UpdateClip { id, patch } = op else {
            return None;
        };
        let speed = patch.speed.filter(|s| SPEEDS.contains(s))?;
        let i = places.clip(main, id)?;
        let v = visits.entry(i).or_insert_with(|| Visits {
            speeds: vec![main[i].speed],
            lengths: vec![main[i].duration_us()],
            distinct: vec![0],
            first: n,
        });
        if !v.speeds.contains(&speed) {
            v.distinct.push(v.speeds.len());
        }
        v.speeds.push(speed);
        v.lengths.push(scaled(source_len(&main[i]), speed));
        clips.push(i);
    }
    // A clip whose sound plays from separated layers keeps its speed (`main_ops::update_clip` refuses a change): the
    // run goes op by op and is refused there.
    if changes_separated_sound(p, &visits, places) {
        return None;
    }
    let (mut lo, mut hi): (Vec<i64>, Vec<i64>) = (
        Vec::with_capacity(main.len() + 1),
        Vec::with_capacity(main.len() + 1),
    );
    lo.push(0);
    hi.push(0);
    for (k, c) in main.iter().enumerate() {
        let (least, most) = visits
            .get(&k)
            .map_or((c.duration_us(), c.duration_us()), |v| {
                let least = v.lengths.iter().copied().min().unwrap_or(0);
                (least, v.lengths.iter().copied().max().unwrap_or(0))
            });
        lo.push(lo[k].saturating_add(least));
        hi.push(hi[k].saturating_add(most));
    }
    // Each visited clip's camera layers, and their lengths through its visits (`camera_layer_follows`, simulated).
    let camera_of: HashMap<usize, &RecordingSource> = visits
        .keys()
        .filter_map(|&i| Some((i, camera_file(p, &main[i])?)))
        .collect();
    let mut cameras: HashMap<usize, Vec<(usize, usize)>> = HashMap::new();
    let mut lens: HashMap<(usize, usize), (usize, Vec<Us>)> = HashMap::new();
    if !camera_of.is_empty() {
        for (t, track) in p.timeline.tracks.iter().enumerate() {
            for (k, item) in track.items.iter().enumerate() {
                let (
                    Some(a),
                    ItemContent::Video {
                        asset_id, speed, ..
                    },
                ) = (&item.attach, &item.content)
                else {
                    continue;
                };
                let Some(x) = places.clip(main, &a.clip_id) else {
                    continue;
                };
                let is_camera = camera_of
                    .get(&x)
                    .is_some_and(|s| s.runs.iter().any(|r| r.asset_id == *asset_id));
                if !is_camera {
                    continue;
                }
                cameras.entry(x).or_default().push((t, k));
                let (mut len, mut at) = (item.duration_us, *speed);
                let mut seq = vec![len];
                for &s in &visits[&x].speeds[1..] {
                    if at != s {
                        len = scaled(unscaled(len, at), s);
                        at = s;
                    }
                    seq.push(len);
                }
                lens.insert((t, k), (x, seq));
            }
        }
    }
    let mut run = Run {
        main,
        visits,
        lo,
        hi,
        firsts: Vec::new(),
    };
    let mut safe = ops.len();
    'tracks: for (t, track) in p.timeline.tracks.iter().enumerate() {
        let mut prev: Option<Pos> = None;
        for (k, item) in track.items.iter().enumerate() {
            let Some(pos) = place_of(main, places, item, lens.get(&(t, k))) else {
                // An anchor that doesn't resolve (never in a settled project): nothing is laid out once.
                safe = 0;
                break 'tracks;
            };
            if let Some(u) = &prev {
                let sorted = run.least(u, &pos, false) >= 0;
                let apart = run.least(u, &pos, true) + OVERLAP_TOLERANCE_US >= 0;
                if !(sorted && apart) {
                    safe = safe.min(run.first_for(u, &pos));
                    if safe == 0 {
                        break 'tracks;
                    }
                }
            }
            prev = Some(pos);
        }
    }
    Some(Plan {
        clips,
        cameras,
        safe,
    })
}

/// Whether the run changes the speed of a clip with sound separated over it (`main_ops::sound_separated`). Nearly always
/// one pass over the layers that finds no sound layer of a recorded file, so nothing more; else each clip whose speed
/// changes is checked. O(layers), or O(layers × changed clips) with separated sound.
fn changes_separated_sound(
    p: &Project,
    visits: &HashMap<usize, Visits>,
    places: &mut Places,
) -> bool {
    let recorded = |file: &str| {
        p.recordings.iter().any(|r| {
            r.audio_sources()
                .any(|s| s.runs.iter().any(|run| run.asset_id == file))
        })
    };
    let any =
        p.timeline.tracks.iter().flat_map(|t| &t.items).any(
            |i| matches!(&i.content, ItemContent::Audio { asset_id, .. } if recorded(asset_id)),
        );
    if !any {
        return false;
    }
    let main = &p.timeline.main;
    visits
        .iter()
        .filter(|(_, v)| v.speeds.iter().any(|s| *s != v.speeds[0]))
        .any(|(&i, _)| {
            let start = places.start(main, i);
            super::main_ops::sound_separated(p, &main[i], start, start + main[i].duration_us())
        })
}

/// A recording clip's camera source, borrowed (`main_ops::camera_assets` without the copies).
fn camera_file<'a>(p: &'a Project, clip: &Clip) -> Option<&'a RecordingSource> {
    match &clip.source {
        ClipSource::Recording { id } => p.recording(id)?.camera(),
        ClipSource::Asset { .. } => None,
    }
}

/// Where `item` is through the run: free, or anchored where its anchor resolves (`MainIndex::locate`'s rules: the speeds
/// never change which clip plays a moment).
fn place_of<'a>(
    main: &[Clip],
    places: &mut Places,
    item: &Item,
    camera: Option<&'a (usize, Vec<Us>)>,
) -> Option<Pos<'a>> {
    let len = match camera {
        Some((x, seq)) => Len::Visits(*x, seq),
        None => Len::Fixed(item.duration_us),
    };
    let Some(attach) = &item.attach else {
        return Some(Pos::Free {
            start: item.start_us,
            len: item.duration_us,
        });
    };
    let own = places.clip(main, &attach.clip_id);
    let clip = pick(main, attach, own)?;
    Some(Pos::Anchored {
        clip,
        offset: attach.source_us.saturating_sub(main[clip].source_in_us),
        len,
    })
}
