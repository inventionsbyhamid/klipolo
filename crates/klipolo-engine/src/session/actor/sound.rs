//! The sound side of playback: the device reporting in (the sound is on the clock, or it failed) and edits that
//! change what plays.

use std::sync::Arc;
use std::time::Instant;

use super::Actor;
use super::playback::Clock;
use crate::api::EngineEvent;
use crate::session::{AudioTime, PreviewBackend};

pub(super) const NO_SOUND: &str = "Sound isn't available right now. Playing without sound.";

impl<B: PreviewBackend> Actor<B> {
    /// The sound arrived on the clock, or the device failed.
    pub(super) fn on_audio(&mut self) {
        let Some(play) = &mut self.playing else {
            return;
        };
        match self.p.backend.audio_time() {
            AudioTime::At(_) if matches!(play.clock, Clock::Waiting { .. }) => {
                play.clock = Clock::Audio;
                play.next = Instant::now();
            }
            AudioTime::Failed(message) => self.play_without_sound(&message),
            _ => {}
        }
    }

    /// The device failed or never started: tell the user once, and carry on with the host clock from here.
    pub(super) fn play_without_sound(&mut self, why: &str) {
        tracing::warn!(%why, "playing without sound");
        let at = self.clock_us();
        self.p.backend.stop_audio();
        if let Some(play) = &mut self.playing {
            play.from_us = at;
            play.clock = Clock::Host {
                started: Instant::now(),
                silent: false,
            };
            play.next = Instant::now();
        }
        if self.warned.insert(NO_SOUND.into()) {
            self.emit(EngineEvent::Warning {
                message: NO_SOUND.into(),
            });
        }
    }

    /// An edit during playback: the clock keeps going and the sound follows the new plan. At another speed than 1×
    /// nothing plays; the next 1× start plans the sound afresh. An edit takes its revision's plan, and a dry run whose
    /// ops can't change the sound takes the committed one, so neither plans again (`Current::sound`).
    pub(super) fn replan_audio(&mut self) {
        let (Some(cur), Some(play)) = (&self.current, &mut self.playing) else {
            return;
        };
        if play.rate != 1.0 {
            return;
        }
        let audio = cur.sound();
        if Arc::ptr_eq(&audio, &play.audio) {
            return;
        }
        if audio == play.audio {
            // The same sound: keep the shared plan, so the next dry-run frame compares pointers only.
            play.audio = audio;
            return;
        }
        play.audio = audio;
        if matches!(play.clock, Clock::Host { silent: true, .. }) && !play.audio.sources.is_empty()
        {
            // The project gained sound: the audio clock takes over from here.
            let at = self.clock_us();
            return self.start_playing(at, 1.0);
        }
        let at = self.clock_us();
        let audio = self.playing.as_ref().map(|p| p.audio.clone());
        if let Some(audio) = audio {
            self.p.backend.update_audio(audio, at);
        }
    }
}
