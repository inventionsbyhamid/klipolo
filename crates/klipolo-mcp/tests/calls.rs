//! An app's edit calls, on a `FakeHost` holding the chat crate's two-clip fixture: c1 plays 0 to 13 s of the space
//! station Q&A and c2 13 to 144.04 s. Additions apply at once as one undo step the app made; removals become a
//! proposal and nothing is cut; a call that can't stand is refused with the reason, and nothing changes.
//!
//! Nothing is said, shown or cut between 48.46 and 51.5 s on the timeline, so a cut from 49.5 to 50.5 s is invented
//! unless the person selected it.

#[path = "support/agent.rs"]
mod agent;
#[path = "support/fake_host.rs"]
mod fake_host;

use agent::{Agent, assert_refused, outcome, request, text};
use fake_host::FakeHost;
use std::collections::HashSet;
use std::sync::Arc;

use klipolo_chat::{AnalysisState, FunctionCall, Job, Moment, Proposal, ToolCall, View, compile};
use klipolo_mcp::Host;
use klipolo_mcp::call::from_app;
use klipolo_mcp::rpc::{Handled, Mcp};
use klipolo_mcp::words::{
    DECIDED, DONE, KLIPOLO_CHAT, MOVED, NO_CREDIT, NO_MOMENTS, NOT_PROPOSED, NOT_YOURS,
    NOTHING_TO_UNDO, OVER_SHARE, OWN_CHANGED, OWN_MOVED, OWN_WAITING, PROPOSED_NO_FOOTAGE,
    PUSHES_OUT, READ_FIRST, REDO_WAITS, ROUGH_WORDS_STAY, STALE, STARTED, UNCHANGED, UNDO_DROPPED,
    UNDONE, WAITING_OTHER, fill,
};
use klipolo_model::Transaction;
use klipolo_model::coded::ErrorParam;
use klipolo_model::history::{HistoryLimits, Maker};
use klipolo_model::ops::{Op, Origin};
use klipolo_model::project::FormatKind;
use klipolo_model::speech::WordKind;
use klipolo_model::time::RangeUs;
use klipolo_model::timeline::ItemContent;
use serde_json::{Value, json};

fn span(a: f64, b: f64) -> RangeUs {
    RangeUs {
        start: (a * 1e6).round() as i64,
        end: (b * 1e6).round() as i64,
    }
}

/// A removal compiled on the host's project as it is now: all of c1.
fn cut_c1(host: &FakeHost) -> (ToolCall, Proposal) {
    let call = ToolCall::CutRange {
        range: span(0.0, 13.0),
    };
    let snap = host.snapshot().expect("a project");
    let out = compile(&snap.project, &snap.inputs, std::slice::from_ref(&call));
    (call, out.proposal.expect("a proposal"))
}

fn set_format(format: &str, keep_others: bool) -> Value {
    json!({"format": format, "keepOthers": keep_others})
}

#[test]
fn addition_applies_at_once_as_one_step_made_by_the_client() {
    let host = FakeHost::two_clips();
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    let before = host.revision();
    let result = agent.call("setFormat", set_format("square", true));
    assert_eq!(result["isError"], false, "{result}");
    assert_eq!(text(&result), fill(DONE, &[("tools", "setFormat")]));
    assert_eq!(
        text(&result),
        "Done in Klipolo (setFormat). The person can undo it there."
    );
    assert_eq!(
        result["structuredContent"],
        json!({"outcome": "done", "revision": before + 1})
    );
    // One step, made by the app, holding the format.
    assert_eq!(host.revision(), before + 1);
    let project = host.project();
    assert_eq!(project.settings.primary_format, FormatKind::Square);
    assert_eq!(
        project.settings.formats,
        [
            FormatKind::Landscape,
            FormatKind::Portrait,
            FormatKind::Square
        ]
    );
    {
        let s = host.state();
        let doc = s.doc.as_ref().unwrap();
        assert_eq!(doc.undo_steps(), 1);
        let newest = doc.undo_view().last().unwrap();
        assert_eq!(newest.maker, &Maker::App("Agent".into()));
        // As chat labels it, with the app's name, from the app.
        let [(client, tx)] = s.applied.as_slice() else {
            panic!("one batch: {:?}", s.applied)
        };
        assert_eq!(client, "Agent");
        assert_eq!(tx.label, "chat.setFormat");
        assert_eq!(
            tx.origin,
            Some(Origin::External {
                client: "Agent".into()
            })
        );
        let params = tx.params.as_ref().expect("params");
        assert_eq!(params.get("client"), Some(&ErrorParam::from("Agent")));
        assert_eq!(params.get("format"), Some(&ErrorParam::from("square")));
    }
    // Asked again, nothing changes: no step.
    let again = agent.call("setFormat", set_format("square", true));
    assert_eq!(again["isError"], false);
    assert_eq!(text(&again), UNCHANGED);
    assert_eq!(
        again["structuredContent"],
        json!({"outcome": "done", "revision": before + 1})
    );
    assert_eq!(host.revision(), before + 1);
}

#[test]
fn an_apps_batch_is_judged_as_an_apps_not_the_persons() {
    let host = FakeHost::two_clips();
    let cut = Transaction {
        label: "chat.cutRange".into(),
        ops: vec![Op::DeleteRanges {
            ranges: vec![span(1.0, 2.0)],
        }],
        origin: Some(Origin::Chat),
        ..Transaction::default()
    };
    let from = from_app(&cut, "Agent");
    assert_eq!(
        (from.label.as_str(), &from.ops),
        ("chat.cutRange", &cut.ops)
    );
    assert_eq!(
        from.origin,
        Some(Origin::External {
            client: "Agent".into()
        })
    );
    // A removal from an app always needs the person's review: the check before an app's batch applies holds.
    assert!(from.needs_review(Some(&host.project())));
    assert!(from.needs_review(None));
}

#[test]
fn removal_becomes_a_proposal_and_nothing_is_cut() {
    let host = FakeHost::two_clips();
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    agent.read();
    let (before, project) = (host.revision(), host.project());
    let result = agent.cut(0.0, 13.0);
    assert_eq!(result["isError"], false, "{result}");
    assert_eq!(
        text(&result),
        "Proposed in Klipolo (cutRange): 13 s of footage would go (0 to 13 s). Nothing is cut until the person \
         clicks Apply in Klipolo's chat panel. Call getProposal to see what they decided."
    );
    // Nothing changed: the removal waits in the chat panel as the app's.
    assert_eq!(host.revision(), before);
    assert_eq!(host.project(), project);
    let slot = host.state().slot.clone().expect("a proposal waits");
    assert_eq!(slot.from.as_deref(), Some("Agent"));
    assert_eq!(slot.base_revision, before);
    assert_eq!(
        slot.calls,
        [ToolCall::CutRange {
            range: span(0.0, 13.0)
        }]
    );
    assert_eq!(slot.proposal.ranges, [span(0.0, 13.0)]);
    assert_eq!(
        result["structuredContent"],
        json!({"outcome": "proposed", "revision": before, "proposalId": slot.id, "removedSec": 13.0})
    );
    assert_eq!(host.state().counts.applies, 0);
    // The app sees it waiting.
    let waiting = agent.call("getProposal", json!({}));
    assert_eq!(waiting["structuredContent"]["status"], "waiting");
    assert_eq!(waiting["structuredContent"]["id"], json!(slot.id));
    assert_eq!(waiting["structuredContent"]["from"], "Agent");
}

