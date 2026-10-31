//! The provider calls of one clip's analysis: the analysis traits (`Transcriber`, `Aligner`, `SceneLogger`) over the
//! app's current Meta adapter, so the pipeline in `klipolo_analysis` runs unchanged while this adds what the app needs:
//!
//! - **The run's stops** (`Run`): after Cancel nothing more is sent; after a used-up credit, a refused key or no way to
//!   pay, no more paid calls (word timing is free and carries on). Each check is made before a request, so the
//!   requests in flight finish.
//! - **A refused token or key (401):** through Klipolo's relay (every Meta call with an access code, and word timing in
//!   both modes) the device registers again once (`Credentials::recover_unauthorized`) and the request goes again with
//!   the new adapter. Meta refusing the person's own key is a wrong key: said so, nothing registered. Which one it was
//!   comes from the mode of the adapter that sent it, never from the error's text.
//! - **One video call at a time** for the whole app, whatever the mode.
//! - **Progress:** each answered request moves its step's meter on.
//!
//! The adapter is read from `Credentials` for each request, so a change in Settings (a key added, a token renewed)
//! counts from the next request on.

use std::path::Path;
use std::sync::{Arc, Mutex};

use klipolo_analysis::providers::meta::Meta;
use klipolo_analysis::providers::{
    AlignLimits, AlignTurn, Aligner, Alignment, BoxFuture, ProviderError, ProviderErrorKind,
    SceneAnswer, SceneLogger, Transcriber, Transcription,
};
use tokio::sync::Semaphore;

use super::run::Run;
use super::{ADD_KEY, AnalysisStep, NEEDS_KEY};
use crate::credentials::{Credentials, KEY_REFUSED};

/// Moves one clip's progress on: the step running, how many requests (or turns) it has, and how many are answered,
/// over a share of the step's 0 to 100.
pub(crate) struct Meter {
    tell: Box<dyn Fn(AnalysisStep, u8) + Send + Sync>,
    now: Mutex<Span>,
}

#[derive(Clone, Copy)]
struct Span {
    step: AnalysisStep,
    total: usize,
    done: usize,
    from: u8,
    to: u8,
}

impl Meter {
    pub(crate) fn new(tell: impl Fn(AnalysisStep, u8) + Send + Sync + 'static) -> Self {
        Self {
            tell: Box::new(tell),
            now: Mutex::new(Span {
                step: AnalysisStep::Audio,
                total: 0,
                done: 0,
                from: 0,
                to: 100,
            }),
        }
    }

    /// `step` starts, `total` parts of it to answer, taking it from `from` to `to` percent.
    pub(crate) fn begin(&self, step: AnalysisStep, total: usize, from: u8, to: u8) {
        *self.span() = Span {
            step,
            total,
            done: 0,
            from,
            to,
        };
        (self.tell)(step, from);
    }

    /// `n` more parts answered.
    pub(crate) fn advance(&self, n: usize) {
        let (step, pct) = {
            let mut span = self.span();
            span.done = (span.done + n).min(span.total);
            let share = if span.total == 0 {
                1.0
            } else {
                span.done as f64 / span.total as f64
            };
            let pct = f64::from(span.from) + share * f64::from(span.to - span.from);
            (span.step, pct.floor() as u8)
        };
        (self.tell)(step, pct);
    }

    fn span(&self) -> std::sync::MutexGuard<'_, Span> {
        self.now
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// One clip's calls (see the module notes).
pub(crate) struct Calls {
    credentials: Arc<Credentials>,
    run: Arc<Run>,
    /// The adapter when the clip started: its limits and estimates (the same for every adapter of a mode).
    first: Meta,
    videos: Arc<Semaphore>,
    pub(crate) meter: Meter,
}

impl Calls {
    pub(crate) fn new(
        credentials: Arc<Credentials>,
        run: Arc<Run>,
        first: Meta,
        videos: Arc<Semaphore>,
        meter: Meter,
    ) -> Self {
        Self {
            credentials,
            run,
            first,
            videos,
            meter,
        }
    }

    /// Whether this request may go: never after Cancel, and a paid one never after the run stopped paying.
    fn gate(&self, paid: bool) -> Result<(), ProviderError> {
        if self.run.cancelled() {
            return Err(ProviderError::new(
                ProviderErrorKind::Unavailable,
                super::STOPPED,
            ));
        }
        match self.run.stopped().filter(|_| paid) {
            Some((kind, reason)) => Err(ProviderError::new(kind, reason)),
            None => Ok(()),
        }
    }

    /// The adapter in force now.
    fn adapter(&self) -> Result<Meta, ProviderError> {
        self.credentials
            .meta()
            .ok_or_else(|| ProviderError::new(ProviderErrorKind::NotAllowed, NEEDS_KEY))
    }

    /// After a 401 from the relay: the device registered again (once), and the adapter with its new token.
    async fn recovered(&self) -> Result<Meta, ProviderError> {
        tracing::info!("analysis: the relay refused the device's token; registering again");
        self.credentials
            .recover_unauthorized()
            .await
            .map_err(|e| ProviderError::new(ProviderErrorKind::NotAllowed, e.message))?;
        self.adapter()
    }

