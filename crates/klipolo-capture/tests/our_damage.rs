//! Frames that changed only Klipolo's own windows, pure: what counts as
//! such a frame, and when one is still encoded.

use klipolo_capture::platform::common::our_damage::{
    CURSOR_MOVED_PT, CURSOR_PT, Freshness, SLACK_PX, only_ours, rect_in,
};
use klipolo_capture::platform::common::sck::CgRect;

fn r(x: f64, y: f64, w: f64, h: f64) -> CgRect {
    CgRect { x, y, w, h }
}

/// A 1440 × 900 display at AppKit's origin; the bubble, 132 pt, top centre 10 pt under the menu bar's 25 pt.
fn bubble() -> CgRect {
    let (display, whole) = (r(0.0, 0.0, 1440.0, 900.0), r(0.0, 0.0, 1440.0, 900.0));
    rect_in(r(654.0, 900.0 - 35.0 - 132.0, 132.0, 132.0), display, whole)
}

#[test]
fn klipolo_s_windows_land_in_the_picture_from_its_top_left() {
    let b = bubble();
    assert!((b.x - 654.0 / 1440.0).abs() < 1e-6, "{b:?}");
    assert!((b.y - 35.0 / 900.0).abs() < 1e-6, "{b:?}");
    assert!((b.w - 132.0 / 1440.0).abs() < 1e-9 && (b.h - 132.0 / 900.0).abs() < 1e-9);
}

#[test]
fn only_a_change_inside_klipolo_s_windows_counts_as_ours() {
    let ours = [bubble()];
    let reach = (CURSOR_PT / 1440.0, CURSOR_PT / 900.0);
    // A 1728 × 1080 frame: two pixels of slack absorb the window server's rounding, no more.
    let slack = (SLACK_PX / 1728.0, SLACK_PX / 1080.0);
    let far = Some((0.05, 0.9));
    let px = 1.0 / 1728.0;
    // The bubble's own area, rounded out by a pixel: ours.
    let inside = r(ours[0].x - px, ours[0].y, ours[0].w + 2.0 * px, ours[0].h);
    assert!(only_ours(Some(&[inside]), &ours, far, reach, slack));
    // Nothing changed at all: idle as well.
    assert!(only_ours(Some(&[]), &ours, far, reach, slack));
    // No attachment: never guessed, a real change.
    assert!(!only_ours(None, &ours, far, reach, slack));
    // Anything elsewhere (typing in a window beside the bubble) is real, even with the bubble's area too.
    let typing = r(0.30, 0.40, 0.10, 0.02);
    assert!(!only_ours(
        Some(&[inside, typing]),
        &ours,
        far,
        reach,
        slack
    ));
    // A few pixels beside the bubble (inside the old 2 % slack): real now.
    let beside_it = r(ours[0].x + ours[0].w + 4.0 * px, ours[0].y, 10.0 * px, 0.01);
    assert!(!only_ours(Some(&[beside_it]), &ours, far, reach, slack));
    // Partly under the bubble, partly out: real.
    let across = r(ours[0].x - 0.1, ours[0].y, 0.15, 0.05);
    assert!(!only_ours(Some(&[across]), &ours, far, reach, slack));
    // The cursor over or near the bubble: the change there may be the cursor itself, so it's real.
    let over = Some((ours[0].x + ours[0].w / 2.0, ours[0].y + ours[0].h / 2.0));
    let beside = Some((ours[0].x - reach.0 / 2.0, ours[0].y));
    assert!(!only_ours(Some(&[inside]), &ours, over, reach, slack));
    assert!(!only_ours(Some(&[inside]), &ours, beside, reach, slack));
    // A take that draws no cursor has none to worry about.
    assert!(only_ours(Some(&[inside]), &ours, None, reach, slack));
    // No windows of Klipolo's: any dirty rectangle is real.
    assert!(!only_ours(Some(&[inside]), &[], far, reach, slack));
}

#[test]
fn such_a_frame_is_still_encoded_when_a_keyframe_is_wanted_after_a_blank_or_before_any_real_frame()
{
    let still = Some((700.0, 400.0));
    let mut f = Freshness::default();
    assert!(!f.skip(true, false, false, still), "nothing to re-feed yet");
    f.sent(true, still);
    assert!(f.skip(true, false, false, still));
    assert!(!f.skip(false, false, false, still), "a real change");
    assert!(!f.skip(true, true, false, still), "a keyframe is wanted");
    assert!(
        !f.skip(true, false, true, still),
        "the first picture after a blank one"
    );
}

/// After a pause (the screen may have changed while its frames went uncounted), the first frame
/// after the resume is encoded even if only the bubble or the HUD changed; so is the one after a real frame that was
/// dropped on its way to the encoder.
#[test]
fn after_a_pause_or_a_dropped_frame_the_next_frame_is_encoded() {
    let still = Some((700.0, 400.0));
    let mut f = Freshness::default();
    f.sent(true, still);
    assert!(f.skip(true, false, false, still));
    f.paused();
    // Resumed: the encoder's re-feed already used the wanted keyframe, and only the bubble changed.
    assert!(
        !f.skip(true, false, false, still),
        "the picture from before the pause would stay"
    );
    f.sent(true, still);
    assert!(f.skip(true, false, false, still), "fresh again");
    f.sent(false, still);
    assert!(
        !f.skip(true, false, false, still),
        "the dropped frame's change must still reach the file"
    );
}

/// A frame is real whenever the drawn cursor moved since the last real
/// frame, whatever its dirty rectangles say, even none.
#[test]
fn a_moved_cursor_makes_the_frame_real_whatever_its_dirty_rectangles_say() {
    let mut f = Freshness::default();
    f.sent(true, Some((700.0, 400.0)));
    // An empty dirty list counts as ours (`only_ours`), yet the cursor moved: real.
    let moved = Some((700.0 + 2.0 * CURSOR_MOVED_PT, 400.0));
    assert!(!f.skip(true, false, false, moved));
    assert!(!f.skip(true, false, false, Some((700.0, 397.0))));
    // A still cursor (sub-threshold jitter) keeps the gain; a picture that draws no cursor has none to move.
    let jitter = Some((700.0 + CURSOR_MOVED_PT / 2.0, 400.0));
    assert!(f.skip(true, false, false, jitter));
    let mut none = Freshness::default();
    none.sent(true, None);
    assert!(none.skip(true, false, false, None));
}