#[test]
fn invented_cut_comes_back_as_a_tool_error_with_the_reason() {
    let host = FakeHost::two_clips();
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    agent.read();
    let before = host.revision();
    let result = agent.cut(49.5, 50.5);
    assert_eq!(result["isError"], true, "{result}");
    assert_eq!(outcome(&result), "refused");
    assert!(
        text(&result).starts_with(
            "cutRange: startSec 49.5 is not within 0.3 s of a word, a pause, a scene, a clip's start or end"
        ),
        "{result}"
    );
    // Said for an app: its own words are no time, and it reads times from the reads it has.
    assert!(
        text(&result).ends_with(
            "a clip's start or end, the person's selection or playhead in Klipolo, or the video's start or end. Use a \
             time from getContext or getTranscript."
        ),
        "{result}"
    );
    assert!(!text(&result).contains("a time the person said"));
    assert_eq!(host.revision(), before);
    assert!(host.state().slot.is_none());
    let counts = host.state().counts;
    assert_eq!((counts.applies, counts.proposes), (0, 0));
}

#[test]
fn agent_text_is_never_typed_time_evidence() {
    let host = FakeHost::two_clips();
    // Even if the app's own words reached the snapshot as the person's message, they would be no evidence.
    host.state().message = "cut 49.5 to 50.5".into();
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    agent.read();
    let result = agent.cut(49.5, 50.5);
    assert_eq!(result["isError"], true, "{result}");
    assert!(
        text(&result).starts_with("cutRange: startSec 49.5 is not within 0.3 s of"),
        "{result}"
    );
    // Times in the app's own arguments are none either.
    let found = agent.call(
        "findMoments",
        json!({
            "query": "the cut from 49.5 to 50.5",
            "ranges": [{"startSec": 49.5, "endSec": 50.5, "label": "49.5 to 50.5"}],
        }),
    );
    assert_eq!(found["isError"], true, "{found}");
    assert!(host.state().slot.is_none());
    assert!(host.state().moments.is_empty());
}

#[test]
fn selection_in_klipolo_is_evidence() {
    let host = FakeHost::two_clips();
    host.set_view(View {
        playhead_us: 0,
        selection: Some(span(49.5, 50.5)),
    });
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    agent.read();
    let result = agent.cut(49.5, 50.5);
    assert_eq!(result["isError"], false, "{result}");
    assert_eq!(outcome(&result), "proposed");
    assert!(
        text(&result).starts_with(
            "Proposed in Klipolo (cutRange): 1 s of footage would go (49.5 to 50.5 s)."
        ),
        "{result}"
    );
    let slot = host.state().slot.clone().expect("a proposal waits");
    assert_eq!(slot.proposal.ranges, [span(49.5, 50.5)]);
}

#[test]
fn another_proposal_waiting_refuses_every_edit() {
    let host = FakeHost::two_clips();
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    agent.read();
    let (_, proposal) = cut_c1(&host);
    let chat = host.chat_proposes(proposal);
    let (before, project) = (host.revision(), host.project());
    let chats = fill(WAITING_OTHER, &[("who", KLIPOLO_CHAT)]);
    assert_eq!(
        chats,
        "A proposal from Klipolo's chat is waiting for the person in Klipolo. Wait until they apply or discard it \
         (call getProposal), then try again."
    );
    for (tool, arguments) in [
        ("setFormat", set_format("square", true)),
        ("setCaptions", json!({"on": false, "style": null})),
        ("cutRange", json!({"startSec": 0, "endSec": 13})),
        ("removeFillers", json!({})),
        ("undo", json!({})),
    ] {
        assert_refused(&agent.call(tool, arguments), &chats);
    }
    // Another app's proposal holds every edit back the same way.
    host.person_discards();
    let (call, proposal) = cut_c1(&host);
    host.propose("Other app", before, &[call], proposal, None)
        .expect("the other app proposes");
    let others = fill(WAITING_OTHER, &[("who", "Other app")]);
    assert_refused(
        &agent.call("setFormat", set_format("square", true)),
        &others,
    );
    assert_refused(&agent.cut(0.0, 13.0), &others);
    assert_refused(&agent.call("undo", json!({})), &others);
    // Nothing changed, and the other app's proposal still waits.
    assert_eq!((host.revision(), host.project()), (before, project));
    let counts = host.state().counts;
    assert_eq!((counts.applies, counts.undos), (0, 0));
    let open = host.open_proposal().expect("still waiting");
    assert_eq!(open.from.as_deref(), Some("Other app"));
    assert_ne!(open.id, chat);
}

#[test]
fn reads_still_work_while_a_proposal_waits() {
    let host = FakeHost::two_clips();
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    let (_, proposal) = cut_c1(&host);
    let id = host.chat_proposes(proposal);
    for read in ["getSummary", "getContext", "getTranscript"] {
        let result = agent.call(read, json!({}));
        assert_eq!(result["isError"], false, "{read}: {result}");
    }
    let waiting = agent.call("getProposal", json!({}));
    assert_eq!(waiting["structuredContent"]["status"], "waiting");
    assert_eq!(waiting["structuredContent"]["id"], json!(id));
    assert_eq!(waiting["structuredContent"]["from"], KLIPOLO_CHAT);
    // Pointing the person at moments changes nothing in the edit: it still works.
    let found = agent.call(
        "findMoments",
        json!({"query": "the pool", "ranges": [{"startSec": 0, "endSec": 13, "label": "the pool"}]}),
    );
    assert_eq!(outcome(&found), "shown", "{found}");
    assert_eq!(host.open_proposal().map(|p| p.id), Some(id));
}

#[test]
fn find_moments_only_points_the_person_at_them() {
    let host = FakeHost::two_clips();
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    agent.read();
    let before = host.revision();
    let found = agent.call(
        "findMoments",
        json!({"query": "the pool", "ranges": [{"startSec": 0, "endSec": 13, "label": "the pool"}]}),
    );
    assert_eq!(found["isError"], false, "{found}");
    assert_eq!(
        text(&found),
        "Shown to the person in Klipolo: 1 moment (0 to 13 s)."
    );
    assert_eq!(found["structuredContent"], json!({"outcome": "shown"}));
    assert_eq!(
        host.state().moments,
        [(
            "Agent".to_owned(),
            vec![Moment {
                range: span(0.0, 13.0),
                label: "the pool".into()
            }]
        )]
    );
    let two = agent.call(
        "findMoments",
        json!({"query": "the pool", "ranges": [
            {"startSec": 0, "endSec": 13, "label": "the pool"},
            {"startSec": 13, "endSec": 144.04, "label": "the library"},
        ]}),
    );
    assert_eq!(
        text(&two),
        "Shown to the person in Klipolo: 2 moments (0 to 13 s, 13 to 144.04 s)."
    );
    // None found: nothing to point at.
    let none = agent.call("findMoments", json!({"query": "a dog", "ranges": []}));
    assert_eq!(none["isError"], false);
    assert_eq!(text(&none), NO_MOMENTS);
    assert_eq!(host.state().moments.len(), 2);
    assert_eq!(host.revision(), before);
    let counts = host.state().counts;
    assert_eq!((counts.applies, counts.proposes), (0, 0));
}

