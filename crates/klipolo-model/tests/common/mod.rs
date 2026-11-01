#![allow(dead_code)]
use std::collections::BTreeMap;
use std::path::PathBuf;

use klipolo_model::coded::CodedError;
use klipolo_model::contrib::builtin;
use klipolo_model::cursor_style::{ClickSource, CursorCapture, RecordingCursor};
use klipolo_model::layout::{Anchor, BoxN, Layout};
use klipolo_model::messages::MESSAGES;
use klipolo_model::motion::{Motion, MotionUse};
use klipolo_model::ops::{ItemPatch, Op, TrackTarget};
use klipolo_model::project::*;
use klipolo_model::time::{secs_to_us, Us};
use klipolo_model::timeline::*;
use klipolo_model::ModelError;

mod imported;
pub mod speech;
#[allow(unused_imports)]
pub use imported::{clip, imported, sync_shapes, title_on, title_start};

pub fn secs(s: f64) -> Us {
    secs_to_us(s)
}

/// The refusal whose English is `english` (a code without values), for tests written against the sentences.
pub fn refused(english: &str) -> ModelError {
    let code = MESSAGES
        .iter()
        .find(|(_, m)| *m == english)
        .map(|(c, _)| *c)
        .unwrap_or_else(|| panic!("no model code says {english:?}"));
    ModelError::Invalid(CodedError::new(code))
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

/// A single-run source whose file has the source's id.
pub fn rec_source(
    id: &str,
    role: SourceRole,
    label: &str,
    offset_us: Us,
    seconds: f64,
) -> RecordingSource {
    RecordingSource {
        id: id.into(),
        role,
        label: label.into(),
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

/// A copy of file `of` as `id`: a second run's own file (a reconnected device starts one).
pub fn second_file(p: &mut Project, of: &str, id: &str) {
    let mut file = p.asset(of).unwrap().clone();
    file.id = id.into();
    file.path = format!("/media/{id}");
    p.assets.push(file);
}

/// Main track: clip "a" = recording "rec" 0–20 s, clip "b" = asset "broll" 0–10 s (30 s total). No tracks.
pub fn fixture() -> Project {
    let mut p = Project::new("Fixture");
    p.id = "project".into();
    p.assets = vec![
        asset("screen", AssetKind::Video, 60.0, 1920, 1080, true),
        asset("camera", AssetKind::Video, 60.0, 1280, 720, false),
        asset("mic", AssetKind::Audio, 60.0, 0, 0, true),
        asset("music", AssetKind::Audio, 120.0, 0, 0, true),
        asset("logo", AssetKind::Image, 0.0, 512, 512, false),
        asset("broll", AssetKind::Video, 10.0, 1920, 1080, true),
    ];
    // The camera starts 0.2 s late and ends with the screen, so the take is 60 s long.
    p.recordings = vec![Recording {
        id: "rec".into(),
        kind: RecordingKind::Screen,
        sources: vec![
            rec_source(
                "screen",
                SourceRole::Screen,
                "capture.source.screen",
                0,
                60.0,
            ),
            rec_source(
                "camera",
                SourceRole::Camera,
                "capture.source.camera",
                200_000,
                59.8,
            ),
            rec_source("mic", SourceRole::Mic, "capture.source.mic", 0, 60.0),
        ],
        primary: "screen".into(),
        cursor: None,
        markers: vec![],
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

/// [`fixture`] with a cursor file on its screen recording, recorded separately with exact clicks.
pub fn fixture_with_cursor() -> Project {
    let mut p = fixture();
    p.recordings[0].cursor = Some(RecordingCursor {
        path: "/media/rec/cursor.jsonl".into(),
        capture: CursorCapture::Separate,
        clicks: ClickSource::Events,
    });
    p
}

pub fn overlay_track(id: &str, items: Vec<Item>) -> Track {
    let mut t = Track::new(TrackKind::Overlay, id);
    t.id = id.into();
    t.items = items;
    t
}

/// A built-in motion preset by its word, 0.3 s long, with no settings.
pub fn preset(word: &str) -> MotionUse {
    MotionUse {
        preset: builtin::motion(word),
        params: Default::default(),
        duration_us: secs(0.3),
        ease: None,
    }
}

/// Fades in and out over 0.3 s, as a new title does.
pub fn fade_in_out() -> Motion {
    Motion {
        enter: Some(preset("fade")),
        exit: Some(preset("fade")),
        ..Motion::default()
    }
}

/// A layer at rest (no motion) to turn into another kind: `Item { content, ..still_item(…) }`. A title's fade would
/// make an audio or zoom layer invalid (only pictures move).
pub fn still_item(id: &str, start: f64, duration: f64) -> Item {
    let mut item = text_item(id, start, duration);
    item.layout.motion = None;
    item
}

/// A title that fades in and out.
pub fn text_item(id: &str, start: f64, duration: f64) -> Item {
    let mut layout = Layout::new(
        Anchor::TopLeft,
        BoxN {
            x: 0.05,
            y: 0.05,
            w: 0.4,
            h: 0.1,
        },
    );
    layout.motion = Some(fade_in_out());
    Item {
        id: id.into(),
        start_us: secs(start),
        duration_us: secs(duration),
        attach: None,
        layout,
        content: ItemContent::Text {
            text: "Hello".into(),
            style: TextStyle::default(),
        },
        effects: Vec::new(),
    }
}

pub fn video_item(id: &str, start: f64, duration: f64, source_in: f64) -> Item {
    Item {
        id: id.into(),
        start_us: secs(start),
        duration_us: secs(duration),
        attach: None,
        layout: Layout::new(
            Anchor::Center,
            BoxN {
                x: 0.25,
                y: 0.25,
                w: 0.5,
                h: 0.5,
            },
        ),
        content: ItemContent::Video {
            asset_id: "broll".into(),
            source_in_us: secs(source_in),
            speed: 1.0,
            volume: 1.0,
            muted: false,
            mirror: false,
            fill: false,
            crop: None,
            tie: None,
        },
        effects: Vec::new(),
    }
}

/// The whole canvas, from the top left.
pub fn full() -> Layout {
    Layout::new(
        Anchor::TopLeft,
        BoxN {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        },
    )
}

/// A layer at rest showing `content`, `start` to `start + len` seconds, unattached.
pub fn item(id: &str, start: f64, len: f64, layout: Layout, content: ItemContent) -> Item {
    Item {
        id: id.into(),
        start_us: secs(start),
        duration_us: secs(len),
        attach: None,
        layout,
        content,
        effects: Vec::new(),
    }
}

/// The fixture with one overlay track, "ov", holding `items`.
pub fn project_with_overlay(items: Vec<Item>) -> Project {
    let mut p = fixture();
    p.timeline.tracks.push(overlay_track("ov", items));
    p
}

/// The first overlay track's layers.
pub fn overlay_items(p: &Project) -> &[Item] {
    p.timeline
        .tracks
        .iter()
        .find(|t| t.kind == TrackKind::Overlay)
        .map_or(&[], |t| &t.items)
}

pub fn item_by_id<'a>(p: &'a Project, id: &str) -> &'a Item {
    p.timeline
        .tracks
        .iter()
        .flat_map(|t| &t.items)
        .find(|i| i.id == id)
        .unwrap_or_else(|| panic!("no layer {id}"))
}

/// `op` applied as one batch; `p` is unchanged when it is refused.
pub fn try_apply(p: &mut Project, op: Op) -> Result<(), ModelError> {
    *p = klipolo_model::apply_ops(p, &[op])?;
    Ok(())
}

pub fn apply(p: &mut Project, op: Op) {
    if let Err(e) = try_apply(p, op) {
        panic!("refused: {e:?}");
    }
}

pub fn content_patch(content: ItemContent) -> ItemPatch {
    ItemPatch {
        content: Some(content),
        ..ItemPatch::default()
    }
}

/// Adds a 2 s full-frame generator layer of `source` (default settings) at the start, on the first overlay track
/// with room.
pub fn add_generator_op(id: &str, source: &str) -> Op {
    Op::AddItem {
        track: TrackTarget::Auto,
        item: item(
            id,
            0.0,
            2.0,
            full(),
            ItemContent::generator(source.into(), BTreeMap::new()),
        ),
        attach: false,
    }
}

/// `v` with every object's keys sorted, at every depth. A golden built from `serde_json::Value`s (`json!`,
/// `to_value`) goes through this first: their key order is sorted in a build of this crate alone but insertion order
/// once a crate that turns on serde_json's `preserve_order` (klipolo-ai) builds alongside, as CI's combined runs do.
pub fn sorted_keys(v: serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(map) => {
            let mut entries: Vec<(String, serde_json::Value)> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            serde_json::Value::Object(
                entries
                    .into_iter()
                    .map(|(k, v)| (k, sorted_keys(v)))
                    .collect(),
            )
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(sorted_keys).collect())
        }
        other => other,
    }
}

/// Compares `value`'s JSON with a checked-in file (`UPDATE_GOLDEN=1` rewrites it). A diff is a change to the public
/// contract chat and plugins rely on: add variants and optional fields, never rename.
pub fn assert_golden<T>(path: PathBuf, value: &T)
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let json = serde_json::to_string_pretty(value).unwrap() + "\n";
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &json).unwrap();
    }
    let stored = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("{} is missing: run with UPDATE_GOLDEN=1", path.display()));
    assert_eq!(
        serde_json::from_str::<T>(&stored).unwrap(),
        *value,
        "the stored shape still loads"
    );
    assert_eq!(stored, json, "public JSON changed: {}", path.display());
}
