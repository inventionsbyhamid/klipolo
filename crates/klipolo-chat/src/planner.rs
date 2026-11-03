//! The planner: one chat message to the language model (`model::LanguageModel`), out as a reply for the
//! person and compiled edits (`compile`): additions applied at once, removals proposed.
//!
//! The request is the system prompt, the tools and the project context, in that order and the same bytes for every
//! message on one project revision, so a provider's prompt cache reuses them; then the conversation (`Turn`s, the last
//! `KEPT_TURNS`), then the new message last.
//!
//! The agent loop, at most `MAX_STEPS` model calls a message. Each answer with calls gets what came of each call back:
//! the whole input is sent again with nothing stored at the provider, then the answer's text and calls, then one output
//! per call (what a `findMoments` found, `DONE_OUTPUT` or `PROPOSED_OUTPUT`, the reason for one refused back to the
//! model: its arguments unreadable, or the compiler's `Audience::Model` refusal such as a time that stands on nothing;
//! `TOLD_PERSON` for one refused for the person, whose reason is never sent back). It goes on until an answer has no
//! call: those closing words are the reply. An answer with no call that says what it will do ("I will turn this into a
//! short") is told `GO_ON` instead. An answer that only repeats calls already made ends it too.
//!
//! The calls kept (all but those refused back, a repeat dropped) are compiled once, in the order made, against the same
//! snapshot and inputs, so the message makes one additions transaction and one proposal. A call refused back that no
//! later call of its tool made good (counted call by call, and no question or `cannotDo` instead) stays undone, and the
//! reply says so. When a model call after the first fails (the spend cap, the network), what was done stands and the
//! reply says it stopped (`STOPPED`).
//!
//! The reply is the model's closing words, then the sentences of refusals meant for the person that they don't say yet.
//! When the steps run out right after an answer with calls and the edits did something, the model's last words were
//! about what it was still doing: what the edits did is said instead (`summary`).
//! Without closing words (the steps ran out, a call failed, or they were empty), it is built from the model's last
//! text: that text, then Klipolo's own notes on what the edits did or left as they were (`Compiled::notes` and `waits`:
//! a speed that shortens a transition, or waits for the person as it leaves one no room, a camera that is its own
//! layer), then the sentences of refusals meant for the person, then what couldn't be done; all of it without em or en
//! dashes (`sanitize::reply`). When a call was refused for the person beside others that went through, the model's
//! text, written before that refusal, may claim it: it is left out, and what went through is said plainly instead ("The
//! music is in."). When nothing came of the message and a call was refused (still refused back, or refused for the
//! person, `cannotDo` aside), that text, which may claim the edit, is left out. With none of these, the reply is
//! `askUser`'s questions, else a short sentence on what was done (or, for only work handed to a runner, that it is
//! under way); otherwise the questions are only in `Compiled::questions`. A `followSubject` runs once the plan is made,
//! so its output says done before it has: should the follow come to nothing, the app says `PlanOut::without_follow`
//! instead of a reply that may claim it.
//!
//! An error before anything was done leaves the project untouched: nothing is compiled, and each is one plain
//! sentence (`ChatError`). A project whose context is over budget (`ChatContext::fits`) is refused before any model
//! call.

use std::fmt;

use klipolo_model::Project;
use klipolo_model::time::US_PER_SEC;
use serde::{Deserialize, Serialize};

use crate::compile::{Audience, Compiled, Inputs, compile};
use crate::context::ChatContext;
use crate::model::{Item, LanguageModel, ModelError, ModelRequest, ModelResponse};
use crate::prompt::system_prompt;
use crate::sanitize;
use crate::tools::{FunctionCall, ToolCall, tools};

/// The exchanges the conversation keeps.
pub const KEPT_TURNS: usize = 6;
/// The most model calls one message makes: each answer with calls gets what came of them and is asked again, until
/// one answers in words alone. A safety net: the prompt asks for calls that don't depend on each other in one answer,
/// and the spend cap still bounds what a message costs (a paid short took 8 steps, about $0.19; an answer cut off at
/// its output cap takes one more).
pub const MAX_STEPS: usize = 10;
/// Sent back when an answer says what it will do but calls no tool.
pub const GO_ON: &str = "Go on: make the calls now.";
/// Sent back after an answer cut off at its output cap: its finished calls are kept, and it is asked once more.
pub const CUT_OFF: &str =
    "Your last answer was cut off. Send fewer calls at a time; you can finish in the next step.";
/// Said after what was done when a model call failed before the model was done.
pub const STOPPED: &str = "I couldn't finish the rest of it.";

/// A call's output for the model: it is done (applied, started or asked).
pub const DONE_OUTPUT: &str = "done";
/// A call's output for the model: it is proposed, for the person to apply or discard.
pub const PROPOSED_OUTPUT: &str = "proposed";
/// A call's output for the model when it was refused for the person: the reason is theirs, not sent back.
pub const TOLD_PERSON: &str = "Not done. The person was told why.";

/// Said when the context is over budget.
pub const TOO_LONG: &str =
    "This project is too long for chat right now. Select a part and ask again.";
/// Said when the credit is used up and the provider gave no sentence of its own.
pub const OUT_OF_CREDIT: &str = "Your AI credit has run out.";
/// Said when the provider is busy, or doesn't answer, and gave no sentence of its own (who: Meta, Klipolo's server).
pub const BUSY: &str = "Klipolo AI is busy right now. Try again in a moment.";
pub const UNAVAILABLE: &str = "Klipolo AI isn't answering right now. Try again in a moment.";
pub const NO_CONNECTION: &str =
    "Couldn't reach Klipolo AI. Check your internet connection and try again.";
