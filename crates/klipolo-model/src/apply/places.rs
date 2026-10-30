//! Where one batch finds its clips and layers: ids are hashed once into an index, so an op finds
//! its clip or layer in O(1) instead of scanning the timeline, and a batch of n ops on a big project costs O(n), not
//! O(n · (clips + layers)). Every hit is checked against the id at that place, and a stale hit (the layer moved) or a
//! slot two ids share falls back to the scan it replaced. A miss means the id isn't there, because each index holds
//! every id: so it can't give an answer the scan wouldn't. Debug tests check both indexes after every op.
//!
//! - **Clips** by id: built from the main track and dropped by every op that adds, removes or reorders clips, so a
//!   present index is exact (first clip with each id, as the scan finds it).
//! - **Layers** by id: built from the tracks and complete from then on (every op that adds a layer id outside a main-track
//!   edit registers it, and main-track edits drop it); entries go stale when layers shift, and a batch whose tracks hold
//!   one id twice falls back to scans, since the scan's "first one" then depends on where each copy sits.
//! - **Clip starts**: the main track's prefix sums, dropped when its timing changes.
//! - **Recorded footage on the tracks**, per file (video and sound layers), for `DetachSource`'s "already separate"
//!   test.
//!
//! Memory: about 24 bytes per clip and per layer while a batch runs; nothing outlives it.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

use crate::project::Project;
use crate::query::{pick, position};
use crate::time::Us;
use crate::timeline::{Attach, Clip, ItemContent, Track};

/// A fast hash for ids (an FxHash-style multiply-rotate over 8-byte words). Its quality only decides how often two ids
/// share a slot; a shared slot costs a scan, never a wrong answer.
#[derive(Default)]
pub(crate) struct IdHasher(u64);

impl IdHasher {
    fn mix(&mut self, word: u64) {
        self.0 = (self.0.rotate_left(5) ^ word).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
}

impl Hasher for IdHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut word = [0u8; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.mix(u64::from_le_bytes(word));
        }
    }

    fn write_u64(&mut self, n: u64) {
        self.mix(n);
    }
}

type Map<V> = HashMap<u64, V, BuildHasherDefault<IdHasher>>;

fn key(id: &str) -> u64 {
    let mut h = IdHasher::default();
    h.write(id.as_bytes());
    h.mix(id.len() as u64);
    h.finish()
}

/// Lookups an index may serve by scanning after it was dropped before it is built again: a batch that changes the
/// main track (or moves layers) before every lookup keeps its scans and never pays for an index it can't use.
const SCANS_BEFORE_INDEX: u8 = 1;

/// A file's layers, sorted by start, each with the furthest end reached by it or an earlier one.
type Spans = Vec<(Us, Us)>;

#[derive(Default)]
pub(crate) struct Places {
    clips: Option<Map<usize>>,
    clip_scans: u8,
    items: Option<Map<(usize, usize)>>,
    item_scans: u8,
    /// The tracks hold an id twice: the layer index is off for the rest of the batch.
    twice: bool,
    starts: Option<Vec<Us>>,
    spans: Option<HashMap<String, Spans>>,
    /// Every track is in order (`in_order`) and every attached layer starts where its anchor plays: true from the end of a
    /// batch's first op, which runs the whole-project pass, and kept by every later op's settling.
    pub(crate) settled: bool,
}

impl Places {
    /// The index of the first clip with `id`, as `main.iter().position` finds it.
    pub(crate) fn clip(&mut self, main: &[Clip], id: &str) -> Option<usize> {
        if self.clips.is_none() {
            if self.clip_scans < SCANS_BEFORE_INDEX {
                self.clip_scans += 1;
                return main.iter().position(|c| c.id == id);
            }
            let mut map = Map::with_capacity_and_hasher(main.len(), Default::default());
            for (i, c) in main.iter().enumerate() {
                map.entry(key(&c.id)).or_insert(i);
            }
            self.clips = Some(map);
        }
        match self.clips.as_ref().and_then(|m| m.get(&key(id)).copied()) {
            Some(i) if main.get(i).is_some_and(|c| c.id == id) => Some(i),
            // Two ids in one slot: the scan decides.
            Some(_) => main.iter().position(|c| c.id == id),
            // The index holds every clip's id (it's dropped whenever one is added), so an id it lacks isn't there.
            None => None,
        }
    }

