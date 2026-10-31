//! The Record dialog's mic meter: the chosen mic's loudest sample ten times a second, in a worker of its own
//! (`worker --meter <mic id> [--ask]`), before any take. Nothing is written and nothing else opens (no screen, no
//! camera, no other mic), and no orphan mode: it stops on the app's Stop or its stdin's end, and the mic closes then.
//!
//! - **Consent:** the guard first, so a test run stops there before any call to the backend. Then the mic's
//!   permission, read without a prompt (`Backend::permissions`): a mic the person refused is never opened; one they
//!   haven't answered for opens only with `ask` (the person's own "Allow microphone" press, when macOS asks), so the
//!   meter never makes the dialog ask by itself. Either refusal is `Failed` with `capture.permission.mic`, and nothing
//!   opens.
//! - **The mic:** the take's own pipe for it (`Backend::audio`: cpal on macOS), started only if the app's Stop didn't
//!   come first, and stopped with it.
//! - **The level:** each `METER_EVERY` of the mic's own time (its blocks' host times), the loudest sample in it, in
//!   dBFS as the take's countdown meter shows it (`health::dbfs`: −120 for silence), sent as a `Meter` event for "mic".
//! - **Its end:** a mic that goes away or stops by itself ends the meter with `Failed` (why), so the dialog never keeps
//!   its last level as live; so does a permission the person takes back while it runs, read again every
//!   `PERMISSION_EVERY` (a refused mic on macOS sends silence, never an error).

use std::sync::mpsc::{RecvTimeoutError, sync_channel};
use std::time::Duration;

use klipolo_model::coded::CodedError;

use super::preview_loop::{Events, PreviewIo, the_app_s_word};
use crate::backend::{AudioSpec, Backend, Permission};
use crate::english::msg;
use crate::guard::assert_real_capture_allowed;
use crate::health::{dbfs, peak_of};
use crate::media::PcmBlock;
use crate::pipe::{AudioOut, EVENT_CHANNEL, PCM_CHANNEL, PipeError, PipeEvent};
use crate::protocol::Event;

/// A peak goes to the app for each this much of the mic's time.
pub const METER_EVERY: Duration = Duration::from_millis(100);
/// The mic's permission is read again this often while the meter runs.
pub const PERMISSION_EVERY: Duration = Duration::from_secs(1);
/// The meter's source, as the take's countdown names a mic's.
const MIC: &str = "mic";
/// How long a wait for the mic's next block lasts before the app's Stop is looked at again.
const LOOK: Duration = Duration::from_millis(50);

/// The guard first, then `meter_with`: the worker's form.
pub fn run_meter(backend: &dyn Backend, mic: &str, ask: bool, io: PreviewIo) -> i32 {
    assert_real_capture_allowed("the Record dialog's mic meter");
    meter_with(backend, mic, ask, io)
}

/// The peaks of the mic `mic` (its id from `Devices::mics`) to `io`'s output until the app's Stop or its stdin's end.
/// `ask`: the person pressed "Allow microphone", so a mic they haven't answered for may open (and macOS may ask).
/// 0 when stopped; 1 when the mic isn't allowed, didn't start, went away, stopped by itself or was refused while it
/// ran (`Failed` says why).
pub fn meter_with(backend: &dyn Backend, mic: &str, ask: bool, io: PreviewIo) -> i32 {
    let PreviewIo {
        input,
        output,
        clock,
    } = io;
    let mut events = Events::new(output);
    let word = the_app_s_word(input);
    let permission = backend.permissions().mic;
    if permission == Permission::Denied || (!ask && permission != Permission::Granted) {
        tracing::info!(
            ?permission,
            ask,
            "meter: the mic isn't allowed, so nothing opens"
        );
        events.send(&Event::Failed {
            error: msg!("capture.permission.mic"),
        });
        return 1;
    }
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
    let mut window = Window::default();
    let mut look_again = clock.now() + PERMISSION_EVERY;
    let mut ended = None;
    while !word.stopped() {
        if let Some(why) = said_rx.try_iter().find_map(|e| ended_by(&e)) {
            ended = Some(why);
            break;
        }
        if clock.now() >= look_again {
            if backend.permissions().mic == Permission::Denied {
                ended = Some(msg!("capture.permission.mic"));
                break;
            }
            look_again = clock.now() + PERMISSION_EVERY;
        }
        match blocks_rx.recv_timeout(LOOK) {
            Ok(block) => {
                if let Some(peak_dbfs) = window.feed(&block) {
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
    tracing::info!(code = %error.code, "meter: the mic ended");
    events.send(&Event::Failed { error });
    1
}

/// `METER_EVERY` of the mic's time: when it began (its first block's host time) and its loudest sample so far.
#[derive(Default)]
struct Window {
    from_ns: Option<u64>,
    peak: f32,
}

impl Window {
    /// `block`, the mic's next: the window's peak in dBFS once a block starts `METER_EVERY` or more after the window
    /// began (that block begins the next one).
    fn feed(&mut self, block: &PcmBlock) -> Option<f32> {
        let from = *self.from_ns.get_or_insert(block.host_ns);
        let mut done = None;
        if block.host_ns.saturating_sub(from) >= METER_EVERY.as_nanos() as u64 {
            done = Some(dbfs(self.peak));
            *self = Self {
                from_ns: Some(block.host_ns),
                peak: 0.0,
            };
        }
        self.peak = self.peak.max(peak_of(block));
        done
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
