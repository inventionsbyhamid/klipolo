//! Chat's one proposal slot, shared with other apps (MCP): an app's proposal waits there as chat's do, the person's
//! Apply holds the slot until its commit lands (a proposal being applied still waits for every app), and what became
//! of an app's proposal is kept for it: applied, discarded, or dropped and why.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use klipolo_analysis::providers::images::ImageMaker;
use klipolo_app::analysis::AssetAnalysis;
use klipolo_app::chat::{
    At, ChatSession, CommitError, Committed, Decided, Findings, Gone, MemoryProjects, Models,
    ProjectNow, Projects, Readiness,
};
use klipolo_chat::{AnalysisState, BoxFuture, Proposal};
use klipolo_model::ops::{Op, Transaction};
use klipolo_model::project::{Asset, AssetKind, Project};
use klipolo_model::speech::Levels;
use klipolo_model::time::RangeUs;
use klipolo_model::timeline::{Clip, ClipSource};

/// "talk.mov", 30 s, played whole.
fn talk() -> Project {
    let mut p = Project::new("Talk");
    p.assets = vec![Asset {
        id: "talk".into(),
        kind: AssetKind::Video,
        path: "/media/talk.mov".into(),
        name: "talk.mov".into(),
        duration_us: 30_000_000,
        width: 1920,
        height: 1080,
        fps: 30.0,
        rotation: None,
        has_audio: true,
        audio_lead_us: None,
        made_with: None,
    }];
    let mut clip = Clip::new(ClipSource::Asset { id: "talk".into() }, 0, 30_000_000);
    clip.id = "t".into();
    p.timeline.main = vec![clip];
    p
}

struct NoFindings;

impl Findings for NoFindings {
    fn found(&self, _: &str) -> Option<Arc<AssetAnalysis>> {
        None
    }

    fn state(&self, _: &str) -> AnalysisState {
        AnalysisState::NotStarted
    }

    fn levels(&self) -> HashMap<String, Arc<Levels>> {
        HashMap::new()
    }
}

struct NoModels;

impl Models for NoModels {
    fn ready(&self) -> BoxFuture<'_, Readiness> {
        Box::pin(async { Readiness::NeedsKey })
    }

    fn images(&self) -> BoxFuture<'_, Result<Arc<dyn ImageMaker>, String>> {
        Box::pin(async { Err("none".to_owned()) })
    }

    fn picture_usd(&self) -> f64 {
        0.04
    }

    fn recover(&self) -> BoxFuture<'_, bool> {
        Box::pin(async { false })
    }
}

/// The project in memory, whose next commit waits for the test to let it go.
struct Gated {
    inner: MemoryProjects,
    /// Told when a commit starts waiting; then it waits for `release`.
    entered: Mutex<Option<Sender<()>>>,
    release: Mutex<Option<Receiver<()>>>,
}

impl Projects for Gated {
    fn now(&self) -> Option<ProjectNow> {
        self.inner.now()
    }

    fn commit(
        &self,
        at: &At,
        tx: &Transaction,
        next: Option<Project>,
    ) -> Result<Committed, CommitError> {
        if let Some(entered) = self.entered.lock().unwrap().take() {
            entered.send(()).unwrap();
            let release = self.release.lock().unwrap().take().unwrap();
            release.recv().unwrap();
        }
        self.inner.commit(at, tx, next)
    }

    fn undo(&self, at: &At) -> Result<Committed, CommitError> {
        self.inner.undo(at)
    }
}

/// A cut of the first second, judged on `project`.
fn cut(project: &Project) -> Proposal {
    let ranges = vec![RangeUs {
        start: 0,
        end: 1_000_000,
    }];
    let tx = Transaction {
        label: "chat.cutRange".into(),
        ops: vec![Op::DeleteRanges {
            ranges: ranges.clone(),
        }],
        ..Default::default()
    };
    Proposal {
        review: tx.review(project).unwrap(),
        removed_us: 1_000_000,
        ranges,
        tx,
    }
}

/// `client`'s proposal, put in the slot on the project as it is.
fn propose(chat: &ChatSession, projects: &dyn Projects, client: &str) -> String {
    let now = projects.now().unwrap();
    let mut slot = chat.slot();
    let card = slot.propose(client, &now, &cut(&now.project), &["cutRange"]);
    card.id().to_owned()
}

