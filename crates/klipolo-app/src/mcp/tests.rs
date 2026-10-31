//! The app as the MCP server's host, over a real project session (`project::opened`) holding the chat crate's two-clip
//! project, chat's own session and its slot, and credentials in memory: an app's batch takes the one apply path and is
//! published, its proposal waits in chat's slot under its name, the person's own changes and decisions always win, and
//! a job that costs money is refused at once without a key, a code or enough credit. The switch, with a real socket on
//! 127.0.0.1.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use klipolo_analysis::providers::images::ImageMaker;
use klipolo_app::analysis::AssetAnalysis;
use klipolo_app::chat::{
    BLUR_COMING, ChatOutcome, ChatRequest, ChatSession, CommitError, FOLLOW_COMING, Findings,
    Models, Projects, Readiness,
};
use klipolo_app::credentials::{
    Credentials, Endpoints, MemoryStore, RELAY_CODE, RELAY_TOKEN, SecretStore,
};
use klipolo_app::prefs;
use klipolo_chat::tools::{PictureSize, Position};
use klipolo_chat::{AnalysisState as ChatAnalysis, BoxFuture, Job, MakeSticker, Moment, Proposal};
use klipolo_mcp::Mcp;
use klipolo_mcp::call::from_app;
use klipolo_mcp::connection;
use klipolo_mcp::host::{Applied, Host, HostError, OpenProposal, ProposalOutcome};
use klipolo_mcp::words::{DROPPED_CHANGED, DROPPED_CLOSED, KLIPOLO_CHAT, NO_PROMPT, REMOVES};
use klipolo_meta::SecretString;
use klipolo_model::Op;
use klipolo_model::history::Maker;
use klipolo_model::project::Project;
use klipolo_model::speech::Levels;
use klipolo_model::time::RangeUs;
use serde_json::json;
use std::collections::HashMap;

use super::*;
use crate::project::{OpenProject, apply_transaction, create_in, opened, step, with_open};
use crate::view::{EditorView, ViewState};

const PROJECT: &str = include_str!("../../../klipolo-chat/tests/fixtures/project_two_clips.json");
/// Nothing listens on the discard port: a call sent here fails at once.
const CLOSED: &str = "http://127.0.0.1:9";

fn two_clips() -> Project {
    klipolo_model::load_project(PROJECT).expect("the fixture project loads")
}

/// No clip analyzed.
struct NoFindings;

impl Findings for NoFindings {
    fn found(&self, _: &str) -> Option<Arc<AssetAnalysis>> {
        None
    }

    fn state(&self, _: &str) -> ChatAnalysis {
        ChatAnalysis::NotStarted
    }

    fn levels(&self) -> HashMap<String, Arc<Levels>> {
        HashMap::new()
    }
}

/// No model: chat's own messages aren't sent here.
struct NoModels;

impl Models for NoModels {
    fn ready(&self) -> BoxFuture<'_, Readiness> {
        Box::pin(async { Readiness::NeedsKey })
    }

    fn images(&self) -> BoxFuture<'_, Result<Arc<dyn ImageMaker>, String>> {
        Box::pin(async { Err("no pictures here".to_owned()) })
    }

    fn picture_usd(&self) -> f64 {
        0.04
    }

    fn recover(&self) -> BoxFuture<'_, bool> {
        Box::pin(async { false })
    }
}

/// The host over the two-clip project, with what it told the page.
struct Rig {
    state: Arc<ProjectSessionState>,
    chat: Arc<ChatSession>,
    host: AppHost<Arc<ProjectSessionState>>,
    told: Arc<Mutex<Vec<Told>>>,
    view: ViewHandle,
    rt: tokio::runtime::Runtime,
}

impl Rig {
    fn new() -> Rig {
        Rig::with(|_| {}, CLOSED)
    }

