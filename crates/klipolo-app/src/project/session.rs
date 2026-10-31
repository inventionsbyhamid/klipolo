//! The project session: the open project, the feed the engine sees (with what analysis found said in the clips), the
//! saver, and the project to open again at the next launch.

use klipolo_engine::registry::Registry;
use klipolo_model::speech::Speech;
use klipolo_model::time::RangeUs;

use super::*;

pub struct ProjectSessionState {
    pub(super) open: Mutex<Option<OpenProject>>,
    /// What the engine sees: published with the lock held, so the engine has the project before the command returns
    /// and publish order is revision order.
    pub(super) feed: watch::Sender<Option<ProjectFeed>>,
    /// Bumped by every create and open: revisions restart at 0, so each opening is a new engine session.
    pub(super) opens: AtomicU64,
    pub(super) saver: Saver,
    /// The file that names the last project opened (in the app's data folder), when there is one.
    last: Mutex<Option<PathBuf>>,
    /// What was said in the clips (analysis), sent with every publish.
    speech: Mutex<Arc<Speech>>,
    /// Klipolo's pack where this install has it: a project's pack files are pointed here as it opens.
    pack: Mutex<Arc<Pack>>,
    /// Told when the open project is about to change (another opens, a new one, or it closes), once the change is sure:
    /// the old project is saved and the new one read. Analysis stops and forgets the old project's clips here.
    on_switch: Mutex<Option<SwitchHook>>,
}

/// What `ProjectSessionState::on_switch` calls.
pub(crate) type SwitchHook = Arc<dyn Fn() + Send + Sync>;

impl Default for ProjectSessionState {
    fn default() -> Self {
        Self::with_saver(Saver::default())
    }
}

impl ProjectSessionState {
    pub(super) fn with_saver(saver: Saver) -> Self {
        Self {
            open: Mutex::default(),
            feed: watch::channel(None).0,
            opens: AtomicU64::new(0),
            saver,
            last: Mutex::default(),
            speech: Mutex::default(),
            pack: Mutex::default(),
            on_switch: Mutex::default(),
        }
    }

    /// Klipolo's pack for this install (read from the app's resources at launch).
    pub(crate) fn set_pack(&self, pack: Pack) {
        *self.pack.lock().unwrap_or_else(PoisonError::into_inner) = Arc::new(pack);
    }

