//! A clip's words from its 16 kHz WAV (what `audio::extract_wav_16k` wrote), in four steps:
//! - `Sound::load`: the WAV's samples mapped from the file (not held in memory), and cut into transcription requests at
//!   pauses (`chunks::plan_with_loud_cuts`), planned one second under the transcriber's limit so a last chunk under
//!   `MIN_CHUNK_US` can join the one before. Each request's WAV is made from the samples when it is sent. A chunk with
//!   nothing louder than `SILENT_PEAK_DBFS` makes no request; a silent clip makes none at all.
//! - `hear`: each chunk's speaker turns from the `Transcriber`, as many requests at once as it takes
//!   (`Transcriber::max_in_flight`). The first failure stops the step.
//! - `align`: the turns' word times from the `Aligner`, in requests within its `AlignLimits`. Each request is the WAV
//!   around its own turns, so a turn is never sent twice and no request carries audio its turns don't need. Words
//!   scoring under `MIN_SCORE` (the transcriber's invented repeats) are dropped here, and a turn whose words don't
//!   match its audio (median score under `MATCH_MEDIAN`) is left untimed. It never fails: what it can't time stays
//!   untimed.
//! - `finish`: one `Transcript`. A turn without times has its text spread across the turn by character count and
//!   makes the result approximate. Fillers are tagged and a `Pause` word fills every gap of `PAUSE_US` or more.
//!
//! Times: the WAV, its chunks, the turns and every word are the clip's SOUND time (`audio`'s module notes). Each
//! chunk's start is added to its turns and each align request's start to its words; the asset's audio lead never is
//! (the captions add it themselves). Chunks meet at pauses with no overlap, so no word is heard twice at a seam.
//!
//! A `Busy` answer (a service's requests-at-once limit) is waited out and the request sent again, at most
//! `BUSY_ATTEMPTS` times in all (`providers::retry_busy`); still busy, it fails like any other answer. Nothing else is
//! retried. A budget or quota stop from the aligner (`ProviderError::is_stop`), or an aligner that is missing, refuses
//! the token, isn't answering or stays busy, ends aligning for the clip: the turns left keep spread words, and the
//! aligner's own sentence is kept for the person (`TranscriptOut::approximate_reason`). Alignment can run again
//! later on the same turns (`Sound::load`, `align`, `finish`), and `finish` without a timing gives the turn-level
//! transcript to show before alignment ends.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::stream::{self, StreamExt, TryStreamExt};
use klipolo_model::speech::{PAUSE_US, Transcript, Word, WordKind};
use klipolo_model::time::{RangeUs, Us};
use serde::{Deserialize, Serialize};

use crate::audio::{self, AudioError, MappedPcm, MonoPcm};
use crate::chunks::{self, sample_at, us};
use crate::error::AnalysisError;
use crate::fillers::is_filler;
use crate::providers::{
    AlignLimits, AlignTurn, Aligner, Alignment, ProviderError, ProviderErrorKind, SpeakerTurn,
    TimedWord, Transcriber, retry_busy,
};

/// The language Voice Transcribe is biased to, and whose fillers are tagged.
pub const LANGUAGE: &str = "en";
/// Aligned words scoring under this aren't in the audio (often a repeat the transcriber invented): dropped.
pub const MIN_SCORE: f64 = 0.05;
/// A turn whose kept words' median score is under this doesn't match its audio (text never spoken there scores about
/// 0.37, real turns 0.74 to 0.97): its words are spread instead.
pub const MATCH_MEDIAN: f64 = 0.5;
/// A turn shorter than this (Voice Transcribe sometimes gives one no length) is made this long, so its words keep a
/// time.
pub const MIN_TURN_US: Us = 200_000;
/// A last chunk shorter than this joins the one before.
pub const MIN_CHUNK_US: Us = 500_000;
/// A WAV (or chunk) whose loudest sample is under this holds nothing to transcribe: no request is made for it.
pub const SILENT_PEAK_DBFS: f32 = -60.0;
/// Why a transcript has no words.
pub const NO_SPEECH: &str = "No speech found.";
/// Why words are approximate when the aligner gave no reason of its own (it skipped a turn, or a turn didn't match).
pub const ROUGH_WORDS: &str = "Some words are timed roughly.";