pub const NOT_ACCEPTED: &str =
    "Your API key or access code was not accepted. Check it in Settings.";
pub const FAILED: &str = "Something went wrong with that message. Try again.";
/// Said when the provider declined the request under its content policy and gave no sentence of its own.
pub const DECLINED: &str = "The AI turned this down. Try wording it differently.";

/// One exchange: what the person wrote, the reply, and a one-line note of what came of it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Turn {
    pub user: String,
    pub reply: String,
    /// What was applied, proposed, asked or not done (`PlanOut::note`).
    pub note: String,
}

/// Adds `turn` to `history`, keeping the last `KEPT_TURNS`.
pub fn remember(history: &mut Vec<Turn>, turn: Turn) {
    history.push(turn);
    let over = history.len().saturating_sub(KEPT_TURNS);
    history.drain(..over);
}

/// What one message came to.
#[derive(Clone, Debug, PartialEq)]
pub struct PlanOut {
    /// For the person: one or two plain sentences, no em or en dashes.
    pub reply: String,
    /// The edits, against the project the context was built from.
    pub compiled: Compiled,
    /// Both model calls, in US dollars.
    pub usage_usd: f64,
    /// One line on what was applied, proposed, asked or not done, for `Turn::note`.
    pub note: String,
    /// The calls compiled into `compiled`, in order (`compiled`'s call indexes are into this).
    pub calls: Vec<ToolCall>,
    /// Each model answer's calls as it made them, one entry per answer.
    pub answers: Vec<Vec<FunctionCall>>,
    /// The reply to give instead when its `followSubject` comes to nothing once it runs (`instead_of_follow`): None
    /// without a follow that went through, empty when nothing else is said.
    pub without_follow: Option<String>,
}

impl PlanOut {
    /// This message as a `Turn` of the conversation.
    pub fn turn(&self, user: &str) -> Turn {
        Turn {
            user: user.to_owned(),
            reply: self.reply.clone(),
            note: self.note.clone(),
        }
    }
}

/// What kind of failure stopped a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatErrorKind {
    /// The project's context is over budget; nothing was sent.
    TooLong,
    Budget,
    Busy,
    Unavailable,
    Network,
    /// The key, token or access code was refused.
    Refused,
    /// Turned down under the provider's content policy: asking in other words may work.
    Declined,
    Other,
}

/// Why a message came to nothing: the project is untouched.
#[derive(Clone, Debug, PartialEq)]
pub struct ChatError {
    pub kind: ChatErrorKind,
    /// One plain sentence for the person, no em or en dashes.
    pub message: String,
    /// What the model calls made before the failure cost, in US dollars.
    pub usage_usd: f64,
    /// What went wrong, for logs only (`ModelError`'s text, already without secrets): never shown.
    pub detail: String,
    /// The model calls made for the message, the failed one included: 0 when nothing was sent.
    pub model_calls: usize,
}

impl fmt::Display for ChatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ChatError {}

impl ChatError {
    fn from_model(e: ModelError, usage_usd: f64, model_calls: usize) -> Self {
        let detail = e.to_string();
        // The provider's own sentence without dashes, or ours when it gave none.
        let said = |said: Option<String>, ours: &str| {
            let said = sanitize::reply(said.as_deref().unwrap_or_default().trim());
            if said.is_empty() {
                ours.to_owned()
            } else {
                said
            }
        };
        let (kind, message) = match e {
            ModelError::Budget(s) => (ChatErrorKind::Budget, said(Some(s), OUT_OF_CREDIT)),
            ModelError::Busy(s) => (ChatErrorKind::Busy, said(s, BUSY)),
            ModelError::Unavailable(s) => (ChatErrorKind::Unavailable, said(s, UNAVAILABLE)),
            ModelError::Network => (ChatErrorKind::Network, NO_CONNECTION.to_owned()),
            ModelError::Refused => (ChatErrorKind::Refused, NOT_ACCEPTED.to_owned()),
            ModelError::Declined(s) => (ChatErrorKind::Declined, said(Some(s), DECLINED)),
            ModelError::CutOff(_) | ModelError::Other { .. } => {
                (ChatErrorKind::Other, FAILED.to_owned())
            }
        };
        Self {
            kind,
            message,
            usage_usd,
            detail,
            model_calls,
        }
    }
}

