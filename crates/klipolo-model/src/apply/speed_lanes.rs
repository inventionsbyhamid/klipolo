//! A run of speed changes that bumps layers, with exactly the op-by-op result in O(tracks · log layers) per op, plus
//! O(log layers) per free layer that may meet a moved one. Applied one by one, each op
//! runs the whole pass: every layer's start is derived again, every track is looked at, and a bump moves every later
//! layer of its track (816 bytes each). So a run of n costs O(n · layers) whatever it bumps: 0.8 s for "speed every
//! clip up 4×" at 1,500 clips when each clip's titles collide.
//!
//! Here the run plays out on lanes of 4-byte handles, one per track, and an anchored layer's start is worked out when
//! needed from a sum tree of the clips' lengths. An op looks only at the neighbours its clip can move apart or
//! together (on each track, the layers in that clip and the pair across it), at the free layers a moved layer can now
//! reach, and at layers bumped in its own pass, and the project's tracks are rebuilt once, at the end.
//!
//! A lane keeps its anchored layers and its free layers in two lists, each in the order its layers start in: anchored
//! layers in clip and source order (an anchored layer never starts after one in a later clip, or later in the same
//! clip), free ones by their start, which a speed change never moves. The track's order is the two merged by start.
//!
//! Each op's pass is the whole-project normalise's, step for step:
//! - tracks are visited bottom to top;
//! - a track out of order is swept, keeping each layer that doesn't collide with the last one kept. It is sorted first,
//!   which the merged lists already are. Layers that only moved past each other without colliding are re-sorted and
//!   nothing is bumped: the merge gives that order by itself;
//! - each bumped layer, anchored or free, goes to the first track above of its kind, unlocked and visible, where it
//!   fits, or to a new track right above.
//!
//! Which pairs can collide after clip c changed (the track was in order before: no two of its layers overlapped by more
//! than the tolerance, so none did, and two neighbours collide whenever any two layers do):
//! - two anchored layers, only around clip c: layers before it didn't move, and layers after it moved together;
//! - a free layer and an anchored one, only when the free one reaches past the first layer anchored in clip c or
//!   later: every anchored layer that moved starts there or later (a lane with none is skipped). Its neighbours among
//!   the moved layers are found by binary search;
//! - two free layers, never: neither moved;
//! - a layer bumped onto the lane in this pass, never: it went where it fits.
//!
//! It takes the runs it can play exactly: anchored layers in clip and source order on each track; no layer as short as
//! the overlap tolerance; and a separated camera anchored in its own clip. When something else comes up (two layers at
//! one moment out of clip order, a free layer and an anchored one starting at the same moment, which the whole pass
//! orders by where they were before), the op that hit it is undone on the lanes and laid out by the whole pass, and
//! the rest of the run goes one by one.

use std::collections::HashMap;

use super::main_ops::update_clip_at;
use super::places::Places;
use super::{default_name, fresh_id, whole_indexed};
use crate::ops::Op;
use crate::project::Project;
use crate::query::pick;
use crate::time::{scaled, Us, OVERLAP_TOLERANCE_US};
use crate::timeline::{Item, Track, TrackKind};

/// A layer on the lanes: anchored at a source `offset` into a clip, or free at `start`.
#[derive(Clone, Copy)]
struct Layer {
    clip: Option<usize>,
    offset: Us,
    start: Us,
    len: Us,
    /// Where the item is in the project's tracks, which the run leaves alone until the end.
    at: (usize, usize),
}

/// Which of a lane's two lists a layer is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum List {
    Anchored,
    Free,
}

/// A layer where it is on its lane: its list, its index there, and its handle.
#[derive(Clone, Copy, Debug)]
struct Slot {
    list: List,
    index: usize,
    h: u32,
}

impl Slot {
    fn anchored(index: usize, h: u32) -> Self {
        Self {
            list: List::Anchored,
            index,
            h,
        }
    }

    fn free(index: usize, h: u32) -> Self {
        Self {
            list: List::Free,
            index,
            h,
        }
    }
}

