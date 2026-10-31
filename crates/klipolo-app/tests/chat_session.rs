//! Chat in the app: a message's answer applied, proposed and remembered against the open project. The model is a fake
//! that answers from a script (and can be held mid-answer), the analysis a fake with "talk.mov"'s words, and the
//! project a `MemoryProjects` with its own history, so every undo step and revision is the real `Document`'s.
//!
//! "talk.mov" is 30 s, played whole; its words run from 0.5 to 7.8 s ("Um, so today we look at the new module. It took
//! [pause 1.5 s] uh a long time."), with two fillers: "Um," at 0.5 and "uh" at 6.3.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use klipolo_app::analysis::AssetAnalysis;
use klipolo_app::chat::session::{KEEP_ONLY_READY, MOVED, NEEDS_KEY, OTHER_PROJECT};
use klipolo_app::chat::{
    BLUR_COMING, ChatOutcome, ChatRequest, ChatSession, CommitError, FOLLOW_COMING, Findings,
    MemoryProjects, Models, Projects, Readiness,
};
use klipolo_chat::{
    AnalysisState, BoxFuture, FunctionCall, Item, LanguageModel, ModelError, ModelRequest,
    ModelResponse,
};
use klipolo_model::ops::{Op, Origin};
use klipolo_model::project::{Asset, AssetKind, FormatKind};
use klipolo_model::speech::{Levels, Transcript, Word, WordKind};
use klipolo_model::time::{RangeUs, Us};
use klipolo_model::timeline::{Clip, ClipSource};
use klipolo_model::{Project, Transaction};
use serde_json::Value;
use tokio::sync::Notify;

fn secs(s: f64) -> Us {
    (s * 1e6).round() as Us
}

/// Words said one after another from 0.5 s, each 0.3 s long with 0.1 s between; `|1.5` makes the next gap 1.5 s.
/// "Um," and "uh" are fillers; a gap of 0.7 s or more holds a pause word, as the analysis writes them.
fn said(script: &str) -> Vec<Word> {
    let mut words: Vec<Word> = Vec::new();
    let mut gap = 0.1;
    let mut at = 0.4;
    for token in script.split_whitespace() {
        if let Some(g) = token.strip_prefix('|') {
            gap = g.parse().unwrap();
            continue;
        }
        let start = at + gap;
        if gap >= 0.7 {
            words.push(Word {
                text: String::new(),
                start_us: secs(at),
                end_us: secs(start),
                confidence: 1.0,
                kind: WordKind::Pause,
            });
        }
        let filler = matches!(token.to_lowercase().trim_end_matches(','), "um" | "uh");
        words.push(Word {
            text: token.into(),
            start_us: secs(start),
            end_us: secs(start + 0.3),
            confidence: 0.9,
            kind: if filler {
                WordKind::Filler
            } else {
                WordKind::Word
            },
        });
        at = start + 0.3;
        gap = 0.1;
    }
    words
}

/// "talk.mov", 30 s, played whole.
fn talk() -> Project {
    let mut p = Project::new("Talk");
    p.assets = vec![Asset {
        id: "talk".into(),
        kind: AssetKind::Video,
        path: "/media/talk.mov".into(),
        name: "talk.mov".into(),
        duration_us: secs(30.0),
        width: 1920,
        height: 1080,
        fps: 30.0,
        rotation: None,
        has_audio: true,
        audio_lead_us: None,
    }];
    let mut clip = Clip::new(ClipSource::Asset { id: "talk".into() }, 0, secs(30.0));
    clip.id = "t".into();
    p.timeline.main = vec![clip];
    p
}

fn transcript() -> Arc<Transcript> {
    Arc::new(Transcript {
        key: "key".into(),
        language: "en".into(),
        model: "test".into(),
        covered_us: secs(30.0),
        complete: true,
        words: said("Um, so today we look at the new module. It took |1.5 uh a long time."),
    })
}

