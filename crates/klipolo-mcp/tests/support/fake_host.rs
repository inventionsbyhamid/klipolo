//! A `Host` over a real `Document` holding the chat crate's two-clip fixture (clip A of NASA's space station Q&A as
//! two pieces around a cut, with its transcript, speaker turns and scene log), one proposal slot shared by Klipolo's
//! chat and every app, and counters for what the server asked of it.
//!
//! Its rules, as the app keeps them: an app's batch commits with `commit_tx_keeping` as a step the app made, within
//! the apps' shares of the history; any change to the project drops a waiting proposal; a proposal is set only on the
//! revision and slot the caller saw; an app undoes only its own newest step; while a proposal that isn't the app's own
//! waits, its batches and undos are refused (checked under the same lock as the commit).

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use klipolo_chat::{
    AnalysisState, Inputs, Job, Library, Moment, Proposal, Refs, SceneLog, SpeakerTurn, ToolCall,
    View,
};
use klipolo_mcp::host::{Applied, Host, HostError, OpenProposal, ProposalOutcome, Snapshot};
use klipolo_mcp::limits::undo_shares;
use klipolo_mcp::words::KLIPOLO_CHAT;
use klipolo_model::history::{ByApp, HistoryLimits, Maker, Refused};
use klipolo_model::speech::{Speech, Transcript};
use klipolo_model::{Document, Project, Transaction, apply_ops, load_project};

const PROJECT: &str = include_str!("../../../klipolo-chat/tests/fixtures/project_two_clips.json");
const TRANSCRIPT: &str =
    include_str!("../../../klipolo-chat/tests/fixtures/transcript_clip_a.json");
const SCENES: &str = include_str!("../../../klipolo-chat/tests/fixtures/scenes_clip_a.json");
const TURNS: &str = include_str!("../../../klipolo-chat/tests/fixtures/turns_clip_a.json");

/// The one proposal waiting for the person, whoever made it.
#[derive(Clone, Debug)]
pub struct Slot {
    pub id: String,
    pub base_revision: u64,
    /// None: Klipolo's chat.
    pub from: Option<String>,
    pub calls: Vec<ToolCall>,
    pub proposal: Proposal,
}

/// How often the server asked for each thing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub snapshots: usize,
    pub applies: usize,
    pub proposes: usize,
    pub undos: usize,
    /// Jobs started.
    pub paid: usize,
    /// Jobs priced, and jobs asked to start (started or not).
    pub estimates: usize,
    pub paid_asked: usize,
}

pub struct State {
    /// None: no project is open.
    pub doc: Option<Document>,
    pub speech: Speech,
    pub scenes: HashMap<String, SceneLog>,
    pub turns: HashMap<String, Vec<SpeakerTurn>>,
    pub library: Library,
    /// The person's playhead and selection.
    pub view: View,
    /// What the snapshot gives as the person's typed message. Always empty in the app; set here to show a call never
    /// reads it.
    pub message: String,
    /// Each file's analysis where it isn't done.
    pub analysis: HashMap<String, AnalysisState>,
    pub slot: Option<Slot>,
    /// What became of each client's latest proposal once it no longer waits.
    pub outcomes: HashMap<String, ProposalOutcome>,
    pub next_proposal: u64,
    pub moments: Vec<(String, Vec<Moment>)>,
    pub started: Vec<(String, Job)>,
    /// Each batch an app committed, as it came.
    pub applied: Vec<(String, Transaction)>,
    /// Each list of connected apps the server told.
    pub connected: Vec<Vec<String>>,
    /// A Meta key or an access code, and enough relay credit.
    pub credit: bool,
    /// What one paid job costs.
    pub usd_per_job: f64,
    pub limits: HistoryLimits,
    pub counts: Counts,
    /// The next `propose` finds the project moved, as if someone proposed or edited just before it.
    pub move_before_propose: bool,
    /// Klipolo's chat proposes this just before the next app's batch or undo, as if the person asked while the app's
    /// call ran.
    pub chat_before_commit: Option<Proposal>,
}

