//! One chat per app: a message in, the model's answer applied, proposed and remembered.
//!
//! - **One message at a time.** A message waits for the one before it to land before anything is read (`queue`), so
//!   its context and its compiler inputs are built from the project as it is then (`inputs::gather`), and its calls
//!   compile against that same snapshot.
//! - **Times the request names** (the selection, "Keep only these" ranges) are on the revision the page made it on
//!   (`ChatRequest::base_revision`): a project that moved on since refuses the request with `project.moved` before
//!   anything is compiled. A message that names no time is read on the project as it is when its turn comes.
//! - **Additions** apply at once as one undo step (`origin: Chat`), on the revision the message was sent on; the reply
//!   carries what changed. **Removals** are a proposal, kept by id with the revision it applies to (the one after the
//!   additions). One proposal is open at a time: a newer one replaces it, so red always means "will be cut". Apply
//!   records the project its review made (`commit_reviewed`): one undo step. A proposal replaced, or one whose project
//!   moved on from its revision (a hand edit, an undo), or whose words were timed again since (Captions' "Time the
//!   words again": new words at new times, the revision the same), refuses Apply with `project.moved` and is dropped.
//!   Discard changes nothing. The model's `undo` takes back the top step, on the revision it saw.
//! - **The project moved while the model answered** (a hand edit meanwhile): nothing of the answer is applied or
//!   proposed, and the reply says so.
//! - **Errors** never touch the project: each is one plain sentence (`ChatFailure`). No key or code is `NeedsKey`; a
//!   code whose device couldn't register says why with Retry; refused credentials are renewed once when the adapter can
//!   (a relay's device token) and the message sent again.
//! - **The conversation** keeps the last `KEPT_TURNS` exchanges with their notes (what was applied or proposed, and
//!   later whether the person applied or discarded it), for the project open; another project, or New chat, starts it
//!   over.
//! - **Jobs:** following a subject and blurring aren't in Klipolo yet: the reply says they are coming (`jobs`). A
//!   picture (a sticker, a title card's background) is named in the reply, drawn when the page asks, and placed on Use
//!   as one undo step (`pictures`).
//! - **Undo chips** name the revision their change left (`undo_last`): one takes its change back only while nothing
//!   came after it.
//! - **Spend:** every model call's cost goes into the session's AI spend (`SpendCounter`); nothing is shown per
//!   message.
//! - **Other apps** (MCP) share the one proposal slot (`slot`, `SlotGuard`): an app's proposal waits there as chat's
//!   do, Apply and Discard take it by id the same way, and what became of it (applied, discarded, or dropped and why)
//!   is kept for the app. Every write to the slot takes its lock first, before the project's: Apply holds it until its
//!   commit lands, so a proposal being applied still waits for every app. Nothing takes it while holding the project's
//!   lock (a project switch tells `project_changed` under that lock: a proposal left from the old project is dropped
//!   the next time the slot is read, `SlotGuard::settle`) or chat's conversation lock.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use klipolo_chat::{
    Audience, ChatErrorKind, Compiled, Inputs, PlanOut, Proposal, ToolCall, Turn, View, compile,
    plan,
};
use klipolo_model::coded::CodedError;
use klipolo_model::history::SnapshotPatch;
use klipolo_model::risk::Review;
use klipolo_model::speech::{Speech, Transcript};
use klipolo_model::time::{RangeUs, Us};
use klipolo_model::timeline::ClipSource;
use klipolo_model::{Project, Transaction};

use super::describe::{describe, seconds};
use super::inputs::{gather, inputs_for};
use super::jobs;
use super::pictures::{ChatPicture, ChatPictureDrawn, PictureAsk, Pictures};
use super::project::{At, CommitError, ProjectNow, Projects};
use super::reply::{
    ChatApplied, ChatFailure, ChatFailureKind, ChatOutcome, ChatProposal, ChatReply, ChatRequest,
};
use super::sources::{Findings, Models, Readiness};
use super::spend::{ChatSpend, SpendCounter};

