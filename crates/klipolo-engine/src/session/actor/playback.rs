//! Playback: Rust owns the clock, and audio is its master. The actor renders the frame under the clock on
//! every frame boundary, stamps it as a playback frame with its time (the UI playhead follows it) and keeps the sound
//! in step. With sound, the clock is the samples the device has played, and frames are drawn ahead of it by the
//! measured delivery delay, so a frame reaches the screen as its sound is heard. Without sound (a silent project, or
//! a device that failed or never started) the host clock runs the picture. Device work never blocks the actor.
//! How the sound arrives, fails or follows edits is in `sound.rs`.
//!
//! Shuttle (J/K/L): playback also runs backward and at double speed. At any rate but 1 the picture runs silently
//! on the host clock, `from + elapsed × rate`; each tick draws the frame under it (at 2× every other frame, so drawing
//! stays at the project's frame rate), and backward playback ends on frame 0. Going backward, the lookahead warms what
//! comes before the playhead (`lookahead::upcoming_backward`). Every rate starts in `start_playing` and ends in
//! `stop_playing`, so the thumbnail scans' playback hold and the sound follow it.

use std::sync::Arc;
use std::time::{Duration, Instant};

use klipolo_model::time::Us;

use super::Actor;
use super::render::{hide_edited, within_budget};
use crate::api::{
    CodedError, EngineEvent, PLAY_NO_PROJECT, PLAY_NO_VIDEO, RATE_UNAVAILABLE, StateReason,
    rate_supported,
};
use crate::audio::plan::AudioPlan;
use crate::frames::trailer::{FLAG_PLAYBACK, FrameMeta, now_ms};
use crate::media::Hold;
use crate::plan::{frame_plan, frame_time_us, last_frame_us};
use crate::present::PresenterKind;
use crate::session::{AudioTime, PreviewBackend, RenderRequest};

mod clock;
mod late;
mod lookahead;
mod seek;
#[cfg(test)]
mod tests;

use clock::{next_draw_in, timeline_us};
use late::Late;
use lookahead::warm_ahead;

/// The sound gets this long to start (a Bluetooth device wakes up); then playback goes on without it.
const START_TIMEOUT: Duration = Duration::from_secs(2);

pub(super) enum Clock {
    /// The sound for `from_us` is on its way (device opening, or a live seek): the picture holds there.
    Waiting { deadline: Instant },
    /// Following the samples the device plays.
    Audio,
    /// The host clock: `from_us` at `started`. `silent`: the plan had no sound (an edit that adds some starts it).
    Host { started: Instant, silent: bool },
}

pub(super) struct Playing {
    pub(super) from_us: Us,
    /// Timeline µs per real µs: one of `api::PLAYBACK_RATES` (negative backward). Sound plays at 1 only.
    pub(super) rate: f64,
    pub(super) clock: Clock,
    pub(super) next: Instant,
    pub(super) audio: Arc<AudioPlan>,
    /// Starts up to here (down to here going backward) were handed to `warm` already (each upcoming start is planned
    /// and warmed once).
    warmed_to: Us,
    /// The newest audio clock reading: it never steps back.
    pub(super) heard: Us,
    /// The frame drawn last; the same frame isn't drawn twice unless something changed.
    drawn: Option<Us>,
    /// This run's late frames: playback says once when it falls behind (`late`).
    late: Late,
}

/// Resolves on the next frame boundary while playing; never while paused.
pub(super) async fn next_tick(playing: &Option<Playing>) {
    match playing {
        Some(p) => tokio::time::sleep_until(p.next.into()).await,
        None => std::future::pending().await,
    }
}

impl<B: PreviewBackend> Actor<B> {
    /// Plays from `from_us` at `rate`; its frames carry `seq` (`Engine::play`).
    pub(super) fn on_play(&mut self, from_us: Us, rate: f64, seq: u32) -> Result<(), CodedError> {
        if !rate_supported(rate) {
            return Err(CodedError::new(RATE_UNAVAILABLE));
        }
        let Some(cur) = &self.current else {
            return Err(CodedError::new(PLAY_NO_PROJECT));
        };
        let p = &cur.ctx.project;
        let duration = cur.ctx.video_duration_us;
        if duration == 0 {
            return Err(CodedError::new(PLAY_NO_VIDEO));
        }
        let last = last_frame_us(duration, p.settings.fps);
        let from = if rate < 0.0 {
            // Backward from the last frame at most; from the first it ends at once.
            from_us.clamp(0, last)
        } else if from_us >= last {
            0
        } else {
            from_us.max(0)
        };
        // The UI drops frames that don't carry it: those still in flight from the run before.
        self.seq = seq;
        if self.view.is_none() {
            // The preview hasn't said how big it is yet: start as soon as it does.
            self.play_on_view = Some((from, rate));
        } else {
            if self.playing.is_some() {
                // A new speed or direction: what was warmed ahead is stale, as after a live seek.
                self.p.backend.cancel_warm();
            }
            self.start_playing(from, rate);
        }
        self.emit(EngineEvent::State {
            playing: true,
            at_us: from,
            reason: StateReason::Play,
            rate,
        });
        Ok(())
    }

