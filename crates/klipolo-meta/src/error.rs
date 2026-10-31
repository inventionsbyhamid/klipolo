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
    #[error("out of credit (402){}", detail(Some(.message)))]
    Budget { message: String },
    /// 403: the key or the device may not make this call, with the server's
    /// reason when it gave one (e.g. "This access code is not valid...").
    #[error("not allowed (403){}", detail(.message.as_ref()))]
    NotAllowed { message: Option<String> },
    /// 401: the key or the device token was not accepted.
    #[error("not authorized (401)")]
    Unauthorized,
    /// 429: too many requests (Meta's rate limit, or the relay's `busy` or
    /// daily `align_quota`). `retry_after` is the `Retry-After` header in
    /// seconds (up to a day for the quota); `code` the server's error code
    /// (the relay's `busy` or `align_quota`, Meta's own), which tells a busy
    /// moment from a used-up allowance; `message` the server's reason.
    #[error("rate limited (429){}{}{}", code_hint(.code.as_ref()), retry_hint(*.retry_after), detail(.message.as_ref()))]
    RateLimited {
        retry_after: Option<u64>,
        code: Option<String>,
        message: Option<String>,
    },
    /// 503 and other server errors: Meta or the relay cannot answer right
    /// now, with the server's reason when it gave one.
    #[error("Meta or the relay is overloaded or unavailable (5xx){}", detail(.message.as_ref()))]
    Overloaded { message: Option<String> },
    /// Any other refused request (400, 404, 413, 422...), with the server's reason (for the log only: the person
    /// reads a plain sentence).
    #[error("bad request{}", detail(Some(.message)))]
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
    /// A local file to upload could not be opened or read; `name` is its file
    /// name, without folders.
    #[error("could not read the local file {name}")]
    LocalFile { name: String },
    /// A request Meta declined under its content policy (a 4xx with the code
    /// `content_policy_violation`, from Meta or passed on by the relay), with
    /// its reason when it gave one.
    #[error("declined under Meta's content policy{}", detail(.message.as_ref()))]
    Refused { message: Option<String> },
    /// 504 or 408: no answer in time (Meta's non-streaming limit, the
    /// relay's wait for Meta, or a request body too slow to arrive), with the
    /// server's reason when it gave one.
    #[error("timed out ({status}){}", detail(.message.as_ref()))]
    TimedOut {
        status: u16,
        message: Option<String>,
    },
    /// 503 `not_configured`: the relay has no Meta key of its own yet.
    #[error("the relay has no Meta key (503 not_configured)")]
    NotConfigured,
}

/// The most of a server's message kept in an error.
const MAX_MESSAGE_CHARS: usize = 300;

/// The relay's 429 code for a device's (or the relay's) calls at once.
const RELAY_BUSY_CODE: &str = "busy";
/// Meta's code for a request it won't serve under its content policy.
const CONTENT_POLICY_CODE: &str = "content_policy_violation";
/// The relay's 503 code while it has no Meta key.
const NOT_CONFIGURED_CODE: &str = "not_configured";

/// ": message" for `Display`, or nothing when there is no message.
fn detail(message: Option<&String>) -> String {
    match message {
        Some(m) if !m.is_empty() => format!(": {m}"),
        _ => String::new(),
    }
}

fn code_hint(code: Option<&String>) -> String {
    code.map(|c| format!(" {c}")).unwrap_or_default()
}

fn retry_hint(retry_after: Option<u64>) -> String {
    retry_after
        .map(|s| format!(", retry after {s} s"))
        .unwrap_or_default()
}