struct Lane {
    /// Stable through the run, as lanes are added.
    uid: usize,
    /// The project's track it is, or None for a track the run adds.
    from: Option<usize>,
    id: String,
    name: String,
    kind: TrackKind,
    locked: bool,
    hidden: bool,
    /// Its anchored layers, in clip and source order, which is the order they start in.
    layers: Vec<u32>,
    /// Its free layers, in the order they start in.
    frees: Vec<u32>,
}

impl Lane {
    fn list_mut(&mut self, list: List) -> &mut Vec<u32> {
        match list {
            List::Anchored => &mut self.layers,
            List::Free => &mut self.frees,
        }
    }
}

/// The clips' lengths, summed by a Fenwick tree: a clip's start, and a length change, in O(log clips).
struct Sums {
    tree: Vec<Us>,
}

impl Sums {
    fn new(lengths: &[Us]) -> Self {
        let n = lengths.len();
        let mut tree = vec![0; n + 1];
        for (k, &len) in lengths.iter().enumerate() {
            let i = k + 1;
            tree[i] += len;
            let parent = i + (i & i.wrapping_neg());
            if parent <= n {
                tree[parent] += tree[i];
            }
        }
        Self { tree }
    }

    fn add(&mut self, k: usize, delta: Us) {
        let mut i = k + 1;
        while i < self.tree.len() {
            self.tree[i] += delta;
            i += i & i.wrapping_neg();
        }
    }

    /// The length of the clips before clip `k`.
    fn before(&self, k: usize) -> Us {
        let (mut i, mut sum) = (k, 0);
        while i > 0 {
            sum += self.tree[i];
            i -= i & i.wrapping_neg();
        }
        sum
    }
}

/// A change to the lanes in the current op's pass, to undo it when the pass can't go on.
enum Undo {
    Removed(usize, List, usize, u32),
    Inserted(usize, List, usize),
    NewLane(usize),
}

/// The pass can't be played on the lanes.
struct Stop;

struct Lanes {
    layers: Vec<Layer>,
    lanes: Vec<Lane>,
    next_uid: usize,
    sums: Sums,
    speeds: Vec<f64>,
    lengths: Vec<Us>,
    /// Each run clip's separated camera layers: their lengths follow its speed.
    cameras: HashMap<usize, Vec<u32>>,
    /// The clip whose op is being played.
    current: usize,
}

/// Plays the run `ops` (speed changes on `clips`, a settled project) on lanes. Returns how many ops it applied, each
/// with exactly its op-by-op result (at least one), or None, changing nothing, when the lanes can't hold the project.
pub(super) fn run(
    p: &mut Project,
    ops: &[Op],
    clips: &[usize],
    cameras: &HashMap<usize, Vec<(usize, usize)>>,
    places: &mut Places,
) -> Option<usize> {
    let mut lanes = Lanes::of(p, cameras, places)?;
    #[cfg(test)]
    super::tests::lane_event(0);
    for (n, (op, &c)) in ops.iter().zip(clips).enumerate() {
        let Op::UpdateClip { patch, .. } = op else {
            return Some(lanes.finish(p, places, n));
        };
        let at = cameras.get(&c).map_or(&[][..], Vec::as_slice);
        let was = (lanes.lengths[c], lanes.speeds[c]);
        update_clip_at(p, c, patch, Some(at));
        lanes.clip_changed(p, c);
        if lanes.pass(p, c, places).is_err() {
            #[cfg(test)]
            super::tests::lane_event(1);
            // Laid out by the whole pass, from the tracks as the op before left them: its lists merged by the starts
            // before the op (by the new ones, a free layer and an anchored one could swap).
            lanes.clip_back(c, was);
            lanes.finish(p, places, n);
            whole_indexed(p, places);
            return Some(n + 1);
        }
    }
    Some(lanes.finish(p, places, ops.len()))
}

