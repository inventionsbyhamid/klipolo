//! A pipe's controller thread (`pipes`): it starts, restarts, pauses and stops its pipe, swaps the mic, and publishes
//! the pipe's counters; a forwarder per stream moves what the pipe sends into the core's queue.
//!
//! A mic source's controller also looks for the person's own mic while the take records on another, or after a start
//! on it failed because it wasn't there (`own_mic`). Once the app's Stop, Cancel or Disarm was read, no pipe starts:
//! the check is right before each `start`, so a start already asked for, a heal restart or a mic swap opens nothing
//! after the person's Stop.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use klipolo_platform::qos::{self, Qos};

use super::effect::{PipeCmd, Started};
use super::own_mic::OwnMic;
use super::pipes::{Ctl, Owned, SMALL_STACK, STATS_EVERY};
use super::threads::Input;
use crate::backend::{AudioSpec, Backend};
use crate::pipe::{
    AudioOut, EVENT_CHANNEL, PACKET_CHANNEL, PCM_CHANNEL, PipeError, StopFlag, VideoOut, VideoStats,
};

mod cursor;
#[cfg(all(test, feature = "synthetic"))]
mod tests;

pub(super) struct Controller {
    i: usize,
    owned: Owned,
    core: SyncSender<Input>,
    start_no: Arc<AtomicU64>,
    stats: Arc<Mutex<VideoStats>>,
    backend: Arc<dyn Backend>,
    running: bool,
    /// A mic source's own device and the look for it (`own_mic`).
    own: Option<OwnMic>,
    /// The app's Stop, Cancel or Disarm was read: nothing opens from then on.
    stop_seen: Arc<AtomicBool>,
    /// Cursor events a full core queue dropped, and how many the log has told.
    cursor_dropped: Arc<AtomicU64>,
    cursor_dropped_told: u64,
}

impl Controller {
    pub(super) fn new(
        i: usize,
        owned: Owned,
        core: SyncSender<Input>,
        start_no: Arc<AtomicU64>,
        stats: Arc<Mutex<VideoStats>>,
        backend: Arc<dyn Backend>,
        own: Option<String>,
    ) -> Self {
        Self {
            i,
            owned,
            core,
            start_no,
            stats,
            backend,
            running: false,
            own: own.map(OwnMic::new),
            stop_seen: Arc::default(),
            cursor_dropped: Arc::default(),
            cursor_dropped_told: 0,
        }
    }

    /// Shares the session's stop flag (`run::prepare::Commands::stop_seen`).
    pub(super) fn stopping_on(mut self, stop_seen: Arc<AtomicBool>) -> Self {
        self.stop_seen = stop_seen;
        self
    }

