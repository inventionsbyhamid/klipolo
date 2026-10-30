//! Ducking's envelope (`AudioSource::duck`): how far down a ducking source is at each timeline frame. Each stretch of
//! speech pulls it down from `ATTACK_US` before it starts, holds it there while it lasts, and lets it back up over
//! `RELEASE_US` after it ends. `speech_ranges` joins stretches closer than the two together (`JOIN_US`), so between
//! sentences the music stays down, and the ramps of the stretches it gives never overlap. Where ranges closer than that
//! do meet (given directly), the deeper of the two ramps wins at each frame: the music comes up only as far as where
//! the release and the next attack cross, then goes down again, without a step. The depth moves in a straight line in
//! dB (music falls and rises evenly to the ear) and is worked out for every sample, so the gain never steps: 12 dB over
//! 150 ms is under 0.1 dB a millisecond.

use klipolo_model::time::Us;

use super::to_frame;
use crate::audio::plan::Duck;

/// How long before speech the sound starts going down: it is all the way down as the first word starts.
pub const ATTACK_US: Us = 150_000;
/// How long after speech the sound takes to come back up.
pub const RELEASE_US: Us = 400_000;

/// A walk through a duck's envelope, frame by frame, forward only (one per source per block): O(1) a frame, after one
/// binary search where it starts.
pub(crate) struct Envelope<'a> {
    duck: &'a Duck,
    /// The first stretch of speech whose release hasn't ended by the last frame asked.
    next: usize,
    attack: i64,
    release: i64,
    /// The gain all the way down.
    full: f32,
}

impl<'a> Envelope<'a> {
    /// From timeline frame `from` on.
    pub(crate) fn new(duck: &'a Duck, from: i64) -> Self {
        let release = to_frame(RELEASE_US);
        Self {
            next: duck
                .speech
                .partition_point(|r| to_frame(r.end) + release <= from),
            attack: to_frame(ATTACK_US),
            release,
            full: 10f32.powf(duck.db / 20.0),
            duck,
        }
    }

    /// The gain at timeline frame `f` (no earlier than the last frame asked).
    pub(crate) fn gain(&mut self, f: i64) -> f32 {
        let speech = &self.duck.speech;
        while speech
            .get(self.next)
            .is_some_and(|r| to_frame(r.end) + self.release <= f)
        {
            self.next += 1;
        }
        // The deepest pull of the stretches around `f`: the one releasing, and any whose attack has begun.
        let mut depth = 0f32;
        for r in &speech[self.next..] {
            let (start, end) = (to_frame(r.start), to_frame(r.end));
            if f <= start - self.attack {
                break;
            }
            let pull = if f < start {
                (f - (start - self.attack)) as f32 / self.attack as f32
            } else if f < end {
                1.0
            } else {
                1.0 - (f - end) as f32 / self.release as f32
            };
            depth = depth.max(pull);
            if depth >= 1.0 {
                return self.full;
            }
        }
        if depth <= 0.0 {
            1.0
        } else {
            10f32.powf(self.duck.db * depth / 20.0)
        }
    }
}

/// The gain at one timeline frame, for the loudness estimate (`loudness::Fades`): one binary search.
pub(crate) fn gain_at(duck: &Duck, f: i64) -> f32 {
    Envelope::new(duck, f).gain(f)
}