#[test]
fn addition_that_removes_is_proposed_never_applied() {
    let host = FakeHost::two_clips();
    // The captions were placed by hand in landscape: dropping landscape loses that placement.
    host.change_project(|p| {
        p.captions.place.insert(FormatKind::Landscape, 0.8);
    });
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    let (before, project) = (host.revision(), host.project());
    let result = agent.call("setFormat", set_format("portrait", false));
    assert_eq!(result["isError"], false, "{result}");
    assert_eq!(outcome(&result), "proposed");
    assert_eq!(
        text(&result),
        fill(PROPOSED_NO_FOOTAGE, &[("tools", "setFormat")])
    );
    assert_eq!(result["structuredContent"]["removedSec"], 0.0);
    // Never applied: the formats are as they were, and the change waits for the person.
    assert_eq!((host.revision(), host.project()), (before, project));
    assert_eq!(host.state().counts.applies, 0);
    let slot = host.state().slot.clone().expect("a proposal waits");
    assert_eq!(slot.from.as_deref(), Some("Agent"));
    assert_eq!(
        slot.proposal.review.removals.formats,
        [FormatKind::Landscape]
    );
}

#[test]
fn still_listening_refusal_names_the_clip() {
    let host = FakeHost::two_clips();
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    for (state, said) in [
        (
            AnalysisState::Transcribing,
            "Klipolo is still listening to this clip (c1). Try again in a minute, once its analysis is done.",
        ),
        (
            AnalysisState::NotStarted,
            "Klipolo hasn't analyzed this clip yet (c1). Ask the person to analyze it in Klipolo, then try again.",
        ),
    ] {
        host.state().analysis.insert("clip-a".into(), state);
        assert_refused(&agent.call("removeFillers", json!({})), said);
        assert_refused(
            &agent.call("removeSilences", json!({"minPauseSec": null})),
            said,
        );
    }
    assert!(host.state().slot.is_none());
}

#[test]
fn rough_filler_refusal_asks_the_person_to_time_the_words_again() {
    let host = FakeHost::two_clips();
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    {
        // The first word, heard as "um", was spread over its turn when word timing failed: its time is a guess.
        let mut s = host.state();
        let t = Arc::make_mut(s.speech.transcripts.get_mut("clip-a").unwrap());
        let w = &mut t.words[0];
        (w.text, w.kind, w.confidence) = ("um".into(), WordKind::Filler, 0.0);
    }
    // The preview offers Time words again for the clip: the app asks the person to press it.
    host.state().time_again = HashSet::from(["clip-a".to_owned()]);
    assert_refused(
        &agent.call("removeFillers", json!({})),
        "Some words in this clip (c1) are only roughly timed, so Klipolo can't cut filler words exactly. Ask the \
         person to press Time words again above the video in Klipolo, then try again.",
    );
    // Not offered (timing again can't help, or another file's words are being timed): no button is named.
    host.state().time_again = HashSet::new();
    assert_refused(
        &agent.call("removeFillers", json!({})),
        "Some words in this clip (c1) are only roughly timed, so Klipolo can't cut filler words exactly.",
    );
    assert_eq!(
        fill(ROUGH_WORDS_STAY, &[("clip", "c1")]),
        "Some words in this clip (c1) are only roughly timed, so Klipolo can't cut filler words exactly."
    );
    assert!(host.state().slot.is_none());
}

#[test]
fn parse_error_names_the_field() {
    let host = FakeHost::two_clips();
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    agent.read();
    for (tool, arguments, starts) in [
        (
            "cutRange",
            json!({"startSec": "ten", "endSec": 5}),
            "cutRange: startSec must be",
        ),
        (
            "cutRange",
            json!({"startSec": 1}),
            "cutRange: endSec is missing.",
        ),
        (
            "setFormat",
            json!({"format": "round", "keepOthers": true}),
            "setFormat: format must be one of landscape, portrait, square",
        ),
        (
            "setCaptions",
            json!({"on": true, "style": null, "color": "red"}),
            "setCaptions: \"color\" is not an argument of this tool.",
        ),
    ] {
        let result = agent.call(tool, arguments.clone());
        // The parse error's own sentence.
        let parsed = ToolCall::parse(&FunctionCall {
            name: tool.into(),
            call_id: "1".into(),
            arguments: arguments.to_string(),
        })
        .expect_err("not a call");
        assert_refused(&result, &parsed.to_string());
        assert!(text(&result).starts_with(starts), "{result}");
    }
    assert_eq!(host.state().counts.applies, 0);
}

#[test]
fn agent_words_are_exact() {
    use klipolo_mcp::words::*;
    assert_eq!(
        PROPOSED,
        "Proposed in Klipolo ({tools}): {removed} s of footage would go ({ranges}). Nothing is cut until the person \
         clicks Apply in Klipolo's chat panel. Call getProposal to see what they decided."
    );
    assert_eq!(
        SHOWN,
        "Shown to the person in Klipolo: {n} moments ({ranges})."
    );
    assert_eq!(READ_FIRST, "Call getContext first, then use times from it.");
    assert_eq!(
        STALE,
        "The timeline changed since you last read it. Call getContext again, then retry with times from it."
    );
    assert_eq!(
        MOVED,
        "The project changed while this call ran. Call getContext again, then retry."
    );
    assert_eq!(
        PUSHES_OUT,
        "Klipolo can't make room in its undo history for this without dropping one of the person's steps. Ask the \
         person first."
    );
    assert_eq!(
        OVER_SHARE,
        "Your changes fill the undo history Klipolo keeps for other apps. Ask the person to continue in Klipolo."
    );
    assert_eq!(
        NOT_YOURS,
        "The last change in Klipolo isn't yours. Only the person can undo it."
    );
    assert_eq!(NOTHING_TO_UNDO, "There is nothing of yours to undo.");
    assert_eq!(
        REMOVES,
        "Klipolo found this would take something out of the edit, so it was not applied. Nothing changed. Ask the \
         person to make this change in Klipolo."
    );
    assert_eq!(
        PACED,
        "Klipolo is pacing calls from {client}. Wait {wait}, then try again."
    );
    assert_eq!(
        NO_CREDIT,
        "This needs AI credit, and Klipolo has no Meta key or access code. The person can add one in Klipolo's \
         Settings."
    );
    assert_eq!(
        OVER_CAP,
        "Other apps have used this session's AI allowance in Klipolo. The person can do this in Klipolo's own chat."
    );
    assert_eq!(
        READ_ONLY,
        "This project was made by a newer Klipolo, so it can't be edited here."
    );
    // No em or en dash in anything an agent reads.
    for said in [
        DONE,
        PROPOSED,
        PROPOSED_NO_FOOTAGE,
        SHOWN,
        SHOWN_ONE,
        NO_MOMENTS,
        UNCHANGED,
        READ_FIRST,
        STALE,
        WAITING_OTHER,
        MOVED,
        NOT_PROPOSED,
        OWN_MOVED,
        OWN_CHANGED,
        OWN_WAITING,
        DECIDED,
        UNDO_DROPPED,
        REDO_WAITS,
        UNDONE,
        STARTED,
        REMOVES,
        PUSHES_OUT,
        OVER_SHARE,
        NOT_YOURS,
        NOTHING_TO_UNDO,
        PACED,
        NO_CREDIT,
        LOW_CREDIT,
        OVER_CAP,
        READ_ONLY,
        NOT_ANALYZED,
        LISTENING,
        WATCHING,
        ROUGH_WORDS,
        ROUGH_WORDS_STAY,
        TIMES_FROM,
        NO_PROMPT,
        DROPPED_CHANGED,
        DROPPED_RETIMED,
        DROPPED_REPLACED,
        DROPPED_NEW_CHAT,
        DROPPED_CLOSED,
        OFF,
        UNANSWERED,
        MAY_BE_MADE,
        BUSY,
        REPLY_TOO_LARGE,
        VERSION_UNKNOWN,
    ] {
        assert!(!said.contains(['\u{2013}', '\u{2014}']), "{said}");
    }
    // What the stdio bridge says ends in what to do next.
    assert_eq!(
        REPLY_TOO_LARGE,
        "Klipolo's answer was too large to pass on. Ask for a smaller part, such as the transcript between two times."
    );
    assert_eq!(
        MAY_BE_MADE,
        "No answer came back from Klipolo, but a change you asked for may already be made. Call getContext to see \
         the project as it is now before trying again."
    );
    // A value is filled in once: an app named like a key fills nothing else.
    assert_eq!(
        fill(PACED, &[("client", "{wait}"), ("wait", "30 seconds")]),
        "Klipolo is pacing calls from {wait}. Wait 30 seconds, then try again."
    );
    assert_eq!(
        fill("{a} and {b} and {c", &[("a", "{b}"), ("b", "2")]),
        "{b} and 2 and {c"
    );
}

