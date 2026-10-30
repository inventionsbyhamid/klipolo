//! Where a build's decoded blocks go: the cache file (through a one-chunk buffer, each chunk marked readable once
//! written), the loudness energies and the peaks; and when a run should end early.

use klipolo_platform::file::PositionedIo;
use std::fs::File;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use super::energy::EnergyWriter;
use super::handle::CHUNK;
use super::peaks::Peaks;
use super::{BuildError, HEADER, Pcm};
use crate::audio::decode::{self, DecodeError};
use crate::audio::mix::RATE;

/// Frames converted before each write: one chunk, whose readiness follows the write.
const FLUSH_FRAMES: usize = CHUNK as usize;
/// A playhead this far past the frame being decoded retargets the build (a nearer one is reached anyway).
pub(super) const RETARGET_AHEAD: u64 = 10 * RATE as u64;

/// A running build, as the store sees it.
#[derive(Default)]
pub(super) struct Progress {
    /// The frame the current run writes next.
    pub(super) at: AtomicU64,
    /// A playhead to decode from next (frame + 1; 0: none).
    pub(super) retarget: AtomicU64,
    /// A close cancelled it (`PcmStore::close_project`). A caller's own flag (`build_cancellable`) is checked beside
    /// it, at every block.
    pub(super) cancel: AtomicBool,
    /// Callers waiting on this build (`PcmStore::build_cancellable` joins it). While any are, a worker build runs at
    /// user-initiated QoS (`Progress::follow_urgency`).
    pub(super) urgent: AtomicU32,
    /// The store's build thread runs it: that thread follows `urgent`.
    pub(super) worker: bool,
    /// The build thread is raised now (only that thread changes it).
    pub(super) raised: AtomicBool,
    /// Tests: the QoS classes the build thread switched to for this build.
    #[cfg(test)]
    pub(super) qos: std::sync::Mutex<Vec<crate::qos::Qos>>,
}

/// Where decoded blocks go: the cache file (through a one-chunk buffer), the loudness energies and the peaks.
pub(super) struct Sink<'a> {
    file: &'a File,
    pub(super) pcm: Option<&'a Pcm>,
    pub(super) progress: &'a Progress,
    /// The caller's own cancel flag (`build_cancellable`; never set for the worker's builds).
    caller: &'a AtomicBool,
    channels: usize,
    buf: Vec<i16>,
    first: u64,
    /// Where the current run started (a chunk boundary) and the furthest frame it wrote.
    run_from: u64,
    run_to: u64,
    /// The run may stop at written chunks or for a new playhead (false: a file that can't seek decodes in one go).
    may_stop: bool,
    pub(super) stopped: bool,
    pub(super) end: u64,
    pub(super) energy: EnergyWriter,
    pub(super) peaks: Peaks,
    /// Why `push` stopped the decode (it can only hand the decoder a message).
    error: Option<BuildError>,
    /// Tests: the hook this build's blocks go through (`hooks`), and how many have.
    #[cfg(test)]
    pub(super) hook: Option<super::hooks::Hook>,
    #[cfg(test)]
    blocks: u64,
}

impl<'a> Sink<'a> {
    pub(super) fn new(
        file: &'a File,
        pcm: Option<&'a Pcm>,
        progress: &'a Progress,
        caller: &'a AtomicBool,
        channels: usize,
        energy: EnergyWriter,
        peaks: Peaks,
    ) -> Self {
        Self {
            file,
            pcm,
            progress,
            caller,
            channels,
            buf: Vec::with_capacity(FLUSH_FRAMES * channels),
            first: 0,
            run_from: 0,
            run_to: 0,
            may_stop: true,
            stopped: false,
            end: 0,
            energy,
            peaks,
            error: None,
            #[cfg(test)]
            hook: None,
            #[cfg(test)]
            blocks: 0,
        }
    }

    /// A close or the caller cancelled the build.
    pub(super) fn cancelled(&self) -> bool {
        self.progress.cancel.load(Ordering::Acquire) || self.caller.load(Ordering::Acquire)
    }

    pub(super) fn begin(&mut self, from: u64, may_stop: bool) -> Result<(), BuildError> {
        self.flush()?;
        (self.run_from, self.run_to) = (from, from);
        (self.may_stop, self.stopped) = (may_stop, false);
        self.progress.at.store(from, Ordering::Relaxed);
        self.energy.begin(from);
        Ok(())
    }