    /// With `store` filled by `fill`, and the relay at `relay`.
    fn with(fill: impl FnOnce(&MemoryStore), relay: &str) -> Rig {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let store = MemoryStore::default();
        fill(&store);
        let endpoints = Endpoints {
            meta_base: CLOSED.to_owned(),
            relay_base: relay.to_owned(),
        };
        let credentials = Arc::new(Credentials::load(
            Arc::new(store),
            endpoints,
            "device".into(),
        ));
        let state = Arc::new(opened(two_clips()));
        let chat = Arc::new(ChatSession::new(
            state.clone(),
            Arc::new(NoFindings),
            Arc::new(NoModels),
        ));
        let told = Arc::new(Mutex::new(Vec::new()));
        let sink = told.clone();
        let tell: Tell = Arc::new(move |t| sink.lock().unwrap().push(t));
        let view: ViewHandle = Arc::new(ViewState::default());
        let host = AppHost::new(
            state.clone(),
            chat.clone(),
            Arc::new(NoFindings),
            credentials,
            view.clone(),
            tell,
            rt.handle().clone(),
        );
        Rig {
            state,
            chat,
            host,
            told,
            view,
            rt,
        }
    }

    fn base(&self) -> u64 {
        self.host.snapshot().expect("a project").revision
    }

    fn project(&self) -> Arc<Project> {
        Projects::now(&*self.state).expect("a project").project
    }

    /// What was told, as JSON, emptied.
    fn told(&self) -> Vec<serde_json::Value> {
        let told = std::mem::take(&mut *self.told.lock().unwrap());
        (told.into_iter())
            .map(|t| match t {
                Told::Reply(reply) => json!({"reply": reply}),
                Told::Apps(apps) => json!({"apps": apps}),
            })
            .collect()
    }

    /// `client`'s proposal to cut the first second, made on the project as it is.
    fn propose(&self, client: &str, replacing: Option<&str>) -> Result<String, HostError> {
        let base = self.base();
        let ranges = vec![RangeUs {
            start: 0,
            end: 1_000_000,
        }];
        self.host.propose(
            client,
            base,
            &[ToolCall::KeepOnly {
                ranges: vec![RangeUs {
                    start: 1_000_000,
                    end: 13_000_000,
                }],
            }],
            cut(&self.project(), ranges),
            replacing,
        )
    }

    /// The person's hand edit: the project renamed.
    fn hand_edit(&self, name: &str) {
        let id = self.project().id.clone();
        apply_transaction(&self.state, &id, &rename(name), None).expect("the hand edit");
    }

    /// Who made the newest undo step, and its label and params.
    fn newest_step(&self) -> Option<(Maker, String, serde_json::Value)> {
        with_open(&self.state, |open: &mut OpenProject| {
            Ok(open.document.undo_view().last().map(|step| {
                let params = serde_json::to_value(step.params).unwrap();
                (step.maker.clone(), step.label.to_owned(), params)
            }))
        })
        .unwrap()
    }

    fn engine_revision(&self) -> u64 {
        self.state.subscribe().borrow().as_ref().unwrap().revision
    }
}

fn rename(name: &str) -> Transaction {
    Transaction {
        label: "Rename".into(),
        ops: vec![Op::Rename { name: name.into() }],
        ..Default::default()
    }
}

/// A proposal to cut `ranges` from `project`, judged on it.
fn cut(project: &Project, ranges: Vec<RangeUs>) -> Proposal {
    let tx = Transaction {
        label: "chat.keepOnly".into(),
        ops: vec![Op::DeleteRanges {
            ranges: ranges.clone(),
        }],
        ..Default::default()
    };
    let review = tx.review(project).unwrap();
    Proposal {
        removed_us: ranges.iter().map(RangeUs::len).sum(),
        ranges,
        review,
        tx,
    }
}

fn sticker(prompt: &str) -> Job {
    Job::MakeSticker(MakeSticker {
        prompt: prompt.to_owned(),
        at_us: 0,
        duration_us: 3_000_000,
        position: Position::TopRight,
        size: PictureSize::Medium,
        anchor: None,
    })
}

