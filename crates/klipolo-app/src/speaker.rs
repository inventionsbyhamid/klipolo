//! Follow the speaker: a clip's vertical or square crop kept on the person talking, from SAM's boxes on a small, low
//! frame rate copy of only the footage the clips play (`klipolo_analysis::subject`).
//!
//! - **Price first** (`Speaker::estimate`): each stretch to send, at the tracker's price; stretches tracked before are
//!   in the analysis cache and free. With an access code the run is refused when it costs more than the credit left.
//!   Nothing is sent to price it.
//! - **The run** (`Speaker::follow`): each stretch copied small, sent, cached, its progress told (`SpeakerProgress`),
//!   its cost counted into the session's spend (`spent_usd`); then each clip's path in each format whose Fill crops
//!   it. A used-up credit stops it with the relay's sentence.
//! - **Who speaks** (`klipolo_analysis::subject::voices`): following a person in a clip whose people don't all fit
//!   the crop, each voice of its file's transcript (`Voices`) is matched to one of them by Muse Spark (`Trackers::
//!   matcher`), priced with the tracking (at most, before anything is tracked) and cached beside it, so the crop keeps
//!   whoever is speaking. Who the card or chat keeps instead (`Keep`) is made from the cache at no charge. A matcher
//!   that fails leaves the follow as it was, framing each shot's main subject.
//! - **Landing** is the caller's: the paths are in the clips' source time, so `Followed::ops` builds the ops on the
//!   project as it is once the run is done (a hand edit meanwhile is kept), as one undo step.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, PoisonError};

use klipolo_analysis::cache::{Dir, content_key};
use klipolo_analysis::providers::{BoxFuture, Heard, SubjectTracker, VoiceMatcher};
use klipolo_analysis::subject::clips::{
    Footage, Speech, clip_people, follow_op_keeping, followable_formats, footage, speaker_path,
    voice_windows,
};
use klipolo_analysis::subject::voices::{self, Asking, VoiceTurn, Window};
use klipolo_analysis::subject::{self, Ask, FPS, PERSON, SubjectCopy, SubjectTrack};
use klipolo_model::fill_focus::{FocusPath, Keep, Person, Voice};
use klipolo_model::ids::Id;
use klipolo_model::ops::Op;
use klipolo_model::project::{FormatKind, Project};
use klipolo_model::time::RangeUs;
use serde::Serialize;
use ts_rs::TS;

use crate::analysis::{AnalysisState, NEEDS_KEY};
use crate::credentials::{AiMode, CODE_INACTIVE, Credentials};

/// The event a follow's progress goes out on.
pub const PROGRESS_EVENT: &str = "speaker://progress";
/// Said when the footage has nobody in it to follow.
pub const NO_ONE: &str = "Klipolo couldn't find a person to follow in this clip.";
/// Said when matching voices to people stopped partway: no voice is used.
pub const UNMATCHED: &str =
    "Klipolo couldn't tell who is speaking, so the crop follows the main person.";
/// Said when who to keep can't be made from what was tracked before: the tracking isn't in the cache any more.
pub const NOT_TRACKED: &str =
    "This footage isn't tracked any more. Follow the speaker again first.";

/// What following the speaker costs before it runs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpeakerEstimate {
    /// About what it costs, in US dollars, at Meta's list price: 0 when every stretch was tracked before.
    pub usd: f64,
    /// The clips asked for that can follow: an imported video or a camera take, cropped in a format asked for.
    pub clips: Vec<String>,
    /// Every stretch was tracked before: it costs nothing.
    pub cached: bool,
    /// Why it can't run now (no key or code, or more than an access code's credit left), as one sentence.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub refusal: Option<String>,
}

/// `speaker://progress`: how far a follow is.
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpeakerProgress {
    pub clip_ids: Vec<String>,
    /// 0 to 100.
    pub pct: u8,
}

