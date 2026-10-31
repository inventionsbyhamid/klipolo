//! Editing copies for the open project. The feed watcher lists the project's video files (main clips and video layers)
//! whenever the project or its set of video files changes; each is probed once (cached by its key) and the policy
//! (`needs_proxy`, by the engine's GPU class) picks those that play and scrub slowly. Copies are made one at a time by
//! the app binary run as `--proxy-worker`, in timeline order, and pause while the person plays the video, exports or
//! records, and while analysis decodes a clip on this Mac (`schedule`). A finished copy goes to the engine
//! (`Engine::set_media`), whose preview reads it in place of the source file with the same times; export never sees it.
//! A copy the preview can't open (a purged cache) falls back to the source file there, and the copy is made again.
//!
//! Never losing work: a paused, crashed or quit copy keeps every finished segment, and the next start resumes it
//! (`ProxyJob::open_with`). Copies are keyed by their source file (`key::stem`), never by project, so a renamed or
//! moved project folder finds them as before. Nothing is written for a source until its copy really starts
//! (`open_with` makes the job folder). A copy is only trusted once its source is checked again (`copy_state`).
//!
//! One background job at a time: a copy never starts while a thumbnails-only scan runs, and a running copy holds new
//! scans (`Hold::Copy`); exports hold scans too (`Hold::Export`).
//!
//! Colour: a copy is drawn from the preview's own decode of the source, so an HLG or PQ source's copy looks exactly as
//! the preview showed the source (neither is tone-mapped yet); swapping it in changes no look, and export always reads
//! the source file.

pub mod commands;
mod decide;
mod driver;
pub mod schedule;
mod tidy;

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use klipolo_engine::api::PauseBy;
use klipolo_engine::api::{CodedError, EngineEvent, MediaJobList};
use klipolo_engine::media::key::{CacheKind, SourceId, source_id, stem_of};
use klipolo_engine::media::proxy::decide::{ProxyPolicy, ProxyReason, needs_proxy, proxy_rate};
use klipolo_engine::media::proxy::facts::{ProbeError, SourceFacts, probe_facts};
use klipolo_engine::media::proxy::job::{
    CopyRecord, Opened, ProxyJob, Refused, job_dir, proxy_path, record_path, remove_copy,
};
use klipolo_engine::media::proxy::run::{FailCode, PauseReason};
use klipolo_engine::media::proxy::runner::{
    Launcher, ProcessLauncher, ProxyEvent, ProxyRunner, remove_job,
};
use klipolo_engine::media::{Hold, MediaKind, ThumbStore};
use klipolo_engine::plan::MediaMap;
use klipolo_engine::platform::gpu::GpuClass;
use klipolo_engine::session::{Engine, FeedRx, ProjectFeed};
use klipolo_model::contrib::AssetPath;
use klipolo_model::project::{AssetKind, Project};
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

use crate::english::code;
use decide::{files_key, view_of};
use driver::Driver;
use schedule::{Action, Failure, Job, JobState, Pause, ScheduleState};

/// The proxies folder is kept under this many bytes, least recently used copies first (never the open project's).
pub const PROXY_DISK_BUDGET: u64 = 10 << 30;
/// `media-jobs` events carry progress at most this often (state changes go at once).
const PROGRESS_EVERY: Duration = Duration::from_millis(250);
/// After playback stops (or analysis stops decoding), copies wait this long before starting again: a worker launched
/// as the person scrubs, only to pause at the next Play, would cost its start and its partial segment for nothing.
const SETTLE: Duration = Duration::from_secs(2);
/// How long a command waits for the driver's answer.
const COMMAND_WAIT: Duration = Duration::from_secs(10);
/// Probed facts kept (by copy key) before the cache starts over.
const FACTS_KEPT: usize = 1024;
/// Files that weren't there are looked at again (a stat each, off the driver) this often while any is absent, on a
/// timer of their own, and at once when Klipolo comes to the front: a drive plugged back in, or a file put back, gets
/// its copies without an edit or reopening the project. No timer runs while nothing is absent.
const ABSENT_EVERY: Duration = Duration::from_secs(5);
/// The refusal for a path the open project's list doesn't have: a stop or retry never reaches another project's copy.
pub const NOT_IN_PROJECT: &str = code!("copies.refused.notInProject");

/// What editing copies need from the preview engine: the `Engine`, or a fake in tests.
pub trait Preview: Send + Sync {
    fn set_media(&self, map: Arc<MediaMap>);
    fn media_event(&self, path: String, kind: MediaKind);
    fn gpu_class(&self) -> GpuClass;
}

impl Preview for Engine {
    fn set_media(&self, map: Arc<MediaMap>) {
        Engine::set_media(self, map);
    }

    fn media_event(&self, path: String, kind: MediaKind) {
        Engine::media_event(self, path, kind);
    }

    fn gpu_class(&self) -> GpuClass {
        self.info().gpu_class
    }
}

