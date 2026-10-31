//! One clip's analysis, step by step (the module notes of `analysis` say what each step does), and the run it belongs
//! to: the clips started together, which stop together.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use klipolo_analysis::cache::{Dir, content_key};
use klipolo_analysis::providers::meta::Meta;
use klipolo_analysis::providers::{ProviderErrorKind, Transcriber};
use klipolo_analysis::scenes::{self, SceneLog};
use klipolo_analysis::transcript::{ROUGH_WORDS, Sound, TranscriptOut, align, finish, hear};
use klipolo_analysis::{AnalysisError, VideoError};
use klipolo_model::speech::Levels;
use klipolo_model::speech::Word;
use klipolo_platform::qos::{self, Qos};
use serde::{Deserialize, Serialize};

use super::calls::{Calls, Meter};
use super::{
    AnalysisState, AnalysisStep, AssetState, Cached, Clip, Content, Media, NEEDS_KEY, UNREADABLE,
    lock,
};

/// The clips started together. Cancel stops every request not sent yet; a used-up credit (or a refused key, or no way
/// to pay) stops every paid request not sent yet, with the first such reason.
#[derive(Default)]
pub(crate) struct Run {
    cancel: AtomicBool,
    stop: Mutex<Option<(ProviderErrorKind, String)>>,
}

impl Run {
    pub(crate) fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    pub(crate) fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    /// Why paid requests stopped, if they did.
    pub(crate) fn stopped(&self) -> Option<(ProviderErrorKind, String)> {
        lock(&self.stop).clone()
    }

    /// No more paid requests in this run, because `reason` (the first reason given stays).
    pub(crate) fn stop_paying(&self, kind: ProviderErrorKind, reason: &str) {
        lock(&self.stop).get_or_insert_with(|| {
            tracing::info!(%reason, "analysis: no more paid requests in this run");
            (kind, reason.to_owned())
        });
    }

    /// Whether new clips may still join this run.
    pub(crate) fn open(&self) -> bool {
        !self.cancelled() && self.stopped().is_none()
    }
}

/// Why a transcript cached when Cancel stopped its word timing is approximate (it is timed again on the next run).
pub(crate) const WORDS_STOPPED: &str = "Word timing stopped before it finished.";
/// Why the turn-level transcript cached before its words are timed is approximate. The timed one replaces it; one left
/// by a quit or a crash is timed again on the next run.
pub(crate) const WORDS_UNFINISHED: &str = "Word timing didn't finish last time.";

/// Whether `found`'s words are timed roughly for a reason another try may fix (word timing failed, stopped or was
/// unreachable), rather than because its turns don't match their sound (`transcript::ROUGH_WORDS`).
pub(crate) fn timed_again(found: &TranscriptOut) -> bool {
    found.approximate
        && !found.turns.is_empty()
        && found.approximate_reason.as_deref() != Some(ROUGH_WORDS)
}

/// What `hear_and_time` heard.
enum Heard {
    /// The whole transcript, its words timed (or roughly, where word timing failed).
    All(TranscriptOut),
    /// Cancelled while timing words: what was heard and timed so far.
    Stopped(TranscriptOut),
}

/// How a clip's analysis ended.
pub(crate) enum Outcome {
    Done(Words),
    Failed(String),
    /// Cancelled: back to not analysed.
    Stopped,
}

/// How a finished clip's words came out: timed, timed roughly (why, and whether timing them again can help), or none
/// (why).
pub(crate) struct Words {
    approximate: bool,
    approximate_reason: Option<String>,
    empty_reason: Option<String>,
    time_again: bool,
}

impl Words {
    fn of(found: &TranscriptOut) -> Self {
        Self {
            approximate: found.approximate,
            approximate_reason: found
                .approximate_reason
                .clone()
                .filter(|_| found.approximate),
            empty_reason: found.empty_reason.clone(),
            time_again: timed_again(found),
        }
    }

    /// The clip's state, done.
    pub(crate) fn state(self) -> AssetState {
        AssetState::Done {
            approximate: self.approximate,
            approximate_reason: self.approximate_reason,
            empty_reason: self.empty_reason,
            time_again: self.time_again,
        }
    }
}