/// Sends `user` to `model` about `project`, with the last `KEPT_TURNS` of `history`, `ctx` built from `project` and
/// `inputs` from the same snapshot. The person's message counts as evidence for the times it says, whatever
/// `inputs.message` holds.
pub async fn plan(
    model: &dyn LanguageModel,
    history: &[Turn],
    ctx: &ChatContext,
    user: &str,
    inputs: &Inputs,
    project: &Project,
) -> Result<PlanOut, ChatError> {
    if !ctx.fits() {
        return Err(ChatError {
            kind: ChatErrorKind::TooLong,
            message: TOO_LONG.to_owned(),
            usage_usd: 0.0,
            detail: format!("context of {} tokens", ctx.tokens),
            model_calls: 0,
        });
    }
    let owned;
    let inputs = if inputs.message == user {
        inputs
    } else {
        owned = Inputs {
            message: user.to_owned(),
            ..inputs.clone()
        };
        &owned
    };

    let mut request = ModelRequest {
        system: system_prompt().to_owned(),
        tools: tools(),
        input: opening(history, ctx, user),
    };
    let mut spent = 0.0;
    let mut steps = 0;
    let mut answers: Vec<Vec<FunctionCall>> = Vec::new();
    // Every call kept so far, in the order made: compiled once at the end, as one additions step and one proposal.
    let mut calls: Vec<ToolCall> = Vec::new();
    // The tools of calls refused back to the model, one entry per call, until a new call of the tool goes through.
    let mut pending: Vec<String> = Vec::new();
    // The last words the model wrote, and its closing words: an answer with no call after what came of its calls.
    let mut said = String::new();
    let mut closing: Option<String> = None;
    let mut stopped: Option<ChatError> = None;
    // Whether an answer was cut off at its output cap: the first is kept and asked again once, the next fails.
    let mut cut_before = false;
    // The loop ran out of steps right after an answer with calls: the model never said it was done.
    let mut out_of_steps = false;
    loop {
        steps += 1;
        let (answer, cut) = match ask(model, &request, spent, steps).await {
            Ok(answer) => (answer, false),
            Err(Asked::CutOff(partial)) if !cut_before && steps < MAX_STEPS => {
                cut_before = true;
                (partial, true)
            }
            Err(Asked::CutOff(partial)) => {
                let e = ModelError::CutOff(partial);
                let usage_usd = spent + e.usage_usd();
                let e = ChatError::from_model(e, usage_usd, steps);
                if calls.is_empty() {
                    return Err(e);
                }
                spent = e.usage_usd;
                stopped = Some(e);
                break;
            }
            // Nothing done yet: the message failed.
            Err(Asked::Failed(e)) if calls.is_empty() => return Err(e),
            // What was done stands, and the reply says it stopped.
            Err(Asked::Failed(e)) => {
                spent = e.usage_usd;
                stopped = Some(e);
                break;
            }
        };
        spent += answer.usage_usd;
        answers.push(answer.calls.clone());
        // A cut-off answer's words end where its last whole sentence does.
        let text = if cut {
            whole_sentences(answer.text.trim())
        } else {
            answer.text.trim()
        };
        if !text.is_empty() {
            said = text.to_owned();
        }
        if answer.calls.is_empty() && cut {
            // Cut off before any call was finished: asked again, to send fewer at a time.
            request.input.push(Item::User {
                text: CUT_OFF.to_owned(),
            });
            continue;
        }
        if answer.calls.is_empty() {
            // It says what it will do but did nothing: told to go on.
            if promises_more(text) && steps < MAX_STEPS {
                request.input.push(Item::Assistant {
                    text: text.to_owned(),
                });
                request.input.push(Item::User {
                    text: GO_ON.to_owned(),
                });
                continue;
            }
            closing = Some(text.to_owned());
            break;
        }
        // Its calls on top of those kept, compiled to tell it what came of each. A repeat of a call kept adds nothing.
        let read = Read::of(&answer.calls);
        let fresh: Vec<ToolCall> = (read.calls.iter())
            .filter(|c| !calls.contains(c))
            .cloned()
            .collect();
        if fresh.is_empty() && read.unreadable.is_empty() && !cut {
            // Only what it already did: it has nothing more.
            break;
        }
        let trial: Vec<ToolCall> = calls.iter().chain(&fresh).cloned().collect();
        let compiled = compile(project, inputs, &trial);
        let outcomes = read.outcomes(&compiled, &calls);
        if !text.is_empty() {
            request.input.push(Item::Assistant {
                text: text.to_owned(),
            });
        }
        (request.input).extend(answer.calls.iter().cloned().map(Item::FunctionCall));
        for ((call, parsed), outcome) in answer.calls.iter().zip(&read.parsed).zip(&outcomes) {
            let output = match (parsed, outcome) {
                (Ok(look @ ToolCall::FindMoments { .. }), Outcome::Done) => {
                    looked_output(project, inputs, look)
                }
                _ => outcome.output().to_owned(),
            };
            request.input.push(Item::FunctionCallOutput {
                call_id: call.call_id.clone(),
                output,
            });
        }
        // A question or `cannotDo` deals with what was refused; a new call of a refused tool that goes through, with
        // one of its refusals.
        if (read.calls.iter())
            .any(|c| matches!(c, ToolCall::AskUser { .. } | ToolCall::CannotDo { .. }))
        {
            pending.clear();
        }
        for (k, parsed) in read.parsed.iter().enumerate() {
            match (parsed, &outcomes[k]) {
                // An undo refused after the message's own changes left nothing the person asked for undone: it was
                // the model taking back its own call, which the refusal tells it to do another way.
                (Ok(ToolCall::Undo), Outcome::Back(_)) => {}
                (Ok(call), Outcome::Back(_)) => pending.push(call.name().to_owned()),
                (Err(_), _) => pending.push(answer.calls[k].name.clone()),
                (Ok(call), _) if fresh.contains(call) => {
                    if let Some(at) = pending.iter().position(|p| p == call.name()) {
                        pending.remove(at);
                    }
                }
                _ => {}
            }
        }
        let kept: Vec<ToolCall> = (read.parsed.iter().zip(&outcomes))
            .filter(|(_, outcome)| !outcome.is_back())
            .filter_map(|(parsed, _)| parsed.as_ref().ok())
            .filter(|c| !calls.contains(c))
            .cloned()
            .collect();
        calls.extend(kept);
        if cut {
            // Its finished calls are kept; the rest is asked for once more, fewer at a time.
            request.input.push(Item::User {
                text: CUT_OFF.to_owned(),
            });
        }
        if steps >= MAX_STEPS {
            out_of_steps = true;
            break;
        }
    }
    let compiled = compile(project, inputs, &calls);
    // Its words then were written before what came of its calls, often what it was about to do: said instead is what
    // the edits did.
    if out_of_steps
        && closing.is_none()
        && stopped.is_none()
        && (compiled.additions.is_some() || compiled.proposal.is_some())
    {
        said = summary(&calls, &compiled);
    }
    // Closing words, when it had them, are the reply: written knowing what came of every call.
    let (text, summed) = match closing.filter(|c| !c.is_empty()) {
        Some(closing) => (closing, stopped.is_none()),
        None => (said, false),
    };
    let mut out = finish(
        &text,
        calls,
        compiled,
        &pending,
        (spent, summed),
        answers,
        stopped.as_ref().map(|e| e.message.as_str()),
    );
    if let Some(e) = stopped {
        out.note = format!("{} Stopped early: {}.", out.note, e.detail)
            .trim()
            .to_owned();
    }
    Ok(out)
}