/// The person cuts 5 to 6 s by hand: c1 becomes 0 to 5 s and 5 to 12 s.
fn person_cuts(host: &FakeHost) -> u64 {
    host.person_edit(&Transaction {
        label: "edit.cut".into(),
        ops: vec![Op::DeleteRanges {
            ranges: vec![span(5.0, 6.0)],
        }],
        ..Transaction::default()
    })
}

#[test]
fn time_call_before_reading_is_refused() {
    let host = FakeHost::two_clips();
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    let before = host.revision();
    for (tool, arguments) in [
        ("cutRange", json!({"startSec": 0, "endSec": 13})),
        (
            "keepOnly",
            json!({"ranges": [{"startSec": 0, "endSec": 13}]}),
        ),
        ("splitAt", json!({"atSec": 13})),
        (
            "trimClip",
            json!({"clip": "c2", "edge": "end", "toSec": 140}),
        ),
        (
            "findMoments",
            json!({"query": "the pool", "ranges": [{"startSec": 0, "endSec": 13, "label": "the pool"}]}),
        ),
        ("addSound", json!({"name": "whoosh", "atSec": [13]})),
    ] {
        assert_refused(&agent.call(tool, arguments), READ_FIRST);
    }
    assert_eq!(host.revision(), before);
    assert!(host.state().slot.is_none());
    assert!(host.state().moments.is_empty());
    // Another session's read is its own: this one still hasn't read.
    Agent::open(&mcp, "Agent").read();
    assert_refused(&agent.cut(0.0, 13.0), READ_FIRST);
    // A call that names no time or clip needs no read.
    let format = agent.call("setFormat", set_format("square", true));
    assert_eq!(outcome(&format), "done", "{format}");
    // Once read, the cut is proposed.
    agent.read();
    assert_eq!(outcome(&agent.cut(0.0, 13.0)), "proposed");
}

#[test]
fn the_summary_is_not_a_read_of_the_timeline() {
    let host = FakeHost::two_clips();
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    // The summary lists the clips with their times but no transcript or scenes, as a tool or a resource: a call naming
    // times still needs a read of what is said and shown there.
    let summary = agent.call("getSummary", json!({}));
    assert_eq!(summary["isError"], false, "{summary}");
    assert_refused(&agent.cut(0.0, 13.0), READ_FIRST);
    let uri = json!({"uri": "klipolo://project/summary"});
    match mcp.handle(Some(&agent.session), request(9, "resources/read", uri)) {
        Handled::Reply { body, .. } => {
            assert!(body["result"]["contents"][0]["text"].is_string(), "{body}")
        }
        other => panic!("expected a reply, got {other:?}"),
    }
    assert_refused(&agent.cut(0.0, 13.0), READ_FIRST);
    // Nor does it bring a read the person's cut made stale up to date.
    agent.read();
    let before = person_cuts(&host);
    agent.call("getSummary", json!({}));
    assert_refused(&agent.cut(0.0, 13.0), STALE);
    assert_eq!(host.revision(), before);
    agent.read();
    assert_eq!(outcome(&agent.cut(0.0, 12.0)), "proposed");
}

#[test]
fn stale_timeline_is_refused_until_read_again() {
    let host = FakeHost::two_clips();
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    agent.read();
    // The person cuts by hand: the times the app read no longer hold.
    let before = person_cuts(&host);
    assert_refused(&agent.cut(0.0, 13.0), STALE);
    assert_refused(&agent.call("splitAt", json!({"atSec": 20})), STALE);
    assert_eq!(host.revision(), before);
    assert!(host.state().slot.is_none());
    // Reading the transcript is reading too: times from it hold.
    agent.call("getTranscript", json!({}));
    let result = agent.cut(0.0, 12.0);
    assert_eq!(outcome(&result), "proposed", "{result}");
    assert_eq!(
        host.state().slot.as_ref().unwrap().proposal.ranges,
        [span(0.0, 12.0)]
    );
    // Another project open in Klipolo: what the app read was of the one before.
    host.person_discards();
    host.change_project(|p| p.id = "another-project".into());
    assert_refused(&agent.cut(0.0, 12.0), STALE);
}

#[test]
fn format_change_keeps_read_times_valid() {
    let host = FakeHost::two_clips();
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    agent.read();
    // The shape, the captions and the words change; the main track's clips don't move.
    for (tool, arguments) in [
        ("setFormat", set_format("square", true)),
        ("setCaptions", json!({"on": false, "style": null})),
    ] {
        let result = agent.call(tool, arguments);
        assert_eq!(outcome(&result), "done", "{result}");
    }
    let result = agent.cut(0.0, 13.0);
    assert_eq!(outcome(&result), "proposed", "{result}");
}

fn cut_call(a: f64, b: f64) -> ToolCall {
    ToolCall::CutRange { range: span(a, b) }
}

fn select(host: &FakeHost, range: Option<RangeUs>) {
    host.set_view(View {
        playhead_us: 0,
        selection: range,
    });
}

#[test]
fn second_removal_joins_the_clients_own_proposal() {
    let host = FakeHost::two_clips();
    select(&host, Some(span(49.5, 50.5)));
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    agent.read();
    let before = host.revision();
    let first = agent.cut(0.0, 13.0);
    let first_id = first["structuredContent"]["proposalId"].clone();
    let second = agent.cut(49.5, 50.5);
    assert_eq!(second["isError"], false, "{second}");
    assert_eq!(
        text(&second),
        "Proposed in Klipolo (cutRange): 14 s of footage would go (0 to 13 s, 49.5 to 50.5 s). Nothing is cut until \
         the person clicks Apply in Klipolo's chat panel. Call getProposal to see what they decided."
    );
    // One proposal holds both, in place of the first; nothing is cut.
    assert_eq!(host.revision(), before);
    let slot = host.state().slot.clone().expect("a proposal waits");
    assert_eq!(slot.from.as_deref(), Some("Agent"));
    assert_eq!(slot.calls, [cut_call(0.0, 13.0), cut_call(49.5, 50.5)]);
    assert_eq!(slot.proposal.ranges, [span(0.0, 13.0), span(49.5, 50.5)]);
    assert_ne!(json!(slot.id), first_id);
    assert_eq!(
        second["structuredContent"],
        json!({"outcome": "proposed", "revision": before, "proposalId": slot.id, "removedSec": 14.0})
    );
    assert_eq!(host.state().counts.applies, 0);
    // Another app never joins it: it waits for the person.
    let other = Agent::open(&mcp, "Other app");
    other.read();
    assert_refused(
        &other.cut(0.0, 13.0),
        &fill(WAITING_OTHER, &[("who", "Agent")]),
    );
    // Once the person decides, the app's next removal stands alone.
    host.person_discards();
    let third = agent.cut(49.5, 50.5);
    assert_eq!(third["structuredContent"]["removedSec"], 1.0, "{third}");
    assert_eq!(
        host.state().slot.as_ref().unwrap().calls,
        [cut_call(49.5, 50.5)]
    );
}

