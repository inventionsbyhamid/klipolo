//! The one task that owns the schedule's effects: it starts, pauses and stops the worker, follows its events, decides
//! the project's files in the background and hands the engine its map. Commands and events reach it as messages, so
//! nothing else ever changes the schedule.

use super::decide::{CopyState, copy_state, decide, facts_now, touch};
use super::tidy::tidy_in_background;
use super::*;

/// The copy being made.
struct Run {
    id: u64,
    path: Arc<str>,
    stem: String,
    ctl: mpsc::UnboundedSender<Ctl>,
    /// A pause was asked for, and why.
    asked: Option<Pause>,
    /// Stopped by the person: what the worker still says is ignored.
    stopping: bool,
    /// Its row goes once it has paused (its file left the project, or automatic copies were turned off and not back on
    /// before it paused).
    forget: bool,
    /// It has said how it ended; the worker is exiting.
    ended: bool,
}

enum Ctl {
    Pause,
    Stop,
}

pub(super) struct Driver {
    inner: Arc<Inner>,
    preview: Option<Arc<dyn Preview>>,
    thumbs: Option<Arc<ThumbStore>>,
    class: GpuClass,
    project: Option<ProjectView>,
    /// The listing in force: decisions of older ones are dropped.
    generation: u64,
    run: Option<Run>,
    runs: u64,
    /// The map the engine has.
    pushed: Option<MediaMap>,
    map_dirty: bool,
    facts: FactsCache,
    emitted: Option<Instant>,
    emit_due: Option<Instant>,
    /// No copy starts before this (`SETTLE` after playback stops).
    quiet_until: Option<Instant>,
    /// A driving step is due then (the settle's end).
    drive_at: Option<Instant>,
    /// Files of the open project that weren't there when decided (an unplugged drive), and when they were last looked
    /// at again.
    absent: HashSet<Arc<str>>,
    absent_checked: Option<Instant>,
    /// A look at those files is under way (its stats may wait on a volume that hangs): never a second one.
    looking: Arc<std::sync::atomic::AtomicBool>,
    /// Finished copies of files that couldn't be looked at (unmapped, kept): when the file is back, the copy is mapped
    /// again if it is still of it, else deleted.
    kept: HashMap<Arc<str>, PathBuf>,
}

