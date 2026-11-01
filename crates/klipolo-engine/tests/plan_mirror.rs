//! Mirrored video layers in the frame plan: a video layer's own mirror reaches its plan (a
//! separated camera keeps its bubble's), with its effect passes too (that the picture is flipped before them is the
//! GPU test's, `gpu/mirror.rs`). A mirrored camera-only clip frames and zooms the side the person sees:
//! its crop is picked as shown.

mod support;
use klipolo_engine::plan::{VideoRole, frame_plan};
use klipolo_model::layout::BoxN;
use klipolo_model::project::FormatKind::{Landscape, Portrait};
use klipolo_model::project::{FormatKind, RecordingKind};
use klipolo_model::timeline::{EffectInstance, Framing, FramingMode, ItemContent, ZoomTarget};
use support::*;

/// The overlay video at 2 s, mirrored or not, with a blur or without: its mirror, its `alone` copy's, and how many
/// effect passes it has.
fn planned(mirror: bool, blur: bool) -> (bool, bool, usize) {
    let mut p = project();
    let content = ItemContent::Video {
        asset_id: "broll".into(),
        source_in_us: 0,
        speed: 1.0,
        volume: 1.0,
        muted: true,
        mirror,
        tie: None,
    };
    let mut layer = item("v", 1.0, 3.0, bx(0.5, 0.5, 0.25, 0.25), content);
    if blur {
        layer.effects = vec![EffectInstance::new("klipolo.builtin/effect.blur")];
    }
    p.timeline.tracks = vec![overlay_track("o", vec![layer])];
    let plan = frame_plan(&ctx(p), secs(2.0), Landscape);
    let v = (videos(&plan).into_iter())
        .find(|v| v.role == VideoRole::Overlay)
        .expect("the layer is planned");
    (v.mirror, v.alone().mirror, v.effects.len())
}

#[test]
fn a_video_layer_plans_its_own_mirror() {
    assert_eq!(planned(true, false), (true, true, 0));
    assert_eq!(planned(false, false), (false, false, 0));
}

/// The plan keeps a mirrored layer's mirror beside its effect passes.
#[test]
fn a_mirrored_layer_with_an_effect_keeps_its_mirror_in_the_plan() {
    assert_eq!(planned(true, true), (true, true, 1));
}

/// The fixture's recording as a Camera only take (the camera is the main footage), mirrored or not.
fn camera_only(mirror: bool) -> klipolo_model::Project {
    let mut p = project();
    let rec = &mut p.recordings[0];
    rec.sources.remove(0);
    rec.sources[0].runs[0].offset_us = 0;
    rec.primary = "camera".into();
    rec.kind = RecordingKind::Camera;
    p.timeline.main[0].camera.as_mut().unwrap().mirror = mirror;
    p
}

/// The main footage's crop at `at` seconds in `format`, in the file's coordinates.
fn crop(p: klipolo_model::Project, at: f64, format: FormatKind) -> BoxN {
    let v = videos(&frame_plan(&ctx(p), secs(at), format))[0].clone();
    assert_eq!(
        (v.role, v.source.asset_id.as_ref()),
        (VideoRole::Main, "camera")
    );
    v.crop
}

/// "Keep the right side in view in the vertical version": the right of the picture as shown, which is the file's left
/// when the camera is mirrored.
#[test]
fn a_mirrored_camera_only_clip_keeps_the_side_it_shows_in_view() {
    let focus_right = |mirror| {
        let mut p = camera_only(mirror);
        let framing = Framing {
            mode: FramingMode::Fill,
            focus_x: 1.0,
            focus_y: 0.5,
            follow: None,
        };
        p.timeline.main[0].framing.insert(Portrait, framing);
        crop(p, 2.0, Portrait)
    };
    // A 16:9 camera filling 9:16 shows 0.3164 of its width.
    close(focus_right(false), bx(0.68359375, 0.0, 0.31640625, 1.0));
    close(focus_right(true), bx(0.0, 0.0, 0.31640625, 1.0));
}

/// A zoom on the top right (and the zoom's "Area across" slider moved right) zooms where the person sees it.
#[test]
fn a_mirrored_camera_only_clip_zooms_the_side_it_shows() {
    let zoomed = |mirror, rect| {
        let mut p = camera_only(mirror);
        let zoom = ItemContent::Zoom {
            target: ZoomTarget::Rect { rect },
            scale: 2.0,
        };
        p.timeline.tracks = vec![zoom_track(vec![item(
            "z",
            1.0,
            3.0,
            bx(0.0, 0.0, 1.0, 1.0),
            zoom,
        )])];
        crop(p, 2.5, Landscape)
    };
    let near = |got: BoxN, want: BoxN| {
        let off = [
            got.x - want.x,
            got.y - want.y,
            got.w - want.w,
            got.h - want.h,
        ];
        assert!(
            off.iter().all(|d| d.abs() < 0.02),
            "{got:?}, wanted {want:?}"
        );
    };
    let top_right = bx(0.5, 0.0, 0.5, 0.5);
    near(zoomed(false, top_right), bx(0.5, 0.0, 0.5, 0.5));
    near(zoomed(true, top_right), bx(0.0, 0.0, 0.5, 0.5));
    // The slider a little right of the middle: its mirror image in the file.
    let across = bx(0.3, 0.25, 0.5, 0.5);
    near(zoomed(false, across), bx(0.3, 0.25, 0.5, 0.5));
    near(zoomed(true, across), bx(0.2, 0.25, 0.5, 0.5));
}
