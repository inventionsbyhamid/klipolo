//! Waiting for the sound track without blocking the worker's runtime, and the heartbeat its parent hears
//! meanwhile: a sound that advances is never mistaken for a stall, and one that stops advancing goes quiet so the
//! parent's watchdog still catches it.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use super::shared::{WorkerError, internal};
use crate::audio::mix::RATE;

/// How far the sound has got: samples into the current pass (encoding or the true-peak check) and that pass's
/// length.
#[derive(Default)]
pub struct SoundProgress {
    done: AtomicU64,
    total: AtomicU64,
}

impl SoundProgress {
    pub fn start_pass(&self, samples: u64) {
        self.total.store(samples, Ordering::Relaxed);
        self.done.store(0, Ordering::Relaxed);
    }

    pub fn advance(&self, samples: u64) {
        self.done.fetch_add(samples, Ordering::Relaxed);
    }

    /// Seconds done and in all, of the current pass.
    fn seconds(&self) -> (f64, f64, u64) {
        let (done, total) = (
            self.done.load(Ordering::Relaxed),
            self.total.load(Ordering::Relaxed),
        );
        let rate = f64::from(RATE);
        (
            done as f64 / rate,
            total as f64 / rate,
            done + (total << 32),
        )
    }
}

/// The sound thread and its progress.
pub struct Sound {
    pub thread: JoinHandle<Result<bool, WorkerError>>,
    pub progress: Arc<SoundProgress>,
}

/// Waits for the sound without blocking the runtime. Every `tick` that it advanced, `beat(done_s, total_s)` (the
/// worker's heartbeat to its parent); a sound that stops advancing goes quiet, so the parent's watchdog still
/// catches a stuck one. Its error is returned as it is.
pub async fn wait(
    sound: Sound,
    beat: &mut dyn FnMut(f64, f64),
    tick: Duration,
) -> Result<bool, WorkerError> {
    let Sound { thread, progress } = sound;
    let joined = tokio::task::spawn_blocking(move || thread.join());
    tokio::pin!(joined);
    let mut ticker = tokio::time::interval(tick);
    ticker.tick().await;
    let mut seen = progress.seconds().2;
    loop {
        tokio::select! {
            done = &mut joined => {
                return done
                    .map_err(internal)?
                    .map_err(|_| internal("the sound crashed"))?;
            }
            _ = ticker.tick() => {
                let (done_s, total_s, mark) = progress.seconds();
                if mark != seen {
                    seen = mark;
                    beat(done_s, total_s);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::protocol::ErrorCode;

    #[tokio::test]
    async fn waiting_beats_while_the_sound_advances_and_returns_its_error() {
        let progress = Arc::new(SoundProgress::default());
        let shared = progress.clone();
        let thread = std::thread::spawn(move || {
            shared.start_pass(48_000);
            for _ in 0..20 {
                std::thread::sleep(Duration::from_millis(10));
                shared.advance(2_400);
            }
            Ok(true)
        });
        let mut beats = Vec::new();
        let sound = Sound { thread, progress };
        let tick = Duration::from_millis(30);
        let done = wait(
            sound,
            &mut |done_s, total_s| beats.push((done_s, total_s)),
            tick,
        )
        .await;
        assert!(matches!(done, Ok(true)));
        assert!(beats.len() >= 3, "{beats:?}");
        assert!(beats.windows(2).all(|w| w[0].0 < w[1].0) && beats[0].1 == 1.0);

        let thread = std::thread::spawn(|| {
            Err(WorkerError {
                code: ErrorCode::DiskFull,
                message: "disk full".into(),
                retryable: false,
            })
        });
        let sound = Sound {
            thread,
            progress: Arc::default(),
        };
        let error = wait(sound, &mut |_, _| {}, tick).await.unwrap_err();
        assert_eq!(error.code, ErrorCode::DiskFull);
    }
}
