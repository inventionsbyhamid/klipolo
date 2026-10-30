//! Errors from Meta's API or the relay, and the plain sentence shown to people.
//!
//! No variant ever holds the key or a token: text taken from a response is
//! redacted before it is stored, so `Display`, `Debug` and `user_message` are
//! all safe to log or show.

use serde_json::Value;
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

/// The most of a server's message kept in an error.
const MAX_MESSAGE_CHARS: usize = 300;

fn retry_hint(retry_after: Option<u64>) -> String {
    retry_after
        .map(|s| format!(", retry after {s} s"))
        .unwrap_or_default()
}

impl MetaError {
    /// The error for a refused call: its status, `Retry-After` header and body.
    /// The body's message is kept (redacted) for 402 and plain refusals.
    pub(crate) fn from_status(
        status: u16,
        retry_after: Option<&str>,
        body: &[u8],
        secrets: &[&str],
    ) -> Self {
        match status {
            401 => Self::Unauthorized,
            402 => Self::Budget {
                message: body_message(body, secrets).unwrap_or_default(),
            },
            403 => Self::NotAllowed,
            429 => Self::RateLimited {
                retry_after: retry_after.and_then(parse_retry_after),
            },
            500..=599 => Self::Overloaded,
            _ => Self::BadRequest {
                message: body_message(body, secrets)
                    .unwrap_or_else(|| format!("The server answered with status {status}.")),
            },
        }
    }

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

/// The message in an error body: Meta's `{"error": {"message": ..}}` or the
/// relay's `{"error": code, "message": ..}`. Not JSON (an HTML page), blank
/// text or a bare code gives none.
fn body_message(body: &[u8], secrets: &[&str]) -> Option<String> {
    let value: Value = serde_json::from_slice(body).ok()?;
    let error = value.get("error");
    let text = error
        .and_then(|e| e.get("message"))
        .or_else(|| value.get("message"))
        .or(error)
        .and_then(Value::as_str)?;
    let text = redact(
        &text.split_whitespace().collect::<Vec<_>>().join(" "),
        secrets,
    );
    // A bare code such as `charging_unavailable` is not words for a person.
    let is_code = text
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    if is_code {
        return None;
    }
    Some(match text.char_indices().nth(MAX_MESSAGE_CHARS) {
        Some((cut, _)) => format!("{}...", &text[..cut]),
        None => text,
    })
}

/// `text` with every secret replaced. Run before any truncation, so no part
/// of a secret survives.
pub(crate) fn redact(text: &str, secrets: &[&str]) -> String {
    secrets
        .iter()
        .filter(|s| !s.is_empty())
        .fold(text.to_owned(), |text, secret| {
            text.replace(secret, "[redacted]")
        })
}

/// Seconds from a `Retry-After` header. An HTTP date gives none.
fn parse_retry_after(value: &str) -> Option<u64> {
    let value = value.trim();
    value.parse::<u64>().ok().or_else(|| {
        let seconds = value.parse::<f64>().ok()?;
        (seconds.is_finite() && seconds >= 0.0).then(|| seconds.ceil() as u64)
    })
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
    fn messages_are_redacted_before_they_are_cut() {
        let secret = "sk-secret-123";
        let body = format!(r#"{{"error": {{"message": "bad key {secret}"}}}}"#);
        assert_eq!(
            body_message(body.as_bytes(), &[secret]).as_deref(),
            Some("bad key [redacted]")
        );
        let long = format!(
            r#"{{"message": "{}{secret}"}}"#,
            "x".repeat(MAX_MESSAGE_CHARS - 3)
        );
        let message = body_message(long.as_bytes(), &[secret]).unwrap();
        assert!(!message.contains("sk-"), "{message}");
        assert!(message.ends_with("..."));
    }

    #[test]
    fn non_json_bodies_have_no_message() {
        assert_eq!(body_message(b"<html>Bad gateway</html>", &[]), None);
        assert_eq!(body_message(br#"{"error": {"message": "  "}}"#, &[]), None);
    }

    #[test]
    fn bare_error_codes_are_not_messages() {
        assert_eq!(
            body_message(br#"{"error": "charging_unavailable"}"#, &[]),
            None
        );
        assert_eq!(
            body_message(br#"{"error": {"message": "service_overloaded"}}"#, &[]),
            None
        );
        assert_eq!(
            body_message(br#"{"error": "Relay not configured."}"#, &[]).as_deref(),
            Some("Relay not configured.")
        );
    }

    #[test]
    fn retry_after_reads_seconds_only() {
        assert_eq!(parse_retry_after(" 7 "), Some(7));
        assert_eq!(parse_retry_after("1.5"), Some(2));
        assert_eq!(parse_retry_after("Wed, 21 Oct 2026 07:28:00 GMT"), None);
        assert_eq!(parse_retry_after("-3"), None);
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
