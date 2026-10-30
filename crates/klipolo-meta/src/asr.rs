//! Speech to text: `POST /asr/transcribe` with Voice Transcribe in diarization mode.

use reqwest::multipart::{Form, Part};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{MetaClient, MetaError};

/// Meta's speech to text model.
pub const TRANSCRIBE_MODEL: &str = "muse-voice-transcribe-1.0";

/// Meta's transcript of one WAV: speaker turns, times in milliseconds from
/// the start of that WAV. Serializes in the API's own shape.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AsrResult {
    /// The audio's length as Meta measured it (what it bills).
    #[serde(rename = "audioDurationMs", default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub turns: Vec<AsrTurn>,
}

/// One speaker turn: roughly one person's whole answer, not a sentence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AsrTurn {
    #[serde(rename = "startMs")]
    pub start_ms: u64,
    #[serde(rename = "endMs")]
    pub end_ms: u64,
    #[serde(rename = "transcript", default)]
    pub text: String,
    /// The diarization label ("A", "B", ...), only meaningful within one request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
}

impl MetaClient {
    /// Transcribes one WAV (mono 16-bit PCM at 16 kHz, at most 600 s and
    /// 32 MB) with speaker labels. `keywords` (names, jargon) bias the
    /// spelling; leave empty for none.
    pub async fn transcribe(
        &self,
        wav: Vec<u8>,
        keywords: &[String],
    ) -> Result<AsrResult, MetaError> {
        let audio = Part::bytes(wav)
            .file_name("audio.wav")
            .mime_str("audio/wav")
            .expect("static media type");
        let form = Form::new()
            .part("request", json_part(&transcribe_request(keywords)))
            .part("audio", audio);
        // Speaker labels are scoped to the session, so each call gets its own.
        let session = format!("klipolo-{}", Uuid::new_v4());
        let request = self
            .http
            .post(self.meta_url("asr/transcribe"))
            .query(&[("sessionId", session.as_str())])
            .multipart(form);
        let response = self.send_meta(request).await?;
        self.read_json(response).await
    }
}

fn transcribe_request(keywords: &[String]) -> Value {
    let mut request = json!({
        "mode": "DIARIZATION",
        "model": TRANSCRIBE_MODEL,
        "audioEncoding": "WAV",
        "languageBias": ["English"],
    });
    if !keywords.is_empty() {
        request["keywords"] = json!(keywords);
    }
    request
}

fn json_part(value: &Value) -> Part {
    Part::text(value.to_string())
        .mime_str("application/json")
        .expect("static media type")
}