pub struct FakeHost {
    state: Mutex<State>,
}

impl FakeHost {
    /// The two-clip project open, every file analyzed, the playhead at the start.
    pub fn two_clips() -> Arc<FakeHost> {
        let project = load_project(PROJECT).expect("the fixture project loads");
        let transcript: Transcript = serde_json::from_str(TRANSCRIPT).unwrap();
        let mut speech = Speech::default();
        speech
            .transcripts
            .insert("clip-a".into(), Arc::new(transcript));
        let scenes: SceneLog = serde_json::from_str(SCENES).unwrap();
        let turns: Vec<SpeakerTurn> = serde_json::from_str(TURNS).unwrap();
        Self::with(
            Some(Document::new(project).expect("a valid project")),
            speech,
            HashMap::from([("clip-a".to_owned(), scenes)]),
            HashMap::from([("clip-a".to_owned(), turns)]),
        )
    }

    /// No project open.
    pub fn empty() -> Arc<FakeHost> {
        Self::with(None, Speech::default(), HashMap::new(), HashMap::new())
    }

    fn with(
        doc: Option<Document>,
        speech: Speech,
        scenes: HashMap<String, SceneLog>,
        turns: HashMap<String, Vec<SpeakerTurn>>,
    ) -> Arc<FakeHost> {
        Arc::new(FakeHost {
            state: Mutex::new(State {
                doc,
                speech,
                scenes,
                turns,
                library: Library::default(),
                view: View::default(),
                message: String::new(),
                analysis: HashMap::new(),
                slot: None,
                outcomes: HashMap::new(),
                next_proposal: 1,
                moments: Vec::new(),
                started: Vec::new(),
                applied: Vec::new(),
                connected: Vec::new(),
                credit: true,
                usd_per_job: 0.04,
                limits: HistoryLimits::default(),
                counts: Counts::default(),
                move_before_propose: false,
                chat_before_commit: None,
            }),
        })
    }

    pub fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn project(&self) -> Project {
        self.state()
            .doc
            .as_ref()
            .expect("a project")
            .project()
            .clone()
    }

    pub fn revision(&self) -> u64 {
        self.state().doc.as_ref().expect("a project").revision()
    }

    pub fn set_view(&self, view: View) {
        self.state().view = view;
    }

    /// The history keeps `limits`, and the apps' shares follow them.
    pub fn set_limits(&self, limits: HistoryLimits) {
        let mut s = self.state();
        s.limits = limits;
        s.doc.as_mut().expect("a project").set_limits(limits);
    }

    /// The person clicks Undo. A waiting proposal is dropped.
    pub fn person_undo(&self) {
        let mut s = self.state();
        s.doc
            .as_mut()
            .expect("a project")
            .undo()
            .expect("a step to undo");
        s.drop_waiting("The project changed.");
    }

    /// The person clicks Redo. A waiting proposal is dropped.
    pub fn person_redo(&self) {
        let mut s = self.state();
        s.doc
            .as_mut()
            .expect("a project")
            .redo()
            .expect("a step to redo");
        s.drop_waiting("The project changed.");
    }

    /// The project as `change` leaves it, opened afresh (no history), before the test starts.
    pub fn change_project(&self, change: impl FnOnce(&mut Project)) {
        let mut s = self.state();
        let doc = s.doc.as_mut().expect("a project");
        let mut project = doc.project().clone();
        change(&mut project);
        doc.replace(project).expect("a valid project");
    }

    /// The person edits by hand: one step they made. A waiting proposal is dropped.
    pub fn person_edit(&self, tx: &Transaction) -> u64 {
        let mut s = self.state();
        let doc = s.doc.as_mut().expect("a project");
        let revision = doc.apply(tx).expect("the person's edit applies");
        s.drop_waiting("The project changed.");
        revision
    }

