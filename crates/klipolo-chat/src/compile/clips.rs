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
//!   as the model checks a batch's ops in order.
//!
//! Undo labels: `chat.setClip {clip, change}` (`change` one of speed, volume, mute, unmute, fit, fill, or several), and
//! `chat.setClip.note` with the page's own note (`timeline.refit {removed, shortened}`) when a speed refits
//! transitions, as the card's `speed.undo.note` does; `chat.setTransition {after, change, count}` (`change` one of add,
//! change, remove).

use std::collections::BTreeMap;

use klipolo_model::Project;
use klipolo_model::coded::{CodedError, ErrorParam};
use klipolo_model::framing::effective_framing;
use klipolo_model::ops::{ClipPatch, Op};
use klipolo_model::project::FormatKind;
use klipolo_model::query::clip_has_sound;
use klipolo_model::time::Us;
use klipolo_model::timeline::{Clip, Framing, FramingMode, Transition};
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
    let change = match changed.as_slice() {
        [one] => *one,
        _ => "several",
    };
    Ok(Edit {
        ops,
        label: label(
            "setClip",
            vec![
                ("clip", ErrorParam::from(clip.word())),
                ("change", ErrorParam::from(change)),
            ],
        ),
    })
}

/// `clip`'s framings with `mode` in each of `formats` where it is framed otherwise now, each built on its framing there
/// as the engine draws it (the card's base), the other formats' kept; None when every one is `mode` already.
fn framed(
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
    /// transitions are removed and one is shortened."
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

    /// Said when the change waits for the person because a transition no longer fits.
    pub(super) fn waits(self) -> String {
        let fits = match self.removed {
            1 => "a transition no longer fits".to_owned(),
            n => format!("{n} transitions no longer fit"),
        };
        format!("At that speed {fits}, so the change waits for you.")
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
    if transition_length_us(MIN_TRANSITION_US, max, fps) == 0 {
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
) -> Result<Edit, Refused> {
    let main = &work.timeline.main;
    let cuts: Vec<usize> = match after {
        ClipRef::All => (0..main.len().saturating_sub(1)).collect(),
        ClipRef::One(short) => {
            // The cut after the piece that holds the clip's end.
            let pieces = named(c, work, "setTransition", after)?;
            let last = (pieces.iter().copied())
                .max_by_key(|&i| main[i].source_out_us)
                .unwrap_or_default();
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
    let (mut added, mut changed, mut removed, mut no_room) = (0usize, 0usize, 0usize, 0usize);
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
        match (kind, after) {
            (None, ClipRef::All) if none_there => {
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
    } else if changed == 0 {
        "add"
    } else {
        "change"
    };
    Ok(Edit {
        ops,
        label: label(
            "setTransition",
            vec![
                ("after", ErrorParam::from(after.word())),
                ("change", ErrorParam::from(change)),
                ("count", ErrorParam::from(added + changed + removed)),
            ],
        ),
    })
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
