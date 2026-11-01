//! `autoZoom` on the additions' working copy: the clicks of every recording clip on the main track as Auto zoom
//! items, planned as a new take's auto zoom plans them (`klipolo_cursor::zooms::take_spans` over each clip's span with
//! one `Rules::default()`, each item attached to the clip under it on the first zoom track with room), or every Auto zoom
//! over a recording clip taken away. A zoom already there is never covered (a piece overlapping one is left out), and a
//! zoom the person framed by hand (a Rect zoom) is never taken away, so both are additions: applied at once as one undo
//! step, never proposed.
//!
//! A take's facts come from `Inputs::cursor` (the app reads each take's cursor file); a recording without them has no
//! clicks to zoom on. What can't be done is said to the person: it is a fact about the project.

use klipolo_cursor::camera::Rules;
use klipolo_cursor::timeline::on_timeline_between;
use klipolo_cursor::zooms::{AUTO_SCALE, CLICKS, auto_zoom_ops, take_spans};
use klipolo_model::Project;
use klipolo_model::coded::ErrorParam;
use klipolo_model::ids::new_id;
use klipolo_model::ops::Op;
use klipolo_model::project::Recording;
use klipolo_model::query::clip_starts;
use klipolo_model::time::Us;
use klipolo_model::timeline::{ClipSource, ItemContent, TrackKind, ZoomTarget};

use super::{Compiler, Edit, Refused, label, to_person};

/// What the person is told when the main track plays no recording.
pub const NO_RECORDING: &str = "There is no screen recording to zoom in on.";
/// What the person is told when Klipolo knows no click in the recordings.
pub const NO_CLICKS: &str = "These screen recordings have no clicks to zoom in on.";
/// What the person is told when every click already has a zoom.
pub const ZOOMED_ALREADY: &str = "The clicks are zoomed in on already.";
/// What the person is told when there is no auto zoom to take away.
pub const NO_ZOOMS: &str = "There are no auto zooms to take away.";

/// Every zoom item there now, on every zoom track (Auto and Rect, shown or hidden): its span on the timeline.
fn zooms_there(work: &Project) -> Vec<(Us, Us)> {
    (work.timeline.tracks.iter())
        .filter(|t| t.kind == TrackKind::Zoom)
        .flat_map(|t| &t.items)
        .filter(|i| matches!(i.content, ItemContent::Zoom { .. }))
        .map(|i| (i.start_us, i.end_us()))
        .collect()
}

pub(super) fn auto_zoom(
    c: &Compiler,
    work: &Project,
    on: bool,
    scale: Option<f64>,
) -> Result<Edit, Refused> {
    let main = &work.timeline.main;
    let starts = clip_starts(main);
    let spans: Vec<(Us, Us)> = (main.iter().zip(&starts))
        .filter(|(clip, _)| matches!(clip.source, ClipSource::Recording { .. }))
        .map(|(clip, &start)| (start, start.saturating_add(clip.duration_us())))
        .collect();
    if spans.is_empty() {
        return Err(to_person(NO_RECORDING.to_owned()));
    }
    if !on {
        let over = |from: Us, to: Us| spans.iter().any(|&(a, b)| from < b && a < to);
        let ops: Vec<Op> = (work.timeline.tracks.iter())
            .filter(|t| t.kind == TrackKind::Zoom)
            .flat_map(|t| &t.items)
            .filter(|i| {
                matches!(
                    i.content,
                    ItemContent::Zoom {
                        target: ZoomTarget::Auto { .. },
                        ..
                    }
                ) && over(i.start_us, i.end_us())
            })
            .map(|i| Op::RemoveItem { id: i.id.clone() })
            .collect();
        if ops.is_empty() {
            return Err(to_person(NO_ZOOMS.to_owned()));
        }
        let count = ops.len();
        return Ok(Edit {
            ops,
            label: label(
                "autoZoom",
                vec![
                    ("on", ErrorParam::from("off")),
                    ("count", ErrorParam::from(count)),
                ],
            ),
        });
    }
    let take = |r: &Recording| (c.inputs.cursor.as_ref()).and_then(|t| t.facts(r));
    let rules = Rules::default();
    let mut taken = zooms_there(work);
    let (mut found, mut clicked) = (Vec::new(), false);
    for &(from, to) in &spans {
        let facts = on_timeline_between(work, &starts, &take, from, to);
        clicked |= (facts.clicks.iter()).any(|k| (from..to).contains(&k.timeline_us));
        let pieces = take_spans(&facts, CLICKS, (from, to), &taken, rules);
        taken.extend(pieces.iter().copied());
        found.extend(pieces);
    }
    let mut mint = new_id;
    let ops = auto_zoom_ops(&found, scale.unwrap_or(AUTO_SCALE), CLICKS, &mut mint);
    if ops.is_empty() {
        let said = if clicked { ZOOMED_ALREADY } else { NO_CLICKS };
        return Err(to_person(said.to_owned()));
    }
    let count = ops.len();
    Ok(Edit {
        ops,
        label: label(
            "autoZoom",
            vec![
                ("on", ErrorParam::from("on")),
                ("count", ErrorParam::from(count)),
            ],
        ),
    })
}
