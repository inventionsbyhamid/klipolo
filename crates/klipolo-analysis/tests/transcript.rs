//! A clip's transcript: speaker turns from a transcriber, word times from an aligner, one `Transcript` of `Word`s in
//! the clip's sound time. Small fakes stand in for the services; one test goes through the Meta adapter against a mock
//! server. The WAVs are made here: a steady tone is "speech" to the silence check, zeros are silence.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use klipolo_analysis::AnalysisError;
use klipolo_analysis::providers::meta::Meta;
use klipolo_analysis::providers::{
    AlignLimits, AlignTurn, Aligner, Alignment, BoxFuture, ProviderError, ProviderErrorKind,
    SpeakerTurn, TimedWord, Transcriber, Transcription,
};
use klipolo_analysis::transcript::{
    NO_SPEECH, ROUGH_WORDS, Sound, TranscriptOut, align, finish, hear, transcribe_asset,
};
use klipolo_meta::{MetaClient, SecretString};
use klipolo_model::speech::{PAUSE_US, Word, WordKind};
use klipolo_model::time::Us;
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const RATE: u32 = 16_000;

// ---------------------------------------------------------------------------------------------------------------------
// WAVs

/// A mono 16-bit PCM WAV at 16 kHz.
fn wav_of(samples: &[i16]) -> Vec<u8> {
    let data = (samples.len() * 2) as u32;
    let mut w = Vec::with_capacity(44 + data as usize);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&1u16.to_le_bytes()); // mono
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

fn samples(seconds: f64) -> usize {
    (seconds * f64::from(RATE)).round() as usize
}

/// A steady 220 Hz tone at a third of full scale.
fn tone(seconds: f64) -> Vec<i16> {
    (0..samples(seconds))
        .map(|i| {
            let t = i as f64 / f64::from(RATE);
            (0.33 * 32767.0 * (2.0 * std::f64::consts::PI * 220.0 * t).sin()).round() as i16
        })
        .collect()
}

fn silence(seconds: f64) -> Vec<i16> {
    vec![0; samples(seconds)]
}

/// A clip's WAV in a temporary folder of its own.
struct Clip {
    _dir: tempfile::TempDir,
    path: PathBuf,
}

fn clip(parts: &[Vec<i16>]) -> Clip {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sound.wav");
    std::fs::write(&path, wav_of(&parts.concat())).unwrap();
    Clip { _dir: dir, path }
}

/// The samples in a request's WAV (its data size at byte 40, as `wav_slice` writes it).
fn samples_in(wav: &[u8]) -> usize {
    u32::from_le_bytes(wav[40..44].try_into().unwrap()) as usize / 2
}

/// µs of `n` samples, as the chunk planner counts them.
fn us_of(n: usize) -> Us {
    (n as i64) * 1_000_000 / i64::from(RATE)
}

fn secs(s: f64) -> Us {
    (s * 1e6).round() as Us
}

// ---------------------------------------------------------------------------------------------------------------------
// Fakes

/// Counts requests at once.
#[derive(Default)]
struct InFlight {
    now: AtomicUsize,
    most: AtomicUsize,
}

impl InFlight {
    async fn hold(&self, delay_ms: u64) {
        let now = self.now.fetch_add(1, Ordering::SeqCst) + 1;
        self.most.fetch_max(now, Ordering::SeqCst);
        if delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        } else {
            tokio::task::yield_now().await;
        }
        self.now.fetch_sub(1, Ordering::SeqCst);
    }

    fn most(&self) -> usize {
        self.most.load(Ordering::SeqCst)
    }
}

type AsrAnswer = Box<dyn Fn(usize) -> Result<Transcription, ProviderError> + Send + Sync>;

/// A transcriber that answers by the request's length in samples.
struct FakeAsr {
    max_s: u32,
    answer: AsrAnswer,
    /// Each request's samples, in the order they arrived.
    calls: Mutex<Vec<usize>>,
    delay_ms: u64,
    in_flight: InFlight,
    /// What it says it takes at once.
    at_once: usize,
}

impl FakeAsr {
    fn new(
        max_s: u32,
        answer: impl Fn(usize) -> Result<Transcription, ProviderError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            max_s,
            answer: Box::new(answer),
            calls: Mutex::new(Vec::new()),
            delay_ms: 0,
            in_flight: InFlight::default(),
            at_once: 4,
        }
    }

    fn calls(&self) -> Vec<usize> {
        self.calls.lock().unwrap().clone()
    }
}

impl Transcriber for FakeAsr {
    fn model(&self) -> &str {
        "fake-transcribe-1"
    }

    fn max_audio_s(&self) -> u32 {
        self.max_s
    }

    fn max_in_flight(&self) -> usize {
        self.at_once
    }

    fn estimate_usd(&self, seconds: f64) -> f64 {
        seconds / 1000.0
    }

    fn transcribe<'a>(
        &'a self,
        wav: Vec<u8>,
        _keywords: &'a [String],
    ) -> BoxFuture<'a, Result<Transcription, ProviderError>> {
        Box::pin(async move {
            let n = samples_in(&wav);
            self.calls.lock().unwrap().push(n);
            self.in_flight.hold(self.delay_ms).await;
            (self.answer)(n)
        })
    }
}

/// The fake aligner's limits: ten-minute requests (about five preferred) of up to 300 turns, two at once.
const FAKE_LIMITS: AlignLimits = AlignLimits {
    max_audio_ms: 600_000,
    preferred_audio_ms: 300_000,
    max_turns: 300,
    max_text_bytes: 128 * 1024,
    pad_ms: 250,
    overlap_per_turn_ms: 1_000,
    max_in_flight: 2,
};

type AlignAnswer = Box<dyn Fn(&[AlignTurn]) -> Result<Alignment, ProviderError> + Send + Sync>;

/// One align request as the fake aligner got it.
#[derive(Clone, Debug)]
struct AlignCall {
    samples: usize,
    turns: Vec<AlignTurn>,
}

struct FakeAligner {
    limits: AlignLimits,
    answer: AlignAnswer,
    calls: Mutex<Vec<AlignCall>>,
    delay_ms: u64,
    in_flight: InFlight,
}

impl FakeAligner {
    fn new(
        answer: impl Fn(&[AlignTurn]) -> Result<Alignment, ProviderError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            limits: FAKE_LIMITS,
            answer: Box::new(answer),
            calls: Mutex::new(Vec::new()),
            delay_ms: 0,
            in_flight: InFlight::default(),
        }
    }

    fn calls(&self) -> Vec<AlignCall> {
        self.calls.lock().unwrap().clone()
    }
}

impl Aligner for FakeAligner {
    fn limits(&self) -> AlignLimits {
        self.limits
    }

