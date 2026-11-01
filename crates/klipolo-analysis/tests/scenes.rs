//! The scene log: clip A's 7 hard cuts found to the frame, the small copy's chunks (made only on a Mac: VideoToolbox;
//! elsewhere the committed fixture stands in for it), Spark's real answer read through the Meta adapter against a mock
//! Meta and relay (the upload deleted on every path), and chunk answers merged and snapped to cuts and words.

#[cfg(target_os = "macos")]
#[path = "../../klipolo-engine/tests/support/fixture_set.rs"]
mod fixture_set;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use klipolo_analysis::cuts::{self, Finder, Histogram};
use klipolo_analysis::providers::meta::Meta;
use klipolo_analysis::providers::{
    BoxFuture, ProviderError, ProviderErrorKind, SceneAnswer, SceneKind, SceneLogger, SceneMoment,
};
use klipolo_analysis::proxy::{self, ProxyChunk, ProxyInfo};
use klipolo_analysis::scenes::{self, SceneItem, WordEdges};
use klipolo_engine::media::small_copy::EveryFrame;
use klipolo_meta::{MetaClient, SecretString};
use klipolo_model::speech::{Word, WordKind};
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
fn the_cut_finder_dates_a_hard_cut_to_its_frame_and_lets_a_fade_or_a_small_change_pass() {
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

/// Frames at 30 fps from `from` (a frame number) for as long as `looks` lasts, each its own look.
fn push_looks(finder: &mut Finder, from: i64, looks: &[Histogram]) -> i64 {
    for (k, look) in looks.iter().enumerate() {
        finder.push_histogram(frame_us(from + k as i64), *look);
    }
    from + looks.len() as i64
}

#[test]
fn a_flash_across_a_sample_boundary_is_not_a_cut() {
    let (dark, white) = (split(0), split(57_600));
    let mut finder = Finder::new();
    // Dark, then 3 white frames (59 to 61: 1.967 s to 2.033 s) across the 2.0 s sample, then dark again; then a hard
    // cut at frame 90 (3.0 s) that stays.
    let mut n = push_looks(&mut finder, 0, &[dark; 59]);
    n = push_looks(&mut finder, n, &[white; 3]);
    n = push_looks(&mut finder, n, &[dark; 28]);
    push_looks(&mut finder, n, &[white; 30]);
    assert_eq!(finder.finish(), vec![frame_us(90)]);
}

#[test]
fn a_flicker_that_returns_to_the_old_look_is_not_a_cut() {
    let (dark, bright) = (split(0), split(57_600));
    let mut finder = Finder::new();
    // Dark, then 6 frames flickering bright and dark (1.0 s to 1.167 s) across the 1.0 s sample, then dark for 2 s.
    let mut n = push_looks(&mut finder, 0, &[dark; 30]);
    n = push_looks(&mut finder, n, &[bright, dark, bright, dark, bright, dark]);
    push_looks(&mut finder, n, &[dark; 60]);
    assert_eq!(finder.finish(), Vec::<Us>::new());
}

#[test]
fn cuts_merge_keeps_each_chunks_cuts_and_overlaps_once() {
    let ms = |ms: Us| ms * 1_000;
    let merged = cuts::merge(vec![
        (
            RangeUs {
                start: 0,
                end: ms(60_000),
            },
            vec![ms(10_000), ms(55_000), ms(59_990)],
        ),
        (
            RangeUs {
                start: ms(40_000),
                end: ms(100_000),
            },
            // 35 s is before this chunk; 55.02 s is the first chunk's 55 s found again; 100 s is its end, not in it.
            vec![ms(35_000), ms(41_000), ms(55_020), ms(61_000), ms(100_000)],
        ),
    ]);
    assert_eq!(
        merged,
        vec![ms(10_000), ms(41_000), ms(55_000), ms(59_990), ms(61_000)]
    );
}

#[test]
fn a_finder_told_to_start_again_forgets_what_it_saw() {
    let (dark, bright) = (split(0), split(57_600));
    let mut finder = Finder::new();
    // A cut at frame 30, then the copy is made again from the start: only the second run's cut (frame 60) counts.
    let n = push_looks(&mut finder, 0, &[dark; 30]);
    push_looks(&mut finder, n, &[bright; 30]);
    EveryFrame::again(&mut finder);
    let n = push_looks(&mut finder, 0, &[dark; 60]);
    push_looks(&mut finder, n, &[bright; 30]);
    assert_eq!(finder.finish(), vec![frame_us(60)]);
    assert_eq!(EveryFrame::size(&Finder::new()), cuts::PICTURE);
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
    // Found by decoding it at its own rate.
    assert_clip_a_cuts(&copy.cuts);
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
    let per_ten_minutes = bytes.len() as f64 / 155.0 * 600.0;
    eprintln!(
        "clip A's copy at 640 x 360: {} bytes, {:.2} MB per 10 minutes",
        bytes.len(),
        per_ten_minutes / 1e6
    );
    // H.264 and AAC, its index before its media (faststart).
    assert!(bytes.windows(4).any(|w| w == b"avc1") && bytes.windows(4).any(|w| w == b"mp4a"));
    assert!(box_at(&bytes, b"moov").unwrap() < box_at(&bytes, b"mdat").unwrap());
    // Its bitrates, and a ten-minute chunk small enough for a slow link: at most 10 MB.
    let budget = 155 * (proxy::VIDEO_BPS + proxy::AUDIO_BPS) as usize / 8;
    assert!(
        bytes.len() < budget * 13 / 10,
        "{} over {budget}",
        bytes.len()
    );
    assert!(per_ten_minutes < 10e6, "{per_ten_minutes}");
    // 4 frames a second, whatever the source's rate (clip A's is 30), while its cuts come from every source frame.
    let info = probe_media(&chunk.path).unwrap();
    assert!((info.duration - 155.0).abs() < 0.3, "{}", info.duration);
    assert!((info.fps - 4.0).abs() < 0.05, "{}", info.fps);
    assert_eq!(copy.fps, 4.0);
    assert_eq!(info.audio.map(|a| a.channels), Some(1));
    assert_clip_a_cuts(&copy.cuts);

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
    assert_clip_a_cuts(&chunked.cuts);
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
async fn parses_a_real_moments_answer() {
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
    assert_eq!(body["max_output_tokens"], 16384);
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

/// The call is dropped while its upload's delete waits for an answer: the delete is sent again from a task of its own
/// (a file already gone counts as deleted).
#[tokio::test(flavor = "multi_thread")]
async fn upload_deleted_when_cancelled_during_delete() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/files"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "file-slow-delete"})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(spark_answer()))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/v1/files/file-slow-delete"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"deleted": true}))
                .set_delay(Duration::from_secs(3)),
        )
        .mount(&server)
        .await;
    let meta = direct(&server);
    let video = clip_a();
    let watched = tokio::time::timeout(Duration::from_millis(1_500), meta.watch(&video, 2.0)).await;
    assert!(watched.is_err(), "the call should still be deleting");
    assert_eq!(
        requests(&server, "DELETE", "/files/file-slow-delete")
            .await
            .len(),
        1
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while requests(&server, "DELETE", "/files/file-slow-delete")
        .await
        .len()
        < 2
    {
        assert!(Instant::now() < deadline, "the delete was not sent again");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The call is dropped while its file is still going up: no id has come back, so there is nothing it can delete (Meta
/// expires an upload after a day), and nothing else is sent.
#[tokio::test(flavor = "multi_thread")]
async fn a_call_dropped_during_its_upload_sends_nothing_more() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/files"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id": "file-never-known"}))
                .set_delay(Duration::from_secs(5)),
        )
        .mount(&server)
        .await;
    let meta = direct(&server);
    let video = clip_a();
    let watched = tokio::time::timeout(Duration::from_millis(1_000), meta.watch(&video, 2.0)).await;
    assert!(watched.is_err(), "the call should still be uploading");
    tokio::time::sleep(Duration::from_millis(500)).await;
    let sent: Vec<String> = server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .map(|r| format!("{} {}", r.method, r.url.path()))
        .collect();
    assert_eq!(sent, ["POST /v1/files"]);
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

