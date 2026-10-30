//! Credentials and Settings against a mocked Meta and relay, with the secrets in memory (`MemoryStore` stands in for
//! the Keychain). Nothing here reaches Meta or Klipolo's relay: every base URL is a local mock or a closed port.

use klipolo_app::credentials::{
    META_API_KEY, MemoryStore, RELAY_CODE, RELAY_TOKEN, SecretStore, ai_client, device_id,
};
use klipolo_meta::SecretString;
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const KEY: &str = "sk-meta-test-key-0123456789";
const TOKEN: &str = "relay-test-token-abcdefghij";
const CODE: &str = "KLIP-TEST-0001";

fn secret(value: &str) -> SecretString {
    SecretString::from(value.to_owned())
}

/// The relay's device answer (`POST /v1/devices` adds the token).
fn device(relay_ai: bool, budget: f64, spent: f64) -> Value {
    json!({ "relay_ai": relay_ai, "budget_usd": budget, "spent_usd": spent })
}

#[tokio::test]
async fn key_takes_precedence_over_relay_token() {
    let relay = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/devices/me"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(device(true, 2.0, 0.1)))
        .expect(2)
        .mount(&relay)
        .await;
    let store = MemoryStore::default();
    store.set(META_API_KEY, &secret(KEY)).unwrap();
    store.set(RELAY_TOKEN, &secret(TOKEN)).unwrap();
    store.set(RELAY_CODE, &secret(CODE)).unwrap();

    let client = ai_client(&store, &relay.uri()).expect("a key gives a client");
    assert!(!client.is_relay(), "the key wins over the relay token");
    // The device's relay token still times words (and reads the device) in key mode.
    client
        .device_status()
        .await
        .expect("key mode keeps the relay token");

    // Without the key, the access code's token goes through the relay.
    store.delete(META_API_KEY).unwrap();
    let client = ai_client(&store, &relay.uri()).expect("a code gives a client");
    assert!(client.is_relay());
    client.device_status().await.expect("the relay token");

    // A token without a code (registered for word timing only) is no way to pay for Meta's models.
    store.delete(RELAY_CODE).unwrap();
    assert!(ai_client(&store, &relay.uri()).is_none());
}

#[test]
fn device_id_is_stable_across_launches() {
    let dir = tempfile::tempdir().unwrap();
    let support = dir.path().join("Klipolo");
    let first = device_id(&support).unwrap();
    assert!(uuid::Uuid::parse_str(&first).is_ok(), "{first}");
    assert!(support.join("device.json").is_file());
    assert_eq!(
        device_id(&support).unwrap(),
        first,
        "the next launch reads it"
    );

    let other = tempfile::tempdir().unwrap();
    assert_ne!(
        device_id(other.path()).unwrap(),
        first,
        "another Mac, another id"
    );
}

/// The real Keychain, under a test service of its own (never the app's). Run on the Mac by hand:
/// `cargo test -p klipolo-app --test credentials -- --ignored keychain`.
#[cfg(target_os = "macos")]
#[test]
#[ignore = "touches the login Keychain"]
fn keychain_store_keeps_replaces_and_deletes() {
    use klipolo_app::credentials::KeychainStore;
    use klipolo_meta::ExposeSecret;

    let store = KeychainStore::new("com.klipolo.app.test");
    let account = format!("test-{}", uuid::Uuid::new_v4());
    assert!(store.get(&account).unwrap().is_none());
    store.set(&account, &secret("first value")).unwrap();
    store.set(&account, &secret("second value")).unwrap();
    assert_eq!(
        store.get(&account).unwrap().unwrap().expose_secret(),
        "second value"
    );
    store.delete(&account).unwrap();
    store.delete(&account).unwrap();
    assert!(store.get(&account).unwrap().is_none());
}
