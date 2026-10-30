//! One loop for every engine: a worker loads its model, then hands its engine to `serve`, which owns the rest
//! of the worker's life. The engine only answers requests; `serve` reads them, says `ready` once the worker is lowered,
//! stops a request when an abort names it or stdin closes, says each request's first `progress` line and passes the
//! engine's later ones at most 4 a second, and times every answer. Each `serve` call has its own state (`Report`), so
//! two loops in one process never touch each other.
//!
//! One request at a time: a reader thread reads stdin (so an `abort` arrives while the engine computes) and queues at
//! most one request behind the one in flight. A request an abort named before it started is answered `aborted` and
//! never reaches the engine; one the engine finishes after an abort named it is `aborted` too, whatever it made.

use std::io::{self, BufRead, Write};
use std::sync::{mpsc, Arc, Mutex, PoisonError};
use std::time::Instant;

use klipolo_model::time::Us;

use crate::protocol::{
    read_request, say_ready, write_progress, write_reply, Reply, Request, MAX_LANGUAGES,
};
use crate::report::Report;
use crate::words::RawWord;

/// One engine inside a speech worker. `serve` owns the rest: the wire, `ready` after `lower`, aborts (an id, or
/// stdin closing), progress lines (at most 4 a second, from a stack buffer) and `compute_ms`.
pub trait SpeechEngine {
    /// One chunk of 16 kHz mono: its words in the chunk's own time and its language ("" when it can't tell). Long work
    /// calls `job.stopped()` often and `job.progress(at)` whenever it knows more; both may be called from any thread.
    fn transcribe(&mut self, ask: &EngineAsk<'_>, job: &Job<'_>) -> Result<Transcribed, String>;
    /// Where people speak (µs from the samples' start); None: this worker has no VAD file.
    fn find_speech(
        &mut self,
        _samples: &[f32],
        _job: &Job<'_>,
    ) -> Option<Result<Vec<[Us; 2]>, String>> {
        None
    }
    /// Each allowed language's score, best first, and the share the allowed ones hold; None: no language finder here.
    fn languages(
        &mut self,
        _samples: &[f32],
        _allowed: &[String],
        _job: &Job<'_>,
    ) -> Option<Result<Languages, String>> {
        None
    }
}

/// A `transcribe` request as an engine sees it.
pub struct EngineAsk<'a> {
    pub samples: &'a [f32],
    /// None (or empty): detect it.
    pub language: Option<&'a str>,
    pub prompt: &'a str,
    pub threads: u32,
}

/// What an engine heard in a chunk.
#[derive(Clone, Debug, PartialEq)]
pub struct Transcribed {
    pub language: String,
    pub words: Vec<RawWord>,
}

/// What a language finder heard: each allowed language's score, best first and renormalised over the allowed ones,
/// and `allowed_mass`, how much of the unrestricted score they hold (0–1; low: probably none of them).
#[derive(Clone, Debug, PartialEq)]
pub struct Languages {
    pub scores: Vec<(String, f32)>,
    pub allowed_mass: f32,
}

/// The request in flight as an engine sees it: whether to stop, and where to report. `Sync`, so decoding threads
/// may report too.
pub struct Job<'a> {
    id: u64,
    state: &'a Report,
    out: &'a Mutex<dyn Write + Send>,
}