    fn align<'a>(
        &'a self,
        wav: Vec<u8>,
        turns: &'a [AlignTurn],
    ) -> BoxFuture<'a, Result<Alignment, ProviderError>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(AlignCall {
                samples: samples_in(&wav),
                turns: turns.to_vec(),
            });
            self.in_flight.hold(self.delay_ms).await;
            (self.answer)(turns)
        })
    }
}

fn turn(start_ms: u64, end_ms: u64, text: &str) -> SpeakerTurn {
    SpeakerTurn {
        start_ms,
        end_ms,
        text: text.into(),
        speaker: Some("A".into()),
    }
}

fn heard(turns: Vec<SpeakerTurn>) -> Transcription {
    Transcription {
        duration_ms: 0,
        turns,
        cost_usd: 0.001,
    }
}

/// Each turn's words spread by count across the turn as sent, each 10 ms short of the next, at `score`.
fn timed_evenly(turns: &[AlignTurn], score: f64) -> Alignment {
    let mut words = Vec::new();
    for (k, t) in turns.iter().enumerate() {
        let parts: Vec<&str> = t.text.split_whitespace().collect();
        let step = (t.end_ms - t.start_ms) / parts.len().max(1) as u64;
        for (i, w) in parts.iter().enumerate() {
            let start = t.start_ms + step * i as u64;
            words.push(TimedWord {
                text: (*w).into(),
                start_ms: start,
                end_ms: start + step.saturating_sub(10).max(1),
                score,
                turn: k,
            });
        }
    }
    Alignment {
        words,
        skipped: Vec::new(),
        cost_usd: 0.0,
    }
}

fn error(kind: ProviderErrorKind, message: &str) -> ProviderError {
    ProviderError::new(kind, message)
}

fn spoken(out: &TranscriptOut) -> Vec<&Word> {
    out.transcript
        .words
        .iter()
        .filter(|w| w.kind != WordKind::Pause)
        .collect()
}

fn texts(out: &TranscriptOut) -> Vec<String> {
    spoken(out).iter().map(|w| w.text.clone()).collect()
}

/// Sorted by start, never overlapping, a pause word in (and only in) every gap of `PAUSE_US` or more.
fn assert_well_formed(out: &TranscriptOut) {
    let w = &out.transcript.words;
    for pair in w.windows(2) {
        assert!(pair[0].start_us < pair[0].end_us, "{:?}", pair[0]);
        assert!(pair[0].end_us <= pair[1].start_us, "overlap: {pair:?}");
        let gap = pair[1].start_us - pair[0].end_us;
        if pair[0].kind != WordKind::Pause && pair[1].kind != WordKind::Pause {
            assert!(gap < PAUSE_US, "a gap with no pause: {pair:?}");
        }
    }
    for (i, p) in w
        .iter()
        .enumerate()
        .filter(|(_, p)| p.kind == WordKind::Pause)
    {
        assert!(p.text.is_empty());
        assert!(p.end_us - p.start_us >= PAUSE_US, "{p:?}");
        assert!(i > 0 && i + 1 < w.len(), "a pause sits between words");
        assert_eq!(p.start_us, w[i - 1].end_us);
        assert_eq!(p.end_us, w[i + 1].start_us);
    }
}

fn fixture(name: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    )
    .unwrap()
}

/// The clip A fixtures as the services' answers: 9 turns, and 367 timed words (3 under 0.05).
fn clip_a() -> (Transcription, Alignment) {
    let asr: Value = serde_json::from_str(&fixture("asr_clip_a.json")).unwrap();
    let turns = asr["turns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| SpeakerTurn {
            start_ms: t["startMs"].as_u64().unwrap(),
            end_ms: t["endMs"].as_u64().unwrap(),
            text: t["transcript"].as_str().unwrap().into(),
            speaker: t["speaker"].as_str().map(Into::into),
        })
        .collect();
    let align: Value = serde_json::from_str(&fixture("align_clip_a.json")).unwrap();
    let words: Vec<TimedWord> = serde_json::from_value(align["words"].clone()).unwrap();
    (
        Transcription {
            duration_ms: asr["audioDurationMs"].as_u64().unwrap(),
            turns,
            cost_usd: 0.007752,
        },
        Alignment {
            words,
            skipped: Vec::new(),
            cost_usd: 0.0,
        },
    )
}

// ---------------------------------------------------------------------------------------------------------------------
// The core tests

