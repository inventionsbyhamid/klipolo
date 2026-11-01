//! An edit tool's call from an app: chat's own tool, read, checked and compiled as chat's are, then applied, proposed
//! or refused. The app's pace of calls is checked before a call reaches here (`rpc`). In order:
//! 1. `ToolCall::parse`: a call that doesn't fit its tool is refused with the parse error's sentence. `undo` undoes the
//!    app's own newest step and nothing else (`Host::undo_own`), once no one else's proposal waits.
//! 2. The open project (`NO_PROJECT` without one). A call naming times or clip refs needs the app to have read the
//!    project's main track as it is now (`Seen`): `READ_FIRST` when it never read, `STALE` when the clips moved since.
//!    One naming a song or a video layer by its ref needs its layers as they are too (`STALE` when one came or went).
//! 3. `findMoments` only points the person at moments (it is read-only): it compiles alone and is shown.
//! 4. The proposal slot, shared with Klipolo's chat: while a proposal that isn't the app's own waits, every edit is
//!    refused (`WAITING_OTHER`). The app's own absorbs the call: it compiles after the calls the proposal holds
//!    (`Own`), on the revision the proposal was made on, while the clips they name stay where they were (else
//!    `OWN_MOVED`) and they still remove what was proposed (else `OWN_CHANGED`). One Klipolo doesn't know waits for
//!    the person (`OWN_WAITING`).
//! 5. `compile_for_app`, with no typed message: an app's words are never the person's times, but the words it puts on
//!    screen (`addText`, `makeTitleCard`'s title) are its own, as it gave them. A refusal of the call is the tool's
//!    error, with its reason, and nothing changes.
//! 6. Additions apply at once as one undo step the app made (`from_app`), checked again first to take nothing away,
//!    within the app's pace of applied batches (`Limits::applied`; a batch the host refuses gives its place back).
//!    Removals become a proposal for the person, never applied here: in place of the app's own, or, once additions
//!    applied, on top of them, only when the commit dropped the app's own proposal. One the person decided on while
//!    the call ran stays decided (`DECIDED`).
//! 7. Each job that costs money passes the run's allowance and the app's pace of jobs (`Limits::paid`), then only
//!    `Host::start_paid` starts it, after the app's own checks. Nothing here calls a model.
//!
//! The only commit is `Host::apply` of additions that `needs_review` passes, and the only other change the app's own
//! undo: no path here cuts footage.
//!
//! A result's text says what was done, with chat's own notes on it (a new speed that shortens a transition, as the clip
//! card says it, or a camera left as it was because it is its own layer), what was proposed and what wasn't done. A
//! result is one text and `structuredContent`: `{"outcome": "done" | "proposed" | "shown" | "started" | "refused",
//! "revision"?, "proposalId"?, "removedSec"?}`, the outcome what changed most (a proposal, then the project, then a
//! job started). `isError` is true exactly when nothing was done; when part was, the text says what wasn't.

use klipolo_chat::{
    ANALYZE_FIRST, Audience, Compiled, FunctionCall, Job, Moment, ROUGH_FILLERS,
    ROUGH_FILLERS_STAY, Refusal, STILL_LISTENING, STILL_WATCHING, ToolCall, compile_for_app,
    layers_key, timeline_key,
};
use klipolo_model::Transaction;
use klipolo_model::coded::ErrorParam;
use klipolo_model::ops::{Op, Origin};
use klipolo_model::time::{RangeUs, Us};
use serde_json::{Value, json};

use crate::host::{Host, HostError, OpenProposal, ProposalOutcome, Snapshot};
use crate::rpc::Mcp;
use crate::session::{Seen, Session};
use crate::words::{self, fill};

/// The most ranges a result names; the rest are counted (`getProposal` lists them all).
const RANGES_SAID: usize = 10;

