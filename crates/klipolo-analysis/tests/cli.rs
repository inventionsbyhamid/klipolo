//! The dev CLI `klipolo-analyze transcript <video>` and `scenes <video>`, run as a process against a mock Meta and
//! relay: it writes `transcript.json` or `scenes.json`, prints the cost, logs spend only when asked, and never prints
//! the key or token. The test of a clip without sound needs `scripts/gen-engine-fixtures.sh` (the gate runs it with
//! `--ignored`).

#[path = "../../klipolo-engine/tests/support/fixture_set.rs"]
mod fixture_set;

use std::path::Path;
use std::process::{Command, Output};

use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const KEY: &str = "sk-cli-test-key-0123456789";
const TOKEN: &str = "relay-cli-token-0123456789";

/// 5 s of a 220 Hz tone as a 16 kHz mono WAV: a clip with sound.
fn write_clip(path: &Path) {
    let samples: Vec<i16> = (0..80_000)
        .map(|i| {
            (10_000.0 * (2.0 * std::f64::consts::PI * 220.0 * i as f64 / 16_000.0).sin()) as i16
        })
        .collect();
    let data = (samples.len() * 2) as u32;
    let mut w = Vec::new();
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    // PCM, mono, 16 kHz, 32000 bytes a second, 2 bytes a frame, 16 bits.
    w.extend_from_slice(&[1, 0, 1, 0]);
    w.extend_from_slice(&16_000u32.to_le_bytes());
    w.extend_from_slice(&32_000u32.to_le_bytes());
    w.extend_from_slice(&[2, 0, 16, 0]);
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        w.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, w).unwrap();
}

async fn run(env: &[(&str, String)], args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_klipolo-analyze"));
    command
        .args(args)
        .env_remove("MODEL_API_KEY")
        .env_remove("KLIPOLO_RELAY_TOKEN")
        .env_remove("KLIPOLO_SPEND_LEDGER")
        .env_remove("KLIPOLO_META_BASE")
        .env_remove("KLIPOLO_RELAY_BASE");
    for (k, v) in env {
        command.env(k, v);
    }
    tokio::task::spawn_blocking(move || command.output().unwrap())
        .await
        .unwrap()
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_needs_a_video_and_a_key_or_token() {
    let out = run(&[], &[]).await;
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out).contains("usage: klipolo-analyze transcript <video>"));

    let out = run(&[], &["transcript", "clip.mp4"]).await;
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out).contains("Set MODEL_API_KEY or KLIPOLO_RELAY_TOKEN."));

    let out = run(&[], &["scenes"]).await;
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out).contains("klipolo-analyze scenes <video>"));
    let out = run(&[], &["scenes", "clip.mp4"]).await;
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out).contains("Set MODEL_API_KEY or KLIPOLO_RELAY_TOKEN."));
}

