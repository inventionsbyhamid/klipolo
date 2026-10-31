//! Analysis in the app against a mocked Meta and relay (one local mock server answers both), with the secrets in memory
//! (`MemoryStore`) and the clips' local work faked (`Media`): the sound is a tone made here, the small copy a few bytes.
//! Nothing here reaches Meta or Klipolo's relay.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use klipolo_analysis::VideoError;
use klipolo_analysis::audio::{AudioError, AudioInfo};
use klipolo_analysis::cache::{Dir, content_key};
use klipolo_analysis::providers::SceneKind;
use klipolo_analysis::providers::meta::{
    SCENE_OUTPUT_TOKENS_BASE, SCENE_OUTPUT_TOKENS_PER_S, SCENE_PROMPT_TOKENS,
};
use klipolo_analysis::proxy::{ProxyChunk, ProxyInfo};
use klipolo_analysis::scenes::{SceneItem, SceneLog};
use klipolo_analysis::transcript::{Timing, TranscriptOut, Turn, finish};
use klipolo_app::analysis::chat::{scene_entries, speaker_turns};
use klipolo_app::analysis::media::{EngineMedia, Media};
use klipolo_app::analysis::{
    AnalysisDone, AnalysisEvent, AnalysisFailed, AnalysisState, AnalysisStep, AnalysisStopped,
    Asked, AssetState, NEEDS_KEY,
};
use klipolo_app::credentials::{
    Credentials, Endpoints, META_API_KEY, MemoryStore, RELAY_CODE, RELAY_TOKEN, SecretStore,
};
use klipolo_engine::audio::pcm::{PcmKey, PcmStore};
use klipolo_meta::{ExposeSecret, SecretString, pricing};
use klipolo_model::project::{Asset, AssetKind};
use klipolo_model::speech::{Levels, Speech, Word, WordKind};
use klipolo_model::time::{RangeUs, Us};
use serde_json::{Value, json};
use wiremock::matchers::{any, body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const KEY: &str = "sk-analysis-test-key-0123456789";
const TOKEN: &str = "relay-analysis-token-0123456789";
const NEW_TOKEN: &str = "relay-analysis-new-token-9876543";
const CODE: &str = "KLIP-TEST-0003";
const DEVICE: &str = "6f1c2a4e-9d3b-4b7a-8e21-0c5d9f3a7b64";
const CREDIT_USED: &str = "This device has used its Klipolo credit.";
const RATE: u32 = 16_000;
/// The test clips' length: one turn of speech in it.
const SECONDS: f64 = 6.0;

fn us(seconds: f64) -> Us {
    (seconds * 1e6).round() as Us
}

fn secret(value: &str) -> SecretString {
    SecretString::from(value.to_owned())
}

// ---------------------------------------------------------------------------------------------------------------------
// Clips and their local work

/// A mono 16-bit PCM WAV at 16 kHz.
fn wav_of(samples: &[i16]) -> Vec<u8> {
    let data = (samples.len() * 2) as u32;
    let mut w = Vec::with_capacity(44 + data as usize);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&RATE.to_le_bytes());
    w.extend_from_slice(&(RATE * 2).to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        w.extend_from_slice(&s.to_le_bytes());
    }
    w
}

/// A steady 220 Hz tone at a third of full scale: speech to the silence check.
fn tone(seconds: f64) -> Vec<i16> {
    let n = (seconds * f64::from(RATE)).round() as usize;
    (0..n)
        .map(|i| {
            let t = i as f64 / f64::from(RATE);
            (0.33 * 32767.0 * (2.0 * std::f64::consts::PI * 220.0 * t).sin()).round() as i16
        })
        .collect()
}

/// Local work standing in for the engine's: a tone for the sound, flat levels, and a small copy of a few bytes in
/// `chunks`.
struct FakeMedia {
    seconds: f64,
    chunks: Vec<RangeUs>,
    sounds: AtomicUsize,
    copies: AtomicUsize,
    levels: AtomicUsize,
}

impl FakeMedia {
    fn new(seconds: f64, chunks: Vec<RangeUs>) -> Arc<Self> {
        Arc::new(Self {
            seconds,
            chunks,
            sounds: AtomicUsize::new(0),
            copies: AtomicUsize::new(0),
            levels: AtomicUsize::new(0),
        })
    }

    /// One chunk over the whole clip.
    fn whole(seconds: f64) -> Arc<Self> {
        Self::new(
            seconds,
            vec![RangeUs {
                start: 0,
                end: us(seconds),
            }],
        )
    }
}

impl Media for FakeMedia {
    fn sound(
        &self,
        _src: &Path,
        out: &Path,
        _cancel: &AtomicBool,
    ) -> Result<AudioInfo, AudioError> {
        self.sounds.fetch_add(1, Ordering::SeqCst);
        std::fs::write(out, wav_of(&tone(self.seconds)))?;
        Ok(AudioInfo {
            duration_us: us(self.seconds),
            has_audio: true,
            peak_dbfs: -9.6,
        })
    }

    fn levels(&self, _src: &Path, _cancel: &AtomicBool) -> Option<Levels> {
        self.levels.fetch_add(1, Ordering::SeqCst);
        Some(Levels {
            db: vec![-20.0; (self.seconds * 10.0) as usize].into(),
        })
    }

    fn small_copy(
        &self,
        _src: &Path,
        dir: &Path,
        _cancel: &AtomicBool,
        seen: &mut dyn FnMut(Us),
    ) -> Result<ProxyInfo, VideoError> {
        self.copies.fetch_add(1, Ordering::SeqCst);
        std::fs::create_dir_all(dir).unwrap();
        let chunks = self
            .chunks
            .iter()
            .enumerate()
            .map(|(k, range)| {
                let path = dir.join(format!("chunk-{k:02}.mp4"));
                std::fs::write(&path, b"a small copy").unwrap();
                seen(range.end);
                ProxyChunk {
                    path,
                    range: *range,
                    bytes: 12,
                }
            })
            .collect();
        Ok(ProxyInfo {
            duration_us: us(self.seconds),
            audio_lead_us: 0,
            width: 640,
            height: 360,
            fps: 4.0,
            has_audio: true,
            chunks,
            cuts: Vec::new(),
        })
    }
}

