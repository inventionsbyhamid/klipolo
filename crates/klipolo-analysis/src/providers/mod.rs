//! The services the analysis needs, behind interfaces it owns: a `Transcriber` hears speaker turns in a WAV, an
//! `Aligner` times each turn's words, a `SceneLogger` watches a video and logs what happens in it, a `SubjectTracker`
//! finds and follows every person (or other named thing) frame by frame, a `VoiceMatcher` says which person on screen
//! a voice is. Their types and
//! errors are Klipolo's own, so no vendor's type reaches the pipeline: each vendor lives in an adapter that implements
//! the traits and maps its types and errors (`meta` today). Tests use small fakes of the traits.
//!
//! Times are whole milliseconds from the start of the WAV or video sent, as the services count them; the pipeline turns
//! them into the clip's own time.

pub mod images;
pub mod meta;
pub mod meta_chat;
pub mod meta_images;
mod meta_sam;
mod meta_voices;

use std::future::Future;
use std::path::Path;
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

    /// What hearing `seconds` of sound is expected to cost, in US dollars, before any call (the estimate shown before
    /// analysis).
    fn estimate_usd(&self, seconds: f64) -> f64;

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

/// What kind of moment a scene log item is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SceneKind {
    /// A new shot: who or what is on screen now.
    Shot,
    /// Something said that matters (a question, an answer, a key statement): its gist, not a transcript.
    Speech,
    /// A visible action, gesture or demonstration.
    Action,
    /// A reaction: laughing, smiling, clapping, surprise.
    Reaction,
    /// Text that appears on screen: a title, caption, lower third or sign.
    Text,
    /// The talk turns to another topic.
    Topic,
    Other,
}

/// One moment a scene logger saw, timed in the video it was sent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneMoment {
    pub start_ms: u64,
    /// At or after `start_ms` (equal for an instant, such as a cut).
    pub end_ms: u64,
    pub kind: SceneKind,
    /// Who it is about ("woman in a blue shirt", a name shown on screen); empty when no one in particular.
    pub who: String,
    /// What happens, in one line.
    pub text: String,
    /// The text on screen during it, as written; empty for none.
    pub on_screen_text: String,
}

/// A scene logger's answer for one video.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneAnswer {
    /// In time order as the service gave them.
    pub moments: Vec<SceneMoment>,
    /// What the call cost, in US dollars.
    pub cost_usd: f64,
}

/// A video (its picture and sound) to a timestamped log of what happens in it.
pub trait SceneLogger: Send + Sync {
    /// The model's id, kept in the log (`SceneLog::model`).
    fn model(&self) -> &str;

    /// Videos it watches at once for one clip. A provider that limits a whole device keeps that limit itself, across
    /// every caller.
    fn max_in_flight(&self) -> usize;

    /// What watching `seconds` of video sampled `fps` times a second is expected to cost, in US dollars, before any
    /// call (the estimate shown before analysis).
    fn estimate_usd(&self, seconds: f64, fps: f64) -> f64;

    /// The moments in `video` (an MP4 of at most ten minutes, with its sound), its picture sampled `fps` times a
    /// second (at most 2).
    fn watch<'a>(
        &'a self,
        video: &'a Path,
        fps: f64,
    ) -> BoxFuture<'a, Result<SceneAnswer, ProviderError>>;
}

/// One thing a tracker found on one frame: its box as parts of the picture's width and height (left, top, right,
/// bottom, the picture as the file shows it).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackedBox {
    /// The same thing keeps its id from frame to frame (mostly: one that leaves and comes back may get a new one).
    pub id: u32,
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    /// 0 to 1, when the tracker says how sure it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
}

impl TrackedBox {
    pub fn width(&self) -> f64 {
        (self.right - self.left).max(0.0)
    }

    pub fn height(&self) -> f64 {
        (self.bottom - self.top).max(0.0)
    }

    pub fn centre_x(&self) -> f64 {
        (self.left + self.right) / 2.0
    }
}

/// Everything a tracker found on one frame of the video it was sent (frame 0 is its first).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackedFrame {
    pub frame: u32,
    pub boxes: Vec<TrackedBox>,
}

/// A tracker's answer for one video.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tracking {
    /// In frame order; a frame with nothing found may be missing.
    pub frames: Vec<TrackedFrame>,
    /// The frames the call was billed for.
    pub frames_billed: u64,
    /// What the call cost, in US dollars.
    pub cost_usd: f64,
    /// The answer stopped part-way: frames after the last one here weren't answered (though billed).
    #[serde(default)]
    pub cut_off: bool,
}

/// A video to every match of a short noun phrase ("person", "man in red"), frame by frame.
pub trait SubjectTracker: Send + Sync {
    /// The model's id, kept with what it found.
    fn model(&self) -> &str;

    /// The most frames one video may have.
    fn max_frames(&self) -> u64;

    /// What tracking a video of `frames` frames is expected to cost, in US dollars, before any call (the price shown
    /// before it runs).
    fn estimate_usd(&self, frames: u64) -> f64;

    /// Every `phrase` in `video` (an MP4 of at most `max_frames` frames), on each of its frames.
    fn track<'a>(
        &'a self,
        video: &'a Path,
        phrase: &'a str,
    ) -> BoxFuture<'a, Result<Tracking, ProviderError>>;
}

/// Where someone is on screen: parts of the picture's width and height from its top left.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenBox {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

/// Whose a voice is: a person on screen (from 1 at the left), nobody on screen, or an answer that didn't say.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Heard {
    Person { person: u8 },
    Nobody,
    Unclear,
}

/// A matcher's answer for one video.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceAnswer {
    pub heard: Heard,
    /// What the call cost, in US dollars.
    pub cost_usd: f64,
}

/// A few seconds of video in which one voice speaks, and the people on screen, to which of them is speaking.
pub trait VoiceMatcher: Send + Sync {
    /// The model's id, kept with its answers.
    fn model(&self) -> &str;

    /// What asking about `seconds` of video costs, in US dollars, before any call.
    fn estimate_usd(&self, seconds: f64) -> f64;

    /// Which of `people` (numbered from 1, left to right) speaks in `video` (an MP4 with its sound, at most 2 frames a
    /// second), or nobody on screen.
    fn who_speaks<'a>(
        &'a self,
        video: &'a Path,
        people: &'a [ScreenBox],
    ) -> BoxFuture<'a, Result<VoiceAnswer, ProviderError>>;
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
    /// The key or token was not accepted (401). Through Klipolo's relay a new token may fix it (the app registers the
    /// device again); a person's own key that isn't accepted is wrong.
    Unauthorized,
    /// The key, token or code may not make this call (403): a code that was turned off, a model the key can't use.
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
