//! Animated words (`media::text_motion`): the settings a layer stores, read back as they were; the scene they make,
//! checked as any scene is; the limits a text is held to; and the layer itself, which a project validates.

use klipolo_model::contrib::Catalog;
use klipolo_model::font::FontRef;
use klipolo_model::layout::{Anchor, BoxN, Layout};
use klipolo_model::media::text_motion::{
    self, From, MAX_LETTERS, Order, SOURCE, TextMotion, Unit, problem, scene, scene_of,
};
use klipolo_model::motion::Ease;
use klipolo_model::ops::{Op, TrackTarget};
use klipolo_model::scene::{Split, check_scene};
use klipolo_model::timeline::{Item, ItemContent, Outline, TextAlign, TextStyle};
use klipolo_model::{Project, apply_ops};

fn pop_in(unit: Unit) -> TextMotion {
    TextMotion {
        unit,
        from: From {
            scale: Some(0.0),
            opacity: Some(0.0),
            ..From::default()
        },
        unit_secs: Some(0.4),
        ease: Some(Ease::BackOut),
        stagger_secs: Some(0.15),
        order: Some(Order::Start),
        exit: false,
    }
}

fn styled() -> TextStyle {
    TextStyle {
        font: FontRef::inter(),
        size: 96.0,
        weight: 800,
        italic: true,
        color: "#FFD02E".into(),
        background: Some("#121A33DB".into()),
        outline: Some(Outline {
            color: "#000000".into(),
            width: 6.72,
        }),
        shadow: false,
        align: TextAlign::Left,
    }
}

#[test]
fn settings_read_back_as_they_were_stored() {
    let motion = TextMotion {
        from: From {
            y: Some(60.0),
            rotation: Some(-30.0),
            blur: Some(8.0),
            ..pop_in(Unit::Line).from
        },
        exit: true,
        order: Some(Order::Center),
        ..pop_in(Unit::Line)
    };
    let params = text_motion::params("Day one\nin Lisbon", &styled(), &motion);
    let (text, style, read) = text_motion::read(&params);
    assert_eq!(text, "Day one\nin Lisbon");
    assert_eq!(style, styled());
    assert_eq!(read, motion);
    // Every setting is one the definition lists, and each value is one it takes.
    let def = Catalog::builtin().get(&text_motion::source()).expect("in the catalogue");
    assert_eq!(def.id.as_str(), SOURCE);
    for (key, value) in &params {
        let spec = def.param(key).unwrap_or_else(|| panic!("{key} is not a setting"));
        spec.check(value, &Project::new("p")).unwrap_or_else(|e| panic!("{key}: {e:?}"));
    }
}

#[test]
fn settings_left_out_or_unreadable_take_the_defaults() {
    let (text, style, motion) = text_motion::read(&Default::default());
    assert_eq!(text, "Your words");
    assert_eq!(style, TextStyle::default());
    assert_eq!(motion.unit, Unit::Word);
    assert!(motion.from.is_rest());
    let mut params = text_motion::params("Hi", &TextStyle::default(), &pop_in(Unit::Letter));
    params.insert("fromScale".into(), klipolo_model::params::ParamValue::Number(99.0));
    params.insert("ease".into(), klipolo_model::params::ParamValue::Text("wobbly".into()));
    let (_, _, motion) = text_motion::read(&params);
    assert_eq!(motion.from.scale, Some(10.0), "held to its range");
    assert_eq!(motion.ease, None);
}

#[test]
fn each_unit_makes_a_scene_that_checks() {
    for unit in [Unit::Letter, Unit::Word, Unit::Line] {
        for exit in [false, true] {
            let motion = TextMotion {
                exit,
                ..pop_in(unit)
            };
            let s = scene("Make each word pop\nin one by one", &styled(), &motion);
            let facts = check_scene(&s, Catalog::builtin())
                .unwrap_or_else(|p| panic!("{unit:?} exit {exit}: {p:#?}"));
            assert!(facts.animates);
            let split = s.nodes[0].props.split;
            assert_eq!(
                split,
                Some(match unit {
                    Unit::Letter => Split::Chars,
                    Unit::Word => Split::Words,
                    Unit::Line => Split::Lines,
                })
            );
            assert_eq!(s.timeline.len(), if exit { 2 } else { 1 });
            let segments = s.segments.unwrap();
            assert!(segments.enter > 0.0 && (segments.out > 0.0) == exit);
        }
    }
}

#[test]
fn only_the_values_that_move_are_tweened() {
    let s = scene("Hello there", &TextStyle::default(), &pop_in(Unit::Word));
    let vars: Vec<&str> = s.timeline[0].vars.keys().map(String::as_str).collect();
    assert_eq!(vars, ["opacity", "scale"]);
    let rise = TextMotion {
        from: From {
            y: Some(40.0),
            opacity: Some(1.0),
            ..From::default()
        },
        ..pop_in(Unit::Word)
    };
    let s = scene("Hello there", &TextStyle::default(), &rise);
    assert_eq!(
        serde_json::to_value(&s.timeline[0].vars).unwrap(),
        serde_json::json!({"y": "40px"})
    );
}

#[test]
fn a_text_too_long_to_draw_letter_by_letter_is_refused_with_what_to_do() {
    let long = "a".repeat(MAX_LETTERS + 1);
    let said = problem(&long, &pop_in(Unit::Letter)).expect("refused");
    assert!(said.contains("by word"), "{said}");
    assert_eq!(problem(&long, &pop_in(Unit::Word)), None);
    let slow = TextMotion {
        stagger_secs: Some(2.0),
        ..pop_in(Unit::Word)
    };
    let said = problem(&"word ".repeat(20), &slow).expect("too slow");
    assert!(said.contains("stagger"), "{said}");
    // What a stored layer asks past it still draws, by word.
    let mut params = text_motion::params(&long, &TextStyle::default(), &pop_in(Unit::Letter));
    params.insert("by".into(), klipolo_model::params::ParamValue::Text("letter".into()));
    let s = scene_of(&params);
    assert_eq!(s.nodes[0].props.split, Some(Split::Words));
    check_scene(&s, Catalog::builtin()).unwrap();
}

#[test]
fn too_many_letters_blurring_at_once_is_what_the_scene_check_refuses() {
    let blurred = TextMotion {
        from: From {
            blur: Some(20.0),
            ..From::default()
        },
        unit_secs: Some(1.0),
        stagger_secs: Some(0.02),
        ..pop_in(Unit::Letter)
    };
    let s = scene("A long line of letters that blur in", &TextStyle::default(), &blurred);
    let problems = check_scene(&s, Catalog::builtin()).unwrap_err();
    assert!(
        problems.iter().any(|p| p.message.contains("blur")),
        "{problems:#?}"
    );
}

#[test]
fn a_layer_of_animated_words_is_one_a_project_takes() {
    let p = Project::new("p");
    let params = text_motion::params("Day one", &styled(), &pop_in(Unit::Word));
    let item = Item {
        id: "w".into(),
        start_us: 0,
        duration_us: 3_000_000,
        attach: None,
        layout: Layout::new(
            Anchor::TopCenter,
            BoxN {
                x: 0.1,
                y: 0.1,
                w: 0.8,
                h: 0.15,
            },
        ),
        content: ItemContent::generator(text_motion::source(), params),
        effects: Vec::new(),
    };
    let ops = [Op::AddItem {
        track: TrackTarget::Auto,
        item,
        attach: false,
    }];
    let after = apply_ops(&p, &ops).expect("valid");
    assert_eq!(after.timeline.tracks[0].items.len(), 1);
}
