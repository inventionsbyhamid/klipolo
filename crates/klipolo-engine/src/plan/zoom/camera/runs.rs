//! The Auto runs of a project's zoom items and their keys (`split`): which Auto zooms are camera runs, which zooms
//! are fixed shots inside them, and what each run's plan reads.

use klipolo_cursor::camera::{Fixed, View};
use klipolo_model::cursor_style::FocusKind;
use klipolo_model::layout::BoxN;
use klipolo_model::project::{Project, Recording};
use klipolo_model::time::Us;
use klipolo_model::timeline::{ClipSource, ZoomTarget};

use super::super::super::takes::Takes;
use super::super::ZoomItem;
use super::{AutoRun, CameraKey, Known, TakeKey, auto_rect};

/// The view a person's Rect zoom shows: its rectangle's centre at its scale, kept inside the frame.
fn view_of(rect: &BoxN, scale: f64) -> View {
    let scale = if scale.is_finite() {
        scale.max(1.0)
    } else {
        1.0
    };
    let half = 0.5 / scale;
    let at = |c: f64| c.clamp(half, 1.0 - half);
    View {
        cx: at(rect.x + rect.w / 2.0),
        cy: at(rect.y + rect.h / 2.0),
        scale,
    }
}

/// `items` (the visible zoom items, sorted by start) as the springs' items and the Auto runs: each Auto item over
/// cursor data is a run (one starting inside an earlier run starts where it ends); one over none is a centred spring
/// item; Rect items wholly inside a run are its fixed shots, the others springs (those overlapping a run are its fixed
/// shots there too). O(items × log runs + clips under the runs).
pub(in crate::plan::zoom) fn split(
    items: Vec<ZoomItem>,
    p: &Project,
    starts: &[Us],
    takes: &Takes,
) -> (Vec<ZoomItem>, Vec<AutoRun>) {
    let mut runs: Vec<AutoRun> = Vec::new();
    let mut springs = Vec::with_capacity(items.len());
    for (start, end, scale, target) in &items {
        let ZoomTarget::Auto { using } = target else {
            continue;
        };
        let start = runs.last().map_or(*start, |r| r.end.max(*start));
        if start >= *end {
            continue;
        }
        match key((p, starts, takes), start, *end, *scale, using) {
            Some((key, facts)) => runs.push(AutoRun {
                key,
                start,
                end: *end,
                scale: *scale,
                using: using.clone(),
                fixed: Vec::new(),
                facts,
            }),
            None => springs.push((start, *end, *scale, centred(*scale))),
        }
    }
    for item in items {
        let ZoomTarget::Rect { rect } = &item.3 else {
            continue;
        };
        let (from, to) = (item.0, item.1);
        let first = runs.partition_point(|r| r.end <= from);
        let mut inside = false;
        for r in runs[first..].iter_mut().take_while(|r| r.start < to) {
            let (a, b) = (from.max(r.start) - r.start, to.min(r.end) - r.start);
            let view = view_of(rect, item.2);
            r.fixed.push(Fixed {
                from: a,
                to: b,
                view,
            });
            r.key
                .fixed
                .push((a, b, [view.cx, view.cy, view.scale].map(f64::to_bits)));
            inside |= from >= r.start && to <= r.end;
        }
        if !inside {
            springs.push(item);
        }
    }
    springs.sort_by_key(|i| (i.0, i.1));
    (springs, runs)
}

fn centred(scale: f64) -> ZoomTarget {
    ZoomTarget::Rect {
        rect: auto_rect(None, scale),
    }
}

/// The key of an Auto item over `[start, end)` and the facts of the takes under it as the key names them, or None when
/// no clip touching it has a take with cursor samples.
fn key(
    (p, starts, takes): (&Project, &[Us], &Takes),
    start: Us,
    end: Us,
    scale: f64,
    using: &[FocusKind],
) -> Option<(CameraKey, Vec<Known>)> {
    let mut facts: Vec<Known> = Vec::new();
    let main = &p.timeline.main;
    let first = starts.partition_point(|&s| s < start).saturating_sub(1);
    let last = starts.partition_point(|&s| s <= end);
    let mut data = false;
    let mut clips = Vec::with_capacity(last.saturating_sub(first));
    for (clip, &at) in main[first.min(last)..last]
        .iter()
        .zip(&starts[first.min(last)..last])
    {
        let rec = match &clip.source {
            ClipSource::Recording { id } => p.recording(id),
            ClipSource::Asset { .. } => None,
        };
        let take = rec.and_then(|r| take_key(takes, r, &mut facts));
        data |= take.as_ref().is_some_and(|t| t.1 > 0);
        let window = (at - start, clip.source_in_us, clip.source_out_us);
        clips.push((window.0, window.1, window.2, clip.speed.to_bits(), take));
    }
    let key = CameraKey {
        len: end - start,
        scale: scale.to_bits(),
        using: using.to_vec(),
        fixed: Vec::new(),
        clips,
    };
    data.then_some((key, facts))
}

/// A take's part of a key (its file, its samples, its activity and its screen's nudge), its facts of that same
/// activity added to `facts` once.
fn take_key(takes: &Takes, r: &Recording, facts: &mut Vec<Known>) -> Option<TakeKey> {
    let (path, take) = (takes.key_of(r)?, takes.of_recording(r)?);
    let nudge = r.screen().map_or(0, |s| s.nudge_us);
    let (print, known) = takes.known_of(r)?;
    if !facts.iter().any(|(p, _)| p == path) {
        facts.push((path.clone(), known.clone()));
    }
    Some((path.clone(), take.track.len(), print, nudge))
}
