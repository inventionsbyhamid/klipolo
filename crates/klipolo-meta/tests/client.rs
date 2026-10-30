//! The client against a local mock server. No test reaches Meta or the relay:
//! every client here points both base URLs at a mock or at a closed port, and
//! writes no spend ledger.

use klipolo_meta::{Auth, MetaClient, MetaError, SecretString};

const KEY: &str = "sk-meta-test-key-0123456789";
const TOKEN: &str = "relay-test-token-abcdefghij";

fn secret(value: &str) -> SecretString {
    SecretString::from(value.to_owned())
}

#[test]
fn is_relay_follows_auth() {
    assert!(!MetaClient::direct(secret(KEY)).is_relay());
    assert!(MetaClient::relay(secret(TOKEN)).is_relay());
}

#[test]
fn debug_output_redacts_secrets() {
    let direct = MetaClient::direct(secret(KEY)).with_align_token(secret(TOKEN));
    let relay = MetaClient::relay(secret(TOKEN));
    let auth = Auth::Key(secret(KEY));
    for text in [
        format!("{direct:?}"),
        format!("{relay:?}"),
        format!("{auth:?}"),
    ] {
        assert!(!text.contains(KEY), "{text}");
        assert!(!text.contains(TOKEN), "{text}");
    }
}

#[test]
fn user_messages_are_plain_and_name_settings_where_needed() {
    let errors = [
        MetaError::Budget {
            message: "This device has used its Klipolo credit \u{2014} add a key.".into(),
        },
        MetaError::NotAllowed,
        MetaError::Unauthorized,
        MetaError::RateLimited {
            retry_after: Some(7),
        },
        MetaError::RateLimited { retry_after: None },
        MetaError::Overloaded,
        MetaError::BadRequest {
            message: "Invalid audio \u{2013} expected WAV".into(),
        },
        MetaError::Network("connection refused".into()),
        MetaError::AlignUnavailable,
        MetaError::Decode("expected value at line 1".into()),
    ];
    for error in &errors {
        let text = error.user_message();
        assert!(!text.is_empty());
        assert!(
            !text.contains('\u{2014}') && !text.contains('\u{2013}'),
            "dash in {text:?}"
        );
    }
    assert!(MetaError::Unauthorized.user_message().contains("Settings"));
    assert!(MetaError::RateLimited {
        retry_after: Some(7)
    }
    .user_message()
    .contains("7 seconds"));
}