/// The exchanges kept for the model: the planner's own `KEPT_TURNS`.
pub const KEPT_EXCHANGES: usize = klipolo_chat::KEPT_TURNS;

/// Said when no project is open.
pub const NO_PROJECT: &str = "Open a project first.";
/// Said when another project opened while the message waited its turn.
pub const OTHER_PROJECT: &str = "Another project opened, so I didn't send that.";
/// Said for a message with no words.
pub const EMPTY: &str = "Type what you'd like to change first.";
/// Said for a project from a newer Klipolo.
pub const READ_ONLY: &str = "This project is from a newer Klipolo, so chat can't change it.";
/// Said with no key and no code (the panel shows the NeedsKey card).
pub const NEEDS_KEY: &str = "Add your Meta API key or an access code in Settings to chat.";
/// Said when the project changed while the model answered: nothing of the answer was applied or proposed.
pub const MOVED: &str =
    "The project changed while I was answering, so I left it as it was. Ask me again.";
/// Said when the answer's changes couldn't be applied (they don't fit the project any more).
pub const NOT_APPLIED: &str =
    "I couldn't make those changes, so I left the project as it was. Ask me again.";
/// Said beside the additions applied when the removals of the same answer couldn't be proposed.
pub const NOT_PROPOSED: &str = "I couldn't get the removals ready, so I left them out.";
/// Said beside the additions applied when the project changed before the removals could be proposed.
pub const REMOVALS_MOVED: &str =
    "The project changed meanwhile, so I left the removals out. Ask me again.";
/// Said beside the model's undo when its answer also found moments: they were found before the undo.
pub const MOMENTS_UNDONE: &str =
    "The undo changed the video, so I left out the moments I found. Ask me again to find them.";
/// Said beside what landed when the project changed before the moments found could be shown.
pub const MOMENTS_MOVED: &str =
    "The project changed meanwhile, so I left out the moments I found. Ask me again.";
/// The reply to "Keep only these" when the moments still stand.
pub const KEEP_ONLY_READY: &str = "Here is what I would remove. Apply it or discard it.";
/// The reply to "Keep only these" when the moments no longer match the video.
pub const KEEP_ONLY_MOVED: &str =
    "Those moments don't match the video any more. Ask me to find them again.";

/// What a message's turn note says when nothing of it landed.
const MOVED_NOTE: &str = "Nothing was applied: the person changed the project meanwhile.";
const NOT_APPLIED_NOTE: &str = "Nothing was applied: the changes no longer fit the project.";
const APPLIED_NOTE: &str = "The person applied the proposal.";
const DISCARDED_NOTE: &str = "The person discarded the proposal.";
const REPLACED_NOTE: &str = "Replaced by a newer proposal.";
const DROPPED_NOTE: &str =
    "The proposal was dropped: the project changed before the person applied it.";
const RETIMED_NOTE: &str =
    "The proposal was dropped: the clip's words were timed again before the person applied it.";

/// The app's chat.
pub struct ChatSession {
    projects: Arc<dyn Projects>,
    findings: Arc<dyn Findings>,
    models: Arc<dyn Models>,
    /// One message at a time: held from before its context is built until its edits have landed.
    queue: tokio::sync::Mutex<()>,
    /// The one proposal waiting for Apply or Discard, chat's or another app's. Taken before the project's lock and
    /// `talk`, never while holding either.
    slot: Mutex<Slot>,
    talk: Mutex<Talk>,
    pictures: Pictures,
    spend: SpendCounter,
}

/// The conversation with the open project.
#[derive(Default)]
struct Talk {
    /// Whose conversation it is: the project and its opening (the revision isn't part of it).
    owner: Option<At>,
    /// Oldest first, at most `KEPT_EXCHANGES`.
    exchanges: Vec<Exchange>,
    /// Where the project stood right after chat's last change: `undo_last` takes it back only from there.
    last_change: Option<At>,
}