/// Clip A through the Meta adapter (mock Meta and relay): every word's time is the fixture's milliseconds as µs of
/// the clip's sound (one chunk at 0, one align request starting at 0), with pauses in the gaps.
#[tokio::test]
async fn assembles_words_in_source_microseconds() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/asr/transcribe"))
        .and(header("authorization", "Bearer sk-test-key"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(fixture("asr_clip_a.json"), "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/align"))
        .and(header("authorization", "Bearer relay-test-token"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(fixture("align_clip_a.json"), "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;
    let client = MetaClient::direct(SecretString::from("sk-test-key"))
        .with_meta_base(format!("{}/v1", server.uri()))
        .with_relay_base(server.uri())
        .with_align_token(SecretString::from("relay-test-token"))
        .with_ledger(None);
    let meta = Meta::new(client);
    let clip = clip(&[tone(155.04)]);

    let out = transcribe_asset(&meta, &meta, &clip.path, &[])
        .await
        .unwrap();

    let align: Value = serde_json::from_str(&fixture("align_clip_a.json")).unwrap();
    let expected: Vec<(String, Us, Us)> = align["words"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|w| w["score"].as_f64().unwrap() >= 0.05)
        .map(|w| {
            (
                w["text"].as_str().unwrap().to_owned(),
                w["startMs"].as_i64().unwrap() * 1000,
                w["endMs"].as_i64().unwrap() * 1000,
            )
        })
        .collect();
    let got: Vec<(String, Us, Us)> = spoken(&out)
        .iter()
        .map(|w| (w.text.clone(), w.start_us, w.end_us))
        .collect();
    assert_eq!(got.len(), 364);
    assert_eq!(got, expected);
    assert_eq!(got[0], ("But".into(), 60_000, 184_000));
    assert_well_formed(&out);
    assert!(
        out.transcript
            .words
            .iter()
            .any(|w| w.kind == WordKind::Pause)
    );
    assert!(spoken(&out).iter().all(|w| w.kind == WordKind::Word));

    let t = &out.transcript;
    assert_eq!(t.model, "muse-voice-transcribe-1.0");
    assert_eq!(t.language, "en");
    assert_eq!(t.covered_us, 155_040_000);
    assert!(t.complete);
    assert!(!out.approximate);
    assert_eq!(out.approximate_reason, None);
    assert_eq!(out.empty_reason, None);
    assert!((out.cost_usd - 155.04 / 3600.0 * 0.18).abs() < 1e-9);
    assert_eq!(out.turns.len(), 9);
    assert!(out.turns.iter().all(|t| t.aligned && t.chunk == 0));
    assert_eq!(
        (
            out.turns[1].start_us,
            out.turns[1].end_us,
            out.turns[1].speaker.as_deref()
        ),
        (14_380_000, 20_860_000, Some("B"))
    );

    // The align request carried the turns in the WAV sent with it, which starts at the clip's start.
    let requests = server.received_requests().await.unwrap();
    let align = requests
        .iter()
        .find(|r| r.url.path() == "/v1/align")
        .unwrap();
    let body = String::from_utf8_lossy(&align.body);
    assert!(
        body.contains(r#"{"startMs":60,"endMs":10780,"text":"But it's also also"#),
        "{body:.300}"
    );
}

#[tokio::test]
async fn drops_low_score_words() {
    let (asr, align) = clip_a();
    let transcriber = FakeAsr::new(600, move |_| Ok(asr.clone()));
    let aligner = FakeAligner::new(move |_| Ok(align.clone()));
    let clip = clip(&[tone(155.04)]);

    let out = transcribe_asset(&transcriber, &aligner, &clip.path, &[])
        .await
        .unwrap();

    let at = |text: &str, start: Us| {
        spoken(&out)
            .iter()
            .find(|w| w.text == text && w.start_us == start)
            .map(|w| w.confidence)
    };
    assert_eq!(spoken(&out).len(), 367 - 3);
    // The invented repeats (0.0, 0.0 and 0.04) are gone; the words they repeat stay.
    assert_eq!(at("also", 661_000), None);
    assert_eq!(at("my", 15_845_000), None);
    assert_eq!(at("abilities", 56_371_000), None);
    assert!(at("also", 442_000).is_some());
    assert_eq!(texts(&out).iter().filter(|t| *t == "abilities").count(), 1);
    // 0.05 is kept.
    assert_eq!(at("how", 69_671_000), Some(0.05));
    assert!(out.turns.iter().all(|t| t.aligned));
    assert!(!out.approximate);
    assert_well_formed(&out);
}

#[tokio::test]
async fn tags_um_and_uh_as_fillers() {
    let transcriber = FakeAsr::new(600, |_| {
        Ok(heard(vec![turn(
            1_000,
            7_000,
            "So um I think, uh, we should Um. go hmm okay Er",
        )]))
    });
    let aligner = FakeAligner::new(|turns| Ok(timed_evenly(turns, 0.9)));
    let clip = clip(&[tone(8.0)]);

    let out = transcribe_asset(&transcriber, &aligner, &clip.path, &[])
        .await
        .unwrap();

    let kinds: Vec<(String, WordKind)> = spoken(&out)
        .iter()
        .map(|w| (w.text.clone(), w.kind))
        .collect();
    let (f, w) = (WordKind::Filler, WordKind::Word);
    let expected = [
        ("So", w),
        ("um", f),
        ("I", w),
        ("think,", w),
        ("uh,", f),
        ("we", w),
        ("should", w),
        ("Um.", f),
        ("go", w),
        ("hmm", f),
        ("okay", w),
        ("Er", f),
    ]
    .map(|(t, k)| (t.to_owned(), k));
    assert_eq!(kinds, expected);
    // A filler keeps its text and its time, so it can be cut.
    let um = spoken(&out)[1];
    assert!(um.end_us > um.start_us && um.start_us > secs(1.0));
    assert_well_formed(&out);
}

/// The relay is down (or there is no token): captions still come, each turn's words spread across it by character
/// count, marked approximate with the aligner's sentence.
#[tokio::test]
async fn alignment_failure_spreads_words_evenly_and_marks_approximate() {
    let unavailable =
        "Exact word timing is not available right now, so captions use approximate timing.";
    let transcriber = FakeAsr::new(600, |_| {
        Ok(heard(vec![
            turn(1_000, 2_000, "ab abcd ab"),
            turn(5_000, 8_000, "one two three"),
        ]))
    });
    let aligner =
        FakeAligner::new(move |_| Err(error(ProviderErrorKind::Unavailable, unavailable)));
    let clip = clip(&[tone(10.0)]);

    let out = transcribe_asset(&transcriber, &aligner, &clip.path, &[])
        .await
        .unwrap();

    let got: Vec<(String, Us, Us)> = spoken(&out)
        .iter()
        .map(|w| (w.text.clone(), w.start_us, w.end_us))
        .collect();
    // 2 + 4 + 2 characters over 1 s; 3 + 3 + 5 over 3 s.
    let expected = [
        ("ab", 1_000_000, 1_250_000),
        ("abcd", 1_250_000, 1_750_000),
        ("ab", 1_750_000, 2_000_000),
        ("one", 5_000_000, 5_818_181),
        ("two", 5_818_181, 6_636_363),
        ("three", 6_636_363, 8_000_000),
    ]
    .map(|(t, a, b)| (t.to_owned(), a, b));
    assert_eq!(got, expected);
    assert!(spoken(&out).iter().all(|w| w.confidence == 0.0));
    assert_well_formed(&out);
    // The whole file was heard, so the transcript is complete: only its word timing is approximate.
    assert!(out.approximate);
    assert!(out.transcript.complete);
    assert_eq!(out.approximate_reason.as_deref(), Some(unavailable));
    assert!(out.turns.iter().all(|t| !t.aligned));
    assert_eq!(out.empty_reason, None);
}

/// No speech: no request is made (an all-zero clip, one under -60 dBFS), or the transcriber hears nothing.
#[tokio::test]
async fn silent_clip_returns_empty_transcript_with_no_speech_reason() {
    let hiss: Vec<i16> = (0..samples(5.0))
        .map(|i| [-20i16, 7, 31, -3, 12][i % 5])
        .collect();
    for (name, sound) in [("zeros", silence(5.0)), ("hiss", hiss)] {
        let transcriber = FakeAsr::new(600, |_| panic!("no request for silence"));
        let aligner = FakeAligner::new(|_| panic!("no request for silence"));
        let clip = clip(&[sound]);

        let out = transcribe_asset(&transcriber, &aligner, &clip.path, &[])
            .await
            .unwrap();

        assert!(out.transcript.words.is_empty(), "{name}");
        assert_eq!(out.empty_reason.as_deref(), Some(NO_SPEECH), "{name}");
        assert!(
            !out.approximate && out.approximate_reason.is_none(),
            "{name}"
        );
        assert!(out.turns.is_empty(), "{name}");
        assert_eq!(out.cost_usd, 0.0, "{name}");
        assert!(out.transcript.complete, "{name}");
        assert_eq!(out.transcript.covered_us, 5_000_000, "{name}");
        assert!(transcriber.calls().is_empty() && aligner.calls().is_empty());
    }

    // Sound, but nobody speaks: the request was made and paid for, nothing to align.
    let transcriber = FakeAsr::new(600, |_| Ok(heard(Vec::new())));
    let aligner = FakeAligner::new(|_| panic!("nothing to align"));
    let clip = clip(&[tone(5.0)]);
    let out = transcribe_asset(&transcriber, &aligner, &clip.path, &[])
        .await
        .unwrap();
    assert!(out.transcript.words.is_empty());
    assert_eq!(out.empty_reason.as_deref(), Some(NO_SPEECH));
    assert_eq!(transcriber.calls().len(), 1);
    assert!((out.cost_usd - 0.001).abs() < 1e-12);
    assert!(aligner.calls().is_empty());
}

/// Two chunks cut at a pause: the second chunk's times start at its own start, every word is there once, and each
/// align request's words land at its own WAV's start.
#[tokio::test]
async fn chunk_offsets_make_times_continuous() {
    // Requests of at most 10 s: the cut goes in the 0.6 s pause at 7.4 s, so the chunks are about 7.85 s and 4.15 s.
    let transcriber = FakeAsr::new(10, |n| {
        Ok(heard(vec![if n > samples(6.0) {
            turn(500, 7_300, "one two three four")
        } else {
            turn(200, 4_000, "five six seven")
        }]))
    });
    let mut aligner = FakeAligner::new(|turns| Ok(timed_evenly(turns, 0.9)));
    // One turn a request, so each has its own start.
    aligner.limits.preferred_audio_ms = 5_000;
    let clip = clip(&[tone(7.4), silence(0.6), tone(4.0)]);

    let out = transcribe_asset(&transcriber, &aligner, &clip.path, &[])
        .await
        .unwrap();

    let calls = transcriber.calls();
    assert_eq!(calls.len(), 2);
    let first = calls.iter().copied().max().unwrap();
    let cut = us_of(first);
    assert!((secs(7.4)..secs(8.0)).contains(&cut), "cut at {cut}");
    assert_eq!(calls.iter().sum::<usize>(), samples(12.0));

    assert_eq!(
        texts(&out),
        ["one", "two", "three", "four", "five", "six", "seven"]
    );
    assert_well_formed(&out);
    assert_eq!(out.turns.len(), 2);
    assert_eq!((out.turns[0].start_us, out.turns[0].chunk), (500_000, 0));
    assert_eq!(out.turns[1].chunk, 1);
    assert_eq!(out.turns[1].start_us, cut + 200_000);
    assert_eq!(out.turns[1].end_us, cut + 4_000_000);
    let words = spoken(&out);
    let near = |a: Us, b: Us| (a - b).abs() <= 1_000;
    assert!(near(words[0].start_us, 500_000), "{:?}", words[0]);
    assert!(near(words[4].start_us, cut + 200_000), "{:?}", words[4]);
    assert!(words[3].end_us <= cut && words[4].start_us >= cut);
    assert!(out.turns.iter().all(|t| t.aligned));
    assert!(out.transcript.complete);
    assert_eq!(aligner.calls().len(), 2);
}

// ---------------------------------------------------------------------------------------------------------------------
// More of the rules

/// The aligner skips one turn, another doesn't match its audio (median score under 0.5), a third has only invented
/// words: those three are spread, the good one keeps its times.
#[tokio::test]
async fn a_skipped_or_unmatched_turn_alone_is_spread() {
    let transcriber = FakeAsr::new(600, |_| {
        Ok(heard(vec![
            turn(1_000, 3_000, "good words here"),
            turn(4_000, 6_000, "far too many letters"),
            turn(7_000, 9_000, "never said in this audio"),
            turn(10_000, 12_000, "made up"),
        ]))
    });
    let aligner = FakeAligner::new(|turns| {
        let mut a = timed_evenly(turns, 0.9);
        a.words.retain(|w| w.turn != 1);
        a.skipped = vec![(1, "The turn is too short for its text.".into())];
        let scores = [0.3, 0.4, 0.37, 0.9, 0.2];
        for (w, s) in a.words.iter_mut().filter(|w| w.turn == 2).zip(scores) {
            w.score = s;
        }
        for w in a.words.iter_mut().filter(|w| w.turn == 3) {
            w.score = 0.0;
        }
        Ok(a)
    });
    let clip = clip(&[tone(13.0)]);

    let out = transcribe_asset(&transcriber, &aligner, &clip.path, &[])
        .await
        .unwrap();

    let aligned: Vec<bool> = out.turns.iter().map(|t| t.aligned).collect();
    assert_eq!(aligned, [true, false, false, false]);
    assert!(out.approximate && out.transcript.complete);
    assert_eq!(out.approximate_reason.as_deref(), Some(ROUGH_WORDS));
    let words = spoken(&out);
    assert_eq!(words.len(), 3 + 4 + 5 + 2);
    assert!(words[..3].iter().all(|w| w.confidence == 0.9));
    assert!(words[3..].iter().all(|w| w.confidence == 0.0));
    // Spread: "far too many letters" is 3 + 3 + 4 + 7 characters over 2 s.
    assert_eq!((words[3].start_us, words[3].end_us), (4_000_000, 4_352_941));
    assert_eq!(words[6].end_us, 6_000_000);
    assert_eq!(texts(&out)[12..], ["made", "up"]);
    assert_well_formed(&out);
}

/// The relay's daily limit (429): aligning stops at once (no request after the ones already sent, no retry), every
/// turn is spread, and the relay's own sentence is kept for the person.
#[tokio::test]
async fn a_daily_limit_stops_aligning_and_keeps_its_sentence() {
    let quota =
        "This device has used today's 120 minutes of word timing. It resets at midnight UTC.";
    let transcriber = FakeAsr::new(600, |_| {
        Ok(heard(
            (0..6)
                .map(|k| turn(k * 8_000 + 500, k * 8_000 + 6_000, "a few words"))
                .collect(),
        ))
    });
    let mut aligner = FakeAligner::new(move |_| {
        let mut e = error(ProviderErrorKind::RateLimited, quota);
        e.retry_after_s = Some(50_400);
        Err(e)
    });
    aligner.limits.preferred_audio_ms = 7_000;
    aligner.delay_ms = 30;
    let clip = clip(&[tone(48.0)]);

    let out = transcribe_asset(&transcriber, &aligner, &clip.path, &[])
        .await
        .unwrap();

    let sent = aligner.calls().len();
    assert!((1..=2).contains(&sent), "{sent} requests");
    assert_eq!(out.approximate_reason.as_deref(), Some(quota));
    assert!(out.approximate && out.turns.iter().all(|t| !t.aligned));
    assert!(out.transcript.complete);
    assert_eq!(spoken(&out).len(), 18);
    assert_well_formed(&out);
}

/// One request refused as sent spreads only its own turns; the others are timed.
#[tokio::test]
async fn a_refused_align_request_spreads_only_its_own_turns() {
    let refused = "Meta could not process this request. Try again.";
    let transcriber = FakeAsr::new(600, |_| {
        Ok(heard(vec![
            turn(500, 6_000, "first turn"),
            turn(8_500, 14_000, "odd turn"),
            turn(16_500, 22_000, "last turn"),
        ]))
    });
    let mut aligner = FakeAligner::new(move |turns| {
        if turns.iter().any(|t| t.text == "odd turn") {
            Err(error(ProviderErrorKind::Failed, refused))
        } else {
            Ok(timed_evenly(turns, 0.9))
        }
    });
    aligner.limits.preferred_audio_ms = 7_000;
    let clip = clip(&[tone(24.0)]);

    let out = transcribe_asset(&transcriber, &aligner, &clip.path, &[])
        .await
        .unwrap();

    let aligned: Vec<bool> = out.turns.iter().map(|t| t.aligned).collect();
    assert_eq!(aligned, [true, false, true]);
    assert!(out.approximate && out.transcript.complete);
    assert_eq!(aligner.calls().len(), 3);
    assert_eq!(out.approximate_reason.as_deref(), Some(refused));
}

/// A turn heard with no length keeps its words: it gets 0.2 s, never past the next turn's start or its chunk's end
/// (one at the very end reaches back instead).
#[tokio::test]
async fn a_turn_with_no_length_keeps_its_words() {
    let transcriber = FakeAsr::new(600, |_| {
        Ok(heard(vec![
            turn(1_000, 1_000, "question"),
            turn(1_100, 3_000, "next words"),
            turn(5_000, 5_000, "middle"),
            turn(10_000, 10_000, "last"),
        ]))
    });
    let unavailable = "Exact word timing is not available right now.";
    let aligner =
        FakeAligner::new(move |_| Err(error(ProviderErrorKind::Unavailable, unavailable)));
    let clip = clip(&[tone(10.0)]);

    let out = transcribe_asset(&transcriber, &aligner, &clip.path, &[])
        .await
        .unwrap();

    let spans: Vec<(Us, Us)> = out.turns.iter().map(|t| (t.start_us, t.end_us)).collect();
    assert_eq!(
        spans,
        [
            (1_000_000, 1_100_000),
            (1_100_000, 3_000_000),
            (5_000_000, 5_200_000),
            (9_800_000, 10_000_000)
        ]
    );
    assert_eq!(texts(&out), ["question", "next", "words", "middle", "last"]);
    assert_well_formed(&out);
}

/// The relay leaves out a turn too short for its text, and then the turns left must still cover at most the audio plus
/// 1 s each, or the whole request is refused. So every request fits that bound whichever of its turns are left out:
/// here seven short turns would otherwise make room for two voices talking over each other in one request.
#[tokio::test]
async fn align_requests_stay_within_the_bound_whatever_the_aligner_skips() {
    let transcriber = FakeAsr::new(600, |_| {
        let mut said: Vec<SpeakerTurn> = (0..7)
            .map(|i| turn(300 + 100 * i, 350 + 100 * i, &format!("s{i}")))
            .collect();
        said.push(turn(1_000, 5_000, "first voice talking here"));
        said.push(turn(1_000, 5_000, "second voice at once"));
        Ok(heard(said))
    });
    let aligner = FakeAligner::new(|turns| Ok(timed_evenly(turns, 0.9)));
    let clip = clip(&[tone(6.0)]);

    transcribe_asset(&transcriber, &aligner, &clip.path, &[])
        .await
        .unwrap();

    for call in aligner.calls() {
        let audio_ms = call.samples as u64 * 1_000 / u64::from(RATE);
        // The worst the aligner can leave: only the turns whose padded span is over 1 s.
        let worst: u64 = call
            .turns
            .iter()
            .map(|t| {
                let padded = (t.end_ms + 250).min(audio_ms) - t.start_ms.saturating_sub(250);
                padded.saturating_sub(1_000)
            })
            .sum();
        assert!(
            worst <= audio_ms,
            "{worst} ms over {audio_ms} ms: {:?}",
            call.turns
        );
    }
}

/// Requests keep to the aligner's limits: audio, turns, JSON size, and how much the turns may overlap; turns are sent
/// in the WAV's own time, each turn once, never more requests at once than allowed. A turn too big for any request is
/// spread.
#[tokio::test]
async fn align_requests_stay_within_the_aligners_limits() {
    let limits = AlignLimits {
        max_audio_ms: 20_000,
        preferred_audio_ms: 12_000,
        max_turns: 5,
        max_text_bytes: 300,
        pad_ms: 250,
        overlap_per_turn_ms: 1_000,
        max_in_flight: 3,
    };
    let huge = ["word"; 80].join(" ");
    let mut said: Vec<SpeakerTurn> = (0..20)
        .map(|k| {
            turn(
                k * 2_000 + 100,
                k * 2_000 + 1_500,
                &format!("turn {k} has words"),
            )
        })
        .collect();
    // Two people at once, then one long turn of its own.
    said.push(turn(40_100, 45_000, "first voice talking"));
    said.push(turn(40_200, 44_900, "second voice talking"));
    said.push(turn(46_000, 47_000, &huge));
    said.push(turn(48_000, 65_000, "a long answer alone"));
    let all = said.clone();
    let transcriber = FakeAsr::new(600, move |_| Ok(heard(said.clone())));
    let mut aligner = FakeAligner::new(|turns| Ok(timed_evenly(turns, 0.9)));
    aligner.limits = limits;
    aligner.delay_ms = 20;
    let clip = clip(&[tone(66.0)]);

    let out = transcribe_asset(&transcriber, &aligner, &clip.path, &[])
        .await
        .unwrap();

    let calls = aligner.calls();
    let mut sent: Vec<String> = Vec::new();
    for call in &calls {
        let audio_ms = call.samples as u64 * 1_000 / u64::from(RATE);
        let n = call.turns.len() as u64;
        assert!(audio_ms <= limits.max_audio_ms, "{audio_ms} ms");
        assert!(
            n == 1 || audio_ms <= limits.preferred_audio_ms,
            "{audio_ms} ms"
        );
        assert!(call.turns.len() <= limits.max_turns);
        assert!(serde_json::to_string(&call.turns).unwrap().len() <= limits.max_text_bytes);
        let padded: u64 = call
            .turns
            .iter()
            .map(|t| (t.end_ms + 250).min(audio_ms) - t.start_ms.saturating_sub(250))
            .sum();
        assert!(
            padded <= audio_ms + 1_000 * n,
            "{padded} > {audio_ms} + {n} s"
        );
        for t in &call.turns {
            assert!(
                t.start_ms < t.end_ms && t.end_ms <= audio_ms,
                "{t:?} in {audio_ms} ms"
            );
            sent.push(t.text.clone());
        }
    }
    let mut expected: Vec<String> = all
        .iter()
        .map(|t| t.text.clone())
        .filter(|t| *t != huge)
        .collect();
    sent.sort();
    expected.sort();
    assert_eq!(sent, expected);
    assert!(calls.iter().any(|c| c.turns.len() == 5));
    assert!(aligner.in_flight.most() <= 3);
    assert!(aligner.in_flight.most() >= 2);
    // Only the turn no request could hold is spread.
    let spread: Vec<&str> = out
        .turns
        .iter()
        .filter(|t| !t.aligned)
        .map(|t| t.text.as_str())
        .collect();
    assert_eq!(spread, [huge.as_str()]);
    assert!(out.approximate);
    assert_well_formed(&out);
}

/// Requests of at most 10 s plan 9 s chunks, so a tail under 0.5 s can join the chunk before it and stay in limits.
/// Here the 9 s chunk would be cut at 8.85 s (the window reaching into the quiet end), leaving 0.45 s.
#[tokio::test]
async fn a_tiny_last_chunk_joins_the_one_before() {
    let transcriber = FakeAsr::new(10, |_| Ok(heard(vec![turn(100, 8_800, "all of it")])));
    let aligner = FakeAligner::new(|turns| Ok(timed_evenly(turns, 0.9)));
    let clip = clip(&[tone(8.9), silence(0.4)]);

    let out = transcribe_asset(&transcriber, &aligner, &clip.path, &[])
        .await
        .unwrap();

    assert_eq!(transcriber.calls(), [samples(9.3)]);
    assert_eq!(out.turns.len(), 1);
}

/// A silent stretch between chunks makes no request, and the chunk after it keeps its own start.
#[tokio::test]
async fn silent_chunks_make_no_request() {
    let transcriber = FakeAsr::new(10, |_| Ok(heard(vec![turn(1_000, 4_000, "hello there")])));
    let aligner = FakeAligner::new(|turns| Ok(timed_evenly(turns, 0.9)));
    let clip = clip(&[tone(5.0), silence(10.0), tone(5.0)]);

    let out = transcribe_asset(&transcriber, &aligner, &clip.path, &[])
        .await
        .unwrap();

    // Cuts at 8.85 s and 14.85 s (the latest quiet windows): the middle chunk is all zeros.
    let calls = transcriber.calls();
    assert_eq!(calls.len(), 2, "{calls:?}");
    let last = *calls.iter().min().unwrap();
    assert_eq!(us_of(samples(20.0) - last), 14_850_000);
    assert_eq!(out.turns.len(), 2);
    assert_eq!(out.turns[0].start_us, 1_000_000);
    assert_eq!(out.turns[1].start_us, 15_850_000);
    assert_well_formed(&out);
}

/// A transcription request refused for credit stops the whole step with the service's sentence; nothing is aligned,
/// and no part-heard transcript is made (so none is ever taken for a finished one).
#[tokio::test]
async fn a_failed_transcription_stops_with_its_sentence() {
    let used = "This device used its Klipolo credit. Add your Meta API key in Settings.";
    let transcriber = FakeAsr::new(10, move |_| Err(error(ProviderErrorKind::Budget, used)));
    let aligner = FakeAligner::new(|_| panic!("nothing to align"));
    let clip = clip(&[tone(7.4), silence(0.6), tone(4.0)]);

    let err = transcribe_asset(&transcriber, &aligner, &clip.path, &[])
        .await
        .unwrap_err();

    assert!(err.is_stop());
    assert_eq!(err.user_message(), used);
    assert!(matches!(err, AnalysisError::Provider(ref e) if e.kind == ProviderErrorKind::Budget));
    assert!(aligner.calls().is_empty());
}

/// As many requests at once as each service says it takes: the transcriber's `max_in_flight`, the aligner's limits.
#[tokio::test]
async fn runs_as_many_requests_at_once_as_each_service_takes() {
    let mut transcriber = FakeAsr::new(3, |_| Ok(heard(vec![turn(100, 1_500, "some words")])));
    transcriber.delay_ms = 30;
    transcriber.at_once = 5;
    let mut aligner = FakeAligner::new(|turns| Ok(timed_evenly(turns, 0.9)));
    aligner.limits.preferred_audio_ms = 1_000;
    aligner.delay_ms = 30;
    // 1.5 s of sound, then 0.3 s of quiet, 16 times: requests of at most 3 s are cut in each quiet.
    let parts: Vec<Vec<i16>> = (0..16).flat_map(|_| [tone(1.5), silence(0.3)]).collect();
    let clip = clip(&parts);

    let out = transcribe_asset(&transcriber, &aligner, &clip.path, &[])
        .await
        .unwrap();

    let chunks = transcriber.calls().len();
    assert_eq!(chunks, 16);
    assert_eq!(transcriber.in_flight.most(), 5);
    assert_eq!(aligner.calls().len(), chunks);
    assert_eq!(aligner.in_flight.most(), 2);
    assert_eq!(out.turns.len(), chunks);
    assert!(out.turns.windows(2).all(|t| t[0].end_us <= t[1].start_us));
}

// ---------------------------------------------------------------------------------------------------------------------
// The steps, one by one

/// Turn-level captions before alignment: `finish` without a timing keeps every word, each spread across its turn.
#[tokio::test]
async fn finish_without_a_timing_keeps_every_word() {
    let transcriber = FakeAsr::new(600, |_| {
        Ok(heard(vec![
            turn(1_000, 3_000, "one two three"),
            turn(5_000, 6_000, "Um, four."),
        ]))
    });
    let clip = clip(&[tone(8.0)]);

    let sound = Sound::load(&clip.path, transcriber.max_audio_s())
        .await
        .unwrap();
    let said = hear(&transcriber, &sound, &[]).await.unwrap();
    let rough = finish(
        transcriber.model(),
        sound.duration_us,
        said.turns,
        None,
        said.cost_usd,
    );

    assert_eq!(texts(&rough), ["one", "two", "three", "Um,", "four."]);
    assert_eq!(spoken(&rough)[3].kind, WordKind::Filler);
    assert!(rough.approximate && rough.transcript.complete);
    assert_eq!(rough.approximate_reason.as_deref(), Some(ROUGH_WORDS));
    assert!(rough.turns.iter().all(|t| !t.aligned));
    assert!((rough.cost_usd - 0.001).abs() < 1e-12);
    assert_eq!(rough.transcript.covered_us, 8_000_000);
    assert_well_formed(&rough);
}

/// Alignment that failed runs again later on the cached turns (`Sound::load`, `align`, `finish`), with no new
/// transcription.
#[tokio::test]
async fn alignment_runs_again_on_the_cached_turns() {
    let transcriber = FakeAsr::new(600, |_| {
        Ok(heard(vec![
            turn(1_000, 3_000, "one two three"),
            turn(5_000, 7_000, "four five"),
        ]))
    });
    let down = FakeAligner::new(|_| {
        Err(error(
            ProviderErrorKind::Unavailable,
            "Word timing is not available right now.",
        ))
    });
    let clip = clip(&[tone(8.0)]);
    let first = transcribe_asset(&transcriber, &down, &clip.path, &[])
        .await
        .unwrap();
    assert!(first.approximate);
    // As the cache keeps it.
    let cached: TranscriptOut =
        serde_json::from_str(&serde_json::to_string(&first).unwrap()).unwrap();

    let up = FakeAligner::new(|turns| Ok(timed_evenly(turns, 0.9)));
    let sound = Sound::load(&clip.path, transcriber.max_audio_s())
        .await
        .unwrap();
    let timing = align(&up, &sound, &cached.turns).await;
    let again = finish(
        &cached.transcript.model,
        sound.duration_us,
        cached.turns,
        Some(timing),
        cached.cost_usd,
    );

    assert!(!again.approximate && again.approximate_reason.is_none());
    assert!(again.turns.iter().all(|t| t.aligned));
    assert!(spoken(&again).iter().all(|w| w.confidence == 0.9));
    assert_eq!(texts(&again), texts(&first));
    assert_eq!(again.cost_usd, first.cost_usd);
    assert_eq!(transcriber.calls().len(), 1);
    assert_eq!(up.calls().len(), 1);
    assert_well_formed(&again);
}

/// A stop (a used-up allowance) is the sentence the person reads, even after an earlier request was refused as sent.
#[tokio::test]
async fn a_stop_reason_wins_over_a_refused_request() {
    let quota =
        "This device has used today's 120 minutes of word timing. It resets at midnight UTC.";
    let transcriber = FakeAsr::new(600, |_| {
        Ok(heard(vec![
            turn(500, 6_000, "refused turn"),
            turn(8_500, 14_000, "later turn"),
        ]))
    });
    let mut aligner = FakeAligner::new(move |turns| {
        Err(if turns[0].text == "refused turn" {
            error(
                ProviderErrorKind::Failed,
                "Meta could not process this request.",
            )
        } else {
            error(ProviderErrorKind::RateLimited, quota)
        })
    });
    aligner.limits.preferred_audio_ms = 7_000;
    aligner.delay_ms = 20;
    let clip = clip(&[tone(15.0)]);

    let out = transcribe_asset(&transcriber, &aligner, &clip.path, &[])
        .await
        .unwrap();

    assert_eq!(aligner.calls().len(), 2);
    assert_eq!(out.approximate_reason.as_deref(), Some(quota));
}

/// The app runs analysis on its runtime's threads: the future must be `Send`.
#[test]
fn the_analysis_future_can_move_between_threads() {
    fn sends<T: Send>(_: &T) {}
    let transcriber = FakeAsr::new(600, |_| Ok(heard(Vec::new())));
    let aligner = FakeAligner::new(|_| Ok(Alignment::default()));
    let future = transcribe_asset(&transcriber, &aligner, Path::new("sound.wav"), &[]);
    sends(&future);
}

// ---------------------------------------------------------------------------------------------------------------------
// Busy answers and the device's slots, through the Meta adapter in relay mode (mock relay)

const ASR_PATH: &str = "/meta/v1/asr/transcribe";
const ALIGN_PATH: &str = "/v1/align";
const BUSY: &str = "Too many requests at once from this device. Wait for one to finish.";

fn relay_meta(server: &MockServer) -> Meta {
    Meta::new(
        MetaClient::relay(SecretString::from("relay-test-token"))
            .with_relay_base(server.uri())
            .with_meta_base("http://127.0.0.1:9")
            .with_ledger(None),
    )
}

/// One turn, "hello there friends", 0.5 to 4 s into each chunk.
fn asr_answer() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "audioDurationMs": 5000,
        "turns": [{"turnId": 0, "startMs": 500, "endMs": 4000,
                   "transcript": "hello there friends", "speaker": "A"}]
    }))
}