/// The editing-copy worker: this app's binary as `--proxy-worker`. If the binary can't be found, every copy fails with
/// that reason (nothing is retried by itself).
pub fn launcher() -> Arc<dyn Launcher> {
    match std::env::current_exe() {
        Ok(exe) => Arc::new(ProcessLauncher::proxy(exe)),
        Err(error) => {
            tracing::warn!(%error, "editing copies: the app's own binary can't be found");
            Arc::new(NoWorker)
        }
    }
}

struct NoWorker;

impl Launcher for NoWorker {
    fn launch(&self, _: &Path) -> Result<klipolo_engine::media::proxy::runner::Attempt, String> {
        Err("Klipolo couldn't start the editing copy's worker.".into())
    }
}

type Probe = Arc<dyn Fn(&Path) -> Result<SourceFacts, ProbeError> + Send + Sync>;
type Emit = Arc<dyn Fn(MediaJobList) + Send + Sync>;
/// A version of a source file: its path and what it is now (size, time, number). Facts never depend on the GPU class.
type FactKey = (Arc<str>, SourceId);
type FactsCache = Arc<Mutex<HashMap<FactKey, Arc<SourceFacts>>>>;

/// The app's editing copies (Tauri state; cheap to clone).
#[derive(Clone)]
pub struct MediaJobs {
    inner: Arc<Inner>,
}

struct Inner {
    state: Mutex<ScheduleState>,
    tx: mpsc::UnboundedSender<Msg>,
    rx: Mutex<Option<mpsc::UnboundedReceiver<Msg>>>,
    /// `<app cache>/media/proxies`.
    root: PathBuf,
    launcher: Arc<dyn Launcher>,
    probe: Probe,
    policy: fn(GpuClass) -> ProxyPolicy,
    emit: Emit,
    /// The listing in force (`Driver::generation`): a listing thread of an older one stops at its next file.
    listing: Arc<std::sync::atomic::AtomicU64>,
}

/// What the person (or the app) asks of the copies.
pub(crate) enum Command {
    Pause(PauseBy),
    Resume(PauseBy),
    Stop(String),
    Retry(String),
    SetAuto(bool),
}

enum Msg {
    Project(Option<ProjectView>),
    /// The open project changed but not its files (an edit).
    Revision,
    Decided {
        generation: u64,
        path: Arc<str>,
        decision: Decision,
    },
    Attach(Arc<dyn Preview>, Option<Arc<ThumbStore>>),
    Playing(bool),
    Exporting(bool),
    Recording(bool),
    Analyzing(bool),
    Event {
        run: u64,
        event: ProxyEvent,
    },
    Exited {
        run: u64,
    },
    /// The preview couldn't open this source's copy and reads the source file.
    Unusable(String),
    /// Run the driving step (a scan may have ended; a settle or a timer came due).
    Drive,
    /// A timer for a copy paused for heat or a busy encoder.
    Wake {
        path: Arc<str>,
        stem: String,
    },
    /// The app came back to the front: mapped copies are checked against their sources (overwritten meanwhile?).
    Recheck,
    /// These sources' copies are no longer of the file (overwritten): unmapped, deleted, made again.
    Stale(Vec<Arc<str>>),
    /// These sources can't be looked at now: their copies are unmapped and kept until the files are back.
    Unreachable(Vec<Arc<str>>),
    /// These files that weren't there are there again: decided again.
    Back(Vec<Arc<str>>),
    Command(Command, oneshot::Sender<Result<(), CodedError>>),
}

/// The open project, as copies see it: its video files, the timeline's first.
#[derive(Clone)]
struct ProjectView {
    paths: Arc<[Arc<str>]>,
}

