//! Opening a source's file: its editing copy when that is current, else the original.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use klipolo_model::time::Us;

use super::decoder::{Decoder, request_s};
use super::{
    Ask, Failure, FileState, NAMES, Opened, file_id, file_state, probe_fps, probe_rotation,
};
use crate::plan::SourceRef;
use crate::vendor::cap::rendering::decoder::{DecoderTuning, spawn_decoder_at};

/// Opens `s.path`. An editing copy that can't be opened (purged, damaged), or that is no longer the copy of its source
/// as the file is now (overwritten since: checked at every open against the copy's record), falls
/// back to the original, whose rate and turn are probed (the plan carries the copy's); a failure is the original's. A
/// copy whose original's disk doesn't answer is read meanwhile, and checked once it answers (`Decoder::unchecked`).
pub(super) async fn open(
    s: &SourceRef,
    lane: u8,
    tuning: DecoderTuning,
    force_ffmpeg: bool,
    ask: &Ask,
) -> Opened {
    // Whether `s.path` is read, and if so unchecked; None: a copy that isn't its original's any more.
    let read = if s.is_copy() {
        match copy_check(&s.path, &s.original, ask).await {
            CopyCheck::Current => Some(false),
            CopyCheck::Unchecked => Some(true),
            // A copy whose disk hasn't answered yet is asked again, never given up for the original.
            CopyCheck::Silent => return Err(Failure::NotAnswering),
            CopyCheck::Stale => None,
        }
    } else {
        Some(false)
    };
    let opened = match read {
        Some(unchecked) => open_file(
            &s.path,
            s.fps,
            s.rotation,
            lane,
            tuning,
            force_ffmpeg,
            ask,
            s.time_us,
        )
        .await
        .map(|decoder| Decoder {
            unchecked,
            ..decoder
        }),
        None => Err(Failure::Unreadable),
    };
    match opened {
        // A copy whose disk hasn't answered yet is asked again, never given up for the original.
        Err(failure) if s.is_copy() && failure != Failure::NotAnswering => {
            open_file(
                &s.original,
                0.0,
                None,
                lane,
                tuning,
                force_ffmpeg,
                ask,
                s.time_us,
            )
            .await
        }
        opened => opened,
    }
}

/// Whether an editing copy may be read in place of its original.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CopyCheck {
    /// Made from the original as the file is now (the record beside it says so).
    Current,
    /// Not the copy of the original as it is now (overwritten since, gone, or no record): the original is read.
    Stale,
    /// The original's disk hasn't answered: the copy is read meanwhile (made from the original as it last was) and
    /// checked once it answers, when the caller's redraw signal fires.
    Unchecked,
    /// The copy's own disk hasn't answered (or its record didn't read in time): asked again, never given up for the
    /// original.
    Silent,
}

/// Whether the copy at `copy` was made from `original` as the file is now, at decoder opens and,
/// for an unchecked copy, each frame until its original answers. Nothing here runs on the engine's task (the
/// original's stat once did, and a dropped volume holding an original froze the preview for every source with a
/// copy): the original's stat and the copy's are `file_id`'s (bounded by `ask`, one per path in flight, the redraw
/// signal registered when one is given up on), and the copy's record, a small file beside it, is read on the blocking
/// pool within the same limit.
pub(super) async fn copy_check(copy: &Arc<str>, original: &Arc<str>, ask: &Ask) -> CopyCheck {
    use crate::media::proxy::job::CopyRecord;
    let now = match file_id(original, ask).await {
        (FileState::NotAnswering, _) => return CopyCheck::Unchecked,
        (_, now) => now,
    };
    match file_state(copy, ask).await {
        FileState::There => {}
        FileState::NotAnswering => return CopyCheck::Silent,
        FileState::Missing | FileState::Unreadable => return CopyCheck::Stale,
    }
    let (copy, original) = (copy.clone(), original.clone());
    let read = tokio::task::spawn_blocking(move || {
        let copy = Path::new(&*copy);
        CopyRecord::read(copy).is_some_and(|record| record.current(&original, copy, now, None))
    });
    let current = match ask.limit {
        Some(limit) => tokio::time::timeout(limit, read).await.ok(),
        None => Some(read.await),
    };
    match current {
        Some(Ok(true)) => CopyCheck::Current,
        Some(Ok(false) | Err(_)) => CopyCheck::Stale,
        None => CopyCheck::Silent,
    }
}

/// Opens `file` for a first frame at `at_us` (an FFmpeg decoder starts at the keyframe before it).
#[allow(clippy::too_many_arguments)]
pub(super) async fn open_file(
    file: &Arc<str>,
    fps: f64,
    rotation: Option<u16>,
    lane: u8,
    tuning: DecoderTuning,
    force_ffmpeg: bool,
    ask: &Ask,
    at_us: Us,
) -> Result<Decoder, Failure> {
    match file_state(file, ask).await {
        FileState::There => {}
        FileState::Missing => return Err(Failure::Missing),
        FileState::Unreadable => return Err(Failure::Unreadable),
        FileState::NotAnswering => return Err(Failure::NotAnswering),
    }
    let path = PathBuf::from(&**file);
    // Exact (30000/1001 for 29.97): frames are numbered on the source's own grid, which a whole-number rate drifts
    // off a little more every frame.
    let fps = if fps.is_finite() && fps >= 1.0 {
        fps
    } else {
        probe_fps(&path).max(1.0)
    };
    // Imports store both, so an open doesn't probe the container; older assets (and recordings) are probed here.
    let rotation = rotation.unwrap_or_else(|| probe_rotation(&path));
    let at = request_s(at_us, fps);
    spawn_decoder_at(
        NAMES[lane as usize],
        path,
        fps,
        0.0,
        force_ffmpeg,
        tuning,
        at,
    )
    .await
    .map(|handle| Decoder {
        handle,
        rotation,
        fps,
        file: file.clone(),
        unchecked: false,
        reopened_at: None,
    })
    .map_err(|_| Failure::Unreadable)
}
