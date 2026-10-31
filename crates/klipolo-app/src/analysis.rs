//! Analysis in the app: after import, each clip's transcript, word times, scene log and sound levels, with the cost
//! shown first, progress as it goes, results cached per file, and a clean stop when the credit runs out. Captions and
//! chat read what it finds.
//!
//! - **Estimate** (`AnalysisState::estimate`): what analysing the clips asked for costs at Meta's list prices (the
//!   providers' own estimates: `transcript::estimate_usd`, `scenes::estimate_usd`), how long they are, and which are
//!   cached already (they cost nothing). A clip whose file can't be found or opened is neither priced nor offered
//!   (`Estimate::missing`). With an access code it also gives the credit left, and why Analyze is refused when the
//!   estimate is over it. What it finds cached shows at once (a re-imported clip's captions, before Analyze).
//! - **Start** (`AnalysisState::start`): cached clips load at once, with no network call; a clip whose file is missing
//!   fails at once, saying what to do (`FILE_MISSING`). The others run each its own
//!   pipeline, all at once (the Meta adapter keeps the device's request slots): its sound (step `audio`), its speaker
//!   turns (`transcript`, published at once as turn-level captions), its word times (`words`), then its scene log
//!   (`scenes`) for a video. Decoding (the sound, the scene log's small copy) runs one clip at a time on a thread of
//!   utility priority, and one video call runs at a time.
//! - **Results** go into the engine's `Speech` (a new `Arc<Speech>` for the open revision: the captions update live),
//!   with each clip's sound levels (`Speech::levels`, from the sound cache's energies), and are kept here for chat
//!   (`AnalysisState::analysis_for`). Each finished step is cached (`klipolo_analysis::cache`): `transcript.json` as
//!   soon as it is heard (its words timed roughly, for a quit or a crash to keep) and again once they are timed,
//!   `scenes.json` once logged. A half-done scene log isn't cached.
//! - **Stops:** Cancel (`AnalysisState::cancel`) sends nothing more after the requests in flight; their clips go back to
//!   not analysed and keep what was cached. A used-up credit (402) stops every paid request of the run, and the clips
//!   that needed one fail with the reason (the relay's sentence and the way on: a Meta key); word timing, which is free,
//!   carries on. A relay that refuses the device's token (401) gets the device registered again once
//!   (`Credentials::recover_unauthorized`) and the request sent again; Meta refusing the person's own key is a wrong
//!   key, said so, with nothing registered.
//!
//! Nothing here holds a key or a token, or puts one in an event or a log line.

mod calls;
pub mod chat;
pub mod media;
mod run;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use klipolo_analysis::providers::meta::Meta;
use klipolo_analysis::scenes::{self, SceneLog};
use klipolo_analysis::transcript::{self, TranscriptOut, Turn};
use klipolo_model::contrib::AssetPath;
use klipolo_model::project::{Asset, AssetKind};
use klipolo_model::speech::{Levels, Speech, Transcript};
use klipolo_model::time::Us;
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;
use ts_rs::TS;

use crate::credentials::{AiMode, AiStatus, CODE_INACTIVE, Credentials};
pub use media::{EngineMedia, Media};
use run::{Outcome, Run};

/// The event a step's progress goes out on.
pub const PROGRESS_EVENT: &str = "analysis://progress";
/// The event a clip's finished analysis goes out on.
pub const DONE_EVENT: &str = "analysis://asset_done";
/// The event a clip's failed analysis goes out on.
pub const FAILED_EVENT: &str = "analysis://failed";
/// The event a clip whose analysis was cancelled goes out on (it is back to not analysed).
pub const STOPPED_EVENT: &str = "analysis://stopped";

/// Why nothing can run: no key and no code.
pub const NEEDS_KEY: &str = "Add your Meta API key or access code in Settings.";
/// Said after a used-up credit through Klipolo's relay.
pub const ADD_KEY: &str = "Add your own Meta API key in Settings to keep going.";
/// A clip whose file can't be read.
pub const UNREADABLE: &str = "Klipolo could not read this clip.";
/// A clip whose file was moved, deleted or can't be opened: nothing to price or analyse.
pub const FILE_MISSING: &str =
    "Klipolo can't find this clip's file. Put it back or import it again.";
/// A request refused because the run was cancelled (never shown: the clip goes back to not analysed).
pub(crate) const STOPPED: &str = "Stopped.";

/// One step of a clip's analysis.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum AnalysisStep {
    /// Getting the clip's sound (and its levels).
    Audio,
    /// Hearing its speaker turns.
    Transcript,
    /// Timing each word.
    Words,
    /// Watching the footage for the scene log.
    Scenes,
}

/// Where a clip's analysis is.
#[derive(Clone, Debug, Default, PartialEq, Serialize, TS)]
#[serde(tag = "state", rename_all = "camelCase")]
#[ts(export)]
pub enum AssetState {
    /// Not analysed (or cancelled).
    #[default]
    None,
    /// A step is running, `pct` (0 to 100) of the way through it.
    Running { step: AnalysisStep, pct: u8 },
    /// Every step is done; `approximate` when some words are timed roughly.
    #[serde(rename_all = "camelCase")]
    Done {
        approximate: bool,
        /// Why some words are timed roughly, as one plain sentence: word timing's own (it failed, stopped or didn't
        /// finish), or that they don't match their sound.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        approximate_reason: Option<String>,
        /// Why the clip has no words ("No speech found."), when it has none.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        empty_reason: Option<String>,
        /// Timing the words again can time the rough ones (word timing failed, stopped or didn't finish): the page
        /// offers it. Not for words that don't match their sound.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        time_again: bool,
    },
    /// A step failed or was stopped; the steps before it are kept. `reason` is one plain sentence.
    Failed { reason: String },
}

