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

use agent::{Agent, assert_refused, outcome, text};
use fake_host::FakeHost;
use klipolo_chat::{AnalysisState, FunctionCall, Moment, Proposal, ToolCall, View, compile};
use klipolo_mcp::Host;
use klipolo_mcp::call::from_app;
use klipolo_mcp::rpc::Mcp;
use klipolo_mcp::words::{
    DONE, KLIPOLO_CHAT, NO_MOMENTS, PROPOSED_NO_FOOTAGE, UNCHANGED, WAITING_OTHER, fill,
};
use klipolo_model::Transaction;
use klipolo_model::coded::ErrorParam;
use klipolo_model::history::Maker;
use klipolo_model::ops::{Op, Origin};
use klipolo_model::project::FormatKind;
use klipolo_model::time::RangeUs;
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
    assert!(text(&result).ends_with("Use a time from the context."));
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
    // Nothing changed, and the other app's proposal still waits.
    assert_eq!((host.revision(), host.project()), (before, project));
    assert_eq!(host.state().counts.applies, 0);
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
            "Klipolo is still listening to this clip (c1).",
        ),
        (AnalysisState::NotStarted, "Analyze this clip first (c1)."),
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
        PACED,
        "Klipolo is pacing changes from {client}. Wait a minute, then try again."
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
        REMOVES,
        PUSHES_OUT,
        OVER_SHARE,
        NOT_YOURS,
        NOTHING_TO_UNDO,
        PACED,
        NO_CREDIT,
        OVER_CAP,
        READ_ONLY,
    ] {
        assert!(!said.contains(['\u{2013}', '\u{2014}']), "{said}");
    }
    // A value is filled in once: an app named like a key fills nothing else.
    assert_eq!(
        fill(PACED, &[("client", "{client}")]),
        "Klipolo is pacing changes from {client}. Wait a minute, then try again."
    );
    assert_eq!(
        fill("{a} and {b} and {c", &[("a", "{b}"), ("b", "2")]),
        "{b} and 2 and {c"
    );
}
