//! One chat per app: a message in, the model's answer applied, proposed and remembered.
//!
//! - **One message at a time.** A message waits for the one before it to land before anything is read (`queue`), so
//!   its context and its compiler inputs are built from the project as it is then (`inputs::gather`), and its calls
//!   compile against that same snapshot.
//! - **Additions** apply at once as one undo step (`origin: Chat`), on the revision the message was sent on; the reply
//!   carries what changed. **Removals** are a proposal, kept by id with the revision it applies to (the one after the
//!   additions). Apply records the project its review made (`commit_reviewed`): one undo step. A project that moved on
//!   from that revision (a hand edit, an undo) refuses it with `project.moved`, and it is dropped. Discard changes
//!   nothing. The model's `undo` takes back the top step, on the revision it saw.
//! - **The project moved while the model answered** (a hand edit meanwhile): nothing of the answer is applied or
//!   proposed, and the reply says so.
//! - **Errors** never touch the project: each is one plain sentence (`ChatFailure`). No key or code is `NeedsKey`; a
//!   code whose device couldn't register says why with Retry; a token the relay refused is renewed once and the message
//!   sent again.
//! - **The conversation** keeps the last `KEPT_TURNS` exchanges with their notes (what was applied or proposed, and
//!   later whether the person applied or discarded it), for the project open; another project, or New chat, starts it
//!   over.
//! - **Jobs:** following a subject and blurring aren't in Klipolo yet: the reply says they are coming (`jobs`).
//! - **Spend:** every model call's cost goes into the session's AI spend (`SpendCounter`); nothing is shown per message.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use klipolo_chat::{
    Audience, ChatErrorKind, Compiled, Inputs, PlanOut, ToolCall, Turn, View, compile, plan,
};
use klipolo_model::coded::CodedError;
use klipolo_model::history::SnapshotPatch;
use klipolo_model::time::RangeUs;
use klipolo_model::timeline::ClipSource;
use klipolo_model::{Project, Transaction};

use super::describe::{describe, seconds};
use super::inputs::gather;
use super::jobs;
use super::project::{At, CommitError, ProjectNow, Projects};
use super::reply::{
    ChatApplied, ChatFailure, ChatFailureKind, ChatOutcome, ChatProposal, ChatReply, ChatRequest,
};
use super::sources::{Findings, Models, Readiness};
use super::spend::{ChatSpend, SpendCounter};

/// The exchanges kept for the model: the planner's own `KEPT_TURNS`.
pub const KEPT_EXCHANGES: usize = klipolo_chat::KEPT_TURNS;
/// Proposals kept for Apply at once (each holds the project it would make); the oldest goes first.
pub const KEPT_PROPOSALS: usize = 4;

/// Said when no project is open.
pub const NO_PROJECT: &str = "Open a project first.";
/// Said when another project opened while the message waited its turn.
pub const OTHER_PROJECT: &str = "Another project opened, so I didn't send that.";
/// Said for a project from a newer Klipolo.
pub const READ_ONLY: &str = "This project is from a newer Klipolo, so chat can't change it.";
/// Said with no key and no code (the panel shows the NeedsKey card).
pub const NEEDS_KEY: &str = "Add your Meta API key or an access code in Settings to chat.";
/// Said when the project changed while the model answered: nothing of the answer was applied or proposed.
pub const MOVED: &str =
    "The project changed while I was answering, so I left it as it was. Ask me again.";
/// Said when the answer's changes couldn't be applied (they don't fit the project any more).
pub const NOT_APPLIED: &str = "I couldn't make those changes, so I left the project as it was.";
/// Said beside the additions applied when the removals of the same answer couldn't be proposed.
pub const NOT_PROPOSED: &str = "I couldn't get the removals ready, so I left them out.";
/// Said beside the additions applied when the project changed before the removals could be proposed.
pub const REMOVALS_MOVED: &str =
    "The project changed meanwhile, so I left the removals out. Ask me again.";
/// The reply to "Keep only these" when the moments still stand.
pub const KEEP_ONLY_READY: &str = "Here is what I would remove. Apply it or discard it.";
/// The reply to "Keep only these" when the moments no longer match the video.
pub const KEEP_ONLY_MOVED: &str =
    "Those moments don't match the video any more. Ask me to find them again.";