/// What one source needs.
enum Decision {
    /// No copy (it scrubs well as it is, or none can be made here).
    NoCopy,
    /// It isn't there now, or reading it took too long: decided again at the next listing.
    Unavailable,
    /// It can't be read as video.
    Unreadable(String),
    Done {
        copy: Arc<str>,
        fps: f64,
        stem: String,
        facts: Arc<SourceFacts>,
        why: ProxyReason,
    },
    /// A copy to make (`progress`: an unfinished job's).
    Queue {
        stem: String,
        fps: f64,
        progress: f64,
        facts: Arc<SourceFacts>,
        why: ProxyReason,
    },
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl MediaJobs {
    /// Copies kept in `root` (`<app cache>/media/proxies`), made by `launcher` (`ProcessLauncher::proxy`), listed to
    /// the page through `emit` (the `media-jobs` event).
    pub fn new(root: PathBuf, launcher: Arc<dyn Launcher>, emit: Emit) -> Self {
        Self::with(
            root,
            launcher,
            emit,
            Arc::new(|path: &Path| probe_facts(path)),
            ProxyPolicy::for_gpu,
        )
    }

    fn with(
        root: PathBuf,
        launcher: Arc<dyn Launcher>,
        emit: Emit,
        probe: Probe,
        policy: fn(GpuClass) -> ProxyPolicy,
    ) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            inner: Arc::new(Inner {
                state: Mutex::new(ScheduleState::with(Vec::new())),
                tx,
                rx: Mutex::new(Some(rx)),
                root,
                launcher,
                probe,
                policy,
                emit,
                listing: Arc::default(),
            }),
        }
    }

    /// Starts the driving task (once; needs a tokio runtime).
    pub fn start(&self) {
        let Some(rx) = lock(&self.inner.rx).take() else {
            return;
        };
        tauri::async_runtime::spawn(Driver::new(self.inner.clone()).run(rx));
    }

    /// Lists the open project's files for copies whenever the project or its set of video files changes; closing it
    /// pauses the running copy and clears the map.
    pub fn watch(&self, mut feed: FeedRx) {
        let tx = self.inner.tx.clone();
        tauri::async_runtime::spawn(async move {
            let mut last: Option<u64> = None;
            loop {
                let view = {
                    let feed = feed.borrow_and_update();
                    feed.as_ref().map(|f| (files_key(f), f.project.clone()))
                };
                match view {
                    None if last.is_some() => {
                        last = None;
                        let _ = tx.send(Msg::Project(None));
                    }
                    Some((key, project)) if last != Some(key) => {
                        last = Some(key);
                        let _ = tx.send(Msg::Project(Some(view_of(&project))));
                    }
                    Some(_) => {
                        let _ = tx.send(Msg::Revision);
                    }
                    None => {}
                }
                if feed.changed().await.is_err() {
                    return;
                }
            }
        });
    }

    /// The engine runs: its GPU class picks the policy (Integrated, the cautious one, until now; files not started yet
    /// are decided again when it differs), finished copies go to it, and copies hold its thumbnail scans. No copy
    /// starts before this.
    pub fn attach(&self, engine: Engine, thumbs: Option<Arc<ThumbStore>>) {
        self.attach_preview(Arc::new(engine), thumbs);
    }

    fn attach_preview(&self, preview: Arc<dyn Preview>, thumbs: Option<Arc<ThumbStore>>) {
        self.send(Msg::Attach(preview, thumbs));
    }

    /// The app came back to the front (the person may have exported a source again elsewhere): every mapped copy is
    /// checked against its source off the main thread, and one that is no longer of the file is dropped at once.
    pub fn recheck(&self) {
        self.send(Msg::Recheck);
    }

    pub fn set_playing(&self, playing: bool) {
        self.send(Msg::Playing(playing));
    }

    /// An export took (true) or released (false) the export slot: copies pause, and so do thumbnail scans.
    pub fn set_exporting(&self, exporting: bool) {
        self.send(Msg::Exporting(exporting));
    }

    /// A take is recording (true, from arming until it has landed) or not (false): copies wait, so nothing competes
    /// with the take. The engine's own background work waits through the same take's hold (`Hold::Recording`).
    #[allow(dead_code, reason = "the recorder's take hold calls it")]
    pub fn set_recording(&self, recording: bool) {
        self.send(Msg::Recording(recording));
    }

    /// Analysis started (true) or stopped (false) decoding a clip on this Mac (its sound, its small copy): copies wait,
    /// as analysis is the person's own action. A copy under way pauses as analysis starts (the two may overlap for
    /// about a frame while it stops), and none starts meanwhile. After it stops, copies wait `SETTLE` before starting
    /// again: the next clip's decode often follows at once.
    pub fn set_analyzing(&self, analyzing: bool) {
        self.send(Msg::Analyzing(analyzing));
    }

    /// The engine's events that concern copies (its event sink calls this for each).
    pub fn on_engine_event(&self, event: &EngineEvent) {
        match event {
            EngineEvent::State { playing, .. } => self.set_playing(*playing),
            EngineEvent::Media {
                path,
                kind: MediaKind::Proxy,
            } => self.send(Msg::Unusable(path.clone())),
            // A scan's trailing event: it may have ended, so a waiting copy may start.
            EngineEvent::Media {
                kind: MediaKind::Thumbs,
                ..
            } => self.send(Msg::Drive),
            _ => {}
        }
    }

    /// The copies and what holds them, as the `media-jobs` event says them.
    pub fn list(&self) -> MediaJobList {
        lock(&self.inner.state).list()
    }

    /// Asks the driver and waits for its answer, at most `COMMAND_WAIT`: a source on a volume that hangs can hold the
    /// driver, never a command forever. A refusal is a code (`copies.refused.*`).
    pub(crate) async fn command(&self, command: Command) -> Result<(), CodedError> {
        let (reply, answer) = oneshot::channel();
        self.send(Msg::Command(command, reply));
        match tokio::time::timeout(COMMAND_WAIT, answer).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(CodedError::new(code!("copies.refused.unavailable"))),
            Err(_) => Err(CodedError::new(code!("copies.refused.slowDrive"))),
        }
    }

    fn send(&self, msg: Msg) {
        let _ = self.inner.tx.send(msg);
    }
}

#[cfg(test)]
mod tests;
