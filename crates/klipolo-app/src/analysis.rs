//! Analysis in the app: after import, each clip's transcript, word times, scene log and sound levels, with the cost
//! shown first, progress as it goes, results cached per file, and a clean stop when the credit runs out. Captions and
//! chat read what it finds.
//!
//! - **Estimate** (`AnalysisState::estimate`): what analysing the clips asked for costs at Meta's list prices (the
//!   providers' own estimates: `transcript::estimate_usd`, `scenes::estimate_usd`), how long they are, and which are
//!   cached already (they cost nothing). With an access code it also gives the credit left, and why Analyze is refused
//!   when the estimate is over it.
//! - **Start** (`AnalysisState::start`): cached clips load at once, with no network call. The others run each its own
//!   pipeline, all at once (the Meta adapter keeps the device's request slots): its sound (step `audio`), its speaker
//!   turns (`transcript`, published at once as turn-level captions), its word times (`words`), then its scene log
//!   (`scenes`) for a video. Decoding (the sound, the scene log's small copy) runs one clip at a time on a thread of
//!   utility priority, and one video call runs at a time.
//! - **Results** go into the engine's `Speech` (a new `Arc<Speech>` for the open revision: the captions update live),
//!   with each clip's sound levels (`Speech::levels`, from the sound cache's energies), and are kept here for chat
//!   (`AnalysisState::analysis_for`). Each finished step is cached (`klipolo_analysis::cache`): `transcript.json` once
//!   its words are timed, `scenes.json` once logged. Nothing half done is cached.
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
    Done { approximate: bool },
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
    /// With an access code: the credit left, when Klipolo's relay said.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub credit_left_usd: Option<f64>,
    /// Why analysing `clips` is refused now (more than the credit left, or a code turned off), as one sentence.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub refusal: Option<String>,
}

/// What a clip's analysis found so far: what chat reads (Plan 4), in the clip's own times (words and turns in its sound
/// time, the scene log in its file time).
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
    /// What this session's calls for it cost, in US dollars (nothing for what came from the cache).
    pub spent_usd: f64,
}

/// What a `start` set going.
pub struct Started {
    tasks: Vec<JoinHandle<()>>,
    /// Why the clips that needed paying for didn't start (cached ones load all the same).
    pub refused: Option<String>,
}

impl Started {
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

/// A clip's cached results.
#[derive(Default)]
pub(crate) struct Cached {
    pub key: Option<String>,
    pub transcript: Option<TranscriptOut>,
    pub scenes: Option<SceneLog>,
}

impl Cached {
    /// Every step `clip` needs is cached.
    fn complete(&self, clip: &Clip) -> bool {
        self.transcript.is_some() && (!clip.video || self.scenes.is_some())
    }
}

#[derive(Default)]
struct Entry {
    state: AssetState,
    analysis: Arc<AssetAnalysis>,
}

#[derive(Default)]
struct Inner {
    assets: HashMap<String, Entry>,
    /// The clips a `start` took on, until their analysis ends: another start leaves them alone.
    claimed: HashSet<String>,
    speech: Speech,
    /// The runs started, while any of their clips is still going (`cancel` stops them all).
    runs: Vec<Weak<Run>>,
}

/// The app's analysis: each clip's state and results, and the runs going. Make one per app.
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

    /// What analysing `assets` costs (see the module notes). Reads each file's two ends for its cache key; asks the
    /// relay for the credit left with an access code.
    pub async fn estimate(&self, assets: &[Asset]) -> Estimate {
        let meta = self.credentials.meta();
        let mut estimate = Estimate::default();
        for clip in clips_of(assets) {
            let cached = self.look(&clip).await;
            if cached.complete(&clip) {
                estimate.cached.push(clip.id);
                continue;
            }
            estimate.seconds += clip.duration_us as f64 / 1e6;
            if let Some(meta) = &meta {
                estimate.usd += price(meta, &clip, &cached);
            }
            estimate.clips.push(clip.id);
        }
        let relay = meta.is_some_and(|m| m.client().is_relay());
        if relay && !estimate.clips.is_empty() {
            let status = self.credentials.status().await;
            (estimate.credit_left_usd, estimate.refusal) = credit(&status, estimate.usd);
        }
        estimate
    }

