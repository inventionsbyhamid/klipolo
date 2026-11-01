//! One rule in one place for chat, external agents, polish and plugins: a batch that takes content away is
//! proposed, not applied at once. `Transaction::review` applies a batch once and lists what it takes away
//! ([`Removals`]); the batch is a removal exactly when that list isn't empty. `op_risk` judges an op by itself,
//! conservatively, for when there is no project to compare against.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::apply::apply_ops;
use crate::error::ModelError;
use crate::ids::Id;
use crate::layout::Layout;
use crate::ops::{Op, Origin, Transaction};
use crate::project::{FormatKind, Marker, Project};
use crate::query::{anchored_end, piece_of};
use crate::time::{drifted, merge_ranges, scaled, unscaled, RangeUs, Us};
use crate::timeline::{Clip, ClipSource, EffectInstance, Item, ItemContent};

mod footage;
mod sound;

/// Rounding slack when comparing lengths (speed changes rescale them).
const SLACK_US: Us = 1_000;

/// What an op can do to the user's content.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum OpRisk {
    /// Adds or restyles: applies at once with an Undo chip.
    Additive,
    /// Removes footage, layers, tracks, effects or media: chat, polish and plugins propose it first.
    Removal,
}

/// An op judged by itself. Conservative: an op that may shorten or drop something (a layer's new length, a
/// replaced effect stack, a transition taken away) counts as a removal. No wildcard arm: a new op must be
/// classified.
pub fn op_risk(op: &Op) -> OpRisk {
    match op {
        Op::DeleteRanges { .. }
        | Op::RemoveClip { .. }
        | Op::RemoveItem { .. }
        | Op::RemoveTrack { .. }
        | Op::RemoveAsset { .. }
        | Op::RemoveLut { .. }
        | Op::TrimClip { .. }
        | Op::TrimItem { .. }
        | Op::RemoveEffect { .. }
        | Op::RemoveMarker { .. } => OpRisk::Removal,
        Op::UpdateItem { patch, .. } if patch.duration_us.is_some() || patch.effects.is_some() => {
            OpRisk::Removal
        }
        Op::UpdateClip { patch, .. } if patch.effects.is_some() => OpRisk::Removal,
        // A source's mix, or a clip's own, replaced whole: its audio effects may go, as a replaced effect stack's do. A
        // sync change slips the source's separated sound layers, which may cut one back where its file ends. Re-timed
        // runs keep every file; the layers that follow them are judged by review (a cut-back one is shortened).
        Op::UpdateRecordingSource { patch, .. }
            if patch.mix.is_some() || (patch.nudge_us.is_some() && patch.runs.is_none()) =>
        {
            OpRisk::Removal
        }
        Op::SetClipSourceMix { .. } => OpRisk::Removal,
        // Taking a transition away, as `Transaction::risk_against` judges it; adding or changing one restyles.
        Op::SetTransition {
            transition: None, ..
        } => OpRisk::Removal,
        Op::Rename { .. }
        | Op::SetSettings { .. }
        | Op::SetCaptions { .. }
        // Taking a fix away brings back the words heard: nothing of the video goes.
        | Op::FixWords { .. }
        // A look restyles words; a hidden word stays in the word list, and undo brings it back.
        | Op::SetWordLook { .. }
        | Op::SetBackground { .. }
        | Op::SetAudio { .. }
        | Op::AddAssets { .. }
        | Op::AddRecording { .. }
        | Op::AddLut { .. }
        | Op::AddTrack { .. }
        | Op::MoveTrack { .. }
        | Op::UpdateTrack { .. }
        | Op::InsertClip { .. }
        | Op::MoveClip { .. }
        | Op::Split { .. }
        | Op::FreezeFrame { .. }
        | Op::ReverseClip { .. }
        | Op::UpdateClip { .. }
        | Op::AddItem { .. }
        | Op::UpdateItem { .. }
        | Op::MoveItem { .. }
        | Op::SplitItem { .. }
        | Op::SetAttach { .. }
        | Op::DetachSource { .. }
        | Op::DetachSound { .. }
        // A source's name, or its files re-timed with every file kept: nothing goes.
        | Op::UpdateRecordingSource { .. }
        // The same footage at the same place, and a new source.
        | Op::UseRecording { .. }
        | Op::AddRecordingSource { .. }
        | Op::AddMarker { .. }
        | Op::AddEffect { .. }
        | Op::UpdateEffect { .. }
        | Op::MoveEffect { .. }
        | Op::SetTransition { .. }
        | Op::SetCursorStyle { .. } => OpRisk::Additive,
    }
}