async fn sleep_until(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

impl Driver {
    pub(super) fn new(inner: Arc<Inner>) -> Self {
        Self {
            inner,
            preview: None,
            thumbs: None,
            class: GpuClass::Integrated,
            project: None,
            generation: 0,
            run: None,
            runs: 0,
            pushed: None,
            map_dirty: false,
            facts: Arc::default(),
            emitted: None,
            emit_due: None,
            quiet_until: None,
            drive_at: None,
            absent: HashSet::new(),
            absent_checked: None,
            looking: Arc::default(),
            kept: HashMap::new(),
        }
    }

    fn state(&self) -> MutexGuard<'_, ScheduleState> {
        lock(&self.inner.state)
    }

    pub(super) async fn run(mut self, mut rx: mpsc::UnboundedReceiver<Msg>) {
        loop {
            let (emit_due, drive_at, look_at) = (self.emit_due, self.drive_at, self.look_due());
            tokio::select! {
                msg = rx.recv() => match msg {
                    Some(msg) => self.handle(msg).await,
                    // The app is ending: the worker sees its stdin close and stops after its current frame.
                    None => return,
                },
                () = sleep_until(emit_due) => {}
                () = sleep_until(drive_at) => self.drive_at = None,
                () = sleep_until(look_at) => self.look_again(),
            }
            self.drive().await;
            self.flush_emit();
        }
    }

    async fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Project(view) => self.on_project(view),
            Msg::Revision => self.look_again(),
            Msg::Decided {
                generation,
                path,
                decision,
            } => {
                if generation == self.generation {
                    self.on_decided(path, decision);
                }
            }
            Msg::Attach(preview, thumbs) => self.on_attach(preview, thumbs),
            Msg::Playing(playing) => {
                let was = std::mem::replace(&mut self.state().playing, playing);
                if was && !playing {
                    let until = Instant::now() + SETTLE;
                    self.quiet_until = Some(until);
                    self.drive_at = Some(until);
                }
            }
            Msg::Exporting(exporting) => {
                self.state().exporting = exporting;
                if let Some(thumbs) = &self.thumbs {
                    if exporting {
                        thumbs.pause(Hold::Export);
                    } else {
                        thumbs.resume(Hold::Export);
                    }
                }
            }
            // The take's own hold keeps the engine's scans waiting (`Hold::Recording`); copies only wait here.
            Msg::Recording(recording) => self.state().recording = recording,
            Msg::Event {
                run,
                event: ProxyEvent::Done { path: reported },
            } => {
                // The copy is where its key puts it in the cache (never a path taken from the worker's manifest), and
                // is checked off the schedule's lock (a stat of the source, which may be on a slow volume).
                let checked = match self.run.as_ref().filter(|r| r.id == run) {
                    Some(r) => {
                        let copy = proxy_path(&self.inner.root, &r.stem);
                        if copy != reported {
                            tracing::warn!("editing copies: a worker delivered its copy elsewhere");
                        }
                        let (source, at) = (r.path.clone(), copy.clone());
                        let state =
                            tokio::task::spawn_blocking(move || copy_state(&source, &at, None))
                                .await
                                .unwrap_or(CopyState::Unreachable);
                        Some((copy, state))
                    }
                    None => None,
                };
                if let Some((copy, state)) = checked {
                    self.on_done(run, copy, state);
                }
            }
            Msg::Event { run, event } => self.on_event(run, event),
            Msg::Recheck => {
                self.recheck();
                // Files that weren't there may be back (a drive plugged in while Klipolo was behind).
                self.absent_checked = None;
                self.look_again();
            }
            Msg::Stale(paths) => self.on_stale(paths),
            Msg::Unreachable(paths) => self.on_unreachable(paths),
            Msg::Back(paths) => {
                for path in &paths {
                    self.absent.remove(path);
                    self.kept.remove(path);
                }
                self.decide_in_background(paths);
            }
            Msg::Exited { run } => self.on_exited(run),
            Msg::Unusable(path) => self.on_unusable(&path),
            Msg::Drive => {}
            Msg::Wake { path, stem } => {
                let mut s = self.state();
                if let Some(i) = s.index_of(&path).filter(|&i| s.jobs[i].stem == stem) {
                    s.wake(i);
                }
                drop(s);
                self.changed();
            }
            Msg::Command(command, reply) => {
                let _ = reply.send(self.on_command(command));
                self.changed();
            }
        }
    }

    /// The list changed in a way the UI shows at once.
    fn changed(&mut self) {
        self.emit_due = Some(Instant::now());
    }

    /// Only progress changed: told at most every `PROGRESS_EVERY`.
    fn progressed(&mut self) {
        if self.emit_due.is_none() {
            let at = self
                .emitted
                .map_or_else(Instant::now, |last| last + PROGRESS_EVERY);
            self.emit_due = Some(at.max(Instant::now()));
        }
    }

    fn flush_emit(&mut self) {
        if self.emit_due.is_some_and(|due| due <= Instant::now()) {
            self.emit_due = None;
            self.emitted = Some(Instant::now());
            let list = self.state().list();
            (self.inner.emit)(list);
        }
    }

    fn policy(&self) -> ProxyPolicy {
        (self.inner.policy)(self.class)
    }

    /// The project opened, switched, closed, or its files changed: rows of files it no longer has go (a copy being
    /// made of one pauses, keeping its finished segments), and every file is decided again in the background.
    fn on_project(&mut self, view: Option<ProjectView>) {
        self.generation += 1;
        // Listing threads of older listings stop at their next file.
        self.inner
            .listing
            .store(self.generation, std::sync::atomic::Ordering::SeqCst);
        self.project = view.clone();
        self.absent.clear();
        let keep: HashSet<Arc<str>> = view
            .as_ref()
            .map(|v| v.paths.iter().cloned().collect())
            .unwrap_or_default();
        // Copies kept for files that are away stay known while the project still has those files.
        self.kept.retain(|path, _| keep.contains(path));
        if self.run.as_ref().is_some_and(|r| !keep.contains(&r.path)) {
            self.pause_run(Pause::User, true);
        }
        let running = self.run.as_ref().map(|r| r.path.clone());
        self.state()
            .retain(|j| keep.contains(&j.path) || Some(&j.path) == running.as_ref());
        self.map_dirty = true;
        self.changed();
        if let Some(view) = view {
            self.decide_in_background(view.paths.to_vec());
        }
    }

    /// Decides `paths` on a utility thread, one at a time (each probe reads a little of the file), sending each answer
    /// as it comes so the first copy can start before the last file is probed. A newer listing makes it stop. While
    /// automatic copies are off it only looks for finished copies (a stat for each file without one, no probe).
    fn decide_in_background(&self, paths: Vec<Arc<str>>) {
        let (generation, tx, listing) = (
            self.generation,
            self.inner.tx.clone(),
            self.inner.listing.clone(),
        );
        let finished_only = !self.state().auto;
        let (root, probe, cache, policy) = (
            self.inner.root.clone(),
            self.inner.probe.clone(),
            self.facts.clone(),
            self.policy(),
        );
        let spawned = std::thread::Builder::new()
            .name("klipolo-copies".into())
            .spawn(move || {
                klipolo_platform::qos::set_current(klipolo_platform::qos::Qos::Utility);
                for path in paths {
                    if listing.load(std::sync::atomic::Ordering::SeqCst) != generation {
                        return;
                    }
                    let decision = decide(&root, &path, &policy, &*probe, &cache, finished_only);
                    if tx
                        .send(Msg::Decided {
                            generation,
                            path,
                            decision,
                        })
                        .is_err()
                    {
                        return;
                    }
                }
            });
        if let Err(error) = spawned {
            tracing::warn!(%error, "editing copies: the project's files couldn't be looked at");
        }
    }

    /// When files of the open project that aren't there are looked at again by themselves: every
    /// `ABSENT_EVERY` while any is absent, without waiting for an edit or for Klipolo to come to the front (which look at
    /// once). None, so no timer, while none is absent or no project is open.
    fn look_due(&mut self) -> Option<Instant> {
        let absent = self.project.is_some()
            && (!self.absent.is_empty() || self.state().jobs.iter().any(is_missing));
        if !absent {
            return None;
        }
        let since = *self.absent_checked.get_or_insert_with(Instant::now);
        Some(since + ABSENT_EVERY)
    }

    /// Files that weren't there (no row, or failed as missing) and are there now are decided again: at most every
    /// `ABSENT_EVERY`, a stat each, on a thread of its own, one look at a time.
    fn look_again(&mut self) {
        use std::sync::atomic::Ordering;
        let due = self
            .absent_checked
            .is_none_or(|at| at.elapsed() >= ABSENT_EVERY);
        if !due {
            return;
        }
        self.absent_checked = Some(Instant::now());
        // The last look still waits on a volume that hangs: this round is skipped (it is tried again next time).
        if self.looking.load(Ordering::SeqCst) {
            return;
        }
        let missing: Vec<Arc<str>> = self
            .state()
            .jobs
            .iter()
            .filter(|j| is_missing(j))
            .map(|j| j.path.clone())
            .collect();
        if missing.is_empty() && self.absent.is_empty() {
            return;
        }
        let candidates: Vec<(Arc<str>, Option<PathBuf>)> = missing
            .into_iter()
            .chain(self.absent.iter().cloned())
            .map(|p| {
                let kept = self.kept.get(&p).cloned();
                (p, kept)
            })
            .collect();
        // The stats on a thread of their own: a volume that hangs never holds the driver.
        let (tx, looking) = (self.inner.tx.clone(), self.looking.clone());
        looking.store(true, Ordering::SeqCst);
        let spawned = std::thread::Builder::new()
            .name("klipolo-copies-look".into())
            .spawn(move || {
                // Done looking however this ends.
                struct Done(Arc<std::sync::atomic::AtomicBool>);
                impl Drop for Done {
                    fn drop(&mut self) {
                        self.0.store(false, Ordering::SeqCst);
                    }
                }
                let _done = Done(looking);
                let mut back = Vec::new();
                for (path, kept) in candidates {
                    if !Path::new(&*path).is_file() {
                        continue;
                    }
                    // A copy kept while the file was away is of another version if the file came back changed: the
                    // cache's own, deleted (the listing that follows makes a new one). Unchanged, it is found and
                    // mapped again by that listing, never made again.
                    if let Some(copy) = kept
                        && copy_state(&path, &copy, None) == CopyState::Changed
                    {
                        remove_copy(&copy);
                    }
                    back.push(path);
                }
                if !back.is_empty() {
                    let _ = tx.send(Msg::Back(back));
                }
            });
        if let Err(error) = spawned {
            self.looking.store(false, Ordering::SeqCst);
            tracing::warn!(%error, "editing copies: missing files couldn't be looked at");
        }
    }

    /// Every mapped copy is checked against its source on a thread of its own: those of a file that is there but changed
    /// come back as `Stale` (deleted, made again), those whose file can't be looked at now as `Unreachable` (unmapped,
    /// kept).
    fn recheck(&mut self) {
        let done: Vec<(Arc<str>, Arc<str>)> = self
            .state()
            .jobs
            .iter()
            .filter_map(|j| match &j.state {
                JobState::Done(copy) => Some((j.path.clone(), copy.clone())),
                _ => None,
            })
            .collect();
        if done.is_empty() {
            return;
        }
        let tx = self.inner.tx.clone();
        let spawned = std::thread::Builder::new()
            .name("klipolo-copies-check".into())
            .spawn(move || {
                let (mut stale, mut away) = (Vec::new(), Vec::new());
                for (source, copy) in done {
                    match copy_state(&source, Path::new(&*copy), None) {
                        CopyState::Current => {}
                        CopyState::Changed => stale.push(source),
                        CopyState::Unreachable => away.push(source),
                    }
                }
                if !stale.is_empty() {
                    let _ = tx.send(Msg::Stale(stale));
                }
                if !away.is_empty() {
                    let _ = tx.send(Msg::Unreachable(away));
                }
            });
        if let Err(error) = spawned {
            tracing::warn!(%error, "editing copies: copies couldn't be checked");
        }
    }

    /// These sources were overwritten since their copies were made: the copies go from the preview at once, are deleted
    /// (the cache's own), and the files are decided again (a new version, a new copy).
    fn on_stale(&mut self, paths: Vec<Arc<str>>) {
        let mut s = lock(&self.inner.state);
        let mut again = Vec::new();
        for path in paths {
            let Some(i) = s.index_of(&path) else { continue };
            if let JobState::Done(copy) = &s.jobs[i].state {
                let copy = PathBuf::from(&**copy);
                if copy.starts_with(&self.inner.root) {
                    remove_copy(&copy);
                }
                s.requeue(i);
                self.map_dirty = true;
                again.push(path);
            }
        }
        drop(s);
        if !again.is_empty() {
            tracing::info!(
                count = again.len(),
                "editing copies: sources changed since their copies were made"
            );
            self.decide_in_background(again);
            self.changed();
        }
    }

    /// These sources can't be looked at now (an ejected drive, a share that dropped): their copies leave the preview but
    /// stay on disk, and the files are looked at again (`look_again`) until they are back; one back unchanged is mapped
    /// again at once (its record still matches), one back changed gets a new copy.
    fn on_unreachable(&mut self, paths: Vec<Arc<str>>) {
        let running = self.run.as_ref().map(|r| r.path.clone());
        let mut s = lock(&self.inner.state);
        for path in paths {
            if Some(&path) == running.as_ref() {
                continue;
            }
            let Some(i) = s.index_of(&path) else { continue };
            if let JobState::Done(copy) = &s.jobs[i].state {
                self.kept.insert(path.clone(), PathBuf::from(&**copy));
                s.retain(|j| j.path != path);
                self.map_dirty = true;
                self.absent.insert(path);
            }
        }
        drop(s);
        self.changed();
    }

    /// One file's decision. While automatic copies are off it adds no row and keeps none but a finished copy: a
    /// file to copy, or one that can't be read, isn't listed, since nothing would make its copy.
    fn on_decided(&mut self, path: Arc<str>, decision: Decision) {
        let running = self
            .run
            .as_ref()
            .is_some_and(|r| r.path == path && !r.ended);
        let mut s = lock(&self.inner.state);
        let at = s.index_of(&path);
        let auto = s.auto;
        let row = |s: &mut ScheduleState| match at {
            Some(i) => i,
            None => {
                s.jobs.push(Job::new(&path, JobState::Waiting));
                s.jobs.len() - 1
            }
        };
        match decision {
            // A copy being made keeps going (its worker checks its source itself), whatever a new policy says.
            _ if running => return,
            // A row that goes may have been a finished copy the preview reads: the map is checked again.
            Decision::NoCopy => self.map_dirty |= unlist(&mut s, at, &path),
            // Nothing would make the copy while automatic copies are off.
            Decision::Unreadable(_) | Decision::Queue { .. } if !auto => {
                self.map_dirty |= unlist(&mut s, at, &path);
            }
            // Looked at again as the project is edited (`look_again`). A row saying it is missing keeps saying so.
            Decision::Unavailable => {
                self.absent.insert(path.clone());
                if at.is_some_and(|i| {
                    !matches!(
                        s.jobs[i].state,
                        JobState::Failed {
                            failure: Failure::Missing,
                            ..
                        }
                    )
                }) {
                    self.map_dirty |= unlist(&mut s, at, &path);
                }
            }
            Decision::Unreadable(message) => {
                let i = row(&mut s);
                if !matches!(s.jobs[i].state, JobState::Stopped) {
                    // A finished copy whose source can't be read now (overwritten with something that isn't video)
                    // leaves the preview with its row.
                    self.map_dirty |= matches!(s.jobs[i].state, JobState::Done(_));
                    s.failed(i, Failure::Unreadable, &message);
                }
            }
            Decision::Done {
                copy,
                fps,
                stem,
                facts,
                why,
            } => {
                let i = row(&mut s);
                let job = &mut s.jobs[i];
                (job.fps, job.stem, job.facts) = (fps, stem, Some(facts));
                job.why = Some(why);
                touch(Path::new(&*copy));
                s.done(i, copy);
                self.map_dirty = true;
            }
            Decision::Queue {
                stem,
                fps,
                progress,
                facts,
                why,
            } => {
                let i = row(&mut s);
                let job = &mut s.jobs[i];
                // What the person or the worker decided stays for the same file (a stop, a failure, a pause, a copy
                // finished since this was decided); a done copy that is gone, or a changed file, is made (again).
                let keeps = job.stem == stem
                    && match &job.state {
                        JobState::Stopped | JobState::Paused(_) => true,
                        JobState::Failed { failure, .. } => *failure != Failure::Missing,
                        JobState::Done(copy) => Path::new(&**copy).is_file(),
                        _ => false,
                    };
                (job.fps, job.facts, job.why) = (fps, Some(facts), Some(why));
                if !keeps {
                    job.stem = stem;
                    job.state = JobState::Waiting;
                    job.progress = progress;
                    self.map_dirty = true;
                }
            }
        }
        drop(s);
        self.changed();
    }

    fn on_attach(&mut self, preview: Arc<dyn Preview>, thumbs: Option<Arc<ThumbStore>>) {
        let class = preview.gpu_class();
        // A new engine isn't playing (one that died mid-play never said it stopped).
        self.state().playing = false;
        self.preview = Some(preview);
        self.thumbs = thumbs;
        if let Some(thumbs) = &self.thumbs {
            if self.state().exporting {
                thumbs.pause(Hold::Export);
            }
            if self.run.as_ref().is_some_and(|r| !r.ended) {
                thumbs.pause(Hold::Copy);
            }
        }
        // A new engine has no map yet.
        self.pushed = None;
        self.map_dirty = true;
        if class != self.class {
            self.class = class;
            // Files not started yet are decided again under this class's policy; a running copy keeps its size.
            if let Some(project) = self.project.clone() {
                self.on_project(Some(project));
            }
        }
    }

    /// Does what the schedule says until nothing more is to be done now, then hands the engine its map if it changed.
    async fn drive(&mut self) {
        loop {
            let action = {
                let scanning = self.thumbs.as_ref().is_some_and(|t| t.scanning());
                let mut s = self.state();
                s.scanning = scanning;
                s.next()
            };
            match action {
                Action::Nothing => break,
                Action::Pause(_, why) => {
                    self.pause_run(why, false);
                    break;
                }
                Action::Start(i) => {
                    // Not before the engine has said its GPU class, not while the last worker exits, not while
                    // playback has only just stopped.
                    if self.preview.is_none() || self.run.is_some() {
                        break;
                    }
                    if self.quiet_until.is_some_and(|until| Instant::now() < until) {
                        break;
                    }
                    if self.start(i).await {
                        break;
                    }
                }
            }
        }
        self.push_map();
    }

    /// Starts job `i`'s worker (making its job folder now, or resuming the one there). False when it didn't start
    /// (it was already finished, or it can't be made): the next one may.
    async fn start(&mut self, i: usize) -> bool {
        let path = self.state().jobs[i].path.clone();
        if let Some(thumbs) = &self.thumbs {
            // A scan running now stops between packets: the copy runs alone.
            thumbs.pause(Hold::Copy);
        }
        let policy = self.policy();
        let (root, source) = (self.inner.root.clone(), path.clone());
        // A new job reads its source again as it starts (no facts handed in): it is planned from the file as it is now,
        // never from facts of a version overwritten since it was listed. An unfinished job resumes
        // under the key of the version it was planned from.
        let (launcher, probe, cache) = (
            self.inner.launcher.clone(),
            self.inner.probe.clone(),
            self.facts.clone(),
        );
        let started = tokio::task::spawn_blocking(move || {
            // The facts of the file as it is now: probed once per version (a resume after a pause reads nothing again).
            let opened = match facts_now(&source, &*probe, &cache) {
                Ok(facts) => ProxyJob::open_with(&root, &source, Some(&facts), &policy),
                Err(_) if !Path::new(&*source).is_file() => {
                    Opened::Refused(Refused::SourceUnavailable)
                }
                Err(ProbeError::Unreadable(_)) => Opened::Stale,
                Err(ProbeError::TimedOut) => Opened::Refused(Refused::SourceTimedOut),
                Err(ProbeError::Changing) => Opened::Refused(Refused::Changing),
            };
            // The worker starts here too: spawning a process never runs under the schedule's lock or on the driver.
            let runner = match &opened {
                Opened::Fresh(job) | Opened::Resumed(job) => {
                    Some(ProxyRunner::start(&job.dir, &*launcher))
                }
                _ => None,
            };
            // A copy finished meanwhile is held to the same rule as any other: mapped only with a matching record.
            let finished = match &opened {
                Opened::Done(copy) => Some(copy_state(&source, copy, None)),
                _ => None,
            };
            (opened, runner, finished)
        })
        .await;
        let (opened, runner, finished) = match started {
            Ok((opened, runner, finished)) => (Ok(opened), runner, finished),
            Err(e) => (Err(e), None, None),
        };
        let mut s = lock(&self.inner.state);
        // The list may have moved while the folder was looked at.
        let Some(i) = s.index_of(&path) else {
            drop(s);
            self.resume_scans();
            return false;
        };
        match opened {
            Ok(Opened::Fresh(job) | Opened::Resumed(job)) => {
                match runner.expect("started with its job") {
                    Ok(runner) => {
                        self.runs += 1;
                        let (ctl, ctl_rx) = mpsc::unbounded_channel();
                        let id = self.runs;
                        tauri::async_runtime::spawn(run_copy(
                            runner,
                            ctl_rx,
                            self.inner.tx.clone(),
                            id,
                        ));
                        self.run = Some(Run {
                            id,
                            path: path.clone(),
                            stem: job.stem.clone(),
                            ctl,
                            asked: None,
                            stopping: false,
                            forget: false,
                            ended: false,
                        });
                        let entry = &mut s.jobs[i];
                        (entry.stem, entry.fps) = (job.stem.clone(), job.rate.f64());
                        s.started(i);
                        s.progress(i, job.frames_done(), job.total_frames);
                        drop(s);
                        self.changed();
                        return true;
                    }
                    Err(message) => s.failed(i, Failure::Worker, &message),
                }
            }
            // Finished meanwhile: mapped when its record matches; another version's is deleted and the file decided
            // again; one whose source went meanwhile waits for it.
            Ok(Opened::Done(copy)) => match finished {
                Some(CopyState::Current) => {
                    touch(&copy);
                    s.done(i, copy.to_string_lossy().into());
                    self.map_dirty = true;
                }
                Some(CopyState::Changed) => {
                    remove_copy(&copy);
                    s.requeue(i);
                    drop(s);
                    self.redecide(&path);
                    self.resume_scans();
                    self.changed();
                    return false;
                }
                _ => {
                    self.kept.insert(path.clone(), copy);
                    s.retain(|j| j.path != path);
                    self.absent.insert(path.clone());
                }
            },
            Ok(Opened::Stale) => {
                s.failed(i, Failure::Unreadable, "the source can't be read as video")
            }
            Ok(Opened::Refused(Refused::DiskFull)) => s.paused(i, Pause::Disk),
            // Being written or replaced, or too slow to read now: looked at again as the project is edited.
            Ok(Opened::Refused(reason))
                if reason.retry() == klipolo_engine::media::proxy::job::Retry::Backoff =>
            {
                s.retain(|j| j.path != path);
                self.absent.insert(path.clone());
            }
            Ok(Opened::Refused(reason)) => {
                s.failed(i, Failure::refused(&reason), &reason.to_string())
            }
            Err(_) => s.failed(i, Failure::Other, "the job couldn't be looked at"),
        }
        drop(s);
        self.resume_scans();
        self.changed();
        false
    }

    /// Asks the running worker to stop after its current frame (once), keeping its finished segments; `forget`: its row
    /// goes once it has.
    fn pause_run(&mut self, why: Pause, forget: bool) {
        if let Some(run) = self.run.as_mut().filter(|r| !r.ended) {
            run.forget |= forget;
            if run.asked.is_none() {
                run.asked = Some(why);
                let _ = run.ctl.send(Ctl::Pause);
            }
        }
    }

    fn resume_scans(&self) {
        if let Some(thumbs) = &self.thumbs {
            thumbs.resume(Hold::Copy);
        }
    }

    fn on_event(&mut self, id: u64, event: ProxyEvent) {
        let Some(run) = self.run.as_mut().filter(|r| r.id == id && !r.stopping) else {
            return;
        };
        let (path, asked, forget) = (run.path.clone(), run.asked, run.forget);
        if !matches!(
            event,
            ProxyEvent::Progress { .. } | ProxyEvent::Thumbs { .. }
        ) {
            run.ended = true;
        }
        let mut s = lock(&self.inner.state);
        let Some(i) = s.index_of(&path) else {
            return;
        };
        match event {
            ProxyEvent::Progress { done, total } => {
                s.progress(i, done, total);
                drop(s);
                self.progressed();
                return;
            }
            ProxyEvent::Thumbs { .. } => {
                drop(s);
                // The filmstrip of the file being copied refreshes (the worker's `.f.jpg` chunks).
                if let Some(preview) = &self.preview {
                    preview.media_event(path.to_string(), MediaKind::Thumbs);
                }
                return;
            }
            ProxyEvent::Paused { reason } => {
                let why = match reason {
                    PauseReason::User => s.hold().or(asked).unwrap_or(Pause::User),
                    PauseReason::DiskFull => Pause::Disk,
                    PauseReason::Thermal => Pause::Thermal,
                    PauseReason::EncoderBusy => Pause::Encoder,
                };
                s.paused(i, why);
            }
            // Asked to pause, it was stopped after its grace (or died winding down): paused, as asked.
            ProxyEvent::Crashed { .. } if asked.is_some() => {
                let why = s.hold().or(asked).unwrap_or(Pause::User);
                s.paused(i, why);
            }
            // Crashed or killed with no word of its own: started again by itself, from its finished
            // segments, after `RESTART_WAITS`; then it fails and waits for Try again.
            ProxyEvent::Crashed { message } => {
                s.crashed(i, &message);
                tracing::warn!(
                    file = %schedule::file_name(&path),
                    %message,
                    restarts = s.jobs[i].restarts,
                    gave_up = matches!(s.jobs[i].state, JobState::Failed { .. }),
                    "an editing copy's worker stopped without a word"
                );
            }
            // Handled with its source check first (`on_done`).
            ProxyEvent::Done { .. } => return,
            ProxyEvent::Failed {
                message,
                missing,
                code,
            } => {
                tracing::warn!(file = %schedule::file_name(&path), %message, ?code, "an editing copy failed");
                if code == FailCode::Changed {
                    s.requeue(i);
                    drop(s);
                    self.redecide(&path);
                    self.resume_scans();
                    self.changed();
                    return;
                }
                let failure = if missing {
                    Failure::Missing
                } else {
                    Failure::of(code)
                };
                s.failed(i, failure, &message);
            }
        }
        self.wake_later(&s, i, forget);
        if forget {
            // Its file left the project (or copies were turned off) while it ended: its row goes; what it made stays on
            // disk for later.
            s.retain(|j| j.path != path);
            self.map_dirty = true;
        }
        drop(s);
        self.resume_scans();
        self.changed();
    }

    /// The worker delivered `copy` (its place in the cache); `state`: how it stands against the source now (checked off
    /// the lock). A copy of an older version is never mapped: it is deleted and the file decided again (a new key, a
    /// new job). One whose source can't be looked at now (the drive went during delivery) is kept unmapped until the
    /// file is back.
    fn on_done(&mut self, id: u64, copy: PathBuf, state: CopyState) {
        let Some(run) = self.run.as_mut().filter(|r| r.id == id && !r.stopping) else {
            return;
        };
        run.ended = true;
        let (path, forget) = (run.path.clone(), run.forget);
        let mut s = lock(&self.inner.state);
        let Some(i) = s.index_of(&path) else {
            return;
        };
        match state {
            CopyState::Current => {
                touch(&copy);
                s.done(i, copy.to_string_lossy().into());
                self.map_dirty = true;
                let keep = s.jobs.iter().map(|j| j.stem.clone()).collect();
                tidy_in_background(self.inner.root.clone(), keep);
            }
            CopyState::Changed => {
                // Its source changed after the worker's last look: the copy is of an older version, the cache's own.
                remove_copy(&copy);
                s.requeue(i);
                drop(s);
                self.redecide(&path);
                self.resume_scans();
                self.changed();
                return;
            }
            CopyState::Unreachable => {
                self.kept.insert(path.clone(), copy);
                s.retain(|j| j.path != path);
                self.absent.insert(path.clone());
                self.map_dirty = true;
            }
        }
        if forget {
            s.retain(|j| j.path != path);
            self.map_dirty = true;
        }
        drop(s);
        self.resume_scans();
        self.changed();
    }

    /// Job `i` (paused for heat, a busy encoder or a stopped worker) is tried again by itself after its wait: one
    /// delayed task per pause, no polling. Not for a row that goes (`forget`).
    fn wake_later(&self, s: &ScheduleState, i: usize, forget: bool) {
        let Some(wait) = s.retry_in(i).filter(|_| !forget) else {
            return;
        };
        let tx = self.inner.tx.clone();
        let (path, stem) = (s.jobs[i].path.clone(), s.jobs[i].stem.clone());
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(wait).await;
            let _ = tx.send(Msg::Wake { path, stem });
        });
    }

    /// Decides one file again (its source changed: a new key, so a new job).
    fn redecide(&mut self, path: &Arc<str>) {
        self.decide_in_background(vec![path.clone()]);
    }

    fn on_exited(&mut self, id: u64) {
        let Some(run) = self.run.take_if(|r| r.id == id) else {
            return;
        };
        if !run.ended && !run.stopping {
            // Its events ended without a last one (the runner says a crash as `Crashed`, so this is a channel cut
            // short): a stop without a word, started again as a crash is.
            let mut s = lock(&self.inner.state);
            if let Some(i) = s.index_of(&run.path) {
                match run.asked {
                    Some(asked) => {
                        let why = s.hold().unwrap_or(asked);
                        s.paused(i, why);
                    }
                    None => s.crashed(i, "the editing copy's events ended early"),
                }
                self.wake_later(&s, i, run.forget);
            }
        }
        self.resume_scans();
        self.changed();
    }

    /// The preview couldn't open `path`'s copy (purged, or damaged) and reads the source file: the copy is forgotten and
    /// made again (a damaged one is deleted first, so it isn't found again).
    fn on_unusable(&mut self, path: &str) {
        let mut s = lock(&self.inner.state);
        let Some(i) = s.index_of(path) else {
            return;
        };
        if let JobState::Done(copy) = &s.jobs[i].state {
            let copy = PathBuf::from(&**copy);
            if copy.starts_with(&self.inner.root) {
                remove_copy(&copy);
            }
            s.requeue(i);
            self.map_dirty = true;
        }
        drop(s);
        self.changed();
    }

    fn on_command(&mut self, command: Command) -> Result<(), CodedError> {
        match command {
            Command::Pause(PauseBy::Person) => self.state().user_paused = true,
            Command::Pause(PauseBy::Action) => self.state().action_paused = true,
            Command::Resume(PauseBy::Person) => self.state().resume_all(),
            Command::Resume(PauseBy::Action) => self.state().resume_action(),
            Command::Stop(path) => {
                let mut s = lock(&self.inner.state);
                let i = s
                    .index_of(&path)
                    .ok_or_else(|| CodedError::new(NOT_IN_PROJECT))?;
                match &s.jobs[i].state {
                    // A finished copy stays (it is the preview's); stopping applies to one being made or waiting.
                    JobState::Done(_) | JobState::Stopped => {}
                    _ => {
                        let stem = s.jobs[i].stem.clone();
                        match self.run.as_mut().filter(|r| *r.path == *path && !r.ended) {
                            // The worker is killed and its job folder deleted (`ProxyRunner::stop`).
                            Some(run) => {
                                run.stopping = true;
                                let _ = run.ctl.send(Ctl::Stop);
                            }
                            None if !stem.is_empty() => {
                                let dir = job_dir(&self.inner.root, &stem);
                                tokio::task::spawn_blocking(move || remove_job(&dir));
                            }
                            None => {}
                        }
                        s.stopped(i);
                    }
                }
            }
            Command::Retry(path) => {
                let mut s = self.state();
                let i = s
                    .index_of(&path)
                    .ok_or_else(|| CodedError::new(NOT_IN_PROJECT))?;
                s.retry(i);
            }
            Command::SetAuto(auto) => {
                let was = std::mem::replace(&mut self.state().auto, auto);
                if was && !auto {
                    // The running copy pauses (kept for later) and goes from the list with every other unfinished one;
                    // finished copies stay mapped.
                    self.pause_run(Pause::User, true);
                    let running = self.run.as_ref().map(|r| r.path.clone());
                    self.state().retain(|j| {
                        matches!(j.state, JobState::Done(_)) || Some(&j.path) == running.as_ref()
                    });
                } else if !was && auto {
                    // Back on before the running copy has paused: it stays listed (paused, it is made on from where it
                    // stopped) unless its file left the project meanwhile.
                    let project = &self.project;
                    if let Some(run) = self.run.as_mut().filter(|r| {
                        !r.ended && project.as_ref().is_some_and(|v| v.paths.contains(&r.path))
                    }) {
                        run.forget = false;
                    }
                }
                if was != auto {
                    // Listed again either way. A listing still running stops at its next file and what it decided is
                    // dropped (a new generation): off, nothing more is probed, and an answer it decided before a copy
                    // finished can't take that copy out of the preview; the new listing only looks for finished
                    // copies.
                    let project = self.project.clone();
                    self.on_project(project);
                }
            }
        }
        Ok(())
    }

    /// Hands the engine the finished copies when they changed.
    fn push_map(&mut self) {
        if !self.map_dirty {
            return;
        }
        let Some(preview) = &self.preview else {
            return;
        };
        self.map_dirty = false;
        let map = self.state().media_map();
        if self.pushed.as_ref() != Some(&map) {
            preview.set_media(Arc::new(map.clone()));
            self.pushed = Some(map);
        }
    }
}

