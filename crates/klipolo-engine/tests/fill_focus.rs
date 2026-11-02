//! A main clip's Fill focus beyond its framing's fixed point (`Clip::fill_focus`): the centre, or the speaker's path.
//! The frame plan is what both the preview and an export draw, so a crop checked here is the crop in both.

mod support;

use klipolo_engine::plan::{VideoRole, frame_plan};
use klipolo_model::apply_ops;
use klipolo_model::crop::Crop;
use klipolo_model::fill_focus::{FillFocus, FocusKey, FocusPath};
use klipolo_model::layout::BoxN;
use klipolo_model::ops::Op;
use klipolo_model::project::FormatKind::{self, Landscape, Portrait, Square};
use klipolo_model::project::{Project, RecordingKind};
use klipolo_model::timeline::{Framing, FramingMode, ReverseCopy};
use support::{bx, close, ctx, project, secs, videos};

/// A 16:9 picture filling 9:16 shows this share of its width.
const PORTRAIT_W: f64 = (1080.0 / 1920.0) / (1920.0 / 1080.0);

fn key(at_s: f64, x: f64, y: f64, cut: bool) -> FocusKey {
    FocusKey {
        at_us: secs(at_s),
        x,
        y,
        cut,
    }
}

fn fill(focus_x: f64) -> Framing {
    Framing {
        mode: FramingMode::Fill,
        focus_x,
        focus_y: 0.5,
        follow: None,
    }
}

/// The fixture's imported video (clip b, 20 to 30 s on the timeline, its source from 0) filling every format, with
/// `focus` in `format`.
fn with_focus(format: FormatKind, focus: FillFocus) -> Project {
    let mut p = project();
    p.settings.formats = vec![Landscape, Portrait, Square];
    let b = &mut p.timeline.main[1];
    for f in [Landscape, Portrait, Square] {
        b.framing.insert(f, fill(0.1));
    }
    b.fill_focus.insert(format, focus);
    p
}

/// The speaker moving right over the first 5 s of the source, then a new shot at 8 s with them on the left.
fn speaker() -> FillFocus {
    FillFocus::speaker(FocusPath {
        keys: vec![
            key(0.0, 0.3, 0.5, false),
            key(5.0, 0.7, 0.5, false),
            key(8.0, 0.2, 0.5, true),
        ],
    })
}

/// The main footage's crop the frame plan draws at `t` seconds on the timeline.
fn drawn(p: &Project, t: f64, format: FormatKind) -> BoxN {
    let plan = frame_plan(&ctx(p.clone()), secs(t), format);
    let v = videos(&plan)[0].clone();
    assert_eq!(v.role, VideoRole::Main);
    v.crop
}

/// A crop of the portrait width centred on `x` (kept inside the picture).
fn portrait_at(x: f64) -> BoxN {
    let left = (x - PORTRAIT_W / 2.0).clamp(0.0, 1.0 - PORTRAIT_W);
    bx(left, 0.0, PORTRAIT_W, 1.0)
}

#[test]
fn the_crop_follows_the_speakers_path_in_source_time() {
    let p = with_focus(Portrait, speaker());
    // Source 0 s, 2.5 s (halfway between the first two keys) and 5 s.
    close(drawn(&p, 20.0, Portrait), portrait_at(0.3));
    close(drawn(&p, 22.5, Portrait), portrait_at(0.5));
    close(drawn(&p, 25.0, Portrait), portrait_at(0.7));
    // It holds up to the new shot and jumps at it, never panning across the cut.
    close(drawn(&p, 27.9, Portrait), portrait_at(0.7));
    close(drawn(&p, 28.0, Portrait), portrait_at(0.2));
    // Another format without a focus of its own keeps its framing's fixed point.
    let fixed = drawn(&p, 22.5, Square);
    close(fixed, bx((1.0 - 0.5625) * 0.1, 0.0, 0.5625, 1.0));
}

#[test]
fn a_trimmed_clip_reads_the_path_at_its_own_source_time() {
    let mut p = with_focus(Portrait, speaker());
    // Trimmed to start 2.5 s into its source: its first frame is the path's middle.
    p.timeline.main[1].source_in_us = secs(2.5);
    close(drawn(&p, 20.0, Portrait), portrait_at(0.5));
}

#[test]
fn centre_keeps_the_middle_whatever_the_focus_point() {
    let p = with_focus(Portrait, FillFocus::Center);
    close(drawn(&p, 22.0, Portrait), portrait_at(0.5));
}

