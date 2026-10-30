//! The crash guard's handshake in the app's preview
//! m5). Two kinds of mark keep an agent-made effect's riskiest moments inside the guard, each asked of the app with
//! `FirstDraw { contrib, seq }`, written and synced to disk there (`make::guard`), and acknowledged with
//! `MarkWritten { contrib, seq }`; nothing happens before the acknowledgement of that very request (`seq`):
//!
//! - **A compile mark** before the compile thread parses and compiles its shader (naga's check, wgpu's shader module and
//!   pipeline). It closes (`DrewSafely`) when the compile returns, whatever it returned.
//! - **A draw mark** the first time a frame needs the effect (never at placement). It closes after
//!   [`SAFE_AFTER_FRAMES`] frames that drew it (a static generator's cached picture counts) or [`SAFE_AFTER`], both
//!   counted from when the GPU finished the first frame that drew it, whichever comes first (and
//!   Neither starts at the encoding, so a draw that hangs the GPU keeps its mark open however many
//!   frames follow it), so a paused still or a still picture never keeps it
//!   open. One acknowledged that never drew (the playhead left first) closes [`SAFE_AFTER`] after its acknowledgement
//!   and leaves the list: the next frame that needs the effect asks for a fresh one.
//!
//! A mark the preview lets go while it is open (its registry no longer holds the effect) closes too, as nothing draws
//! it any more; one for a compile in flight stays until that compile returns. At a device loss every open mark is
//! settled as a crash at that moment would settle it, named with every effect whose drawing frame never finished on the
//! GPU and those drawn in the last 60 frames and 2 s before the loss's own time (`GpuLost`); the app strikes
//! them and the stage goes. Marks are per pipeline and effect: two effects with
//! the same hardened text share a pipeline, but each draws only under its own mark.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use klipolo_model::contrib::ContribId;

use super::sampling::Degrade;
use crate::api::EngineEvent;
use crate::registry::ShaderEffect;

mod gpu_done;
pub use gpu_done::GpuDone;

/// Frames that must draw a checked effect before its draw mark closes...
pub const SAFE_AFTER_FRAMES: u32 = 60;
/// ...or this long after the GPU finished its first drawing frame, whichever comes first (and this long after an
/// acknowledgement that never drew).
pub const SAFE_AFTER: Duration = Duration::from_secs(2);

/// Every mark's request number, across stages (a stage made after a device loss never reuses one).
static NEXT_SEQ: AtomicU64 = AtomicU64::new(1);

/// What the effect stage tells the app about agent-made effects (the session's actor sends each as an `EngineEvent`).
#[derive(Clone, Debug, PartialEq)]
pub enum GuardEvent {
    FirstDraw {
        contrib: ContribId,
        seq: u64,
    },
    DrewSafely {
        contrib: ContribId,
        seq: u64,
    },
    Degraded {
        contrib: ContribId,
        to: Degrade,
        ms: f64,
        budget_ms: f64,
    },
    GpuLost(Vec<ContribId>),
}

impl GuardEvent {
    pub fn into_event(self) -> EngineEvent {
        match self {
            Self::FirstDraw { contrib, seq } => EngineEvent::FirstDraw { contrib, seq },
            Self::DrewSafely { contrib, seq } => EngineEvent::DrewSafely { contrib, seq },
            Self::Degraded {
                contrib,
                to,
                ms,
                budget_ms,
            } => EngineEvent::EffectDegraded {
                contrib,
                to,
                ms,
                budget_ms,
            },
            Self::GpuLost(contribs) => EngineEvent::GpuLost { contribs },
        }
    }
}

/// What a mark covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Purpose {
    Compile,
    Draw,
}

struct Mark {
    seq: u64,
    /// The pipeline (shader key) and the effect.
    key: u64,
    id: ContribId,
    purpose: Purpose,
    acked: bool,
    /// Closed (`DrewSafely` said): a draw mark stays listed while its effect is drawn, for a device loss.
    closed: bool,
    frames: u32,
    /// The frame of its first draw, and when the GPU had finished it.
    first_frame: Option<u64>,
    first_done: Option<Instant>,
    acked_at: Option<Instant>,
    last_frame: Option<u64>,
    last_at: Option<Instant>,
}

#[derive(Default)]
pub(super) struct Handshake {
    marks: Vec<Mark>,
    pub events: Vec<GuardEvent>,
}