#[test]
fn addition_after_own_removal_applies_and_reopens_the_proposal_on_top() {
    let host = FakeHost::two_clips();
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    agent.read();
    let (before, main) = (host.revision(), host.project().timeline.main);
    let first = agent.cut(0.0, 13.0);
    let result = agent.call("setCaptions", json!({"on": false, "style": null}));
    assert_eq!(result["isError"], false, "{result}");
    assert_eq!(
        text(&result),
        "Done in Klipolo (setCaptions). The person can undo it there. Proposed in Klipolo (cutRange): 13 s of \
         footage would go (0 to 13 s). Nothing is cut until the person clicks Apply in Klipolo's chat panel. Call \
         getProposal to see what they decided."
    );
    // The captions applied as the app's step, and only they: the cut is never in what applied.
    assert_eq!(host.revision(), before + 1);
    let project = host.project();
    assert!(!project.captions.enabled);
    assert_eq!(project.timeline.main, main);
    {
        let s = host.state();
        let [(_, tx)] = s.applied.as_slice() else {
            panic!("one batch: {:?}", s.applied)
        };
        assert!(
            tx.ops.iter().all(|op| matches!(op, Op::SetCaptions { .. })),
            "{tx:?}"
        );
    }
    // The removal waits again, on top of the captions.
    let slot = host.state().slot.clone().expect("a proposal waits");
    assert_eq!(slot.from.as_deref(), Some("Agent"));
    assert_eq!(slot.base_revision, before + 1);
    assert_eq!(slot.calls, [cut_call(0.0, 13.0)]);
    assert_eq!(slot.proposal.ranges, [span(0.0, 13.0)]);
    assert!(!slot.proposal.review.after.captions.enabled);
    assert_ne!(json!(slot.id), first["structuredContent"]["proposalId"]);
    assert_eq!(
        result["structuredContent"],
        json!({"outcome": "proposed", "revision": before + 1, "proposalId": slot.id, "removedSec": 13.0})
    );
    let waiting = agent.call("getProposal", json!({}));
    assert_eq!(waiting["structuredContent"]["id"], json!(slot.id));
    // Asked again, the captions are as asked: the proposal stands as it is.
    let again = agent.call("setCaptions", json!({"on": false, "style": null}));
    assert_eq!(text(&again), UNCHANGED);
    assert_eq!(
        host.state().slot.as_ref().map(|s| s.id.clone()),
        Some(slot.id)
    );
}

#[test]
fn removals_that_cant_reopen_say_the_additions_are_done() {
    let host = FakeHost::two_clips();
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    agent.read();
    agent.cut(0.0, 13.0);
    let before = host.revision();
    host.state().move_before_propose = true;
    let result = agent.call("setCaptions", json!({"on": false, "style": null}));
    // Never a bare "the project changed": the captions are in, the cut is not proposed.
    assert_eq!(result["isError"], false, "{result}");
    assert_eq!(
        text(&result),
        format!(
            "{} {}",
            fill(DONE, &[("tools", "setCaptions")]),
            NOT_PROPOSED
        )
    );
    assert_eq!(
        result["structuredContent"],
        json!({"outcome": "done", "revision": before + 1})
    );
    assert!(host.state().slot.is_none());
}

#[test]
fn find_moments_never_touches_the_clients_own_proposal() {
    let host = FakeHost::two_clips();
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    agent.read();
    let first = agent.cut(0.0, 13.0);
    let found = agent.call(
        "findMoments",
        json!({"query": "the library", "ranges": [{"startSec": 13, "endSec": 144.04, "label": "the library"}]}),
    );
    assert_eq!(outcome(&found), "shown", "{found}");
    let open = host.open_proposal().expect("still waiting");
    assert_eq!(json!(open.id), first["structuredContent"]["proposalId"]);
    assert_eq!(host.state().counts.proposes, 1);
}

#[test]
fn a_remembered_call_that_reads_differently_now_is_refused() {
    let host = FakeHost::two_clips();
    select(&host, Some(span(49.5, 50.5)));
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    agent.read();
    agent.cut(49.5, 50.5);
    let waiting = host.state().slot.clone().expect("a proposal waits");
    // The person selects a little further on: the earlier cut stood on their selection, and would snap to the new one.
    select(&host, Some(span(49.6, 50.6)));
    assert_refused(&agent.cut(0.0, 13.0), OWN_CHANGED);
    assert_refused(
        &agent.call("setCaptions", json!({"on": false, "style": null})),
        OWN_CHANGED,
    );
    // Selecting nothing, the earlier cut stands on nothing: refused the same way, never left out quietly.
    select(&host, None);
    assert_refused(&agent.cut(0.0, 13.0), OWN_CHANGED);
    // The proposal the person sees is the one the app made; nothing applied.
    let slot = host.state().slot.clone().expect("still waiting");
    assert_eq!((slot.id, slot.calls), (waiting.id, waiting.calls));
    assert_eq!(host.state().counts.applies, 0);
    // With the person's selection back where it was, the call joins.
    select(&host, Some(span(49.5, 50.5)));
    assert_eq!(outcome(&agent.cut(0.0, 13.0)), "proposed");
    assert_eq!(
        host.state().slot.as_ref().unwrap().calls,
        [cut_call(49.5, 50.5), cut_call(0.0, 13.0)]
    );
}

#[test]
fn a_proposal_klipolo_no_longer_remembers_is_waited_on() {
    let host = FakeHost::two_clips();
    let first = Mcp::new(host.clone());
    let agent = Agent::open(&first, "Agent");
    agent.read();
    agent.cut(0.0, 13.0);
    // The server restarted while the app's proposal waited: its calls aren't known, so nothing joins or drops it.
    let again = Mcp::new(host.clone());
    let agent = Agent::open(&again, "Agent");
    agent.read();
    assert_refused(&agent.cut(0.0, 13.0), OWN_WAITING);
    assert_refused(
        &agent.call("setCaptions", json!({"on": false, "style": null})),
        OWN_WAITING,
    );
    assert_eq!(host.state().counts.applies, 0);
    assert!(host.open_proposal().is_some());
}

/// The person renames the project by hand, labelling the step `label`.
fn person_renames(host: &FakeHost, label: &str) {
    host.person_edit(&Transaction {
        label: label.into(),
        ops: vec![Op::Rename { name: label.into() }],
        ..Transaction::default()
    });
}

fn captions(on: bool) -> Value {
    json!({"on": on, "style": null})
}

/// The undo steps, oldest first: their labels and who made them.
fn steps(host: &FakeHost) -> Vec<(String, Maker)> {
    let s = host.state();
    let doc = s.doc.as_ref().unwrap();
    (doc.undo_view())
        .map(|step| (step.label.to_owned(), step.maker.clone()))
        .collect()
}

