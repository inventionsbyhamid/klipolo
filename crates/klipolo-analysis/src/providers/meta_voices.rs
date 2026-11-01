//! Muse Spark as the `VoiceMatcher`, through the Meta adapter's client and slots: a few seconds of the clip (picture
//! and sound) are uploaded, Spark is told where each person on screen is (numbered from the left) and asked, with a
//! strict schema, whose voice it hears, and the upload is deleted on every path, as the scene log's are. A video call
//! holds the device's video slot from its upload to its answer. The answer is paid for, so one that doesn't read, or
//! names someone not listed, is `Unclear`, never an error.

use std::fmt::Write as _;
use std::path::Path;

use klipolo_meta::{ResponseOut, pricing};
use serde::Deserialize;
use serde_json::{Value, json};

use super::meta::{Meta, SCENE_MODEL, Upload, upload};
use super::{BoxFuture, Heard, ProviderError, ScreenBox, VoiceAnswer, VoiceMatcher, retry_busy};

/// The frames a second Spark watches.
pub const VOICE_FPS: f64 = 2.0;
/// About what the question and its schema add to a call's input tokens.
pub const VOICE_PROMPT_TOKENS: u64 = 300;
/// Output tokens an answer is expected to use, its low reasoning included. An answer may use up to
/// `VOICE_MAX_OUTPUT_TOKENS`: one that long costs about three times the window's estimate (the scene log is priced the
/// same way, by what an answer usually takes).
pub const VOICE_OUTPUT_TOKENS: f64 = 300.0;
/// An answer's output tokens at most.
pub const VOICE_MAX_OUTPUT_TOKENS: u64 = 2_048;

impl VoiceMatcher for Meta {
    fn model(&self) -> &str {
        SCENE_MODEL
    }

    /// The scene log's formula for a few seconds: their frames and sound, the question, and a short answer.
    fn estimate_usd(&self, seconds: f64) -> f64 {
        let input = pricing::spark_video_tokens(seconds, VOICE_FPS) + VOICE_PROMPT_TOKENS;
        input as f64 * pricing::SPARK_INPUT_USD + VOICE_OUTPUT_TOKENS * pricing::SPARK_OUTPUT_USD
    }

    fn who_speaks<'a>(
        &'a self,
        video: &'a Path,
        people: &'a [ScreenBox],
    ) -> BoxFuture<'a, Result<VoiceAnswer, ProviderError>> {
        Box::pin(async move {
            let _video = self.videos.acquire().await;
            let id = retry_busy(|| self.call(upload(self.client(), video))).await?;
            let upload = Upload {
                meta: self.clone(),
                id: Some(id),
            };
            let body = voice_request(upload.id(), people);
            let answer = retry_busy(|| self.call(self.client().respond(body.clone()))).await;
            upload.delete().await;
            Ok(voice_answer(&answer?, people.len()))
        })
    }
}

const VOICE_PROMPT: &str = "You are the step of a video editor that finds who is speaking. This video is a few \
seconds long, with its sound, and one voice speaks in it. The people on screen are numbered from the left; each box \
gives parts of the picture's width and height from its top left:";

const VOICE_ASK: &str = "Watch whose mouth moves with the voice. Answer with that person's number, or none when the \
voice belongs to nobody on screen.";

/// The question: the people's boxes, the window by its file id at `VOICE_FPS`, a strict answer, low reasoning.
fn voice_request(file_id: &str, people: &[ScreenBox]) -> Value {
    let mut text = VOICE_PROMPT.to_owned();
    for (i, b) in people.iter().enumerate() {
        let _ = write!(
            text,
            "\n{}: left {:.2}, top {:.2}, right {:.2}, bottom {:.2}",
            i + 1,
            b.left,
            b.top,
            b.right,
            b.bottom
        );
    }
    text.push('\n');
    text.push_str(VOICE_ASK);
    let mut choices: Vec<String> = (1..=people.len()).map(|n| n.to_string()).collect();
    choices.push("none".to_owned());
    json!({
        "model": SCENE_MODEL,
        "input": [{
            "type": "message",
            "role": "user",
            "content": [
                {"type": "input_text", "text": text},
                {"type": "input_video", "file_id": file_id, "fps": VOICE_FPS},
            ],
        }],
        "text": {"format": {
            "type": "json_schema",
            "name": "speaker",
            "schema": {
                "type": "object",
                "properties": {"speaker": {"type": "string", "enum": choices}},
                "required": ["speaker"],
                "additionalProperties": false,
            },
            "strict": true,
        }},
        "reasoning": {"effort": "low"},
        "max_output_tokens": VOICE_MAX_OUTPUT_TOKENS,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireAnswer {
    speaker: String,
}

/// Spark's answer for `people` people, priced by its usage.
fn voice_answer(out: &ResponseOut, people: usize) -> VoiceAnswer {
    let text = out.output_text.as_deref().unwrap_or_default();
    let heard = match serde_json::from_str::<WireAnswer>(text) {
        Ok(a) if a.speaker == "none" => Heard::Nobody,
        Ok(a) => match a.speaker.parse::<u8>() {
            Ok(n) if n >= 1 && usize::from(n) <= people => Heard::Person { person: n },
            _ => Heard::Unclear,
        },
        Err(_) => Heard::Unclear,
    };
    VoiceAnswer {
        heard,
        cost_usd: pricing::spark_usd(out.usage),
    }
}