#[test]
fn host_commits_through_the_one_apply_path_and_publishes() {
    let r = Rig::new();
    let base = r.base();
    assert_eq!(base, 1, "the first opening's revision 0");
    let tx = from_app(&rename("Day one"), "Example Client");
    let applied = r.host.apply("Example Client", base, &tx).unwrap();
    assert_eq!(
        applied,
        Applied {
            revision: base + 1,
            dropped: None
        }
    );
    assert_eq!(r.project().name, "Day one");
    assert_eq!(r.engine_revision(), 1, "the engine has it, as every edit");
    assert_eq!(
        r.newest_step(),
        Some((
            Maker::App("Example Client".into()),
            "Rename".to_owned(),
            json!({"client": "Example Client"})
        )),
        "one undo step the app made, named after it"
    );
    let told = r.told();
    assert_eq!(told.len(), 1, "{told:#?}");
    assert_eq!(told[0]["reply"]["client"], "Example Client");
    let reply = &told[0]["reply"]["reply"];
    assert_eq!(
        reply["applied"]["undo"],
        json!({"code": "Rename", "params": {"client": "Example Client"}})
    );
    assert_eq!(reply["applied"]["change"]["baseRevision"], 0);
    assert_eq!(reply["revision"], 1);
    // The page matches the change by the opening and the revision: revisions restart with each opening.
    assert_eq!(told[0]["reply"]["opening"], 1);
    assert_eq!(reply["proposal"], serde_json::Value::Null);

    // A base the project moved on from, or nothing to change: refused, or nothing told.
    assert_eq!(r.host.apply("Example Client", base, &tx), Err(HostError::Moved));
    r.hand_edit("Mine");
    assert_eq!(r.host.apply("Example Client", base + 1, &tx), Err(HostError::Moved));
    assert_eq!(r.project().name, "Mine", "the person's edit stays");
    let same = from_app(&rename("Mine"), "Example Client");
    let unchanged = r.host.apply("Example Client", r.base(), &same).unwrap();
    assert_eq!(unchanged.revision, r.base());
    assert!(r.told().is_empty(), "nothing changed, nothing told");
    // Checked again on the project as it is: a batch that takes footage away never applies here.
    let cuts = from_app(
        &Transaction {
            label: "chat.cutRange".into(),
            ops: vec![Op::DeleteRanges {
                ranges: vec![RangeUs {
                    start: 0,
                    end: 1_000_000,
                }],
            }],
            ..Default::default()
        },
        "Example Client",
    );
    assert_eq!(
        r.host.apply("Example Client", r.base(), &cuts),
        Err(HostError::Refused(REMOVES.to_owned()))
    );
    // Judged by what its ops take away, never by its origin: one with none is refused all the same.
    let plain = Transaction {
        origin: None,
        ..cuts.clone()
    };
    assert_eq!(
        r.host.apply("Example Client", r.base(), &plain),
        Err(HostError::Refused(REMOVES.to_owned()))
    );
    assert_eq!(r.project().timeline.main.len(), 2);
}

#[test]
fn host_proposal_fills_the_chat_slot_and_names_the_client() {
    let r = Rig::new();
    let base = r.base();
    let id = r.propose("Example Client", None).unwrap();
    assert_eq!(
        r.host.open_proposal(),
        Some(OpenProposal {
            id: id.clone(),
            base_revision: base,
            from: Some("Example Client".into()),
        })
    );
    // Chat's own slot holds it: the panel's one proposal.
    let waiting = r.chat.slot().waiting().unwrap();
    assert_eq!(
        (waiting.id.as_str(), waiting.from.as_deref()),
        (id.as_str(), Some("Example Client"))
    );
    // The person chose nothing: the card says the parts were suggested (the page names who under it).
    assert_eq!(waiting.title, "Keep only the suggested parts");
    let told = r.told();
    assert_eq!(told.len(), 1, "{told:#?}");
    assert_eq!(told[0]["reply"]["client"], "Example Client");
    let card = &told[0]["reply"]["reply"]["proposal"];
    assert_eq!(card["id"], id.as_str());
    assert_eq!(card["title"], "Keep only the suggested parts");
    assert_eq!(card["removedUs"], 1_000_000);
    assert_eq!(told[0]["reply"]["reply"]["revision"], 0);
    let expected = ProposalOutcome::Waiting {
        id: id.clone(),
        from: "Example Client".into(),
        title: "Keep only the suggested parts".into(),
        removed_sec: 1.0,
        ranges: vec![[0.0, 1.0]],
    };
    assert_eq!(r.host.outcome("Example Client"), expected);
    // Another app sees it waiting from Example Client, and its own changes wait for the person.
    assert!(matches!(
        r.host.outcome("Example Client"),
        ProposalOutcome::Waiting { from, .. } if from == "Example Client"
    ));
    let rename_by = |client| from_app(&rename("Theirs"), client);
    assert_eq!(
        r.host.apply("Example Client", base, &rename_by("Example Client")),
        Err(HostError::Moved)
    );
    assert_eq!(r.host.undo_own("Example Client"), Err(HostError::Moved));
    assert_eq!(r.propose("Example Client", None), Err(HostError::Moved));
    assert_eq!(r.propose("Example Client", Some(&id)), Err(HostError::Moved));
    assert_eq!(r.project().name, "Space station Q&A");
    // Cursor's next proposal takes its place only in place of the one it names.
    assert_eq!(r.propose("Example Client", None), Err(HostError::Moved));
    let again = r.propose("Example Client", Some(&id)).unwrap();
    assert_ne!(again, id);
    assert_eq!(r.host.open_proposal().unwrap().id, again);
    assert!(r.chat.apply(&id).is_err(), "the one replaced is gone");
}

