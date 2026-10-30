//! Seeks: a still where the playhead goes, or (while playing) playback going on from there.

use klipolo_model::time::Us;

use super::Actor;
use crate::plan::last_frame_us;
use crate::session::PreviewBackend;

impl<B: PreviewBackend> Actor<B> {
    pub(in crate::session::actor) fn on_seek(&mut self) {
        let (at_us, seq) = *self.p.seek.borrow_and_update();
        // Sent before a play that started a newer run (a shuttle press while playing): the UI has moved on, and its
        // frames would carry an old sequence the UI drops.
        if (seq.wrapping_sub(self.seq) as i32) < 0 {
            return;
        }
        self.seq = seq;
        self.playhead_us = self.clamp_time(at_us);
        if let Some(rate) = self.playing.as_ref().map(|p| p.rate) {
            // A live seek: playback continues from there at its rate (no State event); what was warmed ahead is
            // stale.
            self.p.backend.cancel_warm();
            self.start_playing(self.playhead_us, rate);
        } else {
            if let Some((_, rate)) = self.play_on_view {
                self.play_on_view = Some((self.playhead_us, rate));
            }
            self.dirty = true;
        }
    }

    /// Past the end shows the last frame: the engine never draws the empty instant after the video.
    pub(in crate::session::actor) fn clamp_time(&self, at_us: Us) -> Us {
        let Some(cur) = &self.current else { return 0 };
        at_us.clamp(
            0,
            last_frame_us(cur.ctx.video_duration_us, cur.ctx.project.settings.fps),
        )
    }
}
