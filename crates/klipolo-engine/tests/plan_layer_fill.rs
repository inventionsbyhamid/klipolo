//! A video layer's picture inside its box: contained whole (Fit), or covering the box from the centre (Fill), after
//! its crop; a mirrored layer is cut on the side the person sees, and its motion moves the box as before.

mod support;

use klipolo_engine::plan::framing::{FULL, cover_crop};
use klipolo_engine::plan::overlay::video_rect;
use klipolo_engine::plan::{VideoLayer, VideoRole, frame_plan};
use klipolo_model::crop::{Crop, within};
use klipolo_model::layout::{BoxN, resolve_rect};
use klipolo_model::project::FormatKind::{Landscape, Portrait};
use klipolo_model::project::{FormatKind, Project};
use klipolo_model::timeline::ItemContent;
use support::{animated, bx, close, ctx, item, overlay_track, project, secs, videos};

fn crop(left: f64, top: f64, right: f64, bottom: f64) -> Crop {
    Crop {
        left,
        top,
        right,
        bottom,
    }
}

/// The 16:9 b-roll as a layer in `rect` from 0 to 5 s.
fn with_layer(rect: BoxN, fill: bool, crop: Option<Crop>, mirror: bool) -> Project {
    let mut p = project();
    p.timeline.tracks = vec![overlay_track(
        "o",
        vec![item(
            "v",
            0.0,
            5.0,
            rect,
            ItemContent::Video {
                asset_id: "broll".into(),
                source_in_us: 0,
                speed: 1.0,
                volume: 1.0,
                muted: true,
                mirror,
                fill,
                crop,
                tie: None,
            },
        )],
    )];
    p
}

fn layer_at(p: &Project, t: f64, format: FormatKind) -> VideoLayer {
    let plan = frame_plan(&ctx(p.clone()), secs(t), format);
    videos(&plan)
        .into_iter()
        .find(|v| v.role == VideoRole::Overlay)
        .expect("the layer")
        .clone()
}

/// A square box in 16:9: 0.3 of the width is 576 px, so 0.5333 of the height.
const SQUARE: BoxN = BoxN {
    x: 0.1,
    y: 0.2,
    w: 0.3,
    h: 0.533_333_333_333_333_3,
};

#[test]
fn a_layer_is_contained_in_its_box_by_default() {
    let v = layer_at(&with_layer(SQUARE, false, None, false), 1.0, Landscape);
    // 576×324 centred in the 576×576 box.
    close(v.dest, bx(0.1, 0.2 + 126.0 / 1080.0, 0.3, 0.3));
    close(v.crop, FULL);
}

#[test]
fn a_filled_layer_covers_its_box_from_the_centre() {
    let v = layer_at(&with_layer(SQUARE, true, None, false), 1.0, Landscape);
    close(v.dest, SQUARE);
    // The middle 9/16 of the 16:9 picture across.
    close(v.crop, bx(0.21875, 0.0, 0.5625, 1.0));
}

#[test]
fn a_cropped_layer_shows_what_is_left_contained_or_filling() {
    // A quarter off each side leaves 8:9: contained in the square box, 512×576.
    let c = crop(0.25, 0.0, 0.25, 0.0);
    let v = layer_at(&with_layer(SQUARE, false, Some(c), false), 1.0, Landscape);
    close(v.crop, bx(0.25, 0.0, 0.5, 1.0));
    close(v.dest, bx(0.1 + 32.0 / 1920.0, 0.2, 512.0 / 1920.0, SQUARE.h));
    // Filling the box, it is cut down a little more: 8:9 covering 1:1.
    let v = layer_at(&with_layer(SQUARE, true, Some(c), false), 1.0, Landscape);
    close(v.dest, SQUARE);
    close(
        v.crop,
        within(c.kept(), cover_crop((16.0 / 9.0) * 0.5, 1.0, 0.5, 0.5)),
    );
}

#[test]
fn a_mirrored_layer_is_cut_on_the_side_the_person_sees() {
    let v = layer_at(
        &with_layer(SQUARE, false, Some(crop(0.3, 0.0, 0.0, 0.0)), true),
        1.0,
        Landscape,
    );
    assert!(v.mirror);
    close(v.crop, bx(0.0, 0.0, 0.7, 1.0));
}

#[test]
fn a_filled_layer_moves_with_its_motion() {
    let mut p = with_layer(SQUARE, true, None, false);
    let layer = p.timeline.tracks[0].items[0].clone();
    p.timeline.tracks[0].items[0] = animated(layer, Some("slide"), None);
    let entering = layer_at(&p, 0.05, Landscape);
    let settled = layer_at(&p, 1.0, Landscape);
    close(settled.dest, SQUARE);
    assert!(entering.dest != settled.dest, "{:?}", entering.dest);
    // The crop is the box's, wherever the box is.
    close(entering.crop, settled.crop);
}

#[test]
fn video_rect_places_the_box_the_gizmo_draws() {
    let p = project();
    let broll = p.asset("broll").unwrap();
    for format in [Landscape, Portrait] {
        for fill in [false, true] {
            let layer = with_layer(SQUARE, fill, None, false);
            let rect = resolve_rect(&layer.timeline.tracks[0].items[0].layout, Landscape, format);
            let (dest, shown) = video_rect(rect, broll, fill, Crop::NONE, format);
            let v = layer_at(&layer, 1.0, format);
            close(dest, v.dest);
            close(shown, v.crop);
        }
    }
}
