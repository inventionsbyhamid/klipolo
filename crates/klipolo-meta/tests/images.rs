//! Muse Image (`POST /images/generations`) against a local mock server. No test reaches Meta or the relay: both base
//! URLs point at a mock or a closed port, and only the ledger test writes a spend ledger, to its own file.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use klipolo_meta::{
    pricing, GeneratedImage, ImageEffort, ImageSize, MetaClient, MetaError, SecretString,
    IMAGE_MODEL,
};
use serde_json::{json, Value};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const KEY: &str = "sk-meta-test-key-0123456789";
const TOKEN: &str = "relay-test-token-abcdefghij";
const CLOSED: &str = "http://127.0.0.1:9";

/// A 2x2 white PNG, as Muse Image sends them (no alpha).
const PNG: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 2, 0, 0, 0, 2, 8, 2, 0,
    0, 0, 253, 212, 154, 115, 0, 0, 0, 19, 73, 68, 65, 84, 120, 156, 99, 252, 255, 255, 63, 3, 3,
    3, 19, 3, 24, 0, 0, 36, 6, 3, 1, 93, 162, 78, 136, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96,
    130,
];

fn direct(server: &MockServer) -> MetaClient {
    MetaClient::direct(SecretString::from(KEY))
        .with_meta_base(format!("{}/v1", server.uri()))
        .with_relay_base(CLOSED)
        .with_ledger(None)
}

fn relay(server: &MockServer) -> MetaClient {
    MetaClient::relay(SecretString::from(TOKEN))
        .with_relay_base(server.uri())
        .with_meta_base(CLOSED)
        .with_ledger(None)
}

/// Meta's answer: one `data` item per PNG, each with its revised prompt.
fn answer(images: &[(&[u8], Option<&str>)]) -> Value {
    let data: Vec<Value> = images
        .iter()
        .map(|(png, revised)| {
            let mut item = json!({"b64_json": STANDARD.encode(png)});
            if let Some(revised) = revised {
                item["revised_prompt"] = json!(revised);
            }
            item
        })
        .collect();
    json!({
        "created": 1_793_400_000,
        "data": data,
        "output_format": "png",
        "usage": {"input_tokens": 40, "output_tokens": 4160, "total_tokens": 4200},
    })
}

async fn serve(server: &MockServer, route: &str, response: ResponseTemplate) {
    Mock::given(method("POST"))
        .and(path(route))
        .respond_with(response)
        .mount(server)
        .await;
}

async fn only_body(server: &MockServer) -> Value {
    let requests: Vec<Request> = server.received_requests().await.expect("recording is on");
    assert_eq!(requests.len(), 1, "expected one request");
    serde_json::from_slice(&requests[0].body).expect("a JSON body")
}

#[tokio::test]
async fn meta_client_parses_b64_png() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/images/generations"))
        .and(header("authorization", format!("Bearer {KEY}").as_str()))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(answer(&[(PNG, Some("A single paper plane sticker"))])),
        )
        .mount(&server)
        .await;

    let images = direct(&server)
        .generate_image("A single paper plane", ImageSize::Square, 1)
        .await
        .unwrap();

    assert_eq!(
        images,
        [GeneratedImage {
            png: PNG.to_vec(),
            revised_prompt: Some("A single paper plane sticker".into()),
        }]
    );
    assert_eq!(
        only_body(&server).await,
        json!({
            "model": "muse-image-1.0",
            "prompt": "A single paper plane",
            "n": 1,
            "size": "1024x1024",
            "response_format": "b64_json",
            "output_format": "png",
            "tool_enablement": {
                "enable_image_search": false,
                "enable_web_search": false,
                "enable_shell": false,
            },
        })
    );
}

