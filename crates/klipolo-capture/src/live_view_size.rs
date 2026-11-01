//! The recorder's live view's picture for the view that shows it: a pure rule, so the Linux CI runs test it.
//! - **Small** (`None`: no view size was given): the target fitted to 640 × 640, 10 frames a second.
//! - **Big** (the view's own size in device pixels, as the Record dialog's preview box gives it): the target fitted
//!   inside the view, never above its own pixels (the display's size) nor 1080p (1920 × 1080, turned to the view's
//!   shape), at 15 to 30 frames a second within this computer's pixels a second (`big_pixel_rate`): each JPEG costs by
//!   its pixels, so the rate bounds a big view's cost whatever its size. Where 15 frames a second would pass it, the
//!   picture gets smaller instead.
//!   - Apple silicon (and anything not an Intel Mac): 1080p at 15 (`BIG_PIXEL_RATE`): 1080p gets 15, a 16:10 display's
//!     1728 × 1080 16, 1280 × 800 or less 30.
//!   - An Intel Mac, whose JPEGs are made on its CPU: about 720p at 15 (`INTEL_MAC_PIXEL_RATE`): a 16:10 display's
//!     picture is 1212 × 758 at 15, 1024 × 640 at 21.
//! - A view no bigger than the small picture gets the small one, at its rate: a big view's cost only when it shows more.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::pace::LIVE_VIEW_FPS;
use crate::platform::common::sck::fit_size;

/// The small view's box: the picture's long side at most 640 px, its shape the target's.
pub const SMALL_BOX: (u32, u32) = (640, 640);
/// A big view's most: 1080p, in the view's shape.
pub const BIG_MOST: (u32, u32) = (1920, 1080);
/// A big view's most frames a second.
pub const BIG_MAX_FPS: u32 = 30;
/// A big view's fewest frames a second: below it, its picture gets smaller instead.
pub const BIG_MIN_FPS: u32 = 15;
/// A big view's most pixels a second: 1080p at 15 frames a second (about 31 Mpx/s; the small view sends about 2.6).
pub const BIG_PIXEL_RATE: u64 = 1920 * 1080 * 15;
/// An Intel Mac's: 720p at 15 frames a second (about 14 Mpx/s).
pub const INTEL_MAC_PIXEL_RATE: u64 = 1280 * 720 * 15;

/// A big view's most pixels a second on a computer of `os` and `arch` (`std::env::consts`'s words).
pub fn big_pixel_rate(os: &str, arch: &str) -> u64 {
    if os == "macos" && arch == "x86_64" {
        INTEL_MAC_PIXEL_RATE
    } else {
        BIG_PIXEL_RATE
    }
}

/// This computer's (`big_pixel_rate`).
pub fn this_computer_s_pixel_rate() -> u64 {
    big_pixel_rate(std::env::consts::OS, std::env::consts::ARCH)
}

/// A view's size in device pixels, as the UI measures it (its CSS size times the display's scale).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ViewPixels {
    pub width: u32,
    pub height: u32,
}

/// What the live view's stream sends: its picture's size in pixels, and how many frames a second.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LiveFrame {
    pub size: (u32, u32),
    pub fps: u32,
}

/// The live view's picture of a target of `source` pixels for `view` (None: the small view), within `rate` pixels a
/// second (`big_pixel_rate`).
pub fn live_frame(view: Option<ViewPixels>, source: (u32, u32), rate: u64) -> LiveFrame {
    let small = LiveFrame {
        size: fit_size(SMALL_BOX, source),
        fps: LIVE_VIEW_FPS,
    };
    let Some(view) = view else {
        return small;
    };
    let most = if view.width >= view.height {
        BIG_MOST
    } else {
        (BIG_MOST.1, BIG_MOST.0)
    };
    let mut size = fit_inside((view.width.min(most.0), view.height.min(most.1)), source);
    // Never so many pixels that the rate falls below `BIG_MIN_FPS`: the picture gets smaller, in its own shape.
    let most_pixels = rate / u64::from(BIG_MIN_FPS);
    if pixels(size) > most_pixels {
        let k = (most_pixels as f64 / pixels(size) as f64).sqrt();
        let room = (f64::from(size.0) * k, f64::from(size.1) * k);
        size = fit_inside((room.0 as u32, room.1 as u32), source);
    }
    if size.0 <= small.size.0 && size.1 <= small.size.1 {
        return small;
    }
    let fps = (rate / pixels(size)).clamp(u64::from(BIG_MIN_FPS), u64::from(BIG_MAX_FPS));
    LiveFrame {
        size,
        fps: u32::try_from(fps).unwrap_or(BIG_MAX_FPS),
    }
}

fn pixels(size: (u32, u32)) -> u64 {
    u64::from(size.0) * u64::from(size.1)
}