/// A clip and where its analysis is (`analysis_states`).
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AssetAnalysisState {
    pub asset_id: String,
    pub state: AssetState,
}

/// `analysis://progress`: how far a clip's current step is.
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AnalysisProgress {
    pub asset_id: String,
    pub step: AnalysisStep,
    /// 0 to 100.
    pub pct: u8,
}

/// `analysis://asset_done`: a clip is analysed.
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AnalysisDone {
    pub asset_id: String,
    /// Some words are timed roughly (spread across their turn).
    pub approximate: bool,
    /// Why, as `AssetState::Done` says it.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub approximate_reason: Option<String>,
    /// Why the clip has no words.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub empty_reason: Option<String>,
    /// Timing the words again can time the rough ones.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub time_again: bool,
}

/// `analysis://failed`: a clip's analysis stopped; what finished is kept.
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AnalysisFailed {
    pub asset_id: String,
    /// One plain sentence.
    pub reason: String,
}

/// `analysis://stopped`: a clip's analysis was cancelled; it is back to not analysed.
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AnalysisStopped {
    pub asset_id: String,
}

/// What the page is told, each on its own event (`AnalysisEvent::name`).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum AnalysisEvent {
    Progress(AnalysisProgress),
    Done(AnalysisDone),
    Failed(AnalysisFailed),
    Stopped(AnalysisStopped),
}

impl AnalysisEvent {
    /// The event it goes out on.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Progress(_) => PROGRESS_EVENT,
            Self::Done(_) => DONE_EVENT,
            Self::Failed(_) => FAILED_EVENT,
            Self::Stopped(_) => STOPPED_EVENT,
        }
    }

    pub fn asset_id(&self) -> &str {
        match self {
            Self::Progress(e) => &e.asset_id,
            Self::Done(e) => &e.asset_id,
            Self::Failed(e) => &e.asset_id,
            Self::Stopped(e) => &e.asset_id,
        }
    }
}

/// What analysing some clips is expected to cost, before anything is sent.
#[derive(Clone, Debug, Default, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Estimate {
    /// About what analysing `clips` costs, in US dollars, at Meta's list prices. 0 while nothing can pay (no key or
    /// code, or a code whose device isn't registered).
    pub usd: f64,
    /// How long `clips` are together, in seconds.
    pub seconds: f64,
    /// The clips asked for whose analysis is cached: they cost nothing and load at once.
    pub cached: Vec<String>,
    /// The clips asked for that need analysing (or the steps of it not cached yet), in the order asked.
    pub clips: Vec<String>,
    /// The clips asked for whose file Klipolo can't find or open (moved, deleted): neither priced nor offered
    /// (`FILE_MISSING` says what to do).
    pub missing: Vec<String>,
    /// With an access code: the credit left, when Klipolo's relay said.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub credit_left_usd: Option<f64>,
    /// Why analysing `clips` is refused now (more than the credit left, or a code turned off), as one sentence.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub refusal: Option<String>,
    /// Why nothing can pay for `clips` yet although a key or code is saved (the device couldn't register with
    /// Klipolo's relay), as one sentence to show with Retry. `usd` is 0 then. None with no key and no code: the page
    /// asks for one.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub problem: Option<String>,
}

/// What a clip's analysis found so far: what chat reads (Plan 4), in the clip's own times (words and turns in its sound
/// time, the scene log in its file time). Assets on one file share it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AssetAnalysis {
    /// Its words, as the captions read them.
    pub transcript: Option<Arc<Transcript>>,
    /// Its speaker turns.
    pub turns: Vec<Turn>,
    /// Its scene log (a video's).
    pub scenes: Option<Arc<SceneLog>>,
    /// Some words are timed roughly.
    pub approximate: bool,
    /// What this session's calls for its file cost, in US dollars (nothing for what came from the cache).
    pub spent_usd: f64,
}

/// Who asked for a `start`: the person (Analyze, Retry), or the page on its own (loading cached clips as a project
/// opens). Rough words are timed again for the person every time, for the page at most once per content per app run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Asked {
    Person,
    Page,
}

/// Which project opening analysis serves (`project_changed` moves it on). A command takes it before reading the open
/// project's assets, so a switch between the two is seen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Opening(u64);

/// What a `start` set going.
pub struct Started {
    tasks: Vec<JoinHandle<()>>,
    /// Why the clips that needed paying for didn't start (cached ones load all the same).
    pub refused: Option<String>,
}

impl Started {
    /// Nothing started, nothing refused.
    fn none() -> Self {
        Self {
            tasks: Vec::new(),
            refused: None,
        }
    }

    /// Waits until every clip it started has finished, failed or stopped.
    pub async fn finished(self) {
        for task in self.tasks {
            let _ = task.await;
        }
    }
}