/// What a message's turn note gets when nothing of it landed.
const MOVED_NOTE: &str = "Nothing was applied: the person changed the project meanwhile.";
const APPLIED_NOTE: &str = "The person applied the proposal.";
const DISCARDED_NOTE: &str = "The person discarded the proposal.";
const DROPPED_NOTE: &str =
    "The proposal was dropped: the project changed before the person applied it.";

/// The app's chat.
pub struct ChatSession {
    projects: Arc<dyn Projects>,
    findings: Arc<dyn Findings>,
    models: Arc<dyn Models>,
    /// One message at a time: held from before its context is built until its edits have landed.
    queue: tokio::sync::Mutex<()>,
    talk: Mutex<Talk>,
    spend: SpendCounter,
}

/// The conversation with the open project.
#[derive(Default)]
struct Talk {
    /// Whose conversation it is: the project and its opening (the revision isn't part of it).
    owner: Option<At>,
    /// Oldest first, at most `KEPT_EXCHANGES`.
    exchanges: Vec<Exchange>,
    /// Oldest first, at most `KEPT_PROPOSALS`.
    proposals: Vec<Stored>,
    /// Where the project stood right after chat's last change: `undo_last` takes it back only from there.
    last_change: Option<At>,
}

struct Exchange {
    turn: Turn,
    /// The proposal it made, by id.
    proposal: Option<String>,
}

/// A proposal waiting for Apply or Discard.
struct Stored {
    id: String,
    /// The revision it applies to: the project after the message's additions.
    at: At,
    tx: Transaction,
    /// What applying it makes (its review), recorded as it is.
    after: Project,
}

/// What came of committing a message's edits.
#[derive(Default)]
struct Landed {
    undone: Option<SnapshotPatch>,
    applied: Option<ChatApplied>,
    /// The proposal made, with its id.
    proposal: Option<(String, ChatProposal)>,
    /// Nothing landed (the project moved on, or the edits no longer fit): the reply says so instead.
    lost: Option<&'static str>,
    /// The additions landed but the removals couldn't be proposed: said beside the reply.
    dropped: Option<&'static str>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn failed(kind: ChatFailureKind, message: &str) -> ChatOutcome {
    ChatOutcome::Failed(ChatFailure::new(kind, message))
}

/// A transaction's label as its undo step names it.
fn undo_label(tx: &Transaction) -> CodedError {
    CodedError {
        code: tx.label.clone(),
        params: tx.params.clone().unwrap_or_default(),
    }
}

impl ChatSession {
    pub fn new(
        projects: Arc<dyn Projects>,
        findings: Arc<dyn Findings>,
        models: Arc<dyn Models>,
    ) -> Self {
        Self {
            projects,
            findings,
            models,
            queue: tokio::sync::Mutex::new(()),
            talk: Mutex::default(),
            spend: SpendCounter::default(),
        }
    }

    /// What chat has cost this session.
    pub fn spend(&self) -> ChatSpend {
        self.spend.now()
    }

    /// The conversation as the model gets it, oldest first.
    pub fn turns(&self) -> Vec<Turn> {
        (lock(&self.talk).exchanges.iter())
            .map(|e| e.turn.clone())
            .collect()
    }

