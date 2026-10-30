//! Credentials and Settings against a mocked Meta and relay, with the secrets in memory (`MemoryStore` stands in for
//! the Keychain). Nothing here reaches Meta or Klipolo's relay: every base URL is a local mock or a closed port.

use std::sync::Arc;

use klipolo_app::credentials::{
    AiMode, AiStatus, Credentials, Endpoints, META_API_KEY, MemoryStore, RELAY_CODE, RELAY_TOKEN,
    SecretStore, ai_client, device_id,
};
use klipolo_meta::{ExposeSecret, SecretString};
use serde_json::{Value, json};
use wiremock::matchers::{any, body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const KEY: &str = "sk-meta-test-key-0123456789";
const BAD_KEY: &str = "sk-meta-bad-key-9876543210";
const TOKEN: &str = "relay-test-token-abcdefghij";
const NEW_TOKEN: &str = "relay-new-token-klmnopqrst";
const CODE: &str = "KLIP-TEST-0001";
const DEVICE: &str = "0b6a3c9e-6f0e-4d3c-9a57-5d2f1c7e8a10";
/// Nothing listens on the discard port, so a call sent here fails at once.
const CLOSED: &str = "http://127.0.0.1:9";
const INVALID_CODE: &str =
    "This access code is not valid. Check the code or add your own Meta key in Settings.";

fn secret(value: &str) -> SecretString {
    SecretString::from(value.to_owned())
}

fn stored(store: &MemoryStore, account: &str) -> Option<String> {
    store
        .get(account)
        .expect("the memory store never fails")
        .map(|s| s.expose_secret().to_owned())
}

fn endpoints(meta: &str, relay: &str) -> Endpoints {
    Endpoints {
        meta_base: meta.to_owned(),
        relay_base: relay.to_owned(),
    }
}

/// The relay's device answer (`POST /v1/devices` adds the token).
fn device(relay_ai: bool, budget: f64, spent: f64) -> Value {
    json!({ "relay_ai": relay_ai, "budget_usd": budget, "spent_usd": spent })
}

fn registered(token: &str, budget: f64, spent: f64) -> ResponseTemplate {
    let mut body = device(true, budget, spent);
    body["token"] = json!(token);
    ResponseTemplate::new(200).set_body_json(body)
}

fn no_requests(server: &MockServer) -> impl std::future::Future<Output = ()> + '_ {
    Mock::given(any())
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(server)
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

    // The app's shared adapter is built the same way.
    let credentials = Credentials::load(
        Arc::new(store.clone()),
        endpoints(CLOSED, &relay.uri()),
        DEVICE.into(),
    );
    assert!(!credentials.meta().expect("an adapter").client().is_relay());

    // Without the key, the access code's token goes through the relay.
    store.delete(META_API_KEY).unwrap();
    let client = ai_client(&store, &relay.uri()).expect("a code gives a client");
    assert!(client.is_relay());
    client.device_status().await.expect("the relay token");

    // A token without a code (registered for word timing only) is no way to pay for Meta's models.
    store.delete(RELAY_CODE).unwrap();
    assert!(ai_client(&store, &relay.uri()).is_none());
}

#[tokio::test]
async fn bad_key_is_not_saved() {
    let meta = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header(
            "authorization",
            format!("Bearer {BAD_KEY}").as_str(),
        ))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "error": { "message": format!("Incorrect API key provided: {BAD_KEY}") }
        })))
        .expect(1)
        .mount(&meta)
        .await;
    let relay = MockServer::start().await;
    no_requests(&relay).await;
    let store = Arc::new(MemoryStore::default());
    let credentials = Credentials::load(
        store.clone(),
        endpoints(&format!("{}/v1", meta.uri()), &relay.uri()),
        DEVICE.into(),
    );

    let error = credentials.set_key(BAD_KEY).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "That key didn't work. Check it on dev.meta.ai."
    );
    assert!(!format!("{error:?}").contains(BAD_KEY));
    assert_eq!(stored(&store, META_API_KEY), None);
    assert!(credentials.meta().is_none());
    assert_eq!(credentials.status().await.mode, AiMode::None);

    // Meta out of reach: nothing is saved either, and the message says why.
    let offline = Credentials::load(
        store.clone(),
        endpoints(CLOSED, &relay.uri()),
        DEVICE.into(),
    );
    let error = offline.set_key(KEY).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "Couldn't reach Meta to check the key. Check your internet connection and try again."
    );
    assert!(!format!("{error:?}").contains(KEY));
    assert_eq!(stored(&store, META_API_KEY), None);
}

