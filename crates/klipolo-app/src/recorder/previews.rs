//! The Record dialog's previews, the app's side (`recorder_previews`): the page says what it wants previewed
//! (`Previews`) and the app starts and stops preview workers to match, each this app's own binary in a form that
//! writes nothing and holds nothing once told to stop:
//! - **the live picture** of the chosen display or app, small (`live_view`);
//! - **the camera** for the bubble (`check`).
//!
//! Frames go to the page as raw bytes (`frames`), each with when the app got it. The workers' stderr goes into the
//! app's log (`launch::ProcessLaunch`).
//!
//! **Consent** (whatever the page asks):
//! - The live picture starts only while Screen Recording is allowed (read without a prompt): otherwise
//!   `capture.permission.screen` and nothing is launched.
//! - The camera starts by itself only while allowed; otherwise only in the call the person's "Allow camera" press makes
//!   (`Previews.ask`), when macOS asks; never once refused. One started that way is kept while macOS asks.
//! - Nothing restarts by itself after it ended.
//!
//! **Stopping:** every preview stops on `recorder_previews_off`. In a test run there are none.

use std::io::{BufRead, Read};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use klipolo_capture::backend::{Permission, Permissions};
use klipolo_capture::protocol::Event;
use klipolo_model::coded::CodedError;
use klipolo_platform::clock::Clock;

use super::check::{Closing, camera_wanted, closing, enforce_close};
use super::frames::{PreviewOut, frame_message, now_ms};
use super::launch::Launch;
use super::live_view::live_wanted;
use super::setup::is_os_id;
use super::state::RecorderState;
use super::take::{Recorder, Worker};
use super::types::{PreviewId, PreviewKind, Previews, PrivacyPage, WhatToRecord};
use super::words;

/// How often the ticker looks at closing workers.
const TICK: Duration = Duration::from_millis(100);
/// The longest event line read: a frame is a JPEG in base64, a few hundred KB at most.
const MAX_LINE: u64 = 4 << 20;

/// The previews.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Live,
    Camera,
}

impl Kind {
    const ALL: [Kind; 2] = [Kind::Live, Kind::Camera];

    fn id(self) -> PreviewId {
        match self {
            Kind::Live => PreviewId::Live,
            Kind::Camera => PreviewId::Camera,
        }
    }

    /// What the page hears when this preview's worker ends without saying why.
    fn ended(self) -> CodedError {
        match self {
            Kind::Live => CodedError::new("capture.liveView.unavailable"),
            Kind::Camera => {
                CodedError::new("capture.error.os").with("detail", "the preview stopped")
            }
        }
    }
}

/// What a preview should show: what it is (compared with the open one's), its frames' key, and its worker's arguments
/// when it may start now (None: it waits for a permission or the person's press).
pub(super) struct Wanted {
    pub(super) what: String,
    pub(super) key: String,
    pub(super) args: Option<Vec<String>>,
}

/// A running preview.
struct Open {
    worker: Worker,
    /// What it shows: the live picture's target as JSON, the camera's id.
    what: String,
    /// Its frames' key: the live picture's card, the camera's id.
    key: String,
    /// It said why it ended (`Failed`), so its end needn't.
    said_failed: bool,
}

#[derive(Default)]
struct Running {
    live: Option<Open>,
    camera: Option<Open>,
    closing: Vec<Closing>,
    /// Preview workers launched so far: the next one's generation.
    generations: u64,
    /// Where frames and events go: the page of the last `recorder_previews`.
    out: Option<Arc<dyn PreviewOut>>,
    ticking: bool,
}

impl Running {
    fn slot(&mut self, kind: Kind) -> &mut Option<Open> {
        match kind {
            Kind::Live => &mut self.live,
            Kind::Camera => &mut self.camera,
        }
    }

    /// The running preview whose worker is `generation`.
    fn kind_of(&mut self, generation: u64) -> Option<Kind> {
        (Kind::ALL.into_iter()).find(|&kind| {
            (self.slot(kind).as_ref()).is_some_and(|o| o.worker.generation == generation)
        })
    }
}

