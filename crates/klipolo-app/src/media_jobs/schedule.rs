//! The editing-copy schedule, pure: no processes, files or clocks. Which copy runs next, what holds it, and
//! the code that says why each copy is paused or failed (`Job::reason`, the app's `copies.*` codes: Rust never sends
//! English for the page to show). `MediaJobs` (`mod.rs`) owns one and does what `next` says.
//!
//! One copy runs at a time, and only while nothing else wants the computer: it pauses while the person plays the video
//! (`playing`), exports (`exporting`) or records a take (`recording`), while they asked it to (`user_paused`, or automatic
//! copies are off), and it never starts while a thumbnails-only scan runs (`scanning`): one background job at a time. A copy that
//! paused itself because the disk is almost full, the computer runs hot or the encoder is busy holds the whole queue:
//! the next copy would meet the same. So does a copy whose worker stopped without a word (crashed or killed): it is
//! started again by itself after `RESTART_WAITS`, from its finished segments, and fails after that many restarts.

use std::sync::Arc;
use std::time::Duration;

use klipolo_engine::api::{CodedError, MediaJobInfo, MediaJobList, MediaJobState, MediaJobsHold};
use klipolo_engine::media::proxy::decide::ProxyReason;
use klipolo_engine::media::proxy::facts::SourceFacts;
use klipolo_engine::media::proxy::job::Refused;
use klipolo_engine::media::proxy::run::FailCode;
use klipolo_engine::plan::{MediaCopy, MediaMap};

use crate::english::code;

/// A busy encoder is tried again after 30 s, then 1, 2, 4 and 8 minutes; after that many pauses in a row the copy
/// stops with a coded failure instead of trying forever (a VideoToolbox that can never encode it).
pub const ENCODER_TRIES: u32 = 5;
const ENCODER_FIRST_WAIT: Duration = Duration::from_secs(30);
/// A copy paused because the computer runs hot is tried once more after this long (one delayed task, no polling).
pub const THERMAL_WAIT: Duration = Duration::from_secs(5 * 60);
/// A worker that stopped without a word (crashed, killed, or exited without its own `Failed` line) is started again
/// after these waits, one per restart; stopping once more after the last, the copy fails and waits
/// for Try again. A failure the worker reports itself (a missing or unreadable source, a full disk) never restarts.
pub const RESTART_WAITS: [Duration; 2] = [Duration::from_secs(5), Duration::from_secs(30)];

/// Why a copy is paused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pause {
    /// The video is playing.
    Playing,
    /// An export runs.
    Exporting,
    /// A take is recording.
    Recording,
    /// A hold: the person's pause, an action's, or automatic copies off (the list's `held` says which).
    User,
    /// The disk is almost full: it waits for Resume.
    Disk,
    /// The computer runs hot: tried again after `THERMAL_WAIT`.
    Thermal,
    /// The video encoder is busy: tried again with a backoff (`ENCODER_TRIES`).
    Encoder,
    /// The worker stopped without a word (crashed or killed): started again after `RESTART_WAITS`.
    WorkerStopped,
}

/// Why a copy can't be made, each with a code of its own (`Failure::code`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The source isn't there now (an unplugged drive, a volume not mounted, a moved file): decided again once it is
    /// back (`look_again`), with no retry loop meanwhile.
    Missing,
    /// The source can't be read as video (damaged, not a video).
    Unreadable,
    /// Neither the file nor its packets say how long it is: a copy would show one picture.
    UnknownLength,
    /// The file states no usable frame rate.
    UnusualRate,
    /// The cache folder can't be written: a read-only volume, no permission, a failing drive.
    CacheUnwritable,
    /// The video encoder stayed busy past `ENCODER_TRIES` pauses in a row.
    EncoderBusy,
    /// The worker process couldn't be started.
    Worker,
    /// The worker kept stopping without a word: it was started again `RESTART_WAITS.len()` times and stopped again.
    WorkerKeepsStopping,
    /// Anything else: a decode, GPU or check failure, or a worker that stopped without a word.
    Other,
}

/// A missing source's code, which the preview's notice shares (`klipolo_engine::api::MEDIA_MISSING`), checked against
/// `APP` as `code!` checks the others.
const MISSING: &str = klipolo_engine::api::MEDIA_MISSING;
const _: () = assert!(klipolo_model::coded::has(crate::english::APP, MISSING));

