//! The captions' look from chat: `setCaptions`' `set` keys over the style chosen: a font from the app's fonts, a size as
//! a factor of the style's, a position (a place word or a height share) in every format, the words' and the lit words'
//! colours, and uppercase. All of it is one `SetCaptions`: one undo step. The project is `talk.mov`, 30 s, played whole,
//! landscape and portrait, captions on in Bold.

use std::collections::HashSet;
use std::sync::Arc;

use klipolo_chat::tools::{CaptionChange, CaptionPlace};
use klipolo_chat::{AnalysisState, Compiled, FunctionCall, Inputs, Refs, ToolCall, View, compile};
use klipolo_model::captions::styles::BuiltinCaptionStyle;
use klipolo_model::font::{FontRef, FontSource};
use klipolo_model::ops::Op;
use klipolo_model::project::{Asset, CaptionSettings, FormatKind};
use klipolo_model::speech::Speech;
use klipolo_model::time::Us;
use klipolo_model::timeline::{Clip, ClipSource};
use klipolo_model::{Project, apply_ops};
use serde_json::{Value, json};

fn secs(s: f64) -> Us {
    (s * 1e6).round() as Us
}

fn parse(arguments: Value) -> Result<ToolCall, klipolo_chat::ToolError> {
    ToolCall::parse(&FunctionCall {
        name: "setCaptions".into(),
        call_id: "call_1".into(),
        arguments: arguments.to_string(),
    })
}

fn project() -> Project {
    let mut p = Project::new("Talk");
    let talk: Asset = serde_json::from_value(json!({
        "id": "talk", "kind": "video", "path": "/media/talk.mov", "name": "talk.mov", "durationUs": secs(30.0),
        "width": 1920, "height": 1080, "fps": 30.0, "hasAudio": true,
    }))
    .unwrap();
    p.assets = vec![talk];
    p.timeline.main = vec![Clip::new(
        ClipSource::Asset { id: "talk".into() },
        0,
        secs(30.0),
    )];
    p.settings.formats = vec![FormatKind::Landscape, FormatKind::Portrait];
    p.captions = CaptionSettings {
        enabled: true,
        style: Some(BuiltinCaptionStyle::Bold.id()),
        ..CaptionSettings::default()
    };
    p
}

fn inputs(p: &Project) -> Inputs {
    Inputs {
        message: "make the captions bigger and yellow".into(),
        view: View::default(),
        refs: Refs::of(p),
        speech: Speech::default(),
        scenes: Default::default(),
        analysis: (p.assets.iter())
            .map(|a| (a.id.clone(), AnalysisState::Done))
            .collect(),
        time_again: HashSet::new(),
        cursor: None,
        fonts: klipolo_chat::FontNames::new(Arc::new(|name: &str, weight, italic| {
            let font = match name.to_lowercase().as_str() {
                "anton" => FontRef::bundled("Anton"),
                "poppins" => FontRef {
                    family: "Poppins".into(),
                    source: FontSource::Google {
                        version: "v23".into(),
                    },
                },
                _ => return None,
            };
            let here = font.family != "Poppins";
            Some(klipolo_chat::NamedFont {
                font,
                weight,
                italic,
                download_bytes: (!here).then_some(2_890_000),
            })
        })),
        library: Default::default(),
    }
}

fn run(p: &Project, set: Value) -> Compiled {
    let call = parse(json!({"on": true, "style": null, "set": set})).unwrap();
    compile(p, &inputs(p), &[call])
}

/// The captions the one `SetCaptions` leaves: one undo step.
fn captions(p: &Project, out: &Compiled) -> CaptionSettings {
    assert_eq!(out.refusals, vec![], "{:#?}", out.refusals);
    let tx = out.additions.as_ref().expect("additions");
    assert!(
        matches!(tx.ops.as_slice(), [Op::SetCaptions { .. }]),
        "{:?}",
        tx.ops
    );
    assert_eq!(tx.label, "chat.setCaptions");
    apply_ops(p, &tx.ops).unwrap().captions
}

#[test]
fn set_keys_read_into_a_look_change() {
    let read = parse(json!({"on": true, "style": "karaoke", "set": [
        {"key": "font", "value": "Anton"},
        {"key": "size", "value": 1.3},
        {"key": "position", "value": "top"},
        {"key": "color", "value": "#ffd02e"},
        {"key": "highlight", "value": "none"},
        {"key": "Upper", "value": true},
    ]}))
    .unwrap();
    assert_eq!(
        read,
        ToolCall::SetCaptions {
            on: true,
            style: Some(BuiltinCaptionStyle::Karaoke),
            look: Box::new(CaptionChange {
                font: Some(Some("Anton".into())),
                size: Some(1.3),
                position: Some(CaptionPlace::Top),
                color: Some(Some("#FFD02E".into())),
                highlight: Some(None),
                upper: Some(Some(true)),
            }),
        }
    );
    let at = parse(json!({"on": true, "style": null, "set": [
        {"key": "position", "value": 0.3}, {"key": "upper", "value": "none"}, {"key": "font", "value": "none"},
    ]}))
    .unwrap();
    let ToolCall::SetCaptions { look, .. } = at else {
        panic!("{at:?}");
    };
    assert_eq!(look.position, Some(CaptionPlace::At(0.3)));
    assert_eq!(look.upper, Some(None));
    assert_eq!(look.font, Some(None));
    // Left out, null or empty: no look change, as before.
    for args in [
        json!({"on": true, "style": null}),
        json!({"on": true, "style": null, "set": null}),
        json!({"on": false, "style": null, "set": []}),
    ] {
        let ToolCall::SetCaptions { look, .. } = parse(args.clone()).unwrap() else {
            panic!("{args}");
        };
        assert_eq!(*look, CaptionChange::default(), "{args}");
    }
}

