//! Reverse: an imported clip's footage written backward (`klipolo_engine::media::reversed`) to a file of its own beside
//! the project, `<stem> Media/Reversed/<name> reversed.mov` (then `… reversed 2.mov`), for the page to play the clip from
//! (`ReverseClip`). The copy holds the clip's stretch of its file and up to `HANDLE_US` more on each side, so a
//! transition at either cut keeps its room and a trim can still grow the clip a little. It goes with the project, so
//! every export reads it like any file. Each copy is written to a hidden file of its own and then takes the first free
//! name (`place_copy`), so two made at once never meet. A copy that stops or fails leaves no file, and what a crash
//! left is swept away by a later copy (`sweep`). Blocking: run off the main thread.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};

use klipolo_engine::media::reversed::{self, Reverse, ReverseError};
use klipolo_model::project::{AssetKind, Project};
use klipolo_model::time::{RangeUs, Us};
use klipolo_model::timeline::ClipSource;
use klipolo_platform::paths::safe_file_name;

use super::media_folder::made_folder;

/// Footage kept past each end of the clip, where its file has it: room for the longest transition's half.
pub const HANDLE_US: Us = 2_500_000;
/// The picture's bits per pixel: about 19 Mbit/s for 1080p at 30 fps, a quality that holds up in every export.
const BITS_PER_PIXEL: f64 = 0.3;
/// The bitrate's floor and ceiling.
const MIN_BPS: f64 = 4_000_000.0;
const MAX_BPS: f64 = 100_000_000.0;
/// The sound's bitrate.
const AUDIO_BPS: u32 = 192_000;
/// Decoded pictures held at once while a window is reversed.
const BUFFER_BYTES: usize = 192 << 20;

/// Why a clip wasn't reversed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReverseFailure {
    /// No such clip, or it plays no imported video (the model refuses the rest: a freeze, a recording).
    NotImported,
    /// Its file isn't there.
    Missing { name: String },
    /// This computer has no encoder for the copy.
    NoEncoder,
    /// An HDR clip where this computer can't write HDR video: never flattened to SDR.
    NoHdrEncoder,
    /// Stopped on request.
    Stopped,
    /// Reading, writing or the disk failed (logged; the message names no folder).
    Failed { name: String },
}

/// What was made: the copy's file, and the stretch of the clip's file it holds.
#[derive(Debug, Clone, PartialEq)]
pub struct MadeCopy {
    pub path: PathBuf,
    pub from_us: Us,
    pub to_us: Us,
}

/// The stretch of a file `file_us` long a copy for a clip playing `[source_in, source_out)` of it holds.
pub fn copy_range(source_in: Us, source_out: Us, file_us: Us) -> RangeUs {
    RangeUs {
        start: source_in.saturating_sub(HANDLE_US).max(0),
        end: source_out
            .saturating_add(HANDLE_US)
            .min(file_us.max(source_out)),
    }
}

/// The picture's bitrate for a source of `width` × `height` at `fps`.
pub fn video_bps(width: u32, height: u32, fps: f64) -> u32 {
    let fps = if fps > 0.0 { fps } else { 30.0 };
    (f64::from(width) * f64::from(height) * fps * BITS_PER_PIXEL).clamp(MIN_BPS, MAX_BPS) as u32
}

/// The most names tried for one copy (`<stem> reversed.mov` to `<stem> reversed 9999.mov`).
const MAX_NUMBER: u32 = 9999;
/// A hidden file this old is left over from a crash: no copy takes this long.
const STALE: Duration = Duration::from_secs(6 * 60 * 60);
/// How a copy being made is named in its folder until it is whole: `.reversing-<random>.mov`.
const JOB_PREFIX: &str = ".reversing-";

/// A hidden file in `folder` for one copy to be written to, its own: two copies made at once never share one.
fn job_file(folder: &Path) -> PathBuf {
    folder.join(format!("{JOB_PREFIX}{}.mov", uuid::Uuid::new_v4().simple()))
}

