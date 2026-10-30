//! Screen takes with cursor files on disk: each take in its own folder (`take<k>/cursor.jsonl`, its shapes in
//! `take<k>/cursors/`), played one after another on the main track.

use std::io::Write;
use std::path::{Path, PathBuf};

use klipolo_cursor::format::{Capture, Line, write_line};
use klipolo_cursor::{ClickSource, Us};
use klipolo_model::cursor_style::{CursorCapture, RecordingCursor};
use klipolo_model::project::*;
use klipolo_model::timeline::{Clip, ClipSource};

use super::{rec_source, secs};

/// A take's first line: a 1440 × 900 point screen, exact clicks.
pub fn header(capture: Capture) -> Line {
    Line::Header {
        v: 2,
        capture,
        clicks: ClickSource::Events,
        w: 1440.0,
        h: 900.0,
    }
}

/// `lines` written as `path` (its folder made).
pub fn write(path: &Path, lines: &[Line]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut bytes = Vec::new();
    for line in lines {
        write_line(line, &mut bytes);
    }
    std::fs::File::create(path)
        .unwrap()
        .write_all(&bytes)
        .unwrap();
}

/// `seconds` of a cursor gliding back and forth at 120 Hz (`capture` in the header), so every smoothing stores steps.
pub fn gliding(capture: Capture, seconds: f64) -> Vec<Line> {
    let mut lines = vec![header(capture)];
    let steps = (seconds * 120.0) as i64;
    lines.extend((0..steps).map(|i| {
        let t = i * 8_333;
        let a = t as f64 / 1e6;
        Line::Move {
            t,
            x: (0.5 + 0.3 * (a * 1.3).sin()) as f32,
            y: (0.5 + 0.2 * (a * 0.7).cos()) as f32,
        }
    }));
    lines
}

/// Where take `k` (from 1) keeps its cursor file under `dir`.
pub fn cursor_file(dir: &Path, k: usize) -> PathBuf {
    dir.join(format!("take{k}")).join("cursor.jsonl")
}

/// `n` takes of 10 s, each its own recording (`rec<k>`) whose cursor file glides (`capture`), played by clips
/// `clip<k>` one after another.
pub fn cursor_takes(n: usize, dir: &Path, capture: CursorCapture) -> Project {
    let mut p = Project::new("Takes");
    p.id = "project".into();
    for k in 1..=n {
        let path = cursor_file(dir, k);
        let head = match capture {
            CursorCapture::Burned => Capture::Burned,
            _ => Capture::Separate,
        };
        write(&path, &gliding(head, 10.0));
        add_take(&mut p, k, &path, capture);
    }
    p
}

/// Recording `rec<k>` (a 10 s screen, cursor file `path`) and a clip `clip<k>` playing it at the end of the main track.
pub fn add_take(p: &mut Project, k: usize, path: &Path, capture: CursorCapture) {
    let screen = rec_source(&format!("screen{k}"), SourceRole::Screen, 0, 10.0);
    p.recordings.push(Recording {
        id: format!("rec{k}"),
        kind: RecordingKind::Screen,
        primary: screen.id.clone(),
        sources: vec![screen],
        cursor: Some(RecordingCursor {
            path: path.to_string_lossy().into_owned(),
            capture,
            clicks: ClickSource::Events,
        }),
        markers: Vec::new(),
        game: None,
    });
    let mut clip = Clip::new(
        ClipSource::Recording {
            id: format!("rec{k}"),
        },
        0,
        secs(10.0),
    );
    clip.id = format!("clip{k}");
    p.timeline.main.push(clip);
}

/// A µs time from seconds, for cursor lines.
pub fn us(seconds: f64) -> Us {
    secs(seconds)
}