/// What a follow found: each clip's path per format, in its source time.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Followed {
    pub paths: Vec<(Id, BTreeMap<FormatKind, FocusPath>)>,
    /// Each clip's keep and the people its crop chose between.
    pub kept: BTreeMap<Id, (Keep, Vec<Person>)>,
    /// The clips with nobody to follow.
    pub not_found: Vec<Id>,
    /// What the run cost (0 for footage tracked before).
    pub paid_usd: f64,
    /// Why no voice could be matched to the people (`UNMATCHED`), to say to the person.
    pub note: Option<String>,
}

impl Followed {
    /// The ops that make the clips follow on `p` as it is now, one per clip still there, each only in the formats it
    /// may still follow in there: a format turned to Fit while it tracked stays Fit.
    pub fn ops(&self, p: &Project) -> Vec<Op> {
        (self.paths.iter())
            .filter_map(|(id, paths)| {
                let clip = p.timeline.main.iter().find(|c| &c.id == id)?;
                let formats: Vec<FormatKind> = paths.keys().copied().collect();
                let still = followable_formats(p, clip, &formats);
                let paths: BTreeMap<FormatKind, FocusPath> = (paths.iter())
                    .filter(|(f, _)| still.contains(f))
                    .map(|(f, path)| (*f, path.clone()))
                    .collect();
                let (keep, people) = self.kept.get(id).cloned().unwrap_or_default();
                (!paths.is_empty()).then(|| follow_op_keeping(p, clip, &paths, &keep, &people))
            })
            .collect()
    }
}

/// A file's voices as its transcript heard them.
pub trait Voices: Send + Sync {
    /// The speaker turns of `asset_id`'s transcript, in its sound time (`Asset::lead_us` before its picture's), each
    /// with its part and label; none when it has no transcript yet.
    fn turns(&self, asset_id: &str) -> Vec<VoiceTurn>;
}

/// The tracker as the saved credentials allow it, and an access code's credit.
pub trait Trackers: Send + Sync {
    /// The tracker, or why nothing can pay for it, in one sentence.
    fn tracker(&self) -> BoxFuture<'_, Result<Arc<dyn SubjectTracker>, String>>;

    /// With an access code: the credit left, when Klipolo's relay says. None with a key.
    fn credit_left(&self) -> BoxFuture<'_, Option<f64>>;

    /// `credit_left`, or why nothing can be paid for now (an access code that was turned off), in one sentence.
    fn credit(&self) -> BoxFuture<'_, Result<Option<f64>, String>> {
        Box::pin(async move { Ok(self.credit_left().await) })
    }

    /// What tells whose a voice is, when there is one: without it, the crop frames each shot's main subject.
    fn matcher(&self) -> BoxFuture<'_, Option<Arc<dyn VoiceMatcher>>> {
        Box::pin(async move { None })
    }
}

/// SAM through the app's shared Meta adapter (the person's key, or an access code through the relay).
pub struct MetaTrackers {
    credentials: Arc<Credentials>,
}

impl MetaTrackers {
    pub fn new(credentials: Arc<Credentials>) -> Self {
        Self { credentials }
    }
}

impl Trackers for MetaTrackers {
    fn tracker(&self) -> BoxFuture<'_, Result<Arc<dyn SubjectTracker>, String>> {
        Box::pin(async move {
            let ready = self.credentials.ready().await;
            match (ready.meta, ready.problem) {
                (Some(meta), _) => Ok(Arc::new(meta) as Arc<dyn SubjectTracker>),
                (None, Some(problem)) => Err(problem),
                (None, None) => Err(NEEDS_KEY.to_owned()),
            }
        })
    }

    fn credit_left(&self) -> BoxFuture<'_, Option<f64>> {
        Box::pin(async move { self.credit().await.ok().flatten() })
    }

    fn matcher(&self) -> BoxFuture<'_, Option<Arc<dyn VoiceMatcher>>> {
        Box::pin(async move {
            let meta = self.credentials.ready().await.meta?;
            Some(Arc::new(meta) as Arc<dyn VoiceMatcher>)
        })
    }

    fn credit(&self) -> BoxFuture<'_, Result<Option<f64>, String>> {
        Box::pin(async move {
            let status = self.credentials.status().await;
            if status.mode != AiMode::Relay {
                return Ok(None);
            }
            if status.active == Some(false) {
                return Err(status
                    .notice
                    .clone()
                    .unwrap_or_else(|| CODE_INACTIVE.to_owned()));
            }
            Ok((status.budget_usd.zip(status.spent_usd))
                .map(|(budget, spent)| (budget - spent).max(0.0)))
        })
    }
}

