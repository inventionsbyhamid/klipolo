//! How the harness drives the core (`super`'s module doc): every fake pipe's media in time order with a tick each
//! virtual second, and the core's effects carried out on the fakes and the in-memory files.

use std::collections::VecDeque;

use klipolo_capture::cursor::CursorEvent;
use klipolo_capture::media::AudioFormat;
use klipolo_capture::pipe::Sent;
use klipolo_capture::run::core::{Effect, PipeCmd, Started};
use klipolo_capture::synthetic::synthetic_packet;

use super::{FPS, Harness, S, fixture, video_spec};

impl Harness {
    pub fn feed_for(&mut self, seconds: f64) {
        let from = (self.now - self.base) as f64 / 1e9;
        self.feed(from, from + seconds);
    }

    /// Every source's media in `[from_s, to_s)` after the start, in time order, with a tick each whole second.
    pub fn feed(&mut self, from_s: f64, to_s: f64) {
        let (from, to) = (
            self.base + (from_s * 1e9) as u64,
            self.base + (to_s * 1e9) as u64,
        );
        self.now = self.now.max(from);
        while self.step(to) {}
        self.now = to;
    }

    /// The next input before `to`, if there is one.
    pub(super) fn step(&mut self, to: u64) -> bool {
        let next = (0..self.fakes.len())
            .filter_map(|i| self.next_of(i).map(|t| (t, Some(i))))
            .chain([(self.next_tick, None)])
            .min();
        let Some((t, who)) = next.filter(|(t, _)| *t < to) else {
            return false;
        };
        self.now = self.now.max(t);
        match who {
            None => {
                self.next_tick += S;
                self.tick();
            }
            Some(i) if self.fakes[i].video => self.frame(i, t),
            Some(i) => self.block(i, t),
        }
        true
    }

    fn next_of(&self, i: usize) -> Option<u64> {
        let f = &self.fakes[i];
        if !f.started || f.stopped {
            return None;
        }
        Some(if f.video {
            frame_ns(f.next_frame)
        } else {
            f.audio.as_ref()?.next_span().1
        })
    }

    fn frame(&mut self, i: usize, t: u64) {
        let f = &mut self.fakes[i];
        let n = f.next_frame;
        f.next_frame += 1;
        if !f.flowing || t < f.quiet_until {
            return;
        }
        f.delivered += 1;
        if f.paused {
            f.since_key = None;
            return;
        }
        let key = std::mem::take(&mut f.want_key) || f.since_key.is_none_or(|k| k >= FPS as u32);
        let spec = video_spec();
        let packet = synthetic_packet(&spec, n, key, t);
        let id = f.id.clone();
        let mut out = vec![];
        match &mut f.gate {
            Some((gate, rx, stall)) => {
                match gate.send(packet) {
                    Sent::Sent | Sent::Dropped => {
                        f.since_key = Some(if key { 1 } else { f.since_key.unwrap_or(0) + 1 })
                    }
                    Sent::WantKeyframe => f.since_key = None,
                    Sent::Closed => {}
                }
                if *stall > 0 {
                    *stall -= 1;
                } else {
                    out.extend(rx.try_iter());
                }
            }
            None => {
                f.since_key = Some(if key { 1 } else { f.since_key.unwrap_or(0) + 1 });
                out.push(packet);
            }
        }
        for p in out {
            let fx = self.core.packet(&id, p, t);
            self.apply(fx);
        }
        if self.fakes[i].pipe_cursor {
            self.pipe_cursor(i, n, t);
        }
    }

    /// The screen's own cursor (a pipe-sourced one): the area once, a point a frame on a circle every 4 s, the primary
    /// button down for one frame every 2 s.
    fn pipe_cursor(&mut self, i: usize, n: u64, t: u64) {
        let f = &mut self.fakes[i];
        let mut events = Vec::with_capacity(3);
        if !std::mem::replace(&mut f.cursor_area_sent, true) {
            events.push(CursorEvent::Area {
                host_ns: t,
                w_pts: 1920.0,
                h_pts: 1080.0,
            });
        }
        let turn = (n % (FPS * 4)) as f32 / (FPS * 4) as f32 * std::f32::consts::TAU;
        let (x, y) = (0.5 + 0.3 * turn.cos(), 0.5 + 0.3 * turn.sin());
        events.push(CursorEvent::Move { host_ns: t, x, y });
        let pressed = n.is_multiple_of(FPS * 2);
        if pressed != std::mem::replace(&mut f.cursor_pressed, pressed) {
            events.push(CursorEvent::Button {
                host_ns: t,
                btn: 0,
                down: pressed,
                x,
                y,
            });
        }
        for ev in events {
            let fx = self.core.cursor(ev, t);
            self.apply(fx);
        }
    }

