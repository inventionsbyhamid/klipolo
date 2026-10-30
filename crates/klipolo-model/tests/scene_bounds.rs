//! A scene's costs are upper bounds: a fractional setting counts rounded up;
//! a split text's own blur, repeats and centre-out staggers count every unit they can blur; the opacity-0
//! exemption holds only while nothing else writes the opacity; wrapped lines count as many as the layout can make;
//! keys and tweens share the static ranges; and a bound number shows up to 24 characters.

use klipolo_model::contrib::Catalog;
use klipolo_model::scene::{check_scene, Scene, SceneFacts, SceneProblem};

fn checked(json: &str) -> Result<SceneFacts, Vec<SceneProblem>> {
    let s = Scene::from_stored(json.as_bytes()).unwrap_or_else(|e| panic!("{json}: {e:?}"));
    check_scene(&s, Catalog::builtin())
}

fn blurs_too_many(json: &str) -> bool {
    checked(json).is_err_and(|e| e.iter().any(|p| p.code == "scene.blurringUnits"))
}

/// A 20-character split text `#t` with `extra` properties, and a timeline.
fn letters(extra: &str, timeline: &str) -> String {
    format!(
        r#"{{"scene":1,"duration":4,"nodes":[{{"id":"t","type":"text","text":"ABCDEFGHIJKLMNOPQRST","split":"chars"{extra}}}],
        "timeline":[{timeline}]}}"#
    )
}

#[test]
fn a_float_setting_counts_at_its_maximum_rounded_up() {
    let float = |max: &str| {
        format!(r#"{{"key":"k","label":"K","type":"float","min":0,"max":{max},"default":0}}"#)
    };
    let repeat = |max: &str| {
        format!(
            r#"{{"scene":1,"duration":2,"params":[{}],"nodes":[{{"type":"repeat","times":"$k","children":[{{"type":"rect"}}]}}]}}"#,
            float(max)
        )
    };
    assert_eq!(checked(&repeat("0.9")).unwrap().max_instances, 1);
    assert_eq!(checked(&repeat("1.99")).unwrap().max_instances, 2);
    // The repro: a maximum under 1 read as no instances, so 1,000 emitters of 500 passed.
    let nested = format!(
        r#"{{"scene":1,"duration":2,"params":[{}],"nodes":[{{"type":"repeat","times":"$k","children":[
        {{"type":"repeat","times":1000,"children":[{{"type":"emitter","burst":500,"child":{{"type":"rect"}}}}]}}]}}]}}"#,
        float("0.9")
    );
    assert!(checked(&nested).is_err());
}