#[test]
fn undo_only_takes_back_the_clients_own_step() {
    let host = FakeHost::two_clips();
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    let other = Agent::open(&mcp, "Other app");
    assert_refused(&agent.call("undo", json!({})), NOTHING_TO_UNDO);
    // The person's own step is never an app's to undo.
    person_renames(&host, "mine");
    assert_refused(&agent.call("undo", json!({})), NOT_YOURS);
    // The app's own step goes back, and only it; another app can't take it back.
    let before = host.revision();
    assert_eq!(outcome(&agent.call("setCaptions", captions(false))), "done");
    assert_refused(&other.call("undo", json!({})), NOT_YOURS);
    let undone = agent.call("undo", json!({}));
    assert_eq!(undone["isError"], false, "{undone}");
    assert_eq!(text(&undone), UNDONE);
    assert_eq!(
        undone["structuredContent"],
        json!({"outcome": "done", "revision": before + 2})
    );
    let project = host.project();
    assert!(project.captions.enabled);
    assert_eq!(project.name, "mine");
    assert_eq!(steps(&host), [("mine".to_owned(), Maker::Person)]);
    // What is newest now is the person's again.
    assert_refused(&agent.call("undo", json!({})), NOT_YOURS);
    // A step the person made after the app's is newest: the app's waits behind it.
    agent.call("setCaptions", captions(false));
    person_renames(&host, "later");
    assert_refused(&agent.call("undo", json!({})), NOT_YOURS);
    assert!(!host.project().captions.enabled);
}

#[test]
fn apps_of_one_name_are_one_app() {
    let host = FakeHost::two_clips();
    let mcp = Mcp::new(host.clone());
    // Klipolo knows an app by the name it gives, cleaned: two sessions of one name share their steps.
    let first = Agent::open(&mcp, "Agent");
    let second = Agent::open(&mcp, "  Agent ");
    first.call("setCaptions", captions(false));
    assert_eq!(text(&second.call("undo", json!({}))), UNDONE);
    // Every app that gives no name is "Unnamed app", one app too. (Another host: the first app's undone step waits
    // in redo there, and no other app's change drops it.)
    let host = FakeHost::two_clips();
    let mcp = Mcp::new(host.clone());
    let nameless = Agent::open(&mcp, "");
    let lookalike = Agent::open(&mcp, "Klipolo");
    nameless.call("setCaptions", captions(false));
    assert_eq!(
        steps(&host).last().map(|(_, maker)| maker.clone()),
        Some(Maker::App("Unnamed app".into()))
    );
    assert_eq!(text(&lookalike.call("undo", json!({}))), UNDONE);
    assert!(host.project().captions.enabled);
}

#[test]
fn agent_steps_never_push_out_the_persons_steps() {
    // A history of 8 steps: an app's share is 2 of them, all apps' 4.
    let small = HistoryLimits {
        steps: 8,
        bytes: HistoryLimits::default().bytes,
    };
    let host = FakeHost::two_clips();
    host.set_limits(small);
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    // The app's steps sit apart, so they can't fold into one: its share is full.
    for (k, on) in [false, true].into_iter().enumerate() {
        assert_eq!(outcome(&agent.call("setCaptions", captions(on))), "done");
        person_renames(&host, &format!("person {k}"));
    }
    let before = (host.revision(), steps(&host));
    assert_refused(&agent.call("setCaptions", captions(false)), OVER_SHARE);
    assert_eq!((host.revision(), steps(&host)), before);

    // Seven steps of the person's leave room for one of the app's; the next would push out the person's oldest.
    let host = FakeHost::two_clips();
    host.set_limits(small);
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    for k in 0..7 {
        person_renames(&host, &format!("person {k}"));
    }
    assert_eq!(outcome(&agent.call("setCaptions", captions(false))), "done");
    let before = (host.revision(), steps(&host));
    assert_refused(&agent.call("setCaptions", captions(true)), PUSHES_OUT);
    assert_eq!((host.revision(), steps(&host)), before);
    let kept = steps(&host);
    assert_eq!(kept[0], ("person 0".to_owned(), Maker::Person));
    assert_eq!(kept.iter().filter(|(_, m)| *m == Maker::Person).count(), 7);
}

#[test]
fn redo_the_person_could_take_is_never_dropped() {
    let host = FakeHost::two_clips();
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    person_renames(&host, "mine");
    host.person_undo();
    let before = host.revision();
    // Any commit would drop the person's redo: the app's change waits for them instead, and the app is told why.
    assert_refused(&agent.call("setCaptions", captions(false)), REDO_WAITS);
    assert_eq!(
        REDO_WAITS,
        "A change undone in Klipolo can still be redone there. Wait until the person redoes it or makes an edit of \
         their own, then try again."
    );
    assert_eq!(host.revision(), before);
    assert_eq!(host.state().doc.as_ref().unwrap().redo_steps(), 1);
    assert!(host.project().captions.enabled);
    // Once the person redoes it, the app's change applies.
    host.person_redo();
    assert_eq!(host.project().name, "mine");
    assert_eq!(outcome(&agent.call("setCaptions", captions(false))), "done");
    // An app's own undone step in redo is its own to drop: its next change applies.
    assert_eq!(text(&agent.call("undo", json!({}))), UNDONE);
    assert_eq!(outcome(&agent.call("setCaptions", captions(false))), "done");
    // Another app's undone step waits for that app the same way.
    let other = Agent::open(&mcp, "Other app");
    assert_eq!(outcome(&other.call("setCaptions", captions(true))), "done");
    assert_eq!(text(&other.call("undo", json!({}))), UNDONE);
    assert_refused(&agent.call("setCaptions", captions(true)), REDO_WAITS);
}

#[test]
fn a_proposal_that_appears_mid_call_is_waited_on() {
    let host = FakeHost::two_clips();
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    let chats = fill(WAITING_OTHER, &[("who", KLIPOLO_CHAT)]);
    // The person's proposal lands while the app's batch is on its way: the batch waits, and the app is told why.
    let (_, proposal) = cut_c1(&host);
    host.state().chat_before_commit = Some(proposal.clone());
    let before = host.revision();
    assert_refused(&agent.call("setCaptions", captions(false)), &chats);
    assert_eq!(host.revision(), before);
    assert_eq!(host.open_proposal().map(|p| p.from), Some(None));
    // The same for an undo.
    host.person_discards();
    assert_eq!(outcome(&agent.call("setCaptions", captions(false))), "done");
    host.state().chat_before_commit = Some(proposal);
    assert_refused(&agent.call("undo", json!({})), &chats);
    assert!(!host.project().captions.enabled);
}

/// `trimClip` of `clip`'s end to `to` timeline seconds.
fn trim_end(clip: &str, to: f64) -> Value {
    json!({"clip": clip, "edge": "end", "toSec": to})
}