/// An app's waiting proposal as Klipolo made it, so the app's next call joins it: whose, its id, the calls in it,
/// where the main track's clips were when those calls were made (`timeline_key`) and the ops they were proposed as.
/// Clip refs and times name clips by where they are, so the calls are read again only while the clips stay there: once
/// the app's own additions move them (a split), the proposal waits for the person. And they must still remove exactly
/// what was proposed: a time that stood on the person's selection or playhead, moved since, would snap elsewhere. One
/// proposal waits at a time, so one is remembered; it holds only while a proposal with its id waits.
#[derive(Clone, Debug)]
pub(crate) struct Own {
    client: String,
    id: String,
    calls: Vec<ToolCall>,
    key: u64,
    ops: Vec<Op>,
}

/// What `call` did, as a tool result.
pub fn run(mcp: &Mcp, session: &mut Session, call: &FunctionCall) -> Value {
    let client = session.client.as_str();
    let host = mcp.host();
    let new = match ToolCall::parse(call) {
        Ok(new) => new,
        Err(e) => return refused(&e.to_string()),
    };
    if new == ToolCall::Undo {
        return undo(host, client);
    }
    let Some(mut snap) = host.snapshot() else {
        return refused(words::NO_PROJECT);
    };
    // An app's words are never the person's typed times: only the person's view in Klipolo counts.
    snap.inputs.message.clear();
    if new.names_timeline()
        && let Err(said) = read_as_it_is(session.seen.as_ref(), &snap)
    {
        return refused(said);
    }
    // A layer ref names a layer by its place among the layers the app read: never a different one since.
    if new.names_layer()
        && session
            .seen
            .as_ref()
            .is_some_and(|seen| seen.layers != layers_key(&snap.project))
    {
        return refused(words::STALE);
    }
    if matches!(new, ToolCall::FindMoments { .. }) {
        return show(host, client, &snap, new);
    }
    let open = host.open_proposal();
    // The person decided on the proposal remembered, or it was dropped: it is no one's to join.
    mcp.own()
        .take_if(|own| open.as_ref().is_none_or(|p| p.id != own.id));
    if let Some(who) = someone_else(open.as_ref(), client) {
        return refused(&waiting_other(&who));
    }
    let (own, mut calls) = match open {
        None => (None, Vec::new()),
        Some(open) => match joined(mcp, client, &open, &snap) {
            Ok(calls) => (Some(open), calls),
            Err(why) => return refused(&why),
        },
    };
    calls.push(new);
    let at = calls.len() - 1;
    let out = compile_for_app(&snap.project, &snap.inputs, &calls);
    let mine: Vec<&Refusal> = out.refusals.iter().filter(|r| r.call_index == at).collect();
    if !mine.is_empty() {
        return refused(&reasons(mine));
    }
    // With this call after them, the waiting calls would remove something else: the proposal waits as it is.
    if out.refusals.iter().any(|r| r.call_index < at) {
        return refused(words::OWN_CHANGED);
    }
    edit(mcp, client, &snap, &calls, out, own.as_ref())
}

/// Undoes `client`'s own newest step, unless someone else's proposal waits (the person's, or another app's).
fn undo(host: &dyn Host, client: &str) -> Value {
    if let Some(who) = someone_else(host.open_proposal().as_ref(), client) {
        return refused(&waiting_other(&who));
    }
    match host.undo_own(client) {
        Ok(undone) => {
            let mut said = vec![words::UNDONE.to_owned()];
            // The undo changed the project, so the app's own proposal waiting on it went too.
            if undone.dropped.is_some() {
                said.push(words::UNDO_DROPPED.to_owned());
            }
            answer(
                &said,
                json!({"outcome": "done", "revision": undone.revision}),
            )
        }
        Err(e) => refused(&host_error(host, client, e)),
    }
}

/// The calls in `open`, `client`'s own waiting proposal, to compile again before the app's new call on `snap`; else
/// why the call waits for the person.
fn joined(
    mcp: &Mcp,
    client: &str,
    open: &OpenProposal,
    snap: &Snapshot,
) -> Result<Vec<ToolCall>, String> {
    // A copy, so the lock isn't held across the compile below: other apps' calls take it too.
    let own = (mcp.own().as_ref())
        .filter(|o| o.id == open.id && o.client == client)
        .cloned();
    let Some(own) = own else {
        // What it holds isn't known here (the server started again since): it waits for the person.
        return Err(words::OWN_WAITING.to_owned());
    };
    // Made on a revision this call didn't read (another session of the app moved it on): trying again reads it.
    if open.base_revision != snap.revision {
        return Err(words::MOVED.to_owned());
    }
    // The app's own change moved the clips its calls name: read again, they would name other footage.
    if own.key != timeline_key(&snap.project) {
        return Err(words::OWN_MOVED.to_owned());
    }
    // Read again now, the calls must remove what they did when proposed. One more compile, of the waiting calls alone.
    let again = compile_for_app(&snap.project, &snap.inputs, &own.calls);
    if !again.refusals.is_empty() || again.proposal.is_none_or(|p| p.tx.ops != own.ops) {
        return Err(words::OWN_CHANGED.to_owned());
    }
    Ok(own.calls)
}