/// Where the page hears about analysis (`AnalysisEvent::name` names the event).
pub type EventSink = Arc<dyn Fn(AnalysisEvent) + Send + Sync>;
/// Where each new `Speech` goes (the engine's feed, for the open revision).
pub type SpeechSink = Arc<dyn Fn(Arc<Speech>) + Send + Sync>;

/// A clip as analysis sees it: an imported video or sound file on this Mac.
#[derive(Clone, Debug)]
pub(crate) struct Clip {
    pub id: String,
    pub path: PathBuf,
    pub video: bool,
    pub has_audio: bool,
    pub duration_us: Us,
}

impl Clip {
    /// `asset` when Klipolo analyses it: a local video or sound file.
    fn of(asset: &Asset) -> Option<Self> {
        let video = match asset.kind {
            AssetKind::Video => true,
            AssetKind::Audio => false,
            AssetKind::Image => return None,
        };
        let Some(AssetPath::File(path)) = AssetPath::parse(&asset.path) else {
            return None;
        };
        Some(Self {
            id: asset.id.clone(),
            path: PathBuf::from(path),
            video,
            has_audio: asset.has_audio,
            duration_us: asset.duration_us.max(0),
        })
    }
}

/// A clip's cached results, and the content key they are kept under (None when its file couldn't be read).
#[derive(Default)]
pub(crate) struct Cached {
    pub key: Option<String>,
    pub transcript: Option<TranscriptOut>,
    pub scenes: Option<SceneLog>,
    /// Its sound levels (`levels.json`), so a clip analysed before needs no decode for them.
    pub levels: Option<Levels>,
}

impl Cached {
    /// Every step `clip` needs is cached.
    fn complete(&self, clip: &Clip) -> bool {
        self.transcript.is_some() && (!clip.video || self.scenes.is_some())
    }
}

/// One content (a file, by its content key) to analyse: the first of its assets asked for, and its cache.
pub(crate) struct Content {
    pub key: String,
    pub clip: Clip,
    pub cached: Cached,
}

/// The key a clip is tracked under: its content key, or its own id when its file can't be read.
fn key_of(clip: &Clip, cached: &Cached) -> String {
    cached
        .key
        .clone()
        .unwrap_or_else(|| format!("unread:{}", clip.id))
}

/// One content's analysis: where it is and what it found. Every asset on that content (the same file imported twice)
/// shares it, and hears each change.
#[derive(Default)]
struct Entry {
    state: AssetState,
    analysis: Arc<AssetAnalysis>,
    levels: Option<Arc<Levels>>,
}

#[derive(Default)]
struct Inner {
    /// The open project's assets analysis has seen, and the content key each is on.
    keys: HashMap<String, String>,
    /// By content key.
    entries: HashMap<String, Entry>,
    /// The contents a `start` took on, until their analysis ends: another start leaves them alone.
    claimed: HashSet<String>,
    /// With an access code: each running content's estimate less what its calls have cost so far, by content key. The
    /// relay charges a call only once it is answered, so this is credit already promised: a start or an estimate
    /// counts it as spent.
    reserved: HashMap<String, f64>,
    /// The contents whose rough words were timed again, or whose word timing the person stopped, in this app run: a
    /// start the page makes on its own (loading cached clips) doesn't time them again; only the person's Analyze,
    /// Retry or "Time words again" does.
    timed_again: HashSet<String>,
    /// Bumped by every project switch (`project_changed`). A `start` or `estimate` remembers it on entry and does
    /// nothing more once it has moved on: its assets belong to a project no longer open.
    generation: u64,
    /// What the engine has: the open project's speech, by asset id.
    speech: Speech,
    /// The runs started, while any of their clips is still going (`cancel` stops them all).
    runs: Vec<Weak<Run>>,
}

impl Inner {
    /// The assets on content `key`, in id order.
    fn ids_of(&self, key: &str) -> Vec<String> {
        let mut ids: Vec<String> = self
            .keys
            .iter()
            .filter(|(_, k)| *k == key)
            .map(|(id, _)| id.clone())
            .collect();
        ids.sort();
        ids
    }

    fn entry(&mut self, key: &str) -> &mut Entry {
        self.entries.entry(key.to_owned()).or_default()
    }
}