#[tokio::test]
async fn good_key_is_saved_and_the_device_registers_once_without_a_code() {
    let meta = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header("authorization", format!("Bearer {KEY}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [] })))
        .expect(2)
        .mount(&meta)
        .await;
    let relay = MockServer::start().await;
    // Word timing runs on the relay in key mode too: the first AI use registers the device, with no code.
    Mock::given(method("POST"))
        .and(path("/v1/devices"))
        .and(body_json(json!({ "device_id": DEVICE })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "token": TOKEN, "relay_ai": false, "budget_usd": 0.0, "spent_usd": 0.0
        })))
        .expect(1)
        .mount(&relay)
        .await;
    let store = Arc::new(MemoryStore::default());
    let credentials = Credentials::load(
        store.clone(),
        endpoints(&format!("{}/v1", meta.uri()), &relay.uri()),
        DEVICE.into(),
    );

    let status = credentials.set_key(&format!("  {KEY}\n")).await.unwrap();
    assert_eq!(status.mode, AiMode::Key);
    assert_eq!(status.key_hint.as_deref(), Some("6789"));
    assert_eq!(stored(&store, META_API_KEY).as_deref(), Some(KEY));
    // The status never carries the key itself.
    assert!(!serde_json::to_string(&status).unwrap().contains(KEY));
    assert_eq!(credentials.status().await, status);

    let adapter = credentials.ready().await.expect("an adapter");
    assert!(!adapter.client().is_relay());
    assert_eq!(stored(&store, RELAY_TOKEN).as_deref(), Some(TOKEN));

    // A new key, with the device's token kept: no second registration (one without a code clears a code on the
    // relay).
    credentials.set_key(KEY).await.unwrap();
    assert!(credentials.ready().await.is_some());
}

#[tokio::test]
async fn access_code_registers_and_saves_token() {
    let relay = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/devices"))
        .and(body_json(
            json!({ "device_id": DEVICE, "access_code": CODE }),
        ))
        .respond_with(registered(NEW_TOKEN, 2.0, 0.06))
        .expect(1)
        .mount(&relay)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/devices/me"))
        .and(header(
            "authorization",
            format!("Bearer {NEW_TOKEN}").as_str(),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(device(true, 2.0, 0.06)))
        .expect(1)
        .mount(&relay)
        .await;
    let store = Arc::new(MemoryStore::default());
    // A key saved before: the code replaces it (one way to pay at a time).
    store.set(META_API_KEY, &secret(KEY)).unwrap();
    let credentials = Credentials::load(
        store.clone(),
        endpoints(CLOSED, &relay.uri()),
        DEVICE.into(),
    );

    let status = credentials.set_code(&format!(" {CODE} ")).await.unwrap();
    let expected = AiStatus {
        mode: AiMode::Relay,
        budget_usd: Some(2.0),
        spent_usd: Some(0.06),
        code: Some(CODE.into()),
        active: Some(true),
        ..AiStatus::default()
    };
    assert_eq!(status, expected);
    assert_eq!(stored(&store, RELAY_TOKEN).as_deref(), Some(NEW_TOKEN));
    assert_eq!(stored(&store, RELAY_CODE).as_deref(), Some(CODE));
    assert_eq!(stored(&store, META_API_KEY), None);
    assert!(credentials.meta().expect("an adapter").client().is_relay());

    // Settings read the credit with the token, never by registering again.
    assert_eq!(credentials.status().await, expected);
}

#[tokio::test]
async fn inactive_code_shows_plain_reason() {
    let relay = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/devices"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "error": "invalid_code", "message": INVALID_CODE
        })))
        .expect(1)
        .mount(&relay)
        .await;
    let store = Arc::new(MemoryStore::default());
    // The device registered for word timing earlier: a refused code leaves its token alone.
    store.set(RELAY_TOKEN, &secret(TOKEN)).unwrap();
    let credentials = Credentials::load(
        store.clone(),
        endpoints(CLOSED, &relay.uri()),
        DEVICE.into(),
    );

    let error = credentials.set_code("KLIP-TEST-0009").await.unwrap_err();
    assert_eq!(error.to_string(), INVALID_CODE);
    assert_eq!(stored(&store, RELAY_CODE), None);
    assert_eq!(stored(&store, RELAY_TOKEN).as_deref(), Some(TOKEN));
    assert!(credentials.meta().is_none());
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