/// One speaker turn, in the clip's sound time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Turn {
    pub start_us: Us,
    pub end_us: Us,
    pub text: String,
    /// The speaker's label ("A", "B" ...), meaningful within its chunk only.
    pub speaker: Option<String>,
    /// Its chunk (transcription request), from 0.
    pub chunk: u32,
    /// True when its words carry the aligner's times; false when they are spread across the turn.
    pub aligned: bool,
}

/// A clip's transcript and how it was made.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptOut {
    /// `key` is left empty: the caller sets the source file's content key. `complete`: every chunk was heard (a
    /// failed transcription request gives an error, never a part-heard transcript); word timing doesn't change it.
    pub transcript: Transcript,
    pub turns: Vec<Turn>,
    /// Some turns' words are spread across the turn rather than timed.
    pub approximate: bool,
    /// What the requests cost, in US dollars.
    pub cost_usd: f64,
    /// Why there are no words (`NO_SPEECH`), to show instead of captions.
    pub empty_reason: Option<String>,
    /// Why words are approximate: the aligner's own sentence when it stopped (a daily limit, no word timing), else
    /// `ROUGH_WORDS`.
    pub approximate_reason: Option<String>,
}

/// A clip's sound, read once: its samples (each request's WAV is made from them), its length, and the transcription
/// requests.
#[derive(Clone, Debug)]
pub struct Sound {
    samples: Arc<Samples>,
    rate: u32,
    /// The WAV's length.
    pub duration_us: Us,
    /// The transcription requests in order, silent ones left out: none when the whole WAV is silent.
    pub chunks: Vec<RangeUs>,
    /// Chunk cuts that had to go inside sound (no pause within reach), for the log.
    pub loud_cuts: Vec<Us>,
}

/// A sound's samples: read into memory, or mapped from its WAV file (`audio::map_pcm_file`).
#[derive(Debug)]
enum Samples {
    Read(Vec<i16>),
    Mapped(MappedPcm),
}

impl std::ops::Deref for Samples {
    type Target = [i16];

    fn deref(&self) -> &[i16] {
        match self {
            Self::Read(samples) => samples,
            Self::Mapped(map) => map.samples(),
        }
    }
}

impl Sound {
    /// `pcm` (mono 16-bit samples) cut into requests of at most `max_audio_s` each.
    pub fn from_pcm(pcm: MonoPcm, max_audio_s: u32) -> Self {
        Self::from_samples(Samples::Read(pcm.samples), pcm.rate, max_audio_s)
    }

    fn from_samples(samples: Samples, rate: u32, max_audio_s: u32) -> Self {
        if rate == 0 {
            return Self {
                samples: Arc::new(Samples::Read(Vec::new())),
                rate: 1,
                duration_us: 0,
                chunks: Vec::new(),
                loud_cuts: Vec::new(),
            };
        }
        let n = samples.len();
        let plan =
            chunks::plan_with_loud_cuts(&samples, rate, max_audio_s.saturating_sub(1).max(1));
        let mut ranges = plan.chunks;
        let last = ranges.len();
        if last >= 2 && ranges[last - 1].len() < MIN_CHUNK_US {
            ranges[last - 2].end = ranges[last - 1].end;
            ranges.pop();
        }
        ranges.retain(|r| {
            let from = sample_at(r.start, rate).min(n);
            let to = sample_at(r.end, rate).clamp(from, n);
            audible(&samples[from..to])
        });
        let mut loud_cuts = plan.loud_cuts;
        loud_cuts.retain(|cut| ranges.iter().any(|r| r.start == *cut));
        Self {
            samples: Arc::new(samples),
            rate,
            duration_us: us(n, rate),
            chunks: ranges,
            loud_cuts,
        }
    }

    /// `from_pcm` of a mono 16-bit PCM WAV's bytes.
    pub fn from_wav(wav: &[u8], max_audio_s: u32) -> Result<Self, AudioError> {
        Ok(Self::from_pcm(audio::read_pcm(wav)?, max_audio_s))
    }

