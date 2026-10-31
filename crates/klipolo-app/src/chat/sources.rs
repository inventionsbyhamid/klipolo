//! What a message needs besides the project, each behind a small interface the app fills in: the language model and
//! the picture maker, and whether they can be paid for (`Models`, over the saved credentials and Meta's adapters), and
//! what each clip's analysis found (`Findings`, over `AnalysisState`). Tests use fakes of both.

use std::collections::HashMap;
use std::sync::Arc;

use klipolo_analysis::providers::images::ImageMaker;
use klipolo_analysis::providers::meta_chat::MetaChat;
use klipolo_analysis::providers::meta_images::MetaImages;
use klipolo_chat::{AnalysisState as ChatAnalysis, BoxFuture, LanguageModel};
use klipolo_model::speech::Levels;

use super::session::NEEDS_KEY;
use crate::analysis::{AnalysisState, AssetAnalysis};
use crate::credentials::Credentials;

/// Whether a message can go to the model now.
#[derive(Clone)]
pub enum Readiness {
    /// The model to send it to.
    Ready { model: Arc<dyn LanguageModel> },
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

    /// What draws pictures for chat (stickers, title card backgrounds), or why nothing can pay for one, in one
    /// sentence.
    fn images(&self) -> BoxFuture<'_, Result<Arc<dyn ImageMaker>, String>>;

    /// What one picture costs, in US dollars: counted into the session's spend when it comes back.
    fn picture_usd(&self) -> f64;

    /// The model refused the credentials (`ChatErrorKind::Refused`). True when they could be renewed (through Klipolo's
    /// relay, the device registers again with the saved code), so the message may be sent once more; false when there
    /// is nothing to renew (the person's own key was refused: a wrong key) or renewing failed.
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
                    // Chat keeps its own slot, apart from the analysis's (`MetaChat`); one message goes at a time
                    // (`ChatSession`'s queue), so one model call is in flight.
                    Readiness::Ready {
                        model: Arc::new(MetaChat::new(meta.client().clone())),
                    }
                }
                (None, Some(problem)) => Readiness::Problem(problem),
                (None, None) => Readiness::NeedsKey,
            }
        })
    }

    fn images(&self) -> BoxFuture<'_, Result<Arc<dyn ImageMaker>, String>> {
        Box::pin(async move {
            let ready = self.credentials.ready().await;
            match (ready.meta, ready.problem) {
                // Muse Image through the shared adapter: it waits for one of the device's Meta slots.
                (Some(meta), _) => Ok(Arc::new(MetaImages::new(meta)) as Arc<dyn ImageMaker>),
                (None, Some(problem)) => Err(problem),
                (None, None) => Err(NEEDS_KEY.to_owned()),
            }
        })
    }

    fn picture_usd(&self) -> f64 {
        klipolo_meta::pricing::image_usd(1)
    }

    fn recover(&self) -> BoxFuture<'_, bool> {
        Box::pin(async move {
            // Only the relay's device token can be renewed; Meta refusing the person's own key is a wrong key.
            let relay = (self.credentials.meta()).is_some_and(|meta| meta.client().is_relay());
            if !relay {
                return false;
            }
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

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use klipolo_meta::SecretString;

    use super::*;
    use crate::credentials::{Endpoints, META_API_KEY, MemoryStore, RELAY_TOKEN, SecretStore};

    /// Credentials with the person's own key and a device token saved (nothing to register), Meta and the relay at
    /// `server`, which never answers.
    fn with_key(server: &TcpListener) -> Arc<Credentials> {
        let store = MemoryStore::default();
        store
            .set(META_API_KEY, &SecretString::from("sk-test".to_owned()))
            .unwrap();
        store
            .set(RELAY_TOKEN, &SecretString::from("token".to_owned()))
            .unwrap();
        let at = format!("http://{}", server.local_addr().unwrap());
        let endpoints = Endpoints {
            meta_base: at.clone(),
            relay_base: at,
        };
        Arc::new(Credentials::load(
            Arc::new(store),
            endpoints,
            "device".into(),
        ))
    }

    #[tokio::test]
    async fn a_refused_key_is_not_renewed() {
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        server.set_nonblocking(true).unwrap();
        let models = MetaModels::new(with_key(&server));
        assert!(matches!(models.ready().await, Readiness::Ready { .. }));
        // The person's own key: nothing to renew, so it says so at once and asks nobody.
        let renewed = tokio::time::timeout(std::time::Duration::from_secs(5), models.recover())
            .await
            .expect("answered without asking the relay");
        assert!(!renewed);
        assert!(server.accept().is_err(), "nothing was sent");
    }
}
