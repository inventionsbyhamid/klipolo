//! The core on threads (`run_session`): what `klipolo --record-worker` runs. The calling thread runs the backend's shell
//! (the OS's main loop, which owns the menu-bar or tray item); everything else is a thread that sleeps on its input:
//! - **the core** (user-interactive QoS) owns the `SessionCore`, the durable writer and the pipes, takes every input
//!   from one bounded queue in order and carries out the effects;
//! - **the input reader** turns the app's lines into commands; their end (the parent gone) only marks the take
//!   orphaned: it records on, with Stop in the shell's menu;
//! - **the output writer** writes events as lines; a parent that stopped reading never holds the core up (events are
//!   dropped and counted, the journal being the record);
//! - **the ticker** (once a second, free space every 2 s, and the cursor's poll ten times a second), the shell's input forwarder, the pipes' controllers and
//!   forwarders (`pipes`), and a utility thread for the 10 s read-back.
//!
//! **Stop.** After `Finalize`, the writer drains and makes its last tick, then the files are finished (`finish`) on a
//! thread of their own, watched: while the drive still writes, Stop waits (a slow drive with 20 s queued needs time),
//! and heartbeats go on; when nothing moves for 10 s, the take ends with `capture.error.driveStuck` and exit 1, keeping
//! everything durable for recovery. A take that never passed the gate exits 3, and its folder, which
//! holds no run, is removed.

use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::sync_channel;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use klipolo_model::coded::CodedError;
use klipolo_platform::clock::host_ns;
use klipolo_platform::qos::{self, Qos};

use super::core::{SessionCore, Started};
use super::driver::Core;
use super::feeds::{Ending, forward_shell, read_commands, tick, write_events};
use super::pipes::Pipes;
use super::prepare::{Commands, Prepared, prepare};
use crate::backend::Backend;
use crate::cursor::CursorEvent;
use crate::english::msg;
use crate::io::{Io, OsIo};
use crate::media::{EncodedPacket, PcmBlock};
use crate::pipe::{PipeError, PipeEvent};
use crate::preflight::cursor::CursorPlan;
use crate::protocol::{Command, Event};
use crate::readback::Problem;
use crate::session::{SessionDir, SessionLock};
use crate::shell::{SessionEnd, ShellInput, ShellIo};
use crate::writer::DurableWriter;

/// The core's queue: room for a few hundred media messages ahead of a command.
const CORE_QUEUE: usize = 512;
/// The app's commands read while the worker prepares, before the core runs.
const COMMAND_QUEUE: usize = 64;
const OUT_QUEUE: usize = 256;
const SHELL_QUEUE: usize = 64;
pub(super) const TICK: Duration = Duration::from_secs(1);
/// The cursor's poll between ticks (`Input::Poll`).
pub(super) const POLL: Duration = Duration::from_millis(100);

#[cfg(feature = "synthetic")]
pub use super::scripted::run_with;
/// Stop gives up on a drive that moves nothing for this long.
pub(super) const STUCK: Duration = Duration::from_secs(10);

/// Everything the core thread takes, in one queue. Pipe inputs carry the source's index and the pipe's start number.
pub(super) enum Input {
    Command(Command),
    ParentGone,
    Shell(ShellInput),
    Started {
        src: usize,
        start_no: u64,
        result: Result<Started, PipeError>,
    },
    Packet {
        src: usize,
        start_no: u64,
        packet: EncodedPacket,
    },
    Pcm {
        src: usize,
        start_no: u64,
        block: PcmBlock,
    },
    /// The screen's cursor (its source's, or its pipe's: not tied to one start, since a source outlives restarts).
    Cursor(CursorEvent),
    /// The screen pipe's answer to `PipeCmd::CursorInFrames`, at the host time it switched.
    CursorInFrames {
        ok: bool,
        host_ns: u64,
    },
    Event {
        src: usize,
        start_no: u64,
        event: PipeEvent,
    },
    Tick {
        free: Option<u64>,
    },
    /// Ten times a second between ticks: the cursor's watchdog and gathers (`SessionCore::poll`).
    Poll,
    ReadBack(Vec<Problem>),
}

/// The shell runs on this (main) thread. `input` carries the app's commands and `output` the events; the worker passes
/// stdin and stdout, and tests pass pipes. Returns the exit code (`protocol`).
pub fn run_session(
    backend: Box<dyn Backend>,
    dir: SessionDir,
    resume: bool,
    input: Box<dyn BufRead + Send>,
    output: Box<dyn Write + Send>,
) -> i32 {
    let backend: Arc<dyn Backend> = Arc::from(backend);
    let (events, events_rx) = sync_channel(OUT_QUEUE);
    let writer = std::thread::Builder::new()
        .name("klipolo-output".into())
        .spawn(move || write_events(output, events_rx));
    // The app's commands are read from the start, so a Stop sent while a resumed worker prepares is seen before
    // anything opens.
    let (commands, commands_rx) = sync_channel(COMMAND_QUEUE);
    let stop_seen = Arc::new(AtomicBool::new(false));
    spawn(
        "klipolo-input",
        read_commands(input, commands, stop_seen.clone()),
    );
    let ending = Ending {
        events,
        writer,
        dir: dir.clone(),
    };
    match prepare(&*backend, &dir, resume, &commands_rx, &stop_seen) {
        Prepared::Record(lock, core, early) => {
            let commands = Commands {
                early,
                rest: commands_rx,
                stop_seen,
            };
            session(backend, &dir, *core, commands, ending, lock)
        }
        Prepared::Finished(session) => {
            let _ = ending.events.try_send(Event::Finished { session });
            ending.end(0)
        }
        Prepared::Failed(error, code) => {
            let _ = ending.events.try_send(Event::Failed { error });
            ending.end(code)
        }
    }
}