/// The previews' workers: launched as the take's are (`Launch`), timed by the recorder's clock.
pub(super) struct PreviewWorkers {
    launch: Arc<dyn Launch>,
    clock: Arc<dyn Clock>,
    /// The take's recorder.
    recorder: Arc<Recorder>,
    /// Screen Recording, the camera's and the mic's permissions, read without a prompt.
    permissions: Box<dyn Fn() -> Permissions + Send + Sync>,
    /// A thread ticks (the app); tests tick themselves.
    ticker: bool,
    run: Mutex<Running>,
}

impl PreviewWorkers {
    pub(super) fn new(
        launch: Arc<dyn Launch>,
        clock: Arc<dyn Clock>,
        recorder: Arc<Recorder>,
        permissions: Box<dyn Fn() -> Permissions + Send + Sync>,
        ticker: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            launch,
            clock,
            recorder,
            permissions,
            ticker,
            run: Mutex::default(),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Running> {
        self.run.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn now_ms(&self) -> u64 {
        self.clock.now_ns() / 1_000_000
    }

    /// The previews to match `want`, their frames and events to `out` from now on. `capture.permission.screen` when the
    /// live picture was refused (Screen Recording isn't allowed): the camera's is set all the same.
    pub(super) fn set(
        self: &Arc<Self>,
        want: &Previews,
        out: Arc<dyn PreviewOut>,
    ) -> Result<(), CodedError> {
        check_ids(want)?;
        // Read without a prompt, and before the lock (they ask the OS).
        let allowed = (self.permissions)();
        let now = self.now_ms();
        let mut run = self.lock();
        run.out = Some(out);
        let screen = allowed.screen == Permission::Granted;
        let live = live_wanted(want.live.as_ref().filter(|_| screen));
        self.show(&mut run, Kind::Live, live, now);
        let asked = want.ask == Some(PrivacyPage::Camera);
        let camera = camera_wanted(want.camera.as_ref(), allowed.camera, asked);
        self.show(&mut run, Kind::Camera, camera, now);
        self.start_ticker(&mut run);
        if want.live.is_some() && !screen {
            tracing::info!(
                permission = ?allowed.screen,
                "recorder: the live picture waits for Screen Recording"
            );
            return Err(CodedError::new("capture.permission.screen"));
        }
        Ok(())
    }

    /// Every preview stops: the Record dialog closed, Record was pressed, or the page went out of sight.
    pub(super) fn off(self: &Arc<Self>) {
        let now = self.now_ms();
        let mut run = self.lock();
        if self.stop_all(&mut run, now) {
            tracing::info!("recorder: the previews stop");
        }
    }

    /// The ticker: a closed preview that hasn't ended is killed. False once none is left (the ticker then ends).
    pub(super) fn tick(&self) -> bool {
        let now = self.now_ms();
        let mut run = self.lock();
        enforce_close(&mut run.closing, now);
        let live = run.closing.iter().any(|c| !c.killed);
        if !live {
            run.ticking = false;
        }
        live
    }

    /// Every preview is told to stop, and nothing is sent to the page any more. True when one ran.
    fn stop_all(self: &Arc<Self>, run: &mut Running, now: u64) -> bool {
        let mut stopped = false;
        for kind in Kind::ALL {
            stopped |= close(run, kind, now);
        }
        run.out = None;
        self.start_ticker(run);
        stopped
    }

    /// `kind` shows `wanted`: an open preview showing it already is kept (whatever its permission now: the person's
    /// press may have started it while macOS asks); any other is closed, and `wanted`'s starts when it may.
    fn show(self: &Arc<Self>, run: &mut Running, kind: Kind, wanted: Option<Wanted>, now: u64) {
        let open = run.slot(kind).as_ref();
        if let (Some(open), Some(wanted)) = (open, &wanted)
            && open.what == wanted.what
        {
            return;
        }
        close(run, kind, now);
        if let Some(Wanted {
            what,
            key,
            args: Some(args),
        }) = wanted
        {
            self.launch(run, kind, &args, what, key);
        }
    }

    /// Launches `kind`'s worker with `args`; its events are read on a thread of its own. One that can't be launched
    /// says so to the page.
    fn launch(
        self: &Arc<Self>,
        run: &mut Running,
        kind: Kind,
        args: &[String],
        what: String,
        key: String,
    ) {
        let io = match self.launch.spawn(args) {
            Ok(io) => io,
            Err(error) => {
                tracing::warn!(%error, ?kind, "recorder: a preview didn't start");
                if let Some(out) = &run.out {
                    let error = kind.ended();
                    out.event(kind.id(), &Event::Failed { error });
                }
                return;
            }
        };
        run.generations += 1;
        let generation = run.generations;
        tracing::info!(
            pid = io.pid,
            generation,
            ?kind,
            "recorder: a preview started"
        );
        let worker = Worker {
            generation,
            stdin: io.stdin,
            kill: io.kill,
        };
        *run.slot(kind) = Some(Open {
            worker,
            what,
            key,
            said_failed: false,
        });
        self.read_in_background(generation, io.stdout, io.wait);
    }

    /// The ticker, while a preview is closing (the app; tests tick themselves).
    fn start_ticker(self: &Arc<Self>, run: &mut Running) {
        let needed = run.closing.iter().any(|c| !c.killed);
        if !self.ticker || run.ticking || !needed {
            return;
        }
        run.ticking = true;
        let me = self.clone();
        let started = std::thread::Builder::new()
            .name("klipolo-preview-tick".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(TICK);
                    if !me.tick() {
                        break;
                    }
                }
            });
        if let Err(error) = started {
            run.ticking = false;
            tracing::error!(%error, "recorder: the previews' ticker didn't start");
        }
    }