#[test]
fn hand_edit_drops_the_external_proposal_and_outcome_says_dropped() {
    let r = Rig::new();
    let id = r.propose("Example Client", None).unwrap();
    r.hand_edit("Mine");
    assert_eq!(r.host.open_proposal(), None);
    assert_eq!(
        r.host.outcome("Example Client"),
        ProposalOutcome::Dropped {
            id: id.clone(),
            why: DROPPED_CHANGED.to_owned()
        }
    );
    assert_eq!(r.chat.apply(&id).unwrap_err(), CommitError::Moved);
    assert_eq!(r.project().timeline.main.len(), 2, "nothing was cut");
    // Another app never made one.
    assert_eq!(r.host.outcome("Example Client"), ProposalOutcome::None);
}

#[test]
fn person_apply_records_applied_and_is_the_persons_step() {
    let r = Rig::new();
    let id = r.propose("Example Client", None).unwrap();
    let length = |p: &Project| klipolo_model::query::main_track_duration(&p.timeline.main);
    let before = length(&r.project());
    let applied = r.chat.apply(&id).unwrap();
    assert_eq!(applied.undo.code, "chat.keepOnly");
    assert_eq!(length(&r.project()), before - 1_000_000, "one second cut");
    assert_eq!(r.host.outcome("Example Client"), ProposalOutcome::Applied { id });
    assert_eq!(
        r.newest_step().map(|(maker, ..)| maker),
        Some(Maker::Person),
        "the person applied it: their step"
    );
    assert_eq!(r.host.undo_own("Example Client"), Err(HostError::NotYours));
    assert_eq!(length(&r.project()), before - 1_000_000);
}

#[test]
fn discard_records_discarded_and_changes_nothing() {
    let r = Rig::new();
    let id = r.propose("Example Client", None).unwrap();
    r.chat.discard("another");
    assert!(r.host.open_proposal().is_some(), "only by its id");
    r.chat.discard(&id);
    assert_eq!(r.host.outcome("Example Client"), ProposalOutcome::Discarded { id });
    assert_eq!(r.project().timeline.main.len(), 2);
    // New chat drops the one waiting.
    let next = r.propose("Example Client", None).unwrap();
    r.chat.reset();
    assert!(matches!(
        r.host.outcome("Example Client"),
        ProposalOutcome::Dropped { id, .. } if id == next
    ));
}

/// Fills `Applied::dropped` exactly: the app's own proposal goes with its own change, and it says which.
#[test]
fn an_apps_change_names_its_own_proposal_it_dropped() {
    let r = Rig::new();
    let id = r.propose("Example Client", None).unwrap();
    let applied = r
        .host
        .apply("Example Client", r.base(), &from_app(&rename("Day one"), "Example Client"))
        .unwrap();
    assert_eq!(applied.dropped.as_deref(), Some(id.as_str()));
    assert_eq!(
        r.host.outcome("Example Client"),
        ProposalOutcome::Dropped {
            id,
            why: DROPPED_CHANGED.to_owned()
        }
    );
    // With nothing waiting, nothing is dropped.
    let applied = r
        .host
        .apply("Example Client", r.base(), &from_app(&rename("Day two"), "Example Client"))
        .unwrap();
    assert_eq!(applied.dropped, None);
}

