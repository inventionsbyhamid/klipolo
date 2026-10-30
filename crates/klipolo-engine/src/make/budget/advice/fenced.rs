//! The advice's lines on fenced timing: where this computer's GPU timestamps aren't trusted, the agent is
//! told its effect was timed in batches, and when the batches couldn't resolve a cost that read Light (and that decided
//! its class), that it counts as Normal.

use crate::make::measure::Measured;

pub(super) fn fenced(m: &Measured) -> Vec<String> {
    let Some(f) = &m.fenced else {
        return Vec::new();
    };
    let frames = f
        .preview
        .iter()
        .flat_map(|b| &b.samples)
        .map(|b| b.frames)
        .max()
        .unwrap_or(0);
    let batch = if frames > 1 {
        format!("a batch of {frames} frames")
    } else {
        "a batch of frames".to_owned()
    };
    let mut lines = vec![format!(
        "Timed in batches by fences: this computer's GPU timestamps aren't trusted, so each figure is the difference \
         between {batch} and a single frame, per frame, which cancels the fixed cost each submission adds (its wait \
         for the graphics card)."
    )];
    if f.held {
        lines.push(
            "The batches couldn't resolve its cost to Light's precision this time (the timing was noisier than they \
             allow, or a frame was too slow to batch in full), so it counts as Normal, never Light."
                .into(),
        );
    }
    lines
}
