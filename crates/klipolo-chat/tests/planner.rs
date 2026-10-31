//! The planner: one chat message to the language model, and at most one refusal round. The model is a fake
//! that hands out scripted answers and keeps every request it was sent, so the tests read exactly what a provider
//! would get. The project is `talk()`: "talk.mov", 30 s, played whole, its words from 0.5 to 7.8 s ("Um, so today we
//! look at the new module. It took [pause 1.5 s] uh a long time."), then nothing is said, so a cut from 12.3 to 19.7 s
//! stands on nothing and is refused back to the model.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use klipolo_chat::{
    AnalysisState, Audience, BoxFuture, ChatContext, ChatError, ChatErrorKind, FunctionCall,
    Inputs, Item, KEPT_TURNS, LanguageModel, Library, LibraryItem, MAX_CONTEXT_TOKENS, MediaKind,
    ModelError, ModelRequest, ModelResponse, PROPOSED_OUTPUT, PlanOut, Refs, STILL_LISTENING,
    TOLD_PERSON, TOO_LONG, ToolCall, Turn, View, build_context, plan, remember, sanitize,
    system_prompt, tools,
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
        message: String::new(),
        view: View::default(),
        refs: Refs::of(project),
        speech,
        scenes: HashMap::new(),
        analysis: (project.assets.iter())
            .map(|a| (a.id.clone(), AnalysisState::Done))
            .collect(),
        library: Library::default(),
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
        let answer = (self.answers.lock().unwrap().pop_front())
            .unwrap_or_else(|| Err(ModelError::other("the script ran out")));
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

    let seen = model.seen();
    assert_eq!(seen.len(), 2);
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
    assert_eq!(out.answers.len(), 2);
    assert_eq!(out.reply, "I'll cut everything after the talk.");
    assert!((out.usage_usd - 0.007).abs() < 1e-12, "{}", out.usage_usd);
}