impl Lanes {
    fn of(
        p: &Project,
        cameras: &HashMap<usize, Vec<(usize, usize)>>,
        places: &mut Places,
    ) -> Option<Self> {
        let main = &p.timeline.main;
        let mut layers = Vec::new();
        let mut lanes = Vec::with_capacity(p.timeline.tracks.len());
        let mut first = Vec::with_capacity(p.timeline.tracks.len());
        for (t, track) in p.timeline.tracks.iter().enumerate() {
            first.push(layers.len());
            let (mut anchored, mut frees) = (Vec::new(), Vec::new());
            let mut prev: Option<(usize, Us)> = None;
            for (i, item) in track.items.iter().enumerate() {
                if item.duration_us <= OVERLAP_TOLERANCE_US {
                    return None;
                }
                let h = u32::try_from(layers.len()).ok()?;
                let layer = match &item.attach {
                    None => {
                        frees.push(h);
                        Layer {
                            clip: None,
                            offset: 0,
                            start: item.start_us,
                            len: item.duration_us,
                            at: (t, i),
                        }
                    }
                    Some(a) => {
                        let c = pick(main, a, places.clip(main, &a.clip_id))?;
                        let offset = a.source_us.saturating_sub(main[c].source_in_us);
                        if prev.is_some_and(|k| k > (c, offset)) {
                            return None;
                        }
                        prev = Some((c, offset));
                        anchored.push(h);
                        Layer {
                            clip: Some(c),
                            offset,
                            start: 0,
                            len: item.duration_us,
                            at: (t, i),
                        }
                    }
                };
                layers.push(layer);
            }
            lanes.push(Lane {
                uid: t,
                from: Some(t),
                id: track.id.clone(),
                name: track.name.clone(),
                kind: track.kind,
                locked: track.locked,
                hidden: track.hidden,
                layers: anchored,
                frees,
            });
        }
        let mut camera_handles = HashMap::new();
        for (&c, at) in cameras {
            let mut handles = Vec::with_capacity(at.len());
            for &(t, i) in at {
                let h = first[t] + i;
                // Its length follows clip c, so it has to sit in clip c for c's neighbours to cover it.
                if layers[h].clip != Some(c) {
                    return None;
                }
                handles.push(h as u32);
            }
            camera_handles.insert(c, handles);
        }
        let lengths: Vec<Us> = main.iter().map(|c| c.duration_us()).collect();
        Some(Self {
            layers,
            next_uid: lanes.len(),
            lanes,
            sums: Sums::new(&lengths),
            speeds: main.iter().map(|c| c.speed).collect(),
            lengths,
            cameras: camera_handles,
            current: 0,
        })
    }

    /// Where layer `h` starts now: an anchored layer where its anchor plays (`query::position`), a free one where it is.
    fn start(&self, h: u32) -> Us {
        let l = &self.layers[h as usize];
        match l.clip {
            Some(c) => self
                .sums
                .before(c)
                .saturating_add(scaled(l.offset, self.speeds[c])),
            None => l.start,
        }
    }

    fn end(&self, h: u32) -> Us {
        self.start(h).saturating_add(self.layers[h as usize].len)
    }

    /// The clip-and-source order the lanes keep anchored layers in.
    fn key(&self, h: u32) -> (usize, Us) {
        let l = &self.layers[h as usize];
        (l.clip.unwrap_or(usize::MAX), l.offset)
    }

    /// `collides` (whether `later` starts more than the tolerance before `earlier` ends).
    fn collide(&self, earlier: u32, later: u32) -> bool {
        self.start(later).saturating_add(OVERLAP_TOLERANCE_US) < self.end(earlier)
    }

    /// The project's clip `c` has a new speed and length; its camera layers new lengths.
    fn clip_changed(&mut self, p: &Project, c: usize) {
        let clip = &p.timeline.main[c];
        let len = clip.duration_us();
        self.sums.add(c, len - self.lengths[c]);
        self.lengths[c] = len;
        self.speeds[c] = clip.speed;
        for &h in self.cameras.get(&c).map_or(&[][..], Vec::as_slice) {
            let (t, i) = self.layers[h as usize].at;
            self.layers[h as usize].len = p.timeline.tracks[t].items[i].duration_us;
        }
    }

    /// Clip `c`'s length and speed back to `was` (its camera layers' lengths aside: no start depends on them).
    fn clip_back(&mut self, c: usize, (len, speed): (Us, f64)) {
        self.sums.add(c, len - self.lengths[c]);
        self.lengths[c] = len;
        self.speeds[c] = speed;
    }

