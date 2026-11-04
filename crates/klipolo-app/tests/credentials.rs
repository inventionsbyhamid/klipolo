//! Credentials and Settings against a mocked Meta and relay, with the secrets in memory (`MemoryStore` stands in for
//! the Keychain). Nothing here reaches Meta or Klipolo's relay: every base URL is a local mock or a closed port.

use std::sync::Arc;
use std::time::Duration;

use klipolo_app::credentials::{
    AiMode, AiStatus, Credentials, Endpoints, META_API_KEY, MemoryStore, RELAY_CODE, RELAY_TOKEN,
    SecretStore, StoreError, ai_client, device_id,
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

    let ready = credentials.ready().await;
    assert_eq!(ready.problem, None);
    let adapter = ready.meta.expect("an adapter");
    assert!(!adapter.client().is_relay());
    assert_eq!(stored(&store, RELAY_TOKEN).as_deref(), Some(TOKEN));

    // A new key, with the device's token kept: no second registration (one without a code clears a code on the
    // relay).
    credentials.set_key(KEY).await.unwrap();
    assert!(credentials.ready().await.meta.is_some());
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
    assert!(credentials.ready().await.meta.is_some());
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
async fn a_lost_token_is_got_back_with_the_saved_code() {
    let relay = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/devices"))
        .and(body_json(
            json!({ "device_id": DEVICE, "access_code": CODE }),
        ))
        .respond_with(registered(NEW_TOKEN, 2.0, 0.3))
        .expect(1)
        .mount(&relay)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/devices/me"))
        .and(header(
            "authorization",
            format!("Bearer {NEW_TOKEN}").as_str(),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(device(true, 2.0, 0.3)))
        .expect(1)
        .mount(&relay)
        .await;
    let store = Arc::new(MemoryStore::default());
    // The code is saved but its token is gone (the Keychain item was removed by hand, say).
    store.set(RELAY_CODE, &secret(CODE)).unwrap();
    let credentials = Arc::new(Credentials::load(
        store.clone(),
        endpoints(CLOSED, &relay.uri()),
        DEVICE.into(),
    ));

    // Two readers at once register once; the second reads the credit with the new token.
    let (a, b) = tokio::join!(credentials.status(), credentials.status());
    for status in [a, b] {
        assert_eq!(
            (status.mode, status.budget_usd, status.spent_usd),
            (AiMode::Relay, Some(2.0), Some(0.3))
        );
    }
    assert_eq!(stored(&store, RELAY_TOKEN).as_deref(), Some(NEW_TOKEN));
}

/// The relay's credit answer, held back for `delay`.
fn slow_credit(delay: Duration) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .set_body_json(device(true, 2.0, 0.5))
        .set_delay(delay)
}

#[tokio::test]
async fn a_slow_credit_answer_never_brings_back_a_removed_code() {
    let relay = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/devices/me"))
        .respond_with(slow_credit(Duration::from_millis(600)))
        .expect(1)
        .mount(&relay)
        .await;
    let store = Arc::new(MemoryStore::default());
    store.set(RELAY_TOKEN, &secret(TOKEN)).unwrap();
    store.set(RELAY_CODE, &secret(CODE)).unwrap();
    let credentials = Credentials::load(store, endpoints(CLOSED, &relay.uri()), DEVICE.into());

    // Remove lands while Settings waits for the credit.
    let (status, cleared) = tokio::join!(credentials.status(), async {
        tokio::time::sleep(Duration::from_millis(150)).await;
        credentials.clear().await.unwrap()
    });
    assert_eq!(cleared, AiStatus::default());
    assert_eq!(
        status,
        AiStatus::default(),
        "the answer is the credentials now, not the relay's view from before Remove"
    );
}

#[tokio::test]
async fn a_slow_credit_answer_gives_way_to_a_key_saved_meanwhile() {
    let meta = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [] })))
        .mount(&meta)
        .await;
    let relay = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/devices/me"))
        .respond_with(slow_credit(Duration::from_millis(600)))
        .mount(&relay)
        .await;
    let store = Arc::new(MemoryStore::default());
    store.set(RELAY_TOKEN, &secret(TOKEN)).unwrap();
    store.set(RELAY_CODE, &secret(CODE)).unwrap();
    let credentials = Credentials::load(
        store,
        endpoints(&format!("{}/v1", meta.uri()), &relay.uri()),
        DEVICE.into(),
    );

    let (status, saved) = tokio::join!(credentials.status(), async {
        tokio::time::sleep(Duration::from_millis(150)).await;
        credentials.set_key(KEY).await.unwrap()
    });
    assert_eq!(saved.mode, AiMode::Key);
    assert_eq!(status, saved);
}

