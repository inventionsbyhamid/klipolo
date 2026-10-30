//! A recording's true gaps: the stretches where none of its sound sources has a run, so the
//! recorded sound has nothing there (a recorder started after the camera, stopped before it, or a file is missing
//! between two runs). The camera's own sound fills them where its mix says so (`SourceMix::fills_gaps`): the engine
//! plays it there, and captions follow it. A mute is the person's silence: muting a sound turns its filling off
//! (`SourceMix::set_muted`).

use super::{Recording, SourceMix};
use crate::time::Us;

impl SourceMix {
    /// Mutes or unmutes the mix. A mute is the person's choice, so it means silence: muting a
    /// camera's own sound also turns its gap filling off, wherever the mute applies, and marks it theirs
    /// (`muted_by_person`: no `fillGaps` turns it back on). Unmuting clears the mark and leaves the fill
    /// as it is (an unmuted sound plays everywhere anyway).
    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
        self.muted_by_person = muted;
        if muted {
            self.fills_gaps = false;
        }
    }

    /// Whether the person muted it (`set_muted`): silent, and no fill turns it back on.
    pub fn person_muted(&self) -> bool {
        self.muted && self.muted_by_person
    }
}

/// A gap shorter than this is where two runs meet, give or take their rounding (a recorder's back-to-back files),
/// not a stretch nothing recorded: it isn't filled.
const MIN_GAP_US: Us = 1_000;

impl Recording {
    /// The recording-time stretches of `[from, to)` that no source but a picture source covers with a run (its nudge
    /// included), in order, each at least `MIN_GAP_US` long. Only the runs and nudges count: a mute or a separation
    /// doesn't make a gap, and a picture source never counts, since it always has runs. O(runs ×
    /// gaps); nothing allocated but the result.
    pub fn true_gaps(&self, from: Us, to: Us) -> Vec<(Us, Us)> {
        let mut gaps = Vec::new();
        if from < to {
            gaps.push((from, to));
        }
        for s in self.audio_sources() {
            for run in &s.runs {
                let (a, b) = run.rec_span(s.nudge_us);
                cut(&mut gaps, a, b);
            }
        }
        gaps.retain(|&(a, b)| b.saturating_sub(a) >= MIN_GAP_US);
        gaps
    }
}

/// `gaps` (in order, apart) less `[a, b)`.
fn cut(gaps: &mut Vec<(Us, Us)>, a: Us, b: Us) {
    if a >= b {
        return;
    }
    let mut i = 0;
    while i < gaps.len() {
        let (from, to) = gaps[i];
        if to <= a {
            i += 1;
            continue;
        }
        if b <= from {
            break;
        }
        match (from < a, b < to) {
            (true, true) => {
                gaps[i].1 = a;
                gaps.insert(i + 1, (b, to));
                break;
            }
            (true, false) => {
                gaps[i].1 = a;
                i += 1;
            }
            (false, true) => {
                gaps[i].0 = b;
                break;
            }
            (false, false) => {
                gaps.remove(i);
            }
        }
    }
}