    /// Starts analysing `assets` (see the module notes). A clip another start took on is left to it. Cached clips load
    /// whatever else happens; the others start only when something can pay for them and, with an access code, their
    /// estimate is within the credit left (`Started::refused` says why not).
    pub async fn start(self: &Arc<Self>, assets: &[Asset]) -> Started {
        let clips: Vec<Clip> = {
            let mut inner = lock(&self.inner);
            clips_of(assets)
                .into_iter()
                .filter(|clip| inner.claimed.insert(clip.id.clone()))
                .collect()
        };
        let mut ready = Vec::new();
        let mut paid = Vec::new();
        for clip in clips {
            let cached = self.look(&clip).await;
            if cached.complete(&clip) {
                ready.push((clip, cached));
            } else {
                paid.push((clip, cached));
            }
        }
        let (first, refused) = if paid.is_empty() {
            (None, None)
        } else {
            self.payer(&paid).await
        };
        if let Some(reason) = &refused {
            tracing::info!(%reason, clips = paid.len(), "analysis: refused");
            let mut inner = lock(&self.inner);
            for (clip, _) in &paid {
                inner.claimed.remove(&clip.id);
            }
        }
        let run = self.run_now();
        let starting = ready
            .into_iter()
            .map(|(clip, cached)| (clip, cached, None))
            .chain(
                paid.into_iter()
                    .filter(|_| first.is_some())
                    .map(|(clip, cached)| (clip, cached, first.clone())),
            );
        let mut tasks = Vec::new();
        for (clip, cached, meta) in starting {
            self.progress(&clip.id, AnalysisStep::Audio, 0);
            tasks.push(tokio::spawn(self.clone().analyse(
                clip,
                cached,
                run.clone(),
                meta,
            )));
        }
        Started { tasks, refused }
    }

    /// The adapter that pays for `paid`, or why there is none: no key or code, a code whose device can't register now
    /// (its sentence, never the ask for a key), or an estimate over an access code's credit.
    async fn payer(&self, paid: &[(Clip, Cached)]) -> (Option<Meta>, Option<String>) {
        let ready = self.credentials.ready().await;
        let Some(meta) = ready.meta else {
            return (
                None,
                Some(ready.problem.unwrap_or_else(|| NEEDS_KEY.into())),
            );
        };
        if meta.client().is_relay() {
            let usd = paid
                .iter()
                .map(|(clip, cached)| price(&meta, clip, cached))
                .sum();
            let (_, refusal) = credit(&self.credentials.status().await, usd);
            if refusal.is_some() {
                return (None, refusal);
            }
        }
        (Some(meta), None)
    }

    /// The run new clips join: the last one, unless it was cancelled or stopped paying.
    fn run_now(&self) -> Arc<Run> {
        let mut inner = lock(&self.inner);
        inner.runs.retain(|run| run.strong_count() > 0);
        if let Some(run) = inner.runs.last().and_then(Weak::upgrade)
            && run.open()
        {
            return run;
        }
        let run = Arc::new(Run::default());
        inner.runs.push(Arc::downgrade(&run));
        run
    }

    /// Sends nothing more for the runs going: the requests in flight finish, and their clips go back to not analysed.
    pub fn cancel(&self) {
        let inner = lock(&self.inner);
        for run in inner.runs.iter().filter_map(Weak::upgrade) {
            run.cancel();
        }
    }

    /// Where `asset_id`'s analysis is.
    pub fn state_of(&self, asset_id: &str) -> AssetState {
        lock(&self.inner)
            .assets
            .get(asset_id)
            .map(|e| e.state.clone())
            .unwrap_or_default()
    }

    /// Every clip analysis knows of, and where it is.
    pub fn states(&self) -> Vec<AssetAnalysisState> {
        let inner = lock(&self.inner);
        let mut states: Vec<AssetAnalysisState> = inner
            .assets
            .iter()
            .map(|(id, e)| AssetAnalysisState {
                asset_id: id.clone(),
                state: e.state.clone(),
            })
            .collect();
        states.sort_by(|a, b| a.asset_id.cmp(&b.asset_id));
        states
    }

    /// What `asset_id`'s analysis found so far, if anything.
    pub fn analysis_for(&self, asset_id: &str) -> Option<Arc<AssetAnalysis>> {
        lock(&self.inner)
            .assets
            .get(asset_id)
            .map(|e| e.analysis.clone())
            .filter(|a| a.transcript.is_some() || a.scenes.is_some())
    }

    /// The speech found so far, as the engine has it.
    pub fn speech(&self) -> Arc<Speech> {
        Arc::new(lock(&self.inner).speech.clone())
    }

    /// The cache root (`Klipolo/analysis`).
    pub fn cache_root(&self) -> &Path {
        &self.cache_root
    }

    /// `asset_id`'s `step` is `pct` of the way: told when it changed.
    fn progress(&self, asset_id: &str, step: AnalysisStep, pct: u8) {
        let mut inner = lock(&self.inner);
        let state = AssetState::Running {
            step,
            pct: pct.min(100),
        };
        let entry = inner.assets.entry(asset_id.to_owned()).or_default();
        if entry.state == state {
            return;
        }
        entry.state = state;
        (self.events)(AnalysisEvent::Progress(AnalysisProgress {
            asset_id: asset_id.to_owned(),
            step,
            pct: pct.min(100),
        }));
    }

