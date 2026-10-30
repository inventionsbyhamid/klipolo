//! `trimClip`: one end of a clip moves to `toSec`, a timeline time as the video plays in the snapshot.
//! - Its ref resolves through the context's `Refs`; a ref they don't have, or a clip the project no longer has, is
//!   refused.
//! - Inward (shorter) is a removal: `toSec` must lie strictly inside the clip and stand on the evidence, like a cut's
//!   edge (it is padded the same way on a word). It becomes `TrimClip` while the clip is still one piece in the working
//!   copy, else the same footage removed from its pieces.
//! - Outward (longer) is an addition: `toSec` may reach into the clip next to it (or, for the last clip's end, past the
//!   main track's end; the first clip's start can't move before 0), and only as far as the clip's file has footage.
//!   The model would refuse more, so it is refused here rather than clamped.
//!
//! Every refusal of a clip that resolved gives its range and how far that end may move, "trimClip: c2 plays from 10.0
//! to 20.0 s and can grow to 30.0; 45.0 is outside it.", so the model sees where the clip is, inside a folded run too.

use klipolo_model::Project;
use klipolo_model::coded::{CodedError, ErrorParam};
use klipolo_model::ops::{Edge, Op};
use klipolo_model::query::source_duration_us;
use klipolo_model::time::{RangeUs, Us, scaled, unscaled};

use super::map::source_at;
use super::{Compiler, Edit, Refused, label, to_model, us};
use crate::context::secs;

/// A trim checked against the snapshot.
pub(super) struct Trim {
    /// The ref, for the label.
    clip_ref: String,
    /// The snapshot clip.
    clip: usize,
    edge: Edge,
    /// Where the end goes on the snapshot's timeline (snapped, for a shorter clip).
    to_us: Us,
    /// Where the end goes in the clip's source.
    source_us: Us,
    /// For a shorter clip, the snapshot's footage that goes.
    pub(super) removed: Option<RangeUs>,
}

impl Trim {
    fn label(&self) -> CodedError {
        let edge = match self.edge {
            Edge::Start => "start",
            Edge::End => "end",
        };
        label(
            "trimClip",
            vec![
                ("clip", ErrorParam::from(self.clip_ref.as_str())),
                ("edge", ErrorParam::from(edge)),
                ("toUs", us(self.to_us)),
            ],
        )
    }
}

