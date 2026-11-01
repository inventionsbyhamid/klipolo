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
use klipolo_model::cursor_style::{ClickSource, CursorCapture, RecordingCursor};
use klipolo_model::framing::effective_framing;
use klipolo_model::ops::Op;
use klipolo_model::project::{
    Asset, AssetKind, BackgroundKind, FormatKind, Recording, RecordingKind, Settings, SourceRole,
};
use klipolo_model::recording::{RecordingSource, SourceMix, SourceRun};
use klipolo_model::speech::Speech;
use klipolo_model::timeline::{Clip, ClipSource, Framing, FramingMode};
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
    // A call that sets nothing is sent back.
    let nothing =
        read(r#"{"kind": null, "color": null, "colorEnd": null, "padding": null}"#).unwrap_err();
    assert!(
        matches!(nothing.problem, Problem::NoChange { .. }),
        "{nothing}"
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
    // "Add some padding" keeps the blur, and its step says the background changed.
    let inset = call(r#"{"kind": null, "color": null, "colorEnd": null, "padding": 8}"#);
    let step = compile(&blurred, &inputs(&blurred), std::slice::from_ref(&inset));
    let params = step.additions.expect("an addition").params.unwrap();
    assert_eq!(params.get("kind"), Some(&ErrorParam::from("same")));
    let padded = applied(&blurred, &[inset]);
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

/// `p` made vertical: 9:16 is its only format.
fn vertical(mut p: Project) -> Project {
    p.settings = Settings {
        fps: 30,
        formats: vec![FormatKind::Portrait],
        primary_format: FormatKind::Portrait,
    };
    p
}

const BLUR: &str = r#"{"kind": "blur", "color": null, "colorEnd": null, "padding": null}"#;

#[test]
fn a_background_that_shows_sets_a_filled_clip_to_fit_in_the_same_step() {
    let p = vertical(wide());
    // A wide video fills 9:16 by default, so the blur would be hidden.
    assert_eq!(
        effective_framing(&p, &p.timeline.main[0], FormatKind::Portrait).mode,
        FramingMode::Fill
    );
    let out = compile(&p, &inputs(&p), &[call(BLUR)]);
    let additions = out.additions.expect("applied at once");
    let [Op::SetBackground { .. }, Op::UpdateClip { id, patch }] = additions.ops.as_slice() else {
        panic!("{:?}", additions.ops);
    };
    assert_eq!(id, "w");
    let framing = patch.framing.as_ref().unwrap();
    assert_eq!(framing[&FormatKind::Portrait].mode, FramingMode::Fit);
    assert_eq!(
        out.notes,
        ["A clip is set to Fit so the background shows round it."]
    );
    // One undo step takes both back.
    let after = apply_ops(&p, &additions.ops).unwrap();
    assert_eq!(after.background.kind, BackgroundKind::Blur);
    assert_eq!(
        effective_framing(&after, &after.timeline.main[0], FormatKind::Portrait).mode,
        FramingMode::Fit
    );
}

#[test]
fn a_colour_gradient_or_picture_does_the_same_and_black_does_not() {
    let p = vertical(wide());
    for args in [
        r##"{"kind": "color", "color": "#FFD02E", "colorEnd": null, "padding": null}"##,
        r##"{"kind": "gradient", "color": "#FFD02E", "colorEnd": "#121A33", "padding": null}"##,
    ] {
        let out = compile(&p, &inputs(&p), &[call(args)]);
        let ops = out.additions.expect("applied").ops;
        assert!(
            matches!(ops.as_slice(), [_, Op::UpdateClip { .. }]),
            "{args}: {ops:?}"
        );
    }
    let mut blurred = p.clone();
    blurred.background.kind = BackgroundKind::Blur;
    let black = call(r#"{"kind": "none", "color": null, "colorEnd": null, "padding": null}"#);
    let out = compile(&blurred, &inputs(&blurred), &[black]);
    let ops = out.additions.expect("applied").ops;
    assert!(
        matches!(ops.as_slice(), [Op::SetBackground { .. }]),
        "{ops:?}"
    );
    assert!(out.notes.is_empty());
}

#[test]
fn nothing_is_set_to_fit_where_the_background_already_shows_or_fill_crops_nothing() {
    // With padding, the background shows round every clip.
    let p = vertical(wide());
    let padded = call(r#"{"kind": "blur", "color": null, "colorEnd": null, "padding": 5}"#);
    let out = compile(&p, &inputs(&p), &[padded]);
    assert!(matches!(
        out.additions.unwrap().ops.as_slice(),
        [Op::SetBackground { .. }]
    ));
    assert!(out.notes.is_empty());
    // A 16:9 video fills 16:9 whole: Fill crops nothing, so Fit would change nothing either.
    let p = wide();
    let out = compile(&p, &inputs(&p), &[call(BLUR)]);
    assert!(matches!(
        out.additions.unwrap().ops.as_slice(),
        [Op::SetBackground { .. }]
    ));
    assert!(out.notes.is_empty());
    // A clip already fitted stays as it is; only the colours changing sets nothing to Fit.
    let mut fitted = vertical(wide());
    fitted.timeline.main[0].framing.insert(
        FormatKind::Portrait,
        Framing {
            mode: FramingMode::Fit,
            focus_x: 0.5,
            focus_y: 0.5,
            follow: None,
        },
    );
    let out = compile(&fitted, &inputs(&fitted), &[call(BLUR)]);
    assert!(matches!(
        out.additions.unwrap().ops.as_slice(),
        [Op::SetBackground { .. }]
    ));
    let p = vertical(wide());
    let colour = call(r##"{"kind": null, "color": "#FFFFFF", "colorEnd": null, "padding": null}"##);
    let out = compile(&p, &inputs(&p), &[colour]);
    assert!(matches!(
        out.additions.unwrap().ops.as_slice(),
        [Op::SetBackground { .. }]
    ));
}

/// `p` with a screen take that kept a cursor track (it fills and follows in 9:16 by default) after its clips, as clip
/// "r", playing the wide file as its screen.
fn with_screen_take(mut p: Project) -> Project {
    let screen = p.assets[0].clone();
    p.recordings = vec![Recording {
        id: "rec".into(),
        kind: RecordingKind::Screen,
        sources: vec![RecordingSource {
            id: "wide".into(),
            role: SourceRole::Screen,
            label: "capture.source.screen".into(),
            runs: vec![SourceRun {
                asset_id: screen.id.clone(),
                offset_us: 0,
                duration_us: screen.duration_us,
                drift_ppm: 0.0,
                file_in_us: 0,
            }],
            nudge_us: 0,
            mix: SourceMix::default(),
            synced: None,
        }],
        primary: "wide".into(),
        cursor: Some(RecordingCursor {
            path: "/media/rec/cursor.jsonl".into(),
            capture: CursorCapture::Separate,
            clicks: ClickSource::Events,
        }),
        markers: Vec::new(),
        game: None,
    }];
    let mut take = Clip::new(ClipSource::Recording { id: "rec".into() }, 0, 10_000_000);
    take.id = "r".into();
    p.timeline.main.push(take);
    p
}

#[test]
fn a_screen_take_that_followed_the_cursor_says_it_no_longer_does() {
    let p = with_screen_take(vertical(wide()));
    let follows = effective_framing(&p, &p.timeline.main[1], FormatKind::Portrait);
    assert!(follows.mode == FramingMode::Fill && follows.follow.is_some());
    let out = compile(&p, &inputs(&p), &[call(BLUR)]);
    assert_eq!(
        out.notes,
        [
            "2 clips are set to Fit so the background shows round them. One of them no longer follows the cursor."
        ]
    );
}

/// `p` in 16:9 with a vertical version too.
fn with_vertical_version(mut p: Project) -> Project {
    p.settings.formats = vec![FormatKind::Landscape, FormatKind::Portrait];
    p.settings.primary_format = FormatKind::Landscape;
    p
}

#[test]
fn every_version_where_fill_crops_is_set_to_fit_and_named() {
    // A landscape talk with a vertical version: in 9:16 the wide clip fills, cropped, so the blur would be hidden there;
    // in 16:9 it fills whole, so nothing changes there.
    let p = with_vertical_version(wide());
    let out = compile(&p, &inputs(&p), &[call(BLUR)]);
    let additions = out.additions.expect("applied at once");
    let [Op::SetBackground { .. }, Op::UpdateClip { id, patch }] = additions.ops.as_slice() else {
        panic!("{:?}", additions.ops);
    };
    assert_eq!(id, "w");
    let framing = patch.framing.as_ref().unwrap();
    assert_eq!(framing[&FormatKind::Portrait].mode, FramingMode::Fit);
    assert!(!framing.contains_key(&FormatKind::Landscape), "{framing:?}");
    assert_eq!(
        out.notes,
        ["In the vertical version, a clip is set to Fit so the background shows round it."]
    );
    let after = apply_ops(&p, &additions.ops).unwrap();
    let clip = &after.timeline.main[0];
    assert_eq!(
        effective_framing(&after, clip, FormatKind::Portrait).mode,
        FramingMode::Fit
    );
    assert_eq!(
        effective_framing(&after, clip, FormatKind::Landscape).mode,
        FramingMode::Fill
    );
}

#[test]
fn a_vertical_version_that_followed_the_cursor_says_it_no_longer_does() {
    // The screen take is fitted in 16:9 already and fills and follows in 9:16.
    let mut p = with_screen_take(with_vertical_version(wide()));
    p.timeline.main.remove(0);
    let take = &p.timeline.main[0];
    assert_eq!(
        effective_framing(&p, take, FormatKind::Landscape).mode,
        FramingMode::Fit
    );
    let out = compile(&p, &inputs(&p), &[call(BLUR)]);
    assert_eq!(
        out.notes,
        [
            "In the vertical version, a clip is set to Fit so the background shows round it. It no longer follows the \
             cursor."
        ]
    );
}
