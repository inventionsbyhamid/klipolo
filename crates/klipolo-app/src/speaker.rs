//! Follow the speaker: a clip's vertical or square crop kept on the person talking, from SAM's boxes on a small, low
//! frame rate copy of only the footage the clips play (`klipolo_analysis::subject`).
//!
//! - **Price first** (`Speaker::estimate`): each stretch to send, at the tracker's price; stretches tracked before are
//!   in the analysis cache and free. With an access code the run is refused when it costs more than the credit left.
//!   Nothing is sent to price it.
//! - **The run** (`Speaker::follow`): each stretch copied small, sent, cached, its progress told (`SpeakerProgress`),
//!   its cost counted into the session's spend (`spent_usd`); then each clip's path in each format whose Fill crops
//!   it. A used-up credit stops it with the relay's sentence.
//! - **Landing** is the caller's: the paths are in the clips' source time, so `Followed::ops` builds the ops on the
//!   project as it is once the run is done (a hand edit meanwhile is kept), as one undo step.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, PoisonError};

use klipolo_analysis::cache::{Dir, content_key};
use klipolo_analysis::providers::{BoxFuture, SubjectTracker};
use klipolo_analysis::subject::clips::{
    Footage, clip_path, follow_op, followable_formats, footage,
};
use klipolo_analysis::subject::{self, Ask, FPS, SubjectCopy, SubjectTrack};
use klipolo_model::fill_focus::FocusPath;
use klipolo_model::ids::Id;
use klipolo_model::ops::Op;
use klipolo_model::project::{FormatKind, Project};
use klipolo_model::time::RangeUs;
use serde::Serialize;
use ts_rs::TS;

use crate::analysis::NEEDS_KEY;
use crate::credentials::{AiMode, CODE_INACTIVE, Credentials};

/// The event a follow's progress goes out on.
pub const PROGRESS_EVENT: &str = "speaker://progress";
/// Said when the footage has nobody in it to follow.
pub const NO_ONE: &str = "Klipolo couldn't find a person to follow in this clip.";

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
    /// The clips with nobody to follow.
    pub not_found: Vec<Id>,
    /// What the run cost (0 for footage tracked before).
    pub paid_usd: f64,
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
                (!paths.is_empty()).then(|| follow_op(p, clip, &paths))
            })
            .collect()
    }
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
        }
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
        estimate.usd = price(&*tracker, &to_send);
        estimate.cached = to_send.is_empty() && !targets.is_empty();
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
        let targets = targets(p, clip_ids, formats);
        let tracker = self.trackers.tracker().await?;
        let _one_at_a_time = self.running.lock().await;
        let stretches = stretches(&targets, tracker.max_frames());
        let to_send = self.uncached(&*tracker, &targets, phrase).await;
        if let Some(refusal) = self.over_credit(price(&*tracker, &to_send)).await {
            return Err(refusal);
        }
        let ids: Vec<String> = targets.iter().map(|t| t.id.clone()).collect();
        let tell = |pct: u8| {
            (self.progress)(SpeakerProgress {
                clip_ids: ids.clone(),
                pct,
            })
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
        let mut followed = Followed {
            paid_usd: paid,
            ..Followed::default()
        };
        for target in &targets {
            let Some(clip) = p.timeline.main.iter().find(|c| c.id == target.id) else {
                continue;
            };
            let own = tracks
                .get(&target.footage.path)
                .map_or(&[][..], Vec::as_slice);
            let paths: BTreeMap<FormatKind, FocusPath> = (target.formats.iter())
                .filter_map(|f| Some((*f, clip_path(p, clip, *f, &target.footage, own)?)))
                .collect();
            if paths.is_empty() {
                followed.not_found.push(target.id.clone());
            } else {
                followed.paths.push((target.id.clone(), paths));
            }
        }
        tell(100);
        Ok(followed)
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
