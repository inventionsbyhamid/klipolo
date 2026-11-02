//! The clip tools on a working copy: each makes the ops the same edit by hand makes, so the project ends the same.
//! - `setClip`: one `UpdateClip` per clip it names (every piece of it in the working copy; every clip for `all`) with
//!   what changes, as the clip card's fields do: the speed (the model refits the transitions at its cuts, and the edit
//!   says so, `note_refit`), the volume and mute (only a clip with sound has them, `query::clip_has_sound`), and Fit or
//!   Fill in a format the video has, built on the clip's framing there as the engine draws it
//!   (`framing::effective_framing`, the card's base) with only the mode changed. A value the clip already has adds
//!   nothing, as the card's control does nothing when its value is picked again.
//! - `setTransition`: one `SetTransition` per cut it names (the cut after the clip's last piece; every cut for `all`):
//!   a new one is Add transition's (`transitions::default_transition_us`, the frame grid's half second or the longest
//!   that fits), a new kind starts from its own defaults as the transition card's Type does (its length kept), a length
//!   goes on the frame grid within the cut's room as the card's Length does (`transitions::transition_length_us`), and
//!   none takes it away as Remove does. Each cut is fitted beside the transitions the cuts before it were just given,
//!   as the model checks a batch's ops in order. With every cut asked for, a cut with no spare footage is left as it
//!   is, and the reply says how many were.
//! - `setClip` on a layer, as its own card changes it: a song (`m1`, every piece of it) takes a volume and mute, a video
//!   layer (`v1`) a speed (a separated camera's being its clip's), a volume and mute, each on the layer's content.
//!   Fit and Fill stay main clips', and a speed on music is refused.
//!
//! Undo labels: `chat.setClip {clip, change, noun}` (`change` one of speed, volume, mute, unmute, fit, fill, or
//! several; `noun` clip, sound for a song or video for a video layer, as the cards' mute labels name it), and
//! `chat.setClip.note` with the page's own note (`timeline.refit {removed, shortened}`) when a speed refits
//! transitions, as the card's `speed.undo.note` does; `chat.setTransition {after, change, count}` (`change` one of add,
//! change, length, remove).

use std::collections::BTreeMap;

use klipolo_model::Project;
use klipolo_model::coded::{CodedError, ErrorParam};
use klipolo_model::framing::effective_framing;
use klipolo_model::media::music::runs;
use klipolo_model::ops::{ClipPatch, ItemPatch, Op};
use klipolo_model::project::FormatKind;
use klipolo_model::query::{camera_clip_of, clip_has_sound, find_item, has_own_sound};
use klipolo_model::time::Us;
use klipolo_model::timeline::{Clip, Framing, FramingMode, ItemContent, TrackKind, Transition};
use klipolo_model::transitions::{
    MIN_TRANSITION_US, default_transition_us, max_transition_us, transition_length_us,
};

use super::additions::format_word;
use super::{Compiler, Edit, Refused, Tried, label, to_model, to_person};
use crate::context::secs;
use crate::tools::{ClipRef, TransitionKind};

/// What a refusal says when a transition can't fit at the one cut named.
pub const NO_ROOM: &str = "There isn't enough spare footage at that cut for a transition. Trim the clips there to \
                           make room.";
/// What a refusal says when no cut has room for a transition.
pub const NO_ROOM_ANYWHERE: &str = "No cut has enough spare footage for a transition. Trim the clips at a cut to \
                                    make room.";
/// What a refusal says when there are no transitions to take away.
pub const NO_TRANSITIONS: &str = "There are no transitions to remove.";

/// What `setClip` asks to change, as read.
pub(super) struct Change {
    pub speed: Option<f64>,
    pub volume: Option<f64>,
    pub muted: Option<bool>,
    pub framing: Option<(FramingMode, Option<FormatKind>)>,
}

