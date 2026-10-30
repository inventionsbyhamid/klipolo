//! Klipolo's relay API: device registration and status, and word alignment
//! for every device.

use reqwest::multipart::Form;
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::asr::wav_part;
use crate::client::{http_client, read_json, send, trim_base};
use crate::{AsrTurn, MetaClient, MetaError};

/// A device as the relay sees it.
#[derive(Clone, Debug)]
pub struct DeviceInfo {
    /// The new device token, from [`MetaClient::register_device`] only. Keep
    /// it in the Keychain; `Debug` never shows it.
    pub token: Option<SecretString>,
    /// Whether Meta calls may go through the relay (an active access code).
    pub relay_ai: bool,
    /// The device's Klipolo credit, and how much of it is spent.
    pub budget_usd: f64,
    pub spent_usd: f64,
}

#[derive(Deserialize)]
struct DeviceWire {
    #[serde(default)]
    token: Option<String>,
    relay_ai: bool,
    #[serde(default)]
    budget_usd: f64,
    #[serde(default)]
    spent_usd: f64,
}

impl From<DeviceWire> for DeviceInfo {
    fn from(wire: DeviceWire) -> Self {
        Self {
            token: wire.token.map(SecretString::from),
            relay_ai: wire.relay_ai,
            budget_usd: wire.budget_usd,
            spent_usd: wire.spent_usd,
        }
    }
}

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
    /// Registers this device with the relay at `base`, with the access code
    /// if one was entered, and returns its new token. Registering again
    /// rotates the token, keeps the spend, and replaces the stored code with
    /// `code` (`None` clears it).
    pub async fn register_device(
        base: &str,
        device_id: &str,
        code: Option<&str>,
    ) -> Result<DeviceInfo, MetaError> {
        let code = code.map(str::trim).filter(|c| !c.is_empty());
        let mut body = json!({ "device_id": device_id });
        if let Some(code) = code {
            body["access_code"] = json!(code);
        }
        let secrets: Vec<&str> = code.into_iter().collect();
        let url = format!("{}/v1/devices", trim_base(base.to_owned()));
        let response = send(http_client().post(url).json(&body), &secrets).await?;
        let info = DeviceInfo::from(read_json::<DeviceWire>(response, &secrets).await?);
        if info.token.is_none() {
            return Err(MetaError::Decode("the relay sent no device token".into()));
        }
        Ok(info)
    }

    /// The device's relay access, credit and spend (`GET /v1/devices/me`),
    /// with its relay token; without one it is [`MetaError::Unauthorized`].
    pub async fn device_status(&self) -> Result<DeviceInfo, MetaError> {
        let token = self.align_token.as_ref().ok_or(MetaError::Unauthorized)?;
        let request = self.http.get(self.relay_url("/v1/devices/me"));
        let response = self.send_relay(request, token).await?;
        let wire: DeviceWire = self.read_json(response).await?;
        Ok(DeviceInfo {
            token: None,
            ..wire.into()
        })
    }

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
