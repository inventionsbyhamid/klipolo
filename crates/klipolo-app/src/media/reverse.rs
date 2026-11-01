//! Reverse: an imported clip's footage written backward (`klipolo_engine::media::reversed`) to a file of its own beside
//! the project, `<stem> Media/Reversed/<name> reversed.mov` (then `… reversed 2.mov`), for the page to play the clip from
//! (`ReverseClip`). The copy holds the clip's stretch of its file and up to `HANDLE_US` more on each side, so a
//! transition at either cut keeps its room and a trim can still grow the clip a little. It goes with the project, so
//! every export reads it like any file. A copy that stops or fails leaves no file. Blocking: run off the main thread.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

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

/// The first free `<stem> reversed.mov`, `<stem> reversed 2.mov` … in `folder`.
fn free_path(folder: &Path, name: &str) -> PathBuf {
    let stem = Path::new(name)
        .file_stem()
        .map_or_else(|| "clip".to_owned(), |s| s.to_string_lossy().into_owned());
    let base = safe_file_name(&format!("{stem} reversed"));
    (1u32..)
        .map(|n| match n {
            1 => folder.join(format!("{base}.mov")),
            n => folder.join(format!("{base} {n}.mov")),
        })
        .find(|p| !p.exists())
        .expect("some number is free")
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
    std::fs::create_dir_all(&folder).map_err(|error| {
        tracing::warn!(%error, "the reversed copies' folder couldn't be made");
        ReverseFailure::Failed { name: name.clone() }
    })?;
    let out = free_path(&folder, &asset.name);
    let spec = Reverse {
        range,
        video_bps: video_bps(asset.width, asset.height, asset.fps),
        audio_bps: AUDIO_BPS,
        threads: 0,
        buffer_bytes: BUFFER_BYTES,
    };
    match reversed::write(src, &out, &spec, cancel, progress) {
        Ok(info) => Ok(MadeCopy {
            path: out,
            from_us: info.held.start,
            to_us: info.held.end,
        }),
        Err(ReverseError::Stopped) => Err(ReverseFailure::Stopped),
        Err(ReverseError::NoEncoder) => Err(ReverseFailure::NoEncoder),
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

    #[test]
    fn the_copy_takes_the_first_free_name_beside_the_project() {
        let dir = tempfile::tempdir().unwrap();
        let folder = made_folder(&dir.path().join("Trip.klipolo"), "Reversed");
        std::fs::create_dir_all(&folder).unwrap();
        assert_eq!(
            free_path(&folder, "broll.mp4"),
            folder.join("broll reversed.mov")
        );
        std::fs::write(folder.join("broll reversed.mov"), b"").unwrap();
        assert_eq!(
            free_path(&folder, "broll.mp4"),
            folder.join("broll reversed 2.mov")
        );
        assert!(folder.ends_with("Trip Media/Reversed"));
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
}