/// The chat panel's one proposal slot, which other apps share.
#[derive(Default)]
struct Slot {
    open: Option<Stored>,
    /// What became of each app's latest proposal once it no longer waits, by app: its id, and what.
    decided: HashMap<String, (String, Decided)>,
}

impl Slot {
    /// `stored` no longer waits: what became of it is kept for the app that made it.
    fn decide(&mut self, stored: &Stored, what: Decided) {
        if let Some(app) = &stored.from {
            self.decided.insert(app.clone(), (stored.id.clone(), what));
        }
    }
}

/// What became of an app's proposal once it no longer waits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decided {
    /// The person applied it: one undo step of theirs.
    Applied,
    /// The person discarded it.
    Discarded,
    /// It went before the person decided.
    Dropped(Gone),
}

/// Why a waiting proposal went before the person decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gone {
    /// The project changed under it: an edit, an undo, a change chat or an app made.
    Changed,
    /// The clip's words were timed again since (Captions).
    Retimed,
    /// Klipolo's chat made a newer proposal in its place.
    Replaced,
    /// The person started a new chat.
    NewChat,
    /// Another project opened, or the project closed.
    Closed,
}

/// The proposal waiting for the person, as another app reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct Waiting {
    pub id: String,
    /// Where the project stood when it was made: it applies only there.
    pub at: At,
    /// The app that made it; None for Klipolo's chat.
    pub from: Option<String>,
    /// Its card's title.
    pub title: String,
    /// How much footage it takes away in all, and where, on the timeline as it is.
    pub removed_us: Us,
    pub ranges: Vec<RangeUs>,
}

/// The proposal slot, held (`ChatSession::slot`): other apps read it, and put their proposals in it, through this.
pub struct SlotGuard<'a> {
    slot: MutexGuard<'a, Slot>,
    chat: &'a ChatSession,
}

impl SlotGuard<'_> {
    /// Drops the waiting proposal once the project moved on from where it was made (an edit, an undo, another project):
    /// it can never apply. Its id, when one went.
    pub fn settle(&mut self) -> Option<String> {
        let now = self.chat.projects.now().map(|n| n.at);
        let open = self.slot.open.as_ref()?;
        if now.as_ref() == Some(&open.at) {
            return None;
        }
        let old = self.slot.open.take()?;
        self.slot
            .decide(&old, Decided::Dropped(gone(&old.at, now.as_ref())));
        if old.from.is_none() {
            lock(&self.chat.talk).noted(&old.id, DROPPED_NOTE);
        }
        Some(old.id)
    }

    /// The proposal waiting for the person, once one that can no longer apply is dropped (`settle`).
    pub fn waiting(&mut self) -> Option<Waiting> {
        self.settle();
        self.slot.open.as_ref().map(Stored::waiting)
    }

    /// Puts app `client`'s proposal in the chat panel: `proposal`, compiled from `tools`' calls on the project `now`
    /// (the open project as it stands, under this guard), in place of the one waiting, which must be the app's own (the
    /// caller checks). The card the person sees.
    pub fn propose(
        &mut self,
        client: &str,
        now: &ProjectNow,
        proposal: &Proposal,
        tools: &[&str],
    ) -> ChatProposal {
        let chat = self.chat;
        let (inputs, _) = inputs_for(
            &now.project,
            &*chat.findings,
            "",
            View::default(),
            Vec::new(),
        );
        let id = uuid::Uuid::new_v4().to_string();
        let made = card(
            &id,
            now.at.revision,
            &now.project,
            &inputs.speech,
            &proposal.review,
            tools,
            Some(client),
        );
        self.slot.open = Some(Stored::new(
            &made,
            now.at.clone(),
            &proposal.tx,
            &proposal.review,
            Words::read(&inputs),
            Some(client.to_owned()),
        ));
        // A new one waits: what became of its earlier one no longer answers the app.
        self.slot.decided.remove(client);
        made
    }

    /// What became of `client`'s latest proposal, once it no longer waits.
    pub fn decided(&self, client: &str) -> Option<(String, Decided)> {
        self.slot.decided.get(client).cloned()
    }
}