    /// `from_pcm` of the WAV file at `path`, on a blocking thread: its samples mapped from the file
    /// (`audio::map_pcm_file`), so a long clip's sound isn't held in memory while it is transcribed, or read straight
    /// into memory where they can't be (`audio::read_pcm_file`).
    pub async fn load(path: &Path, max_audio_s: u32) -> Result<Self, AnalysisError> {
        let path = path.to_owned();
        let sound = tokio::task::spawn_blocking(move || match audio::map_pcm_file(&path)? {
            Some(map) => {
                let rate = map.rate;
                Ok(Self::from_samples(Samples::Mapped(map), rate, max_audio_s))
            }
            None => audio::read_pcm_file(&path).map(|pcm| Self::from_pcm(pcm, max_audio_s)),
        })
        .await
        .map_err(|e| AudioError::Io(std::io::Error::other(e.to_string())))??;
        Ok(sound)
    }

    /// Nothing to transcribe.
    pub fn is_silent(&self) -> bool {
        self.chunks.is_empty()
    }

    /// A request's WAV: the samples in `range` (a range past the end stops at the end) behind a WAV header. Made from
    /// samples already read, so it can't fail, and it can be made again for a retry.
    fn wav_of(&self, range: RangeUs) -> Vec<u8> {
        let n = self.samples.len();
        let from = sample_at(range.start, self.rate).min(n);
        let to = sample_at(range.end, self.rate).clamp(from, n);
        let bytes = 2 * (to - from);
        let mut wav = Vec::with_capacity(44 + bytes);
        wav.extend_from_slice(&audio::wav_header(self.rate, 1, bytes as u32));
        for s in &self.samples[from..to] {
            wav.extend_from_slice(&s.to_le_bytes());
        }
        wav
    }

    /// The time of the first sample `wav_of` takes for a range starting at `t`.
    fn slice_start(&self, t: Us) -> Us {
        us(sample_at(t, self.rate).min(self.samples.len()), self.rate)
    }
}

/// Anything at or over `SILENT_PEAK_DBFS`.
fn audible(samples: &[i16]) -> bool {
    let peak = samples
        .iter()
        .map(|&s| i32::from(s).abs())
        .max()
        .unwrap_or(0);
    peak > 0 && 20.0 * (peak as f32 / 32_767.0).log10() >= SILENT_PEAK_DBFS
}

/// The turns heard in a clip's chunks, in sound time.
#[derive(Clone, Debug, PartialEq)]
pub struct Heard {
    pub turns: Vec<Turn>,
    pub cost_usd: f64,
}

/// The aligner's words for each turn.
#[derive(Clone, Debug, PartialEq)]
pub struct Timing {
    /// One entry per turn, in order: its words in sound time (each scoring at least `MIN_SCORE`), or None where they
    /// have no times (spread them).
    pub words: Vec<Option<Vec<Word>>>,
    pub cost_usd: f64,
    /// The aligner's sentence when it failed or stopped.
    pub note: Option<String>,
}

/// What transcribing a clip whose sound lasts `duration_us` is expected to cost, before any call: the transcriber's
/// estimate for all of it (silent stretches are left out only once the sound is read). Word timing is free.
pub fn estimate_usd(transcriber: &dyn Transcriber, duration_us: Us) -> f64 {
    transcriber.estimate_usd(duration_us.max(0) as f64 / 1e6)
}

/// A clip's transcript from its WAV: `Sound::load`, `hear`, `align`, `finish`. A silent clip makes no request. A
/// transcription failure stops it (a budget stop included); an alignment failure only makes words approximate.
pub async fn transcribe_asset(
    transcriber: &dyn Transcriber,
    aligner: &dyn Aligner,
    wav: &Path,
    keywords: &[String],
) -> Result<TranscriptOut, AnalysisError> {
    let model = transcriber.model();
    let sound = Sound::load(wav, transcriber.max_audio_s()).await?;
    if sound.is_silent() {
        return Ok(TranscriptOut::no_speech(model, sound.duration_us, 0.0));
    }
    let heard = hear(transcriber, &sound, keywords).await?;
    if heard.turns.is_empty() {
        return Ok(TranscriptOut::no_speech(
            model,
            sound.duration_us,
            heard.cost_usd,
        ));
    }
    let timing = align(aligner, &sound, &heard.turns).await;
    Ok(finish(
        model,
        sound.duration_us,
        heard.turns,
        Some(timing),
        heard.cost_usd,
    ))
}