/// Checks `trimClip` against the snapshot: which clip, which way, and how far.
pub(super) fn plan(c: &Compiler, clip_ref: &str, edge: Edge, to_us: Us) -> Result<Trim, Refused> {
    let Some(id) = c.inputs.refs.clip(clip_ref) else {
        return Err(to_model(format!("trimClip: there is no clip {clip_ref}.")));
    };
    let Some(i) = c.map.index_of(id) else {
        return Err(to_model("trimClip: that clip is gone.".to_owned()));
    };
    let (clip, start, end) = c.map.clip(i);
    let main = &c.snapshot.timeline.main;
    let file_us = source_duration_us(c.snapshot, &clip.source).unwrap_or(clip.source_out_us);
    // How far each end may move out: into the clip next to it (past the main track's end for the last clip's end; never
    // before 0), and only as far as the file has footage.
    let latest_end = (main.get(i + 1))
        .map_or(Us::MAX, |next| end.saturating_add(next.duration_us()))
        .min(end.saturating_add(scaled(file_us - clip.source_out_us, clip.speed)));
    let earliest_start = (i.checked_sub(1))
        .map_or(0, |p| start.saturating_sub(main[p].duration_us()))
        .max(start.saturating_sub(scaled(clip.source_in_us, clip.speed)));
    let plays = format!(
        "trimClip: {clip_ref} plays from {} to {} s",
        secs(start),
        secs(end)
    );
    let reach = match edge {
        Edge::End if latest_end > end => format!("{plays} and can grow to {}", secs(latest_end)),
        Edge::End => format!("{plays} and can't grow longer"),
        Edge::Start if earliest_start < start => {
            format!("{plays} and can start as early as {}", secs(earliest_start))
        }
        Edge::Start => format!("{plays} and can't start earlier"),
    };
    let outside = || to_model(format!("{reach}; {} is outside it.", secs(to_us)));
    // The evidence's reason, after the clip's range.
    let on_evidence = |r: Refused| match r.reason.strip_prefix("trimClip: ") {
        Some(why) => to_model(format!("{plays}; {why}")),
        None => r,
    };
    let trim = |to_us, source_us, removed| Trim {
        clip_ref: clip_ref.to_owned(),
        clip: i,
        edge,
        to_us,
        source_us,
        removed,
    };
    match edge {
        Edge::End if to_us == end => Err(to_model(format!(
            "{reach}; it already ends at {}.",
            secs(end)
        ))),
        Edge::Start if to_us == start => Err(to_model(format!(
            "{reach}; it already starts at {}.",
            secs(start)
        ))),
        // Longer, within its reach, else refused rather than clamped (the model would refuse footage the file doesn't
        // have). The `min`s only absorb a sped-up clip's rounding at the very end of its reach.
        Edge::End if to_us > end => {
            if to_us > latest_end {
                return Err(outside());
            }
            let source_us = clip
                .source_out_us
                .saturating_add(unscaled(to_us - end, clip.speed))
                .min(file_us);
            Ok(trim(to_us, source_us, None))
        }
        Edge::Start if to_us < start => {
            if to_us < earliest_start {
                return Err(outside());
            }
            let back = unscaled(start - to_us, clip.speed).min(clip.source_in_us);
            Ok(trim(to_us, clip.source_in_us - back, None))
        }
        // Shorter: what goes is a cut from `toSec` to the clip's edge, which stands on the evidence like one.
        Edge::End => {
            if to_us <= start {
                return Err(outside());
            }
            let removed = c
                .checked(
                    "trimClip",
                    ("toSec", "the clip's end"),
                    RangeUs { start: to_us, end },
                )
                .map_err(on_evidence)?;
            if removed.start <= start {
                return Err(outside());
            }
            let source_us = source_at(clip, start, removed.start);
            Ok(trim(removed.start, source_us, Some(removed)))
        }
        Edge::Start => {
            if to_us >= end {
                return Err(outside());
            }
            let removed = c
                .checked(
                    "trimClip",
                    ("the clip's start", "toSec"),
                    RangeUs { start, end: to_us },
                )
                .map_err(on_evidence)?;
            if removed.end >= end {
                return Err(outside());
            }
            let source_us = source_at(clip, start, removed.end);
            Ok(trim(removed.end, source_us, Some(removed)))
        }
    }
}

/// A longer clip, in the additions' working copy: the piece holding that end moves.
pub(super) fn lengthen(c: &Compiler, work: &Project, t: &Trim) -> Result<Edit, Refused> {
    let Some(piece) = c.map.edge_piece(work, t.clip, t.edge) else {
        return Err(to_model("trimClip: that clip is gone.".to_owned()));
    };
    let further = match t.edge {
        Edge::End => t.source_us > piece.source_out_us,
        Edge::Start => t.source_us < piece.source_in_us,
    };
    if !further {
        return Err(to_model(
            "trimClip: an earlier part of this answer already moves that end further.".to_owned(),
        ));
    }
    Ok(Edit {
        ops: vec![Op::TrimClip {
            id: piece.id.clone(),
            edge: t.edge,
            source_us: t.source_us,
        }],
        label: t.label(),
    })
}

/// A shorter clip, in the removals' working copy: `TrimClip` while the clip is one piece that still holds the new
/// end, else its footage that goes, removed from wherever its pieces play it.
pub(super) fn shorten(c: &Compiler, work: &Project, t: &Trim) -> Result<Edit, Refused> {
    let Some(removed) = t.removed else {
        return Err(to_model(
            "trimClip: that would not make the clip shorter.".to_owned(),
        ));
    };
    let pieces = c.map.pieces_of(work, t.clip);
    let ops = match pieces.as_slice() {
        [one] if one.source_in_us < t.source_us && t.source_us < one.source_out_us => {
            vec![Op::TrimClip {
                id: one.id.clone(),
                edge: t.edge,
                source_us: t.source_us,
            }]
        }
        _ => {
            let ranges = c.map.range(work, removed);
            if ranges.is_empty() {
                return Err(to_model(format!(
                    "trimClip: an earlier part of this answer already removes {} to {} s.",
                    secs(removed.start),
                    secs(removed.end)
                )));
            }
            vec![Op::DeleteRanges { ranges }]
        }
    };
    Ok(Edit {
        ops,
        label: t.label(),
    })
}
