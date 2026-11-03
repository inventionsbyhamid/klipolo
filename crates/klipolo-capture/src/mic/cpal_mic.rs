//! The microphone on macOS (CoreAudio) and Windows (WASAPI), through Cap's cpal fork (the engine's pin: macOS streams
//! stop and release the device on drop). Built only with `real-capture`, on those two OSes.
//!
//! **Consent.** `new` asks the guard and keeps the device's name: nothing is listed, opened or asked until `start`, which
//! runs only on the person's Record press. `start` finds the device by name with the host's unfiltered device list (its
//! names come from the OS's properties, opening nothing), and only then reads that one device's default input config
//! and builds its input stream.
//!
//! **Threads.** cpal's stream can't move between threads, so `start` runs it on its own thread, which also pumps the
//! blocks to the session (`ring::MicPump`) every 10 ms at user-interactive QoS. The device's callback only fills the
//! rings (`ring::MicFeed`): real-time safe. `stop` ends the thread: the stream is dropped first (the device is released
//! and no callback follows), then the blocks it captured are handed on, so nothing is sent after `stop` returns.
//!
//! **Faults.** The error callback only marks the shared fault, which the pump sends as one event: the device going away
//! is `DeviceLost` (the heal policy waits 3 s, then falls back to the built-in mic; the take goes on), any other stream
//! error `StoppedBySystem` (a restart). After a loss the thread looks for the device by name once a second (`listed`:
//! the host's device list, names only, opening nothing, and only inside a take the person started) and sends
//! `DeviceBack` when it is there again, so the session restarts on the person's own mic. Raw mode on
//! Windows (`AUDCLNT_STREAMOPTIONS_RAW`) isn't exposed by cpal yet.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{SyncSender, sync_channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{DefaultStreamConfigError, SampleFormat, SizedSample};
use klipolo_platform::clock::host_ns;
use klipolo_platform::qos::{self, Qos};

use super::ring::{self, Flow, MicFeed, MicPump, PUMP_TICK, Shared};
use super::{i16_to_f32, i32_to_f32};
use crate::guard::assert_real_capture_allowed;
use crate::hosttime;
use crate::media::AudioFormat;
use crate::pipe::{AudioOut, AudioPipe, AudioStats, PipeError};

/// One microphone, by the name its OS gives it (the backend maps `AudioSpec::Mic`'s id to it).
pub struct CpalMic {
    name: String,
    shared: Arc<Shared>,
    running: Option<Running>,
}

struct Running {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

impl CpalMic {
    pub fn new(device_name: &str) -> Result<Self, PipeError> {
        assert_real_capture_allowed("the microphone");
        Ok(Self {
            name: device_name.to_owned(),
            shared: Arc::default(),
            running: None,
        })
    }
}

impl AudioPipe for CpalMic {
    /// Opens the device and starts it (a restart when it was running). Blocks until the stream runs or fails.
    fn start(&mut self, out: AudioOut) -> Result<AudioFormat, PipeError> {
        assert_real_capture_allowed("the microphone");
        self.stop();
        let (ready_tx, ready_rx) = sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let (name, flag) = (self.name.clone(), stop.clone());
        let thread = std::thread::Builder::new()
            .name("klipolo-mic".into())
            .spawn(move || run(&name, out, &flag, ready_tx))
            .map_err(|e| PipeError::Os(format!("microphone thread: {e}")))?;
        match ready_rx.recv() {
            Ok(Ok((format, shared))) => {
                self.shared = shared;
                self.running = Some(Running { stop, thread });
                Ok(format)
            }
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                let _ = thread.join();
                Err(PipeError::Os("the microphone thread ended".into()))
            }
        }
    }

    fn stop(&mut self) {
        if let Some(running) = self.running.take() {
            running.stop.store(true, Ordering::Release);
            running.thread.thread().unpark();
            let _ = running.thread.join();
        }
    }

    fn stats(&self) -> AudioStats {
        self.shared.stats()
    }
}

impl Drop for CpalMic {
    fn drop(&mut self) {
        self.stop();
    }
}

type Ready = SyncSender<Result<(AudioFormat, Arc<Shared>), PipeError>>;

