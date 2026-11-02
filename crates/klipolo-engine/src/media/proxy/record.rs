//! What a finished editing copy was made from (`<proxies root>/<stem>.json`, beside it): the scheduler and the preview
//! map a copy only while its record matches its source.

use std::path::{Path, PathBuf};

use klipolo_model::time::Us;
use klipolo_platform::durable::{self, Durability};
use serde::{Deserialize, Serialize};

use super::decide::proxy_rate;
use super::facts::SourceFacts;
use super::job::ProxyJob;
use crate::export::shared::FrameRate;
use crate::media::key::SourceId;

/// `<root>/<stem>.json`, beside a finished copy: what it was made from (`CopyRecord`).
pub fn record_path(out: &Path) -> PathBuf {
    out.with_extension("json")
}

const RECORD_VERSION: u32 = 1;

/// What a finished copy was made from, written beside it before it is put in place: the scheduler maps a copy only while
/// its source is still that file (size, time and number) and, when its facts are known, still those facts. A
/// copy without one, or with one that doesn't match, is the cache's own to delete.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CopyRecord {
    pub version: u32,
    pub source: String,
    pub stem: String,
    pub source_id: SourceId,
    pub source_width: u32,
    pub source_height: u32,
    pub duration_us: Us,
    pub rate: FrameRate,
    pub width: u32,
    pub height: u32,
    pub frames: u64,
    /// Made with the source's frames numbered from its video stream's start, as every decoder numbers them (false in an
    /// older record: such a copy of a source whose picture starts late shows its frames late).
    #[serde(default)]
    pub from_stream_start: bool,
}

impl CopyRecord {
    pub fn of(job: &ProxyJob) -> Self {
        Self {
            version: RECORD_VERSION,
            source: job.source.clone(),
            stem: job.stem.clone(),
            source_id: job.source_id,
            source_width: job.source_width,
            source_height: job.source_height,
            duration_us: job.duration_us,
            rate: job.rate,
            width: job.width,
            height: job.height,
            frames: job.total_frames,
            from_stream_start: job.from_stream_start,
        }
    }

    /// Writes it beside `out` (atomically, ordered after the copy's own writes).
    pub fn write(&self, out: &Path) -> std::io::Result<()> {
        let bytes = serde_json::to_vec(self).map_err(std::io::Error::other)?;
        durable::write_atomic(&record_path(out), &bytes, Durability::Ordered)
    }

    /// The record beside `out`, if one is there and reads.
    pub fn read(out: &Path) -> Option<Self> {
        let record: Self = serde_json::from_slice(&std::fs::read(record_path(out)).ok()?).ok()?;
        (record.version == RECORD_VERSION).then_some(record)
    }

    /// Whether `out` is the copy of `source` as the file is now (`now`: its stat, taken by the caller) and, given the
    /// file's facts, of what they describe (shape, length and exact rate) and showing its frames where the source does
    /// (`numbered_late`).
    pub fn current(
        &self,
        source: &str,
        out: &Path,
        now: Option<SourceId>,
        facts: Option<&SourceFacts>,
    ) -> bool {
        self.source == source
            && out.file_stem().and_then(|s| s.to_str()) == Some(self.stem.as_str())
            && now == Some(self.source_id)
            && facts.is_none_or(|f| {
                f.id.is_none_or(|id| id == self.source_id)
                    && (f.width, f.height, f.duration_us)
                        == (self.source_width, self.source_height, self.duration_us)
                    && proxy_rate(f) == Some(self.rate)
                    && !self.numbered_late(f)
            })
    }

    /// An older copy (made with the old numbering) of a source whose picture starts at least half a frame into the
    /// file: it was made with frames numbered from the file's 0, so it shows each frame that many frames late. A start
    /// under half a frame numbered the same frames either way, so most older copies are kept.
    fn numbered_late(&self, facts: &SourceFacts) -> bool {
        !self.from_stream_start && super::numbering::starts_late(facts.start_us, self.rate)
    }
}

/// Deletes a finished copy and what lies beside it (its record and keyframe index): derived data, the cache's own.
pub fn remove_copy(out: &Path) {
    for path in [
        out.to_path_buf(),
        record_path(out),
        crate::media::keyframes::beside(out),
    ] {
        if let Err(e) = std::fs::remove_file(&path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(%e, "editing copy: a stale copy couldn't be removed");
        }
    }
}
