//! The client: who it talks to, with which secret, and how answers become
//! results or [`MetaError`]s.

use std::error::Error as _;
use std::time::Duration;

use reqwest::header::RETRY_AFTER;
use reqwest::{RequestBuilder, Response};
use secrecy::{ExposeSecret, SecretString};
use serde::de::DeserializeOwned;

use crate::error::{redact, MetaError};

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
/// Cheap to clone (the connection pool is shared). No call retries: callers
/// decide, using [`MetaError::RateLimited`]'s `retry_after`.
#[derive(Clone, Debug)]
pub struct MetaClient {
    pub(crate) http: reqwest::Client,
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
            http: http_client(),
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

    /// The URL of Meta's endpoint `path` (e.g. `responses`): Meta itself, or
    /// the relay's `/meta/v1/` mirror of it.
    pub(crate) fn meta_url(&self, path: &str) -> String {
        match self.auth {
            Auth::Key(_) => format!("{}/{path}", self.meta_base),
            Auth::Relay { .. } => format!("{}/meta/v1/{path}", self.relay_base),
        }
    }

    /// Sends a call to Meta (or its relay mirror) with this client's secret.
    pub(crate) async fn send_meta(&self, request: RequestBuilder) -> Result<Response, MetaError> {
        let request = request.bearer_auth(self.auth_secret().expose_secret());
        send(request, &self.secrets()).await
    }

    fn auth_secret(&self) -> &SecretString {
        match &self.auth {
            Auth::Key(key) => key,
            Auth::Relay { token } => token,
        }
    }

    /// Every secret this client holds, to keep them out of error text.
    pub(crate) fn secrets(&self) -> Vec<&str> {
        std::iter::once(self.auth_secret())
            .chain(self.align_token.as_ref())
            .map(|s| s.expose_secret())
            .collect()
    }

    pub(crate) async fn read_json<T: DeserializeOwned>(
        &self,
        response: Response,
    ) -> Result<T, MetaError> {
        read_json(response, &self.secrets()).await
    }
}

/// The most of an error body read for its message.
const MAX_ERROR_BODY: usize = 64 * 1024;

/// Sends `request`; a status outside 2xx becomes the matching [`MetaError`].
pub(crate) async fn send(request: RequestBuilder, secrets: &[&str]) -> Result<Response, MetaError> {
    let mut response = request
        .send()
        .await
        .map_err(|e| network_error(&e, secrets))?;
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let retry_after = response
        .headers()
        .get(RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let mut body = Vec::new();
    while let Ok(Some(chunk)) = response.chunk().await {
        body.extend_from_slice(&chunk);
        if body.len() >= MAX_ERROR_BODY {
            break;
        }
    }
    Err(MetaError::from_status(
        status.as_u16(),
        retry_after.as_deref(),
        &body,
        secrets,
    ))
}

pub(crate) async fn read_json<T: DeserializeOwned>(
    response: Response,
    secrets: &[&str],
) -> Result<T, MetaError> {
    let body = response
        .bytes()
        .await
        .map_err(|e| network_error(&e, secrets))?;
    // serde's messages can quote the text they could not read.
    serde_json::from_slice(&body).map_err(|e| MetaError::Decode(redact(&e.to_string(), secrets)))
}

/// The error and its causes on one line, e.g. "error sending request for url
/// (...): client error (Connect): Connection refused".
fn network_error(error: &reqwest::Error, secrets: &[&str]) -> MetaError {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    MetaError::Network(redact(&text, secrets))
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

pub(crate) fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("klipolo-meta/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(30))
        // A 10 minute chunk of video or audio can take minutes to analyse.
        .timeout(Duration::from_secs(15 * 60))
        .build()
        .expect("static HTTP client settings are valid")
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
