//! Meta behind the analysis interfaces: Voice Transcribe as the `Transcriber` and the relay's MMS aligner as the
//! `Aligner`, both through one `klipolo_meta::MetaClient`. Whether that client calls Meta with the person's key or
//! goes through the relay with a device token is the client's business, never the caller's. Meta's types and errors
//! stop here.
//!
//! The relay counts one device's requests at once (4 Meta calls, 2 alignments) and answers more with 429 `busy`. So
//! the adapter keeps one device's slots, shared by its clones: make one `Meta` per app and hand clones to every clip
//! and feature, and every request waits for a slot before it is sent.

use std::future::Future;
use std::sync::Arc;

use klipolo_meta::{AsrResult, MetaClient, MetaError, TRANSCRIBE_MODEL, pricing};
use tokio::sync::Semaphore;

use super::{
    AlignLimits, AlignTurn, Aligner, Alignment, BoxFuture, ProviderError, ProviderErrorKind,
    SpeakerTurn, TimedWord, Transcriber, Transcription,
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
}

impl Meta {
    /// One device's adapter. Make one per app and share it (clones share its slots); two made from the same device's
    /// client would each fill the device's slots.
    pub fn new(client: MetaClient) -> Self {
        let calls_at_once = if client.is_relay() {
            RELAY_META_CALLS
        } else {
            DIRECT_META_CALLS
        };
        Self {
            client,
            calls: Arc::new(Semaphore::new(calls_at_once)),
            calls_at_once,
            aligns: Arc::new(Semaphore::new(ALIGN_CALLS)),
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
        assert_eq!(relay.max_in_flight(), 3);
        assert_eq!(direct.max_in_flight(), 4);
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