/// Its words, in a request WAV that starts 0.25 s before the turn.
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

fn busy() -> ResponseTemplate {
    ResponseTemplate::new(429)
        .insert_header("retry-after", "1")
        .set_body_json(json!({"error": "busy", "message": BUSY}))
}

async fn mount(server: &MockServer, at: &str, answer: ResponseTemplate, first_only: bool) {
    let mock = Mock::given(method("POST"))
        .and(path(at))
        .respond_with(answer);
    if first_only {
        mock.up_to_n_times(1).with_priority(1).mount(server).await;
    } else {
        mock.mount(server).await;
    }
}

async fn requests_to(server: &MockServer, at: &str) -> usize {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path() == at)
        .count()
}

/// The relay's `busy` (its requests-at-once limit) is waited out for the second it asks, then the request goes again.
#[tokio::test]
async fn a_busy_answer_is_waited_out_and_tried_again() {
    let server = MockServer::start().await;
    mount(&server, ASR_PATH, busy(), true).await;
    mount(&server, ASR_PATH, asr_answer(), false).await;
    mount(&server, ALIGN_PATH, busy(), true).await;
    mount(&server, ALIGN_PATH, align_answer(), false).await;
    let meta = relay_meta(&server);
    let clip = clip(&[tone(5.0)]);

    let started = Instant::now();
    let out = transcribe_asset(&meta, &meta, &clip.path, &[])
        .await
        .unwrap();

    assert!(started.elapsed() >= Duration::from_secs(2));
    assert_eq!(requests_to(&server, ASR_PATH).await, 2);
    assert_eq!(requests_to(&server, ALIGN_PATH).await, 2);
    assert!(!out.approximate, "{:?}", out.approximate_reason);
    assert_eq!(texts(&out), ["hello", "there", "friends"]);
}

