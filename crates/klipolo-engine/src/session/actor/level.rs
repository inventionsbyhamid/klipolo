//! The project's loudness in the preview: once edits have settled for a second, the mix's loudness is estimated on a
//! utility-QoS thread from each file's 100 ms energies (`loudness::estimate_until`: milliseconds, where mixing the
//! whole timeline again took seconds; measured exactly only when a file has no energies), cancelled by the next
//! edit. The master gain follows the export's own rule at the default quality (`loudness::effective_target`: the
//! project's target, −14 LUFS for a new project), so the preview sounds like the export. A project whose loudness is
//! Off plays at unity and is never measured. Edits that don't change the sound measure nothing.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use klipolo_model::project::Loudness;
use tokio::sync::oneshot;

use super::Actor;
use crate::audio::loudness::{
    Estimate, PREVIEW_QUALITY, effective_target, estimate_until, level_db, measure_until,
};
use crate::audio::pcm::PcmKey;
use crate::audio::plan::{AudioPlan, AudioSource};
use crate::session::PreviewBackend;

/// Quiet time after the last edit before measuring.
const SETTLE: Duration = Duration::from_secs(1);

struct Job {
    plan: Arc<AudioPlan>,
    cancel: Arc<AtomicBool>,
    gain: oneshot::Receiver<f32>,
}

#[derive(Default)]
pub(super) struct Leveller {
    /// The sound to level, and when to start.
    wanted: Option<Arc<AudioPlan>>,
    due: Option<Instant>,
    /// The sound the current gain was measured for.
    done: Option<Arc<AudioPlan>>,
    job: Option<Job>,
}

impl Leveller {
    pub(super) fn due(&self) -> Option<Instant> {
        self.due
    }

    /// The project closed: stop measuring it, and forget its gain (the next project starts at unity until measured).
    pub(super) fn reset(&mut self) {
        if let Some(job) = self.job.take() {
            job.cancel.store(true, Ordering::Release);
        }
        *self = Leveller::default();
    }

    /// The committed project's sound changed (or not): measure again once it settles.
    pub(super) fn want(&mut self, plan: Arc<AudioPlan>) {
        let measuring = self.job.as_ref().is_some_and(|j| j.plan == plan);
        if self.done.as_ref() == Some(&plan) || measuring {
            self.wanted = Some(plan);
            self.due = None;
            return;
        }
        if let Some(job) = self.job.take() {
            job.cancel.store(true, Ordering::Release);
        }
        self.wanted = Some(plan);
        self.due = Some(Instant::now() + SETTLE);
    }
}

/// The preview's master gain for a mix measured at `lufs`: the export's decision at the default quality.
pub(crate) fn preview_gain(lufs: Option<f64>, loudness: Loudness) -> f32 {
    10f64.powf(level_db(lufs, loudness, PREVIEW_QUALITY) / 20.0) as f32
}

/// Resolves when a measurement is due; never without one.
pub(super) async fn level_due(due: Option<Instant>) {
    match due {
        Some(at) => tokio::time::sleep_until(at.into()).await,
        None => std::future::pending().await,
    }
}

/// The running measurement's gain; never resolves without one. Cancel-safe.
pub(super) async fn level_done(l: &mut Leveller) -> Option<f32> {
    match &mut l.job {
        Some(job) => (&mut job.gain).await.ok(),
        None => std::future::pending().await,
    }
}