    /// The pairs of neighbours among lane `i`'s anchored layers (by the first one's index) that clip `c`'s new length or
    /// speed can have moved together, and those around anchored layers bumped onto it in this pass: every other pair
    /// of them is as the last pass left it, in order.
    fn suspects(&self, i: usize, c: usize, received: &[(usize, u32)]) -> Vec<usize> {
        let lane = &self.lanes[i];
        let n = lane.layers.len();
        if n < 2 {
            return Vec::new();
        }
        let clip = |h: &u32| self.layers[*h as usize].clip.unwrap_or(usize::MAX);
        let lo = lane.layers.partition_point(|h| clip(h) < c);
        let hi = lane.layers.partition_point(|h| clip(h) <= c);
        let mut ks: Vec<usize> = (lo.saturating_sub(1)..hi.min(n - 1)).collect();
        for &(uid, h) in received {
            if uid != lane.uid || self.layers[h as usize].clip.is_none() {
                continue;
            }
            let key = self.key(h);
            let from = lane.layers.partition_point(|&o| self.key(o) < key);
            let Some(m) = lane.layers[from..]
                .iter()
                .position(|&o| o == h)
                .map(|m| m + from)
            else {
                continue;
            };
            if m > 0 {
                ks.push(m - 1);
            }
            if m + 1 < n {
                ks.push(m);
            }
        }
        ks.sort_unstable();
        ks.dedup();
        ks
    }

    /// The pairs of lane `i`'s layers that collide after clip `c` changed, each (earlier, later) in the order they start
    /// in. Every pair of neighbours that collides is among them (the module doc says why the rest can't). A free layer
    /// and an anchored one starting at the same moment stop the pass: the whole pass orders them by where they were.
    fn collisions(
        &self,
        i: usize,
        c: usize,
        received: &[(usize, u32)],
    ) -> Result<Vec<(Slot, Slot)>, Stop> {
        let lane = &self.lanes[i];
        let mut pairs = Vec::new();
        for k in self.suspects(i, c, received) {
            let (a, b) = (lane.layers[k], lane.layers[k + 1]);
            if self.collide(a, b) {
                pairs.push((Slot::anchored(k, a), Slot::anchored(k + 1, b)));
            }
        }
        if lane.frees.is_empty() || lane.layers.is_empty() {
            return Ok(pairs);
        }
        // Only a layer that moved can meet a free one: those anchored in clip c or later, which start no earlier than
        // the first of them (`moved`, by binary search: an index into the lane's anchored list). Free layers end in the
        // order they start (they never overlap), so those reaching past it are a suffix of the list, and each one's
        // neighbours among the moved layers are found by binary search in them. A lane none of whose anchored layers
        // moved (layers bumped onto it by earlier ops, a free layer's neighbours there) is skipped at once.
        let clip = |h: &u32| self.layers[*h as usize].clip.unwrap_or(usize::MAX);
        let moved = lane.layers.partition_point(|h| clip(h) < c);
        let Some(&first_moved) = lane.layers.get(moved) else {
            return Ok(pairs);
        };
        let reach = self.start(first_moved).saturating_add(OVERLAP_TOLERANCE_US);
        let first = lane.frees.partition_point(|&f| self.end(f) <= reach);
        for (m, &f) in lane.frees.iter().enumerate().skip(first) {
            #[cfg(test)]
            super::tests::lane_event(5);
            let s = self.start(f);
            let at = moved + lane.layers[moved..].partition_point(|&a| self.start(a) <= s);
            // Its neighbour before it, when that one moved (one that didn't can't meet it, or start with it, anew).
            if at > moved {
                let a = lane.layers[at - 1];
                if self.start(a) == s {
                    return Err(Stop);
                }
                if self.collide(a, f) {
                    pairs.push((Slot::anchored(at - 1, a), Slot::free(m, f)));
                }
            }
            if let Some(&a) = lane.layers.get(at) {
                if self.collide(f, a) {
                    pairs.push((Slot::free(m, f), Slot::anchored(at, a)));
                }
            }
        }
        Ok(pairs)
    }