fn asset(id: &str, kind: AssetKind, file: &Path, seconds: f64) -> Asset {
    let video = kind == AssetKind::Video;
    Asset {
        id: id.into(),
        kind,
        path: file.to_string_lossy().into_owned(),
        name: file.file_name().unwrap().to_string_lossy().into_owned(),
        duration_us: us(seconds),
        width: if video { 1920 } else { 0 },
        height: if video { 1080 } else { 0 },
        fps: if video { 30.0 } else { 0.0 },
        rotation: video.then_some(0),
        has_audio: kind != AssetKind::Image,
        audio_lead_us: video.then_some(0),
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// The app's analysis, with what it tells

/// Each `Speech` published, with how many align requests the mock had received by then.
type Published = Arc<Mutex<Vec<(usize, Arc<Speech>)>>>;

/// Everything one test's analysis tells, and where it keeps its files.
struct Harness {
    state: Arc<AnalysisState>,
    events: Arc<Mutex<Vec<AnalysisEvent>>>,
    speech: Published,
    aligns: Arc<AtomicUsize>,
    store: Arc<MemoryStore>,
    cache_root: PathBuf,
    clips: PathBuf,
    _dir: tempfile::TempDir,
}

fn harness(server: &MockServer, store: MemoryStore, media: Arc<dyn Media>) -> Harness {
    harness_with(server, store, media, None)
}

/// `harness`, its events sink panicking the first time a step `panic_at` starts (a fault inside a clip's task).
fn harness_with(
    server: &MockServer,
    store: MemoryStore,
    media: Arc<dyn Media>,
    panic_at: Option<AnalysisStep>,
) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let (cache_root, work, clips) = (
        dir.path().join("analysis"),
        dir.path().join("work"),
        dir.path().join("clips"),
    );
    std::fs::create_dir_all(&clips).unwrap();
    let store = Arc::new(store);
    let credentials = Arc::new(Credentials::load(
        store.clone(),
        Endpoints {
            meta_base: format!("{}/v1", server.uri()),
            relay_base: server.uri(),
        },
        DEVICE.into(),
    ));
    let events = Arc::new(Mutex::new(Vec::new()));
    let speech = Arc::new(Mutex::new(Vec::new()));
    let aligns = Arc::new(AtomicUsize::new(0));
    let state = AnalysisState::new(
        credentials,
        media,
        cache_root.clone(),
        work,
        {
            let events = events.clone();
            let armed = AtomicBool::new(panic_at.is_some());
            Arc::new(move |event| {
                if let AnalysisEvent::Progress(p) = &event
                    && Some(p.step) == panic_at
                    && armed.swap(false, Ordering::SeqCst)
                {
                    panic!("a fault while telling the page");
                }
                events.lock().unwrap().push(event)
            })
        },
        {
            let (speech, aligns) = (speech.clone(), aligns.clone());
            Arc::new(move |now: Arc<Speech>| {
                speech
                    .lock()
                    .unwrap()
                    .push((aligns.load(Ordering::SeqCst), now))
            })
        },
    );
    Harness {
        state,
        events,
        speech,
        aligns,
        store,
        cache_root,
        clips,
        _dir: dir,
    }
}

impl Harness {
    /// A clip file of its own content (`seed` makes it differ from the others).
    fn clip(&self, name: &str, seed: u8) -> PathBuf {
        let path = self.clips.join(name);
        let bytes: Vec<u8> = (0..65_536u32)
            .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
            .collect();
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn cache_dir(&self, file: &Path) -> Dir {
        Dir::new(&self.cache_root, &content_key(file).unwrap()).unwrap()
    }

    fn events_for(&self, id: &str) -> Vec<AnalysisEvent> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.asset_id() == id)
            .cloned()
            .collect()
    }

    fn last_speech(&self) -> Arc<Speech> {
        self.speech
            .lock()
            .unwrap()
            .last()
            .expect("speech published")
            .1
            .clone()
    }

    fn stored_token(&self) -> Option<String> {
        self.store
            .get(RELAY_TOKEN)
            .unwrap()
            .map(|t| t.expose_secret().to_owned())
    }
}

/// A key and the device's relay token (registered already): Meta calls go to Meta, word timing to the relay.
fn key_store() -> MemoryStore {
    let store = MemoryStore::default();
    store.set(META_API_KEY, &secret(KEY)).unwrap();
    store.set(RELAY_TOKEN, &secret(TOKEN)).unwrap();
    store
}

/// An access code and its device token: everything goes through the relay.
fn code_store() -> MemoryStore {
    let store = MemoryStore::default();
    store.set(RELAY_CODE, &secret(CODE)).unwrap();
    store.set(RELAY_TOKEN, &secret(TOKEN)).unwrap();
    store
}

// ---------------------------------------------------------------------------------------------------------------------
// The mock's answers

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

/// One turn, "hello there friends", 0.5 to 4 s into the clip.
fn asr_answer() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "audioDurationMs": 6000,
        "turns": [{"turnId": 0, "startMs": 500, "endMs": 4000,
                   "transcript": "hello there friends", "speaker": "A"}]
    }))
}

/// Its words, in an align request's WAV that starts 0.25 s before the turn.
fn align_answer() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "words": [
            {"text": "hello", "startMs": 250, "endMs": 1000, "score": 0.9, "turn": 0},
            {"text": "there", "startMs": 1100, "endMs": 2000, "score": 0.9, "turn": 0},
            {"text": "friends", "startMs": 2100, "endMs": 3700, "score": 0.9, "turn": 0}
        ],
        "skipped": []
    }))
}

/// Spark's log of the clip: one moment.
fn spark_answer() -> Value {
    let log = json!({"moments": [{
        "start_sec": 0.6, "end_sec": 3.9, "kind": "speech", "who": "host",
        "description": "Says hello", "on_screen_text": ""
    }]});
    json!({
        "status": "completed",
        "output": [{"type": "message", "role": "assistant",
                    "content": [{"type": "output_text", "text": log.to_string()}]}],
        "usage": {"input_tokens": 1000, "output_tokens": 200}
    })
}

/// The relay's device answer.
fn device(budget: f64, spent: f64) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .set_body_json(json!({"relay_ai": true, "budget_usd": budget, "spent_usd": spent}))
}

/// Answers with `template`, counting each request first.
struct Counted {
    count: Arc<AtomicUsize>,
    template: ResponseTemplate,
}

impl Respond for Counted {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        self.count.fetch_add(1, Ordering::SeqCst);
        self.template.clone()
    }
}

/// Transcription at `prefix` (`/v1` with a key, `/meta/v1` through the relay) and word timing on the relay, counted
/// in `aligns` and answered after `align_delay`.
async fn mount_speech(
    server: &MockServer,
    prefix: &str,
    aligns: &Arc<AtomicUsize>,
    align_delay: Duration,
) {
    Mock::given(method("POST"))
        .and(path(format!("{prefix}/asr/transcribe")))
        .respond_with(asr_answer())
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/align"))
        .respond_with(Counted {
            count: aligns.clone(),
            template: align_answer().set_delay(align_delay),
        })
        .mount(server)
        .await;
}

/// The scene log's upload, delete and answer at `prefix`.
async fn mount_scenes(server: &MockServer, prefix: &str, answer: impl Respond + 'static) {
    Mock::given(method("POST"))
        .and(path(format!("{prefix}/files")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "file-1"})))
        .mount(server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(format!("{prefix}/files/file-1")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"deleted": true})))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{prefix}/responses")))
        .respond_with(answer)
        .mount(server)
        .await;
}

/// Anything not mounted with a higher priority fails the test when it is asked.
async fn nothing_else(server: &MockServer) {
    Mock::given(any())
        .respond_with(ResponseTemplate::new(500))
        .with_priority(u8::MAX)
        .expect(0)
        .mount(server)
        .await;
}

async fn requests(server: &MockServer, verb: &str, at: &str) -> Vec<Request> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.method.as_str() == verb && r.url.path() == at)
        .collect()
}

/// What an earlier analysis of a clip left in the cache: its three words, aligned.
fn cached_transcript() -> TranscriptOut {
    let turn = Turn {
        start_us: 500_000,
        end_us: 4_000_000,
        text: "hello there friends".into(),
        speaker: Some("A".into()),
        chunk: 0,
        aligned: false,
    };
    let word = |text: &str, start: Us, end: Us| Word {
        text: text.into(),
        start_us: start,
        end_us: end,
        confidence: 0.9,
        kind: WordKind::Word,
    };
    let timing = Timing {
        words: vec![Some(vec![
            word("hello", 500_000, 1_250_000),
            word("there", 1_350_000, 2_250_000),
            word("friends", 2_350_000, 3_950_000),
        ])],
        cost_usd: 0.0,
        note: None,
    };
    finish(
        "muse-voice-transcribe-1.0",
        us(SECONDS),
        vec![turn],
        Some(timing),
        0.0003,
    )
}

fn cached_scenes() -> SceneLog {
    SceneLog {
        model: "muse-spark-1.3".into(),
        fps: 2.0,
        items: vec![SceneItem {
            start_us: 500_000,
            end_us: 3_950_000,
            kind: SceneKind::Speech,
            who: "host".into(),
            text: "Says hello".into(),
            on_screen_text: "HELLO".into(),
        }],
        cuts: vec![],
        cost_usd: 0.01,
    }
}