#[test]
fn a_split_texts_own_blur_blurs_every_unit() {
    assert!(blurs_too_many(&letters(r#","blur":8"#, "")));
    assert!(blurs_too_many(&letters(
        r#","blur":8"#,
        r##"{"to":"#t","y":0,"stagger":0.1}"##
    )));
    // At opacity 0 with nothing else writing it, nothing is drawn.
    assert!(checked(&letters(r#","blur":8,"opacity":0"#, "")).is_ok());
}

#[test]
fn the_opacity_exemption_holds_only_while_nothing_else_writes_the_opacity() {
    let blur_out = r##"{"to":"#t","blur":6,"stagger":0.05}"##;
    // A: the node's own keys raise it while a tween holds the blur.
    let a = letters(
        r#","opacity":0,"animate":{"opacity":[{"t":0,"v":0},{"t":0.2,"v":1}]}"#,
        blur_out,
    );
    assert!(blurs_too_many(&a));
    // B: another tween raises it.
    let b = letters(
        r#","opacity":0,"blur":8"#,
        r##"{"to":"#t","opacity":1,"dur":0.1},{"to":"#t","blur":0,"stagger":0.1}"##,
    );
    assert!(blurs_too_many(&b));
    // C: a modifier moves it, on the node or on a group above it.
    let c = letters(
        r#","opacity":0,"modifiers":{"opacity":{"type":"oscillate","freq":1,"amp":1}}"#,
        blur_out,
    );
    assert!(blurs_too_many(&c));
    let wrapped = r##"{"scene":1,"duration":4,"nodes":[{"type":"group","modifiers":{"opacity":{"type":"wiggle","freq":1,"amp":0.5}},
        "children":[{"id":"t","type":"text","text":"ABCDEFGHIJKLMNOPQRST","split":"chars","opacity":0}]}],
        "timeline":[{"to":"#t","blur":6,"stagger":0.05}]}"##;
    assert!(blurs_too_many(wrapped));
    // Alone, the opacity 0 hides the blurred units.
    assert!(checked(&letters(r#","opacity":0"#, blur_out)).is_ok());
}

#[test]
fn repeats_and_centre_out_staggers_count_their_overlap() {
    // Six units overlap per run; ten repeats make every one of the twenty blur at once at some point.
    let repeat = |n: &str| {
        letters(
            "",
            &format!(r##"{{"to":"#t","blur":6,"opacity":0,"stagger":0.1,"repeat":{n}}}"##),
        )
    };
    assert!(checked(&repeat("0")).is_ok(), "one run: 6 at once");
    assert!(blurs_too_many(&repeat("10")));
    assert!(blurs_too_many(&repeat("-1")));
    // From the centre, units start in pairs: twice as many overlap.
    let from = |from: &str| {
        letters(
            "",
            &format!(
                r##"{{"from":"#t","blur":8,"opacity":0,"dur":0.75,"stagger":{{"each":0.125,"from":"{from}"}}}}"##
            ),
        )
    };
    assert!(checked(&from("start")).is_ok(), "7 at once");
    assert!(blurs_too_many(&from("center")));
    assert!(blurs_too_many(&from("edges")));
}

#[test]
fn split_lines_count_every_line_the_layout_can_wrap_into() {
    // One source line, but the title layout wraps by words and breaks long words (WordOrGlyph): as many lines as
    // characters, at most.
    let words = "lorem ipsum ".repeat(30);
    let json = format!(
        r#"{{"scene":1,"duration":2,"nodes":[{{"type":"text","text":"{words}","wrap":0.2,"split":"lines",
        "animate":{{"blur":[{{"t":0,"v":8}},{{"t":1,"v":0}}]}}}}]}}"#
    );
    assert!(blurs_too_many(&json));
}

#[test]
fn keys_and_tweens_share_the_static_ranges() {
    let refused_at = |json: &str, path: &str| {
        let e = checked(json).expect_err(json);
        assert!(
            e.iter().any(|p| p.path == path && p.code == "scene.value"),
            "{path}: {e:?}"
        );
    };
    let node =
        |props: &str| format!(r#"{{"scene":1,"duration":2,"nodes":[{{"type":"group"{props}}}]}}"#);
    refused_at(
        &node(r#","animate":{"blur":[{"t":0,"v":1000000}]}"#),
        "/nodes/0/animate/blur/0/v",
    );
    refused_at(
        &node(r#","animate":{"perspective":[{"t":0,"v":0}]}"#),
        "/nodes/0/animate/perspective/0/v",
    );
    refused_at(
        &node(r#","animate":{"x":[{"t":0,"v":1e300,"unit":"px"}]}"#),
        "/nodes/0/animate/x/0/v",
    );
    let tween = r##"{"scene":1,"duration":2,"nodes":[{"id":"t","type":"text","text":"Hi"}],
        "timeline":[{"to":"#t","size":1e9},{"fromTo":"#t","start":{"opacity":2},"opacity":1}]}"##;
    refused_at(tween, "/timeline/0/size");
    refused_at(tween, "/timeline/1/start/opacity");
}

#[test]
fn a_bound_number_in_text_counts_24_characters() {
    // A hundred indices of 24 characters each are 2,400, over the scene's 2,000.
    let json = r#"{"scene":1,"duration":2,"nodes":[{"type":"repeat","times":100,"children":[{"type":"text","text":"$i"}]}]}"#;
    let e = checked(json).expect_err("2,400 characters");
    assert!(e.iter().any(|p| p.code == "scene.textTooLong"), "{e:?}");
}