/// What the analysis found: "talk.mov"'s words, or nothing yet (`state`).
struct Found {
    state: Mutex<AnalysisState>,
}

impl Found {
    fn done() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(AnalysisState::Done),
        })
    }

    fn not_started() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(AnalysisState::NotStarted),
        })
    }
}

impl Findings for Found {
    fn found(&self, asset_id: &str) -> Option<Arc<AssetAnalysis>> {
        (asset_id == "talk" && *self.state.lock().unwrap() == AnalysisState::Done).then(|| {
            Arc::new(AssetAnalysis {
                transcript: Some(transcript()),
                ..AssetAnalysis::default()
            })
        })
    }

    fn state(&self, _asset_id: &str) -> AnalysisState {
        *self.state.lock().unwrap()
    }

    fn levels(&self) -> HashMap<String, Arc<Levels>> {
        HashMap::new()
    }
}

/// A model that answers from a script, in order, keeps every request, and can be held before it answers.
#[derive(Default)]
struct Script {
    answers: Mutex<VecDeque<Result<ModelResponse, ModelError>>>,
    seen: Mutex<Vec<ModelRequest>>,
    /// While set, each answer waits for a `release`.
    hold: Mutex<Option<Arc<Notify>>>,
    /// Told when a request arrives.
    arrived: Notify,
    in_flight: AtomicUsize,
    most_at_once: AtomicUsize,
}

impl Script {
    fn new(answers: Vec<Result<ModelResponse, ModelError>>) -> Arc<Self> {
        Arc::new(Self {
            answers: Mutex::new(answers.into()),
            ..Self::default()
        })
    }

    fn seen(&self) -> Vec<ModelRequest> {
        self.seen.lock().unwrap().clone()
    }

    /// Every answer from now on waits for `release`.
    fn hold(&self) -> Arc<Notify> {
        let gate = Arc::new(Notify::new());
        *self.hold.lock().unwrap() = Some(gate.clone());
        gate
    }
}

impl LanguageModel for Script {
    fn respond<'a>(
        &'a self,
        request: &'a ModelRequest,
    ) -> BoxFuture<'a, Result<ModelResponse, ModelError>> {
        Box::pin(async move {
            let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.most_at_once.fetch_max(now, Ordering::SeqCst);
            self.seen.lock().unwrap().push(request.clone());
            self.arrived.notify_one();
            let gate = self.hold.lock().unwrap().clone();
            if let Some(gate) = gate {
                gate.notified().await;
            }
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            (self.answers.lock().unwrap().pop_front())
                .unwrap_or_else(|| Err(ModelError::other("the script ran out")))
        })
    }
}

/// How the fake credentials answer: the model behind them, through the relay or not, and how many renewals.
struct Paying {
    readiness: Mutex<Readiness>,
    recovered: AtomicUsize,
    /// Whether registering again works.
    renews: std::sync::atomic::AtomicBool,
}

impl Paying {
    fn key(model: Arc<Script>) -> Arc<Self> {
        Self::with(Readiness::Ready {
            model,
            relay: false,
        })
    }

    fn relay(model: Arc<Script>) -> Arc<Self> {
        Self::with(Readiness::Ready { model, relay: true })
    }

    fn with(readiness: Readiness) -> Arc<Self> {
        Arc::new(Self {
            readiness: Mutex::new(readiness),
            recovered: AtomicUsize::new(0),
            renews: std::sync::atomic::AtomicBool::new(true),
        })
    }
}

impl Models for Paying {
    fn ready(&self) -> BoxFuture<'_, Readiness> {
        let readiness = self.readiness.lock().unwrap().clone();
        Box::pin(async move { readiness })
    }

    fn recover(&self) -> BoxFuture<'_, bool> {
        self.recovered.fetch_add(1, Ordering::SeqCst);
        let renews = self.renews.load(Ordering::SeqCst);
        Box::pin(async move { renews })
    }
}

