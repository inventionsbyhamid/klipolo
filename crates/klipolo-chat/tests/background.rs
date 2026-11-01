//! `setBackground`: "put a blurred background behind it" as one undo step, read from the model's call, compiled on the
//! project with every value left null kept, refused back to the model when the project won't take it, and the
//! background the context shows.

use std::collections::{HashMap, HashSet};

use klipolo_chat::tools::BackgroundChoice;
use klipolo_chat::{
    AnalysisState, Audience, FunctionCall, Inputs, Library, Problem, Refs, ToolCall, View,
    build_context, compile,
};
use klipolo_model::coded::ErrorParam;
use klipolo_model::ops::Op;
use klipolo_model::project::{Asset, AssetKind, BackgroundKind};
use klipolo_model::speech::Speech;
use klipolo_model::timeline::{Clip, ClipSource};
use klipolo_model::{Project, apply_ops};

fn read(arguments: &str) -> Result<ToolCall, klipolo_chat::ToolError> {
    ToolCall::parse(&FunctionCall {
        name: "setBackground".into(),
        call_id: "call_1".into(),
        arguments: arguments.into(),
    })
}

fn call(arguments: &str) -> ToolCall {
    read(arguments).unwrap_or_else(|e| panic!("{arguments}: {e}"))
}

/// A 16:9 video, 10 s, played whole.
fn wide() -> Project {
    let mut p = Project::new("Test");
    p.assets = vec![Asset {
        id: "wide".into(),
        kind: AssetKind::Video,
        path: "/media/wide.mov".into(),
        name: "wide.mov".into(),
        duration_us: 10_000_000,
        width: 1920,
        height: 1080,
        fps: 30.0,
        rotation: None,
        has_audio: false,
        audio_lead_us: None,
        made_with: None,
    }];
    let mut clip = Clip::new(ClipSource::Asset { id: "wide".into() }, 0, 10_000_000);
    clip.id = "w".into();
    p.timeline.main = vec![clip];
    p
}

fn inputs(p: &Project) -> Inputs {
    Inputs {
        message: "put a blurred background behind it".into(),
        view: View::default(),
        refs: Refs::of(p),
        speech: Speech::default(),
        scenes: HashMap::new(),
        analysis: (p.assets.iter())
            .map(|a| (a.id.clone(), AnalysisState::Done))
            .collect(),
        time_again: HashSet::new(),
        library: Library::default(),
    }
}

/// `calls` compiled on `p` and applied: the project they make.
fn applied(p: &Project, calls: &[ToolCall]) -> Project {
    let out = compile(p, &inputs(p), calls);
    assert!(out.refusals.is_empty(), "{:?}", out.refusals);
    apply_ops(p, &out.additions.expect("an addition").ops).unwrap()
}

