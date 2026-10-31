//! What an app reads, each a resource and a read-only tool alike (tool-only clients must see the transcript too):
//! - `klipolo://project/summary` (`getSummary`): `Revision {n}`, then the context's Project and Now sections;
//! - `klipolo://context` (`getContext`): `Revision {n}`, then exactly the text Klipolo's chat model reads;
//! - `klipolo://transcript` (`getTranscript`) and `klipolo://transcript/{from}-{to}`: every line, in pages (`pages`);
//! - `klipolo://proposal` (`getProposal`): JSON, what became of this app's proposal (`ProposalOutcome`).
//!
//! All built from the host's snapshot when read; nothing is kept but what the session read (`Seen`). Reading the
//! summary, the context or the transcript records the project and its `timeline_key`, which a call naming times or
//! clip refs needs to match.

use klipolo_chat::{build_context, full_transcript, timeline_key};
use klipolo_model::time::{MAX_TIME_US, Us, secs_to_us};
use serde_json::{Value, json};

use crate::host::{Host, Snapshot};
use crate::pages::{self, TEMPLATE, parse_time};
use crate::session::{Seen, Session};
use crate::words;

pub const SUMMARY: &str = "klipolo://project/summary";
pub const CONTEXT: &str = "klipolo://context";
pub const TRANSCRIPT: &str = "klipolo://transcript";
pub const PROPOSAL: &str = "klipolo://proposal";

const TEXT: &str = "text/plain";
const JSON: &str = "application/json";

/// `resources/list`'s resources.
pub fn list() -> Value {
    json!([
        {"uri": SUMMARY, "name": "summary", "title": words::SUMMARY_TITLE, "mimeType": TEXT},
        {"uri": CONTEXT, "name": "context", "title": words::CONTEXT_TITLE, "mimeType": TEXT},
        {"uri": TRANSCRIPT, "name": "transcript", "title": words::TRANSCRIPT_TITLE, "mimeType": TEXT},
        {"uri": PROPOSAL, "name": "proposal", "title": words::PROPOSAL_TITLE, "mimeType": JSON},
    ])
}

/// `resources/templates/list`'s templates.
pub fn templates() -> Value {
    json!([{
        "uriTemplate": format!("{TEMPLATE}{{from}}-{{to}}"),
        "name": "transcript-page",
        "title": words::PAGE_TITLE,
        "description": words::PAGE_DESCRIPTION,
        "mimeType": TEXT,
    }])
}

/// A read's answer: its text, and whether it is the agent's to fix (no project, times it can't use).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Read {
    pub text: String,
    pub refused: bool,
}

impl Read {
    fn ok(text: String) -> Self {
        Self {
            text,
            refused: false,
        }
    }

    fn refused(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            refused: true,
        }
    }
}

/// Whether `uri` names a resource Klipolo has (a transcript page's times are read later).
pub fn known(uri: &str) -> bool {
    matches!(uri, SUMMARY | CONTEXT | TRANSCRIPT | PROPOSAL) || uri.starts_with(TEMPLATE)
}

/// Resource `uri` for `session`: its MIME type and text; None when there is no such resource.
pub fn read(host: &dyn Host, session: &mut Session, uri: &str) -> Option<(&'static str, String)> {
    let read = match uri {
        SUMMARY => summary(host, session),
        CONTEXT => context(host, session),
        TRANSCRIPT => transcript(host, session, None, None),
        PROPOSAL => return Some((JSON, proposal(host, session).to_string())),
        _ => {
            let range = uri.strip_prefix(TEMPLATE)?;
            match range
                .split_once('-')
                .and_then(|(a, b)| Some((parse_time(a)?, parse_time(b)?)))
            {
                Some((from, to)) => transcript(host, session, Some(from), Some(to)),
                None => Read::refused(format!(
                    "Ask for a page as {TEMPLATE}<from>-<to>, with timeline times like 1:05, 1:02:03 or 75.5."
                )),
            }
        }
    };
    Some((TEXT, read.text))
}

/// The open project, recording what `session` read; `NO_PROJECT` when none is open.
fn seen(host: &dyn Host, session: &mut Session) -> Result<Snapshot, Read> {
    let snap = host
        .snapshot()
        .ok_or_else(|| Read::refused(words::NO_PROJECT))?;
    session.seen = Some(Seen {
        project_id: snap.project.id.clone(),
        key: timeline_key(&snap.project),
    });
    Ok(snap)
}

/// The context chat's model would read now.
fn chat_context(snap: &Snapshot) -> klipolo_chat::ChatContext {
    let i = &snap.inputs;
    build_context(
        &snap.project,
        &i.speech,
        &snap.turns,
        &i.scenes,
        &i.view,
        &i.library,
    )
}

pub fn summary(host: &dyn Host, session: &mut Session) -> Read {
    match seen(host, session) {
        Ok(snap) => {
            let ctx = chat_context(&snap);
            let part = |heading| ctx.section(heading).unwrap_or_default();
            Read::ok(format!(
                "Revision {}\n\n{}{}",
                snap.revision,
                part("Project"),
                part("Now")
            ))
        }
        Err(no) => no,
    }
}

pub fn context(host: &dyn Host, session: &mut Session) -> Read {
    match seen(host, session) {
        Ok(snap) => Read::ok(format!(
            "Revision {}\n\n{}",
            snap.revision,
            chat_context(&snap).text
        )),
        Err(no) => no,
    }
}

pub fn transcript(
    host: &dyn Host,
    session: &mut Session,
    from: Option<Us>,
    to: Option<Us>,
) -> Read {
    match seen(host, session) {
        Ok(snap) => {
            let full = full_transcript(&snap.project, &snap.inputs.speech, &snap.turns);
            Read::ok(pages::page(&full, from, to))
        }
        Err(no) => no,
    }
}

/// `getTranscript`'s times, in seconds or null: refused when not a number of seconds from 0, or not in order.
pub fn transcript_times(args: &Value) -> Result<(Option<Us>, Option<Us>), Read> {
    let time = |field: &str| -> Result<Option<Us>, Read> {
        match args.get(field) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => v
                .as_f64()
                .filter(|s| s.is_finite() && *s >= 0.0 && secs_to_us(*s) <= MAX_TIME_US)
                .map(|s| Some(secs_to_us(s)))
                .ok_or_else(|| {
                    Read::refused(format!(
                        "getTranscript: {field} must be a number of timeline seconds, 0 or more, or null."
                    ))
                }),
        }
    };
    let (from, to) = (time("fromSec")?, time("toSec")?);
    if let (Some(a), Some(b)) = (from, to)
        && a >= b
    {
        return Err(Read::refused(
            "getTranscript: fromSec must be before toSec.",
        ));
    }
    Ok((from, to))
}

/// What became of `session`'s app's proposal, as JSON.
pub fn proposal(host: &dyn Host, session: &Session) -> Value {
    serde_json::to_value(host.outcome(&session.client))
        .unwrap_or_else(|_| json!({"status": "none"}))
}