/// `capture.error.driveStuck`, with what happened for the log: the take is on the disk, and recovery finishes it.
pub(super) fn failed_with(detail: &str) -> CodedError {
    msg!("capture.error.driveStuck", detail = detail)
}

/// The take on threads, its shell on this one. The session ends on a thread of its own (`SessionEnd`), so a shell that
/// can't return (AppKit inside a system quit) can still learn the exit code.
fn session(
    backend: Arc<dyn Backend>,
    dir: &SessionDir,
    mut core: SessionCore,
    commands: Commands,
    ending: Ending,
    lock: SessionLock,
) -> i32 {
    let cfg = core.config().clone();
    let pipe_cfg = core.pipe_config();
    let io: Arc<dyn Io> = Arc::new(OsIo);
    let events = ending.events.clone();
    let writer = match DurableWriter::spawn(io.clone(), dir) {
        Ok(writer) => writer,
        Err(e) => {
            let _ = events.try_send(Event::Failed {
                error: failed_with(&e.to_string()),
            });
            drop((events, lock));
            return ending.end(1);
        }
    };
    let (core_tx, core_rx) = sync_channel(CORE_QUEUE);
    let (shell_in, shell_in_rx) = sync_channel(SHELL_QUEUE);
    let (shell_out, shell_out_rx) = sync_channel(SHELL_QUEUE);
    // The commands read while preparing come first (the core thread hasn't started: the queue has room for them).
    for input in commands.early {
        let _ = core_tx.send(input);
    }
    let to_core = core_tx.clone();
    spawn("klipolo-commands", move || {
        for input in commands.rest {
            if to_core.send(input).is_err() {
                return;
            }
        }
    });
    spawn(
        "klipolo-shell-in",
        forward_shell(shell_in_rx, core_tx.clone()),
    );
    let ticking = Arc::new(AtomicBool::new(true));
    let polls = core.cursor_plan() == CursorPlan::Separate;
    spawn(
        "klipolo-ticker",
        tick(dir.root.clone(), core_tx.clone(), ticking.clone(), polls),
    );
    let opening = core.opening(host_ns());
    let (keys, words, hud_corner) = (cfg.keys.clone(), cfg.words.clone(), cfg.hud_corner);
    let camera_bubble = cfg.camera_bubble;
    let hud_display = cfg.captured_display().map(str::to_owned);
    let state = Core {
        pipes: Pipes::new(
            backend.clone(),
            core_tx.clone(),
            &pipe_cfg,
            commands.stop_seen.clone(),
        ),
        core,
        dir: dir.clone(),
        cfg,
        backend: backend.clone(),
        io,
        writer: Some(writer),
        events,
        shell: shell_out,
        core_tx,
        stats: BTreeMap::new(),
        beats: 0,
        stop_line: None,
    };
    let core_thread = std::thread::Builder::new()
        .name("klipolo-core".into())
        .spawn(move || {
            qos::set_current(Qos::UserInteractive);
            state.run(core_rx, opening)
        });
    let end = SessionEnd::default();
    let finishing = {
        let end = end.clone();
        move || {
            let code = core_thread.map_or(1, |t| t.join().unwrap_or(1));
            ticking.store(false, Ordering::Release);
            drop(lock);
            end.set(ending.end(code));
        }
    };
    // Whoever takes it first ends the session: the ending thread, or this one once the shell returns. A thread that
    // can't start never drops it, so the take's lock stays held and the core keeps recording until the shell returns
    // (Part B).
    let finishing = Arc::new(Mutex::new(Some(finishing)));
    let take = |f: &Mutex<Option<_>>| f.lock().unwrap_or_else(PoisonError::into_inner).take();
    let ends = {
        let finishing = finishing.clone();
        std::thread::Builder::new()
            .name("klipolo-ending".into())
            .stack_size(256 * 1024)
            .spawn(move || take(&finishing).map_or((), |finish| finish()))
    };
    if let Err(e) = ends {
        tracing::error!(error = %e, "worker: the session ends on the main thread once the shell returns");
    }
    backend.shell().run(ShellIo {
        input: shell_in,
        output: shell_out_rx,
        keys,
        words,
        hud_corner,
        hud_display,
        end: end.clone(),
        camera_bubble,
    });
    if let Some(finish) = take(&finishing) {
        finish();
    }
    end.wait_forever()
}

pub(super) fn spawn(name: &str, f: impl FnOnce() + Send + 'static) {
    if let Err(e) = std::thread::Builder::new().name(name.into()).spawn(f) {
        tracing::error!(name, error = %e, "worker: a thread didn't start");
    }
}