/// A batch judged against a project with one `apply_ops`: the project it makes, its risk and what it takes away. Chat
/// keeps `after` and commits it, so a chat batch is applied once.
#[derive(Clone, Debug, PartialEq)]
pub struct Review {
    pub after: Project,
    pub risk: OpRisk,
    pub removals: Removals,
}

impl Transaction {
    /// The ops judged one by one (`op_risk`): the answer when there is no project to compare against.
    pub fn risk(&self) -> OpRisk {
        self.ops
            .iter()
            .map(op_risk)
            .max()
            .unwrap_or(OpRisk::Additive)
    }

    /// See [`Review`]. The batch is a removal exactly when its [`Removals`] aren't empty. A batch that doesn't apply
    /// returns the model's error.
    pub fn review(&self, p: &Project) -> Result<Review, ModelError> {
        Ok(self.review_applied(p, apply_ops(p, &self.ops)?))
    }

    /// [`Transaction::review`] of a batch already applied: `after` must be `apply_ops(p, &self.ops)`, from one call
    /// (a batch applied in parts can differ: transitions are refitted once, at the end of each apply). The chat
    /// compiler passes its working copy when one call applied the whole batch, so the batch isn't applied twice.
    pub fn review_applied(&self, p: &Project, after: Project) -> Review {
        // The batch's new clips and copies: they and their pieces never make up for footage that goes.
        let inserted: HashSet<&str> = self
            .ops
            .iter()
            .filter_map(|op| match op {
                Op::InsertClip { clip, .. } => Some(clip.id.as_str()),
                _ => None,
            })
            .collect();
        let removals = removals_with(p, &after, &inserted);
        let risk = if removals.is_empty() {
            OpRisk::Additive
        } else {
            OpRisk::Removal
        };
        Review {
            after,
            risk,
            removals,
        }
    }

    /// What applying the batch to `p` would actually take away ([`Removals`]): a clip, layer, track, effect,
    /// transition, LUT or media file that goes, footage the main track no longer plays, or less of what layers hold
    /// (a shorter layer). Splits, moves and speed changes keep everything, unless a transition no longer fits. A batch
    /// that doesn't apply is judged op by op.
    pub fn risk_against(&self, p: &Project) -> OpRisk {
        self.review(p).map_or_else(|_| self.risk(), |r| r.risk)
    }

    /// True when the batch removes content and didn't come from the user's own hands: show it as a proposal.
    /// Judged against `project` when there is one, else op by op. Every origin but the person's is judged this way,
    /// chat and external agents alike; whether an external app's additions wait too is its trust level.
    pub fn needs_review(&self, project: Option<&Project>) -> bool {
        if matches!(self.origin, None | Some(Origin::User)) {
            return false;
        }
        let risk = project.map_or_else(|| self.risk(), |p| self.risk_against(p));
        risk == OpRisk::Removal
    }
}