    /// Where `slot` is among all of lane `i`'s layers, in the order they start in (no free layer starts with an
    /// anchored one: `collisions` stops the pass first).
    fn position(&self, i: usize, slot: Slot) -> usize {
        let lane = &self.lanes[i];
        let s = self.start(slot.h);
        slot.index
            + match slot.list {
                List::Anchored => lane.frees.partition_point(|&f| self.start(f) < s),
                List::Free => lane.layers.partition_point(|&a| self.start(a) < s),
            }
    }

    /// The layer that starts next on lane `i` after `ai` of its anchored layers and `fi` of its free ones.
    fn next_at(&self, i: usize, ai: usize, fi: usize) -> Result<Option<Slot>, Stop> {
        let lane = &self.lanes[i];
        Ok(match (lane.layers.get(ai), lane.frees.get(fi)) {
            (None, None) => None,
            (Some(&a), None) => Some(Slot::anchored(ai, a)),
            (None, Some(&f)) => Some(Slot::free(fi, f)),
            (Some(&a), Some(&f)) => match self.start(a).cmp(&self.start(f)) {
                std::cmp::Ordering::Less => Some(Slot::anchored(ai, a)),
                std::cmp::Ordering::Greater => Some(Slot::free(fi, f)),
                std::cmp::Ordering::Equal => return Err(Stop),
            },
        })
    }

    /// One op's whole pass on the lanes, after clip `c` changed. On `Stop` the lanes are as before it.
    fn pass(&mut self, p: &Project, c: usize, places: &mut Places) -> Result<(), Stop> {
        self.current = c;
        let mut undo: Vec<Undo> = Vec::new();
        let mut received: Vec<(usize, u32)> = Vec::new();
        let mut i = 0;
        while i < self.lanes.len() {
            let swept = self.collisions(i, c, &received).and_then(|pairs| {
                if pairs.is_empty() {
                    return Ok(());
                }
                self.sweep(p, i, &pairs, places, &mut undo, &mut received)
            });
            if let Err(stop) = swept {
                self.undo(undo);
                return Err(stop);
            }
            i += 1;
        }
        Ok(())
    }

