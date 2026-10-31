//! The fake backend's pipes as `FakeBackend` builds them: each with its source key's start delay, scripted events and
//! start error, on the backend's one schedule; a mic pipe with the spans the script unplugs its device for.

use klipolo_platform::clock::host_ns;

use super::FakeBackend;
use crate::pipe::PipeEvent;
use crate::preflight::cursor::CursorPlan;
use crate::synthetic::audio::{FakeAudioPipe, SyntheticAudioSpec};
use crate::synthetic::packets::SyntheticVideoSpec;
use crate::synthetic::video::FakeVideoPipe;

impl FakeBackend {
    pub(super) fn video_pipe(
        &self,
        key: &str,
        spec: SyntheticVideoSpec,
        cursor: CursorPlan,
    ) -> FakeVideoPipe {
        let mut pipe = FakeVideoPipe::new(spec, self.epoch_ns);
        pipe.fixture = self.fixture.clone();
        pipe.live_after_ms = self.live_after(key);
        pipe.events = self.events_of(key);
        pipe.start_error = self.script.start_errors.get(key).cloned();
        (pipe.cursor, pipe.live_switch) = (cursor, self.script.live_switch);
        pipe.pipe_cursor = cursor != CursorPlan::None && self.script.cursor.is_none();
        pipe.plans = (cursor != CursorPlan::None).then(|| self.cursor_plans.clone());
        pipe.starts = Some(self.starts.clone());
        pipe
    }

    pub(super) fn audio_pipe(&self, key: &str, channels: u16) -> FakeAudioPipe {
        let spec = SyntheticAudioSpec {
            rate: 48_000,
            channels,
            block_ms: 10,
            drift_ppm: 0.0,
            signal: self.script.sound.clone(),
            blip_every_ms: Some(1_000),
        };
        let mut pipe = FakeAudioPipe::new(spec, self.epoch_ns);
        pipe.starts = Some(self.starts.clone());
        pipe.live_after_ms = self.live_after(key);
        pipe.events = self.events_of(key);
        pipe.start_error = self.script.start_errors.get(key).cloned();
        pipe
    }

    pub(super) fn live_after(&self, key: &str) -> u32 {
        self.script.live_after_ms.get(key).copied().unwrap_or(0)
    }

    pub(super) fn events_of(&self, key: &str) -> Vec<(u32, PipeEvent)> {
        self.script
            .events
            .iter()
            .filter(|(_, k, _)| k == key)
            .map(|(ms, _, e)| (*ms, e.clone()))
            .collect()
    }

    /// The mics the script unplugged at this moment (`Script::unplugged`).
    pub(super) fn unplugged_now(&self) -> impl Iterator<Item = &(String, u32, u32)> {
        let now = host_ns().saturating_sub(self.epoch_ns) / 1_000_000;
        let now = u32::try_from(now).unwrap_or(u32::MAX);
        (self.script.unplugged.iter()).filter(move |(_, from, to)| (*from..*to).contains(&now))
    }

    /// A mic pipe on `device`, with the spans the script unplugs it for, on the host clock (`FakeAudioPipe::unplugged`).
    pub(super) fn mic_pipe(&self, device: &str) -> FakeAudioPipe {
        let mut pipe = self.audio_pipe("mic", 1);
        let at = |ms: u32| self.epoch_ns + u64::from(ms) * 1_000_000;
        pipe.unplugged = (self.script.unplugged.iter())
            .filter(|(d, _, _)| d == device)
            .map(|&(_, from, to)| (at(from), at(to)))
            .collect();
        pipe.unplugged_error = self.script.unplugged_error.clone();
        pipe
    }
}
