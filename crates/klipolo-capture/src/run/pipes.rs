//! The take's pipes, for the threads layer. Each source's pipe lives on its own controller thread, because a pipe's
//! `start` may block (a system picker) and must never hold the core up (`pipe`'s "Starting"); the run controller also
//! restarts it, swaps the mic, and publishes its counters twice a second for the core's tick. A forwarder thread per
//! stream moves packets, PCM and events into the core's one queue, tagged with the source and the pipe's start number
//! (generation): what an old start still had in flight is dropped, never written into a new run. A screen's cursor
//! comes from its own source (`Backend::cursor_source`, which outlives the pipe's restarts) or from its pipe; its
//! events carry their own times and are never tied to one start, and a full queue drops them, counted, never blocking.
//!
//! The pipes are built from the config on the core's `StartPipes` (`Backend::video`/`audio`, which open nothing), and a
//! video source's own sound (`SourceSpec::AppSound`) comes with its video pipe. Every channel is bounded: a full one
//! backs up to the pipe, whose `PacketGate` (video) or ring (sound) drops and counts there, never blocking a callback.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Sender, SyncSender, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use super::core::PipeCmd;
use super::threads::Input;
use crate::backend::{AudioSpec, Backend, Target, VideoSpec};
use crate::cursor::CursorSource;
use crate::pipe::{AudioPipe, PipeError, VideoPipe, VideoStats};
use crate::preflight::cursor::CursorPlan;
use crate::session::{SessionConfig, SourceSpec};

/// How often a controller publishes its pipe's counters.
pub(super) const STATS_EVERY: Duration = Duration::from_millis(500);
/// Forwarders and controllers only move messages: a small stack each.
pub(super) const SMALL_STACK: usize = 256 * 1024;

pub(super) enum Ctl {
    Start,
    Cmd(PipeCmd),
    Swap(String),
}

pub(super) enum Owned {
    Video {
        pipe: Box<dyn VideoPipe>,
        cursor: CursorFeed,
    },
    Audio(Box<dyn AudioPipe>),
}

/// How a screen's cursor reaches the core.
pub(super) enum CursorFeed {
    None,
    /// The video pipe sends it (`VideoOut::cursor`: portal metadata, DDA).
    Pipe,
    /// Its own source (macOS, Windows' WGC), started with the pipe's first start: it outlives the pipe's restarts.
    Source {
        source: Box<dyn CursorSource>,
        started: bool,
    },
}

pub(super) struct Pipes {
    backend: Arc<dyn Backend>,
    core: SyncSender<Input>,
    specs: Vec<SourceSpec>,
    ids: Vec<String>,
    ctl: Vec<Option<Sender<Ctl>>>,
    starts: Vec<Arc<AtomicU64>>,
    stats: Vec<Arc<Mutex<VideoStats>>>,
    /// Set once the app's Stop, Cancel or Disarm was read, or the pipes were all stopped; each controller checks it
    /// right before a pipe starts.
    stop_seen: Arc<AtomicBool>,
    /// Pipes `start` couldn't build: handed back to the core thread, which runs `start` (a send to its own bounded
    /// queue could wait forever on a full one).
    failures: Vec<(usize, PipeError)>,
    /// The camera also feeds the take's bubble (`SessionConfig::camera_bubble`).
    bubble: bool,
    /// The bubble shows it as in a mirror (`SessionConfig::mirror_camera`).
    bubble_mirrored: bool,
}

impl Pipes {
    pub(super) fn new(
        backend: Arc<dyn Backend>,
        core: SyncSender<Input>,
        cfg: &SessionConfig,
        stop_seen: Arc<AtomicBool>,
    ) -> Self {
        let n = cfg.sources.len();
        Self {
            backend,
            core,
            specs: cfg.sources.iter().map(|s| s.spec.clone()).collect(),
            ids: cfg.sources.iter().map(|s| s.id.clone()).collect(),
            ctl: (0..n).map(|_| None).collect(),
            starts: (0..n).map(|_| Arc::default()).collect(),
            stats: (0..n).map(|_| Arc::default()).collect(),
            stop_seen,
            failures: Vec::new(),
            bubble: cfg.camera_bubble,
            bubble_mirrored: cfg.mirror_camera,
        }
    }

    /// The pipe start that a source's inputs must come from.
    pub(super) fn current(&self, i: usize) -> u64 {
        self.starts[i].load(Ordering::Acquire)
    }

    pub(super) fn index(&self, id: &str) -> Option<usize> {
        self.ids.iter().position(|s| s == id)
    }

    /// The video pipes' latest counters, into `out` (kept by the caller, so a tick allocates nothing once warm).
    pub(super) fn stats_into(&self, out: &mut std::collections::BTreeMap<String, VideoStats>) {
        for (i, spec) in self.specs.iter().enumerate() {
            if matches!(spec, SourceSpec::Screen(_) | SourceSpec::Camera { .. }) {
                let s = *self.stats[i].lock().unwrap_or_else(PoisonError::into_inner);
                match out.get_mut(&self.ids[i]) {
                    Some(v) => *v = s,
                    None => {
                        out.insert(self.ids[i].clone(), s);
                    }
                }
            }
        }
    }

