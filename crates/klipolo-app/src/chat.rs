//! What one chat message comes to, as the chat panel shows it: the shapes `chat_send` answers with (the commands come
//! with the planner). Every text the model wrote goes through `klipolo_chat::sanitize::reply` here, as the reply is
//! built: its fields are set only by these constructors, so the page shows them as they are and never cleans them
//! itself. Klipolo's own sentences (a proposal's title, a failure's plain reason) are written without dashes.

use klipolo_chat::{ChatError, ChatErrorKind, PlanOut, sanitize};
use klipolo_model::coded::CodedError;
use klipolo_model::history::SnapshotPatch;
use klipolo_model::project::FormatKind;
use klipolo_model::risk::Removals;
use klipolo_model::time::{RangeUs, Us};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// One message from the person, with what "this" and "here" mean when it is sent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ChatRequest {
    /// Made by the page: the reply carries it back.
    pub id: String,
    pub text: String,
    /// The time range selected on the timeline ("this"), if any.
    pub selection: Option<RangeUs>,
    /// The overlay selected (a title, an image), if any.
    pub item: Option<String>,
    pub playhead_us: Us,
    /// The format the preview shows.
    pub format: Option<FormatKind>,
    /// "Keep only these" under moments: keep only these timeline ranges and remove the rest, proposed without asking
    /// the model (they are Klipolo's own moments). Empty for a message the person typed.
    #[serde(default)]
    pub keep_only: Vec<RangeUs>,
    /// The revision of the page's copy the request was made on. The selection and `keep_only` are times on it: when the
    /// project moved on since, a request with either is refused with `project.moved` before anything is compiled.
    #[ts(type = "number")]
    pub base_revision: u64,
}

/// What a message came to: a reply, or why nothing happened (the project untouched).
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum ChatOutcome {
    Reply(Box<ChatReply>),
    Failed(ChatFailure),
}

/// Klipolo's reply: its words, then what was done (additions applied at once, one undo step), what waits for the person
/// (a proposal: removals), and what it points at (moments).
#[derive(Clone, Debug, Default, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ChatReply {
    id: String,
    /// One or two plain sentences.
    text: String,
    /// What couldn't be done, one sentence each.
    notes: Vec<String>,
    /// What Klipolo asks back.
    questions: Vec<String>,
    applied: Option<ChatApplied>,
    /// The model took back the last change: what that undo changed, for the page to merge.
    undone: Option<SnapshotPatch>,
    proposal: Option<ChatProposal>,
    moments: Vec<ChatMoment>,
    /// The revision after its commits (what it applied, what the model's undo changed): the one its moments' times
    /// are on, which "Keep only these" sends back.
    #[ts(type = "number")]
    revision: u64,
}

/// Additions applied at once, as one undo step: what changed, and its undo label.
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ChatApplied {
    pub change: SnapshotPatch,
    pub undo: CodedError,
}

/// Removals waiting for the person: red on the timeline and a card in chat. Apply is one undo step.
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ChatProposal {
    id: String,
    /// The revision it was made on: a change made since (a hand edit) drops it.
    #[ts(type = "number")]
    base_revision: u64,
    /// Klipolo's words for it ("Cut 14 pauses and 9 ums").
    title: String,
    /// What it takes away, on the timeline as it is now: footage (`ranges`), and anything else (a format dropped with
    /// what was placed in it).
    removals: Removals,
    /// How much footage goes in all.
    removed_us: Us,
    /// A short reason over each cut ("um", "pause").
    tags: Vec<ProposalTag>,
}

/// A short reason shown on a proposed cut, over the time it covers.
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProposalTag {
    start_us: Us,
    end_us: Us,
    label: String,
}

/// A moment Klipolo points at, in timeline time, with a few words on what happens.
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ChatMoment {
    start_us: Us,
    end_us: Us,
    label: String,
}

/// Why a message came to nothing.
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ChatFailure {
    kind: ChatFailureKind,
    /// One plain sentence: what happened and what to do.
    message: String,
}

/// What kind of failure, so the panel offers the right way out: Retry, or Open Settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ChatFailureKind {
    /// The project is too long for chat; nothing was sent.
    TooLong,
    /// Not enough credit left (Open Settings).
    Budget,
    Busy,
    Unavailable,
    Network,
    /// The key or the code was not accepted (Open Settings).
    Refused,
    /// Turned down under the provider's content policy: the sentence says so, and asking in other words may work, so
    /// the panel offers no Retry of the same text.
    Declined,
    /// No key and no code (the NeedsKey card).
    NeedsKey,
    /// With an access code, this Mac couldn't register with the relay: the sentence says why (Retry).
    Registration,
    Other,
}

