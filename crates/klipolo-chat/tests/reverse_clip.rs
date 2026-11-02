//! Reverse from chat: `reverseClip` backward is a job for the app to write the clip's reversed copy, as the clip menu's
//! Reverse does (the copy and the turn then land as one undo step); forward again is one `ReverseClip` without a copy,
//! applied at once. A clip that can't play backward is refused with the model's own reason. The project: "talk.mov"
//! (30 s, with sound) as c1 0-10 s and c2 10-30 s of the file.

use std::collections::{HashMap, HashSet};
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use klipolo_chat::{
    AnalysisState, Audience, BoxFuture, Compiled, FunctionCall, Inputs, Job, LanguageModel,
    ModelError, ModelRequest, ModelResponse, Refs, ToolCall, View, build_context, compile, plan,
};
use klipolo_model::ops::Op;
use klipolo_model::project::Asset;
use klipolo_model::speech::Speech;
use klipolo_model::time::Us;
use klipolo_model::timeline::{Clip, ClipSource, ReverseCopy};
use klipolo_model::{Project, apply_ops};
use serde_json::json;

fn secs(s: f64) -> Us {
    (s * 1e6).round() as Us
}

fn call(clip: &str, backward: bool) -> ToolCall {
    ToolCall::parse(&FunctionCall {
        name: "reverseClip".into(),
        call_id: "call_1".into(),
        arguments: json!({"clip": clip, "backward": backward}).to_string(),
    })
    .unwrap()
}

fn asset(id: &str, name: &str) -> Asset {
    serde_json::from_value(json!({
        "id": id, "kind": "video", "path": format!("/media/{name}"), "name": name, "durationUs": secs(30.0),
        "width": 1920, "height": 1080, "fps": 30.0, "hasAudio": true,
    }))
    .unwrap()
}

fn project() -> Project {
    let mut p = Project::new("Talk");
    p.assets = vec![asset("talk", "talk.mov")];
    let mut a = Clip::new(ClipSource::Asset { id: "talk".into() }, 0, secs(10.0));
    a.id = "a".into();
    let mut b = Clip::new(
        ClipSource::Asset { id: "talk".into() },
        secs(10.0),
        secs(30.0),
    );
    b.id = "b".into();
    p.timeline.main = vec![a, b];
    p
}

/// The project with clip `b` (c2) playing backward from its copy.
fn reversed() -> Project {
    let p = project();
    apply_ops(
        &p,
        &[
            Op::AddAssets {
                assets: vec![asset("talk-back", "talk reversed.mov")],
            },
            Op::ReverseClip {
                clip_id: "b".into(),
                copy: Some(ReverseCopy {
                    asset_id: "talk-back".into(),
                    from_us: 0,
                    to_us: secs(30.0),
                }),
            },
        ],
    )
    .unwrap()
}

fn inputs(p: &Project) -> Inputs {
    Inputs {
        message: "play the second clip backward".into(),
        view: View::default(),
        refs: Refs::of(p),
        speech: Speech::default(),
        scenes: Default::default(),
        analysis: (p.assets.iter())
            .map(|a| (a.id.clone(), AnalysisState::Done))
            .collect(),
        time_again: HashSet::new(),
        cursor: None,
        fonts: Default::default(),
        library: Default::default(),
    }
}

fn run(p: &Project, calls: &[ToolCall]) -> Compiled {
    compile(p, &inputs(p), calls)
}

#[test]
fn reverse_clip_reads_a_ref_and_which_way() {
    assert_eq!(
        call("c2", true),
        ToolCall::ReverseClip {
            clip: "c2".into(),
            backward: true
        }
    );
    let missing = ToolCall::parse(&FunctionCall {
        name: "reverseClip".into(),
        call_id: "call_1".into(),
        arguments: r#"{"clip": "c2"}"#.into(),
    })
    .unwrap_err();
    assert_eq!(missing.field.as_deref(), Some("backward"));
}

