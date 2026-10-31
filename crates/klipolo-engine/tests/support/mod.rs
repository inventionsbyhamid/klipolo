#![allow(dead_code)]
use std::path::PathBuf;

#[cfg(all(feature = "trial", target_os = "macos"))]
pub mod export_film;
pub mod fixture_set;
pub mod make;
pub mod recording;
pub mod scene;
#[cfg(feature = "trial")]
pub mod stalls;
pub mod takes;
// Helpers live in their own files (`make.rs`, `scene.rs`), and every test's
// `use support::*` sees them. A test that uses none of a lane's helpers mustn't warn.
#[allow(unused_imports)]
pub use make::*;
#[allow(unused_imports)]
pub use scene::*;

/// Luma planes of the decoded frames `wanted` (by index), in order.
pub fn luma(path: &std::path::Path, wanted: &[usize]) -> Vec<Vec<u8>> {
    let mut input = ffmpeg::format::input(path).unwrap();
    let stream = input.streams().best(ffmpeg::media::Type::Video).unwrap();
    let index = stream.index();
    let mut decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
        .unwrap()
        .decoder()
        .video()
        .unwrap();
    let (mut frame, mut n, mut out) = (ffmpeg::frame::Video::empty(), 0usize, Vec::new());
    let mut take = |decoder: &mut ffmpeg::decoder::Video, out: &mut Vec<Vec<u8>>| {
        while decoder.receive_frame(&mut frame).is_ok() {
            if wanted.contains(&n) {
                let (w, h) = (frame.width() as usize, frame.height() as usize);
                let stride = frame.stride(0);
                let data = frame.data(0);
                out.push(
                    (0..h)
                        .flat_map(|y| data[y * stride..y * stride + w].to_vec())
                        .collect(),
                );
            }
            n += 1;
        }
    };
    for (s, packet) in input.packets() {
        if s.index() == index {
            decoder.send_packet(&packet).unwrap();
            take(&mut decoder, &mut out);
        }
    }
    decoder.send_eof().unwrap();
    take(&mut decoder, &mut out);
    out
}

/// A generated media fixture (scripts/gen-engine-fixtures.sh writes them; they are never committed).
pub fn fixture(name: &str) -> PathBuf {
    let path = fixture_set::fixture_dir().join(name);
    assert!(
        path.exists(),
        "{} is missing: run scripts/gen-engine-fixtures.sh",
        path.display()
    );
    path
}

use std::sync::Arc;

use klipolo_engine::plan::{FramePlan, Layer, PlanContext, VideoLayer};
use klipolo_model::contrib::builtin;
use klipolo_model::layout::{Anchor, BoxN, Layout};
use klipolo_model::project::*;
use klipolo_model::time::{Us, secs_to_us};
use klipolo_model::timeline::*;

pub fn secs(s: f64) -> Us {
    secs_to_us(s)
}

fn asset(id: &str, kind: AssetKind, duration: f64, w: u32, h: u32, has_audio: bool) -> Asset {
    Asset {
        id: id.into(),
        kind,
        path: format!("/media/{id}"),
        name: id.into(),
        duration_us: secs(duration),
        width: w,
        height: h,
        fps: if kind == AssetKind::Video { 30.0 } else { 0.0 },
        has_audio,
        audio_lead_us: None,
        made_with: None,
        rotation: None,
    }
}

/// A recording source of one run, whose file has the source's id, `offset_us` into the take for `seconds`.
pub fn rec_source(id: &str, role: SourceRole, offset_us: Us, seconds: f64) -> RecordingSource {
    RecordingSource {
        id: id.into(),
        role,
        label: format!("capture.source.{id}"),
        runs: vec![SourceRun {
            asset_id: id.into(),
            offset_us,
            duration_us: secs(seconds),
            drift_ppm: 0.0,
            file_in_us: 0,
        }],
        nudge_us: 0,
        mix: SourceMix::default(),
        synced: None,
    }
}

/// Every file of the fixture and every run of its recording `seconds` long (runs keep their offsets).
pub fn lengthen(p: &mut Project, seconds: f64) {
    for asset in &mut p.assets {
        asset.duration_us = secs(seconds);
    }
    for run in p
        .recordings
        .iter_mut()
        .flat_map(|r| &mut r.sources)
        .flat_map(|s| &mut s.runs)
    {
        run.duration_us = secs(seconds);
    }
}

