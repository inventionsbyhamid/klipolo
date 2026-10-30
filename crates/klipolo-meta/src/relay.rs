//! Klipolo's relay API: word alignment for every device.

use reqwest::multipart::Form;
use serde::{Deserialize, Serialize};

use crate::asr::wav_part;
use crate::{AsrTurn, MetaClient, MetaError};

/// One turn's text and where it sits in the WAV sent with it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlignTurn {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

impl From<&AsrTurn> for AlignTurn {
    fn from(turn: &AsrTurn) -> Self {
        Self {
            start_ms: turn.start_ms,
            end_ms: turn.end_ms,
            text: turn.text.clone(),
        }
    }
}

/// One word with its time in the WAV. A `score` near 0 means the audio does
/// not hold the word (often a repeat the ASR invented).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlignedWord {
    pub text: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub score: f64,
    /// Index of its turn in the request.
    pub turn: usize,
}

/// The relay's alignment: words, and the turns it skipped with the reason.
/// Serializes in the relay's own JSON shape.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(from = "AlignWire", into = "AlignWire")]
pub struct AlignResult {
    pub words: Vec<AlignedWord>,
    /// (turn index, reason).
    pub skipped: Vec<(usize, String)>,
}

#[derive(Serialize, Deserialize)]
struct AlignWire {
    #[serde(default)]
    words: Vec<AlignedWord>,
    #[serde(default)]
    skipped: Vec<Skipped>,
}

#[derive(Serialize, Deserialize)]
struct Skipped {
    turn: usize,
    reason: String,
}

impl From<AlignWire> for AlignResult {
    fn from(wire: AlignWire) -> Self {
        let skipped = wire
            .skipped
            .into_iter()
            .map(|s| (s.turn, s.reason))
            .collect();
        Self {
            words: wire.words,
            skipped,
        }
    }
}

impl From<AlignResult> for AlignWire {
    fn from(result: AlignResult) -> Self {
        let skipped = result
            .skipped
            .into_iter()
            .map(|(turn, reason)| Skipped { turn, reason })
            .collect();
        Self {
            words: result.words,
            skipped,
        }
    }
}

impl MetaClient {
    /// Word times for `turns` of one WAV (16 kHz mono 16-bit), from the
    /// relay's forced aligner. Runs on the relay in both modes, with the
    /// device's relay token; without one it is [`MetaError::AlignUnavailable`].
    pub async fn align(&self, wav: Vec<u8>, turns: &[AlignTurn]) -> Result<AlignResult, MetaError> {
        let token = self
            .align_token
            .as_ref()
            .ok_or(MetaError::AlignUnavailable)?;
        let turns = serde_json::to_string(turns).expect("turns serialize to JSON");
        let form = Form::new()
            .part("audio", wav_part(wav))
            .text("turns", turns);
        let request = self.http.post(self.relay_url("/v1/align")).multipart(form);
        let response = self.send_relay(request, token).await?;
        self.read_json(response).await
    }
}
