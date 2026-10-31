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

use klipolo_analysis::providers::images::{
    ImageError, ImageErrorKind, ImageMaker, ImageSize, MadeImage,
};
use klipolo_app::analysis::AssetAnalysis;
use klipolo_app::chat::pictures::{
    ChatPictureKind, MOMENT_CUT, PICTURE_GONE, PictureAsk, StickerPlacement, TITLE_GONE,
};
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
    /// The picture maker, or the sentence that says why there is none.
    images: Mutex<Result<Arc<dyn ImageMaker>, String>>,
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
            images: Mutex::new(Err(NEEDS_KEY.to_owned())),
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

    fn images(&self) -> BoxFuture<'_, Result<Arc<dyn ImageMaker>, String>> {
        let images = self.images.lock().unwrap().clone();
        Box::pin(async move { images })
    }

    fn picture_usd(&self) -> f64 {
        0.01
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
        "baseRevision": 0,
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
        .await
        .unwrap();
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
    let out = chat.session.send(request("cut the ums")).await.unwrap();
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
    let out = chat.session.send(request("cut the ums")).await.unwrap();
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
    let out = chat.session.send(request("cut the ums")).await.unwrap();
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
        .await
        .unwrap();
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
        async move { session.send(request("vertical karaoke")).await.unwrap() }
    });
    model.arrived.notified().await;
    let second = tokio::spawn({
        let session = chat.session.clone();
        async move { session.send(request("cut the ums")).await.unwrap() }
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
        async move { session.send(request("vertical karaoke")).await.unwrap() }
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
        let out = chat.session.send(request("cut the ums")).await.unwrap();
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
    let v = json(&chat.session.send(request("cut the ums")).await.unwrap());
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
    let v = json(&chat.session.send(request("cut the ums")).await.unwrap());
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
    let out = chat.session.send(request("cut the ums")).await.unwrap();
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
    let v = json(&chat.session.send(request("cut the ums")).await.unwrap());
    assert_eq!(v["kind"], "refused");
    assert_eq!(chat.session.spend().messages, 1);
    assert_eq!(model.seen().len(), 1);

    // With the person's own key, a refusal is a wrong key: said, nothing renewed.
    let model = Script::new(vec![Err(ModelError::Refused)]);
    let paying = Paying::key(model.clone());
    let chat = chat_with(model.clone(), Found::done(), paying.clone());
    let v = json(&chat.session.send(request("cut the ums")).await.unwrap());
    assert_eq!(v["kind"], "refused");
    assert_eq!(paying.recovered.load(Ordering::SeqCst), 0);
    assert_eq!(model.seen().len(), 1);
}

#[tokio::test]
async fn an_empty_message_is_not_sent() {
    let chat = chat(vec![]);
    let v = json(&chat.session.send(request("  ")).await.unwrap());
    assert_eq!(v["kind"], "other");
    assert!(chat.model.seen().is_empty());
}

#[tokio::test]
async fn a_clip_not_analyzed_yet_is_named() {
    let model = Script::new(vec![answer("Removing them now.", vec![fillers()])]);
    let chat = chat_with(model.clone(), Found::not_started(), Paying::key(model));
    let r = reply(&chat.session.send(request("cut the ums")).await.unwrap());
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
        chat.session
            .send(request(&format!("message {n}")))
            .await
            .unwrap();
    }
    let turns = chat.session.turns();
    assert_eq!(turns.len(), 6);
    assert_eq!(turns[0].user, "message 1");
    assert!(
        turns[5].note.contains("Asked: Which part?"),
        "{}",
        turns[5].note
    );
    chat.session.send(request("message 7")).await.unwrap();
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
    chat.session.send(request("hello again")).await.unwrap();
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
    let id = proposal_id(&chat.session.send(request("cut the ums")).await.unwrap());
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
        async move { session.send(request("vertical")).await.unwrap() }
    });
    model.arrived.notified().await;
    let second = tokio::spawn({
        let session = chat.session.clone();
        async move { session.send(request("cut the ums")).await.unwrap() }
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
    let r = reply(
        &chat
            .session
            .send(request("blur the license plate"))
            .await
            .unwrap(),
    );
    assert_eq!(r["text"], BLUR_COMING, "the model's words don't claim it");
    assert_eq!(chat.project.revision(), 0);

    let mut calls = vertical_karaoke();
    calls.push(fc(
        "s",
        "followSubject",
        r#"{"description": "the speaker"}"#,
    ));
    let chat = self::chat(vec![answer("Done.", calls)]);
    let r = reply(
        &chat
            .session
            .send(request("vertical and follow her"))
            .await
            .unwrap(),
    );
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
    chat.session.send(request("vertical")).await.unwrap();
    chat.session.send(request("again")).await.unwrap();
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
    let r = reply(&chat.session.send(keep).await.unwrap());
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
        .await
        .unwrap();
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
    chat.session.send(request("vertical")).await.unwrap();
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
    chat.session.send(request("vertical again")).await.unwrap();
    chat.project.hand_edit(&rename("Mine")).unwrap();
    assert_eq!(chat.session.undo_last().unwrap_err(), CommitError::Moved);
    assert_eq!(chat.project.project().name, "Mine");
}

#[tokio::test]
async fn the_models_undo_takes_back_the_top_step() {
    let chat = chat(vec![answer("Undone.", vec![fc("u", "undo", "{}")])]);
    chat.project.hand_edit(&rename("Mine")).unwrap();
    let r = reply(&chat.session.send(request("undo that")).await.unwrap());
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

#[tokio::test]
async fn a_new_proposal_replaces_the_open_one() {
    let chat = chat(vec![
        answer("", vec![fillers()]),
        answer(
            "",
            vec![fc("c", "cutRange", r#"{"startSec": 12.0, "endSec": 18.0}"#)],
        ),
    ]);
    let first = proposal_id(&chat.session.send(request("cut the ums")).await.unwrap());
    let second = chat
        .session
        .send(selecting(
            "cut this",
            RangeUs {
                start: secs(12.0),
                end: secs(18.0),
            },
        ))
        .await
        .unwrap();
    // Red always means "will be cut": only the newest proposal can apply.
    assert_eq!(chat.session.apply(&first).unwrap_err(), CommitError::Moved);
    assert_eq!(chat.project.revision(), 0);
    chat.session.apply(&proposal_id(&second)).unwrap();
    assert!(plays(&chat.project.project(), 0.6), "the ums stay");
    assert!(
        !plays(&chat.project.project(), 15.0),
        "the selection is cut"
    );
    assert!(
        chat.session.turns()[0]
            .note
            .ends_with("Replaced by a newer proposal.")
    );
}

#[tokio::test]
async fn keep_only_on_a_revision_the_project_left_is_refused() {
    let chat = chat(vec![]);
    chat.project.hand_edit(&rename("Mine")).unwrap();
    let stale = ChatRequest {
        keep_only: vec![RangeUs {
            start: secs(0.9),
            end: secs(4.6),
        }],
        ..request("Keep only these")
    };
    assert_eq!(
        chat.session.send(stale.clone()).await.unwrap_err(),
        CommitError::Moved
    );
    assert_eq!(chat.project.revision(), 1, "only the hand edit");
    // Made on the revision it is now, the moments stand.
    let fresh = ChatRequest {
        base_revision: 1,
        ..stale
    };
    assert!(reply(&chat.session.send(fresh).await.unwrap())["proposal"].is_object());
}

#[tokio::test]
async fn a_selection_made_on_a_revision_the_project_left_is_refused() {
    let chat = chat(vec![answer("", vec![fillers()])]);
    chat.project.hand_edit(&rename("Mine")).unwrap();
    let range = RangeUs {
        start: secs(12.0),
        end: secs(18.0),
    };
    let refused = chat.session.send(selecting("cut this", range)).await;
    assert_eq!(refused.unwrap_err(), CommitError::Moved);
    assert!(chat.model.seen().is_empty(), "nothing was sent");
    // Without a selection, "this" names nothing stale: the message is read on the project as it is when its turn comes.
    let out = chat.session.send(request("cut the ums")).await.unwrap();
    assert_eq!(reply(&out)["proposal"]["baseRevision"], 1);
}

// ---------- Pictures (Muse Image behind `ImageMaker`) ----------

/// An image maker that answers from a script and keeps what it was asked.
struct Painter {
    answers: Mutex<VecDeque<Result<Vec<MadeImage>, ImageError>>>,
    asked: Mutex<Vec<ImageSize>>,
}

impl Painter {
    fn new(answers: Vec<Result<Vec<MadeImage>, ImageError>>) -> Arc<Self> {
        Arc::new(Self {
            answers: Mutex::new(answers.into()),
            asked: Mutex::new(Vec::new()),
        })
    }
}

impl ImageMaker for Painter {
    fn made_with(&self) -> &str {
        "Fake Image"
    }

    fn make<'a>(
        &'a self,
        _prompt: &'a str,
        size: ImageSize,
        _n: u8,
    ) -> BoxFuture<'a, Result<Vec<MadeImage>, ImageError>> {
        self.asked.lock().unwrap().push(size);
        let answer = (self.answers.lock().unwrap().pop_front())
            .unwrap_or_else(|| Err(ImageError::new(ImageErrorKind::Other, "no more", "no more")));
        Box::pin(async move { answer })
    }
}

/// A dark square on white, as a sticker comes back.
fn drawn() -> Result<Vec<MadeImage>, ImageError> {
    let mut picture = image::RgbImage::from_pixel(64, 64, image::Rgb([255, 255, 255]));
    for x in 16..48 {
        for y in 16..48 {
            picture.put_pixel(x, y, image::Rgb([20, 20, 20]));
        }
    }
    let mut png = Vec::new();
    picture
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    Ok(vec![MadeImage {
        png,
        revised_prompt: None,
    }])
}

/// Where the fake job puts its sticker: at a fixed time, or nowhere once its moment is cut.
struct Spot(Option<(Us, Us)>);

impl StickerPlacement for Spot {
    fn placement(&self, _project: &Project) -> Option<(Us, Us)> {
        self.0
    }

    fn ops(&self, project: &Project, asset: &Asset, start_us: Us, duration_us: Us) -> Vec<Op> {
        let mut ops = Vec::new();
        if project.asset(&asset.id).is_none() {
            ops.push(Op::AddAssets {
                assets: vec![asset.clone()],
            });
        }
        ops.push(Op::AddItem {
            track: klipolo_model::ops::TrackTarget::Auto,
            attach: false,
            item: klipolo_model::timeline::Item {
                id: "sticker".into(),
                start_us,
                duration_us,
                attach: None,
                layout: klipolo_model::layout::Layout::full_frame(),
                content: klipolo_model::timeline::ItemContent::Image {
                    asset_id: asset.id.clone(),
                },
                effects: Vec::new(),
            },
        });
        ops
    }
}

struct Studio {
    chat: Chat,
    painter: Arc<Painter>,
    _folder: tempfile::TempDir,
}

/// A chat on `project`, saved in a folder of its own, whose pictures `painter` draws.
fn studio(project: Project, painter: Arc<Painter>) -> Studio {
    studio_with(project, painter, vec![])
}

/// `studio`, the model answering from `answers`.
fn studio_with(
    project: Project,
    painter: Arc<Painter>,
    answers: Vec<Result<ModelResponse, ModelError>>,
) -> Studio {
    let folder = tempfile::tempdir().unwrap();
    let model = Script::new(answers);
    let paying = Paying::key(model.clone());
    *paying.images.lock().unwrap() = Ok(painter.clone());
    let projects =
        Arc::new(MemoryProjects::open(project, folder.path().join("talk.klipolo")).unwrap());
    let session = Arc::new(ChatSession::new(projects.clone(), Found::done(), paying));
    Studio {
        chat: Chat {
            session,
            project: projects,
            model,
        },
        painter,
        _folder: folder,
    }
}

fn sticker(spot: Option<(Us, Us)>) -> PictureAsk {
    PictureAsk::Sticker {
        prompt: "paper plane".into(),
        placement: Arc::new(Spot(spot)),
    }
}

/// "talk.mov" with a title layer ("title", 2 to 5 s) on an overlay track: a picture stands in for the title template.
fn titled() -> Project {
    let mut p = talk();
    p.assets.push(Asset {
        id: "lettering".into(),
        kind: AssetKind::Image,
        path: "/media/words.png".into(),
        name: "words.png".into(),
        duration_us: secs(5.0),
        width: 1920,
        height: 1080,
        fps: 0.0,
        rotation: None,
        has_audio: false,
        audio_lead_us: None,
    });
    let mut track =
        klipolo_model::timeline::Track::new(klipolo_model::timeline::TrackKind::Overlay, "Words");
    track.id = "words".into();
    track.items.push(klipolo_model::timeline::Item {
        id: "title".into(),
        start_us: secs(2.0),
        duration_us: secs(3.0),
        attach: None,
        layout: klipolo_model::layout::Layout::full_frame(),
        content: klipolo_model::timeline::ItemContent::Image {
            asset_id: "lettering".into(),
        },
        effects: Vec::new(),
    });
    p.timeline.tracks.push(track);
    p
}

fn sentence(error: CommitError) -> String {
    match error {
        CommitError::Refused(klipolo_model::coded::Said::Text(text)) => text,
        other => panic!("a sentence, not {other:?}"),
    }
}

#[tokio::test]
async fn a_sticker_is_drawn_then_placed_as_one_undo_step() {
    let s = studio(talk(), Painter::new(vec![drawn()]));
    let card = s
        .chat
        .session
        .ask_picture(sticker(Some((secs(2.0), secs(3.0)))));
    assert_eq!(card.kind, ChatPictureKind::Sticker);
    assert_eq!(
        s.chat.project.revision(),
        0,
        "nothing is an op until it is drawn"
    );

    let picture = s.chat.session.draw_picture(&card.id).await.unwrap();
    assert!(std::path::Path::new(&picture.path).is_file());
    assert_eq!(picture.made_with, "Fake Image");
    assert_eq!(*s.painter.asked.lock().unwrap(), [ImageSize::Square]);
    let spend = s.chat.session.spend();
    assert_eq!((spend.pictures, spend.pictures_usd), (1, 0.01));
    assert_eq!(s.chat.project.revision(), 0, "drawn, not placed");

    let placed = s.chat.session.place_picture(&card.id).unwrap();
    assert_eq!(placed.undo.code, "chat.makeSticker");
    assert_eq!(s.chat.project.undo_steps(), 1, "one undo step");
    let p = s.chat.project.project();
    let item = (p.timeline.tracks.iter().flat_map(|t| &t.items))
        .find(|i| i.id == "sticker")
        .expect("the sticker is placed");
    assert_eq!((item.start_us, item.duration_us), (secs(2.0), secs(3.0)));
    assert!(p.assets.iter().any(|a| a.path == picture.path));
    // Used once: it is gone from the chat.
    let again = s.chat.session.place_picture(&card.id).unwrap_err();
    assert_eq!(sentence(again), PICTURE_GONE);
}

#[tokio::test]
async fn a_sticker_whose_moment_was_cut_while_drawing_is_not_placed() {
    let s = studio(talk(), Painter::new(vec![drawn()]));
    let card = s.chat.session.ask_picture(sticker(None));
    s.chat.session.draw_picture(&card.id).await.unwrap();
    let refused = s.chat.session.place_picture(&card.id).unwrap_err();
    assert_eq!(sentence(refused), MOMENT_CUT);
    assert_eq!(s.chat.project.revision(), 0, "nothing placed");
}

#[tokio::test]
async fn a_title_background_is_drawn_in_the_format_then_and_goes_under_its_title() {
    let s = studio(titled(), Painter::new(vec![drawn()]));
    let card = s.chat.session.ask_picture(PictureAsk::TitleBackground {
        prompt: "a calm sea".into(),
        title_id: "title".into(),
    });
    assert_eq!(card.kind, ChatPictureKind::TitleBackground);
    // The format changes before the drawing starts: the background is drawn in the one the video has then.
    let portrait = Transaction {
        label: "Portrait".into(),
        ops: vec![Op::SetSettings {
            settings: klipolo_model::project::Settings {
                formats: vec![FormatKind::Landscape, FormatKind::Portrait],
                primary_format: FormatKind::Portrait,
                ..talk().settings
            },
        }],
        ..Default::default()
    };
    s.chat.project.hand_edit(&portrait).unwrap();
    s.chat.session.draw_picture(&card.id).await.unwrap();
    assert_eq!(*s.painter.asked.lock().unwrap(), [ImageSize::Portrait]);

    s.chat.session.place_picture(&card.id).unwrap();
    let p = s.chat.project.project();
    let tracks: Vec<&str> = p.timeline.tracks.iter().map(|t| t.id.as_str()).collect();
    let title_track = tracks.iter().position(|t| *t == "words").unwrap();
    assert_eq!(
        title_track, 1,
        "a track was put under the title's: {tracks:?}"
    );
    let under = &p.timeline.tracks[0].items;
    assert_eq!(under.len(), 1);
    assert_eq!(
        (under[0].start_us, under[0].duration_us),
        (secs(2.0), secs(3.0))
    );
    assert_eq!(
        s.chat.project.undo_steps(),
        2,
        "the format, then the background"
    );
}

#[tokio::test]
async fn a_title_background_whose_title_is_gone_is_dropped() {
    let s = studio(titled(), Painter::new(vec![drawn()]));
    let card = s.chat.session.ask_picture(PictureAsk::TitleBackground {
        prompt: "a calm sea".into(),
        title_id: "title".into(),
    });
    let remove = Transaction {
        label: "Delete".into(),
        ops: vec![Op::RemoveItem { id: "title".into() }],
        ..Default::default()
    };
    s.chat.project.hand_edit(&remove).unwrap();
    let refused = s.chat.session.draw_picture(&card.id).await.unwrap_err();
    assert_eq!(sentence(refused), TITLE_GONE);
    assert!(
        s.painter.asked.lock().unwrap().is_empty(),
        "nothing was drawn"
    );
    assert_eq!(
        sentence(s.chat.session.place_picture(&card.id).unwrap_err()),
        PICTURE_GONE
    );
}

#[tokio::test]
async fn a_failed_drawing_says_why_and_try_again_draws_again() {
    let busy = ImageError::new(
        ImageErrorKind::Busy,
        "Muse Image is busy right now, so try again in a moment.",
        "429",
    );
    let s = studio(talk(), Painter::new(vec![Err(busy), drawn()]));
    let card = s
        .chat
        .session
        .ask_picture(sticker(Some((secs(2.0), secs(3.0)))));
    let failed = s.chat.session.draw_picture(&card.id).await.unwrap_err();
    assert_eq!(
        sentence(failed),
        "Muse Image is busy right now, so try again in a moment."
    );
    assert_eq!(s.chat.session.spend().pictures, 0);
    assert_eq!(s.chat.project.revision(), 0);
    // Try again.
    s.chat.session.draw_picture(&card.id).await.unwrap();
    s.chat.session.place_picture(&card.id).unwrap();
    assert_eq!(s.chat.project.undo_steps(), 1);
}

#[tokio::test]
async fn pictures_go_with_their_project() {
    let s = studio(talk(), Painter::new(vec![drawn()]));
    let card = s
        .chat
        .session
        .ask_picture(sticker(Some((secs(2.0), secs(3.0)))));
    s.chat.project.switch(Some(talk())).unwrap();
    let refused = s.chat.session.draw_picture(&card.id).await.unwrap_err();
    assert_eq!(sentence(refused), PICTURE_GONE);
    assert!(s.painter.asked.lock().unwrap().is_empty());
}

// ---------- Pictures the model asks for, through the compiler's jobs ----------

fn at_playhead(text: &str, at: f64) -> ChatRequest {
    ChatRequest {
        playhead_us: secs(at),
        ..request(text)
    }
}

fn sticker_at_12() -> Result<ModelResponse, ModelError> {
    answer(
        "Drawing a paper plane.",
        vec![fc(
            "s",
            "makeSticker",
            r#"{"prompt": "paper plane", "position": null, "atSec": 12.0, "durationSec": 3.0}"#,
        )],
    )
}

fn cut(from: f64, to: f64) -> Transaction {
    Transaction {
        label: "Cut".into(),
        ops: vec![Op::DeleteRanges {
            ranges: vec![RangeUs {
                start: secs(from),
                end: secs(to),
            }],
        }],
        ..Default::default()
    }
}

/// The image layers on the overlay tracks: (track index, start, length).
fn pictures_on(p: &Project) -> Vec<(usize, Us, Us)> {
    (p.timeline.tracks.iter().enumerate())
        .flat_map(|(t, track)| track.items.iter().map(move |i| (t, i)))
        .filter(|(_, i)| {
            matches!(
                i.content,
                klipolo_model::timeline::ItemContent::Image { .. }
            )
        })
        .map(|(t, i)| (t, i.start_us, i.duration_us))
        .collect()
}

#[tokio::test]
async fn a_sticker_the_model_asks_for_follows_its_footage_while_it_is_drawn() {
    let s = studio_with(talk(), Painter::new(vec![drawn()]), vec![sticker_at_12()]);
    let out = s
        .chat
        .session
        .send(at_playhead("add a paper plane sticker here", 12.0))
        .await
        .unwrap();
    let r = reply(&out);
    assert_eq!(r["pictures"][0]["kind"], "sticker");
    assert_eq!(r["pictures"][0]["prompt"], "paper plane");
    assert_eq!(
        s.chat.project.revision(),
        0,
        "a picture is no op until it exists"
    );
    let id = r["pictures"][0]["id"].as_str().unwrap().to_owned();

    // Four seconds before it are cut while it is drawn: it shows over the same footage, now at 8 s.
    s.chat.project.hand_edit(&cut(0.0, 4.0)).unwrap();
    s.chat.session.draw_picture(&id).await.unwrap();
    let placed = s.chat.session.place_picture(&id).unwrap();
    assert_eq!(placed.undo.code, "chat.makeSticker");
    assert_eq!(
        pictures_on(&s.chat.project.project()),
        [(0, secs(8.0), secs(3.0))]
    );
    assert_eq!(s.chat.project.undo_steps(), 2, "the cut, then the sticker");
}

#[tokio::test]
async fn a_sticker_whose_footage_was_cut_while_it_was_drawn_is_left_out() {
    let s = studio_with(talk(), Painter::new(vec![drawn()]), vec![sticker_at_12()]);
    let out = s
        .chat
        .session
        .send(at_playhead("add a paper plane sticker here", 12.0))
        .await
        .unwrap();
    let id = reply(&out)["pictures"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    s.chat.project.hand_edit(&cut(10.0, 16.0)).unwrap();
    s.chat.session.draw_picture(&id).await.unwrap();
    assert_eq!(
        sentence(s.chat.session.place_picture(&id).unwrap_err()),
        MOMENT_CUT
    );
    assert!(pictures_on(&s.chat.project.project()).is_empty());
    assert_eq!(s.chat.project.undo_steps(), 1, "only the cut");
}

#[tokio::test]
async fn a_title_card_s_background_goes_under_the_title_the_answer_added() {
    let title = answer(
        "",
        vec![fc(
            "t",
            "makeTitleCard",
            r#"{"title": "Day one", "prompt": "a calm sea", "atSec": 0.0, "durationSec": null}"#,
        )],
    );
    let s = studio_with(talk(), Painter::new(vec![drawn()]), vec![title]);
    let out = s
        .chat
        .session
        .send(request("a title card that says Day one over a calm sea"))
        .await
        .unwrap();
    let r = reply(&out);
    assert!(r["applied"].is_object(), "the title is in at once: {r:#}");
    assert_eq!(r["pictures"][0]["kind"], "titleBackground");
    let id = r["pictures"][0]["id"].as_str().unwrap().to_owned();
    let p = s.chat.project.project();
    let (title_track, title) = (p.timeline.tracks.iter().enumerate())
        .find_map(|(t, track)| {
            track
                .items
                .first()
                .map(|i| (t, (i.start_us, i.duration_us)))
        })
        .expect("the title layer");

    s.chat.session.draw_picture(&id).await.unwrap();
    assert_eq!(*s.painter.asked.lock().unwrap(), [ImageSize::Landscape]);
    s.chat.session.place_picture(&id).unwrap();
    let p = s.chat.project.project();
    assert_eq!(
        pictures_on(&p),
        [(title_track, title.0, title.1)],
        "on a track under the title's, for as long as it shows"
    );
    assert!(
        !p.timeline.tracks[title_track + 1].items.is_empty(),
        "the title is above it"
    );
    assert_eq!(
        s.chat.project.undo_steps(),
        2,
        "the title, then its background"
    );
}

#[tokio::test]
async fn the_projects_own_pictures_are_in_the_library_the_model_reads() {
    let mut p = talk();
    p.assets.push(Asset {
        id: "logo".into(),
        kind: AssetKind::Image,
        path: "/media/logo.png".into(),
        name: "logo.png".into(),
        duration_us: secs(5.0),
        width: 512,
        height: 512,
        fps: 0.0,
        rotation: None,
        has_audio: false,
        audio_lead_us: None,
    });
    let logo = answer(
        "",
        vec![fc(
            "i",
            "addImage",
            r#"{"file": "logo", "size": null, "position": "top-right", "atSec": 0.0, "durationSec": null}"#,
        )],
    );
    let s = studio_with(p, Painter::new(vec![]), vec![logo]);
    let out = s
        .chat
        .session
        .send(request("put my logo in the top right"))
        .await
        .unwrap();
    let context = match &s.chat.model.seen()[0].input[0] {
        Item::Context { text } => text.clone(),
        _ => unreachable!(),
    };
    assert!(context.contains("logo.png"), "{context}");
    assert!(reply(&out)["applied"].is_object(), "{:#}", reply(&out));
    assert_eq!(pictures_on(&s.chat.project.project()).len(), 1);
}

/// The project, refusing the next commit as it is (a full disk, say) when told to.
struct Refusing {
    project: Arc<MemoryProjects>,
    refuse_next: std::sync::atomic::AtomicBool,
}

impl Projects for Refusing {
    fn now(&self) -> Option<klipolo_app::chat::ProjectNow> {
        self.project.now()
    }

    fn commit(
        &self,
        at: &klipolo_app::chat::At,
        tx: &Transaction,
        next: Option<Project>,
    ) -> Result<klipolo_app::chat::Committed, CommitError> {
        if self.refuse_next.swap(false, Ordering::SeqCst) {
            return Err(CommitError::Refused(klipolo_model::coded::Said::Text(
                "Your disk is full.".into(),
            )));
        }
        self.project.commit(at, tx, next)
    }

    fn undo(
        &self,
        at: &klipolo_app::chat::At,
    ) -> Result<klipolo_app::chat::Committed, CommitError> {
        self.project.undo(at)
    }
}

#[tokio::test]
async fn an_apply_that_fails_for_another_reason_keeps_the_proposal_for_retry() {
    let model = Script::new(vec![answer("", vec![fillers()])]);
    let project =
        Arc::new(MemoryProjects::open(talk(), PathBuf::from("/tmp/talk.klipolo")).unwrap());
    let refusing = Arc::new(Refusing {
        project: project.clone(),
        refuse_next: std::sync::atomic::AtomicBool::new(false),
    });
    let session = ChatSession::new(refusing.clone(), Found::done(), Paying::key(model));
    let id = proposal_id(&session.send(request("cut the ums")).await.unwrap());

    refusing.refuse_next.store(true, Ordering::SeqCst);
    let failed = session.apply(&id).unwrap_err();
    assert!(matches!(failed, CommitError::Refused(_)), "{failed:?}");
    assert_eq!(project.revision(), 0, "nothing changed");
    assert!(
        !session.turns()[0].note.contains("dropped"),
        "{}",
        session.turns()[0].note
    );

    // Retry: the proposal is still open, and applies as one undo step.
    session.apply(&id).unwrap();
    assert_eq!(project.undo_steps(), 1);
    assert!(!plays(&project.project(), 0.6));
    assert!(
        session.turns()[0]
            .note
            .ends_with("The person applied the proposal.")
    );
}

fn find_intro() -> FunctionCall {
    fc(
        "m",
        "findMoments",
        r#"{"query": "the intro", "ranges": [{"startSec": 0.5, "endSec": 4.0, "label": "intro"}]}"#,
    )
}

#[tokio::test]
async fn a_reply_names_the_revision_its_moments_are_on() {
    let mut calls = vertical_karaoke();
    calls.push(find_intro());
    let chat = chat(vec![
        answer("Here it is.", vec![find_intro()]),
        answer("", calls),
    ]);
    let r = reply(&chat.session.send(request("find the intro")).await.unwrap());
    assert_eq!(r["revision"], 0);
    assert_eq!(r["moments"].as_array().unwrap().len(), 1);
    // With additions, the moments are on the revision they leave.
    let r = reply(
        &chat
            .session
            .send(request("vertical, karaoke, and find the intro"))
            .await
            .unwrap(),
    );
    assert_eq!(r["revision"], 1);
    assert_eq!(r["moments"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn moments_found_while_the_project_changed_are_left_out() {
    let model = Script::new(vec![answer(
        "The intro is at the start.",
        vec![find_intro()],
    )]);
    let gate = model.hold();
    let chat = chat_with(model.clone(), Found::done(), Paying::key(model.clone()));
    let sent = tokio::spawn({
        let session = chat.session.clone();
        async move { session.send(request("find the intro")).await.unwrap() }
    });
    model.arrived.notified().await;
    chat.project.hand_edit(&rename("Mine")).unwrap();
    gate.notify_one();
    let r = reply(&sent.await.unwrap());
    assert_eq!(r["moments"], serde_json::json!([]), "{r:#}");
    assert_eq!(r["text"], MOVED);
    assert_eq!(r["revision"], 0);
    // "Keep only these" echoing that revision is refused: the moments would be stale.
    let keep = ChatRequest {
        keep_only: vec![RangeUs {
            start: secs(0.5),
            end: secs(4.0),
        }],
        base_revision: r["revision"].as_u64().unwrap(),
        ..request("Keep only these")
    };
    assert_eq!(
        chat.session.send(keep).await.unwrap_err(),
        CommitError::Moved
    );
    assert_eq!(chat.project.revision(), 1, "only the hand edit");
}

#[tokio::test]
async fn a_picture_paid_for_counts_even_when_it_cannot_be_read() {
    let garbled = Ok(vec![MadeImage {
        png: b"not a picture at all".to_vec(),
        revised_prompt: None,
    }]);
    let s = studio(talk(), Painter::new(vec![garbled, Ok(Vec::new())]));
    let card = s
        .chat
        .session
        .ask_picture(sticker(Some((secs(2.0), secs(3.0)))));
    let failed = s.chat.session.draw_picture(&card.id).await.unwrap_err();
    assert_eq!(
        sentence(failed),
        "Fake Image sent a picture Klipolo couldn't read, so try again."
    );
    let spend = s.chat.session.spend();
    assert_eq!(
        (spend.pictures, spend.pictures_usd),
        (1, 0.01),
        "Muse Image billed it"
    );
    // An answer with no picture in it isn't billed.
    let failed = s.chat.session.draw_picture(&card.id).await.unwrap_err();
    assert_eq!(
        sentence(failed),
        "Fake Image didn't send a picture back, so try again."
    );
    assert_eq!(s.chat.session.spend().pictures, 1);
    assert_eq!(s.chat.project.revision(), 0);
}