#[test]
fn a_frame_the_footage_fills_exactly_or_a_fit_ignores_the_path() {
    // 16:9 footage in 16:9 crops nothing: the path has nowhere to move it.
    let p = with_focus(Landscape, speaker());
    close(drawn(&p, 22.5, Landscape), bx(0.0, 0.0, 1.0, 1.0));
    // Fit shows the whole picture.
    let mut p = with_focus(Portrait, speaker());
    p.timeline.main[1].framing.get_mut(&Portrait).unwrap().mode = FramingMode::Fit;
    close(drawn(&p, 22.5, Portrait), bx(0.0, 0.0, 1.0, 1.0));
}

#[test]
fn a_mirrored_camera_only_clip_follows_the_speaker_where_the_file_has_them() {
    let mut p = project();
    let rec = &mut p.recordings[0];
    rec.sources.remove(0);
    rec.sources[0].runs[0].offset_us = 0;
    rec.primary = "camera".into();
    rec.kind = RecordingKind::Camera;
    let a = &mut p.timeline.main[0];
    a.camera.as_mut().unwrap().mirror = true;
    a.framing.insert(Portrait, fill(0.5));
    // The path is where the tracker saw the speaker in the file, before the mirror.
    a.fill_focus.insert(
        Portrait,
        FillFocus::speaker(FocusPath {
            keys: vec![key(0.0, 0.25, 0.5, false)],
        }),
    );
    let v = videos(&frame_plan(&ctx(p), secs(2.0), Portrait))[0].clone();
    assert_eq!(v.source.asset_id.as_ref(), "camera");
    assert!(v.mirror);
    close(v.crop, portrait_at(0.25));
}

#[test]
fn a_cropped_clip_keeps_its_focus_inside_what_the_crop_leaves() {
    // The left fifth cut off: the same height, so Fill shows the same share of the file's width.
    let left = Crop {
        left: 0.2,
        top: 0.0,
        right: 0.0,
        bottom: 0.0,
    };
    let in_kept = |x: f64| {
        let w = PORTRAIT_W;
        bx((x - w / 2.0).clamp(0.2, 1.0 - w), 0.0, w, 1.0)
    };
    // Centre is the middle of what is left, not of the file.
    let mut p = with_focus(Portrait, FillFocus::Center);
    p.timeline.main[1].crop = Some(left);
    close(drawn(&p, 22.0, Portrait), in_kept(0.6));
    // The speaker's path is in the whole file; the crop never shows what the clip's crop cut off.
    let mut p = with_focus(Portrait, speaker());
    p.timeline.main[1].crop = Some(left);
    close(drawn(&p, 20.0, Portrait), in_kept(0.3));
    close(drawn(&p, 25.0, Portrait), in_kept(0.7));
    assert!(drawn(&p, 28.0, Portrait).x >= 0.2 - 1e-9);
}

/// `p` with clip b (the b-roll's 0 to 10 s) played backward from a copy of that stretch written backward.
fn reversed(p: &Project) -> Project {
    let mut copy = p.asset("broll").unwrap().clone();
    copy.id = "broll-rev".into();
    copy.path = "/media/broll reversed.mov".into();
    let turn = Op::ReverseClip {
        clip_id: p.timeline.main[1].id.clone(),
        copy: Some(ReverseCopy {
            asset_id: copy.id.clone(),
            from_us: 0,
            to_us: secs(10.0),
        }),
    };
    apply_ops(p, &[Op::AddAssets { assets: vec![copy] }, turn]).unwrap()
}

#[test]
fn a_reversed_clip_follows_the_speaker_on_the_footage_it_shows() {
    let forward = with_focus(Portrait, speaker());
    let back = reversed(&forward);
    assert!(back.timeline.main[1].reversed.is_some());
    // At 20 + t s on the timeline the reversed clip shows the b-roll's 10 - t s: the crop is where it was then.
    for t in [0.5, 1.0, 1.9, 2.1, 3.0, 4.5, 5.0, 7.5, 9.9] {
        close(
            drawn(&back, 20.0 + t, Portrait),
            drawn(&forward, 30.0 - t, Portrait),
        );
    }
    // The later shot plays first: the speaker on the left, then the crop jumps right as the earlier shot comes.
    close(drawn(&back, 21.0, Portrait), portrait_at(0.2));
    close(drawn(&back, 22.5, Portrait), portrait_at(0.7));
    // Forward again, it follows as it did.
    let again = reversed_back(&back);
    for t in [20.0, 22.5, 25.0, 27.9, 28.0, 29.5] {
        close(drawn(&again, t, Portrait), drawn(&forward, t, Portrait));
    }
}

/// `p` with its reversed clip b played forward again.
fn reversed_back(p: &Project) -> Project {
    let turn = Op::ReverseClip {
        clip_id: p.timeline.main[1].id.clone(),
        copy: None,
    };
    apply_ops(p, &[turn]).unwrap()
}
