//! Recordings with runs: a take whose sources are files on the recording clock, and the facts of a file the recorder
//! wrote.

use klipolo_cursor::format::{Capture, Line};
use klipolo_cursor::{ClickSource, CursorTrack};
use klipolo_engine::export::shared::FrameRate;
use klipolo_engine::media::cursor::CursorTracks;
use klipolo_engine::media::proxy::facts::SourceFacts;
use klipolo_model::Op;
use klipolo_model::cursor_style::{CursorCapture, CursorStyle, RecordingCursor};
use klipolo_model::project::*;
use klipolo_model::time::Us;
use klipolo_model::timeline::{Clip, ClipSource, ItemContent, ZoomTarget};

use super::secs;

fn file(id: &str, kind: AssetKind, seconds: f64) -> Asset {
    let video = kind == AssetKind::Video;
    Asset {
        id: id.into(),
        kind,
        path: format!("/media/{id}"),
        name: id.into(),
        duration_us: secs(seconds),
        width: if video { 1920 } else { 0 },
        height: if video { 1080 } else { 0 },
        fps: if video { 60.0 } else { 0.0 },
        has_audio: !video,
        audio_lead_us: None,
        made_with: None,
        rotation: None,
    }
}

/// A source whose runs are `(offset µs, seconds, drift ppm)`, each its own file: `id`, then `id-2`, `id-3`.
fn source(p: &mut Project, id: &str, role: SourceRole, runs: &[(Us, f64, f64)]) -> RecordingSource {
    let kind = if role.is_video() {
        AssetKind::Video
    } else {
        AssetKind::Audio
    };
    let runs = runs
        .iter()
        .enumerate()
        .map(|(k, &(offset_us, seconds, drift_ppm))| {
            let asset_id = if k == 0 {
                id.to_owned()
            } else {
                format!("{id}-{}", k + 1)
            };
            p.assets.push(file(&asset_id, kind, seconds));
            SourceRun {
                asset_id,
                offset_us,
                duration_us: secs(seconds),
                drift_ppm,
                file_in_us: 0,
            }
        })
        .collect();
    RecordingSource {
        id: id.into(),
        role,
        label: format!("capture.source.{id}"),
        runs,
        nudge_us: 0,
        mix: SourceMix::default(),
        synced: None,
    }
}

/// A take of a 60 s screen run (`screen`) plus `sources`, and one clip ("take") over the whole take.
fn take(sources: impl FnOnce(&mut Project) -> Vec<RecordingSource>) -> Project {
    let mut p = Project::new("Recording runs");
    p.id = "project".into();
    let mut all = vec![source(
        &mut p,
        "screen",
        SourceRole::Screen,
        &[(0, 60.0, 0.0)],
    )];
    all.extend(sources(&mut p));
    let rec = Recording {
        id: "rec".into(),
        kind: RecordingKind::Screen,
        sources: all,
        primary: "screen".into(),
        cursor: None,
        markers: Vec::new(),
        game: None,
    };
    let mut clip = Clip::new(
        ClipSource::Recording { id: "rec".into() },
        0,
        rec.duration_us(),
    );
    clip.id = "take".into();
    p.recordings = vec![rec];
    p.timeline.main = vec![clip];
    p
}

/// The screen plus each `(id, role, offset µs, seconds, drift ppm)` as one run of its own file.
pub fn recording_project(sources: &[(&str, SourceRole, Us, f64, f64)]) -> Project {
    take(|p| {
        sources
            .iter()
            .map(|&(id, role, offset, seconds, ppm)| source(p, id, role, &[(offset, seconds, ppm)]))
            .collect()
    })
}

/// The screen plus source `id` recorded as `runs` (`(offset µs, seconds)`; a heal started each new file).
pub fn recording_project_runs(id: &str, role: SourceRole, runs: &[(Us, f64)]) -> Project {
    let runs: Vec<(Us, f64, f64)> = runs.iter().map(|&(o, s)| (o, s, 0.0)).collect();
    take(|p| vec![source(p, id, role, &runs)])
}

/// Where `auto_zoom_project`'s recording keeps its cursor file.
pub const CURSOR_PATH: &str = "/media/rec/cursor.jsonl";

/// The 60 s screen take with cursor data `track` (loaded as `CURSOR_PATH`), and one Auto zoom at `scale` from `start`
/// to `end` seconds on a zoom track.
pub fn auto_zoom_project(
    track: CursorTrack,
    start: f64,
    end: f64,
    scale: f64,
) -> (Project, CursorTracks) {
    let mut p = take(|_| Vec::new());
    p.recordings[0].cursor = Some(RecordingCursor::burned_in(CURSOR_PATH));
    p.timeline.tracks = vec![super::zoom_track(vec![super::item(
        "auto",
        start,
        end - start,
        super::bx(0.0, 0.0, 1.0, 1.0),
        ItemContent::Zoom {
            target: ZoomTarget::Auto { using: vec![] },
            scale,
        },
    )])];
    let tracks = CursorTracks::default();
    tracks.insert(CURSOR_PATH, track);
    (p, tracks)
}