/// What a batch takes away, for the proposal card and the ghosted timeline. `ranges` are main-track
/// footage that goes, on the timeline as it was before the batch; the rest are ids from before the batch, in the
/// project's order. Not empty exactly when `risk_against` says Removal.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Removals {
    pub ranges: Vec<RangeUs>,
    /// Main-track clips that go entirely.
    pub clips: Vec<Id>,
    /// Layers that go.
    pub items: Vec<Id>,
    /// Layers that keep less of their content (a video layer at a new speed plays the same footage: not shortened).
    pub shortened: Vec<Id>,
    pub tracks: Vec<Id>,
    /// Clips and layers that keep existing but lose at least one effect: removed, or replaced by another (effects are
    /// told by content, not by id; adjusting one in place keeps it). Also recordings whose source lost an audio effect
    /// from its default mix, and clips whose own mix of a source lost one.
    pub effects: Vec<Id>,
    /// Clips that keep existing but lose their transition into the next clip.
    pub transitions: Vec<Id>,
    pub assets: Vec<Id>,
    pub luts: Vec<Id>,
    pub recordings: Vec<Id>,
    /// Markers that go from a recording still there, or move or lose their label (another with the same id replaced
    /// it); a recording that goes takes its markers with it.
    pub markers: Vec<Id>,
    /// Recordings still there whose recorded sound covers less of them: a sound source's runs re-timed over more than
    /// a second less (a Sync again or a hand line-up; `sound::shrunk`).
    pub sounds: Vec<Id>,
    /// Formats the batch drops together with work stored for them: a layout placed by hand for that format (a
    /// layer's, the captions', a camera bubble's), or any other per-format setting that doesn't survive the change.
    pub formats: Vec<FormatKind>,
    /// Free layers placed after the video's end (an end card, a closing sound) that a cut moved with it, so they stay
    /// right after it. Not a removal: never counted, so it never makes a batch one.
    #[serde(default)]
    pub moved: Vec<Id>,
}

impl Removals {
    pub fn is_empty(&self) -> bool {
        self.count() == 0 && self.ranges.is_empty()
    }

    /// Things listed by id (footage ranges aren't counted: they show on the timeline).
    pub fn count(&self) -> usize {
        self.clips.len()
            + self.items.len()
            + self.shortened.len()
            + self.tracks.len()
            + self.effects.len()
            + self.transitions.len()
            + self.assets.len()
            + self.luts.len()
            + self.recordings.len()
            + self.markers.len()
            + self.sounds.len()
            + self.formats.len()
    }
}

/// What `after` lacks that `before` had (see [`Removals`]). O(clips + layers + effects) with hashing, plus a binary
/// search per clip into the footage the batch added: one pass per batch, never per frame. Without the batch's ops, a
/// new clip is told from a piece by its id alone (see [`piece_of`]), and one inserted under a removed clip's id reads as
/// that clip; `Transaction::review` also knows which clips the batch inserted.
pub fn removals(before: &Project, after: &Project) -> Removals {
    removals_with(before, after, &HashSet::new())
}

/// [`removals`], where `inserted` names the clips the batch inserted: never pieces of what was there, and never the
/// clip that was there either, even under its id (a batch that removes a clip and inserts another with the same id).
fn removals_with(before: &Project, after: &Project, inserted: &HashSet<&str>) -> Removals {
    // The clips still there from before, by id: an inserted clip is new, whatever its id.
    let clips_after: HashMap<&str, &Clip> = after
        .timeline
        .main
        .iter()
        .filter(|c| !inserted.contains(c.id.as_str()))
        .map(|c| (c.id.as_str(), c))
        .collect();
    let items_after: HashMap<&str, &Item> = layers(after).map(|i| (i.id.as_str(), i)).collect();
    let clip_effects = before.timeline.main.iter().filter(|c| {
        clips_after
            .get(c.id.as_str())
            .is_some_and(|now| loses(&c.effects, &now.effects))
    });
    let item_effects = layers(before).filter(|i| {
        items_after
            .get(i.id.as_str())
            .is_some_and(|now| loses(&i.effects, &now.effects))
    });
    Removals {
        ranges: lost_footage(before, after, &clips_after, inserted),
        clips: gone(before.timeline.main.iter().map(|c| c.id.as_str()), |id| {
            clips_after.contains_key(id)
        }),
        items: gone(layers(before).map(|i| i.id.as_str()), |id| {
            items_after.contains_key(id)
        }),
        shortened: shortened(before, after, &items_after),
        tracks: gone_from(
            before.timeline.tracks.iter().map(|t| t.id.as_str()),
            after.timeline.tracks.iter().map(|t| t.id.as_str()),
        ),
        effects: clip_effects
            .map(|c| c.id.clone())
            .chain(item_effects.map(|i| i.id.clone()))
            .chain(lost_mix_effects(before, after, &clips_after))
            .collect(),
        transitions: lost_transitions(before, after, &clips_after, inserted),
        assets: gone_from(
            before.assets.iter().map(|a| a.id.as_str()),
            after.assets.iter().map(|a| a.id.as_str()),
        ),
        luts: gone_from(
            before.luts.iter().map(|l| l.id.as_str()),
            after.luts.iter().map(|l| l.id.as_str()),
        ),
        recordings: gone_from(
            before.recordings.iter().map(|r| r.id.as_str()),
            after.recordings.iter().map(|r| r.id.as_str()),
        ),
        markers: lost_markers(before, after),
        sounds: sound::shrunk(before, after),
        formats: lost_formats(before, after, &clips_after, &items_after),
        moved: moved_with_end(before, &items_after),
    }
}

