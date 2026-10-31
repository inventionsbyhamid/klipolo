//! Which part of a main clip's footage its Fill shows from the centre (`fill_crop`): the same crop the frame plan draws,
//! so the clip card offers the Fill focus only the way Fill crops.

mod support;

use klipolo_engine::plan::fill_crop;
use klipolo_engine::plan::frame_plan;
use klipolo_model::project::FormatKind::{self, Landscape, Portrait, Square};
use klipolo_model::project::Project;
use klipolo_model::timeline::{Framing, FramingMode};
use support::{bx, close, ctx, project, secs, videos};

const CENTRE_FILL: Framing = Framing {
    mode: FramingMode::Fill,
    focus_x: 0.5,
    focus_y: 0.5,
    follow: None,
};

/// The fixture (a screen recording with a camera, split in portrait by default, then a 16:9 video) filling from the
/// centre in every format, with the background inset `padding`.
fn filled(padding: f64) -> Project {
    let mut p = project();
    p.background.padding = padding;
    p.settings.formats = vec![Landscape, Portrait, Square];
    for clip in &mut p.timeline.main {
        for format in [Landscape, Portrait, Square] {
            clip.framing.insert(format, CENTRE_FILL);
        }
    }
    p
}

/// The main footage's crop the frame plan draws at `t`.
fn drawn(p: &Project, t: f64, format: FormatKind) -> klipolo_model::layout::BoxN {
    let plan = frame_plan(&ctx(p.clone()), secs(t), format);
    videos(&plan)[0].crop
}

#[test]
fn a_clips_fill_crop_is_the_one_the_frame_plan_draws() {
    for padding in [0.0, 0.1] {
        let p = filled(padding);
        for format in [Landscape, Portrait, Square] {
            // The recording (0 to 20 s): its screen, in the split's top pane in portrait.
            let a = fill_crop(&p, &p.timeline.main[0], format).expect("the screen is a video");
            close(a, drawn(&p, 5.0, format));
            // The imported video (20 to 30 s).
            let b = fill_crop(&p, &p.timeline.main[1], format).expect("broll is a video");
            close(b, drawn(&p, 25.0, format));
        }
    }
}

#[test]
fn a_16_9_video_crops_across_in_9_16_and_1_1_and_not_at_all_in_16_9() {
    let p = filled(0.0);
    let b = &p.timeline.main[1];
    close(fill_crop(&p, b, Landscape).unwrap(), bx(0.0, 0.0, 1.0, 1.0));
    let w = (1080.0 / 1920.0) / (1920.0 / 1080.0);
    close(
        fill_crop(&p, b, Portrait).unwrap(),
        bx((1.0 - w) / 2.0, 0.0, w, 1.0),
    );
    close(
        fill_crop(&p, b, Square).unwrap(),
        bx(0.21875, 0.0, 0.5625, 1.0),
    );
    // A 4:3 video in 16:9 crops down.
    let mut p = filled(0.0);
    let broll = p.assets.iter_mut().find(|a| a.id == "broll").unwrap();
    (broll.width, broll.height) = (1440, 1080);
    let crop = fill_crop(&p, &p.timeline.main[1], Landscape).unwrap();
    close(crop, bx(0.0, 0.125, 1.0, 0.75));
    // A clip whose file isn't a video draws nothing, so it has no crop.
    p.timeline.main[1].source = klipolo_model::timeline::ClipSource::Asset { id: "logo".into() };
    assert_eq!(fill_crop(&p, &p.timeline.main[1], Landscape), None);
}
