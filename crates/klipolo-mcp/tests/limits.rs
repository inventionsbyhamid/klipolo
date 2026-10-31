//! What an app may do in a while, and what of the undo history apps may hold: the limits on their own, with times
//! given, then through the server.

#[path = "support/agent.rs"]
mod agent;
#[path = "support/fake_host.rs"]
mod fake_host;

use std::time::{Duration, Instant};

use agent::{Agent, assert_paced, outcome, request, says_paced};
use fake_host::FakeHost;
use klipolo_mcp::limits::{
    APPLIED_ALL_PER_WINDOW, APPLIED_PER_WINDOW, CALLS_PER_MINUTE, Limits, OPENED_ALL_PER_MINUTE,
    OPENED_PER_MINUTE, PACE_WINDOW, Paced, undo_shares,
};
use klipolo_mcp::rpc::{Handled, Mcp};
use klipolo_mcp::words::{OVER_CAP, paced};
use klipolo_model::history::{HistoryLimits, Whose};
use serde_json::json;

fn at(t0: Instant, secs: f64) -> Instant {
    t0 + Duration::from_secs_f64(secs)
}

#[test]
fn calls_past_60_a_minute_are_paced() {
    assert_eq!(CALLS_PER_MINUTE, 60);
    let limits = Limits::default();
    let t0 = Instant::now();
    for k in 0..60 {
        assert_eq!(limits.call("Agent", at(t0, k as f64 * 0.5)), Ok(()), "{k}");
    }
    // The oldest call leaves the minute at 60 s: 30 s to wait.
    assert_eq!(
        limits.call("Agent", at(t0, 30.0)),
        Err(Paced::Wait(Duration::from_secs(30)))
    );
    // Each app has its own pace.
    assert_eq!(limits.call("Other app", at(t0, 30.0)), Ok(()));
    // A minute after the first call there is room for one more.
    assert_eq!(limits.call("Agent", at(t0, 60.0)), Ok(()));
    assert_eq!(
        limits.call("Agent", at(t0, 60.0)),
        Err(Paced::Wait(Duration::from_millis(500)))
    );

    // Through the server: the 61st edit call within a minute is answered with the pace, and nothing is done.
    let host = FakeHost::two_clips();
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    for _ in 0..60 {
        let result = agent.call("cutRange", json!({}));
        assert_eq!(result["isError"], true);
    }
    let before = host.revision();
    let square = json!({"format": "square", "keepOthers": true});
    assert_paced(&agent.call("setFormat", square.clone()), "Agent");
    assert_eq!(host.revision(), before);
    // Reads share the pace, as tools and as resources.
    assert_paced(&agent.call("getContext", json!({})), "Agent");
    let context = read(&agent, "klipolo://context");
    assert!(
        says_paced(context["text"].as_str().unwrap_or_default(), "Agent"),
        "{context}"
    );
    // What became of a proposal is never paced: an app polls it while the person decides.
    let proposal = agent.call("getProposal", json!({}));
    assert_eq!(proposal["isError"], false, "{proposal}");
    assert_eq!(proposal["structuredContent"]["status"], "none");
    let polled = read(&agent, "klipolo://proposal");
    assert_eq!(polled["mimeType"], "application/json");
    // A resource Klipolo doesn't have is still said to be none, paced or not.
    let unknown = body(
        &agent,
        request(9, "resources/read", json!({"uri": "klipolo://nothing"})),
    );
    assert_eq!(unknown["error"]["code"], -32002);
    let other = Agent::open(&mcp, "Other app");
    assert_eq!(outcome(&other.call("setFormat", square.clone())), "done");

    // Reads count toward the pace like edits: 60 reads leave no room for an edit within the minute.
    let reader = Agent::open(&mcp, "Reader");
    for k in 0..60 {
        let read = if k % 2 == 0 {
            reader.call("getSummary", json!({}))
        } else {
            reader.call("getTranscript", json!({"fromSec": null, "toSec": null}))
        };
        assert_eq!(read["isError"], false, "{k}");
    }
    assert_paced(&reader.call("setFormat", square), "Reader");
}

/// The reply to `msg` in `agent`'s session.
fn body(agent: &Agent, msg: serde_json::Value) -> serde_json::Value {
    match agent.mcp.handle(Some(&agent.session), msg) {
        Handled::Reply { body, .. } => body,
        other => panic!("expected a reply, got {other:?}"),
    }
}

/// Resource `uri` as `agent` reads it: its one content.
fn read(agent: &Agent, uri: &str) -> serde_json::Value {
    let reply = body(agent, request(9, "resources/read", json!({"uri": uri})));
    reply["result"]["contents"][0].clone()
}