/// Free layers placed after the video's end (`query::anchored_end`) that now start elsewhere: moved with the end by a
/// cut. O(layers) with hashing.
fn moved_with_end(before: &Project, items_after: &HashMap<&str, &Item>) -> Vec<Id> {
    let end = anchored_end(&before.timeline);
    layers(before)
        .filter(|i| i.attach.is_none() && i.start_us >= end)
        .filter(|i| {
            items_after
                .get(i.id.as_str())
                .is_some_and(|now| now.start_us != i.start_us)
        })
        .map(|i| i.id.clone())
        .collect()
}

/// The markers of each recording in both that `after`'s copy lacks by id, or holds at another time or with another
/// label (`AddMarker` replaces one with the same id). O(recordings² + markers) with hashing: a project has a handful
/// of recordings.
fn lost_markers(before: &Project, after: &Project) -> Vec<Id> {
    before
        .recordings
        .iter()
        .filter_map(|r| Some((r, after.recording(&r.id)?)))
        .flat_map(|(was, now)| {
            let now: HashMap<&str, &Marker> =
                now.markers.iter().map(|m| (m.id.as_str(), m)).collect();
            was.markers
                .iter()
                .filter(move |m| {
                    now.get(m.id.as_str())
                        .is_none_or(|n| n.at_us != m.at_us || n.label != m.label)
                })
                .map(|m| m.id.clone())
        })
        .collect()
}

/// Audio effects lost from recording sources' mixes: a recording still there whose source's default mix lost one
/// (by the recording's id), then each clip still there whose own mix of a source lost one (by the clip's id). A clip
/// with no mix of its own, before and after, plays the default, so the recording's line speaks for it. O(recordings ×
/// sources + clips with their own mix × sources), each stack at most eight effects.
fn lost_mix_effects(
    before: &Project,
    after: &Project,
    clips_after: &HashMap<&str, &Clip>,
) -> Vec<Id> {
    let mut out = Vec::new();
    for r in &before.recordings {
        let Some(now) = after.recording(&r.id) else {
            continue;
        };
        let lost = r.sources.iter().any(|s| {
            now.source(&s.id)
                .is_some_and(|n| loses(&s.mix.effects, &n.mix.effects))
        });
        if lost {
            out.push(r.id.clone());
        }
    }
    for c in &before.timeline.main {
        let ClipSource::Recording { id } = &c.source else {
            continue;
        };
        let Some(now) = clips_after.get(c.id.as_str()) else {
            continue;
        };
        if c.source_mix.is_empty() && now.source_mix.is_empty() {
            continue;
        }
        let (Some(was_r), Some(now_r)) = (before.recording(id), after.recording(id)) else {
            continue;
        };
        let lost = was_r.sources.iter().any(|s| {
            let own = c.source_mix.contains_key(&s.id) || now.source_mix.contains_key(&s.id);
            own && now_r
                .source(&s.id)
                .is_some_and(|n| loses(&c.mix_for(s).effects, &now.mix_for(n).effects))
        });
        if lost {
            out.push(c.id.clone());
        }
    }
    out
}