/// Puts the whole copy `job` in `folder` under the first free `<stem> reversed.mov`, `<stem> reversed 2.mov` … (`name`
/// is its source's file name), and returns that path. A name is taken by a hard link, which fails when the name
/// exists, so no file is ever replaced, even by a copy finished at the same moment; then `job` goes.
fn place_copy(job: &Path, folder: &Path, name: &str) -> io::Result<PathBuf> {
    let stem = Path::new(name)
        .file_stem()
        .map_or_else(|| "clip".to_owned(), |s| s.to_string_lossy().into_owned());
    let base = safe_file_name(&format!("{stem} reversed"));
    for n in 1..=MAX_NUMBER {
        let path = match n {
            1 => folder.join(format!("{base}.mov")),
            n => folder.join(format!("{base} {n}.mov")),
        };
        match fs::hard_link(job, &path) {
            Ok(()) => {
                let _ = fs::remove_file(job);
                return Ok(path);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "every name is taken",
    ))
}

/// Removes what a crash left in `folder` while making a copy (`job_file`'s and the engine's hidden partials) once it is
/// older than `STALE`. Anything else stays, and what can't be read or removed is left for next time.
fn sweep(folder: &Path) {
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let ours = name.starts_with(JOB_PREFIX)
            || (name.starts_with('.') && name.ends_with(reversed::PARTIAL_SUFFIX));
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age > STALE);
        if ours && old {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Deletes reversed copy `path` that the page didn't use (its clip went or changed, the project changed, or the edit
/// was refused), and says whether it did. Only a `.mov` in a `<stem> Media/Reversed` folder goes, and never one that
/// `project` (the open one) plays.
pub fn discard_unused(project: Option<&Project>, path: &Path) -> bool {
    let folder = path.parent();
    let in_reversed = folder.is_some_and(|f| f.file_name().is_some_and(|n| n == "Reversed"))
        && folder
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .is_some_and(|n| n.to_string_lossy().ends_with(" Media"));
    let movie = path.extension().is_some_and(|e| e == "mov");
    let used = project.is_some_and(|p| p.assets.iter().any(|a| Path::new(&a.path) == path));
    in_reversed && movie && !used && fs::remove_file(path).is_ok()
}

/// Writes clip `clip_id` of `project` (saved as `project_file`) backward, as the module says, telling `progress` 0 to 1.
pub fn make(
    project: &Project,
    project_file: &Path,
    clip_id: &str,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f64),
) -> Result<MadeCopy, ReverseFailure> {
    let clip = (project.timeline.main.iter())
        .find(|c| c.id == clip_id)
        .ok_or(ReverseFailure::NotImported)?;
    let ClipSource::Asset { id } = &clip.source else {
        return Err(ReverseFailure::NotImported);
    };
    let asset = project
        .asset(id)
        .filter(|a| a.kind == AssetKind::Video)
        .ok_or(ReverseFailure::NotImported)?;
    let name = asset.name.clone();
    let src = Path::new(&asset.path);
    if !src.is_file() {
        return Err(ReverseFailure::Missing { name });
    }
    let range = copy_range(clip.source_in_us, clip.source_out_us, asset.duration_us);
    let folder = made_folder(project_file, "Reversed");
    fs::create_dir_all(&folder).map_err(|error| {
        tracing::warn!(%error, "the reversed copies' folder couldn't be made");
        ReverseFailure::Failed { name: name.clone() }
    })?;
    sweep(&folder);
    let job = job_file(&folder);
    let spec = Reverse {
        range,
        video_bps: video_bps(asset.width, asset.height, asset.fps),
        audio_bps: AUDIO_BPS,
        threads: 0,
        buffer_bytes: BUFFER_BYTES,
    };
    match reversed::write(src, &job, &spec, cancel, progress) {
        Ok(info) => match place_copy(&job, &folder, &asset.name) {
            Ok(path) => Ok(MadeCopy {
                path,
                from_us: info.held.start,
                to_us: info.held.end,
            }),
            Err(error) => {
                tracing::warn!(%error, file = %name, "a reversed copy couldn't be named");
                let _ = fs::remove_file(&job);
                Err(ReverseFailure::Failed { name })
            }
        },
        Err(ReverseError::Stopped) => Err(ReverseFailure::Stopped),
        Err(ReverseError::NoEncoder) => Err(ReverseFailure::NoEncoder),
        Err(ReverseError::NoHdrEncoder) => Err(ReverseFailure::NoHdrEncoder),
        Err(ReverseError::NoVideo) => Err(ReverseFailure::NotImported),
        Err(ReverseError::Failed(error)) => {
            tracing::warn!(%error, file = %name, "a clip couldn't be reversed");
            Err(ReverseFailure::Failed { name })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use klipolo_model::project::{Asset, AssetKind, Project};
    use klipolo_model::timeline::{Clip, ClipSource};

    use super::*;

    const S: Us = 1_000_000;

    fn project(path: &str) -> Project {
        let mut p = Project::new("Trip");
        p.assets.push(Asset {
            id: "broll".into(),
            kind: AssetKind::Video,
            path: path.into(),
            name: "broll.mp4".into(),
            duration_us: 10 * S,
            width: 1920,
            height: 1080,
            fps: 30.0,
            rotation: Some(0),
            has_audio: true,
            audio_lead_us: Some(0),
            made_with: None,
        });
        let mut b = Clip::new(ClipSource::Asset { id: "broll".into() }, 3 * S, 5 * S);
        b.id = "b".into();
        p.timeline.main.push(b);
        p
    }

    #[test]
    fn a_copy_holds_the_clip_and_room_for_a_transition_on_each_side_where_the_file_has_it() {
        assert_eq!(
            copy_range(3 * S, 5 * S, 10 * S),
            RangeUs {
                start: S / 2,
                end: 7 * S + S / 2
            }
        );
        assert_eq!(
            copy_range(S, 9 * S, 10 * S),
            RangeUs {
                start: 0,
                end: 10 * S
            }
        );
    }

    #[test]
    fn the_bitrate_follows_the_picture_within_bounds() {
        assert_eq!(video_bps(1920, 1080, 30.0), 18_662_400);
        assert_eq!(video_bps(320, 180, 30.0), 4_000_000);
        assert_eq!(video_bps(7680, 4320, 60.0), 100_000_000);
        // A file that says no rate counts as 30 fps.
        assert_eq!(video_bps(1920, 1080, 0.0), 18_662_400);
    }

    /// Two copies of one file made at once (two halves of a split clip) each write a hidden file of their own, then take
    /// the first free name beside the project: neither overwrites the other, nor a file already there.
    #[test]
    fn two_copies_of_one_file_take_two_names_and_never_replace_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let folder = made_folder(&dir.path().join("Trip.klipolo"), "Reversed");
        std::fs::create_dir_all(&folder).unwrap();
        assert!(folder.ends_with("Trip Media/Reversed"));
        std::fs::write(folder.join("broll reversed.mov"), b"kept").unwrap();
        let (a, b) = (job_file(&folder), job_file(&folder));
        assert_ne!(a, b);
        assert!(a.file_name().unwrap().to_string_lossy().starts_with('.'));
        std::fs::write(&a, b"a").unwrap();
        std::fs::write(&b, b"b").unwrap();
        let placed_a = place_copy(&a, &folder, "broll.mp4").unwrap();
        let placed_b = place_copy(&b, &folder, "broll.mp4").unwrap();
        assert_eq!(placed_a, folder.join("broll reversed 2.mov"));
        assert_eq!(placed_b, folder.join("broll reversed 3.mov"));
        let read = |p: &Path| std::fs::read(p).unwrap();
        assert_eq!(read(&folder.join("broll reversed.mov")), b"kept");
        assert_eq!(
            (read(&placed_a), read(&placed_b)),
            (b"a".to_vec(), b"b".to_vec())
        );
        assert!(!a.exists() && !b.exists());
    }

    /// What a crash left while making a copy goes once it is stale; a fresh one (a copy being made now) and the
    /// person's own files stay.
    #[test]
    fn a_crash_s_leftovers_are_swept_once_stale() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path();
        let stale = SystemTime::now() - Duration::from_secs(7 * 60 * 60);
        let make = |name: &str, old: bool| {
            let path = folder.join(name);
            fs::write(&path, b"x").unwrap();
            if old {
                fs::File::options()
                    .write(true)
                    .open(&path)
                    .unwrap()
                    .set_modified(stale)
                    .unwrap();
            }
            path
        };
        let old_job = make(".reversing-1.mov", true);
        let old_partial = make("..reversing-1.mov.2.partial.mov", true);
        let new_job = make(".reversing-3.mov", false);
        let own = make("broll reversed.mov", true);
        sweep(folder);
        assert!(!old_job.exists() && !old_partial.exists());
        assert!(new_job.exists() && own.exists());
    }

    #[test]
    fn only_an_imported_clip_whose_file_is_there_is_reversed() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Trip.klipolo");
        let stop = AtomicBool::new(false);
        let gone = project("/nowhere/broll.mp4");
        assert_eq!(
            make(&gone, &file, "b", &stop, &mut |_| {}),
            Err(ReverseFailure::Missing {
                name: "broll.mp4".into()
            })
        );
        assert_eq!(
            make(&gone, &file, "nope", &stop, &mut |_| {}),
            Err(ReverseFailure::NotImported)
        );
        // Nothing was written for either.
        assert!(!made_folder(&file, "Reversed").exists());
    }

    /// A copy the page didn't use (its clip went, the project changed, the edit was refused) is deleted, but only a
    /// reversed copy, and never one the open project plays.
    #[test]
    fn only_an_unused_reversed_copy_is_thrown_away() {
        let dir = tempfile::tempdir().unwrap();
        let folder = made_folder(&dir.path().join("Trip.klipolo"), "Reversed");
        fs::create_dir_all(&folder).unwrap();
        let unused = folder.join("broll reversed.mov");
        let used = folder.join("broll reversed 2.mov");
        let elsewhere = dir.path().join("broll reversed.mov");
        for f in [&unused, &used, &elsewhere] {
            fs::write(f, b"x").unwrap();
        }
        let mut p = project("/media/broll.mp4");
        let mut copy = p.assets[0].clone();
        copy.id = "copy".into();
        copy.path = used.to_string_lossy().into_owned();
        p.assets.push(copy);
        assert!(discard_unused(Some(&p), &unused));
        assert!(!unused.exists());
        assert!(!discard_unused(Some(&p), &used));
        assert!(!discard_unused(None, &elsewhere));
        assert!(used.exists() && elsewhere.exists());
    }
}