    /// Klipolo's chat proposes `proposal` on the current revision.
    pub fn chat_proposes(&self, proposal: Proposal) -> String {
        let mut s = self.state();
        let base_revision = s.doc.as_ref().expect("a project").revision();
        let id = s.new_id();
        s.slot = Some(Slot {
            id: id.clone(),
            base_revision,
            from: None,
            calls: Vec::new(),
            proposal,
        });
        id
    }

    /// The person clicks Apply on the waiting proposal: one step they made.
    pub fn person_applies(&self) -> u64 {
        let mut s = self.state();
        let slot = s.slot.take().expect("a waiting proposal");
        let doc = s.doc.as_mut().expect("a project");
        doc.commit_tx(&slot.proposal.tx, slot.proposal.review.after.clone());
        let revision = doc.revision();
        if let Some(client) = &slot.from {
            s.outcomes
                .insert(client.clone(), ProposalOutcome::Applied { id: slot.id });
        }
        revision
    }

    /// The person clicks Discard on the waiting proposal.
    pub fn person_discards(&self) {
        let mut s = self.state();
        let slot = s.slot.take().expect("a waiting proposal");
        if let Some(client) = &slot.from {
            s.outcomes
                .insert(client.clone(), ProposalOutcome::Discarded { id: slot.id });
        }
    }
}

impl State {
    /// Klipolo's chat proposes what `chat_before_commit` holds, if anything.
    fn chat_cuts_in(&mut self) {
        if let Some(proposal) = self.chat_before_commit.take() {
            let base_revision = self.doc.as_ref().expect("a project").revision();
            let id = self.new_id();
            self.slot = Some(Slot {
                id,
                base_revision,
                from: None,
                calls: Vec::new(),
                proposal,
            });
        }
    }

    fn new_id(&mut self) -> String {
        let id = format!("proposal-{}", self.next_proposal);
        self.next_proposal += 1;
        id
    }

    /// Whether a proposal waits that isn't `client`'s own: Klipolo's chat's or another app's.
    fn others_waiting(&self, client: &str) -> bool {
        self.slot
            .as_ref()
            .is_some_and(|slot| slot.from.as_deref() != Some(client))
    }

    /// A change to the project drops the waiting proposal: its removals were judged on the project before it.
    fn drop_waiting(&mut self, why: &str) {
        if let Some(slot) = self.slot.take()
            && let Some(client) = slot.from
        {
            self.outcomes.insert(
                client,
                ProposalOutcome::Dropped {
                    id: slot.id,
                    why: why.to_owned(),
                },
            );
        }
    }
}

fn secs(us: i64) -> f64 {
    us as f64 / 1e6
}

impl Host for FakeHost {
    fn snapshot(&self) -> Option<Snapshot> {
        let mut s = self.state();
        s.counts.snapshots += 1;
        let doc = s.doc.as_ref()?;
        let project = doc.shared();
        let inputs = Inputs {
            message: s.message.clone(),
            view: s.view,
            refs: Refs::of(&project),
            speech: s.speech.clone(),
            scenes: s.scenes.clone(),
            analysis: (project.assets.iter())
                .map(|a| {
                    let state = s.analysis.get(&a.id).copied();
                    (a.id.clone(), state.unwrap_or(AnalysisState::Done))
                })
                .collect(),
            library: s.library.clone(),
        };
        Some(Snapshot {
            revision: doc.revision(),
            project,
            inputs,
            turns: s.turns.clone(),
        })
    }

    fn open_proposal(&self) -> Option<OpenProposal> {
        self.state().slot.as_ref().map(|slot| OpenProposal {
            id: slot.id.clone(),
            base_revision: slot.base_revision,
            from: slot.from.clone(),
        })
    }