// ---------------------------------------------------------------------------------------------------------------------
// Frame rate and cost

#[test]
fn fps_and_estimate_follow_the_clip_length() {
    assert_eq!(scenes::fps_for(155 * S), 2.0);
    assert_eq!(scenes::fps_for(300 * S), 2.0);
    assert_eq!(scenes::fps_for(300 * S + 1), 1.0);
    let meta = Meta::new(MetaClient::direct(SecretString::from(KEY)));
    // Clip A: 310 frames × 131 + 1,550 s of sound × 10... + 450 prompt in; 1,500 + 8 × 155 out.
    let clip_a = (42_610.0 * 1.25 + 2_740.0 * 4.25) / 1e6;
    assert!((scenes::estimate_usd(&meta, 155 * S) - clip_a).abs() < 1e-12);
    // 25 minutes at 1 fps: three chunks of 513.33 s.
    let chunk =
        ((514.0 * 131.0 + 5_134.0 + 450.0) * 1.25 + (1_500.0 + 8.0 * 513.333_334) * 4.25) / 1e6;
    assert!(
        (scenes::estimate_usd(&meta, 1500 * S) - 3.0 * chunk).abs() < 1e-6,
        "{}",
        scenes::estimate_usd(&meta, 1500 * S)
    );
}

// ---------------------------------------------------------------------------------------------------------------------
// Merging chunks and snapping

