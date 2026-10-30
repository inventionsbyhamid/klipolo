//! The scene log: clip A's 7 hard cuts found to the frame, the small copy's chunks (made only on a Mac: VideoToolbox;
//! elsewhere the committed fixture stands in for it), and Spark's real answer read through the Meta adapter against a
//! mock Meta and relay, its requests within the relay's contract, one video call at a time, and the upload deleted on
//! every path.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use klipolo_analysis::cuts::{self, Finder, Histogram};
use klipolo_analysis::providers::meta::Meta;
use klipolo_analysis::providers::{ProviderErrorKind, SceneKind, SceneLogger, SceneMoment};
use klipolo_analysis::proxy;
use klipolo_meta::{MetaClient, SecretString};
use klipolo_model::time::{RangeUs, Us};
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

/// Clip A's 7 hard cuts, as frame numbers at 30 fps (13.47, 24.07, 52.07, 62.70, 86.07, 96.10, 150.67 s).
const CLIP_A_CUT_FRAMES: [i64; 7] = [404, 722, 1562, 1881, 2582, 2883, 4520];

/// One frame at 30 fps, rounded up.
const FRAME_US: Us = 33_334;

fn frame_us(frame: i64) -> Us {
    (frame as f64 * 1e6 / 30.0).round() as Us
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn clip_a() -> PathBuf {
    fixture("clip_a_360p.mp4")
}

fn assert_clip_a_cuts(cuts: &[Us]) {
    assert_eq!(cuts.len(), 7, "{cuts:?}");
    for (&found, &frame) in cuts.iter().zip(&CLIP_A_CUT_FRAMES) {
        let want = frame_us(frame);
        assert!(
            (found - want).abs() <= FRAME_US,
            "cut at {found} µs, want frame {frame} ({want} µs): {cuts:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Cuts

#[test]
fn cuts_detect_known_cuts_of_clip_a_within_one_frame() {
    let cuts = cuts::detect(&clip_a(), &AtomicBool::new(false)).unwrap();
    assert_clip_a_cuts(&cuts);
}

/// A histogram of 57,600 pixels (320 × 180) with `bright` of them in bin 50 and the rest in bin 10.
fn split(bright: u32) -> Histogram {
    let mut h = [0; cuts::BINS];
    h[50] = bright;
    h[10] = 57_600 - bright;
    h
}

#[test]
fn the_cut_finder_dates_a_hard_cut_to_its_frame_and_lets_a_fade_or_a_flicker_pass() {
    let mut finder = Finder::new();
    let at = |n: i64| frame_us(n);
    let mut n = 0;
    // 2 s dark, then a hard cut to bright at frame 60 (2.0 s).
    while n < 60 {
        finder.push_histogram(at(n), split(0));
        n += 1;
    }
    while n < 120 {
        finder.push_histogram(at(n), split(57_600));
        n += 1;
    }
    // A fade back to dark over half a second (15 frames): each frame moves 1/15 of the picture (0.067), a quarter
    // second about half of it.
    for step in 1..=15 {
        finder.push_histogram(at(n), split(57_600 - 3_840 * step));
        n += 1;
    }
    while n < 200 {
        finder.push_histogram(at(n), split(0));
        n += 1;
    }
    // Then a sudden change of 15% of the picture between two frames: under the threshold.
    while n < 260 {
        finder.push_histogram(at(n), split(8_640));
        n += 1;
    }
    // And a hard cut on the last frame.
    finder.push_histogram(at(n), split(57_600));
    assert_eq!(finder.finish(), vec![at(60), at(260)]);

    // The same distance the module documents: half the L1 distance, 0 to 1.
    assert_eq!(cuts::distance(&split(0), &split(57_600)), 1.0);
    assert!((cuts::distance(&split(0), &split(8_640)) - 0.15).abs() < 1e-9);
    let grey = vec![200u8; 57_600];
    let h = cuts::histogram(&grey);
    assert_eq!((h[50], h.iter().sum::<u32>()), (57_600, 57_600));
}

// ---------------------------------------------------------------------------------------------------------------------
// The copy's chunks

const S: Us = 1_000_000;

fn range(start_s: f64, end_s: f64) -> RangeUs {
    RangeUs {
        start: (start_s * 1e6).round() as Us,
        end: (end_s * 1e6).round() as Us,
    }
}

#[test]
fn chunks_cover_a_long_clip_evenly_with_twenty_seconds_of_overlap() {
    let (max, overlap) = (proxy::CHUNK_US, proxy::OVERLAP_US);
    assert_eq!(proxy::plan(155 * S, max, overlap), vec![range(0.0, 155.0)]);
    assert_eq!(proxy::plan(600 * S, max, overlap), vec![range(0.0, 600.0)]);
    assert_eq!(
        proxy::plan(610 * S, max, overlap),
        vec![range(0.0, 315.0), range(295.0, 610.0)]
    );
    // 25 minutes: three chunks of 513.33 s.
    let long = proxy::plan(1500 * S, max, overlap);
    assert_eq!(long.len(), 3, "{long:?}");
    assert_eq!((long[0].start, long[2].end), (0, 1500 * S));
    for pair in long.windows(2) {
        assert_eq!(pair[0].end - pair[1].start, overlap, "{long:?}");
    }
    for chunk in &long {
        assert!(chunk.end - chunk.start <= max, "{long:?}");
        // Equal to the microsecond, but for the rounding the last one takes up.
        assert!(
            (chunk.end - chunk.start - 513_333_334).abs() <= 3,
            "{long:?}"
        );
    }
}

#[test]
fn a_ready_copy_is_one_chunk_with_the_clips_facts() {
    let copy = proxy::existing(&clip_a()).unwrap();
    // The file's length: its sound ends 16 ms after its picture.
    assert!(
        (copy.duration_us - 155 * S).abs() < 50_000,
        "{}",
        copy.duration_us
    );
    assert_eq!(copy.chunks.len(), 1);
    assert_eq!(
        copy.chunks[0].range,
        RangeUs {
            start: 0,
            end: copy.duration_us
        }
    );
    assert_eq!(copy.chunks[0].path, clip_a());
    assert_eq!(
        copy.chunks[0].bytes,
        std::fs::metadata(clip_a()).unwrap().len()
    );
    assert_eq!(
        (copy.width, copy.height, copy.has_audio, copy.audio_lead_us),
        (640, 360, true, 0)
    );
    assert!((copy.fps - 30.0).abs() < 1e-6, "{}", copy.fps);
}

// ---------------------------------------------------------------------------------------------------------------------
// The small copy itself: VideoToolbox, so on a Mac only

/// Where `kind` (a top-level MP4 box) starts in `bytes`.
#[cfg(target_os = "macos")]
fn box_at(bytes: &[u8], kind: &[u8; 4]) -> Option<usize> {
    let mut at = 0;
    while at + 8 <= bytes.len() {
        let size = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        if &bytes[at + 4..at + 8] == kind {
            return Some(at);
        }
        if size < 8 {
            return None;
        }
        at += size;
    }
    None
}

#[cfg(target_os = "macos")]
#[test]
fn makes_a_small_copy_on_this_mac_and_finds_the_same_cuts_in_it() {
    use klipolo_engine::render::probe::probe_media;
    let cancel = AtomicBool::new(false);
    let dir = tempfile::tempdir().unwrap();
    let copy = proxy::make(&clip_a(), dir.path(), 360, &cancel).unwrap();
    assert_eq!(copy.chunks.len(), 1);
    assert_eq!((copy.width, copy.height, copy.has_audio), (640, 360, true));
    let chunk = &copy.chunks[0];
    assert_eq!(
        chunk.range,
        RangeUs {
            start: 0,
            end: copy.duration_us
        }
    );
    assert!(
        (copy.duration_us - 155 * S).abs() < 50_000,
        "{}",
        copy.duration_us
    );
    let bytes = std::fs::read(&chunk.path).unwrap();
    assert_eq!(chunk.bytes, bytes.len() as u64);
    eprintln!("clip A's copy at 640 x 360: {} bytes", bytes.len());
    // H.264 and AAC, its index before its media (faststart).
    assert!(bytes.windows(4).any(|w| w == b"avc1") && bytes.windows(4).any(|w| w == b"mp4a"));
    assert!(box_at(&bytes, b"moov").unwrap() < box_at(&bytes, b"mdat").unwrap());
    // About 300 kb/s of picture and 48 kb/s of sound.
    assert!(
        bytes.len() < 155 * (300_000 + 48_000) / 8 * 13 / 10,
        "{}",
        bytes.len()
    );
    let info = probe_media(&chunk.path).unwrap();
    assert!((info.duration - 155.0).abs() < 0.2, "{}", info.duration);
    assert_eq!(info.audio.map(|a| a.channels), Some(1));
    assert_clip_a_cuts(&cuts::of_proxy(&copy, &cancel).unwrap());

    // In four chunks of 53.75 s at 320 x 180, the cuts still land on their frames across the joins.
    let dir = tempfile::tempdir().unwrap();
    let chunked =
        proxy::make_in_chunks(&clip_a(), dir.path(), 180, (60 * S, 20 * S), &cancel).unwrap();
    assert_eq!(
        chunked.chunks.iter().map(|c| c.range).collect::<Vec<_>>(),
        proxy::plan(chunked.duration_us, 60 * S, 20 * S)
    );
    assert_eq!((chunked.width, chunked.height), (320, 180));
    for chunk in &chunked.chunks {
        eprintln!("chunk {:?}: {} bytes", chunk.range, chunk.bytes);
    }
    assert_clip_a_cuts(&cuts::of_proxy(&chunked, &cancel).unwrap());
}

// ---------------------------------------------------------------------------------------------------------------------
// The Meta adapter (mock Meta and relay)

const KEY: &str = "sk-scenes-test-key-0123456789";

const TOKEN: &str = "relay-scenes-token-0123456789";

fn spark_answer() -> Value {
    serde_json::from_str(&std::fs::read_to_string(fixture("spark_moments_clip_a.json")).unwrap())
        .unwrap()
}

fn direct(server: &MockServer) -> Meta {
    Meta::new(
        MetaClient::direct(SecretString::from(KEY))
            .with_meta_base(format!("{}/v1", server.uri()))
            .with_ledger(None),
    )
}

fn relay(server: &MockServer) -> Meta {
    Meta::new(
        MetaClient::relay(SecretString::from(TOKEN))
            .with_relay_base(server.uri())
            .with_ledger(None),
    )
}

/// Answers with `template`, noting when each request arrived.
struct Stamped {
    template: ResponseTemplate,
    arrivals: Arc<Mutex<Vec<Instant>>>,
}

impl Respond for Stamped {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        self.arrivals.lock().unwrap().push(Instant::now());
        self.template.clone()
    }
}

async fn mount_files(server: &MockServer, prefix: &str, id: &str) {
    Mock::given(method("POST"))
        .and(path(format!("{prefix}/files")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": id, "object": "file"})))
        .mount(server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(format!("{prefix}/files/{id}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": id, "deleted": true})))
        .expect(1)
        .mount(server)
        .await;
}

async fn requests(server: &MockServer, verb: &str, path_end: &str) -> Vec<Request> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.method.as_str() == verb && r.url.path().ends_with(path_end))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn parses_spike_moments_fixture() {
    let server = MockServer::start().await;
    mount_files(&server, "/v1", "file-moments").await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .and(header("authorization", format!("Bearer {KEY}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(spark_answer()))
        .expect(1)
        .mount(&server)
        .await;
    let meta = direct(&server);
    let answer = meta.watch(&clip_a(), 2.0).await.unwrap();
    server.verify().await;

    // 20 moments, in the order Spark gave them, kinds mapped from its vocabulary.
    let m = &answer.moments;
    assert_eq!(m.len(), 20);
    assert_eq!(
        m[0],
        SceneMoment {
            start_ms: 0,
            end_ms: 12_800,
            kind: SceneKind::Shot,
            who: "divers and astronauts".into(),
            text: "Overhead view of spacesuited astronauts training underwater with scuba divers around station mockup"
                .into(),
            on_screen_text: String::new(),
        }
    );
    let count = |kind: SceneKind| m.iter().filter(|x| x.kind == kind).count();
    // b_roll and scene_cut are shots; questions and answers are speech; gestures and the demonstration are actions; the
    // laugh is a reaction.
    assert_eq!(
        (
            count(SceneKind::Shot),
            count(SceneKind::Speech),
            count(SceneKind::Action),
            count(SceneKind::Reaction)
        ),
        (8, 8, 3, 1)
    );
    let arm = m.iter().find(|x| x.text.starts_with("Raises arm")).unwrap();
    assert_eq!(
        (arm.start_ms, arm.end_ms, arm.kind, arm.who.as_str()),
        (40_500, 45_000, SceneKind::Action, "male astronaut")
    );
    // Its usage at Spark's prices: 42,340 tokens in, 2,795 out.
    let cost = 42_340.0 * 1.25e-6 + 2_795.0 * 4.25e-6;
    assert!(
        (answer.cost_usd - cost).abs() < 1e-12,
        "{}",
        answer.cost_usd
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn scene_requests_keep_to_the_relays_contract() {
    let server = MockServer::start().await;
    mount_files(&server, "/meta/v1", "file-relay1").await;
    Mock::given(method("POST"))
        .and(path("/meta/v1/responses"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(spark_answer()))
        .expect(1)
        .mount(&server)
        .await;
    let meta = relay(&server);
    meta.watch(&clip_a(), 1.0).await.unwrap();
    server.verify().await;

    let uploads = requests(&server, "POST", "/meta/v1/files").await;
    assert_eq!(uploads.len(), 1);
    let form = String::from_utf8_lossy(&uploads[0].body);
    assert!(
        form.contains("user_data") && form.contains("video/mp4"),
        "{form:.300}"
    );
    assert!(form.contains("filename=\"upload.mp4\""), "{form:.300}");

    let sent = &requests(&server, "POST", "/meta/v1/responses").await[0];
    let body: Value = serde_json::from_slice(&sent.body).unwrap();
    let allowed = [
        "model",
        "input",
        "instructions",
        "tools",
        "tool_choice",
        "parallel_tool_calls",
        "text",
        "reasoning",
        "max_output_tokens",
        "temperature",
        "top_p",
        "stream",
        "store",
        "metadata",
        "user",
        "safety_identifier",
    ];
    for key in body.as_object().unwrap().keys() {
        assert!(
            allowed.contains(&key.as_str()),
            "{key} is not a field the relay takes"
        );
    }
    assert_eq!(body["model"], "muse-spark-1.3");
    assert_eq!(body["reasoning"], json!({"effort": "low"}));
    assert_eq!(body["max_output_tokens"], 8192);
    let format = &body["text"]["format"];
    assert_eq!(
        (format["type"].as_str(), format["strict"].as_bool()),
        (Some("json_schema"), Some(true))
    );
    let kinds = &format["schema"]["properties"]["moments"]["items"]["properties"]["kind"]["enum"];
    assert_eq!(
        kinds,
        &json!([
            "shot", "speech", "action", "reaction", "text", "topic", "other"
        ])
    );
    let content = body["input"][0]["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "input_text");
    assert_eq!(
        content[1],
        json!({"type": "input_video", "file_id": "file-relay1", "fps": 1.0})
    );
    let prompt = content[0]["text"].as_str().unwrap();
    assert!(
        !prompt.contains('\u{2014}') && !prompt.contains('\u{2013}'),
        "{prompt}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn uploaded_file_deleted_on_error() {
    // Meta fails the call: the upload is deleted, the error is Meta's outage, nothing is asked again.
    let server = MockServer::start().await;
    mount_files(&server, "/v1", "file-err1").await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({"error": {"message": "x"}})))
        .expect(1)
        .mount(&server)
        .await;
    let err = direct(&server).watch(&clip_a(), 2.0).await.unwrap_err();
    assert_eq!(err.kind, ProviderErrorKind::Unavailable);
    server.verify().await;

    // The relay's credit is used up: a stop, with the relay's sentence, and the upload deleted.
    let server = MockServer::start().await;
    mount_files(&server, "/meta/v1", "file-err2").await;
    let sentence = "This device used its Klipolo credit. Add your Meta API key in Settings.";
    Mock::given(method("POST"))
        .and(path("/meta/v1/responses"))
        .respond_with(
            ResponseTemplate::new(402)
                .set_body_json(json!({"error": "budget_exhausted", "message": sentence})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let err = relay(&server).watch(&clip_a(), 2.0).await.unwrap_err();
    assert_eq!(err.kind, ProviderErrorKind::Budget);
    assert!(err.is_stop());
    assert_eq!(err.user_message(), sentence);
    server.verify().await;

    // An answer that isn't the log asked for: the upload is deleted too.
    let server = MockServer::start().await;
    mount_files(&server, "/v1", "file-err3").await;
    let mut cut_off = spark_answer();
    cut_off["status"] = json!("incomplete");
    cut_off["output"][1]["content"][0]["text"] = json!("{\"moments\": [{\"start_sec\": 0.0, \"end");
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(cut_off))
        .expect(1)
        .mount(&server)
        .await;
    let err = direct(&server).watch(&clip_a(), 2.0).await.unwrap_err();
    assert_eq!(err.kind, ProviderErrorKind::Failed);
    assert!(!err.user_message().contains(KEY));
    server.verify().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn uploaded_file_deleted_when_cancelled() {
    let server = MockServer::start().await;
    mount_files(&server, "/v1", "file-cancel").await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(spark_answer())
                .set_delay(Duration::from_secs(20)),
        )
        .mount(&server)
        .await;
    let meta = direct(&server);
    let video = clip_a();
    let watched = tokio::time::timeout(Duration::from_millis(1_500), meta.watch(&video, 2.0)).await;
    assert!(watched.is_err(), "the call should still be waiting");
    let deadline = Instant::now() + Duration::from_secs(5);
    while requests(&server, "DELETE", "/files/file-cancel")
        .await
        .is_empty()
    {
        assert!(Instant::now() < deadline, "the upload was not deleted");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    server.verify().await;
}

/// Two chunks watched at once through clones of one adapter, each answer 400 ms late: through the relay the second
/// upload waits for the first answer; with a key both go at once.
#[tokio::test(flavor = "multi_thread")]
async fn one_video_call_at_a_time_through_the_relay() {
    for (relay_mode, at_once) in [(true, 1), (false, 2)] {
        let server = MockServer::start().await;
        let prefix = if relay_mode { "/meta/v1" } else { "/v1" };
        let uploads = Arc::new(Mutex::new(Vec::new()));
        let answers = Arc::new(Mutex::new(Vec::new()));
        Mock::given(method("POST"))
            .and(path(format!("{prefix}/files")))
            .respond_with(Stamped {
                template: ResponseTemplate::new(200).set_body_json(json!({"id": "file-twice"})),
                arrivals: uploads.clone(),
            })
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path(format!("{prefix}/files/file-twice")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"deleted": true})))
            .expect(2)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("{prefix}/responses")))
            .respond_with(Stamped {
                template: ResponseTemplate::new(200)
                    .set_body_json(spark_answer())
                    .set_delay(Duration::from_millis(400)),
                arrivals: answers.clone(),
            })
            .expect(2)
            .mount(&server)
            .await;
        let meta = if relay_mode {
            relay(&server)
        } else {
            direct(&server)
        };
        assert_eq!(meta.max_in_flight(), at_once);
        let other = meta.clone();
        let video = clip_a();
        let (a, b) = tokio::join!(meta.watch(&video, 2.0), other.watch(&video, 2.0));
        a.unwrap();
        b.unwrap();
        server.verify().await;
        let uploads = uploads.lock().unwrap().clone();
        let answers = answers.lock().unwrap().clone();
        let second_upload = *uploads.iter().max().unwrap();
        let first_answer = *answers.iter().min().unwrap();
        if relay_mode {
            assert!(
                second_upload >= first_answer + Duration::from_millis(400),
                "the second video went up while the first was being watched"
            );
        } else {
            assert!(
                second_upload < first_answer + Duration::from_millis(400),
                "with a key two videos go at once"
            );
        }
    }
}
