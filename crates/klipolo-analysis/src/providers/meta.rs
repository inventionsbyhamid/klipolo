//! Meta behind the analysis interfaces: Voice Transcribe as the `Transcriber`, the relay's MMS aligner as the
//! `Aligner` and Muse Spark as the `SceneLogger`, all through one `klipolo_meta::MetaClient`. Whether that client calls
//! Meta with the person's key or goes through the relay with a device token is the client's business, never the
//! caller's. Meta's types and errors stop here.
//!
//! The relay counts one device's requests at once (4 Meta calls, 2 alignments, one video call) and answers more with
//! 429 `busy`. So the adapter keeps one device's slots, shared by its clones: make one `Meta` per app and hand clones
//! to every clip and feature, and every request waits for a slot before it is sent.
//!
//! **Scene log:** each chunk of the clip's small copy is uploaded (Files API), watched by Spark through `input_video`
//! by its file id (strict `json_schema` output, reasoning effort low, at most 16384 output tokens), and its upload is
//! deleted on every path: answered, failed, or dropped part-way (a delete is sent from a task of its own then; Meta
//! also expires uploads after a day). A video call holds the device's video slot from its upload to its answer.

use std::future::Future;
use std::path::Path;
use std::sync::Arc;

use klipolo_meta::{AsrResult, MetaClient, MetaError, ResponseOut, TRANSCRIBE_MODEL, pricing};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::Semaphore;

use super::{
    AlignLimits, AlignTurn, Aligner, Alignment, BoxFuture, ProviderError, ProviderErrorKind,
    SceneAnswer, SceneKind, SceneLogger, SceneMoment, SpeakerTurn, TimedWord, Transcriber,
    Transcription, retry_busy,
};

/// The relay's error code for a device's requests-at-once limit (`Retry-After` 1 s for Meta calls, 30 s for
/// alignment): a busy moment, not a used-up allowance.
pub const RELAY_BUSY: &str = "busy";

/// Meta calls at once through the relay: it takes 4 from one device, and one is left for chat, which must never wait
/// behind analysis.
pub const RELAY_META_CALLS: usize = 3;
/// Meta calls at once with the person's own key.
pub const DIRECT_META_CALLS: usize = 4;
/// Align requests at once, in both modes (alignment always runs on the relay, 2 at once per device).
pub const ALIGN_CALLS: usize = 2;

/// Video calls at once through the relay: one per device (a second is answered `busy`).
pub const RELAY_VIDEO_CALLS: usize = 1;
/// Video calls at once with the person's own key: two uploads share the connection.
pub const DIRECT_VIDEO_CALLS: usize = 2;

/// Voice Transcribe's longest request.
pub const TRANSCRIBE_MAX_S: u32 = 600;

/// The model that watches clips for the scene log.
pub const SCENE_MODEL: &str = "muse-spark-1.3";
/// A scene log call's output tokens at most, reasoning included: the relay's cap. Only the tokens used are billed, and a
/// busy ten minutes (about 80 moments and the reasoning) needs room.
pub const SCENE_MAX_OUTPUT_TOKENS: u64 = 16_384;
/// About what the scene log's prompt and schema add to a call's input tokens.
pub const SCENE_PROMPT_TOKENS: u64 = 450;
/// Output tokens a scene log call is expected to use: a base (reasoning) and some for each second watched (its
/// moments). Clip A's 155 s used 2,795 at default reasoning; this gives 2,740.
pub const SCENE_OUTPUT_TOKENS_BASE: f64 = 1_500.0;
pub const SCENE_OUTPUT_TOKENS_PER_S: f64 = 8.0;

/// The relay's `/v1/align`: ten minutes and 300 turns a request (its turns JSON may be 1 MiB; 128 KiB is plenty and
/// leaves room), a turn read with 0.25 s on each side, padded turns adding up to at most the audio plus 1 s each, and
/// two requests at once per device (a third is refused as busy). About five minutes a request keeps each answer
/// within a few minutes on the relay's CPUs.
pub const RELAY_ALIGN_LIMITS: AlignLimits = AlignLimits {
    max_audio_ms: 600_000,
    preferred_audio_ms: 300_000,
    max_turns: 300,
    max_text_bytes: 128 * 1024,
    pad_ms: 250,
    overlap_per_turn_ms: 1_000,
    max_in_flight: ALIGN_CALLS,
};