fn moment(kind: SceneKind, start_s: f64, end_s: f64, text: &str) -> SceneMoment {
    SceneMoment {
        start_ms: (start_s * 1000.0).round() as u64,
        end_ms: (end_s * 1000.0).round() as u64,
        kind,
        who: String::new(),
        text: text.into(),
        on_screen_text: String::new(),
    }
}

fn item(kind: SceneKind, start_s: f64, end_s: f64, text: &str) -> SceneItem {
    SceneItem {
        start_us: (start_s * 1e6).round() as Us,
        end_us: (end_s * 1e6).round() as Us,
        kind,
        who: String::new(),
        text: text.into(),
        on_screen_text: String::new(),
    }
}

fn word(text: &str, start_s: f64, end_s: f64) -> Word {
    Word {
        text: text.into(),
        start_us: (start_s * 1e6).round() as Us,
        end_us: (end_s * 1e6).round() as Us,
        confidence: 0.9,
        kind: if text.is_empty() {
            WordKind::Pause
        } else {
            WordKind::Word
        },
    }
}

/// (start s, end s, kind, text) of each item, for readable comparisons.
fn summary(items: &[SceneItem]) -> Vec<(f64, f64, SceneKind, String)> {
    items
        .iter()
        .map(|i| {
            (
                i.start_us as f64 / 1e6,
                i.end_us as f64 / 1e6,
                i.kind,
                i.text.clone(),
            )
        })
        .collect()
}

#[test]
fn drops_items_in_last_3s_of_non_final_chunk() {
    let first = vec![
        moment(SceneKind::Action, 596.9, 597.5, "kept: before the last 3 s"),
        moment(SceneKind::Speech, 597.0, 599.0, "dropped: at the last 3 s"),
        moment(
            SceneKind::Reaction,
            599.5,
            600.0,
            "dropped: in the last 3 s",
        ),
    ];
    let last = vec![
        moment(
            SceneKind::Text,
            17.5,
            18.0,
            "kept: the next chunk from 597 s",
        ),
        moment(SceneKind::Shot, 118.0, 120.0, "kept: the clip's last 3 s"),
    ];
    let items = scenes::merge(vec![
        (range(0.0, 600.0), first),
        (range(580.0, 700.0), last),
    ]);
    assert_eq!(
        summary(&items),
        vec![
            (
                596.9,
                597.5,
                SceneKind::Action,
                "kept: before the last 3 s".into()
            ),
            (
                597.5,
                598.0,
                SceneKind::Text,
                "kept: the next chunk from 597 s".into()
            ),
            (
                698.0,
                700.0,
                SceneKind::Shot,
                "kept: the clip's last 3 s".into()
            ),
        ]
    );
    // One chunk is the last chunk: its last 3 s are kept.
    let only = scenes::merge(vec![(
        range(0.0, 155.0),
        vec![moment(SceneKind::Speech, 153.0, 155.0, "the end")],
    )]);
    assert_eq!(
        summary(&only),
        vec![(153.0, 155.0, SceneKind::Speech, "the end".into())]
    );
}