/// Why a proposal made at `made` can't apply with the project at `now`.
fn gone(made: &At, now: Option<&At>) -> Gone {
    if now.is_some_and(|now| now.same_project(made)) {
        Gone::Changed
    } else {
        Gone::Closed
    }
}

/// The card for removals `review` made on `base` (the project they apply to, its words in `speech`) by `tools`' calls:
/// Klipolo's title for them and a tag over each cut, for proposal `id` on `revision`. Chat's proposals and other apps'
/// are worded alike, but that an app's keepOnly keeps parts it suggests (`from`: the app; None for chat).
fn card(
    id: &str,
    revision: u64,
    base: &Project,
    speech: &Speech,
    review: &Review,
    tools: &[&str],
    from: Option<&str>,
) -> ChatProposal {
    let removed_us = review.removals.ranges.iter().map(RangeUs::len).sum();
    let said = describe(base, speech, &review.removals, tools, from);
    ChatProposal::new(
        id,
        revision,
        &said.title,
        review.removals.clone(),
        removed_us,
        said.tags,
    )
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
    /// The words it was made on (`Words::read`).
    words: Words,
    /// The app that made it; None for chat's own.
    from: Option<String>,
    /// Its card's title, the footage it takes away and where.
    title: String,
    removed_us: Us,
    ranges: Vec<RangeUs>,
}

impl Stored {
    /// The proposal `card` shows, made at `at` as `tx` judged by `review`, on `words`, by `from`.
    fn new(
        card: &ChatProposal,
        at: At,
        tx: &Transaction,
        review: &Review,
        words: Words,
        from: Option<String>,
    ) -> Self {
        Self {
            id: card.id().to_owned(),
            at,
            tx: tx.clone(),
            after: review.after.clone(),
            words,
            from,
            title: card.title().to_owned(),
            removed_us: review.removals.ranges.iter().map(RangeUs::len).sum(),
            ranges: review.removals.ranges.clone(),
        }
    }

    fn waiting(&self) -> Waiting {
        Waiting {
            id: self.id.clone(),
            at: self.at.clone(),
            from: self.from.clone(),
            title: self.title.clone(),
            removed_us: self.removed_us,
            ranges: self.ranges.clone(),
        }
    }
}

/// Each file's transcript as a message read it, by asset id (None: it had none yet). Words timed again since are a new
/// transcript (`Findings::found`), even when the revision stayed: a proposal made on the old ones would cut where they
/// were.
struct Words(HashMap<String, Option<Arc<Transcript>>>);

impl Words {
    fn read(inputs: &Inputs) -> Self {
        Self(
            (inputs.analysis.keys())
                .map(|id| (id.clone(), inputs.speech.transcripts.get(id).cloned()))
                .collect(),
        )
    }

    /// Whether every file's words are still the ones read.
    fn still(&self, findings: &dyn Findings) -> bool {
        self.0.iter().all(|(id, read)| {
            let now = findings.found(id).and_then(|f| f.transcript.clone());
            match (read, &now) {
                (Some(read), Some(now)) => Arc::ptr_eq(read, now),
                (None, None) => true,
                _ => false,
            }
        })
    }
}

