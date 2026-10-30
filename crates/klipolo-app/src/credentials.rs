//! How Klipolo pays for Meta's models.
//!
//! The person either saves their own Meta API key or enters an access code. The key, the relay's device token and the
//! code live only in the secret store (the macOS Keychain: service `com.klipolo.app`, accounts `meta-api-key`,
//! `relay-token`, `relay-code`). Nothing here writes them to a file, a log line or an error.
//!
//! A key wins over a code.

mod device;
mod store;

pub use device::{DEVICE_FILE, device_id};
use klipolo_meta::{MetaClient, SecretString};
#[cfg(target_os = "macos")]
pub use store::KeychainStore;
pub use store::{MemoryStore, SecretStore, StoreError};

/// The Keychain service every Klipolo secret is saved under.
pub const SERVICE: &str = "com.klipolo.app";
/// The person's own Meta API key.
pub const META_API_KEY: &str = "meta-api-key";
/// The relay's token for this device.
pub const RELAY_TOKEN: &str = "relay-token";
/// The access code the person entered, sent again whenever the device registers again.
pub const RELAY_CODE: &str = "relay-code";

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