    /// Klipolo's pack: empty until `set_pack`.
    pub(crate) fn pack(&self) -> Arc<Pack> {
        self.pack
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// `hook` is called whenever the open project changes, after the old one is saved and before the new one is
    /// published (`set_open`); never for a create or open that failed.
    pub(crate) fn on_switch(&self, hook: SwitchHook) {
        *self
            .on_switch
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(hook);
    }

    /// The open project is changing: tells the hook.
    pub(super) fn switching(&self) {
        let hook = self
            .on_switch
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(hook) = hook {
            hook();
        }
    }

    /// New speech for the engine: a new `Arc<Speech>` on the revision it has (it builds its captions again), and with
    /// every publish after, this project's or the next one's. Held while the feed is sent, so a publish never sends
    /// older speech after newer.
    pub(crate) fn set_speech(&self, speech: Arc<Speech>) {
        let mut held = self.speech.lock().unwrap_or_else(PoisonError::into_inner);
        *held = speech.clone();
        self.feed.send_if_modified(|current| match current {
            Some(feed) if !Arc::ptr_eq(&feed.speech, &speech) => {
                feed.speech = speech;
                true
            }
            _ => false,
        });
    }

    /// The open project's assets with these ids, in the order given (ids it doesn't have are left out).
    pub(crate) fn assets(&self, ids: &[String]) -> Vec<klipolo_model::project::Asset> {
        let open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(open) = open.as_ref() else {
            return Vec::new();
        };
        let assets = &open.document.project().assets;
        ids.iter()
            .filter_map(|id| assets.iter().find(|a| &a.id == id).cloned())
            .collect()
    }

    /// The engine's side of the feed.
    pub fn subscribe(&self) -> FeedRx {
        self.feed.subscribe()
    }

    /// Where someone speaks on the open project's timeline, as the engine's mix ducks music under it
    /// (`ducking_speech`, from the speech the feed carries): empty when nothing ducks, nobody is heard yet, or no
    /// project is open.
    pub(crate) fn ducking_speech(&self) -> Vec<RangeUs> {
        self.feed.borrow().as_ref().map_or_else(Vec::new, |feed| {
            klipolo_engine::audio::plan::ducking_speech(&feed.project, &feed.speech).to_vec()
        })
    }

    /// Where save failures (and the save that works again) are told.
    pub(crate) fn set_save_report(&self, report: saver::Report) {
        self.saver.set_report(report);
    }

    /// Waits until every edit is saved, at most `FLUSH_TIMEOUT` (closing, switching, quitting).
    pub(crate) fn flush(&self) -> Result<(), SaveError> {
        self.saver.flush(FLUSH_TIMEOUT)
    }

    /// The last project opened is named in `memo` (a small text file), and opens again at the next launch.
    pub(crate) fn remember_last_in(&self, memo: PathBuf) {
        *self.last.lock().unwrap_or_else(PoisonError::into_inner) = Some(memo);
    }

    /// `file` is the project open now: named in the memo, best effort.
    pub(super) fn remember_last(&self, file: &Path) {
        let memo = self
            .last
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let Some(memo) = memo else { return };
        let written = memo
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&memo, file.to_string_lossy().as_bytes()));
        if let Err(error) = written {
            tracing::warn!(%error, "the last project couldn't be remembered");
        }
    }

    /// Opens the project the memo names (the one open when the app last quit), if it is still there.
    pub(crate) fn open_last(&self) -> Option<Snapshot> {
        let memo = self
            .last
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()?;
        let file = PathBuf::from(std::fs::read_to_string(memo).ok()?.trim());
        if !file.is_file() {
            return None;
        }
        open_in(self, &file)
            .inspect_err(|error| tracing::warn!(?error, "the last project couldn't be opened"))
            .ok()
    }

    /// Tells the engine what is open now, and returns what was published. An unchanged (project id, revision,
    /// opening), such as a no-op batch or a failed apply, publishes nothing. `reopened` marks a create, open or close.
    pub(super) fn publish(
        &self,
        open: Option<&OpenProject>,
        reopened: bool,
    ) -> Option<Arc<Project>> {
        let opened = if reopened {
            self.opens.fetch_add(1, Ordering::SeqCst) + 1
        } else {
            self.opens.load(Ordering::SeqCst)
        };
        let mut published = None;
        // Held while the feed is sent (`set_speech`): the open lock, then this one, then the feed's.
        let speech = self.speech.lock().unwrap_or_else(PoisonError::into_inner);
        self.feed.send_if_modified(|current| {
            let next = open.map(|o| {
                (
                    o.document.project().id.as_str(),
                    o.document.revision(),
                    opened,
                )
            });
            let now = current
                .as_ref()
                .map(|f| (f.project_id.as_str(), f.revision, f.opened));
            if next == now {
                return false;
            }
            *current = open.map(|o| {
                // The document's own revision, shared: no copy of the project.
                let project = o.document.shared();
                published = Some(Arc::clone(&project));
                ProjectFeed {
                    project_id: project.id.clone(),
                    revision: o.document.revision(),
                    opened,
                    text_language: Arc::from("en"),
                    project,
                    speech: speech.clone(),
                    registry: Registry::builtin(),
                }
            });
            true
        });
        published
    }
}

/// A session with `project` open (saved nowhere unless flushed), for other modules' tests.
#[cfg(test)]
pub(crate) fn opened(project: Project) -> ProjectSessionState {
    let state = ProjectSessionState::with_saver(Saver::new(
        Duration::from_secs(600),
        Arc::new(klipolo_platform::durable::Os),
        Arc::new(|_| {}),
    ));
    let file = std::env::temp_dir().join(format!("{}.{PROJECT_EXTENSION}", project.id));
    set_open(
        &state,
        Some(OpenProject {
            document: Document::new(project).unwrap(),
            file,
            read_only: false,
        }),
    )
    .unwrap();
    state
}