/// Clip "a" = recording "rec" 0–20 s (screen 1920×1080, camera 1280×720 starting 0.2 s later, mic), default camera
/// layout; clip "b" = asset "broll" (1920×1080) 0–10 s. 30 s at 30 fps, landscape primary, black background.
pub fn project() -> Project {
    let mut p = Project::new("Engine fixture");
    p.id = "project".into();
    p.assets = vec![
        asset("screen", AssetKind::Video, 60.0, 1920, 1080, true),
        asset("camera", AssetKind::Video, 60.0, 1280, 720, false),
        asset("mic", AssetKind::Audio, 60.0, 0, 0, true),
        asset("broll", AssetKind::Video, 10.0, 1920, 1080, true),
        asset("logo", AssetKind::Image, 0.0, 512, 512, false),
        asset("music", AssetKind::Audio, 120.0, 0, 0, true),
    ];
    p.recordings = vec![Recording {
        id: "rec".into(),
        kind: RecordingKind::Screen,
        sources: vec![
            // Its file has sound, so it plays muted, as capture import leaves it.
            RecordingSource {
                mix: SourceMix {
                    muted: true,
                    ..SourceMix::default()
                },
                ..rec_source("screen", SourceRole::Screen, 0, 60.0)
            },
            rec_source("camera", SourceRole::Camera, secs(0.2), 59.8),
            rec_source("mic", SourceRole::Mic, 0, 60.0),
        ],
        primary: "screen".into(),
        cursor: None,
        markers: Vec::new(),
        game: None,
    }];
    let mut a = Clip::new(ClipSource::Recording { id: "rec".into() }, 0, secs(20.0));
    a.id = "a".into();
    a.camera = Some(CameraLayout::default());
    let mut b = Clip::new(ClipSource::Asset { id: "broll".into() }, 0, secs(10.0));
    b.id = "b".into();
    p.timeline.main = vec![a, b];
    p
}

pub fn ctx(p: Project) -> PlanContext {
    PlanContext::new(Arc::new(p))
}

pub fn videos(plan: &FramePlan) -> Vec<&VideoLayer> {
    plan.layers
        .iter()
        .filter_map(|l| match l {
            Layer::Video(v) => Some(v),
            _ => None,
        })
        .collect()
}

pub fn bx(x: f64, y: f64, w: f64, h: f64) -> BoxN {
    BoxN { x, y, w, h }
}

pub fn close(a: BoxN, b: BoxN) {
    for (x, y) in [(a.x, b.x), (a.y, b.y), (a.w, b.w), (a.h, b.h)] {
        assert!((x - y).abs() < 1e-6, "{a:?} != {b:?}");
    }
}

pub fn overlay_track(id: &str, items: Vec<Item>) -> Track {
    let mut t = Track::new(TrackKind::Overlay, id);
    t.id = id.into();
    t.items = items;
    t
}

pub fn zoom_track(items: Vec<Item>) -> Track {
    let mut t = Track::new(TrackKind::Zoom, "Zoom");
    t.id = "zoom".into();
    t.items = items;
    t
}

pub fn item(id: &str, start: f64, duration: f64, rect: BoxN, content: ItemContent) -> Item {
    Item {
        id: id.into(),
        start_us: secs(start),
        duration_us: secs(duration),
        attach: None,
        layout: Layout::new(Anchor::TopLeft, rect),
        content,
        effects: Vec::new(),
    }
}

/// A text layer's content, 72 px.
pub fn text(text: &str) -> ItemContent {
    ItemContent::Text {
        text: text.into(),
        style: TextStyle {
            size: 72.0,
            ..TextStyle::default()
        },
    }
}

/// A built-in motion preset by its word (`"pop"`), 0.3 s long, with its default settings.
pub fn preset(word: &str) -> klipolo_model::motion::MotionUse {
    klipolo_model::motion::MotionUse {
        preset: builtin::motion(word),
        params: Default::default(),
        duration_us: secs(0.3),
        ease: None,
    }
}

/// `item` coming in and going out with the named presets (0.3 s each), or without.
pub fn animated(mut item: Item, enter: Option<&str>, exit: Option<&str>) -> Item {
    let motion = klipolo_model::motion::Motion {
        enter: enter.map(preset),
        exit: exit.map(preset),
        ..Default::default()
    };
    item.layout.motion = (!motion.is_empty()).then_some(motion);
    item
}

/// A frame showing only `source` as the main footage.
pub fn plan_with(source: &klipolo_engine::plan::SourceRef) -> FramePlan {
    plan_of(std::slice::from_ref(source))
}

/// A frame showing each of `sources` as a full-frame layer, in order.
pub fn plan_of(sources: &[klipolo_engine::plan::SourceRef]) -> FramePlan {
    FramePlan {
        format: klipolo_model::project::FormatKind::Landscape,
        canvas: (1920, 1080),
        at_us: 0,
        background: klipolo_engine::plan::BackgroundPlan::Solid { color: [0; 3] },
        layers: sources
            .iter()
            .map(|source| {
                Layer::Video(VideoLayer {
                    role: klipolo_engine::plan::VideoRole::Main,
                    id: source.asset_id.clone(),
                    source: source.clone(),
                    dest: klipolo_engine::plan::framing::FULL,
                    crop: klipolo_engine::plan::framing::FULL,
                    rotation: 0.0,
                    opacity: 1.0,
                    radius: 0.0,
                    mirror: false,
                    border: false,
                    look: None,
                    effects: Vec::new(),
                    pad_px: 0.0,
                    blend: klipolo_model::motion::Blend::Normal,
                    reach: 1.0,
                })
            })
            .collect(),
        missing: Vec::new(),
    }
}