#[test]
fn merges_overlapping_chunks_without_duplicates() {
    // Three chunks of a 25-minute clip, timed in each chunk: the overlaps are 580 to 600 s and 1160 to 1180 s.
    let c0 = vec![
        moment(SceneKind::Speech, 10.0, 20.0, "intro"),
        moment(SceneKind::Shot, 590.0, 590.5, "cut to the kitchen"),
        // Cut off by the chunk's end: the next chunk saw it go on to 620 s.
        moment(SceneKind::Speech, 595.0, 600.0, "a long answer"),
        moment(SceneKind::Topic, 596.5, 597.0, "on to cooking"),
        moment(SceneKind::Action, 598.0, 599.0, "waves (late in the chunk)"),
    ];
    let c1 = vec![
        moment(SceneKind::Shot, 10.2, 10.6, "kitchen again"),
        moment(SceneKind::Speech, 15.0, 40.0, "the long answer again"),
        moment(SceneKind::Topic, 17.2, 17.5, "cooking again"),
        moment(SceneKind::Reaction, 17.5, 18.0, "laughs"),
        moment(SceneKind::Action, 18.2, 19.0, "waves"),
        moment(SceneKind::Text, 300.0, 305.0, "a title"),
        moment(SceneKind::Topic, 598.5, 599.0, "late in the chunk"),
    ];
    let c2 = vec![
        moment(
            SceneKind::Speech,
            10.0,
            15.0,
            "in the chunk before's stretch",
        ),
        moment(SceneKind::Action, 337.0, 340.0, "bows"),
    ];
    let items = scenes::merge(vec![
        (range(0.0, 600.0), c0),
        (range(580.0, 1180.0), c1),
        (range(1160.0, 1500.0), c2),
    ]);
    assert_eq!(
        summary(&items),
        vec![
            (10.0, 20.0, SceneKind::Speech, "intro".into()),
            (590.0, 590.6, SceneKind::Shot, "cut to the kitchen".into()),
            (595.0, 620.0, SceneKind::Speech, "a long answer".into()),
            (596.5, 597.5, SceneKind::Topic, "on to cooking".into()),
            (597.5, 598.0, SceneKind::Reaction, "laughs".into()),
            (598.2, 599.0, SceneKind::Action, "waves".into()),
            (880.0, 885.0, SceneKind::Text, "a title".into()),
            (1497.0, 1500.0, SceneKind::Action, "bows".into()),
        ]
    );
    // No moment twice: no two items of one kind start within 2 s of each other.
    for (i, a) in items.iter().enumerate() {
        for b in &items[i + 1..] {
            assert!(
                a.kind != b.kind || (a.start_us - b.start_us).abs() > scenes::SAME_MOMENT_US,
                "{a:?} and {b:?}"
            );
        }
    }
}

#[test]
fn a_repeat_lends_its_end_only_to_the_nearest_item_of_its_kind() {
    let first = vec![
        moment(SceneKind::Speech, 595.0, 596.0, "a short line"),
        moment(SceneKind::Speech, 596.2, 600.0, "a long answer"),
    ];
    let second = vec![
        // The long answer again (0.3 s from it, 1.5 s from the short line): it goes on to 610 s.
        moment(SceneKind::Speech, 16.5, 30.0, "the long answer again"),
        // Another kind at the same time lends no end to either.
        moment(SceneKind::Action, 16.4, 40.0, "waves"),
    ];
    let items = scenes::merge(vec![
        (range(0.0, 600.0), first),
        (range(580.0, 700.0), second),
    ]);
    assert_eq!(
        summary(&items),
        vec![
            (595.0, 596.0, SceneKind::Speech, "a short line".into()),
            (596.2, 610.0, SceneKind::Speech, "a long answer".into()),
        ]
    );
}