/// What came of committing a message's edits.
struct Landed {
    /// Where the project stands after them (where it stood, when none landed).
    at: At,
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
            slot: Mutex::default(),
            talk: Mutex::default(),
            pictures: Pictures::default(),
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
    /// project untouched. A message waits for the one before it. A request whose selection or "Keep only these" ranges
    /// were made on a revision the project left is refused with `CommitError::Moved`.
    pub async fn send(&self, request: ChatRequest) -> Result<ChatOutcome, CommitError> {
        let text = request.text.trim().to_owned();
        // Which project the person sent it to, before it waits its turn.
        let asked = self.projects.now().map(|p| p.at);
        let _turn = self.queue.lock().await;
        let Some(now) = self.projects.now() else {
            return Ok(failed(ChatFailureKind::Other, NO_PROJECT));
        };
        if asked.is_some_and(|a| !a.same_project(&now.at)) {
            return Ok(failed(ChatFailureKind::Other, OTHER_PROJECT));
        }
        self.follow(&now.at);
        // "This" and the moments are times on the page's copy: stale once the project moved on.
        let names_times = request.selection.is_some() || !request.keep_only.is_empty();
        if names_times && request.base_revision != now.at.revision {
            return Err(CommitError::Moved);
        }
        if now.read_only {
            return Ok(failed(ChatFailureKind::Other, READ_ONLY));
        }
        if text.is_empty() && request.keep_only.is_empty() {
            return Ok(failed(ChatFailureKind::Other, EMPTY));
        }
        let view = View {
            playhead_us: request.playhead_us,
            selection: request.selection,
        };
        if !request.keep_only.is_empty() {
            let (_, inputs) = gather(
                &now.project,
                &*self.findings,
                &text,
                view,
                self.projects.pack(),
            );
            let out = keep_only(&now.project, &inputs, request.keep_only.clone());
            return Ok(self.land(&request.id, &text, &now, &inputs, out));
        }

        let model = match self.models.ready().await {
            Readiness::Ready { model } => model,
            Readiness::Problem(problem) => {
                return Ok(failed(ChatFailureKind::Registration, &problem));
            }
            Readiness::NeedsKey => return Ok(failed(ChatFailureKind::NeedsKey, NEEDS_KEY)),
        };
        let (context, inputs) = gather(
            &now.project,
            &*self.findings,
            &text,
            view,
            self.projects.pack(),
        );
        let history = self.turns();
        let ask = |model: Arc<dyn klipolo_chat::LanguageModel>| {
            let (context, inputs, history, text, project) =
                (&context, &inputs, &history, &text, &now.project);
            async move { plan(&*model, history, context, text, inputs, project).await }
        };
        let mut planned = ask(model).await;
        // Credentials refused: renewed once when they can be (the adapter knows), and the message sent again.
        if let Err(refused) = &planned
            && refused.kind == ChatErrorKind::Refused
            && self.models.recover().await
            && let Readiness::Ready { model } = self.models.ready().await
        {
            // Its cost counts; the message counts once, with the try that answers.
            self.spend.message(refused.usage_usd, 0);
            planned = ask(model).await;
        }
        Ok(match planned {
            Err(error) => {
                self.spend.message(error.usage_usd, error.model_calls);
                tracing::info!(kind = ?error.kind, detail = %error.detail, "chat: the message failed");
                ChatOutcome::Failed(ChatFailure::from(&error))
            }
            Ok(out) => {
                self.spend.message(out.usage_usd, out.answers.len());
                self.land(&request.id, &text, &now, &inputs, out)
            }
        })
    }

    /// Applies proposal `id` (chat's, or another app's) as one undo step of the person's. Refused with `project.moved`
    /// (and dropped) when the project moved on from the revision it was made for, or when it is gone (replaced,
    /// discarded, dropped, another project). Refused for any other reason (a read-only project, a disk that refused
    /// it), it stays open for Retry. The slot is held until the commit lands: while it is applied, it still waits.
    pub fn apply(&self, id: &str) -> Result<ChatApplied, CommitError> {
        let mut slot = lock(&self.slot);
        let Some(stored) = slot.open.take_if(|p| p.id == id) else {
            return Err(CommitError::Moved);
        };
        if !stored.words.still(&*self.findings) {
            slot.decide(&stored, Decided::Dropped(Gone::Retimed));
            lock(&self.talk).noted(id, RETIMED_NOTE);
            return Err(CommitError::Moved);
        }
        let committed = (self.projects).commit(&stored.at, &stored.tx, Some(stored.after.clone()));
        // Why it can't apply, read before the conversation's lock is taken.
        let why = matches!(committed, Err(CommitError::Moved))
            .then(|| gone(&stored.at, self.projects.now().map(|n| n.at).as_ref()));
        let mut talk = lock(&self.talk);
        match committed {
            Ok(c) => {
                talk.last_change = Some(c.at.clone());
                talk.noted(id, APPLIED_NOTE);
                slot.decide(&stored, Decided::Applied);
                Ok(ChatApplied {
                    change: c.patch,
                    undo: undo_label(&stored.tx),
                })
            }
            Err(CommitError::Moved) => {
                talk.noted(id, DROPPED_NOTE);
                slot.decide(&stored, Decided::Dropped(why.unwrap_or(Gone::Changed)));
                Err(CommitError::Moved)
            }
            // Still open: the slot was held, so nothing newer came meanwhile.
            Err(e) => {
                slot.open = Some(stored);
                Err(e)
            }
        }
    }