    /// One message: the model's answer, its additions applied, its removals proposed; or why nothing happened, the
    /// project untouched. A message waits for the one before it.
    pub async fn send(&self, request: ChatRequest) -> ChatOutcome {
        let text = request.text.trim().to_owned();
        // Which project the person sent it to, before it waits its turn.
        let asked = self.projects.now().map(|p| p.at);
        let _turn = self.queue.lock().await;
        let Some(now) = self.projects.now() else {
            return failed(ChatFailureKind::Other, NO_PROJECT);
        };
        if asked.is_some_and(|a| !a.same_project(&now.at)) {
            return failed(ChatFailureKind::Other, OTHER_PROJECT);
        }
        self.follow(&now.at);
        if now.read_only {
            return failed(ChatFailureKind::Other, READ_ONLY);
        }
        let view = View {
            playhead_us: request.playhead_us,
            selection: request.selection,
        };
        if !request.keep_only.is_empty() {
            let (_, inputs) = gather(&now.project, &*self.findings, &text, view);
            let out = keep_only(&now.project, &inputs, request.keep_only.clone());
            return self.land(&request.id, &text, &now, &inputs, out);
        }

        let (model, relay) = match self.models.ready().await {
            Readiness::Ready { model, relay } => (model, relay),
            Readiness::Problem(problem) => return failed(ChatFailureKind::Registration, &problem),
            Readiness::NeedsKey => return failed(ChatFailureKind::NeedsKey, NEEDS_KEY),
        };
        let (context, inputs) = gather(&now.project, &*self.findings, &text, view);
        let history = self.turns();
        let ask = |model: Arc<dyn klipolo_chat::LanguageModel>| {
            let (context, inputs, history, text, project) =
                (&context, &inputs, &history, &text, &now.project);
            async move { plan(&*model, history, context, text, inputs, project).await }
        };
        let mut planned = ask(model).await;
        // The relay refused the device's token: registered again once, and the message sent again.
        if relay && matches!(&planned, Err(e) if e.kind == ChatErrorKind::Refused) {
            if let Err(e) = &planned {
                self.spend.message(e.usage_usd, e.model_calls);
            }
            if self.models.recover().await
                && let Readiness::Ready { model, .. } = self.models.ready().await
            {
                planned = ask(model).await;
            }
        }
        match planned {
            Err(error) => {
                self.spend.message(error.usage_usd, error.model_calls);
                tracing::info!(kind = ?error.kind, detail = %error.detail, "chat: the message failed");
                ChatOutcome::Failed(ChatFailure::from(&error))
            }
            Ok(out) => {
                self.spend.message(out.usage_usd, out.answers.len());
                self.land(&request.id, &text, &now, &inputs, out)
            }
        }
    }

    /// Applies proposal `id` as one undo step. Refused with `project.moved` (and dropped) when the project moved on from
    /// the revision it was made for, or when it is gone (discarded, dropped, another project).
    pub fn apply(&self, id: &str) -> Result<ChatApplied, CommitError> {
        let stored = {
            let mut talk = lock(&self.talk);
            let at = talk.proposals.iter().position(|p| p.id == id);
            at.map(|at| talk.proposals.remove(at))
        };
        let Some(stored) = stored else {
            return Err(CommitError::Moved);
        };
        let committed = self
            .projects
            .commit(&stored.at, &stored.tx, Some(stored.after));
        let mut talk = lock(&self.talk);
        match committed {
            Ok(c) => {
                talk.last_change = Some(c.at.clone());
                talk.noted(id, APPLIED_NOTE);
                Ok(ChatApplied {
                    change: c.patch,
                    undo: undo_label(&stored.tx),
                })
            }
            Err(e) => {
                talk.noted(id, DROPPED_NOTE);
                Err(e)
            }
        }
    }

    /// Drops proposal `id`: nothing changes.
    pub fn discard(&self, id: &str) {
        let mut talk = lock(&self.talk);
        let before = talk.proposals.len();
        talk.proposals.retain(|p| p.id != id);
        if talk.proposals.len() < before {
            talk.noted(id, DISCARDED_NOTE);
        }
    }

    /// Takes back chat's last change, when nothing has changed since; else `project.moved`.
    pub fn undo_last(&self) -> Result<SnapshotPatch, CommitError> {
        let last = lock(&self.talk).last_change.clone();
        let last = last.ok_or(CommitError::Moved)?;
        let committed = self.projects.undo(&last)?;
        lock(&self.talk).last_change = None;
        Ok(committed.patch)
    }

    /// New chat: the conversation and its proposals are forgotten.
    pub fn reset(&self) {
        let mut talk = lock(&self.talk);
        talk.exchanges.clear();
        talk.proposals.clear();
    }

    /// Another project opened, or the project closed: everything about the old one is forgotten.
    pub fn project_changed(&self) {
        *lock(&self.talk) = Talk::default();
    }

