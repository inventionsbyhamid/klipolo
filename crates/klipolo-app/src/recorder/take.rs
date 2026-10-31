//! The recorder: at most one take, whose worker it launches and supervises (`supervise`, `events`). Nothing here
//! captures: arming launches a worker that waits, and only the person's Record press (`send(Start)`) starts it.

use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use klipolo_capture::session::{self, SessionConfig, SessionDir};
use klipolo_model::coded::CodedError;

use super::env::Env;
use super::supervise::Supervisor;
use super::types::ArmedTake;
use super::words;

/// A worker's ends the recorder keeps. `generation` tells its lines from an earlier worker's.
pub(super) struct Worker {
    pub(super) generation: u64,
    pub(super) stdin: Box<dyn Write + Send>,
    pub(super) kill: Box<dyn Fn(bool) + Send + Sync>,
}

pub(super) struct Take {
    pub(super) dir: SessionDir,
    pub(super) cfg: SessionConfig,
    /// The file of the project open when it was armed (its `cfg.target`), where it lands.
    pub(super) project_file: Option<PathBuf>,
    /// None between a worker's end and its resume, and while the take is finished from its files.
    pub(super) worker: Option<Worker>,
    pub(super) sup: Supervisor,
    /// The app stopped it (its screen's encoder gave up): said once.
    pub(super) stopped_by_app: bool,
    /// Its camera's encoder gave up and the screen records on: said once.
    pub(super) camera_dropped: bool,
}

#[derive(Default)]
pub(super) struct Slot {
    pub(super) take: Option<Take>,
    /// Workers launched so far: the next one's generation.
    pub(super) generations: u64,
    pub(super) ticking: bool,
    /// Reopen Klipolo is under way: no take arms, as the restart would cut it off.
    pub(super) reopening: bool,
}

pub struct Recorder {
    pub(super) env: Env,
    pub(super) slot: Mutex<Slot>,
    /// Lines and exits handled so far (tests wait for their fake worker's).
    pub(super) handled: AtomicU64,
    /// A take is the slot's (`active`), mirrored so it is read without the slot's lock, which covers file and process
    /// work.
    pub(super) active: AtomicBool,
}

impl Recorder {
    pub fn new(env: Env) -> Arc<Self> {
        Arc::new(Self {
            env,
            slot: Mutex::default(),
            handled: AtomicU64::new(0),
            active: AtomicBool::new(false),
        })
    }

    /// `active` follows the slot's take; called wherever the take is placed or let go, under the lock.
    pub(super) fn sync_active(&self, slot: &Slot) {
        self.active.store(slot.take.is_some(), Ordering::SeqCst);
    }

    /// The slot, locked. A command that panicked must not leave the recorder locked for the rest of the session.
    pub(super) fn lock(&self) -> MutexGuard<'_, Slot> {
        self.slot.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(super) fn now_ms(&self) -> u64 {
        self.env.clock.now_ns() / 1_000_000
    }

    /// `capture.busy`: "A recording is already running. Stop it from {place}."
    pub(super) fn busy_error(&self) -> CodedError {
        words::busy((self.env.place)())
    }

    /// `capture.busy` while a take is the recorder's, armed through saving, or Reopen Klipolo is under way.
    pub(super) fn refuse_while_busy(&self) -> Result<(), CodedError> {
        self.refuse_while_busy_in(&self.lock())
    }

    fn refuse_while_busy_in(&self, slot: &Slot) -> Result<(), CodedError> {
        if slot.take.is_some() || slot.reopening {
            return Err(self.busy_error());
        }
        Ok(())
    }

    /// Reopen Klipolo begins: refused while a take is the recorder's; from now on no take arms. False when it is under
    /// way already (a second press): nothing more to do.
    pub(super) fn begin_reopen(&self) -> Result<bool, CodedError> {
        let mut slot = self.lock();
        if slot.reopening {
            return Ok(false);
        }
        self.refuse_while_busy_in(&slot)?;
        slot.reopening = true;
        Ok(true)
    }

