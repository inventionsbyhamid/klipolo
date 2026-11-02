//! The tools the model composes any edit with, beyond the presets: keyframes and presets with their settings on any
//! layer (`setMotion`), words that come in by letter, word or line (`animateText`), a layer's or clip's card fields by
//! key (`setLayer`: place, size, turn, opacity, blend, corners, crop, Fit or Fill, order, fades and
//! ducking), and one effect at a time from the catalogue (`setEffects`). The project is made here:
//! - `talk.mov`, 30 s, played whole as `c1`, landscape;
//! - `t1`: a title reading "Day one" from 0.5 to 3.5 s; `i1`: the Heart picture from 10.0 to 15.0 s; `m1`: a song from
//!   the start to the end.

use std::collections::HashSet;
use std::sync::Arc;

use klipolo_chat::tools::{KeyIn, LayerRef, Property, Slot};
use klipolo_chat::{
    AnalysisState, Audience, Compiled, FunctionCall, Inputs, Library, LibraryItem, MediaKind, Refs,
    ToolCall, View, compile,
};
use klipolo_model::contrib::ContribId;
use klipolo_model::media::music::{MusicOpts, add_music_ops};
use klipolo_model::media::text::{TextPreset, add_text_ops};
use klipolo_model::media::text_motion::{self, Unit};
use klipolo_model::motion::{Blend, Ease, Key};
use klipolo_model::params::ParamValue;
use klipolo_model::project::Asset;
use klipolo_model::speech::Speech;
use klipolo_model::time::Us;
use klipolo_model::timeline::{Clip, ClipSource, Item, ItemContent, TextAlign, TrackKind};
use klipolo_model::{Project, apply_ops};
use serde_json::{Value, json};

fn secs(s: f64) -> Us {
    (s * 1e6).round() as Us
}

fn parse(name: &str, arguments: Value) -> Result<ToolCall, klipolo_chat::ToolError> {
    ToolCall::parse(&FunctionCall {
        name: name.into(),
        call_id: "call_1".into(),
        arguments: arguments.to_string(),
    })
}

fn call(name: &str, arguments: Value) -> ToolCall {
    parse(name, arguments.clone()).unwrap_or_else(|e| panic!("{name} {arguments}: {e}"))
}

fn refused(name: &str, arguments: Value) -> String {
    let said = parse(name, arguments.clone())
        .map(|c| format!("{c:?}"))
        .expect_err(&format!("{name} {arguments} is refused"))
        .to_string();
    assert!(!said.contains(['\n', '\u{2013}', '\u{2014}']), "{said}");
    said
}

fn asset(id: &str, kind: &str, name: &str, length: f64, size: (u32, u32), audio: bool) -> Asset {
    serde_json::from_value(json!({
        "id": id,
        "kind": kind,
        "path": format!("/media/{name}"),
        "name": name,
        "durationUs": secs(length),
        "width": size.0,
        "height": size.1,
        "fps": if kind == "video" { 30.0 } else { 0.0 },
        "hasAudio": audio,
    }))
    .unwrap()
}

fn heart() -> Asset {
    asset("pack-heart", "image", "heart.png", 5.0, (512, 512), false)
}

fn song() -> Asset {
    asset("lofi", "audio", "lofi.mp3", 60.0, (0, 0), true)
}

/// "talk.mov" played whole, with "Day one" from 0.5 s, the Heart picture from 10.0 s and a song under it all.
fn project() -> Project {
    let mut p = Project::new("Trip");
    p.assets = vec![
        asset("talk", "video", "talk.mov", 30.0, (1920, 1080), true),
        song(),
    ];
    let mut clip = Clip::new(ClipSource::Asset { id: "talk".into() }, 0, secs(30.0));
    clip.id = "c".into();
    p.timeline.main = vec![clip];
    let mut ops = add_text_ops(&p, TextPreset::Title, "Day one", secs(0.5)).unwrap();
    ops.extend(klipolo_model::media::image::add_image_ops(&p, &heart(), secs(10.0)).unwrap());
    ops.extend(add_music_ops(&p, &song(), MusicOpts::default()).unwrap());
    apply_ops(&p, &ops).unwrap()
}

fn library() -> Library {
    Library {
        items: vec![LibraryItem {
            name: "Heart".into(),
            kind: MediaKind::Image,
            asset: heart(),
            pack: true,
            tags: Vec::new(),
        }],
    }
}