/// An app's change waits while the redo holds a step of the person's (any commit would drop it), told apart from a
/// push-out, and nothing changes.
#[test]
fn a_redo_of_the_persons_waits() {
    let r = Rig::new();
    r.hand_edit("Mine");
    with_open(&r.state, |open| step(open, false)).unwrap();
    let base = r.base();
    let tx = from_app(&rename("Day one"), "Example Client");
    assert_eq!(r.host.apply("Example Client", base, &tx), Err(HostError::RedoWaits));
    assert_eq!(r.base(), base);
    // Once the person redoes it, the app's change goes in.
    with_open(&r.state, |open| step(open, true)).unwrap();
    assert!(r.host.apply("Example Client", r.base(), &tx).is_ok());
}

#[test]
fn an_app_undoes_only_its_own_newest_step() {
    let r = Rig::new();
    assert_eq!(r.host.undo_own("Example Client"), Err(HostError::NothingToUndo));
    r.host
        .apply("Example Client", r.base(), &from_app(&rename("Day one"), "Example Client"))
        .unwrap();
    r.told();
    assert_eq!(r.host.undo_own("Example Client"), Err(HostError::NotYours));
    let undone = r.host.undo_own("Example Client").unwrap();
    assert_eq!(
        undone,
        Applied {
            revision: r.base(),
            dropped: None
        }
    );
    assert_eq!(r.project().name, "Space station Q&A");
    let told = r.told();
    assert_eq!(told.len(), 1);
    assert!(told[0]["reply"]["reply"]["undone"].is_object(), "{told:#?}");
    // The person's own step is never an app's to undo.
    r.hand_edit("Mine");
    assert_eq!(r.host.undo_own("Example Client"), Err(HostError::NotYours));
}

/// Another project opening drops the waiting proposal, and a revision read on the old one never names the new one,
/// whose own revisions start at 0 again.
#[test]
fn another_opening_drops_the_proposal_and_its_revisions() {
    let r = Rig::new();
    let old = r.base();
    let id = r.propose("Example Client", None).unwrap();
    let library = tempfile::tempdir().unwrap();
    create_in(&r.state, library.path(), two_clips()).unwrap();
    assert_eq!(r.host.open_proposal(), None);
    assert_eq!(
        r.host.outcome("Example Client"),
        ProposalOutcome::Dropped {
            id,
            why: DROPPED_CLOSED.to_owned()
        }
    );
    let tx = from_app(&rename("Day one"), "Example Client");
    assert_eq!(r.host.apply("Example Client", old, &tx), Err(HostError::Moved));
    assert!(
        r.base() > old,
        "the new opening reads above every old revision"
    );
    r.told();
    assert!(r.host.apply("Example Client", r.base(), &tx).is_ok());
    let told = r.told();
    assert_eq!(told[0]["reply"]["opening"], 2, "the new opening's");
    assert_eq!(told[0]["reply"]["reply"]["revision"], 1);
}

#[test]
fn revisions_only_grow_across_openings() {
    let revisions = Revisions::default();
    let at = |opening, revision| At {
        project_id: "p".into(),
        opening,
        revision,
    };
    assert_eq!(revisions.of(&at(1, 0)), 1);
    assert_eq!(revisions.of(&at(1, 7)), 8);
    assert_eq!(
        revisions.of(&at(1, 3)),
        4,
        "the same number for the same revision"
    );
    // The next opening starts above every number read: its 0 reads 9.
    assert_eq!(revisions.of(&at(2, 0)), 9);
    assert_eq!(revisions.of(&at(2, 2)), 11);
    // An older opening read late stands for none.
    assert_eq!(revisions.of(&at(1, 9)), 0);
    assert_eq!(revisions.of(&at(5, 0)), 12);
}