/// The turns `transcriber` hears in `sound`'s chunks, `max_in_flight` requests at once, in order and in sound
/// time (each clamped to its chunk; blank ones left out). A busy answer is waited out (`retry_busy`); the first request
/// that fails otherwise ends the step with its error, and the requests still running are dropped.
pub async fn hear(
    transcriber: &dyn Transcriber,
    sound: &Sound,
    keywords: &[String],
) -> Result<Heard, AnalysisError> {
    let mut heard: Vec<(usize, Vec<Turn>, f64)> =
        stream::iter(sound.chunks.iter().copied().enumerate())
            .map(|(i, range)| async move {
                // Made only when its request starts, so at most `max_in_flight` WAVs exist at once.
                let said =
                    retry_busy(|| transcriber.transcribe(sound.wav_of(range), keywords)).await?;
                Ok::<_, AnalysisError>((i, turns_in(said.turns, range, i), said.cost_usd))
            })
            .buffer_unordered(transcriber.max_in_flight().max(1))
            .try_collect()
            .await?;
    heard.sort_by_key(|(i, ..)| *i);
    Ok(Heard {
        cost_usd: heard.iter().map(|(_, _, cost)| cost).sum(),
        turns: heard.into_iter().flat_map(|(_, turns, _)| turns).collect(),
    })
}

/// A chunk's turns in sound time. A turn shorter than `MIN_TURN_US` (one heard with no length) is given that much, so
/// its words keep a time: never past the next turn's start or the chunk's end, and one at the chunk's very end reaches
/// back instead.
fn turns_in(said: Vec<SpeakerTurn>, chunk: RangeUs, index: usize) -> Vec<Turn> {
    let at = |ms: u64| {
        chunk
            .start
            .saturating_add(ms_us(ms))
            .clamp(chunk.start, chunk.end)
    };
    let mut turns: Vec<Turn> = said
        .into_iter()
        .filter(|t| !t.text.trim().is_empty())
        .map(|t| {
            let start = at(t.start_ms);
            Turn {
                start_us: start,
                end_us: at(t.end_ms).max(start),
                text: t.text.trim().to_owned(),
                speaker: t.speaker,
                chunk: index as u32,
                aligned: false,
            }
        })
        .collect();
    turns.sort_by_key(|t| t.start_us);
    for i in 0..turns.len() {
        let start = turns[i].start_us;
        if turns[i].end_us - start >= MIN_TURN_US {
            continue;
        }
        let next = turns[i + 1..]
            .iter()
            .map(|t| t.start_us)
            .find(|&s| s > start);
        let end = (start + MIN_TURN_US)
            .min(chunk.end)
            .min(next.unwrap_or(Us::MAX))
            .max(turns[i].end_us);
        if end > start {
            turns[i].end_us = end;
        } else {
            turns[i].start_us = (end - MIN_TURN_US).max(chunk.start);
        }
    }
    turns
}