/// Spark's estimate for a clip of `seconds` (one chunk, 2 frames a second): the plan's formula, from the price table.
fn scene_usd(seconds: f64) -> f64 {
    let input = (seconds * 2.0).ceil() * pricing::VIDEO_TOKENS_PER_FRAME as f64
        + (seconds * pricing::AUDIO_TOKENS_PER_SECOND as f64).ceil()
        + SCENE_PROMPT_TOKENS as f64;
    let output = SCENE_OUTPUT_TOKENS_BASE + SCENE_OUTPUT_TOKENS_PER_S * seconds;
    input * pricing::SPARK_INPUT_USD + output * pricing::SPARK_OUTPUT_USD
}

fn asr_usd(seconds: f64) -> f64 {
    seconds / 3600.0 * pricing::ASR_USD_PER_HOUR
}

// ---------------------------------------------------------------------------------------------------------------------
// The core tests

/// A file analysed before (re-imported, or its project opened again) loads from the cache: no request of any kind,
/// nothing decoded for it, $0 in the estimate, and its words and scenes are there as before.
#[tokio::test(flavor = "multi_thread")]
async fn cached_asset_makes_no_network_call() {
    let server = MockServer::start().await;
    nothing_else(&server).await;
    let media = FakeMedia::whole(SECONDS);
    let h = harness(&server, key_store(), media.clone());
    let file = h.clip("a.mp4", 1);
    let clip = asset("clip-a", AssetKind::Video, &file, SECONDS);
    let dir = h.cache_dir(&file);
    let transcript = cached_transcript();
    dir.store("transcript", &transcript).unwrap();
    dir.store("scenes", &cached_scenes()).unwrap();

    let estimate = h.state.estimate(std::slice::from_ref(&clip)).await;
    assert_eq!(estimate.cached, ["clip-a"]);
    assert!(estimate.clips.is_empty());
    assert_eq!((estimate.usd, estimate.seconds), (0.0, 0.0));
    assert_eq!(estimate.refusal, None);

    let started = h.state.start(std::slice::from_ref(&clip)).await;
    assert_eq!(started.refused, None);
    started.finished().await;
    server.verify().await;

    assert_eq!(
        h.state.state_of("clip-a"),
        AssetState::Done { approximate: false }
    );
    assert_eq!(
        h.events_for("clip-a").last(),
        Some(&AnalysisEvent::Done(AnalysisDone {
            asset_id: "clip-a".into(),
            approximate: false
        }))
    );
    assert_eq!(media.sounds.load(Ordering::SeqCst), 0, "nothing decoded");
    assert_eq!(media.copies.load(Ordering::SeqCst), 0);
    let speech = h.last_speech();
    assert_eq!(
        speech.transcripts["clip-a"].words,
        transcript.transcript.words
    );
    assert!(speech.levels.contains_key("clip-a"), "levels too");
    let found = h.state.analysis_for("clip-a").unwrap();
    assert_eq!(found.scenes.as_deref(), Some(&cached_scenes()));
    assert_eq!(found.turns, transcript.turns);
    assert_eq!(found.spent_usd, 0.0);
}

/// The estimate is Meta's list prices for what isn't cached: Voice Transcribe for each clip's sound, Spark for each
/// video's footage (131 tokens a frame at 2 frames a second, 10 a second of sound, the prompt, the expected output).
/// A cached clip costs nothing; a clip with only its transcript cached costs its scene log; an image isn't analysed.
#[tokio::test(flavor = "multi_thread")]
async fn estimate_matches_pricing_formula() {
    let server = MockServer::start().await;
    nothing_else(&server).await;
    let h = harness(&server, key_store(), FakeMedia::whole(SECONDS));
    let a = asset("a", AssetKind::Video, &h.clip("a.mov", 1), 155.04);
    let b = asset("b", AssetKind::Audio, &h.clip("b.m4a", 2), 60.0);
    let c = asset("c", AssetKind::Image, &h.clip("c.png", 3), 0.0);
    let d_file = h.clip("d.mov", 4);
    let d = asset("d", AssetKind::Video, &d_file, 30.0);
    h.cache_dir(&d_file)
        .store("transcript", &cached_transcript())
        .unwrap();
    h.cache_dir(&d_file)
        .store("scenes", &cached_scenes())
        .unwrap();
    let e_file = h.clip("e.mov", 5);
    let e = asset("e", AssetKind::Video, &e_file, 20.0);
    h.cache_dir(&e_file)
        .store("transcript", &cached_transcript())
        .unwrap();

    let estimate = h.state.estimate(&[a, b, c, d, e]).await;

    let expected = asr_usd(155.04) + scene_usd(155.04) + asr_usd(60.0) + scene_usd(20.0);
    assert!(
        (estimate.usd - expected).abs() < 1e-12,
        "{} vs {expected}",
        estimate.usd
    );
    assert!((estimate.seconds - 235.04).abs() < 1e-9);
    assert_eq!(estimate.clips, ["a", "b", "e"]);
    assert_eq!(estimate.cached, ["d"]);
    assert_eq!((estimate.credit_left_usd, estimate.refusal), (None, None));
    server.verify().await;
}

/// The relay runs out of credit at the scene log (402): the run stops there, nothing asks again, the clip fails with
/// the relay's sentence and the way on (a Meta key), and the transcript it finished is kept: published and cached. No
/// scene log is cached.
#[tokio::test(flavor = "multi_thread")]
async fn relay_402_stops_and_keeps_finished_steps() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/devices/me"))
        .respond_with(device(2.0, 0.5))
        .mount(&server)
        .await;
    let h = harness(&server, code_store(), FakeMedia::whole(SECONDS));
    mount_speech(&server, "/meta/v1", &h.aligns, Duration::ZERO).await;
    mount_scenes(
        &server,
        "/meta/v1",
        ResponseTemplate::new(402)
            .set_body_json(json!({"error": "budget_exhausted", "message": CREDIT_USED})),
    )
    .await;
    nothing_else(&server).await;
    let file = h.clip("a.mp4", 1);
    let clip = asset("clip-a", AssetKind::Video, &file, SECONDS);

    let started = h.state.start(std::slice::from_ref(&clip)).await;
    assert_eq!(started.refused, None);
    started.finished().await;

    let reason = format!("{CREDIT_USED} Add your own Meta API key in Settings to keep going.");
    assert_eq!(
        h.state.state_of("clip-a"),
        AssetState::Failed {
            reason: reason.clone()
        }
    );
    assert_eq!(
        h.events_for("clip-a").last(),
        Some(&AnalysisEvent::Failed(AnalysisFailed {
            asset_id: "clip-a".into(),
            reason
        }))
    );
    assert_eq!(
        requests(&server, "POST", "/meta/v1/responses").await.len(),
        1,
        "a used-up credit is never asked again"
    );
    assert_eq!(
        requests(&server, "DELETE", "/meta/v1/files/file-1")
            .await
            .len(),
        1
    );
    // The finished transcript stays: in the engine, for chat, and in the cache. The scene log is nowhere.
    let dir = h.cache_dir(&file);
    let cached: TranscriptOut = dir.load("transcript").expect("the transcript is cached");
    assert_eq!(
        h.last_speech().transcripts["clip-a"].words,
        cached.transcript.words
    );
    assert!(dir.load::<SceneLog>("scenes").is_none());
    let found = h.state.analysis_for("clip-a").unwrap();
    assert!(found.transcript.is_some() && found.scenes.is_none());
    server.verify().await;
}

