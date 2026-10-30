//! The speech worker's wire (the proxy worker's pattern, with audio): the app writes one JSON line per request, a `transcribe`,
//! `findSpeech` or `language` line followed by exactly `samples × 4` bytes of little-endian f32 (16 kHz mono); the
//! worker answers one JSON line per reply on stdout. Logs go to stderr. Closing stdin ends the worker. Every engine's
//! worker speaks it (`engine::serve`).
//!
//! The worker's first line is `ready` (the model loaded) or `failed` with id 0 (it didn't; the worker then exits).

use std::io::{self, BufRead, Read, Write};

use klipolo_model::time::Us;
use serde::{Deserialize, Serialize};

use crate::words::RawWord;

/// 62.5 s of 16 kHz audio: a job's chunk is at most 30 s and a `findSpeech` 60 s (`plan::VAD_BLOCK_US`); the frame cap
/// leaves room.
pub const MAX_SAMPLES: usize = 1_000_000;
/// The longest reply line the app reads, and the most words (or speech spans) a reply may carry: 62 s of speech
/// is a few hundred.
pub const MAX_REPLY_BYTES: usize = 8 << 20;
pub const MAX_REPLY_WORDS: usize = 20_000;
/// The most scores a `language` reply carries: VoxLingua107's labels.
pub const MAX_LANGUAGES: usize = 107;
/// The samples go to and from the pipe through a buffer this big, never one the size of the chunk.
const IO_BYTES: usize = 64 << 10;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Request {
    Transcribe {
        id: u64,
        samples: usize,
        /// None: detect it.
        language: Option<String>,
        prompt: String,
        threads: u32,
    },
    /// Where people speak in the samples that follow: the worker's speech finder (Silero VAD)
    /// answers `speech`; a worker without one answers `failed`, and the job plans that block from the levels.
    FindSpeech { id: u64, samples: usize },
    /// Which of `allowed` is spoken in the samples that follow: a worker with a language finder (VoxLingua107
    /// in klipolo-asr) answers `language`; one without answers `failed`.
    Language {
        id: u64,
        samples: usize,
        allowed: Vec<String>,
    },
    /// Stop request `id` and every request before it: the one in flight answers `aborted` at
    /// its engine's next look, one still queued answers `aborted` without starting, and a later one is never touched.
    Abort { id: u64 },
}

impl Request {
    pub fn id(&self) -> u64 {
        match self {
            Self::Transcribe { id, .. }
            | Self::FindSpeech { id, .. }
            | Self::Language { id, .. }
            | Self::Abort { id } => *id,
        }
    }

