//! A preview worker's loop, Check camera's and the live view's (`check`, `live_view`): the app's Stop is read from the
//! start, the preview pipe starts unless the person stopped first, and each frame it paces (`pace::PreviewPacer`, before
//! the JPEG is made) goes to the app as a `Preview` event, until the Stop or the pipe's own end. The live view's view
//! size, as the app last said it, goes to the pipe between frames, and again every `RESIZE_AGAIN` until
//! the stream takes it. Nothing is written and nothing is re-encoded here. The Record dialog's stills and mic meter
//! (`thumbs`, `meter`) read the app's word and send their events the same way, through `PreviewIo`.

use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, sync_channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use base64::Engine;

use crate::backend::{PreviewFrame, PreviewPipe};
use crate::live_view_size::ViewPixels;
use crate::pipe::{PipeError, StopFlag};
use crate::protocol::{self, Command, Event};
use crate::shell::WorkerState;

/// Frames waiting between the pipe and the app: older ones are dropped by the pipe.
const FRAMES: usize = 2;
/// A view size the stream didn't take is tried again this long after: never left big while small.
const RESIZE_AGAIN: Duration = Duration::from_millis(500);
/// How often a wait on the real clock looks at the app's Stop.
const STOP_LOOK: Duration = Duration::from_millis(50);

/// A preview worker's ends (the stills', the mic meter's): the app's lines in, its events out, and the clock it keeps
/// time by.
pub struct PreviewIo {
    pub input: Box<dyn BufRead + Send>,
    pub output: Box<dyn Write + Send>,
    pub clock: Box<dyn Clock>,
}

impl PreviewIo {
    /// The worker's own: stdin, stdout and real time.
    pub fn std() -> Self {
        let input = Box::new(std::io::BufReader::new(std::io::stdin()));
        Self::new(input, Box::new(std::io::stdout()))
    }

    /// `input` and `output` on real time.
    pub fn new(input: Box<dyn BufRead + Send>, output: Box<dyn Write + Send>) -> Self {
        Self {
            input,
            output,
            clock: Box::new(RealClock(Instant::now())),
        }
    }
}

/// A preview worker's time since it started: real time, or a test's own.
pub trait Clock: Send {
    fn now(&self) -> Duration;
    /// Waits until `at`, unless `stopped` says so first: then false, at once.
    fn wait_until(&mut self, at: Duration, stopped: &dyn Fn() -> bool) -> bool;
}

/// Real time from the worker's start, its waits looking at the app's Stop every `STOP_LOOK`.
struct RealClock(Instant);

impl Clock for RealClock {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }

    fn wait_until(&mut self, at: Duration, stopped: &dyn Fn() -> bool) -> bool {
        loop {
            if stopped() {
                return false;
            }
            let Some(left) = at.checked_sub(self.now()).filter(|l| !l.is_zero()) else {
                return true;
            };
            std::thread::sleep(left.min(STOP_LOOK));
        }
    }
}

/// The worker's events to the app, one JSON line each.
pub(super) struct Events(Box<dyn Write + Send>);

impl Events {
    pub(super) fn new(output: Box<dyn Write + Send>) -> Self {
        Self(output)
    }

    pub(super) fn send(&mut self, ev: &Event) {
        let _ = (self.0)
            .write_all(&protocol::encode(ev))
            .and_then(|()| self.0.flush());
    }
}

/// How a preview ended.
#[derive(Debug)]
pub(super) enum Ended {
    /// The app's Stop (or Cancel, Disarm, or its stdin's end), before the pipe opened too: then nothing opened.
    Stopped,
    /// The pipe wasn't built or didn't start.
    Failed(PipeError),
    /// The pipe stopped sending by itself (its stream ended).
    Lost,
}

/// What the app said to a preview worker, read on a thread of its own from the start (`the_app_s_word`).
pub(super) struct AppWord {
    /// Set by Cancel, Disarm or Stop, or at stdin's end. So a Stop sent right after the press opens nothing:
    /// the flag is checked right before the pipe starts, and by the pipe's `start` right before it opens its
    /// device.
    stop: Arc<AtomicBool>,
    /// The live view's newest view size (`ViewSize`) until the loop takes it: only the newest counts, so it holds one.
    view: Arc<Mutex<Option<Option<ViewPixels>>>>,
}

impl AppWord {
    pub(super) fn stopped(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }

    /// The Stop as a pipe's `start` reads it, right before it opens its device.
    pub(super) fn stop_flag(&self) -> StopFlag {
        StopFlag::new(self.stop.clone())
    }

    /// The view size the app said since the last call, if it said one.
    fn new_view(&self) -> Option<Option<ViewPixels>> {
        self.view
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }
}

/// Reads the app's lines on a thread of its own: its Stop (and Cancel, Disarm or stdin's end), and the live view's
/// view size.
pub(super) fn the_app_s_word(input: Box<dyn BufRead + Send>) -> AppWord {
    let word = AppWord {
        stop: Arc::new(AtomicBool::new(false)),
        view: Arc::new(Mutex::new(None)),
    };
    let (flag, view) = (word.stop.clone(), word.view.clone());
    let _ = std::thread::Builder::new()
        .name("klipolo-preview-input".into())
        .spawn(move || {
            for line in input.lines().map_while(Result::ok) {
                match protocol::decode_command(&line) {
                    Some(Command::Cancel | Command::Disarm | Command::Stop) => break,
                    Some(Command::ViewSize { view: size }) => {
                        *view.lock().unwrap_or_else(PoisonError::into_inner) = Some(size);
                    }
                    _ => {}
                }
            }
            flag.store(true, Ordering::Release);
        });
    word
}