/// What one content's steps work with.
struct Job<'a> {
    /// The content key its results fan out under.
    key: &'a str,
    clip: &'a Clip,
    calls: &'a Calls,
    run: &'a Arc<Run>,
    /// Its working folder.
    work: &'a Path,
    /// Its cache folder.
    dir: &'a Dir,
    /// A video whose scene log is still to do (after its words).
    needs_scenes: bool,
}

/// Why a clip's analysis ended without its steps saying how (its task panicked, or was dropped part-way).
pub(crate) const BROKE: &str = "Klipolo's analysis stopped unexpectedly. Try again.";

/// Ends a content's analysis as failed if its task goes before it ends (a panic, or the task dropped): its claim and
/// its promised credit are let go, and its assets told, so it can be analysed again.
struct Ending {
    state: Arc<AnalysisState>,
    key: String,
    ended: bool,
}

impl Drop for Ending {
    fn drop(&mut self) {
        if !self.ended {
            tracing::warn!("analysis: a clip's task ended part-way");
            self.state.end(&self.key, Outcome::Failed(BROKE.into()));
        }
    }
}

/// A clip's working folder (its WAV, its small copy), removed when dropped.
struct WorkDir(PathBuf);

impl WorkDir {
    fn new(root: &Path) -> Result<Self, Outcome> {
        let path = root.join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&path).map_err(|e| {
            tracing::warn!(%e, "analysis: no working folder");
            Outcome::Failed(UNREADABLE.into())
        })?;
        Ok(Self(path))
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// What the cache holds for the clip at `path`: its key and the results that load. A file that can't be read has no
/// key. Blocking.
pub(crate) fn look(root: &Path, path: &Path, video: bool) -> Cached {
    let Ok(key) = content_key(path) else {
        return Cached::default();
    };
    let Ok(dir) = Dir::new(root, &key) else {
        return Cached::default();
    };
    Cached {
        transcript: dir.load("transcript"),
        scenes: if video { dir.load("scenes") } else { None },
        levels: dir
            .load::<LevelsFile>("levels")
            .map(|file| Levels { db: file.db.into() }),
        key: Some(key),
    }
}

/// `levels.json`: a clip's sound levels, dB per `LEVEL_WINDOW_US` of its sound.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LevelsFile {
    db: Vec<f32>,
}

impl AnalysisState {
    /// The cache's results for `clip`, read off the async threads.
    pub(crate) async fn look(&self, clip: &Clip) -> Cached {
        let (root, path, video) = (self.cache_root.clone(), clip.path.clone(), clip.video);
        tokio::task::spawn_blocking(move || look(&root, &path, video))
            .await
            .unwrap_or_default()
    }

    /// Analyses `content` in `run`, through `first` (the adapter when it started; None when everything is cached), and
    /// tells each asset on it how it ended.
    pub(crate) async fn analyse(
        self: Arc<Self>,
        content: Content,
        run: Arc<Run>,
        first: Option<Meta>,
    ) {
        let Content { key, clip, cached } = content;
        let mut ending = Ending {
            state: self.clone(),
            key: key.clone(),
            ended: false,
        };
        let outcome = self.steps(&key, &clip, cached, &run, first).await;
        ending.ended = true;
        self.end(&key, outcome);
    }

