//! Roll, slip and slide: footage moves across a cut or through a clip while the video keeps its length. Nothing past the
//! edit moves, so free layers stay where they are. The clips' new points are held to their media, to the footage their
//! transitions blend (each keeps its length) and to layers on locked tracks.
//!
//! Layers attached to footage that still shows stay with it (their anchors are in source time). Those attached to
//! footage the edit hides keep their place and are attached to the footage that plays there now. A recording's
//! separated sound and camera (`ItemContent::tie`) are their clip's own footage: one at an edge that moves follows that
//! edge, and a slip slips them with the clip, each held to its own file.

use crate::error::{invalid, not_found, Result};
use crate::messages::code;
use crate::motion;
use crate::project::Project;
use crate::query::{anchor_at, clip_starts, locate_one, source_duration_us};
use crate::time::{drifted, unscaled, Us, MAX_TIME_US, MIN_DURATION_US};
use crate::timeline::{Attach, Clip, Item, ItemContent};
use crate::transitions::refitted;

use super::item_ops::{advance_source, end_at, fit_fades};
use super::main_ops::clamp_fades;
use super::places::Places;

const NO_FOOTAGE: &str = code!("model.edit.noFootage");
const TOO_SHORT: &str = code!("model.clip.minLength");
const LOCKED: &str = code!("model.edit.locked");
const SEPARATED: &str = code!("model.edit.separated");
/// A file may be read this far past its end (validation's slack).
const SLACK_US: Us = 1_000;
/// How near (µs) a separated layer's edge must be to its clip's to follow it.
const AT_EDGE_US: Us = 1_000;

fn clip_index(p: &Project, id: &str, places: &mut Places) -> Result<usize> {
    places
        .clip(&p.timeline.main, id)
        .ok_or_else(|| not_found("clip", id))
}

fn check_shift(by: Us) -> Result<()> {
    if (-MAX_TIME_US..=MAX_TIME_US).contains(&by) {
        Ok(())
    } else {
        Err(invalid(code!("model.project.timeOutside")))
    }
}

/// `clip` with its in point (`start`) and out point (`end`) moved `by` of timeline, in its source time at its speed.
fn moved(clip: &Clip, by: Us, start: bool, end: bool) -> Clip {
    let mut c = clip.clone();
    let source = unscaled(by, c.speed);
    if start {
        c.source_in_us = c.source_in_us.saturating_add(source);
    }
    if end {
        c.source_out_us = c.source_out_us.saturating_add(source);
    }
    clamp_fades(&mut c);
    c
}

pub(super) fn roll(p: &mut Project, clip_id: &str, by: Us, places: &mut Places) -> Result<()> {
    check_shift(by)?;
    let i = clip_index(p, clip_id, places)?;
    let main = &p.timeline.main;
    if i + 1 >= main.len() {
        return Err(invalid(code!("model.roll.noCut")));
    }
    if by == 0 {
        return Ok(());
    }
    let clips = vec![
        (i, moved(&main[i], by, false, true)),
        (i + 1, moved(&main[i + 1], by, true, false)),
    ];
    retime(p, clips, None)
}

pub(super) fn slip(p: &mut Project, clip_id: &str, by: Us, places: &mut Places) -> Result<()> {
    check_shift(by)?;
    let i = clip_index(p, clip_id, places)?;
    if by == 0 {
        return Ok(());
    }
    let clip = &p.timeline.main[i];
    let source = unscaled(by, clip.speed);
    let clips = vec![(i, moved(clip, by, true, true))];
    retime(p, clips, Some((i, source)))
}

pub(super) fn slide(p: &mut Project, clip_id: &str, by: Us, places: &mut Places) -> Result<()> {
    check_shift(by)?;
    let i = clip_index(p, clip_id, places)?;
    let main = &p.timeline.main;
    if i == 0 || i + 1 >= main.len() {
        return Err(invalid(code!("model.slide.alone")));
    }
    if by == 0 {
        return Ok(());
    }
    let clips = vec![
        (i - 1, moved(&main[i - 1], by, false, true)),
        (i + 1, moved(&main[i + 1], by, true, false)),
    ];
    retime(p, clips, None)
}

/// What the edit does to one attached layer.
enum Change {
    /// A separated layer of the slipped clip: its file moves by this much of the clip's source time, in place.
    Slip(Us),
    /// A separated layer at a clip's edge that moves: its new start and end.
    Edges { start: Us, end: Us },
    /// Its footage is hidden: it keeps its place, attached to what plays there now.
    Reanchor(Option<Attach>),
}

