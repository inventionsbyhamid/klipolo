//! What an edit changed. The UI receives the forward patch instead of the whole project, and the undo
//! history keeps the reverse one instead of a whole copy. `apply(a, &diff(a, b)) == b` exactly. Clips and items are
//! matched by id (unique in a valid project); every other part is replaced whole when it changed. A project's `id`
//! and `version` never change through edits, so a patch doesn't carry them.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::io;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::captions::looks::WordLook;
use crate::cursor_style::CursorStyle;
use crate::error::{invalid, Result};
use crate::ids::Id;
use crate::messages::code;
use crate::project::{
    Asset, AudioSettings, Background, CaptionSettings, ColorLut, Project, Recording, Settings,
};
use crate::speech::WordFix;
use crate::timeline::{Clip, Item, Timeline, Track, TrackKind};

/// A track with its items as ids, in order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TrackShape {
    pub id: Id,
    pub kind: TrackKind,
    pub name: String,
    pub hidden: bool,
    pub muted: bool,
    pub locked: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub solo: bool,
    pub items: Vec<Id>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProjectPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub settings: Option<Settings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub assets: Option<Vec<Asset>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub recordings: Option<Vec<Recording>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub luts: Option<Vec<ColorLut>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub captions: Option<CaptionSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub background: Option<Background>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub audio: Option<AudioSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub cursor: Option<CursorStyle>,
    /// The whole list, when any word fix changed.
    // The project's own shared list, never a copy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub word_fixes: Option<Arc<Vec<WordFix>>>,
    /// The whole list, when any word's look changed: up to `MAX_WORD_LOOKS` (about 2 MB of JSON at the cap) per look
    /// edit, acceptable at click rate; a ranged patch can come later.
    // Shared with the project, as the fixes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub word_looks: Option<Arc<Vec<WordLook>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub requires: Option<BTreeSet<String>>,
    /// Clip ids in order, when the main track's order or membership changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub main: Option<Vec<Id>>,
    /// Clips that are new or changed.
    #[serde(default)]
    pub clips: Vec<Clip>,
    /// Every track's shape, when any track's settings, order or list of items changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub tracks: Option<Vec<TrackShape>>,
    /// Items that are new or changed (wherever they now are).
    #[serde(default)]
    pub items: Vec<Item>,
}

impl ProjectPatch {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// About how much memory the patch holds: its JSON length (the history budget's unit), counted without
    /// building the JSON.
    pub fn approx_bytes(&self) -> usize {
        struct Count(usize);
        impl io::Write for Count {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                self.0 += buf.len();
                Ok(buf.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut count = Count(0);
        serde_json::to_writer(&mut count, self).map_or(0, |()| count.0)
    }
}

fn shape(t: &Track) -> TrackShape {
    TrackShape {
        id: t.id.clone(),
        kind: t.kind,
        name: t.name.clone(),
        hidden: t.hidden,
        muted: t.muted,
        locked: t.locked,
        solo: t.solo,
        items: t.items.iter().map(|i| i.id.clone()).collect(),
    }
}

fn changed<T: Clone + PartialEq>(a: &T, b: &T) -> Option<T> {
    (a != b).then(|| b.clone())
}

fn same_shapes(a: &[Track], b: &[Track]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            x.id == y.id
                && x.kind == y.kind
                && x.name == y.name
                && (x.hidden, x.muted, x.locked, x.solo) == (y.hidden, y.muted, y.locked, y.solo)
                && x.items.len() == y.items.len()
                && x.items.iter().zip(&y.items).all(|(i, j)| i.id == j.id)
        })
}

fn clip_id(c: &Clip) -> &str {
    &c.id
}

fn item_id(i: &Item) -> &str {
    &i.id
}

fn index<'a, T: 'a>(
    all: impl IntoIterator<Item = &'a T>,
    id: fn(&T) -> &str,
) -> HashMap<&'a str, &'a T> {
    all.into_iter().map(|t| (id(t), t)).collect()
}

/// The entities in `b` that are new or differ from `a`'s with the same id.
fn changed_in<'a, T: Clone + PartialEq + 'a>(
    a: impl IntoIterator<Item = &'a T>,
    b: impl IntoIterator<Item = &'a T>,
    id: fn(&T) -> &str,
) -> Vec<T> {
    let old = index(a, id);
    b.into_iter()
        .filter(|t| old.get(id(t)).copied() != Some(*t))
        .cloned()
        .collect()
}

/// What turns `a` into `b`.
pub fn diff(a: &Project, b: &Project) -> ProjectPatch {
    let (am, bm) = (&a.timeline.main, &b.timeline.main);
    let main = (am.len() != bm.len() || am.iter().zip(bm).any(|(x, y)| x.id != y.id))
        .then(|| bm.iter().map(|c| c.id.clone()).collect());
    let (at, bt) = (&a.timeline.tracks, &b.timeline.tracks);
    let tracks = (!same_shapes(at, bt)).then(|| bt.iter().map(shape).collect());
    ProjectPatch {
        name: changed(&a.name, &b.name),
        settings: changed(&a.settings, &b.settings),
        assets: changed(&a.assets, &b.assets),
        recordings: changed(&a.recordings, &b.recordings),
        luts: changed(&a.luts, &b.luts),
        captions: changed(&a.captions, &b.captions),
        background: changed(&a.background, &b.background),
        audio: changed(&a.audio, &b.audio),
        cursor: changed(&a.cursor, &b.cursor),
        word_fixes: changed(&a.word_fixes, &b.word_fixes),
        word_looks: changed(&a.word_looks, &b.word_looks),
        requires: changed(&a.requires, &b.requires),
        main,
        clips: changed_in(am, bm, clip_id),
        tracks,
        items: changed_in(
            at.iter().flat_map(|t| &t.items),
            bt.iter().flat_map(|t| &t.items),
            item_id,
        ),
    }
}