/// Whether what the app last read (`seen`) is the project's main track as it is now; else what to read.
fn read_as_it_is(seen: Option<&Seen>, snap: &Snapshot) -> Result<(), &'static str> {
    let Some(seen) = seen else {
        return Err(words::READ_FIRST);
    };
    if seen.project_id == snap.project.id && seen.key == timeline_key(&snap.project) {
        Ok(())
    } else {
        Err(words::STALE)
    }
}

/// `tx`, an app's batch: the same label and ops, the app's name added to its params, from the app.
pub fn from_app(tx: &Transaction, client: &str) -> Transaction {
    let mut params = tx.params.clone().unwrap_or_default();
    params.insert("client".to_owned(), ErrorParam::from(client));
    Transaction {
        label: tx.label.clone(),
        params: Some(params),
        ops: tx.ops.clone(),
        origin: Some(Origin::External {
            client: client.to_owned(),
        }),
    }
}

/// The compiled `calls`, the last one the app's new call and the others those of `own`, its waiting proposal: additions
/// applied, then the removals proposed, then the jobs started.
fn edit(
    mcp: &Mcp,
    client: &str,
    snap: &Snapshot,
    calls: &[ToolCall],
    out: Compiled,
    own: Option<&OpenProposal>,
) -> Value {
    let host = mcp.host();
    let at = calls.len() - 1;
    let compiled = |i: &usize| !out.refusals.iter().any(|r| r.call_index == *i);
    let mut said: Vec<String> = Vec::new();
    let mut revision = snap.revision;
    // The waiting proposal the additions' commit dropped, if one waited.
    let mut dropped: Option<String> = None;
    if let Some(additions) = &out.additions {
        // The music's refit is the host's to make, as for every edit of the person's: the app's edits are judged and
        // committed without it, so a shorter video cutting the music isn't a removal of the app's.
        let own = additions.ops.len().saturating_sub(out.music_refit);
        let edits = Transaction {
            ops: additions.ops[..own].to_vec(),
            ..additions.clone()
        };
        let tx = from_app(&edits, client);
        if tx.needs_review(Some(&snap.project)) {
            return refused(words::REMOVES);
        }
        let applied_at = mcp.now();
        if let Err(paced) = mcp.limits().applied(client, applied_at) {
            return refused(&words::paced(client, paced));
        }
        match host.apply(client, snap.revision, &tx) {
            Ok(applied) => (revision, dropped) = (applied.revision, applied.dropped),
            Err(e) => {
                // Nothing applied: the batch's place in the app's pace, and all apps', goes back.
                mcp.limits().unapplied(client, applied_at);
                return refused(&host_error(host, client, e));
            }
        }
    }
    let changed = revision != snap.revision;
    if changed {
        let applied = (0..calls.len())
            .filter(|i| !out.proposed_calls.contains(i) && compiled(i) && adds(&calls[*i]));
        said.push(fill(
            words::DONE,
            &[("tools", &names(applied.map(|i| &calls[i])))],
        ));
        // What the additions did beyond what was asked, or left as they were: a new speed that shortens a transition, a
        // camera that is its own layer.
        said.extend(out.notes.iter().cloned());
    }

    // The commit dropped no proposal of the app's: the person decided on it while this call ran (a Discard), and their
    // decision stands. Its removals aren't proposed again.
    let decided = changed && own.is_some_and(|o| dropped.as_deref() != Some(o.id.as_str()));
    if decided {
        said.push(words::DECIDED.to_owned());
    }
    // The app's own proposal stands as it is, unless the call adds to it or the additions changed the project (which
    // dropped it, and it waits again on top of them).
    let proposal = (out.proposal)
        .filter(|_| !decided && (own.is_none() || changed || out.proposed_calls.contains(&at)));
    let mut proposed_as: Option<(String, Us)> = None;
    if let Some(proposal) = proposal {
        let proposed: Vec<ToolCall> = (out.proposed_calls.iter())
            .map(|&i| calls[i].clone())
            .collect();
        let (removed_us, ranges) = (proposal.removed_us, proposal.ranges.clone());
        let ops = proposal.tx.ops.clone();
        // A commit drops the open proposal; until one, the new proposal takes its place.
        let replacing = if changed {
            None
        } else {
            own.map(|p| p.id.as_str())
        };
        match host.propose(client, revision, &proposed, proposal, replacing) {
            Ok(id) => {
                let tools = names(proposed.iter());
                said.push(if removed_us > 0 {
                    fill(
                        words::PROPOSED,
                        &[
                            ("tools", &tools),
                            ("removed", &secs(removed_us)),
                            ("ranges", &ranges_said(&ranges)),
                        ],
                    )
                } else {
                    fill(words::PROPOSED_NO_FOOTAGE, &[("tools", &tools)])
                });
                // Why a change the app asked for waits: a speed that leaves a transition no room.
                said.extend(out.waits.iter().cloned());
                // Remembered only while the slot holds this proposal: another session of the app may have put its own
                // in its place since, and that one's calls are what its next call joins. The calls were made on the
                // snapshot's clips, even when they wait on top of additions now.
                let mut own = mcp.own();
                if host.open_proposal().is_some_and(|p| p.id == id) {
                    *own = Some(Own {
                        client: client.to_owned(),
                        id: id.clone(),
                        calls: proposed,
                        key: timeline_key(&snap.project),
                        ops,
                    });
                }
                drop(own);
                proposed_as = Some((id, removed_us));
            }
            // The additions are in: the answer says so, and that the removals weren't proposed.
            // When someone else's proposal waits now, the app is told to wait for the person, not to retry.
            Err(_) if changed => {
                said.push(match someone_else(host.open_proposal().as_ref(), client) {
                    Some(who) => waiting_other(&who),
                    None => words::NOT_PROPOSED.to_owned(),
                })
            }
            // The person decided on the app's own proposal while this call ran (a Discard): their decision stands.
            Err(HostError::Moved) if own.is_some_and(|o| decided_on(host, client, &o.id)) => {
                return refused(words::DECIDED);
            }
            Err(e) => return refused(&host_error(host, client, e)),
        }
    }

    let mut started = false;
    let mut not_started: Vec<String> = Vec::new();
    for job in &out.jobs {
        match start(mcp, client, job) {
            Ok(()) => started = true,
            Err(why) if !not_started.contains(&why) => not_started.push(why),
            Err(_) => {}
        }
    }
    if started {
        let makers = (0..calls.len()).filter(|i| compiled(i) && makes_jobs(&calls[*i]));
        said.push(fill(
            words::STARTED,
            &[("tools", &names(makers.map(|i| &calls[i])))],
        ));
    }
    if !changed && proposed_as.is_none() && !started && !not_started.is_empty() {
        return refused(&not_started.join(" "));
    }
    said.extend(not_started);

    let structured = match proposed_as {
        Some((id, removed_us)) => json!({
            "outcome": "proposed",
            "revision": revision,
            "proposalId": id,
            "removedSec": removed_us as f64 / 1e6,
        }),
        None if !changed && started => json!({"outcome": "started"}),
        None => json!({"outcome": "done", "revision": revision}),
    };
    if said.is_empty() {
        said.push(words::UNCHANGED.to_owned());
    }
    answer(&said, structured)
}

