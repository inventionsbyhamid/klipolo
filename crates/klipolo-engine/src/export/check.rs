//! Before a run:
//! - every file the export reads must exist. The check mirrors the plans: the main track, overlay tracks unless hidden
//!   (pictures) and tracks and sound layers unless muted (sound), each only when soloed while any is (`Solo`). A disk
//!   waking up is waited for; one that doesn't answer within a minute pauses the job, naming the file.
//! - every part an earlier run finished must still check out (size, CRC, frame count and duration). One that doesn't
//!   is rendered again; nothing else is.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use klipolo_model::project::Project;
use klipolo_model::timeline::{ClipSource, ItemContent, Solo};

use super::job::PartStatus;
use super::protocol::ErrorCode;
use super::segment::verify;
use super::shared::{FrameRate, JobHandle, WorkerError, checksum};
use crate::plan::PlanContext;
use crate::registry::Registry;
use crate::render::pool::{
    Ask, EXPORT_STAT_LIMIT, FileState, file_state, missing, not_answering, unreadable,
};

/// Every file the export's pictures and sound use must be there (a missing one fails the job, naming it; not retried).
pub async fn check_media(ctx: &PlanContext) -> Result<(), WorkerError> {
    check_files(media_files(&ctx.project, &ctx.registry)).await
}

/// `check_media` of `files` (`media_files`): each is asked off the thread and waited for up to `EXPORT_STAT_LIMIT`. A
/// disk that doesn't answer stops the run, naming the file (the job pauses), never a generic stall.
pub async fn check_files(files: Vec<Arc<str>>) -> Result<(), WorkerError> {
    let ask = Ask {
        limit: Some(EXPORT_STAT_LIMIT),
        wake: None,
    };
    for path in files {
        let (code, message) = match file_state(&path, &ask).await {
            FileState::There => continue,
            FileState::Missing => (ErrorCode::MissingMedia, missing(&path)),
            FileState::Unreadable => (ErrorCode::MissingMedia, unreadable(&path)),
            FileState::NotAnswering => (ErrorCode::NotAnswering, not_answering(&path)),
        };
        return Err(WorkerError {
            code,
            message,
            retryable: false,
        });
    }
    Ok(())
}

/// The files `p`'s pictures and sound use in an export (what solo leaves on), resolved with `registry`.
pub fn media_files(p: &Project, registry: &Registry) -> Vec<Arc<str>> {
    let solo = Solo::of(&p.timeline.tracks);
    let mut ids: Vec<&str> = Vec::new();
    for clip in &p.timeline.main {
        match &clip.source {
            ClipSource::Asset { id } => ids.push(id),
            ClipSource::Recording { id } => {
                if let Some(r) = p.recording(id) {
                    ids.extend(r.asset_ids().map(String::as_str));
                }
            }
        }
    }
    for track in &p.timeline.tracks {
        let visible = solo.shows(track);
        let audible = solo.sounds(track);
        for item in &track.items {
            match &item.content {
                ItemContent::Video { asset_id, .. } if visible || audible => ids.push(asset_id),
                ItemContent::Image { asset_id, .. } if visible => ids.push(asset_id),
                ItemContent::Audio {
                    asset_id,
                    muted: false,
                    ..
                } if audible => ids.push(asset_id),
                _ => {}
            }
        }
    }
    ids.into_iter()
        .filter_map(|id| p.asset(id))
        .map(|asset| Arc::from(registry.resolve_path(&asset.path)))
        .collect()
}

/// A finished segment to check again: output, index, file, frames, size and CRC.
type Done = (usize, u32, PathBuf, u64, u64, u32);