/// The voices of each file's analysis: its transcript's turns that have a label.
impl Voices for AnalysisState {
    fn turns(&self, asset_id: &str) -> Vec<VoiceTurn> {
        let Some(found) = self.analysis_for(asset_id) else {
            return Vec::new();
        };
        (found.turns.iter())
            .filter_map(|t| {
                Some(VoiceTurn {
                    start_us: t.start_us,
                    end_us: t.end_us,
                    voice: Voice {
                        part: t.chunk,
                        label: t.speaker.clone()?,
                    },
                })
            })
            .collect()
    }
}

/// The app's speaker following.
pub struct Speaker {
    trackers: Arc<dyn Trackers>,
    copier: Arc<dyn SubjectCopy>,
    /// The analysis cache's root: tracked stretches are kept with each file's analysis.
    cache_root: PathBuf,
    /// Where the small copies are made, each removed once sent.
    work_root: PathBuf,
    progress: Arc<dyn Fn(SpeakerProgress) + Send + Sync>,
    spent: Mutex<f64>,
    /// One follow at a time: a second one then finds what the first tracked in the cache, and pays nothing for it.
    running: tokio::sync::Mutex<()>,
    /// The files' voices; none follows each shot's main subject.
    voices: Option<Arc<dyn Voices>>,
}

/// Whether a run may pay (a follow), or only reads what was tracked and matched before (who to keep).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Pay,
    CacheOnly,
}

/// A clip that can follow, and where.
struct Target {
    id: Id,
    footage: Footage,
    formats: Vec<FormatKind>,
}

/// One stretch of a file to send (or read from its cache).
struct Stretch {
    path: String,
    range: RangeUs,
}

impl Speaker {
    pub fn new(
        trackers: Arc<dyn Trackers>,
        copier: Arc<dyn SubjectCopy>,
        cache_root: PathBuf,
        work_root: PathBuf,
        progress: Arc<dyn Fn(SpeakerProgress) + Send + Sync>,
    ) -> Self {
        Self {
            trackers,
            copier,
            cache_root,
            work_root,
            progress,
            spent: Mutex::new(0.0),
            running: tokio::sync::Mutex::new(()),
            voices: None,
        }
    }

    /// Keeps whoever speaks in frame, from `voices`' transcripts (see the module notes).
    #[must_use]
    pub fn with_voices(mut self, voices: Arc<dyn Voices>) -> Self {
        self.voices = Some(voices);
        self
    }

    /// The app's: SAM through `credentials`, the engine's small copy, the analysis cache under `cache_root`.
    pub fn for_app(
        credentials: Arc<Credentials>,
        cache_root: PathBuf,
        work_root: PathBuf,
        progress: Arc<dyn Fn(SpeakerProgress) + Send + Sync>,
    ) -> Self {
        Self::new(
            Arc::new(MetaTrackers::new(credentials)),
            Arc::new(subject::EngineCopy),
            cache_root,
            work_root,
            progress,
        )
    }