#[test]
fn backward_is_a_job_for_the_clips_reversed_copy_and_changes_nothing_yet() {
    let p = project();
    let out = run(&p, &[call("c2", true)]);
    assert_eq!(out.refusals, vec![]);
    assert_eq!(out.additions, None, "the copy and the turn land later");
    assert_eq!(out.proposal, None);
    assert_eq!(
        out.jobs,
        vec![Job::Reverse {
            clip_id: "b".into()
        }]
    );
    assert_eq!(out.unchanged, Vec::<usize>::new());
    assert_eq!(out.notes, vec![klipolo_chat::compile::REVERSING.to_owned()]);
}

#[test]
fn backward_on_a_clip_already_playing_backward_changes_nothing() {
    let p = reversed();
    let out = run(&p, &[call("c2", true)]);
    assert_eq!(out.refusals, vec![]);
    assert_eq!(out.jobs, vec![]);
    assert_eq!(out.unchanged, vec![0]);
}

#[test]
fn forward_again_is_one_undo_step_at_once() {
    let p = reversed();
    let out = run(&p, &[call("c2", false)]);
    assert_eq!(out.refusals, vec![]);
    assert_eq!(out.jobs, vec![]);
    let tx = out.additions.expect("applied at once");
    assert_eq!(
        tx.ops,
        vec![Op::ReverseClip {
            clip_id: "b".into(),
            copy: None
        }]
    );
    assert_eq!(tx.label, "chat.reverseClip");
    let after = apply_ops(&p, &tx.ops).unwrap();
    assert!(after.timeline.main.iter().all(|c| c.reversed.is_none()));
    // Forward on a clip that plays forward: nothing to do.
    let out = run(&project(), &[call("c1", false)]);
    assert_eq!(out.additions, None);
    assert_eq!(out.unchanged, vec![0]);
}

#[test]
fn a_clip_that_cant_play_backward_is_refused_with_why() {
    let mut p = project();
    p.timeline.main[0].freeze_at_us = Some(secs(2.0));
    let out = run(&p, &[call("c1", true)]);
    assert_eq!(out.jobs, vec![]);
    assert_eq!(out.refusals.len(), 1);
    assert_eq!(out.refusals[0].audience, Audience::Model);
    assert_eq!(
        out.refusals[0].reason,
        "reverseClip: c1: A freeze frame can't play backward."
    );
    let out = run(&project(), &[call("c9", true)]);
    assert_eq!(out.refusals[0].reason, "reverseClip: there is no clip c9.");
}

/// Answers with one `reverseClip` call, then with no words.
struct Reverses(std::sync::Mutex<usize>);

impl LanguageModel for Reverses {
    fn respond<'a>(
        &'a self,
        _request: &'a ModelRequest,
    ) -> BoxFuture<'a, Result<ModelResponse, ModelError>> {
        let mut n = self.0.lock().unwrap();
        *n += 1;
        let calls = if *n == 1 {
            vec![FunctionCall {
                name: "reverseClip".into(),
                call_id: "call_1".into(),
                arguments: json!({"clip": "c2", "backward": true}).to_string(),
            }]
        } else {
            Vec::new()
        };
        Box::pin(async move {
            Ok(ModelResponse {
                text: String::new(),
                calls,
                usage_usd: 0.0,
            })
        })
    }
}

#[test]
fn the_reply_says_the_copy_is_being_made() {
    let p = project();
    let i = inputs(&p);
    let ctx = build_context(
        &p,
        &i.speech,
        &HashMap::new(),
        &i.scenes,
        &i.view,
        &i.library,
    );
    let model = Reverses(std::sync::Mutex::new(0));
    let mut future = pin!(plan(&model, &[], &ctx, "play c2 backward", &i, &p));
    let Poll::Ready(out) = future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    else {
        panic!("the planner waited on a fake that answers at once");
    };
    let out = out.unwrap();
    assert_eq!(out.compiled.jobs.len(), 1);
    assert_eq!(out.reply, "The reversed copy is being made.");
}