/// Moments an app points at are told to the page and never touch the proposal slot.
#[test]
fn moments_never_touch_the_slot() {
    let r = Rig::new();
    let id = r.propose("Example Client", None).unwrap();
    r.told();
    let moment = Moment {
        range: RangeUs {
            start: 2_000_000,
            end: 4_000_000,
        },
        label: "The launch".into(),
    };
    r.host.show_moments("Example Client", &[moment]);
    assert_eq!(r.host.open_proposal().unwrap().id, id, "the card stays");
    let told = r.told();
    assert_eq!(told.len(), 1);
    let reply = &told[0]["reply"]["reply"];
    assert_eq!(
        reply["moments"],
        json!([{"startUs": 2_000_000, "endUs": 4_000_000, "label": "The launch"}])
    );
    assert_eq!(reply["proposal"], serde_json::Value::Null);
}

/// The snapshot's message is always empty: an app's words are never the person's typed times. The person's view in
/// Klipolo is the one the page last sent.
#[test]
fn the_snapshot_reads_the_persons_view_and_no_message() {
    let r = Rig::new();
    let selection = RangeUs {
        start: 2_000_000,
        end: 3_000_000,
    };
    r.view.set(EditorView {
        selection: Some(selection),
        playhead_us: 2_500_000,
        item: None,
        format: None,
    });
    let snap = r.host.snapshot().unwrap();
    assert_eq!(snap.inputs.message, "");
    assert_eq!(snap.inputs.view.selection, Some(selection));
    assert_eq!(snap.inputs.view.playhead_us, 2_500_000);
    assert_eq!(snap.inputs.refs.ref_of("clip-a1-2").as_deref(), Some("c2"));
    r.view.forget();
    assert_eq!(r.host.snapshot().unwrap().inputs.view.selection, None);
}

#[test]
fn start_paid_without_key_or_code_is_no_credit() {
    let r = Rig::new();
    let started = Instant::now();
    assert_eq!(
        r.host.start_paid("Example Client", &sticker("paper plane")),
        Err(HostError::NoCredit)
    );
    let title = Job::MakeTitleBackground {
        prompt: "Lisbon trams at sunset".into(),
        format: klipolo_model::project::FormatKind::Landscape,
        title_id: "title".into(),
    };
    assert_eq!(
        r.host.start_paid("Example Client", &title),
        Err(HostError::NoCredit)
    );
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "at once: {:?}",
        started.elapsed()
    );
    // Following and blurring answer as chat does, whatever pays.
    let follow = Job::Follow {
        description: "the dog".into(),
    };
    let blur = Job::Blur {
        description: "faces".into(),
    };
    assert_eq!(
        r.host.start_paid("Example Client", &follow),
        Err(HostError::Refused(FOLLOW_COMING.to_owned()))
    );
    assert_eq!(
        r.host.start_paid("Example Client", &blur),
        Err(HostError::Refused(BLUR_COMING.to_owned()))
    );
    assert!(r.host.estimate_usd(&sticker("x")) > 0.0);
    assert_eq!(r.host.estimate_usd(&follow), 0.0);
    assert!(r.told().is_empty(), "nothing asked for");
}

/// An access code whose relay credit is below the estimate, or turned off, refuses before any picture is asked for;
/// with credit, the picture is asked for as chat asks, its prompt cleaned and capped.
#[test]
fn low_relay_credit_is_refused_and_a_prompt_is_capped() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let rt = tokio::runtime::Runtime::new().unwrap();
    let relay = rt.block_on(MockServer::start());
    let device = |active: bool, budget: f64, spent: f64| {
        ResponseTemplate::new(200)
            .set_body_json(json!({"relay_ai": active, "budget_usd": budget, "spent_usd": spent}))
    };
    let r = Rig::with(
        |store| {
            let secret = |s: &str| SecretString::from(s.to_owned());
            store.set(RELAY_CODE, &secret("KLIP-TEST-0001")).unwrap();
            store.set(RELAY_TOKEN, &secret("relay-token")).unwrap();
        },
        &relay.uri(),
    );
    for (answer, refused) in [
        (device(true, 1.0, 0.995), true),
        (device(false, 5.0, 0.0), true),
        (device(true, 2.0, 0.1), false),
    ] {
        rt.block_on(async {
            relay.reset().await;
            Mock::given(method("GET"))
                .and(path("/v1/devices/me"))
                .respond_with(answer)
                .mount(&relay)
                .await;
        });
        let long = format!("paper\u{202E} plane {}", "x".repeat(400));
        let said = r.host.start_paid("Example Client", &sticker(&long));
        if refused {
            assert_eq!(said, Err(HostError::LowCredit));
            assert!(r.told().is_empty());
        } else {
            assert_eq!(said, Ok(()));
            let told = r.told();
            assert_eq!(told.len(), 1, "{told:#?}");
            let picture = &told[0]["reply"]["reply"]["pictures"][0];
            assert_eq!(picture["kind"], "sticker");
            let prompt = picture["prompt"].as_str().unwrap();
            assert!(prompt.starts_with("paper plane xxx"), "{prompt}");
            assert_eq!(prompt.chars().count(), PROMPT_CHARS);
        }
    }
    // A prompt with no words left once cleaned asks for one.
    assert_eq!(
        r.host.start_paid("Example Client", &sticker("\u{200B}\u{202E}")),
        Err(HostError::Refused(NO_PROMPT.to_owned()))
    );
}