impl Handshake {
    fn ask(&mut self, shader: &ShaderEffect, purpose: Purpose) -> u64 {
        let seq = NEXT_SEQ.fetch_add(1, Ordering::Relaxed);
        self.marks.push(Mark {
            seq,
            key: shader.key,
            id: shader.id.clone(),
            purpose,
            acked: false,
            closed: false,
            frames: 0,
            first_frame: None,
            first_done: None,
            acked_at: None,
            last_frame: None,
            last_at: None,
        });
        self.events.push(GuardEvent::FirstDraw {
            contrib: shader.id.clone(),
            seq,
        });
        seq
    }

    fn close(events: &mut Vec<GuardEvent>, m: &mut Mark) {
        if !m.closed {
            m.closed = true;
            events.push(GuardEvent::DrewSafely {
                contrib: m.id.clone(),
                seq: m.seq,
            });
        }
    }

    /// Asks for a compile mark for `shader`: its request number, which `ack` answers.
    pub fn ask_compile(&mut self, shader: &ShaderEffect) -> u64 {
        self.ask(shader, Purpose::Compile)
    }

    /// The app wrote the mark `seq` of `id` to disk, at `now`: what it covers, if this stage asked for it.
    pub fn ack(&mut self, id: &ContribId, seq: u64, now: Instant) -> Option<Purpose> {
        let m = self
            .marks
            .iter_mut()
            .find(|m| m.seq == seq && m.id == *id && !m.closed)?;
        m.acked = true;
        m.acked_at = Some(now);
        Some(m.purpose)
    }

    /// The compile sent under mark `seq` (pipeline `key`) returned: that mark closes.
    pub fn compile_returned(&mut self, key: u64, seq: u64) {
        let events = &mut self.events;
        self.marks.retain_mut(|m| {
            let done = m.purpose == Purpose::Compile && m.key == key && m.seq == seq;
            if done {
                Self::close(events, m);
            }
            !done
        });
    }

    /// Whether `shader` may draw with its pipeline: its own draw mark is acknowledged. Asks for it when a frame first
    /// needs it. No allocation once asked.
    pub fn may_draw(&mut self, shader: &ShaderEffect) -> bool {
        let found = self
            .marks
            .iter()
            .find(|m| m.purpose == Purpose::Draw && m.key == shader.key && m.id == shader.id);
        match found {
            Some(m) => m.acked,
            None => {
                self.ask(shader, Purpose::Draw);
                false
            }
        }
    }

    /// `shader` drew in frame `frame` at `now` (a pass, or its picture from the static cache): its draw mark closes at
    /// the 60th such frame once its first drawing frame finished on the GPU (a hang doesn't stop
    /// frames, since a wait gives up after 2 s, so frames before that count for nothing). No allocation.
    pub fn drew(&mut self, shader: &ShaderEffect, frame: u64, now: Instant) {
        let Some(m) = self.marks.iter_mut().find(|m| {
            m.purpose == Purpose::Draw && m.acked && m.key == shader.key && m.id == shader.id
        }) else {
            return;
        };
        if m.last_frame == Some(frame) {
            return;
        }
        m.last_frame = Some(frame);
        m.last_at = Some(now);
        if m.closed {
            return;
        }
        m.first_frame.get_or_insert(frame);
        if m.first_done.is_none() {
            return;
        }
        m.frames += 1;
        if m.frames >= SAFE_AFTER_FRAMES {
            Self::close(&mut self.events, m);
        }
    }

    /// Draw marks at `now`, the GPU having finished frame `done_frame` (taken as finished then): see `tick_done` (tests).
    #[cfg(test)]
    pub fn tick(&mut self, now: Instant, done_frame: u64) {
        self.tick_done(now, done_frame, now);
    }

    /// Draw marks at `now`, the GPU having finished frame `done_frame` at `done_at`: a mark whose first drawing frame is
    /// finished starts its [`SAFE_AFTER`] then, and closes when they are over; one acknowledged [`SAFE_AFTER`] ago
    /// that never drew closes and leaves the list.
    pub fn tick_done(&mut self, now: Instant, done_frame: u64, done_at: Instant) {
        let events = &mut self.events;
        self.marks.retain_mut(|m| {
            if m.purpose != Purpose::Draw || m.closed || !m.acked {
                return true;
            }
            match m.first_frame {
                Some(f) if f <= done_frame => {
                    let done = *m.first_done.get_or_insert(done_at);
                    if now.saturating_duration_since(done) >= SAFE_AFTER {
                        Self::close(events, m);
                    }
                    true
                }
                Some(_) => true,
                None => {
                    let gone = m
                        .acked_at
                        .is_some_and(|a| now.saturating_duration_since(a) >= SAFE_AFTER);
                    if gone {
                        Self::close(events, m);
                    }
                    !gone
                }
            }
        });
    }