/// Formats `before` had and `after` doesn't, for which something still there lost data stored under that format: a
/// layout's own placement (a layer's, the captions', a camera bubble's), a clip's framing or a camera's mode. Data the
/// model keeps in the file when a format goes (it comes back if the format does) isn't lost; neither is a format that
/// held nothing of its own. O(dropped formats × (clips + layers)) with hashing.
fn lost_formats(
    before: &Project,
    after: &Project,
    clips_after: &HashMap<&str, &Clip>,
    items_after: &HashMap<&str, &Item>,
) -> Vec<FormatKind> {
    let dropped = before
        .settings
        .formats
        .iter()
        .copied()
        .filter(|f| !after.settings.formats.contains(f));
    dropped
        .filter(|f| {
            let lost = |had: bool, has: bool| had && !has;
            let placement = |was: &Layout, now: &Layout| {
                lost(was.overrides.contains_key(f), now.overrides.contains_key(f))
            };
            let layers_lost = layers(before).any(|i| {
                items_after
                    .get(i.id.as_str())
                    .is_some_and(|now| placement(&i.layout, &now.layout))
            });
            let clips_lost = before.timeline.main.iter().any(|c| {
                let Some(now) = clips_after.get(c.id.as_str()) else {
                    return false;
                };
                let camera = match (&c.camera, &now.camera) {
                    (Some(was), Some(is)) => {
                        placement(&was.bubble, &is.bubble)
                            || lost(
                                was.mode_overrides.contains_key(f),
                                is.mode_overrides.contains_key(f),
                            )
                    }
                    _ => false,
                };
                camera || lost(c.framing.contains_key(f), now.framing.contains_key(f))
            });
            let captions_lost = lost(
                before.captions.place.contains_key(f),
                after.captions.place.contains_key(f),
            );
            layers_lost || clips_lost || captions_lost
        })
        .collect()
}

/// Whether a stack that held `had` loses an effect now that it holds `kept`. An effect is kept by content rather than
/// by id: by the same effect adjusted in place (its id and contribution, as `UpdateEffect` leaves it; a new setting
/// or switching it off is an edit, as the adjustEffect tool's; or the same id switched to another version
/// of the same library effect, a modification and not a removal), or by one with the same contribution and the same
/// settings under any id (a copy, a re-made stack). An id reused for another effect never keeps it: ids are minted
/// per stack, so every stack's first effect is "fx". Whether it is switched on isn't part of what it is. Each effect
/// keeps one effect only, so a doubled blur that loses a copy loses an effect: ids first (they are unique in a stack),
/// then equal settings, which match as a whole class. Stacks hold eight at most: O(8²).
fn loses(had: &[EffectInstance], kept: &[EffectInstance]) -> bool {
    let mut used = vec![false; kept.len()];
    let mut matched = vec![false; had.len()];
    for (h, e) in had.iter().enumerate() {
        if let Some(k) = kept
            .iter()
            .position(|k| k.id == e.id && (k.effect == e.effect || k.effect.same_line(&e.effect)))
        {
            (used[k], matched[h]) = (true, true);
        }
    }
    had.iter()
        .zip(&matched)
        .filter(|(_, m)| !**m)
        .any(|(e, _)| {
            let same = (0..kept.len())
                .find(|&k| !used[k] && kept[k].effect == e.effect && kept[k].params == e.params);
            same.map(|k| used[k] = true).is_none()
        })
}

fn layers(p: &Project) -> impl Iterator<Item = &Item> {
    p.timeline.tracks.iter().flat_map(|t| &t.items)
}

/// The ids of `before` that `kept` doesn't keep, in order.
fn gone<'a>(before: impl Iterator<Item = &'a str>, kept: impl Fn(&str) -> bool) -> Vec<Id> {
    before.filter(|id| !kept(id)).map(str::to_owned).collect()
}

/// The ids of `before` missing from `after`, in order.
fn gone_from<'a>(
    before: impl Iterator<Item = &'a str>,
    after: impl Iterator<Item = &'a str>,
) -> Vec<Id> {
    let after: HashSet<&str> = after.collect();
    gone(before, |id| after.contains(id))
}

/// What a layer holds, for telling whether another layer holds the same: media by file and the source time it plays
/// (a video layer's at speed 1), the rest by their length (what they show is [`shows`]).
enum Holds<'a> {
    Media {
        key: (&'static str, &'a str),
        span: RangeUs,
    },
    Shown {
        len: Us,
    },
}

/// A layer's kind and what it shows (`shows`).
type Shows<'a> = (&'static str, Cow<'a, str>);