/// The working copy's clips `target` names, by index: every piece of a ref's clip, or every clip.
fn named(
    c: &Compiler,
    work: &Project,
    tool: &str,
    target: &ClipRef,
) -> Result<Vec<usize>, Refused> {
    let main = &work.timeline.main;
    let ClipRef::One(short) = target else {
        return Ok((0..main.len()).collect());
    };
    let Some(id) = c.inputs.refs.clip(short) else {
        return Err(to_model(format!("{tool}: there is no clip {short}.")));
    };
    let pieces: Vec<usize> = match c.map.index_of(id) {
        Some(i) => (c.map.pieces_of(work, i).into_iter())
            .filter_map(|piece| main.iter().position(|clip| clip.id == piece.id))
            .collect(),
        None => Vec::new(),
    };
    if pieces.is_empty() {
        return Err(to_model(format!("{tool}: that clip is gone.")));
    }
    Ok(pieces)
}

pub(super) fn set_clip(
    c: &Compiler,
    work: &Project,
    clip: &ClipRef,
    change: &Change,
) -> Result<Edit, Refused> {
    // A song or a video layer, as its own card changes it.
    if let ClipRef::One(short) = clip {
        if let Some(first) = c.inputs.refs.music(short) {
            return set_song(work, short, first, change);
        }
        if let Some(id) = c.inputs.refs.video(short) {
            return set_video(work, short, id, change);
        }
        if short.starts_with(['m', 'v']) {
            return Err(to_model(format!("setClip: there is no layer {short}.")));
        }
    }
    let clips = named(c, work, "setClip", clip)?;
    let main = &work.timeline.main;
    // Volume and mute only where there is sound, as the card shows them.
    let sound = change.volume.is_some() || change.muted.is_some();
    let heard: Vec<usize> = (clips.iter().copied())
        .filter(|&i| clip_has_sound(work, &main[i]))
        .collect();
    if sound && heard.is_empty() {
        return Err(to_model(match clip {
            ClipRef::One(short) => {
                format!("setClip: {short} has no sound, so it has no volume or mute.")
            }
            ClipRef::All => "setClip: no clip has sound, so none has a volume or mute.".to_owned(),
        }));
    }
    let formats: Vec<FormatKind> = match change.framing {
        Some((_, Some(format))) if !work.settings.formats.contains(&format) => {
            return Err(to_model(format!(
                "setClip: the video has no {} format. Add it with setFormat first.",
                format_word(format)
            )));
        }
        Some((_, Some(format))) => vec![format],
        Some((_, None)) => work.settings.formats.clone(),
        None => Vec::new(),
    };
    let mut ops = Vec::new();
    let mut changed: Vec<&str> = Vec::new();
    let mut note = |what: &'static str| {
        if !changed.contains(&what) {
            changed.push(what);
        }
    };
    for &i in &clips {
        let now = &main[i];
        let mut patch = ClipPatch::default();
        if let Some(speed) = change.speed.filter(|&s| s != now.speed) {
            patch.speed = Some(speed);
            note("speed");
        }
        if heard.contains(&i) {
            if let Some(volume) = change.volume.filter(|&v| v != now.volume) {
                patch.volume = Some(volume);
                note("volume");
            }
            if let Some(muted) = change.muted.filter(|&m| m != now.muted) {
                patch.muted = Some(muted);
                note(if muted { "mute" } else { "unmute" });
            }
        }
        if let Some((mode, _)) = change.framing
            && let Some(framing) = framed(work, now, mode, &formats)
        {
            patch.framing = Some(framing);
            note(match mode {
                FramingMode::Fit => "fit",
                FramingMode::Fill => "fill",
            });
        }
        if patch != ClipPatch::default() {
            ops.push(Op::UpdateClip {
                id: now.id.clone(),
                patch,
            });
        }
    }
    Ok(Edit {
        ops,
        label: change_label(clip.word(), &changed, "clip"),
    })
}

