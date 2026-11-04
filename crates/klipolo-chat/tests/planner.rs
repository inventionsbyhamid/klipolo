//! The planner: one chat message to the language model, step by step until it answers in words. The model is a fake
//! that hands out scripted answers and keeps every request it was sent, so the tests read exactly what a provider
//! would get. The project is `talk()`: "talk.mov", 30 s, played whole, its words from 0.5 to 7.8 s ("Um, so today we
//! look at the new module. It took [pause 1.5 s] uh a long time."), then nothing is said, so a cut from 12.3 to 19.7 s
//! stands on nothing and is refused back to the model.

use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::pin::pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use klipolo_chat::planner::OUT_OF_CREDIT;
use klipolo_chat::{
    ANALYZE_FIRST, AnalysisState, BoxFuture, ChatContext, ChatError, ChatErrorKind, DONE_OUTPUT,
    FunctionCall, Inputs, Item, KEPT_TURNS, LanguageModel, Library, LibraryItem,
    MAX_CONTEXT_TOKENS, MAX_STEPS, MediaKind, ModelError, ModelRequest, ModelResponse,
    PROPOSED_OUTPUT, PlanOut, Refs, STILL_LISTENING, STOPPED, TOLD_PERSON, TOO_LONG, ToolCall,
    Turn, View, build_context, plan, remember, sanitize, system_prompt, tools,
};
use klipolo_model::Project;
use klipolo_model::project::{Asset, AssetKind};
use klipolo_model::speech::{Speech, Transcript, Word, WordKind};
use klipolo_model::time::{RangeUs, Us};
use klipolo_model::timeline::{Clip, ClipSource};

fn secs(s: f64) -> Us {
    (s * 1e6).round() as Us
}

fn span(a: f64, b: f64) -> RangeUs {
    RangeUs {
        start: secs(a),
        end: secs(b),
    }
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

/// "talk.mov", 30 s, played whole, with its words.
fn talk() -> (Project, Speech) {
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
    let mut speech = Speech::default();
    speech.transcripts.insert(
        "talk".into(),
        Arc::new(Transcript {
            key: "key".into(),
            language: "en".into(),
            model: "test".into(),
            covered_us: secs(30.0),
            complete: true,
            words: said("Um, so today we look at the new module. It took |1.5 uh a long time."),
        }),
    );
    (p, speech)
}

/// What one message compiles against, every file analyzed, the playhead at 0 and no selection. `message` is left
/// empty: the planner reads the person's message from its own argument.
fn inputs(project: &Project, speech: Speech) -> Inputs {
    Inputs {
        turns: Default::default(),
        message: String::new(),
        view: View::default(),
        refs: Refs::of(project),
        speech,
        scenes: HashMap::new(),
        analysis: (project.assets.iter())
            .map(|a| (a.id.clone(), AnalysisState::Done))
            .collect(),
        time_again: HashSet::new(),
        cursor: None,
        library: Library::default(),
        fonts: Default::default(),
    }
}

fn context(project: &Project, inputs: &Inputs) -> ChatContext {
    build_context(
        project,
        &inputs.speech,
        &HashMap::new(),
        &inputs.scenes,
        &inputs.view,
        &inputs.library,
    )
}

/// A model that answers from a script, in order, and keeps every request.
struct Fake {
    answers: Mutex<VecDeque<Result<ModelResponse, ModelError>>>,
    seen: Mutex<Vec<ModelRequest>>,
}

impl Fake {
    fn new(answers: Vec<Result<ModelResponse, ModelError>>) -> Self {
        Self {
            answers: Mutex::new(answers.into()),
            seen: Mutex::new(Vec::new()),
        }
    }

    fn seen(&self) -> Vec<ModelRequest> {
        self.seen.lock().unwrap().clone()
    }

    fn left(&self) -> usize {
        self.answers.lock().unwrap().len()
    }
}

impl LanguageModel for Fake {
    fn respond<'a>(
        &'a self,
        request: &'a ModelRequest,
    ) -> BoxFuture<'a, Result<ModelResponse, ModelError>> {
        self.seen.lock().unwrap().push(request.clone());
        // Past the script, the model is done: words alone, here none.
        let answer =
            (self.answers.lock().unwrap().pop_front()).unwrap_or_else(|| answer("", vec![], 0.0));
        Box::pin(async move { answer })
    }
}

/// `future`'s output: the fake answers at once, so the planner never waits.
fn now<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    match future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(out) => out,
        Poll::Pending => panic!("the planner waited on a fake that answers at once"),
    }
}

fn fc(call_id: &str, name: &str, arguments: &str) -> FunctionCall {
    FunctionCall {
        name: name.into(),
        call_id: call_id.into(),
        arguments: arguments.into(),
    }
}

