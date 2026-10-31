//! How Klipolo pays for Meta's models, and the one Meta adapter built from it.
//!
//! The person either saves their own Meta API key or enters an access code. The key, the relay's device token and the
//! code live only in the secret store (the macOS Keychain: service `com.klipolo.app`, accounts `meta-api-key`,
//! `relay-token`, `relay-code`). Nothing here writes them to a file, a log line or an error.
//!
//! **The relay's device rules.** Every device registers with Klipolo's relay, because word timing runs there in both
//! modes. Registering again rotates the token and replaces the device's code with the one sent, so sending none
//! clears it. So:
//! - a device registers once, the first time AI is used ([`Credentials::ready`]), with its code if one was entered;
//! - it never registers again while a token is saved, except after the relay refused that token (401), and then the
//!   saved code goes with it;
//! - entering a code registers with that code (that is how the relay learns it);
//! - credit and spend come from `GET /v1/devices/me`, never from registering.
//!
//! A key wins over a code. Saving one removes the other, so there is one way to pay at a time.

mod device;
mod store;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

pub use device::{DEVICE_FILE, device_id};
use klipolo_analysis::providers::meta::Meta;
use klipolo_meta::{DeviceInfo, ExposeSecret, MetaClient, MetaError, SecretString};
use serde::Serialize;
#[cfg(target_os = "macos")]
pub use store::KeychainStore;
pub use store::{MemoryStore, SecretStore, StoreError};
use ts_rs::TS;

/// The Keychain service every Klipolo secret is saved under.
pub const SERVICE: &str = "com.klipolo.app";
/// The person's own Meta API key.
pub const META_API_KEY: &str = "meta-api-key";
/// The relay's token for this device.
pub const RELAY_TOKEN: &str = "relay-token";
/// The access code the person entered, sent again whenever the device registers again.
pub const RELAY_CODE: &str = "relay-code";

pub(crate) const KEY_REFUSED: &str = "That key didn't work. Check it on dev.meta.ai.";
const KEY_OFFLINE: &str =
    "Couldn't reach Meta to check the key. Check your internet connection and try again.";
const KEY_EMPTY: &str = "Paste your Meta API key first.";
const CODE_EMPTY: &str = "Enter your access code first.";
const CODE_OFFLINE: &str =
    "Couldn't reach Klipolo to check the code. Check your internet connection and try again.";
const WORD_TIMING_OFFLINE: &str =
    "Couldn't reach Klipolo for word timing, so captions use approximate timing for now.";
const DEVICE_OFFLINE: &str =
    "Couldn't reach Klipolo to set up this Mac. Check your internet connection and try again.";
const CREDIT_OFFLINE: &str =
    "Couldn't reach Klipolo to check your credit. Check your internet connection.";
pub(crate) const CODE_INACTIVE: &str =
    "This access code is no longer active. Enter another code or add your own Meta key.";
const RELAY_BUSY: &str = "Klipolo's server is busy right now. Try again in a moment.";
const RELAY_UNREADABLE: &str = "Klipolo's server sent an answer the app couldn't read. Try again.";
const STORE_FAILED: &str = "Klipolo couldn't save this in your Mac's Keychain. Try again.";

/// Where Meta and Klipolo's relay are: `KLIPOLO_META_BASE` and `KLIPOLO_RELAY_BASE`, else the real ones.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoints {
    /// Meta's API, e.g. `https://api.meta.ai/v1`.
    pub meta_base: String,
    /// The relay, e.g. `https://api.klipolo.com`.
    pub relay_base: String,
}

impl Endpoints {
    pub fn from_env() -> Self {
        Self {
            meta_base: klipolo_meta::meta_base(),
            relay_base: klipolo_meta::relay_base(),
        }
    }
}

/// How AI is paid for right now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum AiMode {
    /// Neither a key nor a code: AI features ask for one, editing and export keep working.
    #[default]
    None,
    /// The person's own Meta API key.
    Key,
    /// An access code, through Klipolo's relay.
    Relay,
}

