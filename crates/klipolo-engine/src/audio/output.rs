//! Preview audio output. One thread (`klipolo-audio-out`, user-interactive) opens the default OUTPUT device,
//! owns the cpal stream (`Stream` is !Send, so it never leaves that thread) and keeps ~250 ms of mixed sound ahead of
//! the callback. Every device operation (open, reposition, pause, close) runs on that thread: `AudioOutput` only
//! sends it messages, so the engine actor never waits on the device. The thread reports through `Status` and wakes
//! the actor when sound for a new position is mapped onto the clock, or when the device fails.
//!
//! Pause stops the stream but keeps the device for a grace period, so playing again skips the device open (and a
//! Bluetooth wake). Dropping the handle ends the thread on its own; nobody joins it.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::StreamTrait;
use klipolo_model::time::Us;
use tokio::sync::Notify;

use super::clock::{AudioClock, now_ns};
use super::mix::RATE;
use super::pcm::{PcmStore, lock};
use super::plan::AudioPlan;
use super::route::{self, RouteWatch, hardware_latency};
use super::stream::{Callback, build_stream, default_output, pick_config};
use mixer::Mixer;

/// Sound kept mixed ahead of the callback.
const AHEAD: Duration = Duration::from_millis(250);
/// Frames mixed per block.
const BLOCK: usize = 1024;
/// How often the mixer tops the ring up (50 wake-ups a second; the ring holds 250 ms).
const TOP_UP: Duration = Duration::from_millis(20);
/// A device can take a while to start (Bluetooth wakes up); past this, playback goes on without sound.
const START_TIMEOUT: Duration = Duration::from_secs(2);
/// A running callback answers a flush within one buffer.
const FLUSH_TIMEOUT: Duration = Duration::from_millis(500);
/// A paused output keeps its device this long, then lets it go.
const GRACE: Duration = Duration::from_secs(30);
/// A pause waits this long for the callback to fade out what is playing before stopping the stream.
const PAUSE_FADE: Duration = Duration::from_millis(50);

mod mixer;
#[cfg(test)]
mod tests;

enum Control {
    /// Play `plan` from `from_us` (start, resume or a live seek).
    Play {
        plan: Arc<AudioPlan>,
        from_us: Us,
        request: u64,
    },
    /// An edit while playing: the same position with the new plan.
    Plan(Arc<AudioPlan>),
    Pause,
}

/// What the output thread reports.
pub struct Status {
    alive: AtomicBool,
    /// Set (with this lock held) when the thread stops taking messages; a send checks it under the same lock, so a
    /// message is either handled or refused, never lost as the grace period ends.
    closed: Mutex<bool>,
    failure: Mutex<Option<String>>,
    wake: Arc<Notify>,
}

impl Status {
    fn close(&self) {
        *lock(&self.closed) = true;
        self.alive.store(false, Ordering::Release);
    }

    fn fail(&self, message: String) {
        tracing::warn!(%message, "audio output");
        *lock(&self.failure) = Some(message);
        self.close();
        self.wake.notify_one();
    }
}

/// The gain on the whole preview mix, which the output's mixer reads every block (`shared`, f32 bits): the loudness
/// target's gain times the person's listening volume (the player bar's). Export never reads it.
pub struct MasterGain {
    loudness: f32,
    volume: f32,
    shared: Arc<AtomicU32>,
}

impl Default for MasterGain {
    fn default() -> Self {
        Self {
            loudness: 1.0,
            volume: 1.0,
            shared: Arc::new(AtomicU32::new(1f32.to_bits())),
        }
    }
}

impl MasterGain {
    /// What the mixer reads.
    pub fn shared(&self) -> Arc<AtomicU32> {
        self.shared.clone()
    }

    /// The loudness target's gain (1.0: none); the volume stays.
    pub fn set_loudness(&mut self, gain: f32) {
        self.loudness = gain;
        self.publish();
    }

    /// The listening volume, held to 0 (silent) to 1 (as measured); the loudness gain stays.
    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
        self.publish();
    }

    fn publish(&self) {
        self.shared
            .store((self.loudness * self.volume).to_bits(), Ordering::Relaxed);
    }
}

/// The Send handle the GPU backend holds.
pub struct AudioOutput {
    control: Sender<Control>,
    status: Arc<Status>,
    clock: Arc<AudioClock>,
}

impl AudioOutput {
    /// Starts opening the default output device on its own thread; returns at once. `gain` is the master gain (f32
    /// bits); `wake` is notified when sound is mapped onto the clock or the device fails.
    pub fn open(store: Arc<PcmStore>, gain: Arc<AtomicU32>, wake: Arc<Notify>) -> AudioOutput {
        let (control, rx) = mpsc::channel();
        let status = Arc::new(Status {
            alive: AtomicBool::new(true),
            closed: Mutex::new(false),
            failure: Mutex::new(None),
            wake,
        });
        let clock = Arc::new(AudioClock::new(RATE));
        let (thread_status, thread_clock) = (status.clone(), clock.clone());
        let spawned = std::thread::Builder::new()
            .name("klipolo-audio-out".into())
            .spawn(move || run(rx, &thread_status, thread_clock, store, gain));
        if let Err(e) = spawned {
            status.fail(format!("audio thread: {e}"));
        }
        AudioOutput {
            control,
            status,
            clock,
        }
    }