    async fn steps(
        self: &Arc<Self>,
        key: &str,
        clip: &Clip,
        cached: Cached,
        run: &Arc<Run>,
        first: Option<Meta>,
    ) -> Outcome {
        // A file that couldn't be read has no content key, and so no cache folder.
        let Some(dir) = cached
            .key
            .as_deref()
            .and_then(|k| Dir::new(&self.cache_root, k).ok())
        else {
            return Outcome::Failed(UNREADABLE.into());
        };
        // What the cache holds shows at once (unless the estimate showed it already).
        self.show_cached(key, &cached);
        let scenes_cached = cached.scenes.is_some();
        // The levels: cached with the transcript, else from the sound cache (a decode), then cached.
        if clip.has_audio && cached.levels.is_none() {
            let (src, run) = (clip.path.clone(), run.clone());
            if let Ok(Some(levels)) = self
                .local(move |media| media.levels(&src, &run.cancel))
                .await
            {
                let file = LevelsFile {
                    db: levels.db.to_vec(),
                };
                self.store(&dir, "levels", &file, "").await;
                self.found_levels(key, levels);
            }
        }
        let needs_scenes = clip.video && !scenes_cached;
        // Words timed roughly because word timing failed or stopped are timed again when something can call the
        // aligner (free: no transcription).
        let retime = first.is_some() && cached.transcript.as_ref().is_some_and(timed_again);
        if let (Some(found), false, false) = (&cached.transcript, needs_scenes, retime) {
            return Outcome::Done(Words::of(found));
        }
        let Some(first) = first else {
            return Outcome::Failed(NEEDS_KEY.into());
        };
        let meter = {
            let (state, key) = (self.clone(), key.to_owned());
            Meter::new(move |step, pct| state.progress(&key, step, pct))
        };
        let calls = Calls::new(
            self.credentials.clone(),
            run.clone(),
            first,
            self.videos.clone(),
            meter,
        );
        let work = match WorkDir::new(&self.work_root) {
            Ok(work) => work,
            Err(outcome) => return outcome,
        };
        let job = Job {
            key,
            clip,
            calls: &calls,
            run,
            work: &work.0,
            dir: &dir,
            needs_scenes,
        };
        let transcript = match cached.transcript {
            Some(found) if retime => match self.time_again(&job, found).await {
                Ok(timed) => timed,
                Err(outcome) => return outcome,
            },
            Some(found) => found,
            None => match self.transcribe(&job).await {
                Ok(made) => made,
                Err(outcome) => return outcome,
            },
        };
        if needs_scenes {
            if run.cancelled() {
                return Outcome::Stopped;
            }
            if let Err(outcome) = self.log_scenes(&job, &transcript.transcript.words).await {
                return outcome;
            }
        }
        Outcome::Done(Words::of(&transcript))
    }

    /// The clip's transcript: its sound, its turns (published and cached at once as turn-level captions), its words
    /// timed; cached again once timed. A clip with no sound, or only silence, makes no request and has no words.
    async fn transcribe(self: &Arc<Self>, job: &Job<'_>) -> Result<TranscriptOut, Outcome> {
        let Job {
            key,
            clip,
            calls,
            run,
            work,
            dir,
            ..
        } = *job;
        if run.cancelled() {
            return Err(Outcome::Stopped);
        }
        let model = Transcriber::model(calls).to_owned();
        let mut out = if clip.has_audio {
            self.progress(key, AnalysisStep::Audio, 0);
            let wav = work.join("sound.wav");
            let made = {
                let (src, wav, run) = (clip.path.clone(), wav.clone(), run.clone());
                self.local(move |media| media.sound(&src, &wav, &run.cancel))
                    .await
            };
            let info = made
                .map_err(Outcome::Failed)?
                .map_err(|e| self.failure(AnalysisError::from(e), run))?;
            self.progress(key, AnalysisStep::Audio, 100);
            if info.has_audio {
                let heard = self.hear_and_time(job, &wav, &model).await;
                let _ = std::fs::remove_file(&wav);
                match heard? {
                    Heard::All(out) => out,
                    Heard::Stopped(mut out) => {
                        // Paid for already: kept, and the clip is analysed with what was timed. Its rough words are
                        // timed again by the person's Analyze, Retry or "Time words again" (never by the page's own
                        // start in this app run: `timing_stopped`).
                        out.transcript.key = key.to_owned();
                        self.store(dir, "transcript", &out, &model).await;
                        self.found_transcript(key, &out);
                        return Err(self.timing_stopped(job, &out));
                    }
                }
            } else {
                TranscriptOut::no_speech(&model, clip.duration_us, 0.0)
            }
        } else {
            TranscriptOut::no_speech(&model, clip.duration_us, 0.0)
        };
        out.transcript.key = key.to_owned();
        self.store(dir, "transcript", &out, &model).await;
        self.found_transcript(key, &out);
        Ok(out)
    }

