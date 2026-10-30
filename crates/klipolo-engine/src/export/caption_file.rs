//! An export's caption file: SRT or WebVTT, made once when the job is
//! (`frozen`, from the pages it freezes: `klipolo_model::captions::sidecar`) and written into its folder with the rest,
//! then delivered beside each landscape video under the video's own name (`deliver`), never overwriting.
//!
//! Failure halfway: nothing half-written ever reaches the exports folder. The frozen file is written with
//! the job (temp and rename, in its staging folder); each delivery copies it to a temp file in the job folder, then
//! renames that into place, its destination recorded before the move and the move after it, so a crash in between
//! delivers it once. A cancelled export deletes the job folder with everything in it; a failed one keeps it, and no
//! caption file is outside it. A caption file that can't be delivered leaves the video delivered: the export finishes
//! and says so (`ExportEvent::CaptionFileFailed`).

use std::collections::HashSet;
use std::io;
use std::path::{Component, Path, PathBuf};

use klipolo_model::captions::sidecar::{CaptionFile, cues_of, sidecar_pages};
use klipolo_model::captions::{Captions, captions_in};
use klipolo_model::project::{FormatKind, Project};
use klipolo_model::speech::Speech;

use super::job::{ExportJob, JobOutput};
use super::naming::free_path;
use super::shared::{is_disk_full, write_atomic};

/// The frozen caption file's name in the job folder.
pub fn frozen_name(kind: CaptionFile) -> String {
    format!("captions.{}", kind.extension())
}

/// The caption file `want` asks for, made from the pages `captions` freezes (Clean's: one source of truth with what
/// is drawn), when the export has a landscape file and some word to show: its kind and text. None otherwise.
pub(crate) fn frozen(
    project: &Project,
    speech: &Speech,
    captions: &Captions,
    outputs: &[JobOutput],
    want: Option<CaptionFile>,
) -> Option<(CaptionFile, String)> {
    let kind = want?;
    if !outputs.iter().any(|o| o.format == FormatKind::Landscape) {
        return None;
    }
    let pages = sidecar_pages(project, speech, captions)?;
    let cues = cues_of(&pages);
    (!cues.is_empty()).then(|| (kind, kind.write(&cues)))
}

/// What a job freezes of what was said: the pages of its files' formats, their bytes as `captions.json` holds them, and
/// the caption file asked for (`caption_file::frozen`: its kind and text), made from those pages.
pub(crate) struct Frozen {
    pub captions: Captions,
    pub json: Vec<u8>,
    pub file: Option<(CaptionFile, String)>,
}

/// The pages of `outputs`' formats for `project` and `speech`, and the caption file `want` asks for.
pub(crate) fn job_captions(
    project: &Project,
    speech: &Speech,
    outputs: &[JobOutput],
    want: Option<CaptionFile>,
) -> serde_json::Result<Frozen> {
    let formats: Vec<FormatKind> = outputs.iter().map(|o| o.format).collect();
    let captions = captions_in(project, speech, &formats);
    let json = serde_json::to_vec(&captions)?;
    let file = frozen(project, speech, &captions, outputs, want);
    Ok(Frozen {
        captions,
        json,
        file,
    })
}

/// Why a caption file wasn't delivered (the video was).
#[derive(Debug)]
pub(crate) struct NotDelivered {
    pub disk_full: bool,
    pub reason: String,
}

impl From<io::Error> for NotDelivered {
    fn from(e: io::Error) -> Self {
        Self {
            disk_full: is_disk_full(&e),
            reason: e.to_string(),
        }
    }
}

fn refused(reason: &str) -> NotDelivered {
    NotDelivered {
        disk_full: false,
        reason: reason.into(),
    }
}

/// Where a caption file beside `video` goes: `<exports>/<video's stem>.<ext>`, the next free name when taken (on disk
/// or by `taken`). Refused unless the video is in the export's own folder and its stem is a plain file name: a caption
/// file is never written anywhere else.
fn beside(job: &ExportJob, video: &Path, ext: &str, taken: &HashSet<PathBuf>) -> Option<PathBuf> {
    let exports = job.dir.parent()?.parent()?;
    let stem = video.file_stem()?.to_str()?;
    let plain = matches!(
        Path::new(stem).components().collect::<Vec<_>>().as_slice(),
        [Component::Normal(_)]
    );
    if !plain || !video.parent().is_some_and(|dir| same_folder(dir, exports)) {
        return None;
    }
    let target = free_path(exports, stem, ext, taken);
    (target.parent() == Some(exports)).then_some(target)
}

/// Whether `target` is a file directly in the export's own folder.
fn in_exports(job: &ExportJob, target: &Path) -> bool {
    let exports = job.dir.parent().and_then(Path::parent);
    match (target.parent(), exports) {
        (Some(dir), Some(exports)) => same_folder(dir, exports),
        _ => false,
    }
}

/// The same folder, spelled alike or once links are resolved.
fn same_folder(a: &Path, b: &Path) -> bool {
    a == b || matches!((a.canonicalize(), b.canonicalize()), (Ok(a), Ok(b)) if a == b)
}

/// Output `index`'s caption file, after its video is in place at `video`: Ok(None) when it has none (not landscape, or
/// the job saves no caption file), else where it now is.
pub(super) fn deliver(
    job: &mut ExportJob,
    index: usize,
    video: &Path,
    taken: &HashSet<PathBuf>,
) -> Result<Option<PathBuf>, NotDelivered> {
    let Some(kind) = job.caption_file else {
        return Ok(None);
    };
    if job.outputs[index].format != FormatKind::Landscape {
        return Ok(None);
    }
    if let Some(done) = job.outputs[index].caption_delivered.clone() {
        return Ok(Some(done));
    }
    let bytes = std::fs::read(job.path(&frozen_name(kind)))?;
    // Recorded before a crash: moved already (it holds this file), still to move there, or taken since. Only ever in
    // the export's own folder, as `beside` names a new one: `job.json` can't send it elsewhere.
    let recorded = job.outputs[index]
        .caption_delivering
        .clone()
        .filter(|t| in_exports(job, t));
    if let Some(target) = &recorded
        && target.is_file()
        && std::fs::read(target).is_ok_and(|b| b == bytes)
    {
        return recorded_delivered(job, index, target.clone());
    }
    let target = match recorded.filter(|t| !t.exists() && !taken.contains(t)) {
        Some(t) => t,
        None => beside(job, video, kind.extension(), taken)
            .ok_or_else(|| refused("its name would leave the exports folder"))?,
    };
    let temp = job.path(&format!("caption-{index}.{}", kind.extension()));
    write_atomic(&temp, &bytes)?;
    job.outputs[index].caption_delivering = Some(target.clone());
    job.save()?;
    if let Err(e) = std::fs::rename(&temp, &target) {
        let _ = std::fs::remove_file(&temp);
        return Err(e.into());
    }
    recorded_delivered(job, index, target)
}

fn recorded_delivered(
    job: &mut ExportJob,
    index: usize,
    target: PathBuf,
) -> Result<Option<PathBuf>, NotDelivered> {
    job.outputs[index].caption_delivered = Some(target.clone());
    if let Err(e) = job.save() {
        tracing::warn!(%e, "export: the delivered caption file couldn't be recorded");
    }
    Ok(Some(target))
}