/// What Settings, the chat panel and the analysis banner show about AI. Never holds the key or the token.
#[derive(Clone, Debug, Default, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AiStatus {
    pub mode: AiMode,
    /// The access code's credit, in US dollars, when the relay said.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub budget_usd: Option<f64>,
    /// What the device has spent, as the relay counts it (it may pass the budget by the last call's cost).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub spent_usd: Option<f64>,
    /// The saved key's last 4 characters, for "Saved, ending in 6789".
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub key_hint: Option<String>,
    /// The access code, shown in full (it isn't a secret to the person who typed it).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub code: Option<String>,
    /// Whether the relay lets this device's Meta calls through (an active code), when the relay said.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub active: Option<bool>,
    /// A plain sentence when something needs the person's attention: the code was turned off, the credit couldn't be
    /// checked.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub notice: Option<String>,
    /// What AI cost since the app started, as Klipolo counts it: the app adds it (`SessionSpend`).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub session: Option<SessionSpend>,
}

/// What AI cost since the app started, by kind, in US dollars at Meta's prices: Settings shows it with a key or a
/// code. Nothing is shown per message.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SessionSpend {
    /// Transcripts, word times and scene logs.
    pub analysis_usd: f64,
    /// Chat's messages.
    pub chat_usd: f64,
    /// The messages that reached the model.
    pub chat_messages: u32,
    /// Pictures drawn for chat (stickers, title card backgrounds).
    pub pictures_usd: f64,
    /// The pictures drawn.
    pub pictures: u32,
}

/// How long a device registration may take (it is a tiny POST).
pub const REGISTER_TIMEOUT: Duration = Duration::from_secs(10);
/// How long [`Credentials::ready`] waits after a failed registration before it tries again.
pub const REGISTER_BACKOFF: Duration = Duration::from_secs(60);

/// What an AI feature gets from [`Credentials::ready`].
#[derive(Clone, Debug)]
pub struct Ready {
    /// The shared adapter. None without a key or code, or with a code whose device isn't registered (no token, so
    /// nothing can pay).
    pub meta: Option<Meta>,
    /// Why the device isn't registered with the relay, as a sentence for the person. With a key, the feature runs and
    /// words are timed approximately meanwhile; with a code, nothing runs until it registers. None when registered,
    /// and with nothing to pay with (then the surface asks for a key or code).
    pub problem: Option<String>,
}

/// Why a Settings change didn't happen, as one plain sentence for the person. Never holds a secret.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct SettingsError {
    pub message: String,
}

impl SettingsError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl From<StoreError> for SettingsError {
    fn from(error: StoreError) -> Self {
        tracing::warn!(%error, "secret store");
        Self::new(STORE_FAILED)
    }
}

/// The secrets as saved, read once and then kept in step with every change.
#[derive(Clone, Default)]
struct Held {
    key: Option<SecretString>,
    token: Option<SecretString>,
    code: Option<SecretString>,
}

impl Held {
    /// What `store` has. A secret it can't read counts as missing, with a line in the log.
    fn read(store: &dyn SecretStore) -> Self {
        let get = |account| {
            store.get(account).unwrap_or_else(|error| {
                tracing::warn!(%error, "secret store");
                None
            })
        };
        Self {
            key: get(META_API_KEY),
            token: get(RELAY_TOKEN),
            code: get(RELAY_CODE),
        }
    }
}

/// The client AI features use, from what `store` holds: the key (with the device's relay token for word timing) when
/// there is one, else the access code's relay token; None with neither. Meta's base URL comes from the environment
/// (`KLIPOLO_META_BASE`).
pub fn ai_client(store: &dyn SecretStore, relay_base: &str) -> Option<MetaClient> {
    let endpoints = Endpoints {
        meta_base: klipolo_meta::meta_base(),
        relay_base: relay_base.to_owned(),
    };
    client_for(&Held::read(store), &endpoints)
}

fn client_for(held: &Held, endpoints: &Endpoints) -> Option<MetaClient> {
    let client = match (&held.key, &held.code, &held.token) {
        (Some(key), _, token) => {
            let direct =
                MetaClient::direct(key.clone()).with_meta_base(endpoints.meta_base.clone());
            match token {
                Some(token) => direct.with_align_token(token.clone()),
                None => direct,
            }
        }
        // A token alone (registered for word timing) doesn't pay for Meta's models.
        (None, Some(_), Some(token)) => MetaClient::relay(token.clone()),
        _ => return None,
    };
    Some(client.with_relay_base(endpoints.relay_base.clone()))
}