    pub(super) fn run(mut self, rx: Receiver<Ctl>) {
        qos::set_current(Qos::UserInteractive);
        loop {
            match rx.recv_timeout(STATS_EVERY) {
                Ok(Ctl::Start | Ctl::Cmd(PipeCmd::Restart)) => self.start(),
                Ok(Ctl::Cmd(PipeCmd::Lower)) => {
                    if let Owned::Video { pipe, .. } = &mut self.owned
                        && !pipe.lower_preset()
                    {
                        tracing::info!(
                            "pipes: nothing lighter, so the encoder is rebuilt as it was"
                        );
                    }
                    self.start();
                }
                Ok(Ctl::Cmd(PipeCmd::Pause)) => self.paused(true),
                Ok(Ctl::Cmd(PipeCmd::Resume)) => self.paused(false),
                Ok(Ctl::Cmd(PipeCmd::CursorInFrames)) => self.cursor_in_frames(),
                Ok(Ctl::Cmd(PipeCmd::Keyframe)) => {
                    if let Owned::Video { pipe, .. } = &mut self.owned {
                        pipe.request_keyframe();
                    }
                }
                Ok(Ctl::Swap(device)) => {
                    if let Some(own) = &mut self.own {
                        own.swapped(&device, Instant::now());
                    }
                    self.stop();
                    match self.backend.audio(&AudioSpec::Mic { device }) {
                        Ok(pipe) => {
                            self.owned = Owned::Audio(pipe);
                            self.start();
                        }
                        Err(e) => {
                            self.after_start(Some(&e));
                            self.report(Err(e));
                        }
                    }
                }
                Ok(Ctl::Cmd(PipeCmd::Stop)) | Err(RecvTimeoutError::Disconnected) => {
                    self.stop();
                    self.stop_cursor();
                    return;
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
            self.publish_stats();
            self.look_for_own();
        }
    }

    /// The look for the person's own mic, when one is due (`own_mic`): what it says goes to the core from the current
    /// start.
    fn look_for_own(&mut self) {
        let Some(own) = &mut self.own else {
            return;
        };
        let backend = &self.backend;
        let said = own.look(Instant::now(), |id| {
            backend.mics().iter().any(|m| m.id == id)
        });
        if let Some(event) = said {
            let start_no = self.start_no.load(Ordering::Acquire);
            let _ = self.core.send(Input::Event {
                src: self.i,
                start_no,
                event,
            });
        }
    }

    fn after_start(&mut self, failed: Option<&PipeError>) {
        if let Some(own) = &mut self.own {
            own.started(failed, Instant::now());
        }
    }

    /// (Re)starts the pipe as a new generation, with fresh channels and forwarders.
    fn start(&mut self) {
        self.stop();
        let start_no = self.start_no.fetch_add(1, Ordering::AcqRel) + 1;
        let (i, core) = (self.i, &self.core);
        let (events, events_rx) = sync_channel(EVENT_CHANNEL);
        forward(events_rx, core.clone(), move |event| Input::Event {
            src: i,
            start_no,
            event,
        });
        let result = match &mut self.owned {
            Owned::Video { pipe, cursor } => {
                let (packets, packets_rx) = sync_channel(PACKET_CHANNEL);
                forward(packets_rx, core.clone(), move |packet| Input::Packet {
                    src: i,
                    start_no,
                    packet,
                });
                if self.stop_seen.load(Ordering::Acquire) {
                    tracing::info!("pipes: a Stop was read, so the screen or camera stays closed");
                    return;
                }
                let cursor = cursor::wire(cursor, core, &self.cursor_dropped);
                pipe.start(VideoOut {
                    packets,
                    cursor,
                    events,
                    stopping: StopFlag::new(self.stop_seen.clone()),
                })
                .map(Started::Video)
            }
            Owned::Audio(pipe) => {
                let (blocks, blocks_rx) = sync_channel(PCM_CHANNEL);
                forward(blocks_rx, core.clone(), move |block| Input::Pcm {
                    src: i,
                    start_no,
                    block,
                });
                if self.stop_seen.load(Ordering::Acquire) {
                    tracing::info!("pipes: a Stop was read, so the sound device stays closed");
                    return;
                }
                let stopping = StopFlag::new(self.stop_seen.clone());
                (pipe.start(AudioOut {
                    blocks,
                    events,
                    stopping,
                }))
                .map(Started::Audio)
            }
        };
        self.running = result.is_ok();
        self.after_start(result.as_ref().err());
        self.report(result);
    }

    fn report(&self, result: Result<Started, PipeError>) {
        let start_no = self.start_no.load(Ordering::Acquire);
        let _ = self.core.send(Input::Started {
            src: self.i,
            start_no,
            result,
        });
    }

    fn paused(&mut self, paused: bool) {
        if let Owned::Video { pipe, .. } = &mut self.owned {
            pipe.set_paused(paused);
        }
        self.cursor_paused(paused);
    }

    fn stop(&mut self) {
        if std::mem::take(&mut self.running) {
            match &mut self.owned {
                Owned::Video { pipe, .. } => pipe.stop(),
                Owned::Audio(pipe) => pipe.stop(),
            }
        }
    }
}

/// Moves what `rx` gets into the core's queue until either end closes; a full queue waits (the pipe's own bounded
/// channel then fills, and the pipe drops and counts there).
fn forward<T: Send + 'static>(
    rx: Receiver<T>,
    core: SyncSender<Input>,
    wrap: impl Fn(T) -> Input + Send + 'static,
) {
    let spawned = std::thread::Builder::new()
        .name("klipolo-forward".into())
        .stack_size(SMALL_STACK)
        .spawn(move || {
            qos::set_current(Qos::UserInteractive);
            for item in rx {
                if core.send(wrap(item)).is_err() {
                    return;
                }
            }
        });
    if let Err(e) = spawned {
        tracing::error!(error = %e, "pipes: a forwarder didn't start");
    }
}
