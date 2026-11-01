//! Whether Fill crops a main clip in a format (`framing::fill_crops`): what its crop leaves has another shape than the
//! area it fills, the frame less the background's padding or a split's top pane.

mod common;

use common::*;
use klipolo_model::crop::Crop;
use klipolo_model::framing::fill_crops;
use klipolo_model::project::FormatKind::{Landscape, Portrait, Square};
use klipolo_model::timeline::CameraMode;

#[test]
fn a_wide_video_crops_only_in_frames_of_another_shape() {
    let p = fixture();
    let broll = &p.timeline.main[1];
    assert!(!fill_crops(&p, broll, Landscape));
    assert!(fill_crops(&p, broll, Portrait));
    assert!(fill_crops(&p, broll, Square));
}

#[test]
fn what_the_crop_leaves_is_the_shape_that_counts() {
    let mut p = fixture();
    // A 16:9 video cut to 9:16 (1920 wide, 1080 tall: keep 607.5 of the width) fills 9:16 whole.
    let keep = (1080.0 * 9.0 / 16.0) / 1920.0;
    p.timeline.main[1].crop = Some(Crop {
        left: (1.0 - keep) / 2.0,
        right: (1.0 - keep) / 2.0,
        ..Crop::NONE
    });
    let broll = &p.timeline.main[1];
    assert!(!fill_crops(&p, broll, Portrait));
    assert!(fill_crops(&p, broll, Landscape));
}

#[test]
fn padding_and_a_split_change_the_area() {
    let mut p = fixture();
    // 1920×1080 less 108 px each side: 1704×864, wider than 16:9.
    p.background.padding = 0.1;
    assert!(fill_crops(&p, &p.timeline.main[1], Landscape));
    p.background.padding = 0.0;
    // The recording's screen in 9:16 sits in the split's top pane (1080×960) while its camera is split below.
    let mut rec = p.timeline.main[0].clone();
    let camera = rec.camera.as_mut().unwrap();
    camera
        .mode_overrides
        .insert(Portrait, CameraMode::SplitBottom);
    assert!(fill_crops(&p, &rec, Portrait));
    // A screen the size of that pane fills it whole.
    let screen = p.assets.iter_mut().find(|a| a.id == "screen").unwrap();
    (screen.width, screen.height) = (1080, 960);
    assert!(!fill_crops(&p, &rec, Portrait));
    camera_mode(&mut rec, CameraMode::Bubble);
    assert!(fill_crops(&p, &rec, Portrait));
}

fn camera_mode(clip: &mut klipolo_model::timeline::Clip, mode: CameraMode) {
    let camera = clip.camera.as_mut().unwrap();
    camera.mode_overrides.insert(Portrait, mode);
}

#[test]
fn footage_of_no_known_size_crops_nothing() {
    let mut p = fixture();
    let broll = p.assets.iter_mut().find(|a| a.id == "broll").unwrap();
    (broll.width, broll.height) = (0, 0);
    assert!(!fill_crops(&p, &p.timeline.main[1], Portrait));
}