fn cut_call(call_id: &str, a: f64, b: f64) -> FunctionCall {
    fc(
        call_id,
        "cutRange",
        &format!(r#"{{"startSec": {a}, "endSec": {b}}}"#),
    )
}

fn square_call(call_id: &str) -> FunctionCall {
    fc(
        call_id,
        "setFormat",
        r#"{"format": "square", "keepOthers": true}"#,
    )
}

fn answer(
    text: &str,
    calls: Vec<FunctionCall>,
    usage_usd: f64,
) -> Result<ModelResponse, ModelError> {
    Ok(ModelResponse {
        text: text.into(),
        calls,
        usage_usd,
    })
}

fn parsed(call: &FunctionCall) -> ToolCall {
    ToolCall::parse(call).unwrap()
}

/// Runs one message against `talk()` with `history`.
fn send(
    model: &Fake,
    history: &[Turn],
    user: &str,
    inputs: &Inputs,
    project: &Project,
) -> Result<PlanOut, ChatError> {
    let ctx = context(project, inputs);
    now(plan(model, history, &ctx, user, inputs, project))
}

/// The output the second request sends back for `call_id`.
fn output_for<'r>(request: &'r ModelRequest, call_id: &str) -> &'r str {
    (request.input.iter())
        .find_map(|item| match item {
            Item::FunctionCallOutput {
                call_id: id,
                output,
            } if id == call_id => Some(output.as_str()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no output for {call_id}"))
}

const DASHES: [char; 4] = ['\u{2012}', '\u{2013}', '\u{2014}', '\u{2015}'];

fn assert_plain(text: &str) {
    assert!(!text.is_empty(), "a sentence for the person");
    assert!(!text.contains(DASHES), "{text}");
}

#[test]
fn refusal_round_trip_then_success() {
    let (p, speech) = talk();
    let i = inputs(&p, speech);
    let invented = cut_call("call_1", 12.3, 19.7);
    let fixed = cut_call("call_2", 7.9, 29.8);
    let model = Fake::new(vec![
        answer("Cutting the quiet part.", vec![invented.clone()], 0.004),
        answer(
            "I'll cut everything after the talk.",
            vec![fixed.clone()],
            0.003,
        ),
    ]);
    let out = send(&model, &[], "cut the boring bit", &i, &p).unwrap();

    // Asked again after each answer with calls: the third answer, words alone (none here), ends it.
    let seen = model.seen();
    assert_eq!(seen.len(), 3);
    let (first, second) = (&seen[0], &seen[1]);
    // The whole first request is sent again, with nothing stored at the provider: same system text and tools, and
    // the first input whole, then what the model said and called, then what came of each call.
    assert_eq!(
        (&second.system, &second.tools),
        (&first.system, &first.tools)
    );
    assert_eq!(&second.input[..first.input.len()], &first.input[..]);
    let added = &second.input[first.input.len()..];
    assert_eq!(added.len(), 3, "{added:?}");
    assert_eq!(
        added[0],
        Item::Assistant {
            text: "Cutting the quiet part.".into()
        }
    );
    assert_eq!(added[1], Item::FunctionCall(invented));
    let reason = output_for(second, "call_1");
    assert!(
        reason.starts_with("cutRange: startSec 12.3 is not within 0.3 s of a word"),
        "{reason}"
    );

    // The second answer is compiled against the same snapshot: its cut snaps to the last word and the video's end.
    let proposal = out.compiled.proposal.as_ref().expect("a proposal");
    assert_eq!(proposal.ranges, vec![span(7.8, 30.0)]);
    assert_eq!(out.compiled.additions, None);
    assert_eq!(out.compiled.refusals, vec![]);
    assert_eq!(out.calls, vec![parsed(&fixed)]);
    assert_eq!(out.answers.len(), 3);
    assert_eq!(out.reply, "I'll cut everything after the talk.");
    assert!((out.usage_usd - 0.007).abs() < 1e-12, "{}", out.usage_usd);
}

#[test]
fn a_call_refused_on_every_step_is_left_undone() {
    let (p, speech) = talk();
    let i = inputs(&p, speech);
    // Every step's try at another part nothing stands on, then one that would stand: never asked for.
    let tries: Vec<_> = (0..MAX_STEPS)
        .map(|k| (12.0 + 0.5 * k as f64, 18.0 + 0.5 * k as f64))
        .chain([(7.9, 29.8)])
        .enumerate()
        .map(|(k, (a, b))| {
            answer(
                "Cutting it now.",
                vec![cut_call(&format!("call_{k}"), a, b)],
                0.004,
            )
        })
        .collect();
    let model = Fake::new(tries);
    let out = send(&model, &[], "cut the boring bit", &i, &p).unwrap();

    assert_eq!(model.seen().len(), MAX_STEPS);
    assert_eq!(model.left(), 1, "the last answer is never asked for");
    // Nothing invented is applied, and the reply says plainly what couldn't be done.
    assert_eq!(out.compiled.proposal, None);
    assert_eq!(out.compiled.additions, None);
    assert_plain(&out.reply);
    // What it couldn't find, and what to do next.
    assert_eq!(
        out.reply,
        "I couldn't find that part, so I left the video as it was. Select it on the timeline and ask again."
    );
    // The model's claim that it is cutting is not repeated when nothing came of it.
    assert!(!out.reply.contains("Cutting it now"), "{}", out.reply);
    assert!((out.usage_usd - 0.004 * MAX_STEPS as f64).abs() < 1e-12);
}

/// The serialized request up to and including the context item.
fn through_context(request: &ModelRequest) -> String {
    let all = serde_json::to_string(request).unwrap();
    let ctx = (request.input.iter())
        .find(|item| matches!(item, Item::Context { .. }))
        .expect("a context item");
    let ctx = serde_json::to_string(ctx).unwrap();
    let at = all.find(&ctx).expect("the context in the request");
    all[..at + ctx.len()].to_owned()
}

#[test]
fn prefix_is_byte_identical_across_messages() {
    let (p, speech) = talk();
    let i = inputs(&p, speech);
    let model = Fake::new(vec![
        answer("Which part is boring?", vec![], 0.002),
        answer("Done.", vec![square_call("call_1")], 0.002),
    ]);
    let first = send(&model, &[], "cut the boring bit", &i, &p).unwrap();
    let history = vec![first.turn("cut the boring bit")];
    // The second message is on the same revision: the square format is applied only after it.
    send(&model, &history, "make it square", &i, &p).unwrap();

    let seen = model.seen();
    let (a, b) = (through_context(&seen[0]), through_context(&seen[1]));
    assert_eq!(a, b);
    // The prefix holds the system text, then the tools, then the context, in that order.
    let system = a.find(&serde_json::to_string(system_prompt()).unwrap());
    let tools = a.find(&serde_json::to_string(&tools()).unwrap());
    assert!(system.is_some() && tools.is_some());
    assert!(system < tools);
    assert!(matches!(seen[1].input[0], Item::Context { .. }));
    // The conversation comes after the context, and the new message last.
    assert_eq!(
        seen[1].input.last(),
        Some(&Item::User {
            text: "make it square".into()
        })
    );
    assert_eq!(
        seen[1].input[1],
        Item::User {
            text: "cut the boring bit".into()
        }
    );
}

#[test]
fn reply_dashes_are_removed() {
    let (p, speech) = talk();
    let i = inputs(&p, speech);
    let text = "Done \u{2014} it's square now \u{2013} want captions too?";
    let model = Fake::new(vec![answer(text, vec![square_call("call_1")], 0.002)]);
    let out = send(&model, &[], "make it square", &i, &p).unwrap();
    assert_eq!(out.reply, sanitize::reply(text));
    assert_plain(&out.reply);
    assert!(out.compiled.additions.is_some());

    // What the model writes for the person through a tool loses its dashes too.
    let model = Fake::new(vec![answer(
        "",
        vec![fc(
            "call_1",
            "cannotDo",
            r#"{"reason": "Klipolo can't translate captions — not yet."}"#,
        )],
        0.002,
    )]);
    let out = send(&model, &[], "translate the captions", &i, &p).unwrap();
    assert_plain(&out.reply);
    assert!(out.reply.contains("translate captions"), "{}", out.reply);
}

#[test]
fn meta_402_returns_budget_error_and_no_changes() {
    let (p, speech) = talk();
    let before = p.clone();
    let i = inputs(&p, speech);
    let said = "This device has used its Klipolo credit.";
    let model = Fake::new(vec![Err(ModelError::Budget(said.into()))]);
    let e = send(&model, &[], "make it square", &i, &p).unwrap_err();
    assert_eq!(e.kind, ChatErrorKind::Budget);
    assert_eq!(e.message, said);
    assert_eq!(e.usage_usd, 0.0);
    assert_eq!(e.model_calls, 1);
    assert_eq!(p, before);

    // Out of credit on the next step: what the first answer did stands, the cut it got wrong is named as not done,
    // and the reply says it stopped. The first call counts.
    let model = Fake::new(vec![
        answer(
            "Square, and cutting it.",
            vec![square_call("call_1"), cut_call("call_2", 12.3, 19.7)],
            0.004,
        ),
        Err(ModelError::Budget(String::new())),
    ]);
    let out = send(&model, &[], "square, and cut the boring bit", &i, &p).unwrap();
    assert_eq!(out.calls, vec![parsed(&square_call("call_1"))]);
    assert!(out.compiled.additions.is_some());
    assert!(
        out.reply.contains("I couldn't find that part"),
        "{}",
        out.reply
    );
    assert!(
        out.reply.ends_with(&format!("{OUT_OF_CREDIT} {STOPPED}")),
        "{}",
        out.reply
    );
    assert_plain(&out.reply);
    assert_eq!(out.usage_usd, 0.004);
    assert_eq!(p, before);
}

#[test]
fn an_answer_cut_off_or_failed_changes_nothing_and_still_counts() {
    let (p, speech) = talk();
    let before = p.clone();
    let i = inputs(&p, speech);
    let cut_off = || ModelError::Other {
        detail: "cut off: max_output_tokens".into(),
        usage_usd: 0.01,
    };
    let model = Fake::new(vec![Err(cut_off())]);
    let e = send(&model, &[], "make it square", &i, &p).unwrap_err();
    assert_eq!(e.kind, ChatErrorKind::Other);
    assert_plain(&e.message);
    assert_eq!(e.detail, "failed: cut off: max_output_tokens");
    assert_eq!(e.usage_usd, 0.01);

    // Cut off on the next step: both calls count, what the first answer did stands, and the reply says it stopped.
    let model = Fake::new(vec![
        answer(
            "",
            vec![square_call("call_1"), cut_call("call_2", 12.3, 19.7)],
            0.004,
        ),
        Err(cut_off()),
    ]);
    let out = send(&model, &[], "square, and cut the boring bit", &i, &p).unwrap();
    assert!((out.usage_usd - 0.014).abs() < 1e-12, "{}", out.usage_usd);
    assert!(out.compiled.additions.is_some());
    assert!(out.reply.ends_with(STOPPED), "{}", out.reply);
    assert!(
        out.note.contains("Stopped early: failed: cut off"),
        "{}",
        out.note
    );
    assert_eq!(p, before);
}

#[test]
fn every_model_error_is_one_plain_sentence_and_changes_nothing() {
    let (p, speech) = talk();
    let before = p.clone();
    let i = inputs(&p, speech);
    let cases = [
        (ModelError::Busy(None), ChatErrorKind::Busy),
        (ModelError::Unavailable(None), ChatErrorKind::Unavailable),
        (ModelError::Network, ChatErrorKind::Network),
        (ModelError::Refused, ChatErrorKind::Refused),
        (ModelError::Declined(String::new()), ChatErrorKind::Declined),
        (
            ModelError::other("400: unsupported field"),
            ChatErrorKind::Other,
        ),
    ];
    let mut sentences = Vec::new();
    for (error, kind) in cases {
        let model = Fake::new(vec![Err(error.clone())]);
        let e = send(&model, &[], "make it square", &i, &p).unwrap_err();
        assert_eq!(e.kind, kind, "{error:?}");
        assert_plain(&e.message);
        assert!(e.message.ends_with('.'), "{}", e.message);
        // A log detail is never shown, but kept for logs.
        assert!(!e.message.contains("unsupported"), "{}", e.message);
        assert_eq!(e.detail, error.to_string());
        sentences.push(e.message);
    }
    sentences.sort();
    sentences.dedup();
    assert_eq!(sentences.len(), 6, "one sentence for each: {sentences:?}");
    assert_eq!(p, before);
}

#[test]
fn a_providers_own_sentence_is_shown_without_dashes() {
    let (p, speech) = talk();
    let i = inputs(&p, speech);
    let cases = [
        (
            ModelError::Unavailable(Some(
                "Klipolo's server is busy right now \u{2014} try again.".into(),
            )),
            ChatErrorKind::Unavailable,
            "Klipolo's server is busy right now, try again.",
        ),
        (
            ModelError::Declined("Meta turned this down under its content policy.".into()),
            ChatErrorKind::Declined,
            "Meta turned this down under its content policy.",
        ),
    ];
    for (error, kind, said) in cases {
        let model = Fake::new(vec![Err(error)]);
        let e = send(&model, &[], "make it square", &i, &p).unwrap_err();
        assert_eq!((e.kind, e.message.as_str()), (kind, said));
    }
}

#[test]
fn too_long_a_context_is_refused_before_any_model_call() {
    let (p, speech) = talk();
    let i = inputs(&p, speech);
    let ctx = ChatContext {
        tokens: MAX_CONTEXT_TOKENS + 1,
        ..context(&p, &i)
    };
    assert!(!ctx.fits());
    let model = Fake::new(vec![]);
    let e = now(plan(&model, &[], &ctx, "make it square", &i, &p)).unwrap_err();
    assert_eq!(e.kind, ChatErrorKind::TooLong);
    assert_eq!(e.message, TOO_LONG);
    assert_plain(&e.message);
    assert_eq!(model.seen().len(), 0);
    assert_eq!(e.model_calls, 0);
}

#[test]
fn the_first_answers_accepted_calls_are_kept_through_the_round() {
    let (p, speech) = talk();
    let i = inputs(&p, speech);
    let square = square_call("call_1");
    let fillers = fc("call_2", "removeFillers", "{}");
    let fixed = cut_call("call_4", 7.9, 29.8);
    let model = Fake::new(vec![
        answer(
            "",
            vec![
                square.clone(),
                fillers.clone(),
                cut_call("call_3", 12.3, 19.7),
            ],
            0.004,
        ),
        // The model repeats an accepted call: it is compiled once.
        answer(
            "Square, and the rest cut.",
            vec![square_call("call_5"), fixed.clone()],
            0.003,
        ),
    ]);
    let out = send(&model, &[], "square, no ums, and cut the rest", &i, &p).unwrap();

    let second = &model.seen()[1];
    assert_eq!(output_for(second, "call_1"), "done");
    assert_eq!(output_for(second, "call_2"), PROPOSED_OUTPUT);
    assert!(output_for(second, "call_3").starts_with("cutRange: startSec 12.3"));
    // No empty assistant text is sent back.
    assert!(
        !(second.input.iter())
            .any(|item| matches!(item, Item::Assistant { text } if text.is_empty()))
    );

    // One additions transaction and one proposal, from both answers.
    assert_eq!(
        out.calls,
        vec![parsed(&square), parsed(&fillers), parsed(&fixed)]
    );
    let additions = out.compiled.additions.as_ref().expect("the square format");
    assert_eq!(additions.label, "chat.setFormat");
    let proposal = out
        .compiled
        .proposal
        .as_ref()
        .expect("the fillers and the cut");
    assert_eq!(proposal.tx.label, "chat.andMore");
    assert_eq!(proposal.ranges.last(), Some(&span(7.8, 30.0)));
    assert!(proposal.ranges.len() >= 2, "{:?}", proposal.ranges);
    assert_eq!(out.compiled.refusals, vec![]);
    assert_eq!(out.reply, "Square, and the rest cut.");
    assert!(out.note.contains("setFormat"), "{}", out.note);
    assert!(out.note.contains("cutRange"), "{}", out.note);
    assert!(!out.note.contains('\n'), "{}", out.note);
}

#[test]
fn refusals_for_the_person_are_shown_not_sent_back() {
    let (p, speech) = talk();
    let mut i = inputs(&p, speech);
    i.analysis
        .insert("talk".into(), AnalysisState::Transcribing);

    // Only refusals for the person: the model is told only that the person was told, and the sentences are the reply's.
    let model = Fake::new(vec![answer(
        "",
        vec![
            fc("call_1", "removeFillers", "{}"),
            fc(
                "call_2",
                "cannotDo",
                r#"{"reason": "Klipolo can't translate captions yet."}"#,
            ),
        ],
        0.002,
    )]);
    let out = send(&model, &[], "remove the ums and translate", &i, &p).unwrap();
    assert_eq!(model.seen().len(), 2);
    assert_eq!(output_for(&model.seen()[1], "call_1"), TOLD_PERSON);
    assert!(out.reply.contains(STILL_LISTENING), "{}", out.reply);
    assert!(
        out.reply.contains("Klipolo can't translate captions yet."),
        "{}",
        out.reply
    );
    assert_eq!(out.compiled.proposal, None);

    // Beside a refusal for the model, the person's is answered with a plain note, never its reason.
    let model = Fake::new(vec![
        answer(
            "",
            vec![
                fc("call_1", "removeFillers", "{}"),
                cut_call("call_2", 12.3, 19.7),
            ],
            0.002,
        ),
        answer("Cut the end.", vec![cut_call("call_3", 7.9, 29.8)], 0.002),
    ]);
    let out = send(&model, &[], "remove the ums and cut the rest", &i, &p).unwrap();
    let second = &model.seen()[1];
    assert_eq!(output_for(second, "call_1"), TOLD_PERSON);
    assert!(!output_for(second, "call_1").contains(STILL_LISTENING));
    assert!(out.reply.contains(STILL_LISTENING), "{}", out.reply);
    assert!(out.compiled.proposal.is_some());
}

#[test]
fn nothing_to_remove_gets_one_clear_reply() {
    // talk.mov has no loudness, so no pause to remove: the person is told, and the model's claim that it is removing
    // them is not repeated when it closes with no words.
    let (p, speech) = talk();
    let i = inputs(&p, speech);
    let model = Fake::new(vec![
        answer(
            "Removing the long pauses.",
            vec![fc("call_1", "removeSilences", r#"{"minPauseSec": 1}"#)],
            0.002,
        ),
        answer("", vec![], 0.002),
    ]);
    let out = send(&model, &[], "remove pauses over a second", &i, &p).unwrap();
    assert_eq!(model.seen().len(), 2);
    assert_eq!(output_for(&model.seen()[1], "call_1"), TOLD_PERSON);
    assert_eq!(
        out.reply,
        "There are no pauses longer than 1.0 s to remove."
    );
    assert_eq!(out.compiled.proposal, None);
    assert_eq!(out.usage_usd, 0.004);
}

#[test]
fn unreadable_calls_go_back_to_the_model() {
    let (p, speech) = talk();
    let i = inputs(&p, speech);
    let model = Fake::new(vec![
        answer(
            "",
            vec![fc("call_1", "cutRange", r#"{"startSec": "soon"}"#)],
            0.002,
        ),
        answer("Cut.", vec![cut_call("call_2", 7.9, 29.8)], 0.002),
    ]);
    let out = send(&model, &[], "cut the end", &i, &p).unwrap();
    let seen = model.seen();
    let said = output_for(&seen[1], "call_1");
    assert_eq!(
        said,
        "cutRange: startSec must be a number of seconds, not text."
    );
    assert!(out.compiled.proposal.is_some());
}

#[test]
fn a_part_still_refused_is_named_beside_what_was_done() {
    let (p, speech) = talk();
    let i = inputs(&p, speech);
    let model = Fake::new(vec![
        answer(
            "",
            vec![square_call("call_1"), cut_call("call_2", 12.3, 19.7)],
            0.002,
        ),
        answer(
            "Trying that cut again.",
            vec![cut_call("call_3", 13.1, 18.4)],
            0.002,
        ),
    ]);
    let out = send(&model, &[], "square, and cut the boring bit", &i, &p).unwrap();
    assert!(out.compiled.additions.is_some(), "the square format stays");
    assert_eq!(out.compiled.proposal, None);
    assert!(
        out.reply.starts_with("Trying that cut again."),
        "{}",
        out.reply
    );
    // The square format went in, so the video wasn't left as it was: only the part is.
    assert!(
        out.reply.ends_with(
            "I couldn't find that part, so I left it out. Select it on the timeline and ask again."
        ),
        "{}",
        out.reply
    );
    assert_plain(&out.reply);
}

#[test]
fn the_message_is_evidence_for_the_times_it_says() {
    let (p, speech) = talk();
    // The caller left the message out of the inputs: the planner reads it from its own argument.
    let i = inputs(&p, speech);
    let model = Fake::new(vec![answer(
        "Cut.",
        vec![cut_call("call_1", 12.0, 15.0)],
        0.002,
    )]);
    let out = send(&model, &[], "cut 12 to 15", &i, &p).unwrap();
    assert_eq!(model.seen().len(), 2);
    assert_eq!(
        out.compiled.proposal.expect("the cut").ranges,
        vec![span(12.0, 15.0)]
    );
}

#[test]
fn history_keeps_the_last_six_exchanges() {
    let (p, speech) = talk();
    let i = inputs(&p, speech);
    let turn = |n: usize| Turn {
        user: format!("ask {n}"),
        reply: format!("reply {n}"),
        note: format!("Applied setFormat {n}."),
    };
    let mut history = Vec::new();
    for n in 1..=8 {
        remember(&mut history, turn(n));
    }
    assert_eq!(history.len(), KEPT_TURNS);
    assert_eq!(history[0], turn(3));

    // Even a longer history given whole sends only its last six exchanges.
    let long: Vec<Turn> = (1..=8).map(turn).collect();
    let model = Fake::new(vec![answer("OK.", vec![], 0.001)]);
    send(&model, &long, "and now?", &i, &p).unwrap();
    let input = &model.seen()[0].input;
    assert_eq!(input.len(), 1 + 2 * KEPT_TURNS + 1);
    assert_eq!(
        input[1],
        Item::User {
            text: "ask 3".into()
        }
    );
    let Item::Assistant { text } = &input[2] else {
        panic!("{:?}", input[2]);
    };
    assert!(
        text.contains("reply 3") && text.contains("Applied setFormat 3."),
        "{text}"
    );
    assert_eq!(
        input[2 * KEPT_TURNS - 1],
        Item::User {
            text: "ask 8".into()
        }
    );

    // A plan's turn carries its reply and a note of what came of it.
    let model = Fake::new(vec![answer(
        "Square now.",
        vec![square_call("call_1")],
        0.001,
    )]);
    let out = send(&model, &[], "make it square", &i, &p).unwrap();
    let t = out.turn("make it square");
    assert_eq!(
        (t.user.as_str(), t.reply.as_str()),
        ("make it square", "Square now.")
    );
    assert!(t.note.contains("Applied setFormat"), "{}", t.note);
}

#[test]
fn a_refused_call_the_second_answer_lets_go_is_named() {
    let (p, speech) = talk();
    let i = inputs(&p, speech);
    // The cut is refused back, and the second answer says it is done without calling anything.
    let model = Fake::new(vec![
        answer(
            "",
            vec![square_call("call_1"), cut_call("call_2", 12.3, 19.7)],
            0.002,
        ),
        answer("", vec![], 0.002),
    ]);
    let out = send(&model, &[], "square, and cut the boring bit", &i, &p).unwrap();
    assert!(out.compiled.additions.is_some());
    assert!(
        out.reply
            .contains("I couldn't find that part, so I left it out."),
        "{}",
        out.reply
    );
    assert!(!out.reply.contains("Done"), "{}", out.reply);
    assert!(out.note.contains("Not done: cutRange."), "{}", out.note);

    // The model explains after the refusal, without a call: its words were written knowing, so they stay.
    let model = Fake::new(vec![
        answer("", vec![cut_call("call_1", 12.3, 19.7)], 0.002),
        answer("Nothing is said after 7.8 s.", vec![], 0.002),
    ]);
    let out = send(&model, &[], "cut the boring bit", &i, &p).unwrap();
    assert!(
        out.reply.starts_with("Nothing is said after 7.8 s."),
        "{}",
        out.reply
    );
    assert!(
        out.reply.contains(
            "I couldn't find that part, so I left the video as it was. Select it on the timeline"
        ),
        "{}",
        out.reply
    );

    // Asking or saying it can't: the model dealt with it, nothing more is said.
    for (name, args) in [
        ("askUser", r#"{"question": "Which part is boring?"}"#),
        ("cannotDo", r#"{"reason": "Nothing there is boring."}"#),
    ] {
        let model = Fake::new(vec![
            answer("", vec![cut_call("call_1", 12.3, 19.7)], 0.002),
            answer("", vec![fc("call_2", name, args)], 0.002),
        ]);
        let out = send(&model, &[], "cut the boring bit", &i, &p).unwrap();
        assert!(!out.reply.contains("couldn't"), "{name}: {}", out.reply);
    }
}

#[test]
fn each_refused_call_is_tracked_not_just_its_tool() {
    let (p, speech) = talk();
    let i = inputs(&p, speech);
    // The first answer cuts the "Um," (kept) and an invented range (refused); the second only repeats the first cut.
    let done = cut_call("call_1", 0.5, 0.8);
    let model = Fake::new(vec![
        answer(
            "",
            vec![done.clone(), cut_call("call_2", 12.3, 19.7)],
            0.002,
        ),
        answer("Cutting the um.", vec![cut_call("call_3", 0.5, 0.8)], 0.002),
    ]);
    let out = send(&model, &[], "cut the um and the boring bit", &i, &p).unwrap();
    assert_eq!(out.calls, vec![parsed(&done)]);
    assert!(out.compiled.proposal.is_some());
    assert!(
        out.reply.contains("I couldn't find that part"),
        "{}",
        out.reply
    );
    assert!(out.note.contains("Not done: cutRange."), "{}", out.note);
}

#[test]
fn a_later_answer_does_not_redo_a_call_already_made() {
    let (p, speech) = talk();
    let i = inputs(&p, speech);
    let square = square_call("call_1");
    let fixed = cut_call("call_4", 7.9, 29.8);
    // The format was done in the first answer: the same setFormat again is dropped, and told it is done.
    let model = Fake::new(vec![
        answer(
            "",
            vec![square.clone(), cut_call("call_2", 12.3, 19.7)],
            0.002,
        ),
        answer("", vec![square_call("call_3"), fixed.clone()], 0.002),
    ]);
    let out = send(&model, &[], "square, and cut the rest", &i, &p).unwrap();
    assert_eq!(out.calls, vec![parsed(&square), parsed(&fixed)]);
    assert_eq!(output_for(&model.seen()[2], "call_3"), DONE_OUTPUT);

    // A tool both done and refused in the first answer may be called again.
    let done = cut_call("call_1", 0.5, 0.8);
    let model = Fake::new(vec![
        answer(
            "",
            vec![done.clone(), cut_call("call_2", 12.3, 19.7)],
            0.002,
        ),
        answer("", vec![done.clone(), fixed.clone()], 0.002),
    ]);
    let out = send(&model, &[], "cut the um and the rest", &i, &p).unwrap();
    assert_eq!(out.calls, vec![parsed(&done), parsed(&fixed)]);
    assert_eq!(out.compiled.refusals, vec![]);
}

#[test]
fn an_empty_exchange_sends_no_empty_reply() {
    let (p, speech) = talk();
    let i = inputs(&p, speech);
    let history = vec![Turn {
        user: "hello?".into(),
        reply: " ".into(),
        note: String::new(),
    }];
    let model = Fake::new(vec![answer("OK.", vec![], 0.001)]);
    send(&model, &history, "and now?", &i, &p).unwrap();
    let input = &model.seen()[0].input;
    assert_eq!(input.len(), 3, "{input:?}");
    assert_eq!(
        input[1],
        Item::User {
            text: "hello?".into()
        }
    );
    assert!(
        !input
            .iter()
            .any(|item| matches!(item, Item::Assistant { .. }))
    );
}

#[test]
fn a_picture_being_drawn_is_said_to_be_on_its_way() {
    let (p, speech) = talk();
    let i = inputs(&p, speech);
    // No text from the model: the reply says the work has started, not that nothing changed.
    let sticker = fc(
        "call_1",
        "makeSticker",
        r#"{"prompt": "paper plane", "position": "top-right", "atSec": 0.0, "durationSec": null}"#,
    );
    let model = Fake::new(vec![answer("", vec![sticker], 0.002)]);
    let out = send(&model, &[], "a paper plane sticker in the corner", &i, &p).unwrap();
    assert_eq!(out.compiled.jobs.len(), 1);
    assert_eq!(
        (&out.compiled.additions, &out.compiled.refusals),
        (&None, &vec![])
    );
    assert_eq!(out.reply, "Working on it.");
}

#[test]
fn following_or_blurring_alone_is_said_to_be_on_its_way() {
    // Like a picture being drawn, a subject being followed or blurred is work for a runner: under way, not nothing.
    let (p, speech) = talk();
    let i = inputs(&p, speech);
    for (tool, kind) in [("followSubject", "follow"), ("blurObjects", "blur")] {
        let job = fc("call_1", tool, r#"{"description": "face"}"#);
        let model = Fake::new(vec![answer("", vec![job], 0.002)]);
        let out = send(&model, &[], "keep it on her face", &i, &p).unwrap();
        assert_eq!(out.compiled.jobs.len(), 1, "{kind}");
        assert_eq!(out.compiled.additions, None, "{kind}");
        assert_eq!(out.reply, "Working on it.", "{kind}");
        // With words of its own, the model's text is the reply.
        let job = fc("call_1", tool, r#"{"description": "face"}"#);
        let model = Fake::new(vec![answer("On it.", vec![job], 0.002)]);
        let out = send(&model, &[], "keep it on her face", &i, &p).unwrap();
        assert_eq!(out.reply, "On it.", "{kind}");
    }
}

#[test]
fn a_follow_that_comes_to_nothing_has_a_reply_without_it() {
    // A follow runs once the plan is made, so the model is told it is done and its closing words may claim it: what
    // to say should it come to nothing is what else went through, plainly; nothing when there is nothing else.
    let (p, speech) = talk();
    let i = inputs(&p, speech);
    let follow = || {
        fc(
            "call_2",
            "followSubject",
            r#"{"description": "person", "focus": null}"#,
        )
    };
    let model = Fake::new(vec![
        answer("", vec![square_call("call_1"), follow()], 0.002),
        answer("Done. It is square and keeps her in frame.", vec![], 0.001),
    ]);
    let out = send(&model, &[], "square, and keep her in frame", &i, &p).unwrap();
    assert_eq!(out.reply, "Done. It is square and keeps her in frame.");
    assert_eq!(
        out.without_follow.as_deref(),
        Some("The format is changed.")
    );
    let model = Fake::new(vec![
        answer("", vec![follow()], 0.002),
        answer("Done. She stays in frame.", vec![], 0.001),
    ]);
    let out = send(&model, &[], "keep her in frame", &i, &p).unwrap();
    assert_eq!(out.without_follow.as_deref(), Some(""));
    // No follow: nothing to say instead.
    let model = Fake::new(vec![
        answer("", vec![square_call("call_1")], 0.002),
        answer("Done.", vec![], 0.001),
    ]);
    let out = send(&model, &[], "square", &i, &p).unwrap();
    assert_eq!(out.without_follow, None);
}

#[test]
fn a_part_refused_for_the_person_is_not_claimed_by_the_models_text() {
    // There is no pause to remove (no loudness), the music goes in: the model's sentence claims both, so it is left
    // out and what went through is said plainly, then why the rest wasn't done.
    let (p, speech) = talk();
    let mut i = inputs(&p, speech);
    let lofi = serde_json::from_value(serde_json::json!({
        "id": "lofi", "kind": "audio", "path": "/media/lofi.mp3", "name": "lofi.mp3", "durationUs": 60_000_000,
        "width": 0, "height": 0, "fps": 0.0, "hasAudio": true,
    }))
    .unwrap();
    i.library.items.push(LibraryItem {
        name: "lofi.mp3".into(),
        kind: MediaKind::Music,
        asset: lofi,
        pack: false,
        tags: Vec::new(),
    });
    let music = |id: &str| {
        fc(
            id,
            "addMusic",
            r#"{"file": "lofi", "volume": null, "duck": null, "atSec": 0}"#,
        )
    };
    let model = Fake::new(vec![answer(
        "Removed the long pauses and added the lofi music.",
        vec![
            fc("call_1", "removeSilences", r#"{"minPauseSec": 1}"#),
            music("call_2"),
        ],
        0.002,
    )]);
    // The model closes with no words (the fake's end), so the reply is built from them.
    let out = send(&model, &[], "cut the pauses and add lofi", &i, &p).unwrap();
    assert_eq!(model.seen().len(), 2);
    assert!(out.compiled.additions.is_some());
    assert_eq!(
        out.reply,
        "The music is in. There are no pauses longer than 1.0 s to remove."
    );
    // With nothing refused for the person, the model's own words stay.
    let model = Fake::new(vec![answer(
        "Added the lofi music.",
        vec![music("call_1")],
        0.002,
    )]);
    let out = send(&model, &[], "lofi music", &i, &p).unwrap();
    assert_eq!(out.reply, "Added the lofi music.");
    // A format the video already has changed nothing: it isn't said as done.
    let model = Fake::new(vec![answer(
        "Made it landscape, cut the pauses and added the music.",
        vec![
            fc(
                "call_1",
                "setFormat",
                r#"{"format": "landscape", "keepOthers": true}"#,
            ),
            fc("call_2", "removeSilences", r#"{"minPauseSec": 1}"#),
            music("call_3"),
        ],
        0.002,
    )]);
    let out = send(&model, &[], "landscape, no pauses, lofi", &i, &p).unwrap();
    assert_eq!(out.compiled.unchanged, vec![0]);
    assert_eq!(
        out.reply,
        "The music is in. There are no pauses longer than 1.0 s to remove."
    );
}

#[test]
fn a_media_call_refused_twice_is_named_by_what_it_does() {
    let (p, speech) = talk();
    let i = inputs(&p, speech);
    // The library is empty: no whoosh to add, in either answer.
    let whoosh = |id: &str| fc(id, "addSound", r#"{"name": "whoosh", "atSec": [0.5]}"#);
    let model = Fake::new(vec![
        answer("Adding a whoosh.", vec![whoosh("call_1")], 0.002),
        answer("Adding a whoosh.", vec![whoosh("call_2")], 0.002),
    ]);
    let out = send(&model, &[], "a whoosh at the start", &i, &p).unwrap();
    assert_eq!(
        out.reply,
        "I couldn't add the sound, so I left the video as it was. Try asking in other words."
    );
    let seen = model.seen();
    let output = output_for(&seen[1], "call_1");
    assert!(
        output.starts_with(
            "addSound: no sound in Library is called \"whoosh\". Library has no sound."
        ),
        "{output}"
    );
}

/// "talk.mov" as two clips, the first 1 s short of the file's end, with a 2 s dissolve into a b-roll clip that has 2 s
/// of footage before its in point: at 2x the first clip's spare footage halves, so the dissolve shortens to 1 s.
fn with_dissolve() -> Project {
    let (mut p, _) = talk();
    let mut broll = p.assets[0].clone();
    broll.id = "broll".into();
    broll.name = "broll.mov".into();
    broll.has_audio = false;
    p.assets.push(broll);
    p.timeline.main[0].source_out_us = secs(29.0);
    let mut b = Clip::new(
        ClipSource::Asset { id: "broll".into() },
        secs(2.0),
        secs(12.0),
    );
    b.id = "b".into();
    p.timeline.main.push(b);
    let dissolve = klipolo_model::timeline::Transition {
        effect: klipolo_model::contrib::builtin::TRANSITION_DISSOLVE.into(),
        duration_us: secs(2.0),
        params: Default::default(),
    };
    let set = klipolo_model::Op::SetTransition {
        clip_id: p.timeline.main[0].id.clone(),
        transition: Some(dissolve),
    };
    klipolo_model::apply_ops(&p, &[set]).unwrap()
}

#[test]
fn the_reply_says_what_a_new_speed_did_to_a_transition_after_the_models_words() {
    let p = with_dissolve();
    let i = inputs(&p, talk().1);
    let speed = || fc("s", "setClip", r#"{"clip": "c1", "speed": 2}"#);
    let model = Fake::new(vec![answer("Sped up the talk.", vec![speed()], 0.001)]);
    let out = send(&model, &[], "speed up the talk 2x", &i, &p).unwrap();
    assert!(out.compiled.additions.is_some());
    assert_eq!(out.reply, "Sped up the talk. A transition is shortened.");
    // With no words of the model's, after the short sentence.
    let model = Fake::new(vec![answer("", vec![speed()], 0.001)]);
    let out = send(&model, &[], "speed up the talk 2x", &i, &p).unwrap();
    assert_eq!(out.reply, "Done. A transition is shortened.");
}

fn find_call(call_id: &str) -> FunctionCall {
    fc(
        call_id,
        "findMoments",
        r#"{"query": "the module", "ranges": [{"startSec": 0.0, "endSec": 30.0, "label": "the whole talk"}]}"#,
    )
}

/// `i` with a pack picture (Star), sound (Pop) and song (Soft ambient) in its library.
fn with_pack(mut i: Inputs) -> Inputs {
    let item = |id: &str, name: &str, kind: MediaKind| {
        let picture = kind == MediaKind::Image;
        LibraryItem {
            name: name.into(),
            kind,
            asset: serde_json::from_value(serde_json::json!({
                "id": id, "kind": if picture { "image" } else { "audio" }, "path": format!("/pack/{id}"),
                "name": name, "durationUs": 60_000_000, "width": if picture { 512 } else { 0 },
                "height": if picture { 512 } else { 0 }, "fps": 0.0, "hasAudio": !picture,
            }))
            .unwrap(),
            pack: true,
            tags: Vec::new(),
        }
    };
    i.library.items.extend([
        item("klipolo-pack-star", "Star", MediaKind::Image),
        item("klipolo-pack-pop", "Pop", MediaKind::Sound),
        item("klipolo-pack-ambient", "Soft ambient", MediaKind::Music),
    ]);
    i
}

#[test]
fn a_short_is_a_look_then_one_batch_then_words() {
    let (p, speech) = talk();
    let i = with_pack(inputs(&p, speech));
    // Once it knows the moments, the whole plan in one answer: three model calls for the whole short.
    let batch = vec![
        fc(
            "call_2",
            "keepOnly",
            r#"{"ranges": [{"startSec": 0.5, "endSec": 7.8}]}"#,
        ),
        fc(
            "call_3",
            "setFormat",
            r#"{"format": "portrait", "keepOthers": true}"#,
        ),
        fc(
            "call_4",
            "followSubject",
            r#"{"description": "person", "focus": null}"#,
        ),
        fc(
            "call_5",
            "setCaptions",
            r#"{"on": true, "style": "word-pop"}"#,
        ),
        fc(
            "call_6",
            "addText",
            r#"{"text": "the new module", "preset": "title", "atSec": 0.5, "durationSec": 2.0, "animation": "kinetic", "font": null}"#,
        ),
        fc(
            "call_7",
            "addImage",
            r#"{"file": "Star", "size": "medium", "position": "top-right", "atSec": 3.0, "durationSec": 2.0}"#,
        ),
        fc(
            "call_8",
            "animate",
            r#"{"layer": "last", "enter": "pop", "exit": null, "emphasis": null, "sound": "Pop"}"#,
        ),
        fc(
            "call_9",
            "addMusic",
            r#"{"file": "Soft ambient", "volume": 0.15, "duck": true, "atSec": null}"#,
        ),
    ];
    let model = Fake::new(vec![
        answer("", vec![find_call("call_1")], 0.001),
        answer("", batch, 0.002),
        answer(
            "Here is your vertical short. Review the cut below.",
            vec![],
            0.001,
        ),
    ]);
    let out = send(&model, &[], "make a 60 s short with animations", &i, &p).unwrap();
    assert_eq!(model.seen().len(), 3);
    assert_eq!(out.compiled.refusals, vec![], "{:?}", out.compiled.refusals);
    let changes: Vec<&str> = out
        .compiled
        .changes
        .iter()
        .map(|c| c.code.as_str())
        .collect();
    assert_eq!(
        changes,
        [
            "chat.setFormat",
            "chat.setCaptions",
            "chat.addText",
            "chat.addImage",
            "chat.addMusic"
        ]
    );
    // The sticker pops in as Klipolo times a short's stickers: the model's animate of it changes nothing more.
    assert!(
        out.compiled.unchanged.contains(&7),
        "{:?}",
        out.compiled.unchanged
    );
    assert!(out.compiled.proposal.is_some(), "the cut is proposed");
    assert_eq!(out.compiled.jobs.len(), 1, "the follow");
    assert_eq!(
        out.reply,
        "Here is your vertical short. Review the cut below."
    );
    // A short whose model goes one call a step still has room: the steps are a safety net, not the plan.
    assert_eq!(MAX_STEPS, 10);
}

#[test]
fn the_model_goes_on_after_each_step_until_it_answers_in_words() {
    let (p, speech) = talk();
    let i = inputs(&p, speech);
    // As a short is made: a look, then a cut, then the style, then the closing words.
    let model = Fake::new(vec![
        answer("", vec![find_call("call_1")], 0.001),
        answer(
            "I'll shape it into a short.",
            vec![cut_call("call_2", 7.8, 30.0)],
            0.001,
        ),
        answer(
            "",
            vec![
                square_call("call_3"),
                fc(
                    "call_4",
                    "setCaptions",
                    r#"{"on": true, "style": "word-pop"}"#,
                ),
            ],
            0.001,
        ),
        answer(
            "Made a square short with captions. Review the cut below.",
            vec![],
            0.001,
        ),
        answer("Never asked for.", vec![], 0.001),
    ]);
    let out = send(&model, &[], "make a short", &i, &p).unwrap();
    let seen = model.seen();
    assert_eq!(seen.len(), 4);
    assert_eq!(model.left(), 1);
    // Each request carries the one before whole, then what came of each call of the last answer.
    for k in 1..4 {
        assert_eq!(
            &seen[k].input[..seen[k - 1].input.len()],
            &seen[k - 1].input[..]
        );
    }
    assert!(output_for(&seen[1], "call_1").contains("0.0-30.0"));
    assert_eq!(output_for(&seen[2], "call_2"), PROPOSED_OUTPUT);
    assert_eq!(output_for(&seen[3], "call_3"), DONE_OUTPUT);
    assert_eq!(output_for(&seen[3], "call_4"), DONE_OUTPUT);
    // One additions step, one proposal, and the moments still shown.
    let additions = out.compiled.additions.as_ref().expect("additions");
    assert_eq!(additions.label, "chat.andMore");
    assert!(out.compiled.proposal.is_some());
    assert_eq!(out.compiled.moments.len(), 1);
    assert_eq!(out.calls.len(), 4);
    // Its closing words are the reply.
    assert_eq!(
        out.reply,
        "Made a square short with captions. Review the cut below."
    );
    assert_eq!(out.answers.len(), 4);
    assert!((out.usage_usd - 0.004).abs() < 1e-12);
}

#[test]
fn a_step_that_fails_keeps_what_was_done_and_says_it_stopped() {
    let (p, speech) = talk();
    let i = inputs(&p, speech);
    let model = Fake::new(vec![
        answer("Making it square.", vec![square_call("call_1")], 0.002),
        Err(ModelError::Budget(String::new())),
    ]);
    let out = send(&model, &[], "make a square short", &i, &p).unwrap();
    assert_eq!(model.seen().len(), 2);
    assert!(out.compiled.additions.is_some(), "what was done stands");
    assert_eq!(
        out.reply,
        format!("Making it square. {OUT_OF_CREDIT} {STOPPED}")
    );
    assert!(out.note.contains("Stopped early"), "{}", out.note);
    assert!((out.usage_usd - 0.002).abs() < 1e-12);
    // A first call that fails is the message's error: nothing was done.
    let model = Fake::new(vec![Err(ModelError::Budget(String::new()))]);
    let e = send(&model, &[], "make a square short", &i, &p).unwrap_err();
    assert_eq!(e.kind, ChatErrorKind::Budget);
}

#[test]
fn a_promise_without_calls_is_asked_to_go_on() {
    let (p, speech) = talk();
    let i = inputs(&p, speech);
    let model = Fake::new(vec![
        answer("I will turn this into a square short.", vec![], 0.002),
        answer("", vec![square_call("call_1")], 0.002),
        answer("Made it square.", vec![], 0.002),
    ]);
    let out = send(&model, &[], "make a square short", &i, &p).unwrap();
    assert_eq!(model.seen().len(), 3);
    let second = &model.seen()[1];
    assert!(
        matches!(second.input.last(), Some(Item::User { text }) if text == klipolo_chat::GO_ON),
        "{:?}",
        second.input.last()
    );
    assert!(out.compiled.additions.is_some());
    assert_eq!(out.reply, "Made it square.");
    // An answer that is done says so without a promise: one call.
    let model = Fake::new(vec![answer(
        "This video is 30 seconds long.",
        vec![],
        0.002,
    )]);
    let out = send(&model, &[], "how long is it", &i, &p).unwrap();
    assert_eq!(model.seen().len(), 1);
    assert_eq!(out.reply, "This video is 30 seconds long.");
}

#[test]
fn a_message_never_takes_more_than_max_steps_model_calls() {
    let (p, speech) = talk();
    let i = inputs(&p, speech);
    // Each answer another look: the steps run out, and what was found is shown.
    let looks: Vec<_> = (0..MAX_STEPS + 2)
        .map(|k| {
            let look = fc(
                &format!("call_{k}"),
                "findMoments",
                &format!(r#"{{"query": "part {k}", "ranges": [{{"startSec": 0.0, "endSec": 30.0, "label": "a part"}}]}}"#),
            );
            answer("Looking again.", vec![look], 0.001)
        })
        .collect();
    let model = Fake::new(looks);
    let out = send(&model, &[], "find the module part", &i, &p).unwrap();
    assert_eq!(model.seen().len(), MAX_STEPS);
    assert_eq!(model.left(), 2);
    assert!(!out.compiled.moments.is_empty());
}

#[test]
fn an_answer_that_only_repeats_its_calls_ends_the_message() {
    let (p, speech) = talk();
    let i = inputs(&p, speech);
    let model = Fake::new(vec![
        answer("Here is the module.", vec![find_call("call_1")], 0.001),
        answer("Here it is.", vec![find_call("call_2")], 0.001),
        answer("And again.", vec![find_call("call_3")], 0.001),
    ]);
    let out = send(&model, &[], "find the module part", &i, &p).unwrap();
    assert_eq!(model.seen().len(), 2);
    assert_eq!(out.compiled.moments.len(), 1, "shown once");
    assert_eq!(out.reply, "Here it is.");
    assert!((out.usage_usd - 0.002).abs() < 1e-12, "both calls count");
}

#[test]
fn closing_words_are_followed_once_by_what_the_person_has_to_do() {
    let (p, speech) = talk();
    let mut i = inputs(&p, speech);
    i.analysis.insert("talk".into(), AnalysisState::NotStarted);
    let calls = || {
        vec![
            square_call("call_1"),
            cut_call("call_2", 7.8, 30.0),
            fc("call_3", "removeFillers", "{}"),
        ]
    };
    // The ums wait for analysis: the line that says so follows the model's closing words, which didn't say it.
    let model = Fake::new(vec![
        answer(
            "Making it square, cutting the end and the ums.",
            calls(),
            0.002,
        ),
        answer(
            "Made a square version and marked the end to cut.",
            vec![],
            0.001,
        ),
    ]);
    let out = send(&model, &[], "square, cut the end and the ums", &i, &p).unwrap();
    assert_eq!(output_for(&model.seen()[1], "call_3"), TOLD_PERSON);
    assert_eq!(
        out.reply,
        format!("Made a square version and marked the end to cut. {ANALYZE_FIRST}.")
    );
    // Closing words that say it already are not followed by it again.
    let model = Fake::new(vec![
        answer(
            "Making it square, cutting the end and the ums.",
            calls(),
            0.002,
        ),
        answer(
            &format!("Made it square and marked the end to cut. {ANALYZE_FIRST}."),
            vec![],
            0.001,
        ),
    ]);
    let out = send(&model, &[], "square, cut the end and the ums", &i, &p).unwrap();
    assert_eq!(out.reply.matches(ANALYZE_FIRST).count(), 1, "{}", out.reply);
}

#[test]
fn the_model_is_asked_to_close_on_what_was_done_and_never_promise_more() {
    let (p, speech) = talk();
    let i = inputs(&p, speech);
    let model = Fake::new(vec![
        answer("", vec![square_call("call_1")], 0.001),
        answer("Made it square.", vec![], 0.001),
    ]);
    send(&model, &[], "make it square", &i, &p).unwrap();
    let rule = concat!(
        "Your last words say what you did and what waits for their approval; ",
        "never promise more work later"
    );
    for request in model.seen() {
        assert!(request.system.contains(rule), "{}", request.system);
    }
}

#[test]
fn clearing_transitions_that_are_not_there_is_silent() {
    let (p, speech) = talk();
    let i = inputs(&p, speech);
    let model = Fake::new(vec![
        answer(
            "",
            vec![
                square_call("call_1"),
                fc(
                    "call_2",
                    "setTransition",
                    r#"{"after": "all", "kind": "none", "durationSec": null}"#,
                ),
            ],
            0.001,
        ),
        answer("Made it square.", vec![], 0.001),
    ]);
    let out = send(&model, &[], "make a square short", &i, &p).unwrap();
    assert_eq!(output_for(&model.seen()[1], "call_2"), DONE_OUTPUT);
    assert_eq!(out.compiled.refusals, vec![]);
    assert_eq!(out.reply, "Made it square.");
}

#[test]
fn a_corrective_keep_only_replaces_the_one_before() {
    let (p, speech) = talk();
    let i = inputs(&p, speech);
    let keep = |id: &str, ranges: &str| fc(id, "keepOnly", &format!(r#"{{"ranges": {ranges}}}"#));
    // First the module part (0.9 to 4.0 s), then, corrected, that and the end (6.3 to 7.8 s).
    let model = Fake::new(vec![
        answer(
            "",
            vec![keep("call_1", r#"[{"startSec": 0.9, "endSec": 4.0}]"#)],
            0.001,
        ),
        answer(
            "",
            vec![keep(
                "call_2",
                r#"[{"startSec": 0.9, "endSec": 4.0}, {"startSec": 6.3, "endSec": 7.8}]"#,
            )],
            0.001,
        ),
        answer("Kept the module part and the end.", vec![], 0.001),
    ]);
    let out = send(&model, &[], "keep the module part and the end", &i, &p).unwrap();
    assert_eq!(out.compiled.refusals, vec![]);
    assert_eq!(
        out.compiled.proposed_calls,
        vec![1],
        "the later one stands alone"
    );
    let proposal = out.compiled.proposal.as_ref().expect("the cut");
    let kept = klipolo_model::query::video_duration(&proposal.review.after.timeline);
    // Both parts, each snapped to the words it stands on: more than the first part alone (about 3.4 s).
    assert!((secs(4.4)..secs(5.3)).contains(&kept), "{kept}");
}

#[test]
fn a_short_places_its_sticker_and_sounds_on_the_words_said() {
    use klipolo_model::query::video_duration;
    use klipolo_model::timeline::{ItemContent, TrackKind};
    let (p, speech) = talk();
    let i = with_pack(inputs(&p, speech));
    // As the recipe asks: the cut, then a sticker on the word that names a thing, popping in with its sound, and a
    // ding on the key point, each by the words said there. The ding's first quote is misheard and comes back with
    // what is said most like it; the next step quotes it as said.
    let batch = vec![
        fc(
            "call_2",
            "keepOnly",
            r#"{"ranges": [{"startSec": 2.9, "endSec": 7.8}]}"#,
        ),
        fc(
            "call_3",
            "addImage",
            r#"{"file": "Star", "size": "medium", "position": "top-right", "atSec": null, "atWords": "the new module", "durationSec": 2.0}"#,
        ),
        fc(
            "call_4",
            "animate",
            r#"{"layer": "last", "enter": "pop", "exit": null, "emphasis": null, "sound": "Pop"}"#,
        ),
        fc(
            "call_5",
            "addSound",
            r#"{"name": "Ding", "atWords": ["a long tme"], "atSec": null}"#,
        ),
    ];
    let fixed = fc(
        "call_6",
        "addSound",
        r#"{"name": "Ding", "atWords": ["a long time."], "atSec": null}"#,
    );
    let model = Fake::new(vec![
        answer("", vec![find_call("call_1")], 0.001),
        answer("", batch, 0.002),
        answer("", vec![fixed], 0.001),
        answer("Here is your short. Review the cut below.", vec![], 0.001),
    ]);
    let mut i = i;
    let ding = serde_json::from_value(serde_json::json!({
        "id": "klipolo-pack-ding", "kind": "audio", "path": "/pack/ding", "name": "Ding", "durationUs": 800_000,
        "width": 0, "height": 0, "fps": 0.0, "hasAudio": true,
    }))
    .unwrap();
    i.library.items.push(LibraryItem {
        name: "Ding".into(),
        kind: MediaKind::Sound,
        asset: ding,
        pack: true,
        tags: Vec::new(),
    });
    let out = send(&model, &[], "make a short with stickers and sounds", &i, &p).unwrap();
    let seen = model.seen();
    assert_eq!(seen.len(), 4);
    // The tools offer the words said there, and the prompt asks for them.
    let tools_text = serde_json::to_string(&seen[0].tools).unwrap();
    assert!(tools_text.contains("atWords"), "{tools_text}");
    assert!(seen[0].system.contains("atWords"));
    assert_eq!(
        output_for(&seen[2], "call_5"),
        "addSound: atWords[0] \"a long tme\" is not said in the video. Said most like it: \"a long time.\" (6.7 s). \
         Quote 1 to 4 words exactly as Transcript shows them."
    );
    assert_eq!(out.compiled.refusals, vec![], "{:?}", out.compiled.refusals);
    // In the short as the cut leaves it (from "the" at 2.9 s, padded), the sticker starts on "the" with its pop, and
    // the ding on "a".
    let short = &out
        .compiled
        .proposal
        .as_ref()
        .expect("the cut")
        .review
        .after;
    let from = short.timeline.main[0].source_in_us;
    let starts = |kind: TrackKind, asset: Option<&str>| -> Vec<Us> {
        (short.timeline.tracks.iter())
            .filter(|t| t.kind == kind)
            .flat_map(|t| t.items.iter())
            .filter(|item| match (&item.content, asset) {
                (ItemContent::Audio { asset_id, .. }, Some(a)) => asset_id == a,
                (_, None) => true,
                _ => false,
            })
            .map(|item| item.start_us)
            .collect()
    };
    // The sticker from just before its word, as a short's are, with its pop.
    let sticker = (secs(2.8).max(from)) - from;
    assert_eq!(starts(TrackKind::Overlay, None), [sticker]);
    assert_eq!(
        starts(TrackKind::Audio, Some("klipolo-pack-pop")),
        [sticker]
    );
    assert_eq!(
        starts(TrackKind::Audio, Some("klipolo-pack-ding")),
        [secs(6.7) - from]
    );
    assert!(video_duration(&short.timeline) < secs(6.0));
}

#[test]
fn an_answer_cut_off_keeps_its_finished_calls_and_is_asked_again_once() {
    let (p, speech) = talk();
    let i = inputs(&p, speech);
    let cut_off = |calls: Vec<FunctionCall>| {
        Err(ModelError::CutOff(ModelResponse {
            text: String::new(),
            calls,
            usage_usd: 0.004,
        }))
    };
    // Its finished cut is kept and proposed; the model is told it was cut off and finishes in the next step.
    let model = Fake::new(vec![
        cut_off(vec![cut_call("call_1", 7.9, 29.8)]),
        answer("I proposed the cut.", vec![], 0.001),
    ]);
    let out = send(&model, &[], "cut the end", &i, &p).unwrap();
    let seen = model.seen();
    assert_eq!(seen.len(), 2);
    assert_eq!(output_for(&seen[1], "call_1"), PROPOSED_OUTPUT);
    assert_eq!(
        seen[1].input.last(),
        Some(&Item::User {
            text: klipolo_chat::CUT_OFF.into()
        })
    );
    assert!(out.compiled.proposal.is_some());
    assert_eq!(out.reply, "I proposed the cut.");
    assert!((out.usage_usd - 0.005).abs() < 1e-12, "{}", out.usage_usd);
    // Cut off before any call: asked again all the same.
    let model = Fake::new(vec![
        cut_off(vec![]),
        answer("", vec![cut_call("call_2", 7.9, 29.8)], 0.002),
    ]);
    let out = send(&model, &[], "cut the end", &i, &p).unwrap();
    assert!(out.compiled.proposal.is_some());
    // Its words end at their last whole sentence, should the reply be built from them.
    let model = Fake::new(vec![
        Err(ModelError::CutOff(ModelResponse {
            text: "I proposed cutting the end. Next I will add music at 1.5".into(),
            calls: vec![cut_call("call_3", 7.9, 29.8)],
            usage_usd: 0.004,
        })),
        answer("", vec![], 0.001),
    ]);
    let out = send(&model, &[], "cut the end", &i, &p).unwrap();
    assert!(
        out.reply.starts_with("I proposed cutting the end."),
        "{}",
        out.reply
    );
    assert!(!out.reply.contains("music"), "{}", out.reply);
    let seen = model.seen();
    assert!(
        seen[1].input.contains(&Item::Assistant {
            text: "I proposed cutting the end.".into()
        }),
        "{:?}",
        seen[1].input
    );
    // Cut off again: the message fails, as any failed answer does, billed for both.
    let model = Fake::new(vec![cut_off(vec![]), cut_off(vec![])]);
    let e = send(&model, &[], "cut the end", &i, &p).unwrap_err();
    assert_eq!(e.kind, ChatErrorKind::Other);
    assert_plain(&e.message);
    assert!((e.usage_usd - 0.008).abs() < 1e-12, "{}", e.usage_usd);
    assert_eq!(model.seen().len(), 2);
}

#[test]
fn a_message_that_runs_out_of_steps_says_what_its_edits_did() {
    let (p, speech) = talk();
    let i = with_pack(inputs(&p, speech));
    // One call a step, each answer saying what comes next, until the steps run out mid-way: the reply says what the
    // edits did, never the model's words about what it was still going to do.
    let busy = "Now I'm adding the pop animations and final sounds to finish the short.";
    let steps: Vec<Vec<FunctionCall>> = vec![
        vec![find_call("call_1")],
        vec![
            fc(
                "call_2",
                "keepOnly",
                r#"{"ranges": [{"startSec": 0.5, "endSec": 7.8}]}"#,
            ),
            fc(
                "call_3",
                "setFormat",
                r#"{"format": "portrait", "keepOthers": true}"#,
            ),
        ],
        vec![fc(
            "call_4",
            "setCaptions",
            r#"{"on": true, "style": "word-pop"}"#,
        )],
        vec![fc(
            "call_5",
            "addText",
            r#"{"text": "the new module", "preset": "title", "atSec": 0.5, "atWords": null, "durationSec": 2.0, "animation": "pop", "font": null}"#,
        )],
        vec![fc(
            "call_6",
            "addImage",
            r#"{"file": "Star", "size": "medium", "position": "top-right", "atSec": null, "atWords": "the new module", "durationSec": 2.0}"#,
        )],
        vec![fc(
            "call_7",
            "animate",
            r#"{"layer": "last", "enter": "pop", "exit": null, "emphasis": null, "sound": "Pop"}"#,
        )],
        vec![fc(
            "call_8",
            "addSound",
            r#"{"name": "Pop", "atWords": ["a long time"], "atSec": null}"#,
        )],
        vec![fc(
            "call_9",
            "addMusic",
            r#"{"file": "Soft ambient", "volume": 0.15, "duck": true, "atSec": null}"#,
        )],
        vec![fc(
            "call_10",
            "addImage",
            r#"{"file": "Star", "size": "medium", "position": "top-left", "atSec": null, "atWords": "It took", "durationSec": 2.0}"#,
        )],
        vec![fc(
            "call_11",
            "findMoments",
            r#"{"query": "the end", "ranges": [{"startSec": 6.7, "endSec": 7.8, "label": "the end"}]}"#,
        )],
    ];
    assert_eq!(steps.len(), MAX_STEPS);
    let model = Fake::new(
        steps
            .into_iter()
            .map(|calls| answer(busy, calls, 0.001))
            .collect(),
    );
    let out = send(&model, &[], "make a short with stickers and sounds", &i, &p).unwrap();
    assert_eq!(model.seen().len(), MAX_STEPS);
    assert_eq!(out.compiled.refusals, vec![], "{:?}", out.compiled.refusals);
    assert!(!out.reply.contains("Now I'm adding"), "{}", out.reply);
    // An 8 s short shows one sticker (one per 12 s at most, never under the hook), with its pop; the model's own pop on a
    // word is left out.
    assert!(
        out.reply.starts_with(
            "I made it vertical and added captions, a title, a sticker, a sound and music. Approve the cut to keep \
             the "
        ),
        "{}",
        out.reply
    );
    assert!(out.reply.contains(" s version."), "{}", out.reply);
    assert_plain(&out.reply);
}

#[test]
fn an_undo_partway_through_a_short_never_wipes_it() {
    // As a paid run went: the short's batch, one more layer, its removal, then an undo to take that back, then an
    // animation with optional words written as the text "null". The undo would take back the person's change from
    // before the message and refuse every change of this one; it is refused back instead, and the short stands.
    let (p, speech) = talk();
    let i = with_pack(inputs(&p, speech));
    let steps = vec![
        answer("", vec![find_call("call_1")], 0.001),
        answer(
            "",
            vec![
                fc(
                    "call_2",
                    "keepOnly",
                    r#"{"ranges": [{"startSec": 0.5, "endSec": 7.8}]}"#,
                ),
                fc(
                    "call_3",
                    "setFormat",
                    r#"{"format": "portrait", "keepOthers": true}"#,
                ),
                fc(
                    "call_4",
                    "setCaptions",
                    r#"{"on": true, "style": "word-pop"}"#,
                ),
                fc(
                    "call_5",
                    "addText",
                    r#"{"text": "the new module", "preset": "title", "atSec": 0.5, "durationSec": 2.0, "animation": "kinetic", "font": "null"}"#,
                ),
                fc(
                    "call_6",
                    "addMusic",
                    r#"{"file": "Soft ambient", "volume": 0.15, "duck": true, "atSec": null}"#,
                ),
            ],
            0.002,
        ),
        answer(
            "",
            vec![fc(
                "call_7",
                "addText",
                r#"{"text": "the new module", "preset": "caption", "atSec": 4.0, "durationSec": 2.0, "animation": null, "font": null}"#,
            )],
            0.001,
        ),
        answer(
            "",
            vec![fc(
                "call_8",
                "setLayer",
                r#"{"layer": "last", "set": [{"key": "remove", "value": true}]}"#,
            )],
            0.001,
        ),
        answer("", vec![fc("call_9", "undo", "{}")], 0.001),
        answer(
            "",
            vec![fc(
                "call_10",
                "animate",
                r#"{"layer": "last", "enter": "pop", "exit": "null", "emphasis": "null", "sound": "null"}"#,
            )],
            0.001,
        ),
        answer(
            "Here is your vertical short. Review the cut below.",
            vec![],
            0.001,
        ),
    ];
    let model = Fake::new(steps);
    let out = send(&model, &[], "make a 60 s short with animations", &i, &p).unwrap();
    let seen = model.seen();
    assert_eq!(seen.len(), 7);
    // The undo is refused back with why and what to do instead; nothing else is refused.
    assert_eq!(
        output_for(&seen[5], "call_9"),
        klipolo_chat::UNDO_AFTER_CHANGES
    );
    assert!(!out.compiled.undo);
    let refused: Vec<(usize, &str)> = (out.compiled.refusals.iter())
        .map(|r| (r.call_index, out.calls[r.call_index].name()))
        .collect();
    assert_eq!(refused, vec![], "the undo is never kept: {refused:?}");
    assert!(!out.calls.iter().any(|c| matches!(c, ToolCall::Undo)));
    // The short stands: its look applies, its cut is proposed.
    let changes: Vec<&str> = (out.compiled.changes.iter())
        .map(|c| c.code.as_str())
        .collect();
    for change in [
        "chat.setFormat",
        "chat.setCaptions",
        "chat.addText",
        "chat.addMusic",
    ] {
        assert!(changes.contains(&change), "{changes:?}");
    }
    assert!(out.compiled.proposal.is_some(), "the cut is proposed");
    // The animation's "null" words read as left out: it goes through.
    assert_eq!(output_for(&seen[6], "call_10"), DONE_OUTPUT);
    // The reply says nothing went undone: the refused undo was the model's own, not the person's request.
    assert_eq!(
        out.reply,
        "Here is your vertical short. Review the cut below."
    );
}

#[test]
fn an_undo_the_person_asked_for_after_another_change_is_said_to_be_undone() {
    // "Make it square and undo the music": the model changes the format, then calls undo, which is refused back with
    // how to do it instead. If it never does, the reply says part of it wasn't done.
    let (p, speech) = talk();
    let i = inputs(&p, speech);
    let model = Fake::new(vec![
        answer(
            "",
            vec![
                fc(
                    "call_1",
                    "setFormat",
                    r#"{"format": "square", "keepOthers": true}"#,
                ),
                fc("call_2", "undo", "{}"),
            ],
            0.001,
        ),
        answer("It's square now.", vec![], 0.001),
    ]);
    let out = send(&model, &[], "make it square and undo the music", &i, &p).unwrap();
    assert_eq!(
        output_for(&model.seen()[1], "call_2"),
        klipolo_chat::UNDO_AFTER_CHANGES
    );
    assert!(out.reply.starts_with("It's square now."), "{}", out.reply);
    assert!(out.reply.contains("couldn't"), "{}", out.reply);
}

#[test]
fn an_undo_the_person_asked_for_done_by_taking_the_thing_away_is_not_said_to_be_left_undone() {
    // "Make it square and undo the picture": the undo is refused back after the format change, and the model takes the
    // picture away with setLayer, as the refusal says. Nothing is left undone: the reply never says part of it wasn't.
    let (mut p, speech) = talk();
    let picture = Asset {
        id: "logo".into(),
        kind: AssetKind::Image,
        path: "/media/logo.png".into(),
        name: "logo.png".into(),
        duration_us: 0,
        width: 512,
        height: 512,
        fps: 0.0,
        rotation: None,
        has_audio: false,
        audio_lead_us: None,
        made_with: None,
    };
    let ops = klipolo_model::media::image::add_image_ops(&p, &picture, 0).unwrap();
    p = klipolo_model::apply_ops(&p, &ops).unwrap();
    let i = inputs(&p, speech);
    let model = Fake::new(vec![
        answer(
            "",
            vec![
                fc(
                    "call_1",
                    "setFormat",
                    r#"{"format": "square", "keepOthers": true}"#,
                ),
                fc("call_2", "undo", "{}"),
            ],
            0.001,
        ),
        answer(
            "",
            vec![fc(
                "call_3",
                "setLayer",
                r#"{"layer": "i1", "set": [{"key": "remove", "value": true}]}"#,
            )],
            0.001,
        ),
        answer(
            "It's square, and the picture goes once you apply the change.",
            vec![],
            0.001,
        ),
    ]);
    let out = send(&model, &[], "make it square and undo the picture", &i, &p).unwrap();
    assert_eq!(
        output_for(&model.seen()[1], "call_2"),
        klipolo_chat::UNDO_AFTER_CHANGES
    );
    assert!(
        out.compiled.proposal.is_some(),
        "the picture's removal is proposed"
    );
    assert_eq!(
        out.reply,
        "It's square, and the picture goes once you apply the change."
    );
}

/// A short's cut, 9:16 version and captions, as the recipe makes them on the talk.
fn short_calls() -> Vec<FunctionCall> {
    vec![
        fc(
            "call_1",
            "keepOnly",
            r#"{"ranges": [{"startSec": 0.5, "endSec": 7.8}]}"#,
        ),
        fc(
            "call_2",
            "setFormat",
            r#"{"format": "portrait", "keepOthers": true}"#,
        ),
        fc(
            "call_3",
            "setCaptions",
            r#"{"on": true, "style": "word-pop"}"#,
        ),
    ]
}

#[test]
fn a_shorts_styling_refused_back_is_never_said_as_left_undone() {
    // The short is made; an animation of a layer that isn't there is refused back and never made good. Klipolo's own
    // rules decide a short's styling, so the reply says nothing went undone.
    let (p, speech) = talk();
    let i = inputs(&p, speech);
    let mut calls = short_calls();
    calls.push(fc(
        "call_4",
        "animate",
        r#"{"layer": "i7", "enter": "pop", "exit": null, "emphasis": null, "sound": null}"#,
    ));
    let model = Fake::new(vec![
        answer("", calls, 0.002),
        answer(
            "Here is your vertical short. Review the cut below.",
            vec![],
            0.001,
        ),
    ]);
    let out = send(
        &model,
        &[],
        "make a 30 second short with nice animations",
        &i,
        &p,
    )
    .unwrap();
    assert_ne!(output_for(&model.seen()[1], "call_4"), DONE_OUTPUT);
    assert!(out.compiled.short);
    assert_eq!(
        out.reply,
        "Here is your vertical short. Review the cut below."
    );
}

#[test]
fn a_sound_the_person_named_that_fails_in_a_short_is_still_said() {
    let (p, speech) = talk();
    let i = inputs(&p, speech);
    let mut calls = short_calls();
    calls.push(fc(
        "call_4",
        "addSound",
        r#"{"name": "Cha ching", "atSec": [1.0], "atWords": null}"#,
    ));
    let model = Fake::new(vec![
        answer("", calls, 0.002),
        answer("Here is your vertical short.", vec![], 0.001),
    ]);
    let out = send(
        &model,
        &[],
        "make a 30 second short with a cha ching sound at the start",
        &i,
        &p,
    )
    .unwrap();
    assert!(
        out.reply.contains("I couldn't add the sound"),
        "{}",
        out.reply
    );
}

#[test]
fn a_refused_call_is_named_never_said_as_part_of_this() {
    // Outside a short, an animation refused back and never made good is said by what it does.
    let (p, speech) = talk();
    let i = inputs(&p, speech);
    let model = Fake::new(vec![
        answer(
            "",
            vec![fc(
                "call_1",
                "animate",
                r#"{"layer": "i7", "enter": "pop", "exit": null, "emphasis": null, "sound": null}"#,
            )],
            0.001,
        ),
        answer("Done.", vec![], 0.001),
    ]);
    let out = send(&model, &[], "animate the picture", &i, &p).unwrap();
    assert!(!out.reply.contains("part of this"), "{}", out.reply);
    assert!(
        out.reply.contains("I couldn't animate the layer"),
        "{}",
        out.reply
    );
}