/// Meta's speech services through one client, within one device's slots. Its clones share the client and the slots.
#[derive(Clone, Debug)]
pub struct Meta {
    client: MetaClient,
    /// Meta calls at once: `RELAY_META_CALLS` through the relay, `DIRECT_META_CALLS` with a key.
    calls: Arc<Semaphore>,
    calls_at_once: usize,
    /// Align requests at once: `ALIGN_CALLS`.
    aligns: Arc<Semaphore>,
    /// Video calls at once (upload to answer): `RELAY_VIDEO_CALLS` through the relay, `DIRECT_VIDEO_CALLS` with a key.
    videos: Arc<Semaphore>,
    videos_at_once: usize,
}

impl Meta {
    /// One device's adapter. Make one per app and share it (clones share its slots); two made from the same device's
    /// client would each fill the device's slots.
    pub fn new(client: MetaClient) -> Self {
        let (calls_at_once, videos_at_once) = if client.is_relay() {
            (RELAY_META_CALLS, RELAY_VIDEO_CALLS)
        } else {
            (DIRECT_META_CALLS, DIRECT_VIDEO_CALLS)
        };
        Self {
            client,
            calls: Arc::new(Semaphore::new(calls_at_once)),
            calls_at_once,
            aligns: Arc::new(Semaphore::new(ALIGN_CALLS)),
            videos: Arc::new(Semaphore::new(videos_at_once)),
            videos_at_once,
        }
    }

    /// The client, for Meta calls this adapter doesn't make itself (a scene log's upload and response, say): send them
    /// through `call`, so they take this device's slots too.
    pub fn client(&self) -> &MetaClient {
        &self.client
    }

    /// One Meta call in this device's slots, its error in Klipolo's terms. The call starts once a slot is free and
    /// holds it until it is answered.
    pub async fn call<T>(
        &self,
        call: impl Future<Output = Result<T, MetaError>>,
    ) -> Result<T, ProviderError> {
        let _slot = self.calls.acquire().await;
        call.await.map_err(provider_error)
    }
}

impl Transcriber for Meta {
    fn model(&self) -> &str {
        TRANSCRIBE_MODEL
    }

    fn max_audio_s(&self) -> u32 {
        TRANSCRIBE_MAX_S
    }

    fn max_in_flight(&self) -> usize {
        self.calls_at_once
    }

    fn transcribe<'a>(
        &'a self,
        wav: Vec<u8>,
        keywords: &'a [String],
    ) -> BoxFuture<'a, Result<Transcription, ProviderError>> {
        Box::pin(async move {
            let heard = self.call(self.client.transcribe(wav, keywords)).await?;
            Ok(transcription(heard))
        })
    }
}

impl Aligner for Meta {
    fn limits(&self) -> AlignLimits {
        RELAY_ALIGN_LIMITS
    }

    /// Free to the person: the relay counts the minutes against the device's daily allowance instead.
    fn align<'a>(
        &'a self,
        wav: Vec<u8>,
        turns: &'a [AlignTurn],
    ) -> BoxFuture<'a, Result<Alignment, ProviderError>> {
        Box::pin(async move {
            let turns: Vec<klipolo_meta::AlignTurn> = turns
                .iter()
                .map(|t| klipolo_meta::AlignTurn {
                    start_ms: t.start_ms,
                    end_ms: t.end_ms,
                    text: t.text.clone(),
                })
                .collect();
            let aligned = {
                let _slot = self.aligns.acquire().await;
                self.client
                    .align(wav, &turns)
                    .await
                    .map_err(provider_error)?
            };
            Ok(Alignment {
                words: aligned
                    .words
                    .into_iter()
                    .map(|w| TimedWord {
                        text: w.text,
                        start_ms: w.start_ms,
                        end_ms: w.end_ms,
                        score: w.score,
                        turn: w.turn,
                    })
                    .collect(),
                skipped: aligned.skipped,
                cost_usd: 0.0,
            })
        })
    }
}

impl SceneLogger for Meta {
    fn model(&self) -> &str {
        SCENE_MODEL
    }

    fn max_in_flight(&self) -> usize {
        self.videos_at_once
    }