/// Starts `job` for `client`, within the run's allowance and the app's pace of jobs, only through the host and its own
/// checks; else why not.
fn start(mcp: &Mcp, client: &str, job: &Job) -> Result<(), String> {
    let host = mcp.host();
    let usd = host.estimate_usd(job);
    match mcp.limits().paid(client, mcp.now(), usd) {
        Ok(()) => {}
        Err(paced) => return Err(words::paced(client, paced)),
    }
    host.start_paid(client, job).map_err(|e| {
        // Nothing was spent.
        mcp.limits().unpaid(usd);
        host_error(host, client, e)
    })
}

/// `findMoments`, compiled alone on the project as it is: the moments shown to the person, nothing else.
fn show(host: &dyn Host, client: &str, snap: &Snapshot, call: ToolCall) -> Value {
    let out = compile_for_app(&snap.project, &snap.inputs, std::slice::from_ref(&call));
    if !out.refusals.is_empty() {
        return refused(&reasons(&out.refusals));
    }
    let shown = json!({"outcome": "shown"});
    if out.moments.is_empty() {
        return answer(&[words::NO_MOMENTS.to_owned()], shown);
    }
    host.show_moments(client, &out.moments);
    let ranges: Vec<RangeUs> = out.moments.iter().map(|m: &Moment| m.range).collect();
    let text = match ranges.len() {
        1 => fill(words::SHOWN_ONE, &[("ranges", &ranges_said(&ranges))]),
        n => fill(
            words::SHOWN,
            &[("n", &n.to_string()), ("ranges", &ranges_said(&ranges))],
        ),
    };
    answer(&[text], shown)
}

