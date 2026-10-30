//! Pauses after travel: where the person moved the cursor to something and held it there, a focus for
//! the camera and a fact for chat.

use serde::{Deserialize, Serialize};

use super::still::{Diagonal, StillRuns};
use crate::{CursorTrack, Us};

/// The cursor resting at (x, y) over [from, to] on the recording clock.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Dwell {
    pub from: Us,
    pub to: Us,
    pub x: f32,
    pub y: f32,
}

pub const DWELL_MIN_US: Us = 600_000;
pub const DWELL_TRAVEL: f32 = 0.03;

/// Still ≥ 600 ms after travelling ≥ 3 % of the frame's diagonal since the last dwell or press. O(runs × log samples +
/// presses).
///
/// The travel is how far the run's place is from the last dwell's or press's (the track's first place before either),
/// in points like stillness. A press counts once it is down before the run starts. The take's last run ends at its last
/// kept position here (`StillRuns` holds it open).
pub fn dwells(track: &CursorTrack, still: &StillRuns) -> Vec<Dwell> {
    let mut out = Vec::new();
    let Some((first, last)) = track.span() else {
        return out;
    };
    let Some(mut since) = track.position(first) else {
        return out;
    };
    let presses = track.presses();
    let mut next_press = 0;
    let travel = DWELL_TRAVEL * DWELL_TRAVEL;
    for &(from, to) in still.runs() {
        while let Some(p) = presses.get(next_press).filter(|p| p.down < from) {
            since = (p.x, p.y);
            next_press += 1;
        }
        let to = to.min(last);
        if to - from < DWELL_MIN_US {
            continue;
        }
        let Some((x, y)) = track.position(from) else {
            continue;
        };
        let diagonal = Diagonal::of(track.area_at(from));
        if diagonal.share2(x - since.0, y - since.1) >= travel {
            out.push(Dwell { from, to, x, y });
            since = (x, y);
        }
    }
    out
}
