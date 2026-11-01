//! The previews' pace: Check camera's about 15 frames a second and the live view's 10,
//! paced in one place, the preview pipe, before a JPEG is made (none is made to be dropped). A camera's frames come a
//! little early or late (its own spacing, delivery), so pacing by an exact interval would drop half of them: a frame is
//! taken once 0.8 of the interval has passed since the last one taken. The cadence is even: at
//! 15 fps, a 30 fps camera gives every other frame, a 15 to 18.75 fps one (15 / 0.8) every frame, a 20 fps one every
//! other; never more than 18.75 a second.

/// Check camera's preview rate.
pub const PREVIEW_FPS: u32 = 15;
/// The recorder's live view's rate in its small view; a big view's is `live_view_size`'s.
pub const LIVE_VIEW_FPS: u32 = 10;

/// When the next preview frame is due, from the frames' own times (host ns).
#[derive(Clone, Debug)]
pub struct PreviewPacer {
    /// The pacer's rate, and the time between two frames at it.
    fps: u32,
    interval_ns: u64,
    /// The last frame taken, and when the next one is due.
    last_ns: Option<u64>,
    next_ns: u64,
}

impl PreviewPacer {
    /// A pacer for about `fps` frames a second (at least 1).
    pub fn at(fps: u32) -> Self {
        let fps = fps.max(1);
        Self {
            fps,
            interval_ns: 1_000_000_000 / u64::from(fps),
            last_ns: None,
            next_ns: 0,
        }
    }

    /// The rate becomes about `fps` (the live view's, as its view changes size); the next frame is due
    /// the new interval after the last one taken. The same rate changes nothing, so it may be called before each frame.
    pub fn retime(&mut self, fps: u32) {
        let fps = fps.max(1);
        if fps == self.fps {
            return;
        }
        self.fps = fps;
        self.interval_ns = 1_000_000_000 / u64::from(fps);
        if let Some(last) = self.last_ns {
            self.next_ns = last + self.interval_ns;
        }
    }

    /// Whether the frame at `at_ns` is shown: it is when it comes no more than a fifth of the interval before it is
    /// due, the interval after the last frame taken. The schedule starts from each frame taken (so an early frame never
    /// makes the next one wait longer); the first frame, and one whose time went back, start it again.
    pub fn take(&mut self, at_ns: u64) -> bool {
        let restart = self.last_ns.is_none_or(|last| at_ns < last);
        if !restart && at_ns + self.interval_ns / 5 < self.next_ns {
            return false;
        }
        self.next_ns = at_ns + self.interval_ns;
        self.last_ns = Some(at_ns);
        true
    }
}

impl Default for PreviewPacer {
    /// Check camera's pace (`PREVIEW_FPS`).
    fn default() -> Self {
        Self::at(PREVIEW_FPS)
    }
}
