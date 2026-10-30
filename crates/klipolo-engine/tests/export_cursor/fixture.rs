//! The fixtures: takes written to disk (`take<k>/cursor.jsonl` with their shape images), recordings and clips playing
//! them, and the pixel test's project.

use std::path::{Path, PathBuf};

use klipolo_cursor::format::{Capture, Line};
use klipolo_cursor::{ClickSource, Us};
use klipolo_model::contrib::builtin;
use klipolo_model::cursor_style::{ClickEffect, CursorCapture, RecordingCursor};
use klipolo_model::project::{Asset, AssetKind, Project, Recording, RecordingKind, SourceRole};
use klipolo_model::timeline::{Clip, ClipSource, ItemContent, Transition, ZoomTarget};

use crate::FPS;
use crate::support::takes::{header, write};
use crate::support::{bx, item, rec_source, secs, zoom_track};

/// A take's shape: its id, its side in pixels (drawn as half as many points), its colour, and whether its image is
/// damaged.
#[derive(Clone, Copy)]
pub struct Shape {
    pub id: u16,
    pub side: u16,
    pub rgba: [u8; 4],
    pub damaged: bool,
}

/// Take `k`'s folder in `dir` (`take<k>/`) with `shapes` as `cursors/<id>.png` and a separate cursor file defining them
/// (hotspots at their top left), then `timed` in time order: its cursor file's path. A damaged image keeps a whole PNG
/// header (so the take's check passes) and loses the rest (so it never decodes).
pub fn write_take(dir: &Path, k: usize, shapes: &[Shape], mut timed: Vec<Line>) -> PathBuf {
    let folder = dir.join(format!("take{k}"));
    std::fs::create_dir_all(folder.join("cursors")).unwrap();
    let mut lines = vec![header(Capture::Separate)];
    for s in shapes {
        let png = folder.join("cursors").join(format!("{}.png", s.id));
        let side = u32::from(s.side);
        image::RgbaImage::from_pixel(side, side, image::Rgba(s.rgba))
            .save(&png)
            .unwrap();
        if s.damaged {
            let whole = std::fs::read(&png).unwrap();
            std::fs::write(&png, &whole[..40]).unwrap();
        }
        lines.push(Line::ShapeDef {
            id: s.id,
            key: format!("{:016x}", u64::from(s.id) + 1),
            file: format!("cursors/{}.png", s.id),
            pw: s.side,
            ph: s.side,
            hx: 0.0,
            hy: 0.0,
            pts: f32::from(s.side) / 2.0,
            sys: None,
        });
    }
    timed.sort_by_key(time_of);
    lines.extend(timed);
    let path = folder.join("cursor.jsonl");
    write(&path, &lines);
    path
}

fn time_of(line: &Line) -> Us {
    match line {
        Line::Move { t, .. }
        | Line::Button { t, .. }
        | Line::Shape { t, .. }
        | Line::Visible { t, .. }
        | Line::Area { t, .. }
        | Line::Clicks { t, .. }
        | Line::Burned { t } => *t,
        Line::Header { .. } | Line::ShapeDef { .. } => 0,
    }
}

/// The cursor gliding straight from `a` to `b` between `from` and `to` seconds, at 120 Hz.
pub fn glide(a: (f32, f32), b: (f32, f32), from: f64, to: f64) -> impl Iterator<Item = Line> {
    let steps = ((to - from) * 120.0).round() as i64;
    (0..=steps).map(move |k| {
        let s = k as f32 / steps as f32;
        Line::Move {
            t: secs(from) + (secs(to) - secs(from)) * k / steps,
            x: a.0 + (b.0 - a.0) * s,
            y: a.1 + (b.1 - a.1) * s,
        }
    })
}

/// The cursor at rest at `(x, y)` from `from` to `to` seconds: a heartbeat every 500 ms.
pub fn rest(x: f32, y: f32, from: f64, to: f64) -> impl Iterator<Item = Line> {
    let beats = ((to - from) * 2.0).round() as i64;
    (0..=beats).map(move |k| Line::Move {
        t: secs(from) + k * 500_000,
        x,
        y,
    })
}

/// A primary click at `(x, y)` from `down` to `up` seconds.
pub fn click((x, y): (f32, f32), down: f64, up: f64) -> [Line; 2] {
    let line = |t, d| Line::Button { t, btn: 0, d, x, y };
    [line(secs(down), 1), line(secs(up), 0)]
}