/// `setClip`'s label: the change (`several` for more than one), on what (`noun`, as the cards' mute labels name it:
/// clip, sound for music, video for a video layer).
fn change_label(short: &str, changed: &[&str], noun: &str) -> CodedError {
    let change = match changed {
        [one] => *one,
        _ => "several",
    };
    label(
        "setClip",
        vec![
            ("clip", ErrorParam::from(short)),
            ("change", ErrorParam::from(change)),
            ("noun", ErrorParam::from(noun)),
        ],
    )
}

/// `what` once in `changed`.
fn note_change<'a>(changed: &mut Vec<&'a str>, what: &'a str) {
    if !changed.contains(&what) {
        changed.push(what);
    }
}

/// `setClip` on the song `short` names (its first piece `first`), as the music card does: its Volume on every piece of
/// the song (`runContentOps`), and Mute on every piece (the layer's Mute, `toggleSoundMute`). No speed, and no Fit or
/// Fill: the card has none.
fn set_song(work: &Project, short: &str, first: &str, change: &Change) -> Result<Edit, Refused> {
    if change.speed.is_some() || change.framing.is_some() {
        return Err(to_model(format!(
            "setClip: {short} is music: it takes a volume or mute, not a speed or Fit or Fill."
        )));
    }
    let song = (work.timeline.tracks.iter())
        .filter(|t| t.kind == TrackKind::Audio)
        .flat_map(|t| runs(t).into_iter().map(move |run| &t.items[run]))
        .find(|pieces| pieces.iter().any(|piece| piece.id == first));
    let Some(pieces) = song else {
        return Err(to_model("setClip: that music is gone.".to_owned()));
    };
    let mut changed: Vec<&str> = Vec::new();
    let mut ops = Vec::new();
    for piece in pieces {
        let mut content = piece.content.clone();
        let ItemContent::Audio { volume, muted, .. } = &mut content else {
            continue;
        };
        if let Some(v) = change.volume {
            if (pieces.iter())
                .any(|p| matches!(p.content, ItemContent::Audio { volume, .. } if volume != v))
            {
                note_change(&mut changed, "volume");
            }
            *volume = v;
        }
        if let Some(m) = change.muted {
            if (pieces.iter())
                .any(|p| matches!(p.content, ItemContent::Audio { muted, .. } if muted != m))
            {
                note_change(&mut changed, if m { "mute" } else { "unmute" });
            }
            *muted = m;
        }
        ops.push(item_op(&piece.id, content));
    }
    // What it has already changes nothing, as the card's control does nothing when its value is picked again.
    if changed.is_empty() {
        ops.clear();
    }
    Ok(Edit {
        ops,
        label: change_label(short, &changed, "sound"),
    })
}

/// `setClip` on the video layer `short` names (`id`), as the video layer card does: its Speed (a separated camera's is
/// its clip's, `query::camera_clip_of`, so the lips stay in sync), and its Volume and Mute when its file has sound
/// (`query::has_own_sound`), each on the layer's content (`contentOps`). No Fit or Fill: those are for main clips.
fn set_video(work: &Project, short: &str, id: &str, change: &Change) -> Result<Edit, Refused> {
    if change.framing.is_some() {
        return Err(to_model(format!(
            "setClip: {short} is a video layer: Fit and Fill are for clips on the main track."
        )));
    }
    let Some((t, i)) = find_item(work, id) else {
        return Err(to_model("setClip: that video layer is gone.".to_owned()));
    };
    let item = &work.timeline.tracks[t].items[i];
    if (change.volume.is_some() || change.muted.is_some()) && !has_own_sound(work, item) {
        return Err(to_model(format!(
            "setClip: {short} has no sound, so it has no volume or mute."
        )));
    }
    let mut changed: Vec<&str> = Vec::new();
    let mut ops = Vec::new();
    let mut content = item.content.clone();
    let ItemContent::Video {
        speed,
        volume,
        muted,
        ..
    } = &mut content
    else {
        return Err(to_model(format!("setClip: {short} is not a video layer.")));
    };
    match (change.speed, camera_clip_of(work, item)) {
        (Some(s), Some(clip)) if work.timeline.main[clip].speed != s => {
            ops.push(Op::UpdateClip {
                id: work.timeline.main[clip].id.clone(),
                patch: ClipPatch {
                    speed: Some(s),
                    ..ClipPatch::default()
                },
            });
            note_change(&mut changed, "speed");
        }
        (Some(s), None) if *speed != s => {
            *speed = s;
            note_change(&mut changed, "speed");
        }
        _ => {}
    }
    if let Some(v) = change.volume.filter(|v| *v != *volume) {
        *volume = v;
        note_change(&mut changed, "volume");
    }
    if let Some(m) = change.muted.filter(|m| *m != *muted) {
        *muted = m;
        note_change(&mut changed, if m { "mute" } else { "unmute" });
    }
    if content != item.content {
        ops.push(item_op(id, content));
    }
    Ok(Edit {
        ops,
        label: change_label(short, &changed, "video"),
    })
}

