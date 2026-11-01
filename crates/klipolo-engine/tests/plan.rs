mod support;
use std::sync::Arc;

use klipolo_engine::plan::color::parse_hex;
use klipolo_engine::plan::framing::FULL;
use klipolo_engine::plan::{
    BackgroundPlan, GRADIENT_ANGLE_DEG, MediaCopy, MediaMap, PlanContext, VideoRole, frame_plan,
    last_frame_us,
};
use klipolo_model::Project;
use klipolo_model::ops::SourcePatch;
use klipolo_model::project::FormatKind::*;
use klipolo_model::project::{BackgroundKind, RecordingKind, SourceRun};
use klipolo_model::timeline::{CameraMode, Clip, ClipSource, Framing, FramingMode};
use support::*;

#[test]
fn landscape_recording_shows_screen_and_camera_bubble() {
    let plan = frame_plan(&ctx(project()), secs(2.0), Landscape);
    let v = videos(&plan);
    assert_eq!(v.len(), 2);
    assert_eq!(
        (
            v[0].role,
            v[0].id.as_ref(),
            v[0].source.asset_id.as_ref(),
            v[0].source.time_us
        ),
        (VideoRole::Main, "a", "screen", secs(2.0))
    );
    close(v[0].dest, FULL);
    close(v[0].crop, FULL);
    // Camera source time = screen time − 0.2 s offset.
    assert_eq!(
        (
            v[1].role,
            v[1].id.as_ref(),
            v[1].source.asset_id.as_ref(),
            v[1].source.time_us
        ),
        (VideoRole::Camera, "camera:a", "camera", secs(1.8))
    );
    close(v[1].dest, bx(0.76, 0.6044, 0.2, 0.3556));
    // 16:9 camera filling a 384×384.048 px bubble: crop width 0.99987/1.77778 = 0.562430, centred.
    close(v[1].crop, bx(0.218785, 0.0, 0.562430, 1.0));
    // Mirrored by default.
    assert_eq!((v[1].radius, v[1].border, v[1].mirror), (0.5, true, true));
}

#[test]
fn portrait_splits_screen_over_camera() {
    let plan = frame_plan(&ctx(project()), secs(2.0), Portrait);
    let v = videos(&plan);
    // Screen fits the 1080×960 top pane: 1080×607.5 at y = 176.25 px.
    close(v[0].dest, bx(0.0, 0.091796875, 1.0, 0.31640625));
    close(v[0].crop, FULL);
    // Camera fills the bottom pane (aspect 1.125): crop width 1.125/1.7778 = 0.6328125.
    close(v[1].dest, bx(0.0, 0.5, 1.0, 0.5));
    close(v[1].crop, bx(0.18359375, 0.0, 0.6328125, 1.0));
    assert_eq!((v[1].radius, v[1].border), (0.0, false));
}

/// The clip's framing reaches the split's screen pane, as it does the full frame. "Fill it" fills the 1080×960 top pane
/// from the centre; "keep the left side in view" from the left.
#[test]
fn the_split_screen_pane_takes_the_clips_framing() {
    let filled = |focus_x| {
        let mut p = project();
        p.timeline.main[0].framing.insert(
            Portrait,
            Framing {
                mode: FramingMode::Fill,
                focus_x,
                focus_y: 0.5,
                follow: None,
            },
        );
        p
    };
    let screen = |p, format| {
        let plan = frame_plan(&ctx(p), secs(2.0), format);
        let v = videos(&plan);
        assert_eq!(v[0].role, VideoRole::Main);
        (v[0].dest, v[0].crop, v[1].dest)
    };
    // The pane, filled: crop width 1.125/1.7778 = 0.6328125. The camera's bottom pane is unchanged.
    for (focus_x, crop_x) in [(0.5, 0.18359375), (0.0, 0.0)] {
        let (dest, crop, camera) = screen(filled(focus_x), Portrait);
        close(dest, bx(0.0, 0.0, 1.0, 0.5));
        close(crop, bx(crop_x, 0.0, 0.6328125, 1.0));
        close(camera, bx(0.0, 0.5, 1.0, 0.5));
    }
    // The landscape version keeps its own framing (none: fit).
    close(screen(filled(0.0), Landscape).1, FULL);
}

#[test]
fn padding_insets_the_footage() {
    let mut p = project();
    p.background.padding = 0.1; // 108 px on every edge
    let c = ctx(p);
    // Recording: Fit inside 1704×864 → 1536×864 centred.
    close(
        videos(&frame_plan(&c, secs(2.0), Landscape))[0].dest,
        bx(0.1, 0.1, 0.8, 0.8),
    );
    // Imported video: Fill the padded area; crop height 1.7778/1.9722 = 0.901408.
    let broll = frame_plan(&c, secs(25.0), Landscape);
    let v = videos(&broll);
    assert_eq!((v[0].id.as_ref(), v[0].source.time_us), ("b", secs(5.0)));
    close(v[0].dest, bx(0.05625, 0.1, 0.8875, 0.8));
    close(v[0].crop, bx(0.0, 0.049296, 1.0, 0.901408));
}

#[test]
fn imported_video_fills_portrait_from_the_centre() {
    let plan = frame_plan(&ctx(project()), secs(21.0), Portrait);
    close(videos(&plan)[0].dest, FULL);
    close(videos(&plan)[0].crop, bx(0.341796875, 0.0, 0.31640625, 1.0));
}

