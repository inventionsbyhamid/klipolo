//! Synthetic sources (feature `synthetic`): what tests and the kill-sweep example record instead of devices.
//! - `packets`: encoded packets that carry their frame number and a flash flag, and a well-formed `avcC`/`hvcC`;
//! - `video`: the fake backend's video pipe, which sends them (or a fixture's) on a schedule;
//! - `audio`: PCM on a drifting device clock, with known levels and sync blips;
//! - `backend`: `FakeBackend`, whose pipes run in real time on threads, from a `Script` (`script`), and Check camera's
//!   fake preview (`preview`);
//! - `cursor`: `SyntheticCursor`, a screen's cursor source following a `CursorScript`;
//! - `shell`: `FakeShell`, which sends scripted inputs and collects the outputs;
//! - `fixture`: `FixtureVideo`, real encoded packets from a committed test file, which decode.
//!
//! The flash-and-blip sync schedule follows the design of Cap's `crates/recording/src/test_sources/{config,sync,
//! validation}.rs` at d327051 (design only, no code). Ours emits
//! encoded packets and PCM, not raw frames, so the muxers, the writer and recovery need no encoder.

pub mod audio;
pub mod backend;
pub mod cursor;
pub mod fixture;
pub mod packets;
mod preview;
pub mod script;
pub mod shell;
pub mod video;

pub use audio::*;
pub use backend::*;
pub use cursor::*;
pub use fixture::*;
pub use packets::*;
pub use preview::FAKE_DISPLAY;
pub use script::*;
pub use shell::*;

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::thread::JoinHandle;
use std::time::Duration;

use klipolo_platform::clock::host_ns;

use crate::pipe::PipeEvent;

/// A fake pipe's thread: `stop` ends it at once (it parks between frames and blocks, never busy-waits).
struct Worker {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    fn spawn(name: &str, body: impl FnOnce(&AtomicBool) + Send + 'static) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || body(&flag))
            .expect("spawn a fake pipe's thread");
        Self {
            stop,
            thread: Some(thread),
        }
    }

    fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            let _ = thread.join();
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Sleeps until `due_ns` on the host clock; false when `stop` was set first.
fn wait_until(due_ns: u64, stop: &AtomicBool) -> bool {
    loop {
        if stop.load(Ordering::SeqCst) {
            return false;
        }
        let now = host_ns();
        if now >= due_ns {
            return true;
        }
        std::thread::park_timeout(Duration::from_nanos(due_ns - now));
    }
}

/// A pipe's scripted events on the host clock, and whether its media flows: a lost device, a stop or a gone target
/// stops it until the device or the target is back.
struct Scripted {
    events: VecDeque<(u64, PipeEvent)>,
    flowing: bool,
}

impl Scripted {
    /// `events`: (ms after `start_ns`, event).
    fn new(start_ns: u64, mut events: Vec<(u32, PipeEvent)>) -> Self {
        events.sort_by_key(|(ms, _)| *ms);
        let events = events
            .into_iter()
            .map(|(ms, e)| (start_ns + u64::from(ms) * 1_000_000, e))
            .collect();
        Self {
            events,
            flowing: true,
        }
    }

    fn next_ns(&self) -> u64 {
        self.events.front().map_or(u64::MAX, |(at, _)| *at)
    }

    /// Sends the events due by `now`; true when media starts flowing again (a video pipe then sends a keyframe).
    fn deliver(&mut self, now: u64, out: &SyncSender<PipeEvent>) -> bool {
        let mut back = false;
        while self.events.front().is_some_and(|(at, _)| *at <= now) {
            let (_, event) = self.events.pop_front().expect("a due event");
            match event {
                PipeEvent::DeviceBack | PipeEvent::TargetBack => {
                    back |= !self.flowing;
                    self.flowing = true;
                }
                PipeEvent::Stalled
                | PipeEvent::DeviceLost
                | PipeEvent::StoppedBySystem
                | PipeEvent::StoppedByUser
                | PipeEvent::PermissionRevoked
                | PipeEvent::TargetGone
                | PipeEvent::EncoderFailed { .. } => self.flowing = false,
                PipeEvent::Blank(_) | PipeEvent::Note(_) | PipeEvent::RestoreToken(_) => {}
            }
            let _ = out.try_send(event);
        }
        back
    }
}
