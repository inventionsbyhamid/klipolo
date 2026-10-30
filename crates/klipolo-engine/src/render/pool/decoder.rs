//! One open decoder of the pool, where its frames are asked for, and how a frame its decoder stalls on is recovered
//! the file is asked about off the engine's task, then the reader is opened again once, at the
//! keyframe before the frame (the stuck one is let go, never cancelled from this thread: a reader's own queues race a
//! cancel), then the frame is decoded in software from that keyframe; only then is it "couldn't be read".
//! The preview runs the same steps on its fixed deadline, so one passing stall doesn't leave a file on the CPU; its
//! reopen comes back after a quiet spell, and a stalled still that a newer seek replaced isn't waited for again
//!. Asking about the file and opening a decoder keep the export heartbeat's beat.

use std::sync::Arc;
use std::time::{Duration, Instant};

use klipolo_model::time::Us;

use super::failure::Failure;
use super::open::open_file;
use super::stat::{Ask, FileState, file_state};
use crate::decode_wait::{DecodeBeat, DecodeWait, Waited};
use crate::vendor::cap::rendering::decoder::{
    AsyncVideoDecoderHandle, DecodedFrame, DecoderTuning, Superseded,
};

/// An open decoder, the clockwise degrees that turn its frames upright, and its source's exact frame rate.
#[derive(Clone)]
pub(super) struct Decoder {
    pub(super) handle: AsyncVideoDecoderHandle,
    pub(super) rotation: u16,
    pub(super) fps: f64,
    /// The file it reads: the plan's `path`, or its `original` when the editing copy couldn't be opened.
    pub(super) file: Arc<str>,
    /// An editing copy opened while its original's disk didn't answer, so not yet checked against the original as it is
    /// now: each frame checks it (`copy_check`) until the original answers.
    pub(super) unchecked: bool,
    /// When it was opened again after its reader stalled. A stall on it goes to software, unless the
    /// recovery allows another reopen after a quiet spell (`Recovery::reopen_quiet`, the preview).
    pub(super) reopened_at: Option<Instant>,
}

/// What a request needs to recover a stalled frame: how its slot's decoders are opened, the disk's limit, and the
/// export heartbeat's flag.
pub(super) struct Recovery {
    pub(super) ask: Ask,
    pub(super) beat: Arc<DecodeBeat>,
    pub(super) lane: u8,
    pub(super) tuning: DecoderTuning,
    pub(super) force_ffmpeg: bool,
    /// How long a reopened decoder must go without a stall before a stall may reopen it again (the preview: one
    /// passing stall a session mustn't put a file on the CPU); None: once (export, per attempt).
    pub(super) reopen_quiet: Option<Duration>,
}

/// A frame read: the frame and its turn (None: a newer seek replaced it while its reader stalled, so it was skipped), or
/// what its file is now; and the decoder that serves the slot from now on when the stalled one was opened again.
pub(super) struct Read {
    pub(super) frame: Result<Option<(DecodedFrame, u16)>, FileState>,
    pub(super) reopened: Option<Decoder>,
}

/// A frame that waited this long on a decoder that kept making progress is logged (export).
const LOGGED_WAIT: Duration = Duration::from_secs(1);

/// The preview's reopened reader may be opened again after this long without a stall.
const PREVIEW_REOPEN_QUIET: Duration = Duration::from_secs(120);

/// `PREVIEW_REOPEN_QUIET`, or a test's (`decode_wait::hooks::set_reopen_quiet`) in test and trial builds.
pub(super) fn reopen_quiet() -> Duration {
    #[cfg(any(test, feature = "trial"))]
    if let Some(quiet) = crate::decode_wait::hooks::reopen_quiet() {
        return quiet;
    }
    PREVIEW_REOPEN_QUIET
}

impl Decoder {
    /// The frame shown at source time `time_us`.
    pub(super) async fn frame_at(&self, time_us: Us) -> Option<DecodedFrame> {
        self.handle.get_frame(request_s(time_us, self.fps)).await
    }

    /// The frame shown at `time_us`, answered early with the nearest frame of its GOP read so far once `superseded`
    /// says a newer seek replaced it and that GOP lasts at most a second. A request the decoder drops is read
    /// in software once the file is heard to be there; one it stalls on is recovered (the module's notes).
    pub(super) async fn read(
        &self,
        time_us: Us,
        superseded: Option<Superseded>,
        how: &Recovery,
    ) -> Read {
        let started = Instant::now();
        let t = request_s(time_us, self.fps);
        let asked = superseded.clone();
        let frame = match self.handle.read_frame(t, asked, Some(&how.beat)).await {
            Waited::Got(frame) => {
                let waited = started.elapsed();
                if waited >= LOGGED_WAIT && matches!(how.tuning.wait, DecodeWait::Progress { .. }) {
                    tracing::info!(
                        file = %crate::log_name(&*self.file),
                        frame = frame_shown(time_us, self.fps),
                        waited_ms = waited.as_millis() as u64,
                        "a frame waited for its decoder, which kept making progress"
                    );
                }
                Ok(Some((frame, self.rotation)))
            }
            Waited::Dropped => match how.beat.alive_while(file_state(&self.file, &how.ask)).await {
                FileState::There => self.software(time_us, how).await.map(Some),
                state => Err(state),
            },
            Waited::Stalled(waited) => {
                return self.recover(time_us, waited, superseded, how).await;
            }
        };
        Read {
            frame,
            reopened: None,
        }
    }