#[test]
fn framing_overrides_apply_per_format() {
    let mut p = project();
    p.timeline.main[1].framing.insert(
        Portrait,
        Framing {
            mode: FramingMode::Fit,
            focus_x: 0.5,
            focus_y: 0.5,
            follow: None,
        },
    );
    p.timeline.main[1].framing.insert(
        Square,
        Framing {
            mode: FramingMode::Fill,
            focus_x: 0.0,
            focus_y: 0.5,
            follow: None,
        },
    );
    let c = ctx(p);
    close(
        videos(&frame_plan(&c, secs(21.0), Portrait))[0].dest,
        bx(0.0, 0.341796875, 1.0, 0.31640625),
    );
    close(
        videos(&frame_plan(&c, secs(21.0), Square))[0].crop,
        bx(0.0, 0.0, 0.5625, 1.0),
    );
    close(videos(&frame_plan(&c, secs(21.0), Landscape))[0].crop, FULL);
}

#[test]
fn boundary_shows_next_clip() {
    let c = ctx(project());
    let before = frame_plan(&c, secs(20.0) - 1, Landscape);
    assert_eq!(
        (
            videos(&before)[0].id.as_ref(),
            videos(&before)[0].source.time_us
        ),
        ("a", secs(20.0) - 1)
    );
    let at = frame_plan(&c, secs(20.0), Landscape);
    assert_eq!(
        (videos(&at)[0].id.as_ref(), videos(&at)[0].source.time_us),
        ("b", 0)
    );
}

#[test]
fn end_of_timeline_is_empty_and_last_frame_is_clamped() {
    let c = ctx(project());
    assert!(videos(&frame_plan(&c, secs(30.0), Landscape)).is_empty());
    assert_eq!(last_frame_us(secs(30.0), 30), 29_966_667);
    assert_eq!(last_frame_us(0, 30), 0);
    assert_eq!(&*videos(&frame_plan(&c, 29_966_667, Landscape))[0].id, "b");
}

#[test]
fn clip_speed_scales_source_time() {
    let mut p = project();
    p.timeline.main[0].speed = 2.0;
    let plan = frame_plan(&ctx(p), secs(3.0), Landscape);
    assert_eq!(videos(&plan)[0].source.time_us, secs(6.0));
    assert_eq!(videos(&plan)[1].source.time_us, secs(5.8));
}

#[test]
fn camera_layout_is_stable_outside_its_footage() {
    let mut p = project();
    p.recordings[0].sources[1].runs[0].offset_us = secs(3.0);
    let c = ctx(p);
    // Before the camera starts, the bubble holds its first frame (no jump when its footage begins).
    let early = frame_plan(&c, secs(1.0), Landscape);
    assert_eq!(videos(&early).len(), 2);
    assert_eq!(videos(&early)[1].source.time_us, 0);
    assert_eq!(
        videos(&frame_plan(&c, secs(4.0), Landscape))[1]
            .source
            .time_us,
        secs(1.0)
    );
    // Portrait keeps the split for the whole clip: the screen stays in the top pane.
    close(
        videos(&frame_plan(&c, secs(1.0), Portrait))[0].dest,
        bx(0.0, 0.091796875, 1.0, 0.31640625),
    );
}

#[test]
fn a_screen_holds_its_nearest_frame_across_a_gap_between_runs() {
    // The screen's encoder was rebuilt at 10 s: its first file holds 0–10 s of the take, the second starts at 12 s.
    let mut p = project();
    let first = p.assets.iter_mut().find(|a| a.id == "screen").unwrap();
    first.duration_us = secs(10.0);
    let mut second = first.clone();
    (second.id, second.path, second.duration_us) =
        ("screen-2".into(), "/media/screen-2".into(), secs(18.0));
    p.assets.push(second);
    let screen = &mut p.recordings[0].sources[0];
    screen.runs[0].duration_us = secs(10.0);
    screen.runs.push(SourceRun {
        asset_id: "screen-2".into(),
        offset_us: secs(12.0),
        duration_us: secs(18.0),
        drift_ppm: 0.0,
        file_in_us: 0,
    });
    let c = ctx(p);
    let main_at = |t: f64| {
        let plan = frame_plan(&c, secs(t), Landscape);
        let v = videos(&plan)[0];
        (v.source.asset_id.to_string(), v.source.time_us)
    };
    // Inside the gap the nearer run holds its frame: run 1's last, then run 2's first.
    assert_eq!(main_at(10.5), ("screen".into(), secs(10.0) - 1));
    assert_eq!(main_at(11.5), ("screen-2".into(), 0));
    assert_eq!(main_at(13.0), ("screen-2".into(), secs(1.0)));
}

/// A sync change moves a separated camera exactly as it moves the clip's own: split at 10 s, the
/// camera separated in both pieces, then nudged 200 ms later, its layers show the frames the clips' own camera shows
/// after the same nudge, frame by frame across the cut.
#[test]
fn a_sync_change_moves_a_separated_camera_as_it_moves_the_clip_s_own() {
    let mut p = project();
    support::recording::split_at(&mut p, 10.0);
    let nudge = Op::UpdateRecordingSource {
        recording_id: "rec".into(),
        source_id: "camera".into(),
        patch: SourcePatch {
            nudge_us: Some(200_000),
            ..SourcePatch::default()
        },
    };
    let mut separate: Vec<Op> = p
        .timeline
        .main
        .iter()
        .filter(|c| matches!(c.source, ClipSource::Recording { .. }))
        .map(|c| Op::DetachSource {
            clip_id: c.id.clone(),
            source_id: "camera".into(),
        })
        .collect();
    assert_eq!(separate.len(), 2);
    separate.push(nudge.clone());
    // Every frame of the second around the cut, and a moment in each piece.
    let mut at: Vec<i64> = (0..30).map(|k| secs(9.5) + k * 33_333).collect();
    at.extend([secs(1.0), secs(15.0)]);
    let camera = |p: Project| {
        let c = ctx(p);
        at.iter()
            .map(|&t| {
                let plan = frame_plan(&c, t, Landscape);
                videos(&plan)
                    .iter()
                    .find(|v| v.source.asset_id.as_ref() == "camera")
                    .map(|v| v.source.time_us)
            })
            .collect::<Vec<_>>()
    };
    let heard = camera(klipolo_model::apply_ops(&p, &[nudge]).unwrap());
    let separated = camera(klipolo_model::apply_ops(&p, &separate).unwrap());
    assert!(heard.iter().all(Option::is_some));
    assert_eq!(separated, heard);
}