    /// The conversation belongs to the project at `at`: one about another project (or another opening) is forgotten.
    fn follow(&self, at: &At) {
        let mut talk = lock(&self.talk);
        if !talk.owner.as_ref().is_some_and(|o| o.same_project(at)) {
            *talk = Talk {
                owner: Some(at.clone()),
                ..Talk::default()
            };
        }
    }

    /// Commits what `out` compiled to on the project `now` (the snapshot it compiled against), and builds the reply.
    fn land(
        &self,
        id: &str,
        user: &str,
        now: &ProjectNow,
        inputs: &Inputs,
        mut out: PlanOut,
    ) -> ChatOutcome {
        let landed = self.commit(now, inputs, &out);
        let jobs = jobs::start(&out.compiled);
        name_clips(&mut out, &now.project, inputs);

        let something = landed.undone.is_some()
            || landed.applied.is_some()
            || landed.proposal.is_some()
            || !out.compiled.moments.is_empty()
            || !out.compiled.questions.is_empty();
        let mut note = out.note.clone();
        let mut notes: Vec<String> = Vec::new();
        if let Some(dropped) = landed.dropped {
            notes.push(dropped.to_owned());
            note = format!("{note} The removals were not proposed.");
        }
        if let Some(lost) = landed.lost {
            // Nothing of the answer landed: its words may claim it did.
            out.reply = lost.to_owned();
            note = format!("{note} {MOVED_NOTE}");
        } else if !jobs.coming.is_empty() {
            if something {
                notes.extend(jobs.coming.iter().map(|s| (*s).to_owned()));
            } else {
                out.reply = jobs.coming.join(" ");
            }
            note = format!("{note} {}", jobs.note());
        }
        let mut reply = ChatReply::from_plan(id, &out);
        for n in &notes {
            reply = reply.note(n);
        }
        if let Some(applied) = landed.applied {
            reply = reply.applied(applied.change, applied.undo);
        }
        if let Some(undone) = landed.undone {
            reply = reply.undone(undone);
        }
        let mut made = None;
        if let Some((proposal_id, proposal)) = landed.proposal {
            made = Some(proposal_id);
            reply = reply.proposal(proposal);
        }
        let turn = Turn {
            user: user.to_owned(),
            reply: reply.text().to_owned(),
            note,
        };
        lock(&self.talk).remember(Exchange {
            turn,
            proposal: made,
        });
        ChatOutcome::Reply(Box::new(reply))
    }

    /// The model's undo, then the additions, then the proposal, each on the revision the one before left. The first
    /// that finds the project moved on stops the rest.
    fn commit(&self, now: &ProjectNow, inputs: &Inputs, out: &PlanOut) -> Landed {
        let compiled = &out.compiled;
        let mut landed = Landed::default();
        let mut at = now.at.clone();
        let mut base = now.project.clone();
        let lost = |e: CommitError| match e {
            CommitError::Refused(said) => {
                tracing::warn!(?said, "chat: an answer's changes didn't apply");
                NOT_APPLIED
            }
            CommitError::Moved | CommitError::NoProject => MOVED,
        };
        if compiled.undo {
            match self.projects.undo(&at) {
                Ok(c) => {
                    landed.undone = Some(c.patch);
                    (at, base) = (c.at, c.project);
                }
                Err(e) => {
                    landed.lost = Some(lost(e));
                    return landed;
                }
            }
        }
        if let Some(tx) = &compiled.additions {
            match self.projects.commit(&at, tx, None) {
                Ok(c) => {
                    landed.applied = Some(ChatApplied {
                        change: c.patch,
                        undo: undo_label(tx),
                    });
                    (at, base) = (c.at, c.project);
                    lock(&self.talk).last_change = Some(at.clone());
                }
                Err(e) => {
                    landed.lost = Some(lost(e));
                    return landed;
                }
            }
        }
        if let Some(p) = &compiled.proposal {
            // Made on the project the additions leave: as committed (applied once), it is judged again on that.
            let review = if landed.applied.is_some() {
                p.tx.review(&base).ok()
            } else {
                Some(p.review.clone())
            };
            let still = self.projects.now().is_some_and(|n| n.at == at);
            match review {
                Some(review) if still && !review.removals.is_empty() => {
                    let tools: Vec<&str> = (compiled.proposed_calls.iter())
                        .filter_map(|&i| out.calls.get(i))
                        .map(ToolCall::name)
                        .collect();
                    let removed_us = review.removals.ranges.iter().map(RangeUs::len).sum();
                    let said =
                        describe(&base, &inputs.speech, &review.removals, removed_us, &tools);
                    let id = uuid::Uuid::new_v4().to_string();
                    let card = ChatProposal::new(
                        id.clone(),
                        at.revision,
                        &said.title,
                        review.removals.clone(),
                        removed_us,
                        said.tags,
                    );
                    landed.proposal = Some((id.clone(), card));
                    lock(&self.talk).keep(Stored {
                        id,
                        at: at.clone(),
                        tx: p.tx.clone(),
                        after: review.after,
                    });
                }
                // Nothing to remove once the additions are in.
                Some(_) if still => {}
                // Applied once, the additions left a project the removals no longer fit; or the project moved on.
                review => {
                    let why = if review.is_none() { NOT_APPLIED } else { MOVED };
                    match landed.applied {
                        Some(_) if review.is_none() => landed.dropped = Some(NOT_PROPOSED),
                        Some(_) => landed.dropped = Some(REMOVALS_MOVED),
                        None => landed.lost = Some(why),
                    }
                }
            }
        }
        // Proposals made on a revision the project left can never apply.
        lock(&self.talk).proposals.retain(|p| p.at == at);
        landed
    }
}