#[tokio::test]
async fn credit_is_read_without_registering_again() {
    let relay = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/devices"))
        .respond_with(registered(NEW_TOKEN, 2.0, 0.0))
        .expect(0)
        .mount(&relay)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/devices/me"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(device(true, 2.0, 2.31)))
        .expect(2)
        .mount(&relay)
        .await;
    let store = Arc::new(MemoryStore::default());
    store.set(RELAY_TOKEN, &secret(TOKEN)).unwrap();
    store.set(RELAY_CODE, &secret(CODE)).unwrap();
    let credentials = Credentials::load(
        store.clone(),
        endpoints(CLOSED, &relay.uri()),
        DEVICE.into(),
    );

    let status = credentials.status().await;
    assert_eq!(status.mode, AiMode::Relay);
    // The relay may pass its cap by the last call's cost; the dialog clamps it, the status keeps what the relay said.
    assert_eq!(
        (status.budget_usd, status.spent_usd),
        (Some(2.0), Some(2.31))
    );
    assert_eq!(status.notice, None);
    // The first AI use finds the device registered.
    assert!(credentials.ready().await.is_some());
    credentials.status().await;
}

#[tokio::test]
async fn a_code_turned_off_says_so() {
    let relay = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/devices/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(device(false, 0.0, 1.2)))
        .mount(&relay)
        .await;
    let store = Arc::new(MemoryStore::default());
    store.set(RELAY_TOKEN, &secret(TOKEN)).unwrap();
    store.set(RELAY_CODE, &secret(CODE)).unwrap();
    let credentials = Credentials::load(store, endpoints(CLOSED, &relay.uri()), DEVICE.into());

    let status = credentials.status().await;
    assert_eq!(status.mode, AiMode::Relay);
    assert_eq!(status.active, Some(false));
    assert_eq!(
        status.notice.as_deref(),
        Some("This access code is no longer active. Enter another code or add your own Meta key.")
    );
}

#[tokio::test]
async fn a_refused_token_registers_again_with_the_stored_code() {
    let relay = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/devices/me"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "error": "unauthorized", "message": "This device is not registered. Register it again."
        })))
        .expect(1)
        .mount(&relay)
        .await;
    // The code goes again with the new registration, or the relay would clear it.
    Mock::given(method("POST"))
        .and(path("/v1/devices"))
        .and(body_json(
            json!({ "device_id": DEVICE, "access_code": CODE }),
        ))
        .respond_with(registered(NEW_TOKEN, 2.0, 0.4))
        .expect(1)
        .mount(&relay)
        .await;
    let store = Arc::new(MemoryStore::default());
    store.set(RELAY_TOKEN, &secret(TOKEN)).unwrap();
    store.set(RELAY_CODE, &secret(CODE)).unwrap();
    let credentials = Credentials::load(
        store.clone(),
        endpoints(CLOSED, &relay.uri()),
        DEVICE.into(),
    );

    let status = credentials.status().await;
    assert_eq!(
        (
            status.mode,
            status.budget_usd,
            status.spent_usd,
            status.active
        ),
        (AiMode::Relay, Some(2.0), Some(0.4), Some(true))
    );
    assert_eq!(stored(&store, RELAY_TOKEN).as_deref(), Some(NEW_TOKEN));
    assert_eq!(stored(&store, RELAY_CODE).as_deref(), Some(CODE));
}