/// The memory store with deletes that take a while, so another call can start while Remove is still saving.
struct SlowDeletes(MemoryStore);

impl SecretStore for SlowDeletes {
    fn get(&self, account: &str) -> Result<Option<SecretString>, StoreError> {
        self.0.get(account)
    }

    fn set(&self, account: &str, secret: &SecretString) -> Result<(), StoreError> {
        self.0.set(account, secret)
    }

    fn delete(&self, account: &str) -> Result<(), StoreError> {
        std::thread::sleep(Duration::from_millis(150));
        self.0.delete(account)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_token_is_not_got_back_for_a_code_just_removed() {
    let relay = MockServer::start().await;
    no_requests(&relay).await;
    // A code without its token, and Remove under way.
    let store = MemoryStore::default();
    store.set(RELAY_CODE, &secret(CODE)).unwrap();
    let credentials = Arc::new(Credentials::load(
        Arc::new(SlowDeletes(store)),
        endpoints(CLOSED, &relay.uri()),
        DEVICE.into(),
    ));
    let clearing = tokio::spawn({
        let credentials = credentials.clone();
        async move { credentials.clear().await }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Settings opens before Remove has finished: it must not register the removed code again.
    let status = credentials.status().await;
    clearing.await.unwrap().unwrap();
    assert_eq!(status, AiStatus::default());
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
    // No pause between tries here; `parallel_first_uses_against_a_hanging_relay_cost_one_timeout` covers it.
    let credentials = Credentials::load(
        store.clone(),
        endpoints(&format!("{}/v1", meta.uri()), &relay.uri()),
        DEVICE.into(),
    )
    .with_register_backoff(Duration::ZERO);

    credentials.set_key(KEY).await.unwrap();
    // The relay is down at the first use: the feature still gets the key's adapter (approximate word timing), and
    // the reason.
    let ready = credentials.ready().await;
    assert!(!ready.meta.expect("a key").client().is_relay());
    assert_eq!(
        ready.problem.as_deref(),
        Some("Klipolo's server is busy right now. Try again in a moment.")
    );
    assert_eq!(stored(&store, RELAY_TOKEN), None);
    // The next use registers.
    let ready = credentials.ready().await;
    assert!(ready.meta.is_some());
    assert_eq!(ready.problem, None);
    assert_eq!(stored(&store, RELAY_TOKEN).as_deref(), Some(TOKEN));
}

/// A bare 504 (the relay's proxy, or Traefik, giving up on it) is Klipolo's server being busy, like a bare 503: not
/// Meta's slowness.
#[tokio::test]
async fn a_bare_504_from_the_relay_says_klipolos_server_is_busy() {
    let meta = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [] })))
        .mount(&meta)
        .await;
    let relay = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/devices"))
        .respond_with(ResponseTemplate::new(504))
        .mount(&relay)
        .await;
    let store = Arc::new(MemoryStore::default());
    let credentials = Credentials::load(
        store.clone(),
        endpoints(&format!("{}/v1", meta.uri()), &relay.uri()),
        DEVICE.into(),
    )
    .with_register_backoff(Duration::ZERO);

    credentials.set_key(KEY).await.unwrap();
    let ready = credentials.ready().await;

