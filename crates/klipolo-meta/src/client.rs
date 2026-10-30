//! The client: who it talks to and with which secret.

use secrecy::SecretString;

/// Meta's Model API, used with the person's own key.
pub const DEFAULT_META_BASE: &str = "https://api.meta.ai/v1";
/// Klipolo's relay: Meta calls under `/meta/v1/...`, alignment and devices under `/v1/...`.
pub const DEFAULT_RELAY_BASE: &str = "https://api.klipolo.com";
/// Overrides [`DEFAULT_META_BASE`] (tests, staging).
pub const META_BASE_ENV: &str = "KLIPOLO_META_BASE";
/// Overrides [`DEFAULT_RELAY_BASE`] (tests, staging).
pub const RELAY_BASE_ENV: &str = "KLIPOLO_RELAY_BASE";

/// How calls to Meta are authorized. `Debug` never shows the secret.
#[derive(Clone, Debug)]
pub enum Auth {
    /// The person's own Meta API key: calls go straight to Meta.
    Key(SecretString),
    /// A device token from Klipolo's relay: Meta calls go through the relay,
    /// which uses its own key and meters the device's credit.
    Relay { token: SecretString },
}

/// One HTTP client for Meta's Model API, direct or through the relay.
///
#[derive(Clone, Debug)]
pub struct MetaClient {
    pub(crate) auth: Auth,
    pub(crate) meta_base: String,
    pub(crate) relay_base: String,
    pub(crate) align_token: Option<SecretString>,
}

impl MetaClient {
    /// A client for `auth`, with the base URLs from the environment (or the
    /// defaults). In relay mode the relay token also authorizes alignment.
    pub fn new(auth: Auth) -> Self {
        let align_token = match &auth {
            Auth::Key(_) => None,
            Auth::Relay { token } => Some(token.clone()),
        };
        Self {
            auth,
            meta_base: meta_base(),
            relay_base: relay_base(),
            align_token,
        }
    }

    /// Calls Meta directly with the person's own key.
    pub fn direct(key: SecretString) -> Self {
        Self::new(Auth::Key(key))
    }

    /// Calls Meta through the relay with the device's token.
    pub fn relay(token: SecretString) -> Self {
        Self::new(Auth::Relay { token })
    }

    /// The device's relay token for word alignment (and device status), which
    /// runs on the relay in both modes.
    pub fn with_align_token(mut self, token: SecretString) -> Self {
        self.align_token = Some(token);
        self
    }

    /// Meta's base URL, e.g. `https://api.meta.ai/v1`.
    pub fn with_meta_base(mut self, base: impl Into<String>) -> Self {
        self.meta_base = trim_base(base.into());
        self
    }

    /// The relay's base URL, e.g. `https://api.klipolo.com`.
    pub fn with_relay_base(mut self, base: impl Into<String>) -> Self {
        self.relay_base = trim_base(base.into());
        self
    }

    /// True when Meta calls go through the relay.
    pub fn is_relay(&self) -> bool {
        matches!(self.auth, Auth::Relay { .. })
    }
}

/// Meta's base URL: `KLIPOLO_META_BASE` when set, else [`DEFAULT_META_BASE`].
pub fn meta_base() -> String {
    env_base(META_BASE_ENV, DEFAULT_META_BASE)
}

/// The relay's base URL: `KLIPOLO_RELAY_BASE` when set, else [`DEFAULT_RELAY_BASE`].
pub fn relay_base() -> String {
    env_base(RELAY_BASE_ENV, DEFAULT_RELAY_BASE)
}

fn env_base(var: &str, default: &str) -> String {
    let base = std::env::var(var).ok().filter(|v| !v.trim().is_empty());
    trim_base(base.unwrap_or_else(|| default.to_owned()))
}

fn trim_base(base: String) -> String {
    base.trim().trim_end_matches('/').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bases_lose_trailing_slashes() {
        assert_eq!(trim_base(" http://x/v1/ ".into()), "http://x/v1");
        let client = MetaClient::direct(SecretString::from("k"))
            .with_meta_base("http://m/v1/")
            .with_relay_base("http://r/");
        assert_eq!(
            (client.meta_base.as_str(), client.relay_base.as_str()),
            ("http://m/v1", "http://r")
        );
    }

    #[test]
    fn relay_mode_aligns_with_its_own_token_and_direct_mode_needs_one() {
        assert!(MetaClient::relay(SecretString::from("t"))
            .align_token
            .is_some());
        assert!(MetaClient::direct(SecretString::from("k"))
            .align_token
            .is_none());
    }
}