/// `text` up to the end of its last whole sentence: a `.`, `!` or `?` and any closing quotes or brackets, then a space
/// or the end (so `1.5` ends none); empty when no sentence ends in it. The words of an answer cut off part-way.
fn whole_sentences(text: &str) -> &str {
    let closing = |c: char| matches!(c, '"' | '\'' | ')' | '\u{201d}' | '\u{2019}');
    let mut end = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if !matches!(c, '.' | '!' | '?') {
            continue;
        }
        let mut stop = i + c.len_utf8();
        while let Some(&(j, q)) = chars.peek().filter(|&&(_, q)| closing(q)) {
            stop = j + q.len_utf8();
            chars.next();
        }
        if chars.peek().is_none_or(|&(_, next)| next.is_whitespace()) {
            end = stop;
        }
    }
    &text[..end]
}

/// Whether `text`, an answer with no tool call, says the model will do something it hasn't yet ("I will turn this
/// into a short"): it is asked to go on.
fn promises_more(text: &str) -> bool {
    let t = text.trim().to_lowercase();
    !t.ends_with('?')
        && [
            "i will ",
            "i'll ",
            "let me ",
            "i am going to ",
            "i'm going to ",
            "i'm now going",
        ]
        .iter()
        .any(|p| t.starts_with(p) || t.contains(&format!(". {p}")))
}

/// What a call that only looked found, for the model: the moments shown to the person, or why none were.
fn looked_output(project: &Project, inputs: &Inputs, call: &ToolCall) -> String {
    let compiled = compile(project, inputs, std::slice::from_ref(call));
    if let Some(r) = compiled.refusals.first() {
        return format!("Not done: {}", r.reason);
    }
    if compiled.moments.is_empty() {
        return "Nothing matched.".to_owned();
    }
    let found: Vec<String> = (compiled.moments.iter())
        .map(|m| {
            let at = format!(
                "{}-{}",
                crate::context::secs(m.range.start),
                crate::context::secs(m.range.end)
            );
            if m.label.is_empty() {
                at
            } else {
                format!("{at} {}", m.label)
            }
        })
        .collect();
    format!("Shown to the person: {}.", found.join("; "))
}

/// The first request's input: the context, the conversation, the new message. An exchange with neither a reply nor a
/// note sends only what the person wrote.
fn opening(history: &[Turn], ctx: &ChatContext, user: &str) -> Vec<Item> {
    let kept = &history[history.len().saturating_sub(KEPT_TURNS)..];
    let mut input = Vec::with_capacity(2 + 2 * kept.len());
    input.push(Item::Context {
        text: ctx.text.clone(),
    });
    for turn in kept {
        input.push(Item::User {
            text: turn.user.clone(),
        });
        let text = match (turn.reply.trim(), turn.note.trim()) {
            ("", "") => continue,
            (reply, "") => reply.to_owned(),
            ("", note) => format!("({note})"),
            (reply, note) => format!("{reply}\n({note})"),
        };
        input.push(Item::Assistant { text });
    }
    input.push(Item::User {
        text: user.to_owned(),
    });
    input
}

/// The message's `nth` model call; its error as the person's sentence, with what was spent before it and on it.
async fn ask(
    model: &dyn LanguageModel,
    request: &ModelRequest,
    spent: f64,
    nth: usize,
) -> Result<ModelResponse, Asked> {
    (model.respond(request).await).map_err(|e| match e {
        ModelError::CutOff(partial) => Asked::CutOff(partial),
        e => {
            let usage_usd = spent + e.usage_usd();
            Asked::Failed(ChatError::from_model(e, usage_usd, nth))
        }
    })
}

/// Why a model call gave no whole answer: cut off at its output cap (what it finished, `ModelError::CutOff`), or
/// failed.
enum Asked {
    CutOff(ModelResponse),
    Failed(ChatError),
}

/// One answer's calls, read.
struct Read {
    /// Each call as read, in the answer's order.
    parsed: Vec<Result<ToolCall, String>>,
    /// The calls read, in order: what is compiled.
    calls: Vec<ToolCall>,
    /// The names called by the calls that couldn't be read.
    unreadable: Vec<String>,
}