/// Checks what earlier runs finished; anything that doesn't check out is marked for rendering again.
pub fn check_parts(job: &JobHandle) -> Result<(), WorkerError> {
    let (done, fps, audio, lost, moved) = job.read(|j| {
        let done: Vec<Done> = j
            .outputs
            .iter()
            .enumerate()
            .filter(|(_, o)| !o.joined)
            .flat_map(|(i, o)| {
                o.segments
                    .iter()
                    .filter(|s| s.status == PartStatus::Done)
                    .map(move |s| (i, s.index, j.path(&s.file), s.frames, s.bytes, s.checksum))
            })
            .collect();
        let audio = (j.audio.status == PartStatus::Done)
            .then(|| (j.path(&j.audio.file), j.audio.bytes, j.audio.checksum));
        // A joined file that is gone before it was recorded as delivered: either it was renamed into place just
        // before a crash (it is at `delivering` and checks out), or it is lost with its segments.
        let mut lost: Vec<usize> = Vec::new();
        let mut moved: Vec<(usize, std::path::PathBuf)> = Vec::new();
        for (i, o) in j.outputs.iter().enumerate() {
            if !o.joined || o.delivered.is_some() || j.path(&o.file).is_file() {
                continue;
            }
            match o.delivering.clone() {
                Some(path) if delivered_ok(&path, j.total_frames, j.fps) => moved.push((i, path)),
                _ => lost.push(i),
            }
        }
        (done, j.fps, audio, lost, moved)
    });
    // A few files at a time: the check reads every byte (CRC), so it is disk-bound.
    let failed: Vec<(usize, u32)> = std::thread::scope(|scope| {
        let workers: Vec<_> = done
            .chunks(done.len().div_ceil(4).max(1))
            .map(|chunk| {
                scope.spawn(move || {
                    chunk
                        .iter()
                        .filter(|(_, _, path, frames, bytes, crc)| {
                            verify(path, *frames, FrameRate::whole(fps), Some((*bytes, *crc)))
                                .is_err()
                        })
                        .map(|(i, index, ..)| (*i, *index))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|w| w.join().unwrap_or_default())
            .collect()
    });
    let audio_bad = audio.is_some_and(|(path, bytes, crc)| {
        std::fs::metadata(&path).map_or(true, |m| m.len() != bytes)
            || checksum(&path).map_or(true, |c| c != crc)
    });
    if failed.is_empty() && !audio_bad && lost.is_empty() && moved.is_empty() {
        return Ok(());
    }
    eprintln!(
        "export resume: {} segment(s) to render again{}{}",
        failed.len(),
        if audio_bad { ", the sound" } else { "" },
        if lost.is_empty() {
            ""
        } else {
            ", a joined file"
        }
    );
    job.update(|j| {
        for (i, index) in &failed {
            if let Some(s) = j.outputs[*i]
                .segments
                .iter_mut()
                .find(|s| s.index == *index)
            {
                s.status = PartStatus::Failed;
            }
        }
        if audio_bad {
            j.audio.status = PartStatus::Pending;
        }
        for &i in &lost {
            let o = &mut j.outputs[i];
            (o.joined, o.delivering) = (false, None);
            for s in o.segments.iter_mut() {
                s.status = PartStatus::Pending;
            }
        }
        for (i, path) in &moved {
            j.outputs[*i].delivered = Some(path.clone());
        }
    })
}

/// A file renamed into place by a run that crashed before recording it: it is this job's video if its frame count
/// and duration match.
fn delivered_ok(path: &Path, frames: u64, fps: u32) -> bool {
    super::segment::probe(path).is_some_and(|(count, seconds)| {
        count == frames && (seconds - frames as f64 / f64::from(fps)).abs() <= 1.5 / f64::from(fps)
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use klipolo_model::layout::{Anchor, BoxN, Layout};
    use klipolo_model::project::{Asset, AssetKind, Project};
    use klipolo_model::timeline::{Item, ItemContent, Track, TrackKind};

    use super::*;

    fn asset(id: &str, kind: AssetKind, path: &Path) -> Asset {
        Asset {
            id: id.into(),
            kind,
            path: path.display().to_string(),
            name: id.into(),
            duration_us: 10_000_000,
            width: 640,
            height: 360,
            fps: 30.0,
            has_audio: true,
            audio_lead_us: None,
            made_with: None,
            rotation: None,
        }
    }

    fn track(id: &str, kind: TrackKind, content: ItemContent) -> Track {
        let mut t = Track::new(kind, id);
        t.id = id.into();
        t.items = vec![Item {
            id: format!("{id}-item"),
            start_us: 0,
            duration_us: 1_000_000,
            attach: None,
            layout: Layout::new(
                Anchor::TopLeft,
                BoxN {
                    x: 0.0,
                    y: 0.0,
                    w: 1.0,
                    h: 1.0,
                },
            ),
            content,
            effects: Vec::new(),
        }];
        t
    }

    /// An export reads only what solo leaves on, the same as the plans it draws and mixes.
    #[tokio::test]
    async fn an_export_checks_only_the_media_solo_leaves_on() {
        let dir = std::env::temp_dir().join(format!("klipolo-solo-check-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let here = dir.join("here.mp4");
        std::fs::write(&here, b"x").unwrap();
        let gone = dir.join("gone.mp4");
        let video = |id: &str| ItemContent::Video {
            asset_id: id.into(),
            source_in_us: 0,
            speed: 1.0,
            volume: 1.0,
            muted: false,
            mirror: false,
            fill: false,
            crop: None,
            tie: None,
        };
        let mut p = Project::new("Solo");
        p.assets = vec![
            asset("here", AssetKind::Video, &here),
            asset("gone", AssetKind::Video, &gone),
            asset("song", AssetKind::Audio, &here),
        ];
        let music = ItemContent::Audio {
            asset_id: "song".into(),
            source_in_us: 0,
            volume: 1.0,
            muted: false,
            fade_in_us: 0,
            fade_out_us: 0,
            effect: None,
            drift_ppm: 0.0,
            centre_from: None,
            tie: None,
            duck: None,
        };
        p.timeline.tracks = vec![
            track("kept", TrackKind::Overlay, video("here")),
            track("other", TrackKind::Overlay, video("gone")),
            track("music", TrackKind::Audio, music),
        ];
        async fn check(p: &Project) -> Result<(), WorkerError> {
            check_media(&PlanContext::new(Arc::new(p.clone()))).await
        }
        assert!(
            check(&p).await.is_err(),
            "everything plays: the missing files count"
        );
        p.timeline.tracks[0].solo = true;
        assert!(
            check(&p).await.is_ok(),
            "soloed: the other tracks' files aren't read"
        );
        p.timeline.tracks[0].solo = false;
        p.timeline.tracks[2].solo = true;
        let err = check(&p).await.unwrap_err();
        assert_eq!(
            err.code,
            ErrorCode::MissingMedia,
            "an audio solo hides no pictures"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
