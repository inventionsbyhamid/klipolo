//! The Record dialog's previews, the app's side (`recorder_previews`): the page says what it wants previewed
//! (`Previews`) and the app starts and stops four preview workers to match, each this app's own binary in a form that
//! writes nothing and holds nothing once told to stop:
//! - **the stills** (`--record-worker --thumbs`): each display and app of the dialog's last setup, about once a second
//!   each, for the cards;
//! - **the live picture** of the chosen display or app, small (`live_view`);
//! - **the camera** for the bubble (`check`);
//! - **the mic's meter** (`--record-worker --meter`): ten peaks a second, as `recorder-event`s of `PreviewId::Mic`.
//!
//! Frames go to the page as raw bytes (`frames`), each with when the app got it, so a card whose stills have stopped
//! coming can be shown as such. The workers' stderr goes into the app's log (`launch::ProcessLaunch`).
//!
//! **Consent** (whatever the page asks):
//! - The stills and the live picture start only while Screen Recording is allowed (read without a prompt): otherwise
//!   `capture.permission.screen` and nothing is launched. Once one of them is refused by the system mid-run (the person
//!   said no to macOS's question, or took the permission back), neither starts again until the dialog reads its setup
//!   again (it reopens), so macOS's question is never raised again and again.
//! - The camera and the mic start by themselves only while allowed; otherwise only in the call the person's "Allow
//!   camera" or "Allow microphone" press makes (`Previews.ask`), when macOS asks; never once refused. One started that
//!   way is kept while macOS asks.
//! - Nothing restarts by itself after it ended, but a stills worker held back by its pace (below).
//!
//! **Stopping:** every preview stops when Klipolo's main window goes behind another app or is hidden or minimised
//! (nothing starts again until it is in front and the page asks), when a take arms (before its worker is launched, and
//! none starts while the take is the recorder's: its worker owns the screen and the devices), on
//! `recorder_previews_off`, and when Klipolo exits (killed). In a test run there are none.
//!
//! **The stills' pace:** each stills worker starts with a content read and a still at once, so a new one starts at most
//! once every `THUMB_EVERY`; one asked for sooner waits its turn (the ticker's).

use std::io::{BufRead, Read};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use klipolo_capture::backend::{Devices, Permission, Permissions, Target};
use klipolo_capture::protocol::Event;
use klipolo_capture::run::thumbs::{MAX_THUMBS, THUMB_EVERY};
use klipolo_model::coded::CodedError;
use klipolo_platform::clock::Clock;
use tauri::{AppHandle, Manager, RunEvent, WindowEvent};

use super::check::{Closing, camera_wanted, closing, enforce_close, may_open};
use super::config::take_target;
use super::frames::{PreviewOut, frame_message, now_ms};
use super::launch::Launch;
use super::live_view::{card_key, live_wanted};
use super::setup::is_os_id;
use super::state::RecorderState;
use super::take::{Recorder, Worker};
use super::types::{PreviewId, PreviewKind, Previews, PrivacyPage, WhatToRecord};
use super::words;

/// How often the ticker looks at closing workers and a stills worker waiting its turn.
const TICK: Duration = Duration::from_millis(100);
/// The longest event line read: a frame is a JPEG in base64, a few hundred KB at most.
const MAX_LINE: u64 = 4 << 20;
/// Klipolo's main window (`tauri.conf.json`'s `main`).
const MAIN: &str = "main";

/// The four previews.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Thumbs,
    Live,
    Camera,
    Mic,
}

impl Kind {
    const ALL: [Kind; 4] = [Kind::Thumbs, Kind::Live, Kind::Camera, Kind::Mic];

    fn id(self) -> PreviewId {
        match self {
            Kind::Thumbs => PreviewId::Thumbs,
            Kind::Live => PreviewId::Live,
            Kind::Camera => PreviewId::Camera,
            Kind::Mic => PreviewId::Mic,
        }
    }