/// The person's chat proposal waits: an app's change, undo and proposal are refused, and it reads as Klipolo's chat's.
#[test]
fn the_persons_own_proposal_wins() {
    let r = Rig::new();
    // "Keep only these" under moments: chat's own proposal, made without a model, keeping the second clip.
    let request = ChatRequest {
        id: "m1".into(),
        text: String::new(),
        selection: None,
        item: None,
        playhead_us: 0,
        format: None,
        keep_only: vec![RangeUs {
            start: 13_000_000,
            end: 144_040_000,
        }],
        base_revision: 0,
    };
    let outcome = r.rt.block_on(r.chat.send(request)).unwrap();
    let reply = match outcome {
        ChatOutcome::Reply(reply) => reply,
        other => panic!("a reply: {other:?}"),
    };
    let card = reply.proposal_ref().expect("a proposal");
    let id = card.id().to_owned();
    // The person chose these parts themselves: chat's own title stays as it was.
    assert_eq!(card.title(), "Keep only the parts you chose");
    assert_eq!(r.host.open_proposal().unwrap().id, id);
    assert_eq!(r.host.open_proposal().unwrap().from, None);
    assert!(matches!(
        r.host.outcome("Example Client"),
        ProposalOutcome::Waiting { from, title, .. }
            if from == KLIPOLO_CHAT && title == "Keep only the parts you chose"
    ));
    let tx = from_app(&rename("Day one"), "Example Client");
    assert_eq!(r.host.apply("Example Client", r.base(), &tx), Err(HostError::Moved));
    assert_eq!(r.host.undo_own("Example Client"), Err(HostError::Moved));
    assert_eq!(r.propose("Example Client", None), Err(HostError::Moved));
    assert_eq!(r.propose("Example Client", Some(&id)), Err(HostError::Moved));
    assert_eq!(r.project().name, "Space station Q&A");
}

/// Writes a POST to `/mcp` on 127.0.0.1:`port` with `headers`, and reads the status code back.
fn post(port: u16, headers: &[(&str, &str)], body: &str) -> u16 {
    post_as(port, &format!("127.0.0.1:{port}"), headers, body)
}