    /// Drops proposal `id`: nothing changes.
    pub fn discard(&self, id: &str) {
        let mut slot = lock(&self.slot);
        if let Some(stored) = slot.open.take_if(|p| p.id == id) {
            slot.decide(&stored, Decided::Discarded);
            lock(&self.talk).noted(id, DISCARDED_NOTE);
        }
    }

    /// The proposal slot, held, for another app (MCP): what waits, and the app's own proposals. Every write to it takes
    /// this lock first, before the project's; never take it while holding the project's lock.
    pub fn slot(&self) -> SlotGuard<'_> {
        SlotGuard {
            slot: lock(&self.slot),
            chat: self,
        }
    }

    /// The Undo chip under chat's change that left the project at `revision`: takes it back while it is still the last
    /// change; else `project.moved` (another change came after it, chat's or the person's).
    pub fn undo_last(&self, revision: u64) -> Result<SnapshotPatch, CommitError> {
        let last = lock(&self.talk).last_change.clone();
        let last = (last.filter(|l| l.revision == revision)).ok_or(CommitError::Moved)?;
        let committed = self.projects.undo(&last)?;
        lock(&self.talk).last_change = None;
        Ok(committed.patch)
    }

    /// New chat: the conversation, its proposal (another app's too: it shows in the panel), its last change and its
    /// pictures are forgotten.
    pub fn reset(&self) {
        let mut slot = lock(&self.slot);
        if let Some(old) = slot.open.take() {
            slot.decide(&old, Decided::Dropped(Gone::NewChat));
        }
        drop(slot);
        let mut talk = lock(&self.talk);
        talk.exchanges.clear();
        talk.last_change = None;
        self.pictures.clear();
    }

    /// Another project opened, or the project closed: everything about the old one is forgotten. Called under the
    /// project's lock, so the slot isn't taken here: a proposal left from the old project is dropped the next time the
    /// slot is read (`SlotGuard::settle`), and can never apply meanwhile (it names the old project).
    pub fn project_changed(&self) {
        *lock(&self.talk) = Talk::default();
        self.pictures.clear();
    }

    /// A picture for the open project, waiting to be drawn: what its card says. The compiler's picture jobs come in
    /// here (`jobs`).
    pub fn ask_picture(&self, ask: PictureAsk) -> ChatPicture {
        self.pictures.ask(self.projects.now().map(|n| n.at), ask)
    }

    /// Draws picture `id` (again, for Try again), for its card: about 35 s. A sentence when it can't be drawn.
    pub async fn draw_picture(&self, id: &str) -> Result<ChatPictureDrawn, CommitError> {
        (self.pictures)
            .draw(id, &*self.projects, &*self.models, &self.spend)
            .await
    }

    /// Use: places picture `id`'s drawing at `drawing` (its path: the one the card shows, though a newer one may have
    /// landed) in the open project as one undo step. A sentence when it can't be placed (its moment or its title is
    /// gone, or that drawing isn't there).
    pub fn place_picture(&self, id: &str, drawing: &str) -> Result<ChatApplied, CommitError> {
        let (applied, at) = self.pictures.place(id, drawing, &*self.projects)?;
        lock(&self.talk).last_change = Some(at);
        Ok(applied)
    }

    /// The conversation belongs to the project at `at`: one about another project (or another opening) is forgotten.
    fn follow(&self, at: &At) {
        let mut talk = lock(&self.talk);
        if !talk.owner.as_ref().is_some_and(|o| o.same_project(at)) {
            *talk = Talk {
                owner: Some(at.clone()),
                ..Talk::default()
            };
            self.pictures.clear();
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
        let mut landed = self.commit(now, inputs, &out);
        let mut jobs = jobs::start(&out.compiled);
        name_clips(&mut out, &now.project, inputs);
        // The moments are times on the snapshot the answer was compiled on, the additions in: none survive the project
        // moving on (a hand edit, another project) while the answer came, nor the model's own undo, which they were
        // found before.
        let current = self.projects.now().map(|n| n.at);
        let same_project = current.as_ref().is_some_and(|c| c.same_project(&landed.at));
        let moved = current.as_ref() != Some(&landed.at);
        let committed =
            landed.undone.is_some() || landed.applied.is_some() || landed.proposal.is_some();
        // Another project opened (or this one closed) while the answer came, before any of it landed: nothing of it is
        // for the project open now. What did land, landed on the old project, and the reply says so.
        if !same_project && landed.lost.is_none() && !committed {
            landed.lost = Some(MOVED);
        }
        let mut notes: Vec<String> = Vec::new();
        let mut moments_note = None;
        let stale = landed.lost.is_some() || moved || landed.undone.is_some();
        if !out.compiled.moments.is_empty() && stale {
            out.compiled.moments.clear();
            let rest = committed
                || !out.compiled.questions.is_empty()
                || !jobs.pictures.is_empty()
                || !jobs.coming.is_empty();
            if landed.lost.is_none() && !rest {
                landed.lost = Some(MOVED);
            } else if landed.lost.is_none() {
                let (said, why) = if landed.undone.is_some() {
                    (MOMENTS_UNDONE, "the undo moved them")
                } else {
                    (MOMENTS_MOVED, "the person changed the project meanwhile")
                };
                notes.push(said.to_owned());
                moments_note = Some(format!("The moments found were left out: {why}."));
            }
        }

        let something = landed.undone.is_some()
            || landed.applied.is_some()
            || landed.proposal.is_some()
            || !out.compiled.moments.is_empty()
            || !out.compiled.questions.is_empty();
        let mut note = out.note.clone();
        if let Some(left_out) = moments_note {
            note = format!("{note} {left_out}");
        }
        if let Some(dropped) = landed.dropped {
            notes.push(dropped.to_owned());
            note = format!(
                "{} The removals were not proposed.",
                without_proposed(&note)
            )
            .trim()
            .to_owned();
        }
        if let Some(lost) = landed.lost {
            // Nothing of the answer landed: its words, and the note, may claim it did.
            out.reply = lost.to_owned();
            note = (if lost == MOVED {
                MOVED_NOTE
            } else {
                NOT_APPLIED_NOTE
            })
            .to_owned();
        } else if !jobs.coming.is_empty() {
            if something {
                notes.extend(jobs.coming.iter().map(|s| (*s).to_owned()));
            } else {
                out.reply = jobs.coming.join(" ");
            }
            note = format!("{note} {}", jobs.note());
        }
        let mut reply = ChatReply::from_plan(id, &out).on_revision(landed.at.revision);
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
        // Pictures are asked for once the edits are in (a title card's title is one of them), for the project the
        // message was sent to.
        if landed.lost.is_none() && same_project {
            for ask in jobs.pictures.drain(..) {
                reply = reply.picture(self.pictures.ask(Some(now.at.clone()), ask));
            }
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
        let mut landed = Landed {
            at: now.at.clone(),
            undone: None,
            applied: None,
            proposal: None,
            lost: None,
            dropped: None,
        };
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
                    landed.at = at.clone();
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
                    landed.at = at.clone();
                    lock(&self.talk).last_change = Some(at.clone());
                }
                Err(e) => {
                    landed.lost = Some(lost(e));
                    return landed;
                }
            }
        }
        // Held from the check that the project still stands where the answer left it until its proposal is kept, so no
        // app's change lands between.
        let mut slot = lock(&self.slot);
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
                    let id = uuid::Uuid::new_v4().to_string();
                    let made = card(
                        &id,
                        at.revision,
                        &base,
                        &inputs.speech,
                        &review,
                        &tools,
                        None,
                    );
                    let stored =
                        Stored::new(&made, at.clone(), &p.tx, &review, Words::read(inputs), None);
                    landed.proposal = Some((id, made));
                    // A newer proposal replaces the one waiting, so red always means "will be cut".
                    if let Some(old) = slot.open.replace(stored) {
                        slot.decide(&old, Decided::Dropped(Gone::Replaced));
                        if old.from.is_none() {
                            lock(&self.talk).noted(&old.id, REPLACED_NOTE);
                        }
                    }
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
        // A proposal made on a revision the project left can never apply: dropped, and its turn says so.
        if let Some(old) = slot.open.take_if(|p| p.at != at) {
            slot.decide(&old, Decided::Dropped(gone(&old.at, Some(&at))));
            if old.from.is_none() {
                lock(&self.talk).noted(&old.id, DROPPED_NOTE);
            }
        }
        drop(slot);
        landed
    }
}