/// The cards' op for a layer's new content (`itemOp(item, { content })`).
fn item_op(id: &str, content: ItemContent) -> Op {
    Op::UpdateItem {
        id: id.to_owned(),
        patch: ItemPatch {
            content: Some(content),
            ..ItemPatch::default()
        },
    }
}

/// `clip`'s framings with `mode` in each of `formats` where it is framed otherwise now, each built on its framing there
/// as the engine draws it (the card's base), the other formats' kept; None when every one is `mode` already.
pub(super) fn framed(
    work: &Project,
    clip: &Clip,
    mode: FramingMode,
    formats: &[FormatKind],
) -> Option<BTreeMap<FormatKind, Framing>> {
    let mut framing = clip.framing.clone();
    let mut any = false;
    for &format in formats {
        let now = effective_framing(work, clip, format);
        if now.mode != mode {
            framing.insert(format, Framing { mode, ..now });
            any = true;
        }
    }
    any.then_some(framing)
}

/// What a speed change did to transitions, once the model refitted them: how many went and how many got shorter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Refit {
    pub removed: usize,
    pub shortened: usize,
}

impl Refit {
    /// The transitions of `before`'s clips, refitted as `after` has them (clips matched by id).
    fn between(before: &Project, after: &Project) -> Self {
        let mut out = Self::default();
        for clip in &before.timeline.main {
            let (Some(was), Some(now)) = (
                clip.transition_out.as_ref(),
                (after.timeline.main.iter()).find(|c| c.id == clip.id),
            ) else {
                continue;
            };
            match &now.transition_out {
                None => out.removed += 1,
                Some(t) if t.duration_us < was.duration_us => out.shortened += 1,
                Some(_) => {}
            }
        }
        out
    }

    /// The page's note on it, as a label's value: `timeline.refit {removed, shortened}`.
    fn coded(self) -> CodedError {
        CodedError {
            code: "timeline.refit".to_owned(),
            params: [
                ("removed".to_owned(), ErrorParam::from(self.removed)),
                ("shortened".to_owned(), ErrorParam::from(self.shortened)),
            ]
            .into(),
        }
    }

    /// The note in Klipolo's words, as the page's `refitNote` says it in English: "A transition is shortened.", "2
    /// transitions are removed and one is shortened." Keep it in step with the page's `timeline.refit` message
    /// (`apps/desktop/src/lib/labelTable.ts`), which words the same counts in the undo label.
    pub(super) fn said(self) -> String {
        let removed = match self.removed {
            1 => "A transition is removed".to_owned(),
            n => format!("{n} transitions are removed"),
        };
        match (self.removed, self.shortened) {
            (0, 1) => "A transition is shortened.".to_owned(),
            (0, n) => format!("{n} transitions are shortened."),
            (_, 0) => format!("{removed}."),
            (_, 1) => format!("{removed} and one is shortened."),
            (_, n) => format!("{removed} and {n} are shortened."),
        }
    }

