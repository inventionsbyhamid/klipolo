//! Muse Image behind `ImageMaker`, end to end through the Meta adapter against a local mock server: the request it
//! sends, the busy moments it waits out, and the one plain sentence each failure becomes. Nothing here reaches Meta or
//! the relay.

use std::time::{Duration, Instant};

use klipolo_analysis::providers::images::{ImageErrorKind, ImageMaker, ImageSize, MadeImage};
use klipolo_analysis::providers::meta::Meta;
use klipolo_analysis::providers::meta_images::{IMAGE_TIMEOUT, MADE_WITH, MetaImages, QUEUE_WAIT};
use klipolo_meta::{MetaClient, MetaError, SecretString};
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const KEY: &str = "sk-meta-test-key-0123456789";
const TOKEN: &str = "relay-test-token-abcdefghij";
const CLOSED: &str = "http://127.0.0.1:9";
const RELAY_ROUTE: &str = "/meta/v1/images/generations";
const META_ROUTE: &str = "/v1/images/generations";

/// The start of a PNG: enough for the client, which checks the signature and passes the bytes on.
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n not a whole picture";
/// `PNG` in base64, as Meta sends it.
const PNG_B64: &str = "iVBORw0KGgogbm90IGEgd2hvbGUgcGljdHVyZQ==";

fn relay_meta(server: &MockServer) -> Meta {
    Meta::new(
        MetaClient::relay(SecretString::from(TOKEN))
            .with_relay_base(server.uri())
            .with_meta_base(CLOSED)
            .with_ledger(None),
    )
}

fn relay(server: &MockServer) -> MetaImages {
    MetaImages::new(relay_meta(server))
}

/// Takes every one of the device's relay slots (3) for `long`, as a clip's analysis can, and returns once they are
/// held.
async fn hold_every_slot(meta: &Meta, long: Duration) {
    for _ in 0..3 {
        let meta = meta.clone();
        tokio::spawn(async move {
            let _ = meta
                .call(async {
                    tokio::time::sleep(long).await;
                    Ok::<(), MetaError>(())
                })
                .await;
        });
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
}

fn direct(server: &MockServer) -> MetaImages {
    MetaImages::new(Meta::new(
        MetaClient::direct(SecretString::from(KEY))
            .with_meta_base(format!("{}/v1", server.uri()))
            .with_relay_base(CLOSED)
            .with_ledger(None),
    ))
}

fn picture(revised: &str) -> Value {
    json!({"created": 1, "data": [{"b64_json": PNG_B64, "revised_prompt": revised}]})
}

async fn answering(route: &str, response: ResponseTemplate) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(route))
        .respond_with(response)
        .mount(&server)
        .await;
    server
}

/// One short plain sentence: a full stop at the end only, no em or en dashes.
fn assert_one_sentence(message: &str) {
    assert!(message.ends_with('.'), "{message}");
    assert!(!message.trim_end_matches('.').contains(". "), "{message}");
    assert!(
        !message.contains(['\u{2014}', '\u{2013}']),
        "dash in {message}"
    );
}

#[tokio::test]
async fn a_sticker_comes_back_as_a_png_through_the_relay() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(RELAY_ROUTE))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(picture("A paper plane sticker")))
        .mount(&server)
        .await;
    let maker = relay(&server);

    let made = maker
        .make("A single paper plane", ImageSize::Square, 1)
        .await
        .unwrap();

    assert_eq!(
        made,
        [MadeImage {
            png: PNG.to_vec(),
            revised_prompt: Some("A paper plane sticker".into()),
        }]
    );
    assert_eq!(maker.made_with(), "Muse Image");
    assert_eq!(MADE_WITH, "Muse Image");
    let requests = server.received_requests().await.unwrap();
    let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["model"], "muse-image-1.0");
    assert_eq!(body["size"], "1024x1024");
    assert_eq!(body["n"], 1);
    assert!(body.get("reasoning_strength").is_none());
}

#[tokio::test]
async fn title_card_shapes_and_low_effort_reach_the_request() {
    let server = answering(
        META_ROUTE,
        ResponseTemplate::new(200).set_body_json(picture("A title card")),
    )
    .await;
    let maker = direct(&server).fast();

    maker
        .make("Day 1 in Lisbon", ImageSize::Portrait, 1)
        .await
        .unwrap();
    maker
        .make("Day 1 in Lisbon", ImageSize::Landscape, 1)
        .await
        .unwrap();

    let requests = server.received_requests().await.unwrap();
    let sizes: Vec<Value> = requests
        .iter()
        .map(|r| serde_json::from_slice::<Value>(&r.body).unwrap())
        .map(|b| {
            assert_eq!(b["reasoning_strength"], "low");
            b["size"].clone()
        })
        .collect();
    assert_eq!(sizes, [json!("1024x1792"), json!("1792x1024")]);
}