/// The app's credentials and its one Meta adapter. Make one per app; every AI feature takes its adapter from here, so
/// they share the device's request slots.
pub struct Credentials {
    store: Arc<dyn SecretStore>,
    endpoints: Endpoints,
    device_id: String,
    held: Mutex<Held>,
    meta: Mutex<Option<Meta>>,
    /// One change at a time: two registrations at once would each rotate the token.
    changes: tokio::sync::Mutex<()>,
    /// Bumped by every change to `held`, so an answer that took a network call can tell it is out of date.
    generation: AtomicU64,
    register_timeout: Duration,
    register_backoff: Duration,
    /// When [`Self::ready`]'s last registration failed, and the sentence it gave. About the credentials held then:
    /// every change clears it (`change`), so a new key or code is never paused by, or shown, an older one's failure.
    failed: Mutex<Option<(Instant, String)>>,
}

impl Credentials {
    /// Reads the saved secrets once and builds the adapter from them. Makes no network call.
    pub fn load(store: Arc<dyn SecretStore>, endpoints: Endpoints, device_id: String) -> Self {
        let held = Held::read(&*store);
        let meta = client_for(&held, &endpoints).map(Meta::new);
        Self {
            store,
            endpoints,
            device_id,
            held: Mutex::new(held),
            meta: Mutex::new(meta),
            changes: tokio::sync::Mutex::new(()),
            generation: AtomicU64::new(0),
            register_timeout: REGISTER_TIMEOUT,
            register_backoff: REGISTER_BACKOFF,
            failed: Mutex::new(None),
        }
    }

    /// How long a registration may take ([`REGISTER_TIMEOUT`] by default).
    pub fn with_register_timeout(mut self, limit: Duration) -> Self {
        self.register_timeout = limit;
        self
    }

    /// How long [`Self::ready`] waits after a failed registration before trying again ([`REGISTER_BACKOFF`] by
    /// default).
    pub fn with_register_backoff(mut self, pause: Duration) -> Self {
        self.register_backoff = pause;
        self
    }

    /// This device's id with the relay.
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// The shared adapter as it is now (None without a key or code). Clones share its slots.
    pub fn meta(&self) -> Option<Meta> {
        lock(&self.meta).clone()
    }

    /// The adapter for an AI feature about to run. The first time, the device registers with the relay (with the
    /// saved code, if any), so word timing works in key mode too.
    ///
    /// If registering fails, `problem` says why. With a key the feature still runs (`meta` is the key's adapter, words
    /// are timed approximately). With a code there is no token to pay with, so `meta` is None: show `problem`, not the
    /// ask for a key or code. After a failure, calls within [`REGISTER_BACKOFF`] don't try again (callers waiting
    /// for the attempt included); they give the same sentence at once, until the key or code changes.
    pub async fn ready(&self) -> Ready {
        let unregistered = {
            let held = lock(&self.held);
            held.token.is_none() && (held.key.is_some() || held.code.is_some())
        };
        let problem = if !unregistered {
            None
        } else if let Some(problem) = self.recent_failure() {
            Some(problem)
        } else {
            let _turn = self.changes.lock().await;
            let held = lock(&self.held).clone();
            if let Some(problem) = self.recent_failure() {
                Some(problem)
            } else if held.token.is_some() || (held.key.is_none() && held.code.is_none()) {
                // Another caller registered, or Remove landed, while this one waited.
                None
            } else {
                match self.register(held.code.as_ref()).await {
                    Ok(_) => None,
                    Err(error) => {
                        let offline = if held.key.is_some() {
                            WORD_TIMING_OFFLINE
                        } else {
                            DEVICE_OFFLINE
                        };
                        let problem = relay_error(&error, offline).message;
                        *lock(&self.failed) = Some((Instant::now(), problem.clone()));
                        Some(problem)
                    }
                }
            }
        };
        Ready {
            meta: self.meta(),
            problem,
        }
    }

    /// The sentence of a registration that failed less than the back-off ago.
    fn recent_failure(&self) -> Option<String> {
        lock(&self.failed)
            .as_ref()
            .filter(|(at, _)| at.elapsed() < self.register_backoff)
            .map(|(_, problem)| problem.clone())
    }

    /// After the relay refused the device's token (401): registers again with the saved code, so the relay keeps it,
    /// and saves the new token. When another caller already did, the current token works and nothing is sent again.
    pub async fn recover_unauthorized(&self) -> Result<(), SettingsError> {
        let _turn = self.changes.lock().await;
        let held = lock(&self.held).clone();
        if let Some(token) = &held.token {
            match self.device_status(token).await {
                Ok(_) => return Ok(()),
                Err(MetaError::Unauthorized) => {}
                Err(error) => return Err(relay_error(&error, CREDIT_OFFLINE)),
            }
        }
        self.register(held.code.as_ref())
            .await
            .map(drop)
            .map_err(|e| relay_error(&e, CODE_OFFLINE))
    }

