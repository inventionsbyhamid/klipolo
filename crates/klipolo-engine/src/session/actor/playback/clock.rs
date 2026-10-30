//! The playback clock's arithmetic at any shuttle rate, and where it reads now.

use std::time::Duration;

use klipolo_model::time::Us;

use super::{Actor, Clock};
use crate::plan::{frame_index, frame_start_us};
use crate::session::{AudioTime, PreviewBackend};

/// Timeline µs that `elapsed` of real time covers at `rate`.
pub(super) fn timeline_us(elapsed: Duration, rate: f64) -> Us {
    let us = elapsed.as_micros() as Us;
    if rate == 1.0 {
        us
    } else {
        (us as f64 * rate).round() as Us
    }
}

/// Real time until the clock reaches the next frame to draw, from `show`: the next frame boundary in the direction of
/// travel, `|rate|` frames on (at 2× every other frame is drawn). At 1× this is the next frame's start, as it always
/// was. A render that ran long skips frames instead of drifting.
pub(super) fn next_draw_in(show: Us, fps: u32, rate: f64) -> Duration {
    let index = frame_index(show, fps);
    let stride = rate.abs().max(1.0) as i64;
    let distance = if rate > 0.0 {
        frame_start_us(index + stride, fps) - show
    } else {
        // Frame `index - stride` shows once the clock is before the start of the frame after it.
        show - (frame_start_us(index + 1 - stride, fps) - 1)
    };
    let real = if rate.abs() == 1.0 {
        distance
    } else {
        (distance as f64 / rate.abs()).round() as Us
    };
    Duration::from_micros(real.max(1_000) as u64)
}

impl<B: PreviewBackend> Actor<B> {
    /// Timeline time on the playback clock (the playhead when paused).
    pub(in crate::session::actor) fn clock_us(&self) -> Us {
        let Some(p) = &self.playing else {
            return self.playhead_us;
        };
        match p.clock {
            Clock::Waiting { .. } => p.from_us,
            Clock::Audio => match self.p.backend.audio_time() {
                AudioTime::At(t) => t.max(p.heard).max(p.from_us),
                _ => p.heard.max(p.from_us),
            },
            Clock::Host { started, .. } => p.from_us + timeline_us(started.elapsed(), p.rate),
        }
    }
}