    /// `bump_overlaps` on lane `i`, whose only colliding neighbours are among `pairs`. From the first pair's earlier
    /// layer (every layer before it is in order, so kept), each layer that collides with the last one kept is bumped,
    /// until past the last pair a layer is kept: the layers after it are in order behind it. Between pairs far apart the
    /// sweep jumps ahead, as all the layers it passes over would be kept.
    fn sweep(
        &mut self,
        p: &Project,
        i: usize,
        pairs: &[(Slot, Slot)],
        places: &mut Places,
        undo: &mut Vec<Undo>,
        received: &mut Vec<(usize, u32)>,
    ) -> Result<(), Stop> {
        // Each pair by its two layers' places in the track's order, and its earlier layer.
        let mut spans: Vec<(usize, usize, Slot)> = pairs
            .iter()
            .map(|&(a, b)| (self.position(i, a), self.position(i, b), a))
            .collect();
        spans.sort_unstable_by_key(|&(from, ..)| from);
        let mut bumped: Vec<Slot> = Vec::new();
        let mut n = 0;
        while n < spans.len() {
            let (at, mut last, first) = spans[n];
            let (mut ai, mut fi) = match first.list {
                List::Anchored => (first.index + 1, at - first.index),
                List::Free => (at - first.index, first.index + 1),
            };
            let (mut kept, mut m) = (first.h, at);
            'sweep: loop {
                // The pairs that start where the sweep has reached are swept with it.
                while n < spans.len() && spans[n].0 <= m {
                    last = last.max(spans[n].1);
                    n += 1;
                }
                if m >= last {
                    break;
                }
                loop {
                    let Some(next) = self.next_at(i, ai, fi)? else {
                        break 'sweep;
                    };
                    match next.list {
                        List::Anchored => ai += 1,
                        List::Free => fi += 1,
                    }
                    m += 1;
                    if self.collide(kept, next.h) {
                        bumped.push(next);
                    } else {
                        kept = next.h;
                        break;
                    }
                }
            }
            // The lane ran out: the pairs left were swept with it.
            while n < spans.len() && spans[n].0 <= m {
                n += 1;
            }
        }
        #[cfg(test)]
        {
            if !bumped.is_empty() {
                super::tests::lane_event(2);
            }
            if pairs
                .iter()
                .any(|(a, b)| a.list == List::Free || b.list == List::Free)
            {
                super::tests::lane_event(4);
            }
        }
        let uid = self.lanes[i].uid;
        for list in [List::Anchored, List::Free] {
            for slot in bumped.iter().rev().filter(|s| s.list == list) {
                self.lanes[i].list_mut(list).remove(slot.index);
                undo.push(Undo::Removed(uid, list, slot.index, slot.h));
            }
        }
        for slot in &bumped {
            self.place_above(p, i, slot.h, places, undo, received)?;
        }
        Ok(())
    }

    fn index_of(&self, uid: usize) -> usize {
        self.lanes
            .iter()
            .position(|l| l.uid == uid)
            .expect("a lane of this run")
    }

    /// `place_above` from a bump: the first lane above `i` of its kind, unlocked and visible, where `h` fits (`fits`,
    /// by binary search on a lane in order), else a new lane right above, named and with an id as a new track gets
    /// them (its id avoids every id in the project, the layers this sweep bumped among them).
    fn place_above(
        &mut self,
        p: &Project,
        i: usize,
        h: u32,
        places: &mut Places,
        undo: &mut Vec<Undo>,
        received: &mut Vec<(usize, u32)>,
    ) -> Result<(), Stop> {
        let kind = self.lanes[i].kind;
        let (start, end) = (self.start(h), self.end(h));
        let anchored = self.layers[h as usize].clip.is_some();
        for j in i + 1..self.lanes.len() {
            let lane = &self.lanes[j];
            if lane.kind != kind
                || lane.locked
                || lane.hidden
                || !self.fits(j, start, end, received)?
            {
                continue;
            }
            let (list, at) = if anchored {
                let at = lane.layers.partition_point(|&o| self.start(o) <= start);
                let key = self.key(h);
                let ordered = (at == 0 || self.key(lane.layers[at - 1]) <= key)
                    && lane.layers.get(at).is_none_or(|&o| key <= self.key(o));
                if !ordered {
                    return Err(Stop);
                }
                (List::Anchored, at)
            } else {
                (
                    List::Free,
                    lane.frees.partition_point(|&o| self.start(o) <= start),
                )
            };
            let uid = lane.uid;
            self.lanes[j].list_mut(list).insert(at, h);
            undo.push(Undo::Inserted(uid, list, at));
            received.push((uid, h));
            return Ok(());
        }
        let name = default_name(kind, |n| self.lanes.iter().any(|l| l.name == n));
        let (t, k) = self.layers[h as usize].at;
        let base = format!("{}-track", p.timeline.tracks[t].items[k].id);
        let id = fresh_id(base, |id| {
            p.id == id
                || p.assets.iter().any(|a| a.id == id)
                || p.recordings.iter().any(|r| r.id == id)
                || p.luts.iter().any(|l| l.id == id)
                || self.lanes.iter().any(|l| l.id == id)
                || places.has_clip_or_item(p, id)
        });
        let uid = self.next_uid;
        self.next_uid += 1;
        let (layers, frees) = if anchored {
            (vec![h], Vec::new())
        } else {
            (Vec::new(), vec![h])
        };
        self.lanes.insert(
            i + 1,
            Lane {
                uid,
                from: None,
                id,
                name,
                kind,
                locked: false,
                hidden: false,
                layers,
                frees,
            },
        );
        #[cfg(test)]
        super::tests::lane_event(3);
        undo.push(Undo::NewLane(uid));
        received.push((uid, h));
        Ok(())
    }

    /// `fits` on lane `j` (nothing ignored): by binary search in each of its lists when the lane is in order now (no
    /// two of its layers collide), else layer by layer.
    fn fits(&self, j: usize, start: Us, end: Us, received: &[(usize, u32)]) -> Result<bool, Stop> {
        if end <= start {
            return Ok(false);
        }
        // Its collisions for the op being played are worked out by `pass` when it gets there; here, whether it has any.
        let in_order = self.collisions(j, self.current, received)?.is_empty();
        let lane = &self.lanes[j];
        if in_order {
            return Ok(self.fits_in_order(&lane.layers, start, end)
                && self.fits_in_order(&lane.frees, start, end));
        }
        Ok(lane.layers.iter().chain(&lane.frees).all(|&o| {
            let s = self.start(o);
            if start <= s {
                end <= s.saturating_add(OVERLAP_TOLERANCE_US)
            } else {
                start.saturating_add(OVERLAP_TOLERANCE_US) >= self.end(o)
            }
        }))
    }

    /// `fits_in_order` on one list of a lane in order: of the layers starting at or after `start`, the first bounds
    /// `end`; of those starting before it, only the last can reach past `start`.
    fn fits_in_order(&self, layers: &[u32], start: Us, end: Us) -> bool {
        let at = layers.partition_point(|&o| self.start(o) < start);
        let after = layers
            .get(at)
            .is_none_or(|&o| end <= self.start(o).saturating_add(OVERLAP_TOLERANCE_US));
        let before = at
            .checked_sub(1)
            .is_none_or(|b| start.saturating_add(OVERLAP_TOLERANCE_US) >= self.end(layers[b]));
        after && before
    }

    fn undo(&mut self, undo: Vec<Undo>) {
        for step in undo.into_iter().rev() {
            match step {
                Undo::Removed(uid, list, k, h) => {
                    let i = self.index_of(uid);
                    self.lanes[i].list_mut(list).insert(k, h);
                }
                Undo::Inserted(uid, list, k) => {
                    let i = self.index_of(uid);
                    self.lanes[i].list_mut(list).remove(k);
                }
                Undo::NewLane(uid) => {
                    let i = self.index_of(uid);
                    self.lanes.remove(i);
                }
            }
        }
    }

    /// The project's tracks as the lanes have them: each lane's two lists merged by start (no two of its layers start
    /// at one moment), each anchored layer starting where its anchor plays (the last pass's `follow`). Returns `done`.
    fn finish(self, p: &mut Project, places: &mut Places, done: usize) -> usize {
        let old = std::mem::take(&mut p.timeline.tracks);
        let mut shells: Vec<Option<Track>> = Vec::with_capacity(old.len());
        let mut items: Vec<Vec<Option<Item>>> = Vec::with_capacity(old.len());
        for mut track in old {
            items.push(
                std::mem::take(&mut track.items)
                    .into_iter()
                    .map(Some)
                    .collect(),
            );
            shells.push(Some(track));
        }
        let mut tracks = Vec::with_capacity(self.lanes.len());
        for lane in &self.lanes {
            let mut track = match lane.from {
                Some(t) => shells[t].take().expect("each track once"),
                None => {
                    let mut track = Track::new(lane.kind, lane.name.clone());
                    track.id = lane.id.clone();
                    track
                }
            };
            let (mut a, mut f) = (0, 0);
            track.items = Vec::with_capacity(lane.layers.len() + lane.frees.len());
            while a < lane.layers.len() || f < lane.frees.len() {
                let anchored = match (lane.layers.get(a), lane.frees.get(f)) {
                    (Some(&x), Some(&y)) => self.start(x) < self.start(y),
                    (Some(_), None) => true,
                    _ => false,
                };
                let h = if anchored {
                    a += 1;
                    lane.layers[a - 1]
                } else {
                    f += 1;
                    lane.frees[f - 1]
                };
                let layer = &self.layers[h as usize];
                let mut item = items[layer.at.0][layer.at.1]
                    .take()
                    .expect("each layer once");
                if layer.clip.is_some() {
                    item.start_us = self.start(h);
                }
                track.items.push(item);
            }
            tracks.push(track);
        }
        p.timeline.tracks = tracks;
        places.timing_changed();
        places.items_changed();
        places.layers_moved();
        done
    }
}