/// `source` scaled to fit inside `room` with its own shape (the view shows it whole, letterboxed), never above its own
/// pixels, with even sides, at least 2×2. Unlike `fit_size`, the room is never turned: it is the view's own shape.
fn fit_inside(room: (u32, u32), source: (u32, u32)) -> (u32, u32) {
    let (sw, sh) = (f64::from(source.0.max(1)), f64::from(source.1.max(1)));
    let scale = (f64::from(room.0) / sw)
        .min(f64::from(room.1) / sh)
        .min(1.0);
    let even = |n: f64| (((n / 2.0).floor() as u32) * 2).max(2);
    (even(sw * scale), even(sh * scale))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RETINA: (u32, u32) = (2560, 1600);

    fn view(width: u32, height: u32) -> Option<ViewPixels> {
        Some(ViewPixels { width, height })
    }

    #[test]
    fn the_small_view_keeps_today_s_picture_and_rate() {
        let small = LiveFrame {
            size: (640, 400),
            fps: 10,
        };
        assert_eq!(live_frame(None, RETINA, BIG_PIXEL_RATE), small);
        // A view no bigger than the small picture costs what it does.
        assert_eq!(live_frame(view(704, 396), RETINA, BIG_PIXEL_RATE), small);
        assert_eq!(live_frame(view(300, 200), RETINA, BIG_PIXEL_RATE), small);
    }

    #[test]
    fn a_big_view_s_picture_follows_its_size_within_the_caps() {
        // The view's own size, in the target's shape.
        assert_eq!(
            live_frame(view(1280, 800), RETINA, BIG_PIXEL_RATE),
            LiveFrame {
                size: (1280, 800),
                fps: 30
            }
        );
        assert_eq!(
            live_frame(view(1400, 700), RETINA, BIG_PIXEL_RATE).size,
            (1120, 700)
        );
        // Never above 1080p: a 2880 × 1800 view of a 16:10 display gets 1728 × 1080, at 16 frames a second.
        assert_eq!(
            live_frame(view(2880, 1800), RETINA, BIG_PIXEL_RATE),
            LiveFrame {
                size: (1728, 1080),
                fps: 16
            }
        );
        // A 16:9 display at 1080p: 15 frames a second.
        assert_eq!(
            live_frame(view(3840, 2160), (3840, 2160), BIG_PIXEL_RATE),
            LiveFrame {
                size: (1920, 1080),
                fps: 15
            }
        );
        // Never above the display's own pixels.
        assert_eq!(
            live_frame(view(2880, 1800), (1440, 900), BIG_PIXEL_RATE).size,
            (1440, 900)
        );
        // A portrait view's 1080p is 1080 × 1920.
        assert_eq!(
            live_frame(view(1200, 2400), (1080, 1920), BIG_PIXEL_RATE).size,
            (1080, 1920)
        );
        // A tall window in a wide view fits its height.
        assert_eq!(
            live_frame(view(2400, 1200), (900, 1600), BIG_PIXEL_RATE).size,
            (606, 1080)
        );
    }

    #[test]
    fn frames_a_second_stay_within_the_caps() {
        for (w, h) in [
            (700, 500),
            (1000, 700),
            (1600, 1000),
            (2000, 1200),
            (5000, 3000),
        ] {
            for rate in [BIG_PIXEL_RATE, INTEL_MAC_PIXEL_RATE] {
                let frame = live_frame(view(w, h), RETINA, rate);
                let small = frame.fps == LIVE_VIEW_FPS && frame.size == (640, 400);
                let big = (BIG_MIN_FPS..=BIG_MAX_FPS).contains(&frame.fps);
                assert!(small || big, "{frame:?}");
                assert!(
                    small || pixels(frame.size) * u64::from(frame.fps) <= rate,
                    "{frame:?}"
                );
                assert!(frame.size.0 <= 1920 && frame.size.1 <= 1080, "{frame:?}");
            }
        }
    }

    #[test]
    fn the_record_dialog_s_box_gets_its_own_pixels_on_both_computers() {
        // The dialog's 16:9 box is 672 × 378 points: 1344 × 756 device pixels on a 2x display.
        let dialog = view(1344, 756);
        let intel = big_pixel_rate("macos", "x86_64");
        let apple = big_pixel_rate("macos", "aarch64");
        // A 16:10 display fills the box's height, a pixel for each of the box's: about twice the small picture's
        // width. An Intel Mac sends fewer frames a second, within its pixels a second.
        for (rate, fps) in [(apple, 30), (intel, 15)] {
            assert_eq!(
                live_frame(dialog, RETINA, rate),
                LiveFrame {
                    size: (1208, 756),
                    fps
                }
            );
        }
        // A 16:9 display fills the whole box on Apple silicon; on an Intel Mac it is 720p, a little smaller, so as not
        // to go below 15 frames a second.
        assert_eq!(
            live_frame(dialog, (3840, 2160), apple),
            LiveFrame {
                size: (1344, 756),
                fps: 30
            }
        );
        assert_eq!(
            live_frame(dialog, (3840, 2160), intel),
            LiveFrame {
                size: (1280, 720),
                fps: 15
            }
        );
    }

    #[test]
    fn an_intel_mac_s_big_view_is_about_720p_at_15() {
        let intel = big_pixel_rate("macos", "x86_64");
        assert_eq!(intel, INTEL_MAC_PIXEL_RATE);
        assert_eq!(big_pixel_rate("macos", "aarch64"), BIG_PIXEL_RATE);
        // Full screen on a 16:10 Retina display: smaller rather than below 15 frames a second.
        assert_eq!(
            live_frame(view(2880, 1800), RETINA, intel),
            LiveFrame {
                size: (1212, 758),
                fps: 15
            }
        );
        // A smaller view keeps its size, at more frames.
        assert_eq!(
            live_frame(view(1024, 640), RETINA, intel),
            LiveFrame {
                size: (1024, 640),
                fps: 21
            }
        );
    }
}