    /// Said when the change waits for the person because a transition no longer fits, with any it shortens: to the
    /// person in chat, and to another app beside its proposal.
    pub(super) fn waits(self) -> String {
        let fits = match self.removed {
            1 => "a transition no longer fits".to_owned(),
            n => format!("{n} transitions no longer fit"),
        };
        let shorter = match self.shortened {
            0 => String::new(),
            1 => " and one gets shorter".to_owned(),
            n => format!(" and {n} get shorter"),
        };
        format!("At that speed {fits}{shorter}, so the change waits for Apply.")
    }
}

/// What a `setClip` edit tried on `before` did to transitions: when it refitted any, its label carries the page's note
/// (`chat.setClip.note`) as the card's `speed.undo.note` does, and the refit is returned for the reply.
pub(super) fn note_refit(before: &Project, tried: &mut Tried) -> Option<Refit> {
    let (_, after) = tried.after.as_ref()?;
    let refit = Refit::between(before, after);
    if refit == Refit::default() {
        return None;
    }
    let label = &mut tried.edit.label;
    label.code = "chat.setClip.note".to_owned();
    (label.params).insert(
        "note".to_owned(),
        ErrorParam::Coded(Box::new(refit.coded())),
    );
    Some(refit)
}

/// The transition `kind` lasting `asked` (None: the default) at the cut after `p`'s clip `i`, as the card and Add
/// transition make it, or why not. `now` is the transition there.
fn transition_at(
    p: &Project,
    i: usize,
    kind: TransitionKind,
    asked: Option<Us>,
    now: Option<&Transition>,
) -> Result<Option<Transition>, Room> {
    let fps = p.settings.fps;
    let max = max_transition_us(p, i);
    // One already there keeps its length through a new kind, as the card's Type keeps it: no room is needed.
    let kept = now.is_some() && asked.is_none();
    if !kept && transition_length_us(MIN_TRANSITION_US, max, fps) == 0 {
        return Err(Room::None);
    }
    let length = match asked {
        Some(asked) if asked > max => return Err(Room::AtMost(max)),
        Some(asked) => transition_length_us(asked, max, fps),
        None => match now {
            Some(t) => t.duration_us,
            None => default_transition_us(p, i, fps),
        },
    };
    if length == 0 {
        return Err(Room::None);
    }
    let next = match now {
        // A new kind starts from its own defaults, as the card's Type does.
        Some(t) if t.effect.as_str() == kind.effect() => Transition {
            duration_us: length,
            ..t.clone()
        },
        _ => Transition {
            effect: kind.effect().into(),
            duration_us: length,
            params: Default::default(),
        },
    };
    Ok((now != Some(&next)).then_some(next))
}

/// Why a cut can't take a transition: no spare footage for even the shortest one, or not as much as asked.
enum Room {
    None,
    AtMost(Us),
}

