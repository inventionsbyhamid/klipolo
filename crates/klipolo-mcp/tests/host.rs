//! The `Host` contract the calls rely on, as the fake host keeps it: while a proposal that isn't an app's own waits,
//! the app's batches and undos are refused under the host's lock, so a proposal that appears after the app looked
//! still wins; any commit drops the open proposal.

#[path = "support/fake_host.rs"]
mod fake_host;

use fake_host::FakeHost;
use klipolo_chat::{Proposal, ToolCall, compile};
use klipolo_mcp::host::{Host, HostError, ProposalOutcome};
use klipolo_model::Transaction;
use klipolo_model::project::FormatKind;
use klipolo_model::time::RangeUs;

/// What `call` compiles to on the host's project as it is now.
fn compiled(host: &FakeHost, call: ToolCall) -> klipolo_chat::Compiled {
    let snap = host.snapshot().expect("a project");
    let out = compile(&snap.project, &snap.inputs, &[call]);
    assert!(out.refusals.is_empty(), "{:?}", out.refusals);
    out
}

/// An addition: the square format, the others kept.
fn square(host: &FakeHost) -> Transaction {
    compiled(
        host,
        ToolCall::SetFormat {
            format: FormatKind::Square,
            keep_others: true,
        },
    )
    .additions
    .expect("an addition")
}

/// A removal: all of c1 (0.0 to 13.0, the video's start and a clip edge).
fn cut_c1(host: &FakeHost) -> (ToolCall, Proposal) {
    let call = ToolCall::CutRange {
        range: RangeUs {
            start: 0,
            end: 13_000_000,
        },
    };
    let proposal = compiled(host, call.clone()).proposal.expect("a proposal");
    (call, proposal)
}

#[test]
fn a_proposal_that_appears_before_the_commit_refuses_an_apps_batch() {
    let host = FakeHost::two_clips();
    // The app looks: nothing waits. It compiles its addition on this revision.
    assert_eq!(host.open_proposal(), None);
    let base = host.revision();
    let tx = square(&host);
    let before = host.project();
    // Before it commits, the person's own proposal appears in Klipolo's chat.
    let (_, proposal) = cut_c1(&host);
    let id = host.chat_proposes(proposal);
    assert_eq!(host.apply("Agent", base, &tx), Err(HostError::Moved));
    // Nothing committed, and the person's proposal still waits.
    assert_eq!(host.revision(), base);
    assert_eq!(host.project(), before);
    assert_eq!(host.state().doc.as_ref().unwrap().undo_steps(), 0);
    let open = host.open_proposal().expect("still waiting");
    assert_eq!((open.id.as_str(), open.from), (id.as_str(), None));

    // Another app's proposal refuses it the same way.
    host.person_discards();
    let (call, proposal) = cut_c1(&host);
    host.propose("Other app", base, &[call], proposal, None)
        .expect("the other app proposes");
    assert_eq!(host.apply("Agent", base, &tx), Err(HostError::Moved));
    assert_eq!(host.revision(), base);
}

#[test]
fn an_apps_undo_waits_for_someone_elses_proposal_too() {
    let host = FakeHost::two_clips();
    let base = host.revision();
    let applied = host.apply("Agent", base, &square(&host)).expect("applies");
    assert_eq!(applied.revision, base + 1);
    let after = host.project();
    let (call, proposal) = cut_c1(&host);
    host.propose("Other app", applied.revision, &[call], proposal, None)
        .expect("the other app proposes");
    assert_eq!(host.undo_own("Agent"), Err(HostError::Moved));
    assert_eq!(host.revision(), applied.revision);
    assert_eq!(host.project(), after);
    // Once the person decides, the app's undo goes through.
    host.person_discards();
    assert_eq!(
        host.undo_own("Agent").map(|a| a.revision),
        Ok(applied.revision + 1)
    );
}

#[test]
fn an_apps_own_proposal_never_blocks_its_batches_and_a_commit_drops_it() {
    let host = FakeHost::two_clips();
    let base = host.revision();
    let (call, proposal) = cut_c1(&host);
    let id = host
        .propose("Agent", base, &[call], proposal, None)
        .expect("proposes");
    let applied = host.apply("Agent", base, &square(&host)).expect("applies");
    assert_eq!(applied.revision, base + 1);
    // The revision changed: its proposal was judged on the one before, so it is dropped.
    assert_eq!(host.open_proposal(), None);
    assert_eq!(
        host.outcome("Agent"),
        ProposalOutcome::Dropped {
            id,
            why: "The project changed.".into()
        }
    );
}