/// Still busy after three tries: the request fails as any failure does, with the relay's sentence; it isn't a stop.
#[tokio::test]
async fn a_request_still_busy_after_three_tries_fails() {
    let server = MockServer::start().await;
    mount(&server, ASR_PATH, busy(), false).await;
    let meta = relay_meta(&server);
    let clip = clip(&[tone(5.0)]);

    let err = transcribe_asset(&meta, &meta, &clip.path, &[])
        .await
        .unwrap_err();

    assert_eq!(requests_to(&server, ASR_PATH).await, 3);
    assert_eq!(err.user_message(), BUSY);
    assert!(!err.is_stop());
    assert!(matches!(err, AnalysisError::Provider(ref e) if e.kind == ProviderErrorKind::Busy));
}

/// The daily alignment allowance and the credit are stops: one request each, never tried again.
#[tokio::test]
async fn a_used_up_allowance_or_credit_is_never_tried_again() {
    let quota =
        "This device has used today's 120 minutes of word timing. It resets at midnight UTC.";
    let server = MockServer::start().await;
    mount(&server, ASR_PATH, asr_answer(), false).await;
    let used_up = ResponseTemplate::new(429)
        .insert_header("retry-after", "50400")
        .set_body_json(json!({"error": "align_quota", "message": quota}));
    mount(&server, ALIGN_PATH, used_up, false).await;
    let meta = relay_meta(&server);
    let clip = clip(&[tone(5.0)]);

    let out = transcribe_asset(&meta, &meta, &clip.path, &[])
        .await
        .unwrap();

    assert_eq!(requests_to(&server, ALIGN_PATH).await, 1);
    assert!(out.approximate && out.transcript.complete);
    assert_eq!(out.approximate_reason.as_deref(), Some(quota));

    let credit = "This device has used its Klipolo credit. Add your Meta API key in Settings.";
    let server = MockServer::start().await;
    let spent = ResponseTemplate::new(402)
        .set_body_json(json!({"error": "budget_exhausted", "message": credit}));
    mount(&server, ASR_PATH, spent, false).await;
    let meta = relay_meta(&server);

    let err = transcribe_asset(&meta, &meta, &clip.path, &[])
        .await
        .unwrap_err();

    assert_eq!(requests_to(&server, ASR_PATH).await, 1);
    assert!(err.is_stop());
    assert_eq!(err.user_message(), credit);
}

