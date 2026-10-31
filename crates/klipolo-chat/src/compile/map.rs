//! Snapshot time into the working copy, by content: a moment the model named is a snapshot clip and a time in its
//! source, and plays now wherever a piece of that clip plays that source time. Pieces are found by id
//! (`query::piece_of`: a split's or a cut's pieces are `<id>-2`, `-3`) and source. So an earlier call in the same answer
//! never shifts what a later one names: a cut before it, a split, a clip made longer. Footage an earlier call removed
//! maps to nothing.
//!
//! Costs: O(clips) to find the pieces, then O(pieces) per snapshot clip a range covers.

use std::collections::HashMap;

use klipolo_model::Project;
use klipolo_model::ops::Edge;
use klipolo_model::query::{clip_at, clip_starts, main_track_duration, piece_of};
use klipolo_model::time::{RangeUs, Us, merge_ranges, scaled, unscaled};
use klipolo_model::timeline::Clip;

/// The snapshot's main track, to find its content in a working copy.
pub(super) struct Map<'a> {
    snapshot: &'a Project,
    starts: Vec<Us>,
    index: HashMap<&'a str, usize>,
}

/// Where the source moment `source_us` of `c` (which starts at `start`) plays on the timeline.
pub(super) fn timeline_at(c: &Clip, start: Us, source_us: Us) -> Us {
    let into = source_us.clamp(c.source_in_us, c.source_out_us) - c.source_in_us;
    start.saturating_add(scaled(into, c.speed))
}

/// The source moment playing at timeline time `t` in `c` (which starts at `start`), within the clip.
pub(super) fn source_at(c: &Clip, start: Us, t: Us) -> Us {
    let into = unscaled(t.saturating_sub(start).max(0), c.speed);
    c.source_in_us.saturating_add(into).min(c.source_out_us)
}

impl<'a> Map<'a> {
    pub(super) fn new(snapshot: &'a Project) -> Self {
        let main = &snapshot.timeline.main;
        Self {
            snapshot,
            starts: clip_starts(main),
            index: (main.iter().enumerate())
                .map(|(i, c)| (c.id.as_str(), i))
                .collect(),
        }
    }

    /// The snapshot clip with id `id`.
    pub(super) fn index_of(&self, id: &str) -> Option<usize> {
        self.index.get(id).copied()
    }

    /// Snapshot clip `i`, where it starts and ends on the snapshot's timeline.
    pub(super) fn clip(&self, i: usize) -> (&'a Clip, Us, Us) {
        let c = &self.snapshot.timeline.main[i];
        let start = self.starts[i];
        (c, start, start.saturating_add(c.duration_us()))
    }

    /// For each snapshot clip, the working copy's pieces of it: their index and start, in timeline order.
    fn pieces(&self, work: &Project) -> Vec<Vec<(usize, Us)>> {
        let mut out = vec![Vec::new(); self.starts.len()];
        let mut start: Us = 0;
        for (j, c) in work.timeline.main.iter().enumerate() {
            let was = piece_of(&c.id, |id| self.index.contains_key(id), |_| false)
                .and_then(|id| self.index_of(id))
                .filter(|&i| self.snapshot.timeline.main[i].source == c.source);
            if let Some(i) = was {
                out[i].push((j, start));
            }
            start = start.saturating_add(c.duration_us());
        }
        out
    }

    /// Where the snapshot's `r` plays in `work`: each part of it still there, merged. Empty when it is all gone.
    pub(super) fn range(&self, work: &Project, r: RangeUs) -> Vec<RangeUs> {
        let pieces = self.pieces(work);
        let mut out = Vec::new();
        for (i, own) in pieces.iter().enumerate() {
            let (clip, start, end) = self.clip(i);
            if end <= r.start || start >= r.end {
                continue;
            }
            let from = source_at(clip, start, r.start.max(start));
            let to = source_at(clip, start, r.end.min(end));
            for &(j, piece_start) in own {
                let piece = &work.timeline.main[j];
                let (a, b) = (from.max(piece.source_in_us), to.min(piece.source_out_us));
                if a < b {
                    out.push(RangeUs {
                        start: timeline_at(piece, piece_start, a),
                        end: timeline_at(piece, piece_start, b),
                    });
                }
            }
        }
        merge_ranges(&out, main_track_duration(&work.timeline.main))
    }

    /// Where the snapshot's moment `t` plays in `work`, if a clip plays it there and it is still there.
    pub(super) fn time(&self, work: &Project, t: Us) -> Option<Us> {
        let (i, start) = clip_at(&self.snapshot.timeline.main, t)?;
        let source = source_at(&self.snapshot.timeline.main[i], start, t);
        self.pieces(work)[i].iter().find_map(|&(j, piece_start)| {
            let piece = &work.timeline.main[j];
            (piece.source_in_us..piece.source_out_us)
                .contains(&source)
                .then(|| timeline_at(piece, piece_start, source))
        })
    }

    /// The working copy's pieces of snapshot clip `i`.
    pub(super) fn pieces_of<'w>(&self, work: &'w Project, i: usize) -> Vec<&'w Clip> {
        (self.pieces(work).swap_remove(i).into_iter())
            .map(|(j, _)| &work.timeline.main[j])
            .collect()
    }

    /// The piece of snapshot clip `i` in `work` that holds its `edge`: the one reaching furthest that way in its
    /// source.
    pub(super) fn edge_piece<'w>(
        &self,
        work: &'w Project,
        i: usize,
        edge: Edge,
    ) -> Option<&'w Clip> {
        let pieces = self.pieces_of(work, i).into_iter();
        match edge {
            Edge::Start => pieces.min_by_key(|c| c.source_in_us),
            Edge::End => pieces.max_by_key(|c| c.source_out_us),
        }
    }
}