impl MetaError {
    /// The error for a refused call: its status, `Retry-After` header and body.
    /// The body's message is kept (redacted) for 402, 403, 5xx, content
    /// policy refusals and plain refusals.
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
            403 => Self::NotAllowed {
                message: body_message(body, secrets),
            },
            429 => Self::RateLimited {
                retry_after: retry_after.and_then(parse_retry_after),
                code: body_code(body, secrets),
                message: body_message(body, secrets),
            },
            408 | 504 => Self::TimedOut {
                status,
                message: body_message(body, secrets),
            },
            503 if body_code(body, secrets).as_deref() == Some(NOT_CONFIGURED_CODE) => {
                Self::NotConfigured
            }
            500..=599 => Self::Overloaded {
                message: body_message(body, secrets),
            },
            _ if body_code(body, secrets).as_deref() == Some(CONTENT_POLICY_CODE) => {
                Self::Refused {
                    message: body_message(body, secrets),
                }
            }
            _ => Self::BadRequest {
                message: body_message(body, secrets)
                    .unwrap_or_else(|| format!("The server answered with status {status}.")),
            },
        }
    }

    /// A busy moment to wait out (`retry_after`) and send again, not a
    /// used-up allowance: the relay's 429 `busy`, or a 429 with neither an
    /// error code nor a message (Traefik's plain-text rate limit in front of
    /// the relay). A JSON 429 that gives a reason (the relay's daily
    /// `align_quota`, Meta's own rate limit, even with a null code) is a stop.
    pub fn is_busy(&self) -> bool {
        match self {
            Self::RateLimited { code, message, .. } => match code.as_deref() {
                Some(code) => code == RELAY_BUSY_CODE,
                None => message.is_none(),
            },
            _ => false,
        }
    }

    /// A short plain sentence for the person using the app. Never contains
    /// the key or a token, and no em or en dashes.
    pub fn user_message(&self) -> String {
        match self {
            Self::Budget { message } => sentence(message, "Your Meta credit has run out."),
            Self::NotAllowed { message } => sentence(
                message.as_deref().unwrap_or_default(),
                "Your Meta API key or access code does not allow this. Check it in Settings.",
            ),
            Self::Unauthorized => {
                "Your Meta API key or access code was not accepted. Check it in Settings.".into()
            }
            Self::RateLimited {
                retry_after,
                message,
                ..
            } => sentence(
                message.as_deref().unwrap_or_default(),
                &format!(
                    "Too many requests right now. Try again {}.",
                    wait(*retry_after)
                ),
            ),
            Self::Overloaded { message } => sentence(
                message.as_deref().unwrap_or_default(),
                "Meta is busy right now. Try again in a moment.",
            ),
            // The server's own text (often about fields the person never sees) is for the log: `Display` has it.
            Self::BadRequest { .. } => "Meta could not process this request. Try again.".into(),
            Self::Network(_) => {
                "Could not reach Meta. Check your internet connection and try again.".into()
            }
            Self::AlignUnavailable => {
                "Exact word timing is not available right now, so captions use approximate timing."
                    .into()
            }
            Self::Decode(_) => "Meta sent an answer Klipolo could not read. Try again.".into(),
            Self::LocalFile { name } => format!("Klipolo could not read {name}."),
            Self::Refused { .. } => {
                "Meta turned this down under its content policy. Try wording it differently.".into()
            }
            Self::TimedOut { message, .. } => sentence(
                message.as_deref().unwrap_or_default(),
                "Meta took too long to answer. Try again.",
            ),
            Self::NotConfigured => "Klipolo's AI service isn't set up yet. Try again later or add your own Meta key in Settings.".into(),
        }
    }
}

/// When to try again, for a person: seconds up to 90, then minutes up to 90,
/// then "later today" (the relay's daily quota resets within a day).
fn wait(retry_after: Option<u64>) -> String {
    match retry_after.unwrap_or(0) {
        0 => "in a moment".into(),
        1 => "in 1 second".into(),
        s @ 2..=90 => format!("in {s} seconds"),
        s @ 91..=5400 => format!("in {} minutes", s.div_ceil(60)),
        86_401.. => "later".into(),
        _ => "later today".into(),
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

/// The longest error code kept.
const MAX_CODE_CHARS: usize = 64;

/// The error code in an error body: the relay's `{"error": code}` or Meta's
/// `{"error": {"code": ..}}`. Only a code (lowercase letters, digits and
/// underscores, redacted first) is kept, so it can never carry words or a
/// secret.
fn body_code(body: &[u8], secrets: &[&str]) -> Option<String> {
    let value: Value = serde_json::from_slice(body).ok()?;
    let error = value.get("error")?;
    let code = error
        .as_str()
        .or_else(|| error.get("code").and_then(Value::as_str))?;
    let code = redact(code, secrets);
    let is_code = !code.is_empty()
        && code.len() <= MAX_CODE_CHARS
        && code
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    is_code.then_some(code)
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
    fn display_has_no_trailing_colon_without_a_message() {
        let empty = String::new;
        assert_eq!(
            MetaError::Budget { message: empty() }.to_string(),
            "out of credit (402)"
        );
        assert_eq!(
            MetaError::BadRequest { message: empty() }.to_string(),
            "bad request"
        );
        assert_eq!(
            MetaError::NotAllowed {
                message: Some(empty())
            }
            .to_string(),
            "not allowed (403)"
        );
        assert_eq!(
            MetaError::Budget {
                message: "Credit used.".into()
            }
            .to_string(),
            "out of credit (402): Credit used."
        );
    }

    #[test]
    fn display_names_the_status() {
        let limited = |retry_after, message: Option<&str>| MetaError::RateLimited {
            retry_after,
            code: None,
            message: message.map(str::to_owned),
        };
        assert_eq!(
            limited(Some(3), None).to_string(),
            "rate limited (429), retry after 3 s"
        );
        assert_eq!(limited(None, None).to_string(), "rate limited (429)");
        assert_eq!(
            limited(Some(1), Some("Busy.")).to_string(),
            "rate limited (429), retry after 1 s: Busy."
        );
    }
}