#[test]
fn hidden_camera_mode_shows_only_the_screen() {
    let mut p = project();
    p.timeline.main[0]
        .camera
        .as_mut()
        .unwrap()
        .mode_overrides
        .insert(Square, CameraMode::Hidden);
    assert_eq!(videos(&frame_plan(&ctx(p), secs(2.0), Square)).len(), 1);
}

#[test]
fn camera_only_recording_fills_and_mirrors() {
    let mut p = project();
    // A camera-only take: the camera is the main footage, from the take's start.
    let rec = &mut p.recordings[0];
    rec.sources.remove(0);
    rec.sources[0].runs[0].offset_us = 0;
    rec.primary = "camera".into();
    rec.kind = RecordingKind::Camera;
    p.timeline.main[0].camera.as_mut().unwrap().mirror = true;
    let plan = frame_plan(&ctx(p), secs(2.0), Portrait);
    let v = videos(&plan);
    assert_eq!(v.len(), 1);
    assert_eq!(
        (
            v[0].role,
            v[0].source.asset_id.as_ref(),
            v[0].source.time_us,
            v[0].mirror
        ),
        (VideoRole::Main, "camera", secs(2.0), true)
    );
    close(v[0].crop, bx(0.341796875, 0.0, 0.31640625, 1.0));
}

#[test]
fn backgrounds() {
    let mut p = project();
    assert_eq!(
        frame_plan(&ctx(p.clone()), 0, Landscape).background,
        BackgroundPlan::Solid { color: [0, 0, 0] }
    );
    p.background.kind = BackgroundKind::Gradient;
    p.background.color = "#1A2B3C".into();
    p.background.color_end = Some("#FFF".into());
    assert_eq!(
        frame_plan(&ctx(p.clone()), 0, Landscape).background,
        BackgroundPlan::Gradient {
            from: [0x1A, 0x2B, 0x3C],
            to: [255, 255, 255],
            angle_deg: GRADIENT_ANGLE_DEG
        }
    );
    p.background.kind = BackgroundKind::Image;
    p.background.asset_id = Some("missing".into());
    assert_eq!(
        frame_plan(&ctx(p), 0, Landscape).background,
        BackgroundPlan::Solid { color: [0, 0, 0] }
    );
    assert_eq!(parse_hex("nonsense"), [0, 0, 0]);
}

#[test]
fn sources_list_main_then_camera() {
    let plan = frame_plan(&ctx(project()), secs(2.0), Landscape);
    let ids: Vec<&str> = plan.sources().iter().map(|s| s.asset_id.as_ref()).collect();
    assert_eq!(ids, ["screen", "camera"]);
}

#[test]
fn frame_times_use_the_rounded_frame_starts() {
    use klipolo_engine::plan::{frame_start_us, frame_time_us, last_frame_us};
    // Frame 1 at 30 fps starts at 33 333 µs: that instant already shows frame 1 (the playback clock wakes exactly there).
    assert_eq!(frame_time_us(33_333, 30), 33_333);
    assert_eq!(frame_time_us(33_332, 30), 0);
    assert_eq!(frame_time_us(1_000_000, 30), 1_000_000);
    assert_eq!(frame_start_us(31, 30), 1_033_333);
    let last = last_frame_us(secs(0.5), 30);
    assert_eq!(frame_time_us(last, 30), last);
    assert_eq!(frame_time_us(1_234_567, 25), 1_200_000);
}

use klipolo_engine::plan::{Layer, LookPlan, LutRef};
use klipolo_engine::registry::Registry;
use klipolo_model::contrib::builtin;
use klipolo_model::project::ColorLut;
use klipolo_model::timeline::{ItemContent, ZoomTarget};

fn texts(plan: &klipolo_engine::plan::FramePlan) -> Vec<&klipolo_engine::plan::TextLayer> {
    plan.layers
        .iter()
        .filter_map(|l| match l {
            Layer::Text(t) => Some(t),
            _ => None,
        })
        .collect()
}

#[test]
fn text_uses_its_layout_and_a_1080_based_font_size() {
    let mut p = project();
    p.timeline.tracks = vec![overlay_track(
        "t",
        vec![item(
            "title",
            1.0,
            3.0,
            bx(0.05, 0.05, 0.4, 0.1),
            text("Hello"),
        )],
    )];
    let c = ctx(p);
    let land = frame_plan(&c, secs(2.0), Landscape);
    close(texts(&land)[0].dest, bx(0.05, 0.05, 0.4, 0.1));
    assert_eq!(texts(&land)[0].font_px, 72.0);
    let port = frame_plan(&c, secs(2.0), Portrait);
    close(
        texts(&port)[0].dest,
        bx(0.088889, 0.028125, 0.711111, 0.05625),
    );
    // A derived box keeps the font; a format with its own placement scales it with the box's height (D: text scales
    // with its box): a square box twice as tall in pixels draws the font twice as big.
    assert!((texts(&port)[0].font_px - 72.0).abs() < 1e-9);
    let mut p = c.project.as_ref().clone();
    p.timeline.tracks[0].items[0]
        .layout
        .overrides
        .insert(Square, bx(0.1, 0.1, 0.8, 0.2));
    let square = frame_plan(&ctx(p), secs(2.0), Square);
    assert!((texts(&square)[0].font_px - 144.0).abs() < 1e-9);
    assert!(
        frame_plan(&c, secs(4.0), Landscape)
            .layers
            .iter()
            .all(|l| !matches!(l, Layer::Text(_))),
        "ends at 4 s"
    );
}