#[test]
fn a_proposal_being_applied_still_waits() {
    let (entered, waiting) = channel();
    let (release, released) = channel();
    let projects = Arc::new(Gated {
        inner: MemoryProjects::open(talk(), PathBuf::from("/tmp/talk.klipolo")).unwrap(),
        entered: Mutex::new(Some(entered)),
        release: Mutex::new(Some(released)),
    });
    let chat = Arc::new(ChatSession::new(
        projects.clone(),
        Arc::new(NoFindings),
        Arc::new(NoModels),
    ));
    let id = propose(&chat, &*projects, "Muse Spark agent");
    let applying = {
        let (chat, id) = (chat.clone(), id.clone());
        std::thread::spawn(move || chat.apply(&id))
    };
    waiting.recv().unwrap();
    // While the person's Apply commits, another app reading the slot waits for it: never "nothing waits".
    let (said, heard) = channel();
    let reader = {
        let chat = chat.clone();
        std::thread::spawn(move || {
            let mut slot = chat.slot();
            let _ = said.send((slot.waiting(), slot.decided("Muse Spark agent")));
        })
    };
    assert!(
        heard.recv_timeout(Duration::from_millis(300)).is_err(),
        "the slot is held until the commit lands"
    );
    release.send(()).unwrap();
    assert!(applying.join().unwrap().is_ok());
    let (waiting, decided) = heard.recv_timeout(Duration::from_secs(5)).unwrap();
    reader.join().unwrap();
    assert_eq!(waiting, None);
    assert_eq!(decided, Some((id, Decided::Applied)));
    assert_eq!(
        projects.inner.project().timeline.main[0].source_in_us,
        1_000_000
    );
}

#[test]
fn what_became_of_an_apps_proposal_is_kept_for_it() {
    let projects =
        Arc::new(MemoryProjects::open(talk(), PathBuf::from("/tmp/talk.klipolo")).unwrap());
    let chat = ChatSession::new(projects.clone(), Arc::new(NoFindings), Arc::new(NoModels));
    assert_eq!(chat.slot().waiting(), None);
    assert_eq!(chat.slot().decided("Muse Spark agent"), None);

    // An app's keepOnly is worded as suggested: the person chose nothing.
    let now = projects.now().unwrap();
    let card = chat
        .slot()
        .propose("Muse Spark agent", &now, &cut(&now.project), &["keepOnly"]);
    assert_eq!(card.title(), "Keep only the suggested parts");
    chat.discard(card.id());

    // Discarded, by its id only.
    let id = propose(&chat, &*projects, "Muse Spark agent");
    let waiting = chat.slot().waiting().unwrap();
    assert_eq!(
        (waiting.from.as_deref(), waiting.removed_us),
        (Some("Muse Spark agent"), 1_000_000)
    );
    assert_eq!(
        waiting.ranges,
        vec![RangeUs {
            start: 0,
            end: 1_000_000
        }]
    );
    chat.discard("another");
    assert!(chat.slot().waiting().is_some());
    chat.discard(&id);
    assert_eq!(
        chat.slot().decided("Muse Spark agent"),
        Some((id, Decided::Discarded))
    );

    // A hand edit drops it; reading the slot says so, and the dropped one's id.
    let id = propose(&chat, &*projects, "Muse Spark agent");
    assert_eq!(
        chat.slot().decided("Muse Spark agent"),
        None,
        "a new one waits"
    );
    projects
        .hand_edit(&Transaction {
            label: "Rename".into(),
            ops: vec![Op::Rename {
                name: "Mine".into(),
            }],
            ..Default::default()
        })
        .unwrap();
    assert_eq!(chat.slot().settle(), Some(id.clone()));
    assert_eq!(
        chat.slot().decided("Muse Spark agent"),
        Some((id.clone(), Decided::Dropped(Gone::Changed)))
    );
    assert_eq!(chat.apply(&id).unwrap_err(), CommitError::Moved);

    // New chat drops it; so does another project.
    let id = propose(&chat, &*projects, "Muse Spark agent");
    chat.reset();
    assert_eq!(
        chat.slot().decided("Muse Spark agent"),
        Some((id, Decided::Dropped(Gone::NewChat)))
    );
    let id = propose(&chat, &*projects, "Example Client");
    projects.switch(Some(talk())).unwrap();
    chat.project_changed();
    assert_eq!(chat.slot().waiting(), None);
    assert_eq!(
        chat.slot().decided("Example Client"),
        Some((id, Decided::Dropped(Gone::Closed)))
    );
}