    /// The plan's formula: 131 tokens a sampled frame at 640 × 360, 10 a second of sound, the prompt, and the output
    /// expected (`SCENE_OUTPUT_TOKENS_*`), at Spark's list prices.
    fn estimate_usd(&self, seconds: f64, fps: f64) -> f64 {
        let input = pricing::spark_video_tokens(seconds, fps) + SCENE_PROMPT_TOKENS;
        let output = SCENE_OUTPUT_TOKENS_BASE + SCENE_OUTPUT_TOKENS_PER_S * seconds.max(0.0);
        input as f64 * pricing::SPARK_INPUT_USD + output * pricing::SPARK_OUTPUT_USD
    }

    fn watch<'a>(
        &'a self,
        video: &'a Path,
        fps: f64,
    ) -> BoxFuture<'a, Result<SceneAnswer, ProviderError>> {
        Box::pin(async move {
            // The device's video slot, from the upload to the answer.
            let _video = self.videos.acquire().await;
            let id = retry_busy(|| self.call(upload(&self.client, video))).await?;
            let upload = Upload {
                meta: self.clone(),
                id: Some(id),
            };
            let body = scene_request(upload.id(), fps);
            let answer = retry_busy(|| self.call(self.client.respond(body.clone()))).await;
            upload.delete().await;
            scene_answer(answer?)
        })
    }
}

/// Uploads a chunk of the clip's small copy. A file that can't be read is named as what it is, not by its temporary
/// name.
async fn upload(client: &MetaClient, video: &Path) -> Result<String, MetaError> {
    client
        .upload_file(video, "video/mp4")
        .await
        .map_err(|e| match e {
            MetaError::LocalFile { .. } => MetaError::LocalFile {
                name: "this clip's small copy".into(),
            },
            e => e,
        })
}

/// An uploaded file, deleted when the call is done (`delete`) or, should the call be dropped part-way, from a task of
/// its own. It owns the id from the moment the upload returns it, with no await in between. A call dropped while its
/// file is still going up never learns the id, so nothing can delete that file; neither can a failed delete be tried
/// again: Meta expires uploads after a day.
struct Upload {
    meta: Meta,
    id: Option<String>,
}

impl Upload {
    fn id(&self) -> &str {
        self.id.as_deref().unwrap_or_default()
    }

    /// Deletes the file. The id is kept until the delete is answered: dropped before then (waiting for a slot, or for
    /// the answer), the guard sends the delete again (a file already gone counts as deleted).
    async fn delete(mut self) {
        if let Some(id) = self.id.clone() {
            let _ = self.meta.call(self.meta.client.delete_file(&id)).await;
            self.id = None;
        }
    }
}

impl Drop for Upload {
    fn drop(&mut self) {
        let Some(id) = self.id.take() else {
            return;
        };
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let meta = self.meta.clone();
            runtime.spawn(async move {
                let _ = meta.call(meta.client.delete_file(&id)).await;
            });
        }
    }
}

/// What Spark is asked: the log's prompt, the video by its file id, a strict schema, low reasoning. Only fields the
/// relay passes on.
fn scene_request(file_id: &str, fps: f64) -> Value {
    json!({
        "model": SCENE_MODEL,
        "input": [{
            "type": "message",
            "role": "user",
            "content": [
                {"type": "input_text", "text": SCENE_PROMPT},
                {"type": "input_video", "file_id": file_id, "fps": fps},
            ],
        }],
        "text": {"format": {
            "type": "json_schema",
            "name": "scene_log",
            "schema": scene_schema(),
            "strict": true,
        }},
        "reasoning": {"effort": "low"},
        "max_output_tokens": SCENE_MAX_OUTPUT_TOKENS,
    })
}

const SCENE_PROMPT: &str = "You are the analysis step of a video editor. Watch and listen to this video and log what \
happens, in time order, so the editor can find moments later. Log every shot change (kind shot: who or what is on \
screen now), each thing people say that matters, such as a question, an answer or a key statement (kind speech: its \
gist in a few words, not a transcript), visible actions, gestures and demonstrations (kind action), reactions such as \
laughing, smiling, clapping or surprise (kind reaction), text that appears on screen, such as a title, caption, lower \
third or sign (kind text), and changes of topic (kind topic). For each moment give start_sec and end_sec measured from \
the start of this video in seconds (decimals allowed), who (a short description of the person or people, such as \
'woman in a blue shirt', or a name shown on screen; empty when no one in particular), a one-line description, and \
on_screen_text (the exact text visible on screen during the moment, or empty). Be precise about times.";