/// A copy that failed because its file isn't there.
fn is_missing(job: &Job) -> bool {
    matches!(
        job.state,
        JobState::Failed {
            failure: Failure::Missing,
            ..
        }
    )
}

/// Takes `path`'s row (at `at`) out of the list; true when there was one.
fn unlist(s: &mut ScheduleState, at: Option<usize>, path: &str) -> bool {
    if at.is_none() {
        return false;
    }
    s.retain(|j| *j.path != *path);
    true
}

/// Owns one worker: forwards its events, pauses or stops it when asked. Never loses work: if the app lets go of it
/// (`ctl` closed), it pauses it.
async fn run_copy(
    mut runner: ProxyRunner,
    mut ctl: mpsc::UnboundedReceiver<Ctl>,
    tx: mpsc::UnboundedSender<Msg>,
    id: u64,
) {
    let mut paused = false;
    let mut stop = loop {
        tokio::select! {
            event = runner.next() => match event {
                Some(event) => {
                    let _ = tx.send(Msg::Event { run: id, event });
                }
                None => break false,
            },
            asked = ctl.recv(), if !paused => match asked {
                Some(Ctl::Pause) | None => {
                    paused = true;
                    // Its last events keep coming while it winds down.
                    runner.pause().await;
                }
                Some(Ctl::Stop) => break true,
            },
        }
    };
    // A stop asked while it was pausing still deletes its job (the person stopped it).
    while let Ok(asked) = ctl.try_recv() {
        stop |= matches!(asked, Ctl::Stop);
    }
    if stop {
        runner.stop().await;
    }
    let _ = tx.send(Msg::Exited { run: id });
}