/// The turns are published as captions (each turn's words spread across it) before any word is timed, then again with
/// the aligner's times: a new `Speech` each time.
#[tokio::test(flavor = "multi_thread")]
async fn turn_level_captions_publish_before_alignment() {
    let server = MockServer::start().await;
    let h = harness(&server, key_store(), FakeMedia::whole(SECONDS));
    mount_speech(&server, "/v1", &h.aligns, Duration::from_millis(300)).await;
    nothing_else(&server).await;
    let clip = asset("talk", AssetKind::Audio, &h.clip("talk.m4a", 7), SECONDS);

    h.state
        .start(std::slice::from_ref(&clip))
        .await
        .finished()
        .await;

    assert_eq!(
        h.state.state_of("talk"),
        AssetState::Done { approximate: false }
    );
    let published: Vec<(usize, Arc<Speech>)> = h
        .speech
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, s)| s.transcripts.contains_key("talk"))
        .cloned()
        .collect();
    assert!(published.len() >= 2, "{}", published.len());
    let (aligns_then, first) = &published[0];
    assert_eq!(*aligns_then, 0, "published before any word was timed");
    let rough = &first.transcripts["talk"].words;
    let said: Vec<&str> = rough
        .iter()
        .filter(|w| w.kind != WordKind::Pause)
        .map(|w| w.text.as_str())
        .collect();
    assert_eq!(said, ["hello", "there", "friends"]);
    assert!(
        rough.iter().all(|w| w.confidence == 0.0),
        "spread, not timed"
    );
    let (_, last) = published.last().unwrap();
    assert!(!Arc::ptr_eq(first, last), "a new Speech");
    let timed = &last.transcripts["talk"].words;
    assert!(
        timed
            .iter()
            .filter(|w| w.kind == WordKind::Word)
            .all(|w| (w.confidence - 0.9).abs() < 1e-6)
    );
    assert_eq!(timed[0].start_us, 500_000, "the aligner's time");
    assert_eq!(h.aligns.load(Ordering::SeqCst), 1);
    server.verify().await;
}

/// Cancel while the first of two scene log chunks is being watched: that request finishes, the second is never sent,
/// the clip is back to not analysed, and only the finished transcript is cached.
#[tokio::test(flavor = "multi_thread")]
async fn cancel_stops_after_current_request() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/devices/me"))
        .respond_with(device(2.0, 0.0))
        .mount(&server)
        .await;
    let media = FakeMedia::new(
        SECONDS,
        vec![
            RangeUs {
                start: 0,
                end: us(4.0),
            },
            RangeUs {
                start: us(2.0),
                end: us(SECONDS),
            },
        ],
    );
    let h = harness(&server, code_store(), media);
    mount_speech(&server, "/meta/v1", &h.aligns, Duration::ZERO).await;
    let watched = Arc::new(AtomicUsize::new(0));
    mount_scenes(
        &server,
        "/meta/v1",
        Counted {
            count: watched.clone(),
            template: ResponseTemplate::new(200)
                .set_body_json(spark_answer())
                .set_delay(Duration::from_millis(500)),
        },
    )
    .await;
    nothing_else(&server).await;
    let file = h.clip("a.mp4", 1);
    let clip = asset("clip-a", AssetKind::Video, &file, SECONDS);

    let started = h.state.start(std::slice::from_ref(&clip)).await;
    let deadline = Instant::now() + Duration::from_secs(20);
    while watched.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline, "the scene log never started");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    h.state.cancel();
    started.finished().await;

    assert_eq!(
        watched.load(Ordering::SeqCst),
        1,
        "the second chunk never went"
    );
    assert_eq!(requests(&server, "POST", "/meta/v1/files").await.len(), 1);
    assert_eq!(h.state.state_of("clip-a"), AssetState::None);
    assert_eq!(
        h.events_for("clip-a").last(),
        Some(&AnalysisEvent::Stopped(AnalysisStopped {
            asset_id: "clip-a".into()
        }))
    );
    let dir = h.cache_dir(&file);
    assert!(dir.load::<TranscriptOut>("transcript").is_some());
    assert!(
        dir.load::<SceneLog>("scenes").is_none(),
        "nothing half done"
    );
    server.verify().await;
}

// ---------------------------------------------------------------------------------------------------------------------
// Credentials, credit and the relay's token

/// The relay refuses the device's token (401): the device registers again once, with its code, and the request goes
/// again with the new token.
#[tokio::test(flavor = "multi_thread")]
async fn a_relay_401_registers_again_once_and_retries() {
    let server = MockServer::start().await;
    // The credit check at the start answers; asked again with the old token after the 401, the relay refuses it.
    Mock::given(method("GET"))
        .and(path("/v1/devices/me"))
        .and(header("authorization", bearer(TOKEN).as_str()))
        .respond_with(device(2.0, 0.0))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/devices/me"))
        .and(header("authorization", bearer(TOKEN).as_str()))
        .respond_with(ResponseTemplate::new(401))
        .with_priority(2)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/devices"))
        .and(body_string_contains(CODE))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "token": NEW_TOKEN, "relay_ai": true, "budget_usd": 2.0, "spent_usd": 0.0
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/meta/v1/asr/transcribe"))
        .and(header("authorization", bearer(TOKEN).as_str()))
        .respond_with(ResponseTemplate::new(401))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/meta/v1/asr/transcribe"))
        .and(header("authorization", bearer(NEW_TOKEN).as_str()))
        .respond_with(asr_answer())
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/align"))
        .and(header("authorization", bearer(NEW_TOKEN).as_str()))
        .respond_with(align_answer())
        .expect(1)
        .mount(&server)
        .await;
    nothing_else(&server).await;
    let h = harness(&server, code_store(), FakeMedia::whole(SECONDS));
    let clip = asset("talk", AssetKind::Audio, &h.clip("talk.m4a", 2), SECONDS);

    h.state
        .start(std::slice::from_ref(&clip))
        .await
        .finished()
        .await;

    assert_eq!(
        h.state.state_of("talk"),
        AssetState::Done { approximate: false }
    );
    assert_eq!(h.stored_token().as_deref(), Some(NEW_TOKEN));
    server.verify().await;
}

/// Meta refuses the person's own key (401): the clip fails saying the key didn't work, and nothing registers the device
/// again (that would not fix a key).
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_key_says_so_and_never_registers() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/asr/transcribe"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "error": {"message": "Incorrect API key provided"}
        })))
        .expect(1)
        .mount(&server)
        .await;
    nothing_else(&server).await;
    let h = harness(&server, key_store(), FakeMedia::whole(SECONDS));
    let clip = asset("talk", AssetKind::Audio, &h.clip("talk.m4a", 2), SECONDS);

    h.state
        .start(std::slice::from_ref(&clip))
        .await
        .finished()
        .await;

    assert_eq!(
        h.state.state_of("talk"),
        AssetState::Failed {
            reason: "That key didn't work. Check it on dev.meta.ai.".into()
        }
    );
    assert_eq!(h.stored_token().as_deref(), Some(TOKEN));
    server.verify().await;
}

/// A code whose device can't register now (the relay is busy) gets the relay's sentence, never the ask for a key or
/// code; with neither, the ask. A cached clip loads either way.
#[tokio::test(flavor = "multi_thread")]
async fn a_code_whose_device_cannot_register_says_why() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/devices"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;
    nothing_else(&server).await;
    let store = MemoryStore::default();
    store.set(RELAY_CODE, &secret(CODE)).unwrap();
    let h = harness(&server, store, FakeMedia::whole(SECONDS));
    let clip = asset("talk", AssetKind::Audio, &h.clip("talk.m4a", 2), SECONDS);

    let started = h.state.start(std::slice::from_ref(&clip)).await;
    assert_eq!(
        started.refused.as_deref(),
        Some("Klipolo's server is busy right now. Try again in a moment.")
    );
    started.finished().await;
    assert_eq!(h.state.state_of("talk"), AssetState::None);

    let empty = harness(&server, MemoryStore::default(), FakeMedia::whole(SECONDS));
    let file = empty.clip("a.mp4", 9);
    empty
        .cache_dir(&file)
        .store("transcript", &cached_transcript())
        .unwrap();
    empty
        .cache_dir(&file)
        .store("scenes", &cached_scenes())
        .unwrap();
    let cached = asset("cached", AssetKind::Video, &file, SECONDS);
    let fresh = asset("fresh", AssetKind::Audio, &empty.clip("b.m4a", 3), SECONDS);
    let started = empty.state.start(&[cached, fresh]).await;
    assert_eq!(started.refused.as_deref(), Some(NEEDS_KEY));
    started.finished().await;
    assert_eq!(
        empty.state.state_of("cached"),
        AssetState::Done { approximate: false }
    );
    assert_eq!(empty.state.state_of("fresh"), AssetState::None);
    server.verify().await;
}

