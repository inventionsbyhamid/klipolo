//! The Record dialog's mic meter: the chosen mic's loudest sample ten times a second, in a worker of its own
//! (`worker --meter <mic id>`), before any take. Nothing is written and nothing else opens (no screen, no camera, no
//! other mic), and no orphan mode: it stops on the app's Stop or its stdin's end, and the mic closes then.
//!
//! - **Consent:** the guard first, so a test run stops there before any call to the backend. The app starts it only
//!   while the mic is allowed already, or on the person's own "Allow microphone" press, when macOS may ask.
//! - **The mic:** the take's own pipe for it (`Backend::audio`: cpal on macOS), started only if the app's Stop didn't
//!   come first, and stopped with it.
//! - **The level:** each block's loudest sample, the loudest of them over `METER_EVERY`, as the take's countdown meter
//!   reads them, sent as a `Meter` event for "mic" in dBFS (−120 for silence).
//! - **Its end:** a mic that goes away or stops by itself ends the meter with `Failed` (why), so the dialog never keeps
//!   its last level as live.

use std::sync::mpsc::{RecvTimeoutError, sync_channel};
use std::time::Duration;

use klipolo_model::coded::CodedError;

use super::preview_loop::{Events, PreviewIo, the_app_s_word};
use crate::backend::{AudioSpec, Backend};
use crate::english::msg;
use crate::guard::assert_real_capture_allowed;
use crate::media::PcmBlock;
use crate::pipe::{AudioOut, EVENT_CHANNEL, PCM_CHANNEL, PipeError, PipeEvent};
use crate::protocol::Event;

/// A peak goes to the app this often.
pub const METER_EVERY: Duration = Duration::from_millis(100);
/// The meter's source, as the take's countdown names a mic's.
const MIC: &str = "mic";
/// How long a wait for the mic's next block lasts before the app's Stop is looked at again.
const LOOK: Duration = Duration::from_millis(50);
/// The level of a block with nothing in it.
const SILENCE_DBFS: f32 = -120.0;

/// The guard first, then `meter_with`: the worker's form.
pub fn run_meter(backend: &dyn Backend, mic: &str, io: PreviewIo) -> i32 {
    assert_real_capture_allowed("the Record dialog's mic meter");
    meter_with(backend, mic, io)
}

/// The peaks of the mic `mic` (its id from `Devices::mics`) to `io`'s output until the app's Stop or its stdin's end.
/// 0 when stopped; 1 when the mic didn't start, or went away or stopped by itself (`Failed` says why).
pub fn meter_with(backend: &dyn Backend, mic: &str, io: PreviewIo) -> i32 {
    let PreviewIo {
        input,
        output,
        clock,
    } = io;
    let mut events = Events::new(output);
    let word = the_app_s_word(input);
    let (blocks, blocks_rx) = sync_channel(PCM_CHANNEL);
    let (said, said_rx) = sync_channel(EVENT_CHANNEL);
    let spec = AudioSpec::Mic {
        device: mic.to_owned(),
    };
    let started = backend.audio(&spec).and_then(|mut pipe| {
        if word.stopped() {
            return Ok(None);
        }
        let out = AudioOut {
            blocks,
            events: said,
            stopping: word.stop_flag(),
        };
        pipe.start(out).map(|_| Some(pipe))
    });
    let mut pipe = match started {
        Ok(Some(pipe)) => pipe,
        // Stopped before the mic opened: nothing opened.
        Ok(None) => return 0,
        Err(_) if word.stopped() => return 0,
        Err(error) => {
            tracing::info!(%error, "meter: the mic didn't start");
            events.send(&Event::Failed {
                error: start_error(&error),
            });
            return 1;
        }
    };
    let mut window = Window::at(clock.now());
    let mut ended = None;
    while !word.stopped() {
        if let Some(why) = said_rx.try_iter().find_map(|e| ended_by(&e)) {
            ended = Some(why);
            break;
        }
        match blocks_rx.recv_timeout(LOOK) {
            Ok(block) => {
                if let Some(peak_dbfs) = window.feed(&block, clock.now()) {
                    events.send(&Event::Meter {
                        source: MIC.into(),
                        peak_dbfs,
                    });
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                ended = Some(PipeError::Os("the mic stopped sending".into()).coded());
                break;
            }
        }
    }
    // The mic closes: nothing is captured after this.
    pipe.stop();
    let Some(error) = ended else {
        return 0;
    };
    tracing::info!(code = %error.code, "meter: the mic ended by itself");
    events.send(&Event::Failed { error });
    1
}

/// The loudest sample since the window began, sent once `METER_EVERY` has passed.
struct Window {
    from: Duration,
    peak: f32,
}

impl Window {
    fn at(now: Duration) -> Self {
        Self {
            from: now,
            peak: 0.0,
        }
    }

    /// `block`, which came at `now`: the window's peak in dBFS once it is `METER_EVERY` long (a new one begins).
    fn feed(&mut self, block: &PcmBlock, now: Duration) -> Option<f32> {
        let peak = block.samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        self.peak = self.peak.max(peak.min(1.0));
        if now.saturating_sub(self.from) < METER_EVERY {
            return None;
        }
        let dbfs = if self.peak > 0.0 {
            20.0 * self.peak.log10()
        } else {
            SILENCE_DBFS
        };
        *self = Self::at(now);
        Some(dbfs)
    }
}

/// Why the mic didn't start, as the app hears it: a refused permission is the microphone's step.
fn start_error(error: &PipeError) -> CodedError {
    match error {
        PipeError::PermissionDenied => msg!("capture.permission.mic"),
        _ => error.coded(),
    }
}

/// The mic's events that end the meter, and why.
fn ended_by(event: &PipeEvent) -> Option<CodedError> {
    match event {
        PipeEvent::DeviceLost => Some(PipeError::NoDevice.coded()),
        PipeEvent::PermissionRevoked => Some(msg!("capture.permission.mic")),
        PipeEvent::StoppedBySystem | PipeEvent::StoppedByUser => {
            Some(PipeError::Os("the mic stopped".into()).coded())
        }
        _ => None,
    }
}
