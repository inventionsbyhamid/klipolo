//! The synthetic 1-hour project: 1,500 clips cut from a screen + camera recording and 300 layers.

use std::path::Path;

use klipolo_model::contrib::builtin;
use klipolo_model::layout::{Anchor, BoxN, Layout};
use klipolo_model::motion::{Motion, MotionUse};
use klipolo_model::project::*;
use klipolo_model::time::{Us, secs_to_us};
use klipolo_model::timeline::*;

// ---------------------------------------------------------------------------------------------------------------
// The synthetic 1-hour project.

pub fn secs(s: f64) -> Us {
    secs_to_us(s)
}

pub fn fixture(name: &str) -> String {
    let path = crate::fixture_set::fixture_dir().join(name);
    assert!(
        path.exists(),
        "{} is missing: run scripts/gen-engine-fixtures.sh",
        path.display()
    );
    path.canonicalize().unwrap().display().to_string()
}

fn asset(id: &str, kind: AssetKind, path: String, seconds: f64, w: u32, h: u32) -> Asset {
    Asset {
        id: id.into(),
        kind,
        path,
        name: id.into(),
        duration_us: secs(seconds),
        width: w,
        height: h,
        fps: if kind == AssetKind::Video { 30.0 } else { 0.0 },
        has_audio: false,
        audio_lead_us: None,
        made_with: None,
        rotation: None,
    }
}

/// A built-in motion preset by its word, 0.3 s long, with its default settings.
pub fn preset(word: &str) -> MotionUse {
    MotionUse {
        preset: builtin::motion(word),
        params: Default::default(),
        duration_us: secs(0.3),
        ease: None,
    }
}

fn item(id: String, start: f64, duration: f64, rect: BoxN, content: ItemContent) -> Item {
    Item {
        id,
        start_us: secs(start),
        duration_us: secs(duration),
        attach: None,
        layout: Layout::new(Anchor::TopLeft, rect),
        content,
        effects: Vec::new(),
    }
}

const CLIPS: usize = 1_500;
const CLIP_S: f64 = 2.4;

/// Whether the frame at `at_us` is the first frame of a main clip (30 fps).
pub fn is_cut(at_us: Us) -> bool {
    at_us % secs(CLIP_S) < 33_334
}