    /// What the page hears when this preview's worker ends without saying why.
    fn ended(self) -> CodedError {
        match self {
            Kind::Live => CodedError::new("capture.liveView.unavailable"),
            _ => CodedError::new("capture.error.os").with("detail", "the preview stopped"),
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
    /// What it shows: the stills' targets as JSON, the live picture's target as JSON, the camera's or the mic's id.
    what: String,
    /// Its frames' key: the live picture's card, the camera's id.
    key: String,
    /// It said why it ended (`Failed`), so its end needn't.
    said_failed: bool,
}

#[derive(Default)]
struct Running {
    thumbs: Option<Open>,
    live: Option<Open>,
    camera: Option<Open>,
    mic: Option<Open>,
    closing: Vec<Closing>,
    /// Preview workers launched so far: the next one's generation.
    generations: u64,
    /// Where frames and events go: the page of the last `recorder_previews`.
    out: Option<Arc<dyn PreviewOut>>,
    /// The stills' targets: the displays and apps of the dialog's last setup.
    targets: Vec<(String, Target)>,
    /// The stills are wanted and allowed.
    want_thumbs: bool,
    /// When the last stills worker was launched.
    thumbs_at_ms: Option<u64>,
    /// A stills worker waits its turn (`THUMB_EVERY` after the last one's launch).
    thumbs_due: bool,
    /// A screen preview was refused by the system while it ran: neither starts again until the setup is read again.
    screen_refused: bool,
    /// Klipolo's main window is behind another app, hidden or minimised.
    behind: bool,
    ticking: bool,
}

impl Running {
    fn slot(&mut self, kind: Kind) -> &mut Option<Open> {
        match kind {
            Kind::Thumbs => &mut self.thumbs,
            Kind::Live => &mut self.live,
            Kind::Camera => &mut self.camera,
            Kind::Mic => &mut self.mic,
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
    /// The take's recorder: while it has a take, no preview runs.
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

    /// The Record dialog read its setup (it opened, or tried again): the stills show its displays and apps from now on,
    /// and the screen's previews may start again after a refusal.
    pub(super) fn setup_read(&self, devices: &Devices) {
        let mut run = self.lock();
        run.targets = thumb_targets(devices);
        run.screen_refused = false;
    }

    /// The previews to match `want`, their frames and events to `out` from now on. Refused during a take
    /// (`capture.busy`: every preview stops). `capture.permission.screen` when the stills or the live picture were
    /// refused (Screen Recording isn't allowed, or the system refused them while they ran): the camera's and the mic's
    /// are set all the same. While Klipolo is behind, nothing starts.
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
        if self.recorder.active() {
            self.stop_all(&mut run, now);
            return Err(self.recorder.busy_error());
        }
        if run.behind {
            tracing::info!("recorder: previews asked for while Klipolo is behind, so none starts");
            return Ok(());
        }
        run.out = Some(out);
        let screen = allowed.screen == Permission::Granted && !run.screen_refused;
        run.want_thumbs = want.thumbs && screen;
        self.set_thumbs(&mut run, now);
        let live = live_wanted(want.live.as_ref().filter(|_| screen));
        self.show(&mut run, Kind::Live, live, now);
        let asked = |page| want.ask == Some(page);
        let camera = camera_wanted(
            want.camera.as_ref(),
            allowed.camera,
            asked(PrivacyPage::Camera),
        );
        self.show(&mut run, Kind::Camera, camera, now);
        let mic = meter_wanted(want.mic.as_ref(), allowed.mic, asked(PrivacyPage::Mic));
        self.show(&mut run, Kind::Mic, mic, now);
        self.start_ticker(&mut run);
        if (want.thumbs || want.live.is_some()) && !screen {
            tracing::info!(
                permission = ?allowed.screen,
                refused = run.screen_refused,
                "recorder: the screen's previews wait for Screen Recording"
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

    /// A take arms: every preview stops, then `arm` runs, under the previews' lock, so none starts until the take is
    /// the recorder's (`set` refuses from then on).
    pub(super) fn around_arm<T>(self: &Arc<Self>, arm: impl FnOnce() -> T) -> T {
        let now = self.now_ms();
        let mut run = self.lock();
        if self.stop_all(&mut run, now) {
            tracing::info!("recorder: a take arms, so the previews stop first");
        }
        arm()
    }

    /// Klipolo's main window came to the front (`true`), or went behind another app or was hidden or minimised: then
    /// every preview stops, and none starts until it is in front again and the page asks.
    pub(super) fn in_front(self: &Arc<Self>, front: bool) {
        let now = self.now_ms();
        let mut run = self.lock();
        run.behind = !front;
        if !front && self.stop_all(&mut run, now) {
            tracing::info!("recorder: Klipolo went behind, so the previews stop");
        }
    }

    /// Klipolo exits: every preview worker, open or closing, is killed now (each runs in a session of its own).
    pub(super) fn exit(&self) {
        let mut run = self.lock();
        for kind in Kind::ALL {
            if let Some(open) = run.slot(kind).take() {
                (open.worker.kill)(true);
            }
        }
        for closing in (run.closing.iter_mut()).filter(|c| !c.killed) {
            (closing.worker.kill)(true);
            closing.killed = true;
        }
    }

    /// The ticker: a closed preview that hasn't ended is killed, and a stills worker that waited its turn starts. False
    /// once neither is left (the ticker then ends).
    pub(super) fn tick(self: &Arc<Self>) -> bool {
        let now = self.now_ms();
        let mut run = self.lock();
        enforce_close(&mut run.closing, now);
        if run.thumbs_due && (run.behind || self.recorder.active()) {
            run.want_thumbs = false;
        }
        if run.thumbs_due {
            self.set_thumbs(&mut run, now);
        }
        let live = run.thumbs_due || run.closing.iter().any(|c| !c.killed);
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
        run.want_thumbs = false;
        run.thumbs_due = false;
        run.out = None;
        self.start_ticker(run);
        stopped
    }

    /// The stills, as `want_thumbs` and the setup's targets say: the open worker is kept for the same targets, and a new
    /// one starts at most once every `THUMB_EVERY`, or waits its turn.
    fn set_thumbs(self: &Arc<Self>, run: &mut Running, now: u64) {
        let targets = (run.want_thumbs && !run.targets.is_empty())
            .then(|| serde_json::to_string(&run.targets).ok())
            .flatten();
        let Some(targets) = targets else {
            run.thumbs_due = false;
            close(run, Kind::Thumbs, now);
            return;
        };
        if (run.thumbs.as_ref()).is_some_and(|open| open.what == targets) {
            run.thumbs_due = false;
            return;
        }
        close(run, Kind::Thumbs, now);
        let every = THUMB_EVERY.as_millis() as u64;
        if run.thumbs_at_ms.is_some_and(|at| now < at + every) {
            run.thumbs_due = true;
            return;
        }
        run.thumbs_due = false;
        run.thumbs_at_ms = Some(now);
        let args = ["--record-worker", "--thumbs", targets.as_str()].map(String::from);
        self.launch(run, Kind::Thumbs, &args, targets, String::new());
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

    /// The ticker, while a preview is closing or a stills worker waits its turn (the app; tests tick themselves).
    fn start_ticker(self: &Arc<Self>, run: &mut Running) {
        let needed = run.thumbs_due || run.closing.iter().any(|c| !c.killed);
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
                (
                    Kind::Thumbs,
                    Event::Thumb {
                        key, jpeg_base64, ..
                    },
                ) => Some((PreviewKind::Thumb, key, jpeg_base64)),
                (Kind::Live, Event::Preview { jpeg_base64, .. }) => {
                    Some((PreviewKind::Live, key, jpeg_base64))
                }
                (Kind::Camera, Event::Preview { jpeg_base64, .. }) => {
                    Some((PreviewKind::Camera, key, jpeg_base64))
                }
                (Kind::Mic, ev @ Event::Meter { .. }) => {
                    if let Some(out) = &run.out {
                        out.event(kind.id(), &ev);
                    }
                    None
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

/// Open preview `kind` said why it ends, and the page hears it. The system refused the screen to the stills or the live
/// picture: neither starts again until the dialog reads its setup again, so macOS's question isn't raised again and
/// again.
fn failed(run: &mut Running, kind: Kind, error: CodedError) {
    tracing::info!(?kind, code = %error.code, "recorder: a preview failed");
    if let Some(open) = run.slot(kind) {
        open.said_failed = true;
    }
    if matches!(kind, Kind::Thumbs | Kind::Live) && error.code == "capture.permission.screen" {
        tracing::warn!(
            "recorder: the screen was refused, so its previews stay off until the dialog reopens"
        );
        run.screen_refused = true;
        run.want_thumbs = false;
        run.thumbs_due = false;
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

/// The mic's meter for `mic`, with `--ask` only on the person's own press while macOS hasn't asked yet; its worker's
/// arguments only when it may start now (`check::may_open`).
fn meter_wanted(mic: Option<&String>, permission: Permission, asked: bool) -> Option<Wanted> {
    let mic = mic?;
    let args = may_open(permission, asked).map(|ask| {
        let mut args: Vec<String> = ["--record-worker", "--meter", mic].map(String::from).into();
        if ask {
            args.push("--ask".into());
        }
        args
    });
    if args.is_none() {
        tracing::info!(
            ?permission,
            "recorder: the mic's meter waits for its permission or Allow microphone"
        );
    }
    Some(Wanted {
        what: mic.clone(),
        key: mic.clone(),
        args,
    })
}

/// The stills' targets, each under its card's key: every display, then every app's windows on the first display (as
/// "One app" records them), at most `MAX_THUMBS` (the worker takes no more).
fn thumb_targets(devices: &Devices) -> Vec<(String, Target)> {
    let screens = (devices.displays.iter()).map(|d| WhatToRecord::Screen {
        display: d.id.clone(),
    });
    let main = devices.displays.first().map(|d| d.id.clone());
    let apps = main.into_iter().flat_map(|display| {
        (devices.apps.iter()).map(move |a| WhatToRecord::App {
            app_id: a.app_id.clone(),
            display: display.clone(),
        })
    });
    (screens.chain(apps))
        .take(MAX_THUMBS)
        .map(|what| (card_key(&what), take_target(&what)))
        .collect()
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

    /// Klipolo's main window came to the front, or went behind (every preview stops).
    pub(super) fn window_in_front(&self, front: bool) {
        if let Ok(previews) = self.preview_workers() {
            previews.in_front(front);
        }
    }

    /// Klipolo exits: every preview worker is killed.
    pub(super) fn previews_exit(&self) {
        if let Ok(previews) = self.preview_workers() {
            previews.exit();
        }
    }
}

/// The previews follow Klipolo's main window: they stop when it goes behind another app or is hidden or minimised, and
/// may start again once it is in front; every preview worker is killed as Klipolo exits.
pub fn follow_window(app: &AppHandle, event: &RunEvent) {
    let Some(state) = app.try_state::<RecorderState>() else {
        return;
    };
    match event {
        RunEvent::WindowEvent {
            label,
            event: WindowEvent::Focused(front),
            ..
        } if label == MAIN => state.window_in_front(*front),
        RunEvent::Exit => state.previews_exit(),
        _ => {}
    }
}