#[tokio::test]
async fn a_busy_relay_saves_nothing_and_can_be_tried_again() {
    let relay = MockServer::start().await;
    // The proxy's own rate limit: plain text, no JSON.
    Mock::given(method("POST"))
        .and(path("/v1/devices"))
        .respond_with(ResponseTemplate::new(429).set_body_string("Too Many Requests"))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&relay)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/devices"))
        .respond_with(registered(NEW_TOKEN, 2.0, 0.0))
        .expect(1)
        .with_priority(2)
        .mount(&relay)
        .await;
    let store = Arc::new(MemoryStore::default());
    let credentials = Credentials::load(
        store.clone(),
        endpoints(CLOSED, &relay.uri()),
        DEVICE.into(),
    );

    let error = credentials.set_code(CODE).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "Too many requests right now. Try again in a moment."
    );
    assert_eq!(stored(&store, RELAY_TOKEN), None);
    assert_eq!(stored(&store, RELAY_CODE), None);

    let status = credentials.set_code(CODE).await.unwrap();
    assert_eq!(status.mode, AiMode::Relay);
    assert_eq!(stored(&store, RELAY_TOKEN).as_deref(), Some(NEW_TOKEN));
}

#[tokio::test]
async fn a_saved_key_registers_on_the_next_use_when_the_relay_was_down() {
    let meta = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [] })))
        .mount(&meta)
        .await;
    let relay = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/devices"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&relay)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/devices"))
        .and(body_json(json!({ "device_id": DEVICE })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "token": TOKEN, "relay_ai": false, "budget_usd": 0.0, "spent_usd": 0.0
        })))
        .expect(1)
        .with_priority(2)
        .mount(&relay)
        .await;
    let store = Arc::new(MemoryStore::default());
    let credentials = Credentials::load(
        store.clone(),
        endpoints(&format!("{}/v1", meta.uri()), &relay.uri()),
        DEVICE.into(),
    );

    credentials.set_key(KEY).await.unwrap();
    // The relay is down at the first use: the feature still gets the key's adapter (approximate word timing).
    let adapter = credentials.ready().await.expect("a key");
    assert!(!adapter.client().is_relay());
    assert_eq!(stored(&store, RELAY_TOKEN), None);
    // The next use registers.
    credentials.ready().await.expect("a key");
    assert_eq!(stored(&store, RELAY_TOKEN).as_deref(), Some(TOKEN));
}

#[tokio::test]
async fn clearing_keeps_the_device_and_drops_the_key_and_code() {
    let relay = MockServer::start().await;
    no_requests(&relay).await;
    let store = Arc::new(MemoryStore::default());
    store.set(META_API_KEY, &secret(KEY)).unwrap();
    store.set(RELAY_TOKEN, &secret(TOKEN)).unwrap();
    store.set(RELAY_CODE, &secret(CODE)).unwrap();
    let credentials = Credentials::load(
        store.clone(),
        endpoints(CLOSED, &relay.uri()),
        DEVICE.into(),
    );

    let status = credentials.clear().await.unwrap();
    assert_eq!(status, AiStatus::default());
    assert_eq!(status.mode, AiMode::None);
    assert_eq!(stored(&store, META_API_KEY), None);
    assert_eq!(stored(&store, RELAY_CODE), None);
    // The device keeps its token, so a key added later times words without registering again.
    assert_eq!(stored(&store, RELAY_TOKEN).as_deref(), Some(TOKEN));
    assert!(credentials.meta().is_none());
    assert!(credentials.ready().await.is_none());
}

/// The real Keychain, under a test service of its own (never the app's). Run on the Mac by hand:
/// `cargo test -p klipolo-app --test credentials -- --ignored keychain`.
#[cfg(target_os = "macos")]
#[test]
#[ignore = "touches the login Keychain"]
fn keychain_store_keeps_replaces_and_deletes() {
    use klipolo_app::credentials::KeychainStore;

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
