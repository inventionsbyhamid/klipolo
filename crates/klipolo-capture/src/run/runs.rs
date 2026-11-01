//! Ending runs (a heal, a full queue, Stop), flushing what is gathered, the running levels each tick, and Stop's mild
//! notes.

use super::core::SessionCore;
use super::effect::{Effect, RunDone, RunKind};
use super::source::{Run, flush_sound};
use crate::fmp4::TIMESCALE;
use crate::heal::SourceKind;
use crate::journal::Entry;
use crate::manifest;

impl SessionCore {
    /// Ends source `i`'s run: the muxer's last fragment or the gathered sound appended, its levels (sound) and
    /// `RunEnded` journaled at the session time just after its last sample, the file closed. Kept for Stop.
    pub(super) fn end_run(&mut self, i: usize, fx: &mut Vec<Effect>) {
        let src = &mut self.srcs[i];
        let Some(run) = src.run.take() else {
            return;
        };
        let (n, key, end_us, kind) = match run {
            Run::Video(mut r) => {
                if let Some(f) = r.mux.flush() {
                    let media_us = f.media_us();
                    fx.push(Effect::Append {
                        key: r.key.clone(),
                        bytes: f.bytes,
                        media_us,
                    });
                }
                let duration_us = r.mux.index().duration_us(TIMESCALE);
                let kind = RunKind::Video {
                    info: r.mux.info().clone(),
                    plan: r.mux.remux_plan(),
                    bytes_written: r.mux.bytes_written(),
                    duration_us,
                };
                (r.n, r.key, r.offset_us + duration_us, kind)
            }
            Run::Sound(mut r) => {
                if r.pending_frames > 0 {
                    fx.push(flush_sound(&mut r));
                }
                fx.push(Effect::Journal(Entry::Levels {
                    file: r.key.clone(),
                    peaks: r.peaks,
                    through: None,
                }));
                (r.n, r.key, r.next_us, RunKind::Sound { format: r.format })
            }
        };
        fx.push(Effect::Journal(Entry::RunEnded {
            source: src.id.clone(),
            run: n,
            end_us,
        }));
        fx.push(Effect::Close { key: key.clone() });
        self.done.push(RunDone {
            source: src.id.clone(),
            run: n,
            key,
            kind,
        });
    }

    /// Gathered sound appended now (a pause, a tick), so nothing waits in memory.
    pub(super) fn flush_gathered(&mut self, fx: &mut Vec<Effect>) {
        for src in &mut self.srcs {
            if let Some(Run::Sound(r)) = &mut src.run
                && r.pending_frames > 0
            {
                fx.push(flush_sound(r));
            }
        }
    }

    /// Each open sound run's levels so far, journaled just after the tick that pushed its sound (the gathered sound was
    /// appended before it, so the line covers what reached the file, and never reaches the disk ahead of it):
    /// recovery and a resumed recorder read only what came after the last one. A
    /// run that took nothing new (paused) journals nothing. About 90 bytes a second each.
    pub(super) fn running_levels(&mut self, fx: &mut Vec<Effect>) {
        for src in &mut self.srcs {
            let Some(Run::Sound(r)) = &mut src.run else {
                continue;
            };
            let appended = r.frames - r.pending_frames;
            let through = crate::caf::HEADER_LEN + appended * crate::caf::frame_bytes(r.format);
            if through == r.levels_through {
                continue;
            }
            r.levels_through = through;
            fx.push(Effect::Journal(Entry::Levels {
                file: r.key.clone(),
                peaks: r.peaks.clone(),
                through: Some(through),
            }));
        }
    }

    /// At Stop: a game or computer sound source that sent only exact zeros the whole take gets a mild note (silence
    /// is normal for game sound, so this is never a warning during the take).
    pub(super) fn silent_sound_notes(&mut self, fx: &mut Vec<Effect>) {
        for i in 0..self.srcs.len() {
            let src = &self.srcs[i];
            if src.kind == SourceKind::Sound && src.wrote && !src.sounded {
                let note = crate::english::msg!(
                    "capture.note.silentSound",
                    source = manifest::source_param(&src.label)
                );
                let id = src.id.clone();
                self.note(note, crate::shell::Level::Ok, Some(&id), true, None, fx);
            }
        }
    }

    /// The video run waits for a keyframe again (a resume).
    pub(super) fn await_keyframe(&mut self, i: usize) {
        if let Some(Run::Video(r)) = &mut self.srcs[i].run {
            r.awaiting_key = true;
        }
    }
}
