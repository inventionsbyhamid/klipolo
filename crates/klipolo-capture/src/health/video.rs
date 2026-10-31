//! The video rules (`VideoRules`): a stall, and frames dropped, from a video pipe's counters once a second.
//!
//! A stall is 2 s without a buffer of any status or an answered nudge. A stream that answers its nudges but delivers
//! no buffer at all for `ANSWERS_ONLY_NS` is a stall too: a healthy still stream keeps sending
//! idle buffers (about 32 a second on the T2 Mac, even for a fully still window), and the longest silence seen was
//! 3.4 s, so a minute of answers without one buffer is a stream that stopped delivering, whose file would otherwise
//! show a frozen picture for the rest of the take with no note. The heal policy restarts it, within its budget.

use std::collections::VecDeque;

use klipolo_model::coded::ErrorParam;

use super::{DROP_WINDOW_NS, HealthNote, STALL_NS};

/// A stream that answers its nudges but sends no buffer for this long, while the display is awake, is a stall.
pub const ANSWERS_ONLY_NS: u64 = 60_000_000_000;
use crate::english::code;
use crate::media::SourceId;
use crate::pipe::VideoStats;
use crate::shell::Level;

#[derive(Clone, Debug, PartialEq)]
pub enum VideoProblem {
    /// No buffer for 2 s while the display is awake: the heal policy restarts the source (`PipeEvent::Stalled`).
    Stalled,
    /// `capture.health.dropping`.
    Dropping(HealthNote),
}

/// The stall and drop rules for one video pipe. Make a new one when a pipe is replaced; a pipe whose counters start
/// again from zero is handled too.
#[derive(Clone, Debug)]
pub struct VideoRules {
    source: SourceId,
    fps: u32,
    delivered: Option<u64>,
    unchanged_since: u64,
    /// The buffers alone (no answers), and since when they haven't changed.
    buffers: Option<u64>,
    buffers_since: u64,
    /// (at, delivered, dropped) at least a second apart; the oldest at or before the drop window's start. About 11.
    window: VecDeque<(u64, u64, u64)>,
    dropping: bool,
}

impl VideoRules {
    pub fn new(source: &str, fps: u32) -> Self {
        Self {
            source: source.into(),
            fps,
            delivered: None,
            unchanged_since: 0,
            buffers: None,
            buffers_since: 0,
            window: VecDeque::with_capacity(12),
            dropping: false,
        }
    }

    /// Called once a second with the pipe's counters. Returns a stall (no buffer of any status and no answered nudge
    /// for 2 s while the display is awake; again after 2 more s without one) or a drop warning (more than 1 % of frames dropped over 10 s, against
    /// the frames delivered or the frame rate's worth, whichever is more; again only after a window at or under 1 %).
    pub fn check(
        &mut self,
        stats: &VideoStats,
        now_ns: u64,
        display_awake: bool,
    ) -> Option<VideoProblem> {
        let quiet = self.answers_only(stats.delivered, now_ns, display_awake);
        let stalled = self.stall(stats.delivered + stats.answered, now_ns, display_awake) || quiet;
        let dropping = self.drops(stats, now_ns);
        match (stalled, dropping) {
            (true, _) => Some(VideoProblem::Stalled),
            (false, note) => note.map(VideoProblem::Dropping),
        }
    }

    /// `live`: the buffers delivered and the nudges answered so far, the stream's signs of life.
    fn stall(&mut self, live: u64, now_ns: u64, display_awake: bool) -> bool {
        if self.delivered != Some(live) || !display_awake {
            (self.delivered, self.unchanged_since) = (Some(live), now_ns);
            return false;
        }
        if now_ns.saturating_sub(self.unchanged_since) < STALL_NS {
            return false;
        }
        self.unchanged_since = now_ns;
        true
    }

    /// No buffer at all for `ANSWERS_ONLY_NS` while awake, the nudges' answers aside; again after as long once more.
    fn answers_only(&mut self, delivered: u64, now_ns: u64, display_awake: bool) -> bool {
        if self.buffers != Some(delivered) || !display_awake {
            (self.buffers, self.buffers_since) = (Some(delivered), now_ns);
            return false;
        }
        if now_ns.saturating_sub(self.buffers_since) < ANSWERS_ONLY_NS {
            return false;
        }
        self.buffers_since = now_ns;
        tracing::warn!(
            source = self.source,
            "health: the stream answers its nudges but sent nothing for a minute; restarting it"
        );
        true
    }

    fn drops(&mut self, s: &VideoStats, now_ns: u64) -> Option<HealthNote> {
        let dropped = s.dropped_pool + s.dropped_encoder + s.dropped_channel;
        if self
            .window
            .back()
            .is_some_and(|&(at, d, x)| now_ns < at || s.delivered < d || dropped < x)
        {
            self.window.clear();
        }
        let start = now_ns.saturating_sub(DROP_WINDOW_NS);
        while self.window.len() >= 2 && self.window[1].0 <= start {
            self.window.pop_front();
        }
        let mut note = None;
        if let Some(&(at, d, x)) = self.window.front().filter(|w| w.0 <= start) {
            let expected = u64::from(self.fps).saturating_mul(now_ns - at) / 1_000_000_000;
            let frames = (s.delivered - d).max(expected);
            let lost = dropped - x;
            if lost.saturating_mul(100) <= frames {
                self.dropping = false;
            } else if !self.dropping {
                self.dropping = true;
                note = Some(HealthNote {
                    code: code!("capture.health.dropping"),
                    source: Some(self.source.clone()),
                    params: vec![
                        ("dropped", ErrorParam::Number(lost as f64)),
                        ("frames", ErrorParam::Number(frames as f64)),
                    ],
                    level: Level::Warning,
                });
            }
        }
        if self
            .window
            .back()
            .is_none_or(|&(at, ..)| now_ns >= at + 1_000_000_000)
        {
            self.window.push_back((now_ns, s.delivered, dropped));
        }
        note
    }
}
