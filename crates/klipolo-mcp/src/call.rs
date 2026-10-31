//! An edit tool's call from an app: chat's own tool, read, checked and compiled as chat's are, then applied, proposed
//! or refused. In order:
//! 0. The app's pace of calls (`Limits::call`): `PACED` past it.
//! 1. `ToolCall::parse`: a call that doesn't fit its tool is refused with the parse error's sentence. `undo` undoes the
//!    app's own newest step and nothing else (`Host::undo_own`), once no one else's proposal waits.
//! 2. The open project (`NO_PROJECT` without one). A call naming times or clip refs needs the app to have read the
//!    project's main track as it is now (`Seen`): `READ_FIRST` when it never read, `STALE` when the clips moved since.
//! 3. `findMoments` only points the person at moments (it is read-only): it compiles alone and is shown.
//! 4. The proposal slot, shared with Klipolo's chat: while a proposal that isn't the app's own waits, every edit is
//!    refused (`WAITING_OTHER`). The app's own absorbs the call: it compiles after the calls the proposal holds
//!    (`Own`), on the revision the proposal was made on.
//! 5. `compile`, with no typed message: an app's words are never the person's times. A refusal of the call is the
//!    tool's error, with its reason, and nothing changes.
//! 6. Additions apply at once as one undo step the app made (`from_app`), checked again first to take nothing away,
//!    within the app's pace of applied batches (`Limits::applied`).
//!    Removals become a proposal for the person, never applied here: in place of the app's own, or, once additions
//!    applied (which drops it, as any change does), on top of them.
//! 7. Each job that costs money passes the run's allowance and the app's pace of jobs (`Limits::paid`), then only
//!    `Host::start_paid` starts it, after the app's own checks. Nothing here calls a model.
//!
//! The only commit is `Host::apply` of additions that `needs_review` passes, and the only other change the app's own
//! undo: no path here cuts footage.
//!
//! A result is one text and `structuredContent`: `{"outcome": "done" | "proposed" | "shown" | "started" | "refused",
//! "revision"?, "proposalId"?, "removedSec"?}`, the outcome what changed most (a proposal, then the project, then a
//! job started). `isError` is true exactly when nothing was done; when part was, the text says what wasn't.

use std::time::Instant;

use klipolo_chat::{
    Audience, Compiled, FunctionCall, Job, Moment, Refusal, ToolCall, compile, timeline_key,
};
use klipolo_model::Transaction;
use klipolo_model::coded::ErrorParam;
use klipolo_model::ops::Origin;
use klipolo_model::time::{RangeUs, Us};
use serde_json::{Value, json};

use crate::host::{Host, HostError, OpenProposal, Snapshot};
use crate::limits::Paced;
use crate::rpc::Mcp;
use crate::session::{Seen, Session};
use crate::words::{self, fill};

/// The most ranges a result names; the rest are counted (`getProposal` lists them all).
const RANGES_SAID: usize = 10;

/// An app's waiting proposal as Klipolo made it, so the app's next call joins it: whose, its id and the calls in it.
/// One proposal waits at a time, so one is remembered; it holds only while a proposal with its id waits.
#[derive(Clone, Debug)]
pub(crate) struct Own {
    client: String,
    id: String,
    calls: Vec<ToolCall>,
}

/// What `call` did, as a tool result.
pub fn run(mcp: &Mcp, session: &mut Session, call: &FunctionCall) -> Value {
    let client = session.client.as_str();
    let host = mcp.host();
    if mcp.limits().call(client, Instant::now()).is_err() {
        return refused(&paced(client));
    }
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
        Some(open) => match joined(mcp, client, &open, snap.revision) {
            Some(calls) => (Some(open), calls),
            // What it holds isn't known here (the server started again since): it waits for the person like any other.
            None => return refused(&waiting_other(client)),
        },
    };
    calls.push(new);
    let at = calls.len() - 1;
    let out = compile(&snap.project, &snap.inputs, &calls);
    let mine: Vec<&Refusal> = out.refusals.iter().filter(|r| r.call_index == at).collect();
    if !mine.is_empty() {
        return refused(&reasons(mine));
    }
    edit(mcp, client, &snap, &calls, out, own.as_ref())
}

/// Undoes `client`'s own newest step, unless someone else's proposal waits (the person's, or another app's).
fn undo(host: &dyn Host, client: &str) -> Value {
    if let Some(who) = someone_else(host.open_proposal().as_ref(), client) {
        return refused(&waiting_other(&who));
    }
    match host.undo_own(client) {
        Ok(undone) => answer(
            &[words::UNDONE.to_owned()],
            json!({"outcome": "done", "revision": undone.revision}),
        ),
        Err(e) => refused(&host_error(host, client, e)),
    }
}

