//! The render worker: `klipolo --render-worker <session folder>`. The app's
//! own binary, started with a cleared environment in its session folder (the app's `make::launch`). Before anything
//! else it closes every descriptor it inherited above 2, then sandboxes itself from its session's
//! `in/profile.json`; only then does it read requests, one JSON line at a time on stdin, and answer on stdout
//! (`protocol`). It configures its fonts from `in/fonts.json` once (`make::fonts`), and holds its own GPU device, made
//! on the first request that needs one and kept warm (`make::gpu`), never the app's. What each request does is in
//! `answer` (validate, compile, measure, frames); a lost device answers the request and ends the worker
//! (`EXIT_GONE`), which the client restarts.

mod answer;
mod compile;
mod cost;
mod scene;
mod stdin;
mod watchdog;

pub use scene::MAX_CHECKED;

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use klipolo_platform::process::sandbox::{Applied, close_inherited, limit_file_size};
use serde::Deserialize;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, BufReader};

use super::measure::{GpuMeter, Meter};
use super::protocol::{MAX_REPLY_LINE, MAX_REQUEST_BYTES, Step, WorkerReply, WorkerRequest};
use answer::{Answer, Worker};

/// The exit code of a worker that couldn't sandbox itself (or read its profile): it read no request. The app then says
/// "Klipolo's effect checker couldn't start safely" and sends it no agent code.
pub const EXIT_UNSAFE: i32 = 5;
/// What the worker may write in its `tmp/` and `out/`, in all (the parent's cap), and so in one file (`RLIMIT_FSIZE`).
pub const MAX_FILE_BYTES: u64 = 256 << 20;
/// The parent is gone (stdout closed, stdin unreadable or closed, a new parent: the watchdog), or the GPU device was
/// lost: the client starts a new worker for its next call.
pub const EXIT_GONE: i32 = 3;
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// `args`: the session folder (the app's `main.rs` passes what follows `--render-worker`).
pub fn main(args: &[String]) -> i32 {
    // First, while only this thread runs: a descriptor another thread opened meanwhile would be closed under it.
    close_inherited();
    let Some(session) = args.first().map(PathBuf::from) else {
        crate::make::say!("usage: --render-worker <session folder>");
        return 2;
    };
    crate::qos::set_ceiling(crate::qos::Qos::UserInitiated);
    crate::qos::set_current(crate::qos::Qos::UserInitiated);
    // Before the sandbox: one file can't pass the parent's disk cap between two of its polls.
    if let Err(e) = limit_file_size(MAX_FILE_BYTES) {
        crate::make::say!("render worker: its file size can't be capped: {e}");
        return EXIT_UNSAFE;
    }
    // Said once, so a test reads the cap on every OS (`/proc` only on Linux).
    if let Some(cap) = klipolo_platform::process::sandbox::file_size_limit() {
        crate::make::say!("render worker: files capped at {cap} bytes");
    }
    let applied = match super::sandbox::apply(&session) {
        Ok(applied) => applied,
        Err(e) => {
            crate::make::say!("render worker: it can't start safely: {e}");
            return EXIT_UNSAFE;
        }
    };
    // Its threads start inside the sandbox. The watchdog ends a worker whose app is gone.
    let closed = Arc::new(AtomicBool::new(false));
    let busy = Arc::new(AtomicBool::new(false));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build();
    let (Ok(runtime), Ok(input), Ok(())) = (
        runtime,
        stdin::Stdin::spawn(closed.clone()),
        watchdog::start(closed, busy.clone(), EXIT_GONE),
    ) else {
        return 2;
    };
    let sandboxed = applied == Applied::Sandboxed;
    runtime.block_on(serve_as(
        input,
        io::stdout(),
        &session,
        sandboxed,
        Box::new(GpuMeter::new()),
        busy,
    ))
}

/// The worker's loop without a sandbox (tests): `Ready` says `sandboxed: false`. Exits 0 on `Shutdown` or when
/// `input` ends, `EXIT_GONE` when its device was lost.
pub async fn serve(input: impl AsyncRead + Unpin, out: impl Write, session: &Path) -> i32 {
    serve_as(
        input,
        out,
        session,
        false,
        Box::new(GpuMeter::new()),
        Arc::default(),
    )
    .await
}

/// `serve` with another meter (tests: `FakeMeter`).
#[doc(hidden)]
pub async fn serve_with(
    meter: Box<dyn Meter>,
    input: impl AsyncRead + Unpin,
    out: impl Write,
    session: &Path,
) -> i32 {
    serve_as(input, out, session, false, meter, Arc::default()).await
}