    /// The main track's clips were added, removed or reordered.
    pub(crate) fn clips_changed(&mut self) {
        self.clips = None;
        self.clip_scans = 0;
        self.starts = None;
    }

    /// The main track's timing changed (a clip's length): its starts are stale, its order isn't.
    pub(crate) fn timing_changed(&mut self) {
        self.starts = None;
    }

    /// The start of clip `i` on the timeline, as `main_track_duration(&main[..i])` gives it.
    pub(crate) fn start(&mut self, main: &[Clip], i: usize) -> Us {
        let starts = self.starts.get_or_insert_with(|| {
            let mut t: Us = 0;
            let mut starts = Vec::with_capacity(main.len() + 1);
            for c in main {
                starts.push(t);
                t = t.saturating_add(c.duration_us());
            }
            starts.push(t);
            starts
        });
        starts[i]
    }

    /// Where `attach` plays now, as `query::locate_one` finds it: the clip and its timeline position.
    pub(crate) fn locate(&mut self, main: &[Clip], attach: &Attach) -> Option<(usize, Us)> {
        let own = self.clip(main, &attach.clip_id);
        let i = pick(main, attach, own)?;
        Some((i, position(&main[i], self.start(main, i), attach)))
    }

    /// (track, index) of the first layer with `id`, as `query::find_item` finds it.
    pub(crate) fn item(&mut self, tracks: &[Track], id: &str) -> Option<(usize, usize)> {
        if self.twice {
            return scan(tracks, id);
        }
        if self.items.is_none() {
            if self.item_scans < SCANS_BEFORE_INDEX {
                self.item_scans += 1;
                return scan(tracks, id);
            }
            self.build_items(tracks);
            if self.twice {
                return scan(tracks, id);
            }
        }
        let k = key(id);
        let hit = self.items.as_ref().and_then(|m| m.get(&k).copied());
        if let Some((t, i)) = hit {
            if at(tracks, t, i).is_some_and(|x| x == id) {
                return Some((t, i));
            }
        }
        // A stale entry (the layer moved) or two ids in one slot: scan, and remember where it is now.
        let found = scan(tracks, id);
        if let (Some(place), Some(map)) = (found, self.items.as_mut()) {
            map.insert(k, place);
        }
        found
    }

    fn build_items(&mut self, tracks: &[Track]) {
        let n = tracks.iter().map(|t| t.items.len()).sum();
        let mut map = Map::with_capacity_and_hasher(n, Default::default());
        for (t, track) in tracks.iter().enumerate() {
            for (i, item) in track.items.iter().enumerate() {
                if map.insert(key(&item.id), (t, i)).is_some() {
                    // The same id twice, or two ids in one slot: either way the index can't say which comes first.
                    self.twice = true;
                    self.items = None;
                    return;
                }
            }
        }
        self.items = Some(map);
    }

    /// Whether layers are found by scanning only (the tracks hold an id twice).
    pub(crate) fn unindexed(&self) -> bool {
        self.twice
    }

    /// A layer `id` now sits at (track, index): an op added it, or moved it there.
    pub(crate) fn put_item(&mut self, tracks: &[Track], id: &str, place: (usize, usize)) {
        let k = key(id);
        let Some(map) = self.items.as_mut() else {
            return;
        };
        if let Some(&(t, i)) = map.get(&k) {
            if (t, i) != place && at(tracks, t, i).is_some_and(|x| x == id) {
                // Another layer already has this id (a batch that adds one twice): back to scans.
                self.twice = true;
                self.items = None;
                return;
            }
        }
        map.insert(k, place);
    }

    /// Layers were added without being registered (a main-track edit cuts or splits them): the index rebuilds when
    /// next needed.
    pub(crate) fn items_changed(&mut self) {
        self.items = None;
        self.item_scans = 0;
    }