/// What a layer that plays no media shows, for matching it with a layer the batch added: an image's file, a text's
/// words, a generator's contribution and settings, a zoom's target and scale, an adjustment layer's area and effects
/// (without their ids). Built only for layers that lost time and layers the batch added.
fn shows(item: &Item) -> Shows<'_> {
    let json = |v: serde_json::Result<String>| Cow::Owned(v.unwrap_or_default());
    match &item.content {
        ItemContent::Image { asset_id } => ("image", Cow::Borrowed(asset_id.as_str())),
        ItemContent::Text { text, .. } => ("text", Cow::Borrowed(text.as_str())),
        ItemContent::Generator { source, params, .. } => {
            ("generator", json(serde_json::to_string(&(source, params))))
        }
        ItemContent::Zoom { target, scale } => {
            ("zoom", json(serde_json::to_string(&(target, scale))))
        }
        // An adjustment layer re-draws what is under its box at its opacity: the same effects over another area, or
        // fainter, is another layer. Its rotation isn't drawn (the engine draws the box upright), so it isn't part of it.
        ItemContent::Adjustment => {
            let effects: Vec<_> = item
                .effects
                .iter()
                .map(|e| (&e.effect, e.enabled, &e.params))
                .collect();
            let l = &item.layout;
            let area = (&l.rect, &l.overrides, l.opacity);
            ("adjustment", json(serde_json::to_string(&(area, effects))))
        }
        // Media is matched by `Holds::Media`.
        ItemContent::Video { asset_id, .. } => ("video", Cow::Borrowed(asset_id.as_str())),
        ItemContent::Audio { asset_id, .. } => ("audio", Cow::Borrowed(asset_id.as_str())),
    }
}

fn holds(item: &Item) -> Holds<'_> {
    let d = item.duration_us;
    let media = |kind, asset, from: Us, len: Us| Holds::Media {
        key: (kind, asset),
        span: RangeUs {
            start: from,
            end: from.saturating_add(len),
        },
    };
    match &item.content {
        ItemContent::Video {
            asset_id,
            source_in_us,
            speed,
            ..
        } => media(
            "video",
            asset_id.as_str(),
            *source_in_us,
            unscaled(d, *speed),
        ),
        ItemContent::Audio {
            asset_id,
            source_in_us,
            drift_ppm,
            ..
        } => media(
            "audio",
            asset_id.as_str(),
            *source_in_us,
            drifted(d, *drift_ppm),
        ),
        ItemContent::Image { .. }
        | ItemContent::Text { .. }
        | ItemContent::Generator { .. }
        | ItemContent::Zoom { .. }
        | ItemContent::Adjustment => Holds::Shown { len: d },
    }
}

/// Layers that keep less of what they held. Only layers the batch added can make up for it, and only by holding the
/// same (a split's second piece holds the rest of its first): a longer layer that was already there, or a new layer
/// of another text, stretch of the file, zoom target or effect, doesn't. A layer whose own words or picture change
/// keeps its length (a restyle); one that becomes another kind, or plays another file, holds none of what it held.
fn shortened(before: &Project, after: &Project, items_after: &HashMap<&str, &Item>) -> Vec<Id> {
    let ids_before: HashSet<&str> = layers(before).map(|i| i.id.as_str()).collect();
    let mut added_spans: HashMap<(&str, &str), Vec<RangeUs>> = HashMap::new();
    let mut added_lengths: HashMap<Shows, Us> = HashMap::new();
    for item in layers(after).filter(|i| !ids_before.contains(i.id.as_str())) {
        match holds(item) {
            Holds::Media { key, span } => added_spans.entry(key).or_default().push(span),
            Holds::Shown { len } => *added_lengths.entry(shows(item)).or_default() += len,
        }
    }
    for spans in added_spans.values_mut() {
        *spans = merge_ranges(spans, Us::MAX);
    }
    // Media: footage no layer still plays. Shown: what each kind lost, made up only by what the batch added of it.
    let mut lost: HashSet<&str> = HashSet::new();
    let mut lost_lengths: HashMap<Shows, (Us, Vec<&str>)> = HashMap::new();
    for item in layers(before) {
        let Some(now) = items_after.get(item.id.as_str()) else {
            continue;
        };
        match holds(item) {
            Holds::Media { key, span } => {
                let own = match holds(now) {
                    Holds::Media { key: k, span } if k == key => Some(span),
                    _ => None,
                };
                let pool = added_spans.get(&key).map_or(&[][..], Vec::as_slice);
                if !uncovered(span.start, span.end, own, pool).is_empty() {
                    lost.insert(&item.id);
                }
            }
            Holds::Shown { len } => {
                let same_kind =
                    std::mem::discriminant(&now.content) == std::mem::discriminant(&item.content);
                let kept = if same_kind { now.duration_us } else { 0 };
                if kept.saturating_add(SLACK_US) < len {
                    let entry = lost_lengths.entry(shows(item)).or_default();
                    entry.0 = entry.0.saturating_add(len - kept);
                    entry.1.push(&item.id);
                }
            }
        }
    }
    for (key, (length, ids)) in lost_lengths {
        let made_up = added_lengths.get(&key).copied().unwrap_or(0);
        if made_up.saturating_add(SLACK_US) < length {
            lost.extend(ids);
        }
    }
    layers(before)
        .filter(|i| lost.contains(i.id.as_str()))
        .map(|i| i.id.clone())
        .collect()
}