#[test]
fn own_proposal_is_never_read_again_once_the_apps_change_moved_the_clips() {
    let host = FakeHost::two_clips();
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    agent.read();
    // c2 (13 to 144.04 s) ends at 140.5 s instead: 3.54 s would go.
    let trimmed = agent.call("trimClip", trim_end("c2", 140.5));
    assert_eq!(outcome(&trimmed), "proposed", "{trimmed}");
    let before = host.state().slot.clone().expect("a proposal waits");
    // A split at 5 s applies; the trim waits again on top of it, still taking the same footage.
    let split = agent.call("splitAt", json!({"atSec": 5}));
    assert_eq!(outcome(&split), "proposed", "{split}");
    let after_split = host.state().slot.clone().expect("a proposal waits");
    assert_eq!(after_split.calls, before.calls);
    assert_eq!(after_split.proposal.ranges, before.proposal.ranges);
    // Now "c2" is the piece from 5 to 13 s: the waiting trim's ref names other footage, so no call joins it.
    agent.read();
    let revision = host.revision();
    assert_refused(
        &agent.call("setCaptions", json!({"on": false, "style": null})),
        OWN_MOVED,
    );
    assert_refused(&agent.cut(0.0, 5.0), OWN_MOVED);
    let slot = host.state().slot.clone().expect("still waiting");
    assert_eq!(
        (slot.id, slot.proposal),
        (after_split.id, after_split.proposal)
    );
    assert_eq!(host.revision(), revision);
}

#[test]
fn a_join_on_another_revision_says_to_retry() {
    let host = FakeHost::two_clips();
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    agent.read();
    agent.cut(0.0, 13.0);
    // The waiting proposal was made on a revision the call didn't read (another session of this app moved it on):
    // trying again reads it as it is.
    host.change_project(|_| {});
    assert_refused(
        &agent.call("setCaptions", json!({"on": false, "style": null})),
        MOVED,
    );
    assert_eq!(host.state().counts.applies, 0);
}

#[test]
fn a_proposal_the_person_discards_mid_call_stays_discarded() {
    let host = FakeHost::two_clips();
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    agent.read();
    let first = agent.cut(0.0, 13.0);
    let id = first["structuredContent"]["proposalId"].clone();
    let before = host.revision();
    // The person clicks Discard while the app's next call compiles, just before its captions commit.
    host.state().discard_before_commit = true;
    let result = agent.call("setCaptions", json!({"on": false, "style": null}));
    assert_eq!(result["isError"], false, "{result}");
    assert_eq!(
        text(&result),
        format!("{} {}", fill(DONE, &[("tools", "setCaptions")]), DECIDED)
    );
    assert_eq!(
        result["structuredContent"],
        json!({"outcome": "done", "revision": before + 1})
    );
    // Nothing came back: no proposal waits, and the person's Discard is what the app reads.
    assert!(host.state().slot.is_none());
    assert_eq!(host.state().counts.proposes, 1);
    let outcome = agent.call("getProposal", json!({}));
    assert_eq!(
        outcome["structuredContent"],
        json!({"status": "discarded", "id": id})
    );
}

#[test]
fn a_proposal_the_person_discards_while_a_removal_joins_it_reads_decided() {
    let host = FakeHost::two_clips();
    select(&host, Some(span(49.5, 50.5)));
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    agent.read();
    let first = agent.cut(0.0, 13.0);
    let id = first["structuredContent"]["proposalId"].clone();
    let before = host.revision();
    // The person clicks Discard while the app's next removal compiles with it, just before it would take its place:
    // nothing applies, and the app reads that the person decided, not that the project moved.
    host.state().discard_before_propose = true;
    assert_refused(&agent.cut(49.5, 50.5), DECIDED);
    assert_eq!(host.revision(), before);
    assert!(host.state().slot.is_none());
    assert_eq!(host.state().counts.proposes, 2);
    let decided = agent.call("getProposal", json!({}));
    assert_eq!(
        decided["structuredContent"],
        json!({"status": "discarded", "id": id})
    );
    // The project moving under the same call still reads as moved.
    assert_eq!(outcome(&agent.cut(0.0, 13.0)), "proposed");
    host.state().move_before_propose = true;
    assert_refused(&agent.cut(49.5, 50.5), MOVED);
}

#[test]
fn an_undo_that_drops_the_apps_proposal_says_so() {
    let host = FakeHost::two_clips();
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    agent.read();
    agent.call("setCaptions", captions(false));
    agent.cut(0.0, 13.0);
    let undone = agent.call("undo", json!({}));
    assert_eq!(undone["isError"], false, "{undone}");
    assert_eq!(text(&undone), format!("{UNDONE} {UNDO_DROPPED}"));
    assert!(host.state().slot.is_none());
    // With nothing waiting, an undo says only that.
    agent.call("setCaptions", captions(false));
    assert_eq!(text(&agent.call("undo", json!({}))), UNDONE);
}

#[test]
fn removals_that_cant_reopen_because_another_proposal_waits_say_so() {
    let host = FakeHost::two_clips();
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    agent.read();
    agent.cut(0.0, 13.0);
    // The person's chat proposes between the app's captions and its removals proposed again on top.
    let (_, proposal) = cut_c1(&host);
    host.state().chat_before_propose = Some(proposal);
    let before = host.revision();
    let result = agent.call("setCaptions", captions(false));
    assert_eq!(result["isError"], false, "{result}");
    assert_eq!(
        text(&result),
        format!(
            "{} {}",
            fill(DONE, &[("tools", "setCaptions")]),
            fill(WAITING_OTHER, &[("who", KLIPOLO_CHAT)])
        )
    );
    assert_eq!(host.revision(), before + 1);
    assert_eq!(host.open_proposal().map(|p| p.from), Some(None));
}