impl Failure {
    pub fn code(self) -> &'static str {
        match self {
            Self::Missing => MISSING,
            Self::Unreadable => code!("copies.failed.unreadable"),
            Self::UnknownLength => code!("copies.failed.unknownLength"),
            Self::UnusualRate => code!("copies.failed.unusualRate"),
            Self::CacheUnwritable => code!("copies.failed.cacheUnwritable"),
            Self::EncoderBusy => code!("copies.failed.encoderBusy"),
            Self::Worker => code!("copies.failed.worker"),
            Self::WorkerKeepsStopping => code!("copies.failed.workerKeepsStopping"),
            Self::Other => code!("copies.failed.other"),
        }
    }

    /// The code with the file's `name` where its message has it.
    pub fn coded(self, name: &str) -> CodedError {
        let coded = CodedError::new(self.code());
        match self {
            // Never the file's fault: neither names it.
            Self::CacheUnwritable | Self::WorkerKeepsStopping => coded,
            _ => coded.with("name", name),
        }
    }

    /// A job refused for good (`Opened::Refused`, retried only when the person asks). A full disk pauses instead, and
    /// refusals retried by themselves leave no row: neither reaches this.
    pub fn refused(why: &Refused) -> Self {
        match why {
            Refused::Unreadable => Self::Unreadable,
            Refused::SourceUnavailable => Self::Missing,
            Refused::UnknownLength => Self::UnknownLength,
            Refused::UnusualRate => Self::UnusualRate,
            Refused::CacheUnwritable(_) => Self::CacheUnwritable,
            Refused::SourceTimedOut | Refused::Changing | Refused::DiskFull => Self::Other,
        }
    }

    /// A worker's failure (a changed source is made again, never shown).
    pub fn of(code: FailCode) -> Self {
        match code {
            FailCode::Missing => Self::Missing,
            FailCode::CacheUnwritable => Self::CacheUnwritable,
            FailCode::Changed | FailCode::Other => Self::Other,
        }
    }
}

/// Where one copy stands.
#[derive(Clone, Debug, PartialEq)]
pub enum JobState {
    Waiting,
    Working,
    Paused(Pause),
    /// The copy, ready to read.
    Done(Arc<str>),
    /// It can't be made: why (its code), and what went wrong in the log's words. A missing source is decided again
    /// at the next listing of the project's files (no retry loop).
    Failed {
        failure: Failure,
        detail: String,
    },
    /// Stopped by the person: its unfinished copy was deleted; Retry starts over.
    Stopped,
}

pub use JobState::*;

/// One source's editing copy.
#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    /// The source file, as the project names it.
    pub path: Arc<str>,
    /// Its file name.
    pub name: String,
    pub state: JobState,
    /// 0 to 1, from frames already copied.
    pub progress: f64,
    /// The copy's frame rate: the source's own, exactly (`decide::proxy_rate`), so the preview reads copy frame n where
    /// it would read source frame n.
    pub fps: f64,
    /// The copy's key (`key::stem` of the source as it was decided): which copy and job folder are this one's.
    pub stem: String,
    /// What the source is (None until probed): a job starts from them without probing again.
    pub facts: Option<Arc<SourceFacts>>,
    /// Why it gets a copy (`needs_proxy`), once decided.
    pub why: Option<ProxyReason>,
    /// Encoder-busy pauses in a row (the backoff's step).
    busy: u32,
    /// Times its worker was started again by itself after stopping without a word (at most `RESTART_WAITS.len()`;
    /// back to 0 when the person asks again, the copy is done or its file changed).
    pub restarts: u32,
}

impl Job {
    pub fn new(path: &str, state: JobState) -> Self {
        Self {
            path: path.into(),
            name: file_name(path).to_owned(),
            state,
            progress: 0.0,
            fps: 0.0,
            stem: String::new(),
            facts: None,
            why: None,
            busy: 0,
            restarts: 0,
        }
    }

    /// Why it is paused or failed, with nothing holding copies (the tests' view; the list says `reason_under`).
    #[cfg(test)]
    pub fn reason(&self) -> Option<CodedError> {
        self.reason_under(None)
    }