/// A main-track source, told apart by kind.
fn source_key(s: &ClipSource) -> (bool, &str) {
    (matches!(s, ClipSource::Recording { .. }), s.id().as_str())
}

/// Where a clip the batch added ends in its source, whether it carries a transition, and whether a clip from before
/// was already judged by it.
struct PieceEnd {
    out_us: Us,
    carries: bool,
    taken: bool,
}

/// Clips that keep existing but lost the transition at their old end. A transition is judged where the clip ended. A
/// clip whose end didn't move keeps it only if it still carries one. A clip whose end moved (a split or a cut) is
/// judged by the clip the batch added of the same source ending there (the piece left at the clip's end, which meets
/// the next clip): the transition is kept only if that piece carries one; with no such piece, only if the clip itself
/// still does (a trim). So a transition given to a clip that was already there, to a new clip of other footage, or to
/// the first piece of a split doesn't make up for it, and another clip of the same footage ending at the same moment
/// (a replay of a clip's tail) neither takes a piece nor gives one. O(T log T) over the batch's clips.
fn lost_transitions(
    before: &Project,
    after: &Project,
    clips_after: &HashMap<&str, &Clip>,
    inserted: &HashSet<&str>,
) -> Vec<Id> {
    let ids_before: HashSet<&str> = before.timeline.main.iter().map(|c| c.id.as_str()).collect();
    // Where each added clip ends, by source, sorted (an inserted clip is added, even under an id from before).
    let mut added: HashMap<(bool, &str), Vec<PieceEnd>> = HashMap::new();
    for c in after
        .timeline
        .main
        .iter()
        .filter(|c| !ids_before.contains(c.id.as_str()) || inserted.contains(c.id.as_str()))
    {
        added
            .entry(source_key(&c.source))
            .or_default()
            .push(PieceEnd {
                out_us: c.source_out_us,
                carries: c.transition_out.is_some(),
                taken: false,
            });
    }
    for ends in added.values_mut() {
        ends.sort_unstable_by_key(|e| e.out_us);
    }
    before
        .timeline
        .main
        .iter()
        .filter(|c| c.transition_out.is_some())
        .filter(|c| {
            let Some(now) = clips_after.get(c.id.as_str()) else {
                return false; // gone: listed under `clips`
            };
            let carries = now.transition_out.is_some();
            if now.source_out_us.abs_diff(c.source_out_us) <= SLACK_US.unsigned_abs() {
                return !carries; // its end is where it was: judged by itself
            }
            // The added piece ending where the clip ended (one carrying a transition first), taken once.
            let piece = added.get_mut(&source_key(&c.source)).and_then(|ends| {
                let (lo, hi) = (c.source_out_us - SLACK_US, c.source_out_us + SLACK_US);
                let from = ends.partition_point(|e| e.out_us < lo);
                let to = from + ends[from..].partition_point(|e| e.out_us <= hi);
                let free = (from..to).filter(|&i| !ends[i].taken);
                let i = free
                    .clone()
                    .find(|&i| ends[i].carries)
                    .or_else(|| free.clone().next())?;
                ends[i].taken = true;
                Some(ends[i].carries)
            });
            !piece.unwrap_or(carries)
        })
        .map(|c| c.id.clone())
        .collect()
}