/// The patch that does `first`, then `then`, for two patches made one after the other on one project:
/// `apply(a, &compose(f, t)) == apply(&apply(a, f)?, t)`. A part `then` sets wins; clips and items are merged by id,
/// the later one kept. The history uses it to fold a step into the one before it. O(patch).
pub fn compose(first: &ProjectPatch, then: &ProjectPatch) -> ProjectPatch {
    fn either<T: Clone>(first: &Option<T>, then: &Option<T>) -> Option<T> {
        then.as_ref().or(first.as_ref()).cloned()
    }
    fn merged<T: Clone>(first: &[T], then: &[T], id: fn(&T) -> &str) -> Vec<T> {
        let later: HashSet<&str> = then.iter().map(id).collect();
        first
            .iter()
            .filter(|t| !later.contains(id(t)))
            .chain(then)
            .cloned()
            .collect()
    }
    ProjectPatch {
        name: either(&first.name, &then.name),
        settings: either(&first.settings, &then.settings),
        assets: either(&first.assets, &then.assets),
        recordings: either(&first.recordings, &then.recordings),
        luts: either(&first.luts, &then.luts),
        captions: either(&first.captions, &then.captions),
        background: either(&first.background, &then.background),
        audio: either(&first.audio, &then.audio),
        cursor: either(&first.cursor, &then.cursor),
        word_fixes: either(&first.word_fixes, &then.word_fixes),
        word_looks: either(&first.word_looks, &then.word_looks),
        requires: either(&first.requires, &then.requires),
        main: either(&first.main, &then.main),
        clips: merged(&first.clips, &then.clips, clip_id),
        tracks: either(&first.tracks, &then.tracks),
        items: merged(&first.items, &then.items, item_id),
    }
}

const BROKEN: &str = code!("model.project.patchMismatch");

/// The patch's value, else `a`'s.
fn or_keep<T: Clone>(patched: &Option<T>, old: &T) -> T {
    patched.as_ref().unwrap_or(old).clone()
}

/// `old` in the same order, with each entity the patch changed taken from it.
fn refreshed<T: Clone>(old: &[T], new: &HashMap<&str, &T>, id: fn(&T) -> &str) -> Vec<T> {
    old.iter()
        .map(|t| new.get(id(t)).copied().unwrap_or(t).clone())
        .collect()
}

/// The entities named by `ids`, in that order.
fn pick<T: Clone>(ids: &[Id], by_id: &HashMap<&str, &T>) -> Result<Vec<T>> {
    ids.iter()
        .map(|id| {
            by_id
                .get(id.as_str())
                .map(|t| (*t).clone())
                .ok_or_else(|| invalid(BROKEN))
        })
        .collect()
}

/// `a` with `patch` applied. Fails only for a patch made from another project. Each part is cloned once: from the
/// patch where it changed, from `a` where it didn't.
pub fn apply(a: &Project, patch: &ProjectPatch) -> Result<Project> {
    let main = match &patch.main {
        None if patch.clips.is_empty() => a.timeline.main.clone(),
        None => refreshed(&a.timeline.main, &index(&patch.clips, clip_id), clip_id),
        Some(ids) => {
            let mut by_id = index(&a.timeline.main, clip_id);
            by_id.extend(index(&patch.clips, clip_id));
            pick(ids, &by_id)?
        }
    };
    let tracks = match &patch.tracks {
        None if patch.items.is_empty() => a.timeline.tracks.clone(),
        None => {
            let new = index(&patch.items, item_id);
            a.timeline
                .tracks
                .iter()
                .map(|t| Track {
                    id: t.id.clone(),
                    kind: t.kind,
                    name: t.name.clone(),
                    hidden: t.hidden,
                    muted: t.muted,
                    locked: t.locked,
                    solo: t.solo,
                    items: refreshed(&t.items, &new, item_id),
                })
                .collect()
        }
        Some(shapes) => {
            let mut by_id = index(a.timeline.tracks.iter().flat_map(|t| &t.items), item_id);
            by_id.extend(index(&patch.items, item_id));
            shapes
                .iter()
                .map(|s| {
                    Ok(Track {
                        id: s.id.clone(),
                        kind: s.kind,
                        name: s.name.clone(),
                        hidden: s.hidden,
                        muted: s.muted,
                        locked: s.locked,
                        solo: s.solo,
                        items: pick(&s.items, &by_id)?,
                    })
                })
                .collect::<Result<_>>()?
        }
    };
    Ok(Project {
        version: a.version,
        id: a.id.clone(),
        name: or_keep(&patch.name, &a.name),
        settings: or_keep(&patch.settings, &a.settings),
        assets: or_keep(&patch.assets, &a.assets),
        recordings: or_keep(&patch.recordings, &a.recordings),
        luts: or_keep(&patch.luts, &a.luts),
        timeline: Timeline { main, tracks },
        captions: or_keep(&patch.captions, &a.captions),
        background: or_keep(&patch.background, &a.background),
        audio: or_keep(&patch.audio, &a.audio),
        cursor: or_keep(&patch.cursor, &a.cursor),
        word_fixes: or_keep(&patch.word_fixes, &a.word_fixes),
        word_looks: or_keep(&patch.word_looks, &a.word_looks),
        requires: or_keep(&patch.requires, &a.requires),
    })
}