    /// Builds the pipes of `ids` and starts each on its controller. Returns the sources whose pipe couldn't be built
    /// (each with its error), for the core's `failed_to_start`.
    pub(super) fn start(&mut self, ids: &[String]) -> Vec<(usize, PipeError)> {
        for id in ids {
            let Some(i) = self.index(id) else { continue };
            if self.ctl[i].is_some() {
                continue;
            }
            let built = match self.specs[i].clone() {
                SourceSpec::Screen(spec) => self.video(i, &spec),
                SourceSpec::Camera {
                    device,
                    preset,
                    encoders,
                } => {
                    let spec = VideoSpec {
                        target: Target::Camera { id: device },
                        preset,
                        cursor: CursorPlan::None,
                        sound: false,
                        encoders,
                        bubble: self.bubble,
                        bubble_mirrored: self.bubble_mirrored,
                    };
                    self.video(i, &spec)
                }
                SourceSpec::Mic { device } => self
                    .backend
                    .audio(&AudioSpec::Mic { device })
                    .map(|p| self.control(i, Owned::Audio(p))),
                SourceSpec::System => self
                    .backend
                    .audio(&AudioSpec::System)
                    .map(|p| self.control(i, Owned::Audio(p))),
                // Comes with its video source's pipe.
                SourceSpec::AppSound { .. } => Ok(()),
            };
            if let Err(e) = built {
                self.failed(i, e);
            }
        }
        // A video source's sound that its pipe didn't bring can't record.
        for id in ids {
            if let Some(i) = self.index(id)
                && self.ctl[i].is_none()
                && matches!(self.specs[i], SourceSpec::AppSound { .. })
            {
                self.failed(i, PipeError::Unsupported);
            }
        }
        for tx in self.ctl.iter().flatten() {
            let _ = tx.send(Ctl::Start);
        }
        std::mem::take(&mut self.failures)
    }

    fn video(&mut self, i: usize, spec: &VideoSpec) -> Result<(), PipeError> {
        let pipes = self.backend.video(spec)?;
        let cursor = match spec.cursor {
            CursorPlan::None => CursorFeed::None,
            CursorPlan::Burned | CursorPlan::Separate => {
                match self.backend.cursor_source(&spec.target) {
                    Ok(Some(source)) => CursorFeed::Source {
                        source,
                        started: false,
                    },
                    Ok(None) => CursorFeed::Pipe,
                    // No cursor events: a separate cursor's watchdog burns it in.
                    Err(e) => {
                        tracing::warn!(error = %e, "pipes: the cursor's source couldn't be built");
                        CursorFeed::None
                    }
                }
            }
        };
        self.control(
            i,
            Owned::Video {
                pipe: pipes.video,
                cursor,
            },
        );
        if let Some(sound) = pipes.sound {
            let of = &self.ids[i];
            let j = self
                .specs
                .iter()
                .position(|s| matches!(s, SourceSpec::AppSound { of: o } if o == of));
            if let Some(j) = j {
                self.control(j, Owned::Audio(sound));
            }
        }
        Ok(())
    }

    fn failed(&mut self, i: usize, e: PipeError) {
        self.failures.push((i, e));
    }

    pub(super) fn cmd(&self, id: &str, cmd: PipeCmd) {
        if let Some(tx) = self.index(id).and_then(|i| self.ctl[i].as_ref()) {
            let _ = tx.send(Ctl::Cmd(cmd));
        }
    }

    pub(super) fn swap(&self, id: &str, device: String) {
        if let Some(tx) = self.index(id).and_then(|i| self.ctl[i].as_ref()) {
            let _ = tx.send(Ctl::Swap(device));
        }
    }

    /// Every pipe stops (on its controller: a start still blocked in a picker stops once it returns). The stop flag is
    /// set first, so a restart or mic swap already queued ahead of the Stop opens nothing: this covers the Stops the
    /// app's command reader never sees (the menu, the hotkey, power off, `stop_and_save`).
    pub(super) fn stop_all(&mut self) {
        self.stop_seen.store(true, Ordering::Release);
        for tx in self.ctl.iter_mut().filter_map(Option::take) {
            let _ = tx.send(Ctl::Cmd(PipeCmd::Stop));
        }
    }

    fn control(&mut self, i: usize, owned: Owned) {
        let (tx, rx) = channel();
        let own = match &self.specs[i] {
            SourceSpec::Mic { device } => Some(device.clone()),
            _ => None,
        };
        let c = super::controller::Controller::new(
            i,
            owned,
            self.core.clone(),
            self.starts[i].clone(),
            self.stats[i].clone(),
            self.backend.clone(),
            own,
        )
        .stopping_on(self.stop_seen.clone());
        let spawned = std::thread::Builder::new()
            .name(format!("klipolo-pipe-{}", self.ids[i]))
            .stack_size(SMALL_STACK)
            .spawn(move || c.run(rx));
        match spawned {
            Ok(_) => self.ctl[i] = Some(tx),
            Err(e) => self.failed(i, PipeError::Os(format!("a pipe's thread: {e}"))),
        }
    }
}