    /// Plays `plan` from `from_us`; the clock answers for `request` once that sound is mapped. False: this output
    /// has closed (its grace period ran out), so open another.
    pub fn play(&self, plan: Arc<AudioPlan>, from_us: Us, request: u64) -> bool {
        self.send(Control::Play {
            plan,
            from_us,
            request,
        })
    }

    /// An edit while playing: keep going from where the sound is, with the new plan.
    pub fn set_plan(&self, plan: Arc<AudioPlan>) {
        self.send(Control::Plan(plan));
    }

    pub fn pause(&self) {
        self.send(Control::Pause);
    }

    /// Timeline time heard now, once `request`'s sound is mapped (see `AudioClock::timeline_us`).
    pub fn time_us(&self, request: u64) -> Option<Us> {
        self.clock.timeline_us(request, now_ns())
    }

    /// Why the device stopped, if it failed.
    pub fn failure(&self) -> Option<String> {
        lock(&self.status.failure).clone()
    }

    /// False once the thread has ended (failed, or the grace period after a pause ran out).
    pub fn is_alive(&self) -> bool {
        self.status.alive.load(Ordering::Acquire)
    }

    fn send(&self, msg: Control) -> bool {
        let closed = lock(&self.status.closed);
        let sent = !*closed && self.control.send(msg).is_ok();
        drop(closed);
        if !sent {
            self.status.alive.store(false, Ordering::Release);
        }
        sent
    }
}

fn run(
    rx: Receiver<Control>,
    status: &Status,
    clock: Arc<AudioClock>,
    store: Arc<PcmStore>,
    gain: Arc<AtomicU32>,
) {
    crate::qos::set_current(crate::qos::Qos::UserInteractive);
    let opened = (|| {
        let device = default_output()?;
        let config = pick_config(&device)?;
        let (rate, channels) = (
            config.sample_rate().0.max(1),
            usize::from(config.channels()),
        );
        clock.set_rate(rate);
        let (mixer, cons, shared) = Mixer::new(store, rate, clock.clone(), gain);
        let callback = Callback::new(cons, shared, clock.clone(), channels);
        let stream = build_stream(&device, &config, callback)?;
        Ok::<_, String>((stream, mixer))
    })();
    let (stream, mut mixer) = match opened {
        Ok(opened) => opened,
        Err(e) => return status.fail(e),
    };
    let mut route = RouteWatch::new();
    let mut seen = route::changes();
    clock.set_hardware_ns(hardware_latency().as_nanos() as u64);
    let mut playing = false;
    loop {
        let msg = match rx.recv_timeout(if playing { TOP_UP } else { GRACE }) {
            Ok(msg) => Some(msg),
            Err(RecvTimeoutError::Timeout) if playing => None,
            // The grace period is over: close, unless a message slipped in meanwhile (checked under the lock sends
            // take, so none is lost).
            Err(RecvTimeoutError::Timeout) => {
                let mut closed = lock(&status.closed);
                match rx.try_recv() {
                    Ok(msg) => Some(msg),
                    Err(_) => {
                        *closed = true;
                        break;
                    }
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
        match msg {
            Some(Control::Play {
                plan,
                from_us,
                request,
            }) => {
                // A stream just started (or resumed) may take a while to ask for sound (a device waking up).
                let wait = if playing {
                    FLUSH_TIMEOUT
                } else {
                    START_TIMEOUT
                };
                if !playing {
                    if let Err(e) = stream.play() {
                        return status.fail(format!("output stream: {e}"));
                    }
                    playing = true;
                }
                mixer.set_plan(plan);
                if let Err(e) = mixer.seek(from_us, request, wait) {
                    return status.fail(e);
                }
                status.wake.notify_one();
            }
            Some(Control::Plan(plan)) => {
                mixer.set_plan(plan);
                if playing && let Err(e) = mixer.replan() {
                    return status.fail(e);
                }
            }
            Some(Control::Pause) => {
                if playing {
                    // Fade out what is playing, and keep what is queued from playing on resume.
                    mixer.pause(PAUSE_FADE);
                    let _ = stream.pause();
                }
                playing = false;
            }
            None => {}
        }
        if playing {
            if mixer.shared.failed.load(Ordering::Acquire) {
                return status.fail("The sound device stopped.".into());
            }
            let now = route::changes();
            if now != seen {
                // AirPods mid-play: follow the new device's latency (the clock re-anchors from the next read).
                seen = now;
                route.follow();
                clock.set_hardware_ns(hardware_latency().as_nanos() as u64);
            }
            mixer.top_up();
        }
    }
    status.close();
    let _ = stream.pause();
    drop(stream);
}