/// The four text animations are motion presets now (0.3 s each): fade is linear, slide moves out of the canvas,
/// typewriter cuts the text, and pop scales the text about its box, which keeps its size (laid out once).
#[test]
fn text_presets_fade_slide_type_and_pop() {
    use klipolo_model::motion::Ease;
    let mut p = project();
    p.timeline.tracks = vec![overlay_track(
        "t",
        vec![
            animated(
                item("a", 1.0, 3.0, bx(0.05, 0.05, 0.4, 0.1), text("Hello")),
                Some("fade"),
                Some("slide"),
            ),
            animated(
                item("b", 5.0, 3.0, bx(0.05, 0.5, 0.4, 0.1), text("Hello")),
                Some("typewriter"),
                Some("pop"),
            ),
        ],
    )];
    let c = ctx(p);
    let t = |s: f64| texts(&frame_plan(&c, secs(s), Landscape))[0].clone();
    assert!(
        (t(1.15).opacity - 0.5).abs() < 1e-9,
        "half-way through a 0.3 s fade"
    );
    assert_eq!((t(2.0).opacity, t(2.0).dest.y), (1.0, 0.05));
    // Half-way through the slide out (to the bottom, power2.out): 0.875 of the way back, so 1/8 of the way out.
    let out = t(3.85);
    assert!(
        out.opacity == 1.0 && (out.dest.y - (0.05 + 0.95 / 8.0)).abs() < 1e-9,
        "{out:?}"
    );
    assert_eq!(
        (&*t(5.15).text, t(5.15).reveal),
        ("Hel", 0.5),
        "typewriter is linear: ceil(5 × 0.5) = 3 characters"
    );
    let pop = t(7.85);
    assert!((pop.scale - (0.85 + 0.15 * Ease::BackOut.apply(0.5))).abs() < 1e-9);
    close(pop.dest, bx(0.05, 0.5, 0.4, 0.1));
}

/// Presets on both ends combine, neither resetting the other: typewriter in and out types the text in
/// and out, and blur-in in and out blurs the layer in and out (one blur pass while it runs, none in between).
#[test]
fn presets_on_both_ends_combine() {
    let mut p = project();
    let logo = item(
        "logo",
        1.0,
        3.0,
        bx(0.4, 0.4, 0.2, 0.2),
        ItemContent::Image {
            asset_id: "logo".into(),
        },
    );
    p.timeline.tracks = vec![
        overlay_track(
            "t",
            vec![animated(
                item("a", 1.0, 3.0, bx(0.05, 0.05, 0.4, 0.1), text("Hello")),
                Some("typewriter"),
                Some("typewriter"),
            )],
        ),
        overlay_track("i", vec![animated(logo, Some("blur-in"), Some("blur-in"))]),
    ];
    let c = ctx(p);
    let typed = |s: f64| {
        texts(&frame_plan(&c, secs(s), Landscape))[0]
            .text
            .to_string()
    };
    assert_eq!(
        [typed(1.15), typed(2.5), typed(3.85)],
        ["Hel", "Hello", "Hel"]
    );
    let blurs = |s: f64| {
        frame_plan(&c, secs(s), Landscape)
            .layers
            .iter()
            .find_map(|l| match l {
                Layer::Image(i) => Some(i.effects.len()),
                _ => None,
            })
            .unwrap()
    };
    assert_eq!([blurs(1.15), blurs(2.5), blurs(3.85)], [1, 0, 1]);
}

/// The camera bubble's keys run on the recording's source time, so across a cut they stay with the
/// footage; its turn is its own rotation plus its keys' (the composite draws it).
#[test]
fn the_camera_bubble_moves_on_source_time_across_a_cut() {
    use klipolo_model::motion::{Key, Motion, MotionKeys};
    let key = |s: f64, v: f64| Key {
        t_us: secs(s),
        v,
        ease: None,
    };
    let keyed = |mut p: klipolo_model::project::Project| {
        let bubble = &mut p.timeline.main[0].camera.as_mut().unwrap().bubble;
        bubble.rotation = 5.0;
        bubble.motion = Some(Motion {
            keys: Some(MotionKeys {
                x: vec![key(0.0, 0.0), key(20.0, 0.2)],
                rotation: vec![key(0.0, 0.0), key(20.0, 40.0)],
                ..MotionKeys::default()
            }),
            ..Motion::default()
        });
        p
    };
    // The recording's 0–5 s, then its 15–20 s: the timeline's 6 s shows the source's 16 s.
    let cut = |mut p: klipolo_model::project::Project| {
        let mut later = p.timeline.main[0].clone();
        later.id = "a2".into();
        later.source_in_us = secs(15.0);
        p.timeline.main[0].source_out_us = secs(5.0);
        p.timeline.main.insert(1, later);
        p
    };
    let camera = |p| {
        videos(&frame_plan(&ctx(p), secs(6.0), Landscape))
            .into_iter()
            .find(|v| v.role == VideoRole::Camera)
            .cloned()
            .unwrap()
    };
    let still = camera(cut(project()));
    let moved = camera(cut(keyed(project())));
    assert!(
        (moved.dest.x - still.dest.x - 0.16).abs() < 1e-9,
        "{moved:?}"
    );
    assert!((moved.rotation - 37.0).abs() < 1e-9, "5° plus 40° × 16/20");
}