/// `post`, naming `host` as the `Host` it was sent to.
fn post_as(port: u16, host: &str, headers: &[(&str, &str)], body: &str) -> u16 {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut request = format!(
        "POST /mcp HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nAccept: application/json, \
         text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    request.push_str(body);
    stream.write_all(request.as_bytes()).unwrap();
    let mut answer = String::new();
    let _ = stream.read_to_string(&mut answer);
    answer
        .split(' ')
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or(0)
}

/// The switch, with a real socket: on, the connection file is this user's alone (0600) and a request without the
/// token is refused (401) before anything is read; off, the file goes, the setting is saved, and nothing listens.
#[test]
fn switch_off_removes_the_connection_file() {
    use std::os::unix::fs::PermissionsExt;
    let r = Rig::new();
    let dir = tempfile::tempdir().unwrap();
    let handle = r.rt.handle().clone();
    let told = r.told.clone();
    let mcp = Arc::new(Mcp::new(Arc::new(r.host)));
    let state = McpState::new(mcp, Some(dir.path().to_owned()), handle);
    assert!(!state.status().enabled);
    assert!(state.status().connected.is_empty());

    let on = state.set_enabled(true).unwrap();
    assert!(on.enabled);
    assert!(prefs::load(dir.path()).mcp_enabled, "saved on");
    let file = dir.path().join(connection::FILE);
    let mode = std::fs::metadata(&file).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    let written = connection::read(dir.path()).unwrap();
    let port = state.port().unwrap();
    assert_eq!(written.port, port);
    assert_eq!(written.pid, std::process::id());
    let hello = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "Example Client", "version": "1"}}})
    .to_string();
    assert_eq!(post(port, &[], &hello), 401, "no token");
    let wrong = format!("Bearer {}", "0".repeat(64));
    assert_eq!(post(port, &[("Authorization", &wrong)], &hello), 401);
    let bearer = format!("Bearer {}", written.token);
    // A browser page, or a name another site resolves to 127.0.0.1, is refused even with the token.
    let page = [
        ("Authorization", bearer.as_str()),
        ("Origin", "https://evil.test"),
    ];
    assert_eq!(post(port, &page, &hello), 403, "an Origin");
    let rebound = format!("evil.test:{port}");
    let token = [("Authorization", bearer.as_str())];
    assert_eq!(post_as(port, &rebound, &token, &hello), 403, "another Host");
    assert_eq!(post(port, &[("Authorization", &bearer)], &hello), 200);
    assert_eq!(state.status().connected, ["Example Client"]);
    // On again changes nothing: the same server, the same file.
    state.set_enabled(true).unwrap();
    assert_eq!(state.port(), Some(port));

    let off = state.set_enabled(false).unwrap();
    assert!(!off.enabled);
    assert!(off.connected.is_empty());
    assert!(!file.exists(), "the connection file goes");
    assert!(!prefs::load(dir.path()).mcp_enabled, "saved off");
    // The port closes once the accept loop hears the stop.
    let closed = Instant::now();
    while TcpStream::connect(("127.0.0.1", port)).is_ok() {
        assert!(closed.elapsed() < Duration::from_secs(5), "nothing listens");
        std::thread::sleep(Duration::from_millis(20));
    }
    // The page heard Cursor come and go.
    let apps: Vec<serde_json::Value> = (told.lock().unwrap().iter())
        .filter_map(|t| match t {
            Told::Apps(apps) => Some(json!(apps)),
            Told::Reply(_) => None,
        })
        .collect();
    assert_eq!(apps, [json!(["Example Client"]), json!([])]);
    // The config names this executable.
    let exe = std::env::current_exe().unwrap();
    assert_eq!(off.config, klipolo_app::mcp_config::config_json(&exe));
}

/// At launch: listening again when the person left it on; with it off, a file a crashed Klipolo left goes, but one a
/// Klipolo running now wrote stays.
#[test]
fn at_launch_listens_or_clears_what_a_crash_left() {
    let r = Rig::new();
    let dir = tempfile::tempdir().unwrap();
    let handle = r.rt.handle().clone();
    let mcp = Arc::new(Mcp::new(Arc::new(r.host)));
    let state = McpState::new(mcp, Some(dir.path().to_owned()), handle);
    let left = |pid| {
        let file = connection::ConnectionFile {
            port: 9,
            token: connection::new_token(),
            pid,
            version: connection::VERSION,
        };
        connection::write(dir.path(), &file).unwrap();
    };
    let mut gone = std::process::Command::new("true").spawn().unwrap();
    let dead = gone.id();
    gone.wait().unwrap();
    left(dead);
    state.at_launch();
    assert!(
        connection::read(dir.path()).is_none(),
        "a crash's file goes"
    );
    left(1);
    state.at_launch();
    assert!(
        connection::read(dir.path()).is_some(),
        "a live Klipolo's stays"
    );
    assert!(!state.status().enabled);
    prefs::save(dir.path(), &prefs::Prefs { mcp_enabled: true }).unwrap();
    state.at_launch();
    assert!(state.status().enabled);
    assert_eq!(
        connection::read(dir.path()).unwrap().pid,
        std::process::id()
    );
    state.stop();
    assert!(connection::read(dir.path()).is_none());
}
