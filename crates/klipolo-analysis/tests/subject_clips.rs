//! A main clip's footage for tracking (an imported video, or a camera take's run), the formats whose Fill crops it,
//! its path from a tracked stretch (in its own source time, trimmed to what it plays), and the op that makes it follow.

use std::collections::BTreeMap;

use klipolo_analysis::providers::{TrackedBox, TrackedFrame};
use klipolo_analysis::subject::clips::{clip_path, cropped_formats, follow_op, footage};
use klipolo_analysis::subject::{FPS, PERSON, SubjectTrack};
use klipolo_model::fill_focus::{FillFocus, FocusKey, FocusPath};
use klipolo_model::ops::{ClipPatch, Op};
use klipolo_model::project::FormatKind::{Landscape, Portrait, Square};
use klipolo_model::project::{Asset, AssetKind, Project};
use klipolo_model::recording::{
    Recording, RecordingKind, RecordingSource, SourceMix, SourceRole, SourceRun,
};
use klipolo_model::time::{RangeUs, Us};
use klipolo_model::timeline::{Clip, ClipSource, Framing, FramingMode};

fn s(seconds: f64) -> Us {
    (seconds * 1e6) as Us
}

fn video(id: &str, w: u32, h: u32) -> Asset {
    Asset {
        id: id.into(),
        kind: AssetKind::Video,
        path: format!("/media/{id}.mp4"),
        name: format!("{id}.mp4"),
        duration_us: s(600.0),
        width: w,
        height: h,
        fps: 30.0,
        rotation: None,
        has_audio: true,
        audio_lead_us: None,
        made_with: None,
    }
}

fn source(id: &str, role: SourceRole, offset_us: Us, file_in_us: Us) -> RecordingSource {
    RecordingSource {
        id: id.into(),
        role,
        label: format!("capture.source.{id}"),
        runs: vec![SourceRun {
            asset_id: id.into(),
            offset_us,
            duration_us: s(300.0),
            drift_ppm: 0.0,
            file_in_us,
        }],
        nudge_us: 0,
        mix: SourceMix::default(),
        synced: None,
    }
}

/// A talk (an imported 16:9 video) cut to 30 to 40 s, a camera take whose camera starts 2 s in, and a screen take.
fn project() -> Project {
    let mut p = Project::new("Talk");
    p.assets = vec![
        video("talk", 1920, 1080),
        video("cam", 1280, 720),
        video("screen", 1920, 1080),
    ];
    p.settings.formats = vec![Landscape, Portrait, Square];
    p.recordings = vec![
        Recording {
            id: "camtake".into(),
            kind: RecordingKind::Camera,
            sources: vec![source("cam", SourceRole::Camera, s(2.0), s(1.0))],
            primary: "cam".into(),
            cursor: None,
            markers: vec![],
            game: None,
        },
        Recording {
            id: "screentake".into(),
            kind: RecordingKind::Screen,
            sources: vec![source("screen", SourceRole::Screen, 0, 0)],
            primary: "screen".into(),
            cursor: None,
            markers: vec![],
            game: None,
        },
    ];
    let clip = |id: &str, source: ClipSource, from: f64, to: f64| {
        let mut c = Clip::new(source, s(from), s(to));
        c.id = id.into();
        c
    };
    p.timeline.main = vec![
        clip("talk1", ClipSource::Asset { id: "talk".into() }, 30.0, 40.0),
        clip(
            "cam1",
            ClipSource::Recording {
                id: "camtake".into(),
            },
            12.0,
            20.0,
        ),
        clip(
            "screen1",
            ClipSource::Recording {
                id: "screentake".into(),
            },
            0.0,
            5.0,
        ),
    ];
    p
}

/// One person at `x` on every frame of a stretch.
fn track(range: RangeUs, x: f64) -> SubjectTrack {
    let frames = ((range.end - range.start) as f64 / 1e6 * FPS) as u32;
    SubjectTrack {
        model: "sam-3.1".into(),
        phrase: PERSON.into(),
        fps: FPS,
        range,
        frames: (0..frames)
            .map(|frame| TrackedFrame {
                frame,
                boxes: vec![TrackedBox {
                    id: 1,
                    left: x - 0.1,
                    top: 0.1,
                    right: x + 0.1,
                    bottom: 1.0,
                    confidence: Some(0.9),
                }],
            })
            .collect(),
        cuts: vec![],
        frames_billed: u64::from(frames),
        cost_usd: 0.0,
    }
}