impl Talk {
    fn remember(&mut self, exchange: Exchange) {
        self.exchanges.push(exchange);
        let over = self.exchanges.len().saturating_sub(KEPT_EXCHANGES);
        self.exchanges.drain(..over);
    }

    fn keep(&mut self, stored: Stored) {
        self.proposals.push(stored);
        let over = self.proposals.len().saturating_sub(KEPT_PROPOSALS);
        self.proposals.drain(..over);
    }

    /// What became of proposal `id`, added to the note of the exchange that made it.
    fn noted(&mut self, id: &str, what: &str) {
        if let Some(e) = (self.exchanges.iter_mut()).find(|e| e.proposal.as_deref() == Some(id)) {
            e.turn.note = format!("{} {what}", e.turn.note.trim());
        }
    }
}

/// "Keep only these" under moments: the moments' ranges kept, the rest proposed for removal, without asking the model
/// (they are Klipolo's own moments). Ranges that no longer stand on the video are refused.
fn keep_only(project: &Project, inputs: &Inputs, ranges: Vec<RangeUs>) -> PlanOut {
    let calls = vec![ToolCall::KeepOnly { ranges }];
    let compiled: Compiled = compile(project, inputs, &calls);
    let (reply, note) = match &compiled.proposal {
        Some(p) => (
            KEEP_ONLY_READY,
            format!("Proposed keepOnly ({}).", seconds(p.removed_us)),
        ),
        None => (KEEP_ONLY_MOVED, "Not done: keepOnly.".to_owned()),
    };
    PlanOut {
        reply: reply.to_owned(),
        compiled,
        usage_usd: 0.0,
        note,
        calls,
        answers: Vec::new(),
    }
}

/// A refusal about a clip that isn't analyzed yet names it: "Analyze this clip first (interview.mov)."
fn name_clips(out: &mut PlanOut, project: &Project, inputs: &Inputs) {
    for r in &out.compiled.refusals {
        if r.audience != Audience::Person {
            continue;
        }
        let Some(short) = &r.clip else { continue };
        let name = (inputs.refs.clip(short))
            .and_then(|id| project.timeline.main.iter().find(|c| c.id == id))
            .and_then(|clip| match &clip.source {
                ClipSource::Asset { id } => project.asset(id).map(|a| a.name.clone()),
                ClipSource::Recording { .. } => None,
            })
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| short.clone());
        let reason = r.reason.trim().trim_end_matches('.');
        let said = format!("{reason}.");
        let named = format!("{reason} ({}).", name.trim());
        if out.reply.contains(&said) && !out.reply.contains(&named) {
            out.reply = out.reply.replacen(&said, &named, 1);
        }
    }
}