    /// How many samples follow the request's line (none after an `abort`).
    pub fn samples(&self) -> usize {
        match self {
            Self::Transcribe { samples, .. }
            | Self::FindSpeech { samples, .. }
            | Self::Language { samples, .. } => *samples,
            Self::Abort { .. } => 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Reply {
    Ready {
        model: String,
    },
    /// `language`: the one asked for, else the one the engine detected; empty when it can't tell (never a guessed
    /// "en").
    Heard {
        id: u64,
        language: String,
        words: Vec<RawWord>,
        compute_ms: f64,
    },
    /// A `findSpeech`'s answer: each stretch of speech as `[start, end)` in µs from the request's start, in order.
    Speech {
        id: u64,
        spans: Vec<[Us; 2]>,
    },
    /// A `language`'s answer: each allowed language's score, best first (at most `MAX_LANGUAGES`), renormalised over
    /// the allowed ones, and `allowed_mass`, how much of the unrestricted score they hold (0–1): a low one says the
    /// speech is probably in none of them (a test saw 0.15 on synthetic German, so never a cut-off alone).
    Language {
        id: u64,
        scores: Vec<(String, f32)>,
        allowed_mass: f32,
    },
    Aborted {
        id: u64,
    },
    /// Request `id` is being computed: said as the worker begins it, then at most 4 times a second
    /// while its engine knows more (whisper's callbacks; transcribe.cpp 0.4.1 has none, so klipolo-asr says only the
    /// first). `at` is the share of its sound decoded so far (0–1). Any such line says the worker is working, so the
    /// app waits for it however slow it is (`runtime::quiet_limit`).
    Progress {
        id: u64,
        at: f32,
    },
    /// Id 0: the worker couldn't start: `model: …` and `state: …`, its model didn't load; `engine: …`, it refused (a
    /// processor without AVX2, bad arguments). The app checked the model's receipt right before, so either is the
    /// program's, never the model's.
    Failed {
        id: u64,
        message: String,
    },
}

/// A worker's `ready` (its model loaded), said only once `lower` has set its priority below normal
/// (`lower::lower`), so every thread it starts afterwards (the stdin reader, its engine's) inherits it.
/// Every worker says it through here (`engine::serve`); a refusal is told on stderr and the
/// worker goes on.
pub fn say_ready(
    out: &mut (impl Write + ?Sized),
    model: &str,
    lower: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    if let Err(e) = lower() {
        eprintln!("speech worker: the lower priority was refused: {e}");
    }
    write_reply(
        out,
        &Reply::Ready {
            model: model.into(),
        },
    )
}

/// One reply line, flushed.
pub fn write_reply(out: &mut (impl Write + ?Sized), r: &Reply) -> io::Result<()> {
    writeln!(out, "{}", serde_json::to_string(r).expect("a reply"))?;
    out.flush()
}

/// The start of every `progress` line (`write_progress`); `parse_reply` reads such a line without allocating.
const PROGRESS: &str = r#"{"type":"progress","#;

/// One `progress` line for request `id`, made in a stack buffer: an engine's callbacks write it, so nothing is
/// allocated.
pub fn write_progress(out: &mut (impl Write + ?Sized), id: u64, at: f32) -> io::Result<()> {
    let mut line = [0u8; 96];
    let mut cur = io::Cursor::new(&mut line[..]);
    let at = if at.is_finite() {
        at.clamp(0.0, 1.0)
    } else {
        0.0
    };
    writeln!(cur, r#"{PROGRESS}"id":{id},"at":{at:.4}}}"#)?;
    let n = cur.position() as usize;
    out.write_all(&line[..n])?;
    out.flush()
}

/// A request as it goes on the wire, part by part: its JSON line, then its samples as little-endian f32, at most
/// `IO_BYTES` at a time through one buffer. `write_request` writes it to a blocking writer; the app's runtime writes
/// the same parts to the worker's pipe without blocking.
pub struct Frame<'a> {
    bytes: Vec<u8>,
    samples: &'a [f32],
    started: bool,
}

impl<'a> Frame<'a> {
    pub fn new(req: &Request, samples: &'a [f32]) -> Self {
        let samples = if req.samples() > 0 { samples } else { &[] };
        let mut bytes = serde_json::to_vec(req).expect("a request");
        bytes.push(b'\n');
        if !samples.is_empty() {
            bytes.reserve(IO_BYTES.saturating_sub(bytes.len()));
        }
        Self {
            bytes,
            samples,
            started: false,
        }
    }

    /// The next part to write; None once all of it is out.
    pub fn next_part(&mut self) -> Option<&[u8]> {
        if !self.started {
            self.started = true;
            return Some(&self.bytes);
        }
        if self.samples.is_empty() {
            return None;
        }
        let (part, rest) = self.samples.split_at(self.samples.len().min(IO_BYTES / 4));
        self.samples = rest;
        self.bytes.clear();
        self.bytes.extend(part.iter().flat_map(|x| x.to_le_bytes()));
        Some(&self.bytes)
    }
}

/// `req` and its samples to a blocking writer, flushed.
pub fn write_request(
    w: &mut (impl Write + ?Sized),
    req: &Request,
    samples: &[f32],
) -> io::Result<()> {
    let mut frame = Frame::new(req, samples);
    while let Some(part) = frame.next_part() {
        w.write_all(part)?;
    }
    w.flush()
}

/// The next request, with its samples (a `transcribe`'s, `findSpeech`'s or `language`'s) in `buf`; None at the end of
/// input. A line that isn't a request, or a frame over `MAX_SAMPLES`, is an error before anything is allocated for its
/// samples.
pub fn read_request<R: BufRead>(r: &mut R, buf: &mut Vec<f32>) -> io::Result<Option<Request>> {
    let mut line = String::new();
    if r.by_ref().take(64 << 10).read_line(&mut line)? == 0 {
        return Ok(None);
    }
    let req: Request = serde_json::from_str(line.trim())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if !matches!(req, Request::Abort { .. }) {
        let samples = req.samples();
        if samples > MAX_SAMPLES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "too many samples",
            ));
        }
        buf.clear();
        buf.reserve(samples);
        let mut bytes = [0u8; IO_BYTES];
        let mut left = samples * 4;
        while left > 0 {
            let part = &mut bytes[..left.min(IO_BYTES)];
            r.read_exact(part)?;
            buf.extend(
                part.chunks_exact(4)
                    .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
            );
            left -= part.len();
        }
    }
    Ok(Some(req))
}

/// One reply line; None for anything else on stdout, a line over `MAX_REPLY_BYTES`, `heard` with more than
/// `MAX_REPLY_WORDS` words, `speech` with more than `MAX_REPLY_WORDS` spans, or `language` with more than
/// `MAX_LANGUAGES` scores. The runtime reads lines with `read_until` through `take(MAX_REPLY_BYTES + 1)`, so a line
/// without an end never grows past it.
pub fn parse_reply(line: &str) -> Option<Reply> {
    if line.len() > MAX_REPLY_BYTES {
        return None;
    }
    let line = line.trim();
    // Up to 4 a second while a request is computed: read into a plain struct (its keys borrowed), so a report costs no
    // allocation here either.
    if line.starts_with(PROGRESS) {
        #[derive(Deserialize)]
        struct Line {
            id: u64,
            at: f32,
        }
        let p: Line = serde_json::from_str(line).ok()?;
        return p.at.is_finite().then(|| Reply::Progress {
            id: p.id,
            at: p.at.clamp(0.0, 1.0),
        });
    }
    let r: Reply = serde_json::from_str(line).ok()?;
    match &r {
        Reply::Heard { words, .. } if words.len() > MAX_REPLY_WORDS => None,
        Reply::Speech { spans, .. } if spans.len() > MAX_REPLY_WORDS => None,
        Reply::Language { scores, .. } if scores.len() > MAX_LANGUAGES => None,
        _ => Some(r),
    }
}
