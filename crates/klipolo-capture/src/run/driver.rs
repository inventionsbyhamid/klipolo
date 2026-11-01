//! The core thread (`run::threads`'s module doc): it owns the `SessionCore`, the durable writer and the pipes, takes
//! every input from one queue in order, carries out the effects, and runs Stop's watched finishing (`stopping`).

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};

use klipolo_platform::clock::host_ns;
use klipolo_platform::qos::{self, Qos};

use super::core::SessionCore;
use super::effect::Effect;
use super::pipes::Pipes;
use super::threads::{Input, spawn};
use crate::backend::Backend;
use crate::io::Io;
use crate::journal::{self, Entry};
use crate::pipe::{PipeEvent, VideoStats};
use crate::protocol::Event;
use crate::readback;
use crate::session::{SessionConfig, SessionDir};
use crate::shell::{ShellInput, ShellOutput};
use crate::writer::{DurableWriter, WriterError};

/// The core thread's state.
pub(super) struct Core {
    pub(super) core: SessionCore,
    pub(super) dir: SessionDir,
    pub(super) cfg: SessionConfig,
    pub(super) backend: Arc<dyn Backend>,
    pub(super) io: Arc<dyn Io>,
    pub(super) writer: Option<DurableWriter>,
    pub(super) pipes: Pipes,
    pub(super) events: SyncSender<Event>,
    pub(super) shell: SyncSender<ShellOutput>,
    pub(super) core_tx: SyncSender<Input>,
    pub(super) stats: BTreeMap<String, VideoStats>,
    /// The last heartbeat's number, so Stop's watchdog goes on counting.
    pub(super) beats: u64,
    /// The Stop line given to the writer, and whether it took it: an unfinished Stop writes it itself (`finalize`).
    pub(super) stop_line: Option<(Vec<u8>, bool)>,
}

impl Core {
    pub(super) fn run(mut self, rx: Receiver<Input>, opening: Vec<Effect>) -> i32 {
        if let Some(code) = self.apply(opening) {
            return code;
        }
        for input in rx {
            let now = host_ns();
            // The system's alerts first, before an input that may open the gate: at each tick and poll, and whenever the
            // countdown has run out, so one that came in the last moment before the gate still holds it.
            if matches!(input, Input::Tick { .. } | Input::Poll) || self.core.gate_due(now) {
                let fx = self.system_alert(now);
                if let Some(code) = self.apply(fx) {
                    return code;
                }
            }
            let fx = match input {
                Input::Command(cmd) => self.core.command(cmd, now),
                Input::ParentGone => self.core.shell(ShellInput::ParentGone, now),
                Input::Shell(input) => self.core.shell(input, now),
                Input::Tick { free } => {
                    let snap = self
                        .writer
                        .as_ref()
                        .map(DurableWriter::snapshot)
                        .unwrap_or_default();
                    self.pipes.stats_into(&mut self.stats);
                    let awake = self.backend.display_awake();
                    self.core.tick(now, &snap, free, &self.stats, awake)
                }
                Input::ReadBack(problems) => self.core.read_back(problems, now),
                Input::Started {
                    src,
                    start_no,
                    result,
                } => match (self.mine(src, start_no), result) {
                    (Some(id), Ok(started)) => self.core.started(&id, started, now),
                    (Some(id), Err(e)) => {
                        // A restart that failed on a sleeping display must not spend another restart.
                        self.core.display(self.backend.display_awake());
                        self.core.failed_to_start(&id, e, now)
                    }
                    (None, _) => vec![],
                },
                Input::Packet {
                    src,
                    start_no,
                    packet,
                } => self.with(src, start_no, |c, id| c.packet(id, packet, now)),
                Input::Pcm {
                    src,
                    start_no,
                    block,
                } => self.with(src, start_no, |c, id| c.pcm(id, block, now)),
                Input::Cursor(ev) => self.core.cursor(ev, now),
                Input::CursorInFrames { ok, host_ns } => {
                    self.core.cursor_in_frames(ok, host_ns, now)
                }
                Input::Poll => self.core.poll(now),
                Input::Event {
                    src,
                    start_no,
                    event,
                } => {
                    if matches!(event, PipeEvent::StoppedBySystem | PipeEvent::Stalled) {
                        self.core.display(self.backend.display_awake());
                    }
                    self.with(src, start_no, |c, id| c.pipe_event(id, event, now))
                }
            };
            if let Some(code) = self.apply(fx) {
                return code;
            }
        }
        1
    }

    /// From the start until the gate, whether a system alert is on the screen, as the backend reads it now: read before
    /// each tick and poll (ten times a second while a separate cursor is watched, as on a Mac's screen take) and before
    /// the input that would open the gate (`SessionCore::gate_due`), never after.
    fn system_alert(&mut self, now: u64) -> Vec<Effect> {
        if !self.core.watches_system_alerts() {
            return vec![];
        }
        let up = self.backend.system_alert();
        self.core.system_alert(up, now)
    }

