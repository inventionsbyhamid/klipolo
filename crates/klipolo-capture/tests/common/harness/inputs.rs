//! What a test does to the take: the app's commands, the shell's inputs, pipe events, ticks with the writer's and the
//! disk's state, and the fakes' behaviour (signals, stalls, a slow first frame).

use std::collections::BTreeMap;

use klipolo_capture::pipe::{PipeEvent, VideoStats};
use klipolo_capture::protocol::Command;
use klipolo_capture::shell::ShellInput;
use klipolo_capture::synthetic::Signal;
use klipolo_capture::writer::WriterSnapshot;

use super::Harness;

impl Harness {
    /// The source goes quiet without its pipe saying so (only the stall rule can tell).
    pub fn go_quiet(&mut self, source: &str) {
        self.fakes
            .iter_mut()
            .find(|f| f.id == source)
            .unwrap()
            .flowing = false;
    }

    /// The source's pipe fails to start, every time, with `error`.
    pub fn fails_to_start(&mut self, source: &str, error: klipolo_capture::pipe::PipeError) {
        let f = self.fakes.iter_mut().find(|f| f.id == source).unwrap();
        f.start_error = Some(error);
    }

    /// A pipe's event to the core, the fake's media left as it is. A stop or a stall comes with the display's state
    /// now, as the threads layer asks the backend.
    pub fn pipe_says(&mut self, source: &str, event: PipeEvent) {
        if matches!(event, PipeEvent::StoppedBySystem | PipeEvent::Stalled) {
            self.core.display(self.display_awake);
        }
        let fx = self.core.pipe_event(source, event, self.now);
        self.apply(fx);
    }

    /// What the source sends from now (it goes on with its device frames).
    pub fn set_signal(&mut self, source: &str, signal: Signal) {
        let now = self.now;
        let f = self.fakes.iter_mut().find(|f| f.id == source).unwrap();
        f.signal = signal;
        if f.audio.is_some() {
            f.audio = Some(f.audio_source(now));
        }
    }

    /// After each (re)start, the source sends nothing for this long (a slow camera's first frame).
    pub fn first_frame_after(&mut self, source: &str, seconds: f64) {
        self.fakes
            .iter_mut()
            .find(|f| f.id == source)
            .unwrap()
            .hold_ns = (seconds * 1e9) as u64;
    }

    pub fn clear(&mut self) {
        self.events.clear();
        self.effects.clear();
        self.shell_out.clear();
    }

    pub fn mic_signal_zeros(&mut self) {
        for f in self.fakes.iter_mut().filter(|f| f.id == "mic") {
            f.signal = Signal::Zeros;
        }
    }

    pub fn free_bytes(&mut self, n: u64) {
        self.free = Some(n);
    }

    pub fn queued_ms(&mut self, n: u32) {
        self.queued_ms = n;
    }

    /// The screen's next `frames` frames aren't drained from its gate's channel of 2.
    pub fn stall_packets(&mut self, source: &str, frames: u32) {
        let f = self.fakes.iter_mut().find(|f| f.id == source).unwrap();
        f.gate.as_mut().expect("the screen goes through a gate").2 = frames;
    }

    pub fn command(&mut self, cmd: Command) {
        let fx = self.core.command(cmd, self.now);
        self.apply(fx);
    }

    pub fn shell(&mut self, input: ShellInput) {
        let fx = self.core.shell(input, self.now);
        self.apply(fx);
    }

    /// A system alert is on the screen now (`up`), or not, as the threads layer reads it from the backend.
    pub fn system_alert(&mut self, up: bool) {
        let fx = self.core.system_alert(up, self.now);
        self.apply(fx);
    }

    /// A pipe's event: it also stops or starts the fake's media, as the fake backend's pipes do.
    pub fn pipe_event(&mut self, source: &str, event: PipeEvent) {
        if let Some(f) = self.fakes.iter_mut().find(|f| f.id == source) {
            match event {
                PipeEvent::DeviceBack | PipeEvent::TargetBack => f.flowing = true,
                PipeEvent::Blank(_) | PipeEvent::Note(_) | PipeEvent::RestoreToken(_) => {}
                _ => f.flowing = false,
            }
        }
        if matches!(event, PipeEvent::StoppedBySystem | PipeEvent::Stalled) {
            self.core.display(self.display_awake);
        }
        let fx = self.core.pipe_event(source, event, self.now);
        self.apply(fx);
    }

    pub fn tick(&mut self) {
        let bytes = self.files.values().map(|f| f.len() as u64).sum();
        let snap = WriterSnapshot {
            queued_ms: self.queued_ms,
            bytes,
            ..WriterSnapshot::default()
        };
        let stats: BTreeMap<String, VideoStats> = self
            .fakes
            .iter()
            .filter(|f| f.video)
            .map(|f| {
                let dropped = f.gate.as_ref().map_or(0, |g| g.0.dropped());
                (
                    f.id.clone(),
                    VideoStats {
                        delivered: f.delivered,
                        complete: f.delivered,
                        dropped_channel: dropped,
                        ..VideoStats::default()
                    },
                )
            })
            .collect();
        let fx = self
            .core
            .tick(self.now, &snap, self.free, &stats, self.display_awake);
        self.apply(fx);
    }
}