/// Where take `k`'s cursor rests in `auto_zoom_takes`: its own spot along the top.
pub fn take_cursor(k: usize) -> (f32, f32) {
    (0.1 + 0.15 * k as f32, 0.2)
}

/// `n` takes of 10 s recorded into one project (UX decision 6: each a recording of its own), `rec1`… with cursor files
/// `cursor1.jsonl`… in `dir` (format 2; take `k`'s cursor resting at `take_cursor(k)`, sampled at 30 Hz, clicking there
/// 5 s in), played one after another by clips `take1`…, each under an Auto zoom at 2×.
pub fn auto_zoom_takes(n: usize, dir: &std::path::Path) -> Project {
    use std::io::Write;
    let mut p = Project::new("Takes");
    p.id = "project".into();
    let mut zooms = Vec::new();
    for k in 1..=n {
        let path = dir.join(format!("cursor{k}.jsonl"));
        let mut f = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
        let (x, y) = take_cursor(k);
        writeln!(
            f,
            r#"{{"k":"h","v":2,"capture":"burned","clicks":"sampled","w":1920,"h":1080}}"#
        )
        .unwrap();
        for i in 0..300_i64 {
            writeln!(f, r#"{{"k":"m","t":{},"x":{x},"y":{y}}}"#, i * 33_334).unwrap();
            // A click where it rests, 5 s in: what the take's Auto zoom frames.
            if i == 150 {
                for d in [1, 0] {
                    let t = 5_000_000 + 50_000 * (1 - d);
                    writeln!(f, r#"{{"k":"b","t":{t},"btn":0,"d":{d},"x":{x},"y":{y}}}"#).unwrap();
                }
            }
        }
        f.flush().unwrap();
        let screen = source(
            &mut p,
            &format!("screen{k}"),
            SourceRole::Screen,
            &[(0, 10.0, 0.0)],
        );
        p.recordings.push(Recording {
            id: format!("rec{k}"),
            kind: RecordingKind::Screen,
            primary: screen.id.clone(),
            sources: vec![screen],
            cursor: Some(RecordingCursor::burned_in(path.to_string_lossy())),
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
        clip.id = format!("take{k}");
        p.timeline.main.push(clip);
        zooms.push(super::item(
            &format!("auto{k}"),
            (k - 1) as f64 * 10.0 + 1.0,
            8.0,
            super::bx(0.0, 0.0, 1.0, 1.0),
            ItemContent::Zoom {
                target: ZoomTarget::Auto { using: vec![] },
                scale: 2.0,
            },
        ));
    }
    p.timeline.tracks = vec![super::zoom_track(zooms)];
    p
}

/// The 60 s screen take (1920 × 1080) whose cursor file is `lines` (written as `dir/take/cursor.jsonl`), drawn with
/// `style`. Its capture is what import reads from the file: burned in when the header says so, else separate, burned in
/// from an `x` line's time if there is one; its clicks are exact.
pub fn cursor_project(dir: &std::path::Path, lines: &[Line], style: CursorStyle) -> Project {
    let path = dir.join("take").join("cursor.jsonl");
    super::takes::write(&path, lines);
    let burned_at = lines.iter().find_map(|l| match l {
        Line::Burned { t } => Some(*t),
        _ => None,
    });
    let capture = match (lines.first(), burned_at) {
        (
            Some(Line::Header {
                capture: Capture::Burned,
                ..
            }),
            _,
        ) => CursorCapture::Burned,
        (_, Some(at_us)) => CursorCapture::BurnedFrom { at_us },
        _ => CursorCapture::Separate,
    };
    let mut p = take(|_| Vec::new());
    p.recordings[0].cursor = Some(RecordingCursor {
        path: path.to_string_lossy().into_owned(),
        capture,
        clicks: ClickSource::Events,
    });
    p.cursor = style;
    p
}

/// The main track split at `seconds` (`Op::Split`).
pub fn split_at(p: &mut Project, seconds: f64) {
    *p = klipolo_model::apply_ops(
        p,
        &[Op::Split {
            at_us: secs(seconds),
        }],
    )
    .expect("split");
}

/// A file as the recorder writes it: `gop_us` between keyframes, 8-bit SDR at a steady rate, decoded in hardware.
pub fn facts(width: u32, height: u32, fps: u32, codec: &str, gop_us: Us) -> SourceFacts {
    SourceFacts {
        width,
        height,
        rate: FrameRate::whole(fps),
        codec: codec.into(),
        ten_bit: false,
        ten_bit_hw: false,
        hdr: false,
        max_key_interval_us: gop_us,
        variable_rate: false,
        hw_decodes: true,
        duration_us: secs(3_600.0),
        start_us: 0,
        id: None,
    }
}
