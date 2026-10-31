//! Jobs that cost money (a sticker or a background for Muse Image to draw): they start only through the host's own
//! checks (a Meta key or an access code, relay credit), within what apps may spend in one run of Klipolo, all apps
//! together, and within each app's pace of jobs. Edits that make no job never reach it.

#[path = "support/agent.rs"]
mod agent;
#[path = "support/fake_host.rs"]
mod fake_host;

use std::time::{Duration, Instant};

use agent::{Agent, assert_paced, assert_refused, outcome, text};
use fake_host::FakeHost;
use klipolo_chat::Job;
use klipolo_mcp::limits::{Limits, PAID_JOBS_PER_WINDOW, PAID_RUN_CAP_USD, PAID_WINDOW, Paced};
use klipolo_mcp::rpc::Mcp;
use klipolo_mcp::words::{LOW_CREDIT, NO_CREDIT, OVER_CAP};
use serde_json::{Value, json};

fn sticker() -> Value {
    json!({"prompt": "paper plane", "position": null, "atSec": null, "durationSec": null})
}

#[test]
fn no_paid_job_without_a_key_or_code() {
    let host = FakeHost::two_clips();
    host.state().credit = false;
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    let before = host.revision();
    assert_refused(&agent.call("makeSticker", sticker()), NO_CREDIT);
    assert!(host.state().started.is_empty());
    assert_eq!(host.state().counts.paid_asked, 1);
    assert_eq!(host.revision(), before);
    // Nothing was spent: once the person adds a key, the whole allowance is there (6 jobs at $0.04).
    host.state().credit = true;
    for k in 0..6 {
        let result = agent.call("makeSticker", sticker());
        assert_eq!(outcome(&result), "started", "{k}: {result}");
    }
    assert_eq!(host.state().started.len(), 6);
}

#[test]
fn paid_jobs_stop_at_the_run_allowance_across_clients() {
    assert_eq!(PAID_RUN_CAP_USD, 0.25);
    let host = FakeHost::two_clips();
    let mcp = Mcp::new(host.clone());
    let (a, b) = (Agent::open(&mcp, "Agent"), Agent::open(&mcp, "Other app"));
    for _ in 0..3 {
        for app in [&a, &b] {
            let result = app.call("makeSticker", sticker());
            assert_eq!(result["isError"], false, "{result}");
            assert_eq!(
                text(&result),
                "Started in Klipolo (makeSticker). What it makes appears in the edit once it is ready."
            );
            assert_eq!(result["structuredContent"], json!({"outcome": "started"}));
        }
    }
    // $0.24 spent: one more at $0.04 would pass $0.25, whichever app asks, and a new name doesn't start over.
    let c = Agent::open(&mcp, "Third app");
    for app in [&a, &b, &c] {
        assert_refused(&app.call("makeSticker", sticker()), OVER_CAP);
    }
    let s = host.state();
    assert_eq!(s.started.len(), 6);
    assert!(matches!(s.started[0], (ref client, Job::MakeSticker(_)) if client == "Agent"));
    assert_eq!(s.counts.paid_asked, 6, "never asked past the allowance");
    drop(s);

    let limits = Limits::default();
    let t0 = Instant::now();
    assert_eq!(limits.paid("Agent", t0, 0.25), Ok(()));
    assert_eq!(limits.paid("Other app", t0, 0.000_001), Err(Paced::OverCap));
    // A price that isn't a number is never under the allowance.
    assert_eq!(
        Limits::default().paid("Agent", t0, f64::NAN),
        Err(Paced::OverCap)
    );
}

#[test]
fn paid_jobs_past_10_in_10_minutes_are_paced() {
    assert_eq!(
        (PAID_JOBS_PER_WINDOW, PAID_WINDOW),
        (10, Duration::from_secs(10 * 60))
    );
    let limits = Limits::default();
    let t0 = Instant::now();
    for k in 0..10 {
        assert_eq!(limits.paid("Agent", t0, 0.001), Ok(()), "{k}");
    }
    assert_eq!(
        limits.paid("Agent", t0, 0.001),
        Err(Paced::Wait(Duration::from_secs(600)))
    );
    assert_eq!(limits.paid("Other app", t0, 0.001), Ok(()));
    assert_eq!(
        limits.paid("Agent", t0 + Duration::from_secs(600), 0.001),
        Ok(())
    );

    // Through the server: an app looping on stickers is paced, another app is not.
    let host = FakeHost::two_clips();
    host.state().usd_per_job = 0.001;
    let mcp = Mcp::new(host.clone());
    let (a, b) = (Agent::open(&mcp, "Agent"), Agent::open(&mcp, "Other app"));
    for k in 0..10 {
        assert_eq!(outcome(&a.call("makeSticker", sticker())), "started", "{k}");
    }
    assert_paced(&a.call("makeSticker", sticker()), "Agent");
    assert_eq!(outcome(&b.call("makeSticker", sticker())), "started");
    assert_eq!(host.state().started.len(), 11);
}

#[test]
fn edits_without_jobs_never_reach_start_paid() {
    let host = FakeHost::two_clips();
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    agent.read();
    for (tool, arguments) in [
        ("setFormat", json!({"format": "square", "keepOthers": true})),
        ("setCaptions", json!({"on": false, "style": null})),
        (
            "findMoments",
            json!({"query": "the pool", "ranges": [{"startSec": 0, "endSec": 13, "label": "the pool"}]}),
        ),
        ("undo", json!({})),
        ("cutRange", json!({"startSec": 0, "endSec": 13})),
    ] {
        let result = agent.call(tool, arguments);
        assert_eq!(result["isError"], false, "{tool}: {result}");
    }
    let s = host.state();
    assert_eq!((s.counts.estimates, s.counts.paid_asked), (0, 0));
    assert!(s.started.is_empty());
}

#[test]
fn too_little_relay_credit_is_told_apart_from_no_key() {
    let host = FakeHost::two_clips();
    host.state().low_credit = true;
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    assert_refused(&agent.call("makeSticker", sticker()), LOW_CREDIT);
    assert_eq!(
        LOW_CREDIT,
        "Klipolo's AI credit is too low for this. The person can add credit in Klipolo's Settings."
    );
    assert!(host.state().started.is_empty());
    // Nothing was spent: with credit added, the whole allowance is there.
    host.state().low_credit = false;
    for k in 0..6 {
        assert_eq!(
            outcome(&agent.call("makeSticker", sticker())),
            "started",
            "{k}"
        );
    }
}
