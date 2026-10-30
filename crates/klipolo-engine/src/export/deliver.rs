//! Delivering a finished job: each joined file renamed into place, never overwriting, with its destination
//! recorded before the move and the move recorded after it (a crash in between can't deliver it twice), each landscape
//! video's caption file beside it the same way, then the job folder deleted.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::caption_file;
use super::job::{ExportJob, JobState};
use super::naming::free_path;
use super::runner::{Outcome, failed};
use crate::api::ExportEvent;

/// Renames each joined file into place (a name taken since planning gets the next free one: nothing is ever
/// overwritten), recording where before the move and that it moved after it, then its caption file beside it, then
/// deletes the job folder. A caption file that can't be delivered is said (`CaptionFileFailed`) and the video kept; one
/// asked for with nothing to save is said too (`CaptionFileEmpty`).
pub(crate) fn finish(job: &mut ExportJob, emit: &(dyn Fn(ExportEvent) + Send + Sync)) -> Outcome {
    let mut finals: Vec<PathBuf> = Vec::new();
    let videos = job.outputs.len();
    for index in 0..videos {
        let video = match job.outputs[index].delivered.clone() {
            Some(path) => path,
            None => match move_into_place(job, index, &finals, emit) {
                Ok(path) => path,
                Err(outcome) => return outcome,
            },
        };
        emit(ExportEvent::FileReady {
            index: index as u32,
            path: video.display().to_string(),
        });
        finals.push(video.clone());
        let taken: HashSet<PathBuf> = finals.iter().cloned().collect();
        match caption_file::deliver(job, index, &video, &taken) {
            Ok(None) => {}
            Ok(Some(path)) => {
                // After the videos' indexes: `Started.files` holds the videos only.
                emit(ExportEvent::FileReady {
                    index: (videos + index) as u32,
                    path: path.display().to_string(),
                });
                finals.push(path);
            }
            Err(e) => {
                tracing::warn!(reason = %e.reason, "export: the caption file couldn't be saved");
                emit(ExportEvent::CaptionFileFailed {
                    index: index as u32,
                    disk_full: e.disk_full,
                });
            }
        }
    }
    if job.captions_empty {
        emit(ExportEvent::CaptionFileEmpty);
    }
    if let Err(e) = std::fs::remove_dir_all(&job.dir) {
        tracing::warn!(%e, "export: the job folder couldn't be removed");
    }
    emit(ExportEvent::Finished {
        notes: (!job.notes.is_empty()).then(|| job.notes.clone()),
    });
    Outcome::Finished(finals)
}

/// Output `index`'s joined file renamed into place; the failed outcome when it can't be.
fn move_into_place(
    job: &mut ExportJob,
    index: usize,
    finals: &[PathBuf],
    emit: &(dyn Fn(ExportEvent) + Send + Sync),
) -> Result<PathBuf, Outcome> {
    let o = &job.outputs[index];
    let mut target = o.final_path.clone();
    if target.exists() || finals.contains(&target) {
        let dir = target.parent().map(Path::to_path_buf).unwrap_or_default();
        let base = target
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let taken: HashSet<PathBuf> = finals.iter().cloned().collect();
        target = free_path(&dir, &base, o.settings.extension(), &taken);
    }
    let joined = job.path(&o.file);
    job.outputs[index].delivering = Some(target.clone());
    if let Err(e) = job.save() {
        // Without that record a crash after the move couldn't be told apart: don't move it.
        tracing::warn!(%e, "export: the file's destination couldn't be recorded");
        return Err(failed(
            "The finished video couldn't be moved into your Exports folder. Try again.".into(),
            emit,
        ));
    }
    if let Err(e) = std::fs::rename(&joined, &target) {
        tracing::warn!(%e, "export: the finished file couldn't be moved into place");
        let message =
            "The finished video couldn't be moved into your Exports folder. Try again.".to_string();
        job.state = JobState::Failed;
        job.message = Some(message.clone().into());
        let _ = job.save();
        return Err(failed(message.into(), emit));
    }
    job.outputs[index].delivered = Some(target.clone());
    if let Err(e) = job.save() {
        tracing::warn!(%e, "export: the delivered file couldn't be recorded");
    }
    Ok(target)
}