#[test]
fn images_and_videos_keep_their_shape() {
    let mut p = project();
    p.timeline.tracks = vec![overlay_track(
        "o",
        vec![
            item(
                "logo",
                1.0,
                3.0,
                bx(0.1, 0.1, 0.4, 0.2),
                ItemContent::Image {
                    asset_id: "logo".into(),
                },
            ),
            item(
                "clip",
                5.0,
                3.0,
                bx(0.5, 0.5, 0.25, 0.25),
                ItemContent::Video {
                    asset_id: "broll".into(),
                    source_in_us: secs(2.0),
                    speed: 2.0,
                    volume: 1.0,
                    muted: false,
                    mirror: false,
                    tie: None,
                },
            ),
        ],
    )];
    let c = ctx(p);
    let img = frame_plan(&c, secs(2.0), Landscape);
    let Some(Layer::Image(logo)) = img.layers.iter().find(|l| matches!(l, Layer::Image(_))) else {
        panic!("no image")
    };
    // A 512×512 logo in a 768×216 px box: 216×216 centred.
    close(logo.dest, bx(0.24375, 0.1, 0.1125, 0.2));
    let vid = frame_plan(&c, secs(6.0), Landscape);
    let overlay = videos(&vid)
        .into_iter()
        .find(|v| v.role == VideoRole::Overlay)
        .unwrap();
    close(overlay.dest, bx(0.5, 0.5, 0.25, 0.25));
    assert_eq!(overlay.source.time_us, secs(4.0), "2 s in + 1 s × 2");
}

/// A picture whose box is the whole frame (a title card's background) covers it in every format, keeping its shape,
/// rather than sitting inside it with footage showing round it: Muse Image's 4:7 portrait in 9:16, and a 16:9 picture
/// drawn for a landscape video shown in its 9:16 version.
#[test]
fn a_full_frame_picture_covers_the_frame_in_every_format() {
    let mut p = project();
    let logo = p.asset("logo").unwrap().clone();
    for (id, (w, h)) in [("tall", (1152, 2016)), ("wide", (1920, 1080))] {
        let mut card = logo.clone();
        (card.id, card.width, card.height) = (id.into(), w, h);
        p.assets.push(card);
    }
    p.settings.formats = vec![Landscape, Portrait, Square];
    let background = |id: &str, start: f64| {
        let mut layer = item(
            id,
            start,
            3.0,
            FULL,
            ItemContent::Image {
                asset_id: id.into(),
            },
        );
        layer.layout = klipolo_model::layout::Layout::full_frame();
        layer
    };
    p.timeline.tracks = vec![overlay_track(
        "o",
        vec![background("tall", 1.0), background("wide", 5.0)],
    )];
    let c = ctx(p);
    let picture = |at: f64, format| {
        let plan = frame_plan(&c, secs(at), format);
        match plan.layers.iter().find(|l| matches!(l, Layer::Image(_))) {
            Some(Layer::Image(i)) => i.dest,
            _ => panic!("no picture"),
        }
    };
    // 4:7 in 1080 × 1920: as tall as the frame, 1097 px across, 9 px past each side.
    close(
        picture(2.0, Portrait),
        bx(
            -8.571428571428571 / 1080.0,
            0.0,
            1097.142857142857 / 1080.0,
            1.0,
        ),
    );
    // 16:9 in 1080 × 1920: as tall as the frame, 3413 px across, its middle third shown.
    close(
        picture(6.0, Portrait),
        bx(
            -1166.6666666666667 / 1080.0,
            0.0,
            3413.3333333333335 / 1080.0,
            1.0,
        ),
    );
    // And as it is in its own shape: the frame exactly.
    close(picture(6.0, Landscape), FULL);
    for (at, format) in [(2.0, Landscape), (2.0, Square), (6.0, Square)] {
        let d = picture(at, format);
        assert!(
            d.x <= 1e-9 && d.y <= 1e-9 && d.x + d.w >= 1.0 - 1e-9 && d.y + d.h >= 1.0 - 1e-9,
            "{format:?} at {at}: covers the frame: {d:?}"
        );
    }
    // A picture in a box of its own still sits inside it.
    let mut boxed = project();
    boxed.timeline.tracks = vec![overlay_track(
        "o",
        vec![item(
            "logo",
            1.0,
            3.0,
            bx(0.1, 0.1, 0.4, 0.2),
            ItemContent::Image {
                asset_id: "logo".into(),
            },
        )],
    )];
    let plan = frame_plan(&ctx(boxed), secs(2.0), Landscape);
    let Some(Layer::Image(logo)) = plan.layers.iter().find(|l| matches!(l, Layer::Image(_))) else {
        panic!("no image")
    };
    close(logo.dest, bx(0.24375, 0.1, 0.1125, 0.2));
}

#[test]
fn track_order_is_z_order_and_hidden_tracks_are_skipped() {
    let mut p = project();
    p.timeline.tracks = vec![
        overlay_track(
            "below",
            vec![item(
                "logo",
                1.0,
                3.0,
                bx(0.1, 0.1, 0.2, 0.2),
                ItemContent::Image {
                    asset_id: "logo".into(),
                },
            )],
        ),
        overlay_track(
            "above",
            vec![item("title", 1.0, 3.0, bx(0.1, 0.1, 0.4, 0.1), text("Hi"))],
        ),
    ];
    let kinds = |p: &klipolo_model::project::Project| -> Vec<&'static str> {
        frame_plan(&ctx(p.clone()), secs(2.0), Landscape)
            .layers
            .iter()
            .map(|l| match l {
                Layer::Video(_) => "video",
                Layer::Image(_) => "image",
                Layer::Text(_) => "text",
                Layer::Generator(_) => "generator",
                Layer::Scene(_) => "scene",
                Layer::Adjustment(_) => "adjustment",
                Layer::Black { .. } => "black",
                Layer::Transition(_) => "transition",
                Layer::Caption(_) => "caption",
                Layer::Cursor(_) => "cursor",
            })
            .collect()
    };
    assert_eq!(kinds(&p), ["video", "video", "image", "text"]);
    p.timeline.tracks[0].hidden = true;
    assert_eq!(kinds(&p), ["video", "video", "text"]);
}