/// Main-track footage of `before` that `after` no longer plays, in `before`'s timeline time, merged and ascending. A
/// clip's footage is kept by the clip with its id, and by the pieces the batch cut or split from it ([`piece_of`]),
/// whatever their look: a new speed, volume, effect or crop on a piece is an edit, as it is on the whole clip. Nothing
/// else keeps it: not a clip of the same file the batch inserted (a fresh copy plays it raw, even one named like a
/// piece), and not another clip that was already there (trimming one of two copies of a moment takes that copy away).
/// Footage is what plays: a clip or piece now on another source (`useRecording`) keeps the stretches where it
/// shows the same files over the same file spans (`footage`).
fn lost_footage(
    before: &Project,
    after: &Project,
    clips_after: &HashMap<&str, &Clip>,
    inserted: &HashSet<&str>,
) -> Vec<RangeUs> {
    let clips_before: HashMap<&str, &Clip> = before
        .timeline
        .main
        .iter()
        .map(|c| (c.id.as_str(), c))
        .collect();
    let mut pieces: HashMap<&str, Vec<RangeUs>> = HashMap::new();
    for c in after
        .timeline
        .main
        .iter()
        .filter(|c| !clips_before.contains_key(c.id.as_str()))
    {
        let base = piece_of(
            &c.id,
            |id| clips_before.contains_key(id),
            |id| inserted.contains(id),
        );
        let Some((base, was)) = base.and_then(|b| clips_before.get_key_value(b)) else {
            continue;
        };
        if c.source == was.source {
            pieces.entry(base).or_default().push(RangeUs {
                start: c.source_in_us,
                end: c.source_out_us,
            });
        } else {
            let kept = footage::kept_spans(before, was, after, c);
            pieces.entry(base).or_default().extend(kept);
        }
    }
    footage::converted(before, after, clips_after, &mut pieces);
    for spans in pieces.values_mut() {
        *spans = merge_ranges(spans, Us::MAX);
    }
    let mut out = Vec::new();
    let mut start: Us = 0;
    for c in &before.timeline.main {
        let own = clips_after
            .get(c.id.as_str())
            .filter(|now| now.source == c.source)
            .map(|now| RangeUs {
                start: now.source_in_us,
                end: now.source_out_us,
            });
        let pool = pieces.get(c.id.as_str()).map_or(&[][..], Vec::as_slice);
        for (a, b) in uncovered(c.source_in_us, c.source_out_us, own, pool) {
            out.push(RangeUs {
                start: start.saturating_add(scaled(a - c.source_in_us, c.speed)),
                end: start.saturating_add(scaled(b - c.source_in_us, c.speed)),
            });
        }
        start = start.saturating_add(c.duration_us());
    }
    merge_ranges(&out, start)
}

/// The parts of `[from, to)` that neither `own` nor the sorted, disjoint `pool` covers, ascending; gaps within
/// `SLACK_US` (speed rounding) don't count. A binary search finds the first pooled span that matters.
fn uncovered(from: Us, to: Us, own: Option<RangeUs>, pool: &[RangeUs]) -> Vec<(Us, Us)> {
    let mut own = own.filter(|r| r.end > from && r.start < to);
    let first = pool.partition_point(|r| r.end <= from);
    let mut spans = pool[first..]
        .iter()
        .copied()
        .take_while(|r| r.start < to)
        .peekable();
    let mut out = Vec::new();
    let mut at = from;
    loop {
        let pooled = spans.peek().map(|r| r.start);
        let next = match (own, pooled) {
            (Some(o), Some(p)) if o.start <= p => own.take(),
            (Some(_), None) => own.take(),
            (_, Some(_)) => spans.next(),
            (None, None) => None,
        };
        let Some(r) = next else { break };
        if r.start > at.saturating_add(SLACK_US) {
            out.push((at, r.start));
        }
        at = at.max(r.end);
        if at >= to {
            return out;
        }
    }
    if to > at.saturating_add(SLACK_US) {
        out.push((at, to));
    }
    out
}