/// The kinds Spark may give (`SceneKind`, in its words).
const SCENE_KINDS: [&str; 7] = [
    "shot", "speech", "action", "reaction", "text", "topic", "other",
];

fn scene_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "moments": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "start_sec": {"type": "number"},
                        "end_sec": {"type": "number"},
                        "kind": {"type": "string", "enum": SCENE_KINDS},
                        "who": {"type": "string"},
                        "description": {"type": "string"},
                        "on_screen_text": {"type": "string"},
                    },
                    "required": ["start_sec", "end_sec", "kind", "who", "description", "on_screen_text"],
                    "additionalProperties": false,
                },
            },
        },
        "required": ["moments"],
        "additionalProperties": false,
    })
}

/// Spark's log as it writes it.
#[derive(Deserialize)]
struct WireLog {
    moments: Vec<WireMoment>,
}

#[derive(Deserialize)]
struct WireMoment {
    start_sec: f64,
    end_sec: f64,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    who: String,
    #[serde(default)]
    description: String,
    /// Absent from answers to the spike's schema.
    #[serde(default)]
    on_screen_text: String,
}

/// The longest text kept from one field.
const MAX_FIELD_CHARS: usize = 400;

/// Spark's answer as moments, priced by its usage.
fn scene_answer(out: ResponseOut) -> Result<SceneAnswer, ProviderError> {
    let text = out.output_text.as_deref().unwrap_or_default();
    let log: WireLog = serde_json::from_str(text).map_err(|e| {
        if out.status.as_deref() == Some("incomplete") {
            ProviderError {
                detail: format!("scene log cut off: {e}"),
                ..ProviderError::new(
                    ProviderErrorKind::Failed,
                    "Meta's scene log was cut off. Try again.",
                )
            }
        } else {
            provider_error(MetaError::Decode(format!("scene log: {e}")))
        }
    })?;
    let moments = log
        .moments
        .into_iter()
        .filter(|m| m.start_sec.is_finite() && m.end_sec.is_finite())
        .map(|m| {
            let ms = |sec: f64| (sec.max(0.0) * 1000.0).round() as u64;
            let start_ms = ms(m.start_sec);
            SceneMoment {
                start_ms,
                end_ms: ms(m.end_sec).max(start_ms),
                kind: scene_kind(&m.kind),
                who: tidy(&m.who),
                text: tidy(&m.description),
                on_screen_text: tidy(&m.on_screen_text),
            }
        })
        .collect();
    Ok(SceneAnswer {
        moments,
        cost_usd: pricing::spark_usd(out.usage),
    })
}

/// A kind in Spark's words: this schema's, or the spike's older ones (still read, for saved answers).
fn scene_kind(kind: &str) -> SceneKind {
    match kind {
        "shot" | "scene_cut" | "b_roll" => SceneKind::Shot,
        "speech" | "question" | "answer" => SceneKind::Speech,
        "action" | "gesture" | "demonstration" => SceneKind::Action,
        "reaction" | "laugh" => SceneKind::Reaction,
        "text" | "on_screen_text" => SceneKind::Text,
        "topic" | "topic_change" => SceneKind::Topic,
        _ => SceneKind::Other,
    }
}

/// One line, its spaces collapsed, at most `MAX_FIELD_CHARS`.
fn tidy(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match line.char_indices().nth(MAX_FIELD_CHARS) {
        Some((cut, _)) => line[..cut].to_owned(),
        None => line,
    }
}

/// Voice Transcribe's answer, priced by the audio it measured.
fn transcription(heard: AsrResult) -> Transcription {
    Transcription {
        duration_ms: heard.duration_ms,
        cost_usd: pricing::asr_usd(heard.duration_ms),
        turns: heard
            .turns
            .into_iter()
            .map(|t| SpeakerTurn {
                start_ms: t.start_ms,
                end_ms: t.end_ms,
                text: t.text,
                speaker: t.speaker,
            })
            .collect(),
    }
}

