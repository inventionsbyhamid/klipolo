//! One clip's analysis, step by step (the module notes of `analysis` say what each step does), and the run it belongs
//! to: the clips started together, which stop together.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use klipolo_analysis::cache::{Dir, content_key};
use klipolo_analysis::providers::meta::Meta;
use klipolo_analysis::providers::{ProviderErrorKind, Transcriber};
use klipolo_analysis::scenes;
use klipolo_analysis::transcript::{Sound, TranscriptOut, align, finish, hear};
use klipolo_analysis::{AnalysisError, VideoError};
use klipolo_model::speech::Word;
use klipolo_platform::qos::{self, Qos};
use serde::Serialize;

use super::calls::{Calls, Meter};
use super::{AnalysisState, AnalysisStep, Cached, Clip, Media, NEEDS_KEY, UNREADABLE, lock};

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

/// How a clip's analysis ended.
pub(crate) enum Outcome {
    Done {
        approximate: bool,
    },
    Failed(String),
    /// Cancelled: back to not analysed.
    Stopped,
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
        key: Some(key),
    }
}

impl AnalysisState {
    /// The cache's results for `clip`, read off the async threads.
    pub(crate) async fn look(&self, clip: &Clip) -> Cached {
        let (root, path, video) = (self.cache_root.clone(), clip.path.clone(), clip.video);
        tokio::task::spawn_blocking(move || look(&root, &path, video))
            .await
            .unwrap_or_default()
    }

    /// Analyses `clip` (its cache read as `cached`) in `run`, through `first` (the adapter when it started; None when
    /// everything is cached), and says how it ended.
    pub(crate) async fn analyse(
        self: Arc<Self>,
        clip: Clip,
        cached: Cached,
        run: Arc<Run>,
        first: Option<Meta>,
    ) {
        let outcome = self.steps(&clip, cached, &run, first).await;
        self.end(&clip.id, outcome);
    }