impl ChatReply {
    /// A reply to request `id` with the model's `text`, cleaned.
    pub fn new(id: impl Into<String>, text: &str) -> Self {
        Self {
            id: id.into(),
            text: sanitize::reply(text.trim()),
            ..Self::default()
        }
    }

    /// Something that couldn't be done, cleaned.
    pub fn note(mut self, note: &str) -> Self {
        self.notes.push(sanitize::reply(note.trim()));
        self
    }

    /// A question asked back, cleaned.
    pub fn question(mut self, question: &str) -> Self {
        self.questions.push(sanitize::reply(question.trim()));
        self
    }

    /// A moment pointed at, its words cleaned.
    pub fn moment(mut self, range: RangeUs, label: &str) -> Self {
        self.moments.push(ChatMoment {
            start_us: range.start,
            end_us: range.end,
            label: sanitize::reply(label.trim()),
        });
        self
    }

    pub fn applied(mut self, change: SnapshotPatch, undo: CodedError) -> Self {
        self.applied = Some(ChatApplied { change, undo });
        self
    }

    /// The revision the project is at once its commits are in.
    pub fn at(mut self, revision: u64) -> Self {
        self.revision = revision;
        self
    }

    pub fn undone(mut self, change: SnapshotPatch) -> Self {
        self.undone = Some(change);
        self
    }

    pub fn proposal(mut self, proposal: ChatProposal) -> Self {
        self.proposal = Some(proposal);
        self
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn notes(&self) -> &[String] {
        &self.notes
    }

    pub fn questions(&self) -> &[String] {
        &self.questions
    }

    pub fn moments(&self) -> &[ChatMoment] {
        &self.moments
    }

    pub fn proposal_ref(&self) -> Option<&ChatProposal> {
        self.proposal.as_ref()
    }
}

impl ChatReply {
    /// The reply to request `id` from what the planner made of it: its words (cleaned again here), the questions it asked
    /// that its words don't already say, and the moments it points at. The additions applied and the proposal are the
    /// app's to add (`applied`, `proposal`), once it has applied or kept them.
    pub fn from_plan(id: impl Into<String>, plan: &PlanOut) -> Self {
        let mut reply = Self::new(id, &plan.reply);
        for question in &plan.compiled.questions {
            if !plan.reply.contains(question.trim()) {
                reply = reply.question(question);
            }
        }
        for moment in &plan.compiled.moments {
            reply = reply.moment(moment.range, &moment.label);
        }
        reply
    }
}

impl From<&ChatError> for ChatFailure {
    /// The planner's failure as the panel shows it: its sentence, and what way out it offers.
    fn from(error: &ChatError) -> Self {
        let kind = match error.kind {
            ChatErrorKind::TooLong => ChatFailureKind::TooLong,
            ChatErrorKind::Budget => ChatFailureKind::Budget,
            ChatErrorKind::Busy => ChatFailureKind::Busy,
            ChatErrorKind::Unavailable => ChatFailureKind::Unavailable,
            ChatErrorKind::Network => ChatFailureKind::Network,
            ChatErrorKind::Refused => ChatFailureKind::Refused,
            ChatErrorKind::Declined => ChatFailureKind::Declined,
            ChatErrorKind::Other => ChatFailureKind::Other,
        };
        Self::new(kind, &error.message)
    }
}

impl ChatProposal {
    /// A proposal `id` made on `base_revision`, titled `title`, taking away `removals` (`removed_us` of footage in
    /// all), with a tag over each cut. Tag labels are cleaned (one may be the model's short reason).
    pub fn new(
        id: impl Into<String>,
        base_revision: u64,
        title: &str,
        removals: Removals,
        removed_us: Us,
        tags: impl IntoIterator<Item = (RangeUs, String)>,
    ) -> Self {
        Self {
            id: id.into(),
            base_revision,
            title: sanitize::reply(title.trim()),
            removals,
            removed_us,
            tags: tags
                .into_iter()
                .map(|(range, label)| ProposalTag {
                    start_us: range.start,
                    end_us: range.end,
                    label: sanitize::reply(label.trim()),
                })
                .collect(),
        }
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn tag_labels(&self) -> impl Iterator<Item = &str> {
        self.tags.iter().map(|t| t.label.as_str())
    }
}

impl ChatMoment {
    pub fn label(&self) -> &str {
        &self.label
    }
}

impl ChatFailure {
    /// Why a message came to nothing, as one sentence (a relay's own sentence about the budget included), cleaned.
    pub fn new(kind: ChatFailureKind, message: &str) -> Self {
        Self {
            kind,
            message: sanitize::reply(message.trim()),
        }
    }

    pub fn kind(&self) -> ChatFailureKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}