fn font_names() -> klipolo_chat::FontNames {
    use klipolo_model::font::{FontRef, FontSource};
    klipolo_chat::FontNames::new(Arc::new(|name: &str, weight, italic| {
        let font = match name.to_lowercase().as_str() {
            "inter" => FontRef::bundled("Inter"),
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
    }))
}

fn inputs(p: &Project, message: &str) -> Inputs {
    Inputs {
        message: message.into(),
        view: View::default(),
        refs: Refs::of(p),
        speech: Speech::default(),
        scenes: Default::default(),
        analysis: (p.assets.iter())
            .map(|a| (a.id.clone(), AnalysisState::Done))
            .collect(),
        time_again: HashSet::new(),
        cursor: None,
        fonts: font_names(),
        library: library(),
    }
}

fn run(p: &Project, calls: &[(&str, Value)]) -> Compiled {
    let calls: Vec<ToolCall> = calls.iter().map(|(n, a)| call(n, a.clone())).collect();
    compile(p, &inputs(p, "make it move"), &calls)
}

/// The project with the additions applied.
fn applied(p: &Project, out: &Compiled) -> Project {
    assert_eq!(out.refusals, vec![], "{:#?}", out.refusals);
    let tx = out.additions.as_ref().expect("additions");
    apply_ops(p, &tx.ops).unwrap()
}

/// The one refusal, for call `index`, to the model.
fn refused_to_model(out: &Compiled, index: usize) -> &str {
    assert_eq!(out.refusals.len(), 1, "{:#?}", out.refusals);
    let r = &out.refusals[0];
    assert_eq!(
        (r.call_index, r.audience),
        (index, Audience::Model),
        "{r:?}"
    );
    assert!(!r.reason.contains(['\n', '\u{2013}', '\u{2014}']), "{r:?}");
    &r.reason
}

fn layer(p: &Project, pick: impl Fn(&Item) -> bool) -> &Item {
    (p.timeline.tracks.iter())
        .flat_map(|t| &t.items)
        .find(|i| pick(i))
        .expect("the layer")
}

fn words(p: &Project) -> &Item {
    layer(p, |i| {
        matches!(&i.content, ItemContent::Text { .. })
            || matches!(&i.content, ItemContent::Generator { source, .. } if source.as_str() == text_motion::SOURCE)
    })
}

fn picture(p: &Project) -> &Item {
    layer(p, |i| matches!(i.content, ItemContent::Image { .. }))
}

fn key(t: f64, v: f64, ease: Option<Ease>) -> Key {
    Key {
        t_us: secs(t),
        v,
        ease,
    }
}

fn motion(keys: Value, presets: Value) -> Value {
    json!({"layer": "t1", "keys": keys, "presets": presets})
}

// ---- setMotion ----

#[test]
fn set_motion_reads_keys_with_eases_and_presets_with_their_settings() {
    let parsed = call(
        "setMotion",
        motion(
            json!([
                {"property": "x", "atSec": 0, "value": -0.6, "ease": null},
                {"property": "x", "atSec": 0.5, "value": 0, "ease": "spring 0.4"},
                {"property": "rotation", "atSec": 0, "value": -360, "ease": "OUT"},
                {"property": "scale", "atSec": 1, "value": 2, "ease": "cubic 0.3, 1.6, 0.6, 1"},
            ]),
            json!("enter slide left 0.5s spring; loop float 1.5 2s inOut, exit fade"),
        ),
    );
    let ToolCall::SetMotion {
        layer,
        keys: Some(keys),
        presets,
    } = parsed
    else {
        panic!("{parsed:?}");
    };
    assert_eq!(layer, LayerRef::One("t1".into()));
    assert_eq!(
        keys[1],
        KeyIn {
            property: Property::X,
            at_us: secs(0.5),
            value: 0.0,
            ease: Some(klipolo_chat::tools::motion::spring(0.4)),
        }
    );
    assert_eq!(keys[2].ease, Some(Ease::Power2Out));
    assert_eq!(
        keys[3].ease,
        Some(Ease::CubicBezier {
            x1: 0.3,
            y1: 1.6,
            x2: 0.6,
            y2: 1.0
        })
    );
    let slots: Vec<Slot> = presets.iter().map(|p| p.slot).collect();
    assert_eq!(slots, [Slot::Enter, Slot::Loop, Slot::Exit]);
    let enter = &presets[0];
    assert_eq!(
        (enter.preset, enter.from, enter.duration_us),
        (Some("slide"), Some("left"), Some(secs(0.5)))
    );
    assert!(matches!(enter.ease, Some(Ease::Spring { .. })));
    let float = &presets[1];
    assert_eq!(
        (float.preset, float.strength, float.duration_us, float.ease),
        (
            Some("float"),
            Some(1.5),
            Some(secs(2.0)),
            Some(Ease::Power2InOut)
        )
    );
    assert_eq!(presets[2].preset, Some("fade"));
    // `blur` is the model's blur-in; none takes the slot's away.
    let ToolCall::SetMotion { presets, .. } = call(
        "setMotion",
        motion(Value::Null, json!("enter blur; exit none")),
    ) else {
        panic!()
    };
    assert_eq!(
        presets.iter().map(|p| p.preset).collect::<Vec<_>>(),
        [Some("blur-in"), None]
    );
}

#[test]
fn set_motion_refuses_values_and_words_out_of_their_range_with_the_range() {
    let key = |property: &str, value: f64| {
        motion(
            json!([{"property": property, "atSec": 0, "value": value, "ease": null}]),
            Value::Null,
        )
    };
    assert_eq!(
        refused("setMotion", key("opacity", 2.0)),
        "setMotion: keys[0].value must be between 0 and 1, not 2."
    );
    assert_eq!(
        refused("setMotion", key("x", 9.0)),
        "setMotion: keys[0].value must be between -4 and 4, not 9."
    );
    let said = refused(
        "setMotion",
        motion(
            json!([{"property": "y", "atSec": 0, "value": 0, "ease": "bouncy"}]),
            Value::Null,
        ),
    );
    assert!(
        said.contains("must be linear, in, out, inOut, spring"),
        "{said}"
    );
    assert!(refused("setMotion", motion(json!(null), json!("enter spin"))).contains("slide left"));
    assert!(
        refused("setMotion", motion(json!(null), json!("loop float 3"))).contains("from 0 to 2")
    );
    assert!(
        refused(
            "setMotion",
            motion(json!(null), json!("enter pop; enter fade"))
        )
        .contains("semicolons")
    );
    assert_eq!(
        refused("setMotion", motion(json!(null), json!(null))),
        "setMotion: set at least one of keys or presets."
    );
}

#[test]
fn keys_replace_their_property_only_and_an_empty_list_takes_them_all() {
    let p = project();
    let slide = motion(
        json!([
            {"property": "x", "atSec": 0, "value": -0.6, "ease": null},
            {"property": "x", "atSec": 0.5, "value": 0, "ease": "spring 0.3"},
            {"property": "opacity", "atSec": 0, "value": 0, "ease": null},
            {"property": "opacity", "atSec": 0.3, "value": 1, "ease": "out"},
        ]),
        Value::Null,
    );
    let out = run(&p, &[("setMotion", slide)]);
    assert_eq!(out.additions.as_ref().unwrap().label, "chat.setMotion");
    let after = applied(&p, &out);
    let keys = words(&after)
        .layout
        .motion
        .as_ref()
        .unwrap()
        .keys
        .clone()
        .unwrap();
    assert_eq!(
        keys.x,
        [
            key(0.0, -0.6, None),
            key(0.5, 0.0, Some(klipolo_chat::tools::motion::spring(0.3)))
        ]
    );
    assert_eq!(keys.opacity.len(), 2);
    // The title's own fade in and out stay: keys and presets go together.
    assert!(
        words(&after)
            .layout
            .motion
            .as_ref()
            .unwrap()
            .enter
            .is_some()
    );
    // A new x replaces x and leaves opacity; keys in any order are put in time order.
    let again = motion(
        json!([
            {"property": "x", "atSec": 1, "value": 0, "ease": null},
            {"property": "x", "atSec": 0, "value": 0.6, "ease": null},
        ]),
        Value::Null,
    );
    let twice = applied(&after, &run(&after, &[("setMotion", again)]));
    let keys = words(&twice)
        .layout
        .motion
        .as_ref()
        .unwrap()
        .keys
        .clone()
        .unwrap();
    assert_eq!(keys.x, [key(0.0, 0.6, None), key(1.0, 0.0, None)]);
    assert_eq!(keys.opacity.len(), 2);
    let cleared = applied(
        &twice,
        &run(&twice, &[("setMotion", motion(json!([]), Value::Null))]),
    );
    assert_eq!(words(&cleared).layout.motion.as_ref().unwrap().keys, None);
}

#[test]
fn a_key_past_the_layers_end_or_twice_at_one_time_goes_back_to_the_model() {
    let p = project();
    let late = motion(
        json!([{"property": "y", "atSec": 9, "value": 0.1, "ease": null}]),
        Value::Null,
    );
    let out = run(&p, &[("setMotion", late)]);
    assert_eq!(
        refused_to_model(&out, 0),
        "setMotion: keys[0].atSec 9.0 is past the layer's end; it lasts 3.0 s, so use 0 to 3.0."
    );
    let twice = motion(
        json!([
            {"property": "y", "atSec": 1, "value": 0.1, "ease": null},
            {"property": "y", "atSec": 1, "value": 0.2, "ease": null},
        ]),
        Value::Null,
    );
    let out = run(&p, &[("setMotion", twice)]);
    assert!(refused_to_model(&out, 0).contains("both set y at 1.0 s"));
}

#[test]
fn presets_take_their_settings_length_and_ease_and_a_typewriter_is_for_words() {
    let p = project();
    let spin = json!({"layer": "i1", "keys": null, "presets": "enter zoom 0.8s spring 0.5; loop spin 0.5 4s"});
    let after = applied(&p, &run(&p, &[("setMotion", spin)]));
    let m = picture(&after).layout.motion.clone().unwrap();
    let enter = m.enter.unwrap();
    assert_eq!(
        enter.preset,
        klipolo_model::contrib::builtin::motion("zoom")
    );
    assert_eq!(enter.duration_us, secs(0.8));
    assert_eq!(enter.ease, Some(klipolo_chat::tools::motion::spring(0.5)));
    let looping = m.looping.unwrap();
    assert_eq!(
        looping.preset,
        klipolo_model::contrib::builtin::motion("spin")
    );
    assert_eq!(
        looping.params.get("strength"),
        Some(&ParamValue::Number(0.5))
    );
    assert_eq!(looping.duration_us, secs(4.0));
    let typed = json!({"layer": "i1", "keys": null, "presets": "enter typewriter"});
    let out = run(&p, &[("setMotion", typed)]);
    assert_eq!(
        refused_to_model(&out, 0),
        "setMotion: presets: typewriter is for words. Use another preset for this layer."
    );
    // Animated words are words, but they come in by their own units.
    let animated = applied(
        &p,
        &run(
            &p,
            &[(
                "animateText",
                json!({"layer": "t1", "set": [{"key": "opacity", "value": 0}]}),
            )],
        ),
    );
    let typed = json!({"layer": "t1", "keys": null, "presets": "enter typewriter"});
    let out = run(&animated, &[("setMotion", typed)]);
    assert_eq!(
        refused_to_model(&out, 0),
        "setMotion: presets: animated words come in by their own units. Change how with animateText."
    );
}

#[test]
fn set_motion_moves_the_layer_added_just_before_it_or_a_sticker_still_to_be_drawn() {
    let p = project();
    let out = run(
        &p,
        &[
            (
                "addImage",
                json!({"file": "Heart", "size": null, "position": null, "atSec": null, "durationSec": null}),
            ),
            (
                "setMotion",
                json!({"layer": "last", "keys": null, "presets": "loop float"}),
            ),
        ],
    );
    let after = applied(&p, &out);
    let floated = layer(&after, |i| {
        matches!(i.content, ItemContent::Image { .. }) && i.start_us == 0
    });
    assert!(floated.layout.motion.as_ref().unwrap().looping.is_some());
    assert_eq!(out.additions.as_ref().unwrap().label, "chat.andMore");
    let out = run(
        &p,
        &[
            (
                "makeSticker",
                json!({"prompt": "a paper plane", "position": null, "atSec": null, "durationSec": null}),
            ),
            (
                "setMotion",
                json!({"layer": "last", "keys": [{"property": "y", "atSec": 0, "value": 0.05, "ease": null},
                    {"property": "y", "atSec": 1, "value": -0.05, "ease": "inOut"}], "presets": null}),
            ),
        ],
    );
    assert_eq!(out.refusals, vec![]);
    let Some(klipolo_chat::Job::MakeSticker(job)) = out.jobs.first() else {
        panic!("{:?}", out.jobs);
    };
    assert_eq!(
        job.motion.as_ref().unwrap().keys.as_ref().unwrap().y.len(),
        2
    );
}

// ---- animateText ----

#[test]
fn animate_text_reads_its_keys_and_wants_a_value_to_start_from() {
    let parsed = call(
        "animateText",
        json!({"layer": "t1", "set": [
            {"key": "by", "value": "letter"}, {"key": "scale", "value": 0}, {"key": "opacity", "value": 0},
            {"key": "stagger", "value": 0.05}, {"key": "ease", "value": "spring 0.5"}, {"key": "order", "value": "center"},
            {"key": "exit", "value": true}
        ]}),
    );
    let ToolCall::AnimateText {
        motion: Some(m), ..
    } = parsed
    else {
        panic!("{parsed:?}");
    };
    assert_eq!(m.unit, Unit::Letter);
    assert_eq!(
        (m.from.scale, m.from.opacity, m.from.y),
        (Some(0.0), Some(0.0), None)
    );
    assert_eq!(m.stagger_secs, Some(0.05));
    assert_eq!(m.order, Some(text_motion::Order::Center));
    assert!(m.exit);
    assert!(
        refused(
            "animateText",
            json!({"layer": "t1", "set": [{"key": "by", "value": "word"}]})
        )
        .contains("must set where the units start from")
    );
    assert_eq!(
        refused(
            "animateText",
            json!({"layer": "t1", "set": [{"key": "opacity", "value": 3}]})
        ),
        "animateText: set[0].value must be between 0 and 1, not 3."
    );
    assert!(
        refused(
            "animateText",
            json!({"layer": "t1", "set": [{"key": "wobble", "value": 3}]})
        )
        .contains("must be one of by, y, scale")
    );
}

#[test]
fn animated_words_become_a_scene_made_from_the_words_and_their_look() {
    let p = project();
    let out = run(
        &p,
        &[(
            "animateText",
            json!({"layer": "t1", "set": [{"key": "by", "value": "word"}, {"key": "y", "value": 60},
                {"key": "opacity", "value": 0}, {"key": "ease", "value": "out"}]}),
        )],
    );
    assert_eq!(out.additions.as_ref().unwrap().label, "chat.animateText");
    let after = applied(&p, &out);
    let before = words(&p);
    let now = words(&after);
    assert_eq!(
        (now.id.as_str(), now.start_us, now.duration_us),
        (before.id.as_str(), before.start_us, before.duration_us)
    );
    assert_eq!(now.layout.rect, before.layout.rect);
    let ItemContent::Generator { source, params, .. } = &now.content else {
        panic!("{:?}", now.content);
    };
    assert_eq!(source, &ContribId::from(text_motion::SOURCE));
    let (text, style, motion) = text_motion::read(params);
    let ItemContent::Text { style: was, .. } = &before.content else {
        panic!()
    };
    assert_eq!((text.as_str(), &style), ("Day one", was));
    assert_eq!(
        (motion.unit, motion.from.y, motion.from.opacity),
        (Unit::Word, Some(60.0), Some(0.0))
    );
    // Its own fade in goes: the words come in instead. Its fade out stays.
    let m = now.layout.motion.as_ref().unwrap();
    assert!(m.enter.is_none() && m.exit.is_some());
    // The words are still t1, and can be animated again.
    let again = run(
        &after,
        &[(
            "animateText",
            json!({"layer": "t1", "set": [{"key": "by", "value": "letter"}, {"key": "scale", "value": 0}]}),
        )],
    );
    let twice = applied(&after, &again);
    let ItemContent::Generator { params, .. } = &words(&twice).content else {
        panic!()
    };
    assert_eq!(text_motion::read(params).2.unit, Unit::Letter);
}

#[test]
fn words_klipolo_cant_draw_in_time_go_back_to_the_model_with_the_reason() {
    let mut p = project();
    let long = "a".repeat(text_motion::MAX_LETTERS + 5);
    let ops = add_text_ops(&p, TextPreset::Plain, &long, secs(20.0)).unwrap();
    p = apply_ops(&p, &ops).unwrap();
    let letters = json!({"layer": "t2", "set": [{"key": "by", "value": "letter"}, {"key": "opacity", "value": 0}]});
    let out = run(&p, &[("animateText", letters)]);
    assert!(refused_to_model(&out, 0).contains("Animate by word instead"));
    let blurred = json!({"layer": "t1", "set": [{"key": "by", "value": "letter"}, {"key": "blur", "value": 30},
        {"key": "sec", "value": 2}, {"key": "stagger", "value": 0.01}]});
    let p = project();
    let ok = run(&p, &[("animateText", blurred.clone())]);
    // "Day one" is 6 letters: never more than 12 blurring at once.
    assert_eq!(ok.refusals, vec![]);
    let mut p = p;
    let ops = add_text_ops(
        &p,
        TextPreset::Plain,
        "So many letters blurring in at once",
        secs(20.0),
    )
    .unwrap();
    p = apply_ops(&p, &ops).unwrap();
    let mut many = blurred;
    many["layer"] = json!("t2");
    let out = run(&p, &[("animateText", many)]);
    assert_eq!(
        refused_to_model(&out, 0),
        "animateText: too many units would blur at once for Klipolo to draw in time (at most 12). Use a longer stagger, \
         a shorter sec, less blur, or by word."
    );
    let picture = json!({"layer": "i1", "set": [{"key": "opacity", "value": 0}]});
    let out = run(&project(), &[("animateText", picture)]);
    assert!(refused_to_model(&out, 0).contains("is not words"));
}

// ---- setLayer ----

fn set(layer: &str, pairs: Value) -> Value {
    json!({"layer": layer, "set": pairs})
}

#[test]
fn set_layer_reads_its_keys_and_refuses_one_it_doesnt_take() {
    let ToolCall::SetLayer { change, .. } = call(
        "setLayer",
        set(
            "i1",
            json!([{"key": "box", "value": "0.7 0.05 0.25 0.25"}, {"key": "rotation", "value": 15},
                {"key": "opacity", "value": 0.8}, {"key": "corners", "value": 0.2}, {"key": "blend", "value": "screen"},
                {"key": "crop", "value": "none"}, {"key": "order", "value": "back"},
                {"key": "fadeIn", "value": 1}]),
        ),
    ) else {
        panic!()
    };
    assert_eq!(change.rect.unwrap().x, 0.7);
    assert_eq!(
        (
            change.rotation,
            change.opacity,
            change.corners,
            change.blend
        ),
        (Some(15.0), Some(0.8), Some(0.2), Some(Blend::Screen))
    );
    assert_eq!(change.crop, Some(klipolo_model::crop::Crop::NONE));
    assert_eq!(change.fade_in_us, Some(secs(1.0)));
    assert!(
        refused("setLayer", set("i1", json!([{"key": "glow", "value": 1}])))
            .contains("must be one of box,")
    );
    assert!(
        refused(
            "setLayer",
            set("i1", json!([{"key": "box", "value": "0.1 0.2"}]))
        )
        .contains("four numbers: x y width height")
    );
    assert!(
        refused(
            "setLayer",
            set(
                "i1",
                json!([{"key": "opacity", "value": 1}, {"key": "OPACITY", "value": 0}])
            )
        )
        .contains("is set twice")
    );
    assert_eq!(
        refused(
            "styleText",
            set("t1", json!([{"key": "color", "value": "red"}]))
        ),
        "styleText: set[0].value must be a hex colour like #FFD02E, not \"red\"."
    );
    assert!(
        refused("setLayer", set("t1", json!([{"key": "size", "value": 80}])))
            .contains("must be one of box,")
    );
}

#[test]
fn set_layer_places_turns_fades_blends_and_rounds_a_layer_in_one_step() {
    let p = project();
    let out = run(
        &p,
        &[(
            "setLayer",
            set(
                "i1",
                json!([{"key": "box", "value": [0.7, 0.05, 0.25, 0.25]}, {"key": "rotation", "value": -10},
                    {"key": "opacity", "value": 0.5}, {"key": "corners", "value": 0.5}, {"key": "blend", "value": "multiply"}]),
            ),
        )],
    );
    assert_eq!(out.additions.as_ref().unwrap().label, "chat.setLayer");
    let after = applied(&p, &out);
    let l = &picture(&after).layout;
    assert_eq!(
        (l.rect.x, l.rect.y, l.rect.w, l.rect.h),
        (0.7, 0.05, 0.25, 0.25)
    );
    assert_eq!(
        (l.rotation, l.opacity, l.radius, l.blend),
        (-10.0, 0.5, 0.5, Some(Blend::Multiply))
    );
    assert!(l.overrides.is_empty(), "every format follows the box");
    assert_eq!(l.anchor, klipolo_model::layout::Anchor::TopRight);
    // A picture has no crop or Fill, and no fades.
    for (key, value) in [
        ("crop", json!("0.1 0 0 0")),
        ("fill", json!(true)),
        ("fadeIn", json!(1)),
    ] {
        let out = run(
            &p,
            &[("setLayer", set("i1", json!([{"key": key, "value": value}])))],
        );
        assert!(refused_to_model(&out, 0).contains(key), "{key}");
    }
}

#[test]
fn forward_and_back_move_a_layer_onto_the_track_above_or_below() {
    let mut p = project();
    // A second picture over the title, on a track of its own above it.
    let ops = klipolo_model::media::image::add_image_ops(&p, &heart(), secs(1.0)).unwrap();
    p = apply_ops(&p, &ops).unwrap();
    let title_track = |p: &Project| {
        let id = &words(p).id;
        p.timeline
            .tracks
            .iter()
            .position(|t| t.items.iter().any(|i| &i.id == id))
            .unwrap()
    };
    let at = title_track(&p);
    let after = applied(
        &p,
        &run(
            &p,
            &[(
                "setLayer",
                set("t1", json!([{"key": "order", "value": "forward"}])),
            )],
        ),
    );
    assert!(title_track(&after) > at);
    assert_eq!(words(&after).start_us, words(&p).start_us);
    let out = run(
        &after,
        &[(
            "setLayer",
            set("t1", json!([{"key": "order", "value": "forward"}])),
        )],
    );
    assert!(refused_to_model(&out, 0).contains("already in front"));
}

#[test]
fn style_text_restyles_words_as_the_text_card_does() {
    let p = project();
    let out = run(
        &p,
        &[(
            "styleText",
            set(
                "t1",
                json!([{"key": "look", "value": "pop"}, {"key": "size", "value": 120}, {"key": "font", "value": "Anton"},
                    {"key": "align", "value": "left"}, {"key": "italic", "value": true}]),
            ),
        )],
    );
    let after = applied(&p, &out);
    let ItemContent::Text { style, .. } = &words(&after).content else {
        panic!()
    };
    assert_eq!(style.color, "#FFD02E");
    assert_eq!(style.font.family, "Anton");
    assert_eq!(
        (style.size, style.align, style.italic),
        (120.0, TextAlign::Left, true)
    );
    // Pop's medium outline, for the new size.
    assert_eq!(style.outline.as_ref().unwrap().width, 8.4);
    assert!(out.notes.is_empty());
    // A font still to download is said.
    let out = run(
        &p,
        &[(
            "styleText",
            set("t1", json!([{"key": "font", "value": "Poppins"}])),
        )],
    );
    assert_eq!(
        out.notes,
        [
            "Poppins isn't on this Mac yet, so the words show in Inter until you download it over the preview."
        ]
    );
    let out = run(
        &p,
        &[(
            "styleText",
            set("t1", json!([{"key": "font", "value": "Comic Neue"}])),
        )],
    );
    assert!(refused_to_model(&out, 0).contains("no font is named \"Comic Neue\""));
    // Style on a picture is refused; on animated words it changes their settings.
    let out = run(
        &p,
        &[(
            "styleText",
            set("i1", json!([{"key": "bold", "value": true}])),
        )],
    );
    assert!(refused_to_model(&out, 0).contains("is not words"));
    let animated = applied(
        &p,
        &run(
            &p,
            &[(
                "animateText",
                json!({"layer": "t1", "set": [{"key": "opacity", "value": 0}]}),
            )],
        ),
    );
    let out = run(
        &animated,
        &[(
            "styleText",
            set("t1", json!([{"key": "color", "value": "#ff0000"}])),
        )],
    );
    let red = applied(&animated, &out);
    let ItemContent::Generator { params, .. } = &words(&red).content else {
        panic!()
    };
    assert_eq!(text_motion::read(params).1.color, "#FF0000");
}

#[test]
fn fades_and_ducking_go_on_clips_and_music() {
    let p = project();
    let out = run(
        &p,
        &[
            (
                "setLayer",
                set(
                    "c1",
                    json!([{"key": "fadeIn", "value": 1}, {"key": "fadeOut", "value": 2}]),
                ),
            ),
            (
                "setLayer",
                set(
                    "m1",
                    json!([{"key": "fadeOut", "value": 3}, {"key": "duck", "value": false}]),
                ),
            ),
        ],
    );
    let after = applied(&p, &out);
    let clip = &after.timeline.main[0];
    assert_eq!((clip.fade_in_us, clip.fade_out_us), (secs(1.0), secs(2.0)));
    let music = layer(&after, |i| matches!(i.content, ItemContent::Audio { .. }));
    let ItemContent::Audio {
        fade_out_us, duck, ..
    } = &music.content
    else {
        panic!()
    };
    assert_eq!((*fade_out_us, *duck), (secs(3.0), Some(0.0)));
    let out = run(
        &p,
        &[(
            "setLayer",
            set("c1", json!([{"key": "opacity", "value": 0.5}])),
        )],
    );
    assert!(
        refused_to_model(&out, 0).contains("takes only crop, separate, after, fadeIn and fadeOut")
    );
    let out = run(
        &p,
        &[(
            "setLayer",
            set("m1", json!([{"key": "box", "value": "0 0 1 1"}])),
        )],
    );
    assert!(
        refused_to_model(&out, 0)
            .contains("is music, which takes only fadeIn, fadeOut, duck and remove")
    );
}

// ---- setEffects ----

fn effect(target: &str, name: &str, pairs: Value) -> Value {
    json!({"target": target, "name": name, "set": pairs})
}

#[test]
fn set_effects_adds_changes_moves_and_proposes_taking_one_off() {
    let p = project();
    let added = run(
        &p,
        &[
            ("setEffects", effect("i1", "drop-shadow", json!([]))),
            (
                "setEffects",
                effect("i1", "blur", json!([{"key": "radius", "value": 20}])),
            ),
        ],
    );
    let after = applied(&p, &added);
    let fx: Vec<(&str, &str)> = (picture(&after).effects.iter())
        .map(|e| (e.id.as_str(), e.effect.as_str()))
        .collect();
    assert_eq!(
        fx,
        [
            ("drop-shadow", "klipolo.builtin/effect.drop-shadow"),
            ("blur", "klipolo.builtin/effect.blur")
        ]
    );
    assert_eq!(
        picture(&after).effects[1].params.get("radius"),
        Some(&ParamValue::Number(20.0))
    );
    let changed = applied(
        &after,
        &run(
            &after,
            &[(
                "setEffects",
                effect(
                    "i1",
                    "blur",
                    json!([{"key": "radius", "value": 40}, {"key": "at", "value": 0}]),
                ),
            )],
        ),
    );
    let first = &picture(&changed).effects[0];
    assert_eq!(
        (first.id.as_str(), first.params.get("radius")),
        ("blur", Some(&ParamValue::Number(40.0)))
    );
    // Taking one off is a removal: proposed.
    let off = run(
        &changed,
        &[(
            "setEffects",
            effect("i1", "blur", json!([{"key": "remove", "value": true}])),
        )],
    );
    assert!(off.additions.is_none());
    assert_eq!(off.proposed_calls, [0]);
    let tx = &off.proposal.as_ref().unwrap().tx;
    assert_eq!(tx.label, "chat.setEffects");
}

#[test]
fn settings_are_checked_against_the_effects_own_schema() {
    let p = project();
    let out = run(
        &p,
        &[(
            "setEffects",
            effect("i1", "blur", json!([{"key": "radius", "value": 500}])),
        )],
    );
    assert_eq!(
        refused_to_model(&out, 0),
        "setEffects: blur's radius takes a number from 0 to 64."
    );
    let out = run(
        &p,
        &[(
            "setEffects",
            effect("i1", "blur", json!([{"key": "size", "value": 5}])),
        )],
    );
    assert!(
        refused_to_model(&out, 0)
            .contains("blur has no setting \"size\"; its settings are radius.")
    );
    let out = run(&p, &[("setEffects", effect("i1", "sparkles", json!([])))]);
    let said = refused_to_model(&out, 0);
    assert!(
        said.contains("is not an effect") && said.contains("blur") && said.contains("vignette"),
        "{said}"
    );
    let out = run(
        &p,
        &[(
            "setEffects",
            effect(
                "i1",
                "mask",
                json!([{"key": "shape", "value": "Circle"}, {"key": "center", "value": "0.5, 0.4"}]),
            ),
        )],
    );
    let masked = applied(&p, &out);
    let mask = &picture(&masked).effects[0];
    assert_eq!(
        mask.params.get("shape"),
        Some(&ParamValue::Text("circle".into()))
    );
    assert_eq!(
        mask.params.get("center"),
        Some(&ParamValue::Point(klipolo_model::params::PointN {
            x: 0.5,
            y: 0.4
        }))
    );
    let out = run(
        &p,
        &[(
            "setEffects",
            effect("i1", "blur", json!([{"key": "remove", "value": true}])),
        )],
    );
    assert!(refused_to_model(&out, 0).contains("has no blur to remove"));
}

#[test]
fn a_clip_takes_effects_and_a_look_by_its_ref() {
    let p = project();
    let out = run(
        &p,
        &[
            (
                "setEffects",
                effect("c1", "vignette", json!([{"key": "amount", "value": 0.7}])),
            ),
            (
                "setEffects",
                effect("c1", "warm", json!([{"key": "strength", "value": 0.5}])),
            ),
        ],
    );
    let after = applied(&p, &out);
    let clip = &after.timeline.main[0];
    assert_eq!(
        clip.effects[0].effect.as_str(),
        "klipolo.builtin/effect.vignette"
    );
    assert_eq!(
        clip.look.preset.as_ref().map(|l| l.as_str()),
        Some("klipolo.builtin/look.warm")
    );
    let out = run(&p, &[("setEffects", effect("i1", "warm", json!([])))]);
    assert!(refused_to_model(&out, 0).contains("is not an effect"));
}

#[test]
fn a_message_of_several_compose_calls_is_one_undo_step() {
    let p = project();
    let out = run(
        &p,
        &[
            (
                "setMotion",
                json!({"layer": "i1", "keys": null, "presets": "loop float"}),
            ),
            (
                "setLayer",
                set("i1", json!([{"key": "opacity", "value": 0.9}])),
            ),
            ("setEffects", effect("i1", "glow", json!([]))),
            (
                "animateText",
                json!({"layer": "t1", "set": [{"key": "scale", "value": 0}]}),
            ),
        ],
    );
    let tx = out.additions.as_ref().unwrap();
    assert_eq!(tx.label, "chat.andMore");
    let labels: Vec<&str> = out.changes.iter().map(|c| c.code.as_str()).collect();
    assert_eq!(
        labels,
        [
            "chat.setMotion",
            "chat.setLayer",
            "chat.setEffects",
            "chat.animateText"
        ]
    );
}

#[test]
fn a_layer_ref_holds_only_while_the_layers_do_and_a_clips_while_the_timeline_does() {
    let by_ref = call(
        "setLayer",
        set("t1", json!([{"key": "opacity", "value": 1}])),
    );
    assert!(by_ref.names_layer() && !by_ref.names_timeline());
    let last = call(
        "setMotion",
        json!({"layer": "last", "keys": [], "presets": null}),
    );
    assert!(!last.names_layer() && !last.names_timeline());
    let clip = call("setEffects", effect("c1", "blur", json!([])));
    assert!(!clip.names_layer() && clip.names_timeline());
    let words = call(
        "animateText",
        json!({"layer": "t2", "set": [{"key": "y", "value": 40}]}),
    );
    assert!(words.names_layer());
    let _ = TrackKind::Overlay;
}

#[test]
fn the_catalogues_name_exactly_the_effects_looks_and_presets_klipolo_has() {
    use std::collections::BTreeSet;

    use klipolo_model::contrib::{Catalog, ContribKind};
    let catalogues = klipolo_chat::prompt::CATALOGUES;
    assert!(klipolo_chat::prompt::SYSTEM_PROMPT.contains(catalogues));
    let line = |start: &str| {
        (catalogues.lines())
            .find_map(|l| l.strip_prefix(start))
            .unwrap_or_else(|| panic!("no line {start}"))
    };
    // `name (key, key)`, each effect once, up to the list's end.
    let effects = line("- Effects and their settings: ");
    let effects = &effects[..effects.find(").").expect("the list ends") + 1];
    let said: BTreeSet<(String, Vec<String>)> = (effects.split("), "))
        .map(|e| {
            let (name, keys) = e
                .trim_end_matches(')')
                .split_once(" (")
                .expect("name (keys)");
            let mut keys: Vec<String> = keys.split(", ").map(str::to_owned).collect();
            keys.sort();
            (name.to_owned(), keys)
        })
        .collect();
    let looks = line("- Looks, for clips: ");
    let looks: BTreeSet<&str> = looks[..looks.find(" (").unwrap()].split(", ").collect();
    let presets = line("- Presets: ");
    let (mut has_effects, mut has_looks) = (BTreeSet::new(), BTreeSet::new());
    for def in Catalog::builtin().defs() {
        let Some(word) = (def.id.as_str().strip_prefix("klipolo.builtin/"))
            .and_then(|local| local.split_once('.'))
            .map(|(_, word)| word)
        else {
            continue;
        };
        match def.kind {
            ContribKind::Effect => {
                let mut keys: Vec<String> = def.params.iter().map(|p| p.key.clone()).collect();
                keys.sort();
                has_effects.insert((word.to_owned(), keys));
            }
            ContribKind::Look => {
                has_looks.insert(word);
            }
            ContribKind::Motion => {
                let said = if word == "blur-in" { "blur" } else { word };
                assert!(presets.contains(said), "preset {word}");
            }
            _ => {}
        }
    }
    assert_eq!(said, has_effects);
    assert_eq!(looks, has_looks);
    // The fonts named are bundled with Klipolo (`assets/fonts/<family without spaces>`).
    let fonts = line("- Fonts: ");
    for family in fonts.split(", ").filter(|f| !f.starts_with("or ")) {
        let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/fonts")
            .join(family.replace(' ', ""));
        assert!(folder.is_dir(), "{family} is not bundled");
    }
}

#[test]
fn the_context_says_animated_words_keys_and_effects() {
    let p = project();
    let out = run(
        &p,
        &[
            (
                "animateText",
                json!({"layer": "t1", "set": [{"key": "by", "value": "letter"}, {"key": "opacity", "value": 0}]}),
            ),
            (
                "setMotion",
                json!({"layer": "i1", "keys": [{"property": "y", "atSec": 0, "value": 0.1, "ease": null},
                    {"property": "y", "atSec": 1, "value": 0, "ease": "out"}], "presets": null}),
            ),
            ("setEffects", effect("i1", "glow", json!([]))),
        ],
    );
    let after = applied(&p, &out);
    let ctx = klipolo_chat::build_context(
        &after,
        &Speech::default(),
        &Default::default(),
        &Default::default(),
        &View::default(),
        &Library::default(),
    );
    let at = ctx.text.find("Layers:\n").expect("a Layers list");
    let lines: Vec<&str> = ctx.text[at..].lines().skip(1).take(3).collect();
    assert_eq!(
        lines,
        [
            "t1 0.5-3.5 \"Day one\" (words, animated by letter, exit fade)",
            "i1 10.0-15.0 heart.png (picture, keys y, effects glow)",
            "m1 0.0-30.0 lofi.mp3 (music, volume 30%)",
        ]
    );
}

#[test]
fn a_layer_moves_to_a_time_the_context_shows_lasts_as_long_as_asked_and_its_removal_is_proposed() {
    let p = project();
    let out = run(
        &p,
        &[(
            "setLayer",
            set(
                "i1",
                json!([{"key": "start", "value": 0}, {"key": "length", "value": 8}]),
            ),
        )],
    );
    let after = applied(&p, &out);
    let heart = picture(&after);
    assert_eq!((heart.start_us, heart.duration_us), (0, secs(8.0)));
    // A shorter layer loses what it showed: proposed, as every removal is.
    let shorter = run(
        &p,
        &[(
            "setLayer",
            set("i1", json!([{"key": "length", "value": 2.5}])),
        )],
    );
    assert_eq!(shorter.proposed_calls, [0]);
    // A start nothing in the context stands on goes back to the model.
    let out = run(
        &p,
        &[(
            "setLayer",
            set("i1", json!([{"key": "start", "value": 17.3}])),
        )],
    );
    assert!(refused_to_model(&out, 0).contains("setLayer: start 17.3 is not within 0.3 s of"));
    let gone = run(
        &p,
        &[(
            "setLayer",
            set("i1", json!([{"key": "remove", "value": true}])),
        )],
    );
    assert!(gone.additions.is_none());
    assert_eq!(gone.proposed_calls, [0]);
    assert_eq!(gone.proposal.as_ref().unwrap().tx.label, "chat.removeLayer");
    let out = run(
        &p,
        &[(
            "setLayer",
            set(
                "i1",
                json!([{"key": "remove", "value": true}, {"key": "opacity", "value": 1}]),
            ),
        )],
    );
    assert!(refused_to_model(&out, 0).contains("remove takes a layer away, so it goes alone"));
    let music = run(
        &p,
        &[(
            "setLayer",
            set("m1", json!([{"key": "remove", "value": true}])),
        )],
    );
    assert_eq!(music.proposed_calls, [0]);
    let start = call("setLayer", set("i1", json!([{"key": "start", "value": 0}])));
    assert!(start.names_timeline());
}

#[test]
fn a_clip_takes_a_crop_and_its_sound_on_a_layer_of_its_own() {
    let p = project();
    let out = run(
        &p,
        &[(
            "setLayer",
            set(
                "c1",
                json!([{"key": "crop", "value": "0.1 0 0.1 0"}, {"key": "separate", "value": true}]),
            ),
        )],
    );
    let after = applied(&p, &out);
    let clip = &after.timeline.main[0];
    assert_eq!(clip.crop.unwrap().left, 0.1);
    assert!(clip.muted, "its sound plays from its own layer now");
    let sound = layer(&after, |i| i.id.starts_with("c-sound"));
    assert!(matches!(sound.content, ItemContent::Audio { .. }));
    let out = run(
        &p,
        &[("setLayer", set("c1", json!([{"key": "start", "value": 0}])))],
    );
    assert!(refused_to_model(&out, 0).contains("trimClip or cutRange"));
    let out = run(
        &p,
        &[(
            "setLayer",
            set("i1", json!([{"key": "separate", "value": true}])),
        )],
    );
    assert!(refused_to_model(&out, 0).contains("separate is for imported clips"));
}

/// A setLayer that waits in the proposal (a shorter length) is made again on what the additions leave, so a later call
/// in the same message (a float) survives Apply, and the box and the length apply too.
#[test]
fn a_waiting_set_layer_keeps_what_later_calls_in_the_message_did() {
    let p = project();
    let out = run(
        &p,
        &[
            (
                "setLayer",
                set(
                    "i1",
                    json!([{"key": "box", "value": "0.7 0.05 0.25 0.25"}, {"key": "length", "value": 2}]),
                ),
            ),
            (
                "setMotion",
                json!({"layer": "i1", "keys": null, "presets": "loop float"}),
            ),
        ],
    );
    assert_eq!(out.proposed_calls, [0]);
    let added = applied(&p, &out);
    let all = apply_ops(&added, &out.proposal.as_ref().expect("a proposal").tx.ops).unwrap();
    let heart = picture(&all);
    assert_eq!((heart.layout.rect.x, heart.duration_us), (0.7, secs(2.0)));
    assert!(
        heart
            .layout
            .motion
            .as_ref()
            .is_some_and(|m| m.looping.is_some()),
        "the float survives Apply: {:?}",
        heart.layout.motion
    );
}

/// A colour, an angle or a point that doesn't fit says what does, as a range does.
#[test]
fn a_colour_angle_or_point_that_doesnt_fit_says_what_does() {
    let p = project();
    for (name, key, value, said) in [
        (
            "glow",
            "color",
            json!("gold"),
            "setEffects: glow's color takes a hex like #FF8800.",
        ),
        (
            "drop-shadow",
            "angle",
            json!(400),
            "setEffects: drop-shadow's angle takes a number of degrees from -360 to 360.",
        ),
        (
            "mask",
            "center",
            json!("3, 0.5"),
            "setEffects: mask's center takes a point \"x,y\", each from -1 to 2.",
        ),
    ] {
        let out = run(
            &p,
            &[(
                "setEffects",
                effect("i1", name, json!([{"key": key, "value": value}])),
            )],
        );
        assert_eq!(refused_to_model(&out, 0), said, "{name}.{key}");
    }
}

fn run_said(p: &Project, message: &str, calls: &[(&str, Value)]) -> Compiled {
    let calls: Vec<ToolCall> = calls.iter().map(|(n, a)| call(n, a.clone())).collect();
    compile(p, &inputs(p, message), &calls)
}

/// "Change the title to Launch day": styleText's text key gives words new words, the person's own (as addText takes
/// them), on a text layer and on animated words alike.
#[test]
fn style_text_changes_the_words_to_the_persons_own() {
    let p = project();
    let message = "change the title to Launch day";
    let launch = set("t1", json!([{"key": "text", "value": "Launch day"}]));
    let after = applied(&p, &run_said(&p, message, &[("styleText", launch.clone())]));
    let ItemContent::Text { text, .. } = &words(&after).content else {
        panic!()
    };
    assert_eq!(text, "Launch day");
    let out = run_said(&p, "change the title", &[("styleText", launch.clone())]);
    assert!(refused_to_model(&out, 0).contains("the person's own or said in the video"));
    let animated = applied(
        &p,
        &run(
            &p,
            &[(
                "animateText",
                json!({"layer": "t1", "set": [{"key": "opacity", "value": 0}]}),
            )],
        ),
    );
    let relabelled = applied(
        &animated,
        &run_said(&animated, message, &[("styleText", launch)]),
    );
    let ItemContent::Generator { params, .. } = &words(&relabelled).content else {
        panic!()
    };
    assert_eq!(text_motion::read(params).0, "Launch day");
}

/// animateText by none: animated words become plain words again, with their words and look, fading in as words do.
#[test]
fn animated_words_go_back_to_plain_words() {
    let p = project();
    let animated = applied(
        &p,
        &run(
            &p,
            &[(
                "animateText",
                json!({"layer": "t1", "set": [{"key": "opacity", "value": 0}]}),
            )],
        ),
    );
    let plain = run(
        &animated,
        &[(
            "animateText",
            json!({"layer": "t1", "set": [{"key": "by", "value": "none"}]}),
        )],
    );
    assert_eq!(plain.additions.as_ref().unwrap().label, "chat.animateText");
    let after = applied(&animated, &plain);
    let ItemContent::Text { text, style } = &words(&after).content else {
        panic!("{:?}", words(&after).content)
    };
    let ItemContent::Text { style: was, .. } = &words(&p).content else {
        panic!()
    };
    assert_eq!((text.as_str(), style), ("Day one", was));
    assert!(
        words(&after)
            .layout
            .motion
            .as_ref()
            .is_some_and(|m| m.enter.is_some())
    );
    let out = run(
        &p,
        &[(
            "animateText",
            json!({"layer": "t1", "set": [{"key": "by", "value": "none"}]}),
        )],
    );
    assert!(refused_to_model(&out, 0).contains("is plain words already"));
}

/// A clip moves to another place on the main track by setLayer's after key: after another clip, or to the start.
#[test]
fn a_clip_moves_after_another_or_to_the_start() {
    let p = project();
    let ops = [
        klipolo_model::ops::Op::Split { at_us: secs(10.0) },
        klipolo_model::ops::Op::Split { at_us: secs(20.0) },
    ];
    let p = apply_ops(&p, &ops).unwrap();
    let ids =
        |p: &Project| -> Vec<String> { p.timeline.main.iter().map(|c| c.id.clone()).collect() };
    let [a, b, c] = <[String; 3]>::try_from(ids(&p)).unwrap();
    let first = applied(
        &p,
        &run(
            &p,
            &[(
                "setLayer",
                set("c3", json!([{"key": "after", "value": "start"}])),
            )],
        ),
    );
    assert_eq!(ids(&first), [c.clone(), a.clone(), b.clone()]);
    let after = applied(
        &p,
        &run(
            &p,
            &[(
                "setLayer",
                set("c3", json!([{"key": "after", "value": "c1"}])),
            )],
        ),
    );
    assert_eq!(ids(&after), [a.clone(), c.clone(), b.clone()]);
    let out = run(
        &p,
        &[(
            "setLayer",
            set("c3", json!([{"key": "after", "value": "c9"}])),
        )],
    );
    assert!(refused_to_model(&out, 0).contains("there is no clip c9"));
    let out = run(
        &p,
        &[(
            "setLayer",
            set("i1", json!([{"key": "after", "value": "c1"}])),
        )],
    );
    assert!(refused_to_model(&out, 0).contains("after is for clips"));
}

/// A layer splits in two at a time the context shows, as splitAt splits a clip.
#[test]
fn a_layer_splits_at_a_time_the_context_shows() {
    let p = project();
    let mut i = inputs(&p, "split the heart here");
    i.view.playhead_us = secs(12.0);
    let calls = [call(
        "setLayer",
        set("i1", json!([{"key": "split", "value": 12}])),
    )];
    let out = compile(&p, &i, &calls);
    let after = applied(&p, &out);
    let mut hearts: Vec<(Us, Us)> = (after.timeline.tracks.iter())
        .flat_map(|t| &t.items)
        .filter(|i| matches!(i.content, ItemContent::Image { .. }))
        .map(|i| (i.start_us, i.duration_us))
        .collect();
    hearts.sort();
    assert_eq!(hearts, [(secs(10.0), secs(2.0)), (secs(12.0), secs(3.0))]);
    let out = run(
        &p,
        &[("setLayer", set("i1", json!([{"key": "split", "value": 3}])))],
    );
    assert!(refused_to_model(&out, 0).contains("split"));
}

/// A recording of a screen and a camera, played from 10 s of it for 20 s as c1.
fn take() -> Project {
    use klipolo_model::project::{
        AssetKind, Recording, RecordingKind, RecordingSource, SourceMix, SourceRole, SourceRun,
    };
    let file = |id: &str| {
        let mut a = asset(id, "video", &format!("{id}.mov"), 60.0, (1920, 1080), false);
        a.kind = AssetKind::Video;
        a
    };
    let source = |id: &str, role, file: &str| RecordingSource {
        id: id.into(),
        role,
        label: format!("capture.source.{id}"),
        runs: vec![SourceRun {
            asset_id: file.into(),
            offset_us: 0,
            duration_us: secs(60.0),
            drift_ppm: 0.0,
            file_in_us: 0,
        }],
        nudge_us: 0,
        mix: SourceMix::default(),
        synced: None,
    };
    let mut p = Project::new("Take");
    p.assets = vec![file("s1"), file("cam1")];
    p.recordings = vec![Recording {
        id: "rec1".into(),
        kind: RecordingKind::Screen,
        sources: vec![
            source("screen", SourceRole::Screen, "s1"),
            source("camera", SourceRole::Camera, "cam1"),
        ],
        primary: "screen".into(),
        cursor: None,
        markers: Vec::new(),
        game: None,
    }];
    let mut clip = Clip::new(
        ClipSource::Recording { id: "rec1".into() },
        secs(10.0),
        secs(30.0),
    );
    clip.id = "take".into();
    clip.camera = Some(klipolo_model::timeline::CameraLayout::default());
    p.timeline.main = vec![clip];
    p
}

/// setMotion on a recording's clip keys its camera bubble: x, y and scale, from the clip's start, kept in the
/// recording's own time as the bubble's keys are, so cuts keep them on the content. No presets, no turn or fade.
#[test]
fn set_motion_on_a_recordings_clip_keys_its_camera_bubble() {
    let p = take();
    let grow = json!({"layer": "c1", "keys": [
        {"property": "scale", "atSec": 0, "value": 1, "ease": null},
        {"property": "scale", "atSec": 2, "value": 1.5, "ease": "out"}
    ], "presets": null});
    let out = run(&p, &[("setMotion", grow)]);
    let after = applied(&p, &out);
    let bubble = &after.timeline.main[0].camera.as_ref().unwrap().bubble;
    let keys = bubble.motion.as_ref().unwrap().keys.as_ref().unwrap();
    assert_eq!(
        keys.scale,
        [key(10.0, 1.0, None), key(12.0, 1.5, Some(Ease::Power2Out))]
    );
    let floats = json!({"layer": "c1", "keys": null, "presets": "loop float"});
    let out = run(&p, &[("setMotion", floats)]);
    assert!(refused_to_model(&out, 0).contains("the camera bubble takes keys only"));
    let turns = json!({"layer": "c1", "keys": [{"property": "rotation", "atSec": 0, "value": 10, "ease": null}],
        "presets": null});
    let out = run(&p, &[("setMotion", turns)]);
    assert!(refused_to_model(&out, 0).contains("x, y and scale"));
    let out = run(
        &project(),
        &[(
            "setMotion",
            json!({"layer": "c1", "keys": [], "presets": null}),
        )],
    );
    assert!(refused_to_model(&out, 0).contains("has no camera"));
}

fn whoosh() -> Asset {
    asset("whoosh", "audio", "whoosh.wav", 2.0, (0, 0), true)
}

/// `project()` with the Whoosh sound at 4.0 s.
fn with_sound() -> Project {
    let mut p = project();
    p.assets.push(whoosh());
    let ops = klipolo_model::media::sound::add_sound_ops(&p, &whoosh(), secs(4.0)).unwrap();
    apply_ops(&p, &ops).unwrap()
}

fn level(t: f64, v: f64, ease: Option<Ease>) -> Key {
    key(t, v, ease)
}

fn volume_keys(layer: &str, keys: Value) -> Value {
    json!({"layer": layer, "set": [], "volumeKeys": keys})
}

/// setLayer's volumeKeys: a level from 0 to 2 at times from the start, with an ease, refused with the range when a
/// level is out of it; an empty list takes them away.
#[test]
fn volume_keys_are_read_with_their_range() {
    let ToolCall::SetLayer { change, .. } = call(
        "setLayer",
        volume_keys(
            "m1",
            json!([{"atSec": 0, "value": 0.5, "ease": null}, {"atSec": 10, "value": 0.15, "ease": "out"}]),
        ),
    ) else {
        panic!()
    };
    assert_eq!(
        change.volume_keys,
        Some(vec![
            level(0.0, 0.5, None),
            level(10.0, 0.15, Some(Ease::Power2Out))
        ])
    );
    assert_eq!(
        refused(
            "setLayer",
            volume_keys("m1", json!([{"atSec": 0, "value": 3, "ease": null}]))
        ),
        "setLayer: volumeKeys[0].value must be between 0 and 2, not 3."
    );
    assert_eq!(
        refused(
            "setLayer",
            json!({"layer": "m1", "set": [], "volumeKeys": null})
        ),
        "setLayer: set must not be empty."
    );
}

/// "Duck the music between 0:10 and 0:20": the song's keys go on its pieces, from its start; a clip's on the clip, a
/// sound's on its layer; [] takes them away; a key past the end, or keys on a picture, go back to the model.
#[test]
fn volume_keys_go_on_music_clips_and_sounds() {
    let p = with_sound();
    let duck = json!([
        {"atSec": 9.5, "value": 0.3, "ease": null}, {"atSec": 10, "value": 0.1, "ease": "inOut"},
        {"atSec": 20, "value": 0.1, "ease": null}, {"atSec": 20.5, "value": 0.3, "ease": "inOut"}
    ]);
    let out = run(&p, &[("setLayer", volume_keys("m1", duck.clone()))]);
    let after = applied(&p, &out);
    let music = layer(&after, klipolo_model::media::music::is_music);
    assert_eq!(music.content.volume_keys().len(), 4);
    assert_eq!(
        music.content.volume_keys()[1],
        level(10.0, 0.1, Some(Ease::Power2InOut))
    );
    let cleared = applied(
        &after,
        &run(&after, &[("setLayer", volume_keys("m1", json!([])))]),
    );
    assert!(
        layer(&cleared, klipolo_model::media::music::is_music)
            .content
            .volume_keys()
            .is_empty()
    );
    let clip = applied(&p, &run(&p, &[("setLayer", volume_keys("c1", duck))]));
    assert_eq!(clip.timeline.main[0].volume_keys.len(), 4);
    let fade =
        json!([{"atSec": 0, "value": 1, "ease": null}, {"atSec": 2, "value": 0, "ease": "in"}]);
    let sound = applied(&p, &run(&p, &[("setLayer", volume_keys("s1", fade))]));
    let whoosh = layer(
        &sound,
        |i| matches!(&i.content, ItemContent::Audio { asset_id, .. } if asset_id == "whoosh"),
    );
    assert_eq!(whoosh.content.volume_keys().len(), 2);
    let late = json!([{"atSec": 40, "value": 1, "ease": null}]);
    let out = run(&p, &[("setLayer", volume_keys("m1", late))]);
    assert!(refused_to_model(&out, 0).contains("volumeKeys[0].atSec 40.0 is past"));
    let out = run(&p, &[("setLayer", volume_keys("i1", json!([])))]);
    assert!(refused_to_model(&out, 0).contains("volumeKeys are for clips, music and sounds"));
}

/// The context names sound effects (`s1`) and says when a clip, a song or a sound has volume keys.
#[test]
fn the_context_names_sounds_and_says_volume_keys() {
    let p = with_sound();
    let keyed = json!([{"atSec": 0, "value": 1, "ease": null}, {"atSec": 1, "value": 0.5, "ease": null},
        {"atSec": 2, "value": 1, "ease": null}]);
    let out = run(
        &p,
        &[
            ("setLayer", volume_keys("m1", keyed.clone())),
            ("setLayer", volume_keys("c1", keyed.clone())),
            (
                "setLayer",
                volume_keys(
                    "s1",
                    json!([{"atSec": 0, "value": 1, "ease": null}, {"atSec": 1, "value": 0, "ease": null}]),
                ),
            ),
        ],
    );
    let after = applied(&p, &out);
    let ctx = klipolo_chat::build_context(
        &after,
        &Speech::default(),
        &Default::default(),
        &Default::default(),
        &View::default(),
        &Library::default(),
    );
    let text = &ctx.text;
    assert!(
        text.contains("c1 0.0-30.0 talk.mov (volume keyed, 3 keys)"),
        "{text}"
    );
    assert!(
        text.contains("m1 0.0-30.0 lofi.mp3 (music, volume keyed, 3 keys)"),
        "{text}"
    );
    assert!(
        text.contains("s1 4.0-6.0 whoosh.wav (sound, volume keyed, 2 keys)"),
        "{text}"
    );
}

/// As Muse Spark wrote them in a paid run: an ease "spring 0.4" is a spring bouncing 0.4, and the text "null" where a
/// value may be null is null, in presets and in a key's ease alike.
#[test]
fn spring_with_a_bounce_and_the_text_null_read_as_meant() {
    let parsed = call(
        "setMotion",
        json!({"layer": "i1", "presets": "null", "keys": [
            {"atSec": 0.0, "ease": "null", "property": "rotation", "value": -180.0},
            {"atSec": 1.0, "ease": "spring 0.4", "property": "rotation", "value": 0.0}
        ]}),
    );
    let ToolCall::SetMotion {
        keys: Some(keys),
        presets,
        ..
    } = parsed
    else {
        panic!("{parsed:?}");
    };
    assert!(presets.is_empty());
    assert_eq!(keys[0].ease, None);
    assert_eq!(keys[1].ease, Some(klipolo_chat::tools::motion::spring(0.4)));
    let ToolCall::SetLayer { change, .. } = call(
        "setLayer",
        json!({"layer": "m1", "set": [{"key": "duck", "value": true}], "volumeKeys": "null"}),
    ) else {
        panic!()
    };
    assert_eq!((change.duck, change.volume_keys), (Some(true), None));
}