/// The Meta adapter asked for short requests, and for more at once than one device may send: chunks of at most 3 s,
/// one turn an align request, 8 of each at once. Only the adapter's own slots keep them within the relay's.
struct Eager(Meta);

impl Transcriber for Eager {
    fn model(&self) -> &str {
        self.0.model()
    }

    fn max_audio_s(&self) -> u32 {
        3
    }

    fn max_in_flight(&self) -> usize {
        8
    }

    fn estimate_usd(&self, seconds: f64) -> f64 {
        self.0.estimate_usd(seconds)
    }

    fn transcribe<'a>(
        &'a self,
        wav: Vec<u8>,
        keywords: &'a [String],
    ) -> BoxFuture<'a, Result<Transcription, ProviderError>> {
        self.0.transcribe(wav, keywords)
    }
}

impl Aligner for Eager {
    fn limits(&self) -> AlignLimits {
        AlignLimits {
            preferred_audio_ms: 1_000,
            max_in_flight: 8,
            ..self.0.limits()
        }
    }

    fn align<'a>(
        &'a self,
        wav: Vec<u8>,
        turns: &'a [AlignTurn],
    ) -> BoxFuture<'a, Result<Alignment, ProviderError>> {
        self.0.align(wav, turns)
    }
}

/// Answers every request with `answer` after `DELAY`, keeping when each arrived.
struct Clocked {
    answer: ResponseTemplate,
    arrivals: std::sync::Arc<Mutex<Vec<Instant>>>,
}