/// Who made the proposal waiting, when it isn't `client`'s own: an app's name, or Klipolo's chat.
fn someone_else(open: Option<&OpenProposal>, client: &str) -> Option<String> {
    let open = open?;
    match open.from.as_deref() {
        Some(app) if app == client => None,
        Some(app) => Some(app.to_owned()),
        None => Some(words::KLIPOLO_CHAT.to_owned()),
    }
}

fn waiting_other(who: &str) -> String {
    fill(words::WAITING_OTHER, &[("who", who)])
}

/// Whether the person applied or discarded `client`'s proposal `id`.
fn decided_on(host: &dyn Host, client: &str, id: &str) -> bool {
    match host.outcome(client) {
        ProposalOutcome::Applied { id: decided } | ProposalOutcome::Discarded { id: decided } => {
            decided == id
        }
        _ => false,
    }
}

/// What the agent reads when the host refused. A `Moved` while someone else's proposal now waits says so, so the
/// agent waits for the person rather than retrying.
fn host_error(host: &dyn Host, client: &str, e: HostError) -> String {
    match e {
        HostError::NoProject => words::NO_PROJECT.to_owned(),
        HostError::Moved => match someone_else(host.open_proposal().as_ref(), client) {
            Some(who) => waiting_other(&who),
            None => words::MOVED.to_owned(),
        },
        HostError::ReadOnly => words::READ_ONLY.to_owned(),
        HostError::PushesOut => words::PUSHES_OUT.to_owned(),
        HostError::RedoWaits => words::REDO_WAITS.to_owned(),
        HostError::OverShare => words::OVER_SHARE.to_owned(),
        HostError::NotYours => words::NOT_YOURS.to_owned(),
        HostError::NothingToUndo => words::NOTHING_TO_UNDO.to_owned(),
        HostError::NoCredit => words::NO_CREDIT.to_owned(),
        HostError::LowCredit => words::LOW_CREDIT.to_owned(),
        HostError::Refused(why) => why,
    }
}

/// The refusals' reasons, one after another.
fn reasons<'a>(refusals: impl IntoIterator<Item = &'a Refusal>) -> String {
    let said: Vec<String> = refusals.into_iter().map(reason).collect();
    said.join(" ")
}

/// How chat ends a refusal of a time that stands on nothing: what counts for chat's model.
const CHAT_TIMES_FROM: &str = ", the selection, the playhead, the video's start or end, or a time the person said. Use \
a time from the context.";
/// How chat points its model at the person: a tool apps aren't offered.
const CHAT_ASK: &str = " with askUser";