    /// A paid call's failure, as the run takes it: a used-up credit (with the way on, through the relay), a refused
    /// key or token, or no way to pay stops every paid call after it, with this sentence.
    fn paid_failure(&self, e: ProviderError, relay: bool) -> ProviderError {
        let message = match e.kind {
            ProviderErrorKind::Budget if relay => format!("{} {ADD_KEY}", e.message),
            ProviderErrorKind::Budget
            | ProviderErrorKind::Unauthorized
            | ProviderErrorKind::NotAllowed => e.message.clone(),
            _ => return e,
        };
        self.run.stop_paying(e.kind, &message);
        ProviderError { message, ..e }
    }
}

/// Meta refusing the person's own key.
fn key_refused(e: &ProviderError) -> ProviderError {
    ProviderError {
        kind: ProviderErrorKind::Unauthorized,
        message: KEY_REFUSED.to_owned(),
        detail: format!("Meta refused the key: {}", e.detail),
        retry_after_s: None,
    }
}

fn unauthorized<T>(answer: &Result<T, ProviderError>) -> bool {
    matches!(answer, Err(e) if e.kind == ProviderErrorKind::Unauthorized)
}

impl Transcriber for Calls {
    fn model(&self) -> &str {
        Transcriber::model(&self.first)
    }

    fn max_audio_s(&self) -> u32 {
        self.first.max_audio_s()
    }

    fn max_in_flight(&self) -> usize {
        Transcriber::max_in_flight(&self.first)
    }

    fn estimate_usd(&self, seconds: f64) -> f64 {
        Transcriber::estimate_usd(&self.first, seconds)
    }

    fn transcribe<'a>(
        &'a self,
        wav: Vec<u8>,
        keywords: &'a [String],
    ) -> BoxFuture<'a, Result<Transcription, ProviderError>> {
        Box::pin(async move {
            self.gate(true)?;
            let meta = self.adapter().map_err(|e| self.paid_failure(e, false))?;
            let relay = meta.client().is_relay();
            // Kept for one more try only where a new token can help.
            let again = relay.then(|| wav.clone());
            let mut answer = meta.transcribe(wav, keywords).await;
            // The rule for a 401: relay-bound calls recover, Meta-bound calls in key mode don't. With an access code
            // this call went to the relay with the device's token (register again once, then ask again); with the
            // person's own key it went to Meta, and a refused key is a wrong key.
            if unauthorized(&answer) {
                answer = match again {
                    Some(wav) => match self.recovered().await {
                        Ok(meta) => meta.transcribe(wav, keywords).await,
                        Err(e) => Err(e),
                    },
                    None => Err(key_refused(answer.as_ref().unwrap_err())),
                };
            }
            let heard = answer.map_err(|e| self.paid_failure(e, relay))?;
            self.meter.advance(1);
            Ok(heard)
        })
    }
}

impl Aligner for Calls {
    fn limits(&self) -> AlignLimits {
        self.first.limits()
    }

    /// Free, and always on the relay: never held back by a used-up credit. Word timing always goes to the relay with
    /// the device's token, in key mode too, so a 401 here registers the device again once and asks again in either
    /// mode (relay-bound calls recover; Meta-bound calls in key mode don't).
    fn align<'a>(
        &'a self,
        wav: Vec<u8>,
        turns: &'a [AlignTurn],
    ) -> BoxFuture<'a, Result<Alignment, ProviderError>> {
        Box::pin(async move {
            self.gate(false)?;
            let meta = self.adapter()?;
            let again = wav.clone();
            let mut answer = meta.align(wav, turns).await;
            // Relay-bound in every mode: a refused token is renewed, even with the person's own key.
            if unauthorized(&answer) {
                answer = match self.recovered().await {
                    Ok(meta) => meta.align(again, turns).await,
                    Err(e) => Err(e),
                };
            }
            let aligned = answer?;
            self.meter.advance(turns.len());
            Ok(aligned)
        })
    }
}

impl SceneLogger for Calls {
    fn model(&self) -> &str {
        SceneLogger::model(&self.first)
    }

    /// One chunk at a time: the app sends one video call at a time.
    fn max_in_flight(&self) -> usize {
        1
    }

    fn estimate_usd(&self, seconds: f64, fps: f64) -> f64 {
        SceneLogger::estimate_usd(&self.first, seconds, fps)
    }

    fn watch<'a>(
        &'a self,
        video: &'a Path,
        fps: f64,
    ) -> BoxFuture<'a, Result<SceneAnswer, ProviderError>> {
        Box::pin(async move {
            let _video = self.videos.acquire().await;
            // Checked once the slot is ours: a stop that came while waiting for it counts.
            self.gate(true)?;
            let meta = self.adapter().map_err(|e| self.paid_failure(e, false))?;
            let relay = meta.client().is_relay();
            let mut answer = meta.watch(video, fps).await;
            // Relay-bound (an access code) recovers; Meta-bound with the person's own key is a wrong key.
            if unauthorized(&answer) {
                answer = if relay {
                    match self.recovered().await {
                        Ok(meta) => meta.watch(video, fps).await,
                        Err(e) => Err(e),
                    }
                } else {
                    Err(key_refused(answer.as_ref().unwrap_err()))
                };
            }
            let seen = answer.map_err(|e| self.paid_failure(e, relay))?;
            self.meter.advance(1);
            Ok(seen)
        })
    }
}