    /// What following cost this session, in US dollars.
    pub fn spent_usd(&self) -> f64 {
        *self.spent.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Which clips can follow in `formats`, and what that costs (see the module notes). Calls nothing but the relay
    /// for an access code's credit.
    pub async fn estimate(
        &self,
        p: &Project,
        clip_ids: &[Id],
        formats: &[FormatKind],
        phrase: &str,
    ) -> SpeakerEstimate {
        let targets = targets(p, clip_ids, formats);
        let mut estimate = SpeakerEstimate {
            clips: targets.iter().map(|t| t.id.clone()).collect(),
            ..SpeakerEstimate::default()
        };
        let tracker = match self.trackers.tracker().await {
            Ok(tracker) => tracker,
            Err(why) => {
                estimate.refusal = Some(why);
                return estimate;
            }
        };
        let to_send = self.uncached(&*tracker, &targets, phrase).await;
        let (matcher, _) = self.matcher(phrase).await;
        let voices_usd = match &matcher {
            Some(m) => {
                let cached = to_send.is_empty();
                self.voices_usd(m, &*tracker, p, &targets, phrase, cached)
                    .await
            }
            None => 0.0,
        };
        estimate.usd = price(&*tracker, &to_send) + voices_usd;
        estimate.cached = to_send.is_empty() && voices_usd <= 0.0 && !targets.is_empty();
        estimate.refusal = self.over_credit(estimate.usd).await;
        estimate
    }

    /// Tracks the speaker for `clip_ids` in `formats` (see the module notes): their paths, or why it couldn't run, in
    /// one sentence. Refused before anything is sent when it costs more than an access code's credit left.
    pub async fn follow(
        &self,
        p: &Project,
        clip_ids: &[Id],
        formats: &[FormatKind],
        phrase: &str,
    ) -> Result<Followed, String> {
        self.follow_keeping(p, clip_ids, formats, phrase, &BTreeMap::new())
            .await
    }

    /// [`Self::follow`], each clip of `keeps` keeping who it names (the rest whoever speaks).
    pub async fn follow_keeping(
        &self,
        p: &Project,
        clip_ids: &[Id],
        formats: &[FormatKind],
        phrase: &str,
        keeps: &BTreeMap<Id, Keep>,
    ) -> Result<Followed, String> {
        self.run(p, clip_ids, formats, phrase, keeps, Mode::Pay)
            .await
    }

    /// Who `keeps` names kept in frame for its clips in `formats`, made only from what earlier follows tracked and
    /// matched: nothing is sent, so it never pays (voices the cache doesn't settle are left unmatched). Refused
    /// (`NOT_TRACKED`) when any of the footage isn't tracked in the cache any more.
    pub async fn keep(
        &self,
        p: &Project,
        clip_ids: &[Id],
        formats: &[FormatKind],
        keeps: &BTreeMap<Id, Keep>,
    ) -> Result<Followed, String> {
        self.run(p, clip_ids, formats, PERSON, keeps, Mode::CacheOnly)
            .await
    }

    async fn run(
        &self,
        p: &Project,
        clip_ids: &[Id],
        formats: &[FormatKind],
        phrase: &str,
        keeps: &BTreeMap<Id, Keep>,
        mode: Mode,
    ) -> Result<Followed, String> {
        let paying = mode == Mode::Pay;
        let targets = targets(p, clip_ids, formats);
        let tracker = self.trackers.tracker().await?;
        let _one_at_a_time = self.running.lock().await;
        let stretches = stretches(&targets, tracker.max_frames());
        let to_send = self.uncached(&*tracker, &targets, phrase).await;
        if !paying && !to_send.is_empty() {
            return Err(NOT_TRACKED.to_owned());
        }
        let (matcher, short_credit) = if paying {
            self.matcher(phrase).await
        } else {
            (self.cache_matcher(phrase).await, false)
        };
        if paying {
            let voices_usd = match &matcher {
                Some(m) => {
                    let cached = to_send.is_empty();
                    self.voices_usd(m, &*tracker, p, &targets, phrase, cached)
                        .await
                }
                None => 0.0,
            };
            if let Some(refusal) = self
                .over_credit(price(&*tracker, &to_send) + voices_usd)
                .await
            {
                return Err(refusal);
            }
        }
        let ids: Vec<String> = targets.iter().map(|t| t.id.clone()).collect();
        // Who to keep changes with no progress to show: nothing is sent.
        let tell = |pct: u8| {
            if paying {
                (self.progress)(SpeakerProgress {
                    clip_ids: ids.clone(),
                    pct,
                })
            }
        };
        tell(0);
        // Each file's tracked stretches: a clip reads only its own file's.
        let mut tracks: BTreeMap<String, Vec<SubjectTrack>> = BTreeMap::new();
        let mut paid = 0.0;
        for (i, stretch) in stretches.iter().enumerate() {
            let src = PathBuf::from(&stretch.path);
            let dir = cache_dir(&self.cache_root, &src).await;
            let ask = Ask {
                src: &src,
                cache: dir.as_ref(),
                work: &self.work_root,
                range: stretch.range,
                phrase,
                fps: FPS,
                cancel: Arc::new(AtomicBool::new(false)),
            };
            let tracked = subject::track(&*tracker, self.copier.clone(), &ask).await;
            match tracked {
                Ok(tracked) => {
                    paid += tracked.paid_usd;
                    self.count(tracked.paid_usd);
                    (tracks.entry(stretch.path.clone()).or_default()).extend(tracked.tracks);
                }
                Err(e) => {
                    tell(100);
                    return Err(e.user_message());
                }
            }
            tell((((i + 1) * 100) / stretches.len().max(1)).min(99) as u8);
        }
        // Each file's voices, matched to its people where they don't all fit.
        let mut heard: BTreeMap<String, (Vec<VoiceTurn>, BTreeMap<Voice, Heard>)> = BTreeMap::new();
        let mut note: Option<String> = None;
        if matcher.is_some() || short_credit {
            for (file, asset_id) in files(&targets) {
                let turns = self.file_turns(p, &asset_id);
                let own = tracks.get(&file).map_or(&[][..], Vec::as_slice);
                let windows = windows_of(p, &targets, &file, own, &turns);
                let mut matched = BTreeMap::new();
                if !windows.is_empty() && note.is_none() {
                    let src = PathBuf::from(&file);
                    let dir = cache_dir(&self.cache_root, &src).await;
                    match (&matcher, paying) {
                        (None, _) => {}
                        (Some(m), false) => {
                            let model = m.model().to_owned();
                            matched = tokio::task::spawn_blocking(move || {
                                voices::cached_voices(dir.as_ref(), &model, &windows)
                            })
                            .await
                            .unwrap_or_default();
                        }
                        (Some(m), true) => {
                            let asking = Asking {
                                src: &src,
                                cache: dir.as_ref(),
                                work: &self.work_root,
                                cancel: Arc::new(AtomicBool::new(false)),
                            };
                            let found =
                                voices::match_voices(&**m, self.copier.clone(), &asking, &windows)
                                    .await;
                            paid += found.paid_usd;
                            self.count(found.paid_usd);
                            match &found.stopped {
                                // No half state: a voice matched before the stop would hold the crop on its person.
                                Some(why) => {
                                    tracing::warn!(%why, "matching voices stopped; the crop follows each shot's main person");
                                    note = Some(UNMATCHED.to_owned());
                                }
                                None => matched = found.voices,
                            }
                        }
                    }
                }
                heard.insert(file, (turns, matched));
            }
            if note.is_some() {
                for (_, matched) in heard.values_mut() {
                    matched.clear();
                }
            }
        }
        let mut followed = Followed {
            paid_usd: paid,
            note,
            ..Followed::default()
        };
        let none = (Vec::new(), BTreeMap::new());
        for target in &targets {
            let Some(clip) = p.timeline.main.iter().find(|c| c.id == target.id) else {
                continue;
            };
            let own = tracks
                .get(&target.footage.path)
                .map_or(&[][..], Vec::as_slice);
            let (turns, voices) = heard.get(&target.footage.path).unwrap_or(&none);
            let keep = keeps.get(&target.id).cloned().unwrap_or_default();
            let speech = Speech {
                turns,
                voices,
                keep: &keep,
            };
            let paths: BTreeMap<FormatKind, FocusPath> = (target.formats.iter())
                .filter_map(|f| {
                    let path = speaker_path(p, clip, *f, &target.footage, own, &speech)?;
                    Some((*f, path))
                })
                .collect();
            if paths.is_empty() {
                followed.not_found.push(target.id.clone());
            } else {
                // People to choose between only when following a person: two of a named thing aren't.
                let people = if phrase == PERSON {
                    clip_people(clip, &target.footage, own, turns, voices)
                } else {
                    Vec::new()
                };
                followed.kept.insert(target.id.clone(), (keep, people));
                followed.paths.push((target.id.clone(), paths));
            }
        }
        tell(100);
        Ok(followed)
    }

    /// The matcher, when following a person and there are voices to match (and false: nothing holds it back).
    async fn matcher(&self, phrase: &str) -> (Option<Arc<dyn VoiceMatcher>>, bool) {
        (self.cache_matcher(phrase).await, false)
    }

    /// The matcher whose cached answers count, when following a person and there are voices: never called by a keep.
    async fn cache_matcher(&self, phrase: &str) -> Option<Arc<dyn VoiceMatcher>> {
        if phrase != PERSON || self.voices.is_none() {
            return None;
        }
        self.trackers.matcher().await
    }

    /// `asset_id`'s voice turns on its file's clock.
    fn file_turns(&self, p: &Project, asset_id: &str) -> Vec<VoiceTurn> {
        let Some(voices) = &self.voices else {
            return Vec::new();
        };
        let lead = p.asset(asset_id).map_or(0, |a| a.lead_us());
        (voices.turns(asset_id).into_iter())
            .map(|t| VoiceTurn {
                start_us: t.start_us + lead,
                end_us: t.end_us + lead,
                ..t
            })
            .collect()
    }

    /// What matching `targets`' voices may cost now: the windows the cache doesn't settle once the footage is tracked
    /// (`cached`), else the most it can be.
    async fn voices_usd(
        &self,
        matcher: &Arc<dyn VoiceMatcher>,
        tracker: &dyn SubjectTracker,
        p: &Project,
        targets: &[Target],
        phrase: &str,
        cached: bool,
    ) -> f64 {
        let mut usd = 0.0;
        for (file, asset_id) in files(targets) {
            let turns = self.file_turns(p, &asset_id);
            if turns.is_empty() {
                continue;
            }
            let src = PathBuf::from(&file);
            let dir = cache_dir(&self.cache_root, &src).await;
            if !cached {
                // Once for the file, over the footage all its clips play.
                let plays: Vec<RangeUs> = (targets.iter())
                    .filter(|t| t.footage.path == file)
                    .map(|t| t.footage.range)
                    .collect();
                usd += voices::most_usd(&**matcher, &turns, &plays);
                continue;
            }
            let own: Vec<SubjectTrack> = match &dir {
                Some(dir) => {
                    let mut own = Vec::new();
                    for stretch in stretches(targets, tracker.max_frames()) {
                        if stretch.path != file {
                            continue;
                        }
                        let (dir, model, phrase) =
                            (dir.clone(), tracker.model().to_owned(), phrase.to_owned());
                        let cover = tokio::task::spawn_blocking(move || {
                            subject::cover(&dir, &model, &phrase, FPS, stretch.range).tracks
                        })
                        .await
                        .unwrap_or_default();
                        own.extend(cover);
                    }
                    own
                }
                None => Vec::new(),
            };
            let windows = windows_of(p, targets, &file, &own, &turns);
            usd += voices::uncached_usd(&**matcher, dir.as_ref(), &windows);
        }
        usd
    }

    /// The parts of `targets`' stretches the cache doesn't hold yet: what a follow sends.
    async fn uncached(
        &self,
        tracker: &dyn SubjectTracker,
        targets: &[Target],
        phrase: &str,
    ) -> Vec<RangeUs> {
        let mut out = Vec::new();
        for stretch in stretches(targets, tracker.max_frames()) {
            let dir = cache_dir(&self.cache_root, &PathBuf::from(&stretch.path)).await;
            let (range, model, phrase) =
                (stretch.range, tracker.model().to_owned(), phrase.to_owned());
            let missing = match dir {
                Some(dir) => tokio::task::spawn_blocking(move || {
                    subject::cover(&dir, &model, &phrase, FPS, range).missing
                })
                .await
                .unwrap_or_else(|_| vec![range]),
                None => vec![range],
            };
            out.extend(missing);
        }
        out
    }

    /// Why `usd` is refused: more than an access code's credit left. Free (all tracked before) asks nothing.
    async fn over_credit(&self, usd: f64) -> Option<String> {
        if usd <= 0.0 {
            return None;
        }
        let left = match self.trackers.credit().await {
            Ok(left) => left?,
            Err(turned_off) => return Some(turned_off),
        };
        (usd > left).then(|| {
            format!(
                "Your access code has ${left:.2} left, and following the speaker needs {}. Add your own Meta key in \
                 Settings.",
                about(usd)
            )
        })
    }

    fn count(&self, usd: f64) {
        *self.spent.lock().unwrap_or_else(PoisonError::into_inner) += usd.max(0.0);
    }
}

/// What sending `parts` costs at `tracker`'s price: each its own request.
fn price(tracker: &dyn SubjectTracker, parts: &[RangeUs]) -> f64 {
    (parts.iter())
        .map(|r| tracker.estimate_usd(subject::frames_for(r.end - r.start, FPS)))
        .sum()
}

/// "about $0.02", or "less than $0.01".
pub fn about(usd: f64) -> String {
    if usd < 0.01 {
        "less than $0.01".into()
    } else {
        format!("about ${usd:.2}")
    }
}

/// The clips of `clip_ids` that can follow in some of `formats`, in the order asked.
fn targets(p: &Project, clip_ids: &[Id], formats: &[FormatKind]) -> Vec<Target> {
    (clip_ids.iter())
        .filter_map(|id| {
            let clip = p.timeline.main.iter().find(|c| &c.id == id)?;
            let footage = footage(p, clip)?;
            // Only where it is on Fill: a format the person set to Fit stays as it is.
            let formats = followable_formats(p, clip, formats);
            (!formats.is_empty()).then(|| Target {
                id: id.clone(),
                footage,
                formats,
            })
        })
        .collect()
}

/// The stretches to track for `targets`: per file, its clips' stretches joined and cut (`subject::requests`).
fn stretches(targets: &[Target], max_frames: u64) -> Vec<Stretch> {
    let mut files: Vec<&str> = targets.iter().map(|t| t.footage.path.as_str()).collect();
    files.sort_unstable();
    files.dedup();
    let mut out = Vec::new();
    for file in files {
        let on: Vec<&Target> = targets.iter().filter(|t| t.footage.path == file).collect();
        let ranges: Vec<RangeUs> = on.iter().map(|t| t.footage.range).collect();
        for range in subject::requests(&ranges, FPS, max_frames) {
            out.push(Stretch {
                path: file.to_owned(),
                range,
            });
        }
    }
    out
}

/// Each file `targets` play, once, with its asset's id.
fn files(targets: &[Target]) -> Vec<(String, Id)> {
    let mut out: Vec<(String, Id)> = (targets.iter())
        .map(|t| (t.footage.path.clone(), t.footage.asset_id.clone()))
        .collect();
    out.sort();
    out.dedup_by(|a, b| a.0 == b.0);
    out
}

/// The windows of `file`'s clips among `targets` (`voice_windows`), each once.
fn windows_of(
    p: &Project,
    targets: &[Target],
    file: &str,
    tracks: &[SubjectTrack],
    turns: &[VoiceTurn],
) -> Vec<Window> {
    let mut out: Vec<Window> = Vec::new();
    for target in targets.iter().filter(|t| t.footage.path == file) {
        let Some(clip) = p.timeline.main.iter().find(|c| c.id == target.id) else {
            continue;
        };
        for w in voice_windows(p, clip, &target.formats, &target.footage, tracks, turns) {
            if !out.iter().any(|o| o.voice == w.voice && o.range == w.range) {
                out.push(w);
            }
        }
    }
    // A voice's windows together, longest first, as `voices::windows` gives them.
    out.sort_by(|a, b| {
        a.voice.cmp(&b.voice).then(
            (b.range.end - b.range.start)
                .cmp(&(a.range.end - a.range.start))
                .then(a.range.start.cmp(&b.range.start)),
        )
    });
    out
}

/// The analysis cache folder of the file at `path`; None when it can't be read.
async fn cache_dir(root: &std::path::Path, path: &std::path::Path) -> Option<Dir> {
    let (root, path) = (root.to_path_buf(), path.to_path_buf());
    tokio::task::spawn_blocking(move || {
        let key = content_key(&path).ok()?;
        Dir::new(&root, &key).ok()
    })
    .await
    .ok()
    .flatten()
}
