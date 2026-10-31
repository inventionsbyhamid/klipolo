//! What a message needs besides the project, each behind a small interface the app fills in: the language model and
//! whether it can be paid for (`Models`, over the saved credentials and Meta's chat adapter), and what each clip's
//! analysis found (`Findings`, over `AnalysisState`). Tests use fakes of both.

use std::collections::HashMap;
use std::sync::Arc;

use klipolo_analysis::providers::meta_chat::MetaChat;
use klipolo_chat::{AnalysisState as ChatAnalysis, BoxFuture, LanguageModel};
use klipolo_model::speech::Levels;

use crate::analysis::{AnalysisState, AssetAnalysis};
use crate::credentials::Credentials;

/// Whether a message can go to the model now.
#[derive(Clone)]
pub enum Readiness {
    /// The model to send it to. `relay`: through Klipolo's relay, where a refused token is renewed once
    /// (`Models::recover`) and the message sent again.
    Ready {
        model: Arc<dyn LanguageModel>,
        relay: bool,
    },
    /// Neither a key nor a code: the panel asks for one.
    NeedsKey,
    /// A key or code is saved, but nothing can pay yet (with a code, this Mac couldn't register with the relay): why, in
    /// one sentence, to show with Retry.
    Problem(String),
}

/// The language model behind chat, as the saved credentials allow it.
pub trait Models: Send + Sync {
    /// Whether a message can go now, and to which model.
    fn ready(&self) -> BoxFuture<'_, Readiness>;

    /// The relay refused the device's token (401): registers the device again (with the saved code). True when it
    /// worked, so the message may be sent once more.
    fn recover(&self) -> BoxFuture<'_, bool>;
}

/// Muse Spark through the app's credentials: the person's key, or an access code through the relay.
pub struct MetaModels {
    credentials: Arc<Credentials>,
}

impl MetaModels {
    pub fn new(credentials: Arc<Credentials>) -> Self {
        Self { credentials }
    }
}

impl Models for MetaModels {
    fn ready(&self) -> BoxFuture<'_, Readiness> {
        Box::pin(async move {
            let ready = self.credentials.ready().await;
            match (ready.meta, ready.problem) {
                // A key with a registration problem still chats: only word timing needs the relay.
                (Some(meta), _) => {
                    let client = meta.client().clone();
                    let relay = client.is_relay();
                    // Chat keeps its own slot, apart from the analysis's (`MetaChat`); one message goes at a time
                    // (`ChatSession`'s queue), so one model call is in flight.
                    Readiness::Ready {
                        model: Arc::new(MetaChat::new(client)),
                        relay,
                    }
                }
                (None, Some(problem)) => Readiness::Problem(problem),
                (None, None) => Readiness::NeedsKey,
            }
        })
    }

    fn recover(&self) -> BoxFuture<'_, bool> {
        Box::pin(async move {
            (self.credentials.recover_unauthorized().await)
                .inspect_err(
                    |e| tracing::warn!(error = %e, "chat: the device couldn't register again"),
                )
                .is_ok()
        })
    }
}

/// What the analysis found for the open project's clips.
pub trait Findings: Send + Sync {
    /// What `asset_id`'s analysis found so far (transcript, speaker turns, scene log), if anything.
    fn found(&self, asset_id: &str) -> Option<Arc<AssetAnalysis>>;

    /// How far `asset_id`'s analysis is, as the compiler asks.
    fn state(&self, asset_id: &str) -> ChatAnalysis;

    /// The sound levels found, by asset id: pauses are read from them.
    fn levels(&self) -> HashMap<String, Arc<Levels>>;
}

impl Findings for AnalysisState {
    fn found(&self, asset_id: &str) -> Option<Arc<AssetAnalysis>> {
        self.analysis_for(asset_id)
    }

    fn state(&self, asset_id: &str) -> ChatAnalysis {
        self.chat_state(asset_id)
    }

    fn levels(&self) -> HashMap<String, Arc<Levels>> {
        self.speech().levels.clone()
    }
}