impl Read {
    fn of(calls: &[FunctionCall]) -> Self {
        let parsed: Vec<Result<ToolCall, String>> = (calls.iter())
            .map(|c| ToolCall::parse(c).map_err(|e| e.to_string()))
            .collect();
        Self {
            calls: (parsed.iter().flatten().cloned()).collect(),
            unreadable: (calls.iter().zip(&parsed))
                .filter(|(_, p)| p.is_err())
                .map(|(c, _)| c.name.clone())
                .collect(),
            parsed,
        }
    }

    /// What came of each call of the answer, `compiled` being the calls `kept` before it, then its calls read that
    /// repeat none of them. A repeat of a call kept is done already.
    fn outcomes(&self, compiled: &Compiled, kept: &[ToolCall]) -> Vec<Outcome> {
        let mut at = kept.len();
        (self.parsed.iter())
            .map(|parsed| match parsed {
                Err(reason) => Outcome::Back(reason.clone()),
                Ok(call) if kept.contains(call) => Outcome::Done,
                Ok(_) => {
                    let i = at;
                    at += 1;
                    match compiled.refusals.iter().find(|r| r.call_index == i) {
                        Some(r) if r.audience == Audience::Model => Outcome::Back(r.reason.clone()),
                        Some(_) => Outcome::Told,
                        None if compiled.proposed_calls.contains(&i) => Outcome::Proposed,
                        // What Klipolo did with a short's sticker or sound, instead of "done".
                        None => (compiled.told.iter())
                            .find(|(k, _)| *k == i)
                            .map_or(Outcome::Done, |(_, said)| Outcome::Said(said.clone())),
                    }
                }
            })
            .collect()
    }
}

/// What came of one call of an answer.
enum Outcome {
    /// Refused back to the model, with the reason.
    Back(String),
    /// Refused for the person.
    Told,
    Proposed,
    Done,
    /// Done, or left out, as Klipolo says (`Compiled::told`).
    Said(String),
}

impl Outcome {
    fn is_back(&self) -> bool {
        matches!(self, Outcome::Back(_))
    }

    /// The call's output for the model.
    fn output(&self) -> &str {
        match self {
            Outcome::Back(reason) => reason,
            Outcome::Told => TOLD_PERSON,
            Outcome::Proposed => PROPOSED_OUTPUT,
            Outcome::Done => DONE_OUTPUT,
            Outcome::Said(said) => said,
        }
    }
}

/// The message's result from the model's `text` and what `calls` compiled to. `unfinished` names the calls refused back
/// to the model that no later call made good, which its text may claim. `summed`: `text` is the model's closing words,
/// written knowing what came of every call, then what the person has to do (a clip to analyze) that it doesn't say
/// yet, each once. `stopped`: why a model call failed before the model was done, said last with `STOPPED`.
fn finish(
    text: &str,
    calls: Vec<ToolCall>,
    compiled: Compiled,
    unfinished: &[String],
    (usage_usd, summed): (f64, bool),
    answers: Vec<Vec<FunctionCall>>,
    stopped: Option<&str>,
) -> PlanOut {
    let without_follow = instead_of_follow(&calls, &compiled, unfinished, stopped);
    let came_of_it = compiled.additions.is_some()
        || compiled.proposal.is_some()
        || !compiled.moments.is_empty()
        || !compiled.jobs.is_empty()
        || !compiled.questions.is_empty()
        || compiled.undo;
    // The model's own closing words, then what needs the person, then what couldn't be done.
    if summed {
        let mut reply = text.trim().to_owned();
        for r in compiled
            .refusals
            .iter()
            .filter(|r| r.audience == Audience::Person)
        {
            let said = full_stop(r.reason.trim());
            if !said.is_empty() && !reply.contains(said.trim_end_matches('.')) {
                reply = format!("{reply} {said}").trim().to_owned();
            }
        }
        if !unfinished.is_empty() {
            reply = format!("{reply} {}", could_not(unfinished, came_of_it));
        }
        return PlanOut {
            note: note(&calls, &compiled, unfinished),
            reply: sanitize::reply(&reply),
            compiled,
            usage_usd,
            calls,
            answers,
            without_follow,
        };
    }
    // An edit refused for the person (nothing to remove, a clip not analyzed yet): the text was written before it, and
    // may say it was done. It is left out; what else was done is said plainly instead.
    let told_undone = (compiled.refusals.iter()).any(|r| {
        r.audience == Audience::Person && !matches!(calls[r.call_index], ToolCall::CannotDo { .. })
    });
    let mut parts: Vec<String> = Vec::new();
    let text = text.trim();
    if !text.is_empty() && !told_undone && (came_of_it || unfinished.is_empty()) {
        parts.push(text.to_owned());
    }
    if told_undone && came_of_it {
        parts.extend(done_sentences(&calls, &compiled));
    }
    // What was said of the edits, then Klipolo's own notes on them (a speed that shortens a transition).
    let said_of_edits = parts.len();
    for r in &compiled.refusals {
        if r.audience == Audience::Person {
            let said = full_stop(r.reason.trim());
            if !said.is_empty() && !parts.contains(&said) {
                parts.push(said);
            }
        }
    }
    if !unfinished.is_empty() {
        parts.push(could_not(unfinished, came_of_it));
    }
    if parts.is_empty() {
        parts.extend(compiled.questions.iter().map(|q| full_stop(q.trim())));
    }
    if parts.is_empty() {
        parts.push(
            match (compiled.additions.is_some(), compiled.proposal.is_some()) {
                (true, true) => "Done. The removals are waiting for you to apply them.",
                (true, false) => "Done.",
                (false, true) => "Here is what I would remove. Apply it or discard it.",
                // Only clips being reversed: their copies are being written.
                (false, false)
                    if !compiled.jobs.is_empty()
                        && (compiled.jobs.iter())
                            .all(|j| matches!(j, crate::Job::Reverse { .. })) =>
                {
                    crate::compile::REVERSING
                }
                // Only work for a runner: a picture being drawn, a subject being tracked.
                (false, false) if !compiled.jobs.is_empty() => "Working on it.",
                (false, false) => "Nothing changed.",
            }
            .to_owned(),
        );
    }
    // After what was said of the edits: the model's text, the sentences of what was done, or the short sentence alone.
    let at = if parts.len() == 1 && said_of_edits == 0 {
        1
    } else {
        said_of_edits
    };
    let notes: Vec<String> = (compiled.notes.iter().chain(&compiled.waits))
        .filter(|n| !parts.contains(n))
        .cloned()
        .collect();
    parts.splice(at..at, notes);
    if let Some(why) = stopped {
        parts.push(format!("{} {STOPPED}", full_stop(why.trim())));
    }
    let reply = sanitize::reply(&parts.join(" "));
    PlanOut {
        note: note(&calls, &compiled, unfinished),
        reply,
        compiled,
        usage_usd,
        calls,
        answers,
        without_follow,
    }
}

