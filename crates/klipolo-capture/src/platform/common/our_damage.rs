//! Frames that changed only Klipolo's own windows (pure, built on every OS, so the Linux CI runs test it):
//! the camera bubble's live picture (and the HUD) are left out of a whole-display take, yet the window server still
//! reports their area as changed, so ScreenCaptureKit delivers a new frame at the camera's rate over a still screen.
//! Such a frame is handled as an idle one: counted (`delivered`, `idle`, and the stream's sign of life), never encoded,
//! so the encoder thread's re-feed and keyframe rule give the file a still screen's picture, as without the bubble.
//!
//! Everything here is in the captured picture, as fractions of its width and height from its top left (`cursor_in`'s
//! space): ScreenCaptureKit's dirty rectangles (its frame's pixels, over the frame's size), Klipolo's windows and the
//! cursor (AppKit's global points, through the picture's place on the display).

use super::sck::{CgRect, cursor_in};

/// How far a dirty rectangle may reach past one of Klipolo's windows and still be its own, in the frame's pixels: the
/// device check found the bubble's damage equal to its frame to within float rounding (about 1e-8 of the picture), so
/// two pixels only absorb rounding (it was 2 % of the picture).
pub const SLACK_PX: f64 = 2.0;
/// The cursor's reach around its hot spot, in points: a cursor near one of Klipolo's windows may be what changed.
pub const CURSOR_PT: f64 = 48.0;
/// The cursor moved since the last real frame by more than this, in points: the frame is real, whatever its dirty
/// rectangles say (they needn't include the drawn cursor).
pub const CURSOR_MOVED_PT: f64 = 0.5;

/// `window` (AppKit's global points, bottom-left origin) in the captured picture: fractions from its top left.
/// `display` and `captured` are the picture's place, as for `cursor_in`.
pub fn rect_in(window: CgRect, display: CgRect, captured: CgRect) -> CgRect {
    let (x, y) = cursor_in((window.x, window.y + window.h), display, captured);
    CgRect {
        x: f64::from(x),
        y: f64::from(y),
        w: window.w / captured.w.max(f64::MIN_POSITIVE),
        h: window.h / captured.h.max(f64::MIN_POSITIVE),
    }
}

/// Whether a delivered frame changed nothing but Klipolo's own windows (`ours`), all in picture fractions:
/// - no dirty-rectangle attachment (`None`): a real change, never guessed;
/// - no dirty rectangle: nothing changed;
/// - the cursor drawn in the picture (`cursor`, its hot spot; None when the take records none) within `cursor_reach`
///   of one of Klipolo's windows: a real change, since a dirty rectangle there may be the cursor crossing it;
/// - else every dirty rectangle within one of Klipolo's windows, give or take `slack` (`SLACK_PX` over the frame's width
///   and height).
pub fn only_ours(
    dirty: Option<&[CgRect]>,
    ours: &[CgRect],
    cursor: Option<(f64, f64)>,
    cursor_reach: (f64, f64),
    slack: (f64, f64),
) -> bool {
    let Some(dirty) = dirty else {
        return false;
    };
    if dirty.is_empty() {
        return true;
    }
    if let Some((x, y)) = cursor {
        let (rx, ry) = cursor_reach;
        let near = |o: &CgRect| {
            x >= o.x - rx && x <= o.x + o.w + rx && y >= o.y - ry && y <= o.y + o.h + ry
        };
        if ours.iter().any(near) {
            return false;
        }
    }
    let (sx, sy) = slack;
    let inside = |d: &CgRect, o: &CgRect| {
        d.x >= o.x - sx
            && d.y >= o.y - sy
            && d.x + d.w <= o.x + o.w + sx
            && d.y + d.h <= o.y + o.h + sy
    };
    dirty.iter().all(|d| ours.iter().any(|o| inside(d, o)))
}

/// What a screen stream remembers between frames for the rule: whether the file's picture is fresh. A frame that
/// changed only Klipolo's windows is idle only while the last real frame still shows what the screen shows.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Freshness {
    /// A real frame went to the encoder: there is one to re-feed.
    sent_any: bool,
    /// The next frame must be encoded, whatever changed: the take was paused (the screen may have changed meanwhile, its
    /// frames uncounted), or a real frame was dropped before it reached the encoder.
    fresh_needed: bool,
    /// Where the cursor was (AppKit's global points) at the last real frame, when the picture draws it.
    cursor_at: Option<(f64, f64)>,
}

impl Freshness {
    /// A fresh picture is needed (after a pause or a dropped real frame): the next frame is encoded.
    pub fn needs_fresh(&self) -> bool {
        self.fresh_needed
    }

    /// A buffer came while the take is paused.
    pub fn paused(&mut self) {
        self.fresh_needed = true;
    }

    /// A real frame was handed to the encoder (`queued`), or dropped on its way there (a full pool or channel: not
    /// queued, so the next frame must carry its change). `cursor`: where the cursor was for it.
    pub fn sent(&mut self, queued: bool, cursor: Option<(f64, f64)>) {
        if queued {
            (self.sent_any, self.fresh_needed, self.cursor_at) = (true, false, cursor);
        } else {
            self.fresh_needed = true;
        }
    }

    /// Whether a frame that changed only Klipolo's windows (`only_ours`) is handled as idle. Never when a keyframe is
    /// wanted (the start, a resume), on the first picture after a blank or suspended one, while a fresh picture is needed
    /// (after a pause or a dropped real frame), before any real frame went to the encoder (nothing to re-feed yet), or
    /// when the drawn cursor (`cursor`, AppKit's global points) moved since the last real frame.
    pub fn skip(
        &self,
        only_ours: bool,
        key_wanted: bool,
        after_blank: bool,
        cursor: Option<(f64, f64)>,
    ) -> bool {
        only_ours
            && !key_wanted
            && !after_blank
            && !self.fresh_needed
            && self.sent_any
            && !self.cursor_moved(cursor)
    }

    /// The drawn cursor (`cursor`, AppKit's global points) is more than `CURSOR_MOVED_PT` from where it was at the last
    /// real frame (or none was recorded then). A picture that draws no cursor (`None`) has none to move.
    pub fn cursor_moved(&self, cursor: Option<(f64, f64)>) -> bool {
        match (cursor, self.cursor_at) {
            (Some((x, y)), Some((was_x, was_y))) => {
                (x - was_x).abs() > CURSOR_MOVED_PT || (y - was_y).abs() > CURSOR_MOVED_PT
            }
            (Some(_), None) => true,
            (None, _) => false,
        }
    }
}