    /// Why it is paused or failed, as a code, while `hold` holds every copy (None while it waits, works, is done or
    /// was stopped: the state says it). Who holds a copy paused by a hold is in the list's `held`. A stopped worker's
    /// copy says the hold's words while one holds, not "Trying again": it won't start before the hold ends.
    fn reason_under(&self, hold: Option<Pause>) -> Option<CodedError> {
        let code = match (&self.state, hold) {
            (Paused(Pause::WorkerStopped), Some(why)) => pause_code(why),
            (Paused(why), _) => pause_code(*why),
            (Failed { failure, .. }, _) => return Some(failure.coded(&self.name)),
            (Waiting | Working | Done(_) | Stopped, _) => return None,
        };
        Some(CodedError::new(code))
    }

    /// What the list says of it while `hold` holds every copy (`ScheduleState::hold`).
    pub fn info(&self, hold: Option<Pause>) -> MediaJobInfo {
        MediaJobInfo {
            path: self.path.to_string(),
            name: self.name.clone(),
            state: match self.state {
                Waiting => MediaJobState::Waiting,
                Working => MediaJobState::Working,
                Paused(_) => MediaJobState::Paused,
                Done(_) => MediaJobState::Done,
                Failed { .. } => MediaJobState::Failed,
                Stopped => MediaJobState::Stopped,
            },
            progress: self.progress,
            why: self.why,
            reason: self.reason_under(hold),
        }
    }

    /// Whether a copy in this state starts by itself once nothing holds copies.
    fn ready(&self) -> bool {
        matches!(
            self.state,
            Waiting | Paused(Pause::Playing | Pause::Exporting | Pause::Recording | Pause::User)
        )
    }
}

/// A paused copy's code.
fn pause_code(why: Pause) -> &'static str {
    match why {
        Pause::Playing => code!("copies.paused.playing"),
        Pause::Exporting => code!("copies.paused.exporting"),
        Pause::Recording => code!("copies.paused.recording"),
        Pause::User => code!("copies.paused.held"),
        Pause::Disk => code!("copies.paused.diskFull"),
        Pause::Thermal => code!("copies.paused.hot"),
        Pause::Encoder => code!("copies.paused.encoderBusy"),
        Pause::WorkerStopped => code!("copies.paused.workerStopped"),
    }
}

/// A file name, for the codes' `name` and the log.
pub fn file_name(path: &str) -> &str {
    std::path::Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path)
}

/// What to do now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Start(usize),
    Pause(usize, Pause),
    Nothing,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScheduleState {
    /// The open project's copies, in the order they are made (the timeline's first).
    pub jobs: Vec<Job>,
    /// Copies are made by themselves (`media_jobs_set_auto`).
    pub auto: bool,
    pub playing: bool,
    pub exporting: bool,
    /// A take is recording: copies wait for it.
    pub recording: bool,
    /// The person paused copies (`media_jobs_pause`, the pill).
    pub user_paused: bool,
    /// An action paused them (`media_jobs_pause` with `by: "action"`): a hold of its own, so
    /// neither undoes the other's.
    pub action_paused: bool,
    /// A thumbnails-only scan is running (`ThumbStore::scanning`).
    pub scanning: bool,
    /// The job being made.
    pub running: Option<usize>,
    /// The waits before a stopped worker is started again (`RESTART_WAITS`; shorter in tests).
    pub restart_waits: [Duration; 2],
}

impl ScheduleState {
    pub fn with(jobs: Vec<Job>) -> Self {
        Self {
            jobs,
            auto: true,
            playing: false,
            exporting: false,
            recording: false,
            user_paused: false,
            action_paused: false,
            scanning: false,
            running: None,
            restart_waits: RESTART_WAITS,
        }
    }

    /// What holds every copy now: the person's pause first (it lasts until they resume), then an export, then
    /// playback.
    pub fn hold(&self) -> Option<Pause> {
        if self.user_paused || self.action_paused || !self.auto {
            Some(Pause::User)
        } else if self.exporting {
            Some(Pause::Exporting)
        } else if self.recording {
            Some(Pause::Recording)
        } else if self.playing {
            Some(Pause::Playing)
        } else {
            None
        }
    }