    /// The words of the clip's WAV: `hear`, the turn-level transcript published and cached, `align`, `finish`.
    /// Cancelled while timing words: what was heard (and timed so far) is given as `Heard::Stopped`, to cache: its
    /// transcription is paid for, and its words can be timed again for free.
    async fn hear_and_time(
        self: &Arc<Self>,
        job: &Job<'_>,
        wav: &Path,
        model: &str,
    ) -> Result<Heard, Outcome> {
        let Job {
            key, calls, run, ..
        } = *job;
        if run.cancelled() {
            return Err(Outcome::Stopped);
        }
        let sound = Sound::load(wav, calls.max_audio_s())
            .await
            .map_err(|e| self.failure(e, run))?;
        if sound.is_silent() {
            return Ok(Heard::All(TranscriptOut::no_speech(
                model,
                sound.duration_us,
                0.0,
            )));
        }
        calls
            .meter
            .begin(AnalysisStep::Transcript, sound.chunks.len(), 0, 100);
        let heard = hear(calls, &sound, &[])
            .await
            .map_err(|e| self.failure(e, run))?;
        self.spend(key, heard.cost_usd);
        if heard.turns.is_empty() {
            return Ok(Heard::All(TranscriptOut::no_speech(
                model,
                sound.duration_us,
                heard.cost_usd,
            )));
        }
        let mut rough = finish(
            model,
            sound.duration_us,
            heard.turns.clone(),
            None,
            heard.cost_usd,
        );
        rough.transcript.key = key.to_owned();
        self.found_transcript(key, &rough);
        // Paid for already: cached now, so a quit or a crash while words are timed doesn't lose it. Its reason isn't
        // `ROUGH_WORDS`, so the next run times its words again (`timed_again`); the timed transcript replaces it.
        rough.approximate_reason = Some(WORDS_UNFINISHED.to_owned());
        self.store(job.dir, "transcript", &rough, model).await;
        calls
            .meter
            .begin(AnalysisStep::Words, heard.turns.len(), 0, 100);
        let timing = align(calls, &sound, &heard.turns).await;
        let mut out = finish(
            model,
            sound.duration_us,
            heard.turns,
            Some(timing),
            heard.cost_usd,
        );
        if run.cancelled() {
            if out.approximate {
                out.approximate_reason = Some(WORDS_STOPPED.to_owned());
            }
            return Ok(Heard::Stopped(out));
        }
        Ok(Heard::All(out))
    }

    /// `found`'s words timed again (no transcription: free), from the clip's WAV made again; cached and shown when
    /// more of its turns are timed than before. A WAV that can't be made, or an aligner that still fails, keeps
    /// `found` as it is.
    async fn time_again(
        self: &Arc<Self>,
        job: &Job<'_>,
        found: TranscriptOut,
    ) -> Result<TranscriptOut, Outcome> {
        let Job {
            key,
            clip,
            calls,
            run,
            work,
            dir,
            ..
        } = *job;
        if run.cancelled() {
            return Err(self.timing_stopped(job, &found));
        }
        self.progress(key, AnalysisStep::Audio, 0);
        let wav = work.join("sound.wav");
        let made = {
            let (src, wav, run) = (clip.path.clone(), wav.clone(), run.clone());
            self.local(move |media| media.sound(&src, &wav, &run.cancel))
                .await
        };
        let Ok(Ok(info)) = made else {
            return Ok(found);
        };
        let sound = match Sound::load(&wav, calls.max_audio_s()).await {
            Ok(sound) if info.has_audio => sound,
            _ => return Ok(found),
        };
        let _ = std::fs::remove_file(&wav);
        calls
            .meter
            .begin(AnalysisStep::Words, found.turns.len(), 0, 100);
        let timing = align(calls, &sound, &found.turns).await;
        let mut timed = finish(
            &found.transcript.model,
            sound.duration_us,
            found.turns.clone(),
            Some(timing),
            found.cost_usd,
        );
        let aligned = |t: &TranscriptOut| t.turns.iter().filter(|t| t.aligned).count();
        if aligned(&timed) <= aligned(&found) {
            return if run.cancelled() {
                Err(self.timing_stopped(job, &found))
            } else {
                Ok(found)
            };
        }
        timed.transcript.key = key.to_owned();
        if run.cancelled() && timed.approximate {
            timed.approximate_reason = Some(WORDS_STOPPED.to_owned());
        }
        self.store(dir, "transcript", &timed, &timed.transcript.model)
            .await;
        self.found_transcript(key, &timed);
        if run.cancelled() {
            return Err(self.timing_stopped(job, &timed));
        }
        Ok(timed)
    }