/// Recording `rec<k>` of screen `screen<k>` (1920 × 1080 at 30 fps, `seconds` long, its file at `file`) whose cursor,
/// `capture`d, is `cursor`, and a clip `clip<k>` playing its `[from, to)` seconds at the end of the main track.
pub fn add_take(
    p: &mut Project,
    k: usize,
    file: &str,
    seconds: f64,
    cursor: (&Path, CursorCapture),
    (from, to): (f64, f64),
) {
    let screen = rec_source(&format!("screen{k}"), SourceRole::Screen, 0, seconds);
    p.assets.push(Asset {
        id: screen.id.clone(),
        kind: AssetKind::Video,
        path: file.into(),
        name: screen.id.clone(),
        duration_us: secs(seconds),
        width: 1920,
        height: 1080,
        fps: 30.0,
        has_audio: false,
        audio_lead_us: None,
        rotation: None,
    });
    p.recordings.push(Recording {
        id: format!("rec{k}"),
        kind: RecordingKind::Screen,
        primary: screen.id.clone(),
        sources: vec![screen],
        cursor: Some(RecordingCursor {
            path: cursor.0.to_string_lossy().into_owned(),
            capture: cursor.1,
            clicks: ClickSource::Events,
        }),
        markers: Vec::new(),
        game: None,
    });
    let mut clip = Clip::new(
        ClipSource::Recording {
            id: format!("rec{k}"),
        },
        secs(from),
        secs(to),
    );
    clip.id = format!("clip{k}");
    p.timeline.main.push(clip);
}

/// A generated media file, or (planning only) a path where none is.
fn media(name: &str, real: bool) -> String {
    if real {
        crate::support::fixture(name).display().to_string()
    } else {
        format!("/media/{name}")
    }
}

/// The fixture: take 1 over red footage, its cursor gliding with a click and two shape changes (green,
/// yellow at 0.5 s, then from 0.7 s a shape whose image doesn't decode, so both draw the Arrow look there), under a 2×
/// Rect zoom from 0.1 s to 0.7 s; a 0.4 s dissolve at 1 s into take 2 over blue footage, its cursor at rest (two cursor
/// slots). Clicks ripple and press. `real`: the footage is the generated fixtures (the pixel tests), else paths with
/// nothing there (planning only).
pub fn fixture(dir: &Path, real: bool) -> Project {
    let mut p = Project::new("Cursor export");
    p.id = "project".into();
    p.settings.fps = FPS;
    p.cursor.click = ClickEffect::RippleAndPress;
    let shape = |id, rgba, damaged| Shape {
        id,
        side: 48,
        rgba,
        damaged,
    };
    let shapes = [
        shape(0, [40, 200, 60, 255], false),
        shape(1, [250, 210, 30, 255], false),
        shape(2, [255, 255, 255, 255], true),
    ];
    let mut one: Vec<Line> = [(0.0, 0), (0.5, 1), (0.7, 2)]
        .map(|(t, id)| Line::Shape { t: secs(t), id })
        .into();
    one.extend(glide((0.3, 0.35), (0.6, 0.55), 0.0, 1.4));
    one.extend(click((0.4, 0.42), 0.32, 0.4));
    let one = write_take(dir, 1, &shapes, one);
    let mut two: Vec<Line> = vec![Line::Shape { t: 0, id: 0 }];
    two.extend(rest(0.55, 0.45, 0.0, 2.0));
    let two = write_take(dir, 2, &[shape(0, [230, 60, 230, 255], false)], two);
    add_take(
        &mut p,
        1,
        &media("solid-red.mp4", real),
        2.0,
        (&one, CursorCapture::Separate),
        (0.0, 1.0),
    );
    add_take(
        &mut p,
        2,
        &media("solid-blue.mp4", real),
        2.0,
        (&two, CursorCapture::Separate),
        (0.5, 2.0),
    );
    p.timeline.main[0].transition_out = Some(Transition {
        effect: builtin::TRANSITION_DISSOLVE.into(),
        duration_us: secs(0.4),
        params: Default::default(),
    });
    p.timeline.tracks = vec![zoom_track(vec![item(
        "zoom",
        0.1,
        0.6,
        bx(0.0, 0.0, 1.0, 1.0),
        ItemContent::Zoom {
            target: ZoomTarget::Rect {
                rect: bx(0.25, 0.25, 0.5, 0.5),
            },
            scale: 2.0,
        },
    )])];
    p
}