/// Word times for `turns` from `aligner`, in requests within its limits, `max_in_flight` at once. A request the
/// aligner refuses as sent (`ProviderErrorKind::Failed`) leaves only its own turns untimed. Any other failure (a
/// budget or quota stop, a refused token, an aligner that is missing, not answering or still busy after
/// `retry_busy`) also stops the requests not yet sent. Only a busy answer is tried again.
pub async fn align(aligner: &dyn Aligner, sound: &Sound, turns: &[Turn]) -> Timing {
    let limits = aligner.limits();
    let batches = batches(turns, sound.duration_us, &limits);
    let stopped = AtomicBool::new(false);
    let (stopped, batches_ref) = (&stopped, &batches);
    // By index, not `&Batch`: a closure taking a borrowed item makes the future fail rustc's `Send` check.
    let outcomes: Vec<(usize, Outcome)> = stream::iter(0..batches.len())
        .map(|b| async move {
            if stopped.load(Ordering::SeqCst) {
                return (b, Outcome::Stopped);
            }
            match request(aligner, sound, &batches_ref[b], turns).await {
                Ok((offset, answer)) => (b, Outcome::Answered(offset, answer)),
                Err(e) => {
                    if e.kind != ProviderErrorKind::Failed {
                        stopped.store(true, Ordering::SeqCst);
                    }
                    (b, Outcome::Failed(e))
                }
            }
        })
        .buffer_unordered(limits.max_in_flight.max(1))
        .collect()
        .await;

    let mut words: Vec<Option<Vec<Word>>> = vec![None; turns.len()];
    let mut cost_usd = 0.0;
    let mut errors: Vec<(usize, ProviderError)> = Vec::new();
    for (b, outcome) in outcomes {
        let (offset, answer) = match outcome {
            Outcome::Stopped => continue,
            Outcome::Failed(e) => {
                errors.push((b, e));
                continue;
            }
            Outcome::Answered(offset, answer) => (offset, answer),
        };
        cost_usd += answer.cost_usd;
        let batch = &batches[b];
        let mut by_turn: Vec<Vec<&TimedWord>> = vec![Vec::new(); batch.turns.len()];
        for w in &answer.words {
            if let Some(own) = by_turn.get_mut(w.turn) {
                own.push(w);
            }
        }
        let skipped: HashSet<usize> = answer.skipped.iter().map(|(k, _)| *k).collect();
        for (k, &i) in batch.turns.iter().enumerate() {
            if !skipped.contains(&k) {
                words[i] = timed(&by_turn[k], offset);
            }
        }
    }
    // The person reads one sentence: a stop or an outage before a single refused request, else the earliest.
    errors.sort_by_key(|(b, e)| (e.kind == ProviderErrorKind::Failed, *b));
    Timing {
        words,
        cost_usd,
        note: errors.into_iter().next().map(|(_, e)| e.message),
    }
}

/// What became of one align request.
enum Outcome {
    /// Not sent: aligning had stopped.
    Stopped,
    /// Its answer, with the time of its WAV's first sample (to add to its words).
    Answered(Us, Alignment),
    Failed(ProviderError),
}

/// One align request: the WAV of `batch.span` and its turns in that WAV's time. Its words come back with the time of
/// the WAV's first sample, to add to them.
async fn request(
    aligner: &dyn Aligner,
    sound: &Sound,
    batch: &Batch,
    turns: &[Turn],
) -> Result<(Us, Alignment), ProviderError> {
    let offset = sound.slice_start(batch.span.start);
    let sent: Vec<AlignTurn> = batch
        .turns
        .iter()
        .map(|&i| AlignTurn {
            start_ms: us_ms(turns[i].start_us - offset),
            end_ms: us_ms(turns[i].end_us - offset),
            text: turns[i].text.clone(),
        })
        .collect();
    let answer = retry_busy(|| aligner.align(sound.wav_of(batch.span), &sent)).await?;
    Ok((offset, answer))
}

/// A turn's aligned words in sound time (`offset`: the request WAV's start), without those under `MIN_SCORE`; None
/// when none are left or they don't match the audio (median under `MATCH_MEDIAN`).
fn timed(words: &[&TimedWord], offset: Us) -> Option<Vec<Word>> {
    let kept: Vec<&TimedWord> = words
        .iter()
        .copied()
        .filter(|w| w.score >= MIN_SCORE && !w.text.trim().is_empty())
        .collect();
    let mut scores: Vec<f64> = kept.iter().map(|w| w.score).collect();
    if median(&mut scores)? < MATCH_MEDIAN {
        return None;
    }
    Some(
        kept.iter()
            .map(|w| Word {
                text: w.text.trim().to_owned(),
                start_us: offset.saturating_add(ms_us(w.start_ms)),
                end_us: offset.saturating_add(ms_us(w.end_ms)),
                confidence: w.score as f32,
                kind: WordKind::Word,
            })
            .collect(),
    )
}

/// The middle value (the mean of the middle two for an even count); None for none.
fn median(v: &mut [f64]) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    })
}

/// One align request: the WAV span it sends (its turns with the aligner's padding, within the clip) and its turns.
#[derive(Debug)]
struct Batch {
    span: RangeUs,
    turns: Vec<usize>,
    /// The turns' padded spans, added up.
    padded_us: Us,
    /// The turns' JSON, each with a comma, at most (their times in the clip, never shorter than in the request).
    text_bytes: usize,
}