/// `scenes` with a ready copy (Linux makes none): Spark's real answer for clip A through a mock Meta, snapped to the
/// cuts found in the copy and to the words of a transcript file.
#[tokio::test(flavor = "multi_thread")]
async fn cli_logs_the_scenes_of_a_ready_copy_and_prints_the_cost() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let clip = fixtures.join("clip_a_360p.mp4");
    let spark: Value =
        serde_json::from_slice(&std::fs::read(fixtures.join("spark_moments_clip_a.json")).unwrap())
            .unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/files"))
        .and(header("authorization", format!("Bearer {KEY}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "file-cli"})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(spark))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/v1/files/file-cli"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"deleted": true})))
        .expect(1)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    // A transcript with one word in it, as `transcript` writes it (only its words are read).
    let transcript = dir.path().join("transcript.json");
    std::fs::write(
        &transcript,
        json!({"transcript": {"words": [
            {"text": "Hi", "startUs": 40_300_000, "endUs": 40_600_000, "confidence": 0.9, "kind": "word"}
        ]}})
        .to_string(),
    )
    .unwrap();
    let ledger = dir.path().join("ledger.csv");
    let folder = dir.path().join("out");
    let env = [
        ("MODEL_API_KEY", KEY.to_owned()),
        ("KLIPOLO_META_BASE", format!("{}/v1", server.uri())),
        ("KLIPOLO_SPEND_LEDGER", ledger.display().to_string()),
    ];
    let out = run(
        &env,
        &[
            "scenes",
            clip.to_str().unwrap(),
            "--proxy",
            clip.to_str().unwrap(),
            "--transcript",
            transcript.to_str().unwrap(),
            "--out",
            folder.to_str().unwrap(),
        ],
    )
    .await;
    let printed = text(&out);
    assert!(out.status.success(), "{printed}");
    server.verify().await;
    assert!(printed.contains("estimate: $0.065073"), "{printed}");
    assert!(printed.contains("cost: $0.064804"), "{printed}");
    assert!(printed.contains("cuts: 7"), "{printed}");
    assert!(printed.contains("items: 20"), "{printed}");
    assert!(!printed.contains(KEY), "{printed}");
    let saved: Value =
        serde_json::from_slice(&std::fs::read(folder.join("scenes.json")).unwrap()).unwrap();
    assert_eq!(saved["model"], "muse-spark-1.3");
    assert_eq!(saved["fps"], 2.0);
    assert_eq!(saved["cuts"].as_array().unwrap().len(), 7);
    let items = saved["items"].as_array().unwrap();
    assert_eq!(items.len(), 20);
    // The arm raise starts at 40.5 s: the word at 40.3 s is the nearest edge within 1 s (no cut is near).
    let arm = items
        .iter()
        .find(|i| i["text"].as_str().unwrap().starts_with("Raises arm"))
        .unwrap();
    assert_eq!(
        (arm["startUs"].as_i64(), arm["kind"].as_str()),
        (Some(40_300_000), Some("action"))
    );
    let rows = std::fs::read_to_string(&ledger).unwrap();
    assert!(
        rows.contains(",muse-spark-1.3,responses,42340,2795,"),
        "{rows}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_writes_the_transcript_prints_the_cost_and_logs_spend_only_when_asked() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/asr/transcribe"))
        .and(header("authorization", format!("Bearer {KEY}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "audioDurationMs": 5000,
            "turns": [{"turnId": 0, "startMs": 500, "endMs": 4000,
                       "transcript": "Um, hello there, uh, friends.", "speaker": "A"}]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/align"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "words": [
                {"text": "Um,", "startMs": 250, "endMs": 600, "score": 0.9, "turn": 0},
                {"text": "hello", "startMs": 700, "endMs": 1100, "score": 0.95, "turn": 0},
                {"text": "there,", "startMs": 1150, "endMs": 1500, "score": 0.93, "turn": 0},
                {"text": "uh,", "startMs": 2400, "endMs": 2700, "score": 0.8, "turn": 0},
                {"text": "friends.", "startMs": 2800, "endMs": 3600, "score": 0.97, "turn": 0}
            ],
            "skipped": []
        })))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let clip = dir.path().join("clip.wav");
    write_clip(&clip);
    let ledger = dir.path().join("ledger.csv");
    let folder = dir.path().join("out");
    let env = |with_ledger: bool| {
        let mut env = vec![
            ("MODEL_API_KEY", KEY.to_owned()),
            ("KLIPOLO_RELAY_TOKEN", TOKEN.to_owned()),
            ("KLIPOLO_META_BASE", format!("{}/v1", server.uri())),
            ("KLIPOLO_RELAY_BASE", server.uri()),
        ];
        if with_ledger {
            env.push(("KLIPOLO_SPEND_LEDGER", ledger.display().to_string()));
        }
        env
    };
    let args = [
        "transcript",
        clip.to_str().unwrap(),
        "--out",
        folder.to_str().unwrap(),
    ];

    // Without the ledger variable: the transcript and the cost, no ledger file.
    let out = run(&env(false), &args).await;
    let printed = text(&out);
    assert!(out.status.success(), "{printed}");
    assert!(printed.contains("cost: $0.000250"), "{printed}");
    assert!(printed.contains("word timing: exact"), "{printed}");
    assert!(
        !printed.contains(KEY) && !printed.contains(TOKEN),
        "{printed}"
    );
    assert!(!ledger.exists());
    let saved: Value =
        serde_json::from_slice(&std::fs::read(folder.join("transcript.json")).unwrap()).unwrap();
    assert_eq!(saved["transcript"]["model"], "muse-voice-transcribe-1.0");
    assert_eq!(saved["transcript"]["key"].as_str().unwrap().len(), 64);
    assert_eq!(saved["approximate"], false);
    let words = saved["transcript"]["words"].as_array().unwrap();
    let kinds: Vec<(&str, &str)> = words
        .iter()
        .map(|w| (w["text"].as_str().unwrap(), w["kind"].as_str().unwrap()))
        .collect();
    assert_eq!(
        kinds,
        [
            ("Um,", "filler"),
            ("hello", "word"),
            ("there,", "word"),
            ("", "pause"),
            ("uh,", "filler"),
            ("friends.", "word")
        ]
    );
    // The align request's WAV starts 0.25 s in: "hello" is at 0.95 s of the clip.
    assert_eq!(words[1]["startUs"], 950_000);

    // With it: one row for the paid transcription (alignment is free to the person).
    let out = run(&env(true), &args).await;
    assert!(out.status.success(), "{}", text(&out));
    let rows = std::fs::read_to_string(&ledger).unwrap();
    let lines: Vec<&str> = rows.lines().collect();
    assert_eq!(lines.len(), 2, "{rows}");
    assert!(lines[0].starts_with("time_utc,model,purpose"));
    assert!(
        lines[1].contains(",muse-voice-transcribe-1.0,asr/transcribe,,,5.0s,"),
        "{rows}"
    );
    assert!(!rows.contains(KEY) && !rows.contains(TOKEN));
}

/// A clip with no sound makes no request and writes an empty transcript covering the whole clip.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs scripts/gen-engine-fixtures.sh"]
async fn cli_writes_an_empty_transcript_for_a_clip_without_sound() {
    let video = fixture_set::fixture_dir().join("camera-720p.mp4");
    assert!(video.exists(), "run scripts/gen-engine-fixtures.sh");
    let length_us = (klipolo_engine::render::probe::probe_media(&video)
        .unwrap()
        .duration
        * 1e6)
        .round() as i64;
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("out");
    let env = [
        ("MODEL_API_KEY", KEY.to_owned()),
        ("KLIPOLO_META_BASE", "http://127.0.0.1:9/v1".to_owned()),
        ("KLIPOLO_RELAY_BASE", "http://127.0.0.1:9".to_owned()),
    ];

    let out = run(
        &env,
        &[
            "transcript",
            video.to_str().unwrap(),
            "--out",
            folder.to_str().unwrap(),
        ],
    )
    .await;

    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("No speech found."), "{}", text(&out));
    let saved: Value =
        serde_json::from_slice(&std::fs::read(folder.join("transcript.json")).unwrap()).unwrap();
    assert!(length_us > 0);
    assert_eq!(saved["transcript"]["coveredUs"], length_us);
    assert_eq!(saved["transcript"]["complete"], true);
    assert_eq!(saved["transcript"]["words"], json!([]));
    assert_eq!(saved["emptyReason"], "No speech found.");
}