#[test]
fn applied_past_30_in_5_minutes_are_paced() {
    assert_eq!(
        (APPLIED_PER_WINDOW, PACE_WINDOW),
        (30, Duration::from_secs(5 * 60))
    );
    let limits = Limits::default();
    let t0 = Instant::now();
    for k in 0..30 {
        assert_eq!(
            limits.applied("Agent", at(t0, k as f64 * 9.0)),
            Ok(()),
            "{k}"
        );
    }
    assert_eq!(
        limits.applied("Agent", at(t0, 299.0)),
        Err(Paced::Wait(Duration::from_secs(1)))
    );
    assert_eq!(limits.applied("Agent", at(t0, 300.0)), Ok(()));

    // Through the server: the 31st batch the app applies within 5 minutes waits, and nothing changes.
    let host = FakeHost::two_clips();
    let mcp = Mcp::new(host.clone());
    let agent = Agent::open(&mcp, "Agent");
    let before = host.revision();
    for k in 0..30 {
        let result = agent.call("setCaptions", json!({"on": k % 2 == 1, "style": null}));
        assert_eq!(outcome(&result), "done", "{k}: {result}");
    }
    assert_eq!(host.revision(), before + 30);
    // The captions are on: turning them off would be the 31st batch.
    assert!(host.project().captions.enabled);
    assert_paced(
        &agent.call("setCaptions", json!({"on": false, "style": null})),
        "Agent",
    );
    assert_eq!(host.revision(), before + 30);
    // A removal is never applied here: it is still proposed for the person.
    agent.read();
    assert_eq!(outcome(&agent.cut(0.0, 13.0)), "proposed");
}

#[test]
fn all_apps_share_60() {
    assert_eq!(APPLIED_ALL_PER_WINDOW, 60);
    let limits = Limits::default();
    let t0 = Instant::now();
    for app in ["Agent", "Other app"] {
        for _ in 0..30 {
            assert_eq!(limits.applied(app, t0), Ok(()), "{app}");
        }
    }
    // A new name doesn't start over: all apps together reached theirs.
    let five_minutes = Paced::Wait(Duration::from_secs(300));
    assert_eq!(limits.applied("Third app", t0), Err(five_minutes));
    assert_eq!(limits.applied("Agent", t0), Err(five_minutes));
    assert_eq!(limits.applied("Third app", at(t0, 300.0)), Ok(()));
}

#[test]
fn initialize_past_10_a_minute_is_paced() {
    assert_eq!((OPENED_PER_MINUTE, OPENED_ALL_PER_MINUTE), (10, 20));
    let limits = Limits::default();
    let t0 = Instant::now();
    for _ in 0..10 {
        assert_eq!(limits.opened("Agent", t0), Ok(()));
    }
    let minute = Paced::Wait(Duration::from_secs(60));
    assert_eq!(limits.opened("Agent", t0), Err(minute));
    // Names an app picks itself: all apps together open at most 20 a minute, so new names can't push out the
    // sessions of apps already connected.
    for k in 0..10 {
        assert_eq!(limits.opened(&format!("App {k}"), t0), Ok(()), "{k}");
    }
    assert_eq!(limits.opened("Yet another app", t0), Err(minute));
    assert_eq!(limits.opened("Yet another app", at(t0, 60.0)), Ok(()));

    // Through the server: the 11th `initialize` of one app within a minute is paced and opens nothing.
    let host = FakeHost::two_clips();
    let mcp = Mcp::new(host.clone());
    for _ in 0..10 {
        Agent::open(&mcp, "Agent");
    }
    let params = json!({"protocolVersion": "2025-06-18", "clientInfo": {"name": "Agent"}});
    assert_eq!(
        mcp.handle(None, request(1, "initialize", params)),
        Handled::Paced
    );
    assert_eq!(mcp.connected(), ["Agent"]);
    Agent::open(&mcp, "Other app");
    assert_eq!(mcp.connected(), ["Agent", "Other app"]);
}

#[test]
fn undo_shares_are_a_quarter_and_a_half() {
    let [app, apps] = undo_shares("Agent", HistoryLimits::default());
    assert_eq!(
        (app.of, app.steps, app.bytes),
        (Whose::App("Agent"), 50, 16 << 20)
    );
    assert_eq!(
        (apps.of, apps.steps, apps.bytes),
        (Whose::Apps, 100, 32 << 20)
    );
    // Under memory pressure the history keeps less, and so do the shares.
    let [app, apps] = undo_shares("Agent", HistoryLimits::under_pressure());
    assert_eq!((app.steps, app.bytes), (12, 4 << 20));
    assert_eq!((apps.steps, apps.bytes), (25, 8 << 20));
}

#[test]
fn paced_says_how_long_to_wait() {
    let said = |wait: Duration| paced("Agent", Paced::Wait(wait));
    assert_eq!(
        said(Duration::from_secs(30)),
        "Klipolo is pacing calls from Agent. Wait 30 seconds, then try again."
    );
    // Rounded up, never "0 seconds".
    assert_eq!(
        said(Duration::from_millis(200)),
        "Klipolo is pacing calls from Agent. Wait 1 second, then try again."
    );
    assert_eq!(
        said(Duration::from_millis(59_200)),
        "Klipolo is pacing calls from Agent. Wait 1 minute, then try again."
    );
    assert_eq!(
        said(Duration::from_secs(241)),
        "Klipolo is pacing calls from Agent. Wait 5 minutes, then try again."
    );
    assert_eq!(
        said(Duration::from_secs(600)),
        "Klipolo is pacing calls from Agent. Wait 10 minutes, then try again."
    );
    // The run's allowance is used: no wait helps.
    assert_eq!(paced("Agent", Paced::OverCap), OVER_CAP);
}
