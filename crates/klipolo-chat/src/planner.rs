//! The planner: one chat message to the language model (`model::LanguageModel`), out as a reply for the
//! person and compiled edits (`compile`): additions applied at once, removals proposed.
//!
//! The request is the system prompt, the tools and the project context, in that order and the same bytes for every
//! message on one project revision, so a provider's prompt cache reuses them; then the conversation (`Turn`s, the last
//! `KEPT_TURNS`), then the new message last.
//!
//! One refusal round, never more than two model calls a message. When a call of the first answer is refused back to the
//! model (its arguments unreadable, or the compiler's `Audience::Model` refusal, such as a time that stands on
//! nothing), the whole input is sent again with nothing stored at the provider: the first answer's text and calls, then
//! one output per call (the reason for a refused one, `DONE_OUTPUT` or `PROPOSED_OUTPUT` for the others, and
//! `TOLD_PERSON` for one refused for the person, whose reason is never sent back). The calls of the second answer
//! replace the refused ones: the first answer's other calls and the second's (a repeat of what was done dropped) are
//! compiled once, in that order, against the same snapshot and inputs, so the message still makes one additions
//! transaction and one proposal. A call refused back to the model again stays undone, and the reply says so; so does a
//! call refused in the first answer that the second answer lets go (no new call of its tool for it, counted call by
//! call, and no question or `cannotDo` instead).
//!
//! The reply is the model's text, then what the edits left as they were (`Compiled::notes`), then the sentences of
//! refusals meant for the person, then what couldn't be done; all of it without em or en dashes (`sanitize::reply`). When a call was refused for the person beside others that went
//! through, the model's text, written before that refusal, may claim it: it is left out, and what went through is said
//! plainly instead ("The music is in."). When nothing came of the answer and a call of it was refused after
//! the model wrote its text (still refused back, or refused for the person, `cannotDo` aside), that text, which may
//! claim the edit, is left out. With none of these, the reply is `askUser`'s questions, else a short sentence on what
//! was done (or, for only work handed to a runner, that it is under way); otherwise the questions are only in
//! `Compiled::questions`.
//!
//! Errors leave the project untouched: nothing is compiled, and each is one plain sentence (`ChatError`). A project
//! whose context is over budget (`ChatContext::fits`) is refused before any model call.

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
    /// Each model answer's calls as it made them: one answer, or two after a refusal round.
    pub answers: Vec<Vec<FunctionCall>>,
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
            ModelError::Other { .. } => (ChatErrorKind::Other, FAILED.to_owned()),
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
    let first = ask(model, &request, spent, 1).await?;
    spent += first.usage_usd;
    let read = Read::of(&first.calls);
    let compiled = compile(project, inputs, &read.calls);
    let outcomes = read.outcomes(&compiled);
    if !outcomes.iter().any(Outcome::is_back) {
        return Ok(finish(
            &first.text,
            read.calls,
            compiled,
            (&[], &[]),
            spent,
            vec![first.calls],
        ));
    }

    // The refusal round: everything again, then the first answer and what came of each of its calls.
    let text = first.text.trim();
    if !text.is_empty() {
        request.input.push(Item::Assistant {
            text: text.to_owned(),
        });
    }
    (request.input).extend(first.calls.iter().cloned().map(Item::FunctionCall));
    for (call, outcome) in first.calls.iter().zip(&outcomes) {
        request.input.push(Item::FunctionCallOutput {
            call_id: call.call_id.clone(),
            output: outcome.output().to_owned(),
        });
    }
    let second = ask(model, &request, spent, 2).await?;
    spent += second.usage_usd;

    // The first answer's calls not sent back, then the second's, compiled once. The second answer redoes only what was
    // refused: a call of a tool the first answer already did (and had none of refused) is dropped, as is an exact
    // repeat of a call done.
    let mut calls: Vec<ToolCall> = (read.parsed.iter().zip(&outcomes))
        .filter(|(_, outcome)| !outcome.is_back())
        .filter_map(|(parsed, _)| parsed.as_ref().ok().cloned())
        .collect();
    let kept = calls.len();
    let done: Vec<&str> = calls.iter().map(ToolCall::name).collect();
    let refused: Vec<&str> = (first.calls.iter().zip(&outcomes))
        .filter(|(_, outcome)| outcome.is_back())
        .map(|(call, _)| call.name.as_str())
        .collect();
    let again = Read::of(&second.calls);
    let dealt_with = (again.calls.iter())
        .any(|c| matches!(c, ToolCall::AskUser { .. } | ToolCall::CannotDo { .. }));
    // The tools of the second answer's new calls, one entry per call (unreadable ones too: they are named anyway).
    let mut retried: Vec<String> = again.unreadable.clone();
    for call in again.calls {
        let repeat = calls[..kept].contains(&call);
        let redone = done.contains(&call.name()) && !refused.contains(&call.name());
        if !repeat && !redone {
            retried.push(call.name().to_owned());
            calls.push(call);
        }
    }
    // Each refused call of the first answer needs a new call of its tool in the second, or a question or `cannotDo`
    // instead: one without is still not done, even when another call of that tool was only repeated.
    let mut dropped: Vec<String> = Vec::new();
    for name in refused.iter().filter(|_| !dealt_with) {
        match retried.iter().position(|r| r == name) {
            Some(k) => {
                retried.swap_remove(k);
            }
            None if !dropped.iter().any(|d| d == name) => dropped.push((*name).to_owned()),
            None => {}
        }
    }
    let compiled = compile(project, inputs, &calls);
    // Still refused back to the model: by name, as the model called them.
    let mut unfinished: Vec<String> = (compiled.refusals.iter())
        .filter(|r| r.audience == Audience::Model)
        .map(|r| calls[r.call_index].name().to_owned())
        .collect();
    unfinished.extend(again.unreadable);
    Ok(finish(
        &second.text,
        calls,
        compiled,
        (&unfinished, &dropped),
        spent,
        vec![first.calls, second.calls],
    ))
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
) -> Result<ModelResponse, ChatError> {
    (model.respond(request).await).map_err(|e| {
        let usage_usd = spent + e.usage_usd();
        ChatError::from_model(e, usage_usd, nth)
    })
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

    /// What came of each call of the answer, `compiled` being its calls read.
    fn outcomes(&self, compiled: &Compiled) -> Vec<Outcome> {
        let mut at = 0;
        (self.parsed.iter())
            .map(|parsed| {
                let Err(reason) = parsed else {
                    let i = at;
                    at += 1;
                    return match compiled.refusals.iter().find(|r| r.call_index == i) {
                        Some(r) if r.audience == Audience::Model => Outcome::Back(r.reason.clone()),
                        Some(_) => Outcome::Told,
                        None if compiled.proposed_calls.contains(&i) => Outcome::Proposed,
                        None => Outcome::Done,
                    };
                };
                Outcome::Back(reason.clone())
            })
            .collect()
    }
}