    /// How `asset_id`'s analysis ended, kept and told.
    fn end(&self, asset_id: &str, outcome: Outcome) {
        let mut inner = lock(&self.inner);
        let id = asset_id.to_owned();
        let (state, event) = match outcome {
            Outcome::Done { approximate } => (
                AssetState::Done { approximate },
                AnalysisEvent::Done(AnalysisDone {
                    asset_id: id,
                    approximate,
                }),
            ),
            Outcome::Failed(reason) => (
                AssetState::Failed {
                    reason: reason.clone(),
                },
                AnalysisEvent::Failed(AnalysisFailed {
                    asset_id: id,
                    reason,
                }),
            ),
            Outcome::Stopped => (
                AssetState::None,
                AnalysisEvent::Stopped(AnalysisStopped { asset_id: id }),
            ),
        };
        inner.assets.entry(asset_id.to_owned()).or_default().state = state;
        inner.claimed.remove(asset_id);
        (self.events)(event);
    }

    /// `asset_id`'s analysis, changed by `edit`.
    fn edit(inner: &mut Inner, asset_id: &str, edit: impl FnOnce(&mut AssetAnalysis)) {
        let entry = inner.assets.entry(asset_id.to_owned()).or_default();
        let mut analysis = (*entry.analysis).clone();
        edit(&mut analysis);
        entry.analysis = Arc::new(analysis);
    }

    /// A transcript for `asset_id` (turn-level, timed, or cached): kept, and published to the engine.
    fn found_transcript(&self, asset_id: &str, out: &TranscriptOut) {
        let mut inner = lock(&self.inner);
        let transcript = Arc::new(out.transcript.clone());
        inner
            .speech
            .transcripts
            .insert(asset_id.to_owned(), transcript.clone());
        Self::edit(&mut inner, asset_id, |a| {
            a.transcript = Some(transcript);
            a.turns = out.turns.clone();
            a.approximate = out.approximate;
        });
        // Under the lock: a newer `Speech` never reaches the engine before an older one.
        (self.speech_sink)(Arc::new(inner.speech.clone()));
    }

    /// `asset_id`'s sound levels: kept, and published to the engine.
    fn found_levels(&self, asset_id: &str, levels: Levels) {
        let mut inner = lock(&self.inner);
        inner
            .speech
            .levels
            .insert(asset_id.to_owned(), Arc::new(levels));
        (self.speech_sink)(Arc::new(inner.speech.clone()));
    }

    /// `asset_id`'s scene log: kept for chat.
    fn found_scenes(&self, asset_id: &str, log: SceneLog) {
        let mut inner = lock(&self.inner);
        Self::edit(&mut inner, asset_id, |a| a.scenes = Some(Arc::new(log)));
    }

    /// Calls for `asset_id` cost `usd`.
    fn spend(&self, asset_id: &str, usd: f64) {
        let mut inner = lock(&self.inner);
        Self::edit(&mut inner, asset_id, |a| a.spent_usd += usd);
    }
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

/// With an access code: the credit left (when the relay said), and why `usd` of analysis is refused (a code turned
/// off, or more than is left).
fn credit(status: &AiStatus, usd: f64) -> (Option<f64>, Option<String>) {
    if status.mode != AiMode::Relay {
        return (None, None);
    }
    let left = match (status.budget_usd, status.spent_usd) {
        (Some(budget), Some(spent)) => Some((budget - spent).max(0.0)),
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
            "Your access code has ${left:.2} left, and this needs {}. Trim the clips first, or add your own Meta key \
             in Settings.",
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

    #[test]
    fn credit_is_refused_only_with_a_code_and_over_what_is_left() {
        let relay = |budget, spent| AiStatus {
            mode: AiMode::Relay,
            budget_usd: Some(budget),
            spent_usd: Some(spent),
            active: Some(true),
            ..AiStatus::default()
        };
        assert_eq!(credit(&relay(2.0, 0.5), 0.07), (Some(1.5), None));
        let (left, refusal) = credit(&relay(2.0, 1.995), 0.004);
        assert!((left.unwrap() - 0.005).abs() < 1e-12);
        assert_eq!(refusal, None);
        assert_eq!(
            credit(&relay(2.0, 2.4), 0.004).1.as_deref(),
            Some(
                "Your access code has $0.00 left, and this needs less than $0.01. Trim the clips first, or add \
                 your own Meta key in Settings."
            )
        );
        let off = AiStatus {
            active: Some(false),
            notice: Some("This access code is no longer active.".into()),
            ..relay(2.0, 0.0)
        };
        assert_eq!(
            credit(&off, 0.0).1.as_deref(),
            Some("This access code is no longer active.")
        );
        // The relay didn't say: nothing to refuse on.
        let unknown = AiStatus {
            budget_usd: None,
            ..relay(2.0, 0.0)
        };
        assert_eq!(credit(&unknown, 9.0), (None, None));
        let key = AiStatus {
            mode: AiMode::Key,
            ..AiStatus::default()
        };
        assert_eq!(credit(&key, 9.0), (None, None));
    }
}