    /// When the next open draw mark runs out of time (from its finished first frame, or from an acknowledgement that
    /// never drew).
    pub fn deadline(&self) -> Option<Instant> {
        self.marks
            .iter()
            .filter(|m| m.purpose == Purpose::Draw && m.acked && !m.closed)
            .filter_map(|m| match m.first_frame {
                Some(_) => m.first_done,
                None => m.acked_at,
            })
            .min()
            .map(|t| t + SAFE_AFTER)
    }

    /// Whether an open draw mark waits for the GPU to finish its first drawing frame (the stage then asks to hear when
    /// a frame is done).
    pub fn waiting_for_gpu(&self) -> bool {
        self.marks.iter().any(|m| {
            m.purpose == Purpose::Draw
                && !m.closed
                && m.first_frame.is_some()
                && m.first_done.is_none()
        })
    }

    /// Only the marks `keep` (pipeline, effect, whether a compile's) holds stay; each other open one closes, as
    /// nothing draws it any more.
    pub fn let_go(&mut self, keep: impl Fn(u64, &ContribId, bool) -> bool) {
        let events = &mut self.events;
        self.marks.retain_mut(|m| {
            let kept = keep(m.key, &m.id, m.purpose == Purpose::Compile);
            if !kept {
                Self::close(events, m);
            }
            kept
        });
    }

    /// The device was lost at `lost_at` (its own time, from the device's callback), this stage at frame `frame` and the
    /// GPU done with `done_frame`. Every mark is settled as a crash at that moment would settle it: an
    /// acknowledged open mark (on disk) is named for a strike, with every effect whose drawing frame never finished on
    /// the GPU, whatever the time, and every one drawn in the last [`SAFE_AFTER_FRAMES`] frames and [`SAFE_AFTER`]
    /// before the loss; a mark asked for and not acknowledged closes (nothing ran under it). The marks go with the
    /// stage.
    pub fn lost(&mut self, frame: u64, done_frame: u64, lost_at: Instant) -> Vec<ContribId> {
        let named = |m: &Mark| {
            let Some(last) = m.last_frame else {
                return m.acked && !m.closed;
            };
            (m.acked && !m.closed)
                || last > done_frame
                || (frame.saturating_sub(last) <= u64::from(SAFE_AFTER_FRAMES)
                    && m.last_at
                        .is_some_and(|t| lost_at.saturating_duration_since(t) <= SAFE_AFTER))
        };
        let mut ids: Vec<ContribId> = self
            .marks
            .iter()
            .filter(|m| named(m))
            .map(|m| m.id.clone())
            .collect();
        for m in &mut self.marks {
            if !m.acked {
                Self::close(&mut self.events, m);
            }
        }
        ids.sort();
        ids.dedup();
        self.marks.clear();
        ids
    }

