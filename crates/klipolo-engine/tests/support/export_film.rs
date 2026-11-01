//! Exporting the film in process, as the worker runs it, and checking what the output shows (macOS, `trial` builds).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use klipolo_engine::api::ExportQuality;
use klipolo_engine::export::job::ExportJob;
use klipolo_engine::export::naming::plan_files;
use klipolo_engine::export::protocol::{Lines, WorkerLine, parse_line};
use klipolo_engine::export::runner::Batch;
use klipolo_engine::export::shared::{Control, WorkerError};
use klipolo_engine::export::worker::{Ended, export};
use klipolo_model::project::{Asset, AssetKind, FormatKind, Project};
use klipolo_model::timeline::{Clip, ClipSource};

use super::stalls::{FILM_FPS, film_luma};
use super::{luma, secs};

/// The bytes of a line not finished yet, and every line so far with when it came.
pub type Said = (Vec<u8>, Vec<(Instant, WorkerLine)>);

/// What the worker said, each line with when.
#[derive(Clone, Default)]
pub struct Heard(pub Arc<Mutex<Said>>);

impl Write for Heard {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut heard = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        heard.0.extend_from_slice(buf);
        while let Some(end) = heard.0.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = heard.0.drain(..=end).collect();
            if let Some(line) = parse_line(&String::from_utf8_lossy(&line)) {
                heard.1.push((Instant::now(), line));
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub struct Exported {
    pub ended: Result<Ended, WorkerError>,
    pub lines: Vec<(Instant, WorkerLine)>,
    pub job: ExportJob,
    pub output: Option<PathBuf>,
    pub ended_at: Instant,
}

/// An export job of the first `seconds` of the film to 1080p30 (Standard), in `segment_s` segments, in `dir`.
pub fn film_job(file: &Arc<str>, dir: &Path, seconds: f64, segment_s: u32) -> ExportJob {
    let mut p = Project::new("Stalls");
    p.assets = vec![Asset {
        id: "film".into(),
        kind: AssetKind::Video,
        path: file.to_string(),
        name: "film".into(),
        duration_us: secs(11.9),
        width: 1920,
        height: 1080,
        fps: FILM_FPS,
        has_audio: true,
        audio_lead_us: None,
        made_with: None,
        rotation: Some(0),
    }];
    p.timeline.main = vec![Clip::new(
        ClipSource::Asset { id: "film".into() },
        0,
        secs(seconds),
    )];
    let quality = ExportQuality::Standard;
    let batch = Batch {
        files: plan_files(&p, &[FormatKind::Landscape], quality, dir),
        project: p,
        fps: 30,
        quality,
        pcm_dir: None,
        speech: Default::default(),
        text_language: "en".into(),
        contribs: Default::default(),
        caption_file: None,
        allow_stand_ins: false,
        activity_dir: None,
        allow_missing_cursor: false,
    };
    ExportJob::create(&batch, segment_s).unwrap()
}

/// The first `seconds` of the film exported to 1080p30 (Standard) in `segment_s` segments, in process.
pub async fn export_film(file: &Arc<str>, dir: &Path, seconds: f64, segment_s: u32) -> Exported {
    ffmpeg::init().unwrap();
    let job = film_job(file, dir, seconds, segment_s);
    let heard = Heard::default();
    let ended = export(
        job.clone(),
        &mut Lines::new(heard.clone()),
        Arc::new(Control::default()),
    )
    .await;
    let ended_at = Instant::now();
    let lines = std::mem::take(&mut heard.0.lock().unwrap().1);
    let output = lines.iter().find_map(|(_, line)| match line {
        WorkerLine::FormatDone { temp_path, .. } => Some(temp_path.clone()),
        _ => None,
    });
    Exported {
        ended,
        lines,
        job: ExportJob::open(&job.dir).unwrap(),
        output,
        ended_at,
    }
}

/// The output shows the film frames it should: (output frame, film frame) pairs, by luma.
pub fn shows(output: &Path, pairs: &[(usize, u32)]) {
    let wanted: Vec<usize> = pairs.iter().map(|(n, _)| *n).collect();
    let planes = luma(output, &wanted);
    assert_eq!(planes.len(), pairs.len());
    for ((n, film), plane) in pairs.iter().zip(&planes) {
        let middle = plane[plane.len() / 2 + 960];
        assert!(
            middle.abs_diff(film_luma(*film)) <= 3,
            "output frame {n} shows luma {middle}, film frame {film} is {}",
            film_luma(*film)
        );
    }
}

/// Output frames 263 and 264 (1080p30) show film frame 210, the one the reader stalled before; 262 and 265 its
/// neighbours.
pub const AROUND_THE_STALL: [(usize, u32); 4] = [(262, 209), (263, 210), (264, 210), (265, 211)];