/// The app's analysis: each content's state and results, the assets on it, and the runs going. Make one per app.
pub struct AnalysisState {
    credentials: Arc<Credentials>,
    media: Arc<dyn Media>,
    /// `Klipolo/analysis` in app support: the per-file cache.
    cache_root: PathBuf,
    /// Where each clip's WAV and small copy are made, and removed once used.
    work_root: PathBuf,
    events: EventSink,
    speech_sink: SpeechSink,
    inner: Mutex<Inner>,
    /// Decoding a clip (its sound, its small copy), one clip at a time.
    local: Semaphore,
    /// One video call at a time, whatever the mode.
    videos: Arc<Semaphore>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl AnalysisState {
    /// The app's analysis, its results cached under `cache_root` and its working files made under `work_root` (what an
    /// earlier run left there is removed now). `events` hears every state change, `speech` every new `Speech`.
    pub fn new(
        credentials: Arc<Credentials>,
        media: Arc<dyn Media>,
        cache_root: PathBuf,
        work_root: PathBuf,
        events: EventSink,
        speech: SpeechSink,
    ) -> Arc<Self> {
        // A quit or a crash mid-analysis leaves its WAVs and copies behind.
        let _ = std::fs::remove_dir_all(&work_root);
        Arc::new(Self {
            credentials,
            media,
            cache_root,
            work_root,
            events,
            speech_sink: speech,
            inner: Mutex::default(),
            local: Semaphore::new(1),
            videos: Arc::new(Semaphore::new(1)),
        })
    }

    /// The project switch count now (`Inner::generation`).
    fn generation(&self) -> u64 {
        lock(&self.inner).generation
    }

    /// The opening analysis serves now.
    pub fn opening(&self) -> Opening {
        Opening(self.generation())
    }

    /// `assets` as contents: each analysable clip's cache read, its asset put on its content (`on_content`), and the
    /// clips on one content grouped, in the order asked. None once the project changed since `generation`.
    async fn contents(&self, assets: &[Asset], generation: u64) -> Vec<(Content, Vec<String>)> {
        let mut contents: Vec<(Content, Vec<String>)> = Vec::new();
        for clip in clips_of(assets) {
            let cached = self.look(&clip).await;
            let key = key_of(&clip, &cached);
            if !self.on_content(&clip.id, &key, generation) {
                return Vec::new();
            }
            match contents.iter_mut().find(|(c, _)| c.key == key) {
                Some((_, ids)) => ids.push(clip.id),
                None => {
                    let id = clip.id.clone();
                    contents.push((Content { key, clip, cached }, vec![id]));
                }
            }
        }
        for (content, _) in &contents {
            self.show_cached(&content.key, &content.cached);
        }
        contents
    }

    /// What analysing `assets` costs (see the module notes): each content once, however many assets are on it. Reads
    /// each file's two ends for its cache key and shows what its cache holds (`show_cached`). When some need paying
    /// for, gets the adapter as a start would (`Credentials::ready`, which registers the device the first time; why it
    /// can't is `Estimate::problem`) and, with an access code, asks the relay for the credit left.
    /// An estimate the project switch overtook is empty.
    pub async fn estimate(&self, assets: &[Asset]) -> Estimate {
        self.estimate_at(self.opening(), assets).await
    }

    /// `estimate`, for assets read during `opening`: empty when the project has changed since.
    pub async fn estimate_at(&self, opening: Opening, assets: &[Asset]) -> Estimate {
        let generation = opening.0;
        let mut estimate = Estimate::default();
        let mut paid = Vec::new();
        for (content, ids) in self.contents(assets, generation).await {
            if content.cached.key.is_none() {
                estimate.missing.extend(ids);
            } else if content.cached.complete(&content.clip) {
                estimate.cached.extend(ids);
            } else {
                estimate.seconds += content.clip.duration_us as f64 / 1e6;
                estimate.clips.extend(ids);
                paid.push(content);
            }
        }
        if paid.is_empty() {
            return estimate;
        }
        let ready = self.credentials.ready().await;
        let Some(meta) = ready.meta else {
            estimate.problem = ready.problem;
            return estimate;
        };
        estimate.usd = paid.iter().map(|c| price(&meta, &c.clip, &c.cached)).sum();
        if meta.client().is_relay() {
            let status = self.credentials.status().await;
            let reserved = lock(&self.inner).reserved.values().sum();
            (estimate.credit_left_usd, estimate.refusal) = credit(&status, estimate.usd, reserved);
        }
        if self.generation() != generation {
            return Estimate::default();
        }
        estimate
    }

    /// Starts analysing `assets` (see the module notes): one pipeline per content, whose state and results reach every
    /// asset on it. A content another start took on is left to it (the assets asked for now hear it too). Cached
    /// contents load whatever else happens; the others start only when something can pay for them and, with an access
    /// code, their estimate is within the credit left (`Started::refused` says why not; what of them is cached shows
    /// all the same). Asked by the person (`start_as` tells the page's own starts apart).
    pub async fn start(self: &Arc<Self>, assets: &[Asset]) -> Started {
        self.start_as(assets, Asked::Person).await
    }

    /// `start`, asked by `asked`: cached words timed roughly are timed again when the person asks, and when the page
    /// asks only for a content not timed again (nor stopped by the person) in this app run.
    /// A start the project switch overtook (its assets belong to the project no longer open) does nothing and
    /// promises no credit.
    pub async fn start_as(self: &Arc<Self>, assets: &[Asset], asked: Asked) -> Started {
        self.start_at(self.opening(), assets, asked).await
    }

    /// `start_as`, for assets read during `opening`: nothing starts when the project has changed since.
    pub async fn start_at(
        self: &Arc<Self>,
        opening: Opening,
        assets: &[Asset],
        asked: Asked,
    ) -> Started {
        let generation = opening.0;
        let contents: Vec<Content> = {
            let looked = self.contents(assets, generation).await;
            let mut inner = lock(&self.inner);
            if inner.generation != generation {
                return Started::none();
            }
            looked
                .into_iter()
                .map(|(content, _)| content)
                .filter(|content| inner.claimed.insert(content.key.clone()))
                .collect()
        };
        // A clip whose file can't be found or opened has nothing to analyse or pay for: it fails at once, saying what
        // to do.
        let (missing, contents): (Vec<Content>, Vec<Content>) = contents
            .into_iter()
            .partition(|content| content.cached.key.is_none());
        for content in &missing {
            self.end(&content.key, Outcome::Failed(FILE_MISSING.into()));
        }
        let (ready, paid): (Vec<Content>, Vec<Content>) = contents
            .into_iter()
            .partition(|content| content.cached.complete(&content.clip));
        let (first, refused) = if paid.is_empty() {
            (None, None)
        } else {
            self.payer(&paid, generation).await
        };
        if let Some(reason) = &refused {
            tracing::info!(%reason, clips = paid.len(), "analysis: refused");
            let mut inner = lock(&self.inner);
            for content in &paid {
                inner.claimed.remove(&content.key);
            }
        }
        // Cached words timed roughly because word timing failed or stopped are timed again: free, so with whatever
        // adapter there is, whatever the credit; for the page's own starts, once per content per app run.
        let retime: HashSet<String> = {
            let mut inner = lock(&self.inner);
            ready
                .iter()
                .filter(|c| c.cached.transcript.as_ref().is_some_and(run::timed_again))
                .filter(|c| inner.timed_again.insert(c.key.clone()) || asked == Asked::Person)
                .map(|c| c.key.clone())
                .collect()
        };
        let timer = match (&first, retime.is_empty()) {
            (_, true) => None,
            (Some(meta), false) => Some(meta.clone()),
            (None, false) => self.credentials.ready().await.meta,
        };
        let Some(run) = self.run_now(generation) else {
            // The project changed meanwhile: what this start took on is let go.
            self.let_go(&ready, &paid, refused.is_some());
            return Started::none();
        };
        let starting = ready
            .into_iter()
            .map(|content| {
                let meta = timer.clone().filter(|_| retime.contains(&content.key));
                (content, meta)
            })
            .chain(
                paid.into_iter()
                    .filter(|_| first.is_some())
                    .map(|content| (content, first.clone())),
            );
        let mut tasks = Vec::new();
        for (content, meta) in starting {
            // Only a content with something to send shows as running: one whose results are all cached goes straight
            // to done, with no "Analyzing" on the way.
            if meta.is_some() {
                self.progress(&content.key, AnalysisStep::Audio, 0);
            }
            tasks.push(tokio::spawn(self.clone().analyse(
                content,
                run.clone(),
                meta,
            )));
        }
        Started { tasks, refused }
    }

    /// A start the project switch overtook lets go of what it took on: its claims and the credit it promised. A
    /// refused start let its paid contents go already, and promised nothing for them: another start may hold them now,
    /// with its own credit promised, so they are not this start's to let go.
    fn let_go(&self, ready: &[Content], paid: &[Content], refused: bool) {
        let paid = if refused { &[] } else { paid };
        let mut inner = lock(&self.inner);
        for content in ready.iter().chain(paid) {
            inner.claimed.remove(&content.key);
            inner.reserved.remove(&content.key);
        }
    }

    /// The adapter that pays for `paid`, or why there is none: no key or code, a code whose device can't register now
    /// (its sentence, never the ask for a key), or an estimate over an access code's credit left after what the runs
    /// going have promised. With an access code, each content's estimate is promised now, with the check (so two
    /// starts at once can't both take the same credit), and released as its calls cost or when it ends.
    async fn payer(&self, paid: &[Content], generation: u64) -> (Option<Meta>, Option<String>) {
        let ready = self.credentials.ready().await;
        let Some(meta) = ready.meta else {
            return (
                None,
                Some(ready.problem.unwrap_or_else(|| NEEDS_KEY.into())),
            );
        };
        if meta.client().is_relay() {
            let prices: Vec<(String, f64)> = paid
                .iter()
                .map(|c| (c.key.clone(), price(&meta, &c.clip, &c.cached)))
                .collect();
            let usd = prices.iter().map(|(_, usd)| usd).sum();
            let status = self.credentials.status().await;
            let mut inner = lock(&self.inner);
            if inner.generation != generation {
                // Another project opened meanwhile: nothing is promised for this one's clips.
                return (None, None);
            }
            let (_, refusal) = credit(&status, usd, inner.reserved.values().sum());
            if refusal.is_some() {
                return (None, refusal);
            }
            inner.reserved.extend(prices);
        }
        (Some(meta), None)
    }

    /// The run new clips join: the last one, unless it was cancelled or stopped paying. None once the project changed
    /// since `generation`.
    fn run_now(&self, generation: u64) -> Option<Arc<Run>> {
        let mut inner = lock(&self.inner);
        if inner.generation != generation {
            return None;
        }
        inner.runs.retain(|run| run.strong_count() > 0);
        if let Some(run) = inner.runs.last().and_then(Weak::upgrade)
            && run.open()
        {
            return Some(run);
        }
        let run = Arc::new(Run::default());
        inner.runs.push(Arc::downgrade(&run));
        Some(run)
    }

    /// Sends nothing more for the runs going: the requests in flight finish, and their clips go back to not analysed.
    pub fn cancel(&self) {
        let inner = lock(&self.inner);
        for run in inner.runs.iter().filter_map(Weak::upgrade) {
            run.cancel();
        }
    }

    /// Another project opened, or the project closed: the runs going are cancelled (their requests in flight finish,
    /// and an uploaded clip is still deleted), and the old project's assets are forgotten, so nothing more is told
    /// about them and the engine's speech starts empty. What the files' analysis found stays (as their cache does):
    /// the next project's assets on the same files get it at once.
    pub fn project_changed(&self) {
        self.cancel();
        let mut inner = lock(&self.inner);
        inner.generation += 1;
        inner.keys.clear();
        inner.speech = Speech::default();
        (self.speech_sink)(Arc::new(inner.speech.clone()));
    }

    /// These assets left the project (removed, not the project closing): they are forgotten, so they get no more
    /// events or speech. Their file's analysis carries on for any other asset on it; nothing is cancelled.
    pub fn forget(&self, asset_ids: &[String]) {
        let mut inner = lock(&self.inner);
        let mut changed = false;
        for id in asset_ids {
            inner.keys.remove(id);
            changed |= inner.speech.transcripts.remove(id).is_some();
            changed |= inner.speech.levels.remove(id).is_some();
        }
        if changed {
            (self.speech_sink)(Arc::new(inner.speech.clone()));
        }
    }

    /// Where `asset_id`'s analysis is.
    pub fn state_of(&self, asset_id: &str) -> AssetState {
        let inner = lock(&self.inner);
        inner
            .keys
            .get(asset_id)
            .and_then(|key| inner.entries.get(key))
            .map(|e| e.state.clone())
            .unwrap_or_default()
    }

    /// Every asset analysis knows of in the open project, and where it is.
    pub fn states(&self) -> Vec<AssetAnalysisState> {
        let inner = lock(&self.inner);
        let mut states: Vec<AssetAnalysisState> = inner
            .keys
            .iter()
            .map(|(id, key)| AssetAnalysisState {
                asset_id: id.clone(),
                state: inner
                    .entries
                    .get(key)
                    .map(|e| e.state.clone())
                    .unwrap_or_default(),
            })
            .collect();
        states.sort_by(|a, b| a.asset_id.cmp(&b.asset_id));
        states
    }

    /// What `asset_id`'s analysis found so far, if anything.
    pub fn analysis_for(&self, asset_id: &str) -> Option<Arc<AssetAnalysis>> {
        let inner = lock(&self.inner);
        inner
            .keys
            .get(asset_id)
            .and_then(|key| inner.entries.get(key))
            .map(|e| e.analysis.clone())
            .filter(|a| a.transcript.is_some() || a.scenes.is_some())
    }

    /// What this session's analysis calls cost, in US dollars: every file analysed since the app started, each once
    /// (what came from the cache cost nothing).
    pub fn spent_usd(&self) -> f64 {
        let inner = lock(&self.inner);
        inner.entries.values().map(|e| e.analysis.spent_usd).sum()
    }

    /// The speech found so far, as the engine has it.
    pub fn speech(&self) -> Arc<Speech> {
        Arc::new(lock(&self.inner).speech.clone())
    }

    /// The cache root (`Klipolo/analysis`).
    pub fn cache_root(&self) -> &Path {
        &self.cache_root
    }

    /// Asset `asset_id` is on content `key`: it gets what the content has found (speech for the engine), and hears
    /// where its analysis is when it is under way or over. False (and nothing done) once the project changed since
    /// `generation`.
    fn on_content(&self, asset_id: &str, key: &str, generation: u64) -> bool {
        let mut inner = lock(&self.inner);
        if inner.generation != generation {
            return false;
        }
        if inner.keys.get(asset_id).is_some_and(|k| k == key) {
            return true;
        }
        inner.keys.insert(asset_id.to_owned(), key.to_owned());
        let entry = inner.entry(key);
        let (transcript, levels, state) = (
            entry.analysis.transcript.clone(),
            entry.levels.clone(),
            entry.state.clone(),
        );
        let mut published = false;
        if let Some(transcript) = transcript {
            inner
                .speech
                .transcripts
                .insert(asset_id.to_owned(), transcript);
            published = true;
        }
        if let Some(levels) = levels {
            inner.speech.levels.insert(asset_id.to_owned(), levels);
            published = true;
        }
        if published {
            (self.speech_sink)(Arc::new(inner.speech.clone()));
        }
        if let Some(event) = event_for(asset_id, &state) {
            (self.events)(event);
        }
        true
    }

    /// Content `key`'s `step` is `pct` of the way: told to each of its assets when it changed.
    fn progress(&self, key: &str, step: AnalysisStep, pct: u8) {
        let mut inner = lock(&self.inner);
        let state = AssetState::Running {
            step,
            pct: pct.min(100),
        };
        let entry = inner.entry(key);
        if entry.state == state {
            return;
        }
        entry.state = state.clone();
        for id in inner.ids_of(key) {
            if let Some(event) = event_for(&id, &state) {
                (self.events)(event);
            }
        }
    }

    /// How content `key`'s analysis ended, kept and told to each of its assets.
    fn end(&self, key: &str, outcome: Outcome) {
        let mut inner = lock(&self.inner);
        let state = match outcome {
            Outcome::Done(words) => words.state(),
            Outcome::Failed(reason) => AssetState::Failed { reason },
            Outcome::Stopped => AssetState::None,
        };
        inner.entry(key).state = state.clone();
        inner.claimed.remove(key);
        inner.reserved.remove(key);
        for id in inner.ids_of(key) {
            let stopped = AnalysisEvent::Stopped(AnalysisStopped {
                asset_id: id.clone(),
            });
            let event = event_for(&id, &state).unwrap_or(stopped);
            (self.events)(event);
        }
    }

    /// What the cache holds for content `key`, for the engine and chat at once, where they have none of it yet: a clip
    /// analysed before shows its captions as soon as it is seen, whatever is left to do.
    fn show_cached(&self, key: &str, cached: &Cached) {
        let (transcript, scenes, levels) = match lock(&self.inner).entries.get(key) {
            Some(entry) => (
                entry.analysis.transcript.is_none(),
                entry.analysis.scenes.is_none(),
                entry.levels.is_none(),
            ),
            None => (true, true, true),
        };
        if let Some(found) = cached.levels.as_ref().filter(|_| levels) {
            self.found_levels(key, found.clone());
        }
        if let Some(found) = cached.transcript.as_ref().filter(|_| transcript) {
            self.found_transcript(key, found);
        }
        if let Some(found) = cached.scenes.as_ref().filter(|_| scenes) {
            self.found_scenes(key, found.clone());
        }
    }

    /// Content `key`'s analysis, changed by `edit`.
    fn edit(inner: &mut Inner, key: &str, edit: impl FnOnce(&mut AssetAnalysis)) {
        let entry = inner.entry(key);
        let mut analysis = (*entry.analysis).clone();
        edit(&mut analysis);
        entry.analysis = Arc::new(analysis);
    }

    /// A transcript for content `key` (turn-level, timed, or cached): kept, and published to the engine for each of
    /// its assets.
    fn found_transcript(&self, key: &str, out: &TranscriptOut) {
        let mut inner = lock(&self.inner);
        let transcript = Arc::new(out.transcript.clone());
        for id in inner.ids_of(key) {
            inner.speech.transcripts.insert(id, transcript.clone());
        }
        Self::edit(&mut inner, key, |a| {
            a.transcript = Some(transcript);
            a.turns = out.turns.clone();
            a.approximate = out.approximate;
        });
        // Under the lock: a newer `Speech` never reaches the engine before an older one.
        (self.speech_sink)(Arc::new(inner.speech.clone()));
    }

    /// Content `key`'s sound levels: kept, and published to the engine for each of its assets.
    fn found_levels(&self, key: &str, levels: Levels) {
        let mut inner = lock(&self.inner);
        let levels = Arc::new(levels);
        for id in inner.ids_of(key) {
            inner.speech.levels.insert(id, levels.clone());
        }
        inner.entry(key).levels = Some(levels);
        (self.speech_sink)(Arc::new(inner.speech.clone()));
    }

    /// Content `key`'s scene log: kept for chat.
    fn found_scenes(&self, key: &str, log: SceneLog) {
        let mut inner = lock(&self.inner);
        Self::edit(&mut inner, key, |a| a.scenes = Some(Arc::new(log)));
    }

    /// Calls for content `key` cost `usd` (charged now: no longer only promised).
    fn spend(&self, key: &str, usd: f64) {
        let mut inner = lock(&self.inner);
        if let Some(promised) = inner.reserved.get_mut(key) {
            *promised = (*promised - usd).max(0.0);
        }
        Self::edit(&mut inner, key, |a| a.spent_usd += usd);
    }
}

/// The event that tells asset `asset_id` its analysis is at `state`; none for not analysed.
fn event_for(asset_id: &str, state: &AssetState) -> Option<AnalysisEvent> {
    let asset_id = asset_id.to_owned();
    Some(match state {
        AssetState::None => return None,
        AssetState::Running { step, pct } => AnalysisEvent::Progress(AnalysisProgress {
            asset_id,
            step: *step,
            pct: *pct,
        }),
        AssetState::Done {
            approximate,
            approximate_reason,
            empty_reason,
            time_again,
        } => AnalysisEvent::Done(AnalysisDone {
            asset_id,
            approximate: *approximate,
            approximate_reason: approximate_reason.clone(),
            empty_reason: empty_reason.clone(),
            time_again: *time_again,
        }),
        AssetState::Failed { reason } => AnalysisEvent::Failed(AnalysisFailed {
            asset_id,
            reason: reason.clone(),
        }),
    })
}

/// The analysable clips among `assets`, each once, in order.
fn clips_of(assets: &[Asset]) -> Vec<Clip> {
    let mut clips: Vec<Clip> = Vec::new();
    for clip in assets.iter().filter_map(Clip::of) {
        if !clips.iter().any(|c| c.id == clip.id) {
            clips.push(clip);
        }
    }
    clips
}

/// What the steps of `clip` not in `cached` are expected to cost through `meta`.
fn price(meta: &Meta, clip: &Clip, cached: &Cached) -> f64 {
    let mut usd = 0.0;
    if cached.transcript.is_none() && clip.has_audio {
        usd += transcript::estimate_usd(meta, clip.duration_us);
    }
    if clip.video && cached.scenes.is_none() {
        usd += scenes::estimate_usd(meta, clip.duration_us);
    }
    usd
}

/// With an access code: the credit left (when the relay said), less what the runs going have `reserved`, and why `usd`
/// of analysis is refused (a code turned off, or more than is left). Analysis covers each file whole, however its clip
/// is trimmed, so the way on is fewer or shorter files.
fn credit(status: &AiStatus, usd: f64, reserved: f64) -> (Option<f64>, Option<String>) {
    if status.mode != AiMode::Relay {
        return (None, None);
    }
    let left = match (status.budget_usd, status.spent_usd) {
        (Some(budget), Some(spent)) => Some((budget - spent - reserved).max(0.0)),
        _ => None,
    };
    if status.active == Some(false) {
        let notice = status
            .notice
            .clone()
            .unwrap_or_else(|| CODE_INACTIVE.into());
        return (left, Some(notice));
    }
    let refusal = left.filter(|&left| usd > left).map(|left| {
        format!(
            "Your access code has ${left:.2} left, and this needs {}. Analyze fewer or shorter files, or add your own \
             Meta key in Settings.",
            about(usd)
        )
    });
    (left, refusal)
}

/// "about $0.07", or "less than $0.01".
fn about(usd: f64) -> String {
    if usd < 0.01 {
        "less than $0.01".into()
    } else {
        format!("about ${usd:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::MemoryStore;

    /// An analysis with nothing to pay with and nowhere to tell: for its bookkeeping alone.
    fn bare() -> Arc<AnalysisState> {
        let credentials = Credentials::load(
            Arc::new(MemoryStore::default()),
            crate::credentials::Endpoints {
                meta_base: "http://127.0.0.1:9/v1".into(),
                relay_base: "http://127.0.0.1:9".into(),
            },
            "device".into(),
        );
        AnalysisState::new(
            Arc::new(credentials),
            Arc::new(EngineMedia::new(None)),
            std::env::temp_dir().join("klipolo-let-go-test"),
            std::env::temp_dir().join("klipolo-let-go-test-work"),
            Arc::new(|_| {}),
            Arc::new(|_| {}),
        )
    }

    fn content(key: &str) -> Content {
        Content {
            key: key.into(),
            clip: Clip {
                id: key.into(),
                path: PathBuf::from("/clips/x.mov"),
                video: true,
                has_audio: true,
                duration_us: 1_000_000,
            },
            cached: Cached::default(),
        }
    }

    /// A refused start already let its paid contents go (another start may hold them since, with credit promised):
    /// overtaken by a switch, it lets go only of what it still holds. One that wasn't refused holds its paid ones too.
    #[test]
    fn a_refused_start_overtaken_by_a_switch_lets_go_only_of_what_it_holds() {
        let state = bare();
        {
            let mut inner = lock(&state.inner);
            // The overtaken start holds "ready"; a new start holds "paid", with its credit promised.
            inner
                .claimed
                .extend(["ready".to_owned(), "paid".to_owned()]);
            inner.reserved.insert("paid".into(), 0.07);
        }
        state.let_go(&[content("ready")], &[content("paid")], true);
        {
            let inner = lock(&state.inner);
            assert!(!inner.claimed.contains("ready"));
            assert!(
                inner.claimed.contains("paid"),
                "the new start keeps its claim"
            );
            assert_eq!(
                inner.reserved.get("paid"),
                Some(&0.07),
                "and its promised credit"
            );
        }
        state.let_go(&[], &[content("paid")], false);
        let inner = lock(&state.inner);
        assert!(!inner.claimed.contains("paid") && inner.reserved.is_empty());
    }

    #[test]
    fn credit_is_refused_only_with_a_code_and_over_what_is_left() {
        let relay = |budget, spent| AiStatus {
            mode: AiMode::Relay,
            budget_usd: Some(budget),
            spent_usd: Some(spent),
            active: Some(true),
            ..AiStatus::default()
        };
        assert_eq!(credit(&relay(2.0, 0.5), 0.07, 0.0), (Some(1.5), None));
        // What runs going have promised counts as spent.
        let (left, refusal) = credit(&relay(2.0, 1.9), 0.07, 0.07);
        assert!((left.unwrap() - 0.03).abs() < 1e-9);
        assert!(
            refusal
                .unwrap()
                .starts_with("Your access code has $0.03 left")
        );
        let (left, refusal) = credit(&relay(2.0, 1.995), 0.004, 0.0);
        assert!((left.unwrap() - 0.005).abs() < 1e-12);
        assert_eq!(refusal, None);
        assert_eq!(
            credit(&relay(2.0, 2.4), 0.004, 0.0).1.as_deref(),
            Some(
                "Your access code has $0.00 left, and this needs less than $0.01. Analyze fewer or shorter files, \
                 or add your own Meta key in Settings."
            )
        );
        let off = AiStatus {
            active: Some(false),
            notice: Some("This access code is no longer active.".into()),
            ..relay(2.0, 0.0)
        };
        assert_eq!(
            credit(&off, 0.0, 0.0).1.as_deref(),
            Some("This access code is no longer active.")
        );
        // The relay didn't say: nothing to refuse on.
        let unknown = AiStatus {
            budget_usd: None,
            ..relay(2.0, 0.0)
        };
        assert_eq!(credit(&unknown, 9.0, 0.0), (None, None));
        let key = AiStatus {
            mode: AiMode::Key,
            ..AiStatus::default()
        };
        assert_eq!(credit(&key, 9.0, 0.0), (None, None));
    }
}