    fn apply(&self, client: &str, base: u64, tx: &Transaction) -> Result<Applied, HostError> {
        let mut s = self.state();
        s.counts.applies += 1;
        s.chat_cuts_in();
        if s.others_waiting(client) {
            return Err(HostError::Moved);
        }
        let limits = s.limits;
        let doc = s.doc.as_mut().ok_or(HostError::NoProject)?;
        if doc.revision() != base {
            return Err(HostError::Moved);
        }
        let next =
            apply_ops(doc.project(), &tx.ops).map_err(|e| HostError::Refused(e.to_string()))?;
        let shares = undo_shares(client, limits);
        let by = ByApp {
            app: client,
            shares: &shares,
            after_point: false,
        };
        match doc.commit_tx_keeping(tx, next, &by) {
            Ok(_) => {
                let revision = doc.revision();
                if revision != base {
                    s.drop_waiting("The project changed.");
                }
                s.applied.push((client.to_owned(), tx.clone()));
                Ok(Applied { revision })
            }
            Err(Refused::PushesOut(_)) => Err(HostError::PushesOut),
            Err(Refused::OverShare(..)) => Err(HostError::OverShare),
        }
    }

    fn propose(
        &self,
        client: &str,
        base: u64,
        calls: &[ToolCall],
        proposal: Proposal,
        replacing: Option<&str>,
    ) -> Result<String, HostError> {
        let mut s = self.state();
        s.counts.proposes += 1;
        if std::mem::take(&mut s.move_before_propose) {
            return Err(HostError::Moved);
        }
        let revision = s.doc.as_ref().ok_or(HostError::NoProject)?.revision();
        let open = s.slot.as_ref().map(|slot| slot.id.as_str());
        if revision != base || open != replacing {
            return Err(HostError::Moved);
        }
        let id = s.new_id();
        s.slot = Some(Slot {
            id: id.clone(),
            base_revision: base,
            from: Some(client.to_owned()),
            calls: calls.to_vec(),
            proposal,
        });
        s.outcomes.remove(client);
        Ok(id)
    }

    fn outcome(&self, client: &str) -> ProposalOutcome {
        let s = self.state();
        match &s.slot {
            Some(slot) => ProposalOutcome::Waiting {
                id: slot.id.clone(),
                from: slot.from.clone().unwrap_or_else(|| KLIPOLO_CHAT.to_owned()),
                title: (slot.calls.iter())
                    .map(ToolCall::name)
                    .collect::<Vec<_>>()
                    .join(", "),
                removed_sec: secs(slot.proposal.removed_us),
                ranges: (slot.proposal.ranges.iter())
                    .map(|r| [secs(r.start), secs(r.end)])
                    .collect(),
            },
            None => s
                .outcomes
                .get(client)
                .cloned()
                .unwrap_or(ProposalOutcome::None),
        }
    }

    fn undo_own(&self, client: &str) -> Result<Applied, HostError> {
        let mut s = self.state();
        s.counts.undos += 1;
        s.chat_cuts_in();
        if s.others_waiting(client) {
            return Err(HostError::Moved);
        }
        let doc = s.doc.as_mut().ok_or(HostError::NoProject)?;
        let mine = Maker::App(client.to_owned());
        match doc.undo_view().last().map(|step| step.maker == &mine) {
            None => Err(HostError::NothingToUndo),
            Some(false) => Err(HostError::NotYours),
            Some(true) => {
                doc.undo().ok_or(HostError::NothingToUndo)?;
                let revision = doc.revision();
                s.drop_waiting("The project changed.");
                Ok(Applied { revision })
            }
        }
    }

    fn show_moments(&self, client: &str, moments: &[Moment]) {
        self.state()
            .moments
            .push((client.to_owned(), moments.to_vec()));
    }

    fn estimate_usd(&self, _job: &Job) -> f64 {
        let mut s = self.state();
        s.counts.estimates += 1;
        s.usd_per_job
    }

    fn start_paid(&self, client: &str, job: &Job) -> Result<(), HostError> {
        let mut s = self.state();
        s.counts.paid_asked += 1;
        if !s.credit {
            return Err(HostError::NoCredit);
        }
        s.counts.paid += 1;
        s.started.push((client.to_owned(), job.clone()));
        Ok(())
    }

    fn connected(&self, clients: &[String]) {
        self.state().connected.push(clients.to_vec());
    }
}