impl<'a> Job<'a> {
    /// Public, so `klipolo-asr`'s bench can call its engine outside `serve`.
    pub fn new(id: u64, state: &'a Report, out: &'a Mutex<dyn Write + Send>) -> Self {
        Self { id, state, out }
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn stopped(&self) -> bool {
        self.state.stopped(self.id)
    }

    /// A `progress` line if one is due (`Report::due`): one lock and a stack buffer (`write_progress`), no allocation.
    pub fn progress(&self, at: f32) {
        if self.state.due() {
            let mut out = self.out.lock().unwrap_or_else(PoisonError::into_inner);
            let _ = write_progress(&mut *out, self.id, at);
        }
    }
}

/// What a worker says when its engine has no speech finder, or no language finder.
pub const NO_VAD: &str = "no vad model";
pub const NO_FINDER: &str = "no language finder";

/// Serves `engine` until `input` closes; the worker's exit code. One `Arc<Report>` per call, shared only with this
/// call's stdin reader thread (never a static), and one `Mutex` around `out`.
pub fn serve(
    engine: &mut dyn SpeechEngine,
    model: &str,
    lower: impl FnOnce() -> io::Result<()>,
    input: impl BufRead + Send + 'static,
    out: impl Write + Send + 'static,
) -> i32 {
    let out = Mutex::new(out);
    let out: &Mutex<dyn Write + Send> = &out;
    let say = |r: &Reply| {
        let _ = write_reply(&mut *out.lock().unwrap_or_else(PoisonError::into_inner), r);
    };
    // Lowered before any other thread starts (the reader below, the engine's per request), so they all inherit it.
    if say_ready(
        &mut *out.lock().unwrap_or_else(PoisonError::into_inner),
        model,
        lower,
    )
    .is_err()
    {
        return 1; // the app is gone already
    }
    let report = Arc::new(Report::new());
    let (tx, rx) = mpsc::sync_channel::<(Request, Vec<f32>)>(1);
    let reader = report.clone();
    let read = std::thread::Builder::new()
        .name("speech-stdin".into())
        .spawn(move || read_requests(input, &reader, &tx));
    if let Err(e) = read {
        eprintln!("speech worker: no stdin reader: {e}");
        return 1;
    }
    for (req, samples) in rx {
        let id = req.id();
        // Aborted while it waited behind the request before (a pause, then a resume and a pause within one look): it
        // never runs.
        if report.stopped(id) {
            say(&Reply::Aborted { id });
            continue;
        }
        report.begin(id);
        // Said at once: the app's wait for this request counts from what the worker says.
        {
            let mut o = out.lock().unwrap_or_else(PoisonError::into_inner);
            let _ = write_progress(&mut *o, id, 0.0);
        }
        let job = Job::new(id, &report, out);
        let started = Instant::now();
        let reply = answer(engine, req, &samples, &job, started);
        say(&reply);
    }
    0
}

/// The reader thread: requests go to the loop (at most one waiting), aborts straight to the state. The end of input,
/// or a line that isn't a request, is the app gone: whatever runs stops and the loop ends once the channel closes.
fn read_requests(
    mut input: impl BufRead,
    report: &Report,
    tx: &mpsc::SyncSender<(Request, Vec<f32>)>,
) {
    let mut buf = Vec::new();
    loop {
        match read_request(&mut input, &mut buf) {
            Ok(Some(Request::Abort { id })) => report.abort(id),
            Ok(Some(req)) => {
                if tx.send((req, std::mem::take(&mut buf))).is_err() {
                    break;
                }
            }
            Ok(None) | Err(_) => {
                report.close();
                break;
            }
        }
    }
}

/// One request's reply: `aborted` when an abort named it by the time its engine answered, whatever it answered.
fn answer(
    engine: &mut dyn SpeechEngine,
    req: Request,
    samples: &[f32],
    job: &Job<'_>,
    started: Instant,
) -> Reply {
    let id = job.id();
    let failed = |message: String| Reply::Failed { id, message };
    match req {
        Request::Transcribe {
            language,
            prompt,
            threads,
            ..
        } => {
            let ask = EngineAsk {
                samples,
                language: language.as_deref(),
                prompt: &prompt,
                threads,
            };
            match engine.transcribe(&ask, job) {
                _ if job.stopped() => Reply::Aborted { id },
                Ok(t) => Reply::Heard {
                    id,
                    language: t.language,
                    words: t.words,
                    compute_ms: started.elapsed().as_secs_f64() * 1e3,
                },
                Err(message) => failed(message),
            }
        }
        Request::FindSpeech { .. } => match engine.find_speech(samples, job) {
            _ if job.stopped() => Reply::Aborted { id },
            None => failed(NO_VAD.into()),
            Some(Ok(spans)) => Reply::Speech { id, spans },
            Some(Err(message)) => failed(message),
        },
        Request::Language { allowed, .. } => match engine.languages(samples, &allowed, job) {
            _ if job.stopped() => Reply::Aborted { id },
            None => failed(NO_FINDER.into()),
            Some(Ok(Languages {
                mut scores,
                allowed_mass,
            })) => {
                scores.truncate(MAX_LANGUAGES);
                Reply::Language {
                    id,
                    scores,
                    allowed_mass: if allowed_mass.is_finite() {
                        allowed_mass.clamp(0.0, 1.0)
                    } else {
                        0.0
                    },
                }
            }
            Some(Err(message)) => failed(message),
        },
        // Aborts never reach the loop (the reader takes them).
        Request::Abort { .. } => Reply::Aborted { id },
    }
}