impl Batch {
    /// `turns[i]` added, when the request still fits `limits` with it.
    fn try_add(&mut self, i: usize, padded: RangeUs, bytes: usize, limits: &AlignLimits) -> bool {
        let span = RangeUs {
            start: self.span.start.min(padded.start),
            end: self.span.end.max(padded.end),
        };
        let (n, padded_us, text_bytes) = (
            self.turns.len() + 1,
            self.padded_us + padded.len(),
            self.text_bytes + bytes,
        );
        if !fits(span, n, padded_us, text_bytes, limits) {
            return false;
        }
        (self.span, self.padded_us, self.text_bytes) = (span, padded_us, text_bytes);
        self.turns.push(i);
        true
    }
}

/// Whether a request of `n` turns over `span` is within `limits`. One turn may use all of `max_audio_ms`; more keep
/// to `preferred_audio_ms`.
///
/// The padded turns may add up to the audio plus `overlap_per_turn_ms` each, but an aligner leaves out a turn it can't
/// time (too short for its text), and then the turns left must still fit. A turn's padded span is at least two pads,
/// so with an allowance of `overlap_per_turn_ms` less two pads a turn, any turns may be left out and the rest still fit
/// (one alone always does: its span is the request's). Turns that don't overlap always fit; only voices talking over
/// each other are split up sooner. A millisecond a turn is kept spare for rounding to whole ms.
fn fits(span: RangeUs, n: usize, padded_us: Us, text_bytes: usize, limits: &AlignLimits) -> bool {
    let audio = span.len();
    let overlap = ms_us(
        limits
            .overlap_per_turn_ms
            .saturating_sub(2 * limits.pad_ms)
            .saturating_sub(1),
    );
    audio <= ms_us(limits.max_audio_ms)
        && (n == 1 || audio <= ms_us(limits.preferred_audio_ms))
        && n <= limits.max_turns
        // With the commas counted one a turn, the last comma's byte holds one bracket and this `<` the other.
        && text_bytes < limits.max_text_bytes
        && padded_us <= audio + overlap * n as Us
}

/// `turns` (in order) grouped into align requests, greedily: each turn joins the request before it while that still
/// fits, else starts its own. A turn with no length or no text (nothing to time), or too big for any request alone,
/// is in none.
fn batches(turns: &[Turn], duration_us: Us, limits: &AlignLimits) -> Vec<Batch> {
    let pad = ms_us(limits.pad_ms);
    let mut out: Vec<Batch> = Vec::new();
    let mut open: Option<Batch> = None;
    for (i, t) in turns.iter().enumerate() {
        if t.end_us <= t.start_us || t.text.trim().is_empty() {
            continue;
        }
        let padded = RangeUs {
            start: (t.start_us - pad).max(0),
            end: (t.end_us + pad).min(duration_us),
        };
        let bytes = json_len(t) + 1;
        if open
            .as_mut()
            .is_some_and(|b| b.try_add(i, padded, bytes, limits))
        {
            continue;
        }
        out.extend(open.take());
        if fits(padded, 1, padded.len(), bytes, limits) {
            open = Some(Batch {
                span: padded,
                turns: vec![i],
                padded_us: padded.len(),
                text_bytes: bytes,
            });
        }
    }
    out.extend(open);
    out
}

/// A turn's JSON in a request, at most: with its times in the clip, which are never shorter than in the request.
fn json_len(t: &Turn) -> usize {
    serde_json::to_vec(&AlignTurn {
        start_ms: us_ms(t.start_us),
        end_ms: us_ms(t.end_us),
        text: t.text.clone(),
    })
    .map_or(usize::MAX / 2, |json| json.len())
}

/// Whole ms to µs.
fn ms_us(ms: u64) -> Us {
    Us::try_from(ms).unwrap_or(Us::MAX).saturating_mul(1_000)
}

/// µs to the nearest whole ms (never below 0).
fn us_ms(t: Us) -> u64 {
    (t.max(0) as u64).saturating_add(500) / 1_000
}