    assert_eq!(
        ready.problem.as_deref(),
        Some("Klipolo's server is busy right now. Try again in a moment.")
    );
}

#[tokio::test]
async fn parallel_first_uses_against_a_hanging_relay_cost_one_timeout() {
    let relay = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/devices"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(10)))
        .expect(1)
        .mount(&relay)
        .await;
    let store = Arc::new(MemoryStore::default());
    store.set(META_API_KEY, &secret(KEY)).unwrap();
    let limit = Duration::from_millis(400);
    let credentials = Credentials::load(store, endpoints(CLOSED, &relay.uri()), DEVICE.into())
        .with_register_timeout(limit);

    // Five features start at once: one registers and waits out the limit; the others see that it failed.
    let started = std::time::Instant::now();
    let all = tokio::join!(
        credentials.ready(),
        credentials.ready(),
        credentials.ready(),
        credentials.ready(),
        credentials.ready()
    );
    let took = started.elapsed();
    assert!(
        took < limit * 2,
        "five callers took {took:?}, more than one limit of {limit:?}"
    );
    let offline =
        "Couldn't reach Klipolo for word timing, so captions use approximate timing for now.";
    for ready in [all.0, all.1, all.2, all.3, all.4] {
        assert!(ready.meta.is_some(), "the key still works");
        assert_eq!(ready.problem.as_deref(), Some(offline));
    }

    // Within the pause, the next use doesn't try at all.
    let started = std::time::Instant::now();
    let ready = credentials.ready().await;
    assert!(started.elapsed() < Duration::from_millis(100));
    assert_eq!(ready.problem.as_deref(), Some(offline));
}

/// A failed registration pauses the next tries, but only for the credentials it was about: a new key (or code, or
/// Remove) starts afresh, so the old sentence never shows with the new key.
#[tokio::test]
async fn a_new_key_tries_to_register_again_at_once() {
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
        .respond_with(registered(TOKEN, 0.0, 0.0))
        .expect(1)
        .with_priority(2)
        .mount(&relay)
        .await;
    let store = Arc::new(MemoryStore::default());
    store.set(META_API_KEY, &secret(BAD_KEY)).unwrap();
    // The default pause: a minute.
    let credentials = Credentials::load(
        store.clone(),
        endpoints(&format!("{}/v1", meta.uri()), &relay.uri()),
        DEVICE.into(),
    );

    let busy = "Klipolo's server is busy right now. Try again in a moment.";
    assert_eq!(credentials.ready().await.problem.as_deref(), Some(busy));
    // Within the pause, the same key gets the same sentence without a try.
    assert_eq!(credentials.ready().await.problem.as_deref(), Some(busy));
    assert_eq!(relay.received_requests().await.unwrap().len(), 1);

    credentials.set_key(KEY).await.unwrap();
    let ready = credentials.ready().await;
    assert_eq!(ready.problem, None, "the new key registers at once");
    assert_eq!(stored(&store, RELAY_TOKEN).as_deref(), Some(TOKEN));
}

#[tokio::test]
async fn a_code_whose_device_cannot_register_says_why_instead_of_asking_for_a_key() {
    let relay = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/devices"))
        .and(body_json(
            json!({ "device_id": DEVICE, "access_code": CODE }),
        ))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "error": "invalid_code", "message": INVALID_CODE
        })))
        .expect(1)
        .mount(&relay)
        .await;
    let store = Arc::new(MemoryStore::default());
    // A code whose token is gone (the code was turned off since).
    store.set(RELAY_CODE, &secret(CODE)).unwrap();
    let credentials = Credentials::load(store, endpoints(CLOSED, &relay.uri()), DEVICE.into());

    let ready = credentials.ready().await;
    assert!(ready.meta.is_none(), "a code without a token can't pay");
    assert_eq!(ready.problem.as_deref(), Some(INVALID_CODE));
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
    // Nothing to pay with: no adapter and no problem, so the AI surfaces ask for a key or code.
    let ready = credentials.ready().await;
    assert!(ready.meta.is_none());
    assert_eq!(ready.problem, None);
}

