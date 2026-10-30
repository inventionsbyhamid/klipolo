//! The export mix: every source decoded into the PCM cache first (the editor's cache when the
//! job names it, so nothing is decoded twice), the whole mix measured once (EBU R128 integrated loudness) for the
//! loudness gain, which the job keeps (a resumed job doesn't measure again), then rendered block by block through a
//! `Mixdown`, the gain and a true-peak limiter (`sound.rs` writes the track and checks its true peak).

use std::path::Path;
use std::sync::Arc;

use klipolo_model::project::{Loudness, Project};
use klipolo_model::time::RangeUs;

use super::job::{ExportJob, Levels};
use super::protocol::ErrorCode;
use super::shared::{WorkerError, is_disk_full};
use crate::api::ExportQuality;
use crate::audio::limiter::Limiter;
use crate::audio::loudness::{effective_target, level_db, measure};
use crate::audio::mix::{Mixdown, finish};
use crate::audio::pcm::{BuildError, PcmKey, PcmStore};
use crate::audio::plan::{AudioPlan, audio_plan};
use crate::log_name;
use crate::registry::Registry;

/// Every export's true peak stays at or under −1 dBTP (uploads; platforms re-encode). The limiter's output measures
/// −2.0 dBTP on libebur128 (its 4× detector reads about 0.5 dB under the meter's), which leaves room for the AAC
/// encoder's overshoot (0.2–1.1 dB measured); `sound.rs` measures the encoded track and lowers the ceiling if
/// it still went over.
pub const CEILING_DBTP: f32 = -2.5;

impl Levels {
    /// The export's gain decision for a mix measured at `lufs` (the preview makes the same one: `level_db`).
    pub fn decide(lufs: Option<f64>, loudness: Loudness, quality: ExportQuality) -> Levels {
        Levels {
            lufs,
            gain_db: level_db(lufs, loudness, quality),
        }
    }
}

/// What an export job's sound is made with besides its project and registry: its quality, the preview's PCM cache, the
/// measurement it keeps and where someone speaks, frozen with it (`ExportJob::speech_ranges`). Read from the job under
/// its lock (`JobSound::of`), then prepared off it (`prepare`), as the worker does.
#[derive(Clone, Debug)]
pub struct JobSound {
    pub quality: ExportQuality,
    pub pcm_dir: Option<std::path::PathBuf>,
    pub levels: Option<Levels>,
    pub speech: Arc<[RangeUs]>,
}

impl JobSound {
    pub fn of(job: &ExportJob) -> Self {
        Self {
            quality: job.quality,
            pcm_dir: job.pcm_dir.clone(),
            levels: job.levels.clone(),
            speech: job.speech_ranges.as_slice().into(),
        }
    }

    /// `ExportAudio::prepare` with these settings. Blocking.
    pub fn prepare(
        self,
        project: &Project,
        registry: &Registry,
    ) -> Result<(ExportAudio, Levels), WorkerError> {
        ExportAudio::prepare(
            project,
            registry,
            self.quality,
            self.pcm_dir.as_deref(),
            self.levels,
            self.speech,
        )
    }
}

/// The decoded, measured sound (cheap to clone; each pass streams its own `AudioStream` on its own thread).
#[derive(Clone)]
pub struct ExportAudio {
    plan: Arc<AudioPlan>,
    store: Arc<PcmStore>,
    gain: f32,
}

impl ExportAudio {
    /// Decodes the project's sound and measures it (unless `levels` has the measurement already). Blocking (run it
    /// off the async threads). `speech` is where someone speaks, frozen with the job (`ExportJob::speech_ranges`): a
    /// ducking sound layer goes down under it as it does in the preview.
    pub fn prepare(
        project: &Project,
        registry: &Registry,
        quality: ExportQuality,
        pcm_dir: Option<&Path>,
        levels: Option<Levels>,
        speech: Arc<[RangeUs]>,
    ) -> Result<(Self, Levels), WorkerError> {
        let plan = Arc::new(audio_plan(project, registry, speech));
        let dir = pcm_dir.map_or_else(
            || std::env::temp_dir().join("klipolo-pcm"),
            Path::to_path_buf,
        );
        let store = PcmStore::new(dir);
        for s in &plan.sources {
            store.build(&PcmKey::of(s)).map_err(|error| {
                tracing::warn!(%error, file = %log_name(&s.path), "export audio decode");
                // The cache couldn't be written because the disk (or the user's quota) is full: the job pauses.
                if let BuildError::Cache(e) = &error
                    && is_disk_full(e)
                {
                    return WorkerError {
                        code: ErrorCode::DiskFull,
                        message: "disk full".into(),
                        retryable: false,
                    };
                }
                WorkerError {
                    code: ErrorCode::Decode,
                    message: format!(
                        "The sound of this file couldn't be read: {}",
                        file_name(&s.path)
                    ),
                    retryable: false,
                }
            })?;
        }
        let levels = levels.unwrap_or_else(|| {
            // Nothing to level to (Off, or Master): the mix isn't measured. Otherwise exactly: the rendered mix.
            let lufs = effective_target(plan.loudness, quality)
                .and_then(|_| measure(&plan, &|s| store.ready(&PcmKey::of(s))));
            Levels::decide(lufs, plan.loudness, quality)
        });
        let audio = Self {
            plan,
            store,
            gain: 10f32.powf(levels.gain_db as f32 / 20.0),
        };
        Ok((audio, levels))
    }

    /// What it mixes: the same plan the preview plays for the same project and speech.
    pub fn plan(&self) -> &Arc<AudioPlan> {
        &self.plan
    }

    /// The mix from the start of the timeline, limited at `ceiling_dbtp`.
    pub fn stream(&self, ceiling_dbtp: f32) -> AudioStream {
        let mut stream = AudioStream {
            mixdown: Mixdown::new(self.plan.clone()),
            store: self.store.clone(),
            limiter: Limiter::new(ceiling_dbtp, self.gain),
            at: 0,
        };
        // Prime the limiter's look-ahead: its first output is then timeline frame 0.
        let mut prime = [0f32; Limiter::LATENCY * 2];
        stream.render(&mut prime);
        stream
    }
}

/// Consecutive blocks of the limited mix (48 kHz interleaved stereo).
pub struct AudioStream {
    mixdown: Mixdown,
    store: Arc<PcmStore>,
    limiter: Limiter,
    /// The next timeline frame going into the limiter.
    at: u64,
}

impl AudioStream {
    fn render(&mut self, out: &mut [f32]) {
        let store = &self.store;
        self.mixdown
            .render(&|s| store.ready(&PcmKey::of(s)), self.at, out);
        self.limiter.process(out);
        // Linear below −1 dBFS: a safety net only, the limiter already keeps peaks under the ceiling.
        finish(out, 1.0, 1.0);
        self.at += (out.len() / 2) as u64;
    }

    /// The next `out.len() / 2` frames.
    pub fn next(&mut self, out: &mut [f32]) {
        self.render(out);
    }
}

fn file_name(path: &str) -> &str {
    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path)
}