struct Chat {
    session: Arc<ChatSession>,
    project: Arc<MemoryProjects>,
    model: Arc<Script>,
}

fn chat_with(model: Arc<Script>, found: Arc<Found>, paying: Arc<Paying>) -> Chat {
    let project =
        Arc::new(MemoryProjects::open(talk(), PathBuf::from("/tmp/talk.klipolo")).unwrap());
    let session = Arc::new(ChatSession::new(project.clone(), found, paying));
    Chat {
        session,
        project,
        model,
    }
}

/// A chat on "talk.mov", analyzed, paid with a key.
fn chat(answers: Vec<Result<ModelResponse, ModelError>>) -> Chat {
    let model = Script::new(answers);
    chat_with(model.clone(), Found::done(), Paying::key(model))
}

fn request(text: &str) -> ChatRequest {
    serde_json::from_value(serde_json::json!({
        "id": format!("m-{text}"),
        "text": text,
        "selection": null,
        "item": null,
        "playheadUs": 0,
        "format": null,
    }))
    .unwrap()
}

fn selecting(text: &str, range: RangeUs) -> ChatRequest {
    ChatRequest {
        selection: Some(range),
        ..request(text)
    }
}

fn fc(call_id: &str, name: &str, arguments: &str) -> FunctionCall {
    FunctionCall {
        name: name.into(),
        call_id: call_id.into(),
        arguments: arguments.into(),
    }
}

fn answer(text: &str, calls: Vec<FunctionCall>) -> Result<ModelResponse, ModelError> {
    Ok(ModelResponse {
        text: text.into(),
        calls,
        usage_usd: 0.005,
    })
}

fn fillers() -> FunctionCall {
    fc("f", "removeFillers", "{}")
}

