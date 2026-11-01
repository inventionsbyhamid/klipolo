//! Export's cursors before its first frame.
//!
//! - **Takes** (`load_cursors`): every take the project wants, read with the smoothed paths its clips draw with, as
//!   the preview holds them (`CursorTracks::load_all`: the same order and the same budget), so export draws and zooms
//!   as the preview did.
//! - **Never dropped silently** (`missing`): a take whose frames lack the cursor somewhere (`Separate`, `BurnedFrom`)
//!   and which some clip draws, but which the cache doesn't hold (its file can't be read, `load_take` failed, or the
//!   takes before it fill the budget), is missing. A run that draws frames where it is drawn then stops
//!   before anything is written, once its footage is checked (`Loaded::question_in`: `ErrorCode::CursorMissing`,
//!   naming its recording and why, and how many finished parts already show it), unless the person allowed it: then
//!   it plans as no take (nothing is held for it, so nothing is drawn) and says so (`Loaded::notes_in`). Both cover
//!   only the parts still to render: a run that draws nothing (it only joins, or only renders the
//!   sound), or only parts where that recording isn't drawn, neither asks nor says it: those frames were drawn with
//!   the cursor.
//! - **Shapes** (`cursor_shapes`, `prepare_cursors`; a pass's formats: `preload_formats`): before a pass's first
//!   frame, every shape its clips draw goes in each format's atlas (none, and no cursor pass, when no clip draws
//!   one), so frame 0 has the real shape, never the Arrow stand-in. A shape whose image doesn't decode is refused on
//!   its take (`CursorTracks::refuse_shape`): every frame then plans the Arrow look there, as the preview does once its
//!   decoder finds the same, so their pixels still match. The atlas says which failed (`CursorPass::failed`), and the
//!   shapes it has no room for yet are decoded once here to check them, so every refusal is known before
//!   frame 1.
//!
//! **Cost:** the load is IO and smoothing, O(samples) a take, off the runtime's threads; the preload decodes each shape
//! once (≤ 256 × 256) and uploads it. Neither runs per frame.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use klipolo_model::coded::CodedError;
use klipolo_model::cursor_style::CursorCapture;
use klipolo_model::project::Project;
use klipolo_model::time::Us;
use klipolo_model::timeline::ClipSource;

use crate::api::{
    EXPORT_CURSOR_DROPPED, EXPORT_CURSOR_MISSING, EXPORT_CURSOR_OVER_BUDGET,
    EXPORT_CURSOR_OVER_BUDGET_DROPPED,
};
use crate::export::protocol::ErrorCode;
use crate::export::shared::WorkerError;
use crate::media::cursor::{ActivityOf, CursorTracks, Want, wanted};
use crate::plan::{PlanContext, shapes_in};
use crate::render::cursor::{CursorPass, decode};
use crate::render::device::RenderDevice;
use crate::render::export::FormatTargets;

/// Export's cursor takes (`load_cursors`).
pub struct Loaded {
    pub cursors: Arc<CursorTracks>,
    /// The recordings whose cursor is drawn over frames without one but isn't held, in timeline order.
    pub missing: Vec<Missing>,
}

/// A recording whose cursor an export can't draw: its index in `Project::recordings`, whether its file reads but the
/// takes before it fill the budget rather than it can't be read, and where the timeline draws it (its
/// clips' spans, in order).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Missing {
    pub recording: usize,
    pub over_budget: bool,
    pub spans: Vec<(Us, Us)>,
}

/// The whole timeline, for a run that renders every part.
const EVERYWHERE: &[(Us, Us)] = &[(0, Us::MAX)];

impl Loaded {
    /// [`Loaded::question_in`] for a run that renders the whole timeline and has finished nothing.
    pub fn question(&self) -> Option<WorkerError> {
        self.question_in(EVERYWHERE, &[])
    }