/// Puts `clips` (index, new points) in, then checks and moves the layers attached to them (see the module). `slipped`:
/// the slipped clip and its source shift.
fn retime(p: &mut Project, clips: Vec<(usize, Clip)>, slipped: Option<(usize, Us)>) -> Result<()> {
    for (_, c) in &clips {
        let length = source_duration_us(p, &c.source).unwrap_or(c.source_out_us);
        if c.source_in_us < 0 || c.source_out_us > length.saturating_add(SLACK_US) {
            return Err(invalid(NO_FOOTAGE));
        }
        if c.duration_us() < MIN_DURATION_US {
            return Err(invalid(TOO_SHORT));
        }
    }
    let fitted = refitted(p);
    let old = p.timeline.main.clone();
    for (i, c) in clips.iter() {
        p.timeline.main[*i] = c.clone();
    }
    // Each transition keeps its length: none may need footage the edit took.
    if refitted(p) != fitted {
        return Err(invalid(code!("model.edit.transition")));
    }
    let (old_starts, new_starts) = (clip_starts(&old), clip_starts(&p.timeline.main));
    let span = |main: &[Clip], starts: &[Us], i: usize| {
        (starts[i], starts[i].saturating_add(main[i].duration_us()))
    };
    // Each changed clip's edges: (index, old start, new start, old end, new end).
    let edges: Vec<(usize, Us, Us, Us, Us)> = clips
        .iter()
        .map(|&(i, _)| {
            let (s0, e0) = span(&old, &old_starts, i);
            let (s1, e1) = span(&p.timeline.main, &new_starts, i);
            (i, s0, s1, e0, e1)
        })
        .collect();
    let mut changes = Vec::new();
    for (t, track) in p.timeline.tracks.iter().enumerate() {
        for (k, item) in track.items.iter().enumerate() {
            let Some(attach) = &item.attach else {
                continue;
            };
            let Some((was, at)) = locate_one(&old, attach) else {
                continue;
            };
            let tied = item.content.tie().is_some();
            let end = at.saturating_add(item.duration_us);
            let edge = edges.iter().find(|e| tied && e.0 == was);
            let at_start = edge.filter(|e| e.1 != e.2 && (at - e.1).abs() <= AT_EDGE_US);
            let at_end = edge.filter(|e| e.3 != e.4 && (end - e.3).abs() <= AT_EDGE_US);
            let change = if tied && slipped.is_some_and(|(c, _)| c == was) {
                slipped.map(|(_, source)| Change::Slip(source))
            } else if at_start.is_some() || at_end.is_some() {
                let start = at_start.map_or(at, |e| e.2);
                let end = at_end.map_or(end, |e| e.4);
                if end.saturating_sub(start) < MIN_DURATION_US {
                    return Err(invalid(SEPARATED));
                }
                Some(Change::Edges { start, end })
            } else {
                match locate_one(&p.timeline.main, attach) {
                    // Still played by its clip: it moves with its footage, which a locked track doesn't allow.
                    Some((now, to)) if now == was => {
                        if track.locked && to != at {
                            return Err(invalid(LOCKED));
                        }
                        None
                    }
                    _ if tied => return Err(invalid(SEPARATED)),
                    _ => Some(Change::Reanchor(anchor_at(&p.timeline.main, at))),
                }
            };
            let Some(change) = change else {
                continue;
            };
            let moves = !matches!(change, Change::Reanchor(_));
            if track.locked && moves {
                return Err(invalid(LOCKED));
            }
            if let Some(footage) = footage_after(p, item, at, &change) {
                if !footage {
                    return Err(invalid(NO_FOOTAGE));
                }
            }
            changes.push(((t, k), at, change));
        }
    }
    let main = &p.timeline.main;
    for ((t, k), at, change) in changes {
        let item = &mut p.timeline.tracks[t].items[k];
        item.start_us = at;
        match change {
            Change::Slip(source) => {
                shift_file(&mut item.content, source);
                if let Some(a) = item.attach.as_mut() {
                    a.source_us = a.source_us.saturating_add(source);
                }
            }
            Change::Edges { start, end } => {
                if start != at {
                    let offset = start.saturating_sub(at);
                    let len = item.end_us().saturating_sub(start);
                    advance_source(&mut item.content, offset);
                    item.start_us = start;
                    item.duration_us = len;
                    item.layout.motion = item.layout.motion.take().and_then(|mut m| {
                        m.trim_start(offset, len);
                        motion::kept(m)
                    });
                    item.attach = anchor_at(main, start);
                    fit_fades(item);
                }
                if end != item.end_us() {
                    end_at(item, end);
                }
            }
            Change::Reanchor(anchor) => item.attach = anchor,
        }
    }
    Ok(())
}

/// A sound or video layer's file moved `source` of its recording's time: a sound at its file's drift.
fn shift_file(content: &mut ItemContent, source: Us) {
    match content {
        ItemContent::Video { source_in_us, .. } => {
            *source_in_us = source_in_us.saturating_add(source);
        }
        ItemContent::Audio {
            source_in_us,
            drift_ppm,
            ..
        } => *source_in_us = source_in_us.saturating_add(drifted(source, *drift_ppm)),
        _ => {}
    }
}

/// Whether a separated layer (at `at`) still plays inside its file after `change`; None for any other layer or change.
fn footage_after(p: &Project, item: &Item, at: Us, change: &Change) -> Option<bool> {
    let (file, _) = item.content.played()?;
    let length = p.asset(file)?.duration_us;
    let (from, duration) = match change {
        Change::Slip(source) => {
            let mut content = item.content.clone();
            shift_file(&mut content, *source);
            (content.played()?.1, item.duration_us)
        }
        Change::Edges { start, end } => {
            let mut content = item.content.clone();
            advance_source(&mut content, start.saturating_sub(at));
            (content.played()?.1, end.saturating_sub(*start))
        }
        Change::Reanchor(_) => return None,
    };
    let footage = match &item.content {
        ItemContent::Video { speed, .. } => unscaled(duration, *speed),
        ItemContent::Audio { drift_ppm, .. } => drifted(duration, *drift_ppm),
        _ => return None,
    };
    Some(from >= 0 && from.saturating_add(footage) <= length.saturating_add(SLACK_US))
}
