//! The blurred video background (`BackgroundKind::Blur`): where a clip leaves some of the frame uncovered (Fit's bars,
//! or the background's padding), the same footage covers the frame under it, cut as the clip cuts it, blurred and
//! dimmed over black, with the clip's look. A clip that covers the frame plans none.

mod support;

use klipolo_engine::plan::framing::{FULL, cover_crop};
use klipolo_engine::plan::{BackgroundPlan, FramePlan, VideoLayer, VideoRole, frame_plan};
use klipolo_engine::render::pool::DecoderPool;
use klipolo_engine::render::{EXPORT_TUNING, PREVIEW_TUNING};
use klipolo_model::contrib::builtin;
use klipolo_model::crop::{Crop, within};
use klipolo_model::project::FormatKind::{Landscape, Portrait};
use klipolo_model::project::{BackgroundKind, FormatKind, Project, RecordingKind};
use klipolo_model::timeline::{Framing, FramingMode};
use support::{close, ctx, project, secs, videos};

fn fit() -> Framing {
    Framing {
        mode: FramingMode::Fit,
        focus_x: 0.5,
        focus_y: 0.5,
        follow: None,
    }
}

/// The fixture with the blurred background, its b-roll fitted in portrait.
fn blurred() -> Project {
    let mut p = project();
    p.background.kind = BackgroundKind::Blur;
    p.timeline.main[1].framing.insert(Portrait, fit());
    p
}

fn plan_at(p: &Project, t: f64, format: FormatKind) -> FramePlan {
    frame_plan(&ctx(p.clone()), secs(t), format)
}

fn role(plan: &FramePlan, role: VideoRole) -> Option<VideoLayer> {
    videos(plan).into_iter().find(|v| v.role == role).cloned()
}

#[test]
fn a_fitted_clip_has_its_own_footage_blurred_behind_it() {
    let plan = plan_at(&blurred(), 25.0, Portrait);
    assert_eq!(plan.background, BackgroundPlan::Solid { color: [0, 0, 0] });
    let v = videos(&plan);
    assert_eq!(
        v.iter().map(|v| v.role).collect::<Vec<_>>(),
        [VideoRole::Backdrop, VideoRole::Main],
        "under the footage"
    );
    let (back, main) = (v[0], v[1]);
    // The same frame of the same file, so the decoder reads it once.
    assert_eq!(back.source, main.source);
    // It covers the frame from the middle of the 16:9 picture.
    close(back.dest, FULL);
    close(back.crop, cover_crop(16.0 / 9.0, 9.0 / 16.0, 0.5, 0.5));
    // Blurred and dimmed.
    assert_eq!(back.effects.len(), 1);
    assert_eq!(back.effects[0].shader.id.as_str(), builtin::EFFECT_BLUR);
    assert!(back.opacity > 0.3 && back.opacity < 0.9, "{}", back.opacity);
    assert_eq!(back.id.as_ref(), "backdrop:b");
}

#[test]
fn a_clip_that_covers_the_frame_has_nothing_behind_it() {
    let p = blurred();
    // Landscape: the 16:9 video fills 16:9 whole.
    assert!(role(&plan_at(&p, 25.0, Landscape), VideoRole::Backdrop).is_none());
    // Fill in portrait covers the frame too.
    let mut filled = p.clone();
    filled.timeline.main[1].framing.clear();
    assert!(role(&plan_at(&filled, 25.0, Portrait), VideoRole::Backdrop).is_none());
    // Other backgrounds never plan one.
    let mut solid = p;
    solid.background.kind = BackgroundKind::Solid;
    assert!(role(&plan_at(&solid, 25.0, Portrait), VideoRole::Backdrop).is_none());
}

#[test]
fn the_padding_shows_it_round_a_filled_clip() {
    let mut p = blurred();
    p.timeline.main[1].framing.clear();
    p.background.padding = 0.1;
    let back = role(&plan_at(&p, 25.0, Portrait), VideoRole::Backdrop).expect("behind the inset");
    close(back.dest, FULL);
}

#[test]
fn it_is_cut_as_the_clip_is_cut_and_mirrored_as_it_is() {
    let mut p = blurred();
    let c = Crop {
        left: 0.25,
        right: 0.25,
        ..Crop::NONE
    };
    p.timeline.main[1].crop = Some(c);
    let back = role(&plan_at(&p, 25.0, Portrait), VideoRole::Backdrop).unwrap();
    close(
        back.crop,
        within(
            c.kept(),
            cover_crop((16.0 / 9.0) * 0.5, 9.0 / 16.0, 0.5, 0.5),
        ),
    );
    // A mirrored camera-only take: mirrored behind as in front.
    let mut cam = blurred();
    let rec = &mut cam.recordings[0];
    rec.sources.remove(0);
    rec.sources[0].runs[0].offset_us = 0;
    rec.primary = "camera".into();
    rec.kind = RecordingKind::Camera;
    let clip = &mut cam.timeline.main[0];
    clip.camera.as_mut().unwrap().mirror = true;
    clip.framing.insert(Portrait, fit());
    clip.crop = Some(Crop {
        left: 0.4,
        ..Crop::NONE
    });
    let plan = plan_at(&cam, 2.0, Portrait);
    let (back, main) = (
        role(&plan, VideoRole::Backdrop).unwrap(),
        role(&plan, VideoRole::Main).unwrap(),
    );
    assert!(back.mirror && main.mirror);
    // The left 40 % the person cut is the file's right 40 %: neither shows it.
    assert!(back.crop.x + back.crop.w <= 0.6 + 1e-9, "{:?}", back.crop);
    assert!(main.crop.x + main.crop.w <= 0.6 + 1e-9, "{:?}", main.crop);
}

#[test]
fn it_takes_the_clips_look_but_not_its_effects_or_zoom() {
    let mut p = blurred();
    p.timeline.main[1].look.preset = Some(builtin::look("warm"));
    let plan = plan_at(&p, 25.0, Portrait);
    let (back, main) = (
        role(&plan, VideoRole::Backdrop).unwrap(),
        role(&plan, VideoRole::Main).unwrap(),
    );
    assert!(back.look.is_some());
    assert_eq!(back.look, main.look);
    assert!(main.effects.is_empty());
}

#[test]
fn its_frames_decode_with_the_main_footages_budget() {
    // The plan lists it first, so its request is the one both layers' frame is decoded for.
    for tuning in [PREVIEW_TUNING, EXPORT_TUNING] {
        let pool = DecoderPool::new(tuning, false);
        for lane in [0, 1] {
            let (back, main) = (
                pool.tuning_for(VideoRole::Backdrop, lane),
                pool.tuning_for(VideoRole::Main, lane),
            );
            assert_eq!(back.max_output, main.max_output);
            assert_eq!(back.max_cache_bytes, main.max_cache_bytes);
        }
    }
}