    async fn steps(
        self: &Arc<Self>,
        clip: &Clip,
        cached: Cached,
        run: &Arc<Run>,
        first: Option<Meta>,
    ) -> Outcome {
        let Some(key) = cached.key else {
            return Outcome::Failed(UNREADABLE.into());
        };
        let Ok(dir) = Dir::new(&self.cache_root, &key) else {
            return Outcome::Failed(UNREADABLE.into());
        };
        if let Some(found) = &cached.transcript {
            self.found_transcript(&clip.id, found);
        }
        let scenes_cached = cached.scenes.is_some();
        if let Some(found) = cached.scenes {
            self.found_scenes(&clip.id, found);
        }
        if clip.has_audio {
            let (src, run) = (clip.path.clone(), run.clone());
            if let Ok(Some(levels)) = self
                .local(move |media| media.levels(&src, &run.cancel))
                .await
            {
                self.found_levels(&clip.id, levels);
            }
        }
        let needs_scenes = clip.video && !scenes_cached;
        if let (Some(found), false) = (&cached.transcript, needs_scenes) {
            return Outcome::Done {
                approximate: found.approximate,
            };
        }
        let Some(first) = first else {
            return Outcome::Failed(NEEDS_KEY.into());
        };
        let meter = {
            let (state, id) = (self.clone(), clip.id.clone());
            Meter::new(move |step, pct| state.progress(&id, step, pct))
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
        let transcript = match cached.transcript {
            Some(found) => found,
            None => match self
                .transcribe(clip, &calls, run, &work.0, &dir, &key)
                .await
            {
                Ok(made) => made,
                Err(outcome) => return outcome,
            },
        };
        if needs_scenes {
            if run.cancelled() {
                return Outcome::Stopped;
            }
            if let Err(outcome) = self
                .log_scenes(
                    clip,
                    &calls,
                    run,
                    &work.0,
                    &dir,
                    &transcript.transcript.words,
                )
                .await
            {
                return outcome;
            }
        }
        Outcome::Done {
            approximate: transcript.approximate,
        }
    }

    /// The clip's transcript: its sound, its turns (published at once as turn-level captions), its words timed;
    /// cached once timed. A clip with no sound, or only silence, makes no request and has no words.
    async fn transcribe(
        self: &Arc<Self>,
        clip: &Clip,
        calls: &Calls,
        run: &Arc<Run>,
        work: &Path,
        dir: &Dir,
        key: &str,
    ) -> Result<TranscriptOut, Outcome> {
        let model = Transcriber::model(calls).to_owned();
        let mut out = if clip.has_audio {
            self.progress(&clip.id, AnalysisStep::Audio, 0);
            let wav = work.join("sound.wav");
            let made = {
                let (src, wav) = (clip.path.clone(), wav.clone());
                self.local(move |media| media.sound(&src, &wav)).await
            };
            let info = made
                .map_err(Outcome::Failed)?
                .map_err(|e| self.failure(AnalysisError::from(e), run))?;
            self.progress(&clip.id, AnalysisStep::Audio, 100);
            if info.has_audio {
                let out = self
                    .hear_and_time(clip, calls, run, &wav, key, &model)
                    .await;
                let _ = std::fs::remove_file(&wav);
                out?
            } else {
                TranscriptOut::no_speech(&model, clip.duration_us, 0.0)
            }
        } else {
            TranscriptOut::no_speech(&model, clip.duration_us, 0.0)
        };
        out.transcript.key = key.to_owned();
        self.store(dir, "transcript", &out, &model).await;
        self.found_transcript(&clip.id, &out);
        Ok(out)
    }

    /// The words of the clip's WAV: `hear`, the turn-level transcript published, `align`, `finish`. Cancelled while
    /// timing words: what was timed is shown, and nothing is cached.
    async fn hear_and_time(
        self: &Arc<Self>,
        clip: &Clip,
        calls: &Calls,
        run: &Arc<Run>,
        wav: &Path,
        key: &str,
        model: &str,
    ) -> Result<TranscriptOut, Outcome> {
        if run.cancelled() {
            return Err(Outcome::Stopped);
        }
        let sound = Sound::load(wav, calls.max_audio_s())
            .await
            .map_err(|e| self.failure(e, run))?;
        if sound.is_silent() {
            return Ok(TranscriptOut::no_speech(model, sound.duration_us, 0.0));
        }
        calls
            .meter
            .begin(AnalysisStep::Transcript, sound.chunks.len(), 0, 100);
        let heard = hear(calls, &sound, &[])
            .await
            .map_err(|e| self.failure(e, run))?;
        self.spend(&clip.id, heard.cost_usd);
        if heard.turns.is_empty() {
            return Ok(TranscriptOut::no_speech(
                model,
                sound.duration_us,
                heard.cost_usd,
            ));
        }
        let mut rough = finish(
            model,
            sound.duration_us,
            heard.turns.clone(),
            None,
            heard.cost_usd,
        );
        rough.transcript.key = key.to_owned();
        self.found_transcript(&clip.id, &rough);
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
            out.transcript.key = key.to_owned();
            self.found_transcript(&clip.id, &out);
            return Err(Outcome::Stopped);
        }
        Ok(out)
    }

    /// The clip's scene log from its small copy, snapped to `words` (its transcript's), cached once logged. A clip
    /// with no picture after all has none.
    async fn log_scenes(
        self: &Arc<Self>,
        clip: &Clip,
        calls: &Calls,
        run: &Arc<Run>,
        work: &Path,
        dir: &Dir,
        words: &[Word],
    ) -> Result<(), Outcome> {
        // The copy is the step's first half, by how far into the clip its frames have got.
        calls.meter.begin(AnalysisStep::Scenes, 0, 0, 50);
        let made = {
            let (state, id, src, out, run) = (
                self.clone(),
                clip.id.clone(),
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
                        state.progress(&id, AnalysisStep::Scenes, pct);
                    }
                })
            })
            .await
            .map_err(Outcome::Failed)?
        };
        let copy = match made {
            Ok(copy) => copy,
            Err(VideoError::NoVideo) => return Ok(()),
            Err(e) => return Err(self.failure(AnalysisError::from(e), run)),
        };
        calls
            .meter
            .begin(AnalysisStep::Scenes, copy.chunks.len(), 50, 100);
        let log = scenes::log(calls, &copy, words)
            .await
            .map_err(|e| self.failure(e, run))?;
        self.spend(&clip.id, log.cost_usd);
        self.store(dir, "scenes", &log, &log.model).await;
        self.found_scenes(&clip.id, log);
        Ok(())
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

    /// Caches `value` as `<name>.json` with the model it came from. A cache that can't be written is noted in the log;
    /// the result still counts for this session.
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