const DELAY: Duration = Duration::from_millis(250);

impl wiremock::Respond for Clocked {
    fn respond(&self, _: &wiremock::Request) -> ResponseTemplate {
        self.arrivals.lock().unwrap().push(Instant::now());
        self.answer.clone().set_delay(DELAY)
    }
}

/// The most requests answered at once. A slot is only free again once its answer has come (`DELAY` after its request
/// arrived), so requests arriving within `DELAY` of each other were in flight together.
fn most_at_once(arrivals: &[Instant]) -> usize {
    let window = DELAY - Duration::from_millis(20);
    arrivals
        .iter()
        .map(|a| {
            arrivals
                .iter()
                .filter(|b| **b >= *a && **b < *a + window)
                .count()
        })
        .max()
        .unwrap_or(0)
}

/// Two clips analysed at once through one adapter (the second through a clone) never send this device more than 3
/// Meta calls (the relay takes 4; one is left for chat) or 2 align requests at once.
#[tokio::test(flavor = "multi_thread")]
async fn two_clips_through_one_adapter_stay_within_the_devices_slots() {
    let server = MockServer::start().await;
    let (asr, align) = (
        std::sync::Arc::new(Mutex::new(Vec::new())),
        std::sync::Arc::new(Mutex::new(Vec::new())),
    );
    let asr_turn = ResponseTemplate::new(200).set_body_json(json!({
        "audioDurationMs": 1800,
        "turns": [{"turnId": 0, "startMs": 100, "endMs": 1500, "transcript": "some words", "speaker": "A"}]
    }));
    let words = ResponseTemplate::new(200).set_body_json(json!({
        "words": [
            {"text": "some", "startMs": 250, "endMs": 800, "score": 0.9, "turn": 0},
            {"text": "words", "startMs": 900, "endMs": 1600, "score": 0.9, "turn": 0}
        ],
        "skipped": []
    }));
    for (at, answer, arrivals) in [(ASR_PATH, asr_turn, &asr), (ALIGN_PATH, words, &align)] {
        Mock::given(method("POST"))
            .and(path(at))
            .respond_with(Clocked {
                answer,
                arrivals: arrivals.clone(),
            })
            .mount(&server)
            .await;
    }
    let meta = relay_meta(&server);
    let (one, two) = (Eager(meta.clone()), Eager(meta));
    // 1.5 s of sound, then 0.3 s of quiet, 8 times: 8 chunks each.
    let parts: Vec<Vec<i16>> = (0..8).flat_map(|_| [tone(1.5), silence(0.3)]).collect();
    let (clip_one, clip_two) = (clip(&parts), clip(&parts));

    let (a, b) = tokio::join!(
        transcribe_asset(&one, &one, &clip_one.path, &[]),
        transcribe_asset(&two, &two, &clip_two.path, &[]),
    );

    let (a, b) = (a.unwrap(), b.unwrap());
    assert!(!a.approximate && !b.approximate);
    let (asr, align) = (asr.lock().unwrap().clone(), align.lock().unwrap().clone());
    assert_eq!((asr.len(), align.len()), (16, 16));
    assert_eq!(most_at_once(&asr), 3, "Meta calls at once");
    assert_eq!(most_at_once(&align), 2, "align requests at once");
}