#[test]
fn clip_fades_go_to_black_under_the_overlays() {
    let mut p = project();
    p.timeline.main[0].fade_in_us = secs(1.0);
    p.timeline.main[0].fade_out_us = secs(2.0);
    p.timeline.tracks = vec![overlay_track(
        "t",
        vec![item("title", 0.0, 20.0, bx(0.1, 0.1, 0.4, 0.1), text("Hi"))],
    )];
    let c = ctx(p);
    let start = frame_plan(&c, secs(0.25), Landscape);
    assert!(
        matches!(start.layers[2], Layer::Black { alpha } if (alpha - 0.75).abs() < 1e-9),
        "{:?}",
        start.layers[2]
    );
    assert!(matches!(start.layers[3], Layer::Text(_)));
    let end = frame_plan(&c, secs(19.0), Landscape);
    assert!(matches!(end.layers[2], Layer::Black { alpha } if (alpha - 0.5).abs() < 1e-9));
    assert!(
        frame_plan(&c, secs(10.0), Landscape)
            .layers
            .iter()
            .all(|l| !matches!(l, Layer::Black { .. }))
    );
}

#[test]
fn zoom_eases_to_its_target_in_source_space() {
    let mut p = project();
    let target = ZoomTarget::Rect {
        rect: bx(0.5, 0.5, 0.5, 0.5),
    };
    p.timeline.tracks = vec![zoom_track(vec![item(
        "z",
        1.0,
        3.0,
        bx(0.0, 0.0, 1.0, 1.0),
        ItemContent::Zoom { target, scale: 2.0 },
    )])];
    let c = ctx(p);
    let main = |s: f64, f| videos(&frame_plan(&c, secs(s), f))[0].crop;
    close(main(0.5, Landscape), FULL);
    let zoomed = main(2.5, Landscape);
    for (got, want) in [
        (zoomed.x, 0.5),
        (zoomed.y, 0.5),
        (zoomed.w, 0.5),
        (zoomed.h, 0.5),
    ] {
        assert!((got - want).abs() < 0.02, "{zoomed:?}");
    }
    let split = main(2.5, Portrait); // the same part of the screen in every format
    assert!((split.x - zoomed.x).abs() < 1e-9 && (split.w - zoomed.w).abs() < 1e-9);
    close(main(6.0, Landscape), FULL);
}

#[test]
fn footage_appended_after_a_late_zoom_is_not_zoomed() {
    let mut p = project();
    // A zoom that ends just before the end of the 30 s timeline: the spring is still easing out at the end.
    p.timeline.tracks = vec![zoom_track(vec![item(
        "z",
        28.0,
        1.9,
        bx(0.0, 0.0, 1.0, 1.0),
        ItemContent::Zoom {
            target: ZoomTarget::Auto { using: vec![] },
            scale: 2.0,
        },
    )])];
    let before = ctx(p.clone());
    // Append 10 s of footage; the zoom items are unchanged.
    let mut more = Clip::new(ClipSource::Asset { id: "broll".into() }, 0, secs(10.0));
    more.id = "c".into();
    p.timeline.main.push(more);
    let after = PlanContext::reusing(Arc::new(p), &before);
    let main = |s: f64| videos(&frame_plan(&after, secs(s), Landscape))[0].crop;
    assert!(main(29.0).w < 0.9, "still zoomed inside the zoom");
    close(main(33.0), FULL);
    close(main(39.0), FULL);
}

#[test]
fn looks_reach_main_and_camera() {
    let mut p = project();
    p.luts = vec![ColorLut {
        id: "lut1".into(),
        name: "Film".into(),
        path: "/luts/film.cube".into(),
    }];
    p.timeline.main[0].look.preset = Some(builtin::look("warm"));
    p.timeline.main[0].look.strength = 0.6;
    p.timeline.main[0].look.lut_id = Some("lut1".into());
    let plan = frame_plan(&ctx(p), secs(2.0), Landscape);
    let want = LookPlan {
        look: Some((
            "klipolo.builtin/look.warm".into(),
            Registry::builtin().look(&builtin::look("warm")).unwrap(),
        )),
        strength: 0.6,
        lut: Some(LutRef {
            id: "lut1".into(),
            path: "/luts/film.cube".into(),
            strength: 1.0,
        }),
    };
    assert_eq!(videos(&plan)[0].look.as_ref(), Some(&want));
    assert_eq!(videos(&plan)[1].look.as_ref(), Some(&want));
    assert_eq!(
        videos(&frame_plan(&ctx(project()), secs(2.0), Landscape))[0].look,
        None
    );
}

// ---- Transitions ----

use klipolo_engine::plan::{FramePlan, TransitionLayer, frame_index, frame_start_us};
use klipolo_model::ops::Op;
use klipolo_model::time::Us;
use klipolo_model::timeline::Transition;

/// The fixture with "b" playing broll 2–10 s and a 1 s transition from "a" (cut at 20 s): window 19.5–20.5 s.
fn with_transition(effect: &str) -> PlanContext {
    let mut p = project();
    p.timeline.main[1].source_in_us = secs(2.0);
    let p = klipolo_model::apply_ops(
        &p,
        &[Op::SetTransition {
            clip_id: "a".into(),
            transition: Some(Transition {
                effect: effect.into(),
                duration_us: secs(1.0),
                params: Default::default(),
            }),
        }],
    )
    .unwrap();
    ctx(p)
}

fn transition_of(plan: &FramePlan) -> &TransitionLayer {
    match &plan.layers[0] {
        Layer::Transition(t) => t,
        other => panic!("expected a transition, got {other:?}"),
    }
}