#[test]
fn stops_after_two_model_calls() {
    let (p, speech) = talk();
    let i = inputs(&p, speech);
    let model = Fake::new(vec![
        answer("Cutting it.", vec![cut_call("call_1", 12.3, 19.7)], 0.004),
        answer(
            "Cutting it now.",
            vec![cut_call("call_2", 13.1, 18.4)],
            0.004,
        ),
        answer("A third try.", vec![cut_call("call_3", 7.9, 29.8)], 0.004),
    ]);
    let out = send(&model, &[], "cut the boring bit", &i, &p).unwrap();

    assert_eq!(model.seen().len(), 2);
    assert_eq!(model.left(), 1, "the third answer is never asked for");
    // Nothing invented is applied, and the reply says plainly what couldn't be done.
    assert_eq!(out.compiled.proposal, None);
    assert_eq!(out.compiled.additions, None);
    assert!(
        (out.compiled.refusals.iter()).any(|r| r.audience == Audience::Model),
        "{:?}",
        out.compiled.refusals
    );
    assert_plain(&out.reply);
    // What it couldn't find, and what to do next.
    assert_eq!(
        out.reply,
        "I couldn't find that part, so I left the video as it was. Select it on the timeline and ask again."
    );
    // The model's claim that it is cutting is not repeated when nothing came of it.
    assert!(!out.reply.contains("Cutting it now"), "{}", out.reply);
    assert!((out.usage_usd - 0.008).abs() < 1e-12);
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

    // Out of credit in the refusal round: still nothing changes, and the first call still counts.
    let model = Fake::new(vec![
        answer(
            "Square, and cutting it.",
            vec![square_call("call_1"), cut_call("call_2", 12.3, 19.7)],
            0.004,
        ),
        Err(ModelError::Budget(String::new())),
    ]);
    let e = send(&model, &[], "square, and cut the boring bit", &i, &p).unwrap_err();
    assert_eq!(e.kind, ChatErrorKind::Budget);
    assert_plain(&e.message);
    assert_eq!(e.usage_usd, 0.004);
    assert_eq!(e.model_calls, 2);
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

    // Cut off in the refusal round: both calls count, and nothing of the first answer is applied.
    let model = Fake::new(vec![
        answer(
            "",
            vec![square_call("call_1"), cut_call("call_2", 12.3, 19.7)],
            0.004,
        ),
        Err(cut_off()),
    ]);
    let e = send(&model, &[], "square, and cut the boring bit", &i, &p).unwrap_err();
    assert!((e.usage_usd - 0.014).abs() < 1e-12, "{}", e.usage_usd);
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

    // Only refusals for the person: no second call, and the sentences are the reply's.
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
    assert_eq!(model.seen().len(), 1);
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
fn nothing_to_remove_takes_one_call_and_one_clear_reply() {
    // talk.mov has no loudness, so no pause to remove: the person is told, with no second call, and the model's
    // claim that it is removing them is not repeated.
    let (p, speech) = talk();
    let i = inputs(&p, speech);
    let model = Fake::new(vec![
        answer(
            "Removing the long pauses.",
            vec![fc("call_1", "removeSilences", r#"{"minPauseSec": 1}"#)],
            0.002,
        ),
        answer("A second call.", vec![], 0.002),
    ]);
    let out = send(&model, &[], "remove pauses over a second", &i, &p).unwrap();
    assert_eq!(model.seen().len(), 1);
    assert_eq!(
        out.reply,
        "There are no pauses longer than 1.0 s to remove."
    );
    assert_eq!(out.compiled.proposal, None);
    assert_eq!(out.usage_usd, 0.002);
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
    assert_eq!(model.seen().len(), 1);
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
fn a_second_answer_does_not_redo_a_tool_already_done() {
    let (p, speech) = talk();
    let i = inputs(&p, speech);
    let square = square_call("call_1");
    let fixed = cut_call("call_4", 7.9, 29.8);
    // The format was done in the first answer: a second, different setFormat is dropped.
    let model = Fake::new(vec![
        answer(
            "",
            vec![square.clone(), cut_call("call_2", 12.3, 19.7)],
            0.002,
        ),
        answer(
            "",
            vec![
                fc(
                    "call_3",
                    "setFormat",
                    r#"{"format": "portrait", "keepOthers": true}"#,
                ),
                fixed.clone(),
            ],
            0.002,
        ),
    ]);
    let out = send(&model, &[], "square, and cut the rest", &i, &p).unwrap();
    assert_eq!(out.calls, vec![parsed(&square), parsed(&fixed)]);

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
fn a_part_still_to_come_is_not_claimed_by_the_models_text() {
    // Words on screen are still to come, the music goes in: the model's sentence claims both, so it is left out and
    // what went through is said plainly, then what is still to come.
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
    });
    let model = Fake::new(vec![answer(
        "Added your title and the lofi music.",
        vec![
            fc(
                "call_1",
                "addText",
                r#"{"text": "Day 1", "preset": "title", "atSec": 0, "durationSec": null}"#,
            ),
            fc(
                "call_2",
                "addMusic",
                r#"{"file": "lofi", "volume": null, "duck": null, "atSec": 0}"#,
            ),
        ],
        0.002,
    )]);
    let out = send(&model, &[], "a title Day 1 and lofi music", &i, &p).unwrap();
    assert_eq!(model.seen().len(), 1);
    assert!(out.compiled.additions.is_some());
    assert_eq!(
        out.reply,
        format!(
            "The music is in. {}",
            klipolo_chat::compile::TEXT_COMING_SOON
        )
    );
    // With nothing refused for the person, the model's own words stay.
    let model = Fake::new(vec![answer(
        "Added the lofi music.",
        vec![fc(
            "call_1",
            "addMusic",
            r#"{"file": "lofi", "volume": null, "duck": null, "atSec": 0}"#,
        )],
        0.002,
    )]);
    let out = send(&model, &[], "lofi music", &i, &p).unwrap();
    assert_eq!(out.reply, "Added the lofi music.");
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
