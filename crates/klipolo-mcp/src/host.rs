//! The app as the server sees it: a small trait the app implements over its project session, the chat panel's one
//! proposal slot and its credentials. Nothing here knows Tauri or a model vendor; tests use a fake.

use std::collections::HashMap;
use std::sync::Arc;

use klipolo_chat::{Inputs, Job, Moment, Proposal, SpeakerTurn, ToolCall};
use klipolo_model::{Project, Transaction};
use serde::{Deserialize, Serialize};

/// The open project as a call compiles against it, all of one revision.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub project: Arc<Project>,
    pub revision: u64,
    /// `message` empty (an agent's words are never the person's typed times), `refs` the project's main track
    /// (`Refs::of`), `view` the person's live playhead and selection in Klipolo.
    pub inputs: Inputs,
    /// Each file's speaker turns, by asset id.
    pub turns: HashMap<String, Vec<SpeakerTurn>>,
}

/// The proposal waiting for the person in the chat panel, whoever made it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenProposal {
    pub id: String,
    /// The revision its removals were judged on.
    pub base_revision: u64,
    /// The app that made it; None for Klipolo's chat.
    pub from: Option<String>,
}

/// A batch committed: the project's revision after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Applied {
    pub revision: u64,
}

/// Why the host didn't do what was asked. Nothing changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostError {
    NoProject,
    /// The revision, or the open proposal, isn't the one the caller saw.
    Moved,
    /// The project was made by a newer Klipolo.
    ReadOnly,
    /// Making room in the history would push out a step someone else made (the person's, or another app's).
    PushesOut,
    /// The app's share of the history, or all apps' together, is full.
    OverShare,
    /// The newest undo step isn't this app's.
    NotYours,
    NothingToUndo,
    /// A paid job with no Meta key or access code, or too little relay credit for its estimate.
    NoCredit,
    /// Anything else, in one plain sentence for the agent.
    Refused(String),
}

/// What an app reads about proposals (`getProposal`, `klipolo://proposal`), as JSON: `{"status": "waiting", ...}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ProposalOutcome {
    /// Nothing waits, and this app made no proposal yet.
    None,
    /// A proposal waits for the person: this app's, or someone else's (`from`, an app's name or "Klipolo's chat").
    Waiting {
        id: String,
        from: String,
        title: String,
        /// How much footage would go, in seconds.
        removed_sec: f64,
        /// Where, in timeline seconds as the project is now.
        ranges: Vec<[f64; 2]>,
    },
    /// The person applied this app's latest proposal.
    Applied { id: String },
    /// The person discarded it.
    Discarded { id: String },
    /// It was dropped before the person decided, and why (the project changed).
    Dropped { id: String, why: String },
}

/// The app, for the server. Every method is quick and never waits for the person; the server calls them from a
/// blocking thread.
pub trait Host: Send + Sync + 'static {
    /// The open project, or None when none is.
    fn snapshot(&self) -> Option<Snapshot>;

    /// The proposal waiting in the chat panel, if any.
    fn open_proposal(&self) -> Option<OpenProposal>;

    /// Commits `tx` as one undo step made by `client`. Refused with `Moved`, nothing committed, when the project isn't
    /// at `base`, or when a proposal waits that isn't `client`'s own (Klipolo's chat's or another app's): both checked
    /// under the host's own lock, in one step with the commit, so a proposal that appeared after the caller looked
    /// still wins. Implementations commit with `Document::commit_tx_keeping(tx, next, &ByApp { app: client, shares,
    /// after_point: false })`, `shares` the apps' shares of the history limits, so an app's step never pushes out the
    /// person's steps (`PushesOut`) or holds more than its share (`OverShare`). A commit changes the revision, so it
    /// drops the open proposal (then `client`'s own), as any revision change does.
    fn apply(&self, client: &str, base: u64, tx: &Transaction) -> Result<Applied, HostError>;

    /// Puts `proposal` (what `calls` compiled to on `base`) in the chat panel for the person to apply or discard, as
    /// `client`'s, in place of `replacing` (this client's open proposal) when given. Refused with `Moved` when the
    /// revision isn't `base` or the open proposal isn't `replacing`. The proposal's id.
    fn propose(
        &self,
        client: &str,
        base: u64,
        calls: &[ToolCall],
        proposal: Proposal,
        replacing: Option<&str>,
    ) -> Result<String, HostError>;

    /// The open proposal while one waits, whoever made it; else what became of `client`'s latest proposal; else
    /// `ProposalOutcome::None`.
    fn outcome(&self, client: &str) -> ProposalOutcome;

    /// Undoes the newest undo step, only when `client` made it (its maker is `Maker::App(client)`): `NotYours` when
    /// someone else did, `NothingToUndo` when there is none. Refused with `Moved`, nothing undone, when a proposal
    /// waits that isn't `client`'s own, checked under the host's own lock in one step with the undo. An undo changes
    /// the revision, so it drops the open proposal (then `client`'s own), as any revision change does.
    fn undo_own(&self, client: &str) -> Result<Applied, HostError>;

    /// Points the person at `moments` (timeline ranges), as `client` found them.
    fn show_moments(&self, client: &str, moments: &[Moment]);

    /// What `job` would cost, in US dollars.
    fn estimate_usd(&self, job: &Job) -> f64;

    /// Starts a job that costs money, after the app's own checks (a Meta key or an access code, relay credit for its
    /// estimate): `NoCredit` when they fail.
    fn start_paid(&self, client: &str, job: &Job) -> Result<(), HostError>;

    /// The apps connected now, by name, sorted: told whenever the list changes.
    fn connected(&self, clients: &[String]);
}
