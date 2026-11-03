//! Chat in the app: a message's answer applied, proposed and remembered against the open project. The model is a fake
//! that answers from a script (and can be held mid-answer), the analysis a fake with "talk.mov"'s words, and the
//! project a `MemoryProjects` with its own history, so every undo step and revision is the real `Document`'s.
//!
//! "talk.mov" is 30 s, played whole; its words run from 0.5 to 7.8 s ("Um, so today we look at the new module. It took
//! [pause 1.5 s] uh a long time."), with two fillers: "Um," at 0.5 and "uh" at 6.3.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use klipolo_analysis::providers::images::{
    ImageError, ImageErrorKind, ImageMaker, ImageSize, MadeImage,
};
use klipolo_app::analysis::AssetAnalysis;
use klipolo_app::chat::pictures::{
    ChatPictureKind, DRAWING_GONE, MOMENT_CUT, PICTURE_GONE, PictureAsk, StickerPlacement,
    TITLE_GONE,
};
use klipolo_app::chat::session::{
    KEEP_ONLY_READY, MOMENTS_MOVED, MOMENTS_UNDONE, MOVED, NEEDS_KEY, NOT_CHATS, OTHER_PROJECT,
};
use klipolo_app::chat::{
    BLUR_COMING, ChatOutcome, ChatRequest, ChatSession, CommitError, FOLLOW_COMING, Findings,
    MemoryProjects, Models, Projects, Readiness,
};
use klipolo_chat::{
    AnalysisState, BoxFuture, FunctionCall, GO_ON, Item, LanguageModel, LibraryItem, MediaKind,
    ModelError, ModelRequest, ModelResponse,
};
use klipolo_model::media::music::{MUSIC_FADE_OUT_US, is_music};
use klipolo_model::ops::{Op, Origin};
use klipolo_model::project::{Asset, AssetKind, FormatKind};
use klipolo_model::query::video_duration;
use klipolo_model::speech::{Levels, Transcript, Word, WordKind};
use klipolo_model::time::{RangeUs, Us};
use klipolo_model::timeline::{Clip, ClipSource, ItemContent};
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
        made_with: None,
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

/// What the analysis found: "talk.mov"'s words once they are in (done, or the scene log still being made), or nothing
/// yet (`state`). Its words are one transcript until they are timed again (`retime`), as the analysis keeps them.
/// Other files have words once a test gives them (`hear`).
struct Found {
    state: Mutex<AnalysisState>,
    analysis: Mutex<Arc<AssetAnalysis>>,
    levels: HashMap<String, Arc<Levels>>,
    /// The preview's header offers Time words again for "talk.mov"'s rough words (`rough`).
    time_again: AtomicBool,
    /// Other files' words, by asset id.
    others: Mutex<HashMap<String, Arc<AssetAnalysis>>>,
}

impl Found {
    fn at(state: AnalysisState) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(state),
            analysis: Mutex::new(Arc::new(AssetAnalysis {
                transcript: Some(transcript()),
                ..AssetAnalysis::default()
            })),
            levels: HashMap::new(),
            time_again: AtomicBool::new(false),
            others: Mutex::new(HashMap::new()),
        })
    }

    fn done() -> Arc<Self> {
        Self::at(AnalysisState::Done)
    }

    fn not_started() -> Arc<Self> {
        Self::at(AnalysisState::NotStarted)
    }

    /// Done, with "talk.mov"'s loudness: quiet from 10.0 to 10.8 s and from 15.0 to 17.0 s.
    fn with_pauses() -> Arc<Self> {
        let db: Vec<f32> = (0..300)
            .map(|w| {
                if (100..108).contains(&w) || (150..170).contains(&w) {
                    -70.0
                } else {
                    -20.0
                }
            })
            .collect();
        let mut found = Arc::into_inner(Self::done()).unwrap();
        found
            .levels
            .insert("talk".into(), Arc::new(Levels { db: db.into() }));
        Arc::new(found)
    }

    /// The words timed again: the same words, a new transcript.
    fn retime(&self) {
        *self.analysis.lock().unwrap() = Arc::new(AssetAnalysis {
            transcript: Some(transcript()),
            ..AssetAnalysis::default()
        });
    }

    /// Word timing failed for "uh": it was spread over its turn (confidence 0), so its time is a guess. `offered`: the
    /// preview's header offers Time words again for the clip (timing again can help).
    fn rough(&self, offered: bool) {
        let mut words = Arc::unwrap_or_clone(transcript());
        for w in words.words.iter_mut().filter(|w| w.text == "uh") {
            w.confidence = 0.0;
        }
        *self.analysis.lock().unwrap() = Arc::new(AssetAnalysis {
            transcript: Some(Arc::new(words)),
            approximate: true,
            ..AssetAnalysis::default()
        });
        self.time_again.store(offered, Ordering::SeqCst);
    }

    /// File `asset_id`'s words come in, or come in again (heard, then timed): a new transcript each time.
    fn hear(&self, asset_id: &str) {
        self.others.lock().unwrap().insert(
            asset_id.into(),
            Arc::new(AssetAnalysis {
                transcript: Some(transcript()),
                ..AssetAnalysis::default()
            }),
        );
    }
}

impl Findings for Found {
    fn found(&self, asset_id: &str) -> Option<Arc<AssetAnalysis>> {
        let state = *self.state.lock().unwrap();
        if !matches!(state, AnalysisState::Done | AnalysisState::Watching) {
            return None;
        }
        if asset_id == "talk" {
            return Some(self.analysis.lock().unwrap().clone());
        }
        self.others.lock().unwrap().get(asset_id).cloned()
    }

    fn state(&self, _asset_id: &str) -> AnalysisState {
        *self.state.lock().unwrap()
    }

    fn levels(&self) -> HashMap<String, Arc<Levels>> {
        self.levels.clone()
    }

    fn time_again(&self, asset_id: &str) -> bool {
        asset_id == "talk" && self.time_again.load(Ordering::SeqCst)
    }
}

/// A model that answers each message's first request from a script, in order, keeps those requests (`seen`), and can be
/// held before it answers one. A request after it, with what came of its calls, is answered at once as a model that is
/// done: no words, no call, nothing spent; so is a first request past the script.
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
            let opening =
                matches!(request.input.last(), Some(Item::User { text }) if text != GO_ON);
            if opening {
                self.seen.lock().unwrap().push(request.clone());
                self.arrived.notify_one();
                let gate = self.hold.lock().unwrap().clone();
                if let Some(gate) = gate {
                    gate.notified().await;
                }
            }
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            let scripted = if opening {
                self.answers.lock().unwrap().pop_front()
            } else {
                None
            };
            scripted.unwrap_or_else(|| {
                Ok(ModelResponse {
                    text: String::new(),
                    calls: Vec::new(),
                    usage_usd: 0.0,
                })
            })
        })
    }
}

/// How the fake credentials answer: the model behind them, through the relay or not, and how many renewals.
struct Paying {
    readiness: Mutex<Readiness>,
    /// Through the relay, where a refused token can be renewed.
    relay: bool,
    /// The picture maker, or the sentence that says why there is none.
    images: Mutex<Result<Arc<dyn ImageMaker>, String>>,
    recovered: AtomicUsize,
    /// Whether registering again works.
    renews: std::sync::atomic::AtomicBool,
}

impl Paying {
    fn key(model: Arc<Script>) -> Arc<Self> {
        Self::new(Readiness::Ready { model }, false)
    }

    fn relay(model: Arc<Script>) -> Arc<Self> {
        Self::new(Readiness::Ready { model }, true)
    }

    fn with(readiness: Readiness) -> Arc<Self> {
        Self::new(readiness, false)
    }