/// The reply should the message's `followSubject` come to nothing once it runs (nobody found, the tracking failed, no
/// clip can follow). It runs after the plan, so the model was told it was done and its words may claim it: they are
/// left out, as for a call refused for the person. Said instead: what else went through, plainly (`done_sentences`
/// without the follow), Klipolo's notes on it, the sentences of refusals meant for the person, what couldn't be done,
/// and why it stopped; empty when none of these, so that what the follow came to is the reply. None without a follow
/// that went through.
fn instead_of_follow(
    calls: &[ToolCall],
    compiled: &Compiled,
    unfinished: &[String],
    stopped: Option<&str>,
) -> Option<String> {
    let refused = |i: usize| compiled.refusals.iter().any(|r| r.call_index == i);
    let followed = (calls.iter().enumerate())
        .any(|(i, c)| matches!(c, ToolCall::FollowSubject { .. }) && !refused(i));
    if !followed {
        return None;
    }
    let mut parts = said_done(calls, compiled, |c| {
        !matches!(c, ToolCall::FollowSubject { .. })
    });
    let else_done = !parts.is_empty();
    for said in (compiled.notes.iter().chain(&compiled.waits)).map(|n| full_stop(n.trim())) {
        if !said.is_empty() && !parts.contains(&said) {
            parts.push(said);
        }
    }
    for r in (compiled.refusals.iter()).filter(|r| r.audience == Audience::Person) {
        let said = full_stop(r.reason.trim());
        if !said.is_empty() && !parts.contains(&said) {
            parts.push(said);
        }
    }
    if !unfinished.is_empty() {
        parts.push(could_not(unfinished, else_done));
    }
    if let Some(why) = stopped {
        parts.push(format!("{} {STOPPED}", full_stop(why.trim())));
    }
    Some(sanitize::reply(&parts.join(" ")))
}

/// What a call that went through did, said plainly for a reply that leaves the model's text out.
fn done(call: &ToolCall) -> Option<&'static str> {
    Some(match call {
        ToolCall::AddMusic { .. } => "The music is in.",
        ToolCall::AddSound { .. } => "The sound is in.",
        ToolCall::AddImage { .. } => "The picture is in.",
        ToolCall::AddText { .. } => "The words are on screen.",
        ToolCall::MakeTitleCard { .. } => "The title is in.",
        ToolCall::MakeSticker { .. } => "The sticker is being drawn.",
        ToolCall::Animate { .. } => "The animation is in.",
        ToolCall::SetMotion { .. } => "The motion is set.",
        ToolCall::AnimateText { .. } => "The words are animated.",
        ToolCall::SetLayer { .. } => "The layer is changed.",
        ToolCall::StyleText { .. } => "The words are restyled.",
        ToolCall::SetEffects { .. } => "The effects are changed.",
        ToolCall::FollowSubject { .. } | ToolCall::BlurObjects { .. } => "Working on it.",
        ToolCall::SetFormat { .. } => "The format is changed.",
        ToolCall::SetCaptions { .. } => "The captions are changed.",
        ToolCall::SetBackground { .. } => "The background is changed.",
        ToolCall::FixCaptionWords { .. } => "The words are fixed.",
        ToolCall::SplitAt { .. } => "The clip is split.",
        ToolCall::FreezeFrame { .. } => "The frame holds still there.",
        ToolCall::SetClip {
            clip: crate::tools::ClipRef::All,
            ..
        } => "The clips are changed.",
        ToolCall::SetClip { .. } => "The clip is changed.",
        ToolCall::SetTransition { .. } => "The transitions are changed.",
        ToolCall::ReverseClip { backward: true, .. } => crate::compile::REVERSING,
        ToolCall::ReverseClip {
            backward: false, ..
        } => "The clip plays forward again.",
        ToolCall::TrimClip { .. } => "The clip is longer.",
        ToolCall::SetCameraMode { .. } | ToolCall::AdjustCamera { .. } => "The camera is changed.",
        ToolCall::DetachCamera { .. } => "The camera is its own layer now.",
        ToolCall::MuteSource { muted: true, .. } => "The sound is muted.",
        ToolCall::MuteSource { muted: false, .. } => "The sound is back on.",
        ToolCall::SetSourceVolume { .. } => "The volume is changed.",
        ToolCall::SeparateSound { .. } => "The sound is on its own track now.",
        ToolCall::AutoZoom { on: true, .. } => "The zooms are in.",
        ToolCall::AutoZoom { on: false, .. } => "The zooms are gone.",
        ToolCall::Undo => "Undone.",
        // Shown on their own (moments, questions), or proposed.
        _ => return None,
    })
}

