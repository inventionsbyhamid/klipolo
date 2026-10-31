//! The chat panel shows what the model wrote as it is: a reply is cleaned of em and en dashes as it is built, so the
//! page never cleans it itself.

use klipolo_app::chat::{
    ChatFailure, ChatFailureKind, ChatOutcome, ChatProposal, ChatReply, ChatRequest,
};
use klipolo_model::risk::Removals;
use klipolo_model::time::RangeUs;

const DASHES: [char; 4] = ['\u{2012}', '\u{2013}', '\u{2014}', '\u{2015}'];

fn no_dash(text: &str) -> bool {
    !text.contains(DASHES)
}

fn range(start: i64, end: i64) -> RangeUs {
    RangeUs { start, end }
}

#[test]
fn every_text_the_model_wrote_is_cleaned_of_dashes() {
    let proposal = ChatProposal::new(
        "p1",
        4,
        "Cut 2 pauses",
        Removals {
            ranges: vec![range(1_000_000, 2_000_000)],
            ..Removals::default()
        },
        1_000_000,
        [(
            range(1_000_000, 2_000_000),
            "long pause \u{2014} 1 s".to_owned(),
        )],
    );
    let reply = ChatReply::new("r1", "Done \u{2014} want more?")
        .note("Couldn\u{2019}t cut 10\u{2013}20 s: nothing was said there.")
        .question("Which one \u{2014} the first or the second?")
        .moment(range(0, 1_000_000), "The kiln \u{2013} first firing")
        .proposal(proposal);
    assert_eq!(reply.text(), "Done, want more?");
    assert_eq!(
        reply.notes()[0],
        "Couldn\u{2019}t cut 10-20 s: nothing was said there."
    );
    assert!(reply.questions().iter().all(|q| no_dash(q)));
    assert!(reply.moments().iter().all(|m| no_dash(m.label())));
    let p = reply.proposal_ref().unwrap();
    assert!(no_dash(p.title()) && p.tag_labels().all(no_dash));
    let failure = ChatFailure::new(
        ChatFailureKind::Budget,
        "Your access code has $0.03 left \u{2014} this needs about $0.04.",
    );
    assert!(no_dash(failure.message()));
}

#[test]
fn serializes_as_the_page_reads_it() {
    let reply = ChatReply::new("r1", "I found 2 pauses.").proposal(ChatProposal::new(
        "p1",
        7,
        "Cut 2 pauses",
        Removals::default(),
        0,
        [],
    ));
    let json = serde_json::to_value(ChatOutcome::Reply(Box::new(reply))).unwrap();
    assert_eq!(json["type"], "reply");
    assert_eq!(json["id"], "r1");
    assert_eq!(json["text"], "I found 2 pauses.");
    assert_eq!(json["proposal"]["baseRevision"], 7);
    assert_eq!(json["proposal"]["removedUs"], 0);
    assert!(json["applied"].is_null() && json["undone"].is_null());
    let failed = serde_json::to_value(ChatOutcome::Failed(ChatFailure::new(
        ChatFailureKind::Registration,
        "This Mac couldn't reach Klipolo to use the access code.",
    )))
    .unwrap();
    assert_eq!(failed["type"], "failed");
    assert_eq!(failed["kind"], "registration");
}

#[test]
fn a_request_reads_as_the_page_sends_it() {
    // A typed message has no ranges to keep; "Keep only these" sends its moments'.
    let typed: ChatRequest = serde_json::from_value(serde_json::json!({
        "id": "r1", "text": "cut the ums", "selection": null, "item": null, "playheadUs": 0, "format": "portrait"
    }))
    .unwrap();
    assert!(typed.keep_only.is_empty());
    let keep: ChatRequest = serde_json::from_value(serde_json::json!({
        "id": "r2", "text": "Keep only these", "selection": { "start": 1, "end": 2 }, "item": null,
        "playheadUs": 5, "format": null, "keepOnly": [{ "start": 10, "end": 20 }]
    }))
    .unwrap();
    assert_eq!(keep.keep_only, vec![range(10, 20)]);
    assert_eq!(keep.selection, Some(range(1, 2)));
}