/// With an access code, an estimate over the credit left is refused with why and what to do, and Analyze sends nothing.
#[tokio::test(flavor = "multi_thread")]
async fn an_estimate_over_the_credit_left_is_refused() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/devices/me"))
        .respond_with(device(2.0, 1.98))
        .mount(&server)
        .await;
    nothing_else(&server).await;
    let media = FakeMedia::whole(SECONDS);
    let h = harness(&server, code_store(), media.clone());
    let clip = asset("long", AssetKind::Video, &h.clip("long.mov", 1), 155.04);

    let estimate = h.state.estimate(std::slice::from_ref(&clip)).await;
    let refusal = "Your access code has $0.02 left, and this needs about $0.07. Trim the clips first, or add your own \
                   Meta key in Settings.";
    assert!((estimate.credit_left_usd.unwrap() - 0.02).abs() < 1e-9);
    assert_eq!(estimate.refusal.as_deref(), Some(refusal));

    let started = h.state.start(std::slice::from_ref(&clip)).await;
    assert_eq!(started.refused.as_deref(), Some(refusal));
    started.finished().await;
    assert_eq!(h.state.state_of("long"), AssetState::None);
    assert_eq!(media.sounds.load(Ordering::SeqCst), 0);
    server.verify().await;
}

/// A clip with no sound stream: no transcription request, a transcript with no words, and the scene log still runs.
#[tokio::test(flavor = "multi_thread")]
async fn a_clip_without_sound_still_gets_its_scene_log() {
    let server = MockServer::start().await;
    mount_scenes(
        &server,
        "/v1",
        ResponseTemplate::new(200).set_body_json(spark_answer()),
    )
    .await;
    nothing_else(&server).await;
    let h = harness(&server, key_store(), FakeMedia::whole(SECONDS));
    let mut clip = asset("quiet", AssetKind::Video, &h.clip("quiet.mp4", 4), SECONDS);
    clip.has_audio = false;

    h.state
        .start(std::slice::from_ref(&clip))
        .await
        .finished()
        .await;

    assert_eq!(
        h.state.state_of("quiet"),
        AssetState::Done { approximate: false }
    );
    let found = h.state.analysis_for("quiet").unwrap();
    assert!(found.transcript.as_ref().unwrap().words.is_empty());
    let scenes = found.scenes.as_ref().unwrap();
    assert_eq!(scenes.items.len(), 1);
    assert_eq!(scenes.items[0].text, "Says hello");
    let progress: Vec<AnalysisStep> = h
        .events_for("quiet")
        .iter()
        .filter_map(|e| match e {
            AnalysisEvent::Progress(p) => Some(p.step),
            _ => None,
        })
        .collect();
    assert!(progress.contains(&AnalysisStep::Scenes));
    assert!(!progress.contains(&AnalysisStep::Transcript));
    server.verify().await;
}

// ---------------------------------------------------------------------------------------------------------------------
// Levels and chat's inputs

/// A file's levels come from the sound cache's energies: one value per 100 ms of its sound, `10 log10(power / 2)` dB.
#[test]
fn levels_follow_the_energies_formula() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("tone-then-silence.wav");
    let mut samples = tone(1.0);
    samples.extend(vec![0i16; RATE as usize]);
    std::fs::write(&file, wav_of(&samples)).unwrap();
    let store = PcmStore::new(dir.path().join("pcm"));
    let media = EngineMedia::new(Some(store.clone()));

    let levels = media
        .levels(&file, &AtomicBool::new(false))
        .expect("levels");

    assert_eq!(levels.db.len(), 20, "2 s in 100 ms windows");
    let key = PcmKey {
        path: file.to_string_lossy().into_owned(),
    };
    let pcm = store.build(&key).unwrap();
    let energies = store.energies(&key, &pcm).unwrap();
    for (w, db) in levels.db.iter().enumerate() {
        let want = (10.0 * (energies.power(w) / 2.0).max(1e-12).log10()) as f32;
        assert!((db - want).abs() < 1e-4, "window {w}: {db} vs {want}");
    }
    // The tone at a third of full scale, then silence.
    assert!(
        levels.db[2..9]
            .iter()
            .all(|&db| (-25.0..-5.0).contains(&db)),
        "{:?}",
        levels.db
    );
    assert!(
        levels.db[12..].iter().all(|&db| db < -100.0),
        "{:?}",
        levels.db
    );
    // No sound cache: no levels.
    assert!(
        EngineMedia::new(None)
            .levels(&file, &AtomicBool::new(false))
            .is_none()
    );
}

#[test]
fn chat_reads_scene_items_and_turns_in_its_own_types() {
    let entries = scene_entries(&cached_scenes()).items;
    assert_eq!(entries.len(), 1);
    let e = &entries[0];
    assert_eq!(
        (
            e.start_us,
            e.end_us,
            e.kind.as_str(),
            e.who.as_str(),
            e.text.as_str()
        ),
        (500_000, 3_950_000, "speech", "host", "Says hello")
    );
    assert_eq!(e.on_screen_text.as_deref(), Some("HELLO"));
    let mut quiet = cached_scenes();
    quiet.items[0].on_screen_text.clear();
    quiet.items[0].kind = SceneKind::Shot;
    let e = &scene_entries(&quiet).items[0];
    assert_eq!((e.kind.as_str(), e.on_screen_text.clone()), ("shot", None));

    let turns = speaker_turns(&cached_transcript().turns);
    assert_eq!(turns.len(), 1);
    assert_eq!(
        (
            turns[0].start_us,
            turns[0].end_us,
            turns[0].speaker.as_deref()
        ),
        (500_000, 4_000_000, Some("A"))
    );
}

/// Two starts for one clip at once (a double click, the page and a retry) analyse it once.
#[tokio::test(flavor = "multi_thread")]
async fn a_clip_started_twice_at_once_is_analysed_once() {
    let server = MockServer::start().await;
    let h = harness(&server, key_store(), FakeMedia::whole(SECONDS));
    Mock::given(method("POST"))
        .and(path("/v1/asr/transcribe"))
        .respond_with(asr_answer().set_delay(Duration::from_millis(200)))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/align"))
        .respond_with(align_answer())
        .expect(1)
        .mount(&server)
        .await;
    nothing_else(&server).await;
    let clip = asset("talk", AssetKind::Audio, &h.clip("talk.m4a", 2), SECONDS);

    let (first, second) = tokio::join!(
        h.state.start(std::slice::from_ref(&clip)),
        h.state.start(std::slice::from_ref(&clip))
    );
    first.finished().await;
    second.finished().await;

    assert_eq!(
        h.state.state_of("talk"),
        AssetState::Done { approximate: false }
    );
    server.verify().await;
}

/// A clip whose transcript is cached but not its scene log (its credit ran out last time): the estimate offers only the
/// scene log, and the transcript is in the engine and for chat at once, before anything starts.
#[tokio::test(flavor = "multi_thread")]
async fn a_cached_transcript_shows_before_the_rest_is_analysed() {
    let server = MockServer::start().await;
    nothing_else(&server).await;
    let h = harness(&server, key_store(), FakeMedia::whole(SECONDS));
    let file = h.clip("a.mp4", 1);
    let transcript = cached_transcript();
    h.cache_dir(&file).store("transcript", &transcript).unwrap();
    let clip = asset("clip-a", AssetKind::Video, &file, SECONDS);

    let estimate = h.state.estimate(std::slice::from_ref(&clip)).await;

    assert_eq!(estimate.clips, ["clip-a"]);
    assert!((estimate.usd - scene_usd(SECONDS)).abs() < 1e-12);
    assert_eq!(
        h.last_speech().transcripts["clip-a"].words,
        transcript.transcript.words
    );
    assert!(h.state.analysis_for("clip-a").unwrap().transcript.is_some());
    assert_eq!(h.state.state_of("clip-a"), AssetState::None);
    server.verify().await;
}