/// One or two sentences on what a message's edits did, for a reply when the model never closed: what the additions
/// did ("I made it vertical and added captions, a title, 3 stickers, 3 sounds and music."), from the calls that went
/// through and the layers they added, then the cut waiting for the person and how long the video is once applied.
fn summary(calls: &[ToolCall], compiled: &Compiled) -> String {
    use klipolo_model::Op;
    use klipolo_model::media::music::is_music;
    use klipolo_model::timeline::ItemContent;
    let through = |i: usize| {
        !compiled.refusals.iter().any(|r| r.call_index == i)
            && !compiled.proposed_calls.contains(&i)
            && !compiled.unchanged.contains(&i)
    };
    let mut did: Vec<String> = Vec::new();
    let mut added: Vec<String> = Vec::new();
    let count = |n: usize, one: &str, many: &str| match n {
        0 => None,
        1 => Some(one.to_owned()),
        n => Some(format!("{n} {many}")),
    };
    for (_, call) in calls.iter().enumerate().filter(|&(i, _)| through(i)) {
        let said = match call {
            ToolCall::SetFormat { format, .. } => match format {
                klipolo_model::project::FormatKind::Portrait => "made it vertical",
                klipolo_model::project::FormatKind::Square => "made it square",
                klipolo_model::project::FormatKind::Landscape => "made it wide",
            },
            ToolCall::FollowSubject { .. } => "kept the speaker in frame",
            ToolCall::AutoZoom { on: true, .. } => "zoomed in on the clicks",
            _ => continue,
        };
        if !did.iter().any(|d| d == said) {
            did.push(said.to_owned());
        }
    }
    let ops: &[Op] = (compiled.additions.as_ref()).map_or(&[][..], |tx| tx.ops.as_slice());
    let items = || {
        ops.iter().filter_map(|op| match op {
            Op::AddItem { item, .. } => Some(item),
            _ => None,
        })
    };
    let captions = (calls.iter().enumerate())
        .any(|(i, c)| through(i) && matches!(c, ToolCall::SetCaptions { on: true, .. }));
    if captions {
        added.push("captions".to_owned());
    }
    let words = items()
        .filter(|i| {
            matches!(
                i.content,
                ItemContent::Text { .. } | ItemContent::Generator { .. }
            )
        })
        .count();
    let drawn = (compiled.jobs.iter())
        .filter(|j| matches!(j, crate::Job::MakeSticker(_)))
        .count();
    let pictures = items()
        .filter(|i| matches!(i.content, ItemContent::Image { .. }))
        .count()
        + drawn;
    let sounds = items()
        .filter(|i| matches!(i.content, ItemContent::Audio { .. }) && !is_music(i))
        .count();
    let music = items().any(is_music);
    added.extend(count(words, "a title", "titles"));
    added.extend(count(pictures, "a sticker", "stickers"));
    added.extend(count(sounds, "a sound", "sounds"));
    if music {
        added.push("music".to_owned());
    }
    let list = |parts: &[String]| match parts {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    };
    let mut out = match (did.is_empty(), added.is_empty()) {
        (false, false) => format!("I {} and added {}.", list(&did), list(&added)),
        (false, true) => format!("I {}.", list(&did)),
        (true, false) => format!("I added {}.", list(&added)),
        (true, true) if compiled.additions.is_some() => "I made the changes.".to_owned(),
        (true, true) => String::new(),
    };
    if let Some(proposal) = &compiled.proposal {
        let length = klipolo_model::query::video_duration(&proposal.review.after.timeline);
        let secs = (length as f64 / US_PER_SEC as f64).round();
        out = format!("{out} Approve the cut to keep the {secs} s version.")
            .trim()
            .to_owned();
    }
    out
}

/// The calls of `compiled` that went through, each said once (`done`), then the proposal if there is one.
fn done_sentences(calls: &[ToolCall], compiled: &Compiled) -> Vec<String> {
    said_done(calls, compiled, |_| true)
}

/// [`done_sentences`] of the calls `said` keeps.
fn said_done(
    calls: &[ToolCall],
    compiled: &Compiled,
    said: impl Fn(&ToolCall) -> bool,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (i, call) in calls.iter().enumerate().filter(|(_, c)| said(c)) {
        let refused = compiled.refusals.iter().any(|r| r.call_index == i);
        // Proposed, or through but changing nothing (a format the video already has): not said as done.
        if refused || compiled.proposed_calls.contains(&i) || compiled.unchanged.contains(&i) {
            continue;
        }
        if let Some(said) = done(call).filter(|said| !out.iter().any(|o| o == said)) {
            out.push(said.to_owned());
        }
    }
    if compiled.proposal.is_some() {
        out.push("Here is what I would remove. Apply it or discard it.".to_owned());
    }
    out
}

