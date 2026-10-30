//! Meta behind the analysis interfaces: Voice Transcribe as the `Transcriber` and the relay's MMS aligner as the
//! `Aligner`, both through one `klipolo_meta::MetaClient`. Whether that client calls Meta with the person's key or
//! goes through the relay with a device token is the client's business, never the caller's. Meta's types and errors
//! stop here.

use klipolo_meta::{AsrResult, MetaClient, MetaError, TRANSCRIBE_MODEL, pricing};

use super::{
    AlignLimits, AlignTurn, Aligner, Alignment, BoxFuture, ProviderError, ProviderErrorKind,
    SpeakerTurn, TimedWord, Transcriber, Transcription,
};

/// Voice Transcribe's longest request.
pub const TRANSCRIBE_MAX_S: u32 = 600;

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
    max_in_flight: 2,
};

/// Meta's speech services through one client.
#[derive(Clone, Debug)]
pub struct Meta {
    client: MetaClient,
}

impl Meta {
    pub fn new(client: MetaClient) -> Self {
        Self { client }
    }
}

impl Transcriber for Meta {
    fn model(&self) -> &str {
        TRANSCRIBE_MODEL
    }

    fn max_audio_s(&self) -> u32 {
        TRANSCRIBE_MAX_S
    }

    fn transcribe<'a>(
        &'a self,
        wav: Vec<u8>,
        keywords: &'a [String],
    ) -> BoxFuture<'a, Result<Transcription, ProviderError>> {
        Box::pin(async move {
            let heard = self
                .client
                .transcribe(wav, keywords)
                .await
                .map_err(provider_error)?;
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
            let aligned = self
                .client
                .align(wav, &turns)
                .await
                .map_err(provider_error)?;
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
/// Any 429 is `RateLimited`: Meta's rate limit, the relay's busy answer and its daily alignment quota alike.
pub fn provider_error(e: MetaError) -> ProviderError {
    let (kind, retry_after_s) = match &e {
        MetaError::Budget { .. } => (ProviderErrorKind::Budget, None),
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
                    message: Some(quota.into()),
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
            message: Some(quota.into()),
        });
        assert_eq!(limited.retry_after_s, Some(50_400));
        assert_eq!(limited.user_message(), quota);
    }
}