    pub fn next(&self) -> Action {
        if let Some(i) = self.running {
            return match self.hold() {
                Some(why) => Action::Pause(i, why),
                None => Action::Nothing,
            };
        }
        if self.hold().is_some() || self.scanning {
            return Action::Nothing;
        }
        // A full disk, a hot computer or a busy encoder would stop the next copy too; a stopped worker is started again
        // first (in a few seconds), before the copies after it.
        if self.jobs.iter().any(|j| {
            matches!(
                j.state,
                Paused(Pause::Disk | Pause::Thermal | Pause::Encoder | Pause::WorkerStopped)
            )
        }) {
            return Action::Nothing;
        }
        self.jobs
            .iter()
            .position(Job::ready)
            .map_or(Action::Nothing, Action::Start)
    }

    /// Keeps the jobs `keep` says; `running` follows the running one (None if it went).
    pub fn retain(&mut self, keep: impl Fn(&Job) -> bool) {
        let running = self.running.map(|i| self.jobs[i].path.clone());
        self.jobs.retain(|j| keep(j));
        self.running = running.and_then(|p| self.index_of(&p));
    }

    pub fn index_of(&self, path: &str) -> Option<usize> {
        self.jobs.iter().position(|j| &*j.path == path)
    }

    pub fn started(&mut self, i: usize) {
        self.jobs[i].state = Working;
        self.running = Some(i);
    }

    /// `done` of `total` frames are copied. A copy that moves on after a busy encoder starts its backoff over.
    pub fn progress(&mut self, i: usize, done: u64, total: u64) {
        let job = &mut self.jobs[i];
        let progress = if total == 0 {
            0.0
        } else {
            (done as f64 / total as f64).clamp(0.0, 1.0)
        };
        if progress > job.progress {
            job.busy = 0;
        }
        job.progress = progress;
    }

    /// The job stopped with every finished segment kept. A busy encoder past `ENCODER_TRIES` pauses in a row stops it
    /// for good (`Failure::EncoderBusy`).
    pub fn paused(&mut self, i: usize, why: Pause) {
        self.leave(i);
        let job = &mut self.jobs[i];
        job.state = Paused(why);
        if why == Pause::Encoder {
            job.busy += 1;
            if job.busy > ENCODER_TRIES {
                job.state = Failed {
                    failure: Failure::EncoderBusy,
                    detail: "encoderBusy: the video encoder stayed busy".into(),
                };
            }
        }
    }

    pub fn done(&mut self, i: usize, copy: Arc<str>) {
        self.leave(i);
        let job = &mut self.jobs[i];
        job.state = Done(copy);
        job.progress = 1.0;
        job.busy = 0;
        job.restarts = 0;
    }

    /// The worker stopped without a word of its own (crashed, killed, or exited without saying why), with every
    /// finished segment kept: it is started again by itself after the next of `restart_waits` (`retry_in`), resuming
    /// from those segments. Once it has been started again that many times, the copy fails (`WorkerKeepsStopping`)
    /// and waits for Try again. `detail` is for the log.
    pub fn crashed(&mut self, i: usize, detail: &str) {
        self.leave(i);
        let tries = self.restart_waits.len() as u32;
        let job = &mut self.jobs[i];
        if job.restarts < tries {
            job.restarts += 1;
            job.state = Paused(Pause::WorkerStopped);
        } else {
            job.state = Failed {
                failure: Failure::WorkerKeepsStopping,
                detail: detail.to_owned(),
            };
        }
    }

    /// The copy can't be made, for `failure`; `detail` is for the log. Not tried again by itself.
    pub fn failed(&mut self, i: usize, failure: Failure, detail: &str) {
        self.leave(i);
        self.jobs[i].state = Failed {
            failure,
            detail: detail.to_owned(),
        };
    }

    /// Its source changed (or its copy went): it is made again from the start, under a new key, from facts read
    /// again (the old ones describe the old file). With automatic copies off nothing would make it, so its row goes
    /// instead: no row waits while copies are off.
    pub fn requeue(&mut self, i: usize) {
        self.leave(i);
        if !self.auto {
            let path = self.jobs[i].path.clone();
            self.retain(|j| j.path != path);
            return;
        }
        let job = &mut self.jobs[i];
        job.state = Waiting;
        job.progress = 0.0;
        job.busy = 0;
        job.restarts = 0;
        job.facts = None;
        job.stem.clear();
    }

    pub fn stopped(&mut self, i: usize) {
        self.leave(i);
        let job = &mut self.jobs[i];
        job.state = Stopped;
        job.progress = 0.0;
    }

