//! Each format of an export job: its file and its segments of about 30 s (`plan`), and which of them are still to
//! render.

use std::path::PathBuf;

use klipolo_model::project::FormatKind;
use serde::{Deserialize, Serialize};

use super::PartStatus;
use crate::api::ExportQuality;
use crate::export::jobs::format_dir;
use crate::export::quality::{EncodeSettings, EncoderCaps, encode_settings};
use crate::export::runner::Batch;

/// About 30 s of one format: frames `start_frame..start_frame + frames`, closed GOP (it starts with a keyframe and
/// nothing refers across its edges), in its own file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Segment {
    pub index: u32,
    pub start_frame: u64,
    pub frames: u64,
    /// Relative to the job folder.
    pub file: String,
    pub bytes: u64,
    /// CRC-32 of the whole file.
    pub checksum: u32,
    pub status: PartStatus,
    /// CRC-32 of the parameter sets of the encoder that wrote it: segments join only with ones that share them.
    pub config: Option<u32>,
    /// Written by VideoToolbox's software encoder (SoftwareSafe), not the hardware one.
    pub software: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobOutput {
    pub format: FormatKind,
    pub width: u32,
    pub height: u32,
    pub quality: ExportQuality,
    /// As planned with `EncoderCaps::ASSUMED` (part of the settings hash); the worker resolves them again with the
    /// encoders it found.
    pub settings: EncodeSettings,
    pub final_path: PathBuf,
    /// The joined file, relative to the job folder.
    pub file: String,
    pub segments: Vec<Segment>,
    /// `file` is joined and verified (its segments are deleted).
    pub joined: bool,
    /// Where the joined file is being renamed to (saved before the rename: a crash in between can't deliver it
    /// twice).
    pub delivering: Option<PathBuf>,
    /// Where the joined file was renamed to (the job finishes once every output has one).
    pub delivered: Option<PathBuf>,
    /// Its caption file's destination, saved before the move, and where it was moved (a landscape output of a job with
    /// a `caption_file`; the videos' pattern, `caption_file::deliver`).
    #[serde(default)]
    pub caption_delivering: Option<PathBuf>,
    #[serde(default)]
    pub caption_delivered: Option<PathBuf>,
}

impl JobOutput {
    /// Segments still to render (none once joined).
    pub fn pending(&self) -> impl Iterator<Item = &Segment> {
        self.segments
            .iter()
            .filter(move |s| !self.joined && s.status != PartStatus::Done)
    }
}

/// One output per file of `batch`, each cut into segments of `segment_frames` (the last one shorter) over
/// `total_frames` at `fps`, every segment still to render.
pub(super) fn plan(
    batch: &Batch,
    fps: u32,
    total_frames: u64,
    segment_frames: u64,
) -> Vec<JobOutput> {
    batch
        .files
        .iter()
        .map(|f| {
            let settings = encode_settings(
                batch.quality,
                (f.width, f.height),
                fps,
                &EncoderCaps::ASSUMED,
            );
            let ext = settings.extension();
            let name = format_dir(f.format);
            let segments = (0..total_frames.div_ceil(segment_frames))
                .map(|i| {
                    let start_frame = i * segment_frames;
                    Segment {
                        index: i as u32,
                        start_frame,
                        frames: segment_frames.min(total_frames - start_frame),
                        file: format!("{name}/seg-{i:05}.{ext}"),
                        bytes: 0,
                        checksum: 0,
                        status: PartStatus::Pending,
                        config: None,
                        software: false,
                    }
                })
                .collect();
            JobOutput {
                format: f.format,
                width: f.width,
                height: f.height,
                quality: batch.quality,
                settings,
                final_path: PathBuf::from(&f.path),
                file: format!("{name}.{ext}"),
                segments,
                joined: false,
                delivering: None,
                delivered: None,
                caption_delivering: None,
                caption_delivered: None,
            }
        })
        .collect()
}