/// The same file imported twice (two assets on one content): it is priced once and analysed once, and both assets hear
/// every step and get its words.
#[tokio::test(flavor = "multi_thread")]
async fn one_file_imported_twice_is_analysed_and_priced_once() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/asr/transcribe"))
        .respond_with(asr_answer())
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/align"))
        .respond_with(align_answer())
        .expect(1)
        .mount(&server)
        .await;
    nothing_else(&server).await;
    let h = harness(&server, key_store(), FakeMedia::whole(SECONDS));
    let file = h.clip("talk.m4a", 2);
    let first = asset("first", AssetKind::Audio, &file, SECONDS);
    let second = asset("second", AssetKind::Audio, &file, SECONDS);

    let estimate = h.state.estimate(&[first.clone(), second.clone()]).await;
    assert_eq!(estimate.clips, ["first", "second"]);
    assert!(
        (estimate.usd - asr_usd(SECONDS)).abs() < 1e-12,
        "{}",
        estimate.usd
    );
    assert!((estimate.seconds - SECONDS).abs() < 1e-9);

    h.state.start(&[first, second]).await.finished().await;

    let speech = h.last_speech();
    for id in ["first", "second"] {
        assert_eq!(
            h.state.state_of(id),
            AssetState::Done { approximate: false }
        );
        assert_eq!(
            h.events_for(id).last(),
            Some(&AnalysisEvent::Done(AnalysisDone {
                asset_id: id.into(),
                approximate: false
            }))
        );
        assert!(
            h.events_for(id)
                .iter()
                .any(|e| matches!(e, AnalysisEvent::Progress(p) if p.step == AnalysisStep::Words))
        );
        assert_eq!(speech.transcripts[id].words.len(), 3);
    }
    server.verify().await;
}

/// With an access code, a run going has promised its estimate: a second start whose estimate fits the credit the relay
/// reports, but not what is left after the first, is refused with the plain sentence. Once the first ends, its promise
/// is released.
#[tokio::test(flavor = "multi_thread")]
async fn a_second_start_counts_the_credit_the_first_promised() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/devices/me"))
        .respond_with(device(2.0, 1.9))
        .mount(&server)
        .await;
    let h = harness(&server, code_store(), FakeMedia::whole(SECONDS));
    Mock::given(method("POST"))
        .and(path("/meta/v1/asr/transcribe"))
        .respond_with(asr_answer().set_delay(Duration::from_millis(300)))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/align"))
        .respond_with(align_answer())
        .mount(&server)
        .await;
    mount_scenes(
        &server,
        "/meta/v1",
        ResponseTemplate::new(200).set_body_json(spark_answer()),
    )
    .await;
    nothing_else(&server).await;
    let first = asset("first", AssetKind::Video, &h.clip("a.mov", 1), 155.04);
    let second = asset("second", AssetKind::Video, &h.clip("b.mov", 2), 155.04);
    let each = asr_usd(155.04) + scene_usd(155.04);
    assert!(each < 0.1 && 2.0 * each > 0.1, "{each}");

    let started = h.state.start(std::slice::from_ref(&first)).await;
    assert_eq!(started.refused, None);
    let refusal = "Your access code has $0.03 left, and this needs about $0.07. Trim the clips first, or add your own \
                   Meta key in Settings.";
    let estimate = h.state.estimate(std::slice::from_ref(&second)).await;
    assert_eq!(estimate.refusal.as_deref(), Some(refusal));
    let refused = h.state.start(std::slice::from_ref(&second)).await;
    assert_eq!(refused.refused.as_deref(), Some(refusal));
    refused.finished().await;
    started.finished().await;

    assert_eq!(
        h.state.state_of("first"),
        AssetState::Done { approximate: false }
    );
    assert_eq!(h.state.state_of("second"), AssetState::None);
    // The first's promise is released: the second fits again (the mock's spend doesn't move).
    let estimate = h.state.estimate(std::slice::from_ref(&second)).await;
    assert_eq!(estimate.refusal, None);
    assert!((estimate.credit_left_usd.unwrap() - 0.1).abs() < 1e-9);
    server.verify().await;
}

/// A code whose device can't register now: the estimate says why (to show with Retry), never a $0 price to accept.
#[tokio::test(flavor = "multi_thread")]
async fn the_estimate_says_why_a_code_cannot_pay_yet() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/devices"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;
    nothing_else(&server).await;
    let store = MemoryStore::default();
    store.set(RELAY_CODE, &secret(CODE)).unwrap();
    let h = harness(&server, store, FakeMedia::whole(SECONDS));
    let clip = asset("long", AssetKind::Video, &h.clip("long.mov", 1), 155.04);

    let estimate = h.state.estimate(std::slice::from_ref(&clip)).await;

    assert_eq!(
        estimate.problem.as_deref(),
        Some("Klipolo's server is busy right now. Try again in a moment.")
    );
    assert_eq!(estimate.clips, ["long"]);
    // Nothing to pay with: nothing to ask for, and no problem to report (the page asks for a key or code).
    let none = harness(&server, MemoryStore::default(), FakeMedia::whole(SECONDS));
    let other = asset("other", AssetKind::Audio, &none.clip("b.m4a", 2), SECONDS);
    let estimate = none.state.estimate(std::slice::from_ref(&other)).await;
    assert_eq!((estimate.problem, estimate.usd), (None, 0.0));
    server.verify().await;
}

/// Another project opens while a clip is being analysed: its run is cancelled, the old project's clip is forgotten (no
/// more events for it, no state), and the engine's speech starts empty.
#[tokio::test(flavor = "multi_thread")]
async fn another_project_stops_analysis_and_starts_with_no_speech() {
    let server = MockServer::start().await;
    let h = harness(&server, key_store(), FakeMedia::whole(SECONDS));
    mount_speech(&server, "/v1", &h.aligns, Duration::from_millis(400)).await;
    nothing_else(&server).await;
    let clip = asset("talk", AssetKind::Audio, &h.clip("talk.m4a", 2), SECONDS);

    let started = h.state.start(std::slice::from_ref(&clip)).await;
    let deadline = Instant::now() + Duration::from_secs(20);
    while h.aligns.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline, "word timing never started");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(h.last_speech().transcripts.contains_key("talk"));
    let told = h.events.lock().unwrap().len();
    h.state.project_changed();
    started.finished().await;

    assert!(h.last_speech().transcripts.is_empty() && h.last_speech().levels.is_empty());
    assert_eq!(h.state.state_of("talk"), AssetState::None);
    assert!(h.state.states().is_empty());
    assert!(h.state.analysis_for("talk").is_none());
    assert_eq!(
        h.events.lock().unwrap().len(),
        told,
        "nothing told about the old project"
    );
    server.verify().await;
}