#[test]
fn snaps_to_cut_within_tolerance_and_not_beyond() {
    let cuts = [frame_us(404), frame_us(722)]; // 13.467 s and 24.067 s
    // In sound time; the sound starts 0.2 s after the picture, so each is 0.2 s later in file time.
    let words = [
        word("near", 12.9, 13.0),  // 13.1 to 13.2
        word("Hello", 30.0, 30.4), // 30.2 to 30.6
        word("there", 30.5, 31.0), // 30.7 to 31.2
        word("", 31.0, 32.0),      // a pause: no edges of its own
        word("again", 40.0, 40.5), // 40.2 to 40.7
        word("late", 49.6, 49.8),  // 49.8 to 50.0
        word("early", 50.3, 50.6), // 50.5 to 50.8
    ];
    let edges = scenes::word_edges(&words, 200_000);
    assert_eq!(
        edges,
        WordEdges {
            starts: vec![
                13_100_000, 30_200_000, 30_700_000, 40_200_000, 49_800_000, 50_500_000
            ],
            ends: vec![
                13_200_000, 30_600_000, 31_200_000, 40_700_000, 50_000_000, 50_800_000
            ],
        }
    );
    let exactly = SceneItem {
        start_us: frame_us(404) - scenes::CUT_SNAP_US,
        end_us: frame_us(722) - scenes::CUT_SNAP_US,
        ..item(SceneKind::Shot, 0.0, 0.0, "exactly 1.5 s")
    };
    let mut items = vec![
        item(SceneKind::Shot, 12.0, 22.6, "both edges 1.467 s from a cut"),
        exactly,
        item(SceneKind::Shot, 11.9, 25.6, "1.567 s and 1.533 s: too far"),
        item(
            SceneKind::Speech,
            13.0,
            13.2,
            "speech takes a word over a nearer cut",
        ),
        item(SceneKind::Speech, 29.5, 31.9, "words 0.7 s away"),
        item(SceneKind::Speech, 39.1, 41.8, "words 1.1 s away: too far"),
        item(
            SceneKind::Action,
            50.2,
            50.3,
            "its words would put the start past the end",
        ),
        item(SceneKind::Topic, 13.0, 13.0, "a topic takes the cut"),
    ];
    scenes::snap(&mut items, &cuts, &edges);
    let edges_of = |i: &SceneItem| (i.start_us, i.end_us);
    assert_eq!(edges_of(&items[0]), (frame_us(404), frame_us(722)));
    assert_eq!(edges_of(&items[1]), (frame_us(404), frame_us(722)));
    assert_eq!(edges_of(&items[2]), (11_900_000, 25_600_000));
    assert_eq!(edges_of(&items[3]), (13_100_000, 13_200_000));
    assert_eq!(edges_of(&items[4]), (30_200_000, 31_200_000));
    assert_eq!(edges_of(&items[5]), (39_100_000, 41_800_000));
    // Its start's word (50.5) is past every end it could take (50.0, or 50.3 as written): it stays as written.
    assert_eq!(edges_of(&items[6]), (50_200_000, 50_300_000));
    // A topic tries the cut before the word starting 13.1 s.
    assert_eq!(edges_of(&items[7]), (frame_us(404), frame_us(404)));
}