async fn serve_as(
    input: impl AsyncRead + Unpin,
    mut out: impl Write,
    session: &Path,
    sandboxed: bool,
    meter: Box<dyn Meter>,
    busy: Arc<AtomicBool>,
) -> i32 {
    let mut input = BufReader::new(input);
    let ready = WorkerReply::Ready {
        version: VERSION.into(),
        sandboxed,
    };
    if send(&mut out, &ready, Step::Scan).is_err() {
        return EXIT_GONE;
    }
    // Before the first frame and before the device: the faces the preview uses.
    let mut worker = Worker::new(session, meter);
    let mut line = Vec::new();
    loop {
        let read = read_line(&mut input, &mut line).await;
        // When the request arrived: its measurement's budget runs from here.
        let arrived = std::time::Instant::now();
        let (reply, step) = match read {
            Ok(Line::End) => return 0,
            Err(_) => return EXIT_GONE,
            Ok(Line::TooLong) => (failed(0, Step::Scan, "request too large"), Step::Scan),
            Ok(Line::Read) => match serde_json::from_slice::<WorkerRequest>(&line) {
                Ok(WorkerRequest::Shutdown) => return 0,
                Ok(request) => {
                    let step = request.first_step();
                    // The watchdog's stdin rule waits while a request runs.
                    busy.store(true, Ordering::SeqCst);
                    let answer = worker.answer(request, &mut out, arrived).await;
                    busy.store(false, Ordering::SeqCst);
                    match answer {
                        Answer::Reply(reply) => (reply, step),
                        Answer::Lost(reply) => {
                            let _ = send(&mut out, &reply, step);
                            return EXIT_GONE;
                        }
                    }
                }
                Err(e) => (
                    failed(
                        id_of(&line),
                        Step::Scan,
                        &format!("the request isn't valid: {e}"),
                    ),
                    Step::Scan,
                ),
            },
        };
        if send(&mut out, &reply, step).is_err() {
            return EXIT_GONE;
        }
    }
}

fn failed(id: u64, step: Step, reason: &str) -> WorkerReply {
    WorkerReply::Failed {
        id,
        step,
        reason: reason.into(),
    }
}

/// The id of a request that didn't parse, when it has one.
fn id_of(line: &[u8]) -> u64 {
    #[derive(Deserialize)]
    struct Id {
        id: u64,
    }
    serde_json::from_slice::<Id>(line).map_or(0, |r| r.id)
}

/// One reply line; one over `MAX_REPLY_LINE` becomes a `Failed` at `step` (the parent would refuse it).
fn send(out: &mut dyn Write, reply: &WorkerReply, step: Step) -> io::Result<()> {
    let mut line = serde_json::to_vec(reply).map_err(io::Error::other)?;
    if line.len() > MAX_REPLY_LINE {
        let id = match reply {
            WorkerReply::Validated { id, .. }
            | WorkerReply::Compiled { id, .. }
            | WorkerReply::Measured { id, .. }
            | WorkerReply::Framed { id, .. }
            | WorkerReply::Failed { id, .. } => *id,
            _ => 0,
        };
        line = serde_json::to_vec(&failed(id, step, "the reply was too large"))
            .map_err(io::Error::other)?;
    }
    line.push(b'\n');
    out.write_all(&line)?;
    out.flush()
}

enum Line {
    Read,
    TooLong,
    End,
}

/// The next request line into `line` (without its `\n`), at most `MAX_REQUEST_BYTES`: a longer one is read to its end
/// and dropped, so the worker goes on with the next.
async fn read_line(
    input: &mut (impl AsyncBufRead + Unpin),
    line: &mut Vec<u8>,
) -> io::Result<Line> {
    line.clear();
    let read = (&mut *input)
        .take(MAX_REQUEST_BYTES as u64 + 1)
        .read_until(b'\n', line)
        .await?;
    if read == 0 {
        return Ok(Line::End);
    }
    if line.last() == Some(&b'\n') {
        line.pop();
    } else if line.len() > MAX_REQUEST_BYTES {
        line.clear();
        drain_line(input).await?;
        return Ok(Line::TooLong);
    }
    Ok(Line::Read)
}

async fn drain_line(input: &mut (impl AsyncBufRead + Unpin)) -> io::Result<()> {
    loop {
        let buffer = input.fill_buf().await?;
        if buffer.is_empty() {
            return Ok(());
        }
        match buffer.iter().position(|&b| b == b'\n') {
            Some(at) => {
                input.consume(at + 1);
                return Ok(());
            }
            None => {
                let all = buffer.len();
                input.consume(all);
            }
        }
    }
}