    /// Whether this run should end here: it reached written chunks, or a far playhead asks for another place.
    fn should_stop(&self, first: u64) -> bool {
        let Some(pcm) = self.pcm.filter(|_| self.may_stop) else {
            return false;
        };
        if first > self.run_from && pcm.chunk_ready(first / CHUNK) {
            return true;
        }
        let asked = self.progress.retarget.load(Ordering::Acquire);
        if asked == 0 {
            return false;
        }
        let target = asked - 1;
        if target < first || target > first + RETARGET_AHEAD {
            return true;
        }
        // Near enough: this run gets there.
        let _ =
            self.progress
                .retarget
                .compare_exchange(asked, 0, Ordering::AcqRel, Ordering::Relaxed);
        false
    }

    /// The decoder's block handler: false ends this run; an error ends the decode (`failed` then says why).
    pub(super) fn push(&mut self, first: u64, samples: &[f32]) -> Result<bool, String> {
        self.take(first, samples).map_err(|e| {
            let text = e.to_string();
            self.error = Some(e);
            text
        })
    }

    fn take(&mut self, first: u64, samples: &[f32]) -> Result<bool, BuildError> {
        self.progress.follow_urgency();
        #[cfg(test)]
        if let Some(hook) = &self.hook {
            hook(self.blocks);
            self.blocks += 1;
        }
        if self.cancelled() {
            return Err(BuildError::Cancelled);
        }
        if self.should_stop(first) {
            self.stopped = true;
            return Ok(false);
        }
        let written = (self.buf.len() / self.channels) as u64;
        if self.buf.is_empty() {
            self.first = first;
        } else if self.first + written != first {
            self.flush()?;
            self.first = first;
        }
        // Half scale: 6 dB of headroom for a lossy decoder's overs.
        self.buf.extend(
            samples
                .iter()
                .map(|x| ((x * 0.5).clamp(-1.0, 1.0) * 32767.0).round() as i16),
        );
        self.energy.push(first, samples)?;
        self.peaks.push(first, samples)?;
        let to = first + (samples.len() / self.channels) as u64;
        self.progress.at.store(to, Ordering::Relaxed);
        if self.buf.len() >= FLUSH_FRAMES * self.channels {
            self.flush()?;
        }
        Ok(true)
    }

    /// Writes what is buffered and marks the chunks this run has now written in full.
    pub(super) fn flush(&mut self) -> Result<(), BuildError> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let at = HEADER as u64 + self.first * self.channels as u64 * 2;
        self.file
            .write_all_at(bytemuck::cast_slice(&self.buf), at)?;
        let to = self.first + (self.buf.len() / self.channels) as u64;
        self.buf.clear();
        self.end = self.end.max(to);
        self.run_to = self.run_to.max(to);
        if let Some(pcm) = self.pcm {
            (self.run_from / CHUNK..self.run_to / CHUNK).for_each(|c| pcm.set_chunk_ready(c));
        }
        Ok(())
    }

    /// The run reached the end of the file: its last, partial chunk is ready too. Returns the file's length.
    pub(super) fn reached_end(&mut self) -> Result<u64, BuildError> {
        self.flush()?;
        let total = self.run_to;
        if let Some(pcm) = self.pcm {
            pcm.total.store(total, Ordering::Release);
            (self.run_from / CHUNK..total.div_ceil(CHUNK)).for_each(|c| pcm.set_chunk_ready(c));
        }
        self.peaks.end_run(true)?;
        self.energy.end_run(true)?;
        Ok(total)
    }

    /// One run from `from` to the end of the file in order, never stopping early.
    pub(super) fn whole(&mut self, path: &Path, from: u64) -> Result<u64, BuildError> {
        self.begin(from, false)?;
        let decoded = decode::decode(path, from, None, &mut |at, s| self.push(at, s));
        if let Err(e) = decoded {
            return Err(self.failed(e));
        }
        self.reached_end()
    }

    /// Why the decode ended with `e`: what `push` stopped it for (cancelled, the cache couldn't be written), else
    /// the decoder's own reason. A read error part-way writes what decoded before it and marks it ready up to and
    /// including the chunk the bad spot is in (the rest of that chunk reads as silence): the store can keep it
    /// playable (`PcmStore`, `READ_TRIES`), and a later run from below stops there instead of reading it again.
    pub(super) fn failed(&mut self, e: DecodeError) -> BuildError {
        if let Some(error) = self.error.take() {
            return error;
        }
        match e {
            DecodeError::CantSeek => {
                BuildError::Source("The audio couldn't be read in order.".into())
            }
            DecodeError::Failed(e) => BuildError::Source(e),
            DecodeError::Unreadable(message) => {
                if let Err(error) = self.flush() {
                    return error;
                }
                if let Some(pcm) = self.pcm {
                    (self.run_from / CHUNK..=self.run_to / CHUNK)
                        .for_each(|c| pcm.set_chunk_ready(c));
                }
                BuildError::Unreadable {
                    message,
                    at: self.run_to,
                }
            }
        }
    }
}
