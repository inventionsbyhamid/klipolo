//! An edit tool's call from an app: chat's own tool, read, checked and compiled as chat's are, then applied, proposed
//! or refused. In order:
//! 1. `ToolCall::parse`: a call that doesn't fit its tool is refused with the parse error's sentence.
//! 2. The open project (`NO_PROJECT` without one). A call naming times or clip refs needs the app to have read the
//!    project's main track as it is now (`Seen`): `READ_FIRST` when it never read, `STALE` when the clips moved since.
//! 3. `findMoments` only points the person at moments (it is read-only): it compiles alone and is shown.
//! 4. The proposal slot, shared with Klipolo's chat: while a proposal that isn't the app's own waits, every edit is
//!    refused (`WAITING_OTHER`).
//! 5. `compile`, with no typed message: an app's words are never the person's times. A refusal of the call is the
//!    tool's error, with every reason, and nothing changes.
//! 6. Additions apply at once as one undo step the app made (`from_app`), checked again first to take nothing away.
//!    Removals become a proposal for the person, never applied here.
//!
//! The only commit is `Host::apply` of additions that `needs_review` passes: no path here cuts footage.
//!
//! A result is one text and `structuredContent`: `{"outcome": "done" | "proposed" | "shown" | "refused", "revision"?,
//! "proposalId"?, "removedSec"?}`. `isError` is true exactly when nothing was done.

use klipolo_chat::{
    Audience, Compiled, FunctionCall, Moment, Refusal, ToolCall, compile, timeline_key,
};
use klipolo_model::Transaction;
use klipolo_model::coded::ErrorParam;
use klipolo_model::ops::Origin;
use klipolo_model::time::{RangeUs, Us};
use serde_json::{Value, json};

use crate::host::{Host, HostError, OpenProposal, Snapshot};
use crate::rpc::Mcp;
use crate::session::{Seen, Session};
use crate::words::{self, fill};

/// The most ranges a result names; the rest are counted (`getProposal` lists them all).
const RANGES_SAID: usize = 10;

/// What `call` did, as a tool result.
pub fn run(mcp: &Mcp, session: &mut Session, call: &FunctionCall) -> Value {
    let client = session.client.as_str();
    let host = mcp.host();
    let new = match ToolCall::parse(call) {
        Ok(new) => new,
        Err(e) => return refused(&e.to_string()),
    };
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
    if let Some(who) = someone_else(open.as_ref(), client) {
        return refused(&waiting_other(&who));
    }
    let own = open.filter(|p| p.from.as_deref() == Some(client));
    let calls = vec![new];
    let at = calls.len() - 1;
    let out = compile(&snap.project, &snap.inputs, &calls);
    if out.refusals.iter().any(|r| r.call_index == at) {
        return refused(&reasons(&out.refusals));
    }
    edit(host, client, &snap, &calls, out, own.as_ref())
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

/// The compiled `calls`, done: additions applied, then removals proposed (in place of `own`, the app's open proposal,
/// while the revision is the one it was made on).
fn edit(
    host: &dyn Host,
    client: &str,
    snap: &Snapshot,
    calls: &[ToolCall],
    out: Compiled,
    own: Option<&OpenProposal>,
) -> Value {
    let mut said: Vec<String> = Vec::new();
    let mut revision = snap.revision;
    if let Some(additions) = &out.additions {
        let tx = from_app(additions, client);
        if tx.needs_review(Some(&snap.project)) {
            return refused(words::REMOVES);
        }
        match host.apply(client, snap.revision, &tx) {
            Ok(applied) => revision = applied.revision,
            Err(e) => return refused(&host_error(host, client, e)),
        }
    }
    let changed = revision != snap.revision;
    if changed {
        let applied = (0..calls.len()).filter(|i| {
            !out.proposed_calls.contains(i)
                && !out.refusals.iter().any(|r| r.call_index == *i)
                && adds(&calls[*i])
        });
        said.push(fill(
            words::DONE,
            &[("tools", &names(applied.map(|i| &calls[i])))],
        ));
    }
    let Some(proposal) = out.proposal else {
        if said.is_empty() {
            said.push(words::UNCHANGED.to_owned());
        }
        return answer(&said, json!({"outcome": "done", "revision": revision}));
    };
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
            answer(
                &said,
                json!({
                    "outcome": "proposed",
                    "revision": revision,
                    "proposalId": id,
                    "removedSec": removed_us as f64 / 1e6,
                }),
            )
        }
        // The additions are in: the answer says so, and that the removals weren't proposed.
        Err(_) if changed => {
            said.push(words::NOT_PROPOSED.to_owned());
            answer(&said, json!({"outcome": "done", "revision": revision}))
        }
        Err(e) => refused(&host_error(host, client, e)),
    }
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

/// Every refusal's reason, in call order. One for the person names the clip it is about: "Klipolo is still listening
/// to this clip (c2)."
fn reasons(refusals: &[Refusal]) -> String {
    let said: Vec<String> = (refusals.iter())
        .map(|r| match (r.audience, &r.clip) {
            (Audience::Person, Some(clip)) => {
                format!("{} ({clip}).", r.reason.trim_end_matches('.'))
            }
            (Audience::Person, None) if !r.reason.ends_with(['.', '?', '!']) => {
                format!("{}.", r.reason)
            }
            _ => r.reason.clone(),
        })
        .collect();
    said.join(" ")
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