    /// The person asked again: a paused, failed or stopped copy waits its turn, with its restarts back to none.
    pub fn retry(&mut self, i: usize) {
        let job = &mut self.jobs[i];
        if matches!(job.state, Paused(_) | Failed { .. } | Stopped) {
            job.state = Waiting;
            job.busy = 0;
            job.restarts = 0;
        }
    }

    /// A timer came due: a copy still paused for heat, a busy encoder or a stopped worker waits its turn again (its
    /// backoff and restart counts kept). Anything that holds copies (a pause, playback, an export, copies off) still
    /// holds it (`next`).
    pub fn wake(&mut self, i: usize) {
        let job = &mut self.jobs[i];
        if matches!(
            job.state,
            Paused(Pause::Thermal | Pause::Encoder | Pause::WorkerStopped)
        ) {
            job.state = Waiting;
        }
    }

    /// The action's resume: only its own hold goes. A full disk, heat or a busy encoder
    /// still waits for the person, and the person's own pause stays.
    pub fn resume_action(&mut self) {
        self.action_paused = false;
    }

    /// The person's Resume: their pause goes, and copies paused for a full disk, heat or a busy encoder go again. The
    /// action's hold stays.
    pub fn resume_all(&mut self) {
        self.user_paused = false;
        for job in &mut self.jobs {
            if matches!(job.state, Paused(_)) {
                job.state = Waiting;
                job.busy = 0;
            }
        }
    }

    /// How long until job `i` is tried again by itself: a hot computer after `THERMAL_WAIT`, a busy encoder after its
    /// backoff (30 s, doubling), a stopped worker after its restart's wait (5 s, then 30 s). None for every other state.
    pub fn retry_in(&self, i: usize) -> Option<Duration> {
        let job = self.jobs.get(i)?;
        match job.state {
            Paused(Pause::Thermal) => Some(THERMAL_WAIT),
            Paused(Pause::Encoder) => {
                Some(ENCODER_FIRST_WAIT * 2u32.pow(job.busy.saturating_sub(1).min(8)))
            }
            Paused(Pause::WorkerStopped) => {
                let step = (job.restarts as usize).saturating_sub(1);
                self.restart_waits.get(step).copied()
            }
            _ => None,
        }
    }

    /// Source path → copy, from the finished jobs (the preview reads these; export never does).
    pub fn media_map(&self) -> MediaMap {
        self.jobs
            .iter()
            .filter_map(|j| match &j.state {
                Done(copy) => Some((j.path.clone(), MediaCopy::new(copy.clone(), j.fps))),
                _ => None,
            })
            .collect()
    }

    /// What holds every copy now (`MediaJobList::held`), in the list's order: the person's pause, the action's, and
    /// automatic copies turned off.
    pub fn held(&self) -> Vec<MediaJobsHold> {
        [
            (self.user_paused, MediaJobsHold::Person),
            (self.action_paused, MediaJobsHold::Action),
            (!self.auto, MediaJobsHold::AutoOff),
        ]
        .into_iter()
        .filter_map(|(on, hold)| on.then_some(hold))
        .collect()
    }

    /// The list the page is told (`media_jobs_list`, the `media-jobs` event).
    pub fn list(&self) -> MediaJobList {
        let hold = self.hold();
        MediaJobList {
            jobs: self.jobs.iter().map(|j| j.info(hold)).collect(),
            held: self.held(),
        }
    }

