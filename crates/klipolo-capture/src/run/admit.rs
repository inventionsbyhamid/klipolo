//! Samples into runs (the module doc of `source`): video packets and sound blocks admitted on the session clock, each
//! source's first opening its run.

use klipolo_model::time::Us;

use super::core::SessionCore;
use super::effect::{Effect, PipeCmd, Started};
use super::source::{GATHER_US, LOSS_US, Run, SoundRun, Status, TIMING, VideoRun, flush_sound};
use crate::caf;
use crate::drift::Anchor;
use crate::fmp4::Fmp4Writer;
use crate::journal::{self, Entry};
use crate::manifest;
use crate::media::{AudioFormat, EncodedPacket, PcmBlock};
use crate::session::SessionDir;

impl SessionCore {
    pub(super) fn write_packet(&mut self, i: usize, p: &EncodedPacket, fx: &mut Vec<Effect>) {
        let Some(t) = self.clock.session_us(p.host_ns) else {
            return;
        };
        if self.srcs[i].status != Status::Live {
            return;
        }
        self.srcs[i].restart_at = None;
        if self.srcs[i].run.is_none() {
            if !p.keyframe {
                if !self.srcs[i].key_asked {
                    self.srcs[i].key_asked = true;
                    fx.push(self.pipe(i, PipeCmd::Keyframe));
                }
                return;
            }
            self.open_video(i, t, fx);
        }
        let Some(Run::Video(run)) = &mut self.srcs[i].run else {
            return;
        };
        if run.awaiting_key {
            if !p.keyframe {
                return;
            }
            run.awaiting_key = false;
        }
        self.mapped_until = self.mapped_until.max(p.host_ns + 1);
        if let Some(f) = run.mux.push((t - run.offset_us).max(0), p) {
            let media_us = f.media_us();
            fx.push(Effect::Append {
                key: run.key.clone(),
                bytes: f.bytes,
                media_us,
            });
        }
    }

    pub(super) fn open_video(&mut self, i: usize, t: Us, fx: &mut Vec<Effect>) {
        let src = &mut self.srcs[i];
        let Some(Started::Video(info)) = &src.format else {
            return;
        };
        let n = src.next_run;
        src.next_run += 1;
        let key = SessionDir::run_key(&src.id, n, true);
        let mux = Fmp4Writer::at_rate(info, src.fps);
        fx.push(Effect::Open {
            key: key.clone(),
            head: mux.head(),
        });
        fx.push(Effect::Journal(Entry::RunStarted {
            source: src.id.clone(),
            run: n,
            file: key.clone(),
            offset_us: t,
            video: Some(info.clone()),
            audio: None,
        }));
        (src.wrote, src.key_asked) = (true, false);
        src.run = Some(Run::Video(VideoRun {
            n,
            key,
            mux,
            offset_us: t,
            awaiting_key: false,
        }));
        self.gap_note(i, t, fx);
        self.cursor_run_opened(i, t, fx);
    }

    pub(super) fn write_pcm(&mut self, i: usize, block: &PcmBlock, fx: &mut Vec<Effect>) {
        let src = &self.srcs[i];
        let Some(Started::Audio(format)) = src.format else {
            return;
        };
        if src.status != Status::Live {
            return;
        }
        let frames_in = match block.whole_frames(format.channels) {
            Ok(n) => n,
            Err(partial) => {
                if !src.partial_noted {
                    let id = src.id.clone();
                    self.srcs[i].partial_noted = true;
                    self.note(
                        partial.coded(),
                        crate::shell::Level::Ok,
                        Some(&id),
                        false,
                        None,
                        fx,
                    );
                }
                return;
            }
        };
        let rate = u64::from(format.rate.max(1));
        let ns_of = |f: u64| (u128::from(f) * 1_000_000_000 / u128::from(rate)) as u64;
        let start = block.host_ns;
        let end = start + ns_of(frames_in);
        // Cut at the gate and at pauses, by the block's own frames.
        let Some(first) = self.clock.next_running(start).filter(|&f| f < end) else {
            return;
        };
        let until = self.clock.next_pause(first).map_or(end, |p| p.min(end));
        let (skip, keep_end) = cut(start, frames_in, rate, first, until);
        let keep = keep_end.saturating_sub(skip);
        let Some(t) = self
            .clock
            .session_us(start + ns_of(skip))
            .filter(|_| keep > 0)
        else {
            return;
        };
        self.mapped_until = self.mapped_until.max(start + ns_of(keep_end));
        let block_us = (frames_in * 1_000_000 / rate) as Us;
        if let Some(Run::Sound(r)) = &self.srcs[i].run
            && (t - r.next_us).abs() >= block_us.clamp(1, LOSS_US)
        {
            tracing::info!(
                source = self.srcs[i].id,
                lost_us = t - r.next_us,
                "core: a sound discontinuity ends the run"
            );
            self.end_run(i, fx);
        }
        if self.srcs[i].run.is_none() {
            self.open_sound(i, t, format, fx);
        }
        let channels = usize::from(format.channels.max(1));
        let samples = &block.samples[skip as usize * channels..keep_end as usize * channels];
        let src = &mut self.srcs[i];
        let Some(Run::Sound(r)) = &mut src.run else {
            return;
        };
        if t >= r.next_anchor_us {
            let anchor = Anchor {
                session_us: t,
                frame: r.frames,
            };
            fx.push(Effect::Append {
                key: TIMING.into(),
                bytes: journal::anchor_line(&src.id, r.n, &anchor),
                media_us: 0,
            });
            r.next_anchor_us = t + 1_000_000;
        }
        for frame in samples.chunks_exact(channels) {
            for (peak, s) in r.peaks.iter_mut().zip(frame) {
                // Clamped: a float device can go past full scale (or send Inf), and a journal line holds finite levels.
                *peak = peak.max(s.abs().min(1.0));
            }
        }
        src.sounded |= r.peaks.iter().any(|&p| p > 0.0);
        caf::encode_into(samples, &mut r.pending);
        r.frames += keep;
        r.pending_frames += keep;
        r.next_us = t + (keep * 1_000_000 / rate) as Us;
        if r.pending_frames * 1_000_000 >= GATHER_US * rate {
            fx.push(flush_sound(r));
        }
    }