#[test]
fn an_imported_video_and_a_camera_take_have_footage_and_a_screen_take_does_not() {
    let p = project();
    let talk = footage(&p, &p.timeline.main[0]).unwrap();
    assert_eq!(
        (talk.asset_id.as_str(), talk.path.as_str()),
        ("talk", "/media/talk.mp4")
    );
    assert_eq!(
        (talk.range, talk.shift_us),
        (
            RangeUs {
                start: s(30.0),
                end: s(40.0)
            },
            0
        )
    );
    // The camera run starts 2 s into the take and 1 s into its file: take time 12 s is file time 11 s.
    let cam = footage(&p, &p.timeline.main[1]).unwrap();
    assert_eq!(cam.asset_id, "cam");
    assert_eq!(
        (cam.range, cam.shift_us),
        (
            RangeUs {
                start: s(11.0),
                end: s(19.0)
            },
            s(1.0)
        )
    );
    // A screen take follows the cursor instead.
    assert!(footage(&p, &p.timeline.main[2]).is_none());
}

#[test]
fn fill_crops_a_wide_video_only_in_vertical_and_square() {
    let p = project();
    let all = [Landscape, Portrait, Square];
    assert_eq!(
        cropped_formats(&p, &p.timeline.main[0], &all),
        [Portrait, Square]
    );
}

#[test]
fn a_clips_path_is_in_its_source_time_and_trimmed_to_what_it_plays() {
    let p = project();
    // A stretch of the talk wider than the clip: 20 to 50 s.
    let wide = track(
        RangeUs {
            start: s(20.0),
            end: s(50.0),
        },
        0.3,
    );
    let clip = &p.timeline.main[0];
    let path = clip_path(&p, clip, Portrait, &footage(&p, clip).unwrap(), &wide).unwrap();
    assert!(path.is_valid());
    let first = path.keys.first().unwrap();
    let last = path.keys.last().unwrap();
    assert!(
        first.at_us >= s(29.5) && first.at_us <= s(30.0),
        "{first:?}"
    );
    assert!(last.at_us >= s(39.75) && last.at_us <= s(40.5), "{last:?}");
    assert!(!first.cut);
    assert!((first.x - 0.3).abs() < 0.02);

    // The camera clip's keys are on the take's clock: the file's 11 s is the take's 12 s.
    let clip = &p.timeline.main[1];
    let cam = footage(&p, clip).unwrap();
    let path = clip_path(&p, clip, Portrait, &cam, &track(cam.range, 0.6)).unwrap();
    assert!(
        path.keys.first().unwrap().at_us >= s(11.5),
        "{:?}",
        path.keys[0]
    );
    assert!(path.keys.last().unwrap().at_us <= s(20.5));
    // Nobody tracked: no path.
    let empty = SubjectTrack {
        frames: vec![],
        ..track(cam.range, 0.5)
    };
    assert!(clip_path(&p, clip, Portrait, &cam, &empty).is_none());
}

#[test]
fn the_follow_op_fills_each_format_with_its_path_and_keeps_the_rest() {
    let mut p = project();
    let own = Framing {
        mode: FramingMode::Fit,
        focus_x: 0.2,
        focus_y: 0.5,
        follow: None,
    };
    p.timeline.main[0].framing.insert(Portrait, own);
    p.timeline.main[0]
        .fill_focus
        .insert(Landscape, FillFocus::Center);
    let path = FocusPath {
        keys: vec![FocusKey {
            at_us: s(30.0),
            x: 0.3,
            y: 0.5,
            cut: false,
        }],
    };
    let op = follow_op(
        &p,
        &p.timeline.main[0],
        &BTreeMap::from([(Portrait, path.clone())]),
    );
    let Op::UpdateClip { id, patch } = op else {
        panic!("{op:?}")
    };
    assert_eq!(id, "talk1");
    let ClipPatch {
        framing: Some(framing),
        fill_focus: Some(focus),
        ..
    } = patch
    else {
        panic!()
    };
    // Portrait fills now, its own focus point kept for Fixed; the other formats as they were.
    assert_eq!(
        framing[&Portrait],
        Framing {
            mode: FramingMode::Fill,
            ..own
        }
    );
    assert!(!framing.contains_key(&Square));
    assert_eq!(focus[&Portrait], FillFocus::Speaker { path });
    assert_eq!(focus[&Landscape], FillFocus::Center);
}