#[test]
fn the_model_names_a_background_and_leaves_the_rest_null() {
    let blur = r#"{"kind": "blur", "color": null, "colorEnd": null, "padding": null}"#;
    assert_eq!(
        call(blur),
        ToolCall::SetBackground {
            kind: Some(BackgroundChoice::Blur),
            color: None,
            color_end: None,
            padding: None,
        }
    );
    let inset = r##"{"kind": null, "color": " #1A2B3C ", "colorEnd": null, "padding": 10}"##;
    assert_eq!(
        call(inset),
        ToolCall::SetBackground {
            kind: None,
            color: Some("#1A2B3C".into()),
            color_end: None,
            padding: Some(0.1),
        }
    );
    // A picture isn't chat's to choose, and the inset stops at 40 %.
    let picture =
        read(r#"{"kind": "image", "color": null, "colorEnd": null, "padding": null}"#).unwrap_err();
    assert!(
        matches!(picture.problem, Problem::NotOneOf { .. }),
        "{picture}"
    );
    let wide_inset =
        read(r#"{"kind": null, "color": null, "colorEnd": null, "padding": 60}"#).unwrap_err();
    assert!(
        matches!(wide_inset.problem, Problem::NotWithin { .. }),
        "{wide_inset}"
    );
    let not_text =
        read(r#"{"kind": null, "color": 3, "colorEnd": null, "padding": null}"#).unwrap_err();
    assert!(
        matches!(not_text.problem, Problem::WrongType { .. }),
        "{not_text}"
    );
}

#[test]
fn the_tool_is_offered_with_its_words() {
    let tools = klipolo_chat::tools();
    let tool = (tools.as_array().unwrap().iter())
        .find(|t| t["name"] == "setBackground")
        .expect("setBackground is offered");
    let kinds = &tool["parameters"]["properties"]["kind"]["enum"];
    assert_eq!(
        kinds,
        &serde_json::json!(["blur", "color", "gradient", "none", null])
    );
    assert_eq!(
        tool["parameters"]["required"],
        serde_json::json!(["kind", "color", "colorEnd", "padding"])
    );
}

#[test]
fn a_blurred_background_is_one_undo_step() {
    let p = wide();
    let blur = call(r#"{"kind": "blur", "color": null, "colorEnd": null, "padding": null}"#);
    let out = compile(&p, &inputs(&p), std::slice::from_ref(&blur));
    let additions = out.additions.expect("applied at once");
    assert!(out.proposal.is_none());
    assert_eq!(additions.label, "chat.setBackground");
    assert_eq!(
        additions.params.unwrap().get("kind"),
        Some(&ErrorParam::from("blur"))
    );
    let [Op::SetBackground { background }] = additions.ops.as_slice() else {
        panic!("{:?}", additions.ops);
    };
    assert_eq!(background.kind, BackgroundKind::Blur);
    // Everything else as it was.
    assert_eq!(background.color, p.background.color);
    assert_eq!(background.padding, p.background.padding);
}

#[test]
fn values_left_null_keep_the_projects_own() {
    let p = wide();
    let blurred = applied(
        &p,
        &[call(
            r#"{"kind": "blur", "color": null, "colorEnd": null, "padding": null}"#,
        )],
    );
    // "Add some padding" keeps the blur.
    let padded = applied(
        &blurred,
        &[call(
            r#"{"kind": null, "color": null, "colorEnd": null, "padding": 8}"#,
        )],
    );
    assert_eq!(padded.background.kind, BackgroundKind::Blur);
    assert_eq!(padded.background.padding, 0.08);
    // A gradient given one colour fades it into itself until it has a second.
    let gradient = applied(
        &padded,
        &[call(
            r##"{"kind": "gradient", "color": "#FF0000", "colorEnd": null, "padding": null}"##,
        )],
    );
    assert_eq!(gradient.background.kind, BackgroundKind::Gradient);
    assert_eq!(gradient.background.color_end.as_deref(), Some("#FF0000"));
    assert_eq!(gradient.background.padding, 0.08);
}

#[test]
fn a_background_it_already_has_changes_nothing() {
    let p = wide();
    let none = call(r#"{"kind": "none", "color": null, "colorEnd": null, "padding": null}"#);
    let out = compile(&p, &inputs(&p), &[none]);
    assert!(out.additions.is_none());
    assert_eq!(out.unchanged, [0]);
}

#[test]
fn a_colour_the_project_wont_take_goes_back_to_the_model() {
    let p = wide();
    let red = call(r#"{"kind": "color", "color": "red", "colorEnd": null, "padding": null}"#);
    let out = compile(&p, &inputs(&p), &[red]);
    assert!(out.additions.is_none());
    let [refusal] = out.refusals.as_slice() else {
        panic!("{:?}", out.refusals);
    };
    assert_eq!(refusal.audience, Audience::Model);
    assert!(
        refusal.reason.starts_with("setBackground: "),
        "{}",
        refusal.reason
    );
}

#[test]
fn the_context_shows_a_background_once_there_is_one() {
    let context = |p: &Project| {
        build_context(
            p,
            &Speech::default(),
            &HashMap::new(),
            &HashMap::new(),
            &View::default(),
            &Library::default(),
        )
        .text
    };
    let p = wide();
    assert!(!context(&p).contains("Background:"), "{}", context(&p));
    let mut blurred = p.clone();
    blurred.background.kind = BackgroundKind::Blur;
    blurred.background.padding = 0.1;
    assert!(
        context(&blurred).contains("Background: blur, padding 10%\n"),
        "{}",
        context(&blurred)
    );
    let mut solid = p;
    solid.background.kind = BackgroundKind::Solid;
    solid.background.color = "#1A2B3C".into();
    assert!(context(&solid).contains("Background: color #1A2B3C\n"));
}