    /// Starts (or, during playback, moves) the clock and the sound to `from_us` at `rate`. Never waits on the device.
    /// Sound plays at 1× only: at another rate the picture runs silently on the host clock, and a change from 1× fades
    /// out what was playing (the output's pause).
    pub(super) fn start_playing(&mut self, from_us: Us, rate: f64) {
        let Some(cur) = &self.current else { return };
        let audio = cur.sound();
        let silent = audio.sources.is_empty();
        // Thumbnail scans wait while the video plays (the sound cache's builds don't: playback needs them).
        self.p.backend.hold_media(Hold::Playback, true);
        let sounding = if rate == 1.0 {
            self.p.backend.start_audio(audio.clone(), from_us)
        } else {
            if self.playing.as_ref().is_some_and(|p| p.rate == 1.0) {
                self.p.backend.stop_audio();
            }
            false
        };
        let now = Instant::now();
        self.playhead_us = from_us;
        self.playing = Some(Playing {
            from_us,
            rate,
            clock: if sounding {
                Clock::Waiting {
                    deadline: now + START_TIMEOUT,
                }
            } else {
                Clock::Host {
                    started: now,
                    silent,
                }
            },
            next: now,
            audio,
            warmed_to: from_us,
            heard: Us::MIN,
            drawn: None,
            late: Late::default(),
        });
    }

    /// Stops at the frame showing at `at_us` and draws it as a still.
    pub(super) fn stop_playing(&mut self, at_us: Us, reason: StateReason) {
        let Some(play) = self.playing.take() else {
            return;
        };
        if play.rate == 1.0 {
            self.p.backend.stop_audio();
        }
        self.p.backend.hold_media(Hold::Playback, false);
        // Decoders warmed for starts ahead won't be needed now.
        self.p.backend.cancel_warm();
        // Editing copies that finished (or went away) while it played are read from the next still.
        if let Some(map) = self.pending_media.take() {
            self.apply_media(map);
        }
        // A take's facts or follow paths that arrived while it played.
        if self.cursor_due {
            self.on_cursor();
        }
        let fps = self
            .current
            .as_ref()
            .map_or(30, |c| c.ctx.project.settings.fps);
        self.playhead_us = self.clamp_time(frame_time_us(at_us, fps));
        self.dirty = true;
        self.emit(EngineEvent::State {
            playing: false,
            at_us: self.playhead_us,
            reason,
            rate: 1.0,
        });
    }

    pub(super) async fn on_tick(&mut self) {
        let now = Instant::now();
        self.note_late(now);
        let (Some(cur), Some(view), Some(play)) = (&self.current, self.view, &mut self.playing)
        else {
            return self.stop_playing(self.playhead_us, StateReason::Error);
        };
        let fps = cur.ctx.project.settings.fps.max(1);
        let last = last_frame_us(cur.ctx.video_duration_us, fps);
        // The time heard now, and the time to draw (ahead of it by the delivery delay when the sound leads).
        let (heard, show) = match play.clock {
            Clock::Waiting { deadline } if now >= deadline => {
                return self.play_without_sound("the output device didn't start in time");
            }
            Clock::Waiting { deadline } => {
                // Show where playback starts; the sound arriving wakes the next tick.
                play.next = deadline;
                (play.from_us, play.from_us)
            }
            Clock::Audio => match self.p.backend.audio_time() {
                AudioTime::At(t) => {
                    play.heard = play.heard.max(t);
                    (play.heard, (play.heard + self.lead_us).max(play.from_us))
                }
                AudioTime::Failed(message) => return self.play_without_sound(&message),
                AudioTime::Waiting | AudioTime::Off => {
                    let held = play.heard.max(play.from_us);
                    (held, held)
                }
            },
            Clock::Host { started, .. } => {
                let t =
                    play.from_us + timeline_us(now.saturating_duration_since(started), play.rate);
                (t, t)
            }
        };
        let backward = play.rate < 0.0;
        if backward && heard <= 0 {
            // Backward ends on the first picture.
            return self.stop_playing(0, StateReason::Ended);
        }
        if !backward && heard >= last {
            // Stop on the last picture, not on black.
            return self.stop_playing(last, StateReason::Ended);
        }
        let at_us = frame_time_us(show.clamp(0, last), fps);
        if !matches!(play.clock, Clock::Waiting { .. }) {
            play.next = now + next_draw_in(show, fps, play.rate);
        }
        if play.drawn == Some(at_us) && !self.dirty {
            return;
        }
        play.drawn = Some(at_us);
        warm_ahead(
            &cur.ctx,
            view.format,
            play,
            at_us,
            &mut self.starts,
            &mut self.p.backend,
        );
        let mut plan = frame_plan(&cur.ctx, at_us, view.format);
        hide_edited(&mut plan, self.editing.as_deref());
        // On screen when its sound is heard: `at_us` is ahead of what is heard now by up to the lead (the
        // the presenter holds it until then). While the sound is on its way the picture holds, shown at once.
        let due_ms = match play.clock {
            Clock::Waiting { .. } => 0.0,
            _ => now_ms() + (at_us - heard).max(0) as f64 / 1000.0,
        };
        let meta = FrameMeta {
            flags: FLAG_PLAYBACK,
            seq: self.seq,
            session: self.session,
            revision: cur.revision as u32,
            time_us: at_us,
            due_ms,
            ..FrameMeta::default()
        };
        self.playhead_us = at_us;
        self.dirty = false;
        // Over the socket, playback renders at most the socket's playback budget; a pause draws the still at the full
        // size again. The native layer has no transport cost, so it keeps the full size.
        let budget = match self.presenter.kind() {
            PresenterKind::Socket => self.p.socket_playback_pixels,
            PresenterKind::Native => self.p.max_pixels,
        };
        let (width, height) = within_budget(view.width, view.height, budget);
        let output = self.presenter.output(true);
        self.ticked = Some(now);
        self.render(
            RenderRequest {
                plan,
                width,
                height,
                output,
            },
            meta,
        )
        .await;
    }
}
