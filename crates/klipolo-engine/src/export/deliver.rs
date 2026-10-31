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

/// Output `index`'s joined file moved into place; the failed outcome when it can't be. A name taken by the time it
/// moves (another app can take it after the check) gets the next free one: the move itself never replaces a file
/// (`place_without_overwrite`).
fn move_into_place(
    job: &mut ExportJob,
    index: usize,
    finals: &[PathBuf],
    emit: &(dyn Fn(ExportEvent) + Send + Sync),
) -> Result<PathBuf, Outcome> {
    let o = &job.outputs[index];
    let joined = job.path(&o.file);
    let ext = o.settings.extension();
    // Linked into place before a crash cut the move short: it is there already.
    if let Some(at) = o.delivering.clone()
        && same_file(&joined, &at)
    {
        let _ = std::fs::remove_file(&joined);
        return Ok(recorded(job, index, at));
    }
    let mut taken: HashSet<PathBuf> = finals.iter().cloned().collect();
    let mut target = o.final_path.clone();
    let dir = target.parent().map(Path::to_path_buf).unwrap_or_default();
    let base = target
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    if target.exists() || taken.contains(&target) {
        target = free_path(&dir, &base, ext, &taken);
    }
    let cant =
        || "The finished video couldn't be moved into your Exports folder. Try again.".to_string();
    // A few names at most: each one taken was taken by someone between the check and the move.
    for _ in 0..100 {
        job.outputs[index].delivering = Some(target.clone());
        if let Err(e) = job.save() {
            // Without that record a crash after the move couldn't be told apart: don't move it.
            tracing::warn!(%e, "export: the file's destination couldn't be recorded");
            return Err(failed(cant().into(), emit));
        }
        match place_without_overwrite(&joined, &target) {
            Ok(()) => return Ok(recorded(job, index, target)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                taken.insert(target);
                target = free_path(&dir, &base, ext, &taken);
            }
            Err(e) => {
                tracing::warn!(%e, "export: the finished file couldn't be moved into place");
                job.state = JobState::Failed;
                job.message = Some(cant().into());
                let _ = job.save();
                return Err(failed(cant().into(), emit));
            }
        }
    }
    Err(failed(cant().into(), emit))
}

/// Output `index` is in place at `target`: recorded, and returned.
fn recorded(job: &mut ExportJob, index: usize, target: PathBuf) -> PathBuf {
    job.outputs[index].delivered = Some(target.clone());
    if let Err(e) = job.save() {
        tracing::warn!(%e, "export: the delivered file couldn't be recorded");
    }
    target
}

/// Moves `from` to `to` without ever replacing a file at `to`, even one another app puts there meanwhile: a hard link
/// fails with `AlreadyExists` when the name is taken (one step, unlike a check and then a rename), then `from` goes.
/// On a volume without hard links (FAT, exFAT, some network shares) a placeholder made with `create_new` claims the
/// name the same way, and the rename then replaces only that placeholder.
pub(crate) fn place_without_overwrite(from: &Path, to: &Path) -> std::io::Result<()> {
    match std::fs::hard_link(from, to) {
        Ok(()) => {
            // Both names are the file now; the job's goes with its folder if this fails.
            if let Err(e) = std::fs::remove_file(from) {
                tracing::warn!(%e, "export: the joined file stayed in the job folder");
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(e),
        Err(_) => {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(to)?;
            std::fs::rename(from, to).inspect_err(|_| {
                let _ = std::fs::remove_file(to);
            })
        }
    }
}

/// Whether `a` and `b` are one file (two links to it). Unknown elsewhere than on Unix: false.
fn same_file(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (std::fs::metadata(a), std::fs::metadata(b)) {
            (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (a, b);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placing_a_file_never_replaces_one_already_there() {
        let dir = tempfile::tempdir().unwrap();
        let joined = dir.path().join("joined.mp4");
        let theirs = dir.path().join("Demo 16x9.mp4");
        std::fs::write(&joined, b"ours").unwrap();
        // Another app took the name after the check.
        std::fs::write(&theirs, b"theirs").unwrap();
        let refused = place_without_overwrite(&joined, &theirs).unwrap_err();
        assert_eq!(refused.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&theirs).unwrap(), b"theirs", "never replaced");
        assert_eq!(
            std::fs::read(&joined).unwrap(),
            b"ours",
            "and ours is still there to move"
        );
        let free = dir.path().join("Demo 16x9 2.mp4");
        place_without_overwrite(&joined, &free).unwrap();
        assert_eq!(std::fs::read(&free).unwrap(), b"ours");
        assert!(!joined.exists(), "moved, not copied");
    }

    #[test]
    fn a_file_linked_into_place_is_the_same_file() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        std::fs::write(&a, b"x").unwrap();
        std::fs::hard_link(&a, &b).unwrap();
        assert!(same_file(&a, &b));
        std::fs::write(dir.path().join("c"), b"x").unwrap();
        assert!(!same_file(&a, &dir.path().join("c")));
    }
}