    /// Reopen Klipolo didn't go ahead (the project couldn't be saved): takes arm again.
    pub(super) fn end_reopen(&self) {
        self.lock().reopening = false;
    }

    /// Arms take `cfg.id`: its folder, its config and its worker, which waits for the person's Record. `notes`: what
    /// the person is told about it before Record; `project_file`: the file of the project it lands in (`cfg.target`).
    /// Refused while a take is the recorder's or Klipolo reopens (`capture.busy`), checked again under the lock the
    /// take is placed under.
    pub fn arm(
        self: &Arc<Self>,
        cfg: SessionConfig,
        notes: Vec<CodedError>,
        project_file: Option<PathBuf>,
    ) -> Result<ArmedTake, CodedError> {
        let mut slot = self.lock();
        self.refuse_while_busy_in(&slot)?;
        let armed = ArmedTake {
            id: cfg.id.clone(),
            countdown_s: cfg.countdown_s,
            notes: (!notes.is_empty()).then_some(notes),
        };
        let reader = self.place(&mut slot, cfg, project_file)?;
        self.sync_active(&slot);
        self.start_ticker(&mut slot);
        drop(slot);
        reader.start(self);
        tracing::info!(take = armed.id, "recorder: armed");
        Ok(armed)
    }

    /// Take `cfg.id`'s folder, config and worker, waiting for the person's Record: the slot's take from now. Its
    /// events are read once the caller starts the returned reader.
    fn place(
        &self,
        slot: &mut Slot,
        cfg: SessionConfig,
        project_file: Option<PathBuf>,
    ) -> Result<Reader, CodedError> {
        let root = (self.env.root.clone()).ok_or_else(|| {
            words::set_up_failed(&io::Error::new(
                io::ErrorKind::NotFound,
                "no recordings folder",
            ))
        })?;
        let dir = SessionDir::create(&root, &cfg.id).map_err(|e| words::set_up_failed(&e))?;
        let args = vec![
            "--record-worker".to_string(),
            dir.root.to_string_lossy().into_owned(),
        ];
        // Only its config was written this instant: nothing recorded is lost when it goes again.
        let undo = |error: CodedError| {
            let _ = std::fs::remove_dir_all(&dir.root);
            error
        };
        session::write_config(&dir, &cfg).map_err(|e| undo(words::set_up_failed(&e)))?;
        let (worker, stdout, wait) =
            (self.spawn(slot, &args)).map_err(|e| undo(words::worker_didnt_start(&e)))?;
        let generation = worker.generation;
        slot.take = Some(Take {
            dir,
            cfg,
            project_file,
            worker: Some(worker),
            sup: Supervisor::new(self.now_ms()),
            stopped_by_app: false,
            camera_dropped: false,
        });
        Ok(Reader {
            generation,
            stdout,
            wait,
        })
    }

    /// The take's id while there is one, armed through saving.
    pub fn take_id(&self) -> Option<String> {
        self.lock().take.as_ref().map(|t| t.cfg.id.clone())
    }

    /// A worker launched with `args`, numbered.
    #[allow(clippy::type_complexity)]
    pub(super) fn spawn(
        &self,
        slot: &mut Slot,
        args: &[String],
    ) -> io::Result<(
        Worker,
        Box<dyn BufRead + Send>,
        Box<dyn FnOnce() -> Option<i32> + Send>,
    )> {
        let io = self.env.launch.spawn(args)?;
        slot.generations += 1;
        tracing::info!(
            pid = io.pid,
            generation = slot.generations,
            "recorder: a worker was launched"
        );
        let worker = Worker {
            generation: slot.generations,
            stdin: io.stdin,
            kill: io.kill,
        };
        Ok((worker, io.stdout, io.wait))
    }
}

/// A worker's stdout and exit, read on a thread of its own once `start`ed.
pub(super) struct Reader {
    generation: u64,
    stdout: Box<dyn BufRead + Send>,
    wait: Box<dyn FnOnce() -> Option<i32> + Send>,
}

impl Reader {
    pub(super) fn start(self, recorder: &Arc<Recorder>) {
        recorder.read_in_background(self.generation, self.stdout, self.wait);
    }
}