/// A refusal's reason, as a sentence an app can act on. Chat words some for its own model and the person in its panel;
/// an app reads them in its own terms:
/// - a clip not analyzed yet is the person's to analyze (`NOT_ANALYZED`); one still being analyzed is for later
///   (`LISTENING`, `WATCHING`); roughly timed words are the person's to time again with the button above the video
///   while it shows (`ROUGH_WORDS`), and only said to be rough when it doesn't (`ROUGH_WORDS_STAY`): chat's own say
///   "ask me" to the person;
/// - a time must stand on the reads an app has, never its own words (`TIMES_FROM`);
/// - it asks its own person, not through `askUser`.
///
/// One for the person names the clip it is about: "Klipolo is still listening to this clip (c2). Try again ..."
fn reason(r: &Refusal) -> String {
    let own = match r.reason.as_str() {
        ANALYZE_FIRST => Some(words::NOT_ANALYZED),
        STILL_LISTENING => Some(words::LISTENING),
        STILL_WATCHING => Some(words::WATCHING),
        ROUGH_FILLERS => Some(words::ROUGH_WORDS),
        ROUGH_FILLERS_STAY => Some(words::ROUGH_WORDS_STAY),
        _ => None,
    };
    if r.audience == Audience::Person
        && let Some(own) = own
    {
        return match &r.clip {
            Some(clip) => fill(own, &[("clip", clip)]),
            None => own.replace(" ({clip})", ""),
        };
    }
    let said = (r.reason)
        .replace(CHAT_TIMES_FROM, words::TIMES_FROM)
        .replace(CHAT_ASK, "");
    match (r.audience, &r.clip) {
        (Audience::Person, Some(clip)) => format!("{} ({clip}).", said.trim_end_matches('.')),
        (Audience::Person, None) if !said.ends_with(['.', '?', '!']) => format!("{said}."),
        _ => said,
    }
}

/// Whether `call` makes a job for a runner or an image maker.
fn makes_jobs(call: &ToolCall) -> bool {
    matches!(
        call,
        ToolCall::MakeSticker { .. }
            | ToolCall::MakeTitleCard { .. }
            | ToolCall::FollowSubject { .. }
            | ToolCall::BlurObjects { .. }
    )
}

/// Whether `call` can make additions (what `findMoments` and the jobs make is not an edit to apply).
fn adds(call: &ToolCall) -> bool {
    !matches!(
        call,
        ToolCall::FindMoments { .. }
            | ToolCall::MakeSticker { .. }
            | ToolCall::FollowSubject { .. }
            | ToolCall::BlurObjects { .. }
            | ToolCall::Undo
            | ToolCall::AskUser { .. }
            | ToolCall::CannotDo { .. }
    )
}

/// The tools' names, each once, in order: "cutRange, setFormat".
fn names<'a>(calls: impl Iterator<Item = &'a ToolCall>) -> String {
    let mut seen: Vec<&str> = Vec::new();
    for call in calls {
        if !seen.contains(&call.name()) {
            seen.push(call.name());
        }
    }
    seen.join(", ")
}

/// Timeline seconds to the millisecond, without trailing zeros: `13`, `49.5`, `144.04`.
fn secs(us: Us) -> String {
    let ms = (us.max(0) + 500) / 1000;
    let (whole, part) = (ms / 1000, ms % 1000);
    if part == 0 {
        whole.to_string()
    } else {
        format!("{whole}.{part:03}")
            .trim_end_matches('0')
            .to_owned()
    }
}

/// "0 to 13 s, 20 to 21.5 s", the first `RANGES_SAID`, then how many more.
fn ranges_said(ranges: &[RangeUs]) -> String {
    let mut said: Vec<String> = (ranges.iter().take(RANGES_SAID))
        .map(|r| format!("{} to {} s", secs(r.start), secs(r.end)))
        .collect();
    if ranges.len() > RANGES_SAID {
        said.push(format!("and {} more", ranges.len() - RANGES_SAID));
    }
    said.join(", ")
}

/// A result something was done in: `said`, one sentence after another.
fn answer(said: &[String], structured: Value) -> Value {
    json!({
        "content": [{"type": "text", "text": said.join(" ")}],
        "isError": false,
        "structuredContent": structured,
    })
}

/// A result nothing was done in, and why.
pub(crate) fn refused(why: &str) -> Value {
    json!({
        "content": [{"type": "text", "text": why}],
        "isError": true,
        "structuredContent": {"outcome": "refused"},
    })
}