    /// What Settings shows. With an access code this asks the relay for the credit and spend (re-registering only if
    /// the relay refused the token); a key or nothing needs no network call. If the credentials changed while the relay
    /// was asked (Remove, a key or another code saved), the answer is built from the credentials as they are now.
    pub async fn status(&self) -> AiStatus {
        let (held, asked) = self.snapshot();
        let Some(code) = relay_code(&held) else {
            return local_status(&held);
        };
        let info = match &held.token {
            Some(token) => match self.device_status(token).await {
                Err(MetaError::Unauthorized) => self.register_again_unless_changed(token).await,
                answer => answer,
            },
            // The token went missing from the store: registering with the code is how to get one back, unless another
            // caller just did, or the code was removed meanwhile (read under the lock, never from the copy above).
            None => {
                let _turn = self.changes.lock().await;
                let now = lock(&self.held).clone();
                match (relay_code(&now), &now.token) {
                    (None, _) => return local_status(&now),
                    (Some(_), Some(token)) => self.device_status(token).await,
                    (Some(code), None) => self.register(Some(code)).await,
                }
            }
        };
        let (now, current) = self.snapshot();
        if current != asked {
            match relay_code(&now) {
                // Remove or a key landed while the relay answered: that answer is about credentials no longer in use.
                None => return local_status(&now),
                // Another code: the answer was about the old one.
                Some(now_code) if now_code.expose_secret() != code.expose_secret() => {
                    return relay_base_status(now_code);
                }
                // The same code (only its token changed): the device's credit is still the answer.
                Some(_) => {}
            }
        }
        match info {
            Ok(info) => relay_status(code, &info),
            Err(error) => AiStatus {
                notice: Some(relay_error(&error, CREDIT_OFFLINE).message),
                ..relay_base_status(code)
            },
        }
    }

    /// Checks `key` with Meta (a free call) and saves it only when Meta accepts it. It replaces an access code.
    pub async fn set_key(&self, key: &str) -> Result<AiStatus, SettingsError> {
        let key = key.trim();
        if key.is_empty() {
            return Err(SettingsError::new(KEY_EMPTY));
        }
        let key = SecretString::from(key.to_owned());
        let check = MetaClient::direct(key.clone())
            .with_meta_base(self.endpoints.meta_base.clone())
            .with_ledger(None);
        match check.check_key().await {
            Ok(()) => {}
            Err(MetaError::Unauthorized | MetaError::NotAllowed { .. }) => {
                return Err(SettingsError::new(KEY_REFUSED));
            }
            Err(MetaError::Network(_)) => return Err(SettingsError::new(KEY_OFFLINE)),
            Err(error) => return Err(SettingsError::new(error.user_message())),
        }
        let _turn = self.changes.lock().await;
        self.store.set(META_API_KEY, &key)?;
        self.store.delete(RELAY_CODE)?;
        self.change(|held| {
            held.key = Some(key.clone());
            held.code = None;
        });
        Ok(key_status(&key))
    }

    /// Registers this device with `code` and saves the token and the code. It replaces a saved key.
    pub async fn set_code(&self, code: &str) -> Result<AiStatus, SettingsError> {
        let code = code.trim();
        if code.is_empty() {
            return Err(SettingsError::new(CODE_EMPTY));
        }
        let code = SecretString::from(code.to_owned());
        let _turn = self.changes.lock().await;
        let info = self
            .register(Some(&code))
            .await
            .map_err(|e| relay_error(&e, CODE_OFFLINE))?;
        self.store.set(RELAY_CODE, &code)?;
        self.store.delete(META_API_KEY)?;
        self.change(|held| {
            held.key = None;
            held.code = Some(code.clone());
        });
        Ok(relay_status(&code, &info))
    }

    /// Removes the key and the code. The device keeps its relay token, so word timing works again as soon as either is
    /// added, without registering again.
    pub async fn clear(&self) -> Result<AiStatus, SettingsError> {
        let _turn = self.changes.lock().await;
        self.store.delete(META_API_KEY)?;
        self.store.delete(RELAY_CODE)?;
        self.change(|held| {
            held.key = None;
            held.code = None;
        });
        Ok(AiStatus::default())
    }