#[tokio::test]
async fn relay_mode_asks_the_relays_mirror_with_the_device_token() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/meta/v1/images/generations"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(answer(&[(PNG, None)])))
        .mount(&server)
        .await;

    let images = relay(&server)
        .generate_image("Day 1 in Lisbon", ImageSize::Portrait, 1)
        .await
        .unwrap();

    assert_eq!(images.len(), 1);
    assert_eq!(images[0].revised_prompt, None);
    assert_eq!(only_body(&server).await["size"], "1024x1792");
}

#[test]
fn sizes_are_metas_aspect_ratios() {
    assert_eq!(ImageSize::Square.as_str(), "1024x1024");
    assert_eq!(ImageSize::Landscape.as_str(), "1792x1024");
    assert_eq!(ImageSize::Portrait.as_str(), "1024x1792");
}

#[tokio::test]
async fn low_effort_asks_for_one_pass() {
    let server = MockServer::start().await;
    serve(
        &server,
        "/v1/images/generations",
        ResponseTemplate::new(200).set_body_json(answer(&[(PNG, None), (PNG, None)])),
    )
    .await;

    let images = direct(&server)
        .generate_image_with("a coffee cup", ImageSize::Landscape, 2, ImageEffort::Low)
        .await
        .unwrap();

    assert_eq!(images.len(), 2);
    let body = only_body(&server).await;
    assert_eq!(body["reasoning_strength"], "low");
    assert_eq!(body["n"], 2);
    assert_eq!(body["size"], "1792x1024");
}