/// Word timing is down the first time (the words are spread, and cached so), and back the next: the words are timed
/// again from the cached turns, with no second transcription, and the better transcript replaces the cached one.
#[tokio::test(flavor = "multi_thread")]
async fn rough_words_are_timed_again_for_free_once_word_timing_is_back() {
    let server = MockServer::start().await;
    let h = harness(&server, key_store(), FakeMedia::whole(SECONDS));
    Mock::given(method("POST"))
        .and(path("/v1/asr/transcribe"))
        .respond_with(asr_answer())
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/align"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/align"))
        .respond_with(align_answer())
        .expect(1)
        .with_priority(2)
        .mount(&server)
        .await;
    nothing_else(&server).await;
    let file = h.clip("talk.m4a", 2);
    let clip = asset("talk", AssetKind::Audio, &file, SECONDS);

    h.state
        .start(std::slice::from_ref(&clip))
        .await
        .finished()
        .await;
    assert_eq!(
        h.state.state_of("talk"),
        AssetState::Done { approximate: true }
    );
    let rough: TranscriptOut = h.cache_dir(&file).load("transcript").unwrap();
    assert!(rough.approximate);

    h.state
        .start(std::slice::from_ref(&clip))
        .await
        .finished()
        .await;

    assert_eq!(
        h.state.state_of("talk"),
        AssetState::Done { approximate: false }
    );
    let timed: TranscriptOut = h.cache_dir(&file).load("transcript").unwrap();
    assert!(!timed.approximate && timed.turns.iter().all(|t| t.aligned));
    assert_eq!(
        h.last_speech().transcripts["talk"].words,
        timed.transcript.words
    );
    assert_eq!(requests(&server, "POST", "/v1/align").await.len(), 2);
    server.verify().await;
}

/// Cancel while words are being timed: what was heard is cached (it is paid for), the clip is analysed as far as it
/// got, and the next run sends no transcription.
#[tokio::test(flavor = "multi_thread")]
async fn cancel_during_word_timing_keeps_what_was_heard() {
    let server = MockServer::start().await;
    let h = harness(&server, key_store(), FakeMedia::whole(SECONDS));
    Mock::given(method("POST"))
        .and(path("/v1/asr/transcribe"))
        .respond_with(asr_answer())
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/align"))
        .respond_with(Counted {
            count: h.aligns.clone(),
            template: align_answer().set_delay(Duration::from_millis(400)),
        })
        .mount(&server)
        .await;
    nothing_else(&server).await;
    let file = h.clip("talk.m4a", 2);
    let clip = asset("talk", AssetKind::Audio, &file, SECONDS);

    let started = h.state.start(std::slice::from_ref(&clip)).await;
    let deadline = Instant::now() + Duration::from_secs(20);
    while h.aligns.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline, "word timing never started");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    h.state.cancel();
    started.finished().await;
    // The request in flight finished: its words are timed.
    assert_eq!(
        h.state.state_of("talk"),
        AssetState::Done { approximate: false }
    );
    assert!(
        h.cache_dir(&file)
            .load::<TranscriptOut>("transcript")
            .is_some(),
        "what was heard is cached"
    );

    h.state
        .start(std::slice::from_ref(&clip))
        .await
        .finished()
        .await;

    assert_eq!(
        h.state.state_of("talk"),
        AssetState::Done { approximate: false }
    );
    server.verify().await;
}

/// A clip's sound levels are cached with its transcript: a clip analysed before gets them with no decode.
#[tokio::test(flavor = "multi_thread")]
async fn cached_levels_need_no_decode() {
    let server = MockServer::start().await;
    let media = FakeMedia::whole(SECONDS);
    let h = harness(&server, key_store(), media.clone());
    mount_speech(&server, "/v1", &h.aligns, Duration::ZERO).await;
    nothing_else(&server).await;
    let file = h.clip("talk.m4a", 2);
    let clip = asset("talk", AssetKind::Audio, &file, SECONDS);

    h.state
        .start(std::slice::from_ref(&clip))
        .await
        .finished()
        .await;
    assert_eq!(media.levels.load(Ordering::SeqCst), 1);
    assert!(h.cache_dir(&file).load::<Value>("levels").is_some());

    h.state
        .start(std::slice::from_ref(&clip))
        .await
        .finished()
        .await;

    assert_eq!(
        media.levels.load(Ordering::SeqCst),
        1,
        "read from the cache"
    );
    assert_eq!(h.last_speech().levels["talk"].db.len(), 60);
    server.verify().await;
}

/// Cancel stops word timing for good in this app run: the clip ends analysed with its words timed roughly, and the
/// page's own start (loading cached clips as the project opens) neither decodes it again nor sends an align request.
/// Only the person's Analyze times its words again.
#[tokio::test(flavor = "multi_thread")]
async fn cancelled_word_timing_does_not_start_again_by_itself() {
    let server = MockServer::start().await;
    let media = FakeMedia::whole(SECONDS);
    let h = harness(&server, key_store(), media.clone());
    Mock::given(method("POST"))
        .and(path("/v1/asr/transcribe"))
        .respond_with(asr_answer())
        .expect(1)
        .mount(&server)
        .await;
    // The first align request is told to wait a second (busy); Cancel lands meanwhile, so its second try never goes.
    Mock::given(method("POST"))
        .and(path("/v1/align"))
        .respond_with(Counted {
            count: h.aligns.clone(),
            template: ResponseTemplate::new(429)
                .insert_header("retry-after", "1")
                .set_body_json(json!({"error": "busy", "message": "Busy."})),
        })
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/align"))
        .respond_with(Counted {
            count: h.aligns.clone(),
            template: align_answer(),
        })
        .with_priority(2)
        .mount(&server)
        .await;
    nothing_else(&server).await;
    let clip = asset("talk", AssetKind::Audio, &h.clip("talk.m4a", 2), SECONDS);

    let started = h.state.start(std::slice::from_ref(&clip)).await;
    let deadline = Instant::now() + Duration::from_secs(20);
    while h.aligns.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline, "word timing never started");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    h.state.cancel();
    started.finished().await;
    assert_eq!(
        h.state.state_of("talk"),
        AssetState::Done { approximate: true }
    );
    assert_eq!(
        (
            h.aligns.load(Ordering::SeqCst),
            media.sounds.load(Ordering::SeqCst)
        ),
        (1, 1)
    );

    // The page loads the project's cached clips on its own: nothing is timed again.
    h.state
        .start_as(std::slice::from_ref(&clip), Asked::Page)
        .await
        .finished()
        .await;
    assert_eq!(
        h.state.state_of("talk"),
        AssetState::Done { approximate: true }
    );
    assert_eq!(
        (
            h.aligns.load(Ordering::SeqCst),
            media.sounds.load(Ordering::SeqCst)
        ),
        (1, 1),
        "no align request and no decode"
    );

    // The person asks: the words are timed again.
    h.state
        .start(std::slice::from_ref(&clip))
        .await
        .finished()
        .await;
    assert_eq!(
        h.state.state_of("talk"),
        AssetState::Done { approximate: false }
    );
    assert_eq!(h.aligns.load(Ordering::SeqCst), 2);
    server.verify().await;
}

/// Rough words in the cache are timed again by the page's own start once per app run, not on every opening.
#[tokio::test(flavor = "multi_thread")]
async fn the_page_times_rough_words_again_once_per_app_run() {
    let server = MockServer::start().await;
    let h = harness(&server, key_store(), FakeMedia::whole(SECONDS));
    Mock::given(method("POST"))
        .and(path("/v1/align"))
        .respond_with(Counted {
            count: h.aligns.clone(),
            template: ResponseTemplate::new(503),
        })
        .mount(&server)
        .await;
    nothing_else(&server).await;
    let file = h.clip("talk.m4a", 2);
    let mut rough = finish(
        "muse-voice-transcribe-1.0",
        us(SECONDS),
        cached_transcript().turns,
        None,
        0.0003,
    );
    rough.approximate_reason = Some("Word timing is not available right now.".into());
    h.cache_dir(&file).store("transcript", &rough).unwrap();
    let clip = asset("talk", AssetKind::Audio, &file, SECONDS);

    for _ in 0..3 {
        h.state
            .start_as(std::slice::from_ref(&clip), Asked::Page)
            .await
            .finished()
            .await;
    }

    assert_eq!(h.aligns.load(Ordering::SeqCst), 1, "once, not every time");
    assert_eq!(
        h.state.state_of("talk"),
        AssetState::Done { approximate: true }
    );
    server.verify().await;
}