    /// The decoder stalled on the frame at `time_us`: once the file is heard to be there, a decoder opened afresh at the
    /// keyframe before it (unless this one is that already and its quiet spell hasn't passed), then software, then
    /// "couldn't be read". A frame `superseded` says a newer seek replaced is skipped rather than waited for again.
    async fn recover(
        &self,
        time_us: Us,
        waited: Duration,
        superseded: Option<Superseded>,
        how: &Recovery,
    ) -> Read {
        let (file, frame_no) = (crate::log_name(&*self.file), frame_shown(time_us, self.fps));
        let stale = || superseded.as_ref().is_some_and(|replaced| replaced());
        match how.beat.alive_while(file_state(&self.file, &how.ask)).await {
            FileState::There => {}
            state => {
                return Read {
                    frame: Err(state),
                    reopened: None,
                };
            }
        }
        let may_reopen = match self.reopened_at {
            None => true,
            Some(at) => how.reopen_quiet.is_some_and(|quiet| at.elapsed() >= quiet),
        };
        let reopened = if !may_reopen {
            None
        } else {
            tracing::info!(
                %file,
                frame = frame_no,
                waited_ms = waited.as_millis() as u64,
                "the video reader stalled; opening it again at the keyframe before the frame"
            );
            let opened = how.beat.alive_while(open_file(
                &self.file,
                self.fps,
                Some(self.rotation),
                how.lane,
                how.tuning,
                how.force_ffmpeg,
                &how.ask,
                time_us,
            ));
            match opened.await {
                Ok(new) => Some(Decoder {
                    reopened_at: Some(Instant::now()),
                    unchecked: self.unchecked,
                    ..new
                }),
                Err(failure) => {
                    return Read {
                        frame: Err(failed(failure)),
                        reopened: None,
                    };
                }
            }
        };
        // A scrub moved on while the reader stalled: the reader is replaced for the frames to come, but this frame isn't
        // waited for again (nor decoded in software).
        if stale() {
            return Read {
                frame: Ok(None),
                reopened,
            };
        }
        if let Some(new) = &reopened {
            let t = request_s(time_us, self.fps);
            match new
                .handle
                .read_frame(t, superseded.clone(), Some(&how.beat))
                .await
            {
                Waited::Got(frame) => {
                    return Read {
                        frame: Ok(Some((frame, new.rotation))),
                        reopened,
                    };
                }
                Waited::Dropped | Waited::Stalled(_) => {}
            }
            if stale() {
                return Read {
                    frame: Ok(None),
                    reopened,
                };
            }
        }
        let decoder = reopened.as_ref().unwrap_or(self);
        let frame = decoder.software(time_us, how).await.map(Some);
        if frame.is_err() {
            tracing::warn!(
                %file,
                frame = frame_no,
                "no decoder of this file made progress on the frame: it couldn't be read"
            );
        }
        Read { frame, reopened }
    }

    /// The frame at `time_us` read in software, after the reader stopped returning frames and its file was heard to be
    /// there: the handle's fallback is started here (once), never by the handle, which would ask about the file on the
    /// engine's task. It starts at the frame, seeking to the keyframe before it.
    /// `There` (couldn't be read) when there is no fallback left to start, or it couldn't read the frame either.
    async fn software(
        &self,
        time_us: Us,
        how: &Recovery,
    ) -> Result<(DecodedFrame, u16), FileState> {
        let t = request_s(time_us, self.fps);
        if !self.handle.can_fall_back() || !self.handle.start_fallback(t, Some(&how.beat)).await {
            return Err(FileState::There);
        }
        tracing::info!(
            file = %crate::log_name(&*self.file),
            frame = frame_shown(time_us, self.fps),
            "decoding in software from the keyframe before the frame"
        );
        self.handle
            .read_frame(t, None, Some(&how.beat))
            .await
            .got()
            .map(|frame| (frame, self.rotation))
            .ok_or(FileState::There)
    }
}

/// What a failed open means for the frame: what its file is now.
fn failed(failure: Failure) -> FileState {
    match failure {
        Failure::Missing => FileState::Missing,
        Failure::Unreadable => FileState::Unreadable,
        Failure::NotAnswering => FileState::NotAnswering,
    }
}

/// Where to ask a decoder (which numbers frames `floor(t × fps)` on the source's own grid) for the frame shown at
/// `time_us`: the middle of that frame, so the seconds' f32 rounding can't move it across an edge.
pub(super) fn request_s(time_us: Us, fps: f64) -> f32 {
    ((frame_shown(time_us, fps) as f64 + 0.5) / fps) as f32
}

/// The frame of a file at `fps` that the preview (and export, through the same pool) shows at `time_us`: the one whose
/// span holds it. A time up to 1 µs before a frame's start counts as that frame: the timeline rounds frame starts to
/// whole µs.
pub fn frame_shown(time_us: Us, fps: f64) -> i64 {
    ((time_us.max(0) + 1) as f64 * fps / 1e6).floor() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::frame_start_us;
    use crate::vendor::cap::rendering::decoder::time_to_frame;

    /// The stalled export (1080p30 from 23.976 fps): output frames 263 and 1103 asked for the middles of film frames
    /// 210 and 881 (8.7796 s and 36.7659 s, as the log shows), and those times map back to those frames. The stalls
    /// were the reader's, not the mapping's.
    #[test]
    fn the_owners_stalled_requests_map_to_film_frames_210_and_881() {
        let film = 24_000.0 / 1_001.0;
        for (output, frame, seconds) in [(263, 210, 8.7796), (1103, 881, 36.7659)] {
            let at = frame_start_us(output, 30);
            assert_eq!(frame_shown(at, film), frame);
            let t = request_s(at, film);
            assert!((f64::from(t) - seconds).abs() < 1e-4, "{t}");
            assert_eq!(i64::from(time_to_frame(t, film)), frame);
        }
    }
}