#[test]
fn speech_edges_prefer_words_and_never_collapse_to_one_cut() {
    let (a, b) = (frame_us(404), frame_us(722)); // 13.467 s and 24.067 s
    let words = [word("so", 13.3, 13.5), word("then", 13.6, 13.8)];
    let edges = scenes::word_edges(&words, 0);
    let mut items = vec![
        item(
            SceneKind::Speech,
            13.4,
            13.9,
            "words, though the cut is nearer",
        ),
        item(
            SceneKind::Speech,
            23.6,
            24.5,
            "no words: the start takes the cut",
        ),
        item(
            SceneKind::Reaction,
            23.9,
            24.2,
            "no words: the start takes the cut",
        ),
        item(
            SceneKind::Action,
            23.0,
            23.5,
            "written before the cut it would take",
        ),
        item(
            SceneKind::Topic,
            13.0,
            13.2,
            "the cut, then the next word end",
        ),
        item(SceneKind::Shot, 12.9, 12.9, "an instant may sit on its cut"),
    ];
    let written: Vec<(Us, Us)> = items.iter().map(|i| (i.start_us, i.end_us)).collect();
    scenes::snap(&mut items, &[a, b], &edges);
    assert_eq!(
        items
            .iter()
            .map(|i| (i.start_us, i.end_us))
            .collect::<Vec<_>>(),
        vec![
            (13_300_000, 13_800_000),
            (b, 24_500_000),
            (b, 24_200_000),
            (23_000_000, b),
            (a, 13_500_000),
            (a, a),
        ]
    );
    // Nothing that had a span lost it to one cut.
    for (item, (start, end)) in items.iter().zip(written) {
        if end > start {
            assert!(item.end_us > item.start_us, "{item:?}");
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// The whole log, through a fake logger

/// Answers each call with the next of its answers, counting calls.
struct Fake {
    answers: Mutex<Vec<Result<SceneAnswer, ProviderError>>>,
    calls: AtomicUsize,
    fps: Mutex<Vec<f64>>,
}

impl Fake {
    fn new(answers: Vec<Result<SceneAnswer, ProviderError>>) -> Self {
        Self {
            answers: Mutex::new(answers.into_iter().rev().collect()),
            calls: AtomicUsize::new(0),
            fps: Mutex::new(Vec::new()),
        }
    }
}

impl SceneLogger for Fake {
    fn model(&self) -> &str {
        "fake-scenes"
    }

    fn max_in_flight(&self) -> usize {
        1
    }

    fn estimate_usd(&self, seconds: f64, fps: f64) -> f64 {
        seconds * fps * 1e-4
    }

    fn watch<'a>(
        &'a self,
        _video: &'a Path,
        fps: f64,
    ) -> BoxFuture<'a, Result<SceneAnswer, ProviderError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.fps.lock().unwrap().push(fps);
        let next = self
            .answers
            .lock()
            .unwrap()
            .pop()
            .expect("an answer per call");
        Box::pin(async move { next })
    }
}

fn answer(moments: Vec<SceneMoment>, cost_usd: f64) -> Result<SceneAnswer, ProviderError> {
    Ok(SceneAnswer { moments, cost_usd })
}

#[tokio::test(flavor = "multi_thread")]
async fn the_scene_log_snaps_a_fakes_moments_to_the_clips_cuts_and_words() {
    let mut copy = proxy::existing(&clip_a()).unwrap();
    // The sound starts 0.2 s after the picture: words are 0.2 s later in file time.
    copy.audio_lead_us = 200_000;
    let words = [
        word("floating", 40.0, 40.6), // 40.2 to 40.8
        word("up", 44.5, 44.7),       // 44.7 to 44.9
    ];
    let fake = Fake::new(vec![answer(
        vec![
            moment(SceneKind::Shot, 12.8, 13.5, "cut to a student"),
            moment(
                SceneKind::Speech,
                13.5,
                23.0,
                "asks how they stay entertained",
            ),
            moment(SceneKind::Action, 40.5, 45.0, "raises an arm"),
        ],
        0.0123,
    )]);
    let log = scenes::log(&fake, &copy, &words).await.unwrap();
    assert_eq!(
        (log.model.as_str(), log.fps, log.cost_usd),
        ("fake-scenes", 2.0, 0.0123)
    );
    assert_eq!(*fake.fps.lock().unwrap(), vec![2.0]);
    assert_clip_a_cuts(&log.cuts);
    let (a, b) = (log.cuts[0], log.cuts[1]);
    assert_eq!(
        log.items
            .iter()
            .map(|i| (i.start_us, i.end_us, i.kind))
            .collect::<Vec<_>>(),
        vec![
            // Both edges are near the first cut: the end keeps its own time.
            (a, 13_500_000, SceneKind::Shot),
            (a, b, SceneKind::Speech),
            (40_200_000, 44_900_000, SceneKind::Action),
        ]
    );
}

/// A copy with no cuts (a single shot): the log is snapped to the words alone.
#[tokio::test(flavor = "multi_thread")]
async fn a_copy_without_cuts_is_snapped_to_words_alone() {
    let mut copy = proxy::existing(&clip_a()).unwrap();
    copy.cuts.clear();
    let words = [word("floating", 40.0, 40.6)];
    let fake = Fake::new(vec![answer(
        vec![
            moment(SceneKind::Shot, 12.8, 13.5, "cut to a student"),
            moment(SceneKind::Action, 40.5, 45.0, "raises an arm"),
        ],
        0.01,
    )]);
    let log = scenes::log(&fake, &copy, &words).await.unwrap();
    assert!(log.cuts.is_empty());
    assert_eq!(
        log.items
            .iter()
            .map(|i| (i.start_us, i.end_us))
            .collect::<Vec<_>>(),
        vec![(12_800_000, 13_500_000), (40_000_000, 45_000_000)]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_used_up_credit_stops_the_scene_log_with_its_sentence() {
    let mut copy = proxy::existing(&clip_a()).unwrap();
    // Three chunks of a 25-minute clip (the same file stands in for each).
    copy.duration_us = 1500 * S;
    copy.chunks = proxy::plan(1500 * S, proxy::CHUNK_US, proxy::OVERLAP_US)
        .into_iter()
        .map(|range| ProxyChunk {
            path: clip_a(),
            range,
            bytes: 1,
        })
        .collect();
    let sentence = "This device used its Klipolo credit. Add your Meta API key in Settings.";
    let fake = Fake::new(vec![
        answer(vec![], 0.1),
        Err(ProviderError::new(ProviderErrorKind::Budget, sentence)),
        answer(vec![], 0.1),
    ]);
    let err = scenes::log(&fake, &copy, &[]).await.unwrap_err();
    assert!(err.is_stop());
    assert_eq!(err.user_message(), sentence);
    assert_eq!(
        fake.calls.load(Ordering::SeqCst),
        2,
        "the third chunk was sent"
    );
    // A clip over 5 minutes is watched at 1 frame a second.
    assert_eq!(*fake.fps.lock().unwrap(), vec![1.0, 1.0]);
}

#[test]
fn the_scene_log_future_can_move_between_threads() {
    fn sends<T: Send>(_: &T) {}
    let fake = Fake::new(vec![]);
    let copy = ProxyInfo {
        duration_us: 0,
        audio_lead_us: 0,
        width: 0,
        height: 0,
        fps: 0.0,
        has_audio: false,
        chunks: vec![],
        cuts: vec![],
    };
    let future = scenes::log(&fake, &copy, &[]);
    sends(&future);
}

/// Sources without sound, tiny (64 × 64, which VideoToolbox's hardware encoder refuses and its software one takes) and
/// turned (a phone's display matrix), from the engine's generated media.
#[cfg(target_os = "macos")]
#[test]
#[ignore = "needs scripts/gen-engine-fixtures.sh"]
fn makes_small_copies_of_silent_tiny_and_turned_sources_on_this_mac() {
    use klipolo_engine::render::probe::probe_media;
    let media = fixture_set::fixture_dir();
    assert!(
        media.join(".complete").exists(),
        "run scripts/gen-engine-fixtures.sh"
    );
    let cancel = AtomicBool::new(false);
    let out = tempfile::tempdir().unwrap();

    // No sound: a copy of the picture alone.
    let silent = proxy::make(
        &media.join("camera-720p.mp4"),
        &out.path().join("silent"),
        360,
        &cancel,
    )
    .unwrap();
    assert_eq!(
        (silent.width, silent.height, silent.has_audio),
        (640, 360, false)
    );
    let bytes = std::fs::read(&silent.chunks[0].path).unwrap();
    assert!(!bytes.windows(4).any(|w| w == b"mp4a"));
    // 4 frames a second: 16 frames over 4 s, which a container may count over 3.75 s.
    let fps = probe_media(&silent.chunks[0].path).unwrap().fps;
    assert!((3.95..4.3).contains(&fps), "{fps}");

    // Tiny: never enlarged, encoded all the same, its sound placed by its lead.
    let tiny = proxy::make(
        &media.join("lead-40ms.mp4"),
        &out.path().join("tiny"),
        360,
        &cancel,
    )
    .unwrap();
    assert_eq!((tiny.width, tiny.height, tiny.has_audio), (64, 64, true));
    assert_ne!(tiny.audio_lead_us, 0);

    // Turned: stored landscape, shown portrait; the copy is portrait in its pixels, with no turn left to apply.
    let turned = proxy::make(
        &media.join("phone-portrait.mov"),
        &out.path().join("turned"),
        360,
        &cancel,
    )
    .unwrap();
    assert_eq!((turned.width, turned.height), (360, 640));
    let info = probe_media(&turned.chunks[0].path).unwrap();
    assert_eq!((info.width, info.height, info.rotation), (360, 640, 0));
}