    /// The question a run asks before it draws `left` (the timeline ranges of its parts still to render) without a
    /// cursor (`ErrorCode::CursorMissing`): the first missing recording drawn there, named, and why
    /// (`exports.cursorMissing` or `exports.cursorOverBudget`). With `drawnParts` when some of `done` (the finished
    /// parts' ranges) already show its cursor, so the person can start over rather than get a file that shows it only
    /// in places (the mixed output). None when every cursor drawn in `left` is held. O(parts × log clips).
    pub fn question_in(&self, left: &[(Us, Us)], done: &[(Us, Us)]) -> Option<WorkerError> {
        let first = self.missing.iter().find(|m| drawn_in(m, left))?;
        let code = match first.over_budget {
            false => EXPORT_CURSOR_MISSING,
            true => EXPORT_CURSOR_OVER_BUDGET,
        };
        let shown = done.iter().filter(|&&r| meets(&first.spans, r)).count();
        let mut said = naming(code, first.recording);
        if shown > 0 {
            said = said.with("drawnParts", shown);
        }
        Some(WorkerError {
            code: ErrorCode::CursorMissing,
            message: serde_json::to_string(&said).unwrap_or_default(),
            retryable: false,
        })
    }

    /// [`Loaded::notes_in`] over the whole timeline.
    pub fn notes(&self) -> Vec<CodedError> {
        self.notes_in(EVERYWHERE)
    }

    /// With the person's yes, what a run that draws `left` says of each missing recording drawn there
    /// (`exports.note.cursorDropped` or `exports.note.cursorOverBudget`).
    pub fn notes_in(&self, left: &[(Us, Us)]) -> Vec<CodedError> {
        let code = |m: &Missing| match m.over_budget {
            false => EXPORT_CURSOR_DROPPED,
            true => EXPORT_CURSOR_OVER_BUDGET_DROPPED,
        };
        (self.missing.iter())
            .filter(|m| drawn_in(m, left))
            .map(|m| naming(code(m), m.recording))
            .collect()
    }
}

/// Whether `m`'s cursor is drawn somewhere in `ranges`.
fn drawn_in(m: &Missing, ranges: &[(Us, Us)]) -> bool {
    ranges.iter().any(|&r| meets(&m.spans, r))
}

/// Whether `[a, b)` meets one of `spans` (in order, apart). O(log spans).
fn meets(spans: &[(Us, Us)], (a, b): (Us, Us)) -> bool {
    let i = spans.partition_point(|&(_, end)| end <= a);
    spans.get(i).is_some_and(|&(start, _)| start < b)
}

/// Export's cursor takes for `project`, in a cache of the engine's budget, and the recordings whose cursor it can't
/// draw (see the module notes). Blocks: IO and smoothing.
pub fn load_cursors(project: &Project) -> Loaded {
    load_cursors_into(CursorTracks::default(), project)
}

/// `load_cursors` of `wants` (`project`'s `wanted`, found once by the caller) with the screens' activity export made
/// before its first frame (`activity`, from `Analysed::source`), so its takes' facts and follow paths are the
/// preview's.
pub fn load_cursors_with(
    project: &Project,
    wants: &[Want],
    activity: Option<ActivityOf>,
) -> Loaded {
    let tracks = CursorTracks::default();
    if let Some(activity) = activity {
        tracks.set_activity(activity);
    }
    load_wants(tracks, project, wants)
}

/// `load_cursors` into `tracks` (a test passes a small budget).
pub fn load_cursors_into(tracks: CursorTracks, project: &Project) -> Loaded {
    load_wants(tracks, project, &wanted(project))
}

fn load_wants(tracks: CursorTracks, project: &Project, wants: &[Want]) -> Loaded {
    tracks.load_all(wants);
    let missing = missing(project, &tracks);
    for m in &missing {
        tracing::warn!(
            recording = m.recording + 1,
            over_budget = m.over_budget,
            "export: a recording's cursor isn't held"
        );
    }
    Loaded {
        cursors: Arc::new(tracks),
        missing,
    }
}

/// `code` with recording `n`'s name as `recording` ("Recording 3", the model's `name.recording`).
fn naming(code: &str, n: usize) -> CodedError {
    let name = klipolo_model::coded_msg!(
        klipolo_model::messages::MESSAGES,
        "name.recording",
        number = n + 1
    );
    CodedError::new(code).with("recording", name)
}