    fn new(readiness: Readiness, relay: bool) -> Arc<Self> {
        Arc::new(Self {
            readiness: Mutex::new(readiness),
            relay,
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
        if !self.relay {
            return Box::pin(async { false });
        }
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
    chat_on(talk(), model, found, paying)
}

/// A chat on `project`.
fn chat_on(project: Project, model: Arc<Script>, found: Arc<Found>, paying: Arc<Paying>) -> Chat {
    let project =
        Arc::new(MemoryProjects::open(project, PathBuf::from("/tmp/talk.klipolo")).unwrap());
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
    // A step of several changes lists each, in the order asked, for the reply to show.
    let changes: Vec<&str> = (r["applied"]["changes"].as_array().expect("each change"))
        .iter()
        .map(|c| c["code"].as_str().unwrap())
        .collect();
    assert_eq!(changes, ["chat.setFormat", "chat.setCaptions"]);
    assert!(r["proposal"].is_null());
    assert_eq!(r["analyze"], false, "every clip is analyzed");
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
    assert_eq!(r["proposal"]["title"], "Cut 2 filler words");
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
        (ModelError::Busy(None), "busy", klipolo_chat::planner::BUSY),
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

    assert_eq!(chat.session.spend().messages, 1, "one message, sent twice");

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
    assert_eq!(r["text"], "Analyze this clip (talk.mov) first.");
    assert!(r["proposal"].is_null());
    // The panel offers Analyze under it, with its price before anything is spent.
    assert_eq!(r["analyze"], true);
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

/// `reverseClip` changes nothing yet: the reply names the clip for the page to write backward, as the clip menu's
/// Reverse does, and says the copy is being made.
#[tokio::test]
async fn a_clip_to_reverse_is_named_for_the_page() {
    let chat = chat(vec![answer(
        "Reversing it.",
        vec![fc(
            "r",
            "reverseClip",
            r#"{"clip": "c1", "backward": true}"#,
        )],
    )]);
    let r = reply(
        &chat
            .session
            .send(request("play it backward"))
            .await
            .unwrap(),
    );
    assert_eq!(r["reverse"], serde_json::json!(["t"]));
    assert_eq!(r["text"], "Reversing it. The reversed copy is being made.");
    assert!(r["applied"].is_null());
    assert_eq!(chat.project.revision(), 0);
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
    assert_eq!(r["proposal"]["title"], "Keep only the parts you chose");
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
    assert_eq!(r["proposal"]["title"], "Cut 1 part");
    assert_eq!(r["proposal"]["removedUs"], secs(6.0));
}

#[tokio::test]
async fn undo_last_takes_back_chats_change_only_while_it_is_the_last() {
    let chat = chat(vec![
        answer("", vertical_karaoke()),
        answer("", vertical_karaoke()),
    ]);
    let out = chat.session.send(request("vertical")).await.unwrap();
    let chip = reply(&out)["applied"]["change"]["history"]["revision"]
        .as_u64()
        .unwrap();
    assert_eq!(chip, 1);
    chat.session.undo_last(chip).unwrap();
    assert_eq!(
        chat.project.project().settings.primary_format,
        FormatKind::Landscape
    );
    assert_eq!(
        chat.session.undo_last(chip).unwrap_err(),
        CommitError::Moved,
        "once"
    );

    chat.project.step(true).unwrap();
    chat.project.step(false).unwrap();
    let out = chat.session.send(request("vertical again")).await.unwrap();
    let chip = reply(&out)["applied"]["change"]["history"]["revision"]
        .as_u64()
        .unwrap();
    chat.project.hand_edit(&rename("Mine")).unwrap();
    assert_eq!(
        chat.session.undo_last(chip).unwrap_err(),
        CommitError::Moved
    );
    assert_eq!(chat.project.project().name, "Mine");
}

#[tokio::test]
async fn an_undo_chip_takes_back_only_the_change_it_is_under() {
    let chat = chat(vec![
        answer("", vertical_karaoke()),
        answer("", vec![fillers()]),
    ]);
    let out = chat.session.send(request("vertical")).await.unwrap();
    let additions = reply(&out)["applied"]["change"]["history"]["revision"]
        .as_u64()
        .unwrap();
    let out = chat.session.send(request("cut the ums")).await.unwrap();
    let applied = chat.session.apply(&proposal_id(&out)).unwrap();
    // The additions' chip, after Apply: the top step is the cut, so it is refused and the cut stays.
    assert_eq!(
        chat.session.undo_last(additions).unwrap_err(),
        CommitError::Moved
    );
    assert!(!plays(&chat.project.project(), 0.6), "the cut stays");
    assert_eq!(chat.project.undo_steps(), 2);
    // The cut's own chip takes it back.
    chat.session
        .undo_last(applied.change.history.revision)
        .unwrap();
    assert!(plays(&chat.project.project(), 0.6));
    assert_eq!(
        chat.project.project().settings.primary_format,
        FormatKind::Portrait
    );
}

#[tokio::test]
async fn new_chat_forgets_the_last_change_too() {
    let chat = chat(vec![answer("", vertical_karaoke())]);
    let out = chat.session.send(request("vertical")).await.unwrap();
    let chip = reply(&out)["applied"]["change"]["history"]["revision"]
        .as_u64()
        .unwrap();
    chat.session.reset();
    assert_eq!(
        chat.session.undo_last(chip).unwrap_err(),
        CommitError::Moved
    );
    assert_eq!(
        chat.project.project().settings.primary_format,
        FormatKind::Portrait
    );
}

#[tokio::test]
async fn the_models_undo_takes_back_chats_own_last_change() {
    let chat = chat(vec![
        answer("", vertical_karaoke()),
        answer("Undone.", vec![fc("u", "undo", "{}")]),
        answer("Undone.", vec![fc("u", "undo", "{}")]),
    ]);
    chat.project.hand_edit(&rename("Mine")).unwrap();
    chat.session.send(request("vertical")).await.unwrap();
    let r = reply(&chat.session.send(request("undo that")).await.unwrap());
    assert_eq!(r["undone"]["baseRevision"], 2);
    assert_eq!(
        chat.project.project().settings.primary_format,
        FormatKind::Landscape
    );
    // Once: the step under it is the person's own.
    let r = reply(&chat.session.send(request("undo again")).await.unwrap());
    assert_eq!(r["text"], NOT_CHATS);
    assert!(r["undone"].is_null(), "{r:#}");
    assert_eq!(chat.project.project().name, "Mine");
}

#[tokio::test]
async fn the_models_undo_leaves_a_step_that_isnt_chats() {
    let chat = chat(vec![
        answer("", vertical_karaoke()),
        answer("Undone.", vec![fc("u", "undo", "{}")]),
    ]);
    chat.session.send(request("vertical")).await.unwrap();
    // The person's own edit lands on top of chat's change.
    chat.project.hand_edit(&rename("Mine")).unwrap();
    let before = chat.project.revision();
    let r = reply(
        &chat
            .session
            .send(request("undo the captions"))
            .await
            .unwrap(),
    );
    // The person's edit and chat's change both stay, and the reply says why.
    assert_eq!(r["text"], NOT_CHATS);
    assert!(r["undone"].is_null(), "{r:#}");
    assert_eq!(chat.project.revision(), before);
    assert_eq!(chat.project.project().name, "Mine");
    assert_eq!(
        chat.project.project().settings.primary_format,
        FormatKind::Portrait
    );
    assert_eq!(chat.project.undo_steps(), 2);
    assert!(
        chat.session.turns()[1].note.contains("not chat's"),
        "{:?}",
        chat.session.turns()[1].note
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
    fn ops(&self, project: &Project, asset: &Asset) -> Option<Vec<Op>> {
        let (start_us, duration_us) = self.0?;
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
        Some(ops)
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
        place: "Goes in the middle, over the moment it was asked for.".into(),
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
        made_with: None,
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

    let placed = s
        .chat
        .session
        .place_picture(&card.id, &picture.path)
        .unwrap();
    assert_eq!(placed.undo.code, "chat.makeSticker");
    assert_eq!(s.chat.project.undo_steps(), 1, "one undo step");
    let p = s.chat.project.project();
    let item = (p.timeline.tracks.iter().flat_map(|t| &t.items))
        .find(|i| i.id == "sticker")
        .expect("the sticker is placed");
    assert_eq!((item.start_us, item.duration_us), (secs(2.0), secs(3.0)));
    assert!(p.assets.iter().any(|a| a.path == picture.path));
    // Used once: it is gone from the chat.
    let again = s
        .chat
        .session
        .place_picture(&card.id, &picture.path)
        .unwrap_err();
    assert_eq!(sentence(again), PICTURE_GONE);
}

#[tokio::test]
async fn a_sticker_whose_moment_was_cut_while_drawing_is_not_placed() {
    let s = studio(talk(), Painter::new(vec![drawn()]));
    let card = s.chat.session.ask_picture(sticker(None));
    let drawn = s.chat.session.draw_picture(&card.id).await.unwrap();
    let refused = s
        .chat
        .session
        .place_picture(&card.id, &drawn.path)
        .unwrap_err();
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
    let drawn = s.chat.session.draw_picture(&card.id).await.unwrap();
    assert_eq!(*s.painter.asked.lock().unwrap(), [ImageSize::Portrait]);

    s.chat.session.place_picture(&card.id, &drawn.path).unwrap();
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
        sentence(s.chat.session.place_picture(&card.id, "").unwrap_err()),
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
    let drawn = s.chat.session.draw_picture(&card.id).await.unwrap();
    s.chat.session.place_picture(&card.id, &drawn.path).unwrap();
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
        r["pictures"][0]["place"],
        "Goes in the middle, over the moment it was asked for."
    );
    assert_eq!(
        s.chat.project.revision(),
        0,
        "a picture is no op until it exists"
    );
    let id = r["pictures"][0]["id"].as_str().unwrap().to_owned();

    // Four seconds before it are cut while it is drawn: it shows over the same footage, now at 8 s.
    s.chat.project.hand_edit(&cut(0.0, 4.0)).unwrap();
    let drawn = s.chat.session.draw_picture(&id).await.unwrap();
    let placed = s.chat.session.place_picture(&id, &drawn.path).unwrap();
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
    let drawn = s.chat.session.draw_picture(&id).await.unwrap();
    assert_eq!(
        sentence(s.chat.session.place_picture(&id, &drawn.path).unwrap_err()),
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
    assert_eq!(
        r["pictures"][0]["place"],
        "Goes behind the title, for as long as it shows."
    );
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

    let drawn = s.chat.session.draw_picture(&id).await.unwrap();
    assert_eq!(*s.painter.asked.lock().unwrap(), [ImageSize::Landscape]);
    s.chat.session.place_picture(&id, &drawn.path).unwrap();
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
        made_with: None,
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
    /// Every transaction committed, in order.
    committed: Mutex<Vec<Transaction>>,
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
        let done = self.project.commit(at, tx, next)?;
        self.committed.lock().unwrap().push(tx.clone());
        Ok(done)
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
        committed: Mutex::new(Vec::new()),
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

#[tokio::test]
async fn every_picture_paid_for_stays_in_the_muse_image_folder() {
    let exists = |path: &str| std::path::Path::new(path).is_file();
    let s = studio(
        talk(),
        Painter::new(vec![drawn(), drawn(), drawn(), drawn()]),
    );
    // Try again keeps the first picture as well.
    let card = s.chat.session.ask_picture(sticker(None));
    let first = s.chat.session.draw_picture(&card.id).await.unwrap();
    let second = s.chat.session.draw_picture(&card.id).await.unwrap();
    assert!(
        exists(&first.path) && exists(&second.path),
        "both pictures are kept"
    );
    // Its moment cut: nothing placed, the pictures kept.
    s.chat
        .session
        .place_picture(&card.id, &second.path)
        .unwrap_err();
    // New chat and another project forget the cards, not the pictures.
    let card = s
        .chat
        .session
        .ask_picture(sticker(Some((secs(2.0), secs(3.0)))));
    let third = s.chat.session.draw_picture(&card.id).await.unwrap();
    s.chat.session.reset();
    let card = s
        .chat
        .session
        .ask_picture(sticker(Some((secs(2.0), secs(3.0)))));
    let fourth = s.chat.session.draw_picture(&card.id).await.unwrap();
    s.chat.project.switch(Some(talk())).unwrap();
    s.chat.session.project_changed();
    let paths: std::collections::HashSet<&str> = [&first, &second, &third, &fourth]
        .iter()
        .map(|p| p.path.as_str())
        .collect();
    assert_eq!(paths.len(), 4, "four files: {paths:?}");
    for picture in [&first, &second, &third, &fourth] {
        assert!(exists(&picture.path), "{} is kept", picture.path);
        assert!(
            picture.path.contains("Fake Image"),
            "in the maker's folder: {}",
            picture.path
        );
    }
    assert_eq!(s.chat.session.spend().pictures, 4);
}

#[tokio::test]
async fn a_proposal_a_later_message_moved_past_is_dropped_with_a_note() {
    let chat = chat(vec![
        answer("", vec![fillers()]),
        answer("", vertical_karaoke()),
    ]);
    let id = proposal_id(&chat.session.send(request("cut the ums")).await.unwrap());
    chat.session.send(request("vertical")).await.unwrap();
    assert_eq!(chat.session.apply(&id).unwrap_err(), CommitError::Moved);
    let note = &chat.session.turns()[0].note;
    assert!(
        note.ends_with(
            "The proposal was dropped: the project changed before the person applied it."
        ),
        "{note}"
    );
}

#[tokio::test]
async fn what_chat_commits_is_the_compilers_from_chat() {
    let mut calls = vertical_karaoke();
    calls.push(fillers());
    let model = Script::new(vec![answer("", calls)]);
    let project =
        Arc::new(MemoryProjects::open(talk(), PathBuf::from("/tmp/talk.klipolo")).unwrap());
    let watched = Arc::new(Refusing {
        project: project.clone(),
        refuse_next: std::sync::atomic::AtomicBool::new(false),
        committed: Mutex::new(Vec::new()),
    });
    let session = ChatSession::new(watched.clone(), Found::done(), Paying::key(model));
    let out = session
        .send(request("vertical, karaoke, and cut the ums"))
        .await
        .unwrap();
    session.apply(&proposal_id(&out)).unwrap();
    let committed = watched.committed.lock().unwrap().clone();
    let made: Vec<(&str, Option<&Origin>)> = (committed.iter())
        .map(|tx| (tx.label.as_str(), tx.origin.as_ref()))
        .collect();
    assert_eq!(
        made,
        [
            ("chat.andMore", Some(&Origin::Chat)),
            ("chat.removeFillers", Some(&Origin::Chat))
        ]
    );
    assert_eq!(project.undo_steps(), 2);
}

#[tokio::test]
async fn an_apply_racing_a_message_lands_and_the_message_does_not() {
    let model = Script::new(vec![
        answer("", vec![fillers()]),
        answer("Done.", vertical_karaoke()),
    ]);
    let chat = chat_with(model.clone(), Found::done(), Paying::key(model.clone()));
    let id = proposal_id(&chat.session.send(request("cut the ums")).await.unwrap());
    // The first message's arrival was told already: that word is taken, so the next one is the second message's.
    model.arrived.notified().await;
    let gate = model.hold();
    let sent = tokio::spawn({
        let session = chat.session.clone();
        async move { session.send(request("vertical")).await.unwrap() }
    });
    model.arrived.notified().await;
    // Apply while the message is out: the cut lands first.
    chat.session.apply(&id).unwrap();
    gate.notify_one();
    let r = reply(&sent.await.unwrap());
    assert_eq!(r["text"], MOVED, "made on the revision before the cut");
    assert!(r["applied"].is_null());
    assert_eq!(chat.project.undo_steps(), 1, "only the cut");
    assert!(!plays(&chat.project.project(), 0.6));
    assert_eq!(
        chat.project.project().settings.primary_format,
        FormatKind::Landscape
    );
}

#[tokio::test]
async fn a_project_switch_while_the_model_answers_leaves_the_new_project_alone() {
    let mut calls = vertical_karaoke();
    calls.push(fillers());
    calls.push(fc(
        "s",
        "makeSticker",
        r#"{"prompt": "paper plane", "position": null, "atSec": 12.0, "durationSec": 3.0}"#,
    ));
    for answered in [calls, vec![sticker_at_12().unwrap().calls[0].clone()]] {
        let model = Script::new(vec![answer("Done, and drawing a plane.", answered)]);
        let gate = model.hold();
        let chat = chat_with(model.clone(), Found::done(), Paying::key(model.clone()));
        let sent = tokio::spawn({
            let session = chat.session.clone();
            async move {
                session
                    .send(at_playhead("vertical, cut the ums, a plane here", 12.0))
                    .await
                    .unwrap()
            }
        });
        model.arrived.notified().await;
        chat.project.switch(Some(talk())).unwrap();
        gate.notify_one();
        let r = reply(&sent.await.unwrap());
        assert_eq!(r["text"], MOVED, "{r:#}");
        assert!(r["applied"].is_null() && r["proposal"].is_null());
        assert_eq!(
            r["pictures"],
            serde_json::json!([]),
            "no picture for a project that closed"
        );
        assert_eq!(chat.project.revision(), 0, "the new project is untouched");
        assert_eq!(chat.project.undo_steps(), 0);
    }
}

#[tokio::test]
async fn a_title_background_moves_with_its_attached_title() {
    let mut p = titled();
    // The title is attached to the footage under it, as layers placed by chat are.
    let title = p.timeline.tracks.pop().unwrap().items.pop().unwrap();
    let s = studio(p, Painter::new(vec![drawn()]));
    s.chat
        .project
        .hand_edit(&Transaction {
            label: "Title".into(),
            ops: vec![Op::AddItem {
                track: klipolo_model::ops::TrackTarget::Auto,
                attach: true,
                item: title,
            }],
            ..Default::default()
        })
        .unwrap();
    let attached = (s.chat.project.project().timeline.tracks.iter())
        .flat_map(|t| &t.items)
        .find(|i| i.id == "title")
        .and_then(|i| i.attach.clone());
    assert!(attached.is_some(), "the title is attached");
    let card = s.chat.session.ask_picture(PictureAsk::TitleBackground {
        prompt: "a calm sea".into(),
        title_id: "title".into(),
    });
    let drawn = s.chat.session.draw_picture(&card.id).await.unwrap();
    s.chat.session.place_picture(&card.id, &drawn.path).unwrap();
    let p = s.chat.project.project();
    let background = (p.timeline.tracks.iter())
        .flat_map(|t| &t.items)
        .find(|i| i.id != "title")
        .expect("the background");
    assert_eq!(background.attach, attached, "attached where the title is");
}

#[tokio::test]
async fn moments_found_beside_the_models_undo_are_left_out() {
    let chat = chat(vec![
        answer("", vec![fillers()]),
        answer(
            "Undone, and here is the intro.",
            vec![fc("u", "undo", "{}"), find_intro()],
        ),
    ]);
    let id = proposal_id(&chat.session.send(request("cut the ums")).await.unwrap());
    chat.session.apply(&id).unwrap();
    let r = reply(
        &chat
            .session
            .send(request("undo that and find the intro"))
            .await
            .unwrap(),
    );
    assert!(r["undone"].is_object(), "the cut is taken back");
    // Found on the timeline before the undo: their times would cut the wrong footage.
    assert_eq!(r["moments"], serde_json::json!([]), "{r:#}");
    assert_eq!(r["notes"], serde_json::json!([MOMENTS_UNDONE]));
    assert_eq!(r["revision"], 2);
    assert!(
        chat.session.turns()[1]
            .note
            .contains("The moments found were left out")
    );
}

#[tokio::test]
async fn moments_left_out_after_a_hand_edit_keep_the_question_and_say_so() {
    let ask = fc(
        "q",
        "askUser",
        r#"{"question": "Which intro, the first or the second?"}"#,
    );
    let model = Script::new(vec![answer("", vec![find_intro(), ask])]);
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
    assert_eq!(r["moments"], serde_json::json!([]));
    assert_eq!(r["notes"], serde_json::json!([MOMENTS_MOVED]));
    // The question is the reply (the model wrote nothing else), still asked.
    assert_eq!(r["text"], "Which intro, the first or the second?");
    let note = &chat.session.turns()[0].note;
    assert!(note.contains("The moments found were left out"), "{note}");
}

#[tokio::test]
async fn a_sticker_beside_moments_left_out_is_still_asked_for() {
    let calls = vec![find_intro(), sticker_at_12().unwrap().calls[0].clone()];
    let model = Script::new(vec![answer("Here, and drawing a plane.", calls)]);
    let gate = model.hold();
    let chat = chat_with(model.clone(), Found::done(), Paying::key(model.clone()));
    let sent = tokio::spawn({
        let session = chat.session.clone();
        async move {
            session
                .send(at_playhead("find the intro, and a plane here", 12.0))
                .await
                .unwrap()
        }
    });
    model.arrived.notified().await;
    chat.project.hand_edit(&rename("Mine")).unwrap();
    gate.notify_one();
    let r = reply(&sent.await.unwrap());
    assert_eq!(r["moments"], serde_json::json!([]));
    assert_eq!(r["pictures"][0]["kind"], "sticker", "{r:#}");
    assert_eq!(r["notes"], serde_json::json!([MOMENTS_MOVED]));
}

/// The project, another one opening right after a commit lands (before the reply is built).
struct SwitchingAfterCommit {
    project: Arc<MemoryProjects>,
}

impl Projects for SwitchingAfterCommit {
    fn now(&self) -> Option<klipolo_app::chat::ProjectNow> {
        self.project.now()
    }

    fn commit(
        &self,
        at: &klipolo_app::chat::At,
        tx: &Transaction,
        next: Option<Project>,
    ) -> Result<klipolo_app::chat::Committed, CommitError> {
        let done = self.project.commit(at, tx, next)?;
        self.project.switch(Some(talk())).unwrap();
        Ok(done)
    }

    fn undo(
        &self,
        at: &klipolo_app::chat::At,
    ) -> Result<klipolo_app::chat::Committed, CommitError> {
        self.project.undo(at)
    }
}

#[tokio::test]
async fn edits_that_landed_before_the_project_closed_are_still_said() {
    let model = Script::new(vec![answer("Done.", vertical_karaoke())]);
    let project =
        Arc::new(MemoryProjects::open(talk(), PathBuf::from("/tmp/talk.klipolo")).unwrap());
    let switching = Arc::new(SwitchingAfterCommit { project });
    let session = ChatSession::new(switching, Found::done(), Paying::key(model));
    let r = reply(&session.send(request("vertical")).await.unwrap());
    // The old project has them: the reply doesn't deny what it carries.
    assert_eq!(r["text"], "Done.");
    assert!(r["applied"].is_object());
}

#[test]
fn a_picture_that_cant_be_placed_says_what_to_do_next() {
    for said in [MOMENT_CUT, TITLE_GONE] {
        assert!(said.ends_with("Ask me for it again."), "{said}");
    }
}

#[tokio::test]
async fn use_places_the_drawing_the_card_shows_though_a_newer_one_landed() {
    let s = studio(talk(), Painter::new(vec![drawn(), drawn()]));
    let card = s
        .chat
        .session
        .ask_picture(sticker(Some((secs(2.0), secs(3.0)))));
    let first = s.chat.session.draw_picture(&card.id).await.unwrap();
    // Try again lands before Use goes: the card still showed the first.
    let second = s.chat.session.draw_picture(&card.id).await.unwrap();
    assert_ne!(first.path, second.path);
    s.chat.session.place_picture(&card.id, &first.path).unwrap();
    let p = s.chat.project.project();
    assert!(
        p.assets.iter().any(|a| a.path == first.path),
        "the one the card showed"
    );
    assert!(!p.assets.iter().any(|a| a.path == second.path));
}

#[tokio::test]
async fn use_of_a_drawing_that_isnt_there_says_so() {
    let s = studio(talk(), Painter::new(vec![drawn()]));
    let card = s
        .chat
        .session
        .ask_picture(sticker(Some((secs(2.0), secs(3.0)))));
    s.chat.session.draw_picture(&card.id).await.unwrap();
    let refused = s
        .chat
        .session
        .place_picture(&card.id, "/elsewhere/other.png")
        .unwrap_err();
    assert_eq!(sentence(refused), DRAWING_GONE);
    assert_eq!(s.chat.project.revision(), 0);
}

fn sound_file(id: &str, name: &str, seconds: f64) -> Asset {
    Asset {
        id: id.into(),
        kind: AssetKind::Audio,
        path: format!("/media/{name}"),
        name: name.into(),
        duration_us: secs(seconds),
        width: 0,
        height: 0,
        fps: 0.0,
        rotation: None,
        has_audio: true,
        audio_lead_us: None,
        made_with: None,
    }
}

#[tokio::test]
async fn chats_media_tools_find_the_projects_music_and_sounds_and_the_pack() {
    let mut p = talk();
    p.assets.push(sound_file("lofi", "lofi-beat.mp3", 60.0));
    p.assets.push(sound_file("whoosh", "whoosh.wav", 1.0));
    let soft = LibraryItem {
        name: "Soft ambient".into(),
        kind: MediaKind::Music,
        asset: sound_file("klipolo-pack-soft-ambient", "soft-ambient.wav", 30.0),
        pack: true,
        tags: Vec::new(),
    };
    let model = Script::new(vec![
        answer(
            "",
            vec![
                fc(
                    "m",
                    "addMusic",
                    r#"{"file": "lofi", "volume": null, "duck": null, "atSec": 0.0}"#,
                ),
                fc("s", "addSound", r#"{"name": "whoosh", "atSec": [0.0]}"#),
            ],
        ),
        answer(
            "",
            vec![fc(
                "m",
                "addMusic",
                r#"{"file": "soft ambient", "volume": null, "duck": null, "atSec": 0.0}"#,
            )],
        ),
    ]);
    let project = Arc::new(
        MemoryProjects::open(p, PathBuf::from("/tmp/talk.klipolo"))
            .unwrap()
            .with_pack(vec![soft]),
    );
    let session = ChatSession::new(project.clone(), Found::done(), Paying::key(model.clone()));
    let r = reply(
        &session
            .send(request("add my lofi beat and a whoosh at the start"))
            .await
            .unwrap(),
    );
    assert!(r["applied"].is_object(), "{r:#}");
    let context = match &model.seen()[0].input[0] {
        Item::Context { text } => text.clone(),
        _ => unreachable!(),
    };
    for name in ["lofi-beat.mp3", "whoosh.wav", "Soft ambient"] {
        assert!(context.contains(name), "{name} in {context}");
    }
    // A song from the pack comes into the project with the music that plays it.
    let r = reply(
        &session
            .send(request("use the soft ambient music instead"))
            .await
            .unwrap(),
    );
    assert!(r["applied"].is_object(), "{r:#}");
    assert!(
        project
            .project()
            .asset("klipolo-pack-soft-ambient")
            .is_some()
    );
}

/// Where the music on `p` ends, and its last piece's fade-out.
fn music_end(p: &Project) -> (Us, Us) {
    let last = (p.timeline.tracks.iter())
        .flat_map(|t| &t.items)
        .filter(|i| is_music(i))
        .max_by_key(|i| i.end_us())
        .expect("music");
    let ItemContent::Audio { fade_out_us, .. } = &last.content else {
        unreachable!()
    };
    (last.end_us(), *fade_out_us)
}

#[tokio::test]
async fn chats_cuts_fit_the_music_to_the_new_end() {
    // "Add my song", then "cut the long pauses", then Apply: the song ends at the video's new end, fading out there.
    let mut p = talk();
    p.assets.push(sound_file("song", "song.mp3", 60.0));
    let model = Script::new(vec![
        answer(
            "",
            vec![fc(
                "m",
                "addMusic",
                r#"{"file": "song", "volume": null, "duck": null, "atSec": 0.0}"#,
            )],
        ),
        answer(
            "",
            vec![fc("s", "removeSilences", r#"{"minPauseSec": null}"#)],
        ),
    ]);
    let project = Arc::new(MemoryProjects::open(p, PathBuf::from("/tmp/talk.klipolo")).unwrap());
    let session = ChatSession::new(project.clone(), Found::with_pauses(), Paying::key(model));
    reply(&session.send(request("add my song")).await.unwrap());
    assert_eq!(
        music_end(&project.project()),
        (secs(30.0), MUSIC_FADE_OUT_US)
    );
    let out = session.send(request("cut the long pauses")).await.unwrap();
    // The card says the song is shortened, beside the 2.2 s of footage.
    let r = reply(&out);
    assert_eq!(r["proposal"]["removedUs"], secs(2.2));
    assert_eq!(
        r["proposal"]["removals"]["shortened"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "{r:#}"
    );
    session.apply(&proposal_id(&out)).unwrap();
    let after = project.project();
    assert_eq!(video_duration(&after.timeline), secs(27.8));
    assert_eq!(music_end(&after), (secs(27.8), MUSIC_FADE_OUT_US));
    // One undo takes back the cut and the song's new end together.
    project.step(false).unwrap();
    assert_eq!(
        music_end(&project.project()),
        (secs(30.0), MUSIC_FADE_OUT_US)
    );
}

#[tokio::test]
async fn ums_are_cut_while_the_scene_log_is_made_and_moments_wait() {
    let model = Script::new(vec![
        answer("", vec![fillers()]),
        answer(
            "",
            vec![fc(
                "f",
                "findMoments",
                r#"{"query": "the module", "ranges": []}"#,
            )],
        ),
    ]);
    let chat = chat_with(
        model.clone(),
        Found::at(AnalysisState::Watching),
        Paying::key(model),
    );
    let out = chat.session.send(request("cut the ums")).await.unwrap();
    let ranges = &reply(&out)["proposal"]["removals"]["ranges"];
    assert_eq!(ranges.as_array().unwrap().len(), 2, "both fillers");
    let r = reply(&chat.session.send(request("find the module")).await.unwrap());
    assert_eq!(
        r["text"],
        "Klipolo is still watching this clip (talk.mov). Ask me again when its analysis is done."
    );
}

/// A video file like "talk.mov", `seconds` long.
fn video_file(id: &str, name: &str, seconds: f64) -> Asset {
    Asset {
        id: id.into(),
        path: format!("/media/{name}"),
        name: name.into(),
        duration_us: secs(seconds),
        ..talk().assets[0].clone()
    }
}

/// "talk.mov" played whole, then "more.mov" (10 s) played whole, from 30 to 40 s.
fn talk_then_more() -> Project {
    let mut p = talk();
    p.assets.push(video_file("more", "more.mov", 10.0));
    let mut clip = Clip::new(ClipSource::Asset { id: "more".into() }, 0, secs(10.0));
    clip.id = "m".into();
    p.timeline.main.push(clip);
    p
}

/// The model cuts from `from` to `to`, the person's selection.
fn cut_selected(from: f64, to: f64) -> (Result<ModelResponse, ModelError>, ChatRequest) {
    let range = RangeUs {
        start: secs(from),
        end: secs(to),
    };
    let call = format!(r#"{{"startSec": {from:.1}, "endSec": {to:.1}}}"#);
    (
        answer("", vec![fc("c", "cutRange", &call)]),
        selecting("cut this", range),
    )
}

#[tokio::test]
async fn a_proposal_outlives_words_timed_again_in_files_it_doesnt_cut() {
    // A song and a clip in the library, off the timeline, analysed with words of their own.
    let mut p = talk();
    p.assets.push(sound_file("song", "song.m4a", 60.0));
    p.assets.push(video_file("spare", "spare.mov", 20.0));
    let found = Found::done();
    found.hear("song");
    found.hear("spare");
    let model = Script::new(vec![answer("", vec![fillers()])]);
    let chat = chat_on(p, model.clone(), found.clone(), Paying::key(model));
    let out = chat.session.send(request("cut the ums")).await.unwrap();
    // The card names the files whose words its cuts stand on, for the page to drop it as Rust does.
    assert_eq!(
        reply(&out)["proposal"]["files"],
        serde_json::json!(["talk"])
    );
    let id = proposal_id(&out);
    // Their words are timed again: nothing the cuts stand on moved.
    found.hear("song");
    found.hear("spare");
    chat.session.apply(&id).expect("applied");
    assert_eq!(chat.project.revision(), 1);
    assert!(!plays(&chat.project.project(), 0.6), "the um is cut");
}

#[tokio::test]
async fn a_cut_outlives_another_clips_words_being_timed() {
    let found = Found::done();
    // "more.mov"'s turns are heard and its words are being timed: its words are rough for now.
    found.hear("more");
    let (cut, asked) = cut_selected(12.0, 18.0);
    let model = Script::new(vec![cut]);
    let chat = chat_on(
        talk_then_more(),
        model.clone(),
        found.clone(),
        Paying::key(model),
    );
    let out = chat.session.send(asked).await.unwrap();
    assert_eq!(
        reply(&out)["proposal"]["files"],
        serde_json::json!(["talk"])
    );
    let id = proposal_id(&out);
    // Its words are timed: new words, none of them under the cut.
    found.hear("more");
    chat.session.apply(&id).expect("applied");
    assert_eq!(chat.project.revision(), 1);
}

#[tokio::test]
async fn a_cut_into_a_clip_with_no_words_yet_outlives_its_words_coming_in() {
    let found = Found::done();
    // From the end of "talk.mov" into "more.mov", which has no words yet.
    let (cut, asked) = cut_selected(28.0, 32.0);
    let model = Script::new(vec![cut]);
    let chat = chat_on(
        talk_then_more(),
        model.clone(),
        found.clone(),
        Paying::key(model),
    );
    let out = chat.session.send(asked).await.unwrap();
    assert_eq!(
        reply(&out)["proposal"]["files"],
        serde_json::json!(["talk"])
    );
    let id = proposal_id(&out);
    found.hear("more");
    chat.session.apply(&id).expect("applied");
    assert_eq!(chat.project.revision(), 1);
}

#[tokio::test]
async fn a_cut_is_dropped_when_words_it_cuts_into_are_timed_again() {
    let found = Found::done();
    found.hear("more");
    // From the end of "talk.mov" into "more.mov", made on its words as they were.
    let (cut, asked) = cut_selected(28.0, 32.0);
    let model = Script::new(vec![cut]);
    let chat = chat_on(
        talk_then_more(),
        model.clone(),
        found.clone(),
        Paying::key(model),
    );
    let out = chat.session.send(asked).await.unwrap();
    assert_eq!(
        reply(&out)["proposal"]["files"],
        serde_json::json!(["more", "talk"])
    );
    let id = proposal_id(&out);
    found.hear("more");
    assert_eq!(chat.session.apply(&id).unwrap_err(), CommitError::Moved);
    assert_eq!(chat.project.revision(), 0, "nothing was cut");
}

#[tokio::test]
async fn rough_fillers_point_at_time_words_again_only_while_the_preview_offers_it() {
    let found = Found::done();
    let model = Script::new(vec![
        answer("", vec![fillers()]),
        answer("", vec![fillers()]),
    ]);
    let chat = chat_with(model.clone(), found.clone(), Paying::key(model));
    // Word timing failed for "uh", and timing again can help: the preview's header offers Time words again.
    found.rough(true);
    let r = reply(&chat.session.send(request("cut the ums")).await.unwrap());
    assert_eq!(
        r["text"],
        "Some words in this clip (talk.mov) are only roughly timed, so I can't cut filler words exactly. Press Time \
         words again above the video, then ask me."
    );
    // The words don't match their sound: timing again can't help, the header offers nothing, and nothing points there.
    found.rough(false);
    let r = reply(&chat.session.send(request("remove the ums")).await.unwrap());
    assert_eq!(
        r["text"],
        "Some words in this clip (talk.mov) are only roughly timed, so I can't cut filler words exactly."
    );
    assert_eq!(chat.project.revision(), 0, "nothing was cut");
}

#[tokio::test]
async fn a_proposal_made_before_the_words_were_timed_again_is_dropped() {
    let found = Found::done();
    let model = Script::new(vec![answer("", vec![fillers()])]);
    let chat = chat_with(model.clone(), found.clone(), Paying::key(model));
    let out = chat.session.send(request("cut the ums")).await.unwrap();
    let id = proposal_id(&out);
    // Time words again above the video: new words at new times, the revision the same.
    found.retime();
    assert_eq!(chat.session.apply(&id).unwrap_err(), CommitError::Moved);
    assert_eq!(chat.project.revision(), 0, "nothing was cut");
    assert!(plays(&chat.project.project(), 0.6));
    // Dropped for good.
    assert_eq!(chat.session.apply(&id).unwrap_err(), CommitError::Moved);
    let note = &chat.session.turns()[0].note;
    assert!(
        note.ends_with(
            "The proposal was dropped: the clip's words were timed again before the person applied it."
        ),
        "{note}"
    );
}

/// "talk.mov" played to `spare` s before its end, then a b-roll clip with 2 s of footage before its in point, joined by
/// a dissolve as long as fits (2 s with 1 s spare; 0.16 s with 0.08 s spare). At 2x the talk's spare footage halves.
fn talk_into_broll(spare: f64) -> Project {
    let mut p = talk();
    let mut broll = p.assets[0].clone();
    broll.id = "broll".into();
    broll.name = "broll.mov".into();
    broll.has_audio = false;
    p.assets.push(broll);
    p.timeline.main[0].source_out_us = secs(30.0 - spare);
    let mut b = Clip::new(
        ClipSource::Asset { id: "broll".into() },
        secs(2.0),
        secs(12.0),
    );
    b.id = "b".into();
    p.timeline.main.push(b);
    let dissolve = klipolo_model::timeline::Transition {
        effect: klipolo_model::contrib::builtin::TRANSITION_DISSOLVE.into(),
        duration_us: secs(2.0_f64.min(spare * 2.0)),
        params: Default::default(),
    };
    let set = Op::SetTransition {
        clip_id: "t".into(),
        transition: Some(dissolve),
    };
    klipolo_model::apply_ops(&p, &[set]).unwrap()
}

/// The clip card's speed change of clip "t", as the person makes it by hand.
fn speed_by_hand(p: &Project, speed: f64) -> Project {
    let patch = klipolo_model::ops::ClipPatch {
        speed: Some(speed),
        ..Default::default()
    };
    let op = Op::UpdateClip {
        id: "t".into(),
        patch,
    };
    klipolo_model::apply_ops(p, &[op]).unwrap()
}

#[tokio::test]
async fn a_speed_that_shortens_a_transition_applies_at_once_and_says_so_as_the_card_does() {
    let model = Script::new(vec![answer(
        "Sped up the talk.",
        vec![fc("s", "setClip", r#"{"clip": "c1", "speed": 2}"#)],
    )]);
    let p = talk_into_broll(1.0);
    let chat = chat_on(p, model.clone(), Found::done(), Paying::key(model));
    let p = chat.project.project().clone();
    let out = chat
        .session
        .send(request("speed up the talk 2x"))
        .await
        .unwrap();
    let r = reply(&out);
    assert_eq!(r["text"], "Sped up the talk. A transition is shortened.");
    assert!(r["proposal"].is_null());
    // As the card's `speed.undo.note`: the change, then the page's note on the transitions.
    assert_eq!(r["applied"]["undo"]["code"], "chat.setClip.note");
    assert_eq!(
        r["applied"]["undo"]["params"]["note"],
        serde_json::json!({"code": "timeline.refit", "params": {"removed": 0.0, "shortened": 1.0}})
    );
    assert_eq!(chat.project.undo_steps(), 1);
    // The project is the hand edit's, origin aside.
    assert_eq!(*chat.project.project(), speed_by_hand(&p, 2.0));
    let shortened = chat.project.project().timeline.main[0]
        .transition_out
        .clone();
    assert_eq!(shortened.map(|t| t.duration_us), Some(secs(1.0)));
    chat.project.step(false).unwrap();
    assert_eq!(*chat.project.project(), *p);
}

#[tokio::test]
async fn a_speed_that_leaves_a_transition_no_room_waits_for_apply_then_undoes_as_one_step() {
    let model = Script::new(vec![answer(
        "",
        vec![fc("s", "setClip", r#"{"clip": "c1", "speed": 2}"#)],
    )]);
    let p = talk_into_broll(0.08);
    let chat = chat_on(p, model.clone(), Found::done(), Paying::key(model));
    let p = chat.project.project().clone();
    let out = chat
        .session
        .send(request("speed up the talk 2x"))
        .await
        .unwrap();
    let r = reply(&out);
    assert!(r["applied"].is_null(), "it takes a transition away");
    assert_eq!(
        r["text"],
        "Here is what I would remove. Apply it or discard it. At that speed a transition no longer fits, so the \
         change waits for Apply."
    );
    assert_eq!(r["proposal"]["title"], "Remove 1 transition");
    assert_eq!(
        r["proposal"]["removals"]["transitions"],
        serde_json::json!(["t"])
    );
    let applied = chat.session.apply(&proposal_id(&out)).unwrap();
    assert_eq!(applied.undo.code, "chat.setClip.note");
    let note = serde_json::to_value(&applied.undo.params["note"]).unwrap();
    assert_eq!(
        note,
        serde_json::json!({"code": "timeline.refit", "params": {"removed": 1.0, "shortened": 0.0}})
    );
    assert_eq!(chat.project.undo_steps(), 1);
    assert_eq!(*chat.project.project(), speed_by_hand(&p, 2.0));
    assert_eq!(chat.project.project().timeline.main[0].transition_out, None);
    chat.project.step(false).unwrap();
    assert_eq!(*chat.project.project(), *p);
}

// Following the speaker from chat (`chat::follow`): a fake SAM that sees one person at 0.3, a fake small copy.

mod follow {
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use klipolo_analysis::VideoError;
    use klipolo_analysis::providers::{
        BoxFuture, ProviderError, ProviderErrorKind, ScreenBox, SubjectTracker, TrackedBox,
        TrackedFrame, Tracking, VoiceAnswer, VoiceMatcher,
    };
    use klipolo_analysis::subject::voices::VoiceTurn;
    use klipolo_analysis::subject::{SubjectCopy, frames_for};
    use klipolo_app::chat::follow::NO_CROP;
    use klipolo_app::speaker::{Speaker, Trackers, UNMATCHED, Voices};
    use klipolo_model::fill_focus::{FillFocus, Keep, KeepRange, Voice, Who};
    use klipolo_model::project::FormatKind;
    use klipolo_model::time::{RangeUs, Us};

    use super::*;

    #[derive(Default)]
    pub struct Sam {
        pub calls: AtomicUsize,
        pub phrases: Mutex<Vec<String>>,
        /// A second person far to the right, too.
        pub two: bool,
    }

    impl SubjectTracker for Sam {
        fn model(&self) -> &str {
            "fake-sam"
        }

        fn max_frames(&self) -> u64 {
            15_000
        }

        fn estimate_usd(&self, frames: u64) -> f64 {
            frames.max(50) as f64 * 0.0002
        }

        fn track<'a>(
            &'a self,
            video: &'a Path,
            phrase: &'a str,
        ) -> BoxFuture<'a, Result<Tracking, ProviderError>> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                self.phrases.lock().unwrap().push(phrase.to_owned());
                let frames: u32 = std::fs::read_to_string(video).unwrap().parse().unwrap();
                let person = TrackedBox {
                    id: 1,
                    left: 0.2,
                    top: 0.1,
                    right: 0.4,
                    bottom: 1.0,
                    confidence: Some(0.9),
                };
                let other = TrackedBox {
                    id: 2,
                    left: 0.65,
                    right: 0.85,
                    ..person
                };
                let boxes = if self.two {
                    vec![person, other]
                } else {
                    vec![person]
                };
                Ok(Tracking {
                    frames: (0..frames)
                        .map(|frame| TrackedFrame {
                            frame,
                            boxes: boxes.clone(),
                        })
                        .collect(),
                    frames_billed: u64::from(frames),
                    cost_usd: f64::from(frames.max(50)) * 0.0002,
                    cut_off: false,
                })
            })
        }
    }

    struct Pays(Arc<Sam>, Option<Arc<NoAnswer>>);

    impl Trackers for Pays {
        fn tracker(&self) -> BoxFuture<'_, Result<Arc<dyn SubjectTracker>, String>> {
            let sam = self.0.clone();
            Box::pin(async move { Ok(sam as Arc<dyn SubjectTracker>) })
        }

        fn credit_left(&self) -> BoxFuture<'_, Option<f64>> {
            Box::pin(async { None })
        }

        fn matcher(&self) -> BoxFuture<'_, Option<Arc<dyn VoiceMatcher>>> {
            let spark = self.1.clone();
            Box::pin(async move { spark.map(|s| s as Arc<dyn VoiceMatcher>) })
        }
    }

    /// A matcher whose every call fails, as one cut off by the network.
    pub struct NoAnswer;

    impl VoiceMatcher for NoAnswer {
        fn model(&self) -> &str {
            "fake-spark"
        }

        fn estimate_usd(&self, _seconds: f64) -> f64 {
            0.003
        }

        fn who_speaks<'a>(
            &'a self,
            _video: &'a Path,
            _people: &'a [ScreenBox],
        ) -> BoxFuture<'a, Result<VoiceAnswer, ProviderError>> {
            Box::pin(async {
                Err(ProviderError::new(
                    ProviderErrorKind::Unavailable,
                    "Meta isn't answering.",
                ))
            })
        }
    }

    /// The talk's two voices: A to 15 s, B after, in part 0.
    struct TwoVoices;

    impl Voices for TwoVoices {
        fn turns(&self, _asset_id: &str) -> Vec<VoiceTurn> {
            let turn = |from: f64, to: f64, label: &str| VoiceTurn {
                start_us: secs(from),
                end_us: secs(to),
                voice: Voice {
                    part: 0,
                    label: label.into(),
                },
            };
            vec![turn(0.0, 15.0, "A"), turn(15.0, 30.0, "B")]
        }
    }

    #[derive(Default)]
    pub struct Copies(pub Mutex<Vec<RangeUs>>);

    impl SubjectCopy for Copies {
        fn copy(
            &self,
            _src: &Path,
            range: RangeUs,
            fps: f64,
            out: &Path,
            _cancel: &AtomicBool,
        ) -> Result<Vec<Us>, VideoError> {
            self.0.lock().unwrap().push(range);
            std::fs::write(out, frames_for(range.end - range.start, fps).to_string()).unwrap();
            Ok(vec![])
        }
    }

    pub struct Following {
        pub chat: Chat,
        pub sam: Arc<Sam>,
        pub copies: Arc<Copies>,
        _dir: tempfile::TempDir,
    }

    /// A chat on "talk.mov" (its file a real one, so its tracking is cached) that follows with the fake SAM.
    pub fn following(answers: Vec<Result<ModelResponse, ModelError>>) -> Following {
        following_on(talk(), answers)
    }

    /// `following` on `project` (its first file, "talk.mov", made real).
    pub fn following_on(
        mut project: Project,
        answers: Vec<Result<ModelResponse, ModelError>>,
    ) -> Following {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("talk.mov");
        std::fs::write(&file, b"the talk").unwrap();
        project.assets[0].path = file.to_string_lossy().into_owned();
        let model = Script::new(answers);
        let chat = chat_on(project, model.clone(), Found::done(), Paying::key(model));
        let (sam, copies) = (Arc::new(Sam::default()), Arc::new(Copies::default()));
        let speaker = Speaker::new(
            Arc::new(Pays(sam.clone(), None)),
            copies.clone(),
            dir.path().join("analysis"),
            dir.path().join("work"),
            Arc::new(|_| {}),
        );
        chat.session.set_speaker(Arc::new(speaker));
        Following {
            chat,
            sam,
            copies,
            _dir: dir,
        }
    }

    fn portrait() -> FunctionCall {
        fc(
            "v",
            "setFormat",
            r#"{"format": "portrait", "keepOthers": true}"#,
        )
    }

    fn follow_the_speaker() -> FunctionCall {
        fc("s", "followSubject", r#"{"description": "the speaker"}"#)
    }

    fn speaker_x(p: &Project, at: f64) -> Option<f64> {
        let clip = p.timeline.main.first()?;
        match clip.fill_focus.get(&FormatKind::Portrait)? {
            FillFocus::Speaker { path, .. } => path.centre_at(secs(at)).map(|c| c.0),
            FillFocus::Center => None,
        }
    }

    #[tokio::test]
    async fn a_vertical_version_that_follows_the_speaker_is_one_step_and_says_its_price() {
        let f = following(vec![answer(
            "Done.",
            vec![portrait(), follow_the_speaker()],
        )]);
        let r = reply(
            &f.chat
                .session
                .send(request("make it vertical and keep me in frame"))
                .await
                .unwrap(),
        );
        assert!(r["applied"].is_object(), "{r:#}");
        // 30 s at 2 frames a second: 60 frames, $0.012. SAM looked for a person, not a loudspeaker.
        assert_eq!(
            r["notes"],
            serde_json::json!([
                "The vertical version keeps the speaker in frame now. Tracking cost $0.01."
            ])
        );
        assert_eq!(*f.sam.phrases.lock().unwrap(), ["person"]);
        let p = f.chat.project.project();
        assert_eq!(p.settings.primary_format, FormatKind::Portrait);
        assert!((speaker_x(&p, 10.0).unwrap() - 0.3).abs() < 0.02);
        // One step back takes the format and the follow together.
        assert_eq!(f.chat.project.undo_steps(), 1);
        f.chat.project.step(false).unwrap();
        assert!(speaker_x(&f.chat.project.project(), 10.0).is_none());
        assert!(f.chat.session.turns()[0].note.contains("Followed person"));
    }

    #[tokio::test]
    async fn who_chat_keeps_in_frame_lands_on_the_clip_in_its_source_time() {
        let mut project = talk();
        // The clip plays the talk from 4 s: the timeline's 2 to 8 s is the file's 6 to 12 s.
        project.timeline.main[0].source_in_us = secs(4.0);
        let guest = fc(
            "s",
            "followSubject",
            r#"{"description": "person", "focus": [{"fromSec": 2, "toSec": 8, "person": "P2"}]}"#,
        );
        let f = following_on(
            project,
            vec![answer(
                "Keeping the guest in frame.",
                vec![portrait(), guest],
            )],
        );
        let r = reply(
            &f.chat
                .session
                .send(request("keep the guest in frame while the host asks"))
                .await
                .unwrap(),
        );
        assert!(r["applied"].is_object(), "{r:#}");
        let p = f.chat.project.project();
        let Some(FillFocus::Speaker { keep, .. }) =
            p.timeline.main[0].fill_focus.get(&FormatKind::Portrait)
        else {
            panic!("{:?}", p.timeline.main[0].fill_focus)
        };
        assert_eq!(
            *keep,
            Keep::Ranges {
                ranges: vec![KeepRange {
                    start_us: secs(6.0),
                    end_us: secs(12.0),
                    who: Who::Person { person: 2 },
                }],
            }
        );
        // One step, as any follow.
        assert_eq!(f.chat.project.undo_steps(), 1);
    }

    #[tokio::test]
    async fn who_chat_keeps_in_frame_carries_to_the_pieces_a_cut_in_the_same_answer_makes() {
        let cut = fc("c", "cutRange", r#"{"startSec": 10.0, "endSec": 12.0}"#);
        let guest = fc(
            "s",
            "followSubject",
            r#"{"description": "person", "focus": [{"fromSec": 2, "toSec": 20, "person": "P2"}]}"#,
        );
        let f = following(vec![answer("Done.", vec![portrait(), cut, guest])]);
        let out = f
            .chat
            .session
            .send(request(
                "cut 10.0 to 12.0 and keep the guest in frame from 2.0 to 20.0",
            ))
            .await
            .unwrap();
        f.chat.session.apply(&proposal_id(&out)).unwrap();
        let p = f.chat.project.project();
        assert_eq!(p.timeline.main.len(), 2, "{:?}", p.timeline.main);
        // Both pieces keep the guest over their own part of 2 to 20 s of the file.
        for clip in &p.timeline.main {
            let Some(FillFocus::Speaker { keep, .. }) = clip.fill_focus.get(&FormatKind::Portrait)
            else {
                panic!("{}: {:?}", clip.id, clip.fill_focus)
            };
            assert_eq!(
                *keep,
                Keep::Ranges {
                    ranges: vec![KeepRange {
                        start_us: secs(2.0),
                        end_us: secs(20.0),
                        who: Who::Person { person: 2 },
                    }],
                },
                "{}",
                clip.id
            );
        }
    }

    #[tokio::test]
    async fn focus_chat_named_that_isnt_used_is_told_in_the_follow_note() {
        // One person on screen: P3 isn't there; and a range past the video's end touches no clip.
        let guest = fc(
            "s",
            "followSubject",
            r#"{"description": "person", "focus": [{"fromSec": 2, "toSec": 8, "person": "P3"}, {"fromSec": 50, "toSec": 60, "person": "P1"}]}"#,
        );
        let f = following(vec![answer("Done.", vec![portrait(), guest])]);
        f.chat
            .session
            .send(request("keep the third person in frame"))
            .await
            .unwrap();
        let note = &f.chat.session.turns()[0].note;
        assert!(note.contains("P3 isn't on screen"), "{note}");
        assert!(note.contains("outside the video"), "{note}");
    }

    #[tokio::test]
    async fn the_reply_says_when_who_speaks_could_not_be_told() {
        let dir = tempfile::tempdir().unwrap();
        let mut project = talk();
        let file = dir.path().join("talk.mov");
        std::fs::write(&file, b"the talk").unwrap();
        project.assets[0].path = file.to_string_lossy().into_owned();
        let model = Script::new(vec![answer(
            "Done.",
            vec![portrait(), follow_the_speaker()],
        )]);
        let chat = chat_on(project, model.clone(), Found::done(), Paying::key(model));
        let sam = Arc::new(Sam {
            two: true,
            ..Sam::default()
        });
        let speaker = Speaker::new(
            Arc::new(Pays(sam, Some(Arc::new(NoAnswer)))),
            Arc::new(Copies::default()),
            dir.path().join("analysis"),
            dir.path().join("work"),
            Arc::new(|_| {}),
        )
        .with_voices(Arc::new(TwoVoices));
        chat.session.set_speaker(Arc::new(speaker));
        let r = reply(
            &chat
                .session
                .send(request("make it vertical and keep whoever talks in frame"))
                .await
                .unwrap(),
        );
        assert!(r["applied"].is_object(), "{r:#}");
        assert!(r["notes"].to_string().contains(UNMATCHED), "{r:#}");
    }

    #[tokio::test]
    async fn a_short_tracks_only_what_its_cut_keeps_and_follows_once_applied() {
        let keep = fc(
            "k",
            "keepOnly",
            r#"{"ranges": [{"startSec": 0.0, "endSec": 7.8}]}"#,
        );
        let f = following(vec![answer(
            "Here is the short.",
            vec![portrait(), keep, follow_the_speaker()],
        )]);
        let out = f
            .chat
            .session
            .send(request("make a vertical short of the first sentence"))
            .await
            .unwrap();
        let r = reply(&out);
        assert!(r["proposal"].is_object(), "{r:#}");
        // Only the 8 s or so the cut keeps (to its frame) was sent: 16 frames, billed as 50.
        let sent = f.copies.0.lock().unwrap().clone();
        assert_eq!(sent.len(), 1, "{sent:?}");
        assert_eq!(sent[0].start, 0);
        assert!((secs(7.8)..secs(8.0)).contains(&sent[0].end), "{sent:?}");
        let notes = r["notes"].to_string();
        assert!(
            notes.contains("once you apply this") && notes.contains("$0.01"),
            "{notes}"
        );
        // The vertical version is in; the follow lands with the cut.
        assert!(speaker_x(&f.chat.project.project(), 5.0).is_none());
        f.chat.session.apply(&proposal_id(&out)).unwrap();
        assert!((speaker_x(&f.chat.project.project(), 5.0).unwrap() - 0.3).abs() < 0.02);
    }

    #[tokio::test]
    async fn chats_new_vertical_version_offers_to_follow_with_its_price_and_follows_free_footage() {
        let f = following(vec![
            answer("It's vertical now.", vec![portrait()]),
            answer("Following.", vec![follow_the_speaker()]),
            answer("Back to 16:9.", vec![fc("u", "undo", "{}")]),
            answer("Vertical again.", vec![portrait()]),
        ]);
        let r = reply(
            &f.chat
                .session
                .send(request("make it vertical"))
                .await
                .unwrap(),
        );
        assert_eq!(
            r["notes"],
            serde_json::json!([
                "To keep the speaker in frame, say follow the speaker. It costs about $0.01."
            ])
        );
        assert_eq!(
            f.sam.calls.load(Ordering::SeqCst),
            0,
            "offering sends nothing"
        );
        let r = reply(
            &f.chat
                .session
                .send(request("follow the speaker"))
                .await
                .unwrap(),
        );
        assert!(r["applied"].is_object(), "{r:#}");
        assert_eq!(r["applied"]["undo"]["code"], "chat.followSubject");
        assert_eq!(f.sam.calls.load(Ordering::SeqCst), 1);
        // Tracked once: the next vertical version of that footage follows at no charge.
        f.chat.session.send(request("undo")).await.unwrap();
        f.chat.project.step(false).unwrap();
        let r = reply(
            &f.chat
                .session
                .send(request("vertical again"))
                .await
                .unwrap(),
        );
        assert!(r["notes"].to_string().contains("at no charge"), "{r:#}");
        assert_eq!(f.sam.calls.load(Ordering::SeqCst), 1);
        assert!(speaker_x(&f.chat.project.project(), 5.0).is_some());
    }

    #[tokio::test]
    async fn following_in_a_16_9_video_asks_for_a_vertical_version_first() {
        let f = following(vec![answer("Done.", vec![follow_the_speaker()])]);
        let r = reply(
            &f.chat
                .session
                .send(request("follow the speaker"))
                .await
                .unwrap(),
        );
        // The model's "Done." was written before the follow ran: what it came to is the reply instead.
        assert_eq!(r["text"], NO_CROP);
        assert_eq!(r["notes"], serde_json::json!([]));
        assert_eq!(f.sam.calls.load(Ordering::SeqCst), 0);
        assert_eq!(f.chat.project.revision(), 0);
    }

    #[tokio::test]
    async fn a_format_the_person_left_on_fit_is_not_followed() {
        let mut project = talk();
        project.settings.formats = vec![FormatKind::Landscape, FormatKind::Square];
        let fit = klipolo_model::timeline::Framing {
            mode: klipolo_model::timeline::FramingMode::Fit,
            focus_x: 0.5,
            focus_y: 0.5,
            follow: None,
        };
        project.timeline.main[0]
            .framing
            .insert(FormatKind::Square, fit);
        let f = following_on(
            project,
            vec![answer("Done.", vec![portrait(), follow_the_speaker()])],
        );
        let r = reply(
            &f.chat
                .session
                .send(request("vertical, and follow me"))
                .await
                .unwrap(),
        );
        assert!(r["applied"].is_object(), "{r:#}");
        let p = f.chat.project.project();
        let clip = &p.timeline.main[0];
        // The new vertical version follows; the square one stays on Fit, untouched.
        assert!(speaker_x(&p, 10.0).is_some());
        assert!(!clip.fill_focus.contains_key(&FormatKind::Square));
        assert_eq!(clip.framing[&FormatKind::Square], fit);
        assert_eq!(
            r["notes"],
            serde_json::json!([
                "The vertical version keeps the speaker in frame now. Tracking cost $0.01."
            ])
        );
    }

    #[tokio::test]
    async fn a_named_subject_is_sent_without_its_article_and_two_versions_keep_them_in_frame() {
        let mut project = talk();
        project.settings.formats = vec![FormatKind::Landscape, FormatKind::Square];
        let car = fc("s", "followSubject", r#"{"description": "the red car"}"#);
        let f = following_on(project, vec![answer("Done.", vec![portrait(), car])]);
        let r = reply(
            &f.chat
                .session
                .send(request("vertical, and follow the red car"))
                .await
                .unwrap(),
        );
        assert_eq!(*f.sam.phrases.lock().unwrap(), ["red car"]);
        assert_eq!(
            r["notes"],
            serde_json::json!([
                "The vertical and square versions keep the red car in frame now. Tracking cost $0.01."
            ])
        );
    }
}