/// The trial code tests build in, as the app builds in `KLIPOLO_TRIAL_CODE`.
const TRIAL: &str = "KLIP-TRIAL-TEST";
const USED_UP: &str =
    "Your free credit is used up. Add your own Meta key in Settings to keep using AI.";

fn trial_registration(
    server: &MockServer,
    answer: ResponseTemplate,
    times: u64,
) -> impl std::future::Future<Output = ()> + '_ {
    Mock::given(method("POST"))
        .and(path("/v1/devices"))
        .and(body_json(
            json!({ "device_id": DEVICE, "access_code": TRIAL }),
        ))
        .respond_with(answer)
        .expect(times)
        .mount(server)
}

fn on_trial(store: Arc<MemoryStore>, relay: &MockServer) -> Credentials {
    Credentials::load(store, endpoints(CLOSED, &relay.uri()), DEVICE.into())
        .with_trial_code(Some(secret(TRIAL)))
}

#[tokio::test]
async fn the_free_credit_pays_with_nothing_saved_and_registers_once_a_run() {
    let relay = MockServer::start().await;
    trial_registration(&relay, registered(NEW_TOKEN, 5.0, 0.2), 1).await;
    Mock::given(method("GET"))
        .and(path("/v1/devices/me"))
        .and(header(
            "authorization",
            format!("Bearer {NEW_TOKEN}").as_str(),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(device(true, 5.0, 0.2)))
        .expect(1)
        .mount(&relay)
        .await;
    let store = Arc::new(MemoryStore::default());
    // A token saved before (another code's, or none's): the free credit doesn't pay with it until it registered.
    store.set(RELAY_TOKEN, &secret(TOKEN)).unwrap();
    let credentials = on_trial(store.clone(), &relay);
    assert!(credentials.meta().is_none());

    let ready = credentials.ready().await;
    assert_eq!(ready.problem, None);
    let meta = ready.meta.expect("the free credit pays");
    assert!(meta.client().is_relay());
    assert_eq!(meta.client().used_up_message(), Some(USED_UP));
    // The token is saved; the trial code never is.
    assert_eq!(stored(&store, RELAY_TOKEN).as_deref(), Some(NEW_TOKEN));
    assert_eq!(stored(&store, RELAY_CODE), None);

    let status = credentials.status().await;
    assert_eq!(
        status,
        AiStatus {
            mode: AiMode::Relay,
            trial: true,
            budget_usd: Some(5.0),
            spent_usd: Some(0.2),
            active: Some(true),
            ..AiStatus::default()
        }
    );
    assert!(!format!("{status:?}").contains(TRIAL));
    assert!(!serde_json::to_string(&status).unwrap().contains(TRIAL));
    // Registered this run: the next use doesn't register again.
    assert!(credentials.ready().await.meta.is_some());
}

#[tokio::test]
async fn without_a_trial_code_nothing_saved_still_asks_for_a_key_or_code() {
    let relay = MockServer::start().await;
    no_requests(&relay).await;
    let credentials = Credentials::load(
        Arc::new(MemoryStore::default()),
        endpoints(CLOSED, &relay.uri()),
        DEVICE.into(),
    )
    .with_trial_code(None);

    assert!(credentials.meta().is_none());
    let ready = credentials.ready().await;
    assert!(ready.meta.is_none());
    assert_eq!(ready.problem, None);
    assert_eq!(credentials.status().await, AiStatus::default());
    assert_eq!(credentials.clear().await.unwrap(), AiStatus::default());
}

#[tokio::test]
async fn a_key_or_code_of_the_persons_wins_and_remove_goes_back_to_the_free_credit() {
    let relay = MockServer::start().await;
    trial_registration(&relay, registered(NEW_TOKEN, 5.0, 0.0), 2).await;
    Mock::given(method("POST"))
        .and(path("/v1/devices"))
        .and(body_json(
            json!({ "device_id": DEVICE, "access_code": CODE }),
        ))
        .respond_with(registered(TOKEN, 2.0, 0.0))
        .expect(1)
        .mount(&relay)
        .await;
    let store = Arc::new(MemoryStore::default());
    store.set(META_API_KEY, &secret(KEY)).unwrap();
    store.set(RELAY_TOKEN, &secret(TOKEN)).unwrap();
    let credentials = on_trial(store.clone(), &relay);

    // The key wins: Meta directly, no free credit in the status, and no call to the relay to say so.
    let meta = credentials.meta().expect("the key's adapter");
    assert!(!meta.client().is_relay());
    assert_eq!(meta.client().used_up_message(), None);
    let status = credentials.status().await;
    assert_eq!((status.mode, status.trial), (AiMode::Key, false));

    // Remove: the free credit pays again, registered with its code.
    let trial = AiStatus {
        mode: AiMode::Relay,
        trial: true,
        budget_usd: Some(5.0),
        spent_usd: Some(0.0),
        active: Some(true),
        ..AiStatus::default()
    };
    assert_eq!(credentials.clear().await.unwrap(), trial);
    assert_eq!(stored(&store, META_API_KEY), None);
    assert_eq!(stored(&store, RELAY_CODE), None);
    assert!(
        credentials
            .meta()
            .expect("the free credit's adapter")
            .client()
            .is_relay()
    );

    // The person's own code wins too, and shows; its adapter says the relay's own sentences.
    let status = credentials.set_code(CODE).await.unwrap();
    assert_eq!((status.trial, status.code.as_deref()), (false, Some(CODE)));
    assert_eq!(stored(&store, RELAY_CODE).as_deref(), Some(CODE));
    assert_eq!(credentials.meta().unwrap().client().used_up_message(), None);

    // Removing it registers with the trial code again: the relay knew the device by the person's code.
    assert_eq!(credentials.clear().await.unwrap(), trial);
    assert_eq!(stored(&store, RELAY_CODE), None);
}

#[tokio::test]
async fn the_free_credit_used_up_or_turned_off_says_so() {
    let shared_used_up = json!({
        "error": "budget",
        "message": "Klipolo's shared AI credit is used up for now. Add your own Meta key in Settings."
    });
    let turned_off = json!({ "error": "invalid_code", "message": INVALID_CODE });
    for refusal in [
        ResponseTemplate::new(402).set_body_json(shared_used_up),
        ResponseTemplate::new(403).set_body_json(turned_off),
    ] {
        let relay = MockServer::start().await;
        trial_registration(&relay, refusal, 2).await;
        let credentials = on_trial(Arc::new(MemoryStore::default()), &relay)
            .with_register_backoff(Duration::ZERO);

        let ready = credentials.ready().await;
        assert!(ready.meta.is_none());
        assert_eq!(ready.problem.as_deref(), Some(USED_UP));
        let status = credentials.status().await;
        assert_eq!(
            status,
            AiStatus {
                mode: AiMode::Relay,
                trial: true,
                notice: Some(USED_UP.into()),
                ..AiStatus::default()
            }
        );
    }

    // Registered, then nothing left (the relay cuts the budget to the trial's), or turned off.
    for (relay_ai, budget, spent) in [(true, 5.0, 5.0), (true, 1.2, 1.2), (false, 0.0, 0.4)] {
        let relay = MockServer::start().await;
        let mut answer = device(relay_ai, budget, spent);
        answer["token"] = json!(NEW_TOKEN);
        trial_registration(&relay, ResponseTemplate::new(200).set_body_json(answer), 1).await;
        let credentials = on_trial(Arc::new(MemoryStore::default()), &relay);

        let status = credentials.status().await;
        assert_eq!(
            status.notice.as_deref(),
            Some(USED_UP),
            "{relay_ai} {budget} {spent}"
        );
        assert!(status.trial);
        assert_eq!(status.code, None);
    }
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