/// The transcript of `turns` (in order, sound time): each turn's timed words from `timing` where it has them, its text
/// spread across it where not (all of them without a timing: the turn-level transcript). Sets each turn's `aligned`.
/// `cost_usd` is the transcription's; the alignment's is added.
///
/// `turns` are the whole file's, as `hear` gives them only once every chunk has its answer, so the transcript is
/// `complete` (the engine and the export skip a transcript that isn't), however its words are timed: approximate
/// timing is `TranscriptOut::approximate` and each turn's `aligned`.
pub fn finish(
    model: &str,
    duration_us: Us,
    mut turns: Vec<Turn>,
    timing: Option<Timing>,
    cost_usd: f64,
) -> TranscriptOut {
    let (mut timed, align_cost, note) = match timing {
        Some(t) => (t.words, t.cost_usd, t.note),
        None => (Vec::new(), 0.0, None),
    };
    timed.resize(turns.len(), None);
    let mut words = Vec::new();
    for (turn, timed) in turns.iter_mut().zip(timed) {
        turn.aligned = timed.is_some();
        match timed {
            Some(timed) => words.extend(timed),
            None => words.extend(spread(turn)),
        }
    }
    let words = assemble(words, duration_us);
    let approximate = turns.iter().any(|t| !t.aligned);
    TranscriptOut {
        empty_reason: words.is_empty().then(|| NO_SPEECH.to_owned()),
        transcript: Transcript {
            key: String::new(),
            language: LANGUAGE.into(),
            model: model.into(),
            covered_us: duration_us,
            complete: true,
            words,
        },
        turns,
        approximate,
        cost_usd: cost_usd + align_cost,
        approximate_reason: approximate.then(|| note.unwrap_or_else(|| ROUGH_WORDS.to_owned())),
    }
}

/// A turn's words spread across it by character count (punctuation counts), end to end. No model timed them, so
/// their confidence is 0.
fn spread(turn: &Turn) -> Vec<Word> {
    let parts: Vec<&str> = turn.text.split_whitespace().collect();
    let total: i128 = parts.iter().map(|w| w.chars().count() as i128).sum();
    if total == 0 {
        return Vec::new();
    }
    let len = i128::from((turn.end_us - turn.start_us).max(0));
    let mut before = 0i128;
    parts
        .into_iter()
        .map(|w| {
            let start = turn.start_us + (len * before / total) as Us;
            before += w.chars().count() as i128;
            Word {
                text: w.to_owned(),
                start_us: start,
                end_us: turn.start_us + (len * before / total) as Us,
                confidence: 0.0,
                kind: WordKind::Word,
            }
        })
        .collect()
}

/// `words` as one transcript's: inside the clip, sorted by start, each ending by the next one's start (overlapping
/// voices meet end to end), empty ones left out, fillers tagged, and a `Pause` word in every gap of `PAUSE_US` or
/// more.
fn assemble(mut words: Vec<Word>, duration_us: Us) -> Vec<Word> {
    for w in &mut words {
        w.start_us = w.start_us.clamp(0, duration_us);
        w.end_us = w.end_us.clamp(0, duration_us);
        w.kind = if is_filler(LANGUAGE, &w.text) {
            WordKind::Filler
        } else {
            WordKind::Word
        };
    }
    words.sort_by_key(|w| w.start_us);
    for i in 1..words.len() {
        let next = words[i].start_us;
        words[i - 1].end_us = words[i - 1].end_us.min(next);
    }
    words.retain(|w| w.end_us > w.start_us);
    let mut out: Vec<Word> = Vec::with_capacity(words.len() + words.len() / 8);
    for w in words {
        if let Some(prev) = out.last().map(|p| p.end_us)
            && w.start_us - prev >= PAUSE_US
        {
            out.push(Word {
                text: String::new(),
                start_us: prev,
                end_us: w.start_us,
                confidence: 1.0,
                kind: WordKind::Pause,
            });
        }
        out.push(w);
    }
    out
}

impl TranscriptOut {
    /// No words (`NO_SPEECH`): a silent clip, or one where nobody speaks. `cost_usd` is what hearing that cost.
    pub fn no_speech(model: &str, duration_us: Us, cost_usd: f64) -> Self {
        Self {
            transcript: Transcript {
                key: String::new(),
                language: LANGUAGE.into(),
                model: model.into(),
                covered_us: duration_us,
                complete: true,
                words: Vec::new(),
            },
            turns: Vec::new(),
            approximate: false,
            cost_usd,
            empty_reason: Some(NO_SPEECH.into()),
            approximate_reason: None,
        }
    }
}