    pub(super) fn open_sound(
        &mut self,
        i: usize,
        t: Us,
        format: AudioFormat,
        fx: &mut Vec<Effect>,
    ) {
        if !self.timing_open {
            self.timing_open = true;
            fx.push(Effect::OpenLines { key: TIMING.into() });
        }
        let src = &mut self.srcs[i];
        let n = src.next_run;
        src.next_run += 1;
        let key = SessionDir::run_key(&src.id, n, false);
        fx.push(Effect::Open {
            key: key.clone(),
            head: caf::header(format),
        });
        fx.push(Effect::Journal(Entry::RunStarted {
            source: src.id.clone(),
            run: n,
            file: key.clone(),
            offset_us: t,
            video: None,
            audio: Some(format),
        }));
        src.wrote = true;
        let bytes_per_append = (u64::from(format.rate) * GATHER_US / 1_000_000) as usize * 3;
        src.run = Some(Run::Sound(SoundRun {
            n,
            key,
            format,
            frames: 0,
            pending: Vec::with_capacity(bytes_per_append * usize::from(format.channels) + 4_096),
            pending_frames: 0,
            next_us: t,
            next_anchor_us: t,
            peaks: vec![0.0; usize::from(format.channels.max(1))],
            levels_through: 0,
        }));
        self.gap_note(i, t, fx);
    }

    /// A resumed take: the gap from where this source's media ended before the crash to its new run (non-critical: the
    /// manifest derives the gap from the runs).
    pub(super) fn gap_note(&mut self, i: usize, t: Us, fx: &mut Vec<Effect>) {
        let src = &mut self.srcs[i];
        let Some(from) = src.gap_from.take() else {
            return;
        };
        let seconds = ((t - from).max(0) as f64 / 100_000.0).round() / 10.0;
        let note = crate::english::msg!(
            "capture.note.gap",
            source = manifest::source_param(&src.label),
            seconds = seconds,
            atUs = from
        );
        let id = src.id.clone();
        fx.push(Effect::Journal(Entry::health(&note, Some(id), false)));
    }
}

/// The frames of a block (`frames` at `rate`, the first at host `start`) that start in `[first, until)`, as (frames
/// skipped, frames up to the cut). Both ends count the frames that start before them, rounding up, so a block whose
/// length isn't a whole number of ns keeps its last frame: cpal's 512-frame blocks at 48 kHz lost one each, 0.2 % of the
/// mic, which read as −2,000 ppm of drift and cost the run its line (seen in a smoke run).
pub(super) fn cut(start: u64, frames: u64, rate: u64, first: u64, until: u64) -> (u64, u64) {
    let before =
        |ns: u64| ((u128::from(ns) * u128::from(rate)).div_ceil(1_000_000_000) as u64).min(frames);
    (
        before(first.saturating_sub(start)),
        before(until.saturating_sub(start)),
    )
}

#[cfg(test)]
mod tests {
    use super::cut;

    #[test]
    fn a_block_keeps_every_frame_and_a_pause_cuts_at_its_own_frame() {
        let ns = |f: u64, rate: u64| f * 1_000_000_000 / rate;
        for (frames, rate) in [
            (512, 48_000),
            (480, 48_000),
            (441, 44_100),
            (1_024, 48_000),
            (333, 48_000),
        ] {
            let end = 7 + ns(frames, rate);
            assert_eq!(
                cut(7, frames, rate, 7, end),
                (0, frames),
                "{frames} at {rate}"
            );
        }
        // A pause at frame 100's host time (ns, floored as stamps are) keeps frames 0-99; a gate at frame 20's skips
        // 0-19.
        let at = |f: u64| 7 + ns(f, 48_000);
        assert_eq!(cut(7, 512, 48_000, 7, at(100)), (0, 100));
        assert_eq!(cut(7, 512, 48_000, at(20), at(100)), (20, 100));
    }
}