#[tokio::test]
async fn no_image_asked_for_is_refused_without_a_call() {
    let server = MockServer::start().await;
    let error = direct(&server)
        .generate_image("a cup", ImageSize::Square, 0)
        .await
        .unwrap_err();
    assert!(matches!(error, MetaError::BadRequest { .. }), "{error:?}");
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn an_answer_with_no_images_is_empty() {
    let server = MockServer::start().await;
    serve(
        &server,
        "/v1/images/generations",
        ResponseTemplate::new(200).set_body_json(json!({"created": 1, "data": []})),
    )
    .await;
    let images = direct(&server)
        .generate_image("a cup", ImageSize::Square, 1)
        .await
        .unwrap();
    assert!(images.is_empty());
}

#[tokio::test]
async fn an_image_that_is_not_a_png_is_a_decode_error() {
    for item in [
        json!({"b64_json": STANDARD.encode(b"GIF89a not a png")}),
        json!({"b64_json": "%%% not base64 %%%"}),
        json!({"url": "https://example.com/image.png"}),
    ] {
        let server = MockServer::start().await;
        serve(
            &server,
            "/v1/images/generations",
            ResponseTemplate::new(200).set_body_json(json!({"created": 1, "data": [item]})),
        )
        .await;
        let error = direct(&server)
            .generate_image("a cup", ImageSize::Square, 1)
            .await
            .unwrap_err();
        assert!(matches!(error, MetaError::Decode(_)), "{item}: {error:?}");
        assert_eq!(
            error.user_message(),
            "Meta sent an answer Klipolo could not read. Try again."
        );
    }
}

/// Meta answers a prompt it won't draw with 400 `content_policy_violation`; the relay passes that code on in its own
/// shape. Either way it is a refusal, not a malformed request.
#[tokio::test]
async fn a_content_policy_refusal_is_refused_directly_and_through_the_relay() {
    let reason = "Your request was rejected by the safety system.";
    let server = MockServer::start().await;
    serve(
        &server,
        "/v1/images/generations",
        ResponseTemplate::new(400).set_body_json(json!({"error": {
            "message": reason,
            "type": "invalid_request_error",
            "param": null,
            "code": "content_policy_violation",
        }})),
    )
    .await;
    let direct_error = direct(&server)
        .generate_image("something", ImageSize::Square, 1)
        .await
        .unwrap_err();

    let server = MockServer::start().await;
    serve(
        &server,
        "/meta/v1/images/generations",
        ResponseTemplate::new(400)
            .set_body_json(json!({"error": "content_policy_violation", "message": reason})),
    )
    .await;
    let relay_error = relay(&server)
        .generate_image("something", ImageSize::Square, 1)
        .await
        .unwrap_err();

    for error in [direct_error, relay_error] {
        assert_eq!(
            error,
            MetaError::Refused {
                message: Some(reason.into())
            }
        );
        assert_eq!(
            error.user_message(),
            "Meta turned this down under its content policy. Try wording it differently."
        );
        assert!(!error.is_busy());
    }
}

#[tokio::test]
async fn a_plain_bad_request_is_still_a_bad_request() {
    let server = MockServer::start().await;
    serve(
        &server,
        "/v1/images/generations",
        ResponseTemplate::new(400).set_body_json(json!({"error": {
            "message": "size: must be WIDTHxHEIGHT", "type": "invalid_request_error", "code": null,
        }})),
    )
    .await;
    let error = direct(&server)
        .generate_image("a cup", ImageSize::Square, 1)
        .await
        .unwrap_err();
    assert_eq!(
        error,
        MetaError::BadRequest {
            message: "size: must be WIDTHxHEIGHT".into()
        }
    );
}

#[tokio::test]
async fn each_answer_is_one_ledger_row_priced_per_image() {
    let server = MockServer::start().await;
    serve(
        &server,
        "/v1/images/generations",
        ResponseTemplate::new(200).set_body_json(answer(&[(PNG, None), (PNG, None)])),
    )
    .await;
    let ledger = std::env::temp_dir().join(format!(
        "klipolo-meta-images-ledger-{}.csv",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&ledger);

    direct(&server)
        .with_ledger(Some(ledger.clone()))
        .generate_image("a cup", ImageSize::Square, 2)
        .await
        .unwrap();

    let text = std::fs::read_to_string(&ledger).unwrap();
    let _ = std::fs::remove_file(&ledger);
    let rows: Vec<&str> = text.lines().collect();
    assert_eq!(rows.len(), 2, "{text}");
    let fields: Vec<&str> = rows[1].split(',').collect();
    assert_eq!(
        fields[1..],
        [
            IMAGE_MODEL,
            "images/generations",
            "",
            "",
            "2 images",
            "0.020000"
        ]
    );
    assert_eq!(pricing::image_usd(1), 0.01);
    assert_eq!(pricing::image_usd(2), 0.02);
}

/// Meta bills every image it returns, so an answer whose picture can't be read still leaves its ledger row.
#[tokio::test]
async fn an_unreadable_answer_is_still_in_the_ledger() {
    let server = MockServer::start().await;
    serve(
        &server,
        "/v1/images/generations",
        ResponseTemplate::new(200).set_body_json(json!({
            "created": 1,
            "data": [{"b64_json": STANDARD.encode(b"GIF89a not a png")}],
        })),
    )
    .await;
    let ledger = std::env::temp_dir().join(format!(
        "klipolo-meta-images-unreadable-{}.csv",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&ledger);

    let error = direct(&server)
        .with_ledger(Some(ledger.clone()))
        .generate_image("a cup", ImageSize::Square, 1)
        .await
        .unwrap_err();

    let text = std::fs::read_to_string(&ledger).unwrap_or_default();
    let _ = std::fs::remove_file(&ledger);
    assert!(matches!(error, MetaError::Decode(_)), "{error:?}");
    let rows: Vec<&str> = text.lines().collect();
    assert_eq!(rows.len(), 2, "{text}");
    assert!(rows[1].ends_with(",1 image,0.010000"), "{}", rows[1]);
}

#[test]
fn debug_output_shows_the_size_not_the_bytes() {
    let image = GeneratedImage {
        png: PNG.to_vec(),
        revised_prompt: None,
    };
    let text = format!("{image:?}");
    assert!(text.contains("76 bytes"), "{text}");
    assert!(!text.contains("137, 80"), "{text}");
}