/// A start still waiting for the relay's credit when another project opens: it does nothing (no request, no state,
/// no event for the old project's clip) and promises no credit; an estimate overtaken the same way is empty.
#[tokio::test(flavor = "multi_thread")]
async fn a_start_the_project_switch_overtook_does_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/devices/me"))
        .respond_with(device(2.0, 0.0).set_delay(Duration::from_millis(400)))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/meta/v1/asr/transcribe"))
        .respond_with(asr_answer())
        .expect(0)
        .mount(&server)
        .await;
    nothing_else(&server).await;
    let h = harness(&server, code_store(), FakeMedia::whole(SECONDS));
    let clip = asset("old", AssetKind::Video, &h.clip("old.mov", 1), 155.04);

    let starting = {
        let (state, clip) = (h.state.clone(), clip.clone());
        tokio::spawn(async move { state.start(std::slice::from_ref(&clip)).await })
    };
    let estimating = {
        let (state, clip) = (h.state.clone(), clip.clone());
        tokio::spawn(async move { state.estimate(std::slice::from_ref(&clip)).await })
    };
    tokio::time::sleep(Duration::from_millis(150)).await;
    h.state.project_changed();
    let told = h.events.lock().unwrap().len();
    let started = starting.await.unwrap();
    assert_eq!(started.refused, None);
    started.finished().await;
    let estimate = estimating.await.unwrap();

    assert!(
        estimate.clips.is_empty() && estimate.usd == 0.0,
        "{estimate:?}"
    );
    assert_eq!(h.state.state_of("old"), AssetState::None);
    assert!(h.state.states().is_empty());
    assert_eq!(
        h.events.lock().unwrap().len(),
        told,
        "nothing told after the switch"
    );
    // Nothing was promised: the next project's estimate has the whole credit.
    let next = asset("new", AssetKind::Video, &h.clip("new.mov", 2), 155.04);
    let estimate = h.state.estimate(std::slice::from_ref(&next)).await;
    assert!((estimate.credit_left_usd.unwrap() - 2.0).abs() < 1e-9);
    server.verify().await;
}

/// A clip's task that panics part-way still ends: its clip fails with a plain sentence, its claim and its promised
/// credit are let go, and it can be analysed again.
#[tokio::test(flavor = "multi_thread")]
async fn a_task_that_panics_still_ends_and_lets_go() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/devices/me"))
        .respond_with(device(2.0, 0.0))
        .mount(&server)
        .await;
    let h = harness_with(
        &server,
        code_store(),
        FakeMedia::whole(SECONDS),
        Some(AnalysisStep::Words),
    );
    mount_speech(&server, "/meta/v1", &h.aligns, Duration::ZERO).await;
    nothing_else(&server).await;
    let clip = asset("talk", AssetKind::Audio, &h.clip("talk.m4a", 2), SECONDS);

    h.state
        .start(std::slice::from_ref(&clip))
        .await
        .finished()
        .await;

    assert_eq!(
        h.state.state_of("talk"),
        AssetState::Failed {
            reason: "Klipolo's analysis stopped unexpectedly. Try again.".into()
        }
    );
    let estimate = h.state.estimate(std::slice::from_ref(&clip)).await;
    assert!(
        (estimate.credit_left_usd.unwrap() - 2.0).abs() < 1e-9,
        "nothing stays promised"
    );
    h.state
        .start(std::slice::from_ref(&clip))
        .await
        .finished()
        .await;
    assert_eq!(
        h.state.state_of("talk"),
        AssetState::Done { approximate: false }
    );
    server.verify().await;
}

/// One of two assets on a file is removed from the project: it is forgotten (no state, no speech, no more events);
/// the other keeps its analysis, and nothing is cancelled.
#[tokio::test(flavor = "multi_thread")]
async fn a_removed_asset_is_forgotten_and_its_twin_keeps_its_analysis() {
    let server = MockServer::start().await;
    let h = harness(&server, key_store(), FakeMedia::whole(SECONDS));
    mount_speech(&server, "/v1", &h.aligns, Duration::ZERO).await;
    nothing_else(&server).await;
    let file = h.clip("talk.m4a", 2);
    let first = asset("first", AssetKind::Audio, &file, SECONDS);
    let second = asset("second", AssetKind::Audio, &file, SECONDS);
    h.state
        .start(&[first.clone(), second.clone()])
        .await
        .finished()
        .await;

    h.state.forget(&["second".to_owned()]);
    let told = h.events.lock().unwrap().len();
    h.state
        .start(std::slice::from_ref(&first))
        .await
        .finished()
        .await;

    let speech = h.last_speech();
    assert!(!speech.transcripts.contains_key("second") && !speech.levels.contains_key("second"));
    assert!(speech.transcripts.contains_key("first"));
    assert_eq!(h.state.state_of("second"), AssetState::None);
    assert!(h.state.analysis_for("second").is_none());
    assert_eq!(
        h.state.state_of("first"),
        AssetState::Done { approximate: false }
    );
    let after: Vec<AnalysisEvent> = h.events.lock().unwrap()[told..].to_vec();
    assert!(after.iter().all(|e| e.asset_id() == "first"), "{after:?}");
    server.verify().await;
}

/// A video whose word timing the person stops goes back to not analysed (its scene log is still to do, and the page
/// offers it), with what was heard cached; nothing of its scene log is sent.
#[tokio::test(flavor = "multi_thread")]
async fn a_video_stopped_during_word_timing_goes_back_to_not_analysed() {
    let server = MockServer::start().await;
    let h = harness(&server, key_store(), FakeMedia::whole(SECONDS));
    Mock::given(method("POST"))
        .and(path("/v1/asr/transcribe"))
        .respond_with(asr_answer())
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/align"))
        .respond_with(Counted {
            count: h.aligns.clone(),
            template: ResponseTemplate::new(429)
                .insert_header("retry-after", "1")
                .set_body_json(json!({"error": "busy", "message": "Busy."})),
        })
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/files"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "file-1"})))
        .expect(0)
        .mount(&server)
        .await;
    nothing_else(&server).await;
    let file = h.clip("talk.mp4", 3);
    let clip = asset("talk", AssetKind::Video, &file, SECONDS);

    let started = h.state.start(std::slice::from_ref(&clip)).await;
    let deadline = Instant::now() + Duration::from_secs(20);
    while h.aligns.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline, "word timing never started");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    h.state.cancel();
    started.finished().await;

    assert_eq!(h.state.state_of("talk"), AssetState::None);
    assert_eq!(
        h.events_for("talk").last(),
        Some(&AnalysisEvent::Stopped(AnalysisStopped {
            asset_id: "talk".into()
        }))
    );
    assert!(
        h.cache_dir(&file)
            .load::<TranscriptOut>("transcript")
            .is_some()
    );
    server.verify().await;
}

/// A start or estimate whose assets were read for an opening the switch has since left (the commands take the opening
/// before reading the assets) does nothing.
#[tokio::test(flavor = "multi_thread")]
async fn assets_read_for_an_opening_left_behind_start_nothing() {
    let server = MockServer::start().await;
    nothing_else(&server).await;
    let media = FakeMedia::whole(SECONDS);
    let h = harness(&server, key_store(), media.clone());
    let clip = asset("talk", AssetKind::Audio, &h.clip("talk.m4a", 2), SECONDS);

    let opening = h.state.opening();
    h.state.project_changed();
    let estimate = h
        .state
        .estimate_at(opening, std::slice::from_ref(&clip))
        .await;
    let started = h
        .state
        .start_at(opening, std::slice::from_ref(&clip), Asked::Person)
        .await;
    started.finished().await;

    assert!(estimate.clips.is_empty() && estimate.cached.is_empty());
    assert!(h.state.states().is_empty());
    assert!(h.events.lock().unwrap().is_empty());
    assert_eq!(media.sounds.load(Ordering::SeqCst), 0);
    server.verify().await;
}