/// Starts the preview `open` builds, unless the app's Stop came first, and sends each of its frames to the app until
/// the Stop or its own end, passing on each new view size between frames; then stops it.
pub(super) fn run_preview(
    open: impl FnOnce() -> Result<Box<dyn PreviewPipe>, PipeError>,
    word: &AppWord,
    events: &mut Events,
) -> Ended {
    let stop = &word.stop;
    let stopped = || word.stopped();
    let (frames, frames_rx) = sync_channel(FRAMES);
    let started = open().and_then(|mut p| {
        if stopped() {
            return Ok(None);
        }
        p.start(frames, StopFlag::new(stop.clone()))
            .map(|()| Some(p))
    });
    let mut preview = match started {
        Ok(Some(preview)) => preview,
        // Stopped before the pipe opened, here or inside `start` (after the system's prompt): nothing opened.
        Ok(None) => return Ended::Stopped,
        Err(_) if stopped() => return Ended::Stopped,
        Err(e) => return Ended::Failed(e),
    };
    let mut ended = Ended::Stopped;
    // The view size the stream hasn't taken yet, and when to try it again.
    let mut untaken: Option<(Option<ViewPixels>, Instant)> = None;
    while !stopped() {
        let due = untaken
            .filter(|(_, at)| Instant::now() >= *at)
            .map(|(view, _)| view);
        if let Some(view) = word.new_view().or(due) {
            untaken = (!preview.resize(view)).then(|| (view, Instant::now() + RESIZE_AGAIN));
        }
        match frames_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(frame) => events.send(&preview_event(&frame)),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                if !stopped() {
                    ended = Ended::Lost;
                }
                break;
            }
        }
    }
    preview.stop();
    ended
}

fn preview_event(frame: &PreviewFrame) -> Event {
    Event::Preview {
        jpeg_base64: base64_of(&frame.jpeg),
        width: frame.width,
        height: frame.height,
    }
}

/// A JPEG's bytes as the protocol's text.
pub(super) fn base64_of(jpeg: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(jpeg)
}

/// The worker's state, as a preview worker says it.
pub(super) fn state(state: WorkerState) -> Event {
    Event::State {
        state,
        elapsed_us: 0,
    }
}

#[cfg(test)]
mod tests {
    use std::hint::black_box;
    use std::time::Instant;

    use super::*;
    use crate::preview_wire::preview_wire;

    /// The relay's cost per frame, each side measured apart: the worker's base64 and JSON line; the
    /// app's parse and its channel message (`preview_wire`: the JPEG's own bytes, no base64 or JSON to the WebView).
    /// For the small view's JPEG (640 × 400, about 47 KB, 10 a second) and a big view's at 1080p's rate (1728 × 1080,
    /// about 340 KB by the same bytes per pixel, 16 a second). The capture, Core Image's JPEG and the WebView's decode
    /// are the Mac's own and aren't here. Budget: under 5 % of a core for each side at each view's rate.
    /// `cargo test --release -p klipolo-capture --features synthetic --lib relay_cost -- --ignored --nocapture`
    #[test]
    #[ignore = "a measurement: run in release"]
    fn relay_cost() {
        for (name, bytes, size, fps) in [
            ("small", 47_000, (640, 400), 10),
            ("big", 340_000, (1728, 1080), 16),
        ] {
            // Bytes that look like a JPEG's to base64 and JSON: no runs to shortcut.
            let mut seed = 0x9e37_79b9_u32;
            let jpeg = (0..bytes)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    seed as u8
                })
                .collect();
            let frame = PreviewFrame {
                jpeg,
                width: size.0,
                height: size.1,
            };
            let n = 300;
            let started = Instant::now();
            let mut lines = Vec::with_capacity(n);
            for _ in 0..n {
                lines.push(protocol::encode(&preview_event(&frame)));
            }
            let worker = started.elapsed().as_secs_f64() * 1_000.0 / n as f64;
            let started = Instant::now();
            for line in &lines {
                let ev: Event = serde_json::from_slice(line.trim_ascii()).expect("an event");
                let Event::Preview { jpeg_base64, .. } = ev else {
                    panic!("a preview")
                };
                black_box(preview_wire("live-1", &jpeg_base64).expect("a message"));
            }
            let app = started.elapsed().as_secs_f64() * 1_000.0 / n as f64;
            let core = |ms: f64| ms * f64::from(fps) / 10.0;
            println!(
                "relay {name} {size:?} at {fps} fps: worker {worker:.3} ms a frame ({:.2} % of a core), app {app:.3} ms \
                 ({:.2} %)",
                core(worker),
                core(app)
            );
            assert!(core(worker) < 5.0 && core(app) < 5.0, "{name}");
        }
    }
}