/// `text` ending in a full stop, a question mark or an exclamation mark.
fn full_stop(text: &str) -> String {
    if text.is_empty() || text.ends_with(['.', '?', '!']) {
        text.to_owned()
    } else {
        format!("{text}.")
    }
}

/// What a tool does, as the reply says it couldn't.
fn task(tool: &str) -> &'static str {
    match tool {
        "cutRange" => "make the cut",
        "keepOnly" => "keep only that part",
        "removeSilences" => "remove the pauses",
        "removeFillers" => "remove the filler words",
        "trimClip" => "trim the clip",
        "splitAt" => "split the clip",
        "freezeFrame" => "hold that frame",
        "setClip" => "change the clip",
        "setTransition" => "change the transition",
        "reverseClip" => "reverse the clip",
        "setFormat" => "change the format",
        "setCaptions" => "change the captions",
        "setBackground" => "change the background",
        "fixCaptionWords" => "fix the words",
        "findMoments" => "find those moments",
        "followSubject" => "follow the subject",
        "blurObjects" => "add the blur",
        "addMusic" => "add the music",
        "addSound" => "add the sound",
        "addText" => "put the words on screen",
        "addImage" => "add the picture",
        "makeSticker" => "make the sticker",
        "makeTitleCard" => "make the title card",
        "setCameraMode" => "change the camera",
        "adjustCamera" => "adjust the camera",
        "detachCamera" => "separate the camera",
        "muteSource" => "mute the sound",
        "setSourceVolume" => "change the volume",
        "separateSound" => "separate the sound",
        "undo" => "undo the last change",
        _ => "do part of this",
    }
}

/// Whether a tool works on a part of the video the model names by its times: refused back, that part stood on nothing
/// the context showed, so the person is asked to point at it.
fn names_a_part(tool: &str) -> bool {
    matches!(tool, "cutRange" | "keepOnly" | "trimClip" | "findMoments")
}

/// What the reply says the tools named couldn't do, with what to do next, `else_done` when something else of the
/// answer went through (so the video wasn't left as it was). Parts of the video it couldn't find: "I couldn't find that
/// part, so I left the video as it was. Select it on the timeline and ask again." Anything else by what it does: "I
/// couldn't add the sound, so I left the video as it was. Try asking in other words."
fn could_not(tools: &[String], else_done: bool) -> String {
    let left = if else_done {
        "so I left it out"
    } else {
        "so I left the video as it was"
    };
    if tools.iter().all(|t| names_a_part(t)) {
        return format!(
            "I couldn't find that part, {left}. Select it on the timeline and ask again."
        );
    }
    let mut tasks: Vec<&str> = Vec::new();
    for t in tools {
        let task = task(t);
        if !tasks.contains(&task) {
            tasks.push(task);
        }
    }
    let list = match tasks.as_slice() {
        [one] => (*one).to_owned(),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
        [] => "do part of this".to_owned(),
    };
    let left = if else_done && tasks.len() > 1 {
        "so I left those out"
    } else {
        left
    };
    format!("I couldn't {list}, {left}. Try asking in other words.")
}

/// One line on what `calls` came to, with the tools refused back and never made good (`dropped`): "Applied setFormat.
/// Proposed cutRange (10.0 s). Asked: Which part? Not done: trimClip."
fn note(calls: &[ToolCall], compiled: &Compiled, dropped: &[String]) -> String {
    let refused = |i: usize| compiled.refusals.iter().any(|r| r.call_index == i);
    let mut applied: Vec<&str> = Vec::new();
    let mut proposed: Vec<&str> = Vec::new();
    let mut not_done: Vec<&str> = Vec::new();
    fn add<'a>(list: &mut Vec<&'a str>, name: &'a str) {
        if !list.contains(&name) {
            list.push(name);
        }
    }
    for (i, call) in calls.iter().enumerate() {
        let name = call.name();
        if refused(i) {
            if !matches!(call, ToolCall::CannotDo { .. }) {
                add(&mut not_done, name);
            }
        } else if compiled.proposed_calls.contains(&i) {
            add(&mut proposed, name);
        } else if !matches!(
            call,
            ToolCall::AskUser { .. } | ToolCall::CannotDo { .. } | ToolCall::Undo
        ) {
            add(&mut applied, name);
        }
    }
    for name in dropped {
        add(&mut not_done, name);
    }
    let mut parts: Vec<String> = Vec::new();
    if !applied.is_empty() {
        parts.push(format!("Applied {}.", applied.join(", ")));
    }
    if let Some(p) = &compiled.proposal {
        let tenths = (p.removed_us.max(0) + US_PER_SEC / 20) / (US_PER_SEC / 10);
        parts.push(format!(
            "Proposed {} ({}.{} s).",
            proposed.join(", "),
            tenths / 10,
            tenths % 10
        ));
    }
    if compiled.undo {
        parts.push("Asked to undo.".to_owned());
    }
    for q in &compiled.questions {
        parts.push(format!("Asked: {}", full_stop(q.trim())));
    }
    if !not_done.is_empty() {
        parts.push(format!("Not done: {}.", not_done.join(", ")));
    }
    if parts.is_empty() {
        return "Nothing changed.".to_owned();
    }
    (parts.join(" ").split_whitespace())
        .collect::<Vec<_>>()
        .join(" ")
}