/// A Meta client error in Klipolo's terms: its sentence for the person, its `Display` (already redacted) for logs.
/// A 429 is `Busy` when the relay says `busy` or gives no code at all (Traefik's rate limit; wait and try again); any
/// other (the relay's daily `align_quota`, Meta's own rate limit) is `RateLimited`, a stop.
pub fn provider_error(e: MetaError) -> ProviderError {
    let (kind, retry_after_s) = match &e {
        MetaError::Budget { .. } => (ProviderErrorKind::Budget, None),
        MetaError::RateLimited { retry_after, .. } if e.is_busy() => {
            (ProviderErrorKind::Busy, *retry_after)
        }
        MetaError::RateLimited { retry_after, .. } => {
            (ProviderErrorKind::RateLimited, *retry_after)
        }
        MetaError::Unauthorized | MetaError::NotAllowed { .. } => {
            (ProviderErrorKind::NotAllowed, None)
        }
        MetaError::Overloaded { .. } | MetaError::Network(_) | MetaError::AlignUnavailable => {
            (ProviderErrorKind::Unavailable, None)
        }
        MetaError::BadRequest { .. } | MetaError::Decode(_) | MetaError::LocalFile { .. } => {
            (ProviderErrorKind::Failed, None)
        }
        MetaError::Refused { .. } => (ProviderErrorKind::Failed, None),
        MetaError::TimedOut { .. } | MetaError::NotConfigured => {
            (ProviderErrorKind::Unavailable, None)
        }
    };
    ProviderError {
        kind,
        message: e.user_message(),
        detail: e.to_string(),
        retry_after_s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ProviderErrorKind as Kind;

    #[test]
    fn the_relay_leaves_one_of_a_devices_four_meta_calls_for_chat() {
        let relay = Meta::new(MetaClient::relay("relay-token".to_owned().into()));
        let direct = Meta::new(MetaClient::direct("sk-key".to_owned().into()));
        assert_eq!(Transcriber::max_in_flight(&relay), 3);
        assert_eq!(Transcriber::max_in_flight(&direct), 4);
        assert_eq!(relay.limits().max_in_flight, 2);
        assert_eq!(direct.limits().max_in_flight, 2);
    }

    #[test]
    fn meta_errors_keep_their_sentence_and_say_when_to_stop() {
        let quota =
            "This device has used today's 120 minutes of word timing. It resets at midnight UTC.";
        let cases = [
            (
                MetaError::Budget {
                    message: "This device used its Klipolo credit.".into(),
                },
                Kind::Budget,
                true,
            ),
            (
                MetaError::RateLimited {
                    retry_after: Some(50_400),
                    code: Some("align_quota".into()),
                    message: Some(quota.into()),
                },
                Kind::RateLimited,
                true,
            ),
            (
                MetaError::RateLimited {
                    retry_after: Some(1),
                    code: Some("busy".into()),
                    message: Some("Too many requests at once from this device.".into()),
                },
                Kind::Busy,
                false,
            ),
            (
                MetaError::RateLimited {
                    retry_after: Some(3),
                    code: Some("rate_limit_exceeded".into()),
                    message: None,
                },
                Kind::RateLimited,
                true,
            ),
            (MetaError::Unauthorized, Kind::NotAllowed, false),
            (
                MetaError::NotAllowed { message: None },
                Kind::NotAllowed,
                false,
            ),
            (
                MetaError::Overloaded { message: None },
                Kind::Unavailable,
                false,
            ),
            (
                MetaError::Network("refused".into()),
                Kind::Unavailable,
                false,
            ),
            (MetaError::AlignUnavailable, Kind::Unavailable, false),
            (
                MetaError::BadRequest {
                    message: "Turn 3 is not valid.".into(),
                },
                Kind::Failed,
                false,
            ),
            (MetaError::Decode("eof".into()), Kind::Failed, false),
            (
                MetaError::LocalFile {
                    name: "a.wav".into(),
                },
                Kind::Failed,
                false,
            ),
        ];
        for (meta, kind, stop) in cases {
            let e = provider_error(meta.clone());
            assert_eq!(e.kind, kind, "{meta:?}");
            assert_eq!(e.is_stop(), stop, "{meta:?}");
            assert_eq!(e.user_message(), meta.user_message(), "{meta:?}");
            assert_eq!(e.detail, meta.to_string(), "{meta:?}");
        }
        let limited = provider_error(MetaError::RateLimited {
            retry_after: Some(50_400),
            code: Some("align_quota".into()),
            message: Some(quota.into()),
        });
        assert_eq!(limited.retry_after_s, Some(50_400));
        assert_eq!(limited.user_message(), quota);
    }
}