    /// The clip's scene log from its small copy, snapped to `words` (its transcript's), cached once logged. A clip
    /// with no picture after all has an empty one, made with no request.
    async fn log_scenes(self: &Arc<Self>, job: &Job<'_>, words: &[Word]) -> Result<(), Outcome> {
        let Job {
            key,
            clip,
            calls,
            run,
            work,
            dir,
            ..
        } = *job;
        // The copy is the step's first half, by how far into the clip its frames have got.
        calls.meter.begin(AnalysisStep::Scenes, 0, 0, 50);
        let made = {
            let (state, key, src, out, run) = (
                self.clone(),
                key.to_owned(),
                clip.path.clone(),
                work.join("copy"),
                run.clone(),
            );
            let duration = clip.duration_us.max(1);
            self.local(move |media| {
                let mut furthest = 0;
                media.small_copy(&src, &out, &run.cancel, &mut |at| {
                    if at > furthest {
                        furthest = at.min(duration);
                        let pct = (furthest as f64 / duration as f64 * 50.0).floor() as u8;
                        state.progress(&key, AnalysisStep::Scenes, pct);
                    }
                })
            })
            .await
            .map_err(Outcome::Failed)?
        };
        let copy = match made {
            Ok(copy) => copy,
            Err(VideoError::NoVideo) => {
                // No picture after all: an empty log, cached, so the clip is never offered again at a scene log's price.
                let log = SceneLog {
                    model: String::new(),
                    fps: 0.0,
                    items: Vec::new(),
                    cuts: Vec::new(),
                    cost_usd: 0.0,
                };
                self.store(dir, "scenes", &log, "").await;
                self.found_scenes(key, log);
                return Ok(());
            }
            Err(e) => return Err(self.failure(AnalysisError::from(e), run)),
        };
        calls
            .meter
            .begin(AnalysisStep::Scenes, copy.chunks.len(), 50, 100);
        let log = scenes::log(calls, &copy, words)
            .await
            .map_err(|e| self.failure(e, run))?;
        self.spend(key, log.cost_usd);
        self.store(dir, "scenes", &log, &log.model).await;
        self.found_scenes(key, log);
        Ok(())
    }

    /// The person stopped the job's word timing with `kept` cached: only the person's Analyze, Retry or "Time words
    /// again" times its words again. A clip with nothing else to do is analysed as far as it got (its words timed roughly where they weren't
    /// timed); a video whose scene log is still to do goes back to not analysed, as a Cancel during its scene log does
    /// (the page offers it again; it never starts an unfinished video on its own).
    fn timing_stopped(&self, job: &Job<'_>, kept: &TranscriptOut) -> Outcome {
        lock(&self.inner).timed_again.insert(job.key.to_owned());
        if job.needs_scenes {
            return Outcome::Stopped;
        }
        Outcome::Done(Words::of(kept))
    }

    /// How a failed step ends the clip: back to not analysed after Cancel, else failed with the step's sentence.
    fn failure(&self, e: AnalysisError, run: &Run) -> Outcome {
        if run.cancelled() {
            return Outcome::Stopped;
        }
        tracing::info!(error = %e, "analysis: a step failed");
        Outcome::Failed(e.user_message())
    }

    /// `work` on a thread of its own at utility priority, one clip at a time: decoding is heavy, and the editor runs
    /// beside it. A thread of its own, so the class never stays with a pooled thread.
    pub(crate) async fn local<T: Send + 'static>(
        &self,
        work: impl FnOnce(&dyn Media) -> T + Send + 'static,
    ) -> Result<T, String> {
        let stopped = || "Klipolo's analysis stopped. Try again.".to_owned();
        let _turn = self.local.acquire().await.map_err(|_| stopped())?;
        let media = self.media.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        std::thread::Builder::new()
            .name("klipolo-analysis".into())
            .spawn(move || {
                qos::set_current(Qos::Utility);
                let _ = tx.send(work(&*media));
            })
            .map_err(|e| {
                tracing::warn!(%e, "analysis: no thread");
                stopped()
            })?;
        rx.await.map_err(|_| stopped())
    }

    /// Caches `value` as `<name>.json` with the model it came from (none for what no model made). A cache that can't be
    /// written is noted in the log; the result still counts for this session.
    async fn store<T: Serialize + Clone + Send + 'static>(
        &self,
        dir: &Dir,
        name: &'static str,
        value: &T,
        model: &str,
    ) {
        let (dir, value, model) = (dir.clone(), value.clone(), model.to_owned());
        let stored = tokio::task::spawn_blocking(move || {
            dir.store(name, &value)?;
            if model.is_empty() {
                return Ok(());
            }
            dir.set_model(name, &model)
        })
        .await;
        match stored {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::warn!(%e, name, "analysis: not cached"),
            Err(e) => tracing::warn!(%e, name, "analysis: not cached"),
        }
    }
}