impl<B: PreviewBackend> Actor<B> {
    pub(super) fn start_level(&mut self) {
        let l = &mut self.level;
        l.due = None;
        let Some(plan) = l.wanted.clone() else { return };
        let target = effective_target(plan.loudness, PREVIEW_QUALITY);
        let (Some(_), Some(store)) = (target, self.p.backend.pcm()) else {
            self.p.backend.set_master_gain(1.0);
            l.done = Some(plan);
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, gain) = oneshot::channel();
        let (job_plan, job_cancel, loudness) = (plan.clone(), cancel.clone(), plan.loudness);
        let spawned = std::thread::Builder::new()
            .name("klipolo-loudness".into())
            .spawn(move || {
                crate::qos::set_current(crate::qos::Qos::Utility);
                // Every file's finished audio and its energies first (failed files are silent), then the mix.
                let mut files = HashMap::new();
                for s in &job_plan.sources {
                    let key = PcmKey::of(s);
                    if let std::collections::hash_map::Entry::Vacant(slot) = files.entry(key) {
                        let pcm = store.wait_complete(slot.key(), &job_cancel);
                        let energies = pcm.as_ref().and_then(|p| store.energies(slot.key(), p));
                        slot.insert((pcm, energies));
                    }
                }
                let pcm = |s: &_| files.get(&PcmKey::of(s)).and_then(|f| f.0.clone());
                let files = &files;
                let energies =
                    |s: &AudioSource| files.get(&PcmKey::of(s)).and_then(|f| f.1.as_ref());
                let effected =
                    |s: &AudioSource, p: &Arc<_>, c: &AtomicBool| store.effected(s, p, c);
                let lufs = match estimate_until(&job_plan, &pcm, &energies, &effected, &job_cancel)
                {
                    Estimate::Lufs(lufs) => lufs,
                    Estimate::Missing => measure_until(&job_plan, &pcm, &job_cancel),
                    Estimate::Cancelled => return,
                };
                if !job_cancel.load(Ordering::Acquire) {
                    let _ = tx.send(preview_gain(lufs, loudness));
                }
            });
        match spawned {
            Ok(_) => l.job = Some(Job { plan, cancel, gain }),
            Err(error) => tracing::warn!(%error, "loudness thread"),
        }
    }

    pub(super) fn on_level(&mut self, gain: Option<f32>) {
        let Some(job) = self.level.job.take() else {
            return;
        };
        if let Some(gain) = gain {
            self.p.backend.set_master_gain(gain);
            self.level.done = Some(job.plan);
        }
    }
}

#[cfg(test)]
mod tests {
    use klipolo_model::project::{Loudness, Project};

    use super::preview_gain;
    use crate::api::ExportQuality;
    use crate::audio::loudness::{PREVIEW_QUALITY, effective_target};
    use crate::export::job::Levels;

    /// The gain (dB) the preview plays a mix measured at `lufs` with.
    fn preview_db(lufs: f64, loudness: Loudness) -> f64 {
        20.0 * f64::from(preview_gain(Some(lufs), loudness)).log10()
    }

    #[test]
    fn off_is_untouched_everywhere_and_a_new_project_aims_at_minus_14() {
        // A quiet mic recording and a hot music bed.
        for lufs in [-26.0, -8.0] {
            // Off: no gain in the preview or in any export, and nothing to measure for.
            assert!(effective_target(Loudness::Off, PREVIEW_QUALITY).is_none());
            assert_eq!(preview_db(lufs, Loudness::Off), 0.0);
            for quality in [ExportQuality::High, ExportQuality::Master] {
                assert_eq!(
                    Levels::decide(Some(lufs), Loudness::Off, quality).gain_db,
                    0.0
                );
            }
            // A new project: −14 LUFS in the preview and in a High export, the same gain in both.
            let loudness = Project::new("Demo").audio.loudness;
            assert_eq!(effective_target(loudness, PREVIEW_QUALITY), Some(-14.0));
            assert_eq!(effective_target(loudness, ExportQuality::High), Some(-14.0));
            let export = Levels::decide(Some(lufs), loudness, ExportQuality::High).gain_db;
            assert!((export - (-14.0 - lufs)).abs() < 1e-9, "{lufs}: {export}");
            assert!((preview_db(lufs, loudness) - export).abs() < 1e-3);
            // Master never levels.
            assert_eq!(
                Levels::decide(Some(lufs), loudness, ExportQuality::Master).gain_db,
                0.0
            );
        }
        // Silence gets no gain.
        assert_eq!(preview_gain(None, Loudness::Youtube), 1.0);
    }
}
