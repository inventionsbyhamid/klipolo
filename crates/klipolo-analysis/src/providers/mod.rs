//! The speech services the analysis needs, behind interfaces it owns: a `Transcriber` hears speaker turns in a WAV, an
//! `Aligner` times each turn's words. Their types and errors are Klipolo's own, so no vendor's type reaches the
//! pipeline: each vendor lives in an adapter that implements the traits and maps its types and errors (`meta` today).
//! Tests use small fakes of the traits.
//!
//! Times are whole milliseconds from the start of the WAV sent, as the services count them; the pipeline turns them
//! into the clip's sound time.

pub mod meta;

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Attempts per request while the service answers `Busy`, the first one included.
pub const BUSY_ATTEMPTS: u32 = 3;
/// The longest wait before a busy request is sent again.
pub const BUSY_WAIT_MAX_S: u64 = 30;

/// A boxed future that may move between threads, so the traits work as `dyn` objects.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What a transcriber heard in one WAV.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transcription {
    /// The audio's length as the service measured it.
    pub duration_ms: u64,
    pub turns: Vec<SpeakerTurn>,
    /// What the call cost, in US dollars.
    pub cost_usd: f64,
}

/// One speaker's turn: often a whole answer, not a sentence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeakerTurn {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    /// The speaker's label ("A", "B" ...), meaningful within one request only.
    pub speaker: Option<String>,
}

/// One turn to align: its text and where it sits in the WAV sent with it. It serializes as the request's JSON
/// (`{"startMs","endMs","text"}`), so the pipeline can measure a request against `AlignLimits::max_text_bytes`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlignTurn {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

/// One word with its time in the WAV sent. A `score` near 0 means the audio doesn't hold the word (often a repeat the
/// transcriber invented).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimedWord {
    /// As written in the turn, with its punctuation.
    pub text: String,
    pub start_ms: u64,
    pub end_ms: u64,
    /// 0 to 1.
    pub score: f64,
    /// Its turn's index in the request.
    pub turn: usize,
}

/// An aligner's answer: the words it timed, and the turns it skipped with its reason.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Alignment {
    pub words: Vec<TimedWord>,
    /// (turn index in the request, reason).
    pub skipped: Vec<(usize, String)>,
    /// What the call cost, in US dollars.
    pub cost_usd: f64,
}

/// What one align request may hold, and how many may run at once: each aligner's own (`Aligner::limits`), with no
/// default here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AlignLimits {
    /// The longest WAV one request takes.
    pub max_audio_ms: u64,
    /// Requests are kept to about this much audio (sooner answers, shorter waits); one long turn still goes alone, up
    /// to `max_audio_ms`.
    pub preferred_audio_ms: u64,
    /// The most turns in one request.
    pub max_turns: usize,
    /// The most bytes of turns JSON in one request.
    pub max_text_bytes: usize,
    /// The audio the service reads on each side of a turn.
    pub pad_ms: u64,
    /// The turns' padded spans may add up to the audio's length plus this much per turn: overlapping turns beyond it
    /// are refused.
    pub overlap_per_turn_ms: u64,
    /// Requests at once.
    pub max_in_flight: usize,
}

/// Speech to speaker turns.
pub trait Transcriber: Send + Sync {
    /// The model's id, kept in the transcript (`Transcript::model`).
    fn model(&self) -> &str;

    /// The longest WAV one call takes, in seconds.
    fn max_audio_s(&self) -> u32;

    /// Calls at once for one clip. A provider that limits a whole device (several clips, other features) keeps that
    /// limit itself, across every caller.
    fn max_in_flight(&self) -> usize;

    /// The turns heard in `wav` (mono 16-bit PCM at 16 kHz, at most `max_audio_s`). `keywords` (names, jargon) bias
    /// the spelling; empty for none.
    fn transcribe<'a>(
        &'a self,
        wav: Vec<u8>,
        keywords: &'a [String],
    ) -> BoxFuture<'a, Result<Transcription, ProviderError>>;
}

/// Turns to word times.
pub trait Aligner: Send + Sync {
    /// What one request may hold.
    fn limits(&self) -> AlignLimits;

    /// Word times for `turns` of `wav` (mono 16-bit PCM at 16 kHz), within `limits`.
    fn align<'a>(
        &'a self,
        wav: Vec<u8>,
        turns: &'a [AlignTurn],
    ) -> BoxFuture<'a, Result<Alignment, ProviderError>>;
}

/// What kind of failure a service reported, which decides what the pipeline does next.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderErrorKind {
    /// The credit or budget for these calls is used up: stop; asking again won't help until the person acts.
    Budget,
    /// Too many requests, or a daily allowance used up: stop for now (`ProviderError::retry_after_s` says when).
    RateLimited,
    /// Busy with this device's other requests for a moment: wait `ProviderError::retry_after_s` and send the request
    /// again (`retry_busy`). Not a stop.
    Busy,
    /// The key, token or code was refused.
    NotAllowed,
    /// The service is missing or not answering (none set up, down, unreachable).
    Unavailable,
    /// This request failed (refused as sent, or its answer unreadable); others may still work.
    Failed,
}

/// A service's failure, in Klipolo's terms. `message` is the plain sentence to show the person; `detail` is for logs.
/// Neither holds a key or token: adapters build them from text that is already safe.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{detail}")]
pub struct ProviderError {
    pub kind: ProviderErrorKind,
    pub message: String,
    pub detail: String,
    /// When to try again, in seconds, if the service said.
    pub retry_after_s: Option<u64>,
}

impl ProviderError {
    /// An error whose log detail is its message.
    pub fn new(kind: ProviderErrorKind, message: impl Into<String>) -> Self {
        let message = message.into();
        Self {
            kind,
            detail: message.clone(),
            message,
            retry_after_s: None,
        }
    }

    /// The plain sentence to show the person.
    pub fn user_message(&self) -> &str {
        &self.message
    }

    /// A budget or quota stop: the step stops here, and nothing should ask again in a loop.
    pub fn is_stop(&self) -> bool {
        matches!(
            self.kind,
            ProviderErrorKind::Budget | ProviderErrorKind::RateLimited
        )
    }
}

/// `call`'s answer, asked again while it is `Busy`: after the wait the service gave (at least 1 s, at most
/// `BUSY_WAIT_MAX_S`), `BUSY_ATTEMPTS` attempts in all, then its last error. Any other answer returns at once: a
/// used-up allowance or credit is never asked again. `call` makes its request afresh each time.
pub async fn retry_busy<T, F, Fut>(mut call: F) -> Result<T, ProviderError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, ProviderError>>,
{
    let mut attempt = 1;
    loop {
        match call().await {
            Err(e) if e.kind == ProviderErrorKind::Busy && attempt < BUSY_ATTEMPTS => {
                let wait = e.retry_after_s.unwrap_or(1).clamp(1, BUSY_WAIT_MAX_S);
                tokio::time::sleep(Duration::from_secs(wait)).await;
                attempt += 1;
            }
            answer => return answer,
        }
    }
}