    /// Marks still open (asked for and not closed): tests.
    pub fn open(&self) -> usize {
        self.marks.iter().filter(|m| !m.closed).count()
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use klipolo_model::contrib::{ContribDef, ContribKind};

    use super::*;
    use crate::registry::ShaderKind;

    fn shader() -> ShaderEffect {
        let def = ContribDef::new(
            "lib.k3f9q2ma/generator.flat.v1",
            ContribKind::Generator,
            "Flat",
        );
        ShaderEffect::assemble_untrusted(
            &def,
            ShaderKind::Generator,
            "fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(uv, 0.0, 1.0); }",
        )
        .unwrap()
    }

    /// The draw mark `h` just asked for `s`, acknowledged at `at`.
    fn asked_and_acked(h: &mut Handshake, s: &ShaderEffect, at: Instant) -> u64 {
        assert!(!h.may_draw(s));
        let Some(GuardEvent::FirstDraw { seq, .. }) = h.events.last().cloned() else {
            panic!("no FirstDraw");
        };
        assert_eq!(h.ack(&s.id, seq, at), Some(Purpose::Draw));
        seq
    }

    fn first_draws(h: &Handshake) -> usize {
        h.events
            .iter()
            .filter(|e| matches!(e, GuardEvent::FirstDraw { .. }))
            .count()
    }

    /// A draw mark acknowledged after the playhead left (so it never drew) closes 2 s after its
    /// acknowledgement and leaves the list; the next frame that needs the effect asks for a fresh one.
    #[test]
    fn a_draw_mark_that_never_drew_closes_2_s_after_its_acknowledgement() {
        let (mut h, s, t0) = (Handshake::default(), shader(), Instant::now());
        asked_and_acked(&mut h, &s, t0);
        h.tick(t0 + Duration::from_millis(1900), 0);
        assert_eq!(h.open(), 1, "not yet");
        h.tick(t0 + Duration::from_millis(2100), 0);
        assert_eq!(h.open(), 0, "closed, never drawn");
        let asked = first_draws(&h);
        assert!(!h.may_draw(&s), "it draws only under a new mark");
        assert_eq!(first_draws(&h), asked + 1, "a fresh mark asked for");
    }

    /// A draw mark's 2 s start when the frame that drew it is done on the GPU, never at its encoding:
    /// a hang keeps it open, and a device loss then names it, whatever the time.
    #[test]
    fn the_2_s_start_when_the_gpu_finished_the_frame_and_a_hang_is_named() {
        let (mut h, s, t0) = (Handshake::default(), shader(), Instant::now());
        asked_and_acked(&mut h, &s, t0);
        h.drew(&s, 1, t0);
        h.tick(t0 + Duration::from_secs(3), 0);
        assert_eq!(h.open(), 1, "frame 1 isn't done on the GPU: still open");
        h.tick(t0 + Duration::from_millis(3500), 1);
        assert_eq!(h.open(), 1, "done now: its 2 s start");
        h.tick(t0 + Duration::from_millis(5600), 1);
        assert_eq!(h.open(), 0, "2 s after the GPU finished it");
        // A hang: the frame that drew it never finished, the loss comes 3 s later.
        let (mut h, t0) = (Handshake::default(), Instant::now());
        asked_and_acked(&mut h, &s, t0);
        h.drew(&s, 1, t0);
        assert_eq!(
            h.lost(2, 0, t0 + Duration::from_secs(3)),
            vec![s.id.clone()]
        );
        // A frame that finished, its draw over 2 s before the loss: not named.
        let (mut h, t0) = (Handshake::default(), Instant::now());
        asked_and_acked(&mut h, &s, t0);
        h.drew(&s, 1, t0);
        h.tick(t0, 1);
        h.tick(t0 + Duration::from_millis(2100), 1);
        assert!(h.lost(1, 1, t0 + Duration::from_secs(3)).is_empty());
        // Closed by its 60 frames, but its last frame never finished: named at a loss long after (added after the red run).
        let (mut h, t0) = (Handshake::default(), Instant::now());
        asked_and_acked(&mut h, &s, t0);
        h.drew(&s, 1, t0);
        h.tick(t0, 1);
        for f in 2..=61 {
            h.drew(&s, f, t0);
        }
        assert_eq!(h.open(), 0, "60 frames closed it");
        assert_eq!(
            h.lost(62, 60, t0 + Duration::from_secs(10)),
            vec![s.id.clone()]
        );
    }

    /// A hang doesn't stop frames (each wait gives up after 2 s), so a draw mark counts toward its
    /// 60 frames only once its first drawing frame finished on the GPU: 70 frames over a first frame that never
    /// finished leave it open; once it finished, the next 60 close it.
    #[test]
    fn the_60_frames_count_only_once_the_first_frame_finished() {
        let (mut h, s, t0) = (Handshake::default(), shader(), Instant::now());
        asked_and_acked(&mut h, &s, t0);
        for f in 1..=70 {
            h.drew(&s, f, t0);
            h.tick(t0, 0);
        }
        assert_eq!(h.open(), 1, "its first frame never finished: still open");
        h.tick(t0, 1);
        for f in 71..=129 {
            h.drew(&s, f, t0);
        }
        assert_eq!(h.open(), 1, "59 frames since it finished");
        h.drew(&s, 130, t0);
        assert_eq!(h.open(), 0, "the 60th since its first frame finished");
    }
}
