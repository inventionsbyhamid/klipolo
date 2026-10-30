//! Errors from Meta's API or the relay, and the plain sentence shown to people.
//!
//! No variant ever holds the key or a token: text taken from a response is
//! redacted before it is stored, so `Display`, `Debug` and `user_message` are
//! all safe to log or show.

use thiserror::Error;

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum MetaError {
    /// 402: the relay's credit for this device (or Meta's for the key) is used up.
    #[error("out of credit (402): {message}")]
    Budget { message: String },
    /// 403: the key or the device may not make this call.
    #[error("not allowed (403)")]
    NotAllowed,
    /// 401: the key or the device token was not accepted.
    #[error("not authorized (401)")]
    Unauthorized,
    /// 429: too many requests; `retry_after` is the `Retry-After` header in seconds.
    #[error("rate limited (429){}", retry_hint(*.retry_after))]
    RateLimited { retry_after: Option<u64> },
    /// 503 and other server errors: Meta or the relay cannot answer right now.
    #[error("Meta or the relay is overloaded or unavailable (5xx)")]
    Overloaded,
    /// Any other refused request (400, 404, 413, 422...), with the server's reason.
    #[error("bad request: {message}")]
    BadRequest { message: String },
    /// The request never got an answer (connection, timeout, cut off body).
    #[error("network error: {0}")]
    Network(String),
    /// Word alignment needs a relay device token and this client has none.
    #[error("word alignment unavailable: no relay token")]
    AlignUnavailable,
    /// The answer arrived but is not what the API documents.
    #[error("could not read the answer: {0}")]
    Decode(String),
}

fn retry_hint(retry_after: Option<u64>) -> String {
    retry_after
        .map(|s| format!(", retry after {s} s"))
        .unwrap_or_default()
}

impl MetaError {
    /// A short plain sentence for the person using the app. Never contains
    /// the key or a token, and no em or en dashes.
    pub fn user_message(&self) -> String {
        match self {
            Self::Budget { message } => sentence(message, "Your Meta credit has run out."),
            Self::NotAllowed => {
                "Your Meta API key or access code does not allow this. Check it in Settings.".into()
            }
            Self::Unauthorized => {
                "Your Meta API key or access code was not accepted. Check it in Settings.".into()
            }
            Self::RateLimited { retry_after } => match retry_after {
                Some(1) => "Meta has had too many requests. Try again in 1 second.".into(),
                Some(n) if *n > 1 => {
                    format!("Meta has had too many requests. Try again in {n} seconds.")
                }
                _ => "Meta has had too many requests. Try again in a moment.".into(),
            },
            Self::Overloaded => "Meta is busy right now. Try again in a moment.".into(),
            Self::BadRequest { message } => match sentence(message, "") {
                reason if reason.is_empty() => "Meta could not process this request.".into(),
                reason => format!("Meta could not process this request. {reason}"),
            },
            Self::Network(_) => {
                "Could not reach Meta. Check your internet connection and try again.".into()
            }
            Self::AlignUnavailable => {
                "Exact word timing is not available right now, so captions use approximate timing."
                    .into()
            }
            Self::Decode(_) => "Meta sent an answer Klipolo could not read. Try again.".into(),
        }
    }
}

/// `text` as plain copy ending in a full stop, or `fallback` when it is blank.
fn sentence(text: &str, fallback: &str) -> String {
    let text = plain(text);
    let text = text.trim();
    if text.is_empty() {
        return fallback.to_owned();
    }
    if text.ends_with(['.', '!', '?']) {
        text.to_owned()
    } else {
        format!("{text}.")
    }
}

/// Server text without em or en dashes: a spaced dash becomes a comma, a
/// bare one a hyphen.
fn plain(text: &str) -> String {
    text.replace(" \u{2014} ", ", ")
        .replace(" \u{2013} ", ", ")
        .replace(['\u{2014}', '\u{2013}'], "-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_copy_drops_dashes() {
        assert_eq!(plain("used \u{2014} add a key"), "used, add a key");
        assert_eq!(plain("pages 2\u{2013}3"), "pages 2-3");
    }

    #[test]
    fn sentence_adds_a_full_stop_and_falls_back_when_blank() {
        assert_eq!(sentence("Invalid audio", "x"), "Invalid audio.");
        assert_eq!(sentence("Done!", "x"), "Done!");
        assert_eq!(sentence("  ", "Fallback."), "Fallback.");
    }

    #[test]
    fn display_names_the_status() {
        assert_eq!(
            MetaError::RateLimited {
                retry_after: Some(3)
            }
            .to_string(),
            "rate limited (429), retry after 3 s"
        );
        assert_eq!(
            MetaError::RateLimited { retry_after: None }.to_string(),
            "rate limited (429)"
        );
    }
}