pub(super) fn set_transition(
    c: &Compiler,
    work: &Project,
    after: &ClipRef,
    kind: Option<TransitionKind>,
    asked: Option<Us>,
) -> Result<(Edit, Option<String>), Refused> {
    let main = &work.timeline.main;
    let cuts: Vec<usize> = match after {
        ClipRef::All => (0..main.len().saturating_sub(1)).collect(),
        ClipRef::One(short) => {
            // The cut after its last piece.
            let pieces = named(c, work, "setTransition", after)?;
            let last = pieces.iter().copied().max().unwrap_or_default();
            if last + 1 >= main.len() {
                return Err(to_model(format!(
                    "setTransition: {short} is the last clip, so no cut follows it."
                )));
            }
            vec![last]
        }
    };
    let one = matches!(after, ClipRef::One(_));
    // Each cut fitted beside the transitions the ones before it get, as the model checks the ops in order.
    let mut scratch = work.clone();
    let mut ops = Vec::new();
    let (mut added, mut changed, mut lengths, mut removed, mut no_room) = (0usize, 0, 0, 0, 0);
    for i in cuts {
        let now = scratch.timeline.main[i].transition_out.clone();
        let next = match kind {
            None if now.is_none() => continue,
            None => None,
            Some(kind) => match transition_at(&scratch, i, kind, asked, now.as_ref()) {
                Ok(Some(next)) => Some(next),
                Ok(None) => continue,
                // With every cut asked for, those with no spare footage are left as they are.
                Err(Room::None) if !one => {
                    no_room += 1;
                    continue;
                }
                Err(Room::None) => return Err(to_person(NO_ROOM.to_owned())),
                Err(Room::AtMost(max)) => return Err(to_model(no_room_for(c, work, i, max))),
            },
        };
        match (&now, &next) {
            (_, None) => removed += 1,
            (None, Some(_)) => added += 1,
            // Only its length: the card's Length.
            (Some(was), Some(is)) if was.effect == is.effect && was.params == is.params => {
                lengths += 1
            }
            (Some(_), Some(_)) => changed += 1,
        }
        scratch.timeline.main[i].transition_out = next.clone();
        ops.push(Op::SetTransition {
            clip_id: scratch.timeline.main[i].id.clone(),
            transition: next,
        });
    }
    if ops.is_empty() {
        let none_there = main.iter().all(|clip| clip.transition_out.is_none());
        // Nothing to take away is said when the person asked about transitions (or another app did); the model's own
        // tidying, as a short's plan clearing transitions there never were, changes nothing and says nothing.
        let person_asked = c.by_app || c.inputs.message.to_lowercase().contains("transition");
        match (kind, after) {
            (None, ClipRef::All) if none_there && person_asked => {
                return Err(to_person(NO_TRANSITIONS.to_owned()));
            }
            (None, ClipRef::One(short)) => {
                return Err(to_model(format!(
                    "setTransition: the cut after {short} has no transition to remove."
                )));
            }
            (Some(_), _) if no_room > 0 => return Err(to_person(NO_ROOM_ANYWHERE.to_owned())),
            _ => {}
        }
    }
    let change = if removed > 0 {
        "remove"
    } else if changed + lengths == 0 {
        "add"
    } else if changed + added == 0 {
        "length"
    } else {
        "change"
    };
    let edit = Edit {
        ops,
        label: label(
            "setTransition",
            vec![
                ("after", ErrorParam::from(after.word())),
                ("change", ErrorParam::from(change)),
                (
                    "count",
                    ErrorParam::from(added + changed + lengths + removed),
                ),
            ],
        ),
    };
    // The cuts asked for that got none, said rather than left out quietly.
    let skipped = match no_room {
        0 => None,
        1 => Some("1 cut has no spare footage for a transition, so it has none.".to_owned()),
        n => Some(format!(
            "{n} cuts have no spare footage for a transition, so they have none."
        )),
    };
    Ok((edit, skipped))
}

/// Why the cut after the working copy's clip `i` can't take the length asked: at most `max` (floored to tenths, so the
/// length named always fits, as the model says it). Named by the clip's ref when it has one, else by where the cut is.
fn no_room_for(c: &Compiler, work: &Project, i: usize, max: Us) -> String {
    let main = &work.timeline.main;
    let cut = match c.inputs.refs.ref_of(&main[i].id) {
        Some(r) => format!("the cut after {r}"),
        None => {
            let end: Us = main[..=i].iter().map(Clip::duration_us).sum();
            format!("the cut at {} s", secs(end))
        }
    };
    let tenths = max / 100_000;
    format!(
        "setTransition: {cut} has room for at most {}.{} s.",
        tenths / 10,
        tenths % 10
    )
}