fn vertical_karaoke() -> Vec<FunctionCall> {
    vec![
        fc(
            "v",
            "setFormat",
            r#"{"format": "portrait", "keepOthers": true}"#,
        ),
        fc("k", "setCaptions", r#"{"on": true, "style": "karaoke"}"#),
    ]
}

/// The outcome as the page reads it.
fn json(outcome: &ChatOutcome) -> Value {
    serde_json::to_value(outcome).unwrap()
}

fn reply(outcome: &ChatOutcome) -> Value {
    let v = json(outcome);
    assert_eq!(v["type"], "reply", "{v:#}");
    v
}

fn proposal_id(outcome: &ChatOutcome) -> String {
    reply(outcome)["proposal"]["id"]
        .as_str()
        .expect("a proposal")
        .to_owned()
}

fn rename(name: &str) -> Transaction {
    Transaction {
        label: "Rename".into(),
        ops: vec![Op::Rename { name: name.into() }],
        ..Default::default()
    }
}

/// Whether the moment `at` s of "talk.mov" still plays on the main track.
fn plays(project: &Project, at: f64) -> bool {
    let at = secs(at);
    (project.timeline.main.iter()).any(|clip| clip.source_in_us <= at && at < clip.source_out_us)
}

#[tokio::test]
async fn additions_apply_at_once_as_one_undo_step_from_chat() {
    let chat = chat(vec![answer("Done.", vertical_karaoke())]);
    let out = chat
        .session
        .send(request("make it vertical with karaoke captions"))
        .await;
    let r = reply(&out);
    assert_eq!(r["applied"]["change"]["baseRevision"], 0);
    assert_eq!(r["applied"]["change"]["history"]["revision"], 1);
    assert_eq!(r["applied"]["undo"]["code"], "chat.andMore");
    assert!(r["proposal"].is_null());
    let p = chat.project.project();
    assert_eq!(p.settings.primary_format, FormatKind::Portrait);
    assert_eq!(chat.project.undo_steps(), 1, "one undo step");
    // One step back: both changes go.
    chat.project.step(false).unwrap();
    assert_eq!(
        chat.project.project().settings.primary_format,
        FormatKind::Landscape
    );
}

#[tokio::test]
async fn apply_is_one_undo_step() {
    let chat = chat(vec![answer("These are the fillers.", vec![fillers()])]);
    let before = chat.project.project();
    let out = chat.session.send(request("cut the ums")).await;
    let r = reply(&out);
    assert!(r["applied"].is_null(), "removals wait for Apply");
    assert_eq!(chat.project.revision(), 0, "nothing changed yet");
    assert_eq!(r["proposal"]["baseRevision"], 0);
    let ranges = r["proposal"]["removals"]["ranges"].as_array().unwrap();
    assert_eq!(ranges.len(), 2, "both fillers: {r:#}");
    assert_eq!(r["proposal"]["title"], "Cut 2 filler words, 1.0 s in total");
    let tags: Vec<&str> = (r["proposal"]["tags"].as_array().unwrap().iter())
        .map(|t| t["label"].as_str().unwrap())
        .collect();
    assert_eq!(tags, ["um", "uh"]);

    let applied = chat.session.apply(&proposal_id(&out)).unwrap();
    assert_eq!(applied.change.base_revision, 0);
    assert_eq!(chat.project.undo_steps(), 1, "Apply is one undo step");
    let after = chat.project.project();
    assert!(
        !plays(&after, 0.6) && !plays(&after, 6.45),
        "the fillers are cut"
    );
    assert!(plays(&after, 1.2), "the words around them stay");
    let removed: Us = (after.timeline.main.iter())
        .map(|c| c.source_out_us - c.source_in_us)
        .sum();
    assert_eq!(
        secs(30.0) - removed,
        r["proposal"]["removedUs"].as_i64().unwrap(),
        "what goes is what the card said"
    );

    // Undo right after Apply brings the cut footage back; Redo cuts it again.
    chat.project.step(false).unwrap();
    assert_eq!(*chat.project.project(), *before);
    chat.project.step(true).unwrap();
    assert_eq!(*chat.project.project(), *after);
}

#[tokio::test]
async fn stale_proposal_is_dropped_after_a_hand_edit() {
    let chat = chat(vec![answer("", vec![fillers()])]);
    let out = chat.session.send(request("cut the ums")).await;
    let id = proposal_id(&out);
    chat.project.hand_edit(&rename("Edited by hand")).unwrap();
    assert_eq!(chat.session.apply(&id).unwrap_err(), CommitError::Moved);
    assert_eq!(chat.project.undo_steps(), 1, "only the hand edit");
    assert!(plays(&chat.project.project(), 0.6), "nothing was cut");
    // Dropped: it can't come back after an undo either.
    chat.project.step(false).unwrap();
    assert_eq!(chat.session.apply(&id).unwrap_err(), CommitError::Moved);
    assert_eq!(chat.project.revision(), 2);
    assert!(plays(&chat.project.project(), 0.6));
    let note = &chat.session.turns()[0].note;
    assert!(
        note.ends_with(
            "The proposal was dropped: the project changed before the person applied it."
        ),
        "{note}"
    );
}

#[tokio::test]
async fn discard_changes_nothing() {
    let chat = chat(vec![answer("", vec![fillers()])]);
    let out = chat.session.send(request("cut the ums")).await;
    let id = proposal_id(&out);
    chat.session.discard(&id);
    assert_eq!(chat.project.revision(), 0);
    assert_eq!(chat.session.apply(&id).unwrap_err(), CommitError::Moved);
    assert_eq!(chat.project.revision(), 0);
    assert!(
        chat.session.turns()[0]
            .note
            .ends_with("The person discarded the proposal.")
    );
}

#[tokio::test]
async fn additions_and_removals_of_one_answer_apply_and_propose_on_the_revision_after() {
    let mut calls = vertical_karaoke();
    calls.push(fillers());
    let chat = chat(vec![answer("", calls)]);
    let out = chat
        .session
        .send(request("vertical, karaoke, and cut the ums"))
        .await;
    let r = reply(&out);
    assert_eq!(r["applied"]["change"]["history"]["revision"], 1);
    assert_eq!(
        r["proposal"]["baseRevision"], 1,
        "the proposal applies after the additions"
    );
    chat.session.apply(&proposal_id(&out)).unwrap();
    assert_eq!(
        chat.project.undo_steps(),
        2,
        "the additions, then the proposal"
    );
    assert_eq!(
        chat.project.project().settings.primary_format,
        FormatKind::Portrait
    );
    assert!(!plays(&chat.project.project(), 0.6));
}

#[tokio::test]
async fn a_second_message_waits_and_reads_the_project_the_first_left() {
    let model = Script::new(vec![
        answer("Done.", vertical_karaoke()),
        answer("Here they are.", vec![fillers()]),
    ]);
    let gate = model.hold();
    let chat = chat_with(model.clone(), Found::done(), Paying::key(model.clone()));
    let first = tokio::spawn({
        let session = chat.session.clone();
        async move { session.send(request("vertical karaoke")).await }
    });
    model.arrived.notified().await;
    let second = tokio::spawn({
        let session = chat.session.clone();
        async move { session.send(request("cut the ums")).await }
    });
    // The second waits its turn: nothing of it is read, let alone sent, while the first is out.
    tokio::task::yield_now().await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(model.seen().len(), 1);
    gate.notify_one();
    let first = first.await.unwrap();
    model.arrived.notified().await;
    gate.notify_one();
    let second = second.await.unwrap();
    assert_eq!(
        model.most_at_once.load(Ordering::SeqCst),
        1,
        "one message in flight"
    );

    assert_eq!(reply(&first)["applied"]["change"]["history"]["revision"], 1);
    // The second message's context was built on the first's result.
    let context = match &model.seen()[1].input[0] {
        Item::Context { text } => text.clone(),
        other => panic!("the context comes first: {other:?}"),
    };
    assert!(context.contains("portrait"), "{context}");
    assert_eq!(reply(&second)["proposal"]["baseRevision"], 1);
}

#[tokio::test]
async fn a_hand_edit_while_the_model_answers_leaves_the_answer_unapplied() {
    let model = Script::new(vec![answer("Made it vertical.", vertical_karaoke())]);
    let gate = model.hold();
    let chat = chat_with(model.clone(), Found::done(), Paying::key(model.clone()));
    let sent = tokio::spawn({
        let session = chat.session.clone();
        async move { session.send(request("vertical karaoke")).await }
    });
    model.arrived.notified().await;
    chat.project.hand_edit(&rename("Mine")).unwrap();
    gate.notify_one();
    let out = sent.await.unwrap();
    let r = reply(&out);
    assert_eq!(r["text"], MOVED, "its words don't claim it");
    assert!(r["applied"].is_null());
    assert_eq!(chat.project.undo_steps(), 1, "only the hand edit");
    assert_eq!(
        chat.project.project().settings.primary_format,
        FormatKind::Landscape
    );
    assert!(chat.session.turns()[0].note.contains("Nothing was applied"));
}

#[tokio::test]
async fn errors_say_one_sentence_and_never_touch_the_project() {
    let cases = [
        (
            ModelError::Budget("Your access code's $2.00 is used up.".into()),
            "budget",
            "Your access code's $2.00 is used up.",
        ),
        (ModelError::Busy, "busy", klipolo_chat::planner::BUSY),
        (
            ModelError::Network,
            "network",
            klipolo_chat::planner::NO_CONNECTION,
        ),
        (
            ModelError::Unavailable(None),
            "unavailable",
            klipolo_chat::planner::UNAVAILABLE,
        ),
        (
            ModelError::Declined("Meta can't help with that request.".into()),
            "declined",
            "Meta can't help with that request.",
        ),
    ];
    for (error, kind, message) in cases {
        let chat = chat(vec![Err(error)]);
        let out = chat.session.send(request("cut the ums")).await;
        let v = json(&out);
        assert_eq!(v["type"], "failed", "{v:#}");
        assert_eq!(v["kind"], kind);
        assert_eq!(v["message"], message);
        assert_eq!(chat.project.revision(), 0, "untouched");
    }
}

#[tokio::test]
async fn no_key_and_no_code_asks_for_one_and_a_registration_problem_says_why() {
    let model = Script::new(vec![]);
    let chat = chat_with(
        model.clone(),
        Found::done(),
        Paying::with(Readiness::NeedsKey),
    );
    let v = json(&chat.session.send(request("cut the ums")).await);
    assert_eq!(
        (v["kind"].as_str(), v["message"].as_str()),
        (Some("needsKey"), Some(NEEDS_KEY))
    );

    let problem =
        "Couldn't reach Klipolo to set up this Mac. Check your internet connection and try again.";
    let chat = chat_with(
        model.clone(),
        Found::done(),
        Paying::with(Readiness::Problem(problem.into())),
    );
    let v = json(&chat.session.send(request("cut the ums")).await);
    assert_eq!(
        (v["kind"].as_str(), v["message"].as_str()),
        (Some("registration"), Some(problem))
    );
    assert!(model.seen().is_empty(), "nothing was sent");
}

#[tokio::test]
async fn a_token_the_relay_refused_is_renewed_once_and_the_message_sent_again() {
    let model = Script::new(vec![Err(ModelError::Refused), answer("", vec![fillers()])]);
    let paying = Paying::relay(model.clone());
    let chat = chat_with(model.clone(), Found::done(), paying.clone());
    let out = chat.session.send(request("cut the ums")).await;
    assert!(reply(&out)["proposal"].is_object());
    assert_eq!(paying.recovered.load(Ordering::SeqCst), 1);

    assert_eq!(
        chat.session.spend().messages,
        2,
        "both tries reached the model"
    );

    // A renewal that fails: the refusal is said, its cost counted once.
    let model = Script::new(vec![Err(ModelError::Refused)]);
    let paying = Paying::relay(model.clone());
    paying.renews.store(false, Ordering::SeqCst);
    let chat = chat_with(model.clone(), Found::done(), paying.clone());
    let v = json(&chat.session.send(request("cut the ums")).await);
    assert_eq!(v["kind"], "refused");
    assert_eq!(chat.session.spend().messages, 1);
    assert_eq!(model.seen().len(), 1);

    // With the person's own key, a refusal is a wrong key: said, nothing renewed.
    let model = Script::new(vec![Err(ModelError::Refused)]);
    let paying = Paying::key(model.clone());
    let chat = chat_with(model.clone(), Found::done(), paying.clone());
    let v = json(&chat.session.send(request("cut the ums")).await);
    assert_eq!(v["kind"], "refused");
    assert_eq!(paying.recovered.load(Ordering::SeqCst), 0);
    assert_eq!(model.seen().len(), 1);
}

#[tokio::test]
async fn an_empty_message_is_not_sent() {
    let chat = chat(vec![]);
    let v = json(&chat.session.send(request("  ")).await);
    assert_eq!(v["kind"], "other");
    assert!(chat.model.seen().is_empty());
}

#[tokio::test]
async fn a_clip_not_analyzed_yet_is_named() {
    let model = Script::new(vec![answer("Removing them now.", vec![fillers()])]);
    let chat = chat_with(model.clone(), Found::not_started(), Paying::key(model));
    let r = reply(&chat.session.send(request("cut the ums")).await);
    assert_eq!(r["text"], "Analyze this clip first (talk.mov).");
    assert!(r["proposal"].is_null());
}

#[tokio::test]
async fn the_conversation_keeps_six_exchanges_and_starts_over_on_another_project() {
    let answers = (0..8).map(|_| {
        answer(
            "Done.",
            vec![fc("q", "askUser", r#"{"question": "Which part?"}"#)],
        )
    });
    let chat = chat(answers.collect());
    for n in 0..7 {
        chat.session.send(request(&format!("message {n}"))).await;
    }
    let turns = chat.session.turns();
    assert_eq!(turns.len(), 6);
    assert_eq!(turns[0].user, "message 1");
    assert!(
        turns[5].note.contains("Asked: Which part?"),
        "{}",
        turns[5].note
    );
    chat.session.send(request("message 7")).await;
    let users: Vec<String> = (model_input(&chat.model.seen()[7]).into_iter())
        .filter_map(|item| match item {
            Item::User { text } => Some(text),
            _ => None,
        })
        .collect();
    assert_eq!(
        users.len(),
        7,
        "six exchanges and the new message: {users:?}"
    );

    chat.project.switch(Some(talk())).unwrap();
    let model_calls = chat.model.seen().len();
    chat.session.send(request("hello again")).await;
    let users = (model_input(&chat.model.seen()[model_calls]).into_iter())
        .filter(|item| matches!(item, Item::User { .. }))
        .count();
    assert_eq!(users, 1, "another opening starts over");
}

fn model_input(request: &ModelRequest) -> Vec<Item> {
    request.input.clone()
}

#[tokio::test]
async fn another_project_drops_its_proposals() {
    let chat = chat(vec![answer("", vec![fillers()])]);
    let id = proposal_id(&chat.session.send(request("cut the ums")).await);
    chat.project.switch(Some(talk())).unwrap();
    assert_eq!(chat.session.apply(&id).unwrap_err(), CommitError::Moved);
    chat.session.project_changed();
    assert!(chat.session.turns().is_empty());
}

#[tokio::test]
async fn a_message_sent_to_a_project_that_closed_while_it_waited_is_not_sent() {
    let model = Script::new(vec![
        answer("Done.", vertical_karaoke()),
        answer("", vec![]),
    ]);
    let gate = model.hold();
    let chat = chat_with(model.clone(), Found::done(), Paying::key(model.clone()));
    let first = tokio::spawn({
        let session = chat.session.clone();
        async move { session.send(request("vertical")).await }
    });
    model.arrived.notified().await;
    let second = tokio::spawn({
        let session = chat.session.clone();
        async move { session.send(request("cut the ums")).await }
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    chat.project.switch(Some(talk())).unwrap();
    gate.notify_one();
    first.await.unwrap();
    let v = json(&second.await.unwrap());
    assert_eq!(v["message"], OTHER_PROJECT);
    assert_eq!(model.seen().len(), 1);
}

#[tokio::test]
async fn follow_and_blur_say_they_are_coming() {
    let chat = chat(vec![answer(
        "I'll blur it.",
        vec![fc(
            "b",
            "blurObjects",
            r#"{"description": "license plate"}"#,
        )],
    )]);
    let r = reply(&chat.session.send(request("blur the license plate")).await);
    assert_eq!(r["text"], BLUR_COMING, "the model's words don't claim it");
    assert_eq!(chat.project.revision(), 0);

    let mut calls = vertical_karaoke();
    calls.push(fc(
        "s",
        "followSubject",
        r#"{"description": "the speaker"}"#,
    ));
    let chat = self::chat(vec![answer("Done.", calls)]);
    let r = reply(&chat.session.send(request("vertical and follow her")).await);
    assert_eq!(r["text"], "Done.");
    assert_eq!(r["notes"], serde_json::json!([FOLLOW_COMING]));
    assert!(r["applied"].is_object());
    assert!(
        chat.session.turns()[0]
            .note
            .contains("Not available yet, nothing done: followSubject.")
    );
}

#[tokio::test]
async fn spend_counts_every_model_call() {
    let chat = chat(vec![
        answer("Done.", vertical_karaoke()),
        Err(ModelError::Other {
            detail: "cut off".into(),
            usage_usd: 0.003,
        }),
    ]);
    chat.session.send(request("vertical")).await;
    chat.session.send(request("again")).await;
    let spend = chat.session.spend();
    assert_eq!(spend.messages, 2);
    assert!((spend.chat_usd - 0.008).abs() < 1e-9, "{spend:?}");
}

#[tokio::test]
async fn keep_only_these_proposes_without_the_model() {
    let chat = chat(vec![]);
    let keep = ChatRequest {
        keep_only: vec![RangeUs {
            start: secs(0.9),
            end: secs(4.6),
        }],
        ..request("Keep only these")
    };
    let r = reply(&chat.session.send(keep).await);
    assert_eq!(r["text"], KEEP_ONLY_READY);
    assert!(
        r["proposal"]["title"]
            .as_str()
            .unwrap()
            .starts_with("Keep only the parts you chose, cutting")
    );
    assert!(chat.model.seen().is_empty());
}

#[tokio::test]
async fn the_selection_is_this() {
    let chat = chat(vec![answer(
        "",
        vec![fc("c", "cutRange", r#"{"startSec": 12.0, "endSec": 18.0}"#)],
    )]);
    let out = chat
        .session
        .send(selecting(
            "cut this",
            RangeUs {
                start: secs(12.0),
                end: secs(18.0),
            },
        ))
        .await;
    let context = match &chat.model.seen()[0].input[0] {
        Item::Context { text } => text.clone(),
        _ => unreachable!(),
    };
    assert!(
        context.contains("12.0") && context.contains("18.0"),
        "{context}"
    );
    let r = reply(&out);
    assert_eq!(r["proposal"]["title"], "Cut 6.0 s");
    assert_eq!(r["proposal"]["removedUs"], secs(6.0));
}

#[tokio::test]
async fn undo_last_takes_back_chats_change_only_while_it_is_the_last() {
    let chat = chat(vec![
        answer("", vertical_karaoke()),
        answer("", vertical_karaoke()),
    ]);
    chat.session.send(request("vertical")).await;
    chat.session.undo_last().unwrap();
    assert_eq!(
        chat.project.project().settings.primary_format,
        FormatKind::Landscape
    );
    assert_eq!(
        chat.session.undo_last().unwrap_err(),
        CommitError::Moved,
        "once"
    );

    chat.project.step(true).unwrap();
    chat.project.step(false).unwrap();
    chat.session.send(request("vertical again")).await;
    chat.project.hand_edit(&rename("Mine")).unwrap();
    assert_eq!(chat.session.undo_last().unwrap_err(), CommitError::Moved);
    assert_eq!(chat.project.project().name, "Mine");
}

#[tokio::test]
async fn the_models_undo_takes_back_the_top_step() {
    let chat = chat(vec![answer("Undone.", vec![fc("u", "undo", "{}")])]);
    chat.project.hand_edit(&rename("Mine")).unwrap();
    let r = reply(&chat.session.send(request("undo that")).await);
    assert_eq!(r["undone"]["baseRevision"], 1);
    assert_eq!(chat.project.project().name, "Talk");
}

#[test]
fn chat_edits_are_the_persons_with_the_chat_origin() {
    // What chat commits is a `Transaction` from chat: the compiler marks it, and the history keeps its label.
    let project = MemoryProjects::open(talk(), PathBuf::from("/tmp/t.klipolo")).unwrap();
    let now = project.now().unwrap();
    let tx = Transaction {
        label: "chat.setFormat".into(),
        ops: vec![Op::SetSettings {
            settings: klipolo_model::project::Settings {
                formats: vec![FormatKind::Landscape, FormatKind::Square],
                primary_format: FormatKind::Square,
                ..project.project().settings.clone()
            },
        }],
        origin: Some(Origin::Chat),
        ..Default::default()
    };
    let done = project.commit(&now.at, &tx, None).unwrap();
    assert_eq!(
        done.patch.history.undo_label.as_deref(),
        Some("chat.setFormat")
    );
    assert_eq!(
        project.commit(&now.at, &tx, None).unwrap_err(),
        CommitError::Moved
    );
}