    fn block(&mut self, i: usize, t: u64) {
        let f = &mut self.fakes[i];
        let source = f.audio.as_mut().unwrap();
        if !f.flowing {
            source.skip_to(t);
            return;
        }
        let Some(block) = source.next_block(t).filter(|b| !b.samples.is_empty()) else {
            return;
        };
        let id = f.id.clone();
        let fx = self.core.pcm(&id, block, t);
        self.apply(fx);
    }

    /// Carries out the effects as the threads layer would, on the fakes and the in-memory files.
    pub fn apply(&mut self, fx: Vec<Effect>) {
        let mut queue = VecDeque::from(fx);
        while let Some(e) = queue.pop_front() {
            let now = self.now;
            match &e {
                Effect::StartPipes(ids) => {
                    for id in ids {
                        queue.extend(self.start(id, now));
                    }
                }
                Effect::StopPipes => self.fakes.iter_mut().for_each(|f| f.stopped = true),
                Effect::Pipe { source, cmd } => queue.extend(self.pipe(source, *cmd, now)),
                Effect::SwapMic { source, .. } => queue.extend(self.start(source, now)),
                Effect::Open { key, head } => {
                    self.files.insert(key.clone(), head.clone());
                }
                Effect::OpenLines { key } => {
                    self.files.entry(key.clone()).or_default();
                }
                Effect::WriteAtomic { key, bytes } => {
                    self.files.insert(key.clone(), bytes.clone());
                }
                Effect::Append { key, bytes, .. } => self
                    .files
                    .entry(key.clone())
                    .or_default()
                    .extend_from_slice(bytes),
                Effect::Journal(entry) => self.journal.push(entry.clone()),
                Effect::Emit(ev) => self.events.push(ev.clone()),
                Effect::Shell(out) => self.shell_out.push(out.clone()),
                Effect::ReadBack => queue.extend(self.core.read_back(vec![], now)),
                Effect::Close { .. } | Effect::Tick | Effect::Finalize | Effect::Exit(_) => {}
            }
            // The log keeps no media bytes (a long take's would double the test's memory).
            self.effects.push(match e {
                Effect::Append { key, media_us, .. } => Effect::Append {
                    key,
                    bytes: vec![],
                    media_us,
                },
                Effect::Open { key, .. } => Effect::Open { key, head: vec![] },
                Effect::WriteAtomic { key, .. } => Effect::WriteAtomic { key, bytes: vec![] },
                other => other,
            });
        }
    }

    /// A pipe (re)starts: its media flows from now, its first frame a keyframe, and it reports `started`.
    fn start(&mut self, id: &str, now: u64) -> Vec<Effect> {
        let Some(f) = self.fakes.iter_mut().find(|f| f.id == id) else {
            return vec![];
        };
        if let Some(error) = f.start_error.clone() {
            return self.core.failed_to_start(id, error, now);
        }
        (
            f.started,
            f.flowing,
            f.paused,
            f.stopped,
            f.want_key,
            f.since_key,
        ) = (true, true, false, false, true, None);
        f.next_frame = first_frame_at(now);
        f.quiet_until = now + f.hold_ns;
        let started = if f.video {
            Started::Video(fixture().info)
        } else {
            f.audio = Some(f.audio_source(now));
            Started::Audio(AudioFormat {
                rate: 48_000,
                channels: f.channels,
            })
        };
        self.core.started(id, started, now)
    }

    fn pipe(&mut self, id: &str, cmd: PipeCmd, now: u64) -> Vec<Effect> {
        let Some(f) = self.fakes.iter_mut().find(|f| f.id == id) else {
            return vec![];
        };
        match cmd {
            PipeCmd::Pause => f.paused = true,
            PipeCmd::Resume => (f.paused, f.since_key) = (false, None),
            PipeCmd::Keyframe => f.want_key = true,
            PipeCmd::Stop => f.stopped = true,
            PipeCmd::Restart | PipeCmd::Lower => return self.start(id, now),
            // The test answers (`ack_cursor_in_frames`).
            PipeCmd::CursorInFrames => {}
        }
        vec![]
    }
}

fn frame_ns(n: u64) -> u64 {
    (u128::from(n) * 1_000_000_000 / u128::from(FPS)) as u64
}

fn first_frame_at(now: u64) -> u64 {
    (u128::from(now) * u128::from(FPS)).div_ceil(1_000_000_000) as u64
}