/// The words on the project's text layers.
fn texts_on_screen(host: &FakeHost) -> Vec<String> {
    let p = host.project();
    (p.timeline.tracks.iter())
        .flat_map(|t| &t.items)
        .filter_map(|i| match &i.content {
            ItemContent::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// An app acts for its own person: the words it gives go on screen at once, as one undo step the app made.
#[test]
fn an_apps_own_words_go_on_screen() {
    let host = FakeHost::two_clips();
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    let before = host.revision();
    let words =
        json!({"text": "Hello from orbit", "preset": "title", "atSec": null, "durationSec": null});
    let result = agent.call("addText", words);
    assert_eq!(result["isError"], false, "{result}");
    assert_eq!(text(&result), fill(DONE, &[("tools", "addText")]));
    assert_eq!(host.revision(), before + 1);
    assert_eq!(host.state().applied.last().unwrap().0, "Agent");
    assert_eq!(texts_on_screen(&host), ["Hello from orbit"]);
}

/// An app's title card takes its own title at once, as a step the app made, and its background is a paid job that
/// starts only through the host's checks and the app's pace, as a sticker's does.
#[test]
fn an_apps_title_card_takes_its_own_title_and_starts_its_background() {
    let host = FakeHost::two_clips();
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    let before = host.revision();
    let card = json!({"title": "Welcome aboard", "prompt": "a sunrise over the station", "atSec": null, "durationSec": null});
    let result = agent.call("makeTitleCard", card.clone());
    assert_eq!(result["isError"], false, "{result}");
    let tools = [("tools", "makeTitleCard")];
    assert_eq!(
        text(&result),
        format!("{} {}", fill(DONE, &tools), fill(STARTED, &tools))
    );
    assert_eq!(result["structuredContent"]["outcome"], "done");
    assert_eq!(host.revision(), before + 1);
    {
        let s = host.state();
        assert_eq!(s.applied.last().unwrap().0, "Agent");
        assert!(
            matches!(&s.started[..], [(client, Job::MakeTitleBackground { prompt, .. })]
                if client == "Agent" && prompt == "a sunrise over the station"),
            "{:?}",
            s.started
        );
        assert_eq!((s.counts.estimates, s.counts.paid_asked), (1, 1));
    }
    // With no Meta key or access code, the title still goes in, and the background is said not to start.
    host.state().credit = false;
    let result = agent.call("makeTitleCard", card);
    assert_eq!(result["isError"], false, "{result}");
    assert_eq!(
        text(&result),
        format!("{} {}", fill(DONE, &tools), NO_CREDIT)
    );
    assert_eq!(host.revision(), before + 2);
    assert_eq!(host.state().started.len(), 1);
}

#[test]
fn chat_worded_refusals_are_said_for_apps() {
    let host = FakeHost::two_clips();
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    let music = |file: &str| json!({"file": file, "volume": null, "duck": null, "atSec": null});
    // Library has no music: an app asks its own person, never through askUser, which it isn't offered.
    for (file, ends) in [
        ("the song", "Ask them which music."),
        ("jazz", "Use a name from it, or ask the person."),
    ] {
        let result = agent.call("addMusic", music(file));
        assert_eq!(result["isError"], true, "{result}");
        assert!(text(&result).ends_with(ends), "{result}");
        assert!(!text(&result).contains("askUser"), "{result}");
    }
    assert_eq!(host.state().counts.applies, 0);
}

/// A 2 s dissolve from c1 into c2, with c1 playing the file to 1 s before its end (1 s of footage after its out point,
/// c2 13 s before its in point): at 2x c1's spare footage lasts 0.5 s, so the dissolve shortens to 1 s.
fn with_dissolve(host: &FakeHost) {
    host.change_project(|p| {
        p.timeline.main[0].source_out_us = p.assets[0].duration_us - 1_000_000;
        p.timeline.main[0].transition_out = Some(klipolo_model::timeline::Transition {
            effect: klipolo_model::contrib::builtin::TRANSITION_DISSOLVE.into(),
            duration_us: 2_000_000,
            params: Default::default(),
        });
        p.requires
            .insert(klipolo_model::transitions::FEATURE.to_owned());
    });
}

#[test]
fn an_apps_clip_and_transition_changes_are_the_cards_applied_at_once() {
    let host = FakeHost::two_clips();
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    agent.read();
    let before = host.revision();
    let set = json!({"after": "c1", "kind": "dissolve", "durationSec": null});
    let result = agent.call("setTransition", set);
    assert_eq!(outcome(&result), "done", "{result}");
    assert_eq!(text(&result), fill(DONE, &[("tools", "setTransition")]));
    assert_eq!(host.revision(), before + 1);
    let added = host.project().timeline.main[0].transition_out.clone();
    assert_eq!(added.map(|t| t.duration_us), Some(500_000));
    // A speed that shortens a transition applies at once, as the card's does, and says so.
    with_dissolve(&host);
    agent.read();
    let speed = json!({"clip": "c1", "speed": 2, "volume": null, "muted": null, "framing": null, "framingFormat": null});
    let result = agent.call("setClip", speed);
    assert_eq!(outcome(&result), "done", "{result}");
    assert_eq!(
        text(&result),
        format!(
            "{} A transition is shortened.",
            fill(DONE, &[("tools", "setClip")])
        )
    );
    let project = host.project();
    assert_eq!(project.timeline.main[0].speed, 2.0);
    assert_eq!(
        project.timeline.main[0]
            .transition_out
            .as_ref()
            .map(|t| t.duration_us),
        Some(1_000_000)
    );
    {
        let s = host.state();
        let (client, tx) = s.applied.last().expect("a batch");
        assert_eq!(
            (client.as_str(), tx.label.as_str()),
            ("Agent", "chat.setClip.note")
        );
    }
    // A value the card doesn't offer is refused with the reason, and nothing changes.
    let revision = host.revision();
    let result = agent.call("setClip", json!({"clip": "c1", "speed": 3}));
    assert_refused(
        &result,
        "setClip: speed must be one of 0.5, 0.75, 1, 1.25, 1.5, 2 or null, not 3.",
    );
    assert_eq!(host.revision(), revision);
    // Taking the transitions away waits for the person, as every removal does.
    let result = agent.call("setTransition", json!({"after": "all", "kind": "none"}));
    assert_eq!(outcome(&result), "proposed", "{result}");
    assert_eq!(
        text(&result),
        fill(PROPOSED_NO_FOOTAGE, &[("tools", "setTransition")])
    );
    assert_eq!(host.revision(), revision);
    assert!(host.project().timeline.main[0].transition_out.is_some());
}

#[test]
fn an_apps_speed_up_with_music_to_the_end_applies_as_the_card_does() {
    let host = FakeHost::two_clips();
    // A song laid from the start to the video's end, as addMusic lays it.
    host.change_project(|p| {
        let song = klipolo_model::project::Asset {
            id: "song".into(),
            kind: klipolo_model::project::AssetKind::Audio,
            path: "/media/song.mp3".into(),
            name: "song.mp3".into(),
            duration_us: 300_000_000,
            width: 0,
            height: 0,
            fps: 0.0,
            rotation: None,
            has_audio: true,
            audio_lead_us: None,
            made_with: None,
        };
        p.assets.push(song.clone());
        let opts = klipolo_model::media::music::MusicOpts::default();
        let ops = klipolo_model::media::music::add_music_ops(p, &song, opts).unwrap();
        *p = klipolo_model::apply_ops(p, &ops).unwrap();
    });
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    agent.read();
    let before = host.project();
    let speed = json!({"clip": "c2", "speed": 2, "volume": null, "muted": null, "framing": null, "framingFormat": null});
    let result = agent.call("setClip", speed);
    assert_eq!(outcome(&result), "done", "{result}");
    assert_eq!(text(&result), fill(DONE, &[("tools", "setClip")]));
    // The card's change: the clip's speed, the music cut at the new end in the same step.
    let by_hand = Op::UpdateClip {
        id: before.timeline.main[1].id.clone(),
        patch: klipolo_model::ops::ClipPatch {
            speed: Some(2.0),
            ..Default::default()
        },
    };
    let card = klipolo_model::media::music::apply_with_music_refit(&before, vec![by_hand.clone()])
        .unwrap();
    assert!(card.ops.len() > 1, "the music is refitted");
    assert_eq!(host.project(), card.project);
    let s = host.state();
    let (_, tx) = s.applied.last().expect("a batch");
    assert_eq!(
        tx.ops,
        [by_hand],
        "the app's own edit; the host fits the music"
    );
}

#[test]
fn an_apps_speed_that_leaves_a_transition_no_room_is_proposed_and_says_why() {
    let host = FakeHost::two_clips();
    // c1 plays the file to 0.08 s before its end, joined to c2 by a 0.16 s dissolve: at 2x no transition fits there.
    host.change_project(|p| {
        p.timeline.main[0].source_out_us = p.assets[0].duration_us - 80_000;
        p.timeline.main[0].transition_out = Some(klipolo_model::timeline::Transition {
            effect: klipolo_model::contrib::builtin::TRANSITION_DISSOLVE.into(),
            duration_us: 160_000,
            params: Default::default(),
        });
        p.requires
            .insert(klipolo_model::transitions::FEATURE.to_owned());
    });
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    agent.read();
    let revision = host.revision();
    let result = agent.call("setClip", json!({"clip": "c1", "speed": 2}));
    assert_eq!(outcome(&result), "proposed", "{result}");
    assert_eq!(
        text(&result),
        format!(
            "{} At that speed a transition no longer fits, so the change waits for Apply.",
            fill(PROPOSED_NO_FOOTAGE, &[("tools", "setClip")])
        )
    );
    assert_eq!(
        host.revision(),
        revision,
        "nothing changes until the person applies it"
    );
}