fn main_source(layers: &[Layer]) -> (&str, Us) {
    match &layers[0] {
        Layer::Video(v) => (v.source.asset_id.as_ref(), v.source.time_us),
        other => panic!("expected video, got {other:?}"),
    }
}

#[test]
fn a_dissolve_shows_the_next_clip_before_its_in_point() {
    let plan = frame_plan(
        &with_transition(builtin::TRANSITION_DISSOLVE),
        secs(19.75),
        Landscape,
    );
    let t = transition_of(&plan);
    assert_eq!(main_source(&t.outgoing), ("screen", secs(19.75)));
    // 0.25 s before the cut, "b" shows 0.25 s before its in point (2 s).
    assert_eq!(main_source(&t.incoming), ("broll", secs(1.75)));
    assert!((t.pass.progress - 0.25).abs() < 1e-6);
    assert_eq!(t.id.as_ref(), "a");
}

#[test]
fn after_the_cut_the_outgoing_clip_shows_its_spare_footage() {
    let plan = frame_plan(
        &with_transition(builtin::TRANSITION_DISSOLVE),
        secs(20.25),
        Landscape,
    );
    let t = transition_of(&plan);
    assert_eq!(main_source(&t.outgoing), ("screen", secs(20.25)));
    assert_eq!(main_source(&t.incoming), ("broll", secs(2.25)));
    assert!((t.pass.progress - 0.75).abs() < 1e-6);
    // Both sides' video reach the decoder pool, outgoing first.
    let ids: Vec<_> = plan
        .video_sources()
        .iter()
        .map(|(_, s)| s.asset_id.to_string())
        .collect();
    assert_eq!(ids, ["screen", "camera", "broll"]);
}

#[test]
fn outside_its_window_a_frame_is_unchanged() {
    let c = with_transition(builtin::TRANSITION_DISSOLVE);
    assert!(matches!(
        frame_plan(&c, secs(19.4), Landscape).layers[0],
        Layer::Video(_)
    ));
    assert!(matches!(
        frame_plan(&c, secs(20.6), Landscape).layers[0],
        Layer::Video(_)
    ));
}

#[test]
fn an_uninstalled_transition_is_a_cut_and_is_reported() {
    let c = with_transition("com.acme.fx/transition.spin");
    let before = frame_plan(&c, secs(19.75), Landscape);
    assert_eq!(main_source(&before.layers), ("screen", secs(19.75)));
    assert_eq!(
        before.missing.iter().map(|m| &*m.id).collect::<Vec<_>>(),
        ["com.acme.fx/transition.spin"]
    );
    assert_eq!(
        main_source(&frame_plan(&c, secs(20.25), Landscape).layers),
        ("broll", secs(2.25))
    );
}

/// Handles are rounded to timeline µs, so at speeds other than 1× a side can ask for a few µs
/// outside its media. Here both halves use all the spare footage there is, and unclamped the window's last µs would
/// ask broll for 10 000 000 µs (its length) and its first µs screen for −1 µs.
#[test]
fn a_sped_up_transition_never_asks_for_time_outside_the_media() {
    let mut p = project();
    // "a": broll 0.5–9 s at 1.5× (1 s spare after its out point); "b": screen 1–30 s at 1.5× (1 s spare before).
    let clip = |id: &str, asset: &str, from: f64, to: f64| Clip {
        id: id.into(),
        speed: 1.5,
        ..Clip::new(ClipSource::Asset { id: asset.into() }, secs(from), secs(to))
    };
    p.timeline.main = vec![clip("a", "broll", 0.5, 9.0), clip("b", "screen", 1.0, 30.0)];
    let max = klipolo_model::transitions::max_transition_us(&p, 0);
    let p = klipolo_model::apply_ops(
        &p,
        &[Op::SetTransition {
            clip_id: "a".into(),
            transition: Some(Transition {
                effect: builtin::TRANSITION_DISSOLVE.into(),
                duration_us: max,
                params: Default::default(),
            }),
        }],
    )
    .unwrap();
    let c = ctx(p);
    let w = c.transitions[0];
    assert_eq!(
        (w.cut_us - w.start_us, w.end_us - w.cut_us),
        (666_667, 666_667)
    );
    let duration = |id: &str| c.project.asset(id).unwrap().duration_us;
    let fps = c.project.settings.fps;
    let frames = (frame_index(w.start_us, fps)..=frame_index(w.end_us - 1, fps))
        .map(|n| frame_start_us(n, fps).max(w.start_us))
        .chain([w.start_us, w.end_us - 1]);
    for at in frames {
        let plan = frame_plan(&c, at, Landscape);
        let t = transition_of(&plan);
        let (out, inc) = (main_source(&t.outgoing), main_source(&t.incoming));
        assert!(
            (0..duration("broll")).contains(&out.1),
            "{at}: outgoing at {}",
            out.1
        );
        assert!(
            (0..duration("screen")).contains(&inc.1),
            "{at}: incoming at {}",
            inc.1
        );
    }
}