    /// Preview worker `generation`'s stdout, read on a thread of its own until it closes, then its exit.
    #[allow(clippy::type_complexity)]
    fn read_in_background(
        self: &Arc<Self>,
        generation: u64,
        stdout: Box<dyn BufRead + Send>,
        wait: Box<dyn FnOnce() -> Option<i32> + Send>,
    ) {
        let me = self.clone();
        let started = std::thread::Builder::new()
            .name("klipolo-preview-events".into())
            .spawn(move || me.read(generation, stdout, wait));
        if let Err(error) = started {
            tracing::error!(generation, %error, "recorder: a preview's events can't be read");
        }
    }

    #[allow(clippy::type_complexity)]
    fn read(
        &self,
        generation: u64,
        mut stdout: Box<dyn BufRead + Send>,
        wait: Box<dyn FnOnce() -> Option<i32> + Send>,
    ) {
        let mut line = Vec::new();
        loop {
            line.clear();
            match (&mut stdout).take(MAX_LINE).read_until(b'\n', &mut line) {
                Ok(0) => break,
                Ok(n) if n as u64 == MAX_LINE && line.last() != Some(&b'\n') => {
                    tracing::warn!(generation, "recorder: a preview's line too long to read");
                    let mut rest = Vec::new();
                    let _ = stdout.read_until(b'\n', &mut rest);
                }
                Ok(_) => match serde_json::from_slice::<Event>(line.trim_ascii()) {
                    Ok(ev) => self.on_event(generation, ev),
                    Err(error) => {
                        tracing::warn!(generation, %error, "recorder: a preview's line isn't an event");
                    }
                },
                Err(error) => {
                    tracing::warn!(generation, %error, "recorder: a preview's events stopped");
                    break;
                }
            }
            self.handled();
        }
        let code = wait();
        self.on_exit(generation, code);
        self.handled();
    }

    /// One more line or exit handled (tests wait for their fake workers' with the recorder's own).
    fn handled(&self) {
        self.recorder.handled.fetch_add(1, Ordering::SeqCst);
    }