/// The mic's thread: opens the stream, reports, then pumps until `stop` or the session goes.
fn run(name: &str, out: AudioOut, stop: &AtomicBool, ready: Ready) {
    qos::set_current(Qos::UserInteractive);
    // The person's Stop may have come while this thread was starting: then nothing opens.
    if out.stopping.is_set() {
        let _ = ready.send(Err(PipeError::Os("stopped before the mic opened".into())));
        return;
    }
    let (stream, mut pump, shared) = match open(name) {
        Ok(opened) => opened,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    if ready.send(Ok((pump.format(), shared))).is_err() {
        return;
    }
    while !stop.load(Ordering::Acquire) {
        if pump.tick(&out) == Flow::Closed
            || pump.watch(&out, Instant::now(), || listed(name)) == Flow::Closed
        {
            break;
        }
        std::thread::park_timeout(PUMP_TICK);
    }
    // Releases the device; no callback runs after this.
    drop(stream);
    pump.finish(&out);
}

/// Finds the device by name, then reads its config and starts its input stream: the first moment anything is opened.
fn open(name: &str) -> Result<(cpal::Stream, MicPump, Arc<Shared>), PipeError> {
    let (device, config) = find(name)?;
    if config.channels() == 0 {
        return Err(PipeError::Unsupported);
    }
    let (feed, pump, shared) = ring::handoff(config.sample_rate().0, config.channels());
    let stream_config = config.config();
    let late = input_latency(name);
    let built = |feed| Built {
        feed,
        late,
        shared: shared.clone(),
    };
    let stream = match config.sample_format() {
        SampleFormat::F32 => build::<f32>(&device, &stream_config, built(feed), |s| s),
        SampleFormat::I16 => build::<i16>(&device, &stream_config, built(feed), i16_to_f32),
        SampleFormat::I32 => build::<i32>(&device, &stream_config, built(feed), i32_to_f32),
        other => {
            tracing::warn!(format = ?other, "microphone: a sample format Klipolo doesn't record");
            return Err(PipeError::Unsupported);
        }
    }
    .map_err(|e| match e {
        cpal::BuildStreamError::DeviceNotAvailable => PipeError::NoDevice,
        cpal::BuildStreamError::StreamConfigNotSupported
        | cpal::BuildStreamError::InvalidArgument => PipeError::Unsupported,
        other => PipeError::Os(other.to_string()),
    })?;
    stream.play().map_err(|e| match e {
        cpal::PlayStreamError::DeviceNotAvailable => PipeError::NoDevice,
        other => PipeError::Os(other.to_string()),
    })?;
    Ok((stream, pump, shared))
}

/// The input device called `name`, with its default config. The unfiltered list reads only names; a device of the same
/// name that has no input (Windows lists each endpoint apart) is passed over.
fn find(name: &str) -> Result<(cpal::Device, cpal::SupportedStreamConfig), PipeError> {
    let devices = cpal::default_host()
        .devices()
        .map_err(|e| PipeError::Os(e.to_string()))?;
    for device in devices {
        if !device.name().is_ok_and(|n| n == name) {
            continue;
        }
        match device.default_input_config() {
            Ok(config) => return Ok((device, config)),
            Err(
                DefaultStreamConfigError::StreamTypeNotSupported
                | DefaultStreamConfigError::DeviceNotAvailable,
            ) => {}
            Err(e) => return Err(PipeError::Os(e.to_string())),
        }
    }
    Err(PipeError::NoDevice)
}

/// Whether a device called `name` is in the host's list: names only (the OS's properties), nothing opened or asked.
fn listed(name: &str) -> bool {
    cpal::default_host()
        .devices()
        .is_ok_and(|mut devices| devices.any(|d| d.name().is_ok_and(|n| n == name)))
}

/// The input pipeline's latency the stamps are moved back by (the A/V check): cpal's stamp is when the
/// samples left the HAL, so a sound is stamped this much after it was heard (Core Audio's device latency, safety offset
/// and stream latency: 33 ms for this Intel Mac's built-in mic, a frame at 30 fps). The design is Cap's
/// (`recording/src/feeds/microphone.rs` @ d327051, design only), capped at 0.5 s as Cap's (`mic::latency_ns`). Windows
/// reports none yet.
fn input_latency(name: &str) -> Duration {
    #[cfg(target_os = "macos")]
    let late = crate::platform::macos::input_latency(name).unwrap_or_default();
    #[cfg(not(target_os = "macos"))]
    let late = {
        let _ = name;
        Duration::ZERO
    };
    tracing::info!(
        name,
        ?late,
        "microphone: stamps moved back by the input's latency"
    );
    late
}

/// What an input stream's callback needs.
struct Built {
    feed: MicFeed,
    late: Duration,
    shared: Arc<Shared>,
}

/// The input stream: each callback goes straight into the rings, stamped on the host clock at the moment it was heard.
fn build<T: SizedSample + 'static>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    built: Built,
    to_f32: fn(T) -> f32,
) -> Result<cpal::Stream, cpal::BuildStreamError> {
    let Built {
        mut feed,
        late,
        shared,
    } = built;
    device.build_input_stream(
        config,
        move |data: &[T], info: &cpal::InputCallbackInfo| {
            let (capture, callback) = hosttime::from_cpal(&info.timestamp());
            feed.push(
                data,
                to_f32,
                capture.saturating_sub(late),
                callback,
                host_ns(),
            );
        },
        // Once per failure, off the data path.
        move |error| {
            tracing::warn!(%error, "microphone stream");
            match error {
                cpal::StreamError::DeviceNotAvailable => shared.device_lost(),
                _ => shared.stream_stopped(),
            }
        },
        None,
    )
}