/// What came of one call of the first answer.
enum Outcome {
    /// Refused back to the model, with the reason.
    Back(String),
    /// Refused for the person.
    Told,
    Proposed,
    Done,
}

impl Outcome {
    fn is_back(&self) -> bool {
        matches!(self, Outcome::Back(_))
    }

    /// The call's output for the model in the refusal round.
    fn output(&self) -> &str {
        match self {
            Outcome::Back(reason) => reason,
            Outcome::Told => TOLD_PERSON,
            Outcome::Proposed => PROPOSED_OUTPUT,
            Outcome::Done => DONE_OUTPUT,
        }
    }
}

/// The message's result from the last answer's `text` and what `calls` compiled to. `unfinished` names the calls still
/// refused back to the model after the last answer, whose text may claim them; `dropped` the tools refused in the first
/// answer that the second let go, which its text was written knowing.
fn finish(
    text: &str,
    calls: Vec<ToolCall>,
    compiled: Compiled,
    (unfinished, dropped): (&[String], &[String]),
    usage_usd: f64,
    answers: Vec<Vec<FunctionCall>>,
) -> PlanOut {
    let came_of_it = compiled.additions.is_some()
        || compiled.proposal.is_some()
        || !compiled.moments.is_empty()
        || !compiled.jobs.is_empty()
        || !compiled.questions.is_empty()
        || compiled.undo;
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
    // What the edits left as they were follows what was done.
    let mut lead = parts.len();
    for r in &compiled.refusals {
        if r.audience == Audience::Person {
            let said = full_stop(r.reason.trim());
            if !said.is_empty() && !parts.contains(&said) {
                parts.push(said);
            }
        }
    }
    let not_done: Vec<String> = unfinished.iter().chain(dropped).cloned().collect();
    if !not_done.is_empty() {
        parts.push(could_not(&not_done, came_of_it));
    }
    if parts.is_empty() {
        parts.extend(compiled.questions.iter().map(|q| full_stop(q.trim())));
    }
    if parts.is_empty() {
        lead = 1;
        parts.push(
            match (compiled.additions.is_some(), compiled.proposal.is_some()) {
                (true, true) => "Done. The removals are waiting for you to apply them.",
                (true, false) => "Done.",
                (false, true) => "Here is what I would remove. Apply it or discard it.",
                // Only work for a runner: a picture being drawn, a subject being tracked.
                (false, false) if !compiled.jobs.is_empty() => "Working on it.",
                (false, false) => "Nothing changed.",
            }
            .to_owned(),
        );
    }
    let notes: Vec<String> = (compiled.notes.iter())
        .map(|n| full_stop(n.trim()))
        .filter(|n| !n.is_empty() && !parts.contains(n))
        .collect();
    parts.splice(lead..lead, notes);
    let reply = sanitize::reply(&parts.join(" "));
    PlanOut {
        note: note(&calls, &compiled, dropped),
        reply,
        compiled,
        usage_usd,
        calls,
        answers,
    }
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
        ToolCall::FollowSubject { .. } | ToolCall::BlurObjects { .. } => "Working on it.",
        ToolCall::SetFormat { .. } => "The format is changed.",
        ToolCall::SetCaptions { .. } => "The captions are changed.",
        ToolCall::FixCaptionWords { .. } => "The words are fixed.",
        ToolCall::SplitAt { .. } => "The clip is split.",
        ToolCall::TrimClip { .. } => "The clip is longer.",
        ToolCall::SetCameraMode { .. } | ToolCall::AdjustCamera { .. } => "The camera is changed.",
        ToolCall::DetachCamera { .. } => "The camera is its own layer now.",
        ToolCall::Undo => "Undone.",
        // Shown on their own (moments, questions), or proposed.
        _ => return None,
    })
}

/// The calls of `compiled` that went through, each said once (`done`), then the proposal if there is one.
fn done_sentences(calls: &[ToolCall], compiled: &Compiled) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (i, call) in calls.iter().enumerate() {
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
        "setFormat" => "change the format",
        "setCaptions" => "change the captions",
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

/// One line on what `calls` came to, with the tools `dropped` in a refusal round: "Applied setFormat. Proposed cutRange
/// (10.0 s). Asked: Which part? Not done: trimClip."
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