/// While an overlay track is soloed only soloed overlay tracks draw, over the main track, which always
/// shows; hide still wins; a solo on an audio track hides no pictures; un-soloing gives back exactly the plan from
/// before (solo changes no other setting).
#[test]
fn solo_shows_only_the_soloed_overlay_tracks() {
    use klipolo_model::project::Project;
    use klipolo_model::timeline::{Track, TrackKind};

    let mut p = project();
    let title = |id: &str, y: f64| item(id, 0.0, 5.0, bx(0.1, y, 0.3, 0.2), text(id));
    let mut music = Track::new(TrackKind::Audio, "Music");
    music.id = "music".into();
    p.timeline.tracks = vec![
        overlay_track("o1", vec![title("t1", 0.1)]),
        overlay_track("o2", vec![title("t2", 0.4)]),
        overlay_track("o3", vec![title("t3", 0.7)]),
        music,
    ];
    let plan = |p: &Project| frame_plan(&ctx(p.clone()), secs(2.0), Landscape);
    let titles = |p: &Project| {
        texts(&plan(p))
            .iter()
            .map(|t| t.id.to_string())
            .collect::<Vec<_>>()
    };
    let before = plan(&p);
    assert_eq!(titles(&p), ["t1", "t2", "t3"]);

    let mut soloed = p.clone();
    soloed.timeline.tracks[1].solo = true;
    assert_eq!(titles(&soloed), ["t2"]);
    assert_eq!(
        videos(&plan(&soloed)).len(),
        videos(&before).len(),
        "the main track's screen and camera always show"
    );
    let mut hidden = soloed.clone();
    hidden.timeline.tracks[2].solo = true;
    hidden.timeline.tracks[2].hidden = true;
    assert_eq!(titles(&hidden), ["t2"], "hide wins over solo");

    let mut sound_only = p.clone();
    sound_only.timeline.tracks[3].solo = true;
    assert_eq!(plan(&sound_only), before, "an audio solo picks sound only");

    soloed.timeline.tracks[1].solo = false;
    assert_eq!(soloed, p);
    assert_eq!(plan(&soloed), before, "un-solo restores the plan exactly");
}

#[test]
fn an_editing_copy_replaces_the_source_in_preview_only() {
    let map = Arc::new(MediaMap::from([(
        "/media/broll".into(),
        "/cache/broll-copy.mp4".into(),
    )]));
    let preview = ctx(project()).with_media(map);
    let v = videos(&frame_plan(&preview, secs(25.0), Landscape))[0]
        .source
        .clone();
    assert_eq!(
        (v.path.as_ref(), v.original.as_ref()),
        ("/cache/broll-copy.mp4", "/media/broll")
    );
    // Export builds its context without one.
    let export = frame_plan(&ctx(project()), secs(25.0), Landscape);
    assert_eq!(videos(&export)[0].source.path.as_ref(), "/media/broll");
}

/// Copies are stored upright and numbered on their own grid, so a swapped-in copy has no turn and the
/// copy's rate (a portrait phone clip would turn twice otherwise); times and framing stay the source's. A revision
/// keeps the map.
#[test]
fn a_swapped_in_copy_is_upright_at_its_own_rate_with_the_same_times() {
    let mut p = project();
    let broll = p.assets.iter_mut().find(|a| a.id == "broll").unwrap();
    (broll.rotation, broll.fps) = (Some(90), 29.97);
    let map = Arc::new(MediaMap::from([(
        "/media/broll".into(),
        MediaCopy::new("/cache/broll-copy.mp4", 30.0),
    )]));
    let original = frame_plan(&ctx(p.clone()), secs(25.0), Landscape);
    let before = ctx(p.clone()).with_media(map);
    let edited = PlanContext::reusing(Arc::new(p), &before);
    let copy = frame_plan(&edited, secs(25.0), Landscape);
    let (o, c) = (&videos(&original)[0], &videos(&copy)[0]);
    assert_eq!((o.source.rotation, o.source.fps), (Some(90), 29.97));
    assert_eq!((c.source.rotation, c.source.fps), (Some(0), 30.0));
    assert_eq!(c.source.time_us, o.source.time_us);
    assert_eq!((c.dest, c.crop), (o.dest, o.crop));
    assert!(c.source.is_copy() && !o.source.is_copy());
}

/// A copy of a 23.976 source is made at its exact rate (copy frame n drawn from the source at frame
/// n's start, as the worker does), so at every timeline frame of a 24, 25, 30 or 60 fps project, at 1× and 1.5×, the
/// preview reading the copy shows the source frame export shows reading the original. A copy rounded to 24 fps showed
/// the frame before at most stills.
#[test]
fn a_copy_shows_the_frame_the_original_shows_at_every_timeline_frame() {
    use klipolo_engine::export::shared::FrameRate;
    use klipolo_engine::plan::frame_start_us;
    use klipolo_engine::render::pool::frame_shown;
    let rate = FrameRate {
        num: 24_000,
        den: 1001,
    };
    let fps = rate.f64();
    // The source frame copy frame n holds: the worker draws it from the source at n's start.
    let held = |n: i64| frame_shown(rate.frame_start_us(n as u64), fps);
    let map = Arc::new(MediaMap::from([(
        "/media/broll".into(),
        MediaCopy::new("/cache/broll-copy.mp4", fps),
    )]));
    for project_fps in [24, 25, 30, 60] {
        for speed in [1.0, 1.5] {
            let mut p = project();
            p.settings.fps = project_fps;
            p.assets.iter_mut().find(|a| a.id == "broll").unwrap().fps = fps;
            let b = p.timeline.main.iter_mut().find(|c| c.id == "b").unwrap();
            b.speed = speed;
            let original = ctx(p.clone());
            let preview = ctx(p).with_media(map.clone());
            let start = original.starts[1];
            for f in 0..(5 * i64::from(project_fps)) {
                let at = start + frame_start_us(f, project_fps);
                let o = videos(&frame_plan(&original, at, Landscape))[0]
                    .source
                    .clone();
                let c = videos(&frame_plan(&preview, at, Landscape))[0]
                    .source
                    .clone();
                assert_eq!(c.path.as_ref(), "/cache/broll-copy.mp4");
                let export = frame_shown(o.time_us, o.fps);
                let shown = held(frame_shown(c.time_us, c.fps));
                assert_eq!(
                    shown, export,
                    "{project_fps} fps at {speed}×, timeline frame {f}: the copy shows source frame {shown}, export {export}"
                );
            }
        }
    }
}