/// The calls in `open`, `client`'s own waiting proposal, when Klipolo remembers making it, on `revision`.
fn joined(mcp: &Mcp, client: &str, open: &OpenProposal, revision: u64) -> Option<Vec<ToolCall>> {
    let own = mcp.own();
    let own = own.as_ref()?;
    (own.id == open.id && own.client == client && open.base_revision == revision)
        .then(|| own.calls.clone())
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
    if let Some(additions) = &out.additions {
        let tx = from_app(additions, client);
        if tx.needs_review(Some(&snap.project)) {
            return refused(words::REMOVES);
        }
        if mcp.limits().applied(client, Instant::now()).is_err() {
            return refused(&paced(client));
        }
        match host.apply(client, snap.revision, &tx) {
            Ok(applied) => revision = applied.revision,
            Err(e) => return refused(&host_error(host, client, e)),
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
    }

    // The app's own proposal stands as it is, unless the call adds to it or the additions changed the project (which
    // dropped it).
    let proposal =
        (out.proposal).filter(|_| own.is_none() || changed || out.proposed_calls.contains(&at));
    // An earlier call of its proposal that no longer compiles (it stood on a selection gone since) is left out of what
    // takes the proposal's place: the app hears which, and why.
    let mut left_out: Vec<String> = if own.is_some() && (changed || proposal.is_some()) {
        (out.refusals.iter())
            .filter(|r| r.call_index < at)
            .map(|r| fill(words::LEFT_OUT, &[("reason", &reason(r))]))
            .collect()
    } else {
        Vec::new()
    };
    let mut proposed_as: Option<(String, Us)> = None;
    if let Some(proposal) = proposal {
        let proposed: Vec<ToolCall> = (out.proposed_calls.iter())
            .map(|&i| calls[i].clone())
            .collect();
        let (removed_us, ranges) = (proposal.removed_us, proposal.ranges.clone());
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
                *mcp.own() = Some(Own {
                    client: client.to_owned(),
                    id: id.clone(),
                    calls: proposed,
                });
                proposed_as = Some((id, removed_us));
            }
            // The additions are in: the answer says so, and that the removals weren't proposed.
            Err(_) if changed => {
                said.push(words::NOT_PROPOSED.to_owned());
                left_out.clear();
            }
            Err(e) => return refused(&host_error(host, client, e)),
        }
    }
    said.extend(left_out);

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
    match mcp.limits().paid(client, Instant::now(), usd) {
        Ok(()) => {}
        Err(Paced::OverCap) => return Err(words::OVER_CAP.to_owned()),
        Err(Paced::Wait) => return Err(paced(client)),
    }
    host.start_paid(client, job).map_err(|e| {
        // Nothing was spent.
        mcp.limits().unpaid(usd);
        host_error(host, client, e)
    })
}

/// `findMoments`, compiled alone on the project as it is: the moments shown to the person, nothing else.
fn show(host: &dyn Host, client: &str, snap: &Snapshot, call: ToolCall) -> Value {
    let out = compile(&snap.project, &snap.inputs, std::slice::from_ref(&call));
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

fn paced(client: &str) -> String {
    fill(words::PACED, &[("client", client)])
}

fn waiting_other(who: &str) -> String {
    fill(words::WAITING_OTHER, &[("who", who)])
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
        HostError::OverShare => words::OVER_SHARE.to_owned(),
        HostError::NotYours => words::NOT_YOURS.to_owned(),
        HostError::NothingToUndo => words::NOTHING_TO_UNDO.to_owned(),
        HostError::NoCredit => words::NO_CREDIT.to_owned(),
        HostError::Refused(why) => why,
    }
}

/// The refusals' reasons, one after another.
fn reasons<'a>(refusals: impl IntoIterator<Item = &'a Refusal>) -> String {
    let said: Vec<String> = refusals.into_iter().map(reason).collect();
    said.join(" ")
}

/// A refusal's reason, as a sentence. One for the person names the clip it is about: "Klipolo is still listening to
/// this clip (c2)."
fn reason(r: &Refusal) -> String {
    match (r.audience, &r.clip) {
        (Audience::Person, Some(clip)) => format!("{} ({clip}).", r.reason.trim_end_matches('.')),
        (Audience::Person, None) if !r.reason.ends_with(['.', '?', '!']) => {
            format!("{}.", r.reason)
        }
        _ => r.reason.clone(),
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
fn refused(why: &str) -> Value {
    json!({
        "content": [{"type": "text", "text": why}],
        "isError": true,
        "structuredContent": {"outcome": "refused"},
    })
}