/// Traefik answers the relay's per-IP limit in plain text, with no code: the adapter waits a moment and asks again.
#[tokio::test]
async fn a_plain_text_429_is_waited_out_and_sent_again() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(RELAY_ROUTE))
        .respond_with(ResponseTemplate::new(429).set_body_raw("Too Many Requests", "text/plain"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(RELAY_ROUTE))
        .respond_with(ResponseTemplate::new(200).set_body_json(picture("A cup")))
        .mount(&server)
        .await;

    let made = relay(&server)
        .make("A coffee cup", ImageSize::Square, 1)
        .await
        .unwrap();

    assert_eq!(made.len(), 1);
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn still_busy_after_three_tries_says_so_in_one_sentence() {
    let busy = json!({"error": "busy", "message": "Too many requests at once from this device. Wait for one to finish."});
    let server = answering(
        RELAY_ROUTE,
        ResponseTemplate::new(429)
            .insert_header("retry-after", "1")
            .set_body_json(busy),
    )
    .await;

    let error = relay(&server)
        .make("A coffee cup", ImageSize::Square, 1)
        .await
        .unwrap_err();

    assert_eq!(error.kind, ImageErrorKind::Busy);
    assert_eq!(
        error.user_message(),
        "Muse Image is busy right now, so try again in a moment."
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
}

/// 402, 429, 503, a refused prompt and the rest each come back as one plain sentence.
#[tokio::test]
async fn every_failure_is_one_plain_sentence() {
    let relay_budget = json!({"error": "budget", "message": "This device has used its Klipolo credit. Add your own Meta key in Settings."});
    let meta_limit = json!({"error": {"code": "rate_limit_exceeded", "message": "Rate limit exceeded. Please retry after 15 seconds.", "type": "rate_limit_error"}});
    let overloaded = json!({"error": {"code": "service_overloaded", "message": "The backend is temporarily overloaded. Please retry.", "type": "server_error"}});
    let refused = json!({"error": {"code": "content_policy_violation", "message": "Rejected by the safety system.", "type": "invalid_request_error"}});
    let relay_refused =
        json!({"error": "content_policy_violation", "message": "Rejected by the safety system."});
    let cases: Vec<(bool, ResponseTemplate, ImageErrorKind, &str)> = vec![
        (
            true,
            ResponseTemplate::new(402).set_body_json(relay_budget),
            ImageErrorKind::OutOfCredit,
            "Klipolo's credit for Muse Image has run out, so add your own Meta key in Settings to keep going.",
        ),
        (
            false,
            ResponseTemplate::new(402).set_body_json(json!({"error": {"code": "billing_not_configured", "message": "Billing account issue."}})),
            ImageErrorKind::OutOfCredit,
            "Your Meta account is out of credit or has a billing problem, so check it on Meta's site.",
        ),
        (
            false,
            ResponseTemplate::new(429)
                .insert_header("retry-after", "15")
                .set_body_json(meta_limit),
            ImageErrorKind::Busy,
            "Muse Image is busy right now, so try again in a moment.",
        ),
        (
            false,
            ResponseTemplate::new(503).set_body_json(overloaded),
            ImageErrorKind::Busy,
            "Muse Image is busy right now, so try again in a moment.",
        ),
        (
            false,
            ResponseTemplate::new(400).set_body_json(refused),
            ImageErrorKind::Refused,
            "Muse Image won't draw that, so try describing it another way.",
        ),
        (
            true,
            ResponseTemplate::new(400).set_body_json(relay_refused),
            ImageErrorKind::Refused,
            "Muse Image won't draw that, so try describing it another way.",
        ),
        (
            false,
            ResponseTemplate::new(504).set_body_json(json!({"error": {"code": "gateway_timeout", "message": "Request exceeded the non-streaming server time limit before a response was produced.", "type": "server_error"}})),
            ImageErrorKind::Timeout,
            "Muse Image took too long to answer, so try again.",
        ),
        (
            true,
            ResponseTemplate::new(504).set_body_json(json!({"error": "meta_timeout", "message": "Meta took too long to answer. Try again."})),
            ImageErrorKind::Timeout,
            "Muse Image took too long to answer, so try again.",
        ),
        (
            true,
            ResponseTemplate::new(408).set_body_json(json!({"error": "request_timeout", "message": "The request took too long to arrive. Try again."})),
            ImageErrorKind::Timeout,
            "Muse Image took too long to answer, so try again.",
        ),
        (
            true,
            ResponseTemplate::new(503).set_body_json(json!({"error": "not_configured", "message": "Relay not configured."})),
            ImageErrorKind::Other,
            "Klipolo's AI service isn't set up yet, so try again later or add your own Meta key in Settings.",
        ),
        (
            true,
            ResponseTemplate::new(401),
            ImageErrorKind::Other,
            "Your Meta key or access code wasn't accepted, so check it in Settings.",
        ),
        (
            false,
            ResponseTemplate::new(400).set_body_json(json!({"error": {"message": "size is not valid"}})),
            ImageErrorKind::Other,
            "Muse Image couldn't make that picture, so try again.",
        ),
        (
            false,
            ResponseTemplate::new(200).set_body_json(json!({"created": 1, "data": []})),
            ImageErrorKind::Other,
            "Muse Image didn't send a picture back, so try again.",
        ),
    ];
    for (through_relay, response, kind, sentence) in cases {
        let server = answering(
            if through_relay {
                RELAY_ROUTE
            } else {
                META_ROUTE
            },
            response,
        )
        .await;
        let maker = if through_relay {
            relay(&server)
        } else {
            direct(&server)
        };
        let error = maker
            .make("A coffee cup", ImageSize::Square, 1)
            .await
            .unwrap_err();
        assert_eq!((error.kind, error.user_message()), (kind, sentence));
        assert_one_sentence(error.user_message());
        assert!(!error.to_string().contains(KEY) && !error.to_string().contains(TOKEN));
        // A final answer is never sent again on its own: the person decides with Try again.
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            1,
            "{sentence}"
        );
    }
}

#[tokio::test]
async fn an_unreachable_service_says_to_check_the_connection() {
    let maker = MetaImages::new(Meta::new(
        MetaClient::direct(SecretString::from(KEY))
            .with_meta_base(format!("{CLOSED}/v1"))
            .with_ledger(None),
    ));
    let error = maker
        .make("A coffee cup", ImageSize::Square, 1)
        .await
        .unwrap_err();
    assert_eq!(error.kind, ImageErrorKind::Other);
    assert_eq!(
        error.user_message(),
        "Klipolo couldn't reach Muse Image, so check your internet connection and try again."
    );
}

#[tokio::test]
async fn a_slow_answer_times_out_with_one_sentence() {
    let server = answering(
        RELAY_ROUTE,
        ResponseTemplate::new(200)
            .set_body_json(picture("late"))
            .set_delay(Duration::from_secs(10)),
    )
    .await;
    let started = Instant::now();

    let error = relay(&server)
        .with_timeout(Duration::from_millis(300))
        .make("A coffee cup", ImageSize::Square, 1)
        .await
        .unwrap_err();

    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(error.kind, ImageErrorKind::Timeout);
    assert_eq!(
        error.user_message(),
        "Muse Image took too long to answer, so try again."
    );
    assert_eq!(IMAGE_TIMEOUT, Duration::from_secs(120));
}

/// The deadline starts once the request holds a slot: a picture queued behind three long calls waits its turn, then
/// gets its whole time.
#[tokio::test]
async fn a_picture_queued_behind_busy_slots_gets_its_full_time() {
    let server = answering(
        RELAY_ROUTE,
        ResponseTemplate::new(200)
            .set_body_json(picture("A cup"))
            .set_delay(Duration::from_millis(400)),
    )
    .await;
    let meta = relay_meta(&server);
    hold_every_slot(&meta, Duration::from_millis(800)).await;
    let started = Instant::now();

    let made = MetaImages::new(meta)
        .with_timeout(Duration::from_millis(600))
        .with_queue_wait(Duration::from_secs(5))
        .make("A coffee cup", ImageSize::Square, 1)
        .await;

    assert_eq!(made.unwrap().len(), 1);
    assert!(
        started.elapsed() >= Duration::from_millis(1100),
        "{:?}",
        started.elapsed()
    );
}

/// A request that can't get a slot in time is busy: nothing was sent, so nothing was spent.
#[tokio::test]
async fn a_picture_that_cannot_get_a_slot_in_time_is_busy_and_never_sent() {
    let server = answering(
        RELAY_ROUTE,
        ResponseTemplate::new(200).set_body_json(picture("A cup")),
    )
    .await;
    let meta = relay_meta(&server);
    hold_every_slot(&meta, Duration::from_secs(5)).await;

    let error = MetaImages::new(meta)
        .with_queue_wait(Duration::from_millis(300))
        .make("A coffee cup", ImageSize::Square, 1)
        .await
        .unwrap_err();

    assert_eq!(error.kind, ImageErrorKind::Busy);
    assert_eq!(
        error.user_message(),
        "Muse Image is busy right now, so try again in a moment."
    );
    assert!(server.received_requests().await.unwrap().is_empty());
    assert_eq!(QUEUE_WAIT, Duration::from_secs(60));
}

#[tokio::test]
async fn try_again_after_a_failure_works() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(RELAY_ROUTE))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(RELAY_ROUTE))
        .respond_with(ResponseTemplate::new(200).set_body_json(picture("A cup")))
        .mount(&server)
        .await;
    let maker = relay(&server);

    let first = maker.make("A coffee cup", ImageSize::Square, 1).await;
    let again = maker.make("A coffee cup", ImageSize::Square, 1).await;

    assert_eq!(first.unwrap_err().kind, ImageErrorKind::Busy);
    assert_eq!(again.unwrap().len(), 1);
}