    /// One event from preview worker `generation`. A closing or ended preview's never reach the page. A frame's JPEG
    /// is decoded outside the lock, then sent under it once its preview is seen to be still open.
    fn on_event(&self, generation: u64, ev: Event) {
        let at_ms = now_ms();
        let frame = {
            let mut run = self.lock();
            let Some(kind) = run.kind_of(generation) else {
                return;
            };
            let key = (run.slot(kind).as_ref()).map_or_else(String::new, |o| o.key.clone());
            match (kind, ev) {
                (Kind::Live, Event::Preview { jpeg_base64, .. }) => {
                    Some((PreviewKind::Live, key, jpeg_base64))
                }
                (Kind::Camera, Event::Preview { jpeg_base64, .. }) => {
                    Some((PreviewKind::Camera, key, jpeg_base64))
                }
                (_, Event::Failed { error }) => {
                    failed(&mut run, kind, error);
                    None
                }
                // The workers' own states.
                _ => None,
            }
        };
        let Some((kind, key, jpeg_base64)) = frame else {
            return;
        };
        let Some(message) = frame_message(kind, &key, at_ms, &jpeg_base64) else {
            tracing::warn!(generation, "recorder: a preview frame that isn't one");
            return;
        };
        let mut run = self.lock();
        if run.kind_of(generation).is_none() {
            return;
        }
        if let Some(out) = run.out.clone()
            && !out.frame(message)
        {
            tracing::info!("recorder: the page's preview channel has gone");
            run.out = None;
        }
    }

    /// Preview worker `generation` ended with `code`. One that ended by itself (not told to) is forgotten, and the page
    /// hears why when it didn't say; it isn't started again by itself.
    fn on_exit(&self, generation: u64, code: Option<i32>) {
        let mut run = self.lock();
        run.closing.retain(|c| c.worker.generation != generation);
        let Some(kind) = run.kind_of(generation) else {
            return;
        };
        let open = run.slot(kind).take().expect("the preview is open");
        tracing::info!(
            generation,
            ?kind,
            ?code,
            "recorder: a preview ended by itself"
        );
        if !open.said_failed
            && let Some(out) = &run.out
        {
            out.event(
                kind.id(),
                &Event::Failed {
                    error: kind.ended(),
                },
            );
        }
    }
}

/// Open preview `kind` said why it ends, and the page hears it.
fn failed(run: &mut Running, kind: Kind, error: CodedError) {
    tracing::info!(?kind, code = %error.code, "recorder: a preview failed");
    if let Some(open) = run.slot(kind) {
        open.said_failed = true;
    }
    if let Some(out) = &run.out {
        out.event(kind.id(), &Event::Failed { error });
    }
}

/// Closes `kind`'s preview (`check::closing`). True when one was open.
fn close(run: &mut Running, kind: Kind, now: u64) -> bool {
    let Some(open) = run.slot(kind).take() else {
        return false;
    };
    closing(&mut run.closing, open.worker, now);
    true
}

/// The page's ids are names the OS gave (`setup::is_os_id`).
fn check_ids(want: &Previews) -> Result<(), CodedError> {
    let what = match &want.live {
        Some(WhatToRecord::Screen { display }) => [Some(display), None],
        Some(WhatToRecord::App { app_id, display }) => [Some(app_id), Some(display)],
        None => [None, None],
    };
    let mut ids = (want.camera.iter().chain(&want.mic)).chain(what.into_iter().flatten());
    if ids.all(|id| is_os_id(id)) {
        Ok(())
    } else {
        Err(words::no_device())
    }
}

impl RecorderState {
    /// The Record dialog's previews to match `want` (`PreviewWorkers::set`). In a test run, `capture.error.testRun` and
    /// nothing starts.
    pub(super) fn previews(
        &self,
        want: &Previews,
        out: Arc<dyn PreviewOut>,
    ) -> Result<(), CodedError> {
        self.preview_workers()?.set(want, out)
    }

    /// Every preview stops. In a test run there are none.
    pub(super) fn previews_off(&self) {
        if let Ok(previews) = self.preview_workers() {
            previews.off();
        }
    }
}