/// The recordings (in timeline order, each once) whose cursor some main clip draws over frames without one, but which
/// `cursors` doesn't hold: read and refused, or never read (the takes before it fill the budget), each with the spans
/// of its clips that draw it. O(clips + recordings), plus a lookup per drawn clip.
fn missing(p: &Project, cursors: &CursorTracks) -> Vec<Missing> {
    let index: HashMap<&str, usize> = (p.recordings.iter().enumerate())
        .map(|(n, r)| (r.id.as_str(), n))
        .collect();
    let mut out: Vec<Missing> = Vec::new();
    let mut at: HashMap<usize, usize> = HashMap::new();
    let mut start: Us = 0;
    for clip in &p.timeline.main {
        let span = (start, start.saturating_add(clip.duration_us()));
        start = span.1;
        let ClipSource::Recording { id } = &clip.source else {
            continue;
        };
        let Some(&n) = index.get(id.as_str()) else {
            continue;
        };
        let Some(cursor) = p.recordings[n].cursor.as_ref() else {
            continue;
        };
        let drawn =
            cursor.capture != CursorCapture::Burned && !p.cursor_for(clip).effective().hidden;
        if !drawn || cursors.get(&cursor.path).is_some() {
            continue;
        }
        let i = *at.entry(n).or_insert_with(|| {
            out.push(Missing {
                recording: n,
                over_budget: !cursors.unreadable(&cursor.path),
                spans: Vec::new(),
            });
            out.len() - 1
        });
        out[i].spans.push(span);
    }
    out
}

/// Every shape the clips of `project` draw over the whole timeline (`shapes_in`), each once: what each atlas gets
/// before frame 1. Empty when no clip draws a shape, and then no cursor pass is made for it.
pub fn cursor_shapes(project: &Project, cursors: &CursorTracks) -> Vec<(Arc<str>, u16, Arc<str>)> {
    shapes_in(project, cursors, 0, Us::MAX)
}

/// Before an export pass's first frame: every shape its clips draw (`cursor_shapes`) in each of its formats' atlases
/// (`prepare_cursors`; one refused in an earlier format is left out of the next), and no cursor pass where no clip
/// draws a shape. How many were refused. Blocks: decoding and uploads.
pub fn preload_formats(ctx: &PlanContext, targets: &mut FormatTargets) -> usize {
    let shapes = cursor_shapes(&ctx.project, &ctx.cursors);
    if shapes.is_empty() {
        return 0;
    }
    (targets.iter_mut())
        .map(|target| {
            (target.compositor_mut())
                .with_cursor_pass(|cursor, dev| prepare_cursors(&ctx.cursors, &shapes, cursor, dev))
        })
        .sum()
}

/// Before a pass's first frame (each export format, a headless render): `shapes` (`cursor_shapes`) put in `pass`'s
/// atlas, and those whose image can't be used refused on their takes (see the module notes); one an earlier format
/// refused already is left out. How many were refused. Blocks: decoding and uploads.
pub fn prepare_cursors(
    cursors: &CursorTracks,
    shapes: &[(Arc<str>, u16, Arc<str>)],
    pass: &mut CursorPass,
    dev: &RenderDevice,
) -> usize {
    let usable = |take: &str, shape| {
        cursors
            .get(take)
            .is_some_and(|t| t.shape_file(shape).is_some())
    };
    let shapes: Vec<_> = shapes
        .iter()
        .filter(|(take, shape, _)| usable(take, *shape))
        .cloned()
        .collect();
    // Its own warning for each image that can't be used; the atlas keeps which (`failed`).
    let _ = pass.preload(&dev.device, &dev.queue, &shapes);
    let mut refused = 0;
    for (take, shape, png) in &shapes {
        let unusable =
            pass.failed(take, *shape) || (!pass.resident(take, *shape) && unreadable(take, png));
        if unusable {
            cursors.refuse_shape(take, *shape);
            refused += 1;
        }
    }
    if refused > 0 {
        tracing::warn!(
            refused,
            "export: cursor shapes that can't be read draw as the arrow"
        );
    }
    refused
}

/// A shape the atlas had no room for yet, decoded to check it (it loads again when a frame draws it).
fn unreadable(take: &Arc<str>, png: &Arc<str>) -> bool {
    decode(Path::new(&**take), Path::new(&**png))
        .inspect_err(|error| {
            tracing::warn!(file = %crate::log_name(&**png), %error, "cursor shape: drawn as the arrow");
        })
        .is_err()
}