    /// The source's id when the input comes from its pipe's current start (an older start's are dropped).
    fn mine(&self, src: usize, start_no: u64) -> Option<String> {
        (self.pipes.current(src) == start_no).then(|| self.cfg.sources[src].id.clone())
    }

    fn with(
        &mut self,
        src: usize,
        start_no: u64,
        f: impl FnOnce(&mut SessionCore, &str) -> Vec<Effect>,
    ) -> Vec<Effect> {
        if self.pipes.current(src) != start_no {
            return vec![];
        }
        let id = &self.cfg.sources[src].id;
        f(&mut self.core, id)
    }

    /// Carries out the effects in order (some give more: a writer's refusal, the end of Stop). Some(code): exit.
    fn apply(&mut self, fx: Vec<Effect>) -> Option<i32> {
        let mut queue = VecDeque::from(fx);
        while let Some(e) = queue.pop_front() {
            let now = host_ns();
            let refused = match e {
                Effect::StartPipes(ids) => {
                    for (i, e) in self.pipes.start(&ids) {
                        let id = self.cfg.sources[i].id.clone();
                        queue.extend(self.core.failed_to_start(&id, e, now));
                    }
                    None
                }
                Effect::StopPipes => {
                    self.pipes.stop_all();
                    None
                }
                Effect::Pipe { source, cmd } => {
                    self.pipes.cmd(&source, cmd);
                    None
                }
                Effect::SwapMic { source, device } => {
                    self.pipes.swap(&source, device);
                    None
                }
                Effect::Open { key, head } => self.write(&key, |w| w.open(&key, head)),
                Effect::OpenLines { key } => self.write(&key, |w| w.open_lines(&key)),
                Effect::Append {
                    key,
                    bytes,
                    media_us,
                } => self.write(&key, |w| w.append(&key, bytes, media_us)),
                Effect::Close { key } => {
                    let _ = self.write(&key, |w| w.close(&key));
                    None
                }
                Effect::WriteAtomic { key, bytes } => {
                    self.write(&key, |w| w.write_atomic(&key, bytes))
                }
                Effect::Journal(entry) => {
                    let refused = self.write("journal.jsonl", |w| w.journal(&entry));
                    if matches!(entry, Entry::StopRequested { .. }) {
                        self.stop_line = Some((journal::line(&entry), refused.is_none()));
                    }
                    refused
                }
                Effect::Tick => self.write("journal.jsonl", |w| w.tick()),
                Effect::Emit(ev) => {
                    if let Event::Heartbeat { n, .. } = ev {
                        self.beats = n;
                    }
                    self.emit(ev);
                    None
                }
                Effect::Shell(out) => {
                    let _ = self.shell.try_send(out);
                    None
                }
                Effect::ReadBack => {
                    self.read_back();
                    None
                }
                Effect::Finalize => {
                    queue.extend(self.finalize());
                    None
                }
                Effect::Exit(code) => {
                    if let Some(w) = self.writer.take() {
                        let _ = w.finish();
                    }
                    self.pipes.stop_all();
                    return Some(code);
                }
            };
            if let Some((key, error)) = refused {
                let failed = self.writer.as_ref().and_then(|w| w.snapshot().failed);
                queue.extend(self.core.writer_refused(&key, error, failed, now));
            }
        }
        None
    }

    /// One writer call; its refusal, with the key, for the core. After Stop's `finish` there is no writer: dropped.
    fn write(
        &self,
        key: &str,
        call: impl FnOnce(&DurableWriter) -> Result<(), WriterError>,
    ) -> Option<(String, WriterError)> {
        let w = self.writer.as_ref()?;
        match call(w) {
            Ok(()) => None,
            Err(WriterError::Full) if key.ends_with(".jsonl") => {
                tracing::warn!(key, "core: the writer's queue was full; a line was dropped");
                None
            }
            Err(e) => Some((key.to_owned(), e)),
        }
    }

    pub(super) fn emit(&self, ev: Event) {
        if let Err(TrySendError::Full(ev)) = self.events.try_send(ev) {
            tracing::warn!(?ev, "core: the app isn't reading; an event was dropped");
        }
    }

    fn read_back(&self) {
        let (dir, backend, tx) = (self.dir.clone(), self.backend.clone(), self.core_tx.clone());
        spawn("klipolo-readback", move || {
            qos::set_current(Qos::Utility);
            let problems = readback::check(&dir, &*backend);
            let _ = tx.send(Input::ReadBack(problems));
        });
    }
}
