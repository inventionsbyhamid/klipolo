//! A new take's auto zoom: right after its "Add recording" step, the take's clicks become Auto zoom items as a step of
//! their own, "Add auto zoom" (`undo.autoZoom`), so one Undo takes back only the zooms and a second the recording. A
//! take without clicks gets no step.
//!
//! The zooms are planned as the engine plans them, with one `Rules::default()`: the take's cursor file read as the
//! engine reads it (`load_track`), its facts (`TakeFacts::of_cursor`) of that clip only, on the timeline over the
//! clip's span (`on_timeline_between`), framing its clicks (`take_spans`). A piece that overlaps a zoom already there,
//! on any zoom track, is left out, so the person's zooms are never covered. Each item is attached to the clip under
//! it, at 2x at most, on the first zoom track with room (`auto_zoom_ops`).
//!
//! The cursor file is read before the project's lock is taken; the plan is made under it, from the project as it is,
//! so nothing lands between the look and the step.
//!
//! A take whose cursor file is missing or can't be read lands without zooms, with a warning in the log: never an error.
//!
//! Cost: reading the file is O(samples); the plan is O(n log n) in the clip's facts.

use std::path::Path;
use std::sync::Arc;

use klipolo_cursor::camera::Rules;
use klipolo_cursor::facts::StillRuns;
use klipolo_cursor::timeline::{TakeFacts, on_timeline_between};
use klipolo_cursor::zooms::{AUTO_SCALE, CLICKS, auto_zoom_ops, take_spans};
use klipolo_engine::media::cursor::load_track;
use klipolo_model::Op;
use klipolo_model::coded::Said;
use klipolo_model::project::{Project, Recording};
use klipolo_model::query::clip_starts;
use klipolo_model::time::Us;
use klipolo_model::timeline::{ItemContent, TrackKind};

use crate::project::{ApplyError, ProjectSessionState, build_and_apply, with_open};

/// The step's label: "Add auto zoom".
pub(super) const LABEL: &str = "undo.autoZoom";

/// Adds the auto zoom of take `recording_id`, played by clip `clip_id` of project `project_id`'s main track, as one step
/// (the module doc): how many zoom items it added, 0 with no step. A cursor file that is missing or can't be read gives
/// 0; Err when that project isn't the open one any more (`project.moved`) or refuses the step.
pub(super) fn zoom_take(
    sessions: &ProjectSessionState,
    project_id: &str,
    recording_id: &str,
    clip_id: &str,
) -> Result<u32, Said> {
    let Some(file) = cursor_file(sessions, project_id, recording_id)? else {
        return Ok(0);
    };
    let facts = match load_track(Path::new(&file)) {
        Ok(track) => Arc::new(TakeFacts::of_cursor(&track, &StillRuns::of(&track))),
        Err(error) => {
            tracing::warn!(
                %error,
                take = recording_id,
                "recorder: the take's cursor file couldn't be read, so it gets no auto zoom"
            );
            return Ok(0);
        }
    };
    let mut added = 0;
    build_and_apply(sessions, Some(project_id), None, LABEL, |project| {
        let mut mint = || uuid::Uuid::new_v4().to_string();
        let ops = zoom_ops(project, recording_id, clip_id, &facts, &mut mint);
        added = ops.len();
        Ok(ops)
    })
    .map_err(|e| e.said())?;
    tracing::info!(
        take = recording_id,
        zooms = added,
        "recorder: the take's auto zoom was planned"
    );
    Ok(u32::try_from(added).unwrap_or(u32::MAX))
}

/// The cursor file recording `recording_id` names in project `project_id`, the open one; None when it has none (or
/// isn't there).
fn cursor_file(
    sessions: &ProjectSessionState,
    project_id: &str,
    recording_id: &str,
) -> Result<Option<String>, Said> {
    with_open(sessions, |open| {
        let project = open.document.project();
        if project.id != project_id {
            return Err(ApplyError::OtherProject.said());
        }
        let recording = project.recording(recording_id);
        Ok(recording
            .and_then(Recording::cursor_path)
            .map(str::to_owned))
    })
}

/// The take's auto zoom in `project` as it is, from the take's `facts` on its recording clock: none when clip `clip_id`
/// isn't on the main track. Item ids from `mint`.
fn zoom_ops(
    project: &Project,
    recording_id: &str,
    clip_id: &str,
    facts: &Arc<TakeFacts>,
    mint: &mut dyn FnMut() -> String,
) -> Vec<Op> {
    let main = &project.timeline.main;
    let Some(i) = main.iter().position(|c| c.id == clip_id) else {
        return Vec::new();
    };
    let starts = clip_starts(main);
    let span = (starts[i], starts[i].saturating_add(main[i].duration_us()));
    let take = |r: &Recording| (r.id == recording_id).then(|| facts.clone());
    let on_timeline = on_timeline_between(project, &starts, &take, span.0, span.1);
    let rules = Rules::default();
    let spans = take_spans(&on_timeline, CLICKS, span, &zooms_there(project), rules);
    auto_zoom_ops(&spans, AUTO_SCALE, CLICKS, mint)
}

/// Every zoom item there now, on every zoom track (Auto and Rect, shown or hidden): its span on the timeline.
fn zooms_there(project: &Project) -> Vec<(Us, Us)> {
    (project.timeline.tracks.iter())
        .filter(|t| t.kind == TrackKind::Zoom)
        .flat_map(|t| &t.items)
        .filter(|i| matches!(i.content, ItemContent::Zoom { .. }))
        .map(|i| (i.start_us, i.end_us()))
        .collect()
}