/// 1,500 clips cut from a screen + camera recording (1 h), 100 titles, 100 images (10 files), 50 b-roll overlays
/// shrunk to a quarter, 50 zooms; a warm look on every 5th clip and a LUT on every 10th.
pub fn one_hour_project(scratch: &Path) -> Project {
    let mut p = Project::new("Perf");
    p.id = "perf".into();
    let mut assets = vec![
        asset(
            "screen",
            AssetKind::Video,
            fixture("bars-30fps-0.5gop.mp4"),
            4.0,
            1920,
            1080,
        ),
        asset(
            "camera",
            AssetKind::Video,
            fixture("camera-720p.mp4"),
            4.0,
            1280,
            720,
        ),
        asset(
            "broll",
            AssetKind::Video,
            fixture("camera-720p.mp4"),
            4.0,
            1280,
            720,
        ),
    ];
    for k in 0..10 {
        let path = scratch.join(format!("logo-{k}.png"));
        let img = image::RgbaImage::from_fn(1600, 1200, |x, y| {
            image::Rgba([(x / 7) as u8, (y / 5) as u8, (k * 25) as u8, 200])
        });
        img.save(&path).unwrap();
        assets.push(asset(
            &format!("logo-{k}"),
            AssetKind::Image,
            path.display().to_string(),
            0.0,
            1600,
            1200,
        ));
    }
    p.assets = assets;
    let cube = scratch.join("look.cube");
    let n = 33;
    let mut text = format!("LUT_3D_SIZE {n}\n");
    for b in 0..n {
        for g in 0..n {
            for r in 0..n {
                let f = |c: usize| c as f32 / (n - 1) as f32;
                text.push_str(&format!("{} {} {}\n", f(r), f(g) * 0.9, f(b) * 0.8));
            }
        }
    }
    std::fs::write(&cube, text).unwrap();
    p.luts = vec![ColorLut {
        id: "lut".into(),
        name: "Look".into(),
        path: cube.display().to_string(),
    }];
    // Each source one 4 s run over its file.
    let source = |id: &str, role| RecordingSource {
        id: id.into(),
        role,
        label: format!("capture.source.{id}"),
        runs: vec![SourceRun {
            asset_id: id.into(),
            offset_us: 0,
            duration_us: secs(4.0),
            drift_ppm: 0.0,
            file_in_us: 0,
        }],
        nudge_us: 0,
        mix: SourceMix::default(),
        synced: None,
    };
    p.recordings = vec![Recording {
        id: "rec".into(),
        kind: RecordingKind::Screen,
        sources: vec![
            source("screen", SourceRole::Screen),
            source("camera", SourceRole::Camera),
        ],
        primary: "screen".into(),
        cursor: None,
        markers: Vec::new(),
        game: None,
    }];
    let ins = [0.0, 1.0, 1.5];
    p.timeline.main = (0..CLIPS)
        .map(|k| {
            let at = ins[k % ins.len()];
            let mut c = Clip::new(
                ClipSource::Recording { id: "rec".into() },
                secs(at),
                secs(at + CLIP_S),
            );
            c.id = format!("clip-{k}");
            c.camera = Some(CameraLayout::default());
            if k % 5 == 0 {
                c.look.preset = Some(builtin::look("warm"));
                c.look.strength = 0.7;
            }
            if k % 10 == 0 {
                c.look.lut_id = Some("lut".into());
                c.look.lut_strength = 0.8;
            }
            c
        })
        .collect();
    let mut titles = Track::new(TrackKind::Overlay, "Titles");
    let mut images = Track::new(TrackKind::Overlay, "Images");
    let mut videos = Track::new(TrackKind::Overlay, "B-roll");
    let mut zooms = Track::new(TrackKind::Zoom, "Zoom");
    for j in 0..100 {
        let t = 36.0 * j as f64;
        let mut title = item(
            format!("title-{j}"),
            t,
            3.0,
            BoxN {
                x: 0.1,
                y: 0.05,
                w: 0.8,
                h: 0.2,
            },
            ItemContent::Text {
                text: format!("Chapter {j}: what we build next"),
                style: TextStyle {
                    size: 40.0 + (j % 8) as f64 * 8.0,
                    ..TextStyle::default()
                },
            },
        );
        // Fades in and slides out, 0.3 s each.
        title.layout.motion = Some(Motion {
            enter: Some(preset("fade")),
            exit: Some(preset("slide")),
            ..Motion::default()
        });
        titles.items.push(title);
        images.items.push(item(
            format!("image-{j}"),
            t + 6.0,
            3.0,
            BoxN {
                x: 0.7,
                y: 0.65,
                w: 0.25,
                h: 0.3,
            },
            ItemContent::Image {
                asset_id: format!("logo-{}", j % 10),
            },
        ));
    }
    for j in 0..50 {
        let t = 72.0 * j as f64;
        videos.items.push(item(
            format!("broll-{j}"),
            t + 12.0,
            4.0,
            BoxN {
                x: 0.05,
                y: 0.6,
                w: 0.25,
                h: 0.25,
            },
            ItemContent::Video {
                asset_id: "broll".into(),
                source_in_us: 0,
                speed: 1.0,
                volume: 1.0,
                muted: true,
                mirror: false,
                fill: false,
                crop: None,
                tie: None,
            },
        ));
        zooms.items.push(item(
            format!("zoom-{j}"),
            t + 20.0,
            3.0,
            BoxN {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 1.0,
            },
            ItemContent::Zoom {
                target: ZoomTarget::Auto { using: vec![] },
                scale: 1.5,
            },
        ));
    }
    p.timeline.tracks = vec![titles, images, videos, zooms];
    p
}