    fn leave(&mut self, i: usize) {
        if self.running == Some(i) {
            self.running = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    fn job(name: &str, state: JobState) -> Job {
        Job::new(&format!("/videos/{name}"), state)
    }

    #[test]
    fn playing_pauses_the_running_copy_and_stopping_resumes_it() {
        let mut s = ScheduleState::with(vec![job("a.mp4", Waiting)]);
        assert_eq!(s.next(), Action::Start(0));
        s.started(0);
        s.playing = true;
        assert_eq!(s.next(), Action::Pause(0, Pause::Playing));
        s.paused(0, Pause::Playing);
        assert_eq!(
            s.jobs[0].reason(),
            Some(CodedError::new("copies.paused.playing"))
        );
        s.playing = false;
        assert_eq!(s.next(), Action::Start(0));
    }

    /// A take holds copies while it records, with its own words, and they go on once it ends.
    #[test]
    fn a_take_pauses_copies_while_it_records() {
        let mut s = ScheduleState::with(vec![job("a.mp4", Waiting)]);
        s.recording = true;
        assert_eq!(s.hold(), Some(Pause::Recording));
        assert_eq!(s.next(), Action::Nothing);
        s.recording = false;
        assert_eq!(s.next(), Action::Start(0));
    }

    #[test]
    fn an_export_pauses_it_too_and_a_user_pause_holds_until_resume() {
        let mut s = ScheduleState::with(vec![job("a.mp4", Waiting)]);
        s.exporting = true;
        assert_eq!(s.next(), Action::Nothing);
        s.exporting = false;
        s.user_paused = true;
        assert_eq!(s.next(), Action::Nothing);
        s.user_paused = false;
        assert_eq!(s.next(), Action::Start(0));
    }

    #[test]
    fn a_missing_source_fails_its_job_with_the_file_name() {
        let mut s = ScheduleState::with(vec![job("intro.mp4", Waiting)]);
        s.started(0);
        s.failed(0, Failure::Missing, "missing");
        assert_eq!(
            s.jobs[0].reason(),
            Some(CodedError::new("copies.failed.missing").with("name", "intro.mp4"))
        );
        assert_eq!(s.next(), Action::Nothing, "no retry loop");
    }

    #[test]
    fn a_thumbnail_scan_and_a_copy_never_run_together() {
        let mut s = ScheduleState::with(vec![job("a.mp4", Waiting)]);
        s.scanning = true;
        assert_eq!(
            s.next(),
            Action::Nothing,
            "one background job at a time: the scan finishes first"
        );
        s.scanning = false;
        assert_eq!(s.next(), Action::Start(0));
    }

    #[test]
    fn finished_copies_are_mapped_and_auto_off_makes_none() {
        let mut s = ScheduleState::with(vec![job("a.mp4", Waiting)]);
        s.started(0);
        s.done(0, "/cache/a.mp4".into());
        assert_eq!(
            s.media_map().get("/videos/a.mp4").map(|p| p.as_ref()),
            Some("/cache/a.mp4")
        );
        let mut off = ScheduleState::with(vec![job("b.mp4", Waiting)]);
        off.auto = false;
        assert_eq!(off.next(), Action::Nothing);
    }

    /// The person's pause and an action's are holds of their own: an
    /// action's resume neither undoes the person's pause nor restarts a copy waiting for the person (a full disk).
    #[test]
    fn the_persons_pause_and_an_actions_pause_dont_undo_each_other() {
        let mut s = ScheduleState::with(vec![job("a.mp4", Waiting), job("b.mp4", Waiting)]);
        s.user_paused = true;
        s.action_paused = true;
        s.resume_action();
        assert_eq!(s.next(), Action::Nothing, "the person's pause stays");
        s.user_paused = false;
        s.action_paused = true;
        s.resume_all();
        assert_eq!(s.next(), Action::Nothing, "the action's pause stays");
        s.resume_action();
        s.started(0);
        s.paused(0, Pause::Disk);
        s.action_paused = true;
        s.resume_action();
        assert_eq!(
            s.next(),
            Action::Nothing,
            "a full disk waits for the person"
        );
        s.resume_all();
        assert_eq!(s.next(), Action::Start(0));
    }

    /// A busy encoder backs off (30 s, doubling) and, past `ENCODER_TRIES` pauses in a row, stops with a
    /// coded failure instead of trying forever. A copy that moves on starts its backoff over. Heat waits five minutes;
    /// a full disk waits for Resume. Each holds the queue meanwhile.
    #[test]
    fn a_busy_encoder_backs_off_then_stops_and_a_hot_computer_waits() {
        let mut s = ScheduleState::with(vec![job("a.mp4", Waiting), job("b.mp4", Waiting)]);
        let mut waits = Vec::new();
        for _ in 0..ENCODER_TRIES {
            assert_eq!(s.next(), Action::Start(0));
            s.started(0);
            s.paused(0, Pause::Encoder);
            assert_eq!(
                s.next(),
                Action::Nothing,
                "the next copy would meet the same"
            );
            waits.push(s.retry_in(0).unwrap().as_secs());
            s.wake(0);
        }
        assert_eq!(waits, [30, 60, 120, 240, 480]);
        s.started(0);
        s.progress(0, 10, 100);
        s.paused(0, Pause::Encoder);
        assert_eq!(
            s.retry_in(0).unwrap().as_secs(),
            30,
            "it moved on: the backoff starts over"
        );
        for _ in 0..ENCODER_TRIES {
            s.wake(0);
            s.started(0);
            s.paused(0, Pause::Encoder);
        }
        assert_eq!(
            s.jobs[0].reason(),
            Some(CodedError::new("copies.failed.encoderBusy").with("name", "a.mp4"))
        );
        assert!(
            matches!(&s.jobs[0].state, Failed { detail, .. } if detail.starts_with("encoderBusy"))
        );
        assert_eq!(s.retry_in(0), None);
        assert_eq!(s.next(), Action::Start(1), "a failed copy holds nothing");
        s.started(1);
        s.paused(1, Pause::Thermal);
        assert_eq!(s.retry_in(1), Some(THERMAL_WAIT));
        assert_eq!(s.next(), Action::Nothing);
        s.wake(1);
        assert_eq!(s.next(), Action::Start(1));
        s.started(1);
        s.paused(1, Pause::Disk);
        assert_eq!(s.retry_in(1), None, "a full disk waits for Resume");
        s.wake(1);
        assert_eq!(s.next(), Action::Nothing);
        s.resume_all();
        assert_eq!(s.next(), Action::Start(1));
    }

    /// A worker that stopped without a word is started again by itself, after 5 s and then 30 s,
    /// holding the copies after it meanwhile; a third stop fails the copy, waiting for Try again, which gives it its
    /// restarts back. Its words never blame the file. A hold still wins over a restart that came due.
    #[test]
    fn a_stopped_worker_is_started_again_twice_then_waits_for_try_again() {
        let mut s = ScheduleState::with(vec![job("a.mp4", Waiting), job("b.mp4", Waiting)]);
        let mut waits = Vec::new();
        for restart in 1..=2 {
            assert_eq!(s.next(), Action::Start(0));
            s.started(0);
            s.crashed(0, "killed");
            assert_eq!(s.jobs[0].restarts, restart);
            assert_eq!(
                s.jobs[0].reason(),
                Some(CodedError::new("copies.paused.workerStopped"))
            );
            assert_eq!(s.next(), Action::Nothing, "it goes first, in a moment");
            waits.push(s.retry_in(0).unwrap().as_secs());
            s.wake(0);
        }
        assert_eq!(waits, [5, 30]);
        // While a hold holds copies, a stopped worker's row says the hold's words, not "Trying again".
        s.paused(0, Pause::WorkerStopped);
        s.playing = true;
        let said = |s: &ScheduleState| s.list().jobs[0].reason.clone().map(|r| r.code);
        assert_eq!(said(&s).as_deref(), Some("copies.paused.playing"));
        s.user_paused = true;
        assert_eq!(said(&s).as_deref(), Some("copies.paused.held"));
        (s.playing, s.user_paused) = (false, false);
        assert_eq!(said(&s).as_deref(), Some("copies.paused.workerStopped"));
        s.wake(0);
        // A restart that came due while the video plays waits for it to stop.
        s.playing = true;
        assert_eq!(s.next(), Action::Nothing);
        s.playing = false;
        assert_eq!(s.next(), Action::Start(0));
        s.started(0);
        s.crashed(0, "killed again");
        assert_eq!(
            s.jobs[0].reason(),
            Some(CodedError::new("copies.failed.workerKeepsStopping")),
            "no file name: never the file's fault"
        );
        assert_eq!(s.retry_in(0), None, "it waits for Try again");
        assert_eq!(s.next(), Action::Start(1), "a failed copy holds nothing");
        s.started(1);
        s.done(1, "/cache/b.mp4".into());
        s.retry(0);
        assert_eq!(
            s.jobs[0].restarts, 0,
            "Try again gives it its restarts back"
        );
        assert_eq!(s.next(), Action::Start(0));
    }

    /// Every reason a copy is paused or failed is a code of its own in the app's table, with the file's name where
    /// its message has it, so the page never reads English. A copy that waits, works, is done or was stopped has none.
    #[test]
    fn each_pause_and_failure_is_its_own_code() {
        use klipolo_model::coded::english;

        let pauses = [
            (Pause::Playing, "copies.paused.playing"),
            (Pause::Exporting, "copies.paused.exporting"),
            (Pause::Recording, "copies.paused.recording"),
            (Pause::User, "copies.paused.held"),
            (Pause::Disk, "copies.paused.diskFull"),
            (Pause::Thermal, "copies.paused.hot"),
            (Pause::Encoder, "copies.paused.encoderBusy"),
            (Pause::WorkerStopped, "copies.paused.workerStopped"),
        ];
        let failures = [
            (Failure::Missing, "copies.failed.missing"),
            (Failure::Unreadable, "copies.failed.unreadable"),
            (Failure::UnknownLength, "copies.failed.unknownLength"),
            (Failure::UnusualRate, "copies.failed.unusualRate"),
            (Failure::CacheUnwritable, "copies.failed.cacheUnwritable"),
            (Failure::EncoderBusy, "copies.failed.encoderBusy"),
            (Failure::Worker, "copies.failed.worker"),
            (
                Failure::WorkerKeepsStopping,
                "copies.failed.workerKeepsStopping",
            ),
            (Failure::Other, "copies.failed.other"),
        ];
        let mut codes = HashSet::new();
        for (pause, code) in pauses {
            let reason = job("a.mp4", Paused(pause)).reason().unwrap();
            assert_eq!(reason, CodedError::new(code));
            assert!(codes.insert(code));
        }
        for (failure, code) in failures {
            let state = Failed {
                failure,
                detail: "for the log".into(),
            };
            let reason = job("intro.mp4", state).reason().unwrap();
            assert_eq!(reason.code, code);
            assert!(codes.insert(code));
            // Its values are the ones its message names: the English (logs) has no gap.
            let said = english(&[crate::english::APP], &reason);
            assert!(!said.contains('{'), "{code}: {said}");
            assert!(!said.contains("for the log"), "the detail stays in the log");
        }
        for state in [Waiting, Working, Done("/cache/a.mp4".into()), Stopped] {
            assert_eq!(job("a.mp4", state).reason(), None);
        }

        // Why a job was refused, or a worker failed, as the row says it: a read-only cache, a file that doesn't say how
        // long it is, a source that isn't there.
        assert_eq!(
            Failure::refused(&Refused::CacheUnwritable("Read-only file system".into())),
            Failure::CacheUnwritable
        );
        assert_eq!(
            Failure::refused(&Refused::UnknownLength),
            Failure::UnknownLength
        );
        assert_eq!(
            Failure::refused(&Refused::UnusualRate),
            Failure::UnusualRate
        );
        assert_eq!(
            Failure::refused(&Refused::SourceUnavailable),
            Failure::Missing
        );
        assert_eq!(Failure::refused(&Refused::Unreadable), Failure::Unreadable);
        assert_eq!(Failure::of(FailCode::Missing), Failure::Missing);
        assert_eq!(
            Failure::of(FailCode::CacheUnwritable),
            Failure::CacheUnwritable
        );
        assert_eq!(Failure::of(FailCode::Other), Failure::Other);
    }

    /// The list says what holds every copy (the person's pause, the action's, automatic copies off), so a page loaded
    /// again knows who paused; and with copies off, a copy to be made again leaves the list instead of waiting.
    #[test]
    fn the_list_says_who_holds_copies_and_copies_off_keep_no_waiting_row() {
        let mut s = ScheduleState::with(vec![job("a.mp4", Waiting), job("b.mp4", Waiting)]);
        assert_eq!(s.list().held, []);
        s.action_paused = true;
        s.user_paused = true;
        assert_eq!(
            s.list().held,
            [MediaJobsHold::Person, MediaJobsHold::Action]
        );
        s.resume_all();
        assert_eq!(s.list().held, [MediaJobsHold::Action], "the action's stays");
        s.resume_action();
        s.auto = false;
        assert_eq!(s.list().held, [MediaJobsHold::AutoOff]);

        s.done(0, "/cache/a.mp4".into());
        s.requeue(0);
        assert_eq!(
            s.list()
                .jobs
                .iter()
                .map(|j| j.name.as_str())
                .collect::<Vec<_>>(),
            ["b.mp4"],
            "nothing would make it again"
        );
        s.auto = true;
        s.requeue(0);
        assert_eq!(s.jobs[0].state, Waiting);
    }
}