#[test]
fn a_value_that_does_not_fit_its_key_is_said_back() {
    for (set, field, says) in [
        (
            json!([{"key": "size", "value": 3}]),
            "set[0].value",
            "between 0.5 and 2",
        ),
        (
            json!([{"key": "position", "value": "left"}]),
            "set[0].value",
            "top, middle, bottom",
        ),
        (
            json!([{"key": "position", "value": 0.99}]),
            "set[0].value",
            "0.05",
        ),
        (
            json!([{"key": "color", "value": "yellow"}]),
            "set[0].value",
            "#FFD02E",
        ),
        (
            json!([{"key": "color", "value": "#FFD02E80"}]),
            "set[0].value",
            "#FFD02E",
        ),
        (
            json!([{"key": "outline", "value": "#000000"}]),
            "set[0].key",
            "font",
        ),
    ] {
        let err = parse(json!({"on": true, "style": null, "set": set})).unwrap_err();
        assert_eq!(err.field.as_deref(), Some(field), "{set}: {err}");
        assert!(err.to_string().contains(says), "{set}: {err}");
        assert!(!err.to_string().contains(['\u{2013}', '\u{2014}']), "{err}");
    }
}

#[test]
fn the_look_goes_over_the_style_in_one_step() {
    let p = project();
    let out = run(
        &p,
        json!([
            {"key": "font", "value": "anton"},
            {"key": "size", "value": 1.3},
            {"key": "position", "value": "top"},
            {"key": "color", "value": "#FFD02E"},
            {"key": "highlight", "value": "#FF2E5B"},
            {"key": "upper", "value": false},
        ]),
    );
    let c = captions(&p, &out);
    assert!(c.enabled);
    assert_eq!(
        c.style,
        Some(BuiltinCaptionStyle::Bold.id()),
        "the style kept"
    );
    assert_eq!(c.font, Some(FontRef::bundled("Anton")));
    assert_eq!(c.size, Some(1.3));
    assert_eq!(c.color.as_deref(), Some("#FFD02E"));
    assert_eq!(c.accent.as_deref(), Some("#FF2E5B"));
    assert_eq!(c.upper, Some(false));
    // Top as the Captions panel places it, in each format the video has.
    assert_eq!(
        c.place,
        [(FormatKind::Landscape, 0.14), (FormatKind::Portrait, 0.22)].into()
    );
    assert_eq!(out.notes, Vec::<String>::new());
}

#[test]
fn the_middle_bottom_or_a_height_is_every_formats_place() {
    let p = project();
    let c = captions(
        &p,
        &run(&p, json!([{"key": "position", "value": "bottom"}])),
    );
    assert_eq!(
        c.place,
        [(FormatKind::Landscape, 0.88), (FormatKind::Portrait, 0.7)].into()
    );
    let c = captions(
        &p,
        &run(&p, json!([{"key": "position", "value": "middle"}])),
    );
    assert_eq!(
        c.place,
        [(FormatKind::Landscape, 0.5), (FormatKind::Portrait, 0.5)].into()
    );
    let c = captions(&p, &run(&p, json!([{"key": "position", "value": 0.3}])));
    assert_eq!(
        c.place,
        [(FormatKind::Landscape, 0.3), (FormatKind::Portrait, 0.3)].into()
    );
}

#[test]
fn none_or_a_size_of_one_gives_back_the_styles_own() {
    let mut p = project();
    p.captions.font = Some(FontRef::bundled("Anton"));
    p.captions.size = Some(1.5);
    p.captions.color = Some("#FFD02E".into());
    p.captions.accent = Some("#FF2E5B".into());
    p.captions.upper = Some(true);
    let c = captions(
        &p,
        &run(
            &p,
            json!([
                {"key": "font", "value": "none"},
                {"key": "size", "value": 1},
                {"key": "color", "value": "none"},
                {"key": "highlight", "value": "none"},
                {"key": "upper", "value": "none"},
            ]),
        ),
    );
    assert_eq!(
        c,
        CaptionSettings {
            enabled: true,
            style: Some(BuiltinCaptionStyle::Bold.id()),
            ..CaptionSettings::default()
        }
    );
}

#[test]
fn a_font_is_named_from_the_apps_fonts() {
    let p = project();
    let out = run(&p, json!([{"key": "font", "value": "Comic Neue Ultra"}]));
    assert_eq!(out.additions, None);
    assert_eq!(out.refusals.len(), 1);
    let reason = &out.refusals[0].reason;
    assert!(
        reason.starts_with("setCaptions: no font is named \"Comic Neue Ultra\""),
        "{reason}"
    );
    // A Google font not on this computer is used all the same, and the reply says it still has to be downloaded.
    let out = run(&p, json!([{"key": "font", "value": "Poppins"}]));
    let c = captions(&p, &out);
    assert_eq!(c.font.as_ref().map(|f| f.family.as_str()), Some("Poppins"));
    assert_eq!(out.notes.len(), 1, "{:?}", out.notes);
    assert_eq!(
        out.notes[0],
        "Poppins isn't on this Mac yet, so the captions show in Inter until you download it over the preview."
    );
}
