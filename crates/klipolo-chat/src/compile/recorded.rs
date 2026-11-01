//! The recording tools, on the additions' working copy: a recording's camera (`cameras`, and the bubble's place and
//! look in `bubble`) and its recorded sounds (`sources`).
//!
//! A call names clips by the context's refs (`c2`, or a run `c3-c41` as the context folds it), each resolved through
//! the `Refs` the context was built with and then to its pieces in the working copy (`Map::pieces_of`), so a split
//! earlier in the answer still leaves the clip named; none named is every clip.
//!
//! Each is an addition, applied at once in the answer's one undo step. What a call left as it was (a camera that is its
//! own layer already, a sound playing from its own track) is a note for the person (`Noted::notes`), said after the
//! reply; a call that changes nothing but has a note is refused for the person with it, since nothing was done.
//! Refusals the model can mend (a ref that names no clip, a bubble change that names nothing) go back to it; facts
//! about the project (no camera, no such sound, a format the video doesn't have) are said to the person.
//!
//! Undo labels: `chat.setCameraMode {mode}`, `chat.adjustCamera`, `chat.detachCamera`, `chat.muteSource {source,
//! mute}`, `chat.setSourceVolume {source, percent}` and `chat.separateSound {source}`, `source` the tool's word.

use std::collections::HashSet;

use klipolo_model::Project;
use klipolo_model::ids::Id;
use klipolo_model::project::FormatKind;
use klipolo_model::timeline::ClipSource;

use super::{Compiler, Edit, Refused, bubble, cameras, sources, to_model, to_person};
use crate::tools::ToolCall;

/// A recording tool's edit, and what it left as it was, in sentences for the person.
pub(super) struct Noted {
    pub(super) edit: Edit,
    pub(super) notes: Vec<String>,
}

/// `edit` with `notes`; refused for the person with the notes when it changes nothing.
pub(super) fn noted(edit: Edit, notes: Vec<String>) -> Result<Noted, Refused> {
    if edit.ops.is_empty() && !notes.is_empty() {
        return Err(to_person(notes.join(" ")));
    }
    Ok(Noted { edit, notes })
}

/// A recording tool's call on `work`.
pub(super) fn compile(c: &Compiler, work: &Project, call: &ToolCall) -> Result<Noted, Refused> {
    match call {
        ToolCall::SetCameraMode {
            clips,
            mode,
            format,
        } => cameras::set_mode(c, work, clips.as_deref(), *mode, *format),
        ToolCall::AdjustCamera { clips, change } => {
            bubble::adjust(c, work, clips.as_deref(), change)
        }
        ToolCall::DetachCamera { clips } => cameras::detach(c, work, clips.as_deref()),
        ToolCall::MuteSource {
            source,
            muted,
            which,
            clips,
        } => sources::mute(
            c,
            work,
            *source,
            *muted,
            (which.as_deref(), clips.as_deref()),
        ),
        ToolCall::SetSourceVolume {
            source,
            volume,
            which,
            clips,
        } => sources::set_volume(
            c,
            work,
            *source,
            *volume,
            (which.as_deref(), clips.as_deref()),
        ),
        ToolCall::SeparateSound {
            source,
            which,
            clips,
        } => sources::separate(c, work, *source, (which.as_deref(), clips.as_deref())),
        other => Err(to_model(format!("{}: not a recording tool.", other.name()))),
    }
}

/// The working copy's clips that `refs` name for `tool`, by id: each ref's snapshot clip (or each clip of a run
/// `c3-c41`) and the pieces it is in now. None when no clip is named (every clip). A ref the context didn't give is
/// refused back to the model. O(refs × clips).
pub(super) fn named(
    c: &Compiler,
    work: &Project,
    tool: &str,
    refs: Option<&[String]>,
) -> Result<Option<HashSet<Id>>, Refused> {
    let Some(refs) = refs else {
        return Ok(None);
    };
    let index = |short: &str| (c.inputs.refs.clip(short.trim())).and_then(|id| c.map.index_of(id));
    let mut out = HashSet::new();
    for r in refs {
        let (first, last) = (r.split_once(['-', '\u{2013}'])).unwrap_or((r.as_str(), r.as_str()));
        let (Some(i), Some(j)) = (index(first), index(last)) else {
            return Err(to_model(format!("{tool}: there is no clip {r}.")));
        };
        for k in i.min(j)..=i.max(j) {
            out.extend(
                c.map
                    .pieces_of(work, k)
                    .into_iter()
                    .map(|clip| clip.id.clone()),
            );
        }
    }
    Ok(Some(out))
}

/// A recording's name, as Klipolo gives it wherever it shows (the clip lines, the person's sentences): "Recording 2"
/// for the project's second; None for one the project lacks.
pub(crate) fn recording_name(p: &Project, id: &str) -> Option<String> {
    (p.recordings.iter().position(|r| r.id == id)).map(|n| format!("Recording {}", n + 1))
}

/// The name of the recording that clip `clip_id` of `p` plays, for a sentence that starts with it.
pub(super) fn clip_recording(p: &Project, clip_id: &str) -> String {
    let source = (p.timeline.main.iter())
        .find(|clip| clip.id == clip_id)
        .map(|clip| &clip.source);
    match source {
        Some(ClipSource::Recording { id }) => recording_name(p, id),
        _ => None,
    }
    .unwrap_or_else(|| "A recording".to_owned())
}

/// `f`, when the video has that format; else the person is told to add it first.
pub(super) fn has_format(p: &Project, f: FormatKind) -> Result<FormatKind, Refused> {
    if p.settings.formats.contains(&f) {
        return Ok(f);
    }
    let name = match f {
        FormatKind::Landscape => "landscape (16:9)",
        FormatKind::Portrait => "vertical (9:16)",
        FormatKind::Square => "square (1:1)",
    };
    Err(to_person(format!(
        "Your video has no {name} version. Add it first."
    )))
}