    /// Whether any clip or layer has `id` (with `Project`'s own small lists checked by the caller): O(1) while the
    /// indexes are there, a scan otherwise.
    pub(crate) fn has_clip_or_item(&mut self, p: &Project, id: &str) -> bool {
        if self.clip(&p.timeline.main, id).is_some() {
            return true;
        }
        let tracks = &p.timeline.tracks;
        if self.items.is_none() && !self.twice {
            self.build_items(tracks);
        }
        match &self.items {
            // Complete: an id it doesn't have isn't on the tracks (a slot it shares with another id is checked).
            Some(map) => match map.get(&key(id)) {
                None => false,
                Some(&(t, i)) => {
                    at(tracks, t, i).is_some_and(|x| x == id) || scan(tracks, id).is_some()
                }
            },
            None => scan(tracks, id).is_some(),
        }
    }

    /// Whether a video or sound layer of `file` plays anywhere in `[start, end)`: O(log layers) once the spans are built
    /// (the first `DetachSource` of a run builds them in one pass).
    pub(crate) fn file_shown(&mut self, tracks: &[Track], file: &str, start: Us, end: Us) -> bool {
        let spans = self.spans.get_or_insert_with(|| {
            let mut spans: HashMap<String, Spans> = HashMap::new();
            for item in tracks.iter().flat_map(|t| &t.items) {
                if let ItemContent::Video { asset_id, .. } | ItemContent::Audio { asset_id, .. } =
                    &item.content
                {
                    spans
                        .entry(asset_id.clone())
                        .or_default()
                        .push((item.start_us, item.end_us()));
                }
            }
            for list in spans.values_mut() {
                list.sort_by_key(|s| s.0);
                furthest_from(list, 0);
            }
            spans
        });
        let Some(list) = spans.get(file) else {
            return false;
        };
        let before_end = list.partition_point(|s| s.0 < end);
        before_end.checked_sub(1).is_some_and(|k| list[k].1 > start)
    }

    /// `DetachSource` added a layer of `file` over `[start, end)` (it keeps those times: its anchor plays it there).
    pub(crate) fn file_added(&mut self, file: &str, start: Us, end: Us) {
        let Some(spans) = self.spans.as_mut() else {
            return;
        };
        let list = spans.entry(file.to_owned()).or_default();
        let at = list.partition_point(|s| s.0 <= start);
        list.insert(at, (start, end));
        furthest_from(list, at);
    }

    /// Any op but `DetachSource` may move video and sound layers: the spans are rebuilt when next needed.
    pub(crate) fn layers_moved(&mut self) {
        self.spans = None;
    }

    /// Tests only: the indexes agree with the project (each present index exact or complete, as its doc says).
    #[cfg(all(feature = "verify-normalize", debug_assertions))]
    pub(crate) fn verify(&self, p: &Project) {
        if let Some(map) = &self.clips {
            for (i, c) in p.timeline.main.iter().enumerate() {
                let first = p.timeline.main.iter().position(|x| x.id == c.id) == Some(i);
                assert!(
                    !first || map.get(&key(&c.id)) == Some(&i),
                    "clip {} isn't indexed at {i}",
                    c.id
                );
            }
        }
        if let Some(map) = &self.items {
            for item in p.timeline.tracks.iter().flat_map(|t| &t.items) {
                assert!(
                    map.contains_key(&key(&item.id)),
                    "layer {} isn't indexed",
                    item.id
                );
            }
        }
    }
}

/// Each span's second value becomes the furthest end reached by it or any earlier one, from `from` on.
fn furthest_from(list: &mut [(Us, Us)], from: usize) {
    let mut furthest = from.checked_sub(1).map_or(Us::MIN, |k| list[k].1);
    for span in &mut list[from..] {
        furthest = furthest.max(span.1);
        span.1 = furthest;
    }
}

fn at(tracks: &[Track], t: usize, i: usize) -> Option<&str> {
    tracks
        .get(t)
        .and_then(|track| track.items.get(i))
        .map(|item| item.id.as_str())
}

fn scan(tracks: &[Track], id: &str) -> Option<(usize, usize)> {
    tracks.iter().enumerate().find_map(|(t, track)| {
        track
            .items
            .iter()
            .position(|item| item.id == id)
            .map(|i| (t, i))
    })
}