impl Talk {
    fn remember(&mut self, exchange: Exchange) {
        self.exchanges.push(exchange);
        let over = self.exchanges.len().saturating_sub(KEPT_EXCHANGES);
        self.exchanges.drain(..over);
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

/// A turn's note without what it says was proposed ("Proposed cutRange (10.0 s)."): the proposal wasn't made.
fn without_proposed(note: &str) -> String {
    let Some(at) = note.find("Proposed ") else {
        return note.to_owned();
    };
    let end = note[at..]
        .find(" s).")
        .map_or(note.len(), |e| at + e + " s).".len());
    format!("{} {}", note[..at].trim(), note[end..].trim())
        .trim()
        .to_owned()
}

/// A refusal about one clip (not analyzed yet, its fillers roughly timed) names it after "this clip": "Analyze this clip
/// (interview.mov) first."
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
        let said = format!("{}.", r.reason.trim().trim_end_matches('.'));
        let named = named_in(&said, name.trim());
        if out.reply.contains(&said) && !out.reply.contains(&named) {
            out.reply = out.reply.replacen(&said, &named, 1);
        }
    }
}

/// `said` with the clip's `name` after "this clip", or before its last full stop when it doesn't say "this clip".
fn named_in(said: &str, name: &str) -> String {
    const THIS: &str = "this clip";
    match said.find(THIS) {
        Some(at) => {
            let (before, after) = said.split_at(at + THIS.len());
            format!("{before} ({name}){after}")
        }
        None => format!("{} ({name}).", said.trim_end_matches('.')),
    }
}

#[cfg(test)]
mod tests {
    use super::{named_in, without_proposed};

    #[test]
    fn a_clip_is_named_after_this_clip() {
        assert_eq!(
            named_in("Analyze this clip first.", "talk.mov"),
            "Analyze this clip (talk.mov) first."
        );
        assert_eq!(
            named_in(
                "Klipolo is still watching this clip. Ask me again when its analysis is done.",
                "talk.mov"
            ),
            "Klipolo is still watching this clip (talk.mov). Ask me again when its analysis is done."
        );
        assert_eq!(named_in("Not yet.", "talk.mov"), "Not yet (talk.mov).");
    }

    #[test]
    fn a_note_drops_what_it_says_was_proposed() {
        assert_eq!(
            without_proposed(
                "Applied setFormat. Proposed removeFillers, cutRange (1.0 s). Asked: Which part?"
            ),
            "Applied setFormat. Asked: Which part?"
        );
        assert_eq!(without_proposed("Proposed cutRange (6.0 s)."), "");
        assert_eq!(without_proposed("Applied setFormat."), "Applied setFormat.");
    }
}