    /// Registers the device with `code` (or none) and saves the new token. The caller holds `changes`. Gives up after
    /// the registration limit, as a network error.
    async fn register(&self, code: Option<&SecretString>) -> Result<DeviceInfo, MetaError> {
        let registering = MetaClient::register_device(
            &self.endpoints.relay_base,
            &self.device_id,
            code.map(|c| c.expose_secret()),
        );
        let info = tokio::time::timeout(self.register_timeout, registering)
            .await
            .map_err(|_| {
                MetaError::Network("the relay didn't answer the registration in time".into())
            })??;
        let token = info
            .token
            .clone()
            .ok_or_else(|| MetaError::Decode("the relay sent no device token".into()))?;
        if let Err(error) = self.store.set(RELAY_TOKEN, &token) {
            // Kept for this run; the next launch registers again (with the saved code).
            tracing::warn!(%error, "secret store");
        }
        self.change(|held| held.token = Some(token.clone()));
        Ok(info)
    }

    /// After `refused` got a 401: registers again with the saved code, unless the token changed meanwhile (another
    /// caller registered), in which case the new token is asked instead.
    async fn register_again_unless_changed(
        &self,
        refused: &SecretString,
    ) -> Result<DeviceInfo, MetaError> {
        let _turn = self.changes.lock().await;
        let held = lock(&self.held).clone();
        match &held.token {
            Some(token) if token.expose_secret() != refused.expose_secret() => {
                self.device_status(token).await
            }
            _ => self.register(held.code.as_ref()).await,
        }
    }

    async fn device_status(&self, token: &SecretString) -> Result<DeviceInfo, MetaError> {
        MetaClient::relay(token.clone())
            .with_relay_base(self.endpoints.relay_base.clone())
            .with_ledger(None)
            .device_status()
            .await
    }

    /// Applies `edit` to the held secrets, rebuilds the adapter from them and moves the generation on. A registration
    /// failure noted for the credentials before is forgotten: the next use tries again with these.
    fn change(&self, edit: impl FnOnce(&mut Held)) {
        let mut held = lock(&self.held);
        edit(&mut held);
        *lock(&self.meta) = client_for(&held, &self.endpoints).map(Meta::new);
        *lock(&self.failed) = None;
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    /// The held secrets and their generation, read together.
    fn snapshot(&self) -> (Held, u64) {
        let held = lock(&self.held);
        (held.clone(), self.generation.load(Ordering::SeqCst))
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The access code in use: one is saved and no key wins over it.
fn relay_code(held: &Held) -> Option<&SecretString> {
    held.code.as_ref().filter(|_| held.key.is_none())
}

/// The status that needs no relay: the key's, or none.
fn local_status(held: &Held) -> AiStatus {
    match &held.key {
        Some(key) => key_status(key),
        None => AiStatus::default(),
    }
}

fn key_status(key: &SecretString) -> AiStatus {
    AiStatus {
        mode: AiMode::Key,
        key_hint: store::last_four(key),
        ..AiStatus::default()
    }
}

fn relay_base_status(code: &SecretString) -> AiStatus {
    AiStatus {
        mode: AiMode::Relay,
        code: Some(code.expose_secret().to_owned()),
        ..AiStatus::default()
    }
}

fn relay_status(code: &SecretString, info: &DeviceInfo) -> AiStatus {
    AiStatus {
        budget_usd: Some(info.budget_usd),
        spent_usd: Some(info.spent_usd),
        active: Some(info.relay_ai),
        notice: (!info.relay_ai).then(|| CODE_INACTIVE.to_owned()),
        ..relay_base_status(code)
    }
}

/// A relay error as a sentence about Klipolo's server (the client's own wording names Meta): `offline` when it
/// couldn't be reached, the relay's own sentence when it gave one.
fn relay_error(error: &MetaError, offline: &str) -> SettingsError {
    tracing::warn!(%error, "relay");
    SettingsError::new(match error {
        MetaError::Network(_) => offline.to_owned(),
        MetaError::Overloaded { message: None } => RELAY_BUSY.to_owned(),
        // A bare 504 is the relay (or Traefik in front of it) giving up, not Meta.
        MetaError::TimedOut { message: None, .. } => RELAY_BUSY.to_owned(),
        MetaError::Decode(_) => RELAY_UNREADABLE.to_owned(),
        other => other.user_message(),
    })
}
